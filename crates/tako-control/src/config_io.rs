//! config_io — 設定ファイルの安全な読み書き共通部品（Issue #169）
//!
//! projects.yaml が並行 add で全消失した事故（#169）の再発防止部品。
//! 根本原因は三段連鎖:
//!
//! 1. 旧 save が `std::fs::write`（truncate → write の 2 段階）で、並行プロセスに
//!    空 / 部分ファイルが見える窓があった
//! 2. serde_yaml は空文字列・`projects:` だけの部分内容を「0 件」として**成功**パースする
//!    （`#[serde(default)]` + 空ドキュメント = null のため。エラーにならない）
//! 3. read-modify-write に直列化がなく、0 件を読んだプロセスの add が
//!    「add した 1 件だけ」を書き戻して既存全件を消した
//!
//! ここの部品で 3 層を防御する:
//!
//! - [`atomic_write`] — tmp ファイル + rename で書き込みを原子化
//!   （空 / 部分ファイルが並行プロセスから見える窓を構造的に排除）
//! - [`lock_exclusive`] — `<path>.lock` の排他 flock で複数プロセス間の
//!   read-modify-write を直列化
//! - [`atomic_write_with_backup`] — 書き込み前に `.bak.1`〜`.bak.3` の世代バックアップ
//!   （万一の消失時に直前の内容へ即復元できる）
//!
//! 設定ファイル（projects.yaml / profiles/*.yaml / config.yaml）の書き込みは
//! すべてここを経由すること。読み取り側はロック不要（rename により常に
//! 完全なスナップショットが見えるため）。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 世代バックアップの数（`.bak.1`〜`.bak.N`）
const BACKUP_GENERATIONS: u32 = 3;

/// 設定ファイルの排他ロック。Drop で解放される
/// （プロセスが異常終了しても OS がロックを解放するため、デッドロックは残らない）
pub struct ConfigLock {
    /// ロック保持のためだけに生かしておくハンドル
    _file: File,
}

/// `<target>.lock` を排他ロックして返す。他プロセスが保持中ならブロックして待つ。
/// ロックファイル自体は消さない（削除すると別プロセスが新旧 2 つの inode を
/// 別々にロックできてしまい排他が破れるため）
pub fn lock_exclusive(target: &Path) -> Result<ConfigLock, String> {
    let lock_path = sibling_with_suffix(target, ".lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false) // 内容は使わない（ロック専用）。truncate 不要を明示
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("ロックファイルを開けない ({}): {e}", lock_path.display()))?;
    file.lock()
        .map_err(|e| format!("ロックの取得に失敗 ({}): {e}", lock_path.display()))?;
    Ok(ConfigLock { _file: file })
}

/// tmp ファイル + rename によるアトミック書き込み。
/// rename は同一ファイルシステム内で原子的なので、並行プロセスの読み取りには
/// 「旧内容」か「新内容」しか見えない（空・書きかけが見える瞬間がない）。
///
/// tmp 名は **書き込み 1 回ごと**に固有にする（#1313）。pid までしか分けないと、
/// 同一プロセスの 2 スレッドが同じ tmp を共有し、A が rename したあとに B が
/// **本番ファイルになった同じ inode へ書き込む**（B のハンドルは rename に付いていく）。
/// 短い本文が長い本文の先頭を潰した「どちらでもない中身」が本番へ残り、B の rename は
/// 消えた tmp を探して失敗する。[`lock_exclusive`] を取ってから呼ぶ経路は flock が
/// 同一プロセスのスレッドも直列化するので無事だが、ロック無しで直接呼ぶ経路がある
/// （#638 が `shell_integration::write_state_file` で、#625 が `tmux-backend.conf` で
/// 閉じたのと同じ穴）
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let Some(parent) = path.parent() else {
        return Err(format!("親ディレクトリがない: {}", path.display()));
    };
    std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    // pid でプロセス間の衝突を、連番で同一プロセス内の衝突を防ぐ
    // （同一ディレクトリ = 同一 FS なので rename できる）
    let tmp = sibling_with_suffix(
        path,
        &tmp_suffix(
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            legacy_tmp_name(),
        ),
    );
    let write_result = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "一時ファイルの書き込みに失敗 ({}): {e}",
            tmp.display()
        ));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!(
            "rename に失敗 ({} → {}): {e}",
            tmp.display(),
            path.display()
        )
    })
}

/// アトミック書き込み + 世代バックアップ。
/// 既存内容と同一なら何もしない（無変更の save でバックアップを回転させない）
pub fn atomic_write_with_backup(path: &Path, content: &str) -> Result<(), String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    rotate_backups(path)?;
    atomic_write(path, content)
}

/// バックアップパス（`projects.yaml` → `projects.yaml.bak.<generation>`）
pub fn backup_path(path: &Path, generation: u32) -> PathBuf {
    sibling_with_suffix(path, &format!(".bak.{generation}"))
}

/// 既存ファイルを `.bak.1` へ複製し、既存バックアップを 1 世代ずつ繰り下げる。
/// 本体 → `.bak.1` は rename ではなく copy を使う（rename だと本体が一瞬消え、
/// 並行プロセスの load が「ファイル不在 = 0 件」と誤読する窓を作るため）。
/// `atomic_write_with_backup` は毎書き込みで回すが、呼び出し側が独自条件で
/// 世代を残したい場合（layout.json の縮退保存ガード。#177）は単体でも使える
pub fn rotate_backups(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    for generation in (1..BACKUP_GENERATIONS).rev() {
        let from = backup_path(path, generation);
        if from.is_file() {
            let to = backup_path(path, generation + 1);
            std::fs::rename(&from, &to)
                .map_err(|e| format!("バックアップの繰り下げに失敗 ({}): {e}", from.display()))?;
        }
    }
    std::fs::copy(path, backup_path(path, 1))
        .map_err(|e| format!("バックアップの作成に失敗 ({}): {e}", path.display()))?;
    Ok(())
}

/// tmp ファイルの suffix。`seq` が書き込み 1 回ごとに進むので、同一プロセス内で
/// 並行に書いても tmp を奪い合わない。`legacy` は #1313 の A/B（修正前の pid 止まり）。
///
/// **`.tmp.` を含む形は変えない**。共有カタログ（[`crate::config_share`]）が
/// `contains(".tmp.")` で派生ファイルを共有対象から外しているため
fn tmp_suffix(pid: u32, seq: u64, legacy: bool) -> String {
    if legacy {
        return format!(".tmp.{pid}");
    }
    format!(".tmp.{pid}.{seq}")
}

/// #1313 の A/B。`TAKO_1313_LEGACY=1` で**同一バイナリのまま**修正前の tmp 名へ戻す
fn legacy_tmp_name() -> bool {
    std::env::var_os("TAKO_1313_LEGACY").is_some_and(|v| !v.is_empty())
}

/// `path` と同じディレクトリの「ファイル名 + suffix」のパスを作る
fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-config-io-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_replaces_content_and_cleans_tmp() {
        let dir = temp_dir("atomic");
        let path = dir.join("a.yaml");
        atomic_write(&path, "one").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        atomic_write(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        // tmp ファイルが残っていない
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "tmp が残った: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #1313: tmp 名は**書き込み 1 回ごと**に分かれる。修正前（pid 止まり）は
    /// 2 回目の書き込みが同じ名前を使う = 同一プロセスの書き手同士が奪い合う
    #[test]
    fn tmp_suffix_differs_per_write() {
        let first = tmp_suffix(42, 0, false);
        let second = tmp_suffix(42, 1, false);
        assert_ne!(first, second, "書き込みごとに tmp 名が変わること");
        // 修正前の形。pid が同じなら連番が進んでも同じ名前になる
        assert_eq!(
            tmp_suffix(42, 0, true),
            tmp_suffix(42, 1, true),
            "A/B の legacy アームは修正前（プロセス単位まで）の名前へ戻ること"
        );
        // 共有カタログが派生ファイルを外す手がかり（`.tmp.`）を保つこと
        assert!(first.contains(".tmp."), "{first}");
        assert!(!first.contains(std::path::MAIN_SEPARATOR), "{first}");
    }

    #[test]
    fn backup_rotates_up_to_three_generations() {
        let dir = temp_dir("backup");
        let path = dir.join("a.yaml");
        for content in ["v1", "v2", "v3", "v4", "v5"] {
            atomic_write_with_backup(&path, content).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v5");
        assert_eq!(
            std::fs::read_to_string(backup_path(&path, 1)).unwrap(),
            "v4"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path, 2)).unwrap(),
            "v3"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path, 3)).unwrap(),
            "v2"
        );
        // 4 世代目は作られない
        assert!(!backup_path(&path, 4).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_content_skips_write_and_backup() {
        let dir = temp_dir("unchanged");
        let path = dir.join("a.yaml");
        atomic_write_with_backup(&path, "same").unwrap();
        atomic_write_with_backup(&path, "same").unwrap();
        // 内容が同じなら bak は生まれない
        assert!(!backup_path(&path, 1).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_exclusive_serializes_two_holders() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let dir = temp_dir("lock");
        let path = dir.join("a.yaml");
        let inside = Arc::new(AtomicBool::new(false));

        let lock1 = lock_exclusive(&path).unwrap();
        inside.store(true, Ordering::SeqCst);

        let inside2 = Arc::clone(&inside);
        let path2 = path.clone();
        let handle = std::thread::spawn(move || {
            // flock はハンドル単位なので同一プロセス内の別 open でも排他される
            let _lock2 = lock_exclusive(&path2).unwrap();
            // ロックが取れた時点で先行保持者は解放済みのはず
            assert!(
                !inside2.load(Ordering::SeqCst),
                "排他ロック中に第二の保持者が進入した"
            );
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        inside.store(false, Ordering::SeqCst);
        drop(lock1);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

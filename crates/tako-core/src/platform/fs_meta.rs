//! ファイルのメタデータの OS 差（抽象境界 B37。Issue #2861。B28 の `fs_copy` / `fs_move` の隣）
//!
//! ファイルの**同一性**・**状態変更時刻**・**実行ビット**・**symlink**・**置き換えて保存する**の
//! 5 つは、unix ではファイルの属性（`st_dev` / `st_ino` / `st_ctime` / mode / `symlink(2)` /
//! 同じフォルダの rename）で表せるが、Windows には同じ形のものが無い。呼び出し側
//! （usage の索引・利用者タスクの置き場の版・remote daemon の世代・言語サーバの取得・
//! `tako setup bootstrap`・`tako setup` の CLI の置き場・エディタの保存・テストの偽の実行ファイル）に
//! `cfg` を書かず、この境界の口を 1 本呼ぶ（`platform` の原則）。
//!
//! ## Windows で縮退している口（#2861 では挙動を変えずに移しただけ）
//!
//! | 口 | unix | Windows |
//! |---|---|---|
//! | [`identity`] | `Some((st_dev, st_ino))` | **`None`**（未実装。呼び手は 0 で埋める = 比べると常に等しい = 同一性を比較に使わない。大きさと更新時刻が同じまま差し替わったファイルを見分けられない） |
//! | [`change_time_secs`] | `Some(st_ctime)` | **`None`**（未実装） |
//! | [`is_executable`] | 通常ファイル かつ mode に実行ビット | 通常ファイルか**だけ**（実行ビットが無い。拡張子は見ない） |
//! | [`make_executable`] / [`EXECUTABLE_BY_MODE`] | mode を `0o755` に / `true` | **何もしない** / `false`（拡張子で決まる。shebang の台本は起こせない） |
//! | [`make_symlink`] | `Some(symlink(2) の結果)` | **`None`**（張らない。開発者モードか管理者の権限が要る） |
//! | [`save_replacing`] | 一時ファイル + 権限の引き継ぎ + rename（途中で落ちても元のファイルは残る） | **その場で上書き**（truncate。途中で落ちると中身が欠ける） |
//!
//! Windows の実装（`GetFileInformationByHandle` のボリュームのシリアル + ファイル番号ほか）は
//! 別 Issue。縮退を `Option` / 定数で表すので、呼び手が Windows の値を「本物の同一性」と
//! 取り違えることは型で起きない。

use std::io::Write;
use std::path::Path;

/// ファイルの同一性（unix の `(st_dev, st_ino)`）。同じパスのまま中身が差し替わった
/// （一時ファイルからの rename・別のファイルの移し込み）ことを、大きさと更新時刻が同じでも言える
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    /// ボリューム（unix の `st_dev`）
    pub dev: u64,
    /// inode（unix の `st_ino`）
    pub ino: u64,
}

/// ファイルの同一性。**Windows は `None`**（未実装 = 比較に使わない）。
///
/// `m` は `std::fs::metadata`（symlink を辿る）でも `symlink_metadata`（辿らない）でもよい
/// （どちらの属性を読むかは呼び手が決める）
#[cfg(unix)]
pub fn identity(m: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt as _;
    Some(FileIdentity {
        dev: m.dev(),
        ino: m.ino(),
    })
}

/// ファイルの同一性。**Windows は `None`**（未実装 = 比較に使わない）
#[cfg(not(unix))]
pub fn identity(_: &std::fs::Metadata) -> Option<FileIdentity> {
    None
}

/// 状態変更時刻（unix の `st_ctime`。epoch 秒）。**Windows は `None`**（未実装）
#[cfg(unix)]
pub fn change_time_secs(m: &std::fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt as _;
    Some(m.ctime())
}

/// 状態変更時刻（unix の `st_ctime`。epoch 秒）。**Windows は `None`**（未実装）
#[cfg(not(unix))]
pub fn change_time_secs(_: &std::fs::Metadata) -> Option<i64> {
    None
}

/// 実行できるかをファイルの mode（実行ビット）で決める OS か。
///
/// `true`（unix）の OS では [`make_executable`] の後のファイル（shebang の台本を含む）を
/// そのまま起こせる。`false`（Windows）は拡張子で決まり、[`make_executable`] は何もしない
#[cfg(unix)]
pub const EXECUTABLE_BY_MODE: bool = true;

/// 実行できるかをファイルの mode（実行ビット）で決める OS か（Windows は `false` = 拡張子で決まる）
#[cfg(not(unix))]
pub const EXECUTABLE_BY_MODE: bool = false;

/// 実行できる通常ファイルか（symlink は辿る）。unix は mode の実行ビットを見る
#[cfg(unix)]
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// 実行できる通常ファイルか。**Windows は通常ファイルかだけを見る**（実行ビットが無い。拡張子は見ない）
#[cfg(not(unix))]
pub fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 書いたファイルを実行できるようにする（unix だけ。Windows は拡張子で決まる）
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// `target` を指す symlink を `link` に張る。
///
/// **Windows は `None`**（張らない = 開発者モードか管理者の権限が要る）。呼び手は `None` を
/// 「この OS では張らない」と読み、黙って成功させないこと
#[cfg(unix)]
pub fn make_symlink(target: &Path, link: &Path) -> Option<std::io::Result<()>> {
    Some(std::os::unix::fs::symlink(target, link))
}

/// `target` を指す symlink を `link` に張る。**Windows は `None`**（張らない）
#[cfg(not(unix))]
pub fn make_symlink(_target: &Path, _link: &Path) -> Option<std::io::Result<()>> {
    None
}

/// `path` の中身を `bytes` に置き換えて保存する（エディタの保存・ワークスペースの置換）。
///
/// unix は同じフォルダの一時ファイルへ書いて `sync` し、元のファイルの権限を引き継いでから
/// rename する（途中で落ちても元のファイルは残る）。元のファイルが無ければ一時ファイルの既定の
/// 権限のまま置く（#1659）
#[cfg(unix)]
pub fn save_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".{name}.tako-save-{}-{nonce}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        match std::fs::metadata(path) {
            Ok(metadata) => std::fs::set_permissions(&temp, metadata.permissions())?,
            // 外で消されたファイルを上書き保存で作り直す（#1659）。引き継ぐ権限が無いので
            // 一時ファイルの既定（新規ファイルと同じ）のまま置く
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// `path` の中身を `bytes` に置き換えて保存する。**Windows はその場で上書きする**
/// （一時ファイル + rename を使わない = 途中で落ちると中身が欠ける）
#[cfg(not(unix))]
pub fn save_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // `create`: 外で消されたファイルを上書き保存で作り直す（#1659）。
    // 消えたファイルへの通常の保存は `save_inner` が書く前に断る
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_residue::ScratchDir;

    #[test]
    fn 同一性は差し替えで変わり同じファイルでは変わらない() {
        let dir = ScratchDir::new("fs-meta-identity");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let first = identity(&std::fs::metadata(&path).unwrap());
        assert_eq!(first, identity(&std::fs::metadata(&path).unwrap()));
        // 同じ大きさのまま一時ファイルから差し替える
        let temp = dir.path().join("b.txt");
        std::fs::write(&temp, "y").unwrap();
        std::fs::rename(&temp, &path).unwrap();
        let second = identity(&std::fs::metadata(&path).unwrap());
        if cfg!(unix) {
            assert!(first.is_some() && second.is_some());
            assert_ne!(first, second, "差し替えたのに同一性が変わらない");
        } else {
            assert_eq!((first, second), (None, None), "Windows は未実装 = None");
        }
    }

    #[test]
    fn 状態変更時刻はunixだけ読める() {
        let dir = ScratchDir::new("fs-meta-ctime");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let ctime = change_time_secs(&std::fs::metadata(&path).unwrap());
        assert_eq!(ctime.is_some(), cfg!(unix));
    }

    #[test]
    fn 実行ビットを付けると実行できると判定される() {
        let dir = ScratchDir::new("fs-meta-exec");
        let path = dir.path().join("run.sh");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        assert_eq!(is_executable(&path), !EXECUTABLE_BY_MODE, "付ける前");
        make_executable(&path).unwrap();
        assert!(is_executable(&path), "付けた後");
        // フォルダと存在しないパスは実行できない
        assert!(!is_executable(dir.path()));
        assert!(!is_executable(&dir.path().join("none")));
    }

    #[test]
    fn symlinkはunixだけ張る() {
        let dir = ScratchDir::new("fs-meta-symlink");
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("link");
        let made = make_symlink(&target, &link);
        assert_eq!(made.is_some(), cfg!(unix), "張るのは unix だけ");
        if let Some(result) = made {
            result.unwrap();
            assert_eq!(std::fs::read_link(&link).unwrap(), target);
            // 既に在れば失敗する（上書きしない）
            assert!(make_symlink(&target, &link).unwrap().is_err());
        }
    }

    #[test]
    fn 置き換え保存は中身を替え権限を引き継ぐ() {
        let dir = ScratchDir::new("fs-meta-save");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "old").unwrap();
        make_executable(&path).unwrap();
        save_replacing(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(is_executable(&path), "権限を引き継いでいない");
        // 一時ファイルを残さない
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // 消えたファイルは作り直す（#1659）
        std::fs::remove_file(&path).unwrap();
        save_replacing(&path, b"again").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "again");
    }
}

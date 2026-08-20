//! owner — 器のセッション単位のオーナー記録（#519 M2。設計 §8.1 の I3 = #177）
//!
//! ## なぜ tako 側に記録が要るのか
//!
//! tmux 版の復元強奪ガード（#177）は「守るべき資源そのもの」= 器を握っている
//! クライアントを **器に尋ねて** 判定していた（`list-clients -F '#{client_pid}…'` →
//! 祖先辿りで所有 tako を特定）。psmux ではこれが成立しない:
//!
//! - `list-clients` が `-F`（書式指定）を無視して人間向け書式を返す。
//!   **クライアント PID が取れない**（2026-07-27 実測）
//! - `new-session -D` が他クライアントを切り離さない。tmux なら「最新インスタンスへ
//!   収束」する場面で、psmux は **2 つの tako が同じシェルへ同時に attach したまま**になる
//!
//! 観測できないまま復元すると、片方の close が他方のシェルを殺し、両者の layout 保存が
//! 殴り合う。これは #113 / #177 / #381 で 4 回踏んだ事故クラスそのものなので、
//! 器に尋ねられないなら **tako 側で記録する**。
//!
//! ## 仕組み: OS のファイルロックを「生存の証明」に使う
//!
//! `<data_dir>/backend-owners/<session>.<pid>.owner` を作り、所有インスタンスが
//! **プロセスの生存中ずっと排他ロックを保持**する（`std::fs::File::try_lock`。
//! unix = flock / Windows = LockFileEx）。
//!
//! - ロックが取れない = 生きた誰かが握っている（PID の照合も、プロセス名の判定も要らない）
//! - tako が異常終了してもハンドルは OS が閉じる = ロックは必ず解放される。
//!   **死んだインスタンスの記録が居座らない**のが PID 記録に対する決定的な利点
//! - 誰が握っているかは **ファイル名の PID** で分かる（ロック中のファイルは
//!   Windows では内容を読めないため、内容ではなく名前に載せる）
//!
//! tmux 実装はこの仕組みを使わない（`list-clients` が正しく動くので従来どおり。
//! macOS の挙動を 1 ミリも変えないため）。

use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::SessionRef;

/// オーナー記録ファイルの拡張子
const SUFFIX: &str = ".owner";

/// `claim` の結果
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// このプロセスが所有者になった（新規取得 or 既に保持中）
    Ours,
    /// **生きた別プロセス**が握っている。復元強奪ガードが発動すべき状態
    Foreign(u32),
    /// 記録できない（data_dir が無い・名前が記録に使えない等）。
    /// 器の割り当て自体は止めない（記録は安全網であってゲートではない）
    Unavailable,
}

/// セッション単位のオーナー記録。
///
/// **ディレクトリを引数に取る**のはテスト容易性のため（プロセス全体の記録は
/// [`records`] のシングルトンが `<data_dir>/backend-owners` を指す）。
pub struct OwnerRecords {
    dir: PathBuf,
    /// 保持中のロックハンドル。**閉じるとロックが解放される**ので生かしておく
    held: Mutex<HashMap<String, File>>,
}

impl OwnerRecords {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            held: Mutex::new(HashMap::new()),
        }
    }

    /// セッションの所有権を主張する。既に生きた別プロセスが握っていれば
    /// [`Claim::Foreign`] を返す（**奪わない**）
    pub fn claim(&self, session: &SessionRef) -> Claim {
        self.claim_as(session, std::process::id())
    }

    /// PID を明示しての主張。**テスト専用の入口**（「別インスタンスが握っている」状態を
    /// 1 プロセス内で作れるようにする。ロックはハンドル単位なので同一プロセスでも排他される）
    pub fn claim_as(&self, session: &SessionRef, pid: u32) -> Claim {
        let Some(stem) = record_stem(session) else {
            return Claim::Unavailable;
        };
        {
            let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            if held.contains_key(&stem) {
                return Claim::Ours;
            }
        }
        // 先に他インスタンスの保持を確認する。**奪う手段は持たない**
        if let Some(other) = self.locked_pid(&stem) {
            return Claim::Foreign(other);
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return Claim::Unavailable;
        }
        // 死んだインスタンスが残した記録（ロックされていない同一セッションのファイル）を掃除する。
        // ファイル名に PID が入るため、掃除しないとペインを開くたびにゴミが積もる
        self.sweep(&stem);
        let path = self.dir.join(format!("{stem}.{pid}{SUFFIX}"));
        let Ok(file) = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
        else {
            return Claim::Unavailable;
        };
        match file.try_lock() {
            Ok(()) => {
                self.held
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(stem, file);
                Claim::Ours
            }
            // 同名 PID のファイルを他プロセスが握っている（PID 再利用）。奪わない
            Err(TryLockError::WouldBlock) => Claim::Foreign(pid),
            Err(TryLockError::Error(_)) => Claim::Unavailable,
        }
    }

    /// 所有権を手放す（器を kill したとき）。
    /// **プロセス終了時に呼ぶ必要はない**（ハンドルが閉じてロックは自動解放される）
    pub fn release(&self, session: &SessionRef) {
        let Some(stem) = record_stem(session) else {
            return;
        };
        let file = self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&stem);
        if let Some(file) = file {
            let _ = file.unlock();
            drop(file);
        }
        self.sweep(&stem);
    }

    /// このセッションを**生きたプロセス**が握っていれば、その PID。
    /// 自分自身が握っている場合も自分の PID を返す（呼び出し側が除外する）
    pub fn holder(&self, session: &SessionRef) -> Option<u32> {
        let stem = record_stem(session)?;
        self.locked_pid(&stem)
    }

    /// 指定セッション群のうち、**自分以外の生きたプロセス**が握っているもの
    pub fn foreign_holders(&self, sessions: &[SessionRef]) -> Vec<(SessionRef, u32)> {
        let me = std::process::id();
        sessions
            .iter()
            .filter_map(|s| {
                let pid = self.holder(s)?;
                (pid != me).then(|| (s.clone(), pid))
            })
            .collect()
    }

    /// ロックされている記録ファイルの PID（= 生きた保持者）。無ければ `None`
    fn locked_pid(&self, stem: &str) -> Option<u32> {
        for (pid, path) in self.records_for(stem) {
            match File::open(&path) {
                // 開けてロックも取れる = 保持者は死んでいる（ロックは即座に返す）
                Ok(file) => match file.try_lock() {
                    Ok(()) => {
                        let _ = file.unlock();
                    }
                    Err(TryLockError::WouldBlock) => return Some(pid),
                    // 読み取り自体が拒まれる = 誰かが握っている可能性が高い。
                    // 「握られていない」と誤断定しない側へ倒す
                    Err(TryLockError::Error(_)) => return Some(pid),
                },
                Err(_) => return Some(pid),
            }
        }
        None
    }

    /// ロックされていない（= 死んだインスタンスの）記録ファイルを消す
    fn sweep(&self, stem: &str) {
        for (_, path) in self.records_for(stem) {
            let held = self
                .held
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .any(|f| f.metadata().is_ok() && same_file(f, &path));
            if held {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            if file.try_lock().is_ok() {
                let _ = file.unlock();
                drop(file);
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// `<stem>.<pid>.owner` に一致するファイルの (pid, path) 一覧
    fn records_for(&self, stem: &str) -> Vec<(u32, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some((base, pid)) = parse_record_name(name) else {
                continue;
            };
            if base == stem {
                out.push((pid, entry.path()));
            }
        }
        out
    }
}

/// 同一ファイルか（sweep が自分の保持ハンドルを消さないための保険）。
/// メタデータ比較は移植性のため長さと更新時刻に留める（厳密な inode 比較は不要:
/// 保持中ファイルはそもそもロックが取れないので二重の安全網）
fn same_file(file: &File, path: &Path) -> bool {
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    }
}

/// 記録ファイル名 `<session>.<pid>.owner` を分解する（純関数）
fn parse_record_name(name: &str) -> Option<(&str, u32)> {
    let rest = name.strip_suffix(SUFFIX)?;
    let (base, pid) = rest.rsplit_once('.')?;
    Some((base, pid.parse().ok()?))
}

/// セッション名を記録ファイル名に使える形か検査する（純関数）。
/// パス区切り・`.`・記号を含む名前でディレクトリ外へ出さないための門番。
/// tako が払い出す名前は `tako-<12 桁 16 進>` なので通常はそのまま通る
fn record_stem(session: &SessionRef) -> Option<String> {
    let name = session.as_str();
    let safe = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_records(tag: &str) -> (OwnerRecords, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tako-owner-test-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (OwnerRecords::new(dir.clone()), dir)
    }

    fn session(name: &str) -> SessionRef {
        SessionRef::new(name).unwrap()
    }

    #[test]
    fn 主張すると自分が保持者になり解放で消える() {
        let (records, dir) = temp_records("basic");
        let s = session("tako-000000000001");
        assert_eq!(records.claim(&s), Claim::Ours);
        assert_eq!(records.holder(&s), Some(std::process::id()));
        // 自分の保持は「他インスタンス」に数えない
        assert!(records.foreign_holders(std::slice::from_ref(&s)).is_empty());
        // 二度目の主張も自分のまま（再 spawn・復元で二重に呼ばれても壊れない）
        assert_eq!(records.claim(&s), Claim::Ours);

        records.release(&s);
        assert_eq!(records.holder(&s), None);
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "解放後に記録ファイルが残らない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **#519 M2 の受け入れ条件 7**: 生きた別インスタンスが握っているセッションは
    /// 「他インスタンスの所有」として観測できる。
    /// ロックはハンドル単位なので、別 PID を名乗って主張すれば 1 プロセス内で再現できる
    #[test]
    fn 生きた別インスタンスの保持を検出する() {
        let (records, dir) = temp_records("foreign");
        let s = session("tako-000000000002");
        let other_pid = 424242;
        assert_eq!(records.claim_as(&s, other_pid), Claim::Ours);

        // 別の記録インスタンス（= 別プロセスの tako 相当）から見ると保持されている
        let observer = OwnerRecords::new(dir.clone());
        assert_eq!(observer.holder(&s), Some(other_pid));
        assert_eq!(
            observer.foreign_holders(std::slice::from_ref(&s)),
            vec![(s.clone(), other_pid)]
        );
        // 主張しても奪えない
        assert_eq!(observer.claim(&s), Claim::Foreign(other_pid));

        records.release(&s);
        assert_eq!(observer.holder(&s), None, "解放後は保持者なし");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 死んだインスタンスの記録は「保持されていない」と判定され、掃除される。
    /// ここが PID を書いたファイルを置くだけの実装との決定的な差
    /// （tako がクラッシュしても記録が居座らない）
    #[test]
    fn 死んだインスタンスの記録は保持と見なさず掃除する() {
        let (records, dir) = temp_records("stale");
        std::fs::create_dir_all(&dir).unwrap();
        let s = session("tako-000000000003");
        // ロックされていない記録ファイル = 前回起動の残骸
        let stale = dir.join("tako-000000000003.999999.owner");
        std::fs::write(&stale, b"").unwrap();

        assert_eq!(records.holder(&s), None, "ロックが無ければ保持者ではない");
        assert_eq!(records.claim(&s), Claim::Ours, "残骸は主張を妨げない");
        assert!(!stale.exists(), "残骸は掃除される");
        assert_eq!(records.holder(&s), Some(std::process::id()));

        records.release(&s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 記録に使えない名前は記録できないとして扱う() {
        let (records, dir) = temp_records("unsafe");
        // ディレクトリを外れる形の名前は門前払い（`.` を含む名前は記録名の分解を壊す）
        let s = SessionRef::new("tako.evil").unwrap();
        assert_eq!(records.claim(&s), Claim::Unavailable);
        assert_eq!(records.holder(&s), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 記録ファイル名の分解は純関数として正しい() {
        assert_eq!(
            parse_record_name("tako-abc.1234.owner"),
            Some(("tako-abc", 1234))
        );
        assert_eq!(parse_record_name("tako-abc.owner"), None);
        assert_eq!(parse_record_name("tako-abc.x.owner"), None);
        assert_eq!(parse_record_name("tako-abc.1234.txt"), None);
    }
}

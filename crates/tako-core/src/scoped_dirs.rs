//! 検証起動の GUI が env へ自分で当てた置き場の後片付け（#2686）。
//!
//! 隔離起動（`TAKO_ISOLATED=1`）とセルフテストの GUI は、利用者が置き場を指定しなかった env に、
//! 自分の pid 入りの一時の置き場（`tako-iso-data-<pid>`・`tako-iso-pane-logs-<pid>`・
//! `tako-st-workers-<pid>.yaml` ほか 10 個）を当てる（#177 / #112 / #658）。これは検証のために GUI が
//! 作ったもので、プロセスが終われば誰も使わない（後の起動は pid が違うので別の置き場を作る）。
//! 以前は成功・失敗・SIGTERM のどの出口でも残り、検証のたびに一時 dir へ積もった（#2686 で実測）。
//!
//! **明示 = 残す・暗黙 = 片付ける**。**本番（検証プロセスでない）の GUI では何もしない**
//! （門は [`crate::paths::is_verification_process`]）:
//! - 当てる口 [`adopt`]: env が無いときだけ当てて覚える。利用者が渡した値（`TAKO_DATA_DIR` /
//!   `TAKO_PANE_LOG_DIR` ほか）は上書きも記録もしない
//! - 消す口 [`remove_adopted`]: 覚えたもののうち [`removable`]（名前が自分の pid のもの・いまも env が
//!   その値のもの）だけを、付属ファイル（`<名前>.bak.N` / `<名前>.lock` ほか）ごと消す。呼ぶのは
//!   tako-app の終了の後片付け `cleanup_process_scoped` の 1 実装（成功・SIGTERM・セルフテストの
//!   FAILED・visual-test の出口が共有する）
//! - 消した後は [`is_retired`] が真を返し、書き手（`tako_control::diag` の診断ログ・セッションカタログの
//!   書き出し）は書こうとした行を捨てて置き場を作り直さない（診断ログの書き手は親 dir を
//!   `create_dir_all` で作るので、片付けの後の 1 行で data dir が生き返る）
//!
//! A/B は `TAKO_2686_LEGACY=1`（覚えない = 何も消さない = 修正前）。番犬は
//! `crates/tako-control/tests/issue2686_scoped_dirs_watchdog.rs`。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// GUI が env へ自分で当てた置き場 1 つ
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    /// 当てた env の名前（`TAKO_DATA_DIR` など）
    pub key: &'static str,
    /// 当てた値
    pub path: PathBuf,
}

/// 自分で決めた置き場の名前の頭（隔離起動 = `tako-iso-`・セルフテスト = `tako-st-`）
const OWN_PREFIXES: [&str; 2] = ["tako-iso-", "tako-st-"];

static ADOPTED: Mutex<Vec<Adopted>> = Mutex::new(Vec::new());
static RETIRED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// A/B（`TAKO_2686_LEGACY=1`）: 覚えない = 何も消さない（修正前）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_2686_LEGACY").is_some_and(|v| v == "1")
}

/// 覚える・消す・退役を見るのは検証プロセスだけ（本番の GUI は 1 ビットも変えない）
fn active() -> bool {
    crate::paths::is_verification_process() && !legacy()
}

/// `key` が env に無ければ `path` を当てる。当てたら `true`。検証プロセスなら自分で決めた置き場として覚える。
///
/// **利用者が渡した値には触らない**（空の値も「渡した」と読む = 修正前の `var_os(..).is_none()` と同じ）。
/// 起動の最初（スレッドを起こす前）に呼ぶ: env の書き換えは別スレッドの読みと競る
pub fn adopt(key: &'static str, path: PathBuf) -> bool {
    if std::env::var_os(key).is_some() {
        return false;
    }
    std::env::set_var(key, &path);
    if active() {
        lock(&ADOPTED).push(Adopted { key, path });
    }
    true
}

/// 覚えた置き場の写し
pub fn adopted() -> Vec<Adopted> {
    lock(&ADOPTED).clone()
}

/// `path` が覚えた置き場（かその下）か。終了の後片付けが、置き場の外の物（data dir から逃がした
/// IPC ソケットの短縮パス）の持ち主を確かめるのに使う
pub fn is_adopted(path: &Path) -> bool {
    lock(&ADOPTED).iter().any(|a| path.starts_with(&a.path))
}

/// 消してよいか（**純関数**）: 名前が自分の pid の置き場（`tako-iso-…-<pid>` / `tako-st-…-<pid>`。
/// ファイルは拡張子の前）で、いまも env が当てた値のまま（途中で利用者の値へ差し替わっていない）
pub fn removable(adopted: &Adopted, pid: u32, env_now: Option<&OsStr>) -> bool {
    owned_name(&adopted.path, pid) && env_now == Some(adopted.path.as_os_str())
}

/// 名前が自分の pid の置き場か（**純関数**）
pub fn owned_name(path: &Path, pid: u32) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let stem = name.strip_suffix(".yaml").unwrap_or(name);
    OWN_PREFIXES.iter().any(|p| stem.starts_with(p)) && stem.ends_with(&format!("-{pid}"))
}

/// `sibling` が `path` の付属ファイル（同じ dir の `<path の名前>.…`。世代バックアップ `.bak.N`・
/// 排他の `.lock`・書きかけの一時ファイル）か（**純関数**）
pub fn is_sidecar(path: &Path, sibling: &Path) -> bool {
    let (Some(name), Some(other)) = (
        path.file_name().and_then(OsStr::to_str),
        sibling.file_name().and_then(OsStr::to_str),
    ) else {
        return false;
    };
    sibling.parent() == path.parent()
        && other.len() > name.len() + 1
        && other.starts_with(name)
        && other.as_bytes()[name.len()] == b'.'
}

/// 覚えた置き場のうち [`removable`] なものを付属ファイルごと消し、消した（もう無い）置き場を返す。
/// 何度呼んでも壊れない（無い置き場は黙って飛ばす）。消した置き場は [`is_retired`] が真になる。
/// 検証プロセスでなければ何もしない
pub fn remove_adopted() -> Vec<PathBuf> {
    if !active() {
        return Vec::new();
    }
    let pid = std::process::id();
    let mut removed = Vec::new();
    for adopted in adopted() {
        let env_now: Option<OsString> = std::env::var_os(adopted.key);
        if !removable(&adopted, pid, env_now.as_deref()) {
            continue;
        }
        // 先に「もう作らない」と印を付ける（消している最中の書き手に作り直させない）
        retire(&adopted.path);
        remove_path(&adopted.path);
        for sidecar in sidecars(&adopted.path) {
            remove_path(&sidecar);
        }
        if !adopted.path.exists() {
            removed.push(adopted.path);
        }
    }
    removed
}

/// 覚えた置き場のうち、片付けた後もまだ在るもの（付属ファイルを含む。失敗の出口の申告 `left=` 用）
pub fn left_adopted() -> Vec<PathBuf> {
    if !active() {
        return Vec::new();
    }
    let pid = std::process::id();
    let mut left = Vec::new();
    for a in adopted() {
        if !removable(&a, pid, std::env::var_os(a.key).as_deref()) {
            continue;
        }
        if a.path.exists() {
            left.push(a.path.clone());
        }
        left.extend(sidecars(&a.path));
    }
    left
}

fn sidecars(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_sidecar(path, p))
        .collect()
}

fn remove_path(path: &Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

fn retire(path: &Path) {
    let mut retired = lock(&RETIRED);
    if !retired.iter().any(|p| p == path) {
        retired.push(path.to_path_buf());
    }
}

/// `path` が片付けた置き場（かその下・その付属ファイル）か。書き手は真なら書こうとした行を捨て、
/// 置き場を作り直さない。検証プロセスでなければ常に偽
pub fn is_retired(path: &Path) -> bool {
    if !active() {
        return false;
    }
    lock(&RETIRED)
        .iter()
        .any(|dir| path.starts_with(dir) || is_sidecar(dir, path))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopted(key: &'static str, path: &str) -> Adopted {
        Adopted {
            key,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn 自分の_pid_の名前だけを自分の置き場と読む() {
        assert!(owned_name(Path::new("/t/tako-iso-data-42"), 42));
        assert!(owned_name(Path::new("/t/tako-iso-pane-logs-42"), 42));
        assert!(owned_name(Path::new("/t/tako-iso-sessions-42.yaml"), 42));
        assert!(owned_name(Path::new("/t/tako-st-orchestrator-42"), 42));
        assert!(owned_name(Path::new("/t/tako-st-workers-42.yaml"), 42));
        // 別の pid・pid の頭だけ一致・頭の違う名前・利用者の置き場・項目の固定名の一時 dir（#2704）
        assert!(!owned_name(Path::new("/t/tako-iso-data-43"), 42));
        assert!(!owned_name(Path::new("/t/tako-iso-data-142"), 42));
        assert!(!owned_name(Path::new("/t/tako-iso-data-421"), 42));
        assert!(!owned_name(Path::new("/t/tako-test-data-42"), 42));
        assert!(!owned_name(Path::new("/t/tako-osc-e2e"), 42));
        assert!(!owned_name(Path::new("/t/my-data"), 42));
        assert!(!owned_name(Path::new("/"), 42));
    }

    #[test]
    fn env_が当てた値のままのときだけ消してよい() {
        let a = adopted("TAKO_DATA_DIR", "/t/tako-iso-data-42");
        assert!(removable(&a, 42, Some(OsStr::new("/t/tako-iso-data-42"))));
        // 途中で利用者の値へ差し替わった・外された・別の pid
        assert!(!removable(&a, 42, Some(OsStr::new("/home/u/data"))));
        assert!(!removable(&a, 42, None));
        assert!(!removable(&a, 43, Some(OsStr::new("/t/tako-iso-data-42"))));
        // 名前が自分の pid でないものは env が一致しても消さない
        let explicit = adopted("TAKO_DATA_DIR", "/home/u/data");
        assert!(!removable(&explicit, 42, Some(OsStr::new("/home/u/data"))));
    }

    #[test]
    fn 付属ファイルは同じ_dir_の名前_ドット_だけ() {
        let file = Path::new("/t/tako-iso-sessions-42.yaml");
        assert!(is_sidecar(
            file,
            Path::new("/t/tako-iso-sessions-42.yaml.bak.1")
        ));
        assert!(is_sidecar(
            file,
            Path::new("/t/tako-iso-sessions-42.yaml.lock")
        ));
        assert!(is_sidecar(
            file,
            Path::new("/t/tako-iso-sessions-42.yaml.tmp.42.0")
        ));
        // 名前の続きが `.` でない・同じ名前・別の dir・名前の頭だけ一致
        assert!(!is_sidecar(
            file,
            Path::new("/t/tako-iso-sessions-42.yamlx")
        ));
        assert!(!is_sidecar(file, file));
        assert!(!is_sidecar(
            file,
            Path::new("/u/tako-iso-sessions-42.yaml.lock")
        ));
        assert!(!is_sidecar(
            file,
            Path::new("/t/tako-iso-sessions-421.yaml.lock")
        ));
    }

    /// 明示した置き場は当ても覚えもしないので、片付けても中身が残る。暗黙の置き場は付属ファイルごと
    /// 消え、消した後は作り直さない印が付く（env を書き換えるので子の中で）
    #[test]
    fn 明示した置き場は残し暗黙の置き場だけを消す() {
        if crate::run_alone!(明示した置き場は残し暗黙の置き場だけを消す) {
            return;
        }
        let root = crate::test_residue::ScratchDir::new("scoped2686");
        let tmp = root.path();
        let pid = std::process::id();
        let explicit = tmp.join("explicit-data");
        std::fs::create_dir_all(&explicit).unwrap();
        std::fs::write(explicit.join("persist.log"), "keep").unwrap();
        std::env::set_var("TAKO_DATA_DIR", &explicit);
        std::env::remove_var("TAKO_PANE_LOG_DIR");
        std::env::remove_var("TAKO_SESSIONS_FILE");
        std::env::remove_var("TAKO_2686_LEGACY");

        assert!(!adopt(
            "TAKO_DATA_DIR",
            tmp.join(format!("tako-iso-data-{pid}"))
        ));
        assert_eq!(
            std::env::var_os("TAKO_DATA_DIR"),
            Some(explicit.clone().into())
        );
        let logs = tmp.join(format!("tako-iso-pane-logs-{pid}"));
        assert!(adopt("TAKO_PANE_LOG_DIR", logs.clone()));
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("pane1.log"), "x").unwrap();
        let sessions = tmp.join(format!("tako-iso-sessions-{pid}.yaml"));
        assert!(adopt("TAKO_SESSIONS_FILE", sessions.clone()));
        for name in ["", ".bak.1", ".lock"] {
            std::fs::write(format!("{}{name}", sessions.display()), "x").unwrap();
        }
        assert!(is_adopted(&logs.join("pane1.log")));
        assert!(!is_adopted(&explicit.join("persist.log")));
        assert_eq!(left_adopted().len(), 4, "{:?}", left_adopted());

        assert_eq!(remove_adopted(), vec![logs.clone(), sessions.clone()]);
        assert!(!logs.exists(), "暗黙の置き場は消える");
        let remaining: Vec<String> = std::fs::read_dir(tmp)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            remaining,
            vec!["explicit-data".to_string()],
            "付属ファイルも消える"
        );
        assert!(
            explicit.join("persist.log").exists(),
            "明示の置き場の中身は残る"
        );
        assert!(is_retired(&logs.join("pane1.log")));
        assert!(is_retired(Path::new(&format!(
            "{}.lock",
            sessions.display()
        ))));
        assert!(!is_retired(&explicit.join("persist.log")));
        assert!(left_adopted().is_empty());
        // 2 度目は何もしない（無い置き場は黙って飛ばす）
        assert_eq!(remove_adopted(), vec![logs, sessions]);
    }

    /// A/B: 修正前は当てるだけで覚えない = 何も消さない・退役も無い
    #[test]
    fn legacy_は覚えないので消さない() {
        if crate::run_alone!(legacy_は覚えないので消さない) {
            return;
        }
        let root = crate::test_residue::ScratchDir::new("scoped2686l");
        let pid = std::process::id();
        let logs = root.path().join(format!("tako-iso-pane-logs-{pid}"));
        std::env::remove_var("TAKO_PANE_LOG_DIR");
        std::env::set_var("TAKO_2686_LEGACY", "1");
        assert!(adopt("TAKO_PANE_LOG_DIR", logs.clone()));
        assert_eq!(
            std::env::var_os("TAKO_PANE_LOG_DIR"),
            Some(logs.clone().into())
        );
        std::fs::create_dir_all(&logs).unwrap();
        assert!(remove_adopted().is_empty());
        assert!(logs.exists());
        assert!(!is_retired(&logs));
    }
}

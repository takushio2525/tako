//! discovery — CLI / MCP 接続情報の永続化（FR-2.2.9）
//!
//! トークンとソケットパスはアプリ起動毎に変わる（トークンはメモリ生成、ソケットは
//! PID 入りパス）ため、環境変数しか手段が無いとアプリ再起動後に外部の長寿命プロセス
//! （tmux 内のエージェント等）が繋ぎ直せない。アプリは起動時に接続情報を
//! **ユーザー専用パーミッション（ファイル 0600 / ディレクトリ 0700）**で書き出し、
//! CLI は環境変数が無い・古い（接続不可 / 認証失敗）場合にこのファイルへ
//! フォールバックする。
//!
//! ## 複数インスタンス（2026-06-12 バグ (8) の恒久対策）
//!
//! 「最新優先の単純上書き」だと、セルフテストの一時インスタンスや二重起動が
//! `control.json` を上書きして exit した時点で、メインインスタンスへの接続が壊れる
//! （AI フルコントロールの全断）。そのため:
//!
//! - 各インスタンスは `instances/control-<pid>.json` に自分の接続情報を書き、
//!   `control.json`（current ポインタ = 最新起動）も従来どおり更新する
//! - CLI は current が死んでいたら instances/ を新しい順に走査し、
//!   **生きているインスタンス**（接続試行で判定）へ自動フォールバックする
//! - 書き出し時に死んだインスタンスのファイルを掃除し、アプリの明示終了時は
//!   自分のファイルを削除する（クラッシュ残骸は次回の掃除と接続プローブが吸収）
//! - セルフテストは `TAKO_DISCOVERY_DIR` で専用の一時ディレクトリへ隔離し、
//!   メインの control.json には一切触らない

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 接続情報ファイルの中身。トークンを含むためログに出さないこと（`conventions.md`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    /// フォーマット版数（後方互換の判断用）
    pub version: u32,
    /// 書き出したアプリの PID（インスタンスファイル名と掃除に使う）
    pub pid: u32,
    /// IPC ソケットパス（`TAKO_SOCKET` 相当）
    pub socket: String,
    /// 認証トークン（`TAKO_TOKEN` 相当）
    pub token: String,
    /// MCP エンドポイント（`TAKO_MCP_URL` 相当。MCP サーバーが立たなければ None）
    pub mcp_url: Option<String>,
}

/// 接続情報の置き場。`TAKO_DISCOVERY_DIR` で差し替え可能（セルフテストの隔離用）
fn base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TAKO_DISCOVERY_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    tako_core::paths::data_dir()
}

/// current ポインタ（`<base>/control.json` = 最新起動インスタンス）
pub fn info_path() -> Option<PathBuf> {
    base_dir().map(|d| d.join("control.json"))
}

/// インスタンスごとの接続情報ディレクトリ（`<base>/instances/`）
fn instances_dir() -> Option<PathBuf> {
    base_dir().map(|d| d.join("instances"))
}

fn instance_path(pid: u32) -> Option<PathBuf> {
    instances_dir().map(|d| d.join(format!("control-{pid}.json")))
}

/// 接続情報を書き出す（アプリ起動時に呼ぶ）。
/// 自分のインスタンスファイル + current ポインタの両方を更新し、
/// 死んだインスタンスの残骸ファイルを掃除する
pub fn write(info: &ControlInfo) -> io::Result<PathBuf> {
    let path = info_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "データディレクトリを解決できない",
        )
    })?;
    if let Some(instance) = instance_path(info.pid) {
        write_to(&instance, info)?;
    }
    write_to(&path, info)?;
    prune_dead_instances(info.pid);
    Ok(path)
}

/// 明示終了時のクリーンアップ（アプリの Quit 経路から呼ぶ）。
/// 自分のインスタンスファイルを消し、current が自分を指していたらそれも消す
/// （死んだ接続先を新規 CLI に掴ませない）。クラッシュ時は呼ばれないが、
/// 残骸は次回起動の掃除と CLI 側の接続プローブで無害化される
pub fn cleanup(pid: u32) {
    if let Some(instance) = instance_path(pid) {
        let _ = std::fs::remove_file(instance);
    }
    if let Some(path) = info_path() {
        if read_from(&path).is_some_and(|info| info.pid == pid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 死んだインスタンス（ソケットへ接続できない）の残骸ファイルを消す。
/// `keep_pid`（自分）は接続確認せず残す
fn prune_dead_instances(keep_pid: u32) {
    let Some(dir) = instances_dir() else {
        return;
    };
    for info in list_instances(&dir) {
        if info.pid == keep_pid {
            continue;
        }
        if !socket_alive(&info.socket) {
            if let Some(path) = instance_path(info.pid) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// ソケットが接続を受け付けるか（生存プローブ。認証はしない）
#[cfg(unix)]
pub fn socket_alive(socket: &str) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
pub fn socket_alive(_socket: &str) -> bool {
    // Windows named pipe は Phase 6。それまで存在確認しない（候補に残して接続試行に任せる）
    true
}

fn write_to(path: &Path, info: &ControlInfo) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリが無い"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(info)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // トークンを含むためユーザー専用にする（ソケットの 0600 と同じ防御線）
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

/// current（最新起動インスタンス）の接続情報を読む。
/// ファイルが無い・読めない・版数不明なら None（呼び出し側は環境変数のみで判断する）
pub fn read() -> Option<ControlInfo> {
    read_from(&info_path()?)
}

/// フォールバック候補列: current → 各インスタンス（更新の新しい順）。
/// 重複（同一ソケット）は除く。CLI は先頭から順に接続を試す（バグ (8) の恒久対策:
/// current が一時インスタンスの残骸でも、生きているメインへ自動で届く）
pub fn read_candidates() -> Vec<ControlInfo> {
    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(info) = read() {
        seen.insert(info.socket.clone());
        candidates.push(info);
    }
    if let Some(dir) = instances_dir() {
        let mut instances: Vec<(std::time::SystemTime, ControlInfo)> = list_instances(&dir)
            .into_iter()
            .filter_map(|info| {
                let mtime = instance_path(info.pid)
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok())?;
                Some((mtime, info))
            })
            .collect();
        instances.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        for (_, info) in instances {
            if seen.insert(info.socket.clone()) {
                candidates.push(info);
            }
        }
    }
    candidates
}

fn list_instances(dir: &Path) -> Vec<ControlInfo> {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    reader
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("control-")
                .is_some_and(|rest| rest.ends_with(".json"))
        })
        .filter_map(|e| read_from(&e.path()))
        .collect()
}

fn read_from(path: &Path) -> Option<ControlInfo> {
    let json = std::fs::read_to_string(path).ok()?;
    let info: ControlInfo = serde_json::from_str(&json).ok()?;
    (info.version == 1).then_some(info)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-discovery-test-{}-{name}", std::process::id()))
    }

    fn info(pid: u32, socket: &str) -> ControlInfo {
        ControlInfo {
            version: 1,
            pid,
            socket: socket.into(),
            token: format!("token-{pid}"),
            mcp_url: None,
        }
    }

    #[test]
    fn 書き出しと読み戻しが往復しパーミッションは0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control.json");
        let payload = info(42, "/tmp/tako-42-0.sock");
        write_to(&path, &payload).unwrap();
        assert_eq!(read_from(&path), Some(payload.clone()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "ファイルはユーザー専用");
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700, "ディレクトリはユーザー専用");
        // 上書き = 最新優先
        let newer = info(43, "/tmp/tako-42-0.sock");
        write_to(&path, &newer).unwrap();
        assert_eq!(read_from(&path), Some(newer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 不在や未知の版数はnone() {
        assert_eq!(read_from(&temp_path("missing").join("control.json")), None);
        let dir = temp_path("badver");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control.json");
        let payload = ControlInfo {
            version: 99,
            ..info(1, "s")
        };
        write_to(&path, &payload).unwrap();
        assert_eq!(read_from(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// バグ (8) の回帰: 一時インスタンスが current を上書きして exit しても、
    /// 候補列に生きているインスタンスが残り、死んだ候補は掃除される
    #[test]
    fn 候補列はcurrentと生存インスタンスを返し死骸は掃除される() {
        // このテストは env（TAKO_DISCOVERY_DIR）でモジュール全体の置き場を差し替える。
        // 他テストと並列でも安全なよう、env を使う検証はこの 1 本に集約してある
        let dir = temp_path("candidates");
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("TAKO_DISCOVERY_DIR", &dir);

        // 生きているソケット（実 UnixListener）と死んだソケットパス
        let alive_socket = dir.join("alive.sock");
        std::fs::create_dir_all(&dir).unwrap();
        let _listener =
            std::os::unix::net::UnixListener::bind(&alive_socket).expect("テスト用ソケット");
        let alive = info(100, alive_socket.to_str().unwrap());
        let dead = info(200, dir.join("dead.sock").to_str().unwrap());

        // メイン（生存）→ 一時インスタンス（死亡。current を上書きして exit した状況）
        write(&alive).unwrap();
        write(&dead).unwrap();
        // current は死んだ一時インスタンスを指しているが、候補列には生存側が続く
        assert_eq!(read().map(|i| i.pid), Some(200));
        let candidates = read_candidates();
        assert_eq!(candidates[0].pid, 200, "先頭は current");
        assert!(
            candidates.iter().any(|c| c.pid == 100),
            "生きているメインが候補に残る: {candidates:?}"
        );
        // 死んだインスタンスのファイルは次の write で掃除される
        write(&alive).unwrap();
        let candidates = read_candidates();
        assert!(
            candidates.iter().all(|c| c.pid != 200),
            "死骸が掃除される: {candidates:?}"
        );
        // 明示終了のクリーンアップ: 自分のファイルと current が消える
        cleanup(100);
        assert_eq!(read(), None);
        assert!(read_candidates().is_empty());

        std::env::remove_var("TAKO_DISCOVERY_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

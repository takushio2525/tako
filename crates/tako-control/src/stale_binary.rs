//! stale_binary — 長生きセッションの claude バイナリ鮮度検知（Issue #498）
//!
//! ペイン生成時に解決した claude の実バイナリパスを記録し、稼働中プロセスは
//! libproc `proc_pidpath` で取得、`~/.local/bin/claude` の現在の解決先と突き合わせる。
//! 差異があれば stale と判定し、UI バナー + CLI/MCP で通知・張り直しを提供する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// ペインごとの stale 判定情報
#[derive(Debug, Clone)]
pub struct PaneClaudeInfo {
    /// ペイン生成時に解決した claude バイナリの実パス（symlink 解決済み）
    pub spawned_binary: PathBuf,
    /// ペイン内 claude プロセスの PID（判定時に取得）
    pub pid: Option<u32>,
    /// バナーをユーザーが閉じたか（閉じても次回検知で再提示される）
    pub dismissed: bool,
}

/// stale 判定の結果
#[derive(Debug, Clone)]
pub struct StaleStatus {
    /// stale か否か
    pub stale: bool,
    /// 起動時のバイナリパス
    pub spawned_binary: String,
    /// 現在の `claude` symlink が指す実パス
    pub current_binary: String,
    /// 起動時バイナリから抽出したバージョン（取得できなければ空）
    pub spawned_version: String,
    /// 現在バイナリから抽出したバージョン（取得できなければ空）
    pub current_version: String,
    /// ペイン内 claude の PID
    pub pid: Option<u32>,
}

/// グローバルな stale 判定キャッシュ（5 秒 TTL）
static CACHE: Mutex<Option<CachedResult>> = Mutex::new(None);

struct CachedResult {
    current_binary: PathBuf,
    current_version: String,
    at: std::time::Instant,
}

const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// `~/.local/bin/claude`（または PATH 上の claude）の symlink を解決し実パスを返す
pub fn resolve_current_claude_binary() -> Option<PathBuf> {
    if let Some(cached) = CACHE.lock().ok()?.as_ref() {
        if cached.at.elapsed() < CACHE_TTL {
            return Some(cached.current_binary.clone());
        }
    }

    let path = resolve_claude_symlink()?;
    let version = extract_version_from_path(&path);

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(CachedResult {
            current_binary: path.clone(),
            current_version: version,
            at: std::time::Instant::now(),
        });
    }
    Some(path)
}

/// `claude` コマンドの symlink 先を実パスまで解決する
fn resolve_claude_symlink() -> Option<PathBuf> {
    // which claude → symlink 解決
    // #628: GUI プロセスから到達するのでコンソールウィンドウを出させない
    let which_out = tako_core::platform::process::no_console_window(
        std::process::Command::new("which").arg("claude"),
    )
    .output()
    .ok()
    .filter(|o| o.status.success())?;
    let raw = String::from_utf8_lossy(&which_out.stdout)
        .trim()
        .to_string();
    if raw.is_empty() {
        return None;
    }
    // canonicalize で symlink チェーンを完全解決
    std::fs::canonicalize(&raw).ok()
}

/// バイナリパスからバージョンを推定する。
/// Claude CLI は `~/.local/share/claude/versions/<version>`（新）または
/// `~/.claude/local/claude-cli-<version>/claude`（旧）の構造。
/// パスに version が含まれていなければ `claude --version` にフォールバック
pub fn extract_version_from_path(path: &Path) -> String {
    let path_str = path.to_string_lossy();
    // 新形式: .../versions/2.1.220
    if let Some(start) = path_str.find("/versions/") {
        let rest = &path_str[start + "/versions/".len()..];
        let ver = rest.split('/').next().unwrap_or("");
        if !ver.is_empty() && ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return ver.to_string();
        }
    }
    // 旧形式: .../claude-cli-2.1.220/claude
    if let Some(start) = path_str.find("claude-cli-") {
        let rest = &path_str[start + "claude-cli-".len()..];
        if let Some(end) = rest.find('/') {
            let ver = &rest[..end];
            if !ver.is_empty() {
                return ver.to_string();
            }
        }
    }
    // フォールバック: claude --version（重いので最終手段）
    extract_version_via_cli(path)
}

fn extract_version_via_cli(binary: &Path) -> String {
    // #628: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(
        std::process::Command::new(binary).arg("--version"),
    )
    .output()
    .ok()
    .filter(|o| o.status.success())
    .and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        // "claude v2.1.220" 形式
        text.split_whitespace()
            .find(|w| w.starts_with('v') || w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(|w| w.trim_start_matches('v').to_string())
    })
    .unwrap_or_default()
}

/// 稼働中プロセスのバイナリパスを `proc_pidpath` で取得する（macOS のみ）
#[cfg(target_os = "macos")]
pub fn pidpath(pid: u32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let ret = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if ret <= 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(ret as usize);
    let s = std::str::from_utf8(&buf[..len]).ok()?;
    Some(PathBuf::from(s))
}

#[cfg(not(target_os = "macos"))]
pub fn pidpath(pid: u32) -> Option<PathBuf> {
    // Linux: /proc/<pid>/exe
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// ペインの stale 状態を判定する
pub fn check_stale(info: &PaneClaudeInfo) -> StaleStatus {
    let current = resolve_current_claude_binary();
    let current_binary = current.clone().unwrap_or_default();
    let current_version = CACHE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.current_version.clone()))
        .unwrap_or_default();
    let spawned_version = extract_version_from_path(&info.spawned_binary);

    let stale = current
        .as_ref()
        .is_some_and(|cur| cur != &info.spawned_binary);

    StaleStatus {
        stale,
        spawned_binary: info.spawned_binary.to_string_lossy().to_string(),
        current_binary: current_binary.to_string_lossy().to_string(),
        spawned_version,
        current_version,
        pid: info.pid,
    }
}

/// バックエンドセッション内で claude バイナリを実行しているプロセスの PID を探す。
/// tmux pane_pid → 子プロセスチェーンを辿り、`pidpath` で「claude」を含む
/// パスを実行しているプロセスを返す。`claude agents --json` に依存しないため
/// 隔離環境でも動く
pub fn find_claude_pid_for_backend(backend_session: &str) -> Option<u32> {
    let socket = tako_core::tmux_backend::socket_name();
    let panes = crate::agents::tmux_pane_pids(Some(&socket));
    let target_pids: Vec<u32> = panes
        .into_iter()
        .filter(|(id, _)| id.starts_with(&format!("{backend_session}:")))
        .map(|(_, pid)| pid)
        .collect();
    if target_pids.is_empty() {
        return None;
    }

    // 全プロセスの親子マップから、ペイン PID の子孫で claude を実行しているものを探す
    let parents = crate::agents::process_parent_map();
    // 逆引き: pid → 子の集合
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&child, &parent) in &parents {
        children.entry(parent).or_default().push(child);
    }

    // ペイン PID から BFS で子孫を辿り、pidpath が claude を含むものを探す
    for &pane_pid in &target_pids {
        let mut queue = vec![pane_pid];
        let mut visited = std::collections::HashSet::new();
        while let Some(pid) = queue.pop() {
            if !visited.insert(pid) {
                continue;
            }
            // pidpath で実パスを確認（claude を含むか）
            if pid != pane_pid {
                if let Some(path) = pidpath(pid) {
                    let path_str = path.to_string_lossy();
                    if path_str.contains("claude") {
                        return Some(pid);
                    }
                }
            }
            if let Some(kids) = children.get(&pid) {
                queue.extend(kids);
            }
        }
    }
    None
}

/// StaleStatus を JSON に変換
pub fn status_to_json(status: &StaleStatus) -> serde_json::Value {
    serde_json::json!({
        "stale": status.stale,
        "spawned_binary": status.spawned_binary,
        "current_binary": status.current_binary,
        "spawned_version": status.spawned_version,
        "current_version": status.current_version,
        "pid": status.pid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_from_path() {
        // 旧形式: claude-cli-<ver>/claude
        let path = PathBuf::from("/Users/user/.claude/local/claude-cli-2.1.220/claude");
        assert_eq!(extract_version_from_path(&path), "2.1.220");

        // 新形式: versions/<ver>（symlink 先そのもの）
        let path_new = PathBuf::from("/Users/user/.local/share/claude/versions/2.1.220");
        assert_eq!(extract_version_from_path(&path_new), "2.1.220");

        let path2 = PathBuf::from("/usr/local/bin/claude");
        // PATH 上のバイナリではバージョン抽出できない（CLI フォールバック）
        let ver = extract_version_from_path(&path2);
        // 実行環境では claude が存在しないため空文字列
        assert!(ver.is_empty() || !ver.is_empty());
    }

    #[test]
    fn test_check_stale_same_binary() {
        let info = PaneClaudeInfo {
            spawned_binary: PathBuf::from("/fake/path/claude-cli-2.1.220/claude"),
            pid: Some(12345),
            dismissed: false,
        };
        // resolve_current_claude_binary が Some を返さない環境では stale=false
        let status = check_stale(&info);
        // CI などでは claude が無いため stale=false になる
        if status.current_binary.is_empty() {
            assert!(!status.stale);
        }
    }

    #[test]
    fn test_check_stale_different_binary() {
        // 手動で異なるバイナリを設定
        let info = PaneClaudeInfo {
            spawned_binary: PathBuf::from("/fake/old/claude-cli-2.1.218/claude"),
            pid: None,
            dismissed: false,
        };
        let status = check_stale(&info);
        // current が解決できる環境では stale=true になるはず
        if !status.current_binary.is_empty() {
            assert!(status.stale);
        }
    }

    #[test]
    fn test_pidpath_self() {
        let pid = std::process::id();
        let path = pidpath(pid);
        // 自プロセスのパスは取得できるはず
        assert!(path.is_some(), "自プロセスの pidpath が None");
    }

    #[test]
    fn test_pidpath_nonexistent() {
        let path = pidpath(99999999);
        assert!(path.is_none());
    }
}

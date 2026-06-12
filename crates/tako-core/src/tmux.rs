//! tmux — tmux セッションの一覧・kill のデータ取得層（FR-2.13）
//!
//! 消し忘れて裏で動き続ける tmux に気づいて殺せるようにするための土台。
//! **表示には関知しない**（FR-2.13.5: 取得層と表示の分離）。UI（tmuxview タブ）も
//! CLI / MCP も dispatch 経由でこの層を呼ぶ。
//!
//! tmux CLI（`list-sessions` / `list-windows` / `list-clients` / `kill-*`）を呼び出し、
//! タブ区切りのフォーマット出力をパースする。パースは純関数（ユニットテスト対象）、
//! コマンド実行は薄いラッパに分離してある。tmux 不在・サーバー未起動は空一覧で無害に劣化する。

use std::process::Command;
use std::sync::OnceLock;

/// tmux バイナリの場所（プロセス内で 1 回だけ解決してキャッシュする）。
/// Dock 起動の .app は PATH が最小構成（/usr/bin:/bin:…）で Homebrew の tmux が
/// 見えない（2026-06-12 実機リグレッション: tmuxview が空 + バックエンド永続化が
/// 沈黙劣化）。解決順: `TAKO_TMUX_BIN` → PATH → 既知の場所 → ログインシェルの
/// `command -v`（autorename の claude 解決と同じ手口）。不在なら "tmux" を返し、
/// 呼び出し側は実行失敗として無害に劣化する
pub fn tmux_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(resolve_tmux_bin)
}

fn resolve_tmux_bin() -> String {
    if let Some(bin) = std::env::var_os("TAKO_TMUX_BIN") {
        if !bin.is_empty() {
            return bin.to_string_lossy().into_owned();
        }
    }
    // PATH 直（ターミナルからの起動・開発時はこれで足りる）
    if Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "tmux".into();
    }
    // 既知の場所（Homebrew arm64 / Intel / MacPorts）
    for candidate in [
        "/opt/homebrew/bin/tmux",
        "/usr/local/bin/tmux",
        "/opt/local/bin/tmux",
    ] {
        if std::path::Path::new(candidate).is_file() {
            return candidate.into();
        }
    }
    // ログインシェル経由でユーザーの PATH を引く（unix のみ）
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = Command::new(shell)
            .args(["-l", "-c", "command -v tmux"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && std::path::Path::new(&path).is_file() {
                    return path;
                }
            }
        }
    }
    "tmux".into()
}

/// tmux の 1 window
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow {
    pub index: u32,
    pub name: String,
    pub active: bool,
    /// window 内のペイン数
    pub panes: u32,
}

/// tmux の 1 セッション
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TmuxSession {
    pub name: String,
    /// 作成日時（unix epoch 秒）
    pub created: i64,
    pub attached: bool,
    pub windows: Vec<TmuxWindow>,
    /// attach 中クライアントの tty（`/dev/ttysNNN`）。tako ペインとの対応付けに使う
    pub client_ttys: Vec<String>,
}

/// 対象 tmux サーバー。`None` は既定サーバー、`Some(name)` は `tmux -L <name>`
/// （セルフテストの隔離や複数サーバー運用に使う）
pub fn list_sessions(socket: Option<&str>) -> Vec<TmuxSession> {
    let sessions = run_tmux(
        socket,
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_created}\t#{session_attached}",
        ],
    )
    .unwrap_or_default();
    let windows = run_tmux(
        socket,
        &[
            "list-windows",
            "-a",
            "-F",
            "#{session_name}\t#{window_index}\t#{window_name}\t#{window_active}\t#{window_panes}",
        ],
    )
    .unwrap_or_default();
    let clients = run_tmux(
        socket,
        &["list-clients", "-F", "#{session_name}\t#{client_tty}"],
    )
    .unwrap_or_default();
    parse_sessions(&sessions, &windows, &clients)
}

/// セッションを kill する。誤爆防止の確認は呼び出し側（UI / AI）の責務
pub fn kill_session(socket: Option<&str>, name: &str) -> Result<(), String> {
    run_tmux(socket, &["kill-session", "-t", &format!("={name}")]).map(|_| ())
}

/// window を kill する（`session:index` 指定）
pub fn kill_window(socket: Option<&str>, session: &str, window: u32) -> Result<(), String> {
    run_tmux(
        socket,
        &["kill-window", "-t", &format!("={session}:{window}")],
    )
    .map(|_| ())
}

/// tmux CLI 実行。サーバー未起動（list 系の "no server running"）はエラー文字列を返す
/// （list 側で空扱いにする）。tmux バイナリ不在も同様
fn run_tmux(socket: Option<&str>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(tmux_bin());
    if let Some(name) = socket {
        command.args(["-L", name]);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|e| format!("tmux を実行できない: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// `list-sessions` / `list-windows -a` / `list-clients` のフォーマット出力を統合する
fn parse_sessions(sessions: &str, windows: &str, clients: &str) -> Vec<TmuxSession> {
    let mut result: Vec<TmuxSession> = sessions
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            Some(TmuxSession {
                name: f.next()?.to_string(),
                created: f.next()?.parse().ok()?,
                attached: f.next()? != "0",
                windows: Vec::new(),
                client_ttys: Vec::new(),
            })
        })
        .collect();
    for line in windows.lines() {
        let mut f = line.split('\t');
        let (Some(session), Some(index), Some(name), Some(active), Some(panes)) =
            (f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let (Ok(index), Ok(panes)) = (index.parse(), panes.parse()) else {
            continue;
        };
        if let Some(s) = result.iter_mut().find(|s| s.name == session) {
            s.windows.push(TmuxWindow {
                index,
                name: name.to_string(),
                active: active != "0",
                panes,
            });
        }
    }
    for line in clients.lines() {
        let mut f = line.split('\t');
        let (Some(session), Some(tty)) = (f.next(), f.next()) else {
            continue;
        };
        if let Some(s) = result.iter_mut().find(|s| s.name == session) {
            s.client_ttys.push(tty.to_string());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn フォーマット出力を統合してパースする() {
        let sessions = "work\t1760000000\t1\nbg-agent\t1760000100\t0\n";
        let windows = "work\t0\tvim\t1\t2\nwork\t1\tserver\t0\t1\nbg-agent\t0\tzsh\t1\t1\n";
        let clients = "work\t/dev/ttys012\n";
        let parsed = parse_sessions(sessions, windows, clients);
        assert_eq!(parsed.len(), 2);
        let work = &parsed[0];
        assert_eq!(work.name, "work");
        assert_eq!(work.created, 1760000000);
        assert!(work.attached);
        assert_eq!(work.windows.len(), 2);
        assert_eq!(work.windows[0].name, "vim");
        assert!(work.windows[0].active);
        assert_eq!(work.windows[1].panes, 1);
        assert_eq!(work.client_ttys, vec!["/dev/ttys012"]);
        let bg = &parsed[1];
        assert!(!bg.attached);
        assert!(bg.client_ttys.is_empty());
    }

    #[test]
    fn 空入力と壊れた行は無害に読み飛ばす() {
        assert!(parse_sessions("", "", "").is_empty());
        let parsed = parse_sessions("only-name\nok\t1\t0\n", "broken\n", "broken\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "ok");
    }
}

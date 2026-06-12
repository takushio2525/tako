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

/// tmux クライアント子プロセスの雛形。バイナリ解決（`tmux_bin`）と
/// **UTF-8 ロケールの明示注入**を一手に引き受ける。
///
/// tmux 3.6 はクライアントのロケールが C（LC_CTYPE / LANG 未設定）だと、
/// コマンド出力中の制御文字を `_` に置換する（サニタイズ）。Dock 起動の .app は
/// ロケール環境変数ゼロのため、`-F "…\t…"` のタブ区切り出力が
/// `master-2_1781179563_0` になり**全パースが沈黙全滅**していた
/// （tmuxview 空表示・tako 駆動スクロール無反応の共通根本原因。2026-06-12 実機）。
/// ペイン側の CJK 対策（LC_CTYPE=UTF-8 既定注入）と同じ Terminal.app 方式で、
/// クライアント側にも UTF-8 を明示する。LC_ALL は LC_CTYPE より優先されるため、
/// 親から C が継承されても効くよう除去する
pub(crate) fn tmux_command(socket: Option<&str>) -> Command {
    let mut command = Command::new(tmux_bin());
    command.env_remove("LC_ALL").env("LC_CTYPE", "UTF-8");
    if let Some(name) = socket {
        command.args(["-L", name]);
    }
    command
}

/// tmux CLI 実行。サーバー未起動（list 系の "no server running"）はエラー文字列を返す
/// （list 側で空扱いにする）。tmux バイナリ不在も同様
pub(crate) fn run_tmux(socket: Option<&str>, args: &[&str]) -> Result<String, String> {
    let output = tmux_command(socket)
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

    /// tmux クライアント子プロセスに UTF-8 ロケールが必ず注入される
    /// （Dock 起動の .app はロケール環境変数ゼロ → tmux 3.6 がタブ区切り出力の
    /// TAB を `_` にサニタイズし全パースが沈黙全滅した 2026-06-12 実機バグの再発防止）
    #[test]
    fn tmux_commandはutf8ロケールを注入する() {
        let command = tmux_command(Some("sock"));
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(
            envs.contains(&("LC_CTYPE".into(), Some("UTF-8".into()))),
            "LC_CTYPE=UTF-8 が注入されていない: {envs:?}"
        );
        assert!(
            envs.contains(&("LC_ALL".into(), None)),
            "LC_ALL が除去されていない（C 継承で LC_CTYPE が無効化される）: {envs:?}"
        );
    }

    /// tmux 3.6 の実挙動の検証（カナリア）+ 修正の e2e:
    /// C ロケールのクライアントはタブ区切り `-F` 出力の TAB を `_` に置換するが、
    /// ロケール無し環境（Dock 起動の .app 相当）でも `run_tmux` 経由なら TAB が保持される
    #[test]
    #[cfg(unix)]
    fn ロケール無し環境でもタブ区切り出力が壊れない() {
        if !crate::tmux_backend::available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-loc-{}", std::process::id());
        let _ = run_tmux(Some(&socket), &["new-session", "-d", "-s", "loc-e2e"]);
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                crate::tmux_backend::kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());

        // カナリア（観測のみ）: C ロケールの素のクライアントでは TAB が `_` に
        // サニタイズされる、が元バグの前提。2026-06-12 夜、同一 tmux 3.6b バイナリ・
        // 同一マシンでサニタイズが再現しなくなった（数時間前の同テストは緑）ことが
        // 観測され、この挙動は環境要因で変動すると判明したため hard assert をやめた。
        // 修正本体の保証（下の 2 つの assert）は環境に依らず成立する
        let raw = Command::new(tmux_bin())
            .env_remove("LANG")
            .env_remove("LC_CTYPE")
            .env("LC_ALL", "C")
            .args([
                "-L",
                &socket,
                "list-sessions",
                "-F",
                "#{session_name}\t#{session_created}",
            ])
            .output()
            .expect("tmux を実行できる");
        let raw = String::from_utf8_lossy(&raw.stdout);
        eprintln!(
            "カナリア観測: C ロケールクライアントの TAB サニタイズ = {}（出力: {raw:?}）",
            raw.contains("loc-e2e_") && !raw.contains('\t')
        );

        // 修正の本体: ロケール無しの親環境を模しても run_tmux は TAB を保持する
        // （tmux_command が LC_CTYPE=UTF-8 を明示注入するため、親に依存しない）
        let fixed = tmux_command(Some(&socket))
            .env_remove("LANG")
            .args(["list-sessions", "-F", "#{session_name}\t#{session_created}"])
            .output()
            .expect("tmux を実行できる");
        let fixed = String::from_utf8_lossy(&fixed.stdout);
        assert!(
            fixed.contains("loc-e2e\t"),
            "ロケール注入後も TAB が保持されない: {fixed:?}"
        );

        // データ取得層まで通しで: list_sessions がセッションをパースできる
        let sessions = list_sessions(Some(&socket));
        assert!(
            sessions.iter().any(|s| s.name == "loc-e2e"),
            "list_sessions がパースできない: {sessions:?}"
        );
    }
}

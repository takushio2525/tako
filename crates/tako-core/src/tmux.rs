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
    crate::resolve_bin(
        "TAKO_TMUX_BIN",
        "tmux",
        "-V",
        &[
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/opt/local/bin/tmux",
        ],
    )
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
    /// アクティブペインのプロセス PID（`#{pane_pid}`）
    pub pane_pid: Option<u32>,
    /// アクティブペインで走っているコマンド名（`#{pane_current_command}`）
    pub pane_command: Option<String>,
    /// アクティブペインの cwd（`#{pane_current_path}`）
    pub pane_current_path: Option<String>,
    /// 最終アクティビティ時刻（`#{session_activity}` unix epoch 秒）
    pub last_activity: i64,
}

/// 対象 tmux サーバー。`None` は既定サーバー、`Some(name)` は `tmux -L <name>`
/// （セルフテストの隔離や複数サーバー運用に使う）
pub fn list_sessions(socket: Option<&str>) -> Vec<TmuxSession> {
    let sessions = run_tmux(
        socket,
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_created}\t#{session_attached}\t#{pane_pid}\t#{pane_current_command}\t#{pane_current_path}\t#{session_activity}",
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

/// セッションの存在確認（`has-session`、1 コマンド）。
/// `list_sessions`（3 コマンド）よりはるかに軽量
pub fn has_session(socket: Option<&str>, name: &str) -> bool {
    run_tmux(socket, &["has-session", "-t", &format!("={name}")]).is_ok()
}

/// 全クライアントの (client_pid, セッション名) 一覧（`list-clients`、1 コマンド）。
/// 復元強奪ガード（#177）が「生きた別 tako のクライアントが attach 中のセッション」を
/// 検出するのに使う。サーバー不在・クライアント無しは空
pub fn list_client_pids(socket: Option<&str>) -> Vec<(u32, String)> {
    let out = run_tmux(
        socket,
        &["list-clients", "-F", "#{client_pid}\t#{session_name}"],
    )
    .unwrap_or_default();
    parse_client_pids(&out)
}

fn parse_client_pids(out: &str) -> Vec<(u32, String)> {
    out.lines()
        .filter_map(|line| {
            let (pid, session) = line.split_once('\t')?;
            Some((pid.trim().parse().ok()?, session.trim().to_string()))
        })
        .collect()
}

/// grouped session の所属グループ名を返す。tmux はグループ名を「最初に作られた
/// 元セッション名」にするため、これが事実上の「元セッション」になる
/// （例: `tako-view-master-tako-2` → `master-tako`）。単独セッション（グループ無し）や
/// 不在の場合は `None`。tako-view-* ラッパーの再ラップ（無限ネスト）解消に使う
pub fn session_group(socket: Option<&str>, name: &str) -> Option<String> {
    let out = run_tmux(
        socket,
        &[
            "display-message",
            "-p",
            "-t",
            &format!("={name}"),
            "#{session_group}",
        ],
    )
    .ok()?;
    let group = out.trim();
    if group.is_empty() {
        None
    } else {
        Some(group.to_string())
    }
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

/// window を指定サイズへリサイズする（`resize-window -x -y`）。
/// tmux は window-size を manual に切り替えるため、attach クライアントのリサイズが
/// 効かなくなる。元へ戻すには `reset_window_size` を呼ぶこと
pub fn resize_window(
    socket: Option<&str>,
    session: &str,
    window: u32,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    run_tmux(
        socket,
        &[
            "resize-window",
            "-t",
            &format!("={session}:{window}"),
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ],
    )
    .map(|_| ())
}

/// `resize_window` による manual サイズを解除し、window-size をサーバー既定へ戻す
pub fn reset_window_size(socket: Option<&str>, session: &str, window: u32) -> Result<(), String> {
    run_tmux(
        socket,
        &[
            "set-window-option",
            "-t",
            &format!("={session}:{window}"),
            "-u",
            "window-size",
        ],
    )
    .map(|_| ())
}

/// アクティブ window を切り替える（`session:index` 指定）
pub fn select_window(socket: Option<&str>, session: &str, index: u32) -> Result<(), String> {
    run_tmux(
        socket,
        &["select-window", "-t", &format!("={session}:{index}")],
    )
    .map(|_| ())
}

/// 特定 window のペイン内容をテキストとして取得する（ホバープレビュー用）。
/// `-p` で標準出力へ、`-e` なし = ANSI 除去のプレーンテキスト
pub fn capture_pane_text(socket: Option<&str>, session: &str, window: u32) -> Vec<String> {
    run_tmux(
        socket,
        &["capture-pane", "-t", &format!("={session}:{window}"), "-p"],
    )
    .map(|output| output.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// target-pane 系コマンド（capture-pane / send-keys / paste-buffer）向けの
/// セッション exact-match ターゲット。tmux 3.6 は裸の `=session` を target-pane として
/// 解決できず "can't find pane" になるため、末尾コロンでセッション部を明示する
/// （`=session:` = そのセッションのアクティブ window / pane。Issue #32 の E2E で発覚）
fn session_pane_target(session: &str) -> String {
    format!("={session}:")
}

/// セッションのアクティブ window からペイン内容を取得する。
/// GUI の表示状態に依存せず、tmux session が生きていれば常に読める
pub fn capture_session(socket: Option<&str>, session: &str) -> Result<Vec<String>, String> {
    run_tmux(
        socket,
        &["capture-pane", "-t", &session_pane_target(session), "-p"],
    )
    .map(|output| output.lines().map(str::to_string).collect())
}

/// セッションのアクティブ window へキー入力を送信する。
/// `text` はそのまま send-keys へ渡す（リテラルモード `-l`）。
/// 注意: 改行を含むテキストの一括送信は TUI 側で貼り付けと誤認され崩れる
/// （Issue #32）。マルチラインは `paste_text` + `send_key(…, "Enter")` を使う
pub fn send_keys(socket: Option<&str>, session: &str, text: &str) -> Result<(), String> {
    run_tmux(
        socket,
        &["send-keys", "-t", &session_pane_target(session), "-l", text],
    )
    .map(|_| ())
}

/// キー名指定でセッションへ 1 キーを送る（`Enter` / `Escape` 等。`-l` なしの send-keys）。
/// プロンプト送達では送信の Enter を貼り付けと分離した単独キーとして送る（Issue #32）
pub fn send_key(socket: Option<&str>, session: &str, key: &str) -> Result<(), String> {
    run_tmux(
        socket,
        &["send-keys", "-t", &session_pane_target(session), key],
    )
    .map(|_| ())
}

/// テキストを tmux バッファ経由でセッションのアクティブ window へ貼り付ける。
/// `paste-buffer -p` はアプリが bracketed paste を要求している場合のみ貼り付け括りを
/// 付けるため、claude TUI にはマルチラインがそのまま入力欄へ載り、シェル等にも
/// 無害に劣化する（Issue #32）。バッファ名は並行配送で混線しないよう呼び出しごとに一意
pub fn paste_text(socket: Option<&str>, session: &str, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let buf_name = format!(
        "tako-paste-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    // load-buffer は stdin から読ませる（引数渡しはサイズ・エスケープの制約があるため）
    let mut child = tmux_command(socket)
        .args(["load-buffer", "-b", &buf_name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tmux を実行できない: {e}"))?;
    child
        .stdin
        .take()
        .expect("直前に Stdio::piped() を設定済み")
        .write_all(text.as_bytes())
        .map_err(|e| format!("tmux load-buffer へ書き込めない: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("tmux load-buffer の終了を待てない: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    // -d でバッファを使い捨てにする（tmux バッファ一覧を汚さない）
    run_tmux(
        socket,
        &[
            "paste-buffer",
            "-p",
            "-d",
            "-b",
            &buf_name,
            "-t",
            &session_pane_target(session),
        ],
    )
    .map(|_| ())
}

/// ペインログ用の観測結果（Issue #112）。バックエンドセッションのアクティブペインの
/// 履歴状態を 1 コマンドで取得する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneLogProbe {
    /// `#{history_size}`（履歴行数。増分検知の基準）
    pub history: usize,
    /// `#{history_limit}`（保持上限。飽和判定に使う）
    pub limit: usize,
    /// `#{history_bytes}`（履歴の格納バイト数。飽和後の変化検知に使う）
    pub bytes: u64,
    /// `#{alternate_on}`（内側アプリが alt screen = TUI 実行中か）
    pub alternate: bool,
}

/// バックエンドセッションのペインログ観測（Issue #112）。セッション消滅では None
pub fn pane_log_probe(socket: Option<&str>, session: &str) -> Option<PaneLogProbe> {
    let output = run_tmux(
        socket,
        &[
            "display-message",
            "-p",
            "-t",
            &session_pane_target(session),
            "#{history_size} #{history_limit} #{history_bytes} #{alternate_on}",
        ],
    )
    .ok()?;
    let line = output.lines().next()?;
    let mut f = line.split_whitespace();
    Some(PaneLogProbe {
        history: f.next()?.parse().ok()?,
        limit: f.next()?.parse().ok()?,
        bytes: f.next()?.parse().ok()?,
        alternate: f.next() == Some("1"),
    })
}

/// 履歴末尾の `count` 行を平文で取得する（Issue #112 ペインログ用）。
/// `-e` なし = ANSI 除去済み、`-J` なし = 折り返し行のまま（`#{history_size}` の
/// 行数カウントと 1:1 に対応する）。履歴が足りなければ取れた分だけ返す
pub fn capture_history_plain(
    socket: Option<&str>,
    session: &str,
    count: usize,
) -> Option<Vec<String>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let start = format!("-{count}");
    let output = run_tmux(
        socket,
        &[
            "capture-pane",
            "-p",
            "-t",
            &session_pane_target(session),
            "-S",
            &start,
            "-E",
            "-1",
        ],
    )
    .ok()?;
    Some(output.lines().map(|l| l.trim_end().to_string()).collect())
}

/// セッションが生きているか確認する（`has-session`）
pub fn session_alive(socket: Option<&str>, session: &str) -> bool {
    tmux_command(socket)
        .args(["has-session", "-t", &format!("={session}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
pub fn tmux_command(socket: Option<&str>) -> Command {
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
            let name = f.next()?.to_string();
            let created = f.next()?.parse().ok()?;
            let attached = f.next()? != "0";
            let pane_pid = f.next().and_then(|s| s.parse().ok());
            let pane_command = f.next().map(|s| s.to_string()).filter(|s| !s.is_empty());
            let pane_current_path = f.next().map(|s| s.to_string()).filter(|s| !s.is_empty());
            let last_activity = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            Some(TmuxSession {
                name,
                created,
                attached,
                windows: Vec::new(),
                client_ttys: Vec::new(),
                pane_pid,
                pane_command,
                pane_current_path,
                last_activity,
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
        let sessions = "work\t1760000000\t1\t1234\tzsh\t/Users/tako\t1760000500\nbg-agent\t1760000100\t0\t5678\tclaude\t/tmp\t1760000200\n";
        let windows = "work\t0\tvim\t1\t2\nwork\t1\tserver\t0\t1\nbg-agent\t0\tzsh\t1\t1\n";
        let clients = "work\t/dev/ttys012\n";
        let parsed = parse_sessions(sessions, windows, clients);
        assert_eq!(parsed.len(), 2);
        let work = &parsed[0];
        assert_eq!(work.name, "work");
        assert_eq!(work.created, 1760000000);
        assert!(work.attached);
        assert_eq!(work.pane_pid, Some(1234));
        assert_eq!(work.pane_command.as_deref(), Some("zsh"));
        assert_eq!(work.pane_current_path.as_deref(), Some("/Users/tako"));
        assert_eq!(work.last_activity, 1760000500);
        assert_eq!(work.windows.len(), 2);
        assert_eq!(work.windows[0].name, "vim");
        assert!(work.windows[0].active);
        assert_eq!(work.windows[1].panes, 1);
        assert_eq!(work.client_ttys, vec!["/dev/ttys012"]);
        let bg = &parsed[1];
        assert!(!bg.attached);
        assert_eq!(bg.pane_pid, Some(5678));
        assert_eq!(bg.pane_command.as_deref(), Some("claude"));
        assert!(bg.client_ttys.is_empty());
    }

    #[test]
    fn 空入力と壊れた行は無害に読み飛ばす() {
        assert!(parse_sessions("", "", "").is_empty());
        let parsed = parse_sessions("only-name\nok\t1\t0\n", "broken\n", "broken\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "ok");
    }

    #[test]
    fn 旧フォーマット3列でもパースできる() {
        let parsed = parse_sessions("old\t1760000000\t0\n", "", "");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "old");
        assert_eq!(parsed[0].pane_pid, None);
        assert_eq!(parsed[0].pane_command, None);
        assert_eq!(parsed[0].last_activity, 0);
    }

    #[test]
    fn クライアントpid一覧をパースする() {
        let parsed = parse_client_pids("1234\ttako-abc\n5678\tmaster-tako\n");
        assert_eq!(
            parsed,
            vec![
                (1234, "tako-abc".to_string()),
                (5678, "master-tako".to_string())
            ]
        );
        // 壊れた行（pid 非数値・タブ無し）と空入力は読み飛ばす
        assert_eq!(parse_client_pids("x\ttako-abc\nno-tab\n"), vec![]);
        assert!(parse_client_pids("").is_empty());
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
        let _cleanup = crate::tmux_backend::TmuxTestGuard::new(vec![socket.clone()]);

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

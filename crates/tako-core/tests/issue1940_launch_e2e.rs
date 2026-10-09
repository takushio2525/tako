//! 起動コマンドの送達を「行数の少ないペイン + 準備の遅いシェル」で確かめる（#1940）
//!
//! 本番（10/9）で worker が立たなかった 4 件は、どれも worker ペインが **2〜3 行**まで
//! 潰れていた（`tako list` の rows=2 を実測）。zsh は 2 行では入力行を 1 行の横スクロール
//! （`<…`）、3 行では `>....` に畳むので、**起動コマンドの全文が画面に一度も出ない**。
//! 全文一致でエコーを確かめる送達フローはそこで書き直しを使い切り、最後に
//! 「消していない前の本文 + 本文 + Enter」を実行して `--permission-mode autoexport` に化けた。
//!
//! ここでは本番と同じ経路（`TmuxBackend` の器 + `TerminalSession` + `ShellSendFlow` を
//! 500ms tick で回す）を、隔離ソケット・一時 data dir・遅い `.envrc`（direnv の実物）・
//! 偽の claude（受け取った引数を記録し、不正な `--permission-mode` で落ちる）で再現する。
//!
//! 既定では走らない（tmux / zsh / direnv と実シェルを起動する実測）。実測は
//! `cargo test -p tako-core --test issue1940_launch_e2e -- --ignored --nocapture --test-threads=1`。
//! 旧挙動は同じバイナリで `TAKO_1940_LEGACY=1` を付けて測る。
//! 寸法と遅さは `TAKO_E1940_ROWS` / `TAKO_E1940_COLS` / `TAKO_E1940_SLOW`（秒）で変えられる。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tako_core::backend::{SessionBackend, SessionRef, TmuxBackend};
use tako_core::shell_send::{ShellObservation, ShellSendAction, ShellSendFlow, TICK_MS};
use tako_core::terminal::{SpawnOptions, TerminalSession};

/// 本番の `COMMAND_FLOW_TIMEOUT`（tako-app）と揃える。短く切ると製品ではなく
/// ハーネスの上限を測ることになる
const FLOW_LIMIT: Duration = Duration::from_secs(120);

/// 偽の claude。引数を 1 行ずつ記録し、`--permission-mode` が既知の値でなければ
/// 本物と同じ文言で落ちる。既知なら起動したことを示して居座る（TUI の代わり）
const FAKE_CLAUDE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"
mode=""; prev=""
for a in "$@"; do
  if [ "$prev" = "--permission-mode" ]; then mode="$a"; fi
  prev="$a"
done
case "$mode" in
  auto|default|plan|acceptEdits|bypassPermissions)
    echo "FAKECLAUDE-STARTED"
    exec sleep 600 ;;
  *)
    echo "error: option '--permission-mode <mode>' argument '$mode' is invalid."
    exit 1 ;;
esac
"#;

fn have(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg(if bin == "tmux" { "-V" } else { "--version" })
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// 1 試行ぶんの置き場。Drop で器のサーバーごと片付ける（自分のソケットだけ）
struct Fixture {
    root: PathBuf,
    socket: String,
    work: PathBuf,
    launches: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn new(tag: &str, slow_secs: u64) -> Self {
        // ソケットのパス長（104 バイト）に収まるよう、置き場は短い一時 dir にする
        let root = std::env::temp_dir().join(format!("t1940-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let zdot = root.join("zdot");
        let work = root.join("work");
        let bin = root.join("bin");
        for d in [&zdot, &work, &bin, &root.join("xd"), &root.join("xc")] {
            std::fs::create_dir_all(d).unwrap();
        }
        // ユーザーの rc は読まない（検証が本番の履歴・設定へ触れない）。
        // 本番と同じ形のプロンプト + direnv の hook だけを持つ
        std::fs::write(
            zdot.join(".zshrc"),
            "PS1=\"[test@host:%1~]\\$ \"\neval \"$(direnv hook zsh)\"\n",
        )
        .unwrap();
        std::fs::write(
            work.join(".envrc"),
            format!("sleep {slow_secs}\nexport CLAUDE_CONFIG_DIR=\"/tmp/fake-config\"\n"),
        )
        .unwrap();
        let fake = bin.join("claude");
        std::fs::write(&fake, FAKE_CLAUDE).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let allow = std::process::Command::new("direnv")
            .arg("allow")
            .arg(&work)
            .env("XDG_DATA_HOME", root.join("xd"))
            .env("XDG_CONFIG_HOME", root.join("xc"))
            .output()
            .expect("direnv allow");
        assert!(allow.status.success(), "direnv allow が通る");
        Self {
            socket: format!("t1940-{tag}-{}", std::process::id()),
            launches: root.join("launches.log"),
            work,
            root,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        vec![
            (
                "TAKO_ORIG_ZDOTDIR".into(),
                self.root.join("zdot").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".into(),
                self.root.join("xd").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                self.root.join("xc").display().to_string(),
            ),
            ("PATH".into(), path),
            ("FAKE_LOG".into(), self.launches.display().to_string()),
            // シェル統合のフック（OSC 133）は TAKO_PANE_ID を見て登録される
            ("TAKO_PANE_ID".into(), "1940".into()),
        ]
    }

    fn launches(&self) -> Vec<String> {
        std::fs::read_to_string(&self.launches)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

type Rx = futures::channel::mpsc::UnboundedReceiver<tako_core::SessionEvent>;

fn spawn(fx: &Fixture, cols: usize, rows: usize) -> (TerminalSession, Rx) {
    unsafe {
        std::env::remove_var("TMUX");
    }
    let backend = TmuxBackend::with_socket(fx.socket.clone());
    let options = backend.wrap_spawn(
        SpawnOptions {
            command: None,
            cwd: Some(fx.work.clone()),
            env: fx.env(),
            scrollback_lines: None,
        },
        &SessionRef::new(format!("{}-s", fx.socket)).unwrap(),
    );
    TerminalSession::spawn(cols, rows, options).expect("PTY を起こせる")
}

/// 溜まったイベントを本番の UI スレッドと同じく `process_event` へ通す
/// （OSC 133 の実行印はここで session へ反映される）
fn pump(session: &mut TerminalSession, rx: &mut Rx) {
    while let Ok(ev) = rx.try_recv() {
        let _ = session.process_event(ev);
    }
}

/// 本番の worker の起動コマンドと同じ形（export の前置き + role のインライン前置き + 引数）
fn launch_command() -> String {
    "export CLAUDE_CONFIG_DIR='/tmp/fake-config-dir'; \
     TAKO_ORCHESTRATOR_ROLE='worker:tako:#1940 label' claude --model claude-opus-5-5 \
     --effort xhigh --remote-control --permission-mode auto"
        .to_string()
}

struct Outcome {
    verified: Option<bool>,
    confirmation: &'static str,
    elapsed: Duration,
    rewrites: u32,
    launches: Vec<String>,
}

/// 本番の `drive_command_flows` と同じ回し方で 1 回送る
fn run_flow(fx: &Fixture, session: &mut TerminalSession, rx: &mut Rx) -> Outcome {
    let mut flow = ShellSendFlow::new(launch_command());
    let t0 = Instant::now();
    let verbose = std::env::var_os("TAKO_E1940_VERBOSE").is_some();
    let mut verified = None;
    while t0.elapsed() < FLOW_LIMIT {
        pump(session, rx);
        let screen = session.visible_lines();
        // 本番の `drive_command_flows` と同じ材料（画面 + 桁数 + シェル統合の印）
        let observed = ShellObservation {
            screen: &screen,
            cols: session.size().0,
            marks: session.shell_marks(),
            // 本番の GUI は別ペインで印を観測済み（統合の効いた環境）。
            // `TAKO_E1940_NO_MARK_WAIT=1` で「印をまだ見ていないプロセス」を再現する
            marks_expected: std::env::var_os("TAKO_E1940_NO_MARK_WAIT").is_none(),
        };
        let action = flow.observe(&observed);
        if verbose {
            println!(
                "{:>6}ms {} action={} 印={:?} 画面={:?}",
                t0.elapsed().as_millis(),
                flow.stage_name(),
                match &action {
                    ShellSendAction::Wait => "wait".to_string(),
                    ShellSendAction::Write(b) => format!("write({})", b.len()),
                    ShellSendAction::Done { verified } => format!("done({verified})"),
                },
                observed.marks,
                screen
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.chars().take(70).collect::<String>())
                    .collect::<Vec<_>>()
            );
        }
        match action {
            ShellSendAction::Wait => {}
            ShellSendAction::Write(bytes) => session.write(bytes),
            ShellSendAction::Done { verified: v } => {
                verified = Some(v);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
    // 起動先が引数を記録し終えるのを待つ（実行の観測はファイルが正）
    let settle = Instant::now();
    let wait = Duration::from_secs(5 + env_num("TAKO_E1940_SLOW", 3));
    while settle.elapsed() < wait && fx.launches().is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(500));
    Outcome {
        verified,
        confirmation: flow.confirmation().as_str(),
        elapsed: t0.elapsed(),
        rewrites: flow.rewrites(),
        launches: fx.launches(),
    }
}

fn assert_launched_once(tag: &str, out: &Outcome) {
    println!(
        "[{tag}] verified={:?} 確かめ方={} 経過={:.1}s 書き直し={} 起動={} 回: {:?}",
        out.verified,
        out.confirmation,
        out.elapsed.as_secs_f32(),
        out.rewrites,
        out.launches.len(),
        out.launches
    );
    assert_eq!(
        out.launches.len(),
        1,
        "[{tag}] 起動先は 1 回だけ起動する（化けた行・二重起動が無い）: {:?}",
        out.launches
    );
    assert!(
        out.launches[0].ends_with("--permission-mode auto"),
        "[{tag}] 正しい引数で起動する: {:?}",
        out.launches[0]
    );
    assert_eq!(out.verified, Some(true), "[{tag}] 実行まで確かめて完了する");
}

fn prerequisites() -> bool {
    if cfg!(windows) {
        eprintln!("macOS / Linux の実測（tmux + zsh + direnv）なのでスキップ");
        return false;
    }
    for bin in ["tmux", "zsh", "direnv"] {
        if !have(bin) {
            eprintln!("{bin} が無いのでスキップ");
            return false;
        }
    }
    true
}

/// 本番 10/9 の型 2: 行数の少ないペイン + 準備の遅いシェル
#[test]
#[ignore = "tmux / zsh / direnv と実シェルを起動する実測用"]
fn 実測_行数の少ないペインと遅いシェルでも起動コマンドは1回だけ正しく届く() {
    if !prerequisites() {
        return;
    }
    let rows = env_num("TAKO_E1940_ROWS", 2) as usize;
    let cols = env_num("TAKO_E1940_COLS", 125) as usize;
    let slow = env_num("TAKO_E1940_SLOW", 3);
    let fx = Fixture::new("short", slow);
    let (mut session, mut rx) = spawn(&fx, cols, rows);
    let out = run_flow(&fx, &mut session, &mut rx);
    assert_launched_once(&format!("{cols}x{rows} slow={slow}s"), &out);
}

/// 対照: 行数の足りるペインでは旧来どおり画面のエコーで確かめて届く
#[test]
#[ignore = "tmux / zsh / direnv と実シェルを起動する実測用"]
fn 実測_行数の足りるペインと遅いシェルでも起動コマンドは1回だけ正しく届く() {
    if !prerequisites() {
        return;
    }
    let slow = env_num("TAKO_E1940_SLOW", 3);
    let fx = Fixture::new("tall", slow);
    let (mut session, mut rx) = spawn(&fx, 63, 30);
    let out = run_flow(&fx, &mut session, &mut rx);
    assert_launched_once(&format!("63x30 slow={slow}s"), &out);
}

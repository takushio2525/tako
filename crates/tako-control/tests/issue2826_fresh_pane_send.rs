//! #2826: 作りたてのペイン（まだ最初のプロンプトを出していないシェル）へ、**作った直後に**
//! 改行つきの行を送ったとき、その行が実行されるかを実 PTY / 実 ConPTY で数える。
//!
//! 外部の GUI フロントエンドは `tako_create_tab` の直後に `tako_send_input("tako master")` を
//! 送り、tako 自身の `tako master --tab` / `tako solo --tab` も TabNew の直後に素の Send を撃つ。
//! どちらも dispatch の `Request::Send`（CLI / MCP の 1 実装）を通るので、ここも
//! **dispatch をそのまま呼ぶ**（GUI の代わりの host は本物の端末を 1 枚持ち、GUI と同じく
//! 「素のシェルとして起こしてからの経過」と「送達フローが残っているか」に答える）。
//!
//! 腕は 3 つ:
//!
//! - `send`: dispatch の `Request::Send`（newline = true）をペインを作った直後に撃つ。
//!   送る時点でまだ最初のプロンプトの印が無ければ、修正後は送達フロー（シェル準備待ち →
//!   エコー確認 → 分離 Enter。#640）へ寄って全回届く。印を受けた後なら修正前と同じ経路
//!   （`TAKO2826_DELAY_MS=300` だと器なしの zsh は印を出し終えている）。
//!   `TAKO_2826_LEGACY=1` は修正前（即時書き込み）を記録だけする
//! - `flow`: 参照。worker の spawn / スターターと同じ `queue_command_flow` へ直接積む
//! - `prompted`: 最初のプロンプトの印（OSC 133;A）を受けてから送る。修正の前後で経路が同じ
//!   （器なしは即時書き込みで `null` の応答・器は外側の代替画面 = TUI 向けの送達フロー）。
//!   印の届かない組み合わせ（テストの host は器の側路を読まない = psmux）は記録して飛ばす
//!
//! 修正前の実測（Windows CI・psmux 3.4.0）: 作ってから 300 ms 後の即時書き込みは全損した
//! （#2092 の門が開いた後・器のクライアントが入力を読み始める前の窓）。#2092 の門を外す
//! `TAKO_2092_LEGACY=1` では 0 ms でも 0/10。数字は Issue #2826 のコメント。
//!
//! 組み合わせ（シェル × 器）はテスト関数 1 本ずつで、それぞれ [`tako_core::run_alone!`] の子で
//! 走る（シェル統合の印を見たか = `shell_marks_seen` はプロセスの状態なので、前の組み合わせの
//! 印を持ち越さない。env の書き換えも子の中だけ）。
//!
//! 結果は `TAKO2826_ROUND` / `TAKO2826_SUMMARY` の行で出す（CI のログから回数を拾う）。
//! 回数は `TAKO2826_ROUNDS`（既定 1）、作ってから送るまでの遅れは `TAKO2826_DELAY_MS`（既定 0）。

use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedReceiver;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::shell_send::{ShellObservation, ShellSendAction, ShellSendFlow};
use tako_core::terminal::SessionEvent;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

#[cfg(windows)]
#[path = "../../tako-core/tests/common/psmux_ctl.rs"]
mod psmux_ctl;

#[cfg(unix)]
#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

/// GUI の代わり: ペイン 1 枚に本物の端末を持たせ、`queue_command_flow` を GUI と同じ
/// 500 ms の tick で回す（`tako-app` の `drive_command_flows` と同じ観測と書き込み）
struct Host {
    workspace: Workspace,
    pane: PaneId,
    term: TerminalSession,
    rx: UnboundedReceiver<SessionEvent>,
    /// 端末を起こした時刻（GUI の `fresh_shell_spawns` と同じ「素のシェルとして起こした時刻」）
    born: Instant,
    flows: VecDeque<ShellSendFlow>,
    /// `queue_command_flow` が呼ばれた回数
    flow_calls: u32,
    /// `queue_send_flow`（全画面 TUI 向けの送達フロー）が呼ばれた回数。GUI ではそちらの
    /// フロー（2 秒待って貼り付け + 分離 Enter）が送るが、ここには無い = 結果は `tui_flow`
    tui_flow_calls: u32,
    last_tick: Instant,
    /// 器のセッション名（GUI の `backend_session` と同じ答え）
    backend: Option<String>,
}

/// ペイン 1 枚のワークスペース（端末を起こす前に ID を決める = GUI と同じく
/// `TAKO_PANE_ID` を渡せる。zsh の統合はこれが無いとフックを登録しない）
fn new_workspace() -> (Workspace, PaneId) {
    let workspace = Workspace::new("1", Pane::new(PaneOrigin::User));
    let pane = workspace.active_tab().tree().focused();
    (workspace, pane)
}

/// GUI の `spawn_session` がペインの env へ足すもののうち、シェルの起動に効くもの
fn pane_env(pane: PaneId) -> Vec<(String, String)> {
    vec![("TAKO_PANE_ID".into(), pane.to_string())]
}

impl Host {
    fn new(
        (workspace, pane): (Workspace, PaneId),
        term: TerminalSession,
        rx: UnboundedReceiver<SessionEvent>,
        born: Instant,
    ) -> Self {
        Self {
            workspace,
            pane,
            term,
            rx,
            born,
            flows: VecDeque::new(),
            flow_calls: 0,
            tui_flow_calls: 0,
            last_tick: Instant::now(),
            backend: None,
        }
    }

    /// 届いている出力を画面へ反映する（**汲むのが必須**: 器のクライアントは端末クエリへの
    /// 応答を待つ）。積まれた送達フローは 500 ms ごとに 1 段進める
    fn step(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            let _ = self.term.process_event(event);
        }
        if self.flows.is_empty() || self.last_tick.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_tick = Instant::now();
        let screen = self.term.visible_lines();
        let observed = ShellObservation {
            screen: &screen,
            cols: self.term.size().0,
            marks: self.term.shell_marks(),
            marks_expected: tako_core::terminal::shell_marks_seen(),
        };
        let flow = self.flows.front_mut().expect("空でない");
        match flow.observe(&observed) {
            ShellSendAction::Wait => {}
            ShellSendAction::Write(bytes) => self.term.write(bytes),
            ShellSendAction::Done { .. } => {
                let done = self.flows.pop_front().expect("空でない");
                eprintln!(
                    "TAKO2826_FLOW_DONE confirmation={} rewrites={}",
                    done.confirmation().as_str(),
                    done.rewrites()
                );
            }
        }
    }

    fn screen(&self) -> Vec<String> {
        self.term.visible_lines()
    }
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
        (pane == self.pane).then_some(&self.term)
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
    fn queue_command_flow(&mut self, pane: PaneId, command: String) {
        assert_eq!(pane, self.pane, "別のペインへ積んだ");
        self.flow_calls += 1;
        self.flows.push_back(ShellSendFlow::new(command));
    }
    fn queue_send_flow(&mut self, pane: PaneId, _text: String) {
        assert_eq!(pane, self.pane, "別のペインへ積んだ");
        self.tui_flow_calls += 1;
    }
    fn fresh_shell_age(&self, pane: PaneId) -> Option<Duration> {
        (pane == self.pane).then(|| self.born.elapsed())
    }
    fn command_flow_pending(&self, pane: PaneId) -> bool {
        pane == self.pane && !self.flows.is_empty()
    }
}

impl TmuxHost for Host {
    fn backend_session(&self, pane: PaneId) -> Option<String> {
        (pane == self.pane).then(|| self.backend.clone()).flatten()
    }
}
impl PreviewHost for Host {}
impl SystemHost for Host {}
impl UiStateHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}

/// シェルの方言（送る行と、実行されたときにだけ出る行）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 片方の OS の組み合わせしか作らない（unix は Posix・Windows は PowerShell）
#[allow(dead_code)]
enum Dialect {
    PowerShell,
    Posix,
}

impl Dialect {
    /// 送る行と、実行されたときにだけ画面に出る行。送った行のエコーには `40+2` が
    /// そのまま写るので、`42` の行が出たら**実行された**と言える
    fn probe(self, round: usize) -> (String, String) {
        let expected = format!("TAKO2826-R{round}-42");
        let line = match self {
            Self::PowerShell => format!("echo \"TAKO2826-R{round}-$(40+2)\""),
            Self::Posix => format!("echo TAKO2826-R{round}-$((40+2))"),
        };
        (line, expected)
    }
}

/// 1 回の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// 送った行が実行された
    Delivered,
    /// 送った行の一部は画面に出たが、実行された印（`42` の行）が出ない
    Partial,
    /// 何も出ない
    Lost,
    /// 全画面 TUI 向けの送達フローへ回った（ここでは送らない = 測っていない）
    TuiFlow,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Partial => "partial",
            Self::Lost => "lost",
            Self::TuiFlow => "tui_flow",
        }
    }
}

/// 送った行が実行されるのを待つ。出たら `Delivered`。画面が落ち着いたまま
/// 変わらなくなったら（もう何も起きない）打ち切る
fn wait_outcome(host: &mut Host, expected: &str, limit: Duration) -> (Outcome, Option<u128>) {
    let started = Instant::now();
    let mut last = String::new();
    let mut still_since = Instant::now();
    loop {
        host.step();
        let screen = host.screen();
        if screen.iter().any(|l| l.trim() == expected) {
            return (Outcome::Delivered, Some(started.elapsed().as_millis()));
        }
        let joined = screen.join("\n");
        if joined != last {
            last = joined;
            still_since = Instant::now();
        }
        // 積んだフローが残っている間は打ち切らない（フローの決着まで待つ）
        let settled = host.flows.is_empty()
            && !last.trim().is_empty()
            && still_since.elapsed() >= Duration::from_secs(8);
        if settled || started.elapsed() >= limit {
            let partial = last.contains("TAKO2826");
            let outcome = if partial {
                Outcome::Partial
            } else {
                Outcome::Lost
            };
            return (outcome, None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// どの腕で送るか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// dispatch の `Request::Send`（CLI `tako send` / MCP `tako_send_input` と同じ 1 実装）
    Send,
    /// 参照: spawn / スターターと同じ `queue_command_flow` へ直接積む
    Flow,
    /// 最初のプロンプトの印を受けてから dispatch の `Request::Send` を撃つ
    Prompted,
}

impl Arm {
    fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Flow => "flow",
            Self::Prompted => "prompted",
        }
    }
}

/// ペインを作る口（組み合わせごと）。作った直後の端末を返す。1 つ目の引数は組み合わせの中で
/// 一意な通し番号（器のセッション名に使う。腕をまたいで同じ名前にすると `new-session -A` が
/// 前の腕のセッションへ繋ぎ直し、前の出力を拾ってしまう）
type Spawner<'a> = dyn FnMut(
        usize,
        PaneId,
    ) -> (
        TerminalSession,
        UnboundedReceiver<SessionEvent>,
        Option<String>,
    ) + 'a;

/// `TAKO2826_ROUNDS` / `TAKO2826_DELAY_MS`
fn env_number(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// dispatch の `Request::Send`（改行つき）
fn send(host: &mut Host, line: String) -> serde_json::Value {
    let pane = host.pane.as_u64();
    tako_control::dispatch::dispatch(
        host,
        tako_control::protocol::Request::Send {
            pane: Some(pane),
            text: line,
            newline: true,
            tmux_session: None,
            await_prompt: false,
            paste_guard: None,
            attachments: Vec::new(),
        },
        PaneOrigin::Mcp,
    )
    .expect("送信が受け付けられる")
}

/// 1 つの組み合わせを `rounds` 回 × 3 腕で測り、修正後の約束を確かめる
fn measure(combo: &str, dialect: Dialect, spawn: &mut Spawner<'_>) {
    let rounds = env_number("TAKO2826_ROUNDS", 1) as usize;
    // 作ってから送るまでの遅れ（ms）。0 = TabNew の応答の直後（外部のフロントエンドの形）。
    // `tako master --tab` は TabNew → TabRename → Title → Send の往復を挟む
    let delay = Duration::from_millis(env_number("TAKO2826_DELAY_MS", 0));
    let legacy = tako_core::shell_send::legacy_2826();
    let mut broken = Vec::new();
    let mut seq = 0usize;
    for arm in [Arm::Send, Arm::Flow, Arm::Prompted] {
        let mut delivered = 0usize;
        let mut ran = 0usize;
        for round in 0..rounds {
            let (workspace, pane) = new_workspace();
            let (term, rx, backend) = spawn(seq, pane);
            seq += 1;
            let mut host = Host::new((workspace, pane), term, rx, Instant::now());
            host.backend = backend;
            if arm == Arm::Prompted {
                let deadline = Instant::now() + Duration::from_secs(20);
                while host.term.shell_marks().prompts == 0 && Instant::now() < deadline {
                    host.step();
                    std::thread::sleep(Duration::from_millis(10));
                }
                if host.term.shell_marks().prompts == 0 {
                    eprintln!(
                        "TAKO2826_SKIP combo={combo} arm=prompted \
                         reason=プロンプトの印が届かない（器の側路を読まない host）"
                    );
                    break;
                }
            } else {
                while host.born.elapsed() < delay {
                    host.step();
                    std::thread::sleep(Duration::from_millis(5));
                }
                host.step();
            }
            ran += 1;
            let alt_at_send = host.term.is_alt_screen();
            // 送る時点で最初のプロンプトの印を受けていたか（遅れがあると器なしの zsh は 300 ms で
            // 出している = 作りたてではない = 修正前と同じ経路が正しい）
            let prompts_at_send = host.term.shell_marks().prompts;
            let (line, expected) = dialect.probe(round);
            let reply = match arm {
                Arm::Send | Arm::Prompted => send(&mut host, line),
                Arm::Flow => {
                    host.queue_command_flow(pane, line);
                    serde_json::Value::Null
                }
            };
            let sent_ms = host.born.elapsed().as_millis();
            let (outcome, at) = if host.tui_flow_calls > 0 {
                (Outcome::TuiFlow, None)
            } else {
                wait_outcome(&mut host, &expected, Duration::from_secs(60))
            };
            if outcome == Outcome::Delivered {
                delivered += 1;
            }
            let shell_wait = reply["shell_wait"].as_str().unwrap_or("-");
            eprintln!(
                "TAKO2826_ROUND combo={combo} arm={} round={round} sent_ms={sent_ms} \
                 alt_at_send={alt_at_send} prompts_at_send={prompts_at_send} queued={} \
                 shell_wait={shell_wait} flow_calls={} outcome={} output_ms={} prompts={} alt_end={}",
                arm.as_str(),
                reply["queued"].as_bool().unwrap_or(false),
                host.flow_calls,
                outcome.as_str(),
                at.map_or_else(|| "-".to_string(), |ms| ms.to_string()),
                host.term.shell_marks().prompts,
                host.term.is_alt_screen(),
            );
            if outcome != Outcome::Delivered && cfg!(windows) {
                // ランナーの画面だけ（手元の画面はホスト名・パスを含むので出さない）
                for l in host.screen().iter().filter(|l| !l.trim().is_empty()) {
                    eprintln!("TAKO2826_SCREEN combo={combo} round={round} | {l}");
                }
            }
            // 修正後の約束（`TAKO_2826_LEGACY=1` の send の腕は記録だけ = 修正前の実測）
            let ok = match arm {
                Arm::Send if legacy => true,
                // まだ最初のプロンプトの印が無い = 送達フローへ寄って全回届く
                Arm::Send if prompts_at_send == 0 => {
                    shell_wait == "first_prompt" && outcome == Outcome::Delivered
                }
                Arm::Flow => outcome == Outcome::Delivered,
                // 印を受けた後は修正前と同じ経路: 送達フローを積まない。器なしは即時書き込み
                // （`null`）で届き、器は外側の代替画面 = TUI 向けの送達フロー
                Arm::Send | Arm::Prompted => {
                    host.flow_calls == 0
                        && reply.get("shell_wait").is_none()
                        && (alt_at_send || (reply.is_null() && outcome == Outcome::Delivered))
                }
            };
            if !ok {
                broken.push(format!(
                    "{}#{round}（shell_wait={shell_wait} outcome={}）",
                    arm.as_str(),
                    outcome.as_str()
                ));
            }
        }
        eprintln!(
            "TAKO2826_SUMMARY combo={combo} arm={} delivered={delivered}/{ran}",
            arm.as_str()
        );
    }
    assert!(
        broken.is_empty(),
        "{combo}: 作りたてのペインへの送信の約束が崩れた（legacy={legacy}）: {}",
        broken.join(" / ")
    );
}

// --- unix（zsh / bash の作りたてのペイン） ------------------------------------------

/// 利用者の rc を読まない砂場の HOME（`.zshrc` / `.bashrc` は空。`TAKO2826_RC_SLEEP=<秒>` で
/// 重い rc（pyenv / direnv の初期化）の代わりに眠らせる）。**子の中だけで** env を書き換える
#[cfg(unix)]
fn sandbox_home(dir: &Path) {
    tako_core::test_child::assert_alone("sandbox_home");
    let sleep = std::env::var("TAKO2826_RC_SLEEP").unwrap_or_default();
    let rc = if sleep.is_empty() {
        String::new()
    } else {
        format!("sleep {sleep}\n")
    };
    for name in [".zshrc", ".bashrc", ".bash_profile"] {
        std::fs::write(dir.join(name), &rc).expect("砂場の rc を書ける");
    }
    std::env::set_var("HOME", dir);
    std::env::remove_var("ZDOTDIR");
}

#[cfg(unix)]
fn shell_options(dir: &Path, pane: PaneId) -> SpawnOptions {
    SpawnOptions {
        command: None,
        cwd: Some(dir.to_path_buf()),
        env: pane_env(pane),
        scrollback_lines: None,
    }
}

#[cfg(unix)]
#[test]
fn 器なしのログインシェルの作りたてのペイン() {
    if tako_core::run_alone!(器なしのログインシェルの作りたてのペイン) {
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("2826-direct");
    sandbox_home(scratch.path());
    measure("unix/direct", Dialect::Posix, &mut |_, pane| {
        let (term, rx) = TerminalSession::spawn(120, 40, shell_options(scratch.path(), pane))
            .expect("PTY を張れる");
        (term, rx, None)
    });
}

#[cfg(unix)]
#[test]
fn 器ありのログインシェルの作りたてのペイン() {
    use tako_core::backend::{SessionBackend, SessionRef};
    if tako_core::run_alone!(器ありのログインシェルの作りたてのペイン) {
        return;
    }
    if !tako_core::tmux_test_gate::real_tmux() {
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("2826-tmux");
    sandbox_home(scratch.path());
    let socket = tmux_e2e::socket_for("2826");
    let backend = tako_core::backend::TmuxBackend::with_socket(socket.to_string());
    measure("unix/tmux", Dialect::Posix, &mut |seq, pane| {
        let name = format!("tako2826-{}-{seq}", std::process::id());
        let session = SessionRef::new(name.clone()).expect("器の名前");
        let options = backend.wrap_spawn(shell_options(scratch.path(), pane), &session);
        let (term, rx) = TerminalSession::spawn(120, 40, options).expect("PTY を張れる");
        (term, rx, Some(name))
    });
    // 器ごと畳む（ソケットファイルまで消す 1 実装）
    tmux_e2e::release_session(socket, "tako2826-none");
}

// --- Windows（pwsh 7 / PowerShell 5.1 × 器なし / psmux） ------------------------------

#[cfg(windows)]
mod windows {
    use super::*;
    use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
    use tako_core::encoding_pin::ContainerPin;
    use tako_core::terminal::SpawnCommand;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }

    /// 出荷構成（コンソールを持たない GUI プロセス）に合わせる（#2092 の門が効く条件）
    pub(super) fn ship_like() {
        unsafe {
            FreeConsole();
        }
    }

    /// 製品の既定シェル（`pane_shell` = 統合を読ませる PowerShell）が pwsh 7 か
    pub(super) fn default_is_pwsh7() -> bool {
        tako_core::shell_integration::pane_shell()
            .is_some_and(|c| c.program.to_ascii_lowercase().ends_with("pwsh.exe"))
    }

    /// 同梱の Windows PowerShell 5.1 を、製品と同じ統合の引数で起こす形
    pub(super) fn ps51() -> SpawnCommand {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let program = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        match tako_core::shell_integration::pane_shell() {
            Some(mut c) => {
                c.program = program;
                c
            }
            None => SpawnCommand {
                program,
                args: Vec::new(),
            },
        }
    }

    pub(super) fn options(dir: &Path, command: Option<SpawnCommand>, pane: PaneId) -> SpawnOptions {
        SpawnOptions {
            command,
            cwd: Some(dir.to_path_buf()),
            env: pane_env(pane),
            scrollback_lines: None,
        }
    }

    pub(super) fn psmux_bin() -> Option<String> {
        if let Some(bin) = std::env::var("TAKO_PSMUX_BIN")
            .ok()
            .filter(|s| !s.is_empty())
        {
            return psmux_ctl::probe(&bin).then_some(bin);
        }
        psmux_ctl::probe("psmux").then(|| "psmux".to_string())
    }

    /// 隔離ソケット上の器（`&'static` が要るので backend は leak する。テスト 1 本に 1 つ）
    pub(super) struct Vessel {
        pub backend: &'static PsmuxBackend,
        bin: String,
        socket: String,
    }

    impl Vessel {
        pub(super) fn new(tag: &str) -> Option<Self> {
            let bin = psmux_bin()?;
            let socket = format!("tako-2826-{}-{tag}", std::process::id());
            let owners = tako_core::test_residue::process_scratch(&format!("2826-owners-{tag}"));
            let backend: &'static PsmuxBackend = Box::leak(Box::new(PsmuxBackend::with_parts(
                bin.clone(),
                "3.4.0".into(),
                socket.clone(),
                owners,
            )));
            Some(Self {
                backend,
                bin,
                socket,
            })
        }

        /// 器の中でシェルを起こすペイン（器の内側の固定も同じ門へ載せる = 本番の GUI と同じ）
        pub(super) fn pane(
            &self,
            name: &str,
            options: SpawnOptions,
        ) -> (TerminalSession, UnboundedReceiver<SessionEvent>) {
            let session = SessionRef::new(name).expect("器の名前");
            let options = self.backend.wrap_spawn(options, &session);
            TerminalSession::spawn_in(
                120,
                40,
                options,
                Some(ContainerPin::new(self.backend, session)),
            )
            .expect("ConPTY を起動できる")
        }
    }

    impl Drop for Vessel {
        fn drop(&mut self) {
            psmux_ctl::kill_server(&self.bin, &self.socket);
        }
    }

    pub(super) fn direct(dir: &Path, command: Option<SpawnCommand>, combo: &str) {
        measure(combo, Dialect::PowerShell, &mut |_, pane| {
            let (term, rx) =
                TerminalSession::spawn_in(120, 40, options(dir, command.clone(), pane), None)
                    .expect("ConPTY を起動できる");
            (term, rx, None)
        });
    }

    pub(super) fn vessel(dir: &Path, command: Option<SpawnCommand>, combo: &str, tag: &str) {
        let Some(vessel) = Vessel::new(tag) else {
            eprintln!("skip: psmux が無い環境（TAKO_PSMUX_BIN / PATH）");
            return;
        };
        let mut names = Vec::new();
        measure(combo, Dialect::PowerShell, &mut |seq, pane| {
            let name = format!("tako-2826-{tag}-{seq}");
            names.push(name.clone());
            let (term, rx) = vessel.pane(&name, options(dir, command.clone(), pane));
            (term, rx, Some(name))
        });
        for name in names {
            let _ = vessel
                .backend
                .kill(&SessionRef::new(name.as_str()).unwrap());
        }
    }
}

#[cfg(windows)]
#[test]
fn 器なしのpwsh7の作りたてのペイン() {
    if tako_core::run_alone!(器なしのpwsh7の作りたてのペイン) {
        return;
    }
    windows::ship_like();
    if !windows::default_is_pwsh7() {
        eprintln!("skip: pwsh 7 が無い環境");
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("2826-pwsh7");
    // 製品の既定（command 無し = `pane_shell` が pwsh 7 を選ぶ）
    windows::direct(scratch.path(), None, "pwsh7/direct");
}

#[cfg(windows)]
#[test]
fn 器なしのpowershell51の作りたてのペイン() {
    if tako_core::run_alone!(器なしのpowershell51の作りたてのペイン) {
        return;
    }
    windows::ship_like();
    let scratch = tako_core::test_residue::ScratchDir::new("2826-ps51");
    windows::direct(scratch.path(), Some(windows::ps51()), "ps51/direct");
}

#[cfg(windows)]
#[test]
fn 器ありのpwsh7の作りたてのペイン() {
    if tako_core::run_alone!(器ありのpwsh7の作りたてのペイン) {
        return;
    }
    windows::ship_like();
    if !windows::default_is_pwsh7() {
        eprintln!("skip: pwsh 7 が無い環境");
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("2826-vpwsh7");
    windows::vessel(scratch.path(), None, "pwsh7/psmux", "p7");
}

#[cfg(windows)]
#[test]
fn 器ありのpowershell51の作りたてのペイン() {
    if tako_core::run_alone!(器ありのpowershell51の作りたてのペイン) {
        return;
    }
    windows::ship_like();
    let scratch = tako_core::test_residue::ScratchDir::new("2826-vps51");
    windows::vessel(scratch.path(), Some(windows::ps51()), "ps51/psmux", "p51");
}

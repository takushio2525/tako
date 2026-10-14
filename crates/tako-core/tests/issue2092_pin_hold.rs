//! #2092: 文字コード固定の決着まで入力を預かる門の実測（**実際に ConPTY / psmux を起動する**）。
//!
//! 固定（#655 / #659）は別スレッドで非同期に走り、シェルと同期していない。固定より先に
//! 届いた入力に応じて子がバイト列で書いた出力（`type` 等）は後から直せない形で化ける。
//! ここでは固定を**遅らせる / 諦めさせる注入**（`TAKO_2092_INJECT_PIN_*`）で窓を広げ、
//!
//! - 新: 最初の書き込みが決着まで預かられ、化けない（直接ペイン・器のペイン）
//! - 旧（`TAKO_2092_LEGACY=1`）: 同じ注入で化ける（= この検査に検出力がある）
//! - 決着しないまま上限が来たら流し、出来事が通知される
//! - 諦めたら記録と通知が出る
//! - 保留中にペインを閉じても落ちず、閉じた後に流さない
//!
//! を確かめる。器（psmux）の検査は psmux が無い環境ではスキップする（`TAKO_PSMUX_BIN` /
//! PATH）。器での固定までの窓の実測（`TAKO_2092_MEASURE`）もここに置く。
//!
//! env はプロセス全体の状態なので、**専用のテストバイナリ**に置き、各テストは
//! [`serial`] のロックの内側で env を立てて spawn する（注入は spawn の時点で読まれる）。
//!
//! 固定は「tako 自身がコンソールを持っていないとき」だけ働く（出荷構成 = GUI サブシステム）。
//! `encoding_conpty.rs` と同じく、最初に `FreeConsole` して出荷構成に合わせる。

#![cfg(windows)]

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedReceiver;
use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::encoding_pin::{
    ContainerPin, PinLogEvent, PinTarget, Release, CONTAINER_HOLD_LIMIT, DIRECT_HOLD_LIMIT,
    INJECTED_GIVE_UP_REASON,
};
use tako_core::terminal::{
    SessionEvent, SessionNotice, SpawnCommand, SpawnOptions, TerminalSession,
};

/// psmux を叩く経路は**すべて期限つき**（#1271）
#[path = "common/psmux_ctl.rs"]
mod psmux_ctl;

/// 化けの判定に使う文字列（CP932 にも UTF-8 にもある常用漢字）
const JP: &str = "日本語テスト";

/// UTF-8 の `日本語` が OEM コードページで解釈されたときの並び。
/// CI の英語版 Windows は CP437（`µùÑµ£¼`）、日本語版は CP932（`譌･譛ｬ`）
fn garbled(screen: &str) -> bool {
    screen.contains("µùÑ") || screen.contains("譌･譛ｬ")
}

const ENV_KEYS: &[&str] = &[
    "TAKO_2092_LEGACY",
    "TAKO_2092_INJECT_PIN_DELAY_MS",
    "TAKO_2092_INJECT_PIN_GIVE_UP",
    "TAKO_2092_INJECT_PIN_TARGET",
];

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
}

/// テストを 1 本ずつ通す（env と固定のロックを共有するため）。出荷構成に合わせて
/// 自分のコンソールも手放す
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        FreeConsole();
    });
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for key in ENV_KEYS {
        std::env::remove_var(key);
    }
    guard
}

/// 注入を立てた状態で spawn し、すぐ下ろす（注入は spawn の時点で読まれる）
fn with_env<T>(pairs: &[(&str, &str)], spawn: impl FnOnce() -> T) -> T {
    for (key, value) in pairs {
        std::env::set_var(key, value);
    }
    let got = spawn();
    for key in ENV_KEYS {
        std::env::remove_var(key);
    }
    got
}

fn cmd() -> SpawnCommand {
    SpawnCommand {
        program: "cmd.exe".into(),
        args: vec!["/d".into(), "/q".into(), "/k".into(), "prompt $G".into()],
    }
}

fn fixture(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("tako-2092-{}-{tag}.txt", std::process::id()));
    std::fs::write(&path, format!("{JP}\r\n").as_bytes()).expect("fixture を書ける");
    path
}

struct Pane {
    session: TerminalSession,
    rx: UnboundedReceiver<SessionEvent>,
    born: Instant,
    /// 届いた文字コード固定の出来事（`SessionNotice::EncodingPin`）
    events: Vec<PinLogEvent>,
}

impl Pane {
    fn direct() -> Self {
        Self::spawn(
            SpawnOptions {
                command: Some(cmd()),
                cwd: Some(std::env::temp_dir()),
                env: Vec::new(),
                scrollback_lines: None,
            },
            None,
        )
    }

    fn spawn(options: SpawnOptions, container: Option<ContainerPin>) -> Self {
        let born = Instant::now();
        let (session, rx) =
            TerminalSession::spawn_in(120, 40, options, container).expect("ConPTY を起動できる");
        Self {
            session,
            rx,
            born,
            events: Vec::new(),
        }
    }

    /// 届いている出力を画面へ反映する（**汲むのが必須**: psmux クライアントは
    /// 端末クエリへの応答を待つ）
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Some(SessionNotice::EncodingPin(event)) = self.session.process_event(event) {
                self.events.push(event);
            }
        }
    }

    fn screen(&self) -> String {
        self.session.visible_lines().join("\n")
    }

    fn wait(&mut self, timeout: Duration, mut ok: impl FnMut(&mut Self) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if ok(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn report(&self) -> tako_core::encoding_pin::PinReport {
        self.session
            .encoding_pin()
            .expect("Windows の既定では門がある（legacy 以外）")
    }

    /// `type` の結果（化けた / 化けていない）が画面に出るまで待つ
    fn wait_type_result(&mut self) -> bool {
        self.wait(Duration::from_secs(20), |p| {
            let s = p.screen();
            s.contains(JP) || garbled(&s)
        })
    }
}

/// **受け入れ条件 1（直接ペイン）**: 固定を遅らせても、最初の書き込みが決着まで預かられ
/// 化けない。同じ注入で旧挙動は化ける（検出力の実証）。
///
/// 書き込みは spawn の直後（tako が自動で流す最初の入力 = 起動コマンドの形）。
/// 旧は cmd.exe が起動してすぐ `type` を実行し、遅らせた固定より先に UTF-8 を書く
#[test]
fn 直接ペインの最初の書き込みは固定の決着まで預かられ化けない() {
    let _serial = serial();
    let path = fixture("direct");
    let line = format!("type {}\r\n", path.display());
    // 上限（500ms）より十分短く、cmd.exe の起動（〜100ms）より十分長い遅れ
    let delay = "300";

    for legacy in [false, true] {
        let mut env = vec![
            ("TAKO_2092_INJECT_PIN_DELAY_MS", delay),
            ("TAKO_2092_INJECT_PIN_TARGET", "direct"),
        ];
        if legacy {
            env.push(("TAKO_2092_LEGACY", "1"));
        }
        let mut pane = with_env(&env, Pane::direct);
        pane.session.write(line.clone().into_bytes());
        let written_ms = pane.born.elapsed().as_millis();

        if legacy {
            assert!(
                pane.session.encoding_pin().is_none(),
                "legacy は門を作らない"
            );
        } else {
            let at_write = pane.report();
            assert_eq!(
                at_write.state(),
                "pending",
                "書いた時点で固定が決着していた（注入が効いていない）: {at_write:?}"
            );
            assert_eq!(
                at_write.hold.held_writes, 1,
                "最初の書き込みを預かっていない"
            );
        }

        assert!(
            pane.wait_type_result(),
            "type の結果が出ない（legacy={legacy}）: {}",
            pane.screen()
        );
        let screen = pane.screen();
        eprintln!(
            "[2092] direct legacy={legacy} 書き込み={written_ms}ms 画面:\n{}",
            screen
        );
        if legacy {
            // 旧挙動は化ける。OEM コードページが元から UTF-8 の機では化けようがないので明示する
            if screen.contains(JP) {
                eprintln!("note: legacy でも化けなかった（この機の OEM コードページが UTF-8）");
            } else {
                assert!(garbled(&screen), "旧挙動は化けるはず: {screen}");
            }
        } else {
            assert!(
                screen.contains(JP),
                "新挙動で UTF-8 の日本語が出ない: {screen}"
            );
            assert!(!garbled(&screen), "新挙動で化けた: {screen}");
            let report = pane.report();
            assert_eq!(report.state(), "pinned", "{report:?}");
            assert_eq!(report.hold.release, Some(Release::Settled), "{report:?}");
            let settled = report.targets[0].settled_ms.expect("決着の時刻");
            eprintln!(
                "[2092] direct 決着={settled}ms 保留={:?}ms 上限={}ms",
                report.hold.held_ms,
                DIRECT_HOLD_LIMIT.as_millis()
            );
            assert!(
                pane.events.is_empty(),
                "正常系で出来事が通知された: {:?}",
                pane.events
            );
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// **境界: 固定が上限まで決着しない**。上限（起動から 500ms）で預かりを流し、
/// 出来事（`HoldLimit`）が UI 層へ届く。入力は失われない（`type` の行が届いて実行される）
#[test]
fn 固定が上限まで決着しなければ上限で流して通知する() {
    let _serial = serial();
    let path = fixture("limit");
    let mut pane = with_env(
        &[
            ("TAKO_2092_INJECT_PIN_DELAY_MS", "3000"),
            ("TAKO_2092_INJECT_PIN_TARGET", "direct"),
        ],
        Pane::direct,
    );
    pane.session
        .write(format!("type {}\r\n", path.display()).into_bytes());

    assert!(
        pane.wait(Duration::from_secs(10), |p| p
            .report()
            .hold
            .release
            .is_some()),
        "上限を過ぎても預かりが解けない: {:?}",
        pane.report()
    );
    let released_ms = pane.born.elapsed().as_millis();
    let report = pane.report();
    assert_eq!(report.hold.release, Some(Release::Limit), "{report:?}");
    assert_eq!(
        report.state(),
        "pending",
        "決着より先に上限が来たはず: {report:?}"
    );
    // 解けた時刻は証拠として出すだけ（所要は assert にしない = `.agent/conventions.md`
    // 「効果を測る単体テストは実時間で比べない」）。上限の経路で解けたことは
    // `release=limit`・`state=pending`・`HoldLimit` の出来事の 3 つで見る
    let limit = DIRECT_HOLD_LIMIT.as_millis();
    // 入力は流れた（cmd.exe が `type` を実行し、遅らせた固定より先に書くので化ける）
    assert!(
        pane.wait_type_result(),
        "上限で流した入力が届かない: {}",
        pane.screen()
    );
    assert!(
        pane.wait(Duration::from_secs(5), |p| p
            .events
            .iter()
            .any(|e| matches!(e, PinLogEvent::HoldLimit { .. }))),
        "上限で流したことが通知されない: {:?}",
        pane.events
    );
    let PinLogEvent::HoldLimit {
        limit_ms,
        held_writes,
        pending,
    } = pane.events[0].clone()
    else {
        panic!("最初の出来事が上限ではない: {:?}", pane.events);
    };
    assert_eq!(limit_ms as u128, limit);
    assert_eq!(held_writes, 1);
    assert_eq!(pending, vec![PinTarget::Direct]);
    eprintln!(
        "[2092] 上限で解けた={released_ms}ms 記録: {}",
        pane.events[0].log_line(1)
    );
    // 固定は遅れて決着する（上限の後でも記録は更新される）
    assert!(
        pane.wait(Duration::from_secs(10), |p| p.report().state() == "pinned"),
        "遅れた固定が決着しない: {:?}",
        pane.report()
    );
    let _ = std::fs::remove_file(&path);
}

/// **受け入れ条件 2 の前半（直接ペイン）**: 注入で諦めさせると、記録（診断の材料）と
/// 出来事（persist.log の材料）の両方に出る。諦めも決着なので預かりはすぐ流れる
#[test]
fn 固定を諦めたら記録と通知に出て預かりは流れる() {
    let _serial = serial();
    let mut pane = with_env(
        &[
            ("TAKO_2092_INJECT_PIN_GIVE_UP", "1"),
            ("TAKO_2092_INJECT_PIN_TARGET", "direct"),
        ],
        Pane::direct,
    );
    pane.session.write(b"echo TAKO-2092-GAVE-UP\r\n".to_vec());
    assert!(
        pane.wait(Duration::from_secs(10), |p| p
            .events
            .iter()
            .any(|e| matches!(e, PinLogEvent::GaveUp { .. }))),
        "諦めが通知されない: {:?}",
        pane.events
    );
    let report = pane.report();
    assert_eq!(report.state(), "gave_up", "{report:?}");
    assert_eq!(report.hold.release, Some(Release::Settled), "{report:?}");
    assert_eq!(
        report.targets[0].state.reason(),
        Some(INJECTED_GIVE_UP_REASON)
    );
    let line = pane.events[0].log_line(9);
    assert!(
        line.contains("pane=9") && line.contains("対象=direct"),
        "{line}"
    );
    assert!(
        !line.contains("TAKO-2092-GAVE-UP"),
        "送った文字が記録に載った: {line}"
    );
    // 預かっていた入力は届く（ASCII なので化けずに出る）
    assert!(
        pane.wait(Duration::from_secs(15), |p| p
            .screen()
            .lines()
            .any(|l| l.trim() == "TAKO-2092-GAVE-UP")),
        "諦めた後に預かりが流れない: {}",
        pane.screen()
    );
}

/// **境界: 保留中にペインが閉じられる**。預かったまま端末を捨てても落ちず、
/// 遅れて決着した固定も閉じた門へは流さない（ダイアログ応答の口 = `PaneAccess` も同じ門）
#[test]
fn 保留中にペインを閉じても落ちず閉じた後に流さない() {
    let _serial = serial();
    let pane = with_env(
        &[
            ("TAKO_2092_INJECT_PIN_DELAY_MS", "1500"),
            ("TAKO_2092_INJECT_PIN_TARGET", "direct"),
        ],
        Pane::direct,
    );
    let access = pane.session.access();
    pane.session.write(b"echo should-not-run\r\n".to_vec());
    assert_eq!(pane.report().hold.held_writes, 1);
    drop(pane);

    let report = access.encoding_pin().expect("門は PaneAccess からも読める");
    assert_eq!(report.hold.release, Some(Release::Closed), "{report:?}");
    // 閉じた後の入力は捨てる（流す先がもう無い）
    access.write(b"after-close\r\n".to_vec());
    assert_eq!(access.encoding_pin().unwrap().hold.held_writes, 1);
    // 上限・遅れた固定の決着を跨いでも落ちない（見張りと固定のスレッドが閉じた門を扱える）
    std::thread::sleep(Duration::from_millis(2500));
    let after = access.encoding_pin().unwrap();
    assert_eq!(after.hold.release, Some(Release::Closed), "{after:?}");
}

// --- 器（psmux）のペイン -----------------------------------------------------

fn psmux_bin() -> Option<String> {
    if let Some(bin) = std::env::var("TAKO_PSMUX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return psmux_ctl::probe(&bin).then_some(bin);
    }
    psmux_ctl::probe("psmux").then(|| "psmux".to_string())
}

/// 隔離ソケット上の器（`&'static` が要るので backend は leak する。テスト 1 本に 1 つ）
struct Vessel {
    backend: &'static PsmuxBackend,
    bin: String,
    socket: String,
}

impl Vessel {
    fn new(tag: &str) -> Option<Self> {
        let bin = psmux_bin()?;
        let socket = format!("tako-2092-{}-{tag}", std::process::id());
        let owners =
            std::env::temp_dir().join(format!("tako-2092-owners-{}-{tag}", std::process::id()));
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

    /// 器の中で cmd.exe を起こすペイン（器の内側の固定も同じ門へ載せる = 本番の GUI と同じ）
    fn pane(&self, name: &str) -> Pane {
        let session = SessionRef::new(name).expect("器の名前");
        let options = self.backend.wrap_spawn(
            SpawnOptions {
                command: Some(cmd()),
                cwd: Some(std::env::temp_dir()),
                env: Vec::new(),
                scrollback_lines: None,
            },
            &session,
        );
        Pane::spawn(options, Some(ContainerPin::new(self.backend, session)))
    }
}

impl Drop for Vessel {
    fn drop(&mut self) {
        psmux_ctl::kill_server(&self.bin, &self.socket);
    }
}

macro_rules! vessel {
    ($tag:expr) => {
        match Vessel::new($tag) {
            Some(v) => v,
            None => {
                eprintln!("skip: psmux が無い環境（TAKO_PSMUX_BIN / PATH）");
                return;
            }
        }
    };
}

/// 器の中の cmd.exe のプロンプト（`>` だけの行）が出たか
fn inner_prompt(screen: &str) -> bool {
    screen.lines().any(|l| l.trim() == ">")
}

/// **受け入れ条件 1（器のペイン）**: 器の中のシェルの固定を遅らせても、入力は器の内側の
/// 決着まで預かられ化けない。同じ注入で旧挙動は化ける。
///
/// 器は中のシェルが起動しきる前の打鍵を食うことがある（`psmux_backend.rs` の実測）ので、
/// 書き込みは**器の中のプロンプトが出てから**にする（tako の送信の既定 = プロンプト待ちと同じ）。
/// 遅らせた固定はそれより後に当たるので、旧は `type` が先に走って化ける
#[test]
fn 器のペインの入力は器の内側の固定の決着まで預かられ化けない() {
    let _serial = serial();
    let path = fixture("vessel");
    let line = format!("type {}\r\n", path.display());

    for legacy in [false, true] {
        let vessel = vessel!(if legacy { "vlegacy" } else { "vnew" });
        let name = format!("tako-2092-{}", if legacy { "old" } else { "new" });
        let mut env = vec![
            ("TAKO_2092_INJECT_PIN_DELAY_MS", "600"),
            ("TAKO_2092_INJECT_PIN_TARGET", "container"),
        ];
        if legacy {
            env.push(("TAKO_2092_LEGACY", "1"));
        }
        let mut pane = with_env(&env, || vessel.pane(&name));
        assert!(
            pane.wait(Duration::from_secs(30), |p| inner_prompt(&p.screen())),
            "器の中のプロンプトが出ない（legacy={legacy}）: {}",
            pane.screen()
        );
        let prompt_ms = pane.born.elapsed().as_millis();
        if !legacy {
            let at_write = pane.report();
            assert_ne!(
                at_write.state(),
                "pinned",
                "プロンプトの時点で器の内側が固定済み（注入が効いていない）: {at_write:?}"
            );
        }
        pane.session.write(line.clone().into_bytes());
        assert!(
            pane.wait_type_result(),
            "type の結果が出ない（legacy={legacy}）: {}",
            pane.screen()
        );
        let screen = pane.screen();
        eprintln!(
            "[2092] vessel legacy={legacy} プロンプト={prompt_ms}ms 画面:\n{}",
            screen
        );
        if legacy {
            if screen.contains(JP) {
                eprintln!("note: legacy でも化けなかった（この機の OEM コードページが UTF-8）");
            } else {
                assert!(garbled(&screen), "旧挙動は化けるはず: {screen}");
            }
        } else {
            assert!(
                screen.contains(JP),
                "新挙動で UTF-8 の日本語が出ない: {screen}"
            );
            assert!(!garbled(&screen), "新挙動で化けた: {screen}");
            let report = pane.report();
            assert_eq!(report.state(), "pinned", "{report:?}");
            assert_eq!(report.hold.release, Some(Release::Settled), "{report:?}");
            let inner = report
                .targets
                .iter()
                .find(|t| t.target == PinTarget::Container)
                .expect("器の内側の対象");
            assert!(
                inner.pid.is_some(),
                "器の中の pid が記録されていない: {report:?}"
            );
            eprintln!(
                "[2092] vessel 器の内側の決着={:?}ms 保留={:?}ms 上限={}ms",
                inner.settled_ms,
                report.hold.held_ms,
                CONTAINER_HOLD_LIMIT.as_millis()
            );
        }
        drop(pane);
        let _ = vessel
            .backend
            .kill(&SessionRef::new(name.as_str()).unwrap());
    }
    let _ = std::fs::remove_file(&path);
}

/// **受け入れ条件 2 の後半（器のペイン）**: 器の内側を諦めさせると、器の内側の対象が
/// `gave_up` になり出来事が通知される
#[test]
fn 器の内側の固定を諦めたら記録と通知に出る() {
    let _serial = serial();
    let vessel = vessel!("vgiveup");
    let mut pane = with_env(
        &[
            ("TAKO_2092_INJECT_PIN_GIVE_UP", "1"),
            ("TAKO_2092_INJECT_PIN_TARGET", "container"),
        ],
        || vessel.pane("tako-2092-giveup"),
    );
    assert!(
        pane.wait(Duration::from_secs(30), |p| p.events.iter().any(
            |e| matches!(
                e,
                PinLogEvent::GaveUp {
                    target: PinTarget::Container,
                    ..
                }
            )
        )),
        "器の内側の諦めが通知されない: {:?} / {:?}",
        pane.events,
        pane.session.encoding_pin()
    );
    let report = pane.report();
    assert_eq!(report.state(), "gave_up", "{report:?}");
    let line = pane.events[0].log_line(4);
    assert!(line.contains("対象=container"), "{line}");
    eprintln!("[2092] vessel 諦めの記録: {line}");
    drop(pane);
    let _ = vessel
        .backend
        .kill(&SessionRef::new("tako-2092-giveup").unwrap());
}

/// **受け入れ条件 3**: 器での固定までの窓の実測（注入なし）。
///
/// 器のペインの起動から「器の中のプロンプトが画面に出る」「器の内側の固定が決着する」
/// までを測り、上限（[`CONTAINER_HOLD_LIMIT`]）に収まることを確かめる。直接ペインの
/// 同じ値（#1696 で測った「プロンプトの何 ms 前に固定が済んだか」）も並べて出す。
/// 回数は `TAKO_2092_MEASURE=<n>`（既定 3）
#[test]
fn 固定までの窓を測る() {
    let _serial = serial();
    let rounds: usize = std::env::var("TAKO_2092_MEASURE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    for round in 0..rounds {
        let mut pane = Pane::direct();
        assert!(pane.wait(Duration::from_secs(15), |p| inner_prompt(&p.screen())));
        let prompt = pane.born.elapsed().as_millis();
        assert!(pane.wait(Duration::from_secs(10), |p| p.report().state() != "pending"));
        let settled = pane.report().targets[0].settled_ms.unwrap();
        eprintln!(
            "TAKO_2092_WINDOW direct round={round} prompt={prompt}ms pinned={settled}ms \
             margin={}ms",
            prompt as i64 - settled as i64
        );
    }

    let Some(vessel) = Vessel::new("vmeasure") else {
        eprintln!("skip: psmux が無い環境（器の窓は測れない）");
        return;
    };
    let mut worst: u64 = 0;
    for round in 0..rounds {
        let name = format!("tako-2092-m{round}");
        let mut pane = vessel.pane(&name);
        assert!(
            pane.wait(Duration::from_secs(30), |p| inner_prompt(&p.screen())),
            "器の中のプロンプトが出ない: {}",
            pane.screen()
        );
        let prompt = pane.born.elapsed().as_millis();
        assert!(
            pane.wait(Duration::from_secs(20), |p| p.report().state() != "pending"),
            "器の固定が決着しない: {:?}",
            pane.report()
        );
        let report = pane.report();
        let at = |target| {
            report
                .targets
                .iter()
                .find(|t| t.target == target)
                .and_then(|t| t.settled_ms)
                .unwrap_or(0)
        };
        let (direct, inner) = (at(PinTarget::Direct), at(PinTarget::Container));
        worst = worst.max(inner);
        eprintln!(
            "TAKO_2092_WINDOW vessel round={round} prompt={prompt}ms client_pinned={direct}ms \
             inner_pinned={inner}ms inner_after_prompt={}ms state={}",
            inner as i64 - prompt as i64,
            report.state()
        );
        assert_eq!(report.state(), "pinned", "{report:?}");
        drop(pane);
        let _ = vessel
            .backend
            .kill(&SessionRef::new(name.as_str()).unwrap());
    }
    eprintln!(
        "TAKO_2092_WINDOW vessel worst_inner_pinned={worst}ms limit={}ms",
        CONTAINER_HOLD_LIMIT.as_millis()
    );
}

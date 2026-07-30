//! 起動コマンドの送達を**実シェル・実バックエンド**で確かめる（#640）
//!
//! `shell_send` の単体テストは「画面がこう見えたらこう動く」を固定する。
//! こちらは **本当に届くのか**を器ごと通して測る。#640 の症状（新規ペインへ
//! 書きっぱなしにすると全損・途中欠落する）が、旧経路で再現し新経路で消えることを
//! 同じプロセス・同じ条件で見せるのが目的。
//!
//! 既定では走らない。実測は
//! `cargo test -p tako-core --test shell_send_e2e -- --ignored --nocapture`。
//! psmux が無い環境（macOS / CI）はスキップする。

use std::time::{Duration, Instant};

use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::shell_send::{ShellSendAction, ShellSendFlow, TICK_MS};
use tako_core::terminal::{SpawnOptions, TerminalSession};

/// 実行できたときだけ画面に出る目印。コマンド側は連結して書くのでエコーには出ない
const MARK: &str = "TAKOMARK640";

fn marker_command() -> String {
    if cfg!(windows) {
        "Write-Output (\"TAKOMARK\" + \"640\")".to_string()
    } else {
        "echo \"TAKOMARK\"\"640\"".to_string()
    }
}

fn psmux_bin() -> Option<String> {
    let bin = std::env::var("TAKO_PSMUX_BIN").unwrap_or_else(|_| "psmux".into());
    std::process::Command::new(&bin)
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        .then_some(bin)
}

/// 1 回ぶんの試行。Drop でシェルも器のセッションも片付ける
/// （後始末を怠ると試行のたびに PowerShell が積み上がり、後半の試行だけ
/// 極端に遅くなって比較にならない）
struct Trial {
    session: TerminalSession,
    bin: String,
    socket: String,
    name: String,
}

impl Drop for Trial {
    fn drop(&mut self) {
        let _ = std::process::Command::new(&self.bin)
            .args(["-L", &self.socket, "kill-session", "-t", &self.name])
            .output();
    }
}

/// 本番と同じ経路（`wrap_spawn`）で器の中にシェルを立てる
fn spawn_wrapped(bin: &str, socket: &str, tag: &str) -> TerminalSession {
    // このテスト自身が器の中で走ると入れ子ガードに弾かれる。本番の tako-app は器の外
    unsafe {
        std::env::remove_var("PSMUX_SESSION");
        std::env::remove_var("TMUX");
    }
    let backend = PsmuxBackend::with_parts(
        bin.to_string(),
        "3.3.7".into(),
        socket.to_string(),
        std::env::temp_dir().join(format!("tako-e640-own-{}", std::process::id())),
    );
    let options = backend.wrap_spawn(
        SpawnOptions {
            command: None,
            cwd: None,
            env: Vec::new(),
        },
        &SessionRef::new(format!("tako-e640-{tag}-{}", std::process::id())).unwrap(),
    );
    let (session, rx) = TerminalSession::spawn(120, 40, options).expect("PTY を起こせる");
    // イベントは読み捨てる（グリッドは IO スレッドが直接更新するので観測には要らない）。
    // レシーバを落とすと送信側がエラーになるだけなので、試行中は生かしておく
    std::thread::spawn(move || {
        let mut rx = rx;
        while futures::executor::block_on(futures::StreamExt::next(&mut rx)).is_some() {}
    });
    session
}

fn start_trial(bin: &str, socket: &str, tag: &str) -> Trial {
    let name = format!("tako-e640-{tag}-{}", std::process::id());
    Trial {
        session: spawn_wrapped(bin, socket, tag),
        bin: bin.to_string(),
        socket: socket.to_string(),
        name,
    }
}

fn ran(session: &TerminalSession) -> bool {
    session
        .visible_lines()
        .iter()
        .any(|l| l.contains(MARK) && !l.contains('"'))
}

fn wait_ran(session: &TerminalSession, limit: Duration) -> Option<u128> {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        if ran(session) {
            return Some(t0.elapsed().as_millis());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn kill(bin: &str, socket: &str) {
    let _ = std::process::Command::new(bin)
        .args(["-L", socket, "kill-server"])
        .output();
}

/// 旧経路: PTY 起動直後に「本文 + Enter」を 1 回書いて、あとは放置する
fn trial_old(bin: &str, socket: &str, tag: &str) -> bool {
    let trial = start_trial(bin, socket, tag);
    let mut bytes = marker_command().into_bytes();
    bytes.push(b'\r');
    trial.session.write(bytes);
    wait_ran(&trial.session, Duration::from_secs(30)).is_some()
}

/// 新経路: `ShellSendFlow` を 500ms tick で回す（tako-app の `drive_command_flows` と同じ）
fn trial_new(bin: &str, socket: &str, tag: &str) -> bool {
    let trial = start_trial(bin, socket, tag);
    let session = &trial.session;
    let mut flow = ShellSendFlow::new(marker_command());
    let t0 = Instant::now();
    let verbose = std::env::var_os("TAKO_E2E_VERBOSE").is_some();
    // 本番（tako-app）はフローを 120 秒まで回す。ここで短く切ると
    // 「製品の挙動」ではなく「ハーネスの上限」を測ることになる（実際、並行ビルドで
    // machine を潰した状態で 60 秒に切ったら未達が多発した）
    while t0.elapsed() < Duration::from_secs(120) {
        let lines = session.visible_lines();
        let action = flow.tick(&lines);
        if verbose {
            println!(
                "[{tag}] {}ms {} action={} 画面={:?}",
                t0.elapsed().as_millis(),
                flow.stage_name(),
                match &action {
                    ShellSendAction::Wait => "wait".to_string(),
                    ShellSendAction::Write(b) => format!("write({})", b.len()),
                    ShellSendAction::Done { verified } => format!("done({verified})"),
                },
                lines
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
            );
        }
        match action {
            ShellSendAction::Wait => {}
            ShellSendAction::Write(bytes) => session.write(bytes),
            ShellSendAction::Done { .. } => break,
        }
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
    // フロー完了後にコマンドが実際に走ったか（実行の観測は画面が正）
    ran(session) || wait_ran(session, Duration::from_secs(20)).is_some()
}

/// 旧経路と新経路を同じ条件で N 回ずつ回して到達率を比べる
#[test]
#[ignore = "psmux と実シェルを起動する実測用"]
fn 実測_起動コマンドの到達率_旧経路と新経路() {
    let Some(bin) = psmux_bin() else {
        eprintln!("psmux が無いのでスキップ");
        return;
    };
    let rounds = std::env::var("TAKO_E2E_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let socket = format!("tako-e640-{}", std::process::id());

    // 旧経路の再現は時間がかかる（1 回あたり 30 秒の待ち）ので、新経路だけ見たいときは飛ばせる
    let skip_old = std::env::var_os("TAKO_E2E_SKIP_OLD").is_some();
    let mut old_ok = 0;
    for i in 0..rounds {
        if skip_old {
            break;
        }
        if trial_old(&bin, &socket, &format!("old{i}")) {
            old_ok += 1;
        }
    }
    let mut new_ok = 0;
    for i in 0..rounds {
        let ok = trial_new(&bin, &socket, &format!("new{i}"));
        println!("[new{i}] {}", if ok { "到達" } else { "未達" });
        if ok {
            new_ok += 1;
        }
    }
    kill(&bin, &socket);
    println!("旧経路（書きっぱなし）: {old_ok}/{rounds} 到達");
    println!("新経路（送達確認つき）: {new_ok}/{rounds} 到達");
    assert_eq!(
        new_ok, rounds,
        "送達確認つきなら全部届く（旧経路は {old_ok}/{rounds}）"
    );
}

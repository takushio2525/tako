//! ペインの PTY の子へは GUI が起動したときの fd 上限（`RLIMIT_NOFILE` の soft）を渡すことを
//! **実 PTY で**測る（#2359）
//!
//! GUI は起動時に soft を引き上げる（`fd_limit::raise_at_startup`）が、ペインのシェル・
//! エージェントの `ulimit -n` は修正前と同じに保つ（Ghostty #4232 と同じ判断）。戻すのは
//! fork の子ハンドラ（`fd_inherit::spawn_sealed` が構える）で、ここではテストプロセスで
//! 「起動時の値」を構え（`arm_children`）、**ペインと同じ経路**（`TerminalSession::spawn`）で
//! 起こした `/bin/sh` の `ulimit -Sn` を読む。tako が自分で起こす裏方（std の `Command`）は
//! 引き上げた値のまま受け継ぐことも確かめる。
//!
//! **このバイナリのテストは直列に回す**（`LOCK`）。構えはプロセス全体の 1 つの値なので、
//! 並走すると他のテストの子の上限まで変わる。
#![cfg(target_os = "macos")]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tako_core::platform::fd_limit::arm_children;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

static LOCK: Mutex<()> = Mutex::new(());

/// 1 回の観測の上限（状態待ちなので空いていれば即返る）
const LIMIT: Duration = Duration::from_secs(30);

/// 子へ戻す「起動時の値」（Finder 起動の 256 より小さく、テストプロセスの soft より小さい値）
const LAUNCH_SOFT: u64 = 200;

const PROBE: &str = "echo \"NOFILE=[$(ulimit -Sn)]\"";

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 構えを必ず外す（テストが落ちても後に残さない）
struct Disarm;
impl Drop for Disarm {
    fn drop(&mut self) {
        arm_children(0);
    }
}

/// テストプロセスの soft を `LAUNCH_SOFT` より上にしておき、いまの soft を返す
fn ensure_raised() -> u64 {
    let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
    if lim.rlim_cur <= LAUNCH_SOFT + 100 && lim.rlim_max > lim.rlim_cur {
        lim.rlim_cur = lim.rlim_max.min(1024);
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
    }
    assert!(
        lim.rlim_cur > LAUNCH_SOFT,
        "前提: テストプロセスの soft（{}）が {LAUNCH_SOFT} より大きい",
        lim.rlim_cur
    );
    lim.rlim_cur
}

fn parse(screen: &str) -> Option<u64> {
    screen.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("NOFILE=[")?;
        rest.strip_suffix(']')?.parse().ok()
    })
}

/// ペインと同じ経路で `/bin/sh` を起こし、子の `ulimit -Sn` を返す
fn soft_in_pane() -> u64 {
    let (session, rx) = TerminalSession::spawn(
        80,
        12,
        SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), PROBE.to_string()],
            }),
            ..SpawnOptions::default()
        },
    )
    .expect("PTY を起こせる");
    std::thread::spawn(move || {
        let mut rx = rx;
        while futures::executor::block_on(futures::StreamExt::next(&mut rx)).is_some() {}
    });
    let started = Instant::now();
    loop {
        let screen = session.visible_lines().join("\n");
        if let Some(soft) = parse(&screen) {
            return soft;
        }
        assert!(
            started.elapsed() < LIMIT,
            "子の ulimit が画面に出ない（waited={:.1}s 画面={screen:?}）",
            started.elapsed().as_secs_f32()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// tako が自分で起こす裏方と同じ起こし方（std の `Command`）で子の `ulimit -Sn` を返す
fn soft_in_helper() -> u64 {
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(PROBE)
        .output()
        .expect("/bin/sh を起こせる");
    parse(&String::from_utf8_lossy(&out.stdout)).expect("NOFILE= の行")
}

#[test]
fn ペインのptyの子は起動時のsoftを受け継ぐ() {
    let _g = lock();
    let parent = ensure_raised();
    arm_children(LAUNCH_SOFT);
    let _disarm = Disarm;
    let child = soft_in_pane();
    assert_eq!(
        child, LAUNCH_SOFT,
        "ペインのシェルが親の soft {parent} を受け継いだ。fork の子ハンドラ \
         （fd_inherit の on_fork_child）が fd_limit::restore_for_child を呼んでいない（#2359）"
    );
}

#[test]
fn 裏方の子は引き上げた値のまま() {
    let _g = lock();
    let parent = ensure_raised();
    arm_children(LAUNCH_SOFT);
    let _disarm = Disarm;
    assert_eq!(
        soft_in_helper(),
        parent,
        "構えの外の子（言語サーバ・git 等）は親と同じ値"
    );
}

#[test]
fn 構えていなければペインの子も親と同じ値() {
    // 引き上げなかった（A/B・既に足りていた）起動では戻さない
    let _g = lock();
    let parent = ensure_raised();
    arm_children(0);
    assert_eq!(soft_in_pane(), parent);
}

//! ペインの PTY の子が GUI の fd を受け継がないことを**実 PTY で**測る（#1768）
//!
//! GUI の中では Metal が CLOEXEC 無しでシェーダキャッシュを開いたままにしていて、
//! 修正前は全ペインの子の fd 4 / 5 にそれが入っていた（隔離 GUI で 3000 / 3000）。
//! ここではテストプロセスで同じ形の fd（CLOEXEC 無しの通常の fd とパイプの両端）を開き、
//! **ペインと同じ経路**（`TerminalSession::spawn`）で起こした `/bin/sh` の中から
//! それが見えるかを調べる。見えたら、掃除を通すべき箇所（`terminal.rs` の
//! `tty::new`）を file:line で名指して落ちる。
//!
//! 掃除は fork 後・exec 前の**子の中で**走る（`fd_inherit::spawn_sealed`）。親で掃く案だと
//! 「掃いてから fork までに開いた fd」が残るので、その形も 1 本で見分ける
//! （`ptyを起こすあいだに開いたfdも子へ渡らない`）。
//!
//! **このバイナリのテストは直列に回す**（`LOCK`）。掃除の本体を親で直接呼ぶ 2 本
//! （本数と総当たりの検査）はテストプロセスの fd へ CLOEXEC を立てるので、並走すると
//! 「開いた直後は CLOEXEC が無い」の前提が崩れる。同じ理由で、このバイナリに
//! ロックを取らずに掃除を走らせるテストを足さないこと。
#![cfg(unix)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tako_core::platform::fd_inherit::{
    inherited_probe_script, observed_inherited, seal_by_scan, seal_inherited_fds, spawn_sealed,
};
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

static LOCK: Mutex<()> = Mutex::new(());

/// 1 回の観測の上限。`/bin/sh -c` が 1 行出すだけなので実測は数十 ms。
/// 混んだ機でも足りるように広く採る（状態待ちなので空いていれば即返る）
const LIMIT: Duration = Duration::from_secs(30);

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 掃除を通すべき箇所 = ペインの PTY を起こす `tty::new` の行（名指し用）
fn spawn_site() -> String {
    let src = include_str!("../src/terminal.rs");
    let line = src
        .lines()
        .position(|l| l.contains("tty::new(") && !l.trim_start().starts_with("//"))
        .map_or_else(|| "?".to_string(), |i| (i + 1).to_string());
    format!("crates/tako-core/src/terminal.rs:{line}")
}

fn cloexec_of(fd: i32) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags >= 0, "fd {fd} が開いている");
    flags & libc::FD_CLOEXEC != 0
}

/// CLOEXEC 無しの fd を 3 本開く（通常の fd 1 本 + パイプの両端）。
/// GUI の中で Metal が開くキャッシュと、並行 spawn の `pipe()` → `set_cloexec` の
/// あいだのパイプと同じ状態
fn open_leaky_fds() -> Vec<i32> {
    let file = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
    assert!(file >= 3, "/dev/null を開ける: {file}");
    let mut pipe = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0, "pipe を張れる");
    let fds = vec![file, pipe[0], pipe[1]];
    for &fd in &fds {
        assert!(
            !cloexec_of(fd),
            "前提: fd {fd} は開いた直後に CLOEXEC を持たない（LOCK で直列にしてある）"
        );
    }
    fds
}

fn close_all(fds: &[i32]) {
    for &fd in fds {
        unsafe { libc::close(fd) };
    }
}

/// ペインと同じ経路で `/bin/sh` を起こし、子の中で `fds` のうち開いていたものを返す
fn inherited_in_pane(fds: &[i32]) -> Vec<i32> {
    let (session, rx) = TerminalSession::spawn(
        80,
        12,
        SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), inherited_probe_script(fds)],
            }),
            ..SpawnOptions::default()
        },
    )
    .expect("PTY を起こせる");
    // イベントは読み捨てる（グリッドは IO スレッドが直接更新する）
    std::thread::spawn(move || {
        let mut rx = rx;
        while futures::executor::block_on(futures::StreamExt::next(&mut rx)).is_some() {}
    });
    // **固定待ちにしない**（`conventions.md`「セルフテストの待ち条件の書き方」）
    let started = Instant::now();
    loop {
        let screen = session.visible_lines().join("\n");
        if let Some(seen) = observed_inherited(&screen) {
            return seen;
        }
        assert!(
            started.elapsed() < LIMIT,
            "子の検査結果が画面に出ない（waited={:.1}s 画面={screen:?}）",
            started.elapsed().as_secs_f32()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn ペインのptyの子はcloexec無しで開いたfdを受け継がない() {
    let _g = lock();
    let fds = open_leaky_fds();
    let seen = inherited_in_pane(&fds);
    close_all(&fds);
    assert!(
        seen.is_empty(),
        "{}: ペインの PTY の子が CLOEXEC 無しの fd {seen:?} を受け継いだ（開いた fd {fds:?}）。\
         tty::new を platform::fd_inherit::spawn_sealed で包んでいない（#1768）",
        spawn_site()
    );
}

#[test]
fn 連続で開いてもptyの子は余計なfdを持たない() {
    // ペインの開閉を重ねる運用（spawn のたびに新しい fd が開く）でも毎回掃けている
    let _g = lock();
    for round in 0..5 {
        let fds = open_leaky_fds();
        let seen = inherited_in_pane(&fds);
        close_all(&fds);
        assert!(
            seen.is_empty(),
            "{}: {round} 回目のペインで fd {seen:?} を受け継いだ（#1768）",
            spawn_site()
        );
    }
}

#[test]
fn fdを4096本より多く開いていてもptyの子は受け継がない() {
    // 列挙の器（4096 件）に入り切らず、総当たりの逃げ道を通る状態をペインの経路で踏む
    const MANY: usize = 4200;
    let _g = lock();
    let mut before: libc::rlimit = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut before) },
        0
    );
    let want = (MANY as libc::rlim_t) + 256;
    if before.rlim_cur < want {
        let raised = libc::rlimit {
            rlim_cur: want.min(before.rlim_max),
            rlim_max: before.rlim_max,
        };
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) };
    }
    let mut now: libc::rlimit = unsafe { std::mem::zeroed() };
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut now) };
    assert!(
        now.rlim_cur >= want,
        "fd 上限を {want} まで上げられない（soft={} hard={}）",
        now.rlim_cur,
        now.rlim_max
    );
    let many: Vec<i32> = (0..MANY)
        .map(|_| unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) })
        .collect();
    assert!(many.iter().all(|&fd| fd >= 3), "{MANY} 本とも開ける");
    let probe = [many[0], many[MANY / 2], many[MANY - 1]];
    let seen = inherited_in_pane(&probe);
    close_all(&many);
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &before) };
    assert!(
        seen.is_empty(),
        "{}: fd を {MANY} 本開いた状態で {seen:?} を受け継いだ（総当たりの逃げ道が効いていない。#1768）",
        spawn_site()
    );
}

/// PTY と同じ起こし方（`pre_exec` を持つ = fork + exec）で `/bin/sh` を起こし、
/// 子の中で `fds` のうち開いていたものを返す
fn inherited_by_fork(fds: &[i32]) -> Option<Vec<i32>> {
    use std::os::unix::process::CommandExt;
    let out = unsafe {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(inherited_probe_script(fds))
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .pre_exec(|| Ok(()))
            .output()
            .expect("/bin/sh を起こせる")
    };
    observed_inherited(&String::from_utf8_lossy(&out.stdout))
}

#[test]
fn 検査の片は受け継いだfdを見つけられる() {
    // 上の 2 本が素通りで緑にならないことの確認: 掃除を通さずに fork + exec すれば
    // 子の中で CLOEXEC 無しの fd が見える（= 検査の片が漏れを検出できる）
    let _g = lock();
    let fds = open_leaky_fds();
    let seen = inherited_by_fork(&fds);
    close_all(&fds);
    assert_eq!(seen, Some(fds.clone()), "掃除を通さなければ全部見える");
}

#[test]
fn ptyを起こすあいだに開いたfdも子へ渡らない() {
    // 並行 spawn の `pipe()` → `set_cloexec` のあいだに fork が当たった形。
    // 親で掃いてから起こす案では、掃いたあとに開いたこの fd が子へ渡る（実測 3000 回中 1 回）
    let _g = lock();
    let (fds, seen) = spawn_sealed(|| {
        let fds = open_leaky_fds();
        let seen = inherited_by_fork(&fds);
        (fds, seen)
    });
    close_all(&fds);
    assert_eq!(
        seen,
        Some(vec![]),
        "{}: 構えの中で開いた fd {fds:?} が子へ渡った。掃除が fork 後の子の中で走っていない\
         （親で掃く形へ戻っていないか。#1768）",
        spawn_site()
    );
}

#[test]
fn 親のfdには触らない() {
    // 掃除は子の fd 表の写しにだけ効く。親の CLOEXEC を書き換えると、親が意図して
    // 子へ渡す fd（今は無いが）の扱いが変わる
    let _g = lock();
    let fds = open_leaky_fds();
    let seen = inherited_in_pane(&fds);
    let parent: Vec<bool> = fds.iter().map(|&fd| cloexec_of(fd)).collect();
    close_all(&fds);
    assert!(seen.is_empty(), "子には渡らない: {seen:?}");
    assert_eq!(parent, vec![false; 3], "親の fd は CLOEXEC 無しのまま残る");
}

#[test]
fn 構えはspawn_sealedのあいだだけ効く() {
    // 子ハンドラはプロセス全体に 1 回登録されるが、構えていない fork には何もしない
    let _g = lock();
    spawn_sealed(|| ());
    let fds = open_leaky_fds();
    let seen = inherited_by_fork(&fds);
    close_all(&fds);
    assert_eq!(
        seen,
        Some(fds.clone()),
        "構えの外の fork は素のまま（全部見える）"
    );
}

#[test]
fn 掃除はcloexec無しのfdへ立てて本数に数える() {
    let _g = lock();
    let fds = open_leaky_fds();
    let sealed = seal_inherited_fds();
    let after: Vec<bool> = fds.iter().map(|&fd| cloexec_of(fd)).collect();
    close_all(&fds);
    assert_eq!(after, vec![true; 3], "開いた 3 本すべてに CLOEXEC が立つ");
    assert!(sealed >= 3, "立てた本数に数える: {sealed}");
}

#[test]
fn 総当たりの逃げ道でも立てる() {
    // fd が 4096 本を超えて列挙の器に入り切らないときの道（ふだんは通らない）
    let _g = lock();
    let fds = open_leaky_fds();
    let sealed = seal_by_scan();
    let after: Vec<bool> = fds.iter().map(|&fd| cloexec_of(fd)).collect();
    close_all(&fds);
    assert_eq!(
        after,
        vec![true; 3],
        "総当たりでも開いた 3 本すべてに CLOEXEC が立つ"
    );
    assert!(sealed >= 3, "立てた本数に数える: {sealed}");
}

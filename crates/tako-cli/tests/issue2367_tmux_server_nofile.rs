//! #2367 の実経路: 器の tmux サーバーへ引き上げた fd の上限を渡し、器のペインのシェルには起動時の値を戻す
//!
//! 直した症状: GUI は起動時に fd の上限（RLIMIT_NOFILE）の soft を引き上げるが（#2359）、ペインの PTY の子へは
//! 起動時の soft を戻すので、attach クライアントが起こす器の tmux サーバーも起動時の値（Finder 起動なら 256）で
//! 動いていた。サーバーは器の 1 ペインで fd を 3 本使い、上限に届くと新しいペインを
//! `create window failed: fork failed: Too many open files` で作れない（tmux 3.6b は自分の上限を変えない）。
//!
//! このテストのプロセスが「引き上げた GUI」を演じる: 起動時の値を [`LAUNCH_SOFT`] として構え
//! （`arm_children`）、器の中のペインの子の段に CLI の実行ファイル（main の 2 文目で GUI と同じ段が動く）を置き、
//! 製品と同じ経路（`TerminalSession::spawn(wrap_options(..))`）で器を起こす。起動時の値を小さく構えるのが
//! 枯渇の注入（負荷は作らない。足すのは `sleep` の window だけ）。
//!
//! 1. 修正後: サーバーの soft はこのプロセスの soft（引き上げた値）・器のペインのシェルは起動時の値
//!    （明示コマンドも既定のログインシェルも。ログインシェルは `$0` が `-sh`・`$SHELL` も変わらない）・
//!    起動時の値を超える window を足しても新しいペインが作れる
//! 2. A/B（`TAKO_2367_LEGACY=1`）: サーバーも起動時の値で起き、window を足すと上限に届いて
//!    新しいペインが `fork failed: Too many open files` で作れない（修正前の症状）
//!
//! env（A/B）と構え（`arm_children`）はプロセス全体に効くので、このバイナリのテストはこの 1 本だけにする。
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_core::platform::fd_limit;
use tako_core::terminal::{SpawnCommand, SpawnOptions};
use tako_core::tmux_backend::{self, ServerNofile};
use tako_core::TerminalSession;

/// 起動時の soft（Finder 起動の 256 の代わりに小さくして、少ない window で上限に届かせる）
const LAUNCH_SOFT: u64 = 48;

/// 修正後に足す window の数（起動時の値のサーバーなら確実に上限を越える数）
const WINDOWS_OVER_LAUNCH: usize = 60;

/// 器を後片付けする（ソケットごと。自分の器だけ）
struct ServerGuard(String);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        tmux_backend::kill_server(&self.0);
    }
}

/// 一時の HOME（このテストの器のシェルは利用者の rc を読まない）
struct TempHome(PathBuf);

impl TempHome {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tako-clitest-2367-{}", std::process::id()));
        assert!(
            dir.starts_with(std::env::temp_dir()) && dir != std::env::temp_dir(),
            "一時 dir の中だけを使う: {}",
            dir.display()
        );
        std::fs::create_dir_all(&dir).expect("一時の HOME を作れる");
        Self(dir)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        // 一時 dir の中だけを消す（new が確かめた場所。ここでは panic しない = 巻き戻し中でも止まらない）
        if self.0.starts_with(std::env::temp_dir()) && self.0 != std::env::temp_dir() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn options(home: &Path, command: Option<SpawnCommand>) -> SpawnOptions {
    SpawnOptions {
        command,
        cwd: Some(home.to_path_buf()),
        // サーバーはこの env の attach クライアントが起こす = default-shell は /bin/sh（rc を読まない）
        env: vec![
            ("SHELL".to_string(), "/bin/sh".to_string()),
            ("HOME".to_string(), home.display().to_string()),
        ],
        scrollback_lines: None,
    }
}

/// 器のペインの子が自分の上限を画面へ出して眠るコマンド
fn probe_command() -> SpawnCommand {
    SpawnCommand {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "echo \"NOFILE=[$(ulimit -Sn)/$(ulimit -Hn)]\"; exec sleep 600".to_string(),
        ],
    }
}

fn screen(session: &TerminalSession) -> String {
    session.visible_lines().join("\n")
}

fn wait_for(session: &TerminalSession, needle: &str) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if screen(session).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_server(socket: &str) -> tmux_backend::ServerLimits {
    let start = Instant::now();
    loop {
        match tmux_backend::server_nofile(socket) {
            ServerNofile::Running(limits) => return limits,
            other if start.elapsed() > Duration::from_secs(15) => {
                panic!("器のサーバーが読めない: {other:?}")
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// detached の window（`sleep`）を `max` 本まで足す。足せた本数と、失敗したときの字面
fn add_windows(socket: &str, max: usize) -> (usize, Option<String>) {
    for added in 0..max {
        let out = tako_core::tmux::tmux_command(Some(socket))
            .args(["new-window", "-d", "sleep 600"])
            .output()
            .expect("tmux new-window を起こせる");
        if !out.status.success() {
            return (
                added,
                Some(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            );
        }
    }
    (max, None)
}

#[test]
fn 器のサーバーへ引き上げた値を渡し器のシェルには起動時の値を戻す() {
    if !tako_core::tmux_test_gate::container() {
        return;
    }
    let own = fd_limit::usage().expect("macOS では読める");
    assert!(
        own.soft >= LAUNCH_SOFT * 4,
        "このプロセスの soft が小さすぎて「引き上げた GUI」を演じられない: {own:?}"
    );
    let own_hard = own
        .hard
        .map_or_else(|| "unlimited".to_string(), |h| h.to_string());
    let home = TempHome::new();
    fd_limit::arm_children(LAUNCH_SOFT);
    fd_limit::register_container_exec(PathBuf::from(env!("CARGO_BIN_EXE_tako")));

    // ── 1. 修正後 ──────────────────────────────────────────────────────────────
    let socket = format!("tako-clitest-{}-nofile2367", std::process::id());
    let _server = ServerGuard(socket.clone());
    let (pane1, _rx1) = TerminalSession::spawn(
        80,
        24,
        tmux_backend::wrap_options(options(&home.0, Some(probe_command())), &socket, "tako-n1"),
    )
    .expect("器の attach クライアントを起こせる");
    let want = format!("NOFILE=[{LAUNCH_SOFT}/{own_hard}]");
    assert!(
        wait_for(&pane1, &want),
        "器の明示コマンドの子は起動時の soft・hard はそのまま（{want}）。画面: {}",
        screen(&pane1)
    );
    let limits = wait_server(&socket);
    assert_eq!(
        limits.soft,
        Some(own.soft),
        "器のサーバーは引き上げた値（このプロセスの soft）を受け継ぐ: {limits:?}"
    );

    // 既定のログインシェル（コマンド無し）も段を通って起動時の値・ログイン起動・SHELL は変わらない
    let (pane2, _rx2) = TerminalSession::spawn(
        80,
        24,
        tmux_backend::wrap_options(options(&home.0, None), &socket, "tako-n2"),
    )
    .expect("器のログインシェルを起こせる");
    let login_want = format!("SH=[login/{LAUNCH_SOFT}/{own_hard}//bin/sh]");
    let start = Instant::now();
    while !screen(&pane2).contains(&login_want) && start.elapsed() < Duration::from_secs(15) {
        // シェルの起動前に食われることがあるので、出るまで 1 秒ごとに打ち直す
        pane2.write(
            b"case $0 in -*) l=login;; *) l=nologin;; esac; echo \"SH=[$l/$(ulimit -Sn)/$(ulimit -Hn)/$SHELL]\"\r"
                .to_vec(),
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    assert!(
        screen(&pane2).contains(&login_want),
        "ログインシェルは -sh の名で起き、soft は起動時の値・SHELL はそのまま（{login_want}）。画面: {}",
        screen(&pane2)
    );

    // 同じ注入（起動時の値 48）を越える window を足しても、新しいペインが作れる
    let (added, err) = add_windows(&socket, WINDOWS_OVER_LAUNCH);
    assert_eq!(
        (added, err.as_deref()),
        (WINDOWS_OVER_LAUNCH, None),
        "引き上げた器のサーバーは起動時の値を越えても window を作れる"
    );
    let (pane3, _rx3) = TerminalSession::spawn(
        80,
        24,
        tmux_backend::wrap_options(options(&home.0, Some(probe_command())), &socket, "tako-n3"),
    )
    .expect("器の attach クライアントを起こせる");
    assert!(
        wait_for(&pane3, &want),
        "起動時の値を越えた器にも新しいペインが作れ、その子は起動時の値（{want}）。画面: {}",
        screen(&pane3)
    );
    drop((pane1, pane2, pane3));
    drop(_server);

    // ── 2. A/B: TAKO_2367_LEGACY=1（修正前） ─────────────────────────────────────
    std::env::set_var(fd_limit::CONTAINER_LEGACY_ENV, "1");
    let legacy = format!("tako-clitest-{}-nofile2367legacy", std::process::id());
    let _legacy_server = ServerGuard(legacy.clone());
    let (old1, _old_rx1) = TerminalSession::spawn(
        80,
        24,
        tmux_backend::wrap_options(options(&home.0, Some(probe_command())), &legacy, "tako-o1"),
    )
    .expect("器の attach クライアントを起こせる");
    assert!(
        wait_for(&old1, &want),
        "修正前も器の子は起動時の値（{want}）。画面: {}",
        screen(&old1)
    );
    let old_limits = wait_server(&legacy);
    assert_eq!(
        old_limits.soft,
        Some(LAUNCH_SOFT),
        "修正前は器のサーバーも起動時の値: {old_limits:?}"
    );
    let (old_added, old_err) = add_windows(&legacy, WINDOWS_OVER_LAUNCH);
    eprintln!("修正前: window を {old_added} 本足したところで: {old_err:?}");
    assert!(
        old_added < WINDOWS_OVER_LAUNCH
            && old_err
                .as_deref()
                .is_some_and(|e| e.contains("fork failed: Too many open files")),
        "修正前は起動時の値で上限に届いて window を作れない: {old_added} 本・{old_err:?}"
    );
    let (old2, _old_rx2) = TerminalSession::spawn(
        80,
        24,
        tmux_backend::wrap_options(options(&home.0, Some(probe_command())), &legacy, "tako-o2"),
    )
    .expect("attach クライアント自体は起こせる");
    assert!(
        wait_for(&old2, "fork failed: Too many open files"),
        "修正前は新しいペインが作れず、attach クライアントが理由を出して終わる。画面: {}",
        screen(&old2)
    );
    eprintln!("修正前の新しいペインの画面: {}", screen(&old2).trim());
    std::env::remove_var(fd_limit::CONTAINER_LEGACY_ENV);
}

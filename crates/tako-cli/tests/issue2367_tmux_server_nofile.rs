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
//! # 2 の「新しいペインが作れない」は理由を stderr から採る（#2411）
//!
//! 新しい attach クライアントの端末は、サーバーが開いた時点で tmux の代替画面（`ESC[?1049h`）へ入り、理由は
//! その上に書かれる。待ちの状態（attach 前）のクライアントは理由を受け取るとそのまま終わり、サーバーは
//! ソケットの切断を見てから端末を閉じる = 代替画面を消して抜ける（`ESC[H ESC[2J … ESC[?1049l`）。一方
//! カーネルは、セッションリーダー（= この attach クライアント）が終わると端末の出力が流れ切るのを待ってから
//! 端末を revoke し、その後のサーバーの書き込みは失敗して何も届かない。**どちらが先かは別プロセスどうしの
//! 競争**で、手元では revoke が先なので理由が画面に残るが、macOS CI では 1 度サーバーが先に書き、画面が
//! 24 行とも空のまま 15 秒の窓が切れた（PR #2408 の CI）。そこで理由は attach クライアントの stderr
//! （ファイルへ向ける）から採り、終わったことは `ChildExit` を状態として待つ。
//!
//! もう 1 つの前提も状態で待つ: 枯渇させた直後は、失敗した `new-window` のクライアントの切断をサーバーが
//! まだ処理しておらず fd の空きが 1 本足りないことがある（手元の実測で 20 回中 2 回）。その間に繋ぐと
//! 端末を受け取れずに `open terminal failed: not a terminal` で終わるので、空きが [`ATTACH_FDS`] 本に
//! 戻ってから新しい attach クライアントを起こす。
//!
//! 待ちはどれも状態待ちで、上限は `state_wait_budget`（混み具合で伸ばすだけ・4 倍で打ち切り）。
//! 旧の判定（画面を固定 15 秒の窓で見る・前提を待たない）は A/B の腕として [`LEGACY_ENV`] に、
//! 2 つの競争の負け側を作る注入は [`INJECT_ENV`] に置く（どちらも読むだけ）。
//!
//! env（A/B）と構え（`arm_children`）はプロセス全体に効くので、このバイナリのテストはこの 1 本だけにする。
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_core::platform::fd_limit;
use tako_core::terminal::{SpawnCommand, SpawnOptions};
use tako_core::tmux_backend::{self, ServerNofile};
use tako_core::wait_budget::{machine_busy, state_wait_budget};
use tako_core::TerminalSession;

/// 起動時の soft（Finder 起動の 256 の代わりに小さくして、少ない window で上限に届かせる）
const LAUNCH_SOFT: u64 = 48;

/// 修正後に足す window の数（起動時の値のサーバーなら確実に上限を越える数）
const WINDOWS_OVER_LAUNCH: usize = 60;

/// 状態待ちの素の上限（#2411。混み具合で [`state_wait_budget`] が伸ばす）
const WAIT_BASE: Duration = Duration::from_secs(15);

/// 状態を読み直す間隔
const POLL: Duration = Duration::from_millis(50);

/// 新しい attach クライアントがペインの起動まで進むのに要るサーバーの fd の空き（#2411）。
/// 接続の accept と、クライアントが送ってくる端末の fd の受け取りで 2 本
const ATTACH_FDS: u64 = 2;

/// 修正前のペインの起動が失敗した理由（tmux の `spawn.c` の字面 + EMFILE の strerror）
const FORK_FAILED: &str = "fork failed: Too many open files";

/// 旧の判定（画面を固定 15 秒の窓で見る・前提を待たない）へ戻す A/B の腕（#2411）
const LEGACY_ENV: &str = "TAKO_2411_LEGACY";

/// 競争の負け側を作る注入（#2411）:
/// - `late-read`: 新しい attach クライアントを起こした直後に、このプロセス（tako の PTY の読み手を含む）を
///   [`FREEZE_FOR`] だけ止める = 混んだ CI のランナーで読み手が遅れた回。出力が流れ切らないので
///   クライアントの終了（端末の revoke）が待たされ、その間にサーバーが代替画面を消して抜ける
/// - `late-revoke`: 同じ順序を時間に依らずに作る（セッションリーダーのシェルをクライアントより 1 秒長生きさせ、
///   revoke をサーバーが端末を閉じた後へ遅らせる）
/// - `hold`: 枯渇の直後にサーバーへ素の接続を 1 本握らせ、[`HOLD_FOR`] 後に離す
///   = 失敗した `new-window` のクライアントの切断をサーバーがまだ処理していない回
const INJECT_ENV: &str = "TAKO_2411_INJECT";

/// `late-read` の注入がこのプロセスを止める時間
const FREEZE_FOR: &str = "0.5";

/// `hold` の注入が接続を握る時間（[`WAIT_BASE`] より十分短い）
const HOLD_FOR: Duration = Duration::from_secs(2);

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

/// `wrap_options` が組んだ attach クライアントの起動を `/bin/sh -c <script>` で包む（#2411）。
/// `script` は `$1` に `extra` を、`$2` 以降に元のコマンドを受け取る
fn wrapped(mut options: SpawnOptions, script: &str, extra: &str) -> SpawnOptions {
    let command = options
        .command
        .take()
        .expect("wrap_options はコマンドを組む");
    let mut args = vec![
        "-c".to_string(),
        script.to_string(),
        "sh".to_string(),
        extra.to_string(),
        command.program,
    ];
    args.extend(command.args);
    options.command = Some(SpawnCommand {
        program: "/bin/sh".to_string(),
        args,
    });
    options
}

/// attach クライアントの stderr を `file` へ向ける（#2411）。`exec` で置き換わるので、セッションリーダーも
/// 端末も製品と同じく tmux のクライアントのまま。`late_revoke` の注入ではシェルがクライアントより
/// 1 秒長生きし、端末の revoke を遅らせる（終了コードはクライアントのものを返す）
fn stderr_to(options: SpawnOptions, file: &Path, late_revoke: bool) -> SpawnOptions {
    let script = if late_revoke {
        "f=$1; shift; \"$@\" 2>>\"$f\"; rc=$?; sleep 1; exit $rc"
    } else {
        "f=$1; shift; exec \"$@\" 2>>\"$f\""
    };
    wrapped(options, script, &file.display().to_string())
}

/// `late-read` の注入: このプロセスを止めて [`FREEZE_FOR`] 秒後に再開させる外の `sh` を起こす
/// （止めるのも再開させるのも外のプロセス。終わったら呼び手が回収する）
fn freeze_self() -> std::process::Child {
    std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "kill -STOP \"$1\"; sleep \"$2\"; kill -CONT \"$1\"",
            "sh",
            &std::process::id().to_string(),
            FREEZE_FOR,
        ])
        .spawn()
        .expect("注入: このプロセスを止める sh を起こせる")
}

fn screen(session: &TerminalSession) -> String {
    session.visible_lines().join("\n")
}

/// 状態待ちの上限（[`WAIT_BASE`] を混み具合で伸ばす。1 回だけ測る）
fn budget() -> Duration {
    static BUDGET: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| state_wait_budget(WAIT_BASE, machine_busy()))
}

/// `probe` が `Some` を返すまで待つ（上限は [`budget`]）。待った時間も返す
fn wait_state<T>(mut probe: impl FnMut() -> Option<T>) -> (Option<T>, Duration) {
    let start = Instant::now();
    loop {
        if let Some(value) = probe() {
            return (Some(value), start.elapsed());
        }
        if start.elapsed() >= budget() {
            return (None, start.elapsed());
        }
        std::thread::sleep(POLL);
    }
}

/// 画面に `needle` が出るまで待つ。出なければ待った時間・上限・その時点の画面を添えて落ちる
fn wait_for(session: &TerminalSession, needle: &str, what: &str) {
    let (seen, waited) = wait_state(|| screen(session).contains(needle).then_some(()));
    assert!(
        seen.is_some(),
        "{what}（{needle}）が画面に出ない: waited={waited:?} budget={:?}。画面: {}",
        budget(),
        screen(session)
    );
}

fn wait_server(socket: &str) -> tmux_backend::ServerLimits {
    let mut last = None;
    let (limits, waited) = wait_state(|| match tmux_backend::server_nofile(socket) {
        ServerNofile::Running(limits) => Some(limits),
        other => {
            last = Some(other);
            None
        }
    });
    limits.unwrap_or_else(|| {
        panic!(
            "器のサーバーが読めない: waited={waited:?} budget={:?} 最後={last:?}",
            budget()
        )
    })
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

/// サーバーの fd の使用数と上限の 1 行（失敗文の材料）
fn server_fds(pid: u32) -> String {
    format!(
        "サーバー（pid {pid}）の fd={:?}/{LAUNCH_SOFT}",
        fd_limit::open_fds_of(pid)
    )
}

/// 修正前の器で新しい attach クライアントを起こし、ペインが作れずに理由を出して終わることを確かめる（#2411）
fn legacy_new_pane_fails(home: &Path, socket: &str, server_pid: u32) {
    let inject = std::env::var(INJECT_ENV).unwrap_or_default();
    let late_revoke = inject == "late-revoke";
    // 注入: 失敗した new-window のクライアントの切断を、サーバーがまだ処理していない回を作る
    let _held = (inject == "hold").then(|| {
        let path = tmux_backend::socket_dir()
            .expect("macOS ではソケットの置き場がある")
            .join(socket);
        let held = std::os::unix::net::UnixStream::connect(&path).expect("注入: 器へ繋げる");
        let (accepted, waited) = wait_state(|| {
            let open = fd_limit::open_fds_of(server_pid)?;
            (LAUNCH_SOFT.saturating_sub(open) < ATTACH_FDS).then_some(open)
        });
        assert!(
            accepted.is_some(),
            "注入の前提: 握った接続をサーバーが受け付ける: waited={waited:?} {}",
            server_fds(server_pid)
        );
        std::thread::spawn(move || {
            std::thread::sleep(HOLD_FOR);
            drop(held);
        })
    });
    let wrap = tmux_backend::wrap_options(options(home, Some(probe_command())), socket, "tako-o2");

    if std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1") {
        // A/B: 旧の判定 = 前提を待たずに起こし、画面を固定 15 秒の窓で見る
        let wrap = if late_revoke {
            wrapped(wrap, "shift; \"$@\"; rc=$?; sleep 1; exit $rc", "")
        } else {
            wrap
        };
        let (old2, _old_rx2) =
            TerminalSession::spawn(80, 24, wrap).expect("attach クライアント自体は起こせる");
        let freezer = (inject == "late-read").then(freeze_self);
        let start = Instant::now();
        let mut seen = false;
        while start.elapsed() < Duration::from_secs(15) {
            if screen(&old2).contains(FORK_FAILED) {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(mut freezer) = freezer {
            let _ = freezer.wait();
        }
        assert!(
            seen,
            "修正前は新しいペインが作れず、attach クライアントが理由を出して終わる。画面: {}",
            screen(&old2)
        );
        return;
    }

    // 前提: 新しい attach クライアントがペインの起動まで進める空きがある
    // （失敗した new-window のクライアントの切断をサーバーが処理し終えた）
    let (open, waited) = wait_state(|| {
        let open = fd_limit::open_fds_of(server_pid)?;
        (LAUNCH_SOFT.saturating_sub(open) >= ATTACH_FDS).then_some(open)
    });
    assert!(
        open.is_some(),
        "枯渇させたサーバーの fd の空きが {ATTACH_FDS} 本に戻らない: waited={waited:?} budget={:?} {}",
        budget(),
        server_fds(server_pid)
    );

    let stderr_file = home.join("tako-o2.stderr");
    let (mut old2, mut old_rx2) =
        TerminalSession::spawn(80, 24, stderr_to(wrap, &stderr_file, late_revoke))
            .expect("attach クライアント自体は起こせる");
    let freezer = (inject == "late-read").then(freeze_self);
    let stderr = || std::fs::read_to_string(&stderr_file).unwrap_or_default();
    let (status, waited) = wait_state(|| {
        while let Ok(event) = old_rx2.try_recv() {
            old2.process_event(event);
        }
        old2.exit_status()
    });
    let Some(status) = status else {
        panic!(
            "修正前は新しいペインが作れず attach クライアントが終わるはずが、終わらない: \
             waited={waited:?} budget={:?} 生存={:?} {} stderr={:?}。画面: {}",
            budget(),
            old2.child_pid().map(tako_core::ports::process_alive),
            server_fds(server_pid),
            stderr(),
            screen(&old2)
        )
    };
    if let Some(mut freezer) = freezer {
        let _ = freezer.wait();
    }
    let reason = stderr();
    eprintln!(
        "修正前の新しいペインの attach クライアント: {status:?}・waited={waited:?}・stderr={:?}",
        reason.trim()
    );
    assert!(
        !status.success() && reason.contains(FORK_FAILED),
        "修正前は新しいペインが作れず、attach クライアントが理由（{FORK_FAILED}）を出して終わる: \
         終了={status:?} stderr={reason:?} {}。画面: {}",
        server_fds(server_pid),
        screen(&old2)
    );
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
    // tako:caller-env-ok 器の段として登録するだけ（段は上限を下げて exec し、呼び出し元・接続先を読まない。#1716）
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
    wait_for(
        &pane1,
        &want,
        "器の明示コマンドの子は起動時の soft・hard はそのまま",
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
    let mut typed = 0u64;
    let (seen, waited) = wait_state(|| {
        if screen(&pane2).contains(&login_want) {
            return Some(());
        }
        // シェルの起動前に食われることがあるので、出るまで 1 秒ごとに打ち直す
        if start.elapsed() >= Duration::from_secs(typed) {
            pane2.write(
                b"case $0 in -*) l=login;; *) l=nologin;; esac; echo \"SH=[$l/$(ulimit -Sn)/$(ulimit -Hn)/$SHELL]\"\r"
                    .to_vec(),
            );
            typed += 1;
        }
        None
    });
    assert!(
        seen.is_some(),
        "ログインシェルは -sh の名で起き、soft は起動時の値・SHELL はそのまま（{login_want}）: \
         waited={waited:?} budget={:?} 打った回数={typed}。画面: {}",
        budget(),
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
    wait_for(
        &pane3,
        &want,
        "起動時の値を越えた器にも新しいペインが作れ、その子は起動時の値",
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
    wait_for(&old1, &want, "修正前も器の子は起動時の値");
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
            && old_err.as_deref().is_some_and(|e| e.contains(FORK_FAILED)),
        "修正前は起動時の値で上限に届いて window を作れない: {old_added} 本・{old_err:?}"
    );
    legacy_new_pane_fails(&home.0, &legacy, old_limits.pid);
    std::env::remove_var(fd_limit::CONTAINER_LEGACY_ENV);
}

//! **#2741**: 器（tmux）の最後のセッションが消えても、器のサーバーを終わらせない（`exit-empty off`）。
//!
//! ## 何が起きていたか
//!
//! tmux の既定（`exit-empty on`）では、最後のセッションが消えるとサーバーは「終わる」と決め、プロセスが
//! 消えるまで待ち受けのソケットを開いたままにする。その間に器のペインのクライアント
//! （`wrap_options` = `new-session -A -D`）が繋ぐと、connect は通るが受け付けられず、サーバーが消えた時点で
//! `server exited unexpectedly`（終了コード 1）で終わる。拒否（ECONNREFUSED）ではないので新しいサーバーも
//! 起こさない。ペインの PTY の子はそのクライアントなので、#1857 の繋ぎ直しはサーバー無しと判定して閉じる =
//! **作った直後の器のペインが即死する**（隔離セルフテストの項目 739 / 749 の間欠。混んだ機で隙間が延びる）。
//!
//! いまは器の conf（`tmux_backend::exit_empty_conf`）が `exit-empty off` を立てるので、サーバーは
//! 終わると決めない。A/B は `TAKO_2741_LEGACY=1`（`exit-empty on` = tmux の既定へ戻す）。
//!
//! ## 注入
//!
//! 隙間は空いた機ではマイクロ秒以下なので、`history-file` を FIFO に向けて、サーバーを終わり際
//! （履歴の書き出し）で止める = **CPU を焼かずに隙間を開けたままにする**。FIFO は試験の置き場の中にあり、
//! 置き場はサーバーを落とす前に消す（落とす時の履歴の書き出しが FIFO で止まらない）。
//!
//! 実 tmux の試験は `tmux_test_gate` を通る（CI の macOS ジョブは tmux 必須）。

use tako_core::tmux_backend::exit_empty_conf;

/// 器の conf は能力のある器（tmux）へだけ `exit-empty off` を書き、A/B は tmux の既定の `on` へ戻す。
/// 能力の無い器（psmux = 未実測）へは 1 行も書かない（#974）
#[test]
fn 器のconfはtmuxへだけexit_empty_offを書きabはonへ戻す() {
    use tako_core::backend::{PsmuxBackend, SessionBackend, TmuxBackend};
    let tmux = TmuxBackend::new().capabilities();
    let psmux = PsmuxBackend::new("psmux".into(), "3.3.7".into()).capabilities();
    assert!(tmux.keeps_empty_server);
    assert!(!psmux.keeps_empty_server);
    assert_eq!(exit_empty_conf(&tmux, false), "set -s exit-empty off\n");
    assert_eq!(exit_empty_conf(&tmux, true), "set -s exit-empty on\n");
    assert_eq!(exit_empty_conf(&psmux, false), "");
    assert_eq!(exit_empty_conf(&psmux, true), "");
}

/// 実 tmux の e2e（unix だけ）
#[cfg(unix)]
mod e2e {
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    use futures::channel::mpsc::UnboundedReceiver;
    use tako_core::backend_reattach::{probe_session, CloseCause, SessionProbe, Verdict};
    use tako_core::tmux_backend::{kill_session, wrap_options};
    use tako_core::{SessionEvent, SpawnCommand, SpawnOptions, TermEvent, TerminalSession};

    const MARK: &str = "TAKO2741-READY";

    /// このテストの器のソケット。Drop でサーバーごと落とす（空のサーバーを残さない = #2741 の副作用の面倒）
    struct Server(String);

    impl Server {
        fn new(tag: &str) -> Self {
            Self(format!("tako-coretest-2741{tag}-{}", std::process::id()))
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            tako_core::tmux_backend::kill_server(&self.0);
        }
    }

    /// 器のペインの中身（目印を出して入力を待つだけ）
    fn base() -> SpawnOptions {
        SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), format!("echo {MARK}; exec cat")],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        }
    }

    /// 本番と同じ器のペイン（`wrap_options` のクライアント + #2625 の閉じ方の印）
    fn spawn(socket: &str, session: &str) -> (TerminalSession, UnboundedReceiver<SessionEvent>) {
        let (mut pane, rx) = TerminalSession::spawn(80, 24, wrap_options(base(), socket, session))
            .expect("器のクライアントを spawn できる");
        pane.hang_up_client_on_close();
        (pane, rx)
    }

    /// 目印が出たら `Some(true)`、目印の前に子（器のクライアント）が終わったら `Some(false)`、上限まで何も無ければ `None`
    fn outcome(
        pane: &mut TerminalSession,
        rx: &mut UnboundedReceiver<SessionEvent>,
        limit: Duration,
    ) -> Option<bool> {
        let ready = |p: &TerminalSession| p.visible_lines().iter().any(|l| l.contains(MARK));
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            while let Ok(event) = rx.try_recv() {
                let exit = matches!(event, SessionEvent::Term(TermEvent::ChildExit(_)));
                pane.process_event(event);
                if ready(pane) {
                    return Some(true);
                }
                if exit {
                    return Some(false);
                }
            }
            if ready(pane) {
                return Some(true);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// 子（器のクライアント）の終わりが届くまで待つ。届いたら true
    fn wait_exit(
        pane: &mut TerminalSession,
        rx: &mut UnboundedReceiver<SessionEvent>,
        limit: Duration,
    ) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            while let Ok(event) = rx.try_recv() {
                let exit = matches!(event, SessionEvent::Term(TermEvent::ChildExit(_)));
                pane.process_event(event);
                if exit {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// 画面の空でない行（失敗の診断用）
    fn screen(pane: &TerminalSession) -> String {
        pane.visible_lines()
            .iter()
            .filter(|l| !l.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// 器のサーバーの pid（`display-message` で引く。セッションが要る）
    fn server_pid(socket: &str, session: &str) -> i32 {
        let out = tako_core::tmux::tmux_command(Some(socket))
            .args([
                "display-message",
                "-p",
                "-t",
                &tako_core::tmux::exact_target(session),
                "#{pid}",
            ])
            .output()
            .expect("display-message を起こせる");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("器のサーバーの pid を読めない: {out:?}"))
    }

    /// そのプロセスが居るか（器のサーバーは launchd の子なので waitpid ではなく kill 0 で見る）
    fn alive(pid: i32) -> bool {
        // SAFETY: シグナル 0 は送らずに存在だけを問う
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// 器のサーバーの `exit-empty` の値
    fn exit_empty(socket: &str) -> String {
        let out = tako_core::tmux::tmux_command(Some(socket))
            .args(["show-options", "-sv", "exit-empty"])
            .output()
            .expect("show-options を起こせる");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// 修正前の形（`TAKO_2741_LEGACY=1` で回した）か
    fn legacy() -> bool {
        tako_core::tmux_backend::issue2741_legacy()
    }

    /// 本題: 器の最後のセッションを製品どおりに閉じ（#2625 の SIGHUP → 読み捨て → 閉じる → kill-session）、
    /// サーバーを終わり際で止めた（注入）間に次の器のペインを作る。
    /// 修正後はサーバーが終わると決めない = 止まりもせず、次のペインは目印を出して生き残る。
    /// 修正前（A/B）はサーバーが止まった後に消え、次のペインは目印の前に死ぬ
    #[test]
    fn 最後の器を閉じた直後に作った器のペインは終わり際のサーバーに当たらず生き残る() {
        if !tako_core::tmux_test_gate::container() {
            return;
        }
        let server = Server::new("i");
        // 置き場はサーバーより後に宣言する = 先に消える（落とす時の履歴の書き出しが FIFO で止まらない）
        let dir = tako_core::test_residue::ScratchDir::new("tako-2741-fifo");
        let fifo = dir.path().join("hist");
        let c_fifo = std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("NUL を含まない");
        // SAFETY: このテストの置き場の中へ FIFO を 1 つ作るだけ
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);

        let (first, mut rx1) = spawn(&server.0, "tako-e2e-2741-a");
        let mut first = first;
        assert_eq!(
            outcome(&mut first, &mut rx1, Duration::from_secs(10)),
            Some(true),
            "A が立つ"
        );
        let pid = server_pid(&server.0, "tako-e2e-2741-a");
        let set = tako_core::tmux::tmux_command(Some(&server.0))
            .args(["set", "-g", "history-file"])
            .arg(&fifo)
            .output()
            .expect("history-file を立てる");
        assert!(set.status.success(), "{set:?}");

        // 製品の閉じ方: 端末を落とし（#2625）、kill-session（`drop_backend_session` と同じ口）
        drop(first);
        drop(rx1);
        kill_session(&server.0, "tako-e2e-2741-a");
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(pid), "最後のセッションが消えた 300ms 後もサーバーのプロセスは居る（修正後は残る・修正前は終わり際で止まっている）");

        let (second, mut rx2) = spawn(&server.0, "tako-e2e-2741-b");
        let mut second = second;
        if legacy() {
            // 修正前: 止まっている間は目印も終わりも無い → 止めを外すとサーバーが消え、目印の前に死ぬ
            let before = outcome(&mut second, &mut rx2, Duration::from_millis(500));
            let reader = {
                let fifo = fifo.clone();
                std::thread::spawn(move || std::fs::read(&fifo).map(|b| b.len()))
            };
            let after = outcome(&mut second, &mut rx2, Duration::from_secs(10));
            let _ = reader.join();
            println!(
                "TAKO_2741_LEGACY: before={before:?} after={after:?} status={:?} screen={:?}",
                second.exit_status(),
                screen(&second)
            );
            assert_eq!(before, None);
            assert_eq!(
                after,
                Some(false),
                "修正前は終わり際のサーバーに当たって目印の前に死ぬ"
            );
            return;
        }
        let got = outcome(&mut second, &mut rx2, Duration::from_secs(10));
        assert_eq!(
            got,
            Some(true),
            "最後の器を閉じた直後に作った器のペインが生き残る。画面: {:?} 終了: {:?}",
            screen(&second),
            second.exit_status()
        );
        assert!(
            alive(pid),
            "同じサーバーがそのまま受け付けた（終わると決めていない）"
        );
    }

    /// 最後のセッションを消しても器のサーバーは残る（空のまま）。修正前（A/B）は消える
    #[test]
    fn 最後のセッションを消しても器のサーバーは空のまま残る() {
        if !tako_core::tmux_test_gate::container() {
            return;
        }
        let server = Server::new("w");
        let (first, mut rx1) = spawn(&server.0, "tako-e2e-2741-w");
        let mut first = first;
        assert_eq!(
            outcome(&mut first, &mut rx1, Duration::from_secs(10)),
            Some(true)
        );
        let pid = server_pid(&server.0, "tako-e2e-2741-w");
        assert_eq!(exit_empty(&server.0), if legacy() { "on" } else { "off" });
        drop(first);
        drop(rx1);
        kill_session(&server.0, "tako-e2e-2741-w");
        let killed = Instant::now();
        if legacy() {
            while alive(pid) && killed.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_micros(200));
            }
            println!(
                "TAKO_2741_LEGACY: kill-session からサーバーが消えるまで {:?}",
                killed.elapsed()
            );
            assert!(
                !alive(pid),
                "修正前は最後のセッションと一緒にサーバーも消える"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(pid), "最後のセッションが消えても器のサーバーは残る");
        let sessions = tako_core::tmux::tmux_command(Some(&server.0))
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .expect("list-sessions を起こせる");
        assert!(
            String::from_utf8_lossy(&sessions.stdout).trim().is_empty(),
            "残るサーバーは空: {sessions:?}"
        );
    }

    /// 修正前に起きた器のサーバー（tmux の既定 `exit-empty on` のまま）へも、`sync_conf` の再適用で `off` が届く
    /// （conf は `-f` でサーバーの起動時にしか読まれないので、GUI の起動・persist の有効化が呼ぶ再適用が要る）
    #[test]
    fn 走っている器のサーバーへsync_confでexit_empty_offが届く() {
        if !tako_core::tmux_test_gate::container() {
            return;
        }
        let server = Server::new("c");
        // 修正前の器の形: conf を読ませずに起こしたサーバー
        let started = tako_core::tmux::tmux_command(Some(&server.0))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "tako-e2e-2741-c",
                "exec cat",
            ])
            .output()
            .expect("new-session を起こせる");
        assert!(started.status.success(), "{started:?}");
        assert_eq!(
            exit_empty(&server.0),
            "on",
            "conf を読んでいない器は tmux の既定"
        );
        tako_core::tmux_backend::sync_conf(&server.0);
        let after = exit_empty(&server.0);
        println!("TAKO_2741_SYNC: sync_conf の前 on → 後 {after}");
        assert_eq!(
            after,
            if legacy() { "on" } else { "off" },
            "走っている器のサーバーへ再適用で届く"
        );
    }

    /// 注入なしで、kill からの間合いを 0〜20 ms 振っても次の器のペインは生き残る（修正前後とも = 普段の経路を壊さない）
    #[test]
    fn 最後の器を閉じてから間合いを振って作った器のペインは生き残る() {
        if !tako_core::tmux_test_gate::container() {
            return;
        }
        let server = Server::new("s");
        let mut dead = Vec::new();
        for i in 0..21u64 {
            let a = format!("tako-e2e-2741-a{i}");
            let (first, mut rx1) = spawn(&server.0, &a);
            let mut first = first;
            assert_eq!(
                outcome(&mut first, &mut rx1, Duration::from_secs(10)),
                Some(true),
                "A が立つ（周 {i}）"
            );
            drop(first);
            drop(rx1);
            let killer = {
                let socket = server.0.clone();
                std::thread::spawn(move || kill_session(&socket, &a))
            };
            std::thread::sleep(Duration::from_millis(i));
            let b = format!("tako-e2e-2741-b{i}");
            let (second, mut rx2) = spawn(&server.0, &b);
            let mut second = second;
            let got = outcome(&mut second, &mut rx2, Duration::from_secs(10));
            let _ = killer.join();
            if got != Some(true) {
                dead.push((i, got, screen(&second)));
            }
            drop(second);
            drop(rx2);
            kill_session(&server.0, &b);
        }
        assert!(
            dead.is_empty(),
            "間合い（ms）ごとの死んだ器のペイン: {dead:?}"
        );
    }

    /// 空の器のサーバーが残っても、#1857 の繋ぎ直しと #1197 のサーバー消失の判定は変わらない:
    /// - 器の中のコマンドが自ら終わる → クライアントが終わり、生死は「セッションが無い」= 閉じる（修正前はサーバーごと消えて
    ///   「サーバーが無い」= 同じく閉じる。どちらも `Verdict::Close`）
    /// - 器のサーバーが落ちる（`kill-server` = #1197 の事故）→ 全クライアントが終わり、生死は「サーバーが無い」= 閉じる
    #[test]
    fn 空の器が残っても繋ぎ直しとサーバー消失の判定は変わらない() {
        if !tako_core::tmux_test_gate::container() {
            return;
        }
        use tako_core::backend_reattach::ReattachState;
        let server = Server::new("r");
        let (first, mut rx1) = spawn(&server.0, "tako-e2e-2741-r1");
        let mut first = first;
        assert_eq!(
            outcome(&mut first, &mut rx1, Duration::from_secs(10)),
            Some(true)
        );
        // 器の中のコマンド（cat）を自ら終わらせる（EOF）
        first.write(vec![0x04]);
        assert!(
            wait_exit(&mut first, &mut rx1, Duration::from_secs(10)),
            "器の中のコマンドが終わるとクライアントも終わる。画面: {:?}",
            screen(&first)
        );
        let probe = probe_session(&server.0, "tako-e2e-2741-r1");
        let verdict = ReattachState::new().decide(&probe, first.child_pid(), Instant::now());
        println!("TAKO_2741_REATTACH: 自ら終わった後 probe={probe:?} verdict={verdict:?}");
        assert!(
            matches!(probe, SessionProbe::SessionGone | SessionProbe::ServerGone),
            "{probe:?}"
        );
        assert!(
            matches!(verdict, Verdict::Close(_)),
            "繋ぎ直さず閉じる: {verdict:?}"
        );
        if !legacy() {
            assert_eq!(
                probe,
                SessionProbe::SessionGone,
                "修正後は空のサーバーが残るのでセッションだけが無い"
            );
        }

        // 空のサーバーへ次の器のペインを作り、サーバーを落とす（#1197 の事故）
        let (second, mut rx2) = spawn(&server.0, "tako-e2e-2741-r2");
        let mut second = second;
        assert_eq!(
            outcome(&mut second, &mut rx2, Duration::from_secs(10)),
            Some(true)
        );
        let killed = tako_core::tmux::tmux_command(Some(&server.0))
            .arg("kill-server")
            .output()
            .expect("kill-server を起こせる");
        assert!(killed.status.success(), "{killed:?}");
        assert!(
            wait_exit(&mut second, &mut rx2, Duration::from_secs(10)),
            "サーバーが落ちるとクライアントも終わる"
        );
        let probe = probe_session(&server.0, "tako-e2e-2741-r2");
        let verdict = ReattachState::new().decide(&probe, second.child_pid(), Instant::now());
        println!("TAKO_2741_REATTACH: kill-server の後 probe={probe:?} verdict={verdict:?}");
        assert_eq!(probe, SessionProbe::ServerGone);
        assert_eq!(verdict, Verdict::Close(CloseCause::ServerGone));
    }
}

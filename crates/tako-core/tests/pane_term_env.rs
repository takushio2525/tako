//! ペインへ渡る TERM / COLORTERM を**実シェルで**測る（#946）
//!
//! 単体テスト（`terminal::tests`）は env の組み立てが親にも `options.env` にも
//! 依らないことを純関数で固定する。こちらは **本当にペインの中でそう見えるのか**を
//! 実 PTY で測る。#946 が疑った壊れ方（tako-app を起こした親の TERM がペインへ
//! 素通しになる）は、このテストが `TAKO_946_INJECT=inherit` で再現できる。
//!
//! **このテストはプロセス env（TERM / COLORTERM）を書き換える**ので、専用の
//! 統合テストバイナリに 1 本だけ置き、親 TERM の系統は 1 つの test 関数の中で
//! 順に回す（同じバイナリに別のテストを足さないこと）。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

/// プロセス env を触る検査は直列化する（同じバイナリの 2 本が並走すると
/// 互いの親 env を踏む）
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 1 系統あたりの観測上限。`/bin/sh -c echo` が 1 行出すだけなので実測は数十 ms。
/// 混んだ機でも足りるように広く採る（状態待ちなので空いていれば即返る）
const LIMIT: Duration = Duration::from_secs(30);

/// ペインの中で `TERMCHK=<TERM>,<COLORTERM>` を 1 行出し、画面から拾って返す
fn observe_termchk() -> String {
    let (session, rx) = TerminalSession::spawn(
        80,
        12,
        SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    // 打った行がそのまま画面に出ることはない（-c 実行なのでエコーしない）
                    "echo \"TERMCHK=${TERM},${COLORTERM}\"".to_string(),
                ],
            }),
            // **ここに TERM を入れても効かない**のが #946 の要点。
            // 呼び出し側の env で注入が消えないことを実シェルでも押さえる
            env: vec![
                ("TERM".to_string(), "dumb".to_string()),
                ("COLORTERM".to_string(), "8bit".to_string()),
            ],
            ..SpawnOptions::default()
        },
    )
    .expect("PTY を起こせる");
    // イベントは読み捨てる（グリッドは IO スレッドが直接更新する）
    std::thread::spawn(move || {
        let mut rx = rx;
        while futures::executor::block_on(futures::StreamExt::next(&mut rx)).is_some() {}
    });
    // **固定待ちにしない**（`conventions.md`「セルフテストの待ち条件の書き方」）。
    // 出るまで待ち、上限に届いたら画面をそのまま返して診断に載せる
    let started = Instant::now();
    loop {
        let screen = session.visible_lines().join("\n");
        if let Some(v) = tako_core::terminal::observed_termchk(&screen) {
            return v;
        }
        if started.elapsed() >= LIMIT {
            return format!(
                "<未観測 waited={:.1}s 画面={screen:?}>",
                started.elapsed().as_secs_f32()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 親プロセスの TERM / COLORTERM を差し替えて 1 系統を測る
fn with_parent_env(
    term: Option<&str>,
    colorterm: Option<&str>,
    f: impl FnOnce() -> String,
) -> String {
    let (old_term, old_color) = (std::env::var_os("TERM"), std::env::var_os("COLORTERM"));
    match term {
        Some(v) => std::env::set_var("TERM", v),
        None => std::env::remove_var("TERM"),
    }
    match colorterm {
        Some(v) => std::env::set_var("COLORTERM", v),
        None => std::env::remove_var("COLORTERM"),
    }
    let out = f();
    match old_term {
        Some(v) => std::env::set_var("TERM", v),
        None => std::env::remove_var("TERM"),
    }
    match old_color {
        Some(v) => std::env::set_var("COLORTERM", v),
        None => std::env::remove_var("COLORTERM"),
    }
    out
}

/// #946: ペインの端末申告は**親プロセスが何を名乗っていても**同じ。
///
/// 系統は Issue が踏んだ形（tmux の器の中 = `tmux-256color`）と、
/// 境界（COLORTERM だけある / TERM が空文字 / そもそも既定値と同じ / 未設定）。
#[test]
#[cfg(unix)]
fn ペインのtermは親プロセスに依らない() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let expected = tako_core::terminal::expected_termchk();
    for (term, colorterm) in [
        (Some("tmux-256color"), Some("truecolor")), // #946 が踏んだ形
        (Some("screen-256color"), None),
        (Some(""), Some("truecolor")),               // TERM が空文字
        (None, Some("truecolor")),                   // COLORTERM だけある
        (Some("xterm-256color"), Some("truecolor")), // 既定値そのもの
        (None, None),                                // .app（Finder 起動）相当
    ] {
        let observed = with_parent_env(term, colorterm, observe_termchk);
        assert_eq!(
            observed, expected,
            "親 TERM={term:?} COLORTERM={colorterm:?} でペインの申告が変わった"
        );
    }
}

/// #946 の注入口: 注入を外すと親の TERM がペインへ素通しになる（= Issue が疑った壊れ方）。
///
/// 検出力の担保でもある。上の検査が「たまたま通っている」のではなく、
/// **注入が効いているから通っている**ことをこのアームが示す
#[test]
#[cfg(unix)]
fn 注入を外すと親のtermがペインへ素通しになる() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("TAKO_946_INJECT", "inherit");
    let observed = with_parent_env(Some("tmux-256color"), Some("truecolor"), observe_termchk);
    std::env::remove_var("TAKO_946_INJECT");
    assert_eq!(
        observed, "tmux-256color,truecolor",
        "注入を外しても親の TERM が届いていない（注入口が効いていない）"
    );
    // 診断が「継承」と名指しできること（セルフテスト項目 1b が引く判定と同じ関数）
    let diag = tako_core::terminal::diagnose_termchk(
        &format!("TERMCHK={observed}"),
        Some("tmux-256color"),
        Some("truecolor"),
    );
    assert!(
        matches!(diag, tako_core::terminal::TermCheck::Inherited { .. }),
        "継承と診断されない: {diag:?}"
    );
}

//! 子の終了を知らせる時点で、その子の出力が**画面へ全部入っている**こと（#1707 / #1628）。
//!
//! `PtyLoop` は子の終了を観測すると `break 'event_loop` でループを畳み、
//! **以後 PTY を読む者はいない**。そこで読み残した末尾は永久に失われる。
//! upstream alacritty は同じ位置に `drain_on_exit` の段を持つが、#817 の移植で
//! 「tako は使っていない」として落ちていた（実際は `..tty::Options::default()` の
//! 既定 `false` を受け取っていただけ）。
//!
//! 利用者から見ると「すぐ終わるコマンドの出力の末尾が消える」で、ペインの表示だけでなく
//! ペインログ（FR-5.13）・`tako logs`・`tako run` の結果にも同じ欠落が出る。
//!
//! # 欠落が起きる形（実測から）
//!
//! 機構は**出力量ではなく競合**。mio が返す 1 束の中で子の終了トークンが読み取り
//! トークンより先に処理されると、まだパースしていないバイトを抱えたまま畳んでしまう。
//! だから当たるのは「**短い出力を吐いて即終了する子**」で、実測（PR #1641 の 1 巡目）は
//! `Write-Output` 3 行のうち 1 行目しか画面に入らなかった。
//!
//! 逆に**大量出力では再現しない**。ConPTY と中継パイプが詰まると子は書き進められず
//! tako が読むまで待つ（背圧）ので、子が終了する頃には取りこぼす残りが無い。
//! 最初この網を大量出力で書いて「決定的」と考えたが、Windows CI で旧経路が緑になり
//! 前提の誤りが分かった。いまの形は実測で欠落が出た短命・即終了に寄せてある。
//!
//! # この網が守るもの
//!
//! **修正後は毎回緑**（読み切ってから知らせるので競合しても結果が変わらない）。
//! 逆向き（読み切りを外すと必ず落ちる）は競合次第なので保証できない。
//! そこで同じ形を [`ROUNDS`] 回繰り返して検出力を稼ぐ。
//! 旧経路の確認は `TAKO_1628_LEGACY=1` の A/B で手元から行う（手順は PR #1641 本文）。

use std::time::{Duration, Instant};

use tako_core::terminal::{SessionNotice, SpawnCommand, SpawnOptions, TerminalSession};

/// 末尾に置く目印。これが画面に無ければ「終了を知らせる時点で読み切れていない」
const TAIL: &str = "TAIL-MARKER-1707";

/// 競合待ちなので 1 回では当たらない。同じ形を繰り返して検出力を稼ぐ
const ROUNDS: usize = 30;

/// 終了の知らせを待つ上限（安全弁）。混み具合で伸ばす政策は
/// `tako_core::wait_budget` の 1 実装を通す
fn exit_budget() -> Duration {
    tako_core::wait_budget::state_wait_budget(
        Duration::from_secs(30),
        tako_core::wait_budget::machine_busy(),
    )
}

/// 短い出力を吐いて**即終了する**子（実測で欠落が出た形）
fn short_then_exit() -> SpawnCommand {
    if cfg!(windows) {
        SpawnCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-Command".into(),
                format!(
                    "[Console]::Out.WriteLine('HEAD'); [Console]::Out.WriteLine('MID'); \
                     [Console]::Out.WriteLine('{TAIL}')"
                ),
            ],
        }
    } else {
        SpawnCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("printf 'HEAD\\nMID\\n{TAIL}\\n'")],
        }
    }
}

/// 子の終了が知らされるまで回し、**その時点の画面**を返す。
/// 終了が来なければ `None`（上限は安全弁で、判定は観測した事象で行う）
fn screen_when_child_exits() -> Option<String> {
    let (mut session, mut rx) = TerminalSession::spawn(
        140,
        40,
        SpawnOptions {
            command: Some(short_then_exit()),
            cwd: Some(std::env::temp_dir()),
            env: Vec::new(),
            scrollback_lines: None,
        },
    )
    .expect("PTY を起動できること");

    let deadline = Instant::now() + exit_budget();
    while Instant::now() < deadline {
        let mut exited = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(session.process_event(ev), Some(SessionNotice::Exited)) {
                exited = true;
            }
        }
        if exited {
            // 終了を知った**その時点**の画面を見る。修正後はここで読み切り済み
            return Some(session.visible_lines().join("\n"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// 短命な子の出力は、終了を知らせる時点で末尾まで画面に入っていること（#1707）。
///
/// 読み切りの段を外すと、競合に当たった回だけここが落ちる
#[test]
fn 短命な子の出力は終了を知らせる時点で画面へ全部入っている() {
    let mut missed = Vec::new();
    for round in 0..ROUNDS {
        let Some(screen) = screen_when_child_exits() else {
            panic!(
                "{round} 回目: 子の終了が {:?} 以内に知らされない（この網の前提が崩れている）",
                exit_budget()
            );
        };
        if !screen.contains(TAIL) {
            let seen: Vec<&str> = screen.lines().filter(|l| !l.is_empty()).collect();
            missed.push(format!("{round} 回目: 画面={seen:?}"));
        }
    }
    assert!(
        missed.is_empty(),
        "終了を知らせる時点で末尾が画面に無い回がある（#1707: 読み切らずに break すると失われる）\n\
         期待する目印: {TAIL}\n落ちた回 {} / {ROUNDS}:\n{}",
        missed.len(),
        missed.join("\n")
    );
}

/// 旧経路（#817 の形 = 読み切らずに畳む）では末尾が落ちること = 読み切りの段が効いている A/B。
///
/// **CI からは外してある**（`#[ignore]`）。理由は 2 つ:
///
/// 1. 欠落は mio が返す 1 束の中での順序に依るので、旧経路が**必ず落ちる**形にはできない。
///    実測（PR #1641 の 2 巡目）で Windows CI の旧経路が緑になり、この網が偽の赤を出した
/// 2. macOS では構造的に一度も再現しない（unix の pty に中継スレッドが無く、子は tako が
///    読むまで書き進められないので、終了を観測する頃には読み終わっている）
///
/// 手元で確かめるときは **Windows 実機**で数回回す:
///
/// ```text
/// cargo test -p tako-core --test pty_exit_drain -- --ignored --nocapture
/// ```
#[test]
#[ignore = "旧経路の欠落は競合次第で必ずは落ちない。Windows 実機で手動確認する A/B（#1707）"]
fn 旧経路では終了時に末尾が落ちる() {
    let exe = std::env::current_exe().expect("テストバイナリの位置を取れること");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            "短命な子の出力は終了を知らせる時点で画面へ全部入っている",
            "--nocapture",
        ])
        .env("TAKO_1628_LEGACY", "1")
        .output()
        .expect("同じ網を旧経路で起動できること");
    assert!(
        !out.status.success(),
        "旧経路（TAKO_1628_LEGACY=1）でも緑になった。\n\
         競合に当たらなかっただけの可能性があるので、数回繰り返してから判断すること\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

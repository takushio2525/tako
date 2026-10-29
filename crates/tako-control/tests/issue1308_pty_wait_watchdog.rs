//! 番犬: 実 PTY の fixture を待つテストが**固定窓**へ戻っていない（#1308）
//!
//! ## なぜ止めるのか
//!
//! `dispatch::tests::respondは保持しているペインへin_process経路で届く` は元々
//!
//! 1. 「素のシェルのプロンプトが出る」を**固定 10 秒**で待ち
//! 2. 尽きても**検査せずに**打ち込み
//! 3. 「ダイアログが描かれる」を**固定 20 秒**で待ち
//! 4. 尽きても**検査せずに** `dispatch` していた
//!
//! ので、混んだ機で素のシェルの起動が 10 秒を超えると、起動前の PTY へ打ち込んだ行は
//! エコーされるだけで実行されず、最後の `dispatch` が
//! **「器越しへ倒れている（#1200）」という無関係な原因**を名指しして落ちていた。
//!
//! 実測（2026-09-11・全件走 18 スレッド + 外の `cargo build`、load 60〜76）:
//!
//! ```text
//! prompt_ok=false prompt_waited=10.03s dialog_seen=false dialog_waited=20.04s load=3.89
//! break_screen=clear; printf '%b' '…'; sleep 30      ← 打った行のエコーだけ
//! panicked: in-process 経路で応答できていない（#1200。器越しへ倒れている）
//! ```
//!
//! 19 回の全件走に 1 回（Issue の棚卸しでは 3/30）。**窓を伸ばすだけでは足りない**のが
//! 肝で、固定窓に戻ると「窓が短いのか / シェルが起きていないのか」が出力から消える
//! （#1265 と同じ理由）。だからここで止める。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 対象は `crates/tako-control/src/dispatch.rs` の**実 PTY を張るテストだけ**
//! （`TerminalSession::spawn(` を持つ関数本体）。
//!
//! 1. 実 PTY を張るテストが待ちを持つなら、上限は `state_wait_budget` 由来であること
//!    （#1297 の `issue1297_実ptyのゴースト提案で…` は自前の `Instant::now() + budget` だが
//!    `budget` が `state_wait_budget` なので通る）
//! 2. #1308 の現場は状態待ちのドライバ `i1308_wait_for_state` を通すこと
//!    （ドライバは**尽きたら panic する** = 呼び出し側が素通りできない構造にしてある）
//! 3. そのドライバ自身が予算・混み具合・診断・A/B の旧経路アームを持っていること
//! 4. （10/25 の再発）#1308 の現場は POSIX でログインシェルを経由せず、描画のコマンドを
//!    直接起こすこと（`i1308_direct_options`）
//! 5. （#2415）実 PTY を張るテストは、どれも**素の既定**（`SpawnOptions::default()` =
//!    開発者のログインシェル）を起こさないこと。セッションがあるだけでよいなら
//!    `i2415_session_options` を通す

use std::path::{Path, PathBuf};

/// A/B の旧経路アーム（`TAKO_1308_LEGACY=1`）の囲み
const ARM_BEGIN: &str = "TAKO_1308_LEGACY_ARM 開始";
const ARM_END: &str = "TAKO_1308_LEGACY_ARM 終了";

/// #1308 の現場
const GUARDED: &str = "respondは保持しているペインへin_process経路で届く";
/// 状態待ちの共通ドライバ
const DRIVER: &str = "i1308_wait_for_state";

/// 自前の待ち（固定窓）の印
const HAND_ROLLED: [&str; 3] = ["Instant::now() +", "thread::sleep", "for _ in 0.."];

fn target() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .join("crates/tako-control/src/dispatch.rs")
}

/// 切り出した関数本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    name: String,
    line: usize,
    text: String,
}

/// インデント 4 の `fn <name>(` から、インデント 4 の閉じ括弧までを本体として切り出す
/// （テストモジュールの中の関数は必ずこの形。`rustfmt` が保証する）。
///
/// `common/test_source.rs` の `body_of` は引数なしの関数しか拾えないので、
/// 引数つきのドライバも見られるようここに置く
fn bodies(src: &str) -> Vec<Body> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.strip_prefix("    fn ") else {
            continue;
        };
        let Some(name) = rest.split('(').next() else {
            continue;
        };
        if name.is_empty() || name.contains(' ') {
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && lines[j] != "    }" {
            j += 1;
        }
        out.push(Body {
            name: name.to_string(),
            line: i + 1,
            text: lines[i..=j.min(lines.len() - 1)].join("\n"),
        });
    }
    out
}

/// A/B の旧経路アームを**空行へ潰す**（行番号を保つ）
fn blank_arm(body: &str) -> (String, usize) {
    let mut out = String::new();
    let mut dropped = 0usize;
    let mut inside = false;
    for line in body.lines() {
        if line.contains(ARM_BEGIN) {
            inside = true;
            dropped += 1;
        }
        if !inside && !line.contains(ARM_END) {
            out.push_str(line);
        }
        if line.contains(ARM_END) {
            inside = false;
        }
        out.push('\n');
    }
    (out, dropped)
}

#[path = "common/code_view.rs"]
mod code_view;

fn read_src() -> String {
    std::fs::read_to_string(target()).expect("dispatch.rs を読める")
}

/// 違反の名指し（`file:line fn 名: 理由`）
fn at(body: &Body, offset: usize, why: &str) -> String {
    format!(
        "crates/tako-control/src/dispatch.rs:{} fn {}: {why}",
        body.line + offset,
        body.name
    )
}

#[test]
fn 実ptyを張るテストの待ちはstate_wait_budget由来である() {
    let src = read_src();
    let mut offenders: Vec<String> = Vec::new();
    for body in bodies(&src) {
        if !body.text.contains("TerminalSession::spawn(") {
            continue;
        }
        let (code, _) = blank_arm(&body.text);
        // 状態待ちの上限を持っているなら文句は無い
        if code.contains("state_wait_budget") || code.contains(&format!("{DRIVER}(")) {
            continue;
        }
        for (n, line) in code.lines().enumerate() {
            if let Some(pat) = HAND_ROLLED.iter().find(|p| line.contains(**p)) {
                offenders.push(at(
                    &body,
                    n,
                    &format!("実 PTY を自前の固定窓（`{pat}`）で待っている"),
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "固定窓は「窓が足りない」のか「シェルが起きていない」のかを診断から消す\n\
         （#1308 は起動前の PTY へ打ち込み、`#1200 の回帰`という無関係な原因で落ちていた）。\n\
         上限は `tako_core::wait_budget::state_wait_budget`（混み具合で伸ばすだけ・\n\
         4 倍で打ち切り）由来にするか、`{DRIVER}` を通すこと:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn issue1308の現場は状態待ちのドライバを通している() {
    let src = read_src();
    let body = bodies(&src)
        .into_iter()
        .find(|b| b.name == GUARDED)
        .unwrap_or_else(|| panic!("#1308 の現場（{GUARDED}）が切り出せていない"));
    let (code, _) = blank_arm(&body.text);
    let mut offenders: Vec<String> = Vec::new();
    if !code.contains(&format!("{DRIVER}(")) {
        offenders.push(at(
            &body,
            0,
            &format!("状態待ちのドライバ（{DRIVER}）を通していない"),
        ));
    }
    for (n, line) in code.lines().enumerate() {
        if let Some(pat) = HAND_ROLLED.iter().find(|p| line.contains(**p)) {
            offenders.push(at(&body, n, &format!("自前の待ち（`{pat}`）が戻っている")));
        }
    }
    assert!(
        offenders.is_empty(),
        "#1308 の現場は待ちを 1 か所（{DRIVER}）に寄せてある。\n\
         ドライバは**上限に達したら panic する**ので、呼び出し側が素通りできない\n\
         （旧実装は尽きても検査せず次へ進み、原因が別の場所で別の顔をして現れていた）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 状態待ちのドライバは予算と診断とab経路を持つ() {
    let src = read_src();
    let body = bodies(&src)
        .into_iter()
        .find(|b| b.name == DRIVER)
        .unwrap_or_else(|| panic!("状態待ちのドライバ（{DRIVER}）が切り出せていない"));
    // A/B のアームは**コメントの目印**で測るので、除去は生の本文でやってから
    // コメントを落とす（#1609。全文のままだと「この綴りを通す」と書いた
    // 説明コメントで緑になり、実体が消えても気づけない）
    let (code, dropped) = blank_arm(&body.text);
    let code = code_view::without_comments(&code);
    assert_eq!(
        dropped, 1,
        "A/B の旧経路アーム（{ARM_BEGIN}）が 1 つでない。\
         マーカーを消したなら番犬の前提が崩れているので、番犬側も直すこと"
    );
    for needed in [
        "state_wait_budget(",
        "machine_busy()",
        "panic!(",
        "TAKO_1308_WAIT",
    ] {
        assert!(
            code.contains(needed),
            "{DRIVER} に `{needed}` が無い（予算・混み具合・素通り防止・診断の\
             どれかが落ちている）"
        );
    }
    // A/B の旧経路の**入口**（env の読み）は兄弟の関数 `legacy_1308` が持ち、
    // ドライバはそれを呼ぶだけ。ドライバ本体に現れる `TAKO_1308_LEGACY` の綴りは
    // A/B アームの**目印コメント**しかないので、全文一致だと入口を消しても
    // 目印だけで緑のままになる（#1609 で実測して分けた）
    let driver_code = code_view::without_comments(&body.text);
    assert!(
        driver_code.contains("legacy_1308()"),
        "ドライバが A/B の旧経路（legacy_1308）へ分岐していない"
    );
    assert!(
        code_view::without_comments(&src).contains("std::env::var(\"TAKO_1308_LEGACY\")"),
        "A/B の旧経路（TAKO_1308_LEGACY=1）が無いと、直したことを実測で示せない"
    );
}

/// 10/25 の再発（#1308）: 現場は POSIX で**ログインシェルを経由しない**。
///
/// 10/25 までは既定のログインシェル（開発者の rc を全部読む）を起こしてプロンプトを待ち、
/// 打ち込んでいた。詰まった機でこの起動（手元 0.6 秒）が予算 約 47 秒を使い切り、
/// CI（素のシェル）では緑のまま手元でだけ落ちた。描画のコマンドを直接起こす
/// `i1308_direct_options` へ戻っていること・A/B の旧経路（`TAKO_1308_TYPED=1`）の入口が
/// 残っていることを見る
#[test]
fn issue1308の現場はposixでログインシェルを経由しない() {
    let src = read_src();
    let body = bodies(&src)
        .into_iter()
        .find(|b| b.name == GUARDED)
        .unwrap_or_else(|| panic!("#1308 の現場（{GUARDED}）が切り出せていない"));
    let code = code_view::without_comments(&body.text);
    assert!(
        code.contains("i1308_direct_options("),
        "crates/tako-control/src/dispatch.rs:{} fn {GUARDED}: 描画のコマンドを直接起こす\
         `i1308_direct_options` を通していない（ログインシェルへ戻ると、開発者の rc の起動が\
         詰まった機で予算を使い切る = 10/25 の再発）",
        body.line
    );
    assert!(
        code.contains("typed_fixture_1308()"),
        "crates/tako-control/src/dispatch.rs:{} fn {GUARDED}: A/B の旧経路\
         （typed_fixture_1308）へ分岐していない",
        body.line
    );
    assert!(
        code_view::without_comments(&src).contains("std::env::var(\"TAKO_1308_TYPED\")"),
        "A/B の旧経路（TAKO_1308_TYPED=1）が無いと、直したことを実測で示せない"
    );
}

/// 「セッションがあるペイン」の fixture（#2415）
const SESSION_FIXTURE: &str = "i2415_session_options";
/// 素の既定を残してよい理由の目印（同じ行か直前の行に `tako:login-shell-ok <理由>`）
const LOGIN_SHELL_OK: &str = "tako:login-shell-ok";
/// 素の既定（command 無し = 開発者のログインシェル）
const BARE_DEFAULT: &str = "SpawnOptions::default()";

/// 実 PTY を張るテストのうち、**素の既定**を起こしている行を `file:line fn 名` で返す（#2415）。
///
/// `..SpawnOptions::default()`（構造体の残りを埋める形 = `command` を自分で渡している）は
/// 違反にしない。目印 `tako:login-shell-ok` の後ろに理由があれば通す
/// （ログインシェルそのものが主題のテスト用。今の dispatch.rs には無い）。
///
/// 既知の穴: `Default::default()` で作る形・`SpawnOptions { cwd, ..SpawnOptions::default() }`
/// のように `command` を渡さない構造体・素の既定を返す helper を通す形は見えない
/// （helper は理由を doc に書く。#1308 の打ち込む経路 `i1308_spawn_options` がその形）
fn login_shell_offenders(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for body in bodies(src) {
        if !body.text.contains("TerminalSession::spawn(") {
            continue;
        }
        let raw: Vec<&str> = body.text.lines().collect();
        // コメントと文字列を潰した眺め（行番号は保たれる）。説明文の綴りを拾わない
        let code = code_view::code_view(&body.text);
        for (n, line) in code.lines().enumerate() {
            let bare = line
                .match_indices(BARE_DEFAULT)
                .any(|(i, _)| !line[..i].ends_with(".."));
            if !bare {
                continue;
            }
            let justified = [Some(n), n.checked_sub(1)].into_iter().flatten().any(|k| {
                raw.get(k)
                    .and_then(|l| l.split_once(LOGIN_SHELL_OK))
                    .is_some_and(|(_, why)| !why.trim().is_empty())
            });
            if justified {
                continue;
            }
            out.push(at(
                &body,
                n,
                &format!("実 PTY に素の `{BARE_DEFAULT}`（開発者のログインシェル）を起こしている"),
            ));
        }
    }
    out
}

/// #2415: 実 PTY を張るテストは POSIX で**開発者のログインシェルを起こさない**。
///
/// 10/28 まで 3 本（`read_pane` の送達の顛末・`list` の `can_ssh`・`open_remote` の
/// target=pane）が `SpawnOptions::default()` を起こしていた。どれもプロンプトも打鍵も
/// 待たないが、`open_remote` は判定の前に `ssh` の場所をログインシェルへ尋ねる（上限 5 秒）
/// ので、その間に fixture の rc が進み、alt screen へ入る rc だと落ちていた（偽の rc で 20/20）。
/// 判定に要るのは「セッションがある」だけなので、直接起こす fixture へ寄せてある
#[test]
fn 実ptyを張るテストはposixで素のログインシェルを起こさない() {
    let src = read_src();
    let offenders = login_shell_offenders(&src);
    assert!(
        offenders.is_empty(),
        "実 PTY を張るテストが開発者のログインシェル（rc を全部読む）を起こしている。\n\
         判定がその瞬間の rc の進み具合に左右され、CI（素のシェル）では緑のまま手元でだけ落ちる\n\
         （#1308 / #2415）。セッションがあるだけでよいなら `{SESSION_FIXTURE}()`、絵が要るなら\n\
         `i1308_direct_options` のように描くコマンドを直接起こすこと。ログインシェルそのものが\n\
         主題なら、同じ行か直前の行に `{LOGIN_SHELL_OK} <理由>` を書く:\n  {}",
        offenders.join("\n  ")
    );
    // fixture 自身が POSIX で直接起こしていること（素の既定へ戻すと上の検査が素通りになる）
    let fixture = bodies(&src)
        .into_iter()
        .find(|b| b.name == SESSION_FIXTURE)
        .unwrap_or_else(|| panic!("#2415 の fixture（{SESSION_FIXTURE}）が切り出せていない"));
    let code = code_view::without_comments(&fixture.text);
    assert!(
        code.contains("program: \"/bin/sh\"") && code.contains("cfg!(windows)"),
        "crates/tako-control/src/dispatch.rs:{} fn {SESSION_FIXTURE}: POSIX で `/bin/sh -c` を\
         直接起こしていない（素の既定へ戻すとログインシェルを経由する = #2415）",
        fixture.line
    );
}

/// 上の番犬の検出力: **素の既定だけ**を名指しし、構造体の残りを埋める形・説明文・
/// 理由つきの目印は通す
#[test]
fn ログインシェルの番犬は素の既定だけを名指しする() {
    let src = [
        "mod tests {",
        "    fn 素の既定() {",
        "        let (s, _rx) = TerminalSession::spawn(80, 24, SpawnOptions::default()).unwrap();",
        "    }",
        "    fn 変数へ退避した既定() {",
        "        let opts = tako_core::terminal::SpawnOptions::default();",
        "        let (s, _rx) = TerminalSession::spawn(80, 24, opts).unwrap();",
        "    }",
        "    fn 直接起こす() {",
        "        // 説明文に SpawnOptions::default() と書いても拾わない",
        "        let (s, _rx) = TerminalSession::spawn(80, 24, SpawnOptions {",
        "            command: Some(cmd()),",
        "            ..SpawnOptions::default()",
        "        }).unwrap();",
        "        let msg = \"SpawnOptions::default()\";",
        "    }",
        "    fn 理由つき() {",
        "        // tako:login-shell-ok 主題が対話ログインシェルの履歴",
        "        let (s, _rx) = TerminalSession::spawn(80, 24, SpawnOptions::default()).unwrap();",
        "    }",
        "    fn 理由なしの目印() {",
        "        // tako:login-shell-ok",
        "        let (s, _rx) = TerminalSession::spawn(80, 24, SpawnOptions::default()).unwrap();",
        "    }",
        "    fn 実ptyを張らない() {",
        "        host.attach_session(pane, SpawnOptions::default());",
        "    }",
        "}",
    ]
    .join("\n");
    let got = login_shell_offenders(&src);
    let names: Vec<&str> = got
        .iter()
        .filter_map(|o| o.split(" fn ").nth(1))
        .filter_map(|o| o.split(':').next())
        .collect();
    assert_eq!(
        names,
        ["素の既定", "変数へ退避した既定", "理由なしの目印"],
        "名指しが期待とずれた: {got:#?}"
    );
    assert!(
        got[0].starts_with("crates/tako-control/src/dispatch.rs:3 "),
        "行番号がずれている: {}",
        got[0]
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    let src = read_src();
    let all = bodies(&src);
    let pty: Vec<&Body> = all
        .iter()
        .filter(|b| b.text.contains("TerminalSession::spawn("))
        .collect();
    assert!(
        pty.len() >= 2,
        "実 PTY を張るテストが {} 本しか見つからない = 切り出しが壊れている",
        pty.len()
    );
    assert!(
        all.iter().any(|b| b.name == GUARDED),
        "#1308 の現場（{GUARDED}）を見つけられていない"
    );
    assert!(
        all.iter().any(|b| b.name == DRIVER),
        "状態待ちのドライバ（{DRIVER}）を見つけられていない"
    );
    // 違反の印そのものがソースから消えていたら、番犬は永久に何も落とせない
    assert!(
        HAND_ROLLED.iter().any(|p| src.contains(p)),
        "自前の待ちの印（{HAND_ROLLED:?}）が 1 つもソースに無い = 検出力の確認ができない"
    );
}

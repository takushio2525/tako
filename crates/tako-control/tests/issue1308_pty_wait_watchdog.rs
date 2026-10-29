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
//! 対象は `crates/tako-control/src/dispatch.rs` と `crates/tako-control/src/test_write_isolation.rs`
//! （#2479）の**実 PTY を張るテストだけ**（`TerminalSession::spawn(` を持つ関数本体）。
//! 1 と 5 は両方のファイルを、2〜4 は dispatch.rs の #1308 の現場を、6 は
//! test_write_isolation.rs のシェル probe を見る。
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
//! 6. （#2479）対話ログインシェルの履歴（#1253）を見る子の probe `shell_probe` は、
//!    ログインシェルそのものが主題なので素の既定を理由つきで残すが、待ちは**尽きたら落ちる**
//!    ドライバ `wait_for_shell` を前提（プロンプト）と本題（打った行の実行）の 2 回通し、
//!    打つのは前提を待った後であること。旧実装は固定 20 秒の窓で待ち、尽きても検査せずに打ち、
//!    実行を見届けられなくても警告だけで返していた（= 履歴の番犬が何も見ないまま緑）

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

/// 走査するファイル（リポジトリルートからの相対）
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
/// #2479: 対話ログインシェルの履歴（#1253）を見る子の probe の置き場
const WRITE_ISOLATION: &str = "crates/tako-control/src/test_write_isolation.rs";
/// 実 PTY を張るテストの共通の検査（1 と 5）を当てるファイル
const TARGETS: [&str; 2] = [DISPATCH, WRITE_ISOLATION];

/// #2479 の現場（子のシェル probe）
const PROBE: &str = "shell_probe";
/// その状態待ちのドライバ（尽きたら panic する）
const PROBE_DRIVER: &str = "wait_for_shell";

fn target(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .join(rel)
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

fn read(rel: &str) -> String {
    std::fs::read_to_string(target(rel)).unwrap_or_else(|e| panic!("{rel} を読める: {e}"))
}

fn read_src() -> String {
    read(DISPATCH)
}

/// 違反の名指し（`file:line fn 名: 理由`）
fn at(rel: &str, body: &Body, offset: usize, why: &str) -> String {
    format!("{rel}:{} fn {}: {why}", body.line + offset, body.name)
}

/// 実 PTY を張るテストのうち、自前の固定窓で待っている行を `file:line fn 名` で返す
fn hand_rolled_offenders(rel: &str, src: &str) -> Vec<String> {
    let mut offenders: Vec<String> = Vec::new();
    for body in bodies(src) {
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
                    rel,
                    &body,
                    n,
                    &format!("実 PTY を自前の固定窓（`{pat}`）で待っている"),
                ));
            }
        }
    }
    offenders
}

#[test]
fn 実ptyを張るテストの待ちはstate_wait_budget由来である() {
    let offenders: Vec<String> = TARGETS
        .iter()
        .flat_map(|rel| hand_rolled_offenders(rel, &read(rel)))
        .collect();
    assert!(
        offenders.is_empty(),
        "固定窓は「窓が足りない」のか「シェルが起きていない」のかを診断から消す\n\
         （#1308 は起動前の PTY へ打ち込み、`#1200 の回帰`という無関係な原因で落ちていた）。\n\
         上限は `tako_core::wait_budget::state_wait_budget`（混み具合で伸ばすだけ・\n\
         4 倍で打ち切り）由来にするか、`{DRIVER}`（{WRITE_ISOLATION} は `{PROBE_DRIVER}`）を\n\
         通すこと:\n  {}",
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
            DISPATCH,
            &body,
            0,
            &format!("状態待ちのドライバ（{DRIVER}）を通していない"),
        ));
    }
    for (n, line) in code.lines().enumerate() {
        if let Some(pat) = HAND_ROLLED.iter().find(|p| line.contains(**p)) {
            offenders.push(at(
                DISPATCH,
                &body,
                n,
                &format!("自前の待ち（`{pat}`）が戻っている"),
            ));
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
/// （ログインシェルそのものが主題のテスト用。今は test_write_isolation.rs の `shell_probe`
/// = 対話ログインシェルの履歴（#1253）だけ。dispatch.rs には無い）。
///
/// 既知の穴: `Default::default()` で作る形・`SpawnOptions { cwd, ..SpawnOptions::default() }`
/// のように `command` を渡さない構造体・素の既定を返す helper を通す形は見えない
/// （helper は理由を doc に書く。#1308 の打ち込む経路 `i1308_spawn_options` がその形）
fn login_shell_offenders(rel: &str, src: &str) -> Vec<String> {
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
                rel,
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
    let offenders: Vec<String> = TARGETS
        .iter()
        .flat_map(|rel| login_shell_offenders(rel, &read(rel)))
        .collect();
    let src = read_src();
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
    let got = login_shell_offenders(DISPATCH, &src);
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

/// #2479 の現場（子のシェル probe）の違反を `file:line fn 名` で返す。
///
/// - 状態待ちのドライバ [`PROBE_DRIVER`] を**前提（プロンプト）と本題（打った行の実行）の
///   2 回**通していること（1 つの窓にまとめると「準備ができていない」と「本題が壊れた」が
///   同じ失敗に見える = #1308 と同じ理由）
/// - 打つ（`.write(`）のは**最初の待ちより後**であること（旧実装は尽きても検査せずに打っていた）
/// - ドライバが予算（`state_wait_budget`）・混み具合・素通り防止（`panic!`）・診断の見出しを持つこと
///
/// 自前の固定窓（`Instant::now() +` 等）の復活は共通の検査（[`hand_rolled_offenders`]）が落とす
fn probe_offenders(rel: &str, src: &str) -> Vec<String> {
    let all = bodies(src);
    let Some(probe) = all.iter().find(|b| b.name == PROBE) else {
        return vec![format!(
            "{rel}: #2479 の現場（fn {PROBE}）が切り出せていない"
        )];
    };
    let mut out = Vec::new();
    // コメントと文字列を潰した眺め（行番号は保たれる）
    let code = code_view::code_view(&probe.text);
    let call = format!("{PROBE_DRIVER}(");
    let waits: Vec<usize> = code
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(&call))
        .map(|(n, _)| n)
        .collect();
    if waits.len() < 2 {
        out.push(at(
            rel,
            probe,
            0,
            &format!(
                "状態待ちのドライバ（{PROBE_DRIVER}）を前提（プロンプト）と本題（打った行の実行）の \
                 2 回通していない（{} 回）",
                waits.len()
            ),
        ));
    }
    if let Some(n) = code.lines().position(|l| l.contains(".write(")) {
        if waits.first().is_none_or(|&w| n < w) {
            out.push(at(
                rel,
                probe,
                n,
                "前提（プロンプト）を待つ前に打っている = 尽きても検査せずに打つ形",
            ));
        }
    }
    let Some(driver) = all.iter().find(|b| b.name == PROBE_DRIVER) else {
        out.push(format!(
            "{rel}: 状態待ちのドライバ（fn {PROBE_DRIVER}）が切り出せていない"
        ));
        return out;
    };
    let driver_code = code_view::without_comments(&driver.text);
    for needed in [
        "state_wait_budget(",
        "machine_busy()",
        "panic!(",
        "TAKO_2479_WAIT",
    ] {
        if !driver_code.contains(needed) {
            out.push(at(
                rel,
                driver,
                0,
                &format!(
                    "`{needed}` が無い（予算・混み具合・素通り防止・診断のどれかが落ちている）"
                ),
            ));
        }
    }
    out
}

/// #2479: 対話ログインシェルの履歴（#1253）を見る子の probe は、**尽きたら落ちる**状態待ちを
/// 前提と本題の 2 回通し、前提を待ってから打つ。
///
/// 旧実装は固定 20 秒の窓でプロンプトを待ち、尽きても検査せずに打ち、実行を見届けられなくても
/// 警告だけで返していた。打った行が実行されなければ履歴は書かれないので、番犬
/// `検証プロセスはユーザーのシェル履歴へ書かない` が何も見ないまま緑になる（手元では、
/// プロンプトに載る長い一時 dir 名で打った行が折り返し、注入なしでも毎回この形だった）
#[test]
fn issue2479_シェルprobeは尽きたら落ちる状態待ちを通す() {
    let offenders = probe_offenders(WRITE_ISOLATION, &read(WRITE_ISOLATION));
    assert!(
        offenders.is_empty(),
        "シェル probe（#1253）の待ちが、尽きたら落ちる形になっていない（#2479）。\n\
         固定窓で待って尽きても次へ進むと、履歴の番犬が何も見ないまま緑になる。\n\
         `{PROBE_DRIVER}` を前提（プロンプト）と本題（打った行の実行）の 2 回通し、\n\
         打つのは前提を待った後にすること:\n  {}",
        offenders.join("\n  ")
    );
}

/// 上の番犬の検出力: **修正前の形**（固定窓で待って尽きても打つ）と、ドライバを 1 回しか
/// 通さない形・待つ前に打つ形・panic しないドライバを名指しし、直した形は通す
#[test]
fn シェルprobeの番犬は修正前の形を名指しする() {
    let driver = [
        "    fn wait_for_shell(what: &str) {",
        "        let budget = tako_core::wait_budget::state_wait_budget(BASE, machine_busy());",
        "        panic!(\"TAKO_2479_WAIT: 待っていたもの={what}\");",
        "    }",
    ];
    let fixed = [
        "mod tests {",
        "    fn shell_probe() {",
        "        let spawned = TerminalSession::spawn(80, 24, SpawnOptions::default());",
        "        wait_for_shell(\"プロンプト\", None, &screen, &ready);",
        "        let send = || session.write(line());",
        "        wait_for_shell(\"実行\", Some(&send), &screen, &probe_executed);",
        "    }",
    ];
    let src = |probe: &[&str], driver: &[&str]| -> String {
        let mut lines: Vec<&str> = Vec::new();
        lines.push("mod tests {");
        lines.extend_from_slice(&probe[1..]);
        lines.extend_from_slice(driver);
        lines.push("}");
        lines.join("\n")
    };
    assert_eq!(
        probe_offenders(WRITE_ISOLATION, &src(&fixed, &driver)),
        Vec::<String>::new(),
        "直した形を名指しした"
    );

    // 修正前の形（#2479 の前の main と同じ並び）
    let legacy = [
        "mod tests {",
        "    fn shell_probe() {",
        "        let ready = std::time::Instant::now() + std::time::Duration::from_secs(20);",
        "        while session.visible_lines().iter().all(|l| l.trim().is_empty()) {",
        "            std::thread::sleep(poll);",
        "        }",
        "        session.write(format!(\"echo {SHELL_PROBE_MARKER}\\r\").into_bytes());",
        "    }",
    ];
    let got = probe_offenders(WRITE_ISOLATION, &src(&legacy, &driver));
    assert!(
        got.iter()
            .any(|o| o.contains(":2 fn shell_probe: 状態待ちのドライバ"))
            && got.iter().any(|o| o.contains(":7 fn shell_probe: 前提")),
        "修正前の形を名指しできていない: {got:#?}"
    );

    // ドライバを 1 回しか通さない（前提と本題を 1 つの窓にまとめた）形
    let once = [fixed[0], fixed[1], fixed[2], fixed[4], fixed[5], fixed[6]];
    let got = probe_offenders(WRITE_ISOLATION, &src(&once, &driver));
    assert!(
        got.iter().any(|o| o.contains("2 回通していない（1 回）")),
        "1 回だけの待ちを名指しできていない: {got:#?}"
    );

    // 待つ前に打つ形
    let early = [
        fixed[0], fixed[1], fixed[2], fixed[4], fixed[3], fixed[5], fixed[6],
    ];
    let got = probe_offenders(WRITE_ISOLATION, &src(&early, &driver));
    assert!(
        got.iter().any(|o| o.contains(":4 fn shell_probe: 前提")),
        "待つ前に打つ形を名指しできていない: {got:#?}"
    );

    // 尽きても panic しないドライバ（警告だけで返す = 旧実装の終わり方）
    let quiet = [
        driver[0],
        driver[1],
        "        eprintln!(\"warn: シェル probe のマーカーが画面に出そろわなかった\");",
        driver[3],
    ];
    let got = probe_offenders(WRITE_ISOLATION, &src(&fixed, &quiet));
    assert!(
        got.iter().any(|o| o.contains("`panic!(` が無い"))
            && got.iter().any(|o| o.contains("`TAKO_2479_WAIT` が無い")),
        "panic しないドライバを名指しできていない: {got:#?}"
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    // #2479: test_write_isolation.rs のシェル probe が実 PTY を張る関数として見えている
    let probe_src = read(WRITE_ISOLATION);
    let probe_bodies = bodies(&probe_src);
    assert!(
        probe_bodies
            .iter()
            .any(|b| b.name == PROBE && b.text.contains("TerminalSession::spawn(")),
        "{WRITE_ISOLATION} の #2479 の現場（{PROBE}）を実 PTY を張る関数として見つけられていない"
    );
    assert!(
        probe_bodies.iter().any(|b| b.name == PROBE_DRIVER),
        "{WRITE_ISOLATION} の状態待ちのドライバ（{PROBE_DRIVER}）を見つけられていない"
    );
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

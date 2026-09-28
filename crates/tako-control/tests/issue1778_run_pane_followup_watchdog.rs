//! 番犬: 実行ペインの続き（#1778）
//!
//! ## 何を止めたいのか
//!
//! #1657（終了コードを側路で運ぶ）と #1662（`--wait` の上限・auto_close）の後に残った 4 つ:
//!
//! 1. `tako split --command` の失敗時の保持（#1031）が画面へ `__TAKO_EXIT=N` を出していた。
//!    終了コードは実行ペインと同じ側路で運び、画面には「終了コード N / Enter で閉じる」だけ
//! 2. 読む側（`run_pane_exit_code`）が側路を持つペインでも画面を読んでいたので、実行中の
//!    プログラム自身が `__TAKO_EXIT=7` を印字すると、終了を早め・偽の値で確定していた
//! 3. `tako run --wait` / `run-interactive --wait` の上限での打ち切りが exit 1 で、
//!    コマンドの失敗（exit 1）と見分けられなかった → 124（`timeout(1)` と同じ）
//! 4. auto_close で閉じた実行ペインのペインログのマーカーが `close:internal`
//!    （`--wait` 経由なら `close:dispatch(cli)`）で、手で閉じたものと見分けられなかった
//!    → 引き金に依らず `close:auto`
//!
//! 戻る形はそれぞれ「split の腕が側路を渡さない / ペインに控えない」
//! 「側路を読んで外れたら画面へ落ちる形に戻す」「打ち切りを Err（= exit 1）へ戻す」
//! 「auto_close が呼び手の発生源を受け取る形へ戻す」。**落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! - 偽の確定をしない / split の失敗が案内だけで側路から読める（実 PTY）:
//!   `dispatch::tests::issue1778_*`
//! - 保持のスクリプトが側路へ書き、画面にマーカーを出さない（実 `/bin/sh`）:
//!   `platform::shell::tests_1031::失敗したコマンドは側路へ終了コードを書き画面には案内だけを出す`
//! - `close:auto`: `pane_log::tests::クローズマーカーに発生源が入る` /
//!   `dispatch::tests::issue1662_auto_closeは聞きに来なくても閉じて結末を控える`
//! - 124: `tako-cli` の `tests::waitの終了コードは打ち切りを124でコマンドの失敗と分ける`
//! - 隔離 GUI の実経路（修正前のバイナリでも同じスクリプトが走り、症状は FAIL で出る）:
//!   `scripts/test-run-pane-followup-1778.sh`
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `run_pane_exit_code` の `return tako_core::run_pane::read(file);` を
//!    `if let Some(code) = tako_core::run_pane::read(file) { return Some(code); }` へ（画面へ落ちる）/
//! ② Split の腕の `hold_on_failure_command(` の `exit_file.as_deref(),` を `None,` へ /
//! ③ `wait_run_pane` の `Ok(WaitEnd::TimedOut(…))` を `Err(…)` へ（打ち切りが exit 1 に戻る）/
//! ④ `auto_close_run_pane` に `origin: CloseOrigin` を足して `detach_session(pane, origin, None)` へ

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const PANE_LOG: &str = "crates/tako-core/src/pane_log.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*')
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで。コメント行は除く）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が無い"));
    let body: Vec<(usize, String)> = (start..=end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

/// `from` を含む行から `to` を含む行の手前まで（コメント行は除く）
fn region(rel: &str, src: &str, from: &str, to: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(from))
        .unwrap_or_else(|| panic!("{rel}: 目印 {from:?} が消えている"));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.contains(to))
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}:{}: {from:?} の終わり {to:?} が無い", start + 1));
    let body = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    (body, start + 1)
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------- 1 / 2. 側路を持つペインは画面を読まない

/// `run_pane_exit_code` の違反（本体を渡す）。側路を持つペインでは側路の読みを
/// **そのまま返す**（外れても画面へ落ちない）こと。側路の読みが `return` 以外の形で
/// 書かれていたら、その行を名指す
fn reader_violations(rel: &str, body: &[(usize, String)], at: usize) -> Vec<String> {
    let mut out = Vec::new();
    let guard = body
        .iter()
        .position(|(_, l)| l.contains(".exit_file())") && l.trim_start().starts_with("if let"));
    let screen = body
        .iter()
        .position(|(_, l)| l.contains("find_exit_marker("));
    match (guard, screen) {
        (Some(g), Some(s)) if g < s => {
            let next = &body[g + 1];
            if next.1.trim() != "return tako_core::run_pane::read(file);" {
                out.push(format!(
                    "{rel}:{}: 側路を持つペインで側路の読みをそのまま返していない（{}。外れると画面の\
                     マーカーへ落ち、プログラムが印字した `__TAKO_EXIT=N` で偽の確定をする = #1778 の症状）",
                    next.0,
                    next.1.trim()
                ));
            }
        }
        _ => out.push(format!(
            "{rel}:{at}: run_pane_exit_code が画面のマーカーより前に側路の有無で分けていない\
             （guard={guard:?} screen={screen:?}。#1778）"
        )),
    }
    for (n, l) in body {
        if l.contains("run_pane::read") && !l.contains("return tako_core::run_pane::read(file);") {
            out.push(format!(
                "{rel}:{n}: 側路の読みが返す形の外にある（{}）",
                l.trim()
            ));
        }
    }
    out
}

#[test]
fn 側路を持つペインでは画面のマーカーを読まない() {
    let src = production(DISPATCH);
    let (body, at) = fn_body(DISPATCH, &src, "pub fn run_pane_exit_code(");
    let bad = reader_violations(DISPATCH, &body, at);
    assert!(
        bad.is_empty(),
        "run_pane_exit_code が側路を持つペインでも画面を読んでいる（#1778）:\n{}",
        bad.join("\n")
    );
}

#[test]
fn i1778_注入_側路の外れで画面へ落ちる形を名指しで落とせる() {
    // 注入 ①: #1778 以前の「側路 → 画面」の順（側路が外れたら画面へ落ちる）
    let body = vec![
        (
            10,
            "pub fn run_pane_exit_code(host: &dyn ControlHost, pane: PaneId) -> Option<i32> {"
                .to_string(),
        ),
        (
            11,
            "    if let Some(file) = target.and_then(|p| p.exit_file()) {".to_string(),
        ),
        (
            12,
            "        if let Some(code) = tako_core::run_pane::read(file) {".to_string(),
        ),
        (13, "            return Some(code);".to_string()),
        (14, "        }".to_string()),
        (15, "    }".to_string()),
        (16, "    find_exit_marker(&rows)".to_string()),
        (17, "}".to_string()),
    ];
    let got = reader_violations(DISPATCH, &body, 10);
    assert!(
        got.iter().any(|l| l.contains(":12:")),
        "注入を拾えていない: {got:?}"
    );
}

// --------------------------------------------- 1. split --command の保持は側路で運ぶ

/// Split の腕の違反（腕の本体を渡す）。側路を用意してペインに控え、保持の包みへ渡すこと
fn split_violations(rel: &str, arm: &[(usize, String)], at: usize) -> Vec<String> {
    let mut out = Vec::new();
    let text = joined(arm);
    for (needle, why) in [
        (
            "prepare_exit_file(host.workspace(), new_id)",
            "側路を用意していない",
        ),
        (
            "new_pane.set_exit_file(",
            "側路をペインに控えていない（読む側 run_pane_exit_code が引けない）",
        ),
    ] {
        if !text.contains(needle) {
            out.push(format!(
                "{rel}:{at}: Split の腕が{why}（{needle:?} が無い。#1778）"
            ));
        }
    }
    // `hold_on_failure_command(` の呼び出しの引数に側路が居る
    match arm
        .iter()
        .position(|(_, l)| l.contains("hold_on_failure_command("))
    {
        Some(call) => {
            let close = arm[call..]
                .iter()
                .position(|(_, l)| l.trim_start().starts_with(')'))
                .map(|i| call + i)
                .unwrap_or(arm.len() - 1);
            let args = &arm[call..=close];
            if !args
                .iter()
                .any(|(_, l)| l.trim() == "exit_file.as_deref(),")
            {
                let text = args
                    .iter()
                    .map(|(_, l)| l.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push(format!(
                    "{rel}:{}: 保持の包みへ側路を渡していない（{text}。画面へ `__TAKO_EXIT=N` が戻る = #1778 の症状）",
                    arm[call].0
                ));
            }
        }
        None => out.push(format!(
            "{rel}:{at}: Split の腕が失敗時の保持（hold_on_failure_command）を通っていない（#1031）"
        )),
    }
    out
}

#[test]
fn splitの失敗時の保持は側路で運ぶ() {
    let src = production(DISPATCH);
    let (arm, at) = region(
        DISPATCH,
        &src,
        "        Request::Split {",
        "        Request::Close {",
    );
    let bad = split_violations(DISPATCH, &arm, at);
    assert!(
        bad.is_empty(),
        "split --command の失敗時の保持が側路で終了コードを運んでいない（#1778）:\n{}",
        bad.join("\n")
    );
    // 実行ペインと同じ用意の 1 実装を通す（置き場・掃除の規則を 2 つ持たない）
    let (body, at) = fn_body(DISPATCH, &src, "fn spawn_command_pane(");
    assert!(
        joined(&body).contains("prepare_exit_file(host.workspace(), new_id)"),
        "{DISPATCH}:{at}: 実行ペインが側路の用意の 1 実装（prepare_exit_file）を通っていない"
    );
}

#[test]
fn i1778_注入_保持へ側路を渡さない形を名指しで落とせる() {
    // 注入 ②: `exit_file.as_deref()` を `None` にした形
    let arm = vec![
        (20, "            let exit_file = command".to_string()),
        (
            21,
            "                .and_then(|_| prepare_exit_file(host.workspace(), new_id));"
                .to_string(),
        ),
        (
            22,
            "            new_pane.set_exit_file(exit_file.clone());".to_string(),
        ),
        (
            23,
            "                        tako_core::platform::shell::hold_on_failure_command("
                .to_string(),
        ),
        (24, "                            c,".to_string()),
        (
            25,
            "                            EXIT_MARKER_PREFIX,".to_string(),
        ),
        (26, "                            None,".to_string()),
        (27, "                        )".to_string()),
    ];
    let got = split_violations(DISPATCH, &arm, 20);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains(":23:"), "行番号が合っていない: {got:?}");
    assert!(got[0].contains("None,"), "何を渡したかが読めない: {got:?}");
}

// --------------------------------------------- 3. --wait の打ち切りは 124

/// `wait_run_pane` の違反（本体を渡す）。打ち切りは `Ok(WaitEnd::TimedOut(…))` で返し、
/// `Err`（= 終了コード 1 = コマンドの失敗と同じ）へ畳まない
fn wait_violations(rel: &str, body: &[(usize, String)], at: usize) -> Vec<String> {
    let mut out = Vec::new();
    match body
        .iter()
        .position(|(_, l)| l.contains("Polled::TimedOut"))
    {
        Some(arm) => {
            let ret = body[arm + 1..]
                .iter()
                .find(|(_, l)| l.contains("Ok(") || l.contains("Err("));
            match ret {
                Some((_, l)) if l.contains("Ok(WaitEnd::TimedOut(") => {}
                Some((n, l)) => out.push(format!(
                    "{rel}:{n}: 上限での打ち切りを WaitEnd::TimedOut で返していない（{}。exit 1 に戻り、\
                     コマンドの失敗と見分けられない = #1778 の症状）",
                    l.trim()
                )),
                None => out.push(format!(
                    "{rel}:{}: 打ち切りの腕が何も返していない",
                    body[arm].0
                )),
            }
        }
        None => out.push(format!(
            "{rel}:{at}: wait_run_pane が上限での打ち切り（Polled::TimedOut）を扱っていない"
        )),
    }
    out
}

#[test]
fn waitの打ち切りは124で終わる() {
    let src = production(CLI);
    let (body, at) = fn_body(CLI, &src, "fn wait_run_pane(");
    let bad = wait_violations(CLI, &body, at);
    assert!(
        bad.is_empty(),
        "`--wait` の打ち切りがコマンドの失敗と同じ終了コードになる（#1778）:\n{}",
        bad.join("\n")
    );
    // 124 は 1 か所で決まる（`timeout(1)` と同じ値）
    assert!(
        src.lines()
            .any(|l| l.trim() == "const RUN_WAIT_TIMED_OUT_EXIT: u8 = 124;"),
        "{CLI}: 打ち切りの終了コード RUN_WAIT_TIMED_OUT_EXIT = 124 が無い"
    );
    let (body, at) = fn_body(CLI, &src, "    fn exit_code(&self) -> u8 {");
    assert!(
        joined(&body).contains("WaitEnd::TimedOut(_) => RUN_WAIT_TIMED_OUT_EXIT,"),
        "{CLI}:{at}: WaitEnd::exit_code が打ち切りを RUN_WAIT_TIMED_OUT_EXIT へ写していない"
    );
    // 2 つの `--wait` はどちらも結末を終了コードへ写す 1 本を通る
    let (arms, at) = region(
        CLI,
        &src,
        "Command::RunInteractive(ref args) if args.wait =>",
        "Command::Run(ref args) if args.list =>",
    );
    let text = joined(&arms);
    for needle in [
        "return wait_exit_code(run_interactive_wait(&cli.command));",
        "return wait_exit_code(run_wait(&cli.command))",
    ] {
        assert!(
            text.contains(needle),
            "{CLI}:{at}: `--wait` の腕が {needle:?} を通っていない（終了コードが 1 に畳まれる）"
        );
    }
}

#[test]
fn i1778_注入_打ち切りをerrへ戻した形を名指しで落とせる() {
    // 注入 ③: #1778 以前の「打ち切りは Err（= exit 1）」
    let body = vec![
        (
            30,
            "fn wait_run_pane(pane: u64) -> Result<WaitEnd, String> {".to_string(),
        ),
        (31, "        Polled::TimedOut { waited } => {".to_string()),
        (
            32,
            "            println!(\"{}\", pretty_json(&run_wait_timed_out(pane, waited, budget)));"
                .to_string(),
        ),
        (
            33,
            "            Err(run_wait_timed_out_message(pane, budget))".to_string(),
        ),
        (34, "        }".to_string()),
    ];
    let got = wait_violations(CLI, &body, 30);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains(":33:"), "行番号が合っていない: {got:?}");
}

// --------------------------------------------- 4. auto_close は close:auto

/// `auto_close_run_pane` の違反（本体を渡す）。発生源を呼び手から受け取らず、
/// 閉じるときは常に `CloseOrigin::AutoClose` で畳むこと
fn auto_close_violations(rel: &str, body: &[(usize, String)], at: usize) -> Vec<String> {
    let mut out = Vec::new();
    let (sig_no, sig) = &body[0];
    if sig.contains("origin") {
        out.push(format!(
            "{rel}:{sig_no}: auto_close_run_pane が発生源を呼び手から受け取っている（{}。\
             GUI は close:internal、`--wait` は close:dispatch(cli) になり、手で閉じたものと\
             見分けられない = #1778 の症状）",
            sig.trim()
        ));
    }
    let detach: Vec<&(usize, String)> = body
        .iter()
        .filter(|(_, l)| l.contains("detach_session("))
        .collect();
    if detach.is_empty() {
        out.push(format!(
            "{rel}:{at}: auto_close_run_pane がセッションを畳んでいない"
        ));
    }
    for (n, l) in detach {
        if !l.contains("CloseOrigin::AutoClose") {
            out.push(format!(
                "{rel}:{n}: auto_close の発生源が close:auto ではない（{}）",
                l.trim()
            ));
        }
    }
    out
}

#[test]
fn auto_closeで閉じたペインはclose_autoで記録する() {
    let src = production(DISPATCH);
    let (body, at) = fn_body(DISPATCH, &src, "pub fn auto_close_run_pane(");
    let bad = auto_close_violations(DISPATCH, &body, at);
    assert!(
        bad.is_empty(),
        "auto_close のペインログのマーカーが手で閉じたものと見分けられない（#1778）:\n{}",
        bad.join("\n")
    );
    // 綴りは pane_log の 1 か所（読み手 close_marker_reason と同じ語彙）
    let log = production(PANE_LOG);
    assert!(
        log.lines()
            .any(|l| l.trim() == "CloseOrigin::AutoClose => \"close:auto\","),
        "{PANE_LOG}: CloseOrigin::AutoClose の綴り close:auto が無い"
    );
}

#[test]
fn i1778_注入_発生源を受け取るauto_closeを名指しで落とせる() {
    // 注入 ④: #1778 以前の形（呼び手の発生源で畳む）
    let body = vec![
        (
            40,
            "pub fn auto_close_run_pane(host: &mut dyn ControlHost, pane: PaneId, origin: CloseOrigin) -> bool {"
                .to_string(),
        ),
        (
            41,
            "    let Some(tab) = host.workspace().find_tab_of_pane(pane) else {".to_string(),
        ),
        (42, "    host.detach_session(pane, origin, None);".to_string()),
        (43, "}".to_string()),
    ];
    let got = auto_close_violations(DISPATCH, &body, 40);
    assert!(
        got.iter().any(|l| l.contains(":40:")) && got.iter().any(|l| l.contains(":42:")),
        "注入を拾えていない: {got:?}"
    );
}

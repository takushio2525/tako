//! 番犬: `--wait` の上限 / GUI 側の auto_close / 実行前の保存（#1662）
//!
//! ## 何を止めたいのか
//!
//! #1662 以前の Code Runner には 3 つの穴があった:
//!
//! 1. `tako run --wait`（と `tako run-interactive --wait`）が **2 秒刻みの無限ループ**で、
//!    サーバーや入力待ちを走らせると CLI が永久に返らなかった
//!    （`.agent/conventions.md`「外部コマンドを待つときは上限を持つ」違反）
//! 2. `--auto-close success` の処理が `RunInteractiveStatus` の中にしか無く、`--wait` 無しだと
//!    誰も聞きに来ないので**閉じなかった**
//! 3. GUI の再生ボタンだけが実行前に保存し、CLI / MCP の `tako run` は保存しなかった
//!    （AI が `tako_run` を呼ぶと**ディスク上の古い内容**が走る）
//!
//! 直したあとに戻る形はそれぞれ「聞き直しに上限を渡さない / 無限ループを書き戻す」
//! 「GUI の終了検知から auto_close を外す / 判定を別の場所へ書く」
//! 「`Run` の腕から保存を外す / 再生ボタンが自前で保存する形へ戻す」。
//! **落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! - 上限・上限ちょうど・間隔の頭打ち: `tako-core` の `probe::tests`（時計を差し替える）
//! - 閉じる / 閉じない / 控え / タブ最後の 1 枚 / 保存するプレビューの選び方:
//!   `dispatch::tests::issue1662_*`
//! - 隔離 GUI の実経路（CLI と MCP が同じ応答・未保存の編集が走る・auto_close）:
//!   `scripts/test-run-wait-save-1662.sh`
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `wait_run_pane` の `let budget = run_wait_timeout();` を `Duration::MAX` へ（上限を外す）/
//! ② `Run` の腕の `save_previews_before_run(host, &resolved, target)?` を `Vec::new()` へ
//!    （保存を外す）/
//! ③ 定期更新の `app.auto_close_finished_run_panes()` の塊と `on_term_event` の
//!    `self.auto_close_finished_run_pane(pane_id)` を消す（auto_close を GUI 側から外す）

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const CLI: &str = "crates/tako-cli/src/main.rs";
const PROBE: &str = "crates/tako-core/src/probe.rs";
const RUN_PANE: &str = "crates/tako-core/src/run_pane.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";

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

/// `needle` を含む最初の行の行番号（無ければ `None`）
fn line_of(body: &[(usize, String)], needle: &str) -> Option<usize> {
    body.iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

// --------------------------------------------- 1. --wait は上限つきの 1 実装

/// `--wait` の待ちの違反（`wait_run_pane` の本体を渡す）。上限を `run_wait_timeout()`
/// から取り、`poll_with_timeout` へ渡し、自前の `loop` / `sleep` を持たないこと
fn wait_violations(rel: &str, body: &[(usize, String)], at: usize) -> Vec<String> {
    let mut out = Vec::new();
    let budget = body
        .iter()
        .find(|(_, l)| l.contains("let budget ="))
        .map(|(n, l)| (*n, l.trim().to_string()));
    match budget {
        Some((_, l)) if l == "let budget = run_wait_timeout();" => {}
        Some((n, l)) => out.push(format!(
            "{rel}:{n}: 上限を `run_wait_timeout()` から取っていない（{l}。env 0 が既定へ落ちず、\
             上限が外れうる = #1662 の症状）"
        )),
        None => out.push(format!(
            "{rel}:{at}: wait_run_pane に上限（let budget）が無い"
        )),
    }
    if !joined(body).contains("poll_with_timeout(budget, RUN_WAIT_POLL,") {
        out.push(format!(
            "{rel}:{at}: wait_run_pane が上限つきの聞き直し（probe::poll_with_timeout）を通っていない"
        ));
    }
    for (n, l) in body {
        if l.contains("loop {") || l.contains("thread::sleep") {
            out.push(format!(
                "{rel}:{n}: 自前の聞き直し（{}）= 上限の無い待ちへ戻る形",
                l.trim()
            ));
        }
    }
    out
}

#[test]
fn waitは上限つきの聞き直しの1実装を通す() {
    let src = production(CLI);
    let (body, at) = fn_body(CLI, &src, "fn wait_run_pane(");
    let bad = wait_violations(CLI, &body, at);
    assert!(
        bad.is_empty(),
        "`--wait` の待ちに上限が無い（#1662 / conventions「外部コマンドを待つときは上限を持つ」）:\n{}",
        bad.join("\n")
    );
    // run / run-interactive の --wait はどちらもこの 1 本へ寄っている
    for sig in ["fn run_wait(", "fn run_interactive_wait("] {
        let (body, at) = fn_body(CLI, &src, sig);
        assert!(
            joined(&body).contains("wait_run_pane(pane)"),
            "{CLI}:{at}: {sig:?} が wait_run_pane を通っていない（上限の無い待ちが並ぶ）"
        );
        let own: Vec<String> = body
            .iter()
            .filter(|(_, l)| l.contains("loop {") || l.contains("thread::sleep"))
            .map(|(n, l)| format!("{CLI}:{n}: {}", l.trim()))
            .collect();
        assert!(
            own.is_empty(),
            "{sig:?} が自前で聞き直している（上限の無い待ちへ戻る形。#1662）:\n{}",
            own.join("\n")
        );
    }
    // 聞き直しの本体は上限で打ち切り、眠りを残り時間で頭打ちにする
    let psrc = production(PROBE);
    let (body, at) = fn_body(PROBE, &psrc, "fn poll_with_clock<");
    let text = joined(&body);
    for needle in [
        "if now >= budget {",
        "return Ok(Polled::TimedOut",
        "interval.min(budget - now)",
    ] {
        assert!(
            text.contains(needle),
            "{PROBE}:{at}: poll_with_clock に {needle:?} が無い（上限で打ち切らない / 踏み越える）"
        );
    }
    // 上限は env で変えられ、0 / 不正は既定へ落ちる（外せない）
    let (body, at) = fn_body(PROBE, &psrc, "pub fn run_wait_timeout(");
    let text = joined(&body);
    assert!(
        text.contains("parse_timeout_secs(") && text.contains("RUN_WAIT_TIMEOUT_ENV"),
        "{PROBE}:{at}: run_wait_timeout が env を parse_timeout_secs で解釈していない（0 = 無制限を作りうる）"
    );
}

#[test]
fn i1662_注入_上限を外した待ちを名指しで落とせる() {
    // 注入 ①: 上限を外して自前の loop へ戻した形
    let body = vec![
        (
            10,
            "fn wait_run_pane(pane: u64) -> Result<(), String> {".to_string(),
        ),
        (11, "    let budget = std::time::Duration::MAX;".to_string()),
        (12, "    loop {".to_string()),
        (13, "        std::thread::sleep(RUN_WAIT_POLL);".to_string()),
        (14, "    }".to_string()),
    ];
    let got = wait_violations(CLI, &body, 10);
    assert!(
        got.iter().any(|l| l.contains(":11:")),
        "上限の差し替えを拾えていない: {got:?}"
    );
    assert!(
        got.iter().any(|l| l.contains(":12:")) && got.iter().any(|l| l.contains(":13:")),
        "自前の聞き直しを拾えていない: {got:?}"
    );
    assert!(
        got.iter().any(|l| l.contains("poll_with_timeout")),
        "聞き直しの 1 実装を通っていないことを拾えていない: {got:?}"
    );
}

// --------------------------------------------- 2. auto_close は GUI の終了検知から

#[test]
fn auto_closeはguiの終了検知から効き判定はdispatchの1実装() {
    let app = production(APP);
    // 出力のたび（終わった瞬間）
    let (body, at) = fn_body(APP, &app, "    fn on_term_event(");
    let settle = line_of(
        &body,
        "tako_control::dispatch::settle_run_pane(self, pane_id)",
    );
    let close = line_of(&body, "self.auto_close_finished_run_pane(pane_id)");
    match (settle, close) {
        (Some(s), Some(c)) if s < c => {}
        _ => panic!(
            "{APP}:{at}: on_term_event が終了を確定した直後に auto_close を効かせていない\
             （`--wait` 無しの `--auto-close success` が閉じない = #1662 の症状。\
             settle={settle:?} close={close:?}）"
        ),
    }
    // 2 秒ごとの定期更新（出力の無い終わり方・読み取りが先に確定させたぶん）
    let (periodic, pat) = region(
        APP,
        &app,
        "periodic_prep:run_panes",
        "periodic_prep:sleep_guard",
    );
    let settle = line_of(&periodic, "tako_control::dispatch::settle_run_panes(app)");
    let close = line_of(&periodic, "app.auto_close_finished_run_panes()");
    match (settle, close) {
        (Some(s), Some(c)) if s < c => {}
        _ => panic!(
            "{APP}:{pat}: 定期更新が settle_run_panes のあとに auto_close を効かせていない\
             （settle={settle:?} close={close:?}。#1662）"
        ),
    }
    // GUI の口は dispatch の 1 実装へ委ねるだけ
    for (sig, needle) in [
        (
            "fn auto_close_finished_run_pane(",
            "tako_control::dispatch::auto_close_run_pane(self, pane,",
        ),
        (
            "fn auto_close_finished_run_panes(",
            "tako_control::dispatch::auto_close_run_panes(self,",
        ),
    ] {
        let (body, at) = fn_body(APP, &app, sig);
        assert!(
            joined(&body).contains(needle),
            "{APP}:{at}: {sig:?} が dispatch の {needle:?} を通っていない"
        );
    }
    // GUI に自前の判定を書かない（方針の綴りを GUI で読み直すと規則が 2 つ並ぶ）
    let own: Vec<String> = app
        .lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .filter(|(_, l)| l.contains(".auto_close()") || l.contains(".wants_close()"))
        .map(|(i, l)| format!("{APP}:{}: {}", i + 1, l.trim()))
        .collect();
    assert!(
        own.is_empty(),
        "GUI が auto_close を自前で判定している（dispatch::auto_close_run_pane の 1 実装へ寄せる）:\n{}",
        own.join("\n")
    );

    let src = production(DISPATCH);
    // 閉じる本体は方針の判定（wants_close）・側路の片付け・結末の控えを持つ
    let (body, at) = fn_body(DISPATCH, &src, "pub fn auto_close_run_pane(");
    let text = joined(&body);
    for needle in [
        ".wants_close()",
        "run_pane::discard(",
        "closed_runs_mut()",
        "host.detach_session(pane, origin, None)",
    ] {
        assert!(
            text.contains(needle),
            "{DISPATCH}:{at}: auto_close_run_pane に {needle:?} が無い（#1662）"
        );
    }
    // RunInteractiveStatus は同じ 1 実装で閉じ、閉じたあとは控えから結末を返す
    let (arm, at) = region(
        DISPATCH,
        &src,
        "Request::RunInteractiveStatus { pane, no_wait: _ } => {",
        "// --- Code Runner (FR-3.18, #453) ---",
    );
    let text = joined(&arm);
    assert!(
        text.contains("auto_close_run_pane(host, target, close_origin_of(origin))"),
        "{DISPATCH}:{at}: RunInteractiveStatus が auto_close_run_pane を通っていない（閉じ方が 2 つに割れる）"
    );
    assert!(
        text.contains("closed_runs().get(pane_id)"),
        "{DISPATCH}:{at}: RunInteractiveStatus が閉じた実行の控えを見ていない\
         （GUI が先に閉じると `--wait` が「ペインが無い」で落ちる）"
    );
    let literal: Vec<String> = arm
        .iter()
        .filter(|(_, l)| l.contains("\"always\"") || l.contains("\"success\""))
        .map(|(n, l)| format!("{DISPATCH}:{n}: {}", l.trim()))
        .collect();
    assert!(
        literal.is_empty(),
        "RunInteractiveStatus に auto_close の判定が書き戻っている（判定は run_pane::wants_close）:\n{}",
        literal.join("\n")
    );
    // 判定の本体
    let rsrc = production(RUN_PANE);
    let (body, at) = fn_body(RUN_PANE, &rsrc, "pub fn wants_close(");
    assert!(
        joined(&body).contains("\"success\" => code == 0"),
        "{RUN_PANE}:{at}: wants_close の success が「0 のときだけ」になっていない"
    );
}

// --------------------------------------------- 3. 実行前の保存は Run の 1 実装

/// `Run` の腕の違反（保存が無い / 先頭を読んだあとに保存している）
fn save_violations(rel: &str, arm: &[(usize, String)], at: usize) -> Vec<String> {
    let save = line_of(arm, "save_previews_before_run(host, &resolved, target)?");
    let head = line_of(arm, "read_file_head(&resolved)?");
    match (save, head) {
        (Some(s), Some(h)) if s < h => Vec::new(),
        (Some(s), Some(h)) => vec![format!(
            "{rel}:{s}: 保存が先頭の読み取り（{rel}:{h}）より後ろ（書き換えた `tako:run:` 宣言が効かない）"
        )],
        (None, _) => vec![format!(
            "{rel}:{at}: Run の腕が走らせる前に未保存の編集を保存していない\
             （AI の `tako_run` でディスク上の古い内容が走る = #1662 の症状）"
        )],
        (_, None) => vec![format!("{rel}:{at}: Run の腕に read_file_head が無い（範囲取りが壊れている）")],
    }
}

#[test]
fn runは走らせる前に未保存の編集を保存し再生ボタンも同じ口を通る() {
    let src = production(DISPATCH);
    let (arm, at) = region(
        DISPATCH,
        &src,
        "// --- Code Runner (FR-3.18, #453) ---",
        "        Request::RunResolve {",
    );
    let bad = save_violations(DISPATCH, &arm, at);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    // 保存できなければ走らせない（`?` で返る）
    let (body, at) = fn_body(DISPATCH, &src, "fn save_previews_before_run(");
    let text = joined(&body);
    assert!(
        text.contains("host.save_preview(pane).map_err(") && text.contains("})?;"),
        "{DISPATCH}:{at}: save_previews_before_run が保存の失敗で止まっていない（古い内容を走らせる）"
    );
    assert!(
        text.contains("preview_edit_state(pane)") && text.contains("p == file"),
        "{DISPATCH}:{at}: 保存するプレビューを「そのファイル + 未保存」で選んでいない"
    );
    // 再生ボタンは自前で保存しない（保存の口が 2 つに割れると CLI / MCP だけ古い内容が走る）
    let render = production(RENDER);
    let (body, at) = fn_body(RENDER, &render, "pub(crate) fn run_preview_file(");
    let own: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.contains("save_preview") || l.contains(".save()"))
        .map(|(n, l)| format!("{RENDER}:{n}: {}", l.trim()))
        .collect();
    assert!(
        own.is_empty(),
        "再生ボタンが自前で保存している（dispatch `Run` の save_previews_before_run の 1 実装へ寄せる）:\n{}",
        own.join("\n")
    );
    assert!(
        joined(&body).contains("tako_control::protocol::Request::Run {"),
        "{RENDER}:{at}: 再生ボタンが dispatch `Run` を通っていない"
    );
}

#[test]
fn i1662_注入_保存を外したrunを名指しで落とせる() {
    // 注入 ②: 保存を外した形 / 先頭を読んだあとへずらした形
    let dropped = vec![
        (
            20,
            "            let saved_panes: Vec<PaneId> = Vec::new();".to_string(),
        ),
        (
            21,
            "            let head = read_file_head(&resolved)?;".to_string(),
        ),
    ];
    let got = save_violations(DISPATCH, &dropped, 19);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains(":19:") && got[0].contains("保存していない"),
        "{got:?}"
    );
    let late = vec![
        (
            30,
            "            let head = read_file_head(&resolved)?;".to_string(),
        ),
        (
            31,
            "            let saved_panes = save_previews_before_run(host, &resolved, target)?;"
                .to_string(),
        ),
    ];
    let got = save_violations(DISPATCH, &late, 29);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains(":31:"), "{got:?}");
}

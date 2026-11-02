//! 番犬: 引き継ぎで master が互いを閉じ合わない・届いた送達を届いていないと言わない（Issue #2330）
//!
//! ## なぜ止めるのか
//!
//! 2026-10-18、引き継ぎの前任と後任が 0.2 秒差で互いのペインを `force: true` で閉じ、
//! タブから master が消えた（約 2 日、誰も気付かなかった）。重なった原因は 2 つ:
//!
//! 1. 送達の偽陰性: 狭いペインの claude は入力欄の高さに上限があり、長い本文の先頭と `❯` の
//!    行が画面から流れる。送達フローは画面だけで判定して `gave_up / paste_not_reflected` を返し、
//!    前任は後任に届いていないと読んで閉じ直した（後任は受け取って手順を進めていた）
//! 2. 守りが無い: master のペインを閉じる close に、「受領済みの後任」「タブの最後の master」を
//!    止めるものが無かった
//!
//! 判断の正本は `tako_core::handoff_guard`（close）と `tako_control::delivery_receipt`（受領）の
//! 単体テストが持つ。ここは**経路が判断を通っていること**を見張る。どれか 1 か所が外れると、
//! 判断は正しくても実経路では効かない（10/18 の再来）。
//!
//! ## 何を違反とするか（違反は file:line で名指す）
//!
//! - ① 送達フロー（tako-app `drive_prompt_flows`）の `VerifySubmitted` で諦める枝は、
//!   すべて `hold_for_receipt` を通ってから諦める（画面で確かめられなかったときは会話記録を見る）
//! - ② keys 経路の最初の貼り付けの前に `ReceiptProbe::capture` で材料を控える
//!   （貼り付けの後に控えると、送信した本文がすでに会話記録にあり追記分に入らない）
//! - ③ 送達の決着（`finish_flow`）は引き継ぎの帳面へ受領を書く
//! - ④ dispatch の close は、ペインを木から外す**前**に `handoff_guard::check_close` を通す
//! - ⑤ MCP と CLI の close は呼び出し元のペインを運ぶ（自己片付けと master 同士の close を分ける）
//! - ⑥ 引き継ぎの発行は帳面へ書く（書かないと「受領済みの後任」の守りが効かない）

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `start_pat` を含む行から、その行と同じインデントで閉じる行（`}` / `},` / `} => {` 等の
/// `}` 始まり）までを (1-origin 行番号, 行) で返す。`nth` 番目（0 始まり）の一致を使う
fn block(src: &str, start_pat: &str, nth: usize) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains(start_pat))
        .nth(nth)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("`{start_pat}` が見つからない（名前が変わった？）"));
    block_at(&lines, start)
}

/// `start`（0 始まりの行の添字）から始まるブロック
fn block_at(lines: &[&str], start: usize) -> Vec<(usize, String)> {
    let indent = lines[start].len() - lines[start].trim_start().len();
    let mut out = vec![(start + 1, lines[start].to_string())];
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        out.push((i + 1, line.to_string()));
        let trimmed = line.trim_start();
        let here = line.len() - trimmed.len();
        if here == indent && trimmed.starts_with('}') {
            break;
        }
    }
    out
}

const APP: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

#[test]
fn 画面で確かめられず諦める枝は会話記録を見てから諦める() {
    let src = read(APP);
    let flows = block(&src, "fn drive_prompt_flows(", 0);
    let text: String = flows
        .iter()
        .map(|(_, l)| format!("{l}\n"))
        .collect::<String>();
    let arm = block(&text, "PromptFlowState::VerifySubmitted => {", 0);
    let base = flows[0].0 - 1;
    let mut violations = Vec::new();
    for (idx, (n, line)) in arm.iter().enumerate() {
        if !line.contains("Status::gave_up(") {
            continue;
        }
        // 諦める呼び出しの直前（同じ枝の中）に hold_for_receipt の判定がある
        let window = &arm[idx.saturating_sub(12)..idx];
        if !window.iter().any(|(_, l)| l.contains("hold_for_receipt(")) {
            violations.push(format!("{APP}:{}", base + n));
        }
    }
    assert!(
        violations.is_empty(),
        "VerifySubmitted で会話記録を見ずに諦めている（#2330 の偽陰性）: {violations:?}"
    );
    assert!(
        text.contains("PromptFlowState::ConfirmReceipt =>"),
        "{APP}: 受領の証拠を待つ ConfirmReceipt の枝が drive_prompt_flows に無い"
    );
}

#[test]
fn 最初の貼り付けの前に受領の材料を控える() {
    let src = read(APP);
    let flows = block(&src, "fn drive_prompt_flows(", 0);
    let paste = flows
        .iter()
        .position(|(_, l)| l.contains("Self::paste_prompt("))
        .expect("drive_prompt_flows に paste_prompt が無い");
    let captured = flows[..paste]
        .iter()
        .rev()
        .take(15)
        .any(|(_, l)| l.contains("ReceiptProbe::capture("));
    assert!(
        captured,
        "{APP}:{}: keys 経路の最初の貼り付けの前に ReceiptProbe::capture が無い（送信後の追記だけを見られない）",
        flows[paste].0
    );
}

#[test]
fn 送達の決着は引き継ぎの帳面へ受領を書く() {
    let src = read(APP);
    let body = block(&src, "fn finish_flow(", 0);
    assert!(
        body.iter()
            .any(|(_, l)| l.contains("handoff_guard::note_delivered(")),
        "{APP}:{}: finish_flow が handoff_guard::note_delivered を呼ばない（受領済みの後任を守れない）",
        body[0].0
    );
}

#[test]
fn dispatchのcloseはペインを外す前に守りを通す() {
    let src = read(DISPATCH);
    // dispatch の Close の UI の段（#2657 で `Request::Close` の腕から `close_pane_place` へ分けた。
    // 同期の腕と IPC の着地が同じここを通る）
    let arm = block(&src, "fn close_pane_place(", 0);
    let close_at = arm
        .iter()
        .find(|(_, l)| l.contains(".close(target)"))
        .map(|(n, _)| *n)
        .expect("Close の腕に .close(target) が無い");
    let guard_at = arm
        .iter()
        .find(|(_, l)| l.contains("handoff_guard::check_close("))
        .map(|(n, _)| *n);
    match guard_at {
        Some(g) if g < close_at => {}
        _ => panic!(
            "{DISPATCH}:{close_at}: ペインを外す前に handoff_guard::check_close を通していない（#2330）"
        ),
    }
    assert!(
        arm.iter()
            .any(|(_, l)| l.contains("handoff_guard::forget(")),
        "{DISPATCH}:{close_at}: 閉じたペインを引き継ぎの帳面から消していない"
    );
}

#[test]
fn mcpとcliのcloseは呼び出し元のペインを運ぶ() {
    let mcp = "crates/tako-control/src/mcp/request.rs";
    let body = block(&read(mcp), "\"tako_close_pane\" => Request::Close {", 0);
    assert!(
        body.iter().any(|(_, l)| l.trim() == "caller_pane: caller,"),
        "{mcp}:{}: tako_close_pane が呼び出し元のペインを運ばない",
        body[0].0
    );
    let cli = "crates/tako-cli/src/main.rs";
    let body = block(&read(cli), "Command::Close(args) => Request::Close {", 0);
    assert!(
        body.iter()
            .any(|(_, l)| l.trim() == "caller_pane: caller_pane(),"),
        "{cli}:{}: tako close が呼び出し元のペイン（TAKO_PANE_ID）を運ばない",
        body[0].0
    );
}

#[test]
fn 引き継ぎの発行は帳面へ書く() {
    let src = read(DISPATCH);
    let body = block(&src, "fn dispatch_orchestrator_handoff(", 0);
    assert!(
        body.iter()
            .any(|(_, l)| l.contains("handoff_guard::note_issued(")),
        "{DISPATCH}:{}: 引き継ぎの発行が帳面へ書かれない（受領済みの後任の守りが効かない）",
        body[0].0
    );
}

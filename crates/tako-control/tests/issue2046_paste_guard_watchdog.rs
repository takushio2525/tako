//! 番犬: 貼り付けの確認（#2046）の**関所を迂回させない・本文を診断ログへ出さない**。
//!
//! ## 何を止めたいのか
//!
//! 判定そのものは `tako_core::paste_guard` の単体テストが縛る。壊れ方は判定の外にある:
//!
//! 1. GUI の貼り付け（⌘V / Ctrl+V / Shift+Insert / メニューの「貼り付け」）の振り分けが
//!    **関所（`paste_to_terminal`）を通らずに** PTY へ書く（新しい入口を足したときに起きる）
//! 2. 関所と答え（`answer_pending_paste`）が**本文・伏せ字を診断ログへ書く**
//!    （AGENTS.md の絶対ルール。書いてよいのは `paste_guard::log_line` の 1 行だけ）
//! 3. 確認待ちの本文を持つ `PendingPaste` に `Debug` が付く（`{:?}` 1 つで本文がログへ出る道）
//! 4. `Finding` が生の値を持つ / `log_line` が伏せ字を書く
//!
//! の 4 つ。ここはそれをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! 確認が出て「やめる」で PTY へ 1 バイトも届かないこと・persist.log に本文が出ないことは
//! visual-test `paste-guard`（`scripts/test-paste-guard-2046.sh`）が実 GUI で見る。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `paste` の `self.paste_to_terminal(pane_id, text, cx);` を
//!    `if let Some(s) = self.focused_session() { s.paste(&text); }` へ /
//! ② `paste_to_terminal` の `persist_log(&…log_line(` を `persist_log(&format!("{text}"))` へ /
//! ③ `struct PendingPaste` の上に `#[derive(Debug)]` を足す

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const APP_REL: &str = "crates/tako-app/src/main.rs";
const CORE_REL: &str = "crates/tako-core/src/paste_guard.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `signature` を含む行から、同じ字下げの `}` までを（行番号つきで・コメント行を落として）返す
fn fn_body(rel: &str, src: &str, signature: &str) -> Vec<(usize, String)> {
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
    let body: Vec<(usize, String)> = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    body
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// ① GUI の貼り付けの振り分けは端末ペインへ必ず関所を通して渡す（PTY へ直接書かない）
#[test]
fn 貼り付けの振り分けは関所を通る() {
    let src = read(APP_REL);
    for (signature, must) in [(
        "    fn paste(&mut self, cx: &mut Context<Self>) {",
        "self.paste_to_terminal(",
    )] {
        let body = fn_body(APP_REL, &src, signature);
        assert!(
            joined(&body).contains(must),
            "{APP_REL}:{}: {signature:?} が {must} を呼んでいない（貼り付けが関所を通らない = #2046）",
            body[0].0
        );
        for (line, text) in &body {
            assert!(
                !text.contains("session.paste(") && !text.contains(".paste(&text)"),
                "{APP_REL}:{line}: 貼り付けの振り分けが PTY へ直接書いている（関所 \
                 paste_to_terminal を通す = #2046）: {}",
                text.trim()
            );
        }
    }
    // 関所は判定の 1 実装を引き、当たったら確認待ちへ積む（PTY へ書く前に return する）
    let gate = fn_body(
        APP_REL,
        &src,
        "    fn paste_to_terminal(&mut self, pane: PaneId, text: String, cx: &mut Context<Self>) {",
    );
    let g = joined(&gate);
    let scan = g.find("tako_core::paste_guard::scan(");
    let pend = g.find("self.pending_paste = Some(");
    let write = g.find("session.paste(&text)");
    assert!(
        matches!((scan, pend, write), (Some(s), Some(p), Some(w)) if s < p && p < w),
        "{APP_REL}:{}: 関所の順序が崩れた（判定 → 確認待ちへ積む → それ以外だけ PTY へ）: \
         scan={scan:?} pending={pend:?} write={write:?}",
        gate[0].0
    );
}

/// ② 関所と答えが診断ログへ書くのは `paste_guard::log_line` の 1 行だけ（本文・伏せ字を書かない）
#[test]
fn 関所と答えは本文を診断ログへ書かない() {
    let src = read(APP_REL);
    for signature in [
        "    fn paste_to_terminal(&mut self, pane: PaneId, text: String, cx: &mut Context<Self>) {",
        "    fn answer_pending_paste(",
        "    fn render_paste_guard_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {",
    ] {
        let body = fn_body(APP_REL, &src, signature);
        let lines: Vec<&(usize, String)> = body.iter().collect();
        for (i, (line, text)) in lines.iter().enumerate() {
            for banned in ["eprintln!", "println!", "tracing::", "dbg!", "perf_span("] {
                assert!(
                    !text.contains(banned),
                    "{APP_REL}:{line}: 貼り付けの確認の経路で {banned} を使っている（本文・伏せ字を \
                     診断ログへ出さない = AGENTS.md の絶対ルール / #2046）: {}",
                    text.trim()
                );
            }
            if let Some(after) = text.split("persist_log(").nth(1) {
                // 引数そのものが `paste_guard::log_line(` で始まる（同じ行か、改行したなら次の行の頭）。
                // 「近くに log_line がある」では足りない（直前に本文を書く行を足しても通ってしまう）
                let arg = match after.trim() {
                    "" => lines.get(i + 1).map(|(_, t)| t.trim()).unwrap_or(""),
                    a => a,
                };
                let arg = arg.trim_start_matches('&');
                assert!(
                    arg.starts_with("tako_core::paste_guard::log_line(")
                        || arg.starts_with("paste_guard::log_line("),
                    "{APP_REL}:{line}: persist.log へ log_line 以外を書いている（規則名と件数だけに \
                     する = #2046）: {}",
                    text.trim()
                );
            }
        }
    }
}

/// ③ 確認待ちの本文を持つ型に `Debug` を付けない
#[test]
fn 確認待ちの型にdebugを付けない() {
    let src = read(APP_REL);
    let lines: Vec<&str> = src.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("struct PendingPaste {"))
        .unwrap_or_else(|| panic!("{APP_REL}: struct PendingPaste が消えている"));
    for back in 1..=4 {
        let Some(l) = at.checked_sub(back).map(|i| lines[i]) else {
            break;
        };
        if !l.trim_start().starts_with("#[") && !is_comment(l) {
            break;
        }
        assert!(
            !(l.contains("derive") && l.contains("Debug")),
            "{APP_REL}:{}: PendingPaste に Debug が付いた（`{{:?}}` 1 つで本文がログへ出る = #2046）",
            at - back + 1
        );
    }
    assert!(
        !src.contains("impl std::fmt::Debug for PendingPaste")
            && !src.contains("impl Debug for PendingPaste"),
        "{APP_REL}: PendingPaste に Debug の実装がある（#2046）"
    );
}

/// ④ 判定の結果は生の値を持たず、ログの 1 行は伏せ字も書かない
#[test]
fn 判定の結果は生の値を持たない() {
    let src = read(CORE_REL);
    let finding = fn_body(CORE_REL, &src, "pub struct Finding {");
    let fields: Vec<String> = finding
        .iter()
        .skip(1)
        .filter_map(|(_, l)| {
            let t = l.trim();
            t.strip_prefix("pub ")
                .and_then(|f| f.split(':').next())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        fields,
        ["rule", "line", "masked", "start", "end"],
        "{CORE_REL}:{}: Finding のフィールドが変わった。生の値（本文の切り出し）を持たせない（#2046）",
        finding[0].0
    );
    let log = fn_body(
        CORE_REL,
        &src,
        "pub fn log_line(outcome: &str, pane: u64, report: &ScanReport) -> String {",
    );
    for (line, text) in &log {
        assert!(
            !text.contains("masked") && !text.contains("start") && !text.contains("end"),
            "{CORE_REL}:{line}: log_line が伏せ字・位置を書いている（規則名と件数とバイト数だけ = #2046）: {}",
            text.trim()
        );
    }
}

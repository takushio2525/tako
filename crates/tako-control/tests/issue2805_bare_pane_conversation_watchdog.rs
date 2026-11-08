//! 器なしのペインでも codex / agy の会話 ID を辿る配線の番犬（#2805）
//!
//! codex / agy の会話 ID は器のセッション名からペインの pid を引いて辿っていた
//! （`codex_session::resolve_thread_id_for_backend` / `agy_session::resolve_conversation_id_for_backend`）。
//! 器の無い構成（tmux 不在 / persist OFF の macOS）ではセッション名が無いので必ず空振りし、
//! `tako orchestrator report` は「backend session 不在または scrollback 空」で失敗、
//! `tako orchestrator status` は画面の推定（`status_source: screen`）へ落ちていた。
//!
//! 直した形は #728 の claude と同じ二段構え: 器があれば器のセッションから、器なしのペインは
//! PTY 直下の子 pid（UI スレッドで `bare_pty_pid` が写す）を根にして同じ辿り方をする。
//! 分岐は `codex_thread_for_pane` / `agy_conversation_for_pane` の 2 つにだけ置き、
//! report と status はそれを呼ぶ。ここは次の形を `file:line` で名指す:
//!
//! 1. 2 つの口から器なしの腕（`*_for_pty`）が外れた
//! 2. report / status の本体が器のセッションだけの口（`*_for_backend`）を直に呼ぶ（器なしの腕を素通りする）
//! 3. report / status が 2 つの口を呼ばない
//! 4. UI スレッドの写し（report の文脈・status の材料）が器なしの根を写さない
//! 5. #390 の同一性の確かめが別ペインと分かったときに器なしの根を捨てない
//!
//! 実プロセス（偽 codex / 偽 agy）で辿れることは `dispatch::tests::issue2805_*`（macOS）が確かめる。
//! この番犬は lsof の無い Windows の CI でも配線の外れを落とすためのもの。

use std::path::Path;

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

fn read_production(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    production_range::production(&src, rel)
}

/// 関数の窓（宣言行の 1-based 行番号と行）。終わりは宣言行と同じ字下げの `}`
fn fn_window<'a>(lines: &[&'a str], head: &str) -> Option<(usize, Vec<&'a str>)> {
    let start = lines.iter().position(|l| l.contains(head))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].to_vec()))
}

/// 窓の中で `needle` を含むコードの行（1-based の絶対行番号）
fn code_lines(at: usize, window: &[&str], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| at + i)
        .collect()
}

/// `src`（dispatch.rs の製品部）の違反を `file:line — 理由` で返す
fn scan(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut window = |head: &str| match fn_window(&lines, head) {
        Some(w) => Some(w),
        None => {
            out.push(format!(
                "{DISPATCH}:0 — `{head}` が見つからない（改名したら番犬も直す）"
            ));
            None
        }
    };

    // 1. 分岐の口は器あり・器なしの両方の腕を持つ
    let mut found = Vec::new();
    for (head, backend, pty) in [
        (
            "fn codex_thread_for_pane(",
            "codex_session::resolve_thread_id_for_backend(",
            "codex_session::resolve_thread_id_for_pty(",
        ),
        (
            "fn agy_conversation_for_pane(",
            "agy_session::resolve_conversation_id_for_backend(",
            "agy_session::resolve_conversation_id_for_pty(",
        ),
    ] {
        if let Some((at, w)) = window(head) {
            found.push((head, at, w, backend, pty));
        }
    }
    // 2〜3. report / status は 2 つの口を通し、器のセッションだけの口を直に呼ばない
    let mut bodies = Vec::new();
    for head in ["fn finish_orchestrator_report(", "fn finish_worker_status("] {
        if let Some((at, w)) = window(head) {
            bodies.push((head, at, w));
        }
    }
    // 4〜5. UI スレッドの写しと同一性の確かめ
    let mut copies = Vec::new();
    for (head, needle) in [
        ("fn collect_report_ctx(", "bare_pty_pid("),
        ("fn worker_status_seed(", "bare_pty_pid("),
        ("fn verify_ctx_pane_identity(", "ctx.pty_pid = None"),
    ] {
        if let Some((at, w)) = window(head) {
            copies.push((head, at, w, needle));
        }
    }

    for (head, at, w, backend, pty) in found {
        if code_lines(at, &w, pty).is_empty() {
            out.push(format!(
                "{DISPATCH}:{at} — `{head}` から器なしの腕 `{pty}` が外れた \
                 （器なしのペインの codex / agy の report が失敗し、status が画面の推定へ落ちる = #2805）"
            ));
        }
        if code_lines(at, &w, backend).is_empty() {
            out.push(format!(
                "{DISPATCH}:{at} — `{head}` から器の腕 `{backend}` が外れた（器のペインが辿れない）"
            ));
        }
    }
    for (head, at, w) in bodies {
        for direct in [
            "resolve_thread_id_for_backend(",
            "resolve_conversation_id_for_backend(",
        ] {
            for line in code_lines(at, &w, direct) {
                out.push(format!(
                    "{DISPATCH}:{line} — `{head}` が器のセッションだけの口 `{direct}` を直に呼んでいる \
                     （器なしのペインを素通りする。`codex_thread_for_pane` / `agy_conversation_for_pane` を通す = #2805）"
                ));
            }
        }
        for via in ["codex_thread_for_pane(", "agy_conversation_for_pane("] {
            if code_lines(at, &w, via).is_empty() {
                out.push(format!(
                    "{DISPATCH}:{at} — `{head}` が `{via}` を呼ばない（器なしのペインの会話 ID を辿らない = #2805）"
                ));
            }
        }
    }
    for (head, at, w, needle) in copies {
        if code_lines(at, &w, needle).is_empty() {
            out.push(format!(
                "{DISPATCH}:{at} — `{head}` に `{needle}` が無い（器なしの根を写さない / 別ペインの根を捨てない = #2805）"
            ));
        }
    }
    out
}

#[test]
fn issue2805_器なしのペインもcodexとagyの会話idを辿る() {
    let src = read_production(DISPATCH);
    let offenders = scan(&src);
    assert!(
        offenders.is_empty(),
        "器なしのペインの codex / agy の会話 ID の配線が外れている:\n{}",
        offenders.join("\n")
    );
}

/// 番犬自身の注入: 実物の dispatch.rs を書き換えた眺めで、外れ方ごとに名指すことを確かめる
#[test]
fn issue2805_番犬は配線の外れを名指す() {
    let src = read_production(DISPATCH);
    assert!(scan(&src).is_empty(), "前提: 実物は違反なし");
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "器なしの腕を外す",
            "crate::codex_session::resolve_thread_id_for_pty(pane_id, pid)",
            "None",
            "器なしの腕 `codex_session::resolve_thread_id_for_pty(`",
        ),
        (
            "agy の器なしの腕を外す",
            "crate::agy_session::resolve_conversation_id_for_pty(pane_id, pid)",
            "None",
            "器なしの腕 `agy_session::resolve_conversation_id_for_pty(`",
        ),
        (
            "report が器だけの口を直に呼ぶ",
            "let tid = codex_thread_for_pane(pane_id, backend.as_deref(), pty_pid)?;",
            "let tid = crate::codex_session::resolve_thread_id_for_backend(backend.as_deref()?)?;",
            "`fn finish_orchestrator_report(` が器のセッションだけの口 `resolve_thread_id_for_backend(`",
        ),
        (
            "report の写しが根を写さない",
            ".then(|| bare_pty_pid(host, target))",
            ".then(|| None::<u32>)",
            "`fn collect_report_ctx(` に `bare_pty_pid(` が無い",
        ),
        (
            "同一性の確かめが根を捨てない",
            "ctx.pty_pid = None;",
            "",
            "`fn verify_ctx_pane_identity(` に `ctx.pty_pid = None` が無い",
        ),
    ];
    for (label, from, to, expect) in cases {
        assert!(
            src.contains(from),
            "{label}: 注入の元 `{from}` が実物に無い"
        );
        let injected = src.replacen(from, to, 1);
        let offenders = scan(&injected);
        assert!(
            offenders.iter().any(|o| o.contains(expect)),
            "{label}: `{expect}` を名指さない:\n{}",
            offenders.join("\n")
        );
    }
}

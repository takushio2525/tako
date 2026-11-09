//! 器なしの worker の `has_running_children` を数える配線の番犬（#2804）
//!
//! `tako orchestrator status` / MCP `tako_orchestrator_worker_status` の `has_running_children` は、
//! 照会の後段（`probe_running_children`。background）が埋める。旧実装は器のセッション名がある場合だけ
//! `agents::has_running_children` で数え、器なしのペイン（tmux 不在 / persist OFF の macOS・psmux の無い
//! Windows）は agent が動いていても常に false だった。その結果、#1297 の倒し（入力待ち + 背景作業 → idle）・
//! 画面から決まらない（unknown）ときの子の有無の補助が器なしで効かず、busy の worker が #224 の stalled へ
//! 倒れていた。GUI の busy の判定は #1367 で「器の有無で呼び分けない 1 実装」へ寄せ済みで、worker_status の
//! 経路だけが取り残されていた。
//!
//! 直した形は #2805 の会話 ID と同じ二段構え: 器があれば器のセッションから（修正前と同じ）、器なしのペインは
//! UI スレッドで写した PTY 直下の子 pid（`bare_pty_pid`）を根に `agents::bare_pane_has_running_children` で
//! 数える（規則は #372 / #1367 の走査と同じ「根のシェル自身を除いた子孫」）。ここは次の形を `file:line` で名指す:
//!
//! 1. `probe_running_children` から器なしの腕（`bare_pane_has_running_children(`）が外れた・根に `pty_pid`
//!    を使っていない
//! 2. `probe_running_children` から器の腕（`agents::has_running_children(`）が外れた（器ありの値が変わる）
//! 3. 器なしの口が A/B（`legacy_2804`）を通らない / 規則の 1 実装（`has_running_descendants_inner`）を通らない
//!
//! 実プロセスで数えられることは `dispatch::tests::issue2804_*` と `agents::tests::issue2804_*` が確かめる。
//! UI スレッドで数えないこと（`ps` を起こす口を写しの段で呼ばない）は `issue1968_ui_thread_subprocess_watchdog`
//! の禁止リストが縛る。

use std::path::Path;

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";

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

/// dispatch.rs / agents.rs（製品部）の違反を `file:line — 理由` で返す
fn scan(dispatch: &str, agents: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1〜2. 子の有無を埋める後段は器あり・器なしの両方の腕を持つ
    let lines: Vec<&str> = dispatch.lines().collect();
    match fn_window(&lines, "fn probe_running_children(") {
        None => out.push(format!(
            "{DISPATCH}:0 — `fn probe_running_children(` が見つからない（改名したら番犬も直す）"
        )),
        Some((at, w)) => {
            let bare = code_lines(at, &w, "agents::bare_pane_has_running_children(");
            if bare.is_empty() {
                out.push(format!(
                    "{DISPATCH}:{at} — `probe_running_children` から器なしの腕 \
                     `agents::bare_pane_has_running_children(` が外れた（器なしの worker の \
                     `has_running_children` が常に false に戻り、#1297 の倒しと unknown の補助が効かない = #2804）"
                ));
            } else if code_lines(at, &w, "ctx.pty_pid").is_empty() {
                out.push(format!(
                    "{DISPATCH}:{} — `probe_running_children` の器なしの腕が PTY 直下の子 pid（`pty_pid`）を\
                     根にしていない（UI スレッドの写し `bare_pty_pid` と同じ根で数える = #2804）",
                    bare[0]
                ));
            }
            if code_lines(at, &w, "agents::has_running_children(").is_empty() {
                out.push(format!(
                    "{DISPATCH}:{at} — `probe_running_children` から器の腕 `agents::has_running_children(` が\
                     外れた（器ありの worker の値が修正前と変わる = #2804 は器ありの経路を変えない）"
                ));
            }
        }
    }

    // 3. 器なしの口は A/B と規則の 1 実装を通る
    let lines: Vec<&str> = agents.lines().collect();
    for (head, needle, why) in [
        (
            "pub fn bare_pane_has_running_children(",
            "legacy_2804()",
            "A/B（`TAKO_2804_LEGACY=1`）を通らない",
        ),
        (
            "pub fn bare_pane_has_running_children_in(",
            "has_running_descendants_inner(",
            "規則の 1 実装（`has_running_descendants_inner` = 根のシェル自身を除いた子孫。\
             #372 / #1367 の走査と同じ）を通らない",
        ),
    ] {
        match fn_window(&lines, head) {
            None => out.push(format!(
                "{AGENTS}:0 — `{head}` が見つからない（改名したら番犬も直す）"
            )),
            Some((at, w)) => {
                if code_lines(at, &w, needle).is_empty() {
                    out.push(format!("{AGENTS}:{at} — `{head}` が {why}（#2804）"));
                }
            }
        }
    }
    out
}

#[test]
fn issue2804_器なしのworkerもhas_running_childrenを数える() {
    let offenders = scan(&read_production(DISPATCH), &read_production(AGENTS));
    assert!(
        offenders.is_empty(),
        "器なしの worker の has_running_children の配線が外れている:\n{}",
        offenders.join("\n")
    );
}

/// 番犬自身の注入: 実物を書き換えた眺めで、外れ方ごとに名指すことを確かめる
#[test]
fn issue2804_番犬は配線の外れを名指す() {
    let dispatch = read_production(DISPATCH);
    let agents = read_production(AGENTS);
    assert!(scan(&dispatch, &agents).is_empty(), "前提: 実物は違反なし");
    // (ラベル, dispatch.rs か, 元, 先, 名指しに含まれる語)
    let cases: [(&str, bool, &str, &str, &str); 5] = [
        (
            "器なしの腕を外す（修正前の形）",
            true,
            "(None, Some(pid)) => crate::agents::bare_pane_has_running_children(pid),",
            "(None, Some(_)) => false,",
            "器なしの腕 `agents::bare_pane_has_running_children(`",
        ),
        (
            "器なしの腕が写した根を使わない",
            true,
            "match (ctx.backend_session.as_deref(), ctx.pty_pid) {",
            "match (ctx.backend_session.as_deref(), Some(0u32)) {",
            "根にしていない",
        ),
        (
            "器の腕を外す",
            true,
            "(Some(session), _) => crate::agents::has_running_children(session),",
            "(Some(_), _) => false,",
            "器の腕 `agents::has_running_children(`",
        ),
        (
            "器なしの口が A/B を通らない",
            false,
            "bare_pane_has_running_children_in(pty_pid, &process_parent_map(), legacy_2804())",
            "bare_pane_has_running_children_in(pty_pid, &process_parent_map(), false)",
            "A/B（`TAKO_2804_LEGACY=1`）を通らない",
        ),
        (
            "器なしの口が規則の 1 実装を通らない",
            false,
            "!legacy_backend_only && has_running_descendants_inner(&[pty_pid], parents)",
            "!legacy_backend_only && parents.values().any(|p| *p == pty_pid)",
            "規則の 1 実装",
        ),
    ];
    for (label, in_dispatch, from, to, expect) in cases {
        let target = if in_dispatch { &dispatch } else { &agents };
        assert!(
            target.contains(from),
            "{label}: 注入の元 `{from}` が実物に無い"
        );
        let injected = target.replacen(from, to, 1);
        let offenders = if in_dispatch {
            scan(&injected, &agents)
        } else {
            scan(&dispatch, &injected)
        };
        assert!(
            offenders.iter().any(|o| o.contains(expect)),
            "{label}: `{expect}` を名指さない:\n{}",
            offenders.join("\n")
        );
    }
}

//! **#2737 の番犬**: 編集カーソルの追従（#1649）を、描いた後に確かめ直さない形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 追従（`follow_preview_cursor`）は可視範囲を描いた行の実寸から数えるが、まだ描いていない行は
//! 1 行ぶんの高さで見積もる。描画の Markdown で ⌘F → Enter すると、表示はエディタの行へ落ちた
//! ばかりで飛び先の行はまだ描かれておらず、見積もり（全節実行の本文 351.5px ÷ 21px = 16 行）で
//! 先頭を 158 行目へ置いた。ところが本文が 149px まで狭いと filler の行が 2 段（42px）に折り返し、
//! 描いた後の器には 11 行しか入らず、170 行目のヒットが下端の外に残った（利用者から見るとヒットが
//! 隠れる）。閉じたときの描き直し（#1661 の `restore_rendered_preview`）はキャレットが見えて
//! いなければ先頭可視行の節から描くので、visual-test md-find-restore の「ヒットのあった節（Beta）
//! の見出しから描く (#1873)」が全節実行で 3/3 落ちていた（単独は本文 450px で折り返さず通る）。
//!
//! いまは追従が器を動かしたら予約し（`arm_follow_recheck`）、器を描いたら控え（`follow_recheck_canvas`
//! の paint）、次の描画の頭（`render_preview_pane` の入口）で同じ判断（`follow_cursor` と
//! `FOLLOW_MARGIN`）を描いた行の実寸でやり直す（`recheck_preview_follow`）。最初は `on_next_frame`
//! の中で確かめていたが、ディスプレイの周期が止まった窓（全節実行）では呼ばれず効かなかった
//! （実測）。折り返さなければ見積もりと実寸が同じで何も動かない = #1741 の判定と #2714 の
//! 詰めた余白はそのまま。
//!
//! ## ここで止めるもの
//!
//! 1. [`追従は器を動かしたら確かめ直しを予約する`] — `follow_preview_cursor` が仮想リストを
//!    `scroll_to` した後に `arm_follow_recheck(` を呼ぶ
//! 2. [`確かめ直しは追従と同じ判断を描いた実寸でやり直す`] — `recheck_preview_follow` が
//!    `editor_scroll::follow_cursor(` と `FOLLOW_MARGIN` で判断し、追従の後に器が動いていたら
//!    （`logical_scroll_top`）確かめず、回数の上限（`left - 1`）を持つ。旧の形（`TAKO_2737_LEGACY`）を
//!    `arm_follow_recheck` が読む
//! 3. [`コードの仮想リストは描いた次のフレームで確かめる`] — コード本文の仮想リストの隣に
//!    `follow_recheck_canvas(` があり描いたことを控えて次のフレームを頼み、`render_preview_pane` が
//!    本文を組み直す前に `recheck_preview_follow` を呼ぶ
//! 4. [`md_find_restoreは狭い本文でヒットが見えるのを確かめる`] — 節が本文を狭めて
//!    （`NARROW_BODY_PX`）から ⌘F を押し、Escape の前にヒットが可視範囲に入るのを待つ
//!    （`md-find-hit-visible`）
//! 5. [`修正前の形を名指しできる`] — 修正前の形を注入すると 1〜4 のどれかが名指しで落ちる
//!
//! ## 相方
//!
//! 実 GUI の A/B は visual-test `md-find-restore`（`TAKO_2737_LEGACY=1` は「検索のヒット
//! （Enter で飛んだ行）が本文の可視範囲に描かれる」で落ちる）。追従そのものの設計は
//! `tako_core::editor_scroll` の単体テスト（#1649 / #2714）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `head` の後ろの最初の `{` から対の `}` までのバイト範囲（コードだけの眺めで括弧を数える）
fn block_range(code: &str, head: &str) -> Option<std::ops::Range<usize>> {
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in code.as_bytes()[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open..open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// 関数の本文（文字列は残しコメントだけ潰した眺め）
fn body_of(src: &str, head: &str) -> Option<String> {
    let range = block_range(&code_view::code_view(src), head)?;
    Some(code_view::without_comments(src)[range].to_string())
}

/// 両方が見つかり、`a` が `b` より前にあるか
fn before(a: Option<usize>, b: Option<usize>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a < b)
}

/// 規則 1〜3 をまとめて（注入の確かめ用）
fn render_problems(render: &str) -> Vec<String> {
    let mut problems = follow_problems(render);
    problems.extend(recheck_problems(render));
    problems.extend(canvas_problems(render));
    problems
}

/// 規則 1: 追従が器を動かしたら確かめ直しを予約するか
fn follow_problems(render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(render, "pub(crate) fn follow_preview_cursor(") {
        Some(body) if before(body.find("list.scroll_to("), body.find("arm_follow_recheck(")) => {}
        Some(_) => problems.push(
            "`follow_preview_cursor` が仮想リストを動かした後に `arm_follow_recheck(` を呼んでいない\
             （まだ描いていない行の見積もりのまま = 折り返すとキャレットが器の外に残る）"
                .into(),
        ),
        None => problems.push("`fn follow_preview_cursor(` が見つからない".into()),
    }
    problems
}

/// 規則 2: 確かめ直しが追従と同じ判断・上限・A/B を持つか
fn recheck_problems(render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(render, "pub(crate) fn recheck_preview_follow(") {
        Some(body) => {
            for (needle, why) in [
                ("editor_scroll::follow_cursor(", "追従と同じ判断"),
                (
                    "FOLLOW_MARGIN",
                    "追従と同じ余白（#2714 の詰めた余白も同じ関数の中）",
                ),
                (
                    "logical_scroll_top()",
                    "追従の後に器が動いていたら確かめない",
                ),
                ("left - 1", "回数の上限"),
            ] {
                if !body.contains(needle) {
                    problems.push(format!(
                        "`recheck_preview_follow` に `{needle}` が無い（{why}）"
                    ));
                }
            }
        }
        None => problems
            .push("`fn recheck_preview_follow(` が無い（描いた後に追従を確かめ直す口）".into()),
    }
    match body_of(render, "fn arm_follow_recheck(") {
        Some(body) if body.contains("follow_recheck_legacy()") => {}
        _ => problems.push(
            "`arm_follow_recheck` が旧の形（`follow_recheck_legacy()` = `TAKO_2737_LEGACY`）を読まない（A/B）"
                .into(),
        ),
    }
    if !render.contains("std::env::var_os(\"TAKO_2737_LEGACY\")") {
        problems.push("`TAKO_2737_LEGACY` を読む口が無い（A/B）".into());
    }
    problems
}

/// 規則 3: コード本文の仮想リストが、描いた次のフレームで確かめ直しを積むか
fn canvas_problems(render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    // コード本文の仮想リスト（`render_preview_code_line` を item にする list）の隣に積む canvas。
    // 同じ item の呼び出しは旧挙動（全行を組む `return_all_code_lines`）にもあるので、どれか 1 か所の
    // 隣に在ればよい
    let code = code_view::code_view(render);
    let beside_list = code
        .match_indices("render_preview_code_line(pane_id, ix, cx)")
        .any(|(at, _)| {
            code[at..]
                .find("follow_recheck_canvas(pane_id, cx)")
                .is_some_and(|offset| offset < 600)
        });
    if !beside_list {
        problems.push(
            "コード本文の仮想リストの隣に `follow_recheck_canvas(pane_id, cx)` が無い\
             （描いた次のフレームで確かめ直さない）"
                .into(),
        );
    }
    match body_of(render, "fn follow_recheck_canvas(") {
        Some(body)
            if body.contains("note_follow_recheck_painted(") && body.contains("on_next_frame(") => {}
        _ => problems.push(
            "`follow_recheck_canvas` が描いたことを控えて（`note_follow_recheck_painted`）次のフレームを\
             頼んでいない（`on_next_frame`。描いた行の実寸が揃うのは描いた後）"
                .into(),
        ),
    }
    // 確かめるのは描画の頭 = 本文を組み直して行の控えを空にする前（`on_next_frame` だけに頼ると、
    // ディスプレイの周期が止まった窓（visual-test の全節実行）では確かめが来ない = 実測）
    match body_of(render, "pub(crate) fn render_preview_pane(") {
        Some(body)
            if before(
                body.find("self.recheck_preview_follow(pane_id);"),
                body.find("self.preview_text_layouts.insert(pane_id, vec![None; count]);"),
            ) => {}
        _ => problems.push(
            "`render_preview_pane` が本文を組み直す（行の控えを空にする）前に \
             `recheck_preview_follow(pane_id)` を呼んでいない"
                .into(),
        ),
    }
    problems
}

/// 規則 4: visual-test md-find-restore が狭い本文でヒットが見えるのを確かめるか
fn section_problems(main: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(main, "async fn md_find_restore_visual(") {
        Some(body) => {
            if !before(body.find("NARROW_BODY_PX"), body.find("key(cx, find_key)")) {
                problems.push(
                    "md-find-restore が ⌘F の前に本文を狭めていない（`NARROW_BODY_PX`。単独の 450px では\
                     折り返さず、隠れる場面を作れない）"
                        .into(),
                );
            }
            if !before(
                body.find("\"md-find-hit-visible\""),
                body.find("key(cx, \"escape\")"),
            ) {
                problems.push(
                    "md-find-restore が Escape の前にヒットが可視範囲に入るのを待っていない\
                     （`md-find-hit-visible`。人は見えてから閉じる）"
                        .into(),
                );
            }
        }
        None => problems.push("`async fn md_find_restore_visual(` が見つからない".into()),
    }
    problems
}

#[test]
fn 追従は器を動かしたら確かめ直しを予約する() {
    let problems = follow_problems(&read(RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 確かめ直しは追従と同じ判断を描いた実寸でやり直す() {
    let problems = recheck_problems(&read(RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn コードの仮想リストは描いた次のフレームで確かめる() {
    let problems = canvas_problems(&read(RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn md_find_restoreは狭い本文でヒットが見えるのを確かめる() {
    let problems = section_problems(&read(MAIN));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 修正前の形を名指しできる() {
    let render = read(RENDER);
    let main = read(MAIN);
    assert!(render_problems(&render).is_empty(), "今の本文は通る");
    // 1. 追従が予約しない（修正前）
    let no_arm = render.replacen(
        "self.arm_follow_recheck(pane_id, first, FOLLOW_RECHECK_LIMIT);",
        "",
        1,
    );
    assert_ne!(no_arm, render, "注入先（追従の予約）が本文に在る");
    assert!(
        render_problems(&no_arm)
            .iter()
            .any(|p| p.contains("`follow_preview_cursor` が")),
        "予約しない追従を名指せる"
    );
    // 2. 確かめ直しが追従と別の判断で動かす（確かめ直しの本文の中だけを書き換える。追従の本文にも
    //    同じ呼び出しがあるので、ファイル全体の置換では追従の側が変わる）
    let other_rule = {
        let code = code_view::code_view(&render);
        let range =
            block_range(&code, "pub(crate) fn recheck_preview_follow(").expect("確かめ直しの本文");
        let body = render[range.clone()].replace(
            "editor_scroll::follow_cursor(",
            "editor_scroll::fixed_rows(",
        );
        format!("{}{}{}", &render[..range.start], body, &render[range.end..])
    };
    assert_ne!(other_rule, render, "注入先（確かめ直しの判断）が本文に在る");
    assert!(
        render_problems(&other_rule)
            .iter()
            .any(|p| p.contains("editor_scroll::follow_cursor(")),
        "追従と別の判断で動かす確かめ直しを名指せる"
    );
    // 3. 描いた次のフレームで確かめない（canvas を外す）
    let no_canvas = render.replacen("follow_recheck_canvas(pane_id, cx),\n", "", 1);
    assert_ne!(
        no_canvas, render,
        "注入先（確かめ直しの canvas）が本文に在る"
    );
    assert!(
        render_problems(&no_canvas)
            .iter()
            .any(|p| p.contains("仮想リストの隣")),
        "確かめ直しを積まない仮想リストを名指せる"
    );
    // 4. 節がヒットの見えるのを待たずに閉じる
    let no_wait = main.replacen("\"md-find-hit-visible\"", "\"md-find-other\"", 1);
    assert_ne!(
        no_wait, main,
        "注入先（ヒットの見えるのを待つ）が本文に在る"
    );
    assert!(
        section_problems(&no_wait)
            .iter()
            .any(|p| p.contains("md-find-hit-visible")),
        "見えるのを待たない md-find-restore を名指せる"
    );
}

//! **#2757 の番犬**: 編集カーソルの追従（#1649）が、カーソル行そのものを器に入れることを余白より
//! 優先する形を崩さない。
//!
//! ## 何が起きていたのか
//!
//! 追従の判断（`tako_core::editor_scroll::follow_cursor`）は行の**数**だけで先頭を決めていた。
//! 狭いペイン（visual-test editor-font を 560×600 の窓で回した形）で、器（381px）より背の高い
//! 折り返し行（650px）の次の短い行へ ↓ すると、
//!
//! 1. 背の高い行が先頭 = 可視 1 行・余白 0 → 先頭を次の行へ下げる
//! 2. #2737 の確かめ直しで、下げた先から数え直すと可視 4 行 = 余白 1 行 → 先頭を背の高い行へ戻す
//!
//! を確かめ直しの上限まで往復し、背の高い行が器を覆ったままキャレットが器の外（下）に残った
//! （500ms 待っても戻らない。`TAKO_2737_LEGACY=1` では起きない = PR #2750 の退行）。
//!
//! いまは `follow_cursor` が `LineViewport::fit_top`（カーソル行が器に丸ごと入る、いちばん上の
//! 先頭行）より上へ先頭を置かない。最初の追従も確かめ直しも同じ関数・同じ値（`preview_cursor_viewport`
//! が行の実寸から出す）で判断するので、どちらから見ても答えが同じ = 往復しない。器の外へ出た上の
//! 行の実寸は描いた行の控え（`preview_text_layouts`）から消えるので、背の高い行だけ覚えておく
//! （`RowHeightMemo`。描いた瞬間に `note_row_height` で書き、行の版・文字サイズ・器の幅が変われば捨てる）。
//!
//! ## ここで止めるもの
//!
//! 1. [`カーソル行を器に入れる規則はfollow_cursorの1か所にある`] — `follow_cursor` が `view.fit_top` で
//!    先頭を収め、`fit_top` の純関数がある
//! 2. [`追従と確かめ直しは同じ可視範囲から同じ値を受け取る`] — `preview_cursor_viewport` が
//!    `editor_scroll::fit_top(` に行の実寸（`preview_row_height(`）を渡して `fit_top` を組み、旧の形
//!    （`follow_fit_legacy()` = `TAKO_2757_LEGACY`）を読む。追従（`follow_preview_cursor`）と確かめ直し
//!    （`recheck_preview_follow`）は `preview_cursor_viewport(` を通し、**自分の規則を持たない**
//!    （`fit_top` / `preview_row_height` を直に触らない）
//! 3. [`背の高い行は描いた瞬間に覚え描き直しの前に鍵を合わせる`] — 行が自分の枠へ実寸を書く所で
//!    `note_row_height(` を呼び、本文を組み直す（行の控えを空にする）前に `renew_row_height_memo(` を呼ぶ
//! 4. [`editor_fontはページ移動を確かめ直しの後に設計の式で判定する`] — visual-test editor-font の
//!    `probe` が確かめ直しの予約（`preview_follow_recheck`）が消えるまで描いてから測り、ページ移動の
//!    判定が `page_step(` / `landing_margin(` を通る（`Some(FOLLOW_MARGIN)` を直に期待しない）
//! 5. [`修正前の形を名指しできる`] — 修正前の形を注入すると 1〜4 のどれかが名指しで落ちる
//!
//! ## 相方
//!
//! 追従の算術の A/B は `tako_core::editor_scroll` の単体テスト（背の高い行の次の行で、修正後は 1 回で
//! 収束・修正前は往復）。実 GUI の A/B は visual-test `editor-font` を 560×600 の窓で
//! （`TAKO_2757_LEGACY=1` は「折り返し: ↓ の途中でカーソル行が器の外へ出た」で落ちる）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";
const SCROLL: &str = "crates/tako-core/src/editor_scroll.rs";

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

/// 規則 1: カーソル行を器に入れる規則が `follow_cursor` の 1 か所にあるか
fn core_problems(scroll: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(scroll, "pub fn follow_cursor(") {
        Some(body) if body.contains("view.fit_top") && body.contains(".clamp(") => {}
        Some(_) => problems.push(
            "`follow_cursor` が `view.fit_top` で先頭を収めていない（カーソル行が器に入ることを余白より\
             優先しない = 背の高い行の次の行で追従と確かめ直しが往復する）"
                .into(),
        ),
        None => problems.push("`pub fn follow_cursor(` が見つからない".into()),
    }
    if body_of(scroll, "pub fn fit_top(").is_none() {
        problems.push("`pub fn fit_top(`（カーソル行が器に入る、いちばん上の先頭行）が無い".into());
    }
    problems
}

/// 規則 2: 追従と確かめ直しが同じ可視範囲から同じ値を受け取るか
fn viewport_problems(render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(render, "pub(crate) fn preview_cursor_viewport(") {
        Some(body) => {
            for (needle, why) in [
                ("editor_scroll::fit_top(", "カーソル行が器に入る先頭を出す"),
                (
                    "preview_row_height(",
                    "上へ出た背の高い行の実寸を覚えから引く",
                ),
                ("follow_fit_legacy()", "旧の形（A/B）"),
                ("fit_top,", "可視範囲へ載せる"),
            ] {
                if !body.contains(needle) {
                    problems.push(format!(
                        "`preview_cursor_viewport` に `{needle}` が無い（{why}）"
                    ));
                }
            }
        }
        None => problems.push("`fn preview_cursor_viewport(` が見つからない".into()),
    }
    for head in [
        "pub(crate) fn follow_preview_cursor(",
        "pub(crate) fn recheck_preview_follow(",
    ] {
        match body_of(render, head) {
            Some(body) => {
                if !body.contains("preview_cursor_viewport(") {
                    problems.push(format!(
                        "`{head}` が `preview_cursor_viewport(` を通らない（追従と確かめ直しが別の可視範囲を見る）"
                    ));
                }
                for own in ["fit_top", "preview_row_height", "preview_row_heights"] {
                    if body.contains(own) {
                        problems.push(format!(
                            "`{head}` が `{own}` を直に触る（自分の規則を足さず `follow_cursor` の 1 実装に任せる）"
                        ));
                    }
                }
            }
            None => problems.push(format!("`{head}` が見つからない")),
        }
    }
    if !render.contains("std::env::var_os(\"TAKO_2757_LEGACY\")") {
        problems.push("`TAKO_2757_LEGACY` を読む口が無い（A/B）".into());
    }
    problems
}

/// 規則 3: 背の高い行を描いた瞬間に覚え、描き直しの前に鍵を合わせるか
fn memo_problems(render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::without_comments(render);
    let records = code.match_indices("*slot = Some(layout);").any(|(at, _)| {
        code[at..]
            .find("note_row_height(")
            .is_some_and(|offset| offset < 400)
    });
    if !records {
        problems.push(
            "行が自分の枠へ実寸を書く所（`*slot = Some(layout);`）の隣で `note_row_height(` を呼んでいない\
             （器の外へ出た背の高い行の実寸が残らず、確かめ直しが 1 行ぶんと見積もって往復する）"
                .into(),
        );
    }
    match body_of(render, "pub(crate) fn render_preview_pane(") {
        Some(body)
            if before(
                body.find("self.renew_row_height_memo("),
                body.find("self.preview_text_layouts.insert(pane_id, vec![None; count]);"),
            ) => {}
        _ => problems.push(
            "`render_preview_pane` が本文を組み直す前に `renew_row_height_memo(` を呼んでいない\
             （行の版・文字サイズ・器の幅が変わった後も古い実寸を使う）"
                .into(),
        ),
    }
    problems
}

/// 規則 4: visual-test editor-font がページ移動を確かめ直しの後に設計の式で判定するか
fn section_problems(main: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(main, "async fn editor_font_visual(") {
        Some(body) => {
            match block_range(&body, "let probe = |cx: &mut AsyncApp, pane: PaneId| -> Probe") {
                Some(range) if body[range.clone()].contains("preview_follow_recheck.contains_key(") => {}
                _ => problems.push(
                    "editor-font の `probe` が追従の確かめ直し（`preview_follow_recheck`）を待たずに 1 フレームで\
                     測る（寄せ直す前の着地を測って余白を読み違える）"
                        .into(),
                ),
            }
            for needle in ["page_step(", "landing_margin(", "page_premise("] {
                if !body.contains(needle) {
                    problems.push(format!(
                        "editor-font のページ移動の判定が `{needle}` を通らない（歩幅は押す前の可視 − 1・最低 1 行、\
                         余白は着地した位置の可視の詰めた余白 = #1652 / #2714）"
                    ));
                }
            }
            if body.contains("== Some(FOLLOW_MARGIN)") {
                problems.push(
                    "editor-font が余白に `Some(FOLLOW_MARGIN)` を直に期待している（狭い器では #2714 の詰めた\
                     余白 = `landing_margin`）"
                        .into(),
                );
            }
        }
        None => problems.push("`async fn editor_font_visual(` が見つからない".into()),
    }
    problems
}

#[test]
fn カーソル行を器に入れる規則はfollow_cursorの1か所にある() {
    let problems = core_problems(&read(SCROLL));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 追従と確かめ直しは同じ可視範囲から同じ値を受け取る() {
    let problems = viewport_problems(&read(RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 背の高い行は描いた瞬間に覚え描き直しの前に鍵を合わせる() {
    let problems = memo_problems(&read(RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn editor_fontはページ移動を確かめ直しの後に設計の式で判定する() {
    let problems = section_problems(&read(MAIN));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 修正前の形を名指しできる() {
    let scroll = read(SCROLL);
    let render = read(RENDER);
    let main = read(MAIN);
    assert!(core_problems(&scroll).is_empty(), "今の本文は通る");
    assert!(viewport_problems(&render).is_empty(), "今の本文は通る");
    assert!(memo_problems(&render).is_empty(), "今の本文は通る");
    assert!(section_problems(&main).is_empty(), "今の本文は通る");

    // 1. 追従がカーソル行の実寸を見ない（修正前の follow_cursor）
    let no_fit = scroll.replacen("match view.fit_top {", "match None::<usize> {", 1);
    assert_ne!(no_fit, scroll, "注入先（fit_top の収め）が本文に在る");
    assert!(
        core_problems(&no_fit)
            .iter()
            .any(|p| p.contains("`follow_cursor` が `view.fit_top`")),
        "fit_top を見ない追従を名指せる"
    );
    // 2. 確かめ直しが自分の規則を足す（確かめ直しの本文の中だけを書き換える）
    let own_rule = {
        let code = code_view::code_view(&render);
        let range =
            block_range(&code, "pub(crate) fn recheck_preview_follow(").expect("確かめ直しの本文");
        let body = render[range.clone()].replacen(
            "let Some(first) =",
            "let _guard = view.fit_top;\n        let Some(first) =",
            1,
        );
        format!("{}{}{}", &render[..range.start], body, &render[range.end..])
    };
    assert_ne!(own_rule, render, "注入先（確かめ直しの判断）が本文に在る");
    assert!(
        viewport_problems(&own_rule)
            .iter()
            .any(|p| p.contains("recheck_preview_follow(` が `fit_top`")),
        "自分の規則を足した確かめ直しを名指せる"
    );
    //    可視範囲に fit_top を載せない（修正前の preview_cursor_viewport）
    let no_viewport = render.replacen("tako_core::editor_scroll::fit_top(", "fixed_top(", 1);
    assert_ne!(
        no_viewport, render,
        "注入先（fit_top を出す呼び出し）が本文に在る"
    );
    assert!(
        viewport_problems(&no_viewport)
            .iter()
            .any(|p| p.contains("editor_scroll::fit_top(")),
        "fit_top を出さない可視範囲を名指せる"
    );
    // 3. 背の高い行を覚えない
    let no_memo = render.replacen("app.note_row_height(*pane_id, *ix, height, single);", "", 1);
    assert_ne!(
        no_memo, render,
        "注入先（実寸を覚える呼び出し）が本文に在る"
    );
    assert!(
        memo_problems(&no_memo)
            .iter()
            .any(|p| p.contains("note_row_height(")),
        "背の高い行を覚えない形を名指せる"
    );
    // 4. editor-font が 1 フレームで測り、余白に FOLLOW_MARGIN を直に期待する（修正前の節）
    let one_frame = main.replacen(
        "app.preview_follow_recheck.contains_key(&pane)",
        "app.preview_text_layouts.contains_key(&pane)",
        1,
    );
    assert_ne!(one_frame, main, "注入先（確かめ直しを待つ）が本文に在る");
    assert!(
        section_problems(&one_frame)
            .iter()
            .any(|p| p.contains("`probe` が追従の確かめ直し")),
        "1 フレームで測る probe を名指せる"
    );
    let fixed_margin = main.replacen(
        "p1.below() == Some(landing_margin(&p1))",
        "p1.below() == Some(FOLLOW_MARGIN)",
        1,
    );
    assert_ne!(fixed_margin, main, "注入先（着地の余白の判定）が本文に在る");
    assert!(
        section_problems(&fixed_margin)
            .iter()
            .any(|p| p.contains("`Some(FOLLOW_MARGIN)`")),
        "余白を直に期待する判定を名指せる"
    );
}

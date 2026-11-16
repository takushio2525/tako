//! **#2751 の番犬**: 言語サーバの帯（未導入・取得の失敗・止まった・前提が未設定）を、高さの上限なしで
//! 本文の上に積む形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 編集に入った文書の言語サーバがこの機に無いと、ペインの頭に帯（見出し・理由・次の一手・「入れる」）が
//! 出る。帯の高さに上限が無かったので、狭いペイン（全節実行の visual-test では幅 240px）では帯の文が
//! 高さいっぱいまで折り返し、本文の行が 1 行も描かれなかった（計装の値は `layouts=0/3`・`text_bounds n=0`）。
//! 理由の文に長いパス（差し替え口 `TAKO_LSP_BIN_<ID>` が指す一時のパス）が入ると折り返しがさらに増える。
//! 利用者から見ると、狭いペインで .py を開いて編集に入るとコードが帯に全部隠れる。
//!
//! いまは帯の文（見出し + 理由 + 次の一手）の器が高さの上限（`bar_text_max_h` = ペインの高さの 1/3 まで・
//! 本文がペインの高さの半分を下回らない範囲）を持ち、超えた分は器の中でスクロールして読む。ボタンの行は
//! 器の外 = 常に見えて押せる。
//!
//! ## ここで止めるもの
//!
//! 1. [`帯の文の器は高さに上限を置き中でスクロールする`] — `render_lsp_status_bar` の文の器
//!    （`"preview-lsp-text"`）に `max_h(px(bar_text_max_h(pane_h)))`・`overflow_y_scroll()`・
//!    `track_scroll(&scroll)` があり、ボタンの行（`.children(button)`）は器の外（帯の直下の子）。
//!    A/B（`legacy_2751()` / `TAKO_2751_LEGACY`）を読む
//! 2. [`上限はペインの高さの割合と本文の半分で決まる`] — `bar_text_max_h` が `BAR_TEXT_MAX_SHARE`（1/3 以下）と
//!    「本文の半分」（`pane_h * 0.5` から帯の外側と余白を引く）の小さい方を採り、呼び出し元
//!    （`preview_render.rs`）が描画の矩形の高さ（`area.size.height` = テキスト領域）を渡し、帯がそれを
//!    `pane_height_of_area` でペイン全体の高さへ戻してから上限を決める（テキスト領域のまま割ると上限が
//!    54px ぶん小さくなり、広いペインの帯まで削る = 初版の visual-test ⑤ で落ちた形）
//! 3. [`visual_testの節が狭いペインで本文の行と読める押せるを測る`] — 節 `lsp-band-2751` が単独実行の腕と
//!    全節実行の両方に載り、本文の行・旧の形の再現・全文が読める・「入れる」が押せる・広いペインの画素を確かめる
//! 4. [`修正前の形を名指しできる`] — 修正前の形（上限なし・割合の広げすぎ・ボタンを器の中へ・節を外す）を
//!    注入すると 1〜3 のどれかが名指しで落ちる
//!
//! ## 相方
//!
//! 上限の式そのもの（ペインの高さ 226〜2400px で本文が半分以上残る）は `lsp_status_ui` の単体テスト。
//! 実 GUI の A/B は `scripts/test-lsp-band-2751.sh`（`TAKO_2751_LEGACY=1` は「①: 本文の行がペインの高さの
//! 半分以上描かれる」で FAILED）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const STATUS_UI: &str = "crates/tako-app/src/lsp_status_ui.rs";
const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const SECTION: &str = "crates/tako-app/src/self_test/lsp_band_2751.rs";

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

/// `open` の位置の `(` と対の `)` までの範囲（`code` はコメントを潰した眺め）
fn paren_range(code: &str, open: usize) -> Option<std::ops::Range<usize>> {
    let mut depth = 0usize;
    for (i, b) in code.as_bytes()[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
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

/// `const NAME: f32 = <数>;` の数
fn const_f32(src: &str, name: &str) -> Option<f32> {
    let head = format!("const {name}: f32 = ");
    let at = src.find(&head)? + head.len();
    let end = at + src[at..].find(';')?;
    src[at..end].trim().replace('_', "").parse().ok()
}

/// 規則 1: 帯の文の器は高さに上限を置き、中でスクロールするか。ボタンの行は器の外か
fn cap_problems(status_ui: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(body) = body_of(status_ui, "pub(crate) fn render_lsp_status_bar(") else {
        return vec!["`fn render_lsp_status_bar(` が見つからない".into()];
    };
    // 文の器の要素（`.child(` の中の `div()` から対の `)` まで）
    match body.find(".id((\"preview-lsp-text\", key))") {
        Some(at) => {
            let open = body[..at].rfind(".child(").map(|c| c + ".child".len());
            let element = open
                .and_then(|open| paren_range(&body, open))
                .map_or(&body[at..], |range| &body[range]);
            for (needle, why) in [
                (
                    ".max_h(px(bar_text_max_h(pane_h)))",
                    "高さの上限（狭いペインで本文の行を押し出さない）",
                ),
                (
                    ".overflow_y_scroll()",
                    "超えた分は器の中でスクロールして読む",
                ),
                (
                    ".track_scroll(&scroll)",
                    "文が変わったら頭へ戻せる・つまみの位置",
                ),
                (".child(title_el)", "見出しも器の中"),
                (".children(detail_els)", "理由と次の一手も器の中"),
            ] {
                if !element.contains(needle) {
                    problems.push(format!(
                        "帯の文の器（`preview-lsp-text`）に `{needle}` が無い（{why}）"
                    ));
                }
            }
            if element.contains("button") {
                problems.push(
                    "ボタンが帯の文の器（スクロールの器）の中にある（はみ出すと「入れる」が隠れて押せない）"
                        .into(),
                );
            }
        }
        None => {
            problems.push("帯の文の器（`.id((\"preview-lsp-text\", key))`）が見つからない".into())
        }
    }
    // ボタンの行は帯（`preview-lsp-status`）の直下の子
    match body.find(".id((\"preview-lsp-status\", key))") {
        Some(at) if body[at..].contains(".child(row)") => {}
        _ => problems.push(
            "ボタンの行（`row`）が帯（`preview-lsp-status`）の直下の子でない（文の器の外に置いて常に見せる）"
                .into(),
        ),
    }
    if !body.contains("legacy_2751()") {
        problems.push("`render_lsp_status_bar` が A/B（`legacy_2751()`）を読まない".into());
    }
    if !status_ui.contains("std::env::var(\"TAKO_2751_LEGACY\")") {
        problems.push("`TAKO_2751_LEGACY` を読む口が無い（A/B）".into());
    }
    problems
}

/// 規則 2: 上限はペインの高さの割合と本文の半分で決まり、呼び出し元がペインの高さを渡すか
fn formula_problems(status_ui: &str, preview_render: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match const_f32(status_ui, "BAR_TEXT_MAX_SHARE") {
        Some(share) if share > 0.0 && share <= 1.0 / 3.0 => {}
        Some(share) => problems.push(format!(
            "`BAR_TEXT_MAX_SHARE` が {share}（0 より大きく 1/3 以下の外 = 狭いペインで帯が本文を押し出す）"
        )),
        None => problems.push("`const BAR_TEXT_MAX_SHARE: f32` が無い".into()),
    }
    match body_of(status_ui, "pub(crate) fn bar_text_max_h(") {
        Some(body) => {
            for (needle, why) in [
                ("pane_h * BAR_TEXT_MAX_SHARE", "ペインの高さの割合"),
                ("pane_h * 0.5", "本文がペインの高さの半分を下回らない"),
                ("BAR_CHROME_PX", "帯の外側（ボタンの行）の高さ"),
                ("PANE_RESERVED_PX", "タイトルバーと本文の余白"),
                (".min(half)", "割合と本文の半分の小さい方"),
            ] {
                if !body.contains(needle) {
                    problems.push(format!("`bar_text_max_h` に `{needle}` が無い（{why}）"));
                }
            }
        }
        None => problems.push("`fn bar_text_max_h(` が無い（帯の文の器の高さの上限）".into()),
    }
    match body_of(status_ui, "pub(crate) fn render_lsp_status_bar(") {
        Some(body) if body.contains("let pane_h = pane_height_of_area(area_h);") => {}
        _ => problems.push(
            "`render_lsp_status_bar` が描画の矩形（テキスト領域）の高さを `pane_height_of_area` でペイン全体の\
             高さへ戻していない（上限が小さくなり広いペインの帯まで削る）"
                .into(),
        ),
    }
    match body_of(status_ui, "pub(crate) fn pane_height_of_area(") {
        Some(body)
            if ["PANE_BORDER", "PANE_PADDING", "PANE_TITLE_BAR"]
                .iter()
                .all(|c| body.contains(c)) => {}
        _ => problems.push(
            "`pane_height_of_area` が枠・余白・タイトルバーを足し戻していない（`pane_text_area_rect` の逆）".into(),
        ),
    }
    // 整形で改行が入っても同じに読めるよう、空白を落とした眺めで突き合わせる
    let squashed: String = code_view::without_comments(preview_render)
        .split_whitespace()
        .collect();
    if !squashed.contains("self.render_lsp_status_bar(pane_id,f32::from(area.size.height),") {
        problems.push(
            "`preview_render.rs` が `render_lsp_status_bar` へペインの高さ（`area.size.height`）を渡していない"
                .into(),
        );
    }
    problems
}

/// 規則 3: visual-test の節が単独・全節の両方に載り、狭いペインの本文の行と読める・押せるを確かめるか
fn section_problems(main: &str, section: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::without_comments(main);
    if !code.contains("\"lsp-band-2751\" => {") {
        problems.push("単独実行の腕 `\"lsp-band-2751\"` が無い（TAKO_VISUAL_ONLY）".into());
    }
    // 全節実行の並び（`tree_notice_2760_visual` の後ろ・`flicker_visual` の前）。単独実行の腕の後ろにも
    // 近くに呼び出しがあるので、間に腕の出口（`visual_exit(`）を挟まないものだけを数える
    let full_run = code
        .match_indices("tree_notice_2760_visual(any, window, cx).await;")
        .any(|(at, _)| {
            code[at..]
                .find("lsp_band_2751_visual(any, window, cx).await;")
                .is_some_and(|offset| {
                    offset < 300 && !code[at..at + offset].contains("visual_exit(")
                })
        });
    if !full_run {
        problems.push("全節実行の並びに `lsp_band_2751_visual` が無い（#948）".into());
    }
    let body = code_view::without_comments(section);
    for (needle, why) in [
        (
            "inject_section_failure(SECTION)",
            "全節実行に載っているかの注入（#948）",
        ),
        (
            "本文の行がペインの高さの半分以上描かれる",
            "狭いペインの本文の行",
        ),
        ("旧の形では本文の行が 0", "場面が #2751 の症状を再現する"),
        ("全文がその場で読める", "器の中のスクロール"),
        (
            "「入れる」が帯・ペイン・窓の中に見えている",
            "ボタンが押せる場所にある",
        ),
        ("広いペインの見た目は旧の形と同じ", "広いペインの画素"),
        (
            "set_legacy_2751(env_legacy)",
            "節を抜けるときに起動時の腕へ戻す",
        ),
    ] {
        if !body.contains(needle) {
            problems.push(format!("節 lsp-band-2751 に `{needle}` が無い（{why}）"));
        }
    }
    problems
}

#[test]
fn 帯の文の器は高さに上限を置き中でスクロールする() {
    let problems = cap_problems(&read(STATUS_UI));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 上限はペインの高さの割合と本文の半分で決まる() {
    let problems = formula_problems(&read(STATUS_UI), &read(PREVIEW_RENDER));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn visual_testの節が狭いペインで本文の行と読める押せるを測る() {
    let problems = section_problems(&read(MAIN), &read(SECTION));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 修正前の形を名指しできる() {
    let status_ui = read(STATUS_UI);
    let preview_render = read(PREVIEW_RENDER);
    let main = read(MAIN);
    let section = read(SECTION);
    assert!(cap_problems(&status_ui).is_empty(), "今の本文は通る");
    assert!(
        formula_problems(&status_ui, &preview_render).is_empty(),
        "今の本文は通る"
    );
    assert!(
        section_problems(&main, &section).is_empty(),
        "今の本文は通る"
    );

    // 1. 帯の文の器に上限を置かない（修正前 = 狭いペインで帯が本文を押し出す）
    let uncapped = status_ui.replacen(".max_h(px(bar_text_max_h(pane_h)))", "", 1);
    assert_ne!(uncapped, status_ui, "注入先（上限）が本文に在る");
    assert!(
        cap_problems(&uncapped)
            .iter()
            .any(|p| p.contains("bar_text_max_h(pane_h)")),
        "上限の無い帯を名指せる"
    );
    // 1'. スクロールしない（はみ出した文が読めない）
    let clipped = status_ui.replacen(".overflow_y_scroll()", "", 1);
    assert_ne!(clipped, status_ui, "注入先（スクロール）が本文に在る");
    assert!(
        cap_problems(&clipped)
            .iter()
            .any(|p| p.contains("overflow_y_scroll")),
        "スクロールしない器を名指せる"
    );
    // 1''. ボタンを文の器の中へ入れる（はみ出すと「入れる」が隠れる）
    let buried = status_ui.replacen(
        ".children(detail_els),\n                    )",
        ".children(detail_els)\n                            .children(button),\n                    )",
        1,
    );
    assert_ne!(buried, status_ui, "注入先（文の器の最後の子）が本文に在る");
    assert!(
        cap_problems(&buried)
            .iter()
            .any(|p| p.contains("ボタンが帯の文の器")),
        "器の中のボタンを名指せる"
    );
    // 2. 割合を広げすぎる（帯がペインの大半を取る）
    let tall = status_ui.replacen(
        "const BAR_TEXT_MAX_SHARE: f32 = 0.33;",
        "const BAR_TEXT_MAX_SHARE: f32 = 0.9;",
        1,
    );
    assert_ne!(tall, status_ui, "注入先（割合）が本文に在る");
    assert!(
        formula_problems(&tall, &preview_render)
            .iter()
            .any(|p| p.contains("BAR_TEXT_MAX_SHARE")),
        "広すぎる割合を名指せる"
    );
    // 2'. 本文の半分の守りを外す（低いペインで帯が本文を押し出す）
    let no_half = status_ui.replacen(
        "share.min(half).max(BAR_LINE_PX)",
        "share.max(BAR_LINE_PX)",
        1,
    );
    assert_ne!(no_half, status_ui, "注入先（本文の半分）が本文に在る");
    assert!(
        formula_problems(&no_half, &preview_render)
            .iter()
            .any(|p| p.contains(".min(half)")),
        "本文の半分の守りが無い式を名指せる"
    );
    // 2''. テキスト領域の高さのまま割る（初版 = 上限が小さく、広いペインの帯まで削る）
    let raw_area = status_ui.replacen(
        "let pane_h = pane_height_of_area(area_h);",
        "let pane_h = area_h;",
        1,
    );
    assert_ne!(
        raw_area, status_ui,
        "注入先（ペインの高さへ戻す）が本文に在る"
    );
    assert!(
        formula_problems(&raw_area, &preview_render)
            .iter()
            .any(|p| p.contains("pane_height_of_area")),
        "テキスト領域のまま割る形を名指せる"
    );
    // 3. 節が全節実行に載らない
    // 単独実行の腕の行（字下げが深い）にも同じ字面があるので、全節実行のコメント行ごと外す
    let unlisted = main.replacen(
        "            // #2751: 言語サーバの未導入の帯が狭いペインでも本文の行を押し出さない\n            \
         lsp_band_2751_visual(any, window, cx).await;\n",
        "",
        1,
    );
    assert_ne!(unlisted, main, "注入先（全節実行の並び）が本文に在る");
    assert!(
        section_problems(&unlisted, &section)
            .iter()
            .any(|p| p.contains("全節実行")),
        "全節実行に載らない節を名指せる"
    );
}

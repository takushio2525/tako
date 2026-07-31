//! Markdown ブロックの描画（Issue #656 の基盤を #690 で共有化）
//!
//! プレビューペイン（`preview_render`）とアップデート詳細画面（`update_window`）は、
//! 同じ md を**同じ見た目**で出す必要がある。そこで
//!
//! - **幾何（余白・サイズ・罫線）と md 由来の装飾**は [`render_block`] に 1 本化し、
//! - **インタラクション層の差**（選択・検索ハイライト・`TextLayout` の控え・
//!   コードブロックのコピーボタン）だけを [`MdTextSink`] で受ける
//!
//! という分け方にした。見た目の一貫性を「気をつける」ではなく構造で担保する狙いで、
//! ライトテーマの構文色を 1 関数へ寄せた #669 と同じ考え方。
//!
//! パースは `preview::markdown_blocks`（`pulldown-cmark`）が正で、この層は
//! `Vec<MdBlock>` を GPUI 要素へ落とすだけ。ブロック 1 個 = 要素 1 個の対応は
//! 目次ジャンプ（#232）が依存しているので崩さない。

use gpui::{
    div, prelude::*, px, relative, AnyElement, FontStyle, FontWeight, HighlightStyle, SharedString,
    StrikethroughStyle, StyledText, TextStyle, UnderlineStyle,
};
use std::ops::Range;
use tako_core::theme::Theme;

use crate::preview::{self, MdBlock, MdBlockKind, MdCell, MdSpan};
use crate::{hsla, hsla_alpha, merge_highlights};

/// md 1 行分のテキストを要素へ変換する受け皿。
///
/// 呼び出し順は `md_block_line_texts` の並び（表はヘッダ → 各行のセルの行優先）と
/// 一致する。プレビューの選択行番号がそのまま呼び出し順の添字になるので、
/// 実装側は自前のカウンタで行を突き合わせられる。
pub(crate) trait MdTextSink {
    /// `highlights` は md 由来の装飾（強調・インラインコード・リンク・構文色）だけ。
    /// 実装側は選択・検索・ホバーのハイライトをこの後ろへ重ねる（後の指定が勝つ）
    fn text(
        &mut self,
        text: String,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        color: tako_core::Rgb,
        weight: Option<FontWeight>,
    ) -> AnyElement;

    /// 文字を持たないブロック（罫線）。行番号の対応を保つために通知する
    fn spacer(&mut self) {}

    /// コードブロックの装飾（プレビューのコピーボタン。#680）。既定は無し
    fn code_overlay(&mut self, _index: usize) -> Option<MdCodeOverlay> {
        None
    }
}

/// コードブロックに重ねる装飾。`group` はホバー連動のためコンテナへ付ける名前
pub(crate) struct MdCodeOverlay {
    pub group: SharedString,
    pub element: AnyElement,
}

/// Markdown 用のテキスト既定スタイル。`StyledText` の run は色とフォントを焼き込むため
/// （サイズと行高だけが親要素から継承される）、文字色・太さはここで渡す必要がある
pub(crate) fn md_text_style(
    theme: &Theme,
    color: tako_core::Rgb,
    weight: Option<FontWeight>,
) -> TextStyle {
    TextStyle {
        color: hsla(color),
        font_family: SharedString::from(theme.font_family.clone()),
        font_size: px(theme.font_size).into(),
        line_height: px(theme.line_height).into(),
        font_weight: weight.unwrap_or_default(),
        ..TextStyle::default()
    }
}

/// テキスト 1 本を `StyledText` へ。
///
/// 空文字は空白 1 個に置き換える（空セル・空項目でも 1 行分の高さと選択の
/// 当たり判定を残す）。ハイライトの重ね合わせもここで確定させるので、
/// 受け皿の実装が増えてもこの規則は 1 か所で守られる
pub(crate) fn styled_line(
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    style: &TextStyle,
) -> StyledText {
    StyledText::new(pad_empty(text)).with_default_highlights(style, merge_highlights(highlights))
}

/// 空文字を空白 1 個へ（[`styled_line`] の規則。単体で検証できるよう分けてある）
fn pad_empty(mut text: String) -> String {
    if text.is_empty() {
        text.push(' ');
    }
    text
}

/// インライン列 1 本を「プレーンテキスト + md 由来のハイライト」へ落とす（#656 / #680）。
///
/// 返すテキストは選択・検索・リンクのヒットテストが使う座標系そのもの
/// （`md_block_line_texts` と同じ文字列）。
pub(crate) fn inline_highlights(
    spans: &[MdSpan],
    theme: &Theme,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let is_link = span.is_link();
        let styled = span.bold || span.italic || span.code || span.strike || is_link;
        if !styled {
            continue;
        }
        highlights.push((
            start..text.len(),
            HighlightStyle {
                color: if span.code {
                    Some(hsla(theme.peach))
                } else if is_link {
                    Some(hsla(theme.accent))
                } else if span.strike {
                    Some(hsla(theme.text_muted))
                } else {
                    None
                },
                background_color: span.code.then(|| hsla_alpha(theme.surface_highlight, 0.75)),
                font_weight: span.bold.then_some(FontWeight::BOLD),
                font_style: span.italic.then_some(FontStyle::Italic),
                underline: is_link.then(|| UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(hsla_alpha(theme.accent, 0.6)),
                    wavy: false,
                }),
                strikethrough: span.strike.then_some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(hsla(theme.text_muted)),
                }),
                ..HighlightStyle::default()
            },
        ));
    }
    (text, highlights)
}

/// ⌘+ホバー中のリンクだけ「押せる」ことが分かる装飾へ強める（#680）。
///
/// ターミナル内リンク（#153）と同じ = 下線を実線化 + accent 背景をリンク文字列だけに
/// 限定する。`merge_highlights` は後の指定が勝つので、md 由来のハイライトの**後ろ**へ積む
pub(crate) fn push_hovered_link_highlight(
    highlights: &mut Vec<(Range<usize>, HighlightStyle)>,
    text: &str,
    range: &Range<usize>,
    theme: &Theme,
) {
    let end = range.end.min(text.len());
    if range.start >= end {
        return;
    }
    highlights.push((
        range.start..end,
        HighlightStyle {
            color: Some(hsla(theme.accent)),
            background_color: Some(hsla_alpha(theme.accent, 0.18)),
            underline: Some(UnderlineStyle {
                thickness: px(1.5),
                color: Some(hsla(theme.accent)),
                wavy: false,
            }),
            ..HighlightStyle::default()
        },
    ));
}

/// コードブロック本体を「プレーンテキスト + 構文色ハイライト」へ落とす。
///
/// 言語指定なしフェンスは syntect の既定色（ダーク前提の淡色）を使わず、テーマの
/// 本文色で素直に出す。色を出す場合はライトの面で読める明度へ落とす
/// （#656 / #669。非 md のコードプレビューと同一経路）
pub(crate) fn code_block_highlights(
    lang: Option<&str>,
    lines: &[preview::Line],
    theme: &Theme,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for (line_i, line) in lines.iter().enumerate() {
        if line_i > 0 {
            text.push('\n');
        }
        for span in line {
            let start = text.len();
            text.push_str(&span.text);
            let color = lang.and(span.color).map(|c| theme.adapt_syntax_color(c));
            if color.is_some() || span.bold || span.italic {
                highlights.push((
                    start..text.len(),
                    HighlightStyle {
                        color: color.map(hsla),
                        font_weight: span.bold.then_some(FontWeight::BOLD),
                        font_style: span.italic.then_some(FontStyle::Italic),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
    }
    (text, highlights)
}

/// 見出しレベル 1 本ぶんの見た目（サイズ・色・太さ・下罫線）。
/// レベル差はサイズ・太さ・色の 3 点で付け、H1 / H2 は下罫線で区切る
pub(crate) struct HeadingLook {
    pub size: f32,
    pub color: tako_core::Rgb,
    pub weight: FontWeight,
    /// 下罫線の色（H3 以下は罫線なし）
    pub rule: Option<tako_core::Rgb>,
}

pub(crate) fn heading_look(
    level: u8,
    base: f32,
    theme: &Theme,
    body_color: tako_core::Rgb,
) -> HeadingLook {
    let scale = match level {
        1 => 1.65,
        2 => 1.38,
        3 => 1.18,
        4 => 1.06,
        5 => 1.0,
        _ => 0.94,
    };
    HeadingLook {
        size: base * scale,
        color: match level {
            1..=3 => body_color,
            4 => theme.text_secondary,
            5 => theme.text_tertiary,
            _ => theme.text_muted,
        },
        weight: if level <= 2 {
            FontWeight::EXTRA_BOLD
        } else {
            FontWeight::BOLD
        },
        rule: match level {
            1 => Some(theme.border_heavy),
            2 => Some(theme.border_default),
            _ => None,
        },
    }
}

/// md ブロック 1 個を描く（**幾何とテーマ色の唯一の実装**）。
///
/// `code_index` はコードブロックの出現順（0 始まり）。`None` を渡すと
/// コピーボタン等の装飾を要求しない。
pub(crate) fn render_block(
    theme: &Theme,
    block: &MdBlock,
    code_index: Option<usize>,
    sink: &mut impl MdTextSink,
) -> AnyElement {
    let base = theme.font_size;
    let in_quote = block.quote_depth > 0;
    // 引用の中は本文より一段淡く。引用の入れ子はさらに淡くして深さが分かるようにする
    let body_color = if in_quote {
        theme.text_secondary
    } else {
        theme.foreground
    };

    let element: AnyElement = match &block.kind {
        MdBlockKind::Heading { level, spans } => {
            let look = heading_look(*level, base, theme, body_color);
            let (text, highlights) = inline_highlights(spans, theme);
            let styled = sink.text(text, highlights, look.color, Some(look.weight));
            div()
                .relative()
                .flex_shrink_0()
                .pt(px(base * if *level == 1 { 1.1 } else { 0.85 }))
                .pb(px(base * 0.35))
                .text_size(px(look.size))
                .line_height(px(look.size * 1.4))
                .when_some(look.rule, |d, color| {
                    d.mb(px(base * 0.25))
                        .border_b_1()
                        .border_color(hsla_alpha(color, 0.9))
                })
                .child(styled)
                .into_any_element()
        }
        MdBlockKind::Paragraph { spans } => {
            let (text, highlights) = inline_highlights(spans, theme);
            let styled = sink.text(text, highlights, body_color, None);
            div()
                .relative()
                .flex_shrink_0()
                .py(px(base * 0.3))
                .line_height(px(base * 1.7))
                .child(styled)
                .into_any_element()
        }
        MdBlockKind::ListItem {
            ordered,
            task,
            continuation,
            spans,
        } => {
            let line_height = base * 1.7;
            let step = base * 1.45;
            let marker_width = base * 1.55;
            let (text, highlights) = inline_highlights(spans, theme);
            let styled = sink.text(text, highlights, body_color, None);
            // マーカー列: 行高と同じ高さの箱に入れて 1 行目の中央へ合わせる
            let marker = div()
                .flex_none()
                .w(px(marker_width))
                .h(px(line_height))
                .flex()
                .items_center()
                .justify_end()
                .pr(px(base * 0.35))
                .children(
                    (!*continuation).then(|| list_marker(theme, base, block, *task, *ordered)),
                );
            div()
                .relative()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_start()
                .py(px(base * 0.12))
                .pl(px(step * block.list_depth.saturating_sub(1) as f32))
                .line_height(px(line_height))
                .child(marker)
                .child(div().flex_1().min_w(px(0.0)).child(styled))
                .into_any_element()
        }
        MdBlockKind::CodeBlock { lang, lines } => {
            let (text, highlights) = code_block_highlights(lang.as_deref(), lines, theme);
            let styled = sink.text(text, highlights, theme.text_secondary, None);
            let overlay = code_index.and_then(|index| sink.code_overlay(index));
            div()
                .relative()
                .flex_shrink_0()
                .when_some(overlay.as_ref().map(|o| o.group.clone()), |d, group| {
                    d.group(group)
                })
                .my(px(base * 0.5))
                .px(px(base * 0.8))
                .py(px(base * 0.55))
                .rounded_md()
                .border_1()
                .border_color(hsla(theme.border_subtle))
                .bg(hsla(theme.mantle))
                .text_size(px(base * 0.95))
                .line_height(px(base * 1.45))
                .child(styled)
                .children(overlay.map(|o| o.element))
                .into_any_element()
        }
        MdBlockKind::Table {
            align,
            header,
            rows,
        } => render_table(theme, block, align, header, rows, sink),
        MdBlockKind::Rule => {
            sink.spacer();
            div()
                .relative()
                .flex_shrink_0()
                .my(px(base * 0.9))
                .h(px(1.0))
                .bg(hsla(theme.border_heavy))
                .into_any_element()
        }
    };

    // 引用は「ブロックを包む帯」で表す。連続する引用ブロックは隣接して 1 本に見える
    let mut element = element;
    for level in 0..block.quote_depth {
        let outermost = level + 1 == block.quote_depth;
        element = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pl(px(base * 0.85))
            .py(px(base * 0.15))
            .border_l_2()
            .border_color(hsla_alpha(
                if outermost {
                    theme.accent_muted
                } else {
                    theme.text_faint
                },
                0.85,
            ))
            .when(outermost, |d| d.bg(hsla_alpha(theme.surface_0, 0.6)))
            .child(element)
            .into_any_element();
    }
    element
}

/// 読むだけの md 受け皿（アップデート詳細画面のリリースノート。#690）。
///
/// 選択・検索・コピーボタンは持たず、⌘+クリックでリンクを開くための `TextLayout` だけ
/// 控える。控える並びは `md_block_line_texts` と同じなので `md_document_links` の
/// 行番号がそのまま添字になる（プレビューと同じ座標系）。
pub(crate) struct ReadOnlyMdSink<'a> {
    theme: &'a Theme,
    /// ⌘ 押下中にホバーしているリンク（行番号 + バイト範囲）
    hovered: Option<(usize, Range<usize>)>,
    line: usize,
    layouts: Vec<Option<gpui::TextLayout>>,
}

impl MdTextSink for ReadOnlyMdSink<'_> {
    fn text(
        &mut self,
        text: String,
        mut highlights: Vec<(Range<usize>, HighlightStyle)>,
        color: tako_core::Rgb,
        weight: Option<FontWeight>,
    ) -> AnyElement {
        if let Some((_, range)) = self.hovered.as_ref().filter(|(l, _)| *l == self.line) {
            push_hovered_link_highlight(&mut highlights, &text, range, self.theme);
        }
        let styled = styled_line(text, highlights, &md_text_style(self.theme, color, weight));
        self.layouts.push(Some(styled.layout().clone()));
        self.line += 1;
        styled.into_any_element()
    }

    fn spacer(&mut self) {
        self.layouts.push(None);
        self.line += 1;
    }
}

/// md 文書全体を読むだけの要素列へ落とす（#690）。
///
/// 返す `TextLayout` は「行番号 → 実描画レイアウト」の対応で、⌘+クリックの
/// ヒットテスト（`md_link_at_layouts`）が使う。
pub(crate) fn render_document(
    theme: &Theme,
    blocks: &[MdBlock],
    hovered: Option<(usize, Range<usize>)>,
) -> (Vec<AnyElement>, Vec<Option<gpui::TextLayout>>) {
    let mut sink = ReadOnlyMdSink {
        theme,
        hovered,
        line: 0,
        layouts: Vec::new(),
    };
    let mut code_blocks = 0usize;
    let mut elements = Vec::with_capacity(blocks.len());
    for block in blocks {
        // コードブロックの出現順（装飾を持たないので番号は使われないが、
        // プレビューと同じ引数で render_block を呼んで経路差を作らない）
        let code_index = matches!(block.kind, MdBlockKind::CodeBlock { .. }).then(|| {
            code_blocks += 1;
            code_blocks - 1
        });
        elements.push(render_block(theme, block, code_index, &mut sink));
    }
    (elements, sink.layouts)
}

/// 実描画レイアウトに対するリンクのヒットテスト（#680 と同じ規則。#690）。
///
/// 選択のヒットテストと違い「文字の上にあるか」が要るので
/// `TextLayout::index_for_position` の **Ok だけ**を採る（Err = 文字の外）。
/// 開けない URL（相対パス・`javascript:` 等）はホバーもクリックも対象にしない。
pub(crate) fn md_link_at_layouts(
    links: &[crate::MdLinkHit],
    layouts: &[Option<gpui::TextLayout>],
    position: gpui::Point<gpui::Pixels>,
) -> Option<usize> {
    for (index, hit) in links.iter().enumerate() {
        if tako_core::md_links::browser_url(&hit.url).is_none() {
            continue;
        }
        let Some(Some(layout)) = layouts.get(hit.line) else {
            continue;
        };
        if !layout.bounds().contains(&position) {
            continue;
        }
        let Ok(byte) = layout.index_for_position(position) else {
            continue;
        };
        if hit.range.contains(&byte) {
            return Some(index);
        }
    }
    None
}

/// リスト項目のマーカー（絵文字は使わず図形 + SVG。#217）
fn list_marker(
    theme: &Theme,
    base: f32,
    block: &MdBlock,
    task: Option<bool>,
    ordered: Option<u64>,
) -> AnyElement {
    match (task, ordered) {
        // タスクリストのチェックボックス
        (Some(done), _) => {
            let box_size = base * 0.85;
            div()
                .w(px(box_size))
                .h(px(box_size))
                .rounded(px(2.0))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(hsla(if done {
                    theme.green
                } else {
                    theme.border_heavy
                }))
                .when(done, |d| d.bg(hsla_alpha(theme.green, 0.9)))
                .children(done.then(|| {
                    gpui::svg()
                        .path(crate::file_icons::ui_icon::CHECK)
                        .w(px(box_size - 2.0))
                        .h(px(box_size - 2.0))
                        .text_color(hsla(theme.background))
                }))
                .into_any_element()
        }
        (None, Some(number)) => div()
            .text_size(px(base * 0.92))
            .text_color(hsla(theme.accent_muted))
            .child(SharedString::from(format!("{number}.")))
            .into_any_element(),
        (None, None) => {
            let dot = base * 0.35;
            match preview::md_bullet_for_depth(block.list_depth) {
                preview::MdBullet::Dot => div()
                    .w(px(dot))
                    .h(px(dot))
                    .rounded_full()
                    .bg(hsla(theme.text_muted)),
                preview::MdBullet::Ring => div()
                    .w(px(dot + 1.0))
                    .h(px(dot + 1.0))
                    .rounded_full()
                    .border_1()
                    .border_color(hsla(theme.text_muted)),
                preview::MdBullet::Square => div().w(px(dot)).h(px(dot)).bg(hsla(theme.text_muted)),
            }
            .into_any_element()
        }
    }
}

/// GFM テーブルを罫線つきグリッドで描く（Issue #656）。
/// セル 1 つが 1 行なので、受け皿の呼び出し順はヘッダ → 各行の行優先順になる
fn render_table(
    theme: &Theme,
    block: &MdBlock,
    align: &[preview::MdAlign],
    header: &[MdCell],
    rows: &[Vec<MdCell>],
    sink: &mut impl MdTextSink,
) -> AnyElement {
    let base = theme.font_size;
    let columns = align.len().max(header.len()).max(1);
    let shares = preview::md_table_column_shares(header, rows, columns);
    let mut table = div()
        .relative()
        // 縦に縮まないことを明示する。overflow_hidden を付けた要素は flex の
        // 自動最小サイズ（min-content 高さ）が無効になるため、これが無いと
        // 本文が長いときに親の flex 列が表を潰し、後続ブロックと重なる（#656 / #494）
        .flex_shrink_0()
        .my(px(base * 0.6))
        .flex()
        .flex_col()
        .rounded_md()
        .overflow_hidden()
        .border_1()
        .border_color(hsla(theme.border_default))
        .line_height(px(base * 1.5));

    // ヘッダ → 各本文行の順に組む（この順序が受け皿の呼び出し順 = 選択行番号の順）。
    // セルは flex_basis で列幅比を配り、min_w(0) + 既定の flex_shrink で
    // 狭いペインでも溢れさせない（折り返しで縦に伸びる）
    let body_rows = rows.len();
    for (row_i, cells) in std::iter::once(header)
        .chain(rows.iter().map(Vec::as_slice))
        .enumerate()
    {
        let is_header = row_i == 0;
        let zebra = !is_header && (row_i - 1) % 2 == 1;
        let last_row = !is_header && row_i == body_rows;
        let color = if is_header || block.quote_depth == 0 {
            theme.foreground
        } else {
            theme.text_secondary
        };
        let weight = is_header.then_some(FontWeight::BOLD);
        let mut row = div()
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_stretch()
            // ヘッダ帯は surface_highlight（背景階層のうち地色と明確に差が出る面）。
            // surface_0〜2 はダークの地色と 2/255 しか違わず帯として見えない
            .when(is_header, |d| {
                d.bg(hsla(theme.surface_highlight))
                    .border_b_1()
                    .border_color(hsla(theme.border_heavy))
            })
            .when(!is_header && !last_row, |d| {
                d.border_b_1().border_color(hsla(theme.border_inner))
            })
            .when(zebra, |d| d.bg(hsla_alpha(theme.surface_highlight, 0.35)));
        for column in 0..columns {
            let cell = cells.get(column).cloned().unwrap_or_default();
            let (text, highlights) = inline_highlights(&cell, theme);
            let styled = sink.text(text, highlights, color, weight);
            let alignment = align.get(column).copied().unwrap_or_default();
            row = row.child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    // 配置は flex の寄せで行う。StyledText 自身に text_align を
                    // 掛けると index_for_position が寄せ量を見ないため、
                    // クリック位置と文字位置がずれる（GPUI 実装由来）
                    .map(|d| match alignment {
                        preview::MdAlign::Center => d.justify_center(),
                        preview::MdAlign::Right => d.justify_end(),
                        _ => d.justify_start(),
                    })
                    .flex_basis(relative(shares.get(column).copied().unwrap_or(0.0)))
                    .min_w(px(0.0))
                    .px(px(base * 0.6))
                    .py(px(base * 0.35))
                    .when(column + 1 < columns, |d| {
                        d.border_r_1().border_color(hsla(theme.border_inner))
                    })
                    // StyledText を直接 flex 子にすると、GPUI のテキスト計測が
                    // min-content 幅として「折り返しなしの 1 行分」を返すため、
                    // flex の自動最小サイズで縮まず隣のセルへ溢れる。min_w(0) の
                    // 箱で包むと列幅まで縮み、テキストは列内で折り返す（#656）
                    .child(div().min_w(px(0.0)).child(styled)),
            );
        }
        table = table.child(row);
    }
    table.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::markdown_blocks;
    use tako_core::theme::ThemeMode;

    /// 受け皿の呼び出し順・引数を記録するだけの実装（描画要素は捨てる）
    #[derive(Default)]
    struct RecordingSink {
        /// 描いた 1 行ぶん（テキスト・ハイライト数・色・太さ）
        rows: Vec<(String, usize, tako_core::Rgb, Option<FontWeight>)>,
        spacers: usize,
        code_overlays: Vec<usize>,
    }

    impl MdTextSink for RecordingSink {
        fn text(
            &mut self,
            text: String,
            highlights: Vec<(Range<usize>, HighlightStyle)>,
            color: tako_core::Rgb,
            weight: Option<FontWeight>,
        ) -> AnyElement {
            self.rows
                .push((text.clone(), highlights.len(), color, weight));
            styled_line(text, highlights, &TextStyle::default()).into_any_element()
        }

        fn spacer(&mut self) {
            self.spacers += 1;
        }

        fn code_overlay(&mut self, index: usize) -> Option<MdCodeOverlay> {
            self.code_overlays.push(index);
            None
        }
    }

    fn render_all(md: &str, theme: &Theme) -> RecordingSink {
        let blocks = markdown_blocks(md);
        let mut sink = RecordingSink::default();
        let mut code = 0usize;
        for block in &blocks {
            let code_index = matches!(block.kind, MdBlockKind::CodeBlock { .. }).then(|| {
                code += 1;
                code - 1
            });
            let _ = render_block(theme, block, code_index, &mut sink);
        }
        sink
    }

    /// 受け皿が呼ばれる順序が `md_block_line_texts` の並びと一致する（#690 の前提）。
    /// ここがずれると選択・リンクの行番号が全部ずれる
    #[test]
    fn 受け皿の呼び出し順は選択行の並びと一致する() {
        let theme = Theme::default();
        let md = crate::preview::MARKDOWN_SHOWCASE;
        let blocks = markdown_blocks(md);
        let expected: Vec<String> = blocks.iter().flat_map(crate::md_block_line_texts).collect();
        let sink = render_all(md, &theme);
        // 罫線は文字を持たないので spacer で数え、テキスト行だけを突き合わせる
        let mut got = Vec::new();
        let mut rows = sink.rows.iter();
        for (block, _) in blocks.iter().zip(0..) {
            if matches!(block.kind, MdBlockKind::Rule) {
                got.push(String::new());
                continue;
            }
            for _ in 0..crate::md_block_line_texts(block).len() {
                got.push(rows.next().expect("行が足りない").0.clone());
            }
        }
        assert_eq!(got, expected);
        assert!(rows.next().is_none(), "余分に描いた行がある");
    }

    /// 見出しレベルごとの見た目（#656 の値を固定する）
    #[test]
    fn 見出しはレベルでサイズと太さと罫線が変わる() {
        let theme = Theme::default();
        let base = theme.font_size;
        let looks: Vec<HeadingLook> = (1u8..=6)
            .map(|level| heading_look(level, base, &theme, theme.foreground))
            .collect();
        // サイズは単調減少
        for pair in looks.windows(2) {
            assert!(
                pair[0].size > pair[1].size,
                "見出しサイズが単調減少していない"
            );
        }
        assert_eq!(looks[0].weight, FontWeight::EXTRA_BOLD);
        assert_eq!(looks[1].weight, FontWeight::EXTRA_BOLD);
        assert_eq!(looks[2].weight, FontWeight::BOLD);
        // 罫線は H1 / H2 だけ
        assert_eq!(looks[0].rule, Some(theme.border_heavy));
        assert_eq!(looks[1].rule, Some(theme.border_default));
        assert!(looks[2..].iter().all(|l| l.rule.is_none()));
    }

    /// インライン装飾: リンク・コード・打消しに色が付く（プレビューと共通経路）
    #[test]
    fn インライン装飾が色と下線になる() {
        let theme = Theme::default();
        let blocks = markdown_blocks("[text](https://example.com) `code` ~~strike~~ **b** *i*");
        let MdBlockKind::Paragraph { spans } = &blocks[0].kind else {
            panic!("段落が来ていない: {:?}", blocks[0].kind);
        };
        let (text, highlights) = inline_highlights(spans, &theme);
        assert!(text.starts_with("text "), "{text}");
        // リンク（accent + 下線）・コード（peach + 背景）・打消し・太字・斜体の 5 本
        assert_eq!(highlights.len(), 5, "{highlights:?}");
        let link = &highlights[0].1;
        assert_eq!(link.color, Some(hsla(theme.accent)));
        assert!(link.underline.is_some());
        let code = &highlights[1].1;
        assert_eq!(code.color, Some(hsla(theme.peach)));
        assert!(code.background_color.is_some());
        assert!(highlights[2].1.strikethrough.is_some());
        assert_eq!(highlights[3].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(highlights[4].1.font_style, Some(FontStyle::Italic));
    }

    /// ⌘ホバーの装飾は md 由来の後ろへ積む（後の指定が勝つ = 実線下線 + accent 背景）
    #[test]
    fn ホバー装飾は範囲外を切り詰めて後ろへ積む() {
        let theme = Theme::default();
        let mut highlights = Vec::new();
        push_hovered_link_highlight(&mut highlights, "abc", &(1..99), &theme);
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].0, 1..3, "テキスト長で切り詰める");
        // 始点が末尾以降なら何も積まない（panic しない）
        let before = highlights.len();
        push_hovered_link_highlight(&mut highlights, "abc", &(3..5), &theme);
        push_hovered_link_highlight(&mut highlights, "", &(0..0), &theme);
        assert_eq!(highlights.len(), before);
    }

    /// 空文字は空白 1 個へ（空セル・空項目が潰れない）
    #[test]
    fn 空行は高さを残すため空白へ置き換える() {
        assert_eq!(pad_empty(String::new()), " ");
        assert_eq!(pad_empty("a".into()), "a");
    }

    /// ライトテーマでも構文色が読める明度へ落ちる（#669 と同一経路であることの確認）
    #[test]
    fn コードブロックの構文色はライトで明度を落とす() {
        let bright = tako_core::Rgb {
            r: 250,
            g: 250,
            b: 120,
        };
        let lines = vec![vec![crate::preview::Span {
            text: "let x = 1;".into(),
            color: Some(bright),
            bold: false,
            italic: false,
        }]];
        let light = Theme::for_mode(ThemeMode::Light);
        let (text, highlights) = code_block_highlights(Some("rust"), &lines, &light);
        assert_eq!(text, "let x = 1;");
        assert_eq!(
            highlights[0].1.color,
            Some(hsla(light.adapt_syntax_color(bright))),
            "ライトの面で読める明度へ落ちていない"
        );
        // 言語指定なしフェンスは色を付けない（テーマの本文色で素直に出す）
        let (_, plain) = code_block_highlights(None, &lines, &light);
        assert!(plain.is_empty(), "{plain:?}");
    }

    /// 開けない URL はホバー・クリックの対象にしない（#680 の規則をそのまま使う）
    #[test]
    fn 開けないurlはヒットテストの対象外() {
        let links = vec![
            crate::MdLinkHit {
                line: 0,
                range: 0..4,
                url: "javascript:alert(1)".into(),
            },
            crate::MdLinkHit {
                line: 0,
                range: 0..4,
                url: "./relative.md".into(),
            },
        ];
        // レイアウトが空でも panic せず None（描画前のクリックで落ちない）
        assert_eq!(
            md_link_at_layouts(&links, &[], gpui::point(px(1.0), px(1.0))),
            None
        );
    }

    /// エッジ: 空・壊れた md・極端に長いノートでも panic せず要素が出る（#690 受け入れ条件 3）
    #[test]
    fn 壊れたmdと巨大なmdでも描画が落ちない() {
        for theme in [Theme::default(), Theme::for_mode(ThemeMode::Light)] {
            // 空
            let (elements, layouts) = render_document(&theme, &markdown_blocks(""), None);
            assert!(elements.is_empty() && layouts.is_empty());

            // 閉じていないフェンス・壊れた表・裸のパイプ・未対応スキームのリンク
            let broken = "```rust\nfn main() {\n\n| a | b\n|---\n| 1 |\n\n\
                          [x](javascript:alert(1))\n\n> > 深い引用\n\n\
                          - [ ] \n\n#\n\n######## h8\n\n|||\n";
            let (elements, layouts) = render_document(&theme, &markdown_blocks(broken), None);
            assert!(!elements.is_empty());
            assert!(!layouts.is_empty());

            // 巨大（見出し + 表 + 本文を 500 回）
            let mut big = String::new();
            for i in 0..500 {
                big.push_str(&format!(
                    "## 見出し {i}\n\n| 列 | 値 |\n|---|---:|\n| a | {i} |\n\n本文 {i} \
                     [link](https://example.com/{i})\n\n"
                ));
            }
            let blocks = markdown_blocks(&big);
            let links = crate::md_document_links(&blocks);
            assert_eq!(links.len(), 500);
            let (elements, layouts) = render_document(&theme, &blocks, Some((0, 0..3)));
            assert_eq!(elements.len(), blocks.len());
            let expected_lines: usize = blocks
                .iter()
                .map(|b| crate::md_block_line_texts(b).len())
                .sum();
            assert_eq!(
                layouts.len(),
                expected_lines,
                "行番号と TextLayout の対応が崩れている"
            );
        }
    }

    /// 表は列数を align / header の広い方に揃え、欠けたセルも描く（溢れ・欠落の防止）
    #[test]
    fn 表は列数を揃えて欠けたセルも描く() {
        let theme = Theme::default();
        let sink = render_all(
            "| a | b | c |\n|---|:-:|--:|\n| 1 |\n| 1 | 2 | 3 |\n",
            &theme,
        );
        // ヘッダ 3 + 本文 2 行 × 3 列 = 9 セル
        assert_eq!(sink.rows.len(), 9, "{:?}", sink.rows);
        assert_eq!(sink.rows[0].0, "a");
        assert_eq!(sink.rows[0].3, Some(FontWeight::BOLD), "ヘッダは太字");
        assert_eq!(sink.rows[4].0, "", "欠けたセルは空文字で埋まる");
        assert_eq!(sink.rows[3].3, None, "本文は太字にしない");
    }

    /// 罫線は spacer として通知され、コードブロックは出現順の番号で装飾を求める
    #[test]
    fn 罫線とコードブロックの通知() {
        let theme = Theme::default();
        let sink = render_all("---\n\n```sh\na\n```\n\n---\n\n```\nb\n```\n", &theme);
        assert_eq!(sink.spacers, 2, "罫線 2 本");
        assert_eq!(sink.code_overlays, vec![0, 1], "コードブロックは出現順");
    }
}

//! 端末グリッドの専用 Element（Issue #787）
//!
//! 旧実装は 1 行 = 「行 div + スタイル区間ごとの子 div」を毎フレーム taffy へ流していた。
//! #782 の実測ではこれが **0.39M instructions / 行**（137 桁で 2,800 命令/セル）で、
//! Zed の専用 Element（0.16M / フレーム）とは 1 桁違う固定費になっていた。
//!
//! ここでは行スタックを 1 個の [`TerminalGrid`] element に置き換える。
//! セルの原点を `col * cell_width` で直接決めて `paint_quad` / `shape_line` を呼ぶので、
//! div も taffy ノードも作らない。同時に div 構成に起因していた次の 2 つが構造的に消える:
//!
//! - **#797（下線が 1 px も描かれない）**: GPUI の `paint_line` は下線を
//!   「ベースライン + descent×0.618」= 行ボックスの下端ちょうどへ置くので、
//!   チャンク div の `overflow_hidden`（#64 の折り返し対策で外せない）が丸ごと切り落として
//!   いた。ここでは下線を自分で引く（行の内側に収まる位置を [`underline_y`] が決める）
//! - **#798（全角の長い連なりで描画位置が最大 1 セルずれる）**: 幅
//!   `cell_width * cols` の div を 55 個積むと、GPUI / taffy がそれぞれをデバイス
//!   ピクセルへ丸めるので不足が累積していた。ここは列番号から直接座標を作るので
//!   累積が生まれない
//!
//! ## セル幅とグリフ幅の整合（#64 / #39 / #798）
//!
//! グリフを [`gpui::WindowTextSystem::shape_line`] の `force_width` 付きでシェイプする
//! （Zed の端末 element と同じ方式）。`force_width` は「グリフ 1 個 = 1 セル」を仮定して
//! グリフ位置をセル境界へスナップするので、
//!
//! - フォールバックフォントで advance がセル幅と合わないグリフ（`⏺` 等）は
//!   **後続のグリフが自動でグリッドへ戻る** = #64 の個別 div 隔離が不要になる
//! - 全角（2 セル）文字は仮定を破るので、**占有する 2 セル目にスペースを 1 個差し込む**。
//!   これでグリフ数と列数が 1:1 に戻り、全角が続く行でもスナップが効く（#798）
//!
//! ## 行の切り出し（性能）
//!
//! [`ScreenLine::text`] は空セルもスペースとして全桁ぶん持っている。空白は
//! 背景も装飾も無ければ描くものが無いので、行頭 / 行末の空白は落とし、
//! 行中の長い空白（[`BLANK_SPLIT`] セル以上）でシェイプ区間を切る。
//! 区間ごとに `shape_line` を呼ぶが、行レイアウトは GPUI 側で
//! （テキスト・フォント・force_width をキーに）フレームをまたいでキャッシュされるので、
//! スクロールで同じ行が別の行位置へ移動しただけならシェイプは走らない。

use std::panic::Location;

use gpui::{
    fill, point, px, relative, size, App, Bounds, Element, ElementId, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, Size,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};
use tako_core::screen::{ScreenLine, StyleRun};

/// 専用 element を切って旧経路（行 div のスタック）へ戻す逃げ道
/// （`TAKO_787_NO_GRID_ELEMENT=1`）。
///
/// 効果測定（同じバイナリで A/B を取る）と、描画異常が出たときに
/// 「element 化のせい」と「それ以外」を切り分けるために使う。既定は有効（未設定 = element）
pub(crate) fn element_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_787_NO_GRID_ELEMENT").is_some())
}

/// セル単位の近道（#801: 空行の早期打ち切りとラン単位の色解決）を切って
/// 同じバイナリで A/B を取る逃げ道（`TAKO_801_NO_FAST_CELLS=1`）。
/// `tako_core::screen` の空白セル近道と同じ変数で一緒に切れる
pub(crate) fn fast_cells_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_801_NO_FAST_CELLS").is_some())
}

/// 行中の空白がこのセル数以上続いたらシェイプ区間を切る。
///
/// 空白 1 セルぶんのグリフ（`paint_glyph` の raster 参照）より、区間を増やす
/// （`shape_line` のキャッシュ参照 + `paint_layer`）方が安いところで分ける
const BLANK_SPLIT: usize = 8;

/// 下線・取り消し線の太さ（旧 div 実装の `UnderlineStyle::thickness` と同じ）
pub(crate) const DECORATION_THICKNESS: f32 = 1.0;

/// 下線を行の下端からどれだけ内側へ置くか。
///
/// GPUI の `paint_line` と同じ「ベースライン + descent×0.618」だと
/// 行ボックスの下端ちょうど（tako のテーマでは 16.8 / 17.0 px）に来てしまい、
/// 行の外へ出るか下の行へ食い込む。1 px の余白を残して**必ず行の内側**へ収める
const UNDERLINE_BOTTOM_GAP: f32 = 1.0;

/// ⌘ホバー中のリンク装飾（#153）。色は 1 か所で決めて描画と索引が共有する
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LinkDecoration {
    pub fg: Hsla,
    pub bg: Hsla,
    pub underline: Hsla,
}

/// セル範囲に掛かる一様な色（背景・下線・取り消し線）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellSpan {
    pub col: usize,
    pub cols: usize,
    pub color: Hsla,
}

/// シェイプ 1 回ぶんの文字列と、その中のスタイル区間
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowSegment {
    /// 先頭文字が乗るグリッド列
    pub col: usize,
    /// シェイプするテキスト（全角の 2 セル目にはスペースを差し込んである）。
    /// `SharedString` なのは毎フレームの `shape_line` へ**複製せず**渡すため
    pub text: SharedString,
    pub styles: Vec<SegmentStyle>,
}

/// シェイプ区間内の同一スタイル区間（バイト長 + フォント選択 + 色）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SegmentStyle {
    pub len: usize,
    pub fg: Hsla,
    pub bold: bool,
    pub italic: bool,
}

/// 1 行ぶんの描画計画。GPUI の描画呼び出しを持たない純データなので単体テストできる
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RowPlan {
    pub fills: Vec<CellSpan>,
    pub segments: Vec<RowSegment>,
    pub underlines: Vec<CellSpan>,
    pub strikeouts: Vec<CellSpan>,
}

impl RowPlan {
    /// 描くものが何も無い行（空行）
    pub(crate) fn is_empty(&self) -> bool {
        self.fills.is_empty()
            && self.segments.is_empty()
            && self.underlines.is_empty()
            && self.strikeouts.is_empty()
    }
}

/// 計画を組む途中のセル 1 つ（リンク装飾の上書きまで済んだ状態）
#[derive(Debug, Clone, Copy)]
struct PlanCell {
    ch: char,
    col: usize,
    /// 占有セル数（半角 1 / 全角 2）
    cols: usize,
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: Option<Hsla>,
    strikeout: Option<Hsla>,
}

impl PlanCell {
    /// グリフとして描くものが無いセル（空白）。背景・装飾は別の層が描く
    fn blank(&self) -> bool {
        self.ch == ' '
    }
}

/// 行のスタイルランを引く（`byte_off` を含むラン）。ラン列は昇順・非重複
fn run_at<'a>(runs: &'a [StyleRun], run_idx: &mut usize, byte_off: usize) -> Option<&'a StyleRun> {
    while *run_idx + 1 < runs.len() && byte_off >= runs[*run_idx].range.end {
        *run_idx += 1;
    }
    runs.get(*run_idx)
        .filter(|r| byte_off >= r.range.start && byte_off < r.range.end)
}

/// ラン 1 本ぶんの解決済み色（#801）。
///
/// `Rgb -> Hsla` は除算を含むので、セルごとに引き直すと**空画面でも**毎フレーム
/// セル数ぶん（119x21 = 2,499 回）走る。ランは同じスタイルの連続区間なので、
/// ランが変わったときだけ作って使い回す
#[derive(Debug, Clone, Copy)]
struct RunStyle {
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: Option<Hsla>,
    strikeout: Option<Hsla>,
}

impl RunStyle {
    fn resolve(run: Option<&StyleRun>, default_fg: Hsla) -> Self {
        let Some(r) = run else {
            return Self {
                fg: default_fg,
                bg: None,
                bold: false,
                italic: false,
                underline: None,
                strikeout: None,
            };
        };
        let fg = to_hsla(r.fg);
        Self {
            fg,
            bg: r.bg.map(to_hsla),
            bold: r.bold,
            italic: r.italic,
            // 下線・取り消し線の色は前景色と同じ（旧実装の `to_hsla(r.fg)` と同値）
            underline: r.underline.then_some(fg),
            strikeout: r.strikeout.then_some(fg),
        }
    }
}

/// 描くものが何も無い行か（#801）。
///
/// 全部が空白で、背景・下線・取り消し線がどのランにも無ければ、
/// [`plan_row`] の結果は必ず空になる。空画面ではこれが全行に当たるので、
/// セル単位の組み立て（2,499 個の `PlanCell`）ごと省ける。
/// ⌘ホバーのリンク装飾は空白セルにも掛かるため、リンクがある行は対象外
fn row_draws_nothing(line: &ScreenLine) -> bool {
    line.text.bytes().all(|b| b == b' ')
        && line
            .runs
            .iter()
            .all(|r| r.bg.is_none() && !r.underline && !r.strikeout)
}

/// 1 行を描画計画へ変換する。
///
/// `link` は ⌘ホバー中のリンクのセル範囲 `[start, end)`。`fg` は既定前景色
/// （ランから外れたセルのフォールバック）
pub(crate) fn plan_row(
    line: &ScreenLine,
    fg: Hsla,
    link: Option<(usize, usize)>,
    link_style: LinkDecoration,
) -> RowPlan {
    let fast = !fast_cells_disabled();
    // #801: 描くものが無い行はセル単位の組み立てごと省く（空画面の全行がこれ）
    if fast && link.is_none() && row_draws_nothing(line) {
        return RowPlan::default();
    }
    let mut cells: Vec<PlanCell> = Vec::with_capacity(line.cell_cols.len());
    let mut run_idx = 0usize;
    // 直前に解決したラン（#801。同じランの連続セルで Rgb->Hsla を繰り返さない）
    let mut cached: Option<(usize, bool, RunStyle)> = None;
    for (ci, (byte_off, ch)) in line.text.char_indices().enumerate() {
        let col = line.cell_cols.get(ci).copied().unwrap_or(ci);
        let cols = line
            .cell_cols
            .get(ci + 1)
            .map(|next| next.saturating_sub(col))
            .unwrap_or(1);
        let run = run_at(&line.runs, &mut run_idx, byte_off);
        // ランの同一性は「何番目のランか」+「そのランに入っているか」で決まる
        let hit = run.is_some();
        let style = match cached {
            Some((idx, cached_hit, style)) if fast && idx == run_idx && cached_hit == hit => style,
            _ => {
                let style = RunStyle::resolve(run, fg);
                cached = Some((run_idx, hit, style));
                style
            }
        };
        let mut cell = PlanCell {
            ch,
            col,
            cols,
            fg: style.fg,
            bg: style.bg,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strikeout: style.strikeout,
        };
        // リンク装飾はランより後（同じ ANSI ラン内でもリンク部分だけに掛かる）
        if let Some((start, end)) = link {
            let cell_end = col + cols.max(1);
            if cell_end > start && col < end {
                cell.fg = link_style.fg;
                cell.bg = Some(link_style.bg);
                cell.underline = Some(link_style.underline);
            }
        }
        cells.push(cell);
    }

    RowPlan {
        fills: merge_spans(&cells, |c| c.bg),
        underlines: merge_spans(&cells, |c| c.underline),
        strikeouts: merge_spans(&cells, |c| c.strikeout),
        segments: shape_segments(&cells),
    }
}

/// 同じ色が連続するセル範囲を 1 本の帯へまとめる
fn merge_spans(cells: &[PlanCell], pick: impl Fn(&PlanCell) -> Option<Hsla>) -> Vec<CellSpan> {
    let mut spans: Vec<CellSpan> = Vec::new();
    for cell in cells {
        let Some(color) = pick(cell) else { continue };
        let cols = cell.cols.max(1);
        match spans.last_mut() {
            Some(last) if last.color == color && last.col + last.cols == cell.col => {
                last.cols += cols;
            }
            _ => spans.push(CellSpan {
                col: cell.col,
                cols,
                color,
            }),
        }
    }
    spans
}

/// グリフを描くべきセルを拾ってシェイプ区間へ切る。
///
/// 空白は描くものが無いので、行頭 / 行末は落とし、行中は [`BLANK_SPLIT`] セル以上
/// 続いたところで区間を分ける（区間内の短い空白は素通しでシェイプする）
fn shape_segments(cells: &[PlanCell]) -> Vec<RowSegment> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        if cell.blank() {
            continue;
        }
        match groups.last_mut() {
            // 直前の非空白セルの**右端**からの空白が閾値以下なら同じ区間に載せる
            Some((_, last))
                if cell
                    .col
                    .saturating_sub(cells[*last].col + cells[*last].cols.max(1))
                    <= BLANK_SPLIT =>
            {
                *last = i;
            }
            _ => groups.push((i, i)),
        }
    }

    groups
        .into_iter()
        .map(|(lo, hi)| {
            let mut text = String::with_capacity((hi - lo + 1) * 2);
            let mut styles: Vec<SegmentStyle> = Vec::new();
            for cell in &cells[lo..=hi] {
                let mut len = cell.ch.len_utf8();
                text.push(cell.ch);
                // 全角が占有する 2 セル目以降はスペースで埋める。これでグリフ数と
                // 列数が 1:1 になり `force_width` のセル境界スナップが効く（#798）
                for _ in 1..cell.cols {
                    text.push(' ');
                    len += 1;
                }
                match styles.last_mut() {
                    Some(last)
                        if last.fg == cell.fg
                            && last.bold == cell.bold
                            && last.italic == cell.italic =>
                    {
                        last.len += len;
                    }
                    _ => styles.push(SegmentStyle {
                        len,
                        fg: cell.fg,
                        bold: cell.bold,
                        italic: cell.italic,
                    }),
                }
            }
            RowSegment {
                col: cells[lo].col,
                text: text.into(),
                styles,
            }
        })
        .collect()
}

fn to_hsla(c: tako_core::Rgb) -> Hsla {
    crate::hsla(c)
}

/// 下線の上端 y（行原点からの相対）。**必ず行の内側**に収める（#797）
pub(crate) fn underline_y(line_height: f32, thickness: f32) -> f32 {
    (line_height - thickness - UNDERLINE_BOTTOM_GAP).max(0.0)
}

/// 取り消し線の上端 y（行原点からの相対）。
/// GPUI の `paint_line` と同じ「(ascent×0.5 + ベースライン) × 0.5」。
/// ascent が採れないときは行高比でフォールバックする
pub(crate) fn strikeout_y(line_height: f32, ascent: Option<(f32, f32)>) -> f32 {
    match ascent {
        Some((ascent, descent)) => {
            let baseline = (line_height - ascent - descent) / 2.0 + ascent;
            (ascent * 0.5 + baseline) * 0.5
        }
        None => line_height * 0.6,
    }
}

/// 端末グリッド 1 ペインぶんを描く element（#787）
pub(crate) struct TerminalGrid {
    rows: Vec<RowPlan>,
    cell: Size<Pixels>,
    font_size: Pixels,
    base_font: Font,
    /// サブラインスクロールで行スタック全体を上へずらす量（#159）
    subline: Pixels,
}

impl TerminalGrid {
    pub(crate) fn new(
        rows: Vec<RowPlan>,
        cell: Size<Pixels>,
        font_size: Pixels,
        base_font: Font,
        subline: Pixels,
    ) -> Self {
        Self {
            rows,
            cell,
            font_size,
            base_font,
            subline,
        }
    }

    fn font_for(&self, style: &SegmentStyle) -> Font {
        let mut font = self.base_font.clone();
        if style.bold {
            font.weight = FontWeight::BOLD;
        }
        if style.italic {
            font.style = FontStyle::Italic;
        }
        font
    }
}

impl IntoElement for TerminalGrid {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalGrid {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let cw = self.cell.width;
        let lh = self.cell.height;
        if f32::from(cw) <= 0.0 || f32::from(lh) <= 0.0 {
            return;
        }
        let scale = window.scale_factor().max(1.0);
        // セルの境界をデバイスピクセルへ揃える（隣り合う背景の継ぎ目・すじを消す）。
        // グリフ位置は素の float のままなのでサブピクセル配置は失われない
        let snap = move |v: Pixels| -> Pixels { px((f32::from(v) * scale).round() / scale) };
        let x_of = move |col: usize| -> Pixels { snap(bounds.origin.x + cw * col as f32) };
        let clip_top = bounds.origin.y - lh;
        let clip_bottom = bounds.bottom() + lh;

        for (row_idx, row) in self.rows.iter().enumerate() {
            if row.is_empty() {
                continue;
            }
            let y = bounds.origin.y + lh * row_idx as f32 - self.subline;
            // 完全に見える範囲の外にある行は組み立てごと省く（部分行は残す）
            if y > clip_bottom || y < clip_top {
                continue;
            }
            let y_top = snap(y);
            let y_bottom = snap(y + lh);

            // 背景（選択・ブロックカーソル・SGR 48 はすべてランへ焼かれている）
            for span in &row.fills {
                let x0 = x_of(span.col);
                let x1 = x_of(span.col + span.cols);
                window.paint_quad(fill(
                    Bounds::new(point(x0, y_top), size(x1 - x0, y_bottom - y_top)),
                    span.color,
                ));
            }

            // グリフ。ascent / descent は取り消し線の位置決めに使う
            let mut metrics: Option<(f32, f32)> = None;
            for segment in &row.segments {
                let runs: Vec<TextRun> = segment
                    .styles
                    .iter()
                    .map(|style| TextRun {
                        len: style.len,
                        font: self.font_for(style),
                        color: style.fg,
                        background_color: None,
                        // 下線・取り消し線は自分で引く（#797。GPUI は行ボックスの
                        // 下端へ置くので、この element の内側に収まらない）
                        underline: None,
                        strikethrough: None,
                    })
                    .collect();
                let shaped = window.text_system().shape_line(
                    segment.text.clone(),
                    self.font_size,
                    &runs,
                    Some(cw),
                );
                if metrics.is_none() {
                    metrics = Some((f32::from(shaped.ascent), f32::from(shaped.descent)));
                }
                let origin = point(bounds.origin.x + cw * segment.col as f32, y);
                let _ = shaped.paint(origin, lh, gpui::TextAlign::Left, None, window, cx);
            }

            // 下線（#797）
            let thickness = px(DECORATION_THICKNESS);
            if !row.underlines.is_empty() {
                let dy = px(underline_y(f32::from(lh), DECORATION_THICKNESS));
                for span in &row.underlines {
                    let x0 = x_of(span.col);
                    let x1 = x_of(span.col + span.cols);
                    window.paint_underline(
                        point(x0, y + dy),
                        x1 - x0,
                        &UnderlineStyle {
                            thickness,
                            color: Some(span.color),
                            wavy: false,
                        },
                    );
                }
            }
            if !row.strikeouts.is_empty() {
                let dy = px(strikeout_y(f32::from(lh), metrics));
                for span in &row.strikeouts {
                    let x0 = x_of(span.col);
                    let x1 = x_of(span.col + span.cols);
                    window.paint_strikethrough(
                        point(x0, y + dy),
                        x1 - x0,
                        &StrikethroughStyle {
                            thickness,
                            color: Some(span.color),
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::Rgb;

    fn link_style() -> LinkDecoration {
        LinkDecoration {
            fg: gpui::hsla(0.5, 1.0, 0.5, 1.0),
            bg: gpui::hsla(0.5, 1.0, 0.5, 0.22),
            underline: gpui::hsla(0.5, 1.0, 0.5, 1.0),
        }
    }

    fn fg() -> Hsla {
        gpui::hsla(0.0, 0.0, 1.0, 1.0)
    }

    /// 半角だけの行を、テキストと同じスタイルで組む
    fn line(text: &str) -> ScreenLine {
        let mut cell_cols = Vec::new();
        let mut col = 0usize;
        for ch in text.chars() {
            cell_cols.push(col);
            col += if is_wide(ch) { 2 } else { 1 };
        }
        ScreenLine {
            runs: vec![StyleRun {
                range: 0..text.len(),
                fg: Rgb::new(200, 200, 200),
                bg: None,
                bold: false,
                italic: false,
                underline: false,
                strikeout: false,
                dim: false,
            }],
            has_wide: text.chars().any(is_wide),
            text: text.to_string(),
            cell_cols,
        }
    }

    fn is_wide(ch: char) -> bool {
        matches!(ch, '\u{1100}'..='\u{115F}' | '\u{2E80}'..='\u{A4CF}' | '\u{FF00}'..='\u{FF60}')
    }

    #[test]
    fn 全角セルの二セル目はスペースで埋まる() {
        // 「全」は 2 セル。グリフ数 = 列数になっていないと force_width の
        // セル境界スナップが効かず #798 の位置ずれが戻る
        let plan = plan_row(&line("全全ab"), fg(), None, link_style());
        assert_eq!(plan.segments.len(), 1);
        let seg = &plan.segments[0];
        assert_eq!(seg.col, 0);
        assert_eq!(seg.text, "全 全 ab");
        // バイト長の合計がテキスト長と一致（TextRun の len は utf-8 バイト）
        let total: usize = seg.styles.iter().map(|s| s.len).sum();
        assert_eq!(total, seg.text.len());
    }

    #[test]
    fn 行頭行末の空白は落ちて列位置は保たれる() {
        let plan = plan_row(&line("   ab   "), fg(), None, link_style());
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].col, 3);
        assert_eq!(plan.segments[0].text, "ab");
    }

    #[test]
    fn 行中の長い空白でシェイプ区間が切れる() {
        let gap = " ".repeat(BLANK_SPLIT + 4);
        let plan = plan_row(&line(&format!("ab{gap}cd")), fg(), None, link_style());
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].text, "ab");
        assert_eq!(plan.segments[1].text, "cd");
        assert_eq!(plan.segments[1].col, 2 + BLANK_SPLIT + 4);
    }

    #[test]
    fn 行中の短い空白は同じ区間に残る() {
        let gap = " ".repeat(BLANK_SPLIT - 1);
        let plan = plan_row(&line(&format!("ab{gap}cd")), fg(), None, link_style());
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].text, format!("ab{gap}cd"));
    }

    #[test]
    fn 背景と下線はセル範囲へまとめられる() {
        let mut l = line("abcdef");
        l.runs = vec![
            StyleRun {
                range: 0..2,
                fg: Rgb::new(200, 200, 200),
                bg: Some(Rgb::new(10, 20, 30)),
                bold: false,
                italic: false,
                underline: true,
                strikeout: false,
                dim: false,
            },
            StyleRun {
                range: 2..4,
                fg: Rgb::new(200, 200, 200),
                bg: Some(Rgb::new(10, 20, 30)),
                bold: false,
                italic: false,
                underline: true,
                strikeout: false,
                dim: false,
            },
            StyleRun {
                range: 4..6,
                fg: Rgb::new(200, 200, 200),
                bg: None,
                bold: false,
                italic: false,
                underline: false,
                strikeout: false,
                dim: false,
            },
        ];
        let plan = plan_row(&l, fg(), None, link_style());
        // 同色 4 セルが 1 本に
        assert_eq!(plan.fills.len(), 1);
        assert_eq!((plan.fills[0].col, plan.fills[0].cols), (0, 4));
        assert_eq!(plan.underlines.len(), 1);
        assert_eq!((plan.underlines[0].col, plan.underlines[0].cols), (0, 4));
        assert!(plan.strikeouts.is_empty());
    }

    #[test]
    fn リンク範囲だけに装飾が掛かる() {
        let plan = plan_row(&line("abcdef"), fg(), Some((2, 4)), link_style());
        assert_eq!(plan.underlines.len(), 1);
        assert_eq!((plan.underlines[0].col, plan.underlines[0].cols), (2, 2));
        assert_eq!(plan.fills.len(), 1);
        assert_eq!((plan.fills[0].col, plan.fills[0].cols), (2, 2));
        // リンク部分だけ色が変わる（前半・後半は素のまま = スタイル区間が 3 本）
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].styles.len(), 3);
    }

    #[test]
    fn 全角の途中に掛かるリンクもセル境界で切れる() {
        // 「全」が col 0..2、「本」が col 2..4。リンクが col 1..3 なら両方に掛かる
        let plan = plan_row(&line("全本ab"), fg(), Some((1, 3)), link_style());
        let cols: Vec<(usize, usize)> = plan.underlines.iter().map(|s| (s.col, s.cols)).collect();
        assert_eq!(cols, vec![(0, 4)]);
    }

    #[test]
    fn 空行は何も描かない() {
        let plan = plan_row(&line("        "), fg(), None, link_style());
        assert!(plan.is_empty());
    }

    // ---- #801: セル単位の近道が結果を変えないこと ----

    #[test]
    fn 空行の早期打ち切りは装飾つきの行に効かない() {
        // 背景・下線・取り消し線のどれかが乗っていれば「描くものがある」
        for (bg, ul, st) in [
            (Some(Rgb::new(1, 2, 3)), false, false),
            (None, true, false),
            (None, false, true),
        ] {
            let mut l = line("     ");
            l.runs[0].bg = bg;
            l.runs[0].underline = ul;
            l.runs[0].strikeout = st;
            assert!(
                !row_draws_nothing(&l),
                "bg={bg:?} ul={ul} st={st} は空扱いにしてはいけない"
            );
            assert!(!plan_row(&l, fg(), None, link_style()).is_empty());
        }
        // 素の空白だけなら空
        assert!(row_draws_nothing(&line("     ")));
    }

    #[test]
    fn 空行でもリンク装飾があれば早期打ち切りしない() {
        // ⌘ホバーのリンクは空白セルにも背景と下線を乗せる
        let plan = plan_row(&line("     "), fg(), Some((1, 3)), link_style());
        assert!(!plan.is_empty());
        assert_eq!(plan.underlines.len(), 1);
    }

    #[test]
    fn ラン単位の色解決は複数ランをまたいでも取り違えない() {
        // ラン境界で色が切り替わることを固定する（キャッシュのキーが甘いと崩れる）
        let mut l = line("aabbcc");
        let mk = |range: std::ops::Range<usize>, r: u8| StyleRun {
            range,
            fg: Rgb::new(r, 0, 0),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            dim: false,
        };
        l.runs = vec![mk(0..2, 10), mk(2..4, 20), mk(4..6, 30)];
        let plan = plan_row(&l, fg(), None, link_style());
        assert_eq!(plan.segments.len(), 1);
        let styles = &plan.segments[0].styles;
        assert_eq!(styles.len(), 3, "ランごとにスタイル区間が分かれる");
        assert_eq!(
            styles.iter().map(|s| s.len).collect::<Vec<_>>(),
            vec![2, 2, 2]
        );
        assert_eq!(styles[0].fg, crate::hsla(Rgb::new(10, 0, 0)));
        assert_eq!(styles[1].fg, crate::hsla(Rgb::new(20, 0, 0)));
        assert_eq!(styles[2].fg, crate::hsla(Rgb::new(30, 0, 0)));
    }

    #[test]
    fn ランの外のセルは既定前景色に戻る() {
        // ラン列が行の途中で終わる場合、以降のセルは fg（既定色）で描かれる
        let mut l = line("abcd");
        l.runs = vec![StyleRun {
            range: 0..2,
            fg: Rgb::new(9, 9, 9),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            dim: false,
        }];
        let styles = &plan_row(&l, fg(), None, link_style()).segments[0].styles;
        assert_eq!(styles.len(), 2);
        assert_eq!(styles[0].fg, crate::hsla(Rgb::new(9, 9, 9)));
        assert_eq!(styles[1].fg, fg());
    }

    #[test]
    fn 下線は行の内側に収まる() {
        // #797 の根本原因は「行ボックスの下端ちょうど（16.8/17.0）に置かれて
        // overflow_hidden に切られる」こと。太さぶん + 余白を残すのを固定する
        let lh = 17.0;
        let y = underline_y(lh, DECORATION_THICKNESS);
        assert!(y > 0.0, "行の上端より下");
        assert!(
            y + DECORATION_THICKNESS <= lh - 0.5,
            "下端より内側 (y={y}, lh={lh})"
        );
        // セル下端 25% の帯（visual-test の underline_band）に入る
        assert!(y >= lh * 0.75, "下端 25% の帯の中 (y={y})");
    }

    #[test]
    fn 極端に小さい行高でも下線が負にならない() {
        assert_eq!(underline_y(1.0, 1.0), 0.0);
        assert!(underline_y(4.0, 1.0) >= 0.0);
    }

    #[test]
    fn 取り消し線は行の中ほどに来る() {
        let lh = 17.0;
        let y = strikeout_y(lh, Some((12.4, 3.6)));
        assert!(y > lh * 0.3 && y < lh * 0.8, "行の中ほど (y={y})");
        // ascent が採れないときのフォールバックも同じ範囲に入る
        let fallback = strikeout_y(lh, None);
        assert!(fallback > lh * 0.3 && fallback < lh * 0.8);
    }
}

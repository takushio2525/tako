use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, Bounds, BoxShadow, Context, CursorStyle,
    FontWeight, HighlightStyle, MouseButton, MouseMoveEvent, Pixels, SharedString, StyledText,
    UnderlineStyle, Window,
};
use std::path::PathBuf;
use tako_core::{PaneId, Rect};

use super::*;

/// PDF / 画像 / 動画サムネの描画用 gpui::Image キャッシュ（Issue #168）。
/// `gpui::Image::from_bytes` は id 生成のために全バイトのハッシュを計算するので、
/// render 毎に呼ぶと「全ページ PNG の clone + フルハッシュ」が毎フレーム走り、
/// 71 ページ PDF の実測で 1 フレーム p50 96ms（通常 2ms）まで劣化する。
/// path が変わらない限り load 時に 1 回だけ構築した Arc を使い回す
pub(crate) struct PreviewImageCache {
    key: PreviewImageCacheKey,
    /// PDF: ページごと（描画失敗の空ページは None）。画像 / サムネ: 先頭 1 要素
    images: Vec<Option<CachedPreviewImage>>,
    /// PDF テキストレイヤのページごと Arc（paint の canvas クロージャへ毎フレーム
    /// move する分を to_vec から Arc clone に置き換える）
    text_layers: Vec<std::sync::Arc<Vec<preview::PdfTextLine>>>,
}

pub(crate) struct CachedPreviewImage {
    image: std::sync::Arc<gpui::Image>,
    decoded_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PreviewImageCacheEntryKey {
    pane: PaneId,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreviewImageCacheKey {
    Original(PathBuf),
    Pdf(PathBuf, preview::PdfRasterKey),
}

/// 表示ページの前後を先に、表示ページ自身を最後に返す。
/// LRU 予算が近傍 3 ページ分に満たない場合も表示ページを残すための順序。
fn wanted_pdf_image_indices(current: usize, total: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let current = current.min(total - 1);
    let mut wanted = Vec::with_capacity(3);
    if current > 0 {
        wanted.push(current - 1);
    }
    if current + 1 < total {
        wanted.push(current + 1);
    }
    wanted.push(current);
    wanted
}

/// PDFKit の文字矩形キャッシュから、現在の UTF-8 選択範囲に含まれる描画矩形を返す。
/// ページ画像とは別の最前面 canvas で使い、画像 sprite に隠れない描画順を保証する。
fn pdf_selection_highlight_bounds(
    data: &preview::PdfData,
    char_bounds: &[Vec<Bounds<Pixels>>],
    selection: &PreviewSelection,
) -> Vec<Bounds<Pixels>> {
    let mut result = Vec::new();
    let mut line_idx = 0usize;
    for page in &data.text_layers {
        for line in page {
            if let Some((start, end)) = selection.range_for_line(line_idx, line.text.len()) {
                if let Some(bounds) = char_bounds.get(line_idx) {
                    result.extend(
                        line.char_boxes
                            .iter()
                            .zip(bounds)
                            .filter(|(ch, bounds)| {
                                ch.byte_range.end > start
                                    && ch.byte_range.start < end
                                    && f32::from(bounds.size.width) > 0.0
                                    && f32::from(bounds.size.height) > 0.0
                            })
                            .map(|(_, bounds)| *bounds),
                    );
                }
            }
            line_idx += 1;
        }
    }
    result
}

/// PDF の描画済み文字矩形を UTF-8 byte 座標へ逆写像する。
///
/// 行間・ページ余白では `None` を返す。従来はどの行にも当たらない座標を文書末尾へ
/// フォールバックしていたため、行間からのドラッグが「先頭から末尾まで」の全選択に
/// 化けていた。ドラッグ中の行間では直前の head を維持し、選択開始時の行間では選択を
/// 開始しない。
pub(super) fn pdf_text_hit_test(
    line_bounds: &[Bounds<Pixels>],
    char_bounds: &[Vec<Bounds<Pixels>>],
    texts: &[String],
    position: Point<Pixels>,
) -> Option<(usize, usize)> {
    for (line_idx, bounds) in line_bounds.iter().enumerate() {
        if position.y < bounds.top() || position.y >= bounds.bottom() {
            continue;
        }
        let line_text = texts.get(line_idx).map(String::as_str).unwrap_or("");
        let chars = char_bounds.get(line_idx).map(Vec::as_slice).unwrap_or(&[]);
        if chars.is_empty() {
            return (!line_text.is_empty()).then_some((line_idx, 0));
        }
        let byte_offsets: Vec<usize> = line_text
            .char_indices()
            .map(|(byte, _)| byte)
            .chain(std::iter::once(line_text.len()))
            .collect();
        for (char_idx, char_bounds) in chars.iter().enumerate() {
            let midpoint = char_bounds.left() + (char_bounds.right() - char_bounds.left()) * 0.5;
            if position.x < midpoint {
                return Some((line_idx, byte_offsets.get(char_idx).copied().unwrap_or(0)));
            }
        }
        return Some((line_idx, line_text.len()));
    }
    None
}

/// PDFKit の左下原点座標を、ズーム・パン適用後のページ画像矩形へ写像する。
///
/// `image_bounds` は GPUI のスクロールで既にパン済みの画面座標なので、倍率と移動量を
/// ここへ二重適用しない。テキスト選択・ハイライトは常に実際の画像矩形へ追従する。
pub(crate) fn pdf_box_to_screen(
    bbox: [f64; 4],
    page_size: [f64; 2],
    image_bounds: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let scale_x = f32::from(image_bounds.size.width) / page_size[0] as f32;
    let scale_y = f32::from(image_bounds.size.height) / page_size[1] as f32;
    Bounds {
        origin: point(
            image_bounds.origin.x + px(bbox[0] as f32 * scale_x),
            image_bounds.origin.y + px((page_size[1] - bbox[1] - bbox[3]) as f32 * scale_y),
        ),
        size: gpui::size(px(bbox[2] as f32 * scale_x), px(bbox[3] as f32 * scale_y)),
    }
}

#[cfg(test)]
mod pdf_hit_test_tests {
    use super::*;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: gpui::size(px(width), px(height)),
        }
    }

    #[test]
    fn pdf行間と余白は文字へ解決しない() {
        let lines = vec![
            bounds(10.0, 10.0, 80.0, 10.0),
            bounds(10.0, 30.0, 80.0, 10.0),
        ];
        let chars = vec![
            vec![bounds(10.0, 10.0, 10.0, 10.0)],
            vec![bounds(10.0, 30.0, 10.0, 10.0)],
        ];
        let texts = vec!["A".to_string(), "B".to_string()];

        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(15.0), px(25.0))),
            None
        );
        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(15.0), px(5.0))),
            None
        );
        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(15.0), px(45.0))),
            None
        );
    }

    #[test]
    fn pdf文字矩形はutf8バイト位置へ解決する() {
        let lines = vec![bounds(10.0, 10.0, 60.0, 12.0)];
        let chars = vec![vec![
            bounds(10.0, 10.0, 20.0, 12.0),
            bounds(30.0, 10.0, 20.0, 12.0),
            bounds(50.0, 10.0, 20.0, 12.0),
        ]];
        let texts = vec!["A日B".to_string()];

        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(12.0), px(15.0))),
            Some((0, 0))
        );
        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(32.0), px(15.0))),
            Some((0, 1))
        );
        assert_eq!(
            pdf_text_hit_test(&lines, &chars, &texts, point(px(68.0), px(15.0))),
            Some((0, 5))
        );
    }

    #[test]
    fn pdf文字座標はズームとパン後の画像矩形へ追従する() {
        // 612×792 のページを 150% 相当の 918×1188 へ拡大し、スクロールで
        // 左 120px・上 240px 移動した画面座標を与える。
        let image = bounds(-120.0, -240.0, 918.0, 1188.0);
        let actual = pdf_box_to_screen([72.0, 648.0, 144.0, 72.0], [612.0, 792.0], image);

        assert_eq!(actual, bounds(-12.0, -132.0, 216.0, 108.0));
    }

    #[test]
    fn pdf画像は表示近傍だけを表示ページ優先順で要求する() {
        assert_eq!(wanted_pdf_image_indices(3, 10), vec![2, 4, 3]);
        assert_eq!(wanted_pdf_image_indices(0, 10), vec![1, 0]);
        assert_eq!(wanted_pdf_image_indices(9, 10), vec![8, 9]);
        assert!(wanted_pdf_image_indices(0, 0).is_empty());
    }

    #[test]
    fn pdfリンクのヒットテストはページ画像boundsから正しく判定する() {
        // 612×792 のページが画面座標 (50, 100) に 400×518 で描画されている想定
        let image = bounds(50.0, 100.0, 400.0, 518.0);
        let page_size = [612.0, 792.0];
        // PDF 左下原点で (100, 700, 200, 30) のリンク bbox
        let link_bbox = [100.0, 700.0, 200.0, 30.0];
        let screen = pdf_box_to_screen(link_bbox, page_size, image);

        // リンク矩形の中心をクリック → 含まれる
        let center = point(
            screen.origin.x + screen.size.width / 2.0,
            screen.origin.y + screen.size.height / 2.0,
        );
        assert!(screen.contains(&center));

        // リンク矩形の外 → 含まれない
        let outside = point(px(10.0), px(10.0));
        assert!(!screen.contains(&outside));
    }

    #[test]
    fn pdfリンクはズーム後も正しくヒットする() {
        // 150% ズーム + スクロールオフセット
        let image = bounds(-120.0, -240.0, 918.0, 1188.0);
        let page_size = [612.0, 792.0];
        let link_bbox = [72.0, 648.0, 144.0, 72.0];
        let screen = pdf_box_to_screen(link_bbox, page_size, image);

        let inside = point(screen.origin.x + px(10.0), screen.origin.y + px(10.0));
        assert!(screen.contains(&inside));

        let just_outside = point(screen.origin.x - px(1.0), screen.origin.y + px(10.0));
        assert!(!screen.contains(&just_outside));
    }
}

impl TakoApp {
    /// 表示幅・device scale に合う PDF 画像を background で用意する。
    /// 64px / 1% 量子化キーが変わった時だけ要求し、連続リサイズは 120ms debounce 後の
    /// 最新要求だけを実行する。結果待ち中は旧画像を拡縮表示して UI を止めない。
    fn ensure_pdf_raster_quality(
        &mut self,
        pane_id: PaneId,
        area: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let logical_width = (f32::from(area.size.width) - (PANE_PADDING + 4.0) * 2.0).max(1.0);
        let desired = preview::PdfRasterKey::for_view(
            self.preview_device_scale,
            self.preview_views
                .get(&pane_id)
                .copied()
                .unwrap_or_default()
                .zoom,
            logical_width,
        );
        let Some(state) = self.previews.get(&pane_id) else {
            return;
        };
        let preview::PreviewContent::Pdf(data) = &state.content else {
            return;
        };
        if data.raster_key == desired {
            return;
        }
        self.pending_pdf_rasters.insert(
            pane_id,
            PendingPdfRaster {
                path: state.path.clone(),
                key: desired,
            },
        );
        if !self.active_pdf_rasters.insert(pane_id) {
            return;
        }

        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let request =
                match this.update(cx, |app, _| app.pending_pdf_rasters.get(&pane_id).cloned()) {
                    Ok(Some(request)) => request,
                    _ => {
                        let _ = this.update(cx, |app, _| {
                            app.active_pdf_rasters.remove(&pane_id);
                        });
                        break;
                    }
                };

            let path = request.path.clone();
            let key = request.key;
            let result = cx
                .background_executor()
                .spawn(async move { preview::rasterize_pdf(&path, key) })
                .await;

            let retry = this
                .update(cx, |app, cx| {
                    if app.pending_pdf_rasters.get(&pane_id) != Some(&request) {
                        return true;
                    }
                    app.pending_pdf_rasters.remove(&pane_id);
                    app.active_pdf_rasters.remove(&pane_id);
                    match result {
                        Ok(rasterized) => {
                            if let Some(state) = app.previews.get_mut(&pane_id) {
                                if state.path == request.path {
                                    if let preview::PreviewContent::Pdf(data) = &mut state.content {
                                        data.pages = rasterized.pages;
                                        data.total_pages = rasterized.total_pages;
                                        data.page_sizes = rasterized.page_sizes;
                                        data.pixel_sizes = rasterized.pixel_sizes;
                                        data.raster_key = request.key;
                                        // キャッシュは除去しない。ensure_preview_image_cache が
                                        // 次フレームで raster_key の不一致を検出して再構築する。
                                        // 旧キャッシュは再構築開始まで表示に使い、差し替え時に
                                        // #258 の LRU / GPUI eviction へ送る。(#257 / #258)
                                        cx.notify();
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!("warning: PDF 再ラスタライズ失敗: {error}");
                        }
                    }
                    false
                })
                .unwrap_or(false);
            if !retry {
                break;
            }
        })
        .detach();
    }

    fn preview_label(&self, target: PreviewTarget) -> String {
        match target {
            PreviewTarget::Pane(pane_id) => self.pane_preview_label(pane_id),
            PreviewTarget::ClosedGroup(tab) => {
                let title = self
                    .workspace
                    .shelved_panes()
                    .iter()
                    .find(|p| p.origin_tab() == tab)
                    .map(|p| p.origin_tab_title().to_string())
                    .unwrap_or_default();
                let count = self.background_entries_of_tab(tab).len();
                format!("タブ {}（閉じたタブ・{count} 件）", truncate(&title, 20))
            }
            PreviewTarget::TmuxWindow(pane_id, win) => {
                let win_name = self
                    .backend_windows
                    .get(&pane_id)
                    .and_then(|ws| ws.iter().find(|w| w.index == win))
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| format!("{win}"));
                let pane_label = self.pane_preview_label(pane_id);
                format!("{pane_label} · window {win}:{win_name}")
            }
        }
    }

    /// プレビュー対象が中身（サムネイルにできる端末）を持つか。空ならポップアップ /
    /// ピンを出さない（端末なしの単一ペイン・空グループ）
    fn preview_has_content(&self, target: PreviewTarget) -> bool {
        match target {
            PreviewTarget::Pane(pane_id) => self.terminals.contains_key(&pane_id),
            PreviewTarget::ClosedGroup(tab) => self
                .background_entries_of_tab(tab)
                .iter()
                .any(|e| self.terminals.contains_key(&e.pane)),
            PreviewTarget::TmuxWindow(pane_id, win) => {
                self.window_captures.contains_key(&(pane_id, win))
            }
        }
    }

    /// ペインの表示名（title / role > プレビュー名 > OSC タイトル > 既定）。
    /// tmux ビューの行ラベル（`tmux_view_groups`）と同じ優先順位で揃える。
    /// ツリー内・バックグラウンド中のどちらのペインも解決できる
    fn pane_preview_label(&self, pane_id: PaneId) -> String {
        let pane = self
            .workspace
            .tabs()
            .iter()
            .find_map(|t| t.tree().get(pane_id))
            .or_else(|| self.workspace.shelved(pane_id).map(|s| s.pane()));
        if let Some(p) = pane {
            match (p.title(), p.role()) {
                (Some(t), Some(r)) => return format!("{t} · {r}"),
                (Some(t), None) => return t.to_string(),
                (None, Some(r)) => return r.to_string(),
                (None, None) => {}
            }
        }
        if let Some(preview) = self.previews.get(&pane_id) {
            return preview.file_name().to_string();
        }
        self.terminals
            .get(&pane_id)
            .and_then(|s| s.title())
            .unwrap_or("ターミナル")
            .to_string()
    }

    /// プレビュー本文（実画面サムネイル）。Pane は端末の現在グリッドをそのまま読む
    /// （リサイズしない＝バックグラウンドのプログラムを乱さない）。ClosedGroup はグループ内の
    /// 全バックグラウンドペインを均等高で縦に積む（FR-2.16.16）。ライブ更新は `on_term_event` が出力ごとに
    /// 呼ぶ `cx.notify()` の再描画で自動的に得られる
    fn preview_content(&self, target: PreviewTarget) -> gpui::Div {
        let theme = &self.theme;
        match target {
            PreviewTarget::Pane(pane_id) => div()
                .flex_1()
                .p(px(PANE_PADDING))
                .overflow_hidden()
                .bg(rgba(theme.background))
                .children(self.terminal_screen_lines(pane_id, false)),
            PreviewTarget::ClosedGroup(tab) => {
                let mut body = div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p(px(PANE_PADDING))
                    .overflow_hidden()
                    .bg(rgba(theme.background));
                for entry in self.background_entries_of_tab(tab) {
                    let lines = self.terminal_screen_lines(entry.pane, false);
                    body = body.child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .rounded_sm()
                            .border_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                            .child(
                                div()
                                    .flex_none()
                                    .px_1()
                                    .bg(rgba(theme.tab_bar_background))
                                    .text_size(px(9.0))
                                    .text_color(hsla(theme.tab_inactive_foreground))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(truncate(&entry.label, 32))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .overflow_hidden()
                                    .children(lines),
                            ),
                    );
                }
                body
            }
            PreviewTarget::TmuxWindow(pane_id, win) => {
                let text_style = self.text_style();
                let lines = self
                    .window_captures
                    .get(&(pane_id, win))
                    .cloned()
                    .unwrap_or_default();
                let mut body = div()
                    .flex_1()
                    .p(px(PANE_PADDING))
                    .overflow_hidden()
                    .bg(rgba(theme.background));
                for line in lines {
                    body = body.child(
                        div().whitespace_nowrap().child(
                            StyledText::new(SharedString::from(line))
                                .with_default_highlights(&text_style, Vec::new()),
                        ),
                    );
                }
                body
            }
        }
    }

    /// プレビューの本文ボックス（タイトルバー + 実画面サムネイル）を組む。
    /// ホバーポップアップとピン留めウィンドウで共用する（FR-2.16.13）
    fn preview_body(
        &self,
        target: PreviewTarget,
        live: bool,
        extra_title: Option<gpui::Div>,
    ) -> gpui::Div {
        let theme = &self.theme;
        let label = self.preview_label(target);
        let mut titlebar = div()
            .h(px(PIN_TITLE_BAR))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .bg(rgba(theme.tab_bar_background))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(truncate(&label, 40))),
            );
        if live {
            titlebar = titlebar.child(
                div()
                    .flex_none()
                    .text_size(px(9.0))
                    .text_color(hsla(theme.accent))
                    .child("● LIVE"),
            );
        }
        if let Some(extra) = extra_title {
            titlebar = titlebar.child(extra);
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(titlebar)
            .child(self.preview_content(target))
    }

    /// ホバープレビューのポップアップ（FR-2.16.13 / FR-2.16.16）。マウス位置の左側に実画面
    /// サムネイルを出す（tmux ビューは右パネルにあるため左へ伸ばす）。読み取り専用（ピン留めは
    /// 行 / カード側のボタン）。ライブ更新は通常の再描画で得られる
    pub(crate) fn render_hover_preview(&self, window: &Window) -> Option<gpui::AnyElement> {
        let hp = self.hover_preview?;
        let theme = &self.theme;
        // 中身を持たない対象（プレビューペイン等でサムネイル無し）はポップアップを出さない
        if !self.preview_has_content(hp.target) {
            return None;
        }
        let viewport = window.viewport_size();
        let left = (f32::from(hp.anchor.x) - PREVIEW_POPUP_W - 12.0).max(8.0);
        let top = f32::from(hp.anchor.y)
            .min((f32::from(viewport.height) - PREVIEW_POPUP_H - 8.0).max(8.0))
            .max(8.0);
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(PREVIEW_POPUP_W))
                .h(px(PREVIEW_POPUP_H))
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(hsla(theme.accent))
                .bg(rgba(theme.background))
                .child(self.preview_body(hp.target, true, None))
                .into_any_element(),
        )
    }

    /// ピン留めされた常駐プレビュー群（FR-2.16.15）。アプリ内フローティングウィンドウとして
    /// 絶対配置で描き、タイトルバー D&D で移動・× で解除。中身（端末グリッド）はライブ更新される。
    /// 対象が消えた（kill 等）ピンはこのフレームでは描かず、次の操作で掃除される
    pub(crate) fn render_pinned_previews(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let theme = self.theme.clone();
        // 借用衝突を避けるため対象リストを先に取り出す（PinnedPreview は Copy）
        let pins: Vec<PinnedPreview> = self.pinned_previews.clone();
        pins.into_iter()
            .filter(|pin| self.preview_has_content(pin.target))
            .map(|pin| {
                let target = pin.target;
                let key = pin_key(target);
                let label = self.preview_label(target);
                div()
                    .id(("pin", key))
                    .absolute()
                    .left(pin.pos.x)
                    .top(pin.pos.y)
                    .w(px(PIN_W))
                    .h(px(PIN_H))
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(hsla(theme.accent))
                    .bg(rgba(theme.background))
                    // ピン上の操作が下のペインへ抜けないようにする
                    .occlude()
                    .child(
                        // タイトルバー = ドラッグ移動ハンドル + ラベル + LIVE + × 解除
                        div()
                            .id(("pin-title", key))
                            .h(px(PIN_TITLE_BAR))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .bg(rgba(theme.tab_bar_background))
                            .text_size(px(10.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .cursor(CursorStyle::OpenHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                    if let Some(p) =
                                        this.pinned_previews.iter().find(|p| p.target == target)
                                    {
                                        this.dragging_pin = Some((
                                            target,
                                            point(e.position.x - p.pos.x, e.position.y - p.pos.y),
                                        ));
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(truncate(&label, 28))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.0))
                                    .text_color(hsla(theme.accent))
                                    .child("● LIVE"),
                            )
                            .child(
                                div()
                                    .id(("pin-close", key))
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                                    .hover(|d| {
                                        d.bg(rgba_alpha(theme.red, 0.25))
                                            .text_color(hsla(theme.foreground))
                                    })
                                    .child("×")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_pin(target, Some(false));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(self.preview_content(target))
                    .into_any_element()
            })
            .collect()
    }
    /// ペインの画像キャッシュを論理 LRU と GPUI 解放待ちへ移す（Issue #258）。
    pub(crate) fn remove_preview_image_cache(&mut self, pane_id: PaneId) {
        let Some(cache) = self.preview_image_cache.remove(&pane_id) else {
            return;
        };
        for (index, cached) in cache.images.into_iter().enumerate() {
            self.preview_image_lru.remove(&PreviewImageCacheEntryKey {
                pane: pane_id,
                index,
            });
            if let Some(cached) = cached {
                self.pending_preview_image_evictions.push(cached.image);
            }
        }
    }

    pub(crate) fn evict_preview_image_keys(&mut self, keys: Vec<PreviewImageCacheEntryKey>) {
        for key in keys {
            let Some(cached) = self
                .preview_image_cache
                .get_mut(&key.pane)
                .and_then(|cache| cache.images.get_mut(key.index))
                .and_then(Option::take)
            else {
                continue;
            };
            self.pending_preview_image_evictions.push(cached.image);
        }
    }

    /// GPUI の CPU asset と GPU sprite atlas を同じフレームで除去する。
    pub(crate) fn drain_preview_image_evictions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for image in std::mem::take(&mut self.pending_preview_image_evictions) {
            let render_image = image.clone().get_render_image(window, cx);
            image.remove_asset(cx);
            if let Some(render_image) = render_image {
                cx.drop_image(render_image, Some(window));
            }
        }
        for frame in std::mem::take(&mut self.pending_video_frame_evictions) {
            cx.drop_image(frame, Some(window));
        }
    }

    pub(crate) fn remove_video_frame_cache(&mut self, pane_id: PaneId) {
        if let Some((_, frame)) = self.video_frame_cache.remove(&pane_id) {
            self.pending_video_frame_evictions.push(frame);
        }
    }

    /// 必要なページだけプレビュー画像キャッシュへ遅延追加する（Issue #168 / #258）。
    /// PDF は現在ページの近傍以外を `gpui::Image` 化せず、デコード済み BGRA 推定値を
    /// バイト予算つき LRU へ計上する。
    fn ensure_preview_image_cache(&mut self, pane_id: PaneId, wanted: &[usize]) {
        let Some(state) = self.previews.get(&pane_id) else {
            self.remove_preview_image_cache(pane_id);
            return;
        };
        let cache_matches = self.preview_image_cache.get(&pane_id).is_some_and(|cache| {
            match (&cache.key, &state.content) {
                (PreviewImageCacheKey::Pdf(path, key), preview::PreviewContent::Pdf(data)) => {
                    path == &state.path && *key == data.raster_key
                }
                (PreviewImageCacheKey::Original(path), _) => path == &state.path,
                _ => false,
            }
        });
        if !cache_matches {
            let key_and_lengths = match &state.content {
                preview::PreviewContent::Pdf(data) => Some((
                    PreviewImageCacheKey::Pdf(state.path.clone(), data.raster_key),
                    data.pages.len(),
                    data.text_layers
                        .iter()
                        .map(|lines| std::sync::Arc::new(lines.clone()))
                        .collect(),
                )),
                preview::PreviewContent::Image(_) => Some((
                    PreviewImageCacheKey::Original(state.path.clone()),
                    1,
                    Vec::new(),
                )),
                preview::PreviewContent::Video(data) if !data.thumbnail.is_empty() => Some((
                    PreviewImageCacheKey::Original(state.path.clone()),
                    1,
                    Vec::new(),
                )),
                _ => None,
            };
            self.remove_preview_image_cache(pane_id);
            if let Some((key, image_count, text_layers)) = key_and_lengths {
                self.preview_image_cache.insert(
                    pane_id,
                    PreviewImageCache {
                        key,
                        images: std::iter::repeat_with(|| None).take(image_count).collect(),
                        text_layers,
                    },
                );
            }
        }

        for &index in wanted {
            let entry_key = PreviewImageCacheEntryKey {
                pane: pane_id,
                index,
            };
            if self
                .preview_image_cache
                .get(&pane_id)
                .and_then(|cache| cache.images.get(index))
                .is_some_and(Option::is_some)
            {
                self.preview_image_lru.touch(&entry_key);
                continue;
            }

            let built = match self.previews.get(&pane_id).map(|state| &state.content) {
                Some(preview::PreviewContent::Pdf(data)) => data.pages.get(index).and_then(|png| {
                    (!png.is_empty()).then(|| CachedPreviewImage {
                        image: std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            png.clone(),
                        )),
                        decoded_bytes: data
                            .pixel_sizes
                            .get(index)
                            .map(|[w, h]| u64::from(*w) * u64::from(*h) * 4)
                            .unwrap_or(png.len() as u64),
                    })
                }),
                Some(preview::PreviewContent::Image(data)) => {
                    let gpui_format = match data.format {
                        preview::ImageFileFormat::Png => gpui::ImageFormat::Png,
                        preview::ImageFileFormat::Jpeg => gpui::ImageFormat::Jpeg,
                        preview::ImageFileFormat::Gif => gpui::ImageFormat::Gif,
                        preview::ImageFileFormat::WebP => gpui::ImageFormat::Webp,
                        preview::ImageFileFormat::Svg => gpui::ImageFormat::Svg,
                    };
                    Some(CachedPreviewImage {
                        image: std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui_format,
                            data.bytes.clone(),
                        )),
                        decoded_bytes: data
                            .pixel_size
                            .map(|(w, h)| u64::from(w) * u64::from(h) * 4)
                            .unwrap_or(data.bytes.len() as u64),
                    })
                }
                Some(preview::PreviewContent::Video(data)) if !data.thumbnail.is_empty() => {
                    Some(CachedPreviewImage {
                        image: std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            data.thumbnail.clone(),
                        )),
                        decoded_bytes: data
                            .resolution
                            .map(|(w, h)| u64::from(w) * u64::from(h) * 4)
                            .unwrap_or(data.thumbnail.len() as u64),
                    })
                }
                _ => None,
            };
            let Some(cached) = built else {
                continue;
            };
            let decoded_bytes = cached.decoded_bytes;
            if let Some(slot) = self
                .preview_image_cache
                .get_mut(&pane_id)
                .and_then(|cache| cache.images.get_mut(index))
            {
                *slot = Some(cached);
            }
            let evicted = self.preview_image_lru.insert(entry_key, decoded_bytes);
            self.evict_preview_image_keys(evicted);
        }
    }

    pub(crate) fn render_preview_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        self.ensure_pdf_raster_quality(pane_id, area, cx);
        let wanted_images: Vec<usize> = match self.previews.get(&pane_id).map(|p| &p.content) {
            Some(preview::PreviewContent::Pdf(data)) => {
                let current = self
                    .preview_scroll_handles
                    .get(&pane_id)
                    .filter(|handle| handle.bounds_for_item(0).is_some())
                    .map(gpui::ScrollHandle::top_item)
                    .unwrap_or_else(|| {
                        self.preview_views
                            .get(&pane_id)
                            .map(|view| view.page.saturating_sub(1))
                            .unwrap_or(0)
                    })
                    .min(data.total_pages.saturating_sub(1));
                wanted_pdf_image_indices(current, data.total_pages)
            }
            Some(preview::PreviewContent::Image(_)) | Some(preview::PreviewContent::Video(_)) => {
                vec![0]
            }
            _ => Vec::new(),
        };
        self.ensure_preview_image_cache(pane_id, &wanted_images);
        let state = self.previews.get(&pane_id).expect("呼び出し前に確認済み");
        let file_name = state.file_name();
        let path_label = state.path.display().to_string();
        let md_capable = state.markdown_capable();
        let mode = state.mode;
        let truncated = state.truncated;
        let preview_outline = state.outline.clone();
        let outline_available = !preview_outline.is_empty();
        struct EditSnapshot {
            editing: bool,
            dirty: bool,
            message: Option<String>,
            cursor_pos: (usize, usize),
            save_status: Option<preview::SaveStatus>,
            autosave: bool,
            search_visible: bool,
            search_focus: preview::SearchFieldFocus,
            search_query: String,
            search_cursor: usize,
            search_total: usize,
            search_index: usize,
            search_hits: Vec<tako_core::SearchHit>,
            replace_text: String,
            replace_cursor: usize,
            ime_text: Option<String>,
        }
        let ime_for_search = self
            .ime
            .as_ref()
            .filter(|ime| ime.pane == pane_id)
            .map(|ime| ime.text.clone());
        let edit_snap = self.preview_edits.get(&pane_id).map(|edit| EditSnapshot {
            editing: edit.editing,
            dirty: edit.dirty(),
            message: edit.message.clone(),
            cursor_pos: edit.buffer.line_byte_col(edit.buffer.cursor()),
            save_status: edit.save_status.clone(),
            autosave: edit.autosave,
            search_visible: edit.search_visible,
            search_focus: edit.search_focus,
            search_query: edit.search_query.clone(),
            search_cursor: edit.search_cursor,
            search_total: edit.search_hits.len(),
            search_index: edit.search_index,
            search_hits: edit.search_hits.clone(),
            replace_text: edit.replace_text.clone(),
            replace_cursor: edit.replace_cursor,
            ime_text: if edit.search_visible {
                ime_for_search.clone()
            } else {
                None
            },
        });
        let editing = edit_snap.as_ref().is_some_and(|s| s.editing);
        let dirty = edit_snap.as_ref().is_some_and(|s| s.dirty);
        let edit_message = edit_snap.as_ref().and_then(|s| s.message.clone());
        let edit_cursor = edit_snap
            .as_ref()
            .filter(|s| s.editing)
            .map(|s| s.cursor_pos);
        let save_status = edit_snap.as_ref().and_then(|s| s.save_status.clone());
        let autosave = edit_snap.as_ref().is_some_and(|s| s.autosave);
        let search_visible = edit_snap.as_ref().is_some_and(|s| s.search_visible);
        let search_focus = edit_snap
            .as_ref()
            .map(|s| s.search_focus)
            .unwrap_or(preview::SearchFieldFocus::Query);
        let search_query = edit_snap
            .as_ref()
            .map(|s| s.search_query.clone())
            .unwrap_or_default();
        let search_cursor = edit_snap.as_ref().map(|s| s.search_cursor).unwrap_or(0);
        let search_total = edit_snap.as_ref().map(|s| s.search_total).unwrap_or(0);
        let search_index = edit_snap.as_ref().map(|s| s.search_index).unwrap_or(0);
        let replace_text = edit_snap
            .as_ref()
            .map(|s| s.replace_text.clone())
            .unwrap_or_default();
        let replace_cursor = edit_snap.as_ref().map(|s| s.replace_cursor).unwrap_or(0);
        let editable = matches!(
            &state.content,
            preview::PreviewContent::Code(_) | preview::PreviewContent::Markdown(_)
        ) && !truncated;
        let zoomable = matches!(
            mode,
            preview::PreviewMode::Image | preview::PreviewMode::Pdf
        );
        let preview_view = self
            .preview_views
            .get(&pane_id)
            .copied()
            .unwrap_or_default();
        let preview_zoom = preview_view.zoom;
        let viewport_width = (f32::from(area.size.width) - (PANE_PADDING + 4.0) * 2.0).max(1.0);
        let viewport_height = (f32::from(area.size.height) - (PANE_PADDING + 4.0) * 2.0).max(1.0);

        let pdf_info: Option<(usize, usize)> =
            if let preview::PreviewContent::Pdf(data) = &state.content {
                let current = self
                    .preview_scroll_handles
                    .get(&pane_id)
                    .filter(|handle| handle.bounds_for_item(0).is_some())
                    .map(|handle| handle.top_item() + 1)
                    .unwrap_or(preview_view.page)
                    .clamp(1, data.total_pages.max(1));
                Some((current, data.total_pages))
            } else {
                None
            };
        let navigation_panel = self
            .preview_navigation_panel
            .filter(|(open_pane, _)| *open_pane == pane_id)
            .map(|(_, panel)| panel);

        // 選択状態
        let selection = self.preview_selections.get(&pane_id).cloned();
        let pdf_highlight_bounds = match (&state.content, selection.as_ref()) {
            (preview::PreviewContent::Pdf(data), Some(selection)) => self
                .preview_pdf_char_bounds
                .get(&pane_id)
                .map(|bounds| pdf_selection_highlight_bounds(data, bounds, selection))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if pdf_highlight_bounds.is_empty() {
            self.preview_pdf_highlight_paint_count.remove(&pane_id);
        }

        // テキスト行を収集（選択テキスト抽出 + bounds 追跡用）
        let mut line_texts: Vec<String> = Vec::new();
        // Code / Markdown は StyledText 自身の TextLayout を保持し、ヒットテストと
        // キャレット描画を実際の shaping 結果に一致させる。
        let mut line_layouts: Vec<Option<TextLayout>> = Vec::new();

        // 検索ヒット情報（ハイライト描画用）
        let search_hits = edit_snap
            .as_ref()
            .filter(|s| s.search_visible && !s.search_hits.is_empty())
            .map(|s| (s.search_hits.as_slice(), s.search_index));

        // チェンジログビューの判定（Issue #338）
        let changelog_active = self.preview_changelogs.contains_key(&pane_id);
        let changelog_data = self.preview_changelogs.get(&pane_id).cloned();

        // 本文要素を先に組む（state の借用をここで終える）
        let body: Vec<gpui::AnyElement> = if let Some(cl) = &changelog_data {
            self.render_changelog_body(pane_id, cl, &theme, cx)
        } else {
            match &state.content {
                preview::PreviewContent::Code(lines) => {
                    let number_width = lines.len().to_string().len();
                    let mut doc_offset: usize = 0;
                    lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| {
                            let text: String = line.iter().map(|s| s.text.as_str()).collect();
                            let line_start = doc_offset;
                            let line_end = doc_offset + text.len();
                            doc_offset = line_end + 1; // +1 for '\n'
                            let sel_range = selection
                                .as_ref()
                                .and_then(|s| s.range_for_line(i, text.len()));
                            let hit_ranges = search_hits
                                .map(|(hits, idx)| {
                                    search_hits_for_line(hits, idx, line_start, line_end)
                                })
                                .unwrap_or_default();
                            line_texts.push(text);
                            let cursor_col = edit_cursor
                                .filter(|(line, _)| *line == i)
                                .map(|(_, col)| col);
                            let (element, layout) = self.preview_code_line_sel(
                                line,
                                Some((i + 1, number_width)),
                                (sel_range, cursor_col),
                                &hit_ranges,
                                cx,
                            );
                            line_layouts.push(Some(layout));
                            element.into_any_element()
                        })
                        .collect()
                }
                preview::PreviewContent::Markdown(blocks) => {
                    let mut doc_offset: usize = 0;
                    blocks
                        .iter()
                        .enumerate()
                        .map(|(i, block)| {
                            let text = md_block_plain_text(block);
                            let line_start = doc_offset;
                            let line_end = doc_offset + text.len();
                            doc_offset = line_end + 1;
                            let sel_range = selection
                                .as_ref()
                                .and_then(|s| s.range_for_line(i, text.len()));
                            let hit_ranges = search_hits
                                .map(|(hits, idx)| {
                                    search_hits_for_line(hits, idx, line_start, line_end)
                                })
                                .unwrap_or_default();
                            line_texts.push(text);
                            let (element, layout) =
                                self.preview_md_block_sel(block, sel_range, &hit_ranges);
                            line_layouts.push(layout);
                            element
                        })
                        .collect()
                }
                preview::PreviewContent::Image(_) => {
                    // Issue #168: Image はキャッシュ済み（ensure_preview_image_cache）
                    let image = self
                        .preview_image_cache
                        .get(&pane_id)
                        .and_then(|c| c.images.first())
                        .and_then(|i| i.as_ref().map(|cached| cached.image.clone()));
                    match image {
                        Some(image) => {
                            let scaled_width = viewport_width * preview_zoom;
                            let scaled_height = viewport_height * preview_zoom;
                            let canvas_width = viewport_width.max(scaled_width);
                            let canvas_height = viewport_height.max(scaled_height);
                            vec![div()
                                .flex_none()
                                .w(px(canvas_width))
                                .h(px(canvas_height))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui::img(image)
                                        .object_fit(gpui::ObjectFit::Contain)
                                        .w(px(scaled_width))
                                        .h(px(scaled_height)),
                                )
                                .into_any_element()]
                        }
                        None => Vec::new(),
                    }
                }
                preview::PreviewContent::Pdf(data) => {
                    // テキスト行を line_texts に登録（選択テキスト抽出用）
                    let mut global_line_idx: usize = 0;
                    for page_lines in &data.text_layers {
                        for tl in page_lines {
                            line_texts.push(tl.text.clone());
                            global_line_idx += 1;
                        }
                    }
                    let _ = global_line_idx;

                    let mut line_offset: usize = 0;
                    // Issue #168: ページ画像とテキストレイヤは ensure_preview_image_cache が
                    // 構築済み。ここでは Arc clone だけ行う（旧実装は毎フレーム全ページの
                    // PNG clone + Image::from_bytes のフルハッシュで p50 96ms/frame だった）
                    let empty_cache = PreviewImageCache {
                        key: PreviewImageCacheKey::Original(PathBuf::new()),
                        images: Vec::new(),
                        text_layers: Vec::new(),
                    };
                    let cache = self
                        .preview_image_cache
                        .get(&pane_id)
                        .unwrap_or(&empty_cache);
                    data.pages
                        .iter()
                        .enumerate()
                        .filter(|(_, png)| !png.is_empty())
                        .map(|(i, _)| {
                            // ensure_preview_image_cache 直後なので None は起きない想定
                            // （防御: 欠損時は空要素を返し、次フレームの再構築に任せる）
                            let image = cache
                                .images
                                .get(i)
                                .and_then(|img| img.as_ref().map(|cached| cached.image.clone()));
                            let page_text_lines = data.text_layers.get(i);
                            let page_size =
                                data.page_sizes.get(i).copied().unwrap_or([612.0, 792.0]);
                            let page_line_offset = line_offset;
                            let n_lines = page_text_lines.map(|l| l.len()).unwrap_or(0);
                            line_offset += n_lines;

                            let entity = cx.entity().downgrade();
                            let text_lines_for_canvas: std::sync::Arc<Vec<preview::PdfTextLine>> =
                                cache.text_layers.get(i).cloned().unwrap_or_default();
                            let pdf_w = page_size[0];
                            let pdf_h = page_size[1];
                            let scaled_page_width = viewport_width * preview_zoom;
                            let scaled_page_height = if pdf_w > 0.0 {
                                scaled_page_width * (pdf_h / pdf_w) as f32
                            } else {
                                scaled_page_width
                            };
                            let page_canvas_width = viewport_width.max(scaled_page_width);

                            let page_index = i;
                            let hovered_link_bbox: Option<[f64; 4]> = self
                                .preview_pdf_hovered_link
                                .filter(|(pid, _)| *pid == pane_id)
                                .and_then(|(_, link_idx)| data.links.links.get(link_idx))
                                .filter(|link| link.page_index == i)
                                .map(|link| link.bbox);
                            let link_accent = theme.accent;
                            let overlay = canvas(
                                |_, _, _| (),
                                move |bounds, _, window, cx| {
                                    let img_w = f32::from(bounds.size.width) as f64;
                                    let img_h = f32::from(bounds.size.height) as f64;
                                    if img_w <= 0.0 || img_h <= 0.0 {
                                        return;
                                    }
                                    let has_text = !text_lines_for_canvas.is_empty();
                                    let page_line_bounds: Vec<Bounds<Pixels>> = if has_text {
                                        text_lines_for_canvas
                                            .iter()
                                            .map(|line| {
                                                pdf_box_to_screen(line.bbox, [pdf_w, pdf_h], bounds)
                                            })
                                            .collect()
                                    } else {
                                        Vec::new()
                                    };
                                    let page_char_bounds: Vec<Vec<Bounds<Pixels>>> = if has_text {
                                        text_lines_for_canvas
                                            .iter()
                                            .map(|tl| {
                                                tl.char_boxes
                                                    .iter()
                                                    .map(|ch| {
                                                        pdf_box_to_screen(
                                                            ch.bbox,
                                                            [pdf_w, pdf_h],
                                                            bounds,
                                                        )
                                                    })
                                                    .collect()
                                            })
                                            .collect()
                                    } else {
                                        Vec::new()
                                    };

                                    if let Some(e) = entity.upgrade() {
                                        cx.defer(move |cx| {
                                            e.update(cx, |app, cx| {
                                                // ページ画像 bounds を直接記録（#315）
                                                app.preview_pdf_page_image_bounds
                                                    .entry(pane_id)
                                                    .or_default()
                                                    .insert(page_index, bounds);

                                                if has_text {
                                                    let lines = app
                                                        .preview_line_bounds
                                                        .entry(pane_id)
                                                        .or_default();
                                                    for (j, line_bounds) in
                                                        page_line_bounds.iter().enumerate()
                                                    {
                                                        let idx = page_line_offset + j;
                                                        if lines.len() <= idx {
                                                            lines
                                                                .resize(idx + 1, Bounds::default());
                                                        }
                                                        lines[idx] = *line_bounds;
                                                    }

                                                    let chars = app
                                                        .preview_pdf_char_bounds
                                                        .entry(pane_id)
                                                        .or_default();
                                                    let mut changed = false;
                                                    for (j, line_bounds) in
                                                        page_char_bounds.iter().enumerate()
                                                    {
                                                        let idx = page_line_offset + j;
                                                        if chars.len() <= idx {
                                                            chars.resize(idx + 1, Vec::new());
                                                        }
                                                        if chars[idx] != *line_bounds {
                                                            chars[idx] = line_bounds.clone();
                                                            changed = true;
                                                        }
                                                    }
                                                    if changed {
                                                        cx.notify();
                                                    }
                                                }
                                            });
                                        });
                                    }

                                    // ホバー中リンクの下線ハイライト描画（#315）
                                    if let Some(bbox) = hovered_link_bbox {
                                        let link_bounds =
                                            pdf_box_to_screen(bbox, [pdf_w, pdf_h], bounds);
                                        let underline_h = px(2.0);
                                        let underline = Bounds {
                                            origin: point(
                                                link_bounds.origin.x,
                                                link_bounds.origin.y + link_bounds.size.height
                                                    - underline_h,
                                            ),
                                            size: gpui::size(link_bounds.size.width, underline_h),
                                        };
                                        window.paint_quad(fill(underline, hsla(link_accent)));
                                    }
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full();

                            let mut surface = div()
                                .relative()
                                .w(px(scaled_page_width))
                                .h(px(scaled_page_height))
                                .bg(hsla(theme.background));
                            if let Some(image) = image {
                                surface = surface
                                    .child(
                                        gpui::img(image)
                                            .object_fit(gpui::ObjectFit::Contain)
                                            .size_full(),
                                    )
                                    .child(overlay);
                            }

                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .flex_none()
                                .w(px(page_canvas_width))
                                .pb_2()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                                        .pb_1()
                                        .child(SharedString::from(format!(
                                            "— {} / {} —",
                                            i + 1,
                                            data.total_pages
                                        ))),
                                )
                                .child(surface)
                                .into_any_element()
                        })
                        .collect()
                }
                preview::PreviewContent::Video(data) => {
                    let mut elements: Vec<gpui::AnyElement> = Vec::new();

                    if let Some(player) = self.video_players.get(&pane_id) {
                        // AVFoundation プレイヤー起動中: キャッシュ済みフレームを表示
                        let gen = player.frame_gen;
                        let need_update = match self.video_frame_cache.get(&pane_id) {
                            Some((cached_gen, _)) => *cached_gen != gen,
                            None => true,
                        };
                        if need_update && !player.current_bgra.is_empty() {
                            let w = player.width;
                            let h = player.height;
                            if let Some(rgba_img) =
                                image::RgbaImage::from_raw(w, h, player.current_bgra.clone())
                            {
                                let frame = image::Frame::new(rgba_img);
                                let render =
                                    std::sync::Arc::new(gpui::RenderImage::new(vec![frame]));
                                if let Some((_, old)) =
                                    self.video_frame_cache.insert(pane_id, (gen, render))
                                {
                                    self.pending_video_frame_evictions.push(old);
                                }
                            }
                        }
                        if let Some((_, ref frame_image)) = self.video_frame_cache.get(&pane_id) {
                            let frame_image = frame_image.clone();
                            elements.push(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        gpui::img(frame_image)
                                            .object_fit(gpui::ObjectFit::Contain)
                                            .max_w_full()
                                            .flex_1(),
                                    )
                                    .into_any_element(),
                            );
                        }
                        let is_playing = player.state == video_player::PlaybackState::Playing;
                        let current_time = player.current_time;
                        let duration = player.duration;
                        let current_rate = player.rate;

                        let play_btn_label: SharedString = if is_playing {
                            "\u{23f8}".into() // ⏸
                        } else {
                            "\u{25b6}\u{fe0e}".into() // ▶︎
                        };
                        let cur_m = current_time as u64 / 60;
                        let cur_s = current_time as u64 % 60;
                        let dur_m = duration as u64 / 60;
                        let dur_s = duration as u64 % 60;
                        let time_label: SharedString =
                            format!("{cur_m}:{cur_s:02} / {dur_m}:{dur_s:02}").into();
                        let progress_frac = if duration > 0.0 {
                            (current_time / duration).clamp(0.0, 1.0) as f32
                        } else {
                            0.0
                        };
                        let seek_dur = duration;

                        let is_muted = player.muted;
                        let is_looping = player.looping;

                        // ホバー時刻ツールチップ
                        let hover_tooltip = self
                            .video_seek_hover
                            .filter(|&(pid, _, _)| pid == pane_id)
                            .map(|(_, hover_sec, hover_x)| {
                                let hm = hover_sec as u64 / 60;
                                let hs = hover_sec as u64 % 60;
                                let label: SharedString = format!("{hm}:{hs:02}").into();
                                div()
                                    .absolute()
                                    .bottom(px(16.0))
                                    .left(px(hover_x - 20.0))
                                    .px(px(4.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(hsla_alpha(theme.background, 0.95))
                                    .border_1()
                                    .border_color(hsla_alpha(theme.foreground, 0.3))
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.foreground))
                                    .child(label)
                            });

                        // シークバー（クリック + ドラッグ対応 + つまみノブ + ホバー時刻）
                        let mut seek_bar = div()
                            .id(("video-seek", pane_id.as_u64()))
                            .relative()
                            .flex_1()
                            .h(px(14.0))
                            .cursor_pointer()
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(px(4.0))
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(hsla_alpha(theme.foreground, 0.2))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(3.0))
                                            .bg(hsla(theme.ansi[4]))
                                            .w(relative(progress_frac)),
                                    ),
                            )
                            // つまみノブ
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.0))
                                    .left(relative(progress_frac))
                                    .ml(px(-6.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(theme.ansi[4])),
                            )
                            .child({
                                let entity = cx.entity().downgrade();
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, cx| {
                                        if let Some(e) = entity.upgrade() {
                                            e.update(cx, |app, _| {
                                                app.video_seek_bar_bounds.insert(pane_id, bounds);
                                            });
                                        }
                                    },
                                )
                                .absolute()
                                .size_full()
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    this.video_seek_dragging = Some(pane_id);
                                    this.video_seek_by_click(pane_id, ev.position, seek_dur, cx);
                                }),
                            )
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _ev: &gpui::MouseUpEvent, _, _cx| {
                                    if this.video_seek_dragging == Some(pane_id) {
                                        this.video_seek_dragging = None;
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                                if this.video_seek_dragging == Some(pane_id) {
                                    this.video_seek_by_drag(pane_id, ev.position, cx);
                                }
                                // ホバー時刻を計算
                                if let Some(bounds) =
                                    this.video_seek_bar_bounds.get(&pane_id).copied()
                                {
                                    let bar_x = f32::from(bounds.origin.x);
                                    let bar_w = f32::from(bounds.size.width);
                                    let mouse_x = f32::from(ev.position.x);
                                    if bar_w > 0.0 {
                                        let frac = ((mouse_x - bar_x) / bar_w).clamp(0.0, 1.0);
                                        let hover_sec = frac as f64 * seek_dur;
                                        let rel_x = mouse_x - bar_x;
                                        this.video_seek_hover = Some((pane_id, hover_sec, rel_x));
                                        cx.notify();
                                    }
                                }
                            }))
                            .on_mouse_up_out(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, _| {
                                    if this.video_seek_dragging == Some(pane_id) {
                                        this.video_seek_dragging = None;
                                    }
                                }),
                            );
                        if let Some(tooltip) = hover_tooltip {
                            seek_bar = seek_bar.child(tooltip);
                        }

                        // 再生速度ボタン
                        let rates: &[(f32, &str)] =
                            &[(0.5, "0.5x"), (1.0, "1x"), (1.5, "1.5x"), (2.0, "2x")];
                        let speed_buttons =
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .children(rates.iter().map(|&(rate, label)| {
                                    let is_active = (current_rate - rate).abs() < 0.01;
                                    div()
                                        .id((
                                            "video-rate",
                                            pane_id.as_u64() * 100 + (rate * 10.0) as u64,
                                        ))
                                        .cursor_pointer()
                                        .px(px(4.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .text_size(px(11.0))
                                        .when(is_active, |d| {
                                            d.bg(hsla(theme.ansi[4]))
                                                .text_color(hsla(theme.background))
                                        })
                                        .when(!is_active, |d| {
                                            d.text_color(hsla_alpha(theme.foreground, 0.6))
                                                .hover(|s| s.bg(hsla_alpha(theme.foreground, 0.1)))
                                        })
                                        .child(SharedString::from(label))
                                        .on_click(cx.listener(
                                            move |this, _ev: &gpui::ClickEvent, _, cx| {
                                                if let Some(p) =
                                                    this.video_players.get_mut(&pane_id)
                                                {
                                                    p.set_rate(rate);
                                                    cx.notify();
                                                }
                                            },
                                        ))
                                        .into_any_element()
                                }));

                        // ミュートボタン（絵文字全廃 #217: SVG）
                        let mute_btn = div()
                            .id(("video-mute", pane_id.as_u64()))
                            .cursor_pointer()
                            .px(px(2.0))
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(hsla_alpha(theme.foreground, 0.1)))
                            .child(
                                svg()
                                    .path(if is_muted {
                                        crate::file_icons::ui_icon::VOLUME_OFF
                                    } else {
                                        crate::file_icons::ui_icon::VOLUME_ON
                                    })
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .text_color(hsla_alpha(theme.foreground, 0.8)),
                            )
                            .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _, cx| {
                                if let Some(p) = this.video_players.get_mut(&pane_id) {
                                    p.toggle_mute();
                                    cx.notify();
                                }
                            }));

                        // ループトグルボタン
                        let loop_btn = div()
                            .id(("video-loop", pane_id.as_u64()))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .when(is_looping, |d| {
                                d.bg(hsla(theme.ansi[4])).text_color(hsla(theme.background))
                            })
                            .when(!is_looping, |d| {
                                d.text_color(hsla_alpha(theme.foreground, 0.6))
                                    .hover(|s| s.bg(hsla_alpha(theme.foreground, 0.1)))
                            })
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::LOOP_REPEAT)
                                    .w(px(13.0))
                                    .h(px(13.0))
                                    .text_color(if is_looping {
                                        hsla(theme.background)
                                    } else {
                                        hsla_alpha(theme.foreground, 0.7)
                                    }),
                            )
                            .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _, cx| {
                                if let Some(p) = this.video_players.get_mut(&pane_id) {
                                    p.toggle_loop();
                                    cx.notify();
                                }
                            }));

                        // コントロールバー: 再生/一時停止 + シークバー + 時間 + 速度 + ミュート + ループ
                        elements.push(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .bg(hsla_alpha(theme.background, 0.9))
                                .child(
                                    div()
                                        .id(("video-toggle", pane_id.as_u64()))
                                        .cursor_pointer()
                                        .text_size(px(18.0))
                                        .child(play_btn_label)
                                        .on_click(cx.listener(
                                            move |this, _ev: &gpui::ClickEvent, _, cx| {
                                                if let Some(p) =
                                                    this.video_players.get_mut(&pane_id)
                                                {
                                                    p.toggle();
                                                    this.ensure_video_ticker(cx);
                                                    cx.notify();
                                                }
                                            },
                                        )),
                                )
                                .child(seek_bar)
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(hsla_alpha(theme.foreground, 0.7))
                                        .child(time_label),
                                )
                                .child(speed_buttons)
                                .child(mute_btn)
                                .child(loop_btn)
                                .into_any_element(),
                        );
                    } else {
                        // プレイヤー未起動: ffmpeg サムネイル + 再生ボタン + メタ情報
                        if let Some(image) = self
                            .preview_image_cache
                            .get(&pane_id)
                            .and_then(|c| c.images.first())
                            .and_then(|i| i.as_ref().map(|cached| cached.image.clone()))
                        {
                            // Issue #168: サムネもキャッシュ済み Arc を使う（毎フレームの
                            // from_bytes ハッシュ計算を避ける）
                            elements.push(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .relative()
                                    .p_2()
                                    .child(
                                        gpui::img(image)
                                            .object_fit(gpui::ObjectFit::Contain)
                                            .max_w_full()
                                            .max_h(px(400.0)),
                                    )
                                    .into_any_element(),
                            );
                        }
                        // 再生ボタン
                        elements.push(
                            div()
                                .flex()
                                .justify_center()
                                .p_2()
                                .child(
                                    div()
                                        .id(("video-play", pane_id.as_u64()))
                                        .cursor_pointer()
                                        .px_4()
                                        .py_1()
                                        .rounded(px(6.0))
                                        .bg(hsla(theme.ansi[4]))
                                        .text_color(hsla(theme.background))
                                        .text_size(px(14.0))
                                        .child(SharedString::from("\u{25b6}\u{fe0e} 再生"))
                                        .on_click(cx.listener(
                                            move |this, _ev: &gpui::ClickEvent, _, cx| {
                                                this.start_video_player(pane_id, cx);
                                            },
                                        )),
                                )
                                .into_any_element(),
                        );
                        // メタ情報
                        let mut info_lines = Vec::new();
                        if let Some((w, h)) = data.resolution {
                            info_lines.push(format!("解像度: {w} x {h}"));
                        }
                        if let Some(dur) = data.duration {
                            let mins = dur as u64 / 60;
                            let secs = dur as u64 % 60;
                            info_lines.push(format!("長さ: {mins}:{secs:02}"));
                        }
                        if let Some(codec) = &data.codec {
                            info_lines.push(format!("コーデック: {codec}"));
                        }
                        let size_mb = data.file_size as f64 / 1_000_000.0;
                        if size_mb >= 1.0 {
                            info_lines.push(format!("サイズ: {size_mb:.1} MB"));
                        } else {
                            info_lines
                                .push(format!("サイズ: {:.0} KB", data.file_size as f64 / 1_000.0));
                        }
                        elements.push(
                            div()
                                .p_2()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_size(px(13.0))
                                .text_color(hsla_alpha(theme.foreground, 0.8))
                                .children(info_lines.into_iter().map(|line| {
                                    div().child(SharedString::from(line)).into_any_element()
                                }))
                                .into_any_element(),
                        );
                    }
                    elements
                }
                preview::PreviewContent::Loading => vec![div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .text_color(hsla_alpha(theme.foreground, 0.6))
                    .child(SharedString::from("読み込み中…"))
                    .into_any_element()],
                preview::PreviewContent::Error(message) => vec![div()
                    .p_2()
                    .text_color(hsla(theme.red))
                    .child(SharedString::from(message.clone()))
                    .into_any_element()],
            }
        };

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .rounded(px(7.0))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(focused, |d| {
                d.shadow(vec![
                    BoxShadow {
                        color: hsla_alpha(theme.accent, 0.25),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: false,
                    },
                    BoxShadow {
                        color: gpui::hsla(0., 0., 0., 0.35),
                        offset: point(px(0.), px(8.)),
                        blur_radius: px(24.),
                        spread_radius: px(0.),
                        inset: false,
                    },
                ])
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    let _ = this.workspace.active_tab_mut().tree_mut().focus(pane_id);
                    cx.notify();
                }),
            )
            .child({
                // #185: プレビューヘッダ — × を右側に統一 + パス中間省略 + 省略制御
                let preview_w = f32::from(area.size.width);
                let phv = tako_core::PreviewHeaderVisibility::from_width(preview_w);
                let path_max = ((preview_w / 8.0) as usize).clamp(12, 60);
                let path_short = tako_core::truncate_path_middle(&path_label, path_max);
                div()
                    .id(("preview-titlebar", pane_id.as_u64()))
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .bg(rgba(if focused {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .border_b_1()
                    .border_color(hsla(if focused {
                        theme.border_default
                    } else {
                        theme.border_subtle
                    }))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        PaneDrag { pane: pane_id },
                        self.drag_ghost_builder(DragKind::Pane, truncate(&file_name, 24), cx),
                    )
                    // #185: プレビューヘッダ右クリックメニュー
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.pane_context_menu = Some(PaneContextMenu {
                                pane: pane_id,
                                kind: PaneContextKind::Preview,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
                    // 左コンテナ: ファイル情報
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .when(phv.file_icon, |d| {
                                d.child(
                                    svg()
                                        .path(match mode {
                                            preview::PreviewMode::Image => {
                                                "icons/file_icons/image.svg"
                                            }
                                            preview::PreviewMode::Pdf => {
                                                "icons/file_icons/book.svg"
                                            }
                                            _ => crate::file_icons::ui_icon::FILE_GENERIC,
                                        })
                                        .w(px(13.0))
                                        .h(px(13.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_tertiary)),
                                )
                            })
                            .when(phv.file_name, |d| {
                                d.child(
                                    div()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(if focused {
                                            hsla(theme.foreground)
                                        } else {
                                            hsla(theme.tab_inactive_foreground)
                                        })
                                        .child(SharedString::from({
                                            let suffix = if autosave {
                                                match &save_status {
                                                    Some(preview::SaveStatus::Saved) => {
                                                        " \u{00B7} 保存済"
                                                    }
                                                    Some(preview::SaveStatus::Conflict) => {
                                                        " \u{00B7} 競合"
                                                    }
                                                    Some(preview::SaveStatus::Error(_)) => {
                                                        " \u{00B7} エラー"
                                                    }
                                                    None if dirty => " \u{25CF}",
                                                    None => "",
                                                }
                                            } else if dirty {
                                                " \u{25CF}"
                                            } else {
                                                ""
                                            };
                                            format!("{}{suffix}", truncate(&file_name, 36))
                                        })),
                                )
                            })
                            .when(phv.path_label, |d| {
                                d.child(
                                    div()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                                        .text_size(px(10.0))
                                        .child(SharedString::from(path_short)),
                                )
                            })
                            .children(edit_message.map(|message| {
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(hsla(if dirty {
                                        theme.yellow
                                    } else {
                                        theme.green
                                    }))
                                    .child(SharedString::from(truncate(&message, 36)))
                            }))
                            .when(phv.page_info, |d| {
                                d.children(pdf_info.map(|(current, total)| {
                                    div()
                                        .id(("preview-page-picker", pane_id.as_u64()))
                                        .flex_none()
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                                        .hover(|d| d.bg(rgba(theme.surface_hover)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.preview_navigation_panel =
                                                if this.preview_navigation_panel
                                                    == Some((
                                                        pane_id,
                                                        PreviewNavigationPanel::Pages,
                                                    ))
                                                {
                                                    None
                                                } else {
                                                    Some((
                                                        pane_id,
                                                        PreviewNavigationPanel::Pages,
                                                    ))
                                                };
                                            cx.notify();
                                        }))
                                        .child(SharedString::from(format!("{current} / {total}")))
                                }))
                            }),
                    )
                    // 右コンテナ: 操作ボタン（常に表示）
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .when(outline_available, |d| {
                                d.child(
                                    div()
                                        .id(("preview-outline-toggle", pane_id.as_u64()))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(3.0))
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(theme.accent))
                                        .hover(|d| {
                                            d.bg(rgba_alpha(theme.tab_active_background, 0.8))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.preview_navigation_panel =
                                                if this.preview_navigation_panel
                                                    == Some((
                                                        pane_id,
                                                        PreviewNavigationPanel::Outline,
                                                    ))
                                                {
                                                    None
                                                } else {
                                                    Some((
                                                        pane_id,
                                                        PreviewNavigationPanel::Outline,
                                                    ))
                                                };
                                            cx.notify();
                                        }))
                                        .child(
                                            svg()
                                                .path("icons/file_icons/book.svg")
                                                .w(px(12.0))
                                                .h(px(12.0))
                                                .text_color(hsla(theme.accent)),
                                        )
                                        .child("目次")
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::CHEVRON_DOWN)
                                                .w(px(9.0))
                                                .h(px(9.0))
                                                .text_color(hsla(theme.text_tertiary)),
                                        ),
                                )
                            })
                            .when(zoomable, |d| {
                                let zoom_out_pane = pane_id;
                                let zoom_in_pane = pane_id;
                                let reset_pane = pane_id;
                                d.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(hsla(theme.border_subtle))
                                        .child(
                                            div()
                                                .id(("preview-zoom-out", pane_id.as_u64()))
                                                .p(px(3.0))
                                                .cursor_pointer()
                                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                                .child(
                                                    svg()
                                                        .path(crate::file_icons::ui_icon::MINUS)
                                                        .w(px(10.0))
                                                        .h(px(10.0))
                                                        .text_color(hsla(theme.text_secondary)),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let current = this
                                                        .preview_views
                                                        .get(&zoom_out_pane)
                                                        .copied()
                                                        .unwrap_or_default()
                                                        .zoom;
                                                    this.set_preview_zoom_about(
                                                        zoom_out_pane,
                                                        current / tako_core::PREVIEW_ZOOM_STEP,
                                                        None,
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id(("preview-zoom-reset", pane_id.as_u64()))
                                                .px(px(3.0))
                                                .cursor_pointer()
                                                .text_size(px(10.0))
                                                .text_color(hsla(theme.text_secondary))
                                                .child(SharedString::from(format!(
                                                    "{}%",
                                                    (preview_zoom * 100.0).round() as u32
                                                )))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if <TakoApp as PreviewHost>::update_preview_view(
                                                        this,
                                                        reset_pane,
                                                        tako_core::PreviewViewUpdate {
                                                            zoom: Some(tako_core::PreviewZoomCommand::Reset),
                                                            ..tako_core::PreviewViewUpdate::default()
                                                        },
                                                    )
                                                    .is_ok()
                                                    {
                                                        cx.notify();
                                                    }
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id(("preview-zoom-in", pane_id.as_u64()))
                                                .p(px(3.0))
                                                .cursor_pointer()
                                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                                .child(
                                                    svg()
                                                        .path(crate::file_icons::ui_icon::PLUS)
                                                        .w(px(10.0))
                                                        .h(px(10.0))
                                                        .text_color(hsla(theme.text_secondary)),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let current = this
                                                        .preview_views
                                                        .get(&zoom_in_pane)
                                                        .copied()
                                                        .unwrap_or_default()
                                                        .zoom;
                                                    this.set_preview_zoom_about(
                                                        zoom_in_pane,
                                                        current * tako_core::PREVIEW_ZOOM_STEP,
                                                        None,
                                                        cx,
                                                    );
                                                })),
                                        ),
                                )
                            })
                            .when(phv.mode_toggle && md_capable && edit_snap.is_none(), |d| {
                                let (icon, label) = match mode {
                                    preview::PreviewMode::Markdown => (None, "コードとして表示"),
                                    preview::PreviewMode::Code => (
                                        Some(crate::file_icons::ui_icon::EYE),
                                        "md レンダリング表示",
                                    ),
                                    _ => (None, ""),
                                };
                                d.child(
                                    div()
                                        .id(("preview-mode-toggle", pane_id.as_u64()))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(theme.accent))
                                        .hover(|d| {
                                            d.bg(rgba_alpha(theme.tab_active_background, 0.8))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_preview_mode(pane_id, cx);
                                        }))
                                        .when(mode == preview::PreviewMode::Markdown, |d| {
                                            d.child(SharedString::from("</>"))
                                        })
                                        .children(icon.map(|p| {
                                            svg()
                                                .path(p)
                                                .w(px(13.0))
                                                .h(px(13.0))
                                                .text_color(hsla(theme.accent))
                                        }))
                                        .child(SharedString::from(label)),
                                )
                            })
                            .when(phv.edit_button && !editing, |d| {
                                d.child(
                                    div()
                                        .id(("preview-changelog-toggle", pane_id.as_u64()))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(if changelog_active {
                                            theme.green
                                        } else {
                                            theme.accent
                                        }))
                                        .hover(|d| {
                                            d.bg(rgba_alpha(theme.tab_active_background, 0.8))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_preview_changelog(pane_id, cx);
                                        }))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::GIT_BRANCH)
                                                .w(px(12.0))
                                                .h(px(12.0))
                                                .text_color(hsla(if changelog_active {
                                                    theme.green
                                                } else {
                                                    theme.accent
                                                })),
                                        )
                                        .child("履歴"),
                                )
                            })
                            .when(phv.edit_button && editable, |d| {
                                d.child(
                                    div()
                                        .id(("preview-edit-toggle", pane_id.as_u64()))
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(if editing {
                                            theme.green
                                        } else {
                                            theme.accent
                                        }))
                                        .hover(|d| {
                                            d.bg(rgba_alpha(theme.tab_active_background, 0.8))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Err(message) =
                                                this.set_preview_editing_local(pane_id, !editing)
                                            {
                                                if let Some(edit) =
                                                    this.preview_edits.get_mut(&pane_id)
                                                {
                                                    edit.message = Some(message);
                                                }
                                            }
                                            cx.notify();
                                        }))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::PENCIL)
                                                .w(px(12.0))
                                                .h(px(12.0))
                                                .text_color(hsla(if editing {
                                                    theme.green
                                                } else {
                                                    theme.accent
                                                })),
                                        )
                                        .child(if editing { "編集中" } else { "編集" }),
                                )
                            })
                            .when(phv.save_button && dirty && !autosave, |d| {
                                d.child(
                                    div()
                                        .id(("preview-save", pane_id.as_u64()))
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(theme.accent))
                                        .hover(|d| {
                                            d.bg(rgba_alpha(theme.tab_active_background, 0.8))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let _ = this.save_preview_local(pane_id);
                                            cx.notify();
                                        }))
                                        .child("保存 ⌘S"),
                                )
                            })
                            // 閉じるボタン（常に表示。右側に統一 #185）
                            .child(
                                div()
                                    .id(("pane-close", pane_id.as_u64()))
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba_alpha(theme.red, 0.25)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation()
                                        }),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_pane_button(pane_id, cx);
                                    }))
                                    .child(
                                        svg()
                                            .path(crate::file_icons::ui_icon::CLOSE)
                                            .w(px(13.0))
                                            .h(px(13.0))
                                            .text_color(hsla(theme.text_muted)),
                                    ),
                            ),
                    )
            })
            .children(navigation_panel.map(|panel| {
                let (label, rows): (&str, Vec<gpui::AnyElement>) = match panel {
                    PreviewNavigationPanel::Outline => {
                        let rows = preview_outline
                            .items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| {
                                let item_number = index + 1;
                                let indent = f32::from(item.level.saturating_sub(1)) * 12.0;
                                div()
                                    .id((
                                        "preview-outline-item",
                                        pane_id
                                            .as_u64()
                                            .wrapping_mul(10_000)
                                            .wrapping_add(item_number as u64),
                                    ))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .pl(px(8.0 + indent))
                                    .pr_2()
                                    .py(px(4.0))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.text_secondary))
                                    .hover(|d| {
                                        d.bg(rgba(theme.surface_hover))
                                            .text_color(hsla(theme.foreground))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation()
                                        }),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if tako_control::dispatch(
                                            this,
                                            tako_control::protocol::Request::PreviewOutline {
                                                pane: Some(pane_id.as_u64()),
                                                item: Some(item_number),
                                            },
                                            PaneOrigin::User,
                                        )
                                        .is_ok()
                                        {
                                            this.preview_navigation_panel = None;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        svg()
                                            .path(crate::file_icons::ui_icon::JUMP_ARROW)
                                            .w(px(9.0))
                                            .h(px(9.0))
                                            .flex_none()
                                            .text_color(hsla(theme.text_tertiary)),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(SharedString::from(item.title.clone())),
                                    )
                                    .into_any_element()
                            })
                            .collect();
                        ("アウトライン", rows)
                    }
                    PreviewNavigationPanel::Pages => {
                        let (current, total) = pdf_info.unwrap_or((1, 0));
                        let rows = (1..=total)
                            .map(|page| {
                                div()
                                    .id((
                                        "preview-page-item",
                                        pane_id
                                            .as_u64()
                                            .wrapping_mul(10_000)
                                            .wrapping_add(page as u64),
                                    ))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py(px(4.0))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .when(page == current, |d| {
                                        d.bg(rgba_alpha(theme.accent, 0.14))
                                            .text_color(hsla(theme.accent))
                                    })
                                    .when(page != current, |d| {
                                        d.text_color(hsla(theme.text_secondary))
                                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation()
                                        }),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if tako_control::dispatch(
                                            this,
                                            tako_control::protocol::Request::PreviewView {
                                                pane: Some(pane_id.as_u64()),
                                                zoom: None,
                                                zoom_in: false,
                                                zoom_out: false,
                                                reset: false,
                                                page: Some(page),
                                                pan_x: None,
                                                pan_y: None,
                                            },
                                            PaneOrigin::User,
                                        )
                                        .is_ok()
                                        {
                                            this.preview_navigation_panel = None;
                                        }
                                        cx.notify();
                                    }))
                                    .child(SharedString::from(format!("ページ {page}")))
                                    .into_any_element()
                            })
                            .collect();
                        ("ページへ移動", rows)
                    }
                };
                div()
                    .id(("preview-navigation-panel", pane_id.as_u64()))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .bg(rgba(theme.surface_1))
                    .border_b_1()
                    .border_color(hsla(theme.border_subtle))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .py(px(4.0))
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_tertiary))
                            .child(label)
                            .child(SharedString::from(format!("{} 件", rows.len()))),
                    )
                    .children(rows)
            }))
            .when(search_visible, |el| {
                let query_focused = search_focus == preview::SearchFieldFocus::Query;
                let replace_focused = search_focus == preview::SearchFieldFocus::Replace;
                let ime_text = edit_snap.as_ref().and_then(|s| s.ime_text.clone());
                let query_ime = if query_focused {
                    ime_text.clone()
                } else {
                    None
                };
                let replace_ime = if replace_focused { ime_text } else { None };
                let sq = search_query.clone();
                let sq2 = sq.clone();
                let rt = replace_text.clone();
                let sc = search_cursor;
                let rc = replace_cursor;
                let si = search_index;
                let st = search_total;
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .px_2()
                        .py(px(3.0))
                        .gap(px(2.0))
                        .bg(rgba_alpha(theme.tab_active_background, 0.9))
                        .text_size(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    svg()
                                        .path(crate::file_icons::ui_icon::SEARCH)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .text_color(hsla(theme.text_tertiary)),
                                )
                                .child(
                                    div()
                                        .id(("search-query-field", pane_id.as_u64()))
                                        .flex_1()
                                        .px_1()
                                        .py(px(1.0))
                                        .rounded_sm()
                                        .cursor(CursorStyle::IBeam)
                                        .bg(rgba_alpha(
                                            if query_focused {
                                                theme.accent
                                            } else {
                                                theme.tab_active_background
                                            },
                                            if query_focused { 0.2 } else { 0.5 },
                                        ))
                                        .border_1()
                                        .border_color(hsla_alpha(
                                            theme.accent,
                                            if query_focused { 0.6 } else { 0.15 },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                                if let Some(edit) =
                                                    this.preview_edits.get_mut(&pane_id)
                                                {
                                                    edit.search_focus =
                                                        preview::SearchFieldFocus::Query;
                                                }
                                                cx.stop_propagation();
                                                cx.notify();
                                            }),
                                        )
                                        .child(SharedString::from(render_field_with_cursor(
                                            &sq,
                                            sc,
                                            query_focused,
                                            query_ime.as_deref(),
                                        ))),
                                )
                                .when(st > 0, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(hsla_alpha(
                                                theme.tab_inactive_foreground,
                                                0.7,
                                            ))
                                            .child(SharedString::from(format!(
                                                "{}/{}",
                                                si + 1,
                                                st
                                            ))),
                                    )
                                })
                                .when(st == 0 && !sq2.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(hsla(theme.yellow))
                                            .child("0"),
                                    )
                                }),
                        )
                        .when(editing, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child("↔")
                                    .child(
                                        div()
                                            .id(("search-replace-field", pane_id.as_u64()))
                                            .flex_1()
                                            .px_1()
                                            .py(px(1.0))
                                            .rounded_sm()
                                            .cursor(CursorStyle::IBeam)
                                            .bg(rgba_alpha(
                                                if replace_focused {
                                                    theme.accent
                                                } else {
                                                    theme.tab_active_background
                                                },
                                                if replace_focused { 0.2 } else { 0.5 },
                                            ))
                                            .border_1()
                                            .border_color(hsla_alpha(
                                                theme.accent,
                                                if replace_focused { 0.6 } else { 0.15 },
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |this, _: &MouseDownEvent, _, cx| {
                                                        if let Some(edit) =
                                                            this.preview_edits.get_mut(&pane_id)
                                                        {
                                                            edit.search_focus =
                                                                preview::SearchFieldFocus::Replace;
                                                        }
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    },
                                                ),
                                            )
                                            .child(SharedString::from(render_field_with_cursor(
                                                &rt,
                                                rc,
                                                replace_focused,
                                                replace_ime.as_deref(),
                                            ))),
                                    ),
                            )
                        }),
                )
            })
            .child({
                // テキスト行を保存（選択テキスト抽出用）
                self.preview_line_texts.insert(pane_id, line_texts);
                // bounds 追跡用にリセット（各行の canvas で上書きされる）
                self.preview_line_bounds.insert(pane_id, Vec::new());
                self.preview_pdf_page_image_bounds
                    .insert(pane_id, HashMap::new());
                self.preview_text_layouts.insert(pane_id, line_layouts);
                let scroll_handle = self
                    .preview_scroll_handles
                    .entry(pane_id)
                    .or_default()
                    .clone();

                div()
                    .id(("preview-scroll", pane_id.as_u64()))
                    .flex_1()
                    .p(px(PANE_PADDING + 4.0))
                    .flex()
                    .flex_col()
                    .track_scroll(&scroll_handle)
                    .when(zoomable, |d| d.overflow_scroll())
                    .when(!zoomable, |d| d.overflow_y_scroll())
                    .cursor(
                        if self
                            .preview_pdf_hovered_link
                            .is_some_and(|(pid, _)| pid == pane_id)
                        {
                            CursorStyle::PointingHand
                        } else if mode == preview::PreviewMode::Image {
                            CursorStyle::Arrow
                        } else {
                            CursorStyle::IBeam
                        },
                    )
                    // 非ズーム対象へリスナー自体を登録すると、セルフテストの
                    // dispatch_event が root update 中に listener update を再入させる。
                    // Image / PDF にだけイベント経路を載せる。
                    .when(zoomable, |d| {
                        d.capture_pinch(cx.listener(
                            move |this, event: &gpui::PinchEvent, _, cx| {
                                // GPUI は Pinch 単体では keyboard modality を解除しないため、
                                // bubble の is_hovered 判定ではキーボード操作直後のピンチを
                                // 取りこぼす。capture で受け、対象ペインの実 bounds を自前判定する。
                                let in_pane = this
                                    .preview_scroll_handles
                                    .get(&pane_id)
                                    .is_some_and(|handle| {
                                        handle.bounds().contains(&event.position)
                                    });
                                if !in_pane {
                                    cx.propagate();
                                    return;
                                }
                                let current = this
                                    .preview_views
                                    .get(&pane_id)
                                .copied()
                                .unwrap_or_default()
                                .zoom;
                            let factor = (1.0 + event.delta).max(0.1);
                            this.set_preview_zoom_about(
                                pane_id,
                                current * factor,
                                Some(event.position),
                                cx,
                            );
                            cx.stop_propagation();
                        },
                        ))
                        .on_scroll_wheel(cx.listener(
                            move |this, event: &ScrollWheelEvent, _, cx| {
                                if event.modifiers.platform || event.modifiers.control {
                                let delta = match event.delta {
                                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                                    ScrollDelta::Lines(delta) => delta.y * 20.0,
                                };
                                let factor = if delta >= 0.0 {
                                    1.0 + delta.abs() * 0.01
                                } else {
                                    1.0 / (1.0 + delta.abs() * 0.01)
                                };
                                let current = this
                                    .preview_views
                                    .get(&pane_id)
                                    .copied()
                                    .unwrap_or_default()
                                    .zoom;
                                this.set_preview_zoom_about(
                                    pane_id,
                                    current * factor,
                                    Some(event.position),
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                            },
                        ))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            // ⌘クリック: PDF リンクを開く（#271）
                            if ev.modifiers.platform && ev.click_count == 1 {
                                if let Some(link_idx) = this.preview_pdf_hovered_link
                                    .filter(|(pid, _)| *pid == pane_id)
                                    .map(|(_, idx)| idx)
                                {
                                    this.follow_pdf_link(pane_id, link_idx, cx);
                                    this.preview_pdf_hovered_link = None;
                                    cx.notify();
                                    return;
                                }
                                if let Some(idx) = this.pdf_link_at_position(pane_id, ev.position) {
                                    this.follow_pdf_link(pane_id, idx, cx);
                                    cx.notify();
                                    return;
                                }
                            }
                            if let Some(pos) = this.preview_hit_test(pane_id, ev.position) {
                                this.preview_selections.insert(
                                    pane_id,
                                    PreviewSelection {
                                        anchor: pos,
                                        head: pos,
                                    },
                                );
                                this.preview_selecting = Some(pane_id);
                                this.sync_editor_selection_from_preview(pane_id);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseUpEvent, _, _cx| {
                            if this.preview_selecting == Some(pane_id) {
                                this.preview_selecting = None;
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if this.preview_selecting == Some(pane_id)
                            && ev.pressed_button == Some(MouseButton::Left)
                        {
                            if let Some(pos) = this.preview_hit_test(pane_id, ev.position) {
                                if let Some(sel) = this.preview_selections.get_mut(&pane_id) {
                                    sel.head = pos;
                                }
                                this.sync_editor_selection_from_preview(pane_id);
                                cx.notify();
                            }
                        }
                    }))
                    .children(body)
                    .children(truncated.then(|| {
                        div()
                            .pt_2()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .child("…（大きいファイルのため末尾を省略して表示）")
                    }))
            })
            .children((!pdf_highlight_bounds.is_empty()).then(|| {
                let entity = cx.entity().downgrade();
                let count = pdf_highlight_bounds.len();
                let color = hsla_alpha(theme.accent, 0.32);
                canvas(
                    |_, _, _| (),
                    move |overlay_bounds, _, window, cx| {
                        // ページ画像の子 canvas では画像 sprite に隠れる実機回帰があったため、
                        // ペインの最後の子かつ専用 stacking layer として最前面へ描く。
                        // 親と同じ layer では primitive 種別のバッチ順で Quad が PDF の
                        // PolychromeSprite より先に描かれ、発行済みでも完全に隠れる。
                        window.paint_layer(overlay_bounds, |window| {
                            for bounds in &pdf_highlight_bounds {
                                window.paint_quad(fill(*bounds, color));
                            }
                        });
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |app, _| {
                                app.preview_pdf_highlight_paint_count.insert(pane_id, count);
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            }))
    }

    /// Markdown インラインスパン列 → (テキスト, ハイライト範囲)
    fn preview_md_text(
        &self,
        spans: &[preview::MdSpan],
    ) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights = Vec::new();
        for span in spans {
            let start = text.len();
            text.push_str(&span.text);
            let styled = span.bold || span.italic || span.code || span.strike || span.link;
            if !styled {
                continue;
            }
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: if span.code {
                        Some(hsla(theme.yellow))
                    } else if span.link {
                        Some(hsla(theme.accent))
                    } else {
                        None
                    },
                    background_color: span.code.then(|| hsla(theme.tab_bar_background)),
                    font_weight: span.bold.then_some(FontWeight::BOLD),
                    font_style: span.italic.then_some(FontStyle::Italic),
                    underline: span.link.then(|| UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: span.strike.then_some(StrikethroughStyle {
                        thickness: px(1.0),
                        color: None,
                    }),
                    ..HighlightStyle::default()
                },
            ));
        }
        (text, highlights)
    }

    /// 選択ハイライト + 検索ヒットハイライト付きコード行。返した TextLayout は
    /// StyledText と共有され、ヒットテストとキャレット位置を実描画の shaping に一致させる。
    fn preview_code_line_sel(
        &self,
        line: &preview::Line,
        number: Option<(usize, usize)>,
        interaction: (Option<(usize, usize)>, Option<usize>),
        search_hit_ranges: &[(usize, usize, bool)],
        _cx: &mut Context<Self>,
    ) -> (gpui::Div, TextLayout) {
        let (sel_range, cursor_col) = interaction;
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
        for span in line {
            let start = text.len();
            text.push_str(&span.text);
            let style = HighlightStyle {
                color: span.color.map(hsla),
                font_weight: span.bold.then_some(FontWeight::BOLD),
                font_style: span.italic.then_some(FontStyle::Italic),
                ..HighlightStyle::default()
            };
            if span.color.is_some() || span.bold || span.italic {
                highlights.push((start..text.len(), style));
            }
        }
        if text.is_empty() {
            text.push(' ');
        }
        // 検索ヒットハイライト（選択より先に追加し、選択が上に重なるようにする）
        for &(start, end, is_current) in search_hit_ranges {
            let s = snap_to_char_boundary(&text, start.min(text.len()));
            let e = snap_to_char_boundary(&text, end.min(text.len()));
            if s < e {
                highlights.push((
                    s..e,
                    HighlightStyle {
                        background_color: Some(if is_current {
                            hsla_alpha(theme.yellow, 0.5)
                        } else {
                            hsla_alpha(theme.yellow, 0.2)
                        }),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
        // 選択ハイライト
        if let Some((start, end)) = sel_range {
            let s = snap_to_char_boundary(&text, start.min(text.len()));
            let e = snap_to_char_boundary(&text, end.min(text.len()));
            if s < e {
                highlights.push((
                    s..e,
                    HighlightStyle {
                        background_color: Some(hsla_alpha(theme.accent, 0.35)),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
        let highlights = merge_highlights(highlights);
        let caret_byte = cursor_col.map(|col| snap_to_char_boundary(&text, col.min(text.len())));
        let code_el = StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
        let text_layout = code_el.layout().clone();
        let caret_layout = text_layout.clone();
        let caret_color = hsla(theme.accent);
        let caret_canvas = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                if let Some(origin) =
                    caret_byte.and_then(|byte| caret_layout.position_for_index(byte))
                {
                    window.paint_quad(fill(
                        Bounds::new(origin, gpui::size(px(1.5), caret_layout.line_height())),
                        caret_color,
                    ));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let element =
            if let Some((n, width)) = number {
                let num_label = format!("{n:>width$}  ");
                let num_len = num_label.len();
                div()
                    .flex()
                    .flex_row()
                    .child(div().flex_none().child(
                        StyledText::new(num_label).with_default_highlights(
                            &self.text_style(),
                            vec![(
                                0..num_len,
                                HighlightStyle {
                                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                                    ..HighlightStyle::default()
                                },
                            )],
                        ),
                    ))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(code_el)
                            .child(caret_canvas),
                    )
            } else {
                div().relative().child(code_el).child(caret_canvas)
            };
        (element, text_layout)
    }

    /// 選択ハイライト + 検索ヒットハイライト付き Markdown ブロック + 実描画 TextLayout。
    fn preview_md_block_sel(
        &self,
        block: &preview::MdBlock,
        sel_range: Option<(usize, usize)>,
        search_hit_ranges: &[(usize, usize, bool)],
    ) -> (gpui::AnyElement, Option<TextLayout>) {
        let theme = self.theme.clone();

        let add_search_and_sel =
            |highlights: &mut Vec<(std::ops::Range<usize>, HighlightStyle)>, text: &str| {
                for &(start, end, is_current) in search_hit_ranges {
                    let s = snap_to_char_boundary(text, start.min(text.len()));
                    let e = snap_to_char_boundary(text, end.min(text.len()));
                    if s < e {
                        highlights.push((
                            s..e,
                            HighlightStyle {
                                background_color: Some(if is_current {
                                    hsla_alpha(theme.yellow, 0.5)
                                } else {
                                    hsla_alpha(theme.yellow, 0.2)
                                }),
                                ..HighlightStyle::default()
                            },
                        ));
                    }
                }
                if let Some((start, end)) = sel_range {
                    let s = snap_to_char_boundary(text, start.min(text.len()));
                    let e = snap_to_char_boundary(text, end.min(text.len()));
                    if s < e {
                        highlights.push((
                            s..e,
                            HighlightStyle {
                                background_color: Some(hsla_alpha(theme.accent, 0.35)),
                                ..HighlightStyle::default()
                            },
                        ));
                    }
                }
            };

        match block {
            preview::MdBlock::Heading { level, spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_search_and_sel(&mut highlights, &text);
                let highlights = merge_highlights(highlights);
                let size = match level {
                    1 => 19.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                let styled =
                    StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
                let layout = styled.layout().clone();
                let element = div()
                    .relative()
                    .pt_2()
                    .pb_1()
                    .text_size(px(size))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(theme.foreground))
                    .when(*level <= 2, |d| {
                        d.border_b_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.8))
                    })
                    .child(styled)
                    .into_any_element();
                (element, Some(layout))
            }
            preview::MdBlock::Paragraph { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_search_and_sel(&mut highlights, &text);
                let highlights = merge_highlights(highlights);
                let styled =
                    StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
                let layout = styled.layout().clone();
                let element = div().relative().py_1().child(styled).into_any_element();
                (element, Some(layout))
            }
            preview::MdBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_search_and_sel(&mut highlights, &text);
                let highlights = merge_highlights(highlights);
                let styled =
                    StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
                let layout = styled.layout().clone();
                let element = div()
                    .relative()
                    .flex()
                    .flex_row()
                    .pl(px(8.0 + 16.0 * *depth as f32))
                    .gap_1()
                    .child(
                        div()
                            .flex_none()
                            .text_color(hsla_alpha(theme.foreground, 0.7))
                            .child(SharedString::from(marker.clone())),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(styled))
                    .into_any_element();
                (element, Some(layout))
            }
            preview::MdBlock::CodeBlock { lines } => {
                let mut text = String::new();
                let mut highlights = Vec::new();
                for (line_i, line) in lines.iter().enumerate() {
                    if line_i > 0 {
                        text.push('\n');
                    }
                    for span in line {
                        let start = text.len();
                        text.push_str(&span.text);
                        if span.color.is_some() || span.bold || span.italic {
                            highlights.push((
                                start..text.len(),
                                HighlightStyle {
                                    color: span.color.map(hsla),
                                    font_weight: span.bold.then_some(FontWeight::BOLD),
                                    font_style: span.italic.then_some(FontStyle::Italic),
                                    ..HighlightStyle::default()
                                },
                            ));
                        }
                    }
                }
                add_search_and_sel(&mut highlights, &text);
                if text.is_empty() {
                    text.push(' ');
                }
                let styled = StyledText::new(text)
                    .with_default_highlights(&self.text_style(), merge_highlights(highlights));
                let layout = styled.layout().clone();
                let element = div()
                    .relative()
                    .my_1()
                    .p_2()
                    .rounded_md()
                    .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                    .child(styled)
                    .into_any_element();
                (element, Some(layout))
            }
            preview::MdBlock::Quote { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_search_and_sel(&mut highlights, &text);
                let highlights = merge_highlights(highlights);
                let styled =
                    StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
                let layout = styled.layout().clone();
                let element = div()
                    .relative()
                    .my_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(hsla_alpha(theme.accent, 0.6))
                    .text_color(hsla_alpha(theme.foreground, 0.75))
                    .child(styled)
                    .into_any_element();
                (element, Some(layout))
            }
            preview::MdBlock::Rule => (
                div()
                    .relative()
                    .my_2()
                    .h(px(1.0))
                    .bg(hsla_alpha(theme.pane_border, 0.9))
                    .into_any_element(),
                None,
            ),
        }
    }

    /// チェンジログビューのトグル（Issue #338）
    pub(crate) fn toggle_preview_changelog(
        &mut self,
        pane_id: PaneId,
        cx: &mut gpui::Context<Self>,
    ) {
        let currently_on = self.preview_changelogs.contains_key(&pane_id);
        if currently_on {
            self.preview_changelogs.remove(&pane_id);
        } else {
            let state = match self.previews.get(&pane_id) {
                Some(s) => s,
                None => return,
            };
            let path = state.path.clone();
            let repo = tako_core::git::repo_root(&path);
            match repo {
                Some(repo) => {
                    let rel = path
                        .strip_prefix(&repo)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| path.to_string_lossy().to_string());
                    let commits = tako_core::git::log_file_commits(&repo, &rel, 50);
                    let entries = commits
                        .into_iter()
                        .map(|c| preview::ChangelogEntry {
                            commit: c,
                            expanded_diff: None,
                        })
                        .collect();
                    self.preview_changelogs.insert(
                        pane_id,
                        preview::ChangelogData {
                            entries,
                            repo_root: Some(repo),
                            rel_path: Some(rel),
                        },
                    );
                }
                None => {
                    self.preview_changelogs
                        .insert(pane_id, preview::ChangelogData::default());
                }
            }
        }
        cx.notify();
    }

    /// チェンジログビューの本文描画（Issue #338）
    fn render_changelog_body(
        &self,
        pane_id: PaneId,
        data: &preview::ChangelogData,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if data.entries.is_empty() {
            return vec![div()
                .p_4()
                .text_color(hsla_alpha(theme.foreground, 0.5))
                .child(SharedString::from(if data.repo_root.is_none() {
                    "git 管理外のファイルです"
                } else {
                    "このファイルの変更履歴はありません"
                }))
                .into_any_element()];
        }
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        for (idx, entry) in data.entries.iter().enumerate() {
            let c = &entry.commit;
            let is_expanded = entry.expanded_diff.is_some();
            let hash = c.short_hash.clone();
            let commit_hash_full = c.hash.clone();
            elements.push(
                div()
                    .id(("changelog-entry", (pane_id.as_u64() * 10000 + idx as u64)))
                    .flex()
                    .flex_col()
                    .w_full()
                    .border_b_1()
                    .border_color(hsla_alpha(theme.border_default, 0.3))
                    .child(
                        div()
                            .id(("cl-row", (pane_id.as_u64() * 10000 + idx as u64)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba_alpha(theme.surface_hover, 0.5)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_changelog_diff(pane_id, &commit_hash_full);
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(SharedString::from(if is_expanded { "v" } else { ">" }))
                                    .child(
                                        div()
                                            .text_color(hsla(theme.accent))
                                            .child(SharedString::from(hash)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_x_hidden()
                                    .child(SharedString::from(c.subject.clone())),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(hsla_alpha(theme.foreground, 0.5))
                                    .text_xs()
                                    .child(SharedString::from(format!(
                                        "{} / {}",
                                        c.author, c.date_relative
                                    ))),
                            ),
                    )
                    .when(is_expanded, |d| {
                        let hunks = entry.expanded_diff.as_deref().unwrap_or(&[]);
                        d.child(self.render_changelog_diff(hunks, theme))
                    })
                    .into_any_element(),
            );
        }
        elements
    }

    /// チェンジログの diff 展開描画
    fn render_changelog_diff(
        &self,
        hunks: &[tako_core::DiffHunk],
        theme: &Theme,
    ) -> gpui::AnyElement {
        if hunks.is_empty() {
            return div()
                .px_4()
                .py_1()
                .text_color(hsla_alpha(theme.foreground, 0.5))
                .text_xs()
                .child(SharedString::from("(変更なし)"))
                .into_any_element();
        }
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        for hunk in hunks {
            elements.push(
                div()
                    .px_4()
                    .py(px(1.0))
                    .text_color(hsla_alpha(theme.accent, 0.7))
                    .text_xs()
                    .child(SharedString::from(hunk.header.clone()))
                    .into_any_element(),
            );
            for line in &hunk.lines {
                let (bg, color, prefix) = match line.kind {
                    tako_core::DiffLineKind::Add => {
                        (rgba_alpha(theme.green, 0.15), hsla(theme.green), "+")
                    }
                    tako_core::DiffLineKind::Remove => {
                        (rgba_alpha(theme.red, 0.15), hsla(theme.red), "-")
                    }
                    tako_core::DiffLineKind::Context => (
                        rgba_alpha(theme.background, 0.0),
                        hsla_alpha(theme.foreground, 0.6),
                        " ",
                    ),
                };
                elements.push(
                    div()
                        .px_4()
                        .bg(bg)
                        .text_color(color)
                        .text_xs()
                        .child(SharedString::from(format!("{}{}", prefix, line.content)))
                        .into_any_element(),
                );
            }
        }
        div()
            .w_full()
            .bg(rgba_alpha(theme.surface_1, 0.5))
            .pb_1()
            .children(elements)
            .into_any_element()
    }

    /// toggle_changelog_diff のローカル版（UI から直接呼ぶ）
    fn toggle_changelog_diff(&mut self, pane_id: PaneId, hash: &str) {
        let data = match self.preview_changelogs.get_mut(&pane_id) {
            Some(d) => d,
            None => return,
        };
        let entry = match data
            .entries
            .iter_mut()
            .find(|e| e.commit.hash == hash || e.commit.short_hash == hash)
        {
            Some(e) => e,
            None => return,
        };
        if entry.expanded_diff.is_some() {
            entry.expanded_diff = None;
        } else {
            if let (Some(repo), Some(rel)) = (data.repo_root.as_deref(), data.rel_path.as_deref()) {
                let hunks = tako_core::git::diff_file_commit(repo, &entry.commit.hash, rel);
                entry.expanded_diff = Some(hunks);
            }
        }
    }
}

/// 検索/置換フィールドのテキストにカーソル（|）と IME 未確定テキストを差し込んで
/// 表示用文字列を作る。`ime_text` が Some の場合はカーソル位置に [未確定] を挿入する。
fn render_field_with_cursor(
    text: &str,
    cursor: usize,
    focused: bool,
    ime_text: Option<&str>,
) -> String {
    if !focused {
        if text.is_empty() {
            return " ".into();
        }
        return text.to_string();
    }
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let after = &text[cursor..];
    if let Some(ime) = ime_text.filter(|t| !t.is_empty()) {
        return format!("{before}[{ime}]{after}");
    }
    if text.is_empty() {
        "|".to_string()
    } else {
        format!("{before}|{after}")
    }
}

/// 検索ヒットのうち行に重なる部分を行内バイト範囲のリストとして返す。
/// `is_current` が true のヒットは `(start, end, true)` で区別する。
/// `line_start` / `line_end` は文書全体のバイト位置。
fn search_hits_for_line(
    hits: &[tako_core::SearchHit],
    current_index: usize,
    line_start: usize,
    line_end: usize,
) -> Vec<(usize, usize, bool)> {
    let mut result = Vec::new();
    for (i, hit) in hits.iter().enumerate() {
        if hit.end <= line_start || hit.start >= line_end {
            continue;
        }
        let s = hit.start.max(line_start) - line_start;
        let e = hit.end.min(line_end) - line_start;
        result.push((s, e, i == current_index));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_field_cursor_and_ime() {
        assert_eq!(render_field_with_cursor("abc", 1, true, None), "a|bc");
        assert_eq!(render_field_with_cursor("abc", 0, true, None), "|abc");
        assert_eq!(render_field_with_cursor("", 0, true, None), "|");
        assert_eq!(render_field_with_cursor("abc", 0, false, None), "abc");
        assert_eq!(render_field_with_cursor("", 0, false, None), " ");
        assert_eq!(
            render_field_with_cursor("ab", 1, true, Some("変換")),
            "a[変換]b"
        );
        assert_eq!(render_field_with_cursor("ab", 1, true, Some("")), "a|b");
    }

    #[test]
    fn search_hits_line_intersection() {
        use tako_core::SearchHit;
        let hits = vec![
            SearchHit { start: 2, end: 5 },
            SearchHit { start: 8, end: 11 },
            SearchHit { start: 14, end: 17 },
        ];
        let r = search_hits_for_line(&hits, 1, 0, 6);
        assert_eq!(r, vec![(2, 5, false)]);
        let r = search_hits_for_line(&hits, 1, 7, 13);
        assert_eq!(r, vec![(1, 4, true)]);
        let r = search_hits_for_line(&hits, 0, 20, 30);
        assert!(r.is_empty());
        let r = search_hits_for_line(&hits, 0, 4, 9);
        assert_eq!(r, vec![(0, 1, true), (4, 5, false)]);
    }
}

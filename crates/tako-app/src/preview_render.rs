use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, Bounds, BoxShadow, Context, CursorStyle,
    FontWeight, HighlightStyle, MouseButton, MouseMoveEvent, SharedString, StyledText,
    UnderlineStyle, Window,
};
use tako_core::{PaneId, Rect};

use super::*;

impl TakoApp {
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
            return format!("📄 {}", preview.file_name());
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
    pub(crate) fn render_preview_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let state = self.previews.get(&pane_id).expect("呼び出し前に確認済み");
        let file_name = state.file_name();
        let path_label = state.path.display().to_string();
        let md_capable = state.markdown_capable();
        let mode = state.mode;
        let truncated = state.truncated;
        let edit_info = self.preview_edits.get(&pane_id).map(|edit| {
            (
                edit.editing,
                edit.dirty(),
                edit.message.clone(),
                edit.buffer.line_byte_col(edit.buffer.cursor()),
            )
        });
        let editing = edit_info.as_ref().is_some_and(|info| info.0);
        let dirty = edit_info.as_ref().is_some_and(|info| info.1);
        let edit_message = edit_info.as_ref().and_then(|info| info.2.clone());
        let edit_cursor = edit_info.as_ref().filter(|info| info.0).map(|info| info.3);
        let editable = matches!(
            &state.content,
            preview::PreviewContent::Code(_) | preview::PreviewContent::Markdown(_)
        ) && !truncated;

        let pdf_info: Option<usize> = if let preview::PreviewContent::Pdf(data) = &state.content {
            Some(data.total_pages)
        } else {
            None
        };

        // 選択状態
        let selection = self.preview_selections.get(&pane_id).cloned();

        // テキスト行を収集（選択テキスト抽出 + bounds 追跡用）
        let mut line_texts: Vec<String> = Vec::new();
        // Code / Markdown は StyledText 自身の TextLayout を保持し、ヒットテストと
        // キャレット描画を実際の shaping 結果に一致させる。
        let mut line_layouts: Vec<Option<TextLayout>> = Vec::new();

        // 本文要素を先に組む（state の借用をここで終える）
        let body: Vec<gpui::AnyElement> = match &state.content {
            preview::PreviewContent::Code(lines) => {
                let number_width = lines.len().to_string().len();
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        let text: String = line.iter().map(|s| s.text.as_str()).collect();
                        let sel_range = selection
                            .as_ref()
                            .and_then(|s| s.range_for_line(i, text.len()));
                        line_texts.push(text);
                        let cursor_col = edit_cursor
                            .filter(|(line, _)| *line == i)
                            .map(|(_, col)| col);
                        let (element, layout) = self.preview_code_line_sel(
                            line,
                            Some((i + 1, number_width)),
                            (sel_range, cursor_col),
                            cx,
                        );
                        line_layouts.push(Some(layout));
                        element.into_any_element()
                    })
                    .collect()
            }
            preview::PreviewContent::Markdown(blocks) => blocks
                .iter()
                .enumerate()
                .map(|(i, block)| {
                    let text = md_block_plain_text(block);
                    let sel_range = selection
                        .as_ref()
                        .and_then(|s| s.range_for_line(i, text.len()));
                    line_texts.push(text);
                    let (element, layout) = self.preview_md_block_sel(block, sel_range);
                    line_layouts.push(layout);
                    element
                })
                .collect(),
            preview::PreviewContent::Image(data) => {
                let gpui_format = match data.format {
                    preview::ImageFileFormat::Png => gpui::ImageFormat::Png,
                    preview::ImageFileFormat::Jpeg => gpui::ImageFormat::Jpeg,
                    preview::ImageFileFormat::Gif => gpui::ImageFormat::Gif,
                    preview::ImageFileFormat::WebP => gpui::ImageFormat::Webp,
                    preview::ImageFileFormat::Svg => gpui::ImageFormat::Svg,
                };
                let image =
                    std::sync::Arc::new(gpui::Image::from_bytes(gpui_format, data.bytes.clone()));
                vec![div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui::img(image)
                            .object_fit(gpui::ObjectFit::Contain)
                            .max_w_full()
                            .max_h_full(),
                    )
                    .into_any_element()]
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
                data.pages
                    .iter()
                    .enumerate()
                    .filter(|(_, png)| !png.is_empty())
                    .map(|(i, png)| {
                        let image = std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            png.clone(),
                        ));
                        let page_text_lines = data.text_layers.get(i);
                        let page_size = data.page_sizes.get(i).copied().unwrap_or([612.0, 792.0]);
                        let page_line_offset = line_offset;
                        let n_lines = page_text_lines.map(|l| l.len()).unwrap_or(0);
                        line_offset += n_lines;

                        let sel_highlight = selection.as_ref().and_then(|sel| {
                            let (start, end) = sel.ordered();
                            if end.0 < page_line_offset || start.0 >= page_line_offset + n_lines {
                                return None;
                            }
                            Some((sel.clone(), page_line_offset))
                        });

                        let entity = cx.entity().downgrade();
                        let text_lines_for_canvas: Vec<preview::PdfTextLine> =
                            page_text_lines.map(|l| l.to_vec()).unwrap_or_default();
                        let pdf_w = page_size[0];
                        let pdf_h = page_size[1];
                        let sel_color = theme.selection_background;

                        let overlay = canvas(
                            |_, _, _| (),
                            move |bounds, _, window, cx| {
                                if text_lines_for_canvas.is_empty() {
                                    return;
                                }
                                let img_w = f32::from(bounds.size.width) as f64;
                                let img_h = f32::from(bounds.size.height) as f64;
                                if img_w <= 0.0 || img_h <= 0.0 {
                                    return;
                                }
                                let scale_x = img_w / pdf_w;
                                let scale_y = img_h / pdf_h;
                                let mut page_char_bounds: Vec<Vec<Bounds<Pixels>>> =
                                    Vec::with_capacity(text_lines_for_canvas.len());

                                // bounds 追跡: 各行の画面座標を preview_line_bounds に登録
                                if let Some(e) = entity.upgrade() {
                                    e.update(cx, |app, _| {
                                        let list =
                                            app.preview_line_bounds.entry(pane_id).or_default();
                                        for (j, tl) in text_lines_for_canvas.iter().enumerate() {
                                            let idx = page_line_offset + j;
                                            if list.len() <= idx {
                                                list.resize(idx + 1, Bounds::default());
                                            }
                                            // PDF 座標（左下原点）→ スクリーン座標（左上原点）
                                            let sx = bounds.origin.x
                                                + px(tl.bbox[0] as f32 * scale_x as f32);
                                            let sy = bounds.origin.y
                                                + px((pdf_h - tl.bbox[1] - tl.bbox[3]) as f32
                                                    * scale_y as f32);
                                            let sw = px(tl.bbox[2] as f32 * scale_x as f32);
                                            let sh = px(tl.bbox[3] as f32 * scale_y as f32);
                                            list[idx] = Bounds {
                                                origin: point(sx, sy),
                                                size: gpui::size(sw, sh),
                                            };
                                        }
                                    });
                                }

                                // 選択ハイライトの描画
                                if let Some((ref sel, offset)) = sel_highlight {
                                    for (j, tl) in text_lines_for_canvas.iter().enumerate() {
                                        let mut line_char_bounds =
                                            Vec::with_capacity(tl.char_boxes.len());
                                        for ch in &tl.char_boxes {
                                            let sx = bounds.origin.x
                                                + px(ch.bbox[0] as f32 * scale_x as f32);
                                            let sy = bounds.origin.y
                                                + px((pdf_h - ch.bbox[1] - ch.bbox[3]) as f32
                                                    * scale_y as f32);
                                            let sw = px(ch.bbox[2] as f32 * scale_x as f32);
                                            let sh = px(ch.bbox[3] as f32 * scale_y as f32);
                                            line_char_bounds.push(Bounds {
                                                origin: point(sx, sy),
                                                size: gpui::size(sw, sh),
                                            });
                                        }
                                        let global_j = offset + j;
                                        let line_range =
                                            sel.range_for_line(global_j, tl.text.len());
                                        if let Some((sc, ec)) = line_range {
                                            if !tl.text.is_empty() && tl.bbox[2] > 0.0 {
                                                for (ch, ch_bounds) in tl
                                                    .char_boxes
                                                    .iter()
                                                    .zip(line_char_bounds.iter())
                                                {
                                                    if ch.byte_range.end > sc
                                                        && ch.byte_range.start < ec
                                                        && f32::from(ch_bounds.size.width) > 0.0
                                                        && f32::from(ch_bounds.size.height) > 0.0
                                                    {
                                                        window.paint_quad(fill(
                                                            *ch_bounds,
                                                            hsla_alpha(sel_color, 0.35),
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        page_char_bounds.push(line_char_bounds);
                                    }
                                } else {
                                    for tl in &text_lines_for_canvas {
                                        let mut line_char_bounds =
                                            Vec::with_capacity(tl.char_boxes.len());
                                        for ch in &tl.char_boxes {
                                            let sx = bounds.origin.x
                                                + px(ch.bbox[0] as f32 * scale_x as f32);
                                            let sy = bounds.origin.y
                                                + px((pdf_h - ch.bbox[1] - ch.bbox[3]) as f32
                                                    * scale_y as f32);
                                            let sw = px(ch.bbox[2] as f32 * scale_x as f32);
                                            let sh = px(ch.bbox[3] as f32 * scale_y as f32);
                                            line_char_bounds.push(Bounds {
                                                origin: point(sx, sy),
                                                size: gpui::size(sw, sh),
                                            });
                                        }
                                        page_char_bounds.push(line_char_bounds);
                                    }
                                }

                                if let Some(e) = entity.upgrade() {
                                    e.update(cx, |app, _| {
                                        let list =
                                            app.preview_pdf_char_bounds.entry(pane_id).or_default();
                                        for (j, line_bounds) in page_char_bounds.iter().enumerate()
                                        {
                                            let idx = page_line_offset + j;
                                            if list.len() <= idx {
                                                list.resize(idx + 1, Vec::new());
                                            }
                                            list[idx] = line_bounds.clone();
                                        }
                                    });
                                }
                            },
                        )
                        .absolute()
                        .size_full();

                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .w_full()
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
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .child(
                                        gpui::img(image)
                                            .object_fit(gpui::ObjectFit::Contain)
                                            .max_w_full(),
                                    )
                                    .child(overlay),
                            )
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
                            let render = std::sync::Arc::new(gpui::RenderImage::new(vec![frame]));
                            self.video_frame_cache.insert(pane_id, (gen, render));
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

                    // シークバー（クリック + ドラッグ対応 + つまみノブ）
                    let seek_bar = div()
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
                        }));

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
                                        d.bg(hsla(theme.ansi[4])).text_color(hsla(theme.background))
                                    })
                                    .when(!is_active, |d| {
                                        d.text_color(hsla_alpha(theme.foreground, 0.6))
                                            .hover(|s| s.bg(hsla_alpha(theme.foreground, 0.1)))
                                    })
                                    .child(SharedString::from(label))
                                    .on_click(cx.listener(
                                        move |this, _ev: &gpui::ClickEvent, _, cx| {
                                            if let Some(p) = this.video_players.get_mut(&pane_id) {
                                                p.set_rate(rate);
                                                cx.notify();
                                            }
                                        },
                                    ))
                                    .into_any_element()
                            }));

                    // コントロールバー: 再生/一時停止 + シークバー + 時間 + 速度
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
                                            if let Some(p) = this.video_players.get_mut(&pane_id) {
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
                            .into_any_element(),
                    );
                } else {
                    // プレイヤー未起動: ffmpeg サムネイル + 再生ボタン + メタ情報
                    if !data.thumbnail.is_empty() {
                        let image = std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            data.thumbnail.clone(),
                        ));
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
            preview::PreviewContent::Error(message) => vec![div()
                .p_2()
                .text_color(hsla(theme.red))
                .child(SharedString::from(message.clone()))
                .into_any_element()],
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
            .child(
                // タイトルバー: × / 📄 ファイル名 / （md のみ）モードトグル
                div()
                    .id(("preview-titlebar", pane_id.as_u64()))
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
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
                        self.drag_ghost_builder(
                            DragKind::Pane,
                            format!("📄 {}", truncate(&file_name, 24)),
                            cx,
                        ),
                    )
                    .child(
                        div()
                            .id(("pane-close", pane_id.as_u64()))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .hover(|d| {
                                d.bg(rgba_alpha(theme.red, 0.25))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_pane_button(pane_id, cx);
                            }))
                            .child("×"),
                    )
                    .child(
                        div()
                            .text_color(if focused {
                                hsla(theme.foreground)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from({
                                let icon = match mode {
                                    preview::PreviewMode::Image => "🖼",
                                    preview::PreviewMode::Pdf => "📕",
                                    _ => "📄",
                                };
                                format!(
                                    "{icon} {}{}",
                                    truncate(&file_name, 36),
                                    if dirty { " ●" } else { "" }
                                )
                            })),
                    )
                    .child(div().flex_grow(1.0))
                    .children((md_capable && edit_info.is_none()).then(|| {
                        // 目アイコンのトグル（FR-3.3）: コード表示 ⇔ md レンダリング
                        let (icon, label) = match mode {
                            preview::PreviewMode::Markdown => ("</>", "コードとして表示"),
                            preview::PreviewMode::Code => ("👁", "md レンダリング表示"),
                            _ => ("", ""),
                        };
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
                            .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_preview_mode(pane_id, cx);
                            }))
                            .child(SharedString::from(format!("{icon} {label}")))
                    }))
                    .children(editable.then(|| {
                        div()
                            .id(("preview-edit-toggle", pane_id.as_u64()))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla(if editing { theme.green } else { theme.accent }))
                            .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Err(message) =
                                    this.set_preview_editing_local(pane_id, !editing)
                                {
                                    if let Some(edit) = this.preview_edits.get_mut(&pane_id) {
                                        edit.message = Some(message);
                                    }
                                }
                                cx.notify();
                            }))
                            .child(if editing {
                                "✓ 編集中"
                            } else {
                                "✎ 編集"
                            })
                    }))
                    .children(dirty.then(|| {
                        div()
                            .id(("preview-save", pane_id.as_u64()))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla(theme.accent))
                            .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let _ = this.save_preview_local(pane_id);
                                cx.notify();
                            }))
                            .child("保存 ⌘S")
                    }))
                    .children(pdf_info.map(|total| {
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .child(SharedString::from(format!("{} ページ", total)))
                    }))
                    .children(edit_message.map(|message| {
                        div()
                            .text_size(px(10.0))
                            .text_color(hsla(if dirty { theme.yellow } else { theme.green }))
                            .child(SharedString::from(truncate(&message, 36)))
                    }))
                    .child(
                        div()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .text_size(px(10.0))
                            .child(SharedString::from(truncate(&path_label, 40))),
                    ),
            )
            .child({
                // テキスト行を保存（選択テキスト抽出用）
                self.preview_line_texts.insert(pane_id, line_texts);
                // bounds 追跡用にリセット（各行の canvas で上書きされる）
                self.preview_line_bounds.insert(pane_id, Vec::new());
                self.preview_text_layouts.insert(pane_id, line_layouts);

                div()
                    .id(("preview-scroll", pane_id.as_u64()))
                    .flex_1()
                    .p(px(PANE_PADDING + 4.0))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
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

    /// 選択ハイライト付きコード行。返した TextLayout は StyledText と共有され、
    /// ヒットテストとキャレット位置を実描画の shaping に一致させる。
    fn preview_code_line_sel(
        &self,
        line: &preview::Line,
        number: Option<(usize, usize)>,
        interaction: (Option<(usize, usize)>, Option<usize>),
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

    /// 選択ハイライト付き Markdown ブロック + 実描画 TextLayout。
    fn preview_md_block_sel(
        &self,
        block: &preview::MdBlock,
        sel_range: Option<(usize, usize)>,
    ) -> (gpui::AnyElement, Option<TextLayout>) {
        let theme = self.theme.clone();

        let add_sel = |highlights: &mut Vec<(std::ops::Range<usize>, HighlightStyle)>,
                       text: &str| {
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
                add_sel(&mut highlights, &text);
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
                add_sel(&mut highlights, &text);
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
                add_sel(&mut highlights, &text);
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
                add_sel(&mut highlights, &text);
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
                add_sel(&mut highlights, &text);
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
}

use gpui::{
    div, point, prelude::*, px, BoxShadow, Context, CursorStyle, FontWeight, HighlightStyle,
    MouseButton, SharedString, StyledText, UnderlineStyle, Window,
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

        let pdf_info: Option<usize> = if let preview::PreviewContent::Pdf(data) = &state.content {
            Some(data.total_pages)
        } else {
            None
        };

        // 選択状態
        let selection = self.preview_selections.get(&pane_id).cloned();

        // テキスト行を収集（選択テキスト抽出 + bounds 追跡用）
        let mut line_texts: Vec<String> = Vec::new();

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
                        self.preview_code_line_sel(
                            line,
                            Some((i + 1, number_width)),
                            sel_range,
                            pane_id,
                            i,
                            cx,
                        )
                        .into_any_element()
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
                    self.preview_md_block_sel(block, sel_range, pane_id, i, cx)
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
            preview::PreviewContent::Pdf(data) => data
                .pages
                .iter()
                .enumerate()
                .filter(|(_, png)| !png.is_empty())
                .map(|(i, png)| {
                    let image = std::sync::Arc::new(gpui::Image::from_bytes(
                        gpui::ImageFormat::Png,
                        png.clone(),
                    ));
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
                            gpui::img(image)
                                .object_fit(gpui::ObjectFit::Contain)
                                .max_w_full(),
                        )
                        .into_any_element()
                })
                .collect(),
            preview::PreviewContent::Video(data) => {
                let has_player = self.video_players.contains_key(&pane_id);
                let mut elements: Vec<gpui::AnyElement> = Vec::new();

                if has_player {
                    // AVFoundation プレイヤー起動中: キャッシュ済みフレームを表示
                    let player = self.video_players.get(&pane_id).unwrap();
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

                    // コントロールバー: 再生/一時停止 + シークバー + 時間表示
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
                            .child(
                                div()
                                    .id(("video-seek", pane_id.as_u64()))
                                    .relative()
                                    .flex_1()
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(hsla_alpha(theme.foreground, 0.2))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(3.0))
                                            .bg(hsla(theme.ansi[4]))
                                            .w(relative(progress_frac)),
                                    )
                                    .child({
                                        let entity = cx.entity().downgrade();
                                        canvas(
                                            |_, _, _| (),
                                            move |bounds, _, _, cx| {
                                                if let Some(e) = entity.upgrade() {
                                                    e.update(cx, |app, _| {
                                                        app.video_seek_bar_bounds
                                                            .insert(pane_id, bounds);
                                                    });
                                                }
                                            },
                                        )
                                        .absolute()
                                        .size_full()
                                    })
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(
                                            move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                this.video_seek_by_click(
                                                    pane_id,
                                                    ev.position,
                                                    seek_dur,
                                                    cx,
                                                );
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(hsla_alpha(theme.foreground, 0.7))
                                    .child(time_label),
                            )
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
                                format!("{icon} {}", truncate(&file_name, 36))
                            })),
                    )
                    .child(div().flex_grow(1.0))
                    .children(md_capable.then(|| {
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
                    .children(pdf_info.map(|total| {
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .child(SharedString::from(format!("{} ページ", total)))
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
                                    cx.notify();
                                }
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

    /// ハイライト済みコード 1 行（行番号は固定幅左列、本文は残り幅で折り返す）
    fn preview_code_line(&self, line: &preview::Line, number: Option<(usize, usize)>) -> gpui::Div {
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
        let code_el = StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
        if let Some((n, width)) = number {
            let num_label = format!("{n:>width$}  ");
            let num_len = num_label.len();
            div()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .child(StyledText::new(num_label).with_default_highlights(
                            &self.text_style(),
                            vec![(
                                0..num_len,
                                HighlightStyle {
                                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                                    ..HighlightStyle::default()
                                },
                            )],
                        )),
                )
                .child(div().flex_1().min_w(px(0.0)).child(code_el))
        } else {
            div().child(code_el)
        }
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

    /// Markdown ブロック 1 つの描画（FR-3.3。選択なし版は preview_md_block_sel に統合済み）
    #[allow(dead_code)]
    fn preview_md_block(&self, block: &preview::MdBlock) -> gpui::AnyElement {
        let theme = self.theme.clone();
        match block {
            preview::MdBlock::Heading { level, spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                let size = match level {
                    1 => 19.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                div()
                    .pt_2()
                    .pb_1()
                    .text_size(px(size))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(theme.foreground))
                    .when(*level <= 2, |d| {
                        d.border_b_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.8))
                    })
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::Paragraph { spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
                    .py_1()
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
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
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            StyledText::new(text)
                                .with_default_highlights(&self.text_style(), highlights),
                        ),
                    )
                    .into_any_element()
            }
            preview::MdBlock::CodeBlock { lines } => div()
                .my_1()
                .p_2()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                .flex()
                .flex_col()
                .children(lines.iter().map(|line| self.preview_code_line(line, None)))
                .into_any_element(),
            preview::MdBlock::Quote { spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
                    .my_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(hsla_alpha(theme.accent, 0.6))
                    .text_color(hsla_alpha(theme.foreground, 0.75))
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::Rule => div()
                .my_2()
                .h(px(1.0))
                .bg(hsla_alpha(theme.pane_border, 0.9))
                .into_any_element(),
        }
    }

    /// 選択ハイライト付きコード行 + bounds 追跡 canvas
    fn preview_code_line_sel(
        &self,
        line: &preview::Line,
        number: Option<(usize, usize)>,
        sel_range: Option<(usize, usize)>,
        pane_id: PaneId,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
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
        let code_el = StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
        let entity = cx.entity().downgrade();
        let bounds_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |app, _| {
                        let list = app.preview_line_bounds.entry(pane_id).or_default();
                        if list.len() <= line_idx {
                            list.resize(line_idx + 1, Bounds::default());
                        }
                        list[line_idx] = bounds;
                    });
                }
            },
        )
        .absolute()
        .size_full();

        if let Some((n, width)) = number {
            let num_label = format!("{n:>width$}  ");
            let num_len = num_label.len();
            div()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .child(StyledText::new(num_label).with_default_highlights(
                            &self.text_style(),
                            vec![(
                                0..num_len,
                                HighlightStyle {
                                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                                    ..HighlightStyle::default()
                                },
                            )],
                        )),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(code_el)
                        .child(bounds_canvas),
                )
        } else {
            div().relative().child(code_el).child(bounds_canvas)
        }
    }

    /// 選択ハイライト付き Markdown ブロック + bounds 追跡 canvas
    fn preview_md_block_sel(
        &self,
        block: &preview::MdBlock,
        sel_range: Option<(usize, usize)>,
        pane_id: PaneId,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let entity = cx.entity().downgrade();
        let bounds_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |app, _| {
                        let list = app.preview_line_bounds.entry(pane_id).or_default();
                        if list.len() <= line_idx {
                            list.resize(line_idx + 1, Bounds::default());
                        }
                        list[line_idx] = bounds;
                    });
                }
            },
        )
        .absolute()
        .size_full();

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
                let size = match level {
                    1 => 19.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                div()
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
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::Paragraph { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
                    .relative()
                    .py_1()
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
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
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            StyledText::new(text)
                                .with_default_highlights(&self.text_style(), highlights),
                        ),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::CodeBlock { lines } => div()
                .relative()
                .my_1()
                .p_2()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                .flex()
                .flex_col()
                .children(lines.iter().map(|line| self.preview_code_line(line, None)))
                .child(bounds_canvas)
                .into_any_element(),
            preview::MdBlock::Quote { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
                    .relative()
                    .my_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(hsla_alpha(theme.accent, 0.6))
                    .text_color(hsla_alpha(theme.foreground, 0.75))
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::Rule => div()
                .relative()
                .my_2()
                .h(px(1.0))
                .bg(hsla_alpha(theme.pane_border, 0.9))
                .child(bounds_canvas)
                .into_any_element(),
        }
    }
}

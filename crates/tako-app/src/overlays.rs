//! オーバーレイ UI — Attention トースト + ⌘K コマンドパレット（Issue #217）
//!
//! カンプ: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html` の
//! ATTENTION TOAST セクション（右下 300px、失敗即知 + ジャンプ/再実行）と
//! タブバーの ⌘K 検索エントリから開くコマンドパレット。

use gpui::{div, point, prelude::*, px, svg, BoxShadow, Context, FontWeight, SharedString};

use super::*;
use crate::file_icons::ui_icon;

impl TakoApp {
    /// Attention トースト（#217 カンプ: bottom 44 / right 14 / w300）。
    /// 失敗したペインの即時通知。ジャンプ / 再実行 / × で閉じる
    pub(crate) fn render_attention_toasts(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.toasts.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(44.0))
                .right(px(14.0))
                .w(px(300.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(self.toasts.iter().enumerate().map(|(i, toast)| {
                    let pane = toast.pane;
                    let elapsed = format_state_elapsed(toast.at.elapsed());
                    div()
                        .rounded(px(10.0))
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_heavy))
                        .shadow(vec![
                            BoxShadow {
                                color: gpui::hsla(0., 0., 0., 0.5),
                                offset: point(px(0.), px(12.)),
                                blur_radius: px(32.),
                                spread_radius: px(0.),
                                inset: false,
                            },
                            BoxShadow {
                                color: gpui::hsla(0., 0., 0., 0.3),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(1.),
                                inset: false,
                            },
                        ])
                        .overflow_hidden()
                        .occlude()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(12.0))
                                .py(px(10.0))
                                .child(
                                    svg()
                                        .path(ui_icon::WARNING)
                                        .w(px(15.0))
                                        .h(px(15.0))
                                        .flex_none()
                                        .text_color(hsla(theme.red)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(hsla(theme.foreground))
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(SharedString::from(toast.title.clone())),
                                        )
                                        .child(
                                            div()
                                                .font_family(theme.font_family.clone())
                                                .text_size(px(10.5))
                                                .text_color(hsla(theme.text_muted))
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(SharedString::from(format!(
                                                    "{} \u{00B7} {elapsed}前",
                                                    toast.detail
                                                ))),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(("toast-close", i as u64))
                                        .flex_none()
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toasts.retain(|t| t.pane != pane);
                                            cx.notify();
                                        }))
                                        .child(
                                            svg()
                                                .path(ui_icon::CLOSE)
                                                .w(px(12.0))
                                                .h(px(12.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(6.0))
                                .pl(px(35.0))
                                .pr(px(12.0))
                                .pb(px(10.0))
                                .child(
                                    div()
                                        .id(("toast-jump", i as u64))
                                        .px(px(11.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .border_1()
                                        .border_color(hsla_alpha(theme.red, 0.45))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.red))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.1)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toasts.retain(|t| t.pane != pane);
                                            this.jump_to_pane(pane, cx);
                                        }))
                                        .child("ジャンプ"),
                                )
                                .child(
                                    div()
                                        .id(("toast-retry", i as u64))
                                        .px(px(11.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .border_1()
                                        .border_color(hsla(theme.border_heavy))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.text_tertiary))
                                        .cursor_pointer()
                                        .hover(|d| {
                                            d.text_color(hsla(theme.foreground))
                                                .border_color(hsla(theme.text_overlay))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toasts.retain(|t| t.pane != pane);
                                            this.retry_last_command(pane, cx);
                                        }))
                                        .child("再実行"),
                                ),
                        )
                }))
                .into_any_element(),
        )
    }

    /// ⌘K コマンドパレット（#217 カンプ。上部中央のモーダル + 検索 + 候補リスト）
    pub(crate) fn render_command_palette(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let palette = self.command_palette.as_ref()?;
        let theme = self.theme.clone();
        let query = palette.query.clone();
        let items = self.palette_items(&query);
        let selected = palette.selected.min(items.len().saturating_sub(1));
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                // 背景クリックで閉じる
                .id("palette-backdrop")
                .occlude()
                .bg(gpui::hsla(0., 0., 0., 0.3))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.command_palette = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("palette-panel")
                        .mt(px(90.0))
                        .w(px(560.0))
                        .rounded(px(10.0))
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_heavy))
                        .shadow(vec![BoxShadow {
                            color: gpui::hsla(0., 0., 0., 0.55),
                            offset: point(px(0.), px(16.)),
                            blur_radius: px(40.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .overflow_hidden()
                        .occlude()
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }))
                        // 検索入力行（カンプの ⌘K エントリと同じデザイン言語）
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(9.0))
                                .px(px(14.0))
                                .h(px(44.0))
                                .border_b_1()
                                .border_color(hsla(theme.border_subtle))
                                .child(
                                    svg()
                                        .path(ui_icon::SEARCH)
                                        .w(px(14.0))
                                        .h(px(14.0))
                                        .text_color(hsla(theme.text_muted)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .text_size(px(13.0))
                                        .when(query.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .text_color(hsla(theme.text_faint))
                                                    .child("ペイン・コマンド検索"),
                                            )
                                        })
                                        .when(!query.is_empty(), |d| {
                                            d.text_color(hsla(theme.foreground))
                                                .child(SharedString::from(query.clone()))
                                        })
                                        .child(
                                            // カーソル
                                            div()
                                                .w(px(1.5))
                                                .h(px(16.0))
                                                .ml(px(1.0))
                                                .bg(hsla(theme.accent)),
                                        ),
                                )
                                .child(
                                    div()
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.0))
                                        .text_color(hsla(theme.text_faint))
                                        .border_1()
                                        .border_color(hsla(theme.surface_highlight))
                                        .rounded(px(4.0))
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .child("esc"),
                                ),
                        )
                        // 候補リスト（最大 10 件）
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .p(px(4.0))
                                .when(items.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .px(px(10.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .text_color(hsla(theme.text_faint))
                                            .child("該当なし"),
                                    )
                                })
                                .children(items.into_iter().take(10).enumerate().map(
                                    |(i, item)| {
                                        let is_selected = i == selected;
                                        let label = item.label();
                                        let is_pane = matches!(item, PaletteItem::Pane(..));
                                        div()
                                            .id(("palette-item", i as u64))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.0))
                                            .px(px(10.0))
                                            .py(px(7.0))
                                            .rounded(px(6.0))
                                            .cursor_pointer()
                                            .when(is_selected, |d| {
                                                d.bg(rgba_alpha(theme.accent, 0.12))
                                            })
                                            .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                let query = this
                                                    .command_palette
                                                    .as_ref()
                                                    .map(|p| p.query.clone())
                                                    .unwrap_or_default();
                                                let items = this.palette_items(&query);
                                                this.command_palette = None;
                                                if let Some(item) = items.into_iter().nth(i) {
                                                    this.palette_execute(item, cx);
                                                }
                                            }))
                                            .child(
                                                svg()
                                                    .path(if is_pane {
                                                        ui_icon::SPLIT
                                                    } else {
                                                        ui_icon::JUMP_ARROW
                                                    })
                                                    .w(px(13.0))
                                                    .h(px(13.0))
                                                    .flex_none()
                                                    .text_color(if is_selected {
                                                        hsla(theme.accent)
                                                    } else {
                                                        hsla(theme.text_muted)
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .text_size(px(12.5))
                                                    .text_color(if is_selected {
                                                        hsla(theme.foreground)
                                                    } else {
                                                        hsla(theme.text_tertiary)
                                                    })
                                                    .child(SharedString::from(label)),
                                            )
                                    },
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}

//! オーバーレイ UI — ⌘K コマンドパレット（Issue #217）+
//! 初回起動のウェルカムバナー（Issue #549）

use gpui::{div, point, prelude::*, px, svg, BoxShadow, Context, SharedString};

use super::*;
use crate::file_icons::ui_icon;

impl TakoApp {
    /// 初回起動のウェルカムバナー（Issue #549）。
    ///
    /// タブバーの直下に全幅で出す。初回起動でしか出ないので、ペインを 1 行ぶんも
    /// 削らないオーバーレイにはせず素直に積む（見落とされたら意味が無い）。
    /// 表示条件は `TakoApp::welcome_banner`（起動時に settings.json の実在で判定）
    pub(crate) fn render_welcome_banner(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.welcome_banner {
            return None;
        }
        use crate::ui_text::welcome as txt;
        let theme = self.theme.clone();

        // 主ボタン（アクセント塗り）と副ボタン（枠のみ）の共通形
        let button = |id: &'static str, label: String, primary: bool| {
            let t = theme.clone();
            div()
                .id(id)
                .flex_none()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(5.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(primary, |d| {
                    let t = t.clone();
                    d.bg(rgba(t.accent))
                        .text_color(hsla(t.background))
                        .hover(move |d| d.bg(rgba_alpha(t.accent, 0.85)))
                })
                .when(!primary, |d| {
                    let t = t.clone();
                    d.bg(rgba(t.chip_surface))
                        .border_1()
                        .border_color(hsla(t.border_default))
                        .text_color(hsla(t.accent))
                        .hover(move |d| d.bg(rgba_alpha(t.accent, 0.18)))
                })
                .child(SharedString::from(label))
        };

        // 「説明 …… ボタン」の 1 行。長文は省略記号で畳んでボタンを守る
        let step_text = |text: String| {
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .text_color(hsla(theme.text_secondary))
                .child(SharedString::from(text))
        };
        let step_row = || div().flex().flex_row().items_center().gap(px(10.0));

        Some(
            div()
                .id("welcome-banner")
                .flex_none()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(14.0))
                .py(px(10.0))
                .bg(rgba_alpha(theme.accent, 0.10))
                .border_b_1()
                .border_color(hsla_alpha(theme.accent, 0.28))
                // 見出し行: タイトル + 設定リンク + 閉じる
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(theme.foreground))
                                .child(SharedString::from(txt::title().to_string())),
                        )
                        .child(
                            div()
                                .id("welcome-settings")
                                .flex_none()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .text_size(px(11.0))
                                .text_color(hsla(theme.text_muted))
                                .cursor_pointer()
                                .hover({
                                    let accent = theme.accent;
                                    move |d| d.text_color(hsla(accent))
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dismiss_welcome_banner(cx);
                                    this.pending_settings_open = Some(None);
                                    cx.notify();
                                }))
                                .child(SharedString::from(txt::open_settings_button().to_string())),
                        )
                        // 閉じるは「次回から出さない」ことまで明示する（黙って消えると
                        // 出し直し方が分からなくなる。出し直しは `tako welcome show`）
                        .child(
                            div()
                                .id("welcome-dismiss")
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .text_size(px(11.0))
                                .text_color(hsla(theme.text_muted))
                                .cursor_pointer()
                                .hover({
                                    let hl = theme.surface_highlight;
                                    move |d| d.bg(rgba(hl))
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.dismiss_welcome_banner(cx)),
                                )
                                .child(SharedString::from(txt::dismiss_hint().to_string()))
                                .child(
                                    svg()
                                        .path(ui_icon::CLOSE)
                                        .w(px(9.0))
                                        .h(px(9.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_muted)),
                                ),
                        ),
                )
                .child(
                    step_row()
                        .child(step_text(txt::step_setup().to_string()))
                        .child(
                            button(
                                "welcome-run-setup",
                                txt::run_setup_button().to_string(),
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.run_setup_command(cx))),
                        ),
                )
                .child(
                    step_row()
                        .child(step_text(txt::step_master().to_string()))
                        .child(
                            button(
                                "welcome-run-master",
                                txt::run_master_button().to_string(),
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.run_master_command(cx))),
                        ),
                )
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
                                            d.child(div().text_color(hsla(theme.text_faint)).child(
                                                crate::ui_text::palette::search_placeholder(),
                                            ))
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
                                            .child(crate::ui_text::palette::no_match()),
                                    )
                                })
                                .children(items.into_iter().take(10).enumerate().map(
                                    |(i, item)| {
                                        let is_selected = i == selected;
                                        let label = item.label();
                                        let is_pane = matches!(item, PaletteItem::Pane(..));
                                        // 固定コマンドだけがショートカットを持つ（#648）
                                        let shortcut = match item {
                                            PaletteItem::Command(_, id) => {
                                                crate::keybindings::palette_shortcut(id)
                                            }
                                            _ => None,
                                        };
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
                                            // ショートカット併記（#648）。Windows には
                                            // メニューバーが無く、`cmd-` から機械的に
                                            // 読み替えられないキー（分割 = Ctrl+Shift+D 等）を
                                            // 知る手段がここしか無い
                                            .children(shortcut.map(|keys| {
                                                div()
                                                    .flex_none()
                                                    .text_size(px(11.0))
                                                    .text_color(hsla(theme.text_muted))
                                                    .child(SharedString::from(keys))
                                            }))
                                    },
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}

//! スターター — GUI ライク表示モードの「空ペイン 3 ボタン」（Issue #691 / #694）
//!
//! 仕様の正は `.agent/plans/2026-07-gui-mode.md` §2.2。ターミナルに抵抗感のある
//! ユーザーが最初に見る画面なので、**次の行動が常に 3 つに絞られている**ことを最優先にする。
//!
//! 表示するかどうかは `TakoApp::pane_display_for`（判定表 = `tako_core::ui_mode`）が決める。
//! ここは描画とクリックの配線だけを持ち、押下時の実処理は
//! `TakoApp::starter_action`（= dispatch / シェル書き込み）に委ねる。

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, MouseButton,
    SharedString,
};
use std::time::Duration;
use tako_core::ui_mode::{SettleKind, StarterAction};

use super::*;
use crate::file_icons::ui_icon;

impl TakoApp {
    /// ペイン枠 + ヘッダ（スターターと準備中プレースホルダの共通部分）。
    /// ターミナルペインと同じ高さ・同じ × 位置にしてあるので、GUI 表示のまま閉じられる
    fn render_gui_pane_frame(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        focused: bool,
        show_terminal_button: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        // cwd チップ（ヘッダ左。いまどのフォルダで話が始まるかを見せる）
        let cwd_label = self.terminals.get(&pane_id).and_then(|s| s.cwd()).map(|p| {
            let full = p.to_string_lossy().to_string();
            let home = std::env::var("HOME").unwrap_or_default();
            if !home.is_empty() && full.starts_with(&home) {
                format!("~{}", &full[home.len()..])
            } else {
                full
            }
        });

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
                d.shadow(vec![BoxShadow {
                    color: hsla_alpha(theme.accent, 0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                }])
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    let _ = this.workspace.active_tab_mut().tree_mut().focus(pane_id);
                    cx.notify();
                }),
            )
            .child(
                div()
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
                    .child(
                        svg()
                            .path(ui_icon::CHAT_BUBBLE)
                            .w(px(13.0))
                            .h(px(13.0))
                            .flex_none()
                            .text_color(hsla(theme.accent)),
                    )
                    .when_some(cwd_label, |d, label| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(label)),
                        )
                    })
                    .child(div().flex_grow(1.0))
                    // 準備中は「待たずに中身を見る」逃げ道を必ず用意する（#720）。
                    // チャットヘッダの同名ボタンと同じ揮発解除の経路
                    .when(show_terminal_button, |d| {
                        d.child(
                            div()
                                .id(("gui-pane-to-terminal", pane_id.as_u64()))
                                .flex()
                                .flex_none()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .cursor_pointer()
                                .border_1()
                                .border_color(hsla(theme.border_subtle))
                                .text_color(hsla(theme.text_secondary))
                                .hover(|d| {
                                    d.bg(rgba(theme.surface_hover))
                                        .border_color(hsla(theme.border_default))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation()
                                    }),
                                )
                                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.starter_action(pane_id, StarterAction::UseTerminal, cx);
                                }))
                                .child(
                                    svg()
                                        .path(ui_icon::PROMPT)
                                        .w(px(11.0))
                                        .h(px(11.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_secondary)),
                                )
                                .child(SharedString::from(txt::chat_show_terminal().to_string())),
                        )
                    })
                    .child(
                        div()
                            .id(("starter-close", pane_id.as_u64()))
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
                                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation()
                                }),
                            )
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_pane_with_confirm(
                                    pane_id,
                                    event.modifiers().platform,
                                    CloseOrigin::PaneButton,
                                    cx,
                                );
                            }))
                            .child(
                                svg()
                                    .path(ui_icon::CLOSE)
                                    .w(px(13.0))
                                    .h(px(13.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    ),
            )
    }

    /// 準備中プレースホルダ（Issue #720）。ペインを作った直後・エージェントを起動した
    /// 直後の「表示種別が決まっていない」数秒を覆う。
    ///
    /// **上限つきの過渡期（`tako_core::ui_mode::SettleState`）でしか出ない**ので、
    /// ここが出っぱなしになることはない。加えてヘッダの「ターミナルを表示」で
    /// いつでも中身へ抜けられる（起動が失敗して固まったときの逃げ道）
    pub(crate) fn render_preparing_pane(
        &mut self,
        pane_id: PaneId,
        kind: SettleKind,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        let width = f32::from(area.size.width);
        let height = f32::from(area.size.height);
        let compact = width < 420.0 || height < 260.0;

        // 明滅する点（チャットの busy ドット・タブバーの実行中ドットと同じ表現）。
        // 新しい UI 表現を増やさないのと、**アニメーションが毎フレーム再描画を要求する**
        // ので、猶予が切れた瞬間に何もしなくても通常表示へ入れ替わる
        let pulse = div()
            .w(px(9.0))
            .h(px(9.0))
            .flex_none()
            .rounded_full()
            .bg(hsla(theme.accent))
            .with_animation(
                ("pane-preparing-pulse", pane_id.as_u64()),
                Animation::new(Duration::from_secs(2)).repeat(),
                |el, t| el.opacity(1.0 - 0.7 * (std::f32::consts::PI * t).sin()),
            );

        let detail = match kind {
            SettleKind::Shell => txt::preparing_shell(),
            SettleKind::Agent => txt::preparing_agent(),
        };

        self.render_gui_pane_frame(pane_id, rect, focused, true, cx)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(9.0))
                            .child(pulse)
                            .child(
                                div()
                                    .text_size(px(if compact { 13.0 } else { 15.0 }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(txt::preparing_title().to_string())),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(detail.to_string())),
                    ),
            )
    }

    /// スターター表示のペイン 1 枚（ターミナルペインと同じ枠 + 3 カード + setup リンク）
    pub(crate) fn render_starter_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        // 狭いペインでは説明文とコマンド併記を落とす（見切れさせない。#185 と同方針）
        let width = f32::from(area.size.width);
        let height = f32::from(area.size.height);
        let compact = width < 420.0 || height < 260.0;
        let very_compact = width < 300.0 || height < 190.0;

        let card = |action: StarterRow, cx: &mut Context<Self>| -> gpui::AnyElement {
            let StarterRow {
                action,
                icon,
                title,
                desc,
                command,
                primary,
            } = action;
            let t = theme.clone();
            div()
                .id((
                    "starter-card",
                    (pane_id.as_u64() << 4) | action_index(action),
                ))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .w_full()
                // 説明が 2 行に折り返してもカードが潰れて隣と重ならないようにする
                // （flex の自動最小サイズに頼れないので明示する。#656 と同じ罠）
                .flex_shrink_0()
                .px(px(14.0))
                .py(if compact { px(10.0) } else { px(13.0) })
                .rounded(px(10.0))
                .border_1()
                .cursor_pointer()
                .when(primary, |d| {
                    let t = t.clone();
                    d.bg(rgba_alpha(t.accent, 0.14))
                        .border_color(hsla_alpha(t.accent, 0.55))
                        .hover(move |d| d.bg(rgba_alpha(t.accent, 0.24)))
                })
                .when(!primary, |d| {
                    let t = t.clone();
                    d.bg(rgba(t.surface_1))
                        .border_color(hsla(t.border_subtle))
                        .hover(move |d| {
                            d.bg(rgba(t.surface_hover))
                                .border_color(hsla(t.border_default))
                        })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.starter_action(pane_id, action, cx);
                }))
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(30.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.0))
                        .bg(rgba(if primary {
                            theme.surface_2
                        } else {
                            theme.chip_surface
                        }))
                        .child(svg().path(icon).w(px(15.0)).h(px(15.0)).text_color(hsla(
                            if primary {
                                theme.accent
                            } else {
                                theme.text_secondary
                            },
                        ))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(7.0))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.foreground))
                                        .child(SharedString::from(title)),
                                )
                                // 実コマンドの併記（初心者の学習経路。#322 の最簡形）
                                .when_some(
                                    command.filter(|_| !compact),
                                    |d, command: &'static str| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .font_family(theme.font_family.clone())
                                                .text_size(px(10.0))
                                                .text_color(hsla(theme.text_faint))
                                                .child(command),
                                        )
                                    },
                                ),
                        )
                        .when(!very_compact, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(hsla(theme.text_secondary))
                                    .child(SharedString::from(desc)),
                            )
                        }),
                )
                .into_any_element()
        };

        self.render_gui_pane_frame(pane_id, rect, focused, false, cx)
            // 本体: 見出し + カード 3 枚（縦積み・中央寄せ）
            .child(
                div()
                    .id(("starter-body", pane_id.as_u64()))
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(if compact { px(8.0) } else { px(10.0) })
                    .px(px(16.0))
                    .py(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(460.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(if compact { 14.0 } else { 16.0 }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(txt::starter_title().to_string())),
                            )
                            .when(!very_compact, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(hsla(theme.text_muted))
                                        .child(SharedString::from(
                                            txt::starter_subtitle().to_string(),
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(460.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .children(starter_rows().into_iter().map(|row| card(row, cx))),
                    )
                    // 下部の控えめなリンク行（#720）。カードと同格の主要導線にはしない
                    // （初回バナー #549 と役割が重なるため）。実行方式はカードと同じ
                    // シェル書き込みで、AI 側の等価操作は既存の `tako setup`
                    .when(!very_compact, |d| {
                        d.child(
                            div()
                                .id(("starter-setup-link", pane_id.as_u64()))
                                .w_full()
                                .max_w(px(460.0))
                                .flex_shrink_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(4.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.text_muted))
                                .hover(|d| {
                                    d.bg(rgba(theme.surface_hover))
                                        .text_color(hsla(theme.text_secondary))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation()
                                    }),
                                )
                                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.starter_action(pane_id, StarterAction::Setup, cx);
                                }))
                                // 「やり直す」= 既存の循環矢印（#438）。UI に絵文字は使わない
                                .child(
                                    svg()
                                        .path(ui_icon::REFRESH)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_muted)),
                                )
                                .child(SharedString::from(txt::starter_setup_link().to_string()))
                                .when(!compact, |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .font_family(theme.font_family.clone())
                                            .text_size(px(10.0))
                                            .text_color(hsla(theme.text_faint))
                                            .child(txt::CARD_SETUP_COMMAND),
                                    )
                                }),
                        )
                    })
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .w_full()
                                .max_w(px(460.0))
                                .flex_shrink_0()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(txt::starter_footnote().to_string())),
                        )
                    }),
            )
    }
}

/// 1 枚のカードの材料（描画に必要なものだけ。文言は `ui_text::ui_mode` が正）
struct StarterRow {
    action: StarterAction,
    icon: &'static str,
    title: String,
    desc: String,
    command: Option<&'static str>,
    primary: bool,
}

fn starter_rows() -> Vec<StarterRow> {
    use crate::ui_text::ui_mode as txt;
    vec![
        StarterRow {
            action: StarterAction::Master,
            icon: ui_icon::ORCH,
            title: txt::card_master_title().to_string(),
            desc: txt::card_master_desc().to_string(),
            command: Some(txt::CARD_MASTER_COMMAND),
            primary: true,
        },
        StarterRow {
            action: StarterAction::Solo,
            icon: ui_icon::CHAT_BUBBLE,
            title: txt::card_solo_title().to_string(),
            desc: txt::card_solo_desc().to_string(),
            command: Some(txt::CARD_SOLO_COMMAND),
            primary: false,
        },
        StarterRow {
            action: StarterAction::UseTerminal,
            icon: ui_icon::PROMPT,
            title: txt::card_terminal_title().to_string(),
            desc: txt::card_terminal_desc().to_string(),
            command: None,
            primary: false,
        },
    ]
}

/// 要素 ID の安定した連番（同一ペイン内でカードごとに違う ID にする）
fn action_index(action: StarterAction) -> u64 {
    match action {
        StarterAction::Master => 0,
        StarterAction::Solo => 1,
        StarterAction::UseTerminal => 2,
        // setup はカードではなく下部リンク（専用 ID を持つ）ので、
        // ここに来るのは将来カード化したときだけ
        StarterAction::Setup => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// カードは仕様の 3 枚がこの順で並ぶ（master が主 = 既定の一手）
    #[test]
    fn カードは3枚で順序と起動コマンドが仕様どおり() {
        let rows = starter_rows();
        assert_eq!(rows.len(), 3);
        let actions: Vec<StarterAction> = rows.iter().map(|r| r.action).collect();
        assert_eq!(
            actions,
            vec![
                StarterAction::Master,
                StarterAction::Solo,
                StarterAction::UseTerminal
            ]
        );
        assert!(rows[0].primary, "AI チームに任せるが主ボタン");
        assert_eq!(rows[0].command, Some("tako master"));
        assert_eq!(rows[1].command, Some("tako solo"));
        // 「コマンド入力へ」は何も起動しないのでコマンド併記も無い
        assert_eq!(rows[2].command, None);
        assert_eq!(rows[2].action.subcommand(), None);
        // 要素 ID がカードごとに衝突しない
        let mut ids: Vec<u64> = actions.iter().map(|a| action_index(*a)).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }
}

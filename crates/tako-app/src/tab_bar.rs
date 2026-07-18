//! タブバー — Claude Design カンプ準拠のピル型タブ（Issue #217）
//!
//! カンプ: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html` の
//! tab-bar セクション。高さ 44px / ピル型タブ（h30・radius 8）/ ペイン状態
//! ミニインジケータ / fail 数表示 / ⌘K 検索エントリ / 通知ベル + バッジ /
//! テーマ切替ボタン。traffic lights はタイトルバー統合（native）で同居する。
//!
//! オーバーフロー対応（Issue #208）: タブ数に応じてラベルを縮小 + GPUI
//! ScrollHandle で横スクロール + アクティブタブの自動スクロールイン。

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, DragMoveEvent,
    FontWeight, SharedString, WindowControlArea,
};
use tako_core::{CommandState, TitleSource};

use super::*;
use crate::file_icons::ui_icon;

/// traffic lights（12px × 3 + gap 8px × 2 = 52px）+ 右余白 16px
const TRAFFIC_LIGHTS_SPACER: f32 = 68.0;

/// 1 タブのラベル込みの参考幅（px）。ラベル truncate 上限を決めるために使う概算値。
/// 実測: dot(7) + gap(8) + pl(10) + label + pr(11) + gap(3)。
/// ラベル 1 文字あたり約 7px（12.5px フォントの平均グリフ幅）
const TAB_CHROME_PX: f32 = 42.0;
const CHAR_WIDTH_PX: f32 = 7.0;
/// タブラベルの最大文字数（通常時）
const LABEL_MAX_CHARS: usize = 24;
/// タブラベルの最小文字数（縮小限界）
const LABEL_MIN_CHARS: usize = 6;
/// 右端コントロール群の概算幅（⌘K(210+px) + bell(30) + theme(30) + gap + margin）
const RIGHT_CONTROLS_PX: f32 = 300.0;

impl TakoApp {
    /// タブ数と利用可能幅からラベルの truncate 上限文字数を決定する
    fn tab_label_max_chars(&self, tab_count: usize, window: &Window) -> usize {
        if tab_count == 0 {
            return LABEL_MAX_CHARS;
        }
        let vw = f32::from(window.viewport_size().width);
        let available = vw - TRAFFIC_LIGHTS_SPACER - RIGHT_CONTROLS_PX - 40.0;
        let per_tab = available / tab_count as f32;
        let label_px = (per_tab - TAB_CHROME_PX).max(0.0);
        let chars = (label_px / CHAR_WIDTH_PX) as usize;
        chars.clamp(LABEL_MIN_CHARS, LABEL_MAX_CHARS)
    }

    /// アクティブタブが表示領域に入るよう ScrollHandle を更新する。
    /// タブ切替を行うすべての経路（クリック・⌘数字・CLI/MCP）から呼ぶ。
    /// scroll_to_item は子要素インデックスで動くため、タブバーの表示と同じ
    /// アクティブウィンドウ内の並びで位置を計算する（Issue #339）
    pub(crate) fn scroll_active_tab_into_view(&self) {
        let active = self.workspace.active_tab_id();
        let win_tabs = self
            .workspace
            .window_tab_ids(self.workspace.active_window_id());
        if let Some(idx) = win_tabs.iter().position(|t| *t == active) {
            self.tab_scroll_handle.scroll_to_item(idx);
        }
    }

    /// タブバーの + ボタン: クリックされたウィンドウに新規タブを作る（Issue #339。
    /// 非アクティブウィンドウの + でも activation イベントの順序に依存せず正しく動かす）
    pub(crate) fn new_tab_in_viewport(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(lid) = self.viewport_of(window) {
            if self.workspace.get_window(lid).is_some() && self.workspace.active_window_id() != lid
            {
                let _ = self.workspace.activate_window(lid);
            }
        }
        self.new_tab(cx);
    }

    pub(crate) fn render_tab_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        // このウィンドウの表示タブと所属タブだけを描く（Issue #339 ビューポート方式）
        let viewport = self
            .viewport_of(window)
            .unwrap_or_else(|| self.workspace.active_window_id());
        let active = self
            .workspace
            .get_window(viewport)
            .map(|w| w.active_tab())
            .unwrap_or_else(|| self.workspace.active_tab_id());

        // アクティブタブが変わった（dispatch / new_tab 等）ら自動スクロールイン。
        // スクロール位置（tab_scroll_handle）は共有のためアクティブウィンドウでのみ追従
        if viewport == self.workspace.active_window_id() && self.last_active_tab != Some(active) {
            self.last_active_tab = Some(active);
            self.scroll_active_tab_into_view();
        }

        let window_tabs = self.workspace.window_tab_ids(viewport);
        let tabs: Vec<_> = self
            .workspace
            .tabs()
            .iter()
            .filter(|tab| window_tabs.contains(&tab.id()))
            .map(|tab| {
                let id = tab.id();
                let label = if tab.title_source() == TitleSource::Default {
                    tab.tree()
                        .panes()
                        .iter()
                        .find(|p| p.id() == tab.tree().focused())
                        .and_then(|p| self.terminals.get(&p.id()))
                        .and_then(|s| s.title())
                        .unwrap_or(tab.title())
                        .to_string()
                } else {
                    tab.title().to_string()
                };
                let pane_states: Vec<CommandState> = tab
                    .tree()
                    .panes()
                    .iter()
                    .filter_map(|p| self.terminals.get(&p.id()))
                    .map(|s| s.command_state())
                    .collect();
                let agg = CommandState::aggregate(pane_states.iter().cloned());
                let fails = pane_states
                    .iter()
                    .filter(|s| matches!(s, CommandState::Failed(_)))
                    .count();
                (id, label, agg, pane_states, fails)
            })
            .collect();
        let attention: usize = tabs.iter().map(|(_, _, _, _, fails)| fails).sum();
        let state_color = |state: &CommandState| match state {
            CommandState::Failed(_) => theme.red,
            CommandState::Running => theme.accent,
            CommandState::Idle => theme.green,
            CommandState::Unknown => theme.text_overlay,
        };

        let label_max = self.tab_label_max_chars(tabs.len(), window);
        let tab_drop = self.tab_drop_target;
        let is_pane_dragging = self.drag_kind == Some(DragKind::Pane);
        let tab_reorder = self.tab_reorder_indicator;
        let is_tab_dragging = self.drag_kind == Some(DragKind::Tab);

        div()
            .id("tab-bar")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .pl(px(16.0))
            .pr(px(12.0))
            .bg(rgba(theme.mantle))
            .border_b_1()
            .border_color(hsla(theme.border_subtle))
            .window_control_area(WindowControlArea::Drag)
            // macOS: タブバー空き領域のドラッグでウインドウ移動（#312）。
            // GPUI の WindowControlArea::Drag は hitbox 登録のみ。macOS では
            // on_hit_test_window_control が空実装のため、Zed と同じく
            // mouse_down → mouse_move で start_window_move() を明示呼び出しする
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = true;
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = false;
                    this.tab_mouse_down = false;
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.titlebar_dragging = false;
                this.tab_mouse_down = false;
            }))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.titlebar_dragging && !this.tab_mouse_down {
                    this.titlebar_dragging = false;
                    window.start_window_move();
                }
            }))
            // ダブルクリックでズーム（macOS 標準操作。#312）
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            // native traffic lights の載る領域
            .child(div().w(px(TRAFFIC_LIGHTS_SPACER)).h_full().flex_none())
            // タブ領域（横スクロール対応。Issue #208）
            // scroll_to_item が直接子要素のインデックスで動作するため、
            // タブを scrollable コンテナの直接子要素にする
            .child(
                div()
                    .id("tab-scroll-area")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll_handle)
                    .on_drag_move::<TabDrag>(cx.listener(
                        |this, _: &DragMoveEvent<TabDrag>, _, cx| {
                            this.set_tab_reorder_indicator(None, cx);
                        },
                    ))
                    .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                        this.drop_tab_reorder(drag.tab, None, cx);
                    }))
                    .on_drag_move::<PaneDrag>(cx.listener(
                        |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                            this.set_tab_drop_target(None, cx);
                        },
                    ))
                    .on_drop::<PaneDrag>(cx.listener(|this, drag: &PaneDrag, _, cx| {
                        this.drop_pane_on_tab(drag.pane, None, cx);
                    }))
                    .children(
                        tabs.into_iter()
                            .map(|(id, label, agg, pane_states, fails)| {
                                let is_active = id == active;
                                let dot_color = state_color(&agg);
                                let pulsing = matches!(agg, CommandState::Running);

                                let dot = div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(hsla(dot_color))
                                    .when(is_active, |d| {
                                        d.shadow(vec![BoxShadow {
                                            color: hsla_alpha(dot_color, 0.7),
                                            offset: point(px(0.), px(0.)),
                                            blur_radius: px(6.0),
                                            spread_radius: px(0.),
                                            inset: false,
                                        }])
                                    });
                                let dot = if pulsing {
                                    dot.with_animation(
                                        ("tab-dot-pulse", id.as_u64()),
                                        Animation::new(Duration::from_secs(2)).repeat(),
                                        |el, t| {
                                            el.opacity(
                                                1.0 - 0.65 * (std::f32::consts::PI * t).sin(),
                                            )
                                        },
                                    )
                                    .into_any_element()
                                } else {
                                    dot.into_any_element()
                                };

                                let truncated = truncate(&label, label_max);

                                // タブ D&D 並べ替えの挿入インジケータ（#308）
                                let show_indicator =
                                    is_tab_dragging && tab_reorder == Some(Some(id));

                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .flex_shrink_0()
                                    .when(show_indicator, |d| {
                                        d.child(
                                            div()
                                                .w(px(2.0))
                                                .h(px(22.0))
                                                .flex_none()
                                                .rounded(px(1.0))
                                                .bg(hsla(theme.accent)),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id(("tab", id.as_u64()))
                                            .group("tab-pill")
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.0))
                                            .h(px(30.0))
                                            .pl(px(10.0))
                                            .pr(px(11.0))
                                            .flex_shrink_0()
                                            .rounded(px(8.0))
                                            .cursor_pointer()
                                            .when(is_active, |d| {
                                                d.bg(rgba(theme.tab_active_background))
                                                    .border_1()
                                                    .border_color(hsla(theme.border_heavy))
                                                    .shadow(vec![BoxShadow {
                                                        color: hsla_alpha(theme.foreground, 0.05),
                                                        offset: point(px(0.), px(1.)),
                                                        blur_radius: px(0.),
                                                        spread_radius: px(0.),
                                                        inset: true,
                                                    }])
                                            })
                                            .when(!is_active, |d| {
                                                d.hover(|d| d.bg(rgba(theme.surface_hover)))
                                            })
                                            .when(
                                                is_pane_dragging && tab_drop == Some(Some(id)),
                                                |d| {
                                                    d.bg(rgba_alpha(theme.accent, 0.15))
                                                        .border_2()
                                                        .border_color(hsla(theme.accent))
                                                },
                                            )
                                            .text_color(if is_active {
                                                hsla(theme.tab_active_foreground)
                                            } else if fails > 0 {
                                                hsla(theme.text_tertiary)
                                            } else {
                                                hsla(theme.tab_inactive_foreground)
                                            })
                                            .text_size(px(12.5))
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                cx.listener(move |this, _, _, _| {
                                                    this.tab_mouse_down = true;
                                                }),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let _ = this.workspace.activate_tab(id);
                                                this.scroll_active_tab_into_view();
                                                cx.notify();
                                            }))
                                            .on_drag(
                                                TabDrag { tab: id },
                                                self.drag_ghost_builder(
                                                    DragKind::Tab,
                                                    truncated.clone(),
                                                    cx,
                                                ),
                                            )
                                            .on_drag_move::<TabDrag>(cx.listener(
                                                move |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                                                    if e.drag(cx).tab == id {
                                                        return;
                                                    }
                                                    this.set_tab_reorder_indicator(Some(id), cx);
                                                },
                                            ))
                                            .on_drop::<TabDrag>(cx.listener(
                                                move |this, drag: &TabDrag, _, cx| {
                                                    this.drop_tab_reorder(drag.tab, Some(id), cx);
                                                },
                                            ))
                                            .on_drag_move::<PaneDrag>(cx.listener(
                                                move |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                                                    this.set_tab_drop_target(Some(id), cx);
                                                },
                                            ))
                                            .on_drop::<PaneDrag>(cx.listener(
                                                move |this, drag: &PaneDrag, _, cx| {
                                                    this.drop_pane_on_tab(drag.pane, Some(id), cx);
                                                },
                                            ))
                                            .child(dot)
                                            .child(
                                                div()
                                                    .font_weight(if is_active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::MEDIUM
                                                    })
                                                    .child(SharedString::from(truncated)),
                                            )
                                            .when(is_active && pane_states.len() > 1, |d| {
                                                d.child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(2.5))
                                                        .children(pane_states.iter().map(|s| {
                                                            div()
                                                                .w(px(5.0))
                                                                .h(px(5.0))
                                                                .flex_none()
                                                                .rounded(px(1.5))
                                                                .bg(hsla(state_color(s)))
                                                        })),
                                                )
                                            })
                                            .when(!is_active && fails > 0, |d| {
                                                d.child(
                                                    div()
                                                        .font_family(theme.font_family.clone())
                                                        .text_size(px(10.5))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(hsla(theme.red))
                                                        .child(SharedString::from(format!(
                                                            "{fails} fail"
                                                        ))),
                                                )
                                            })
                                            .when(is_active, |d| {
                                                d.child(
                                                    div()
                                                        .id(("tab-bg", id.as_u64()))
                                                        .w(px(17.0))
                                                        .h(px(17.0))
                                                        .flex()
                                                        .flex_none()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded(px(5.0))
                                                        .cursor_pointer()
                                                        .text_color(hsla(theme.text_muted))
                                                        .hover(|d| {
                                                            d.bg(rgba(theme.surface_highlight))
                                                                .text_color(hsla(theme.foreground))
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.background_tab(id, cx);
                                                            },
                                                        ))
                                                        .child(
                                                            svg()
                                                                .path(ui_icon::MINUS)
                                                                .w(px(12.0))
                                                                .h(px(12.0))
                                                                .text_color(hsla(theme.text_muted)),
                                                        ),
                                                )
                                            })
                                            .when(is_active, |d| {
                                                d.child(
                                            div()
                                                .id(("tab-close", id.as_u64()))
                                                .w(px(17.0))
                                                .h(px(17.0))
                                                .flex()
                                                .flex_none()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(5.0))
                                                .cursor_pointer()
                                                .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                                .on_click(cx.listener(
                                                    move |this, event: &gpui::ClickEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.close_tab_with_confirm(
                                                            id,
                                                            event.modifiers().platform,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    svg()
                                                        .path(ui_icon::CLOSE)
                                                        .w(px(12.0))
                                                        .h(px(12.0))
                                                        .text_color(hsla(theme.text_muted)),
                                                ),
                                        )
                                            }),
                                    ) // .child(div() inner tab pill)
                            }),
                    )
                    // 末尾の挿入インジケータ（タブ D&D 並べ替え: 末尾移動。#308）
                    .when(is_tab_dragging && tab_reorder == Some(None), |d| {
                        d.child(
                            div()
                                .w(px(2.0))
                                .h(px(22.0))
                                .flex_none()
                                .rounded(px(1.0))
                                .bg(hsla(theme.accent)),
                        )
                    })
                    // +: 新規タブ（カンプ 30×30 / radius 8）
                    .child(
                        div()
                            .id("tab-new")
                            .w(px(30.0))
                            .h(px(30.0))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_tab_in_viewport(window, cx)
                            }))
                            .on_drag_move::<TabDrag>(cx.listener(
                                |this, _: &DragMoveEvent<TabDrag>, _, cx| {
                                    this.set_tab_reorder_indicator(None, cx);
                                },
                            ))
                            .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                                this.drop_tab_reorder(drag.tab, None, cx);
                            }))
                            .on_drag_move::<PaneDrag>(cx.listener(
                                |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                                    this.set_tab_drop_target(None, cx);
                                },
                            ))
                            .on_drop::<PaneDrag>(cx.listener(|this, drag: &PaneDrag, _, cx| {
                                this.drop_pane_on_tab(drag.pane, None, cx);
                            }))
                            .when(self.tab_drop_target == Some(None), |d| {
                                d.bg(rgba_alpha(theme.accent, 0.2))
                                    .border_2()
                                    .border_color(hsla(theme.accent))
                            })
                            .child(
                                svg()
                                    .path(ui_icon::PLUS)
                                    .w(px(15.0))
                                    .h(px(15.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    ),
            )
            // ⌘K コマンドパレット入口（カンプ: h30 / min-w 210 / radius 8）
            .child(
                div()
                    .id("cmdk-entry")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(30.0))
                    .px(px(12.0))
                    .min_w(px(210.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .bg(rgba(theme.surface_1))
                    .text_color(hsla(theme.text_muted))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(|d| {
                        d.border_color(hsla(theme.border_heavy))
                            .text_color(hsla(theme.text_tertiary))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_command_palette(window, cx);
                    }))
                    .child(
                        svg()
                            .path(ui_icon::SEARCH)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child("ペイン・コマンド検索")
                    .child(div().flex_grow(1.0))
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
                            .child("⌘K"),
                    ),
            )
            // 通知ベル + 未読バッジ（カンプ: 30×30 / バッジ 14px red）
            .child(
                div()
                    .id("attention-bell")
                    .relative()
                    .w(px(30.0))
                    .h(px(30.0))
                    .ml(px(4.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel_visible = !this.panel_visible;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(ui_icon::BELL)
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(theme.text_tertiary)),
                    )
                    .when(attention > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(2.0))
                                .right(px(2.0))
                                .min_w(px(14.0))
                                .h(px(14.0))
                                .px(px(3.0))
                                .rounded(px(7.0))
                                .bg(hsla(theme.red))
                                .border_2()
                                .border_color(rgba(theme.mantle))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.crust))
                                .child(SharedString::from(attention.to_string())),
                        )
                    }),
            )
            // テーマ切替（カンプ: 太陽アイコン。ライト時は月。Issue #217）
            .child(
                div()
                    .id("theme-toggle")
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_theme(cx);
                    }))
                    .child(
                        svg()
                            .path(match theme.mode {
                                tako_core::theme::ThemeMode::Dark => ui_icon::SUN,
                                tako_core::theme::ThemeMode::Light => ui_icon::MOON,
                            })
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(theme.text_muted)),
                    ),
            )
    }
}

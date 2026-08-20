//! タブバー — Claude Design カンプ準拠のピル型タブ（Issue #217）
//!
//! カンプ: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html` の
//! tab-bar セクション。高さ 44px / ピル型タブ（h30・radius 8）/ ペイン状態
//! ミニインジケータ / fail 数表示 / ⌘K 検索エントリ / 通知ベル + バッジ /
//! テーマ切替ボタン。traffic lights はタイトルバー統合（native）で同居する。
//!
//! オーバーフロー対応（Issue #208）: タブ数に応じてラベルを縮小 + GPUI
//! ScrollHandle で横スクロール + アクティブタブの自動スクロールイン。
//!
//! # ヒットテストの不変条件（Issue #576。Windows でボタンが死ぬ）
//!
//! タブバー根 div は `window_control_area(WindowControlArea::Drag)` を張っている（#312）。
//! GPUI の `Window::hit_test` は hitbox を手前から走査して `HitboxBehavior::BlockMouse`
//! （= `occlude()`）で `break` するため、**子が occlude していないと祖先（= 根 div）の
//! hitbox まで hit test に積まれ**、`on_hit_test_window_control` が Drag を返す。
//! Windows の `WM_NCHITTEST` はこれを `HTCAPTION` に変換するので、子ボタンの上でも
//! mouse down が `DefWindowProc` のウィンドウ移動モーダルループに食われ、mouse up が
//! アプリに届かず click が成立しない（macOS は `on_hit_test_window_control` が空実装の
//! ため同じコードでも壊れない = Windows 固有）。
//!
//! したがって **タブバー上の対話要素は必ず `.occlude()` すること**。
//! 逆に `tab-scroll-area` には付けない —— `flex_1` で空き領域の大半を占めるため、
//! 付けるとタブバー空き領域でのウィンドウドラッグ移動（#312）が死ぬ。
//!
//! # ウィンドウコントロール（Issue #584 → #657 でこの行から移設）
//!
//! Windows は `hide_title_bar` でネイティブのキャプションボタンが生成されないため
//! 自前で描くが、**置き場所は一段上の in-window メニューバー行**（`menu_bar.rs`）。
//! VSCode と同じく「メニュー群 … ウィンドウコントロール」を 1 行に収める形にした。
//! この行に残る macOS 専用の要素は左端の `TRAFFIC_LIGHTS_SPACER`（native traffic
//! lights が載る余白）だけ。

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, DragMoveEvent,
    FontWeight, SharedString, WindowControlArea,
};
use tako_core::{CommandState, TitleSource};

use super::*;
use crate::file_icons::ui_icon;

/// traffic lights（12px × 3 + gap 8px × 2 = 52px）+ 右余白 16px。
/// native traffic lights を持つのは macOS だけなので、他プラットフォームでは
/// 意味のない左端余白になる（#584）。0 にして左端からタブを並べる
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_SPACER: f32 = 68.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHTS_SPACER: f32 = 0.0;

/// 1 タブのラベル込みの参考幅（px）。ラベル truncate 上限を決めるために使う概算値。
/// 実測: dot(7) + gap(8) + pl(10) + label + pr(11) + gap(3)。
/// ラベル 1 文字あたり約 7px（12.5px フォントの平均グリフ幅）
const TAB_CHROME_PX: f32 = 42.0;
const CHAR_WIDTH_PX: f32 = 7.0;
/// タブラベルの最大文字数（通常時）
const LABEL_MAX_CHARS: usize = 24;
/// タブラベルの最小文字数（縮小限界）
const LABEL_MIN_CHARS: usize = 6;
/// 右端コントロール群の概算幅
/// （⌘K(210+px) + bell(30) + ui-mode(30) + theme(30) + gap + margin。#694 で +30）
const RIGHT_CONTROLS_PX: f32 = 330.0;

/// 1 行のヒント表示（#694 のツールチップ用の最小ビュー）。
/// GPUI のツールチップは `AnyView` を要求するので、ドラッグゴースト（`DragGhost`）と
/// 同じ「小さな Render 実装」パターンで用意する
pub(crate) struct HintTooltip {
    label: String,
    theme: Theme,
}

impl HintTooltip {
    pub(crate) fn new(label: String, theme: Theme) -> Self {
        Self { label, theme }
    }
}

impl Render for HintTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgba(self.theme.surface_2))
            .border_1()
            .border_color(hsla(self.theme.border_default))
            .text_size(px(11.0))
            .text_color(hsla(self.theme.foreground))
            .child(SharedString::from(self.label.clone()))
    }
}

impl TakoApp {
    /// タブ数と利用可能幅からラベルの truncate 上限文字数を決定する
    fn tab_label_max_chars(&self, tab_count: usize, window: &Window) -> usize {
        if tab_count == 0 {
            return LABEL_MAX_CHARS;
        }
        let vw = f32::from(window.viewport_size().width);
        // ウィンドウコントロールは一段上のメニューバー行へ移った（#657）ので、
        // この行の右端で場所を取るのは ⌘K / ベル / テーマだけ
        let available = vw - TRAFFIC_LIGHTS_SPACER - RIGHT_CONTROLS_PX - 40.0;
        let per_tab = available / tab_count as f32;
        let label_px = (per_tab - TAB_CHROME_PX).max(0.0);
        let chars = (label_px / CHAR_WIDTH_PX) as usize;
        chars.clamp(LABEL_MIN_CHARS, LABEL_MAX_CHARS)
    }

    /// アクティブタブが表示領域に入るよう ScrollHandle を更新する。
    /// タブ切替を行うすべての経路（クリック・⌘数字・CLI/MCP）から呼ぶ。
    /// scroll_to_item は子要素インデックスで動くため、タブバーの表示と同じ
    /// 全タブの並びで位置を計算する（#380: タブバーは全ウィンドウ共有）
    pub(crate) fn scroll_active_tab_into_view(&self) {
        let active = self.workspace.active_tab_id();
        if let Some(idx) = self.workspace.tabs().iter().position(|t| t.id() == active) {
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
        // タブバーは全ウィンドウ共有（#380）: どのウィンドウにも全タブを描き、
        // 「このウィンドウの表示タブ」だけをアクティブ表示にする。
        // 他ウィンドウで表示中のタブは W バッジで区別する
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

        let tabs: Vec<_> = self
            .workspace
            .tabs()
            .iter()
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
                // 他ウィンドウで表示中のタブに出す区別バッジ（#380。W<番号>）
                let shown_in = self
                    .workspace
                    .windows()
                    .iter()
                    .find(|w| w.id() != viewport && w.active_tab() == id)
                    .map(|w| w.id().as_u64());
                // 自動命名の直後だけ出す「この名前を固定」の印（#552 案 4）
                let pin_hint =
                    tab.title_source() == TitleSource::Auto && self.auto_title_hint_active(id);
                (id, label, agg, pane_states, fails, shown_in, pin_hint)
            })
            .collect();
        let attention: usize = tabs.iter().map(|(_, _, _, _, fails, _, _)| fails).sum();
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
        let dragging_tab_id = self.dragging_tab;

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
            // 上の on_mouse_up は hitbox の hover ゲート付きなので、occlude した子
            //（タブピル・各ボタン。#576）の上で離すと発火しない。「mouse up は必ず
            // 押下フラグを解除する」不変条件を保つため out 側でも同じ解除を行う。
            // これが無いと tab_mouse_down が立ちっぱなしになり、タブをクリックした後の
            // 空き領域ドラッグでウィンドウ移動（#312 / #308）が効かなくなる
            .on_mouse_up_out(
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
            // native traffic lights の載る領域（macOS のみ。#584）
            .when(TRAFFIC_LIGHTS_SPACER > 0.0, |d| {
                d.child(div().w(px(TRAFFIC_LIGHTS_SPACER)).h_full().flex_none())
            })
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
                    .children(tabs.into_iter().map(
                        |(id, label, agg, pane_states, fails, shown_in, pin_hint)| {
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
                                        el.opacity(1.0 - 0.65 * (std::f32::consts::PI * t).sin())
                                    },
                                )
                                .into_any_element()
                            } else {
                                dot.into_any_element()
                            };

                            let truncated = truncate(&label, label_max);

                            // タブ D&D 並べ替えの挿入インジケータ（#371）
                            let show_indicator = is_tab_dragging && tab_reorder == Some(Some(id));
                            let is_drag_source = is_tab_dragging && dragging_tab_id == Some(id);

                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .flex_shrink_0()
                                .when(show_indicator, |d| {
                                    d.child(
                                        div()
                                            .w(px(3.0))
                                            .h(px(22.0))
                                            .flex_none()
                                            .rounded(px(1.5))
                                            .bg(hsla(theme.accent))
                                            .shadow(vec![BoxShadow {
                                                color: hsla_alpha(theme.accent, 0.5),
                                                offset: point(px(0.), px(0.)),
                                                blur_radius: px(4.0),
                                                spread_radius: px(0.),
                                                inset: false,
                                            }]),
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
                                        // 根 div の Drag ヒットテストに勝たせる（#576）
                                        .occlude()
                                        .when(is_drag_source, |d| {
                                            d.opacity(0.4)
                                                .border_1()
                                                .border_color(hsla(theme.border_subtle))
                                                .border_dashed()
                                        })
                                        .when(is_active && !is_drag_source, |d| {
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
                                        .when(!is_active && !is_drag_source, |d| {
                                            d.hover(|d| d.bg(rgba(theme.surface_hover)))
                                        })
                                        .when(is_pane_dragging && tab_drop == Some(Some(id)), |d| {
                                            d.bg(rgba_alpha(theme.accent, 0.15))
                                                .border_2()
                                                .border_color(hsla(theme.accent))
                                        })
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
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // 共有タブバー（#380）: クリックしたウィンドウへ
                                            // 表示を移す（他ウィンドウ所属なら奪取）
                                            this.select_tab_in_viewport(id, window, cx);
                                        }))
                                        .on_drag(
                                            TabDrag { tab: id },
                                            self.drag_ghost_builder_with_tab(
                                                DragKind::Tab,
                                                truncated.clone(),
                                                Some(id),
                                                cx,
                                            ),
                                        )
                                        .on_drag_move::<TabDrag>(cx.listener(
                                            move |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                                                // GPUI の on_drag_move は capture フェーズで
                                                // 全登録要素に発火するため、自身の bounds 内か
                                                // を明示チェックする（#413）
                                                if !e.bounds.contains(&e.event.position) {
                                                    return;
                                                }
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
                                        // 自動命名の直後だけ出る「この名前を固定」の印
                                        // （#552 案 4）。クリックでこの名前が手動名として
                                        // 固定され、以後 自動リネームに書き換えられなくなる。
                                        // 時間（PIN_HINT_TTL）が経てば静かに消える
                                        .when(pin_hint, |d| {
                                            d.child(
                                                div()
                                                    .id(("tab-pin-title", id.as_u64()))
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
                                                        move |this, _: &gpui::ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.pin_auto_tab_title(id, cx);
                                                        },
                                                    ))
                                                    .child(
                                                        svg()
                                                            .path(ui_icon::PIN)
                                                            .w(px(11.0))
                                                            .h(px(11.0))
                                                            .text_color(hsla(theme.accent)),
                                                    ),
                                            )
                                        })
                                        // 他ウィンドウで表示中の区別バッジ（#380。
                                        // クリックすればこのウィンドウへ表示が移る）
                                        .when_some(shown_in, |d, win| {
                                            d.child(
                                                div()
                                                    .flex_none()
                                                    .px(px(4.0))
                                                    .h(px(15.0))
                                                    .flex()
                                                    .items_center()
                                                    .rounded(px(4.0))
                                                    .border_1()
                                                    .border_color(hsla(theme.border_subtle))
                                                    .text_size(px(9.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(hsla(theme.text_muted))
                                                    .child(SharedString::from(format!("W{win}"))),
                                            )
                                        })
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
                                                    // 根 div の Drag ヒットテストに勝たせる（#576）
                                                    .occlude()
                                                    .text_color(hsla(theme.text_muted))
                                                    .hover(|d| {
                                                        d.bg(rgba(theme.surface_highlight))
                                                            .text_color(hsla(theme.foreground))
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.background_tab(id, cx);
                                                    }))
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
                                                // 根 div の Drag ヒットテストに勝たせる（#576）
                                                .occlude()
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
                        },
                    ))
                    // 末尾の挿入インジケータ（タブ D&D 並べ替え: 末尾移動。#371）
                    .when(is_tab_dragging && tab_reorder == Some(None), |d| {
                        d.child(
                            div()
                                .w(px(3.0))
                                .h(px(22.0))
                                .flex_none()
                                .rounded(px(1.5))
                                .bg(hsla(theme.accent))
                                .shadow(vec![BoxShadow {
                                    color: hsla_alpha(theme.accent, 0.5),
                                    offset: point(px(0.), px(0.)),
                                    blur_radius: px(4.0),
                                    spread_radius: px(0.),
                                    inset: false,
                                }]),
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
                            // 根 div の Drag ヒットテストに勝たせる（#576）
                            .occlude()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_tab_in_viewport(window, cx)
                            }))
                            .on_drag_move::<TabDrag>(cx.listener(
                                |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                                    if !e.bounds.contains(&e.event.position) {
                                        return;
                                    }
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
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
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
                    .child(crate::ui_text::palette::search_placeholder())
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
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
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
            // 表示モード切替（#694。GUI ライク ⇔ ターミナル。テーマボタンの左隣 =
            // 「アプリ全体の見た目」と同格の概念という位置づけ。現在モードのアイコンを出す）
            .child(
                div()
                    .id("ui-mode-toggle")
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
                        this.toggle_ui_mode(cx);
                    }))
                    // 新設ボタンなので何が起きるかを言葉で出す（アイコンだけでは
                    // 「表示モードが変わる」と分からない）
                    .tooltip({
                        let theme = theme.clone();
                        let gui = self.ui_mode.is_gui();
                        move |_, cx| {
                            let label = if gui {
                                crate::ui_text::ui_mode::toggle_to_terminal()
                            } else {
                                crate::ui_text::ui_mode::toggle_to_gui()
                            };
                            cx.new(|_| HintTooltip {
                                label: label.to_string(),
                                theme: theme.clone(),
                            })
                            .into()
                        }
                    })
                    .child(
                        svg()
                            .path(if self.ui_mode.is_gui() {
                                ui_icon::CHAT_BUBBLE
                            } else {
                                ui_icon::PROMPT
                            })
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(if self.ui_mode.is_gui() {
                                theme.accent
                            } else {
                                theme.text_muted
                            })),
                    ),
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
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
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

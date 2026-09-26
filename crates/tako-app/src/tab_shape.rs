//! タブ形の描画語彙（#1491）。
//!
//! # なぜ 1 か所に置くか
//!
//! たまり場ドロワー / 右パネルに出る「退避中のタブ」は、**タブバーのタブと同じ形**で
//! 見えていなければならない（ユーザー原文「タブごと復帰ボタンにするんじゃなく、
//! カードごとタブにして欲しい」）。形・色・寸法を描画箇所ごとに書くと、タブバーを
//! 触ったときに片方だけ取り残され、**同じものが 2 つの見た目になる**。
//! そこで寸法と部品をここへ集め、`tab_bar.rs` のタブピルと
//! [`TakoApp::render_shelved_tab_card`] が同じ関数を通る形にする。
//!
//! 中身（クリックで何が起きるか・D&D・並べ替えインジケータ）は置き場所ごとに違うので
//! ここには持たせない。ここが持つのは**見た目の語彙だけ**。

use gpui::{div, point, prelude::*, px, svg, BoxShadow, Context, SharedString};
use tako_core::CommandState;

use super::*;
use crate::file_icons::ui_icon;

/// タブピルの寸法（タブバーのタブ = 退避タブカードで共通）
pub(crate) const TAB_PILL_HEIGHT: f32 = 30.0;
pub(crate) const TAB_PILL_RADIUS: f32 = 8.0;
pub(crate) const TAB_PILL_FONT: f32 = 12.5;
pub(crate) const TAB_PILL_GAP: f32 = 8.0;
pub(crate) const TAB_PILL_PAD_LEFT: f32 = 10.0;
pub(crate) const TAB_PILL_PAD_RIGHT: f32 = 11.0;
/// 状態ドットの直径
pub(crate) const TAB_DOT_SIZE: f32 = 7.0;
/// タブピル右端に載るボタン 1 個ぶんの正方スロット（ー / × / ピン）
pub(crate) const TAB_SLOT_SIZE: f32 = 17.0;
pub(crate) const TAB_SLOT_RADIUS: f32 = 5.0;

/// #1489 の形（見出し + 「タブごと復帰」ボタン + ペインカード列）へ戻す A/B。
/// **env を読むのはここ 1 か所**（描画の分岐が 2 か所に増えると、片方だけ
/// legacy に取り残されて A/B が「どちらでもない形」になる）
pub(crate) fn shelved_tab_card_legacy() -> bool {
    std::env::var("TAKO_1491_LEGACY").as_deref() == Ok("1")
}

/// 状態ドットの色（`CommandState` → テーマ色）。タブバーのタブと退避タブカードで共通
pub(crate) fn tab_state_color(theme: &tako_core::Theme, state: &CommandState) -> tako_core::Rgb {
    match state {
        CommandState::Failed(_) => theme.red,
        CommandState::Running => theme.accent,
        CommandState::Idle => theme.green,
        CommandState::Unknown => theme.text_overlay,
    }
}

/// 状態ドット。`glow` はアクティブタブの発光（退避タブカードは常に false）
pub(crate) fn tab_state_dot(color: tako_core::Rgb, glow: bool) -> gpui::Div {
    div()
        .w(px(TAB_DOT_SIZE))
        .h(px(TAB_DOT_SIZE))
        .flex_none()
        .rounded_full()
        .bg(hsla(color))
        .when(glow, |d| {
            d.shadow(vec![BoxShadow {
                color: hsla_alpha(color, 0.7),
                offset: point(px(0.), px(0.)),
                blur_radius: px(6.0),
                spread_radius: px(0.),
                inset: false,
            }])
        })
}

/// タブピルの器（高さ・角丸・内余白・フォント + アクティブ時の地）。
/// 中身（ドット・ラベル・バッジ・ボタン）と操作は呼び出し側が足す
pub(crate) fn tab_pill_shell(theme: &tako_core::Theme, active: bool) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(TAB_PILL_GAP))
        .h(px(TAB_PILL_HEIGHT))
        .pl(px(TAB_PILL_PAD_LEFT))
        .pr(px(TAB_PILL_PAD_RIGHT))
        .flex_shrink_0()
        .rounded(px(TAB_PILL_RADIUS))
        .text_size(px(TAB_PILL_FONT))
        .when(active, |d| {
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
}

/// タブピルのラベル（太さだけがアクティブで変わる）
pub(crate) fn tab_pill_label(text: impl Into<SharedString>, active: bool) -> gpui::Div {
    div()
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .child(text.into())
}

/// タブピルに載る小バッジ（他ウィンドウ表示の `W<番号>` / 退避タブのペイン数）
pub(crate) fn tab_pill_badge(theme: &tako_core::Theme, text: impl Into<SharedString>) -> gpui::Div {
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
        .child(text.into())
}

/// タブピル右端のボタン 1 個ぶんの器（ー / × / ピン）
pub(crate) fn tab_pill_slot() -> gpui::Div {
    div()
        .w(px(TAB_SLOT_SIZE))
        .h(px(TAB_SLOT_SIZE))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(TAB_SLOT_RADIUS))
        .cursor_pointer()
}

/// セルフテスト（visual-test）が**合成マウスで**押すための実矩形を登録する透明カンバス。
/// ハンドラ直呼びでは #496 型（押下で自分が消えて `on_click` が発火しない）を検出できない
pub(crate) fn probe_canvas(
    probes: std::rc::Rc<std::cell::RefCell<HashMap<String, gpui::Bounds<gpui::Pixels>>>>,
    key: String,
) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, _, _| {
            probes.borrow_mut().insert(key.clone(), bounds);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// 退避タブカードの置き場所（#1491）。**見た目は同じ**で、要素 ID と実矩形の綴りだけが違う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabCardPlace {
    /// たまり場ドロワー
    Drawer,
    /// 右パネル tmux ビュー
    Panel,
}

impl TabCardPlace {
    /// 実矩形（probe）のキー接頭辞
    fn prefix(self) -> &'static str {
        match self {
            TabCardPlace::Drawer => "drawer",
            TabCardPlace::Panel => "panel",
        }
    }

    /// 要素 ID（カード本体 / × / はい / いいえ）
    fn ids(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            TabCardPlace::Drawer => (
                "drawer-shelved-tab",
                "drawer-shelved-tab-kill",
                "drawer-shelved-tab-yes",
                "drawer-shelved-tab-no",
            ),
            TabCardPlace::Panel => (
                "panel-shelved-tab-card",
                "panel-shelved-tab-kill",
                "panel-shelved-tab-yes",
                "panel-shelved-tab-no",
            ),
        }
    }
}

impl TakoApp {
    /// 退避中のタブ 1 枚を**タブ形のカード**で描く（#1491。ドロワーと右パネルで共用）。
    ///
    /// - カード本体のどこを押しても復帰する（既存の [`TakoApp::unshelve_tab_clicked`] =
    ///   tako-core の `unshelve_tab` = `Request::Foreground { tab }` と**同じ 1 経路**。
    ///   ここに別経路を作ると CLI / MCP と挙動が割れる）
    /// - 右端の × はタブバーのタブの × と同じ意味（タブごと kill）。ペインカードと同じ
    ///   2 段確認を通し、`stop_propagation` で本体クリック（復帰）と分ける
    /// - 配下ペインのサムネイルは**並べない**（カード = タブそのもの）
    pub(crate) fn render_shelved_tab_card(
        &self,
        group: &ShelvedTabGroup,
        place: TabCardPlace,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let tab_id = group.tab;
        let key = tab_id.as_u64();
        let (card_id, kill_id, yes_id, no_id) = place.ids();
        let panes = group.entries.len();
        // 状態ドットは配下ペインの集約（1 本でも failed なら red / running なら accent）。
        // タブバーのタブと同じ `CommandState::aggregate` を通す
        let agg = CommandState::aggregate(group.entries.iter().map(|e| e.state));
        let pending = self.bg_pending_kill_tab == Some(tab_id);
        let probes = self.panel_click_probe_bounds.clone();

        let mut pill = tab_pill_shell(&theme, false)
            .id((card_id, key))
            .relative()
            .bg(rgba(theme.tab_bar_background))
            .border_1()
            .border_color(if pending {
                hsla(theme.red)
            } else {
                hsla(theme.border_subtle)
            })
            .text_color(hsla(theme.tab_inactive_foreground));

        if pending {
            // 2 段確認の 1 段目（ペインカードの × と同じ文言・同じ運び）
            pill = pill
                .child(
                    div()
                        .flex_none()
                        .text_color(hsla(theme.red))
                        .child(crate::ui_text::drawer::confirm_destroy()),
                )
                .child(
                    div()
                        .id((yes_id, key))
                        .relative()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(theme.red))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .child(crate::ui_text::common::yes())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.kill_shelved_tab_clicked(tab_id, cx);
                        }))
                        .child(probe_canvas(
                            probes.clone(),
                            format!("{}-shelved-tab-yes-{key}", place.prefix()),
                        )),
                )
                .child(
                    div()
                        .id((no_id, key))
                        .relative()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.5)))
                        .child(crate::ui_text::common::no())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.bg_pending_kill_tab = None;
                            cx.notify();
                        }))
                        .child(probe_canvas(
                            probes.clone(),
                            format!("{}-shelved-tab-no-{key}", place.prefix()),
                        )),
                );
        } else {
            pill = pill
                .cursor_pointer()
                .hover(|d| d.bg(rgba(theme.surface_hover)))
                .child(tab_state_dot(tab_state_color(&theme, &agg), false))
                .child(tab_pill_label(truncate_chars(&group.title, 18), false))
                .child(tab_pill_badge(&theme, panes.to_string()))
                .child(
                    tab_pill_slot()
                        .id((kill_id, key))
                        .relative()
                        .text_color(hsla(theme.text_muted))
                        .hover(|d| {
                            d.bg(rgba_alpha(theme.red, 0.25))
                                .text_color(hsla(theme.foreground))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // 本体クリック（復帰）と分ける。これが無いと × を押した瞬間に戻る
                            cx.stop_propagation();
                            this.bg_pending_kill_tab = Some(tab_id);
                            cx.notify();
                        }))
                        // 色は svg 自身に置く（GPUI の `svg()` は親の `text_color` を
                        // 継承しないので、スロット側だけに置くと**透明な × になる**）
                        .child(
                            svg()
                                .path(ui_icon::CLOSE)
                                .w(px(11.0))
                                .h(px(11.0))
                                .text_color(hsla(theme.text_muted)),
                        )
                        .child(probe_canvas(
                            probes.clone(),
                            format!("{}-shelved-tab-kill-{key}", place.prefix()),
                        )),
                )
                // カード本体のどこを押しても復帰する（#1491 の主眼）
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.unshelve_tab_clicked(tab_id, cx);
                }));
        }

        pill.child(probe_canvas(
            probes,
            format!("{}-shelved-tab-{key}", place.prefix()),
        ))
    }
}

use gpui::{div, point, prelude::*, px, BoxShadow, Context, FontWeight, SharedString};
use tako_core::{CommandState, TitleSource};

use super::*;

impl TakoApp {
    pub(crate) fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.workspace.active_tab_id();
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
                let agg = CommandState::aggregate(
                    tab.tree()
                        .panes()
                        .iter()
                        .filter_map(|p| self.terminals.get(&p.id()))
                        .map(|s| s.command_state()),
                );
                let dot_color = match agg {
                    CommandState::Failed(_) => theme.red,
                    CommandState::Running => theme.accent,
                    CommandState::Idle => theme.green,
                    CommandState::Unknown => theme.text_overlay,
                };
                (id, label, dot_color)
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .border_b_1()
            .border_color(hsla(theme.border_subtle))
            .children(tabs.into_iter().map(|(id, label, dot_color)| {
                let is_active = id == active;
                let pane_count = self
                    .workspace
                    .tabs()
                    .iter()
                    .find(|t| t.id() == id)
                    .map(|t| t.tree().panes().len())
                    .unwrap_or(0);
                div()
                    .id(("tab", id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .h_full()
                    .px_3()
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgba(theme.tab_active_background))
                            .shadow(vec![BoxShadow {
                                color: hsla(theme.accent),
                                offset: point(px(0.), px(-2.)),
                                blur_radius: px(0.),
                                spread_radius: px(0.),
                                inset: true,
                            }])
                    })
                    .when(!is_active, |d| d.hover(|d| d.bg(rgba(theme.surface_1))))
                    .text_color(if is_active {
                        hsla(theme.tab_active_foreground)
                    } else {
                        hsla(theme.tab_inactive_foreground)
                    })
                    .text_size(px(12.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.workspace.activate_tab(id);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded_full()
                            .bg(hsla(dot_color))
                            .shadow(vec![BoxShadow {
                                color: hsla_alpha(dot_color, 0.4),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(3.0),
                                spread_radius: px(0.),
                                inset: false,
                            }]),
                    )
                    .on_drag(
                        TabDrag { tab: id },
                        self.drag_ghost_builder(DragKind::Tab, truncate(&label, 24), cx),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(truncate(&label, 24))),
                    )
                    .when(pane_count > 1, |d| {
                        d.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!("\u{00B7} {pane_count}"))),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-bg", id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.background_tab(id, cx);
                            }))
                            .child("ー"),
                    )
                    .child(
                        div()
                            .id(("tab-close", id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.red))
                            })
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab_with_confirm(id, event.modifiers().platform, cx);
                            }))
                            .child("×"),
                    )
            }))
            .child(
                div()
                    .id("tab-new")
                    .w(px(34.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(15.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .hover(|d| {
                        d.bg(rgba(theme.surface_highlight))
                            .text_color(hsla(theme.tab_active_foreground))
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                    .child("+"),
            )
            .child(div().flex_grow(1.0))
            .child(
                div()
                    .id("tab-settings")
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(14.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .hover(|d| {
                        d.bg(rgba(theme.surface_highlight))
                            .text_color(hsla(theme.tab_active_foreground))
                    })
                    .child("⚙"),
            )
    }
}

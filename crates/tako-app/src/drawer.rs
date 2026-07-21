use gpui::{div, prelude::*, px, Context, CursorStyle, SharedString};
use tako_core::{CommandState, PaneId, SplitDirection};

use super::*;

impl TakoApp {
    pub(crate) fn drop_background_pane(
        &mut self,
        target_pane: PaneId,
        drag: BackgroundPaneDrag,
        cx: &mut Context<Self>,
    ) {
        let zone = self.drop_target.take().map(|(_, z)| z);
        let direction = match zone {
            Some(DropZone::Left) => SplitDirection::Left,
            Some(DropZone::Right) | None => SplitDirection::Right,
            Some(DropZone::Up) => SplitDirection::Up,
            Some(DropZone::Down) => SplitDirection::Down,
            Some(DropZone::Center) => SplitDirection::Right,
        };
        if let Err(e) = self
            .workspace
            .unshelve_pane(drag.pane, target_pane, direction)
        {
            eprintln!("warning: バックグラウンドから復帰できない: {e}");
        }
        self.reattach_backgrounded_preview(drag.pane);
        self.drag_kind = None;
        if self.workspace.shelved_panes().is_empty() {
            self.drawer_visible = false;
        }
        cx.notify();
    }

    pub(crate) fn render_shelf_card(
        &self,
        entry: &BackgroundEntry,
        pending_kill: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let pane_id = entry.pane;
        let label = entry.label.clone();
        let state_color = match entry.state {
            CommandState::Failed(_) => Some(theme.red),
            CommandState::Running => Some(theme.accent),
            CommandState::Idle => Some(theme.yellow),
            _ => None,
        };
        let is_pending_kill = pending_kill == Some(pane_id);
        let lines = self.terminal_screen_lines(pane_id, false);

        let mut titlebar = div()
            .id(("shelf-titlebar", pane_id.as_u64()))
            .h(px(PANE_TITLE_BAR))
            .flex_none()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .bg(rgba(theme.tab_bar_background))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .cursor(CursorStyle::OpenHand)
            .on_drag(
                BackgroundPaneDrag { pane: pane_id },
                self.drag_ghost_builder(DragKind::BackgroundPane, truncate(&label, 24), cx),
            );

        if is_pending_kill {
            titlebar = titlebar
                .child(
                    div()
                        .flex_1()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_color(hsla(theme.red))
                        .child(crate::ui_text::drawer::confirm_destroy()),
                )
                .child(
                    div()
                        .id(("shelf-kill-yes", pane_id.as_u64()))
                        .cursor_pointer()
                        .text_color(hsla(theme.red))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .px_1()
                        .rounded_sm()
                        .child(crate::ui_text::common::yes())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = None;
                            if this.workspace.remove_shelved(pane_id).is_some() {
                                this.terminals.remove(&pane_id);
                                this.previews.remove(&pane_id);
                                this.preview_edits.remove(&pane_id);
                                this.remove_preview_image_cache(pane_id);
                                this.preview_views.remove(&pane_id);
                                this.preview_scroll_handles.remove(&pane_id);
                                this.video_players.remove(&pane_id);
                                this.remove_video_frame_cache(pane_id);
                                this.sync_preview_watches();
                                this.scroll_accum.remove(&pane_id);
                                this.scroll_ctls.remove(&pane_id);
                                this.drop_tmux_view_session(pane_id);
                                this.drop_backend_session(pane_id);
                            }
                            if this.workspace.shelved_panes().is_empty() {
                                this.drawer_visible = false;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("shelf-kill-no", pane_id.as_u64()))
                        .cursor_pointer()
                        .text_color(hsla(theme.tab_inactive_foreground))
                        .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.5)))
                        .px_1()
                        .rounded_sm()
                        .child(crate::ui_text::common::no())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = None;
                            cx.notify();
                        })),
                );
        } else {
            if let Some(color) = state_color {
                titlebar =
                    titlebar.child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color)));
            }
            titlebar = titlebar
                .child(
                    div()
                        .flex_1()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_color(hsla(theme.foreground))
                        .child(SharedString::from(truncate(&label, 40))),
                )
                .child(
                    div()
                        .id(("shelf-restore", pane_id.as_u64()))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(theme.accent))
                        .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                        .child(crate::ui_text::common::restore())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let origin = this.workspace.shelved_origin_tab(pane_id);
                            let target = origin
                                .and_then(|t| this.workspace.get_tab(t))
                                .map(|t| t.tree().focused())
                                .unwrap_or_else(|| this.workspace.active_tab().tree().focused());
                            if let Err(e) =
                                this.workspace
                                    .unshelve_pane(pane_id, target, SplitDirection::Right)
                            {
                                eprintln!("warning: バックグラウンドから復帰できない: {e}");
                            }
                            this.reattach_backgrounded_preview(pane_id);
                            if this.workspace.shelved_panes().is_empty() {
                                this.drawer_visible = false;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("shelf-kill", pane_id.as_u64()))
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
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = Some(pane_id);
                            cx.notify();
                        })),
                );
        }

        let body = if lines.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                .child(SharedString::from(truncate(&label, 24)))
        } else {
            div()
                .flex_1()
                .p(px(PANE_PADDING))
                .overflow_hidden()
                .bg(rgba(theme.background))
                .children(lines)
        };

        div()
            .id(("shelf-card", pane_id.as_u64()))
            .flex_none()
            .w(px(BG_CARD_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(if is_pending_kill {
                hsla(theme.red)
            } else {
                hsla(theme.pane_border)
            })
            .bg(rgba(theme.background))
            .child(titlebar)
            .child(body)
    }

    pub(crate) fn render_drawer(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.drawer_visible {
            return None;
        }
        let theme = self.theme.clone();
        let mut bg_groups: Vec<(String, Vec<BackgroundEntry>)> = Vec::new();
        for tab in self.workspace.tabs() {
            let entries = self.background_entries_of_tab(tab.id());
            if !entries.is_empty() {
                bg_groups.push((tab.title().to_string(), entries));
            }
        }
        for closed in self.tmux_view_closed_origin_background() {
            bg_groups.push((
                crate::ui_text::drawer::closed_tab_group(&closed.title),
                closed.entries,
            ));
        }
        let bg_total: usize = bg_groups.iter().map(|(_, e)| e.len()).sum();

        let pending_kill = self.bg_pending_kill;

        let body_h = (self.drawer_height
            - DRAWER_HEADER_HEIGHT
            - DRAWER_GROUP_HEADER
            - PANE_TITLE_BAR
            - PANE_PADDING * 2.0
            - PANE_BORDER * 2.0
            - 8.0)
            .max(40.0);
        if let Some(cell) = self.cell_size {
            let cols = ((BG_CARD_WIDTH - PANE_BORDER * 2.0 - PANE_PADDING * 2.0)
                / f32::from(cell.width))
            .floor() as usize;
            let rows = (body_h / f32::from(cell.height)).floor() as usize;
            let cw = f32::from(cell.width).round() as u16;
            let ch = f32::from(cell.height).round() as u16;
            let ids: Vec<PaneId> = bg_groups
                .iter()
                .flat_map(|(_, e)| e.iter().map(|x| x.pane))
                .collect();
            for pane_id in ids {
                if let Some(session) = self.terminals.get_mut(&pane_id) {
                    session.resize(cols, rows, cw, ch);
                }
            }
        }

        let mut cards = div()
            .id("drawer-cards")
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .px_2()
            .py_1()
            .overflow_x_scroll();

        if bg_groups.is_empty() {
            cards = cards.child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .py_1()
                    .child(crate::ui_text::drawer::empty()),
            );
        } else {
            for (gi, (title, entries)) in bg_groups.iter().enumerate() {
                let mut group = div()
                    .id(("drawer-group", gi as u64))
                    .flex()
                    .flex_col()
                    .h_full()
                    .gap_1()
                    .when(gi > 0, |d| {
                        d.pl_2()
                            .border_l_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                    })
                    .child(
                        div()
                            .h(px(DRAWER_GROUP_HEADER))
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(crate::ui_text::drawer::tab_group(
                                &truncate(title, 18),
                                entries.len(),
                            ))),
                    );
                let mut row = div().flex().flex_row().flex_1().min_h(px(0.0)).gap_2();
                for entry in entries {
                    row = row.child(self.render_shelf_card(entry, pending_kill, cx));
                }
                group = group.child(row);
                cards = cards.child(group);
            }
        }

        Some(
            div()
                .id("drawer-drop-target")
                .flex()
                .flex_col()
                .flex_none()
                .h(px(self.drawer_height))
                .w_full()
                .bg(rgba(theme.crust))
                .border_t_1()
                .border_color(hsla(theme.border_subtle))
                .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                    this.background_tab(drag.tab, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_none()
                        .h(px(DRAWER_HEADER_HEIGHT))
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.tab_inactive_foreground))
                        .child(SharedString::from(crate::ui_text::drawer::header(bg_total)))
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id("drawer-close")
                                .cursor_pointer()
                                .hover(|d| d.text_color(hsla(theme.foreground)))
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.drawer_visible = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(cards),
        )
    }
}

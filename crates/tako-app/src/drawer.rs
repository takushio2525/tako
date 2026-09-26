use gpui::{div, prelude::*, px, Context, CursorStyle, SharedString};
use tako_core::{CommandState, PaneId, SplitDirection};

use super::*;

impl TakoApp {
    /// 通知に出す退避ペインの見出し（ユーザーが見ている「どれを戻そうとしたか」）。
    /// 右パネルの `shelved_restore_clicked` が採るものと同じ（#1432）
    fn shelved_label(&self, pane_id: PaneId) -> Option<String> {
        self.workspace
            .background_pane(pane_id)
            .and_then(|(p, _, _)| p.title())
            .map(str::to_string)
    }

    /// ドロワーのカードの「復元」ボタン（#1432）。
    ///
    /// 中身を `render` のクロージャから名前付きにしてあるのは #1417 / #1422 と同じ理由
    /// （合成マウスイベントは GPUI へ届かないので、セルフテストが押した経路そのものを
    /// 叩ける名前が要る）。右パネルの同名ボタン（`shelved_restore_clicked`）とは
    /// **プレビュー監視の再開（`reattach_backgrounded_preview`）の有無**が違うので
    /// 1 本に畳んでいない（畳むと右パネル側の挙動が変わる = 別 Issue）
    pub(crate) fn shelf_restore_clicked(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let origin = self.workspace.shelved_origin_tab(pane_id);
        let target = origin
            .and_then(|t| self.workspace.get_tab(t))
            .map(|t| t.tree().focused())
            .unwrap_or_else(|| self.workspace.active_tab().tree().focused());
        let label = self.shelved_label(pane_id);
        if let Err(e) = self
            .workspace
            .unshelve_pane(pane_id, target, SplitDirection::Right)
        {
            // #1432: 以前は `eprintln!` 止まりで「押しても無言」だった
            self.notify_ui_op_failed(
                crate::sidebar::NoticeArea::Drawer,
                crate::sidebar::NoticeArm::Issue1432,
                crate::ui_text::panel::op_unshelve_pane(),
                label.as_deref(),
                &e.to_string(),
            );
        }
        self.reattach_backgrounded_preview(pane_id);
        if self.workspace.shelved_panes().is_empty() && self.workspace.shelved_tabs().is_empty() {
            self.drawer_visible = false;
        }
        cx.notify();
    }

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
        let label = self.shelved_label(drag.pane);
        if let Err(e) = self
            .workspace
            .unshelve_pane(drag.pane, target_pane, direction)
        {
            // #1432: 以前は `eprintln!` 止まりで、**掴んで落としたのに何も起きない**
            // ように見えた（GUI の stderr は誰も読めない = 境界 B8）
            self.notify_ui_op_failed(
                crate::sidebar::NoticeArea::Drawer,
                crate::sidebar::NoticeArm::Issue1432,
                crate::ui_text::panel::op_unshelve_pane(),
                label.as_deref(),
                &e.to_string(),
            );
        }
        self.reattach_backgrounded_preview(drag.pane);
        self.drag_kind = None;
        if self.workspace.shelved_panes().is_empty() && self.workspace.shelved_tabs().is_empty() {
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
                self.drag_ghost_builder(DragKind::BackgroundPane, truncate_chars(&label, 24), cx),
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
                            // 後始末の一式は `kill_shelved_pane` に集約してある（#775）。
                            // ここに並べ直すとレジストリ記録のような後付けが
                            // この経路だけ抜ける（それが #775 の症状）
                            this.kill_shelved_pane(
                                pane_id,
                                tako_core::pane_log::CloseOrigin::PaneButton,
                            );
                            if this.workspace.shelved_panes().is_empty()
                                && this.workspace.shelved_tabs().is_empty()
                            {
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
                        .child(SharedString::from(truncate_chars(&label, 40))),
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
                            this.shelf_restore_clicked(pane_id, cx);
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
                        // 印はグリフではなく描画プリミティブで描く（#1579）。色は svg 自身に
                        // 置く（`svg()` は親の `text_color` を継承しない = #1491 と同じ罠）
                        .child(
                            svg()
                                .path(crate::file_icons::ui_icon::CLOSE)
                                .w(px(10.0))
                                .h(px(10.0))
                                .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8)),
                        )
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
                .child(SharedString::from(truncate_chars(&label, 24)))
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
            .relative()
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
            // #1491: 「退避タブの配下はペインカードを並べない」を機械検証するための実矩形
            //（枚数を数えるためだけのもの。押す対象はタイトルバーの各ボタン）
            .child(crate::tab_shape::probe_canvas(
                self.panel_click_probe_bounds.clone(),
                format!("drawer-shelf-card-{}", pane_id.as_u64()),
            ))
    }

    pub(crate) fn render_drawer(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.drawer_visible {
            return None;
        }
        let theme = self.theme.clone();
        // #1491: 退避タブは**タブ形のカード 1 枚**で出す（配下ペインのサムネイルは並べない）。
        // A/B（`TAKO_1491_LEGACY=1`）のときだけ #1489 の形（見出し + 「タブごと復帰」
        // ボタン + ペインカード列）へ戻る。env を読むのは `tab_shape` の 1 か所
        let legacy1491 = crate::tab_shape::shelved_tab_card_legacy();
        let shelved_tabs = self.shelved_tab_groups();
        // (見出し, 「タブごと復帰」の対象, 中身)。退避タブ（#1487）だけが復帰対象を持つ
        let mut bg_groups: Vec<(String, Option<TabId>, Vec<BackgroundEntry>)> = Vec::new();
        if legacy1491 {
            // TAKO_1491_LEGACY_ARM 開始（#1491 前の形。番犬はここを対象外にする）
            for group in shelved_tabs.iter().cloned() {
                bg_groups.push((
                    crate::ui_text::drawer::shelved_tab_group(
                        &truncate_chars(&group.title, 14),
                        group.entries.len(),
                    ),
                    Some(group.tab),
                    group.entries,
                ));
            }
            // TAKO_1491_LEGACY_ARM 終了
        }
        for tab in self.workspace.tabs() {
            let entries = self.background_entries_of_tab(tab.id());
            if !entries.is_empty() {
                bg_groups.push((
                    crate::ui_text::drawer::tab_group(
                        &truncate_chars(tab.title(), 18),
                        entries.len(),
                    ),
                    None,
                    entries,
                ));
            }
        }
        for closed in self.tmux_view_closed_origin_background() {
            bg_groups.push((
                crate::ui_text::drawer::tab_group(
                    &truncate_chars(&crate::ui_text::drawer::closed_tab_group(&closed.title), 18),
                    closed.entries.len(),
                ),
                None,
                closed.entries,
            ));
        }
        // ドロワー見出しの件数は「バックグラウンドに居るペイン数」（#1489 ②）。
        // タブ形カードにはサムネイルが無いが、中のペインは裏で走っている
        let tab_card_panes: usize = if legacy1491 {
            0
        } else {
            shelved_tabs.iter().map(|g| g.entries.len()).sum()
        };
        let bg_total: usize =
            bg_groups.iter().map(|(_, _, e)| e.len()).sum::<usize>() + tab_card_panes;

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
            // #1491: タブ形カードにサムネイルは無いが、ホバーの一括プレビュー
            //（FR-2.16.16）は配下ペインの画面を映すのでサイズ追従は続ける
            let ids: Vec<PaneId> = bg_groups
                .iter()
                .flat_map(|(_, _, e)| e.iter().map(|x| x.pane))
                .chain(
                    shelved_tabs
                        .iter()
                        .filter(|_| !legacy1491)
                        .flat_map(|g| g.entries.iter().map(|x| x.pane)),
                )
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

        // #1491: 退避タブは**タブ形カードを縦に並べた 1 列**として、ペインカードの左に置く。
        // 「カードのどこを押しても復帰」なので、ここに復帰ボタンは要らない
        let has_tab_cards = !legacy1491 && !shelved_tabs.is_empty();
        if has_tab_cards {
            let mut column = div()
                .id("drawer-shelved-tabs")
                .flex()
                .flex_col()
                .flex_none()
                .items_start()
                .gap_1()
                .py_1()
                .h_full()
                .overflow_y_scroll();
            for group in &shelved_tabs {
                column = column.child(self.render_shelved_tab_card(
                    group,
                    crate::tab_shape::TabCardPlace::Drawer,
                    cx,
                ));
            }
            cards = cards.child(column);
        }

        if bg_groups.is_empty() && !has_tab_cards {
            cards = cards.child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .py_1()
                    .child(crate::ui_text::drawer::empty()),
            );
        } else {
            for (gi, (title, shelved_tab, entries)) in bg_groups.iter().enumerate() {
                let mut header = div()
                    .h(px(DRAWER_GROUP_HEADER))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(10.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(title.clone())),
                    );
                // TAKO_1491_LEGACY_ARM 開始（#1489 の「タブごと復帰」テキストボタン。
                // 本番の描画はタブ形カード（`render_shelved_tab_card`）に移ったので、
                // ここへ入るのは `TAKO_1491_LEGACY=1` のときだけ。番犬は対象外にする）
                if let Some(tab_id) = *shelved_tab {
                    let probe = self.panel_click_probe_bounds.clone();
                    let probe_key = format!("drawer-restore-tab-{}", tab_id.as_u64());
                    header = header.child(
                        div()
                            .id(("drawer-restore-tab", tab_id.as_u64()))
                            .relative()
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla(theme.accent))
                            .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                            .child(crate::ui_text::drawer::restore_tab())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unshelve_tab_clicked(tab_id, cx);
                            }))
                            // セルフテスト（visual-test）が**実マウスで**押すための実矩形
                            .child(
                                gpui::canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, _| {
                                        probe.borrow_mut().insert(probe_key.clone(), bounds);
                                    },
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            ),
                    );
                }
                // TAKO_1491_LEGACY_ARM 終了
                let mut group = div()
                    .id(("drawer-group", gi as u64))
                    .flex()
                    .flex_col()
                    .h_full()
                    .gap_1()
                    .when(gi > 0 || has_tab_cards, |d| {
                        d.pl_2()
                            .border_l_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                    })
                    .child(header);
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
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|d| d.text_color(hsla(theme.foreground)))
                                // 印はグリフではなく描画プリミティブで描く（#1579）
                                .child(
                                    svg()
                                        .path(crate::file_icons::ui_icon::CLOSE)
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .text_color(hsla(theme.tab_inactive_foreground)),
                                )
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

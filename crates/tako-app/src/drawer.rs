use std::rc::Rc;
use std::sync::Arc;

use gpui::{div, prelude::*, px, Context, CursorStyle, SharedString};
use tako_core::background_groups::{
    master_info, BackgroundGroup, GroupOrigin, MasterGroup, MasterInfo, MasterKey, MasterPlace,
};
use tako_core::{CommandState, PaneId, SplitDirection};

use super::*;

/// #1946 の A/B を読む 1 か所（描画と可視判定が同じ答えを使う）。
/// `TAKO_1491_LEGACY=1`（#1489 の形）も #1946 前のたまり場で描く（その形は #1946 より前のもの）
pub(crate) fn drawer_legacy1946() -> bool {
    tako_core::background_groups::legacy() || crate::tab_shape::shelved_tab_card_legacy()
}

/// カードのタイトルバーの高さ（#1946。サムネイルへ高さを回すためペインのものより低い）
const SHELF_TITLE_BAR: f32 = 22.0;
/// カードの幅の下限・上限（縦横比に合わせて決める。#1946）
const SHELF_CARD_MIN_W: f32 = 150.0;
const SHELF_CARD_MAX_W: f32 = 440.0;
/// 器の無いカード（幽霊）の幅
const SHELF_GHOST_W: f32 = 230.0;
/// プレビュー本文のサムネイルの仮想画面（列 × 行）
const PREVIEW_THUMB_COLS: usize = 96;
const PREVIEW_THUMB_LINES: usize = 44;

/// カード本体に描くもの（#1946）
enum ShelfBody {
    /// 端末の画面 / プレビューの本文（端末グリッドの行。縮尺は描くときに決める）
    Grid {
        rows: Rc<Vec<terminal_grid::RowPlan>>,
        cols: usize,
        lines: usize,
    },
    /// 画像 / 動画のサムネ / PDF の現在ページ
    Image {
        image: Arc<gpui::Image>,
        aspect: f32,
    },
    /// 器が無い（幽霊）
    Ghost,
    /// 描ける中身がまだ無い（読み込み中・読めないファイル等）
    Label,
}

/// 親 master の見出しの名前（プロファイル + タイトル。退避中ならそう添える）
fn master_label(info: &MasterInfo) -> String {
    let profile = info
        .profile
        .clone()
        .unwrap_or_else(|| format!("#{}", info.pane.as_u64()));
    let mut label = match info.title.as_deref() {
        Some(title) if !title.is_empty() && title != profile => format!("{profile} · {title}"),
        _ => profile,
    };
    if info.place == MasterPlace::Background {
        label = format!("{label} · {}", crate::ui_text::drawer::master_shelved());
    }
    truncate_chars(&label, 40)
}

/// プレビュー本文を端末の行の形へ写す（#1946。コード / Markdown の頭から仮想画面ぶん）
fn preview_thumb_lines(
    content: &crate::preview::PreviewContent,
    theme: &tako_core::Theme,
) -> Vec<tako_core::screen::ScreenLine> {
    use crate::preview::{MdBlockKind, MdSpan, PreviewContent};
    use tako_core::background_groups::{text_line, wrap_cols};
    let cols = PREVIEW_THUMB_COLS;
    let fg = theme.foreground;
    let muted = theme.tab_inactive_foreground;
    let mut out: Vec<tako_core::screen::ScreenLine> = Vec::new();
    let plain = |spans: &[MdSpan]| spans.iter().map(|s| s.text.as_str()).collect::<String>();
    let push_wrapped =
        |out: &mut Vec<tako_core::screen::ScreenLine>, prefix: &str, text: &str, color, bold| {
            let width = cols.saturating_sub(prefix.chars().count()).max(8);
            for (i, line) in wrap_cols(text, width).into_iter().enumerate() {
                let lead = if i == 0 {
                    prefix.to_string()
                } else {
                    " ".repeat(prefix.chars().count())
                };
                out.push(text_line(
                    &[
                        (lead.as_str(), muted, false, false),
                        (&line, color, bold, false),
                    ],
                    cols,
                ));
            }
        };
    match content {
        PreviewContent::Code(lines) => {
            for line in lines.iter().take(PREVIEW_THUMB_LINES) {
                let segs: Vec<(&str, tako_core::Rgb, bool, bool)> = line
                    .iter()
                    .map(|s| (s.text.as_str(), s.color.unwrap_or(fg), s.bold, s.italic))
                    .collect();
                out.push(text_line(&segs, cols));
            }
        }
        PreviewContent::Markdown(blocks) => {
            for block in blocks {
                if out.len() >= PREVIEW_THUMB_LINES {
                    break;
                }
                let quote = "> ".repeat(block.quote_depth);
                match &block.kind {
                    MdBlockKind::Heading { spans, .. } => {
                        push_wrapped(&mut out, &quote, &plain(spans), theme.accent, true);
                        out.push(text_line(&[], cols));
                    }
                    MdBlockKind::Paragraph { spans } => {
                        push_wrapped(&mut out, &quote, &plain(spans), fg, false);
                        out.push(text_line(&[], cols));
                    }
                    MdBlockKind::ListItem {
                        ordered,
                        task,
                        continuation,
                        spans,
                    } => {
                        let indent = "  ".repeat(block.list_depth.saturating_sub(1));
                        let marker = match (continuation, task, ordered) {
                            (true, _, _) => "  ".to_string(),
                            (false, Some(true), _) => "[x] ".to_string(),
                            (false, Some(false), _) => "[ ] ".to_string(),
                            (false, None, Some(n)) => format!("{n}. "),
                            (false, None, None) => "- ".to_string(),
                        };
                        let prefix = format!("{quote}{indent}{marker}");
                        push_wrapped(&mut out, &prefix, &plain(spans), fg, false);
                    }
                    MdBlockKind::CodeBlock { lines, .. } => {
                        for line in lines {
                            let mut segs: Vec<(&str, tako_core::Rgb, bool, bool)> =
                                vec![("  ", muted, false, false)];
                            segs.extend(line.iter().map(|s| {
                                (s.text.as_str(), s.color.unwrap_or(fg), s.bold, s.italic)
                            }));
                            out.push(text_line(&segs, cols));
                        }
                        out.push(text_line(&[], cols));
                    }
                    MdBlockKind::Table { header, rows, .. } => {
                        let join = |cells: &[crate::preview::MdCell]| {
                            cells
                                .iter()
                                .map(|c| plain(c))
                                .collect::<Vec<_>>()
                                .join(" | ")
                        };
                        out.push(text_line(&[(&join(header), fg, true, false)], cols));
                        for row in rows {
                            out.push(text_line(&[(&join(row), fg, false, false)], cols));
                        }
                        out.push(text_line(&[], cols));
                    }
                    MdBlockKind::Rule => {
                        out.push(text_line(&[(&"-".repeat(40), muted, false, false)], cols));
                    }
                }
            }
        }
        _ => {}
    }
    out.truncate(PREVIEW_THUMB_LINES);
    out
}

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

    /// たまり場（FR-2.15 / #1946）。由来タブ → 親 master の見出しでまとめ、
    /// カードにはペインの画面**全体**を縮小して描く（退避ペインを resize しない）
    pub(crate) fn render_drawer(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.drawer_visible {
            // 閉じている間の出力は配送側で間引かれ（#816）、組み直しの印が付かない
            // ことがあるので、次に開いたときは全部組み直す
            if self.shelf_drawer_was_visible {
                self.shelf_drawer_was_visible = false;
                self.shelf_thumbs.clear();
                self.shelf_thumbs_dirty.clear();
            }
            return None;
        }
        if drawer_legacy1946() {
            return Some(self.render_drawer_legacy1946(cx));
        }
        if !self.shelf_drawer_was_visible {
            self.shelf_drawer_was_visible = true;
            // #816: 開いた瞬間に「見えていない」の申し送りを落とす（タブ切り替えと同じ）。
            // 落とさないと、退避ペインの出力が再確認の間隔まで GUI へ渡らない
            for state in self.pane_delivery.values() {
                state
                    .hidden
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.drawer_renders = self.drawer_renders.saturating_add(1);
        let theme = self.theme.clone();
        let groups = tako_core::background_groups::groups(&self.workspace);
        // 退避でなくなったペインのサムネイルを捨てる（戻した・閉じた）
        let live: std::collections::HashSet<PaneId> =
            groups.iter().flat_map(|g| g.panes()).collect();
        self.shelf_thumbs.retain(|p, _| live.contains(p));
        self.shelf_thumbs_dirty.retain(|p| live.contains(p));
        let shelved_tabs = self.shelved_tab_groups();
        let bg_total: usize = groups.iter().map(BackgroundGroup::pane_count).sum();
        let pending_kill = self.bg_pending_kill;
        let body_h = self.shelf_body_height();

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

        // 退避タブ（#1487 / #1491）は**タブ形カード 1 枚**のまま（1 グループ）。
        // 「カードのどこを押しても復帰」なので、ここに復帰ボタンは要らない
        let has_tab_cards = !shelved_tabs.is_empty();
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

        let pane_groups: Vec<&BackgroundGroup> = groups
            .iter()
            .filter(|g| g.origin != GroupOrigin::ShelvedTab)
            .collect();
        if pane_groups.is_empty() && !has_tab_cards {
            cards = cards.child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .py_1()
                    .child(crate::ui_text::drawer::empty()),
            );
        }
        for (gi, group) in pane_groups.iter().enumerate() {
            let title = match group.origin {
                GroupOrigin::ClosedTab => crate::ui_text::drawer::tab_group(
                    &truncate_chars(&crate::ui_text::drawer::closed_tab_group(&group.title), 18),
                    group.pane_count(),
                ),
                _ => crate::ui_text::drawer::tab_group(
                    &truncate_chars(&group.title, 18),
                    group.pane_count(),
                ),
            };
            let header = div()
                .h(px(DRAWER_GROUP_HEADER))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(10.0))
                .text_color(hsla(theme.foreground))
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(title)),
                );
            let mut masters = div().flex().flex_row().flex_1().min_h(px(0.0)).gap_3();
            for (mi, m) in group.masters.iter().enumerate() {
                masters = masters.child(self.render_master_group(
                    group.origin_tab,
                    mi,
                    m,
                    body_h,
                    pending_kill,
                    cx,
                ));
            }
            cards = cards.child(
                div()
                    .id(("drawer-group", gi as u64))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .h_full()
                    .gap_1()
                    .when(gi > 0 || has_tab_cards, |d| {
                        d.pl_2()
                            .border_l_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                    })
                    .child(header)
                    .child(masters),
            );
        }
        Some(self.drawer_shell(bg_total, cards, cx))
    }

    /// カード本体（サムネイル）の高さ。ドロワーの高さから見出し 2 段（タブ・master）と
    /// タイトルバー・余白を引いたもの（#1946）
    pub(crate) fn shelf_body_height(&self) -> f32 {
        (self.drawer_height
            - DRAWER_HEADER_HEIGHT
            - DRAWER_GROUP_HEADER * 2.0
            - SHELF_TITLE_BAR
            - PANE_BORDER * 2.0
            // 上下の余白（py_1）・見出しの間（gap_1 × 2）・横スクロールバーの逃げ
            - 26.0)
            .max(40.0)
    }

    /// 親 master 1 人ぶんの見出しとカード列（#1946）
    fn render_master_group(
        &mut self,
        origin_tab: TabId,
        index: usize,
        group: &MasterGroup,
        body_h: f32,
        pending_kill: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme.clone();
        let count = group.panes.len();
        let (label, known) = match group.master {
            MasterKey::Pane(_) => match master_info(&self.workspace, group.master) {
                Some(info) => (
                    crate::ui_text::drawer::master_group(&master_label(&info), count),
                    true,
                ),
                None => (crate::ui_text::drawer::no_master(count), false),
            },
            MasterKey::Gone(pane) => (
                crate::ui_text::drawer::parent_gone(pane.as_u64(), count),
                false,
            ),
            MasterKey::Unknown => (crate::ui_text::drawer::no_master(count), false),
        };
        let icon_color = if known {
            theme.accent
        } else {
            theme.tab_inactive_foreground
        };
        let header = div()
            .relative()
            .h(px(DRAWER_GROUP_HEADER))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_size(px(10.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .child(
                svg()
                    .path(crate::file_icons::ui_icon::MASTER)
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(hsla_alpha(icon_color, if known { 1.0 } else { 0.5 })),
            )
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(known, |d| d.text_color(hsla(theme.foreground)))
                    .child(SharedString::from(label)),
            )
            // visual-test が見出しの並びを実矩形で読むためのもの
            .child(crate::tab_shape::probe_canvas(
                self.panel_click_probe_bounds.clone(),
                format!("drawer-master-group-{}-{}", origin_tab.as_u64(), index),
            ));
        let head = match group.master {
            MasterKey::Pane(p) => Some(p),
            _ => None,
        };
        let mut row = div().flex().flex_row().flex_1().min_h(px(0.0)).gap_2();
        for &pane in &group.panes {
            row = row.child(self.render_shelf_card_1946(
                pane,
                head == Some(pane),
                body_h,
                pending_kill,
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .gap_1()
            .child(header)
            .child(row)
    }

    /// カード本体に描くものを決める（#1946）。行の組み直しは「出力があった /
    /// プレビューの版が変わった / テーマが変わった」ときだけ
    fn shelf_body(&mut self, pane_id: PaneId) -> ShelfBody {
        let theme_key = (self.theme.foreground, self.theme.background);
        if !self.previews.contains_key(&pane_id) && self.terminals.contains_key(&pane_id) {
            let fresh = !self.shelf_thumbs_dirty.contains(&pane_id)
                && self
                    .shelf_thumbs
                    .get(&pane_id)
                    .is_some_and(|t| t.theme_key == theme_key);
            if !fresh {
                if let Some((rows, cols, lines)) = self.screen_rows_now(pane_id) {
                    self.shelf_thumbs.insert(
                        pane_id,
                        ShelfThumb {
                            rows: Rc::new(rows),
                            cols,
                            lines,
                            rev: 0,
                            theme_key,
                        },
                    );
                    self.shelf_thumb_rebuilds = self.shelf_thumb_rebuilds.saturating_add(1);
                }
                self.shelf_thumbs_dirty.remove(&pane_id);
            }
            return match self.shelf_thumbs.get(&pane_id) {
                Some(t) => ShelfBody::Grid {
                    rows: t.rows.clone(),
                    cols: t.cols,
                    lines: t.lines,
                },
                None => ShelfBody::Label,
            };
        }
        if let Some((image, aspect)) = self.preview_thumbnail_image(pane_id) {
            return ShelfBody::Image { image, aspect };
        }
        let Some(state) = self.previews.get(&pane_id) else {
            return ShelfBody::Label;
        };
        let rev = state.content_rev;
        let fresh = self
            .shelf_thumbs
            .get(&pane_id)
            .is_some_and(|t| t.rev == rev && t.theme_key == theme_key);
        if !fresh {
            let lines = preview_thumb_lines(&state.content, &self.theme);
            if lines.is_empty() {
                return ShelfBody::Label;
            }
            let fg = hsla(self.theme.foreground);
            let link = self.link_decoration();
            let rows: Vec<terminal_grid::RowPlan> = lines
                .iter()
                .map(|l| terminal_grid::plan_row(l, fg, None, link))
                .collect();
            self.shelf_thumbs.insert(
                pane_id,
                ShelfThumb {
                    rows: Rc::new(rows),
                    cols: PREVIEW_THUMB_COLS,
                    lines: PREVIEW_THUMB_LINES,
                    rev,
                    theme_key,
                },
            );
            self.shelf_thumb_rebuilds = self.shelf_thumb_rebuilds.saturating_add(1);
        }
        match self.shelf_thumbs.get(&pane_id) {
            Some(t) => ShelfBody::Grid {
                rows: t.rows.clone(),
                cols: t.cols,
                lines: t.lines,
            },
            None => ShelfBody::Label,
        }
    }

    /// カード本体の要素と、それに合わせたカードの幅（#1946）
    fn shelf_body_element(
        &self,
        pane_id: PaneId,
        body: ShelfBody,
        body_h: f32,
        label: &str,
    ) -> (f32, gpui::AnyElement) {
        let theme = &self.theme;
        let fit_w = |aspect: f32| {
            (body_h * aspect + PANE_BORDER * 2.0).clamp(SHELF_CARD_MIN_W, SHELF_CARD_MAX_W)
        };
        let frame = || {
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .bg(rgba(theme.background))
        };
        match body {
            ShelfBody::Grid { rows, cols, lines } => {
                let Some((nat_w, nat_h)) = self.grid_natural_px(pane_id, cols, lines) else {
                    return self.shelf_body_element(pane_id, ShelfBody::Label, body_h, label);
                };
                let card_w = fit_w(nat_w / nat_h);
                let inner_w = card_w - PANE_BORDER * 2.0;
                match self.scaled_grid(pane_id, rows, cols, lines, inner_w, body_h) {
                    Some(grid) => (card_w, frame().child(grid).into_any_element()),
                    None => self.shelf_body_element(pane_id, ShelfBody::Label, body_h, label),
                }
            }
            ShelfBody::Image { image, aspect } => (
                fit_w(aspect),
                frame()
                    .child(
                        gpui::img(image)
                            .object_fit(gpui::ObjectFit::Contain)
                            .size_full(),
                    )
                    .into_any_element(),
            ),
            ShelfBody::Ghost => (
                SHELF_GHOST_W,
                frame()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .bg(rgba_alpha(theme.red, 0.06))
                    .child(
                        svg()
                            .path(crate::file_icons::ui_icon::EYE_OFF)
                            .w(px(18.0))
                            .h(px(18.0))
                            .text_color(hsla_alpha(theme.red, 0.8)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.red))
                            .child(crate::ui_text::drawer::ghost_title()),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(crate::ui_text::drawer::ghost_hint()),
                    )
                    .into_any_element(),
            ),
            ShelfBody::Label => (
                SHELF_CARD_MIN_W,
                frame()
                    .text_size(px(11.0))
                    .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                    .child(SharedString::from(truncate_chars(label, 24)))
                    .into_any_element(),
            ),
        }
    }

    /// たまり場のカード（#1946）。画面全体の縮小・master の印・器の無い幽霊の見た目
    fn render_shelf_card_1946(
        &mut self,
        pane_id: PaneId,
        is_master: bool,
        body_h: f32,
        pending_kill: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let vessel = tako_control::dispatch::background_vessel(self, pane_id);
        let ghost = vessel.is_missing();
        let label = self
            .workspace
            .shelved(pane_id)
            .map(|p| self.background_label(p))
            .unwrap_or_default();
        let state = self.background_state(pane_id);
        let body = if ghost {
            ShelfBody::Ghost
        } else {
            self.shelf_body(pane_id)
        };
        let (card_w, body_el) = self.shelf_body_element(pane_id, body, body_h, &label);
        let is_pending_kill = pending_kill == Some(pane_id);
        let probes = self.panel_click_probe_bounds.clone();
        let key = pane_id.as_u64();

        let mut titlebar = div()
            .id(("shelf-titlebar", key))
            .h(px(SHELF_TITLE_BAR))
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
                        .id(("shelf-kill-yes", key))
                        .relative()
                        .cursor_pointer()
                        .text_color(hsla(theme.red))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .px_1()
                        .rounded_sm()
                        .child(crate::ui_text::common::yes())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = None;
                            // 後始末の一式は `kill_shelved_pane` に集約してある（#775）。
                            // 器の無い幽霊の扱い（#1946）もそこが決める
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
                        }))
                        .child(crate::tab_shape::probe_canvas(
                            probes.clone(),
                            format!("drawer-shelf-kill-yes-{key}"),
                        )),
                )
                .child(
                    div()
                        .id(("shelf-kill-no", key))
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
            let state_color = match state {
                CommandState::Failed(_) => Some(theme.red),
                CommandState::Running => Some(theme.accent),
                CommandState::Idle => Some(theme.yellow),
                _ => None,
            };
            if let Some(color) = state_color.filter(|_| !ghost) {
                titlebar = titlebar.child(
                    div()
                        .flex_none()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(hsla(color)),
                );
            }
            if is_master {
                titlebar = titlebar.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .px_1()
                        .rounded_sm()
                        .bg(rgba_alpha(theme.accent, 0.18))
                        .text_size(px(9.0))
                        .text_color(hsla(theme.accent))
                        .child(
                            svg()
                                .path(crate::file_icons::ui_icon::MASTER)
                                .w(px(9.0))
                                .h(px(9.0))
                                .text_color(hsla(theme.accent)),
                        )
                        .child(crate::ui_text::drawer::master_badge()),
                );
            }
            titlebar = titlebar.child(
                div()
                    .flex_1()
                    .overflow_x_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(hsla(if ghost {
                        theme.tab_inactive_foreground
                    } else {
                        theme.foreground
                    }))
                    .child(SharedString::from(truncate_chars(&label, 40))),
            );
            // 幽霊は戻しても中身が無い（器が無い）ので「復元」を出さない
            if !ghost {
                titlebar = titlebar.child(
                    div()
                        .id(("shelf-restore", key))
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(theme.accent))
                        .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                        .child(crate::ui_text::common::restore())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shelf_restore_clicked(pane_id, cx);
                        })),
                );
            }
            titlebar = titlebar.child(
                div()
                    .id(("shelf-kill", key))
                    .relative()
                    .flex_none()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba_alpha(theme.red, 0.25)))
                    // 印はグリフではなく描画プリミティブで描く（#1579）
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
                    }))
                    .child(crate::tab_shape::probe_canvas(
                        probes.clone(),
                        format!("drawer-shelf-kill-{key}"),
                    )),
            );
        }

        let border = if is_pending_kill {
            hsla(theme.red)
        } else if ghost {
            hsla_alpha(theme.red, 0.7)
        } else if is_master {
            hsla_alpha(theme.accent, 0.8)
        } else {
            hsla(theme.pane_border)
        };
        // 拡大のホバー: 画面全体を読める大きさで出す（FR-2.16.13 のポップアップ）
        let hover_body = div()
            .id(("shelf-body", key))
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(body_el)
            .when(!ghost, |d| {
                d.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.hover_preview = Some(HoverPreview {
                            target: PreviewTarget::Pane(pane_id),
                            anchor: window.mouse_position(),
                        });
                    } else if matches!(
                        this.hover_preview,
                        Some(HoverPreview { target: PreviewTarget::Pane(p), .. }) if p == pane_id
                    ) {
                        this.hover_preview = None;
                    }
                    cx.notify();
                }))
            });
        div()
            .id(("shelf-card", key))
            .relative()
            .flex_none()
            .w(px(card_w))
            .h(px(SHELF_TITLE_BAR + body_h + PANE_BORDER * 2.0))
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .when(ghost && !is_pending_kill, |d| d.border_dashed())
            .border_color(border)
            .bg(rgba(theme.background))
            .child(titlebar)
            .child(hover_body)
            // 機械検証が枚数・位置を実矩形で読むためのもの（押す対象はタイトルバーの各ボタン）
            .child(crate::tab_shape::probe_canvas(
                probes,
                format!("drawer-shelf-card-{key}"),
            ))
    }

    /// #1946 前のたまり場（A/B の腕。`TAKO_1946_LEGACY=1` / `TAKO_1491_LEGACY=1`）。
    /// 退避ペインをカード寸法へ resize して行 div を切り取りで並べ、由来タブだけで分ける
    fn render_drawer_legacy1946(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
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

        self.drawer_shell(bg_total, cards, cx)
    }

    /// ドロワーの外枠（ドロップ先 + 見出し + 閉じる）。#1946 の前後で共用する
    fn drawer_shell(
        &self,
        bg_total: usize,
        cards: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
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
            .child(cards)
    }
}

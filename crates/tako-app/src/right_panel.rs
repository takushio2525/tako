use gpui::{
    canvas, div, point, prelude::*, px, svg, BoxShadow, Context, CursorStyle, FontWeight,
    MouseButton, MouseDownEvent, SharedString,
};
use tako_core::{CommandState, PaneId, SplitDirection};

use super::*;
use crate::file_icons::ui_icon;

/// stage / unstage のフィードバック文言用ラベル（#487）。
/// パス空 = 全件、1 件 = そのパス、複数 = "N files"
pub(crate) fn stage_feedback_label(paths: &[String]) -> String {
    match paths.len() {
        0 => "all".to_string(),
        1 => paths[0].clone(),
        n => format!("{n} files"),
    }
}

/// orch ビューのワーカー行（#217。render_orch_view で収集）
struct OrchWorker {
    pane: PaneId,
    name: String,
    subtitle: String,
    state: CommandState,
}

/// orch ビューのオーケストレーターカード（#217）
struct OrchCard {
    pane: PaneId,
    name: String,
    tab_title: String,
    state: CommandState,
    elapsed: Option<String>,
    ctx_percent: Option<u32>,
    workers: Vec<OrchWorker>,
}

impl TakoApp {
    /// 縦積みにし、文言が長くてもボタンがパネル右端へ見切れないようにする
    /// （flex_row 一列だと長文時にボタンごと overflow_hidden で切られる。2026-06-13 実機）。
    /// `confirm_pane` = Some ならペイン kill（dispatch Close）、None なら tmux kill
    fn render_kill_confirm(
        &self,
        id_seed: u64,
        message: String,
        confirm_pane: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = &self.theme;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pl_4()
            .w_full()
            .text_size(px(11.0))
            .text_color(hsla(theme.red))
            .child(div().w_full().child(SharedString::from(message)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(("kill-yes", id_seed))
                            .px_2()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgba_alpha(theme.red, 0.25))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if confirm_pane.is_some() {
                                    this.pane_kill_confirmed(cx);
                                } else {
                                    this.tmux_kill_confirmed(cx);
                                }
                            }))
                            .child(crate::ui_text::panel::kill_button()),
                    )
                    .child(
                        div()
                            .id(("kill-no", id_seed))
                            .px_2()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                            .text_color(hsla(theme.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if confirm_pane.is_some() {
                                    this.pending_pane_kill = None;
                                } else {
                                    this.tmux_pending_kill = None;
                                }
                                cx.notify();
                            }))
                            .child(crate::ui_text::panel::kill_cancel()),
                    ),
            )
    }

    /// 表示分類バッジ（FR-2.16.12）。前面表示中 = アクティブタブ所属、それ以外は裏で実行中。
    /// タブツリーのペイン行・バックグラウンド行で共用する
    /// attach 中の外部 tmux セッションをホストペイン配下に入れ子表示する（FR-2.16.6 一本化 /
    /// FR-2.16.9）。ホスト行の下にインデントして「セッション名 + window 一覧 + 確認つき kill」を
    /// 描く。どのペインが attach しているかはホスト行が示すので「ペイン N で attach 中」は省く
    fn render_attached_session_rows(
        &self,
        group_index: usize,
        s_index: usize,
        session: &AttachedTmuxSession,
        pending_tmux: &Option<(String, Option<u32>, Option<String>)>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = &self.theme;
        // 確認 UI の id 衝突を避ける（ペイン kill は pane id、こちらは上位ビット）
        let id_seed = (1 << 32) | ((group_index as u64) << 16) | s_index as u64;
        let kill_name = session.name.clone();
        let kill_socket = session.socket.clone();
        let mut container = div().flex().flex_col().gap_1().pl_4().child(
            div()
                .id(("tmux-att-row", id_seed))
                .group("tmux-att-row")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .overflow_hidden()
                .cursor(CursorStyle::OpenHand)
                // D&D でタブ内へ取り込み（FR-2.16.10。attach 済みでも多重 attach 可）
                .on_drag(
                    TmuxSessionDrag {
                        name: session.name.clone(),
                        socket: session.socket.clone(),
                        window: None,
                    },
                    self.drag_ghost_builder(
                        DragKind::TmuxSession,
                        format!("tmux: {}", truncate(&session.name, 24)),
                        cx,
                    ),
                )
                .child(
                    div()
                        .px_1()
                        .flex_none()
                        .rounded_sm()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.accent))
                        .bg(rgba_alpha(theme.accent, 0.15))
                        .child("⎇ tmux"),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(11.0))
                        .child(SharedString::from(truncate(&session.name, 24))),
                )
                .child(
                    div()
                        .id(("tmux-att-kill", id_seed))
                        .px_1()
                        .flex_none()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(hsla_alpha(theme.red, 0.8))
                        .opacity(0.0)
                        .group_hover("tmux-att-row", |d| d.opacity(1.0))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.tmux_pending_kill =
                                Some((kill_name.clone(), None, kill_socket.clone()));
                            cx.notify();
                        }))
                        .child("×"),
                ),
        );
        for (w_index, label) in &session.windows {
            let w_index = *w_index;
            let kill_name = session.name.clone();
            let kill_socket = session.socket.clone();
            let drag_name = session.name.clone();
            let drag_socket = session.socket.clone();
            container = container.child(
                div()
                    .id((
                        "tmux-att-window-row",
                        (id_seed << 8) | w_index as u64 | 0x8000_0000,
                    ))
                    .group("tmux-att-wrow")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl_4()
                    .text_size(px(11.0))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        TmuxSessionDrag {
                            name: drag_name,
                            socket: drag_socket,
                            window: Some(w_index),
                        },
                        self.drag_ghost_builder(
                            DragKind::TmuxSession,
                            format!("tmux: {}", truncate(label, 24)),
                            cx,
                        ),
                    )
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(truncate(label, 40))),
                    )
                    .child(
                        div()
                            .id(("tmux-att-kill-window", (id_seed << 8) | w_index as u64))
                            .px_1()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .text_color(hsla_alpha(theme.red, 0.8))
                            .opacity(0.0)
                            .group_hover("tmux-att-wrow", |d| d.opacity(1.0))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tmux_pending_kill =
                                    Some((kill_name.clone(), Some(w_index), kill_socket.clone()));
                                cx.notify();
                            }))
                            .child(
                                svg()
                                    .path(ui_icon::TRASH)
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .text_color(hsla_alpha(theme.red, 0.9)),
                            ),
                    ),
            );
        }
        // attach 済みセッションへの kill 確認（unlisted 側と同じ pending を使う）
        if let Some((pending_session, pending_window, _)) = pending_tmux {
            if *pending_session == session.name {
                let label = match pending_window {
                    Some(w) => crate::ui_text::panel::confirm_kill_window(w),
                    None => crate::ui_text::panel::confirm_kill_session(&session.name),
                };
                container = container.child(self.render_kill_confirm(id_seed, label, None, cx));
            }
        }
        container
    }

    /// バックグラウンドペインのバックグラウンド行（FR-2.15.6）。タブ枠内（タブ別分離）と
    /// 「閉じたタブ」グループで共用。バッジ + 状態ドット + ラベル + 復帰（由来タブへ戻す）。
    /// D&D でもペインエリアへ復帰できる（ドロワーと同じ BackgroundPaneDrag）
    fn render_background_row(
        &self,
        entry: &BackgroundEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        let pane_id = entry.pane;
        let state_color = match entry.state {
            CommandState::Failed(_) => Some(theme.red),
            CommandState::Running => Some(theme.accent),
            CommandState::Idle => Some(theme.yellow),
            _ => None,
        };
        let mut row = div()
            .id(("tmux-bg-row", pane_id.as_u64()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py(px(2.0))
            .rounded_sm()
            .border_1()
            .border_color(hsla(theme.border_heavy))
            .bg(rgba(tako_core::Rgb::from_hex(0x161620)))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .hover(|d| d.border_color(hsla(theme.text_overlay)))
            .cursor(CursorStyle::OpenHand)
            .on_drag(
                BackgroundPaneDrag { pane: pane_id },
                self.drag_ghost_builder(DragKind::BackgroundPane, truncate(&entry.label, 24), cx),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_faint))
                    .child("⠿"),
            );
        if let Some(color) = state_color {
            row = row.child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .flex_none()
                    .rounded_full()
                    .bg(hsla(color)),
            );
        }
        row.child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(format!(
                    "{}（BG）",
                    truncate(&entry.label, 22)
                ))),
        )
        .child(
            div()
                .id(("tmux-bg-restore", pane_id.as_u64()))
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(theme.accent))
                .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                .child("⬆")
                .on_click(cx.listener(move |this, _, _, cx| {
                    // 由来タブが生きていればそこへ、無ければアクティブタブへ戻す
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
                    if this.workspace.shelved_panes().is_empty() {
                        this.drawer_visible = false;
                    }
                    cx.notify();
                })),
        )
    }

    /// 統合 tmux ビュー（FR-2.16.6〜2.16.9。旧 tmuxview FR-2.13 + 集約センター FR-2.10 の
    /// 1 本化）。タブごとの「タブ名ラベル付き四角枠」に全ペインを入れ子表示し、行クリックで
    /// ジャンプ、ゴミ箱 → 確認 → kill（dispatch の Close）。タブ内ペインで attach 中の
    /// 外部 tmux セッションは window 一覧ごとタブ枠へ紐付け表示する（FR-2.16.9）。続けて、
    /// どのタブにも表示されていない tmux セッションを「管理外 / kill 漏れ?」に区別して
    /// 列挙する（確認つき TmuxKill）
    /// orch ビュー（#217 カンプ。master とその依存ツリー・メトリクスを俯瞰する）
    fn render_orch_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let state_color = |s: &CommandState| match s {
            CommandState::Failed(_) => theme.red,
            CommandState::Running => theme.accent,
            CommandState::Idle => theme.green,
            CommandState::Unknown => theme.text_overlay,
        };
        // 全タブから master を収集し、spawned_by チェーンでワーカーを紐付ける
        let mut cards: Vec<OrchCard> = Vec::new();
        let mut standalone: Vec<(PaneId, String, CommandState)> = Vec::new();
        for tab in self.workspace.tabs() {
            for pane in tab.tree().panes() {
                let role = pane.role().unwrap_or("");
                let is_master = role.contains("orchestrator-master")
                    || role == "master"
                    || role.starts_with("master:");
                let is_solo = role.contains("orchestrator-solo") || role.starts_with("solo");
                if is_master {
                    let session = self.terminals.get(&pane.id());
                    // spawned_by で紐づくワーカーを収集。復元後などで spawned_by が
                    // 失われた worker role ペインは、master が唯一のときだけ
                    // フォールバック紐づけする（複数 master の誤認防止 = #210 と同思想）
                    let master_count = self
                        .workspace
                        .tabs()
                        .iter()
                        .flat_map(|t| t.tree().panes())
                        .filter(|p| {
                            p.role().is_some_and(|r| {
                                r.contains("orchestrator-master")
                                    || r == "master"
                                    || r.starts_with("master:")
                            })
                        })
                        .count();
                    let workers = self
                        .workspace
                        .tabs()
                        .iter()
                        .flat_map(|t| t.tree().panes())
                        .filter(|p| {
                            p.spawned_by() == Some(pane.id())
                                || (master_count == 1
                                    && p.spawned_by().is_none()
                                    && p.role().is_some_and(|r| {
                                        r.contains("orchestrator-worker") || r.starts_with("worker")
                                    }))
                        })
                        .map(|p| {
                            let name = p
                                .role()
                                .or_else(|| p.title())
                                .unwrap_or("worker")
                                .to_string();
                            let subtitle = p
                                .title()
                                .map(str::to_string)
                                .filter(|t| *t != name)
                                .unwrap_or_default();
                            OrchWorker {
                                pane: p.id(),
                                name,
                                subtitle,
                                state: self
                                    .terminals
                                    .get(&p.id())
                                    .map(|s| s.command_state())
                                    .unwrap_or(CommandState::Unknown),
                            }
                        })
                        .collect();
                    cards.push(OrchCard {
                        pane: pane.id(),
                        name: pane
                            .title()
                            .or_else(|| pane.role())
                            .unwrap_or("master")
                            .to_string(),
                        tab_title: tab.title().to_string(),
                        state: session
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown),
                        elapsed: session
                            .and_then(|s| s.command_state_since())
                            .map(|t| crate::format_state_elapsed(t.elapsed())),
                        ctx_percent: session
                            .and_then(|s| s.agent_metrics())
                            .and_then(|m| m.ctx_percent),
                        workers,
                    });
                } else if is_solo {
                    standalone.push((
                        pane.id(),
                        pane.title()
                            .or_else(|| pane.role())
                            .unwrap_or("solo")
                            .to_string(),
                        self.terminals
                            .get(&pane.id())
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown),
                    ));
                }
            }
        }
        let total_workers: usize = cards.iter().map(|c| c.workers.len()).sum();
        let n_orch = cards.len();

        div()
            .id("panel-orch-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground))
            // ヘッダ行（カンプ: ORCHESTRATORS + N orch · N workers）
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .pt(px(12.0))
                    .px(px(14.0))
                    .pb(px(6.0))
                    .flex_none()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.text_muted))
                            .child("ORCHESTRATORS"),
                    )
                    .child(div().flex_grow(1.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(format!(
                                "{n_orch} orch \u{00B7} {total_workers} workers"
                            ))),
                    ),
            )
            .child(
                div()
                    .id("orch-scroll")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .when(cards.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(6.0))
                                .py(px(8.0))
                                .text_size(px(11.5))
                                .text_color(hsla(theme.text_muted))
                                .child(crate::ui_text::panel::orch_empty()),
                        )
                    })
                    .children(cards.into_iter().map(|card| {
                        let master_pane = card.pane;
                        let dot = state_color(&card.state);
                        div()
                            .flex_none()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(hsla(theme.border_strong))
                            .bg(rgba(theme.surface_1))
                            .mb(px(8.0))
                            .overflow_hidden()
                            // カードヘッダ（master 名 + ORCH + タブ名 + 状態）
                            .child(
                                div()
                                    .id(("orch-card-head", master_pane.as_u64()))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .px(px(11.0))
                                    .py(px(10.0))
                                    .border_b_1()
                                    .border_color(hsla(theme.border_subtle))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.jump_to_pane(master_pane, cx);
                                    }))
                                    .child(
                                        svg()
                                            .path(ui_icon::MASTER)
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .text_color(hsla(theme.accent)),
                                    )
                                    .child(
                                        div()
                                            .font_family(theme.font_family.clone())
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(hsla(theme.foreground))
                                            .child(SharedString::from(truncate(&card.name, 18))),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hsla(theme.accent))
                                            .child("ORCH"),
                                    )
                                    .child(div().flex_grow(1.0))
                                    .child(
                                        div()
                                            .font_family(theme.font_family.clone())
                                            .text_size(px(10.0))
                                            .text_color(hsla(theme.text_muted))
                                            .child(SharedString::from(truncate(
                                                &card.tab_title,
                                                12,
                                            ))),
                                    )
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(hsla(dot)),
                                    ),
                            )
                            // メトリクス行（カンプ: 稼働 / tok / cost / tasks。取れるものだけ）
                            .when(card.elapsed.is_some() || card.ctx_percent.is_some(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(14.0))
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .border_b_1()
                                        .border_color(hsla(theme.border_inner))
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.text_muted))
                                        .children(card.elapsed.clone().map(|el| {
                                            div()
                                                .flex()
                                                .flex_row()
                                                .gap(px(4.0))
                                                .child(crate::ui_text::panel::orch_uptime_label())
                                                .child(
                                                    div()
                                                        .text_color(hsla(theme.foreground))
                                                        .child(SharedString::from(el)),
                                                )
                                        }))
                                        .children(card.ctx_percent.map(|pct| {
                                            div().flex().flex_row().gap(px(4.0)).child("ctx").child(
                                                div()
                                                    .text_color(hsla(theme.foreground))
                                                    .child(SharedString::from(format!("{pct}%"))),
                                            )
                                        })),
                                )
                            })
                            // ワーカーツリー（カンプ: ツリー罫線 + 状態 + 名前 + 概要）
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .pt(px(6.0))
                                    .pb(px(10.0))
                                    .pl(px(16.0))
                                    .pr(px(10.0))
                                    .when(card.workers.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(hsla(theme.text_faint))
                                                .child(crate::ui_text::panel::orch_no_workers()),
                                        )
                                    })
                                    .children({
                                        let n = card.workers.len();
                                        let rows: Vec<(bool, OrchWorker)> = card
                                            .workers
                                            .into_iter()
                                            .enumerate()
                                            .map(|(i, w)| (i + 1 == n, w))
                                            .collect();
                                        rows.into_iter().map(|(last, w)| {
                                            let failed = matches!(w.state, CommandState::Failed(_));
                                            let wdot = state_color(&w.state);
                                            let target = w.pane;
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_stretch()
                                                // ツリー罫線（縦線 + 横枝）
                                                .child(
                                                    div()
                                                        .w(px(16.0))
                                                        .flex_none()
                                                        .relative()
                                                        .child(
                                                            div()
                                                                .absolute()
                                                                .top(px(0.0))
                                                                .left(px(0.0))
                                                                .w(px(1.0))
                                                                .h(relative(if last {
                                                                    0.5
                                                                } else {
                                                                    1.0
                                                                }))
                                                                .bg(hsla(theme.border_heavy)),
                                                        )
                                                        .child(
                                                            div()
                                                                .absolute()
                                                                .top(relative(0.5))
                                                                .left(px(0.0))
                                                                .w(px(12.0))
                                                                .h(px(1.0))
                                                                .bg(hsla(theme.border_heavy)),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .id(("orch-worker", w.pane.as_u64()))
                                                        .flex_1()
                                                        .min_w(px(0.0))
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(7.0))
                                                        .p(px(6.0))
                                                        .rounded(px(6.0))
                                                        .cursor_pointer()
                                                        .when(failed, |d| {
                                                            d.bg(rgba_alpha(theme.red, 0.06))
                                                        })
                                                        .hover(move |d| {
                                                            if failed {
                                                                d.bg(rgba_alpha(theme.red, 0.1))
                                                            } else {
                                                                d.bg(rgba(
                                                                    theme.surface_hover_strong,
                                                                ))
                                                            }
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.jump_to_pane(target, cx);
                                                            },
                                                        ))
                                                        .child(if failed {
                                                            svg()
                                                                .path(ui_icon::FAIL_X)
                                                                .w(px(9.0))
                                                                .h(px(9.0))
                                                                .text_color(hsla(theme.red))
                                                                .into_any_element()
                                                        } else {
                                                            div()
                                                                .w(px(6.0))
                                                                .h(px(6.0))
                                                                .flex_none()
                                                                .rounded_full()
                                                                .bg(hsla(wdot))
                                                                .into_any_element()
                                                        })
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .font_family(
                                                                    theme.font_family.clone(),
                                                                )
                                                                .text_size(px(11.5))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(hsla(
                                                                    theme.text_secondary,
                                                                ))
                                                                .child(SharedString::from(
                                                                    truncate(&w.name, 22),
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_grow(1.0)
                                                                .min_w(px(0.0))
                                                                .overflow_hidden()
                                                                .text_ellipsis()
                                                                .whitespace_nowrap()
                                                                .text_size(px(10.5))
                                                                .text_color(if failed {
                                                                    hsla(theme.red)
                                                                } else {
                                                                    hsla(theme.text_muted)
                                                                })
                                                                .child(SharedString::from(
                                                                    if failed {
                                                                        "failed".to_string()
                                                                    } else {
                                                                        w.subtitle.clone()
                                                                    },
                                                                )),
                                                        ),
                                                )
                                        })
                                    }),
                            )
                    }))
                    // STANDALONE セクション（カンプ: オーケストレーター配下でないエージェント）
                    .when(!standalone.is_empty(), |d| {
                        let n = standalone.len();
                        d.child(
                            div()
                                .flex_none()
                                .px(px(6.0))
                                .py(px(6.0))
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(format!("STANDALONE \u{00B7} {n}"))),
                        )
                        .children(standalone.into_iter().map(|(pane, name, state)| {
                            let dot = state_color(&state);
                            div()
                                .id(("orch-standalone", pane.as_u64()))
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(11.0))
                                .py(px(9.0))
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(hsla(theme.border_strong))
                                .bg(rgba(theme.chip_surface))
                                .mb(px(8.0))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.jump_to_pane(pane, cx);
                                }))
                                .child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(hsla(dot)),
                                )
                                .child(
                                    div()
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.text_secondary))
                                        .child(SharedString::from(truncate(&name, 24))),
                                )
                                .child(div().flex_grow(1.0))
                                .child(
                                    div()
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.0))
                                        .text_color(hsla(theme.text_faint))
                                        .child("SOLO"),
                                )
                        }))
                    }),
            )
    }

    fn render_tmux_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let groups = self.tmux_view_groups();
        let unlisted = self.tmux_unlisted_sessions();
        let pending_pane = self.pending_pane_kill;
        let pending_tmux = self.tmux_pending_kill.clone();
        let active_tab = self.workspace.active_tab_id();

        // fleet サマリ（#217 カンプ: N running / N failed / N idle）
        let (n_running, n_failed, n_idle) =
            self.terminals
                .values()
                .fold((0usize, 0usize, 0usize), |(r, f, i), s| {
                    match s.command_state() {
                        CommandState::Running => (r + 1, f, i),
                        CommandState::Failed(_) => (r, f + 1, i),
                        CommandState::Idle => (r, f, i + 1),
                        CommandState::Unknown => (r, f, i),
                    }
                });

        let mut root = div()
            .id("tmux-view")
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(8.0))
            .bg(rgba(theme.mantle))
            .text_color(hsla(theme.foreground))
            .text_size(px(12.0))
            .overflow_y_scroll()
            .child({
                // サマリ行（カンプ: ドット + 数 + 状態名）
                let stat = |count: usize, label: &'static str, color: tako_core::theme::Rgb| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(11.0))
                        .text_color(hsla(if label == "failed" && count > 0 {
                            theme.red
                        } else {
                            theme.text_tertiary
                        }))
                        .when(label == "failed" && count > 0, |d| {
                            d.font_weight(FontWeight::SEMIBOLD)
                        })
                        .child(
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(color)),
                        )
                        .child(
                            div()
                                .font_family(theme.font_family.clone())
                                .child(SharedString::from(count.to_string())),
                        )
                        .child(label)
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(4.0))
                    .pt(px(2.0))
                    .child(stat(n_running, "running", theme.accent))
                    .child(stat(n_failed, "failed", theme.red))
                    .child(stat(n_idle, "idle", theme.green))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("WORKSPACE"),
                    )
                    .child(div().flex_grow(1.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.accent)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("run"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.red)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("fail"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.green)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("idle"),
                                    ),
                            ),
                    ),
            );

        // タブ枠: タブ名ラベル付き四角枠 + 枠内に全ペインの入れ子表示（FR-2.16.6）
        for (group_index, group) in groups.into_iter().enumerate() {
            let is_active = group.tab == active_tab;
            let tab_id = group.tab;
            let is_collapsed = self.collapsed_tmux_tabs.contains(&tab_id);
            // 折りたたみ時はバックグラウンド項目（裏で実行中の行 + バックグラウンド）を隠し、前面表示中
            // （アクティブタブ）の行は残す（FR-2.16.14。Q2 = バックグラウンド行＋バックグラウンドだけ隠す）。
            // タブ内の行は surface が一律（アクティブ＝全 foreground / 非アクティブ＝全 background）
            let show_rows = is_active || !is_collapsed;
            let total_pane_count = group.rows.len();

            let tab_tree = self
                .workspace
                .tabs()
                .iter()
                .find(|t| t.id() == tab_id)
                .map(|t| t.tree());
            let tab_focused = tab_tree.map(|t| t.focused());
            // レイアウト順のペイン ID リスト（ミニマップとペインリストで番号を統一するため）
            let layout_order: Vec<PaneId> = tab_tree
                .map(|t| {
                    t.layout(tako_core::Rect::UNIT)
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect()
                })
                .unwrap_or_default();
            let has_failure = group
                .rows
                .iter()
                .any(|r| matches!(r.state, CommandState::Failed(_)));
            let fail_count = group
                .rows
                .iter()
                .filter(|r| matches!(r.state, CommandState::Failed(_)))
                .count();
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .p(px(8.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(hsla(if is_collapsed && has_failure {
                    tako_core::Rgb::from_hex(0x3a2b35)
                } else {
                    theme.border_strong
                }))
                .bg(rgba(if is_active {
                    theme.surface_1
                } else if is_collapsed && has_failure {
                    tako_core::Rgb::from_hex(0x1f1a22)
                } else if is_collapsed {
                    tako_core::Rgb::from_hex(0x1a1b27)
                } else {
                    theme.surface_0
                }))
                .when(is_active, |d| {
                    d.shadow(vec![BoxShadow {
                        color: hsla_alpha(theme.accent, 0.18),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: true,
                    }])
                })
                .child({
                    let tab_agg_color = if has_failure {
                        theme.red
                    } else if group
                        .rows
                        .iter()
                        .any(|r| matches!(r.state, CommandState::Running))
                    {
                        theme.accent
                    } else if group
                        .rows
                        .iter()
                        .any(|r| matches!(r.state, CommandState::Idle))
                    {
                        theme.green
                    } else {
                        theme.tab_inactive_foreground
                    };
                    div()
                        .id(("tmux-tab-header", tab_id.as_u64()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .when(!is_collapsed, |d| {
                            d.bg(rgba_alpha(theme.accent, 0.08))
                                .rounded(px(4.0))
                                .px(px(4.0))
                                .py(px(2.0))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_tmux_collapsed(tab_id, None);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(if is_collapsed { "▸" } else { "▾" }),
                        )
                        .child(
                            div()
                                .w(px(7.0))
                                .h(px(7.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(tab_agg_color))
                                .shadow(vec![BoxShadow {
                                    color: hsla_alpha(tab_agg_color, 0.4),
                                    offset: point(px(0.), px(0.)),
                                    blur_radius: px(3.0),
                                    spread_radius: px(0.),
                                    inset: false,
                                }]),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if is_active {
                                    hsla(theme.tab_active_foreground)
                                } else {
                                    hsla(theme.text_secondary)
                                })
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(truncate(&group.title, 28))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!("{total_pane_count}"))),
                        )
                        .when(is_active, |d| {
                            d.child(
                                div()
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.accent))
                                    .bg(rgba_alpha(theme.accent, 0.14))
                                    .child("ACTIVE"),
                            )
                        })
                        // 折りたたみ時: インラインステートチップ（各ペインの状態を小矩形で表示）
                        .when(is_collapsed && !is_active, |d| {
                            let mut chips = div().flex().flex_row().items_center().gap(px(2.0));
                            for row in &group.rows {
                                let chip_color = match row.state {
                                    CommandState::Failed(_) => theme.red,
                                    CommandState::Running => theme.accent,
                                    CommandState::Idle => theme.green,
                                    CommandState::Unknown => theme.tab_inactive_foreground,
                                };
                                chips = chips.child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(4.0))
                                        .rounded(px(1.0))
                                        .bg(hsla(chip_color)),
                                );
                            }
                            d.child(chips)
                        })
                        // 折りたたみ + fail あり: "N fail" ラベル
                        .when(is_collapsed && fail_count > 0, |d| {
                            d.child(
                                div()
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(hsla(theme.red))
                                    .child(SharedString::from(format!("{fail_count} fail"))),
                            )
                        })
                });
            // ミニレイアウトマップ（ペイン配置を小さな矩形で可視化）
            if show_rows {
                if let Some(tree) = tab_tree {
                    let focused_pane = tree.focused();
                    let layout = tree.layout(tako_core::Rect::new(0.0, 0.0, 92.0, 76.0));
                    let mut map = div()
                        .w(px(92.0))
                        .h(px(76.0))
                        .bg(rgba(theme.crust))
                        .border_1()
                        .border_color(hsla(theme.border_default))
                        .rounded(px(6.0))
                        .relative()
                        .overflow_hidden()
                        .mx_auto();
                    for (idx, (pane_id, rect)) in layout.iter().enumerate() {
                        let is_focused = *pane_id == focused_pane;
                        let pane_state = self
                            .terminals
                            .get(pane_id)
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown);
                        let cell_border_color = match pane_state {
                            CommandState::Failed(_) => theme.red,
                            CommandState::Running if is_focused => theme.accent,
                            _ if is_focused => theme.accent,
                            _ => theme.border_strong,
                        };
                        let pane_num = idx + 1;
                        let cell = div()
                            .absolute()
                            .left(px(rect.x + 1.0))
                            .top(px(rect.y + 1.0))
                            .w(px((rect.width - 2.0).max(4.0)))
                            .h(px((rect.height - 2.0).max(4.0)))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(hsla(cell_border_color))
                            .bg(rgba(theme.surface_1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_family("Monaco")
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_focused {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from(format!("{pane_num}")));
                        let cell = if is_focused || matches!(pane_state, CommandState::Failed(_)) {
                            cell.shadow(vec![BoxShadow {
                                color: hsla_alpha(cell_border_color, 0.4),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(4.0),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                        } else {
                            cell
                        };
                        map = map.child(cell);
                    }
                    card = card.child(map);
                }
            }
            // 折りたたみ時はバックグラウンド行を描かない（空にして既存ループをそのまま流す）。
            // 前面表示中（アクティブタブ）の行は残す（FR-2.16.14）
            let group_rows = if show_rows { group.rows } else { Vec::new() };
            // どの attach セッションをホスト行の下に出したか（取りこぼし防止に使う）
            let mut rendered_sessions: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            for row in group_rows {
                let pane = row.pane;
                let pane_num = layout_order
                    .iter()
                    .position(|id| *id == pane)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let pinned = self
                    .pinned_previews
                    .iter()
                    .any(|p| p.target == PreviewTarget::Pane(pane));
                let pane_role = self
                    .workspace
                    .tabs()
                    .iter()
                    .find(|t| t.id() == tab_id)
                    .and_then(|t| t.tree().get(pane))
                    .and_then(|p| p.role())
                    .unwrap_or("")
                    .to_string();
                let show_state = !matches!(row.state, CommandState::Unknown);
                let color = match row.state {
                    CommandState::Failed(_) => theme.red,
                    CommandState::Idle => theme.green,
                    CommandState::Running => theme.accent,
                    CommandState::Unknown => theme.tab_inactive_foreground,
                };
                // このペインが attach 表示している外部セッション（あれば detail に名前を出す。
                // window 一覧はホスト行の下に入れ子表示するので二重化しない。FR-2.16.6）
                let hosted: Vec<&AttachedTmuxSession> = group
                    .sessions
                    .iter()
                    .filter(|s| s.pane == pane.as_u64())
                    .collect();
                let _detail = if !row.detail_title.is_empty() {
                    truncate(&row.detail_title, 36)
                } else if !hosted.is_empty() {
                    let names: Vec<String> = hosted.iter().map(|s| truncate(&s.name, 18)).collect();
                    format!("tmux: {}", names.join(" / "))
                } else {
                    match &row.backend {
                        Some(b) => format!("tmux: {}", truncate(b, 24)),
                        None => String::new(),
                    }
                };
                let is_pane_focused = tab_focused == Some(pane);
                card = card.child(
                    div()
                        .id(("tmux-pane-row", pane.as_u64()))
                        .group("tmux-row")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .overflow_hidden()
                        .when(is_pane_focused, |d| {
                            d.bg(rgba_alpha(theme.accent, 0.1)).shadow(vec![BoxShadow {
                                color: hsla(theme.accent),
                                offset: point(px(2.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(0.),
                                inset: true,
                            }])
                        })
                        .hover(|d| d.bg(rgba_alpha(theme.tab_bar_background, 0.8)))
                        .on_click(cx.listener(move |this, _, _, cx| this.jump_to_pane(pane, cx)))
                        // バックグラウンド行はホバーで実画面プレビューを出す（FR-2.16.13）。
                        // 前面表示中（アクティブタブ）はペインエリアで見えるので対象外
                        .when(!is_active, |d| {
                            d.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                if *hovered {
                                    this.hover_preview = Some(HoverPreview {
                                        target: PreviewTarget::Pane(pane),
                                        anchor: window.mouse_position(),
                                    });
                                } else if matches!(
                                    this.hover_preview,
                                    Some(HoverPreview { target: PreviewTarget::Pane(p), .. })
                                        if p == pane
                                ) {
                                    this.hover_preview = None;
                                }
                                cx.notify();
                            }))
                        })
                        // ナンバーバッジ
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .bg(rgba_alpha(color, 0.2))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.0))
                                .font_family("Monaco")
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(color))
                                .child(SharedString::from(format!("{pane_num}"))),
                        )
                        // 状態ドット（6px + pulse glow）
                        .when(show_state, |d| {
                            d.child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(hsla(color))
                                    .shadow(vec![BoxShadow {
                                        color: hsla_alpha(color, 0.4),
                                        offset: point(px(0.), px(0.)),
                                        blur_radius: px(3.0),
                                        spread_radius: px(0.),
                                        inset: false,
                                    }]),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family("Monaco")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(12.0))
                                .child(SharedString::from(truncate(&row.label, 20))),
                        )
                        // ロールタグ
                        .when(pane_role.contains("orchestrator-master"), |d| {
                            d.child(
                                div()
                                    .text_size(px(8.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.accent))
                                    .bg(rgba_alpha(theme.accent, 0.14))
                                    .flex_none()
                                    .child("ORCH"),
                            )
                        })
                        .when(pane_role.contains("orchestrator-worker"), |d| {
                            d.child(
                                div()
                                    .text_size(px(8.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.teal))
                                    .bg(rgba_alpha(theme.teal, 0.12))
                                    .flex_none()
                                    .child("WORK"),
                            )
                        })
                        // バックグラウンド行にピン留めボタン（FR-2.16.15。ピン中は常時表示、
                        // 未ピンは行ホバー時のみ）。前面行はプレビュー対象外なので出さない
                        .when(!is_active, |d| {
                            d.child(
                                div()
                                    .id(("pane-pin", pane.as_u64()))
                                    .px_1()
                                    .flex_none()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .when(pinned, |d| d.text_color(hsla(theme.accent)))
                                    .when(!pinned, |d| {
                                        d.opacity(0.0).group_hover("tmux-row", |d| d.opacity(1.0))
                                    })
                                    .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_pin(PreviewTarget::Pane(pane), None);
                                        cx.notify();
                                    }))
                                    .child(
                                        svg()
                                            .path(ui_icon::PIN)
                                            .w(px(12.0))
                                            .h(px(12.0))
                                            .text_color(hsla(theme.text_tertiary)),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .id(("pane-kill", pane.as_u64()))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.red, 0.8))
                                .opacity(0.0)
                                .group_hover("tmux-row", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pending_pane_kill = Some(pane);
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                );
                if pending_pane == Some(pane) {
                    card = card.child(self.render_kill_confirm(
                        pane.as_u64(),
                        crate::ui_text::panel::confirm_kill_pane(pane),
                        Some(pane),
                        cx,
                    ));
                }
                // バックエンドセッションに複数 window がある場合、非アクティブ window を
                // 子行として表示する（tmux window 統合）。クリックで window 切替
                if let Some(windows) = self.backend_windows.get(&pane) {
                    for w in windows {
                        if w.active {
                            continue; // アクティブ window はペイン本体が表示
                        }
                        let win_index = w.index;
                        let win_label = format!("  ↳ {}:{}", w.index, truncate(&w.name, 16));
                        let win_pane_count = w.panes;
                        let win_pinned = self
                            .pinned_previews
                            .iter()
                            .any(|p| p.target == PreviewTarget::TmuxWindow(pane, win_index));
                        card = card.child(
                            div()
                                .id(("tmux-win-row", pane.as_u64() * 100 + win_index as u64))
                                .group("tmux-win-row")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .px_1()
                                .ml_4()
                                .rounded_sm()
                                .cursor_pointer()
                                .overflow_hidden()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .hover(|d| d.bg(rgba_alpha(theme.tab_bar_background, 0.8)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = tako_control::dispatch(
                                        this,
                                        tako_control::protocol::Request::TmuxSelectWindow {
                                            pane: Some(pane.as_u64()),
                                            window: win_index,
                                        },
                                        PaneOrigin::User,
                                    );
                                    cx.notify();
                                }))
                                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                    if *hovered {
                                        this.hover_preview = Some(HoverPreview {
                                            target: PreviewTarget::TmuxWindow(pane, win_index),
                                            anchor: window.mouse_position(),
                                        });
                                    } else if matches!(
                                        this.hover_preview,
                                        Some(HoverPreview { target: PreviewTarget::TmuxWindow(p, w), .. })
                                            if p == pane && w == win_index
                                    ) {
                                        this.hover_preview = None;
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(8.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(hsla(theme.tab_inactive_foreground)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(win_label)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .child(SharedString::from(crate::ui_text::panel::pane_count(win_pane_count))),
                                )
                                .child(
                                    div()
                                        .id(("win-pin", pane.as_u64() * 100 + win_index as u64))
                                        .px_1()
                                        .flex_none()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .when(win_pinned, |d| d.text_color(hsla(theme.accent)))
                                        .when(!win_pinned, |d| {
                                            d.opacity(0.0)
                                                .group_hover("tmux-win-row", |d| d.opacity(1.0))
                                        })
                                        .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.set_pin(
                                                PreviewTarget::TmuxWindow(pane, win_index),
                                                None,
                                            );
                                            cx.notify();
                                        }))
                                        .child(
                                        svg()
                                            .path(ui_icon::PIN)
                                            .w(px(12.0))
                                            .h(px(12.0))
                                            .text_color(hsla(theme.text_tertiary)),
                                    ),
                                ),
                        );
                    }
                }
                // ホストペイン配下に attach 中セッションを入れ子表示（FR-2.16.6 一本化）
                for (s_index, session) in group.sessions.iter().enumerate() {
                    if session.pane != pane.as_u64() {
                        continue;
                    }
                    rendered_sessions.insert(s_index);
                    card = card.child(self.render_attached_session_rows(
                        group_index,
                        s_index,
                        session,
                        &pending_tmux,
                        cx,
                    ));
                }
            }
            // ホストペインが行に出ていない attach セッションの取りこぼし防止（防御的に表示）。
            // 折りたたみ時（show_rows=false）は行ごと隠れているのでこれらも出さない
            for (s_index, session) in group.sessions.iter().enumerate() {
                if !show_rows || rendered_sessions.contains(&s_index) {
                    continue;
                }
                card = card.child(self.render_attached_session_rows(
                    group_index,
                    s_index,
                    session,
                    &pending_tmux,
                    cx,
                ));
            }
            // バックグラウンド/shelved エリア（スペック準拠: border-top 1px dashed #2b2c3e）
            if !is_collapsed && !group.backgrounded.is_empty() {
                let bg_count = group.backgrounded.len();
                let mut bg_section = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .mt(px(6.0))
                    .pt(px(6.0))
                    .border_t_1()
                    .border_color(hsla_alpha(theme.border_strong, 0.5))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(format!(
                                "BACKGROUND \u{00B7} {bg_count}"
                            ))),
                    );
                for entry in &group.backgrounded {
                    bg_section = bg_section.child(self.render_background_row(entry, cx));
                }
                card = card.child(bg_section);
            }
            root = root.child(card);
        }

        // どのタブにも表示されていない tmux セッション（FR-2.16.8 / #183。
        // 管理外 = ユーザー直起動等 / kill 漏れ? = orphan バックエンドの残骸）
        if !unlisted.is_empty() {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .mt(px(12.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.text_muted))
                            .child("DETACHED SESSIONS"),
                    )
                    .child(div().flex_grow(1.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(format!("{}", unlisted.len()))),
                    ),
            );
        }
        for (index, session) in unlisted.iter().enumerate() {
            let (badge_label, badge_color) = if session.orphan_backend {
                ("orphan", theme.red)
            } else {
                (crate::ui_text::panel::external_badge(), theme.yellow)
            };
            let kill_name = session.name.clone();
            let kill_socket = session.socket.clone();
            let open_name = session.name.clone();
            let open_socket = session.socket.clone();
            // 表示名: ロールがあればロール、なければセッション名
            let display_name = if !session.role.is_empty() {
                truncate(&session.role, 24)
            } else {
                truncate(&session.name, 24)
            };
            // メタデータ行（#183: プロセス / cwd / 最終アクティビティ）
            let mut meta_parts: Vec<String> = Vec::new();
            if !session.process.is_empty() {
                meta_parts.push(session.process.clone());
            }
            if !session.cwd.is_empty() {
                let short_cwd = session
                    .cwd
                    .rsplit('/')
                    .next()
                    .unwrap_or(&session.cwd)
                    .to_string();
                meta_parts.push(short_cwd);
            }
            if !session.last_activity_age.is_empty() {
                meta_parts.push(session.last_activity_age.clone());
            }
            let meta_line = meta_parts.join(" \u{00B7} ");

            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(8.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(hsla(if session.orphan_backend {
                    tako_core::Rgb::from_hex(0x3a2b35)
                } else {
                    theme.border_strong
                }))
                .bg(rgba(if session.orphan_backend {
                    tako_core::Rgb::from_hex(0x1f1a22)
                } else {
                    theme.surface_0
                }))
                .mb(px(6.0))
                .child(
                    div()
                        .id(("tmux-unlisted-row", index as u64))
                        .group("tmux-unlisted")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .cursor(CursorStyle::OpenHand)
                        .on_drag(
                            TmuxSessionDrag {
                                name: session.name.clone(),
                                socket: session.socket.clone(),
                                window: None,
                            },
                            self.drag_ghost_builder(
                                DragKind::TmuxSession,
                                format!("tmux: {}", truncate(&session.name, 24)),
                                cx,
                            ),
                        )
                        // バッジ
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(badge_color))
                                .bg(rgba_alpha(badge_color, 0.15))
                                .child(badge_label),
                        )
                        // 表示名
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(12.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(display_name)),
                        )
                        // attached / detached
                        .child(
                            div()
                                .text_size(px(9.5))
                                .flex_none()
                                .whitespace_nowrap()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .text_color(if session.attached {
                                    hsla(theme.accent)
                                } else {
                                    hsla(theme.text_faint)
                                })
                                .when(session.attached, |d| d.bg(rgba_alpha(theme.accent, 0.12)))
                                .child(if session.attached {
                                    "attached"
                                } else {
                                    "detached"
                                }),
                        )
                        // 復帰ボタン（#183: tako tmux open 相当をワンクリック）
                        .child(
                            div()
                                .id(("tmux-restore", index as u64))
                                .px(px(5.0))
                                .py(px(1.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .text_size(px(9.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(theme.accent))
                                .bg(rgba_alpha(theme.accent, 0.12))
                                .opacity(0.0)
                                .group_hover("tmux-unlisted", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.accent, 0.25)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let _ = tako_control::dispatch(
                                        this,
                                        tako_control::protocol::Request::TmuxOpen {
                                            socket: open_socket.clone(),
                                            session: open_name.clone(),
                                            window: None,
                                            pane: None,
                                            direction: None,
                                        },
                                        PaneOrigin::User,
                                    );
                                    cx.notify();
                                }))
                                .child(crate::ui_text::common::restore()),
                        )
                        // kill ボタン（確認つき）
                        .child(
                            div()
                                .id(("tmux-kill", index as u64))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.red, 0.8))
                                .opacity(0.0)
                                .group_hover("tmux-unlisted", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.tmux_pending_kill =
                                        Some((kill_name.clone(), None, kill_socket.clone()));
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                );
            // メタデータ行（#183: プロセス / cwd / 最終アクティビティ）
            if !meta_line.is_empty() {
                card = card.child(
                    div()
                        .pl(px(6.0))
                        .text_size(px(10.0))
                        .text_color(hsla(theme.text_faint))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(meta_line)),
                );
            }
            // window 一覧
            card = card.children(session.windows.iter().map(|(w_index, label)| {
                let w_index = *w_index;
                let kill_name = session.name.clone();
                let kill_socket = session.socket.clone();
                let drag_name = session.name.clone();
                let drag_socket = session.socket.clone();
                div()
                    .id((
                        "tmux-unlisted-wrow",
                        ((index as u64) << 16) | w_index as u64 | 0x8000_0000,
                    ))
                    .group("tmux-unlisted-wrow")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl_4()
                    .text_size(px(11.0))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        TmuxSessionDrag {
                            name: drag_name,
                            socket: drag_socket,
                            window: Some(w_index),
                        },
                        self.drag_ghost_builder(
                            DragKind::TmuxSession,
                            format!("tmux: {}", truncate(label, 24)),
                            cx,
                        ),
                    )
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(truncate(label, 40))),
                    )
                    .child(
                        div()
                            .id(("tmux-kill-window", ((index as u64) << 16) | w_index as u64))
                            .px_1()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .text_color(hsla_alpha(theme.red, 0.8))
                            .opacity(0.0)
                            .group_hover("tmux-unlisted-wrow", |d| d.opacity(1.0))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tmux_pending_kill =
                                    Some((kill_name.clone(), Some(w_index), kill_socket.clone()));
                                cx.notify();
                            }))
                            .child("×"),
                    )
            }));
            // 誤爆防止のインライン確認（FR-2.13.3 / FR-2.16.8）
            if let Some((pending_session, pending_window, _)) = &pending_tmux {
                if *pending_session == session.name {
                    let name = &session.name;
                    let label = match (pending_window, session.orphan_backend) {
                        (Some(w), _) => crate::ui_text::panel::confirm_kill_window(w),
                        (None, true) => crate::ui_text::panel::confirm_kill_leftover(name),
                        (None, false) => crate::ui_text::panel::confirm_kill_unmanaged(name),
                    };
                    card = card.child(self.render_kill_confirm(index as u64, label, None, cx));
                }
            }
            root = root.child(card);
        }

        // 由来タブが閉じたバックグラウンドペインは「タブ <名前>（閉じたタブ）」にまとめて表示する。
        // 生存タブ由来のバックグラウンドは各タブ枠内へバックグラウンド表示済み（FR-2.15.6 タブ別分離）
        let closed_origin = self.tmux_view_closed_origin_background();
        if !closed_origin.is_empty() {
            root = root.child(
                div()
                    .mt_2()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .text_size(px(11.0))
                    .child(crate::ui_text::panel::closed_tab_section()),
            );
        }
        for shelf_group in &closed_origin {
            let group_tab = shelf_group.tab;
            let group_pinned = self
                .pinned_previews
                .iter()
                .any(|p| p.target == PreviewTarget::ClosedGroup(group_tab));
            let mut card = div()
                .id(("tmux-closed-group", group_tab.as_u64()))
                .group("tmux-closed-group")
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(hsla_alpha(theme.pane_border, 0.7))
                // グループ全体をホバーで一括プレビュー（FR-2.16.16。全バックグラウンドペインを並べて出す）
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.hover_preview = Some(HoverPreview {
                            target: PreviewTarget::ClosedGroup(group_tab),
                            anchor: window.mouse_position(),
                        });
                    } else if matches!(
                        this.hover_preview,
                        Some(HoverPreview { target: PreviewTarget::ClosedGroup(t), .. })
                            if t == group_tab
                    ) {
                        this.hover_preview = None;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(
                                    crate::ui_text::panel::closed_tab_group(
                                        &truncate(&shelf_group.title, 20),
                                        shelf_group.entries.len(),
                                    ),
                                )),
                        )
                        // グループ全体をピン留め（FR-2.16.15 / FR-2.16.16）
                        .child(
                            div()
                                .id(("group-pin", group_tab.as_u64()))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .when(group_pinned, |d| d.text_color(hsla(theme.accent)))
                                .when(!group_pinned, |d| {
                                    d.opacity(0.0)
                                        .group_hover("tmux-closed-group", |d| d.opacity(1.0))
                                })
                                .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_pin(PreviewTarget::ClosedGroup(group_tab), None);
                                    cx.notify();
                                }))
                                .child(
                                    svg()
                                        .path(ui_icon::PIN)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .text_color(hsla(theme.text_tertiary)),
                                ),
                        ),
                );
            for entry in &shelf_group.entries {
                card = card.child(self.render_background_row(entry, cx));
            }
            root = root.child(card);
        }

        root
    }

    /// git ビュー（FR-3.6 git graph + FR-3.9 diff ビューア）。cwd 連動で 2 秒ポーリング更新。
    /// セクション: ブランチ → 変更ファイル → コミットグラフ → diff
    /// 折りたたみ三角 + 見出し + 右端の一括操作ボタンからなるセクションヘッダ（#487）。
    /// 三角は SVG chevron（UI 絵文字ゼロの規約に合わせる）
    #[allow(clippy::too_many_arguments)]
    fn git_section_header(
        &self,
        id: &'static str,
        collapsed: bool,
        label: String,
        bulk: Option<(&'static str, &'static str, &'static str)>,
        theme: &tako_core::Theme,
        on_bulk: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let fg = theme.tab_inactive_foreground;
        let bg_hover = theme.selection_background;
        let mut header = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .px_2()
            .py(px(3.0))
            .mt_1()
            .text_size(px(10.0))
            .text_color(hsla(fg))
            .child(
                div()
                    .id(id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_1()
                    .overflow_hidden()
                    .gap(px(2.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git_collapsed.changes = !this.git_collapsed.changes;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(if collapsed {
                                ui_icon::CHEVRON_RIGHT
                            } else {
                                ui_icon::CHEVRON_DOWN
                            })
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(hsla(fg)),
                    )
                    .child(div().text_ellipsis().child(SharedString::from(label))),
            );
        if let Some((btn_id, icon, tip)) = bulk {
            header = header.child(
                div()
                    .id(btn_id)
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(2.0))
                    .px_1()
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .text_color(hsla(fg))
                    .hover(|d| {
                        d.bg(rgba_alpha(theme.accent, 0.2))
                            .text_color(hsla(theme.accent))
                    })
                    .on_click(on_bulk)
                    .child(
                        svg()
                            .path(icon)
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(hsla(theme.accent)),
                    )
                    .child(SharedString::from(tip)),
            );
        }
        header
    }

    /// 変更ファイル 1 行（#487）。右端の +/− でそのファイルだけ stage / unstage する
    fn git_change_row(
        &self,
        id: (&'static str, usize),
        entry: &tako_core::GitStatusEntry,
        staged: bool,
        repo_root: &str,
        theme: &tako_core::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let fg = theme.tab_inactive_foreground;
        let badge = if staged {
            entry.staged_badge()
        } else {
            entry.unstaged_badge()
        };
        // バッジ色: 追加/未追跡 = 緑、変更 = 黄、削除 = 赤、リネーム = accent
        let color = match badge {
            'A' | 'U' => theme.green,
            'M' => theme.yellow,
            'D' => theme.red,
            'R' | 'C' => theme.accent,
            _ => fg,
        };
        let path = entry.path.clone();
        let repo = repo_root.to_string();
        let action_path = entry.path.clone();
        div()
            .id(id)
            .group("git-change-row")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .px_3()
            .py(px(1.0))
            .text_size(px(11.0))
            .hover(|d| d.bg(rgba_alpha(theme.selection_background, 0.25)))
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_color(hsla(color))
                    .child(badge.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .text_ellipsis()
                    .overflow_hidden()
                    .text_color(hsla(if staged {
                        theme.tab_active_foreground
                    } else {
                        fg
                    }))
                    .child(path),
            )
            .child(
                div()
                    .id(("git-row-action", id.1 + if staged { 0 } else { 10_000 }))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .px_1()
                    .h(px(14.0))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .text_size(px(9.0))
                    .text_color(hsla_alpha(fg, 0.6))
                    .hover(|d| {
                        d.bg(rgba_alpha(theme.accent, 0.25))
                            .text_color(hsla(theme.accent))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let paths = vec![action_path.clone()];
                        if staged {
                            this.git_do_unstage(repo.clone(), paths, cx);
                        } else {
                            this.git_do_stage(repo.clone(), paths, cx);
                        }
                    }))
                    .child(
                        svg()
                            .path(if staged {
                                ui_icon::MINUS
                            } else {
                                ui_icon::PLUS
                            })
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(hsla(theme.accent)),
                    )
                    // 何のボタンかは行ホバー時にだけ言葉で出す（常時は VSCode 同様アイコンのみ）
                    .child(
                        div()
                            .opacity(0.0)
                            .group_hover("git-change-row", |d| d.opacity(1.0))
                            .child(if staged {
                                crate::ui_text::panel::git_unstage_file()
                            } else {
                                crate::ui_text::panel::git_stage_file()
                            }),
                    ),
            )
    }

    fn render_git_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let data = self.git_data.clone();
        let collapsed = self.git_collapsed.clone();

        let mut root = div()
            .id("git-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgba(theme.mantle))
            .text_color(hsla(theme.tab_inactive_foreground))
            .text_size(px(11.0));

        let Some(data) = data else {
            // git パネルを開いた瞬間のデータ取得（初回は即 fetch）
            let has_cwd = self.git_cwd_for_tab();
            if self.git_data.is_none() {
                if let Some(cwd) = has_cwd.clone() {
                    cx.spawn(async move |this, cx| {
                        let data = cx
                            .background_executor()
                            .spawn(async move { fetch_git_data(&cwd, None) })
                            .await;
                        let _ = this.update(cx, |app: &mut TakoApp, cx| {
                            app.git_data = data;
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            // #487: リポジトリが無いタブで「検出中…」が永続表示され壊れて見えていたのを、
            // 検出対象が無い場合は明示メッセージにする
            return root.p_4().child(if has_cwd.is_some() {
                crate::ui_text::panel::git_detecting()
            } else {
                crate::ui_text::panel::git_not_a_repo()
            });
        };

        let accent = theme.accent;
        let fg = theme.tab_inactive_foreground;
        let fg_active = theme.tab_active_foreground;
        let bg_hover = theme.selection_background;

        // ──── リポヘッダ ────
        let repo_name = std::path::Path::new(&data.repo_root)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| data.repo_root.clone());
        root = root.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .overflow_hidden()
                .px_2()
                .py_1()
                .bg(rgba(theme.tab_bar_background))
                .child(
                    svg()
                        .path(ui_icon::GIT_BRANCH)
                        .flex_none()
                        .w(px(12.0))
                        .h(px(12.0))
                        .text_color(hsla(accent)),
                )
                .child(
                    div()
                        .ml_1()
                        .text_size(px(12.0))
                        .text_color(hsla(fg_active))
                        .child(data.branch.clone()),
                )
                .child(
                    div()
                        .ml_2()
                        .text_size(px(10.0))
                        .text_color(hsla(fg))
                        .child(repo_name),
                )
                .when(!data.upstream.is_empty(), |d| {
                    d.child(
                        div()
                            .ml_2()
                            .text_size(px(10.0))
                            .text_color(hsla(fg))
                            .text_ellipsis()
                            .child(format!("-> {}", data.upstream)),
                    )
                })
                .child(div().flex_grow(1.0))
                // 手動更新（#487。2 秒ポーリング待ちをせず即座に取り直す）
                .child(
                    div()
                        .id("git-refresh-btn")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .px_1()
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(hsla(fg))
                        .hover(|d| {
                            d.bg(rgba_alpha(bg_hover, 0.4))
                                .text_color(hsla(theme.accent))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_git(cx);
                        }))
                        .child(
                            svg()
                                .path(ui_icon::REFRESH)
                                .w(px(11.0))
                                .h(px(11.0))
                                .flex_none()
                                .text_color(hsla(fg)),
                        )
                        .child(crate::ui_text::panel::git_refresh()),
                ),
        );

        // ──── コミットメッセージ入力 + 操作ボタン（#472 / #487）────
        let commit_msg = self.git_commit_message.clone();
        let commit_cursor = self.git_commit_cursor.min(commit_msg.len());
        let commit_focused = self.git_commit_input_focused;
        let branch_name = data.branch.clone();
        let has_changes = !data.status.is_empty();
        let repo_root_str = data.repo_root.clone();
        // #487: ステージ済み / 未ステージを分類（VSCode ソース管理の 2 セクション構造）
        let staged_entries: Vec<tako_core::GitStatusEntry> = data
            .status
            .iter()
            .filter(|e| e.is_staged())
            .cloned()
            .collect();
        let unstaged_entries: Vec<tako_core::GitStatusEntry> = data
            .status
            .iter()
            .filter(|e| e.is_unstaged())
            .cloned()
            .collect();
        let staged_count = staged_entries.len();

        // フィードバック表示
        if let Some(fb) = &self.git_feedback {
            let color = if fb.is_error { theme.red } else { theme.green };
            root = root.child(
                div()
                    .px_2()
                    .py(px(3.0))
                    .text_size(px(10.0))
                    .text_color(hsla(color))
                    .bg(rgba_alpha(color, 0.1))
                    .child(SharedString::from(fb.message.clone())),
            );
        }

        // コミットメッセージ入力欄（#487: キャレット表示 + フォーカス可視化。
        // 実際の文字入力は replace_text_in_range / handle_git_commit_key が担う）
        let (msg_before, msg_after) = commit_msg.split_at(commit_cursor);
        root = root.child(
            div().px_2().py(px(4.0)).child(
                div()
                    .id("git-commit-input")
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .overflow_hidden()
                    .px_1()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(hsla_alpha(
                        if commit_focused {
                            theme.accent
                        } else {
                            theme.border_subtle
                        },
                        if commit_focused { 0.7 } else { 1.0 },
                    ))
                    .bg(rgba(theme.crust))
                    .text_size(px(11.0))
                    .text_color(hsla(fg_active))
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.git_commit_input_focused = true;
                            this.git_commit_cursor = this.git_commit_message.len();
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .when(commit_msg.is_empty() && !commit_focused, |d| {
                        d.child(
                            div()
                                .text_color(hsla(theme.text_muted))
                                .text_ellipsis()
                                .child(SharedString::from(
                                    crate::ui_text::panel::git_commit_placeholder(&branch_name),
                                )),
                        )
                    })
                    .when(!commit_msg.is_empty(), |d| {
                        d.child(div().child(SharedString::from(msg_before.to_string())))
                    })
                    .when(commit_focused, |d| {
                        d.child(
                            div()
                                .w(px(1.5))
                                .h(px(13.0))
                                .flex_none()
                                .bg(hsla(theme.accent)),
                        )
                    })
                    .when(!commit_msg.is_empty(), |d| {
                        d.child(div().child(SharedString::from(msg_after.to_string())))
                    }),
            ),
        );

        // コミット + プル / プッシュ ボタン行
        let commit_enabled = !self.git_commit_message.trim().is_empty() && has_changes;
        let btn_base = |id: &'static str, th: &tako_core::Theme, enabled: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(3.0))
                .rounded(px(4.0))
                .text_size(px(11.0))
                .cursor_pointer()
                .when(enabled, |d| {
                    d.bg(rgba_alpha(th.accent, 0.2))
                        .text_color(hsla(th.accent))
                        .hover(|d| d.bg(rgba_alpha(th.accent, 0.35)))
                })
                .when(!enabled, |d| {
                    d.bg(rgba_alpha(th.border_subtle, 0.3))
                        .text_color(hsla(th.text_muted))
                })
        };

        let commit_repo = repo_root_str.clone();
        let pull_repo = repo_root_str.clone();
        let push_repo = repo_root_str.clone();
        root = root.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .overflow_hidden()
                .gap_1()
                .px_2()
                .py(px(2.0))
                .child({
                    let btn = btn_base("git-commit-btn", &theme, commit_enabled)
                        .flex_1()
                        .overflow_hidden();
                    let btn = if commit_enabled {
                        btn.on_click(cx.listener(move |this, _, _, cx| {
                            this.git_do_commit(commit_repo.clone(), cx);
                        }))
                    } else {
                        btn
                    };
                    btn.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .child(
                                svg()
                                    .path(ui_icon::CHECK)
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .flex_none()
                                    .text_color(hsla(if commit_enabled {
                                        theme.accent
                                    } else {
                                        theme.text_muted
                                    })),
                            )
                            .child(crate::ui_text::panel::git_commit_btn()),
                    )
                })
                .child(
                    btn_base("git-pull-btn", &theme, true)
                        .flex_none()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.git_do_pull(pull_repo.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    svg()
                                        .path(ui_icon::ARROW_DOWN)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .flex_none()
                                        .text_color(hsla(theme.accent)),
                                )
                                .child("Pull"),
                        ),
                )
                .child(
                    btn_base("git-push-btn", &theme, true)
                        .flex_none()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.git_do_push(push_repo.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    svg()
                                        .path(ui_icon::ARROW_UP)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .flex_none()
                                        .text_color(hsla(theme.accent)),
                                )
                                .child("Push"),
                        ),
                ),
        );
        // #487: コミット対象がステージ済みだけかどうかを言葉で明示する
        root = root.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .h(px(14.0))
                .overflow_hidden()
                .px_2()
                .text_size(px(9.0))
                .text_color(hsla(theme.text_muted))
                .text_ellipsis()
                .child(if staged_count > 0 {
                    SharedString::from(crate::ui_text::panel::git_commit_staged_hint(staged_count))
                } else {
                    SharedString::from(crate::ui_text::panel::git_commit_all_hint())
                }),
        );

        // ──── ブランチ一覧セクション ────
        root = root.child(
            div()
                .id("git-branches-header")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .px_2()
                .py(px(3.0))
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(fg))
                .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_collapsed.branches = !this.git_collapsed.branches;
                    cx.notify();
                }))
                .child(
                    svg()
                        .path(if collapsed.branches {
                            ui_icon::CHEVRON_RIGHT
                        } else {
                            ui_icon::CHEVRON_DOWN
                        })
                        .w(px(10.0))
                        .h(px(10.0))
                        .flex_none()
                        .text_color(hsla(fg)),
                )
                .child(crate::ui_text::panel::git_branches(
                    data.branches.iter().filter(|b| !b.is_remote).count(),
                )),
        );
        if !collapsed.branches {
            for branch in &data.branches {
                if branch.is_remote {
                    continue;
                }
                let is_current = branch.is_current;
                root = root.child(
                    div()
                        .px_3()
                        .py(px(1.0))
                        .text_size(px(11.0))
                        .when(is_current, |d| d.text_color(hsla(accent)))
                        .when(!is_current, |d| d.text_color(hsla(fg)))
                        .child(format!(
                            "{}{}",
                            if is_current { "● " } else { "  " },
                            branch.name
                        )),
                );
            }
        }

        // ──── 変更ファイルセクション（#487: ステージ済み / 未ステージの 2 段構成）────
        // 折りたたみは 2 セクション共通（git_collapsed.changes）で扱う
        if data.status.is_empty() {
            root = root.child(
                div()
                    .px_2()
                    .py(px(4.0))
                    .mt_1()
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_muted))
                    .child(crate::ui_text::panel::git_no_changes()),
            );
        } else {
            let repo_for_stage_all = repo_root_str.clone();
            let repo_for_unstage_all = repo_root_str.clone();
            // ステージ済みセクション
            if staged_count > 0 {
                root = root.child(self.git_section_header(
                    "git-staged-header",
                    collapsed.changes,
                    crate::ui_text::panel::git_staged_section(staged_count),
                    Some((
                        "git-unstage-all",
                        ui_icon::MINUS,
                        crate::ui_text::panel::git_unstage_all(),
                    )),
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.git_do_unstage(repo_for_unstage_all.clone(), Vec::new(), cx);
                    }),
                    cx,
                ));
                if !collapsed.changes {
                    for (i, entry) in staged_entries.iter().enumerate() {
                        root = root.child(self.git_change_row(
                            ("git-staged-row", i),
                            entry,
                            true,
                            &repo_root_str,
                            &theme,
                            cx,
                        ));
                    }
                }
            }
            // 未ステージセクション
            if !unstaged_entries.is_empty() {
                root = root.child(self.git_section_header(
                    "git-unstaged-header",
                    collapsed.changes,
                    crate::ui_text::panel::git_unstaged_section(unstaged_entries.len()),
                    Some((
                        "git-stage-all",
                        ui_icon::PLUS,
                        crate::ui_text::panel::git_stage_all(),
                    )),
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.git_do_stage(repo_for_stage_all.clone(), Vec::new(), cx);
                    }),
                    cx,
                ));
                if !collapsed.changes {
                    for (i, entry) in unstaged_entries.iter().enumerate() {
                        root = root.child(self.git_change_row(
                            ("git-unstaged-row", i),
                            entry,
                            false,
                            &repo_root_str,
                            &theme,
                            cx,
                        ));
                    }
                }
            }
        }

        // ──── コミットグラフセクション ────
        let selected_commit = self.git_selected_commit.clone();
        root = root.child(
            div()
                .id("git-commits-header")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .px_2()
                .py(px(3.0))
                .mt_1()
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(fg))
                .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_collapsed.commits = !this.git_collapsed.commits;
                    cx.notify();
                }))
                .child(
                    svg()
                        .path(if collapsed.commits {
                            ui_icon::CHEVRON_RIGHT
                        } else {
                            ui_icon::CHEVRON_DOWN
                        })
                        .w(px(10.0))
                        .h(px(10.0))
                        .flex_none()
                        .text_color(hsla(fg)),
                )
                .child(crate::ui_text::panel::git_commits(data.commits.len())),
        );
        if !collapsed.commits {
            for (i, commit) in data.commits.iter().enumerate() {
                let hash = commit.short_hash.clone();
                let full_hash = commit.hash.clone();
                let is_selected = selected_commit.as_deref() == Some(&commit.hash);
                let has_refs = !commit.refs.is_empty();

                let mut row = div()
                    .id(("git-commit", i))
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(rgba_alpha(accent, 0.15)))
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.git_selected_commit.as_deref() == Some(&full_hash) {
                            this.git_selected_commit = None;
                        } else {
                            this.git_selected_commit = Some(full_hash.clone());
                        }
                        // 即座に diff を取得する
                        if let Some(cwd) = this.git_cwd_for_tab() {
                            let selected = this.git_selected_commit.clone();
                            cx.spawn(async move |this, cx| {
                                let data = cx
                                    .background_executor()
                                    .spawn(async move { fetch_git_data(&cwd, selected.as_deref()) })
                                    .await;
                                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                                    app.git_data = data;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    }));

                // グラフ列（canvas 描画）
                let graph_w = {
                    const LANE_W: f32 = 14.0;
                    (data.graph.max_lanes as f32 * LANE_W + 4.0).max(18.0)
                };
                let graph_lines: Vec<tako_core::GraphLine> = if i < data.graph.rows.len() {
                    data.graph.rows[i].lines.clone()
                } else {
                    Vec::new()
                };
                let graph_commit_lane = if i < data.graph.rows.len() {
                    data.graph.rows[i].lane
                } else {
                    0
                };
                let graph_commit_color = if i < data.graph.rows.len() {
                    data.graph.rows[i].color_index
                } else {
                    0
                };
                row = row.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_graph_row(
                                window,
                                bounds,
                                &graph_lines,
                                graph_commit_lane,
                                graph_commit_color,
                            );
                        },
                    )
                    .w(px(graph_w))
                    .flex_none(),
                );

                // コミット情報
                let mut info = div().flex_1().flex().flex_col();
                // 1行目: subject + refs
                let mut first_line = div().flex().flex_row().items_center().gap_1();
                first_line = first_line.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(hsla(fg_active))
                        .text_ellipsis()
                        .child(commit.subject.clone()),
                );
                if has_refs {
                    for r in commit.refs.split(", ") {
                        let badge_color = data
                            .graph
                            .ref_colors
                            .get(r)
                            .map(|&ci| tako_core::GRAPH_PALETTE[ci])
                            .unwrap_or(accent);
                        first_line = first_line.child(
                            div()
                                .px_1()
                                .rounded(px(3.0))
                                .text_size(px(9.0))
                                .bg(rgba_alpha(badge_color, 0.25))
                                .text_color(hsla(badge_color))
                                .child(r.to_string()),
                        );
                    }
                }
                info = info.child(first_line);
                // 2行目: hash + author + date
                info = info.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .text_size(px(9.0))
                        .text_color(hsla(fg))
                        .child(hash)
                        .child(commit.author.clone())
                        .child(commit.date_relative.clone()),
                );
                row = row.child(info);
                root = root.child(row);
            }
        }

        // ──── diff セクション（#487: 未ステージ / ステージ済み / 選択コミットを別見出しに）────
        let diff_sections: Vec<(String, &Vec<tako_core::DiffFile>)> = if selected_commit.is_some() {
            vec![(
                crate::ui_text::panel::git_diff_commit(data.diff_files.len()),
                &data.diff_files,
            )]
        } else {
            let mut v: Vec<(String, &Vec<tako_core::DiffFile>)> = Vec::new();
            if !data.diff_files.is_empty() {
                v.push((
                    crate::ui_text::panel::git_diff_unstaged(data.diff_files.len()),
                    &data.diff_files,
                ));
            }
            if !data.diff_staged.is_empty() {
                v.push((
                    crate::ui_text::panel::git_diff_staged(data.diff_staged.len()),
                    &data.diff_staged,
                ));
            }
            v
        };
        for (si, (label, files)) in diff_sections.iter().enumerate() {
            if files.is_empty() {
                continue;
            }
            root = root.child(
                div()
                    .id(("git-diff-header", si))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .px_2()
                    .py(px(3.0))
                    .mt_1()
                    .cursor_pointer()
                    .text_size(px(10.0))
                    .text_color(hsla(fg))
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git_collapsed.diff = !this.git_collapsed.diff;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(if collapsed.diff {
                                ui_icon::CHEVRON_RIGHT
                            } else {
                                ui_icon::CHEVRON_DOWN
                            })
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(hsla(fg)),
                    )
                    .child(SharedString::from(label.clone())),
            );
            if collapsed.diff {
                continue;
            }
            for file in files.iter() {
                // ファイルヘッダ
                root = root.child(
                    div()
                        .px_3()
                        .py(px(2.0))
                        .text_size(px(10.0))
                        .text_color(hsla(fg_active))
                        .bg(rgba_alpha(fg, 0.05))
                        .child(file.path.clone()),
                );
                for hunk in &file.hunks {
                    // ハンクヘッダ
                    root = root.child(
                        div()
                            .px_3()
                            .py(px(1.0))
                            .text_size(px(9.0))
                            .text_color(hsla(theme.ansi[6])) // cyan
                            .child(hunk.header.clone()),
                    );
                    for line in &hunk.lines {
                        let (prefix, color, bg_color) = match line.kind {
                            tako_core::DiffLineKind::Add => (
                                "+",
                                theme.green, // 緑
                                rgba_alpha(theme.green, 0.1),
                            ),
                            tako_core::DiffLineKind::Remove => (
                                "-",
                                theme.red, // 赤
                                rgba_alpha(theme.red, 0.1),
                            ),
                            tako_core::DiffLineKind::Context => (
                                " ",
                                fg,
                                Rgba {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 0.0,
                                },
                            ),
                        };
                        root = root.child(
                            div()
                                .px_3()
                                .text_size(px(11.0))
                                .text_color(hsla(color))
                                .bg(bg_color)
                                .child(format!("{prefix}{}", line.content)),
                        );
                    }
                }
            }
        }

        root
    }

    /// git コミットメッセージ入力欄のキーハンドラ（#472 → #487 で全面修正）。
    /// true を返すとイベントを消費、false を返すと後続ハンドラへ。
    ///
    /// #487 の修正点: ①`keystroke.key` は shift 無視の論理キー名なので大文字が打てなかった →
    /// `key_char` を使う ②未知キーを false で流していたため文字がターミナルへ漏れていた →
    /// 修飾なしキーは常に消費する ③キャレット位置（←→ / Home / End / Delete）に対応
    pub(crate) fn handle_git_commit_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let cursor = self.git_commit_cursor.min(self.git_commit_message.len());
        self.git_commit_cursor = cursor;
        match keystroke.key.as_str() {
            "enter" if keystroke.modifiers.platform => {
                if let Some(data) = &self.git_data {
                    let repo = data.repo_root.clone();
                    self.git_do_commit(repo, cx);
                }
                true
            }
            "escape" => {
                self.git_commit_input_focused = false;
                cx.notify();
                true
            }
            "backspace" => {
                if cursor > 0 {
                    let prev = self.git_commit_message[..cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.git_commit_message.drain(prev..cursor);
                    self.git_commit_cursor = prev;
                }
                cx.notify();
                true
            }
            "delete" => {
                if cursor < self.git_commit_message.len() {
                    let next = cursor
                        + self.git_commit_message[cursor..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                    self.git_commit_message.drain(cursor..next);
                }
                cx.notify();
                true
            }
            "left" => {
                self.git_commit_cursor = self.git_commit_message[..cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                cx.notify();
                true
            }
            "right" => {
                if cursor < self.git_commit_message.len() {
                    self.git_commit_cursor = cursor
                        + self.git_commit_message[cursor..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                }
                cx.notify();
                true
            }
            "home" | "up" => {
                self.git_commit_cursor = 0;
                cx.notify();
                true
            }
            "end" | "down" => {
                self.git_commit_cursor = self.git_commit_message.len();
                cx.notify();
                true
            }
            // cmd / ctrl 付きはアプリのキーバインド（⌘V 等）へ通す
            _ if keystroke.modifiers.platform || keystroke.modifiers.control => false,
            _ => {
                if let Some(ch) = keystroke.key_char.as_deref() {
                    if !ch.is_empty() && !ch.chars().any(|c| c.is_control()) {
                        self.git_commit_message.insert_str(cursor, ch);
                        self.git_commit_cursor = cursor + ch.len();
                        cx.notify();
                        return true;
                    }
                }
                // 空白は key_char が来ないことがある（実機で「Fix-487 staged」の
                // 空白以降が入らないのを観測。#487）ので論理キー名で拾い直す
                if keystroke.key == "space" {
                    self.git_commit_message.insert(cursor, ' ');
                    self.git_commit_cursor = cursor + 1;
                    cx.notify();
                    return true;
                }
                // 修飾なしキーは入力欄が握る（ターミナルへ漏らさない）
                true
            }
        }
    }

    /// ファイル単位 / 全体の git add を background 実行する（#487。
    /// `paths` が空なら全変更をステージ = CLI `tako git stage` と同じ挙動）
    pub(crate) fn git_do_stage(
        &mut self,
        repo_root: String,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let label = stage_feedback_label(&paths);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = std::path::Path::new(&repo_root);
                    let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
                    tako_core::git::stage(repo, &refs)
                })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(_) => app.git_set_feedback(format!("staged: {label}"), false, cx),
                    Err(e) => app.git_set_feedback(e, true, cx),
                }
                app.refresh_git(cx);
            });
        })
        .detach();
    }

    /// ファイル単位 / 全体の git reset HEAD を background 実行する（#487）
    pub(crate) fn git_do_unstage(
        &mut self,
        repo_root: String,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let label = stage_feedback_label(&paths);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = std::path::Path::new(&repo_root);
                    let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
                    tako_core::git::unstage(repo, &refs)
                })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(_) => app.git_set_feedback(format!("unstaged: {label}"), false, cx),
                    Err(e) => app.git_set_feedback(e, true, cx),
                }
                app.refresh_git(cx);
            });
        })
        .detach();
    }

    /// git commit を background で実行し、完了後にフィードバック表示 + データ更新（#472）。
    /// #487: ステージ済みがあるときは `-a` を付けず「ステージした分だけ」コミットする
    /// （付けたままだと UI のステージング操作が無意味になる）
    fn git_do_commit(&mut self, repo_root: String, cx: &mut Context<Self>) {
        let message = self.git_commit_message.clone();
        if message.trim().is_empty() {
            return;
        }
        let has_staged = self
            .git_data
            .as_ref()
            .is_some_and(|d| d.status.iter().any(|e| e.is_staged()));
        let all = !has_staged;
        self.git_commit_message.clear();
        self.git_commit_cursor = 0;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = std::path::Path::new(&repo_root);
                    tako_core::git::commit(repo, &message, all)
                })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(out) => app.git_set_feedback(out.trim().to_string(), false, cx),
                    Err(e) => app.git_set_feedback(e, true, cx),
                }
                app.refresh_git(cx);
            });
        })
        .detach();
    }

    /// git pull を background で実行（#472）
    fn git_do_pull(&mut self, repo_root: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = std::path::Path::new(&repo_root);
                    tako_core::git::pull(repo)
                })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(out) => {
                        let msg = if out.trim().is_empty() {
                            "Already up to date.".to_string()
                        } else {
                            out.trim().lines().last().unwrap_or("pull done").to_string()
                        };
                        app.git_set_feedback(msg, false, cx);
                    }
                    Err(e) => app.git_set_feedback(e, true, cx),
                }
                app.refresh_git(cx);
            });
        })
        .detach();
    }

    /// git push を background で実行（#472）
    fn git_do_push(&mut self, repo_root: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = std::path::Path::new(&repo_root);
                    tako_core::git::push(repo)
                })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(out) => {
                        let msg = if out.trim().is_empty() {
                            "push done".to_string()
                        } else {
                            out.trim().lines().last().unwrap_or("push done").to_string()
                        };
                        app.git_set_feedback(msg, false, cx);
                    }
                    Err(e) => app.git_set_feedback(e, true, cx),
                }
                app.refresh_git(cx);
            });
        })
        .detach();
    }

    /// フィードバックメッセージをセットし、4 秒後に自動クリア（#472）
    fn git_set_feedback(&mut self, message: String, is_error: bool, cx: &mut Context<Self>) {
        self.git_feedback = Some(GitFeedback { message, is_error });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                app.git_feedback = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// git パネルのデータを即座に background 取得する
    pub(crate) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if let Some(cwd) = self.git_cwd_for_tab() {
            let selected = self.git_selected_commit.clone();
            cx.spawn(async move |this, cx| {
                let data = cx
                    .background_executor()
                    .spawn(async move { fetch_git_data(&cwd, selected.as_deref()) })
                    .await;
                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                    app.git_data = data;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// 右サイドバー情報パネル（非表示なら None）。内部タブは統合 tmux ビュー
    /// （FR-2.16.6）と git（git graph FR-3.6 実装まではプレースホルダ）の 2 本
    pub(crate) fn render_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.panel_visible {
            return None;
        }
        let theme = self.theme.clone();
        let view = self.panel_view;
        // カンプ準拠のタブ（アイコン + ラベル、active は下線 inset）
        let tab_button =
            |label: &'static str, icon: &'static str, target: PanelView, active: bool| {
                div()
                    .id(("panel-tab", target as u64))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .px(px(12.0))
                    .h_full()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .when(active, |d| {
                        d.font_weight(gpui::FontWeight::SEMIBOLD)
                            .shadow(vec![gpui::BoxShadow {
                                color: hsla(theme.accent),
                                offset: gpui::point(px(0.), px(-2.)),
                                blur_radius: px(0.),
                                spread_radius: px(0.),
                                inset: true,
                            }])
                    })
                    .when(!active, |d| {
                        d.hover(|d| d.text_color(hsla(theme.foreground)))
                    })
                    .text_color(if active {
                        hsla(theme.foreground)
                    } else {
                        hsla(theme.text_muted)
                    })
                    .child(
                        svg()
                            .path(icon)
                            .w(px(14.0))
                            .h(px(14.0))
                            .text_color(if active {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.text_muted)
                            }),
                    )
                    .child(label)
            };
        Some(
            div()
                .w(px(self.panel_width))
                .h_full()
                .relative()
                .flex()
                .flex_col()
                .bg(rgba(theme.mantle))
                .border_l_1()
                .border_color(hsla(theme.border_subtle))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_stretch()
                        .px_2()
                        .h(px(38.0))
                        .flex_none()
                        .border_b_1()
                        .border_color(hsla(theme.border_inner))
                        .bg(rgba(theme.mantle))
                        .child(
                            tab_button(
                                "fleet",
                                crate::file_icons::ui_icon::FLEET,
                                PanelView::Tmux,
                                view == PanelView::Tmux,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_view = PanelView::Tmux;
                                this.refresh_tmux(cx);
                            })),
                        )
                        .child(
                            tab_button(
                                "orch",
                                if view == PanelView::Orch {
                                    crate::file_icons::ui_icon::ORCH_ACTIVE
                                } else {
                                    crate::file_icons::ui_icon::ORCH
                                },
                                PanelView::Orch,
                                view == PanelView::Orch,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_view = PanelView::Orch;
                                cx.notify();
                            })),
                        )
                        .child(
                            tab_button(
                                "git",
                                crate::file_icons::ui_icon::GIT_BRANCH,
                                PanelView::Git,
                                view == PanelView::Git,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_view = PanelView::Git;
                                cx.notify();
                            })),
                        )
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id("panel-close")
                                .flex()
                                .items_center()
                                .px_1()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.panel_visible = false;
                                    cx.notify();
                                }))
                                .child(
                                    svg()
                                        .path(crate::file_icons::ui_icon::CLOSE)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .text_color(hsla(theme.text_muted)),
                                ),
                        ),
                )
                .child(match view {
                    PanelView::Tmux => self.render_tmux_view(cx).into_any_element(),
                    PanelView::Orch => self.render_orch_view(cx).into_any_element(),
                    PanelView::Git => self.render_git_view(cx).into_any_element(),
                })
                .child(
                    // 左端のリサイズハンドル（ドラッグで幅調整）
                    div()
                        .id("panel-resize")
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(BORDER_HANDLE))
                        .h_full()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.dragging_panel = true;
                                cx.stop_propagation();
                            }),
                        ),
                ),
        )
    }
}

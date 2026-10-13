//! 右パネルの `references` ビュー —— 参照検索・呼び出し階層・型階層の結果（Issue #1685）
//!
//! ## 正本はここに無い
//!
//! 結果とツリーの展開状態の正本は `tako_core::lsp::xref::XrefStore`（`TakoApp::lsp_xref`）で、
//! この画面はそれを読むだけ。行のクリック・ノードの開閉の印・見出しの × は CLI / MCP と同じ
//! `Request::LspResults`（`open-result` / `expand` / `collapse` / `clear-results`）を dispatch で通す
//! （設計原則 5。UI 層に閉じた操作を作らない = 画面で開いたノードも `tako lsp results` の
//! `expanded` で読める。#1407 と同じ穴を作らない）。
//!
//! 子を読んでいないノードの展開と、右クリックメニューからの検索は言語サーバを待つので
//! `prepare_offload` → background → `finish_offload` の 3 段を通す（UI スレッドで待たない）。
//!
//! 行を押す（⌘クリックも同じ）と、定義ジャンプと同じ着地（同じファイル = 同じペイン / 同じタブで
//! 開いている = そのペイン / それ以外 = 新しいペイン）でその行を開く。開閉の印は既存の SVG
//! （chevron）で描く（絵文字・グリフを使わない）。

use gpui::{div, prelude::*, px, svg, Context, SharedString};
use tako_control::protocol::Request;
use tako_core::lsp::xref::{Body, XrefKind, XrefResult};
use tako_core::{PaneId, PaneOrigin};

use super::*;

/// 描く行の上限（残りは「ほか N 件」。全件は CLI / MCP が返す）
const ROWS: usize = 500;

/// 参照・階層の GUI の状態（結果そのものは `TakoApp::lsp_xref`）
#[derive(Debug, Default)]
pub(crate) struct XrefUi {
    /// 問い合わせ中（右クリックの項目を押した〜答えが届くまで。見出しの下に 1 行出す）
    pub(crate) pending: Option<(PaneId, XrefKind, String)>,
    /// 問い合わせの番号（押し直したら前の答えは捨てる）
    pub(crate) seq: u64,
    /// 子を問い合わせているノード（結果のペイン, ノード）
    pub(crate) expanding: Option<(PaneId, usize)>,
}

impl TakoApp {
    /// 右クリックメニューの「参照を検索」等（#1685）。要求は CLI / MCP と同じ `item_request` が組み
    /// （`show` つき = 右パネルに出す）、問い合わせは background で待つ。押した時点でビューを開いて
    /// 「問い合わせています」を出す（答えが遅い大きなプロジェクトで押したことが見える）
    pub(crate) fn request_lsp_xref(
        &mut self,
        pane: PaneId,
        request: Request,
        cx: &mut Context<Self>,
    ) {
        let (kind, symbol) = match &request {
            Request::LspXref {
                action,
                line,
                column,
                ..
            } => (
                XrefKind::parse(action).unwrap_or(XrefKind::References),
                self.preview_line_texts
                    .get(&pane)
                    .and_then(|texts| texts.get(line.saturating_sub(1)))
                    .and_then(|text| {
                        tako_core::lsp::goto::symbol_at(text, *column)
                            .map(|s| text[s.range].to_string())
                    })
                    .unwrap_or_default(),
            ),
            _ => return,
        };
        let job = match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => job,
            Some(Err(e)) => {
                self.show_lsp_status(pane, e.to_string(), true, cx);
                return;
            }
            None => return,
        };
        self.lsp_xref_ui.seq = self.lsp_xref_ui.seq.wrapping_add(1);
        let seq = self.lsp_xref_ui.seq;
        self.lsp_xref_ui.pending = Some((pane, kind, symbol));
        self.panel_visible = true;
        self.panel_view = PanelView::References;
        let staged = cx
            .background_executor()
            .spawn(async move { job.run_staged() });
        cx.spawn(async move |this, cx| {
            let outcome = staged.await;
            let _ = this.update(cx, |app, cx| app.finish_lsp_xref(outcome, seq, pane, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_lsp_xref(
        &mut self,
        outcome: tako_control::OffloadOutcome,
        seq: u64,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        // 答えを待つあいだに次を押した = この答えは誰も待っていない（置かない）
        if seq != self.lsp_xref_ui.seq {
            return;
        }
        self.lsp_xref_ui.pending = None;
        let result = match outcome {
            tako_control::OffloadOutcome::OnUi(next) => {
                tako_control::finish_offload(self, next, PaneOrigin::User)
            }
            tako_control::OffloadOutcome::Reply(result) => result,
        };
        match result {
            // 置けた（0 件も置く = ビューに「見つかりませんでした」）。失敗の答えは理由を出す
            Ok(out) => {
                if !matches!(out["status"].as_str(), Some("found" | "none")) {
                    let reason = out["reason"].as_str().unwrap_or_default().to_string();
                    self.show_lsp_status(pane, reason, true, cx);
                }
            }
            Err(e) => self.show_lsp_status(pane, e.to_string(), true, cx),
        }
        cx.notify();
    }

    /// 行を開く（右パネルの行のクリック = CLI `tako lsp open-result` と同じ要求）
    fn open_xref_item(&mut self, pane: PaneId, item: usize, cx: &mut Context<Self>) {
        let request = Request::LspResults {
            action: tako_control::dispatch::LSP_OPEN_RESULT_ACTION.into(),
            pane: Some(pane.as_u64()),
            item: Some(item),
            open: None,
            focus: Some(true),
            limit: None,
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.notify_ui_dispatch_failed(
                crate::sidebar::NoticeArea::RightPanel,
                crate::sidebar::NoticeArm::Issue1685,
                "xref_open",
                Some(&item.to_string()),
                &e,
            );
        }
        cx.notify();
    }

    /// ノードの開閉（右パネルの印 = CLI `tako lsp expand` / `collapse` と同じ要求）。子を読んでいない
    /// ノードの展開は background で問い合わせる（読み込み済みの開閉は同期で状態だけ変える）
    fn toggle_xref_node(
        &mut self,
        pane: PaneId,
        item: usize,
        expand: bool,
        cx: &mut Context<Self>,
    ) {
        let action = if expand {
            tako_control::dispatch::LSP_EXPAND_ACTION
        } else {
            tako_control::dispatch::LSP_COLLAPSE_ACTION
        };
        let request = Request::LspResults {
            action: action.into(),
            pane: Some(pane.as_u64()),
            item: Some(item),
            open: None,
            focus: None,
            limit: None,
        };
        let fail = move |app: &mut Self, e: &tako_control::DispatchError| {
            app.notify_ui_dispatch_failed(
                crate::sidebar::NoticeArea::RightPanel,
                crate::sidebar::NoticeArm::Issue1685,
                "xref_toggle",
                Some(&item.to_string()),
                e,
            );
        };
        match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => {
                self.lsp_xref_ui.expanding = Some((pane, item));
                let staged = cx
                    .background_executor()
                    .spawn(async move { job.run_staged() });
                cx.spawn(async move |this, cx| {
                    let outcome = staged.await;
                    let _ = this.update(cx, |app, cx| {
                        if app.lsp_xref_ui.expanding == Some((pane, item)) {
                            app.lsp_xref_ui.expanding = None;
                        }
                        let result = match outcome {
                            tako_control::OffloadOutcome::OnUi(next) => {
                                tako_control::finish_offload(app, next, PaneOrigin::User)
                            }
                            tako_control::OffloadOutcome::Reply(result) => result,
                        };
                        match result {
                            Ok(out) => {
                                if out["reason"].is_string() {
                                    let reason =
                                        out["reason"].as_str().unwrap_or_default().to_string();
                                    app.show_lsp_status(pane, reason, true, cx);
                                }
                            }
                            Err(e) => fail(app, &e),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Some(Err(e)) => fail(self, &e),
            None => {
                if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
                    fail(self, &e);
                }
            }
        }
        cx.notify();
    }

    /// 見出しの ×（CLI `tako lsp clear-results` と同じ要求）
    fn clear_xref(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let request = Request::LspResults {
            action: tako_control::dispatch::LSP_CLEAR_RESULTS_ACTION.into(),
            pane: Some(pane.as_u64()),
            item: None,
            open: None,
            focus: None,
            limit: None,
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.notify_ui_dispatch_failed(
                crate::sidebar::NoticeArea::RightPanel,
                crate::sidebar::NoticeArm::Issue1685,
                "xref_clear",
                Some(&pane.as_u64().to_string()),
                &e,
            );
        }
        cx.notify();
    }

    pub(crate) fn render_references_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        // 閉じたペインの結果は出さない（閉じる経路で消すが、取りこぼしの保険 = dispatch と同じ）
        let live = self.workspace.all_pane_ids();
        self.lsp_xref.retain_panes(|p| live.contains(&p));
        let result: Option<XrefResult> = self.lsp_xref.active().cloned();
        let pending = self.lsp_xref_ui.pending.clone();
        let expanding = self.lsp_xref_ui.expanding;
        let probe_bounds = self.panel_click_probe_bounds.clone();
        // 識別子・抜粋はコードと同じ等幅で
        let mono = SharedString::from(theme.font_family.clone());

        let mut root = div()
            .id("panel-references-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground));
        let note = |text: String| {
            div()
                .px(px(10.0))
                .py(px(10.0))
                .text_size(px(11.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        if let Some((_, kind, symbol)) = &pending {
            root = root.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_inner))
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(crate::ui_text::panel::xref_kind(*kind)),
            );
            let label = if symbol.is_empty() {
                crate::ui_text::panel::xref_kind(*kind).to_string()
            } else {
                symbol.clone()
            };
            root = root.child(note(crate::ui_text::panel::xref_searching(&label)));
        }
        let Some(result) = result else {
            if pending.is_none() {
                root = root.child(note(crate::ui_text::panel::xref_empty().into()));
            }
            return root;
        };
        let pane = result.pane;
        let count = match &result.body {
            Body::List(_) => result.total,
            Body::Tree(tree) => tree.len(),
        };
        if pending.is_none() {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_inner))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(crate::ui_text::panel::xref_kind(result.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .child(SharedString::from(result.symbol.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(format!(
                                "{} · {}",
                                crate::ui_text::panel::xref_count(count),
                                crate::ui_text::panel::diagnostics_pane(pane.as_u64())
                            ))),
                    )
                    .child(
                        div()
                            .id("panel-references-close")
                            .flex_none()
                            .px_1()
                            .cursor_pointer()
                            .tooltip({
                                let tip_theme = theme.clone();
                                move |_, cx| {
                                    cx.new(|_| {
                                        crate::tab_bar::HintTooltip::new(
                                            crate::ui_text::panel::xref_close().to_string(),
                                            tip_theme.clone(),
                                        )
                                    })
                                    .into()
                                }
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.clear_xref(pane, cx)))
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::CLOSE)
                                    .w(px(10.0))
                                    .h(px(10.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    ),
            );
        }
        if result.body.is_empty() {
            return root.child(note(crate::ui_text::panel::xref_none().into()));
        }

        let row_base = |id: usize| {
            div()
                .id(("xref-row", id as u64))
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .pr(px(10.0))
                .py(px(3.0))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(theme.surface_hover)))
        };
        // visual-test が実フレームの幾何で行の在処を読むための実矩形（診断ビューと同じ）
        let probe = |key: String| {
            let probe = probe_bounds.clone();
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    probe.borrow_mut().insert(key.clone(), bounds);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let muted = |text: String| {
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        let file_name = |path: &std::path::Path| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };

        let mut body = div()
            .id("panel-references-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(px(11.5));
        let mut drawn = 0;
        let mut hidden = 0;
        match &result.body {
            Body::List(items) => {
                let mut last_path: Option<&std::path::Path> = None;
                for (index, place) in items.iter().enumerate() {
                    if drawn >= ROWS {
                        hidden = items.len() - index;
                        break;
                    }
                    let id = index + 1;
                    // ファイルが変わるたびに見出し（参照は（ファイル, 行）順に並んでいる）
                    if last_path != Some(place.path.as_path()) {
                        last_path = Some(place.path.as_path());
                        let in_file = items.iter().filter(|p| p.path == place.path).count();
                        body = body.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(10.0))
                                .pt(px(8.0))
                                .pb(px(3.0))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(SharedString::from(file_name(&place.path))),
                                )
                                .child(muted(in_file.to_string())),
                        );
                    }
                    body = body.child(
                        row_base(id)
                            .pl(px(18.0))
                            .child(muted(format!("{}:{}", place.line + 1, place.column)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(mono.clone())
                                    .child(SharedString::from(place.excerpt.clone())),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_xref_item(pane, id, cx);
                            }))
                            .child(probe(format!("xref-row-{id}"))),
                    );
                    drawn += 1;
                }
            }
            Body::Tree(tree) => {
                let rows = tree.rows();
                for (index, row) in rows.iter().enumerate() {
                    if drawn >= ROWS {
                        hidden = rows.len() - index;
                        break;
                    }
                    let Some(node) = tree.node(row.id) else {
                        continue;
                    };
                    let id = row.id;
                    let expanded = node.expanded;
                    // 子が 0 件と分かっているノードは印を出さない（押しても何も無い）
                    let leaf = node.children.as_ref().is_some_and(|c| c.is_empty());
                    let busy = expanding == Some((pane, id));
                    let toggle = div()
                        .id(("xref-toggle", id as u64))
                        .relative()
                        .flex_none()
                        .w(px(14.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!leaf, |d| {
                            d.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_xref_node(pane, id, !expanded, cx);
                                }))
                                .child(
                                    svg()
                                        .path(if expanded || busy {
                                            crate::file_icons::ui_icon::CHEVRON_DOWN
                                        } else {
                                            crate::file_icons::ui_icon::CHEVRON_RIGHT
                                        })
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .text_color(hsla(theme.text_muted)),
                                )
                        })
                        .child(probe(format!("xref-toggle-{id}")));
                    body = body.child(
                        row_base(id)
                            .pl(px(6.0 + 14.0 * row.depth as f32))
                            .child(toggle)
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(180.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(mono.clone())
                                    .child(SharedString::from(node.name.clone())),
                            )
                            .child(muted(
                                tako_core::lsp::xref::symbol_kind_name(node.kind).to_string(),
                            ))
                            .child(div().flex_1().min_w(px(0.0)))
                            .child(muted(format!(
                                "{}:{}",
                                file_name(&node.place.path),
                                node.place.line + 1
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_xref_item(pane, id, cx);
                            }))
                            .child(probe(format!("xref-row-{id}"))),
                    );
                    drawn += 1;
                }
            }
        }
        if hidden > 0 {
            body = body.child(note(crate::ui_text::panel::xref_more(hidden)));
        }
        root.child(body)
    }
}

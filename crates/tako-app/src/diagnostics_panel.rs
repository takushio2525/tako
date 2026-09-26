//! 右パネルの `diagnostics` ビュー —— 言語サーバの診断の一覧（Issue #1679）
//!
//! ## 正本はここに無い
//!
//! 診断の正本は manager の URI ごとの表（`tako_control::lsp`）で、この画面は
//! `EditState::diagnostics`（知らせのたびに差し替わる `Arc` の写し）を読むだけ。
//! `tako lsp diagnostics` / MCP `tako_lsp` が返すのと同じ中身・同じ並び
//! （`tako_core::lsp::diagnostic::sort`）になる。
//!
//! 行を押すと、そのペインへフォーカスを移して診断の範囲を選ぶ。操作は
//! `tako edit cursor` と同じ `Request::PreviewCursor` を dispatch で通す（設計原則 5。
//! UI 層に閉じた位置合わせを作らない）。
//!
//! 印（重大度の丸）は GPUI の図形で描く（絵文字・グリフを使わない）。色は波線と同じ
//! `preview_render::diagnostic_color` の 1 つを読む。

use gpui::{div, prelude::*, px, Context, SharedString};
use tako_control::lsp::{DocDiagnostics, DocLink};
use tako_core::lsp::diagnostic::{self, Diagnostic, Severity};
use tako_core::{PaneId, PaneOrigin};

use super::*;

/// 1 文書あたりに描く行の上限（残りは「ほか N 件」。全件は CLI / MCP が返す）
const ROWS_PER_DOCUMENT: usize = 200;

impl TakoApp {
    /// LSP につながった文書（編集セッション）が 1 つでもあるか（diagnostics タブを出す条件）
    pub(crate) fn lsp_documents_open(&self) -> bool {
        self.preview_edits
            .values()
            .any(|e| matches!(e.lsp, DocLink::Open(_)))
    }

    /// 診断 1 件の位置へ移る（フォーカス + 範囲の選択）。CLI の `tako focus` と
    /// `tako edit cursor` と同じ要求を dispatch で通す
    fn jump_to_diagnostic(&mut self, pane: PaneId, d: &Diagnostic, cx: &mut Context<Self>) {
        let focus = tako_control::protocol::Request::Focus {
            pane: Some(pane.as_u64()),
            direction: None,
        };
        let place = tako_control::protocol::Request::PreviewCursor {
            pane: Some(pane.as_u64()),
            line: d.start.line + 1,
            col: d.start.col,
            select_to_line: (d.end != d.start).then_some(d.end.line + 1),
            select_to_col: (d.end != d.start).then_some(d.end.col),
            expected_version: None,
        };
        for (op, request) in [("diagnostic_focus", focus), ("diagnostic_jump", place)] {
            if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
                // 押した操作の失敗は通知欄へ（GUI の stderr は誰も読めない = #1399）
                self.notify_ui_dispatch_failed(
                    crate::sidebar::NoticeArea::RightPanel,
                    crate::sidebar::NoticeArm::Issue1679,
                    op,
                    Some(&pane.as_u64().to_string()),
                    &e,
                );
                break;
            }
        }
        cx.notify();
    }

    pub(crate) fn render_diagnostics_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let mut docs: Vec<(PaneId, String, Option<DocDiagnostics>)> = self
            .preview_edits
            .iter()
            .filter(|(_, e)| matches!(e.lsp, DocLink::Open(_)))
            .map(|(pane, e)| {
                let name = e
                    .buffer
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (*pane, name, e.diagnostics.clone())
            })
            .collect();
        docs.sort_by_key(|(pane, ..)| pane.as_u64());
        let totals = diagnostic::counts(
            docs.iter()
                .filter_map(|(_, _, d)| d.as_ref())
                .flat_map(|d| d.items.iter()),
        );

        let dot = |severity: Severity| {
            div()
                .flex_none()
                .w(px(7.0))
                .h(px(7.0))
                .rounded_full()
                .bg(hsla(crate::preview_render::diagnostic_color(
                    &theme, severity,
                )))
        };
        let summary = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(Severity::ALL.into_iter().map(|severity| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(dot(severity))
                    .child(SharedString::from(totals[severity.rank()].to_string()))
            }));

        let mut root = div()
            .id("panel-diagnostics-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_inner))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(crate::ui_text::panel::diagnostics_title()),
                    )
                    .child(
                        summary
                            .text_size(px(11.0))
                            .text_color(hsla(theme.text_muted)),
                    ),
            );

        let note = |text: String| {
            div()
                .px(px(10.0))
                .py(px(10.0))
                .text_size(px(11.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        if !self.lsp.is_enabled() {
            return root.child(note(crate::ui_text::panel::diagnostics_disabled().into()));
        }
        if docs.is_empty() {
            return root.child(note(crate::ui_text::panel::diagnostics_empty().into()));
        }

        let probe_bounds = self.panel_click_probe_bounds.clone();
        let mut body = div()
            .id("panel-diagnostics-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(px(11.5));
        for (pane, name, diagnostics) in docs {
            let items = diagnostics.as_ref().map(|d| d.items.clone());
            let count = items.as_ref().map_or(0, |i| i.len());
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
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(format!(
                                "{} · {count}",
                                crate::ui_text::panel::diagnostics_pane(pane.as_u64())
                            ))),
                    ),
            );
            let Some(items) = items else {
                body = body.child(note(crate::ui_text::panel::diagnostics_waiting().into()));
                continue;
            };
            if items.is_empty() {
                body = body.child(note(crate::ui_text::panel::diagnostics_clean().into()));
                continue;
            }
            for (index, d) in items.iter().take(ROWS_PER_DOCUMENT).enumerate() {
                let probe = probe_bounds.clone();
                let probe_key = format!("diag-row-{}-{index}", pane.as_u64());
                let target = d.clone();
                let first_line = d.message.lines().next().unwrap_or("").to_string();
                let origin = [d.source.as_deref(), d.code.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                body = body.child(
                    div()
                        .id(("diag-row", (pane.as_u64() << 20) | index as u64))
                        .relative()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .py(px(3.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(theme.surface_hover)))
                        .child(dot(d.severity))
                        .child(div().flex_none().text_color(hsla(theme.text_muted)).child(
                            SharedString::from(format!("{}:{}", d.start.line + 1, d.start.col)),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(first_line)),
                        )
                        .when(!origin.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.5))
                                    .text_color(hsla(theme.text_muted))
                                    .child(SharedString::from(origin)),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump_to_diagnostic(pane, &target, cx);
                        }))
                        // visual-test が実フレームの幾何で行の在処を読むための実矩形
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
            if items.len() > ROWS_PER_DOCUMENT {
                body = body.child(note(crate::ui_text::panel::diagnostics_more(
                    items.len() - ROWS_PER_DOCUMENT,
                )));
            }
        }
        root = root.child(body);
        root
    }
}

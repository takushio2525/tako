//! エディタに言語サーバの状態を出す（#1944）: タイトルの 1 行（取得中 / 起動中 / 読み込み中 /
//! 動作中）と、未導入・取得の失敗・止まったときの帯（理由 + 次の一手 + 押せば入るボタン）
//!
//! 「インテリジェンス機能が黙って出ない」を無くすのが目的。状態の正本は manager
//! （`LspManager::document_server`。`tako lsp status` / MCP `tako_lsp_server` と同じ判定と同じ文）で、
//! ここは見せ方と、ボタンから manager の同じ口（`install` / `restart`）を背景で呼ぶことだけを持つ。
//! 描き直しのきっかけは manager の状態の知らせ（`LspManager::state_events`。診断の知らせとは別の口）。
//!
//! A/B は `TAKO_1944_LEGACY=1`（取得も状態の表示も出さない = #1678 の「案内だけ」）。

use gpui::prelude::*;
use gpui::{canvas, div, px, Context, MouseButton, MouseDownEvent, SharedString};
use tako_control::lsp::DocumentServer;
use tako_core::lsp::state::ServerState;
use tako_core::theme::Theme;
use tako_core::PaneId;

use crate::{hsla, hsla_alpha, rgba_alpha, TakoApp};

/// タイトルの 1 行の色合い
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadgeTone {
    /// 取得中・起動中・読み込み中（進んでいる）
    Busy,
    /// 動作中（控えめ）
    Quiet,
}

impl TakoApp {
    /// 編集中のペインの言語サーバの状態（無ければ `None` = 受け持つサーバが無い・LSP を止めている）
    pub(crate) fn lsp_document_server(&self, pane_id: PaneId) -> Option<DocumentServer> {
        if tako_control::lsp::fetch::legacy() {
            return None;
        }
        let edit = self.preview_edits.get(&pane_id)?;
        if !edit.editing {
            return None;
        }
        self.lsp.document_server(edit.buffer.path())
    }

    /// タイトルの 1 行（帯を出す状態では出さない）
    pub(crate) fn lsp_status_badge(&self, pane_id: PaneId) -> Option<(String, BadgeTone)> {
        let server = self.lsp_document_server(pane_id)?;
        badge_of(&server)
    }

    /// 未導入・取得の失敗・止まったときの帯（理由 + 次の一手 + ボタン）
    pub(crate) fn render_lsp_status_bar(
        &self,
        pane_id: PaneId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let server = self.lsp_document_server(pane_id)?;
        if badge_of(&server).is_some() {
            return None;
        }
        let (title, action) = match server.state {
            ServerState::NotInstalled if server.fetch_failed => (
                crate::ui_text::preview::lsp_fetch_failed(server.id),
                server.can_install.then_some(BarAction::Install(true)),
            ),
            ServerState::NotInstalled => (
                crate::ui_text::preview::lsp_not_installed(server.id),
                server.can_install.then_some(BarAction::Install(false)),
            ),
            ServerState::GaveUp | ServerState::Stopped => (
                crate::ui_text::preview::lsp_unavailable(server.id),
                Some(BarAction::Restart),
            ),
            _ => return None,
        };
        let key = pane_id.as_u64();
        let id = server.id;
        let detail: Vec<String> = [server.reason.clone(), server.next_step.clone()]
            .into_iter()
            .flatten()
            .collect();
        let accent = theme.yellow;
        let button = action.map(|action| {
            let probe = self.panel_click_probe_bounds.clone();
            let probe_key = format!("preview-lsp-action-{key}");
            let label = match action {
                BarAction::Install(false) => crate::ui_text::preview::lsp_install(),
                BarAction::Install(true) => crate::ui_text::preview::lsp_retry(),
                BarAction::Restart => crate::ui_text::preview::lsp_restart(),
            };
            div()
                .id(("preview-lsp-action", key))
                .relative()
                .flex_none()
                .px(px(6.0))
                .py(px(1.0))
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(hsla_alpha(accent, 0.5))
                .text_color(hsla(theme.foreground))
                .hover(|d| d.bg(rgba_alpha(accent, 0.2)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.lsp_status_action(id, action, cx);
                }))
                .child(label)
                // 実矩形は visual-test が実マウスで押すために記録する（競合の帯と同じ置き方）
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, _, _| {
                            probe.borrow_mut().insert(probe_key.clone(), bounds);
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
        });
        let notice = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(6.0))
            .px(px(8.0))
            .pt(px(4.0))
            // 印: 縦棒（GPUI の矩形。絵文字・記号の字形に頼らない）
            .child(
                div()
                    .flex_none()
                    .mt(px(1.0))
                    .w(px(3.0))
                    .h(px(14.0))
                    .rounded(px(1.5))
                    .bg(hsla(accent)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_color(hsla(theme.foreground))
                            .child(SharedString::from(title)),
                    )
                    .children(detail.into_iter().map(|line| {
                        div()
                            .text_color(hsla_alpha(theme.foreground, 0.7))
                            .child(SharedString::from(line))
                    })),
            );
        let row = div()
            .flex()
            .flex_row()
            .justify_end()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .children(button);
        Some(
            div()
                .id(("preview-lsp-status", key))
                .flex_none()
                .w_full()
                .flex()
                .flex_col()
                .text_size(px(11.0))
                .bg(rgba_alpha(accent, 0.10))
                .border_b_1()
                .border_color(hsla_alpha(accent, 0.45))
                .child(notice)
                .child(row)
                .into_any_element(),
        )
    }

    /// 帯のボタン: manager の `install` / `restart` を背景で呼ぶ（CLI `tako lsp install` / MCP
    /// `tako_lsp_server` と同じ口。取っているあいだの様子は manager の知らせで描き直る）
    fn lsp_status_action(&mut self, id: &'static str, action: BarAction, cx: &mut Context<Self>) {
        let manager = self.lsp.clone();
        cx.background_executor()
            .spawn(async move {
                match action {
                    BarAction::Install(_) => {
                        manager.install(Some(id));
                    }
                    BarAction::Restart => {
                        manager.restart(Some(id));
                    }
                }
            })
            .detach();
        cx.notify();
    }

    /// manager が状態の変化を知らせた（`LspManager::state_events` + `take_refresh_wanted`）: 編集セッションを
    /// 同期し直して（断っていた文書が使えるようになっていれば開く）描き直す
    pub(crate) fn refresh_lsp_links(&mut self, cx: &mut Context<Self>) {
        let panes: Vec<PaneId> = self
            .preview_edits
            .iter()
            .filter(|(_, edit)| edit.editing)
            .map(|(pane, _)| *pane)
            .collect();
        for pane in panes {
            self.sync_preview_lsp(pane);
        }
        cx.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarAction {
    /// 取って入れる（`true` = 失敗のあとのもう一度）
    Install(bool),
    Restart,
}

/// タイトルの 1 行の中身（帯を出す状態は `None`）
fn badge_of(server: &DocumentServer) -> Option<(String, BadgeTone)> {
    // 取っている最中なら状態に依らず進捗（`tako lsp install` が取っているあいだは未導入のまま）。
    // 取得物が決まる前（錠を取った直後）は「取得中」だけ
    if let Some(progress) = &server.fetch {
        let mb = |n: u64| n as f64 / 1_000_000.0;
        let detail = (!progress.item.is_empty() && progress.total > 0).then(|| {
            (
                progress.item.as_str(),
                mb(progress.done),
                mb(progress.total.max(progress.done)),
            )
        });
        return Some((
            crate::ui_text::preview::lsp_fetching(server.id, detail),
            BadgeTone::Busy,
        ));
    }
    Some(match server.state {
        ServerState::Fetching => (
            crate::ui_text::preview::lsp_fetching(server.id, None),
            BadgeTone::Busy,
        ),
        ServerState::Starting | ServerState::Crashed | ServerState::NotStarted => (
            crate::ui_text::preview::lsp_starting(server.id),
            BadgeTone::Busy,
        ),
        ServerState::Running if server.loading => (
            crate::ui_text::preview::lsp_loading(server.id),
            BadgeTone::Busy,
        ),
        ServerState::Running | ServerState::Stopping => (
            crate::ui_text::preview::lsp_running(server.id),
            BadgeTone::Quiet,
        ),
        ServerState::NotInstalled | ServerState::GaveUp | ServerState::Stopped => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(state: ServerState) -> DocumentServer {
        DocumentServer {
            id: "srv",
            state,
            loading: false,
            fetch: None,
            reason: None,
            next_step: None,
            can_install: true,
            fetch_failed: false,
        }
    }

    #[test]
    fn 帯を出す状態ではタイトルの行を出さない() {
        for state in ServerState::ALL {
            let shown = badge_of(&server(state)).is_some();
            let bar = matches!(
                state,
                ServerState::NotInstalled | ServerState::GaveUp | ServerState::Stopped
            );
            assert_eq!(shown, !bar, "{state:?}");
        }
    }

    #[test]
    fn 取っている最中は未導入でも進捗を出す() {
        let mut s = server(ServerState::NotInstalled);
        s.fetch = Some(tako_control::lsp::fetch::ProgressSnapshot {
            item: "x 1".into(),
            phase: tako_control::lsp::fetch::Phase::Downloading,
            step: 1,
            steps: 1,
            done: 2_000_000,
            total: 4_000_000,
            elapsed: std::time::Duration::ZERO,
        });
        let (text, tone) = badge_of(&s).expect("進捗を出す");
        assert_eq!(tone, BadgeTone::Busy);
        assert!(text.contains("2.0"), "{text}");
        assert!(text.contains("4.0"), "{text}");
        // 取得物が決まる前（錠を取った直後）は数字を出さない（0.0 / 0.0 MB と見せない）
        if let Some(p) = s.fetch.as_mut() {
            p.item.clear();
            p.total = 0;
        }
        let (text, _) = badge_of(&s).expect("取得中だけ");
        assert!(!text.contains("0.0"), "{text}");
    }

    #[test]
    fn 読み込み中と動作中を分ける() {
        let mut s = server(ServerState::Running);
        assert_eq!(badge_of(&s).unwrap().1, BadgeTone::Quiet);
        s.loading = true;
        assert_eq!(badge_of(&s).unwrap().1, BadgeTone::Busy);
    }
}

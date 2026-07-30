//! AI コマンド提案カードの描画と操作（FR-2.22 / Issue #666）
//!
//! AI が `tako_show_command` で渡したコマンドを、対象ペイン下部のカードとして出す。
//! 表示は折り返してよいが、**コピー・実行に使うのは保管された論理文字列**なので
//! ペイン幅の影響を受けない（画面から拾ったコマンドが壊れる問題の根治）。
//!
//! ボタンの実体は dispatch（`Request::ShowCommand`）を呼ぶだけ。CLI / MCP と同じ経路を
//! 通るので、UI 操作と AI 操作で挙動が食い違わない（開発不変条件）。

use gpui::{div, prelude::*, px, svg, Context, MouseButton, MouseDownEvent, SharedString};
use tako_core::PaneId;

use super::*;
use crate::file_icons::ui_icon;

/// コピー成功・失敗の表示を維持する時間。2 秒ポーリング（periodic）の再描画で自然に消える
const FEEDBACK_DURATION: std::time::Duration = std::time::Duration::from_millis(2200);

/// コマンド 1 件の本文の最大表示高さ。長いコマンドは折り返して全文出すが、
/// 極端に長いものでターミナルを覆い尽くさないよう高さで止める（スクロールで読める）
const COMMAND_MAX_HEIGHT: f32 = 190.0;

impl TakoApp {
    /// 指定ペインのコマンドカード（FR-2.22）。ペイン下端に**新しいものを上**にして積む。
    ///
    /// `bottom_offset` はポート検知チップ（FR-2.4.3）と重ならないための下端余白、
    /// `max_height` はペインの残り高さ。**新しいカードを必ず見せる**ため、
    /// 収まらない古いカードが下（スクロール領域）へ押し出される順序にしている
    /// （逆順にすると最新カードの見出しがペイン上端で切れる。実機で確認済み）
    pub(crate) fn render_command_cards(
        &mut self,
        pane_id: PaneId,
        bottom_offset: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = self.theme.clone();
        // 表示に必要な情報だけ取り出す（描画中に保管庫を借り続けない）
        let cards: Vec<(u64, Option<String>, Vec<String>)> = self
            .command_cards
            .list(Some(pane_id))
            .into_iter()
            .map(|c| {
                (
                    c.id().as_u64(),
                    c.label().map(str::to_string),
                    c.commands().to_vec(),
                )
            })
            .collect();
        if cards.is_empty() {
            return None;
        }
        let copied = self
            .command_card_copied
            .filter(|(_, _, at)| at.elapsed() < FEEDBACK_DURATION);
        let errored = self
            .command_card_error
            .filter(|(_, at)| at.elapsed() < FEEDBACK_DURATION);

        // 本文はペインの半分までに抑える（1 枚でもボタン行が押し出されないように）
        let command_max = COMMAND_MAX_HEIGHT.min((max_height * 0.5).max(48.0));
        let stack = div()
            .id(("command-card-stack", pane_id.as_u64()))
            .absolute()
            .bottom(px(bottom_offset))
            .left(px(8.0))
            .right(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            // ペインからはみ出さない。溢れた古いカードはスクロールで読める
            .max_h(px(max_height.max(60.0)))
            .overflow_y_scroll()
            // 下のペインへ選択・スクロールを漏らさない（提案チップと同じ）
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .children(cards.into_iter().rev().map(|(card_id, label, commands)| {
                let total = commands.len();
                let heading =
                    label.unwrap_or_else(|| crate::ui_text::command_card::heading().to_string());
                div()
                    .id(("command-card", card_id))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(9.0))
                    .bg(rgba(theme.surface_1))
                    .border_1()
                    .border_color(hsla(theme.accent_border_muted))
                    .shadow_sm()
                    // 見出し行（説明ラベル + × 閉じる）
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.text_secondary))
                                    .child(SharedString::from(heading)),
                            )
                            .when(errored.is_some_and(|(id, _)| id == card_id), |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.red))
                                        .child(crate::ui_text::command_card::run_failed()),
                                )
                            })
                            .child(
                                div()
                                    .id(("command-card-close", card_id))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.dismiss_command_card(card_id, cx);
                                    }))
                                    .child(
                                        svg()
                                            .path(ui_icon::CLOSE)
                                            .w(px(9.0))
                                            .h(px(9.0))
                                            .text_color(hsla(theme.text_muted)),
                                    ),
                            ),
                    )
                    // コマンド本体（等幅 + 背景パネル。折り返して全文を出す）
                    .children(commands.into_iter().enumerate().map(|(i, command)| {
                        let index = i + 1;
                        let is_copied =
                            copied.is_some_and(|(id, idx, _)| id == card_id && idx == index);
                        let theme = theme.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .p(px(7.0))
                            .rounded(px(6.0))
                            .bg(rgba(theme.crust))
                            .border_1()
                            .border_color(hsla(theme.border_subtle))
                            .child(
                                div()
                                    .id(("command-card-text", card_id * 100 + index as u64))
                                    .max_h(px(command_max))
                                    // 極端に長いコマンドは高さで止めてスクロールで読ませる
                                    .overflow_y_scroll()
                                    // 折り返して全文表示（コピーは論理文字列なので無関係）
                                    .whitespace_normal()
                                    .font_family(theme.font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(command)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    // 複数コマンドのときだけ番号を出す
                                    .when(total > 1, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(hsla(theme.text_muted))
                                                .child(SharedString::from(
                                                    crate::ui_text::command_card::index_label(
                                                        index, total,
                                                    ),
                                                )),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(self.command_card_button(
                                        ("command-card-copy", (card_id * 100 + index as u64)),
                                        if is_copied {
                                            ui_icon::CHECK
                                        } else {
                                            ui_icon::COPY
                                        },
                                        if is_copied {
                                            crate::ui_text::command_card::copied()
                                        } else {
                                            crate::ui_text::command_card::copy()
                                        },
                                        is_copied,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.copy_command_card(card_id, index, cx);
                                        }),
                                    ))
                                    .child(self.command_card_button(
                                        ("command-card-run", (card_id * 100 + index as u64)),
                                        ui_icon::PLAY,
                                        crate::ui_text::command_card::run(),
                                        false,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.run_command_card(card_id, index, cx);
                                        }),
                                    )),
                            )
                    }))
            }));
        Some(stack.into_any_element())
    }

    /// カードのボタン（アイコン + ラベル）。狭いペインでも押せるよう高さは固定し、
    /// 並びは flex_wrap で折り返す
    fn command_card_button(
        &self,
        id: (&'static str, u64),
        icon: &'static str,
        label: &'static str,
        active: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let (fg, border) = if active {
            (theme.green, theme.green)
        } else {
            (theme.text_secondary, theme.border_default)
        };
        div()
            .id(id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .px(px(7.0))
            .h(px(20.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .bg(rgba(theme.chip_surface))
            .border_1()
            .border_color(hsla(border))
            .text_size(px(10.5))
            .text_color(hsla(fg))
            .hover(|d| {
                d.bg(rgba(theme.surface_hover))
                    .border_color(hsla(theme.accent))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(hsla(fg)),
            )
            .child(label)
    }

    /// 「コピー」= dispatch 経由でクリップボードへ（CLI / MCP の copy と同一経路）
    pub(crate) fn copy_command_card(&mut self, card: u64, index: usize, cx: &mut Context<Self>) {
        match self.command_card_dispatch("copy", card, index, cx) {
            Ok(_) => {
                self.command_card_copied = Some((card, index, std::time::Instant::now()));
                self.command_card_error = None;
            }
            Err(e) => self.report_command_card_error(card, "copy", &e),
        }
        cx.notify();
    }

    /// 「新規ペインで実行」= dispatch 経由で同じタブに分割して実行
    pub(crate) fn run_command_card(&mut self, card: u64, index: usize, cx: &mut Context<Self>) {
        match self.command_card_dispatch("run", card, index, cx) {
            Ok(_) => self.command_card_error = None,
            Err(e) => self.report_command_card_error(card, "run", &e),
        }
        cx.notify();
    }

    /// × 閉じる
    pub(crate) fn dismiss_command_card(&mut self, card: u64, cx: &mut Context<Self>) {
        if let Err(e) = self.command_card_dispatch("dismiss", card, 1, cx) {
            self.report_command_card_error(card, "dismiss", &e);
        }
        if self
            .command_card_copied
            .is_some_and(|(id, _, _)| id == card)
        {
            self.command_card_copied = None;
        }
        cx.notify();
    }

    /// カード操作の dispatch 呼び出し（copy / run / dismiss 共通）
    fn command_card_dispatch(
        &mut self,
        action: &str,
        card: u64,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::ShowCommand {
                action: Some(action.to_string()),
                commands: Vec::new(),
                label: None,
                pane: None,
                card: Some(card),
                index: Some(index),
                focus: None,
            },
            PaneOrigin::User,
        )
        .map_err(|e| e.to_string());
        // UI から dispatch を直接呼ぶので、IPC / MCP ループがやっている後処理を
        // ここで肩代わりする。**これを欠くと run でツリーにペインだけができて
        // PTY が起動しない**（#153 で同じ穴を踏んでいる）
        for (pane, options) in std::mem::take(&mut self.pending_attach) {
            if let Err(e) = self.spawn_session(pane, options, cx) {
                eprintln!("warning: コマンドカードの実行ペインを起動できない: {e}");
                self.remove_pane(pane, cx);
            }
        }
        for (pane, data) in std::mem::take(&mut self.pending_writes) {
            if let Some(session) = self.terminals.get(&pane) {
                session.write(data);
            }
        }
        // コピーは押した瞬間に効いてほしいので、render を待たずここで流す
        self.flush_pending_clipboard(cx);
        result
    }

    /// 失敗は画面に一言 + 理由は診断ログへ（dispatch のエラー文は日本語固定 =
    /// UI 文言の i18n 対象外なのでそのまま画面には出さない）
    fn report_command_card_error(&mut self, card: u64, action: &str, reason: &str) {
        eprintln!("warning: コマンドカードの {action} に失敗: {reason}");
        self.command_card_error = Some((card, std::time::Instant::now()));
    }

    /// クリップボード書き込みの保留分を流す（render から呼ぶ）。
    /// GPUI の clipboard API は `App` を要するため dispatch では積むだけにしている
    pub(crate) fn flush_pending_clipboard(&mut self, cx: &mut Context<Self>) {
        for text in std::mem::take(&mut self.pending_clipboard) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// 閉じたペインのカードを掃除する（2 秒ポーリングから呼ぶ）。
    /// カードは揮発なので、ペインが消えたら残しておく意味がない
    pub(crate) fn prune_command_cards(&mut self) {
        if self.command_cards.is_empty() {
            return;
        }
        let mut alive: std::collections::HashSet<PaneId> = std::collections::HashSet::new();
        for tab in self.workspace.tabs() {
            alive.extend(tab.tree().panes().iter().map(|p| p.id()));
        }
        // バックグラウンド退避中（FR-2.15）のペインは生きている = カードも残す
        alive.extend(self.workspace.shelved_panes().iter().map(|s| s.pane().id()));
        self.command_cards.retain_panes(|p| alive.contains(&p));
    }
}

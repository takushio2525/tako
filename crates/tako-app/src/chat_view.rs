//! チャットビュー — GUI ライク表示モードで claude 対話ペインを会話として描く（#691 / #702）
//!
//! 仕様の正は `.agent/plans/2026-07-gui-mode.md` §2.3 / §2.4 / §3.1。
//! **表示レイヤだけ**の機能なので、PTY・tmux バックエンド・persist には一切触れない
//! （「ターミナルを表示」に戻せば同じ会話が claude TUI 上に見える。§2.5）。
//!
//! ここが持つのは
//!
//! - 読み取り結果の型（[`ChatPaneState`] / [`ChatMessage`]）と、
//!   transcript の正規化 JSON をそれへ落とす純関数（[`messages_from_json`]）
//! - 描画（`TakoApp::render_chat_pane`）
//!
//! の 2 つだけ。データの取得（`agents::live_claude_sessions_by_backend` /
//! `transcript::read_messages_at`）は main.rs の定期更新に相乗りしていて、
//! **新しいポーリングは 1 つも増やしていない**（§3.1）。

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, MouseButton,
    SharedString,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use super::*;
use crate::file_icons::ui_icon;

/// 会話の読み込み件数（§2.3。古い発話は claude TUI 側 / `tako logs` で辿れる）
pub(crate) const CHAT_TAIL: usize = 50;

/// コンテキスト使用率がこれを超えたら残量バーを警告色にする（§2.3）
const CTX_WARN_PERCENT: f64 = 80.0;

/// 発話者
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatRole {
    User,
    Assistant,
}

/// assistant が使ったツール 1 件（PWA の ToolCard 相当。折りたたみ表示）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatTool {
    pub name: String,
    pub summary: String,
}

/// 会話 1 件（`transcript::normalize_lines` の 1 エントリに対応）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    /// 思考（既定は折りたたみ）
    pub thinking: Option<String>,
    pub tools: Vec<ChatTool>,
    /// 内容から決まる安定キー。**折りたたみ状態の記憶**と md パースキャッシュに使う。
    /// 添字ではなく内容で持つので、上限で古い発話が押し出されても展開状態がずれない
    pub key: u64,
}

/// 1 ペイン分のチャット状態（2 秒ごとの読み取り結果のキャッシュ）。
///
/// `Rc` で持つ理由: 描画のたびに 50 件の発話を clone すると、チャットを開いている
/// だけで毎フレーム数百 KB の確保になる（#168 で潰したのと同じ形の無駄）
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ChatPaneState {
    /// 描画対象の claude セッション（live 解決で確定したもの）
    pub session_id: String,
    /// 解決済みの transcript パス（再走査を避けるため覚えておく）
    pub transcript: Option<std::path::PathBuf>,
    /// 最後に読んだ transcript の (mtime, サイズ)。変化したときだけ読み直す（§3.1）
    pub stamp: Option<(std::time::SystemTime, u64)>,
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub ctx_percent: Option<f64>,
    /// 生成中か（画面採取 `claude_tui::is_busy` + `claude agents --json` の status）
    pub busy: bool,
    /// busy 中に打たれた指示が claude のキューに滞留している（#572）
    pub queued: bool,
    /// worker ペイン = 入力を出さない読み取り専用（§2.4）
    pub read_only: bool,
    /// transcript を読めない等の状態（表示用。会話が無いだけのときは None）
    pub notice: Option<String>,
}

impl ChatPaneState {
    /// ヘッダに出すモデル名（無ければ「Claude」）
    pub(crate) fn model_label(&self) -> String {
        self.model
            .as_deref()
            .map(short_model_name)
            .unwrap_or_else(|| "Claude".to_string())
    }

    /// コンテキスト残量バーの (残量 0.0〜1.0, 警告か)。ctx% が取れないときは None
    pub(crate) fn ctx_gauge(&self) -> Option<(f32, bool)> {
        let used = self.ctx_percent?.clamp(0.0, 100.0);
        Some((((100.0 - used) / 100.0) as f32, used >= CTX_WARN_PERCENT))
    }
}

/// モデル ID を人が読む短い名前へ（`claude-opus-5` → `Opus 5`、
/// `claude-haiku-4-5-20251001` → `Haiku 4.5`）。
///
/// **確実に解ける形だけ**短くし、少しでも崩れていたら原文を出す。
/// 中途半端に省略すると別モデルに見える（`claude-opus-4-6[1m]` を「Opus 4」と
/// 出すような事故）ほうが、長い ID がそのまま出るより悪い
pub(crate) fn short_model_name(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else {
        return model.to_string();
    };
    let mut parts = rest.split('-');
    let known = ["opus", "sonnet", "haiku", "fable"];
    let Some(family) = parts.next().filter(|f| known.contains(f)) else {
        return model.to_string();
    };
    let mut version: Vec<&str> = Vec::new();
    for part in parts {
        let digits = !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
        // 6 桁以上の数字は日付スタンプ（20251001）なので名前に含めない
        if digits && part.len() >= 6 {
            break;
        }
        if !digits {
            return model.to_string();
        }
        version.push(part);
    }
    if version.is_empty() {
        return model.to_string();
    }
    let mut label = family.to_string();
    label[..1].make_ascii_uppercase();
    format!("{label} {}", version.join("."))
}

/// `transcript::read_messages_at` の正規化 JSON を描画用の型へ落とす（純関数）。
///
/// 空の発話（text も thinking も tools も無い）は捨てる。正規化側で弾かれているが、
/// 形の違う transcript を読まされても**空の吹き出しを並べない**ための保険
pub(crate) fn messages_from_json(values: &[serde_json::Value]) -> Vec<ChatMessage> {
    values
        .iter()
        .filter_map(|v| {
            let role = match v["role"].as_str() {
                Some("user") => ChatRole::User,
                Some("assistant") => ChatRole::Assistant,
                _ => return None,
            };
            let text = v["text"].as_str().unwrap_or_default().to_string();
            let thinking = v["thinking"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let tools: Vec<ChatTool> = v["tools"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|t| ChatTool {
                            name: t["name"].as_str().unwrap_or("tool").to_string(),
                            summary: t["summary"].as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if text.trim().is_empty() && thinking.is_none() && tools.is_empty() {
                return None;
            }
            let key = message_key(role, &text, thinking.as_deref(), &tools);
            Some(ChatMessage {
                role,
                text,
                thinking,
                tools,
                key,
            })
        })
        .collect()
}

/// 内容から決まる安定キー（同じ発話なら再読込後も同じ値）
fn message_key(role: ChatRole, text: &str, thinking: Option<&str>, tools: &[ChatTool]) -> u64 {
    let mut hasher = DefaultHasher::new();
    (role == ChatRole::User).hash(&mut hasher);
    text.hash(&mut hasher);
    thinking.hash(&mut hasher);
    for tool in tools {
        tool.name.hash(&mut hasher);
        tool.summary.hash(&mut hasher);
    }
    hasher.finish()
}

/// 折りたたみ要素の識別（展開状態の記憶キー）。
/// メッセージの内容キーと組にするので、再読込で並びが変わっても状態が付いて回る
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChatSection {
    Thinking,
    Tool(usize),
}

impl ChatSection {
    /// 要素 ID 用の安定した数値
    fn slot(self) -> u64 {
        match self {
            Self::Thinking => 0,
            Self::Tool(i) => 1 + i as u64,
        }
    }
}

impl TakoApp {
    /// このペインのチャット状態（判定表が Chat を返すのと同じ条件で存在する）
    pub(crate) fn chat_state(&self, pane_id: PaneId) -> Option<&ChatPaneState> {
        self.chat_panes.get(&pane_id).map(|s| s.as_ref())
    }

    fn chat_expanded(&self, pane_id: PaneId, key: u64, section: ChatSection) -> bool {
        self.chat_expanded.contains(&(pane_id, key, section))
    }

    fn toggle_chat_section(&mut self, pane_id: PaneId, key: u64, section: ChatSection) {
        let entry = (pane_id, key, section);
        if !self.chat_expanded.remove(&entry) {
            self.chat_expanded.insert(entry);
        }
    }

    /// 下端に追従しているか（既定は追従）。手動で上へスクロールすると外れる
    pub(crate) fn chat_following(&self, pane_id: PaneId) -> bool {
        self.chat_follow.get(&pane_id).copied().unwrap_or(true)
    }

    /// チャット本文のホイール操作（描画のリスナーとセルフテストが**同じ経路**を通る）。
    /// 上方向のスクロールで追従を外す。下端まで戻したときの復帰は render 側が見る
    /// （ホイールの時点ではまだスクロールが適用されていないため）
    pub(crate) fn on_chat_scroll(&mut self, pane_id: PaneId, event: &gpui::ScrollWheelEvent) {
        let dy = match event.delta {
            gpui::ScrollDelta::Pixels(d) => f32::from(d.y),
            gpui::ScrollDelta::Lines(d) => d.y,
        };
        if dy > 0.0 {
            self.chat_follow.insert(pane_id, false);
        }
    }

    /// チャット表示のペイン 1 枚（ヘッダ + 会話。入力欄は G3）
    pub(crate) fn render_chat_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let Some(state) = self.chat_panes.get(&pane_id).cloned() else {
            // 判定と描画の間でチャットが外れた場合（次フレームでターミナル表示になる）。
            // 空でもペインの矩形は占めておく（隣のペインの位置を動かさない）
            return div()
                .id(("pane", pane_id.as_u64()))
                .absolute()
                .left(relative(rect.x))
                .top(relative(rect.y))
                .w(relative(rect.width))
                .h(relative(rect.height))
                .bg(rgba(theme.background));
        };
        let width = f32::from(area.size.width);
        let compact = width < 420.0;

        // 追従の再開: 手動スクロールで外れていても、下端まで戻ったら追従に復帰する
        // （前フレームの実測 bounds で判断する。リモートのリーダービュー #63 と同じ振る舞い）
        if !self.chat_following(pane_id) && self.chat_scroll_at_bottom(pane_id) {
            self.chat_follow.insert(pane_id, true);
        }
        let following = self.chat_following(pane_id);
        let scroll_handle = self.chat_scroll_handles.entry(pane_id).or_default().clone();
        // **内容が変わったフレームだけ**下端へ寄せる。毎フレーム寄せると、
        // 追従を外す前の 1 フレームでユーザーのホイール操作を巻き戻してしまう
        if self.chat_content_changed(pane_id, &state) && following {
            scroll_handle.scroll_to_bottom();
        }

        let messages: Vec<gpui::AnyElement> = state
            .messages
            .iter()
            .map(|m| self.render_chat_message(pane_id, m, compact, cx))
            .collect();
        let empty = messages.is_empty();

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .rounded(px(7.0))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla_alpha(theme.accent, 0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                }])
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    let _ = this.workspace.active_tab_mut().tree_mut().focus(pane_id);
                    cx.notify();
                }),
            )
            .child(self.render_chat_header(pane_id, &state, focused, compact, cx))
            .when(state.read_only, |d| {
                d.child(self.render_chat_readonly_note())
            })
            .when_some(state.notice.clone(), |d, notice| {
                d.child(
                    div()
                        .flex_none()
                        .w_full()
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_size(px(11.0))
                        .text_color(hsla(theme.text_muted))
                        .bg(rgba(theme.surface_0))
                        .child(SharedString::from(notice)),
                )
            })
            .child(
                div()
                    .id(("chat-body", pane_id.as_u64()))
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll_handle)
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .px(px(if compact { 10.0 } else { 16.0 }))
                    .py(px(12.0))
                    // 上へスクロールしたら追従を外す（新着で勝手に飛ばない）
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &gpui::ScrollWheelEvent, _, _| {
                            this.on_chat_scroll(pane_id, event);
                        },
                    ))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.0))
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(
                                    crate::ui_text::ui_mode::chat_empty().to_string(),
                                )),
                        )
                    })
                    .children(messages),
            )
    }

    /// ヘッダ: モデル名 / 状態 / コンテキスト残量 / 「ターミナルを表示」/ ×
    fn render_chat_header(
        &self,
        pane_id: PaneId,
        state: &ChatPaneState,
        focused: bool,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        let dot_color = if state.busy {
            theme.yellow
        } else {
            theme.green
        };
        let status_dot = div()
            .w(px(7.0))
            .h(px(7.0))
            .flex_none()
            .rounded_full()
            .bg(hsla(dot_color));
        // 生成中は明滅させる（タブバーの実行中ドットと同じ表現。新しい UI 表現を増やさない）
        let status_dot = if state.busy {
            status_dot
                .with_animation(
                    ("chat-busy-pulse", pane_id.as_u64()),
                    Animation::new(Duration::from_secs(2)).repeat(),
                    |el, t| el.opacity(1.0 - 0.65 * (std::f32::consts::PI * t).sin()),
                )
                .into_any_element()
        } else {
            status_dot.into_any_element()
        };

        let status_label = if state.queued {
            txt::chat_status_queued()
        } else if state.busy {
            txt::chat_status_busy()
        } else {
            txt::chat_status_idle()
        };

        div()
            .h(px(PANE_TITLE_BAR))
            .flex_none()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .bg(rgba(if focused {
                theme.surface_2
            } else {
                theme.surface_0
            }))
            .border_b_1()
            .border_color(hsla(if focused {
                theme.border_default
            } else {
                theme.border_subtle
            }))
            .text_size(px(11.0))
            .child(
                svg()
                    .path(ui_icon::CHAT_BUBBLE)
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(hsla(theme.accent)),
            )
            .child(
                div()
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(theme.foreground))
                    .child(SharedString::from(state.model_label())),
            )
            .child(status_dot)
            .when(!compact, |d| {
                d.child(
                    div()
                        .flex_none()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(status_label.to_string())),
                )
            })
            .child(div().flex_grow(1.0))
            .when_some(
                state.ctx_gauge().filter(|_| !compact),
                |d, (left, warn): (f32, bool)| {
                    let bar_color = if warn { theme.red } else { theme.accent };
                    d.child(
                        div()
                            .flex()
                            .flex_none()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(hsla(if warn {
                                        theme.red
                                    } else {
                                        theme.text_muted
                                    }))
                                    .child(SharedString::from(txt::chat_ctx_label(
                                        (left * 100.0).round() as i32,
                                    ))),
                            )
                            .child(
                                div()
                                    .w(px(56.0))
                                    .h(px(5.0))
                                    .rounded(px(3.0))
                                    .bg(rgba(theme.surface_1))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(left.clamp(0.0, 1.0)))
                                            .bg(hsla(bar_color)),
                                    ),
                            ),
                    )
                },
            )
            // 「ターミナルを表示」= スターターの「コマンド入力へ」と同じ揮発解除（§2.3）
            .child(
                div()
                    .id(("chat-to-terminal", pane_id.as_u64()))
                    .flex()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .text_color(hsla(theme.text_secondary))
                    .hover(|d| {
                        d.bg(rgba(theme.surface_hover))
                            .border_color(hsla(theme.border_default))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.starter_action(
                            pane_id,
                            tako_core::ui_mode::StarterAction::UseTerminal,
                            cx,
                        );
                    }))
                    .child(
                        svg()
                            .path(ui_icon::PROMPT)
                            .w(px(11.0))
                            .h(px(11.0))
                            .flex_none()
                            .text_color(hsla(theme.text_secondary)),
                    )
                    .when(!compact, |d| {
                        d.child(SharedString::from(txt::chat_show_terminal().to_string()))
                    }),
            )
            .child(
                div()
                    .id(("chat-close", pane_id.as_u64()))
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba_alpha(theme.red, 0.25)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.close_pane_with_confirm(
                            pane_id,
                            event.modifiers().platform,
                            CloseOrigin::PaneButton,
                            cx,
                        );
                    }))
                    .child(
                        svg()
                            .path(ui_icon::CLOSE)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(hsla(theme.text_muted)),
                    ),
            )
            .into_any_element()
    }

    /// worker ペインの説明行（§2.4。入力欄の代わりに「自動で動いている」ことを伝える）
    fn render_chat_readonly_note(&self) -> gpui::AnyElement {
        let theme = &self.theme;
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(5.0))
            .bg(rgba_alpha(theme.accent, 0.10))
            .border_b_1()
            .border_color(hsla(theme.border_subtle))
            .child(
                svg()
                    .path(ui_icon::ORCH)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(hsla(theme.accent)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_secondary))
                    .child(SharedString::from(
                        crate::ui_text::ui_mode::chat_worker_note().to_string(),
                    )),
            )
            .into_any_element()
    }

    /// 発話 1 件（user = 背景ブロック / assistant = 地の文 md）
    fn render_chat_message(
        &mut self,
        pane_id: PaneId,
        message: &ChatMessage,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        match message.role {
            ChatRole::User => div()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .justify_end()
                .w_full()
                .child(
                    div()
                        .max_w(relative(if compact { 0.95 } else { 0.82 }))
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_subtle))
                        .text_color(hsla(theme.foreground))
                        .child(SharedString::from(message.text.clone())),
                )
                .into_any_element(),
            ChatRole::Assistant => {
                let mut body = div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(4.0));
                if let Some(thinking) = message.thinking.clone() {
                    body = body.child(self.render_chat_fold(
                        pane_id,
                        message.key,
                        ChatSection::Thinking,
                        crate::ui_text::ui_mode::chat_thinking().to_string(),
                        None,
                        thinking,
                        cx,
                    ));
                }
                if !message.text.trim().is_empty() {
                    let blocks = self.chat_md_blocks(message.key, &message.text);
                    let (elements, _layouts) =
                        crate::md_view::render_document(&theme, &blocks, None);
                    body = body.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .w_full()
                            .children(elements),
                    );
                }
                for (index, tool) in message.tools.iter().enumerate() {
                    body = body.child(self.render_chat_fold(
                        pane_id,
                        message.key,
                        ChatSection::Tool(index),
                        tool.name.clone(),
                        Some(tool.summary.clone()),
                        tool.summary.clone(),
                        cx,
                    ));
                }
                body.into_any_element()
            }
        }
    }

    /// 折りたたみカード（thinking / tool_use 共通）。
    /// 閉じているときは 1 行、開くと本文を出す
    #[allow(clippy::too_many_arguments)]
    fn render_chat_fold(
        &self,
        pane_id: PaneId,
        key: u64,
        section: ChatSection,
        title: String,
        preview: Option<String>,
        body: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let expanded = self.chat_expanded(pane_id, key, section);
        // ハッシュの上位ビットを落とさないように混ぜる（同一フレーム内で衝突しなければよい）
        let element_id = key.rotate_left(11).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ pane_id.as_u64()
            ^ section.slot();
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .w_full()
            .my(px(2.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(hsla(theme.border_subtle))
            .bg(rgba(theme.surface_0))
            .overflow_hidden()
            .child(
                div()
                    .id(("chat-fold", element_id))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .w_full()
                    .px(px(8.0))
                    .py(px(5.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_chat_section(pane_id, key, section);
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(if expanded {
                                ui_icon::CHEVRON_DOWN
                            } else {
                                ui_icon::CHEVRON_RIGHT
                            })
                            .w(px(11.0))
                            .h(px(11.0))
                            .flex_none()
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(theme.text_secondary))
                            .child(SharedString::from(title)),
                    )
                    .when_some(preview.filter(|_| !expanded), |d, preview: String| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(theme.font_family.clone())
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(preview)),
                        )
                    }),
            )
            .when(expanded, |d| {
                d.child(
                    div()
                        .w_full()
                        .px(px(10.0))
                        .py(px(7.0))
                        .border_t_1()
                        .border_color(hsla(theme.border_subtle))
                        .font_family(theme.font_family.clone())
                        .text_size(px(11.0))
                        .text_color(hsla(theme.text_secondary))
                        .child(SharedString::from(body)),
                )
            })
            .into_any_element()
    }

    /// メッセージ本文の md ブロック（内容キーでキャッシュ。#702 / 仕様書 §5）。
    ///
    /// パースは pulldown-cmark + コードブロックの syntect ハイライトを通るので
    /// 毎フレームやると重い。内容が変わらない限り 1 度で済ませる
    fn chat_md_blocks(
        &mut self,
        key: u64,
        text: &str,
    ) -> std::rc::Rc<Vec<crate::preview::MdBlock>> {
        if let Some(blocks) = self.chat_md_cache.get(&key) {
            return blocks.clone();
        }
        let blocks = std::rc::Rc::new(crate::preview::markdown_blocks(text));
        self.chat_md_cache.insert(key, blocks.clone());
        blocks
    }

    /// 前回描画から会話が変わったか（下端追従を発火させるかの判断）。
    /// 末尾の発話キーと件数で見るので、生成中に本文が伸びるたびに真になる
    fn chat_content_changed(&mut self, pane_id: PaneId, state: &ChatPaneState) -> bool {
        let key = state.messages.last().map(|m| m.key).unwrap_or(0)
            ^ (state.messages.len() as u64).rotate_left(32);
        if self.chat_content_keys.get(&pane_id) == Some(&key) {
            return false;
        }
        self.chat_content_keys.insert(pane_id, key);
        true
    }

    /// スクロールが下端にあるか（追従の再開判定）。
    /// `child_bounds` はスクロールオフセットを含まない座標なので offset を足して比べる
    fn chat_scroll_at_bottom(&self, pane_id: PaneId) -> bool {
        let Some(handle) = self.chat_scroll_handles.get(&pane_id) else {
            return true;
        };
        let count = handle.children_count();
        if count == 0 {
            return true;
        }
        let Some(last) = handle.bounds_for_item(count - 1) else {
            return true;
        };
        let viewport = handle.bounds();
        let distance = f32::from(last.bottom() + handle.offset().y - viewport.bottom());
        distance <= CHAT_FOLLOW_EPSILON
    }
}

/// 下端とみなす余白（px）。1 行ぶんより小さくして「ほぼ下端」で追従を再開する
const CHAT_FOLLOW_EPSILON: f32 = 8.0;

// --- 読み取り（定期更新への相乗り。§3.1） ---

/// 1 ペイン分の更新材料（UI スレッドで組み立てる。**サブプロセスは起動しない**）。
///
/// 画面採取（busy / キュー滞留）はペインのスクリーンがプロセス内にあるので
/// `visible_lines()` で足りる。ここで tmux を叩くと 2 秒ごとの UI 専有になる（#212 の教訓）
pub(crate) struct ChatRefreshTarget {
    pane: PaneId,
    /// tmux バックエンドセッション名（live 解決の対応キー）
    backend: String,
    /// このペインに実行中の子プロセスがある（= claude が生きている。#372 の判定を流用）
    agent_running: bool,
    read_only: bool,
    /// 画面が生成中に見える（`claude agents --json` が状態を返せないときの拠り所）
    screen_busy: bool,
    queued: bool,
    /// ペインの TUI フッターから読んだモデル名 / コンテキスト使用率（#357 の採取を流用）。
    /// `claude agents --json` は版によって model / contextPercentUsed を返さない
    /// （実測: claude 2.1.220 は両方とも欠落）ので、こちらが実データの拠り所になる
    screen_model: Option<String>,
    screen_ctx_percent: Option<f64>,
    /// 前回の読み取り結果（mtime ゲートに使う）
    prev_session: Option<String>,
    prev_path: Option<std::path::PathBuf>,
    prev_stamp: Option<(std::time::SystemTime, u64)>,
}

impl ChatRefreshTarget {
    /// 対象ペイン（セルフテストが「読みに行かないこと」を確かめるのに使う）
    pub(crate) fn pane(&self) -> PaneId {
        self.pane
    }
}

/// 背景スレッドが返す 1 ペイン分の結果
pub(crate) struct ChatRefreshResult {
    pane: PaneId,
    /// None = このペインはチャットにしない（claude が居ない / 終了した）
    chat: Option<ChatRefreshData>,
}

pub(crate) struct ChatRefreshData {
    session_id: String,
    transcript: Option<std::path::PathBuf>,
    stamp: Option<(std::time::SystemTime, u64)>,
    /// None = transcript に変化が無いので前回のメッセージを使い回す
    messages: Option<Vec<ChatMessage>>,
    model: Option<String>,
    ctx_percent: Option<f64>,
    /// `claude agents --json` の status（取れないときは None → 画面採取へ委ねる）
    agent_status: Option<String>,
    notice: Option<String>,
    read_only: bool,
    screen_busy: bool,
    queued: bool,
    screen_model: Option<String>,
    screen_ctx_percent: Option<f64>,
}

impl TakoApp {
    /// 更新対象の収集（UI スレッド）。terminal モードでは即空を返すので、
    /// 既存ユーザーの定期更新にはコストが乗らない
    pub(crate) fn collect_chat_targets(&self) -> Vec<ChatRefreshTarget> {
        if !self.ui_mode.is_gui() {
            return Vec::new();
        }
        let roles: std::collections::HashMap<PaneId, Option<String>> = self
            .workspace
            .tabs()
            .iter()
            .flat_map(|t| t.tree().panes())
            .map(|p| (p.id(), p.role().map(|r| r.to_string())))
            .collect();
        self.backend_sessions
            .iter()
            .filter_map(|(pane, backend)| {
                let role = roles.get(pane)?;
                let session = self.terminals.get(pane);
                // alt screen（vim 等）は判定表でターミナル表示に落ちるので読みに行かない。
                // **外側のフラグではなく中身の判定を使う**（tmux クライアントは常に
                // alt screen なので、素直に見るとバックエンドペインが全部除外される）
                if self.pane_inner_alt_screen(*pane) {
                    return None;
                }
                let lines = session.map(|s| s.visible_lines()).unwrap_or_default();
                let metrics = session.and_then(|s| s.agent_metrics());
                let previous = self.chat_panes.get(pane);
                Some(ChatRefreshTarget {
                    pane: *pane,
                    backend: backend.clone(),
                    agent_running: self.busy_backend_sessions.contains(backend),
                    read_only: role
                        .as_deref()
                        .is_some_and(tako_core::ui_mode::is_read_only_role),
                    screen_busy: tako_control::claude_tui::is_busy(&lines),
                    queued: tako_control::claude_tui::queued_messages_pending(&lines),
                    screen_model: metrics.as_ref().and_then(|m| m.model.clone()),
                    screen_ctx_percent: metrics.and_then(|m| m.ctx_percent).map(f64::from),
                    prev_session: previous.map(|p| p.session_id.clone()),
                    prev_path: previous.and_then(|p| p.transcript.clone()),
                    prev_stamp: previous.and_then(|p| p.stamp),
                })
            })
            .collect()
    }

    /// 読み取り結果の反映（UI スレッド）。変化が無ければ `cx.notify()` もしない
    pub(crate) fn apply_chat_refresh(&mut self, results: Vec<ChatRefreshResult>) -> bool {
        let mut changed = false;
        for result in results {
            let Some(data) = result.chat else {
                changed |= self.chat_panes.remove(&result.pane).is_some();
                self.chat_follow.remove(&result.pane);
                continue;
            };
            let previous = self.chat_panes.get(&result.pane);
            let messages = match data.messages {
                Some(messages) => messages,
                // transcript に変化なし = 前回の内容をそのまま使う（再パースもしない）
                None => previous.map(|p| p.messages.clone()).unwrap_or_default(),
            };
            let busy = match data.agent_status.as_deref() {
                // claude 自身の申告が取れたらそれが正（#571）
                Some(status) => status == "busy",
                None => data.screen_busy,
            };
            let state = ChatPaneState {
                session_id: data.session_id,
                transcript: data.transcript,
                stamp: data.stamp,
                messages,
                // agents が返せば優先（将来版）、返さなければ画面採取（現行版）
                model: data.model.or(data.screen_model),
                ctx_percent: data.ctx_percent.or(data.screen_ctx_percent),
                busy,
                queued: data.queued,
                read_only: data.read_only,
                notice: data.notice,
            };
            let same = previous.is_some_and(|p| **p == state);
            if !same {
                self.chat_panes.insert(result.pane, std::rc::Rc::new(state));
                changed = true;
            }
        }
        if changed {
            self.prune_chat_caches();
        }
        changed
    }

    /// 表示中の発話に紐づかない md パース結果と折りたたみ状態を捨てる
    fn prune_chat_caches(&mut self) {
        let live: std::collections::HashSet<u64> = self
            .chat_panes
            .values()
            .flat_map(|s| s.messages.iter().map(|m| m.key))
            .collect();
        self.chat_md_cache.retain(|key, _| live.contains(key));
        self.chat_expanded.retain(|(_, key, _)| live.contains(key));
    }

    /// ペインが消えたときの後始末（プレビューの scroll handle と同じ扱い）。
    /// スターターの揮発解除フラグ（#694）も一緒に落とす — ペイン ID は再利用される
    /// ことがあり（#390）、残していると次に同じ番号を得たペインが GUI モードでも
    /// ターミナル表示のまま始まってしまう
    pub(crate) fn drop_gui_pane_state(&mut self, pane_id: PaneId) {
        self.chat_panes.remove(&pane_id);
        self.chat_follow.remove(&pane_id);
        self.chat_scroll_handles.remove(&pane_id);
        self.chat_expanded.retain(|(pane, _, _)| *pane != pane_id);
        self.chat_content_keys.remove(&pane_id);
        self.starter_released.remove(&pane_id);
    }
}

/// 背景スレッドでの読み取り（tmux / ps / claude agents / transcript のファイル I/O）。
///
/// `claude agents --json` は TTL キャッシュ + sticky 解決（#466）の
/// `live_claude_sessions_by_backend` を 1 回だけ呼ぶ。transcript は
/// **mtime とサイズが変わったときだけ**読み直す
pub(crate) fn load_chat_refresh(targets: Vec<ChatRefreshTarget>) -> Vec<ChatRefreshResult> {
    if targets.is_empty() {
        return Vec::new();
    }
    // どのペインにも実行中の子プロセスが無ければ claude は 1 つも動いていない
    // （`chat_session` が agent_running を必須にしているため結果は全て None）。
    // その場合は tmux / ps / claude agents を**一切起動しない**: GUI モードで
    // アイドルなシェルだけが並んでいる状態（スターター表示）でのコスト増をゼロにする
    if targets.iter().all(|t| !t.agent_running) {
        return targets
            .into_iter()
            .map(|t| ChatRefreshResult {
                pane: t.pane,
                chat: None,
            })
            .collect();
    }
    let live = tako_control::agents::live_claude_sessions_by_backend();
    targets
        .into_iter()
        .map(|target| {
            let session = live.get(&target.backend);
            let eligibility = tako_core::ui_mode::ChatEligibility {
                session_id: session.map(|s| s.session_id.as_str()),
                interactive: session.is_some_and(|s| s.interactive),
                agent_running: target.agent_running,
            };
            let Some(session_id) = tako_core::ui_mode::chat_session(eligibility) else {
                return ChatRefreshResult {
                    pane: target.pane,
                    chat: None,
                };
            };
            let session_id = session_id.to_string();
            let same_session = target.prev_session.as_deref() == Some(session_id.as_str());
            // 解決済みパスの使い回し（同じセッションで実在する間だけ）
            let path = target
                .prev_path
                .filter(|p| same_session && p.is_file())
                .or_else(|| tako_control::transcript::find_transcript(&session_id));
            let stamp = path.as_deref().and_then(file_stamp);
            let unchanged = same_session && stamp.is_some() && stamp == target.prev_stamp;
            let (messages, notice) = match (&path, unchanged) {
                (Some(_), true) => (None, None),
                (Some(path), false) => {
                    match tako_control::transcript::read_messages_at(path.as_path(), CHAT_TAIL) {
                        Ok(values) => (Some(messages_from_json(&values)), None),
                        Err(e) => (Some(Vec::new()), Some(e)),
                    }
                }
                // 会話ファイルがまだ無い（起動直後の新規セッション）
                (None, _) => (
                    Some(Vec::new()),
                    Some(crate::ui_text::ui_mode::chat_transcript_pending().to_string()),
                ),
            };
            ChatRefreshResult {
                pane: target.pane,
                chat: Some(ChatRefreshData {
                    session_id,
                    transcript: path,
                    stamp,
                    messages,
                    model: session.and_then(|s| s.model.clone()),
                    ctx_percent: session.and_then(|s| s.ctx_percent),
                    agent_status: session.and_then(|s| s.status.clone()),
                    notice,
                    read_only: target.read_only,
                    screen_busy: target.screen_busy,
                    queued: target.queued,
                    screen_model: target.screen_model,
                    screen_ctx_percent: target.screen_ctx_percent,
                }),
            }
        })
        .collect()
}

/// transcript の (更新時刻, サイズ)。どちらか変われば読み直す
fn file_stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 正規化jsonから発話を組み立てる() {
        let values = vec![
            json!({ "role": "user", "text": "こんにちは" }),
            json!({
                "role": "assistant",
                "text": "はい",
                "thinking": "  考え中  ",
                "tools": [{ "name": "Bash", "summary": "ls -la" }],
            }),
            // 本文が空 = 描く中身が無いので落とす
            json!({ "role": "assistant", "text": "   " }),
            // 未知の role は落とす
            json!({ "role": "system", "text": "x" }),
        ];
        let messages = messages_from_json(&values);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, ChatRole::User);
        assert_eq!(messages[0].text, "こんにちは");
        assert!(messages[0].thinking.is_none());
        assert_eq!(messages[1].role, ChatRole::Assistant);
        assert_eq!(messages[1].thinking.as_deref(), Some("考え中"));
        assert_eq!(messages[1].tools.len(), 1);
        assert_eq!(messages[1].tools[0].name, "Bash");
    }

    #[test]
    fn 発話キーは内容が同じなら不変で違えば変わる() {
        // 折りたたみ状態がこのキーに紐づく。並びが変わっても付いて回ることが要件
        let a = messages_from_json(&[json!({ "role": "assistant", "text": "同じ" })]);
        let b = messages_from_json(&[
            json!({ "role": "user", "text": "先頭に増えた" }),
            json!({ "role": "assistant", "text": "同じ" }),
        ]);
        assert_eq!(a[0].key, b[1].key);
        let c = messages_from_json(&[json!({ "role": "assistant", "text": "違う" })]);
        assert_ne!(a[0].key, c[0].key);
        // role が違えば別の発話
        let d = messages_from_json(&[json!({ "role": "user", "text": "同じ" })]);
        assert_ne!(a[0].key, d[0].key);
    }

    #[test]
    fn コンテキスト残量バーは使用率の裏返し() {
        let state = |ctx: Option<f64>| ChatPaneState {
            ctx_percent: ctx,
            ..Default::default()
        };
        assert_eq!(state(None).ctx_gauge(), None);
        let (left, warn) = state(Some(20.0)).ctx_gauge().expect("値がある");
        assert!((left - 0.8).abs() < 1e-6);
        assert!(!warn);
        // 80% 使用（残り 20%）で警告色へ
        let (left, warn) = state(Some(80.0)).ctx_gauge().expect("値がある");
        assert!((left - 0.2).abs() < 1e-6);
        assert!(warn);
        // 範囲外の値でもバーは 0〜1 に収まる
        let (left, _) = state(Some(140.0)).ctx_gauge().expect("値がある");
        assert_eq!(left, 0.0);
    }

    #[test]
    fn モデル名は既知の形だけ短くする() {
        assert_eq!(short_model_name("claude-opus-5"), "Opus 5");
        assert_eq!(short_model_name("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(short_model_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        // 未知の形は原文のまま（別モデルに見える省略をしない）
        assert_eq!(short_model_name("gpt-5"), "gpt-5");
        assert_eq!(short_model_name("claude-unknown-9"), "claude-unknown-9");
        assert_eq!(short_model_name("claude"), "claude");
        assert_eq!(short_model_name("claude-opus"), "claude-opus");
        // 1M コンテキスト等のサフィックス付きは省略せずそのまま出す
        assert_eq!(
            short_model_name("claude-opus-4-6[1m]"),
            "claude-opus-4-6[1m]"
        );
    }

    #[test]
    fn モデル名が無ければclaudeと出す() {
        assert_eq!(ChatPaneState::default().model_label(), "Claude");
        let state = ChatPaneState {
            model: Some("claude-opus-5".into()),
            ..Default::default()
        };
        assert_eq!(state.model_label(), "Opus 5");
    }
}

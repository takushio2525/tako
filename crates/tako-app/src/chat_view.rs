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

/// 入力欄の最大高さ（px。#716）。長文を書いても会話が全部隠れないように止める
const CHAT_INPUT_MAX_HEIGHT: f32 = 120.0;

/// 発話者
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChatRole {
    User,
    Assistant,
    /// システムが差し込んだ通知（#715）。会話ではないので薄い 1 行に留める
    System,
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
    /// 添付画像の枚数（#715）。本文が空でもプレースホルダを出すのでここが表示の根拠になる
    pub images: usize,
    /// まとめられたシステム通知の件数（#715。`role == System` のときだけ意味を持つ）
    pub notices: u64,
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
    /// 画面に**実在する** permission ダイアログ（#716 / §2.3）。
    /// 表示条件を PWA（#425）と揃える: transcript からの推定は使わない
    /// （auto mode のツール実行中と承認待ちは transcript では区別できない）
    pub permission: Option<tako_control::claude_tui::PermissionDialog>,
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
                // #715: システム通知。未知の role は従来どおり落とす
                Some("system") if v["kind"] == "notice" => ChatRole::System,
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
            // #715: 画像添付は本文が空でも「画像」プレースホルダを出すので数えておく
            let images = v["attachments"]
                .as_array()
                .map(|a| a.iter().filter(|x| x["kind"] == "image").count())
                .unwrap_or(0);
            let notices = v["count"].as_u64().unwrap_or(1).max(1);
            if text.trim().is_empty() && thinking.is_none() && tools.is_empty() && images == 0 {
                return None;
            }
            let key = message_key(role, &text, thinking.as_deref(), &tools, images, notices);
            Some(ChatMessage {
                role,
                text,
                thinking,
                tools,
                images,
                notices,
                key,
            })
        })
        .collect()
}

/// 楽観 echo の発話キー（transcript 由来の同一本文とは別空間にする）
fn echo_key(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    "echo".hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// 内容から決まる安定キー（同じ発話なら再読込後も同じ値）
fn message_key(
    role: ChatRole,
    text: &str,
    thinking: Option<&str>,
    tools: &[ChatTool],
    images: usize,
    notices: u64,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    role.hash(&mut hasher);
    text.hash(&mut hasher);
    thinking.hash(&mut hasher);
    images.hash(&mut hasher);
    notices.hash(&mut hasher);
    for tool in tools {
        tool.name.hash(&mut hasher);
        tool.summary.hash(&mut hasher);
    }
    hasher.finish()
}

/// チャット入力欄の下書き（#716）。ペインごとに持つので書きかけが消えない
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChatInput {
    pub text: String,
    /// キャレット位置（バイト。**必ず文字境界**に丸めて使う。#494 と同じ約束）
    pub cursor: usize,
}

impl ChatInput {
    /// キャレットを文字境界へ丸める（split / drain の panic を構造的に防ぐ）
    fn snap(&mut self) -> usize {
        self.cursor =
            crate::right_panel::floor_char_boundary(&self.text, self.cursor.min(self.text.len()));
        self.cursor
    }
}

/// 送信直後の楽観 echo（#716 / 仕様 §3.1）。
///
/// transcript へ自分の発話が現れるまで 1〜2 秒あるので、その間だけローカルで見せる。
/// 同じ本文が transcript から返ってきたら破棄して transcript を正とする
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatEcho {
    pub text: String,
    pub at: std::time::Instant,
}

/// 楽観 echo を諦める時間（#716）。送達に失敗しても永遠に残らないようにする。
/// PromptFlow の送達確認（#95）は数秒かかることがあるので短くしすぎない
const ECHO_MAX_AGE: Duration = Duration::from_secs(45);

/// これを超える本文の発話は既定で畳む（#716）。
/// スキル本文の注入のような数万文字の「発話」が会話を埋めるのを防ぐ
const LONG_MESSAGE_CHARS: usize = 1200;

/// 送信・承認の失敗表示を維持する時間（カード帯 #703 と同じ流儀）
const ACTION_ERROR_DURATION: Duration = Duration::from_secs(6);

/// スラッシュボタン 1 つ（§2.3。平易なラベル + 実コマンド併記）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashButton {
    /// 会話を軽くする（/compact）
    Compact,
    /// 新しい会話（/clear。確認ダイアログつき）
    Clear,
    /// ヘルプ（/help）
    Help,
}

impl SlashButton {
    /// 実際に claude へ送るコマンド（CLI / MCP からも同じ文字列を送れる = 1:1）
    pub(crate) fn command(self) -> &'static str {
        match self {
            Self::Compact => "/compact",
            Self::Clear => "/clear",
            Self::Help => "/help",
        }
    }

    /// 押しても会話が消えないか（false = 確認を挟む）
    fn safe(self) -> bool {
        !matches!(self, Self::Clear)
    }

    fn label(self) -> &'static str {
        use crate::ui_text::ui_mode as txt;
        match self {
            Self::Compact => txt::chat_slash_compact(),
            Self::Clear => txt::chat_slash_clear(),
            Self::Help => txt::chat_slash_help(),
        }
    }

    /// 表示順（v1 は 3 つ固定）
    const ALL: [Self; 3] = [Self::Compact, Self::Clear, Self::Help];
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

    // --- 入力と送信（#716 / G3。書きは既存 dispatch のみ。§3.2） ---

    /// いまキー入力を受けているチャット入力欄のペイン（#716）。
    ///
    /// フラグだけでなく**そのペインが実際にチャット表示か**まで見る。
    /// フラグだけを信じると、claude が終了してターミナル表示へ戻ったあとも
    /// 打鍵が入力欄に吸われてターミナルへ届かなくなる（#503 で潰した形の再発）
    pub(crate) fn chat_input_pane(&self) -> Option<PaneId> {
        let pane = self.chat_input_focused?;
        let state = self.chat_panes.get(&pane)?;
        (!state.read_only
            // 表示中のタブでフォーカスを持っていること。タブ移動・ペイン移動で
            // 画面から消えたら打鍵はターミナルへ戻す（#503 の不変条件の一部）
            && self.focused_pane() == pane
            && self.pane_display_for(pane) == tako_core::ui_mode::PaneDisplay::Chat)
            .then_some(pane)
    }

    /// 入力欄の下書き（表示・セルフテスト共用）
    pub(crate) fn chat_input_text(&self, pane_id: PaneId) -> &str {
        self.chat_inputs
            .get(&pane_id)
            .map(|i| i.text.as_str())
            .unwrap_or_default()
    }

    /// 入力欄へフォーカスを移す（他のテキスト入力とは排他）
    pub(crate) fn focus_chat_input(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.chat_state(pane_id).is_some_and(|s| s.read_only) {
            return; // worker は read-only（§2.4）
        }
        self.clear_text_input_focus();
        self.chat_input_focused = Some(pane_id);
        if self.focused_pane() != pane_id {
            self.jump_to_pane(pane_id, cx);
            // jump_to_pane はフォーカス移動の一環で入力欄フラグを落とすので立て直す
            self.chat_input_focused = Some(pane_id);
        }
        cx.notify();
    }

    /// 入力欄へ文字列を挿入する（打鍵・IME 確定・貼り付けの全経路がここを通る）。
    /// 制御文字は改行とタブ以外を落とす（TUI へ生の ESC を流し込まない）
    pub(crate) fn chat_input_insert(
        &mut self,
        pane_id: PaneId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let insert: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        if insert.is_empty() {
            return;
        }
        let input = self.chat_inputs.entry(pane_id).or_default();
        let cursor = input.snap();
        input.text.insert_str(cursor, &insert);
        input.cursor = cursor + insert.len();
        cx.notify();
    }

    /// 入力欄のキーハンドラ。true を返すとイベントを消費する。
    ///
    /// git のコミット欄（#487 / #494）と同じ約束で組む: `key_char` を使う（shift 付きの
    /// 大文字が打てるように）/ 修飾なしキーは必ず消費する（ターミナルへ漏らさない）/
    /// キャレットは常に文字境界へ丸める
    pub(crate) fn handle_chat_input_key(
        &mut self,
        pane_id: PaneId,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        // 確認ダイアログ中は Enter / Esc だけ受ける（誤って本文が流れないように）
        if self.chat_clear_confirm == Some(pane_id) {
            match keystroke.key.as_str() {
                "enter" => self.chat_clear_accept(cx),
                "escape" => {
                    self.chat_clear_confirm = None;
                    cx.notify();
                }
                _ => {}
            }
            return true;
        }
        let input = self.chat_inputs.entry(pane_id).or_default();
        let cursor = input.snap();
        match keystroke.key.as_str() {
            // Enter = 送信 / Shift+Enter = 改行（§2.3）。
            // cmd / ctrl + Enter も送信（リモート #429 と同じ操作を受ける）
            "enter" if keystroke.modifiers.shift => {
                input.text.insert(cursor, '\n');
                input.cursor = cursor + 1;
                cx.notify();
                true
            }
            "enter" => {
                self.chat_send_input(pane_id, cx);
                true
            }
            "escape" => {
                self.chat_input_focused = None;
                cx.notify();
                true
            }
            "backspace" => {
                if cursor > 0 {
                    let prev = input.text[..cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    input.text.drain(prev..cursor);
                    input.cursor = prev;
                }
                cx.notify();
                true
            }
            "delete" => {
                if cursor < input.text.len() {
                    let next = cursor
                        + input.text[cursor..]
                            .chars()
                            .next()
                            .map(char::len_utf8)
                            .unwrap_or(0);
                    input.text.drain(cursor..next);
                }
                cx.notify();
                true
            }
            "left" => {
                input.cursor = input.text[..cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                cx.notify();
                true
            }
            "right" => {
                if cursor < input.text.len() {
                    input.cursor = cursor
                        + input.text[cursor..]
                            .chars()
                            .next()
                            .map(char::len_utf8)
                            .unwrap_or(0);
                }
                cx.notify();
                true
            }
            // 複数行なので Home / End は「その行の端」へ（上下は行移動ではなく端へ倒す。
            // 行移動は折り返しを含む幾何が必要で、v1 では持たない）
            "home" | "up" => {
                input.cursor = input.text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
                cx.notify();
                true
            }
            "end" | "down" => {
                input.cursor = input.text[cursor..]
                    .find('\n')
                    .map(|i| cursor + i)
                    .unwrap_or(input.text.len());
                cx.notify();
                true
            }
            // cmd / ctrl 付きはアプリのキーバインド（⌘V 等）へ通す
            _ if keystroke.modifiers.platform || keystroke.modifiers.control => false,
            _ => {
                if let Some(ch) = keystroke.key_char.as_deref() {
                    if !ch.is_empty() && !ch.chars().any(char::is_control) {
                        self.chat_input_insert(pane_id, ch, cx);
                        return true;
                    }
                }
                // 空白は key_char が来ないことがある（#487 で実機観測）
                if keystroke.key == "space" {
                    self.chat_input_insert(pane_id, " ", cx);
                    return true;
                }
                true
            }
        }
    }

    /// 入力欄の内容を送信する（Enter / 送信ボタン共通）
    pub(crate) fn chat_send_input(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let text = self.chat_input_text(pane_id).trim().to_string();
        if text.is_empty() {
            return; // 空送信は無視（Enter 連打で claude に空行を送らない）
        }
        if self.chat_send_text(pane_id, &text, cx) {
            self.chat_inputs.remove(&pane_id);
        }
    }

    /// スラッシュボタンの押下（#716 / §2.3）。
    /// 破壊的なもの（/clear）は確認を挟み、それ以外は即送信する
    pub(crate) fn chat_slash_action(
        &mut self,
        pane_id: PaneId,
        button: SlashButton,
        cx: &mut Context<Self>,
    ) {
        if !button.safe() {
            self.chat_clear_confirm = Some(pane_id);
            cx.notify();
            return;
        }
        self.chat_send_text(pane_id, button.command(), cx);
    }

    /// 「新しい会話」の確認 OK
    pub(crate) fn chat_clear_accept(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.chat_clear_confirm.take() else {
            return;
        };
        self.chat_send_text(pane_id, SlashButton::Clear.command(), cx);
    }

    /// テキストを claude へ送る（**唯一の書き経路**）。
    ///
    /// 実体は既存の `Send` dispatch なので、送達確認ループ（#95）・busy 中のキュー
    /// 滞留（#572）・スラッシュコマンドの扱いがそのまま効く。CLI `tako send` /
    /// MCP `tako_send_text` と同一コードパス = 開発不変条件を構造で満たす。
    /// 成功したら楽観 echo を積む（transcript 反映のラグを隠す。§3.1）
    fn chat_send_text(&mut self, pane_id: PaneId, text: &str, cx: &mut Context<Self>) -> bool {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Send {
                pane: Some(pane_id.as_u64()),
                text: text.to_string(),
                newline: true,
                tmux_session: None,
                await_prompt: false,
            },
            PaneOrigin::User,
        );
        match result {
            Ok(_) => {
                self.chat_echo.entry(pane_id).or_default().push(ChatEcho {
                    text: text.to_string(),
                    at: std::time::Instant::now(),
                });
                self.chat_action_error = None;
                // 送った直後に「考え中」を出す（transcript / 画面採取の 2 秒を待たない）
                if let Some(state) = self.chat_panes.get(&pane_id) {
                    let mut next = (**state).clone();
                    next.busy = true;
                    self.chat_panes.insert(pane_id, std::rc::Rc::new(next));
                }
                cx.notify();
                true
            }
            Err(e) => {
                self.chat_action_error = Some((
                    pane_id,
                    crate::ui_text::ui_mode::chat_send_failed(&e.to_string()),
                    std::time::Instant::now(),
                ));
                cx.notify();
                false
            }
        }
    }

    /// 承認カードのボタン押下（#716 / §2.3）。
    /// `OrchestratorRespond` はダイアログの実在を再検証してから選択キーを送る（#319）
    fn chat_respond(&mut self, pane_id: PaneId, choice: usize, cx: &mut Context<Self>) {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::OrchestratorRespond {
                pane_id: pane_id.as_u64(),
                choice: choice.to_string(),
                caller_role: None,
            },
            PaneOrigin::User,
        );
        match result {
            Ok(_) => {
                self.chat_action_error = None;
                // 押した瞬間にカードを消す（次の採取まで残ると二重に押せてしまう）
                if let Some(state) = self.chat_panes.get(&pane_id) {
                    let mut next = (**state).clone();
                    next.permission = None;
                    self.chat_panes.insert(pane_id, std::rc::Rc::new(next));
                }
            }
            Err(e) => {
                self.chat_action_error = Some((
                    pane_id,
                    crate::ui_text::ui_mode::chat_respond_failed(&e.to_string()),
                    std::time::Instant::now(),
                ));
            }
        }
        cx.notify();
    }

    /// 表示する発話（transcript + 生きている楽観 echo）。
    ///
    /// transcript に同じ本文の user 発話が現れた echo は捨てる。
    /// 時間切れ（[`ECHO_MAX_AGE`]）の echo も捨てるので、送達に失敗しても残り続けない
    pub(crate) fn chat_visible_messages(
        &self,
        pane_id: PaneId,
        state: &ChatPaneState,
    ) -> Vec<ChatMessage> {
        let mut messages = state.messages.clone();
        let Some(echoes) = self.chat_echo.get(&pane_id) else {
            return messages;
        };
        for echo in echoes {
            if echo.at.elapsed() >= ECHO_MAX_AGE {
                continue;
            }
            if state
                .messages
                .iter()
                .any(|m| m.role == ChatRole::User && m.text.trim() == echo.text.trim())
            {
                continue;
            }
            messages.push(ChatMessage {
                role: ChatRole::User,
                text: echo.text.clone(),
                thinking: None,
                tools: Vec::new(),
                images: 0,
                notices: 1,
                // echo は「まだ届いていない自分の発話」なので専用のキー空間にする
                // （transcript 由来の同じ本文と折りたたみ状態を共有させない）
                key: echo_key(&echo.text),
            });
        }
        messages
    }

    /// この時点で古い echo を捨てる（描画のたびに増え続けないようにする）。
    /// transcript の更新反映（`apply_chat_refresh`）からも呼ぶ
    pub(crate) fn prune_chat_echo(&mut self, pane_id: PaneId) {
        let Some(state) = self.chat_panes.get(&pane_id).cloned() else {
            self.chat_echo.remove(&pane_id);
            return;
        };
        if let Some(echoes) = self.chat_echo.get_mut(&pane_id) {
            echoes.retain(|echo| {
                echo.at.elapsed() < ECHO_MAX_AGE
                    && !state
                        .messages
                        .iter()
                        .any(|m| m.role == ChatRole::User && m.text.trim() == echo.text.trim())
            });
            if echoes.is_empty() {
                self.chat_echo.remove(&pane_id);
            }
        }
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
        // 表示は transcript + 生きている楽観 echo（#716。§3.1）
        let visible = self.chat_visible_messages(pane_id, &state);
        // **内容が変わったフレームだけ**下端へ寄せる。毎フレーム寄せると、
        // 追従を外す前の 1 フレームでユーザーのホイール操作を巻き戻してしまう。
        // 判断材料は**実際に描く列**にする（送信直後の echo でも下端へ付いていく）
        if self.chat_content_changed(pane_id, &visible, state.permission.is_some()) && following {
            scroll_handle.scroll_to_bottom();
        }
        let mut messages: Vec<gpui::AnyElement> = visible
            .iter()
            .map(|m| self.render_chat_message(pane_id, m, compact, cx))
            .collect();
        let empty = messages.is_empty();
        // #716: コマンド提案カード（#666）は会話の流れの中へインラインで置く。
        // ターミナル表示のときは従来どおり専用帯（#703。`pane_shows_terminal` が
        // Chat を除外しているので二重には出ない）
        messages.extend(self.render_chat_inline_cards(pane_id, cx));
        // 承認カードは会話の末尾（いま答えるべきことが一番下にある）
        if let Some(dialog) = state.permission.clone() {
            messages.push(self.render_chat_approval(pane_id, &dialog, cx));
        }

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
            // 入力欄 + スラッシュボタン（#716）。worker は read-only なので出さない
            .when(!state.read_only, |d| {
                d.child(self.render_chat_composer(pane_id, &state, compact, cx))
            })
    }

    /// 入力欄 + スラッシュボタン列（#716 / §2.3）。
    /// 会話の下に固定し、メッセージ一覧だけがスクロールする
    fn render_chat_composer(
        &mut self,
        pane_id: PaneId,
        state: &ChatPaneState,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;
        let focused = self.chat_input_focused == Some(pane_id);
        let text = self.chat_input_text(pane_id).to_string();
        let empty = text.trim().is_empty();
        let confirming = self.chat_clear_confirm == Some(pane_id);
        let error = self
            .chat_action_error
            .as_ref()
            .filter(|(pane, _, at)| *pane == pane_id && at.elapsed() < ACTION_ERROR_DURATION)
            .map(|(_, message, _)| message.clone());
        // キャレットの前後で本文を割って、間にキャレットと未確定文字列を挟む
        let cursor = crate::right_panel::floor_char_boundary(
            &text,
            self.chat_inputs
                .get(&pane_id)
                .map(|i| i.cursor.min(text.len()))
                .unwrap_or(text.len()),
        );
        let (head, tail) = text.split_at(cursor);
        // キャレットのある行より前の行（`head` の最後の行だけを別扱いにする）
        let head_lines: Vec<String> = head.split('\n').map(str::to_string).collect();
        let head_before_caret_line: Vec<String> =
            head_lines[..head_lines.len().saturating_sub(1)].to_vec();

        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(if compact { 8.0 } else { 12.0 }))
            .pt(px(8.0))
            .pb(px(8.0))
            .border_t_1()
            .border_color(hsla(theme.border_subtle))
            .bg(rgba(theme.surface_0))
            // 下のペインへイベントを漏らさない（帯 #703 と同じ）
            .occlude()
            .when_some(error, |d, message: String| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(hsla(theme.red))
                        .child(SharedString::from(message)),
                )
            })
            // 「新しい会話」の確認（破壊的なので必ず 1 段挟む。§受け入れ条件 4）
            .when(confirming, |d| d.child(self.render_chat_clear_confirm(cx)))
            .child(
                div()
                    .id(("chat-input", pane_id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(6.0))
                    .w_full()
                    .px(px(9.0))
                    .py(px(7.0))
                    .rounded(px(9.0))
                    .bg(rgba(theme.background))
                    .border_1()
                    .border_color(hsla(if focused {
                        theme.accent
                    } else {
                        theme.border_default
                    }))
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.focus_chat_input(pane_id, cx);
                        }),
                    )
                    .child(
                        // 本文。**行ごとに要素を分ける**のが要点（#716）: 改行入りの文字列を
                        // 1 個の div に入れるとキャレットはその塊の「隣」に置かれるため、
                        // 複数行では縦位置がずれる（実機スクショで確認して直した）
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .max_h(px(CHAT_INPUT_MAX_HEIGHT))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .text_size(px(12.0))
                            .text_color(hsla(theme.foreground))
                            .when(text.is_empty() && !focused, |d| {
                                d.child(
                                    div().text_color(hsla(theme.text_muted)).child(
                                        SharedString::from(txt::chat_placeholder(state.busy)),
                                    ),
                                )
                            })
                            // キャレットのある行より前
                            .children(head_before_caret_line.iter().map(|line| {
                                div()
                                    .w_full()
                                    .child(SharedString::from(line.clone()))
                                    .into_any_element()
                            }))
                            // キャレットのある行（前半 + 未確定文字列 + キャレット + 後半）
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .when_some(
                                        head.rsplit('\n').next().filter(|s| !s.is_empty()),
                                        |d, line: &str| {
                                            d.child(SharedString::from(line.to_string()))
                                        },
                                    )
                                    .children(
                                        self.text_input_marked(AppTextInput::ChatInput, &theme),
                                    )
                                    .when(focused, |d| {
                                        d.child(
                                            self.text_input_caret(AppTextInput::ChatInput, &theme),
                                        )
                                    })
                                    .when_some(
                                        tail.split('\n').next().filter(|s| !s.is_empty()),
                                        |d, line: &str| {
                                            d.child(SharedString::from(line.to_string()))
                                        },
                                    ),
                            )
                            // キャレットのある行より後
                            .children(tail.split('\n').skip(1).map(|line| {
                                div()
                                    .w_full()
                                    .child(SharedString::from(line.to_string()))
                                    .into_any_element()
                            })),
                    )
                    .child(self.render_chat_send_button(pane_id, empty, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .children(
                        SlashButton::ALL
                            .iter()
                            .map(|b| self.render_chat_slash_button(pane_id, *b, compact, cx)),
                    )
                    .child(div().flex_1())
                    // 送信キーの案内（初心者向けの学習経路。狭いときは落とす）
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(txt::chat_send_hint().to_string())),
                        )
                    }),
            )
            .into_any_element()
    }

    /// 送信ボタン（入力が空のときは淡色 + 押せない）
    fn render_chat_send_button(
        &self,
        pane_id: PaneId,
        empty: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let (fg, bg) = if empty {
            (theme.text_faint, theme.surface_1)
        } else {
            (theme.tab_active_foreground, theme.accent)
        };
        div()
            .id(("chat-send", pane_id.as_u64()))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(7.0))
            .bg(rgba(bg))
            .when(!empty, |d| d.cursor_pointer().hover(|d| d.opacity(0.85)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                this.chat_send_input(pane_id, cx);
            }))
            .child(
                svg()
                    .path(ui_icon::ARROW_UP)
                    .w(px(14.0))
                    .h(px(14.0))
                    .text_color(hsla(fg)),
            )
            .into_any_element()
    }

    /// スラッシュボタン 1 つ（平易なラベル + 実コマンドを小さく併記）
    fn render_chat_slash_button(
        &self,
        pane_id: PaneId,
        button: SlashButton,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let destructive = !button.safe();
        div()
            .id(("chat-slash", pane_id.as_u64() * 8 + button as u64))
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .px(px(7.0))
            .h(px(20.0))
            .rounded(px(10.0))
            .cursor_pointer()
            .bg(rgba(theme.chip_surface))
            .border_1()
            .border_color(hsla(if destructive {
                theme.border_default
            } else {
                theme.border_subtle
            }))
            .text_size(px(10.5))
            .text_color(hsla(theme.text_secondary))
            .hover(|d| {
                d.bg(rgba(theme.surface_hover))
                    .border_color(hsla(if destructive { theme.red } else { theme.accent }))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                this.chat_slash_action(pane_id, button, cx);
            }))
            .child(SharedString::from(button.label().to_string()))
            // 実コマンドの併記（学習経路。狭いときは落とす）
            .when(!compact, |d| {
                d.child(
                    div()
                        .font_family(theme.font_family.clone())
                        .text_size(px(9.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(button.command())),
                )
            })
            .into_any_element()
    }

    /// 「新しい会話」の確認行（#716。会話が消えることを明言してから実行する）
    fn render_chat_clear_confirm(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(9.0))
            .rounded(px(8.0))
            .bg(rgba_alpha(theme.red, 0.12))
            .border_1()
            .border_color(hsla(theme.red))
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hsla(theme.foreground))
                    .child(SharedString::from(
                        txt::chat_clear_confirm_title().to_string(),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_secondary))
                    .child(SharedString::from(
                        txt::chat_clear_confirm_body().to_string(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.0))
                    .justify_end()
                    .child(
                        div()
                            .id("chat-clear-cancel")
                            .px(px(9.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .bg(rgba(theme.surface_highlight))
                            .text_size(px(11.0))
                            .text_color(hsla(theme.foreground))
                            .hover(|d| d.bg(rgba_alpha(theme.surface_highlight, 1.5)))
                            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.chat_clear_confirm = None;
                                cx.notify();
                            }))
                            .child(SharedString::from(txt::chat_clear_cancel().to_string())),
                    )
                    .child(
                        div()
                            .id("chat-clear-ok")
                            .px(px(9.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .bg(rgba_alpha(theme.red, 0.3))
                            .text_size(px(11.0))
                            .text_color(hsla(theme.red))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.5)))
                            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.chat_clear_accept(cx);
                            }))
                            .child(SharedString::from(txt::chat_clear_ok().to_string())),
                    ),
            )
            .into_any_element()
    }

    /// 承認カード（#716 / §2.3）。**画面にダイアログが実在するときだけ**呼ばれる。
    /// 選択肢は実ダイアログのものそのままで、押すと `Respond` が実在を再検証して番号を送る
    fn render_chat_approval(
        &self,
        pane_id: PaneId,
        dialog: &tako_control::claude_tui::PermissionDialog,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;
        let total = dialog.options.len();
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(6.0))
            .p(px(10.0))
            .rounded(px(9.0))
            .bg(rgba(theme.surface_1))
            .border_1()
            .border_color(hsla(theme.yellow))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        svg()
                            .path(ui_icon::WARNING)
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(hsla(theme.yellow)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.foreground))
                            .child(SharedString::from(txt::chat_approval_title().to_string())),
                    ),
            )
            .when(!dialog.command.trim().is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .p(px(7.0))
                        .rounded(px(6.0))
                        .bg(rgba(theme.crust))
                        .font_family(theme.font_family.clone())
                        .text_size(px(11.0))
                        .text_color(hsla(theme.foreground))
                        .child(SharedString::from(dialog.command.clone())),
                )
            })
            .children(dialog.options.iter().enumerate().map(|(i, option)| {
                let choice = i + 1;
                // 実ダイアログの並びで最後の選択肢が「拒否」（PWA #425 と同じ扱い）
                let deny = total > 1 && choice == total;
                let theme = theme.clone();
                div()
                    .id(("chat-approve", pane_id.as_u64() * 16 + choice as u64))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .w_full()
                    .px(px(9.0))
                    .py(px(5.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .bg(rgba_alpha(if deny { theme.red } else { theme.green }, 0.14))
                    .border_1()
                    .border_color(hsla(if deny { theme.red } else { theme.green }))
                    .text_size(px(11.5))
                    .text_color(hsla(theme.foreground))
                    .hover(|d| d.bg(rgba_alpha(if deny { theme.red } else { theme.green }, 0.28)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.chat_respond(pane_id, choice, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(choice.to_string())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(SharedString::from(option.clone())),
                    )
            }))
            .into_any_element()
    }

    /// コマンド提案カード（#666）を会話の中へインラインで置く（#716）。
    ///
    /// カード自体の描画・コピー・実行は `command_card_ui` の**同じ経路**を使う
    /// （見た目の実装を 2 つ持たない = 片方だけ直る事故が起きない）
    fn render_chat_inline_cards(
        &mut self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        self.command_card_elements(pane_id, cx)
            .into_iter()
            .map(|card| {
                div()
                    .flex_shrink_0()
                    .w_full()
                    .child(card)
                    .into_any_element()
            })
            .collect()
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
            // #715: システムが差し込んだ通知は会話ではないので薄い 1 行に留める
            ChatRole::System => div()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(5.0))
                .w_full()
                .py(px(1.0))
                .child(
                    svg()
                        .path(ui_icon::INFO)
                        .w(px(10.0))
                        .h(px(10.0))
                        .flex_none()
                        .text_color(hsla(theme.text_faint)),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(
                            crate::ui_text::ui_mode::chat_system_notice(
                                &message.text,
                                message.notices,
                            ),
                        )),
                )
                .into_any_element(),
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
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_subtle))
                        .text_color(hsla(theme.foreground))
                        // #715: 画像添付は実体が transcript に無いのでプレースホルダを出す
                        // （旧実装は座標変換のメタ文をそのまま発話として並べていた）
                        .when(message.images > 0, |d| {
                            d.child(self.render_chat_attachment(message.images))
                        })
                        .children(self.render_chat_user_text(pane_id, message, cx)),
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

    /// ユーザー発話の本文（#716）。
    ///
    /// 極端に長い発話（スキル本文の注入・巨大な貼り付け）は既定で先頭だけ見せて
    /// 「続きを表示」を付ける。会話全体が 1 個の発話で埋まると、直近のやり取りが
    /// 読めなくなる（実 transcript に 15 万文字の user 行が存在する）
    fn render_chat_user_text(
        &self,
        pane_id: PaneId,
        message: &ChatMessage,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if message.text.is_empty() {
            return Vec::new();
        }
        let theme = self.theme.clone();
        let total = message.text.chars().count();
        if total <= LONG_MESSAGE_CHARS {
            return vec![div()
                .child(SharedString::from(message.text.clone()))
                .into_any_element()];
        }
        let key = message.key;
        let expanded = self.chat_long_expanded.contains(&(pane_id, key));
        let shown = if expanded {
            message.text.clone()
        } else {
            message
                .text
                .chars()
                .take(LONG_MESSAGE_CHARS)
                .collect::<String>()
        };
        vec![
            div().child(SharedString::from(shown)).into_any_element(),
            div()
                .id(("chat-long", key.rotate_left(7) ^ pane_id.as_u64()))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .self_start()
                .mt(px(3.0))
                .cursor_pointer()
                .text_size(px(10.5))
                .text_color(hsla(theme.accent))
                .hover(|d| d.text_color(hsla(theme.foreground)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if !this.chat_long_expanded.remove(&(pane_id, key)) {
                        this.chat_long_expanded.insert((pane_id, key));
                    }
                    cx.notify();
                }))
                .child(
                    svg()
                        .path(if expanded {
                            ui_icon::CHEVRON_DOWN
                        } else {
                            ui_icon::CHEVRON_RIGHT
                        })
                        .w(px(10.0))
                        .h(px(10.0))
                        .flex_none()
                        .text_color(hsla(theme.accent)),
                )
                .child(SharedString::from(if expanded {
                    crate::ui_text::ui_mode::chat_collapse_long().to_string()
                } else {
                    crate::ui_text::ui_mode::chat_expand_long(total)
                }))
                .into_any_element(),
        ]
    }

    /// 画像添付のプレースホルダ（#715）。
    ///
    /// transcript には画像の実体（base64）が入っていることもあるが、
    /// **チャットで復元はしない**: tail=50 件ぶんの巨大な base64 を毎回デコードすると
    /// #258 で潰したメモリ問題を再発させる。ここでは「画像を送った」ことだけ伝える
    fn render_chat_attachment(&self, images: usize) -> gpui::AnyElement {
        let theme = &self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .flex_none()
            .self_start()
            .px(px(7.0))
            .py(px(3.0))
            .rounded(px(6.0))
            .bg(rgba(theme.crust))
            .border_1()
            .border_color(hsla(theme.border_subtle))
            .child(
                svg()
                    .path(ui_icon::IMAGE)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(hsla(theme.text_secondary)),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_secondary))
                    .child(SharedString::from(
                        crate::ui_text::ui_mode::chat_image_attachment(images),
                    )),
            )
            .into_any_element()
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
    /// 末尾の発話キーと件数で見るので、生成中に本文が伸びるたびに真になる。
    /// 承認カードの出現も「新着」として扱う（答えるべきものが下端に出る。#716）
    fn chat_content_changed(
        &mut self,
        pane_id: PaneId,
        messages: &[ChatMessage],
        approval: bool,
    ) -> bool {
        let key = messages.last().map(|m| m.key).unwrap_or(0)
            ^ (messages.len() as u64).rotate_left(32)
            ^ u64::from(approval).rotate_left(16);
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
    /// 画面に実在する permission ダイアログ（#716。採取はプロセス内のスクリーンだけ）
    permission: Option<tako_control::claude_tui::PermissionDialog>,
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
    permission: Option<tako_control::claude_tui::PermissionDialog>,
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
                    // #716: 承認カードの表示条件は「画面にダイアログが実在する」こと
                    // （PWA #425 / #577 と同じ。transcript からの推定は使わない）
                    permission: tako_control::claude_tui::detect_permission_dialog(&lines),
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
                permission: data.permission,
            };
            let same = previous.is_some_and(|p| **p == state);
            if !same {
                self.chat_panes.insert(result.pane, std::rc::Rc::new(state));
                changed = true;
            }
            // #716: transcript に自分の発話が現れた echo を捨てる（二重表示の解消）
            self.prune_chat_echo(result.pane);
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
        self.chat_long_expanded
            .retain(|(_, key)| live.contains(key));
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
        // #720: 過渡期の記録もペインと一緒に落とす（ペイン ID は再利用される。#390）
        self.pane_settle.remove(&pane_id);
        // #716: 入力の下書き・echo・確認待ちもペインと一緒に落とす
        // （ペイン ID は再利用されるので残すと他人の下書きが現れる。#390）
        self.chat_inputs.remove(&pane_id);
        self.chat_echo.remove(&pane_id);
        self.chat_long_expanded.retain(|(pane, _)| *pane != pane_id);
        if self.chat_input_focused == Some(pane_id) {
            self.chat_input_focused = None;
        }
        if self.chat_clear_confirm == Some(pane_id) {
            self.chat_clear_confirm = None;
        }
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
                    permission: target.permission,
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

    /// #715: システム通知は薄い 1 行の材料として拾い、画像は枚数を持つ。
    /// `kind` の無い未知の system エントリは従来どおり落とす
    #[test]
    fn システム通知と画像添付を描画用に拾う() {
        let values = vec![
            json!({ "role": "system", "kind": "notice", "text": "Monitor event", "count": 3 }),
            // 本文が空でも画像があれば表示する（プレースホルダの根拠）
            json!({ "role": "user", "text": "", "attachments": [{ "kind": "image" }] }),
            json!({
                "role": "user",
                "text": "これ見て",
                "attachments": [{ "kind": "image" }, { "kind": "image" }],
            }),
            // kind 無しの system は描き方が決まらないので落とす
            json!({ "role": "system", "text": "未知" }),
        ];
        let messages = messages_from_json(&values);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(messages[0].notices, 3);
        assert_eq!(messages[1].role, ChatRole::User);
        assert!(messages[1].text.is_empty());
        assert_eq!(messages[1].images, 1);
        assert_eq!(messages[2].images, 2);
        // 通知と発話は別物としてキーが分かれる（折りたたみ状態の混線防止）
        assert_ne!(messages[0].key, messages[1].key);
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

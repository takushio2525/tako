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
use std::ops::Range;
use std::time::Duration;

use super::*;
use crate::command_card_ui::FEEDBACK_DURATION;
use crate::file_icons::ui_icon;

/// 会話の読み込み件数（§2.3。古い発話は claude TUI 側 / `tako logs` で辿れる）
pub(crate) const CHAT_TAIL: usize = 50;

/// コンテキスト使用率がこれを超えたら残量バーを警告色にする（§2.3）
const CTX_WARN_PERCENT: f64 = 80.0;

/// 入力欄に見せる最大行数（#718 / #719）。
///
/// 高さは TUI 入力ボックスの行数にそのまま追従する（1 行なら 1 行ぶん）が、
/// 長文を書いたときに会話が全部隠れないようここで止め、以降は入力欄の中で
/// カーソル側へスクロールする（Web 版 Claude の入力欄と同じ感覚）
pub(crate) const CHAT_INPUT_MAX_ROWS: usize = 8;

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
    /// 生成中に打たれてまだ claude へ渡っていない（#737 追加要件 5）。
    /// 表示だけの状態なので**内容キーには混ぜない**（配送された瞬間に鍵が変わると
    /// 折りたたみ状態と md キャッシュが無駄に作り直される）
    pub queued: bool,
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

    /// 残量が少ないときの `/compact` ヒントを出すか（#739 / §2.3）。
    ///
    /// **ヒントは押せるボタン**（押下は G3 のスラッシュボタンと同じ経路）なので、
    /// 「出すかどうか」を警告色と同じ 1 つの根拠から決める = 色だけ赤くて
    /// 逃げ道が出ない / 逆にヒントだけ出る、というズレが構造的に起きない
    pub(crate) fn ctx_hint(&self) -> bool {
        self.ctx_gauge().is_some_and(|(_, warn)| warn)
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
                // #737: 正規化層が「まだ claude へ渡っていない」と印を付けたもの
                queued: v["queued"].as_bool().unwrap_or(false),
                key,
            })
        })
        .collect()
}

/// tako 自前のプレースホルダを出すか（#737。**重なりを決める唯一の判断**）。
///
/// `has_text` = ユーザーが打った文字がある / `tui_shows_text` = TUI が箱の中へ
/// 何か描いている（自前の dim な案内文を含む）。
/// 旧実装は `!has_text` だけで出していたので、claude の案内文の上へ重なった
/// （#737 の実測根因 O1）。純関数にしてセルフテストから同じ判断を検査する
pub(crate) fn chat_placeholder_visible(has_text: bool, tui_shows_text: bool) -> bool {
    !has_text && !tui_shows_text
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

/// 入力欄に映す TUI 入力ボックスの実測（#719）。
///
/// **下書きを別に持たない**のが要点。チャット入力欄は claude TUI の入力行を
/// そのまま映すだけの窓で、打鍵は素通しで PTY へ行く。したがって
///
/// - 入力状態は常に 1 つ（TUI が正）= 表示モードを往復してもズレない
/// - IME・Enter / Shift+Enter・画像ペーストは TUI の挙動そのもの
/// - 箱の高さは TUI の行数に追従する（#718 のオートグロー）
pub(crate) struct ChatInputMirror {
    /// 映す行（`terminal_screen_lines` が作った実描画行の切り出し）
    pub rows: Vec<gpui::Div>,
    /// TUI 入力ボックスの総行数（`rows.len()` は上限で頭打ちになる）
    pub total_rows: usize,
    /// 入力欄に**ユーザーの文字が入っているか**（送信ボタンを押せるかの判断）
    pub has_text: bool,
    /// TUI が入力ボックスに**自前で何か描いているか**（#737）。
    ///
    /// `has_text` とは別物で、claude 自身の dim な案内文
    /// （空欄時の `Try "how does <filepath> work?"` / キュー滞留時の
    /// `Press up to edit queued messages`）も true になる。
    /// これを見ずに `has_text` だけで tako 自前のプレースホルダを重ねていたため、
    /// **claude の案内文と tako の案内文が同じ座標に重なって読めなくなっていた**
    /// （#737 の実測根因 O1。col 2 から始まる dim テキストの上に、
    /// 絶対配置 left(24) のプレースホルダが乗っていた）
    pub tui_shows_text: bool,
    /// カーソルのセル位置（映した行の中での (列, 行)）。IME の位置出しに使う（#737）
    pub caret_cell: Option<(usize, usize)>,
    /// スピナー行（`Manifesting… (5m 16s · ↓ 16.4k tokens)`）。生成中だけ Some（#719）
    pub activity: Option<String>,
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

// --- 選択とコピー（#725） ---

/// メッセージのコピーボタンに割く列の幅（#725）。
/// 本文と重ならないよう**レイアウトで**場所を取る（絶対配置の被りを作らない）
const CHAT_COPY_GUTTER: f32 = 22.0;

/// 折りたたみ（[`ChatSection::slot`]）と混ざらない要素 ID の枠
const SLOT_MESSAGE_COPY: u64 = 1 << 40;
const SLOT_CODE_COPY: u64 = 1 << 41;

/// コピー成功のフィードバックを出しておく時間（#680 のコードブロックと同値）
const CHAT_COPY_FEEDBACK: Duration = crate::preview_render::MD_COPY_FEEDBACK;

/// 何をコピーしたか（フィードバック表示の対象）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatCopyTarget {
    /// メッセージ全文
    Message,
    /// メッセージ内のコードブロック（出現順 0 始まり）
    Code(usize),
}

/// チャット本文の「描いた行」の索引（#725）。
///
/// 選択の座標系はプレビュー（#145 / #656）と同じ **(行番号, UTF-8 byte)** で、
/// 1 ペインぶんの会話を通しで採番する。この 1 点だけで
///
/// - ヒットテストは [`crate::preview_text_layout_hit_test`] をそのまま使える
///   （実 shaping 逆写像 = 日本語・太字・見出しサイズでも位置がずれない）
/// - 行番号が発話をまたいで連続する = **複数メッセージにまたがる選択**が自然に成立する
///
/// 索引は描画のたびに作り直す。`TextLayout` は実描画の結果を指すので、
/// スクロール・折りたたみ・新着で位置が変わっても次のフレームで正しくなる。
#[derive(Default)]
pub(crate) struct ChatTextIndex {
    /// このフレームで塗る選択（描きながら参照するので複製で持つ）
    pub(crate) selection: Option<PreviewSelection>,
    /// 行番号 → プレーンテキスト（**コピーの正**）
    pub(crate) texts: Vec<String>,
    /// 行番号 → 実描画レイアウト（**ヒットテストの正**。文字の無い行は None）
    pub(crate) layouts: Vec<Option<gpui::TextLayout>>,
}

impl ChatTextIndex {
    /// 1 行ぶんを控えて `StyledText` を作る。
    ///
    /// **選択の座標系はここだけで決まる**ので、md も地の文（ユーザー発話・
    /// 折りたたみ本文）も必ずここを通す。素の `SharedString` で描くと
    /// その行だけ選択できない「穴」になる
    fn push(
        &mut self,
        theme: &tako_core::theme::Theme,
        text: String,
        mut highlights: Vec<(Range<usize>, HighlightStyle)>,
        color: tako_core::Rgb,
        weight: Option<FontWeight>,
    ) -> StyledText {
        let line = self.texts.len();
        push_selection_highlight(&mut highlights, &text, line, self.selection.as_ref(), theme);
        let styled = crate::md_view::styled_line(
            text.clone(),
            highlights,
            &crate::md_view::md_text_style(theme, color, weight),
        );
        self.layouts.push(Some(styled.layout().clone()));
        self.texts.push(text);
        styled
    }

    /// 文字を持たない行（md の罫線）。行番号の対応を保つため空で 1 行進める
    fn push_spacer(&mut self) {
        self.texts.push(String::new());
        self.layouts.push(None);
    }
}

/// md ブロック列を「画面と同じプレーンテキスト」へ落とす（#725 のコピー本文）。
///
/// ブロック内の行（表のセル・コードの各行）は改行 1 つ、**ブロックとブロックの間は
/// 空行**でつなぐ。見出しと段落が地続きにならないので、貼り付けてそのまま読める。
/// 罫線は文字を持たない（`md_block_line_texts` が空文字を返す）ので落とす。
///
/// ドラッグ選択が返すのは「掃いた行を改行 1 つでつないだもの」で、ここだけ
/// ブロック間に空行が入る。**掃いた範囲そのまま**と**発話 1 件の再現**という
/// 別の契約なので、意図的に揃えていない。
pub(crate) fn md_plain_text(blocks: &[preview::MdBlock]) -> String {
    blocks
        .iter()
        .map(|block| crate::md_block_line_texts(block).join("\n"))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 発話に紐づく要素 ID（ハッシュの上位ビットを落とさないように混ぜる。
/// 同一フレーム内で衝突しなければよい）
fn chat_element_id(pane_id: PaneId, key: u64, slot: u64) -> u64 {
    key.rotate_left(11).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ pane_id.as_u64() ^ slot
}

/// 選択範囲の背景ハイライトを 1 行ぶん積む（プレビューと同じ色・同じ規則）
fn push_selection_highlight(
    highlights: &mut Vec<(Range<usize>, HighlightStyle)>,
    text: &str,
    line: usize,
    selection: Option<&PreviewSelection>,
    theme: &tako_core::theme::Theme,
) {
    let Some((start, end)) = selection.and_then(|s| s.range_for_line(line, text.len())) else {
        return;
    };
    let start = crate::snap_to_char_boundary(text, start.min(text.len()));
    let end = crate::snap_to_char_boundary(text, end.min(text.len()));
    if start >= end {
        return;
    }
    highlights.push((
        start..end,
        HighlightStyle {
            background_color: Some(hsla_alpha(theme.accent, 0.35)),
            ..HighlightStyle::default()
        },
    ));
}

/// チャット本文の md 受け皿（#725）。プレビューの `MdSelectionSink` と同じ役割で、
/// 幾何は [`crate::md_view::render_block`] に任せ、ここは選択ハイライトの重ねと
/// `TextLayout` の控え、コードブロックのコピーボタンだけを担う。
struct ChatMdSink<'a, 'w> {
    app: &'a TakoApp,
    cx: &'a mut Context<'w, TakoApp>,
    pane_id: PaneId,
    /// この発話の内容キー（コピー対象の識別に使う）
    message_key: u64,
    index: &'a mut ChatTextIndex,
}

impl crate::md_view::MdTextSink for ChatMdSink<'_, '_> {
    fn text(
        &mut self,
        text: String,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        color: tako_core::Rgb,
        weight: Option<FontWeight>,
    ) -> gpui::AnyElement {
        self.index
            .push(&self.app.theme, text, highlights, color, weight)
            .into_any_element()
    }

    fn spacer(&mut self) {
        self.index.push_spacer();
    }

    fn code_overlay(&mut self, index: usize) -> Option<crate::md_view::MdCodeOverlay> {
        let group = SharedString::from(format!(
            "chat-code-{}-{}-{index}",
            self.pane_id.as_u64(),
            self.message_key
        ));
        let element = self.app.render_chat_copy_button(
            self.pane_id,
            self.message_key,
            ChatCopyTarget::Code(index),
            group.clone(),
            self.cx,
        );
        Some(crate::md_view::MdCodeOverlay { group, element })
    }
}

impl TakoApp {
    /// このペインのチャット状態（判定表が Chat を返すのと同じ条件で存在する）
    pub(crate) fn chat_state(&self, pane_id: PaneId) -> Option<&ChatPaneState> {
        self.chat_panes.get(&pane_id).map(|s| s.as_ref())
    }

    // --- 入力（#719。TUI 入力行のミラー + 打鍵パススルー。§3.2） ---

    /// チャット入力欄に映す TUI 入力ボックスを実画面から採る（#719）。
    ///
    /// 描画行（`terminal_screen_lines`）とスクリーン行は**下端からの距離**で
    /// 対応付ける。ミラースクロール中は先頭に履歴行が積まれることがあるので、
    /// 先頭からの添字で切ると 1 行ずれる（#159 の合成経路）
    pub(crate) fn chat_input_mirror(
        &self,
        pane_id: PaneId,
        show_cursor: bool,
    ) -> Option<ChatInputMirror> {
        let screen = self
            .terminals
            .get(&pane_id)
            .map(|s| s.screen_opts(&self.theme, show_cursor))?;
        let region = tako_core::screen::input_region(&screen)?;
        let total = screen.lines.len();
        // 上限を超えたぶんは頭を落とす（打鍵は末尾で進むので、カーソル側が残る）
        let shown = region.rows().min(CHAT_INPUT_MAX_ROWS);
        let first = region.end.saturating_sub(shown);
        // 下端からの距離へ変換してから描画行を切り出す
        let (from_bottom_first, from_bottom_end) = (
            total.saturating_sub(first),
            total.saturating_sub(region.end),
        );
        let mut rendered = self.terminal_screen_lines(pane_id, show_cursor);
        let end = rendered.len().saturating_sub(from_bottom_end);
        let start = rendered.len().saturating_sub(from_bottom_first).min(end);
        let rows: Vec<gpui::Div> = rendered.drain(start..end).collect();
        // 入力欄に「ユーザーが打った文字」があるか（dim のゴースト提案は無しと扱う。#572）
        // **箱と同じ行**を見る（探し直すと走査範囲の違いで食い違う。#719 実スクショ）
        let analyzed = tako_core::screen::analyze_input_line_at(&screen, region.prompt_row);
        let has_text = analyzed
            .as_ref()
            .map(|s| {
                !s.text.is_empty()
                    && !tako_control::claude_tui::input_content_is_empty(&s.text)
                    && s.style != tako_core::screen::InputStyle::Ghost
            })
            .unwrap_or(false);
        // #737: 判定はどちらも tako-core の純関数が正（合成画面で単体テストできる）
        let tui_shows_text = tako_core::screen::input_box_has_content(&screen, &region);
        let caret_cell = tako_core::screen::input_caret_cell(&screen, &region, shown);
        // スピナー行は入力ボックスより上だけを見る（フッターの `(→4h44m)` を拾わない）
        let above: Vec<String> = screen.lines[..region.start.min(screen.lines.len())]
            .iter()
            .map(|l| l.text.clone())
            .collect();
        let activity = tako_control::claude_tui::activity_line(&above);
        Some(ChatInputMirror {
            rows,
            total_rows: region.rows(),
            has_text,
            tui_shows_text,
            caret_cell,
            activity,
        })
    }

    /// IME のキャレットが**入力欄の内側**にあるか（#737 の検査点）。
    ///
    /// 位置ズレの正体は「キャレットが箱の外（ターミナルグリッド上の座標）を
    /// 指していた」ことなので、内側であることを機械検証できる形で持つ。
    /// 返り値は (キャレット矩形が採れたか, 箱の内側か)
    pub(crate) fn chat_caret_inside_input(&self, pane_id: PaneId) -> (bool, bool) {
        let caret = self
            .chat_caret_bounds
            .get()
            .filter(|(p, _)| *p == pane_id)
            .map(|(_, b)| b);
        let Some(caret) = caret else {
            return (false, false);
        };
        let Some(box_bounds) = self.chat_input_bounds.get() else {
            return (true, false);
        };
        // 1px の丸め誤差は許す（枠線・サブピクセル配置のぶん）
        let slack = px(1.0);
        let inside = caret.origin.x >= box_bounds.origin.x - slack
            && caret.origin.y >= box_bounds.origin.y - slack
            && caret.origin.x <= box_bounds.origin.x + box_bounds.size.width + slack
            && caret.origin.y + caret.size.height
                <= box_bounds.origin.y + box_bounds.size.height + slack;
        (true, inside)
    }

    /// 入力欄クリック = そのペインへフォーカスするだけ（#719）。
    ///
    /// 打鍵は素通しで PTY へ行くので、専用のフォーカス状態は持たない
    /// （アプリ内テキスト入力のフラグが残らない = #503 の再発経路が構造的に無い）
    pub(crate) fn focus_chat_input(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.focused_pane() != pane_id {
            self.jump_to_pane(pane_id, cx);
        }
        cx.notify();
    }

    /// 送信ボタン（#719）。TUI の入力行をそのまま確定させる = **Enter だけ**送る。
    ///
    /// 本文を組み立て直さないので、`[Image #1]` やペースト畳み込みが入っていても
    /// TUI が持っているものがそのまま送られる（キーボードの Enter と完全に同じ結果）
    pub(crate) fn chat_submit_input(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let text = self
            .chat_input_mirror(pane_id, false)
            .filter(|m| m.has_text)
            .and_then(|_| {
                self.terminals
                    .get(&pane_id)
                    .map(|s| s.screen_opts(&self.theme, false))
            })
            .and_then(|screen| tako_core::screen::analyze_input_line(&screen))
            .map(|s| s.text)
            .unwrap_or_default();
        if text.trim().is_empty() {
            return; // 空送信は無視（claude に空行を送らない）
        }
        // 本文は送らず Enter だけ（#95 の Enter 単独送達フロー）。楽観 echo には
        // 画面から読んだ本文を使うので、transcript が追いつくまでの見た目は変わらない
        self.chat_send_newline(pane_id, &text, cx);
    }

    /// 楽観 echo を積む（#716 / #737）。
    ///
    /// **送信経路が複数あっても見た目は 1 通り**にするための唯一の入口:
    /// 送信ボタン（`chat_send_inner`）と、素通しで打たれた Enter
    /// （`handle_key`。#735 でこちらが既定の送信経路になった）が同じここを通る。
    /// 空・空白だけなら何もしない（claude に送っていないものを吹き出しにしない）
    pub(crate) fn push_chat_echo(&mut self, pane_id: PaneId, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        // 同じ本文の echo を二重に積まない（Enter の再送検証 #95 で 2 回来ても 1 個）
        let echoes = self.chat_echo.entry(pane_id).or_default();
        if echoes
            .iter()
            .any(|e| e.text.trim() == text.trim() && e.at.elapsed() < ECHO_MAX_AGE)
        {
            return;
        }
        echoes.push(ChatEcho {
            text: text.to_string(),
            at: std::time::Instant::now(),
        });
        // 送った直後に「考え中」を出す（transcript / 画面採取の 2 秒を待たない）
        if let Some(state) = self.chat_panes.get(&pane_id) {
            let mut next = (**state).clone();
            next.busy = true;
            self.chat_panes.insert(pane_id, std::rc::Rc::new(next));
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
        self.chat_send_inner(pane_id, text, text, cx)
    }

    /// 入力行の確定（#719）。本文は組み立て直さず **Enter だけ**送る。
    /// `echo` は画面から読んだ入力内容で、楽観 echo の見た目にだけ使う
    fn chat_send_newline(&mut self, pane_id: PaneId, echo: &str, cx: &mut Context<Self>) -> bool {
        self.chat_send_inner(pane_id, "", echo, cx)
    }

    /// 送信の実体（`chat_send_text` / `chat_send_newline` 共通）
    fn chat_send_inner(
        &mut self,
        pane_id: PaneId,
        text: &str,
        echo: &str,
        cx: &mut Context<Self>,
    ) -> bool {
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
                self.push_chat_echo(pane_id, echo);
                self.chat_action_error = None;
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
                // 楽観 echo は「まだ transcript に無い自分の発話」なので送信待ち扱い。
                // transcript（キュー行 or 本物の user 行）が追いつけばそちらへ入れ替わる
                queued: true,
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

    // --- 選択とコピー（#725） ---

    /// チャット本文のヒットテスト（マウス位置 → (行番号, UTF-8 byte)）。
    ///
    /// 実体はプレビューと同じ実 shaping 逆写像なので、日本語・太字・見出しサイズ・
    /// 表のセルでも位置がずれない（#145 / #656 で実測済みの資産をそのまま使う）
    pub(crate) fn chat_hit_test(
        &self,
        pane_id: PaneId,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<(usize, usize)> {
        let index = self.chat_text_index.get(&pane_id)?;
        if index.layouts.iter().all(Option::is_none) {
            return None;
        }
        crate::preview_text_layout_hit_test(&index.layouts, &index.texts, position)
    }

    /// **いまチャットを描いているか**（選択・コピーの前提）。
    ///
    /// 索引はペインが terminal 表示へ戻っても残る（次に描くまで消えない）ので、
    /// ここを通さないと「ターミナル表示なのに ⌘C が古い会話を返す」事故になる
    fn chat_view_active(&self, pane_id: PaneId) -> bool {
        self.pane_display_for(pane_id) == tako_core::ui_mode::PaneDisplay::Chat
    }

    /// フォーカスペインのチャット選択テキスト（⌘C の材料）。
    /// 選択が空（クリックしただけ）なら None を返すので、⌘C は次の候補へ落ちる
    pub(crate) fn chat_selected_text(&self) -> Option<String> {
        let pane_id = self.focused_pane();
        if !self.chat_view_active(pane_id) {
            return None;
        }
        let index = self.chat_text_index.get(&pane_id)?;
        let selection = self.chat_selections.get(&pane_id)?;
        crate::selection_text(&index.texts, selection)
    }

    /// チャット本文の全選択（⌘A）。チャット表示でなければ false（他の経路へ譲る）
    pub(crate) fn select_all_chat(&mut self, pane_id: PaneId) -> bool {
        if !self.chat_view_active(pane_id) {
            return false;
        }
        let Some(index) = self.chat_text_index.get(&pane_id) else {
            return false;
        };
        let Some(last) = index.texts.len().checked_sub(1) else {
            return false;
        };
        let head = (last, index.texts[last].len());
        self.chat_selections.insert(
            pane_id,
            PreviewSelection {
                anchor: (0, 0),
                head,
            },
        );
        true
    }

    /// 選択を捨てる（表示が組み替わって行番号の意味が変わるとき）
    fn clear_chat_selection(&mut self, pane_id: PaneId) {
        self.chat_selections.remove(&pane_id);
    }

    /// マウス押下 = 選択の開始（#725）。
    /// **伝播は止めない**ので、同じ押下でペインのフォーカスも従来どおり移る
    fn begin_chat_selection(&mut self, pane_id: PaneId, position: gpui::Point<gpui::Pixels>) {
        match self.chat_hit_test(pane_id, position) {
            Some(pos) => {
                self.chat_selections.insert(
                    pane_id,
                    PreviewSelection {
                        anchor: pos,
                        head: pos,
                    },
                );
                self.chat_selecting = Some(pane_id);
            }
            None => {
                self.chat_selections.remove(&pane_id);
            }
        }
    }

    /// ドラッグ中の選択伸長。動いたら true（描き直しが要る）
    fn extend_chat_selection(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<gpui::Pixels>,
    ) -> bool {
        let Some(pos) = self.chat_hit_test(pane_id, position) else {
            return false;
        };
        let Some(selection) = self.chat_selections.get_mut(&pane_id) else {
            return false;
        };
        if selection.head == pos {
            return false;
        }
        selection.head = pos;
        true
    }

    /// 発話のコピー本文（#725。**UI ボタンと CLI / MCP が共有する唯一の定義**）。
    ///
    /// 既定は **画面と同じプレーンテキスト**（md ソースではない）。assistant の md は
    /// ブロック単位で空行を挟んで連結するので、見出しと段落が地続きにならず
    /// そのまま貼って読める。`markdown = true` のときだけ transcript の md ソースを渡す
    /// （表・リストを別のところで再描画したい AI 向けの逃げ道）。
    ///
    /// 折りたたみ（#716 の 1200 字制限）は**無視して全文**を返す。畳んでいるのは
    /// 表示の都合で、コピーの対象は発話そのものだから
    fn chat_message_text(&mut self, message: &ChatMessage, markdown: bool) -> String {
        if markdown || message.role != ChatRole::Assistant {
            return message.text.clone();
        }
        let blocks = self.chat_md_blocks(message.key, &message.text);
        md_plain_text(&blocks)
    }

    /// 発話に含まれるコードブロックの全文（出現順）。装飾は付けない（#680 と同じ規則）
    fn chat_message_codes(&mut self, message: &ChatMessage) -> Vec<String> {
        if message.role != ChatRole::Assistant {
            return Vec::new();
        }
        let blocks = self.chat_md_blocks(message.key, &message.text);
        blocks
            .iter()
            .filter_map(|block| match &block.kind {
                preview::MdBlockKind::CodeBlock { lines, .. } => {
                    Some(preview::md_code_block_text(lines))
                }
                _ => None,
            })
            .collect()
    }

    /// 表示中の発話を内容キーで引く（UI ボタンは添字ではなくキーで対象を指す。
    /// 描画と押下の間に新着が入っても別の発話をコピーしない）
    fn chat_message_by_key(&self, pane_id: PaneId, key: u64) -> Option<ChatMessage> {
        let state = self.chat_panes.get(&pane_id)?.clone();
        self.chat_visible_messages(pane_id, &state)
            .into_iter()
            .find(|m| m.key == key)
    }

    /// 発話（またはその中のコードブロック）をクリップボードへ入れる（#725）。
    ///
    /// UI のコピーボタン・CLI `tako chat copy`・MCP `tako_chat_copy` が
    /// **すべてここを通る**（開発不変条件）。実際の書き込みは
    /// `flush_pending_clipboard` に任せる（カード #666 / コードブロック #680 と同方式）
    pub(crate) fn copy_chat_message(
        &mut self,
        pane_id: PaneId,
        key: u64,
        target: ChatCopyTarget,
        markdown: bool,
    ) -> Result<serde_json::Value, String> {
        let message = self
            .chat_message_by_key(pane_id, key)
            .ok_or_else(|| "その発話はもう表示されていない".to_string())?;
        let text = match target {
            ChatCopyTarget::Message => self.chat_message_text(&message, markdown),
            ChatCopyTarget::Code(index) => {
                let codes = self.chat_message_codes(&message);
                if codes.is_empty() {
                    return Err("この発話にコードブロックが無い".into());
                }
                codes.get(index).cloned().ok_or_else(|| {
                    format!(
                        "コードブロック範囲外: {index}（全 {} 個。0 始まり）",
                        codes.len()
                    )
                })?
            }
        };
        if text.is_empty() {
            return Err("コピーする本文が無い".into());
        }
        let lines = text.split('\n').count();
        let bytes = text.len();
        self.pending_clipboard.push(text);
        self.chat_copied = Some((pane_id, key, target, std::time::Instant::now()));
        Ok(serde_json::json!({
            "pane": pane_id.as_u64(),
            "role": match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::System => "system",
            },
            "code": match target {
                ChatCopyTarget::Code(index) => serde_json::json!(index),
                ChatCopyTarget::Message => serde_json::Value::Null,
            },
            "markdown": markdown && matches!(target, ChatCopyTarget::Message),
            "lines": lines,
            "bytes": bytes,
        }))
    }

    /// CLI / MCP からのコピー（#725。`Request::ChatCopy` の実体）。
    /// `message` は表示順 0 始まりで、省略時は**最後の assistant 発話**
    /// （「いまの答えをコピーしたい」がほぼ唯一の用途なので既定を賢くする。#322）
    pub(crate) fn chat_copy_dispatch(
        &mut self,
        pane_id: PaneId,
        list: bool,
        message: Option<usize>,
        code: Option<usize>,
        markdown: bool,
    ) -> Result<serde_json::Value, String> {
        let state = self
            .chat_panes
            .get(&pane_id)
            .cloned()
            .ok_or_else(|| "チャット表示のペインではない".to_string())?;
        let visible = self.chat_visible_messages(pane_id, &state);
        if visible.is_empty() {
            return Err("会話がまだ読めていない".into());
        }
        if list {
            let messages: Vec<serde_json::Value> = visible
                .iter()
                .enumerate()
                .map(|(index, m)| {
                    let codes = self.chat_message_codes(m).len();
                    serde_json::json!({
                        "index": index,
                        "role": match m.role {
                            ChatRole::User => "user",
                            ChatRole::Assistant => "assistant",
                            ChatRole::System => "system",
                        },
                        "chars": m.text.chars().count(),
                        "code_blocks": codes,
                        // 一覧が会話全文になると読めないので冒頭だけ
                        "preview": m.text.chars().take(60).collect::<String>(),
                    })
                })
                .collect();
            return Ok(serde_json::json!({
                "pane": pane_id.as_u64(),
                "total": visible.len(),
                "messages": messages,
            }));
        }
        let index = match message {
            Some(index) => index,
            None => visible
                .iter()
                .rposition(|m| m.role == ChatRole::Assistant)
                .ok_or("assistant の発話がまだ無い（--message で指定できる）")?,
        };
        let key = visible
            .get(index)
            .ok_or_else(|| {
                format!(
                    "メッセージ範囲外: {index}（全 {} 件。0 始まり）",
                    visible.len()
                )
            })?
            .key;
        let target = code.map_or(ChatCopyTarget::Message, ChatCopyTarget::Code);
        let mut result = self.copy_chat_message(pane_id, key, target, markdown)?;
        result["index"] = serde_json::json!(index);
        result["total"] = serde_json::json!(visible.len());
        Ok(result)
    }

    /// コピーボタン 1 つ（メッセージ全文 / コードブロック共通）。
    ///
    /// 見た目は #680 のコードブロックコピーボタンに合わせる: **常時表示だが淡色**で、
    /// ホバーとコピー直後だけ濃くする。「ホバーで初めて現れる」（`opacity(0)` +
    /// `group_hover`）は GPUI で実機のホバー復帰が発火しないことを #680 で実測済み
    fn render_chat_copy_button(
        &self,
        pane_id: PaneId,
        key: u64,
        target: ChatCopyTarget,
        group: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let copied = self.chat_copied.is_some_and(|(pane, k, t, at)| {
            pane == pane_id && k == key && t == target && at.elapsed() < CHAT_COPY_FEEDBACK
        });
        let (icon, color) = if copied {
            (ui_icon::CHECK, theme.green)
        } else {
            (ui_icon::COPY, theme.text_secondary)
        };
        let slot = match target {
            ChatCopyTarget::Message => SLOT_MESSAGE_COPY,
            ChatCopyTarget::Code(index) => SLOT_CODE_COPY ^ (index as u64).rotate_left(3),
        };
        div()
            .id(("chat-copy", chat_element_id(pane_id, key, slot)))
            .when(matches!(target, ChatCopyTarget::Code(_)), |d| {
                d.absolute().top(px(4.0)).right(px(4.0))
            })
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(3.0))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(hsla_alpha(
                if copied {
                    theme.green
                } else {
                    theme.border_default
                },
                if copied { 1.0 } else { 0.7 },
            ))
            .bg(hsla_alpha(
                theme.surface_highlight,
                if copied { 1.0 } else { 0.8 },
            ))
            .text_size(px(9.5))
            .line_height(px(12.0))
            .text_color(hsla(color))
            .opacity(if copied { 1.0 } else { 0.55 })
            .group_hover(group, |d| d.opacity(1.0))
            .hover(|d| d.opacity(1.0).bg(hsla(theme.surface_hover)))
            .cursor(gpui::CursorStyle::PointingHand)
            .child(
                svg()
                    .path(icon)
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(hsla(color)),
            )
            // 待機中はアイコンだけ（本文の隣で場所を食わない）。コピー直後だけ文字を出す
            .children(
                copied.then(|| {
                    SharedString::from(crate::ui_text::ui_mode::chat_copied().to_string())
                }),
            )
            // 下の本文で選択が始まらないようにする（他のボタンと同じ作法）
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                match this.copy_chat_message(pane_id, key, target, false) {
                    Ok(_) => this.flush_pending_clipboard(cx),
                    Err(e) => eprintln!("warning: チャットのコピーに失敗: {e}"),
                }
                // フィードバックの終わりで元の見た目へ戻す（2 秒ポーリング待ちにしない）
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(CHAT_COPY_FEEDBACK).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
                cx.notify();
            }))
            .into_any_element()
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
        // 実画面の採取は**このフレームで 1 回だけ**（ヘッダのライブ表示と入力欄が共有する）。
        // 描画のたびに何度も採ると #168 で潰したのと同じ形の無駄になる
        let mirror = self.chat_input_mirror(pane_id, focused);
        let activity = mirror.as_ref().and_then(|m| m.activity.clone());

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
        // 判断材料は**実際に描く列**にする（送信直後の echo でも下端へ付いていく）。
        // **ドラッグ選択中は寄せない**（#725。選択の最中に新着で飛ぶと選択が壊れる）
        let content_changed =
            self.chat_content_changed(pane_id, &visible, state.permission.is_some(), state.busy);
        if content_changed && following && self.chat_selecting != Some(pane_id) {
            scroll_handle.scroll_to_bottom();
        }
        // #725: 選択の座標系（行番号）はこのフレームで描く順に決まる。
        // 索引は毎フレーム作り直し、描き終わってから丸ごと差し替える
        let mut index = ChatTextIndex {
            selection: self.chat_selections.get(&pane_id).cloned(),
            ..Default::default()
        };
        let mut messages: Vec<gpui::AnyElement> = visible
            .iter()
            .map(|m| self.render_chat_message(pane_id, m, compact, &mut index, cx))
            .collect();
        self.chat_text_index.insert(pane_id, index);
        let empty = messages.is_empty();
        // #716: コマンド提案カード（#666）は会話の流れの中へインラインで置く。
        // ターミナル表示のときは従来どおり専用帯（#703。`pane_shows_terminal` が
        // Chat を除外しているので二重には出ない）
        messages.extend(self.render_chat_inline_cards(pane_id, cx));
        // #737 追加要件 3: 作業中インジケータは会話末尾の AI 側。
        // ヘッダではなくここに出すので、下端追従スクロールにそのまま乗る
        if state.busy {
            messages.push(self.render_chat_activity(pane_id, activity.clone(), state.queued));
        }
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
            // worker も入力できる（#719 追加要件 5）。説明は残すが入力は妨げない
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
                    // 本文はテキストなのでカーソルもテキスト（選択できることが分かる）
                    .cursor(gpui::CursorStyle::IBeam)
                    // 上へスクロールしたら追従を外す（新着で勝手に飛ばない）
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &gpui::ScrollWheelEvent, _, _| {
                            this.on_chat_scroll(pane_id, event);
                        },
                    ))
                    // #725: ドラッグ選択。**伝播は止めない**ので、同じ押下で
                    // ペインのフォーカス移動（親の on_mouse_down）も従来どおり起きる。
                    // ボタン類は各々が mouse_down で伝播を止めるので選択は始まらない
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.begin_chat_selection(pane_id, event.position);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _: &gpui::MouseUpEvent, _, _| {
                            if this.chat_selecting == Some(pane_id) {
                                this.chat_selecting = None;
                            }
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            if this.chat_selecting == Some(pane_id)
                                && event.pressed_button == Some(MouseButton::Left)
                                && this.extend_chat_selection(pane_id, event.position)
                            {
                                cx.notify();
                            }
                        }),
                    )
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
            // 入力欄 + スラッシュボタン。**worker も含めて全チャットペインに出す**
            // （#719 追加要件 5。実運用では worker への直接指示が日常的にある）
            .child(self.render_chat_composer(pane_id, &state, mirror, compact, cx))
    }

    /// 入力欄 + スラッシュボタン列（#716 / §2.3。中身は #719 でミラー方式）。
    /// 会話の下に固定し、メッセージ一覧だけがスクロールする
    fn render_chat_composer(
        &mut self,
        pane_id: PaneId,
        state: &ChatPaneState,
        mirror: Option<ChatInputMirror>,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;
        let focused = self.focused_pane() == pane_id;
        let confirming = self.chat_clear_confirm == Some(pane_id);
        let error = self
            .chat_action_error
            .as_ref()
            .filter(|(pane, _, at)| *pane == pane_id && at.elapsed() < ACTION_ERROR_DURATION)
            .map(|(_, message, _)| message.clone());
        // 入力欄の中身は **TUI の入力行そのもの**（#719。採取は呼び出し元で 1 回だけ）
        let has_text = mirror.as_ref().is_some_and(|m| m.has_text);
        // #737: TUI が箱の中に自前で何か描いているなら、tako のプレースホルダは出さない。
        // claude は空欄でも dim の案内文（`Try "…"`）を、キュー滞留時は
        // `Press up to edit queued messages` を箱の中へ描くので、`has_text` だけで
        // 判断すると**その上に自前の案内文を重ねて読めなくしていた**（実測根因 O1）
        let tui_shows_text = mirror.as_ref().is_some_and(|m| m.tui_shows_text);
        let line_h = self.pane_line_height(pane_id);
        let cell_w = self
            .pane_cell_sizes
            .get(&pane_id)
            .map(|c| c.width)
            .or_else(|| self.cell_size.map(|c| c.width))
            .unwrap_or(px(8.0));
        // 上限に達したら「あと N 行ある」ことが分かるようにしておく（無音の切り捨てにしない）
        let hidden_rows = mirror
            .as_ref()
            .map(|m| m.total_rows.saturating_sub(m.rows.len()))
            .unwrap_or(0);
        // #737: キャレット矩形の置き場は 1 つしかないので、**誰が書くかを決めておく**。
        // 変換中ならそのペイン、そうでなければフォーカスペインだけが書く。
        // 「最後に描いたペインが勝つ」ままだと、master + worker のように
        // チャットペインが複数あるとき IME の宛先と食い違うことがある
        let ime_pane = self.ime.as_ref().map(|ime| ime.pane);
        let owns_caret = match ime_pane {
            Some(pane) => pane == pane_id,
            None => focused,
        };
        let caret_cell = mirror
            .as_ref()
            .and_then(|m| m.caret_cell)
            .filter(|_| owns_caret);
        // #737: 未確定文字列は**この入力欄の中**へ描く（ウィンドウ側のオーバーレイは
        // `ime_overlay_anchor` が抑止する）。箱の中なので overflow_hidden で clip され、
        // 長い未確定文字列がスラッシュボタンや隣のペインへはみ出すことがない
        let preedit = self
            .ime
            .as_ref()
            .filter(|ime| ime.pane == pane_id && !ime.text.is_empty())
            .map(|ime| ime.text.clone());

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
            // 上限行数に達して隠れているぶんの案内（#718 / #719）。
            // #737: 旧実装は箱の中へ absolute で重ねていたので **1 行目の文字の上に
            // 乗っていた**。行として箱の上へ出す = 何にも重ならない
            .when(hidden_rows > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(9.0))
                        .text_size(px(9.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(txt::chat_input_more_rows(hidden_rows))),
                )
            })
            .child(
                div()
                    .id(("chat-input", pane_id.as_u64()))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_start()
                    .w_full()
                    .px(px(9.0))
                    .py(px(6.0))
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
                    // #718 / #737: 実ピクセルで高さを見るための記録
                    // （absolute なのでレイアウト不変）。#737 からは
                    // 「IME キャレットが箱の内側か」の検査にも使う
                    .child({
                        let slot = self.chat_input_bounds.clone();
                        gpui::canvas(|_, _, _| (), move |bounds, _, _, _| slot.set(Some(bounds)))
                            .absolute()
                            .size_full()
                            .into_any_element()
                    })
                    .child(
                        // 映した行をそのまま縦に積む。**行数 = 箱の高さ**なので、
                        // 1 行なら 1 行ぶんの高さに落ち着く（#718）。送信ボタンは
                        // absolute なので箱の高さを一切押し上げない
                        div()
                            .relative()
                            .flex_1()
                            .min_w(px(0.0))
                            // 上限までは伸び、超えたら `chat_input_mirror` が頭を落とす
                            .max_h(px(line_h * CHAT_INPUT_MAX_ROWS as f32))
                            // 送信ボタンのぶんだけ右を空ける（重なり防止）
                            .pr(px(30.0))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .when_some(mirror, |d, m: ChatInputMirror| d.children(m.rows))
                            // TUI が入力行を出していない状態（選択ダイアログ等）でも
                            // 箱が潰れないように 1 行ぶんは確保する
                            .min_h(px(line_h))
                            // #737: キャレット矩形の採取。ミラー行と**同じ箱**の中で
                            // 採るので、パディング・枠・スクロール位置の分を数え直す
                            // 必要がない（座標系の取り違えが構造的に起きない）
                            .child({
                                let slot = self.chat_caret_bounds.clone();
                                gpui::canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, _| {
                                        // 書く権利が無いペインは触らない（他ペインの
                                        // キャレットを上書きしない）
                                        if !owns_caret {
                                            return;
                                        }
                                        let Some((col, row)) = caret_cell else {
                                            // 権利があるのに箱の外（スクロールバック中
                                            // 等）なら、古い矩形を残さず消す
                                            slot.set(None);
                                            return;
                                        };
                                        let origin = point(
                                            bounds.origin.x + cell_w * col as f32,
                                            bounds.origin.y + px(line_h * row as f32),
                                        );
                                        slot.set(Some((
                                            pane_id,
                                            gpui::Bounds::new(
                                                origin,
                                                gpui::size(cell_w, px(line_h)),
                                            ),
                                        )));
                                    },
                                )
                                .absolute()
                                .size_full()
                                .into_any_element()
                            })
                            // #737: 未確定文字列はキャレット位置へインラインで置く。
                            // カーソルより右のセルは TUI 側が空なので、ここに重なる
                            // 文字は無い（重ならないことが幾何で保証される）
                            .when_some(preedit, |d, text: String| {
                                let (col, row) = caret_cell.unwrap_or((0, 0));
                                d.child(
                                    div()
                                        .absolute()
                                        .left(cell_w * col as f32)
                                        .top(px(line_h * row as f32))
                                        .h(px(line_h))
                                        .whitespace_nowrap()
                                        .bg(rgba(theme.background))
                                        .child(self.ime_preedit_text(text)),
                                )
                            }),
                    )
                    // 空のときの案内。実画面には何も無いので**重ねて**出す
                    // （行を足すと高さが変わってしまう）。
                    // #737: TUI が自前の案内文を出しているときは重ねない
                    .when(chat_placeholder_visible(has_text, tui_shows_text), |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(24.0))
                                .top(px(6.0))
                                .h(px(line_h))
                                .flex()
                                .items_center()
                                .text_size(px(11.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(txt::chat_placeholder(state.busy))),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .right(px(6.0))
                            .bottom(px(5.0))
                            .child(self.render_chat_send_button(pane_id, !has_text, cx)),
                    ),
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
                this.chat_submit_input(pane_id, cx);
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

    /// コマンド提案カード（#666）を会話の中へインラインで置く（#716 / #719 追加要件 6）。
    ///
    /// 見た目は **md のコードブロック**（`md_view` の `CodeBlock`）に揃える:
    /// `mantle` の背景パネル + `border_subtle` + 等幅 + 右上のコピーボタン。
    /// Web 版 Claude の会話に出るコードブロックと同じ読み方・押し方になる。
    /// 押したあとの処理（コピー / 新規ペイン実行 / 破棄）は `command_card_ui` の
    /// **同じ経路**を通す = CLI `tako show-command --copy/--run` と 1:1 のまま。
    /// ターミナル表示側の専用帯（#703）はこの変更の影響を受けない
    fn render_chat_inline_cards(
        &mut self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let rows = self.command_card_rows(pane_id);
        rows.into_iter()
            .rev()
            .map(|(card_id, label, commands)| {
                self.render_chat_command_block(card_id, label, commands, cx)
            })
            .collect()
    }

    /// コマンド提案 1 枚を「コードブロック風」に描く（#719 追加要件 6）
    fn render_chat_command_block(
        &self,
        card_id: u64,
        label: Option<String>,
        commands: Vec<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use crate::ui_text::command_card as ctxt;
        let theme = self.theme.clone();
        let total = commands.len();
        let errored = self
            .command_card_error
            .is_some_and(|(id, at)| id == card_id && at.elapsed() < FEEDBACK_DURATION);
        div()
            .flex_shrink_0()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(3.0))
            // ラベル（説明）はブロックの上に控えめに置く
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(
                                label.unwrap_or_else(|| ctxt::heading().to_string()),
                            )),
                    )
                    .when(errored, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.red))
                                .child(SharedString::from(ctxt::run_failed().to_string())),
                        )
                    })
                    .child(
                        div()
                            .id(("chat-cmd-close", card_id))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(15.0))
                            .h(px(15.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            // 下の本文で選択が始まらないようにする（#725）
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation()
                                }),
                            )
                            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.dismiss_command_card(card_id, cx);
                            }))
                            .child(
                                svg()
                                    .path(ui_icon::CLOSE)
                                    .w(px(9.0))
                                    .h(px(9.0))
                                    .text_color(hsla(theme.text_faint)),
                            ),
                    ),
            )
            .children(commands.into_iter().enumerate().map(|(index, command)| {
                self.render_chat_command_line(card_id, index, total, command, cx)
            }))
            .into_any_element()
    }

    /// コマンド 1 行ぶんのコードパネル（背景 + 等幅 + 右上のコピー / 実行）
    fn render_chat_command_line(
        &self,
        card_id: u64,
        index: usize,
        total: usize,
        command: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use crate::ui_text::command_card as ctxt;
        let theme = self.theme.clone();
        let copied = self.command_card_copied.is_some_and(|(id, i, at)| {
            id == card_id && i == index && at.elapsed() < FEEDBACK_DURATION
        });
        // ボタンの見た目は md コードブロックのコピーボタン（#680）に合わせる
        let button = |id: (&'static str, u64), icon: &'static str, text: String, on: bool| {
            div()
                .id((id.0, id.1))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(px(3.0))
                .px(px(5.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(hsla_alpha(
                    if on {
                        theme.green
                    } else {
                        theme.border_default
                    },
                    if on { 1.0 } else { 0.7 },
                ))
                .bg(hsla_alpha(
                    theme.surface_highlight,
                    if on { 1.0 } else { 0.8 },
                ))
                .text_size(px(9.5))
                .text_color(hsla(if on {
                    theme.green
                } else {
                    theme.text_secondary
                }))
                .opacity(if on { 1.0 } else { 0.75 })
                .hover(|d| d.opacity(1.0).bg(hsla(theme.surface_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path(icon)
                        .w(px(9.5))
                        .h(px(9.5))
                        .flex_none()
                        .text_color(hsla(if on {
                            theme.green
                        } else {
                            theme.text_secondary
                        })),
                )
                .child(SharedString::from(text))
        };
        let slot = (card_id << 8) | index as u64;
        div()
            .relative()
            .flex_shrink_0()
            .w_full()
            .px(px(9.0))
            .py(px(7.0))
            .rounded_md()
            .border_1()
            .border_color(hsla(theme.border_subtle))
            .bg(hsla(theme.mantle))
            .child(
                div()
                    // 等幅（コードブロックと同じ読み方）。長いコマンドは折り返す
                    .font_family(theme.font_family.clone())
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    // ボタンのぶんだけ 1 行目の右を空ける
                    .pr(px(if total > 1 { 132.0 } else { 118.0 }))
                    .text_color(hsla(theme.foreground))
                    .child(SharedString::from(command)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .right(px(4.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    // 複数行のカードでは何番目かを添える（帯 #703 と同じ表示）
                    .when(total > 1, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(9.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(ctxt::index_label(index, total))),
                        )
                    })
                    .child(
                        button(
                            ("chat-cmd-copy", slot),
                            if copied {
                                ui_icon::CHECK
                            } else {
                                ui_icon::COPY
                            },
                            if copied {
                                ctxt::copied().to_string()
                            } else {
                                ctxt::copy().to_string()
                            },
                            copied,
                        )
                        // 下の本文で選択が始まらないようにする（#725）
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(
                            move |this, _: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.copy_command_card(card_id, index, cx);
                            },
                        )),
                    )
                    .child(
                        button(
                            ("chat-cmd-run", slot),
                            ui_icon::PLAY,
                            ctxt::run().to_string(),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(
                            move |this, _: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.run_command_card(card_id, index, cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
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

        // #737 追加要件 3: 生きたスピナー行（作業内容 + 経過 + トークン数）は
        // **会話末尾の AI 側**（`render_chat_activity`）へ移した。ヘッダは
        // 「いまどっちの手番か」だけを短く出す簡素な表示に留める
        // （同じ情報を 2 か所で動かすと目が散る）
        let status_label: String = if state.queued {
            txt::chat_status_queued().to_string()
        } else if state.busy {
            txt::chat_status_busy().to_string()
        } else {
            txt::chat_status_idle().to_string()
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
                        .child(SharedString::from(status_label)),
                )
            })
            .child(div().flex_grow(1.0))
            // #739: 残量が少ないときは「/compact で軽くする」を**押せるボタン**で出す。
            // 警告色（下の ctx バー）と同じ根拠（`ctx_hint`）で出すので、赤いのに
            // 逃げ道が無い状態にならない。押下は G3 のスラッシュボタンと同一経路
            .when(state.ctx_hint() && !compact, |d| {
                d.child(
                    div()
                        .id(("chat-ctx-hint", pane_id.as_u64()))
                        .flex()
                        .flex_none()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(7.0))
                        .h(px(18.0))
                        .rounded(px(9.0))
                        .cursor_pointer()
                        .bg(rgba_alpha(theme.red, 0.14))
                        .border_1()
                        .border_color(hsla_alpha(theme.red, 0.45))
                        .text_size(px(10.0))
                        .text_color(hsla(theme.red))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.24)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.chat_slash_action(pane_id, SlashButton::Compact, cx);
                        }))
                        .child(SharedString::from(txt::chat_ctx_hint().to_string())),
                )
            })
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

    /// 作業中インジケータ（#737 追加要件 3）。
    ///
    /// **会話の末尾 = assistant の発話位置**に出す（Web 版 Claude のタイピング
    /// インジケータと同じ位置感）。中身は TUI のスピナー行そのもの
    /// （`Manifesting… (5m 16s · ↓ 16.4k tokens)` = 作業内容 + 経過 + 受信トークン）で、
    /// 取れないときだけ「考え中…」へ落ちる。
    /// 生成が終われば busy が false になり、本文の発話に自然に置き換わる
    fn render_chat_activity(
        &self,
        pane_id: PaneId,
        activity: Option<String>,
        queued: bool,
    ) -> gpui::AnyElement {
        let theme = &self.theme;
        use crate::ui_text::ui_mode as txt;
        let label = activity.unwrap_or_else(|| txt::chat_status_busy().to_string());
        // 明滅する点（タブバー・ヘッダの実行中ドットと同じ表現。新しい表現を増やさない）
        let dot = div()
            .w(px(7.0))
            .h(px(7.0))
            .flex_none()
            .rounded_full()
            .bg(hsla(theme.accent))
            .with_animation(
                ("chat-activity-pulse", pane_id.as_u64()),
                Animation::new(Duration::from_secs(2)).repeat(),
                |el, t| el.opacity(1.0 - 0.65 * (std::f32::consts::PI * t).sin()),
            );
        div()
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_start()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .px(px(12.0))
                    .py(px(7.0))
                    .rounded(px(10.0))
                    .bg(rgba(theme.surface_0))
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .child(dot)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.text_secondary))
                            .child(SharedString::from(label)),
                    )
                    // キューに滞留しているなら「あとで届く」ことも同じ場所で伝える
                    .when(queued, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(txt::chat_status_queued().to_string())),
                        )
                    }),
            )
            // コピーボタン列と幅を揃える（発話と左右の位置が揃う）
            .child(div().flex_none().w(px(CHAT_COPY_GUTTER)))
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

    /// 発話 1 件（user = 背景ブロック / assistant = 地の文 md）。
    ///
    /// `index` は選択の座標系（#725）。**本文のテキストはすべてここへ通す**ので、
    /// 描いた順がそのまま行番号になり、発話をまたいだ選択が成立する
    fn render_chat_message(
        &mut self,
        pane_id: PaneId,
        message: &ChatMessage,
        compact: bool,
        index: &mut ChatTextIndex,
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
            ChatRole::User => {
                let group = self.chat_message_group(pane_id, message.key);
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_end()
                    .w_full()
                    .group(group.clone())
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
                            .children(self.render_chat_user_text(pane_id, message, index, cx))
                            // #737 追加要件 5: 生成中に打った指示は、届くのが
                            // ターン終了後になる。吹き出しはすぐ出しつつ、
                            // 「まだ渡っていない」ことを小さく添える
                            .when(message.queued, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(9.5))
                                        .text_color(hsla(theme.text_faint))
                                        .child(SharedString::from(
                                            crate::ui_text::ui_mode::chat_queued_badge()
                                                .to_string(),
                                        )),
                                )
                            }),
                    )
                    .child(self.render_chat_message_gutter(pane_id, message, group, cx))
                    .into_any_element()
            }
            ChatRole::Assistant => {
                let group = self.chat_message_group(pane_id, message.key);
                let mut body = div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(4.0));
                if let Some(thinking) = message.thinking.clone() {
                    body = body.child(self.render_chat_fold(
                        pane_id,
                        message.key,
                        ChatSection::Thinking,
                        crate::ui_text::ui_mode::chat_thinking().to_string(),
                        None,
                        thinking,
                        index,
                        cx,
                    ));
                }
                if !message.text.trim().is_empty() {
                    let blocks = self.chat_md_blocks(message.key, &message.text);
                    // プレビューと同じ受け皿方式（#690）。幾何は md_view に任せ、
                    // ここは選択ハイライト・TextLayout の控え・コピーボタンだけ足す
                    let mut elements = Vec::with_capacity(blocks.len());
                    {
                        let mut sink = ChatMdSink {
                            app: self,
                            cx,
                            pane_id,
                            message_key: message.key,
                            index,
                        };
                        let mut code_blocks = 0usize;
                        for block in blocks.iter() {
                            let code_index =
                                matches!(block.kind, preview::MdBlockKind::CodeBlock { .. }).then(
                                    || {
                                        code_blocks += 1;
                                        code_blocks - 1
                                    },
                                );
                            elements.push(crate::md_view::render_block(
                                &theme, block, code_index, &mut sink,
                            ));
                        }
                    }
                    body = body.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .w_full()
                            .children(elements),
                    );
                }
                for (slot, tool) in message.tools.iter().enumerate() {
                    body = body.child(self.render_chat_fold(
                        pane_id,
                        message.key,
                        ChatSection::Tool(slot),
                        tool.name.clone(),
                        Some(tool.summary.clone()),
                        tool.summary.clone(),
                        index,
                        cx,
                    ));
                }
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_start()
                    .w_full()
                    .group(group.clone())
                    // #737 追加要件 4: assistant も枠付きブロックにする。
                    // 地の文だと ①どこまでが 1 発話か分からない ②コピーボタンの
                    // 対象範囲が見えない。user（右寄せ・`surface_1` の濃い背景）とは
                    // 「左寄せ・`surface_0` + subtle な枠」で区別する。
                    // thinking / ツール / コードブロック / インラインカードは
                    // すべて `body` の中なので枠の内側に収まる
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(rgba(theme.surface_0))
                            .border_1()
                            .border_color(hsla(theme.border_subtle))
                            .child(body),
                    )
                    .child(self.render_chat_message_gutter(pane_id, message, group, cx))
                    .into_any_element()
            }
        }
    }

    /// 発話ホバーの連動名（ボタンを濃くするための group）
    fn chat_message_group(&self, pane_id: PaneId, key: u64) -> SharedString {
        SharedString::from(format!("chat-msg-{}-{key}", pane_id.as_u64()))
    }

    /// 発話の右側に固定で確保する列（#725）。
    ///
    /// コピーボタンをここへ置くので、**絶対配置で本文へ被せない**。
    /// 本文が右端まで来ても文字とボタンが重ならないのが要点
    fn render_chat_message_gutter(
        &self,
        pane_id: PaneId,
        message: &ChatMessage,
        group: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // コピーできる中身が無い発話（画像だけ・ツールだけ）にはボタンを出さない
        let has_text = !message.text.trim().is_empty();
        div()
            .flex_none()
            .w(px(CHAT_COPY_GUTTER))
            .flex()
            .flex_row()
            .justify_end()
            .children(has_text.then(|| {
                self.render_chat_copy_button(
                    pane_id,
                    message.key,
                    ChatCopyTarget::Message,
                    group,
                    cx,
                )
            }))
            .into_any_element()
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
        index: &mut ChatTextIndex,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if message.text.is_empty() {
            return Vec::new();
        }
        let theme = self.theme.clone();
        // 選択できる本文は必ず索引経由で描く（#725）。素の SharedString だと
        // その行だけ当たり判定を持てず、発話をまたぐ選択に穴があく
        let styled = |index: &mut ChatTextIndex, text: String| {
            index
                .push(&theme, text, Vec::new(), theme.foreground, None)
                .into_any_element()
        };
        let total = message.text.chars().count();
        if total <= LONG_MESSAGE_CHARS {
            return vec![styled(index, message.text.clone())];
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
            styled(index, shown),
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
                    // 行数が変わる = 行番号の意味が変わるので選択は捨てる（#725）
                    this.clear_chat_selection(pane_id);
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
        index: &mut ChatTextIndex,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let expanded = self.chat_expanded(pane_id, key, section);
        let element_id = chat_element_id(pane_id, key, section.slot());
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
                        // 行数が変わる = 行番号の意味が変わるので選択は捨てる（#725）
                        this.clear_chat_selection(pane_id);
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
                        .text_size(px(11.0))
                        // 本文は索引経由（#725。開いた thinking / ツール出力も選択できる）
                        .child(index.push(&theme, body, Vec::new(), theme.text_secondary, None)),
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
        // #737: 会話末尾の作業中インジケータの **有無**（文言は入れない）。
        // スピナー行は経過秒とトークン数が毎秒変わるので、文言を鍵に混ぜると
        // 毎フレーム「新着」になり、上へスクロールして読んでいる最中に下端へ
        // 引き戻してしまう（追従を外す仕組みが無効化される）
        activity: bool,
    ) -> bool {
        let key = messages.last().map(|m| m.key).unwrap_or(0)
            ^ (messages.len() as u64).rotate_left(32)
            ^ u64::from(approval).rotate_left(16)
            ^ u64::from(activity).rotate_left(8);
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
        // #739: 開いたままのプロファイル選択も落とす（消えたペインへ起動しない）
        if self
            .starter_profile_menu
            .is_some_and(|(p, ..)| p == pane_id)
        {
            self.starter_profile_menu = None;
        }
        // #720: 過渡期の記録もペインと一緒に落とす（ペイン ID は再利用される。#390）
        self.pane_settle.remove(&pane_id);
        // #716: echo・確認待ちもペインと一緒に落とす
        // （ペイン ID は再利用されるので残すと他人の表示が現れる。#390）。
        // #719 以降は下書きを持たない = 入力は TUI 側にあり、ペインと運命を共にする
        self.chat_echo.remove(&pane_id);
        self.chat_long_expanded.retain(|(pane, _)| *pane != pane_id);
        if self.chat_clear_confirm == Some(pane_id) {
            self.chat_clear_confirm = None;
        }
        // #725: 選択と行索引もペインと一緒に落とす（ペイン ID は再利用される。#390）
        self.chat_text_index.remove(&pane_id);
        self.chat_selections.remove(&pane_id);
        if self.chat_selecting == Some(pane_id) {
            self.chat_selecting = None;
        }
        if self.chat_copied.is_some_and(|(pane, ..)| pane == pane_id) {
            self.chat_copied = None;
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

    /// #739: `/compact` ヒントは警告色と同じ境界（80% 使用）で出る。
    /// ctx% が取れないときは出さない（根拠が無いのに急かさない）
    #[test]
    fn compactヒントは残量警告と同じ境界で出る() {
        let state = |ctx: Option<f64>| ChatPaneState {
            ctx_percent: ctx,
            ..Default::default()
        };
        assert!(!state(None).ctx_hint(), "ctx% 不明ならヒントも出さない");
        assert!(!state(Some(79.9)).ctx_hint());
        assert!(state(Some(80.0)).ctx_hint(), "境界そのものは警告側");
        assert!(state(Some(97.0)).ctx_hint());
        // 警告色とヒントは必ず同時（片方だけ出る状態を作らない）
        for used in [0.0, 50.0, 79.9, 80.0, 99.0, 140.0] {
            let s = state(Some(used));
            assert_eq!(
                s.ctx_hint(),
                s.ctx_gauge().expect("値がある").1,
                "使用率 {used}% でヒントと警告色がずれた"
            );
        }
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

    // --- 選択とコピー（#725） ---

    /// コピー本文は「画面と同じプレーンテキスト」（md 記法が残らない）。
    /// ブロック間は空行で、表のセル・コード行はブロック内の改行になる
    fn plain(md: &str) -> String {
        md_plain_text(&crate::preview::markdown_blocks(md))
    }

    #[test]
    fn コピー本文はmd記法を落として空行でブロックを分ける() {
        let text = plain("# 見出し\n\n本文の **強調** と `code`。\n\n- 一つ目\n- 二つ目\n");
        assert_eq!(text, "見出し\n\n本文の 強調 と code。\n\n一つ目\n\n二つ目");
        // コードブロックはフェンスを落として中身だけ（#680 のコピーと同じ規則）
        let code = plain("説明\n\n```rust\nfn main() {}\nlet x = 1;\n```\n");
        assert_eq!(code, "説明\n\nfn main() {}\nlet x = 1;");
        // 罫線は文字を持たないので空行が重ならない
        assert_eq!(plain("上\n\n---\n\n下"), "上\n\n下");
        // 表はセル 1 つが 1 行（選択で掃いたときと同じ並び）
        assert_eq!(plain("| a | b |\n|---|---|\n| 1 | 2 |\n"), "a\nb\n1\n2");
        // 空 md でも panic しない
        assert_eq!(plain(""), "");
    }

    /// 選択の切り出しはプレビューとチャットで同一実装（#725 で 1 本化した）
    #[test]
    fn 選択テキストは行をまたいで連結される() {
        let texts = vec![
            "こんにちは".to_string(),
            "world".to_string(),
            "さようなら".to_string(),
        ];
        let sel = |anchor, head| crate::PreviewSelection { anchor, head };
        // 単一行の一部
        assert_eq!(
            crate::selection_text(&texts, &sel((1, 0), (1, 3))),
            Some("wor".into())
        );
        // 複数行（発話をまたぐ選択と同じ形）。逆ドラッグでも同じ結果
        let forward = crate::selection_text(&texts, &sel((0, 3), (2, 3)));
        let backward = crate::selection_text(&texts, &sel((2, 3), (0, 3)));
        assert_eq!(forward.as_deref(), Some("んにちは\nworld\nさ"));
        assert_eq!(forward, backward);
        // クリックしただけ（長さ 0）は None = ⌘C が次の候補へ落ちる
        assert_eq!(crate::selection_text(&texts, &sel((1, 2), (1, 2))), None);
        // 範囲外・空リストでも panic せず None
        assert_eq!(crate::selection_text(&[], &sel((0, 0), (0, 1))), None);
        assert_eq!(crate::selection_text(&texts, &sel((9, 0), (9, 1))), None);
        // 行末を超える列は行長へ丸める（UTF-8 境界も割らない）
        assert_eq!(
            crate::selection_text(&texts, &sel((0, 0), (0, 999))),
            Some("こんにちは".into())
        );
        assert_eq!(
            crate::selection_text(&texts, &sel((0, 1), (0, 5))),
            Some("こ".into()),
            "文字の途中の byte は手前の境界へ丸める"
        );
    }

    /// 選択ハイライトは選択のある行にだけ乗り、色は accent の半透明
    #[test]
    fn 選択ハイライトは対象行にだけ乗る() {
        let theme = tako_core::theme::Theme::default();
        let selection = crate::PreviewSelection {
            anchor: (1, 1),
            head: (1, 4),
        };
        let mut highlights = Vec::new();
        push_selection_highlight(&mut highlights, "abcdef", 0, Some(&selection), &theme);
        assert!(highlights.is_empty(), "選択外の行には乗らない");
        push_selection_highlight(&mut highlights, "abcdef", 1, Some(&selection), &theme);
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].0, 1..4);
        assert_eq!(
            highlights[0].1.background_color,
            Some(hsla_alpha(theme.accent, 0.35))
        );
        // 選択が無いときは何も積まない
        let mut none = Vec::new();
        push_selection_highlight(&mut none, "abcdef", 1, None, &theme);
        assert!(none.is_empty());
    }

    /// 索引は「描いた順 = 行番号」。文字の無い行（罫線）も 1 行ぶん進める
    #[test]
    fn 行索引は描いた順に採番されテキストを覚える() {
        let theme = tako_core::theme::Theme::default();
        let mut index = ChatTextIndex::default();
        let _ = index.push(&theme, "一行目".into(), Vec::new(), theme.foreground, None);
        index.push_spacer();
        let _ = index.push(&theme, "三行目".into(), Vec::new(), theme.foreground, None);
        assert_eq!(index.texts, vec!["一行目", "", "三行目"]);
        assert_eq!(index.layouts.len(), 3);
        assert!(index.layouts[1].is_none(), "罫線はレイアウトを持たない");
        // 空行は表示上は空白 1 個に置き換わるが、コピーの正は空文字のまま
        let mut empty = ChatTextIndex::default();
        let _ = empty.push(&theme, String::new(), Vec::new(), theme.foreground, None);
        assert_eq!(empty.texts, vec![""]);
    }
}

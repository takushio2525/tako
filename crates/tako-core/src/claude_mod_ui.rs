//! claude_mod_ui — tako mod の定型の UI 設定（`<data_dir>/claude-mod/ui.json`。#1960 / エピック #1877 の S7-2）
//!
//! 設計の正本は `.agent/plans/2026-10-tako-mod.md` §9.7。
//!
//! - **置き場は tako の data dir**（Claude Code の設定 dir へは書かない = FR-2.42.2 と矛盾しない）。
//!   mod はファイルを読まず、報告の応答 `tako.view.ui` で**検証済みの値**を受け取る
//!   （tako の外では効かない・壊れた値は mod へ届かない・`$.store` の読み込み元ごとの分裂に左右されない）
//! - **選ぶだけの語彙**（[`Vocab`]）: 帯の区切り・使用量のバーの置き場と項目・カード / チャットの置き場・
//!   色（Claude Code のテーマのキー）・ボタン（[`MAX_BUTTONS`] 個まで。動作は slash / tako / shell / prompt の
//!   4 種の定型で、**任意の JS は持たない**）
//! - **口は 1 本**: CLI `tako mod ui`・MCP `tako_mod` の `action=ui`・`tako setup`（対話と `--answers` の
//!   `mod_ui`）がすべて [`UiOp`] → [`apply_all`] を通る。不正な値は**書かずに**理由と許される値を返す
//!   （[`UiError`]）。軽いモデルが叩いても、語彙の外の値はファイルへ届かない
//! - **読めない・一部だけ壊れた ui.json**: 読める部分は使い、壊れた部分だけ既定へ落とす（[`parse_lenient`]）。
//!   元の中身は `.unreadable.bak` へ保全する（#916 の機構 = [`crate::migration::quarantine_unreadable`]）。
//!   新しい tako が書いた形（`schema_version` が大きい）は読める範囲で使い、**書き換えない**
//! - **帯のトグルの正本**（S3 #1881 の `$.store` から移した）: `band.hidden` と `band.toggled_at`（epoch ms）。
//!   mod の報告の `band.toggled_at` の方が新しければ取り込む（[`import_band`]）。mod へは応答の
//!   `band_request` で中継する（今の mod はそれを見て従う）
//!
//! 形を変えるとき（キーの増減・語彙の削除）は [`SCHEMA_VERSION`] を上げ、`tako-control::migrations` へ
//! `Step` を足す（`migration_registry` の指紋が落ちて知らせる）。語彙を**足す**だけなら旧い tako は
//! その値を「壊れた部分」として既定へ落とし、元を退避する（消さない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use unicode_width::UnicodeWidthStr;

/// ファイル名（置き場は [`ui_path`]）
pub const FILENAME: &str = "ui.json";
/// 今の形の版
pub const SCHEMA_VERSION: u32 = 1;
/// ボタンの上限（帯は 1 行に詰めるので多くは並ばない）
pub const MAX_BUTTONS: usize = 8;
/// ボタンのラベルの上限（表示の桁。全角は 2 桁）
pub const MAX_LABEL_COLUMNS: usize = 16;
/// ボタンの id の上限（文字数。`[a-z0-9-]`）
pub const MAX_ID_CHARS: usize = 24;
/// `prompt` の文の上限（文字数）
pub const MAX_PROMPT_CHARS: usize = 1000;

/// `<data_dir>/claude-mod/ui.json`（mod の展開先 `claude-mod/tako/` の隣）
pub fn ui_path() -> Option<PathBuf> {
    crate::claude_mod::mod_root().map(|r| r.join(FILENAME))
}

// --- 語彙 -------------------------------------------------------------------------

/// 選ぶだけの語彙。`parse` は大文字小文字・`-` と `_` の違い・先頭の `/` を区別しない
/// （軽いモデルが `/compact` や `split_right` と書いても同じ値に落ちる）
pub trait Vocab: Copy + PartialEq + std::fmt::Debug + Sized + 'static {
    const ALL: &'static [Self];
    /// ui.json に書く語（serde の名前と同じ。単体テストが突き合わせる）
    fn as_str(self) -> &'static str;
    /// 別名（`5h` → five_hour 等）
    fn aliases(self) -> &'static [&'static str] {
        &[]
    }
    fn parse(word: &str) -> Option<Self> {
        let want = canonical(word);
        if want.is_empty() {
            return None;
        }
        Self::ALL.iter().copied().find(|v| {
            canonical(v.as_str()) == want || v.aliases().iter().any(|a| canonical(a) == want)
        })
    }
    fn words() -> Vec<&'static str> {
        Self::ALL.iter().map(|v| v.as_str()).collect()
    }
}

/// 比べるための正規形（小文字・`-` `_` 空白を落とす・先頭の `/` を落とす）
fn canonical(word: &str) -> String {
    word.trim()
        .trim_start_matches('/')
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 帯（プロンプトの上の 1 行）に並べる区切り。並び順のまま描く
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandSegment {
    Pane,
    Tab,
    Workers,
    Attention,
    Ctx,
    Limits,
    Buttons,
    Card,
}

impl Vocab for BandSegment {
    const ALL: &'static [Self] = &[
        Self::Pane,
        Self::Tab,
        Self::Workers,
        Self::Attention,
        Self::Ctx,
        Self::Limits,
        Self::Buttons,
        Self::Card,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Tab => "tab",
            Self::Workers => "workers",
            Self::Attention => "attention",
            Self::Ctx => "ctx",
            Self::Limits => "limits",
            Self::Buttons => "buttons",
            Self::Card => "card",
        }
    }
}

/// 使用制限・ctx のバーの置き場
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageBarPlace {
    PromptHint,
    Band,
    Off,
}

impl Vocab for UsageBarPlace {
    const ALL: &'static [Self] = &[Self::PromptHint, Self::Band, Self::Off];
    fn as_str(self) -> &'static str {
        match self {
            Self::PromptHint => "prompt_hint",
            Self::Band => "band",
            Self::Off => "off",
        }
    }
}

/// バーの項目
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageItem {
    FiveHour,
    SevenDay,
    Ctx,
}

impl Vocab for UsageItem {
    const ALL: &'static [Self] = &[Self::FiveHour, Self::SevenDay, Self::Ctx];
    fn as_str(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::SevenDay => "seven_day",
            Self::Ctx => "ctx",
        }
    }
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::FiveHour => &["5h"],
            Self::SevenDay => &["7d"],
            Self::Ctx => &[],
        }
    }
}

/// カード・チャットを誰が描くか（auto = mod が効いていれば mod、効いていなければ tako）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Auto,
    Mod,
    Tako,
}

impl Vocab for Place {
    const ALL: &'static [Self] = &[Self::Auto, Self::Mod, Self::Tako];
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mod => "mod",
            Self::Tako => "tako",
        }
    }
}

/// チャット風の描き方（吹き出し・ツールの要約）をいつ出すか
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatShow {
    Always,
    GuiOnly,
    Off,
}

impl Vocab for ChatShow {
    const ALL: &'static [Self] = &[Self::Always, Self::GuiOnly, Self::Off];
    fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::GuiOnly => "gui_only",
            Self::Off => "off",
        }
    }
}

/// 色は Claude Code のテーマのキーだけ（2.1.294 の `ThemeKey` のうち UI の部品に使えるもの。
/// diff 専用のキーは入れない）。値（RGB）は持たない = 利用者のテーマに従う
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeColor {
    Text,
    Inactive,
    Subtle,
    Suggestion,
    Remember,
    Success,
    Error,
    Warning,
    Merged,
    Claude,
    Permission,
    PlanMode,
    AutoAccept,
    Ide,
}

impl Vocab for ThemeColor {
    const ALL: &'static [Self] = &[
        Self::Text,
        Self::Inactive,
        Self::Subtle,
        Self::Suggestion,
        Self::Remember,
        Self::Success,
        Self::Error,
        Self::Warning,
        Self::Merged,
        Self::Claude,
        Self::Permission,
        Self::PlanMode,
        Self::AutoAccept,
        Self::Ide,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Inactive => "inactive",
            Self::Subtle => "subtle",
            Self::Suggestion => "suggestion",
            Self::Remember => "remember",
            Self::Success => "success",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Merged => "merged",
            Self::Claude => "claude",
            Self::Permission => "permission",
            Self::PlanMode => "planMode",
            Self::AutoAccept => "autoAccept",
            Self::Ide => "ide",
        }
    }
}

/// `slash` のボタンが送れる組み込みコマンドの許可リスト（と `/tako`）。
/// 描く側（mod）は `$.command.list()` に無い名前を描かない（#1962）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommand {
    Compact,
    Clear,
    Context,
    Cost,
    Model,
    Effort,
    Tako,
}

impl Vocab for SlashCommand {
    const ALL: &'static [Self] = &[
        Self::Compact,
        Self::Clear,
        Self::Context,
        Self::Cost,
        Self::Model,
        Self::Effort,
        Self::Tako,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Clear => "clear",
            Self::Context => "context",
            Self::Cost => "cost",
            Self::Model => "model",
            Self::Effort => "effort",
            Self::Tako => "tako",
        }
    }
}

impl SlashCommand {
    /// 何をするか（`tako mod ui` の一覧と setup の対話に出す）
    pub fn describe(self) -> &'static str {
        match self {
            Self::Compact => "会話を要約して ctx を空ける",
            Self::Clear => "会話を消して新しく始める",
            Self::Context => "ctx の使い方を見る",
            Self::Cost => "このセッションの費用を見る",
            Self::Model => "モデルを選び直す",
            Self::Effort => "effort を選び直す",
            Self::Tako => "tako のサイドバーを開く",
        }
    }
}

/// `tako` のボタンが呼べる tako の操作（#1882 の対応表 = ペインの右クリックの操作の id）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TakoOp {
    SplitRight,
    SplitDown,
    SessionRestartHarness,
    SessionRestartHandoff,
    LimitResumeOn,
    LimitResumeOff,
    Background,
    Close,
    OpenCwd,
}

impl Vocab for TakoOp {
    const ALL: &'static [Self] = &[
        Self::SplitRight,
        Self::SplitDown,
        Self::SessionRestartHarness,
        Self::SessionRestartHandoff,
        Self::LimitResumeOn,
        Self::LimitResumeOff,
        Self::Background,
        Self::Close,
        Self::OpenCwd,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::SplitRight => "split-right",
            Self::SplitDown => "split-down",
            Self::SessionRestartHarness => "session-restart-harness",
            Self::SessionRestartHandoff => "session-restart-handoff",
            Self::LimitResumeOn => "limit-resume-on",
            Self::LimitResumeOff => "limit-resume-off",
            Self::Background => "background",
            Self::Close => "close",
            Self::OpenCwd => "open-cwd",
        }
    }
}

impl TakoOp {
    /// 既定のラベル（[`MAX_LABEL_COLUMNS`] に収まる短い名前）
    pub fn label(self) -> &'static str {
        match self {
            Self::SplitRight => "split right",
            Self::SplitDown => "split down",
            Self::SessionRestartHarness => "restart",
            Self::SessionRestartHandoff => "handoff",
            Self::LimitResumeOn => "resume on",
            Self::LimitResumeOff => "resume off",
            Self::Background => "background",
            Self::Close => "close",
            Self::OpenCwd => "open cwd",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::SplitRight => "右に新しいペインを開く",
            Self::SplitDown => "下に新しいペインを開く",
            Self::SessionRestartHarness => "会話を保ったまま claude を起動し直す",
            Self::SessionRestartHandoff => "引き継ぎを書かせてセッションを交代する",
            Self::LimitResumeOn => "使用制限が解けたら自動で続ける（on）",
            Self::LimitResumeOff => "使用制限の自動復帰を止める（off）",
            Self::Background => "このペインを裏へ回す",
            Self::Close => "このペインを閉じる",
            Self::OpenCwd => "このペインのフォルダを開く",
        }
    }
}

/// ボタンの動作の種類（ui.json の `action.kind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonKind {
    Slash,
    Tako,
    Shell,
    Prompt,
}

impl Vocab for ButtonKind {
    const ALL: &'static [Self] = &[Self::Slash, Self::Tako, Self::Shell, Self::Prompt];
    fn as_str(self) -> &'static str {
        match self {
            Self::Slash => "slash",
            Self::Tako => "tako",
            Self::Shell => "shell",
            Self::Prompt => "prompt",
        }
    }
}

impl ButtonKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Slash => "Claude Code の組み込みコマンドを送る（/compact 等。許可リストだけ）",
            Self::Tako => "tako の操作を呼ぶ（ペインの右クリックと同じ操作）",
            Self::Shell => "書いたコマンドを新しい tako のペインで実行する（4096 バイトまで）",
            Self::Prompt => "入力欄へ文を入れる（送らない。1000 文字まで）",
        }
    }
}

/// 3 択の雛形（setup の対話・`tako mod ui preset`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    Default,
    Recommended,
    Minimal,
}

impl Vocab for Preset {
    const ALL: &'static [Self] = &[Self::Default, Self::Recommended, Self::Minimal];
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Recommended => "recommended",
            Self::Minimal => "minimal",
        }
    }
}

impl Preset {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Default => "既定（ボタンは compact だけ）",
            Self::Recommended => "おすすめ（ボタン: compact / context / split right / handoff）",
            Self::Minimal => {
                "最小（帯はペイン名・要注意・ボタンだけ、バーは ctx だけ、チャットの装飾なし）"
            }
        }
    }

    /// 雛形の中身。帯の隠す / 出すは雛形に含めない（呼び出し側が今の値を保つ）
    pub fn config(self) -> UiConfig {
        let mut cfg = UiConfig::default();
        match self {
            Self::Default => {}
            Self::Recommended => {
                for (kind, value) in [
                    (ButtonKind::Slash, "context"),
                    (ButtonKind::Tako, "split-right"),
                    (ButtonKind::Tako, "session-restart-handoff"),
                ] {
                    let spec = ButtonSpec {
                        kind: Some(kind.as_str().to_string()),
                        value: value.to_string(),
                        ..ButtonSpec::default()
                    };
                    let button = build_button(&spec, &cfg.buttons)
                        .expect("おすすめの雛形は語彙の中で組む（単体テストが固定）");
                    cfg.buttons.push(button);
                }
            }
            Self::Minimal => {
                cfg.band.segments = vec![
                    BandSegment::Pane,
                    BandSegment::Attention,
                    BandSegment::Buttons,
                ];
                cfg.usage_bar.items = vec![UsageItem::Ctx];
                cfg.chat.bubble = ChatShow::Off;
                cfg.chat.tool_summary = ChatShow::Off;
            }
        }
        cfg
    }
}

/// `set` で変えられるキー（ui.json の位置を `.` でつないだ名前）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetKey {
    BandHidden,
    BandSegments,
    UsageBarPlace,
    UsageBarItems,
    CardsPlace,
    ChatPlace,
    ChatBubble,
    ChatCodeCopy,
    ChatToolSummary,
    ColorsAccent,
    ColorsWarn,
    ColorsDim,
}

impl Vocab for SetKey {
    const ALL: &'static [Self] = &[
        Self::BandHidden,
        Self::BandSegments,
        Self::UsageBarPlace,
        Self::UsageBarItems,
        Self::CardsPlace,
        Self::ChatPlace,
        Self::ChatBubble,
        Self::ChatCodeCopy,
        Self::ChatToolSummary,
        Self::ColorsAccent,
        Self::ColorsWarn,
        Self::ColorsDim,
    ];
    fn as_str(self) -> &'static str {
        match self {
            Self::BandHidden => "band.hidden",
            Self::BandSegments => "band.segments",
            Self::UsageBarPlace => "usage_bar.place",
            Self::UsageBarItems => "usage_bar.items",
            Self::CardsPlace => "cards.place",
            Self::ChatPlace => "chat.place",
            Self::ChatBubble => "chat.bubble",
            Self::ChatCodeCopy => "chat.code_copy",
            Self::ChatToolSummary => "chat.tool_summary",
            Self::ColorsAccent => "colors.accent",
            Self::ColorsWarn => "colors.warn",
            Self::ColorsDim => "colors.dim",
        }
    }
}

/// 値の形（`tako mod ui` の choices と、値の読み方を決める）
enum Shape {
    Bool,
    One(Vec<&'static str>),
    /// 並びのある部分集合（重複なし・1 つ以上）
    List(Vec<&'static str>),
}

impl SetKey {
    fn shape(self) -> Shape {
        match self {
            Self::BandHidden | Self::ChatCodeCopy => Shape::Bool,
            Self::BandSegments => Shape::List(BandSegment::words()),
            Self::UsageBarItems => Shape::List(UsageItem::words()),
            Self::UsageBarPlace => Shape::One(UsageBarPlace::words()),
            Self::CardsPlace | Self::ChatPlace => Shape::One(Place::words()),
            Self::ChatBubble | Self::ChatToolSummary => Shape::One(ChatShow::words()),
            Self::ColorsAccent | Self::ColorsWarn | Self::ColorsDim => {
                Shape::One(ThemeColor::words())
            }
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::BandHidden => "帯（プロンプトの上の 1 行）を隠す",
            Self::BandSegments => "帯に並べる区切りと順番",
            Self::UsageBarPlace => {
                "使用制限・ctx のバーの置き場（prompt_hint = 入力欄の下の行の末尾）"
            }
            Self::UsageBarItems => "バーに出す項目と順番",
            Self::CardsPlace => "コマンドカードを誰が描くか",
            Self::ChatPlace => "チャット風の描画を誰が描くか",
            Self::ChatBubble => "発言の吹き出し",
            Self::ChatCodeCopy => "コードブロックの下のコピーボタン",
            Self::ChatToolSummary => "ツール呼び出しの要約",
            Self::ColorsAccent => "強調の色（テーマのキー）",
            Self::ColorsWarn => "警告の色（テーマのキー）",
            Self::ColorsDim => "控えめな文字の色（テーマのキー）",
        }
    }

    /// 今の値（表示用。ui.json の語のまま）
    fn current(self, cfg: &UiConfig) -> Value {
        fn list<T: Vocab>(items: &[T]) -> Value {
            Value::from(items.iter().map(|v| v.as_str()).collect::<Vec<_>>())
        }
        match self {
            Self::BandHidden => json!(cfg.band.hidden),
            Self::BandSegments => list(&cfg.band.segments),
            Self::UsageBarPlace => json!(cfg.usage_bar.place.as_str()),
            Self::UsageBarItems => list(&cfg.usage_bar.items),
            Self::CardsPlace => json!(cfg.cards.place.as_str()),
            Self::ChatPlace => json!(cfg.chat.place.as_str()),
            Self::ChatBubble => json!(cfg.chat.bubble.as_str()),
            Self::ChatCodeCopy => json!(cfg.chat.code_copy),
            Self::ChatToolSummary => json!(cfg.chat.tool_summary.as_str()),
            Self::ColorsAccent => json!(cfg.colors.accent.as_str()),
            Self::ColorsWarn => json!(cfg.colors.warn.as_str()),
            Self::ColorsDim => json!(cfg.colors.dim.as_str()),
        }
    }
}

// --- 形 ----------------------------------------------------------------------------

/// ui.json の全体（`tako.view.ui` も同じ形。mod の契約は `claude-mod/types/index.d.ts` の `TakoUi`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    pub schema_version: u32,
    pub band: BandUi,
    pub usage_bar: UsageBarUi,
    pub buttons: Vec<Button>,
    pub cards: CardsUi,
    pub chat: ChatUi,
    pub colors: ColorsUi,
}

/// 帯
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandUi {
    pub hidden: bool,
    pub segments: Vec<BandSegment>,
    /// `hidden` を最後に変えた時刻（epoch ms）。mod の `$.store` の時刻と比べて新しい方が勝つ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggled_at: Option<u64>,
}

/// 使用制限・ctx のバー
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageBarUi {
    pub place: UsageBarPlace,
    pub items: Vec<UsageItem>,
}

/// カスタムボタン
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Button {
    pub id: String,
    pub label: String,
    pub hotkey: String,
    pub action: ButtonAction,
}

/// ボタンの動作（4 種の定型だけ）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ButtonAction {
    Slash { command: SlashCommand },
    Tako { op: TakoOp },
    Shell { command: String },
    Prompt { text: String },
}

impl ButtonAction {
    pub fn kind(&self) -> ButtonKind {
        match self {
            Self::Slash { .. } => ButtonKind::Slash,
            Self::Tako { .. } => ButtonKind::Tako,
            Self::Shell { .. } => ButtonKind::Shell,
            Self::Prompt { .. } => ButtonKind::Prompt,
        }
    }

    /// 1 行の説明（一覧の表示用。shell / prompt の中身は先頭だけ）
    pub fn summary(&self) -> String {
        match self {
            Self::Slash { command } => format!("slash /{}", command.as_str()),
            Self::Tako { op } => format!("tako {}", op.as_str()),
            Self::Shell { command } => format!("shell {}", clip_columns(command, 40)),
            Self::Prompt { text } => format!("prompt {}", clip_columns(text, 40)),
        }
    }
}

/// コマンドカード
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardsUi {
    pub place: Place,
}

/// チャット風の描画
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatUi {
    pub place: Place,
    pub bubble: ChatShow,
    pub code_copy: bool,
    pub tool_summary: ChatShow,
}

/// 色（テーマのキー）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorsUi {
    pub accent: ThemeColor,
    pub warn: ThemeColor,
    pub dim: ThemeColor,
}

impl Default for BandUi {
    fn default() -> Self {
        Self {
            hidden: false,
            segments: vec![
                BandSegment::Pane,
                BandSegment::Tab,
                BandSegment::Workers,
                BandSegment::Attention,
                BandSegment::Card,
                BandSegment::Buttons,
            ],
            toggled_at: None,
        }
    }
}

impl Default for UsageBarUi {
    fn default() -> Self {
        Self {
            place: UsageBarPlace::PromptHint,
            items: vec![UsageItem::FiveHour, UsageItem::SevenDay, UsageItem::Ctx],
        }
    }
}

impl Default for CardsUi {
    fn default() -> Self {
        Self { place: Place::Auto }
    }
}

impl Default for ChatUi {
    fn default() -> Self {
        Self {
            place: Place::Auto,
            bubble: ChatShow::GuiOnly,
            code_copy: true,
            tool_summary: ChatShow::GuiOnly,
        }
    }
}

impl Default for ColorsUi {
    fn default() -> Self {
        Self {
            accent: ThemeColor::Suggestion,
            warn: ThemeColor::Warning,
            dim: ThemeColor::Subtle,
        }
    }
}

/// 既定のボタン = `/compact` のワンボタン 1 個（ユーザーの依頼の例そのもの）
pub fn default_buttons() -> Vec<Button> {
    vec![Button {
        id: "compact".into(),
        label: "compact".into(),
        hotkey: "c".into(),
        action: ButtonAction::Slash {
            command: SlashCommand::Compact,
        },
    }]
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            band: BandUi::default(),
            usage_bar: UsageBarUi::default(),
            buttons: default_buttons(),
            cards: CardsUi::default(),
            chat: ChatUi::default(),
            colors: ColorsUi::default(),
        }
    }
}

impl UiConfig {
    /// ui.json に書く本文（整形 + 末尾改行。同じ値なら必ず同じバイト列）
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        text.push('\n');
        text
    }

    /// 応答の `tako.view.band_request` の元（`toggled_at` が無い = 誰も切り替えていない = 中継しない）
    pub fn band_request(&self) -> Option<crate::claude_mod::BandRequest> {
        self.band
            .toggled_at
            .map(|at| crate::claude_mod::BandRequest {
                hidden: self.band.hidden,
                at,
            })
    }
}

// --- 検証 --------------------------------------------------------------------------
//
// 問題の文言には**利用者が書いた値を入れない**（キーの位置と種類だけ）。persist.log へ出るので、
// prompt の文や shell のコマンドが診断ログへ流れない（AGENTS.md の絶対ルール）

/// 1 つの部分の問題
fn segments_problems(segments: &[BandSegment]) -> Vec<String> {
    let mut out = Vec::new();
    if segments.is_empty() {
        out.push("band.segments が空（隠すなら band.hidden を true にする）".into());
    }
    if has_duplicate(segments) {
        out.push("band.segments に同じ区切りが 2 回ある".into());
    }
    out
}

fn items_problems(items: &[UsageItem]) -> Vec<String> {
    let mut out = Vec::new();
    if items.is_empty() {
        out.push("usage_bar.items が空（消すなら usage_bar.place を off にする）".into());
    }
    if has_duplicate(items) {
        out.push("usage_bar.items に同じ項目が 2 回ある".into());
    }
    out
}

fn has_duplicate<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(i, a)| items[..i].iter().any(|b| b == a))
}

/// ボタン 1 個の問題（`at` は `buttons[i]` の位置）
fn button_problems(button: &Button, at: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Err(e) = check_id(&button.id) {
        out.push(format!("{at}.id: {}", e.what));
    }
    if let Err(e) = check_label(&button.label) {
        out.push(format!("{at}.label: {}", e.what));
    }
    if let Err(e) = check_hotkey(&button.hotkey) {
        out.push(format!("{at}.hotkey: {}", e.what));
    }
    match &button.action {
        ButtonAction::Shell { command } => {
            if let Err(e) = normalize_shell(command) {
                out.push(format!("{at}.action.command: {}", e.what));
            } else if normalize_shell(command).ok().as_deref() != Some(command.as_str()) {
                out.push(format!(
                    "{at}.action.command: 前後の空白・改行を落とした形で書く"
                ));
            }
        }
        ButtonAction::Prompt { text } => {
            if let Err(e) = normalize_prompt(text) {
                out.push(format!("{at}.action.text: {}", e.what));
            } else if normalize_prompt(text).ok().as_deref() != Some(text.as_str()) {
                out.push(format!(
                    "{at}.action.text: 前後の空白・改行を落とした形で書く"
                ));
            }
        }
        ButtonAction::Slash { .. } | ButtonAction::Tako { .. } => {}
    }
    out
}

/// ボタンの並び全体の問題（個数・id と hotkey の重複）
fn buttons_problems(buttons: &[Button]) -> Vec<String> {
    let mut out = Vec::new();
    if buttons.len() > MAX_BUTTONS {
        out.push(format!(
            "buttons が {} 個（{MAX_BUTTONS} 個まで）",
            buttons.len()
        ));
    }
    for (i, b) in buttons.iter().enumerate() {
        out.extend(button_problems(b, &format!("buttons[{i}]")));
        if buttons[..i].iter().any(|o| o.id == b.id) {
            out.push(format!("buttons[{i}].id が前のボタンと重複"));
        }
        if buttons[..i].iter().any(|o| o.hotkey == b.hotkey) {
            out.push(format!("buttons[{i}].hotkey が前のボタンと重複"));
        }
    }
    out
}

/// 全体の問題（空 = スキーマを満たす。[`validate_text`] と性質テストの判定はこれ 1 本）
pub fn problems(cfg: &UiConfig) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.schema_version != SCHEMA_VERSION {
        out.push(format!("schema_version は {SCHEMA_VERSION}"));
    }
    out.extend(segments_problems(&cfg.band.segments));
    out.extend(items_problems(&cfg.usage_bar.items));
    out.extend(buttons_problems(&cfg.buttons));
    out
}

fn check_id(id: &str) -> Result<(), UiError> {
    let ok = !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(UiError::new(format!(
            "id は英小文字・数字・`-` の {MAX_ID_CHARS} 文字まで（先頭は英数字）"
        )))
    }
}

fn check_label(label: &str) -> Result<(), UiError> {
    if label.trim() != label || label.is_empty() {
        return Err(UiError::new("label は空にできない（前後の空白も不可）"));
    }
    if label.chars().any(char::is_control) {
        return Err(UiError::new("label に改行・制御文字は使えない"));
    }
    let columns = UnicodeWidthStr::width(label);
    if columns > MAX_LABEL_COLUMNS {
        return Err(UiError::new(format!(
            "label は {MAX_LABEL_COLUMNS} 桁まで（全角は 2 桁。指定は {columns} 桁）"
        )));
    }
    Ok(())
}

/// hotkey の語（数字 1 字か英小文字 1 字）。英字を先に並べる（エンジンの仕様で、空の入力欄の
/// 数字は帯のボタンを押してしまうので、自動の割り当ては英字を先に使う）
fn hotkey_alphabet() -> impl Iterator<Item = char> {
    ('a'..='z').chain('1'..='9').chain(std::iter::once('0'))
}

fn hotkey_words() -> Vec<String> {
    vec!["a〜z".into(), "0〜9".into()]
}

fn check_hotkey(hotkey: &str) -> Result<(), UiError> {
    let mut chars = hotkey.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() || c.is_ascii_digit() => Ok(()),
        _ => Err(UiError::with_allowed(
            "hotkey は数字 1 字か英小文字 1 字",
            hotkey_words(),
        )),
    }
}

/// shell のコマンドの正規化（FR-2.22.7 = コマンドカードと同じ 1 実装）
fn normalize_shell(raw: &str) -> Result<String, UiError> {
    use crate::command_card::CommandCardError as E;
    crate::command_card::normalize_command_text(raw).map_err(|e| match e {
        E::CommandTooLong { max, .. } => {
            UiError::new(format!("shell のコマンドは {max} バイトまで"))
        }
        E::ControlCharacter { .. } => {
            UiError::new("shell のコマンドに制御文字は使えない（改行とタブだけ可）")
        }
        _ => UiError::new("shell のコマンドは空にできない"),
    })
}

/// prompt の文の正規化（CRLF → LF・前後の空白を落とす・改行とタブ以外の制御文字は不可）
fn normalize_prompt(raw: &str) -> Result<String, UiError> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text.is_empty() {
        return Err(UiError::new("prompt の文は空にできない"));
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        return Err(UiError::new(format!(
            "prompt の文は {MAX_PROMPT_CHARS} 文字まで"
        )));
    }
    if text
        .chars()
        .any(|c| c != '\n' && c != '\t' && c.is_control())
    {
        return Err(UiError::new(
            "prompt の文に制御文字は使えない（改行とタブだけ可）",
        ));
    }
    Ok(text.to_string())
}

/// 表示の桁で切り詰める（はみ出したら末尾を `…`）
fn clip_columns(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if UnicodeWidthStr::width(line) <= max {
        return line.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w + 1 > max {
            break;
        }
        out.push(c);
        width += w;
    }
    let mut out = out.trim_end().to_string();
    out.push('…');
    out
}

// --- 誤り ---------------------------------------------------------------------------

/// 断った理由と、代わりに使える値（**ui.json は書いていない**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    pub what: String,
    pub allowed: Vec<String>,
}

impl UiError {
    fn new(what: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            allowed: Vec::new(),
        }
    }

    fn with_allowed<S: Into<String>>(what: impl Into<String>, allowed: Vec<S>) -> Self {
        Self {
            what: what.into(),
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }

    /// CLI / MCP / setup が同じ文言で返す 1 行
    pub fn message(&self) -> String {
        if self.allowed.is_empty() {
            format!("{}（ui.json は書いていない）", self.what)
        } else {
            format!(
                "{}。使える値: {}（ui.json は書いていない）",
                self.what,
                self.allowed.join(" / ")
            )
        }
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// 値（文字列・真偽・配列のどれか）から語を取り出す。不正なら理由
fn word_of(value: &Value, key: &str) -> Result<String, UiError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(UiError::new(format!("{key} の値は文字列で渡す"))),
    }
}

fn parse_one<T: Vocab>(value: &Value, key: &str) -> Result<T, UiError> {
    let word = word_of(value, key)?;
    T::parse(&word).ok_or_else(|| UiError::with_allowed(format!("{key} に使えない値"), T::words()))
}

fn parse_bool(value: &Value, key: &str) -> Result<bool, UiError> {
    let word = word_of(value, key)?;
    match canonical(&word).as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(UiError::with_allowed(
            format!("{key} は真偽"),
            vec!["true", "false"],
        )),
    }
}

/// 並びのある部分集合。配列か、`,` / 空白で区切った文字列
fn parse_list<T: Vocab>(value: &Value, key: &str) -> Result<Vec<T>, UiError> {
    let words: Vec<String> = match value {
        Value::Array(items) => items
            .iter()
            .map(|v| word_of(v, key))
            .collect::<Result<_, _>>()?,
        other => word_of(other, key)?
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect(),
    };
    let mut out = Vec::new();
    for word in &words {
        let item = T::parse(word).ok_or_else(|| {
            UiError::with_allowed(format!("{key} に使えない値が入っている"), T::words())
        })?;
        if out.contains(&item) {
            return Err(UiError::new(format!(
                "{key} に同じ値が 2 回ある（並べたい順に 1 回ずつ）"
            )));
        }
        out.push(item);
    }
    if out.is_empty() {
        return Err(UiError::with_allowed(
            format!("{key} は 1 つ以上並べる"),
            T::words(),
        ));
    }
    Ok(out)
}

// --- 操作 ---------------------------------------------------------------------------

/// ボタンを足す材料（CLI の引数・MCP の引数・setup の answers が同じ形に落ちる）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonSpec {
    /// slash / tako / shell / prompt。省略時は値から推す（slash の語 → slash、tako の語 → tako）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// slash のコマンド名・tako の操作 id・shell のコマンド・prompt の文
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// 位置の指定（1 から数える / 先頭・末尾 / 1 つ左・右）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTo {
    Index(usize),
    First,
    Last,
    Left,
    Right,
}

impl MoveTo {
    fn parse(value: &Value) -> Result<Self, UiError> {
        let allowed = || vec!["1〜8 の番号", "first", "last", "left", "right"];
        let word = match value {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.trim().to_string(),
            _ => return Err(UiError::with_allowed("移す先の指定が読めない", allowed())),
        };
        match canonical(&word).as_str() {
            "first" | "top" | "start" => Ok(Self::First),
            "last" | "end" | "bottom" => Ok(Self::Last),
            "left" | "prev" | "up" => Ok(Self::Left),
            "right" | "next" | "down" => Ok(Self::Right),
            other => other
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .map(Self::Index)
                .ok_or_else(|| UiError::with_allowed("移す先の指定が読めない", allowed())),
        }
    }
}

/// 1 つの操作。CLI / MCP / setup がすべてここへ落ちる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiOp {
    /// 今の値と選べる値を返す（書かない）
    Show,
    Set {
        key: SetKey,
        value: Value,
    },
    ButtonAdd(ButtonSpec),
    ButtonRemove {
        id: String,
    },
    ButtonMove {
        id: String,
        to: MoveTo,
    },
    /// ボタンを全部外す（setup の answers の `buttons` = 並びの置き換えの前段だけで使う）
    ButtonsClear,
    /// 既定へ戻す（帯の隠す / 出すは保つ）
    Reset,
    Preset(Preset),
}

/// 操作の名前（MCP の `op` の enum・CLI の語）
pub const OPS: &[&str] = &[
    "show",
    "set",
    "button_add",
    "button_remove",
    "button_move",
    "reset",
    "preset",
];

/// CLI / MCP から届く操作の形（protocol の `Request::Mod` の `ui`）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRequest {
    /// [`OPS`] のどれか（省略 = show）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// set のキー（[`SetKey`]）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// set の値・button_add の中身・preset の名前
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// button_add の動作の種類（省略時は値から推す）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// button_add / button_remove / button_move の対象の id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    /// button_move の移す先
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
}

impl UiRequest {
    /// 形を [`UiOp`] へ。足りない引数は「何を足せばよいか」を返す
    pub fn to_op(&self) -> Result<UiOp, UiError> {
        let op = self.op.as_deref().map(canonical).unwrap_or_default();
        let need = |what: &str| UiError::new(format!("{} には {what} が要る", self.op_name()));
        match op.as_str() {
            "" | "show" | "list" | "status" => Ok(UiOp::Show),
            "set" => {
                let key_word = self.key.as_deref().ok_or_else(|| {
                    UiError::with_allowed("set にはキー（key）が要る", SetKey::words())
                })?;
                let key = SetKey::parse(key_word).ok_or_else(|| {
                    UiError::with_allowed("set のキーが語彙に無い", SetKey::words())
                })?;
                let value = self.value.clone().ok_or_else(|| need("値（value）"))?;
                Ok(UiOp::Set { key, value })
            }
            "buttonadd" | "add" => {
                let value = self
                    .value
                    .as_ref()
                    .map(|v| word_of(v, "value"))
                    .transpose()?
                    .ok_or_else(|| need("中身（value）"))?;
                Ok(UiOp::ButtonAdd(ButtonSpec {
                    kind: self.kind.clone(),
                    value,
                    label: self.label.clone(),
                    hotkey: self.hotkey.clone(),
                    id: self.id.clone(),
                }))
            }
            "buttonremove" | "remove" | "rm" => Ok(UiOp::ButtonRemove {
                id: self.id.clone().ok_or_else(|| need("ボタンの id（id）"))?,
            }),
            "buttonmove" | "move" | "mv" => Ok(UiOp::ButtonMove {
                id: self.id.clone().ok_or_else(|| need("ボタンの id（id）"))?,
                to: MoveTo::parse(self.to.as_ref().ok_or_else(|| need("移す先（to）"))?)?,
            }),
            "reset" => Ok(UiOp::Reset),
            "preset" => {
                let word = self
                    .value
                    .as_ref()
                    .map(|v| word_of(v, "value"))
                    .transpose()?
                    .ok_or_else(|| {
                        UiError::with_allowed(
                            "preset には雛形の名前（value）が要る",
                            Preset::words(),
                        )
                    })?;
                Preset::parse(&word)
                    .map(UiOp::Preset)
                    .ok_or_else(|| UiError::with_allowed("雛形の名前が語彙に無い", Preset::words()))
            }
            _ => Err(UiError::with_allowed("op が語彙に無い", OPS.to_vec())),
        }
    }

    fn op_name(&self) -> String {
        self.op.clone().unwrap_or_else(|| "show".into())
    }
}

/// setup の `--answers` の `mod_ui`（宣言的: 雛形 → 個別の値 → ボタンの並びの置き換え、の順に当てる）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiAnswers {
    /// default / recommended / minimal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// キー（[`SetKey`]）→ 値
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, Value>,
    /// ボタンの並び（**置き換え**。省略 = 今のまま）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<ButtonSpec>>,
}

impl UiAnswers {
    pub fn is_empty(&self) -> bool {
        self.preset.is_none() && self.set.is_empty() && self.buttons.is_none()
    }

    /// 操作の列へ（キー・雛形の名前はここで検証する）
    pub fn to_ops(&self) -> Result<Vec<UiOp>, UiError> {
        let mut ops = Vec::new();
        if let Some(word) = &self.preset {
            let preset = Preset::parse(word).ok_or_else(|| {
                UiError::with_allowed("mod_ui.preset が語彙に無い", Preset::words())
            })?;
            ops.push(UiOp::Preset(preset));
        }
        for (key, value) in &self.set {
            let key = SetKey::parse(key).ok_or_else(|| {
                UiError::with_allowed("mod_ui.set のキーが語彙に無い", SetKey::words())
            })?;
            ops.push(UiOp::Set {
                key,
                value: value.clone(),
            });
        }
        if let Some(buttons) = &self.buttons {
            ops.push(UiOp::ButtonsClear);
            ops.extend(buttons.iter().cloned().map(UiOp::ButtonAdd));
        }
        Ok(ops)
    }
}

/// setup の answers を、書く前に検証する（既定の値へ当ててみる。answers は今の値に依らない形 =
/// 雛形・個別の値・ボタンの並びの置き換え、なので既定で通れば今の値でも通る）
pub fn check_answers(answers: &UiAnswers) -> Result<(), UiError> {
    let ops = answers.to_ops()?;
    apply_all(&mut UiConfig::default(), &ops, 0).map(|_| ())
}

/// 操作の列を当てる。**1 つでも断られたら何も変えない**（途中まで当たった形を作らない）。
/// 戻り値は変わったか。`now_ms` は帯を切り替えたときの `toggled_at`
pub fn apply_all(cfg: &mut UiConfig, ops: &[UiOp], now_ms: u64) -> Result<bool, UiError> {
    let mut next = cfg.clone();
    for op in ops {
        apply_one(&mut next, op, now_ms)?;
    }
    // 最後の砦: どの操作を通っても形を満たす（崩れていたら書かない）
    let left = problems(&next);
    if !left.is_empty() {
        return Err(UiError::new(format!(
            "操作の結果が形を満たさない（{}）",
            left.join("; ")
        )));
    }
    let changed = next != *cfg;
    *cfg = next;
    Ok(changed)
}

fn apply_one(cfg: &mut UiConfig, op: &UiOp, now_ms: u64) -> Result<(), UiError> {
    match op {
        UiOp::Show => Ok(()),
        UiOp::Set { key, value } => set(cfg, *key, value, now_ms),
        UiOp::ButtonAdd(spec) => add_button(cfg, spec),
        UiOp::ButtonRemove { id } => {
            let at = find_button(cfg, id)?;
            cfg.buttons.remove(at);
            Ok(())
        }
        UiOp::ButtonMove { id, to } => move_button(cfg, id, *to),
        UiOp::ButtonsClear => {
            cfg.buttons.clear();
            Ok(())
        }
        UiOp::Reset => {
            replace_keeping_band_toggle(cfg, UiConfig::default(), now_ms);
            Ok(())
        }
        UiOp::Preset(preset) => {
            replace_keeping_band_toggle(cfg, preset.config(), now_ms);
            Ok(())
        }
    }
}

/// 雛形へ置き換える。帯の隠す / 出すは今の値を保つ（雛形で帯が出てくる・消えると驚く）
fn replace_keeping_band_toggle(cfg: &mut UiConfig, mut next: UiConfig, _now_ms: u64) {
    next.band.hidden = cfg.band.hidden;
    next.band.toggled_at = cfg.band.toggled_at;
    *cfg = next;
}

fn set(cfg: &mut UiConfig, key: SetKey, value: &Value, now_ms: u64) -> Result<(), UiError> {
    let k = key.as_str();
    match key {
        SetKey::BandHidden => {
            cfg.band.hidden = parse_bool(value, k)?;
            // 明示の切り替えは常に打ち直す（値が同じでも、mod の `$.store` がずれていれば揃う）
            cfg.band.toggled_at = Some(now_ms);
        }
        SetKey::BandSegments => cfg.band.segments = parse_list(value, k)?,
        SetKey::UsageBarPlace => cfg.usage_bar.place = parse_one(value, k)?,
        SetKey::UsageBarItems => cfg.usage_bar.items = parse_list(value, k)?,
        SetKey::CardsPlace => cfg.cards.place = parse_one(value, k)?,
        SetKey::ChatPlace => cfg.chat.place = parse_one(value, k)?,
        SetKey::ChatBubble => cfg.chat.bubble = parse_one(value, k)?,
        SetKey::ChatCodeCopy => cfg.chat.code_copy = parse_bool(value, k)?,
        SetKey::ChatToolSummary => cfg.chat.tool_summary = parse_one(value, k)?,
        SetKey::ColorsAccent => cfg.colors.accent = parse_one(value, k)?,
        SetKey::ColorsWarn => cfg.colors.warn = parse_one(value, k)?,
        SetKey::ColorsDim => cfg.colors.dim = parse_one(value, k)?,
    }
    Ok(())
}

fn find_button(cfg: &UiConfig, id: &str) -> Result<usize, UiError> {
    let want = id.trim().to_ascii_lowercase();
    cfg.buttons
        .iter()
        .position(|b| b.id == want)
        .ok_or_else(|| {
            UiError::with_allowed(
                "その id のボタンは無い",
                cfg.buttons.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
            )
        })
}

fn move_button(cfg: &mut UiConfig, id: &str, to: MoveTo) -> Result<(), UiError> {
    let from = find_button(cfg, id)?;
    let last = cfg.buttons.len() - 1;
    let dest = match to {
        MoveTo::First => 0,
        MoveTo::Last => last,
        MoveTo::Left => from.saturating_sub(1),
        MoveTo::Right => (from + 1).min(last),
        MoveTo::Index(n) if n <= cfg.buttons.len() => n - 1,
        MoveTo::Index(_) => {
            return Err(UiError::with_allowed(
                format!("移す先は 1〜{}", cfg.buttons.len()),
                vec!["first", "last", "left", "right"],
            ))
        }
    };
    let button = cfg.buttons.remove(from);
    cfg.buttons.insert(dest, button);
    Ok(())
}

fn add_button(cfg: &mut UiConfig, spec: &ButtonSpec) -> Result<(), UiError> {
    let button = build_button(spec, &cfg.buttons)?;
    // 同じ動作のボタンが既にあれば足さない（冪等。ラベル等を変えたいなら消してから足す）
    if let Some(same) = cfg.buttons.iter().find(|b| b.action == button.action) {
        let explicit_differs = spec
            .label
            .as_deref()
            .is_some_and(|l| l.trim() != same.label)
            || spec
                .hotkey
                .as_deref()
                .is_some_and(|h| h.trim().to_ascii_lowercase() != same.hotkey)
            || spec.id.as_deref().is_some_and(|i| i.trim() != same.id);
        if explicit_differs {
            return Err(UiError::new(format!(
                "同じ動作のボタン（id {}）が既にある。ラベル等を変えるなら button_remove してから足す",
                same.id
            )));
        }
        return Ok(());
    }
    if cfg.buttons.len() >= MAX_BUTTONS {
        return Err(UiError::with_allowed(
            format!("ボタンは {MAX_BUTTONS} 個まで（どれかを button_remove で外す）"),
            cfg.buttons.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
        ));
    }
    cfg.buttons.push(button);
    Ok(())
}

/// 材料からボタンを組む（id・ラベル・hotkey を省いたら推す）。`existing` と重ならないように選ぶ
pub fn build_button(spec: &ButtonSpec, existing: &[Button]) -> Result<Button, UiError> {
    let kind = match spec.kind.as_deref() {
        Some(word) => ButtonKind::parse(word).ok_or_else(|| {
            UiError::with_allowed("ボタンの kind が語彙に無い", ButtonKind::words())
        })?,
        None if SlashCommand::parse(&spec.value).is_some() => ButtonKind::Slash,
        None if TakoOp::parse(&spec.value).is_some() => ButtonKind::Tako,
        None => {
            return Err(UiError::with_allowed(
                "ボタンの kind が要る（値が slash / tako の語でないので推せない）",
                ButtonKind::words(),
            ))
        }
    };
    let action = match kind {
        ButtonKind::Slash => ButtonAction::Slash {
            command: SlashCommand::parse(&spec.value).ok_or_else(|| {
                UiError::with_allowed(
                    "slash で送れるのは許可リストのコマンドだけ",
                    SlashCommand::words(),
                )
            })?,
        },
        ButtonKind::Tako => ButtonAction::Tako {
            op: TakoOp::parse(&spec.value).ok_or_else(|| {
                UiError::with_allowed("tako の操作 id が語彙に無い", TakoOp::words())
            })?,
        },
        ButtonKind::Shell => ButtonAction::Shell {
            command: normalize_shell(&spec.value)?,
        },
        ButtonKind::Prompt => ButtonAction::Prompt {
            text: normalize_prompt(&spec.value)?,
        },
    };
    let label = match spec.label.as_deref() {
        Some(raw) => {
            let label = raw.trim().to_string();
            check_label(&label)?;
            label
        }
        None => default_label(&action),
    };
    let id = match spec.id.as_deref() {
        Some(raw) => {
            let id = raw.trim().to_ascii_lowercase();
            check_id(&id)?;
            if existing.iter().any(|b| b.id == id) {
                return Err(UiError::with_allowed(
                    "その id は別のボタンが使っている",
                    existing.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
                ));
            }
            id
        }
        None => free_id(&base_id(&action, &label), existing),
    };
    let hotkey = match spec.hotkey.as_deref() {
        Some(raw) => {
            let hotkey = raw.trim().to_ascii_lowercase();
            check_hotkey(&hotkey)?;
            if existing.iter().any(|b| b.hotkey == hotkey) {
                return Err(UiError::with_allowed(
                    "その hotkey は別のボタンが使っている",
                    free_hotkeys(existing),
                ));
            }
            hotkey
        }
        None => auto_hotkey(&label, existing),
    };
    Ok(Button {
        id,
        label,
        hotkey,
        action,
    })
}

fn default_label(action: &ButtonAction) -> String {
    match action {
        ButtonAction::Slash { command } => command.as_str().to_string(),
        ButtonAction::Tako { op } => op.label().to_string(),
        ButtonAction::Shell { command } => clip_columns(command, MAX_LABEL_COLUMNS),
        ButtonAction::Prompt { text } => clip_columns(text, MAX_LABEL_COLUMNS),
    }
}

/// id の元（slash / tako は語そのもの、shell / prompt はラベルの英数字から）
fn base_id(action: &ButtonAction, label: &str) -> String {
    let from_label = || {
        let mut slug = String::new();
        for c in label.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') && !slug.is_empty() {
                slug.push('-');
            }
        }
        let slug: String = slug
            .trim_end_matches('-')
            .chars()
            .take(MAX_ID_CHARS - 3)
            .collect();
        slug.trim_end_matches('-').to_string()
    };
    let base = match action {
        ButtonAction::Slash { command } => command.as_str().to_string(),
        ButtonAction::Tako { op } => op.as_str().to_string(),
        ButtonAction::Shell { .. } | ButtonAction::Prompt { .. } => from_label(),
    };
    if base.is_empty() {
        action.kind().as_str().to_string()
    } else {
        base
    }
}

/// 空いている id（重なれば `-2` `-3` … を付け、付けるぶんだけ元を詰める = 上限の桁に収める）
fn free_id(base: &str, existing: &[Button]) -> String {
    let taken = |id: &str| existing.iter().any(|b| b.id == id);
    let full: String = base.chars().take(MAX_ID_CHARS).collect();
    if !taken(&full) {
        return full;
    }
    (2..)
        .map(|n| {
            let suffix = format!("-{n}");
            let head: String = base.chars().take(MAX_ID_CHARS - suffix.len()).collect();
            format!("{}{suffix}", head.trim_end_matches('-'))
        })
        .find(|id| !taken(id))
        .expect("ボタンは有限個なので空きはある")
}

fn free_hotkeys(existing: &[Button]) -> Vec<String> {
    hotkey_alphabet()
        .map(String::from)
        .filter(|k| !existing.iter().any(|b| b.hotkey == *k))
        .collect()
}

/// ラベルの文字から順に空いている英数字を選ぶ（無ければ a〜z → 1〜9 → 0 の空き）
fn auto_hotkey(label: &str, existing: &[Button]) -> String {
    let taken = |c: char| {
        existing
            .iter()
            .any(|b| b.hotkey.chars().eq(std::iter::once(c)))
    };
    label
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_ascii_lowercase())
        .chain(hotkey_alphabet())
        .find(|c| !taken(*c))
        .map(String::from)
        .unwrap_or_else(|| "a".into())
}

/// mod の報告の帯の状態を取り込む（[`BandUi::toggled_at`] より新しいときだけ）。変わったら true
pub fn import_band(cfg: &mut UiConfig, hidden: bool, toggled_at: u64) -> bool {
    if cfg.band.toggled_at.is_some_and(|at| at >= toggled_at) {
        return false;
    }
    let changed = cfg.band.hidden != hidden || cfg.band.toggled_at != Some(toggled_at);
    cfg.band.hidden = hidden;
    cfg.band.toggled_at = Some(toggled_at);
    changed
}

// --- 選べる値（`tako mod ui` の一覧） -------------------------------------------------

/// 選べる値の一覧（`tako mod ui` / MCP の応答の `choices`。setup の対話もこれを読む）
pub fn choices() -> Value {
    let mut set = Map::new();
    for key in SetKey::ALL {
        let (shape, values) = match key.shape() {
            Shape::Bool => ("bool", vec!["true", "false"]),
            Shape::One(v) => ("one", v),
            Shape::List(v) => ("list", v),
        };
        set.insert(
            key.as_str().into(),
            json!({ "type": shape, "values": values, "about": key.describe() }),
        );
    }
    json!({
        "ops": OPS,
        "set": set,
        "button": {
            "max": MAX_BUTTONS,
            "label_max_columns": MAX_LABEL_COLUMNS,
            "hotkey": "数字 1 字か英小文字 1 字（重複不可。省くと空いている英字を選ぶ）",
            "kinds": {
                "slash": {
                    "about": ButtonKind::Slash.describe(),
                    "values": SlashCommand::ALL.iter().map(|c| json!({"value": c.as_str(), "about": c.describe()})).collect::<Vec<_>>(),
                },
                "tako": {
                    "about": ButtonKind::Tako.describe(),
                    "values": TakoOp::ALL.iter().map(|o| json!({"value": o.as_str(), "about": o.describe()})).collect::<Vec<_>>(),
                },
                "shell": { "about": ButtonKind::Shell.describe() },
                "prompt": { "about": ButtonKind::Prompt.describe() },
            },
        },
        "presets": Preset::ALL.iter().map(|p| json!({"value": p.as_str(), "about": p.describe()})).collect::<Vec<_>>(),
        "examples": [
            "tako mod ui button add compact",
            "tako mod ui button add split-right",
            "tako mod ui button add shell \"npm test\" --label test",
            "tako mod ui button move compact first",
            "tako mod ui button remove compact",
            "tako mod ui set usage_bar.place off",
            "tako mod ui set band.segments pane,attention,buttons",
            "tako mod ui preset recommended",
            "tako mod ui reset",
        ],
    })
}

/// 今の値を `set` のキーごとに（表示用）
pub fn current_values(cfg: &UiConfig) -> Value {
    let mut map = Map::new();
    for key in SetKey::ALL {
        map.insert(key.as_str().into(), key.current(cfg));
    }
    Value::Object(map)
}

// --- 読み込み（寛容）----------------------------------------------------------------

/// 知っている最上位のキー
const TOP_KEYS: &[&str] = &[
    "schema_version",
    "band",
    "usage_bar",
    "buttons",
    "cards",
    "chat",
    "colors",
];

/// [`parse_lenient`] の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// 必ず形を満たす（壊れた部分は既定）
    pub config: UiConfig,
    /// 既定へ落とした部分（空 = そのまま読めた）。**利用者が書いた値は入れない**
    pub problems: Vec<String>,
    /// 書かれていた `schema_version`（無ければ None）
    pub version: Option<u64>,
}

impl Parsed {
    /// 新しい tako が書いた形か（読める範囲で使い、書き換えない）
    pub fn is_future(&self) -> bool {
        self.version.is_some_and(|v| v > u64::from(SCHEMA_VERSION))
    }
}

/// 中身の版（移行の `detect`）。読めなければ 1
pub fn detect_version(text: &str) -> u32 {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("schema_version").and_then(Value::as_u64))
        .map_or(1, |v| v.min(u64::from(u32::MAX)) as u32)
}

/// 今の形として読めるか（移行の `validate`）。新しい形は通す（移行の計画が「目標超過」で断る）
pub fn validate_text(text: &str) -> Result<(), String> {
    let parsed = parse_lenient(text);
    if parsed.is_future() || parsed.problems.is_empty() {
        Ok(())
    } else {
        Err(parsed.problems.join("; "))
    }
}

/// 部分ごとに読む。壊れた部分は既定へ落として `problems` に積む（読める部分は捨てない）
pub fn parse_lenient(text: &str) -> Parsed {
    let fallback = |problem: String| Parsed {
        config: UiConfig::default(),
        problems: vec![problem],
        version: None,
    };
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return fallback(format!(
                "JSON として読めない（{} 行 {} 桁）",
                e.line(),
                e.column()
            ))
        }
    };
    let Value::Object(map) = value else {
        return fallback("最上位が JSON のオブジェクトではない".into());
    };
    let mut problems = Vec::new();
    let version = map.get("schema_version").and_then(Value::as_u64);
    match version {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) if v > u64::from(SCHEMA_VERSION) => {}
        Some(_) | None => problems.push(format!("schema_version は {SCHEMA_VERSION}")),
    }
    for key in map.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            let shown: String = key.chars().filter(|c| !c.is_control()).take(32).collect();
            problems.push(format!("知らないキー `{shown}`"));
        }
    }
    let defaults = UiConfig::default();
    let mut config = UiConfig {
        schema_version: SCHEMA_VERSION,
        band: section(&map, "band", defaults.band, &mut problems),
        usage_bar: section(&map, "usage_bar", defaults.usage_bar, &mut problems),
        buttons: lenient_buttons(map.get("buttons"), &mut problems),
        cards: section(&map, "cards", defaults.cards, &mut problems),
        chat: section(&map, "chat", defaults.chat, &mut problems),
        colors: section(&map, "colors", defaults.colors, &mut problems),
    };
    let band_problems = segments_problems(&config.band.segments);
    if !band_problems.is_empty() {
        problems.extend(band_problems);
        config.band.segments = BandUi::default().segments;
    }
    let item_problems = items_problems(&config.usage_bar.items);
    if !item_problems.is_empty() {
        problems.extend(item_problems);
        config.usage_bar.items = UsageBarUi::default().items;
    }
    Parsed {
        config,
        problems,
        version,
    }
}

/// 1 つの部分を型どおりに読む。読めなければ既定
fn section<T: serde::de::DeserializeOwned + Serialize>(
    map: &Map<String, Value>,
    key: &str,
    default: T,
    problems: &mut Vec<String>,
) -> T {
    match map.get(key) {
        None => {
            problems.push(format!("{key} が無い"));
            default
        }
        // 型で読めて、書き戻すと同じ形になる = スキーマどおり（余計なキー・null の混入も弾く）
        Some(v) => match serde_json::from_value::<T>(v.clone()) {
            Ok(parsed) if serde_json::to_value(&parsed).ok().as_ref() == Some(v) => parsed,
            _ => {
                problems.push(format!("{key} が語彙・形に合わない"));
                default
            }
        },
    }
}

/// ボタンは 1 個ずつ読む（壊れたものだけ落とす。重複・上限を超えたものも落とす）
fn lenient_buttons(value: Option<&Value>, problems: &mut Vec<String>) -> Vec<Button> {
    let items = match value {
        None => {
            problems.push("buttons が無い".into());
            return default_buttons();
        }
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.push("buttons が配列ではない".into());
            return default_buttons();
        }
    };
    let mut out: Vec<Button> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let at = format!("buttons[{i}]");
        let button = serde_json::from_value::<Button>(item.clone())
            .ok()
            .filter(|b| serde_json::to_value(b).ok().as_ref() == Some(item));
        let Some(button) = button else {
            problems.push(format!("{at} が語彙・形に合わない（外した）"));
            continue;
        };
        let mine = button_problems(&button, &at);
        if !mine.is_empty() {
            problems.extend(mine.into_iter().map(|p| format!("{p}（外した）")));
            continue;
        }
        if out
            .iter()
            .any(|b| b.id == button.id || b.hotkey == button.hotkey)
        {
            problems.push(format!("{at} の id か hotkey が前のボタンと重複（外した）"));
            continue;
        }
        if out.len() >= MAX_BUTTONS {
            problems.push(format!("{at} は {MAX_BUTTONS} 個を超えた（外した）"));
            continue;
        }
        out.push(button);
    }
    out
}

// --- 読み書き -------------------------------------------------------------------------

/// [`load_from`] の結果の様子
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    /// ファイルが無い（既定で動く。書くのは最初に変えたとき）
    Absent,
    /// そのまま読めた
    Valid,
    /// 一部（または全部）を既定へ落とした。元は `quarantine` へ保全した
    Repaired {
        problems: Vec<String>,
        quarantine: Option<crate::migration::Quarantine>,
    },
    /// 新しい tako が書いた形（読める範囲で使い、書き換えない）
    Future { version: u64 },
    /// 読めなかった（権限・UTF-8 でない等）。既定で動く
    ReadError { reason: String },
}

impl LoadState {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Valid => "valid",
            Self::Repaired { .. } => "repaired",
            Self::Future { .. } => "future",
            Self::ReadError { .. } => "read_error",
        }
    }
}

/// 読んだ値と様子
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub config: UiConfig,
    pub state: LoadState,
}

/// 読む。壊れていれば読める部分を使い、**元の中身を `.unreadable.bak` へ保全する**（#916。
/// 同じ中身なら写しを増やさない）。ファイルは書き換えない（書くのは変更の操作だけ）
pub fn load_from(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                config: UiConfig::default(),
                state: LoadState::Absent,
            }
        }
        Err(e) => {
            return Loaded {
                config: UiConfig::default(),
                state: LoadState::ReadError {
                    reason: e.kind().to_string(),
                },
            }
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        // UTF-8 でない中身は文字列の退避口を通らないので、そのままの写しを 1 本目の置き場へ
        let slot = crate::migration::quarantine_slot_path(path, 1);
        if !slot.exists() {
            let _ = std::fs::copy(path, &slot);
        }
        return Loaded {
            config: UiConfig::default(),
            state: LoadState::Repaired {
                problems: vec!["UTF-8 として読めない".into()],
                quarantine: slot.exists().then_some(crate::migration::Quarantine {
                    path: slot,
                    copied: true,
                    evicted: false,
                }),
            },
        };
    };
    let parsed = parse_lenient(&text);
    if parsed.is_future() {
        return Loaded {
            config: parsed.config,
            state: LoadState::Future {
                version: parsed.version.unwrap_or_default(),
            },
        };
    }
    if parsed.problems.is_empty() {
        return Loaded {
            config: parsed.config,
            state: LoadState::Valid,
        };
    }
    let quarantine = crate::migration::quarantine_unreadable(path, &crate::migration::FsIo);
    Loaded {
        config: parsed.config,
        state: LoadState::Repaired {
            problems: parsed.problems,
            quarantine,
        },
    }
}

/// 書く（同じ dir の一時ファイルへ書いてから差し替える = 途中の中身が他のプロセスから見えない）
pub fn save_to(path: &Path, cfg: &UiConfig) -> std::io::Result<()> {
    let left = problems(cfg);
    if !left.is_empty() {
        return Err(std::io::Error::other(format!(
            "形を満たさない値は書かない（{}）",
            left.join("; ")
        )));
    }
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("ui.json の置き場が決まらない"))?;
    std::fs::create_dir_all(dir)?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{FILENAME}.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::write(&tmp, cfg.to_text()).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// [`update_at`] の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    pub config: UiConfig,
    /// 読んだときの様子（壊れていたなら退避先がここに）
    pub before: LoadState,
    /// 値が変わったか
    pub changed: bool,
    /// ファイルへ書いたか
    pub written: bool,
}

/// [`update_at`] が断った理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// 値が語彙・形に合わない（書いていない）
    Invalid(UiError),
    /// 新しい tako が書いた形なので書き換えない
    Future { version: u64 },
    /// 書けなかった
    Io(String),
}

impl UpdateError {
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(e) => e.message(),
            Self::Future { version } => format!(
                "ui.json は新しい tako が書いた形（schema_version {version}）なので、この tako では書き換えない（tako を更新する）"
            ),
            Self::Io(e) => format!("ui.json を書けない: {e}"),
        }
    }
}

/// 読む → 操作を当てる → 変わったら書く。**呼び出し側が排他を取る**（tako-control は
/// `config_io::lock_exclusive`）。操作が 1 つでも断られたら書かない
pub fn update_at(path: &Path, ops: &[UiOp], now_ms: u64) -> Result<Updated, UpdateError> {
    let loaded = load_from(path);
    let mutating = ops.iter().any(|op| !matches!(op, UiOp::Show));
    if let LoadState::Future { version } = loaded.state {
        if mutating {
            return Err(UpdateError::Future { version });
        }
    }
    let mut config = loaded.config;
    let changed = apply_all(&mut config, ops, now_ms).map_err(UpdateError::Invalid)?;
    // 壊れていた中身は退避済みなので、変更の操作なら直した形で書き直す
    let repaired = matches!(loaded.state, LoadState::Repaired { .. }) && mutating;
    let written = changed || repaired;
    if written {
        save_to(path, &config).map_err(|e| UpdateError::Io(e.to_string()))?;
    }
    Ok(Updated {
        config,
        before: loaded.state,
        changed,
        written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-mod-ui-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn req(op: &str) -> UiRequest {
        UiRequest {
            op: Some(op.into()),
            ..UiRequest::default()
        }
    }

    fn run(cfg: &mut UiConfig, r: UiRequest) -> Result<bool, UiError> {
        let op = r.to_op()?;
        apply_all(cfg, &[op], 1_000)
    }

    #[test]
    fn 既定はcompactのボタン1個で形を満たす() {
        let cfg = UiConfig::default();
        assert!(problems(&cfg).is_empty(), "{:?}", problems(&cfg));
        assert_eq!(cfg.buttons.len(), 1);
        assert_eq!(cfg.buttons[0].id, "compact");
        assert_eq!(cfg.buttons[0].hotkey, "c");
        assert_eq!(
            cfg.buttons[0].action,
            ButtonAction::Slash {
                command: SlashCommand::Compact
            }
        );
        // 書いた本文はそのまま読み戻せる
        let parsed = parse_lenient(&cfg.to_text());
        assert_eq!(parsed.problems, Vec::<String>::new());
        assert_eq!(parsed.config, cfg);
        assert!(
            cfg.to_text().contains("\"kind\": \"slash\""),
            "{}",
            cfg.to_text()
        );
    }

    /// 語彙の `as_str` は serde の名前と同じ（ui.json の語と一覧の語がずれない）
    #[test]
    fn 語彙の語はserdeの名前と一致する() {
        fn check<T: Vocab + Serialize>() {
            for v in T::ALL {
                assert_eq!(
                    serde_json::to_value(v).unwrap(),
                    json!(v.as_str()),
                    "{}",
                    v.as_str()
                );
                assert_eq!(T::parse(v.as_str()), Some(*v));
            }
            // 正規形で 2 つの語が重ならない
            for (i, a) in T::ALL.iter().enumerate() {
                for b in &T::ALL[..i] {
                    assert_ne!(canonical(a.as_str()), canonical(b.as_str()));
                }
            }
        }
        check::<BandSegment>();
        check::<UsageBarPlace>();
        check::<UsageItem>();
        check::<Place>();
        check::<ChatShow>();
        check::<ThemeColor>();
        check::<SlashCommand>();
        check::<TakoOp>();
    }

    #[test]
    fn 軽いモデルの書き方の揺れを同じ語へ落とす() {
        assert_eq!(SlashCommand::parse("/compact"), Some(SlashCommand::Compact));
        assert_eq!(
            SlashCommand::parse(" Compact "),
            Some(SlashCommand::Compact)
        );
        assert_eq!(TakoOp::parse("split_right"), Some(TakoOp::SplitRight));
        assert_eq!(TakoOp::parse("SplitRight"), Some(TakoOp::SplitRight));
        assert_eq!(ThemeColor::parse("plan_mode"), Some(ThemeColor::PlanMode));
        assert_eq!(UsageItem::parse("5h"), Some(UsageItem::FiveHour));
        assert_eq!(
            SetKey::parse("usage-bar.place"),
            Some(SetKey::UsageBarPlace)
        );
        assert_eq!(SlashCommand::parse("bash"), None);
        assert_eq!(SlashCommand::parse(""), None);
    }

    #[test]
    fn 不正な値は書かずに許される値を返す() {
        let mut cfg = UiConfig::default();
        let before = cfg.clone();
        // 未知の action（kind）
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                kind: Some("javascript".into()),
                value: Some(json!("alert(1)")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.allowed, ButtonKind::words());
        // 許可リスト外の slash
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                kind: Some("slash".into()),
                value: Some(json!("bash")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.allowed, SlashCommand::words());
        assert!(err.message().contains("ui.json は書いていない"), "{err}");
        // テーマに無い色
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("set".into()),
                key: Some("colors.accent".into()),
                value: Some(json!("#ff0000")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.allowed, ThemeColor::words());
        // 未知のキー
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("set".into()),
                key: Some("band.color".into()),
                value: Some(json!("x")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.allowed, SetKey::words());
        // label 17 桁・全角 9 字（18 桁）
        for label in ["abcdefghijklmnopq", "あいうえおかきくけ"] {
            let err = run(
                &mut cfg,
                UiRequest {
                    op: Some("button_add".into()),
                    value: Some(json!("context")),
                    label: Some(label.into()),
                    ..UiRequest::default()
                },
            )
            .unwrap_err();
            assert!(err.what.contains("16 桁"), "{err}");
        }
        // hotkey の重複（compact が c を持つ）
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("context")),
                hotkey: Some("c".into()),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(!err.allowed.contains(&"c".to_string()), "{err}");
        assert!(err.allowed.contains(&"a".to_string()), "{err}");
        // hotkey の形
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("context")),
                hotkey: Some("ctrl+x".into()),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("hotkey"), "{err}");
        // shell の制御文字（FR-2.22.7）
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                kind: Some("shell".into()),
                value: Some(json!("echo \u{1b}[31m")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("制御文字"), "{err}");
        assert_eq!(cfg, before, "断った操作は何も変えない");
    }

    #[test]
    fn ボタンは8個までで9個目は断る() {
        let mut cfg = UiConfig::default();
        let values = [
            "clear",
            "context",
            "cost",
            "model",
            "effort",
            "tako",
            "split-right",
        ];
        for v in values {
            run(
                &mut cfg,
                UiRequest {
                    op: Some("button_add".into()),
                    value: Some(json!(v)),
                    ..UiRequest::default()
                },
            )
            .unwrap();
        }
        assert_eq!(cfg.buttons.len(), 8);
        assert!(problems(&cfg).is_empty(), "{:?}", problems(&cfg));
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("split-down")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("8 個まで"), "{err}");
        assert_eq!(err.allowed.len(), 8, "今の id を並べる");
        // hotkey は英字が先（数字は空の入力欄で帯のボタンを押してしまう）
        assert!(cfg
            .buttons
            .iter()
            .all(|b| b.hotkey.chars().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn 同じ動作のボタンは冪等で明示が違えば断る() {
        let mut cfg = UiConfig::default();
        let changed = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("/compact")),
                ..UiRequest::default()
            },
        )
        .unwrap();
        assert!(!changed);
        assert_eq!(cfg.buttons.len(), 1);
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("compact")),
                label: Some("圧縮".into()),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("button_remove"), "{err}");
    }

    #[test]
    fn 並べ替えと削除と雛形() {
        let mut cfg = Preset::Recommended.config();
        assert!(problems(&cfg).is_empty());
        let ids: Vec<_> = cfg.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "compact",
                "context",
                "split-right",
                "session-restart-handoff"
            ]
        );
        let keys: Vec<_> = cfg.buttons.iter().map(|b| b.hotkey.as_str()).collect();
        assert_eq!(keys, ["c", "o", "s", "h"]);
        run(
            &mut cfg,
            UiRequest {
                op: Some("button_move".into()),
                id: Some("split-right".into()),
                to: Some(json!("first")),
                ..UiRequest::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.buttons[0].id, "split-right");
        run(
            &mut cfg,
            UiRequest {
                op: Some("button_move".into()),
                id: Some("split-right".into()),
                to: Some(json!(3)),
                ..UiRequest::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.buttons[2].id, "split-right");
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_move".into()),
                id: Some("split-right".into()),
                to: Some(json!(9)),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("1〜4"), "{err}");
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("button_remove".into()),
                id: Some("nope".into()),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.allowed.len(), 4);
        // 雛形は帯の隠す / 出すを保つ
        run(
            &mut cfg,
            UiRequest {
                op: Some("set".into()),
                key: Some("band.hidden".into()),
                value: Some(json!("on")),
                ..UiRequest::default()
            },
        )
        .unwrap();
        run(&mut cfg, req("reset")).unwrap();
        assert!(cfg.band.hidden);
        assert_eq!(cfg.band.toggled_at, Some(1_000));
        let mut want = UiConfig::default();
        want.band.hidden = true;
        want.band.toggled_at = Some(1_000);
        assert_eq!(cfg, want);
        let minimal = Preset::Minimal.config();
        assert!(problems(&minimal).is_empty());
        assert_eq!(minimal.buttons, default_buttons());
    }

    #[test]
    fn 値の書き方はリストも真偽も受ける() {
        let mut cfg = UiConfig::default();
        for (key, value) in [
            ("band.segments", json!("pane, attention buttons")),
            ("usage_bar.items", json!(["ctx", "5h"])),
            ("chat.code_copy", json!(false)),
            ("band.hidden", json!("yes")),
        ] {
            run(
                &mut cfg,
                UiRequest {
                    op: Some("set".into()),
                    key: Some(key.into()),
                    value: Some(value),
                    ..UiRequest::default()
                },
            )
            .unwrap();
        }
        assert_eq!(
            cfg.band.segments,
            [
                BandSegment::Pane,
                BandSegment::Attention,
                BandSegment::Buttons
            ]
        );
        assert_eq!(cfg.usage_bar.items, [UsageItem::Ctx, UsageItem::FiveHour]);
        assert!(!cfg.chat.code_copy);
        assert!(cfg.band.hidden);
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("set".into()),
                key: Some("band.segments".into()),
                value: Some(json!("pane,pane")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("2 回"), "{err}");
        let err = run(
            &mut cfg,
            UiRequest {
                op: Some("set".into()),
                key: Some("usage_bar.items".into()),
                value: Some(json!("")),
                ..UiRequest::default()
            },
        )
        .unwrap_err();
        assert!(err.what.contains("1 つ以上"), "{err}");
    }

    #[test]
    fn 一部だけ壊れたファイルは読める部分を残して既定へ落とす() {
        let mut cfg = UiConfig::default();
        cfg.usage_bar.place = UsageBarPlace::Off;
        cfg.colors.accent = ThemeColor::Claude;
        let mut value = serde_json::to_value(&cfg).unwrap();
        value["chat"]["bubble"] = json!("sometimes");
        value["buttons"] = json!([
            {"id": "compact", "label": "compact", "hotkey": "c", "action": {"kind": "slash", "command": "compact"}},
            {"id": "evil", "label": "evil", "hotkey": "e", "action": {"kind": "js", "code": "x"}},
            {"id": "dup", "label": "dup", "hotkey": "c", "action": {"kind": "slash", "command": "clear"}},
            {"id": "long", "label": "abcdefghijklmnopq", "hotkey": "l", "action": {"kind": "slash", "command": "cost"}},
        ]);
        value["buttons"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "x", "label": "x", "hotkey": "x", "action": {"kind": "slash", "command": "cost", "args": "--all"}}));
        value["extra"] = json!(1);
        let parsed = parse_lenient(&value.to_string());
        assert_eq!(
            parsed.config.usage_bar.place,
            UsageBarPlace::Off,
            "読める部分は残す"
        );
        assert_eq!(parsed.config.colors.accent, ThemeColor::Claude);
        assert_eq!(parsed.config.chat, ChatUi::default(), "壊れた部分だけ既定");
        assert_eq!(parsed.config.buttons, default_buttons());
        assert!(problems(&parsed.config).is_empty());
        assert_eq!(parsed.problems.len(), 6, "{:?}", parsed.problems);
        // 問題の文言に利用者の値（ここでは "sometimes" / "x"）を入れない
        for p in &parsed.problems {
            assert!(
                !p.contains("sometimes") && !p.contains("abcdefghijklmnopq"),
                "{p}"
            );
        }
        assert!(validate_text(&value.to_string()).is_err());
        assert!(validate_text(&UiConfig::default().to_text()).is_ok());
    }

    #[test]
    fn 新しい形は読める範囲で使い書き換えない() {
        let dir = tmp_dir("future");
        let path = dir.join(FILENAME);
        let mut value = serde_json::to_value(UiConfig::default()).unwrap();
        value["schema_version"] = json!(2);
        value["band"]["new_field"] = json!(true);
        std::fs::write(&path, value.to_string()).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.state, LoadState::Future { version: 2 });
        assert!(
            validate_text(&value.to_string()).is_ok(),
            "移行の計画が断る"
        );
        assert_eq!(detect_version(&value.to_string()), 2);
        let err = update_at(
            &path,
            &[UiOp::Set {
                key: SetKey::UsageBarPlace,
                value: json!("off"),
            }],
            1,
        )
        .unwrap_err();
        assert!(matches!(err, UpdateError::Future { version: 2 }));
        assert!(err.message().contains("書き換えない"), "{}", err.message());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            value.to_string(),
            "1 バイトも変えない"
        );
        assert!(!crate::migration::quarantine_slot_path(&path, 1).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 壊れたファイルは既定で動き退避して変更で直す() {
        let dir = tmp_dir("broken");
        let path = dir.join(FILENAME);
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.config, UiConfig::default());
        let LoadState::Repaired { quarantine, .. } = &loaded.state else {
            panic!("{:?}", loaded.state)
        };
        let bak = quarantine.as_ref().expect("退避した").path.clone();
        assert!(
            bak.to_string_lossy().ends_with("ui.json.unreadable.bak"),
            "{bak:?}"
        );
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "{ not json");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ not json",
            "読むだけでは書き換えない"
        );
        // 2 回読んでも写しは増えない
        let _ = load_from(&path);
        assert!(!crate::migration::quarantine_slot_path(&path, 2).exists());
        // 変更の操作で直した形を書く
        let up = update_at(&path, &[UiOp::Reset], 5).unwrap();
        assert!(up.written);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            UiConfig::default().to_text()
        );
        assert_eq!(load_from(&path).state, LoadState::Valid);
        // 無いファイル・UTF-8 でない中身
        assert_eq!(load_from(&dir.join("none.json")).state, LoadState::Absent);
        let raw = dir.join("raw.json");
        std::fs::write(&raw, [0xff, 0xfe, 0x00]).unwrap();
        let loaded = load_from(&raw);
        assert_eq!(loaded.state.code(), "repaired");
        assert!(crate::migration::quarantine_slot_path(&raw, 1).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 書き込みは一時ファイルを残さず断った操作は書かない() {
        let dir = tmp_dir("write");
        let path = dir.join(FILENAME);
        let up = update_at(&path, &[UiOp::Show], 1).unwrap();
        assert!(!up.written && !path.exists(), "見るだけでは作らない");
        let up = update_at(
            &path,
            &[UiOp::ButtonAdd(ButtonSpec {
                value: "split-right".into(),
                ..ButtonSpec::default()
            })],
            1,
        )
        .unwrap();
        assert!(up.written && up.changed);
        let written = std::fs::read_to_string(&path).unwrap();
        let err = update_at(
            &path,
            &[
                UiOp::ButtonRemove {
                    id: "compact".into(),
                },
                UiOp::Set {
                    key: SetKey::ColorsDim,
                    value: json!("magenta"),
                },
            ],
            2,
        )
        .unwrap_err();
        assert!(matches!(err, UpdateError::Invalid(_)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            written,
            "途中まで当てない"
        );
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, [FILENAME], "一時ファイルが残っていない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 帯のトグルは新しい方だけ取り込む() {
        let mut cfg = UiConfig::default();
        assert_eq!(cfg.band_request(), None);
        assert!(import_band(&mut cfg, true, 10));
        assert_eq!(
            cfg.band_request(),
            Some(crate::claude_mod::BandRequest {
                hidden: true,
                at: 10
            })
        );
        assert!(!import_band(&mut cfg, false, 9), "古い報告は捨てる");
        assert!(!import_band(&mut cfg, false, 10), "同じ時刻は捨てる");
        assert!(cfg.band.hidden);
        assert!(import_band(&mut cfg, false, 11));
        assert!(!cfg.band.hidden);
    }

    #[test]
    fn answersは雛形と値とボタンの置き換えの順に当てる() {
        let answers: UiAnswers = serde_json::from_value(json!({
            "preset": "minimal",
            "set": {"usage_bar.place": "band"},
            "buttons": [{"value": "compact"}, {"kind": "tako", "value": "split_right"}],
        }))
        .unwrap();
        let mut cfg = UiConfig::default();
        apply_all(&mut cfg, &answers.to_ops().unwrap(), 1).unwrap();
        assert_eq!(cfg.band.segments, Preset::Minimal.config().band.segments);
        assert_eq!(cfg.usage_bar.place, UsageBarPlace::Band);
        let ids: Vec<_> = cfg.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["compact", "split-right"]);
        // 未知のキーは serde が断る・語彙の外は to_ops が断る
        assert!(serde_json::from_value::<UiAnswers>(json!({"bogus": 1})).is_err());
        let bad: UiAnswers = serde_json::from_value(json!({"preset": "fancy"})).unwrap();
        assert_eq!(bad.to_ops().unwrap_err().allowed, Preset::words());
    }

    #[test]
    fn 選べる値はキーと語彙をすべて載せる() {
        let c = choices();
        for key in SetKey::ALL {
            assert!(
                c["set"][key.as_str()]["values"].is_array(),
                "{}",
                key.as_str()
            );
        }
        assert_eq!(c["button"]["max"], MAX_BUTTONS);
        assert_eq!(
            c["button"]["kinds"]["slash"]["values"]
                .as_array()
                .unwrap()
                .len(),
            SlashCommand::ALL.len()
        );
        assert_eq!(c["ops"].as_array().unwrap().len(), OPS.len());
        let cur = current_values(&UiConfig::default());
        assert_eq!(cur["usage_bar.place"], "prompt_hint");
        assert_eq!(cur["band.segments"][0], "pane");
    }

    /// 性質テスト（受け入れ条件 3）: ランダムな操作列 1,000 通りの後も、ファイルへ書いた
    /// ui.json は常に形を満たし、読み戻すと書いた値と同じになる
    #[test]
    fn ランダムな操作列の後もuijsonは常に形を満たす() {
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                // xorshift64*
                self.0 ^= self.0 >> 12;
                self.0 ^= self.0 << 25;
                self.0 ^= self.0 >> 27;
                self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
            }
            fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
                &items[(self.next() % items.len() as u64) as usize]
            }
        }
        // 正しい語・揺れた語・語彙の外・壊れた形を混ぜる
        let words: Vec<Value> = [
            "compact",
            "/clear",
            "context",
            "cost",
            "model",
            "effort",
            "tako",
            "split-right",
            "split_down",
            "close",
            "open-cwd",
            "background",
            "session-restart-handoff",
            "limit-resume-on",
            "bash",
            "rm -rf /",
            "",
            "  ",
            "pane",
            "tab",
            "workers",
            "attention",
            "ctx",
            "limits",
            "buttons",
            "card",
            "pane,tab",
            "pane pane",
            "5h,7d",
            "five_hour",
            "prompt_hint",
            "band",
            "off",
            "auto",
            "mod",
            "gui_only",
            "always",
            "true",
            "false",
            "yes",
            "suggestion",
            "planMode",
            "#fff",
            "subtle",
            "warning",
            "first",
            "last",
            "left",
            "right",
            "0",
            "1",
            "3",
            "9",
            "npm test",
            "echo \u{1b}[0m",
            "あいうえおかきくけこ",
            "テストを書いて",
            "x\ny",
        ]
        .iter()
        .map(|w| json!(w))
        .chain([
            json!(true),
            json!(false),
            json!(7),
            json!(["pane", "buttons"]),
            json!(null),
        ])
        .collect();
        let keys: Vec<&str> = SetKey::words()
            .into_iter()
            .chain(["band.color", "bogus", ""])
            .collect();
        let ops = [
            "show",
            "set",
            "button_add",
            "button_remove",
            "button_move",
            "reset",
            "preset",
            "bogus",
        ];
        let kinds = [
            None,
            Some("slash"),
            Some("tako"),
            Some("shell"),
            Some("prompt"),
            Some("js"),
        ];
        let dir = tmp_dir("prop");
        let path = dir.join(FILENAME);
        let mut accepted = 0usize;
        let mut refused = 0usize;
        for seed in 1..=1_000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let _ = std::fs::remove_file(&path);
            let steps = 1 + (rng.next() % 12) as usize;
            for step in 0..steps {
                let id_pool: Vec<String> = load_from(&path)
                    .config
                    .buttons
                    .iter()
                    .map(|b| b.id.clone())
                    .chain(["nope".to_string()])
                    .collect();
                let request = UiRequest {
                    op: Some((*rng.pick(&ops)).to_string()),
                    key: Some((*rng.pick(&keys)).to_string()),
                    value: Some(rng.pick(&words).clone()),
                    kind: rng.pick(&kinds).map(str::to_string),
                    id: rng
                        .next()
                        .is_multiple_of(2)
                        .then(|| rng.pick(&id_pool).clone()),
                    label: rng
                        .next()
                        .is_multiple_of(4)
                        .then(|| rng.pick(&words).as_str().unwrap_or("x").to_string()),
                    hotkey: rng
                        .next()
                        .is_multiple_of(4)
                        .then(|| ["a", "c", "1", "Z", "ab", ""][(rng.next() % 6) as usize].into()),
                    to: Some(rng.pick(&words).clone()),
                };
                match request.to_op() {
                    Ok(op) => match update_at(&path, &[op], seed * 100 + step as u64) {
                        Ok(_) => accepted += 1,
                        Err(UpdateError::Invalid(_)) => refused += 1,
                        Err(e) => panic!("seed {seed}: {}", e.message()),
                    },
                    Err(_) => refused += 1,
                }
                if path.exists() {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let parsed = parse_lenient(&text);
                    assert!(
                        parsed.problems.is_empty(),
                        "seed {seed} step {step}: {:?}\n{text}",
                        parsed.problems
                    );
                    assert!(validate_text(&text).is_ok());
                    assert_eq!(parsed.config.to_text(), text, "seed {seed}: 読み戻すと同じ");
                }
            }
        }
        // 両方の道を通っている（全部断られて緑、を防ぐ）
        assert!(
            accepted > 1_000 && refused > 1_000,
            "{accepted} / {refused}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

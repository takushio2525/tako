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
//!   文字として読めない中身（UTF-8 でない・権限で読めない）はバイトのまま番号付きの退避へ移す
//!   （[`crate::load_issue::LoadIssue::read_failed`] = `migration::quarantine_raw`。#2238）。
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

/// カード・チャットを誰が描くか（auto = mod が効いていれば mod、効いていなければ tako）。
///
/// コマンドカード（#1963。判断は `claude_mod::card_owner`）: `auto` = mod が帯に描いたと言った
/// カードだけ GPUI の帯から外す（拾われなければ GPUI へ戻す）・`mod` = mod が効いている間は GPUI の帯を
/// 出さない（帯を隠している等で mod が描けなくても）・`tako` = mod は描かず GPUI の帯のまま（S7-4 前）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    #[default]
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
    /// 何をするか（`tako mod ui` の一覧。表示言語に従う。setup の対話は [`Self::describe_in`] で日本語）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365。日本語は #2365 より前の固定文とバイト一致）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Compact, Ja) => "会話を要約して ctx を空ける",
            (Self::Compact, En) => "summarize the conversation to free ctx",
            (Self::Clear, Ja) => "会話を消して新しく始める",
            (Self::Clear, En) => "clear the conversation and start over",
            (Self::Context, Ja) => "ctx の使い方を見る",
            (Self::Context, En) => "see how ctx is used",
            (Self::Cost, Ja) => "このセッションの費用を見る",
            (Self::Cost, En) => "see the cost of this session",
            (Self::Model, Ja) => "モデルを選び直す",
            (Self::Model, En) => "choose the model again",
            (Self::Effort, Ja) => "effort を選び直す",
            (Self::Effort, En) => "choose the effort again",
            (Self::Tako, Ja) => "tako のサイドバーを開く",
            (Self::Tako, En) => "open the tako sidebar",
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

    /// 何をするか（表示言語に従う。setup の対話は [`Self::describe_in`] で日本語）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365。日本語は #2365 より前の固定文とバイト一致）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::SplitRight, Ja) => "右に新しいペインを開く",
            (Self::SplitRight, En) => "open a new pane on the right",
            (Self::SplitDown, Ja) => "下に新しいペインを開く",
            (Self::SplitDown, En) => "open a new pane below",
            (Self::SessionRestartHarness, Ja) => "会話を保ったまま claude を起動し直す",
            (Self::SessionRestartHarness, En) => "restart claude keeping the conversation",
            (Self::SessionRestartHandoff, Ja) => "引き継ぎを書かせてセッションを交代する",
            (Self::SessionRestartHandoff, En) => {
                "have it write a handoff and switch to a new session"
            }
            (Self::LimitResumeOn, Ja) => "使用制限が解けたら自動で続ける（on）",
            (Self::LimitResumeOn, En) => "continue automatically when the usage limit lifts (on)",
            (Self::LimitResumeOff, Ja) => "使用制限の自動復帰を止める（off）",
            (Self::LimitResumeOff, En) => "stop resuming automatically after the usage limit (off)",
            (Self::Background, Ja) => "このペインを裏へ回す",
            (Self::Background, En) => "send this pane to the background",
            (Self::Close, Ja) => "このペインを閉じる",
            (Self::Close, En) => "close this pane",
            (Self::OpenCwd, Ja) => "このペインのフォルダを開く",
            (Self::OpenCwd, En) => "open this pane's folder",
        }
    }

    /// ボタンを押したときに mod が叩く既存 CLI の引数（`tako` の後ろ。S7-3 #1962）。
    /// 対象のペインは名指しする（tako の再起動をまたいだ claude の `TAKO_PANE_ID` は古いことがある）。
    /// `open-cwd` はペインの cwd が分からなければ `None`（mod はそのボタンを描かない）。
    /// 右クリックメニューとの対応表の正本化は #1882
    pub fn cli_args(self, pane: u64, cwd: Option<&str>) -> Option<Vec<String>> {
        let p = pane.to_string();
        let args: Vec<&str> = match self {
            Self::SplitRight => vec!["split", "--pane", &p, "--right"],
            Self::SplitDown => vec!["split", "--pane", &p, "--down"],
            Self::SessionRestartHarness => {
                vec!["session-restart", "--mode", "harness", "--pane", &p]
            }
            Self::SessionRestartHandoff => {
                vec!["session-restart", "--mode", "handoff", "--pane", &p]
            }
            Self::LimitResumeOn => vec!["limit-resume", "on", "--pane", &p],
            Self::LimitResumeOff => vec!["limit-resume", "off", "--pane", &p],
            Self::Background => vec!["background", "--pane", &p],
            Self::Close => vec!["close", "--pane", &p],
            // ペインの右クリックの「Finder で開く」と同じ（既定のアプリでフォルダを開く）
            Self::OpenCwd => vec!["file", "open", cwd.filter(|c| !c.is_empty())?],
        };
        Some(args.into_iter().map(str::to_string).collect())
    }
}

/// `shell` のボタンの CLI の引数: 新しい tako のペインで実行する（コマンドカードの実行と同じ
/// `spawn_command_pane` = 呼んだペインの下に 35%・ペインの cwd・終わっても閉じない）。
/// コマンドは ui.json へ書く前に `command_card::normalize_command_text` で検証済み
pub fn shell_cli_args(pane: u64, command: &str) -> Vec<String> {
    let p = pane.to_string();
    [
        "run-interactive",
        "--pane",
        &p,
        "--down",
        "--ratio",
        "0.35",
        "--auto-close",
        "never",
        "--",
        command,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// `tako` / `shell` のボタンの id → CLI の引数（応答の `view.button_args`。S7-3 #1962）。
/// `slash` / `prompt` は mod が Claude Code の中で済ませるので載せない
pub fn button_args(
    buttons: &[Button],
    pane: u64,
    cwd: Option<&str>,
) -> BTreeMap<String, Vec<String>> {
    buttons
        .iter()
        .filter_map(|b| {
            let args = match &b.action {
                ButtonAction::Tako { op } => op.cli_args(pane, cwd)?,
                ButtonAction::Shell { command } => shell_cli_args(pane, command),
                ButtonAction::Slash { .. } | ButtonAction::Prompt { .. } => return None,
            };
            Some((b.id.clone(), args))
        })
        .collect()
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
    /// 何をするか（表示言語に従う）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365。日本語は #2365 より前の固定文とバイト一致）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Slash, Ja) => {
                "Claude Code の組み込みコマンドを送る（/compact 等。許可リストだけ）"
            }
            (Self::Slash, En) => {
                "send a built-in Claude Code command (/compact etc.; allow-listed ones only)"
            }
            (Self::Tako, Ja) => "tako の操作を呼ぶ（ペインの右クリックと同じ操作）",
            (Self::Tako, En) => {
                "run a tako action (the same actions as the pane's right-click menu)"
            }
            (Self::Shell, Ja) => {
                "書いたコマンドを新しい tako のペインで実行する（4096 バイトまで）"
            }
            (Self::Shell, En) => "run the command you wrote in a new tako pane (up to 4096 bytes)",
            (Self::Prompt, Ja) => "入力欄へ文を入れる（送らない。1000 文字まで）",
            (Self::Prompt, En) => {
                "put text into the input box (it is not sent; up to 1000 characters)"
            }
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
    /// 何をするか（表示言語に従う。setup の対話は [`Self::describe_in`] で日本語）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365。日本語は #2365 より前の固定文とバイト一致）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Default, Ja) => "既定（ボタンは compact だけ）",
            (Self::Default, En) => "default (the only button is compact)",
            (Self::Recommended, Ja) => {
                "おすすめ（ボタン: compact / context / split right / handoff）"
            }
            (Self::Recommended, En) => {
                "recommended (buttons: compact / context / split right / handoff)"
            }
            (Self::Minimal, Ja) => {
                "最小（帯はペイン名・要注意・ボタンだけ、バーは ctx だけ、チャットの装飾なし）"
            }
            (Self::Minimal, En) => {
                "minimal (the band shows only the pane name, attention and buttons; the bar \
                 shows only ctx; no chat decorations)"
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

    /// 何を決めるか（表示言語に従う）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365。日本語は #2365 より前の固定文とバイト一致）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::BandHidden, Ja) => "帯（プロンプトの上の 1 行）を隠す",
            (Self::BandHidden, En) => "hide the band (the line above the prompt)",
            (Self::BandSegments, Ja) => "帯に並べる区切りと順番",
            (Self::BandSegments, En) => "the segments on the band and their order",
            (Self::UsageBarPlace, Ja) => {
                "使用制限・ctx のバーの置き場（prompt_hint = 入力欄の下の行の末尾）"
            }
            (Self::UsageBarPlace, En) => {
                "where the usage-limit / ctx bar goes (prompt_hint = the end of the line below \
                 the input box)"
            }
            (Self::UsageBarItems, Ja) => "バーに出す項目と順番",
            (Self::UsageBarItems, En) => "the items on the bar and their order",
            (Self::CardsPlace, Ja) => "コマンドカードを誰が描くか",
            (Self::CardsPlace, En) => "who draws the command cards",
            (Self::ChatPlace, Ja) => "チャット風の描画を誰が描くか",
            (Self::ChatPlace, En) => "who draws the chat-style view",
            (Self::ChatBubble, Ja) => "発言の吹き出し",
            (Self::ChatBubble, En) => "speech bubbles for messages",
            (Self::ChatCodeCopy, Ja) => "コードブロックの下のコピーボタン",
            (Self::ChatCodeCopy, En) => "the copy button below code blocks",
            (Self::ChatToolSummary, Ja) => "ツール呼び出しの要約",
            (Self::ChatToolSummary, En) => "summaries of tool calls",
            (Self::ColorsAccent, Ja) => "強調の色（テーマのキー）",
            (Self::ColorsAccent, En) => "the accent color (a theme key)",
            (Self::ColorsWarn, Ja) => "警告の色（テーマのキー）",
            (Self::ColorsWarn, En) => "the warning color (a theme key)",
            (Self::ColorsDim, Ja) => "控えめな文字の色（テーマのキー）",
            (Self::ColorsDim, En) => "the color of subdued text (a theme key)",
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
fn segments_problems(segments: &[BandSegment]) -> Vec<UiProblem> {
    let mut out = Vec::new();
    if segments.is_empty() {
        out.push(UiProblem::SegmentsEmpty);
    }
    if has_duplicate(segments) {
        out.push(UiProblem::SegmentsDuplicate);
    }
    out
}

fn items_problems(items: &[UsageItem]) -> Vec<UiProblem> {
    let mut out = Vec::new();
    if items.is_empty() {
        out.push(UiProblem::ItemsEmpty);
    }
    if has_duplicate(items) {
        out.push(UiProblem::ItemsDuplicate);
    }
    out
}

fn has_duplicate<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(i, a)| items[..i].iter().any(|b| b == a))
}

/// ボタン 1 個の問題（`index` は `buttons[i]` の位置）
fn button_problems(button: &Button, index: usize) -> Vec<UiProblem> {
    let mut out = Vec::new();
    let field = |field, e: UiError| UiProblem::Field {
        index,
        field,
        why: e.what,
    };
    if let Err(e) = check_id(&button.id) {
        out.push(field(ButtonField::Id, e));
    }
    if let Err(e) = check_label(&button.label) {
        out.push(field(ButtonField::Label, e));
    }
    if let Err(e) = check_hotkey(&button.hotkey) {
        out.push(field(ButtonField::Hotkey, e));
    }
    match &button.action {
        ButtonAction::Shell { command } => {
            if let Err(e) = normalize_shell(command) {
                out.push(field(ButtonField::Command, e));
            } else if normalize_shell(command).ok().as_deref() != Some(command.as_str()) {
                out.push(UiProblem::NotNormalized {
                    index,
                    field: ButtonField::Command,
                });
            }
        }
        ButtonAction::Prompt { text } => {
            if let Err(e) = normalize_prompt(text) {
                out.push(field(ButtonField::Text, e));
            } else if normalize_prompt(text).ok().as_deref() != Some(text.as_str()) {
                out.push(UiProblem::NotNormalized {
                    index,
                    field: ButtonField::Text,
                });
            }
        }
        ButtonAction::Slash { .. } | ButtonAction::Tako { .. } => {}
    }
    out
}

/// ボタンの並び全体の問題（個数・id と hotkey の重複）
fn buttons_problems(buttons: &[Button]) -> Vec<UiProblem> {
    let mut out = Vec::new();
    if buttons.len() > MAX_BUTTONS {
        out.push(UiProblem::TooManyButtons { len: buttons.len() });
    }
    for (i, b) in buttons.iter().enumerate() {
        out.extend(button_problems(b, i));
        if buttons[..i].iter().any(|o| o.id == b.id) {
            out.push(UiProblem::DuplicateId { index: i });
        }
        if buttons[..i].iter().any(|o| o.hotkey == b.hotkey) {
            out.push(UiProblem::DuplicateHotkey { index: i });
        }
    }
    out
}

/// 全体の問題（空 = スキーマを満たす。[`validate_text`] と性質テストの判定はこれ 1 本）
pub fn problems(cfg: &UiConfig) -> Vec<UiProblem> {
    let mut out = Vec::new();
    if cfg.schema_version != SCHEMA_VERSION {
        out.push(UiProblem::SchemaVersion);
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
        Err(UiError::new(UiReason::IdShape))
    }
}

fn check_label(label: &str) -> Result<(), UiError> {
    if label.trim() != label || label.is_empty() {
        return Err(UiError::new(UiReason::LabelEmpty));
    }
    if label.chars().any(char::is_control) {
        return Err(UiError::new(UiReason::LabelControl));
    }
    let columns = UnicodeWidthStr::width(label);
    if columns > MAX_LABEL_COLUMNS {
        return Err(UiError::new(UiReason::LabelTooWide { columns }));
    }
    Ok(())
}

/// hotkey の語（数字 1 字か英小文字 1 字）。英字を先に並べる（エンジンの仕様で、空の入力欄の
/// 数字は帯のボタンを押してしまうので、自動の割り当ては英字を先に使う）
fn hotkey_alphabet() -> impl Iterator<Item = char> {
    ('a'..='z').chain('1'..='9').chain(std::iter::once('0'))
}

fn hotkey_words() -> Vec<Allowed> {
    vec![Allowed::Letters, Allowed::Digits]
}

fn check_hotkey(hotkey: &str) -> Result<(), UiError> {
    let mut chars = hotkey.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() || c.is_ascii_digit() => Ok(()),
        _ => Err(UiError::with_items(UiReason::HotkeyShape, hotkey_words())),
    }
}

/// shell のコマンドの正規化（FR-2.22.7 = コマンドカードと同じ 1 実装）
fn normalize_shell(raw: &str) -> Result<String, UiError> {
    use crate::command_card::CommandCardError as E;
    crate::command_card::normalize_command_text(raw).map_err(|e| match e {
        E::CommandTooLong { max, .. } => UiError::new(UiReason::ShellTooLong { max }),
        E::ControlCharacter { .. } => UiError::new(UiReason::ShellControl),
        _ => UiError::new(UiReason::ShellEmpty),
    })
}

/// prompt の文の正規化（CRLF → LF・前後の空白を落とす・改行とタブ以外の制御文字は不可）
fn normalize_prompt(raw: &str) -> Result<String, UiError> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text.is_empty() {
        return Err(UiError::new(UiReason::PromptEmpty));
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        return Err(UiError::new(UiReason::PromptTooLong));
    }
    if text
        .chars()
        .any(|c| c != '\n' && c != '\t' && c.is_control())
    {
        return Err(UiError::new(UiReason::PromptControl));
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
//
// 文言は表示言語に従う（#2365）。日英の正本は各型の `text_in` の (バリアント, 言語) の網羅 match で、
// バリアントを足すと日英の両方を書くまでコンパイルが通らない。`Display` / `message()` は
// `i18n::lang()` でそこを引く。日本語は #2365 より前の固定文とバイト一致
// （`tako-control/tests/issue1762_dispatch_error_lang.rs` が旧文の表と突き合わせる）。
// 1 つの断りの中で日英が混ざらないよう、理由・使える値・枠・形の問題を 1 本でそろえて引く

/// 断った理由と、代わりに使える値（**ui.json は書いていない**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    pub what: UiReason,
    pub allowed: Vec<Allowed>,
}

impl UiError {
    fn new(what: UiReason) -> Self {
        Self {
            what,
            allowed: Vec::new(),
        }
    }

    /// 使える値が語彙の語・ボタンの id（日英で同じ語）のとき
    fn with_allowed<S: Into<String>>(what: UiReason, words: Vec<S>) -> Self {
        Self::with_items(what, Allowed::words(words))
    }

    /// 使える値に範囲（`a〜z` / `1〜8 の番号`）が混ざるとき
    fn with_items(what: UiReason, allowed: Vec<Allowed>) -> Self {
        Self { what, allowed }
    }

    /// CLI / MCP / setup が同じ文言で返す 1 行（表示言語に従う。setup は日本語の枠なので
    /// [`Self::text_in`] で日本語を引く）
    pub fn message(&self) -> String {
        self.text_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての文言（理由・使える値・枠を同じ言語でそろえる）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        let what = self.what.text_in(lang);
        let allowed = || {
            self.allowed
                .iter()
                .map(|a| a.text_in(lang))
                .collect::<Vec<_>>()
                .join(" / ")
        };
        match (self.allowed.is_empty(), lang) {
            (true, Ja) => format!("{what}（ui.json は書いていない）"),
            (true, En) => format!("{what} (ui.json was not written)"),
            (false, Ja) => format!("{what}。使える値: {}（ui.json は書いていない）", allowed()),
            (false, En) => format!(
                "{what}. Allowed values: {} (ui.json was not written)",
                allowed()
            ),
        }
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for UiError {}

/// 断った理由（[`UiError::what`]。枠の「ui.json は書いていない」は [`UiError::text_in`] が足す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiReason {
    /// id の形（英小文字・数字・`-`・[`MAX_ID_CHARS`] 文字まで・先頭は英数字）
    IdShape,
    LabelEmpty,
    LabelControl,
    LabelTooWide {
        columns: usize,
    },
    ShellTooLong {
        max: usize,
    },
    ShellControl,
    ShellEmpty,
    PromptEmpty,
    PromptTooLong,
    PromptControl,
    HotkeyShape,
    /// 値が文字列・真偽・数のどれでもない（`key` は語彙のキーか `value`）
    NotString {
        key: &'static str,
    },
    BadValue {
        key: &'static str,
    },
    NotBool {
        key: &'static str,
    },
    BadListValue {
        key: &'static str,
    },
    ListDuplicate {
        key: &'static str,
    },
    EmptyList {
        key: &'static str,
    },
    MoveToUnreadable,
    MoveToRange {
        len: usize,
    },
    /// 操作に引数が足りない（`op` は頼まれた op の綴りのまま）
    NeedValue {
        op: String,
    },
    NeedContent {
        op: String,
    },
    NeedId {
        op: String,
    },
    NeedTo {
        op: String,
    },
    SetNeedsKey,
    SetKeyUnknown,
    PresetNeedsName,
    PresetUnknown,
    OpUnknown,
    AnswersPresetUnknown,
    AnswersSetKeyUnknown,
    NoSuchButton,
    SameAction {
        id: String,
    },
    TooManyButtons,
    KindUnknown,
    KindNeeded,
    SlashNotAllowed,
    TakoOpUnknown,
    IdTaken,
    HotkeyTaken,
    /// 最後の砦: どの操作を通っても形を満たすはずが崩れた（[`apply_all`]）
    Unshaped {
        problems: Vec<UiProblem>,
    },
}

impl UiReason {
    /// 表示言語を明示しての文言（日英の正本。日本語は #2365 より前の固定文とバイト一致）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::IdShape, Ja) => {
                format!("id は英小文字・数字・`-` の {MAX_ID_CHARS} 文字まで（先頭は英数字）")
            }
            (Self::IdShape, En) => format!(
                "id is up to {MAX_ID_CHARS} lowercase letters, digits or `-` (starting with a letter or digit)"
            ),
            (Self::LabelEmpty, Ja) => "label は空にできない（前後の空白も不可）".to_string(),
            (Self::LabelEmpty, En) => {
                "label cannot be empty (no leading or trailing spaces either)".to_string()
            }
            (Self::LabelControl, Ja) => "label に改行・制御文字は使えない".to_string(),
            (Self::LabelControl, En) => {
                "label cannot contain newlines or control characters".to_string()
            }
            (Self::LabelTooWide { columns }, Ja) => format!(
                "label は {MAX_LABEL_COLUMNS} 桁まで（全角は 2 桁。指定は {columns} 桁）"
            ),
            (Self::LabelTooWide { columns }, En) => format!(
                "label fits in {MAX_LABEL_COLUMNS} columns (a full-width character takes 2; given {columns})"
            ),
            (Self::ShellTooLong { max }, Ja) => format!("shell のコマンドは {max} バイトまで"),
            (Self::ShellTooLong { max }, En) => {
                format!("the shell command is up to {max} bytes")
            }
            (Self::ShellControl, Ja) => {
                "shell のコマンドに制御文字は使えない（改行とタブだけ可）".to_string()
            }
            (Self::ShellControl, En) => {
                "the shell command cannot contain control characters (only newlines and tabs)"
                    .to_string()
            }
            (Self::ShellEmpty, Ja) => "shell のコマンドは空にできない".to_string(),
            (Self::ShellEmpty, En) => "the shell command cannot be empty".to_string(),
            (Self::PromptEmpty, Ja) => "prompt の文は空にできない".to_string(),
            (Self::PromptEmpty, En) => "the prompt text cannot be empty".to_string(),
            (Self::PromptTooLong, Ja) => format!("prompt の文は {MAX_PROMPT_CHARS} 文字まで"),
            (Self::PromptTooLong, En) => {
                format!("the prompt text is up to {MAX_PROMPT_CHARS} characters")
            }
            (Self::PromptControl, Ja) => {
                "prompt の文に制御文字は使えない（改行とタブだけ可）".to_string()
            }
            (Self::PromptControl, En) => {
                "the prompt text cannot contain control characters (only newlines and tabs)"
                    .to_string()
            }
            (Self::HotkeyShape, Ja) => "hotkey は数字 1 字か英小文字 1 字".to_string(),
            (Self::HotkeyShape, En) => "hotkey is one digit or one lowercase letter".to_string(),
            (Self::NotString { key }, Ja) => format!("{key} の値は文字列で渡す"),
            (Self::NotString { key }, En) => format!("pass the value of {key} as a string"),
            (Self::BadValue { key }, Ja) => format!("{key} に使えない値"),
            (Self::BadValue { key }, En) => format!("{key} does not take that value"),
            (Self::NotBool { key }, Ja) => format!("{key} は真偽"),
            (Self::NotBool { key }, En) => format!("{key} is a boolean"),
            (Self::BadListValue { key }, Ja) => format!("{key} に使えない値が入っている"),
            (Self::BadListValue { key }, En) => {
                format!("{key} contains a value it does not take")
            }
            (Self::ListDuplicate { key }, Ja) => {
                format!("{key} に同じ値が 2 回ある（並べたい順に 1 回ずつ）")
            }
            (Self::ListDuplicate { key }, En) => format!(
                "{key} has the same value twice (list each value once, in the order you want)"
            ),
            (Self::EmptyList { key }, Ja) => format!("{key} は 1 つ以上並べる"),
            (Self::EmptyList { key }, En) => format!("{key} needs at least one value"),
            (Self::MoveToUnreadable, Ja) => "移す先の指定が読めない".to_string(),
            (Self::MoveToUnreadable, En) => "cannot read where to move it".to_string(),
            (Self::MoveToRange { len }, Ja) => format!("移す先は 1〜{len}"),
            (Self::MoveToRange { len }, En) => format!("the position is 1 to {len}"),
            (Self::NeedValue { op }, Ja) => format!("{op} には 値（value） が要る"),
            (Self::NeedValue { op }, En) => format!("{op} needs a value (value)"),
            (Self::NeedContent { op }, Ja) => format!("{op} には 中身（value） が要る"),
            (Self::NeedContent { op }, En) => format!("{op} needs the content (value)"),
            (Self::NeedId { op }, Ja) => format!("{op} には ボタンの id（id） が要る"),
            (Self::NeedId { op }, En) => format!("{op} needs the button id (id)"),
            (Self::NeedTo { op }, Ja) => format!("{op} には 移す先（to） が要る"),
            (Self::NeedTo { op }, En) => format!("{op} needs where to move it (to)"),
            (Self::SetNeedsKey, Ja) => "set にはキー（key）が要る".to_string(),
            (Self::SetNeedsKey, En) => "set needs a key (key)".to_string(),
            (Self::SetKeyUnknown, Ja) => "set のキーが語彙に無い".to_string(),
            (Self::SetKeyUnknown, En) => "the set key is not in the vocabulary".to_string(),
            (Self::PresetNeedsName, Ja) => "preset には雛形の名前（value）が要る".to_string(),
            (Self::PresetNeedsName, En) => "preset needs a preset name (value)".to_string(),
            (Self::PresetUnknown, Ja) => "雛形の名前が語彙に無い".to_string(),
            (Self::PresetUnknown, En) => "the preset name is not in the vocabulary".to_string(),
            (Self::OpUnknown, Ja) => "op が語彙に無い".to_string(),
            (Self::OpUnknown, En) => "op is not in the vocabulary".to_string(),
            (Self::AnswersPresetUnknown, Ja) => "mod_ui.preset が語彙に無い".to_string(),
            (Self::AnswersPresetUnknown, En) => {
                "mod_ui.preset is not in the vocabulary".to_string()
            }
            (Self::AnswersSetKeyUnknown, Ja) => "mod_ui.set のキーが語彙に無い".to_string(),
            (Self::AnswersSetKeyUnknown, En) => {
                "a mod_ui.set key is not in the vocabulary".to_string()
            }
            (Self::NoSuchButton, Ja) => "その id のボタンは無い".to_string(),
            (Self::NoSuchButton, En) => "no button has that id".to_string(),
            (Self::SameAction { id }, Ja) => format!(
                "同じ動作のボタン（id {id}）が既にある。ラベル等を変えるなら button_remove してから足す"
            ),
            (Self::SameAction { id }, En) => format!(
                "a button with the same action already exists (id {id}). To change its label or the like, button_remove it first and add it again"
            ),
            (Self::TooManyButtons, Ja) => {
                format!("ボタンは {MAX_BUTTONS} 個まで（どれかを button_remove で外す）")
            }
            (Self::TooManyButtons, En) => {
                format!("up to {MAX_BUTTONS} buttons (remove one with button_remove)")
            }
            (Self::KindUnknown, Ja) => "ボタンの kind が語彙に無い".to_string(),
            (Self::KindUnknown, En) => "the button kind is not in the vocabulary".to_string(),
            (Self::KindNeeded, Ja) => {
                "ボタンの kind が要る（値が slash / tako の語でないので推せない）".to_string()
            }
            (Self::KindNeeded, En) => {
                "the button needs a kind (the value is not a slash / tako word, so it cannot be guessed)"
                    .to_string()
            }
            (Self::SlashNotAllowed, Ja) => "slash で送れるのは許可リストのコマンドだけ".to_string(),
            (Self::SlashNotAllowed, En) => {
                "slash sends only the commands on the allow list".to_string()
            }
            (Self::TakoOpUnknown, Ja) => "tako の操作 id が語彙に無い".to_string(),
            (Self::TakoOpUnknown, En) => {
                "the tako operation id is not in the vocabulary".to_string()
            }
            (Self::IdTaken, Ja) => "その id は別のボタンが使っている".to_string(),
            (Self::IdTaken, En) => "another button already uses that id".to_string(),
            (Self::HotkeyTaken, Ja) => "その hotkey は別のボタンが使っている".to_string(),
            (Self::HotkeyTaken, En) => "another button already uses that hotkey".to_string(),
            (Self::Unshaped { problems }, Ja) => format!(
                "操作の結果が形を満たさない（{}）",
                join_problems(problems, Ja)
            ),
            (Self::Unshaped { problems }, En) => format!(
                "the result of the operation does not fit the shape ({})",
                join_problems(problems, En)
            ),
        }
    }
}

impl std::fmt::Display for UiReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 使える値の 1 つ（[`UiError::allowed`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowed {
    /// 語彙の語・ボタンの id / hotkey（日英で同じ語）
    Word(String),
    /// hotkey の英小文字 1 字
    Letters,
    /// hotkey の数字 1 字
    Digits,
    /// 移す先の番号（1〜[`MAX_BUTTONS`]）
    Position,
}

impl Allowed {
    /// 語の並び（語彙の `words()`・ボタンの id）を使える値へ
    pub fn words<S: Into<String>>(words: Vec<S>) -> Vec<Self> {
        words.into_iter().map(|w| Self::Word(w.into())).collect()
    }

    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Word(word), _) => word.clone(),
            (Self::Letters, Ja) => "a〜z".to_string(),
            (Self::Letters, En) => "a-z".to_string(),
            (Self::Digits, Ja) => "0〜9".to_string(),
            (Self::Digits, En) => "0-9".to_string(),
            (Self::Position, Ja) => format!("1〜{MAX_BUTTONS} の番号"),
            (Self::Position, En) => format!("a number 1-{MAX_BUTTONS}"),
        }
    }
}

impl std::fmt::Display for Allowed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// ボタンのどのフィールドか（問題の文の `buttons[i].<ここ>`。日英で同じ綴り）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonField {
    Id,
    Label,
    Hotkey,
    Command,
    Text,
}

impl ButtonField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Label => "label",
            Self::Hotkey => "hotkey",
            Self::Command => "action.command",
            Self::Text => "action.text",
        }
    }
}

/// ui.json の形の問題（[`problems`] / [`parse_lenient`]）。**利用者が書いた値は入れない**
/// （キーの位置と種類だけ。知らないキーは名前だけを短く載せる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiProblem {
    SchemaVersion,
    SegmentsEmpty,
    SegmentsDuplicate,
    ItemsEmpty,
    ItemsDuplicate,
    TooManyButtons {
        len: usize,
    },
    DuplicateId {
        index: usize,
    },
    DuplicateHotkey {
        index: usize,
    },
    /// ボタンのフィールドが検証で断られる（理由は [`UiReason`] の文）
    Field {
        index: usize,
        field: ButtonField,
        why: UiReason,
    },
    /// 前後の空白・改行を落とした形で書いていない
    NotNormalized {
        index: usize,
        field: ButtonField,
    },
    NotJson {
        line: usize,
        column: usize,
    },
    NotObject,
    UnknownKey {
        key: String,
    },
    /// 最上位の部分が無い
    Missing {
        key: &'static str,
    },
    /// 最上位の部分が語彙・形に合わない
    SectionUnshaped {
        key: &'static str,
    },
    ButtonsNotArray,
    ButtonUnshaped {
        index: usize,
    },
    SameIdOrHotkey {
        index: usize,
    },
    OverMax {
        index: usize,
    },
    /// そのボタンを外した（中身の問題に「外した」を添える）
    Dropped(Box<UiProblem>),
    NotUtf8,
    /// 読み取りに失敗（`kind` は `io::ErrorKind` の表示）
    ReadIo {
        kind: String,
    },
}

impl UiProblem {
    /// 表示言語を明示しての文言（日英の正本。日本語は #2365 より前の固定文とバイト一致）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::SchemaVersion, Ja) => format!("schema_version は {SCHEMA_VERSION}"),
            (Self::SchemaVersion, En) => format!("schema_version is {SCHEMA_VERSION}"),
            (Self::SegmentsEmpty, Ja) => {
                "band.segments が空（隠すなら band.hidden を true にする）".to_string()
            }
            (Self::SegmentsEmpty, En) => {
                "band.segments is empty (to hide the band, set band.hidden to true)".to_string()
            }
            (Self::SegmentsDuplicate, Ja) => "band.segments に同じ区切りが 2 回ある".to_string(),
            (Self::SegmentsDuplicate, En) => "band.segments has the same segment twice".to_string(),
            (Self::ItemsEmpty, Ja) => {
                "usage_bar.items が空（消すなら usage_bar.place を off にする）".to_string()
            }
            (Self::ItemsEmpty, En) => {
                "usage_bar.items is empty (to remove the bar, set usage_bar.place to off)"
                    .to_string()
            }
            (Self::ItemsDuplicate, Ja) => "usage_bar.items に同じ項目が 2 回ある".to_string(),
            (Self::ItemsDuplicate, En) => "usage_bar.items has the same item twice".to_string(),
            (Self::TooManyButtons { len }, Ja) => {
                format!("buttons が {len} 個（{MAX_BUTTONS} 個まで）")
            }
            (Self::TooManyButtons { len }, En) => {
                format!("buttons has {len} (up to {MAX_BUTTONS})")
            }
            (Self::DuplicateId { index }, Ja) => {
                format!("buttons[{index}].id が前のボタンと重複")
            }
            (Self::DuplicateId { index }, En) => {
                format!("buttons[{index}].id is the same as an earlier button")
            }
            (Self::DuplicateHotkey { index }, Ja) => {
                format!("buttons[{index}].hotkey が前のボタンと重複")
            }
            (Self::DuplicateHotkey { index }, En) => {
                format!("buttons[{index}].hotkey is the same as an earlier button")
            }
            (Self::Field { index, field, why }, lang) => {
                format!("buttons[{index}].{}: {}", field.as_str(), why.text_in(lang))
            }
            (Self::NotNormalized { index, field }, Ja) => format!(
                "buttons[{index}].{}: 前後の空白・改行を落とした形で書く",
                field.as_str()
            ),
            (Self::NotNormalized { index, field }, En) => format!(
                "buttons[{index}].{}: write it without leading or trailing spaces and newlines",
                field.as_str()
            ),
            (Self::NotJson { line, column }, Ja) => {
                format!("JSON として読めない（{line} 行 {column} 桁）")
            }
            (Self::NotJson { line, column }, En) => {
                format!("cannot read it as JSON (line {line}, column {column})")
            }
            (Self::NotObject, Ja) => "最上位が JSON のオブジェクトではない".to_string(),
            (Self::NotObject, En) => "the top level is not a JSON object".to_string(),
            (Self::UnknownKey { key }, Ja) => format!("知らないキー `{key}`"),
            (Self::UnknownKey { key }, En) => format!("unknown key `{key}`"),
            (Self::Missing { key }, Ja) => format!("{key} が無い"),
            (Self::Missing { key }, En) => format!("{key} is missing"),
            (Self::SectionUnshaped { key }, Ja) => format!("{key} が語彙・形に合わない"),
            (Self::SectionUnshaped { key }, En) => {
                format!("{key} does not fit the vocabulary or shape")
            }
            (Self::ButtonsNotArray, Ja) => "buttons が配列ではない".to_string(),
            (Self::ButtonsNotArray, En) => "buttons is not an array".to_string(),
            (Self::ButtonUnshaped { index }, Ja) => {
                format!("buttons[{index}] が語彙・形に合わない")
            }
            (Self::ButtonUnshaped { index }, En) => {
                format!("buttons[{index}] does not fit the vocabulary or shape")
            }
            (Self::SameIdOrHotkey { index }, Ja) => {
                format!("buttons[{index}] の id か hotkey が前のボタンと重複")
            }
            (Self::SameIdOrHotkey { index }, En) => {
                format!("buttons[{index}] has the same id or hotkey as an earlier button")
            }
            (Self::OverMax { index }, Ja) => {
                format!("buttons[{index}] は {MAX_BUTTONS} 個を超えた")
            }
            (Self::OverMax { index }, En) => {
                format!("buttons[{index}] is past the {MAX_BUTTONS}-button limit")
            }
            (Self::Dropped(inner), Ja) => format!("{}（外した）", inner.text_in(Ja)),
            (Self::Dropped(inner), En) => format!("{} (dropped)", inner.text_in(En)),
            (Self::NotUtf8, Ja) => "UTF-8 として読めない".to_string(),
            (Self::NotUtf8, En) => "cannot read it as UTF-8".to_string(),
            (Self::ReadIo { kind }, Ja) => format!("読み取りに失敗（{kind}）"),
            (Self::ReadIo { kind }, En) => format!("reading failed ({kind})"),
        }
    }

    fn dropped(self) -> Self {
        Self::Dropped(Box::new(self))
    }
}

impl std::fmt::Display for UiProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 問題の並びを 1 行へ（`; ` 区切り）
pub fn join_problems(problems: &[UiProblem], lang: crate::i18n::Lang) -> String {
    problems
        .iter()
        .map(|p| p.text_in(lang))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 値（文字列・真偽・配列のどれか）から語を取り出す。不正なら理由
fn word_of(value: &Value, key: &'static str) -> Result<String, UiError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(UiError::new(UiReason::NotString { key })),
    }
}

fn parse_one<T: Vocab>(value: &Value, key: &'static str) -> Result<T, UiError> {
    let word = word_of(value, key)?;
    T::parse(&word).ok_or_else(|| UiError::with_allowed(UiReason::BadValue { key }, T::words()))
}

fn parse_bool(value: &Value, key: &'static str) -> Result<bool, UiError> {
    let word = word_of(value, key)?;
    match canonical(&word).as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(UiError::with_allowed(
            UiReason::NotBool { key },
            vec!["true", "false"],
        )),
    }
}

/// 並びのある部分集合。配列か、`,` / 空白で区切った文字列
fn parse_list<T: Vocab>(value: &Value, key: &'static str) -> Result<Vec<T>, UiError> {
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
        let item = T::parse(word)
            .ok_or_else(|| UiError::with_allowed(UiReason::BadListValue { key }, T::words()))?;
        if out.contains(&item) {
            return Err(UiError::new(UiReason::ListDuplicate { key }));
        }
        out.push(item);
    }
    if out.is_empty() {
        return Err(UiError::with_allowed(
            UiReason::EmptyList { key },
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
        let allowed = || {
            let mut out = vec![Allowed::Position];
            out.extend(Allowed::words(vec!["first", "last", "left", "right"]));
            out
        };
        let unreadable = || UiError::with_items(UiReason::MoveToUnreadable, allowed());
        let word = match value {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.trim().to_string(),
            _ => return Err(unreadable()),
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
                .ok_or_else(unreadable),
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
        let need = |what: fn(String) -> UiReason| UiError::new(what(self.op_name()));
        match op.as_str() {
            "" | "show" | "list" | "status" => Ok(UiOp::Show),
            "set" => {
                let key_word = self
                    .key
                    .as_deref()
                    .ok_or_else(|| UiError::with_allowed(UiReason::SetNeedsKey, SetKey::words()))?;
                let key = SetKey::parse(key_word).ok_or_else(|| {
                    UiError::with_allowed(UiReason::SetKeyUnknown, SetKey::words())
                })?;
                let value = self
                    .value
                    .clone()
                    .ok_or_else(|| need(|op| UiReason::NeedValue { op }))?;
                Ok(UiOp::Set { key, value })
            }
            "buttonadd" | "add" => {
                let value = self
                    .value
                    .as_ref()
                    .map(|v| word_of(v, "value"))
                    .transpose()?
                    .ok_or_else(|| need(|op| UiReason::NeedContent { op }))?;
                Ok(UiOp::ButtonAdd(ButtonSpec {
                    kind: self.kind.clone(),
                    value,
                    label: self.label.clone(),
                    hotkey: self.hotkey.clone(),
                    id: self.id.clone(),
                }))
            }
            "buttonremove" | "remove" | "rm" => Ok(UiOp::ButtonRemove {
                id: self
                    .id
                    .clone()
                    .ok_or_else(|| need(|op| UiReason::NeedId { op }))?,
            }),
            "buttonmove" | "move" | "mv" => Ok(UiOp::ButtonMove {
                id: self
                    .id
                    .clone()
                    .ok_or_else(|| need(|op| UiReason::NeedId { op }))?,
                to: MoveTo::parse(
                    self.to
                        .as_ref()
                        .ok_or_else(|| need(|op| UiReason::NeedTo { op }))?,
                )?,
            }),
            "reset" => Ok(UiOp::Reset),
            "preset" => {
                let word = self
                    .value
                    .as_ref()
                    .map(|v| word_of(v, "value"))
                    .transpose()?
                    .ok_or_else(|| {
                        UiError::with_allowed(UiReason::PresetNeedsName, Preset::words())
                    })?;
                Preset::parse(&word)
                    .map(UiOp::Preset)
                    .ok_or_else(|| UiError::with_allowed(UiReason::PresetUnknown, Preset::words()))
            }
            _ => Err(UiError::with_allowed(UiReason::OpUnknown, OPS.to_vec())),
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
                UiError::with_allowed(UiReason::AnswersPresetUnknown, Preset::words())
            })?;
            ops.push(UiOp::Preset(preset));
        }
        for (key, value) in &self.set {
            let key = SetKey::parse(key).ok_or_else(|| {
                UiError::with_allowed(UiReason::AnswersSetKeyUnknown, SetKey::words())
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
        return Err(UiError::new(UiReason::Unshaped { problems: left }));
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
                UiReason::NoSuchButton,
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
                UiReason::MoveToRange {
                    len: cfg.buttons.len(),
                },
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
            return Err(UiError::new(UiReason::SameAction {
                id: same.id.clone(),
            }));
        }
        return Ok(());
    }
    if cfg.buttons.len() >= MAX_BUTTONS {
        return Err(UiError::with_allowed(
            UiReason::TooManyButtons,
            cfg.buttons.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
        ));
    }
    cfg.buttons.push(button);
    Ok(())
}

/// 材料からボタンを組む（id・ラベル・hotkey を省いたら推す）。`existing` と重ならないように選ぶ
pub fn build_button(spec: &ButtonSpec, existing: &[Button]) -> Result<Button, UiError> {
    let kind = match spec.kind.as_deref() {
        Some(word) => ButtonKind::parse(word)
            .ok_or_else(|| UiError::with_allowed(UiReason::KindUnknown, ButtonKind::words()))?,
        None if SlashCommand::parse(&spec.value).is_some() => ButtonKind::Slash,
        None if TakoOp::parse(&spec.value).is_some() => ButtonKind::Tako,
        None => {
            return Err(UiError::with_allowed(
                UiReason::KindNeeded,
                ButtonKind::words(),
            ))
        }
    };
    let action = match kind {
        ButtonKind::Slash => ButtonAction::Slash {
            command: SlashCommand::parse(&spec.value).ok_or_else(|| {
                UiError::with_allowed(UiReason::SlashNotAllowed, SlashCommand::words())
            })?,
        },
        ButtonKind::Tako => ButtonAction::Tako {
            op: TakoOp::parse(&spec.value)
                .ok_or_else(|| UiError::with_allowed(UiReason::TakoOpUnknown, TakoOp::words()))?,
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
                    UiReason::IdTaken,
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
                    UiReason::HotkeyTaken,
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

/// `tako mod ui` の応答（CLI `--json` / MCP `tako_mod` の action=ui）と CLI の表示の枠の文（#2365）。
///
/// 断りの文（[`UiError`] 一式）と同じ形で、日英の正本は [`Self::text_in`] の (バリアント, 言語) の
/// 網羅 match。`Display` は表示言語（`i18n::lang()`）。日本語は #2365 より前の固定文とバイト一致
/// （`tako-control/tests/issue1762_dispatch_error_lang.rs` が旧文の表と突き合わせる）。
/// パス・語彙の語・利用者が書いた値は言語に依らずそのまま載せる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiText {
    // ---- 応答の枠 ----
    /// 変更の応答の `applies_to`（どこへいつ効くか）
    AppliesTo,
    /// 新しい tako が書いた形のときの応答の `note`
    FutureNote { version: u64 },
    /// `choices.button.hotkey`（打鍵の決まり）
    HotkeyRule,
    /// 置き場（`<data_dir>/claude-mod/ui.json`）が決まらない
    NoDataDir,
    // ---- CLI の表示（`tako mod ui` の人向けの出力） ----
    /// 見出し（`{path}` の後ろに置き場の様子 = `State*` が続く）
    Heading { path: String },
    /// 置き場の様子: ファイルが無い
    StateAbsent,
    /// 置き場の様子: 読めない部分を既定で動かした（`quarantine` = 元を残した場所）
    StateRepaired { quarantine: String },
    /// 置き場の様子: 新しい tako が書いた形
    StateFuture,
    /// 置き場の様子: 読めない
    StateReadError,
    /// 変更の結果: 変えた
    Changed { path: String },
    /// 変更の結果: もうその値だった
    Unchanged { path: String },
    /// 値の行の末尾の使える値（`list` = 選んで並べる種類）
    Allowed { values: String, list: bool },
    /// ボタンの一覧の見出し
    ButtonsHeading { count: usize, max: usize },
    /// ボタンに使える動作の見出し
    ButtonActionsHeading,
    /// shell / prompt のボタンの書き方の行
    ShellPromptLine,
    /// 例の見出し
    ExamplesHeading,
}

impl UiText {
    /// 表示言語を明示しての文言（#2365）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::AppliesTo, Ja) => "tako のペインの claude へは次の報告（最大 15 秒）で届く\
                 （ui.json は tako の data dir にあり、Claude Code の設定 dir へは書かない）"
                .into(),
            (Self::AppliesTo, En) => "reaches claude in tako panes with the next report \
                 (within 15 seconds; ui.json lives in tako's data dir and nothing is written \
                 to Claude Code's config dir)"
                .into(),
            (Self::FutureNote { version }, Ja) => format!(
                "新しい tako が書いた形（schema_version {version}）。読める範囲で使い、書き換えない"
            ),
            (Self::FutureNote { version }, En) => format!(
                "written by a newer tako (schema_version {version}); used as far as it can be \
                 read and not rewritten"
            ),
            (Self::HotkeyRule, Ja) => {
                "数字 1 字か英小文字 1 字（重複不可。省くと空いている英字を選ぶ）".into()
            }
            (Self::HotkeyRule, En) => "one digit or one lowercase letter (no duplicates; \
                 when omitted, a free letter is chosen)"
                .into(),
            (Self::NoDataDir, Ja) => crate::paths::NoDataDir.text_in(Ja).into(),
            (Self::NoDataDir, En) => crate::paths::NoDataDir.text_in(En).into(),
            (Self::Heading { path }, Ja) => format!("Claude Code の画面の UI（tako mod）: {path}"),
            (Self::Heading { path }, En) => format!("Claude Code screen UI (tako mod): {path}"),
            (Self::StateAbsent, Ja) => "（ファイルは無い = 既定で動いている）".into(),
            (Self::StateAbsent, En) => " (no file = running on the defaults)".into(),
            (Self::StateRepaired { quarantine }, Ja) => {
                format!("（読めない部分を既定で動かした。元は {quarantine} に残した）")
            }
            (Self::StateRepaired { quarantine }, En) => format!(
                " (the unreadable parts run on the defaults; the original was kept at {quarantine})"
            ),
            (Self::StateFuture, Ja) => {
                "（新しい tako が書いた形。読める範囲で使い、書き換えない）".into()
            }
            (Self::StateFuture, En) => {
                " (written by a newer tako; used as far as it can be read and not rewritten)".into()
            }
            (Self::StateReadError, Ja) => "（読めないので既定で動いている）".into(),
            (Self::StateReadError, En) => " (cannot be read, so running on the defaults)".into(),
            (Self::Changed { path }, Ja) => format!("変えた: {path}"),
            (Self::Changed { path }, En) => format!("changed: {path}"),
            (Self::Unchanged { path }, Ja) => format!("変わらない（もうその値）: {path}"),
            (Self::Unchanged { path }, En) => format!("unchanged (already that value): {path}"),
            // `list` = 選んで並べる種類（帯の区切り・バーの項目）
            (Self::Allowed { values, list }, Ja) => match list {
                true => format!("（{values} から並べる）"),
                false => format!("（{values}）"),
            },
            (Self::Allowed { values, list }, En) => match list {
                true => format!("(pick and order: {values})"),
                false => format!("({values})"),
            },
            (Self::ButtonsHeading { count, max }, Ja) => format!("ボタン（{count} / {max} 個）:"),
            (Self::ButtonsHeading { count, max }, En) => format!("Buttons ({count} / {max}):"),
            (Self::ButtonActionsHeading, Ja) => "ボタンに使える動作:".into(),
            (Self::ButtonActionsHeading, En) => "Actions a button can run:".into(),
            (Self::ShellPromptLine, Ja) => "shell  \"<コマンド>\"（新しいペインで実行）・\
                 prompt \"<文>\"（入力欄へ入れるだけ）"
                .into(),
            (Self::ShellPromptLine, En) => "shell  \"<command>\" (runs in a new pane) · \
                 prompt \"<text>\" (only puts it into the input box)"
                .into(),
            (Self::ExamplesHeading, Ja) => "例:".into(),
            (Self::ExamplesHeading, En) => "Examples:".into(),
        }
    }
}

impl std::fmt::Display for UiText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 選べる値の一覧（`tako mod ui` / MCP の応答の `choices`。表示言語に従う）
pub fn choices() -> Value {
    choices_in(crate::i18n::lang())
}

/// 表示言語を明示しての選べる値の一覧（説明の文 `about` / `hotkey` だけが言語で変わる。#2365）
pub fn choices_in(lang: crate::i18n::Lang) -> Value {
    let mut set = Map::new();
    for key in SetKey::ALL {
        let (shape, values) = match key.shape() {
            Shape::Bool => ("bool", vec!["true", "false"]),
            Shape::One(v) => ("one", v),
            Shape::List(v) => ("list", v),
        };
        set.insert(
            key.as_str().into(),
            json!({ "type": shape, "values": values, "about": key.describe_in(lang) }),
        );
    }
    json!({
        "ops": OPS,
        "set": set,
        "button": {
            "max": MAX_BUTTONS,
            "label_max_columns": MAX_LABEL_COLUMNS,
            "hotkey": UiText::HotkeyRule.text_in(lang),
            "kinds": {
                "slash": {
                    "about": ButtonKind::Slash.describe_in(lang),
                    "values": SlashCommand::ALL.iter().map(|c| json!({"value": c.as_str(), "about": c.describe_in(lang)})).collect::<Vec<_>>(),
                },
                "tako": {
                    "about": ButtonKind::Tako.describe_in(lang),
                    "values": TakoOp::ALL.iter().map(|o| json!({"value": o.as_str(), "about": o.describe_in(lang)})).collect::<Vec<_>>(),
                },
                "shell": { "about": ButtonKind::Shell.describe_in(lang) },
                "prompt": { "about": ButtonKind::Prompt.describe_in(lang) },
            },
        },
        "presets": Preset::ALL.iter().map(|p| json!({"value": p.as_str(), "about": p.describe_in(lang)})).collect::<Vec<_>>(),
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
    pub problems: Vec<UiProblem>,
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

/// 今の形として読めるか（移行の `validate`）。新しい形は通す（移行の計画が「目標超過」で断る）。
/// 理由は日本語で返す（移行の申告 = `tako migrate` と persist.log の文はほかの種別も日本語。#2365）
pub fn validate_text(text: &str) -> Result<(), String> {
    let parsed = parse_lenient(text);
    if parsed.is_future() || parsed.problems.is_empty() {
        Ok(())
    } else {
        Err(join_problems(&parsed.problems, crate::i18n::Lang::Ja))
    }
}

/// 部分ごとに読む。壊れた部分は既定へ落として `problems` に積む（読める部分は捨てない）
pub fn parse_lenient(text: &str) -> Parsed {
    let fallback = |problem: UiProblem| Parsed {
        config: UiConfig::default(),
        problems: vec![problem],
        version: None,
    };
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return fallback(UiProblem::NotJson {
                line: e.line(),
                column: e.column(),
            })
        }
    };
    let Value::Object(map) = value else {
        return fallback(UiProblem::NotObject);
    };
    let mut problems = Vec::new();
    let version = map.get("schema_version").and_then(Value::as_u64);
    match version {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) if v > u64::from(SCHEMA_VERSION) => {}
        Some(_) | None => problems.push(UiProblem::SchemaVersion),
    }
    for key in map.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            let shown: String = key.chars().filter(|c| !c.is_control()).take(32).collect();
            problems.push(UiProblem::UnknownKey { key: shown });
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
    key: &'static str,
    default: T,
    problems: &mut Vec<UiProblem>,
) -> T {
    match map.get(key) {
        None => {
            problems.push(UiProblem::Missing { key });
            default
        }
        // 型で読めて、書き戻すと同じ形になる = スキーマどおり（余計なキー・null の混入も弾く）
        Some(v) => match serde_json::from_value::<T>(v.clone()) {
            Ok(parsed) if serde_json::to_value(&parsed).ok().as_ref() == Some(v) => parsed,
            _ => {
                problems.push(UiProblem::SectionUnshaped { key });
                default
            }
        },
    }
}

/// ボタンは 1 個ずつ読む（壊れたものだけ落とす。重複・上限を超えたものも落とす）
fn lenient_buttons(value: Option<&Value>, problems: &mut Vec<UiProblem>) -> Vec<Button> {
    let items = match value {
        None => {
            problems.push(UiProblem::Missing { key: "buttons" });
            return default_buttons();
        }
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.push(UiProblem::ButtonsNotArray);
            return default_buttons();
        }
    };
    let mut out: Vec<Button> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let button = serde_json::from_value::<Button>(item.clone())
            .ok()
            .filter(|b| serde_json::to_value(b).ok().as_ref() == Some(item));
        let Some(button) = button else {
            problems.push(UiProblem::ButtonUnshaped { index: i }.dropped());
            continue;
        };
        let mine = button_problems(&button, i);
        if !mine.is_empty() {
            problems.extend(mine.into_iter().map(UiProblem::dropped));
            continue;
        }
        if out
            .iter()
            .any(|b| b.id == button.id || b.hotkey == button.hotkey)
        {
            problems.push(UiProblem::SameIdOrHotkey { index: i }.dropped());
            continue;
        }
        if out.len() >= MAX_BUTTONS {
            problems.push(UiProblem::OverMax { index: i }.dropped());
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
        problems: Vec<UiProblem>,
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
/// 同じ中身なら写しを増やさない）。解釈できない部分があるだけならファイルは書き換えない
/// （書くのは変更の操作だけ）。
///
/// 文字として読めない中身（UTF-8 でない・権限で読めない）は文字列の退避口を通れないので、
/// **バイトのまま番号付きの退避へ移して**既定で動く（`LoadIssue::read_failed` =
/// `migration::quarantine_raw`。recent / shortcuts と同じ口。#2238）。以前は 1 本目が空いて
/// いるときだけ写していたので、別の中身で 2 回目に壊れると次の変更の操作の書き込みが消していた
pub fn load_from(path: &Path) -> Loaded {
    let read = if crate::load_issue::legacy_2238() {
        legacy_2238_read(path)
    } else {
        read_text(path)
    };
    let text = match read {
        Ok(text) => text,
        Err(state) => {
            return Loaded {
                config: UiConfig::default(),
                state,
            }
        }
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

/// 文字として読む。読めなければ既定で動く様子を返す（不在 / 文字として読めない中身を
/// バイトのまま退避へ移した。#2238）
fn read_text(path: &Path) -> Result<String, LoadState> {
    let e = match std::fs::read_to_string(path) {
        Ok(text) => return Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(LoadState::Absent),
        Err(e) => e,
    };
    let quarantine = crate::load_issue::LoadIssue::read_failed(path, &e)
        .and_then(|issue| issue.quarantine().cloned());
    let utf8 = e.kind() == std::io::ErrorKind::InvalidData;
    Err(match quarantine {
        // 移せなかった権限の誤りは従来どおり（移せないのは置き場へ書けないときで、
        // そのときは変更の操作の書き込みも失敗する = 元は消えない）
        None if !utf8 => LoadState::ReadError {
            reason: e.kind().to_string(),
        },
        quarantine => LoadState::Repaired {
            problems: vec![if utf8 {
                UiProblem::NotUtf8
            } else {
                UiProblem::ReadIo {
                    kind: e.kind().to_string(),
                }
            }],
            quarantine,
        },
    })
}

/// #2238 の修正前（`TAKO_2238_LEGACY=1`）: UTF-8 でない中身は 1 本目が空いているときだけ写す
/// （2 回目に別の中身で壊れると写らない）。権限で読めないファイルは退避しない
fn legacy_2238_read(path: &Path) -> Result<String, LoadState> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(LoadState::Absent),
        Err(e) => {
            return Err(LoadState::ReadError {
                reason: e.kind().to_string(),
            })
        }
    };
    String::from_utf8(bytes).map_err(|_| {
        let slot = crate::migration::quarantine_slot_path(path, 1);
        if !slot.exists() {
            let _ = std::fs::copy(path, &slot);
        }
        LoadState::Repaired {
            problems: vec![UiProblem::NotUtf8],
            quarantine: slot.exists().then_some(crate::migration::Quarantine {
                path: slot,
                copied: true,
                evicted: false,
            }),
        }
    })
}

/// 書く（同じ dir の一時ファイルへ書いてから差し替える = 途中の中身が他のプロセスから見えない）
pub fn save_to(path: &Path, cfg: &UiConfig) -> std::io::Result<()> {
    let left = problems(cfg);
    if !left.is_empty() {
        return Err(std::io::Error::other(SaveRefusal::Unshaped(left)));
    }
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other(SaveRefusal::NoDir))?;
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
    /// 表示言語に従う 1 行（[`Self::text_in`] を `i18n::lang()` で引く）
    pub fn message(&self) -> String {
        self.text_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての文言（日英の正本。`Io` の中身は io の誤りの表示をそのまま添える）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Invalid(e), lang) => e.text_in(lang),
            (Self::Future { version }, Ja) => format!(
                "ui.json は新しい tako が書いた形（schema_version {version}）なので、この tako では書き換えない（tako を更新する）"
            ),
            (Self::Future { version }, En) => format!(
                "ui.json was written by a newer tako (schema_version {version}), so this tako does not rewrite it (update tako)"
            ),
            (Self::Io(e), Ja) => format!("ui.json を書けない: {e}"),
            (Self::Io(e), En) => format!("cannot write ui.json: {e}"),
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// [`save_to`] が書く前に断った理由（`io::Error` に包んで返す。表示は表示言語に従う）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveRefusal {
    /// 形を満たさない値（[`apply_all`] が先に断るので、ここへ来るのは呼び手の誤り）
    Unshaped(Vec<UiProblem>),
    /// 置き場（親 dir）が決まらない
    NoDir,
}

impl SaveRefusal {
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Unshaped(problems), Ja) => format!(
                "形を満たさない値は書かない（{}）",
                join_problems(problems, Ja)
            ),
            (Self::Unshaped(problems), En) => format!(
                "values that do not fit the shape are not written ({})",
                join_problems(problems, En)
            ),
            (Self::NoDir, Ja) => "ui.json の置き場が決まらない".to_string(),
            (Self::NoDir, En) => "cannot tell where ui.json goes".to_string(),
        }
    }
}

impl std::fmt::Display for SaveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl std::error::Error for SaveRefusal {}

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
        assert_eq!(parsed.problems, Vec::<UiProblem>::new());
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
        assert_eq!(err.allowed, Allowed::words(ButtonKind::words()));
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
        assert_eq!(err.allowed, Allowed::words(SlashCommand::words()));
        // 枠の「書いていない」は日英とも理由と同じ言語で添える（#2365）
        let ja = err.text_in(crate::i18n::Lang::Ja);
        assert!(ja.contains("ui.json は書いていない"), "{ja}");
        let en = err.text_in(crate::i18n::Lang::En);
        assert!(en.contains("(ui.json was not written)"), "{en}");
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
        assert_eq!(err.allowed, Allowed::words(ThemeColor::words()));
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
        assert_eq!(err.allowed, Allowed::words(SetKey::words()));
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
            assert!(
                matches!(err.what, UiReason::LabelTooWide { columns: 17 | 18 }),
                "{err}"
            );
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
        assert!(!err.allowed.contains(&Allowed::Word("c".into())), "{err}");
        assert!(err.allowed.contains(&Allowed::Word("a".into())), "{err}");
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
        assert_eq!(err.what, UiReason::HotkeyShape, "{err}");
        assert_eq!(err.allowed, [Allowed::Letters, Allowed::Digits]);
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
        assert_eq!(err.what, UiReason::ShellControl, "{err}");
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
        assert_eq!(err.what, UiReason::TooManyButtons, "{err}");
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
        assert_eq!(
            err.what,
            UiReason::SameAction {
                id: "compact".into()
            },
            "{err}"
        );
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
        assert_eq!(err.what, UiReason::MoveToRange { len: 4 }, "{err}");
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
        assert_eq!(
            err.what,
            UiReason::ListDuplicate {
                key: "band.segments"
            },
            "{err}"
        );
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
        assert_eq!(
            err.what,
            UiReason::EmptyList {
                key: "usage_bar.items"
            },
            "{err}"
        );
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
        // 問題の文言に利用者の値（ここでは "sometimes" / "x"）を入れない（日英とも）
        for p in &parsed.problems {
            for lang in [crate::i18n::Lang::Ja, crate::i18n::Lang::En] {
                let p = p.text_in(lang);
                assert!(
                    !p.contains("sometimes") && !p.contains("abcdefghijklmnopq"),
                    "{p}"
                );
            }
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
        let ja = err.text_in(crate::i18n::Lang::Ja);
        assert!(ja.contains("書き換えない"), "{ja}");
        let en = err.text_in(crate::i18n::Lang::En);
        assert!(en.contains("does not rewrite"), "{en}");
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

    /// #2238: UTF-8 でない中身はバイトのまま番号付きの退避へ移す。別の中身の 2 回目も残り
    /// （以前は 1 本目が既にあると写さず、変更の操作の書き込みが消していた）、同じ中身は積まない
    #[test]
    fn utf8でない中身はバイトのまま番号付きの退避へ移す() {
        if crate::load_issue::legacy_2238() {
            return;
        }
        let dir = tmp_dir("nonutf8");
        assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
        let path = dir.join(FILENAME);
        let slot = |n| crate::migration::quarantine_slot_path(&path, n);
        let one: &[u8] = b"\xff\xfe MARK_ONE \xff";
        let two: &[u8] = b"\xfe\xff MARK_TWO \x80";

        std::fs::write(&path, one).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.config, UiConfig::default());
        let LoadState::Repaired {
            problems,
            quarantine: Some(q),
        } = &loaded.state
        else {
            panic!("{:?}", loaded.state)
        };
        assert_eq!(problems, &[UiProblem::NotUtf8]);
        assert_eq!(
            (q.path.clone(), q.copied, q.evicted),
            (slot(1), true, false)
        );
        assert!(!path.exists(), "表から外す（次の書き込みが消さない）");
        assert_eq!(std::fs::read(slot(1)).unwrap(), one);

        // 別の中身の 2 回目は変更の操作の中で移してから既定の形を書く
        std::fs::write(&path, two).unwrap();
        let up = update_at(&path, &[UiOp::Reset], 5).unwrap();
        assert!(up.written);
        assert_eq!(up.before.code(), "repaired");
        assert_eq!(std::fs::read(slot(1)).unwrap(), one, "1 回目を上書きしない");
        assert_eq!(std::fs::read(slot(2)).unwrap(), two);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            UiConfig::default().to_text()
        );

        // 同じ中身の 3 回目は積まない（元を外すだけ）
        std::fs::write(&path, two).unwrap();
        let loaded = load_from(&path);
        let LoadState::Repaired {
            quarantine: Some(q),
            ..
        } = &loaded.state
        else {
            panic!("{:?}", loaded.state)
        };
        assert_eq!((q.path.clone(), q.copied), (slot(2), false));
        assert!(!slot(3).exists() && !path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #2238: 権限で読めない ui.json も移す（以前は ReadError のまま変更の操作が上書きしていた）
    #[cfg(unix)]
    #[test]
    fn 権限で読めない中身もバイトのまま移す() {
        use std::os::unix::fs::PermissionsExt;
        if crate::load_issue::legacy_2238() {
            return;
        }
        let dir = tmp_dir("perm");
        assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
        let path = dir.join(FILENAME);
        std::fs::write(&path, "{\"schema_version\": 1}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
        if std::fs::read(&path).is_ok() {
            // root では権限で読めなくならない
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let loaded = load_from(&path);
        let LoadState::Repaired {
            problems,
            quarantine: Some(q),
        } = &loaded.state
        else {
            panic!("{:?}", loaded.state)
        };
        assert_eq!(
            problems,
            &[UiProblem::ReadIo {
                kind: "permission denied".into()
            }]
        );
        assert_eq!(
            problems[0].text_in(crate::i18n::Lang::Ja),
            "読み取りに失敗（permission denied）"
        );
        assert!(q.copied && !path.exists());
        std::fs::set_permissions(&q.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&q.path).unwrap(),
            "{\"schema_version\": 1}"
        );
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
        assert_eq!(
            bad.to_ops().unwrap_err().allowed,
            Allowed::words(Preset::words())
        );
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

#[cfg(test)]
mod issue1962_tests {
    use super::*;

    /// #1962: tako の操作 9 つすべてが CLI の引数になり、対象のペインを名指しする
    /// （実際に CLI が受け付けるかは tako-cli の `issue1962_ボタンの引数はcliが受け付ける`）
    #[test]
    fn issue1962_takoの操作は対象のペインを名指しするcliの引数になる() {
        for op in TakoOp::ALL {
            let args = op
                .cli_args(7, Some("/w"))
                .unwrap_or_else(|| panic!("{op:?} の引数が組めない"));
            if *op == TakoOp::OpenCwd {
                assert_eq!(args, ["file", "open", "/w"]);
            } else {
                let at = args
                    .iter()
                    .position(|a| a == "--pane")
                    .unwrap_or_else(|| panic!("{op:?} が --pane を名指ししない: {args:?}"));
                assert_eq!(args[at + 1], "7");
            }
        }
        // cwd が分からなければフォルダを開くボタンは描かない
        assert_eq!(TakoOp::OpenCwd.cli_args(7, None), None);
        assert_eq!(TakoOp::OpenCwd.cli_args(7, Some("")), None);
    }

    /// #1962: shell は新しいペインで実行する（`--` の後ろに 1 引数のまま = `-` で始まっても割れない）
    #[test]
    fn issue1962_shellのボタンは新しいペインで実行する引数になる() {
        let args = shell_cli_args(3, "-n echo 'a b'");
        assert_eq!(args[0], "run-interactive");
        assert_eq!(args.last().map(String::as_str), Some("-n echo 'a b'"));
        assert_eq!(args[args.len() - 2], "--");
        assert!(args.windows(2).any(|w| w == ["--pane", "3"]));
        assert!(args.windows(2).any(|w| w == ["--auto-close", "never"]));
    }

    /// #1962: 応答の button_args は tako / shell のボタンだけ（slash / prompt は Claude Code の中で済む）
    #[test]
    fn issue1962_button_argsはtakoとshellだけ() {
        let mut buttons = default_buttons();
        for spec in [
            ("tako", "split-down"),
            ("shell", "make test"),
            ("prompt", "続けて"),
            ("tako", "open-cwd"),
        ] {
            let b = build_button(
                &ButtonSpec {
                    kind: Some(spec.0.into()),
                    value: spec.1.into(),
                    ..ButtonSpec::default()
                },
                &buttons,
            )
            .unwrap();
            buttons.push(b);
        }
        let ids: Vec<String> = buttons.iter().map(|b| b.id.clone()).collect();
        let args = button_args(&buttons, 9, Some("/w"));
        assert!(!args.contains_key(&ids[0]), "slash は載せない");
        assert_eq!(args[&ids[1]], ["split", "--pane", "9", "--down"]);
        assert_eq!(args[&ids[2]].last().map(String::as_str), Some("make test"));
        assert!(!args.contains_key(&ids[3]), "prompt は載せない");
        assert_eq!(args[&ids[4]], ["file", "open", "/w"]);
        // cwd が分からなければ open-cwd は落ちる（mod はそのボタンを描かない）
        assert!(!button_args(&buttons, 9, None).contains_key(&ids[4]));
    }
}

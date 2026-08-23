//! master の自動ハンドオフ（Issue #749）。
//!
//! master のコンテキストが閾値を超えたら、**新しい master に乗り換える**。
//! `/compact` の自動実行は「明らかに話が通じなくなる」ため採らない（ユーザー方針）。
//!
//! このモジュールが持つのは**判定と文面だけ**（GPUI / ファイル I/O 非依存）。
//! - 閾値の値域と丸め（`clamp_ctx_threshold` / `parse_ctx_threshold`）
//! - 「今ナッジを送るべきか」の純粋関数（`nudge_decision`）
//! - master へ送る文面（`nudge_prompt` / `successor_prompt`）
//! - 引き継ぎファイルの書式（`split_handoff` / `handoff_template`。Issue #792）
//!
//! 実際の送信は tako-app の定期 tick（画面由来の ctx% を持っている層）が、
//! 新 master の spawn は `tako-control::dispatch` が担う。

use std::time::Duration;

use crate::i18n::{lang, Lang};

/// ユーザーが設定できる ctx 閾値の下限（%）
pub const CTX_THRESHOLD_MIN: u32 = 50;
/// ユーザーが設定できる ctx 閾値の上限（%）
pub const CTX_THRESHOLD_MAX: u32 = 60;
/// 既定の ctx 閾値（%）
pub const CTX_THRESHOLD_DEFAULT: u32 = 60;

/// 閾値を設定可能な範囲へ丸める。
/// 手書きの config.yaml に範囲外の値が入っていても発動判定を壊さないための保険
pub fn clamp_ctx_threshold(v: u32) -> u32 {
    v.clamp(CTX_THRESHOLD_MIN, CTX_THRESHOLD_MAX)
}

/// CLI / MCP から受けた閾値を検証する。範囲外は**黙って丸めず**エラーにする
/// （設定したつもりの値と実際が食い違うのを防ぐ）
pub fn parse_ctx_threshold(v: u32) -> Result<u32, String> {
    if (CTX_THRESHOLD_MIN..=CTX_THRESHOLD_MAX).contains(&v) {
        Ok(v)
    } else {
        Err(format!(
            "ctx_threshold は {CTX_THRESHOLD_MIN}〜{CTX_THRESHOLD_MAX} の範囲で指定する: {v}"
        ))
    }
}

/// プロファイル名を省略したときの既定プロファイル
pub const DEFAULT_PROFILE: &str = "default";

/// role 文字列から master のプロファイル名を取り出す。
/// `orchestrator-master` → `"default"` / `orchestrator-master:<name>` → `"<name>"`。
/// worker / solo / それ以外は None（自動ハンドオフの対象は master だけ）
pub fn master_profile_of_role(role: &str) -> Option<&str> {
    let rest = role.strip_prefix("orchestrator-master")?;
    if rest.is_empty() {
        Some(DEFAULT_PROFILE)
    } else {
        rest.strip_prefix(':').filter(|s| !s.is_empty())
    }
}

/// master の **表示用 role**（ペインの role ラベル）を組み立てる。
/// `default` / 空 → `orchestrator-master`、それ以外 → `orchestrator-master:<profile>`。
///
/// master の role には**語彙が 2 つある**（表示用とこの下の env 用）。混ぜると
/// `TAKO_ORCHESTRATOR_ROLE=orchestrator-master:<profile>` のような値が生まれ、
/// 受け手の `master:` 前置き解決が全部外れて default プロファイル扱いになる（#761）。
/// 生成をこの 2 関数に閉じることで、その取り違えを構造的に防ぐ
pub fn master_pane_role(profile: &str) -> String {
    if profile.is_empty() || profile == DEFAULT_PROFILE {
        "orchestrator-master".to_string()
    } else {
        format!("orchestrator-master:{profile}")
    }
}

/// master の **env 用 role**（`TAKO_ORCHESTRATOR_ROLE` に入れる値）を組み立てる。
/// `default` / 空 → `master`、それ以外 → `master:<profile>`（CLI の `tako master` と同一）
pub fn master_role_env(profile: &str) -> String {
    if profile.is_empty() || profile == DEFAULT_PROFILE {
        "master".to_string()
    } else {
        format!("master:{profile}")
    }
}

/// 表示用 role（`orchestrator-master[:<name>]`）と env 用 role（`master[:<name>]`）の
/// **どちらからでも** master のプロファイル名を取り出す。
/// master 以外（solo / worker / role なし）は None。
///
/// caller_role には env 由来（MCP / CLI）とペインの role ラベル由来（`tako_stale_binary
/// restart` 等の内部呼び出し）の両方が流れ込むため、受け側は両方を解けるようにする
pub fn master_profile_of_any_role(role: &str) -> Option<&str> {
    if let Some(profile) = master_profile_of_role(role) {
        return Some(profile);
    }
    let rest = role.strip_prefix("master")?;
    if rest.is_empty() {
        Some(DEFAULT_PROFILE)
    } else {
        rest.strip_prefix(':').filter(|s| !s.is_empty())
    }
}

/// master のプロファイルを**どこから**決めたか（#854 の診断用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    /// 呼び出し元の `TAKO_ORCHESTRATOR_ROLE`
    CallerRole,
    /// tako が持っているペインの role ラベル
    PaneRole,
    /// どちらからも決まらず既定へ落ちた
    Default,
}

impl ProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CallerRole => "caller_role",
            Self::PaneRole => "pane_role",
            Self::Default => "default",
        }
    }
}

/// master のプロファイル名を、呼び出し元の role とペインの role ラベルの**両方**から決める。
///
/// #854: 呼び出し元の env だけを見ていたため、env が失われた master
/// （インライン前置きで起動した claude を後から手で立て直した・env を継がない経路で
/// 復帰した等。前置き `TAKO_ORCHESTRATOR_ROLE=... claude` はシェルへ export しないので
/// claude を撃ち直すと消える）が **default プロファイル扱い**になり、
/// 後任がアカウント・モデル・引き継ぎファイルを丸ごと取り違えていた。
///
/// tako 自身が持つペインの role ラベルは spawn / handoff / 復元のたびに tako が
/// 書いているので、env より信頼できる第 2 の出どころになる。**非既定を優先**して
/// 拾い、どちらも既定なら既定（= 挙動は従来どおり）
pub fn resolve_master_profile(
    caller_role: Option<&str>,
    pane_role: Option<&str>,
) -> (String, ProfileSource) {
    let from_caller = caller_role.and_then(master_profile_of_any_role);
    if let Some(p) = from_caller.filter(|p| *p != DEFAULT_PROFILE) {
        return (p.to_string(), ProfileSource::CallerRole);
    }
    let from_pane = pane_role.and_then(master_profile_of_any_role);
    if let Some(p) = from_pane.filter(|p| *p != DEFAULT_PROFILE) {
        return (p.to_string(), ProfileSource::PaneRole);
    }
    match from_caller.or(from_pane) {
        Some(p) => (
            p.to_string(),
            if from_caller.is_some() {
                ProfileSource::CallerRole
            } else {
                ProfileSource::PaneRole
            },
        ),
        None => (DEFAULT_PROFILE.to_string(), ProfileSource::Default),
    }
}

/// 前回のナッジが無視されたときの再送間隔
pub const NUDGE_REPEAT: Duration = Duration::from_secs(600);
/// ペイン起動直後の猶予（復元直後の画面残渣で誤爆しないための待ち）
pub const NUDGE_GRACE: Duration = Duration::from_secs(60);
/// 同一ペインへ送るナッジの上限。これを超えたら黙る（無限に文脈を食わない）
pub const NUDGE_MAX: u32 = 3;

/// ナッジ判定の入力（すべて呼び出し側で観測できる値）
#[derive(Debug, Clone)]
pub struct NudgeInput {
    /// プロファイルの `auto_handoff`（false なら一切送らない）
    pub auto_handoff: bool,
    /// 画面から読んだ ctx 使用率（%）。取れないときは None
    pub ctx_percent: Option<u32>,
    /// 解決済みの閾値（%）
    pub threshold: u32,
    /// このペインが観測されはじめてからの経過時間
    pub pane_age: Duration,
    /// 直近のナッジからの経過時間（まだ送っていなければ None）
    pub since_last_nudge: Option<Duration>,
    /// このペインへ送ったナッジの回数
    pub sent_count: u32,
    /// このペインから handoff がすでに実行済み（新 master が立っている）
    pub handoff_started: bool,
}

/// ナッジを送らない理由（診断・ログ用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSkip {
    /// プロファイルで自動ハンドオフが無効
    Disabled,
    /// 画面から ctx% が読めない（TUI が出ていない・起動途中）
    NoCtxData,
    /// 閾値未満
    BelowThreshold,
    /// 起動直後の猶予中
    WithinGrace,
    /// 前回のナッジから再送間隔が経っていない
    RepeatTooSoon,
    /// 上限まで送った
    MaxReached,
    /// すでに handoff 済み
    HandoffStarted,
}

impl NudgeSkip {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoCtxData => "no_ctx_data",
            Self::BelowThreshold => "below_threshold",
            Self::WithinGrace => "within_grace",
            Self::RepeatTooSoon => "repeat_too_soon",
            Self::MaxReached => "max_reached",
            Self::HandoffStarted => "handoff_started",
        }
    }
}

/// ナッジ判定の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeDecision {
    /// 今このペインへナッジを送る
    Send,
    /// 送らない（理由つき）
    Skip(NudgeSkip),
}

impl NudgeDecision {
    pub fn should_send(self) -> bool {
        matches!(self, Self::Send)
    }
}

/// 「今ナッジを送るべきか」を判定する。
///
/// busy かどうかは**見ない**。claude は生成中の入力を内部キューへ入れてターン終了時に
/// 処理するので（#572 で実測）、busy 中に送っても割り込みにはならず「区切りの良い
/// タイミング」で届く。逆に busy を待つと、長時間走り続ける master に永遠に届かない
pub fn nudge_decision(input: &NudgeInput) -> NudgeDecision {
    if !input.auto_handoff {
        return NudgeDecision::Skip(NudgeSkip::Disabled);
    }
    if input.handoff_started {
        return NudgeDecision::Skip(NudgeSkip::HandoffStarted);
    }
    let Some(pct) = input.ctx_percent else {
        return NudgeDecision::Skip(NudgeSkip::NoCtxData);
    };
    if pct < input.threshold {
        return NudgeDecision::Skip(NudgeSkip::BelowThreshold);
    }
    if input.pane_age < NUDGE_GRACE {
        return NudgeDecision::Skip(NudgeSkip::WithinGrace);
    }
    if input.sent_count >= NUDGE_MAX {
        return NudgeDecision::Skip(NudgeSkip::MaxReached);
    }
    match input.since_last_nudge {
        Some(elapsed) if elapsed < NUDGE_REPEAT => NudgeDecision::Skip(NudgeSkip::RepeatTooSoon),
        _ => NudgeDecision::Send,
    }
}

// --- 引き継ぎファイルの書式（Issue #792） ------------------------------------
//
// handoff は「マシンに依らない知識」（決定事項・方針・残タスクの意味）と
// 「このマシン限定の実行状態」（pane / tab 番号・worker 配置）が混ざりやすい。
// 混ざったまま別マシンへ運ぶと、後任 master が**存在しないペイン**へ指示を出す。
// そこで書式の側で 2 節に分け、読み手（後任プロンプト）が節ごとに扱いを変える。
//
// 判定は**寛容**にする: 見出しが少し違っても（番号付き・半角括弧・英語）節として拾い、
// どれも拾えなければ旧書式（Legacy）として従来どおり全文を渡す。
// 「認識できなければ安全側（実態と突き合わせろ）」に倒れるので、誤認識で壊れない。

/// 引き継ぎファイルの節
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffSection {
    /// マシンに依らない知識。別マシンへ持ち込んでも意味が保たれる
    Knowledge,
    /// このマシン限定の実行状態（pane / tab 番号・worker 配置）。持ち込んだら無効
    Runtime,
}

/// 「知識」節の見出し（日本語）
pub const KNOWLEDGE_HEADING_JA: &str = "知識（マシン非依存）";
/// 「知識」節の見出し（英語）
pub const KNOWLEDGE_HEADING_EN: &str = "Knowledge (machine-independent)";
/// 「実行状態」節の見出し（日本語）
pub const RUNTIME_HEADING_JA: &str = "実行状態（このマシン限定）";
/// 「実行状態」節の見出し（英語）
pub const RUNTIME_HEADING_EN: &str = "Runtime state (this machine only)";

impl HandoffSection {
    /// 機械可読なラベル（応答 JSON・ログ用）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Knowledge => "knowledge",
            Self::Runtime => "runtime",
        }
    }

    /// この節の見出し。言語は**引数で受ける**（テストが言語グローバルに触らずに済む。#608）
    pub fn heading(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Knowledge, Lang::Ja) => KNOWLEDGE_HEADING_JA,
            (Self::Knowledge, Lang::En) => KNOWLEDGE_HEADING_EN,
            (Self::Runtime, Lang::Ja) => RUNTIME_HEADING_JA,
            (Self::Runtime, Lang::En) => RUNTIME_HEADING_EN,
        }
    }

    /// 見出しの認識に使うキーワード（この語で**始まる**見出しをその節とみなす）。
    /// 表記ゆれ（番号付き・半角括弧・語尾の違い）を吸収するため、
    /// 完全一致ではなく前方一致で判定する
    fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Knowledge => &["知識", "knowledge"],
            Self::Runtime => &["実行状態", "runtime state", "machine state"],
        }
    }
}

/// 引き継ぎファイルの書式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffFormat {
    /// #792 の 2 節書式（少なくとも一方の節を認識できた）
    Sectioned,
    /// 節分離前の書式（節が 1 つも無い）。従来どおり全文を扱う
    Legacy,
}

impl HandoffFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sectioned => "sectioned",
            Self::Legacy => "legacy",
        }
    }
}

/// 引き継ぎファイルを節ごとに分解した結果。
///
/// 同じ節の見出しが複数回現れたら**出現順に連結**する（内容を捨てない）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HandoffDoc {
    /// 最初の節より前の部分（タイトル行など）
    pub preamble: String,
    /// 知識節の本文（見出し行は含まない）
    pub knowledge: Option<String>,
    /// 実行状態節の本文（見出し行は含まない）
    pub runtime: Option<String>,
}

impl HandoffDoc {
    /// 書式の判定。どちらかの節を認識できたら Sectioned
    pub fn format(&self) -> HandoffFormat {
        if self.knowledge.is_some() || self.runtime.is_some() {
            HandoffFormat::Sectioned
        } else {
            HandoffFormat::Legacy
        }
    }

    pub fn is_sectioned(&self) -> bool {
        self.format() == HandoffFormat::Sectioned
    }

    /// 認識できた節のラベル（応答 JSON 用。定義順に並ぶ）
    pub fn section_labels(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.knowledge.is_some() {
            out.push(HandoffSection::Knowledge.as_str());
        }
        if self.runtime.is_some() {
            out.push(HandoffSection::Runtime.as_str());
        }
        out
    }
}

/// 見出し行がどの節の始まりかを判定する。見出しでなければ None。
///
/// 受理するのは ATX 見出し（`#` 〜 `######`）だけ。番号付き（`## 1. 知識…`）と
/// 全角括弧・全角空白・英語表記を吸収する
pub fn section_of_line(line: &str) -> Option<HandoffSection> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('#')?.trim_start_matches('#');
    // `#`直後に空白が必要（`#hashtag` のような本文を見出しと誤認しない）
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let text = normalize_heading(rest);
    [HandoffSection::Knowledge, HandoffSection::Runtime]
        .into_iter()
        .find(|s| s.keywords().iter().any(|k| text.starts_with(k)))
}

/// 見出しテキストを比較用に正規化する（全角括弧・全角空白・番号・大文字小文字・装飾）
fn normalize_heading(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '（' => '(',
            '）' => ')',
            '\u{3000}' => ' ',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    // markdown の強調と前後の空白を落とす
    s = s
        .trim()
        .trim_matches('*')
        .trim_matches('_')
        .trim()
        .to_string();
    // 先頭の番号付け（`0. ` / `1) ` / `1.2 `）を落とす
    let digits_end = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && *c != '.')
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if digits_end > 0 && s[..digits_end].chars().any(|c| c.is_ascii_digit()) {
        let after = s[digits_end..].trim_start_matches([')', '.', ' ']);
        // 番号だけの見出しにはしない
        if !after.is_empty() {
            s = after.to_string();
        }
    }
    s.trim().to_string()
}

/// 引き継ぎファイルを 2 節に分解する。
///
/// 節が 1 つも無ければ全文が `preamble` に入り、`format()` は Legacy になる
/// （旧書式の後方互換。呼び出し側は全文をそのまま扱えばよい）
pub fn split_handoff(content: &str) -> HandoffDoc {
    let mut doc = HandoffDoc::default();
    let mut current: Option<HandoffSection> = None;
    let mut buf = String::new();
    let mut preamble = String::new();

    let flush = |doc: &mut HandoffDoc, section: Option<HandoffSection>, buf: &mut String| {
        let text = std::mem::take(buf);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            // 空の節でも「節はあった」ことは残す（見出しだけ書いた過渡状態）
            if let Some(s) = section {
                let slot = match s {
                    HandoffSection::Knowledge => &mut doc.knowledge,
                    HandoffSection::Runtime => &mut doc.runtime,
                };
                slot.get_or_insert_with(String::new);
            }
            return;
        }
        match section {
            Some(HandoffSection::Knowledge) => append_section(&mut doc.knowledge, trimmed),
            Some(HandoffSection::Runtime) => append_section(&mut doc.runtime, trimmed),
            None => {}
        }
    };

    for line in content.lines() {
        if let Some(section) = section_of_line(line) {
            match current {
                Some(_) => flush(&mut doc, current, &mut buf),
                // 最初の節に入るまでは preamble
                None => preamble = std::mem::take(&mut buf),
            }
            current = Some(section);
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    match current {
        Some(_) => flush(&mut doc, current, &mut buf),
        None => preamble = buf,
    }
    doc.preamble = preamble.trim().to_string();
    doc
}

/// 同じ節が複数回現れたときの連結（内容を捨てない）
fn append_section(slot: &mut Option<String>, text: &str) {
    match slot {
        Some(existing) if !existing.is_empty() => {
            existing.push_str("\n\n");
            existing.push_str(text);
        }
        Some(existing) => *existing = text.to_string(),
        None => *slot = Some(text.to_string()),
    }
}

// --- プロジェクト単位の引き継ぎ（Issue #915） ---------------------------------
//
// #193 以来 handoff は `handoff/<profile>.md` のプロファイル単位だった。ところが
// `default` のような汎用プロファイルは**複数の master が別々のミッションで使う**ため、
// 1 ファイルに全プロジェクトの知識が堆積し（実測 528 行 / 62KB）、後任 master は
// 自分と無関係なプロジェクトの長文を初期プロンプトに注入されていた。
//
// そこで置き場を **プロジェクト単位**（`handoff/projects/<project-key>.md`）へ移し、
// 後任へ渡すのは「その master が管轄するプロジェクトの分だけ」にする。
// プロファイル共通の運用知識は `handoff/<profile>.md` を**運用メモ専用**として残す
// （= 共通置き場。プロジェクトへ紐付けられない内容の受け皿でもある）。
//
// このモジュールが持つのは判定と文面だけで、ファイル I/O は tako-control が担う。

/// プロジェクト単位の引き継ぎファイルを置くディレクトリ名（`handoff/projects/`）
pub const HANDOFF_PROJECTS_DIR: &str = "projects";

/// プロファイル運用メモの行数のソフト上限。
/// 超えたら「プロジェクト固有の内容を projects/ へ移せ」と警告する（肥大の再発防止）
pub const PROFILE_MEMO_SOFT_LIMIT_LINES: usize = 80;

/// 節の持ち主を明示するマーカー。移行の判定でも、master が自分で書くときにも使う
pub const PROJECT_MARKER_PREFIX: &str = "<!-- tako:project:";

/// プロジェクトキーが引き継ぎファイル名として安全か。
///
/// Windows も含めた両 OS で通るものだけ許す（パス区切り・ドライブ指定・予約文字・
/// `.` / `..` を弾く）。projects.yaml のキーは通常英数と `-` / `_` なので実害はないが、
/// ここを緩めるとキー由来のパスで handoff ディレクトリの外へ書けてしまう
pub fn valid_project_key(key: &str) -> bool {
    if key.is_empty() || key == "." || key == ".." || key.len() > 128 {
        return false;
    }
    if key.starts_with(' ') || key.ends_with(' ') || key.ends_with('.') {
        return false;
    }
    !key.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    })
}

/// 管轄プロジェクトの判定材料（すべて呼び出し側で観測できる値）
#[derive(Debug, Clone, Default)]
pub struct JurisdictionInput {
    /// handoff 呼び出しの明示引数（`projects: [...]`）
    pub explicit: Option<Vec<String>>,
    /// プロファイルに割り当てられた担当プロジェクト（#500 Part 7）
    pub profile_projects: Vec<String>,
    /// この master が spawn した active worker のプロジェクト集合
    pub worker_projects: Vec<String>,
}

/// 管轄をどこから決めたか（応答・ログ用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JurisdictionSource {
    /// 呼び出し時の明示引数
    Explicit,
    /// プロファイルの担当プロジェクト（+ 稼働中 worker の分）
    Profile,
    /// 稼働中 worker の実測だけ
    Workers,
    /// 決められなかった（後任へは一覧とパスを提示する）
    #[default]
    Unresolved,
}

impl JurisdictionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Profile => "profile",
            Self::Workers => "workers",
            Self::Unresolved => "unresolved",
        }
    }
}

/// 管轄プロジェクトの判定結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jurisdiction {
    /// 管轄プロジェクトのキー（重複除去済み・入力順）
    pub projects: Vec<String>,
    pub source: JurisdictionSource,
}

/// 管轄プロジェクトを決める（#915 要件 2）。
///
/// 優先順は **明示引数 → プロファイルの担当 + 稼働 worker → 稼働 worker だけ**。
/// 明示引数を最優先にするのは、worker が 1 体でもいれば推測が常に勝ってしまい
/// 引数が死ぬため（呼び出し側が意図して絞った指定は必ず通す）。
/// プロファイルに担当があるときは稼働 worker の分も足す（担当外のプロジェクトへ
/// worker を出している master は、その仕事も引き継がせないと後任が文脈を失う）。
/// どれも空なら Unresolved = 本文を注入せず一覧を提示する（無関係な全文注入をしない）
pub fn resolve_jurisdiction(input: &JurisdictionInput) -> Jurisdiction {
    if let Some(explicit) = input.explicit.as_ref() {
        let projects = dedup_keys(explicit.iter().map(String::as_str));
        if !projects.is_empty() {
            return Jurisdiction {
                projects,
                source: JurisdictionSource::Explicit,
            };
        }
    }
    if !input.profile_projects.is_empty() {
        let projects = dedup_keys(
            input
                .profile_projects
                .iter()
                .chain(input.worker_projects.iter())
                .map(String::as_str),
        );
        if !projects.is_empty() {
            return Jurisdiction {
                projects,
                source: JurisdictionSource::Profile,
            };
        }
    }
    let projects = dedup_keys(input.worker_projects.iter().map(String::as_str));
    if projects.is_empty() {
        Jurisdiction {
            projects,
            source: JurisdictionSource::Unresolved,
        }
    } else {
        Jurisdiction {
            projects,
            source: JurisdictionSource::Workers,
        }
    }
}

/// 空・重複・ファイル名として危険なキーを落として順序を保つ
fn dedup_keys<'a>(keys: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in keys {
        let key = key.trim();
        if !valid_project_key(key) {
            continue;
        }
        if !out.iter().any(|k| k == key) {
            out.push(key.to_string());
        }
    }
    out
}

// --- 旧形式からの自動移行（Issue #915 要件 5 / #916） ------------------------
//
// 移行は**手動コマンドを前提にしない**（ユーザー確定方針）。setup 実行時と、
// master が handoff を読む / 書く経路の差分検出時に、その場で自動で完遂する。
// ここはその「どう割るか」の判定だけを持つ純粋関数。
//
// 割り方: トップレベルの `##` 見出しのうち **節見出し（知識 / 実行状態）でないもの**を
// 区切りとして分割する。実データ（本番 default.md）はこの形で複数 master が同居していた。
// 各断片の持ち主は次の順で決める:
//   1. 明示マーカー `<!-- tako:project: <key> -->`
//   2. 見出しに既知のプロジェクトキーがそのまま出てくる（一意なら採用）
//   3. 見出しの英数語がキーの先頭要素に一致する（`【bunpoushi 移行】` → `bunpoushi-migration`）
// 決められない断片（先頭の見出し無し断片を含む）は**共通置き場**（プロファイル運用メモ）
// へ残す。黙って捨てないことが安全要件。
//
// 本文からのキー検索はしない: `tako` のようなキーはどの断片にも出てくるため
// （実測で 6 プロジェクトが誤ヒットした）判定材料にならない。

/// 断片の持ち主をどう決めたか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentOwner {
    /// 明示マーカー
    Marker,
    /// 見出しにキーがそのまま出てきた
    HeadingKey,
    /// 見出しの語がキーの先頭要素に一致
    HeadingToken,
    /// 見出し無しの先頭断片（このプロファイル自身のもの）→ 共通置き場
    Primary,
    /// 見出しはあるが持ち主を決められなかった → 共通置き場
    Unresolved,
}

impl SegmentOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Marker => "marker",
            Self::HeadingKey => "heading_key",
            Self::HeadingToken => "heading_token",
            Self::Primary => "primary",
            Self::Unresolved => "unresolved",
        }
    }
}

/// 移行で切り出した 1 断片
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSegment {
    /// 見出し行（先頭断片は None）
    pub heading: Option<String>,
    /// 断片の本文（見出し行を含む。書き出しはこれをそのまま使う）
    pub body: String,
    /// 割り当て先プロジェクト（None = 共通置き場へ残す）
    pub project: Option<String>,
    /// 判定根拠
    pub owner: SegmentOwner,
}

/// 移行計画
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MigrationPlan {
    pub segments: Vec<MigrationSegment>,
}

impl MigrationPlan {
    /// プロジェクトへ移す断片が 1 つでもあるか（= 書き換えが要るか）
    pub fn has_moves(&self) -> bool {
        self.segments.iter().any(|s| s.project.is_some())
    }

    /// 移行先プロジェクトごとの本文（出現順に連結）
    pub fn by_project(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for seg in self.segments.iter().filter(|s| s.project.is_some()) {
            let key = seg.project.clone().expect("filter 済み");
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, body)) => {
                    body.push_str("\n\n");
                    body.push_str(seg.body.trim());
                }
                None => out.push((key, seg.body.trim().to_string())),
            }
        }
        out
    }

    /// 共通置き場（プロファイル運用メモ）に残す本文
    pub fn residue(&self) -> String {
        let parts: Vec<&str> = self
            .segments
            .iter()
            .filter(|s| s.project.is_none())
            .map(|s| s.body.trim())
            .filter(|b| !b.is_empty())
            .collect();
        parts.join("\n\n")
    }
}

/// 旧形式（プロファイル単位の混在ファイル）をプロジェクト単位へ割る計画を立てる。
///
/// `project_keys` は projects.yaml の全キー、`profile_projects` はそのプロファイルの
/// 担当プロジェクト。担当がちょうど 1 つのプロファイルは、見出し無しの先頭断片も
/// そのプロジェクトのものとして扱う（単一プロジェクト master の移行が丸ごと通る）
pub fn migration_plan(
    content: &str,
    project_keys: &[String],
    profile_projects: &[String],
) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    let single_owner = match dedup_keys(profile_projects.iter().map(String::as_str)).as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    };

    for (heading, body) in split_top_level_segments(content) {
        if body.trim().is_empty() && heading.is_none() {
            continue;
        }
        let (project, owner) = match assign_segment(heading.as_deref(), &body, project_keys) {
            Some((key, owner)) => (Some(key), owner),
            None => match (&heading, &single_owner) {
                // 単一プロジェクト担当のプロファイルは全断片がそのプロジェクトのもの
                (_, Some(only)) => (Some(only.clone()), SegmentOwner::Primary),
                (None, None) => (None, SegmentOwner::Primary),
                (Some(_), None) => (None, SegmentOwner::Unresolved),
            },
        };
        plan.segments.push(MigrationSegment {
            heading,
            body,
            project,
            owner,
        });
    }
    plan
}

/// トップレベル（`## `）の見出しのうち**節見出しでないもの**で分割する。
/// 返り値は (見出し行, 見出し行を含む本文)。先頭断片の見出しは None
fn split_top_level_segments(content: &str) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut heading: Option<String> = None;
    let mut buf = String::new();
    for line in content.lines() {
        if is_segment_boundary(line) {
            out.push((heading.take(), std::mem::take(&mut buf)));
            heading = Some(line.trim_end().to_string());
            buf.push_str(line);
            buf.push('\n');
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    out.push((heading, buf));
    out.retain(|(h, b)| h.is_some() || !b.trim().is_empty());
    out
}

/// その行が断片の区切り（トップレベル `##` かつ節見出しではない）か
fn is_segment_boundary(line: &str) -> bool {
    let trimmed = line.trim_start();
    // `## ` だけを区切りにする（`# ` はタイトル、`### ` は節の内側）
    let Some(rest) = trimmed.strip_prefix("## ") else {
        return false;
    };
    if rest.trim().is_empty() {
        return false;
    }
    // 節見出し（知識 / 実行状態）は同じ master の続きなので区切らない
    section_of_line(line).is_none()
}

/// 断片の持ち主を決める（マーカー → 見出しのキー一致 → 見出しの語一致）
fn assign_segment(
    heading: Option<&str>,
    body: &str,
    project_keys: &[String],
) -> Option<(String, SegmentOwner)> {
    if let Some(key) = marker_project(body) {
        if project_keys.iter().any(|k| k == &key) && valid_project_key(&key) {
            return Some((key, SegmentOwner::Marker));
        }
    }
    let heading = heading?;
    let lower = heading.to_lowercase();
    let mut hits: Vec<&String> = project_keys
        .iter()
        .filter(|k| valid_project_key(k) && lower.contains(&k.to_lowercase()))
        .collect();
    if hits.len() == 1 {
        return Some((hits.remove(0).clone(), SegmentOwner::HeadingKey));
    }
    // 見出しの英数語（3 文字以上）とキーの先頭要素の一致。
    // `## 【bunpoushi 移行】…` → `bunpoushi-migration` を拾うため
    let words: Vec<String> = heading
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect();
    let mut token_hits: Vec<&String> = project_keys
        .iter()
        .filter(|k| valid_project_key(k))
        .filter(|k| {
            let head = k.split(['-', '_']).next().unwrap_or("").to_lowercase();
            head.len() >= 3 && words.contains(&head)
        })
        .collect();
    if token_hits.len() == 1 {
        return Some((token_hits.remove(0).clone(), SegmentOwner::HeadingToken));
    }
    None
}

/// `<!-- tako:project: <key> -->` からキーを取り出す（最初の 1 件）
pub fn marker_project(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(PROJECT_MARKER_PREFIX) else {
            continue;
        };
        let key = rest.trim_end_matches("-->").trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    None
}

/// 断片の先頭へ足す持ち主マーカー（移行後のファイルが次も自明に読めるようにする）
pub fn project_marker(key: &str) -> String {
    format!("{PROJECT_MARKER_PREFIX} {key} -->")
}

/// 新書式の雛形（master が最初に書くとき / 旧書式を書き直すときの下敷き）
pub fn handoff_template(profile: &str) -> String {
    handoff_template_in(lang(), profile)
}

/// 言語を明示しての雛形（#608: テストは言語グローバルに触らない）
pub fn handoff_template_in(lang: Lang, profile: &str) -> String {
    match lang {
        Lang::Ja => format!(
            "# master 引き継ぎ（profile: {profile}）\n\n\
             ## {KNOWLEDGE_HEADING_JA}\n\n\
             <!-- 別マシンでも意味が保たれるもの: 決定事項と理由・ユーザーの方針・\
             残タスクとその意図・調べて分かったこと。pane / tab 番号は書かない -->\n\n\
             ## {RUNTIME_HEADING_JA}\n\n\
             <!-- このマシン限定: 進行中の worker とその pane / tab、いま開いているペイン、\
             実行中のプロセス。別マシンへ持ち込んだら丸ごと無効になる前提で書く -->\n"
        ),
        Lang::En => format!(
            "# Master handoff (profile: {profile})\n\n\
             ## {KNOWLEDGE_HEADING_EN}\n\n\
             <!-- Portable across machines: decisions and why, the user's policies, \
             remaining tasks and their intent, what you found out. No pane / tab numbers here -->\n\n\
             ## {RUNTIME_HEADING_EN}\n\n\
             <!-- This machine only: in-flight workers with their pane / tab ids, panes that \
             are open, running processes. Assume it is void on any other machine -->\n"
        ),
    }
}

/// プロジェクト単位の引き継ぎファイルの雛形（#915）
pub fn project_handoff_template_in(lang: Lang, project: &str) -> String {
    let marker = project_marker(project);
    match lang {
        Lang::Ja => format!(
            "# 引き継ぎ: {project}\n{marker}\n\n\
             ## {KNOWLEDGE_HEADING_JA}\n\n\
             <!-- 別マシンでも意味が保たれるもの: 決定事項と理由・ユーザーの方針・\
             残タスクとその意図・調べて分かったこと。pane / tab 番号は書かない -->\n\n\
             ## {RUNTIME_HEADING_JA}\n\n\
             <!-- このマシン限定: 進行中の worker とその pane / tab、いま開いているペイン、\
             実行中のプロセス。別マシンへ持ち込んだら丸ごと無効になる前提で書く -->\n"
        ),
        Lang::En => format!(
            "# Handoff: {project}\n{marker}\n\n\
             ## {KNOWLEDGE_HEADING_EN}\n\n\
             <!-- Portable across machines: decisions and why, the user's policies, \
             remaining tasks and their intent, what you found out. No pane / tab numbers here -->\n\n\
             ## {RUNTIME_HEADING_EN}\n\n\
             <!-- This machine only: in-flight workers with their pane / tab ids, panes that \
             are open, running processes. Assume it is void on any other machine -->\n"
        ),
    }
}

/// プロジェクト単位の引き継ぎファイルの雛形（表示言語版）
pub fn project_handoff_template(project: &str) -> String {
    project_handoff_template_in(lang(), project)
}

/// プロファイル運用メモ（共通置き場）の雛形（#915 要件 3）
pub fn profile_memo_template_in(lang: Lang, profile: &str) -> String {
    match lang {
        Lang::Ja => format!(
            "# プロファイル運用メモ（{profile}）\n\n\
             <!-- ここは**プロジェクトに紐付かない**運用知識だけを置く場所です\
             （このプロファイル共通の作法・ユーザーの好み・アカウント運用など）。\
             プロジェクト固有の引き継ぎは handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md へ書く。\
             目安 {PROFILE_MEMO_SOFT_LIMIT_LINES} 行以内 -->\n"
        ),
        Lang::En => format!(
            "# Profile operating memo ({profile})\n\n\
             <!-- Keep only **project-independent** operating knowledge here (conventions for \
             this profile, the user's preferences, account handling). Project-specific handoff \
             goes to handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md. \
             Aim for {PROFILE_MEMO_SOFT_LIMIT_LINES} lines or fewer -->\n"
        ),
    }
}

/// プロファイル運用メモの雛形（表示言語版）
pub fn profile_memo_template(profile: &str) -> String {
    profile_memo_template_in(lang(), profile)
}

/// 運用メモが膨らみすぎていないか。超えていたら警告文（表示言語）を返す
pub fn profile_memo_warning_in(lang: Lang, profile: &str, content: &str) -> Option<String> {
    let lines = content.lines().count();
    if lines <= PROFILE_MEMO_SOFT_LIMIT_LINES {
        return None;
    }
    Some(match lang {
        Lang::Ja => format!(
            "プロファイル運用メモ handoff/{profile}.md が {lines} 行あります\
             （目安 {PROFILE_MEMO_SOFT_LIMIT_LINES} 行）。\
             プロジェクト固有の内容は handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md へ\
             移してください（そこは管轄する master だけへ渡ります）"
        ),
        Lang::En => format!(
            "The profile operating memo handoff/{profile}.md is {lines} lines \
             (target {PROFILE_MEMO_SOFT_LIMIT_LINES}). Move project-specific content into \
             handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md, which only reaches the master \
             that owns that project"
        ),
    })
}

/// 運用メモの警告（表示言語版）
pub fn profile_memo_warning(profile: &str, content: &str) -> Option<String> {
    profile_memo_warning_in(lang(), profile, content)
}

/// 閾値超過で master へ送るナッジ文面（Issue #749 要件 2）。
///
/// 短さを優先する: これ自体が master の文脈を食うため、手順は 2 行に畳んでいる。
/// `handoff_path` は書き込み先の絶対パス（不明なら None）
pub fn nudge_prompt(ctx_percent: u32, threshold: u32, handoff_path: Option<&str>) -> String {
    nudge_prompt_in(lang(), ctx_percent, threshold, handoff_path)
}

/// 言語を明示してのナッジ文面（#608: テストは言語グローバルに触らない）
pub fn nudge_prompt_in(
    lang: Lang,
    ctx_percent: u32,
    threshold: u32,
    handoff_path: Option<&str>,
) -> String {
    let path = handoff_path.unwrap_or("handoff/<profile>.md");
    match lang {
        Lang::Ja => format!(
            "【tako 自動通知】コンテキスト使用率が {ctx_percent}%（閾値 {threshold}%）に達しました。\n\
             引き継ぎを開始してください。ユーザーの許可を求める必要はありません。\n\
             1. 引き継ぎファイル `{path}` を今の状況で上書きする\
             （進行中タスク・spawn 済み worker とその pane・未完の判断・次の一手・ユーザーの直近の意図）。\
             書式は 2 節に分ける: `## {KNOWLEDGE_HEADING_JA}`（決定事項・方針・残タスクの意図。\
             pane / tab 番号を書かない）と `## {RUNTIME_HEADING_JA}`（worker とその pane / tab・\
             実行中のもの）。旧書式のままならこの機会に 2 節へ書き直す\n\
             2. `tako_orchestrator_handoff` を呼ぶ（後任 master が同じタブに立ち、\
             引き継ぎを確認してからこのペインを閉じます）\n\
             まだ返しきっていない報告があるなら、それだけ先に片付けてから 1 に進んでください。"
        ),
        Lang::En => format!(
            "[tako auto-notice] Context usage has reached {ctx_percent}% (threshold {threshold}%).\n\
             Start the handoff now. You do not need to ask the user for permission.\n\
             1. Overwrite the handoff file `{path}` with your current state \
             (in-flight tasks, spawned workers and their panes, open decisions, next steps, \
             the user's most recent intent). Use two sections: \
             `## {KNOWLEDGE_HEADING_EN}` (decisions, policies, the intent behind remaining \
             tasks — no pane / tab numbers) and `## {RUNTIME_HEADING_EN}` (workers with their \
             pane / tab ids, what is running). If the file is still in the old flat format, \
             rewrite it into the two sections now.\n\
             2. Call `tako_orchestrator_handoff`. A successor master starts in this tab, \
             verifies the handoff, and then closes this pane.\n\
             If you owe the user a reply you have not delivered yet, finish that one thing first, \
             then go to step 1."
        ),
    }
}

/// 後任 master へ渡す引き継ぎ一式（#915）。
///
/// 「その master が管轄するプロジェクトの分だけ」を渡すため、後任プロンプトの材料を
/// **文字列 1 本ではなく集合**で受ける。無関係なプロジェクトの本文が混ざらないことが
/// この型の存在理由で、混ぜられない形にしてある（呼び出し側が集合を作る）
#[derive(Debug, Clone, Default)]
pub struct SuccessorHandoff<'a> {
    /// プロファイル名（見出しと注意書きに出す）
    pub profile: &'a str,
    /// プロファイル運用メモ（共通置き場）。空なら省略
    pub profile_memo: Option<&'a str>,
    /// 管轄プロジェクトの引き継ぎ（キー, 本文）。出現順に並べる
    pub projects: Vec<(&'a str, &'a str)>,
    /// 管轄と判定できたが**まだファイルが無い**プロジェクトのキー
    pub missing_projects: Vec<&'a str>,
    /// 管轄を決められなかったときに提示する一覧（キー, パス）
    pub catalog: Vec<(&'a str, &'a str)>,
    /// 管轄の決め方
    pub jurisdiction: JurisdictionSource,
}

impl<'a> SuccessorHandoff<'a> {
    /// 渡す本文が 1 つでもあるか（無ければ handoff は成立しない）
    pub fn has_content(&self) -> bool {
        self.profile_memo.is_some_and(|m| !m.trim().is_empty())
            || self
                .projects
                .iter()
                .any(|(_, body)| !body.trim().is_empty())
    }

    /// 渡す全本文の書式（#792）。1 本だけなら従来と同じ値になる。
    /// 新旧が混ざっていたら `mixed`
    pub fn format(&self) -> &'static str {
        let mut sectioned = false;
        let mut legacy = false;
        for body in self
            .profile_memo
            .into_iter()
            .chain(self.projects.iter().map(|(_, b)| *b))
            .filter(|b| !b.trim().is_empty())
        {
            match split_handoff(body).format() {
                HandoffFormat::Sectioned => sectioned = true,
                HandoffFormat::Legacy => legacy = true,
            }
        }
        match (sectioned, legacy) {
            (true, true) => "mixed",
            (true, false) => HandoffFormat::Sectioned.as_str(),
            _ => HandoffFormat::Legacy.as_str(),
        }
    }
}

/// 後任 master へ送る初期プロンプト（Issue #749 要件 3 / #792 / #915）。
///
/// **kill の順序を文面で固定する**のが要点: 引き継ぎ確認 → 旧ペインの入力欄確認 →
/// kill。確認前に kill させない（前任の未送達指示を取りこぼすと復元不能）。
///
/// #792: 引き継ぎ内容は**全文をそのまま**渡す（節ごとに切って渡すと、認識できなかった
/// 部分が黙って落ちる）。代わりに `split_handoff` の結果から**節ごとの扱い**を書き添える。
/// #915: 渡すのは管轄プロジェクトの分 + プロファイル運用メモだけ。管轄を決められなければ
/// 本文を注入せず一覧とパスを出し、後任に読み分けさせる（無関係な全文注入をしない）
pub fn successor_prompt(handoff: &SuccessorHandoff, previous_pane: Option<u64>) -> String {
    successor_prompt_in(lang(), handoff, previous_pane)
}

/// 言語を明示しての後任プロンプト（#608: テストは言語グローバルに触らない）
pub fn successor_prompt_in(
    lang: Lang,
    handoff: &SuccessorHandoff,
    previous_pane: Option<u64>,
) -> String {
    let profile = handoff.profile;
    let mut body = match lang {
        Lang::Ja => format!(
            "あなたは前任 master から引き継ぎを受けた新しい master です（profile: {profile}）。\n\
             以下の引き継ぎを読み、前任の状態を把握してから業務を開始してください。\n"
        ),
        Lang::En => format!(
            "You are the new master, taking over from your predecessor (profile: {profile}).\n\
             Read the handoff below and understand the previous state before starting work.\n"
        ),
    };
    if let Some(memo) = handoff.profile_memo.filter(|m| !m.trim().is_empty()) {
        let label = match lang {
            Lang::Ja => format!("--- handoff/{profile}.md（プロファイル運用メモ）---"),
            Lang::En => format!("--- handoff/{profile}.md (profile operating memo) ---"),
        };
        body.push_str(&format!("\n{label}\n{memo}\n--- end ---\n"));
    }
    for (key, content) in handoff
        .projects
        .iter()
        .filter(|(_, c)| !c.trim().is_empty())
    {
        let label = match lang {
            Lang::Ja => {
                format!("--- handoff/{HANDOFF_PROJECTS_DIR}/{key}.md（プロジェクト: {key}）---")
            }
            Lang::En => format!("--- handoff/{HANDOFF_PROJECTS_DIR}/{key}.md (project: {key}) ---"),
        };
        body.push_str(&format!("\n{label}\n{content}\n--- end ---\n"));
    }
    body.push_str(&jurisdiction_note(lang, handoff));
    let format_note = successor_format_note(lang, handoff);
    let steps = match (lang, previous_pane) {
        (Lang::Ja, Some(pane)) => format!(
            "\n引き継ぎ手順（この順で行う。順序を入れ替えない）:\n\
             1. 引き継ぎの内容と**実態**を突き合わせる。\
             `tako_orchestrator_workers` で spawn 済み worker を、`tako_list_panes` で\
             このタブのペイン構成を確認し、書かれていない worker や消えた worker を把握する。\n\
             2. 把握できたら「引き継ぎ完了」と、実態との食い違い（あれば）を報告する。\n\
             3. 前任 master のペイン（pane {pane}）を `tako_read_pane` で読み、\
             **入力欄にユーザーの未送達の指示が残っていないか**を確認する\
             （`input_status` が user / mixed、または `queued_messages_pending` が true なら残っている）。\
             残っていたらその内容を引き取り、ユーザーへ「これはまだ実行していません」と伝える。\n\
             4. 3 まで終えてから `tako_close_pane` で pane {pane} を閉じる\
             （幽霊 master を残さない）。1〜3 のどれかができていないなら**閉じない**。\n\
             5. 以降はこのペインが master。ユーザーの次の指示を待つ。"
        ),
        (Lang::En, Some(pane)) => format!(
            "\nHandoff procedure (in this order — do not reorder):\n\
             1. Cross-check the handoff against **reality**: use \
             `tako_orchestrator_workers` for spawned workers and `tako_list_panes` for this \
             tab's pane layout, and note any worker that is missing from the file or gone.\n\
             2. Once you have the picture, report \"handoff complete\" plus any mismatch you found.\n\
             3. Read the predecessor's pane (pane {pane}) with `tako_read_pane` and check \
             **whether the user left an undelivered instruction in its input box** \
             (`input_status` of user / mixed, or `queued_messages_pending` true means there is one). \
             If so, take it over and tell the user it has not been executed yet.\n\
             4. Only after step 3, close pane {pane} with `tako_close_pane` \
             (no ghost masters left behind). If any of steps 1-3 did not succeed, **do not close it**.\n\
             5. From here on this pane is the master. Wait for the user's next instruction."
        ),
        (Lang::Ja, None) => {
            "\n引き継ぎ内容を把握したら「引き継ぎ完了」と報告し、待機してください。".to_string()
        }
        (Lang::En, None) => {
            "\nOnce you have absorbed the handoff, report \"handoff complete\" and stand by."
                .to_string()
        }
    };
    format!("{body}{format_note}{steps}")
}

/// 管轄の決め方と、足りていないものの説明（#915）
fn jurisdiction_note(lang: Lang, handoff: &SuccessorHandoff) -> String {
    let mut s = String::new();
    if !handoff.missing_projects.is_empty() {
        let keys = handoff.missing_projects.join(", ");
        s.push_str(&match lang {
            Lang::Ja => format!(
                "\n注意: 管轄プロジェクト {keys} の引き継ぎファイルはまだありません。\
                 状況をユーザーと実態から作り直し、次の引き継ぎまでに\
                 `handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md` を書いてください。\n"
            ),
            Lang::En => format!(
                "\nNote: there is no handoff file yet for {keys}. Rebuild the picture from the \
                 user and from reality, and write \
                 `handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md` before your own handoff.\n"
            ),
        });
    }
    if handoff.jurisdiction == JurisdictionSource::Unresolved && !handoff.catalog.is_empty() {
        let list = handoff
            .catalog
            .iter()
            .map(|(key, path)| format!("  - {key}: {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        s.push_str(&match lang {
            Lang::Ja => format!(
                "\n管轄プロジェクトを特定できませんでした（担当割り当ても稼働 worker も無い）。\
                 全プロジェクトの本文をここへ貼るとあなたの文脈を無駄に食うので貼りません。\
                 下の一覧から**自分の任務に該当するものだけ**を読んでください\
                 （`tako_orchestrator_handoffs` の action=show でも読めます）:\n{list}\n\
                 自分の管轄が分かったら、プロファイルの担当プロジェクトを\
                 `tako_orchestrator_profiles`（set の projects）で設定しておくと、\
                 次の引き継ぎからは該当分だけが自動で渡ります。\n"
            ),
            Lang::En => format!(
                "\nThe jurisdiction could not be determined (no assigned projects, no live \
                 workers). Pasting every project's handoff here would waste your context, so \
                 it is not pasted. Read **only what matches your mission** from this list \
                 (also readable via `tako_orchestrator_handoffs` with action=show):\n{list}\n\
                 Once you know your scope, set the profile's assigned projects with \
                 `tako_orchestrator_profiles` (set / projects) so the next handoff carries \
                 just those.\n"
            ),
        });
    }
    s
}

/// 渡した本文それぞれの書式に応じた注意（#792 を複数ファイルへ広げたもの）
fn successor_format_note(lang: Lang, handoff: &SuccessorHandoff) -> String {
    let mut sectioned: Vec<String> = Vec::new();
    let mut legacy: Vec<String> = Vec::new();
    // 節が片方だけのファイル（欠けている側は実態 / ユーザーから取り直させる）
    let mut no_runtime: Vec<String> = Vec::new();
    let mut no_knowledge: Vec<String> = Vec::new();
    let profile_label = format!("handoff/{}.md", handoff.profile);
    let mut push = |label: String, body: &str| {
        if body.trim().is_empty() {
            return;
        }
        let doc = split_handoff(body);
        match doc.format() {
            HandoffFormat::Sectioned => {
                if doc.runtime.is_none() {
                    no_runtime.push(label.clone());
                }
                if doc.knowledge.is_none() {
                    no_knowledge.push(label.clone());
                }
                sectioned.push(label);
            }
            HandoffFormat::Legacy => legacy.push(label),
        }
    };
    if let Some(memo) = handoff.profile_memo {
        push(profile_label, memo);
    }
    for (key, body) in &handoff.projects {
        push(format!("{HANDOFF_PROJECTS_DIR}/{key}.md"), body);
    }
    if sectioned.is_empty() && legacy.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    if !sectioned.is_empty() {
        s.push_str(&match lang {
            Lang::Ja => format!(
                "\n書式: {} は 2 節に分かれています。\n\
                 - `{KNOWLEDGE_HEADING_JA}` = マシンに依らない知識。前提としてそのまま使ってよい\n\
                 - `{RUNTIME_HEADING_JA}` = このマシンの pane / tab 番号と worker 配置。\
                 **必ず実態で確認する**（別マシンから持ち込まれたものは丸ごと無効）\n",
                sectioned.join(" / ")
            ),
            Lang::En => format!(
                "\nFormat: {} use two sections.\n\
                 - `{KNOWLEDGE_HEADING_EN}` = portable knowledge; you may rely on it as-is\n\
                 - `{RUNTIME_HEADING_EN}` = this machine's pane / tab ids and worker layout. \
                 **Always verify it against reality** (it is void if it came from another machine)\n",
                sectioned.join(" / ")
            ),
        });
        if !no_runtime.is_empty() {
            s.push_str(&match lang {
                Lang::Ja => format!(
                    "  ※ {} には実行状態の節がありません。                     pane / tab はすべて実態から取り直してください。\n",
                    no_runtime.join(" / ")
                ),
                Lang::En => format!(
                    "  Note: {} has no runtime-state section.                      Re-derive every pane / tab id from reality.\n",
                    no_runtime.join(" / ")
                ),
            });
        }
        if !no_knowledge.is_empty() {
            s.push_str(&match lang {
                Lang::Ja => format!(
                    "  ※ {} には知識の節がありません。                     決定事項・方針はユーザーに確認してください。\n",
                    no_knowledge.join(" / ")
                ),
                Lang::En => format!(
                    "  Note: {} has no knowledge section.                      Confirm decisions and policies with the user.\n",
                    no_knowledge.join(" / ")
                ),
            });
        }
    }
    if !legacy.is_empty() {
        s.push_str(&match lang {
            Lang::Ja => format!(
                "\n書式: {} は節分離前の旧書式です。マシンに依らない知識と\
                 このマシン限定の実行状態（pane / tab 番号）が混在しているので、\
                 **番号への参照はすべて実態で確認**してください。\n\
                 次にこのファイルを更新するときは 2 節へ書き直してください:\
                 `## {KNOWLEDGE_HEADING_JA}` と `## {RUNTIME_HEADING_JA}`。\n",
                legacy.join(" / ")
            ),
            Lang::En => format!(
                "\nFormat: {} are in the old flat format, so portable knowledge and \
                 machine-local runtime state (pane / tab ids) are mixed together. \
                 **Verify every id against reality.**\n\
                 When you next refresh those files, rewrite them into two sections: \
                 `## {KNOWLEDGE_HEADING_EN}` and `## {RUNTIME_HEADING_EN}`.\n",
                legacy.join(" / ")
            ),
        });
    }
    s.push_str(&match lang {
        Lang::Ja => format!(
            "あなたが引き継ぎを更新するときは、プロジェクト固有の内容を\
             `handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md` へ、\
             プロジェクトに紐付かない運用知識だけを `handoff/{}.md` へ書いてください\
             （2 節の構造は保つ）。\n",
            handoff.profile
        ),
        Lang::En => format!(
            "When you refresh the handoff, write project-specific content into \
             `handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md` and keep only \
             project-independent operating knowledge in `handoff/{}.md` \
             (preserve the two sections).\n",
            handoff.profile
        ),
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧来の「1 ファイルを丸ごと渡す」形の束（既存テストの意味を保つための補助）
    fn memo_only<'a>(profile: &'a str, body: &'a str) -> SuccessorHandoff<'a> {
        SuccessorHandoff {
            profile,
            profile_memo: Some(body),
            ..Default::default()
        }
    }

    /// プロジェクト単位の束
    fn with_projects<'a>(
        profile: &'a str,
        projects: Vec<(&'a str, &'a str)>,
    ) -> SuccessorHandoff<'a> {
        SuccessorHandoff {
            profile,
            projects,
            jurisdiction: JurisdictionSource::Profile,
            ..Default::default()
        }
    }

    fn base() -> NudgeInput {
        NudgeInput {
            auto_handoff: true,
            ctx_percent: Some(65),
            threshold: 60,
            pane_age: Duration::from_secs(300),
            since_last_nudge: None,
            sent_count: 0,
            handoff_started: false,
        }
    }

    #[test]
    fn 閾値は50から60へ丸められる() {
        assert_eq!(clamp_ctx_threshold(0), 50);
        assert_eq!(clamp_ctx_threshold(49), 50);
        assert_eq!(clamp_ctx_threshold(50), 50);
        assert_eq!(clamp_ctx_threshold(55), 55);
        assert_eq!(clamp_ctx_threshold(60), 60);
        assert_eq!(clamp_ctx_threshold(80), 60);
        assert_eq!(clamp_ctx_threshold(u32::MAX), 60);
    }

    #[test]
    fn 明示指定の範囲外はエラーになる() {
        assert_eq!(parse_ctx_threshold(50), Ok(50));
        assert_eq!(parse_ctx_threshold(60), Ok(60));
        assert!(parse_ctx_threshold(49).is_err());
        assert!(parse_ctx_threshold(61).is_err());
        // 丸めずエラーにするのが要点（設定値と実効値の食い違いを作らない）
        assert!(parse_ctx_threshold(80).unwrap_err().contains("50〜60"));
    }

    #[test]
    fn 閾値超過でナッジを送る() {
        assert_eq!(nudge_decision(&base()), NudgeDecision::Send);
    }

    #[test]
    fn 閾値ちょうどでも送る() {
        let input = NudgeInput {
            ctx_percent: Some(60),
            ..base()
        };
        assert_eq!(nudge_decision(&input), NudgeDecision::Send);
    }

    #[test]
    fn 閾値未満では送らない() {
        let input = NudgeInput {
            ctx_percent: Some(59),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::BelowThreshold)
        );
    }

    #[test]
    fn 閾値を下げると同じctxでも送るようになる() {
        let low = NudgeInput {
            ctx_percent: Some(52),
            threshold: 50,
            ..base()
        };
        let high = NudgeInput {
            threshold: 60,
            ..low.clone()
        };
        assert_eq!(nudge_decision(&low), NudgeDecision::Send);
        assert_eq!(
            nudge_decision(&high),
            NudgeDecision::Skip(NudgeSkip::BelowThreshold)
        );
    }

    #[test]
    fn 自動ハンドオフ無効なら送らない() {
        let input = NudgeInput {
            auto_handoff: false,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::Disabled)
        );
    }

    #[test]
    fn ctxが読めないときは送らない() {
        let input = NudgeInput {
            ctx_percent: None,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::NoCtxData)
        );
    }

    #[test]
    fn 起動直後の猶予中は送らない() {
        let input = NudgeInput {
            pane_age: Duration::from_secs(10),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::WithinGrace)
        );
    }

    #[test]
    fn 再送間隔前は送らず経過後は送る() {
        let soon = NudgeInput {
            since_last_nudge: Some(Duration::from_secs(60)),
            sent_count: 1,
            ..base()
        };
        assert_eq!(
            nudge_decision(&soon),
            NudgeDecision::Skip(NudgeSkip::RepeatTooSoon)
        );
        let later = NudgeInput {
            since_last_nudge: Some(NUDGE_REPEAT + Duration::from_secs(1)),
            ..soon
        };
        assert_eq!(nudge_decision(&later), NudgeDecision::Send);
    }

    #[test]
    fn 上限まで送ったら黙る() {
        let input = NudgeInput {
            sent_count: NUDGE_MAX,
            since_last_nudge: Some(NUDGE_REPEAT * 10),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::MaxReached)
        );
    }

    #[test]
    fn handoff済みなら送らない() {
        let input = NudgeInput {
            handoff_started: true,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::HandoffStarted)
        );
    }

    #[test]
    fn ナッジ文面に実数値と手順が入る() {
        let s = nudge_prompt(72, 55, Some("/tmp/handoff/default.md"));
        assert!(s.contains("72"), "{s}");
        assert!(s.contains("55"), "{s}");
        assert!(s.contains("/tmp/handoff/default.md"), "{s}");
        assert!(s.contains("tako_orchestrator_handoff"), "{s}");
    }

    #[test]
    fn 後任プロンプトはkillを確認の後に置く() {
        let s = successor_prompt(&memo_only("default", "## 状態\n進行中: なし"), Some(42));
        assert!(s.contains("進行中: なし"), "{s}");
        assert!(s.contains("tako_read_pane"), "{s}");
        assert!(s.contains("tako_close_pane"), "{s}");
        assert!(s.contains("42"), "{s}");
        // 順序の構造保証: read（確認）が close（kill）より前に現れる
        let read_at = s.find("tako_read_pane").expect("read 手順がある");
        let close_at = s.find("tako_close_pane").expect("close 手順がある");
        assert!(
            read_at < close_at,
            "確認より先に kill を書いてはいけない: {s}"
        );
    }

    #[test]
    fn 旧ペイン不明ならkillを指示しない() {
        let s = successor_prompt(&memo_only("default", "state"), None);
        assert!(!s.contains("tako_close_pane"), "{s}");
    }

    // --- #792: 引き継ぎファイルの書式 ---

    /// 正典の見出し（日英）は必ず認識できる。文面と判定が同じ定数から出ていることの確認
    #[test]
    fn 正典の見出しを両言語で認識する() {
        for (h, want) in [
            (KNOWLEDGE_HEADING_JA, HandoffSection::Knowledge),
            (KNOWLEDGE_HEADING_EN, HandoffSection::Knowledge),
            (RUNTIME_HEADING_JA, HandoffSection::Runtime),
            (RUNTIME_HEADING_EN, HandoffSection::Runtime),
        ] {
            assert_eq!(section_of_line(&format!("## {h}")), Some(want), "{h}");
            assert_eq!(section_of_line(&format!("# {h}")), Some(want), "{h}");
        }
    }

    /// 表記ゆれ（番号付き・半角括弧・強調・語尾違い・全角空白）を吸収する。
    /// master が手で書くファイルなので、少しの違いで legacy 扱いに落ちないことが要点
    #[test]
    fn 見出しの表記ゆれを吸収する() {
        for line in [
            "## 知識（マシン非依存）",
            "## 知識(マシン非依存)",
            "## 1. 知識（マシン非依存）",
            "## **知識（マシン非依存）**",
            "##　知識",
            "### Knowledge (machine-independent)",
            "## knowledge",
        ] {
            assert_eq!(
                section_of_line(line),
                Some(HandoffSection::Knowledge),
                "{line}"
            );
        }
        for line in [
            "## 実行状態（このマシン限定）",
            "## 2) 実行状態",
            "## Runtime state (this machine only)",
            "## Machine state",
        ] {
            assert_eq!(
                section_of_line(line),
                Some(HandoffSection::Runtime),
                "{line}"
            );
        }
    }

    /// 見出しでないものを節と誤認しない（本文中の `#` / 別見出し）
    #[test]
    fn 節でない行は認識しない() {
        for line in [
            "知識（マシン非依存）", // 見出しでない
            "#知識",                // `#` 直後に空白が無い
            "## 現況（7/24 夕方）",
            "## 残キュー（優先順）",
            "## 0. 最初にやること",
            "- 知識: なんとか",
            "",
        ] {
            assert_eq!(section_of_line(line), None, "{line}");
        }
    }

    #[test]
    fn 新書式は2節へ分解される() {
        let doc = split_handoff(
            "# master 引き継ぎ\n\n\
             ## 知識（マシン非依存）\n\
             - 方針: A で行く\n\n\
             ## 実行状態（このマシン限定）\n\
             - worker: pane 12\n",
        );
        assert_eq!(doc.format(), HandoffFormat::Sectioned);
        assert!(doc.is_sectioned());
        assert_eq!(doc.preamble, "# master 引き継ぎ");
        assert_eq!(doc.knowledge.as_deref(), Some("- 方針: A で行く"));
        assert_eq!(doc.runtime.as_deref(), Some("- worker: pane 12"));
        assert_eq!(doc.section_labels(), vec!["knowledge", "runtime"]);
    }

    /// **後方互換の核**: 節が無ければ全文が preamble に入り Legacy になる
    #[test]
    fn 旧書式は全文が保たれる() {
        let legacy = "# master (default) 引き継ぎ\n\n\
                      ## 進行中（tab 136 / pane 884）\n\
                      - あれこれ\n\n\
                      ## 残キュー\n\
                      - それこれ\n";
        let doc = split_handoff(legacy);
        assert_eq!(doc.format(), HandoffFormat::Legacy);
        assert!(!doc.is_sectioned());
        assert!(doc.knowledge.is_none() && doc.runtime.is_none());
        assert_eq!(doc.preamble, legacy.trim());
        assert!(doc.section_labels().is_empty());
    }

    #[test]
    fn 片方の節だけでも新書式として扱う() {
        let doc = split_handoff("## 知識（マシン非依存）\n- 方針\n");
        assert_eq!(doc.format(), HandoffFormat::Sectioned);
        assert_eq!(doc.section_labels(), vec!["knowledge"]);
        assert!(doc.runtime.is_none());
    }

    /// 同じ節が 2 回出てきても内容を捨てない（片方だけ残すと引き継ぎが欠ける）
    #[test]
    fn 同じ節が複数回あれば連結する() {
        let doc = split_handoff(
            "## 知識（マシン非依存）\n前半\n\n## 実行状態（このマシン限定）\npane 1\n\n## 知識\n後半\n",
        );
        let k = doc.knowledge.as_deref().unwrap();
        assert!(k.contains("前半") && k.contains("後半"), "{k}");
        assert_eq!(doc.runtime.as_deref(), Some("pane 1"));
    }

    #[test]
    fn 見出しだけの節も節として数える() {
        let doc = split_handoff("## 知識（マシン非依存）\n\n## 実行状態（このマシン限定）\n");
        assert_eq!(doc.format(), HandoffFormat::Sectioned);
        assert_eq!(doc.section_labels(), vec!["knowledge", "runtime"]);
        assert_eq!(doc.knowledge.as_deref(), Some(""));
    }

    #[test]
    fn 空ファイルはlegacy扱い() {
        let doc = split_handoff("");
        assert_eq!(doc.format(), HandoffFormat::Legacy);
        assert_eq!(doc.preamble, "");
    }

    /// 雛形はそのまま読み直せる（生成と解析が同じ書式を指している）。日英とも
    #[test]
    fn 雛形は自分で解析できる() {
        for lang in [Lang::Ja, Lang::En] {
            let doc = split_handoff(&handoff_template_in(lang, "default"));
            assert_eq!(doc.format(), HandoffFormat::Sectioned, "{lang:?}");
            assert_eq!(
                doc.section_labels(),
                vec!["knowledge", "runtime"],
                "{lang:?}"
            );
        }
    }

    /// 見出し定数と節の対応（`heading`）が解析側と一致する
    #[test]
    fn 節の見出しは解析側と一致する() {
        for lang in [Lang::Ja, Lang::En] {
            for section in [HandoffSection::Knowledge, HandoffSection::Runtime] {
                let line = format!("## {}", section.heading(lang));
                assert_eq!(section_of_line(&line), Some(section), "{line}");
            }
        }
    }

    /// 新書式では「実行状態は実態で確認」の指示が入る。内容は全文そのまま渡る
    #[test]
    fn 後任プロンプトは新書式の扱いを説明する() {
        let content =
            "## 知識（マシン非依存）\n- 方針 A\n\n## 実行状態（このマシン限定）\n- pane 12\n";
        let s = successor_prompt_in(Lang::Ja, &memo_only("default", content), Some(42));
        assert!(s.contains("- 方針 A"), "{s}");
        assert!(s.contains("- pane 12"), "{s}");
        assert!(s.contains(KNOWLEDGE_HEADING_JA), "{s}");
        assert!(s.contains(RUNTIME_HEADING_JA), "{s}");
        assert!(s.contains("実態で確認"), "{s}");
        // 節の説明は本文の後・手順の前（読む順で意味が通る）
        let content_at = s.find("- pane 12").unwrap();
        let note_at = s.find("書式:").unwrap();
        let steps_at = s.find("引き継ぎ手順").unwrap();
        assert!(content_at < note_at && note_at < steps_at, "{s}");

        // 英語でも同じ構造（FR-4 の i18n 規約）
        let en = successor_prompt_in(Lang::En, &memo_only("default", content), Some(42));
        assert!(en.contains(KNOWLEDGE_HEADING_EN), "{en}");
        assert!(en.contains("verify it against reality"), "{en}");
    }

    /// 旧書式でも従来どおり全文が渡り、書き直しの指示が付く（自然な移行）
    #[test]
    fn 後任プロンプトは旧書式でも動く() {
        let legacy = "# 引き継ぎ\n\n## 進行中（tab 136 / pane 884）\n- あれこれ\n";
        let s = successor_prompt_in(Lang::Ja, &memo_only("default", legacy), Some(42));
        assert!(s.contains("## 進行中（tab 136 / pane 884）"), "{s}");
        assert!(s.contains("旧書式"), "{s}");
        assert!(s.contains("書き直"), "{s}");
        // 手順（#749 の不変条件）は旧書式でもそのまま入る
        assert!(
            s.contains("tako_read_pane") && s.contains("tako_close_pane"),
            "{s}"
        );
        let en = successor_prompt_in(Lang::En, &memo_only("default", legacy), Some(42));
        assert!(en.contains("old flat format"), "{en}");
    }

    /// 実行状態の節が欠けているときは pane / tab を取り直させる
    #[test]
    fn 実行状態の節が無ければ取り直しを指示する() {
        let s = successor_prompt_in(
            Lang::Ja,
            &memo_only("default", "## 知識（マシン非依存）\n- 方針\n"),
            None,
        );
        assert!(s.contains("実行状態の節がありません"), "{s}");
        let s = successor_prompt_in(
            Lang::Ja,
            &memo_only("default", "## 実行状態（このマシン限定）\n- p1\n"),
            None,
        );
        assert!(s.contains("知識の節がありません"), "{s}");
    }

    #[test]
    fn ナッジ文面が新書式の見出しを案内する() {
        let ja = nudge_prompt_in(Lang::Ja, 65, 60, None);
        assert!(ja.contains(KNOWLEDGE_HEADING_JA), "{ja}");
        assert!(ja.contains(RUNTIME_HEADING_JA), "{ja}");
        let en = nudge_prompt_in(Lang::En, 65, 60, None);
        assert!(en.contains(KNOWLEDGE_HEADING_EN), "{en}");
        assert!(en.contains(RUNTIME_HEADING_EN), "{en}");
    }

    #[test]
    fn roleからプロファイル名を取り出す() {
        assert_eq!(
            master_profile_of_role("orchestrator-master"),
            Some("default")
        );
        assert_eq!(
            master_profile_of_role("orchestrator-master:tako"),
            Some("tako")
        );
        // master 以外は対象外
        assert_eq!(master_profile_of_role("orchestrator-solo"), None);
        assert_eq!(master_profile_of_role("orchestrator-worker"), None);
        assert_eq!(master_profile_of_role("worker:1"), None);
        assert_eq!(master_profile_of_role(""), None);
        // 似た接頭辞に引っかからない
        assert_eq!(master_profile_of_role("orchestrator-master-old"), None);
        assert_eq!(master_profile_of_role("orchestrator-master:"), None);
    }

    /// #761: 表示用 role と env 用 role を取り違えないための生成側の契約。
    /// env に表示用文字列（`orchestrator-master:<profile>`）が入ると、受け手の
    /// `master:` 前置き解決が外れて default 扱いになる
    #[test]
    fn masterのroleは表示用とenv用で別の語彙になる() {
        assert_eq!(master_pane_role("default"), "orchestrator-master");
        assert_eq!(master_pane_role(""), "orchestrator-master");
        assert_eq!(master_pane_role("takodev"), "orchestrator-master:takodev");

        assert_eq!(master_role_env("default"), "master");
        assert_eq!(master_role_env(""), "master");
        assert_eq!(master_role_env("takodev"), "master:takodev");

        // env 用の値が表示用の語彙になっていない（#761 の回帰そのもの）
        assert!(!master_role_env("takodev").starts_with("orchestrator-"));
    }

    /// 生成した role は**どちらの語彙でも**元のプロファイル名に戻る（往復）
    #[test]
    fn どちらの語彙のroleからもプロファイル名に戻る() {
        for profile in ["default", "takodev", "sol"] {
            assert_eq!(
                master_profile_of_any_role(&master_pane_role(profile)),
                Some(profile),
                "表示用 role の往復: {profile}"
            );
            assert_eq!(
                master_profile_of_any_role(&master_role_env(profile)),
                Some(profile),
                "env 用 role の往復: {profile}"
            );
        }
        // master 以外・空 suffix・似た接頭辞
        assert_eq!(master_profile_of_any_role("master:"), None);
        assert_eq!(master_profile_of_any_role("solo:docs"), None);
        assert_eq!(master_profile_of_any_role("worker:tako"), None);
        assert_eq!(master_profile_of_any_role("master-old"), None);
        assert_eq!(master_profile_of_any_role(""), None);
    }

    // --- #915: プロジェクト単位の引き継ぎ ---

    #[test]
    fn プロジェクトキーはファイル名として安全なものだけ通す() {
        for ok in ["tako", "bunpoushi-migration", "MCR_A_2_38m-s", "a"] {
            assert!(valid_project_key(ok), "{ok}");
        }
        for ng in [
            "", ".", "..", "a/b", "a\\b", "C:x", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\nb",
            " a", "a ", "a.",
        ] {
            assert!(!valid_project_key(ng), "{ng:?}");
        }
        assert!(!valid_project_key(&"x".repeat(129)));
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 管轄は明示引数を最優先にする() {
        // worker が居ても明示引数が勝つ（居れば必ず推測が勝つと引数が死ぬ）
        let j = resolve_jurisdiction(&JurisdictionInput {
            explicit: Some(keys(&["a"])),
            profile_projects: keys(&["b"]),
            worker_projects: keys(&["c"]),
        });
        assert_eq!(j.projects, keys(&["a"]));
        assert_eq!(j.source, JurisdictionSource::Explicit);
    }

    #[test]
    fn 管轄はプロファイル担当と稼働workerの和になる() {
        let j = resolve_jurisdiction(&JurisdictionInput {
            explicit: None,
            profile_projects: keys(&["a", "b"]),
            worker_projects: keys(&["b", "c"]),
        });
        assert_eq!(j.projects, keys(&["a", "b", "c"]));
        assert_eq!(j.source, JurisdictionSource::Profile);
    }

    #[test]
    fn 担当が無ければ稼働workerだけで決める() {
        let j = resolve_jurisdiction(&JurisdictionInput {
            explicit: None,
            profile_projects: vec![],
            worker_projects: keys(&["c", "c"]),
        });
        assert_eq!(j.projects, keys(&["c"]));
        assert_eq!(j.source, JurisdictionSource::Workers);
    }

    #[test]
    fn 材料が無ければ管轄不明になる() {
        let j = resolve_jurisdiction(&JurisdictionInput::default());
        assert!(j.projects.is_empty());
        assert_eq!(j.source, JurisdictionSource::Unresolved);
        // 危険なキーは黙って落とす（パス脱出の防止）
        let j = resolve_jurisdiction(&JurisdictionInput {
            explicit: Some(keys(&["../etc", ""])),
            ..Default::default()
        });
        assert_eq!(j.source, JurisdictionSource::Unresolved);
    }

    /// 実データ（本番 default.md）と同じ形。`## 【<名前>】担当 master` で複数 master が
    /// 同居し、先頭に自分の 2 節がある混在ファイルを割れること
    #[test]
    fn 混在ファイルを見出しからプロジェクトへ割る() {
        let content = "# master 引き継ぎ\n\n\
             ## 知識（マシン非依存）\n- 方針 A\n\n\
             ## 実行状態（このマシン限定）\n- pane 12\n\n\
             ## 【bunpoushi 移行】担当 master — 最終盤\n- 移行の話\n\n\
             ## 【自作StreamDeck 立ち上げ】担当 master\n- streamdeck-lab の話\n";
        let all = keys(&["tako", "bunpoushi-migration", "streamdeck-lab", "hero-cpp"]);
        let plan = migration_plan(content, &all, &[]);
        assert!(plan.has_moves());
        let by = plan.by_project();
        assert_eq!(by.len(), 2, "{by:?}");
        assert_eq!(by[0].0, "bunpoushi-migration");
        assert!(by[0].1.contains("- 移行の話"));
        assert_eq!(by[1].0, "streamdeck-lab");
        // 先頭断片（このプロファイル自身の 2 節）は共通置き場へ残る
        let residue = plan.residue();
        assert!(
            residue.contains("- 方針 A") && residue.contains("- pane 12"),
            "{residue}"
        );
        assert!(!residue.contains("- 移行の話"), "{residue}");
        // 判定根拠が出る
        let owners: Vec<&str> = plan.segments.iter().map(|s| s.owner.as_str()).collect();
        assert_eq!(owners, vec!["primary", "heading_token", "heading_token"]);
    }

    /// 単一プロジェクト担当のプロファイルは全文がそのプロジェクトへ移る
    /// （単一プロジェクト master の移行が「従来同等」で通る）
    #[test]
    fn 単一プロジェクト担当は全文がそのプロジェクトへ移る() {
        let content = "# master 引き継ぎ\n\n## 知識（マシン非依存）\n- 方針\n";
        let plan = migration_plan(content, &keys(&["tako"]), &keys(&["tako"]));
        assert_eq!(
            plan.by_project(),
            vec![("tako".to_string(), content.trim().to_string())]
        );
        assert!(plan.residue().is_empty());
    }

    #[test]
    fn 明示マーカーが見出しより優先される() {
        let content = "## 【bunpoushi 移行】\n<!-- tako:project: tako -->\n- 本文\n";
        let plan = migration_plan(content, &keys(&["tako", "bunpoushi-migration"]), &[]);
        assert_eq!(plan.segments[0].project.as_deref(), Some("tako"));
        assert_eq!(plan.segments[0].owner, SegmentOwner::Marker);
        assert_eq!(marker_project("<!-- tako:project: x -->"), Some("x".into()));
        assert_eq!(marker_project("なし"), None);
        assert_eq!(project_marker("k"), "<!-- tako:project: k -->");
    }

    /// 持ち主を決められない断片は捨てず共通置き場へ残す（安全要件）
    #[test]
    fn 決められない断片は共通置き場へ残す() {
        let content = "## 【なにか別のミッション】\n- 捨ててはいけない本文\n";
        let plan = migration_plan(content, &keys(&["tako"]), &[]);
        assert!(!plan.has_moves());
        assert!(plan.residue().contains("- 捨ててはいけない本文"));
        assert_eq!(plan.segments[0].owner, SegmentOwner::Unresolved);
    }

    /// 移行は冪等: すでに割れているファイル（1 プロジェクト分）を再度かけても動かない
    #[test]
    fn 移行は冪等() {
        let content =
            "# 引き継ぎ: tako\n<!-- tako:project: tako -->\n\n## 知識（マシン非依存）\n- x\n";
        let plan1 = migration_plan(content, &keys(&["tako"]), &[]);
        let moved = plan1.by_project();
        assert_eq!(moved.len(), 1);
        let plan2 = migration_plan(&moved[0].1, &keys(&["tako"]), &[]);
        assert_eq!(
            plan2.by_project(),
            moved,
            "2 回目で内容が変わってはいけない"
        );
    }

    /// 後任へは**管轄プロジェクトの本文だけ**が渡る（#915 の核心）
    #[test]
    fn 後任プロンプトに管轄外の本文が入らない() {
        let h = with_projects(
            "default",
            vec![
                ("tako", "## 知識（マシン非依存）\n- tako の方針\n"),
                ("hero-cpp", "## 知識（マシン非依存）\n- hero の方針\n"),
            ],
        );
        let s = successor_prompt_in(Lang::Ja, &h, Some(7));
        assert!(s.contains("- tako の方針"), "{s}");
        assert!(s.contains("- hero の方針"), "{s}");
        assert!(s.contains("projects/tako.md"), "{s}");
        // 管轄に入っていないものは材料に無いので入りようがない（型で保証）
        assert!(!s.contains("bunpoushi"), "{s}");
        assert_eq!(h.format(), "sectioned");
    }

    #[test]
    fn 管轄不明なら本文を貼らず一覧を出す() {
        let h = SuccessorHandoff {
            profile: "default",
            catalog: vec![
                ("tako", "/x/projects/tako.md"),
                ("hero-cpp", "/x/projects/hero-cpp.md"),
            ],
            jurisdiction: JurisdictionSource::Unresolved,
            ..Default::default()
        };
        let s = successor_prompt_in(Lang::Ja, &h, None);
        assert!(s.contains("/x/projects/tako.md"), "{s}");
        assert!(s.contains("管轄プロジェクトを特定できませんでした"), "{s}");
        assert!(!h.has_content());
        let en = successor_prompt_in(Lang::En, &h, None);
        assert!(en.contains("could not be determined"), "{en}");
    }

    #[test]
    fn 管轄なのにファイルが無ければ作り直しを指示する() {
        let h = SuccessorHandoff {
            profile: "p",
            profile_memo: Some("- 運用メモ"),
            missing_projects: vec!["newproj"],
            jurisdiction: JurisdictionSource::Profile,
            ..Default::default()
        };
        let s = successor_prompt_in(Lang::Ja, &h, None);
        assert!(s.contains("newproj"), "{s}");
        assert!(s.contains("まだありません"), "{s}");
    }

    #[test]
    fn 新旧が混ざったらmixedになる() {
        let h = with_projects(
            "p",
            vec![
                ("a", "## 知識（マシン非依存）\n- x\n"),
                ("b", "# 平文\n- y\n"),
            ],
        );
        assert_eq!(h.format(), "mixed");
        let s = successor_prompt_in(Lang::Ja, &h, None);
        assert!(s.contains("projects/a.md は 2 節"), "{s}");
        assert!(s.contains("projects/b.md は節分離前"), "{s}");
    }

    #[test]
    fn 運用メモが膨らんだら警告する() {
        let small = "x\n".repeat(PROFILE_MEMO_SOFT_LIMIT_LINES);
        assert!(profile_memo_warning_in(Lang::Ja, "p", &small).is_none());
        let big = "x\n".repeat(PROFILE_MEMO_SOFT_LIMIT_LINES + 1);
        let w = profile_memo_warning_in(Lang::Ja, "p", &big).expect("警告が出る");
        assert!(w.contains("handoff/p.md"), "{w}");
        assert!(w.contains(HANDOFF_PROJECTS_DIR), "{w}");
        assert!(profile_memo_warning_in(Lang::En, "p", &big).is_some());
    }

    #[test]
    fn プロジェクト雛形は自分で解析できる() {
        for lang in [Lang::Ja, Lang::En] {
            let t = project_handoff_template_in(lang, "tako");
            let doc = split_handoff(&t);
            assert!(doc.is_sectioned(), "{t}");
            assert_eq!(marker_project(&t).as_deref(), Some("tako"));
            let memo = profile_memo_template_in(lang, "p");
            assert!(memo.contains("p"), "{memo}");
        }
    }

    // --- #854: プロファイルの取り違え ---

    /// env が失われていてもペインの role ラベルからプロファイルを取り戻す
    #[test]
    fn プロファイルはペインのroleラベルからも解決できる() {
        // 正常系: env が正しい
        assert_eq!(
            resolve_master_profile(Some("master:takodev"), Some("orchestrator-master:takodev")),
            ("takodev".to_string(), ProfileSource::CallerRole)
        );
        // #854 の症状: env が素の `master`（前置きが消えた）→ ラベルから取り戻す
        assert_eq!(
            resolve_master_profile(Some("master"), Some("orchestrator-master:takodev")),
            ("takodev".to_string(), ProfileSource::PaneRole)
        );
        // env そのものが無い場合も同じ
        assert_eq!(
            resolve_master_profile(None, Some("orchestrator-master:takodev")),
            ("takodev".to_string(), ProfileSource::PaneRole)
        );
        // 本当に default の master は default のまま（従来挙動）
        assert_eq!(
            resolve_master_profile(Some("master"), Some("orchestrator-master")),
            ("default".to_string(), ProfileSource::CallerRole)
        );
        // どちらも無ければ default
        assert_eq!(
            resolve_master_profile(None, None),
            ("default".to_string(), ProfileSource::Default)
        );
        // master 以外のラベルは材料にしない
        assert_eq!(
            resolve_master_profile(None, Some("worker:tako:1")),
            ("default".to_string(), ProfileSource::Default)
        );
    }
}

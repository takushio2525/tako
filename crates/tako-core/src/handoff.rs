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

/// 後任 master へ送る初期プロンプト（Issue #749 要件 3 / #792）。
///
/// **kill の順序を文面で固定する**のが要点: 引き継ぎ確認 → 旧ペインの入力欄確認 →
/// kill。確認前に kill させない（前任の未送達指示を取りこぼすと復元不能）。
///
/// #792: 引き継ぎ内容は**全文をそのまま**渡す（節ごとに切って渡すと、認識できなかった
/// 部分が黙って落ちる）。代わりに `split_handoff` の結果から**節ごとの扱い**を書き添える。
/// 旧書式（節なし）なら「番号への参照は全部実態で確認しろ + 次の更新で書き直せ」を添え、
/// 自然な更新で新書式へ移行させる
pub fn successor_prompt(
    profile: &str,
    handoff_content: &str,
    previous_pane: Option<u64>,
) -> String {
    successor_prompt_in(lang(), profile, handoff_content, previous_pane)
}

/// 言語を明示しての後任プロンプト（#608: テストは言語グローバルに触らない）
pub fn successor_prompt_in(
    lang: Lang,
    profile: &str,
    handoff_content: &str,
    previous_pane: Option<u64>,
) -> String {
    let format_note = handoff_format_note(lang, &split_handoff(handoff_content));
    let body = match lang {
        Lang::Ja => format!(
            "あなたは前任 master から引き継ぎを受けた新しい master です。\n\
             以下の引き継ぎファイルの内容を読み、前任の状態を把握してから業務を開始してください。\n\n\
             --- handoff/{profile}.md ---\n\
             {handoff_content}\n\
             --- end ---\n"
        ),
        Lang::En => format!(
            "You are the new master, taking over from your predecessor.\n\
             Read the handoff file below and understand the previous state before starting work.\n\n\
             --- handoff/{profile}.md ---\n\
             {handoff_content}\n\
             --- end ---\n"
        ),
    };
    let steps = match (lang, previous_pane) {
        (Lang::Ja, Some(pane)) => format!(
            "\n引き継ぎ手順（この順で行う。順序を入れ替えない）:\n\
             1. 引き継ぎファイルの内容と**実態**を突き合わせる。\
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
             1. Cross-check the handoff file against **reality**: use \
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

/// 後任へ渡す「節ごとの扱い」の説明（#792）。
///
/// 新書式なら「知識はそのまま使え / 実行状態は実態で確認せよ」、
/// 旧書式なら「混在しているので番号は全部確認 + 次の更新で書き直せ」。
/// **旧書式でも動く**ことがこの関数の要点で、書き直しの指示だけを足す
fn handoff_format_note(lang: Lang, doc: &HandoffDoc) -> String {
    match (lang, doc.format()) {
        (Lang::Ja, HandoffFormat::Sectioned) => {
            let mut s = format!(
                "\n書式: この引き継ぎは 2 節に分かれています。\n\
                 - `{KNOWLEDGE_HEADING_JA}` = マシンに依らない知識。前提としてそのまま使ってよい\n\
                 - `{RUNTIME_HEADING_JA}` = このマシンの pane / tab 番号と worker 配置。\
                 **必ず実態で確認する**（別マシンから持ち込まれたものは丸ごと無効）\n"
            );
            if doc.runtime.is_none() {
                s.push_str(
                    "  ※ 実行状態の節がありません。pane / tab はすべて実態から取り直してください。\n",
                );
            }
            if doc.knowledge.is_none() {
                s.push_str(
                    "  ※ 知識の節がありません。決定事項・方針はユーザーに確認してください。\n",
                );
            }
            s.push_str(
                "あなたが次に引き継ぎファイルを更新するときも、この 2 節の構造を保ってください。\n",
            );
            s
        }
        (Lang::En, HandoffFormat::Sectioned) => {
            let mut s = format!(
                "\nFormat: this handoff has two sections.\n\
                 - `{KNOWLEDGE_HEADING_EN}` = portable knowledge; you may rely on it as-is\n\
                 - `{RUNTIME_HEADING_EN}` = this machine's pane / tab ids and worker layout. \
                 **Always verify it against reality** (it is void if it came from another machine)\n"
            );
            if doc.runtime.is_none() {
                s.push_str(
                    "  Note: there is no runtime-state section. Re-derive every pane / tab id from reality.\n",
                );
            }
            if doc.knowledge.is_none() {
                s.push_str(
                    "  Note: there is no knowledge section. Confirm decisions and policies with the user.\n",
                );
            }
            s.push_str("Keep the same two sections when you refresh the handoff file yourself.\n");
            s
        }
        (Lang::Ja, HandoffFormat::Legacy) => format!(
            "\n書式: この引き継ぎは節分離前の旧書式です。マシンに依らない知識と\
             このマシン限定の実行状態（pane / tab 番号）が混在しているので、\
             **番号への参照はすべて実態で確認**してください。\n\
             次にこのファイルを更新するときは 2 節へ書き直してください:\
             `## {KNOWLEDGE_HEADING_JA}` と `## {RUNTIME_HEADING_JA}`。\n"
        ),
        (Lang::En, HandoffFormat::Legacy) => format!(
            "\nFormat: this handoff is in the old flat format, so portable knowledge and \
             machine-local runtime state (pane / tab ids) are mixed together. \
             **Verify every id against reality.**\n\
             When you next refresh this file, rewrite it into two sections: \
             `## {KNOWLEDGE_HEADING_EN}` and `## {RUNTIME_HEADING_EN}`.\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let s = successor_prompt("default", "## 状態\n進行中: なし", Some(42));
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
        let s = successor_prompt("default", "state", None);
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
        let s = successor_prompt_in(Lang::Ja, "default", content, Some(42));
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
        let en = successor_prompt_in(Lang::En, "default", content, Some(42));
        assert!(en.contains(KNOWLEDGE_HEADING_EN), "{en}");
        assert!(en.contains("verify it against reality"), "{en}");
    }

    /// 旧書式でも従来どおり全文が渡り、書き直しの指示が付く（自然な移行）
    #[test]
    fn 後任プロンプトは旧書式でも動く() {
        let legacy = "# 引き継ぎ\n\n## 進行中（tab 136 / pane 884）\n- あれこれ\n";
        let s = successor_prompt_in(Lang::Ja, "default", legacy, Some(42));
        assert!(s.contains("## 進行中（tab 136 / pane 884）"), "{s}");
        assert!(s.contains("旧書式"), "{s}");
        assert!(s.contains("書き直"), "{s}");
        // 手順（#749 の不変条件）は旧書式でもそのまま入る
        assert!(
            s.contains("tako_read_pane") && s.contains("tako_close_pane"),
            "{s}"
        );
        let en = successor_prompt_in(Lang::En, "default", legacy, Some(42));
        assert!(en.contains("old flat format"), "{en}");
    }

    /// 実行状態の節が欠けているときは pane / tab を取り直させる
    #[test]
    fn 実行状態の節が無ければ取り直しを指示する() {
        let s = successor_prompt_in(
            Lang::Ja,
            "default",
            "## 知識（マシン非依存）\n- 方針\n",
            None,
        );
        assert!(s.contains("実行状態の節がありません"), "{s}");
        let s = successor_prompt_in(
            Lang::Ja,
            "default",
            "## 実行状態（このマシン限定）\n- p1\n",
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
}

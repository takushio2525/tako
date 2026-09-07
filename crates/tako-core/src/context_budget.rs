//! 起動時ロードの予算（Issue #1139）
//!
//! AI エージェントが**起動した瞬間に強制ロードされる**もの（グローバル指示ファイル・
//! `AGENTS.md` とその `@import` チェーン・system prompt・引き継ぎ）の量を測り、
//! 種別ごとの上限と突き合わせる。
//!
//! # なぜ要るか
//!
//! tako のリポジトリで実測（2026-09-06）したところ、master は**何も作業しないうちに**
//! コンテキストの 31%（約 31 万トークン）を消費していた。内訳の 18 万トークンは
//! `@import` された作業ログ `.agent/progress.md`（332 エントリ / 56 作業日 / 407 KB）で、
//! 「14 日 / 30 件超で archive を**提案**する」という規約は在ったのに
//! **提案ベースなので誰も実行せず 3 か月積もった**。
//!
//! そこで「毎ターン必ず読む範囲」と「アーカイブへ落とす範囲」を**数値で定義**し、
//! この表を正本にして ①規約文 ②`tako context-budget` の判定 ③CI 番犬 が
//! すべて同じ数値を引く形にする（数値の二重管理を作らない）。
//!
//! # 何を自動で直すか
//!
//! **安全に機械化できるものだけ**。`## YYYY-MM-DD` 見出しが並ぶ作業ログは、
//! 古いエントリを**丸ごと**アーカイブへ移して 1 行へ畳める（本文の要約も改変もしない。
//! 全文は git 履歴に残る）。`AGENTS.md` の肥大や `@import` の増殖は、
//! 何をどこへ分けるかが人間の判断なので**提案**にとどめる。

use crate::platform::support::Note;

// ─── 予算表（正本。規約文もテストもここを引く） ──────────────────────────

/// 起動時ロードの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// `~/.claude/CLAUDE.md`（全プロジェクト共通の指示）
    GlobalGuide,
    /// リポジトリの `AGENTS.md` / `CLAUDE.md`（エージェント規約）
    AgentsGuide,
    /// `## YYYY-MM-DD` 見出しが並ぶ作業ログ（`progress.md` 型）
    ProgressLog,
    /// 現在状態のスナップショット（`activeContext.md` 型）
    ActiveContext,
    /// その他の `@import` 先
    Imported,
    /// 引き継ぎの運用メモ（`handoff/<profile>.md`）
    HandoffMemo,
    /// master / solo の system prompt
    SystemPrompt,
    /// claude の永続メモリ（`MEMORY.md`）
    Memory,
}

impl ItemKind {
    /// JSON / CLI で使う安定した名前
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::GlobalGuide => "global_guide",
            ItemKind::AgentsGuide => "agents_guide",
            ItemKind::ProgressLog => "progress_log",
            ItemKind::ActiveContext => "active_context",
            ItemKind::Imported => "imported",
            ItemKind::HandoffMemo => "handoff_memo",
            ItemKind::SystemPrompt => "system_prompt",
            ItemKind::Memory => "memory",
        }
    }

    /// 表示用の呼び名
    pub fn label(self) -> Note {
        match self {
            ItemKind::GlobalGuide => Note::new("グローバル指示", "global guide"),
            ItemKind::AgentsGuide => Note::new("エージェント規約", "agent guide"),
            ItemKind::ProgressLog => Note::new("作業ログ", "work log"),
            ItemKind::ActiveContext => Note::new("現在状態", "active context"),
            ItemKind::Imported => Note::new("取り込み", "imported"),
            ItemKind::HandoffMemo => Note::new("引き継ぎ運用メモ", "handoff memo"),
            ItemKind::SystemPrompt => Note::new("system prompt", "system prompt"),
            ItemKind::Memory => Note::new("永続メモリ", "memory"),
        }
    }

    /// この種別を `tako context-budget fix` が自動で直せるか。
    /// **直せるのは作業ログだけ**（残りは何をどこへ分けるかが人間の判断）
    pub fn auto_fixable(self) -> bool {
        matches!(self, ItemKind::ProgressLog)
    }
}

/// 種別ごとの上限。`None` はその軸を見ない
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    pub max_bytes: Option<usize>,
    pub max_lines: Option<usize>,
    pub max_entries: Option<usize>,
    pub max_work_days: Option<usize>,
    /// 1 エントリの本文行数。**自動では直せない**（要約の捏造になる）ので
    /// 超過は提案として名指しするだけ
    pub max_entry_lines: Option<usize>,
}

/// 作業ログの予算（規約文が引く定数。ここを直すと規約テストが落ちる）
pub const PROGRESS_MAX_WORK_DAYS: usize = 5;
pub const PROGRESS_MAX_ENTRIES: usize = 20;
pub const PROGRESS_MAX_ENTRY_LINES: usize = 3;
pub const PROGRESS_MAX_BYTES: usize = 12 * 1024;
/// アーカイブに残す日数。これより古い行は消す（git 履歴・Issue・PR が正本）
pub const ARCHIVE_RETAIN_DAYS: i64 = 90;

/// エージェント規約（`AGENTS.md`）の上限
pub const AGENTS_GUIDE_MAX_BYTES: usize = 30 * 1024;
/// `@import` チェーンの合計（`AGENTS.md` 自身は含まない）
pub const IMPORT_TOTAL_MAX_BYTES: usize = 40 * 1024;
/// 現在状態の行数（既存規約 80 行を吸収）
pub const ACTIVE_CONTEXT_MAX_LINES: usize = 80;
/// 引き継ぎ運用メモの行数（#915 の既存警告を吸収）
pub const HANDOFF_MEMO_MAX_LINES: usize = 80;
/// グローバル指示ファイル
pub const GLOBAL_GUIDE_MAX_BYTES: usize = 24 * 1024;
/// master / solo の system prompt（プロファイル別の生成物。Issue #1154）。
///
/// **これは tako 自身が配るものなので、超えたら tako を直す**（ユーザーに我慢させない）。
/// 手順の詳細は `tako orchestrator guide <topic>` で必要なときだけ引く形へ移し、
/// prompt には「いつ引くか」だけを残す。ユーザー側の追記（`prompt_blocks.append` =
/// 個人環境固有のルール）で超えたぶんは、**どの block が何バイトか**を
/// `proposals` に添えて分離先を案内する
pub const SYSTEM_PROMPT_MAX_BYTES: usize = 24 * 1024;

/// #1154 の A/B: 立てると system prompt の予算を外し（= 変更前の「観測だけ」へ戻す）、
/// prompt 側は guide の本文を丸ごと差し戻す（`tako_control::orchestrator::guide`）。
/// **同一バイナリで旧挙動を再現できる**ようにして、新テストの検出力を実測する
pub fn legacy_1154() -> bool {
    std::env::var_os("TAKO_1154_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// 種別ごとの上限を引く（**判定・規約文・番犬がすべてこの 1 本を通る**）
pub fn limits(kind: ItemKind) -> Limits {
    match kind {
        ItemKind::ProgressLog => Limits {
            max_bytes: Some(PROGRESS_MAX_BYTES),
            max_entries: Some(PROGRESS_MAX_ENTRIES),
            max_work_days: Some(PROGRESS_MAX_WORK_DAYS),
            max_entry_lines: Some(PROGRESS_MAX_ENTRY_LINES),
            ..Limits::default()
        },
        ItemKind::AgentsGuide => Limits {
            max_bytes: Some(AGENTS_GUIDE_MAX_BYTES),
            ..Limits::default()
        },
        ItemKind::ActiveContext => Limits {
            max_lines: Some(ACTIVE_CONTEXT_MAX_LINES),
            ..Limits::default()
        },
        ItemKind::HandoffMemo => Limits {
            max_lines: Some(HANDOFF_MEMO_MAX_LINES),
            ..Limits::default()
        },
        ItemKind::GlobalGuide => Limits {
            max_bytes: Some(GLOBAL_GUIDE_MAX_BYTES),
            ..Limits::default()
        },
        // #1154: system prompt は tako 自身の生成物なので上限を持つ。
        // 旧挙動（観測だけ）へ戻す A/B は `TAKO_1154_LEGACY`
        ItemKind::SystemPrompt if legacy_1154() => Limits::default(),
        ItemKind::SystemPrompt => Limits {
            max_bytes: Some(SYSTEM_PROMPT_MAX_BYTES),
            ..Limits::default()
        },
        // 取り込み先の 1 本ずつには上限を置かない（合計 IMPORT_TOTAL_MAX_BYTES で見る）。
        // claude の永続メモリは claude が持ち主なので観測だけ
        ItemKind::Imported | ItemKind::Memory => Limits::default(),
    }
}

/// 規約文を囲むマーカー。この間は**生成物**なので手で書き換えない
pub const RULE_BEGIN: &str = "<!-- tako:context-budget-rule -->";
pub const RULE_END: &str = "<!-- /tako:context-budget-rule -->";

/// 予算の規約文（**この表から組み立てる**）。
///
/// tako が配る規約は 3 か所（リポジトリの `AGENTS.md` / `.agent/conventions.md` /
/// `tako setup` が配る CLAUDE.md 節）に置くが、**数値を手で書くと必ずずれる**ので
/// ここで組み立てたものを埋め込み、一致をテストで固定する
pub fn rule_markdown() -> String {
    format!(
        "\
### 絶対に読む範囲（`@import` してよいもの）

- **現在状態**（`activeContext.md` 型）: {active_lines} 行以内。「現在の対象 / 直近の観点 / 次の一手」だけを置く
- **作業ログ**（`progress.md` 型）: **直近 {days} 作業日 かつ {entries} エントリ かつ {log_kb} KB 以内**。
  1 エントリは「何を / どこを / 結果」の **{entry_lines} 行以内**にとどめ、詳細は git log・Issue・PR に委ねる
- タスクリスト 1 本（プロジェクトにあれば）

### アーカイブとする範囲

- 予算から外れたエントリは `progress-archive.md` へ **1 行**（`- YYYY-MM-DD #番号 一言`）で移す
- アーカイブは `@import` しない・普段は Read しない
- **{retain} 日より古いアーカイブ行は消す**（git log・Issue・PR が正本なので情報は失われない）

### それ以外の上限

- エージェント規約（`AGENTS.md`）は **{guide_kb} KB 以内**。長い注記・実測・罠は `.agent/` 配下の
  別ファイルへ出し、規約からは**バックティック参照**で案内する（`@import` にはしない）
- `@import` の合計は **{import_kb} KB 以内**
- 引き継ぎの運用メモは {handoff_lines} 行以内 / グローバル指示ファイルは {global_kb} KB 以内
- master / solo の **system prompt は {prompt_kb} KB 以内**。手順の詳細は
  `tako orchestrator guide <topic>` で必要なときだけ引く形にし、prompt には「いつ引くか」を残す

### 機械強制

- `tako context-budget` で状態を確認し、`tako context-budget fix` で作業ログの移送を自動で行う
  （**冪等・本文は改変しない・全文は git 履歴に残る**）
- 自動で直せないもの（規約の肥大・`@import` の増殖）は `proposals` として直し方が返る
- CI の番犬がこの予算を検査するので、超えたまま merge できない",
        active_lines = ACTIVE_CONTEXT_MAX_LINES,
        days = PROGRESS_MAX_WORK_DAYS,
        entries = PROGRESS_MAX_ENTRIES,
        log_kb = PROGRESS_MAX_BYTES / 1024,
        entry_lines = PROGRESS_MAX_ENTRY_LINES,
        retain = ARCHIVE_RETAIN_DAYS,
        guide_kb = AGENTS_GUIDE_MAX_BYTES / 1024,
        import_kb = IMPORT_TOTAL_MAX_BYTES / 1024,
        handoff_lines = HANDOFF_MEMO_MAX_LINES,
        global_kb = GLOBAL_GUIDE_MAX_BYTES / 1024,
        prompt_kb = SYSTEM_PROMPT_MAX_BYTES / 1024,
    )
}

/// マーカーに挟まれた規約文を取り出す（規約の一致を検査する側が使う）
pub fn extract_rule(text: &str) -> Option<&str> {
    let start = text.find(RULE_BEGIN)? + RULE_BEGIN.len();
    let rest = &text[start..];
    let end = rest.find(RULE_END)?;
    Some(rest[..end].trim())
}

// ─── 計測 ──────────────────────────────────────────────────────────────

/// 概算トークン数。**推定の手法はここ 1 箇所**（`tako` の日本語主体の文書で
/// 実測 chars 233,519 ≒ 18 万トークンだったので chars ÷ 1.3 に合わせてある）
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count() * 10 / 13
}

/// 1 項目の実測値
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Measurement {
    pub bytes: usize,
    pub lines: usize,
    pub chars: usize,
    pub est_tokens: usize,
    /// 作業ログのときだけ入る
    pub entries: Option<usize>,
    pub work_days: Option<usize>,
    /// 本文が最も長いエントリの行数
    pub longest_entry_lines: Option<usize>,
    /// 本文が `max_entry_lines` を超えているエントリ数
    pub over_long_entries: Option<usize>,
}

/// 本文を測る。`ProgressLog` のときはエントリ構造も数える
pub fn measure(kind: ItemKind, text: &str) -> Measurement {
    let mut m = Measurement {
        bytes: text.len(),
        lines: line_count(text),
        chars: text.chars().count(),
        est_tokens: estimate_tokens(text),
        ..Measurement::default()
    };
    if kind == ItemKind::ProgressLog {
        let log = parse_log(text);
        let cap = limits(kind).max_entry_lines.unwrap_or(usize::MAX);
        m.entries = Some(log.entries.len());
        m.work_days = Some(log.work_days());
        m.longest_entry_lines = Some(
            log.entries
                .iter()
                .map(|e| e.body_lines())
                .max()
                .unwrap_or(0),
        );
        m.over_long_entries = Some(log.entries.iter().filter(|e| e.body_lines() > cap).count());
    }
    m
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

// ─── 判定 ──────────────────────────────────────────────────────────────

/// 超過した軸
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Bytes,
    Lines,
    Entries,
    WorkDays,
    EntryLines,
    ImportTotalBytes,
}

impl Metric {
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Bytes => "bytes",
            Metric::Lines => "lines",
            Metric::Entries => "entries",
            Metric::WorkDays => "work_days",
            Metric::EntryLines => "entry_lines",
            Metric::ImportTotalBytes => "import_total_bytes",
        }
    }
}

/// 予算超過 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub metric: Metric,
    pub actual: usize,
    pub limit: usize,
    /// `tako context-budget fix` が直せるか。
    /// false のものは `proposals` として直し方だけを返す
    pub fixable: bool,
    /// 直し方（日英）
    pub note: Note,
}

/// 超過の理由と直し方。**同じ文言を複数箇所で使うので定数に集約する**
pub mod notes {
    use crate::platform::support::Note;

    pub const PROGRESS_TOO_BIG: Note = Note::new(
        "毎ターン全文が読み込まれる作業ログが予算を超えている。`tako context-budget fix` で古いエントリを archive へ 1 行に畳んで移せる（本文は git 履歴に残るので失われない）",
        "The work log is force-loaded in full every turn and is over budget. Run `tako context-budget fix` to fold old entries into one line each in the archive (full text stays in git history)",
    );

    pub const PROGRESS_ENTRY_TOO_LONG: Note = Note::new(
        "1 エントリが長すぎる。1 件は「何を / どこを / 結果」の 1〜3 行にとどめ、詳細は git log・Issue・PR に委ねる（自動では直せない = 要約の捏造になるため、書くときに守る）",
        "An entry is too long. Keep each entry to 1-3 lines (what / where / outcome) and leave details to git log, issues and PRs (not auto-fixable: summarising would fabricate text, so honour it when writing)",
    );

    pub const AGENTS_GUIDE_TOO_BIG: Note = Note::new(
        "エージェント規約が毎ターン全文読み込まれる量を超えている。長い注記・実測・罠は `.agent/` 配下の別ファイルへ移し、規約からはバックティック参照で案内する（`@import` にはしない）",
        "The agent guide exceeds what should be force-loaded every turn. Move long notes, measurements and pitfalls into a separate file under `.agent/` and reference it with backticks (do not `@import` it)",
    );

    pub const IMPORT_TOTAL_TOO_BIG: Note = Note::new(
        "`@import` の合計が予算を超えている。毎ターン読む価値があるのは「現在状態」「直近の作業ログ」「タスクリスト 1 本」だけ。残りはバックティック参照へ落とす",
        "The `@import` chain exceeds its budget. Only the current state, the recent work log and one task list are worth loading every turn; demote the rest to backtick references",
    );

    pub const ACTIVE_CONTEXT_TOO_LONG: Note = Note::new(
        "現在状態が長すぎる。過去ターンの実装詳細は作業ログへ逃がし、ここには「現在の対象 / 直近の観点 / 次の一手」だけを残す",
        "The active context is too long. Move past-turn implementation details into the work log and keep only the current target, recent findings and next step here",
    );

    pub const HANDOFF_MEMO_TOO_LONG: Note = Note::new(
        "引き継ぎの運用メモが長すぎる。プロジェクトに紐付く知識は `handoff/projects/<key>.md` へ移す（#915）",
        "The handoff memo is too long. Move project-scoped knowledge into `handoff/projects/<key>.md` (#915)",
    );

    pub const GLOBAL_GUIDE_TOO_BIG: Note = Note::new(
        "グローバル指示ファイルが大きい。全プロジェクトの全ターンに載るので、領域固有の詳細は snippets へ出して必要なときだけ読む形にする",
        "The global guide is large. It rides on every turn of every project, so move domain-specific detail into snippets that are read only when needed",
    );

    pub const SYSTEM_PROMPT_TOO_BIG: Note = Note::new(
        "master / solo の system prompt が予算を超えている。長寿命セッションの起動直後の固定費なので、手順の詳細は `tako orchestrator guide <topic>` で必要なときだけ引く形へ移す。プロファイルの `prompt_blocks.append`（個人環境固有のルール）が大きい場合は、常に要る規則だけを残して残りを別ファイルへ分け、そこは AI が必要なときだけ読む",
        "The master / solo system prompt is over budget. It is a fixed cost paid at the start of every long-lived session, so move procedure detail into `tako orchestrator guide <topic>` and fetch it on demand. If the profile's `prompt_blocks.append` (your machine-specific rules) is the large part, keep only the always-needed rules there and split the rest into a file the agent reads only when needed",
    );
}

/// 実測値を予算と突き合わせる
pub fn violations(kind: ItemKind, m: &Measurement) -> Vec<Violation> {
    let lim = limits(kind);
    let mut out = Vec::new();
    let fixable = kind.auto_fixable();

    let size_note = match kind {
        ItemKind::ProgressLog => notes::PROGRESS_TOO_BIG,
        ItemKind::AgentsGuide => notes::AGENTS_GUIDE_TOO_BIG,
        ItemKind::ActiveContext => notes::ACTIVE_CONTEXT_TOO_LONG,
        ItemKind::HandoffMemo => notes::HANDOFF_MEMO_TOO_LONG,
        ItemKind::GlobalGuide => notes::GLOBAL_GUIDE_TOO_BIG,
        ItemKind::SystemPrompt => notes::SYSTEM_PROMPT_TOO_BIG,
        _ => notes::IMPORT_TOTAL_TOO_BIG,
    };

    let mut push = |metric: Metric, actual: usize, limit: Option<usize>, fixable, note| {
        if let Some(limit) = limit {
            if actual > limit {
                out.push(Violation {
                    metric,
                    actual,
                    limit,
                    fixable,
                    note,
                });
            }
        }
    };
    push(Metric::Bytes, m.bytes, lim.max_bytes, fixable, size_note);
    push(Metric::Lines, m.lines, lim.max_lines, false, size_note);
    push(
        Metric::Entries,
        m.entries.unwrap_or(0),
        lim.max_entries,
        fixable,
        size_note,
    );
    push(
        Metric::WorkDays,
        m.work_days.unwrap_or(0),
        lim.max_work_days,
        fixable,
        size_note,
    );
    // 1 エントリの行数だけは **自動で直さない**（本文を削るのは要約の捏造になる）
    push(
        Metric::EntryLines,
        m.longest_entry_lines.unwrap_or(0),
        lim.max_entry_lines,
        false,
        notes::PROGRESS_ENTRY_TOO_LONG,
    );
    out
}

/// `@import` チェーンの合計に対する判定
pub fn import_total_violation(total_bytes: usize) -> Option<Violation> {
    (total_bytes > IMPORT_TOTAL_MAX_BYTES).then_some(Violation {
        metric: Metric::ImportTotalBytes,
        actual: total_bytes,
        limit: IMPORT_TOTAL_MAX_BYTES,
        fixable: false,
        note: notes::IMPORT_TOTAL_TOO_BIG,
    })
}

// ─── 作業ログのパース ──────────────────────────────────────────────────

/// `## YYYY-MM-DD…` 見出し 1 個ぶん
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// 見出しから採った日付（`YYYY-MM-DD`）
    pub date: String,
    /// 見出し行そのもの
    pub heading: String,
    /// 見出しに続く行（末尾の空行は落としてある）
    pub body: Vec<String>,
}

impl LogEntry {
    /// 本文の行数（空行は数えない）
    pub fn body_lines(&self) -> usize {
        self.body.iter().filter(|l| !l.trim().is_empty()).count()
    }

    /// このエントリを丸ごと書き戻したときの文字列
    pub fn render(&self) -> String {
        let mut s = String::from(&self.heading);
        s.push('\n');
        for l in &self.body {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    /// 見出しの日付より後ろの部分（`## 2026-08-24（…）` → `（…）`）
    fn heading_tail(&self) -> &str {
        let after = self.heading.trim_start_matches('#').trim_start();
        after[self.date.len().min(after.len())..].trim()
    }

    /// アーカイブ用の 1 行。**要約は作らない**。見出しから採った文字列と、
    /// 見出しに番号が無ければ本文の先頭で見つかった `#NNN` を添えるだけ
    pub fn archive_line(&self) -> String {
        let tail = strip_wrapping(self.heading_tail());
        let refs = if has_issue_ref(tail) {
            String::new()
        } else {
            match first_issue_ref(&self.body) {
                Some(n) => format!("#{n} "),
                None => String::new(),
            }
        };
        let text = if tail.is_empty() {
            first_body_text(&self.body)
        } else {
            tail.to_string()
        };
        let line = format!("- {} {}{}", self.date, refs, text);
        truncate_chars(&line, ARCHIVE_LINE_MAX_CHARS)
    }
}

/// アーカイブ 1 行の最大文字数（見出しが長いときだけ機械的に切り詰める）
pub const ARCHIVE_LINE_MAX_CHARS: usize = 160;

fn strip_wrapping(s: &str) -> &str {
    let s = s.trim();
    for (open, close) in [('（', '）'), ('(', ')')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            return inner.trim();
        }
    }
    s
}

fn has_issue_ref(s: &str) -> bool {
    first_issue_ref_in(s).is_some()
}

fn first_issue_ref_in(s: &str) -> Option<u32> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '#' {
            let mut j = i + 1;
            let mut num = String::new();
            while j < b.len() && b[j].is_ascii_digit() {
                num.push(b[j]);
                j += 1;
            }
            if !num.is_empty() {
                return num.parse().ok();
            }
        }
        i += 1;
    }
    None
}

fn first_issue_ref(body: &[String]) -> Option<u32> {
    body.iter().find_map(|l| first_issue_ref_in(l))
}

fn first_body_text(body: &[String]) -> String {
    body.iter()
        .map(|l| l.trim().trim_start_matches('-').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// パースした作業ログ
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedLog {
    /// 最初の日付見出しより前（タイトル・書き方の説明など）
    pub preamble: String,
    /// 出現順（古いものが先）
    pub entries: Vec<LogEntry>,
}

impl ParsedLog {
    /// 相異なる日付の数
    pub fn work_days(&self) -> usize {
        let mut days: Vec<&str> = self.entries.iter().map(|e| e.date.as_str()).collect();
        days.sort_unstable();
        days.dedup();
        days.len()
    }
}

/// 見出し行から日付を採る（`## 2026-08-24（…）`）
fn heading_date(line: &str) -> Option<String> {
    let rest = line.strip_prefix("## ")?;
    let c: Vec<char> = rest.chars().take(10).collect();
    if c.len() == 10
        && c[..4].iter().all(|x| x.is_ascii_digit())
        && c[4] == '-'
        && c[5..7].iter().all(|x| x.is_ascii_digit())
        && c[7] == '-'
        && c[8..10].iter().all(|x| x.is_ascii_digit())
    {
        Some(c.iter().collect())
    } else {
        None
    }
}

/// `## YYYY-MM-DD` 見出しでエントリへ割る。**日付見出しでないものは本文の一部**
/// （エントリの中に `### 小見出し` があっても割れない）
pub fn parse_log(text: &str) -> ParsedLog {
    let mut out = ParsedLog::default();
    let mut preamble: Vec<&str> = Vec::new();
    let mut cur: Option<LogEntry> = None;
    for line in text.lines() {
        if let Some(date) = heading_date(line) {
            if let Some(e) = cur.take() {
                out.entries.push(trim_trailing(e));
            }
            cur = Some(LogEntry {
                date,
                heading: line.to_string(),
                body: Vec::new(),
            });
        } else if let Some(e) = cur.as_mut() {
            e.body.push(line.to_string());
        } else {
            preamble.push(line);
        }
    }
    if let Some(e) = cur.take() {
        out.entries.push(trim_trailing(e));
    }
    while preamble.last().is_some_and(|l| l.trim().is_empty()) {
        preamble.pop();
    }
    out.preamble = preamble.join("\n");
    out
}

fn trim_trailing(mut e: LogEntry) -> LogEntry {
    while e.body.last().is_some_and(|l| l.trim().is_empty()) {
        e.body.pop();
    }
    e
}

// ─── 移送計画 ──────────────────────────────────────────────────────────

/// `fix` の計画（**適用しても消えるものは無い**: 移したエントリは
/// アーカイブへ 1 行で残り、全文は git 履歴に残る）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePlan {
    /// progress.md に残すエントリ（出現順）
    pub kept: Vec<LogEntry>,
    /// アーカイブへ移すエントリ（出現順）
    pub archived: Vec<LogEntry>,
    /// 保持期間を過ぎてアーカイブから消す行
    pub aged_out: Vec<String>,
    /// 適用後の progress.md
    pub progress_text: String,
    /// 適用後のアーカイブ
    pub archive_text: String,
    /// 適用前のエントリ数
    pub entries_before: usize,
}

impl PrunePlan {
    /// 何も変わらないか（冪等性の判定に使う）
    pub fn is_noop(&self) -> bool {
        self.archived.is_empty() && self.aged_out.is_empty()
    }

    /// 移送でエントリが失われていないこと。
    /// **`fix` はこれを満たさなければ書き込まない**
    pub fn entries_preserved(&self) -> bool {
        self.kept.len() + self.archived.len() == self.entries_before
    }
}

/// アーカイブの 1 エントリ（新形式の 1 行 / 旧形式の `## 見出し` + 本文 の両方を読む）
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveEntry {
    date: String,
    lines: Vec<String>,
}

fn parse_archive(text: &str) -> (String, Vec<ArchiveEntry>) {
    let mut preamble: Vec<&str> = Vec::new();
    let mut entries: Vec<ArchiveEntry> = Vec::new();
    let mut cur: Option<ArchiveEntry> = None;
    for line in text.lines() {
        if let Some(date) = heading_date(line) {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            cur = Some(ArchiveEntry {
                date,
                lines: vec![line.to_string()],
            });
        } else if let Some(date) = one_line_date(line) {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            entries.push(ArchiveEntry {
                date,
                lines: vec![line.to_string()],
            });
        } else if let Some(e) = cur.as_mut() {
            e.lines.push(line.to_string());
        } else {
            preamble.push(line);
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e);
    }
    while preamble.last().is_some_and(|l| l.trim().is_empty()) {
        preamble.pop();
    }
    for e in &mut entries {
        while e.lines.last().is_some_and(|l| l.trim().is_empty()) {
            e.lines.pop();
        }
    }
    (preamble.join("\n"), entries)
}

/// `- YYYY-MM-DD …` の 1 行形式から日付を採る
fn one_line_date(line: &str) -> Option<String> {
    let rest = line.strip_prefix("- ")?;
    let c: Vec<char> = rest.chars().take(10).collect();
    if c.len() == 10
        && c[..4].iter().all(|x| x.is_ascii_digit())
        && c[4] == '-'
        && c[5..7].iter().all(|x| x.is_ascii_digit())
        && c[7] == '-'
        && c[8..10].iter().all(|x| x.is_ascii_digit())
    {
        Some(c.iter().collect())
    } else {
        None
    }
}

/// `YYYY-MM-DD` → 1970-01-01 からの日数。読めなければ `None`
pub fn date_to_days(date: &str) -> Option<i64> {
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(crate::limit_resume::days_from_civil(y, m, d))
}

/// 1970-01-01 からの日数 → `YYYY-MM-DD`
pub fn days_to_date(days: i64) -> String {
    let (y, m, d) = crate::limit_resume::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 移送計画を立てる（純粋関数。ファイルには触れない）。
///
/// - `today_days`: 「今日」の 1970-01-01 からの日数（保持期間の判定に使う）
/// - 残すのは **新しい方から** 作業日 / 件数 / バイトのすべてに収まるところまで
/// - 収まらなかったものは出現順のままアーカイブへ 1 行で積む
pub fn plan_prune(progress: &str, archive: &str, today_days: i64) -> PrunePlan {
    let lim = limits(ItemKind::ProgressLog);
    let log = parse_log(progress);
    let entries_before = log.entries.len();

    // 1) 新しい方から max_work_days ぶんの日付だけを候補にする
    let mut days: Vec<&str> = log.entries.iter().map(|e| e.date.as_str()).collect();
    days.sort_unstable();
    days.dedup();
    let allowed: Vec<&str> = days
        .iter()
        .rev()
        .take(lim.max_work_days.unwrap_or(usize::MAX))
        .copied()
        .collect();

    // 2) 末尾（新しい方）から件数とバイトに収まるだけ残す。
    //    preamble も毎ターン読まれるのでバイト予算に含める
    let preamble_bytes = log.preamble.len() + 2;
    let max_bytes = lim.max_bytes.unwrap_or(usize::MAX);
    let max_entries = lim.max_entries.unwrap_or(usize::MAX);
    let mut used = preamble_bytes;
    let mut keep_flags = vec![false; log.entries.len()];
    let mut kept_count = 0usize;
    for (i, e) in log.entries.iter().enumerate().rev() {
        if !allowed.contains(&e.date.as_str()) {
            continue;
        }
        if kept_count >= max_entries {
            break;
        }
        let cost = e.render().len() + 1;
        // **1 件も残らない結果にはしない**: 1 エントリだけでバイト予算を超える場合でも
        // 最も新しい 1 件は残す（作業ログを黙って空にするより、超過として報告した方がよい。
        // その 1 件を短く書き直すのは人間の仕事なので `check` が提案として名指しする）
        if used + cost > max_bytes && kept_count > 0 {
            break;
        }
        used += cost;
        kept_count += 1;
        keep_flags[i] = true;
    }

    let mut kept = Vec::new();
    let mut archived = Vec::new();
    for (i, e) in log.entries.iter().enumerate() {
        if keep_flags[i] {
            kept.push(e.clone());
        } else {
            archived.push(e.clone());
        }
    }

    // 3) アーカイブ: 既存 + 今回ぶん。保持期間を過ぎたものを落とす
    let (arch_preamble, existing) = parse_archive(archive);
    let cutoff = today_days - ARCHIVE_RETAIN_DAYS;
    let mut aged_out = Vec::new();
    // エントリ 1 件 = 1 グループ（旧形式は見出し + 本文の複数行になる）
    let mut groups: Vec<Vec<String>> = Vec::new();
    for e in existing {
        let too_old = date_to_days(&e.date).is_some_and(|d| d < cutoff);
        if too_old {
            aged_out.push(e.lines.first().cloned().unwrap_or_default());
        } else {
            groups.push(e.lines);
        }
    }
    // 既に載っている 1 行はもう積まない。`fix` はアーカイブ → 本体の順に書くので、
    // 本体の書き込みだけ失敗した状態でやり直すと**同じ行が二重に積まれる**
    // （アーカイブ側だけ進んだ状態から復帰できる形にしておく）
    let existing_lines: std::collections::BTreeSet<&str> = groups
        .iter()
        .filter(|g| g.len() == 1)
        .map(|g| g[0].as_str())
        .collect();
    let mut appended: Vec<Vec<String>> = Vec::new();
    for e in &archived {
        let line = e.archive_line();
        if date_to_days(&e.date).is_some_and(|d| d < cutoff) {
            aged_out.push(line);
            continue;
        }
        // 突き合わせるのは**この回より前から在った行**だけ。同じ回の中では畳まない
        // （見出しが同じエントリが 2 件あるとき、1 件に減らしてしまわないため）
        if existing_lines.contains(line.as_str()) {
            continue;
        }
        appended.push(vec![line]);
    }
    groups.extend(appended);

    let progress_text = render_log(&log.preamble, &kept);
    let archive_text = render_archive(&arch_preamble, &groups);

    PrunePlan {
        kept,
        archived,
        aged_out,
        progress_text,
        archive_text,
        entries_before,
    }
}

fn render_log(preamble: &str, kept: &[LogEntry]) -> String {
    let mut s = String::new();
    if !preamble.trim().is_empty() {
        s.push_str(preamble.trim_end());
        s.push_str("\n\n");
    }
    for (i, e) in kept.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&e.render());
    }
    s
}

fn render_archive(preamble: &str, groups: &[Vec<String>]) -> String {
    let mut s = String::new();
    if !preamble.trim().is_empty() {
        s.push_str(preamble.trim_end());
        s.push_str("\n\n");
    }
    let mut prev_multi = false;
    for g in groups {
        // 複数行のエントリ（旧形式の見出し + 本文）の前後は空行で区切る。
        // 1 行エントリどうしは詰めて並べる
        let multi = g.len() > 1;
        if (multi || prev_multi) && !s.ends_with("\n\n") && !s.is_empty() {
            s.push('\n');
        }
        for l in g {
            s.push_str(l);
            s.push('\n');
        }
        prev_multi = multi;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_fixture(days: &[(&str, usize)]) -> String {
        let mut s = String::from("# Progress Log\n\n> 説明\n");
        for (date, n) in days {
            for i in 0..*n {
                s.push_str(&format!("\n## {date}（件 {i}）\n- 何を / どこを / 結果\n"));
            }
        }
        s
    }

    #[test]
    fn 概算トークンは日本語主体の実測に合わせてある() {
        // 実測: tako の progress.md は 233,519 chars で約 18 万トークンだった。
        // 手法を変えたらこの比が崩れるので固定する
        let ja = "日本語".repeat(1300);
        assert_eq!(estimate_tokens(&ja), 3000);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn 見出しからエントリと作業日を数える() {
        let text = log_fixture(&[("2026-09-01", 2), ("2026-09-02", 1)]);
        let log = parse_log(&text);
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.work_days(), 2);
        assert!(log.preamble.starts_with("# Progress Log"));
        assert_eq!(log.entries[0].date, "2026-09-01");
    }

    #[test]
    fn 日付でない見出しはエントリを割らない() {
        let text = "# t\n\n## 2026-09-01（あ）\n- x\n\n### 小見出し\n- y\n\n## 参考\n- z\n";
        let log = parse_log(text);
        assert_eq!(log.entries.len(), 1, "日付見出しは 1 個だけ");
        // `### 小見出し` も `## 参考` も本文の一部として保持される
        assert!(log.entries[0].body.iter().any(|l| l == "### 小見出し"));
        assert!(log.entries[0].body.iter().any(|l| l == "## 参考"));
    }

    #[test]
    fn アーカイブの1行は見出しから採り要約を作らない() {
        let e = LogEntry {
            date: "2026-08-24".into(),
            heading: "## 2026-08-24（#927 追い込み: 実名マスクを機械的に完了）".into(),
            body: vec!["- 本文".into()],
        };
        assert_eq!(
            e.archive_line(),
            "- 2026-08-24 #927 追い込み: 実名マスクを機械的に完了"
        );
    }

    #[test]
    fn 見出しに番号が無ければ本文の最初の番号を添える() {
        let e = LogEntry {
            date: "2026-08-24".into(),
            heading: "## 2026-08-24（プレビュー修正）".into(),
            body: vec!["- Issue #123 を直した".into()],
        };
        assert_eq!(e.archive_line(), "- 2026-08-24 #123 プレビュー修正");
    }

    #[test]
    fn 長すぎる見出しは機械的に切り詰める() {
        let e = LogEntry {
            date: "2026-08-24".into(),
            heading: format!("## 2026-08-24（{}）", "あ".repeat(300)),
            body: vec![],
        };
        let line = e.archive_line();
        assert_eq!(line.chars().count(), ARCHIVE_LINE_MAX_CHARS);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn 予算内なら移送しない_冪等() {
        let text = log_fixture(&[("2026-09-02", 1), ("2026-09-03", 1)]);
        let plan = plan_prune(&text, "", date_to_days("2026-09-06").unwrap());
        assert!(plan.is_noop(), "予算内では何も動かない");
        assert_eq!(plan.kept.len(), 2);
        assert!(plan.entries_preserved());
        // 2 回目も同じ
        let again = plan_prune(&plan.progress_text, &plan.archive_text, 20_000);
        assert!(again.is_noop());
        assert_eq!(again.progress_text, plan.progress_text);
    }

    #[test]
    fn 作業日の超過は古い方から落ちる() {
        let text = log_fixture(&[
            ("2026-08-20", 1),
            ("2026-09-01", 1),
            ("2026-09-02", 1),
            ("2026-09-03", 1),
            ("2026-09-04", 1),
            ("2026-09-05", 1),
        ]);
        let plan = plan_prune(&text, "", date_to_days("2026-09-06").unwrap());
        assert_eq!(plan.kept.len(), PROGRESS_MAX_WORK_DAYS);
        assert_eq!(plan.archived.len(), 1);
        assert_eq!(plan.archived[0].date, "2026-08-20", "最も古い日が落ちる");
        assert!(plan.entries_preserved());
    }

    #[test]
    fn 件数の超過は新しい方から残す() {
        let text = log_fixture(&[("2026-09-03", 30)]);
        let plan = plan_prune(&text, "", date_to_days("2026-09-06").unwrap());
        assert_eq!(plan.kept.len(), PROGRESS_MAX_ENTRIES);
        assert_eq!(plan.archived.len(), 10);
        assert!(plan.entries_preserved());
        // 残ったのは新しい方（= ファイル末尾側）
        assert!(plan.kept[0].heading.contains("件 10"));
    }

    #[test]
    fn バイト超過でも1件も失わない() {
        let big = format!(
            "# t\n{}",
            (0..6)
                .map(|i| format!(
                    "\n## 2026-09-0{}（大）\n{}\n",
                    i + 1,
                    "- ".to_string() + &"あ".repeat(2000)
                ))
                .collect::<String>()
        );
        let plan = plan_prune(&big, "", date_to_days("2026-09-06").unwrap());
        assert!(
            plan.progress_text.len() <= PROGRESS_MAX_BYTES,
            "予算内へ収まる"
        );
        assert!(plan.entries_preserved(), "移しても総数は保たれる");
        assert!(!plan.archived.is_empty());
    }

    #[test]
    fn アーカイブ側だけ書けた状態からやり直しても二重に積まない() {
        // fix はアーカイブ → 本体 の順に書く。本体の書き込みが失敗した状態でやり直すと、
        // 素朴な実装では同じ 1 行が 2 度積まれる
        let text = log_fixture(&[
            ("2026-08-20", 1),
            ("2026-09-01", 1),
            ("2026-09-02", 1),
            ("2026-09-03", 1),
            ("2026-09-04", 1),
            ("2026-09-05", 1),
        ]);
        let today = date_to_days("2026-09-06").unwrap();
        let first = plan_prune(&text, "", today);
        assert_eq!(first.archived.len(), 1);
        // アーカイブだけ書けた（= 本体は 332 件のまま）状態でもう一度計画を立てる
        let again = plan_prune(&text, &first.archive_text, today);
        assert_eq!(
            again.archive_text, first.archive_text,
            "同じ行が二重に積まれない"
        );
    }

    #[test]
    fn 見出しが同じエントリが2件あっても1件に畳まない() {
        // 実データに在る形（同じ日・同じ見出しで本文が違う 2 件）を、
        // 直近 5 作業日より古い位置に置いて移送対象にする
        let mut text = String::from(
            "# t\n\n## 2026-08-01（同じ見出し）\n- 本文 A\n\n\
             ## 2026-08-01（同じ見出し）\n- 本文 B\n",
        );
        for d in 1..=5 {
            text.push_str(&format!("\n## 2026-09-0{d}（新しい）\n- x\n"));
        }
        let plan = plan_prune(&text, "", date_to_days("2026-09-06").unwrap());
        assert_eq!(plan.archived.len(), 2, "2 件とも移送される");
        let lines = plan.archive_text.matches("同じ見出し").count();
        assert_eq!(lines, 2, "アーカイブにも 2 行 = 1 エントリ 1 行");
        assert!(plan.entries_preserved());
    }

    #[test]
    fn エントリ1件でバイト予算を超えても作業ログを空にしない() {
        // 予算まるごとを超える 1 エントリ。空にするのではなく残して超過を報告する
        let text = format!(
            "# t\n\n## 2026-09-01（古い）\n- x\n\n## 2026-09-05（巨大）\n- {}\n",
            "あ".repeat(PROGRESS_MAX_BYTES)
        );
        let plan = plan_prune(&text, "", date_to_days("2026-09-06").unwrap());
        assert_eq!(plan.kept.len(), 1, "最も新しい 1 件は残る");
        assert_eq!(plan.kept[0].date, "2026-09-05");
        assert!(plan.entries_preserved());
        // そのうえで超過として報告される（自動では直せない = 書き直しは人間の仕事）
        let m = measure(ItemKind::ProgressLog, &plan.progress_text);
        assert!(m.bytes > PROGRESS_MAX_BYTES);
    }

    #[test]
    fn 保持期間を過ぎたアーカイブ行は消える() {
        let archive = "# Progress Archive\n\n- 2026-01-01 #1 とても古い\n- 2026-09-01 #2 最近\n";
        let plan = plan_prune("# t\n", archive, date_to_days("2026-09-06").unwrap());
        assert_eq!(plan.aged_out.len(), 1);
        assert!(plan.aged_out[0].contains("とても古い"));
        assert!(plan.archive_text.contains("最近"));
        assert!(!plan.archive_text.contains("とても古い"));
    }

    #[test]
    fn 旧形式のアーカイブも1エントリとして読む() {
        let archive = "# A\n\n## 2026-01-01（古い）\n- 本文\n\n- 2026-09-01 #2 新形式\n";
        let (pre, entries) = parse_archive(archive);
        assert_eq!(pre, "# A");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].lines.len(), 2, "見出し + 本文");
        assert_eq!(entries[1].date, "2026-09-01");
    }

    #[test]
    fn 判定は予算表を引く() {
        let text = log_fixture(&[("2026-09-03", 40)]);
        let m = measure(ItemKind::ProgressLog, &text);
        let v = violations(ItemKind::ProgressLog, &m);
        let metrics: Vec<Metric> = v.iter().map(|x| x.metric).collect();
        assert!(metrics.contains(&Metric::Entries));
        assert!(v.iter().all(|x| x.actual > x.limit));
        // 件数超過は fix で直せる。1 エントリの行数は直せない
        assert!(
            v.iter()
                .find(|x| x.metric == Metric::Entries)
                .unwrap()
                .fixable
        );
    }

    #[test]
    fn エントリ1件の行数超過は自動修正の対象にしない() {
        let text = "# t\n\n## 2026-09-03（長い）\n- 1\n- 2\n- 3\n- 4\n- 5\n";
        let m = measure(ItemKind::ProgressLog, text);
        assert_eq!(m.longest_entry_lines, Some(5));
        assert_eq!(m.over_long_entries, Some(1));
        let v = violations(ItemKind::ProgressLog, &m);
        let entry_lines = v.iter().find(|x| x.metric == Metric::EntryLines).unwrap();
        assert!(
            !entry_lines.fixable,
            "本文を削るのは要約の捏造になるので自動では直さない"
        );
    }

    #[test]
    fn 作業ログ以外は自動修正の対象外() {
        for k in [
            ItemKind::GlobalGuide,
            ItemKind::AgentsGuide,
            ItemKind::ActiveContext,
            ItemKind::Imported,
            ItemKind::HandoffMemo,
            ItemKind::SystemPrompt,
            ItemKind::Memory,
        ] {
            assert!(!k.auto_fixable(), "{} は人間の判断が要る", k.as_str());
        }
        assert!(ItemKind::ProgressLog.auto_fixable());
    }

    #[test]
    fn system_promptは予算対象で自動修正の対象外() {
        // #1154: tako 自身が配る生成物なので上限を持つ。ただし本文を機械で削るのは
        // 手順の欠落になるので、直し方は `proposals`（guide への移送）として返すだけ
        assert_eq!(
            limits(ItemKind::SystemPrompt).max_bytes,
            Some(SYSTEM_PROMPT_MAX_BYTES),
            "TAKO_1154_LEGACY が立っていると予算が外れる（A/B の期待どおり）"
        );
        let m = measure(
            ItemKind::SystemPrompt,
            &"x".repeat(SYSTEM_PROMPT_MAX_BYTES + 1),
        );
        let v = violations(ItemKind::SystemPrompt, &m);
        let bytes = v
            .iter()
            .find(|x| x.metric == Metric::Bytes)
            .expect("バイト超過を名指しする");
        assert_eq!(bytes.limit, SYSTEM_PROMPT_MAX_BYTES);
        assert!(!bytes.fixable, "本文の削りは自動化しない");
        assert!(
            bytes.note.ja().contains("guide"),
            "直し方に guide への移送を書く: {}",
            bytes.note.ja()
        );
        // 予算内なら黙る
        let ok = measure(ItemKind::SystemPrompt, &"x".repeat(SYSTEM_PROMPT_MAX_BYTES));
        assert!(violations(ItemKind::SystemPrompt, &ok).is_empty());
    }

    #[test]
    fn 取り込み合計の判定() {
        assert!(import_total_violation(IMPORT_TOTAL_MAX_BYTES).is_none());
        let v = import_total_violation(IMPORT_TOTAL_MAX_BYTES + 1).unwrap();
        assert_eq!(v.metric, Metric::ImportTotalBytes);
        assert!(!v.fixable);
    }

    #[test]
    fn 日付の往復() {
        for d in ["2026-09-06", "2026-01-01", "2024-02-29"] {
            assert_eq!(days_to_date(date_to_days(d).unwrap()), d);
        }
        assert!(date_to_days("2026-13-01").is_none());
        assert!(date_to_days("not-a-date").is_none());
    }

    #[test]
    fn 理由文は日英とも空でない() {
        let m = measure(ItemKind::ProgressLog, &log_fixture(&[("2026-09-03", 40)]));
        for v in violations(ItemKind::ProgressLog, &m) {
            assert!(!v.note.ja().is_empty() && !v.note.en().is_empty());
        }
        for k in [
            ItemKind::GlobalGuide,
            ItemKind::AgentsGuide,
            ItemKind::ProgressLog,
            ItemKind::ActiveContext,
            ItemKind::Imported,
            ItemKind::HandoffMemo,
            ItemKind::SystemPrompt,
            ItemKind::Memory,
        ] {
            assert!(!k.label().ja().is_empty() && !k.label().en().is_empty());
            assert!(!k.as_str().is_empty());
        }
    }
}

//! 使用量の索引の型と判断（usage 分析 S2 #2201。設計書 `.agent/plans/2026-10-usage-analytics.md`
//! §1.1 / §3.2 / §3.3 / §5.2 / §6）。ここは**純関数と型だけ**（ファイルの走査・読み書き・鍵の材料の
//! 読み込みは `tako_control::usage::index`）。
//!
//! # 読むもの（§1.1）
//!
//! claude の会話記録（`<設定 dir>/projects/**/<sessionId>.jsonl`。サブエージェントは
//! `<sessionId>/subagents/agent-*.jsonl` で、行の `sessionId` は親の会話）の **assistant 行の
//! 使用量のメタデータだけ**。読み手の構造体（`RawLine` ほか）は `message.id` / `requestId` /
//! `timestamp` / `message.model` / `message.usage` の 5 成分と `speed` / `sessionId` / `cwd` / `uuid`
//! しか持たず、**`message.content` や `wireToolInputs` は serde が読み飛ばして値を持ち出さない**（§6）。
//! `cwd` は鍵（プロジェクト）を決めるのに使うだけで、索引へは書かない。
//!
//! # 数え方
//!
//! - 1 応答は content ブロックごとに複数行へ分かれ（平均 2.1 行）、**同じ `message.id` の行は
//!   ファイルの中で必ず連続する**。先の行の `output_tokens` はストリーミング途中の仮の値なので、
//!   連続する同じ id の行は**成分ごとの最大**へ寄せ、増えた分だけを足す（[`LastMessage`]。
//!   ccusage の「総量の大きい方へ差し替える」と、output だけが違うこの形では同じ値になる）
//! - 重複の鍵は全体で 1 つ（[`dedup_key`]）。Anthropic 形式の `msg_…` はその id だけ、それ以外は
//!   id + requestId + sessionId + 時刻。`/btw` の写しや、縮んだファイルの読み直しで同じ応答を
//!   2 回数えない
//! - `<synthetic>`（エラー時の合成応答）は数えない
//! - 5 分刻み（[`BUCKET_SECS`]）× 会話 × モデル × fast で集約する（[`BucketRow`]）。金額は持たず、
//!   問い合わせのたびに単価表（`super::price`）で計算する

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::KeyFrom;

/// S2 の A/B: 立っていれば使用量の索引を作らない・読まない（#2201 前 = S1 の観測の要約だけ）
pub const S2_LEGACY_ENV: &str = "TAKO_2192_S2_LEGACY";
/// 集約の刻み（秒）
pub const BUCKET_SECS: i64 = 300;
/// エラー時の合成応答のモデル名（usage は 0。数えない）
pub const SYNTHETIC_MODEL: &str = "<synthetic>";

/// S2 の A/B（[`S2_LEGACY_ENV`]）が立っているか
pub fn s2_legacy() -> bool {
    std::env::var_os(S2_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 使用量の 5 成分（トークン数）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    /// キャッシュ作成（5 分）
    #[serde(default)]
    pub cache_5m: u64,
    /// キャッシュ作成（1 時間）
    #[serde(default)]
    pub cache_1h: u64,
    /// キャッシュ読み出し
    #[serde(default)]
    pub cache_read: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_5m + self.cache_1h + self.cache_read
    }

    pub fn is_zero(&self) -> bool {
        self.total() == 0
    }

    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_5m += other.cache_5m;
        self.cache_1h += other.cache_1h;
        self.cache_read += other.cache_read;
    }

    /// 成分ごとの最大（仮の値と確定値を寄せる）
    pub fn max(&self, other: &Tokens) -> Tokens {
        Tokens {
            input: self.input.max(other.input),
            output: self.output.max(other.output),
            cache_5m: self.cache_5m.max(other.cache_5m),
            cache_1h: self.cache_1h.max(other.cache_1h),
            cache_read: self.cache_read.max(other.cache_read),
        }
    }

    /// 成分ごとの差（負にならない）
    pub fn saturating_sub(&self, other: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(other.input),
            output: self.output.saturating_sub(other.output),
            cache_5m: self.cache_5m.saturating_sub(other.cache_5m),
            cache_1h: self.cache_1h.saturating_sub(other.cache_1h),
            cache_read: self.cache_read.saturating_sub(other.cache_read),
        }
    }
}

// --- 読み手（§1.1 の表のキーだけを持つ。本文系のキーは serde が読み飛ばす）-----------------------

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
    #[serde(rename = "requestId", default)]
    request_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation: Option<RawCacheCreation>,
    #[serde(default)]
    speed: Option<String>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
}

/// assistant 行 1 本から読んだ使用量
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantUsage {
    pub message_id: Option<String>,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    /// 行の時刻（unix 秒）
    pub at: Option<i64>,
    pub model: String,
    pub fast: bool,
    pub tokens: Tokens,
    /// 鍵（プロジェクト）を決める材料。**索引には書かない**
    pub cwd: Option<String>,
    pub line_uuid: Option<String>,
}

/// 部分一致の絞り込み（assistant 行は全体の約 21%。パースはそれだけに掛ける = 設計書 §5.3）
fn assistant_needle() -> &'static regex::bytes::Regex {
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::bytes::Regex::new(&regex::escape("\"type\":\"assistant\"")).expect("固定の正規表現")
    })
}

/// 1 行を読む。assistant の使用量でなければ `None`（他の種類の行・壊れた行・usage の無い行）。
/// `<synthetic>` は返す（数えないのは呼び手 = 件数を数えるため）
pub fn parse_assistant_line(line: &[u8]) -> Option<AssistantUsage> {
    if !assistant_needle().is_match(line) {
        return None;
    }
    let raw: RawLine = serde_json::from_slice(line).ok()?;
    if raw.kind.as_deref() != Some("assistant") {
        return None;
    }
    let message = raw.message?;
    let usage = message.usage?;
    let total_creation = usage.cache_creation_input_tokens.unwrap_or(0);
    let (c5, c1) = match &usage.cache_creation {
        Some(cc) => (
            cc.ephemeral_5m_input_tokens.unwrap_or(0),
            cc.ephemeral_1h_input_tokens.unwrap_or(0),
        ),
        None => (0, 0),
    };
    // 内訳が無い（古い版）・内訳の和が総量に足りないときは、足りない分を 5 分として数える
    let cache_5m = c5.max(total_creation.saturating_sub(c1));
    Some(AssistantUsage {
        message_id: message.id.filter(|s| !s.is_empty()),
        request_id: raw.request_id.filter(|s| !s.is_empty()),
        session_id: raw.session_id.filter(|s| !s.is_empty()),
        at: raw
            .timestamp
            .as_deref()
            .and_then(crate::claude_mod::parse_resets_at),
        model: message
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        fast: usage.speed.as_deref() == Some("fast"),
        tokens: Tokens {
            input: usage.input_tokens.unwrap_or(0),
            output: usage.output_tokens.unwrap_or(0),
            cache_5m,
            cache_1h: c1,
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
        },
        cwd: raw.cwd.filter(|s| !s.is_empty()),
        line_uuid: raw.uuid,
    })
}

/// 64 ビットの FNV-1a（版をまたいで同じ値になる = 永続する鍵に使う。`DefaultHasher` は保証が無い）
pub fn fnv64(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            h ^= u64::from(b'\n');
            h = h.wrapping_mul(PRIME);
        }
        for b in part.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(PRIME);
        }
    }
    h
}

/// 16 進 16 桁
pub fn hex64(v: u64) -> String {
    format!("{v:016x}")
}

pub fn parse_hex64(s: &str) -> Option<u64> {
    u64::from_str_radix(s, 16).ok()
}

/// 応答の重複の鍵（全体で 1 つ。§1.1）
pub fn dedup_key(u: &AssistantUsage) -> u64 {
    let at = u.at.map(|a| a.to_string()).unwrap_or_default();
    match u.message_id.as_deref() {
        Some(id) if id.starts_with("msg_") => fnv64(&["msg", id]),
        Some(id) => fnv64(&[
            "id",
            id,
            u.request_id.as_deref().unwrap_or_default(),
            u.session_id.as_deref().unwrap_or_default(),
            &at,
        ]),
        None => fnv64(&[
            "line",
            u.request_id.as_deref().unwrap_or_default(),
            u.line_uuid.as_deref().unwrap_or_default(),
            u.session_id.as_deref().unwrap_or_default(),
            &at,
        ]),
    }
}

/// 時刻 → 刻みの頭
pub fn bucket_of(at: i64) -> i64 {
    at.div_euclid(BUCKET_SECS) * BUCKET_SECS
}

/// ファイルの最後に数えた応答（続きの行で output が増えたら差分だけ足すための覚え）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastMessage {
    pub id: String,
    /// 刻みの頭（unix 秒）
    pub t: i64,
    pub session: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast: bool,
    pub tokens: Tokens,
}

/// 集約の鍵
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BucketKey {
    pub t: i64,
    pub session: String,
    pub model: String,
    pub fast: bool,
}

/// 会話を初めて見た時刻と鍵の材料（cwd は鍵を決めるのに使うだけで書かない）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionSeen {
    pub first_at: Option<i64>,
    pub cwd: Option<String>,
}

/// 1 回の更新で読んだ分の集約
#[derive(Debug, Clone, Default)]
pub struct Batch {
    pub buckets: BTreeMap<BucketKey, (Tokens, u32)>,
    /// 触った会話（鍵を写す対象）
    pub sessions: BTreeMap<String, SessionSeen>,
    /// 新しく数えた応答
    pub responses: u64,
    /// 続きの行で増えた分を足した回数
    pub continued: u64,
    /// 別の場所で数え済みの写し
    pub duplicates: u64,
    pub synthetic: u64,
    /// 時刻の無い assistant 行（数えない）
    pub no_time: u64,
}

impl Batch {
    fn add(&mut self, key: BucketKey, tokens: &Tokens, responses: u32) {
        let entry = self.buckets.entry(key).or_default();
        entry.0.add(tokens);
        entry.1 += responses;
    }

    fn touch(&mut self, session: &str, at: i64, cwd: Option<&str>) {
        let entry = self.sessions.entry(session.to_string()).or_default();
        entry.first_at = Some(entry.first_at.map_or(at, |f| f.min(at)));
        if entry.cwd.is_none() {
            entry.cwd = cwd.map(str::to_string);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty() && self.sessions.is_empty()
    }

    /// 別の集約（1 ファイルぶん）を足し込む
    pub fn absorb(&mut self, other: Batch) {
        for (key, (tokens, n)) in other.buckets {
            self.add(key, &tokens, n);
        }
        for (sid, seen) in other.sessions {
            let entry = self.sessions.entry(sid).or_default();
            entry.first_at = match (entry.first_at, seen.first_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if entry.cwd.is_none() {
                entry.cwd = seen.cwd;
            }
        }
        self.responses += other.responses;
        self.continued += other.continued;
        self.duplicates += other.duplicates;
        self.synthetic += other.synthetic;
        self.no_time += other.no_time;
    }

    /// 追記する行（刻み → 会話 → モデルの順）
    pub fn rows(&self) -> Vec<BucketRow> {
        self.buckets
            .iter()
            .filter(|(_, (tokens, n))| !tokens.is_zero() || *n > 0)
            .map(|(k, (tokens, n))| BucketRow {
                t: k.t,
                session: k.session.clone(),
                model: k.model.clone(),
                fast: k.fast,
                tokens: *tokens,
                responses: *n,
            })
            .collect()
    }
}

/// 1 ファイルぶんの読みの状態（[`ingest_lines`] が進める）
pub struct FileProgress<'a> {
    pub last: &'a mut Option<LastMessage>,
    pub seen: &'a mut HashSet<u64>,
    /// このファイルで新しく数えた応答の鍵（読み位置と一緒に書く）
    pub new_ids: &'a mut Vec<u64>,
    /// 行に `sessionId` が無いときの会話（ファイル名）
    pub fallback_session: &'a str,
}

/// 改行で終わった行だけを読み、読んだバイト数（最後の改行の次まで）を返す。
/// **書きかけの最終行（改行が無い）は読まずに残す**（次の更新で続きと一緒に読む）
pub fn ingest_lines(data: &[u8], progress: FileProgress<'_>, batch: &mut Batch) -> usize {
    let FileProgress {
        last,
        seen,
        new_ids,
        fallback_session,
    } = progress;
    let mut consumed = 0usize;
    let mut rest = data;
    while let Some(nl) = rest.iter().position(|b| *b == b'\n') {
        let line = &rest[..nl];
        rest = &rest[nl + 1..];
        consumed += nl + 1;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some(u) = parse_assistant_line(line) else {
            continue;
        };
        if u.model == SYNTHETIC_MODEL {
            batch.synthetic += 1;
            continue;
        }
        let Some(at) = u.at else {
            batch.no_time += 1;
            continue;
        };
        let session = u
            .session_id
            .clone()
            .unwrap_or_else(|| fallback_session.to_string());
        // 同じ応答の続きの行（ファイル内で連続する）: 成分ごとの最大へ寄せ、増えた分だけ足す
        if let (Some(prev), Some(id)) = (last.as_mut(), u.message_id.as_deref()) {
            if prev.id == id && prev.session == session {
                let merged = prev.tokens.max(&u.tokens);
                let delta = merged.saturating_sub(&prev.tokens);
                if !delta.is_zero() {
                    batch.continued += 1;
                    let key = BucketKey {
                        t: prev.t,
                        session: prev.session.clone(),
                        model: prev.model.clone(),
                        fast: prev.fast,
                    };
                    batch.add(key, &delta, 0);
                    prev.tokens = merged;
                }
                continue;
            }
        }
        let key = dedup_key(&u);
        if !seen.insert(key) {
            // 別の場所（写し・読み直し）で数え済み
            batch.duplicates += 1;
            *last = None;
            continue;
        }
        new_ids.push(key);
        batch.responses += 1;
        let t = bucket_of(at);
        batch.add(
            BucketKey {
                t,
                session: session.clone(),
                model: u.model.clone(),
                fast: u.fast,
            },
            &u.tokens,
            1,
        );
        batch.touch(&session, at, u.cwd.as_deref());
        *last = u.message_id.clone().map(|id| LastMessage {
            id,
            t,
            session,
            model: u.model,
            fast: u.fast,
            tokens: u.tokens,
        });
    }
    consumed
}

// --- 永続する行 -------------------------------------------------------------------------------

/// `buckets/<YYYY-MM>.jsonl` の 1 行（5 分刻み × 会話 × モデル × fast の 5 成分と応答数）。
/// 追記だけで、読むときに同じ鍵を足し合わせる（[`merge_rows`]）。金額は持たない
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BucketRow {
    /// 刻みの頭（unix 秒。古い月を日単位へまとめた後は日の頭）
    pub t: i64,
    pub session: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast: bool,
    pub tokens: Tokens,
    #[serde(default)]
    pub responses: u32,
}

/// `cursor.jsonl` の 1 行（transcript 1 本の読み位置）。**後ろの行が正**で、`ids` は
/// その行で新しく数えた応答の鍵（16 進）。読むときに同じファイルの全行の `ids` を合わせる
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorRow {
    /// transcript の置き場の鍵（絶対パスの [`fnv64`]。パスそのものは書かない）
    pub file: String,
    #[serde(default)]
    pub dev: u64,
    #[serde(default)]
    pub ino: u64,
    /// 読んだバイト位置（最後の改行の次）
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<LastMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// 書いた時刻（epoch ms）
    #[serde(default)]
    pub at: u64,
}

/// 同じ鍵（刻み・会話・モデル・fast）の行を足し合わせる
pub fn merge_rows(rows: impl IntoIterator<Item = BucketRow>) -> Vec<BucketRow> {
    let mut map: BTreeMap<BucketKey, (Tokens, u32)> = BTreeMap::new();
    for r in rows {
        let entry = map
            .entry(BucketKey {
                t: r.t,
                session: r.session,
                model: r.model,
                fast: r.fast,
            })
            .or_default();
        entry.0.add(&r.tokens);
        entry.1 += r.responses;
    }
    map.into_iter()
        .map(|(k, (tokens, responses))| BucketRow {
            t: k.t,
            session: k.session,
            model: k.model,
            fast: k.fast,
            tokens,
            responses,
        })
        .collect()
}

/// 古い月を日単位へまとめる（ローカル時刻の日の頭。§5.2「35 日より古い刻みは日単位へ」）
pub fn to_daily(rows: impl IntoIterator<Item = BucketRow>, utc_offset: i32) -> Vec<BucketRow> {
    let off = i64::from(utc_offset);
    merge_rows(rows.into_iter().map(|mut r| {
        r.t = (r.t + off).div_euclid(86_400) * 86_400 - off;
        r
    }))
}

/// 月の終わりからこの日数を過ぎた月は日単位へまとめる（`tako_control::usage::index` の月の掃除）
pub const DAILY_AFTER_DAYS: i64 = 35;

/// 行の刻みの長さ（区間按分 = S3 で行の使用量を時間に散らす幅）。日単位へまとめた月の行は 1 日。
/// まとめる前の 5 分の行も日単位とみなす（月の掃除は 1 日に 1 回なので遅れうる。差は区間の端の比だけ）
pub fn row_span(t: i64, now: i64) -> i64 {
    match month_range(&month_of(t)) {
        Some((_, end)) if end + DAILY_AFTER_DAYS * 86_400 < now => 86_400,
        _ => BUCKET_SECS,
    }
}

/// 刻みの頭（unix 秒）→ 月のファイル名の `YYYY-MM`（UTC）
pub fn month_of(t: i64) -> String {
    let (y, m, _) = crate::limit_resume::civil_from_days(t.div_euclid(86_400));
    format!("{y:04}-{m:02}")
}

/// `YYYY-MM` → その月の頭と次の月の頭（unix 秒・UTC）
pub fn month_range(name: &str) -> Option<(i64, i64)> {
    let (y, m) = name.split_once('-')?;
    let (y, m): (i64, i64) = (y.parse().ok()?, m.parse().ok()?);
    if !(1..=12).contains(&m) || name.len() != 7 {
        return None;
    }
    let start = crate::limit_resume::days_from_civil(y, m, 1) * 86_400;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let end = crate::limit_resume::days_from_civil(ny, nm, 1) * 86_400;
    Some((start, end))
}

// --- 会話の鍵（§3.2 / §3.3）---------------------------------------------------------------------

/// プロジェクトの鍵の出どころ（強い順）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFrom {
    /// cwd の最長前方一致（`projects.yaml`）
    Cwd,
    /// master / solo のプロファイル（カタログの `profile`）
    Profile,
    /// カタログの `project`（worker）
    Catalog,
}

/// `keys.jsonl` の 1 行（会話 → 帰属の鍵）。**取り込んだ時点で写し**、カタログ（500 件）・
/// レジストリ（200 件）が追い出しても、transcript が 30 日で消えても鍵を保つ（§3.3）。
/// 同じ会話の行は [`SessionKeys::merged`] で合わせる（良くなったら上書き・悪くはしない）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionKeys {
    pub session_id: String,
    /// 置き場から推したログイン（mod の報告 = S1 の `sessions.jsonl` があればそちらが正）
    pub login: String,
    pub key_from: KeyFrom,
    /// エージェントの系統（S2 は claude だけ）
    #[serde(default = "default_agent")]
    pub agent: String,
    /// tako のアカウント名（accounts.yaml。当たらなければ設定 dir の名前）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// 役割（カタログの `kind` = master / worker / solo / pane）。無ければ手動
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_from: Option<ProjectFrom>,
    /// 主の Issue（ラベルに書いた順で最初の `#N`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<String>,
    /// spawn のラベル（tako 自身のメタデータ。`prompt_head` は写さない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 最初に数えた応答の時刻（unix 秒）
    pub first_at: i64,
    /// 書いた時刻（epoch ms）
    #[serde(default)]
    pub recorded_at: u64,
}

fn default_agent() -> String {
    "claude".to_string()
}

impl SessionKeys {
    /// 新しく引いた鍵 `fresh` で良くなる所だけを上書きした値。変わらなければ `None`
    /// （書き直さない）。**悪くはしない**: 未分類で上書きしない・ログインは資格情報から決まった後は
    /// 動かさない（後から同じ設定 dir で別のログインへ入り直しても、過去の会話の枠は変わらない）
    pub fn merged(&self, fresh: &SessionKeys) -> Option<SessionKeys> {
        let mut out = self.clone();
        if out.key_from == KeyFrom::ConfigDir && fresh.key_from == KeyFrom::Account {
            out.login = fresh.login.clone();
            out.key_from = KeyFrom::Account;
        }
        if out.account.is_none() {
            out.account = fresh.account.clone();
        }
        if out.role.is_none() {
            out.role = fresh.role.clone();
        }
        let better_project = match (out.project_from, fresh.project_from) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(a), Some(b)) => b > a,
        };
        if better_project && fresh.project.is_some() {
            out.project = fresh.project.clone();
            out.project_from = fresh.project_from;
        }
        if out.label.is_none() {
            out.label = fresh.label.clone();
        }
        if out.issue.is_none() {
            out.issue = fresh.issue;
        }
        if out.ledger_id.is_none() {
            out.ledger_id = fresh.ledger_id.clone();
        }
        if out.task_type.is_none() {
            out.task_type = fresh.task_type.clone();
        }
        out.first_at = out.first_at.min(fresh.first_at);
        let mut a = out.clone();
        let mut b = self.clone();
        a.recorded_at = 0;
        b.recorded_at = 0;
        if a == b {
            return None;
        }
        out.recorded_at = fresh.recorded_at;
        Some(out)
    }
}

/// ラベルに書いた順で最初の `#N`（`tako_control::sessions::extract_issues` と同じ読み方 =
/// `#` の直後の 1〜6 桁・0 は無視。#2199 の前後でラベルの書き方は変わらないので、
/// 古い記録（`issues` が昇順）も新しい記録も同じ答えになる）
pub fn first_issue(label: &str) -> Option<u32> {
    label.split('#').skip(1).find_map(|part| {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.chars().count() > 6 {
            return None;
        }
        digits.parse::<u32>().ok().filter(|n| *n > 0)
    })
}

/// パスを区切りごとに分ける（`/` と `\` の両方。空の区切りは落とす）
fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// `tako-wt-2201` → `tako`（worktree の名前を元のプロジェクトへ寄せる）
fn strip_worktree(component: &str) -> &str {
    match component.rsplit_once("-wt-") {
        Some((head, tail))
            if !head.is_empty() && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) =>
        {
            head
        }
        _ => component,
    }
}

/// cwd の最長前方一致でプロジェクトを決める（区切り単位で比べる。当たらなければ worktree の名前を
/// 元のプロジェクトへ寄せてもう一度 = `<project>-wt-N`）。`projects` は（キー, cwd）
pub fn project_for_cwd(cwd: &str, projects: &[(String, String)]) -> Option<String> {
    let best = |cwd_parts: &[&str]| -> Option<String> {
        let mut found: Option<(usize, &str)> = None;
        for (key, root) in projects {
            let root_parts = components(root);
            if root_parts.is_empty() || root_parts.len() > cwd_parts.len() {
                continue;
            }
            if cwd_parts[..root_parts.len()] == root_parts[..]
                && found.is_none_or(|(len, _)| root_parts.len() > len)
            {
                found = Some((root_parts.len(), key.as_str()));
            }
        }
        found.map(|(_, k)| k.to_string())
    };
    let parts = components(cwd);
    best(&parts).or_else(|| {
        let stripped: Vec<&str> = parts.iter().map(|c| strip_worktree(c)).collect();
        (stripped != parts).then(|| best(&stripped)).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, session: &str, ts: &str, model: &str, usage: &str) -> String {
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","message":{{"id":"{id}","model":"{model}","role":"assistant","content":[{{"type":"text","text":"本文の印"}}],"usage":{usage}}},"requestId":"req_{id}","timestamp":"{ts}","sessionId":"{session}","cwd":"/home/testuser/dev/tako","uuid":"u-{id}-{ts}"}}"#
        )
    }

    fn usage(i: u64, o: u64, c5: u64, c1: u64, cr: u64) -> String {
        format!(
            r#"{{"input_tokens":{i},"output_tokens":{o},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{},"cache_creation":{{"ephemeral_5m_input_tokens":{c5},"ephemeral_1h_input_tokens":{c1}}},"service_tier":"standard","speed":"standard"}}"#,
            c5 + c1
        )
    }

    fn ingest(
        text: &str,
        last: &mut Option<LastMessage>,
        seen: &mut HashSet<u64>,
    ) -> (Batch, usize) {
        let mut batch = Batch::default();
        let mut ids = Vec::new();
        let n = ingest_lines(
            text.as_bytes(),
            FileProgress {
                last,
                seen,
                new_ids: &mut ids,
                fallback_session: "file-stem",
            },
            &mut batch,
        );
        (batch, n)
    }

    const TS: &str = "2026-10-16T12:34:56.789Z";

    #[test]
    fn assistant行の使用量だけを読む() {
        let l = line(
            "msg_1",
            "s1",
            TS,
            "claude-opus-5-5",
            &usage(2, 100, 30, 40, 500),
        );
        let u = parse_assistant_line(l.as_bytes()).unwrap();
        assert_eq!(u.message_id.as_deref(), Some("msg_1"));
        assert_eq!(u.session_id.as_deref(), Some("s1"));
        assert_eq!(u.at, crate::claude_mod::parse_resets_at(TS));
        assert_eq!(
            u.tokens,
            Tokens {
                input: 2,
                output: 100,
                cache_5m: 30,
                cache_1h: 40,
                cache_read: 500
            }
        );
        assert!(!u.fast);
        // user 行・本文に assistant の印を含む user 行・壊れた行・usage の無い行は読まない
        let user = r#"{"type":"user","message":{"content":"\"type\":\"assistant\""}}"#;
        assert_eq!(parse_assistant_line(user.as_bytes()), None);
        // 絞り込みには当たる（入れ子に同じ並びがある）が、行そのものは user
        let nested = r#"{"type":"user","toolUseResult":{"type":"assistant","message":{"id":"msg_n","usage":{"input_tokens":1}}}}"#;
        assert!(assistant_needle().is_match(nested.as_bytes()));
        assert_eq!(parse_assistant_line(nested.as_bytes()), None);
        assert_eq!(parse_assistant_line(br#"{"type":"assistant","mess"#), None);
        assert_eq!(
            parse_assistant_line(br#"{"type":"assistant","message":{"id":"x"}}"#),
            None
        );
        // 内訳の無い古い版: キャッシュ作成の総量を 5 分として数える
        let old = r#"{"type":"assistant","timestamp":"2026-10-01T00:00:00Z","message":{"id":"msg_o","model":"m","usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":70,"cache_read_input_tokens":3}}}"#;
        let u = parse_assistant_line(old.as_bytes()).unwrap();
        assert_eq!((u.tokens.cache_5m, u.tokens.cache_1h), (70, 0));
        // fast
        let fast = l.replace(r#""speed":"standard""#, r#""speed":"fast""#);
        assert!(parse_assistant_line(fast.as_bytes()).unwrap().fast);
    }

    #[test]
    fn 分割行と仮のoutputは最大へ寄せて増えた分だけ足す() {
        // 1 応答が 3 行（仮の output 5 → 50 → 確定 120）+ 次の応答
        let text = [
            line(
                "msg_a",
                "s1",
                TS,
                "claude-opus-5-5",
                &usage(2, 5, 10, 0, 100),
            ),
            line(
                "msg_a",
                "s1",
                TS,
                "claude-opus-5-5",
                &usage(2, 50, 10, 0, 100),
            ),
            line(
                "msg_a",
                "s1",
                TS,
                "claude-opus-5-5",
                &usage(2, 120, 10, 0, 100),
            ),
            line(
                "msg_b",
                "s1",
                TS,
                "claude-opus-5-5",
                &usage(1, 7, 0, 3, 200),
            ),
        ]
        .join("\n")
            + "\n";
        let mut last = None;
        let mut seen = HashSet::new();
        let (batch, n) = ingest(&text, &mut last, &mut seen);
        assert_eq!(n, text.len());
        assert_eq!(batch.responses, 2);
        assert_eq!(batch.continued, 2);
        let rows = batch.rows();
        assert_eq!(rows.len(), 1, "同じ刻み・会話・モデルは 1 行");
        assert_eq!(
            rows[0].tokens,
            Tokens {
                input: 3,
                output: 127,
                cache_5m: 10,
                cache_1h: 3,
                cache_read: 300
            }
        );
        assert_eq!(rows[0].responses, 2);
        assert_eq!(last.as_ref().unwrap().id, "msg_b");
    }

    #[test]
    fn 更新をまたいだ続きの行は差分だけ() {
        let first = line("msg_a", "s1", TS, "claude-opus-5-5", &usage(2, 5, 0, 0, 10)) + "\n";
        let second = line(
            "msg_a",
            "s1",
            TS,
            "claude-opus-5-5",
            &usage(2, 80, 0, 0, 10),
        ) + "\n";
        let mut last = None;
        let mut seen = HashSet::new();
        let (a, _) = ingest(&first, &mut last, &mut seen);
        let (b, _) = ingest(&second, &mut last, &mut seen);
        assert_eq!(a.rows()[0].tokens.output, 5);
        assert_eq!(b.rows()[0].tokens.output, 75);
        assert_eq!(b.rows()[0].tokens.cache_read, 0);
        assert_eq!(b.rows()[0].responses, 0, "続きは応答数を増やさない");
        assert_eq!(b.responses, 0);
    }

    #[test]
    fn 書きかけの最終行は読まずに残す() {
        let full = line("msg_a", "s1", TS, "claude-opus-5-5", &usage(1, 1, 0, 0, 1));
        let text = format!("{full}\n{}", &full[..full.len() / 2]);
        let mut last = None;
        let mut seen = HashSet::new();
        let (batch, n) = ingest(&text, &mut last, &mut seen);
        assert_eq!(n, full.len() + 1);
        assert_eq!(batch.responses, 1);
        // 改行の無い 1 行だけ = 何も読まない
        let (batch, n) = ingest(&full, &mut None, &mut HashSet::new());
        assert_eq!((n, batch.responses), (0, 0));
    }

    #[test]
    fn syntheticと写しと時刻の無い行は数えない() {
        let text = [
            line("msg_a", "s1", TS, "claude-opus-5-5", &usage(1, 1, 0, 0, 1)),
            line("msg_syn", "s1", TS, SYNTHETIC_MODEL, &usage(0, 0, 0, 0, 0)),
            r#"{"type":"user","message":{"content":"x"}}"#.to_string(),
            line("msg_b", "s1", TS, "claude-opus-5-5", &usage(1, 1, 0, 0, 1)),
            // 別の応答を挟んだ同じ応答の写し（/btw）= 数え済み
            line("msg_a", "s1", TS, "claude-opus-5-5", &usage(1, 1, 0, 0, 1)),
            line("msg_t", "s1", TS, "claude-opus-5-5", &usage(1, 1, 0, 0, 1))
                .replace(&format!(r#""timestamp":"{TS}","#), ""),
        ]
        .join("\n")
            + "\n";
        let (batch, _) = ingest(&text, &mut None, &mut HashSet::new());
        assert_eq!(batch.responses, 2);
        assert_eq!(batch.synthetic, 1);
        assert_eq!(batch.duplicates, 1);
        assert_eq!(batch.no_time, 1);
    }

    #[test]
    fn 会話の無い行はファイル名へ_鍵はmsg形式とそれ以外で分ける() {
        let l =
            line("msg_a", "s1", TS, "m", &usage(1, 1, 0, 0, 1)).replace(r#""sessionId":"s1","#, "");
        let (batch, _) = ingest(&(l + "\n"), &mut None, &mut HashSet::new());
        assert!(batch.sessions.contains_key("file-stem"));
        let a = parse_assistant_line(line("gw_1", "s1", TS, "m", &usage(1, 1, 0, 0, 1)).as_bytes())
            .unwrap();
        let mut b = a.clone();
        b.session_id = Some("s2".into());
        // msg_ 以外の id（ゲートウェイの使い回し）は会話・時刻まで含めて区別する
        assert_ne!(dedup_key(&a), dedup_key(&b));
        let mut c = a.clone();
        c.message_id = Some("msg_x".into());
        let mut d = c.clone();
        d.session_id = Some("s9".into());
        assert_eq!(
            dedup_key(&c),
            dedup_key(&d),
            "msg_ の id はそれだけで同じ応答"
        );
    }

    #[test]
    fn 行は往復し同じ鍵は足し合わせる() {
        let row = BucketRow {
            t: 600,
            session: "s".into(),
            model: "m".into(),
            fast: false,
            tokens: Tokens {
                input: 1,
                ..Tokens::default()
            },
            responses: 1,
        };
        let text = serde_json::to_string(&row).unwrap();
        assert!(!text.contains("fast"), "既定の fast は書かない: {text}");
        assert_eq!(serde_json::from_str::<BucketRow>(&text).unwrap(), row);
        let merged = merge_rows([row.clone(), row.clone()]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].tokens.input, 2);
        assert_eq!(merged[0].responses, 2);
        // 日単位（JST = +9h）: 15:00Z と 14:59Z は日付が分かれる
        let at = |h: i64, m: i64| {
            crate::limit_resume::days_from_civil(2026, 10, 1) * 86_400 + h * 3600 + m * 60
        };
        let daily = to_daily(
            [
                BucketRow {
                    t: at(14, 55),
                    ..row.clone()
                },
                BucketRow {
                    t: at(15, 0),
                    ..row.clone()
                },
                BucketRow {
                    t: at(16, 0),
                    ..row.clone()
                },
            ],
            9 * 3600,
        );
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[1].responses, 2);
    }

    #[test]
    fn 月のファイル名と範囲() {
        let t = crate::limit_resume::days_from_civil(2026, 12, 31) * 86_400 + 1;
        assert_eq!(month_of(t), "2026-12");
        let (s, e) = month_range("2026-12").unwrap();
        assert!(s <= t && t < e);
        assert_eq!(month_of(e), "2027-01");
        assert_eq!(month_range("2026-13"), None);
        assert_eq!(month_range("x"), None);
    }

    #[test]
    fn 最初のissueはラベルの書いた順() {
        assert_eq!(first_issue("#2192 / #322: 何か"), Some(2192));
        assert_eq!(first_issue("S2 #2201 と #12"), Some(2201));
        assert_eq!(first_issue("番号なし"), None);
        assert_eq!(first_issue("#0 #1234567 #7"), Some(7));
    }

    #[test]
    fn cwdの最長前方一致とworktree() {
        let projects = vec![
            ("tako".to_string(), "/home/testuser/dev/tako".to_string()),
            (
                "docs".to_string(),
                "/home/testuser/dev/tako/docs".to_string(),
            ),
            ("other".to_string(), "/home/testuser/dev/other/".to_string()),
        ];
        assert_eq!(
            project_for_cwd("/home/testuser/dev/tako/crates", &projects).as_deref(),
            Some("tako")
        );
        assert_eq!(
            project_for_cwd("/home/testuser/dev/tako/docs/src", &projects).as_deref(),
            Some("docs")
        );
        // 区切り単位で比べる（tako-x は tako ではない）
        assert_eq!(
            project_for_cwd("/home/testuser/dev/tako-x", &projects),
            None
        );
        // worktree は元のプロジェクトへ
        assert_eq!(
            project_for_cwd("/home/testuser/dev/tako-wt-2201/crates", &projects).as_deref(),
            Some("tako")
        );
        assert_eq!(
            project_for_cwd(
                "C:\\Users\\winuser\\dev\\other\\x",
                &[("w".into(), "C:\\Users\\winuser\\dev\\other".into())]
            )
            .as_deref(),
            Some("w")
        );
        assert_eq!(project_for_cwd("/", &projects), None);
    }

    fn keys(project: Option<(&str, ProjectFrom)>, from: KeyFrom) -> SessionKeys {
        SessionKeys {
            session_id: "s".into(),
            login: format!("{from:?}"),
            key_from: from,
            agent: "claude".into(),
            account: None,
            role: None,
            project: project.map(|p| p.0.to_string()),
            project_from: project.map(|p| p.1),
            issue: None,
            task_type: None,
            ledger_id: None,
            label: None,
            first_at: 100,
            recorded_at: 1,
        }
    }

    #[test]
    fn 鍵は良くなったら上書きし悪くはしない() {
        let cwd = keys(Some(("tako", ProjectFrom::Cwd)), KeyFrom::ConfigDir);
        // 同じなら書き直さない
        assert_eq!(cwd.merged(&cwd.clone()), None);
        // カタログで良くなる（ログインも資格情報から決まった）
        let mut better = keys(Some(("tako-docs", ProjectFrom::Catalog)), KeyFrom::Account);
        better.label = Some("#2201 S2".into());
        better.issue = Some(2201);
        better.role = Some("worker".into());
        better.first_at = 50;
        better.recorded_at = 9;
        let m = cwd.merged(&better).expect("良くなった");
        assert_eq!(m.project.as_deref(), Some("tako-docs"));
        assert_eq!(m.project_from, Some(ProjectFrom::Catalog));
        assert_eq!(m.key_from, KeyFrom::Account);
        assert_eq!(m.issue, Some(2201));
        assert_eq!(m.first_at, 50);
        assert_eq!(m.recorded_at, 9);
        // カタログが追い出した後の引き直し（cwd だけ・鍵なし）では悪くならない
        let worse = keys(Some(("tako", ProjectFrom::Cwd)), KeyFrom::ConfigDir);
        assert_eq!(m.merged(&worse), None);
        let none = keys(None, KeyFrom::ConfigDir);
        assert_eq!(m.merged(&none), None);
        // 資格情報から決まったログインは動かさない
        let mut other_login = m.clone();
        other_login.login = "other".into();
        assert_eq!(m.merged(&other_login), None);
    }
}

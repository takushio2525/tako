//! codex の rollout の使用量と利用制限の観測（usage 分析 S5 #2204。設計書
//! `.agent/plans/2026-10-usage-analytics.md` §1.5 / §9「S5」）。ここは**純関数と型だけ**（走査・読み位置・
//! 書き込み・ログインの鍵は `tako_control::usage::index`）。
//!
//! # 読むもの
//!
//! `$CODEX_HOME/sessions/**/rollout-*.jsonl`（1 行 1 イベント `{timestamp, type, payload}`）のうち 3 種類だけ:
//!
//! | 行 | 使う欄 | 使い道 |
//! |---|---|---|
//! | `session_meta` | `id`（会話）・`timestamp`・`cwd`・`parent_thread_id` / `forked_from_id` / `source.subagent` | 会話の鍵・プロジェクト・fork の見分け |
//! | `turn_context` | `model`・`cwd` | 以後の使用量のモデル・プロジェクト |
//! | `event_msg` / `token_count` | `info.total_token_usage`（累積）・`info.last_token_usage`・`rate_limits` | 使用量・利用制限の観測 |
//!
//! 読み手の構造体（`RawLine` / `RawPayload` ほか）は上の欄しか持たず、**発話・推論・ツールの入出力
//! （`response_item`）・`base_instructions` は serde が読み飛ばして値を持ち出さない**（§6）。部分一致で
//! 3 種類の行に絞ってからパースする（バイトの大半は `response_item`）。`cwd` は鍵を決めるのに使うだけで、
//! 索引へは書かない。
//!
//! # 数え方（累積の差分）
//!
//! `total_token_usage` は会話の累積なので、**前の累積との差**をその行の時刻の使用量にする。差の和は
//! 最後の累積に一致し（受け入れ条件）、`token_count` の行が欠けても次の行の差が埋める（`last_token_usage`
//! を足すと欠けた分を落とす）。同じ累積の行（重複）は 0。累積が下がった（数え直しが始まった）ときは
//! その行の `last_token_usage`（無ければ下がった後の累積）を数え、以後はその累積から差を取る。
//!
//! - **fork / サブエージェント**: 親の履歴を写して始まる rollout がある（ccusage の
//!   `rust/adapters/codex/src/README.md`「MultiAgent V2 subagent replay」）。写しは fork の瞬間の時刻で
//!   **一気に**書かれ、子の最初のターンは数秒後に来る（ccusage の実測: 写しの束は 10〜40 ms・次の
//!   ターンまで 5.8〜15.3 秒）。そこで fork の rollout では、`session_meta` の時刻から
//!   [`REPLAY_BURST_MS`] 以内に続く使用量の行を写しとみなし、数えずに累積の基準だけを進める
//!   （「最後に引き継いだ値を基準に、子のターンで増えた分だけ数える」）。この機の実データ（fork 95 本）では
//!   最初の使用量は `session_meta` の 6.8 秒以上後で、写しは無かった
//! - トークンの成分: codex の `input_tokens` は `cached_input_tokens`（と `cache_write_input_tokens`）を含み、
//!   `output_tokens` は `reasoning_output_tokens` を含む（ccusage と同じ読み方）。tako の 5 成分へは
//!   入力 = 入力 − 読み出し − 書き込み・読み出し・作成（5 分）= 書き込み・出力 として写すので、
//!   5 成分の和は codex の `total_tokens`（入力 + 出力）と一致する
//!
//! # 利用制限の観測
//!
//! 同じ `token_count` の行の `rate_limits` の `primary` / `secondary` を、`window_minutes` で枠へ写す
//! （300 = `five_hour`・10080 = `seven_day`。**primary が週の枠のプランがある**ので位置では決めない）。
//! `limit_id` が `codex` 以外（モデル別の別の枠 = `codex_bengalfox` 等）は別の利用制限なので観測にしない。
//! 時刻は行の `timestamp`（ミリ秒）。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::index::{bucket_of, Batch, BucketKey, Tokens};
use super::share::{FIVE_HOUR, SEVEN_DAY};

/// S5 の A/B: 立っていれば codex の rollout を読まない（#2204 前 = claude だけ）
pub const S5_LEGACY_ENV: &str = "TAKO_2192_S5_LEGACY";
/// fork の rollout で、写しの束とみなす時刻の幅（ミリ秒。ccusage の `CODEX_REWRITTEN_BURST_PAUSE_MS`）
pub const REPLAY_BURST_MS: i64 = 1_000;
/// 観測にする利用制限（モデル別の別の枠は除く）
pub const MAIN_LIMIT_ID: &str = "codex";
/// モデルが分からない使用量（`turn_context` より前の行）
pub const UNKNOWN_MODEL: &str = "unknown";

/// S5 の A/B（[`S5_LEGACY_ENV`]）が立っているか
pub fn s5_legacy() -> bool {
    std::env::var_os(S5_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// codex の使用量の 6 成分（`total_token_usage` / `last_token_usage` の形のまま）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexUsage {
    #[serde(default)]
    pub input: u64,
    /// `input` のうちキャッシュから読んだ分
    #[serde(default)]
    pub cached: u64,
    /// `input` のうちキャッシュへ書いた分
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub output: u64,
    /// `output` のうち推論の分
    #[serde(default)]
    pub reasoning: u64,
    /// codex の `total_tokens`（入力 + 出力）
    #[serde(default)]
    pub total: u64,
}

impl CodexUsage {
    fn parts(&self) -> [u64; 6] {
        [
            self.input,
            self.cached,
            self.cache_write,
            self.output,
            self.reasoning,
            self.total,
        ]
    }

    /// どの成分も `other` 以上
    pub fn covers(&self, other: &CodexUsage) -> bool {
        self.parts().iter().zip(other.parts()).all(|(a, b)| *a >= b)
    }

    pub fn is_zero(&self) -> bool {
        self.parts().iter().all(|v| *v == 0)
    }

    /// 成分ごとの差（負にならない）
    pub fn saturating_sub(&self, other: &CodexUsage) -> CodexUsage {
        CodexUsage {
            input: self.input.saturating_sub(other.input),
            cached: self.cached.saturating_sub(other.cached),
            cache_write: self.cache_write.saturating_sub(other.cache_write),
            output: self.output.saturating_sub(other.output),
            reasoning: self.reasoning.saturating_sub(other.reasoning),
            total: self.total.saturating_sub(other.total),
        }
    }

    /// 成分ごとの最小
    pub fn min(&self, other: &CodexUsage) -> CodexUsage {
        CodexUsage {
            input: self.input.min(other.input),
            cached: self.cached.min(other.cached),
            cache_write: self.cache_write.min(other.cache_write),
            output: self.output.min(other.output),
            reasoning: self.reasoning.min(other.reasoning),
            total: self.total.min(other.total),
        }
    }

    /// tako の 5 成分へ写す（読み出し・書き込みは入力に含まれる・推論は出力に含まれる）。
    /// 5 成分の和は入力 + 出力 = codex の `total_tokens`
    pub fn tokens(&self) -> Tokens {
        let cached = self.cached.min(self.input);
        let write = self.cache_write.min(self.input - cached);
        Tokens {
            input: self.input - cached - write,
            output: self.output,
            cache_5m: write,
            cache_1h: 0,
            cache_read: cached,
        }
    }
}

// --- 読み手（上の表の欄だけを持つ。発話・ツール・base_instructions は serde が読み飛ばす）----------

#[derive(Deserialize)]
struct RawLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    payload: Option<RawPayload>,
}

#[derive(Deserialize)]
struct RawPayload {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    /// `session_meta` の会話
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    parent_thread_id: Option<String>,
    #[serde(default)]
    forked_from_id: Option<String>,
    /// `"cli"` / `{"subagent": …}` など（形が版で違うので値のまま受けて、印だけを見る）
    #[serde(default)]
    source: Option<serde_json::Value>,
    /// `turn_context` のモデル
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    info: Option<RawInfo>,
    /// 形の揺れ（欄の足し引き・数の型）で同じ行の使用量まで落とさないよう、値のまま受けて読む
    #[serde(default)]
    rate_limits: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(default)]
    total_token_usage: Option<RawUsage>,
    #[serde(default)]
    last_token_usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    cached_input_tokens: Option<u64>,
    #[serde(default)]
    cache_write_input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    reasoning_output_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

impl RawUsage {
    fn usage(&self) -> CodexUsage {
        let input = self.input_tokens.unwrap_or(0);
        let output = self.output_tokens.unwrap_or(0);
        CodexUsage {
            input,
            cached: self.cached_input_tokens.unwrap_or(0),
            cache_write: self.cache_write_input_tokens.unwrap_or(0),
            output,
            reasoning: self.reasoning_output_tokens.unwrap_or(0),
            // 古い版で total が無ければ入力 + 出力（ccusage と同じ）
            total: self.total_tokens.unwrap_or(input + output),
        }
    }
}

/// `rate_limits` の 1 枠（`used_percent` / `window_minutes` / `resets_at` だけ。数は整数でも小数でも読む）
struct RawWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

impl RawWindow {
    fn from_value(v: &serde_json::Value) -> Option<RawWindow> {
        let o = v.as_object()?;
        let num = |k: &str| o.get(k).and_then(serde_json::Value::as_f64);
        Some(RawWindow {
            used_percent: num("used_percent"),
            window_minutes: num("window_minutes")
                .filter(|m| *m >= 0.0)
                .map(|m| m.round() as u64),
            resets_at: num("resets_at").map(|r| r.round() as i64),
        })
    }
}

/// 部分一致の絞り込み（読む 3 種類の行だけをパースする）
fn needle() -> &'static regex::bytes::Regex {
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::bytes::Regex::new(&format!(
            "{}|{}|{}",
            regex::escape("\"type\":\"token_count\""),
            regex::escape("\"type\":\"turn_context\""),
            regex::escape("\"type\":\"session_meta\""),
        ))
        .expect("固定の正規表現")
    })
}

/// ISO 8601 → unix ミリ秒（`2026-10-16T12:34:56.789Z`。小数は 3 桁まで読む）
pub fn parse_ms(text: &str) -> Option<i64> {
    let secs = crate::claude_mod::parse_resets_at(text)?;
    let frac = text
        .split_once('.')
        .map(|(_, rest)| {
            let digits: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .take(3)
                .collect();
            let n: i64 = digits.parse().unwrap_or(0);
            n * 10_i64.pow(3 - digits.chars().count() as u32)
        })
        .unwrap_or(0);
    Some(secs * 1000 + frac)
}

/// `window_minutes` → 枠（tako の 5h / 7d と同じ名前）
pub fn kind_for_minutes(minutes: u64) -> Option<&'static str> {
    match minutes {
        300 => Some(FIVE_HOUR),
        10_080 => Some(SEVEN_DAY),
        _ => None,
    }
}

/// 1 本の rollout の読みの状態（`cursor.jsonl` の行に載せて、追記された分の続きから読む）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexCursor {
    /// 会話（`session_meta` の `id`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// fork / サブエージェントの rollout（親の履歴の写しで始まりうる）
    #[serde(default, skip_serializing_if = "is_false")]
    pub fork: bool,
    /// 今のモデル（最後の `turn_context`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 最後に見た累積（差を取る基準）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<CodexUsage>,
    /// 写しの束の最後の時刻（unix ミリ秒）。`None` = 写しの見分けは終わった（fork でない・子のターンに入った）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_at: Option<i64>,
    /// ファイルを最初から読み直すとき（縮んだ・置き換わった）の数え済みの累積。これを超えた分だけ数える
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted: Option<CodexUsage>,
}

impl CodexCursor {
    /// 最初から読み直すときの状態（数え済みの累積だけを持ち越して、同じ使用量を 2 回数えない）
    pub fn restarted(&self) -> CodexCursor {
        let counted = match (self.total, self.counted) {
            (Some(t), Some(c)) => Some(if t.covers(&c) { t } else { c }),
            (t, c) => t.or(c),
        };
        CodexCursor {
            counted,
            ..CodexCursor::default()
        }
    }
}

/// 1 本の rollout から読んだ利用制限の観測（ログインは呼び手が付ける）
#[derive(Debug, Clone, PartialEq)]
pub struct CodexLimit {
    /// `five_hour` / `seven_day`
    pub kind: &'static str,
    pub percent: f64,
    /// 解除時刻（unix 秒）
    pub resets_at: i64,
    /// 観測した時刻（行の `timestamp`。unix ミリ秒）
    pub observed_at: u64,
}

/// 1 本の rollout の読みの集計（[`Batch`] に足りない codex の分）
#[derive(Debug, Clone, Default)]
pub struct CodexRead {
    pub limits: Vec<CodexLimit>,
    /// 観測にしなかった枠（`limit_id` が別の枠・知らない窓の長さ・解除時刻の無いもの）
    pub skipped_limits: u64,
    /// fork の写しとみなして数えなかった使用量の行
    pub replayed: u64,
    /// 累積が下がった行
    pub resets: u64,
}

/// 1 本の rollout の読みの状態（[`ingest_codex_lines`] が進める）
pub struct CodexProgress<'a> {
    pub state: &'a mut CodexCursor,
    /// `session_meta` が無いときの会話（ファイル名）
    pub fallback_session: &'a str,
    /// 今の cwd（鍵の材料。読み位置には書かない = 続きの読みでは次の `turn_context` まで分からない）
    pub cwd: &'a mut Option<String>,
}

/// fork / サブエージェントの印（`parent_thread_id` / `forked_from_id` / `source.subagent`）
fn is_fork(p: &RawPayload) -> bool {
    p.parent_thread_id.as_deref().is_some_and(|s| !s.is_empty())
        || p.forked_from_id.as_deref().is_some_and(|s| !s.is_empty())
        || p.source
            .as_ref()
            .and_then(|s| s.as_object())
            .is_some_and(|o| o.contains_key("subagent"))
}

/// 改行で終わった行だけを読み、読んだバイト数（最後の改行の次まで）を返す。
/// **書きかけの最終行（改行が無い）は読まずに残す**（次の更新で続きと一緒に読む）
pub fn ingest_codex_lines(
    data: &[u8],
    progress: CodexProgress<'_>,
    batch: &mut Batch,
    read: &mut CodexRead,
) -> usize {
    let CodexProgress {
        state,
        fallback_session,
        cwd,
    } = progress;
    let mut consumed = 0usize;
    let mut rest = data;
    while let Some(nl) = rest.iter().position(|b| *b == b'\n') {
        let line = &rest[..nl];
        rest = &rest[nl + 1..];
        consumed += nl + 1;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || !needle().is_match(line) {
            continue;
        }
        let Ok(raw) = serde_json::from_slice::<RawLine>(line) else {
            continue;
        };
        let Some(payload) = raw.payload else {
            continue;
        };
        match (raw.kind.as_deref(), payload.kind.as_deref()) {
            (Some("session_meta"), _) => {
                if state.session.is_none() {
                    state.session = payload.id.clone().filter(|s| !s.is_empty());
                    state.fork = is_fork(&payload);
                    if state.fork {
                        state.replay_at = payload
                            .timestamp
                            .as_deref()
                            .or(raw.timestamp.as_deref())
                            .and_then(parse_ms);
                    }
                }
                if let Some(c) = payload.cwd.filter(|s| !s.is_empty()) {
                    *cwd = Some(c);
                }
            }
            (Some("turn_context"), _) => {
                if let Some(m) = payload.model.map(|m| m.trim().to_string()) {
                    if !m.is_empty() {
                        state.model = Some(m);
                    }
                }
                if let Some(c) = payload.cwd.filter(|s| !s.is_empty()) {
                    *cwd = Some(c);
                }
            }
            (Some("event_msg"), Some("token_count")) => {
                let Some(at_ms) = raw.timestamp.as_deref().and_then(parse_ms) else {
                    batch.no_time += 1;
                    continue;
                };
                token_count(
                    state,
                    fallback_session,
                    cwd.as_deref(),
                    at_ms,
                    &payload,
                    batch,
                    read,
                );
            }
            _ => {}
        }
    }
    consumed
}

fn token_count(
    state: &mut CodexCursor,
    fallback_session: &str,
    cwd: Option<&str>,
    at_ms: i64,
    payload: &RawPayload,
    batch: &mut Batch,
    read: &mut CodexRead,
) {
    let total = payload
        .info
        .as_ref()
        .and_then(|i| i.total_token_usage.as_ref())
        .map(RawUsage::usage);
    let last = payload
        .info
        .as_ref()
        .and_then(|i| i.last_token_usage.as_ref())
        .map(RawUsage::usage);
    let carries_usage = total.is_some() || last.is_some();
    // fork の写しの束: session_meta の時刻から REPLAY_BURST_MS 以内に続く行は数えず、基準だけを進める
    if let Some(prev) = state.replay_at {
        if at_ms - prev <= REPLAY_BURST_MS {
            if carries_usage {
                state.replay_at = Some(at_ms);
                if let Some(t) = total {
                    state.total = Some(t);
                }
                read.replayed += 1;
            }
            // 写しの束の rate_limits は親の古い値なので観測にもしない
            return;
        }
        if carries_usage {
            state.replay_at = None;
        }
    }
    let delta = match (total, state.total) {
        (Some(t), _) if state.counted.is_some() => {
            // 読み直しの追いつき: 数え済みの累積を超えた分だけ
            let c = state.counted.unwrap_or_default();
            if c.covers(&t) {
                CodexUsage::default()
            } else {
                state.counted = None;
                t.saturating_sub(&c)
            }
        }
        (Some(t), None) => t,
        (Some(t), Some(p)) if t.covers(&p) => t.saturating_sub(&p),
        (Some(t), Some(_)) => {
            // 累積が下がった = 数え直しが始まった: この行の使用量だけ（下がった後の累積を超えない）
            read.resets += 1;
            last.map_or(t, |l| l.min(&t))
        }
        // 累積の無い古い形: 1 回の使用量
        (None, _) => last.unwrap_or_default(),
    };
    if let Some(t) = total {
        state.total = Some(t);
    }
    let session = state
        .session
        .clone()
        .unwrap_or_else(|| fallback_session.to_string());
    let tokens = delta.tokens();
    if !tokens.is_zero() {
        let at = at_ms.div_euclid(1000);
        let key = BucketKey {
            t: bucket_of(at),
            session: session.clone(),
            model: state
                .model
                .clone()
                .unwrap_or_else(|| UNKNOWN_MODEL.to_string()),
            fast: false,
        };
        batch.add(key, &tokens, 1);
        batch.responses += 1;
        batch.touch(&session, at, cwd);
    } else if carries_usage {
        batch.duplicates += 1;
    }
    let Some(rl) = payload.rate_limits.as_ref().and_then(|v| v.as_object()) else {
        return;
    };
    let windows: Vec<RawWindow> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| rl.get(*k).and_then(RawWindow::from_value))
        .collect();
    if rl
        .get("limit_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty() && id != MAIN_LIMIT_ID)
    {
        read.skipped_limits += windows.len() as u64;
        return;
    }
    for w in windows {
        let kind = w.window_minutes.and_then(kind_for_minutes);
        let resets = w.resets_at.filter(|r| *r > 0);
        let percent = w.used_percent.filter(|p| p.is_finite());
        match (kind, resets, percent) {
            (Some(kind), Some(resets_at), Some(percent)) => read.limits.push(CodexLimit {
                kind,
                percent,
                resets_at,
                observed_at: at_ms.max(0) as u64,
            }),
            _ => read.skipped_limits += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = "2026-10-16T00:00:00.000Z";

    /// `token_count` の行（累積・この回・rate_limits）
    fn tc(ts: &str, total: [u64; 3], last: [u64; 3], limits: &str) -> String {
        let u = |v: [u64; 3]| {
            format!(
                r#"{{"input_tokens":{},"cached_input_tokens":{},"cache_write_input_tokens":0,"output_tokens":{},"reasoning_output_tokens":0,"total_tokens":{}}}"#,
                v[0],
                v[1],
                v[2],
                v[0] + v[2]
            )
        };
        format!(
            r#"{{"timestamp":"{ts}","ordinal":1,"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{},"model_context_window":258400}},"rate_limits":{limits}}}}}"#,
            u(total),
            u(last)
        )
    }

    fn limits(p5: f64, r5: i64, p7: f64, r7: i64) -> String {
        format!(
            r#"{{"limit_id":"codex","limit_name":null,"primary":{{"used_percent":{p5},"window_minutes":300,"resets_at":{r5}}},"secondary":{{"used_percent":{p7},"window_minutes":10080,"resets_at":{r7}}},"credits":null,"plan_type":"plus","rate_limit_reached_type":null}}"#
        )
    }

    fn meta(extra: &str) -> String {
        format!(
            r#"{{"timestamp":"{META}","type":"session_meta","payload":{{"id":"thread-1","timestamp":"{META}","cwd":"/home/testuser/dev/alpha","originator":"codex_cli_rs","cli_version":"0.150.1","base_instructions":{{"text":"指示の本文"}},"source":"cli"{extra}}}}}"#
        )
    }

    fn turn(model: &str) -> String {
        format!(
            r#"{{"timestamp":"{META}","type":"turn_context","payload":{{"turn_id":"t","cwd":"/home/testuser/dev/alpha","model":"{model}","effort":"high"}}}}"#
        )
    }

    fn body() -> String {
        r#"{"timestamp":"2026-10-16T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"本文の印 \"type\":\"token_count\""}]}}"#.to_string()
    }

    fn ts(min: i64, sec: i64) -> String {
        format!("2026-10-16T00:{min:02}:{sec:02}.250Z")
    }

    fn run(text: &str, state: &mut CodexCursor) -> (Batch, CodexRead, usize) {
        let mut batch = Batch::default();
        let mut read = CodexRead::default();
        let mut cwd = None;
        let n = ingest_codex_lines(
            text.as_bytes(),
            CodexProgress {
                state,
                fallback_session: "rollout-file",
                cwd: &mut cwd,
            },
            &mut batch,
            &mut read,
        );
        (batch, read, n)
    }

    fn sum_total(batch: &Batch) -> u64 {
        batch.rows().iter().map(|r| r.tokens.total()).sum()
    }

    const R5: i64 = 1_776_300_000;
    const R7: i64 = 1_776_900_000;

    #[test]
    fn 累積の差分の和が最後の累積に一致する_重複と欠けた行を含む() {
        let lim = limits(1.0, R5, 2.0, R7);
        let lines = [
            meta(""),
            turn("gpt-5.5"),
            body(),
            tc(&ts(1, 0), [100, 40, 10], [100, 40, 10], &lim),
            // 同じ累積の行（重複）= 0
            tc(&ts(1, 1), [100, 40, 10], [100, 40, 10], &lim),
            tc(&ts(2, 0), [250, 140, 30], [150, 100, 20], &lim),
            // 次の行（累積 400）が欠けている: 次の累積の差が埋める（last を足すと 150 を落とす）
            tc(
                &ts(9, 0),
                [700, 500, 90],
                [300, 200, 30],
                &limits(3.0, R5, 4.0, R7),
            ),
        ];
        let text = lines.join("\n") + "\n";
        let mut state = CodexCursor::default();
        let (batch, read, n) = run(&text, &mut state);
        assert_eq!(n, text.len());
        assert_eq!(
            sum_total(&batch),
            700 + 90,
            "差の和 = 最後の累積の total_tokens"
        );
        assert_eq!(batch.responses, 3);
        assert_eq!(batch.duplicates, 1);
        // 5 成分: 入力 = 入力 − 読み出し・読み出し・出力
        let mut t = Tokens::default();
        for r in batch.rows() {
            t.add(&r.tokens);
            assert_eq!(r.session, "thread-1");
            assert_eq!(r.model, "gpt-5.5");
        }
        assert_eq!((t.input, t.cache_read, t.output), (200, 500, 90));
        assert_eq!(state.total.unwrap().total, 790);
        // 観測: 行ごとに 5h / 7d（重複除去は書く側）
        assert_eq!(read.limits.len(), 8);
        assert_eq!(read.limits[0].kind, FIVE_HOUR);
        assert_eq!(read.limits[1].kind, SEVEN_DAY);
        assert_eq!(
            read.limits[0].observed_at,
            parse_ms(&ts(1, 0)).unwrap() as u64
        );
        assert_eq!(read.limits[7].percent, 4.0);
        // 会話の鍵の材料
        let seen = batch.sessions.get("thread-1").unwrap();
        assert_eq!(seen.cwd.as_deref(), Some("/home/testuser/dev/alpha"));
    }

    #[test]
    fn 窓をまたいでも差は時刻の刻みへ入り和は変わらない() {
        // 5h の窓の解除（R5）をまたぐ 2 行。刻みは行の時刻
        let a = tc(
            "2026-10-16T09:59:59.000Z",
            [10, 0, 1],
            [10, 0, 1],
            &limits(99.0, R5, 50.0, R7),
        );
        let b = tc(
            "2026-10-16T10:00:01.000Z",
            [30, 0, 5],
            [20, 0, 4],
            &limits(1.0, R5 + 5 * 3600, 51.0, R7),
        );
        let mut state = CodexCursor::default();
        let (batch, read, _) = run(&format!("{a}\n{b}\n"), &mut state);
        let rows = batch.rows();
        assert_eq!(rows.len(), 2, "時刻の刻みが違う");
        assert_eq!(sum_total(&batch), 35);
        assert_eq!(rows[0].model, UNKNOWN_MODEL, "turn_context より前");
        assert_eq!(
            rows[0].session, "rollout-file",
            "session_meta が無ければファイル名"
        );
        // 新しい窓の解除時刻がそのまま観測に乗る
        let r: Vec<i64> = read
            .limits
            .iter()
            .filter(|l| l.kind == FIVE_HOUR)
            .map(|l| l.resets_at)
            .collect();
        assert_eq!(r, vec![R5, R5 + 5 * 3600]);
    }

    #[test]
    fn 累積のリセットはその行の使用量から数え直す() {
        let lim = limits(1.0, R5, 1.0, R7);
        let lines = [
            tc(&ts(1, 0), [100, 0, 10], [100, 0, 10], &lim),
            tc(&ts(2, 0), [400, 0, 40], [300, 0, 30], &lim),
            // 数え直し（累積が下がる）: この行の last = 50 + 5
            tc(&ts(3, 0), [50, 0, 5], [50, 0, 5], &lim),
            tc(&ts(4, 0), [120, 0, 12], [70, 0, 7], &lim),
        ];
        let mut state = CodexCursor::default();
        let (batch, read, _) = run(&(lines.join("\n") + "\n"), &mut state);
        assert_eq!(read.resets, 1);
        // 区切りごとの最後の累積の和（440 + 132）
        assert_eq!(sum_total(&batch), 440 + 132);
        assert_eq!(batch.responses, 4);
    }

    #[test]
    fn 更新をまたいでも書きかけの行を残し続きから差を取る() {
        let lim = limits(1.0, R5, 1.0, R7);
        let first = tc(&ts(1, 0), [100, 0, 10], [100, 0, 10], &lim);
        let second = tc(&ts(2, 0), [300, 0, 30], [200, 0, 20], &lim);
        let text = format!("{first}\n{}", &second[..second.len() / 2]);
        let mut state = CodexCursor::default();
        let (a, _, n) = run(&text, &mut state);
        assert_eq!(n, first.len() + 1, "書きかけの最終行は読まない");
        assert_eq!(sum_total(&a), 110);
        // 状態は読み位置の行へ往復する
        let json = serde_json::to_string(&state).unwrap();
        let mut state: CodexCursor = serde_json::from_str(&json).unwrap();
        let (b, _, _) = run(&format!("{second}\n"), &mut state);
        assert_eq!(sum_total(&b), 220);
    }

    #[test]
    fn 読み直しは数え済みの累積を超えた分だけ() {
        let lim = limits(1.0, R5, 1.0, R7);
        let lines = [
            tc(&ts(1, 0), [100, 0, 10], [100, 0, 10], &lim),
            tc(&ts(2, 0), [300, 0, 30], [200, 0, 20], &lim),
        ];
        let mut state = CodexCursor::default();
        let (a, _, _) = run(&(lines[..1].join("\n") + "\n"), &mut state);
        assert_eq!(sum_total(&a), 110);
        // ファイルが置き換わった（同じ中身 + 続き）: 最初から読み直しても 110 は数えない
        let mut state = state.restarted();
        assert!(state.total.is_none());
        let (b, _, _) = run(&(lines.join("\n") + "\n"), &mut state);
        assert_eq!(sum_total(&b), 220);
        assert_eq!(state.counted, None, "追いついたら外す");
    }

    #[test]
    fn forkの写しの束は数えず基準だけ進める() {
        let lim = limits(1.0, R5, 1.0, R7);
        // session_meta（00:00:00.000）から 1 秒以内に続く 2 行 = 親の履歴の写し
        let lines = [
            meta(r#","parent_thread_id":"parent-1""#),
            tc(
                "2026-10-16T00:00:00.020Z",
                [1000, 0, 100],
                [1000, 0, 100],
                &lim,
            ),
            tc(
                "2026-10-16T00:00:00.040Z",
                [3000, 0, 300],
                [2000, 0, 200],
                &lim,
            ),
            // 子の最初のターン（数秒後）: 写しの累積を基準に増えた分だけ
            tc(
                "2026-10-16T00:00:07.000Z",
                [3500, 0, 350],
                [500, 0, 50],
                &lim,
            ),
        ];
        let mut state = CodexCursor::default();
        let (batch, read, _) = run(&(lines.join("\n") + "\n"), &mut state);
        assert!(state.fork);
        assert_eq!(read.replayed, 2);
        assert_eq!(state.replay_at, None, "子のターンに入った");
        assert_eq!(sum_total(&batch), 550);
        assert_eq!(
            read.limits.len(),
            2,
            "写しの束の rate_limits は観測にしない"
        );
        // fork でなければ同じ行も全部数える
        let mut plain = lines.clone();
        plain[0] = meta("");
        let mut state = CodexCursor::default();
        let (batch, read, _) = run(&(plain.join("\n") + "\n"), &mut state);
        assert_eq!(read.replayed, 0);
        assert_eq!(sum_total(&batch), 3850);
        // source.subagent も fork の印
        let sub = meta("").replace(
            r#""source":"cli""#,
            r#""source":{"subagent":{"other":"x"}}"#,
        );
        let mut state = CodexCursor::default();
        run(&(sub + "\n"), &mut state);
        assert!(state.fork);
    }

    #[test]
    fn 空とtoken_countの無いrolloutと壊れた行は何も数えない() {
        let mut state = CodexCursor::default();
        let (batch, read, n) = run("", &mut state);
        assert_eq!((n, batch.responses, read.limits.len()), (0, 0, 0));
        // session_meta と発話だけ
        let text = [meta(""), turn("gpt-5.5"), body()].join("\n") + "\n";
        let (batch, read, n) = run(&text, &mut state);
        assert_eq!(n, text.len());
        assert!(batch.is_empty());
        assert!(read.limits.is_empty());
        assert_eq!(state.session.as_deref(), Some("thread-1"));
        // 途中で切れた行・壊れた行は飛ばす（改行で終わっていれば読み位置は進む）
        let broken = r#"{"timestamp":"2026-10-16T00:01:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_tok"#;
        let (batch, _, n) = run(&format!("{broken}\n"), &mut state);
        assert_eq!(n, broken.len() + 1);
        assert_eq!(batch.responses, 0);
        // info が null（rate_limits だけ）の行: 使用量は無く観測だけ
        let rate_only = format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{}}}}}"#,
            ts(5, 0),
            limits(7.0, R5, 8.0, R7)
        );
        let (batch, read, _) = run(&format!("{rate_only}\n"), &mut state);
        assert_eq!(batch.responses, 0);
        assert_eq!(batch.duplicates, 0);
        assert_eq!(read.limits.len(), 2);
    }

    #[test]
    fn 別の枠と知らない窓の長さは観測にしない() {
        let other = r#"{"limit_id":"codex_bengalfox","limit_name":"GPT-5.3-Codex-Spark","primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1776300000},"secondary":{"used_percent":6.0,"window_minutes":10080,"resets_at":1776900000}}"#;
        let weekly_primary = r#"{"limit_id":"codex","primary":{"used_percent":3.0,"window_minutes":10080,"resets_at":1776900000},"secondary":{"used_percent":9.0,"window_minutes":43200,"resets_at":1777900000}}"#;
        let lines = [
            tc(&ts(1, 0), [10, 0, 1], [10, 0, 1], other),
            // primary が週の枠のプラン（位置ではなく window_minutes で決める）
            tc(&ts(2, 0), [20, 0, 2], [10, 0, 1], weekly_primary),
        ];
        let mut state = CodexCursor::default();
        let (_, read, _) = run(&(lines.join("\n") + "\n"), &mut state);
        assert_eq!(read.limits.len(), 1);
        assert_eq!(read.limits[0].kind, SEVEN_DAY);
        assert_eq!(read.limits[0].percent, 3.0);
        assert_eq!(read.skipped_limits, 3);
    }

    #[test]
    fn 成分の写しとミリ秒の時刻() {
        let u = CodexUsage {
            input: 100,
            cached: 60,
            cache_write: 10,
            output: 30,
            reasoning: 20,
            total: 130,
        };
        let t = u.tokens();
        assert_eq!(
            t,
            Tokens {
                input: 30,
                output: 30,
                cache_5m: 10,
                cache_1h: 0,
                cache_read: 60
            }
        );
        assert_eq!(t.total(), u.total, "5 成分の和 = total_tokens");
        // 読み出しが入力を超える壊れた値でも負にならない
        let odd = CodexUsage {
            input: 5,
            cached: 9,
            cache_write: 9,
            ..CodexUsage::default()
        };
        assert_eq!(odd.tokens().total(), 5);
        assert_eq!(
            parse_ms("2026-10-16T00:00:01.5Z"),
            Some(parse_ms("2026-10-16T00:00:01Z").unwrap() + 500)
        );
        assert_eq!(
            parse_ms("2026-10-16T00:00:01.123456Z").map(|v| v % 1000),
            Some(123)
        );
        assert_eq!(parse_ms("x"), None);
        assert_eq!(kind_for_minutes(300), Some(FIVE_HOUR));
        assert_eq!(kind_for_minutes(60), None);
    }
}

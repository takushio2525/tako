//! 使用量の内訳（`tako usage --by … --window …` / MCP `tako_sessions` の `action: "usage"`。
//! usage 分析 S2 #2201。設計書 `.agent/plans/2026-10-usage-analytics.md` §3.1 / §3.4 / §4.2）。
//!
//! 索引の行（[`BucketRow`]）と写した鍵（[`SessionKeys`]）から、ログインごとに期間の中の
//! トークン（5 成分）・API 換算・応答数・会話数を、指定した段（[`By`]）で束ねて返す純関数。
//! 段の行には利用制限の 5h / 7d の % を区間按分（[`super::share`]。S3 #2202）で配った値も載せる。
//!
//! # % 列の範囲（S3）
//!
//! - 実窓（`5h` / `7d`）の問い合わせ: 枠ごとに**今の窓**（の、期間に重なる分）。`7d` の既定では 7d の列の合計が
//!   今の 7d の観測と、5h の列の合計が今の 5h の観測と一致する（設計書 §2.4「5h は今の窓だけを出す」）
//! - 暦（`day` / `week` / `month`）と `all`: 期間に重なる**すべての窓の延べ**（窓をまたぐと 100% を超えうる）
//!
//! 按分は期間・絞り込み（`account` / `project`）の前のログインの全行で行い、段へ足すときに絞る
//! （絞ってから配ると、そのプロジェクトが区間の Δ を独り占めする）。
//!
//! # 期間（§3.4）
//!
//! - `5h` / `7d`: ログインごとの**実窓** = 観測した解除時刻 − 長さ（S1 の `limits.jsonl`）。
//!   解除時刻を過ぎた・観測が無いログインは「今から長さだけ遡る」（[`Basis::Rolling`]）
//! - `day` / `week`（月曜始まり）/ `month`: ローカル時刻の暦
//! - `all`: 索引にあるすべて
//!
//! 刻みは 5 分なので、窓の端は刻みの頭へ丸める（端の刻みは丸ごと入る）。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::index::{BucketRow, ProjectFrom, SessionKeys, Tokens};
use super::share::{self, Current, ObservedWindow, UsageSlice};
use super::{price, KeyFrom, LimitObservation};

/// 束ねる段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// ログインの合計だけ
    Login,
    /// tako のアカウント（設定 dir）
    Account,
    Project,
    /// プロジェクト × 主の Issue
    Issue,
    /// 委任台帳の task_type
    TaskType,
    /// 会話（プロジェクト・Issue・ラベルつき）
    Session,
    Model,
    /// ローカル時刻の日付
    Day,
    /// master / worker / solo / 手動
    Role,
}

impl By {
    pub const ALL: [By; 9] = [
        By::Project,
        By::Issue,
        By::TaskType,
        By::Session,
        By::Model,
        By::Account,
        By::Day,
        By::Role,
        By::Login,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            By::Login => "login",
            By::Account => "account",
            By::Project => "project",
            By::Issue => "issue",
            By::TaskType => "task-type",
            By::Session => "session",
            By::Model => "model",
            By::Day => "day",
            By::Role => "role",
        }
    }

    pub fn parse(s: &str) -> Option<By> {
        let s = s.trim().to_ascii_lowercase().replace('_', "-");
        By::ALL.into_iter().find(|b| b.as_str() == s)
    }
}

/// 期間
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    FiveHour,
    SevenDay,
    Day,
    Week,
    Month,
    All,
}

impl Window {
    pub const ALL: [Window; 6] = [
        Window::FiveHour,
        Window::SevenDay,
        Window::Day,
        Window::Week,
        Window::Month,
        Window::All,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Window::FiveHour => "5h",
            Window::SevenDay => "7d",
            Window::Day => "day",
            Window::Week => "week",
            Window::Month => "month",
            Window::All => "all",
        }
    }

    pub fn parse(s: &str) -> Option<Window> {
        let s = s.trim().to_ascii_lowercase();
        Window::ALL.into_iter().find(|w| w.as_str() == s)
    }

    /// 実窓の枠（S1 の観測の `kind`）と長さ（秒）
    fn limit_window(self) -> Option<(&'static str, i64)> {
        match self {
            Window::FiveHour => Some(("five_hour", 5 * 3600)),
            Window::SevenDay => Some(("seven_day", 7 * 86_400)),
            _ => None,
        }
    }
}

/// 期間の決まり方
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// 観測した解除時刻 − 長さ（実窓）
    Observed,
    /// 観測が無い・解除時刻を過ぎた → 今から長さだけ遡る（最後に解けた窓の解除時刻より前へは遡らない）
    Rolling,
    /// ローカル時刻の暦
    Calendar,
    All,
}

/// 期間（unix 秒。`from` が無ければ始まりなし）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Range {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    pub to: i64,
    pub basis: Basis,
    /// 実窓の解除時刻
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

fn local_day_start(now: i64, utc_offset: i32) -> i64 {
    let off = i64::from(utc_offset);
    (now + off).div_euclid(86_400) * 86_400 - off
}

/// ログイン `login` の期間
pub fn window_range(
    window: Window,
    login: &str,
    observations: &[LimitObservation],
    now: i64,
    utc_offset: i32,
) -> Range {
    let calendar = |from: i64| Range {
        from: Some(from),
        to: now,
        basis: Basis::Calendar,
        resets_at: None,
    };
    match window {
        Window::All => Range {
            from: None,
            to: now,
            basis: Basis::All,
            resets_at: None,
        },
        Window::Day => calendar(local_day_start(now, utc_offset)),
        Window::Week => {
            let today = local_day_start(now, utc_offset);
            let off = i64::from(utc_offset);
            // 1970-01-01 は木曜（月曜 = 0 にする）
            let weekday = ((now + off).div_euclid(86_400) + 3).rem_euclid(7);
            calendar(today - weekday * 86_400)
        }
        Window::Month => {
            let off = i64::from(utc_offset);
            let (y, m, _) = crate::limit_resume::civil_from_days((now + off).div_euclid(86_400));
            calendar(crate::limit_resume::days_from_civil(y, m, 1) * 86_400 - off)
        }
        Window::FiveHour | Window::SevenDay => {
            let (kind, len) = window.limit_window().unwrap_or(("", 0));
            // 窓の切り方は区間按分（S3）と同じ 1 実装（解除時刻の揺れの寄せ・前の窓との重なりの切り詰め）
            let (windows, _) = share::windows_for(observations, login, kind, len);
            match share::current_window(&windows, len, now) {
                Current::Observed {
                    start, resets_at, ..
                } => Range {
                    from: Some(start),
                    to: now,
                    basis: Basis::Observed,
                    resets_at: Some(resets_at),
                },
                Current::Rolling { start } => Range {
                    from: Some(start),
                    to: now,
                    basis: Basis::Rolling,
                    resets_at: None,
                },
            }
        }
    }
}

/// 読むべき最も早い時刻（どのログインの期間もこれより前は始まらない。読む月のファイルを決める）
pub fn earliest_from(window: Window, now: i64, utc_offset: i32) -> Option<i64> {
    match window {
        Window::FiveHour | Window::SevenDay => window
            .limit_window()
            .map(|(_, len)| now - len - BUCKET_SLACK),
        other => window_range(other, "", &[], now, utc_offset).from,
    }
}

/// 索引から読む最も早い時刻（S3）。期間の始まりに重なる窓の区間（窓は期間の始まりより最大 7 日前に始まる）と、
/// 推定の係数を取る直近 7 日の区間（その窓の始まりまで）を配れるだけ遡る
pub fn read_from(window: Window, now: i64, utc_offset: i32) -> Option<i64> {
    earliest_from(window, now, utc_offset)
        .map(|f| f.min(now - share::COEFFICIENT_RECENT_SECS) - 7 * 86_400 - BUCKET_SLACK)
}

/// 窓の端を刻みの頭へ丸める分の余裕
const BUCKET_SLACK: i64 = super::index::BUCKET_SECS;

/// 束ねた量
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub tokens: Tokens,
    pub total_tokens: u64,
    /// API 換算（USD。単価表に無いモデルの分は含めない = `unpriced_tokens`）
    pub api_usd: f64,
    /// 単価表に無いモデルのトークン数
    #[serde(skip_serializing_if = "is_zero")]
    pub unpriced_tokens: u64,
    pub responses: u64,
    pub sessions: usize,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// % の桁（小数第 2 位）
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// 1% あたりの API 換算の桁（小数第 6 位 = マイクロドル。行の並びで変わる足し算の末尾の桁を応答に出さない）
fn round_usd6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn rounded_spread(s: share::Spread) -> share::Spread {
    share::Spread {
        median: round_usd6(s.median),
        p25: round_usd6(s.p25),
        p75: round_usd6(s.p75),
    }
}

/// 1 つの枠の段の %（観測から配った分と推定）
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Share {
    /// 観測した % の増分から配った分
    pub percent: f64,
    /// 推定（観測の区間の外の使用量 ÷ 1% あたりの API 換算）
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub estimated: f64,
}

impl Share {
    fn rounded(self) -> Share {
        Share {
            percent: round2(self.percent),
            estimated: round2(self.estimated),
        }
    }
}

/// 段の行の 5h / 7d の %（観測の無い枠は無し）
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct GroupShares {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<Share>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<Share>,
}

impl GroupShares {
    fn slot(&mut self, kind: &str) -> &mut Option<Share> {
        if kind == share::FIVE_HOUR {
            &mut self.five_hour
        } else {
            &mut self.seven_day
        }
    }

    pub fn get(&self, kind: &str) -> Option<Share> {
        if kind == share::FIVE_HOUR {
            self.five_hour
        } else {
            self.seven_day
        }
    }
}

/// % 列の範囲
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareScope {
    /// 枠の今の窓（の、期間に重なる分）
    CurrentWindow,
    /// 期間に重なるすべての窓の延べ
    Period,
}

/// ログインの 1 つの枠の按分の要約
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindShare {
    /// `five_hour` / `seven_day`
    pub kind: &'static str,
    /// このログインのこの枠の観測があるか（無ければ % は出さない = tako mod の報告が無いログイン）
    pub available: bool,
    pub scope: ShareScope,
    /// % 列の範囲（unix 秒。`all` は始まりなし）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    pub to: i64,
    /// 今の窓の解除時刻（`current_window` で観測した窓があるとき）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// 今の窓の最新の観測（%）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_percent: Option<f64>,
    /// 範囲の中で観測から配った % の合計（段 + tako の外。絞り込みの前）。今の窓を丸ごと見るときは
    /// `latest_percent` と一致する
    pub observed: f64,
    /// そのうち段へ配った分（絞り込みの後）
    pub allocated: f64,
    /// tako の外（区間にローカルの使用量が無いのに % が増えた分）
    pub outside: f64,
    /// 推定（最後の観測より後・観測の無い窓の使用量 ÷ 係数。絞り込みの後）
    pub estimated: f64,
    /// 推定できなかった API 換算（係数が無い）
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub unestimated_usd: f64,
    /// 推定できなかった単価不明のトークン
    #[serde(skip_serializing_if = "is_zero")]
    pub unestimated_tokens: u64,
    /// 推定の係数（直近 7 日の区間の 1% あたりの量の中央値）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coefficient: Option<share::Coefficient>,
    /// 範囲に重なる窓の当てはまり（新しい方から [`MAX_WINDOW_FITS`] 個まで。古い順）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<share::WindowFit>,
    /// 載せきれなかった窓の数
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub more_windows: usize,
    /// 単価表に無いモデルの行を含み、生トークンの比で配った区間の数
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub token_weighted_intervals: usize,
    /// 使わなかった観測（下がった値 = 古い報告・窓の外の時刻・解除時刻の無いもの）
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub ignored_observations: usize,
}

/// `KindShare::windows` に載せる窓の上限
pub const MAX_WINDOW_FITS: usize = 10;

/// 束ねる鍵（段に要る欄だけが埋まる）
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// master / solo のプロファイル（プロジェクトの代わり）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// 1 段ぶんの行
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub key: GroupKey,
    /// 表示名（「未分類」「Issue なし」「手動」などの言い換え済み）
    pub name: String,
    #[serde(flatten)]
    pub totals: Totals,
    /// 利用制限の 5h / 7d の %（S3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_share: Option<GroupShares>,
}

/// ログインごとの内訳
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoginUsage {
    pub login: String,
    pub key_from: KeyFrom,
    pub range: Range,
    pub total: Totals,
    pub groups: Vec<Group>,
    /// `limit` で切った残りの段の数
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub more_groups: usize,
    /// 利用制限の % の按分（S3。5h → 7d の順）
    pub limit_share: Vec<KindShare>,
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

/// 内訳の全体
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breakdown {
    pub by: &'static str,
    pub window: &'static str,
    pub logins: Vec<LoginUsage>,
    /// 単価表に無いモデル（正規化した名前）
    pub unpriced_models: Vec<String>,
    /// 鍵の無い会話のトークン（取り込みの途中で落ちた等。ふつうは 0）
    #[serde(skip_serializing_if = "is_zero")]
    pub unkeyed_tokens: u64,
    pub price_table_as_of: &'static str,
}

/// 問い合わせ
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    pub by: By,
    pub window: Window,
    /// tako のアカウント名で絞る
    pub account: Option<&'a str>,
    /// プロジェクト（またはプロファイル）で絞る
    pub project: Option<&'a str>,
    /// 段の上位の件数
    pub limit: usize,
}

/// 会話のログイン（mod の報告 = S1 の対応が正、無ければ置き場から推した鍵）
pub fn effective_login<'a>(
    session: &str,
    keys: Option<&'a SessionKeys>,
    mod_logins: &'a BTreeMap<String, (String, KeyFrom)>,
) -> Option<(&'a str, KeyFrom)> {
    if let Some((login, from)) = mod_logins.get(session) {
        return Some((login.as_str(), *from));
    }
    keys.map(|k| (k.login.as_str(), k.key_from))
}

/// 索引に現れたログインと、その最初の時刻（epoch ms）。S1 の要約の短い名前（A, B, …）を
/// 観測の無いログインにも振るのに使う
pub fn login_first_seen(
    keys: &BTreeMap<String, SessionKeys>,
    mod_logins: &BTreeMap<String, (String, KeyFrom)>,
) -> Vec<(String, u64, KeyFrom)> {
    let mut first: BTreeMap<&str, (u64, KeyFrom)> = BTreeMap::new();
    for (sid, k) in keys {
        let Some((login, from)) = effective_login(sid, Some(k), mod_logins) else {
            continue;
        };
        let at = (k.first_at.max(0) as u64).saturating_mul(1000);
        let entry = first.entry(login).or_insert((at, from));
        if at < entry.0 {
            entry.0 = at;
        }
    }
    first
        .into_iter()
        .map(|(l, (at, from))| (l.to_string(), at, from))
        .collect()
}

fn local_date(t: i64, utc_offset: i32) -> String {
    let (y, m, d) =
        crate::limit_resume::civil_from_days((t + i64::from(utc_offset)).div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

fn group_key(by: By, row: &BucketRow, keys: Option<&SessionKeys>, utc_offset: i32) -> GroupKey {
    let project = || match keys {
        Some(k) if k.project_from == Some(ProjectFrom::Profile) => (None, k.project.clone()),
        Some(k) => (k.project.clone(), None),
        None => (None, None),
    };
    match by {
        By::Login => GroupKey::default(),
        By::Account => GroupKey {
            account: keys.and_then(|k| k.account.clone()),
            ..GroupKey::default()
        },
        By::Project => {
            let (project, profile) = project();
            GroupKey {
                project,
                profile,
                ..GroupKey::default()
            }
        }
        By::Issue => {
            let (project, profile) = project();
            GroupKey {
                project,
                profile,
                issue: keys.and_then(|k| k.issue),
                ..GroupKey::default()
            }
        }
        By::TaskType => GroupKey {
            task_type: keys.and_then(|k| k.task_type.clone()),
            ..GroupKey::default()
        },
        By::Session => {
            let (project, profile) = project();
            GroupKey {
                project,
                profile,
                issue: keys.and_then(|k| k.issue),
                role: keys.and_then(|k| k.role.clone()),
                session: Some(row.session.clone()),
                label: keys.and_then(|k| k.label.clone()),
                ..GroupKey::default()
            }
        }
        By::Model => GroupKey {
            model: Some(price::normalize_model(&row.model)),
            ..GroupKey::default()
        },
        By::Day => GroupKey {
            day: Some(local_date(row.t, utc_offset)),
            ..GroupKey::default()
        },
        By::Role => GroupKey {
            role: keys.and_then(|k| k.role.clone()),
            ..GroupKey::default()
        },
    }
}

/// 表示名
pub fn group_name(by: By, key: &GroupKey) -> String {
    let project = || match (&key.project, &key.profile) {
        (Some(p), _) => p.clone(),
        (None, Some(p)) => format!("プロファイル: {p}"),
        (None, None) => "未分類".to_string(),
    };
    let issue = || {
        key.issue
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| "Issue なし".to_string())
    };
    match by {
        By::Login => "合計".to_string(),
        By::Account => key.account.clone().unwrap_or_else(|| "不明".to_string()),
        By::Project => project(),
        By::Issue => format!("{} {}", project(), issue()),
        By::TaskType => key
            .task_type
            .clone()
            .unwrap_or_else(|| "種別なし".to_string()),
        By::Session => {
            let sid: String = key
                .session
                .as_deref()
                .unwrap_or("-")
                .chars()
                .take(8)
                .collect();
            let role = key.role.as_deref().unwrap_or("手動");
            match &key.label {
                Some(label) => format!("{sid} {role} {} {label}", project()),
                None => format!("{sid} {role} {}", project()),
            }
        }
        By::Model => key.model.clone().unwrap_or_else(|| "-".to_string()),
        By::Day => key.day.clone().unwrap_or_default(),
        By::Role => key.role.clone().unwrap_or_else(|| "手動".to_string()),
    }
}

#[derive(Default)]
struct Acc {
    tokens: Tokens,
    api_usd: f64,
    unpriced: u64,
    responses: u64,
    sessions: BTreeSet<String>,
}

impl Acc {
    fn add(&mut self, row: &BucketRow, unpriced_models: &mut BTreeSet<String>) {
        self.tokens.add(&row.tokens);
        self.responses += u64::from(row.responses);
        self.sessions.insert(row.session.clone());
        match price::cost_usd(&row.model, row.fast, &row.tokens) {
            Some(usd) => self.api_usd += usd,
            None => {
                self.unpriced += row.tokens.total();
                if !row.tokens.is_zero() {
                    unpriced_models.insert(price::normalize_model(&row.model));
                }
            }
        }
    }

    fn totals(&self) -> Totals {
        Totals {
            tokens: self.tokens,
            total_tokens: self.tokens.total(),
            api_usd: self.api_usd,
            unpriced_tokens: self.unpriced,
            responses: self.responses,
            sessions: self.sessions.len(),
        }
    }
}

/// 段の絞り込み（`account` / `project`）に通るか
fn passes(query: &Query<'_>, keys: Option<&SessionKeys>) -> bool {
    if let Some(account) = query.account {
        if keys.and_then(|k| k.account.as_deref()) != Some(account) {
            return false;
        }
    }
    if let Some(project) = query.project {
        if keys.and_then(|k| k.project.as_deref()) != Some(project) {
            return false;
        }
    }
    true
}

/// % 列の範囲（[`ShareScope`]）と今の窓の添字
fn column_period(
    query: Window,
    range: &Range,
    windows: &[ObservedWindow],
    len: i64,
    now: i64,
) -> (Option<i64>, i64, ShareScope, Option<usize>) {
    match query {
        Window::FiveHour | Window::SevenDay => {
            let (start, current) = match share::current_window(windows, len, now) {
                Current::Observed { index, start, .. } => (start, Some(index)),
                Current::Rolling { start } => (start, None),
            };
            let from = range.from.map_or(start, |f| f.max(start));
            (Some(from), now, ShareScope::CurrentWindow, current)
        }
        _ => (range.from, range.to, ShareScope::Period, None),
    }
}

/// 1 つのログインの区間按分（S3）: 枠ごとの要約と、段ごとの %
#[allow(clippy::too_many_arguments)]
fn login_shares(
    login: &str,
    login_rows: &[usize],
    rows: &[BucketRow],
    keys: &BTreeMap<String, SessionKeys>,
    observations: &[LimitObservation],
    query: &Query<'_>,
    range: &Range,
    now: i64,
    utc_offset: i32,
) -> (Vec<KindShare>, BTreeMap<GroupKey, GroupShares>) {
    let slices: Vec<UsageSlice> = login_rows
        .iter()
        .map(|&ri| {
            let r = &rows[ri];
            UsageSlice {
                t: r.t,
                span: super::index::row_span(r.t, now),
                usd: price::cost_usd(&r.model, r.fast, &r.tokens),
                tokens: r.tokens.total(),
            }
        })
        .collect();
    let mut groups: BTreeMap<GroupKey, GroupShares> = BTreeMap::new();
    let mut kinds = Vec::with_capacity(share::KINDS.len());
    for (kind, len) in share::KINDS {
        let (windows, unusable) = share::windows_for(observations, login, kind, len);
        let (from, to, scope, current) = column_period(query.window, range, &windows, len, now);
        let mut ks = KindShare {
            kind,
            available: !windows.is_empty(),
            scope,
            from,
            to,
            resets_at: current.map(|i| windows[i].resets_at),
            latest_percent: current.map(|i| windows[i].latest_percent()),
            observed: 0.0,
            allocated: 0.0,
            outside: 0.0,
            estimated: 0.0,
            unestimated_usd: 0.0,
            unestimated_tokens: 0,
            coefficient: None,
            windows: Vec::new(),
            more_windows: 0,
            token_weighted_intervals: 0,
            ignored_observations: unusable + windows.iter().map(|w| w.ignored).sum::<usize>(),
        };
        if !ks.available {
            kinds.push(ks);
            continue;
        }
        // codex（S5）は観測の区間の外を推定しない（使用量と観測が同じ行に載る = share の冒頭）
        let alloc = share::allocate_with(
            &slices,
            &windows,
            now,
            !share::codex_only(observations, login, kind),
        );
        let (lo, hi) = (from.unwrap_or(i64::MIN), to);
        let mut unestimated_tokens = 0.0;
        for piece in &alloc.pieces {
            let f = share::overlap_fraction(piece.from, piece.to, lo, hi);
            if f <= 0.0 {
                continue;
            }
            ks.observed += piece.observed * f;
            let row = &rows[login_rows[piece.slice]];
            let k = keys.get(&row.session);
            if !passes(query, k) {
                continue;
            }
            ks.allocated += piece.observed * f;
            ks.estimated += piece.estimated * f;
            ks.unestimated_usd += piece.unestimated_usd * f;
            unestimated_tokens += piece.unestimated_tokens * f;
            if query.by != By::Login {
                let slot = groups
                    .entry(group_key(query.by, row, k, utc_offset))
                    .or_default()
                    .slot(kind)
                    .get_or_insert_with(Share::default);
                slot.percent += piece.observed * f;
                slot.estimated += piece.estimated * f;
            }
        }
        for o in &alloc.outside {
            let f = share::overlap_fraction(o.from, o.to, lo, hi);
            ks.outside += o.percent * f;
            ks.observed += o.percent * f;
        }
        let fits: Vec<share::WindowFit> = alloc
            .fits
            .into_iter()
            .filter(|w| w.start < hi && w.resets_at > lo)
            .collect();
        ks.more_windows = fits.len().saturating_sub(MAX_WINDOW_FITS);
        ks.windows = fits
            .into_iter()
            .skip(ks.more_windows)
            .map(|mut w| {
                w.usd_per_percent = w.usd_per_percent.map(rounded_spread);
                w
            })
            .collect();
        ks.coefficient = alloc.coefficient.map(|c| share::Coefficient {
            usd_per_percent: c.usd_per_percent.map(round_usd6),
            tokens_per_percent: c.tokens_per_percent.map(f64::round),
            ..c
        });
        ks.token_weighted_intervals = alloc.token_weighted_intervals;
        ks.observed = round2(ks.observed);
        ks.allocated = round2(ks.allocated);
        ks.outside = round2(ks.outside);
        ks.estimated = round2(ks.estimated);
        ks.unestimated_usd = round_usd6(ks.unestimated_usd);
        ks.unestimated_tokens = unestimated_tokens.round() as u64;
        kinds.push(ks);
    }
    (kinds, groups)
}

/// 内訳を組む。`rows` は期間を含む月の行（同じ鍵の行が複数あってよい。区間按分のため期間より前の行も
/// [`read_from`] まで含めてよい = 量は期間の中の行だけを数える）
pub fn breakdown(
    rows: &[BucketRow],
    keys: &BTreeMap<String, SessionKeys>,
    mod_logins: &BTreeMap<String, (String, KeyFrom)>,
    observations: &[LimitObservation],
    query: &Query<'_>,
    now: i64,
    utc_offset: i32,
) -> Breakdown {
    struct PerLogin {
        key_from: KeyFrom,
        range: Range,
        total: Acc,
        groups: BTreeMap<GroupKey, Acc>,
        /// このログインの全行（期間・絞り込みの前。区間按分に使う）
        rows: Vec<usize>,
    }
    let new_login = |login: &str, from: KeyFrom| PerLogin {
        key_from: from,
        range: window_range(query.window, login, observations, now, utc_offset),
        total: Acc::default(),
        groups: BTreeMap::new(),
        rows: Vec::new(),
    };
    let mut logins: BTreeMap<String, PerLogin> = BTreeMap::new();
    let mut unpriced_models = BTreeSet::new();
    let mut unkeyed = 0u64;
    for (ri, row) in rows.iter().enumerate() {
        let k = keys.get(&row.session);
        let Some((login, from)) = effective_login(&row.session, k, mod_logins) else {
            unkeyed += row.tokens.total();
            continue;
        };
        let entry = logins
            .entry(login.to_string())
            .or_insert_with(|| new_login(login, from));
        entry.rows.push(ri);
        if !passes(query, k) {
            continue;
        }
        let in_range = entry
            .range
            .from
            .is_none_or(|f| row.t >= super::index::bucket_of(f))
            && row.t <= entry.range.to;
        if !in_range {
            continue;
        }
        entry.total.add(row, &mut unpriced_models);
        if query.by != By::Login {
            entry
                .groups
                .entry(group_key(query.by, row, k, utc_offset))
                .or_default()
                .add(row, &mut unpriced_models);
        }
    }
    // 使用量が無くても観測のあるログイン（claude.ai だけで使った等 = すべて tako の外）。絞り込みの無いときだけ
    if query.account.is_none() && query.project.is_none() {
        for o in observations {
            if !logins.contains_key(&o.login) {
                let entry = new_login(&o.login, o.key_from);
                logins.insert(o.login.clone(), entry);
            }
        }
    }
    let main_kind = if query.window == Window::FiveHour {
        share::FIVE_HOUR
    } else {
        share::SEVEN_DAY
    };
    let logins = logins
        .into_iter()
        .filter_map(|(login, p)| {
            let (limit_share, mut shares) = login_shares(
                &login,
                &p.rows,
                rows,
                keys,
                observations,
                query,
                &p.range,
                now,
                utc_offset,
            );
            let has_usage = p.total.responses > 0 || !p.total.tokens.is_zero();
            // 絞り込みのあるときは絞った後の分だけで決める（絞り込みに当たる行の無いログインは出さない）
            let filtered = query.account.is_some() || query.project.is_some();
            let has_share = limit_share.iter().any(|k| {
                k.estimated > 0.0
                    || if filtered {
                        k.allocated > 0.0
                    } else {
                        k.observed > 0.0
                    }
            });
            if !has_usage && !has_share {
                return None;
            }
            let mut accs = p.groups;
            // % だけを持つ段（期間の量の外の切れ端）も行にする
            for key in shares.keys() {
                accs.entry(key.clone()).or_default();
            }
            let mut groups: Vec<Group> = accs
                .into_iter()
                .map(|(key, acc)| {
                    let mut gs = shares.remove(&key).unwrap_or_default();
                    for ks in limit_share.iter().filter(|k| k.available) {
                        let slot = gs.slot(ks.kind);
                        *slot = Some(slot.unwrap_or_default().rounded());
                    }
                    let any = gs.five_hour.is_some() || gs.seven_day.is_some();
                    Group {
                        name: group_name(query.by, &key),
                        key,
                        totals: acc.totals(),
                        limit_share: any.then_some(gs),
                    }
                })
                .collect();
            if query.by == By::Day {
                groups.sort_by(|a, b| a.key.day.cmp(&b.key.day));
            } else {
                let score = |g: &Group| {
                    g.limit_share
                        .and_then(|s| s.get(main_kind))
                        .map_or(0.0, |s| s.percent + s.estimated)
                };
                groups.sort_by(|a, b| {
                    score(b)
                        .total_cmp(&score(a))
                        .then(b.totals.api_usd.total_cmp(&a.totals.api_usd))
                        .then(b.totals.total_tokens.cmp(&a.totals.total_tokens))
                        .then(a.name.cmp(&b.name))
                });
            }
            let more = groups.len().saturating_sub(query.limit.max(1));
            groups.truncate(query.limit.max(1));
            Some(LoginUsage {
                login,
                key_from: p.key_from,
                range: p.range,
                total: p.total.totals(),
                groups,
                more_groups: more,
                limit_share,
            })
        })
        .collect();
    Breakdown {
        by: query.by.as_str(),
        window: query.window.as_str(),
        logins,
        unpriced_models: unpriced_models.into_iter().collect(),
        unkeyed_tokens: unkeyed,
        price_table_as_of: price::TABLE_AS_OF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: i64, session: &str, model: &str, output: u64) -> BucketRow {
        BucketRow {
            t,
            session: session.into(),
            model: model.into(),
            fast: false,
            tokens: Tokens {
                output,
                cache_read: 1_000_000,
                ..Tokens::default()
            },
            responses: 1,
        }
    }

    fn keys(
        sid: &str,
        login: &str,
        project: Option<(&str, ProjectFrom)>,
        issue: Option<u32>,
    ) -> SessionKeys {
        SessionKeys {
            session_id: sid.into(),
            login: login.into(),
            key_from: KeyFrom::Account,
            agent: "claude".into(),
            account: Some(if login == "L1" { "univ" } else { "personal" }.into()),
            role: Some("worker".into()),
            project: project.map(|p| p.0.to_string()),
            project_from: project.map(|p| p.1),
            issue,
            task_type: Some("feature".into()),
            ledger_id: None,
            label: issue.map(|n| format!("#{n} 何か")),
            first_at: 0,
            recorded_at: 0,
        }
    }

    const NOW: i64 = 1_792_000_000;

    fn fixture() -> (Vec<BucketRow>, BTreeMap<String, SessionKeys>) {
        let rows = vec![
            row(NOW - 3600, "s1", "claude-opus-5-5", 1_000_000),
            row(NOW - 3600, "s1", "claude-opus-5-5", 0),
            row(NOW - 600, "s2", "claude-opus-5-5", 0),
            row(NOW - 600, "s3", "claude-sonnet-9-9", 5),
            // 8 日前 = 7d の外
            row(NOW - 8 * 86_400, "s1", "claude-opus-5-5", 9),
            row(NOW - 60, "s4", "claude-haiku-4-5-20251001", 0),
        ];
        let mut map = BTreeMap::new();
        for k in [
            keys("s1", "L1", Some(("tako", ProjectFrom::Catalog)), Some(2201)),
            keys("s2", "L1", Some(("tako", ProjectFrom::Catalog)), Some(2202)),
            keys("s3", "L1", Some(("default", ProjectFrom::Profile)), None),
            keys("s4", "L2", None, None),
        ] {
            map.insert(k.session_id.clone(), k);
        }
        (rows, map)
    }

    fn query(by: By, window: Window) -> Query<'static> {
        Query {
            by,
            window,
            account: None,
            project: None,
            limit: 10,
        }
    }

    #[test]
    fn プロジェクトの段とapi換算と単価不明() {
        let (rows, keys) = fixture();
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            &[],
            &query(By::Project, Window::SevenDay),
            NOW,
            0,
        );
        assert_eq!(b.logins.len(), 2);
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        assert_eq!(l1.range.basis, Basis::Rolling, "観測が無ければ今から遡る");
        assert_eq!(l1.total.responses, 4, "8 日前の行は 7d の外");
        assert_eq!(l1.total.sessions, 3);
        assert_eq!(l1.groups[0].name, "tako");
        assert_eq!(l1.groups[0].totals.sessions, 2);
        // tako: 出力 1M × $20 + 読み出し 3M × $0.2
        assert!((l1.groups[0].totals.api_usd - 20.6).abs() < 1e-9);
        let profile = &l1.groups[1];
        assert_eq!(profile.name, "プロファイル: default");
        assert_eq!(profile.totals.api_usd, 0.0);
        assert_eq!(profile.totals.unpriced_tokens, 1_000_005);
        assert_eq!(b.unpriced_models, vec!["claude-sonnet-9-9".to_string()]);
        let l2 = b.logins.iter().find(|l| l.login == "L2").unwrap();
        assert_eq!(l2.groups[0].name, "未分類");
        assert!(
            (l2.total.api_usd - 0.1).abs() < 1e-9,
            "Haiku 4.5 の読み出し"
        );
    }

    #[test]
    fn issueと会話の段_絞り込み_上位の件数() {
        let (rows, keys) = fixture();
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            &[],
            &query(By::Issue, Window::All),
            NOW,
            0,
        );
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        let names: Vec<&str> = l1.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "tako #2201",
                "tako #2202",
                "プロファイル: default Issue なし"
            ]
        );
        assert_eq!(l1.total.responses, 5, "all は 8 日前も入る");
        let mut q = query(By::Session, Window::All);
        q.limit = 1;
        q.account = Some("univ");
        let b = breakdown(&rows, &keys, &BTreeMap::new(), &[], &q, NOW, 0);
        assert_eq!(b.logins.len(), 1, "アカウントで絞る");
        assert_eq!(b.logins[0].groups.len(), 1);
        assert_eq!(b.logins[0].more_groups, 2);
        assert!(b.logins[0].groups[0]
            .name
            .starts_with("s1 worker tako #2201"));
        let mut q = query(By::Project, Window::All);
        q.project = Some("tako");
        let b = breakdown(&rows, &keys, &BTreeMap::new(), &[], &q, NOW, 0);
        assert_eq!(b.logins[0].total.sessions, 2);
    }

    #[test]
    fn 実窓は観測した解除時刻から_modのログインが正() {
        let (rows, keys) = fixture();
        let obs = LimitObservation {
            login: "L1".into(),
            key_from: KeyFrom::Account,
            kind: "five_hour".into(),
            percent: 10.0,
            // 解除まで 4.5 時間 = 窓の開始は 30 分前
            resets_at: Some(NOW + 4 * 3600 + 1800),
            observed_at: 1,
            recorded_at: 1,
            source: "mod".into(),
        };
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            std::slice::from_ref(&obs),
            &query(By::Login, Window::FiveHour),
            NOW,
            0,
        );
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        assert_eq!(l1.range.basis, Basis::Observed);
        assert_eq!(l1.range.from, Some(NOW - 1800));
        assert_eq!(l1.total.responses, 2, "1 時間前の行は窓の外");
        assert!(l1.groups.is_empty());
        // 解除時刻を過ぎた観測は使わない
        let late = window_range(Window::FiveHour, "L1", &[obs], NOW + 5 * 3600, 0);
        assert_eq!(late.basis, Basis::Rolling);
        // mod の報告で別のログインと分かった会話はそちらへ
        let mut mods = BTreeMap::new();
        mods.insert("s4".to_string(), ("L1".to_string(), KeyFrom::Account));
        let b = breakdown(
            &rows,
            &keys,
            &mods,
            &[],
            &query(By::Login, Window::All),
            NOW,
            0,
        );
        assert_eq!(b.logins.len(), 1);
        let first = login_first_seen(&keys, &mods);
        assert_eq!(first.len(), 1);
    }

    #[test]
    fn 暦の期間と日の段() {
        // 2026-10-15（木）12:00 JST
        let off = 9 * 3600;
        let now = crate::limit_resume::days_from_civil(2026, 10, 15) * 86_400 + 3 * 3600;
        let day = window_range(Window::Day, "", &[], now, off);
        assert_eq!(day.from, Some(now - 12 * 3600));
        let week = window_range(Window::Week, "", &[], now, off);
        assert_eq!(week.from, Some(now - 12 * 3600 - 3 * 86_400), "月曜 0 時");
        let month = window_range(Window::Month, "", &[], now, off);
        assert_eq!(month.from, Some(now - 12 * 3600 - 14 * 86_400));
        assert_eq!(earliest_from(Window::All, now, off), None);
        let (rows, keys) = fixture();
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            &[],
            &query(By::Day, Window::All),
            NOW,
            0,
        );
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        assert_eq!(l1.groups.len(), 2);
        assert!(l1.groups[0].name < l1.groups[1].name, "日付は古い順");
    }

    fn limit(login: &str, kind: &str, percent: f64, resets_at: i64, ago: i64) -> LimitObservation {
        LimitObservation {
            login: login.into(),
            key_from: KeyFrom::Account,
            kind: kind.into(),
            percent,
            resets_at: Some(resets_at),
            observed_at: ((NOW - ago) * 1000) as u64,
            recorded_at: 0,
            source: "mod".into(),
        }
    }

    /// #2202: 段の % は今の窓の観測と合計が一致し（段 + tako の外 = 最新の観測）、観測の区間の外は推定。
    /// 絞っても按分は絞る前の全行で行い、絞り込みに当たらないログインは出さない。使用量の無いログインは
    /// 絞り込みの無いときだけ「すべて tako の外」として出る
    #[test]
    fn 段の利用制限の割合は観測と一致し絞っても按分は全行() {
        let (rows, keys) = fixture();
        let r7 = NOW + 3 * 86_400;
        let observations = vec![
            // L1 の 7d: 2 時間前に 10%（それまで使用なし = tako の外）・30 分前に 14%（s1 の 1 時間前の使用）
            limit("L1", "seven_day", 10.0, r7, 7200),
            limit("L1", "seven_day", 14.0, r7, 1800),
            // L2 は区間に使用が無いまま 5%（s4 は最後の観測より後）
            limit("L2", "seven_day", 5.0, NOW + 86_400, 100),
            // L3 は会話記録に現れない（claude.ai だけ）
            limit("L3", "seven_day", 7.0, NOW + 86_400, 100),
        ];
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            &observations,
            &query(By::Project, Window::SevenDay),
            NOW,
            0,
        );
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        let s7 = l1
            .limit_share
            .iter()
            .find(|k| k.kind == "seven_day")
            .unwrap();
        assert_eq!(s7.scope, ShareScope::CurrentWindow);
        assert_eq!(s7.latest_percent, Some(14.0));
        assert_eq!((s7.observed, s7.allocated, s7.outside), (14.0, 4.0, 10.0));
        // 推定: 係数 = 20.4 USD / 4% = 5.1。s2（読み出し 1M = 0.2 USD）→ 0.04・s3（単価不明 1,000,005 トークン /
        // 750,000 トークン毎 %）→ 1.33
        assert!((s7.estimated - 1.37).abs() < 0.011, "{}", s7.estimated);
        assert_eq!(s7.coefficient.unwrap().usd_per_percent, Some(5.1));
        let s5 = l1
            .limit_share
            .iter()
            .find(|k| k.kind == "five_hour")
            .unwrap();
        assert!(!s5.available, "5h の観測は無い");
        let names: Vec<&str> = l1.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["tako", "プロファイル: default"],
            "7d の % の大きい順"
        );
        let tako = l1.groups[0].limit_share.unwrap();
        assert_eq!(tako.seven_day.map(|s| s.percent), Some(4.0));
        assert_eq!(tako.five_hour, None, "観測の無い枠は出さない");
        let default = l1.groups[1].limit_share.unwrap().seven_day.unwrap();
        assert_eq!(default.percent, 0.0);
        assert!((default.estimated - 1.33).abs() < 0.011);
        // L2: 区間に使用が無い → すべて tako の外。最後の観測より後の s4 は係数が無いので % へ直さない
        let l2 = b.logins.iter().find(|l| l.login == "L2").unwrap();
        let s7 = l2
            .limit_share
            .iter()
            .find(|k| k.kind == "seven_day")
            .unwrap();
        assert_eq!((s7.observed, s7.outside, s7.estimated), (5.0, 5.0, 0.0));
        assert!((s7.unestimated_usd - 0.1).abs() < 1e-9);
        // L3: 使用量が無くても観測のあるログインは出る（すべて tako の外）
        let l3 = b
            .logins
            .iter()
            .find(|l| l.login == "L3")
            .expect("観測だけのログイン");
        assert_eq!(l3.limit_share[1].outside, 7.0);
        assert!(l3.groups.is_empty());

        // 絞り込み: tako の % は絞らないときと同じ（区間の Δ を独り占めしない）。当たらないログインは出さない
        let mut q = query(By::Project, Window::SevenDay);
        q.project = Some("tako");
        let b = breakdown(&rows, &keys, &BTreeMap::new(), &observations, &q, NOW, 0);
        assert_eq!(
            b.logins
                .iter()
                .map(|l| l.login.as_str())
                .collect::<Vec<_>>(),
            ["L1"]
        );
        let s7 = &b.logins[0].limit_share[1];
        assert_eq!((s7.observed, s7.allocated), (14.0, 4.0));
        assert!(
            (s7.estimated - 0.04).abs() < 0.011,
            "絞った後の推定は tako の分だけ"
        );

        // 暦の期間は延べ。窓をまたいでも期間に重なる分だけ
        let b = breakdown(
            &rows,
            &keys,
            &BTreeMap::new(),
            &observations,
            &query(By::Login, Window::All),
            NOW,
            0,
        );
        let l1 = b.logins.iter().find(|l| l.login == "L1").unwrap();
        assert_eq!(l1.limit_share[1].scope, ShareScope::Period);
        assert_eq!(l1.limit_share[1].observed, 14.0);
        assert!(l1.groups.is_empty(), "login の段は合計だけ");
    }

    #[test]
    fn 段と期間の名前は往復する() {
        for b in By::ALL {
            assert_eq!(By::parse(b.as_str()), Some(b));
        }
        assert_eq!(By::parse("task_type"), Some(By::TaskType));
        assert_eq!(By::parse("x"), None);
        for w in Window::ALL {
            assert_eq!(Window::parse(w.as_str()), Some(w));
        }
        assert_eq!(Window::parse("7D"), Some(Window::SevenDay));
    }
}

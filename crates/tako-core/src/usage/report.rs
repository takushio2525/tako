//! 使用量の内訳（`tako usage --by … --window …` / MCP `tako_sessions` の `action: "usage"`。
//! usage 分析 S2 #2201。設計書 `.agent/plans/2026-10-usage-analytics.md` §3.1 / §3.4 / §4.2）。
//!
//! 索引の行（[`BucketRow`]）と写した鍵（[`SessionKeys`]）から、ログインごとに期間の中の
//! トークン（5 成分）・API 換算・応答数・会話数を、指定した段（[`By`]）で束ねて返す純関数。
//! **利用制限の % へは配らない**（区間按分は S3 #2202）。
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
    /// 観測が無い・解除時刻を過ぎた → 今から長さだけ遡る
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
            let latest = observations
                .iter()
                .filter(|o| o.login == login && o.kind == kind)
                .filter_map(|o| o.resets_at.map(|r| (o.observed_at, r)))
                .max();
            match latest {
                Some((_, resets)) if resets > now => Range {
                    from: Some(resets - len),
                    to: now,
                    basis: Basis::Observed,
                    resets_at: Some(resets),
                },
                _ => Range {
                    from: Some(now - len),
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

/// 内訳を組む。`rows` は期間を含む月の行（同じ鍵の行が複数あってよい）
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
    }
    let mut logins: BTreeMap<String, PerLogin> = BTreeMap::new();
    let mut unpriced_models = BTreeSet::new();
    let mut unkeyed = 0u64;
    for row in rows {
        let k = keys.get(&row.session);
        let Some((login, from)) = effective_login(&row.session, k, mod_logins) else {
            unkeyed += row.tokens.total();
            continue;
        };
        if let Some(account) = query.account {
            if k.and_then(|k| k.account.as_deref()) != Some(account) {
                continue;
            }
        }
        if let Some(project) = query.project {
            if k.and_then(|k| k.project.as_deref()) != Some(project) {
                continue;
            }
        }
        let entry = logins.entry(login.to_string()).or_insert_with(|| PerLogin {
            key_from: from,
            range: window_range(query.window, login, observations, now, utc_offset),
            total: Acc::default(),
            groups: BTreeMap::new(),
        });
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
    let logins = logins
        .into_iter()
        .filter(|(_, p)| p.total.responses > 0 || !p.total.tokens.is_zero())
        .map(|(login, p)| {
            let mut groups: Vec<Group> = p
                .groups
                .into_iter()
                .map(|(key, acc)| Group {
                    name: group_name(query.by, &key),
                    key,
                    totals: acc.totals(),
                })
                .collect();
            if query.by == By::Day {
                groups.sort_by(|a, b| a.key.day.cmp(&b.key.day));
            } else {
                groups.sort_by(|a, b| {
                    b.totals
                        .api_usd
                        .total_cmp(&a.totals.api_usd)
                        .then(b.totals.total_tokens.cmp(&a.totals.total_tokens))
                        .then(a.name.cmp(&b.name))
                });
            }
            let more = groups.len().saturating_sub(query.limit.max(1));
            groups.truncate(query.limit.max(1));
            LoginUsage {
                login,
                key_from: p.key_from,
                range: p.range,
                total: p.total.totals(),
                groups,
                more_groups: more,
            }
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

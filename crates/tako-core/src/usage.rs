//! usage 分析の判断と型（エピック #2192。S1 = #2200: 利用制限の観測を残す + ログインの鍵）。
//!
//! 設計の正本は `.agent/plans/2026-10-usage-analytics.md`（§1.2 / §1.3 / §3.2 / §5.2 / §6 / §9「S1」）。
//! ここは**純関数と型だけ**（ファイルの読み書き・塩の用意・GUI からの呼び出しは
//! `tako_control::usage`）。
//!
//! # 何を残すか
//!
//! 利用制限の used_percent の履歴は、この機のどこにも残っていない（transcript にも statusLine にも無く、
//! `ModHub` はペインごとに最新の 1 件しか持たない）。後から遡れないので、tako mod の報告が届くたびに
//! **ログイン × 枠（5h / 7d）の（解除時刻, %）が前回と違えば** `<data_dir>/usage/limits.jsonl` へ
//! 1 行足す（[`LimitObservation`]）。報告の `session_id` → ログインの対応も控える（[`SessionLogin`]。
//! S2 が使用量をログインへ帰属させるのに使う）。
//!
//! # ログインの鍵（§1.3 / §3.2）
//!
//! 利用制限はログインごとにかかり、tako のアカウント（設定 dir）は同じログインを重ねうる
//! （この機では 4 アカウントが 2 ログインに収まっていた）。束ねる鍵は、起動に使った資格情報のファイル
//! （`CLAUDE_CONFIG_DIR` があれば `<それ>/.claude.json`、無ければ `<ホーム>/.claude.json`）の
//! `oauthAccount.accountUuid` を、マシン固有の塩（`<data_dir>/machine_id`）で包んだハッシュの先頭
//! [`KEY_HEX_LEN`] 桁にする。**生の accountUuid もメールも持たない・書かない・表示しない**（§6）。
//! 資格情報が無い・読めないときは設定 dir そのものの塩付きハッシュへ落とす（[`KeyFrom::ConfigDir`]）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::claude_mod::{parse_resets_at, ModRateLimit, ModReport};

/// S1 の A/B: 立っていれば観測も会話 → ログインの対応も記録しない（#2200 前の挙動）
pub const S1_LEGACY_ENV: &str = "TAKO_2192_S1_LEGACY";
/// ログインの鍵の桁数（16 進。§3.2 の「12 桁」）
pub const KEY_HEX_LEN: usize = 12;
/// 観測の出どころ: tako mod の報告（S5 で `codex`、画面の読み取りを足すなら `screen`）
pub const SOURCE_MOD: &str = "mod";
/// `ModHub` に積んでおく材料の上限（GUI の定期処理が汲み出さないホストでも溜まり続けない。
/// 超えたら古いものから捨てる）
pub const MAX_PENDING: usize = 256;

/// S1 の A/B（[`S1_LEGACY_ENV`]）が立っているか
pub fn s1_legacy() -> bool {
    std::env::var_os(S1_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// ログインの鍵の出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyFrom {
    /// 資格情報のファイルの `oauthAccount.accountUuid`（塩付きハッシュ）
    Account,
    /// 資格情報が無い・読めない → 設定 dir そのもの（塩付きハッシュ）
    ConfigDir,
}

impl KeyFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyFrom::Account => "account",
            KeyFrom::ConfigDir => "config_dir",
        }
    }
}

/// ログインの鍵（[`KEY_HEX_LEN`] 桁の 16 進）と出どころ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginKey {
    pub key: String,
    pub from: KeyFrom,
}

/// 区切りの語つきの塩付きハッシュの先頭 [`KEY_HEX_LEN`] 桁。区切りの語で、accountUuid の鍵と
/// 設定 dir の鍵が同じ値から作られても突き合わない（他の用途の SHA-256 とも突き合わない）
fn salted_key(domain: &str, salt: &str, value: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"tako.usage.login.v1\n");
    h.update(domain.as_bytes());
    h.update(b"\n");
    h.update(salt.as_bytes());
    h.update(b"\n");
    h.update(value.as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex[..KEY_HEX_LEN].to_string()
}

/// accountUuid からログインの鍵を作る（同じ塩・同じ uuid なら設定 dir が違っても同じ鍵）
pub fn account_key(salt: &str, account_uuid: &str) -> LoginKey {
    LoginKey {
        key: salted_key("account", salt, account_uuid.trim()),
        from: KeyFrom::Account,
    }
}

/// 設定 dir からログインの鍵を作る（資格情報を読めないときの落とし先）
pub fn config_dir_key(salt: &str, config_dir: &Path) -> LoginKey {
    LoginKey {
        key: salted_key("config_dir", salt, &config_dir.to_string_lossy()),
        from: KeyFrom::ConfigDir,
    }
}

/// 報告の `config_dir`（`CLAUDE_CONFIG_DIR` の生の値）を正規化する。空は「無し」、
/// 先頭の `~` はホームへ展開し、末尾の区切りを落とす（`~/.claude/` と `~/.claude` を同じ dir にする）
pub fn normalize_config_dir(raw: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    let expanded = match (raw.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(raw),
    };
    let text = expanded.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        // `/` だけ（根）は落とすと空になるので元のまま
        return Some(expanded);
    }
    Some(PathBuf::from(trimmed))
}

/// 資格情報のファイル（§3.2）: `CLAUDE_CONFIG_DIR` があれば `<それ>/.claude.json`、
/// 無ければ `<ホーム>/.claude.json`。**既定の設定 dir（`~/.claude`）の transcript には 2 通りが混ざる**
/// （`CLAUDE_CONFIG_DIR` 無し = `~/.claude.json` / `=~/.claude` = `~/.claude/.claude.json`）ので、
/// 置き場ではなく報告の `config_dir` で決める
pub fn credentials_file(config_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    match normalize_config_dir(config_dir, home) {
        Some(dir) => Some(dir.join(".claude.json")),
        None => home.map(|h| h.join(".claude.json")),
    }
}

/// 資格情報を読めないときに鍵へ使う設定 dir（`CLAUDE_CONFIG_DIR`、無ければ `<ホーム>/.claude`）
pub fn effective_config_dir(config_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    normalize_config_dir(config_dir, home).or_else(|| home.map(|h| h.join(".claude")))
}

/// `.claude.json` から accountUuid **だけ**を読む。他のキー（会話の履歴・プロジェクトの一覧など）は
/// serde が読み飛ばし、値を持ち出さない
pub fn account_uuid_from_json(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ClaudeJson {
        #[serde(rename = "oauthAccount", default)]
        oauth_account: Option<OauthAccount>,
    }
    #[derive(Deserialize)]
    struct OauthAccount {
        #[serde(rename = "accountUuid", default)]
        account_uuid: Option<String>,
    }
    let parsed: ClaudeJson = serde_json::from_str(text).ok()?;
    parsed
        .oauth_account?
        .account_uuid
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
}

/// `limits.jsonl` の 1 行（観測 1 点。設計書 §5.2）。**値が変わったときだけ**足す
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitObservation {
    /// ログインの鍵（[`LoginKey::key`]）
    pub login: String,
    pub key_from: KeyFrom,
    /// 枠（mod の `kind` = `five_hour` / `seven_day`）
    pub kind: String,
    /// used_percent（mod の値は整数。f64 で届く）
    pub percent: f64,
    /// 解除時刻（unix 秒）。mod の `resets_at` を読めなければ無し
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// 観測した時刻（epoch ms。mod は % か解除時刻が変わったときだけ打ち直す = #1903）
    pub observed_at: u64,
    /// tako が書いた時刻（epoch ms）
    pub recorded_at: u64,
    /// 出どころ（[`SOURCE_MOD`]）
    pub source: String,
}

/// `sessions.jsonl` の 1 行（会話 → ログイン）。S1 はログインの列だけで、S2 が鍵（プロジェクト・
/// Issue・task_type）を足す。同じ `session_id` の行が複数あれば**後ろの行が正**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLogin {
    pub session_id: String,
    pub login: String,
    pub key_from: KeyFrom,
    /// tako がこの対応を初めて見た時刻（epoch ms）
    pub seen_at: u64,
}

/// 報告から積む材料（UI スレッドで積み、背景で鍵を引いて書く = `tako_control::usage`）
#[derive(Debug, Clone, PartialEq)]
pub struct PendingUsage {
    pub session_id: Option<String>,
    /// 報告の `config_dir`（`CLAUDE_CONFIG_DIR` の生の値。既定の設定 dir なら無し）
    pub config_dir: Option<String>,
    /// 同じペインの前の報告から値（% か解除時刻）が変わった枠
    pub limits: Vec<ModRateLimit>,
}

/// 枠の値の同一性（解除時刻は秒へ揃えてから比べる。% は整数で届くので小さな誤差で等しいとみなす）
fn same_value(a: &ModRateLimit, b: &ModRateLimit) -> bool {
    a.kind == b.kind
        && a.resets_at.as_deref().and_then(parse_resets_at)
            == b.resets_at.as_deref().and_then(parse_resets_at)
        && (a.percent_used - b.percent_used).abs() < 1e-6
}

/// 同じペインの前の報告と比べて、積むべきもの（値の変わった枠・初めての会話）を返す。
/// **同じ値の heartbeat は `None`**（UI スレッドの仕事を「比べて積む」だけにする）。
/// 終わった・休眠した報告は積まない（値はその前の報告で積んである）
pub fn pending_from_report(prev: Option<&ModReport>, next: &ModReport) -> Option<PendingUsage> {
    if next.ended || next.dormant.is_some() {
        return None;
    }
    let same_session =
        prev.is_some_and(|p| p.session_id == next.session_id && p.config_dir == next.config_dir);
    let changed: Vec<ModRateLimit> = next
        .rate_limits
        .iter()
        .filter(|limit| {
            !(same_session
                && prev.is_some_and(|p| p.rate_limits.iter().any(|old| same_value(old, limit))))
        })
        .cloned()
        .collect();
    let new_session = !same_session && next.session_id.is_some();
    if changed.is_empty() && !new_session {
        return None;
    }
    Some(PendingUsage {
        session_id: next.session_id.clone(),
        config_dir: next.config_dir.clone(),
        limits: changed,
    })
}

/// ログイン × 枠ごとの「最後に書いた観測」（重複を書かないための覚え）
#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    resets_at: Option<i64>,
    percent: f64,
    observed_at: u64,
}

/// ログイン × 枠の重複除去（§9 S1 の「(解除時刻, %) が前回と違えば 1 行」）。
///
/// 同じログインを複数のペインが報告するので、ペインの前の報告と比べるだけでは足りない
/// （放置したペインの古い % と動いているペインの新しい % が交互に届くと、毎回「前回と違う」になる）。
/// 次の 2 つで弾く:
///
/// 1. 最後に書いた（解除時刻, %）と同じ → 書かない（別のペインの同じ値・heartbeat）
/// 2. 観測時刻（`observed_at`）が最後に書いたものより**古い** → 書かない（放置したペインの古い値。
///    mod は値が変わったときだけ `observed_at` を打つので、古い値は古い時刻のまま届く = #1903）
#[derive(Debug, Clone, Default)]
pub struct LimitTracker {
    last: BTreeMap<(String, String), Seen>,
}

impl LimitTracker {
    /// 既存の行で覚えを作る（再起動の後、最初の追記の前に、ファイルの末尾の行から）
    pub fn seed<'a>(&mut self, rows: impl IntoIterator<Item = &'a LimitObservation>) {
        for row in rows {
            let key = (row.login.clone(), row.kind.clone());
            let seen = Seen {
                resets_at: row.resets_at,
                percent: row.percent,
                observed_at: row.observed_at,
            };
            let newer = self
                .last
                .get(&key)
                .is_none_or(|cur| cur.observed_at <= seen.observed_at);
            if newer {
                self.last.insert(key, seen);
            }
        }
    }

    /// 書くべき観測なら行を組んで覚えを進める（書かないなら `None`）
    pub fn admit(
        &mut self,
        login: &LoginKey,
        limit: &ModRateLimit,
        recorded_at: u64,
    ) -> Option<LimitObservation> {
        let kind = limit.kind.trim();
        if kind.is_empty() || !limit.percent_used.is_finite() {
            return None;
        }
        let resets_at = limit.resets_at.as_deref().and_then(parse_resets_at);
        let key = (login.key.clone(), kind.to_string());
        if let Some(cur) = self.last.get(&key) {
            let same =
                cur.resets_at == resets_at && (cur.percent - limit.percent_used).abs() < 1e-6;
            if same || limit.observed_at < cur.observed_at {
                return None;
            }
        }
        self.last.insert(
            key,
            Seen {
                resets_at,
                percent: limit.percent_used,
                observed_at: limit.observed_at,
            },
        );
        Some(LimitObservation {
            login: login.key.clone(),
            key_from: login.from,
            kind: kind.to_string(),
            percent: limit.percent_used,
            resets_at,
            observed_at: limit.observed_at,
            recorded_at,
            source: SOURCE_MOD.to_string(),
        })
    }
}

// --- 要約（`tako usage` / MCP `tako_sessions` の `action: "usage"`。S1 = 観測の要約だけ）-----------

/// 枠ごとの最新の観測
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowLatest {
    pub kind: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    pub observed_at: u64,
    /// 解除時刻を過ぎている（この % はもう今の窓の値ではない）
    pub expired: bool,
    /// この枠の観測の点数
    pub observations: usize,
}

/// ログインごとの要約
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoginSummary {
    /// 短い名前（初めて見た順に A, B, …）。画面ではこちらを出す（鍵そのものは突き合わせ用）
    pub label: String,
    pub login: String,
    pub key_from: KeyFrom,
    /// 枠ごとの最新の観測（`five_hour` → `seven_day` → その他の名前順）
    pub windows: Vec<WindowLatest>,
    pub observations: usize,
    /// このログインへ対応づけた会話の数
    pub sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_observed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<u64>,
}

/// 観測の要約
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub logins: Vec<LoginSummary>,
    pub observations: usize,
    pub sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_observed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<u64>,
}

/// 0 → A, 25 → Z, 26 → AA（表計算の列名と同じ）
pub fn login_label(index: usize) -> String {
    let mut n = index + 1;
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push(b'A' + rem as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn kind_order(kind: &str) -> (u8, &str) {
    match kind {
        "five_hour" => (0, kind),
        "seven_day" => (1, kind),
        other => (2, other),
    }
}

/// 観測と会話の対応から要約を組む。`now_secs` は解除時刻を過ぎたかの判定に使う
pub fn summarize(
    observations: &[LimitObservation],
    sessions: &[SessionLogin],
    now_secs: i64,
) -> Summary {
    // 会話 → ログインは後ろの行が正
    let mut session_login: BTreeMap<&str, &SessionLogin> = BTreeMap::new();
    for row in sessions {
        session_login.insert(row.session_id.as_str(), row);
    }
    // ログインの並び = 初めて現れた時刻の順（観測は observed_at、会話は seen_at）
    let mut first_seen: BTreeMap<&str, (u64, KeyFrom)> = BTreeMap::new();
    for row in observations {
        let entry = first_seen
            .entry(row.login.as_str())
            .or_insert((row.observed_at, row.key_from));
        if row.observed_at < entry.0 {
            entry.0 = row.observed_at;
        }
    }
    for row in session_login.values() {
        let entry = first_seen
            .entry(row.login.as_str())
            .or_insert((row.seen_at, row.key_from));
        if row.seen_at < entry.0 {
            entry.0 = row.seen_at;
        }
    }
    let mut order: Vec<(&str, u64, KeyFrom)> = first_seen
        .into_iter()
        .map(|(l, (at, from))| (l, at, from))
        .collect();
    order.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));

    let logins = order
        .iter()
        .enumerate()
        .map(|(i, (login, _, from))| {
            let mine: Vec<&LimitObservation> =
                observations.iter().filter(|o| o.login == *login).collect();
            let kinds: BTreeSet<&str> = mine.iter().map(|o| o.kind.as_str()).collect();
            let mut kinds: Vec<&str> = kinds.into_iter().collect();
            kinds.sort_by_key(|k| kind_order(k));
            let windows = kinds
                .into_iter()
                .filter_map(|kind| {
                    let of_kind: Vec<&&LimitObservation> =
                        mine.iter().filter(|o| o.kind == kind).collect();
                    let latest = of_kind.iter().max_by(|a, b| {
                        a.observed_at
                            .cmp(&b.observed_at)
                            .then(a.resets_at.cmp(&b.resets_at))
                    })?;
                    Some(WindowLatest {
                        kind: kind.to_string(),
                        used_percent: latest.percent,
                        resets_at: latest.resets_at,
                        observed_at: latest.observed_at,
                        expired: latest.resets_at.is_some_and(|r| r <= now_secs),
                        observations: of_kind.len(),
                    })
                })
                .collect();
            LoginSummary {
                label: login_label(i),
                login: (*login).to_string(),
                key_from: *from,
                windows,
                observations: mine.len(),
                sessions: session_login.values().filter(|s| s.login == *login).count(),
                first_observed_at: mine.iter().map(|o| o.observed_at).min(),
                last_observed_at: mine.iter().map(|o| o.observed_at).max(),
            }
        })
        .collect();
    Summary {
        logins,
        observations: observations.len(),
        sessions: session_login.len(),
        first_observed_at: observations.iter().map(|o| o.observed_at).min(),
        last_observed_at: observations.iter().map(|o| o.observed_at).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &str = "0123456789abcdef0123456789abcdef";
    const UUID_A: &str = "aaaaaaaa-1111-4222-8333-444444444444";
    const UUID_B: &str = "bbbbbbbb-5555-4666-8777-888888888888";

    fn limit(kind: &str, percent: f64, resets: &str, observed_at: u64) -> ModRateLimit {
        ModRateLimit {
            kind: kind.into(),
            percent_used: percent,
            resets_at: Some(resets.into()),
            observed_at,
        }
    }

    fn report(
        session: Option<&str>,
        config_dir: Option<&str>,
        limits: Vec<ModRateLimit>,
    ) -> ModReport {
        let mut v = serde_json::json!({
            "schema": 1, "mod_version": "t", "at": 1, "turn": "idle", "ended": false,
        });
        if let Some(s) = session {
            v["session_id"] = serde_json::json!(s);
        }
        if let Some(c) = config_dir {
            v["config_dir"] = serde_json::json!(c);
        }
        let mut r = crate::claude_mod::parse_report(v).unwrap();
        r.rate_limits = limits;
        r
    }

    const R1: &str = "2026-10-16T18:20:00.000Z";
    const R2: &str = "2026-10-16T23:20:00.000Z";

    #[test]
    fn 鍵は12桁の16進で生のuuidを含まない() {
        let k = account_key(SALT, UUID_A);
        assert_eq!(k.key.len(), KEY_HEX_LEN);
        assert!(k.key.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!UUID_A.contains(&k.key), "uuid の断片が鍵に出ている");
        assert_eq!(k.from, KeyFrom::Account);
        // 同じ uuid は同じ鍵・別の uuid は別の鍵・塩が違えば別の鍵
        assert_eq!(
            account_key(SALT, UUID_A),
            account_key(SALT, &format!(" {UUID_A}\n"))
        );
        assert_ne!(account_key(SALT, UUID_A).key, account_key(SALT, UUID_B).key);
        assert_ne!(
            account_key(SALT, UUID_A).key,
            account_key("other", UUID_A).key
        );
        // 設定 dir の鍵は同じ文字列でも accountUuid の鍵と突き合わない
        let d = config_dir_key(SALT, Path::new(UUID_A));
        assert_ne!(d.key, k.key);
        assert_eq!(d.from, KeyFrom::ConfigDir);
    }

    #[test]
    fn 資格情報のファイルは設定dirの有無で決まる() {
        let home = Path::new("/home/testuser");
        // CLAUDE_CONFIG_DIR 無し = ホーム直下の .claude.json
        assert_eq!(
            credentials_file(None, Some(home)),
            Some(home.join(".claude.json"))
        );
        assert_eq!(
            credentials_file(Some("  "), Some(home)),
            Some(home.join(".claude.json"))
        );
        // =~/.claude = 既定の設定 dir の中の .claude.json（無し と別のファイル）
        assert_eq!(
            credentials_file(Some("/home/testuser/.claude"), Some(home)),
            Some(home.join(".claude/.claude.json"))
        );
        // `~` の展開と末尾の区切り
        assert_eq!(
            credentials_file(Some("~/.claude/"), Some(home)),
            Some(home.join(".claude/.claude.json"))
        );
        assert_eq!(
            normalize_config_dir(Some("~"), Some(home)),
            Some(home.to_path_buf())
        );
        // 読めないときの設定 dir: 無しは <ホーム>/.claude（=~/.claude と同じ dir）
        assert_eq!(
            effective_config_dir(None, Some(home)),
            effective_config_dir(Some("~/.claude"), Some(home))
        );
        // ホームが無ければ CLAUDE_CONFIG_DIR 無しは決まらない
        assert_eq!(credentials_file(None, None), None);
        assert_eq!(
            credentials_file(Some("/cfg"), None),
            Some(PathBuf::from("/cfg/.claude.json"))
        );
    }

    #[test]
    fn claude_jsonからaccountuuidだけを読む() {
        let text = format!(
            r#"{{"numStartups": 3, "projects": {{"/x": {{"history": ["秘密"]}}}},
                "oauthAccount": {{"accountUuid": "{UUID_A}", "emailAddress": "someone@example.com"}}}}"#
        );
        assert_eq!(account_uuid_from_json(&text).as_deref(), Some(UUID_A));
        // 無い・空・壊れている
        assert_eq!(account_uuid_from_json(r#"{"numStartups": 1}"#), None);
        assert_eq!(
            account_uuid_from_json(r#"{"oauthAccount": {"accountUuid": "  "}}"#),
            None
        );
        assert_eq!(account_uuid_from_json(r#"{"oauthAccount": null}"#), None);
        assert_eq!(account_uuid_from_json("{ 壊れた"), None);
    }

    #[test]
    fn 同じ値のheartbeatは積まない() {
        let a = report(Some("s1"), None, vec![limit("five_hour", 12.0, R1, 100)]);
        // 初めての報告: 値も会話も新しい
        let first = pending_from_report(None, &a).expect("初回は積む");
        assert_eq!(first.limits.len(), 1);
        assert_eq!(first.session_id.as_deref(), Some("s1"));
        // 同じ値の heartbeat は積まない
        assert_eq!(pending_from_report(Some(&a), &a.clone()), None);
        // % が変わった枠だけ積む
        let mut b = a.clone();
        b.rate_limits = vec![
            limit("five_hour", 13.0, R1, 200),
            limit("seven_day", 40.0, R2, 50),
        ];
        let p = pending_from_report(Some(&a), &b).expect("変わった");
        assert_eq!(
            p.limits.iter().map(|l| l.kind.as_str()).collect::<Vec<_>>(),
            ["five_hour", "seven_day"]
        );
        // 解除時刻だけが変わった（新しい窓）も積む
        let mut c = b.clone();
        c.rate_limits = vec![
            limit("five_hour", 13.0, R2, 300),
            limit("seven_day", 40.0, R2, 50),
        ];
        let p = pending_from_report(Some(&b), &c).expect("解除時刻が変わった");
        assert_eq!(p.limits.len(), 1);
        assert_eq!(p.limits[0].kind, "five_hour");
        // ミリ秒の違いだけの解除時刻は同じ値（秒へ揃えて比べる）
        let mut d = c.clone();
        d.rate_limits = vec![
            limit("five_hour", 13.0, "2026-10-16T23:20:00.400Z", 300),
            limit("seven_day", 40.0, R2, 50),
        ];
        assert_eq!(pending_from_report(Some(&c), &d), None);
    }

    #[test]
    fn 会話が変われば値が同じでも積む_終わった報告は積まない() {
        let a = report(Some("s1"), None, vec![limit("five_hour", 12.0, R1, 100)]);
        let b = report(Some("s2"), None, vec![limit("five_hour", 12.0, R1, 100)]);
        let p = pending_from_report(Some(&a), &b).expect("会話が変わった");
        assert_eq!(p.session_id.as_deref(), Some("s2"));
        // 会話 ID が無い報告は値が同じなら積まない
        let none = report(None, None, vec![limit("five_hour", 12.0, R1, 100)]);
        assert_eq!(pending_from_report(Some(&none), &none.clone()), None);
        // 設定 dir が変わったら（別のアカウントで起動し直した）積む
        let other = report(
            Some("s1"),
            Some("/cfg"),
            vec![limit("five_hour", 12.0, R1, 100)],
        );
        assert!(pending_from_report(Some(&a), &other).is_some());
        let mut ended = a.clone();
        ended.ended = true;
        ended.rate_limits = vec![limit("five_hour", 50.0, R1, 999)];
        assert_eq!(pending_from_report(Some(&a), &ended), None);
        let mut dormant = a.clone();
        dormant.dormant = Some("user_disabled".into());
        assert_eq!(pending_from_report(None, &dormant), None);
    }

    #[test]
    fn ログイン単位で重複を除き放置ペインの古い値を書かない() {
        let login = account_key(SALT, UUID_A);
        let mut t = LimitTracker::default();
        // ペイン 1（動いている）: 20% を t=200 に観測
        assert!(t
            .admit(&login, &limit("seven_day", 20.0, R2, 200), 1)
            .is_some());
        // 同じログインのペイン 2 が同じ値を報告 → 書かない
        assert!(t
            .admit(&login, &limit("seven_day", 20.0, R2, 200), 2)
            .is_none());
        // ペイン 3（放置）: 古い 18%（t=100 の観測）→ 書かない（交互に届いても行は増えない）
        for _ in 0..3 {
            assert!(t
                .admit(&login, &limit("seven_day", 18.0, R2, 100), 3)
                .is_none());
            assert!(t
                .admit(&login, &limit("seven_day", 20.0, R2, 200), 3)
                .is_none());
        }
        // 新しい観測 → 書く
        let row = t
            .admit(&login, &limit("seven_day", 21.0, R2, 300), 4)
            .expect("値が変わった");
        assert_eq!(row.percent, 21.0);
        assert_eq!(row.resets_at, parse_resets_at(R2));
        assert_eq!(row.recorded_at, 4);
        assert_eq!(row.source, SOURCE_MOD);
        // 窓が変わる（新しい解除時刻・0%）→ 書く
        assert!(t
            .admit(&login, &limit("seven_day", 0.0, R1, 400), 5)
            .is_some());
        // 別のログインは別の列
        let other = account_key(SALT, UUID_B);
        assert!(t
            .admit(&other, &limit("seven_day", 21.0, R2, 300), 6)
            .is_some());
        // 枠も別の列
        assert!(t
            .admit(&login, &limit("five_hour", 21.0, R2, 300), 7)
            .is_some());
        // 壊れた値は書かない
        assert!(t.admit(&login, &limit("", 1.0, R2, 900), 8).is_none());
        assert!(t
            .admit(&login, &limit("five_hour", f64::NAN, R2, 900), 8)
            .is_none());
    }

    #[test]
    fn 再起動後はファイルの末尾で覚えを作る() {
        let login = account_key(SALT, UUID_A);
        let mut first = LimitTracker::default();
        let rows: Vec<LimitObservation> = [
            limit("seven_day", 20.0, R2, 200),
            limit("seven_day", 21.0, R2, 300),
        ]
        .iter()
        .filter_map(|l| first.admit(&login, l, 1))
        .collect();
        assert_eq!(rows.len(), 2);
        let mut t = LimitTracker::default();
        // 並びが逆でも observed_at の新しい方を覚える
        t.seed(rows.iter().rev());
        assert!(t
            .admit(&login, &limit("seven_day", 21.0, R2, 300), 9)
            .is_none());
        assert!(t
            .admit(&login, &limit("seven_day", 20.0, R2, 200), 9)
            .is_none());
        assert!(t
            .admit(&login, &limit("seven_day", 22.0, R2, 400), 9)
            .is_some());
    }

    #[test]
    fn 要約はログインごとの最新と点数() {
        let a = account_key(SALT, UUID_A);
        let b = config_dir_key(SALT, Path::new("/cfg"));
        let mut t = LimitTracker::default();
        let mut rows = Vec::new();
        for (login, l) in [
            (&b, limit("seven_day", 5.0, R2, 50)),
            (&a, limit("five_hour", 10.0, R1, 100)),
            (&a, limit("five_hour", 11.0, R1, 150)),
            (&a, limit("seven_day", 40.0, R2, 120)),
        ] {
            rows.extend(t.admit(login, &l, 1));
        }
        let sessions = vec![
            SessionLogin {
                session_id: "s1".into(),
                login: a.key.clone(),
                key_from: KeyFrom::Account,
                seen_at: 90,
            },
            SessionLogin {
                session_id: "s2".into(),
                login: a.key.clone(),
                key_from: KeyFrom::Account,
                seen_at: 95,
            },
            // 後ろの行が正: s2 は B へ移った
            SessionLogin {
                session_id: "s2".into(),
                login: b.key.clone(),
                key_from: KeyFrom::ConfigDir,
                seen_at: 99,
            },
        ];
        let r1 = parse_resets_at(R1).unwrap();
        let s = summarize(&rows, &sessions, r1 - 1);
        assert_eq!(s.observations, 4);
        assert_eq!(s.sessions, 2);
        assert_eq!(s.first_observed_at, Some(50));
        assert_eq!(s.last_observed_at, Some(150));
        // 初めて現れた順に A, B（B の観測 50 が最初）
        assert_eq!(s.logins[0].label, "A");
        assert_eq!(s.logins[0].login, b.key);
        assert_eq!(s.logins[0].key_from, KeyFrom::ConfigDir);
        assert_eq!(s.logins[0].sessions, 1);
        let la = &s.logins[1];
        assert_eq!(la.label, "B");
        assert_eq!(la.observations, 3);
        assert_eq!(la.sessions, 1);
        assert_eq!(
            la.windows
                .iter()
                .map(|w| (w.kind.as_str(), w.used_percent, w.observations))
                .collect::<Vec<_>>(),
            [("five_hour", 11.0, 2), ("seven_day", 40.0, 1)]
        );
        assert!(!la.windows[0].expired);
        // 解除時刻を過ぎたら expired
        let later = summarize(&rows, &sessions, r1);
        assert!(later.logins[1].windows[0].expired);
        // 何も無いとき
        let empty = summarize(&[], &[], 0);
        assert!(empty.logins.is_empty());
        assert_eq!(empty.first_observed_at, None);
    }

    #[test]
    fn ログインの短い名前() {
        assert_eq!(login_label(0), "A");
        assert_eq!(login_label(25), "Z");
        assert_eq!(login_label(26), "AA");
        assert_eq!(login_label(27), "AB");
    }

    #[test]
    fn 行は往復し生のuuidを持つ欄が無い() {
        let a = account_key(SALT, UUID_A);
        let row = LimitTracker::default()
            .admit(&a, &limit("five_hour", 3.0, R1, 7), 8)
            .unwrap();
        let line = serde_json::to_string(&row).unwrap();
        assert!(!line.contains(UUID_A));
        let back: LimitObservation = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
        assert!(line.contains("\"key_from\":\"account\""));
        let s = SessionLogin {
            session_id: "s".into(),
            login: a.key,
            key_from: KeyFrom::Account,
            seen_at: 1,
        };
        let back: SessionLogin = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}

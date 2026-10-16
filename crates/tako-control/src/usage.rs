//! usage 分析の置き場と記録（エピック #2192。S1 = #2200: 利用制限の観測を残す + ログインの鍵 /
//! S2 = #2201: 使用量の索引と内訳）。
//!
//! 判断と型は `tako_core::usage`（純関数）。ここはファイルの読み書きだけ:
//!
//! | 口 | 中身 | 呼び手 |
//! |---|---|---|
//! | [`drive_for_gui`] | S1: `ModHub` から汲み出した材料の鍵を引き、重複を除いて追記する（[`record_for_gui`]）。S2: 番が来ていれば使用量の索引を読み進める（起動から 60 秒後・以後 60 秒ごと） | GUI の定期処理（**背景**） |
//! | [`usage_payload`] | 観測の要約（ログインごとの最新の % と観測の点数）+ 期間の中の使用量の内訳（[`UsageQuery`]） | CLI `tako usage`（ローカル処理。答える前に索引を読み終える）/ MCP `tako_sessions` の `action: "usage"`（10 秒まで読み進める）/ dispatch |
//!
//! 置き場は `<data_dir>/usage/`（設定共有のカタログで `Local`。鍵の塩がマシン固有なので共有しない）:
//!
//! - `limits.jsonl`: 観測 1 点 = 1 行（[`LimitObservation`]）。値が変わったときだけ追記
//! - `sessions.jsonl`: 会話 → ログイン（[`SessionLogin`]）。後ろの行が正
//! - `cursor.jsonl` / `buckets/<YYYY-MM>.jsonl` / `keys.jsonl`: 使用量の索引（[`index`]）
//!
//! **起動時は何も読まない**: 覚え（最後に書いた値・対応を書いた会話）は最初の追記の直前に、
//! ファイルの末尾から作る。**診断ログ（persist.log）には件数だけを出す**（cwd・label・鍵・設定 dir・
//! 資格情報の値を出さない = AGENTS.md の絶対ルールの延長。設計書 §6）。

use std::collections::{BTreeMap, HashMap};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tako_core::usage::{
    self as core, LimitObservation, LimitTracker, LoginKey, PendingUsage, SessionLogin,
};

pub mod index;

use index::{Indexer, UpdateOptions, UpdateOutcome};
use tako_core::usage::report::{self, By, Window};
use tako_core::usage::KeyFrom;

/// 観測の履歴
pub const LIMITS_FILE: &str = "limits.jsonl";
/// 会話 → ログイン
pub const SESSIONS_FILE: &str = "sessions.jsonl";
/// 覚えを作るときに読むファイルの末尾の大きさ（ログイン × 枠の最後の行・最近の会話が入れば足りる）
const SEED_TAIL_BYTES: u64 = 256 * 1024;

/// `<data_dir>/usage/`
pub fn dir() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("usage"))
}

/// 鍵の塩（`<data_dir>/machine_id`。設定共有では `Secret` = 共有しない）
fn salt_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("machine_id"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    (now_ms() / 1000) as i64
}

/// 書けた行の数
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub limits: usize,
    pub sessions: usize,
}

/// 資格情報のファイルの覚え（更新時刻と大きさが同じ間は読み直さない）
struct CachedKey {
    stamp: Option<(SystemTime, u64)>,
    key: LoginKey,
}

/// 資格情報のファイルからログインの鍵を引く（観測の記録器と使用量の索引が共有する 1 実装）
pub(crate) struct LoginResolver {
    /// `CLAUDE_CONFIG_DIR` 無しの claude が読む資格情報の置き場（`<home>/.claude.json`）
    home: Option<PathBuf>,
    keys: HashMap<PathBuf, CachedKey>,
}

impl LoginResolver {
    pub(crate) fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            keys: HashMap::new(),
        }
    }

    /// `config_dir`（`CLAUDE_CONFIG_DIR` の値。既定の設定 dir なら `None`）からログインの鍵を引く
    /// （§3.2）。資格情報のファイルが無い・読めない・accountUuid が無いときは設定 dir の鍵へ落とす
    pub(crate) fn login_for(&mut self, config_dir: Option<&str>, salt: &str) -> LoginKey {
        let home = self.home.as_deref();
        let fallback = || {
            let dir = core::effective_config_dir(config_dir, home).unwrap_or_default();
            core::config_dir_key(salt, &dir)
        };
        let Some(cred) = core::credentials_file(config_dir, home) else {
            return fallback();
        };
        let stamp = std::fs::metadata(&cred)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len()));
        if let Some(cached) = self.keys.get(&cred) {
            if stamp.is_some() && cached.stamp == stamp {
                return cached.key.clone();
            }
        }
        let key = stamp
            .and_then(|_| std::fs::read_to_string(&cred).ok())
            .and_then(|text| core::account_uuid_from_json(&text))
            .map(|uuid| core::account_key(salt, &uuid))
            .unwrap_or_else(fallback);
        self.keys.insert(
            cred,
            CachedKey {
                stamp,
                key: key.clone(),
            },
        );
        key
    }
}

/// 観測の記録器（プロセスに 1 つ。GUI は [`record_for_gui`] の中の 1 つを使う）
pub struct Recorder {
    dir: PathBuf,
    salt_file: PathBuf,
    logins: LoginResolver,
    salt: Option<String>,
    /// 最初の追記の直前に `limits.jsonl` の末尾から作る（起動時には読まない）
    tracker: Option<LimitTracker>,
    /// 会話 → 書いたログインの鍵（最初の追記の直前に `sessions.jsonl` の末尾から作る）
    sessions: Option<HashMap<String, String>>,
    logged_start: bool,
    logged_error: bool,
}

impl Recorder {
    /// `home` は `CLAUDE_CONFIG_DIR` 無しの claude が読む資格情報の置き場（`<home>/.claude.json`）
    pub fn new(dir: PathBuf, salt_file: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            dir,
            salt_file,
            logins: LoginResolver::new(home),
            salt: None,
            tracker: None,
            sessions: None,
            logged_start: false,
            logged_error: false,
        }
    }

    /// 覚えを作り終えたか（テスト用: 起動直後には読まないことの確認）
    pub fn seeded(&self) -> bool {
        self.tracker.is_some() || self.sessions.is_some()
    }

    /// 材料の鍵を引き、重複を除いて追記する
    pub fn record(&mut self, batch: &[PendingUsage], now_ms: u64) -> Result<Outcome, String> {
        if batch.is_empty() {
            return Ok(Outcome::default());
        }
        let salt = self.salt()?;
        let limits_path = self.dir.join(LIMITS_FILE);
        let sessions_path = self.dir.join(SESSIONS_FILE);
        if self.tracker.is_none() {
            let mut tracker = LimitTracker::default();
            tracker.seed(read_tail::<LimitObservation>(&limits_path).iter());
            self.tracker = Some(tracker);
        }
        if self.sessions.is_none() {
            self.sessions = Some(
                read_tail::<SessionLogin>(&sessions_path)
                    .into_iter()
                    .map(|row| (row.session_id, row.login))
                    .collect(),
            );
        }
        let mut limit_rows = Vec::new();
        let mut session_rows = Vec::new();
        for item in batch {
            let login = self.logins.login_for(item.config_dir.as_deref(), &salt);
            if let Some(sid) = item.session_id.as_deref().filter(|s| !s.trim().is_empty()) {
                let sessions = self.sessions.get_or_insert_with(HashMap::new);
                if sessions.get(sid) != Some(&login.key) {
                    sessions.insert(sid.to_string(), login.key.clone());
                    session_rows.push(SessionLogin {
                        session_id: sid.to_string(),
                        login: login.key.clone(),
                        key_from: login.from,
                        seen_at: now_ms,
                    });
                }
            }
            let tracker = self.tracker.get_or_insert_with(LimitTracker::default);
            for limit in &item.limits {
                limit_rows.extend(tracker.admit(&login, limit, now_ms));
            }
        }
        let written = append_rows(&self.dir, &limits_path, &limit_rows)
            .and_then(|()| append_rows(&self.dir, &sessions_path, &session_rows));
        if let Err(e) = written {
            // 覚えだけ進んで行が無い状態にしない（次の追記の前に末尾から作り直す）
            self.tracker = None;
            self.sessions = None;
            return Err(e);
        }
        Ok(Outcome {
            limits: limit_rows.len(),
            sessions: session_rows.len(),
        })
    }

    /// 塩を読む。無ければ作る（OS の乱数。0600）
    fn salt(&mut self) -> Result<String, String> {
        if let Some(salt) = &self.salt {
            return Ok(salt.clone());
        }
        let salt = load_or_create_salt(&self.salt_file)?;
        self.salt = Some(salt.clone());
        Ok(salt)
    }
}

/// 塩を読む。無い・短すぎれば OS の乱数で作る（他のプロセスと競ったら先に書いた方を読む）
pub(crate) fn load_or_create_salt(path: &Path) -> Result<String, String> {
    let read = || {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() >= 16)
    };
    if let Some(salt) = read() {
        return Ok(salt);
    }
    let fresh = crate::generate_token().map_err(|e| format!("塩を作れない: {}", e.kind()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("塩の置き場を作れない: {}", e.kind()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(fresh.as_bytes())
                .map_err(|e| format!("塩を書けない: {}", e.kind()))?;
            Ok(fresh)
        }
        // 先に書いた誰か（もう 1 本の背景・空のファイル）が居る: 読めればそれを使う
        Err(_) => read().ok_or_else(|| "塩を読めない".to_string()),
    }
}

/// 行をまとめて 1 回の書き込みで追記する（置き場が無ければ作る）
fn append_rows<T: Serialize>(dir: &Path, path: &Path, rows: &[T]) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for row in rows {
        let line = serde_json::to_string(row).map_err(|e| format!("行を組めない: {e}"))?;
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("置き場を作れない: {}", e.kind()))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("開けない: {}", e.kind()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("書けない: {}", e.kind()))
}

/// 行を読む（読めない行は数えて飛ばす = 書きかけの最終行・手で壊した行）
fn parse_rows<T: DeserializeOwned>(text: &str) -> (Vec<T>, usize) {
    let mut rows = Vec::new();
    let mut bad = 0usize;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str(line) {
            Ok(row) => rows.push(row),
            Err(_) => bad += 1,
        }
    }
    (rows, bad)
}

/// ファイルの末尾 [`SEED_TAIL_BYTES`] の行（途中から読んだときの最初の欠けた行は捨てる）
fn read_tail<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(SEED_TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &text
    };
    parse_rows(text).0
}

/// ファイルの全行（無ければ空）
fn read_all<T: DeserializeOwned>(path: &Path) -> (Vec<T>, usize) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_rows(&text),
        Err(_) => (Vec::new(), 0),
    }
}

static GUI_RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

/// GUI の定期処理が `ModHub` から汲み出した材料を書く（**背景のスレッドで呼ぶ**。UI スレッドで
/// ファイルを読み書きしない）。A/B（`TAKO_2192_S1_LEGACY=1`）では何も書かない（置き場も作らない）
pub fn record_for_gui(batch: Vec<PendingUsage>) {
    if batch.is_empty() || core::s1_legacy() {
        return;
    }
    let (Some(dir), Some(salt_file)) = (dir(), salt_path()) else {
        return;
    };
    let mut guard = GUI_RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    let recorder = guard.get_or_insert_with(|| {
        Recorder::new(dir, salt_file, crate::orchestrator::agent_config_home())
    });
    match recorder.record(&batch, now_ms()) {
        Ok(out) => {
            if !recorder.logged_start && out.limits + out.sessions > 0 {
                recorder.logged_start = true;
                crate::diag::persist_log(&format!(
                    "usage: 利用制限の観測の記録を始めた（観測 {} 行・会話の対応 {} 行。以後は失敗だけを残す）",
                    out.limits, out.sessions
                ));
            }
        }
        Err(e) => {
            if !recorder.logged_error {
                recorder.logged_error = true;
                crate::diag::persist_log(&format!("usage: 利用制限の観測を書けない（{e}）"));
            }
        }
    }
}

/// 観測の要約（S1 の形。索引は読み進めない = テスト・直呼び）
pub fn summary_payload() -> Result<Value, String> {
    usage_payload(&UsageQuery::default(), Refresh::Skip)
}

/// [`summary_payload`] の本体（置き場と現在時刻を引数で受ける = テスト用）
pub fn summary_payload_in(dir: &Path, now_secs: i64) -> Value {
    payload_in(dir, &UsageQuery::default(), None, now_secs, 0)
}

// --- 使用量の内訳（S2 #2201）--------------------------------------------------------------------

/// `tako usage` / MCP `tako_sessions` の `action: "usage"` の引数（どれも省略できる = 最簡形は
/// `tako usage`。今の 7d の窓・プロジェクトの段・上位 10）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageQuery {
    /// 束ねる段（`project` / `issue` / `task-type` / `session` / `model` / `account` / `day` / `role` / `login`）
    pub by: Option<String>,
    /// 期間（`5h` / `7d` / `day` / `week` / `month` / `all`）
    pub window: Option<String>,
    /// tako のアカウント名で絞る
    pub account: Option<String>,
    /// プロジェクト（master はプロファイル名）で絞る
    pub project: Option<String>,
    /// 段の上位の件数
    pub limit: Option<usize>,
}

/// 段の上位の既定の件数
pub const DEFAULT_LIMIT: usize = 10;
/// MCP の問い合わせが答える前に索引を読み進める時間の上限（残りは GUI の背景が続ける）
pub const MCP_REFRESH_BUDGET: Duration = Duration::from_secs(10);

/// 答える前に索引をどこまで読み進めるか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// 読み終えるまで（CLI。GUI が動いていなくても同じ関数で追いつく）
    Full,
    /// 時間の上限つき（MCP / IPC）
    Budget(Duration),
    /// 読み進めない
    Skip,
}

impl UsageQuery {
    /// 段と期間を読む（知らない値は候補つきで断る）
    pub fn parsed(&self) -> Result<(By, Window), String> {
        let by = match self.by.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => By::Project,
            Some(s) => By::parse(s).ok_or_else(|| {
                format!(
                    "不明な by: {s:?}（{} のいずれか）",
                    By::ALL.map(By::as_str).join(" / ")
                )
            })?,
        };
        let window = match self
            .window
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => Window::SevenDay,
            Some(s) => Window::parse(s).ok_or_else(|| {
                format!(
                    "不明な window: {s:?}（{} のいずれか）",
                    Window::ALL.map(Window::as_str).join(" / ")
                )
            })?,
        };
        Ok((by, window))
    }
}

/// プロセスに 1 つの索引（GUI の背景・MCP の問い合わせ・CLI が使い回す。状態の読み直しを省く）
static PROCESS_INDEXER: Mutex<Option<Indexer>> = Mutex::new(None);

/// 索引を読み進める（`Refresh::Skip` / A/B なら何もしない）。失敗は応答の `index.error` に載せる
fn refresh_index(refresh: Refresh) -> Option<Result<UpdateOutcome, String>> {
    let opts = match refresh {
        Refresh::Skip => return None,
        Refresh::Full => UpdateOptions::default(),
        // MCP: CLI の長い取り込みを待たない（ロック中なら今ある索引で答える = `index.update.stopped: locked`）
        Refresh::Budget(budget) => UpdateOptions {
            budget: Some(budget),
            try_lock: true,
            ..UpdateOptions::default()
        },
    };
    if tako_core::usage::index::s2_legacy() {
        return None;
    }
    let mut guard = PROCESS_INDEXER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Indexer::for_process();
    }
    let indexer = guard.as_mut()?;
    Some(indexer.update(opts))
}

/// 使用量の内訳と観測の要約（`tako usage` / MCP `tako_sessions` の `action: "usage"` / dispatch の 1 実装）
pub fn usage_payload(query: &UsageQuery, refresh: Refresh) -> Result<Value, String> {
    let dir = dir().ok_or_else(|| "データディレクトリが決まらない（HOME が無い）".to_string())?;
    // 重い読み進めの前に引数を確かめる
    query.parsed()?;
    let outcome = refresh_index(refresh);
    let mut out = payload_in(
        &dir,
        query,
        outcome.as_ref().and_then(|r| r.as_ref().ok()),
        now_secs(),
        tako_core::platform::clock::local_utc_offset(),
    );
    if let Some(Err(e)) = outcome {
        out["index"]["error"] = json!(e);
    }
    Ok(out)
}

/// 小数第 6 位（マイクロドル）で丸める（応答の桁を揃える）
fn round_usd(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn round_usd_in(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, item) in map.iter_mut() {
                if k == "api_usd" {
                    if let Some(f) = item.as_f64() {
                        *item = json!(round_usd(f));
                    }
                } else {
                    round_usd_in(item);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(round_usd_in),
        _ => {}
    }
}

/// [`usage_payload`] の本体（置き場・読み進めた結果・現在時刻・ローカル時刻のずれを引数で受ける = テスト用）
pub fn payload_in(
    dir: &Path,
    query: &UsageQuery,
    outcome: Option<&UpdateOutcome>,
    now_secs: i64,
    utc_offset: i32,
) -> Value {
    let (observations, bad_limits) = read_all::<LimitObservation>(&dir.join(LIMITS_FILE));
    let (sessions, bad_sessions) = read_all::<SessionLogin>(&dir.join(SESSIONS_FILE));
    let s2_legacy = tako_core::usage::index::s2_legacy();
    let (by, window) = query.parsed().unwrap_or((By::Project, Window::SevenDay));
    let mut extra = Vec::new();
    let mut breakdown = None;
    let mut bad_index = 0usize;
    let mut keyed_sessions = 0usize;
    if !s2_legacy {
        let (keys, bad_keys) = index::read_keys(dir);
        let mut mods: BTreeMap<String, (String, KeyFrom)> = BTreeMap::new();
        for row in &sessions {
            mods.insert(row.session_id.clone(), (row.login.clone(), row.key_from));
        }
        let from = report::earliest_from(window, now_secs, utc_offset);
        let (rows, bad_rows) = index::read_rows(dir, from);
        bad_index = bad_keys + bad_rows;
        keyed_sessions = keys.len();
        extra = report::login_first_seen(&keys, &mods);
        breakdown = Some(report::breakdown(
            &rows,
            &keys,
            &mods,
            &observations,
            &report::Query {
                by,
                window,
                account: query.account.as_deref(),
                project: query.project.as_deref(),
                limit: query.limit.unwrap_or(DEFAULT_LIMIT),
            },
            now_secs,
            utc_offset,
        ));
    }
    let summary = core::summarize_with(&observations, &sessions, &extra, now_secs);
    let mut out = serde_json::to_value(&summary).unwrap_or_else(|_| json!({}));
    let iso = |v: &mut Value, key: &str| {
        if let Some(secs) = v[key].as_i64() {
            v[format!("{key}_iso")] = json!(crate::diag::format_utc(secs));
        }
    };
    if let Some(logins) = out["logins"].as_array_mut() {
        for login in logins {
            // 解除時刻は unix 秒と ISO の両方（`rate_limits` の応答 = #1880 と同じ並び）
            if let Some(windows) = login["windows"].as_array_mut() {
                for w in windows {
                    iso(w, "resets_at");
                }
            }
            let Some(b) = &breakdown else { continue };
            let key = login["login"].as_str().unwrap_or_default().to_string();
            if let Some(u) = b.logins.iter().find(|u| u.login == key) {
                let mut usage = json!({
                    "range": u.range,
                    "total": u.total,
                    "groups": u.groups,
                });
                if u.more_groups > 0 {
                    usage["more_groups"] = json!(u.more_groups);
                }
                iso(&mut usage["range"], "from");
                iso(&mut usage["range"], "to");
                iso(&mut usage["range"], "resets_at");
                login["usage"] = usage;
            }
        }
    }
    let legacy = core::s1_legacy();
    out["recording"] = json!(!legacy);
    if legacy {
        out["legacy_env"] = json!(core::S1_LEGACY_ENV);
    }
    out["source"] = json!(core::SOURCE_MOD);
    if bad_limits + bad_sessions + bad_index > 0 {
        out["unreadable_lines"] = json!(bad_limits + bad_sessions + bad_index);
    }
    let mut notes: Vec<String> = Vec::new();
    match &breakdown {
        Some(b) => {
            out["by"] = json!(b.by);
            out["window"] = json!(b.window);
            out["price_table_as_of"] = json!(b.price_table_as_of);
            out["unpriced_models"] = json!(b.unpriced_models);
            if b.unkeyed_tokens > 0 {
                out["unkeyed_tokens"] = json!(b.unkeyed_tokens);
            }
            notes.push(format!(
                "api_usd は API 換算（モデル別の単価 × 5 成分。単価表 {}）で、サブスクリプションの請求額ではない",
                b.price_table_as_of
            ));
            if !b.unpriced_models.is_empty() {
                notes.push(format!(
                    "単価表に無いモデル（{}）は API 換算に含めず unpriced_tokens に数えた",
                    b.unpriced_models.join(" / ")
                ));
            }
            out["scope"] = json!(
                "ログインごとの観測の要約（最新の % と点数）と、期間の中の使用量の内訳（トークン 5 成分・\
                 API 換算・応答数・会話数）。利用制限の % をプロジェクト・Issue へ配るのは後続のスライス\
                 （#2202 / エピック #2192）"
            );
            let mut index = json!({
                "enabled": true,
                "sessions": keyed_sessions,
            });
            if let Some(o) = outcome {
                index["update"] = json!(o);
                if o.stopped == Some("locked") {
                    notes.push(
                        "ほかの更新（CLI / GUI の背景）が索引を読み進めている最中なので、今ある索引で答えた"
                            .to_string(),
                    );
                } else if !o.caught_up {
                    notes.push(format!(
                        "索引はまだ読み途中（残り {} 本・{} MB）。GUI が背景で続ける（CLI は tako usage で追いつく）",
                        o.pending_files,
                        o.pending_bytes / (1024 * 1024)
                    ));
                }
            }
            out["index"] = index;
        }
        None => {
            out["index"] = json!({
                "enabled": false,
                "legacy_env": tako_core::usage::index::S2_LEGACY_ENV,
            });
            out["scope"] = json!(
                "観測の要約（ログインごとの最新の % と観測の点数）。使用量の索引は A/B で切っている"
            );
        }
    }
    if summary.observations == 0 {
        out["hint"] = json!(
            "まだ観測が無い。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる\
             （mod の状態は tako mod）"
        );
    }
    out["notes"] = json!(notes);
    round_usd_in(&mut out);
    out
}

// --- GUI の背景（S1 の記録 + S2 の索引）-----------------------------------------------------------

/// GUI の最初の索引の更新（起動から。起動の段では読まない = §5.3）
pub const GUI_FIRST_DELAY: Duration = Duration::from_secs(60);
/// 追いついた後の更新の間隔
pub const GUI_INTERVAL: Duration = Duration::from_secs(60);
/// 1 回の更新の時間の上限（初回の取り込みは区切って進む）
pub const GUI_RUN_BUDGET: Duration = Duration::from_secs(5);
/// 読み途中のときの次の更新までの間
pub const GUI_CATCH_UP_GAP: Duration = Duration::from_secs(2);

/// GUI の背景の更新の番と、診断ログに 1 回だけ出す印
struct GuiTick {
    first: Option<Instant>,
    next_due: Option<Instant>,
    /// 追いつくまでに読んだ量（最初に追いついたときに 1 行出す）
    catch_up: UpdateOutcome,
    runs: u32,
    logged_catch_up: bool,
    logged_error: bool,
    logged_pressure: bool,
}

static GUI_TICK: Mutex<GuiTick> = Mutex::new(GuiTick {
    first: None,
    next_due: None,
    catch_up: UpdateOutcome {
        files: 0,
        read_files: 0,
        bytes_read: 0,
        responses: 0,
        keyed_sessions: 0,
        caught_up: false,
        pending_files: 0,
        pending_bytes: 0,
        stopped: None,
        elapsed_ms: 0,
    },
    runs: 0,
    logged_catch_up: false,
    logged_error: false,
    logged_pressure: false,
});

/// GUI の定期処理（2 秒ごと）が背景で呼ぶ: S1 の観測の記録と、番が来ていれば S2 の索引の更新
pub fn drive_for_gui(batch: Vec<PendingUsage>) {
    record_for_gui(batch);
    index_tick_for_gui();
}

fn index_tick_for_gui() {
    if tako_core::usage::index::s2_legacy() {
        return;
    }
    // 前の回がまだ走っていれば何もしない
    let Ok(mut tick) = GUI_TICK.try_lock() else {
        return;
    };
    let now = Instant::now();
    let first = *tick.first.get_or_insert(now);
    if now < tick.next_due.unwrap_or(first + GUI_FIRST_DELAY) {
        return;
    }
    // MCP の問い合わせが読み進めている間は譲る
    let Ok(mut guard) = PROCESS_INDEXER.try_lock() else {
        tick.next_due = Some(now + GUI_CATCH_UP_GAP);
        return;
    };
    if guard.is_none() {
        *guard = Indexer::for_process();
    }
    let Some(indexer) = guard.as_mut() else {
        tick.next_due = Some(now + GUI_INTERVAL);
        return;
    };
    let result = indexer.update(UpdateOptions {
        budget: Some(GUI_RUN_BUDGET),
        stop_on_pressure: true,
        try_lock: true,
    });
    drop(guard);
    match result {
        Ok(out) => {
            tick.next_due = Some(
                now + match out.stopped {
                    Some("budget") | Some("locked") => GUI_CATCH_UP_GAP,
                    _ => GUI_INTERVAL,
                },
            );
            if out.stopped == Some("memory_pressure") && !tick.logged_pressure {
                tick.logged_pressure = true;
                crate::diag::persist_log(
                    "usage: メモリ圧が高いので使用量の索引の読み進めを止めた（60 秒後にもう一度）",
                );
            }
            if out.stopped == Some("locked") {
                return;
            }
            tick.runs += 1;
            let acc = &mut tick.catch_up;
            acc.read_files += out.read_files;
            acc.bytes_read += out.bytes_read;
            acc.responses += out.responses;
            acc.elapsed_ms += out.elapsed_ms;
            if out.caught_up && !tick.logged_catch_up {
                tick.logged_catch_up = true;
                // 件数・バイト数・所要だけ（cwd・label・Issue・トークンの内訳を出さない = §6）
                crate::diag::persist_log(&format!(
                    "usage: 使用量の索引が追いついた（transcript {} 本・読んだ {} 本 {:.1} MB・応答 {} 件・所要 {:.1} 秒・{} 回に分けた）",
                    out.files,
                    tick.catch_up.read_files,
                    tick.catch_up.bytes_read as f64 / (1024.0 * 1024.0),
                    tick.catch_up.responses,
                    tick.catch_up.elapsed_ms as f64 / 1000.0,
                    tick.runs
                ));
            }
        }
        Err(e) => {
            tick.next_due = Some(now + GUI_INTERVAL);
            if !tick.logged_error {
                tick.logged_error = true;
                crate::diag::persist_log(&format!("usage: 使用量の索引を更新できない（{e}）"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::claude_mod::ModRateLimit;
    use tako_core::usage::KeyFrom;

    const UUID_A: &str = "aaaaaaaa-1111-4222-8333-444444444444";
    const UUID_B: &str = "bbbbbbbb-5555-4666-8777-888888888888";
    const R1: &str = "2026-10-16T18:20:00.000Z";

    /// 置き場・ホーム・設定 dir をすべて使い捨ての dir の中に作る（本物の `~/.claude*` に触れない）
    struct Env {
        _scratch: tako_core::test_residue::ScratchDir,
        root: PathBuf,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let scratch = tako_core::test_residue::ScratchDir::new(&format!("usage-{name}"));
            let root = scratch.path().to_path_buf();
            std::fs::create_dir_all(root.join("home")).unwrap();
            Self {
                _scratch: scratch,
                root,
            }
        }
        fn home(&self) -> PathBuf {
            self.root.join("home")
        }
        fn usage(&self) -> PathBuf {
            self.root.join("data/usage")
        }
        fn recorder(&self) -> Recorder {
            Recorder::new(
                self.usage(),
                self.root.join("data/machine_id"),
                Some(self.home()),
            )
        }
        /// 偽の `.claude.json`（accountUuid と、本文に当たるキー・メールも入れる）
        fn claude_json(&self, dir: &Path, uuid: &str) {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join(".claude.json"),
                format!(
                    r#"{{"projects": {{"/secret-cwd": {{"lastPrompt": "label-marker"}}}},
                        "oauthAccount": {{"accountUuid": "{uuid}", "emailAddress": "someone@example.com"}}}}"#
                ),
            )
            .unwrap();
        }
        fn limits(&self) -> Vec<LimitObservation> {
            read_all(&self.usage().join(LIMITS_FILE)).0
        }
        fn sessions(&self) -> Vec<SessionLogin> {
            read_all(&self.usage().join(SESSIONS_FILE)).0
        }
    }

    fn pending(session: &str, config_dir: Option<&Path>, pct: f64, observed: u64) -> PendingUsage {
        PendingUsage {
            session_id: Some(session.into()),
            config_dir: config_dir.map(|p| p.display().to_string()),
            limits: vec![ModRateLimit {
                kind: "five_hour".into(),
                percent_used: pct,
                resets_at: Some(R1.into()),
                observed_at: observed,
            }],
        }
    }

    #[test]
    fn 同じログインの2つの設定dirは同じ鍵_別のログインは別の鍵() {
        let env = Env::new("same-login");
        let cfg1 = env.root.join("cfg1");
        let cfg2 = env.root.join("cfg2");
        let cfg3 = env.root.join("cfg3");
        env.claude_json(&cfg1, UUID_A);
        env.claude_json(&cfg2, UUID_A);
        env.claude_json(&cfg3, UUID_B);
        let mut rec = env.recorder();
        let out = rec
            .record(
                &[
                    pending("s1", Some(&cfg1), 10.0, 100),
                    pending("s2", Some(&cfg2), 11.0, 200),
                    pending("s3", Some(&cfg3), 50.0, 150),
                ],
                1,
            )
            .unwrap();
        assert_eq!(
            out,
            Outcome {
                limits: 3,
                sessions: 3
            }
        );
        let s = env.sessions();
        assert_eq!(s[0].login, s[1].login, "同じログインが別の鍵になった");
        assert_ne!(s[0].login, s[2].login, "別のログインが同じ鍵になった");
        assert!(s.iter().all(|r| r.key_from == KeyFrom::Account));
        // 同じログインの列の重複除去は設定 dir をまたいで効く（cfg1 の古い 10% はもう書かない）
        let out = rec
            .record(&[pending("s1", Some(&cfg1), 10.0, 100)], 2)
            .unwrap();
        assert_eq!(out, Outcome::default());
    }

    #[test]
    fn 既定の設定dirの2通りは資格情報のファイルどおりに分かれる() {
        let env = Env::new("default-dir");
        let home = env.home();
        // CLAUDE_CONFIG_DIR 無し = ~/.claude.json（ログイン A）
        env.claude_json(&home, UUID_A);
        // =~/.claude = ~/.claude/.claude.json（ログイン B）
        env.claude_json(&home.join(".claude"), UUID_B);
        let mut rec = env.recorder();
        rec.record(
            &[
                pending("no-env", None, 1.0, 1),
                pending("with-env", Some(&home.join(".claude")), 2.0, 2),
            ],
            1,
        )
        .unwrap();
        let s = env.sessions();
        assert_eq!(s.len(), 2);
        assert_ne!(s[0].login, s[1].login);
        // 同じログインなら同じ鍵（~/.claude/.claude.json も A にする）
        env.claude_json(&home.join(".claude"), UUID_A);
        // 更新時刻の粒度で覚えが残らないよう大きさも変える
        std::fs::OpenOptions::new()
            .append(true)
            .open(home.join(".claude/.claude.json"))
            .unwrap()
            .write_all(b" ")
            .unwrap();
        rec.record(
            &[pending("with-env-2", Some(&home.join(".claude")), 3.0, 3)],
            2,
        )
        .unwrap();
        let s = env.sessions();
        assert_eq!(s[2].login, s[0].login);
    }

    #[test]
    fn 資格情報が無い_読めないときは設定dirの鍵へ落ちる() {
        let env = Env::new("fallback");
        let missing = env.root.join("missing");
        let broken = env.root.join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(".claude.json"), "{ 壊れた").unwrap();
        let no_uuid = env.root.join("no-uuid");
        std::fs::create_dir_all(&no_uuid).unwrap();
        std::fs::write(no_uuid.join(".claude.json"), r#"{"numStartups": 1}"#).unwrap();
        let mut rec = env.recorder();
        rec.record(
            &[
                pending("m", Some(&missing), 1.0, 1),
                pending("b", Some(&broken), 1.0, 1),
                pending("n", Some(&no_uuid), 1.0, 1),
                // 末尾の区切りは同じ設定 dir
                PendingUsage {
                    config_dir: Some(format!("{}/", missing.display())),
                    ..pending("m2", None, 1.0, 1)
                },
            ],
            1,
        )
        .unwrap();
        let s = env.sessions();
        assert!(s.iter().all(|r| r.key_from == KeyFrom::ConfigDir), "{s:?}");
        assert_ne!(s[0].login, s[1].login);
        assert_ne!(s[1].login, s[2].login);
        assert_eq!(s[0].login, s[3].login);
        let salt = std::fs::read_to_string(env.root.join("data/machine_id")).unwrap();
        assert_eq!(s[0].login, core::config_dir_key(&salt, &missing).key);
    }

    #[test]
    fn 生のuuidとメールと本文がどのファイルにも出ない() {
        let env = Env::new("privacy");
        let cfg = env.root.join("cfg");
        env.claude_json(&cfg, UUID_A);
        let mut rec = env.recorder();
        rec.record(&[pending("s", Some(&cfg), 7.0, 1)], 1).unwrap();
        let mut seen = 0;
        for entry in std::fs::read_dir(env.usage()).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            seen += 1;
            for needle in [
                UUID_A,
                UUID_A.split('-').next().unwrap(),
                "someone@example.com",
                "label-marker",
                "/secret-cwd",
                cfg.to_str().unwrap(),
            ] {
                assert!(!text.contains(needle), "{needle} が usage/ に出た: {text}");
            }
        }
        assert_eq!(seen, 2, "limits.jsonl と sessions.jsonl の 2 つ");
        let payload = summary_payload_in(&env.usage(), 0).to_string();
        assert!(!payload.contains(UUID_A) && !payload.contains("someone@example.com"));
    }

    #[test]
    fn 作っただけでは読まない_最初の追記で末尾から覚える() {
        let env = Env::new("seed");
        let cfg = env.root.join("cfg");
        env.claude_json(&cfg, UUID_A);
        {
            let mut first = env.recorder();
            first
                .record(&[pending("s", Some(&cfg), 30.0, 10)], 1)
                .unwrap();
        }
        // 再起動（新しい記録器）: 作っただけでは何も読まない
        let mut rec = env.recorder();
        assert!(!rec.seeded());
        rec.record(&[], 2).unwrap();
        assert!(!rec.seeded(), "空の材料で読んだ");
        // 最初の追記で末尾から覚えを作る = 前回と同じ値・同じ会話は書かない
        let out = rec
            .record(&[pending("s", Some(&cfg), 30.0, 10)], 3)
            .unwrap();
        assert!(rec.seeded());
        assert_eq!(out, Outcome::default());
        assert_eq!(env.limits().len(), 1);
        assert_eq!(env.sessions().len(), 1);
        let out = rec
            .record(&[pending("s", Some(&cfg), 31.0, 20)], 4)
            .unwrap();
        assert_eq!(
            out,
            Outcome {
                limits: 1,
                sessions: 0
            }
        );
    }

    #[test]
    fn 末尾から読むとき途中の行と壊れた行を飛ばす() {
        let env = Env::new("tail");
        std::fs::create_dir_all(env.usage()).unwrap();
        let path = env.usage().join(LIMITS_FILE);
        let row = |pct: f64| {
            serde_json::to_string(&LimitObservation {
                login: "abc".into(),
                key_from: KeyFrom::Account,
                kind: "seven_day".into(),
                percent: pct,
                resets_at: Some(1),
                observed_at: pct as u64,
                recorded_at: 0,
                source: "mod".into(),
            })
            .unwrap()
        };
        // 末尾の窓より長い前置き + 壊れた行 + 書きかけの最終行
        let mut text = String::new();
        while (text.len() as u64) < SEED_TAIL_BYTES + 100 {
            text.push_str(&row(1.0));
            text.push('\n');
        }
        text.push_str("{ 壊れた\n");
        text.push_str(&row(42.0));
        text.push('\n');
        text.push_str(r#"{"login": "abc", "kind"#);
        std::fs::write(&path, &text).unwrap();
        let rows: Vec<LimitObservation> = read_tail(&path);
        assert_eq!(rows.last().unwrap().percent, 42.0);
        assert!(rows.len() > 100);
        let (all, bad) = read_all::<LimitObservation>(&path);
        assert_eq!(bad, 2);
        assert_eq!(all.last().unwrap().percent, 42.0);
        let payload = summary_payload_in(&env.usage(), 0);
        assert_eq!(payload["unreadable_lines"], 2);
        assert_eq!(payload["logins"][0]["windows"][0]["used_percent"], 42.0);
        assert_eq!(
            payload["logins"][0]["windows"][0]["resets_at_iso"],
            "1970-01-01T00:00:01Z"
        );
    }

    #[test]
    fn 塩は一度作れば同じものを使い0600() {
        let env = Env::new("salt");
        let path = env.root.join("data/machine_id");
        let a = load_or_create_salt(&path).unwrap();
        assert!(a.len() >= 32);
        assert_eq!(load_or_create_salt(&path).unwrap(), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // 既存の値（以前のリモート中継が書いた uuid 形）はそのまま使う
        std::fs::write(&path, "11111111-2222-3333-4444-555555555555\n").unwrap();
        assert_eq!(
            load_or_create_salt(&path).unwrap(),
            "11111111-2222-3333-4444-555555555555"
        );
    }

    /// #2201: 観測（S1）と索引（S2）から、ログインごとの窓・段の内訳・単価不明の注記を返す
    #[test]
    fn 内訳はログインの実窓で区切り段で束ね単価不明を注記する() {
        use tako_core::usage::index::{BucketRow, ProjectFrom, SessionKeys, Tokens};
        let env = Env::new("payload");
        let dir = env.usage();
        let now: i64 = 1_792_000_000;
        let login = "abcdef012345";
        // 7d の窓は 2 日前に始まった（解除まで 5 日）
        let obs = LimitObservation {
            login: login.into(),
            key_from: KeyFrom::Account,
            kind: "seven_day".into(),
            percent: 40.0,
            resets_at: Some(now + 5 * 86_400),
            observed_at: (now as u64 - 60) * 1000,
            recorded_at: 0,
            source: "mod".into(),
        };
        append_rows(&dir, &dir.join(LIMITS_FILE), &[obs]).unwrap();
        let keys = |sid: &str, project: &str, label: Option<&str>| SessionKeys {
            session_id: sid.into(),
            login: login.into(),
            key_from: KeyFrom::Account,
            agent: "claude".into(),
            account: Some("univ".into()),
            role: Some("worker".into()),
            project: Some(project.into()),
            project_from: Some(ProjectFrom::Catalog),
            issue: label.and_then(tako_core::usage::index::first_issue),
            task_type: None,
            ledger_id: None,
            label: label.map(str::to_string),
            first_at: now - 86_400,
            recorded_at: 0,
        };
        append_rows(
            &dir,
            &dir.join(index::KEYS_FILE),
            &[
                keys("s1", "tako", Some("#2201 S2")),
                keys("s2", "tako", None),
                keys("s3", "other", Some("#7 x")),
            ],
        )
        .unwrap();
        let row = |t: i64, sid: &str, model: &str, output: u64| BucketRow {
            t,
            session: sid.into(),
            model: model.into(),
            fast: false,
            tokens: Tokens {
                output,
                ..Tokens::default()
            },
            responses: 1,
        };
        let month = |t: i64| {
            dir.join(index::BUCKETS_DIR)
                .join(format!("{}.jsonl", tako_core::usage::index::month_of(t)))
        };
        let rows = [
            row(now - 3600, "s1", "claude-opus-5-5", 1_000_000),
            row(now - 3600, "s2", "claude-opus-5-5", 500_000),
            row(now - 7200, "s3", "claude-sonnet-9-9", 10),
            // 窓の始まり（2 日前）より前 = 7d の窓には入らない
            row(now - 3 * 86_400, "s1", "claude-opus-5-5", 9_000_000),
        ];
        for r in &rows {
            append_rows(
                &dir.join(index::BUCKETS_DIR),
                &month(r.t),
                std::slice::from_ref(r),
            )
            .unwrap();
        }
        let q = UsageQuery {
            by: Some("issue".into()),
            ..UsageQuery::default()
        };
        let p = payload_in(&dir, &q, None, now, 0);
        assert_eq!(p["by"], "issue");
        assert_eq!(p["window"], "7d");
        assert_eq!(p["index"]["enabled"], true);
        let l = &p["logins"][0];
        assert_eq!(l["label"], "A");
        assert_eq!(l["usage"]["range"]["basis"], "observed");
        assert_eq!(l["usage"]["range"]["from"], now - 2 * 86_400);
        assert!(l["usage"]["range"]["from_iso"].is_string());
        assert_eq!(l["usage"]["total"]["responses"], 3, "窓の外の行が入った");
        let names: Vec<&str> = l["usage"]["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["tako #2201", "tako Issue なし", "other #7"]);
        // 出力 100 万 × $20 = $20（Opus 5.5）
        assert_eq!(l["usage"]["groups"][0]["api_usd"], 20.0);
        assert_eq!(l["usage"]["groups"][2]["unpriced_tokens"], 10);
        assert_eq!(p["unpriced_models"][0], "claude-sonnet-9-9");
        let notes = p["notes"].to_string();
        assert!(
            notes.contains("単価表に無いモデル（claude-sonnet-9-9）"),
            "{notes}"
        );
        assert!(notes.contains("API 換算"), "{notes}");
        // 絞り込み・段の上位
        let q = UsageQuery {
            by: Some("session".into()),
            window: Some("all".into()),
            project: Some("tako".into()),
            limit: Some(1),
            ..UsageQuery::default()
        };
        let p = payload_in(&dir, &q, None, now, 0);
        let u = &p["logins"][0]["usage"];
        assert_eq!(u["range"]["basis"], "all");
        assert_eq!(u["total"]["sessions"], 2);
        assert_eq!(u["groups"].as_array().unwrap().len(), 1);
        assert_eq!(u["more_groups"], 1);
        // 知らない段・期間は候補つきで断る
        let bad = UsageQuery {
            by: Some("branch".into()),
            ..UsageQuery::default()
        };
        let e = bad.parsed().unwrap_err();
        assert!(e.contains("project / issue"), "{e}");
        let bad = UsageQuery {
            window: Some("1y".into()),
            ..UsageQuery::default()
        };
        assert!(bad.parsed().unwrap_err().contains("5h / 7d"));
    }

    #[test]
    fn 観測が無いときの要約() {
        let env = Env::new("empty");
        let payload = summary_payload_in(&env.usage(), 0);
        assert_eq!(payload["observations"], 0);
        assert!(payload["hint"].is_string());
        assert_eq!(payload["recording"], !core::s1_legacy());
        assert!(!env.usage().exists(), "読むだけで置き場を作った");
    }
}

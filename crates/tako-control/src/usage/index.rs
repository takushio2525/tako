//! 使用量の索引の置き場と更新（usage 分析 S2 #2201。設計書 `.agent/plans/2026-10-usage-analytics.md`
//! §3.3 / §5 / §6）。判断と型は `tako_core::usage::index`（純関数）。
//!
//! # 置き場（`<data_dir>/usage/`。設定共有のカタログで `Local`）
//!
//! | ファイル | 中身 | 書き方 |
//! |---|---|---|
//! | `cursor.jsonl` | transcript ごとの読み位置（[`CursorRow`]。置き場はパスのハッシュで、パスそのものは書かない）と、新しく数えた応答の鍵 | 追記・後ろの行が正。行が増えたら生きているファイルの分だけへ書き直す |
//! | `buckets/<YYYY-MM>.jsonl` | 5 分刻み × 会話 × モデルの 5 成分と応答数（[`BucketRow`]） | 追記。育ったら同じ鍵を合わせて書き直す。35 日より古い月は日単位へ・365 日より古い月は消す |
//! | `keys.jsonl` | 会話 → 帰属の鍵（[`SessionKeys`]。取り込んだ時点で写す = §3.3） | 追記・後ろの行が正（良くなったときだけ足す） |
//! | `limits.jsonl` | codex の rollout の `rate_limits`（S5 #2204。`source: codex`。claude の観測は S1 の記録器が書く） | 追記（同じ枠・解除時刻・% はいちばん早い観測の 1 行だけ） |
//!
//! 書く順は「鍵 → 集約 → codex の観測 → 読み位置」。読み位置を最後に書くので、途中で落ちても行の無い読み位置は
//! 残らない（落ちた分は次の更新で読み直す。集約を書いた直後に落ちたときだけ、その回の分を二重に
//! 数えうる）。更新は `cursor.jsonl.lock` の排他 flock で 1 本にする（GUI と CLI が同時に書かない）。
//!
//! # 読むもの
//!
//! 設定 dir（[`roots`]）の `projects/` の下の `*.jsonl`（サブエージェントの `<sessionId>/subagents/` も）。
//! **assistant 行の使用量のメタデータだけ**を読み（本文は serde が読み飛ばす）、診断ログには
//! 件数・所要・バイト数だけを出す（cwd・label・Issue 番号・トークンの内訳を出さない = §6）。
//!
//! codex（S5 #2204）は `CODEX_HOME`（[`codex_homes`]）の `sessions/**/rollout-*.jsonl` と
//! `archived_sessions/` の `token_count`（累積の差分）と `rate_limits` を読む（`tako_core::usage::codex`）。
//! 読み位置の鍵はファイル名（`sessions/` から `archived_sessions/` へ移っても続きから読む）、
//! ログインは `<CODEX_HOME>/auth.json` の `tokens.account_id` の塩付きハッシュ。
//!
//! # 負荷（§5.3）
//!
//! **起動時は何も読まない**（GUI の最初の更新は起動から約 60 秒後）。更新は 1 回あたりの時間の上限と
//! メモリ圧で区切って進み、追いついたら 60 秒ごとに追記された分だけを読む。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tako_core::usage::codex::{
    self as core_codex, CodexCursor, CodexLimit, CodexProgress, CodexRead,
};
use tako_core::usage::index::{
    self as core_index, Batch, BucketRow, CursorRow, FileProgress, LastMessage, ProjectFrom,
    SessionKeys,
};
use tako_core::usage::{LimitObservation, LoginKey, SOURCE_CODEX};

use super::{append_rows, load_or_create_salt, read_all, LoginResolver, LIMITS_FILE};

/// 読み位置
pub const CURSOR_FILE: &str = "cursor.jsonl";
/// 会話 → 帰属の鍵
pub const KEYS_FILE: &str = "keys.jsonl";
/// 集約（月ごと）
pub const BUCKETS_DIR: &str = "buckets";
/// 月のファイルの書き直しの覚え
const COMPACTED_FILE: &str = "compacted.json";
/// 読む設定 dir を差し替える（検証用。OS のパス区切りで並べる）。無ければ [`roots`] の既定
pub const CONFIG_DIRS_ENV: &str = "TAKO_USAGE_CONFIG_DIRS";
/// 読む `CODEX_HOME` を差し替える（検証用。OS のパス区切りで並べる）。無ければ [`codex_homes`] の既定
pub const CODEX_HOMES_ENV: &str = "TAKO_USAGE_CODEX_HOMES";

/// 1 回に読む大きさ
const CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// この量を読んだら途中でも書き出す（落ちたときに読み直す量の上限）
const FLUSH_BYTES: u64 = 64 * 1024 * 1024;
/// メモリ圧を見る間隔
const PRESSURE_EVERY: Duration = Duration::from_secs(1);
/// 月のファイルを書き直す下限（これより小さければ重複の行があっても書き直さない）
const COMPACT_MIN_BYTES: u64 = 1024 * 1024;
/// 日単位へまとめる・消す月（その月の終わりからの日数）
const DAILY_AFTER_DAYS: i64 = core_index::DAILY_AFTER_DAYS;
const RETAIN_DAYS: i64 = 365;
/// 月の掃除（日単位へまとめる・古い月を消す）の間隔
const MAINTAIN_EVERY_MS: u64 = 24 * 3600 * 1000;
/// `projects/` の下を辿る深さ（`<slug>/<sid>/subagents/agent-*.jsonl` = 4）
const MAX_DEPTH: usize = 4;
/// codex の `sessions/` の下を辿る深さ（`<YYYY>/<MM>/<DD>/rollout-*.jsonl` = 3。余裕を 1 つ）
const CODEX_MAX_DEPTH: usize = 4;
/// 走査に出なくなったファイルの読み位置の行を残す期間（transcript の保持 30 日 + 余裕）
const GONE_KEEP_MS: u64 = 40 * 24 * 3600 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 読む設定 dir 1 つ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub dir: PathBuf,
    /// claude の既定の設定 dir（`CLAUDE_CONFIG_DIR` 無し = 資格情報は `<ホーム>/.claude.json`）
    pub is_default: bool,
}

fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// 読む設定 dir（既定 → tako 自身の `CLAUDE_CONFIG_DIR` → accounts.yaml の `config_dir`）。
///
/// **検証プロセス（`TAKO_ISOLATED` / セルフテスト / `cargo test`）では既定を検証用のホームへ倒し、
/// 本物の `~/.claude*` を読まない**（S1 の資格情報の置き場 = `agent_config_home` と揃える）。
/// 隔離の GUI・CLI が利用者の 4 GB の会話記録を取り込み始めないため。差し替えは [`CONFIG_DIRS_ENV`]
pub fn roots() -> Vec<Root> {
    let home = crate::orchestrator::agent_config_home();
    let default_dir = home.as_ref().map(|h| normalize(&h.join(".claude")));
    let mut out: Vec<Root> = Vec::new();
    let push = |dir: PathBuf, out: &mut Vec<Root>| {
        let dir = normalize(&dir);
        if !out.iter().any(|r| r.dir == dir) {
            let is_default = default_dir.as_ref() == Some(&dir);
            out.push(Root { dir, is_default });
        }
    };
    if let Some(list) = std::env::var_os(CONFIG_DIRS_ENV).filter(|v| !v.is_empty()) {
        for dir in std::env::split_paths(&list) {
            if !dir.as_os_str().is_empty() {
                push(dir, &mut out);
            }
        }
        return out;
    }
    if let Some(dir) = &default_dir {
        push(dir.clone(), &mut out);
    }
    let verification = tako_core::paths::is_verification_process();
    if !verification {
        if let Some(env) = std::env::var_os(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV) {
            let value = env.to_string_lossy().to_string();
            if !value.trim().is_empty() {
                push(
                    PathBuf::from(crate::orchestrator::expand_tilde(&value)),
                    &mut out,
                );
            }
        }
    }
    for (_, path) in accounts_list() {
        if let Some(path) = path {
            push(path, &mut out);
        }
    }
    out
}

/// 読む `CODEX_HOME`（S5 #2204）: `CODEX_HOME`（codex 自身と同じく env を先に見る）→ `<ホーム>/.codex`。
///
/// **検証プロセスでは検証用のホームの `.codex` へ倒し、本物の `~/.codex` も `CODEX_HOME` も読まない**
/// （[`roots`] と同じ理由）。差し替えは [`CODEX_HOMES_ENV`]
pub fn codex_homes() -> Vec<PathBuf> {
    if let Some(list) = std::env::var_os(CODEX_HOMES_ENV).filter(|v| !v.is_empty()) {
        return std::env::split_paths(&list)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| normalize(&p))
            .collect();
    }
    if tako_core::paths::is_verification_process() {
        return crate::orchestrator::agent_config_home()
            .map(|h| vec![normalize(&h.join(".codex"))])
            .unwrap_or_default();
    }
    crate::codex_session::codex_home()
        .map(|h| vec![normalize(&h)])
        .unwrap_or_default()
}

/// codex の「アカウント名」（tako の accounts.yaml は claude の設定 dir なので、`CODEX_HOME` の名前で代える。
/// 既定の `~/.codex` は `codex`）
fn codex_account_name(home: &Path) -> String {
    let name = home
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match name.as_str() {
        "" | ".codex" => "codex".to_string(),
        other => other.to_string(),
    }
}

/// accounts.yaml の（名前, 設定 dir。inherit は `None`）。壊れたエントリは飛ばす
fn accounts_list() -> Vec<(String, Option<PathBuf>)> {
    crate::orchestrator::AccountsConfig::load()
        .map(|cfg| accounts_of(&cfg))
        .unwrap_or_default()
}

fn accounts_of(cfg: &crate::orchestrator::AccountsConfig) -> Vec<(String, Option<PathBuf>)> {
    cfg.list_resolved()
        .into_iter()
        .filter_map(|(name, r)| {
            r.ok()
                .map(|r| (name, r.config_dir.path().map(PathBuf::from)))
        })
        .collect()
}

/// 設定 dir に当たる tako のアカウント名（既定 = inherit のアカウント。当たらなければ dir の名前）
fn account_for(root: &Root, accounts: &[(String, Option<PathBuf>)]) -> Option<String> {
    let found = accounts.iter().find(|(_, path)| match path {
        None => root.is_default,
        Some(p) => normalize(p) == root.dir,
    });
    match found {
        Some((name, _)) => Some(name.clone()),
        None => root
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string()),
    }
}

// --- 帰属の材料 ---------------------------------------------------------------------------------

/// カタログの 1 件から使う欄
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogKeys {
    pub kind: Option<String>,
    pub label: Option<String>,
    pub project: Option<String>,
    pub profile: Option<String>,
}

/// レジストリの 1 件から使う欄
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistryKeys {
    pub label: Option<String>,
    pub project: Option<String>,
    pub ledger_id: Option<String>,
}

/// 帰属の材料（カタログ・レジストリ・台帳・プロジェクト・アカウント）。テストは直に組む
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// session_id → カタログ
    pub catalog: HashMap<String, CatalogKeys>,
    /// session_id → レジストリ
    pub registry: HashMap<String, RegistryKeys>,
    /// 台帳の id → task_type
    pub ledger: HashMap<String, String>,
    /// （プロジェクトのキー, cwd）
    pub projects: Vec<(String, String)>,
    /// （アカウント名, 設定 dir。inherit は `None`）
    pub accounts: Vec<(String, Option<PathBuf>)>,
}

/// 帰属の材料の置き場
#[derive(Debug, Clone, Default)]
pub struct SourcePaths {
    pub catalog: Option<PathBuf>,
    pub registry: Option<PathBuf>,
    pub ledger: Option<PathBuf>,
    pub projects: Option<PathBuf>,
    pub accounts: Option<PathBuf>,
}

impl SourcePaths {
    /// 本物の置き場（`sessions.yaml` / `workers.yaml` / `ledger.yaml` / `projects.yaml` / `accounts.yaml`）
    pub fn for_process() -> Self {
        Self {
            catalog: crate::sessions::catalog_path(),
            registry: crate::orchestrator::registry::registry_path(),
            ledger: crate::orchestrator::ledger::ledger_path(),
            projects: crate::orchestrator::projects_yaml_path(),
            accounts: crate::orchestrator::accounts_yaml_path(),
        }
    }
}

impl Sources {
    /// 本物の置き場から読む（読めないものは空 = その鍵は付かない）
    pub fn load() -> Self {
        Self::load_from(&SourcePaths::for_process())
    }

    /// 置き場を指定して読む
    pub fn load_from(paths: &SourcePaths) -> Self {
        let mut out = Sources::default();
        if let Some(cat) = paths
            .catalog
            .as_deref()
            .and_then(|p| crate::sessions::SessionCatalog::load_from(p).ok())
        {
            for (sid, e) in cat.entries {
                let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
                out.catalog.insert(
                    sid,
                    CatalogKeys {
                        kind: some(&e.kind),
                        label: e.label,
                        project: e.project,
                        profile: e.profile,
                    },
                );
            }
        }
        if let Some(reg) = paths
            .registry
            .as_deref()
            .and_then(|p| crate::orchestrator::registry::WorkerRegistry::load_from(p).ok())
        {
            for w in reg.workers.into_values() {
                let Some(sid) = w.session_id.clone() else {
                    continue;
                };
                out.registry.insert(
                    sid,
                    RegistryKeys {
                        label: w.label,
                        project: (!w.project.is_empty()).then_some(w.project),
                        ledger_id: w.ledger_id,
                    },
                );
            }
        }
        if out.registry.values().any(|r| r.ledger_id.is_some()) {
            if let Some(ledger) = paths
                .ledger
                .as_deref()
                .and_then(|p| crate::orchestrator::ledger::Ledger::load_from(p).ok())
            {
                for e in ledger.entries {
                    out.ledger.insert(e.id, e.task_type);
                }
            }
        }
        if let Some(projects) = paths
            .projects
            .as_deref()
            .and_then(|p| crate::orchestrator::ProjectsConfig::load_from(p).ok())
        {
            out.projects = projects
                .list_resolved()
                .into_iter()
                .map(|p| (p.key, p.cwd))
                .collect();
        }
        out.accounts = paths
            .accounts
            .as_deref()
            .and_then(|p| crate::orchestrator::AccountsConfig::load_from(p).ok())
            .map(|cfg| accounts_of(&cfg))
            .unwrap_or_default();
        out
    }
}

/// 会話の鍵を今の材料で引く（§3.2 の決め方の順）
fn fresh_keys(
    session: &str,
    seen: &core_index::SessionSeen,
    (login, agent): (&LoginKey, &str),
    account: Option<String>,
    sources: &Sources,
    now_ms: u64,
) -> SessionKeys {
    let cat = sources.catalog.get(session);
    let reg = sources.registry.get(session);
    let label = cat
        .and_then(|c| c.label.clone())
        .or_else(|| reg.and_then(|r| r.label.clone()));
    let (project, project_from) = if let Some(p) = cat.and_then(|c| c.project.clone()) {
        (Some(p), Some(ProjectFrom::Catalog))
    } else if let Some(p) = reg.and_then(|r| r.project.clone()) {
        (Some(p), Some(ProjectFrom::Catalog))
    } else if let Some(p) = cat.and_then(|c| c.profile.clone()) {
        (Some(p), Some(ProjectFrom::Profile))
    } else if let Some(p) = seen
        .cwd
        .as_deref()
        .and_then(|cwd| core_index::project_for_cwd(cwd, &sources.projects))
    {
        (Some(p), Some(ProjectFrom::Cwd))
    } else {
        (None, None)
    };
    let ledger_id = reg.and_then(|r| r.ledger_id.clone());
    SessionKeys {
        session_id: session.to_string(),
        login: login.key.clone(),
        key_from: login.from,
        agent: agent.to_string(),
        account,
        role: cat.and_then(|c| c.kind.clone()),
        project,
        project_from,
        issue: label.as_deref().and_then(core_index::first_issue),
        task_type: ledger_id
            .as_ref()
            .and_then(|id| sources.ledger.get(id).cloned()),
        ledger_id,
        label,
        first_at: seen.first_at.unwrap_or(0),
        recorded_at: now_ms,
    }
}

// --- 索引の状態 -------------------------------------------------------------------------------

/// 1 ファイルの読みの状態
#[derive(Debug, Clone, Default)]
struct FileState {
    dev: u64,
    ino: u64,
    offset: u64,
    last: Option<LastMessage>,
    /// このファイルで数えた応答の鍵（書き直しのときに 1 行へまとめる）
    ids: Vec<u64>,
    /// 最後に行を書いた時刻（epoch ms。走査に出なくなったファイルの行を残す期間の起点）
    at: u64,
    /// codex の rollout の読みの状態（S5）
    codex: Option<CodexCursor>,
}

/// ファイルの同一性（更新時刻・大きさ・inode が同じ間は読み直さない）
type Stamp = Option<(u64, SystemTime, u64)>;

fn stamp_of(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().unwrap_or(UNIX_EPOCH), ino_of(&m)))
}

#[cfg(unix)]
fn ino_of(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.ino()
}

#[cfg(not(unix))]
fn ino_of(_: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn dev_of(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.dev()
}

#[cfg(not(unix))]
fn dev_of(_: &std::fs::Metadata) -> u64 {
    0
}

/// `dir` の下（深さ `max_depth` まで）の `*.jsonl` を `visit` へ渡す
fn walk_jsonl(dir: &Path, max_depth: usize, visit: &mut dyn FnMut(PathBuf, &std::fs::Metadata)) {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if depth < max_depth {
                    stack.push((path, depth + 1));
                }
            } else if ft.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                let Ok(meta) = entry.metadata() else { continue };
                visit(path, &meta);
            }
        }
    }
}

#[derive(Default)]
struct Loaded {
    files: HashMap<String, FileState>,
    seen: HashSet<u64>,
    cursor_rows: usize,
    keys: BTreeMap<String, SessionKeys>,
    cursor_stamp: Stamp,
    keys_stamp: Stamp,
}

/// 月のファイルの書き直しの覚え（`buckets/compacted.json`）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct CompactedState {
    #[serde(default)]
    months: BTreeMap<String, CompactedMonth>,
    /// 最後に月の掃除をした時刻（epoch ms）
    #[serde(default)]
    maintained_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct CompactedMonth {
    /// 書き直した直後の大きさ
    size: u64,
    #[serde(default)]
    daily: bool,
}

/// 更新の結果（診断ログ・応答の `index` に出すのは件数・バイト数・所要だけ）
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UpdateOutcome {
    /// 見つけた transcript の数
    pub files: usize,
    /// この回に読んだ transcript の数
    pub read_files: usize,
    pub bytes_read: u64,
    /// 新しく数えた応答
    pub responses: u64,
    /// 鍵を写した（良くなった）会話
    pub keyed_sessions: usize,
    /// 全部読み終えた
    pub caught_up: bool,
    /// 読み残し（時間の上限・メモリ圧で止まった分）
    pub pending_files: usize,
    pub pending_bytes: u64,
    /// 途中で止めた理由（`budget` / `memory_pressure`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<&'static str>,
    pub elapsed_ms: u64,
    /// 見つけた codex の rollout の数（`files` の内数。S5）
    pub codex_files: usize,
    /// codex の rollout から `limits.jsonl` へ足した観測
    pub codex_observations: usize,
}

/// 更新の決まり
#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateOptions {
    /// 1 回の時間の上限（`None` = 読み終えるまで）
    pub budget: Option<Duration>,
    /// メモリ圧が warn 以上なら止める（GUI の背景。CLI は利用者が頼んだので止めない）
    pub stop_on_pressure: bool,
    /// 他の更新が走っていれば待たずに諦める（GUI の背景）
    pub try_lock: bool,
}

/// どこの置き場のファイルか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// claude の設定 dir（[`Indexer::roots`] の添字）
    Claude(usize),
    /// `CODEX_HOME`（[`Indexer::codex_homes`] の添字）
    Codex(usize),
}

/// 見つけた transcript 1 本
struct Found {
    path: PathBuf,
    key: String,
    origin: Origin,
    len: u64,
    dev: u64,
    ino: u64,
}

/// 使用量の索引（GUI はプロセスに 1 つを使い回し、CLI は 1 回ごとに作る）
pub struct Indexer {
    dir: PathBuf,
    salt_file: PathBuf,
    roots: Vec<Root>,
    /// 読む `CODEX_HOME`（S5）
    codex_homes: Vec<PathBuf>,
    /// codex の観測の重複除去（ログイン・枠・解除時刻・% → 書いたいちばん早い観測時刻）。
    /// 最初に書く直前に `limits.jsonl` の codex の行から作る
    codex_seen: Option<HashMap<(String, String, i64, u64), u64>>,
    logins: LoginResolver,
    salt: Option<String>,
    loaded: Option<Loaded>,
    /// 材料の差し替え（テスト）。無ければ更新のたびに [`Sources::load_from`]
    sources: Option<Sources>,
    /// 材料の置き場（無ければ本物の置き場）
    source_paths: Option<SourcePaths>,
    /// この回の更新で読んだ材料（書き出しのたびに読み直さない）
    run_sources: Option<Sources>,
    /// 月の掃除のローカル時刻の UTC からのずれ（秒。テストで固定する）
    utc_offset: Option<i32>,
}

impl Indexer {
    /// `home` は `CLAUDE_CONFIG_DIR` 無しの claude が読む資格情報の置き場（`<home>/.claude.json`）
    pub fn new(dir: PathBuf, salt_file: PathBuf, roots: Vec<Root>, home: Option<PathBuf>) -> Self {
        Self {
            dir,
            salt_file,
            roots,
            codex_homes: Vec::new(),
            codex_seen: None,
            logins: LoginResolver::new(home),
            salt: None,
            loaded: None,
            sources: None,
            source_paths: None,
            run_sources: None,
            utc_offset: None,
        }
    }

    /// 本物の置き場（`<data_dir>/usage/` と [`roots`] と [`codex_homes`]）
    pub fn for_process() -> Option<Self> {
        let data = tako_core::paths::data_dir()?;
        Some(
            Self::new(
                data.join("usage"),
                data.join("machine_id"),
                roots(),
                crate::orchestrator::agent_config_home(),
            )
            .with_codex_homes(codex_homes()),
        )
    }

    /// 読む `CODEX_HOME` を決める（S5）
    pub fn with_codex_homes(mut self, homes: Vec<PathBuf>) -> Self {
        self.codex_homes = homes;
        self
    }

    /// 帰属の材料を固定する（テスト）
    pub fn with_sources(mut self, sources: Sources) -> Self {
        self.sources = Some(sources);
        self
    }

    /// 材料の置き場を指定する（テスト・検証）
    pub fn with_source_paths(mut self, paths: SourcePaths) -> Self {
        self.source_paths = Some(paths);
        self
    }

    /// 月の掃除のローカル時刻のずれを固定する（テスト）
    pub fn with_utc_offset(mut self, offset: i32) -> Self {
        self.utc_offset = Some(offset);
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn cursor_path(&self) -> PathBuf {
        self.dir.join(CURSOR_FILE)
    }

    fn keys_path(&self) -> PathBuf {
        self.dir.join(KEYS_FILE)
    }

    /// 状態を読む（自分が最後に書いたときから変わっていなければ読み直さない）
    fn ensure_loaded(&mut self) {
        let cursor_stamp = stamp_of(&self.cursor_path());
        let keys_stamp = stamp_of(&self.keys_path());
        if let Some(l) = &self.loaded {
            if l.cursor_stamp == cursor_stamp && l.keys_stamp == keys_stamp {
                return;
            }
        }
        let mut loaded = Loaded {
            cursor_stamp,
            keys_stamp,
            ..Loaded::default()
        };
        let (rows, _) = read_all::<CursorRow>(&self.cursor_path());
        loaded.cursor_rows = rows.len();
        for row in rows {
            let state = loaded.files.entry(row.file).or_default();
            state.dev = row.dev;
            state.ino = row.ino;
            state.offset = row.offset;
            state.last = row.last;
            state.at = row.at;
            state.codex = row.codex;
            for id in row.ids.iter().filter_map(|s| core_index::parse_hex64(s)) {
                if loaded.seen.insert(id) {
                    state.ids.push(id);
                }
            }
        }
        let (keys, _) = read_all::<SessionKeys>(&self.keys_path());
        for k in keys {
            loaded.keys.insert(k.session_id.clone(), k);
        }
        self.loaded = Some(loaded);
    }

    /// transcript を探す（`projects/` の下の `*.jsonl` と、codex の `sessions/` / `archived_sessions/` の
    /// `rollout-*.jsonl`。並びはパス順 = 写しの数え先が毎回同じ）
    fn walk(&self) -> Vec<Found> {
        self.walk_with(core_codex::s5_legacy())
    }

    /// [`Self::walk`] の本体。`s5_legacy` は [`core_codex::s5_legacy`] の値（テストは env を触らずに
    /// 両アームを検査できる。env はプロセスに 1 つで、立てると並走するテストの走査まで変わる = #2395）
    fn walk_with(&self, s5_legacy: bool) -> Vec<Found> {
        let mut out = Vec::new();
        for (i, root) in self.roots.iter().enumerate() {
            walk_jsonl(&root.dir.join("projects"), MAX_DEPTH, &mut |path, meta| {
                let key =
                    core_index::hex64(core_index::fnv64(&["transcript", &path.to_string_lossy()]));
                out.push(Found {
                    len: meta.len(),
                    dev: dev_of(meta),
                    ino: ino_of(meta),
                    path,
                    key,
                    origin: Origin::Claude(i),
                });
            });
        }
        if !s5_legacy {
            for (i, home) in self.codex_homes.iter().enumerate() {
                // 鍵はファイル名（会話の id 入り）: `sessions/` から `archived_sessions/` へ移っても同じ読み位置。
                // 両方にあれば `sessions/` を正にする（先に見たほう）
                let mut keys: HashSet<String> = HashSet::new();
                for (sub, depth) in [("sessions", CODEX_MAX_DEPTH), ("archived_sessions", 1)] {
                    walk_jsonl(&home.join(sub), depth, &mut |path, meta| {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if !name.starts_with("rollout-") {
                            return;
                        }
                        let key = core_index::hex64(core_index::fnv64(&[
                            "codex",
                            &home.to_string_lossy(),
                            &name,
                        ]));
                        if keys.insert(key.clone()) {
                            out.push(Found {
                                len: meta.len(),
                                dev: dev_of(meta),
                                ino: ino_of(meta),
                                path,
                                key,
                                origin: Origin::Codex(i),
                            });
                        }
                    });
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    fn salt(&mut self) -> Result<String, String> {
        if let Some(salt) = &self.salt {
            return Ok(salt.clone());
        }
        let salt = load_or_create_salt(&self.salt_file)?;
        self.salt = Some(salt.clone());
        Ok(salt)
    }

    /// 追記された分を読んで索引へ足す
    pub fn update(&mut self, opts: UpdateOptions) -> Result<UpdateOutcome, String> {
        let started = Instant::now();
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("置き場を作れない: {}", e.kind()))?;
        let Some(_lock) = lock_index(&self.cursor_path(), opts.try_lock)? else {
            return Ok(UpdateOutcome {
                stopped: Some("locked"),
                ..UpdateOutcome::default()
            });
        };
        self.ensure_loaded();
        self.run_sources = None;
        let found = self.walk();
        let mut outcome = UpdateOutcome {
            files: found.len(),
            codex_files: found
                .iter()
                .filter(|f| matches!(f.origin, Origin::Codex(_)))
                .count(),
            ..UpdateOutcome::default()
        };
        // 読むもの（読み位置より大きい・縮んだ・置き換わった）
        let work: Vec<&Found> = {
            let loaded = self.loaded.as_ref().expect("ensure_loaded の後");
            found
                .iter()
                .filter(|f| match loaded.files.get(&f.key) {
                    None => f.len > 0,
                    Some(s) => s.ino != f.ino || s.dev != f.dev || f.len != s.offset,
                })
                .collect()
        };
        let mut pending = Pending::default();
        // メモリ圧を最後に見た時刻（まだ見ていない = None。最初のファイルの前に 1 回見る）
        let mut last_pressure: Option<Instant> = None;
        let mut touched_months: BTreeSet<String> = BTreeSet::new();
        let mut index = 0usize;
        while index < work.len() {
            let f = work[index];
            if let Some(budget) = opts.budget {
                if started.elapsed() >= budget {
                    outcome.stopped = Some("budget");
                    break;
                }
            }
            if opts.stop_on_pressure && last_pressure.is_none_or(|t| t.elapsed() >= PRESSURE_EVERY)
            {
                last_pressure = Some(Instant::now());
                if memory_pressure_high() {
                    outcome.stopped = Some("memory_pressure");
                    break;
                }
            }
            let done = self.read_file(f, &mut pending, &mut outcome, started, opts)?;
            if pending.bytes >= FLUSH_BYTES {
                self.flush(&mut pending, &mut outcome, &mut touched_months)?;
            }
            if !done {
                // 時間の上限でファイルの途中で止めた（読んだ所までは書く）
                outcome.stopped = Some("budget");
                break;
            }
            index += 1;
        }
        self.flush(&mut pending, &mut outcome, &mut touched_months)?;
        self.run_sources = None;
        let rest = &work[index.min(work.len())..];
        outcome.pending_files = rest.len();
        outcome.pending_bytes = {
            let loaded = self.loaded.as_ref().expect("ensure_loaded の後");
            rest.iter()
                .map(|f| {
                    let offset = loaded
                        .files
                        .get(&f.key)
                        .filter(|s| s.ino == f.ino && s.dev == f.dev && s.offset <= f.len)
                        .map_or(0, |s| s.offset);
                    f.len.saturating_sub(offset)
                })
                .sum()
        };
        outcome.caught_up = outcome.stopped.is_none();
        self.compact_cursor(&found)?;
        self.maintain_buckets(&touched_months)?;
        self.refresh_stamps();
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(outcome)
    }

    /// 1 ファイルを読み位置から読む。時間の上限で途中で止めたら `false`
    fn read_file(
        &mut self,
        f: &Found,
        pending: &mut Pending,
        outcome: &mut UpdateOutcome,
        started: Instant,
        opts: UpdateOptions,
    ) -> Result<bool, String> {
        let loaded = self.loaded.as_mut().expect("ensure_loaded の後");
        let state = loaded.files.entry(f.key.clone()).or_default();
        // 縮んだ・置き換わった: 最初から読み直す（数え済みの応答は鍵で飛ばす。codex は数え済みの累積を持ち越す）
        let mut reset = false;
        if state.ino != f.ino || state.dev != f.dev || f.len < state.offset {
            reset = state.offset > 0 || state.last.is_some() || state.codex.is_some();
            state.offset = 0;
            state.last = None;
            state.codex = state.codex.as_ref().map(CodexCursor::restarted);
            state.dev = f.dev;
            state.ino = f.ino;
        }
        let Ok(mut file) = std::fs::File::open(&f.path) else {
            return Ok(true);
        };
        if file.seek(SeekFrom::Start(state.offset)).is_err() {
            return Ok(true);
        }
        let fallback = f
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut new_ids = Vec::new();
        let mut file_batch = Batch::default();
        let mut codex_read = CodexRead::default();
        // codex の今の cwd（鍵の材料。読み位置には書かない）
        let mut codex_cwd: Option<String> = None;
        if matches!(f.origin, Origin::Codex(_)) && state.codex.is_none() {
            state.codex = Some(CodexCursor::default());
        }
        let mut finished = true;
        let start_offset = state.offset;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            carry.extend_from_slice(&buf[..n]);
            let consumed = match (f.origin, state.codex.as_mut()) {
                (Origin::Codex(_), Some(codex)) => core_codex::ingest_codex_lines(
                    &carry,
                    CodexProgress {
                        state: codex,
                        fallback_session: &fallback,
                        cwd: &mut codex_cwd,
                    },
                    &mut file_batch,
                    &mut codex_read,
                ),
                _ => core_index::ingest_lines(
                    &carry,
                    FileProgress {
                        last: &mut state.last,
                        seen: &mut loaded.seen,
                        new_ids: &mut new_ids,
                        fallback_session: &fallback,
                    },
                    &mut file_batch,
                ),
            };
            carry.drain(..consumed);
            state.offset += consumed as u64;
            if let Some(budget) = opts.budget {
                if started.elapsed() >= budget {
                    finished = false;
                    break;
                }
            }
        }
        let read = state.offset - start_offset;
        outcome.bytes_read += read;
        if read > 0 {
            outcome.read_files += 1;
        }
        // 何も読めなかった（書きかけの 1 行だけ）なら読み位置の行を足さない
        if read == 0 && !reset {
            return Ok(finished);
        }
        state.ids.extend(new_ids.iter().copied());
        state.at = now_ms();
        let row = CursorRow {
            file: f.key.clone(),
            dev: state.dev,
            ino: state.ino,
            offset: state.offset,
            last: state.last.clone(),
            ids: new_ids.iter().map(|id| core_index::hex64(*id)).collect(),
            at: state.at,
            codex: state.codex.clone(),
        };
        for sid in file_batch.sessions.keys() {
            pending.session_root.entry(sid.clone()).or_insert(f.origin);
        }
        if let Origin::Codex(home) = f.origin {
            pending
                .codex_limits
                .extend(codex_read.limits.drain(..).map(|l| (home, l)));
        }
        pending.bytes += read;
        pending.batch.absorb(file_batch);
        pending.cursor.push(row);
        Ok(finished)
    }

    /// 溜めた分を「鍵 → 集約 → 読み位置」の順に書く
    fn flush(
        &mut self,
        pending: &mut Pending,
        outcome: &mut UpdateOutcome,
        touched_months: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if pending.cursor.is_empty() && pending.batch.is_empty() && pending.codex_limits.is_empty()
        {
            return Ok(());
        }
        let p = std::mem::take(pending);
        outcome.responses += p.batch.responses;
        // 1. 鍵（触った会話の鍵を今の材料で引き、良くなったものだけ足す）。順は「鍵 → 集約 → 観測 → 読み位置」
        let key_rows = if p.batch.sessions.is_empty() {
            Vec::new()
        } else {
            self.key_rows(&p.batch, &p.session_root)?
        };
        outcome.keyed_sessions += key_rows.len();
        append_rows(&self.dir, &self.keys_path(), &key_rows)?;
        if let Some(loaded) = self.loaded.as_mut() {
            for k in key_rows {
                loaded.keys.insert(k.session_id.clone(), k);
            }
        }
        // 2. 集約（月ごと）
        let mut by_month: BTreeMap<String, Vec<BucketRow>> = BTreeMap::new();
        for row in p.batch.rows() {
            by_month
                .entry(core_index::month_of(row.t))
                .or_default()
                .push(row);
        }
        let buckets = self.dir.join(BUCKETS_DIR);
        for (month, rows) in &by_month {
            append_rows(&buckets, &buckets.join(format!("{month}.jsonl")), rows)?;
            touched_months.insert(month.clone());
        }
        // 3. codex の利用制限の観測（`limits.jsonl`。読み位置より先に書く = 落ちても次の読み直しで足りる分は足す）
        if !p.codex_limits.is_empty() {
            let rows = self.codex_limit_rows(p.codex_limits)?;
            if let Err(e) = append_rows(&self.dir, &self.dir.join(LIMITS_FILE), &rows) {
                // 覚えだけ進んで行が無い状態にしない（次の読み直しの前にファイルから作り直す）
                self.codex_seen = None;
                return Err(e);
            }
            outcome.codex_observations += rows.len();
        }
        // 4. 読み位置
        append_rows(&self.dir, &self.cursor_path(), &p.cursor)?;
        if let Some(loaded) = self.loaded.as_mut() {
            loaded.cursor_rows += p.cursor.len();
        }
        Ok(())
    }

    fn key_rows(
        &mut self,
        batch: &Batch,
        session_root: &HashMap<String, Origin>,
    ) -> Result<Vec<SessionKeys>, String> {
        let salt = self.salt()?;
        if self.run_sources.is_none() {
            self.run_sources = Some(match (&self.sources, &self.source_paths) {
                (Some(s), _) => s.clone(),
                (None, Some(paths)) => Sources::load_from(paths),
                (None, None) => Sources::load(),
            });
        }
        let sources = self.run_sources.as_ref().expect("直前で入れた");
        let now = now_ms();
        let mut rows = Vec::new();
        for (sid, seen) in &batch.sessions {
            let (login, agent, account) = match session_root.get(sid) {
                Some(Origin::Claude(i)) => {
                    let Some(root) = self.roots.get(*i).cloned() else {
                        continue;
                    };
                    let config_dir =
                        (!root.is_default).then(|| root.dir.to_string_lossy().to_string());
                    (
                        self.logins.login_for(config_dir.as_deref(), &salt),
                        "claude",
                        account_for(&root, &sources.accounts),
                    )
                }
                Some(Origin::Codex(i)) => {
                    let Some(home) = self.codex_homes.get(*i).cloned() else {
                        continue;
                    };
                    (
                        self.logins.codex_login_for(&home, &salt),
                        "codex",
                        Some(codex_account_name(&home)),
                    )
                }
                None => continue,
            };
            let fresh = fresh_keys(sid, seen, (&login, agent), account, sources, now);
            let old = self.loaded.as_ref().and_then(|l| l.keys.get(sid));
            match old {
                None => rows.push(fresh),
                Some(old) => rows.extend(old.merged(&fresh)),
            }
        }
        Ok(rows)
    }

    /// codex の観測へログインを付け、書いていないもの（同じログイン・枠・解除時刻・% のいちばん早い観測）だけを
    /// 行にする。同じログインの会話は同じ値を何度も報告し、読み直しでも同じ行が来るので、値ごとに 1 行へ絞る
    /// （窓の切り方は値の上がった最初の点しか使わない = `tako_core::usage::share::windows_for`）
    fn codex_limit_rows(
        &mut self,
        mut limits: Vec<(usize, CodexLimit)>,
    ) -> Result<Vec<LimitObservation>, String> {
        let salt = self.salt()?;
        if self.codex_seen.is_none() {
            let (rows, _) = read_all::<LimitObservation>(&self.dir.join(LIMITS_FILE));
            let mut seen: HashMap<(String, String, i64, u64), u64> = HashMap::new();
            for o in rows.into_iter().filter(|o| o.source == SOURCE_CODEX) {
                let Some(r) = o.resets_at else { continue };
                let entry = seen
                    .entry((o.login, o.kind, r, o.percent.to_bits()))
                    .or_insert(o.observed_at);
                *entry = (*entry).min(o.observed_at);
            }
            self.codex_seen = Some(seen);
        }
        limits.sort_by_key(|(_, l)| l.observed_at);
        let now = now_ms();
        let mut logins: HashMap<usize, LoginKey> = HashMap::new();
        let mut out = Vec::new();
        for (home, l) in limits {
            let login = match logins.get(&home) {
                Some(login) => login.clone(),
                None => {
                    let Some(path) = self.codex_homes.get(home).cloned() else {
                        continue;
                    };
                    let login = self.logins.codex_login_for(&path, &salt);
                    logins.insert(home, login.clone());
                    login
                }
            };
            let key = (
                login.key.clone(),
                l.kind.to_string(),
                l.resets_at,
                l.percent.to_bits(),
            );
            let seen = self.codex_seen.get_or_insert_with(HashMap::new);
            if seen.get(&key).is_some_and(|at| *at <= l.observed_at) {
                continue;
            }
            seen.insert(key, l.observed_at);
            out.push(LimitObservation {
                login: login.key,
                key_from: login.from,
                kind: l.kind.to_string(),
                percent: l.percent,
                resets_at: Some(l.resets_at),
                observed_at: l.observed_at,
                recorded_at: now,
                source: SOURCE_CODEX.to_string(),
            });
        }
        Ok(out)
    }

    /// `cursor.jsonl` の行が残すファイルの 2 倍 + 500 を超えたら、1 ファイル 1 行へ書き直す。
    /// 走査に出なくなったファイル（30 日で消えた transcript・外したアカウント）の行は、最後の行から
    /// [`GONE_KEEP_MS`] 残してから落とす（一時的に読めなかっただけのファイルの鍵を落とすと、
    /// 次の読み込みで最初から読み直して二重に数えるため）
    fn compact_cursor(&mut self, found: &[Found]) -> Result<(), String> {
        let path = self.cursor_path();
        let Some(loaded) = self.loaded.as_mut() else {
            return Ok(());
        };
        let live: HashSet<&str> = found.iter().map(|f| f.key.as_str()).collect();
        let now = now_ms();
        let keep =
            |k: &str, s: &FileState| live.contains(k) || now.saturating_sub(s.at) < GONE_KEEP_MS;
        let kept = loaded.files.iter().filter(|(k, s)| keep(k, s)).count();
        if loaded.cursor_rows <= kept * 2 + 500 {
            return Ok(());
        }
        let mut rows: Vec<CursorRow> = loaded
            .files
            .iter()
            .filter(|(k, s)| keep(k, s))
            .map(|(k, s)| CursorRow {
                file: k.clone(),
                dev: s.dev,
                ino: s.ino,
                offset: s.offset,
                last: s.last.clone(),
                ids: s.ids.iter().map(|id| core_index::hex64(*id)).collect(),
                at: s.at,
                codex: s.codex.clone(),
            })
            .collect();
        rows.sort_by(|a, b| a.file.cmp(&b.file));
        write_rows(&path, &rows)?;
        loaded.files.retain(|k, s| keep(k, s));
        loaded.cursor_rows = rows.len();
        Ok(())
    }

    /// 月のファイルの掃除: 育った月は同じ鍵を合わせて書き直し、35 日より古い月は日単位へまとめ、
    /// 365 日より古い月は消す（古い月の見直しは 1 日に 1 回）
    fn maintain_buckets(&mut self, touched: &BTreeSet<String>) -> Result<(), String> {
        let dir = self.dir.join(BUCKETS_DIR);
        let state_path = dir.join(COMPACTED_FILE);
        let mut state: CompactedState = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let before = state.clone();
        let now = now_ms();
        let now_secs = (now / 1000) as i64;
        let offset = self
            .utc_offset
            .unwrap_or_else(tako_core::platform::clock::local_utc_offset);
        let mut months: BTreeSet<String> = touched.clone();
        let full = now.saturating_sub(state.maintained_at) >= MAINTAIN_EVERY_MS;
        if full {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if let Some(m) = name.strip_suffix(".jsonl") {
                        months.insert(m.to_string());
                    }
                }
            }
            state.maintained_at = now;
        }
        for month in months {
            let Some((_, end)) = core_index::month_range(&month) else {
                continue;
            };
            let path = dir.join(format!("{month}.jsonl"));
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                state.months.remove(&month);
                continue;
            }
            if end + RETAIN_DAYS * 86_400 < now_secs {
                let _ = std::fs::remove_file(&path);
                state.months.remove(&month);
                continue;
            }
            let daily = end + DAILY_AFTER_DAYS * 86_400 < now_secs;
            let mark = state.months.get(&month).cloned().unwrap_or_default();
            let grown = size >= COMPACT_MIN_BYTES && size > mark.size.saturating_mul(2);
            if !(grown || (daily && !mark.daily) || (daily && size != mark.size)) {
                continue;
            }
            let (rows, _) = read_all::<BucketRow>(&path);
            let merged = if daily {
                core_index::to_daily(rows, offset)
            } else {
                core_index::merge_rows(rows)
            };
            write_rows(&path, &merged)?;
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            state.months.insert(month, CompactedMonth { size, daily });
        }
        if state != before {
            let text = serde_json::to_string(&state).map_err(|e| format!("覚えを組めない: {e}"))?;
            crate::config_io::atomic_write(&state_path, &text)?;
        }
        Ok(())
    }

    /// 自分で書いた後のファイルの同一性を覚え直す（次の更新で読み直さないため）
    fn refresh_stamps(&mut self) {
        let cursor = stamp_of(&self.cursor_path());
        let keys = stamp_of(&self.keys_path());
        if let Some(l) = self.loaded.as_mut() {
            l.cursor_stamp = cursor;
            l.keys_stamp = keys;
        }
    }
}

/// 書き出し待ち
#[derive(Default)]
struct Pending {
    batch: Batch,
    cursor: Vec<CursorRow>,
    /// 会話 → どの置き場（設定 dir / `CODEX_HOME`）のファイルで見たか
    session_root: HashMap<String, Origin>,
    /// codex の利用制限の観測（`CODEX_HOME` の添字つき。ログインは書くときに付ける）
    codex_limits: Vec<(usize, CodexLimit)>,
    bytes: u64,
}

/// 行を書き直す（tmp へ書いて rename）
fn write_rows<T: Serialize>(path: &Path, rows: &[T]) -> Result<(), String> {
    let mut text = String::new();
    for row in rows {
        text.push_str(&serde_json::to_string(row).map_err(|e| format!("行を組めない: {e}"))?);
        text.push('\n');
    }
    crate::config_io::atomic_write(path, &text)
}

/// 索引の排他 flock（`try_lock` なら他の更新が走っているとき `None`）
fn lock_index(target: &Path, try_lock: bool) -> Result<Option<std::fs::File>, String> {
    let mut lock_path = target.as_os_str().to_owned();
    lock_path.push(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(&lock_path))
        .map_err(|e| format!("ロックを開けない: {}", e.kind()))?;
    if try_lock {
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("ロックを取れない: {}", e.kind())),
        }
    } else {
        file.lock()
            .map_err(|e| format!("ロックを取れない: {}", e.kind()))?;
        Ok(Some(file))
    }
}

/// メモリ圧が warn 以上か（採れない OS では止めない）
fn memory_pressure_high() -> bool {
    tako_core::platform::resources::machine()
        .and_then(|m| m.pressure_level)
        .and_then(tako_core::resources::Pressure::from_level)
        .is_some_and(|p| p != tako_core::resources::Pressure::Normal)
}

/// 期間を含む月の集約の行（`from` が無ければ全部）
pub fn read_rows(dir: &Path, from: Option<i64>) -> (Vec<BucketRow>, usize) {
    let buckets = dir.join(BUCKETS_DIR);
    let Ok(entries) = std::fs::read_dir(&buckets) else {
        return (Vec::new(), 0);
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".jsonl")
                .map(str::to_string)
        })
        .filter(|m| match (from, core_index::month_range(m)) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(f), Some((_, end))) => end > f,
        })
        .collect();
    names.sort();
    let mut rows = Vec::new();
    let mut bad = 0;
    for m in names {
        let (r, b) = read_all::<BucketRow>(&buckets.join(format!("{m}.jsonl")));
        rows.extend(r);
        bad += b;
    }
    (rows, bad)
}

/// 写した鍵（会話ごとに後ろの行）
pub fn read_keys(dir: &Path) -> (BTreeMap<String, SessionKeys>, usize) {
    let (rows, bad) = read_all::<SessionKeys>(&dir.join(KEYS_FILE));
    let mut out = BTreeMap::new();
    for k in rows {
        out.insert(k.session_id.clone(), k);
    }
    (out, bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::usage::index::Tokens;

    /// 本文に入れる印（索引のどこかに出たら名指す）
    const BODY: &str = "usage-index-body-marker";

    /// 置き場・設定 dir・材料をすべて使い捨ての dir の中に作る（本物の `~/.claude*` に触れない）
    struct Env {
        _scratch: tako_core::test_residue::ScratchDir,
        root: PathBuf,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let scratch = tako_core::test_residue::ScratchDir::new(&format!("usage-index-{name}"));
            let root = scratch.path().to_path_buf();
            Self {
                _scratch: scratch,
                root,
            }
        }
        fn cfg(&self) -> PathBuf {
            self.root.join("cfg")
        }
        fn usage(&self) -> PathBuf {
            self.root.join("data/usage")
        }
        fn transcript(&self, slug: &str, name: &str) -> PathBuf {
            let path = self.cfg().join("projects").join(slug).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            path
        }
        fn sources(&self) -> Sources {
            Sources {
                projects: vec![
                    ("alpha".into(), "/home/testuser/dev/alpha".into()),
                    ("beta".into(), "/home/testuser/dev/beta".into()),
                ],
                accounts: vec![("univ".into(), Some(self.cfg()))],
                ..Sources::default()
            }
        }
        fn indexer(&self) -> Indexer {
            Indexer::new(
                self.usage(),
                self.root.join("data/machine_id"),
                vec![Root {
                    dir: self.cfg(),
                    is_default: false,
                }],
                Some(self.root.join("home")),
            )
            .with_utc_offset(0)
        }
        fn update(&self, ix: &mut Indexer) -> UpdateOutcome {
            ix.update(UpdateOptions::default()).unwrap()
        }
        fn totals(&self) -> BTreeMap<(String, String), (Tokens, u32)> {
            let (rows, bad) = read_rows(&self.usage(), None);
            assert_eq!(bad, 0);
            let mut out: BTreeMap<(String, String), (Tokens, u32)> = BTreeMap::new();
            for r in rows {
                let e = out.entry((r.session, r.model)).or_default();
                e.0.add(&r.tokens);
                e.1 += r.responses;
            }
            out
        }
    }

    const T1: &str = "2026-10-16T12:00:10.000Z";
    const T2: &str = "2026-10-16T12:07:00.000Z";

    #[allow(clippy::too_many_arguments)]
    fn assistant(id: &str, sid: &str, ts: &str, model: &str, t: [u64; 5], cwd: &str) -> String {
        let [i, o, c5, c1, cr] = t;
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","message":{{"id":"{id}","model":"{model}","role":"assistant","content":[{{"type":"text","text":"{BODY} 本文"}},{{"type":"tool_use","name":"Bash","input":{{"command":"echo {BODY}"}}}}],"usage":{{"input_tokens":{i},"output_tokens":{o},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{},"cache_creation":{{"ephemeral_5m_input_tokens":{c5},"ephemeral_1h_input_tokens":{c1}}},"speed":"standard"}}}},"requestId":"req_{id}","timestamp":"{ts}","sessionId":"{sid}","cwd":"{cwd}","gitBranch":"feat/{BODY}","wireToolInputs":{{"x":"{BODY}"}},"uuid":"u-{id}-{o}"}}"#,
            c5 + c1
        )
    }

    fn user(sid: &str) -> String {
        format!(
            r#"{{"type":"user","sessionId":"{sid}","message":{{"role":"user","content":"{BODY} の依頼 \"type\":\"assistant\""}}}}"#
        )
    }

    const ALPHA: &str = "/home/testuser/dev/alpha";
    const OPUS: &str = "claude-opus-5-5";

    fn write(path: &Path, lines: &[String], tail: &str) {
        let mut text = lines.join("\n");
        text.push('\n');
        text.push_str(tail);
        std::fs::write(path, text).unwrap();
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// 偽の transcript 一式: 分割行（仮の output）・`<synthetic>`・本文の印・サブエージェント・
    /// 書きかけの最終行・単価表に無いモデル
    fn fixture(env: &Env) -> (PathBuf, PathBuf, String) {
        let s1 = env.transcript("-home-testuser-dev-alpha", "s1.jsonl");
        let partial = assistant("msg_c", "s1", T2, OPUS, [1, 9, 0, 0, 9], ALPHA);
        // 書きかけの位置（本文に日本語があるので文字境界へ丸める）
        let half = partial.floor_char_boundary(partial.len() / 2);
        write(
            &s1,
            &[
                user("s1"),
                assistant("msg_a", "s1", T1, OPUS, [2, 5, 10, 0, 100], ALPHA),
                assistant("msg_a", "s1", T1, OPUS, [2, 50, 10, 0, 100], ALPHA),
                assistant("msg_a", "s1", T1, OPUS, [2, 120, 10, 0, 100], ALPHA),
                assistant("msg_syn", "s1", T1, "<synthetic>", [0; 5], ALPHA),
                assistant("msg_b", "s1", T2, OPUS, [1, 7, 0, 3, 200], ALPHA),
            ],
            // 書きかけ（改行なし）
            partial.get(..half).unwrap(),
        );
        let sub = env.transcript("-home-testuser-dev-alpha/s1/subagents", "agent-x.jsonl");
        write(
            &sub,
            &[assistant(
                "msg_s",
                "s1",
                T1,
                "claude-haiku-4-5-20251001",
                [0, 10, 100, 0, 1000],
                ALPHA,
            )],
            "",
        );
        let s2 = env.transcript("-home-testuser-dev-beta", "s2.jsonl");
        write(
            &s2,
            &[assistant(
                "msg_d",
                "s2",
                T1,
                "claude-sonnet-9-9",
                [5, 5, 0, 0, 5],
                "/home/testuser/dev/beta/sub",
            )],
            "",
        );
        (s1, s2, partial.get(half..).unwrap().to_string())
    }

    #[test]
    fn 偽のtranscriptを期待値どおりに数え_2回目は追記分だけ読む() {
        let env = Env::new("expect");
        let (s1, s2, rest) = fixture(&env);
        let mut ix = env.indexer().with_sources(env.sources());
        let first = env.update(&mut ix);
        assert!(first.caught_up);
        assert_eq!(first.files, 3);
        assert_eq!(first.read_files, 3);
        assert_eq!(first.responses, 4, "msg_a / msg_b / msg_s / msg_d");
        let t = env.totals();
        let opus = &t[&("s1".to_string(), OPUS.to_string())];
        assert_eq!(
            opus.0,
            Tokens {
                input: 3,
                output: 127,
                cache_5m: 10,
                cache_1h: 3,
                cache_read: 300
            },
            "分割行は最大へ寄せ、仮の output を足さない"
        );
        assert_eq!(opus.1, 2);
        let haiku = &t[&("s1".to_string(), "claude-haiku-4-5-20251001".to_string())];
        assert_eq!(haiku.0.cache_read, 1000, "サブエージェントは親の会話へ");
        assert!(!t.keys().any(|(_, m)| m == "<synthetic>"));
        // 書きかけの行の手前までしか読まない
        let s1_len = std::fs::metadata(&s1).unwrap().len();
        let s2_len = std::fs::metadata(&s2).unwrap().len();
        let sub_len = std::fs::metadata(
            env.cfg()
                .join("projects/-home-testuser-dev-alpha/s1/subagents/agent-x.jsonl"),
        )
        .unwrap()
        .len();
        let partial_len =
            (s1_len as usize) - std::fs::read_to_string(&s1).unwrap().rfind('\n').unwrap() - 1;
        assert_eq!(
            first.bytes_read,
            s1_len + s2_len + sub_len - partial_len as u64
        );
        // 鍵: cwd の最長前方一致・アカウント
        let (keys, _) = read_keys(&env.usage());
        assert_eq!(keys["s1"].project.as_deref(), Some("alpha"));
        assert_eq!(keys["s1"].project_from, Some(ProjectFrom::Cwd));
        assert_eq!(keys["s2"].project.as_deref(), Some("beta"));
        assert_eq!(keys["s1"].account.as_deref(), Some("univ"));
        assert_eq!(
            keys["s1"].first_at,
            tako_core::claude_mod::parse_resets_at(T1).unwrap()
        );

        // 2 回目（変わっていない）: 何も読まない・何も書かない
        let cursor_before = std::fs::read_to_string(env.usage().join(CURSOR_FILE)).unwrap();
        let second = env.update(&mut ix);
        assert_eq!((second.read_files, second.bytes_read), (0, 0));
        assert_eq!(
            std::fs::read_to_string(env.usage().join(CURSOR_FILE)).unwrap(),
            cursor_before
        );

        // 3 回目: 書きかけの行の残りと新しい応答を足す → 読むのは読み位置から後ろだけ
        let added = format!(
            "{rest}\n{}\n",
            assistant("msg_e", "s1", T2, OPUS, [0, 1, 0, 0, 1], ALPHA)
        );
        append(&s1, &added);
        let third = env.update(&mut ix);
        assert_eq!(third.read_files, 1);
        assert_eq!(third.bytes_read, (partial_len + added.len()) as u64);
        assert_eq!(third.responses, 2, "msg_c / msg_e");
        let t = env.totals();
        assert_eq!(t[&("s1".to_string(), OPUS.to_string())].1, 4);

        // 4 回目: 新しい索引（再起動）でも状態をファイルから読み直して二重に数えない
        let mut fresh = env.indexer().with_sources(env.sources());
        let again = env.update(&mut fresh);
        assert_eq!((again.bytes_read, again.responses), (0, 0));
    }

    #[test]
    fn 切り詰めと置き換えは数え済みを飛ばして新しい応答だけ足す() {
        let env = Env::new("truncate");
        let s3 = env.transcript("-home-testuser-dev-beta", "s3.jsonl");
        let beta = "/home/testuser/dev/beta";
        let d = assistant("msg_d3", "s3", T1, OPUS, [5, 5, 0, 0, 5], beta);
        let g = assistant("msg_g3", "s3", T1, OPUS, [0, 1000, 0, 0, 0], beta);
        write(&s3, &[d.clone(), g.clone()], "");
        let mut ix = env.indexer().with_sources(env.sources());
        assert_eq!(env.update(&mut ix).responses, 2);
        let before = env.totals()[&("s3".to_string(), OPUS.to_string())];
        // 切り詰め: 数え済みの応答だけを残して短い新しい応答を足す（大きさは前より小さい）
        let f = assistant("msg_f3", "s3", T2, OPUS, [0, 2, 0, 0, 0], beta);
        assert!(f.len() < g.len());
        write(&s3, &[d.clone(), f.clone()], "");
        let out = env.update(&mut ix);
        assert_eq!(out.responses, 1, "msg_d3 は数え済み・msg_f3 だけ");
        let after = env.totals()[&("s3".to_string(), OPUS.to_string())];
        assert_eq!(after.1, before.1 + 1);
        assert_eq!(
            after.0.output,
            before.0.output + 2,
            "数え済みの応答を二重に数えた（消えた msg_g3 の分は使った事実として残す）"
        );
        // 置き換え（別の inode へ rename）: 同じ中身なら何も増えない
        let tmp = s3.with_extension("tmp");
        write(&tmp, &[d, f], "");
        std::fs::rename(&tmp, &s3).unwrap();
        let out = env.update(&mut ix);
        assert_eq!(out.responses, 0);
        assert_eq!(env.totals()[&("s3".to_string(), OPUS.to_string())], after);
    }

    #[test]
    fn 本文の印もcwdも設定dirのパスも索引に出ない() {
        let env = Env::new("privacy");
        fixture(&env);
        let mut ix = env.indexer().with_sources(env.sources());
        env.update(&mut ix);
        let mut seen = 0;
        let mut stack = vec![env.usage()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                seen += 1;
                for needle in [BODY, ALPHA, "/home/testuser", env.cfg().to_str().unwrap()] {
                    assert!(
                        !text.contains(needle),
                        "{} に {needle:?} が出た: 本文・cwd・パスを索引へ書かない（#2201 / 設計書 §6）",
                        path.display()
                    );
                }
            }
        }
        assert!(seen >= 4, "走査が空振りしている（{seen} 個）");
    }

    #[test]
    fn 時間の上限で区切って進み_最後は同じ値に追いつく() {
        let env = Env::new("budget");
        fixture(&env);
        let mut ix = env.indexer().with_sources(env.sources());
        let out = ix
            .update(UpdateOptions {
                budget: Some(Duration::ZERO),
                ..UpdateOptions::default()
            })
            .unwrap();
        assert!(!out.caught_up);
        assert_eq!(out.stopped, Some("budget"));
        assert_eq!(out.pending_files, 3);
        assert!(out.pending_bytes > 0);
        let full = env.update(&mut ix);
        assert!(full.caught_up);
        assert_eq!(full.responses, 4);
        // 他の更新が走っている（ロック中）なら待たずに諦める
        let _held = lock_index(&env.usage().join(CURSOR_FILE), false).unwrap();
        let locked = ix
            .update(UpdateOptions {
                try_lock: true,
                ..UpdateOptions::default()
            })
            .unwrap();
        assert_eq!(locked.stopped, Some("locked"));
    }

    #[test]
    fn カタログが追い出した後も取り込み済みの会話は鍵を保つ() {
        let env = Env::new("evict");
        let catalog = env.root.join("data/sessions.yaml");
        let old = "0000-old-session";
        let detect = |sid: &str, pane: u64| crate::sessions::DetectedSession {
            session_id: sid.to_string(),
            tmux_session: Some(format!("t-{pane}")),
            pane: Some(pane),
            agent_cwd: None,
            model: None,
        };
        let meta = |pane: u64, role: &str| crate::sessions::PaneMetaSnapshot {
            pane,
            tab: 1,
            tmux_session: Some(format!("t-{pane}")),
            role: Some(role.to_string()),
            ..crate::sessions::PaneMetaSnapshot::default()
        };
        crate::sessions::sync_detected_at(
            &catalog,
            &[detect(old, 1)],
            &[meta(
                1,
                "orchestrator-worker:gamma:#2201 S2 の索引 / #322 を参照",
            )],
        )
        .unwrap();
        let s = env.transcript("-home-testuser-dev-alpha", &format!("{old}.jsonl"));
        write(
            &s,
            &[assistant("msg_old", old, T1, OPUS, [1, 1, 0, 0, 1], ALPHA)],
            "",
        );
        let paths = SourcePaths {
            catalog: Some(catalog.clone()),
            ..SourcePaths::default()
        };
        let mut ix = env.indexer().with_source_paths(paths.clone());
        env.update(&mut ix);
        let (keys, _) = read_keys(&env.usage());
        assert_eq!(keys[old].project.as_deref(), Some("gamma"));
        assert_eq!(keys[old].project_from, Some(ProjectFrom::Catalog));
        assert_eq!(keys[old].issue, Some(2201), "ラベルに書いた順で最初の番号");
        assert_eq!(keys[old].role.as_deref(), Some("worker"));
        // 500 件を超える会話を検出させ、本物の GC で古い会話を追い出す
        for chunk in 0..6u64 {
            let detected: Vec<_> = (0..100u64)
                .map(|i| detect(&format!("s-{chunk}-{i:03}"), 100 + chunk * 100 + i))
                .collect();
            let metas: Vec<_> = (0..100u64)
                .map(|i| meta(100 + chunk * 100 + i, "orchestrator-worker:delta:#9"))
                .collect();
            crate::sessions::sync_detected_at(&catalog, &detected, &metas).unwrap();
        }
        let cat = crate::sessions::SessionCatalog::load_from(&catalog).unwrap();
        assert!(cat.entries.len() <= 500, "{}", cat.entries.len());
        assert!(
            !cat.entries.contains_key(old),
            "追い出されていない（検査が空振りする）"
        );
        // 追い出された会話に応答が増えても、写した鍵は悪くならない
        append(
            &s,
            &format!(
                "{}\n",
                assistant("msg_old2", old, T2, OPUS, [1, 1, 0, 0, 1], ALPHA)
            ),
        );
        let out = env.update(&mut ix);
        assert_eq!(out.responses, 1);
        let (keys, _) = read_keys(&env.usage());
        assert_eq!(keys[old].project.as_deref(), Some("gamma"));
        assert_eq!(keys[old].issue, Some(2201));
        assert_eq!(
            keys[old].label.as_deref(),
            Some("#2201 S2 の索引 / #322 を参照")
        );
    }

    #[test]
    fn 読み位置の書き直しは走査に出ないファイルの行を40日は残す() {
        let env = Env::new("cursor-compact");
        fixture(&env);
        let mut ix = env.indexer().with_sources(env.sources());
        env.update(&mut ix);
        // 走査に出ないファイル 2 本（最近まで読んでいた / 50 日前に読んだきり）の行と、
        // 書き直しの閾値を超える数の古い行を足す
        let now = now_ms();
        let gone = |file: &str, at: u64, id: u64| CursorRow {
            file: file.into(),
            dev: 1,
            ino: 2,
            offset: 10,
            last: None,
            ids: vec![core_index::hex64(id)],
            at,
            codex: None,
        };
        let mut rows = vec![
            gone("recent-gone", now - 3_600_000, 0xaaaa),
            gone("old-gone", now - 50 * 24 * 3_600_000, 0xbbbb),
        ];
        rows.extend((0..600).map(|i| gone("recent-gone", now - 3_600_000, 0x1000 + i)));
        append_rows(&env.usage(), &env.usage().join(CURSOR_FILE), &rows).unwrap();
        // 状態を読み直させて更新（書き直しが走る）
        let mut fresh = env.indexer().with_sources(env.sources());
        env.update(&mut fresh);
        let (after, _) = read_all::<CursorRow>(&env.usage().join(CURSOR_FILE));
        assert!(
            after.len() < 10,
            "書き直しが走っていない（{} 行）",
            after.len()
        );
        let recent = after
            .iter()
            .find(|r| r.file == "recent-gone")
            .expect("最近のファイルを落とした");
        assert_eq!(recent.ids.len(), 601, "鍵がまとまっていない");
        assert!(
            !after.iter().any(|r| r.file == "old-gone"),
            "50 日前のファイルが残った"
        );
        // 生きているファイルは 1 行ずつ・数え済みの鍵を保つ（読み直しても増えない）
        let again = env.update(&mut env.indexer().with_sources(env.sources()));
        assert_eq!((again.bytes_read, again.responses), (0, 0));
    }

    #[test]
    fn 月のファイルは育ったら合わせ_古い月は日単位へ_とても古い月は消す() {
        let env = Env::new("months");
        let buckets = env.usage().join(BUCKETS_DIR);
        let now = (now_ms() / 1000) as i64;
        let row = |t: i64| BucketRow {
            t,
            session: "s".into(),
            model: OPUS.into(),
            fast: false,
            tokens: Tokens {
                output: 1,
                ..Tokens::default()
            },
            responses: 1,
        };
        let month_file = |t: i64| buckets.join(format!("{}.jsonl", core_index::month_of(t)));
        // 今の月: 同じ鍵の行が 1 MiB を超えて溜まった
        let cur = core_index::bucket_of(now - 60);
        let many: Vec<BucketRow> = (0..8000).map(|_| row(cur)).collect();
        append_rows(&buckets, &month_file(cur), &many).unwrap();
        assert!(std::fs::metadata(month_file(cur)).unwrap().len() > COMPACT_MIN_BYTES);
        // 35 日より古い月: 同じ日の 2 つの刻み
        let old = core_index::bucket_of(now - 80 * 86_400);
        let old_day = old.div_euclid(86_400) * 86_400;
        append_rows(
            &buckets,
            &month_file(old),
            &[row(old_day + 600), row(old_day + 3600)],
        )
        .unwrap();
        // 365 日より古い月
        let ancient = now - 500 * 86_400;
        append_rows(&buckets, &month_file(ancient), &[row(ancient)]).unwrap();

        let mut ix = env.indexer().with_sources(env.sources());
        env.update(&mut ix);
        let (rows, _) = read_all::<BucketRow>(&month_file(cur));
        assert_eq!(rows.len(), 1, "育った月が合わさっていない");
        assert_eq!(rows[0].responses, 8000);
        let (rows, _) = read_all::<BucketRow>(&month_file(old));
        assert_eq!(rows.len(), 1, "古い月が日単位になっていない");
        assert_eq!(rows[0].t, old_day);
        assert_eq!(rows[0].responses, 2);
        assert!(!month_file(ancient).exists(), "365 日より古い月が残った");
        // 2 回目は覚えで飛ばす（書き直さない）
        let stamp = stamp_of(&month_file(old));
        env.update(&mut ix);
        assert_eq!(stamp_of(&month_file(old)), stamp);
    }

    // --- codex（S5 #2204）-------------------------------------------------------------------------

    const CODEX_ACCOUNT: &str = "codex-account-2204";

    impl Env {
        fn codex_home(&self) -> PathBuf {
            self.root.join("codexhome")
        }
        fn codex_indexer(&self) -> Indexer {
            self.indexer()
                .with_sources(self.sources())
                .with_codex_homes(vec![self.codex_home()])
        }
        fn rollout(&self, sub: &str, name: &str) -> PathBuf {
            let path = self.codex_home().join(sub).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            path
        }
        fn salt(&self) -> String {
            std::fs::read_to_string(self.root.join("data/machine_id"))
                .unwrap()
                .trim()
                .to_string()
        }
    }

    fn iso(secs: i64) -> String {
        crate::diag::format_utc(secs)
    }

    fn codex_meta(id: &str, at: i64) -> String {
        format!(
            r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{t}","cwd":"{ALPHA}/crates","originator":"codex_cli_rs","cli_version":"0.150.1","source":"cli","base_instructions":{{"text":"{BODY} 指示"}},"git":{{"branch":"feat/{BODY}"}}}}}}"#,
            t = iso(at)
        )
    }

    fn codex_turn(at: i64, model: &str) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"turn_context","payload":{{"turn_id":"t","cwd":"{ALPHA}","model":"{model}","effort":"high","user_instructions":"{BODY}"}}}}"#,
            iso(at)
        )
    }

    fn codex_body(at: i64) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{BODY} \"type\":\"token_count\""}}]}}}}"#,
            iso(at)
        )
    }

    /// 累積（入力・読み出し・出力）と週の枠の観測を載せた token_count
    fn codex_tc(at: i64, total: [u64; 3], pct: f64, resets: i64) -> String {
        let [i, c, o] = total;
        format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{i},"cached_input_tokens":{c},"cache_write_input_tokens":0,"output_tokens":{o},"reasoning_output_tokens":0,"total_tokens":{}}},"last_token_usage":{{"input_tokens":1,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":1,"reasoning_output_tokens":0,"total_tokens":2}},"model_context_window":258400}},"rate_limits":{{"limit_id":"codex","limit_name":null,"primary":{{"used_percent":{pct},"window_minutes":10080,"resets_at":{resets}}},"secondary":null,"credits":null,"plan_type":"plus","rate_limit_reached_type":null}}}}}}"#,
            iso(at),
            i + o
        )
    }

    fn codex_rows(env: &Env) -> Vec<LimitObservation> {
        read_all::<LimitObservation>(&env.usage().join(LIMITS_FILE)).0
    }

    // #2395 注入（A/B 用・後で外す）: 2 本の codex テストの順序を固定し、A/B の旧挙動の窓の中で
    // 区間按分のテストの更新を走らせる
    static INJ_2395: (std::sync::Mutex<u8>, std::sync::Condvar) =
        (std::sync::Mutex::new(0), std::sync::Condvar::new());
    fn inj_2395_mark(stage: u8) {
        let mut g = INJ_2395.0.lock().unwrap_or_else(|e| e.into_inner());
        *g = (*g).max(stage);
        INJ_2395.1.notify_all();
    }
    fn inj_2395_wait(stage: u8) {
        let g = INJ_2395.0.lock().unwrap_or_else(|e| e.into_inner());
        let _ = INJ_2395
            .1
            .wait_timeout_while(g, Duration::from_secs(60), |s| *s < stage);
    }

    /// 受け入れ 1（索引の側）: 累積の差分の和が最後の累積に一致し、rate_limits は値ごとにいちばん早い
    /// 観測の 1 行だけが limits.jsonl へ入る。追記分だけを読み、archived_sessions へ移っても数え直さない
    #[test]
    fn codexのrolloutは累積の差分を索引へ_観測をlimitsへ入れる() {
        let env = Env::new("codex-ingest");
        std::fs::create_dir_all(env.codex_home()).unwrap();
        std::fs::write(
            env.codex_home().join("auth.json"),
            format!(
                r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"{BODY}","access_token":"{BODY}","refresh_token":"{BODY}","account_id":"{CODEX_ACCOUNT}"}}}}"#
            ),
        )
        .unwrap();
        let now = (now_ms() / 1000) as i64;
        let start = now - 2 * 86_400;
        let r7 = start + 7 * 86_400;
        let path = env.rollout(
            "sessions/2026/10/16",
            "rollout-2026-10-16T00-00-00-thread-a.jsonl",
        );
        write(
            &path,
            &[
                codex_meta("thread-a", start),
                codex_turn(start + 1, "gpt-5.5"),
                codex_body(start + 2),
                codex_tc(start + 60, [1000, 400, 50], 1.0, r7),
                // 同じ累積の重複行・同じ値の観測（別の会話の報告と同じ）
                codex_tc(start + 61, [1000, 400, 50], 1.0, r7),
                codex_tc(start + 3600, [5000, 3000, 300], 3.0, r7 + 1),
            ],
            "",
        );
        let mut ix = env.codex_indexer();
        let out = env.update(&mut ix);
        assert_eq!((out.files, out.codex_files), (1, 1));
        assert_eq!(out.responses, 2);
        let totals = env.totals();
        let (tokens, n) = totals
            .get(&("thread-a".to_string(), "gpt-5.5".to_string()))
            .copied()
            .unwrap();
        assert_eq!(
            tokens.total(),
            5000 + 300,
            "差の和 = 最後の累積の total_tokens"
        );
        assert_eq!(
            (tokens.input, tokens.cache_read, tokens.output),
            (2000, 3000, 300)
        );
        assert_eq!(n, 2);
        // 鍵: codex・プロジェクト（cwd の最長前方一致）・ログイン = auth.json の account_id の塩付きハッシュ
        let (keys, _) = read_keys(&env.usage());
        let k = keys.get("thread-a").unwrap();
        assert_eq!(k.agent, "codex");
        assert_eq!(k.project.as_deref(), Some("alpha"));
        assert_eq!(k.account.as_deref(), Some("codexhome"));
        let want = tako_core::usage::codex_account_key(&env.salt(), CODEX_ACCOUNT);
        assert_eq!(
            (k.login.as_str(), k.key_from),
            (want.key.as_str(), want.from)
        );
        // 観測: 1.0（解除時刻 r7）と 3.0（r7 + 1）の 2 行。同じ値の 2 回目は書かない
        let rows = codex_rows(&env);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows
            .iter()
            .all(|r| r.source == SOURCE_CODEX && r.login == want.key));
        assert_eq!(rows[0].observed_at, ((start + 60) * 1000) as u64);
        assert_eq!(out.codex_observations, 2);

        // 2 回目: 何も増えない（読み位置・観測の重複除去）
        let again = env.update(&mut env.codex_indexer());
        assert_eq!(
            (again.bytes_read, again.responses, again.codex_observations),
            (0, 0, 0)
        );
        // 追記分だけ: 累積 7000 + 500 → 差 2200
        append(
            &path,
            &(codex_tc(start + 7200, [7000, 4000, 500], 5.0, r7) + "\n"),
        );
        let more = env.update(&mut env.codex_indexer());
        assert_eq!((more.responses, more.codex_observations), (1, 1));
        let (tokens, _) = env.totals()[&("thread-a".to_string(), "gpt-5.5".to_string())];
        assert_eq!(tokens.total(), 7500);
        // archived_sessions へ移った（同じ inode）: 数え直さない。続きの追記は差だけ
        let moved = env.rollout(
            "archived_sessions",
            "rollout-2026-10-16T00-00-00-thread-a.jsonl",
        );
        std::fs::rename(&path, &moved).unwrap();
        let after = env.update(&mut env.codex_indexer());
        assert_eq!((after.files, after.responses), (1, 0));
        append(
            &moved,
            &(codex_tc(start + 7300, [7100, 4000, 510], 5.0, r7) + "\n"),
        );
        env.update(&mut env.codex_indexer());
        let (tokens, _) = env.totals()[&("thread-a".to_string(), "gpt-5.5".to_string())];
        assert_eq!(tokens.total(), 7610);
        // 置き換わった（中身を写した別の inode）: 最初から読み直しても数え済みの累積は数えない
        let text = std::fs::read_to_string(&moved).unwrap();
        std::fs::remove_file(&moved).unwrap();
        std::fs::write(&moved, text).unwrap();
        let replaced = env.update(&mut env.codex_indexer());
        assert_eq!((replaced.responses, replaced.codex_observations), (0, 0));
        assert_eq!(
            env.totals()[&("thread-a".to_string(), "gpt-5.5".to_string())]
                .0
                .total(),
            7610
        );
        // A/B: 旧挙動は codex を読まない。env は立てずに値を渡す（lib のテストは 1 プロセスで並列に走り、
        // 立てた窓に並走する codex のテストの更新まで codex を読まなくなる = #2395。env からの配線は
        // 結合テスト issue2204 の実経路の 1 本の中で見る）
        inj_2395_mark(1);
        inj_2395_wait(2);
        let found = env.codex_indexer().walk_with(true).len();
        assert_eq!(found, 0);
        assert_eq!(env.codex_indexer().walk_with(false).len(), 1);
        // 本文・指示・ツール・cwd・ブランチ・トークンの値が索引のどこにも出ない
        for file in files_under(&env.usage()) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for needle in [BODY, ALPHA, CODEX_ACCOUNT] {
                assert!(!text.contains(needle), "{} に {needle:?}", file.display());
            }
        }
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk_all(dir, &mut out);
        out
    }

    fn walk_all(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_all(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
    }

    /// 受け入れ 1（按分の側）: 過去の rollout の rate_limits だけから、窓ごとに段 + tako の外 = 最新の観測で
    /// 配れる（早く始まった窓は別の窓・ローカルの使用量の無い区間は tako の外）
    #[test]
    fn codexの過去の観測から窓ごとに区間按分できる() {
        let env = Env::new("codex-share");
        let now = (now_ms() / 1000) as i64;
        // 窓 A（20 日前に開始）と、A の解除を待たずに始まった窓 B（17 日前に開始）。
        // token_count はターンの終わりに来るので、時刻は刻みの頭から 50 秒ずらす
        let s_a = core_index::bucket_of(now - 20 * 86_400);
        let r_a = s_a + 7 * 86_400;
        let s_b = s_a + 3 * 86_400;
        let r_b = s_b + 7 * 86_400;
        let alpha = env.rollout("sessions/a", "rollout-a-thread-alpha.jsonl");
        write(
            &alpha,
            &[
                codex_meta("thread-alpha", s_a),
                codex_turn(s_a, "gpt-5.5"),
                codex_tc(s_a + 650, [1000, 0, 0], 2.0, r_a),
                codex_tc(s_a + 1850, [4000, 0, 0], 8.0, r_a + 1),
                codex_tc(s_b + 1250, [7000, 0, 0], 20.0, r_b),
            ],
            "",
        );
        let beta = env.rollout("sessions/b", "rollout-b-thread-beta.jsonl");
        write(
            &beta,
            &[
                format!(
                    r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"id":"thread-beta","timestamp":"{t}","cwd":"/home/testuser/dev/beta","source":"cli"}}}}"#,
                    t = iso(s_b)
                ),
                codex_turn(s_b, "gpt-5.5").replace(ALPHA, "/home/testuser/dev/beta"),
                codex_tc(s_b + 950, [1000, 0, 0], 15.0, r_b),
                codex_tc(s_b + 7200, [1000, 0, 0], 26.0, r_b),
                // 使用量の増えないまま 4% 増えた（区間にローカルの使用量が無い = tako の外）
                codex_tc(s_b + 20_000, [1000, 0, 0], 30.0, r_b),
            ],
            "",
        );
        inj_2395_wait(1);
        env.update(&mut env.codex_indexer());
        inj_2395_mark(2);
        let query = crate::usage::UsageQuery {
            by: Some("project".into()),
            window: Some("all".into()),
            ..Default::default()
        };
        let p = crate::usage::payload_in(&env.usage(), &query, None, now, 0);
        let login = &p["logins"][0];
        assert_eq!(login["agent"], "codex", "{p}");
        let kind = |name: &str| {
            login["usage"]["limit_share"]
                .as_array()
                .unwrap()
                .iter()
                .find(|k| k["kind"] == name)
                .unwrap()
                .clone()
        };
        let ks = kind("seven_day");
        assert_eq!(ks["available"], true, "{p}");
        assert_eq!(ks["scope"], "period");
        // 延べ = 窓 A の 8% + 窓 B の 30%（claude の寄せ方なら 1 つに束ねて 30% しか残らない）
        assert_eq!(ks["observed"].as_f64(), Some(38.0), "{ks}");
        assert_eq!(ks["outside"].as_f64(), Some(4.0), "{ks}");
        // codex は使用量と観測が同じ行に載るので推定しない（窓 A の最後の観測より後の alpha の使用も配らない）
        assert_eq!(ks["estimated"].as_f64(), Some(0.0), "{ks}");
        let share = |name: &str| {
            login["usage"]["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["name"] == name)
                .and_then(|g| g["limit_share"]["seven_day"]["percent"].as_f64())
                .unwrap_or(0.0)
        };
        // 手計算: alpha = 2 + 6（窓 A）+ 5 × 500 / 1333（窓 B の 2 本目の区間）+ 6 = 15.875、
        // beta = 15 + 5 × 833 / 1333 = 18.125（区間に重なる刻みの時間の比 × 生トークン）
        assert!((share("alpha") - 15.875).abs() < 0.01, "{}", share("alpha"));
        assert!((share("beta") - 18.125).abs() < 0.01, "{}", share("beta"));
        assert!((share("alpha") + share("beta") + 4.0 - 38.0).abs() < 0.02);
        // 5h の枠は rollout に無いので % を出さない
        assert_eq!(kind("five_hour")["available"], false);
        let notes = p["notes"].to_string();
        assert!(notes.contains("codex は rollout の token_count"), "{notes}");
        assert!(notes.contains("プランが週の枠だけ"), "{notes}");
    }
}

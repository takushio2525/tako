//! Code Runner の実行環境の検出を `Run` / `RunResolve` へ配線する層（Issue #1730 / #1726 S2）
//!
//! 「この `.py` をどの python で走らせるか」を、プロジェクトの中身（`.venv` / `uv.lock` /
//! `environment.yml` …）から**設定なしで**決める。検出の規則そのもの（表・自動選択の順）は
//! `tako_core::runtime_env` の 1 実装で、ここが持つのは次の 3 つだけ。
//! 設計: `.agent/plans/2026-09-runner-settings.md` §5.3 / §6 / §8.1
//!
//! ## 2 段の検出
//!
//! | 段 | 中身 | 走る経路 |
//! |---|---|---|
//! | Tier F | stat と小さなファイルの先頭読み（`runtime_env::detect`） | `Run` と `RunResolve` の両方。毎回走らせる（数 ms） |
//! | Tier P | 子プロセス（道具の場所・環境の置き場・版・env の一覧） | `RunResolve`（background）と再検出（`refresh`）だけ。**`Run` では起こさない** |
//!
//! Tier P の待ちは **`tako_core::probe::output_with_timeout` の 1 実装だけ**を通す
//! （実行ファイルの探索も上限つきの `platform::exe::find_with_timeout` = 同じ待ち）。
//! 上限は `probe::runtime_probe_timeout`（既定 5 秒・env で値だけ変えられる）。
//! 打ち切ったら `timeouts` に載せて応答へ出す（無言にしない = #1503）。
//!
//! ## Tier P の答えは「事実」として覚え、`Run` はそれを重ねるだけ
//!
//! 答え（見つかった道具・env の一覧・環境の置き場・版）はファイル（開始ディレクトリ）ごとに
//! プロセス内へ覚え、`Run` は **Tier F を毎回回したうえで**覚えた事実を重ねる。
//! Tier F の結果（`.venv` を作った・消した）を覚えないので、ファイルシステムの変化は
//! 次の実行ですぐ効く。事実の側はそれぞれ自分の指紋で古さを見る
//! （道具 = そのファイルがまだ在るか / 環境の置き場 = 印のファイル（`poetry.lock` 等）の
//! mtime の組 / 版 = interpreter の mtime）。永続化はしない。
//!
//! ## 言語の名前を書かない
//!
//! 何を聞くかは表から決まる（`runtime_env::tier_p`）。ここへ言語・道具の名前を書くと
//! 「言語を足す = 表に行を足すだけ」が崩れるので、番犬
//! `crates/tako-control/tests/issue1729_runtime_env_table_watchdog.rs` がこのファイルも見る。
//!
//! ## A/B
//!
//! `TAKO_1730_LEGACY=1` で実行環境の層を丸ごと通さない（`${python}` は表の `fallback` =
//! #1730 以前の `python3` / `python`。PATH・env の前置も無し）。ゲートは [`legacy_1730`] の 1 か所

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};
use tako_core::platform::exe::BoundedFind;
use tako_core::platform::support::Platform;
use tako_core::probe::{Outcome, TimeoutNotice};
use tako_core::project_root::{self, SearchBounds};
use tako_core::runner::{RunPlan, RuntimeWords};
use tako_core::runner_config::{self, Declared, EffectiveConfig, RunConfigField, Sourced};
use tako_core::runtime_env::{
    self, tier_p, Candidate, DetectEnv, Detection, FsProbe, RealFs, RuntimeKind,
};

/// #1730 以前（実行環境の層を通さない）へ戻す A/B のゲート。**ここ 1 か所だけ**
pub fn legacy_1730() -> bool {
    std::env::var("TAKO_1730_LEGACY").is_ok_and(|v| v == "1")
}

/// 候補を確かめた段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// ファイルシステムだけ
    F,
    /// 子プロセスの答えを重ねた
    P,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F => "F",
            Self::P => "P",
        }
    }
}

/// 検出の完成品（`Run` の実行と、`Run` / `RunResolve` の応答が同じものを読む）
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub kind: &'static RuntimeKind,
    pub detection: Detection,
    /// Tier F だけの結果（候補ごとの `tier` を決める比較の相手）
    tier_f_only: Detection,
    /// この回に子プロセスを起こさなかった（覚えていた事実だけで済んだ）
    pub cached: bool,
    pub tier_f_elapsed: Duration,
    /// Tier P を走らせた時間（走らせなかったら `None`）
    pub tier_p_elapsed: Option<Duration>,
    pub timeouts: Vec<TimeoutNotice>,
}

impl Snapshot {
    /// 走らせる候補（S2 は自動選択。保存した選択は #1726 S3）
    pub fn selected(&self) -> Option<&Candidate> {
        self.detection.auto_candidate()
    }

    /// `resolve_file` へ渡す `${<variable>}` の値
    pub fn words(&self) -> RuntimeWords {
        let mut out = RuntimeWords::new();
        if let Some(c) = self.selected() {
            out.insert(self.kind.variable.to_string(), c.applied.program.clone());
        }
        out
    }

    /// 実行ペインのコマンドの前に付ける env
    pub fn env(&self) -> Vec<(String, String)> {
        self.selected()
            .map(|c| c.applied.env.clone())
            .unwrap_or_default()
    }

    /// 実行ペインのコマンドの前に付ける PATH（文字列にしたもの）
    pub fn path_prepend(&self) -> Vec<String> {
        self.selected()
            .map(|c| {
                c.applied
                    .path_prepend
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 候補が Tier P の答えで変わったか
    fn tier_of(&self, c: &Candidate) -> Tier {
        let same = self
            .tier_f_only
            .candidates
            .iter()
            .any(|f| f.manager == c.manager && f.key == c.key && f == c);
        if same {
            Tier::F
        } else {
            Tier::P
        }
    }
}

// ─── 問い合わせの口（テストは偽物を渡す） ─────────────────────────────

/// Tier P の問い合わせ。**待ちは上限つきの 1 実装だけ**を通す
pub trait Prober: Sync {
    /// 実行ファイルの場所（unix = ログインシェルの `command -v` / Windows = PATH の走査）
    fn find(&self, name: &str, budget: Duration) -> BoundedFind;
    /// コマンドを起こして出力を待つ
    fn output(&self, program: &str, args: &[&str], cwd: Option<&Path>, budget: Duration)
        -> Outcome;
}

/// 本物の問い合わせ
pub struct RealProber;

impl Prober for RealProber {
    fn find(&self, name: &str, budget: Duration) -> BoundedFind {
        tako_core::platform::exe::find_with_timeout(name, budget)
    }

    fn output(
        &self,
        program: &str,
        args: &[&str],
        cwd: Option<&Path>,
        budget: Duration,
    ) -> Outcome {
        tako_core::probe::output_with_timeout_in(program, args, cwd, budget)
    }
}

/// テストプロセスでは実物の道具を起こさない（空 HOME でも道具が自分のホームを作る =
/// `agent_probe` と同じ判断。Tier P の検査は偽物の口を渡して行う）
struct BlockedProber;

impl Prober for BlockedProber {
    fn find(&self, _name: &str, _budget: Duration) -> BoundedFind {
        BoundedFind::default()
    }

    fn output(&self, program: &str, args: &[&str], _: Option<&Path>, _: Duration) -> Outcome {
        Outcome::Failed {
            label: tako_core::probe::label(program, args),
            reason: "test process".into(),
        }
    }
}

fn default_prober() -> &'static dyn Prober {
    if tako_core::paths::is_test_process() {
        &BlockedProber
    } else {
        &RealProber
    }
}

// ─── 覚えておく事実 ────────────────────────────────────────────────────

/// 指紋（パス → mtime。無いファイルは `None`）
type Stamp = Vec<(PathBuf, Option<SystemTime>)>;

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn stamp_of(paths: &[PathBuf]) -> Stamp {
    paths.iter().map(|p| (p.clone(), mtime(p))).collect()
}

fn stamp_valid(stamp: &Stamp) -> bool {
    stamp.iter().all(|(p, m)| mtime(p) == *m)
}

#[derive(Debug, Clone)]
struct EnvFact {
    manager: String,
    cwd: PathBuf,
    stamp: Stamp,
    /// 聞いた答え（`None` = 聞いたが分からなかった）
    dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct VersionFact {
    program: String,
    stamp: Option<SystemTime>,
    version: Option<String>,
}

/// Tier P の答え（ファイルの開始ディレクトリごと）
#[derive(Debug, Clone, Default)]
struct Facts {
    /// 答えた問い（場所の探索と一覧）。同じ問いなら聞き直さない
    plan: Option<tier_p::Plan>,
    /// 名前 → 見つかった実行ファイル（`None` = 聞いたが無い）
    found: Vec<(String, Option<PathBuf>)>,
    /// 一覧ファイルの代わり
    lists: Vec<(PathBuf, Vec<String>)>,
    envs: Vec<EnvFact>,
    versions: Vec<VersionFact>,
}

impl Facts {
    fn is_empty(&self) -> bool {
        self.found.is_empty()
            && self.lists.is_empty()
            && self.envs.is_empty()
            && self.versions.is_empty()
    }
}

/// 覚えておく開始ディレクトリの上限（超えたら全部捨てる。捨てても次の一覧で聞き直すだけ）
const CACHE_MAX: usize = 256;

type CacheKey = (&'static str, PathBuf);

fn cache() -> &'static Mutex<HashMap<CacheKey, Facts>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Facts>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_facts(key: &CacheKey) -> Option<Facts> {
    cache().lock().ok().and_then(|c| c.get(key).cloned())
}

fn store_facts(key: CacheKey, facts: Facts) {
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_MAX && !c.contains_key(&key) {
            c.clear();
        }
        c.insert(key, facts);
    }
}

// ─── 入口 ──────────────────────────────────────────────────────────────

/// 検出の材料（どの kind・どの段を見るか）
struct Scope {
    kind: &'static RuntimeKind,
    platform: Platform,
    dirs: Vec<PathBuf>,
    env: DetectEnv,
    key: CacheKey,
}

fn scope_for(file: &Path) -> Option<Scope> {
    let ext = file.extension()?.to_string_lossy().to_ascii_lowercase();
    let kind = runtime_env::kind_for_ext(&ext)?;
    let start = file.parent()?;
    let dirs = project_root::candidate_dirs(start, &SearchBounds::for_user());
    Some(Scope {
        kind,
        platform: Platform::current(),
        dirs,
        env: DetectEnv::from_process(),
        key: (kind.id, start.to_path_buf()),
    })
}

/// `Run` の経路: Tier F + 覚えている Tier P の事実。**子プロセスは起こさない**。
/// 実行環境の kind を持たない拡張子・A/B 中は `None`
pub fn for_run(file: &Path) -> Option<Snapshot> {
    if legacy_1730() {
        return None;
    }
    let scope = scope_for(file)?;
    let mut facts = cached_facts(&scope.key).unwrap_or_default();
    Some(detect_with(
        scope.kind,
        scope.platform,
        &scope.dirs,
        &RealFs,
        &scope.env,
        &mut facts,
        None,
    ))
}

/// 一覧（`RunResolve`）の経路: 足りない Tier P の事実だけ聞いて覚える。
/// `refresh` は覚えた事実を捨てて全部聞き直す（再検出）。**UI スレッドで呼ばない**
/// （dispatch は `prepare_offload` で background へ出す）
pub fn for_resolve(file: &Path, refresh: bool) -> Option<Snapshot> {
    if legacy_1730() {
        return None;
    }
    let scope = scope_for(file)?;
    let mut facts = if refresh {
        Facts::default()
    } else {
        cached_facts(&scope.key).unwrap_or_default()
    };
    let snap = detect_with(
        scope.kind,
        scope.platform,
        &scope.dirs,
        &RealFs,
        &scope.env,
        &mut facts,
        Some((default_prober(), tako_core::probe::runtime_probe_timeout())),
    );
    store_facts(scope.key, facts);
    Some(snap)
}

/// 検出の核（I/O と問い合わせの口を外から受ける = 偽のファイルシステムで検査できる）。
///
/// `probe` が `None` なら覚えた事実を重ねるだけ（`Run`）。`Some` なら足りない事実を聞いて
/// `facts` へ足す（`RunResolve`）
fn detect_with(
    kind: &'static RuntimeKind,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
    facts: &mut Facts,
    probe: Option<(&dyn Prober, Duration)>,
) -> Snapshot {
    let started = Instant::now();
    let tier_f_only = tier_f(kind, platform, dirs, fs, env);
    let tier_f_elapsed = started.elapsed();

    let p_started = Instant::now();
    let mut timeouts = Vec::new();
    let mut asked = false;

    // 1. 場所の探索と一覧（問いが変わっていなければ聞き直さない）
    let plan = tier_p::plan(kind, platform, &tier_f_only, fs, env);
    if let Some((prober, budget)) = probe {
        if facts.plan.as_ref() != Some(&plan) {
            asked |= !plan.lookups.is_empty() || !plan.lists.is_empty();
            ask_lookups(facts, &plan, prober, budget, &mut timeouts);
        }
    }
    let found: Vec<PathBuf> = facts
        .found
        .iter()
        .filter_map(|(_, p)| p.clone())
        .filter(|p| fs.is_file(p))
        .collect();
    let env_p = tier_p::env_with_found(env, &found);
    let mut overlay = tier_p::OverlayFs::new(fs);
    for (list_file, prefixes) in &facts.lists {
        if !fs.is_file(list_file) {
            overlay = overlay.with_list(list_file.clone(), prefixes);
        }
    }
    let mut detection = if found.is_empty() && overlay.is_empty() {
        tier_f_only.clone()
    } else {
        tier_f(kind, platform, dirs, &overlay, &env_p)
    };

    // 2. 包む形の環境の置き場（印のファイルの mtime が変わっていなければ覚えた答え）
    for q in tier_p::env_queries(kind, &detection) {
        let c = &detection.candidates[q.index];
        let stamp = stamp_of(&tier_p::env_stamp_files(kind, c));
        let known = facts
            .envs
            .iter()
            .position(|e| e.manager == c.manager && e.cwd == q.cwd && e.stamp == stamp)
            .filter(|i| stamp_valid(&facts.envs[*i].stamp));
        let dir = match (known, probe) {
            (Some(i), _) => facts.envs[i].dir.clone(),
            (None, Some((prober, budget))) => {
                asked = true;
                let out = prober.output(&q.program, q.args, Some(&q.cwd), budget);
                timeouts.extend(out.timeout_notice());
                let dir = out
                    .into_output()
                    .filter(|o| o.status.success())
                    .and_then(|o| first_line(&o.stdout))
                    .map(PathBuf::from);
                facts
                    .envs
                    .retain(|e| !(e.manager == c.manager && e.cwd == q.cwd));
                facts.envs.push(EnvFact {
                    manager: c.manager.to_string(),
                    cwd: q.cwd.clone(),
                    stamp,
                    dir: dir.clone(),
                });
                dir
            }
            (None, None) => None,
        };
        if let Some(dir) = dir.filter(|d| fs.is_dir(d)) {
            runtime_env::apply_probed_env(kind, platform, &mut detection.candidates[q.index], &dir);
        }
    }

    // 3. 版（interpreter の mtime が変わっていなければ覚えた答え）
    for q in tier_p::version_queries(kind, &detection, &overlay) {
        let stamp = mtime(Path::new(&q.program));
        let known = facts
            .versions
            .iter()
            .find(|v| v.program == q.program && v.stamp == stamp)
            .map(|v| v.version.clone());
        let version = match (known, probe) {
            (Some(v), _) => v,
            (None, Some((prober, budget))) => {
                asked = true;
                let out = prober.output(&q.program, q.args, None, budget);
                timeouts.extend(out.timeout_notice());
                // 2 系は版を stderr へ出すので両方を見る
                let version = out.into_output().and_then(|o| {
                    tier_p::parse_version(&String::from_utf8_lossy(&o.stdout))
                        .or_else(|| tier_p::parse_version(&String::from_utf8_lossy(&o.stderr)))
                });
                facts.versions.retain(|v| v.program != q.program);
                facts.versions.push(VersionFact {
                    program: q.program.clone(),
                    stamp,
                    version: version.clone(),
                });
                version
            }
            (None, None) => None,
        };
        if version.is_some() {
            detection.candidates[q.index].version = version;
        }
    }

    let tier_p_elapsed = asked.then(|| p_started.elapsed());
    let cached = !asked && !facts.is_empty();
    Snapshot {
        kind,
        detection,
        tier_f_only,
        cached,
        tier_f_elapsed,
        tier_p_elapsed,
        timeouts,
    }
}

fn tier_f(
    kind: &RuntimeKind,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
) -> Detection {
    let mut d = runtime_env::detect(kind, platform, dirs, fs, env);
    d.demote_missing_interpreters(fs);
    d
}

fn ask_lookups(
    facts: &mut Facts,
    plan: &tier_p::Plan,
    prober: &dyn Prober,
    budget: Duration,
    timeouts: &mut Vec<TimeoutNotice>,
) {
    facts.found.clear();
    facts.lists.clear();
    for name in &plan.lookups {
        let r = prober.find(name, budget);
        timeouts.extend(r.timeout);
        facts.found.push((name.clone(), r.path.map(PathBuf::from)));
    }
    for q in &plan.lists {
        let Some(program) = facts
            .found
            .iter()
            .find(|(n, _)| *n == q.program)
            .and_then(|(_, p)| p.clone())
        else {
            continue;
        };
        let out = prober.output(&program.to_string_lossy(), q.args, None, budget);
        timeouts.extend(out.timeout_notice());
        let prefixes = out
            .into_output()
            .filter(|o| o.status.success())
            .map(|o| tier_p::parse_list(q.json_key, &String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        if !prefixes.is_empty() {
            facts.lists.push((q.list_file.clone(), prefixes));
        }
    }
    facts.plan = Some(plan.clone());
}

/// 出力の最初の空でない行（道具が返す置き場）
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

// ─── 応答 ──────────────────────────────────────────────────────────────

/// 候補の保存用の ID（`<manager>:<key>`。#1726 S3 の `--runtime` が受ける形）
fn candidate_id(c: &Candidate) -> String {
    format!("{}:{}", c.manager, c.key)
}

fn candidate_json(snap: &Snapshot, index: usize, c: &Candidate) -> Value {
    let auto = snap.detection.auto == Some(index);
    json!({
        "id": candidate_id(c),
        "kind": c.kind,
        "manager": c.manager,
        "key": c.key,
        "label": c.label,
        "version": c.version,
        "path": c.location.as_ref().map(|p| p.display().to_string()),
        "program": c.applied.program,
        "auto": auto,
        // S2 は自動で選んだものが走る（保存した選択が勝つのは #1726 S3）
        "selected": auto,
        "tier": snap.tier_of(c).as_str(),
        "needs_probe": c.needs_probe,
    })
}

/// `RunResolve` の `runtimes`（候補の全部。自動選択の順）
pub fn runtimes_json(snap: Option<&Snapshot>) -> Value {
    let Some(snap) = snap else {
        return json!([]);
    };
    Value::Array(
        snap.detection
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| candidate_json(snap, i, c))
            .collect(),
    )
}

/// `Run` / `RunResolve` の `runtime`（走らせる 1 つ + 実行ペインへ前置するもの）
pub fn runtime_json(snap: Option<&Snapshot>) -> Value {
    let Some(snap) = snap else {
        return Value::Null;
    };
    let Some((index, c)) = snap
        .detection
        .auto
        .and_then(|i| snap.detection.candidates.get(i).map(|c| (i, c)))
    else {
        return Value::Null;
    };
    let mut v = candidate_json(snap, index, c);
    if let Value::Object(map) = &mut v {
        map.insert("source".into(), json!("auto"));
        map.insert("path_prepend".into(), json!(snap.path_prepend()));
        let env: Map<String, Value> = snap
            .env()
            .into_iter()
            .map(|(k, val)| (k, Value::String(val)))
            .collect();
        map.insert("env".into(), Value::Object(env));
    }
    v
}

/// 検出の所要と打ち切り（`RunResolve` の `probe`。Tier F の目標は 50 ms 以内）
pub fn probe_json(snap: Option<&Snapshot>) -> Value {
    let Some(snap) = snap else {
        return Value::Null;
    };
    let ms = |d: Duration| (d.as_secs_f64() * 1000.0 * 100.0).round() / 100.0;
    json!({
        "tier_f_ms": ms(snap.tier_f_elapsed),
        "tier_p_ms": snap.tier_p_elapsed.map(ms),
        "cached": snap.cached,
        "timeouts": snap.timeouts.iter().map(|t| json!({
            "label": t.label,
            "waited_secs": t.waited_secs,
        })).collect::<Vec<_>>(),
    })
}

/// 人へ出す注意（検出の注意 + 包む形が効かない宣言への案内 + 打ち切りの知らせ）
pub fn warnings(snap: Option<&Snapshot>, plan: &RunPlan) -> Vec<String> {
    let Some(snap) = snap else {
        return Vec::new();
    };
    let mut out = snap.detection.warnings.clone();
    if let Some(c) = snap.selected() {
        let wrapped = c.applied.program.len() > 1;
        if wrapped && c.applied.path_prepend.is_empty() && !plan.uses_variable(snap.kind.variable) {
            out.push(msg_wrapper_not_applied(&c.label, snap.kind.variable));
        }
    }
    out.extend(snap.timeouts.iter().map(|t| t.to_string()));
    out
}

/// 実行設定の実効値（項目ごとの `{ value, source }`。S2 は保存した設定が無いので
/// 宣言の作業ディレクトリと自動の実行環境だけが既定以外になる）
pub fn effective_config(snap: Option<&Snapshot>, declared_cwd: Option<&str>) -> EffectiveConfig {
    let mut auto = std::collections::BTreeMap::new();
    if let Some((snap, c)) = snap.and_then(|s| s.selected().map(|c| (s, c))) {
        auto.insert(snap.kind.id.to_string(), c.reference());
    }
    runner_config::merge(
        None,
        None,
        &Declared {
            cwd: declared_cwd.map(str::to_string),
        },
        &auto,
    )
}

/// [`effective_config`] の JSON（`RunResolve` の `config`）
pub fn config_json(eff: &EffectiveConfig) -> Value {
    fn scalar(v: &Option<Sourced<String>>) -> Value {
        match v {
            Some(s) => json!({ "value": s.value, "source": s.source.as_str() }),
            None => json!({ "value": null, "source": "default" }),
        }
    }
    let mut out = Map::new();
    for field in RunConfigField::ALL {
        let v = match field {
            RunConfigField::Profile => scalar(&eff.profile),
            RunConfigField::Args => scalar(&eff.args),
            RunConfigField::Cwd => scalar(&eff.cwd),
            RunConfigField::Before => scalar(&eff.before),
            RunConfigField::Runtime => Value::Object(
                eff.runtime
                    .iter()
                    .map(|(kind, s)| {
                        (
                            kind.clone(),
                            json!({
                                "value": { "manager": s.value.manager, "key": s.value.key },
                                "source": s.source.as_str(),
                            }),
                        )
                    })
                    .collect(),
            ),
            RunConfigField::Env => Value::Object(
                eff.env
                    .iter()
                    .map(|(k, s)| {
                        (
                            k.clone(),
                            json!({ "value": s.value, "source": s.source.as_str() }),
                        )
                    })
                    .collect(),
            ),
        };
        out.insert(field.as_str().to_string(), v);
    }
    Value::Object(out)
}

/// 項目ごとの出典だけ（`Run` の `config_sources`）
pub fn config_sources_json(eff: &EffectiveConfig) -> Value {
    Value::Object(
        RunConfigField::ALL
            .iter()
            .map(|f| (f.as_str().to_string(), json!(eff.source_of(*f).as_str())))
            .collect(),
    )
}

fn msg_wrapper_not_applied(label: &str, variable: &str) -> String {
    match tako_core::i18n::lang() {
        tako_core::i18n::Lang::Ja => format!(
            "{label} はこの宣言には効かない（コマンドが実行環境の変数を使っていない）。\
             `${{{variable}}} -m <モジュール>` の形で書くと効く"
        ),
        tako_core::i18n::Lang::En => format!(
            "{label} does not apply to this declaration (the command does not use the runtime \
             variable). Write it as `${{{variable}}} -m <module>` to use the environment"
        ),
    }
}

#[cfg(test)]
mod tests;

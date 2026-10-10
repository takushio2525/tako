//! 未導入の言語サーバを tako の data dir へ取ってくる（#1944）
//!
//! 「どこから・どの版を・どのハッシュで」は `tako_core::lsp::fetch`（検出表の各行の [`Fetch`]）、
//! ここはダウンロード → ハッシュの検証 → 展開 → 置き場への確定だけを持つ。
//!
//! ## 守ること
//!
//! - **利用者の環境を汚さない**: npm / brew を起こさない・PATH へ足さない。置くのは
//!   `<data_dir>/lsp-servers/` の下だけ（`TAKO_DATA_DIR` の隔離に従う）
//! - **検証してから使う**: 取得物全体のハッシュを配布元の公開値（表に固定）と突き合わせ、合わなければ
//!   捨てる。ミラー（`TAKO_LSP_FETCH_BASE`）を通しても同じ値で検証する
//! - **途中の物を使わない**: 一時の段へ展開して印（`.tako-installed`）を書いてから版の段へ rename する。
//!   印の無い段は使わない（途中で落ちた・ディスクが尽きた跡を「入っている」と読まない）
//! - **同じものを 2 回取らない**: 置き場ごとの錠で、同じサーバを 2 つのペインが同時に求めても
//!   1 本だけが取り、他は待って結果を共有する（別プロセス = GUI と CLI が重なったら rename の
//!   勝った側を使う）
//! - **UI スレッドで呼ばない**: どれも待ちうる。manager の起動のスレッドと dispatch の offload から呼ぶ

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::Digest as _;
use tako_core::lsp::fetch::{self as table, Archive, Digest, Fetch, NodeRuntime, OwnedAsset};
use tako_core::lsp::servers::ServerSpec;
use tako_core::platform::child_cmd::{self, ChildCmd};

use super::archive::{self, ArchiveError};
use super::text;

/// `TAKO_1944_LEGACY=1` で取得をまるごと止める（#1678 の「案内だけ」へ戻す A/B の入口）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_1944_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 接続の上限（DNS・TCP・TLS）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 応答のヘッダが来るまでの上限
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// 中身を受け取り切るまでの上限（遅い回線で 50 MB を受ける余裕を見る）
const BODY_TIMEOUT: Duration = Duration::from_secs(600);
/// 進捗の知らせの間隔（UI の描き直しを詰まらせない）
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// 利用者の PATH に足りる Node.js があればそのパス（`node_major` 以上）
pub type NodeLookup = Arc<dyn Fn(u32) -> Option<String> + Send + Sync>;

/// 取得の設定
#[derive(Clone)]
pub struct FetchConfig {
    /// 置き場の根（`<data_dir>/lsp-servers`）
    pub root: PathBuf,
    /// ミラー（`TAKO_LSP_FETCH_BASE`。`https://<host>/…` を `<base>/<host>/…` へ替える）
    pub base: Option<String>,
    /// 文書を開いた時点で取りに行くか（偽なら `tako lsp install` / 画面の「入れる」だけで取る）
    pub auto: bool,
    /// 利用者の Node.js を探す口（本番はログインシェル + `node --version`）
    pub node_lookup: NodeLookup,
    /// 取る OS / CPU（`std::env::consts` の綴り。検証で他の組を選ばせる口）
    pub os: &'static str,
    pub arch: &'static str,
}

impl FetchConfig {
    /// 本番の設定。data dir が決まらない・`TAKO_1944_LEGACY=1` なら `None`（取らない）
    pub fn from_env() -> Option<Self> {
        if legacy() {
            return None;
        }
        let root = tako_core::paths::data_dir()?.join(table::ROOT_DIR_NAME);
        Some(Self {
            root,
            base: std::env::var("TAKO_LSP_FETCH_BASE")
                .ok()
                .filter(|b| !b.trim().is_empty()),
            auto: auto_from_env(),
            node_lookup: Arc::new(default_node_lookup),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        })
    }

    /// テスト・検証用: 置き場とミラーを指定して作る（Node.js は探さない）
    pub fn with_root(root: PathBuf, base: Option<String>) -> Self {
        Self {
            root,
            base,
            auto: true,
            node_lookup: Arc::new(|_| None),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    /// その行を取れるか（この OS / CPU 向けの取得物がある）
    pub fn can_fetch(&self, spec: &ServerSpec) -> bool {
        spec.fetch
            .is_some_and(|f| f.available_for(self.os, self.arch))
    }
}

/// 開いた時点で取りに行くか。既定は取りに行く。`TAKO_LSP_AUTO_FETCH=0` で止め、セルフテスト・
/// visual-test（`TAKO_SELF_TEST` / `TAKO_VISUAL_TEST`）は明示（`=1`）が無ければ取らない
/// （検証のたびに配布元から取り直さない）
pub fn auto_from_env() -> bool {
    auto_from(
        std::env::var("TAKO_LSP_AUTO_FETCH").ok().as_deref(),
        std::env::var_os("TAKO_SELF_TEST").is_some()
            || std::env::var_os("TAKO_VISUAL_TEST").is_some(),
    )
}

/// [`auto_from_env`] の判定（純粋関数）。`explicit` = `TAKO_LSP_AUTO_FETCH` の値
pub fn auto_from(explicit: Option<&str>, self_test: bool) -> bool {
    match explicit.map(str::trim) {
        Some("0" | "false" | "off") => false,
        Some("1" | "true" | "on") => true,
        _ => !self_test,
    }
}

/// 本番の Node.js の探し方: ログインシェルの PATH（`exe::find_with_timeout`）→ `node --version` で
/// major を読み、`node_major` 以上なら採る（古い版で起こしてサーバが黙って落ちる形を作らない）
pub fn default_node_lookup(node_major: u32) -> Option<String> {
    let budget = tako_core::probe::probe_timeout();
    let found = tako_core::platform::exe::find_with_timeout("node", budget);
    let path = found.path?;
    match tako_core::probe::output_with_timeout(&path, &["--version"], budget) {
        tako_core::probe::Outcome::Done { status, stdout, .. } if status.success() => {
            let major = table::node_major_of(&String::from_utf8_lossy(&stdout))?;
            (major >= node_major).then_some(path)
        }
        _ => None,
    }
}

/// 取得の失敗（理由の文は [`FetchError::reason`]。日英は `lsp::text`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// この OS / CPU 向けの取得物が表に無い
    Unsupported,
    /// 届かない（DNS・接続・TLS・途中で切れた）
    Network { item: String, detail: String },
    /// 配布元が 200 以外を返した
    Http { item: String, status: u16 },
    /// ハッシュが固定値と合わない（捨てた）
    Digest { item: String },
    /// 想定の大きさを大きく超えた（打ち切った）
    TooLarge { item: String, limit: u64 },
    /// 書き込めない（ディスクの空き・権限）
    Io { item: String, detail: String },
    /// 展開できない
    Archive { item: String, detail: String },
}

impl FetchError {
    /// 機械可読の綴り（`tako lsp status` の `fetch.error_kind`）
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Network { .. } => "network",
            Self::Http { .. } => "http",
            Self::Digest { .. } => "digest",
            Self::TooLarge { .. } => "too_large",
            Self::Io { .. } => "io",
            Self::Archive { .. } => "archive",
        }
    }

    /// 理由の文（利用者に見せる。ソースコードの本文は含まない）
    pub fn reason(&self) -> String {
        match self {
            Self::Unsupported => text::FETCH_UNSUPPORTED_REASON.text().to_string(),
            Self::Network { item, detail } => text::fill(
                text::FETCH_NETWORK_REASON,
                &[("item", item), ("detail", detail)],
            ),
            Self::Http { item, status } => text::fill(
                text::FETCH_HTTP_REASON,
                &[("item", item), ("status", &status.to_string())],
            ),
            Self::Digest { item } => text::fill(text::FETCH_DIGEST_REASON, &[("item", item)]),
            Self::TooLarge { item, limit } => text::fill(
                text::FETCH_TOO_LARGE_REASON,
                &[("item", item), ("limit", &format_mb(*limit))],
            ),
            Self::Io { item, detail } => {
                text::fill(text::FETCH_IO_REASON, &[("item", item), ("detail", detail)])
            }
            Self::Archive { item, detail } => text::fill(
                text::FETCH_ARCHIVE_REASON,
                &[("item", item), ("detail", detail)],
            ),
        }
    }
}

/// `12.3 MB`（10 進。配布元の表記と同じ桁）
pub fn format_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// 取得の段階
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Verifying,
    Extracting,
}

impl Phase {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Extracting => "extracting",
        }
    }
}

/// 取っている最中の様子（UI の 1 行と `tako lsp status` の `fetch` が読む）
#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    /// いま取っているもの（`Node.js 24.21.0` / `pyright 1.1.414`）
    pub item: String,
    pub phase: Phase,
    /// 何個目 / 全部で何個（Node.js を取るなら 1 つ増える）
    pub step: usize,
    pub steps: usize,
    pub done: u64,
    pub total: u64,
    pub elapsed: Duration,
}

impl ProgressSnapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "item": self.item,
            "phase": self.phase.slug(),
            "step": self.step,
            "steps": self.steps,
            "bytes": self.done,
            "total_bytes": self.total,
            "elapsed_ms": self.elapsed.as_millis() as u64,
        })
    }
}

struct Progress {
    started: Instant,
    state: Mutex<ProgressSnapshot>,
}

impl Progress {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            state: Mutex::new(ProgressSnapshot {
                item: String::new(),
                phase: Phase::Downloading,
                step: 0,
                steps: 0,
                done: 0,
                total: 0,
                elapsed: Duration::ZERO,
            }),
        }
    }

    fn update(&self, f: impl FnOnce(&mut ProgressSnapshot)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut state);
    }

    fn snapshot(&self) -> ProgressSnapshot {
        let mut snapshot = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        snapshot.elapsed = self.started.elapsed();
        snapshot
    }
}

/// 置き場ごとの錠と進捗（プロセス全体で 1 つ。同じ置き場を 2 本が同時に取らない）
#[derive(Default)]
struct Registry {
    locks: HashMap<PathBuf, Arc<Mutex<()>>>,
    progress: HashMap<PathBuf, Arc<Progress>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn lock_for(dir: &Path) -> Arc<Mutex<()>> {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(reg.locks.entry(dir.to_path_buf()).or_default())
}

/// 取っている最中ならその様子（取っていなければ `None`）。**待たない**
pub fn progress_of(config: &FetchConfig, spec: &ServerSpec) -> Option<ProgressSnapshot> {
    let fetch = spec.fetch.as_ref()?;
    let dir = table::install_dir(&config.root, spec.id, fetch);
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    reg.progress.get(&dir).map(|p| p.snapshot())
}

/// 取っている最中の印（落とすと進捗を消す）
struct ProgressGuard {
    dir: PathBuf,
    progress: Arc<Progress>,
}

impl ProgressGuard {
    fn begin(dir: &Path) -> Self {
        let progress = Arc::new(Progress::new());
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .insert(dir.to_path_buf(), Arc::clone(&progress));
        Self {
            dir: dir.to_path_buf(),
            progress,
        }
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        if reg
            .progress
            .get(&self.dir)
            .is_some_and(|p| Arc::ptr_eq(p, &self.progress))
        {
            reg.progress.remove(&self.dir);
        }
    }
}

/// 置き場に済んだもの（起こし方）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    pub plan: ChildCmd,
    /// 表示用（Binary = 実行ファイル、Npm = 入口の .js）
    pub program_path: String,
    /// Node.js で起こすときの Node.js（表示用）
    pub runtime: Option<String>,
    /// Node.js をどこから採ったか（`path` = 利用者の PATH / `managed` = tako が取った）
    pub runtime_source: Option<&'static str>,
    /// 版の段
    pub dir: PathBuf,
}

/// 置き場に済んでいるか（印を見るだけ。**ネットワークに出ない**。Npm は Node.js を探すので待ちうる）
pub fn installed(config: &FetchConfig, spec: &ServerSpec) -> Option<Managed> {
    let fetch = spec.fetch.as_ref()?;
    let dir = table::install_dir(&config.root, spec.id, fetch);
    if !dir.join(table::MARKER).is_file() {
        return None;
    }
    launch_plan(config, spec, fetch, &dir, None)
}

/// 置き場に済んでいればその起こす物のパス（Binary = 実行ファイル、Npm = 入口の .js）と版の段。
/// **待たない**（印の stat 1 回。Node.js は探さない = `tako lsp servers` がログインシェルを起こさない）
pub fn managed_entry(config: &FetchConfig, spec: &ServerSpec) -> Option<(PathBuf, PathBuf)> {
    let fetch = spec.fetch.as_ref()?;
    let dir = table::install_dir(&config.root, spec.id, fetch);
    if !dir.join(table::MARKER).is_file() {
        return None;
    }
    let program = match fetch {
        Fetch::Binary { .. } => table::binary_program(&dir, spec.program, config.os == "windows"),
        Fetch::Npm { entry, .. } => dir.join(entry),
    };
    Some((program, dir))
}

/// 置き場にもう入っているか（**待たない**: 印の stat 1 回。Node.js は探さない）
pub fn present(config: &FetchConfig, spec: &ServerSpec) -> bool {
    spec.fetch.as_ref().is_some_and(|fetch| {
        table::install_dir(&config.root, spec.id, fetch)
            .join(table::MARKER)
            .is_file()
    })
}

fn launch_plan(
    config: &FetchConfig,
    spec: &ServerSpec,
    fetch: &Fetch,
    dir: &Path,
    node: Option<(String, &'static str)>,
) -> Option<Managed> {
    let windows = config.os == "windows";
    match fetch {
        Fetch::Binary { .. } => {
            let program = table::binary_program(dir, spec.program, windows);
            let program = program.to_string_lossy().into_owned();
            let args: Vec<&str> = spec.args.to_vec();
            Some(Managed {
                plan: plan_for(&program, &args)?,
                program_path: program,
                runtime: None,
                runtime_source: None,
                dir: dir.to_path_buf(),
            })
        }
        Fetch::Npm {
            entry, node_major, ..
        } => {
            let (node, source) = match node {
                Some(found) => found,
                None => find_node(config, *node_major)?,
            };
            let entry = dir.join(entry).to_string_lossy().into_owned();
            let mut args: Vec<&str> = vec![entry.as_str()];
            args.extend(spec.args.iter().copied());
            Some(Managed {
                plan: plan_for(&node, &args)?,
                program_path: entry.clone(),
                runtime: Some(node),
                runtime_source: Some(source),
                dir: dir.to_path_buf(),
            })
        }
    }
}

/// 起動計画（`default_launch` と同じくログインシェル経由 = サーバが cargo / python を見つけられる）。
/// Windows はシェルを経由しない（`user_env_cli` は名前を PATH で引くので、置き場の絶対パスはそのまま起こす）
fn plan_for(program: &str, args: &[&str]) -> Option<ChildCmd> {
    if cfg!(windows) {
        return Some(ChildCmd {
            program: program.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        });
    }
    let mut snippet = format!("exec {}", tako_core::shell::quote_for_shell(program));
    for arg in args {
        snippet.push(' ');
        snippet.push_str(&tako_core::shell::quote_for_shell(arg));
    }
    child_cmd::user_env_cli(&snippet, program, args)
}

/// 足りる Node.js: 利用者の PATH → tako が取ったもの。どちらも無ければ `None`
fn find_node(config: &FetchConfig, node_major: u32) -> Option<(String, &'static str)> {
    if let Some(path) = (config.node_lookup)(node_major) {
        return Some((path, "path"));
    }
    let dir = table::node_dir(&config.root, &table::NODE);
    let program = table::node_program(&dir, config.os == "windows");
    (dir.join(table::MARKER).is_file() && program.is_file())
        .then(|| (program.to_string_lossy().into_owned(), "managed"))
}

/// 取って置き場へ確定させ、起こし方を返す（済んでいれば取らずに返す）。**待つ**（ネットワーク）。
///
/// `on_progress` は進捗が動いたとき（間隔 [`PROGRESS_INTERVAL`] 以上あけて）と段の変わり目に呼ぶ
pub fn ensure(
    config: &FetchConfig,
    spec: &ServerSpec,
    on_progress: &dyn Fn(),
) -> Result<Managed, FetchError> {
    let fetch = spec.fetch.as_ref().ok_or(FetchError::Unsupported)?;
    if !fetch.available_for(config.os, config.arch) {
        return Err(FetchError::Unsupported);
    }
    let dir = table::install_dir(&config.root, spec.id, fetch);
    let lock = lock_for(&dir);
    // 同じ置き場を取っている人が居れば、ここで終わるのを待つ（終われば下で済みを見て返る）
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let guard = ProgressGuard::begin(&dir);
    let progress = &guard.progress;
    // Node.js が要るなら先に揃える（利用者の PATH に足りる版があれば取らない）
    let node = match fetch {
        Fetch::Npm { node_major, .. } => Some(match find_node(config, *node_major) {
            Some(found) => found,
            None => ensure_node(config, &table::NODE, progress, on_progress)?,
        }),
        Fetch::Binary { .. } => None,
    };
    if dir.join(table::MARKER).is_file() {
        if let Some(managed) = launch_plan(config, spec, fetch, &dir, node.clone()) {
            return Ok(managed);
        }
    }
    let assets = fetch.assets_for(config.os, config.arch);
    let label = fetch.label(spec.id);
    let steps_before = progress.snapshot().step;
    progress.update(|p| p.steps = steps_before + assets.len());
    install_assets(
        config,
        &dir,
        &label,
        &assets,
        &Layout::Server {
            program: spec.program,
            fetch,
        },
        progress,
        on_progress,
    )?;
    launch_plan(config, spec, fetch, &dir, node).ok_or_else(|| FetchError::Io {
        item: label,
        detail: text::FETCH_LAUNCH_DETAIL.text().to_string(),
    })
}

/// Node.js を取る（済んでいれば取らない）
fn ensure_node(
    config: &FetchConfig,
    node: &NodeRuntime,
    progress: &Progress,
    on_progress: &dyn Fn(),
) -> Result<(String, &'static str), FetchError> {
    let asset =
        table::select(node.assets, config.os, config.arch).ok_or(FetchError::Unsupported)?;
    let dir = table::node_dir(&config.root, node);
    let program = table::node_program(&dir, config.os == "windows");
    let lock = lock_for(&dir);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    if !(dir.join(table::MARKER).is_file() && program.is_file()) {
        progress.update(|p| p.steps = 1);
        install_assets(
            config,
            &dir,
            &node.label(),
            &[OwnedAsset::from(asset)],
            &Layout::Node,
            progress,
            on_progress,
        )?;
    }
    Ok((program.to_string_lossy().into_owned(), "managed"))
}

/// 取得物をどう並べるか
enum Layout<'a> {
    /// サーバ（Binary は実行ファイル 1 つ、Npm は `node_modules/<name>/`）
    Server { program: &'a str, fetch: &'a Fetch },
    /// Node.js の実行ファイル 1 つ
    Node,
}

/// 取って検証して一時の段へ展開し、印を書いてから `dir` へ rename する
fn install_assets(
    config: &FetchConfig,
    dir: &Path,
    label: &str,
    assets: &[OwnedAsset],
    layout: &Layout<'_>,
    progress: &Progress,
    on_progress: &dyn Fn(),
) -> Result<(), FetchError> {
    let io = |e: std::io::Error| FetchError::Io {
        item: label.to_string(),
        detail: e.to_string(),
    };
    let parent = dir.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).map_err(io)?;
    sweep_stale_partials(parent);
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let staging = parent.join(format!("{PARTIAL_PREFIX}{unique}"));
    let downloads = parent.join(format!("{PARTIAL_PREFIX}{unique}.dl"));
    // 途中で失敗したら一時の段を消す（印の無い段は使わないが、ディスクを食い続けない）
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    let _cleanup = Cleanup(vec![staging.clone(), downloads.clone()]);
    std::fs::create_dir_all(&staging).map_err(io)?;
    std::fs::create_dir_all(&downloads).map_err(io)?;
    let windows = config.os == "windows";
    let mut downloaded = 0u64;
    let mut record = Vec::new();
    for (index, asset) in assets.iter().enumerate() {
        let item = asset_label(label, asset, assets.len());
        progress.update(|p| {
            p.item = item.clone();
            p.phase = Phase::Downloading;
            p.step += 1;
            p.done = 0;
            p.total = asset.size;
        });
        on_progress();
        let file = downloads.join(format!("{index}"));
        let bytes = download(config, asset, &file, &item, progress, on_progress)?;
        downloaded += bytes;
        progress.update(|p| p.phase = Phase::Extracting);
        on_progress();
        let limit = extract_limit(asset);
        let extracted = match (asset.archive, layout) {
            (Archive::Gzip, Layout::Server { program, .. }) => {
                let dest = table::binary_program(&staging, program, windows);
                archive::gunzip_to(&file, &dest, limit).map(|_| Some(dest))
            }
            (Archive::ZipMember(member), Layout::Server { program, .. }) => {
                let dest = table::binary_program(&staging, program, windows);
                archive::unzip_member(&file, member, &dest, limit).map(|_| Some(dest))
            }
            (Archive::TarGzMember(member), Layout::Server { program, .. }) => {
                let dest = table::binary_program(&staging, program, windows);
                archive::untar_gz_member(&file, member, &dest, limit).map(|_| Some(dest))
            }
            (Archive::NpmTarball, Layout::Server { fetch, .. }) => {
                let name = match fetch {
                    Fetch::Npm { packages, .. } => packages.get(index).map(|p| p.name),
                    Fetch::Binary { .. } => None,
                };
                match name {
                    Some(name) => archive::untar_gz_all(
                        &file,
                        &staging.join("node_modules").join(name),
                        limit,
                    )
                    .map(|_| None),
                    None => Err(ArchiveError::Format("npm の包みの名前が無い".into())),
                }
            }
            (Archive::TarGzMember(member), Layout::Node) => {
                let dest = table::node_program(&staging, windows);
                archive::untar_gz_member(&file, member, &dest, limit).map(|_| Some(dest))
            }
            (Archive::ZipMember(member), Layout::Node) => {
                let dest = table::node_program(&staging, windows);
                archive::unzip_member(&file, member, &dest, limit).map(|_| Some(dest))
            }
            (Archive::Gzip | Archive::NpmTarball, Layout::Node) => {
                Err(ArchiveError::Format("Node.js の取得物の形が違う".into()))
            }
        };
        let executable = extracted.map_err(|e| match e {
            ArchiveError::Io(e) => FetchError::Io {
                item: item.clone(),
                detail: e.to_string(),
            },
            ArchiveError::Format(detail) => FetchError::Archive {
                item: item.clone(),
                detail,
            },
        })?;
        if let Some(path) = executable {
            archive::make_executable(&path).map_err(io)?;
        }
        let _ = std::fs::remove_file(&file);
        record.push(json!({ "url": asset.url, "digest": digest_label(asset.digest) }));
    }
    let marker = json!({
        "label": label,
        "assets": record,
        "downloaded_bytes": downloaded,
        "installed_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
    });
    std::fs::write(
        staging.join(table::MARKER),
        serde_json::to_vec_pretty(&marker).unwrap_or_default(),
    )
    .map_err(io)?;
    match std::fs::rename(&staging, dir) {
        Ok(()) => {}
        // 別のプロセスが先に確定させた（GUI と CLI の `tako lsp install` が重なった）
        Err(_) if dir.join(table::MARKER).is_file() => {}
        Err(_) => {
            // 印の無い段（以前の途中の跡）が居れば退けてから置き直す
            let _ = std::fs::remove_dir_all(dir);
            std::fs::rename(&staging, dir).map_err(io)?;
        }
    }
    crate::diag::persist_log(&format!(
        "LSP 取得: {label} {:.1} 秒 {downloaded} バイト",
        progress.started.elapsed().as_secs_f32(),
    ));
    Ok(())
}

/// 一時の段の接頭辞（版の段と見分ける）
const PARTIAL_PREFIX: &str = ".partial-";

/// 1 時間より古い一時の段を消す（落ちたプロセスの跡。別プロセスが今まさに使っている段は若いので残る）
fn sweep_stale_partials(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PARTIAL_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn asset_label(label: &str, asset: &OwnedAsset, count: usize) -> String {
    if count <= 1 {
        return label.to_string();
    }
    // npm の包みが複数（typescript-language-server + typescript）なら包みの名前と版
    asset
        .url
        .rsplit('/')
        .next()
        .and_then(|file| file.strip_suffix(".tgz"))
        .map(str::to_string)
        .unwrap_or_else(|| label.to_string())
}

fn digest_label(digest: Digest) -> String {
    match digest {
        Digest::Sha256(hex) => format!("sha256:{hex}"),
        Digest::Sha512Base64(b64) => format!("sha512-{b64}"),
    }
}

/// 展開後の上限（展開した中身が取得物の 8 倍 + 余白を超えたら壊れた入力とみなす。
/// 実測: pyright は 4.2 MB → 19.5 MB、Node.js は 52.9 MB → 117 MB）
fn extract_limit(asset: &OwnedAsset) -> u64 {
    asset.size.saturating_mul(8).saturating_add(64 << 20)
}

/// 取得物 1 つを `dest` へ受け、ハッシュを検証する。受けたバイト数を返す
fn download(
    config: &FetchConfig,
    asset: &OwnedAsset,
    dest: &Path,
    item: &str,
    progress: &Progress,
    on_progress: &dyn Fn(),
) -> Result<u64, FetchError> {
    let url = table::rewrite_url(&asset.url, config.base.as_deref());
    let network = |detail: String| FetchError::Network {
        item: item.to_string(),
        detail,
    };
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .user_agent(format!("tako/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();
    let response = agent.get(&url).call().map_err(|e| network(e.to_string()))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(FetchError::Http {
            item: item.to_string(),
            status,
        });
    }
    // 上限: 表の大きさの 2 倍 + 1 MB（配布元の差し替えで少し育つのは受け、桁違いは打ち切る）
    let limit = asset.size.saturating_mul(2).saturating_add(1 << 20);
    let declared = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(FetchError::TooLarge {
            item: item.to_string(),
            limit,
        });
    }
    let total = declared.unwrap_or(asset.size);
    progress.update(|p| p.total = total);
    let mut reader = response.into_body().into_reader();
    let io = |e: std::io::Error| FetchError::Io {
        item: item.to_string(),
        detail: e.to_string(),
    };
    let mut file = std::fs::File::create(dest).map_err(io)?;
    let mut hasher = Hasher::new(asset.digest);
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_note = Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| network(e.to_string()))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > limit {
            return Err(FetchError::TooLarge {
                item: item.to_string(),
                limit,
            });
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(io)?;
        if last_note.elapsed() >= PROGRESS_INTERVAL {
            last_note = Instant::now();
            progress.update(|p| p.done = done);
            on_progress();
        }
    }
    file.flush().map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    progress.update(|p| {
        p.done = done;
        p.phase = Phase::Verifying;
    });
    on_progress();
    if !hasher.matches(asset.digest) {
        let _ = std::fs::remove_file(dest);
        crate::diag::persist_log(&format!("LSP 取得のハッシュ不一致: {item}"));
        return Err(FetchError::Digest {
            item: item.to_string(),
        });
    }
    Ok(done)
}

/// 取得物全体のハッシュ
enum Hasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
}

impl Hasher {
    fn new(digest: Digest) -> Self {
        match digest {
            Digest::Sha256(_) => Self::Sha256(sha2::Sha256::new()),
            Digest::Sha512Base64(_) => Self::Sha512(sha2::Sha512::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha256(h) => h.update(bytes),
            Self::Sha512(h) => h.update(bytes),
        }
    }

    fn matches(self, digest: Digest) -> bool {
        match (self, digest) {
            (Self::Sha256(h), Digest::Sha256(hex)) => {
                hex_of(&h.finalize()).eq_ignore_ascii_case(hex)
            }
            (Self::Sha512(h), Digest::Sha512Base64(b64)) => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(h.finalize()) == b64
            }
            _ => false,
        }
    }
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 取得物のハッシュを計算する（検証の単体と、ミラーの中身の確かめ用）
pub fn digest_of(bytes: &[u8], sha512: bool) -> String {
    use base64::Engine as _;
    if sha512 {
        let mut h = sha2::Sha512::new();
        h.update(bytes);
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    } else {
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        hex_of(&h.finalize())
    }
}

/// 置き場の中身の大きさ（`tako lsp servers` の `managed.bytes`。stat を辿るだけ）
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ハッシュは配布元の表記で突き合わせる() {
        // `printf abc | shasum -a 256` / `printf abc | openssl dgst -sha512 -binary | base64`
        assert_eq!(
            digest_of(b"abc", false),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest_of(b"abc", true),
            "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw=="
        );
        let mut h = Hasher::new(Digest::Sha256(""));
        h.update(b"a");
        h.update(b"bc");
        assert!(h.matches(Digest::Sha256(
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        )));
        let mut h = Hasher::new(Digest::Sha512Base64(""));
        h.update(b"abd");
        assert!(!h.matches(Digest::Sha512Base64(
            "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw=="
        )));
    }

    #[test]
    fn 自動で取るかは_env_で決まる() {
        assert!(auto_from(None, false), "既定は開いた時点で取る");
        assert!(
            !auto_from(None, true),
            "セルフテスト・visual-test は明示が無ければ取らない"
        );
        assert!(auto_from(Some("1"), true));
        assert!(auto_from(Some(" on "), true));
        assert!(!auto_from(Some("0"), false));
        assert!(!auto_from(Some("off"), false));
        assert!(auto_from(Some("garbage"), false), "解釈できない値は既定へ");
    }

    #[test]
    fn 大きさの表記() {
        assert_eq!(format_mb(52_909_993), "52.9 MB");
        assert_eq!(format_mb(0), "0.0 MB");
    }
}

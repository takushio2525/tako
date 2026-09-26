//! 言語サーバの束ねと、開いている文書の登録簿（#1678）
//!
//! 実体は GUI プロセス（tako-app）が 1 つ持ち、CLI / MCP は IPC → `dispatch` →
//! `ControlHost::lsp` 経由で**同じ 1 つ**を触る（`.agent/conventions.md`「CLI と MCP は
//! 『同じ 1 本』を通す」）。
//!
//! ## UI スレッドから呼ぶもの / 呼ばないもの
//!
//! [`LspManager::sync`]（打鍵のたびに通る）と [`LspManager::status`] は**待たない**
//! （ロックを短く取って送信キューへ積むだけ）。実行ファイルの解決（unix はログインシェルを
//! 起こす = 遅い）・spawn・initialize・shutdown はすべて専用スレッドで行う。
//! [`LspManager::servers`] / [`LspManager::restart`] / [`LspManager::stop`] は
//! 待ちうるので、dispatch は `prepare_offload` で background へ出す。
//!
//! ## 文書の同期
//!
//! 文書ごとに**サーバへ送った本文の写し**を持ち、今の本文との差分を `didChange` で送る
//! （`tako_core::lsp::sync`）。写しは再起動後の `didOpen` と、起動中に編集された分の
//! まとめ送りにも使う。版は `TextBuffer::version`（#1658）をそのまま使い、別に作らない。
//!
//! **サーバが無くても編集経路は何も変わらない**（ゼロコンフィグ原則）。対象外の拡張子・
//! 未導入のサーバは [`DocLink::Declined`] で覚え、打鍵のたびに探し直さない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::{Action, Event, Lifecycle, RestartPolicy, ServerState};
use tako_core::lsp::{root, sync};
use tako_core::platform::child_cmd::{self, ChildCmd};

use super::diagnostics::DiagnosticsStore;
use super::server::{Handlers, ServerProcess};
use super::text;

/// 要求のタイムアウトの既定（Zed の実測値）
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// shutdown の応答を待つ上限（Zed の実測値）。超えたら kill
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// 最後の文書を閉じてからサーバを止めるまでの猶予
pub const DEFAULT_IDLE_GRACE: Duration = Duration::from_secs(60);
/// exit を送ってから自分で終わるのを待つ上限
const EXIT_WAIT: Duration = Duration::from_secs(1);

/// `TAKO_1007_LEGACY=1` で LSP を丸ごと止める（同一バイナリで旧挙動へ戻す A/B の入口）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_1007_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 実行ファイルの解決結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// 起動計画と、解決した実行ファイルのパス（表示用）
    Found {
        plan: ChildCmd,
        program_path: String,
    },
    /// 見つからない。`override_env` は差し替えの環境変数が指定されていたときその名前
    NotFound {
        program: String,
        override_env: Option<String>,
    },
}

/// 起動の方法を決める関数（本番は [`default_launch`]。テストは偽サーバを返す）
pub type Launcher = Arc<dyn Fn(&ServerSpec) -> Launch + Send + Sync>;

/// 本番の解決: `TAKO_LSP_BIN_<ID>` → `platform::exe::find`（境界 B16）→ `child_cmd`（B21）
///
/// unix は `$SHELL -l -c 'exec <path> <args>'` で起こす（`.app` を Dock から起動すると
/// PATH が最小構成で、rust-analyzer が `cargo` を見つけられない）。`exec` なので
/// 子の pid はサーバそのもの（間にシェルが残らない）。Windows はシェルを経由しない
pub fn default_launch(spec: &ServerSpec) -> Launch {
    let override_env = servers::override_env_name(spec.id);
    let resolved = match std::env::var(&override_env) {
        Ok(path) if !path.trim().is_empty() => {
            if !tako_core::platform::exe::is_executable_file(Path::new(&path)) {
                return Launch::NotFound {
                    program: path,
                    override_env: Some(override_env),
                };
            }
            Some(path)
        }
        _ => tako_core::platform::exe::find(spec.program),
    };
    let Some(program_path) = resolved else {
        return Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        };
    };
    let mut snippet = format!("exec {}", tako_core::shell::quote_for_shell(&program_path));
    for arg in spec.args {
        snippet.push(' ');
        snippet.push_str(&tako_core::shell::quote_for_shell(arg));
    }
    match child_cmd::user_env_cli(&snippet, &program_path, spec.args) {
        Some(plan) => Launch::Found { plan, program_path },
        None => Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        },
    }
}

/// 設定
#[derive(Clone)]
pub struct LspConfig {
    pub table: &'static [ServerSpec],
    pub launcher: Launcher,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub idle_grace: Duration,
    pub restart: RestartPolicy,
    /// `Some` なら生の JSON-RPC を `<dir>/<id>.log` へ残す（`TAKO_LSP_DIAG=1`）
    pub raw_log_dir: Option<PathBuf>,
}

impl LspConfig {
    /// 本番の設定（env で値だけを変えられる。**0 / 不正 / 空は既定へ落とす** = #1503）
    pub fn from_env() -> Self {
        Self {
            table: servers::SERVERS,
            launcher: Arc::new(default_launch),
            request_timeout: env_secs("TAKO_LSP_REQUEST_TIMEOUT_SECS")
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT),
            shutdown_timeout: SHUTDOWN_TIMEOUT,
            idle_grace: env_secs("TAKO_LSP_IDLE_GRACE_SECS").unwrap_or(DEFAULT_IDLE_GRACE),
            restart: RestartPolicy::default(),
            raw_log_dir: matches!(
                std::env::var("TAKO_LSP_DIAG").ok().as_deref(),
                Some("1" | "true" | "on")
            )
            .then(|| tako_core::paths::data_dir().map(|d| d.join("lsp")))
            .flatten(),
        }
    }
}

fn env_secs(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
}

/// 編集セッションが持つ LSP とのつながり（UI 側はこれを 1 つ持って [`LspManager::sync`] へ渡す）
#[derive(Debug, Clone, Default)]
pub enum DocLink {
    /// まだ試していない
    #[default]
    Unlinked,
    /// 受け持つサーバが無い / 未導入 / 同じファイルを別のペインが開いている。
    /// `epoch` が進むまで（restart・他の文書を閉じた）探し直さない
    Declined { epoch: u64 },
    /// 開いている。最後の 1 つが落ちると `didClose` を送る
    Open(Arc<DocLease>),
}

/// 開いている文書 1 つ。[`Drop`] で `didClose` と診断の破棄が走る
pub struct DocLease {
    shared: Weak<Shared>,
    uri: String,
}

impl std::fmt::Debug for DocLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLease").field("uri", &self.uri).finish()
    }
}

impl DocLease {
    pub fn uri(&self) -> &str {
        &self.uri
    }
}

impl Drop for DocLease {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.close(&self.uri);
        }
    }
}

/// 言語サーバの束ね（`Clone` は同じ実体を指す）
#[derive(Clone)]
pub struct LspManager {
    shared: Option<Arc<Shared>>,
}

struct Shared {
    config: LspConfig,
    inner: Mutex<Inner>,
    /// 自分自身への弱参照（スレッドへ渡す）
    this: Weak<Shared>,
}

#[derive(Default)]
struct Inner {
    servers: BTreeMap<ServerKey, Slot>,
    docs: BTreeMap<String, Doc>,
    diagnostics: DiagnosticsStore,
    /// 未導入と分かったサーバ（ID ごと。restart で消える）
    not_installed: BTreeMap<&'static str, NotInstalled>,
    epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ServerKey {
    id: &'static str,
    root: PathBuf,
}

struct Slot {
    spec: &'static ServerSpec,
    lifecycle: Lifecycle,
    /// spawn のたびに進む。古い世代のスレッドからの知らせは捨てる
    generation: u64,
    process: Option<Arc<ServerProcess>>,
    capabilities: Option<Value>,
    server_info: Option<Value>,
    sync_kind: SyncKind,
    position_encoding: Option<String>,
    /// これまでに起こした回数（偽サーバの「それ以上 spawn しない」の観測にも使う）
    spawn_count: u32,
    last_exit: Option<String>,
    /// 猶予つき停止の取り消し用（文書を開き直したら進める）
    idle_token: u64,
    root_uri: String,
    /// 直近の stderr（プロセスが終わっても `tako lsp logs` で読めるように写しておく）
    last_stderr: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncKind {
    None,
    Full,
    Incremental,
}

impl SyncKind {
    fn slug(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

struct Doc {
    key: ServerKey,
    language_id: &'static str,
    /// サーバへ送った（= サーバが持っている）本文の写し
    text: String,
    version: i32,
    /// `didOpen` を送った世代。`None` はまだ送っていない
    opened: Option<u64>,
    /// 診断の URI 照合用に正規化した形
    uri_key: String,
}

struct NotInstalled {
    program: String,
    override_env: Option<String>,
}

fn to_lsp_version(version: u64) -> i32 {
    i32::try_from(version).unwrap_or(i32::MAX)
}

/// URI の照合用の正規化（`%XX` を解き、Windows のドライブ文字を小文字へ）。
/// サーバが返す URI はこちらが送った綴りと符号化の流儀が違いうる（`%3A` / `C:`）
fn uri_key(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut decoded = String::from_utf8_lossy(&out).into_owned();
    if let Some(rest) = decoded.strip_prefix("file:///") {
        let rb = rest.as_bytes();
        if rb.len() >= 2 && rb[0].is_ascii_alphabetic() && rb[1] == b':' {
            decoded = format!("file:///{}{}", rest[..1].to_ascii_lowercase(), &rest[1..]);
        }
    }
    decoded
}

impl LspManager {
    /// 設定を渡して作る。`TAKO_1007_LEGACY=1` なら何もしない manager になる
    pub fn new(config: LspConfig) -> Self {
        if legacy() {
            return Self::disabled();
        }
        let shared = Arc::new_cyclic(|this| Shared {
            config,
            inner: Mutex::new(Inner::default()),
            this: this.clone(),
        });
        Self {
            shared: Some(shared),
        }
    }

    /// 本番の設定で作る
    pub fn from_env() -> Self {
        Self::new(LspConfig::from_env())
    }

    /// 何もしない manager
    pub fn disabled() -> Self {
        Self { shared: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.shared.is_some()
    }

    /// 編集セッション 1 つぶんを同期する（UI スレッドから打鍵のたびに呼んでよい）。
    ///
    /// 編集していなければつながりを外す（= `didClose`）。開いていれば差分を送り、
    /// まだなら受け持つサーバを探して開く
    pub fn sync(&self, link: &mut DocLink, editing: bool, path: &Path, text: &str, version: u64) {
        let Some(shared) = &self.shared else {
            return;
        };
        if !editing {
            *link = DocLink::Unlinked;
            return;
        }
        match link {
            DocLink::Open(lease) => shared.change(lease.uri(), text, version),
            DocLink::Declined { epoch } if *epoch == shared.lock().epoch => {}
            _ => *link = shared.open(path, text, version),
        }
    }

    /// 状態（`tako lsp status`）。`name` はサーバの ID（省略で全部）
    pub fn status(&self, name: Option<&str>) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.status(name)
    }

    /// 検出表と解決結果（`tako lsp servers`）。**実行ファイルを探すので待ちうる**
    pub fn servers(&self) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.servers()
    }

    /// 止めて起こし直す（`tako lsp restart`）。未導入・諦めた・止めたも対象
    pub fn restart(&self, name: Option<&str>) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.restart(name)
    }

    /// 止める（`tako lsp stop`）。restart まで自動では起こさない
    pub fn stop(&self, name: Option<&str>) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.stop(name)
    }

    /// stderr の直近の行（`tako lsp logs`）
    pub fn logs(&self, name: Option<&str>) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.logs(name)
    }

    /// 全サーバを止める（GUI の終了経路）。`deadline` を過ぎたら残りは kill
    pub fn shutdown_all(&self, deadline: Duration) {
        if let Some(shared) = &self.shared {
            shared.shutdown_all(deadline);
        }
    }

    /// その文書の診断の数（#1679 の受け入れ条件「閉じたら 0」の観測口）
    pub fn diagnostics_count(&self, path: &Path) -> usize {
        let Some(shared) = &self.shared else {
            return 0;
        };
        let uri = tako_core::file_uri::from_path(path);
        shared.lock().diagnostics.count(&uri)
    }

    /// 動いているサーバの pid（テストと診断用）
    pub fn server_pids(&self) -> Vec<u32> {
        let Some(shared) = &self.shared else {
            return Vec::new();
        };
        shared
            .lock()
            .servers
            .values()
            .filter_map(|slot| slot.process.as_ref().map(|p| p.pid()))
            .collect()
    }
}

fn disabled_status() -> Value {
    json!({
        "enabled": false,
        "reason": text::DISABLED_REASON.text(),
        "servers": [],
    })
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn arc(&self) -> Option<Arc<Shared>> {
        self.this.upgrade()
    }

    fn step(&self, slot: &mut Slot, event: Event) -> Action {
        let (next, action) = slot.lifecycle.step(event, self.config.restart);
        slot.lifecycle = next;
        action
    }

    // --- 文書 ---------------------------------------------------------------

    fn open(&self, path: &Path, text: &str, version: u64) -> DocLink {
        let epoch = self.lock().epoch;
        let Some(resolved) = servers::resolve_in(self.config.table, path) else {
            return DocLink::Declined { epoch };
        };
        let spec = resolved.spec;
        if self.lock().not_installed.contains_key(spec.id) {
            return DocLink::Declined { epoch };
        }
        // ファイルシステムを数十回 stat するだけ（git は起こさない）なのでロックの外で
        let root = root::find_root(spec, path);
        let uri = tako_core::file_uri::from_path(path);
        let key = ServerKey {
            id: spec.id,
            root: root.clone(),
        };
        let mut guard = self.lock();
        let inner = &mut *guard;
        if inner.docs.contains_key(&uri) {
            return DocLink::Declined { epoch: inner.epoch };
        }
        inner.docs.insert(
            uri.clone(),
            Doc {
                key: key.clone(),
                language_id: resolved_language(self.config.table, spec, path),
                text: text.to_string(),
                version: to_lsp_version(version),
                opened: None,
                uri_key: uri_key(&uri),
            },
        );
        let slot = inner
            .servers
            .entry(key.clone())
            .or_insert_with(|| Slot::new(spec, &root));
        // 猶予つきの停止を取り消す
        slot.idle_token = slot.idle_token.wrapping_add(1);
        match self.step(slot, Event::Open) {
            Action::Spawn => self.begin_spawn(slot, &key, None),
            _ => {
                if slot.lifecycle.state == ServerState::Running {
                    let generation = slot.generation;
                    let process = slot.process.clone();
                    if let (Some(process), Some(doc)) = (process, inner.docs.get_mut(&uri)) {
                        send_did_open(&process, &uri, doc, generation);
                    }
                }
            }
        }
        DocLink::Open(Arc::new(DocLease {
            shared: self.this.clone(),
            uri,
        }))
    }

    fn change(&self, uri: &str, text: &str, version: u64) {
        let version = to_lsp_version(version);
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(doc) = inner.docs.get_mut(uri) else {
            return;
        };
        if doc.version == version {
            return;
        }
        let slot = inner.servers.get(&doc.key);
        let live = slot.and_then(|slot| {
            (slot.lifecycle.state == ServerState::Running && doc.opened == Some(slot.generation))
                .then(|| slot.process.clone().map(|p| (p, slot.sync_kind)))
                .flatten()
        });
        let Some((process, kind)) = live else {
            // まだ開いていない（起動中・未導入）: 写しだけ進める。開いたときに全文で送る
            replace_text(&mut doc.text, text);
            doc.version = version;
            return;
        };
        let changes = match kind {
            SyncKind::None => None,
            SyncKind::Full => Some(vec![json!({ "text": text })]),
            SyncKind::Incremental => sync::diff_change(&doc.text, text).map(|change| {
                vec![match change.range {
                    Some(((sl, sc), (el, ec))) => json!({
                        "range": {
                            "start": { "line": sl, "character": sc },
                            "end": { "line": el, "character": ec },
                        },
                        "text": change.text,
                    }),
                    None => json!({ "text": change.text }),
                }]
            }),
        };
        if let Some(changes) = changes {
            let params = json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": changes,
            });
            // 送れなかったら写しを進めない（次の変更と一緒に差分で送り直す）
            if process.notify("textDocument/didChange", params).is_err() {
                return;
            }
        }
        replace_text(&mut doc.text, text);
        doc.version = version;
    }

    fn close(&self, uri: &str) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(doc) = inner.docs.remove(uri) else {
            return;
        };
        inner.diagnostics.forget(uri);
        // 同じファイルで断られていた別のペインが開き直せるように
        inner.epoch = inner.epoch.wrapping_add(1);
        let still_open = inner.docs.values().any(|d| d.key == doc.key);
        let Some(slot) = inner.servers.get_mut(&doc.key) else {
            return;
        };
        if let Some(process) = &slot.process {
            if slot.lifecycle.state == ServerState::Running && doc.opened == Some(slot.generation) {
                let _ = process.notify(
                    "textDocument/didClose",
                    json!({ "textDocument": { "uri": uri } }),
                );
            }
        }
        if !still_open && slot.lifecycle.state == ServerState::Running {
            self.schedule_idle_stop(slot, &doc.key);
        }
    }

    // --- ライフサイクル -----------------------------------------------------

    /// 起こす（前のプロセスがあれば先に止める）。世代はここで進める
    fn begin_spawn(&self, slot: &mut Slot, key: &ServerKey, previous: Option<Arc<ServerProcess>>) {
        slot.generation = slot.generation.wrapping_add(1);
        slot.capabilities = None;
        slot.server_info = None;
        slot.position_encoding = None;
        let generation = slot.generation;
        let Some(shared) = self.arc() else {
            return;
        };
        let key = key.clone();
        let _ = std::thread::Builder::new()
            .name(format!("lsp-start-{}", key.id))
            .spawn(move || {
                if let Some(previous) = previous {
                    stop_process(&previous, shared.config.shutdown_timeout);
                }
                shared.run_start(&key, generation);
            });
    }

    fn run_start(&self, key: &ServerKey, generation: u64) {
        let Some(spec) = self.lock().servers.get(key).map(|slot| slot.spec) else {
            return;
        };
        let launch = (self.config.launcher)(spec);
        let plan = match launch {
            Launch::Found { plan, .. } => plan,
            Launch::NotFound {
                program,
                override_env,
            } => {
                let mut inner = self.lock();
                let Some(slot) = inner.servers.get_mut(key) else {
                    return;
                };
                if slot.generation != generation {
                    return;
                }
                self.step(slot, Event::NotFound);
                crate::diag::persist_log(&format!("LSP 未導入: server={}", spec.id));
                inner.not_installed.insert(
                    spec.id,
                    NotInstalled {
                        program,
                        override_env,
                    },
                );
                return;
            }
        };
        let handlers = self.handlers(key, generation);
        let raw_log = self
            .config
            .raw_log_dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.log", spec.id)));
        let process = match ServerProcess::spawn(&plan, spec.id, handlers, raw_log) {
            Ok(process) => process,
            Err(error) => {
                let mut inner = self.lock();
                let Some(slot) = inner.servers.get_mut(key) else {
                    return;
                };
                if slot.generation != generation {
                    return;
                }
                slot.last_exit = Some(format!("spawn: {}", error.kind()));
                self.on_crash(slot, key, generation);
                return;
            }
        };
        let root_uri = {
            let mut inner = self.lock();
            let Some(slot) = inner.servers.get_mut(key) else {
                return;
            };
            if slot.generation != generation {
                drop(inner);
                process.kill();
                return;
            }
            slot.spawn_count = slot.spawn_count.saturating_add(1);
            slot.process = Some(Arc::clone(&process));
            crate::diag::persist_log(&format!(
                "LSP 起動: server={} pid={} 世代={generation} 回数={}",
                spec.id,
                process.pid(),
                slot.spawn_count
            ));
            slot.root_uri.clone()
        };
        let root_name = key
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let outcome = process.request(
            "initialize",
            initialize_params(&root_uri, &root_name),
            self.config.request_timeout,
        );
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(slot) = inner.servers.get_mut(key) else {
            return;
        };
        if slot.generation != generation {
            return;
        }
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                // 殺せば reader が EOF を受けて on_exit → クラッシュとして数える
                slot.last_exit = Some(format!("initialize: {error}"));
                drop(guard);
                process.kill();
                return;
            }
        };
        let capabilities = parse_capabilities(&result);
        slot.sync_kind = sync_kind_of(&capabilities);
        slot.position_encoding = capabilities
            .get("positionEncoding")
            .and_then(Value::as_str)
            .map(str::to_string);
        slot.capabilities = Some(capabilities);
        slot.server_info = result.get("serverInfo").cloned();
        let _ = process.notify("initialized", json!({}));
        self.step(slot, Event::Initialized);
        for (uri, doc) in inner.docs.iter_mut() {
            if doc.key == *key {
                send_did_open(&process, uri, doc, generation);
            }
        }
        // 起動中に最後の文書が閉じられていたら、ここから猶予を数える
        if !inner.docs.values().any(|d| d.key == *key) {
            if let Some(slot) = inner.servers.get_mut(key) {
                self.schedule_idle_stop(slot, key);
            }
        }
    }

    fn handlers(&self, key: &ServerKey, generation: u64) -> Handlers {
        let (weak_n, weak_e) = (self.this.clone(), self.this.clone());
        let (key_n, key_e) = (key.clone(), key.clone());
        Handlers {
            on_notification: Box::new(move |method, params| {
                if let Some(shared) = weak_n.upgrade() {
                    shared.on_notification(&key_n, method, params);
                }
            }),
            on_exit: Box::new(move || {
                if let Some(shared) = weak_e.upgrade() {
                    shared.on_exit(&key_e, generation);
                }
            }),
        }
    }

    fn on_notification(&self, key: &ServerKey, method: &str, params: Value) {
        if method != "textDocument/publishDiagnostics" {
            // window/logMessage・$/progress 等は S1 では受けて捨てる
            return;
        }
        let mut guard = self.lock();
        let inner = &mut *guard;
        let docs = &inner.docs;
        inner.diagnostics.publish(params, |uri| {
            let wanted = uri_key(uri);
            docs.iter()
                .find(|(_, d)| d.key == *key && d.uri_key == wanted)
                .map(|(ours, _)| ours.clone())
        });
    }

    fn on_exit(&self, key: &ServerKey, generation: u64) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(slot) = inner.servers.get_mut(key) else {
            return;
        };
        if slot.generation != generation {
            return;
        }
        if let Some(process) = slot.process.take() {
            slot.last_stderr = process.stderr_tail();
        }
        for doc in inner.docs.values_mut() {
            if doc.key == *key {
                doc.opened = None;
            }
        }
        if matches!(
            slot.lifecycle.state,
            ServerState::Starting | ServerState::Running
        ) {
            if slot.last_exit.is_none() {
                slot.last_exit = Some("exited".into());
            }
            self.on_crash(slot, key, generation);
        }
    }

    /// 落ちた: 上限までは待ってから起こし直す
    fn on_crash(&self, slot: &mut Slot, key: &ServerKey, generation: u64) {
        let action = self.step(slot, Event::Exited);
        crate::diag::persist_log(&format!(
            "LSP 終了: server={} 世代={generation} 落ちた回数={} 次={}",
            key.id,
            slot.lifecycle.crashes,
            slot.lifecycle.state.slug()
        ));
        let Action::ScheduleRestart { delay_ms, .. } = action else {
            return;
        };
        let Some(shared) = self.arc() else {
            return;
        };
        let key = key.clone();
        let _ = std::thread::Builder::new()
            .name(format!("lsp-restart-{}", key.id))
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(delay_ms));
                let mut inner = shared.lock();
                let Some(slot) = inner.servers.get_mut(&key) else {
                    return;
                };
                if slot.generation != generation || slot.lifecycle.state != ServerState::Crashed {
                    return;
                }
                if shared.step(slot, Event::RestartDue) == Action::Spawn {
                    shared.begin_spawn(slot, &key, None);
                }
            });
    }

    /// 最後の文書を閉じた: 猶予が過ぎても誰も開き直さなければ止める（定期実行ではない 1 回きり）
    fn schedule_idle_stop(&self, slot: &mut Slot, key: &ServerKey) {
        slot.idle_token = slot.idle_token.wrapping_add(1);
        let token = slot.idle_token;
        let Some(shared) = self.arc() else {
            return;
        };
        let key = key.clone();
        let grace = self.config.idle_grace;
        let _ = std::thread::Builder::new()
            .name(format!("lsp-idle-{}", key.id))
            .spawn(move || {
                std::thread::sleep(grace);
                let mut guard = shared.lock();
                let inner = &mut *guard;
                if inner.docs.values().any(|d| d.key == key) {
                    return;
                }
                let Some(slot) = inner.servers.get_mut(&key) else {
                    return;
                };
                if slot.idle_token != token {
                    return;
                }
                if shared.step(slot, Event::IdleTimeout) == Action::Shutdown {
                    shared.begin_stop(slot, &key);
                }
            });
    }

    /// 止める（shutdown → exit → 期限を過ぎたら kill）。状態はもう「停止中」
    fn begin_stop(&self, slot: &mut Slot, key: &ServerKey) {
        let process = slot.process.clone();
        let generation = slot.generation;
        let Some(shared) = self.arc() else {
            return;
        };
        let key = key.clone();
        let _ = std::thread::Builder::new()
            .name(format!("lsp-stop-{}", key.id))
            .spawn(move || {
                if let Some(process) = &process {
                    stop_process(process, shared.config.shutdown_timeout);
                }
                let mut guard = shared.lock();
                let inner = &mut *guard;
                let reopen = inner.docs.values().any(|d| d.key == key);
                let Some(slot) = inner.servers.get_mut(&key) else {
                    return;
                };
                if slot.generation != generation {
                    return;
                }
                if let Some(process) = slot.process.take() {
                    slot.last_stderr = process.stderr_tail();
                }
                shared.step(slot, Event::StopFinished);
                crate::diag::persist_log(&format!(
                    "LSP 停止: server={} 世代={generation} 次={}",
                    key.id,
                    slot.lifecycle.state.slug()
                ));
                for doc in inner.docs.values_mut() {
                    if doc.key == key {
                        doc.opened = None;
                    }
                }
                // 止めている間に開き直された文書があれば起こし直す
                if reopen {
                    if let Some(slot) = inner.servers.get_mut(&key) {
                        if shared.step(slot, Event::Open) == Action::Spawn {
                            shared.begin_spawn(slot, &key, None);
                        }
                    }
                }
            });
    }

    fn restart(&self, name: Option<&str>) -> Value {
        let mut guard = self.lock();
        let inner = &mut *guard;
        inner.epoch = inner.epoch.wrapping_add(1);
        let cleared: Vec<&'static str> = inner
            .not_installed
            .keys()
            .copied()
            .filter(|id| name.is_none_or(|n| n == *id))
            .collect();
        for id in &cleared {
            inner.not_installed.remove(id);
        }
        let mut restarted = Vec::new();
        let keys: Vec<ServerKey> = inner
            .servers
            .keys()
            .filter(|k| name.is_none_or(|n| n == k.id))
            .cloned()
            .collect();
        for key in keys {
            let Some(slot) = inner.servers.get_mut(&key) else {
                continue;
            };
            let previous = slot.process.take();
            if let Some(previous) = &previous {
                slot.last_stderr = previous.stderr_tail();
            }
            slot.last_exit = None;
            for doc in inner.docs.values_mut() {
                if doc.key == key {
                    doc.opened = None;
                }
            }
            let Some(slot) = inner.servers.get_mut(&key) else {
                continue;
            };
            let action = self.step(slot, Event::UserRestart);
            if matches!(action, Action::Spawn | Action::Respawn) {
                self.begin_spawn(slot, &key, previous);
                restarted.push(json!({ "id": key.id, "root": key.root.display().to_string() }));
            }
        }
        crate::diag::persist_log(&format!(
            "LSP 再起動（利用者）: 対象={} 件 未導入の記録を消した={} 件",
            restarted.len(),
            cleared.len()
        ));
        json!({
            "action": "restart",
            "restarted": restarted,
            "cleared_not_installed": cleared,
        })
    }

    fn stop(&self, name: Option<&str>) -> Value {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let keys: Vec<ServerKey> = inner
            .servers
            .keys()
            .filter(|k| name.is_none_or(|n| n == k.id))
            .cloned()
            .collect();
        let mut stopped = Vec::new();
        for key in keys {
            let Some(slot) = inner.servers.get_mut(&key) else {
                continue;
            };
            let action = self.step(slot, Event::UserStop);
            if action == Action::Shutdown {
                self.begin_stop(slot, &key);
            }
            stopped.push(json!({
                "id": key.id,
                "root": key.root.display().to_string(),
                "state": slot.lifecycle.state.slug(),
            }));
        }
        json!({ "action": "stop", "stopped": stopped })
    }

    fn shutdown_all(&self, deadline: Duration) {
        let processes: Vec<Arc<ServerProcess>> = {
            let mut inner = self.lock();
            inner
                .servers
                .values_mut()
                .filter_map(|slot| {
                    slot.generation = slot.generation.wrapping_add(1);
                    slot.lifecycle = Lifecycle {
                        state: ServerState::Stopped,
                        ..slot.lifecycle
                    };
                    slot.process.take()
                })
                .collect()
        };
        let timeout = deadline.min(self.config.shutdown_timeout);
        let handles: Vec<_> = processes
            .into_iter()
            .map(|process| std::thread::spawn(move || stop_process(&process, timeout)))
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }

    // --- 読み取り -----------------------------------------------------------

    fn status(&self, name: Option<&str>) -> Value {
        let inner = self.lock();
        let servers: Vec<Value> = inner
            .servers
            .iter()
            .filter(|(k, _)| name.is_none_or(|n| n == k.id))
            .map(|(key, slot)| slot_status(&inner, key, slot))
            .collect();
        let mut value = json!({
            "enabled": true,
            "servers": servers,
            "documents": inner.docs.len(),
            "diagnostics_total": inner.diagnostics.total(),
        });
        if inner.servers.is_empty() {
            value["note"] = json!(text::IDLE_NOTE.text());
        }
        value
    }

    fn servers(&self) -> Value {
        let rows: Vec<Value> = self
            .config
            .table
            .iter()
            .map(|spec| {
                let extensions: Vec<&str> = spec.documents.iter().map(|d| d.extension).collect();
                let mut row = json!({
                    "id": spec.id,
                    "program": spec.program,
                    "extensions": extensions,
                    "install_command": spec.install.command(),
                });
                match (self.config.launcher)(spec) {
                    Launch::Found { program_path, .. } => {
                        row["installed"] = json!(true);
                        row["path"] = json!(program_path);
                    }
                    Launch::NotFound {
                        program,
                        override_env,
                    } => {
                        row["installed"] = json!(false);
                        let guidance =
                            not_installed_guidance(spec, &program, override_env.as_deref());
                        merge(&mut row, guidance);
                    }
                }
                row
            })
            .collect();
        json!({ "enabled": true, "servers": rows })
    }

    fn logs(&self, name: Option<&str>) -> Value {
        let inner = self.lock();
        let servers: Vec<Value> = inner
            .servers
            .iter()
            .filter(|(k, _)| name.is_none_or(|n| n == k.id))
            .map(|(key, slot)| {
                let lines = slot
                    .process
                    .as_ref()
                    .map(|p| p.stderr_tail())
                    .unwrap_or_else(|| slot.last_stderr.clone());
                json!({
                    "id": key.id,
                    "root": key.root.display().to_string(),
                    "stderr": lines,
                })
            })
            .collect();
        json!({
            "enabled": true,
            "note": text::STDERR_NOTE.text(),
            "raw_log_dir": self.config.raw_log_dir.as_ref().map(|d| d.display().to_string()),
            "servers": servers,
        })
    }
}

impl Slot {
    fn new(spec: &'static ServerSpec, root: &Path) -> Self {
        Self {
            spec,
            lifecycle: Lifecycle::default(),
            generation: 0,
            process: None,
            capabilities: None,
            server_info: None,
            sync_kind: SyncKind::None,
            position_encoding: None,
            spawn_count: 0,
            last_exit: None,
            idle_token: 0,
            root_uri: tako_core::file_uri::from_path(root),
            last_stderr: Vec::new(),
        }
    }
}

fn slot_status(inner: &Inner, key: &ServerKey, slot: &Slot) -> Value {
    let docs: Vec<&String> = inner
        .docs
        .iter()
        .filter(|(_, d)| d.key == *key)
        .map(|(uri, _)| uri)
        .collect();
    let diagnostics: usize = docs.iter().map(|uri| inner.diagnostics.count(uri)).sum();
    let mut value = json!({
        "id": key.id,
        "root": key.root.display().to_string(),
        "root_uri": slot.root_uri,
        "state": slot.lifecycle.state.slug(),
        "pid": slot.process.as_ref().map(|p| p.pid()),
        "generation": slot.generation,
        "spawn_count": slot.spawn_count,
        "crashes": slot.lifecycle.crashes,
        "documents": docs.len(),
        "diagnostics": diagnostics,
        "text_document_sync": slot.sync_kind.slug(),
        "position_encoding": slot.position_encoding.as_deref().unwrap_or("utf-16"),
        "server_info": slot.server_info,
        "capabilities": slot.capabilities,
        "last_exit": slot.last_exit,
        // 受け取ったメッセージの数と、本文が JSON でなかった数（本文そのものは出さない）
        "received_messages": slot.process.as_ref().map(|p| p.received_count()),
        "garbage_messages": slot.process.as_ref().map(|p| p.garbage_count()),
        "pending_requests": slot.process.as_ref().map(|p| p.pending_count()),
    });
    match slot.lifecycle.state {
        ServerState::NotInstalled => {
            let (program, override_env) = inner
                .not_installed
                .get(key.id)
                .map(|n| (n.program.clone(), n.override_env.clone()))
                .unwrap_or_else(|| (slot.spec.program.to_string(), None));
            merge(
                &mut value,
                not_installed_guidance(slot.spec, &program, override_env.as_deref()),
            );
        }
        ServerState::GaveUp => {
            value["reason"] = json!(text::fill(
                text::GAVE_UP_REASON,
                &[("count", &slot.lifecycle.crashes.to_string())]
            ));
            value["next_step"] = json!(text::GAVE_UP_NEXT_STEP.text());
        }
        ServerState::Stopped => {
            value["reason"] = json!(text::STOPPED_REASON.text());
            value["next_step"] = json!(text::STOPPED_NEXT_STEP.text());
        }
        _ => {}
    }
    value
}

/// 未導入の「理由 + 次の一手（導入コマンド）」（#983 の作法）
fn not_installed_guidance(spec: &ServerSpec, program: &str, override_env: Option<&str>) -> Value {
    let reason = match override_env {
        Some(env) => text::fill(
            text::OVERRIDE_INVALID_REASON,
            &[("env", env), ("path", program)],
        ),
        None => text::fill(text::NOT_INSTALLED_REASON, &[("program", program)]),
    };
    let command = spec.install.command();
    json!({
        "reason": reason,
        "next_step": text::fill(text::NOT_INSTALLED_NEXT_STEP, &[("command", command)]),
        "install_command": command,
    })
}

fn merge(target: &mut Value, extra: Value) {
    if let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) {
        target.extend(extra);
    }
}

fn resolved_language(table: &'static [ServerSpec], spec: &ServerSpec, path: &Path) -> &'static str {
    servers::resolve_in(table, path)
        .filter(|r| r.spec.id == spec.id)
        .map(|r| r.language_id)
        .unwrap_or("plaintext")
}

fn replace_text(shadow: &mut String, text: &str) {
    shadow.clear();
    shadow.push_str(text);
}

fn send_did_open(process: &ServerProcess, uri: &str, doc: &mut Doc, generation: u64) {
    let params = json!({
        "textDocument": {
            "uri": uri,
            "languageId": doc.language_id,
            "version": doc.version,
            "text": doc.text,
        }
    });
    if process.notify("textDocument/didOpen", params).is_ok() {
        doc.opened = Some(generation);
    }
}

/// shutdown → exit → 待って終わらなければ kill
fn stop_process(process: &ServerProcess, timeout: Duration) {
    if !process.has_exited() {
        let _ = process.request("shutdown", Value::Null, timeout);
        let _ = process.notify("exit", Value::Null);
    }
    if !process.wait_exit(EXIT_WAIT) {
        process.kill();
    }
    process.kill();
}

/// クライアントの能力。**S1 で実際に使うものだけ**を申告する
/// （文書同期と診断の受信。使わない能力を申告するとサーバが無駄な仕事をする）
fn initialize_params(root_uri: &str, root_name: &str) -> Value {
    use lsp_types::{
        ClientCapabilities, ClientInfo, GeneralClientCapabilities, InitializeParams,
        PositionEncodingKind, PublishDiagnosticsClientCapabilities, TextDocumentClientCapabilities,
        TextDocumentSyncClientCapabilities, Uri, WorkspaceFolder,
    };
    let uri: Option<Uri> = root_uri.parse().ok();
    #[allow(deprecated)]
    let params = InitializeParams {
        process_id: Some(std::process::id()),
        root_uri: uri.clone(),
        workspace_folders: uri.map(|uri| {
            vec![WorkspaceFolder {
                uri,
                name: root_name.to_string(),
            }]
        }),
        capabilities: ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![PositionEncodingKind::UTF16]),
                ..Default::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                synchronization: Some(TextDocumentSyncClientCapabilities {
                    dynamic_registration: Some(false),
                    will_save: Some(false),
                    will_save_wait_until: Some(false),
                    did_save: Some(false),
                }),
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    version_support: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        client_info: Some(ClientInfo {
            name: "tako".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        ..Default::default()
    };
    serde_json::to_value(params).unwrap_or(Value::Null)
}

/// initialize の応答から `ServerCapabilities` を取り出す。
/// `lsp-types` で読めれば読み直した形（正規化）、読めなければ生の JSON を持つ
fn parse_capabilities(result: &Value) -> Value {
    let raw = result.get("capabilities").cloned().unwrap_or(Value::Null);
    serde_json::from_value::<lsp_types::ServerCapabilities>(raw.clone())
        .ok()
        .and_then(|typed| serde_json::to_value(typed).ok())
        .unwrap_or(raw)
}

/// `textDocumentSync` の種別（数値か `{ change: n }`。無ければ None = 送らない）
fn sync_kind_of(capabilities: &Value) -> SyncKind {
    let kind = match capabilities.get("textDocumentSync") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::Object(options)) => options.get("change").and_then(Value::as_i64),
        _ => None,
    };
    match kind {
        Some(1) => SyncKind::Full,
        Some(2) => SyncKind::Incremental,
        _ => SyncKind::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同期の種別は数値でも_options_でも読める() {
        assert_eq!(
            sync_kind_of(&json!({"textDocumentSync": 2})),
            SyncKind::Incremental
        );
        assert_eq!(
            sync_kind_of(&json!({"textDocumentSync": {"openClose": true, "change": 1}})),
            SyncKind::Full
        );
        assert_eq!(sync_kind_of(&json!({})), SyncKind::None);
    }

    #[test]
    fn uri_の照合は符号化とドライブ文字の違いを吸収する() {
        assert_eq!(
            uri_key("file:///c%3A/w/a%20b.rs"),
            uri_key("file:///C:/w/a b.rs")
        );
        assert_eq!(uri_key("file:///w/%E6%97%A5.rs"), "file:///w/日.rs");
        assert_ne!(uri_key("file:///w/a.rs"), uri_key("file:///w/b.rs"));
        // 壊れた `%` はそのまま
        assert_eq!(uri_key("file:///w/%zz"), "file:///w/%zz");
    }

    #[test]
    fn 初期化の申告は_utf16_と文書同期と診断だけ() {
        let params = initialize_params("file:///w", "w");
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        let text_document = params["capabilities"]["textDocument"].as_object().unwrap();
        let mut keys: Vec<&String> = text_document.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["publishDiagnostics", "synchronization"]);
        assert_eq!(params["rootUri"], json!("file:///w"));
        assert_eq!(params["workspaceFolders"][0]["name"], json!("w"));
        assert_eq!(params["clientInfo"]["name"], json!("tako"));
    }

    #[test]
    fn 版は_i32_に収める() {
        assert_eq!(to_lsp_version(7), 7);
        assert_eq!(to_lsp_version(u64::MAX), i32::MAX);
    }
}

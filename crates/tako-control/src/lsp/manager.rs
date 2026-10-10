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
//! ## 実行ファイルの解決のキャッシュ（#1769）
//!
//! 解決（unix はサーバ 1 つにつきログインシェル 1 つ）の結果（見つかった + パス / 見つからない /
//! 打ち切った）をサーバの ID ごとに持ち、`servers` と起動の両方が使う。**引き直すのは**
//! `restart`（明示）と、PATH が変わりうる出来事（シェル統合が知らせた cwd の変化・コマンドの終わり =
//! `tako_core::shell_activity`）があったときと、見つかったパスが実行できなくなったとき（stat 1 回）
//! だけ。キャッシュに無いサーバの解決は並行して行う（1 つずつ待つと合計になる）。
//! ログインシェルは `platform::exe::find_with_timeout`（Code Runner の #1730 と同じ 1 実装）=
//! `probe::output_with_timeout` の上限を通る（profile が入力を待つ形でも固まらず、打ち切ったことを
//! [`Launch::TimedOut`] で知らせる）。
//!
//! ## 未導入のサーバを取る（#1944）
//!
//! 起こすときの解決は **差し替えの env → 利用者の PATH（動くことを確かめる）→ tako の置き場
//! （`<data_dir>/lsp-servers`）→ 取る** の順。グローバルに入っている版があればそちらを使う。
//! 取るのは開いた時点（[`FetchConfig::auto`]）か `tako lsp install` / 画面の「入れる」で、
//! 取っているあいだの状態は「取得中」（[`ServerState::Fetching`]）。取れなければ理由を記録して
//! 「未導入」へ落ち（#1678 の導入コマンドの案内は残す）、自動では取り直さない（`restart` /
//! `install` で取り直す）。未導入のサーバは、シェル統合の合図（[`tako_core::shell_activity`]）で
//! 引き直して見つかれば restart を待たずに起こす（#1823 の 2。間隔は [`RECHECK_INTERVAL`] 以上あける）。
//! UI へは診断の知らせとは**別の口**（[`LspManager::state_events`]。中身を運ばず起こすだけ）で知らせる:
//! 状態が変わったら「つながりを作り直して描き直す」（[`LspManager::take_refresh_wanted`] が真）、取得の
//! 進捗は描き直すだけ。診断の口は従来どおり URI だけを運ぶ（#1679 の約束を崩さない）。
//!
//! ## 文書の同期
//!
//! 文書ごとに**サーバへ送った本文の写し**を持ち、今の本文との差分を `didChange` で送る
//! （`tako_core::lsp::sync`）。写しは再起動後の `didOpen` と、起動中に編集された分の
//! まとめ送りにも使う。版は `TextBuffer::version`（#1658）をそのまま使い、別に作らない。
//!
//! **サーバが無くても編集経路は何も変わらない**（ゼロコンフィグ原則）。対象外の拡張子・
//! 未導入のサーバは [`DocLink::Declined`] で覚え、打鍵のたびに探し直さない。
//!
//! ## 同じファイルを複数のペインで開く（#1769）
//!
//! **1 URI = 1 文書**を持ち手（[`DocLease`]。編集セッション 1 つにつき 1 つ）で共有する
//! （エディタで広く使われる形。LSP は 1 URI に 1 open しか許さない）。`didOpen` は最初の 1 つが
//! 開いたとき、`didClose` は最後の 1 つが閉じたときだけ送る。どのペインの編集も**同じ文書の版**を
//! 進める（版はペインの `TextBuffer::version` より小さくならないよう文書ごとに単調に進める =
//! 2 つ目のペインのバッファの版は 1 つ目より若いことがある）。持ち手ごとに「最後に送った自分の
//! バッファの版」を覚え、**自分の本文が変わったペインだけが送る**（変わっていないペインの同期で
//! 相手の編集を巻き戻さない）。ペインごとのバッファは別々なので、サーバが見るのは最後に編集した
//! ペインの本文で、診断はその本文の座標で両方のペインへ出る。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::{Action, Event, Lifecycle, RestartPolicy, ServerState};
use tako_core::lsp::{position, root, sync};
use tako_core::platform::child_cmd::{self, ChildCmd};

use super::completion::{CompletionAnswer, CompletionError, CompletionRequest};
use super::diagnostics::{DiagnosticsStore, DocDiagnostics};
use super::fetch::{self, FetchConfig, FetchError};
use super::format::{FormatAnswer, FormatError, FormatRequest};
use super::goto::{GotoAnswer, GotoError, GotoRequest};
use super::hover::{HoverAnswer, HoverError, HoverRequest};
use super::menu::{MenuCapabilities, MenuRequest};
use super::rpc::RequestId;
use super::server::{Handlers, RpcError, ServerProcess};
use super::text;
use tako_core::lsp::completion as comp;

/// 要求のタイムアウトの既定（Zed の実測値）
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// shutdown の応答を待つ上限（Zed の実測値）。超えたら kill
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// 最後の文書を閉じてからサーバを止めるまでの猶予
pub const DEFAULT_IDLE_GRACE: Duration = Duration::from_secs(60);
/// exit を送ってから自分で終わるのを待つ上限
const EXIT_WAIT: Duration = Duration::from_secs(1);
/// UI へ「この文書の診断が変わった」を知らせるキューの上限（Zed の
/// `INCOMING_MESSAGE_QUEUE_CAPACITY` の実測値。設計書 §2 / §18）。
/// 溢れたら捨てて「全部読み直して」の印を立てる（UI が詰まってもメモリを食わない）
pub const DIAGNOSTICS_EVENT_CAPACITY: usize = 128;

/// 未導入のサーバの引き直し（シェル統合の合図）の最小間隔（#1944 / #1823 の 2）。
/// 引き直しはサーバ 1 つにつきログインシェル 1 つを起こすので、コマンドを打つたびには起こさない
/// （間隔の中に来た合図は、間隔が明けたときに 1 回だけ引き直す）
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(15);

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
    /// 探すのに起こしたログインシェルが上限までに返らなかった（#1769。打ち切った）
    TimedOut { program: String, waited_secs: u64 },
    /// PATH で見つかったが動かない（#1944。検出表の `probe_args` で確かめて落ちた。
    /// rustup の代理だけが居て component が無い等）。見つからないのと同じに扱う
    Broken {
        program_path: String,
        detail: String,
    },
}

/// 起動の方法を決める関数（本番は [`default_launch`]。テストは偽サーバを返す）
pub type Launcher = Arc<dyn Fn(&ServerSpec) -> Launch + Send + Sync>;

/// 本番の解決: `TAKO_LSP_BIN_<ID>` → `platform::exe::find_with_timeout`（境界 B16）→ `child_cmd`（B21）
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
        // #1769: ログインシェルは上限つき（`probe::probe_timeout` = 既定 15 秒。外せない）
        _ => {
            let found = tako_core::platform::exe::find_with_timeout(
                spec.program,
                tako_core::probe::probe_timeout(),
            );
            if let Some(notice) = found.timeout {
                return Launch::TimedOut {
                    program: spec.program.to_string(),
                    waited_secs: notice.waited_secs,
                };
            }
            found.path
        }
    };
    let Some(program_path) = resolved else {
        return Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        };
    };
    // #1944: 検出表が確かめ方を持つサーバは、PATH のものが動くかを確かめてから信じる
    // （差し替えの env は利用者の明示なので確かめない）
    if std::env::var(&override_env).map_or(true, |v| v.trim().is_empty()) {
        if let Some(detail) = probe_failure(spec, &program_path) {
            return Launch::Broken {
                program_path,
                detail,
            };
        }
    }
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

/// PATH で見つかったものを `probe_args` で起こし、成功で終わらなければその理由（#1944）
fn probe_failure(spec: &ServerSpec, program_path: &str) -> Option<String> {
    let Some(tako_core::lsp::fetch::Fetch::Binary { probe_args, .. }) = spec.fetch else {
        return None;
    };
    if probe_args.is_empty() {
        return None;
    }
    match tako_core::probe::output_with_timeout(
        program_path,
        probe_args,
        tako_core::probe::probe_timeout(),
    ) {
        tako_core::probe::Outcome::Done { status, stderr, .. } if !status.success() => {
            // 理由はサーバの stderr の 1 行目（rustup の「component が無い」）。本文は含まない
            let first = String::from_utf8_lossy(&stderr)
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>();
            Some(if first.is_empty() {
                format!("exit {}", status.code().unwrap_or(-1))
            } else {
                first
            })
        }
        tako_core::probe::Outcome::Done { .. } => None,
        tako_core::probe::Outcome::TimedOut { waited, .. } => {
            Some(format!("{} 秒で返らない", waited.as_secs()))
        }
        tako_core::probe::Outcome::Failed { reason, .. } => Some(reason),
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
    /// 受け持つサーバが無い / 未導入。`epoch` が進むまで（restart）探し直さない
    Declined { epoch: u64 },
    /// 開いている。最後の 1 つが落ちると `didClose` を送る
    Open(Arc<DocLease>),
}

/// 開いている文書の持ち手 1 つ（編集セッション 1 つにつき 1 つ）。[`Drop`] で持ち手を外し、
/// **最後の持ち手**なら `didClose` と診断の破棄が走る（#1769）
pub struct DocLease {
    shared: Weak<Shared>,
    uri: String,
    /// 持ち手の番号（文書の `holders` の鍵）
    holder: u64,
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
            shared.close(&self.uri, self.holder);
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
    /// 未導入のサーバを取る設定（#1944）。`None` は取らない（`TAKO_1944_LEGACY=1`・テストの既定）
    fetch: Option<FetchConfig>,
    inner: Mutex<Inner>,
    /// 自分自身への弱参照（スレッドへ渡す）
    this: Weak<Shared>,
}

#[derive(Default)]
struct Inner {
    servers: BTreeMap<ServerKey, Slot>,
    docs: BTreeMap<String, Doc>,
    diagnostics: DiagnosticsStore,
    /// 診断が変わった文書の URI を UI へ運ぶ（#1679。[`LspManager::diagnostics_events`]）
    events: Option<futures::channel::mpsc::Sender<String>>,
    /// キューが満杯で知らせを捨てた（UI は次に読んだとき全部を読み直す）
    events_overflowed: bool,
    /// 状態が変わったことを UI へ知らせる口（#1944。中身は運ばず起こすだけ = [`LspManager::state_events`]）
    ui_wake: Option<futures::channel::mpsc::Sender<()>>,
    /// 次に起きた UI がつながりを作り直すか（状態が変わった。進捗だけなら偽のまま）
    ui_refresh_wanted: bool,
    /// 未導入と分かったサーバ（ID ごと。restart で消える）
    not_installed: BTreeMap<&'static str, NotInstalled>,
    /// 実行ファイルの解決の結果（ID ごと。#1769 = モジュール冒頭）
    resolved: BTreeMap<&'static str, Resolved>,
    epoch: u64,
    /// 次に配る持ち手の番号（#1769）
    next_holder: u64,
    /// 打鍵の補完と、選んだ候補の説明の補い（#1682）。列ごとに**生きている要求は 1 つだけ**で、
    /// 次の要求が来たら前の 1 つを `$/cancelRequest` で捨てる
    completion_lane: Inflight,
    resolve_lane: Inflight,
    /// マウスのホバー（#1681）。補完と同じく生きている要求は 1 つだけ
    hover_lane: Inflight,
    /// 次に配る列の番号
    next_ticket: u64,
    /// 未導入のサーバを引き直しているか・最後に引き直した時刻・間隔の中に合図が来たか（#1944）
    rechecking: bool,
    last_recheck: Option<Instant>,
    recheck_pending: bool,
}

/// 取り消し合う要求の列（#1682）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// 打鍵の補完（`textDocument/completion`）
    Completion,
    /// 選んだ候補の説明（`completionItem/resolve`）
    Resolve,
    /// マウスのホバー（`textDocument/hover`。#1681）
    Hover,
}

impl Lane {
    /// `tako lsp status` の `inflight` の名前（#1909）
    const ALL: [(Lane, &'static str); 3] = [
        (Lane::Completion, "completion"),
        (Lane::Resolve, "resolve"),
        (Lane::Hover, "hover"),
    ];
}

/// 列の「いま生きている 1 つ」（#1682）
#[derive(Default)]
struct Inflight {
    /// 生きている要求の番号。これと違う番号の要求は置き換わった
    ticket: u64,
    /// 送ったがまだ答えを受けていない要求（取り消す相手）
    call: Option<(Weak<ServerProcess>, RequestId)>,
    /// 列へ入ってまだ答えを返していない問い合わせの数（#1909。置き換わった古い要求も、抜けるまで
    /// 数える = 取り消しを追い越して読み込みを待ち続けている古い要求が見える）
    running: usize,
}

impl Inner {
    fn lane(&mut self, lane: Lane) -> &mut Inflight {
        match lane {
            Lane::Completion => &mut self.completion_lane,
            Lane::Resolve => &mut self.resolve_lane,
            Lane::Hover => &mut self.hover_lane,
        }
    }
}

/// 列へ入った問い合わせ 1 つ（#1909。[`Shared::enter_lane`] が返し、落とすと `inflight` から外れる）
struct LaneCall<'a> {
    shared: &'a Shared,
    lane: Lane,
    ticket: u64,
}

impl LaneCall<'_> {
    fn key(&self) -> (Lane, u64) {
        (self.lane, self.ticket)
    }
}

impl Drop for LaneCall<'_> {
    fn drop(&mut self) {
        let mut inner = self.shared.lock();
        let inflight = inner.lane(self.lane);
        inflight.running = inflight.running.saturating_sub(1);
    }
}

/// 検証用の注入（#1909）: `TAKO_1909_INJECT_HOLD=<ファイル>` があれば、取り消し合う問い合わせは
/// 列の番号を決める**前に**そのファイルができるまで止まる（上限 60 秒）。GUI が要求を背景へ渡してから
/// 背景が走り出すまでの遅れ（background executor が混んでいる）を演じ、その間に UI の取り消しが
/// 入る順序をどの機でも作る（実時間の遅れに任せない）
fn hold_for_injection() {
    let Some(path) = std::env::var_os("TAKO_1909_INJECT_HOLD") else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while !Path::new(&path).exists() && Instant::now() < deadline {
        std::thread::sleep(super::goto::READY_POLL);
    }
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
    /// サーバが知らせた「読み込みが済んだか」（`experimental/serverStatus` の `quiescent`。#1680）。
    /// 送らないサーバは `None` のまま
    quiescent: Option<bool>,
    /// 握手が済んだ時刻（状態を送るサーバかを起動直後の猶予のあいだだけ待って確かめる。#1680）
    running_since: Option<Instant>,
    /// 起こした実行ファイルの出どころ（`path` / `override` / `managed`。#1944）と、そのパス・
    /// Node.js で起こしたならその Node.js
    source: Option<&'static str>,
    program_path: Option<String>,
    runtime: Option<String>,
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
    /// 持ち手（編集セッション）ごとに、最後に写しへ取り込んだ**その持ち手のバッファの版**（#1769）。
    /// 空になったら閉じる
    holders: BTreeMap<u64, u64>,
}

#[derive(Clone)]
struct NotInstalled {
    program: String,
    override_env: Option<String>,
    /// 解決のログインシェルを打ち切った（秒。#1769）。`None` は見つからなかった
    timed_out: Option<u64>,
    /// PATH で見つかったが動かなかった（パスと理由。#1944）
    broken: Option<(String, String)>,
    /// 取りに行って失敗した（#1944）。記録があるあいだは開き直しても自動では取り直さない
    fetch_error: Option<FetchError>,
}

impl NotInstalled {
    /// 解決の答え（見つからない・打ち切った・動かない）から
    fn from_launch(spec: &ServerSpec, launch: Launch) -> Self {
        let mut record = Self {
            program: spec.program.to_string(),
            override_env: None,
            timed_out: None,
            broken: None,
            fetch_error: None,
        };
        match launch {
            Launch::NotFound {
                program,
                override_env,
            } => {
                record.program = program;
                record.override_env = override_env;
            }
            Launch::TimedOut {
                program,
                waited_secs,
            } => {
                record.program = program;
                record.timed_out = Some(waited_secs);
            }
            Launch::Broken {
                program_path,
                detail,
            } => record.broken = Some((program_path, detail)),
            Launch::Found { .. } => {}
        }
        record
    }
}

/// 解決 1 つぶん（#1769）
struct Resolved {
    launch: Launch,
    /// 引いたときの `shell_activity::epoch`（違えば引き直す）
    activity: u64,
}

impl Resolved {
    /// まだ使えるか: PATH が変わりうる出来事が無く、見つかったパスがまだ実行できる
    fn fresh(&self, activity: u64) -> bool {
        self.activity == activity
            && match &self.launch {
                Launch::Found { program_path, .. } => {
                    tako_core::platform::exe::is_executable_file(Path::new(program_path))
                }
                _ => true,
            }
    }
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
    /// 設定を渡して作る（未導入のサーバは取らない = #1678 の振る舞い）。
    /// `TAKO_1007_LEGACY=1` なら何もしない manager になる
    pub fn new(config: LspConfig) -> Self {
        Self::with_fetch(config, None)
    }

    /// 取得の設定も渡して作る（#1944。`None` なら取らない）
    pub fn with_fetch(config: LspConfig, fetch: Option<FetchConfig>) -> Self {
        if legacy() {
            return Self::disabled();
        }
        let shared = Arc::new_cyclic(|this| Shared {
            config,
            fetch,
            inner: Mutex::new(Inner::default()),
            this: this.clone(),
        });
        // #1944 / #1823 の 2: シェル統合の合図で、未導入のサーバを引き直す（manager が消えたら外れる）
        let weak = Arc::downgrade(&shared);
        tako_core::shell_activity::subscribe(move || match weak.upgrade() {
            Some(shared) => {
                shared.on_shell_activity();
                true
            }
            None => false,
        });
        Self {
            shared: Some(shared),
        }
    }

    /// 本番の設定で作る（未導入のサーバは data dir へ取る。#1944）
    pub fn from_env() -> Self {
        Self::with_fetch(LspConfig::from_env(), FetchConfig::from_env())
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
            DocLink::Open(lease) => shared.change(lease, text, version),
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

    /// 未導入のサーバを data dir へ取る（`tako lsp install`。#1944）。`name` 省略で表の取れるもの全部。
    /// **待つ**（ネットワーク）: dispatch は offload で、GUI の「入れる」は背景で呼ぶ。
    /// グローバルに入っていれば取らない。取れたら、その言語の文書を待っている未導入のサーバを起こす
    pub fn install(&self, name: Option<&str>) -> Value {
        let Some(shared) = &self.shared else {
            return disabled_status();
        };
        shared.install(name)
    }

    /// 編集している文書を受け持つサーバの、画面に出す状態（#1944）。**待たない**（ロックを短く取るのと
    /// 置き場の印の stat だけ。UI スレッドで描くたびに呼んでよい）。受け持つサーバが無い・まだ
    /// 起こしていなければ `None`
    pub fn document_server(&self, path: &Path) -> Option<DocumentServer> {
        self.shared.as_ref()?.document_server(path)
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

    /// 保持している診断の総数と URI の数（#1679。閉じたペインのぶんが残っていないかの観測口）
    pub fn diagnostics_retained(&self) -> (usize, usize) {
        let Some(shared) = &self.shared else {
            return (0, 0);
        };
        let inner = shared.lock();
        (inner.diagnostics.total(), inner.diagnostics.uri_count())
    }

    /// 開いている文書 1 つの診断（#1679）。`uri` は [`DocLease::uri`]。
    /// 開いていない URI は `None`（まだ publish が無ければ空の診断で返す）
    pub fn document_diagnostics(&self, uri: &str) -> Option<DocumentDiagnostics> {
        let shared = self.shared.as_ref()?;
        let inner = shared.lock();
        let doc = inner.docs.get(uri)?;
        let state = inner
            .servers
            .get(&doc.key)
            .map_or(ServerState::Starting, |slot| slot.lifecycle.state);
        Some(DocumentDiagnostics {
            server: doc.key.id,
            server_state: state.slug(),
            diagnostics: inner.diagnostics.get(uri).unwrap_or_default(),
        })
    }

    /// 「この文書の診断が変わった」の知らせを受け取る口（#1679。UI が 1 つだけ持つ）。
    ///
    /// 上限 [`DIAGNOSTICS_EVENT_CAPACITY`] の bounded channel。運ぶのは URI だけで、
    /// 中身は受けた側が [`Self::document_diagnostics`] で読む（同じ URI が何度来ても
    /// 最新を 1 回読めば済む）。溢れた回は [`Self::take_events_overflow`] が `true` を返す。
    /// 呼ぶたびに前の口は閉じる。無効な manager（`TAKO_1007_LEGACY=1`）は `None`
    pub fn diagnostics_events(&self) -> Option<futures::channel::mpsc::Receiver<String>> {
        let shared = self.shared.as_ref()?;
        let (tx, rx) = futures::channel::mpsc::channel(DIAGNOSTICS_EVENT_CAPACITY);
        let mut inner = shared.lock();
        inner.events = Some(tx);
        inner.events_overflowed = false;
        Some(rx)
    }

    /// 状態が変わったことの知らせを受け取る口（#1944。UI が 1 つだけ持つ）。中身は運ばず起こすだけで
    /// （容量 1 = 満杯ならもう起こしてある）、起きた UI は [`Self::take_refresh_wanted`] が真なら
    /// 編集セッションを同期し直してから描き直し、偽なら描き直すだけ（取得の進捗）。
    /// 呼ぶたびに前の口は閉じる。無効な manager（`TAKO_1007_LEGACY=1`）は `None`
    pub fn state_events(&self) -> Option<futures::channel::mpsc::Receiver<()>> {
        let shared = self.shared.as_ref()?;
        let (tx, rx) = futures::channel::mpsc::channel(0);
        shared.lock().ui_wake = Some(tx);
        Some(rx)
    }

    /// 状態が変わって UI がつながりを作り直すべきか（読むと倒れる。#1944）
    pub fn take_refresh_wanted(&self) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        std::mem::take(&mut shared.lock().ui_refresh_wanted)
    }

    /// キューが溢れて知らせを捨てたか（読むと倒れる）。`true` なら全文書を読み直す
    pub fn take_events_overflow(&self) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        std::mem::take(&mut shared.lock().events_overflowed)
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

    /// 定義ジャンプの問い合わせ（#1680）。**背景スレッドから呼ぶ**（サーバの起動と応答を待つ。
    /// 上限は `request.timeout`）。
    ///
    /// 文書が開いていなければ（編集モードでないプレビューから ⌘クリックした）、**問い合わせの
    /// あいだだけ** `didOpen` し（本文は `request.document` = 編集セッションの全文、無ければ
    /// ディスクの中身）、答えを受けたら `didClose` する。
    /// 「開いただけではサーバを起こさない」（設計書 §16-2）は保ったまま、利用者が明示的に
    /// 問い合わせたときだけ起こす
    pub fn goto(&self, request: &GotoRequest) -> Result<GotoAnswer, GotoError> {
        match &self.shared {
            Some(shared) => shared.goto(request),
            None => Err(GotoError::Disabled),
        }
    }

    /// 整形の問い合わせ（#1683）。**背景スレッドから呼ぶ**（サーバの起動と応答を待つ。
    /// 上限は `request.timeout`）。
    ///
    /// サーバが見ている本文（送った写し）を `request.text` に揃えてから頼み（打鍵の直後で
    /// 同期がまだ・同じファイルの別のペインが編集した、のどちらでも答えの座標がずれない）、
    /// 答えを待つあいだに写しが変わったら [`FormatError::Stale`] で返す。文書が開いていなければ
    /// 定義ジャンプと同じく問い合わせのあいだだけ `didOpen` する。答えは並べて最小にし、
    /// 範囲の整形なら範囲の外を変えない形へ収めて返す（当てるのは呼び手）
    pub fn format(&self, request: &FormatRequest) -> Result<FormatAnswer, FormatError> {
        match &self.shared {
            Some(shared) => shared.format(request),
            None => Err(FormatError::Lsp(GotoError::Disabled)),
        }
    }

    /// ホバーの問い合わせ（#1681）。**背景スレッドから呼ぶ**（サーバの起動と応答を待つ。
    /// 上限は `request.timeout`）。
    ///
    /// `request.superseding`（GUI のマウス）の要求は、次の `superseding` の要求が来た時点で
    /// `$/cancelRequest` を送って捨て、待っていた側は [`HoverError::Superseded`] で返る
    /// （なぞったあとの古い要求の答えを待たない）。`request.open` が偽（マウス）なら、文書が
    /// 開いていなければ問い合わせずに [`HoverError::NotOpen`]（乗せただけでサーバを起こさない）。
    /// 真（CLI / MCP・メニュー）なら定義ジャンプと同じく問い合わせのあいだだけ `didOpen` する
    pub fn hover(&self, request: &HoverRequest) -> Result<HoverAnswer, HoverError> {
        match &self.shared {
            Some(shared) => shared.hover(request),
            None => Err(GotoError::Disabled.into()),
        }
    }

    /// マウスのホバーの取り消しの番号を**UI スレッドで**先に取る（#1893。列の前の要求は取り消す）。
    /// 要求の `ticket` に載せると、背景で番号を取り直さない = この後の `cancel_hover` を追い越さない。
    /// LSP を止めていれば `None`。**待たない**（ロックを短く取るだけ）
    pub fn reserve_hover(&self) -> Option<u64> {
        self.shared
            .as_ref()
            .map(|shared| shared.supersede(Lane::Hover))
    }

    /// 待っているマウスのホバーを捨てる（カードを閉じた・識別子から外れた。#1681）
    pub fn cancel_hover(&self) {
        if let Some(shared) = &self.shared {
            shared.supersede(Lane::Hover);
        }
    }

    /// この文書が言語サーバにつながっているか（#1681。マウスのホバーの入口が、サーバを
    /// 起こさない文書で背景の問い合わせを立てないために見る。ロックを短く取るだけ）
    pub fn has_document(&self, path: &Path) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        let uri = tako_core::file_uri::from_path(path);
        shared.lock().docs.contains_key(&uri)
    }

    /// この文書を受け持つサーバが起動中か、プロジェクトを読み込み中か（#1869。補完の一覧の
    /// 代わりに「読み込み中」を出すかを、打鍵の問い合わせを出すときに GUI が見る）。
    /// **待たない**（ロックを短く取るだけ）。文書が開いていなければ偽（サーバを起こさない）
    pub fn server_loading(&self, path: &Path) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        let uri = tako_core::file_uri::from_path(path);
        let inner = shared.lock();
        inner
            .docs
            .get(&uri)
            .and_then(|doc| inner.servers.get(&doc.key))
            .is_some_and(|slot| {
                matches!(
                    slot.lifecycle.state,
                    ServerState::Starting | ServerState::Fetching
                ) || slot_loading(slot)
            })
    }

    /// 補完の問い合わせ（#1682）。**背景スレッドから呼ぶ**（サーバの起動と応答を待つ。
    /// 上限は `request.timeout`）。
    ///
    /// `request.superseding`（GUI の打鍵）の要求は、次の `superseding` の要求が来た時点で
    /// `$/cancelRequest` を送って捨て、待っていた側は [`CompletionError::Superseded`] で返る
    /// （打鍵のたびに投げた古い要求の答えを待たない）。文書が開いていなければ定義ジャンプと
    /// 同じく問い合わせのあいだだけ `didOpen` する。サーバが読み込み中で空を返したら、打鍵の
    /// 要求も含めて済むのを待って問い直す（#1869。上限に当たれば [`CompletionError::Loading`]）
    pub fn completion(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionAnswer, CompletionError> {
        match &self.shared {
            Some(shared) => shared.completion(request),
            None => Err(GotoError::Disabled.into()),
        }
    }

    /// 打鍵の補完の取り消しの番号を**UI スレッドで**先に取る（#1909。列の前の要求は取り消す）。
    /// 要求の `ticket` に載せると、背景で番号を取り直さない = この後の `cancel_completion` を
    /// 追い越さない（ホバーの [`Self::reserve_hover`] と同じ口）。LSP を止めていれば `None`。
    /// **待たない**（ロックを短く取るだけ）
    pub fn reserve_completion(&self) -> Option<u64> {
        self.shared
            .as_ref()
            .map(|shared| shared.supersede(Lane::Completion))
    }

    /// 説明の補いの取り消しの番号を**UI スレッドで**先に取る（#1909。前の説明の要求は取り消す）。
    /// [`Self::resolve_completion`] の `ticket` に渡す。**待たない**
    pub fn reserve_resolve(&self) -> Option<u64> {
        self.shared
            .as_ref()
            .map(|shared| shared.supersede(Lane::Resolve))
    }

    /// 選んだ候補の説明を補う（`completionItem/resolve`。#1682）。**背景スレッドから呼ぶ**。
    /// 文書が開いている（編集中の）ときだけ。前の説明の要求は取り消す（選択を動かすたびに投げる）。
    /// `ticket` は UI が先に取った番号（[`Self::reserve_resolve`]。#1909）で、`None` なら背景で取る
    pub fn resolve_completion(
        &self,
        path: &Path,
        item: &Value,
        ticket: Option<u64>,
    ) -> Result<Value, CompletionError> {
        match &self.shared {
            Some(shared) => shared.resolve_completion(path, item, ticket),
            None => Err(GotoError::Disabled.into()),
        }
    }

    /// 打鍵の補完と説明の要求を捨てる（一覧を閉じた。#1682）。答えを待っていれば
    /// `$/cancelRequest` を送る。**待たない**（UI スレッドから呼んでよい）
    pub fn cancel_completion(&self) {
        if let Some(shared) = &self.shared {
            shared.supersede(Lane::Completion);
            shared.supersede(Lane::Resolve);
        }
    }

    /// 右クリックメニューに出す項目を決める能力（#1684）。**待たない**（右クリックのたびに
    /// UI スレッドから呼ぶ。ロックを短く取り、ルートの検出 = 数十回の stat だけ）。
    ///
    /// 開いている文書ならその文書のサーバ、開いていなければ文書を開いたときと同じ規則
    /// （検出表 + ルート）で決まるサーバの申告を読む。**サーバを起こさない**
    pub fn menu_capabilities_now(&self, path: &Path) -> MenuCapabilities {
        match &self.shared {
            Some(shared) => shared.menu_capabilities_now(path),
            None => MenuCapabilities::Failed(GotoError::Disabled),
        }
    }

    /// 同じく、まだ握手していなければ**問い合わせのあいだだけ**文書を開いてサーバを起こし、
    /// 握手を待つ（#1684。**背景スレッドから呼ぶ**。上限は `request.timeout`）。
    /// 起こし方は定義ジャンプと同じ（設計書 §16-2: 利用者が明示的に問い合わせたときだけ起こす）
    pub fn menu_capabilities(
        &self,
        request: &MenuRequest,
    ) -> Result<(&'static str, Value), GotoError> {
        match &self.shared {
            Some(shared) => shared.menu_capabilities(request),
            None => Err(GotoError::Disabled),
        }
    }

    /// その文書を受け持つサーバが申告した補完のきっかけの文字（`.` / `:` 等。#1682）。
    /// まだ握手していなければ空。**待たない**（打鍵のたびに UI スレッドから呼ぶ）
    pub fn completion_trigger_characters(&self, path: &Path) -> Vec<String> {
        let Some(shared) = &self.shared else {
            return Vec::new();
        };
        let uri = tako_core::file_uri::from_path(path);
        let inner = shared.lock();
        inner
            .docs
            .get(&uri)
            .and_then(|doc| inner.servers.get(&doc.key))
            .and_then(|slot| slot.capabilities.as_ref())
            .map(comp::trigger_characters)
            .unwrap_or_default()
    }
}

/// 編集セッションを持つプレビューペイン 1 つと、言語サーバとのつながり（#1679）。
///
/// ペイン → 文書の対応を知っているのは GUI（`EditState` が [`DocLink`] を持つ）なので、
/// `ControlHost::lsp_documents` がこれを返し、`tako lsp diagnostics` はこれで
/// ペインから URI を引いて [`LspManager::document_diagnostics`] を読む
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocument {
    pub pane: u64,
    pub path: String,
    pub editing: bool,
    /// 開いている文書の URI（[`DocLink::Open`]）。つながっていなければ `None`
    pub uri: Option<String>,
    /// そのペインがいま波線として描いている診断の数（GUI の写し = `EditState::diagnostics`。#1769）。
    /// 表（manager）の数と同じになっていれば、そのペインに診断が出ている
    pub drawn: usize,
}

impl LspDocument {
    pub fn new(pane: u64, path: String, editing: bool, link: &DocLink) -> Self {
        let uri = match link {
            DocLink::Open(lease) => Some(lease.uri().to_string()),
            _ => None,
        };
        Self {
            pane,
            path,
            editing,
            uri,
            drawn: 0,
        }
    }

    /// 描いている診断の数を添える（#1769）
    pub fn with_drawn(mut self, drawn: usize) -> Self {
        self.drawn = drawn;
        self
    }
}

/// 開いている文書 1 つの診断と、受け持つサーバ（[`LspManager::document_diagnostics`]）
#[derive(Debug, Clone)]
pub struct DocumentDiagnostics {
    /// 検出表の ID
    pub server: &'static str,
    /// サーバの状態（`tako lsp status` の `state` と同じ綴り）
    pub server_state: &'static str,
    pub diagnostics: DocDiagnostics,
}

/// 編集している文書を受け持つサーバの、画面に出す状態（[`LspManager::document_server`]。#1944）
#[derive(Debug, Clone)]
pub struct DocumentServer {
    /// 検出表の ID
    pub id: &'static str,
    pub state: ServerState,
    /// 稼働しているがプロジェクトを読み込み中（`tako lsp status` の `loading` と同じ判定）
    pub loading: bool,
    /// 取っている最中の様子（状態が未導入でも `tako lsp install` が取っていれば出る）
    pub fetch: Option<fetch::ProgressSnapshot>,
    /// 未導入・諦めた・止めたの理由と次の一手（`tako lsp status` と同じ文）
    pub reason: Option<String>,
    pub next_step: Option<String>,
    /// tako が取って入れられる（この OS / CPU 向けの取得物があり、取得を止めていない）
    pub can_install: bool,
    /// 取りに行って失敗した
    pub fetch_failed: bool,
}

/// 診断が変わった知らせを UI へ積む（満杯なら捨てて印を立てる。待たない）
fn notify_diagnostics(inner: &mut Inner, uri: &str) {
    let Some(tx) = inner.events.as_mut() else {
        return;
    };
    if let Err(error) = tx.try_send(uri.to_string()) {
        if error.is_full() {
            inner.events_overflowed = true;
        } else {
            // 受け手が居なくなった
            inner.events = None;
        }
    }
}

/// 状態が変わったことを UI へ知らせる（#1944。つながりの作り直し + 描き直し）
fn notify_refresh(inner: &mut Inner) {
    inner.ui_refresh_wanted = true;
    wake_ui(inner);
}

/// 取得の進捗が動いたことを UI へ知らせる（#1944。描き直しだけ）
fn notify_redraw(inner: &mut Inner) {
    wake_ui(inner);
}

/// UI を起こす（待たない。満杯 = もう起こしてあるので捨ててよい）
fn wake_ui(inner: &mut Inner) {
    let Some(tx) = inner.ui_wake.as_mut() else {
        return;
    };
    if let Err(error) = tx.try_send(()) {
        if error.is_disconnected() {
            inner.ui_wake = None;
        }
    }
}

/// そのサーバが受け持つ文書の診断を捨てる（サーバが止まった / 落ちた / 起こし直す。
/// 古いサーバの診断を次の publish まで残すと、直したのに消えない波線になる）
fn forget_diagnostics_of(inner: &mut Inner, key: &ServerKey) {
    let uris: Vec<String> = inner
        .docs
        .iter()
        .filter(|(_, d)| d.key == *key)
        .map(|(uri, _)| uri.clone())
        .collect();
    for uri in uris {
        if inner.diagnostics.forget(&uri) {
            notify_diagnostics(inner, &uri);
        }
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
        let holder = inner.next_holder;
        inner.next_holder = inner.next_holder.wrapping_add(1);
        let lease = |uri: String| {
            DocLink::Open(Arc::new(DocLease {
                shared: self.this.clone(),
                uri,
                holder,
            }))
        };
        // #1769: 同じファイルを別のペインが開いていれば、その文書の持ち手に加わるだけ
        // （didOpen は送らない。本文は自分が編集したときに送る = 相手の未保存の編集を上書きしない）
        if let Some(doc) = inner.docs.get_mut(&uri) {
            doc.holders.insert(holder, version);
            return lease(uri);
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
                holders: BTreeMap::from([(holder, version)]),
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
        lease(uri)
    }

    /// 開いている文書へ持ち手として加わる（本文は送らない。#1769 の共有と同じ）。
    /// 開いていなければ `None`（確かめと加わるのを 1 回のロックで行う = すれ違いで閉じた文書へ
    /// 空の本文で開き直さない）
    fn join(&self, uri: &str) -> Option<Arc<DocLease>> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let doc = inner.docs.get_mut(uri)?;
        let holder = inner.next_holder;
        inner.next_holder = inner.next_holder.wrapping_add(1);
        // 版は文書の今の版（この持ち手は同期しない = change を呼ばない）
        doc.holders
            .insert(holder, u64::try_from(doc.version).unwrap_or_default());
        Some(Arc::new(DocLease {
            shared: self.this.clone(),
            uri: uri.to_string(),
            holder,
        }))
    }

    fn change(&self, lease: &DocLease, text: &str, buffer_version: u64) {
        let uri = lease.uri();
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(doc) = inner.docs.get_mut(uri) else {
            return;
        };
        // 自分のバッファが変わっていなければ送らない（#1769: 変わっていないペインの同期 =
        // カーソル移動だけの呼び出しで、別のペインの編集を巻き戻さない）。取り込んだ版を
        // 覚えるのは写しへ取り込めたときだけ（送れなかった回は次の同期で送り直す）
        let holder = lease.holder;
        match doc.holders.get(&holder) {
            Some(&seen) if seen != buffer_version => {}
            _ => return,
        }
        if doc.text == text {
            doc.holders.insert(holder, buffer_version);
            return;
        }
        // 版は文書ごとに単調に進める（ペインが 1 つならバッファの版そのもの。2 つ目のペインの
        // バッファの版は若いことがあるので、下回るときは 1 つ進める）
        let version = to_lsp_version(buffer_version).max(doc.version.saturating_add(1));
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
            doc.holders.insert(holder, buffer_version);
            return;
        };
        if let Some(changes) = content_changes(kind, &doc.text, text) {
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
        doc.holders.insert(holder, buffer_version);
    }

    fn close(&self, uri: &str, holder: u64) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        // #1769: 持ち手を外す。まだ別のペインが開いていれば文書は開いたまま（didClose を送らない）
        let Some(doc) = inner.docs.get_mut(uri) else {
            return;
        };
        doc.holders.remove(&holder);
        if !doc.holders.is_empty() {
            return;
        }
        let Some(doc) = inner.docs.remove(uri) else {
            return;
        };
        if inner.diagnostics.forget(uri) {
            notify_diagnostics(inner, uri);
        }
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
        slot.quiescent = None;
        slot.running_since = None;
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
        let (launch, _) = self.resolve(spec);
        // #1944: 差し替えの env → 利用者の PATH → tako の置き場 → 取る の順。グローバルがあればそれ
        let (plan, source, program_path, runtime) = match launch {
            Launch::Found { plan, program_path } => {
                let overridden = std::env::var(servers::override_env_name(spec.id))
                    .is_ok_and(|v| !v.trim().is_empty());
                let source = if overridden { "override" } else { "path" };
                (plan, source, program_path, None)
            }
            // 見つからない / 打ち切った / 動かない は同じ「未導入」の扱い（理由の文だけが違う。#1769）
            missing => match self.start_managed(key, generation, spec, missing) {
                Some(managed) => (
                    managed.plan,
                    "managed",
                    managed.program_path,
                    managed.runtime,
                ),
                None => return,
            },
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
            slot.source = Some(source);
            slot.program_path = Some(program_path);
            slot.runtime = runtime;
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
        slot.running_since = Some(Instant::now());
        notify_refresh(inner);
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

    /// 見つからなかったサーバ: 置き場にあればそれを、無ければ取って起こし方を返す（#1944）。
    /// 取れない・取らない・取れなかった・取っているあいだに要らなくなったら状態を遷移させて `None`
    fn start_managed(
        &self,
        key: &ServerKey,
        generation: u64,
        spec: &'static ServerSpec,
        missing: Launch,
    ) -> Option<fetch::Managed> {
        let record = NotInstalled::from_launch(spec, missing);
        if let Some(config) = &self.fetch {
            if let Some(managed) = fetch::installed(config, spec) {
                return Some(managed);
            }
            // 前に取りに行って失敗していれば自動では取り直さない（オフラインで開くたびに取りに行かない。
            // restart / install / 画面の「もう一度」で取り直す）
            let failed_before = self
                .lock()
                .not_installed
                .get(spec.id)
                .is_some_and(|r| r.fetch_error.is_some());
            if config.auto && config.can_fetch(spec) && !failed_before {
                return self.fetch_and_start(config, key, generation, spec, record);
            }
        }
        // 取れない / 取らない: 未導入（#1678 の振る舞い。前の取得の失敗の記録は残す）
        let mut guard = self.lock();
        let inner = &mut *guard;
        let previous_error = inner
            .not_installed
            .get(spec.id)
            .and_then(|r| r.fetch_error.clone());
        let slot = inner.servers.get_mut(key)?;
        if slot.generation != generation {
            return None;
        }
        self.step(slot, Event::NotFound);
        crate::diag::persist_log(&match (record.timed_out, &record.broken) {
            (Some(secs), _) => format!("LSP 解決の打ち切り: server={} {secs} 秒", spec.id),
            (None, Some(_)) => format!("LSP PATH のものが動かない: server={}", spec.id),
            (None, None) => format!("LSP 未導入: server={}", spec.id),
        });
        inner.not_installed.insert(
            spec.id,
            NotInstalled {
                fetch_error: previous_error,
                ..record
            },
        );
        notify_refresh(inner);
        None
    }

    /// 取得中へ遷移して取り、取れたら起こし方を返す（起こすのは呼び手 = 同じ起動の流れ）
    fn fetch_and_start(
        &self,
        config: &FetchConfig,
        key: &ServerKey,
        generation: u64,
        spec: &'static ServerSpec,
        record: NotInstalled,
    ) -> Option<fetch::Managed> {
        {
            let mut guard = self.lock();
            let inner = &mut *guard;
            let slot = inner.servers.get_mut(key)?;
            if slot.generation != generation {
                return None;
            }
            self.step(slot, Event::FetchStarted);
            if slot.lifecycle.state != ServerState::Fetching {
                return None;
            }
            notify_refresh(inner);
        }
        crate::diag::persist_log(&format!("LSP 取得の開始: server={}", spec.id));
        let weak = self.this.clone();
        let result = fetch::ensure(config, spec, &move || {
            if let Some(shared) = weak.upgrade() {
                notify_redraw(&mut shared.lock());
            }
        });
        let mut guard = self.lock();
        let inner = &mut *guard;
        let has_docs = inner.docs.values().any(|d| d.key == *key);
        let slot = inner.servers.get_mut(key)?;
        if slot.generation != generation {
            return None;
        }
        match result {
            Ok(managed) => {
                let event = if has_docs {
                    Event::Fetched
                } else {
                    Event::FetchedUnused
                };
                self.step(slot, event);
                let starting = slot.lifecycle.state == ServerState::Starting;
                notify_refresh(inner);
                // 止めた（取っているあいだの stop）・閉じられた なら起こさない
                starting.then_some(managed)
            }
            Err(error) => {
                self.step(slot, Event::FetchFailed);
                crate::diag::persist_log(&format!(
                    "LSP 取得の失敗: server={} 種別={}",
                    spec.id,
                    error.slug()
                ));
                inner.not_installed.insert(
                    spec.id,
                    NotInstalled {
                        fetch_error: Some(error),
                        ..record
                    },
                );
                notify_refresh(inner);
                None
            }
        }
    }

    /// 未導入だったサーバが使えるようになった（取れた・PATH に現れた）: 記録を消し、待っている
    /// 未導入の器を起こす（#1944 / #1823 の 2）。断った文書（`DocLink::Declined`）は epoch を
    /// 進めるので、UI が次に同期したとき（状態の知らせ = [`LspManager::state_events`] で直ちに）開き直す
    fn on_installed(&self, id: &'static str) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        inner.not_installed.remove(id);
        // 解決のキャッシュも捨てる（PATH に現れたなら次の起動で拾う）
        inner.resolved.remove(id);
        inner.epoch = inner.epoch.wrapping_add(1);
        let keys: Vec<ServerKey> = inner
            .servers
            .iter()
            .filter(|(k, slot)| k.id == id && slot.lifecycle.state == ServerState::NotInstalled)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &keys {
            let has_docs = inner.docs.values().any(|d| d.key == *key);
            let Some(slot) = inner.servers.get_mut(key) else {
                continue;
            };
            if !has_docs {
                // 待っている文書が無い器は未起動へ戻すだけ（次に開いたときに起こす）
                slot.lifecycle = Lifecycle::default();
                continue;
            }
            if self.step(slot, Event::Installed) == Action::Spawn {
                self.begin_spawn(slot, key, None);
            }
        }
        crate::diag::persist_log(&format!(
            "LSP 導入を検知: server={id} 起こす器={} 件",
            keys.len()
        ));
        notify_refresh(inner);
    }

    fn install(&self, name: Option<&str>) -> Value {
        let Some(config) = self.fetch.clone() else {
            return json!({
                "action": "install",
                "enabled": false,
                "reason": text::FETCH_DISABLED_REASON.text(),
            });
        };
        let targets: Vec<&'static ServerSpec> = self
            .config
            .table
            .iter()
            .filter(|spec| match name {
                Some(n) => n == spec.id,
                None => spec.fetch.is_some(),
            })
            .collect();
        let rows: Vec<Value> = targets
            .into_iter()
            .map(|spec| self.install_one(&config, spec))
            .collect();
        json!({
            "action": "install",
            "enabled": true,
            "root": config.root.display().to_string(),
            "servers": rows,
        })
    }

    fn install_one(&self, config: &FetchConfig, spec: &'static ServerSpec) -> Value {
        let command = spec.install.command();
        if !config.can_fetch(spec) {
            return json!({
                "id": spec.id,
                "status": "unavailable",
                "reason": text::FETCH_UNSUPPORTED_REASON.text(),
                "next_step": text::fill(text::NOT_INSTALLED_NEXT_STEP, &[("command", command)]),
                "install_command": command,
            });
        }
        // グローバルに入っていればそれを使う（取らない）
        if let (Launch::Found { program_path, .. }, _) = self.resolve(spec) {
            return json!({ "id": spec.id, "status": "global", "path": program_path });
        }
        let present = fetch::present(config, spec);
        let started = Instant::now();
        let weak = self.this.clone();
        let result = fetch::ensure(config, spec, &move || {
            if let Some(shared) = weak.upgrade() {
                notify_redraw(&mut shared.lock());
            }
        });
        match result {
            Ok(managed) => {
                self.on_installed(spec.id);
                json!({
                    "id": spec.id,
                    "status": if present { "present" } else { "installed" },
                    "path": managed.program_path,
                    "runtime": managed.runtime,
                    "runtime_source": managed.runtime_source,
                    "dir": managed.dir.display().to_string(),
                    "disk_bytes": fetch::dir_size(&managed.dir),
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                })
            }
            Err(error) => {
                // 記録があれば理由を差し替える（無ければ作らない = 開いていない言語を断る記録を増やさない）
                {
                    let mut inner = self.lock();
                    if let Some(record) = inner.not_installed.get_mut(spec.id) {
                        record.fetch_error = Some(error.clone());
                    }
                    notify_refresh(&mut inner);
                }
                json!({
                    "id": spec.id,
                    "status": "failed",
                    "error_kind": error.slug(),
                    "reason": error.reason(),
                    "next_step": text::fill(
                        text::FETCH_FAILED_NEXT_STEP,
                        &[("name", spec.id), ("command", command)]
                    ),
                    "install_command": command,
                })
            }
        }
    }

    /// シェル統合の合図（cwd の変化・コマンドの終わり）: 未導入のサーバを引き直す（#1944 / #1823 の 2）。
    /// **待たない**（合図を出したスレッドを止めない。引き直しは背景のスレッド 1 本）
    fn on_shell_activity(&self) {
        let mut inner = self.lock();
        if inner.not_installed.is_empty() || inner.rechecking {
            return;
        }
        if let Some(last) = inner.last_recheck {
            let since = last.elapsed();
            if since < RECHECK_INTERVAL {
                // 間隔の中: 明けたときに 1 回だけ引き直す
                if !inner.recheck_pending {
                    inner.recheck_pending = true;
                    let weak = self.this.clone();
                    let wait = RECHECK_INTERVAL - since;
                    let _ = std::thread::Builder::new()
                        .name("lsp-recheck-wait".into())
                        .spawn(move || {
                            std::thread::sleep(wait);
                            if let Some(shared) = weak.upgrade() {
                                shared.lock().recheck_pending = false;
                                shared.on_shell_activity();
                            }
                        });
                }
                return;
            }
        }
        inner.rechecking = true;
        inner.last_recheck = Some(Instant::now());
        let ids: Vec<&'static str> = inner.not_installed.keys().copied().collect();
        drop(inner);
        let Some(shared) = self.arc() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("lsp-recheck".into())
            .spawn(move || {
                for id in ids {
                    let Some(spec) = servers::find_in(shared.config.table, id) else {
                        continue;
                    };
                    // キャッシュは合図の番号が変わったので引き直しになる
                    let usable = matches!(shared.resolve(spec).0, Launch::Found { .. })
                        || shared
                            .fetch
                            .as_ref()
                            .is_some_and(|config| fetch::present(config, spec));
                    if usable {
                        shared.on_installed(spec.id);
                    }
                }
                shared.lock().rechecking = false;
            });
    }

    fn document_server(&self, path: &Path) -> Option<DocumentServer> {
        let resolved = servers::resolve_in(self.config.table, path)?;
        let spec = resolved.spec;
        let progress = self
            .fetch
            .as_ref()
            .and_then(|config| fetch::progress_of(config, spec));
        let can_install = self
            .fetch
            .as_ref()
            .is_some_and(|config| config.can_fetch(spec));
        let uri = tako_core::file_uri::from_path(path);
        let inner = self.lock();
        let slot = inner
            .docs
            .get(&uri)
            .and_then(|doc| inner.servers.get(&doc.key));
        let record = inner.not_installed.get(spec.id);
        let state = match (slot, record) {
            (Some(slot), _) => slot.lifecycle.state,
            // 断った文書（未導入と分かったあとに開いた）は記録から
            (None, Some(_)) => ServerState::NotInstalled,
            (None, None) if progress.is_some() => ServerState::Fetching,
            (None, None) => return None,
        };
        let loading = slot.is_some_and(slot_loading);
        let (reason, next_step) = match state {
            ServerState::NotInstalled => {
                let fallback;
                let record = match record {
                    Some(record) => record,
                    None => {
                        fallback = NotInstalled::from_launch(
                            spec,
                            Launch::NotFound {
                                program: spec.program.to_string(),
                                override_env: None,
                            },
                        );
                        &fallback
                    }
                };
                let guidance = not_installed_guidance(spec, record, can_install);
                (
                    guidance["reason"].as_str().map(str::to_string),
                    guidance["next_step"].as_str().map(str::to_string),
                )
            }
            ServerState::GaveUp | ServerState::Stopped => {
                let slot = slot?;
                let (reason, next) = stopped_guidance(slot);
                (Some(reason), Some(next))
            }
            _ => (None, None),
        };
        Some(DocumentServer {
            id: spec.id,
            state,
            loading,
            fetch: progress,
            reason,
            next_step,
            can_install,
            fetch_failed: record.is_some_and(|r| r.fetch_error.is_some()),
        })
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
        // #1680: 読み込みが済んだか（定義ジャンプが空の答えを「見つからない」と読み違えないため）
        if method == super::goto::SERVER_STATUS_METHOD {
            let mut guard = self.lock();
            let inner = &mut *guard;
            if let Some(slot) = inner.servers.get_mut(key) {
                let quiescent = super::goto::quiescent_of(&params);
                let changed = slot.quiescent != quiescent;
                slot.quiescent = quiescent;
                // #1944: 画面の「読み込み中 → 動作中」を描き直す
                if changed {
                    notify_refresh(inner);
                }
            }
            return;
        }
        if method != "textDocument/publishDiagnostics" {
            // window/logMessage・$/progress 等は S1 では受けて捨てる
            return;
        }
        let mut guard = self.lock();
        let inner = &mut *guard;
        let docs = &inner.docs;
        // 桁は**サーバへ送った本文の写し**（= サーバが見ている本文）で UTF-8 へ写す
        let published = inner.diagnostics.publish(params, |uri| {
            let wanted = uri_key(uri);
            docs.iter()
                .find(|(_, d)| d.key == *key && d.uri_key == wanted)
                .map(|(ours, doc)| (ours.clone(), doc.text.as_str()))
        });
        if let Some(uri) = published {
            notify_diagnostics(inner, &uri);
        }
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
        forget_diagnostics_of(inner, key);
        let Some(slot) = inner.servers.get_mut(key) else {
            return;
        };
        if matches!(
            slot.lifecycle.state,
            ServerState::Starting | ServerState::Running
        ) {
            if slot.last_exit.is_none() {
                slot.last_exit = Some("exited".into());
            }
            self.on_crash(slot, key, generation);
            notify_refresh(inner);
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
                forget_diagnostics_of(inner, &key);
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
        // #1769: 解決のキャッシュも捨てる（入れた・PATH を直したあとの明示の引き直し）
        inner
            .resolved
            .retain(|id, _| name.is_some_and(|n| n != *id));
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
            forget_diagnostics_of(inner, &key);
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
        let mut inner = self.lock();
        // 取り消し合う要求（打鍵の補完・説明の補い・マウスのホバー）のうち、まだ答えを返していない数
        // （#1909。取り消した古い要求が読み込み待ちに残っていれば 0 に戻らない）
        let inflight: serde_json::Map<String, Value> = Lane::ALL
            .iter()
            .map(|&(lane, name)| (name.to_string(), json!(inner.lane(lane).running)))
            .collect();
        let servers: Vec<Value> = inner
            .servers
            .iter()
            .filter(|(k, _)| name.is_none_or(|n| n == k.id))
            .map(|(key, slot)| slot_status(&inner, self.fetch.as_ref(), key, slot))
            .collect();
        let mut value = json!({
            "enabled": true,
            "servers": servers,
            "documents": inner.docs.len(),
            "diagnostics_total": inner.diagnostics.total(),
            "inflight": inflight,
            // #1944: 未導入のサーバを取るか・置き場（取っている最中のものは各サーバの `fetch`）
            "fetch": fetch_summary(self.fetch.as_ref()),
        });
        if inner.servers.is_empty() {
            value["note"] = json!(text::IDLE_NOTE.text());
        }
        value
    }

    fn servers(&self) -> Value {
        let resolved = self.resolve_all(self.config.table);
        let rows: Vec<Value> = self
            .config
            .table
            .iter()
            .zip(resolved)
            .map(|(spec, (launch, cached))| {
                let extensions: Vec<&str> = spec.documents.iter().map(|d| d.extension).collect();
                let mut row = json!({
                    "id": spec.id,
                    "program": spec.program,
                    "extensions": extensions,
                    "install_command": spec.install.command(),
                    // #1769: 解決をキャッシュから答えたか（false = いま引いた）
                    "cached": cached,
                });
                let can_fetch = self.fetch.as_ref().is_some_and(|c| c.can_fetch(spec));
                // #1944: tako が取れるか・取った版（取れない行は `available: false`）
                row["fetch"] = json!({
                    "available": can_fetch,
                    "label": spec.fetch.map(|f| f.label(spec.id)),
                });
                match launch {
                    Launch::Found { program_path, .. } => {
                        row["installed"] = json!(true);
                        row["source"] =
                            json!(if std::env::var(servers::override_env_name(spec.id))
                                .is_ok_and(|v| !v.trim().is_empty())
                            {
                                "override"
                            } else {
                                "path"
                            });
                        row["path"] = json!(program_path);
                    }
                    missing => {
                        // グローバルに無くても tako の置き場にあれば使える（印の stat だけ。Node.js は
                        // 探さない = 2 回目以降の `servers` がログインシェルを起こさない #1769 を保つ）
                        let managed = self
                            .fetch
                            .as_ref()
                            .and_then(|config| fetch::managed_entry(config, spec));
                        if let Launch::TimedOut { .. } = &missing {
                            row["timed_out"] = json!(true);
                        }
                        if let Launch::Broken {
                            program_path,
                            detail,
                        } = &missing
                        {
                            row["broken"] = json!({ "path": program_path, "detail": detail });
                        }
                        match managed {
                            Some((program, dir)) => {
                                row["installed"] = json!(true);
                                row["source"] = json!("managed");
                                row["path"] = json!(program.display().to_string());
                                row["disk_bytes"] = json!(fetch::dir_size(&dir));
                            }
                            None => {
                                row["installed"] = json!(false);
                                let mut record = NotInstalled::from_launch(spec, missing);
                                record.fetch_error = self
                                    .lock()
                                    .not_installed
                                    .get(spec.id)
                                    .and_then(|r| r.fetch_error.clone());
                                merge(&mut row, not_installed_guidance(spec, &record, can_fetch));
                            }
                        }
                    }
                }
                row
            })
            .collect();
        json!({ "enabled": true, "servers": rows })
    }

    /// 実行ファイルの解決（#1769）。キャッシュが使えればそれを、無ければ引いて覚える。
    /// 戻り値の `bool` はキャッシュから答えたか
    fn resolve(&self, spec: &'static ServerSpec) -> (Launch, bool) {
        let activity = tako_core::shell_activity::epoch();
        if let Some(hit) = self.cached_launch(spec, activity) {
            return (hit, true);
        }
        // 解決は待ちうる（ログインシェル）のでロックの外で
        let launch = (self.config.launcher)(spec);
        self.lock().resolved.insert(
            spec.id,
            Resolved {
                launch: launch.clone(),
                activity,
            },
        );
        (launch, false)
    }

    fn cached_launch(&self, spec: &ServerSpec, activity: u64) -> Option<Launch> {
        self.lock()
            .resolved
            .get(spec.id)
            .filter(|r| r.fresh(activity))
            .map(|r| r.launch.clone())
    }

    /// 表のすべてを解決する。キャッシュに無いものは**並行して**引く
    /// （ログインシェルを 1 つずつ待つと 4 つぶんの合計になる）
    fn resolve_all(&self, specs: &'static [ServerSpec]) -> Vec<(Launch, bool)> {
        let activity = tako_core::shell_activity::epoch();
        let hits: Vec<Option<Launch>> = specs
            .iter()
            .map(|spec| self.cached_launch(spec, activity))
            .collect();
        std::thread::scope(|scope| {
            let handles: Vec<_> = specs
                .iter()
                .zip(hits)
                .map(|(spec, hit)| match hit {
                    Some(launch) => Err((launch, true)),
                    None => Ok(scope.spawn(move || self.resolve(spec))),
                })
                .collect();
            specs
                .iter()
                .zip(handles)
                .map(|(spec, handle)| match handle {
                    Err(hit) => hit,
                    Ok(handle) => handle.join().unwrap_or_else(|_| {
                        (
                            Launch::NotFound {
                                program: spec.program.to_string(),
                                override_env: None,
                            },
                            false,
                        )
                    }),
                })
                .collect()
        })
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

// --- 右クリックメニュー（#1684）---------------------------------------------

impl Shared {
    fn menu_capabilities_now(&self, path: &Path) -> MenuCapabilities {
        let Some(resolved) = servers::resolve_in(self.config.table, path) else {
            return MenuCapabilities::Failed(GotoError::NoServer);
        };
        let spec = resolved.spec;
        if let Some(error) = self.not_installed_error(spec) {
            return MenuCapabilities::Failed(error);
        }
        let uri = tako_core::file_uri::from_path(path);
        let opened = self.lock().docs.get(&uri).map(|doc| doc.key.clone());
        // 開いていなければ `open` と同じ規則でサーバを決める（ルートの検出はロックの外で）
        let key = opened.unwrap_or_else(|| ServerKey {
            id: spec.id,
            root: root::find_root(spec, path),
        });
        let inner = self.lock();
        let Some(slot) = inner.servers.get(&key) else {
            return MenuCapabilities::Pending;
        };
        match (slot.lifecycle.state, &slot.capabilities) {
            (ServerState::Running, Some(capabilities)) => MenuCapabilities::Ready {
                server: spec.id,
                capabilities: capabilities.clone(),
            },
            (ServerState::GaveUp | ServerState::Stopped, _) => {
                MenuCapabilities::Failed(GotoError::Unavailable {
                    server: spec.id,
                    state: slot.lifecycle.state,
                    crashes: slot.lifecycle.crashes,
                })
            }
            _ => MenuCapabilities::Pending,
        }
    }

    fn menu_capabilities(&self, request: &MenuRequest) -> Result<(&'static str, Value), GotoError> {
        match self.menu_capabilities_now(&request.path) {
            MenuCapabilities::Ready {
                server,
                capabilities,
            } => return Ok((server, capabilities)),
            MenuCapabilities::Failed(error) => return Err(error),
            MenuCapabilities::Pending => {}
        }
        let deadline = Instant::now() + request.timeout;
        let Some(resolved) = servers::resolve_in(self.config.table, &request.path) else {
            return Err(GotoError::NoServer);
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(&request.path);
        // 開いていなければ問い合わせのあいだだけ開く（定義ジャンプ・整形・補完と同じ 1 本）
        let _transient =
            self.open_for_request(&request.path, &uri, spec, request.document.as_deref())?;
        let (_, capabilities, _) =
            self.wait_ready(&uri, spec, request.timeout, deadline, &|| false)?;
        Ok((spec.id, capabilities))
    }
}

// --- 定義ジャンプ（#1680）---------------------------------------------------

impl Shared {
    fn goto(&self, request: &GotoRequest) -> Result<GotoAnswer, GotoError> {
        let deadline = Instant::now() + request.timeout;
        let Some(resolved) = servers::resolve_in(self.config.table, &request.path) else {
            return Err(GotoError::NoServer);
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(&request.path);
        // 開いていなければ問い合わせのあいだだけ開く（lease が落ちると didClose）
        let _transient =
            self.open_for_request(&request.path, &uri, spec, request.document.as_deref())?;
        let (process, capabilities, key) =
            self.wait_ready(&uri, spec, request.timeout, deadline, &|| false)?;
        if !tako_core::lsp::goto::server_supports(&capabilities, request.kind) {
            return Err(GotoError::Unsupported { server: spec.id });
        }
        // 問い合わせる位置は**サーバが見ている本文**（送った写し）で LSP の座標へ直す
        // （#1769: 単独の `\r` の後ろは LSP では次の行。画面の 1 行だけでは数えられない）
        let at = {
            let inner = self.lock();
            let Some(doc) = inner.docs.get(&uri) else {
                return Err(GotoError::Closed);
            };
            position::lsp_position_of_line_col(&doc.text, request.line, request.column)
        };
        let mut empty = super::goto::EmptyAnswer::default();
        loop {
            empty.sent(self.reported_loading(&key));
            let targets = self.goto_once(&process, &uri, spec, request, at, deadline)?;
            if !targets.is_empty() {
                return Ok(GotoAnswer {
                    server: spec.id,
                    targets,
                });
            }
            // 空の答え: 読み込み中のサーバ（rust-analyzer は読み込みの前の問い合わせに空で答える）
            // なら、済むのを待って問い直す。済んでいる / 状態を送らないサーバなら見つからない
            match self.wait_after_empty(&key, deadline, &|| false, &mut empty) {
                super::goto::Loading::Retry => continue,
                super::goto::Loading::Settled => {
                    return Ok(GotoAnswer {
                        server: spec.id,
                        targets,
                    })
                }
                super::goto::Loading::TimedOut => {
                    return Err(GotoError::Timeout {
                        server: spec.id,
                        secs: request.timeout.as_secs(),
                        starting: true,
                    })
                }
                // 定義ジャンプは待ちを打ち切らない（`|| false`）。届いたら閉じたのと同じに抜ける
                super::goto::Loading::Abandoned => return Err(GotoError::Closed),
            }
        }
    }

    /// 要求を 1 回投げて、答えを tako の座標へ直す
    fn goto_once(
        &self,
        process: &ServerProcess,
        uri: &str,
        spec: &'static ServerSpec,
        request: &GotoRequest,
        (line, character): (usize, usize),
        deadline: Instant,
    ) -> Result<Vec<super::goto::GotoTarget>, GotoError> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        let answer = process
            .request(request.kind.method(), params, remaining)
            .map_err(|e| rpc_failure(spec, request.timeout, e))?;
        Ok(tako_core::lsp::goto::parse_locations(&answer)
            .into_iter()
            .filter_map(|location| {
                let path = tako_core::lsp::goto::path_of_uri(&location.uri, cfg!(windows))?;
                let shadow = self.shadow_text(&location.uri);
                let disk = match &shadow {
                    Some(_) => None,
                    None => std::fs::read(&path)
                        .ok()
                        .map(|b| String::from_utf8_lossy(&b).into_owned()),
                };
                let source = shadow.as_deref().or(disk.as_deref());
                Some(super::goto::locate(
                    location.uri,
                    path,
                    location.line,
                    location.character,
                    source,
                ))
            })
            .collect())
    }

    /// 空の答えのあと、サーバの読み込みが済むのを待つ（上限つき）。
    ///
    /// 読み込み中と知らせていれば（`quiescent: false`）済むまで待って `Retry`。状態を
    /// まだ 1 度も知らせていなければ、握手の直後の猶予（[`super::goto::STATUS_GRACE`]）の
    /// あいだだけ知らせを待つ（送るサーバは握手の直後に送る）。済んでいる・送らないサーバは
    /// 待たずに `Settled`（= 本当に見つからない）。待つあいだに `abandoned` が真になったら
    /// `Abandoned`（#1869: 補完の打鍵の要求は次の打鍵・閉じる・文書を閉じるで抜ける）
    fn wait_loaded(
        &self,
        key: &ServerKey,
        deadline: Instant,
        abandoned: &dyn Fn() -> bool,
    ) -> super::goto::Loading {
        let mut waited = false;
        loop {
            if abandoned() {
                return super::goto::Loading::Abandoned;
            }
            if !self.loading_now(key) {
                return if waited {
                    super::goto::Loading::Retry
                } else {
                    super::goto::Loading::Settled
                };
            }
            if Instant::now() >= deadline {
                return super::goto::Loading::TimedOut;
            }
            waited = true;
            std::thread::sleep(super::goto::READY_POLL);
        }
    }

    /// 空の答えの後: 読み込みを待ち、問い直すかを決める（#1930。定義ジャンプ・補完・ホバーの
    /// 3 つが呼ぶ 1 実装）。待ち方は [`Self::wait_loaded`]、問い直しの判定は
    /// [`super::goto::EmptyAnswer`]（読み込み中に送った要求の空は、見る前に済んでいても 1 回だけ
    /// 問い直す）。`empty` は要求を送る直前に、サーバ自身が読み込み中と知らせているか
    /// （[`slot_reported_loading`]）で記録しておく
    fn wait_after_empty(
        &self,
        key: &ServerKey,
        deadline: Instant,
        abandoned: &dyn Fn() -> bool,
        empty: &mut super::goto::EmptyAnswer,
    ) -> super::goto::Loading {
        empty.decide(self.wait_loaded(key, deadline, abandoned))
    }

    /// サーバ自身が読み込み中と知らせているか（`quiescent: false`。**待たない**）。握手の直後の
    /// 猶予は含まない（状態を送らないサーバでは常に偽 = #1930 の問い直しが効かない）
    fn reported_loading(&self, key: &ServerKey) -> bool {
        self.lock()
            .servers
            .get(key)
            .is_some_and(slot_reported_loading)
    }

    /// サーバがいま読み込み中か（**待たない**。ロックを短く取るだけ）。
    ///
    /// `quiescent: false` を知らせている、または状態をまだ 1 度も知らせておらず握手の直後の
    /// 猶予（[`super::goto::STATUS_GRACE`]）の中なら読み込み中。[`Self::wait_loaded`] と
    /// 補完の「読み込み中」の答え（#1869）・`tako lsp status` の `loading` が同じ判定を引く
    fn loading_now(&self, key: &ServerKey) -> bool {
        self.lock().servers.get(key).is_some_and(slot_loading)
    }

    /// 未導入と分かっていれば、その理由と導入コマンド（#983）
    fn not_installed_error(&self, spec: &'static ServerSpec) -> Option<GotoError> {
        let inner = self.lock();
        let record = inner.not_installed.get(spec.id)?;
        let can_fetch = self.fetch.as_ref().is_some_and(|c| c.can_fetch(spec));
        let guidance = not_installed_guidance(spec, record, can_fetch);
        let field = |key: &str| guidance[key].as_str().unwrap_or_default().to_string();
        Some(GotoError::NotInstalled {
            server: spec.id,
            reason: field("reason"),
            next_step: field("next_step"),
            install_command: spec.install.command(),
        })
    }

    /// 文書を受け持つサーバが握手を終えて `didOpen` まで済むのを待つ（上限つき）
    fn wait_ready(
        &self,
        uri: &str,
        spec: &'static ServerSpec,
        timeout: Duration,
        deadline: Instant,
        abandoned: &dyn Fn() -> bool,
    ) -> Result<(Arc<ServerProcess>, Value, ServerKey), GotoError> {
        loop {
            // 待っているあいだに要らなくなった（#1682: 次の打鍵の補完に置き換わった）。
            // 呼び手が理由を見分ける（ここでは閉じたのと同じに抜ける）
            if abandoned() {
                return Err(GotoError::Closed);
            }
            {
                let inner = self.lock();
                let Some(doc) = inner.docs.get(uri) else {
                    return Err(GotoError::Closed);
                };
                if let Some(slot) = inner.servers.get(&doc.key) {
                    match slot.lifecycle.state {
                        ServerState::Running if doc.opened == Some(slot.generation) => {
                            if let (Some(process), Some(capabilities)) =
                                (&slot.process, &slot.capabilities)
                            {
                                return Ok((
                                    Arc::clone(process),
                                    capabilities.clone(),
                                    doc.key.clone(),
                                ));
                            }
                        }
                        ServerState::GaveUp | ServerState::Stopped => {
                            return Err(GotoError::Unavailable {
                                server: spec.id,
                                state: slot.lifecycle.state,
                                crashes: slot.lifecycle.crashes,
                            })
                        }
                        _ => {}
                    }
                }
            }
            if let Some(error) = self.not_installed_error(spec) {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err(GotoError::Timeout {
                    server: spec.id,
                    secs: timeout.as_secs(),
                    starting: true,
                });
            }
            std::thread::sleep(super::goto::READY_POLL);
        }
    }

    /// 問い合わせのために文書を開く（#1680 / #1683 / #1682）。既に開いていれば持ち手として加わる。
    ///
    /// 開いていなければ**問い合わせのあいだだけ**開く（返す持ち手が落ちると `didClose`）。本文は
    /// `document`（編集セッションの全文）、無ければディスクの中身。「開いただけではサーバを
    /// 起こさない」（設計書 §16-2）は保ったまま、利用者が明示的に問い合わせたときだけ起こす
    fn open_for_request(
        &self,
        path: &Path,
        uri: &str,
        spec: &'static ServerSpec,
        document: Option<&str>,
    ) -> Result<Option<Arc<DocLease>>, GotoError> {
        // 開いていれば持ち手として加わる（#1682）。加わらずに相乗りすると、同じ文書への
        // 問い合わせが 2 本重なったとき先に終わった側の一時の持ち手が didClose を送り、
        // 後の側が答えの直前で文書を失う
        if let Some(lease) = self.join(uri) {
            return Ok(Some(lease));
        }
        if let Some(error) = self.not_installed_error(spec) {
            return Err(error);
        }
        let source = match document {
            Some(document) => std::borrow::Cow::Borrowed(document),
            None => {
                let bytes = std::fs::read(path).map_err(|e| GotoError::Unreadable {
                    error: e.kind().to_string(),
                })?;
                std::borrow::Cow::Owned(String::from_utf8_lossy(&bytes).into_owned())
            }
        };
        match self.open(path, &source, 0) {
            // すれ違いで別のペインが開いていれば、その文書の持ち手に加わる（#1769）
            DocLink::Open(lease) => Ok(Some(lease)),
            // 断られた = 未導入の記録がある / 受け持つサーバが無い
            _ => Err(self
                .not_installed_error(spec)
                .unwrap_or(GotoError::NoServer)),
        }
    }

    /// 開いている文書ならサーバへ送った本文の写し（URI は照合用の正規化で突き合わせる）
    fn shadow_text(&self, uri: &str) -> Option<String> {
        let wanted = uri_key(uri);
        self.lock()
            .docs
            .values()
            .find(|d| d.uri_key == wanted)
            .map(|d| d.text.clone())
    }

    fn format(&self, request: &FormatRequest) -> Result<FormatAnswer, FormatError> {
        use tako_core::lsp::format as fmt;
        let deadline = Instant::now() + request.timeout;
        let Some(resolved) = servers::resolve_in(self.config.table, &request.path) else {
            return Err(FormatError::Lsp(GotoError::NoServer));
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(&request.path);
        let _transient = self
            .open_for_request(&request.path, &uri, spec, Some(&request.text))
            .map_err(FormatError::Lsp)?;
        let (process, capabilities, _) = self
            .wait_ready(&uri, spec, request.timeout, deadline, &|| false)
            .map_err(FormatError::Lsp)?;
        let ranged = request.range.is_some();
        if !fmt::server_supports(&capabilities, ranged) {
            return Err(FormatError::Unsupported {
                server: spec.id,
                ranged,
            });
        }
        // サーバが見ている本文を頼む本文へ揃える（打鍵の直後で同期がまだ / 同じファイルの
        // 別のペインが後から編集した。どちらも答えの座標が頼んだ本文とずれる）
        self.align_shadow(&uri, &request.text, spec)?;
        let params = fmt::params(&uri, &request.text, request.options, request.range.clone());
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        let answer = match process.request(fmt::method(ranged), params, remaining) {
            Ok(answer) => answer,
            Err(super::server::RpcError::Timeout(_)) => {
                return Err(FormatError::Lsp(GotoError::Timeout {
                    server: spec.id,
                    secs: request.timeout.as_secs(),
                    starting: false,
                }))
            }
            Err(super::server::RpcError::Server(e)) => {
                return Err(FormatError::Lsp(GotoError::ServerError {
                    server: spec.id,
                    code: e.code,
                    detail: e.message,
                }))
            }
            Err(_) => return Err(FormatError::Lsp(GotoError::Crashed { server: spec.id })),
        };
        // 待つあいだに写しが変わった（打鍵・別のペイン）= 答えの座標は今の本文に合わない
        if self.shadow_text(&uri).as_deref() != Some(request.text.as_str()) {
            return Err(FormatError::Stale);
        }
        let invalid = |detail: String| FormatError::InvalidEdits {
            server: spec.id,
            detail,
        };
        let changes =
            fmt::parse_text_edits(&answer, &request.text).map_err(|e| invalid(e.to_string()))?;
        let ordered = tako_core::text_edit::order_changes(&request.text, changes)
            .map_err(|e| invalid(e.to_string()))?;
        let (changes, dropped) = match &request.range {
            Some(range) => {
                fmt::restrict_to_range(&request.text, ordered, range).map_err(|outside| {
                    FormatError::OutsideRange {
                        server: spec.id,
                        count: outside.count,
                    }
                })?
            }
            None => (ordered, 0),
        };
        Ok(FormatAnswer {
            server: spec.id,
            changes,
            dropped,
        })
    }

    /// サーバが見ている本文（写し）を `text` に揃える（#1683）。揃っていれば何も送らない。
    ///
    /// 版は文書ごとに単調に 1 つ進める（#1769 の規則）。持ち手ごとの「取り込んだ版」は
    /// 触らない: 次にそのペインが同期したとき、本文が同じなら送らずに版だけ覚える
    fn align_shadow(
        &self,
        uri: &str,
        text: &str,
        spec: &'static ServerSpec,
    ) -> Result<(), FormatError> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(doc) = inner.docs.get_mut(uri) else {
            return Err(FormatError::Lsp(GotoError::Closed));
        };
        if doc.text == text {
            return Ok(());
        }
        let version = doc.version.saturating_add(1);
        let live = inner.servers.get(&doc.key).and_then(|slot| {
            (slot.lifecycle.state == ServerState::Running && doc.opened == Some(slot.generation))
                .then(|| slot.process.clone().map(|p| (p, slot.sync_kind)))
                .flatten()
        });
        if let Some((process, kind)) = live {
            if let Some(changes) = content_changes(kind, &doc.text, text) {
                let params = json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": changes,
                });
                if process.notify("textDocument/didChange", params).is_err() {
                    return Err(FormatError::Lsp(GotoError::Crashed { server: spec.id }));
                }
            }
        }
        replace_text(&mut doc.text, text);
        doc.version = version;
        Ok(())
    }
}

// --- 補完（#1682）-------------------------------------------------------------

impl Shared {
    /// 列の生きている要求を取り消して新しい番号を配る。前の要求が答えを待っていれば
    /// `$/cancelRequest` を送り、待っていた側を [`RpcError::Cancelled`] で起こす
    fn supersede(&self, lane: Lane) -> u64 {
        let (ticket, previous) = {
            let mut inner = self.lock();
            inner.next_ticket = inner.next_ticket.wrapping_add(1);
            let ticket = inner.next_ticket;
            // 走っている数（`running`）は置き換わった古い要求が抜けるまで数えるので残す
            let inflight = inner.lane(lane);
            inflight.ticket = ticket;
            (ticket, inflight.call.take())
        };
        // 取り消しはロックの外で（待ちの表は別のロック）
        if let Some(process) = previous.and_then(|(p, id)| p.upgrade().map(|p| (p, id))) {
            process.0.cancel_request(&process.1);
        }
        ticket
    }

    /// 取り消し合う要求（打鍵の補完・説明の補い・マウスのホバー）が背景で列へ入る**唯一の口**
    /// （#1893 / #1909）。UI が先に取った番号（`reserved` = `reserve_completion` / `reserve_resolve` /
    /// `reserve_hover`）があればそれを使い、無いときだけここで取る。**背景でここに着く前に** UI が
    /// 取り消していた（一覧を閉じた・語の外へ出た・語から外れた）なら、UI の番号は古くなっているので
    /// 問い合わせずに `Superseded` で抜ける。ここで取り直すと取り消しを追い越して自分が最新になり、
    /// 読み込みが済むまで待ち続ける。返した [`LaneCall`] が生きているあいだは `inflight` に数える
    fn enter_lane(&self, lane: Lane, reserved: Option<u64>) -> LaneCall<'_> {
        self.lock().lane(lane).running += 1;
        // 落とすと数から外れる（注入で止まっているあいだも数える = 背景が走り出したと分かる）
        let mut call = LaneCall {
            shared: self,
            lane,
            ticket: 0,
        };
        hold_for_injection();
        call.ticket = reserved.unwrap_or_else(|| self.supersede(lane));
        call
    }

    fn is_current(&self, lane: Lane, ticket: u64) -> bool {
        self.lock().lane(lane).ticket == ticket
    }

    /// 要求を送って答えを待つ。`lane` があれば列へ載せ、待っているあいだに置き換われば
    /// [`RpcError::Cancelled`] で返る（送る前に置き換わっていたら自分で取り消す）
    fn request_in_lane(
        &self,
        lane: Option<(Lane, u64)>,
        process: &Arc<ServerProcess>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let call = process.send_request(method, params, timeout)?;
        if let Some((lane, ticket)) = lane {
            let attached = {
                let mut inner = self.lock();
                let inflight = inner.lane(lane);
                if inflight.ticket == ticket {
                    inflight.call = Some((Arc::downgrade(process), call.id.clone()));
                    true
                } else {
                    false
                }
            };
            if !attached {
                process.cancel_request(&call.id);
                return Err(RpcError::Cancelled);
            }
        }
        let result = process.wait(call, timeout);
        if let Some((lane, ticket)) = lane {
            let mut inner = self.lock();
            let inflight = inner.lane(lane);
            if inflight.ticket == ticket {
                inflight.call = None;
            }
        }
        result
    }

    fn completion(&self, request: &CompletionRequest) -> Result<CompletionAnswer, CompletionError> {
        let deadline = Instant::now() + request.timeout;
        // 打鍵の要求は列へ入る（最新の 1 つだけを生かす）。番号は UI が先に取ったもの（#1909。
        // 背景で取り直すと一覧を閉じた取り消しを追い越す = ホバーと同じ口）
        let entered = request
            .superseding
            .then(|| self.enter_lane(Lane::Completion, request.ticket));
        let lane = entered.as_ref().map(LaneCall::key);
        let superseded = || lane.is_some_and(|(lane, ticket)| !self.is_current(lane, ticket));
        let Some(resolved) = servers::resolve_in(self.config.table, &request.path) else {
            return Err(GotoError::NoServer.into());
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(&request.path);
        // 開いていなければ問い合わせのあいだだけ開く（定義ジャンプ・整形と同じ 1 本）
        let transient =
            self.open_for_request(&request.path, &uri, spec, request.document.as_deref())?;
        // 起動を待つあいだに次の打鍵に置き換わったら待ちを抜ける（古いスレッドに上限まで
        // 状態を読ませ続けない）
        let ready = self.wait_ready(&uri, spec, request.timeout, deadline, &superseded);
        if superseded() {
            return Err(CompletionError::Superseded);
        }
        let (process, capabilities, key) = ready?;
        if !comp::server_supports(&capabilities) {
            return Err(GotoError::Unsupported { server: spec.id }.into());
        }
        let cursor = comp::At::new(request.line, request.column);
        let started = Instant::now();
        // サーバの読み込みを待ったか（#1869。答えの `waited_for_loading` = CLI / MCP が読み込み中
        // だったと分かる）: 空の答えの後に済むのを待った、か、送ったときにサーバ自身が読み込み中と
        // 知らせていた（rust-analyzer の後半は答えずに済むまで待たせる）
        let mut waited = false;
        // 読み込みを待つあいだに要らなくなったか（次の打鍵・一覧を閉じた・文書を閉じた）。
        // この問い合わせ自身も持ち手として加わっているので、「文書を閉じた」= 始めに居た**ほかの**
        // 持ち手（編集セッション）が全員抜けた（CLI が問い合わせのあいだだけ開いた文書は対象外）
        let own = transient.as_ref().map(|lease| lease.holder);
        let others_hold = || {
            self.lock()
                .docs
                .get(&uri)
                .is_some_and(|doc| doc.holders.keys().any(|holder| Some(*holder) != own))
        };
        let shared_at_start = others_hold();
        let abandoned = || superseded() || (shared_at_start && !others_hold());
        let mut empty = super::goto::EmptyAnswer::default();
        loop {
            // 問い合わせる位置は**サーバが見ている本文**（送った写し）で LSP の座標へ直す（#1769）。
            // 送った時点の文書の版を覚え、答えが届いたときに違っていたらその答えは使わない
            // （候補の範囲は送った本文の座標 = 打ち足した後の本文には当たらない）
            let (at, sent_version) = {
                let inner = self.lock();
                let Some(doc) = inner.docs.get(&uri) else {
                    return Err(GotoError::Closed.into());
                };
                let reported = inner.servers.get(&key).is_some_and(slot_reported_loading);
                empty.sent(reported);
                waited |= reported;
                (
                    position::lsp_position_of_line_col(&doc.text, request.line, request.column),
                    doc.version,
                )
            };
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": at.0, "character": at.1 },
                "context": request.trigger.context(),
            });
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1));
            let answer = match self.request_in_lane(
                lane,
                &process,
                "textDocument/completion",
                params,
                remaining,
            ) {
                Ok(answer) => answer,
                Err(RpcError::Cancelled) => return Err(CompletionError::Superseded),
                // 答えずに待たせたまま上限（rust-analyzer の読み込みの後半）: 読み込み中と分かる答えに
                Err(RpcError::Timeout(_)) if self.loading_now(&key) => {
                    return Err(CompletionError::Loading {
                        server: spec.id,
                        secs: request.timeout.as_secs(),
                    })
                }
                Err(e) => return Err(rpc_failure(spec, request.timeout, e).into()),
            };
            let parsed = comp::parse_response(&answer);
            // 空の答え: 読み込み中のサーバ（rust-analyzer は読み込みの前半の問い合わせに即座に null で
            // 答える = #1869 の実測）なら、済むのを待って問い直す（#1680 と同じ）。**打鍵の要求も待つ**
            // （#1869。待たずに 0 件で返すと GUI は一覧を出さず、済んでも問い直さなかった）。待つあいだに
            // 次の打鍵・一覧を閉じる・文書を閉じるで抜ける（古いスレッドを上限まで残さない）。
            // A/B（`TAKO_1869_LEGACY=1`）は #1869 前 = 打鍵の要求は待たない
            let wait = !request.superseding || !super::completion::legacy_1869();
            if parsed.items.is_empty() && wait {
                match self.wait_after_empty(&key, deadline, &abandoned, &mut empty) {
                    super::goto::Loading::Retry => {
                        waited = true;
                        continue;
                    }
                    super::goto::Loading::Settled => {}
                    super::goto::Loading::TimedOut => {
                        return Err(CompletionError::Loading {
                            server: spec.id,
                            secs: request.timeout.as_secs(),
                        })
                    }
                    super::goto::Loading::Abandoned if superseded() => {
                        return Err(CompletionError::Superseded)
                    }
                    super::goto::Loading::Abandoned => return Err(GotoError::Closed.into()),
                }
            }
            // 写しはロックの中で写し取り、座標の変換（行頭の索引 = 本文の長さに比例）は外で行う
            // （打鍵の同期 `sync` を待たせない）
            let text = {
                let inner = self.lock();
                let Some(doc) = inner.docs.get(&uri) else {
                    return Err(GotoError::Closed.into());
                };
                if doc.version != sent_version {
                    return Err(CompletionError::Edited);
                }
                doc.text.clone()
            };
            let is_incomplete = parsed.is_incomplete;
            let dropped = parsed.dropped;
            let mut items = comp::locate(parsed, &text, cursor);
            let line_text = comp::tako_line(&text, request.line).to_string();
            let resolvable = comp::resolve_supported(&capabilities);
            if request.resolve_top > 0 && resolvable {
                let order = comp::rank(&items, &line_text, cursor);
                let top: Vec<usize> = order.into_iter().take(request.resolve_top).collect();
                self.resolve_items(&process, &mut items, &top, deadline);
            }
            return Ok(CompletionAnswer {
                server: spec.id,
                items,
                is_incomplete,
                dropped,
                cursor,
                line_text,
                resolvable,
                waited_for_loading: waited.then(|| started.elapsed()),
            });
        }
    }

    /// 候補の説明を**まとめて**補う（CLI / MCP の `resolve`）。要求を先に全部送ってから順に待つ
    /// （1 つずつ往復を待たない）。届かなかった候補は説明なしのまま
    fn resolve_items(
        &self,
        process: &ServerProcess,
        items: &mut [comp::CompletionItem],
        indices: &[usize],
        deadline: Instant,
    ) {
        let remaining = || {
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1))
                .min(super::completion::RESOLVE_TIMEOUT)
        };
        let calls: Vec<(usize, super::server::PendingCall)> = indices
            .iter()
            .filter_map(|&i| {
                let raw = items.get(i)?.raw.clone();
                process
                    .send_request("completionItem/resolve", raw, remaining())
                    .ok()
                    .map(|call| (i, call))
            })
            .collect();
        for (i, call) in calls {
            if let Ok(resolved) = process.wait(call, remaining()) {
                comp::apply_resolved(&mut items[i], &resolved);
            }
        }
    }

    fn resolve_completion(
        &self,
        path: &Path,
        item: &Value,
        ticket: Option<u64>,
    ) -> Result<Value, CompletionError> {
        let timeout = super::completion::RESOLVE_TIMEOUT;
        let deadline = Instant::now() + timeout;
        // 番号は UI が先に取ったもの（#1909。打鍵の補完と同じ口）
        let entered = self.enter_lane(Lane::Resolve, ticket);
        let lane = Some(entered.key());
        let Some(resolved) = servers::resolve_in(self.config.table, path) else {
            return Err(GotoError::NoServer.into());
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(path);
        let current = || self.is_current(Lane::Resolve, entered.ticket);
        let ready = self.wait_ready(&uri, spec, timeout, deadline, &|| !current());
        if !current() {
            return Err(CompletionError::Superseded);
        }
        let (process, capabilities, _) = ready?;
        if !comp::resolve_supported(&capabilities) {
            return Err(GotoError::Unsupported { server: spec.id }.into());
        }
        match self.request_in_lane(
            lane,
            &process,
            "completionItem/resolve",
            item.clone(),
            timeout,
        ) {
            Ok(value) => Ok(value),
            Err(RpcError::Cancelled) => Err(CompletionError::Superseded),
            Err(e) => Err(rpc_failure(spec, timeout, e).into()),
        }
    }
}

// --- ホバー（#1681）-------------------------------------------------------------

impl Shared {
    fn hover(&self, request: &HoverRequest) -> Result<HoverAnswer, HoverError> {
        use tako_core::lsp::hover as hv;
        let deadline = Instant::now() + request.timeout;
        // マウスの要求は列へ入る（最新の 1 つだけを生かす。補完と同じ口）。UI が先に取った番号が
        // あればそれを使う（取り直すと UI の取り消しを追い越す。#1893）
        let entered = request
            .superseding
            .then(|| self.enter_lane(Lane::Hover, request.ticket));
        let lane = entered.as_ref().map(LaneCall::key);
        let superseded = || lane.is_some_and(|(lane, ticket)| !self.is_current(lane, ticket));
        let Some(resolved) = servers::resolve_in(self.config.table, &request.path) else {
            return Err(GotoError::NoServer.into());
        };
        let spec = resolved.spec;
        let uri = tako_core::file_uri::from_path(&request.path);
        // マウスは開いている文書に持ち手として加わるだけ（乗せただけでサーバを起こさない =
        // 設計書 §16-2）。明示の問い合わせは定義ジャンプ・整形・補完と同じ 1 本で開く
        let held = if request.open {
            self.open_for_request(&request.path, &uri, spec, request.document.as_deref())?
        } else {
            Some(self.join(&uri).ok_or(HoverError::NotOpen)?)
        };
        let ready = self.wait_ready(&uri, spec, request.timeout, deadline, &superseded);
        if superseded() {
            return Err(HoverError::Superseded);
        }
        let (process, capabilities, key) = ready?;
        if !hv::server_supports(&capabilities) {
            return Err(GotoError::Unsupported { server: spec.id }.into());
        }
        let started = Instant::now();
        // サーバの読み込みを待ったか（#1893。補完の #1869 と同じ = 答えの `waited_for_loading`）
        let mut waited = false;
        // 読み込みを待つあいだに要らなくなったか（次のマウスの要求・カードを閉じた・文書を閉じた）。
        // この問い合わせ自身も持ち手として加わっているので、「文書を閉じた」= 始めに居た**ほかの**
        // 持ち手（編集セッション）が全員抜けた（補完の `completion` と同じ組み立て）
        let own = held.as_ref().map(|lease| lease.holder);
        let others_hold = || {
            self.lock()
                .docs
                .get(&uri)
                .is_some_and(|doc| doc.holders.keys().any(|holder| Some(*holder) != own))
        };
        let shared_at_start = others_hold();
        let abandoned = || superseded() || (shared_at_start && !others_hold());
        let mut empty = super::goto::EmptyAnswer::default();
        loop {
            // 問い合わせる位置は**サーバが見ている本文**（送った写し）で LSP の座標へ直す（#1769）
            let at = {
                let inner = self.lock();
                let Some(doc) = inner.docs.get(&uri) else {
                    return Err(GotoError::Closed.into());
                };
                let reported = inner.servers.get(&key).is_some_and(slot_reported_loading);
                empty.sent(reported);
                waited |= reported;
                position::lsp_position_of_line_col(&doc.text, request.line, request.column)
            };
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": at.0, "character": at.1 },
            });
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1));
            let answer =
                match self.request_in_lane(lane, &process, "textDocument/hover", params, remaining)
                {
                    Ok(answer) => answer,
                    Err(RpcError::Cancelled) => return Err(HoverError::Superseded),
                    // 答えずに待たせたまま上限（rust-analyzer の読み込みの後半）: 読み込み中と分かる答えに
                    Err(RpcError::Timeout(_)) if self.loading_now(&key) => {
                        return Err(HoverError::Loading {
                            server: spec.id,
                            secs: request.timeout.as_secs(),
                        })
                    }
                    Err(e) => return Err(rpc_failure(spec, request.timeout, e).into()),
                };
            let content = hv::parse_response(&answer);
            // 空の答え: 読み込み中のサーバ（rust-analyzer は読み込みの前半に即座に null で答える）なら、
            // 済むのを待って問い直す（#1680 と同じ）。**マウスの要求も待つ**（#1893。補完の打鍵の
            // 要求 = #1869 と同じ manager の待ち）。待つあいだに次のマウスの要求・カードを閉じる・
            // 文書を閉じるで抜ける。A/B（`TAKO_1893_LEGACY=1`）は #1893 前 = マウスの要求は待たない
            let wait = !request.superseding || !super::hover::legacy_1893();
            if content.is_none() && wait {
                match self.wait_after_empty(&key, deadline, &abandoned, &mut empty) {
                    super::goto::Loading::Retry => {
                        waited = true;
                        continue;
                    }
                    super::goto::Loading::Settled => {}
                    super::goto::Loading::TimedOut => {
                        return Err(HoverError::Loading {
                            server: spec.id,
                            secs: request.timeout.as_secs(),
                        })
                    }
                    super::goto::Loading::Abandoned if superseded() => {
                        return Err(HoverError::Superseded)
                    }
                    super::goto::Loading::Abandoned => return Err(GotoError::Closed.into()),
                }
            }
            // 範囲の写しは写しの全文が要る（ロックの中で写し取り、変換は外で = 打鍵の同期を待たせない）
            let range = match content.as_ref().and_then(|c| c.range) {
                Some(range) => {
                    let text = self.lock().docs.get(&uri).map(|d| d.text.clone());
                    text.map(|text| hv::locate_range(range, &text))
                }
                None => None,
            };
            return Ok(HoverAnswer {
                server: spec.id,
                content,
                range,
                waited_for_loading: waited.then(|| started.elapsed()),
            });
        }
    }
}

/// 要求の失敗を言語サーバの状態の失敗へ（定義ジャンプと補完が共有する）
fn rpc_failure(spec: &'static ServerSpec, timeout: Duration, error: RpcError) -> GotoError {
    match error {
        RpcError::Timeout(_) => GotoError::Timeout {
            server: spec.id,
            secs: timeout.as_secs(),
            starting: false,
        },
        RpcError::Server(e) => GotoError::ServerError {
            server: spec.id,
            code: e.code,
            detail: e.message,
        },
        _ => GotoError::Crashed { server: spec.id },
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
            quiescent: None,
            running_since: None,
            source: None,
            program_path: None,
            runtime: None,
        }
    }
}

/// サーバ自身が読み込み中と知らせているか（[`Shared::reported_loading`] と、補完・ホバーが
/// 送る直前にロックの中で引く判定の 1 実装。#1930）
fn slot_reported_loading(slot: &Slot) -> bool {
    slot.quiescent == Some(false)
}

/// サーバがいま読み込み中か（#1680 / #1869）。`quiescent: false` を知らせている、または状態を
/// まだ 1 度も知らせておらず握手の直後の猶予（[`super::goto::STATUS_GRACE`]）の中
fn slot_loading(slot: &Slot) -> bool {
    match slot.quiescent {
        Some(ready) => !ready,
        None => slot
            .running_since
            .is_some_and(|t| t.elapsed() < super::goto::STATUS_GRACE),
    }
}

fn slot_status(
    inner: &Inner,
    fetch_config: Option<&FetchConfig>,
    key: &ServerKey,
    slot: &Slot,
) -> Value {
    let docs: Vec<&String> = inner
        .docs
        .iter()
        .filter(|(_, d)| d.key == *key)
        .map(|(uri, _)| uri)
        .collect();
    let diagnostics: usize = docs.iter().map(|uri| inner.diagnostics.count(uri)).sum();
    // 文書を開いている持ち手（編集セッション + 問い合わせ中の一時的な持ち手）の数（#1769）
    let views: usize = inner
        .docs
        .values()
        .filter(|d| d.key == *key)
        .map(|d| d.holders.len())
        .sum();
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
        "views": views,
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
        // サーバがプロジェクトを読み込み中か（#1869。補完の「読み込み中」と同じ判定）
        "loading": slot_loading(slot),
        // #1944: 起こした実行ファイルの出どころ（path / override / managed）とパス
        "source": slot.source,
        "program_path": slot.program_path,
        "runtime": slot.runtime,
        // #1944: 取っている最中の様子（取っていなければ null）
        "fetch": fetch_config
            .and_then(|config| fetch::progress_of(config, slot.spec))
            .map(|p| p.to_json()),
    });
    match slot.lifecycle.state {
        ServerState::NotInstalled => {
            let fallback = NotInstalled::from_launch(
                slot.spec,
                Launch::NotFound {
                    program: slot.spec.program.to_string(),
                    override_env: None,
                },
            );
            let record = inner.not_installed.get(key.id).unwrap_or(&fallback);
            let can_fetch = fetch_config.is_some_and(|c| c.can_fetch(slot.spec));
            merge(
                &mut value,
                not_installed_guidance(slot.spec, record, can_fetch),
            );
        }
        ServerState::GaveUp | ServerState::Stopped => {
            let (reason, next_step) = stopped_guidance(slot);
            value["reason"] = json!(reason);
            value["next_step"] = json!(next_step);
        }
        _ => {}
    }
    value
}

/// 諦めた・止めたの理由と次の一手（`tako lsp status` と画面の 1 行が同じ文を出す）
fn stopped_guidance(slot: &Slot) -> (String, String) {
    if slot.lifecycle.state == ServerState::GaveUp {
        (
            text::fill(
                text::GAVE_UP_REASON,
                &[("count", &slot.lifecycle.crashes.to_string())],
            ),
            text::GAVE_UP_NEXT_STEP.text().to_string(),
        )
    } else {
        (
            text::STOPPED_REASON.text().to_string(),
            text::STOPPED_NEXT_STEP.text().to_string(),
        )
    }
}

/// `tako lsp status` の `fetch`（#1944）
fn fetch_summary(config: Option<&FetchConfig>) -> Value {
    match config {
        Some(config) => json!({
            "enabled": true,
            "auto": config.auto,
            "root": config.root.display().to_string(),
            "base": config.base,
        }),
        None => json!({
            "enabled": false,
            "reason": text::FETCH_DISABLED_REASON.text(),
        }),
    }
}

/// 未導入の「理由 + 次の一手（導入コマンド）」（#983 の作法）。`timed_out` は解決の
/// ログインシェルを打ち切った秒数（#1769。見つからないのではなく、確かめられなかった）。
/// #1944: 取りに行って失敗していればその理由（+ 取り直し方）、取れるサーバなら `tako lsp install` を
/// 先に案内する。どちらも導入コマンド（今の案内）は残す
fn not_installed_guidance(spec: &ServerSpec, record: &NotInstalled, can_fetch: bool) -> Value {
    let command = spec.install.command();
    let program = record.program.as_str();
    if let Some(error) = &record.fetch_error {
        return json!({
            "reason": text::fill(text::FETCH_FAILED_REASON, &[("reason", &error.reason())]),
            "next_step": text::fill(
                text::FETCH_FAILED_NEXT_STEP,
                &[("name", spec.id), ("command", command)]
            ),
            "install_command": command,
            "fetch_error": error.slug(),
        });
    }
    let next_step = if can_fetch {
        text::fill(
            text::FETCHABLE_NEXT_STEP,
            &[("name", spec.id), ("command", command)],
        )
    } else {
        text::fill(text::NOT_INSTALLED_NEXT_STEP, &[("command", command)])
    };
    if let Some((path, detail)) = &record.broken {
        return json!({
            "reason": text::fill(text::BROKEN_GLOBAL_NOTE, &[("path", path), ("detail", detail)]),
            "next_step": next_step,
            "install_command": command,
        });
    }
    if let Some(secs) = record.timed_out {
        return json!({
            "reason": text::fill(
                text::RESOLVE_TIMEOUT_REASON,
                &[("program", program), ("secs", &secs.to_string())]
            ),
            "next_step": text::RESOLVE_TIMEOUT_NEXT_STEP.text(),
            "install_command": command,
        });
    }
    let reason = match &record.override_env {
        Some(env) => text::fill(
            text::OVERRIDE_INVALID_REASON,
            &[("env", env), ("path", program)],
        ),
        None => text::fill(text::NOT_INSTALLED_REASON, &[("program", program)]),
    };
    json!({
        "reason": reason,
        "next_step": next_step,
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

/// `didChange` の `contentChanges`（写し `old` → 今の本文 `new`）。同期しないサーバは `None`
fn content_changes(kind: SyncKind, old: &str, new: &str) -> Option<Vec<Value>> {
    match kind {
        SyncKind::None => None,
        SyncKind::Full => Some(vec![json!({ "text": position::wire_text(new) })]),
        SyncKind::Incremental => sync::diff_change(old, new).map(|change| {
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
    }
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
            // 単独の `\r` は `\n` に替えて送る（写しは替えない。#1769 = `position` の冒頭）
            "text": position::wire_text(&doc.text),
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

/// クライアントの能力。**実際に使うものだけ**を申告する（使わない能力を申告すると
/// サーバが無駄な仕事をする）。S1 は文書同期と診断の受信、#1680 が定義ジャンプの 4 種、
/// #1683 が整形、#1682 が補完、#1681 がホバーを足した
fn initialize_params(root_uri: &str, root_name: &str) -> Value {
    use lsp_types::{
        ClientCapabilities, ClientInfo, CompletionClientCapabilities, CompletionItemCapability,
        CompletionItemCapabilityResolveSupport, CompletionItemTag, CompletionListCapability,
        DocumentFormattingClientCapabilities, DocumentRangeFormattingClientCapabilities,
        GeneralClientCapabilities, GotoCapability, HoverClientCapabilities, InitializeParams,
        MarkupKind, PositionEncodingKind, PublishDiagnosticsClientCapabilities, TagSupport,
        TextDocumentClientCapabilities, TextDocumentSyncClientCapabilities, Uri, WorkspaceFolder,
    };
    // #1680: 定義ジャンプの 4 種。`LocationLink` を受けられる（識別子の範囲へ正確に着地する）
    let goto = || {
        Some(GotoCapability {
            dynamic_registration: Some(false),
            link_support: Some(true),
        })
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
            // #1680: 読み込みが済んだかを知らせてもらう（`experimental/serverStatus`。
            // 送るサーバだけが送り、知らないサーバは無視する）
            experimental: Some(super::goto::experimental_capabilities()),
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
                definition: goto(),
                declaration: goto(),
                type_definition: goto(),
                implementation: goto(),
                // #1683: 整形（文書全体 / 範囲）。複数範囲（`rangesSupport`）は使わない
                formatting: Some(DocumentFormattingClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                range_formatting: Some(DocumentRangeFormattingClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                // #1681: ホバー。Markdown を先に（カードは `render_block` で描く）。平文も受ける
                hover: Some(HoverClientCapabilities {
                    dynamic_registration: Some(false),
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                }),
                // #1682: 補完。スニペットは申告しない（跳び先の編集を持たない = 平文で入れる）。
                // 長い説明（documentation）だけを後から resolve で補う。型の要約（detail）と
                // 自動 import（additionalTextEdits）は resolve へ回させない = 一覧の答えに最初から
                // 載る（全行に型を出せる・確定を待たずに 1 回で入れられる。実測: detail を回すと
                // rust-analyzer は選んだ 1 行にしか型を返さなかった）
                completion: Some(CompletionClientCapabilities {
                    dynamic_registration: Some(false),
                    completion_item: Some(CompletionItemCapability {
                        snippet_support: Some(false),
                        commit_characters_support: Some(false),
                        documentation_format: Some(vec![
                            MarkupKind::PlainText,
                            MarkupKind::Markdown,
                        ]),
                        deprecated_support: Some(true),
                        preselect_support: Some(true),
                        tag_support: Some(TagSupport {
                            value_set: vec![CompletionItemTag::DEPRECATED],
                        }),
                        insert_replace_support: Some(true),
                        resolve_support: Some(CompletionItemCapabilityResolveSupport {
                            properties: vec!["documentation".into()],
                        }),
                        ..Default::default()
                    }),
                    context_support: Some(true),
                    completion_list: Some(CompletionListCapability {
                        item_defaults: Some(vec![
                            "editRange".into(),
                            "insertTextFormat".into(),
                            "data".into(),
                        ]),
                    }),
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
    fn 初期化の申告は_utf16_と文書同期と診断と定義ジャンプと整形と補完とホバーだけ() {
        let params = initialize_params("file:///w", "w");
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        let text_document = params["capabilities"]["textDocument"].as_object().unwrap();
        let mut keys: Vec<&String> = text_document.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "completion",
                "declaration",
                "definition",
                "formatting",
                "hover",
                "implementation",
                "publishDiagnostics",
                "rangeFormatting",
                "synchronization",
                "typeDefinition"
            ]
        );
        // #1682: スニペットは申告せず、resolve へ回すのは長い説明だけ（型の要約と自動 import は
        // 一覧に載せる）
        let item = &params["capabilities"]["textDocument"]["completion"]["completionItem"];
        assert_eq!(item["snippetSupport"], json!(false));
        assert_eq!(
            item["resolveSupport"]["properties"],
            json!(["documentation"])
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["completion"]["contextSupport"],
            json!(true)
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["definition"]["linkSupport"],
            json!(true)
        );
        // #1681: ホバーは Markdown を先に申告する（平文より Markdown を返させる）
        assert_eq!(
            params["capabilities"]["textDocument"]["hover"]["contentFormat"],
            json!(["markdown", "plaintext"])
        );
        assert_eq!(params["rootUri"], json!("file:///w"));
        assert_eq!(params["workspaceFolders"][0]["name"], json!("w"));
        assert_eq!(params["clientInfo"]["name"], json!("tako"));
    }

    #[test]
    fn 版は_i32_に収める() {
        assert_eq!(to_lsp_version(7), 7);
        assert_eq!(to_lsp_version(u64::MAX), i32::MAX);
    }

    /// #1944: PATH で見つかった物を表の `probe_args` で確かめ、落ちたら理由（stderr の 1 行目）を返す。
    /// rustup の代理は component が無くても PATH に居て、起こすと即座に落ちる（諦めた まで 3 回）
    #[cfg(unix)]
    #[test]
    fn path_の物が動かなければ理由を返す() {
        use std::os::unix::fs::PermissionsExt;
        use tako_core::lsp::fetch::{Archive, Asset, Digest, Fetch, PlatformAsset};
        let dir = std::env::temp_dir().join(format!(
            "tako-1944-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        let broken = script(
            "broken",
            "echo \"error: Unknown binary 'x' in official toolchain 'stable'\" >&2; exit 1",
        );
        let fine = script("fine", "echo 'x 1.0.0'");
        const ASSETS: &[PlatformAsset] = &[PlatformAsset {
            os: "macos",
            arch: "aarch64",
            asset: Asset {
                url: "https://example.test/x.gz",
                digest: Digest::Sha256("00"),
                archive: Archive::Gzip,
                size: 1,
            },
        }];
        let mut spec = servers::SERVERS[0];
        spec.fetch = Some(Fetch::Binary {
            version: "t",
            assets: ASSETS,
            probe_args: &["--version"],
        });
        let detail = probe_failure(&spec, &broken).expect("落ちる物は理由を返す");
        assert!(detail.contains("Unknown binary"), "{detail}");
        assert_eq!(probe_failure(&spec, &fine), None, "動く物は信じる");
        // 確かめ方を持たない行は起こさずに信じる（従来どおり）
        spec.fetch = None;
        assert_eq!(probe_failure(&spec, &broken), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

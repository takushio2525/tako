//! ipc — Layer 1 IPC サーバー（FR-2.2 の受け口）
//!
//! トランスポート（`.agent/architecture.md` プラットフォーム抽象 / 抽象境界 B3）:
//! - unix: Unix domain socket（パーミッション 0600 + セッション毎ランダムトークン）
//! - windows: named pipe（`\\.\pipe\tako-<ユーザー>`。実体は
//!   `platform::named_pipe`、既定 DACL + トークンの二段防御）
//!
//! 1 行 1 JSON の接続処理はトランスポート非依存（`conn` モジュール）で、
//! unix / windows の実装は「バイトストリームの確立」だけを持つ。
//!
//! スレッド構成: accept スレッド + 接続毎スレッド（ブロッキング IO）。
//! 接続スレッドはリクエストを futures channel で UI スレッドへ渡し、応答を同期で待つ。
//! dispatch の実行は受信側（UI のイベントループ。GPUI executor）で行うため、
//! ここでは tokio 等の非同期ランタイムを持ち込まない（Phase 0 の方針踏襲）。

use std::io;

use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use tako_core::PaneOrigin;

use crate::dispatch::DispatchError;
use crate::orchestrator::wait;
use crate::protocol::{error_code, Request};

/// UI 側へ渡す 1 リクエスト。`reply` へ dispatch の結果を返すと接続スレッドが応答を書く。
/// `origin` は新規生成ペインの生成主体（IPC 直 = Cli、MCP 経由 = Mcp）
pub struct IncomingRequest {
    pub request: Request,
    pub origin: PaneOrigin,
    pub reply: std::sync::mpsc::SyncSender<Result<serde_json::Value, DispatchError>>,
}

/// [`submit`] の失敗。受け口まで届かなかったのか、dispatch が断ったのかを分ける
/// （IPC の応答コードが変わる: 前者は `INTERNAL`、後者は dispatch の分類）
#[derive(Debug)]
pub enum SubmitError {
    /// 受け口が閉じている / 応答が返らなかった（アプリの終了中など）
    Unreachable(&'static str),
    /// dispatch（または非同期 run の開始）が断った
    Rejected(DispatchError),
}

impl SubmitError {
    /// IPC 応答の JSON-RPC エラーコード
    pub fn code(&self) -> i64 {
        match self {
            SubmitError::Unreachable(_) => error_code::INTERNAL,
            SubmitError::Rejected(e) => e.code(),
        }
    }
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Unreachable(message) => f.write_str(message),
            SubmitError::Rejected(e) => write!(f, "{e}"),
        }
    }
}

/// 要求を受け口（UI スレッドの dispatch）へ渡して応答を待つ **1 実装**（#1745）。
///
/// IPC の接続スレッド・HTTP MCP のハンドラスレッド・非同期 run の完了待ちスレッドが
/// すべてここを通る。**非同期 run の開始（`OrchestratorRunStart`）だけはここで受ける**:
/// spawn と完了待ちのポーリングは dispatch の往復なので UI スレッドの上では待てず、
/// 待つ係（ポーリングのスレッド）とレジストリは受け口を持つこのプロセス（GUI）に置く。
/// #1745 まではこの処理が HTTP MCP のハンドラにだけあり、stdio ブリッジ（IPC 経由）からは
/// 頼む口が無かった（`sync=true` を付けないと JSON-RPC エラーで返っていた）
pub fn submit(
    tx: &UnboundedSender<IncomingRequest>,
    request: Request,
    origin: PaneOrigin,
) -> Result<Value, SubmitError> {
    if let Some(opts) = wait::RunOptions::from_start_request(&request) {
        return start_run(tx, &opts, origin);
    }
    // #1993 / #1994: PC リソースの状況と順番待ちは UI の受け口へ渡さずここで答える（採取は
    // 背景スレッドの覚え、古ければこの接続のスレッドで採り直す）。UI が固まっていても
    // 状況を返し、ビルドの割り当ても進む
    if let Request::Queue { .. } = &request {
        return crate::queue::answer(&request).map_err(SubmitError::Rejected);
    }
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    tx.unbounded_send(IncomingRequest {
        request,
        origin,
        reply: reply_tx,
    })
    .map_err(|_| SubmitError::Unreachable("アプリ側の受け口が閉じている"))?;
    match reply_rx.recv() {
        Ok(result) => result.map_err(SubmitError::Rejected),
        Err(_) => Err(SubmitError::Unreachable("アプリ側から応答が返らなかった")),
    }
}

/// 非同期 run を開始する: spawn はこのスレッドから受け口へ頼み、完了待ちは
/// `wait::run_start` が立てるスレッドから同じ受け口へ頼む
fn start_run(
    tx: &UnboundedSender<IncomingRequest>,
    opts: &wait::RunOptions,
    origin: PaneOrigin,
) -> Result<Value, SubmitError> {
    let mut exec = |req: Request| submit(tx, req, origin).map_err(|e| e.to_string());
    let poll_tx = tx.clone();
    wait::run_start(&mut exec, opts, move || {
        Box::new(move |req: Request| submit(&poll_tx, req, origin).map_err(|e| e.to_string()))
    })
    .map_err(|e| SubmitError::Rejected(DispatchError::Operation(e)))
}

/// IPC サーバーのハンドル。drop で受け口を閉じ（accept スレッドを止めて listener を手放す。
/// #2136）、ソケットファイル（と #1441 の参照ファイル）を片付ける。張っていた接続は
/// 切らない（接続ごとのスレッドが自分のストリームを持っている）。
/// `endpoint` はペインのシェルへ `TAKO_SOCKET` として注入する
pub struct IpcServer {
    endpoint: String,
    /// data dir 側に置いた実体への参照（#1441。短縮したときだけ `Some`）
    #[cfg_attr(windows, allow(dead_code))]
    pointer: Option<std::path::PathBuf>,
    /// accept スレッドを止める口（#2136）
    accept: AcceptStop,
}

impl IpcServer {
    /// サーバーを起動する。受け取った各リクエストは `tx` 経由で UI スレッドへ届く。
    /// `token` はセッション共有の認証トークン（[`crate::generate_token`] で生成し、
    /// MCP サーバーとも共有する。FR-2.3.4）
    pub fn start(tx: UnboundedSender<IncomingRequest>, token: String) -> io::Result<Self> {
        Self::start_with(tx, token, false)
    }

    /// `prefer_temp_socket` = true は固定ソケット（`<data_dir>/tako.sock`）を候補にせず
    /// PID 入り一時パスで起動する。セカンダリモード（多重起動の後発。Issue #113）が
    /// プライマリの固定ソケットを unlink + bind で乗っ取らないための経路
    pub fn start_with(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix_imp::start(tx, token, prefer_temp_socket)
        }
        #[cfg(windows)]
        {
            windows_imp::start(tx, token, prefer_temp_socket)
        }
    }

    /// **明示したパス**で受け口を張る（順番待ちの調停役の隔離 = `TAKO_QUEUE_SOCKET`。#1994）。
    ///
    /// 置き場の決め方（`tako_core::ipc_socket::plan`）も #1441 の記録も通さない（本体の受け口の
    /// 診断を上書きしない）。生きた先客が居れば奪わずに失敗する。windows は `path` をパイプ名と読む
    pub fn start_at(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        path: &str,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix_imp::start_at(tx, token, path)
        }
        #[cfg(windows)]
        {
            windows_imp::start_at(tx, token, path)
        }
    }

    /// IPC エンドポイント（unix: ソケットパス、windows: パイプ名）
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // 受け口を閉じるのが先（起こす 1 本はソケットファイルを通って繋ぐ = 消す前に）
        if !legacy_2136() {
            self.accept.close(&self.endpoint);
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
            // 死んだ実体を指す参照を残さない（#1441）
            if let Some(pointer) = &self.pointer {
                let _ = std::fs::remove_file(pointer);
            }
        }
    }
}

/// #2136 の A/B（`TAKO_2136_LEGACY=1`）: drop で受け口を閉じない（修正前 = accept スレッドが
/// listener / 待ち受け中のパイプを握ったまま残り、捨てたサーバーごとに fd とスレッドが 1 つずつ残る）
fn legacy_2136() -> bool {
    std::env::var_os("TAKO_2136_LEGACY").is_some_and(|v| !v.is_empty())
}

/// drop が accept スレッドの終わりを待つ上限（#2136）。起こせた後は accept が即座に返るので
/// 普段は 1 ms もかからない。上限は drop（GUI の終了など）を固めないためだけにある
const ACCEPT_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// accept スレッドを止める口（#2136）。
///
/// accept スレッドは受け口（unix の listener / windows の待ち受け中のパイプ）を握ったまま
/// accept で眠っている。別スレッドから fd を閉じるのは fd 番号の再利用と競合するうえ、
/// macOS の `shutdown` は listen 中のソケットに ENOTCONN を返して accept を起こさない（実測）
/// ので、drop は旗を立てたうえで**自分の受け口へ 1 本繋いで起こす**。起きたスレッドは旗を見て
/// 抜け、受け口を手放してから [`AcceptGuard`] を落とす = `done` が切れる。drop はそれを
/// [`ACCEPT_STOP_WAIT`] まで待つ
struct AcceptStop {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::mpsc::Receiver<()>,
}

/// accept スレッド側の持ち物。**受け口を手放した後に drop する**（drop が `done` の切断で
/// 「閉じた」と知るため。順番が逆だと drop が閉じる前に戻る）
struct AcceptGuard {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _done: std::sync::mpsc::Sender<()>,
}

fn accept_stop_pair() -> (AcceptStop, AcceptGuard) {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    (
        AcceptStop {
            stop: stop.clone(),
            done: done_rx,
        },
        AcceptGuard {
            stop,
            _done: done_tx,
        },
    )
}

impl AcceptGuard {
    /// drop が止めにかかっている（いま受けた接続は起こすための 1 本 = 捨てて抜ける）
    fn stopping(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl AcceptStop {
    fn close(&self, endpoint: &str) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        // 起こせなかった（ソケットファイルを外から消された・スレッドが既に抜けた）なら待たない。
        // drop を固めないのが優先で、そのときスレッドは修正前と同じく残る
        if wake_accept(endpoint) {
            // 送る者は居ないので、返るのは「切れた」（= 閉じた）か上限だけ
            let _ = self.done.recv_timeout(ACCEPT_STOP_WAIT);
        }
    }
}

/// accept で眠っているスレッドを起こす 1 本を繋ぐ（繋いだ側はすぐ捨てる）
#[cfg(unix)]
fn wake_accept(endpoint: &str) -> bool {
    std::os::unix::net::UnixStream::connect(endpoint).is_ok()
}

/// accept で眠っているスレッドを起こす 1 本を繋ぐ（繋いだ側はすぐ捨てる）。
/// 全インスタンスが使用中なら次のインスタンスが立つまで少し待つ
#[cfg(windows)]
fn wake_accept(endpoint: &str) -> bool {
    crate::platform::named_pipe::connect_client(endpoint, 1_000).is_ok()
}

/// トランスポート非依存の接続処理（1 行 1 JSON のリクエスト / レスポンス）。
/// unix / windows の実装はストリームの読み書き半分をここへ渡すだけ
mod conn {
    use std::io::{BufRead, BufReader, BufWriter, Read, Write};

    use futures::channel::mpsc::UnboundedSender;
    use tako_core::PaneOrigin;

    use super::IncomingRequest;
    use crate::protocol::{error_code, RequestEnvelope, ResponseEnvelope};

    /// 1 接続を処理する。ストリームが閉じるかエラーで戻る
    pub(super) fn handle_connection<R: Read, W: Write>(
        read_half: R,
        write_half: W,
        token: &str,
        tx: &UnboundedSender<IncomingRequest>,
    ) {
        let reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(write_half);
        for line in reader.lines() {
            let Ok(line) = line else {
                return;
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = process_line(&line, token, tx);
            let Ok(json) = serde_json::to_string(&response) else {
                return;
            };
            if writeln!(writer, "{json}")
                .and_then(|_| writer.flush())
                .is_err()
            {
                return;
            }
        }
    }

    fn process_line(
        line: &str,
        token: &str,
        tx: &UnboundedSender<IncomingRequest>,
    ) -> ResponseEnvelope {
        let envelope: RequestEnvelope = match serde_json::from_str(line) {
            Ok(envelope) => envelope,
            Err(e) => {
                return ResponseEnvelope::err(
                    0,
                    error_code::PARSE,
                    format!("リクエストを解釈できない: {e}"),
                )
            }
        };
        if envelope.token != token {
            // トークン不一致 = アプリ外プロセスからの接続（FR-2.3.4）。詳細は返さない
            return ResponseEnvelope::err(
                envelope.id,
                error_code::AUTH,
                "認証に失敗した（TAKO_TOKEN が一致しない）",
            );
        }
        // MCP stdio ブリッジ（`tako mcp serve`）経由のリクエストは origin = Mcp として扱う
        let origin = match envelope.origin.as_deref() {
            Some("mcp") => PaneOrigin::Mcp,
            _ => PaneOrigin::Cli,
        };
        // 受け口へ渡すのは HTTP MCP と同じ 1 実装（非同期 run の開始もここで受ける = #1745）
        match super::submit(tx, envelope.request, origin) {
            Ok(result) => ResponseEnvelope::ok(envelope.id, result),
            Err(e) => ResponseEnvelope::err(envelope.id, e.code(), e.to_string()),
        }
    }
}

#[cfg(windows)]
mod windows_imp {
    use futures::channel::mpsc::UnboundedSender;

    use super::{accept_stop_pair, conn, IncomingRequest, IpcServer};
    use crate::platform::named_pipe;

    /// PID + 連番の一時パイプ名（テスト・セルフテスト・セカンダリモード用）
    fn temp_pipe_name() -> String {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\tako-{}-{}-{seq}", user_tag(), std::process::id())
    }

    /// パイプ名の名前空間はマシン全体で共有のため、ユーザー名で分離する
    fn user_tag() -> String {
        std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
            .collect()
    }

    /// 立たなかった理由を記録して、そのまま呼び出し元へ返す（#1441）
    fn record_failure(e: std::io::Error) -> std::io::Error {
        tako_core::ipc_socket::record(tako_core::ipc_socket::IpcStatus {
            endpoint: None,
            kind: tako_core::ipc_socket::SocketPathKind::NamedPipe,
            bound: false,
            path_bytes: 0,
            limit: 0,
            well_known_bytes: 0,
            error: Some(e.to_string()),
        });
        e
    }

    /// 再起動をまたいで安定するパイプ名（unix の固定ソケットパスに相当）
    fn preferred_pipe_name(prefer_temp: bool) -> String {
        if prefer_temp || cfg!(test) || std::env::var_os("TAKO_SELF_TEST").is_some() {
            return temp_pipe_name();
        }
        format!(r"\\.\pipe\tako-{}", user_tag())
    }

    pub(super) fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> std::io::Result<IpcServer> {
        let mut name = preferred_pipe_name(prefer_temp_socket);
        // 固定名が既に占有されている（先行インスタンス生存）なら一時名へフォールバック
        // （unix の「固定ソケットへ接続可能なら一時パス」と同じ方針）
        let first = match named_pipe::create_server_instance(&name, true) {
            Ok(instance) => instance,
            Err(_) if !prefer_temp_socket => {
                name = temp_pipe_name();
                match named_pipe::create_server_instance(&name, true) {
                    Ok(instance) => instance,
                    Err(e) => return Err(record_failure(e)),
                }
            }
            Err(e) => return Err(record_failure(e)),
        };
        // #1441: 立った受け口を記録する（`check_health` が読む。名前付きパイプに
        // `sun_path` の上限は無いので長さは 0 で申告する）
        tako_core::ipc_socket::record(tako_core::ipc_socket::IpcStatus {
            endpoint: Some(name.clone()),
            kind: tako_core::ipc_socket::SocketPathKind::NamedPipe,
            bound: true,
            path_bytes: 0,
            limit: 0,
            well_known_bytes: 0,
            error: None,
        });

        serve(first, name, tx, token)
    }

    /// 明示したパイプ名で張る（#1994。先客が居れば失敗する）
    pub(super) fn start_at(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        name: &str,
    ) -> std::io::Result<IpcServer> {
        let first = named_pipe::create_server_instance(name, true)?;
        serve(first, name.to_string(), tx, token)
    }

    fn serve(
        first: named_pipe::ServerInstance,
        name: String,
        tx: UnboundedSender<IncomingRequest>,
        token: String,
    ) -> std::io::Result<IpcServer> {
        let accept_name = name.clone();
        let accept_token = token;
        let (accept, guard) = accept_stop_pair();
        std::thread::Builder::new()
            .name("tako-ipc-accept".into())
            .spawn(move || {
                let mut next = Some(first);
                loop {
                    let instance = match next.take() {
                        Some(instance) => instance,
                        None => match named_pipe::create_server_instance(&accept_name, false) {
                            Ok(instance) => instance,
                            Err(e) => {
                                tracing::warn!("IPC パイプインスタンスを作れない: {e}");
                                break;
                            }
                        },
                    };
                    let waited = instance.wait_client();
                    // #2136: drop が起こした。次のインスタンスを作らずに抜ける（= 待ち受けが消える）
                    if guard.stopping() {
                        break;
                    }
                    match waited {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let token = accept_token.clone();
                            let result = std::thread::Builder::new()
                                .name("tako-ipc-conn".into())
                                .spawn(move || match stream.try_clone() {
                                    Ok(read_half) => {
                                        conn::handle_connection(read_half, stream, &token, &tx)
                                    }
                                    Err(e) => tracing::warn!("IPC 接続の複製に失敗: {e}"),
                                });
                            if let Err(e) = result {
                                tracing::warn!("IPC 接続スレッドを起動できない: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("IPC accept に失敗: {e}"),
                    }
                }
                // 起こした 1 本（`waited`）はループの中で捨て終えている。待ち受けの
                // インスタンスはもう無いので、ここで drop へ知らせる
                drop(guard);
            })?;

        Ok(IpcServer {
            endpoint: name,
            pointer: None,
            accept,
        })
    }
}

#[cfg(unix)]
mod unix_imp {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    use futures::channel::mpsc::UnboundedSender;
    use tako_core::ipc_socket::{IpcStatus, SocketPathKind, SocketPlan};

    use super::{accept_stop_pair, conn, IncomingRequest, IpcServer};

    /// PID ベースの一時ソケットパス（テスト・セルフテスト・多重起動フォールバック用）
    fn temp_socket_path() -> SocketPlan {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tako-{}-{seq}.sock", std::process::id()));
        SocketPlan {
            path,
            kind: SocketPathKind::Temp,
            pointer: None,
            limit: tako_core::ipc_socket::max_path_bytes(),
            well_known_bytes: 0,
        }
    }

    /// 再起動をまたいで安定するソケットパスを決定する。
    /// - 単体テスト / セルフテスト / セカンダリモード（`prefer_temp`）: 一時パス
    ///   （他インスタンスと衝突回避・プライマリの固定ソケットを乗っ取らない）
    /// - 通常起動: [`tako_core::ipc_socket::plan`] の決め方（浅い data dir なら
    ///   `<data_dir>/tako.sock` のまま。深ければ短いパスへ逃がす = #1441）
    /// - 別インスタンスが生きている: フォールバックで一時パス
    ///
    /// **置き場の決め方をここへ書かない**（#1441）。同じ規則は繋ぐ側の診断
    /// （`tako check-health` のオフライン経路）も引くので、1 実装に保つ
    fn preferred_socket_path(prefer_temp: bool) -> SocketPlan {
        if prefer_temp || cfg!(test) || std::env::var_os("TAKO_SELF_TEST").is_some() {
            return temp_socket_path();
        }
        let Some(plan) = tako_core::ipc_socket::plan() else {
            return temp_socket_path();
        };
        // 生きた先客が居るパスは奪わない。**短縮パスにも同じ判定を掛ける**
        // （短縮パスは data dir ごとに安定 = 別インスタンスと同じ値になりうる）
        if plan.path.exists() && UnixStream::connect(&plan.path).is_ok() {
            return temp_socket_path();
        }
        plan
    }

    pub(super) fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> std::io::Result<IpcServer> {
        let plan = preferred_socket_path(prefer_temp_socket);
        match bind(&plan) {
            Ok(listener) => {
                // #1441: 立った受け口を記録する（`check_health` / 通知欄が読む）
                tako_core::ipc_socket::record(IpcStatus {
                    endpoint: Some(plan.path.display().to_string()),
                    kind: plan.kind,
                    bound: true,
                    path_bytes: plan.path_bytes(),
                    limit: plan.limit,
                    well_known_bytes: plan.well_known_bytes,
                    error: None,
                });
                // data dir 側には実体への参照だけ残す（診断の手がかり）。
                // 書けなかったら drop で消す対象にも入れない（他所が書いた物を消さない）
                let pointer = match tako_core::ipc_socket::write_pointer(&plan) {
                    Ok(()) => plan.pointer,
                    Err(_) => None,
                };
                serve(listener, tx, token, plan.path, pointer)
            }
            Err(e) => {
                // #1441: 立たなかった理由も記録する。**黙って縮退させない**
                tako_core::ipc_socket::record(IpcStatus {
                    endpoint: None,
                    kind: plan.kind,
                    bound: false,
                    path_bytes: plan.path_bytes(),
                    limit: plan.limit,
                    well_known_bytes: plan.well_known_bytes,
                    error: Some(e.to_string()),
                });
                Err(e)
            }
        }
    }

    /// 明示したパスで張る（#1994）。生きた先客が居れば奪わない
    pub(super) fn start_at(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        path: &str,
    ) -> std::io::Result<IpcServer> {
        let path = std::path::PathBuf::from(path);
        if path.exists() && UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{} には生きた受け口が居る", path.display()),
            ));
        }
        let plan = SocketPlan {
            path: path.clone(),
            kind: SocketPathKind::Temp,
            pointer: None,
            limit: tako_core::ipc_socket::max_path_bytes(),
            well_known_bytes: 0,
        };
        let listener = bind(&plan)?;
        serve(listener, tx, token, path, None)
    }

    /// ソケットを張る（親ディレクトリ作成 → 残骸除去 → bind → 0600）
    fn bind(plan: &SocketPlan) -> std::io::Result<UnixListener> {
        let path = &plan.path;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // 前回残骸（クラッシュ等で remove されなかったもの）を除去。
        // 生きた先客は `preferred_socket_path` が先に避けているのでここには来ない
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        // 自ユーザーのプロセスのみ接続可能にする（トークンと二段の防御線）
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// accept スレッドを立てる
    fn serve(
        listener: UnixListener,
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        path: std::path::PathBuf,
        pointer: Option<std::path::PathBuf>,
    ) -> std::io::Result<IpcServer> {
        let accept_token = token;
        let (accept, guard) = accept_stop_pair();
        std::thread::Builder::new()
            .name("tako-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    // #2136: drop が起こした（受けたのは起こすための 1 本 = 捨てて抜ける）
                    if guard.stopping() {
                        break;
                    }
                    match stream {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let token = accept_token.clone();
                            let result = std::thread::Builder::new()
                                .name("tako-ipc-conn".into())
                                .spawn(move || match stream.try_clone() {
                                    Ok(read_half) => {
                                        conn::handle_connection(read_half, stream, &token, &tx)
                                    }
                                    Err(e) => tracing::warn!("IPC 接続の複製に失敗: {e}"),
                                });
                            if let Err(e) = result {
                                tracing::warn!("IPC 接続スレッドを起動できない: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("IPC accept に失敗: {e}"),
                    }
                }
                // listener（受け口の fd）を閉じてから drop へ知らせる
                drop(listener);
                drop(guard);
            })?;

        Ok(IpcServer {
            endpoint: path.display().to_string(),
            pointer,
            accept,
        })
    }
}

/// 非同期 run の開始を IPC 越しに頼む回帰テストの本体（#1745）。unix（ソケット）と
/// windows（名前付きパイプ）のテストが「1 往復する係」だけを差し替えて呼ぶ
#[cfg(test)]
mod run_start_over_ipc {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::channel::mpsc::UnboundedReceiver;
    use futures::StreamExt;
    use serde_json::{json, Value};

    use super::IncomingRequest;
    use crate::dispatch::DispatchError;
    use crate::orchestrator::wait;
    use crate::protocol::{Request, RequestEnvelope, ResponseEnvelope};

    /// UI スレッドの dispatch の代役。run が使う要求だけに答え、受けた種別を記録する。
    /// worker は `done` が立つまで busy と答える（開始が完了を待たないことを、
    /// 実時間ではなく「完了させる前に running が見える」で確かめるため）。
    /// run_status / run_result は本物の dispatch と同じく `wait` の関数を直に呼ぶ
    pub(super) struct FakeApp {
        seen: Arc<Mutex<Vec<String>>>,
        done: Arc<AtomicBool>,
    }

    pub(super) fn spawn_fake_app(mut rx: UnboundedReceiver<IncomingRequest>) -> FakeApp {
        let app = FakeApp {
            seen: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(AtomicBool::new(false)),
        };
        let (log, done) = (Arc::clone(&app.seen), Arc::clone(&app.done));
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(answer(incoming.request, &log, &done));
            }
        });
        app
    }

    fn answer(
        request: Request,
        log: &Arc<Mutex<Vec<String>>>,
        done: &AtomicBool,
    ) -> Result<Value, DispatchError> {
        log.lock().unwrap().push(request.kind_name());
        match request {
            Request::OrchestratorSpawn { .. } => {
                Ok(json!({ "pane_id": 42, "spawned_by": 7, "tmux_session": null }))
            }
            Request::OrchestratorWorkerStatus { .. } => Ok(if done.load(Ordering::SeqCst) {
                json!({
                    "status": "idle",
                    "status_source": "agents",
                    "recent_output": "作業が完了しました\n❯ \n──────\nmodel: opus",
                })
            } else {
                json!({ "status": "busy", "status_source": "agents", "recent_output": "" })
            }),
            Request::Read { .. } => Ok(json!({ "text": "WORKER REPORT" })),
            Request::Close { .. } => Ok(json!({})),
            Request::OrchestratorRunStatus { run_id: Some(id) } => {
                wait::run_status(&id).map_err(DispatchError::Operation)
            }
            Request::OrchestratorRunResult { run_id } => wait::run_result(&run_id, &mut |req| {
                answer(req, log, done).map_err(|e| e.to_string())
            })
            .map_err(DispatchError::Operation),
            // 本物の dispatch も開始要求は断る（受け口で受けずに流れてきたら失敗させる）
            other => Err(DispatchError::Operation(format!(
                "dispatch では扱わない: {}",
                other.kind_name()
            ))),
        }
    }

    /// 1 要求を IPC で往復する。stdio ブリッジ（`tako mcp serve`）と同じく origin = mcp
    pub(super) fn exchange<S: Read + Write>(
        mut stream: S,
        token: &str,
        request: Request,
    ) -> ResponseEnvelope {
        let mut envelope = RequestEnvelope::new(1, token, request);
        envelope.origin = Some("mcp".into());
        writeln!(stream, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("レスポンスを解釈できる")
    }

    /// 開始 → run_status で完了まで追う → run_result で回収、をすべて IPC 越しに行う
    pub(super) fn drive(app: &FakeApp, send: impl Fn(Request) -> ResponseEnvelope) {
        let opts = wait::RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(7),
            tab: None,
            caller_role: None,
            timeout: Duration::from_secs(30),
            auto_close: true,
            output_lines: 50,
            initial_delay: Duration::ZERO,
            interval: Duration::from_millis(10),
            task_type: None,
            account: None,
        };
        let status_of = |run_id: &str| {
            send(Request::OrchestratorRunStatus {
                run_id: Some(run_id.to_string()),
            })
            .result
            .expect("run_status が IPC 越しに引ける")
        };

        let response = send(opts.start_request());
        let body = response.result.unwrap_or_else(|| {
            panic!(
                "非同期 run の開始が IPC 越しに断られた（#1745）: {:?}",
                response.error
            )
        });
        let run_id = body["run_id"].as_str().expect("run_id が返る").to_string();
        assert_eq!(body["pane_id"], 42);
        // worker を完了させる前に開始が返っている = 完了を待っていない
        // （待つ実装なら run 自身の timeout で finished/timeout になってから返る）
        let before = status_of(&run_id);
        assert_eq!(before["phase"], "running", "{before}");

        app.done.store(true, Ordering::SeqCst);
        // 完了待ちは回数で縛る（1 回 20ms の待ちを最大 500 回）
        let finished = (0..500)
            .find_map(|_| {
                let status = status_of(&run_id);
                if status["phase"] == "finished" {
                    return Some(status);
                }
                std::thread::sleep(Duration::from_millis(20));
                None
            })
            .expect("run が finished にならない");
        assert_eq!(finished["status"], "completed");

        let result = send(Request::OrchestratorRunResult {
            run_id: run_id.clone(),
        })
        .result
        .expect("run_result が IPC 越しに引ける");
        assert_eq!(result["status"], "completed");
        assert_eq!(result["output"], "WORKER REPORT");
        assert_eq!(result["closed"], true);

        // spawn も完了待ちも受け口の代役（= UI スレッドの dispatch）へ届き、
        // 開始要求そのものは dispatch へ流れていない
        let kinds = app.seen.lock().unwrap().clone();
        let count = |kind: &str| kinds.iter().filter(|k| *k == kind).count();
        assert_eq!(count("OrchestratorSpawn"), 1, "{kinds:?}");
        assert!(count("OrchestratorWorkerStatus") >= 3, "{kinds:?}");
        assert_eq!(
            count("OrchestratorRunStart"),
            0,
            "開始要求が dispatch へ流れた: {kinds:?}"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;

    use futures::channel::mpsc::unbounded;
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::protocol::{error_code, RequestEnvelope, ResponseEnvelope};

    const TEST_TOKEN: &str = "test-token";

    /// サーバー + ダミーディスパッチャ（list に固定値を返す）を立てて 1 往復する
    fn roundtrip(token_for_client: Option<String>) -> ResponseEnvelope {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");

        // UI イベントループの代わりに同期実行のディスパッチャを立てる
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({ "pong": true })));
            }
        });

        let token = token_for_client.unwrap_or_else(|| TEST_TOKEN.to_string());
        let stream = UnixStream::connect(server.endpoint()).expect("ソケットへ接続できる");
        let mut writer = stream.try_clone().unwrap();
        let envelope = RequestEnvelope::new(1, token, Request::List);
        writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("レスポンスを解釈できる")
    }

    #[test]
    fn 正しいトークンでリクエストが往復する() {
        let response = roundtrip(None);
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap()["pong"], json!(true));
        assert!(response.error.is_none());
    }

    /// #2129: fd の計測を子プロセスで走らせるときの目印（これが無ければ子の本体は何もしない）
    const FD_CHILD_ENV: &str = "TAKO_2129_FD_CHILD";

    /// 子のテスト名（`--exact` で名指しするので、改名したらここも直す）
    const FD_CHILD_TEST: &str = "ipc::tests::子プロセス_連続接続の前後でfdを数える";

    /// 子が計測値を親へ渡す目印（libtest は子の出力を飲み込むので `--nocapture` で出す）
    const FD_MARKER: &str = "TAKO_2129_FD";

    /// 注入（#2129）: 計測のあいだ、**テストプロセスの別スレッド**が開いたままにする fd の数。
    /// 並列で走る他のテストが fd を抱えている状態の再現（CI の macOS で +7 を踏んだ形）
    const INJECT_NOISE_ENV: &str = "TAKO_2129_INJECT_FD_NOISE";

    /// 注入（#2129）: 計測する 10 回の接続のうち、先頭から何本を**閉じずに捨てる**か（本当の漏れ）
    const INJECT_LEAK_ENV: &str = "TAKO_2129_INJECT_LEAK";

    /// A/B（#2129）: 修正前の計測（テストプロセスの中でプロセス全体の fd を数え、
    /// 6 秒間の最小値を許容 +6 で見る）へ戻す
    const LEGACY_ENV: &str = "TAKO_2129_LEGACY";

    /// 計測する往復の回数（ウォームアップは別）
    const FD_ROUNDTRIPS: usize = 10;

    fn env_count(key: &str) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }

    fn fd_count() -> usize {
        std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
    }

    /// EOF 待ちの打ち切り（#2129）。13 回の往復で**共有する**予算で、閉じないサーバー
    /// （= 漏れ）で固まらないためだけにある。通る経路は EOF で抜けるので判定に効かない。
    ///
    /// #2129 のローカル（30 秒超え）と #2209 の CI（120 秒）で、応答を返したサーバーが EOF を
    /// 返さないまま予算を使い切ったのは、macOS（XNU）の起こしの取りこぼし（[`EOF_KICK_AFTER`]）と
    /// 同じ形（サーバー側の 2 本が残ったまま、負荷の引いた後も EOF が来ない）で、実機でもその形を
    /// 再現できた。いまは取りこぼしを救済するので、この予算を使い切るのは救済の後もサーバー側が
    /// 閉じない（= 本当の漏れか停止）ときだけ
    const FD_EOF_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

    /// half-close（書き側の `shutdown`）の後、サーバーの EOF をこれだけ待って来なければ、
    /// クライアントを**完全に閉じて**サーバーを起こす（#2209）。
    ///
    /// macOS（XNU）の `soreceive` は、データの無い read が眠る直前に `uipc_rcvd` でソケットの
    /// ロックを一度手放す（相手のソケットとアドレスの順でロックを取り直す）。その窓に相手の
    /// `shutdown(SHUT_WR)`（`unp_shutdown`）が来ると、受信の終わり（`SS_CANTRCVMORE`）は立つが
    /// まだ眠っていないので起こしは空振りし、読み手は眠る前にそれを見直さずに `sbwait` へ入る。
    /// サーバーの接続スレッドは**相手が完全に閉じるまで** read で眠り、EOF を待つクライアントと
    /// 待ち合う（実機の再現で約 42 万往復に 1 回。PR 本文）。完全に閉じる側（`unp_disconnect`）は
    /// `UNP_DONTDISCONNECT` で窓の終わりを待ってから起こすので取りこぼさない。製品の CLI は
    /// 応答を読んだら完全に閉じる（half-close して待たない）ので、この待ち合いは起きない。
    ///
    /// 普段の EOF はマイクロ秒で届く（ローカルの単独 300 回で最遅 0 ms）。救済しても判定は
    /// 変わらない（サーバー側が閉じたのを fd の数で見届ける = [`wait_server_side_closed`]）ので、
    /// 負荷で遅いだけのサーバーを救済しても害は無い
    const EOF_KICK_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

    /// 注入（#2209）: 計測する 10 回の接続のうち、先頭から何本で**書き側の shutdown を送らない**か。
    /// サーバーは half-close を受け取らず、クライアントが完全に閉じるまで read で眠る = XNU の
    /// 起こしの取りこぼしを踏んだサーバーと同じ状態を毎回作る
    const INJECT_LOST_WAKEUP_ENV: &str = "TAKO_2209_INJECT_LOST_WAKEUP";

    /// A/B（#2209）: 修正前（half-close の EOF を予算いっぱいまで待ち続け、救済しない）へ戻す
    const LEGACY_2209_ENV: &str = "TAKO_2209_LEGACY";

    /// この（子）プロセスで救済（[`EOF_KICK_AFTER`]）が走った回数（診断と #2209 の番犬用）
    static EOF_KICKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// 注入（#2161）: 書き側を閉じた後、サーバーが接続を畳んで FIN が届いたのを見届けてから
    /// EOF 待ちへ進む。CI の macOS で間欠的に踏んだ「FIN が先に届く」順序を毎回作る
    const INJECT_FIN_FIRST_ENV: &str = "TAKO_2161_INJECT_FIN_FIRST";

    /// A/B（#2161）: 修正前の順序（書き側を閉じてから読み取りの上限を入れる）へ戻す
    const LEGACY_2161_ENV: &str = "TAKO_2161_LEGACY";

    /// A/B（#2213）: 修正前の FIN の待ち方（FIN の先着の注入で FIN を予算いっぱい待ち、
    /// #2161 の番犬は往復をやり直さない）へ戻す
    const LEGACY_2213_ENV: &str = "TAKO_2213_LEGACY";

    /// 1 往復して、**サーバーが自分の側を閉じたのを見届けてから**戻る（#2129）。
    /// 戻り値は EOF（救済したときはサーバー側の閉じ）を見届けるまでにかかった時間。
    ///
    /// 応答を読んだら書き側を閉じ（サーバーは EOF を読んで接続を畳む）、読み側で EOF を待つ。
    /// ソケットの EOF はサーバー側の fd（読み・書きの 2 本 = `try_clone`）が**両方**閉じた
    /// ときにだけ届く。だから戻った時点で、この接続に由来する fd はクライアント側も含めて
    /// 全部閉じている = 計測に sleep も「落ち着くまで待つ」も要らない。
    /// EOF が [`EOF_KICK_AFTER`] までに来なければ（XNU の起こしの取りこぼし = #2209）、
    /// 完全に閉じてサーバーを起こし、サーバー側の 2 本が閉じたのを fd の数で見届ける。
    /// fd の数を見るので**子プロセスの計測からだけ呼ぶ**（並列のテストが居るプロセスでは数が揺れる）。
    /// `leak` は注入: 接続を閉じずに fd を捨てる（サーバー側も読み待ちのまま 2 本残る）。
    /// `skip_half_close` は注入（#2209）: 書き側の shutdown を送らない
    fn closed_roundtrip(
        endpoint: &str,
        leak: bool,
        skip_half_close: bool,
        deadline: std::time::Instant,
    ) -> std::time::Duration {
        let fin_first = std::env::var_os(INJECT_FIN_FIRST_ENV).is_some();
        let end = closed_roundtrip_ordered(
            endpoint,
            leak,
            deadline,
            fin_first,
            skip_half_close,
            EOF_KICK_AFTER,
        );
        match end.kicked_at {
            Some(pre) => end.elapsed + wait_server_side_closed(pre, deadline),
            None => end.elapsed,
        }
    }

    /// [`closed_roundtrip_ordered`] の結果
    struct RoundtripEnd {
        /// 書き側を閉じてから EOF が届くまで（救済したときは完全に閉じるまで）の時間
        elapsed: std::time::Duration,
        /// 注入（#2161）の FIN を EOF 待ちの前に見届けたか
        fin_seen: bool,
        /// #2209: EOF が来ず、完全に閉じてサーバーを起こした。値は接続前の fd 数
        /// （サーバー側が閉じたのは呼び手が [`wait_server_side_closed`] で見届ける）
        kicked_at: Option<usize>,
    }

    /// [`closed_roundtrip`] の本体。`fin_first` は注入（#2161）: 書き側を閉じた後、FIN が
    /// 届くまで待ってから EOF 待ちへ進む。`kick_after` は half-close の EOF（`fin_first` では
    /// FIN）を待つ区切りで、来なければ完全に閉じてサーバーを起こす（#2209 / #2213。
    /// 普段は [`EOF_KICK_AFTER`]）
    fn closed_roundtrip_ordered(
        endpoint: &str,
        leak: bool,
        deadline: std::time::Instant,
        fin_first: bool,
        skip_half_close: bool,
        kick_after: std::time::Duration,
    ) -> RoundtripEnd {
        let pre = fd_count();
        let stream = UnixStream::connect(endpoint).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let envelope = RequestEnvelope::new(1, TEST_TOKEN.to_string(), Request::List);
        writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "応答が返る前に接続が閉じた");
        if leak {
            std::mem::forget(reader);
            std::mem::forget(writer);
            return RoundtripEnd {
                elapsed: std::time::Duration::ZERO,
                fin_seen: false,
                kicked_at: None,
            };
        }
        let legacy = std::env::var_os(LEGACY_2161_ENV).is_some();
        let legacy_2209 = std::env::var_os(LEGACY_2209_ENV).is_some();
        let legacy_2213 = std::env::var_os(LEGACY_2213_ENV).is_some();
        let started = std::time::Instant::now();
        let left = deadline.saturating_duration_since(started);
        // half-close の EOF を待つのは救済の手前まで（#2209。修正前は予算いっぱい）
        let eof_wait = if legacy_2209 {
            left
        } else {
            left.min(kick_after)
        };
        // 読み取りの上限は書き側を閉じる**前**に入れる（#2161）。閉じた後だと、サーバーが EOF を
        // 読んで接続を畳み FIN が先に届いた回で、送受信の両方が閉じたソケットへの setsockopt に
        // macOS（XNU）が EINVAL を返す。閉じる前ならサーバーはまだ次の行を待っている
        if !legacy && !eof_wait.is_zero() {
            reader.get_ref().set_read_timeout(Some(eof_wait)).unwrap();
        }
        if !skip_half_close {
            writer.shutdown(std::net::Shutdown::Write).unwrap();
        }
        // FIN の先着の注入（#2161）も救済の手前までしか待たない（#2213。修正前は予算いっぱい）。
        // サーバーが half-close の起こしを取りこぼした回（#2209）の FIN は、こちらが完全に
        // 閉じるまで来ない。負荷で遅いだけの回は、呼び手が区切りを延ばして往復をやり直す
        let fin_deadline = if legacy_2213 {
            deadline
        } else {
            started + eof_wait
        };
        let fin_seen = fin_first && wait_for_fin(reader.get_ref(), fin_deadline);
        // 区切りまでに FIN が来なかった = EOF の待ちの区切りも使い切った（読み直さずに救済へ進む）
        let fin_missed = fin_first && !fin_seen && !legacy_2213;
        let eof = if eof_wait.is_zero() || fin_missed {
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
        } else {
            if legacy {
                reader.get_ref().set_read_timeout(Some(eof_wait)).unwrap();
            }
            reader.read_to_end(&mut Vec::new())
        };
        let timed_out = eof.as_ref().is_err_and(|e| {
            matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        });
        if timed_out && !legacy_2209 && std::time::Instant::now() < deadline {
            // #2209: サーバーが half-close の起こしを取りこぼして眠っている（か、負荷で遅い）。
            // 完全に閉じれば unp_disconnect が起こす。閉じたかは呼び手が fd の数で見届ける
            drop(reader);
            drop(writer);
            EOF_KICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return RoundtripEnd {
                elapsed: started.elapsed(),
                fin_seen,
                kicked_at: Some(pre),
            };
        }
        if let Err(e) = eof {
            // 診断（判定はもう決まっている = 落ちる）。クライアント側の 2 本を開いたまま数え、
            // そのあと少しだけ EOF を読み直す。数える前に自分の側を閉じると、そのあいだに
            // サーバーが遅れて閉じた回を「別のプロセスが握っている」と取り違える
            let now = fd_count();
            let _ = reader
                .get_ref()
                .set_read_timeout(Some(std::time::Duration::from_millis(100)));
            let late_eof = reader.read_to_end(&mut Vec::new()).is_ok();
            let verdict = if now >= pre + 4 {
                "サーバー側の 2 本がこの子プロセスに残っていた = 漏れ、またはサーバーの停止"
            } else if late_eof {
                "打ち切りの直後に EOF が届いた = サーバーが遅れて閉じた（停止）"
            } else {
                "この子プロセスではサーバー側が閉じているのに EOF が来ない = 別のプロセスが握っている"
            };
            panic!(
                "サーバーが接続を閉じない（EOF 待ちの予算 {} 秒を使い切った）: {e}。\
                 fd は接続の前 {pre} → 打ち切り時 {now}（クライアント側の 2 本を含む）。{verdict}",
                FD_EOF_BUDGET.as_secs()
            );
        }
        RoundtripEnd {
            elapsed: started.elapsed(),
            fin_seen,
            kicked_at: None,
        }
    }

    /// 救済（#2209）で完全に閉じた後、サーバー側の 2 本が閉じて fd の数が接続前（`pre`）へ
    /// 戻るのを見届ける。クライアント側の 2 本は閉じ済みで、この子プロセスには計測しか
    /// 居ないので、数が戻った = サーバーの接続スレッドが EOF を読んで接続を畳んだ。
    /// 戻り値は見届けるまでの時間
    fn wait_server_side_closed(pre: usize, deadline: std::time::Instant) -> std::time::Duration {
        let started = std::time::Instant::now();
        loop {
            let now = fd_count();
            if now <= pre {
                return started.elapsed();
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "サーバーが接続を閉じない（half-close の EOF が {} ms で来ず、完全に閉じた後も\
                     EOF 待ちの予算 {} 秒を使い切った）。fd は接続の前 {pre} → 打ち切り時 {now}\
                     （クライアント側は閉じ済み）。サーバー側がこの子プロセスに残っていた = 漏れ、\
                     またはサーバーの停止",
                    EOF_KICK_AFTER.as_millis(),
                    FD_EOF_BUDGET.as_secs()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// 注入（#2161）: サーバーが接続を畳んで FIN が届くまで待つ。非ブロッキングで覗くだけで
    /// 読み取りの上限（setsockopt）には触らないので、修正前の順序でも「FIN が届いた後に上限を
    /// 入れる」形をそのまま作れる。`deadline` までに届けば true
    fn wait_for_fin(stream: &UnixStream, deadline: std::time::Instant) -> bool {
        stream.set_nonblocking(true).unwrap();
        let mut peek: &UnixStream = stream;
        let seen = loop {
            match peek.read(&mut [0u8; 1]) {
                Ok(0) => break true,
                Ok(_) => panic!("応答の後にサーバーが余計なバイトを送った"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("FIN を待つ読み取りが失敗した: {e}"),
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        stream.set_nonblocking(false).unwrap();
        seen
    }

    /// 子の計測結果
    struct FdMeasure {
        before: usize,
        after: usize,
        /// EOF が届くまでにいちばんかかった往復（診断用。判定には使わない）
        slowest_eof: std::time::Duration,
        /// half-close の EOF が来ず、完全に閉じて救済した往復の数（#2209。診断と番犬用）
        kicks: usize,
    }

    /// 受け口とダミーのディスパッチャを立て、ウォームアップ 3 回のあと 10 回往復して
    /// 前後の fd 数を返す。`leaks` 本目までは閉じずに捨て、`lost` 本目までは書き側の
    /// shutdown を送らない（どちらも注入）
    fn measure_roundtrip_fds(leaks: usize, lost: usize) -> FdMeasure {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        let deadline = std::time::Instant::now() + FD_EOF_BUDGET;
        let mut slowest_eof = std::time::Duration::ZERO;
        // ウォームアップ（初回だけ確保される物を測定から外す）
        for _ in 0..3 {
            slowest_eof =
                slowest_eof.max(closed_roundtrip(server.endpoint(), false, false, deadline));
        }
        let before = fd_count();
        for i in 0..FD_ROUNDTRIPS {
            let took = closed_roundtrip(server.endpoint(), i < leaks, i < lost, deadline);
            slowest_eof = slowest_eof.max(took);
        }
        FdMeasure {
            before,
            after: fd_count(),
            slowest_eof,
            kicks: EOF_KICKS.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// 注入の雑音: 別スレッドで `n` 本の fd を開き、返り値が drop されるまで開いたままにする。
    /// 開き終わってから返す（計測のあいだ確実に開いている）
    struct HeldFds(Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>);

    impl Drop for HeldFds {
        fn drop(&mut self) {
            if let Some((stop, handle)) = self.0.take() {
                drop(stop);
                let _ = handle.join();
            }
        }
    }

    fn hold_fds_in_other_thread(n: usize) -> HeldFds {
        if n == 0 {
            return HeldFds(None);
        }
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let files: Vec<_> = (0..n)
                .map(|_| std::fs::File::open("/dev/null").expect("/dev/null を開ける"))
                .collect();
            let _ = ready_tx.send(());
            // 送り手が drop されるまで抱える
            let _ = stop_rx.recv();
            drop(files);
        });
        ready_rx.recv().expect("雑音のスレッドが fd を開き終える");
        HeldFds(Some((stop_tx, handle)))
    }

    /// 子が 1 バイトも出さずに失敗した回を起こし直す上限（初回を含む。#1832 と同じ扱い）
    const SILENT_CHILD_ATTEMPTS: usize = 3;

    /// このテストバイナリを子として起こし、[`FD_CHILD_TEST`] だけを走らせて
    /// `[前の fd 数, 後の fd 数, EOF のいちばん遅い往復の ms, 救済した往復の数]` を受け取る。
    /// `envs` は子にだけ足す env（#2209 の番犬が注入を名指しする口）
    fn measure_in_child_process(envs: &[(&str, &str)]) -> [usize; 4] {
        run_fd_child(
            FD_CHILD_TEST,
            ["before", "after", "slowest_eof_ms", "kicks"],
            envs,
        )
    }

    /// このテストバイナリを子として起こし、`child_test` だけを走らせて、子が [`FD_MARKER`] の
    /// 行に出した `key=値` を `keys` の順で受け取る（#2129 の計測と #2136 の計測が共有する）
    fn run_fd_child<const N: usize>(
        child_test: &str,
        keys: [&str; N],
        envs: &[(&str, &str)],
    ) -> [usize; N] {
        let exe = std::env::current_exe().expect("テストバイナリのパス");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--exact", child_test, "--test-threads=1", "--nocapture"])
            .env(FD_CHILD_ENV, "1")
            // 雑音は親（= 並列のテストが同居するプロセス）に留める。漏れの注入はそのまま子へ継ぐ
            .env_remove(INJECT_NOISE_ENV)
            .envs(envs.iter().copied());
        let mut statuses: Vec<String> = Vec::new();
        let out = loop {
            let out = cmd.output().expect("子テストプロセスを起こせる");
            let reached = out.status.success() || !out.stdout.is_empty() || !out.stderr.is_empty();
            if reached || statuses.len() + 1 >= SILENT_CHILD_ATTEMPTS {
                break out;
            }
            statuses.push(out.status.to_string());
        };
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        assert!(
            out.status.success(),
            "fd を数える子が失敗した（{}。起こし直した回: {statuses:?}）:\n{text}",
            out.status
        );
        let field = |line: &str, key: &str| -> Option<usize> {
            line.split_whitespace()
                .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
        };
        // 目印は行頭とは限らない（libtest の `test <名前> ... ` の後ろに改行なしで続く）
        text.lines()
            .filter_map(|line| line.split_once(FD_MARKER).map(|(_, rest)| rest))
            .find_map(|rest| {
                let values: Option<Vec<usize>> = keys.iter().map(|k| field(rest, k)).collect();
                <[usize; N]>::try_from(values?).ok()
            })
            .unwrap_or_else(|| {
                panic!("子が計測まで届いていない（{child_test} の改名漏れ？）:\n{text}")
            })
    }

    /// #2129: 計測の本体。親の [`連続接続でfdが漏れない`] から子プロセスとしてだけ走る
    /// （普段のテスト実行では目印が無いので何もしない）
    #[test]
    fn 子プロセス_連続接続の前後でfdを数える() {
        if std::env::var_os(FD_CHILD_ENV).is_none() {
            return;
        }
        let m = measure_roundtrip_fds(
            env_count(INJECT_LEAK_ENV),
            env_count(INJECT_LOST_WAKEUP_ENV),
        );
        println!(
            "{FD_MARKER} before={} after={} slowest_eof_ms={} kicks={}",
            m.before,
            m.after,
            m.slowest_eof.as_millis(),
            m.kicks
        );
    }

    /// 連続接続で fd が漏れないこと。
    ///
    /// **計測は子プロセスで行う**（#2129）。数えているのはプロセス全体の fd（`/dev/fd` の
    /// 件数）なので、テストプロセスの中で数えると並列で走る他のテストの fd の開閉を拾う
    /// （#916 で 15 → 18 を最小値で見る形でしのぎ、#983 で 10 → 14 を許容 +6 でしのぎ、
    /// #2129 で +7 を踏んだ）。子プロセスにはこの計測しか居ないので雑音が入らない。
    /// 往復はサーバーが閉じたのを見届けてから戻る（[`closed_roundtrip`]）ので、前後の差は
    /// **許容 0** で比べられる = 1 接続の取りこぼしでも落ちる（旧の許容 +6 は、サーバー側
    /// 2 本ずつの漏れなら 3 接続ぶんまで吸収していた）。件数で見るので、ソケット以外の
    /// 種類の fd（ファイル・パイプ）の漏れも取りこぼさない。
    ///
    /// 注入と A/B（既定では効かない）:
    /// - `TAKO_2129_INJECT_FD_NOISE=<n>`: 計測のあいだテストプロセスの別スレッドが n 本の
    ///   fd を開いたままにする（並列テストの再現）。修正後は通る / `TAKO_2129_LEGACY=1` は n ≥ 7 で落ちる
    /// - `TAKO_2129_INJECT_LEAK=<k>`: 10 回の接続のうち k 本を閉じずに捨てる（本当の漏れ）。
    ///   修正後は k ≥ 1 で落ちる / 旧は k = 1 を見逃す
    /// - `TAKO_2209_INJECT_LOST_WAKEUP=<k>`: 10 回の接続のうち k 本で書き側の shutdown を
    ///   送らない（XNU の起こしの取りこぼし = [`EOF_KICK_AFTER`]）。修正後は通る /
    ///   `TAKO_2209_LEGACY=1` は EOF 待ちの予算 120 秒を使い切って落ちる
    #[test]
    fn 連続接続でfdが漏れない() {
        let noise = env_count(INJECT_NOISE_ENV);
        let leaks = env_count(INJECT_LEAK_ENV);
        if std::env::var_os(LEGACY_ENV).is_some() {
            legacy_fd_leak_check(noise, leaks);
            return;
        }
        let [before, after, slowest_eof_ms, kicks] = {
            let _noise = hold_fds_in_other_thread(noise);
            measure_in_child_process(&[])
        };
        println!(
            "#2129 子プロセスの fd: {before} → {after}（雑音の注入 {noise} 本・EOF の最遅 {slowest_eof_ms} ms・\
             取りこぼしの救済 {kicks} 回）"
        );
        assert!(
            after <= before,
            "IPC 接続 {FD_ROUNDTRIPS} 回で fd が {before} → {after} に増えた（漏れ）"
        );
    }

    /// #2209 の番犬: サーバーが half-close（クライアントの書き側の shutdown）の起こしを
    /// 取りこぼしても、往復の後始末（[`closed_roundtrip`]）が EOF 待ちの予算を使い切らずに戻り、
    /// サーバー側の fd も閉じる。
    ///
    /// CI の macOS で「サーバーが接続を閉じない（予算 120 秒を使い切った）」で落ちた型
    /// （PR #2198 の 1 回目）。真因は XNU の取りこぼし（[`EOF_KICK_AFTER`]）で、実機では
    /// 数十万往復に 1 回しか踏まないので、注入で毎回作る: 計測する 1 本目で書き側の shutdown を
    /// 送らない = サーバーは取りこぼしたときと同じく、クライアントが完全に閉じるまで read で眠る。
    /// A/B: `TAKO_2209_LEGACY=1` では救済せずに EOF を待ち続け、予算 120 秒を使い切って落ちる
    #[test]
    fn issue2209_サーバーがhalf_closeの起こしを取りこぼしても往復の後始末が戻る() {
        let [before, after, _, kicks] = measure_in_child_process(&[(INJECT_LOST_WAKEUP_ENV, "1")]);
        println!("#2209 子プロセスの fd: {before} → {after}（取りこぼしの救済 {kicks} 回）");
        assert!(
            kicks >= 1,
            "救済が 1 回も走っていない = 注入が取りこぼしを作れておらず、番犬が何も試していない"
        );
        assert!(
            after <= before,
            "取りこぼしを救済した往復でサーバー側の fd が {before} → {after} に残った"
        );
    }

    /// A/B（`TAKO_2129_LEGACY=1`）: 修正前の計測そのもの。テストプロセスの中でプロセス全体の
    /// fd を数え、往復の後 6 秒間の最小値を許容 +6 で見る。注入の口だけを足してある
    /// （雑音は基準を取った直後に開き、判定が済むまで抱える）
    fn legacy_fd_leak_check(noise: usize, leaks: usize) {
        fn one_roundtrip(endpoint: &str, leak: bool) {
            let stream = UnixStream::connect(endpoint).unwrap();
            let mut writer = stream.try_clone().unwrap();
            let envelope = RequestEnvelope::new(1, TEST_TOKEN.to_string(), Request::List);
            writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).unwrap();
            if leak {
                std::mem::forget(reader);
                std::mem::forget(writer);
            }
        }

        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        for _ in 0..3 {
            one_roundtrip(server.endpoint(), false);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let before = fd_count();
        let _noise = hold_fds_in_other_thread(noise);
        for i in 0..FD_ROUNDTRIPS {
            one_roundtrip(server.endpoint(), i < leaks);
        }
        const FD_NOISE_TOLERANCE: usize = 6;
        let mut lowest = usize::MAX;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            lowest = lowest.min(fd_count());
            if lowest <= before + FD_NOISE_TOLERANCE {
                break;
            }
        }
        assert!(
            lowest <= before + FD_NOISE_TOLERANCE,
            "IPC 接続 10 回で fd が {before} → {lowest}（6 秒間の最小値）に増えた（リーク）"
        );
    }

    #[test]
    fn 不正なトークンは認証エラーで拒否される() {
        let response = roundtrip(Some("bogus-token".into()));
        let error = response.error.expect("エラーになる");
        assert_eq!(error.code, error_code::AUTH);
        assert!(response.result.is_none());
    }

    /// #1745: stdio ブリッジ（`tako mcp serve`）は IPC 越しに頼むしかない。開始要求が
    /// IPC で届いても受け口側（このプロセス）で spawn と完了待ちが回り、
    /// run_status / run_result を IPC で引けることを固定する
    #[test]
    fn 非同期runの開始はipc越しでも受け口側で回る() {
        let (tx, rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");
        let app = super::run_start_over_ipc::spawn_fake_app(rx);
        let endpoint = server.endpoint().to_string();
        super::run_start_over_ipc::drive(&app, |request| {
            let stream = UnixStream::connect(&endpoint).expect("ソケットへ接続できる");
            super::run_start_over_ipc::exchange(stream, TEST_TOKEN, request)
        });
    }

    #[test]
    fn dropでソケットファイルが消える() {
        let (tx, _rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        let path = server.endpoint().to_string();
        assert!(std::fs::metadata(&path).is_ok());
        drop(server);
        assert!(std::fs::metadata(&path).is_err());
    }

    /// #2136: 立てて捨てる回数（ウォームアップの 1 回は別）
    const SERVER_CYCLES: usize = 5;

    /// #2136 の子のテスト名（`--exact` で名指しするので、改名したらここも直す）
    const SERVER_CHILD_TEST: &str = "ipc::tests::子プロセス_サーバーを立てて捨てる前後でfdを数える";

    /// サーバーを立てて 1 往復し（accept スレッドが接続を受けて回った状態にする）、捨てる。
    /// 戻り値は drop にかかった時間（診断用。判定には使わない）
    fn start_roundtrip_drop(deadline: std::time::Instant) -> std::time::Duration {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        closed_roundtrip(server.endpoint(), false, false, deadline);
        let started = std::time::Instant::now();
        drop(server);
        started.elapsed()
    }

    /// #2136: 計測の本体。親の [`サーバーを立てて捨てた後にfdが元へ戻る`] から子プロセスとして
    /// だけ走る（普段のテスト実行では目印が無いので何もしない）
    #[test]
    fn 子プロセス_サーバーを立てて捨てる前後でfdを数える() {
        if std::env::var_os(FD_CHILD_ENV).is_none() {
            return;
        }
        let deadline = std::time::Instant::now() + FD_EOF_BUDGET;
        // ウォームアップ（初回だけ確保される物を測定から外す）
        start_roundtrip_drop(deadline);
        let before = fd_count();
        let mut slowest_drop = std::time::Duration::ZERO;
        for _ in 0..SERVER_CYCLES {
            slowest_drop = slowest_drop.max(start_roundtrip_drop(deadline));
        }
        println!(
            "{FD_MARKER} before={before} after={} slowest_drop_us={}",
            fd_count(),
            slowest_drop.as_micros()
        );
    }

    /// drop した IpcServer は受け口（listener の fd）を閉じる（#2136）。
    ///
    /// 修正前は accept スレッドが listener を握ったまま眠り続け、立てて捨てるたびに fd が 1 本
    /// （とスレッドが 1 本）残った。計測は [`連続接続でfdが漏れない`] と同じく子プロセスで行い
    /// （並列の他のテストの fd を拾わない）、drop が戻った直後に数えて**許容 0** で比べる
    /// （drop は受け口が閉じるのを待ってから戻る）。
    /// A/B: `TAKO_2136_LEGACY=1` で drop が受け口を閉じなくなり、fd が [`SERVER_CYCLES`] 本増えて落ちる
    #[test]
    fn サーバーを立てて捨てた後にfdが元へ戻る() {
        let [before, after, slowest_drop_us] = run_fd_child(
            SERVER_CHILD_TEST,
            ["before", "after", "slowest_drop_us"],
            &[],
        );
        println!(
            "#2136 子プロセスの fd: {before} → {after}（立てて捨てた回数 {SERVER_CYCLES}・drop の最遅 {slowest_drop_us} µs）"
        );
        assert!(
            after <= before,
            "IpcServer を {SERVER_CYCLES} 回立てて捨てたら fd が {before} → {after} に増えた\
             （drop が受け口の fd を閉じていない）"
        );
    }

    /// FIN の先着（#2161）を作れるまで往復をやり直し、FIN を見届けた往復が何本目か
    /// （1 始まり）を返す。FIN を見届けられなかったら `None`（`TAKO_2213_LEGACY=1` のときだけ）。
    ///
    /// サーバーが half-close の起こしを取りこぼした回（#2209）は、こちらが完全に閉じるまで
    /// FIN が来ない。完全に閉じた接続では「FIN が先に届く」順序を作れないので、区切り
    /// （[`EOF_KICK_AFTER`] から倍々。負荷で遅いだけなら次の区切りで間に合う）までに FIN が
    /// 来なければ、[`closed_roundtrip_ordered`] が完全に閉じてサーバーを起こし、ここで新しい
    /// 接続からやり直す（#2213）。取りこぼしは数十万往復に 1 回なので、続けて踏む見込みは無い。
    /// 親プロセス（並列のテストが同居する）で走るので、救済した接続のサーバー側が閉じたかは
    /// fd の数で見届けない（この番犬が見るのは順序だけ）。
    /// サーバーが half-close で一度も閉じない（本物の停止）ときは予算 120 秒で落ちる。
    /// `lost` は注入（#2209）: 先頭から何本の往復で書き側の shutdown を送らないか
    fn fin_first_roundtrip(
        endpoint: &str,
        lost: usize,
        deadline: std::time::Instant,
    ) -> Option<usize> {
        let legacy_2213 = std::env::var_os(LEGACY_2213_ENV).is_some();
        let mut kick_after = EOF_KICK_AFTER;
        let mut tries = 0;
        loop {
            let skip_half_close = tries < lost;
            tries += 1;
            let end = closed_roundtrip_ordered(
                endpoint,
                false,
                deadline,
                true,
                skip_half_close,
                kick_after,
            );
            if end.fin_seen {
                return Some(tries);
            }
            if legacy_2213 || end.kicked_at.is_none() {
                return None;
            }
            kick_after = kick_after.saturating_mul(2);
        }
    }

    /// [`fin_first_roundtrip`] の番犬用に、受け口とダミーのディスパッチャを立てる
    fn start_fin_first_server() -> IpcServer {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        server
    }

    /// #2161 の番犬: サーバーの FIN が EOF 待ちより先に届いた回でも、往復の後始末
    /// （[`closed_roundtrip`]）が落ちない。
    ///
    /// 修正前は書き側を閉じてから読み取りの上限を入れていたので、その間にサーバーが接続を
    /// 畳んだ回は、送受信の両方が閉じたソケットへの setsockopt に macOS が EINVAL を返して
    /// fd 数えのテストが落ちた（CI の macOS で間欠的に踏んだ形）。ここでは注入で FIN の先着を
    /// 毎回作るので、タイミングに依らず検出する。サーバーが half-close の起こしを取りこぼした
    /// 回は往復をやり直す（[`fin_first_roundtrip`]。#2213）。
    /// A/B: `TAKO_2161_LEGACY=1` で macOS では EINVAL で落ちる。
    /// 注入: `TAKO_2209_INJECT_LOST_WAKEUP=<k>` で先頭の k 本を取りこぼした回にする
    #[test]
    fn issue2161_サーバーのfinが先に届いても往復の後始末が落ちない() {
        let server = start_fin_first_server();
        let deadline = std::time::Instant::now() + FD_EOF_BUDGET;
        let lost = env_count(INJECT_LOST_WAKEUP_ENV);
        let tries = fin_first_roundtrip(server.endpoint(), lost, deadline);
        println!("#2161 FIN の先着を作れた往復: {tries:?}（取りこぼしの注入 {lost} 本）");
        assert!(
            tries.is_some(),
            "EOF 待ちの前に FIN が届いていない = 注入が順序を作れておらず、番犬が何も試していない"
        );
    }

    /// #2213 の番犬: FIN の先着を作る往復（#2161 の番犬）で、サーバーが half-close の起こしを
    /// 取りこぼしても（#2209）、予算を使い切らずに往復をやり直して順序を作る。
    ///
    /// 修正前は FIN を予算 120 秒いっぱい待ったので、取りこぼした回は 120 秒で落ちた
    /// （実機では数十万往復に 1 回なので、注入で毎回作る: 1 本目で書き側の shutdown を送らない
    /// = サーバーは取りこぼしたときと同じく、こちらが完全に閉じるまで read で眠る）。
    /// A/B: `TAKO_2213_LEGACY=1` では FIN を待ち続け、予算 120 秒を使い切って落ちる
    #[test]
    fn issue2213_finの先着を待つ往復で起こしを取りこぼしてもやり直して順序を作る() {
        let server = start_fin_first_server();
        let deadline = std::time::Instant::now() + FD_EOF_BUDGET;
        let started = std::time::Instant::now();
        let tries = fin_first_roundtrip(server.endpoint(), 1, deadline);
        println!(
            "#2213 FIN の先着を作れた往復: {tries:?}（取りこぼしの注入 1 本・{:?}）",
            started.elapsed()
        );
        let tries = tries.expect("取りこぼした往復をやり直した後も FIN の先着を作れない");
        assert!(
            tries >= 2,
            "1 本目で FIN が届いた = 注入が取りこぼしを作れておらず、番犬が何も試していない"
        );
    }

    /// #2136: ソケットファイルを外から消されていると、起こす 1 本が繋がらない。そのとき drop は
    /// 待たずに戻る（受け口は修正前と同じく残るが、drop = GUI の終了を固めないのが優先）
    #[test]
    fn ソケットファイルを外から消されていてもdropは待たずに戻る() {
        let (tx, _rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::fs::remove_file(server.endpoint()).unwrap();
        let started = std::time::Instant::now();
        drop(server);
        let took = started.elapsed();
        println!("#2136 起こせないときの drop: {took:?}");
        assert!(
            took < ACCEPT_STOP_WAIT,
            "起こせないのに上限まで待った（{took:?}）"
        );
    }

    /// #2136: drop が閉じるのは受け口だけ。張っていた接続は切らず、その接続で送った
    /// リクエストには drop の後でも応答が返る
    #[test]
    fn drop後も張っていた接続には応答が返る() {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({ "pong": true })));
            }
        });
        let stream = UnixStream::connect(server.endpoint()).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut ask = |id: u64| -> ResponseEnvelope {
            let envelope = RequestEnvelope::new(id, TEST_TOKEN.to_string(), Request::List);
            writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str(&line).expect("レスポンスを解釈できる")
        };
        // 1 往復目で接続が受けられた（接続のスレッドが立った）ことを確かめてから捨てる
        assert_eq!(ask(1).id, 1);
        drop(server);
        let after = ask(2);
        assert_eq!(after.id, 2);
        assert_eq!(after.result.expect("応答が返る")["pong"], json!(true));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::io::{BufRead, BufReader, Write};

    use futures::channel::mpsc::unbounded;
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::platform::named_pipe;
    use crate::protocol::{error_code, Request, RequestEnvelope, ResponseEnvelope};

    const TEST_TOKEN: &str = "test-token";

    /// サーバー + ダミーディスパッチャを立てて 1 往復する（unix 側テストの対）
    fn roundtrip(token_for_client: Option<String>) -> ResponseEnvelope {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");

        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({ "pong": true })));
            }
        });

        let token = token_for_client.unwrap_or_else(|| TEST_TOKEN.to_string());
        let stream =
            named_pipe::connect_client(server.endpoint(), 3_000).expect("パイプへ接続できる");
        let mut writer = stream.try_clone().unwrap();
        let envelope = RequestEnvelope::new(1, token, Request::List);
        writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("レスポンスを解釈できる")
    }

    #[test]
    fn 正しいトークンでリクエストが往復する() {
        let response = roundtrip(None);
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap()["pong"], json!(true));
        assert!(response.error.is_none());
    }

    #[test]
    fn 不正なトークンは認証エラーで拒否される() {
        let response = roundtrip(Some("bogus-token".into()));
        let error = response.error.expect("エラーになる");
        assert_eq!(error.code, error_code::AUTH);
        assert!(response.result.is_none());
    }

    /// #1745: unix 側の同名テストの対（名前付きパイプ越し）
    #[test]
    fn 非同期runの開始はipc越しでも受け口側で回る() {
        let (tx, rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");
        let app = super::run_start_over_ipc::spawn_fake_app(rx);
        let endpoint = server.endpoint().to_string();
        super::run_start_over_ipc::drive(&app, |request| {
            let stream = named_pipe::connect_client(&endpoint, 3_000).expect("パイプへ接続できる");
            super::run_start_over_ipc::exchange(stream, TEST_TOKEN, request)
        });
    }

    #[test]
    fn 連続接続が全件処理される() {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        for i in 0..10 {
            let stream = named_pipe::connect_client(server.endpoint(), 3_000)
                .unwrap_or_else(|e| panic!("{i} 回目の接続に失敗: {e}"));
            let mut writer = stream.try_clone().unwrap();
            let envelope = RequestEnvelope::new(1, TEST_TOKEN.to_string(), Request::List);
            writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert!(!line.is_empty(), "{i} 回目の応答が空");
        }
    }

    /// #2136: drop したら受け口（待ち受け中のパイプのインスタンス）が閉じ、その名前へはもう
    /// 繋がらない。修正前は accept スレッドが次のインスタンスを抱えたまま残り、捨てたサーバーへ
    /// 繋がった（A/B: `TAKO_2136_LEGACY=1` でこのテストは落ちる）
    #[test]
    fn dropで受け口が閉じて繋がらなくなる() {
        let (tx, _rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        let name = server.endpoint().to_string();
        assert!(named_pipe::probe_alive(&name), "立てた直後は待ち受けている");
        drop(server);
        assert!(
            !named_pipe::probe_alive(&name),
            "drop の後も {name} の待ち受けが残っている"
        );
        assert!(
            named_pipe::connect_client(&name, 200).is_err(),
            "drop したサーバー（{name}）へ繋がった"
        );
    }
}

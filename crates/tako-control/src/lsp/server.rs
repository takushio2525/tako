//! 言語サーバ 1 つのプロセス（#1678）
//!
//! **1 サーバにつき 3 スレッド**（Zed の 3 タスクと同じ役割分担。tokio も GPUI の
//! executor も使わない = `.agent/architecture.md`「IPC トランスポート」と同じ形）:
//!
//! | スレッド | 役割 |
//! |---|---|
//! | writer | 送信キューから取り出して stdin へ書く。キューは上限つき（満杯なら送り手へ返す） |
//! | reader | stdout から 1 メッセージずつ読み、応答は待ち手へ・要求は既定の応答・通知は呼び手へ |
//! | stderr | 1 行ずつ読み、直近の行だけをリングに保つ（`tako lsp logs` の診断用） |
//!
//! プロセスは [`Drop`] で必ず kill + 刈り取りする。GUI が kill -9 で落ちたときは
//! stdin が閉じる（EOF）ので、サーバは自分で終わる（LSP サーバの通常の振る舞い。
//! 受け入れ条件の「孤児を残さない」は偽サーバで実測する）。
//!
//! **診断ログに本文を書かない**（AGENTS.md の絶対ルール。LSP の I/O はソースコードを含む）。
//! perf.log へは要求の method 名・所要・結果の種別だけ。生の JSON-RPC は
//! `TAKO_LSP_DIAG=1` のときだけ `<data_dir>/lsp/<id>.log` へ残す。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tako_core::platform::child_cmd::ChildCmd;

use super::rpc::{self, Incoming, PendingTable, RequestId, ResponseError};

/// 送信キューの上限（メッセージ数）。サーバが stdin を読まなくなったら満杯になり、
/// 送り手（UI スレッド）は待たずに失敗を受け取る
pub const OUTBOX_CAPACITY: usize = 256;

/// stderr を覚えておく行数と 1 行の上限（**本文が stderr に出るサーバがありうる**ので
/// 上限を置き、`tako lsp logs` で明示的に求められたときだけ返す）
pub const STDERR_TAIL_LINES: usize = 50;
pub const STDERR_LINE_BYTES: usize = 400;

/// 要求の失敗
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcError {
    #[error("応答が {0} 秒以内に来なかった")]
    Timeout(u64),
    #[error("サーバとの接続が切れた")]
    Disconnected,
    #[error("送信キューが満杯（サーバが読んでいない）")]
    QueueFull,
    #[error("サーバがエラーを返した（{}）: {}", .0.code, .0.message)]
    Server(ResponseError),
}

/// サーバからの通知を受ける関数（method, params）
pub type NotificationHandler = Box<dyn Fn(&str, Value) + Send + Sync>;

/// reader スレッドから呼び手へ渡すもの
pub struct Handlers {
    /// サーバからの通知（`textDocument/publishDiagnostics` 等）
    pub on_notification: NotificationHandler,
    /// stdout が閉じた（= プロセスが終わった）。1 度だけ呼ばれる
    pub on_exit: Box<dyn FnOnce() + Send>,
}

type Waiter = SyncSender<Result<Value, RpcError>>;

/// 動いている言語サーバ 1 つ
pub struct ServerProcess {
    label: String,
    pid: u32,
    child: Mutex<Option<Child>>,
    outbox: SyncSender<Vec<u8>>,
    pending: Arc<Mutex<PendingTable<Waiter>>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    exited: Arc<AtomicBool>,
    /// 受け取ったメッセージの数（診断用。アイドル時に増えないことの観測にも使う）
    received: Arc<AtomicU64>,
    /// 本文が JSON でなかったメッセージの数
    garbage: Arc<AtomicU64>,
}

/// 生の JSON-RPC の記録（`TAKO_LSP_DIAG=1` のときだけ）
type RawLog = Arc<Mutex<std::fs::File>>;

fn raw_log_line(log: &Option<RawLog>, direction: &str, text: &str) {
    if let Some(log) = log {
        if let Ok(mut file) = log.lock() {
            let _ = writeln!(file, "{direction} {text}");
        }
    }
}

impl ServerProcess {
    /// 起こす。stdin / stdout / stderr はパイプにする
    pub fn spawn(
        plan: &ChildCmd,
        label: &str,
        handlers: Handlers,
        raw_log_path: Option<std::path::PathBuf>,
    ) -> std::io::Result<Arc<Self>> {
        let mut command = plan.command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin はパイプ");
        let stdout = child.stdout.take().expect("stdout はパイプ");
        let stderr = child.stderr.take().expect("stderr はパイプ");

        let raw_log = raw_log_path.and_then(|path| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(|f| Arc::new(Mutex::new(f)))
        });

        let (outbox, outbox_rx) = mpsc::sync_channel::<Vec<u8>>(OUTBOX_CAPACITY);
        let process = Arc::new(Self {
            label: label.to_string(),
            pid,
            child: Mutex::new(Some(child)),
            outbox: outbox.clone(),
            pending: Arc::new(Mutex::new(PendingTable::default())),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            exited: Arc::new(AtomicBool::new(false)),
            received: Arc::new(AtomicU64::new(0)),
            garbage: Arc::new(AtomicU64::new(0)),
        });

        // writer: キューが閉じる（送り手が全部落ちる）か書けなくなったら終わる。
        // 終わると stdin が閉じる = サーバは EOF を受けて自分で終わる
        {
            let raw_log = raw_log.clone();
            std::thread::Builder::new()
                .name(format!("lsp-writer-{label}"))
                .spawn(move || {
                    let mut stdin = stdin;
                    for bytes in outbox_rx {
                        if let Some(body) = raw_log.as_ref().and(frame_body(&bytes)) {
                            raw_log_line(&raw_log, "-->", body);
                        }
                        if stdin
                            .write_all(&bytes)
                            .and_then(|()| stdin.flush())
                            .is_err()
                        {
                            break;
                        }
                    }
                })?;
        }

        // reader
        {
            let pending = Arc::clone(&process.pending);
            let exited = Arc::clone(&process.exited);
            let received = Arc::clone(&process.received);
            let garbage = Arc::clone(&process.garbage);
            let raw_log = raw_log.clone();
            let label = label.to_string();
            let Handlers {
                on_notification,
                on_exit,
            } = handlers;
            std::thread::Builder::new()
                .name(format!("lsp-reader-{label}"))
                .spawn(move || {
                    let mut input = BufReader::new(stdout);
                    loop {
                        match rpc::read_message(&mut input) {
                            Ok(Some(message)) => {
                                received.fetch_add(1, Ordering::Relaxed);
                                if raw_log.is_some() {
                                    raw_log_line(&raw_log, "<--", &message.to_string());
                                }
                                handle_incoming(message, &pending, &outbox, &*on_notification);
                            }
                            Ok(None) => break,
                            Err(rpc::ReadError::InvalidJson(_)) => {
                                // 壊れたメッセージ 1 つで接続を捨てない（次から読み続ける）
                                garbage.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => break,
                        }
                    }
                    exited.store(true, Ordering::SeqCst);
                    let waiters = pending.lock().map(|mut t| t.drain()).unwrap_or_default();
                    for (_, _, waiter) in waiters {
                        let _ = waiter.try_send(Err(RpcError::Disconnected));
                    }
                    drop(outbox);
                    on_exit();
                })?;
        }

        // stderr
        {
            let tail = Arc::clone(&process.stderr_tail);
            let raw_log = raw_log.clone();
            std::thread::Builder::new()
                .name(format!("lsp-stderr-{label}"))
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.split(b'\n') {
                        let Ok(line) = line else { break };
                        let text = String::from_utf8_lossy(&line);
                        let text = text.trim_end_matches('\r');
                        raw_log_line(&raw_log, "err", text);
                        let mut end = text.len().min(STDERR_LINE_BYTES);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        if let Ok(mut tail) = tail.lock() {
                            if tail.len() == STDERR_TAIL_LINES {
                                tail.pop_front();
                            }
                            tail.push_back(text[..end].to_string());
                        }
                    }
                })?;
        }
        Ok(process)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// 受け取ったメッセージの数
    pub fn received_count(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// 本文が JSON でなかったメッセージの数
    pub fn garbage_count(&self) -> u64 {
        self.garbage.load(Ordering::Relaxed)
    }

    /// 応答待ちの数（タイムアウトで消えたかの観測に使う）
    pub fn pending_count(&self) -> usize {
        self.pending.lock().map(|t| t.len()).unwrap_or(0)
    }

    /// stderr の直近の行
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 通知を送る（待たない）。キューが満杯なら [`RpcError::QueueFull`]
    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.send(rpc::frame(&rpc::notification_message(method, params)))
    }

    /// 要求を送って応答を待つ（**UI スレッドから呼ばない**）。
    ///
    /// 期限を過ぎたら待ちの表から消し、`$/cancelRequest` を送ってから
    /// [`RpcError::Timeout`] を返す。所要と結果の種別を perf.log へ残す（本文は書かない）
    pub fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        if self.has_exited() {
            return Err(RpcError::Disconnected);
        }
        let started = Instant::now();
        let (tx, rx) = mpsc::sync_channel(1);
        let id = self
            .pending
            .lock()
            .map_err(|_| RpcError::Disconnected)?
            .register(method, timeout, started, tx);
        if let Err(e) = self.send(rpc::frame(&rpc::request_message(&id, method, params))) {
            self.forget(&id);
            return Err(e);
        }
        let outcome = match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.forget(&id);
                let _ = self.cancel(&id);
                Err(RpcError::Timeout(timeout.as_secs()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(RpcError::Disconnected),
        };
        crate::diag::perf_log(&format!(
            "lsp request: server={} method={method} {}ms {}",
            self.label,
            started.elapsed().as_millis(),
            outcome_kind(&outcome)
        ));
        outcome
    }

    /// `$/cancelRequest` を送る（#1682 の補完が使う口。S1 ではタイムアウトで使う）
    pub fn cancel(&self, id: &RequestId) -> Result<(), RpcError> {
        self.notify(
            "$/cancelRequest",
            serde_json::json!({ "id": id.to_value() }),
        )
    }

    fn forget(&self, id: &RequestId) {
        if let Ok(mut table) = self.pending.lock() {
            table.resolve(id);
        }
    }

    fn send(&self, bytes: Vec<u8>) -> Result<(), RpcError> {
        match self.outbox.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(RpcError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(RpcError::Disconnected),
        }
    }

    /// 終わるまで最大 `timeout` 待つ。終わったら `true`
    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(mut guard) = self.child.lock() {
                match guard.as_mut().map(Child::try_wait) {
                    None | Some(Ok(Some(_))) | Some(Err(_)) => return true,
                    Some(Ok(None)) => {}
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// kill して刈り取る（冪等）
    pub fn kill(&self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 送信キューの 1 件からヘッダを剥がした本文（生ログ用）
fn frame_body(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.split_once("\r\n\r\n").map(|(_, body)| body)
}

fn outcome_kind(outcome: &Result<Value, RpcError>) -> String {
    match outcome {
        Ok(_) => "ok".into(),
        Err(RpcError::Timeout(_)) => "timeout".into(),
        Err(RpcError::Disconnected) => "disconnected".into(),
        Err(RpcError::QueueFull) => "queue_full".into(),
        Err(RpcError::Server(e)) => format!("error={}", e.code),
    }
}

fn handle_incoming(
    message: Value,
    pending: &Mutex<PendingTable<Waiter>>,
    outbox: &SyncSender<Vec<u8>>,
    on_notification: &(dyn Fn(&str, Value) + Send + Sync),
) {
    match rpc::classify(message) {
        Incoming::Response { id, result } => {
            let waiter = pending.lock().ok().and_then(|mut t| t.resolve(&id));
            if let Some((_, waiter)) = waiter {
                let _ = waiter.try_send(result.map_err(RpcError::Server));
            }
        }
        Incoming::Request { id, method, params } => {
            // 無視して黙らない（サーバが応答を待ち続ける）。S1 は既定の応答だけを返す
            let reply = default_reply(&id, &method, &params);
            let _ = outbox.try_send(rpc::frame(&reply));
        }
        Incoming::Notification { method, params } => on_notification(&method, params),
        Incoming::Invalid => {}
    }
}

/// サーバからの要求への既定の応答（S1 は機能を持たないので「何もしない」で答える）
pub fn default_reply(id: &RequestId, method: &str, params: &Value) -> Value {
    match method {
        // 要求された項目の数だけ null（= 設定なし）を返す
        "workspace/configuration" => {
            let count = params
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            rpc::response_message(id, Value::Array(vec![Value::Null; count]))
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        | "workspace/codeLens/refresh"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/diagnostic/refresh" => rpc::response_message(id, Value::Null),
        "workspace/applyEdit" => rpc::response_message(id, serde_json::json!({ "applied": false })),
        _ => rpc::error_response_message(id, rpc::METHOD_NOT_FOUND, "tako は未対応"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 設定の問い合わせには項目数ぶんの_null_を返す() {
        let reply = default_reply(
            &RequestId::Int(4),
            "workspace/configuration",
            &json!({"items": [{"section": "a"}, {"section": "b"}]}),
        );
        assert_eq!(reply["result"], json!([null, null]));
        assert_eq!(reply["id"], json!(4));
    }

    #[test]
    fn 知らない要求にはメソッド未対応のエラーで答える() {
        let reply = default_reply(&RequestId::Str("x".into()), "custom/thing", &Value::Null);
        assert_eq!(reply["error"]["code"], json!(rpc::METHOD_NOT_FOUND));
        let reply = default_reply(
            &RequestId::Int(1),
            "window/workDoneProgress/create",
            &Value::Null,
        );
        assert_eq!(reply["result"], Value::Null);
        assert!(reply.get("error").is_none());
    }

    #[test]
    fn 生ログ用にヘッダを剥がす() {
        let bytes = rpc::frame(&json!({"a": 1}));
        assert_eq!(frame_body(&bytes), Some("{\"a\":1}"));
    }
}

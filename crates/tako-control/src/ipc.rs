//! ipc — Layer 1 IPC サーバー（FR-2.2 の受け口）
//!
//! トランスポート（`.agent/architecture.md` プラットフォーム抽象）:
//! - unix: Unix domain socket（パーミッション 0600 + セッション毎ランダムトークン）
//! - windows: named pipe（**未実装**。Phase 6 で実装。`start` は Unsupported を返し、
//!   アプリは IPC なしで動作を継続する。TODO とリスクは architecture.md「IPC トランスポート」節）
//!
//! スレッド構成: accept スレッド + 接続毎スレッド（ブロッキング IO）。
//! 接続スレッドはリクエストを futures channel で UI スレッドへ渡し、応答を同期で待つ。
//! dispatch の実行は受信側（UI のイベントループ。GPUI executor）で行うため、
//! ここでは tokio 等の非同期ランタイムを持ち込まない（Phase 0 の方針踏襲）。

use std::io;

use futures::channel::mpsc::UnboundedSender;
use tako_core::PaneOrigin;

use crate::dispatch::DispatchError;
use crate::protocol::Request;

/// UI 側へ渡す 1 リクエスト。`reply` へ dispatch の結果を返すと接続スレッドが応答を書く。
/// `origin` は新規生成ペインの生成主体（IPC 直 = Cli、MCP 経由 = Mcp）
pub struct IncomingRequest {
    pub request: Request,
    pub origin: PaneOrigin,
    pub reply: std::sync::mpsc::SyncSender<Result<serde_json::Value, DispatchError>>,
}

/// IPC サーバーのハンドル。drop でソケットファイルを片付ける。
/// `endpoint` はペインのシェルへ `TAKO_SOCKET` として注入する
pub struct IpcServer {
    endpoint: String,
}

impl IpcServer {
    /// サーバーを起動する。受け取った各リクエストは `tx` 経由で UI スレッドへ届く。
    /// `token` はセッション共有の認証トークン（[`crate::generate_token`] で生成し、
    /// MCP サーバーとも共有する。FR-2.3.4）
    pub fn start(tx: UnboundedSender<IncomingRequest>, token: String) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix_imp::start(tx, token)
        }
        #[cfg(windows)]
        {
            let _ = (tx, token);
            // TODO(Phase 6): named pipe（`\\.\pipe\tako-<pid>` 等）での実装
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows の IPC（named pipe）は未実装（Phase 6 で対応）",
            ))
        }
    }

    /// IPC エンドポイント（unix: ソケットパス、windows: パイプ名）
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

#[cfg(unix)]
mod unix_imp {
    use std::io::{BufRead, BufReader, BufWriter, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    use futures::channel::mpsc::UnboundedSender;
    use tako_core::PaneOrigin;

    use super::{IncomingRequest, IpcServer};
    use crate::protocol::{error_code, RequestEnvelope, ResponseEnvelope};

    pub(super) fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
    ) -> std::io::Result<IpcServer> {
        // pid + プロセス内連番でユニーク化（テスト等で複数サーバーを立てても衝突しない）
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tako-{}-{seq}.sock", std::process::id()));
        // 前回残骸（クラッシュ等で remove されなかったもの）を除去
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        // 自ユーザーのプロセスのみ接続可能にする（トークンと二段の防御線）
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        let accept_token = token;
        std::thread::Builder::new()
            .name("tako-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let token = accept_token.clone();
                            let result = std::thread::Builder::new()
                                .name("tako-ipc-conn".into())
                                .spawn(move || handle_connection(stream, &token, &tx));
                            if let Err(e) = result {
                                tracing::warn!("IPC 接続スレッドを起動できない: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("IPC accept に失敗: {e}"),
                    }
                }
            })?;

        Ok(IpcServer {
            endpoint: path.display().to_string(),
        })
    }

    /// 1 接続を処理する（1 行 1 JSON のリクエスト / レスポンス）
    fn handle_connection(stream: UnixStream, token: &str, tx: &UnboundedSender<IncomingRequest>) {
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(stream);
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
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        // MCP stdio ブリッジ（`tako mcp serve`）経由のリクエストは origin = Mcp として扱う
        let origin = match envelope.origin.as_deref() {
            Some("mcp") => PaneOrigin::Mcp,
            _ => PaneOrigin::Cli,
        };
        let incoming = IncomingRequest {
            request: envelope.request,
            origin,
            reply: reply_tx,
        };
        if tx.unbounded_send(incoming).is_err() {
            return ResponseEnvelope::err(
                envelope.id,
                error_code::INTERNAL,
                "アプリ側の受け口が閉じている",
            );
        }
        match reply_rx.recv() {
            Ok(Ok(result)) => ResponseEnvelope::ok(envelope.id, result),
            Ok(Err(e)) => ResponseEnvelope::err(envelope.id, e.code(), e.to_string()),
            Err(_) => ResponseEnvelope::err(
                envelope.id,
                error_code::INTERNAL,
                "アプリ側から応答が返らなかった",
            ),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
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

    #[test]
    fn 不正なトークンは認証エラーで拒否される() {
        let response = roundtrip(Some("bogus-token".into()));
        let error = response.error.expect("エラーになる");
        assert_eq!(error.code, error_code::AUTH);
        assert!(response.result.is_none());
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
}

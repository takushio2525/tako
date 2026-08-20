//! localhost 限定の Streamable HTTP トランスポート。

use std::io;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use super::{handle_message, McpSession};
use crate::ipc::IncomingRequest;
use crate::protocol::Request;

/// リクエストボディの上限（暴走・誤接続対策）
const MAX_BODY_BYTES: u64 = 1 << 20;

/// 内蔵 MCP サーバーのハンドル。`url` をペインのシェルへ `TAKO_MCP_URL` として注入する。
/// ポートはプロセス終了時に OS が解放するため明示シャットダウンは持たない
pub struct McpServer {
    url: String,
}

impl McpServer {
    /// 127.0.0.1 の空きポートで Streamable HTTP サーバーを起動する。
    /// 受け取った各操作は IPC と同じ `tx` 経由で UI スレッドへ届く（dispatch 共有）
    pub fn start(tx: UnboundedSender<IncomingRequest>, token: String) -> io::Result<Self> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| io::Error::other(format!("MCP HTTP サーバーを起動できない: {e}")))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("MCP サーバーのポートを特定できない"))?
            .port();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let token = Arc::new(token);
        std::thread::Builder::new()
            .name("tako-mcp-http".into())
            .spawn(move || {
                for request in server.incoming_requests() {
                    let tx = tx.clone();
                    let token = Arc::clone(&token);
                    std::thread::Builder::new()
                        .name("tako-mcp-req".into())
                        .spawn(move || {
                            handle_http(request, &token, &tx);
                        })
                        .ok();
                }
            })?;
        Ok(Self { url })
    }

    /// 接続先 URL（`TAKO_MCP_URL` として注入する）
    pub fn url(&self) -> &str {
        &self.url
    }
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond(request: tiny_http::Request, status: u16, body: Option<String>) {
    let result = match body {
        Some(body) => {
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("固定値のヘッダ構築は失敗しない");
            // 応答サイズは既知なので常に Content-Length で送る。tiny_http の既定は
            // 32KB 超で chunked に切り替わり、チャンク境界がマルチバイト文字の途中に
            // 落ちると素朴なクライアントの read_to_string が壊れる（ツールカタログが
            // 32KB を超えた際にセルフテストで顕在化）
            request.respond(
                tiny_http::Response::from_string(body)
                    .with_chunked_threshold(usize::MAX)
                    .with_header(header)
                    .with_status_code(status),
            )
        }
        None => request.respond(tiny_http::Response::empty(status)),
    };
    if let Err(e) = result {
        tracing::debug!("MCP 応答の送信に失敗: {e}");
    }
}

fn handle_http(
    mut request: tiny_http::Request,
    token: &str,
    tx: &UnboundedSender<IncomingRequest>,
) {
    // Origin 検証: ブラウザからの DNS リバインディング対策（MCP 仕様の要請）。
    // 非ブラウザクライアントは通常 Origin を送らない
    if let Some(origin) = header_value(&request, "origin") {
        let local = [
            "http://127.0.0.1",
            "http://localhost",
            "https://127.0.0.1",
            "https://localhost",
        ]
        .iter()
        .any(|prefix| origin.starts_with(prefix));
        if !local {
            return respond(request, 403, None);
        }
    }
    // Bearer トークン認証（FR-2.3.4。アプリ外プロセスの拒否）
    let authorized =
        header_value(&request, "authorization").is_some_and(|v| v == format!("Bearer {token}"));
    if !authorized {
        return respond(request, 401, None);
    }
    // Streamable HTTP の必須経路は POST のみ実装（GET の SSE ストリームは任意機能のため
    // 405 を返す。サーバー発のリクエスト・通知を持たないため不要）
    if *request.method() != tiny_http::Method::Post {
        return respond(request, 405, None);
    }
    let caller_pane = header_value(&request, "x-tako-pane").and_then(|v| v.parse().ok());
    let mut body = String::new();
    {
        use std::io::Read as _;
        if request
            .as_reader()
            .take(MAX_BODY_BYTES)
            .read_to_string(&mut body)
            .is_err()
        {
            return respond(request, 400, None);
        }
    }
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        let error = json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": -32700, "message": "リクエストを JSON として解釈できない" },
        });
        return respond(request, 400, Some(error.to_string()));
    };
    let mut exec = |req: Request| -> Result<Value, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        tx.unbounded_send(IncomingRequest {
            request: req,
            origin: PaneOrigin::Mcp,
            reply: reply_tx,
        })
        .map_err(|_| "アプリ側の受け口が閉じている".to_string())?;
        match reply_rx.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("アプリ側から応答が返らなかった".into()),
        }
    };
    let caller_role = header_value(&request, "x-tako-role").map(|v| v.to_string());
    let mut session = McpSession {
        caller_pane,
        caller_role,
        connected: true,
        exec: &mut exec,
        ipc_tx: Some(tx.clone()),
    };
    match handle_message(&message, &mut session) {
        Some(response) => respond(request, 200, Some(response.to_string())),
        // notification（initialized 等）には 202 Accepted を返す（Streamable HTTP 仕様）
        None => respond(request, 202, None),
    }
}

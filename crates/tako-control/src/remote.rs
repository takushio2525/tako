//! remote — リモートアクセス HTTP API サーバー
//!
//! スマホからブラウザ経由でペインを操作するための REST API。
//! tiny_http ベースで、既存の MCP サーバーと同じパターン（Bearer 認証 + dispatch チャネル）。
//!
//! エンドポイント:
//! - `GET  /api/health` — ヘルスチェック
//! - `GET  /api/panes` — ペイン一覧（dispatch List）
//! - `GET  /api/panes/:id/screen` — 画面内容（dispatch Read）
//! - `POST /api/panes/:id/input` — テキスト送信（dispatch Send）
//! - `POST /api/panes/:id/close` — ペイン削除（dispatch Close）
//!
//! 認証: `Authorization: Bearer <token>` ヘッダ必須（/api/health 以外）。
//! CORS: PWA からのアクセス用にワイルドカード許可。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use crate::ipc::IncomingRequest;
use crate::protocol::Request;

const DEFAULT_PORT: u16 = 7749;
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// リモート API サーバーのハンドル
pub struct RemoteServer {
    port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
}

impl RemoteServer {
    /// 指定ポートで HTTP API サーバーを起動する
    pub fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        port: Option<u16>,
    ) -> io::Result<Self> {
        let port = port.unwrap_or(DEFAULT_PORT);
        let addr = format!("0.0.0.0:{port}");
        let server = tiny_http::Server::http(&addr)
            .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
        let actual_port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("remote サーバーのポートを特定できない"))?
            .port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let token_clone = token.clone();
        std::thread::Builder::new()
            .name("tako-remote-http".into())
            .spawn(move || {
                while !shutdown_clone.load(Ordering::Relaxed) {
                    // 100ms タイムアウトで shutdown フラグをチェックする
                    match server.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(Some(request)) => {
                            handle_request(request, &token_clone, &tx);
                        }
                        Ok(None) => {} // タイムアウト
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            port: actual_port,
            token,
            shutdown,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// サーバーを停止する
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn cors_headers() -> Vec<tiny_http::Header> {
    vec![
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..])
            .expect("固定ヘッダ"),
        tiny_http::Header::from_bytes(
            &b"Access-Control-Allow-Methods"[..],
            &b"GET, POST, OPTIONS"[..],
        )
        .expect("固定ヘッダ"),
        tiny_http::Header::from_bytes(
            &b"Access-Control-Allow-Headers"[..],
            &b"Authorization, Content-Type"[..],
        )
        .expect("固定ヘッダ"),
    ]
}

fn respond(request: tiny_http::Request, status: u16, body: Option<String>) {
    let cors = cors_headers();
    let result = match body {
        Some(body) => {
            let ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("固定ヘッダ");
            let mut resp = tiny_http::Response::from_string(body)
                .with_header(ct)
                .with_status_code(status);
            for h in cors {
                resp = resp.with_header(h);
            }
            request.respond(resp)
        }
        None => {
            let mut resp = tiny_http::Response::empty(status);
            for h in cors {
                resp = resp.with_header(h);
            }
            request.respond(resp)
        }
    };
    if let Err(e) = result {
        tracing::debug!("remote 応答の送信に失敗: {e}");
    }
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn check_auth(request: &tiny_http::Request, token: &str) -> bool {
    header_value(request, "authorization").is_some_and(|v| v == format!("Bearer {token}"))
}

/// URL パスからペイン ID を抽出する（`/api/panes/42/screen` → Some(42)）
fn extract_pane_id(path: &str) -> Option<u64> {
    let parts: Vec<&str> = path.split('/').collect();
    // /api/panes/:id/... → ["", "api", "panes", ":id", ...]
    if parts.len() >= 4 && parts[1] == "api" && parts[2] == "panes" {
        parts[3].parse().ok()
    } else {
        None
    }
}

fn exec_dispatch(request: Request, tx: &UnboundedSender<IncomingRequest>) -> Result<Value, String> {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    tx.unbounded_send(IncomingRequest {
        request,
        origin: PaneOrigin::Cli,
        reply: reply_tx,
    })
    .map_err(|_| "アプリ側の受け口が閉じている".to_string())?;
    match reply_rx.recv() {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("応答チャネルが閉じた".to_string()),
    }
}

fn handle_request(
    mut request: tiny_http::Request,
    token: &str,
    tx: &UnboundedSender<IncomingRequest>,
) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    // CORS preflight
    if method == tiny_http::Method::Options {
        return respond(request, 204, None);
    }

    // /api/health は認証不要
    if path == "/api/health" && method == tiny_http::Method::Get {
        return respond(
            request,
            200,
            Some(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }).to_string()),
        );
    }

    // 認証チェック
    if !check_auth(&request, token) {
        return respond(
            request,
            401,
            Some(json!({ "error": "認証が必要" }).to_string()),
        );
    }

    // ルーティング
    match (method, path.as_str()) {
        (tiny_http::Method::Get, "/api/panes") => match exec_dispatch(Request::List, tx) {
            Ok(result) => respond(request, 200, Some(result.to_string())),
            Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
        },
        (tiny_http::Method::Get, p) if p.starts_with("/api/panes/") && p.ends_with("/screen") => {
            let Some(pane_id) = extract_pane_id(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            // クエリパラメータから lines を取得
            let lines = request.url().split('?').nth(1).and_then(|qs| {
                qs.split('&')
                    .find(|p| p.starts_with("lines="))
                    .and_then(|p| p[6..].parse::<usize>().ok())
            });
            match exec_dispatch(
                Request::Read {
                    pane: Some(pane_id),
                    lines,
                    tmux_session: None,
                },
                tx,
            ) {
                Ok(result) => respond(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/input") => {
            let Some(pane_id) = extract_pane_id(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let mut body = String::new();
            {
                use std::io::Read as _;
                if request
                    .as_reader()
                    .take(MAX_BODY_BYTES)
                    .read_to_string(&mut body)
                    .is_err()
                {
                    return respond(
                        request,
                        400,
                        Some(json!({ "error": "リクエストボディの読み取りに失敗" }).to_string()),
                    );
                }
            }
            let parsed: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    return respond(
                        request,
                        400,
                        Some(json!({ "error": format!("JSON パースエラー: {e}") }).to_string()),
                    );
                }
            };
            let text = parsed["text"].as_str().unwrap_or("").to_string();
            let newline = parsed["newline"].as_bool().unwrap_or(true);
            match exec_dispatch(
                Request::Send {
                    pane: Some(pane_id),
                    text,
                    newline,
                    tmux_session: None,
                },
                tx,
            ) {
                Ok(result) => respond(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/close") => {
            let Some(pane_id) = extract_pane_id(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            match exec_dispatch(
                Request::Close {
                    pane: Some(pane_id),
                },
                tx,
            ) {
                Ok(result) => respond(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
            }
        }
        _ => respond(
            request,
            404,
            Some(json!({ "error": "エンドポイントが見つからない" }).to_string()),
        ),
    }
}

/// QR コードをターミナルに表示する（Unicode ブロック文字で描画）
pub fn print_qr(url: &str) {
    use qrcode::QrCode;

    let Ok(code) = QrCode::new(url.as_bytes()) else {
        eprintln!("QR コードの生成に失敗: URL が長すぎる可能性あり");
        return;
    };
    let colors = code.to_colors();
    let width = code.width();

    // 上下2行を1文字（▀ / ▄ / █ / 空白）にまとめて高さを半減させる
    let rows = width.div_ceil(2);
    // quiet zone 用の白行を追加
    let quiet = 2;

    // 上の quiet zone
    for _ in 0..quiet {
        print!("{}", "  ".repeat(width + quiet * 2));
        println!();
    }

    for row in 0..rows {
        // 左 quiet zone
        print!("{}", "  ".repeat(quiet));
        for col in 0..width {
            let top_idx = row * 2;
            let bot_idx = row * 2 + 1;
            let top_dark = colors[top_idx * width + col] == qrcode::Color::Dark;
            let bot_dark = if bot_idx < width {
                colors[bot_idx * width + col] == qrcode::Color::Dark
            } else {
                false
            };
            // 黒背景に白で QR を描く: dark = 黒ピクセル
            match (top_dark, bot_dark) {
                (true, true) => print!("  "),   // 両方黒 → 空白（背景色）
                (true, false) => print!("▄▄"),  // 上黒下白 → 下半ブロック
                (false, true) => print!("▀▀"),  // 上白下黒 → 上半ブロック
                (false, false) => print!("██"), // 両方白 → フルブロック
            }
        }
        // 右 quiet zone
        print!("{}", "  ".repeat(quiet));
        println!();
    }

    // 下の quiet zone
    for _ in 0..quiet {
        print!("{}", "  ".repeat(width + quiet * 2));
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    fn start_test_server() -> (RemoteServer, mpsc::UnboundedReceiver<IncomingRequest>) {
        let (tx, rx) = mpsc::unbounded::<IncomingRequest>();
        let token = "test-token-abc123".to_string();
        let server = RemoteServer::start(tx, token, Some(0)).expect("テストサーバー起動");
        (server, rx)
    }

    fn mock_dispatch(rx: &mut mpsc::UnboundedReceiver<IncomingRequest>, response: Value) {
        if let Ok(req) = rx.try_recv() {
            let _ = req.reply.send(Ok(response));
        }
    }

    fn http_request(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("接続失敗");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let body_bytes = body.unwrap_or("").as_bytes();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\n");
        if let Some(t) = token {
            req.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if body.is_some() {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
        }
        req.push_str("Connection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        if !body_bytes.is_empty() {
            stream.write_all(body_bytes).unwrap();
        }
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        let status_line = response.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = response
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(response.len());
        let resp_body = response[body_start..].to_string();
        (status, resp_body)
    }

    fn get(port: u16, path: &str, token: Option<&str>) -> (u16, String) {
        http_request(port, "GET", path, token, None)
    }

    fn post(port: u16, path: &str, token: &str, body: &str) -> (u16, String) {
        http_request(port, "POST", path, Some(token), Some(body))
    }

    #[test]
    fn extract_pane_id_からidを取り出せる() {
        assert_eq!(extract_pane_id("/api/panes/42/screen"), Some(42));
        assert_eq!(extract_pane_id("/api/panes/0/close"), Some(0));
        assert_eq!(extract_pane_id("/api/panes/abc/input"), None);
        assert_eq!(extract_pane_id("/api/health"), None);
    }

    #[test]
    fn qr生成がパニックしない() {
        print_qr("http://192.168.1.100:7749#token=abc123def456");
    }

    #[test]
    fn healthは認証なしでアクセスできる() {
        let (server, _rx) = start_test_server();
        let (status, body) = get(server.port(), "/api/health", None);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
        server.stop();
    }

    #[test]
    fn 認証なしリクエストは401() {
        let (server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/panes", None);
        assert_eq!(status, 401);
        server.stop();
    }

    #[test]
    fn 不正トークンは401() {
        let (server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/panes", Some("wrong-token"));
        assert_eq!(status, 401);
        server.stop();
    }

    #[test]
    fn ペイン一覧を取得できる() {
        let (server, mut rx) = start_test_server();
        let port = server.port();
        let token = server.token().to_string();

        let handle = std::thread::spawn(move || get(port, "/api/panes", Some(&token)));
        std::thread::sleep(std::time::Duration::from_millis(50));
        mock_dispatch(&mut rx, json!({ "panes": [{"id": 1, "title": "zsh"}] }));
        let (status, body) = handle.join().unwrap();

        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["panes"][0]["id"], 1);
        server.stop();
    }

    #[test]
    fn 画面内容を取得できる() {
        let (server, mut rx) = start_test_server();
        let port = server.port();
        let token = server.token().to_string();

        let handle = std::thread::spawn(move || get(port, "/api/panes/1/screen", Some(&token)));
        std::thread::sleep(std::time::Duration::from_millis(50));
        mock_dispatch(&mut rx, json!({ "lines": ["$ hello", "world"] }));
        let (status, body) = handle.join().unwrap();

        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["lines"][0], "$ hello");
        server.stop();
    }

    #[test]
    fn テキスト送信ができる() {
        let (server, mut rx) = start_test_server();
        let port = server.port();
        let token = server.token().to_string();

        let handle = std::thread::spawn(move || {
            post(
                port,
                "/api/panes/1/input",
                &token,
                r#"{"text":"ls","newline":true}"#,
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        mock_dispatch(&mut rx, json!({ "ok": true }));
        let (status, _) = handle.join().unwrap();

        assert_eq!(status, 200);
        server.stop();
    }

    #[test]
    fn 存在しないエンドポイントは404() {
        let (server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/unknown", Some(server.token()));
        assert_eq!(status, 404);
        server.stop();
    }

    #[test]
    fn サーバーの起動と停止() {
        let (server, _rx) = start_test_server();
        let port = server.port();
        assert!(port > 0);
        assert_eq!(server.token(), "test-token-abc123");

        let (status, _) = get(port, "/api/health", None);
        assert_eq!(status, 200);

        server.stop();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(TcpStream::connect(format!("127.0.0.1:{port}")).is_err());
    }
}

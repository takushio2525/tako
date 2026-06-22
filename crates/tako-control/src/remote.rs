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
//!
//! Phase 3: cloudflared Quick Tunnel 統合 + Workers KV リレー登録。
//! `start()` で cloudflared を自動起動し、tunnel URL を取得して KV に登録する。

use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use crate::ipc::IncomingRequest;
use crate::protocol::Request;

const DEFAULT_PORT: u16 = 7749;
const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// KV リレーの Workers URL（Cloudflare Pages / Workers のデプロイ先）
const DEFAULT_RELAY_URL: &str = "https://tako-remote-relay.shiozawa-takumi.workers.dev";
/// PWA のデプロイ先（Cloudflare Pages）
const DEFAULT_PAGES_URL: &str = "https://tako-remote.pages.dev";

/// リモート API サーバーのハンドル
pub struct RemoteServer {
    port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
    tunnel_url: Option<String>,
    tunnel_process: Option<Child>,
    machine_id: Option<String>,
}

impl RemoteServer {
    /// 指定ポートで HTTP API サーバーを起動する。
    /// `no_tunnel` = false の場合、cloudflared Quick Tunnel も起動する
    pub fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        port: Option<u16>,
        no_tunnel: bool,
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
                    match server.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(Some(request)) => {
                            handle_request(request, &token_clone, &tx);
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })?;

        let mut result = Self {
            port: actual_port,
            token,
            shutdown,
            tunnel_url: None,
            tunnel_process: None,
            machine_id: None,
        };

        if !no_tunnel {
            match start_cloudflared(actual_port) {
                Ok((child, url)) => {
                    result.tunnel_url = Some(url.clone());
                    result.tunnel_process = Some(child);
                    let mid = machine_id();
                    result.machine_id = Some(mid.clone());
                    // KV リレーに登録（失敗しても tunnel 自体は有効）
                    if let Err(e) = register_relay(&mid, &url) {
                        tracing::warn!("KV リレー登録失敗（tunnel は有効）: {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!("cloudflared の起動に失敗（LAN のみモードで継続）: {e}");
                }
            }
        }

        Ok(result)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn tunnel_url(&self) -> Option<&str> {
        self.tunnel_url.as_deref()
    }

    pub fn machine_id(&self) -> Option<&str> {
        self.machine_id.as_deref()
    }

    /// サーバーを停止する
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.tunnel_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- cloudflared Quick Tunnel ---

/// cloudflared を起動して Quick Tunnel URL を取得する
fn start_cloudflared(port: u16) -> io::Result<(Child, String)> {
    let cloudflared = find_cloudflared()?;
    let mut child = Command::new(&cloudflared)
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::other(format!("cloudflared の起動に失敗: {e}")))?;

    // stderr から tunnel URL をパースする（cloudflared は URL を stderr に出力する）
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("cloudflared の stderr を取得できない"))?;

    let url = parse_tunnel_url(stderr)?;
    Ok((child, url))
}

/// PATH から cloudflared を探す
fn find_cloudflared() -> io::Result<String> {
    // PATH のよく使われるパスも含めて探す
    let candidates = [
        "cloudflared",
        "/opt/homebrew/bin/cloudflared",
        "/usr/local/bin/cloudflared",
    ];
    for c in &candidates {
        if Command::new(c)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Ok(c.to_string());
        }
    }
    Err(io::Error::other(
        "cloudflared が見つかりません。インストールしてください: brew install cloudflared",
    ))
}

/// cloudflared の stderr 出力から tunnel URL を読み取る。
/// URL パターン: `https://xxx.trycloudflare.com`
fn parse_tunnel_url(stderr: std::process::ChildStderr) -> io::Result<String> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(stderr);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

    for line in reader.lines() {
        if std::time::Instant::now() > deadline {
            break;
        }
        let line = line?;
        if let Some(url) = extract_trycloudflare_url(&line) {
            return Ok(url);
        }
    }
    Err(io::Error::other(
        "cloudflared から tunnel URL を取得できなかった（30 秒タイムアウト）",
    ))
}

/// 1 行のテキストから trycloudflare.com の URL を抽出する
fn extract_trycloudflare_url(line: &str) -> Option<String> {
    // `https://xxx-yyy-zzz.trycloudflare.com` のパターンを探す
    let marker = ".trycloudflare.com";
    let end_pos = line.find(marker)?;
    let url_end = end_pos + marker.len();
    // https:// の位置を逆方向に探す
    let before = &line[..end_pos];
    let https_pos = before.rfind("https://")?;
    let url = &line[https_pos..url_end];
    // 末尾にパスが続く可能性があるので / まで取る
    Some(url.to_string())
}

// --- マシン ID ---

/// マシン固有の安定 ID を取得する。初回は UUID v4 を生成してファイルに保存する
pub fn machine_id() -> String {
    let path = machine_id_path();
    if let Some(ref p) = path {
        if let Ok(id) = std::fs::read_to_string(p) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(ref p) = path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, &id);
    }
    id
}

fn machine_id_path() -> Option<std::path::PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("machine_id"))
}

// --- KV リレー登録 ---

/// Workers KV リレーに最新の tunnel URL を登録する
fn register_relay(machine_id: &str, tunnel_url: &str) -> Result<(), String> {
    let relay_url =
        std::env::var("TAKO_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    let url = format!("{relay_url}/api/register");
    let body = json!({
        "machineId": machine_id,
        "tunnelUrl": tunnel_url,
    });

    // tiny_http は HTTP クライアントではないので、std の TcpStream で最小 POST を行う
    // または外部の curl / cloudflared に頼る。ここでは軽量に std::process::Command で curl を使う
    let status = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &body.to_string(),
            &url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("curl の実行に失敗: {e}"))?;

    let code = String::from_utf8_lossy(&status.stdout);
    if code.starts_with('2') {
        Ok(())
    } else {
        Err(format!("KV リレー登録が HTTP {code} で失敗"))
    }
}

/// QR コードに含める接続 URL を生成する
pub fn connect_url(
    tunnel_url: Option<&str>,
    local_url: &str,
    token: &str,
    machine_id: Option<&str>,
) -> String {
    let pages_url =
        std::env::var("TAKO_PAGES_URL").unwrap_or_else(|_| DEFAULT_PAGES_URL.to_string());

    match (tunnel_url, machine_id) {
        (Some(tunnel), Some(mid)) => {
            // Phase 3: Pages 経由の接続（tunnel URL + machine ID 付き）
            format!(
                "{pages_url}/connect?host={}&token={}&machine={}",
                urlencoding::encode(tunnel),
                urlencoding::encode(token),
                urlencoding::encode(mid),
            )
        }
        (Some(tunnel), None) => {
            // tunnel あり・machine ID なし: tunnel URL に直接トークン付き
            format!("{tunnel}#token={token}")
        }
        _ => {
            // LAN のみ: 従来の直接 URL
            format!("{local_url}#token={token}")
        }
    }
}

// --- HTTP サーバー（既存コード）---

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

    let rows = width.div_ceil(2);
    let quiet = 2;

    for _ in 0..quiet {
        print!("{}", "  ".repeat(width + quiet * 2));
        println!();
    }

    for row in 0..rows {
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
            match (top_dark, bot_dark) {
                (true, true) => print!("  "),
                (true, false) => print!("▄▄"),
                (false, true) => print!("▀▀"),
                (false, false) => print!("██"),
            }
        }
        print!("{}", "  ".repeat(quiet));
        println!();
    }

    for _ in 0..quiet {
        print!("{}", "  ".repeat(width + quiet * 2));
        println!();
    }
}

// --- URL エンコーディング（最小実装。外部依存なし）---

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push('%');
                    result.push_str(&format!("{b:02X}"));
                }
            }
        }
        result
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
        let server = RemoteServer::start(tx, token, Some(0), true).expect("テストサーバー起動");
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
        let (mut server, _rx) = start_test_server();
        let (status, body) = get(server.port(), "/api/health", None);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
        server.stop();
    }

    #[test]
    fn 認証なしリクエストは401() {
        let (mut server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/panes", None);
        assert_eq!(status, 401);
        server.stop();
    }

    #[test]
    fn 不正トークンは401() {
        let (mut server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/panes", Some("wrong-token"));
        assert_eq!(status, 401);
        server.stop();
    }

    #[test]
    fn ペイン一覧を取得できる() {
        let (mut server, mut rx) = start_test_server();
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
        let (mut server, mut rx) = start_test_server();
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
        let (mut server, mut rx) = start_test_server();
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
        let (mut server, _rx) = start_test_server();
        let (status, _) = get(server.port(), "/api/unknown", Some(server.token()));
        assert_eq!(status, 404);
        server.stop();
    }

    #[test]
    fn サーバーの起動と停止() {
        let (mut server, _rx) = start_test_server();
        let port = server.port();
        assert!(port > 0);
        assert_eq!(server.token(), "test-token-abc123");

        let (status, _) = get(port, "/api/health", None);
        assert_eq!(status, 200);

        server.stop();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(TcpStream::connect(format!("127.0.0.1:{port}")).is_err());
    }

    #[test]
    fn trycloudflare_urlをパースできる() {
        assert_eq!(
            extract_trycloudflare_url(
                "INF |  https://foo-bar-baz.trycloudflare.com                    |"
            ),
            Some("https://foo-bar-baz.trycloudflare.com".to_string()),
        );
        assert_eq!(
            extract_trycloudflare_url("Visit it at: https://abc-123.trycloudflare.com"),
            Some("https://abc-123.trycloudflare.com".to_string()),
        );
        assert_eq!(extract_trycloudflare_url("no url here"), None);
    }

    #[test]
    fn machine_idは安定して返る() {
        let id1 = machine_id();
        let id2 = machine_id();
        assert_eq!(id1, id2);
        assert!(!id1.is_empty());
    }

    #[test]
    fn connect_urlの生成() {
        // tunnel + machine ID あり → Pages 経由
        let url = connect_url(
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "abc123",
            Some("m-uuid"),
        );
        assert!(url.contains("connect?"));
        assert!(url.contains("machine=m-uuid"));

        // tunnel なし → LAN 直接
        let url = connect_url(None, "http://localhost:7749", "abc123", None);
        assert_eq!(url, "http://localhost:7749#token=abc123");
    }

    #[test]
    fn urlエンコーディング() {
        assert_eq!(urlencoding::encode("hello"), "hello");
        assert_eq!(
            urlencoding::encode("https://foo.com"),
            "https%3A%2F%2Ffoo.com"
        );
    }
}

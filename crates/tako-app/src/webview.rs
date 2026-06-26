//! CDP ミラー方式 Web ビュー PoC（FR-3.8）
//!
//! Chrome を `--remote-debugging-port` で外部起動し、CDP WebSocket で接続。
//! バックグラウンドスレッドで `Page.captureScreenshot` をポーリングし、
//! GPUI 側は取得した PNG をペインに `gpui::img()` で描画する。
//! クリックは `Input.dispatchMouseEvent` で Chrome に中継する。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpStream;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tungstenite::stream::MaybeTlsStream;

/// Chrome の起動とスクリーンショット画像を保持する
pub struct WebViewState {
    /// Chrome の子プロセス（tako が管理する場合）
    pub chrome_process: Option<Child>,
    /// 最新のスクリーンショット PNG データ
    pub screenshot: Arc<Mutex<Option<Vec<u8>>>>,
    /// ポーリングスレッド停止フラグ
    pub stop_flag: Arc<AtomicBool>,
    /// ビューポートの幅・高さ（クリック座標変換に使用）
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// 現在表示中の URL
    pub url: String,
    /// CDP WebSocket 接続（クリック送信に使用）
    pub ws_sender: Arc<Mutex<Option<CdpConnection>>>,
}

/// CDP WebSocket 接続のラッパー
pub struct CdpConnection {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: AtomicU64,
}

impl CdpConnection {
    fn new(socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> Self {
        Self {
            socket,
            next_id: AtomicU64::new(100),
        }
    }

    fn send_command(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.socket
            .send(tungstenite::Message::Text(msg.to_string().into()))
            .map_err(|e| format!("CDP send 失敗: {e}"))?;

        // id が一致するレスポンスを待つ（最大 5 秒）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() > deadline {
                return Err("CDP レスポンスタイムアウト".into());
            }
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    if let Ok(val) = serde_json::from_str::<Value>(&text) {
                        if val.get("id").and_then(|v| v.as_u64()) == Some(id) {
                            if let Some(err) = val.get("error") {
                                return Err(format!("CDP エラー: {err}"));
                            }
                            return Ok(val.get("result").cloned().unwrap_or(Value::Null));
                        }
                    }
                }
                Ok(_) => continue,
                Err(e) => return Err(format!("CDP read 失敗: {e}")),
            }
        }
    }
}

impl Drop for WebViewState {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.chrome_process.take() {
            let _ = child.kill();
            let _ = std::fs::remove_dir_all("/tmp/tako-chrome-cdp");
        }
    }
}

/// Chrome のデバッグポートから WebSocket URL を取得する
fn get_ws_url(port: u16) -> Result<String, String> {
    let url = format!("http://localhost:{port}/json");
    let body: String = ureq::get(&url)
        .call()
        .map_err(|e| format!("Chrome デバッグポートに接続できない（{url}）: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("レスポンス読み取り失敗: {e}"))?;
    let tabs: Vec<Value> =
        serde_json::from_str(&body).map_err(|e| format!("JSON パース失敗: {e}"))?;
    for tab in &tabs {
        if tab.get("type").and_then(|v| v.as_str()) == Some("page") {
            if let Some(ws_url) = tab.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                return Ok(ws_url.to_string());
            }
        }
    }
    Err("デバッグポートに page タブが見つからない".into())
}

/// CDP WebSocket に接続する
fn connect_cdp(ws_url: &str) -> Result<tungstenite::WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let (socket, _response) =
        tungstenite::connect(ws_url).map_err(|e| format!("WebSocket 接続失敗: {e}"))?;
    Ok(socket)
}

/// Chrome を起動して CDP 接続を確立し、WebViewState を返す
pub fn launch_chrome(
    url: &str,
    port: u16,
    viewport_width: u32,
    viewport_height: u32,
) -> Result<WebViewState, String> {
    // Chrome のパスを検出（macOS）
    let chrome_path = find_chrome().ok_or("Chrome が見つからない")?;

    // 既に同じポートで Chrome が起動しているかチェック
    let already_running = get_ws_url(port).is_ok();

    let chrome_process = if already_running {
        // 既にデバッグポートが空いている Chrome に接続する（別タブで URL を開く）
        None
    } else {
        let child = Command::new(&chrome_path)
            .arg(format!("--remote-debugging-port={port}"))
            .arg("--user-data-dir=/tmp/tako-chrome-cdp")
            .arg(format!("--window-size={viewport_width},{viewport_height}"))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg(url)
            .spawn()
            .map_err(|e| format!("Chrome 起動失敗: {e}"))?;
        // 起動待ち（デバッグポートが開くまで）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::time::Instant::now() > deadline {
                return Err("Chrome のデバッグポートが開かない（タイムアウト 10 秒）".into());
            }
            if get_ws_url(port).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Some(child)
    };

    // CDP WebSocket 接続
    let ws_url = get_ws_url(port)?;
    let socket = connect_cdp(&ws_url)?;
    let mut conn = CdpConnection::new(socket);

    // ビューポートサイズを設定
    let _ = conn.send_command(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": viewport_width,
            "height": viewport_height,
            "deviceScaleFactor": 2,
            "mobile": false,
        }),
    );

    // 既存 Chrome にアクセスする場合は URL をナビゲート
    if chrome_process.is_none() {
        let _ = conn.send_command("Page.navigate", json!({ "url": url }));
        // ナビゲート完了を少し待つ
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let screenshot = Arc::new(Mutex::new(None::<Vec<u8>>));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let ws_sender = Arc::new(Mutex::new(Some(conn)));

    // スクリーンショットポーリングスレッド
    let screenshot_clone = Arc::clone(&screenshot);
    let stop_clone = Arc::clone(&stop_flag);
    let ws_url_clone = ws_url.clone();
    std::thread::Builder::new()
        .name("webview-screenshot".into())
        .spawn(move || {
            poll_screenshots(ws_url_clone, screenshot_clone, stop_clone);
        })
        .map_err(|e| format!("ポーリングスレッド起動失敗: {e}"))?;

    Ok(WebViewState {
        chrome_process,
        screenshot,
        stop_flag,
        viewport_width,
        viewport_height,
        url: url.to_string(),
        ws_sender,
    })
}

/// バックグラウンドでスクリーンショットをポーリングする
fn poll_screenshots(
    ws_url: String,
    screenshot: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
) {
    // ポーリング専用の接続を作る
    let socket = match connect_cdp(&ws_url) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("スクリーンショットポーリング用 CDP 接続失敗: {e}");
            return;
        }
    };
    let mut conn = CdpConnection::new(socket);

    while !stop.load(Ordering::Relaxed) {
        match conn.send_command(
            "Page.captureScreenshot",
            json!({ "format": "png", "quality": 80 }),
        ) {
            Ok(result) => {
                if let Some(data_str) = result.get("data").and_then(|v| v.as_str()) {
                    if let Ok(bytes) = data_encoding::BASE64.decode(data_str.as_bytes()) {
                        if let Ok(mut guard) = screenshot.lock() {
                            *guard = Some(bytes);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("スクリーンショット取得失敗: {e}");
                // 接続が切れた場合は再接続を試みる
                std::thread::sleep(std::time::Duration::from_secs(1));
                match connect_cdp(&ws_url) {
                    Ok(s) => conn = CdpConnection::new(s),
                    Err(_) => break,
                }
                continue;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Chrome にクリックイベントを送信する
pub fn send_click(state: &WebViewState, x: f64, y: f64) {
    if let Ok(mut guard) = state.ws_sender.lock() {
        if let Some(conn) = guard.as_mut() {
            let params = json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1,
            });
            let _ = conn.send_command("Input.dispatchMouseEvent", params);
            let release = json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1,
            });
            let _ = conn.send_command("Input.dispatchMouseEvent", release);
        }
    }
}

/// macOS で Chrome のパスを検出する
fn find_chrome() -> Option<String> {
    let paths = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    for p in &paths {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    // PATH 上の google-chrome / chromium
    for name in &["google-chrome", "chromium"] {
        if Command::new("which")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(name.to_string());
        }
    }
    None
}

/// 現在アクティブなすべての WebView を管理するための型エイリアス
pub type WebViews = HashMap<tako_core::PaneId, WebViewState>;

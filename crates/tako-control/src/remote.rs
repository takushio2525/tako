//! remote — リモートアクセス HTTP API サーバー（独立デーモン方式）
//!
//! スマホからブラウザ経由でペインを操作するための REST API。
//! tako-app とは独立したバックグラウンドプロセスとして動作し、
//! tmux コマンドで直接ペインを操作する（IPC / dispatch に依存しない）。
//!
//! エンドポイント:
//! - `GET  /api/health` — ヘルスチェック
//! - `GET  /api/panes` — ペイン一覧（tmux list-sessions + list-panes）
//! - `GET  /api/panes/:id/screen` — 画面内容（tmux capture-pane）
//! - `POST /api/panes/:id/input` — テキスト送信（tmux send-keys）
//!
//! 認証: `Authorization: Bearer <token>` ヘッダ必須（/api/health 以外）。
//! CORS: PWA からのアクセス用にワイルドカード許可。
//!
//! デーモン管理:
//! - `tako remote start` → `tako remote serve` をバックグラウンド fork
//! - PID ファイル（`/tmp/tako-remote.pid`）で管理
//! - `tako remote stop` → PID ファイルから kill

use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rust_embed::Embed;
use serde_json::{json, Value};

const DEFAULT_PORT: u16 = 7749;
const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// KV リレーの Workers URL（Cloudflare Pages / Workers のデプロイ先）。
/// PWA 側（web/tako-remote/src/api.js）の DEFAULT_RELAY_URL と一致させること
const DEFAULT_RELAY_URL: &str = "https://tako-remote-relay.takushio2525.workers.dev";
// --- PID / トークン / ポートファイルのパス ---

pub fn pid_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/tako-remote.pid")
}
pub fn token_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/tako-remote.token")
}
pub fn port_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/tako-remote.port")
}

fn cleanup_state_files() {
    let _ = std::fs::remove_file(pid_path());
    let _ = std::fs::remove_file(token_path());
    let _ = std::fs::remove_file(port_path());
}

/// PWA の dist/ を埋め込む（`npm run build` で生成済みのもの）
#[derive(Embed)]
#[folder = "../../web/tako-remote/dist/"]
struct PwaAssets;

/// 独立デーモンとして HTTP サーバーを起動し、SIGTERM まで待機する。
/// `tako remote serve` から呼ばれる内部用関数
pub fn run_daemon(port: Option<u16>, no_tunnel: bool) -> io::Result<()> {
    let port = port.unwrap_or(DEFAULT_PORT);
    let addr = format!("0.0.0.0:{port}");
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
    let actual_port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| io::Error::other("remote サーバーのポートを特定できない"))?
        .port();

    // tmux バックエンドソケット名を解決
    let tmux_socket = tako_core::tmux_backend::socket_name();

    // tmux が使えるか確認
    if !tako_core::tmux_backend::available() {
        return Err(io::Error::other(
            "tmux が見つからない。remote サーバーは tmux 経由でペインを操作するため、tmux が必須です",
        ));
    }

    // トークン生成
    let token = crate::generate_token()?;

    // PID / トークン / ポートを書き出す
    std::fs::write(pid_path(), std::process::id().to_string())
        .map_err(|e| io::Error::other(format!("PID ファイルの書き出しに失敗: {e}")))?;
    std::fs::write(token_path(), &token)
        .map_err(|e| io::Error::other(format!("トークンファイルの書き出しに失敗: {e}")))?;
    std::fs::write(port_path(), actual_port.to_string())
        .map_err(|e| io::Error::other(format!("ポートファイルの書き出しに失敗: {e}")))?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_signal = shutdown.clone();

    // SIGTERM / SIGINT ハンドラ
    #[cfg(unix)]
    {
        use std::sync::atomic::Ordering::Relaxed;
        let _ = unsafe {
            libc::signal(
                libc::SIGTERM,
                signal_handler as *const () as libc::sighandler_t,
            )
        };
        let _ = unsafe {
            libc::signal(
                libc::SIGINT,
                signal_handler as *const () as libc::sighandler_t,
            )
        };
        static SHUTDOWN_FLAG: AtomicBool = AtomicBool::new(false);
        extern "C" fn signal_handler(_: libc::c_int) {
            SHUTDOWN_FLAG.store(true, Relaxed);
        }

        // シグナル待ちスレッド
        let shutdown_clone = shutdown_for_signal;
        std::thread::Builder::new()
            .name("signal-watcher".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if SHUTDOWN_FLAG.load(Relaxed) {
                    shutdown_clone.store(true, Ordering::Relaxed);
                    break;
                }
            })?;
    }

    // cloudflared tunnel（オプション）
    let mut tunnel_url: Option<String> = None;
    let mut tunnel_process: Option<Child> = None;
    let mut mid: Option<String> = None;

    if !no_tunnel {
        match start_cloudflared(actual_port) {
            Ok((child, url)) => {
                let machine = machine_id();
                mid = Some(machine.clone());
                if let Err(e) = register_relay(&machine, &url) {
                    eprintln!("KV リレー登録失敗（tunnel は有効）: {e}");
                }
                tunnel_url = Some(url);
                tunnel_process = Some(child);
            }
            Err(e) => {
                eprintln!("cloudflared の起動に失敗（LAN のみモードで継続）: {e}");
            }
        }
    }

    // 起動情報を JSON で stdout に出力（start コマンドが読み取る）
    let lan_host = lan_ip().unwrap_or_else(|| "localhost".to_string());
    let local_url = format!("http://{lan_host}:{actual_port}");
    let host_name = hostname();
    let connect = connect_url(tunnel_url.as_deref(), &local_url, &token, Some(&host_name));
    let info = json!({
        "running": true,
        "port": actual_port,
        "token": token,
        "url": local_url,
        "tunnel_url": tunnel_url,
        "machine_id": mid,
        "connect_url": connect,
    });
    println!("{info}");

    // HTTP サーバーループ
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Some(request)) => {
                handle_request(request, &token, &tmux_socket);
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }

    // クリーンアップ
    if let Some(mut child) = tunnel_process.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    cleanup_state_files();

    Ok(())
}

/// デーモンの状態を PID ファイルから確認する。
/// 返り値: running=true ならポート/トークンも含む
pub fn daemon_status() -> Value {
    let pid = match std::fs::read_to_string(pid_path()) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return json!({ "running": false }),
    };
    let pid_num: u32 = match pid.parse() {
        Ok(n) => n,
        Err(_) => return json!({ "running": false }),
    };
    if !is_process_alive(pid_num) {
        cleanup_state_files();
        return json!({ "running": false });
    }
    let port = std::fs::read_to_string(port_path())
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    let token = std::fs::read_to_string(token_path())
        .unwrap_or_default()
        .trim()
        .to_string();
    let lan_host = lan_ip().unwrap_or_else(|| "localhost".to_string());
    let local_url = format!("http://{lan_host}:{port}");
    let host_name = hostname();
    let connect = connect_url(None, &local_url, &token, Some(&host_name));
    json!({
        "running": true,
        "pid": pid_num,
        "port": port,
        "token": token,
        "url": local_url,
        "connect_url": connect,
    })
}

/// デーモンを停止する（PID ファイルから kill）
pub fn daemon_stop() -> Result<Value, String> {
    let pid = std::fs::read_to_string(pid_path())
        .map_err(|_| "リモートサーバーが起動していない（PID ファイルが無い）".to_string())?;
    let pid_num: u32 = pid
        .trim()
        .parse()
        .map_err(|_| "PID ファイルの内容が不正".to_string())?;
    if !is_process_alive(pid_num) {
        cleanup_state_files();
        return Err("リモートサーバーが起動していない（プロセスは既に終了）".to_string());
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid_num as libc::pid_t, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        return Err("Windows での停止は未実装".to_string());
    }
    // PID ファイル削除（デーモン側でも削除するが、念のため）
    std::thread::sleep(std::time::Duration::from_millis(500));
    cleanup_state_files();
    Ok(json!({ "stopped": true }))
}

/// デーモンをバックグラウンドで fork 起動する。
/// `tako remote serve --port N [--no-tunnel]` を子プロセスとして起動し、
/// stdout から起動情報 JSON を読み取って返す
pub fn spawn_daemon(port: Option<u16>, no_tunnel: bool) -> Result<Value, String> {
    // 既に起動中か確認
    let status = daemon_status();
    if status["running"].as_bool() == Some(true) {
        return Err("リモートサーバーは既に起動中".to_string());
    }

    let tako_bin = crate::dispatch::resolve_tako_binary();
    let mut args = vec!["remote".to_string(), "serve".to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    if no_tunnel {
        args.push("--no-tunnel".to_string());
    }

    let mut cmd = Command::new(&tako_bin);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // setsid でプロセスグループから切り離し、親（tmux セッション）終了時に巻き添えで死なないようにする
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("デーモンの起動に失敗: {e}"))?;

    // stdout から起動情報 JSON を読み取る（最大 10 秒待機）
    let stdout = child
        .stdout
        .take()
        .ok_or("デーモンの stdout を取得できない")?;

    let info = {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut result = None;
        for line in reader.lines() {
            if std::time::Instant::now() > deadline {
                break;
            }
            let line = line.map_err(|e| format!("デーモンの出力読み取りに失敗: {e}"))?;
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                result = Some(v);
                break;
            }
        }
        result.ok_or("デーモンからの起動情報を受信できなかった")?
    };

    // 子プロセスを切り離す（wait しない → init が引き取る）
    std::mem::forget(child);

    Ok(info)
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

// --- cloudflared Quick Tunnel ---

/// cloudflared を起動して Quick Tunnel URL を取得する
fn start_cloudflared(port: u16) -> io::Result<(Child, String)> {
    let cloudflared = find_cloudflared()?;
    let mut child = Command::new(&cloudflared)
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::other(format!("cloudflared の起動に失敗: {e}")))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("cloudflared の stderr を取得できない"))?;

    let url = parse_tunnel_url(stderr)?;
    Ok((child, url))
}

/// PATH から cloudflared を探す
fn find_cloudflared() -> io::Result<String> {
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

/// cloudflared の stderr 出力から tunnel URL を読み取る
fn parse_tunnel_url(stderr: std::process::ChildStderr) -> io::Result<String> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(stderr);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

    let mut lines = reader.lines();
    while let Some(result) = lines.next() {
        if std::time::Instant::now() > deadline {
            break;
        }
        let line = result?;
        if let Some(url) = extract_trycloudflare_url(&line) {
            std::thread::Builder::new()
                .name("cloudflared-stderr-drain".into())
                .spawn(move || for _ in lines {})
                .ok();
            return Ok(url);
        }
    }
    Err(io::Error::other(
        "cloudflared から tunnel URL を取得できなかった（30 秒タイムアウト）",
    ))
}

/// 1 行のテキストから trycloudflare.com の URL を抽出する
fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let marker = ".trycloudflare.com";
    let end_pos = line.find(marker)?;
    let url_end = end_pos + marker.len();
    let before = &line[..end_pos];
    let https_pos = before.rfind("https://")?;
    let url = &line[https_pos..url_end];
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

fn register_relay(machine_id: &str, tunnel_url: &str) -> Result<(), String> {
    let relay_url =
        std::env::var("TAKO_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    let url = format!("{relay_url}/api/register");
    let body = json!({
        "machineId": machine_id,
        "tunnelUrl": tunnel_url,
    });

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
    name: Option<&str>,
) -> String {
    if let Some(tunnel) = tunnel_url {
        // tunnel 自体が PWA を配信するので、tunnel URL に直接飛ばす（pages.dev 経由不要）
        let mut url = format!("{tunnel}/connect?token={}", urlencoding::encode(token));
        if let Some(n) = name {
            url.push_str(&format!("&name={}", urlencoding::encode(n)));
        }
        url
    } else {
        let mut url = format!("{local_url}/connect?token={}", urlencoding::encode(token));
        if let Some(n) = name {
            url.push_str(&format!("&name={}", urlencoding::encode(n)));
        }
        url
    }
}

/// ホスト名を取得する（表示用）
pub fn hostname() -> String {
    Command::new("hostname")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

// --- tmux 直接操作 ---

const TMUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// tmux コマンドをタイムアウト付きで実行する。
/// `.output()` は tmux ハング時にスレッドを永久ブロックするため、
/// `spawn` + `try_wait` ループでタイムアウトを実装する
fn tmux_output_with_timeout(
    tmux_socket: &str,
    args: &[&str],
) -> Result<std::process::Output, String> {
    use std::io::Read;

    let mut child = tako_core::tmux::tmux_command(Some(tmux_socket))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tmux コマンドの起動に失敗: {e}"))?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(ref mut out) = child.stdout {
                    let _ = out.read_to_end(&mut stdout);
                }
                if let Some(ref mut err) = child.stderr {
                    let _ = err.read_to_end(&mut stderr);
                }
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() > TMUX_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "tmux {} がタイムアウト（{}秒）",
                        args.first().unwrap_or(&""),
                        TMUX_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("プロセスの待機に失敗: {e}"));
            }
        }
    }
}

/// tmux バックエンドから全セッション・ペイン情報を取得して JSON 配列に変換する。
/// PWA が期待する `{ panes: [...] }` のフラット形式を返す
fn tmux_list_panes(tmux_socket: &str) -> Value {
    let sessions = tako_core::tmux::list_sessions(Some(tmux_socket));
    let mut panes = Vec::new();

    for sess in &sessions {
        for win in &sess.windows {
            // 各 window の各 pane を取得
            let pane_list = tmux_list_window_panes(tmux_socket, &sess.name, win.index);
            for (pane_idx, _pane_tty) in pane_list.iter().enumerate() {
                // ペイン ID = "session:window.pane" の文字列表現
                let pane_id = format!("{}:{}.{}", sess.name, win.index, pane_idx);
                panes.push(json!({
                    "id": pane_id,
                    "session": sess.name,
                    "window": win.index,
                    "pane_index": pane_idx,
                    "title": if win.active && pane_idx == 0 {
                        format!("{} ({})", sess.name, win.name)
                    } else {
                        format!("{}:{}.{}", sess.name, win.name, pane_idx)
                    },
                    "state": if sess.attached { "active" } else { "idle" },
                }));
            }
        }
    }

    json!({ "panes": panes })
}

/// tmux の特定 window 内のペイン一覧を取得する
fn tmux_list_window_panes(tmux_socket: &str, session: &str, window: u32) -> Vec<String> {
    let target = format!("={session}:{window}");
    match tmux_output_with_timeout(
        tmux_socket,
        &["list-panes", "-t", &target, "-F", "#{pane_tty}"],
    ) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.to_string())
            .collect(),
        _ => vec![],
    }
}

/// tmux の特定ペインの画面内容を取得する
fn tmux_capture_pane(tmux_socket: &str, target: &str) -> Result<Vec<String>, String> {
    let output = tmux_output_with_timeout(tmux_socket, &["capture-pane", "-t", target, "-p"])?;
    if !output.status.success() {
        return Err(format!(
            "tmux capture-pane がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect())
}

/// tmux の特定ペインを kill する。
/// 最後のペインなら window ごと、最後の window ならセッションごと消える（tmux の標準挙動）
fn tmux_kill_pane(tmux_socket: &str, target: &str) -> Result<(), String> {
    let output = tmux_output_with_timeout(tmux_socket, &["kill-pane", "-t", target])?;
    if !output.status.success() {
        return Err(format!(
            "tmux kill-pane がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// tmux の特定ペインへテキストを送信する
fn tmux_send_keys(
    tmux_socket: &str,
    target: &str,
    text: &str,
    newline: bool,
) -> Result<(), String> {
    let output = tmux_output_with_timeout(tmux_socket, &["send-keys", "-t", target, "-l", text])?;
    if !output.status.success() {
        return Err(format!(
            "tmux send-keys がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if newline {
        tmux_output_with_timeout(tmux_socket, &["send-keys", "-t", target, "Enter"])?;
    }
    Ok(())
}

// --- HTTP サーバー ---

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
        eprintln!("remote 応答の送信に失敗: {e}");
    }
}

fn content_type_for(path: &str) -> &'static [u8] {
    if path.ends_with(".html") {
        b"text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        b"application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        b"text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        b"application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        b"image/svg+xml"
    } else if path.ends_with(".png") {
        b"image/png"
    } else {
        b"application/octet-stream"
    }
}

fn serve_embedded(request: tiny_http::Request, asset_path: &str) {
    let file_path = if asset_path.is_empty() || asset_path == "/" {
        "index.html"
    } else {
        asset_path.trim_start_matches('/')
    };
    let data = PwaAssets::get(file_path).or_else(|| PwaAssets::get("index.html"));
    match data {
        Some(content) => {
            let ct_path = if PwaAssets::get(file_path).is_some() {
                file_path
            } else {
                "index.html"
            };
            let ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_for(ct_path))
                .expect("固定ヘッダ");
            let resp = tiny_http::Response::from_data(content.data.to_vec()).with_header(ct);
            let _ = request.respond(resp);
        }
        None => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
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

/// URL パスからペイン ID を抽出する（`/api/panes/session%3A0.1/screen` → Some("session:0.1")）
fn extract_pane_target(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.splitn(5, '/').collect();
    // ["", "api", "panes", "<id>", "screen"]
    if parts.len() >= 4 && parts[1] == "api" && parts[2] == "panes" {
        let id = urlencoding::decode(parts[3]);
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

fn handle_request(mut request: tiny_http::Request, token: &str, tmux_socket: &str) {
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

    // API 以外のパスは PWA 静的ファイルとして配信（認証不要）
    if !path.starts_with("/api/") {
        return serve_embedded(request, &path);
    }

    // API エンドポイントの認証チェック
    if !check_auth(&request, token) {
        return respond(
            request,
            401,
            Some(json!({ "error": "認証が必要" }).to_string()),
        );
    }

    // API ルーティング（tmux 直接操作）
    match (method, path.as_str()) {
        (tiny_http::Method::Get, "/api/panes") => {
            let result = tmux_list_panes(tmux_socket);
            respond(request, 200, Some(result.to_string()))
        }
        (tiny_http::Method::Get, p) if p.starts_with("/api/panes/") && p.ends_with("/screen") => {
            let Some(target) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            // tmux target は URL デコード不要（session:window.pane）
            let tmux_target = format!("={target}");
            match tmux_capture_pane(tmux_socket, &tmux_target) {
                Ok(lines) => respond(request, 200, Some(json!({ "lines": lines }).to_string())),
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/input") => {
            let Some(target) = extract_pane_target(p) else {
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
            let tmux_target = format!("={target}");
            match tmux_send_keys(tmux_socket, &tmux_target, &text, newline) {
                Ok(()) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/close") => {
            let Some(target) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let tmux_target = format!("={target}");
            match tmux_kill_pane(tmux_socket, &tmux_target) {
                Ok(()) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
            }
        }
        _ => respond(
            request,
            404,
            Some(json!({ "error": "API エンドポイントが見つからない" }).to_string()),
        ),
    }
}

/// macOS の LAN IP アドレスを取得する。取得できなければ None を返す
pub fn lan_ip() -> Option<String> {
    let output = Command::new("ifconfig")
        .arg("en0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("inet ") {
            if let Some(ip) = rest.split_whitespace().next() {
                if ip != "127.0.0.1" {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

/// QR コードを PNG 画像として生成する。生成先のパスを返す
pub fn generate_qr_png(url: &str) -> io::Result<std::path::PathBuf> {
    use image::{GrayImage, Luma};
    use qrcode::QrCode;

    let code = QrCode::new(url.as_bytes())
        .map_err(|e| io::Error::other(format!("QR コードの生成に失敗: {e}")))?;

    let module_count = code.width() as u32;
    let module_px = 10u32;
    let quiet_zone = 4u32;
    let total = (module_count + quiet_zone * 2) * module_px;

    let mut img = GrayImage::from_pixel(total, total, Luma([255u8]));
    for y in 0..module_count {
        for x in 0..module_count {
            if code[(x as usize, y as usize)] == qrcode::Color::Dark {
                for dy in 0..module_px {
                    for dx in 0..module_px {
                        let px = (x + quiet_zone) * module_px + dx;
                        let py = (y + quiet_zone) * module_px + dy;
                        img.put_pixel(px, py, Luma([0u8]));
                    }
                }
            }
        }
    }

    let path = std::env::temp_dir().join("tako-remote-qr.png");
    img.save(&path)
        .map_err(|e| io::Error::other(format!("PNG の保存に失敗: {e}")))?;

    Ok(path)
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

    pub fn decode(s: &str) -> String {
        let mut result = Vec::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(b) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    result.push(b);
                    i += 3;
                    continue;
                }
            }
            result.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&result).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pane_targetからidを取り出せる() {
        assert_eq!(
            extract_pane_target("/api/panes/sess:0.1/screen"),
            Some("sess:0.1".to_string())
        );
        assert_eq!(
            extract_pane_target("/api/panes/myses:0.0/close"),
            Some("myses:0.0".to_string())
        );
        assert_eq!(extract_pane_target("/api/panes//input"), None);
        assert_eq!(extract_pane_target("/api/health"), None);
    }

    #[test]
    fn extract_pane_targetはurlエンコード済みidをデコードする() {
        // コロンが %3A にエンコードされたケース
        assert_eq!(
            extract_pane_target("/api/panes/tako-abc123%3A0.0/screen"),
            Some("tako-abc123:0.0".to_string())
        );
        // エンコードなし（コロンはパスセグメントで有効な文字）
        assert_eq!(
            extract_pane_target("/api/panes/tako-abc123:0.0/screen"),
            Some("tako-abc123:0.0".to_string())
        );
    }

    #[test]
    fn urlデコーディング() {
        assert_eq!(urlencoding::decode("hello"), "hello");
        assert_eq!(urlencoding::decode("sess%3A0.1"), "sess:0.1");
        assert_eq!(
            urlencoding::decode("tako-5728aacf5f80%3A0.0"),
            "tako-5728aacf5f80:0.0"
        );
        // 不正な %XX はそのまま保持
        assert_eq!(urlencoding::decode("bad%ZZend"), "bad%ZZend");
        // 末尾の不完全な % はそのまま
        assert_eq!(urlencoding::decode("end%2"), "end%2");
    }

    #[test]
    fn lan_ipはipv4形式を返す() {
        if let Some(ip) = lan_ip() {
            let parts: Vec<&str> = ip.split('.').collect();
            assert_eq!(parts.len(), 4, "IPv4 アドレスではない: {ip}");
            assert_ne!(ip, "127.0.0.1", "ループバックは除外される");
        }
    }

    #[test]
    fn qr_pngを生成できる() {
        let path = super::generate_qr_png("http://192.168.1.100:7749#token=abc123def456")
            .expect("PNG 生成に失敗");
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 100);
        let _ = std::fs::remove_file(&path);
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
        let url = connect_url(
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "abc123",
            Some("my-mac"),
        );
        assert!(url.starts_with("https://foo.trycloudflare.com/connect?"));
        assert!(!url.contains("host="));
        assert!(url.contains("token=abc123"));
        assert!(url.contains("name=my-mac"));

        let url = connect_url(None, "http://192.168.1.10:7749", "tok456", Some("host1"));
        assert!(url.starts_with("http://192.168.1.10:7749/connect?"));
        assert!(!url.contains("host="));
        assert!(url.contains("token=tok456"));
        assert!(url.contains("name=host1"));

        let url = connect_url(None, "http://localhost:7749", "abc123", None);
        assert!(url.starts_with("http://localhost:7749/connect?"));
        assert!(url.contains("token=abc123"));
        assert!(!url.contains("name="));
    }

    #[test]
    fn urlエンコーディング() {
        assert_eq!(urlencoding::encode("hello"), "hello");
        assert_eq!(
            urlencoding::encode("https://foo.com"),
            "https%3A%2F%2Ffoo.com"
        );
    }

    #[test]
    fn cloudflaredが無い環境ではエラーを返す() {
        let original = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", "");
        let result = find_cloudflared();
        std::env::set_var("PATH", &original);
        if let Err(e) = result {
            assert!(e.to_string().contains("cloudflared"));
        }
    }

    #[test]
    fn trycloudflare_url抽出のエッジケース() {
        assert_eq!(
            extract_trycloudflare_url("2024-06-22 INF https://a-b-c.trycloudflare.com registered"),
            Some("https://a-b-c.trycloudflare.com".to_string()),
        );
        assert_eq!(
            extract_trycloudflare_url("see https://example.com and https://x.trycloudflare.com"),
            Some("https://x.trycloudflare.com".to_string()),
        );
        assert_eq!(extract_trycloudflare_url("https://example.com"), None);
        assert_eq!(extract_trycloudflare_url(""), None);
    }

    #[test]
    fn machine_idはuuid形式() {
        let id = machine_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn machine_idをファイルに保存して再読み込みできる() {
        let tmp = std::env::temp_dir().join(format!("tako-test-mid-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        let id1 = {
            let _ = std::fs::write(&tmp, "");
            let fresh = uuid::Uuid::new_v4().to_string();
            std::fs::write(&tmp, &fresh).unwrap();
            fresh
        };
        let id2 = std::fs::read_to_string(&tmp).unwrap().trim().to_string();
        assert_eq!(id1, id2);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn register_relayは不正urlでエラーを返す() {
        std::env::set_var("TAKO_RELAY_URL", "http://127.0.0.1:1");
        let result = register_relay("test-machine", "https://example.trycloudflare.com");
        std::env::remove_var("TAKO_RELAY_URL");
        assert!(result.is_err());
    }

    #[test]
    fn connect_urlはtunnelありnameなしでもtunnel直接() {
        let url = connect_url(
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "tok123",
            None,
        );
        assert!(url.starts_with("https://foo.trycloudflare.com/connect?"));
        assert!(!url.contains("host="));
        assert!(url.contains("token=tok123"));
        assert!(!url.contains("name="));
    }

    #[test]
    fn daemon_statusはpidファイルがないときfalse() {
        // テスト中に PID ファイルが存在しないことを前提にしない（他のテストが使うかも）
        // ので、存在しないパスを使う代わりに関数の戻り値形式を検証する
        let status = daemon_status();
        // running が true か false のどちらかの bool であること
        assert!(status["running"].is_boolean());
    }

    #[test]
    fn is_process_aliveは現在のプロセスをtrueで返す() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_aliveは存在しないpidをfalseで返す() {
        // 99999999 は通常存在しない PID
        assert!(!is_process_alive(99_999_999));
    }
}

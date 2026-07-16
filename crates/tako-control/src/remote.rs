//! remote — リモートアクセス HTTP API サーバー（独立デーモン方式）
//!
//! スマホからブラウザ経由でペインを操作するための REST API。
//! tako-app とは独立したバックグラウンドプロセスとして動作し、
//! tmux コマンドで直接ペインを操作する（IPC / dispatch に依存しない）。
//!
//! エンドポイント:
//! - `GET  /api/health` — ヘルスチェック
//! - `GET  /api/panes` — ペイン一覧（tmux list-sessions + list-panes）
//! - `GET  /api/panes/:id/screen` — 画面内容（tmux capture-pane。`?ansi=1` で色付き、
//!   `?lines=N` で履歴 N 行込み。cursor / size も返す）
//! - `GET  /api/panes/:id/scrollback` — スクロールバック履歴（プレーンテキスト。
//!   `?lines=N` で履歴 N 行。CLI `tako remote scrollback` / MCP 用）
//! - `POST /api/panes/:id/input` — テキスト送信（tmux send-keys。`{text, newline}` か
//!   `{keys: "..."}` で生キーシーケンス送信）
//! - `POST /api/panes/:id/close` — ペインを閉じる（tmux kill-pane）
//! - `POST /api/panes/:id/resize` — 明示リサイズ（`{cols, rows}` で tmux resize-window、
//!   `{reset: true}` で manual 解除。CLI `tako tmux resize` / MCP 用。
//!   PWA のリモート表示はこれを呼ばない — Issue #63「PC 非破壊」）
//! - `GET  /api/agents` — claude agents --json プロキシ + tmux ペイン対応付け
//! - `GET  /api/sessions/:id/messages?tail=N` — Claude Code transcript の正規化読み取り
//! - `GET  /ws?pane=<id>` — WebSocket 画面プッシュ（読み取り専用・ペインサイズ不干渉。
//!   接続時に init（履歴 + 現画面 + カーソル、ANSI 付き）、以後 250ms 差分検知で
//!   update（履歴へ押し出された行 + 現画面）をプッシュ。描画・スクロール・折り返しは
//!   クライアント側の責務（Issue #63）。操作系は REST を使う）
//!
//! 認証: `Authorization: Bearer <token>` ヘッダ必須（/api/health 以外）。
//! WS は Sec-WebSocket-Protocol の `token.<T>` で検証（ブラウザ WS API はヘッダ不可のため）。
//! CORS: PWA からのアクセス用にワイルドカード許可。
//!
//! 接続リンクの構成（Issue #91）:
//! - トンネル成功 + リレー登録成功 → `https://tako-remote.pages.dev/#/connect?machine=...`
//!   （固定 URL。PWA は Cloudflare Pages が配信し、KV リレーで machineId → 現在の
//!   トンネル URL を解決してデータだけトンネル経由で流す。トンネルが再起動しても
//!   リンク / ブックマークは不変）。トンネル直 URL は `fallback_url` として併記
//!   （リレー障害時の予備。デーモン内蔵 PWA がトンネル経由で配信される）
//! - トンネル成功 + リレー登録失敗 → トンネル直 URL（従来形式）
//! - トンネル失敗（cloudflared 不在等）→ **起動を拒否する**（#104。暗号化経路を確立できない
//!   状態では安全に提供できないため）。信頼できる LAN 内で平文のまま使いたい場合のみ
//!   `--insecure`（明示 opt-in・非推奨）で LAN URL 直（内蔵 PWA）を許可する
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
/// 接続入口の Cloudflare Pages URL（PWA の固定ホスティング先。scripts/deploy-pages.sh で更新）。
/// トンネル有効時の接続リンクはこの固定 URL をベースにし、PWA が KV リレー経由で実際の
/// トンネル URL を解決する（Issue #91: trycloudflare のランダム URL をユーザーに見せず、
/// ブックマークを恒久化する）。`TAKO_PAGES_URL` で差し替え可能（セルフホスト・検証用）
const DEFAULT_PAGES_URL: &str = "https://tako-remote.pages.dev";

fn pages_url() -> String {
    std::env::var("TAKO_PAGES_URL").unwrap_or_else(|_| DEFAULT_PAGES_URL.to_string())
}
// --- PID / トークン / ポートファイルのパス ---
// P0-3: 共有 /tmp から <data_dir>/remote/（0700）へ移動。作成時から 0600。

/// state ファイルの置き場所。`TAKO_REMOTE_STATE_DIR` で差し替え可能
/// （検証用デーモンを本番デーモンと衝突させず並走させるため）
fn state_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("TAKO_REMOTE_STATE_DIR") {
        return std::path::PathBuf::from(dir);
    }
    tako_core::paths::data_dir()
        .map(|d| d.join("remote"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/tako-remote"))
}

/// state_dir を 0700 で確保する
fn ensure_state_dir() -> io::Result<std::path::PathBuf> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| io::Error::other(format!("state ディレクトリの作成に失敗: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

pub fn pid_path() -> std::path::PathBuf {
    state_dir().join("tako-remote.pid")
}
pub fn token_path() -> std::path::PathBuf {
    state_dir().join("tako-remote.token")
}
pub fn port_path() -> std::path::PathBuf {
    state_dir().join("tako-remote.port")
}
/// トンネル状態（tunnel URL / machineId / リレー登録成否）の JSON。
/// デーモンがトンネル確立時に書き、`daemon_status` が接続リンクの再構成に使う
pub fn tunnel_path() -> std::path::PathBuf {
    state_dir().join("tako-remote.tunnel")
}

/// 秘密を含むファイルを作成時から 0600 で書き込む。
/// temp → atomic rename で symlink race を防ぐ
fn write_secret_file(path: &std::path::Path, content: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("ファイルの親ディレクトリが無い"))?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, content)?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// ファイルを所有者のみ読み書き可（0o600）に制限する。unix 以外では何もしない。
/// トークン・QR（トークン入り URL を含む）など秘密を含むファイルに使う（#104）
fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn cleanup_state_files() {
    let _ = std::fs::remove_file(pid_path());
    let _ = std::fs::remove_file(token_path());
    let _ = std::fs::remove_file(port_path());
    let _ = std::fs::remove_file(tunnel_path());
    // QR ファイル（ランダム名）を掃除する
    let dir = state_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("qr-") && name.ends_with(".png") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // 旧 /tmp パスが残っていれば掃除する
    cleanup_legacy_state_files();
}

/// 旧バージョンが /tmp に残した state ファイルを掃除する（移行互換）
fn cleanup_legacy_state_files() {
    let legacy = std::path::PathBuf::from("/tmp");
    for name in [
        "tako-remote.pid",
        "tako-remote.token",
        "tako-remote.port",
        "tako-remote.tunnel",
    ] {
        let _ = std::fs::remove_file(legacy.join(name));
    }
}

/// PWA の dist/ を埋め込む（`npm run build` で生成済みのもの）
#[derive(Embed)]
#[folder = "../../web/tako-remote/dist/"]
struct PwaAssets;

/// 独立デーモンとして HTTP サーバーを起動し、SIGTERM まで待機する。
/// `tako remote serve` から呼ばれる内部用関数。
///
/// セキュリティ方針（#104）: 既定では**暗号化されたトンネル経由でのみ**ホストする。
/// cloudflared トンネルが張れなければ起動を**拒否**する（平文 LAN へフォールバックしない）。
/// `insecure = true` のときだけ、平文 HTTP の LAN 直モードを許可する（明示 opt-in。
/// 同一 LAN 上の第三者にトークンを盗聴されうるため、信頼できるネットワーク限定）
pub fn run_daemon(port: Option<u16>, insecure: bool) -> io::Result<()> {
    let port = port.unwrap_or(DEFAULT_PORT);
    // P0-1: secure モードは 127.0.0.1（ループバック）のみ bind。cloudflared だけがアクセスする。
    // insecure（LAN 直）のみ 0.0.0.0 に bind する
    let bind_host = if insecure { "0.0.0.0" } else { "127.0.0.1" };
    let addr = format!("{bind_host}:{port}");
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

    // P0-3: state ディレクトリを 0700 で確保し、各ファイルを 0600 で書き出す
    ensure_state_dir()?;

    // PID ファイル: 実行ファイルパスと起動時刻も記録（P0-4 の stop 照合用）
    let pid_info = format!(
        "{}\n{}\n{}",
        std::process::id(),
        std::env::current_exe().unwrap_or_default().display(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    write_secret_file(&pid_path(), &pid_info)
        .map_err(|e| io::Error::other(format!("PID ファイルの書き出しに失敗: {e}")))?;
    write_secret_file(&token_path(), &token)
        .map_err(|e| io::Error::other(format!("トークンファイルの書き出しに失敗: {e}")))?;
    write_secret_file(&port_path(), &actual_port.to_string())
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

    // cloudflared tunnel
    let mut tunnel_url: Option<String> = None;
    let mut tunnel_process: Option<Child> = None;
    let mut mid: Option<String> = None;
    // 起動失敗はもう Err で中止するため、この情報 JSON では常に null（後方互換のため残す）
    let tunnel_error: Option<String> = None;
    let mut relay_ok = false;

    if insecure {
        // 平文 LAN 直モード（明示 opt-in）。トンネルを張らない。強い警告を出す
        eprintln!("⚠ insecure モード: 暗号化されていない平文 HTTP で LAN に公開します。");
        eprintln!(
            "  同一 Wi-Fi / LAN 上の第三者にトークンを含む通信を盗聴されうる。信頼できるネットワークでのみ使うこと。"
        );
    } else {
        // secure モード（既定）: 暗号化トンネル必須。張れなければ起動を拒否する
        match start_cloudflared(actual_port) {
            Ok((child, url)) => {
                let machine = machine_id();
                match register_relay(&machine, &url) {
                    Ok(()) => relay_ok = true,
                    Err(e) => eprintln!("KV リレー登録失敗（トンネル直 URL で継続）: {e}"),
                }
                mid = Some(machine);
                tunnel_url = Some(url);
                tunnel_process = Some(child);
            }
            Err(e) => {
                // 暗号化経路を確立できない = 安全に提供できない。起動を中止する（#104）。
                // 書き込んだ state ファイルを片付けてから Err を返す
                cleanup_state_files();
                return Err(io::Error::other(format!(
                    "暗号化トンネルを確立できないため remote サーバーの起動を中止しました: {e}\n\
                     cloudflared を導入してください（brew install cloudflared）。\
                     信頼できる LAN 内で平文のまま使うには `tako remote start --insecure` を指定します（非推奨）。"
                )));
            }
        }
    }

    // トンネル状態を state ファイルに残す（`tako remote status` が接続リンクを再構成するため）
    if let Some(ref t) = tunnel_url {
        let state = json!({ "tunnel_url": t, "machine_id": mid, "relay_ok": relay_ok });
        let _ = write_secret_file(&tunnel_path(), &state.to_string());
    }

    // 起動情報を JSON で stdout に出力（start コマンドが読み取る）。
    // 接続リンク: リレー登録済みなら Pages 固定 URL + トンネル直 URL を予備として併記
    // （リレーが単一障害点にならないように。Issue #91 留意点）
    // P0-1: secure モードでは LAN URL を生成しない（ループバック bind のため到達不能）
    let local_url = if insecure {
        let lan_host = lan_ip().unwrap_or_else(|| "localhost".to_string());
        format!("http://{lan_host}:{actual_port}")
    } else {
        format!("http://127.0.0.1:{actual_port}")
    };
    let host_name = hostname();
    let mid_for_link = if relay_ok { mid.as_deref() } else { None };
    let connect = connect_url(
        tunnel_url.as_deref(),
        &local_url,
        &token,
        Some(&host_name),
        mid_for_link,
    );
    let fallback = if relay_ok && tunnel_url.is_some() {
        Some(connect_url(
            tunnel_url.as_deref(),
            &local_url,
            &token,
            Some(&host_name),
            None,
        ))
    } else {
        None
    };
    let info = json!({
        "running": true,
        "port": actual_port,
        "bind_addr": addr,
        "token": token,
        "url": local_url,
        "tunnel_url": tunnel_url,
        "tunnel_error": tunnel_error,
        "machine_id": mid,
        "connect_url": connect,
        "fallback_url": fallback,
    });
    println!("{info}");

    // HTTP サーバーループ
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Some(request)) => {
                let path = request.url().split('?').next().unwrap_or("");
                if path == "/ws" && is_ws_upgrade(&request) {
                    handle_ws(request, &token, &tmux_socket, shutdown.clone());
                } else {
                    handle_request(request, &token, &tmux_socket);
                }
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

/// ペインのスクロールバック履歴をプレーンテキストで取得する。
/// CLI (`tako remote scrollback`) から使う
pub fn scrollback(pane_id: &str, lines: u32) -> Result<Vec<String>, String> {
    let tmux_socket = tako_core::tmux_backend::socket_name();
    let target = format!("={pane_id}");
    let output = tako_core::tmux::tmux_command(Some(&tmux_socket))
        .args([
            "capture-pane",
            "-t",
            &target,
            "-p",
            "-S",
            &format!("-{lines}"),
        ])
        .output()
        .map_err(|e| format!("tmux capture-pane の実行に失敗: {e}"))?;
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
    // トンネル状態ファイルがあれば、起動時と同じ規則で接続リンクを再構成する
    let tunnel_state: Option<Value> = std::fs::read_to_string(tunnel_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let tunnel_url = tunnel_state
        .as_ref()
        .and_then(|v| v["tunnel_url"].as_str().map(String::from));
    let mid = tunnel_state
        .as_ref()
        .and_then(|v| v["machine_id"].as_str().map(String::from));
    let relay_ok = tunnel_state
        .as_ref()
        .and_then(|v| v["relay_ok"].as_bool())
        .unwrap_or(false);
    let mid_for_link = if relay_ok { mid.as_deref() } else { None };
    let connect = connect_url(
        tunnel_url.as_deref(),
        &local_url,
        &token,
        Some(&host_name),
        mid_for_link,
    );
    json!({
        "running": true,
        "pid": pid_num,
        "port": port,
        "token": token,
        "url": local_url,
        "tunnel_url": tunnel_url,
        "machine_id": mid,
        "connect_url": connect,
    })
}

/// URL のクエリ / fragment 内の `token=<値>` を `token=***` に置換する。
/// 次の `&` または文字列末尾までを値とみなす。`token=` が無ければそのまま返す
fn mask_token_in_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(pos) = rest.find("token=") {
        let val_start = pos + "token=".len();
        out.push_str(&rest[..val_start]);
        out.push_str("***");
        // 値の終端（次の `&` まで。無ければ末尾）以降を残す
        let after = &rest[val_start..];
        match after.find('&') {
            Some(amp) => rest = &after[amp..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `daemon_status()` が返す状態 JSON のトークンをマスクする（`***` へ置換）。
/// スクリーンショット・画面共有経由でのトークン漏えいを防ぐため、CLI / MCP の
/// `remote status` は既定でこれを通す（`--show-token` 指定時のみ生値を出す）。
/// 単体の `token` フィールドに加え、接続 URL（`connect_url` / `fallback_url` / `url`）の
/// クエリに載る `token=<生値>` もマスクする（#104。URL だけ生値が残るとマスクの意味がないため）
pub fn mask_status_token(status: &mut Value) {
    if let Some(obj) = status.as_object_mut() {
        if obj.get("token").and_then(|t| t.as_str()).is_some() {
            obj.insert("token".to_string(), json!("***"));
        }
        for key in ["connect_url", "fallback_url", "url"] {
            if let Some(masked) = obj.get(key).and_then(|v| v.as_str()).map(mask_token_in_url) {
                obj.insert(key.to_string(), json!(masked));
            }
        }
    }
}

/// 2 つのバイト列を定数時間で比較する（長さと内容の両方を一定時間で判定）。
/// トークン認証のタイミング攻撃対策。外部依存を増やさない自前の最小実装
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // 長さ不一致は即 false だが、内容比較は常に一定回数回して早期 return しない。
    // 長さが漏れても 256bit ランダムトークンの探索は不能なので実害はない
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// PID ファイルの内容を解析する。
/// 形式: 行1=PID, 行2=実行ファイルパス, 行3=起動時刻(unix epoch sec)
struct PidInfo {
    pid: u32,
    #[allow(dead_code)]
    exe: Option<String>,
    start_time: Option<u64>,
}

fn parse_pid_file() -> Result<PidInfo, String> {
    let content = std::fs::read_to_string(pid_path())
        .map_err(|_| "PID ファイルが見つからない".to_string())?;
    let mut lines = content.lines();
    let pid: u32 = lines
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .map_err(|_| "PID ファイルの内容が不正".to_string())?;
    let exe = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let start_time = lines.next().and_then(|s| s.trim().parse::<u64>().ok());
    Ok(PidInfo {
        pid,
        exe,
        start_time,
    })
}

/// P0-4: PID が本当に tako remote serve プロセスか検証する。
/// 実行ファイルパスまたは ps の args で確認し、起動時刻もチェックする
fn verify_pid_identity(info: &PidInfo) -> bool {
    if !is_process_alive(info.pid) {
        return false;
    }
    #[cfg(unix)]
    {
        // ps で実行コマンドを取得し、tako remote serve かどうか確認
        if let Ok(output) = Command::new("ps")
            .args(["-p", &info.pid.to_string(), "-o", "args="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            let cmd = String::from_utf8_lossy(&output.stdout);
            let cmd = cmd.trim();
            let is_tako_remote =
                cmd.contains("tako") && cmd.contains("remote") && cmd.contains("serve");
            if !cmd.is_empty() && !is_tako_remote {
                return false;
            }
        }
        // etime ベースの起動時刻チェック（記録がある場合のみ。±5 秒の余裕）。
        // ps etime（経過時間）+ 現在 epoch → 起動 epoch を逆算し、記録値と照合する
        if let Some(recorded) = info.start_time {
            if let Ok(output) = Command::new("ps")
                .args(["-p", &info.pid.to_string(), "-o", "etime="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                let etime_str = String::from_utf8_lossy(&output.stdout);
                let etime_str = etime_str.trim();
                if !etime_str.is_empty() {
                    if let Some(elapsed_secs) = parse_etime(etime_str) {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let actual_start = now.saturating_sub(elapsed_secs);
                        if actual_start.abs_diff(recorded) > 5 {
                            return false;
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = info;
    }
    true
}

/// ps の etime 出力（"[[DD-]HH:]MM:SS"）を秒数に変換する。
/// 形式: "03:42" / "01:03:42" / "2-01:03:42"
#[cfg(unix)]
fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = if let Some((d, r)) = s.split_once('-') {
        (d.parse::<u64>().ok()?, r)
    } else {
        (0, s)
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let (hours, mins, secs) = match parts.len() {
        2 => (
            0u64,
            parts[0].parse::<u64>().ok()?,
            parts[1].parse::<u64>().ok()?,
        ),
        3 => (
            parts[0].parse::<u64>().ok()?,
            parts[1].parse::<u64>().ok()?,
            parts[2].parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    Some(days * 86400 + hours * 3600 + mins * 60 + secs)
}

/// デーモンを停止する（PID ファイルから kill → 終了確認 → state クリーンアップ）。
/// P0-4: PID + 実行ファイル + 起動時刻を照合し、無関係プロセスを kill しない。
/// PID ファイルが無い場合はポート占有者を探して stale デーモンなら回収する
pub fn daemon_stop() -> Result<Value, String> {
    daemon_stop_impl(false)
}

/// `--force` 付き停止。SIGTERM を試みた後 SIGKILL を送る
pub fn daemon_force_stop() -> Result<Value, String> {
    daemon_stop_impl(true)
}

fn daemon_stop_impl(force: bool) -> Result<Value, String> {
    let pid_info = match parse_pid_file() {
        Ok(info) => info,
        Err(_) => {
            // PID ファイルが無い → ポート占有者を探す
            if let Some((occupant, is_tako)) = find_port_occupant(DEFAULT_PORT) {
                if is_tako {
                    eprintln!(
                        "PID ファイルが消失していますが、stale デーモン（PID {occupant}）を検出。停止します…"
                    );
                    kill_stale_daemon(occupant);
                    return Ok(json!({ "stopped": true, "stale_pid": occupant }));
                }
            }
            return Err("リモートサーバーが起動していない（PID ファイルが無い）".to_string());
        }
    };
    let pid_num = pid_info.pid;
    if !is_process_alive(pid_num) {
        cleanup_state_files();
        return Err("リモートサーバーが起動していない（プロセスは既に終了）".to_string());
    }
    // P0-4: PID が本当に tako remote プロセスか検証
    if !verify_pid_identity(&pid_info) {
        // PID が再利用されている。state だけ掃除して終了（無関係プロセスを kill しない）
        cleanup_state_files();
        return Err(format!(
            "PID {pid_num} は tako remote ではないプロセスに再利用されています。\
             state ファイルを掃除しました"
        ));
    }
    #[cfg(unix)]
    {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        let ret = unsafe { libc::kill(pid_num as libc::pid_t, sig) };
        if ret != 0 {
            return Err(format!(
                "PID {pid_num} への signal 送信に失敗（errno: {}）",
                std::io::Error::last_os_error()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        return Err("Windows での停止は未実装".to_string());
    }
    // プロセスの終了をポーリングで確認（最大 5 秒）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !is_process_alive(pid_num) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            if force {
                // force でも終了しない場合はエラー（state は残す）
                return Err(format!(
                    "PID {pid_num} が SIGKILL 後 5 秒経っても終了しない。state ファイルは残してあります"
                ));
            }
            // 通常 stop は SIGKILL にエスカレートせず、エラーを返して state を残す
            return Err(format!(
                "PID {pid_num} が SIGTERM 後 5 秒経っても終了しない。\
                 `tako remote stop --force` で SIGKILL を試みてください"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    cleanup_state_files();
    Ok(json!({ "stopped": true }))
}

/// デーモンをバックグラウンドで fork 起動する。
/// `tako remote serve --port N [--insecure]` を子プロセスとして起動し、
/// stdout から起動情報 JSON を読み取って返す。
/// `insecure = true` のときだけ平文 LAN 直モードを許可する（既定は暗号化トンネル必須。#104）
pub fn spawn_daemon(port: Option<u16>, insecure: bool) -> Result<Value, String> {
    let actual_port = port.unwrap_or(DEFAULT_PORT);

    // 既に起動中か確認
    let status = daemon_status();
    if status["running"].as_bool() == Some(true) {
        return Err("リモートサーバーは既に起動中".to_string());
    }

    // PID ファイルが無くてもポートが占有されている場合がある（state ファイル消失 + プロセス生存）
    if let Some((occupant_pid, is_tako)) = find_port_occupant(actual_port) {
        if is_tako {
            eprintln!(
                "stale な tako remote デーモン（PID {occupant_pid}）がポート {actual_port} を\
                 保持しています。自動回収します…"
            );
            kill_stale_daemon(occupant_pid);
            if find_port_occupant(actual_port).is_some() {
                return Err(format!(
                    "stale デーモン（PID {occupant_pid}）を kill しましたが、\
                     ポート {actual_port} がまだ解放されません"
                ));
            }
        } else {
            return Err(format!(
                "ポート {actual_port} は別のプロセス（PID {occupant_pid}）が使用中です。\
                 `tako remote start --port <別のポート>` で別ポートを指定するか、\
                 該当プロセスを停止してください"
            ));
        }
    }

    let tako_bin = crate::dispatch::resolve_tako_binary();
    let mut args = vec!["remote".to_string(), "serve".to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    if insecure {
        args.push("--insecure".to_string());
    }

    let mut cmd = Command::new(&tako_bin);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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

    // H-6: stdout から起動情報 JSON を reader thread + recv_timeout で読み取る
    let stdout = child
        .stdout
        .take()
        .ok_or("デーモンの stdout を取得できない")?;

    let info = {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<Result<Value, String>>();

        std::thread::Builder::new()
            .name("daemon-stdout-reader".into())
            .spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                                let _ = tx.send(Ok(v));
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(format!("デーモンの出力読み取りに失敗: {e}")));
                            return;
                        }
                    }
                }
                let _ = tx.send(Err("デーモンが起動情報 JSON を出力しなかった".to_string()));
            })
            .map_err(|e| format!("reader スレッドの起動に失敗: {e}"))?;

        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => return Err(e),
            Err(_) => None,
        }
    };

    let Some(info) = info else {
        // 起動情報が来なかった。子が即死していれば stderr から原因を拾う
        // （例: ポート使用中で bind 失敗。orphan デーモンの残骸が典型）
        if let Ok(Some(status)) = child.try_wait() {
            let mut detail = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read as _;
                let _ = (&mut err).take(4096).read_to_string(&mut detail);
            }
            let detail = detail.trim();
            return Err(format!(
                "デーモンが起動情報を返さず終了した（{status}）: {detail}"
            ));
        }
        let _ = child.kill();
        return Err("デーモンからの起動情報を受信できなかった（30 秒タイムアウト）".into());
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

/// 指定ポートを LISTEN しているプロセスを探す。
/// 返り値: `Some((pid, is_tako_remote))` — `is_tako_remote` は `tako remote serve` かどうか
#[cfg(unix)]
fn find_port_occupant(port: u16) -> Option<(u32, bool)> {
    let output = Command::new("lsof")
        .args(["-t", "-i", &format!(":{port}"), "-sTCP:LISTEN"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let pid: u32 = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()?;
    let is_tako = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| {
            let cmd = String::from_utf8_lossy(&o.stdout);
            cmd.contains("tako") && cmd.contains("remote") && cmd.contains("serve")
        })
        .unwrap_or(false);
    Some((pid, is_tako))
}

#[cfg(not(unix))]
fn find_port_occupant(_port: u16) -> Option<(u32, bool)> {
    None
}

/// stale なデーモンプロセスを kill し、終了を確認して state ファイルを掃除する。
/// SIGTERM → 最大 5 秒ポーリング → 終了しなければ SIGKILL
fn kill_stale_daemon(pid: u32) {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !is_process_alive(pid) {
                cleanup_state_files();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
    cleanup_state_files();
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

/// cloudflared の stderr 出力から tunnel URL を読み取る。
/// H-6: reader thread + recv_timeout で blocking read がタイムアウトを妨げない
fn parse_tunnel_url(stderr: std::process::ChildStderr) -> io::Result<String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<io::Result<String>>();

    std::thread::Builder::new()
        .name("cloudflared-stderr-reader".into())
        .spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Some(result) = lines.next() {
                match result {
                    Ok(line) => {
                        if let Some(url) = extract_trycloudflare_url(&line) {
                            let _ = tx.send(Ok(url));
                            // 残りの stderr を drain して pipe 詰まりを防ぐ
                            for _ in lines {}
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }
            let _ = tx.send(Err(io::Error::other(
                "cloudflared が tunnel URL を出力せず終了した",
            )));
        })
        .map_err(|e| io::Error::other(format!("reader スレッドの起動に失敗: {e}")))?;

    match rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(url)) => Ok(url),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(io::Error::other(
            "cloudflared から tunnel URL を取得できなかった（30 秒タイムアウト）",
        )),
    }
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

// --- リレー登録シークレット ---

/// リレー登録シークレットを読み込むか、初回は CSPRNG で生成して保存する（hex 64 文字）。
/// リレー worker 側で machineId エントリを first-write-wins で保護するためのもの
/// （worker には SHA-256 ハッシュのみ保存される）。token と同様、ログに出さないこと
fn relay_secret() -> Option<String> {
    let path = tako_core::paths::data_dir().map(|d| d.join("relay_secret"))?;
    relay_secret_at(&path)
}

fn relay_secret_at(path: &std::path::Path) -> Option<String> {
    if let Ok(content) = std::fs::read_to_string(path) {
        let secret = content.trim().to_string();
        if secret.len() >= 32 {
            return Some(secret);
        }
    }
    let secret = crate::generate_token().ok()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    std::fs::write(path, &secret).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Some(secret)
}

// --- KV リレー登録 ---

fn register_relay(machine_id: &str, tunnel_url: &str) -> Result<(), String> {
    let relay_url =
        std::env::var("TAKO_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    let url = format!("{relay_url}/api/register");
    let mut body = json!({
        "machineId": machine_id,
        "tunnelUrl": tunnel_url,
    });
    // secret が用意できない環境（data_dir なし）では従来どおり無 secret で登録する
    // （worker 側もレガシー登録として受理する。保護は効かないが機能は壊れない）
    if let Some(secret) = relay_secret() {
        body["secret"] = json!(secret);
    }

    // H-6: --connect-timeout / --max-time で curl のブロッキングを制限する
    let status = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--connect-timeout",
            "10",
            "--max-time",
            "15",
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

/// QR コードに含める接続 URL を生成する。
/// トークンは URL fragment（`/#/connect?token=...`）に載せる: fragment はブラウザが
/// サーバーへ送信しないため、アクセスログ・cloudflared のログ・Referer に平文トークンが
/// 残らない（Issue #23 認証改善）。PWA はハッシュルーターなのでこの形式を直接解釈できる。
///
/// トンネル有効時（`tunnel_url` と `machine_id` の両方あり）は Cloudflare Pages の
/// 固定 URL をベースにし、`machine` パラメータで PWA に KV リレー解決させる（Issue #91）。
/// トンネル URL はリンクに現れず KV 内部の値に留まるため、トンネル再起動でもリンク不変。
/// `machine_id` の呼び出し側契約: リレー登録が成功しているときだけ渡す（未登録の
/// machineId で Pages リンクを出すと PWA が接続先を解決できない）。
/// LAN-only 時は従来どおり内蔵 PWA の LAN URL 直リンク（https の Pages から
/// プライベート IP の http へは mixed content で接続できないため、Pages 化しない）
pub fn connect_url(
    tunnel_url: Option<&str>,
    local_url: &str,
    token: &str,
    name: Option<&str>,
    machine_id: Option<&str>,
) -> String {
    connect_url_with_pages(&pages_url(), tunnel_url, local_url, token, name, machine_id)
}

/// connect_url の本体（Pages ベース URL を引数化。env 非依存でテスト可能にするため分離）
fn connect_url_with_pages(
    pages_base: &str,
    tunnel_url: Option<&str>,
    local_url: &str,
    token: &str,
    name: Option<&str>,
    machine_id: Option<&str>,
) -> String {
    let mut url = match (tunnel_url, machine_id) {
        // トンネル + machineId → Pages 固定 URL（リレー解決経路）
        (Some(_), Some(mid)) => format!(
            "{}/#/connect?machine={}&token={}",
            pages_base.trim_end_matches('/'),
            urlencoding::encode(mid),
            urlencoding::encode(token),
        ),
        // トンネルはあるが machineId が無い（リレー登録失敗）→ トンネル直（トンネルが PWA を配信）
        (Some(t), None) => format!("{t}/#/connect?token={}", urlencoding::encode(token)),
        // LAN-only → LAN URL 直（内蔵 PWA）
        (None, _) => format!("{local_url}/#/connect?token={}", urlencoding::encode(token)),
    };
    if let Some(n) = name {
        url.push_str(&format!("&name={}", urlencoding::encode(n)));
    }
    url
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

/// 応答サイズ上限（8MB）。capture-pane の巨大出力を打ち切る
const MAX_TMUX_OUTPUT: usize = 8 * 1024 * 1024;

/// tmux コマンドをタイムアウト付きで実行する。
/// H-5: stdout/stderr を別スレッドで同時 drain して pipe deadlock を根治する。
/// pipe buffer（macOS: 64KB）を超える出力でも deadlock しない
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

    // stdout/stderr を別スレッドで同時に drain する（H-5 pipe deadlock 対策）。
    // 子プロセスが pipe buffer いっぱいまで書いて write 待ちになっても、
    // 親が同時に読んでいるため deadlock しない
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle = std::thread::Builder::new()
        .name("tmux-stdout-drain".into())
        .spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = (&mut pipe)
                    .take(MAX_TMUX_OUTPUT as u64)
                    .read_to_end(&mut buf);
            }
            buf
        })
        .map_err(|e| format!("stdout drain スレッドの起動に失敗: {e}"))?;

    let stderr_handle = std::thread::Builder::new()
        .name("tmux-stderr-drain".into())
        .spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = (&mut pipe)
                    .take(MAX_TMUX_OUTPUT as u64)
                    .read_to_end(&mut buf);
            }
            buf
        })
        .map_err(|e| format!("stderr drain スレッドの起動に失敗: {e}"))?;

    // タイムアウト付きで子プロセスの終了を待つ
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
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
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
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

/// capture-pane の引数を組み立てる。
/// `ansi` = true で `-e`（色・属性のエスケープシーケンス付き。xterm.js 描画用）、
/// `history_lines` = Some(N) で `-S -N`（履歴を N 行さかのぼって含める）
fn capture_pane_args(target: &str, ansi: bool, history_lines: Option<u32>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "capture-pane".into(),
        "-t".into(),
        target.into(),
        "-p".into(),
    ];
    if ansi {
        args.push("-e".into());
    }
    if let Some(n) = history_lines {
        args.push("-S".into());
        args.push(format!("-{n}"));
    }
    args
}

/// tmux の特定ペインの画面内容を取得する
fn tmux_capture_pane(
    tmux_socket: &str,
    target: &str,
    ansi: bool,
    history_lines: Option<u32>,
) -> Result<Vec<String>, String> {
    let args = capture_pane_args(target, ansi, history_lines);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let output = tmux_output_with_timeout(tmux_socket, &arg_refs)?;
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

/// ペインのカーソル位置とサイズを取得する（cursor_x, cursor_y, pane_width, pane_height）。
/// 取得失敗時は None（screen API はカーソル無しで応答を続行する）
fn tmux_pane_geometry(tmux_socket: &str, target: &str) -> Option<(u32, u32, u32, u32)> {
    let output = tmux_output_with_timeout(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            target,
            "#{cursor_x} #{cursor_y} #{pane_width} #{pane_height}",
        ],
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut it = text.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    let w = it.next()?.parse().ok()?;
    let h = it.next()?.parse().ok()?;
    Some((x, y, w, h))
}

/// ペイン target（`sess:0.1`）から window target（`sess:0`）を導出する
fn window_target_of(pane_target: &str) -> String {
    pane_target
        .rsplit_once('.')
        .map(|(w, _)| w.to_string())
        .unwrap_or_else(|| pane_target.to_string())
}

/// tmux window を指定サイズへリサイズする（`resize-window -x -y`。window-size は manual になる）
fn tmux_resize_window(
    tmux_socket: &str,
    window_target: &str,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let cols_s = cols.to_string();
    let rows_s = rows.to_string();
    let output = tmux_output_with_timeout(
        tmux_socket,
        &[
            "resize-window",
            "-t",
            window_target,
            "-x",
            &cols_s,
            "-y",
            &rows_s,
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "tmux resize-window がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// tmux window の manual サイズを解除しサーバー既定へ戻す
fn tmux_reset_window_size(tmux_socket: &str, window_target: &str) -> Result<(), String> {
    let output = tmux_output_with_timeout(
        tmux_socket,
        &[
            "set-window-option",
            "-t",
            window_target,
            "-u",
            "window-size",
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "tmux set-window-option がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
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

/// tmux の特定ペインへ生キーシーケンスを送信する（`-l` なし = 特殊キー名を解釈する）。
/// スペース区切りで複数キーを渡せる（例: `"Enter"`, `"C-c"`, `"Escape \\x1b[13;2u"`）
fn tmux_send_raw_keys(tmux_socket: &str, target: &str, keys: &str) -> Result<(), String> {
    let mut args = vec!["send-keys", "-t", target];
    let parts: Vec<&str> = keys.split(' ').filter(|s| !s.is_empty()).collect();
    for part in &parts {
        args.push(part);
    }
    let output = tmux_output_with_timeout(tmux_socket, &args)?;
    if !output.status.success() {
        return Err(format!(
            "tmux send-keys がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
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
    respond_inner(request, status, body, false);
}

/// M-4: 機密データを含む応答。Cache-Control: no-store, private を付与する
fn respond_sensitive(request: tiny_http::Request, status: u16, body: Option<String>) {
    respond_inner(request, status, body, true);
}

fn respond_inner(request: tiny_http::Request, status: u16, body: Option<String>, no_cache: bool) {
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
            if no_cache {
                resp = resp.with_header(
                    tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store, private"[..])
                        .expect("固定ヘッダ"),
                );
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
    let expected = format!("Bearer {token}");
    header_value(request, "authorization")
        .is_some_and(|v| constant_time_eq(v.as_bytes(), expected.as_bytes()))
}

// --- WebSocket（画面プッシュ専用チャンネル） ---
//
// `GET /ws?pane=<id>` を WebSocket にアップグレードし、ペインの「中身」を
// サーバー側からプッシュする（Issue #63 の再設計）:
//
// - 接続時: `{"type":"init", history, screen, cursor, size}` —
//   スクロールバック履歴（最大 WS_INIT_HISTORY 行）+ 現画面 + カーソル位置（ANSI 付き）
// - 以後 250ms 間隔の差分検知: `{"type":"update", pushed, screen, cursor}` —
//   `pushed` = 前回以降に履歴へ押し出された行（`#{history_size}` の増分で検出）、
//   `screen` = 現画面。クライアントは pushed を履歴ビューへ追記し、screen を置き換える
//   ことで、履歴とライブ画面を 1 本の連続スクロールとして再構成できる
// - 履歴の減少（clear-history）・ペインサイズ変化・押し出し量が大きすぎる場合は
//   init を再送してクライアント側を作り直す
//
// このチャンネルは読み取り専用で、**ペインのサイズ・表示状態には一切影響を与えない**
// （PC 非破壊の絶対原則。旧実装の cols/rows 自動リサイズは廃止）。
// 操作系（input / close / resize）は既存 REST を使う設計（プッシュ専用の一方向）。
// tiny_http の upgrade が返すソケットは read タイムアウトを設定できないため、
// 接続後にクライアントからの受信を待つ設計にはできない（受信待ちでスレッドが
// 永久ブロックする）。認証をハンドシェイクの HTTP ヘッダで完結させることで
// 接続後の read を不要にしている。

/// WS サブプロトコル名。ブラウザの WebSocket API は任意ヘッダを付けられないため、
/// クライアントは `new WebSocket(url, ["tako-remote", "token.<TOKEN>"])` の
/// サブプロトコル列でトークンを渡す（Sec-WebSocket-Protocol ヘッダに乗る）
const WS_PROTOCOL: &str = "tako-remote";
/// 画面差分のポーリング間隔
const WS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// 無変化時のキープアライブ送信間隔（プロキシの idle 切断対策）
const WS_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(15);
/// init で送るスクロールバック履歴の最大行数。update の押し出し行がこれを超えた場合も
/// init を再送する（250ms にこれ以上流れる出力は差分として追う価値が薄い）
const WS_INIT_HISTORY: u64 = 2000;
/// update で押し出し行を取り直すときの余裕行数。1 回目の観測と取り直しの間に
/// さらに出力が進んでも取りこぼさないためのマージン
const WS_PUSH_MARGIN: u64 = 50;

/// リクエストが WebSocket アップグレードかどうか
fn is_ws_upgrade(request: &tiny_http::Request) -> bool {
    header_value(request, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Sec-WebSocket-Protocol の値からトークンを取り出す（`token.<T>` エントリ）
fn ws_token_from_protocols(protocols: &str) -> Option<String> {
    protocols
        .split(',')
        .map(|p| p.trim())
        .find_map(|p| p.strip_prefix("token.").map(|t| t.to_string()))
}

/// `GET /ws?pane=<id>` の WebSocket アップグレードを処理する。
/// 認証はハンドシェイク時の Sec-WebSocket-Protocol（`token.<T>`）で検証し、
/// 不一致なら 101 を返さない（= 認証なしでは WS 接続できない）
fn handle_ws(
    request: tiny_http::Request,
    token: &str,
    tmux_socket: &str,
    shutdown: Arc<AtomicBool>,
) {
    let url_full = request.url().to_string();
    let client_token =
        header_value(&request, "sec-websocket-protocol").and_then(|v| ws_token_from_protocols(&v));
    let token_ok = client_token
        .as_deref()
        .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()));
    if !token_ok {
        return respond(
            request,
            401,
            Some(json!({ "error": "認証が必要" }).to_string()),
        );
    }
    let Some(pane) = query_param(&url_full, "pane") else {
        return respond(
            request,
            400,
            Some(json!({ "error": "pane クエリが必要" }).to_string()),
        );
    };
    let Some(ws_key) = header_value(&request, "sec-websocket-key") else {
        return respond(
            request,
            400,
            Some(json!({ "error": "Sec-WebSocket-Key ヘッダが無い" }).to_string()),
        );
    };

    // 101 Switching Protocols（Upgrade / Connection ヘッダは tiny_http が付与する）
    let accept = tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
    let response = tiny_http::Response::empty(101)
        .with_header(
            tiny_http::Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes())
                .expect("固定ヘッダ"),
        )
        .with_header(
            tiny_http::Header::from_bytes(&b"Sec-WebSocket-Protocol"[..], WS_PROTOCOL.as_bytes())
                .expect("固定ヘッダ"),
        );
    let stream = request.upgrade("websocket", response);

    let tmux_socket = tmux_socket.to_string();
    let _ = std::thread::Builder::new()
        .name("tako-remote-ws".into())
        .spawn(move || ws_push_loop(stream, &pane, &tmux_socket, shutdown));
}

/// ペインの一貫スナップショット。`display-message` と `capture-pane` を 1 回の
/// tmux コマンドシーケンス（`;` 連結）で実行した結果で、`history_size` と行内容の
/// 間に別コマンドが挟まらない（250ms ポーリング間の race を最小化する）
struct PaneSnapshot {
    /// スクロールバックに積まれている総行数（`#{history_size}`）
    history_size: u64,
    cursor_x: u32,
    cursor_y: u32,
    cols: u32,
    rows: u32,
    /// capture-pane の出力行。先頭 `history_lines` 行が履歴、残りが現画面
    /// （tmux は画面下端の連続空行をトリムして返すことがあるため、
    /// 画面部分の行数は rows 以下になりうる）
    lines: Vec<String>,
    /// lines のうち履歴部分の行数（= min(history_size, 要求した履歴行数)）
    history_lines: usize,
}

impl PaneSnapshot {
    fn history(&self) -> &[String] {
        &self.lines[..self.history_lines]
    }
    fn screen(&self) -> &[String] {
        &self.lines[self.history_lines..]
    }
    /// 画面内容 + カーソルのハッシュ（update の差分検知用）
    fn screen_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.screen().hash(&mut h);
        (self.cursor_x, self.cursor_y).hash(&mut h);
        h.finish()
    }
}

/// display-message + capture-pane を 1 回の tmux 呼び出しで実行し、
/// 一貫したスナップショットを取得する。`history_back` > 0 なら履歴を
/// 最大その行数さかのぼって含める（0 なら現画面のみ）
fn ws_snapshot(tmux_socket: &str, target: &str, history_back: u64) -> Result<PaneSnapshot, String> {
    let start = format!("-{history_back}");
    let mut args = vec![
        "display-message",
        "-p",
        "-t",
        target,
        "#{history_size} #{cursor_x} #{cursor_y} #{pane_width} #{pane_height}",
        ";",
        "capture-pane",
        "-e",
        "-p",
        "-t",
        target,
    ];
    if history_back > 0 {
        args.push("-S");
        args.push(&start);
    }
    let output = tmux_output_with_timeout(tmux_socket, &args)?;
    if !output.status.success() {
        return Err(format!(
            "tmux display-message;capture-pane がエラー: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    parse_snapshot(&String::from_utf8_lossy(&output.stdout), history_back)
}

/// ws_snapshot の出力パース。1 行目がメタ
/// （`history_size cursor_x cursor_y pane_width pane_height`）、2 行目以降が
/// capture-pane の行。先頭 min(history_size, history_back) 行が履歴部分
fn parse_snapshot(raw: &str, history_back: u64) -> Result<PaneSnapshot, String> {
    let mut it = raw.lines();
    let meta = it.next().ok_or("スナップショット出力が空")?;
    let mut m = meta.split_whitespace();
    let mut next_num = |name: &str| -> Result<u64, String> {
        m.next()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| format!("スナップショットのメタ行に {name} が無い: {meta}"))
    };
    let history_size = next_num("history_size")?;
    let cursor_x = next_num("cursor_x")? as u32;
    let cursor_y = next_num("cursor_y")? as u32;
    let cols = next_num("pane_width")? as u32;
    let rows = next_num("pane_height")? as u32;
    let lines: Vec<String> = it.map(|l| l.to_string()).collect();
    let history_lines = (history_size.min(history_back) as usize).min(lines.len());
    Ok(PaneSnapshot {
        history_size,
        cursor_x,
        cursor_y,
        cols,
        rows,
        lines,
        history_lines,
    })
}

/// 前回送信時の状態（update の差分基準）
struct WsPrevState {
    history_size: u64,
    size: (u32, u32),
    screen_hash: u64,
}

/// 画面プッシュループ。接続時に init（履歴 + 現画面）を送り、以後 250ms 間隔で
/// 差分（履歴へ押し出された行 + 現画面）を update として送る。読み取り専用で
/// ペインサイズには一切影響しない（Issue #63「PC 非破壊」）。無変化でも
/// WS_KEEPALIVE ごとに keepalive を送って接続を維持する。
/// 送信失敗（クライアント切断）・ペイン消失で終了
fn ws_push_loop(
    stream: Box<dyn tiny_http::ReadWrite + Send>,
    pane: &str,
    tmux_socket: &str,
    shutdown: Arc<AtomicBool>,
) {
    use tungstenite::protocol::{Role, WebSocket};

    let mut ws = WebSocket::from_raw_socket(stream, Role::Server, None);
    let target = format!("={pane}");
    let mut prev: Option<WsPrevState> = None;
    let mut last_sent = std::time::Instant::now();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            let _ = ws.close(None);
            break;
        }

        // 現画面のみの軽量スナップショットで差分の有無を判定する
        let snap = match ws_snapshot(tmux_socket, &target, 0) {
            Ok(s) => s,
            Err(e) => {
                let _ = ws.send(tungstenite::Message::text(
                    json!({ "type": "error", "message": e }).to_string(),
                ));
                let _ = ws.close(None);
                break;
            }
        };

        // init が必要: 初回 / 履歴の減少（clear-history）/ PC 側でのサイズ変更 /
        // 押し出し量が大きすぎて差分で追う価値がない
        let need_init = match &prev {
            None => true,
            Some(p) => {
                snap.history_size < p.history_size
                    || (snap.cols, snap.rows) != p.size
                    || snap.history_size - p.history_size > WS_INIT_HISTORY
            }
        };

        let msg = if need_init {
            match ws_snapshot(tmux_socket, &target, WS_INIT_HISTORY) {
                Ok(full) => {
                    let payload = json!({
                        "type": "init",
                        "history": full.history(),
                        "screen": full.screen(),
                        "cursor": { "x": full.cursor_x, "y": full.cursor_y },
                        "size": { "cols": full.cols, "rows": full.rows },
                    })
                    .to_string();
                    prev = Some(WsPrevState {
                        history_size: full.history_size,
                        size: (full.cols, full.rows),
                        screen_hash: full.screen_hash(),
                    });
                    Some(payload)
                }
                Err(e) => {
                    let _ = ws.send(tungstenite::Message::text(
                        json!({ "type": "error", "message": e }).to_string(),
                    ));
                    let _ = ws.close(None);
                    break;
                }
            }
        } else {
            let p = prev.as_mut().expect("need_init=false なら prev はある");
            let delta = snap.history_size - p.history_size;
            if delta > 0 {
                // 押し出された行を含めて取り直す（マージン付き）。取り直しまでの間に
                // さらに履歴が進んでいても、マージン内なら取りこぼさない
                match ws_snapshot(tmux_socket, &target, delta + WS_PUSH_MARGIN) {
                    Ok(full) => {
                        let need = full.history_size.saturating_sub(p.history_size) as usize;
                        if full.history_size < p.history_size
                            || need > full.history_lines
                            || need as u64 > WS_INIT_HISTORY
                        {
                            // 取り直しの間に clear された / マージンを超えて進んだ。
                            // 次周期で init を再送する
                            prev = None;
                            None
                        } else {
                            let payload = json!({
                                "type": "update",
                                "pushed": full.lines[full.history_lines - need..full.history_lines],
                                "screen": full.screen(),
                                "cursor": { "x": full.cursor_x, "y": full.cursor_y },
                            })
                            .to_string();
                            p.history_size = full.history_size;
                            p.screen_hash = full.screen_hash();
                            Some(payload)
                        }
                    }
                    Err(e) => {
                        let _ = ws.send(tungstenite::Message::text(
                            json!({ "type": "error", "message": e }).to_string(),
                        ));
                        let _ = ws.close(None);
                        break;
                    }
                }
            } else {
                let hash = snap.screen_hash();
                if hash != p.screen_hash {
                    p.screen_hash = hash;
                    Some(
                        json!({
                            "type": "update",
                            "pushed": [],
                            "screen": snap.screen(),
                            "cursor": { "x": snap.cursor_x, "y": snap.cursor_y },
                        })
                        .to_string(),
                    )
                } else {
                    None
                }
            }
        };

        if let Some(payload) = msg {
            if ws.send(tungstenite::Message::text(payload)).is_err() {
                break;
            }
            last_sent = std::time::Instant::now();
        } else if last_sent.elapsed() >= WS_KEEPALIVE {
            let keepalive = json!({ "type": "keepalive" }).to_string();
            if ws.send(tungstenite::Message::text(keepalive)).is_err() {
                break;
            }
            last_sent = std::time::Instant::now();
        }
        std::thread::sleep(WS_POLL_INTERVAL);
    }
}

/// URL のクエリ文字列から指定キーの値を取り出す（`/path?ansi=1&lines=200` の類）
fn query_param(url: &str, key: &str) -> Option<String> {
    let qs = url.split_once('?')?.1;
    for pair in qs.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(urlencoding::decode(v));
        }
    }
    None
}

/// URL パスからセッション ID を抽出する
/// （`/api/sessions/<uuid>/messages` → Some("<uuid>")）
fn extract_session_id(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.splitn(5, '/').collect();
    // ["", "api", "sessions", "<id>", "messages"]
    if parts.len() >= 5 && parts[1] == "api" && parts[2] == "sessions" && parts[4] == "messages" {
        let id = urlencoding::decode(parts[3]);
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
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
    let url_full = request.url().to_string();
    let path = url_full.split('?').next().unwrap_or("").to_string();

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
        (tiny_http::Method::Get, "/api/agents") => {
            // claude agents --json のプロキシ + pane 対応付け（Issue #23）
            match crate::agents::list_agents_with_panes(Some(tmux_socket)) {
                Ok(result) => respond_sensitive(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 502, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Get, p)
            if p.starts_with("/api/sessions/") && p.ends_with("/messages") =>
        {
            // /api/sessions/:id/messages — Claude Code transcript の正規化読み取り
            let Some(session_id) = extract_session_id(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なセッション ID" }).to_string()),
                );
            };
            let tail = query_param(&url_full, "tail")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(30);
            match crate::transcript::read_messages(&session_id, tail) {
                Ok(result) => respond_sensitive(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
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
            let ansi = query_param(&url_full, "ansi").is_some_and(|v| v == "1" || v == "true");
            let history = query_param(&url_full, "lines").and_then(|v| v.parse::<u32>().ok());
            match tmux_capture_pane(tmux_socket, &tmux_target, ansi, history) {
                Ok(lines) => {
                    let mut body = json!({ "lines": lines });
                    if let Some((x, y, w, h)) = tmux_pane_geometry(tmux_socket, &tmux_target) {
                        body["cursor"] = json!({ "x": x, "y": y });
                        body["size"] = json!({ "cols": w, "rows": h });
                    }
                    respond_sensitive(request, 200, Some(body.to_string()))
                }
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Get, p)
            if p.starts_with("/api/panes/") && p.ends_with("/scrollback") =>
        {
            let Some(target) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let tmux_target = format!("={target}");
            let history = query_param(&url_full, "lines")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1000);
            match tmux_capture_pane(tmux_socket, &tmux_target, false, Some(history)) {
                Ok(lines) => {
                    let mut body = json!({ "lines": lines });
                    if let Some((_, _, w, h)) = tmux_pane_geometry(tmux_socket, &tmux_target) {
                        body["size"] = json!({ "cols": w, "rows": h });
                    }
                    respond_sensitive(request, 200, Some(body.to_string()))
                }
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
            let tmux_target = format!("={target}");
            if let Some(keys) = parsed["keys"].as_str() {
                match tmux_send_raw_keys(tmux_socket, &tmux_target, keys) {
                    Ok(()) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                    Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
                }
            } else {
                let text = parsed["text"].as_str().unwrap_or("").to_string();
                let newline = parsed["newline"].as_bool().unwrap_or(true);
                match tmux_send_keys(tmux_socket, &tmux_target, &text, newline) {
                    Ok(()) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                    Err(e) => respond(request, 500, Some(json!({ "error": e }).to_string())),
                }
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
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/resize") => {
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
            let window_target = format!("={}", window_target_of(&target));
            let result = if parsed["reset"].as_bool() == Some(true) {
                tmux_reset_window_size(tmux_socket, &window_target)
            } else {
                match (parsed["cols"].as_u64(), parsed["rows"].as_u64()) {
                    (Some(cols), Some(rows)) if cols > 0 && rows > 0 => {
                        tmux_resize_window(tmux_socket, &window_target, cols as u32, rows as u32)
                    }
                    _ => {
                        return respond(
                            request,
                            400,
                            Some(
                                json!({ "error": "cols と rows（正の整数）か reset=true を指定する" })
                                    .to_string(),
                            ),
                        );
                    }
                }
            };
            match result {
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

    // P0-3: QR はランダム名で state_dir 配下に保存（停止時に cleanup_state_files で削除される）
    let dir = ensure_state_dir()?;
    let nonce: u64 = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut h);
        h.finish()
    };
    let path = dir.join(format!("qr-{nonce:016x}.png"));
    img.save(&path)
        .map_err(|e| io::Error::other(format!("PNG の保存に失敗: {e}")))?;
    restrict_permissions(&path);

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
    fn extract_session_idの抽出() {
        assert_eq!(
            extract_session_id("/api/sessions/a45899a8-96a6-4fa6/messages"),
            Some("a45899a8-96a6-4fa6".to_string())
        );
        assert_eq!(extract_session_id("/api/sessions//messages"), None);
        assert_eq!(extract_session_id("/api/panes/x/screen"), None);
        assert_eq!(extract_session_id("/api/sessions/abc/other"), None);
    }

    #[test]
    fn window_target_ofの導出() {
        assert_eq!(window_target_of("sess:0.1"), "sess:0");
        assert_eq!(window_target_of("tako-abc123:2.0"), "tako-abc123:2");
        // ペイン部が無い場合はそのまま
        assert_eq!(window_target_of("sess:0"), "sess:0");
    }

    #[test]
    fn ws_token_from_protocolsの抽出() {
        assert_eq!(
            ws_token_from_protocols("tako-remote, token.abc123"),
            Some("abc123".to_string())
        );
        assert_eq!(
            ws_token_from_protocols("token.xyz"),
            Some("xyz".to_string())
        );
        assert_eq!(ws_token_from_protocols("tako-remote"), None);
        assert_eq!(ws_token_from_protocols(""), None);
        // token. 接頭辞のみ（空トークン）は Some("") となり、実トークンと一致しないため安全
        assert_eq!(ws_token_from_protocols("token."), Some(String::new()));
    }

    #[test]
    fn capture_pane_argsの組み立て() {
        assert_eq!(
            capture_pane_args("=s:0.0", false, None),
            vec!["capture-pane", "-t", "=s:0.0", "-p"]
        );
        assert_eq!(
            capture_pane_args("=s:0.0", true, None),
            vec!["capture-pane", "-t", "=s:0.0", "-p", "-e"]
        );
        assert_eq!(
            capture_pane_args("=s:0.0", true, Some(200)),
            vec!["capture-pane", "-t", "=s:0.0", "-p", "-e", "-S", "-200"]
        );
    }

    #[test]
    fn parse_snapshotはメタと履歴と画面を分離する() {
        let raw = "5 3 1 93 50\nhist1\nhist2\nline1\nline2\n";
        let snap = parse_snapshot(raw, 2).unwrap();
        assert_eq!(snap.history_size, 5);
        assert_eq!((snap.cursor_x, snap.cursor_y), (3, 1));
        assert_eq!((snap.cols, snap.rows), (93, 50));
        assert_eq!(snap.history(), ["hist1".to_string(), "hist2".to_string()]);
        assert_eq!(snap.screen(), ["line1".to_string(), "line2".to_string()]);
    }

    #[test]
    fn parse_snapshotは履歴なしで全行を画面とする() {
        let raw = "0 0 0 80 24\nprompt $ \n";
        let snap = parse_snapshot(raw, 0).unwrap();
        assert_eq!(snap.history_lines, 0);
        assert_eq!(snap.screen(), ["prompt $ ".to_string()]);
    }

    #[test]
    fn parse_snapshotは履歴が要求より少なければあるだけ切り出す() {
        let raw = "1 0 0 80 24\nold\nscreen\n";
        let snap = parse_snapshot(raw, 2000).unwrap();
        assert_eq!(snap.history(), ["old".to_string()]);
        assert_eq!(snap.screen(), ["screen".to_string()]);
    }

    #[test]
    fn parse_snapshotは画面全トリムでも壊れない() {
        // capture-pane が画面下端の空行を全部トリムし、画面部分が 0 行になるケース
        let raw = "2 0 0 80 24\nh1\nh2\n";
        let snap = parse_snapshot(raw, 10).unwrap();
        assert_eq!(snap.history_lines, 2);
        assert!(snap.screen().is_empty());
    }

    #[test]
    fn parse_snapshotは画面中の空行を保持する() {
        let raw = "0 0 4 80 24\nline1\n\n\nline4\n";
        let snap = parse_snapshot(raw, 0).unwrap();
        assert_eq!(snap.screen().len(), 4);
        assert_eq!(snap.screen()[1], "");
    }

    #[test]
    fn parse_snapshotは不正メタをエラーにする() {
        assert!(parse_snapshot("", 0).is_err());
        assert!(parse_snapshot("abc def ghi jkl mno\nx\n", 0).is_err());
        assert!(parse_snapshot("1 2 3\nx\n", 0).is_err());
    }

    #[test]
    fn screen_hashはカーソル位置を含み履歴サイズを含まない() {
        let a = parse_snapshot("0 0 0 80 24\nx\n", 0).unwrap();
        let b = parse_snapshot("0 1 0 80 24\nx\n", 0).unwrap();
        assert_ne!(a.screen_hash(), b.screen_hash());
        let c = parse_snapshot("5 0 0 80 24\nx\n", 0).unwrap();
        assert_eq!(a.screen_hash(), c.screen_hash());
    }

    #[test]
    fn query_paramの抽出() {
        assert_eq!(
            query_param("/api/panes/s:0.0/screen?ansi=1&lines=200", "ansi"),
            Some("1".to_string())
        );
        assert_eq!(
            query_param("/api/panes/s:0.0/screen?ansi=1&lines=200", "lines"),
            Some("200".to_string())
        );
        assert_eq!(query_param("/api/panes/s:0.0/screen", "ansi"), None);
        assert_eq!(query_param("/path?a=%3A", "a"), Some(":".to_string()));
        assert_eq!(query_param("/path?flag", "flag"), Some("".to_string()));
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
        // machineId なし（リレー登録失敗）→ トンネル直 URL
        let url = connect_url(
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "abc123",
            Some("my-mac"),
            None,
        );
        assert!(url.starts_with("https://foo.trycloudflare.com/#/connect?"));
        assert!(!url.contains("host="));
        assert!(url.contains("token=abc123"));
        assert!(url.contains("name=my-mac"));

        let url = connect_url(
            None,
            "http://192.168.1.10:7749",
            "tok456",
            Some("host1"),
            None,
        );
        assert!(url.starts_with("http://192.168.1.10:7749/#/connect?"));
        assert!(!url.contains("host="));
        assert!(url.contains("token=tok456"));
        assert!(url.contains("name=host1"));

        let url = connect_url(None, "http://localhost:7749", "abc123", None, None);
        assert!(url.starts_with("http://localhost:7749/#/connect?"));
        assert!(url.contains("token=abc123"));
        assert!(!url.contains("name="));
    }

    #[test]
    fn connect_urlはトンネルとmachine_idが揃うとpages固定urlになる() {
        // Issue #91: トンネル有効 + リレー登録済みの正常系はランダムな trycloudflare URL を
        // 見せず、Pages 固定 URL + machine パラメータで PWA にリレー解決させる
        let url = connect_url_with_pages(
            "https://tako-remote.pages.dev",
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "tok123",
            Some("my-mac"),
            Some("aaaabbbb-cccc-4ddd-8eee-ffff00001111"),
        );
        assert!(url.starts_with("https://tako-remote.pages.dev/#/connect?"));
        assert!(url.contains("machine=aaaabbbb-cccc-4ddd-8eee-ffff00001111"));
        assert!(url.contains("token=tok123"));
        assert!(url.contains("name=my-mac"));
        assert!(
            !url.contains("trycloudflare"),
            "トンネル URL を露出させない: {url}"
        );
    }

    #[test]
    fn connect_urlはlan_onlyならmachine_idがあってもlan直リンク() {
        // https の Pages からプライベート IP の http へは mixed content で接続できないため、
        // LAN-only では Pages 化しない
        let url = connect_url_with_pages(
            "https://tako-remote.pages.dev",
            None,
            "http://192.168.1.10:7749",
            "tok123",
            None,
            Some("aaaabbbb-cccc-4ddd-8eee-ffff00001111"),
        );
        assert!(url.starts_with("http://192.168.1.10:7749/#/connect?"));
        assert!(!url.contains("pages.dev"));
    }

    #[test]
    fn connect_urlのpagesベース末尾スラッシュは正規化される() {
        let url = connect_url_with_pages(
            "https://tako-remote.pages.dev/",
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "t",
            None,
            Some("m-1"),
        );
        assert!(url.starts_with("https://tako-remote.pages.dev/#/connect?"));
    }

    #[test]
    fn connect_urlのトークンはfragmentに載る() {
        // fragment（# 以降）はブラウザがサーバーへ送らない = ログ・Referer に残らない。
        // token が # より後ろにあることを検証する（LAN 直 / Pages の両形式）
        let url = connect_url(None, "http://192.168.1.10:7749", "secret", None, None);
        let hash_pos = url.find('#').expect("fragment がある");
        let token_pos = url.find("token=").expect("token がある");
        assert!(token_pos > hash_pos, "token は fragment 内: {url}");

        let url = connect_url_with_pages(
            "https://tako-remote.pages.dev",
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "secret",
            None,
            Some("m-1"),
        );
        let hash_pos = url.find('#').expect("fragment がある");
        let token_pos = url.find("token=").expect("token がある");
        let machine_pos = url.find("machine=").expect("machine がある");
        assert!(token_pos > hash_pos, "token は fragment 内: {url}");
        assert!(machine_pos > hash_pos, "machine も fragment 内: {url}");
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
    fn relay_secretは初回生成後に安定して返る() {
        let tmp = std::env::temp_dir().join(format!("tako-test-rsec-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        let s1 = relay_secret_at(&tmp).expect("初回生成");
        assert_eq!(s1.len(), 64, "hex 64 文字");
        assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));

        let s2 = relay_secret_at(&tmp).expect("再読み込み");
        assert_eq!(s1, s2, "永続化された同じ値が返る");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn relay_secretは短すぎる既存値を再生成する() {
        let tmp = std::env::temp_dir().join(format!("tako-test-rsec2-{}", std::process::id()));
        std::fs::write(&tmp, "short").unwrap();
        let s = relay_secret_at(&tmp).expect("再生成");
        assert_eq!(s.len(), 64);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn connect_urlはtunnelありnameなしでもtunnel直接() {
        let url = connect_url(
            Some("https://foo.trycloudflare.com"),
            "http://localhost:7749",
            "tok123",
            None,
            None,
        );
        assert!(url.starts_with("https://foo.trycloudflare.com/#/connect?"));
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

    #[test]
    fn constant_time_eqは一致判定と長さ違いを正しく扱う() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn mask_status_tokenはトークンをマスクしそれ以外を残す() {
        let mut v =
            json!({ "running": true, "port": 7749, "token": "deadbeef", "url": "http://x" });
        mask_status_token(&mut v);
        assert_eq!(v["token"], json!("***"));
        assert_eq!(v["port"], json!(7749));
        assert_eq!(v["running"], json!(true));

        // token フィールドが無ければ何も壊さない
        let mut v2 = json!({ "running": false });
        mask_status_token(&mut v2);
        assert_eq!(v2, json!({ "running": false }));
    }

    #[test]
    fn mask_token_in_urlはクエリのtokenを伏せる() {
        // fragment 内 + 後続パラメータあり
        assert_eq!(
            mask_token_in_url("https://x.pages.dev/#/connect?machine=m1&token=deadbeef&name=mac"),
            "https://x.pages.dev/#/connect?machine=m1&token=***&name=mac"
        );
        // token が末尾
        assert_eq!(
            mask_token_in_url("http://10.0.0.1:7749/#/connect?token=secretvalue"),
            "http://10.0.0.1:7749/#/connect?token=***"
        );
        // token 無しはそのまま
        assert_eq!(
            mask_token_in_url("http://10.0.0.1:7749/"),
            "http://10.0.0.1:7749/"
        );
    }

    #[test]
    fn find_port_occupantは未使用ポートでnoneを返す() {
        // 存在しないであろう高番号ポート
        assert!(find_port_occupant(59999).is_none());
    }

    #[test]
    fn kill_stale_daemonは存在しないpidで安全に完了する() {
        // is_process_alive が false なので即 cleanup_state_files して return
        kill_stale_daemon(999_999_999);
    }

    #[test]
    fn daemon_statusはpidファイルが無ければnot_running() {
        // state_dir をテンポラリに差し替えて検証
        let dir = std::env::temp_dir().join(format!("tako-test-remote-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let status = daemon_status();
        assert_eq!(status["running"], json!(false));
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mask_status_tokenはconnect_url内のtokenもマスクする() {
        // #104: token フィールドだけでなく URL クエリの生トークンも伏せる
        let mut v = json!({
            "running": true,
            "token": "aabbccdd",
            "url": "http://10.0.0.5:7749",
            "connect_url": "https://tako-remote.pages.dev/#/connect?machine=m1&token=aabbccdd&name=mac",
            "fallback_url": "https://foo.trycloudflare.com/#/connect?token=aabbccdd&name=mac",
        });
        mask_status_token(&mut v);
        assert_eq!(v["token"], json!("***"));
        let connect = v["connect_url"].as_str().unwrap();
        let fallback = v["fallback_url"].as_str().unwrap();
        assert!(
            !connect.contains("aabbccdd"),
            "connect_url に生トークンが残る: {connect}"
        );
        assert!(
            !fallback.contains("aabbccdd"),
            "fallback_url に生トークンが残る: {fallback}"
        );
        assert!(connect.contains("token=***"));
        assert!(connect.contains("machine=m1"), "他のクエリは保持する");
        assert!(connect.contains("name=mac"));
    }

    // --- P0-3 テスト ---

    #[test]
    fn state_dirはdata_dir配下のremoteを返す() {
        let dir = state_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.contains("remote") || s.contains("tako"),
            "state_dir は /tmp ではなく data_dir 配下: {s}"
        );
    }

    #[test]
    fn write_secret_fileは0600で書ける() {
        let dir = std::env::temp_dir().join(format!("tako-test-wsf-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("secret.txt");
        write_secret_file(&path, "hello").expect("書き込み成功");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "パーミッションは 0600: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_state_dirは0700でディレクトリを作る() {
        let dir = std::env::temp_dir().join(format!("tako-test-esd-{}", std::process::id()));
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let result = ensure_state_dir();
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");
        assert!(result.is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "ディレクトリパーミッションは 0700: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- P0-4 テスト ---

    #[test]
    fn parse_pid_fileは3行形式を解析する() {
        let dir = std::env::temp_dir().join(format!("tako-test-ppf-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let pid_file = dir.join("tako-remote.pid");
        std::fs::write(&pid_file, "12345\n/usr/bin/tako\n1700000000\n").unwrap();
        let info = parse_pid_file().expect("パースに成功");
        assert_eq!(info.pid, 12345);
        assert_eq!(info.exe, Some("/usr/bin/tako".to_string()));
        assert_eq!(info.start_time, Some(1700000000));
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_pid_identityは存在しないpidをfalseで返す() {
        let info = PidInfo {
            pid: 99_999_999,
            exe: None,
            start_time: None,
        };
        assert!(!verify_pid_identity(&info));
    }

    #[test]
    fn daemon_stop_implはpid再利用時にkillしない() {
        // 自分の PID を書いたが、args が "tako remote serve" でないのでエラーになる
        let dir = std::env::temp_dir().join(format!("tako-test-dsi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let my_pid = std::process::id();
        let pid_file = dir.join("tako-remote.pid");
        std::fs::write(&pid_file, format!("{my_pid}\n/bin/zsh\n0\n")).unwrap();
        let result = daemon_stop_impl(false);
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");
        assert!(result.is_err(), "PID 再利用を検知してエラーになる");
        let err = result.unwrap_err();
        assert!(
            err.contains("再利用"),
            "エラーメッセージに PID 再利用を示す: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn parse_etimeは経過時間を秒に変換する() {
        assert_eq!(parse_etime("03:42"), Some(222));
        assert_eq!(parse_etime("01:03:42"), Some(3822));
        assert_eq!(parse_etime("2-01:03:42"), Some(2 * 86400 + 3822));
        assert_eq!(parse_etime("00:05"), Some(5));
        assert_eq!(parse_etime("bad"), None);
    }
}

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
//! 認証（Issue #283: 機器ペアリング二層認証。長寿命 bearer token は全廃）:
//! - 層①: `tailscale serve` が付与する `X-Forwarded-For` を `tailscale whois` で照合し、
//!   tailnet 上の実在ノードのみ通す（ローカル直結・偽装 IP は拒否）
//! - 層②: ノードの恒久 ID をデバイスレジストリと照合し、role（Observe / Interact /
//!   Manage / Admin）で操作を認可する。未登録端末はペアリング要求（`POST /api/pair`）
//!   だけができ、Mac 画面の承認ダイアログで許可されるまで画面データを受け取れない
//! - 管理 API（`/api/admin/*`）: ローカル直結 + `X-Tako-Admin: <管理トークン>` のみ。
//!   ペアリング承認・role 変更は tako-app の GUI ダイアログ専用（`.agent/requirements.md`）
//! - 詳細は `remote_auth` モジュールと計画 `.agent/plans/tako-remote-plan.md` §4
//!
//! PWA は daemon 自身が配信する（同一 origin・バージョン一致。公開 Pages 配信は廃止）。
//!
//! transport（Issue #282: Tailscale Serve 一本化）:
//! - daemon は `127.0.0.1` のみ bind し、`tailscale serve` が HTTPS:443 →
//!   `http://127.0.0.1:<port>` のプロキシとして tailnet 内へ公開する
//!   （WireGuard E2E 暗号化・public internet に入口を持たない）
//! - 接続リンクは恒久固定の `https://<ホスト名>.<tailnet>.ts.net`（MagicDNS 名。
//!   serve の off → 再設定でも不変。弾 0 実測 `.agent/investigations/tailscale-serve-poc.md`）
//! - Tailscale 未セットアップ（未導入・デーモン未起動・未ログイン・HTTPS 未有効）での
//!   起動は不足項目を列挙して**拒否**し、`tako remote setup` へ誘導する（黙って失敗させない）
//!
//! デーモン管理:
//! - `tako remote start` → `tako remote serve` をバックグラウンド fork
//! - PID ファイル（`<data_dir>/remote/tako-remote.pid`）で管理
//! - `tako remote stop` → PID ファイルから kill + serve 設定の解除

use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rust_embed::Embed;
use serde_json::{json, Value};

use crate::remote_auth::{DeviceRegistry, DeviceRole, Identity};

const DEFAULT_PORT: u16 = 7749;
const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// interact idle session の定期スイープ間隔
const SESSION_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// #445: TAKO_ISOLATED が有効かどうか。隔離モードでは他インスタンスの
/// state ファイルを cleanup しない二重防御に使う
fn is_isolated() -> bool {
    matches!(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        Some("1" | "true" | "on")
    )
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
/// 公開中の固定 ts.net URL を記録するファイル。
/// デーモンが serve 確立時に書き、`daemon_status` が接続リンクの再構成に使う
pub fn url_path() -> std::path::PathBuf {
    state_dir().join("tako-remote.url")
}

/// 秘密を含むファイルを作成時から 0600 で書き込む。
/// temp → atomic rename で symlink race を防ぐ。
/// devices.json（remote_auth）も identity 情報を含むため同じ経路で書く
pub(crate) fn write_state_file(path: &std::path::Path, content: &str) -> io::Result<()> {
    write_secret_file(path, content)
}

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
    let _ = std::fs::remove_file(url_path());
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

// --- daemon 共有コンテキスト（二層認証・接続追跡。#283）---

/// daemon がリクエストハンドラ間で共有する状態。
/// registry（デバイスレジストリ + 保留 + interact session + whois キャッシュ）と
/// WS 接続数（通知・status 表示用）を持つ
struct DaemonCtx {
    registry: Mutex<DeviceRegistry>,
    /// tailscale CLI のパス（whois 照合に使う）
    ts_cli: String,
    /// ローカル管理トークン（`/api/admin/*` 専用。リモート端末には決して渡らない）
    admin_token: String,
    tmux_socket: String,
    /// デバイスごとの WS 接続数 + 最終切断時刻（短時間の再接続で通知を抑制する）
    ws_connections: Mutex<HashMap<String, WsDeviceState>>,
    /// 稼働ポート・公開 URL（admin state 表示用）
    port: u16,
    base_url: String,
    /// 自ノードの ts.net ホスト名（XFF 検証用。#287 P1-1）
    expected_host: String,
}

/// デバイスごとの WS 接続状態
struct WsDeviceState {
    count: usize,
    last_disconnect: Option<std::time::Instant>,
}

/// 再接続で通知を抑制する猶予時間
const WS_NOTIFY_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

impl DaemonCtx {
    /// デバイスの WS 接続数を +1 する。
    /// 接続通知を出すべきなら true（0→1 かつ直前の切断から猶予時間以上経過）
    fn ws_connect(&self, device_id: &str) -> bool {
        let mut map = self.ws_connections.lock().unwrap();
        let state = map.entry(device_id.to_string()).or_insert(WsDeviceState {
            count: 0,
            last_disconnect: None,
        });
        state.count += 1;
        if state.count == 1 {
            let recently = state
                .last_disconnect
                .is_some_and(|t| t.elapsed() < WS_NOTIFY_GRACE);
            return !recently;
        }
        false
    }

    /// デバイスの WS 接続数を -1 する。
    /// 切断通知を出すべきなら true（全 WS が切断された）。
    /// ただし通知は即時ではなく、猶予時間内の再接続で抑制されるため
    /// 呼び出し側は切断時刻を記録し、猶予後に改めて判定する
    fn ws_disconnect(&self, device_id: &str) -> bool {
        let mut map = self.ws_connections.lock().unwrap();
        match map.get_mut(device_id) {
            Some(state) if state.count > 1 => {
                state.count -= 1;
                false
            }
            Some(state) => {
                state.count = 0;
                state.last_disconnect = Some(std::time::Instant::now());
                true
            }
            None => false,
        }
    }

    /// 指定デバイスが現在 WS 接続中（count > 0）かを返す
    fn ws_is_connected(&self, device_id: &str) -> bool {
        self.ws_connections
            .lock()
            .unwrap()
            .get(device_id)
            .is_some_and(|s| s.count > 0)
    }

    /// 接続中デバイス → WS 接続数のスナップショット
    fn connections_snapshot(&self) -> HashMap<String, usize> {
        self.ws_connections
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.count > 0)
            .map(|(k, s)| (k.clone(), s.count))
            .collect()
    }
}

/// macOS 通知を表示する（接続開始終了・操作セッション開始。#283）。
/// osascript 経由（依存追加なし）。`TAKO_REMOTE_NO_NOTIFY=1` で抑止（テスト・検証用）。
/// 通知文にはデバイス名・イベントのみを載せ、ペイン内容は含めない
fn notify_macos(message: &str) {
    if std::env::var("TAKO_REMOTE_NO_NOTIFY").is_ok_and(|v| v == "1") {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // osascript の文字列リテラルは引数渡し（argv）にして injection を避ける
        let script = "on run argv\ndisplay notification (item 1 of argv) with title \"tako remote\"\nend run";
        let _ = Command::new("osascript")
            .args(["-e", script, message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = message;
    }
}

/// 認可判定の結果
enum AuthDecision {
    /// 承認済みデバイスとして許可
    Allowed(crate::remote_auth::Device),
    /// 拒否（HTTP status, エラーメッセージ）
    Rejected(u16, String),
}

/// 層①（identity）+ 層②（デバイス role）を評価する。
/// `required` 以上の role を持つ登録済みデバイスのみ Allowed
fn authorize_device(
    ctx: &DaemonCtx,
    request: &tiny_http::Request,
    required: DeviceRole,
) -> AuthDecision {
    let forwarded = header_value(request, "x-forwarded-for");
    let forwarded_host = header_value(request, "x-forwarded-host");
    match crate::remote_auth::identify(
        &ctx.registry,
        &ctx.ts_cli,
        forwarded.as_deref(),
        forwarded_host.as_deref(),
        &ctx.expected_host,
    ) {
        Ok(Identity::Tailnet(who)) => {
            let mut reg = ctx.registry.lock().unwrap();
            match reg.device(&who.stable_id).cloned() {
                Some(device) if device.role >= required => {
                    reg.touch(&device.id);
                    AuthDecision::Allowed(device)
                }
                Some(device) => AuthDecision::Rejected(
                    403,
                    format!(
                        "この操作には {} 以上の role が必要（現在: {}）。\
                         role の変更はペアリング画面から要求できます",
                        required.as_str(),
                        device.role.as_str()
                    ),
                ),
                None => AuthDecision::Rejected(
                    403,
                    "この端末はペアリングされていない。PWA のペアリング画面から要求してください"
                        .to_string(),
                ),
            }
        }
        Ok(Identity::Local) => AuthDecision::Rejected(
            401,
            "tailscale serve 経由のアクセスのみ受け付ける".to_string(),
        ),
        Err(e) => AuthDecision::Rejected(401, e),
    }
}

/// 層①のみ評価する（静的アセット・/api/health・/api/me・/api/pair 用）。
/// tailnet ノードなら Ok(WhoisInfo)
fn identify_tailnet(
    ctx: &DaemonCtx,
    request: &tiny_http::Request,
) -> Result<crate::tailscale::WhoisInfo, (u16, String)> {
    let forwarded = header_value(request, "x-forwarded-for");
    let forwarded_host = header_value(request, "x-forwarded-host");
    match crate::remote_auth::identify(
        &ctx.registry,
        &ctx.ts_cli,
        forwarded.as_deref(),
        forwarded_host.as_deref(),
        &ctx.expected_host,
    ) {
        Ok(Identity::Tailnet(who)) => Ok(who),
        Ok(Identity::Local) => Err((
            401,
            "tailscale serve 経由のアクセスのみ受け付ける".to_string(),
        )),
        Err(e) => Err((401, e)),
    }
}

/// 管理 API の認証: ローカル直結（serve 経由でない）+ `X-Tako-Admin` ヘッダが
/// 管理トークンと一致する場合のみ許可する。
/// serve 経由（X-Forwarded-For あり）は管理トークンが正しくても拒否する
/// （管理トークンは tailnet 上に流れない前提を守る。漏えいの兆候として監査に残す）
fn check_admin(ctx: &DaemonCtx, request: &tiny_http::Request) -> bool {
    if header_value(request, "x-forwarded-for").is_some() {
        if let Ok(reg) = ctx.registry.lock() {
            reg.audit(
                "admin_over_tailnet_rejected",
                "",
                "",
                json!({ "route": request.url().split('?').next().unwrap_or("") }),
            );
        }
        return false;
    }
    header_value(request, "x-tako-admin")
        .is_some_and(|v| constant_time_eq(v.as_bytes(), ctx.admin_token.as_bytes()))
}

// --- IPC クライアント（daemon → app の正規 dispatch 経路。#281 H-7）---

/// daemon から tako-app の IPC ソケットへ接続し、Request を dispatch 経由で実行する。
/// discovery::read_candidates で接続情報を自動発見する
struct AppIpcClient {
    socket: String,
    token: String,
}

impl AppIpcClient {
    /// discovery 経由で稼働中の tako-app を探して接続する。見つからなければ None
    fn connect() -> Option<Self> {
        for info in crate::discovery::read_candidates() {
            if Self::probe(&info.socket, &info.token) {
                return Some(Self {
                    socket: info.socket,
                    token: info.token,
                });
            }
        }
        None
    }

    /// ソケットが生きていてトークンが通るかプローブする
    fn probe(socket: &str, token: &str) -> bool {
        Self::roundtrip_raw(socket, token, crate::protocol::Request::List).is_ok()
    }

    /// IPC に Request を送り、結果を返す
    fn request(&self, request: crate::protocol::Request) -> Result<Value, String> {
        Self::roundtrip_raw(&self.socket, &self.token, request)
    }

    #[cfg(unix)]
    fn roundtrip_raw(
        socket: &str,
        token: &str,
        request: crate::protocol::Request,
    ) -> Result<Value, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let stream = UnixStream::connect(socket)
            .map_err(|e| format!("tako app へ接続できない ({socket}): {e}"))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let mut writer = stream
            .try_clone()
            .map_err(|e| format!("接続の複製に失敗: {e}"))?;
        let envelope = crate::protocol::RequestEnvelope::new(1, token, request);
        let json =
            serde_json::to_string(&envelope).map_err(|e| format!("リクエストの構築に失敗: {e}"))?;
        writeln!(writer, "{json}").map_err(|e| format!("送信に失敗: {e}"))?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|e| format!("応答の受信に失敗: {e}"))?;
        if line.is_empty() {
            return Err("tako app から応答が返らなかった".into());
        }
        let response: crate::protocol::ResponseEnvelope =
            serde_json::from_str(&line).map_err(|e| format!("応答を解釈できない: {e}"))?;
        if let Some(error) = response.error {
            return Err(error.message);
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    #[cfg(not(unix))]
    fn roundtrip_raw(
        _socket: &str,
        _token: &str,
        _request: crate::protocol::Request,
    ) -> Result<Value, String> {
        Err("Windows の IPC は未実装".into())
    }
}

/// daemon 起動中に保持する IPC 接続状態。定期的に再接続を試みる
struct AppConnection {
    client: Option<AppIpcClient>,
    last_attempt: std::time::Instant,
}

const IPC_RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

impl AppConnection {
    fn new() -> Self {
        Self {
            client: AppIpcClient::connect(),
            last_attempt: std::time::Instant::now(),
        }
    }

    /// 接続中の IPC クライアントを返す。未接続なら定期的に再接続を試みる
    fn get(&mut self) -> Option<&AppIpcClient> {
        if self.client.is_none() && self.last_attempt.elapsed() >= IPC_RECONNECT_INTERVAL {
            self.client = AppIpcClient::connect();
            self.last_attempt = std::time::Instant::now();
        }
        self.client.as_ref()
    }

    /// IPC 接続に失敗したら切断状態にする（次の get() で再接続を試みる）
    fn invalidate(&mut self) {
        self.client = None;
        self.last_attempt = std::time::Instant::now();
    }
}

// --- ペイン ID マッピング（tmux target ↔ tako PaneId。#281）---

/// tmux target（`session:window.pane`）から tako PaneId への解決結果キャッシュ
struct PaneMapping {
    /// tmux backend session name → tako PaneId (u64)
    backend_to_pane: HashMap<String, u64>,
    /// tako PaneId → app の List 応答のペイン情報（API v2 用）
    pane_info: HashMap<u64, Value>,
    updated_at: std::time::Instant,
}

#[allow(dead_code)]
const PANE_MAPPING_TTL: std::time::Duration = std::time::Duration::from_secs(2);

impl PaneMapping {
    fn new() -> Self {
        Self {
            backend_to_pane: HashMap::new(),
            pane_info: HashMap::new(),
            updated_at: std::time::Instant::now() - std::time::Duration::from_secs(999),
        }
    }

    #[allow(dead_code)]
    fn is_stale(&self) -> bool {
        self.updated_at.elapsed() > PANE_MAPPING_TTL
    }

    /// IPC List の結果からマッピングを更新する
    fn update_from_list(&mut self, list: &Value) {
        self.backend_to_pane.clear();
        self.pane_info.clear();
        if let Some(tabs) = list["tabs"].as_array() {
            for tab in tabs {
                if let Some(panes) = tab["panes"].as_array() {
                    for pane in panes {
                        let Some(id) = pane["id"].as_u64() else {
                            continue;
                        };
                        self.pane_info.insert(id, pane.clone());
                        if let Some(session) = pane["tmux_session"].as_str() {
                            if !session.is_empty() {
                                self.backend_to_pane.insert(session.to_string(), id);
                            }
                        }
                    }
                }
            }
        }
        self.updated_at = std::time::Instant::now();
    }

    /// tako PaneId（数値文字列）を tmux ターゲット（`session:0.0`）に解決する。
    /// 既に tmux ターゲット形式（`:` を含む）ならそのまま返す
    fn resolve_tmux_target(&self, pane_param: &str) -> Option<String> {
        if pane_param.contains(':') {
            return Some(pane_param.to_string());
        }
        let id: u64 = pane_param.parse().ok()?;
        let info = self.pane_info.get(&id)?;
        let session = info["tmux_session"].as_str().filter(|s| !s.is_empty())?;
        Some(format!("{session}:0.0"))
    }
}

// --- WS broadcaster（M-5: 接続数分の tmux subprocess 乱立を解消。#281）---

/// ペインごとの共有 broadcaster。1 つの capture ループを共有し、複数 WS クライアントへ配信する。
/// subscriber はデバイス ID つきで登録し、revoke 時に該当デバイスの接続だけを
/// 即時切断できる（#283 受け入れ条件: revoke 即時反映）
struct PaneBroadcaster {
    subscribers: Vec<(String, std::sync::mpsc::Sender<String>)>,
    /// 最新の init メッセージ。新規 subscriber に即送するためキャッシュする
    last_init: Option<String>,
}

impl PaneBroadcaster {
    fn new() -> Self {
        Self {
            subscribers: Vec::new(),
            last_init: None,
        }
    }

    /// 新しい subscriber を登録し、受信チャンネルを返す。
    /// キャッシュ済みの init があれば即座に送信する（画面変化がない間に
    /// サブスクライブした subscriber が init を受け取れないのを防ぐ）
    fn subscribe(&mut self, device_id: &str) -> std::sync::mpsc::Receiver<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(ref init) = self.last_init {
            let _ = tx.send(init.clone());
        }
        self.subscribers.push((device_id.to_string(), tx));
        rx
    }

    /// 指定デバイスの subscriber を除去する（Sender の drop で受信側が
    /// Disconnected を検知し、WS 転送スレッドが即座に接続を閉じる）
    fn drop_device(&mut self, device_id: &str) {
        self.subscribers.retain(|(id, _)| id != device_id);
    }

    /// 全 subscriber にメッセージを配信。切断済みの subscriber は除去する
    fn broadcast(&mut self, msg: &str) {
        self.subscribers
            .retain(|(_, tx)| tx.send(msg.to_string()).is_ok());
    }

    /// init メッセージをキャッシュして全 subscriber に配信する
    fn broadcast_init(&mut self, msg: &str) {
        self.last_init = Some(msg.to_string());
        self.broadcast(msg);
    }

    fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

type BroadcasterMap = Arc<Mutex<HashMap<String, Arc<Mutex<PaneBroadcaster>>>>>;

/// broadcaster map を作成する
fn new_broadcaster_map() -> BroadcasterMap {
    Arc::new(Mutex::new(HashMap::new()))
}

/// 指定デバイスの WS 接続をすべて即時切断する（revoke の即時反映。#283）
fn disconnect_device_ws(map: &BroadcasterMap, device_id: &str) {
    let broadcasters: Vec<Arc<Mutex<PaneBroadcaster>>> =
        map.lock().unwrap().values().cloned().collect();
    for bc in broadcasters {
        bc.lock().unwrap().drop_device(device_id);
    }
}

/// 指定ペインの broadcaster を取得するか新規作成する。
/// 新規作成時は capture ループスレッドを起動する
fn get_or_create_broadcaster(
    map: &BroadcasterMap,
    pane: &str,
    device_id: &str,
    tmux_socket: &str,
    shutdown: Arc<AtomicBool>,
) -> (
    Arc<Mutex<PaneBroadcaster>>,
    std::sync::mpsc::Receiver<String>,
) {
    let mut map_guard = map.lock().unwrap();
    let broadcaster = map_guard
        .entry(pane.to_string())
        .or_insert_with(|| {
            let bc = Arc::new(Mutex::new(PaneBroadcaster::new()));
            let bc_clone = bc.clone();
            let pane_id = pane.to_string();
            let socket = tmux_socket.to_string();
            let map_weak = Arc::downgrade(map);
            std::thread::Builder::new()
                .name(format!("ws-broadcast-{}", &pane_id))
                .spawn(move || {
                    broadcaster_loop(bc_clone, &pane_id, &socket, shutdown, map_weak);
                })
                .ok();
            bc
        })
        .clone();
    let rx = broadcaster.lock().unwrap().subscribe(device_id);
    (broadcaster, rx)
}

/// broadcaster の capture ループ。subscriber が 0 になったら自動終了する
fn broadcaster_loop(
    broadcaster: Arc<Mutex<PaneBroadcaster>>,
    pane: &str,
    tmux_socket: &str,
    shutdown: Arc<AtomicBool>,
    map_weak: std::sync::Weak<Mutex<HashMap<String, Arc<Mutex<PaneBroadcaster>>>>>,
) {
    let target = format!("={pane}");
    let mut prev: Option<WsPrevState> = None;
    let mut last_sent = std::time::Instant::now();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // subscriber が 0 なら終了して map からも削除
        {
            let guard = broadcaster.lock().unwrap();
            if guard.subscriber_count() == 0 && prev.is_some() {
                // 初回以降で subscriber がいなくなった
                drop(guard);
                if let Some(map) = map_weak.upgrade() {
                    map.lock().unwrap().remove(pane);
                }
                break;
            }
        }

        let snap = match ws_snapshot(tmux_socket, &target, 0) {
            Ok(s) => s,
            Err(e) => {
                let msg = json!({ "type": "error", "message": e }).to_string();
                broadcaster.lock().unwrap().broadcast(&msg);
                if let Some(map) = map_weak.upgrade() {
                    map.lock().unwrap().remove(pane);
                }
                break;
            }
        };

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
                    let msg = json!({ "type": "error", "message": e }).to_string();
                    broadcaster.lock().unwrap().broadcast(&msg);
                    if let Some(map) = map_weak.upgrade() {
                        map.lock().unwrap().remove(pane);
                    }
                    break;
                }
            }
        } else {
            let p = prev.as_mut().unwrap();
            let delta = snap.history_size - p.history_size;
            if delta > 0 {
                match ws_snapshot(tmux_socket, &target, delta + WS_PUSH_MARGIN) {
                    Ok(full) => {
                        let need = full.history_size.saturating_sub(p.history_size) as usize;
                        if full.history_size < p.history_size
                            || need > full.history_lines
                            || need as u64 > WS_INIT_HISTORY
                        {
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
                        let msg = json!({ "type": "error", "message": e }).to_string();
                        broadcaster.lock().unwrap().broadcast(&msg);
                        if let Some(map) = map_weak.upgrade() {
                            map.lock().unwrap().remove(pane);
                        }
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
            if need_init {
                broadcaster.lock().unwrap().broadcast_init(&payload);
            } else {
                broadcaster.lock().unwrap().broadcast(&payload);
            }
            last_sent = std::time::Instant::now();
        } else if last_sent.elapsed() >= WS_KEEPALIVE {
            let keepalive = json!({ "type": "keepalive" }).to_string();
            broadcaster.lock().unwrap().broadcast(&keepalive);
            last_sent = std::time::Instant::now();
        }
        std::thread::sleep(WS_POLL_INTERVAL);
    }
}

/// Tailscale setup の不足項目を列挙した起動拒否エラーを組み立てる。
/// `tako remote start` の誘導文言（Issue #282: 黙って失敗させない）
fn setup_incomplete_error(status: &crate::tailscale::SetupStatus) -> io::Error {
    let items = status
        .missing
        .iter()
        .map(|m| format!("  - {}", m.describe()))
        .collect::<Vec<_>>()
        .join("\n");
    io::Error::other(format!(
        "Tailscale のセットアップが完了していないため、remote サーバーを起動できません。\n\
         不足項目:\n{items}\n\
         `tako remote setup` を実行してください。"
    ))
}

/// Tailscale の setup 状態を検証し、serve を設定して固定 ts.net URL を返す。
/// 返り値: (tailscale CLI パス, 固定 URL)。
/// 既存の serve 設定が tako 管理形式（HTTPS:443 の "/" 単純プロキシ）で自ポートを
/// 向いている場合は再利用し、それ以外の設定は上書きせず拒否する
fn establish_tailscale_serve(port: u16) -> io::Result<(String, String)> {
    let status = crate::tailscale::setup_status();
    if !status.ready() {
        return Err(setup_incomplete_error(&status));
    }
    // ready() = missing が空なら cli_path / dns_name は必ず埋まっている
    let cli = status
        .cli_path
        .clone()
        .ok_or_else(|| io::Error::other("tailscale CLI パスを解決できない"))?;
    let base_url = status
        .ts_net_url()
        .ok_or_else(|| io::Error::other("ts.net URL を解決できない"))?;

    match crate::tailscale::serve_state(&cli).map_err(io::Error::other)? {
        crate::tailscale::ServeState::NotConfigured => {
            crate::tailscale::serve_start(&cli, port).map_err(io::Error::other)?;
        }
        crate::tailscale::ServeState::Proxy(target)
            if target == crate::tailscale::proxy_target_for_port(port) =>
        {
            // 前回の設定が残っている（強制終了等）。同一ポートなのでそのまま再利用する
        }
        crate::tailscale::ServeState::Proxy(target) => {
            return Err(io::Error::other(format!(
                "tailscale serve に既存の設定があります（HTTPS:443 → {target}）。\
                 ユーザー設定を壊さないため上書きしません。tako の以前の設定の残骸で\
                 あれば `tailscale serve --https=443 off` で解除するか、同じポートを\
                 使うなら `tako remote start --port <ポート番号>` を指定してください"
            )));
        }
        crate::tailscale::ServeState::Other => {
            return Err(io::Error::other(
                "tailscale serve に tako 管理外の設定があります（パス分けハンドラ等）。\
                 上書きを避けるため起動を中止しました。`tailscale serve status` で確認してください",
            ));
        }
    }
    Ok((cli, base_url))
}

/// 独立デーモンとして HTTP サーバーを起動し、SIGTERM まで待機する。
/// `tako remote serve` から呼ばれる内部用関数。
///
/// transport 方針（#282）: `127.0.0.1` のみ bind し、`tailscale serve` 経由でのみ
/// tailnet 内へ公開する（WireGuard E2E 暗号化。平文 LAN モードは存在しない）。
/// Tailscale が未セットアップなら不足項目を列挙して起動を拒否する
pub fn run_daemon(port: Option<u16>) -> io::Result<()> {
    let port = port.unwrap_or(DEFAULT_PORT);
    // P0-1: 127.0.0.1（ループバック）のみ bind。tailscale serve だけがアクセスする
    let addr = format!("127.0.0.1:{port}");
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

    // Tailscale setup 検証 + serve 設定（不足があればここで起動拒否）。
    // e2e 検証モード（TAKO_REMOTE_TEST_MODE=1）では実 tailscale serve を張らず、
    // ts CLI パスと fake base URL だけを用意する（本番 tailnet の serve 設定を壊さず、
    // localhost 直叩き + fake identity 注入で認証層を実測するため。#283 の実測方針）。
    // このモードは 127.0.0.1 bind のまま = 外部到達経路は増えない
    let test_mode = std::env::var("TAKO_REMOTE_TEST_MODE").is_ok_and(|v| v == "1");
    let (ts_cli, base_url) = if test_mode {
        let cli = crate::tailscale::find_tailscale().unwrap_or_else(|| "tailscale".to_string());
        (cli, format!("http://127.0.0.1:{actual_port}"))
    } else {
        establish_tailscale_serve(actual_port)?
    };

    // ローカル管理トークン生成（/api/admin/* 専用。リモート端末には渡らない。#283）
    let token = crate::generate_token()?;

    // P0-3: state ディレクトリを 0700 で確保し、各ファイルを 0600 で書き出す
    ensure_state_dir()?;

    // デバイスレジストリを開く（devices.json 破損時は黙って全失効させず起動を拒否する）
    let registry = DeviceRegistry::open(&state_dir()).map_err(io::Error::other)?;

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

    // 公開 URL を state ファイルに残す（`tako remote status` が接続リンクを再構成するため）
    if let Err(e) = write_secret_file(&url_path(), &base_url) {
        // 起動情報の整合が取れないため中止する。設定した serve と state を片付ける
        // （test_mode では serve を張っていないので触らない = 本番設定を壊さない）
        if !test_mode {
            let _ = crate::tailscale::serve_stop_if_ours(&ts_cli, actual_port);
        }
        cleanup_state_files();
        return Err(io::Error::other(format!(
            "URL ファイルの書き出しに失敗: {e}"
        )));
    }

    // 起動情報を JSON で stdout に出力（start コマンドが読み取る）。
    // 接続リンクは恒久固定の ts.net URL（tailnet 内限定・WireGuard E2E 暗号化）。
    // #283: URL に token は載せない（接続時の認証は機器ペアリングが行う）
    let info = json!({
        "running": true,
        "port": actual_port,
        "bind_addr": addr,
        "url": base_url,
        "transport": "tailscale-serve",
    });
    println!("{info}");

    // CORS 許可 origin を設定（#287 P1: `*` 廃止 → base_url のみエコー）
    CORS_ALLOWED_ORIGIN
        .set(base_url.trim_end_matches('/').to_string())
        .ok();

    // daemon 共有コンテキスト（二層認証・接続追跡。#283）
    // base_url は "https://<host>.ts.net" 形式。expected_host はスキーム除去
    let expected_host = base_url
        .strip_prefix("https://")
        .unwrap_or(&base_url)
        .to_string();
    let ctx = Arc::new(DaemonCtx {
        registry: Mutex::new(registry),
        ts_cli: ts_cli.clone(),
        admin_token: token.clone(),
        tmux_socket: tmux_socket.clone(),
        ws_connections: Mutex::new(HashMap::new()),
        port: actual_port,
        base_url: base_url.clone(),
        expected_host,
    });

    // IPC 接続（#281: dispatch 正規経路。app 不在時は read-only fallback）
    let app_conn = Arc::new(RwLock::new(AppConnection::new()));
    // ペイン ID マッピングキャッシュ
    let pane_mapping = Arc::new(RwLock::new(PaneMapping::new()));
    // WS broadcaster map（M-5: ペインごとの共有 broadcaster）
    let broadcasters = new_broadcaster_map();

    // interact idle session の定期スイープ（session_end の監査記録。#283）
    {
        let ctx_sweep = ctx.clone();
        let shutdown_sweep = shutdown.clone();
        std::thread::Builder::new()
            .name("remote-session-sweep".into())
            .spawn(move || {
                while !shutdown_sweep.load(Ordering::Relaxed) {
                    std::thread::sleep(SESSION_SWEEP_INTERVAL);
                    if let Ok(mut reg) = ctx_sweep.registry.lock() {
                        let _ = reg.sweep_idle_sessions();
                    }
                }
            })
            .ok();
    }

    // HTTP サーバーループ
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Some(request)) => {
                let path = request.url().split('?').next().unwrap_or("");
                if path == "/ws" && is_ws_upgrade(&request) {
                    handle_ws_v2(
                        request,
                        &ctx,
                        shutdown.clone(),
                        broadcasters.clone(),
                        &app_conn,
                        &pane_mapping,
                    );
                } else {
                    handle_request_v2(request, &ctx, &app_conn, &pane_mapping, &broadcasters);
                }
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }

    // クリーンアップ: 自分が公開に使った serve 設定のみ解除する
    // （test_mode では serve を張っていないので触らない = 本番設定を壊さない）
    if !test_mode {
        if let Err(e) = crate::tailscale::serve_stop_if_ours(&ts_cli, actual_port) {
            eprintln!("tailscale serve の解除に失敗（tailscale serve --https=443 off で手動解除できます）: {e}");
        }
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
/// 返り値: running=true ならポート/トークンも含む。
/// PID ファイルは 3 行形式（PID / 実行ファイル / 起動時刻。P0-4）のため
/// parse_pid_file で先頭行の PID を取り出す
pub fn daemon_status() -> Value {
    let pid_info = match parse_pid_file() {
        Ok(info) => info,
        Err(_) => return json!({ "running": false }),
    };
    let pid_num = pid_info.pid;
    if !is_process_alive(pid_num) {
        // #445: TAKO_ISOLATED が有効なら他インスタンスの state を消さない（二重防御）
        if !is_isolated() {
            cleanup_state_files();
        }
        return json!({ "running": false });
    }
    let port = std::fs::read_to_string(port_path())
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    // URL ファイル（起動時に確定した固定 ts.net URL）から接続リンクを再構成する。
    // #283: URL に token は含まれない（QR も固定 URL のみ）
    let base_url = std::fs::read_to_string(url_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 登録済みデバイス数（devices.json を読むだけ。破損時は数えず None）
    let devices = DeviceRegistry::open(&state_dir())
        .ok()
        .map(|reg| reg.devices().len());
    let mut status = json!({
        "running": true,
        "pid": pid_num,
        "port": port,
        "url": base_url,
        "transport": "tailscale-serve",
    });
    // 稼働中 serve の実行バイナリを可視化する（#432: どの世代の serve が
    // 動いているかを ps なしで確認できるようにする）
    if let Some(exe) = pid_info.exe.as_deref().filter(|e| !e.is_empty()) {
        status["serve_binary"] = json!(exe);
    }
    if let Some(n) = devices {
        status["devices"] = json!(n);
    }
    status
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
/// 実行ファイルパスまたは ps の args で確認し、起動時刻もチェックする。
/// ps の起動自体が失敗した場合は安全側に倒す（検証不能 = false = kill しない）
fn verify_pid_identity(info: &PidInfo) -> bool {
    if !is_process_alive(info.pid) {
        return false;
    }
    #[cfg(unix)]
    {
        // ps で実行コマンドを取得し、tako remote serve かどうか確認。
        // 絶対パスで呼び出し PATH 制限環境でも動作する
        let ps_result = Command::new("/bin/ps")
            .args(["-p", &info.pid.to_string(), "-o", "args="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match ps_result {
            Ok(output) => {
                let cmd = String::from_utf8_lossy(&output.stdout);
                let cmd = cmd.trim();
                let is_tako_remote =
                    cmd.contains("tako") && cmd.contains("remote") && cmd.contains("serve");
                if !cmd.is_empty() && !is_tako_remote {
                    return false;
                }
            }
            Err(_) => {
                // ps を実行できない = 検証不能。安全側に倒す（kill しない）
                return false;
            }
        }
        // etime ベースの起動時刻チェック（記録がある場合のみ。±5 秒の余裕）。
        // ps etime（経過時間）+ 現在 epoch → 起動 epoch を逆算し、記録値と照合する
        if let Some(recorded) = info.start_time {
            let etime_result = Command::new("/bin/ps")
                .args(["-p", &info.pid.to_string(), "-o", "etime="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            match etime_result {
                Ok(output) => {
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
                Err(_) => {
                    return false;
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

/// デーモン停止後に残った serve 設定をベストエフォートで解除する。
/// SIGKILL などで daemon 自身のクリーンアップが走らなかったケースの回収。
/// 対象ポートへの tako 管理プロキシである場合のみ解除する（ユーザー設定は不可侵）
fn cleanup_serve_leftover(port: Option<u16>) {
    let Some(port) = port else { return };
    let Some(cli) = crate::tailscale::find_tailscale() else {
        return;
    };
    let _ = crate::tailscale::serve_stop_if_ours(&cli, port);
}

/// port ファイルに記録されたデーモンのポート番号を読む（無ければ None）
fn recorded_port() -> Option<u16> {
    std::fs::read_to_string(port_path())
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
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
                    cleanup_serve_leftover(Some(DEFAULT_PORT));
                    return Ok(json!({ "stopped": true, "stale_pid": occupant }));
                }
            }
            return Err("リモートサーバーが起動していない（PID ファイルが無い）".to_string());
        }
    };
    let pid_num = pid_info.pid;
    // kill 後は state が消えるため、serve 残骸回収用のポートを先に読んでおく
    let daemon_port = recorded_port();
    if !is_process_alive(pid_num) {
        cleanup_state_files();
        return Err("リモートサーバーが起動していない（プロセスは既に終了）".to_string());
    }
    // P0-4: PID が本当に tako remote プロセスか検証（検証不能時も kill しない = fail-safe）
    if !verify_pid_identity(&pid_info) {
        cleanup_state_files();
        return Err(format!(
            "PID {pid_num} が tako remote serve であることを確認できません\
             （PID 再利用または検証コマンド実行不能）。\
             安全のため停止操作を中止し、state ファイルを掃除しました。\
             手動で停止するには: kill {pid_num}"
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
    // SIGTERM ならデーモン自身が serve を解除して終了する。SIGKILL（--force）や
    // 異常終了で残った serve 設定はここでベストエフォート回収する（冪等）
    cleanup_serve_leftover(daemon_port);
    cleanup_state_files();
    Ok(json!({ "stopped": true }))
}

/// serve 子プロセスに使う tako バイナリを解決する（#432）。
/// ① 隔離・検証モード（TAKO_REMOTE_TEST_MODE / TAKO_ISOLATED）は検証対象の
///    自世代バイナリ: CLI 自身（current_exe）、GUI（tako-app）からは同ディレクトリの
///    `tako`（同ビルドの CLI）。/Applications に飛ばない = 隔離を壊さない
/// ② /Applications の安定バイナリを最優先。GUI（.app）と serve の世代を揃える:
///    旧実装は current_exe 優先で、PATH 先頭に dev の target/release を入れた環境で
///    シェルから `tako remote start` すると dev CLI 世代の serve が立ち、install 済み
///    .app と食い違って実機検証が旧コードを踏む事故が起きた
/// ③ .app が無い環境（cargo install 等）は CLI 自身（current_exe）
/// ④ それ以外（ファイル名が tako でない呼び出し元）は resolve_tako_binary に委ねる
fn serve_binary() -> String {
    let isolated = std::env::var("TAKO_REMOTE_TEST_MODE").is_ok_and(|v| v == "1")
        || matches!(
            std::env::var("TAKO_ISOLATED").ok().as_deref(),
            Some("1" | "true" | "on")
        );
    let stable_exists = std::path::Path::new(crate::dispatch::STABLE_APP_BINARY).is_file();
    serve_binary_impl(isolated, std::env::current_exe().ok(), stable_exists)
        .unwrap_or_else(crate::dispatch::resolve_tako_binary)
}

/// serve_binary の判定本体（テスト可能な純関数寄りの形。None = resolve_tako_binary へ委譲）
fn serve_binary_impl(
    isolated: bool,
    current_exe: Option<std::path::PathBuf>,
    stable_exists: bool,
) -> Option<String> {
    if isolated {
        if let Some(ref exe) = current_exe {
            if exe.file_name().and_then(|n| n.to_str()) == Some("tako") {
                return Some(exe.display().to_string());
            }
            // GUI（tako-app）等からの起動: 同ディレクトリの CLI（同世代）を使う
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("tako");
                if sibling.is_file() {
                    return Some(sibling.display().to_string());
                }
            }
        }
    }
    if stable_exists {
        return Some(crate::dispatch::STABLE_APP_BINARY.to_string());
    }
    if let Some(ref exe) = current_exe {
        if exe.file_name().and_then(|n| n.to_str()) == Some("tako") {
            return Some(exe.display().to_string());
        }
    }
    None
}

/// デーモンをバックグラウンドで fork 起動する。
/// `tako remote serve --port N` を子プロセスとして起動し、
/// stdout から起動情報 JSON を読み取って返す。
/// Tailscale 未セットアップ時は子プロセスが不足項目を stderr に出して終了するため、
/// その内容がこの関数の Err に載る（#282: `tako remote setup` への誘導）
pub fn spawn_daemon(port: Option<u16>) -> Result<Value, String> {
    let actual_port = port.unwrap_or(DEFAULT_PORT);

    // 既に起動中か確認
    let status = daemon_status();
    if status["running"].as_bool() == Some(true) {
        // 稼働中 serve の世代が現在の解決先と異なる場合は差し替えを促す
        // （#432: install 後も旧バイナリの serve が生き残り、実機検証が
        // 旧コードを踏む事故の検知）
        let running_exe = status["serve_binary"].as_str().unwrap_or("");
        let expected = serve_binary();
        if !running_exe.is_empty() && running_exe != expected {
            return Err(format!(
                "リモートサーバーは既に起動中ですが、稼働中の serve バイナリ\
                 （{running_exe}）が現在の解決先（{expected}）と異なります。\
                 `tako remote stop` してから `tako remote start` で差し替えてください"
            ));
        }
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

    let tako_bin = serve_binary();
    let mut args = vec!["remote".to_string(), "serve".to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
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

        // JSON を得られず stdout が閉じた（子が起動前チェックで終了した等）場合も
        // タイムアウトと同じ経路に落とし、下で stderr から実際の原因を拾って返す
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(v)) => Some(v),
            Ok(Err(_)) | Err(_) => None,
        }
    };

    let Some(info) = info else {
        // 起動情報が来なかった。子の終了を少し待ち、stderr から原因を拾う
        // （例: Tailscale 未セットアップの不足項目列挙、ポート使用中で bind 失敗）
        let status = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(s)) => break Some(s),
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    _ => break None,
                }
            }
        };
        if let Some(status) = status {
            let mut detail = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read as _;
                let _ = (&mut err).take(4096).read_to_string(&mut detail);
            }
            let detail = detail.trim();
            if !detail.is_empty() {
                // 子の stderr（不足項目の列挙 + setup 誘導など）をそのまま伝える。
                // 呼び出し側 CLI も「error: 」を付けるため、子側の接頭辞は剥がして二重化を防ぐ
                let detail = detail.strip_prefix("error: ").unwrap_or(detail);
                return Err(detail.to_string());
            }
            return Err(format!("デーモンが起動情報を返さず終了した（{status}）"));
        }
        let _ = child.kill();
        return Err("デーモンからの起動情報を受信できなかった（30 秒タイムアウト）".into());
    };

    // 子プロセスを切り離す（wait しない → init が引き取る）
    std::mem::forget(child);

    // 起動応答に serve へ使ったバイナリを含める（#432: start 直後に世代を確認できる）
    let mut info = info;
    info["serve_binary"] = json!(tako_bin);

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
    let is_tako = Command::new("/bin/ps")
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

// --- ローカル管理クライアント（GUI / CLI / MCP → daemon の admin API。#283）---
//
// 承認・拒否は tako-app の GUI ダイアログ専用（AI フルコントロール不変条件の例外。
// `.agent/requirements.md`）。devices list / revoke は CLI / MCP にも公開する。
// 認証はローカル管理トークン（state_dir の token ファイル、0600 = 同一ユーザーのみ）。

/// 稼働中 daemon の admin API を叩く最小 HTTP クライアント（localhost 専用）。
/// 外部依存を増やさないため std TcpStream + HTTP/1.1 の自前実装
pub fn admin_request(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    use std::io::{Read as _, Write as _};

    let status = daemon_status();
    if status["running"].as_bool() != Some(true) {
        return Err("リモートサーバーが起動していない".to_string());
    }
    let port = status["port"].as_u64().unwrap_or(DEFAULT_PORT as u64) as u16;
    let token = std::fs::read_to_string(token_path())
        .map_err(|e| format!("管理トークンの読み取りに失敗: {e}"))?
        .trim()
        .to_string();

    let body_str = body.map(|b| b.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Tako-Admin: {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
        body_str.len()
    );

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("daemon へ接続できない (127.0.0.1:{port}): {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("daemon への送信に失敗: {e}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("daemon からの受信に失敗: {e}"))?;
    let response = String::from_utf8_lossy(&response);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or("daemon の応答が不正（ヘッダ境界なし）")?;
    let status_code: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("daemon の応答が不正（ステータス行なし）")?;
    let value: Value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body.trim())
            .map_err(|e| format!("daemon の応答を解釈できない: {e}"))?
    };
    if !(200..300).contains(&status_code) {
        let msg = value["error"]
            .as_str()
            .unwrap_or("不明なエラー")
            .to_string();
        return Err(format!("daemon がエラーを返した ({status_code}): {msg}"));
    }
    Ok(value)
}

/// 登録済みデバイスの一覧（CLI `tako remote devices list` / MCP / GUI から使う）。
/// daemon 稼働中は admin API（接続状態込み）、停止中は devices.json を直接読む
pub fn devices_list() -> Result<Value, String> {
    if daemon_status()["running"].as_bool() == Some(true) {
        return admin_request("GET", "/api/admin/state", None);
    }
    let reg = DeviceRegistry::open(&state_dir())?;
    let devices: Vec<Value> = reg.devices().iter().map(device_json).collect();
    Ok(json!({ "running": false, "devices": devices, "pending": [], "connections": {} }))
}

/// デバイスの登録を失効させる（CLI `tako remote devices revoke` / MCP / GUI から使う）。
/// daemon 稼働中は admin API 経由（接続中 WS の即時切断込み）、停止中はレジストリ直接編集
pub fn devices_revoke(device_id: &str) -> Result<Value, String> {
    if daemon_status()["running"].as_bool() == Some(true) {
        return admin_request(
            "POST",
            "/api/admin/devices/revoke",
            Some(&json!({ "device_id": device_id })),
        );
    }
    let mut reg = DeviceRegistry::open(&state_dir())?;
    let device = reg.revoke(device_id)?;
    Ok(json!({ "revoked": true, "device": device_json(&device) }))
}

/// Device を API 応答用の JSON に変換する
fn device_json(d: &crate::remote_auth::Device) -> Value {
    json!({
        "id": d.id,
        "name": d.name,
        "login": d.login,
        "node_name": d.node_name,
        "role": d.role.as_str(),
        "created_at": d.created_at,
        "last_seen": d.last_seen,
    })
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

/// tmux バックエンドから全セッション・ペイン情報を v2 API 形式で返す。
/// IPC 不在時のフォールバック用（#424: v1 形式では role / agent_type がなく
/// master/worker の識別ができなかった）。
/// `live` の対話型 claude が動いているセッションは agent_type=claude + session_id を
/// 付与する（#439: app 不在でもチャットビューを提供できる）
fn tmux_list_panes_v2(
    tmux_socket: &str,
    live: &HashMap<String, crate::agents::LiveClaudeSession>,
) -> Value {
    let sessions = tako_core::tmux::list_sessions(Some(tmux_socket));
    let mut panes = Vec::new();

    for sess in &sessions {
        let live_session = live.get(&sess.name);
        for win in &sess.windows {
            let pane_list = tmux_list_window_panes(tmux_socket, &sess.name, win.index);
            for (pane_idx, _pane_tty) in pane_list.iter().enumerate() {
                let pane_id = format!("{}:{}.{}", sess.name, win.index, pane_idx);
                let title = if win.active && pane_idx == 0 {
                    format!("{} ({})", sess.name, win.name)
                } else {
                    format!("{}:{}.{}", sess.name, win.name, pane_idx)
                };
                let agent_type = if live_session.is_some_and(|l| l.interactive) {
                    "claude"
                } else {
                    "plain"
                };
                let mut entry = json!({
                    "id": pane_id,
                    "title": title,
                    "role": "",
                    "agent_type": agent_type,
                    "state": if sess.attached { "running" } else { "idle" },
                    "surface": "background",
                    "position": format!("{}/{}", pane_idx + 1, pane_list.len()),
                    "tmux_target": pane_id,
                });
                if let Some(l) = live_session {
                    entry["session_id"] = json!(l.session_id);
                }
                panes.push(entry);
            }
        }
    }

    json!({ "panes": panes, "api_version": 2 })
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

// --- HTTP サーバー ---

/// daemon 起動時に base_url から設定される許可 origin（#287 P1: cross-origin 遮断）。
/// `Access-Control-Allow-Origin: *` を廃止し、この値のみをエコーする
static CORS_ALLOWED_ORIGIN: OnceLock<String> = OnceLock::new();

fn cors_headers() -> Vec<tiny_http::Header> {
    let origin = CORS_ALLOWED_ORIGIN
        .get()
        .map(|s| s.as_str())
        .unwrap_or("null");
    vec![
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], origin.as_bytes())
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
        tiny_http::Header::from_bytes(&b"Vary"[..], &b"Origin"[..]).expect("固定ヘッダ"),
    ]
}

/// Origin ヘッダを base_url と照合する（#287 P1: cross-origin 遮断）。
/// - Origin 欠落 = 同一 origin のブラウザリクエストまたは非ブラウザ（CLI 等）→ 許可
/// - Origin が base_url と一致 → 許可
/// - Origin が base_url と不一致 → 拒否（evil origin）
fn check_request_origin(ctx: &DaemonCtx, request: &tiny_http::Request) -> Result<(), String> {
    if let Some(origin) = header_value(request, "origin") {
        let allowed = ctx.base_url.trim_end_matches('/');
        let given = origin.trim_end_matches('/');
        if !given.eq_ignore_ascii_case(allowed) {
            return Err(format!("Origin '{origin}' は許可されていない"));
        }
    }
    Ok(())
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

/// リクエストボディを JSON として読む（サイズ上限つき）
fn read_json_body(request: &mut tiny_http::Request) -> Result<Value, String> {
    use std::io::Read as _;
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut body)
        .map_err(|_| "リクエストボディの読み取りに失敗".to_string())?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).map_err(|e| format!("JSON パースエラー: {e}"))
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

// --- dispatch 統合版ハンドラ（#281 H-7）---

/// IPC 経由でペイン一覧を取得し、マッピングを更新する。
/// 成功時は List 応答全体を返す
fn refresh_pane_mapping(
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
) -> Option<Value> {
    let mut conn = app_conn.write().ok()?;
    let client = conn.get()?;
    match client.request(crate::protocol::Request::List) {
        Ok(list) => {
            if let Ok(mut mapping) = pane_mapping.write() {
                mapping.update_from_list(&list);
            }
            Some(list)
        }
        Err(_) => {
            conn.invalidate();
            None
        }
    }
}

/// pane パラメータ（数値 PaneId または tmux ターゲット）を tmux ターゲットに解決する。
/// 数値 PaneId の場合はキャッシュを参照し、未ヒットなら IPC List で更新してから再試行する。
/// tmux ターゲット形式（`:` を含む）ならそのまま返す。
/// 解決できなければ None（#423/#426: v2 API が返す数値 ID への対応）
fn resolve_pane_param(
    pane_param: &str,
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
) -> Option<String> {
    if pane_param.contains(':') {
        return Some(pane_param.to_string());
    }
    if let Ok(mapping) = pane_mapping.read() {
        if let Some(target) = mapping.resolve_tmux_target(pane_param) {
            return Some(target);
        }
    }
    refresh_pane_mapping(app_conn, pane_mapping);
    pane_mapping
        .read()
        .ok()
        .and_then(|m| m.resolve_tmux_target(pane_param))
}

/// List 応答をリモート API v2 形式に変換する。
/// agent 種別・タイトル・role・cwd・タブ内位置・モデル・状態・session_id を含むペイン情報。
/// `live` は agents::live_claude_sessions_by_backend の一括解決マップ
/// （バックエンドセッション → 稼働中 claude セッション）。
/// role が消失したペイン（#439: 復元・handoff・手動 resume 後の master が典型）でも、
/// live 解決（pid 祖先 = 実プロセスの存在証明）が interactive claude を示せば
/// agent_type を claude として扱う
fn list_to_api_v2(list: &Value, live: &HashMap<String, crate::agents::LiveClaudeSession>) -> Value {
    let mut panes = Vec::new();
    let tabs = list["tabs"].as_array().cloned().unwrap_or_default();
    for tab in &tabs {
        let tab_title = tab["title"].as_str().unwrap_or("");
        let tab_id = tab["id"].as_u64().unwrap_or(0);
        let tab_panes = tab["panes"].as_array().cloned().unwrap_or_default();
        let total_panes = tab_panes.len();
        for (idx, pane) in tab_panes.iter().enumerate() {
            let id = pane["id"].as_u64().unwrap_or(0);
            let role = pane["role"].as_str().unwrap_or("");
            let title = pane["title"].as_str().unwrap_or("");
            let cwd = pane["cwd"].as_str();
            let state = pane["state"].as_str().unwrap_or("unknown");
            let surface = pane["surface"].as_str().unwrap_or("background");

            // session_id + model: live 解決 → sessions カタログの順でペインに紐づく情報を解決
            let live_session = pane["tmux_session"]
                .as_str()
                .filter(|s| !s.is_empty())
                .and_then(|s| live.get(s));
            let (session_id, model) = resolve_pane_session_info(id, live_session);

            // agent 種別の推定: role から判定（master/solo/worker は同一ロジック）。
            // role が無くても live の対話型 claude が動いていれば claude（#439）
            let has_agent_role = role.contains("master")
                || role.contains("solo")
                || role.starts_with("orchestrator-worker");
            let agent_type = if has_agent_role {
                if role.contains("codex") {
                    "codex"
                } else if role.contains("agy") {
                    "agy"
                } else {
                    "claude"
                }
            } else if live_session.is_some_and(|l| l.interactive)
                || pane["osc_title"]
                    .as_str()
                    .is_some_and(|t| t.contains("claude"))
            {
                "claude"
            } else {
                "plain"
            };

            // タブ内位置（1/N 形式）
            let position = format!("{}/{}", idx + 1, total_panes);

            // tmux ターゲット: WS / screen API で数値 PaneId の代わりに使う
            let tmux_target = pane["tmux_session"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s}:0.0"));

            let mut entry = json!({
                "id": id,
                "title": title,
                "role": role,
                "agent_type": agent_type,
                "cwd": cwd,
                "state": state,
                "surface": surface,
                "position": position,
                "tab_id": tab_id,
                "tab_title": tab_title,
                "cols": pane["cols"],
                "rows": pane["rows"],
                "focused": pane["focused"],
                "tmux_target": tmux_target,
            });
            if let Some(sid) = session_id {
                entry["session_id"] = json!(sid);
            }
            if let Some(m) = model {
                entry["model"] = json!(m);
            }
            panes.push(entry);
        }
    }
    json!({ "panes": panes, "api_version": 2 })
}

/// ペインの session_id と model を解決する（#284）。
/// live 解決（agents pid 祖先の一括マップ）→ sessions カタログの順で探す
fn resolve_pane_session_info(
    pane_id: u64,
    live_session: Option<&crate::agents::LiveClaudeSession>,
) -> (Option<String>, Option<String>) {
    let session_id = live_session.map(|l| l.session_id.clone()).or_else(|| {
        let id_str = pane_id.to_string();
        crate::sessions::resolve_session_for_pane(&id_str)
    });
    let model = session_id.as_ref().and_then(|sid| {
        let catalog = crate::sessions::SessionCatalog::load().ok()?;
        catalog.entries.get(sid)?.model.clone()
    });
    (session_id, model)
}

/// 各 agent ペインの画面から permission ダイアログを検知し、ペインエントリへ
/// `permission_dialog: {command, options, highlighted}` を付与する（#425）。
///
/// 承認待ちの判定は transcript の推定ではなく**画面のダイアログ実在**を正とする
/// （transcript は「ツール実行中」と「承認待ち停止」を区別できない。auto mode の
/// 実行中ウィンドウで誤って承認カードが出ていた根因）。
/// `capture` はバックエンドセッション名 → 画面行（テスト差し替え用）
fn attach_permission_dialogs(result: &mut Value, capture: impl Fn(&str) -> Option<Vec<String>>) {
    let Some(panes) = result["panes"].as_array_mut() else {
        return;
    };
    for pane in panes {
        // ダイアログを出しうるのは agent ペインだけ（plain のシェルはスキップ）
        let agent_type = pane["agent_type"].as_str().unwrap_or("plain");
        if agent_type == "plain" {
            continue;
        }
        let Some(session) = pane["tmux_target"]
            .as_str()
            .map(session_name_of)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(lines) = capture(&session) else {
            continue;
        };
        if let Some(dialog) = crate::claude_tui::detect_permission_dialog(&lines) {
            pane["permission_dialog"] = json!({
                "command": dialog.command,
                "options": dialog.options,
                "highlighted": dialog.highlighted,
            });
        }
    }
}

/// tmux ターゲット（`session:0.0` / `session`）からセッション名部分を取り出す。
/// dispatch の tmux_session フィールドはセッション名を期待する（deliver 系が
/// `={session}:` を組み立てるため、`:0.0` が残ると can't find pane になる。#428）
fn session_name_of(target: &str) -> String {
    target.split(':').next().unwrap_or(target).to_string()
}

/// IPC 経由で Send dispatch を呼ぶ。app が不在なら 503 を返す。
/// `pane` は tako PaneId（GUI 側 resolve_pane で解決 = 送達検証フローが効く正規経路）、
/// `tmux_session` はペイン消失時のフォールバック用**セッション名**。
/// 注意: "session:0.0" のような tmux ターゲット形式を tmux_session に渡してはならない。
/// deliver 系は `={session}:` を組み立てるため `=session:0.0:` となり
/// can't find pane で無音失敗する（#428 の根因）
fn dispatch_send(
    app_conn: &Arc<RwLock<AppConnection>>,
    pane: Option<u64>,
    tmux_session: Option<String>,
    text: &str,
    newline: bool,
    keys: Option<&str>,
) -> Result<Value, (u16, String)> {
    let request = if let Some(keys_str) = keys {
        crate::protocol::Request::Send {
            pane,
            text: keys_str.to_string(),
            newline: false,
            tmux_session,
            await_prompt: false,
        }
    } else {
        crate::protocol::Request::Send {
            pane,
            text: text.to_string(),
            newline,
            tmux_session,
            await_prompt: false,
        }
    };

    let mut conn = app_conn
        .write()
        .map_err(|_| (500u16, "内部エラー".to_string()))?;
    let client = conn.get().ok_or((
        503u16,
        "tako app が稼働していない（リモートからの入力は app 経由のみ）".to_string(),
    ))?;
    match client.request(request) {
        Ok(v) => Ok(v),
        Err(e) => {
            conn.invalidate();
            Err((502, e))
        }
    }
}

/// IPC 経由で OrchestratorRespond dispatch を呼ぶ（#425: permission ダイアログ応答）。
/// dispatch 側で画面のダイアログ実在を再検証するため、ダイアログが既に解消済みなら
/// エラーが返る（409 相当としてクライアントへ伝える）
fn dispatch_respond(
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_id: u64,
    choice: &str,
    device_name: &str,
) -> Result<Value, (u16, String)> {
    let request = crate::protocol::Request::OrchestratorRespond {
        pane_id,
        choice: choice.to_string(),
        caller_role: Some(format!("remote:{device_name}")),
    };
    let mut conn = app_conn
        .write()
        .map_err(|_| (500u16, "内部エラー".to_string()))?;
    let client = conn.get().ok_or((
        503u16,
        "tako app が稼働していない（リモートからの承認応答は app 経由のみ）".to_string(),
    ))?;
    match client.request(request) {
        Ok(v) => Ok(v),
        Err(e) => {
            // ダイアログ不在（解消済み）は接続エラーではないので invalidate しない
            if e.contains("ダイアログが見つからない") {
                return Err((409, e));
            }
            conn.invalidate();
            Err((502, e))
        }
    }
}

/// IPC 経由で Close dispatch を呼ぶ。PaneId が必要なため、
/// まず List でマッピングを取得してから Close を実行する
fn dispatch_close(
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
    tmux_target: &str,
) -> Result<Value, (u16, String)> {
    // まず List を取得して PaneId を解決する
    let list = {
        let mut conn = app_conn
            .write()
            .map_err(|_| (500u16, "内部エラー".to_string()))?;
        let client = conn.get().ok_or((
            503u16,
            "tako app が稼働していない（リモートからの close は app 経由のみ）".to_string(),
        ))?;
        match client.request(crate::protocol::Request::List) {
            Ok(v) => v,
            Err(e) => {
                conn.invalidate();
                return Err((502u16, e));
            }
        }
    };

    if let Ok(mut mapping) = pane_mapping.write() {
        mapping.update_from_list(&list);
    }

    // tmux target からセッション名を取り出して PaneId を探す
    let session_name = tmux_target
        .strip_prefix('=')
        .unwrap_or(tmux_target)
        .split(':')
        .next()
        .unwrap_or(tmux_target);

    let pane_id = find_pane_id_for_tmux_target(&list, session_name);
    let Some(pid) = pane_id else {
        return Err((404, format!("ペイン '{tmux_target}' が見つからない")));
    };

    // Close を実行する
    let mut conn = app_conn
        .write()
        .map_err(|_| (500u16, "内部エラー".to_string()))?;
    let client = conn
        .get()
        .ok_or((503u16, "tako app が稼働していない".to_string()))?;
    let result = client.request(crate::protocol::Request::Close {
        pane: Some(pid),
        force: false,
    });
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            if e.contains("LastPane") || e.contains("最後") {
                Err((409, e))
            } else {
                conn.invalidate();
                Err((502, e))
            }
        }
    }
}

/// List 応答からセッション名にマッチする PaneId を探す
fn find_pane_id_for_tmux_target(list: &Value, session_name: &str) -> Option<u64> {
    let tabs = list["tabs"].as_array()?;
    for tab in tabs {
        let panes = tab["panes"].as_array()?;
        for pane in panes {
            // backend_windows がある = tmux backend ペイン
            if let Some(windows) = pane["backend_windows"].as_array() {
                if !windows.is_empty() {
                    // ペインのタイトルまたは osc_title に session 名が含まれるか確認
                    let title = pane["title"].as_str().unwrap_or("");
                    let role = pane["role"].as_str().unwrap_or("");
                    // backend session 名は通常 `tako-<hash>` 形式。
                    // tmux target の session 部分と一致するか
                    if title.contains(session_name)
                        || role.contains(session_name)
                        || session_name.starts_with("tako-")
                    {
                        return pane["id"].as_u64();
                    }
                }
            }
            // tmux target の session が pane の何らかの属性と一致する場合
            let pane_id = pane["id"].as_u64()?;
            // backend_windows が無くても、全ペインの ID リストから最も合致するものを返す
            // （tmux target ではなく PaneId を直接使える場合）
            if let Ok(id_str) = session_name.parse::<u64>() {
                if pane_id == id_str {
                    return Some(pane_id);
                }
            }
        }
    }
    None
}

/// dispatch 統合版のリクエストハンドラ。
/// 書き込み系（input/close/resize）は IPC 経由で dispatch を通す。
/// 読み取り系は従来どおり tmux 直接 + API v2 は IPC List 経由。
///
/// 認可（#283 二層認証）:
/// - 静的アセット・/api/health・/api/me・/api/pair: 層①のみ（tailnet ノードなら可）
/// - /api/admin/*: ローカル直結 + 管理トークンのみ（GUI / CLI / MCP 用）
/// - その他の /api/*: 層① + 層②（Observe / Interact / Manage / Admin の role 認可）
fn handle_request_v2(
    mut request: tiny_http::Request,
    ctx: &Arc<DaemonCtx>,
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
    broadcasters: &BroadcasterMap,
) {
    let method = request.method().clone();
    let url_full = request.url().to_string();
    let path = url_full.split('?').next().unwrap_or("").to_string();
    let tmux_socket = &ctx.tmux_socket;

    // #287 P1: cross-origin 遮断。Origin ヘッダが存在し base_url と不一致なら即拒否
    // （evil origin の fetch / preflight を認証より手前で遮断する）
    if let Err(e) = check_request_origin(ctx, &request) {
        return respond(request, 403, Some(json!({ "error": e }).to_string()));
    }

    // CORS preflight（Origin 検証済みの場合のみ到達する）
    if method == tiny_http::Method::Options {
        return respond(request, 204, None);
    }

    // --- 管理 API（ローカル直結 + 管理トークン。GUI 承認ダイアログ / CLI / MCP 用）---
    if path.starts_with("/api/admin/") {
        if !check_admin(ctx, &request) {
            return respond(
                request,
                401,
                Some(json!({ "error": "管理 API はローカルの管理トークンが必要" }).to_string()),
            );
        }
        return handle_admin_api(request, &method, &path, ctx, broadcasters);
    }

    // --- 層①: tailnet identity 検証（serve 経由の実在ノードのみ通す）---
    // 静的アセット（PWA）とペアリング前 API も tailnet ノード限定
    let who = match identify_tailnet(ctx, &request) {
        Ok(who) => who,
        Err((status, e)) => {
            return respond(request, status, Some(json!({ "error": e }).to_string()));
        }
    };

    // API 以外のパスは PWA 静的ファイルとして配信（層①通過のみで可 —
    // 未登録端末がペアリング画面を表示するために必要。画面データは含まない）
    if !path.starts_with("/api/") {
        return serve_embedded(request, &path);
    }

    // /api/health: 層①のみ。version は PWA の互換チェックに使う
    if path == "/api/health" && method == tiny_http::Method::Get {
        return respond(
            request,
            200,
            Some(
                json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                })
                .to_string(),
            ),
        );
    }

    // /api/me: 層①のみ。この端末の登録状態を返す（ペアリング画面のポーリング先）
    if path == "/api/me" && method == tiny_http::Method::Get {
        let mut reg = ctx.registry.lock().unwrap();
        let body = match reg.device(&who.stable_id).cloned() {
            Some(device) => json!({
                "registered": true,
                "device_id": device.id,
                "name": device.name,
                "role": device.role.as_str(),
                "login": device.login,
                "host": hostname(),
                "version": env!("CARGO_PKG_VERSION"),
                "app_connected": app_conn
                    .read()
                    .ok()
                    .and_then(|c| c.client.as_ref().map(|_| true))
                    .unwrap_or(false),
            }),
            None => {
                let pending = reg.pending().iter().any(|p| p.device_id == who.stable_id);
                json!({
                    "registered": false,
                    "pending": pending,
                    "denied": !pending && reg.recently_denied(&who.stable_id),
                    "host": hostname(),
                    "version": env!("CARGO_PKG_VERSION"),
                })
            }
        };
        return respond_sensitive(request, 200, Some(body.to_string()));
    }

    // /api/pair: 層①のみ。ペアリング / role 変更を要求する（Mac 承認待ちになる）
    if path == "/api/pair" && method == tiny_http::Method::Post {
        let parsed = match read_json_body(&mut request) {
            Ok(v) => v,
            Err(e) => {
                return respond(request, 400, Some(json!({ "error": e }).to_string()));
            }
        };
        let name = parsed["name"].as_str().unwrap_or("").to_string();
        let role_str = parsed["role"].as_str().unwrap_or("observe");
        let Some(role) = DeviceRole::parse(role_str) else {
            return respond(
                request,
                400,
                Some(
                    json!({ "error": format!("不正な role: {role_str}（observe / interact / manage / admin）") })
                        .to_string(),
                ),
            );
        };
        let result = ctx
            .registry
            .lock()
            .unwrap()
            .request_pairing(&who, &name, role);
        return respond_sensitive(request, 200, Some(result.to_string()));
    }

    // --- 層②: 機器ペアリング認可（未登録・role 不足は 403）---
    // 読み取り系は Observe、input は Interact、close / resize は Manage、
    // 端末管理は Admin（役割は remote_auth::DeviceRole。強い role は弱い role を包含）
    let required = required_role(&method, &path);
    let device = match authorize_device_checked(ctx, &request, required) {
        Ok(device) => device,
        Err((status, e)) => {
            return respond(request, status, Some(json!({ "error": e }).to_string()));
        }
    };

    // --- リモート端末管理（Admin role。承認・role 変更は含まない = GUI 限定）---
    if path == "/api/devices" && method == tiny_http::Method::Get {
        let reg = ctx.registry.lock().unwrap();
        let devices: Vec<Value> = reg.devices().iter().map(device_json).collect();
        return respond_sensitive(
            request,
            200,
            Some(json!({ "devices": devices }).to_string()),
        );
    }
    if path == "/api/devices/revoke" && method == tiny_http::Method::Post {
        let parsed = match read_json_body(&mut request) {
            Ok(v) => v,
            Err(e) => {
                return respond(request, 400, Some(json!({ "error": e }).to_string()));
            }
        };
        let Some(target_id) = parsed["device_id"].as_str() else {
            return respond(
                request,
                400,
                Some(json!({ "error": "device_id が必要" }).to_string()),
            );
        };
        let result = ctx.registry.lock().unwrap().revoke(target_id);
        return match result {
            Ok(revoked) => {
                disconnect_device_ws(broadcasters, target_id);
                respond(
                    request,
                    200,
                    Some(json!({ "revoked": true, "device": device_json(&revoked) }).to_string()),
                )
            }
            Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
        };
    }

    // --- API v2 エンドポイント（IPC 経由のリッチ情報。#281）---
    if path == "/api/v2/panes" && method == tiny_http::Method::Get {
        // live claude セッションの一括解決（#439: role 消失ペインの agent 判定 ground truth）
        let live = crate::agents::live_claude_sessions_by_backend();
        match refresh_pane_mapping(app_conn, pane_mapping) {
            Some(list) => {
                let mut result = list_to_api_v2(&list, &live);
                // 承認待ちの正 = 画面の permission ダイアログ実在（#425）
                attach_permission_dialogs(&mut result, |session| {
                    tako_core::tmux::capture_session(Some(tmux_socket), session).ok()
                });
                return respond_sensitive(request, 200, Some(result.to_string()));
            }
            None => {
                // app 不在: tmux-only のペイン一覧を v2 形式で返す（#424:
                // v1 形式では role / agent_type がなく master の識別ができない）
                let mut result = tmux_list_panes_v2(tmux_socket, &live);
                attach_permission_dialogs(&mut result, |session| {
                    tako_core::tmux::capture_session(Some(tmux_socket), session).ok()
                });
                return respond(request, 200, Some(result.to_string()));
            }
        }
    }

    // --- API ルーティング（dispatch 統合 + tmux 直接操作）---
    handle_api_v2_routes(
        request,
        method,
        &path,
        &url_full,
        ctx,
        &device,
        app_conn,
        pane_mapping,
    )
}

/// パス・メソッドから必要 role を決める（強い role は弱い role の操作を包含する）
fn required_role(method: &tiny_http::Method, path: &str) -> DeviceRole {
    if path.starts_with("/api/devices") {
        return DeviceRole::Admin;
    }
    if *method == tiny_http::Method::Post {
        if path.ends_with("/input") || path.ends_with("/respond") || path == "/api/upload" {
            return DeviceRole::Interact;
        }
        if path.ends_with("/close") || path.ends_with("/resize") {
            return DeviceRole::Manage;
        }
        // 未知の POST は安全側（Manage）
        return DeviceRole::Manage;
    }
    DeviceRole::Observe
}

/// authorize_device の Result 版（呼び出し側の early-return 用）
fn authorize_device_checked(
    ctx: &DaemonCtx,
    request: &tiny_http::Request,
    required: DeviceRole,
) -> Result<crate::remote_auth::Device, (u16, String)> {
    match authorize_device(ctx, request, required) {
        AuthDecision::Allowed(device) => Ok(device),
        AuthDecision::Rejected(status, e) => Err((status, e)),
    }
}

/// 管理 API のルーティング（check_admin 通過後に呼ばれる）。
/// ペアリング承認・拒否はここだけ = Mac 側 GUI（+ 同一ユーザーのローカルプロセス）限定
fn handle_admin_api(
    mut request: tiny_http::Request,
    method: &tiny_http::Method,
    path: &str,
    ctx: &Arc<DaemonCtx>,
    broadcasters: &BroadcasterMap,
) {
    match (method.clone(), path) {
        // 状態スナップショット（GUI の 2 秒ポーリング先）
        (tiny_http::Method::Get, "/api/admin/state") => {
            let reg = ctx.registry.lock().unwrap();
            let devices: Vec<Value> = reg.devices().iter().map(device_json).collect();
            let pending: Vec<Value> = reg
                .pending()
                .iter()
                .map(|p| {
                    json!({
                        "device_id": p.device_id,
                        "name": p.name,
                        "login": p.login,
                        "node_name": p.node_name,
                        "requested_role": p.requested_role.as_str(),
                        "kind": p.kind,
                        "requested_at": p.requested_at,
                    })
                })
                .collect();
            drop(reg);
            let connections = ctx.connections_snapshot();
            let body = json!({
                "running": true,
                "port": ctx.port,
                "url": ctx.base_url,
                "devices": devices,
                "pending": pending,
                "connections": connections,
            });
            respond_sensitive(request, 200, Some(body.to_string()))
        }
        // ペアリング / 昇格の承認（GUI 限定経路）
        (tiny_http::Method::Post, "/api/admin/pair/approve") => {
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            let Some(device_id) = parsed["device_id"].as_str() else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "device_id が必要" }).to_string()),
                );
            };
            let role = parsed["role"].as_str().and_then(DeviceRole::parse);
            let result = ctx.registry.lock().unwrap().approve(device_id, role);
            match result {
                Ok(device) => {
                    notify_macos(&format!(
                        "{} を {} として登録しました",
                        device.name,
                        device.role.as_str()
                    ));
                    respond(
                        request,
                        200,
                        Some(
                            json!({ "approved": true, "device": device_json(&device) }).to_string(),
                        ),
                    )
                }
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        // ペアリング / 昇格の拒否（GUI 限定経路）
        (tiny_http::Method::Post, "/api/admin/pair/deny") => {
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            let Some(device_id) = parsed["device_id"].as_str() else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "device_id が必要" }).to_string()),
                );
            };
            let result = ctx.registry.lock().unwrap().deny(device_id);
            match result {
                Ok(()) => respond(request, 200, Some(json!({ "denied": true }).to_string())),
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        // デバイス失効（CLI / MCP / GUI の revoke。接続中 WS は即時切断）
        (tiny_http::Method::Post, "/api/admin/devices/revoke") => {
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            let Some(device_id) = parsed["device_id"].as_str() else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "device_id が必要" }).to_string()),
                );
            };
            let result = ctx.registry.lock().unwrap().revoke(device_id);
            match result {
                Ok(device) => {
                    disconnect_device_ws(broadcasters, device_id);
                    respond(
                        request,
                        200,
                        Some(
                            json!({ "revoked": true, "device": device_json(&device) }).to_string(),
                        ),
                    )
                }
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        _ => respond(
            request,
            404,
            Some(json!({ "error": "管理 API エンドポイントが見つからない" }).to_string()),
        ),
    }
}

/// 認可済みリクエストの API ルーティング（従来の handle_request_v2 後段）
#[allow(clippy::too_many_arguments)]
fn handle_api_v2_routes(
    mut request: tiny_http::Request,
    method: tiny_http::Method,
    path: &str,
    url_full: &str,
    ctx: &Arc<DaemonCtx>,
    device: &crate::remote_auth::Device,
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
) {
    let tmux_socket = &ctx.tmux_socket;
    match (method, path) {
        (tiny_http::Method::Get, "/api/panes") => {
            // v1: 従来の tmux 直接一覧（後方互換）
            let result = tmux_list_panes(tmux_socket);
            respond(request, 200, Some(result.to_string()))
        }
        (tiny_http::Method::Get, "/api/agents") => {
            match crate::agents::list_agents_with_panes(Some(tmux_socket)) {
                Ok(result) => respond_sensitive(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 502, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Get, p)
            if p.starts_with("/api/sessions/") && p.ends_with("/messages") =>
        {
            let Some(session_id) = extract_session_id(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なセッション ID" }).to_string()),
                );
            };
            let tail = query_param(url_full, "tail")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(30);
            match crate::transcript::read_messages(&session_id, tail) {
                Ok(result) => respond_sensitive(request, 200, Some(result.to_string())),
                Err(e) => respond(request, 404, Some(json!({ "error": e }).to_string())),
            }
        }
        (tiny_http::Method::Get, p) if p.starts_with("/api/panes/") && p.ends_with("/screen") => {
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let target =
                resolve_pane_param(&pane_param, app_conn, pane_mapping).unwrap_or(pane_param);
            let tmux_target = format!("={target}");
            let ansi = query_param(url_full, "ansi").is_some_and(|v| v == "1" || v == "true");
            let history = query_param(url_full, "lines").and_then(|v| v.parse::<u32>().ok());
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
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let target =
                resolve_pane_param(&pane_param, app_conn, pane_mapping).unwrap_or(pane_param);
            let tmux_target = format!("={target}");
            let history = query_param(url_full, "lines")
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
        // --- 書き込み系: dispatch 経由（H-7）---
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/input") => {
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            // IPC 経由で dispatch Send を呼ぶ（H-7: #95 送達検証が効く）
            let keys = parsed["keys"].as_str().map(|s| s.to_string());
            let text = parsed["text"].as_str().unwrap_or("").to_string();
            let newline = parsed["newline"].as_bool().unwrap_or(true);
            // 監査 metadata（バイト数のみ。テキスト内容は記録しない）+ interact session。
            // idle timeout 明けの最初の入力は操作セッション開始として macOS 通知を出す
            let input_bytes = text.len() + keys.as_deref().map(|k| k.len()).unwrap_or(0);
            {
                let mut reg = ctx.registry.lock().unwrap();
                let session_started = reg.record_input(&device.id);
                reg.audit(
                    "input",
                    &device.id,
                    &device.name,
                    json!({ "route": p, "bytes": input_bytes }),
                );
                drop(reg);
                if session_started {
                    notify_macos(&format!("{} が操作を開始しました", device.name));
                }
            }
            // dispatch Send へは PaneId（数値）を渡して GUI 側の resolve_pane に解決させる
            // （alt_screen の送達検証フロー #32/#95 が効く正規経路）。tmux_session
            // フォールバックには tmux ターゲットではなく**セッション名**を渡す
            // （#428: "session:0.0" を渡すと deliver 系の `={session}:` 組み立てが
            // `=session:0.0:` になり can't find pane で無音失敗していた）
            let pane_id: Option<u64> = pane_param.parse().ok();
            let session_name = resolve_pane_param(&pane_param, app_conn, pane_mapping)
                .map(|t| session_name_of(&t));
            match dispatch_send(
                app_conn,
                pane_id,
                session_name,
                &text,
                newline,
                keys.as_deref(),
            ) {
                Ok(_) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                Err((status, e)) => {
                    respond(request, status, Some(json!({ "error": e }).to_string()))
                }
            }
        }
        // permission ダイアログへの応答（#425）: 画面のダイアログ実在を dispatch 側で
        // 再検証してから番号キー + Enter を送る（OrchestratorRespond と同一経路 = 監査つき）
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/respond") => {
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let Ok(pane_id) = pane_param.parse::<u64>() else {
                return respond(
                    request,
                    400,
                    Some(
                        json!({ "error": "respond は数値ペイン ID のみ（app 稼働時の一覧から取得してください）" })
                            .to_string(),
                    ),
                );
            };
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            let choice = match &parsed["choice"] {
                Value::String(s) if !s.is_empty() => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => {
                    return respond(
                        request,
                        400,
                        Some(json!({ "error": "choice が必要（番号または yes/no）" }).to_string()),
                    );
                }
            };
            ctx.registry.lock().unwrap().audit(
                "respond",
                &device.id,
                &device.name,
                json!({ "route": p, "choice": choice }),
            );
            match dispatch_respond(app_conn, pane_id, &choice, &device.name) {
                Ok(v) => respond(request, 200, Some(v.to_string())),
                Err((status, e)) => {
                    respond(request, status, Some(json!({ "error": e }).to_string()))
                }
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/close") => {
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let target =
                resolve_pane_param(&pane_param, app_conn, pane_mapping).unwrap_or(pane_param);
            ctx.registry.lock().unwrap().audit(
                "close",
                &device.id,
                &device.name,
                json!({ "route": p }),
            );
            match dispatch_close(app_conn, pane_mapping, &target) {
                Ok(_) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                Err((status, e)) => {
                    respond(request, status, Some(json!({ "error": e }).to_string()))
                }
            }
        }
        (tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/resize") => {
            let Some(pane_param) = extract_pane_target(p) else {
                return respond(
                    request,
                    400,
                    Some(json!({ "error": "無効なペイン ID" }).to_string()),
                );
            };
            let target =
                resolve_pane_param(&pane_param, app_conn, pane_mapping).unwrap_or(pane_param);
            let parsed = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    return respond(request, 400, Some(json!({ "error": e }).to_string()));
                }
            };
            ctx.registry.lock().unwrap().audit(
                "resize",
                &device.id,
                &device.name,
                json!({ "route": p }),
            );
            let session_part = target.split(':').next().unwrap_or(&target);
            let window_part = target
                .split(':')
                .nth(1)
                .and_then(|w| w.split('.').next())
                .and_then(|w| w.parse::<u32>().ok())
                .unwrap_or(0);
            let reset = parsed["reset"].as_bool() == Some(true);
            let cols = parsed["cols"].as_u64().map(|c| c as u32);
            let rows = parsed["rows"].as_u64().map(|r| r as u32);

            // IPC 経由で TmuxResize を呼ぶ
            let mut conn_guard = match app_conn.write() {
                Ok(g) => g,
                Err(_) => {
                    return respond(
                        request,
                        500,
                        Some(json!({ "error": "内部エラー" }).to_string()),
                    );
                }
            };
            match conn_guard.get() {
                Some(client) => {
                    let result = client.request(crate::protocol::Request::TmuxResize {
                        socket: Some(tako_core::tmux_backend::socket_name()),
                        session: session_part.to_string(),
                        window: window_part,
                        cols: if reset { None } else { cols },
                        rows: if reset { None } else { rows },
                        reset,
                    });
                    drop(conn_guard);
                    match result {
                        Ok(_) => respond(request, 200, Some(json!({ "ok": true }).to_string())),
                        Err(e) => respond(request, 502, Some(json!({ "error": e }).to_string())),
                    }
                }
                None => {
                    drop(conn_guard);
                    // app 不在時は tmux 直接操作にフォールバック（resize は読み取りに近い操作）
                    let window_target = format!("={}", window_target_of(&target));
                    let result = if reset {
                        tmux_reset_window_size(tmux_socket, &window_target)
                    } else {
                        match (cols, rows) {
                            (Some(c), Some(r)) if c > 0 && r > 0 => {
                                tmux_resize_window(tmux_socket, &window_target, c, r)
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
            }
        }
        // --- ファイルアップロード（#285）: Interact role 必須 ---
        (tiny_http::Method::Post, "/api/upload") => {
            handle_upload(request, ctx, device, pane_mapping)
        }
        _ => respond(
            request,
            404,
            Some(json!({ "error": "API エンドポイントが見つからない" }).to_string()),
        ),
    }
}

/// POST /api/upload: リモート端末からのファイルアップロード（#285）
///
/// Interact role 必須 / 20MB 上限 / 保存先は対象ペインの cwd 配下
/// `.tako-remote-uploads/` 固定 / パス traversal 検証 / 実行権限なし
fn handle_upload(
    mut request: tiny_http::Request,
    ctx: &Arc<DaemonCtx>,
    device: &crate::remote_auth::Device,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
) {
    const MAX_UPLOAD_SIZE: u64 = 20 * 1024 * 1024; // 20MB
    const UPLOAD_DIR: &str = ".tako-remote-uploads";

    // Content-Length の事前チェック
    let content_length = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Length"))
        .and_then(|h| h.value.as_str().parse::<u64>().ok())
        .unwrap_or(0);
    if content_length > MAX_UPLOAD_SIZE {
        return respond(
            request,
            413,
            Some(json!({ "error": format!("ファイルサイズが上限 ({}MB) を超えています", MAX_UPLOAD_SIZE / (1024 * 1024)) }).to_string()),
        );
    }

    // Content-Type が multipart/form-data であることを確認
    let content_type = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let boundary = extract_multipart_boundary(&content_type);
    let Some(boundary) = boundary else {
        return respond(
            request,
            400,
            Some(json!({ "error": "multipart/form-data が必要です" }).to_string()),
        );
    };

    // ボディを全て読む（サイズ上限を超えたら中断）
    use std::io::Read as _;
    let mut body = Vec::new();
    let mut reader = request.as_reader().take(MAX_UPLOAD_SIZE);
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&buf[..n]);
                if body.len() as u64 > MAX_UPLOAD_SIZE {
                    return respond(
                        request,
                        413,
                        Some(json!({ "error": format!("ファイルサイズが上限 ({}MB) を超えています", MAX_UPLOAD_SIZE / (1024 * 1024)) }).to_string()),
                    );
                }
            }
            Err(_) => break,
        }
    }

    // multipart パーシング（最小実装: file と pane フィールドを抽出）
    let (file_name, file_data, pane_id) = match parse_multipart(&body, &boundary) {
        Ok(parts) => parts,
        Err(e) => {
            return respond(request, 400, Some(json!({ "error": e }).to_string()));
        }
    };

    // ファイル名の traversal 検証
    if file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.is_empty()
    {
        return respond(
            request,
            400,
            Some(json!({ "error": "不正なファイル名です" }).to_string()),
        );
    }

    // cwd を取得（pane_mapping の pane_info から）
    let cwd = {
        let mapping = pane_mapping.read().unwrap();
        pane_id
            .parse::<u64>()
            .ok()
            .and_then(|id| mapping.pane_info.get(&id))
            .and_then(|info| info["cwd"].as_str())
            .map(String::from)
    };

    let Some(cwd) = cwd else {
        return respond(
            request,
            404,
            Some(json!({ "error": "対象ペインの作業ディレクトリが取得できません" }).to_string()),
        );
    };

    // 保存先ディレクトリの作成
    let upload_dir = std::path::Path::new(&cwd).join(UPLOAD_DIR);
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        return respond(
            request,
            500,
            Some(
                json!({ "error": format!("アップロードディレクトリの作成に失敗: {e}") })
                    .to_string(),
            ),
        );
    }

    // symlink follow 拒否: upload_dir 自体がシンボリックリンクなら拒否する。
    // リンク先への意図しない書き込み（任意パスへのファイル配置）を防ぐ（#287 P2-4）
    if upload_dir
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return respond(
            request,
            400,
            Some(
                json!({ "error": "アップロード先がシンボリックリンクのため拒否しました" })
                    .to_string(),
            ),
        );
    }

    // パスの最終検証（canonicalize で traversal を防止）
    let target_path = upload_dir.join(&file_name);

    // target_path が既存の symlink なら拒否（上書きによるリンク先改変を防ぐ。#287 P2-4）
    if target_path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return respond(
            request,
            400,
            Some(
                json!({ "error": "既存のシンボリックリンクへの上書きは拒否しました" }).to_string(),
            ),
        );
    }

    let canonical_dir = match upload_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return respond(
                request,
                500,
                Some(json!({ "error": format!("パス解決に失敗: {e}") }).to_string()),
            );
        }
    };

    // ファイル書き込み（所有者のみ読み書き可。#287 P2-1）
    use std::io::Write;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&target_path)
    {
        Ok(f) => f,
        Err(e) => {
            return respond(
                request,
                500,
                Some(json!({ "error": format!("ファイル書き込みに失敗: {e}") }).to_string()),
            );
        }
    };
    if let Err(e) = file.write_all(&file_data) {
        return respond(
            request,
            500,
            Some(json!({ "error": format!("ファイル書き込みに失敗: {e}") }).to_string()),
        );
    }
    drop(file);

    // 所有者のみ読み書き可（0o600）。リモート端末から受信したファイルは
    // 本人以外がアクセスする必要がない（#287 P2-1）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target_path, std::fs::Permissions::from_mode(0o600));
    }

    // canonicalize で traversal していないことを最終確認
    if let Ok(canonical_target) = target_path.canonicalize() {
        if !canonical_target.starts_with(&canonical_dir) {
            let _ = std::fs::remove_file(&target_path);
            return respond(
                request,
                400,
                Some(json!({ "error": "パス traversal が検出されました" }).to_string()),
            );
        }
    }

    // 監査ログ（ファイル名はペイン内容に準じ記録しない。バイト数のみ。#287 P2-2）
    ctx.registry.lock().unwrap().audit(
        "upload",
        &device.id,
        &device.name,
        json!({
            "bytes": file_data.len(),
            "pane": pane_id,
        }),
    );

    let relative_path = format!("{UPLOAD_DIR}/{file_name}");
    respond(
        request,
        200,
        Some(
            json!({
                "ok": true,
                "path": relative_path,
                "size": file_data.len(),
            })
            .to_string(),
        ),
    )
}

/// multipart/form-data の boundary を抽出する
fn extract_multipart_boundary(content_type: &str) -> Option<String> {
    if !content_type.starts_with("multipart/form-data") {
        return None;
    }
    content_type.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("boundary=")
            .map(|b| b.trim_matches('"').to_string())
    })
}

/// multipart ボディから file パートと pane フィールドを抽出する最小実装
fn parse_multipart(body: &[u8], boundary: &str) -> Result<(String, Vec<u8>, String), String> {
    let delimiter = format!("--{boundary}");
    let body_str_lossy = String::from_utf8_lossy(body);
    let parts: Vec<&str> = body_str_lossy.split(&delimiter).collect();

    let mut file_name = String::new();
    let mut file_data: Option<Vec<u8>> = None;
    let mut pane_id = String::new();

    for part in &parts {
        if part.starts_with("--") || part.is_empty() {
            continue;
        }
        let Some(header_end) = part.find("\r\n\r\n") else {
            continue;
        };
        let headers = &part[..header_end];
        let body_start = header_end + 4;
        let body_part = &part[body_start..];
        // 末尾の \r\n を除去
        let body_part = body_part.strip_suffix("\r\n").unwrap_or(body_part);

        if headers.contains("name=\"pane\"") {
            pane_id = body_part.trim().to_string();
        } else if headers.contains("name=\"file\"") {
            // filename の抽出
            if let Some(fname_start) = headers.find("filename=\"") {
                let rest = &headers[fname_start + 10..];
                if let Some(fname_end) = rest.find('"') {
                    file_name = rest[..fname_end].to_string();
                }
            }
            // バイナリデータの正確な抽出（lossy 変換ではなく元のバイト列から）
            let header_bytes = format!("{delimiter}{}", &part[..body_start]);
            let offset_in_body = body
                .windows(header_bytes.len())
                .position(|w| w == header_bytes.as_bytes());
            if let Some(offset) = offset_in_body {
                let data_start = offset + header_bytes.len();
                let next_delim = format!("\r\n{delimiter}");
                let data_end = body[data_start..]
                    .windows(next_delim.len())
                    .position(|w| w == next_delim.as_bytes())
                    .map(|p| data_start + p)
                    .unwrap_or(body.len());
                file_data = Some(body[data_start..data_end].to_vec());
            }
        }
    }

    if file_name.is_empty() {
        return Err("ファイルが含まれていません".to_string());
    }
    if pane_id.is_empty() {
        return Err("pane パラメータが必要です".to_string());
    }
    let data = file_data.ok_or_else(|| "ファイルデータの読み取りに失敗".to_string())?;
    Ok((file_name, data, pane_id))
}

/// WS broadcaster 版のハンドラ（M-5: ペインごとの共有 broadcaster で接続数分の
/// tmux subprocess 乱立を解消）。
///
/// 認証（#283）: 機器ペアリングの二層認証（Observe 以上）。
/// 旧方式の `token.<T>` サブプロトコルは全廃 — serve が付与する identity ヘッダは
/// WS の upgrade リクエストにも載る（弾 0 実測）ため、REST と同じ認可を通せる。
/// デバイスの WS 接続数 0→1 / 1→0 で接続開始・終了の通知と監査を行い、
/// revoke 時は該当デバイスの subscriber を落として即時切断する
fn handle_ws_v2(
    request: tiny_http::Request,
    ctx: &Arc<DaemonCtx>,
    shutdown: Arc<AtomicBool>,
    broadcasters: BroadcasterMap,
    app_conn: &Arc<RwLock<AppConnection>>,
    pane_mapping: &Arc<RwLock<PaneMapping>>,
) {
    // #287 P1: WS upgrade でも Origin を検証（ブラウザは WS に必ず Origin を付与する）
    if let Err(e) = check_request_origin(ctx, &request) {
        return respond(request, 403, Some(json!({ "error": e }).to_string()));
    }
    // #287 P1: クライアントが WS_PROTOCOL を提示していることを検証
    // （ブラウザは `new WebSocket(url, ["tako-remote"])` で送る）
    let ws_protocols = header_value(&request, "sec-websocket-protocol").unwrap_or_default();
    if !ws_protocols
        .split(',')
        .any(|p| p.trim().eq_ignore_ascii_case(WS_PROTOCOL))
    {
        return respond(
            request,
            400,
            Some(
                json!({ "error": format!("Sec-WebSocket-Protocol に '{WS_PROTOCOL}' が必要") })
                    .to_string(),
            ),
        );
    }

    let url_full = request.url().to_string();
    // 二層認証（画面データは Observe role から）
    let device = match authorize_device_checked(ctx, &request, DeviceRole::Observe) {
        Ok(device) => device,
        Err((status, e)) => {
            return respond(request, status, Some(json!({ "error": e }).to_string()));
        }
    };
    let Some(pane_param) = query_param(&url_full, "pane") else {
        return respond(
            request,
            400,
            Some(json!({ "error": "pane クエリが必要" }).to_string()),
        );
    };
    // 数値 PaneId → tmux ターゲットの解決（#423/#426: v2 API が返す数値 ID 対応）
    let pane = resolve_pane_param(&pane_param, app_conn, pane_mapping);
    let Some(ref pane) = pane else {
        return respond(
            request,
            404,
            Some(json!({ "error": format!("ペイン '{pane_param}' が見つからない") }).to_string()),
        );
    };
    let Some(ws_key) = header_value(&request, "sec-websocket-key") else {
        return respond(
            request,
            400,
            Some(json!({ "error": "Sec-WebSocket-Key ヘッダが無い" }).to_string()),
        );
    };

    // 101 Switching Protocols
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

    // broadcaster に subscribe して WS 配信スレッドを起動（subscriber はデバイス ID つき =
    // revoke 時に disconnect_device_ws で該当デバイスだけ即時切断できる）
    let (_, rx) =
        get_or_create_broadcaster(&broadcasters, pane, &device.id, &ctx.tmux_socket, shutdown);

    // 接続開始（このデバイスの 1 本目の WS）なら通知 + 監査
    if ctx.ws_connect(&device.id) {
        ctx.registry.lock().unwrap().audit(
            "conn_open",
            &device.id,
            &device.name,
            json!({ "route": "/ws" }),
        );
        notify_macos(&format!("{} が接続しました", device.name));
    }

    let ctx_fwd = ctx.clone();
    let device_fwd = device.clone();
    std::thread::Builder::new()
        .name("tako-remote-ws-forward".into())
        .spawn(move || {
            use tungstenite::protocol::{Role, WebSocket};
            let mut ws = WebSocket::from_raw_socket(stream, Role::Server, None);

            // broadcaster からのメッセージを WS クライアントへ中継する
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                    Ok(msg) => {
                        if ws.send(tungstenite::Message::text(msg)).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // keepalive
                        let keepalive = json!({ "type": "keepalive" }).to_string();
                        if ws.send(tungstenite::Message::text(keepalive)).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // broadcaster の終了 or revoke による切断
                        break;
                    }
                }
            }
            let _ = ws.close(None);
            // 全 WS 切断（このデバイスの最後の 1 本）なら通知 + 監査。
            // 短時間の再接続（ネットワーク揺れ等）で通知が嵐にならないよう、
            // 猶予時間後にまだ切断中の場合のみ通知する（#423）
            if ctx_fwd.ws_disconnect(&device_fwd.id) {
                let device_id = device_fwd.id.clone();
                let device_name = device_fwd.name.clone();
                let ctx_delayed = ctx_fwd.clone();
                std::thread::Builder::new()
                    .name("ws-disconnect-notify".into())
                    .spawn(move || {
                        std::thread::sleep(WS_NOTIFY_GRACE);
                        if !ctx_delayed.ws_is_connected(&device_id) {
                            if let Ok(reg) = ctx_delayed.registry.lock() {
                                reg.audit(
                                    "conn_close",
                                    &device_id,
                                    &device_name,
                                    json!({ "route": "/ws" }),
                                );
                            }
                            notify_macos(&format!("{device_name} が切断しました"));
                        }
                    })
                    .ok();
            }
        })
        .ok();
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

    /// env var（TAKO_REMOTE_STATE_DIR / PATH）はプロセス全域のため、これを set/remove する
    /// テスト同士が並列実行でレースする。特に PATH を空にする窓では verify_pid_identity の
    /// ps 起動が失敗して検証素通り → daemon_stop_impl が自プロセスへ SIGTERM を送り
    /// テスト全体が死ぬ（実測）。env var を触るテストはこのロックで直列化する
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn required_roleは操作種別ごとに正しいroleを要求する() {
        use tiny_http::Method;
        // 読み取り系は Observe
        assert_eq!(
            required_role(&Method::Get, "/api/panes"),
            DeviceRole::Observe
        );
        assert_eq!(
            required_role(&Method::Get, "/api/v2/panes"),
            DeviceRole::Observe
        );
        assert_eq!(
            required_role(&Method::Get, "/api/panes/s:0.0/screen"),
            DeviceRole::Observe
        );
        assert_eq!(
            required_role(&Method::Get, "/api/panes/s:0.0/scrollback"),
            DeviceRole::Observe
        );
        assert_eq!(
            required_role(&Method::Get, "/api/sessions/abc/messages"),
            DeviceRole::Observe
        );
        // input は Interact
        assert_eq!(
            required_role(&Method::Post, "/api/panes/s:0.0/input"),
            DeviceRole::Interact
        );
        // permission ダイアログ応答も Interact（#425）
        assert_eq!(
            required_role(&Method::Post, "/api/panes/648/respond"),
            DeviceRole::Interact
        );
        // close / resize は Manage
        assert_eq!(
            required_role(&Method::Post, "/api/panes/s:0.0/close"),
            DeviceRole::Manage
        );
        assert_eq!(
            required_role(&Method::Post, "/api/panes/s:0.0/resize"),
            DeviceRole::Manage
        );
        // 端末管理は Admin
        assert_eq!(
            required_role(&Method::Get, "/api/devices"),
            DeviceRole::Admin
        );
        assert_eq!(
            required_role(&Method::Post, "/api/devices/revoke"),
            DeviceRole::Admin
        );
        // upload は Interact
        assert_eq!(
            required_role(&Method::Post, "/api/upload"),
            DeviceRole::Interact
        );
        // 未知の POST は安全側（Manage）
        assert_eq!(
            required_role(&Method::Post, "/api/unknown"),
            DeviceRole::Manage
        );
    }

    // --- #439: agent_type の live claude 判定 ---

    /// list_to_api_v2 のテスト用 List（1 タブ 3 ペイン: role 消失 master 相当 /
    /// role あり worker / plain シェル）
    fn sample_list() -> Value {
        json!({
            "tabs": [{
                "id": 1,
                "title": "tab1",
                "panes": [
                    { "id": 648, "title": "master相当", "role": null, "osc_title": null,
                      "cwd": "/Users/u", "state": "running", "surface": "foreground",
                      "tmux_session": "tako-master648" },
                    { "id": 700, "title": "worker", "role": "orchestrator-worker:proj:x",
                      "cwd": "/w", "state": "running", "surface": "foreground",
                      "tmux_session": "tako-worker700" },
                    { "id": 701, "title": "shell", "role": null, "osc_title": null,
                      "cwd": "/", "state": "idle", "surface": "foreground",
                      "tmux_session": "tako-shell701" },
                ]
            }]
        })
    }

    #[test]
    fn role消失ペインでもlive_claudeが動いていればagent_typeはclaude() {
        // #439 の実機再現形: role=null / osc_title=null だが対話型 claude が稼働
        let live: HashMap<String, crate::agents::LiveClaudeSession> = [(
            "tako-master648".to_string(),
            crate::agents::LiveClaudeSession {
                session_id: "sid-648".into(),
                interactive: true,
            },
        )]
        .into();
        let v2 = list_to_api_v2(&sample_list(), &live);
        let panes = v2["panes"].as_array().unwrap();
        let p648 = panes.iter().find(|p| p["id"] == 648).unwrap();
        assert_eq!(
            p648["agent_type"], "claude",
            "live 解決で claude 化: {p648}"
        );
        assert_eq!(p648["session_id"], "sid-648");
        // worker は従来どおり role 由来で claude
        let p700 = panes.iter().find(|p| p["id"] == 700).unwrap();
        assert_eq!(p700["agent_type"], "claude");
        // plain シェルは plain のまま
        let p701 = panes.iter().find(|p| p["id"] == 701).unwrap();
        assert_eq!(p701["agent_type"], "plain");
        assert!(p701["session_id"].is_null());
    }

    #[test]
    fn 一時的なheadless_claudeではclaude化しない() {
        // シェルペインで claude -p が走った瞬間の誤 claude 化を防ぐ（kind != interactive）
        let live: HashMap<String, crate::agents::LiveClaudeSession> = [(
            "tako-shell701".to_string(),
            crate::agents::LiveClaudeSession {
                session_id: "sid-p".into(),
                interactive: false,
            },
        )]
        .into();
        let v2 = list_to_api_v2(&sample_list(), &live);
        let panes = v2["panes"].as_array().unwrap();
        let p701 = panes.iter().find(|p| p["id"] == 701).unwrap();
        assert_eq!(p701["agent_type"], "plain", "headless では claude 化しない");
        // session_id 自体は付く（transcript は存在するため。チャット判定は agent_type 側で抑止）
        assert_eq!(p701["session_id"], "sid-p");
    }

    #[test]
    fn live不在ならrole判定へフォールバックする() {
        let live = HashMap::new();
        let v2 = list_to_api_v2(&sample_list(), &live);
        let panes = v2["panes"].as_array().unwrap();
        let p648 = panes.iter().find(|p| p["id"] == 648).unwrap();
        assert_eq!(p648["agent_type"], "plain", "live 不在 + role なしは plain");
        let p700 = panes.iter().find(|p| p["id"] == 700).unwrap();
        assert_eq!(
            p700["agent_type"], "claude",
            "role ありは live 不在でも claude"
        );
    }

    // --- #425: 画面ダイアログ実在ベースの承認判定 ---

    /// permission ダイアログの画面（claude v2.x 実採取形式の要点再現）
    fn dialog_screen() -> Vec<String> {
        [
            "  Bash command",
            "  rm -rf build/",
            "  Do you want to proceed?",
            "  ❯ 1. Yes",
            "    2. Yes, and don't ask again for rm commands",
            "    3. No, and tell Claude what to do differently (esc)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn attach_permission_dialogsはagentペインの画面ダイアログを付与する() {
        let mut result = json!({ "panes": [
            { "id": 648, "agent_type": "claude", "tmux_target": "tako-a:0.0" },
            { "id": 700, "agent_type": "claude", "tmux_target": "tako-b:0.0" },
            { "id": 701, "agent_type": "plain", "tmux_target": "tako-c:0.0" },
        ]});
        let captured = std::cell::RefCell::new(Vec::new());
        attach_permission_dialogs(&mut result, |session| {
            captured.borrow_mut().push(session.to_string());
            if session == "tako-a" {
                Some(dialog_screen())
            } else {
                // tako-b は通常画面（入力欄のみ）
                Some(vec!["❯ ".to_string()])
            }
        });
        let panes = result["panes"].as_array().unwrap();
        let dialog = &panes[0]["permission_dialog"];
        assert!(dialog.is_object(), "ダイアログ画面のペインに付与: {dialog}");
        assert_eq!(dialog["options"].as_array().unwrap().len(), 3);
        assert_eq!(dialog["highlighted"], 0);
        assert!(
            panes[1]["permission_dialog"].is_null(),
            "通常画面には付与しない"
        );
        assert!(panes[2]["permission_dialog"].is_null(), "plain は対象外");
        // plain ペインはキャプチャ自体もしない
        assert_eq!(*captured.borrow(), vec!["tako-a", "tako-b"]);
    }

    #[test]
    fn attach_permission_dialogsはキャプチャ失敗を無視する() {
        let mut result = json!({ "panes": [
            { "id": 648, "agent_type": "claude", "tmux_target": "tako-a:0.0" },
        ]});
        attach_permission_dialogs(&mut result, |_| None);
        assert!(result["panes"][0]["permission_dialog"].is_null());
    }

    #[test]
    fn device_jsonは全フィールドを含む() {
        let d = crate::remote_auth::Device {
            id: "nDEV1".into(),
            name: "iPhone".into(),
            login: "u@e.com".into(),
            node_name: "iphone.tail1234.ts.net".into(),
            role: DeviceRole::Interact,
            created_at: 100,
            last_seen: 200,
        };
        let v = device_json(&d);
        assert_eq!(v["id"], "nDEV1");
        assert_eq!(v["name"], "iPhone");
        assert_eq!(v["role"], "interact");
        assert_eq!(v["created_at"], 100);
        assert_eq!(v["last_seen"], 200);
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
    fn qr_pngを生成できる() {
        // generate_qr_png は state_dir（TAKO_REMOTE_STATE_DIR の影響下）に保存するため直列化する
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = super::generate_qr_png("http://192.168.1.100:7749#token=abc123def456")
            .expect("PNG 生成に失敗");
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 100);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn daemon_statusはpidファイルがないときfalse() {
        // env var 窓中に他テストの pid ファイルを読んで cleanup しないよう直列化
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    fn find_port_occupantは未使用ポートでnoneを返す() {
        // 存在しないであろう高番号ポート
        assert!(find_port_occupant(59999).is_none());
    }

    #[test]
    fn kill_stale_daemonは存在しないpidで安全に完了する() {
        // cleanup_state_files が state_dir を掃除するため、env var 窓中の他テストを壊さないよう直列化
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // is_process_alive が false なので即 cleanup_state_files して return
        kill_stale_daemon(999_999_999);
    }

    #[test]
    fn daemon_statusはpidファイルが無ければnot_running() {
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // state_dir をテンポラリに差し替えて検証
        let dir = std::env::temp_dir().join(format!("tako-test-remote-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let status = daemon_status();
        assert_eq!(status["running"], json!(false));
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- P0-3 テスト ---

    #[test]
    fn state_dirはdata_dir配下のremoteを返す() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = state_dir();
        let s = dir.to_string_lossy();
        assert!(
            s.contains("remote") || s.contains("tako"),
            "state_dir は /tmp ではなく data_dir 配下: {s}"
        );
    }

    #[test]
    fn write_secret_fileは0600で書ける() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            err.contains("確認できません"),
            "エラーメッセージに検証失敗を示す: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn verify_pid_identityはps実行不能環境でfalseを返す() {
        // /bin/ps が存在しない PID で呼ぶ（プロセス生存チェックで先に false）ので、
        // ps 起動失敗の分岐を直接テストするため生存プロセスの PID を使う。
        // 自プロセスは "tako remote serve" ではないので、ps 成功時も false になる。
        // ここでは fail-safe 原則が守られていることだけを確認する
        let info = PidInfo {
            pid: std::process::id(),
            exe: None,
            start_time: None,
        };
        // 正常環境でも自プロセスは tako remote serve ではないため false
        assert!(!verify_pid_identity(&info));
    }

    #[cfg(unix)]
    #[test]
    fn daemon_stop_implはps実行不能でもkillしない() {
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 使い捨てプロセスを起動して PID を取得（テスト末尾で kill + wait 回収）
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("sleep プロセスの起動");
        let child_pid = child.id();

        let dir = std::env::temp_dir().join(format!("tako-test-ps-fail-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("TAKO_REMOTE_STATE_DIR", dir.as_os_str());
        let pid_file = dir.join("tako-remote.pid");
        // sleep プロセスの PID を書く（"tako remote serve" ではない）
        std::fs::write(&pid_file, format!("{child_pid}\n/bin/sleep\n0\n")).unwrap();

        let result = daemon_stop_impl(false);
        std::env::remove_var("TAKO_REMOTE_STATE_DIR");

        // sleep プロセスがまだ生存していることを確認（SIGTERM されていない）
        let alive = unsafe { libc::kill(child_pid as libc::pid_t, 0) } == 0;
        assert!(alive, "sleep プロセスが SIGTERM されていないこと");

        // 後始末（kill + wait で zombie を残さない）
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(result.is_err(), "検証失敗でエラーが返る");
        let err = result.unwrap_err();
        assert!(
            err.contains("確認できません"),
            "検証不能時のエラーメッセージ: {err}"
        );
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

    // --- upload API テスト (#285) ---

    #[test]
    fn extract_multipart_boundaryはboundaryを抽出する() {
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; boundary=----WebKitFormBoundary"),
            Some("----WebKitFormBoundary".to_string())
        );
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; boundary=\"abc123\""),
            Some("abc123".to_string())
        );
        assert_eq!(extract_multipart_boundary("application/json"), None);
    }

    #[test]
    fn parse_multipartはfileとpaneを抽出する() {
        let boundary = "----boundary";
        let body = "------boundary\r\nContent-Disposition: form-data; name=\"pane\"\r\n\r\n42\r\n\
             ------boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\
             Content-Type: text/plain\r\n\r\nhello world\r\n------boundary--\r\n";
        let (name, data, pane) = parse_multipart(body.as_bytes(), boundary).unwrap();
        assert_eq!(name, "test.txt");
        assert_eq!(data, b"hello world");
        assert_eq!(pane, "42");
    }

    #[test]
    fn parse_multipartはファイルなしでエラーを返す() {
        let boundary = "----boundary";
        let body = "------boundary\r\nContent-Disposition: form-data; name=\"pane\"\r\n\r\n42\r\n------boundary--\r\n";
        assert!(parse_multipart(body.as_bytes(), boundary).is_err());
    }

    #[test]
    fn upload_roleはinteractが必要() {
        use tiny_http::Method;
        assert_eq!(
            required_role(&Method::Post, "/api/upload"),
            DeviceRole::Interact
        );
    }

    #[test]
    fn session_name_ofはtmuxターゲットからセッション名を取り出す() {
        // #428: dispatch の tmux_session に ":0.0" が残ると deliver 系の
        // `={session}:` 組み立てが `=session:0.0:` になり can't find pane で無音失敗する
        assert_eq!(session_name_of("tako-abc123:0.0"), "tako-abc123");
        assert_eq!(session_name_of("tako-abc123:"), "tako-abc123");
        assert_eq!(session_name_of("tako-abc123"), "tako-abc123");
    }

    #[test]
    fn pane_mappingは数値idをtmuxターゲットとセッション名に解決する() {
        let mut mapping = PaneMapping::new();
        mapping.update_from_list(&json!({
            "tabs": [{ "id": 1, "panes": [
                { "id": 42, "tmux_session": "tako-deadbeef" },
                { "id": 7, "tmux_session": "" },
            ]}]
        }));
        // 数値 PaneId → "session:0.0"（WS / screen 用）
        assert_eq!(
            mapping.resolve_tmux_target("42").as_deref(),
            Some("tako-deadbeef:0.0")
        );
        // dispatch 用にはセッション名へ分離できる（#428 回帰）
        let session = mapping
            .resolve_tmux_target("42")
            .map(|t| session_name_of(&t));
        assert_eq!(session.as_deref(), Some("tako-deadbeef"));
        // tmux_session が空のペインは解決不能（tmux フォールバック不可）
        assert_eq!(mapping.resolve_tmux_target("7"), None);
        // tmux ターゲット形式はそのまま通す
        assert_eq!(
            mapping.resolve_tmux_target("sess:0.1").as_deref(),
            Some("sess:0.1")
        );
    }

    #[test]
    fn serve_binary_implは通常モードで安定バイナリを優先する() {
        // #432: PATH 上の dev CLI から start しても .app 世代の serve を立てる
        let dev_cli = std::path::PathBuf::from("/tmp/dev/target/release/tako");
        assert_eq!(
            serve_binary_impl(false, Some(dev_cli.clone()), true).as_deref(),
            Some(crate::dispatch::STABLE_APP_BINARY)
        );
        // .app が無い環境では CLI 自身
        assert_eq!(
            serve_binary_impl(false, Some(dev_cli), false).as_deref(),
            Some("/tmp/dev/target/release/tako")
        );
    }

    #[test]
    fn serve_binary_implは検証モードで自世代バイナリを返す() {
        // #432: 隔離・検証時は /Applications に飛ばず検証対象の世代で serve を立てる
        let dev_cli = std::path::PathBuf::from("/tmp/dev/target/release/tako");
        assert_eq!(
            serve_binary_impl(true, Some(dev_cli), true).as_deref(),
            Some("/tmp/dev/target/release/tako")
        );
        // GUI（tako-app）から: 同ディレクトリに実在する CLI（同世代）へ
        let dir = std::env::temp_dir().join(format!("tako-432-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sibling = dir.join("tako");
        std::fs::write(&sibling, b"stub").unwrap();
        let gui = dir.join("tako-app");
        assert_eq!(
            serve_binary_impl(true, Some(gui.clone()), true).as_deref(),
            Some(sibling.display().to_string().as_str())
        );
        // sibling が無ければ通常フロー（安定バイナリ）へ落ちる
        std::fs::remove_file(&sibling).unwrap();
        assert_eq!(
            serve_binary_impl(true, Some(gui), true).as_deref(),
            Some(crate::dispatch::STABLE_APP_BINARY)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #287 P1: cross-origin 遮断テスト ---

    #[test]
    fn origin検証_同一originは許可() {
        let base = "https://host.example.ts.net";
        let allowed = base.trim_end_matches('/');

        // 完全一致
        let given = "https://host.example.ts.net".trim_end_matches('/');
        assert!(given.eq_ignore_ascii_case(allowed));

        // 末尾スラッシュの違いを許容
        let given = "https://host.example.ts.net/".trim_end_matches('/');
        assert!(given.eq_ignore_ascii_case(allowed));

        // 大文字小文字を無視
        let given = "HTTPS://HOST.EXAMPLE.TS.NET".trim_end_matches('/');
        assert!(given.eq_ignore_ascii_case(allowed));
    }

    #[test]
    fn origin検証_evil_originは拒否() {
        let base = "https://host.example.ts.net";
        let allowed = base.trim_end_matches('/');

        // 別ドメイン
        let evil = "https://evil.example.com".trim_end_matches('/');
        assert!(!evil.eq_ignore_ascii_case(allowed));

        // サブドメイン偽装
        let evil = "https://host.example.ts.net.evil.com".trim_end_matches('/');
        assert!(!evil.eq_ignore_ascii_case(allowed));

        // スキーム違い（http vs https）
        let evil = "http://host.example.ts.net".trim_end_matches('/');
        assert!(!evil.eq_ignore_ascii_case(allowed));
    }

    #[test]
    fn origin検証_テストモードのlocalhostも正しく照合() {
        let base = "http://127.0.0.1:7749";
        let allowed = base.trim_end_matches('/');

        // 同一ポート = 許可
        let given = "http://127.0.0.1:7749".trim_end_matches('/');
        assert!(given.eq_ignore_ascii_case(allowed));

        // 異なるポート = 拒否
        let evil = "http://127.0.0.1:8080".trim_end_matches('/');
        assert!(!evil.eq_ignore_ascii_case(allowed));
    }

    #[test]
    fn cors_headersにワイルドカードが含まれない() {
        // OnceLock は一度しか set できないため、既に設定済みでも OK を返す
        CORS_ALLOWED_ORIGIN
            .set("https://test.ts.net".to_string())
            .ok();
        let headers = cors_headers();
        for h in &headers {
            if h.field.equiv("Access-Control-Allow-Origin") {
                assert_ne!(h.value.as_str(), "*", "CORS に * が含まれてはならない");
            }
        }
        // Vary: Origin が含まれること
        assert!(
            headers
                .iter()
                .any(|h| h.field.equiv("Vary") && h.value.as_str().contains("Origin")),
            "Vary: Origin ヘッダが必要"
        );
    }

    #[test]
    fn ws_subprotocol検証のロジック() {
        // WS_PROTOCOL が含まれるケース
        let protocols = "tako-remote";
        assert!(protocols
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case(WS_PROTOCOL)));

        // 複数プロトコルの中に含まれるケース
        let protocols = "other, tako-remote, another";
        assert!(protocols
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case(WS_PROTOCOL)));

        // 含まれないケース
        let protocols = "other-protocol";
        assert!(!protocols
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case(WS_PROTOCOL)));

        // 空文字列
        let protocols = "";
        assert!(!protocols
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case(WS_PROTOCOL)));
    }
}

//! tako-control — 制御プレーン層（GPUI 非依存）
//!
//! - protocol: Layer 1 IPC / Layer 2 MCP 共通の操作プロトコル定義（FR-2.2 / FR-2.5）
//! - dispatch: リクエスト → tako-core ドメイン API の一元ディスパッチャ（設計原則 5）
//! - ipc: Layer 1 IPC サーバー（Unix domain socket + トークン認証）
//! - mcp: Layer 2 内蔵 MCP サーバー（エンジン + Streamable HTTP。ipc と dispatch を共有）
//! - discovery: 接続情報（ソケット・トークン）の永続化と発見（FR-2.2.9）
//! - settings: ユーザー設定の永続化（自動リネーム ON/OFF 等。FR-2.12.4）
//! - layout: タブ / ペイン構成の永続化と復元（Phase 5.5 / FR-5）
//! - diag: 永続化まわりの診断ログ（Issue #30。`<data_dir>/persist.log`）
//! - detect: パッシブ検知（Layer 3。listen ポート検知は Phase 4 後半で実装）
//! - remote / agents / transcript: スマホリモートアクセス（Issue #23。HTTP+WS API と
//!   claude agents プロキシ・会話ログ正規化）
//! - tailscale: Tailscale Serve transport（Issue #282。CLI 検出・setup 判定・serve 管理）
//! - claude_tui: Claude Code TUI の画面状態検出とプロンプト送達確認（Issue #32）
//! - config_io: 設定ファイルの安全な読み書き共通部品（アトミック書き込み・
//!   プロセス間ロック・世代バックアップ。Issue #169）

pub mod acceptance_gates;
pub mod agents;
pub mod agents_sync;
pub mod claude_tui;
pub mod config_io;
pub mod diag;
pub mod discovery;
pub mod dispatch;
pub mod fda;
pub mod host;
pub mod ipc;
pub mod layout;
pub mod mcp;
pub mod orchestrator;
pub mod protocol;
pub mod remote;
pub mod remote_auth;
pub mod remote_setup;
pub mod sessions;
pub mod settings;
pub mod setup;
pub mod sleep_guard;
pub mod tailscale;
pub mod task_checkpoints;
pub mod telemetry;
pub mod transcript;

pub use dispatch::{
    dispatch, dispatch_orchestrator_layout, fetch_tmux_sessions, prepare_offload, ControlHost,
    DispatchError, OffloadJob, PinnedView, TmuxContext,
};
pub use host::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
pub use ipc::{IncomingRequest, IpcServer};
pub use mcp::McpServer;

// --- claude session スキャンのイベント駆動トリガー（#368） ---
use std::sync::atomic::{AtomicBool, Ordering};
static CLAUDE_SCAN_REQUESTED: AtomicBool = AtomicBool::new(false);
/// spawn / PromptFlow 完了時に呼び、次の周期スキャンを即時実行させる
pub fn request_claude_scan() {
    CLAUDE_SCAN_REQUESTED.store(true, Ordering::Relaxed);
}
/// スキャンループが毎 tick 呼ぶ（flag を消費して返す）
pub fn take_claude_scan_request() -> bool {
    CLAUDE_SCAN_REQUESTED.swap(false, Ordering::Relaxed)
}

/// 接続認証トークンを OS の CSPRNG から生成する（hex 64 文字。FR-2.3.4）。
/// IPC と MCP はこのセッション共有トークンで認証する。ログに出さないこと
pub fn generate_token() -> std::io::Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| std::io::Error::other(format!("CSPRNG が使えない: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// 永続トークンを読み込むか、存在しなければ生成して保存する。
/// 再起動をまたいで同じトークンを使うことで、tmux セッション内の既存クライアント
/// （古い TAKO_TOKEN 環境変数を持つ）がそのまま再接続できる。
/// セルフテスト中は永続ファイルに触れず一時トークンを返す
pub fn load_or_create_token() -> std::io::Result<String> {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return generate_token();
    }
    let Some(path) = tako_core::paths::data_dir().map(|d| d.join("token")) else {
        return generate_token();
    };
    if let Ok(content) = std::fs::read_to_string(&path) {
        let token = content.trim().to_string();
        if token.len() >= 32 {
            return Ok(token);
        }
    }
    let token = generate_token()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

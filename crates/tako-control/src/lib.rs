//! tako-control — 制御プレーン層（GPUI 非依存）
//!
//! - protocol: Layer 1 IPC / Layer 2 MCP 共通の操作プロトコル定義（FR-2.2 / FR-2.5）
//! - dispatch: リクエスト → tako-core ドメイン API の一元ディスパッチャ（設計原則 5）
//! - ipc: Layer 1 IPC サーバー（Unix domain socket + トークン認証）
//! - mcp: Layer 2 内蔵 MCP サーバー（エンジン + Streamable HTTP。ipc と dispatch を共有）
//! - discovery: 接続情報（ソケット・トークン）の永続化と発見（FR-2.2.9）
//! - settings: ユーザー設定の永続化（自動リネーム ON/OFF 等。FR-2.12.4）
//! - layout: タブ / ペイン構成の永続化と復元（Phase 5.5 / FR-5）
//! - detect: パッシブ検知（Layer 3。listen ポート検知は Phase 4 後半で実装）

pub mod discovery;
pub mod dispatch;
pub mod ipc;
pub mod layout;
pub mod mcp;
pub mod protocol;
pub mod settings;

pub use dispatch::{dispatch, fetch_tmux_sessions, ControlHost, DispatchError, TmuxContext};
pub use ipc::{IncomingRequest, IpcServer};
pub use mcp::McpServer;

/// 接続認証トークンを OS の CSPRNG から生成する（hex 64 文字。FR-2.3.4）。
/// IPC と MCP はこのセッション共有トークンで認証する。ログに出さないこと
pub fn generate_token() -> std::io::Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| std::io::Error::other(format!("CSPRNG が使えない: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

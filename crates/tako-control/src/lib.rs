//! tako-control — 制御プレーン層（GPUI 非依存）
//!
//! - protocol: Layer 1 IPC / Layer 2 MCP 共通の操作プロトコル定義（FR-2.2 / FR-2.5）
//! - dispatch: リクエスト → tako-core ドメイン API の一元ディスパッチャ（設計原則 5）
//! - ipc: Layer 1 IPC サーバー（Unix domain socket + トークン認証）
//! - mcp: Layer 2 内蔵 MCP サーバー（エンジン + Streamable HTTP。ipc と dispatch を共有）
//! - detect: パッシブ検知（Layer 3。OSC 7/133・listen ポート検知。Phase 4 で実装）

pub mod dispatch;
pub mod ipc;
pub mod mcp;
pub mod protocol;

pub use dispatch::{dispatch, ControlHost, DispatchError};
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

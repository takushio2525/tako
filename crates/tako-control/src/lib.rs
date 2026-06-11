//! tako-control — 制御プレーン層（GPUI 非依存）
//!
//! - protocol: Layer 1 IPC / Layer 2 MCP 共通の操作プロトコル定義（FR-2.2 / FR-2.5）
//! - dispatch: リクエスト → tako-core ドメイン API の一元ディスパッチャ（設計原則 5）
//! - ipc: Layer 1 IPC サーバー（Unix domain socket + トークン認証）
//! - mcp: 内蔵 MCP サーバー（Layer 2。Phase 3 で実装。ipc と dispatch を共有する）
//! - detect: パッシブ検知（Layer 3。OSC 7/133・listen ポート検知。Phase 4 で実装）

pub mod dispatch;
pub mod ipc;
pub mod protocol;

pub use dispatch::{dispatch, ControlHost, DispatchError};
pub use ipc::{IncomingRequest, IpcServer};

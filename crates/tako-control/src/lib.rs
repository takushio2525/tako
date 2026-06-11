//! tako-control — 制御プレーン層（GPUI 非依存）
//!
//! Phase 2 以降で実装する:
//! - ipc: IPC サーバー（Layer 1 CLI の受け口。Unix domain socket / named pipe + JSON-RPC）
//! - mcp: 内蔵 MCP サーバー（Layer 2。操作セットは FR-2.5、ipc と実装を共有）
//! - detect: パッシブ検知（Layer 3。OSC 7/133・listen ポート検知）
//!
//! 現時点はワークスペース構成を確定させるためのプレースホルダ。

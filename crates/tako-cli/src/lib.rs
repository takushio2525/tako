//! `tako` CLI と、MCP の stdio ブリッジ専用の小さな実行ファイル `tako-mcp`（#2021）が
//! 共有する部品。
//!
//! `tako-mcp` は claude 1 本につき 1 本常駐するブリッジの footprint を小さくするための
//! 実行ファイルで（`src/bin/tako-mcp.rs`）、clap もコマンドの実装も持たない。
//! ブリッジと IPC クライアントはどちらの実行ファイルでも同じ 1 実装を通る

pub mod ipc;
pub mod mcp_bridge;

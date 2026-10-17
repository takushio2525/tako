//! `tako` CLI と、MCP の stdio ブリッジ専用の小さな実行ファイル `tako-mcp`（#2021）が
//! 共有する部品。
//!
//! `tako-mcp` は claude 1 本につき 1 本常駐するブリッジの footprint を小さくするための
//! 実行ファイルで（`src/bin/tako-mcp.rs`）、clap もコマンドの実装も持たない。
//! ブリッジと IPC クライアントはどちらの実行ファイルでも同じ 1 実装を通る

// 出力マクロ（`eprintln!` 等）を std の版から差し替える。**他の `mod` より先に置く**
// （`main.rs` と同じ `stdio.rs`。理由は `tako_core::stdio` の先頭 = #1758 / #1805）
#[macro_use]
mod stdio;

pub mod ipc;
pub mod mcp_bridge;

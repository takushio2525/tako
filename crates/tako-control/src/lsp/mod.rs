//! 言語サーバ（LSP）クライアント（#1678 / エピック #1007 の S1）
//!
//! プロセス・JSON-RPC・文書同期を持つ層。GPUI 非依存のまま（依存方向は既存どおり
//! `tako-app → tako-control → tako-core`）。純粋部分（位置変換・検出表・ルート・状態機械）は
//! `tako_core::lsp`。実装設計の正本は `.agent/plans/2026-09-lsp-s1.md`。
//!
//! S1 が持つのは「言語サーバと会話できる状態」までで、**言語機能は 1 つも出さない**
//! （診断の描画は #1679、定義ジャンプは #1680 …）。公開する操作は
//! `tako lsp status / servers / restart / stop / logs` と MCP `tako_lsp_server` の 1 本。

pub mod diagnostics;
pub mod manager;
pub mod rpc;
pub mod server;
pub mod text;

pub use manager::{legacy, DocLease, DocLink, Launch, LspConfig, LspManager};

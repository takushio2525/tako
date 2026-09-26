//! 言語サーバ（LSP）クライアント（#1678 / エピック #1007 の S1）
//!
//! プロセス・JSON-RPC・文書同期を持つ層。GPUI 非依存のまま（依存方向は既存どおり
//! `tako-app → tako-control → tako-core`）。純粋部分（位置変換・検出表・ルート・状態機械）は
//! `tako_core::lsp`。実装設計の正本は `.agent/plans/2026-09-lsp-s1.md`。
//!
//! S1 が持つのは「言語サーバと会話できる状態」まで（`tako lsp status / servers / restart /
//! stop / logs` と MCP `tako_lsp_server`）。言語機能は各スライスが足し、MCP は `tako_lsp` の
//! `action` に載せる: S2（#1679）の診断 = `tako lsp diagnostics`（受信・変換・保持は
//! [`diagnostics`]、UI へのキューは [`LspManager::diagnostics_events`]）。定義ジャンプは #1680 …

pub mod diagnostics;
pub mod manager;
pub mod rpc;
pub mod server;
pub mod text;

pub use diagnostics::DocDiagnostics;
pub use manager::{
    legacy, DocLease, DocLink, DocumentDiagnostics, Launch, LspConfig, LspDocument, LspManager,
    DIAGNOSTICS_EVENT_CAPACITY,
};

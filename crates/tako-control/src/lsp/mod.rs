//! 言語サーバ（LSP）クライアント（#1678 / エピック #1007 の S1）
//!
//! プロセス・JSON-RPC・文書同期を持つ層。GPUI 非依存のまま（依存方向は既存どおり
//! `tako-app → tako-control → tako-core`）。純粋部分（位置変換・検出表・ルート・状態機械）は
//! `tako_core::lsp`。実装設計の正本は `.agent/plans/2026-09-lsp-s1.md`。
//!
//! S1 が持つのは「言語サーバと会話できる状態」まで（`tako lsp status / servers / restart /
//! stop / logs` と MCP `tako_lsp_server`）。言語機能は各スライスが足し、MCP は `tako_lsp` の
//! `action` に載せる: S2（#1679）の診断 = `tako lsp diagnostics`（受信・変換・保持は
//! [`diagnostics`]、UI へのキューは [`LspManager::diagnostics_events`]）。S3（#1680）の
//! 定義ジャンプ = `tako lsp definition` 等（問い合わせの型と応答は [`goto`]、待つのは
//! [`LspManager::goto`]）。S6（#1683）の整形 = `tako lsp format`（型と応答は [`format`]、
//! 待つのは [`LspManager::format`]、当てるのは `TextBuffer::apply_changes`）。
//! S5（#1682）の補完 = `tako lsp completion`（問い合わせの型と応答は
//! [`completion`]、待つのは [`LspManager::completion`]。古い要求は `$/cancelRequest` で捨てる）。
//! S7（#1684）の右クリックメニュー = `tako lsp menu`（項目 → dispatch の対応は [`menu`]、
//! 能力を読むのは [`LspManager::menu_capabilities_now`] / [`LspManager::menu_capabilities`]）。

pub mod completion;
pub mod diagnostics;
pub mod format;
pub mod goto;
pub mod manager;
pub mod menu;
pub mod rpc;
pub mod server;
pub mod text;

pub use completion::{CompletionAnswer, CompletionError, CompletionRequest};
pub use diagnostics::DocDiagnostics;
pub use format::{FormatAnswer, FormatError, FormatRequest};
pub use goto::{GotoAnswer, GotoError, GotoRequest, GotoTarget};
pub use manager::{
    legacy, DocLease, DocLink, DocumentDiagnostics, Launch, LspConfig, LspDocument, LspManager,
    DIAGNOSTICS_EVENT_CAPACITY,
};
pub use menu::{MenuCapabilities, MenuRequest};

//! 言語サーバ（LSP）クライアントの純粋部分（#1678 / エピック #1007 の S1）
//!
//! プロセスも I/O も持たない部分だけをここに置く。プロセス管理・JSON-RPC・文書同期は
//! `tako_control::lsp`（GPUI 非依存のまま）。実装設計の正本は `.agent/plans/2026-09-lsp-s1.md`。
//!
//! - [`position`] — UTF-8 バイト ⇄ UTF-16 コードユニットの変換（入口は 2 本だけ）
//! - [`servers`] — サーバ検出表（言語の追加 = 表への行追加）
//! - [`root`] — プロジェクトルートの検出
//! - [`state`] — ライフサイクルの状態機械
//! - [`sync`] — `didChange` の中身（送った本文の写しとの差分）

pub mod position;
pub mod root;
pub mod servers;
pub mod state;
pub mod sync;

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
//! - [`diagnostic`] — 診断のモデル（重大度・範囲・出所・コード。#1679）
//! - [`goto`] — 定義ジャンプ（S3 / #1680）の応答の読み取り・⌘ホバーの対象・着地の規則
//! - [`format`] — 整形（S6 / #1683）の要求・能力・答えの読み取り・範囲の規則
//! - [`completion`] — 補完（S5 / #1682）の応答の読み取り・絞り込み・キーの振り分け表・版の照合
//! - [`menu`] — 右クリックメニューの LSP 項目（S7 / #1684）の出し分け（能力・位置の種類・選択）
//! - [`hover`] — ホバー（S4 / #1681）の応答の読み取り（3 形）・本文の上限・範囲・能力

pub mod completion;
pub mod diagnostic;
pub mod format;
pub mod goto;
pub mod hover;
pub mod menu;
pub mod position;
pub mod root;
pub mod servers;
pub mod state;
pub mod sync;

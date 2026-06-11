//! tako-core — ドメインモデル層（GPUI 非依存）
//!
//! Workspace / Tab / PaneTree / Pane / TerminalSession を提供する。
//! GPUI への依存はここに置かない（GPUI 破壊的変更リスクの防波堤。`.agent/architecture.md`）。
//!
//! Phase 1 タスク 2 で PaneTree ドメインモデルを実装する。

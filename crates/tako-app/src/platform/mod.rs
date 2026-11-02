//! tako-app 固有の抽象境界。
//!
//! `.agent/plans/2026-07-windows-port-architecture.md` §2.1 の規約により、
//! tako-core / tako-control に属さない境界（B11 Web ビュー・B12 ドキュメントレンダラ・
//! B14 配布と自動更新）は各クレート内の `platform/` に同じ形で置く。
//!
//! **`#[cfg(target_os)]` を書いてよいのはこの配下の実装選択だけ**で、
//! 呼び出し側（`preview` / `preview_render` / UI）は単一のコードパスを持つ。

/// 画面構成の変更の通知を UI ストールの診断へ渡す（境界 B33。診断専用。#2666）
pub mod display_reconfig;
/// アプリ内更新の Windows の腕 = インストーラーを検査して無言で起こす（B14。#2434）
pub mod installer_update;
pub mod pdf;
/// 利用者が待っている重い background 処理を App Nap に間引かせない（#1916 / #1926）
pub mod user_work;

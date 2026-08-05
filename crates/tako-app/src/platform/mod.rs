//! tako-app 固有の抽象境界。
//!
//! `.agent/plans/2026-07-windows-port-architecture.md` §2.1 の規約により、
//! tako-core / tako-control に属さない境界（B11 Web ビュー・B12 ドキュメントレンダラ・
//! B14 配布と自動更新）は各クレート内の `platform/` に同じ形で置く。
//!
//! **`#[cfg(target_os)]` を書いてよいのはこの配下の実装選択だけ**で、
//! 呼び出し側（`preview` / `preview_render` / UI）は単一のコードパスを持つ。

pub mod pdf;
pub mod video;

//! tako-core — ドメインモデル層（GPUI 非依存）
//!
//! Workspace / Tab / PaneTree / Pane / TerminalSession を提供する。
//! GPUI への依存はここに置かない（GPUI 破壊的変更リスクの防波堤。`.agent/architecture.md`）。

pub mod pane;
pub mod pane_tree;
pub mod tab;
pub mod workspace;

pub use pane::{Pane, PaneId, PaneOrigin};
pub use pane_tree::{PaneNode, PaneTree, PaneTreeError, Rect, SplitAxis, SplitDirection};
pub use tab::{Tab, TabId};
pub use workspace::{Workspace, WorkspaceError};

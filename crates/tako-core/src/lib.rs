//! tako-core — ドメインモデル層（GPUI 非依存）
//!
//! Workspace / Tab / PaneTree / Pane / TerminalSession / Theme / Screen を提供する。
//! GPUI への依存はここに置かない（GPUI 破壊的変更リスクの防波堤。`.agent/architecture.md`）。

pub mod git;
pub mod osc_tap;
pub mod pane;
pub mod pane_tree;
pub mod paths;
pub mod ports;
pub mod screen;
pub mod scroll;
pub mod shell_integration;
pub mod tab;
pub mod terminal;
pub mod theme;
pub mod tmux;
pub mod tmux_backend;
pub mod workspace;

pub use git::{
    DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffTarget, GitBranch, GitCommit, GitStatus,
    GitStatusEntry,
};
pub use osc_tap::{OscEvent, PromptMark};
pub use pane::{Pane, PaneId, PaneOrigin, TitleSource};
pub use pane_tree::{
    ratio_for_position, PaneBorder, PaneNode, PaneTree, PaneTreeError, Rect, SplitAxis,
    SplitDirection,
};
pub use ports::ListenPort;
pub use screen::{Screen, ScreenLine, StyleRun};
pub use tab::{Tab, TabId};
pub use terminal::{
    login_shell_command, CommandState, SelectionKind, SessionError, SessionEvent, SessionNotice,
    SpawnCommand, SpawnOptions, TermEvent, TerminalSession,
};
pub use theme::{Rgb, Theme};
pub use tmux::{TmuxSession, TmuxWindow};
pub use workspace::{Workspace, WorkspaceError};

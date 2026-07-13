//! tako-core — ドメインモデル層（GPUI 非依存）
//!
//! Workspace / Tab / PaneTree / Pane / TerminalSession / Theme / Screen を提供する。
//! GPUI への依存はここに置かない（GPUI 破壊的変更リスクの防波堤。`.agent/architecture.md`）。

pub mod git;
pub mod links;
pub mod osc_tap;
pub mod pane;
pub mod pane_tree;
pub mod paths;
pub mod ports;
pub mod screen;
pub mod scroll;
pub mod shell_integration;
pub mod spawn_layout;
pub mod tab;
pub mod terminal;
pub mod text_edit;
pub mod theme;
pub mod tmux;
pub mod tmux_backend;
pub mod workspace;

pub use git::{
    DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffTarget, GitBranch, GitCommit, GitStatus,
    GitStatusEntry, GraphLayout, GraphLine, GraphRow, GRAPH_PALETTE,
};
pub use links::{detect_links, detect_links_with_cwd, link_at, DetectedLink, LinkKind};
pub use osc_tap::{OscEvent, PromptMark};
pub use pane::{Pane, PaneId, PaneOrigin, TitleSource};
pub use pane_tree::{
    ratio_for_position, PaneBorder, PaneNode, PaneTree, PaneTreeError, Rect, SplitAxis,
    SplitDirection,
};
pub use ports::ListenPort;
pub use screen::{InputStatus, InputStyle, Screen, ScreenLine, StyleRun};
pub use spawn_layout::{SpawnLayoutConfig, SpawnLayoutPolicy, WorkerLayoutAlgorithm};
pub use tab::{Tab, TabId};
pub use terminal::{
    login_shell_command, AgentMetrics, CommandState, SelectionKind, SessionError, SessionEvent,
    SessionNotice, SpawnCommand, SpawnOptions, TermEvent, TerminalSession,
};
pub use text_edit::{CursorMovement, TextBuffer, TextEditError};
pub use theme::{Rgb, Theme};
pub use tmux::{TmuxSession, TmuxWindow};
pub use workspace::{BackgroundPane, Workspace, WorkspaceError};

/// 外部バイナリの解決（環境変数 → PATH 直 → 既知パス → ログインシェル）。
/// `tmux_bin()` / `git_bin()` の共通基盤
pub(crate) fn resolve_bin(
    env_var: &str,
    name: &str,
    version_flag: &str,
    candidates: &[&str],
) -> String {
    if let Some(bin) = std::env::var_os(env_var) {
        if !bin.is_empty() {
            return bin.to_string_lossy().into_owned();
        }
    }
    if std::process::Command::new(name)
        .arg(version_flag)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return name.into();
    }
    for candidate in candidates {
        if std::path::Path::new(candidate).is_file() {
            return (*candidate).into();
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = std::process::Command::new(shell)
            .args(["-l", "-c", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && std::path::Path::new(&path).is_file() {
                    return path;
                }
            }
        }
    }
    name.into()
}

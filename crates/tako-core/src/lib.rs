//! tako-core — ドメインモデル層（GPUI 非依存）
//!
//! Workspace / Tab / PaneTree / Pane / TerminalSession / Theme / Screen を提供する。
//! GPUI への依存はここに置かない（GPUI 破壊的変更リスクの防波堤。`.agent/architecture.md`）。

pub mod acceptance_gate;
pub mod byte_lru;
pub mod git;
pub mod header_layout;
pub mod i18n;
pub mod links;
pub mod osc_tap;
pub mod pane;
pub mod pane_log;
pub mod pane_tree;
pub mod paths;
pub mod pdf_links;
pub mod ports;
pub mod preview_cache;
pub mod preview_outline;
pub mod preview_reload;
pub mod preview_view;
pub mod recent;
pub mod runner;
pub mod screen;
pub mod scroll;
pub mod scroll_mirror;
pub mod shell;
pub mod shell_integration;
pub mod spawn_layout;
pub mod ssh_config;
pub mod tab;
pub mod task_checkpoint;
pub mod terminal;
pub mod text_edit;
pub mod theme;
pub mod tmux;
pub mod tmux_backend;
pub mod workspace;

pub use byte_lru::ByteLru;
pub use git::{
    DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffTarget, GitBranch, GitCommit, GitStatus,
    GitStatusEntry, GraphLayout, GraphLine, GraphRow, GRAPH_PALETTE,
};
pub use header_layout::{truncate_path_middle, HeaderVisibility, PreviewHeaderVisibility};
pub use links::{detect_links, detect_links_with_cwd, link_at, DetectedLink, LinkKind};
pub use osc_tap::{OscEvent, PromptMark};
pub use pane::{Pane, PaneId, PaneOrigin, TitleSource};
pub use pane_tree::{
    ratio_for_position, PaneBorder, PaneNode, PaneTree, PaneTreeError, Rect, SplitAxis,
    SplitDirection,
};
pub use pdf_links::{PdfLink, PdfLinkTarget, PdfLinks};
pub use ports::ListenPort;
pub use preview_cache::{
    preview_cache_bytes, PreviewCacheStats, PREVIEW_CACHE_DEFAULT_MB, PREVIEW_CACHE_MAX_MB,
    PREVIEW_CACHE_MIN_MB,
};
pub use preview_outline::{PreviewOutline, PreviewOutlineItem, PreviewOutlineTarget};
pub use preview_reload::PreviewReloadState;
pub use preview_view::{
    PreviewViewState, PreviewViewUpdate, PreviewZoomCommand, PREVIEW_ZOOM_MAX, PREVIEW_ZOOM_MIN,
    PREVIEW_ZOOM_STEP,
};
pub use runner::{
    builtin_defaults, expand_variables, merged_defaults, parse_declarations, resolve, Declarations,
    ProfileDecl, Resolution, RunPlan, RunSource, RunnerError,
};
pub use screen::{InputStatus, InputStyle, Screen, ScreenLine, StyleRun};
pub use shell::{quote_for_shell, quote_paths_for_shell};
pub use spawn_layout::{SpawnLayoutConfig, SpawnLayoutPolicy, WorkerLayoutAlgorithm};
pub use tab::{Tab, TabId};
pub use task_checkpoint::{TaskCheckpoint, TaskPhase};
pub use terminal::{
    login_shell_command, AgentMetrics, CommandState, LimitService, MetricsSource, SelectionKind,
    SessionError, SessionEvent, SessionNotice, SpawnCommand, SpawnOptions, TermEvent,
    TerminalSession,
};
pub use text_edit::{CursorMovement, SearchHit, TextBuffer, TextEditError};
pub use theme::{Rgb, Theme};
pub use tmux::{TmuxSession, TmuxWindow};
pub use workspace::{BackgroundPane, WindowId, Workspace, WorkspaceError, WorkspaceWindow};

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

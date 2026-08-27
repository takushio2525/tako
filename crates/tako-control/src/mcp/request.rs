//! MCP 引数を制御プレーンの [`Request`] へ写す変換と入力検証。

use serde_json::Value;

use super::tools;
use crate::protocol::{Axis, Direction, Request};

/// ツール呼び出しを操作プロトコル（[`Request`]）へ写す。エラーは引数バリデーション失敗
pub(super) fn build_request(
    name: &str,
    args: &Value,
    caller: Option<u64>,
    caller_role: Option<&str>,
) -> Result<Request, String> {
    Ok(match name {
        "tako_list_panes" => Request::List,
        "tako_split_pane" => {
            let tab = u64_arg(args, "tab")?;
            Request::Split {
                // tab 指定時は pane を使わない（タブのフォーカスペインを dispatch が解決）
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
                direction: direction_arg(args)?,
                ratio: f32_arg(args, "ratio")?,
                command: str_vec_arg(args, "command")?.filter(|c| !c.is_empty()),
                cwd: str_arg(args, "cwd")?,
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_send_input" => Request::Send {
            pane: Some(required_u64(args, "pane")?),
            text: str_arg(args, "text")?.ok_or("text を指定する")?,
            newline: bool_arg(args, "newline")?.unwrap_or(true),
            tmux_session: str_arg(args, "tmux_session")?,
            await_prompt: bool_arg(args, "await_prompt")?.unwrap_or(false),
        },
        "tako_read_pane" => Request::Read {
            pane: Some(required_u64(args, "pane")?),
            lines: u64_arg(args, "lines")?.map(|n| n as usize),
            tmux_session: str_arg(args, "tmux_session")?,
        },
        "tako_tmux_list" => Request::TmuxList {
            socket: str_arg(args, "socket")?,
        },
        "tako_tmux_cleanup" => Request::TmuxCleanup {
            socket: str_arg(args, "socket")?,
        },
        "tako_tmux_kill" => Request::TmuxKill {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32),
        },
        "tako_tmux_resize" => Request::TmuxResize {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32).unwrap_or(0),
            cols: u64_arg(args, "cols")?.map(|n| n as u32),
            rows: u64_arg(args, "rows")?.map(|n| n as u32),
            reset: bool_arg(args, "reset")?.unwrap_or(false),
        },
        "tako_tmux_select_window" => Request::TmuxSelectWindow {
            pane: Some(target_pane(args, caller)?),
            window: u64_arg(args, "window")?.ok_or("window を指定する")? as u32,
        },
        "tako_tmux_open" => Request::TmuxOpen {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32),
            pane: Some(target_pane(args, caller)?),
            direction: direction_arg(args)?,
        },
        "tako_scroll_pane" => Request::Scroll {
            pane: Some(target_pane(args, caller)?),
            to: u64_arg(args, "to")?,
            delta: i64_arg(args, "delta")?.map(|n| n as i32),
        },
        "tako_focus_pane" => {
            let pane = u64_arg(args, "pane")?;
            let direction = direction_arg(args)?;
            if pane.is_none() && direction.is_none() {
                return Err("pane か direction のどちらか一方を指定する".into());
            }
            Request::Focus { pane, direction }
        }
        "tako_close_pane" => Request::Close {
            pane: Some(target_pane(args, caller)?),
            force: bool_arg(args, "force")?.unwrap_or(false),
            // #566: 「どのエージェントが閉じたか」をペインログへ残すための監査情報。
            // ツール引数ではなく接続時の role を使う（呼び出し側が名乗り直せない）
            caller_role: caller_role.map(str::to_string),
        },
        "tako_resize_pane" => Request::Resize {
            pane: Some(target_pane(args, caller)?),
            axis: match str_arg(args, "axis")?.as_deref() {
                Some("x") => Axis::X,
                Some("y") => Axis::Y,
                _ => return Err("axis は \"x\" か \"y\" を指定する".into()),
            },
            delta: f32_arg(args, "delta")?,
            share: f32_arg(args, "share")?,
        },
        "tako_equalize_layout" => {
            let tab = u64_arg(args, "tab")?;
            Request::Equalize {
                // tab 省略時は呼び出し元ペインからタブを解決する
                pane: if tab.is_none() {
                    Some(target_pane(args, caller)?)
                } else {
                    None
                },
                tab,
            }
        }
        "tako_set_title" => Request::Title {
            pane: Some(target_pane(args, caller)?),
            title: str_arg(args, "title")?,
            role: str_arg(args, "role")?,
        },
        "tako_rename_tab" => {
            let tab = u64_arg(args, "tab")?;
            Request::TabRename {
                pane: if tab.is_none() {
                    Some(target_pane(args, caller)?)
                } else {
                    None
                },
                tab,
                title: str_arg(args, "title")?.ok_or("title を指定する")?,
                source: str_arg(args, "source")?,
            }
        }
        "tako_pin_tab_title" => {
            let tab = u64_arg(args, "tab")?;
            Request::TabPinTitle {
                pane: if tab.is_none() {
                    Some(target_pane(args, caller)?)
                } else {
                    None
                },
                tab,
                pinned: bool_arg(args, "pinned")?,
            }
        }
        "tako_create_tab" => Request::TabNew {
            title: str_arg(args, "title")?,
            focus: bool_arg(args, "focus")?,
            cwd: str_arg(args, "cwd")?,
        },
        "tako_select_tab" => Request::TabSelect {
            tab: required_u64(args, "tab")?,
        },
        "tako_reorder_tab" => Request::TabReorder {
            tab: required_u64(args, "tab")?,
            index: required_u64(args, "index")? as usize,
        },
        "tako_window" => {
            let action = str_arg(args, "action")?.unwrap_or_else(|| "list".into());
            match action.as_str() {
                "list" => Request::WindowList,
                "new" => Request::WindowNew {
                    tab: u64_arg(args, "tab")?,
                },
                "close" => Request::WindowClose {
                    window: required_u64(args, "window")?,
                },
                "move-tab" => Request::WindowMoveTab {
                    tab: required_u64(args, "tab")?,
                    window: required_u64(args, "window")?,
                },
                "focus" => Request::WindowFocus {
                    window: required_u64(args, "window")?,
                },
                "minimize" => Request::WindowMinimize {
                    window: u64_arg(args, "window")?,
                },
                "maximize" => Request::WindowMaximize {
                    window: u64_arg(args, "window")?,
                },
                "restore" => Request::WindowRestore {
                    window: u64_arg(args, "window")?,
                },
                other => {
                    return Err(format!(
                        "action が不正: {other}（list | new | close | move-tab | focus | \
                         minimize | maximize | restore）"
                    ))
                }
            }
        }
        "tako_menu" => {
            let action = str_arg(args, "action")?.unwrap_or_else(|| "list".into());
            match action.as_str() {
                "list" => Request::MenuList,
                "open" => Request::MenuOpen {
                    menu: str_arg(args, "menu")?
                        .ok_or_else(|| "open には menu が必要".to_string())?,
                },
                "close" => Request::MenuClose,
                "invoke" => Request::MenuInvoke {
                    path: str_arg(args, "path")?
                        .ok_or_else(|| "invoke には path が必要".to_string())?,
                },
                other => {
                    return Err(format!(
                        "action が不正: {other}（list | open | close | invoke）"
                    ))
                }
            }
        }
        "tako_move_pane_to_tab" => {
            let new_tab = bool_arg(args, "new_tab")?.unwrap_or(false);
            Request::MovePane {
                pane: Some(target_pane(args, caller)?),
                tab: if new_tab { None } else { u64_arg(args, "tab")? },
                target: if new_tab {
                    None
                } else {
                    u64_arg(args, "target")?
                },
                direction: if new_tab { None } else { direction_arg(args)? },
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_auto_rename" => Request::AutoRename {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_port_detect" => Request::PortDetect {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_autosuggest" => Request::Autosuggest {
            enabled: bool_arg(args, "enabled")?,
            hint: bool_arg(args, "hint")?,
            tab: bool_arg(args, "tab")?,
        },
        "tako_persist" => Request::Persist {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_confirm_close" => Request::ConfirmClose {
            enabled: bool_arg(args, "enabled")?,
        },
        // #813: 一覧（all）のときは対象ペインを解決しない（呼び出し元が無くても引ける）
        "tako_limit_resume" => {
            let all = bool_arg(args, "all")?;
            Request::LimitResume {
                pane: if all == Some(true) {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                enabled: bool_arg(args, "enabled")?,
                all,
            }
        }
        "tako_open_file" => Request::OpenFile {
            pane: Some(target_pane(args, caller)?),
            path: str_arg(args, "path")?.ok_or("path を指定する")?,
            mode: match str_arg(args, "mode")?.as_deref() {
                None => None,
                Some("code") => Some(crate::protocol::PreviewModeWire::Code),
                Some("markdown") => Some(crate::protocol::PreviewModeWire::Markdown),
                Some(other) => return Err(format!("mode が不正: {other}（code | markdown）")),
            },
            direction: direction_arg(args)?,
            focus: bool_arg(args, "focus")?,
            new_tab: bool_arg(args, "new_tab")?.unwrap_or(false),
        },
        "tako_preview_view" => Request::PreviewView {
            pane: Some(target_pane(args, caller)?),
            zoom: f32_arg(args, "zoom")?,
            zoom_in: bool_arg(args, "zoom_in")?.unwrap_or(false),
            zoom_out: bool_arg(args, "zoom_out")?.unwrap_or(false),
            reset: bool_arg(args, "reset")?.unwrap_or(false),
            page: u64_arg(args, "page")?.map(|page| page as usize),
            pan_x: f32_arg(args, "pan_x")?,
            pan_y: f32_arg(args, "pan_y")?,
        },
        "tako_preview_outline" => Request::PreviewOutline {
            pane: Some(target_pane(args, caller)?),
            item: u64_arg(args, "item")?.map(|item| item as usize),
        },
        "tako_preview_link_list" => Request::PreviewLinkList {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_follow_link" => Request::PreviewFollowLink {
            pane: Some(target_pane(args, caller)?),
            index: u64_arg(args, "index")?.ok_or("index を指定する")? as usize,
        },
        "tako_preview_copy_code" => Request::PreviewCopyCode {
            pane: Some(target_pane(args, caller)?),
            index: u64_arg(args, "index")?.map(|index| index as usize),
        },
        "tako_chat_copy" => Request::ChatCopy {
            pane: Some(target_pane(args, caller)?),
            list: bool_arg(args, "list")?.unwrap_or(false),
            message: u64_arg(args, "message")?.map(|index| index as usize),
            code: u64_arg(args, "code")?.map(|index| index as usize),
            markdown: bool_arg(args, "markdown")?.unwrap_or(false),
        },
        "tako_preview_reload" => Request::PreviewReload {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_preview_cache" => Request::PreviewCache {
            max_mb: u64_arg(args, "max_mb")?,
        },
        "tako_preview_edit" => Request::PreviewEdit {
            pane: Some(target_pane(args, caller)?),
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_preview_apply" => Request::PreviewApply {
            pane: Some(target_pane(args, caller)?),
            text: str_arg(args, "text")?.ok_or("text を指定する")?,
        },
        "tako_preview_save" => Request::PreviewSave {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_undo" => Request::PreviewUndo {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_redo" => Request::PreviewRedo {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_search" => Request::PreviewSearch {
            pane: Some(target_pane(args, caller)?),
            query: str_arg(args, "query")?,
            direction: str_arg(args, "direction")?,
        },
        "tako_preview_replace" => Request::PreviewReplace {
            pane: Some(target_pane(args, caller)?),
            query: str_arg(args, "query")?.ok_or("query を指定する")?,
            replacement: str_arg(args, "replacement")?.ok_or("replacement を指定する")?,
            all: bool_arg(args, "all")?,
        },
        "tako_preview_autosave" => Request::PreviewAutosave {
            pane: Some(target_pane(args, caller)?),
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_preview_changelog" => Request::PreviewChangelog {
            pane: Some(target_pane(args, caller)?),
            enabled: bool_arg(args, "enabled")?,
            max_count: u64_arg(args, "max_count")?.map(|v| v as usize),
            expand: str_arg(args, "expand")?,
        },
        "tako_file_op" => {
            let op_str = str_arg(args, "op")?.ok_or("op を指定する")?;
            let op = match op_str.as_str() {
                "copy_absolute_path" => crate::protocol::FileOpKind::CopyAbsolutePath,
                "copy_relative_path" => crate::protocol::FileOpKind::CopyRelativePath,
                "reveal" => crate::protocol::FileOpKind::Reveal,
                "open_terminal" => crate::protocol::FileOpKind::OpenTerminal,
                "rename" => crate::protocol::FileOpKind::Rename,
                "create_file" => crate::protocol::FileOpKind::CreateFile,
                "create_dir" => crate::protocol::FileOpKind::CreateDir,
                "trash" => crate::protocol::FileOpKind::Trash,
                "open_default" => crate::protocol::FileOpKind::OpenDefault,
                "open_with" => crate::protocol::FileOpKind::OpenWith,
                other => return Err(format!("op が不正: {other}")),
            };
            Request::FileOp {
                op,
                path: str_arg(args, "path")?.ok_or("path を指定する")?,
                name: str_arg(args, "name")?,
                pane: match op {
                    crate::protocol::FileOpKind::OpenTerminal
                    | crate::protocol::FileOpKind::CopyRelativePath => {
                        Some(target_pane(args, caller)?)
                    }
                    _ => None,
                },
            }
        }
        "tako_git_log" => Request::GitLog {
            pane: Some(target_pane(args, caller)?),
            max_count: u64_arg(args, "max_count")?.map(|n| n as usize),
        },
        "tako_git_diff" => Request::GitDiff {
            pane: Some(target_pane(args, caller)?),
            target: str_arg(args, "target")?,
        },
        "tako_git_show" => Request::GitShow {
            pane: Some(target_pane(args, caller)?),
            hash: str_arg(args, "hash")?.ok_or("hash を指定する")?,
            file: str_arg(args, "file")?,
        },
        "tako_git_commit" => Request::GitCommit {
            pane: Some(target_pane(args, caller)?),
            message: str_arg(args, "message")?.ok_or("message を指定する")?,
            all: bool_arg(args, "all")?.unwrap_or(false),
        },
        "tako_git_pull" => Request::GitPull {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_git_push" => Request::GitPush {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_git_stage" => Request::GitStage {
            pane: Some(target_pane(args, caller)?),
            paths: str_array_arg(args, "paths"),
        },
        "tako_git_unstage" => Request::GitUnstage {
            pane: Some(target_pane(args, caller)?),
            paths: str_array_arg(args, "paths"),
        },
        "tako_git_checkout" => Request::GitCheckout {
            pane: Some(target_pane(args, caller)?),
            branch: str_arg(args, "branch")?.ok_or("branch を指定する")?,
            confirm: bool_arg(args, "confirm")?.unwrap_or(false),
        },
        "tako_git_branch_create" => Request::GitBranchCreate {
            pane: Some(target_pane(args, caller)?),
            name: str_arg(args, "name")?.ok_or("name を指定する")?,
            start_point: str_arg(args, "start_point")?,
            checkout: bool_arg(args, "checkout")?,
        },
        "tako_git_merge" => Request::GitMerge {
            pane: Some(target_pane(args, caller)?),
            branch: str_arg(args, "branch")?.ok_or("branch を指定する")?,
            confirm: bool_arg(args, "confirm")?.unwrap_or(false),
            no_ff: bool_arg(args, "no_ff")?.unwrap_or(false),
        },
        "tako_git_merge_abort" => Request::GitMergeAbort {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_git_conflicts" => Request::GitConflicts {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_git_resolve_agent" => Request::GitResolveAgent {
            pane: Some(target_pane(args, caller)?),
            agent: str_arg(args, "agent")?,
            tab: u64_arg(args, "tab")?,
        },
        "tako_background_pane" => {
            let tab = u64_arg(args, "tab")?;
            Request::Background {
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
            }
        }
        "tako_foreground_pane" => Request::Foreground {
            pane: required_u64(args, "pane")?,
            target: u64_arg(args, "target")?,
            direction: direction_arg(args)?,
        },
        "tako_background_list" => Request::BackgroundList,
        "tako_background_kill" => Request::BackgroundKill {
            pane: required_u64(args, "pane")?,
        },
        "tako_panel" => Request::Panel {
            visible: bool_arg(args, "visible")?,
            width: f32_arg(args, "width")?,
            view: match str_arg(args, "view")?.as_deref() {
                None => None,
                // 正式値は GUI のタブ表示名と 1:1、旧称 tmux も後方互換で受理する（#553）
                Some(v) => match crate::protocol::PanelViewWire::parse(v) {
                    Some(view) => Some(view),
                    None => {
                        return Err(format!(
                            "view が不正: {v}（{}）",
                            crate::protocol::PanelViewWire::values_hint()
                        ))
                    }
                },
            },
            filetree: bool_arg(args, "filetree")?,
            sidebar_width: f32_arg(args, "sidebar_width")?,
            show_hidden: bool_arg(args, "show_hidden")?,
        },
        "tako_collapse_tab" => Request::CollapseTab {
            pane: u64_arg(args, "pane")?.or(caller),
            tab: u64_arg(args, "tab")?,
            collapsed: bool_arg(args, "collapsed")?,
        },
        "tako_pin_preview" => {
            let group_tab = u64_arg(args, "group_tab")?;
            Request::Pin {
                // group_tab 指定時は pane を補完しない（排他）
                pane: if group_tab.is_some() {
                    None
                } else {
                    u64_arg(args, "pane")?.or(caller)
                },
                group_tab,
                pinned: bool_arg(args, "pinned")?,
            }
        }
        "tako_check_health" => Request::CheckHealth,
        "tako_setup_mcp" => Request::SetupMcp {
            scope: str_arg(args, "scope")?,
            pane: u64_arg(args, "pane")?.or(caller),
            agent: str_arg(args, "agent")?,
        },
        "tako_video_playback" => Request::VideoPlayback {
            pane: Some(target_pane(args, caller)?),
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
        },
        "tako_video_seek" => Request::VideoSeek {
            pane: Some(target_pane(args, caller)?),
            seconds: f64_arg(args, "seconds")?.ok_or("seconds を指定する")?,
        },
        "tako_video_volume" => Request::VideoVolume {
            pane: Some(target_pane(args, caller)?),
            volume: f64_arg(args, "volume")?.ok_or("volume を指定する")?,
        },
        "tako_orchestrator_projects" => Request::OrchestratorProjects {
            action: str_arg(args, "action")?.unwrap_or_else(|| "list".into()),
            key: str_arg(args, "key")?,
            cwd: str_arg(args, "cwd")?,
            description: str_arg(args, "description")?,
        },
        "tako_orchestrator_profiles" => Request::OrchestratorProfiles {
            action: str_arg(args, "action")?.unwrap_or_else(|| "list".into()),
            name: str_arg(args, "name")?,
            kind: str_arg(args, "kind")?,
            from: str_arg(args, "from")?,
            projects: str_vec_arg(args, "projects")?,
            clear_projects: bool_arg(args, "clear_projects")?.unwrap_or(false),
            model: str_arg(args, "model")?,
            master_agent: str_arg(args, "master_agent")?,
            clear_master_agent: bool_arg(args, "clear_master_agent")?.unwrap_or(false),
            worker_model: str_arg(args, "worker_model")?,
            effort: str_arg(args, "effort")?,
            worker_effort: str_arg(args, "worker_effort")?,
            clear_model: bool_arg(args, "clear_model")?.unwrap_or(false),
            clear_worker_model: bool_arg(args, "clear_worker_model")?.unwrap_or(false),
            worker_agent: str_arg(args, "worker_agent")?,
            clear_worker_agent: bool_arg(args, "clear_worker_agent")?.unwrap_or(false),
            agent: str_arg(args, "agent")?,
            agent_model: str_arg(args, "agent_model")?,
            clear_agent_model: bool_arg(args, "clear_agent_model")?.unwrap_or(false),
            agent_effort: str_arg(args, "agent_effort")?,
            clear_agent_effort: bool_arg(args, "clear_agent_effort")?.unwrap_or(false),
            agent_skip_permissions: bool_arg(args, "agent_skip_permissions")?,
            agent_args: str_vec_arg(args, "agent_args")?,
            worker_model_policy: str_arg(args, "worker_model_policy")?,
            tab_naming_convention: str_arg(args, "tab_naming_convention")?,
            env_set: str_vec_arg(args, "env_set")?,
            env_unset: str_vec_arg(args, "env_unset")?,
            master_account: str_arg(args, "master_account")?,
            clear_master_account: bool_arg(args, "clear_master_account")?.unwrap_or(false),
            worker_account: str_arg(args, "worker_account")?,
            clear_worker_account: bool_arg(args, "clear_worker_account")?.unwrap_or(false),
            ctx_threshold: u64_arg(args, "ctx_threshold")?.map(|v| v as u32),
            clear_ctx_threshold: bool_arg(args, "clear_ctx_threshold")?.unwrap_or(false),
            auto_handoff: bool_arg(args, "auto_handoff")?,
            clear_auto_handoff: bool_arg(args, "clear_auto_handoff")?.unwrap_or(false),
            limit_resume: bool_arg(args, "limit_resume")?,
            clear_limit_resume: bool_arg(args, "clear_limit_resume")?.unwrap_or(false),
            bypass_sandbox: bool_arg(args, "bypass_sandbox")?,
        },
        "tako_orchestrator_accounts" => Request::OrchestratorAccounts {
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
            name: str_arg(args, "name")?,
            config_dir: str_arg(args, "config_dir")?,
            inherit: bool_arg(args, "inherit")?,
            description: str_arg(args, "description")?,
            default_model: str_arg(args, "default_model")?,
            default_effort: str_arg(args, "default_effort")?,
        },
        "tako_orchestrator_layout" => Request::OrchestratorLayout {
            policy: str_arg(args, "policy")?,
            master_ratio: f64_arg(args, "master_ratio")?.map(|v| v as f32),
            algorithm: str_arg(args, "algorithm")?,
        },
        "tako_orchestrator_self" => Request::OrchestratorSelf {
            pane: u64_arg(args, "pane")?.or(caller),
            caller_role: caller_role.map(str::to_string),
            caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
        },
        "tako_orchestrator_handoff" => Request::OrchestratorHandoff {
            pane: u64_arg(args, "pane")?.or(caller),
            caller_role: caller_role.map(str::to_string),
            tab: u64_arg(args, "tab")?,
            caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
            // #915: 明示指定は推定より優先される（省略時はプロファイル担当 + 稼働 worker）
            projects: match str_array_arg(args, "projects") {
                v if v.is_empty() => None,
                v => Some(v),
            },
        },
        "tako_orchestrator_handoffs" => Request::OrchestratorHandoffFiles {
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
            project: str_arg(args, "project")?,
            profile: str_arg(args, "profile")?,
            content: str_arg(args, "content")?,
        },
        "tako_orchestrator_spawn" => {
            let pane = u64_arg(args, "pane")?;
            let tab = u64_arg(args, "tab")?;
            let resolved_pane = if pane.is_some() {
                pane
            } else if tab.is_some() {
                None
            } else {
                caller
            };
            let resolved_tab = if pane.is_some() { None } else { tab };
            if resolved_pane.is_none() && resolved_tab.is_none() {
                return Err("pane または tab を指定してください".into());
            }
            Request::OrchestratorSpawn {
                project: str_arg(args, "project")?.ok_or("project を指定する")?,
                prompt: str_arg(args, "prompt")?.ok_or("prompt を指定する")?,
                label: str_arg(args, "label")?,
                model: str_arg(args, "model")?,
                effort: str_arg(args, "effort")?,
                pane: resolved_pane,
                tab: resolved_tab,
                caller_role: caller_role.map(str::to_string),
                agent: str_arg(args, "agent")?,
                caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
                task_type: str_arg(args, "task_type")?,
                account: str_arg(args, "account")?,
                limit_resume: bool_arg(args, "limit_resume")?,
            }
        }
        "tako_orchestrator_report" => Request::OrchestratorReport {
            pane_id: u64_arg(args, "pane_id")?,
            lines: u64_arg(args, "lines")?.map(|v| v as usize),
            messages: u64_arg(args, "messages")?.map(|v| v as usize),
            worker: str_arg(args, "worker")?,
        },
        "tako_orchestrator_worker_status" => Request::OrchestratorWorkerStatus {
            pane_id: u64_arg(args, "pane_id")?,
            session_id: str_arg(args, "session_id")?,
            tmux_session: str_arg(args, "tmux_session")?,
            worker: str_arg(args, "worker")?,
        },
        "tako_orchestrator_workers" => Request::OrchestratorWorkers {
            all: bool_arg(args, "all")?,
        },
        "tako_orchestrator_run_status" => Request::OrchestratorRunStatus {
            run_id: str_arg(args, "run_id")?,
        },
        "tako_orchestrator_run_result" => Request::OrchestratorRunResult {
            run_id: str_arg(args, "run_id")?.ok_or("run_id を指定する")?,
        },
        "tako_orchestrator_respond" => Request::OrchestratorRespond {
            pane_id: required_u64(args, "pane_id")?,
            // #748: choice 省略 = 送信せず構造だけ返す（下見）
            choice: str_arg(args, "choice")?,
            caller_role: caller_role.map(str::to_string),
        },
        "tako_orchestrator_supervisor" => Request::OrchestratorSupervisor {
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
            mode: str_arg(args, "mode")?,
            auto_resume_dead: bool_arg(args, "auto_resume_dead")?,
            max_retries: u64_arg(args, "max_retries")?.map(|v| v as u32),
            lines: u64_arg(args, "lines")?.map(|v| v as usize),
        },
        "tako_orchestrator_ledger" => Request::OrchestratorLedger {
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
            id: str_arg(args, "id")?,
            outcome: str_arg(args, "outcome")?,
            rounds: u64_arg(args, "rounds")?.map(|v| v as u32),
            note: str_arg(args, "note")?,
            project: str_arg(args, "project")?,
            task_type: str_arg(args, "task_type")?,
            limit: u64_arg(args, "limit")?.map(|v| v as usize),
        },
        "tako_remote_start" => Request::RemoteStart {},
        "tako_remote_stop" => Request::RemoteStop {
            force: bool_arg(args, "force")?.unwrap_or(false),
        },
        "tako_remote_status" => Request::RemoteStatus,
        "tako_remote_agents" => Request::RemoteAgents,
        "tako_remote_messages" => Request::RemoteMessages {
            session_id: str_arg(args, "session_id")?.ok_or("session_id を指定する")?,
            tail: u64_arg(args, "tail")?.map(|n| n as usize),
        },
        "tako_remote_devices" => Request::RemoteDevices {
            action: str_arg(args, "action")?.ok_or("action を指定する（list / revoke）")?,
            device_id: str_arg(args, "device_id")?,
        },
        "tako_remote_setup" => Request::RemoteSetup {
            action: str_arg(args, "action")?.ok_or("action を指定する（check / run）")?,
            answers: args.get("answers").cloned(),
        },
        "tako_remote_scrollback" => Request::RemoteScrollback {
            pane_id: str_arg(args, "pane_id")?
                .ok_or("pane_id を指定する")?
                .to_string(),
            lines: u64_arg(args, "lines")?.map(|n| n as u32),
        },
        "tako_web" => {
            let action = str_arg(args, "action")?.ok_or("action は必須")?.to_string();
            // 分割系（open / show）だけ、基準ペイン省略時に呼び出し元を分割元とする。
            // 対象指定系で caller を埋めると「AI 自身のペイン」を対象と誤解するため埋めない
            let pane = match action.as_str() {
                "open" | "show" => u64_arg(args, "pane")?.or(caller),
                _ => u64_arg(args, "pane")?,
            };
            Request::Web {
                action,
                url: str_arg(args, "url")?.map(|s| s.to_string()),
                id: u64_arg(args, "id")?,
                pane,
                direction: direction_arg(args)?,
                to: str_arg(args, "to")?.map(|s| s.to_string()),
                js: str_arg(args, "js")?.map(|s| s.to_string()),
                token: u64_arg(args, "token")?,
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_update" => Request::Update {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            channel: str_arg(args, "channel")?.map(|s| s.to_string()),
        },
        "tako_fda" => Request::Fda {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_sleep_guard" => Request::SleepGuard {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            mode: str_arg(args, "mode")?.map(|s| s.to_string()),
            power_condition: str_arg(args, "power_condition")?.map(|s| s.to_string()),
            lid_sleep_mode: str_arg(args, "lid_sleep_mode")?.map(|s| s.to_string()),
        },
        "tako_theme" => Request::Theme {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            mode: str_arg(args, "mode")?.map(|s| s.to_string()),
            target: str_arg(args, "target")?.map(|s| s.to_string()),
            key: str_arg(args, "key")?.map(|s| s.to_string()),
            value: str_arg(args, "value")?.map(|s| s.to_string()),
            name: str_arg(args, "name")?.map(|s| s.to_string()),
            font_family: str_arg(args, "font_family")?.map(|s| s.to_string()),
            font_size: str_arg(args, "font_size")?.and_then(|s| s.parse::<f32>().ok()),
        },
        "tako_stale_binary" => Request::StaleBinary {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            pane: u64_arg(args, "pane")?,
        },
        "tako_migrate" => Request::Migrate {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            schema: str_arg(args, "schema")?.map(|s| s.to_string()),
        },
        "tako_welcome" => Request::Welcome {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_show_command" => Request::ShowCommand {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            commands: command_array_arg(args, "commands")?,
            label: str_arg(args, "label")?.map(|s| s.to_string()),
            pane: u64_arg(args, "pane")?,
            card: u64_arg(args, "card")?,
            index: u64_arg(args, "index")?.map(|i| i as usize),
            focus: bool_arg(args, "focus")?,
        },
        "tako_config_share" => Request::ConfigShare {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            target: str_arg(args, "target")?.map(|s| s.to_string()),
            path: str_arg(args, "path")?.map(|s| s.to_string()),
            remote: str_arg(args, "remote")?.map(|s| s.to_string()),
            message: str_arg(args, "message")?.map(|s| s.to_string()),
            no_push: bool_arg(args, "no_push")?.unwrap_or(false),
        },
        "tako_shell_integration" => Request::ShellIntegration {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_agent_support" => Request::AgentSupport {
            agent: str_arg(args, "agent")?.map(|s| s.to_string()),
            status: str_arg(args, "status")?.map(|s| s.to_string()),
        },
        "tako_platform" => Request::Platform {
            platform: str_arg(args, "platform")?.map(|s| s.to_string()),
            status: str_arg(args, "status")?.map(|s| s.to_string()),
            known_limitations: bool_arg(args, "known_limitations")?.unwrap_or(false),
        },
        "tako_lang" => Request::Lang {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            value: str_arg(args, "value")?.map(|s| s.to_string()),
        },
        "tako_ui_mode" => Request::UiMode {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            mode: str_arg(args, "mode")?.map(|s| s.to_string()),
            // release / restore 以外は pane を使わないので、ここでは既定補完だけして
            // 必須判定は dispatch 側（action ごとの意味を知っている側）に任せる
            pane: u64_arg(args, "pane")?.or(caller),
        },
        "tako_limit_service" => Request::LimitService {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            service: str_arg(args, "service")?.map(|s| s.to_string()),
        },
        "tako_telemetry" => Request::Telemetry {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_settings" => Request::Settings {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            tab: str_arg(args, "tab")?.map(|s| s.to_string()),
        },
        "tako_setup_changes" => Request::SetupChanges,
        "tako_setup_bootstrap" => Request::SetupBootstrap {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            dry_run: bool_arg(args, "dry_run")?,
        },
        "tako_setup" => Request::SetupRun {
            answers: Some(args.clone()),
        },
        "tako_agents_sync_rules" => Request::AgentsSyncRules {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            source: str_arg(args, "source")?.map(|s| s.to_string()),
            targets: {
                let arr = args.get("targets").and_then(Value::as_array);
                arr.map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
            },
        },
        "tako_tree_folder" => Request::TreeFolder {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
            path: str_arg(args, "path")?.map(|s| s.to_string()),
            tab: u64_arg(args, "tab")?,
            pane: caller,
        },
        "tako_sessions" => {
            let action = str_arg(args, "action")?.ok_or("action を指定する")?;
            Request::Sessions {
                // resume はペイン省略時に呼び出し元（master 自身の隣）へ分割する
                pane: if action == "resume" && u64_arg(args, "tab")?.is_none() {
                    u64_arg(args, "pane")?.or(caller)
                } else {
                    u64_arg(args, "pane")?
                },
                action,
                id: str_arg(args, "id")?,
                role: str_arg(args, "role")?,
                project: str_arg(args, "project")?,
                limit: u64_arg(args, "limit")?.map(|v| v as usize),
                tab: u64_arg(args, "tab")?,
                direction: direction_arg(args)?,
            }
        }
        "tako_logs" => Request::Logs {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
            // read はペイン・セッション未指定なら呼び出し元ペインのログを引く
            pane: match (u64_arg(args, "pane")?, str_arg(args, "session_id")?) {
                (Some(p), _) => Some(p),
                (None, None) => caller,
                (None, Some(_)) => None,
            },
            session_id: str_arg(args, "session_id")?,
            lines: u64_arg(args, "lines")?.map(|v| v as usize),
            enabled: bool_arg(args, "enabled")?,
            max_mb: u64_arg(args, "max_mb")?,
            total_max_mb: u64_arg(args, "total_max_mb")?,
        },
        "tako_open_dir" => Request::OpenDir {
            path: str_arg(args, "path")?.ok_or("path を指定する")?.to_string(),
            focus: bool_arg(args, "focus")?,
        },
        "tako_open_remote" => Request::OpenRemote {
            host: str_arg(args, "host")?.ok_or("host を指定する")?.to_string(),
            focus: bool_arg(args, "focus")?,
            remote_dir: str_arg(args, "remote_dir")?.map(|s| s.to_string()),
            // #1006: 語彙の正本は `tako_core::remote_open`（CLI / GUI と同じ表）
            target: match str_arg(args, "target")? {
                Some(t) => Some(
                    tako_core::remote_open::RemoteOpenTarget::parse(&t).ok_or_else(|| {
                        format!(
                            "target は {} のいずれか",
                            tako_core::remote_open::RemoteOpenTarget::values_hint()
                        )
                    })?,
                ),
                None => None,
            },
            pane: u64_arg(args, "pane")?,
            tab: u64_arg(args, "tab")?,
            direction: direction_arg(args)?,
        },
        "tako_ssh_hosts" => Request::SshHosts,
        "tako_remote_folder" => Request::RemoteFolder {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
            host: str_arg(args, "host")?.map(|s| s.to_string()),
            path: str_arg(args, "path")?.map(|s| s.to_string()),
            tab: u64_arg(args, "tab")?,
            focus: bool_arg(args, "focus")?,
            all: bool_arg(args, "all")?.unwrap_or(false),
            force: bool_arg(args, "force")?.unwrap_or(false),
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_recent" => Request::RecentItems {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
        },
        "tako_task_checkpoint" => Request::TaskCheckpoint {
            action: "checkpoint".into(),
            task_id: str_arg(args, "task_id")?,
            pane: u64_arg(args, "pane")?.or(caller),
            issue: u64_arg(args, "issue")?.map(|v| v as u32),
            branch: str_arg(args, "branch")?,
            phase: str_arg(args, "phase")?,
            last_commit: str_arg(args, "last_commit")?,
            agent: str_arg(args, "agent")?,
            model: str_arg(args, "model")?,
            prompt_head: str_arg(args, "prompt_head")?,
            suspended_reason: str_arg(args, "suspended_reason")?,
            project: str_arg(args, "project")?,
            cwd: str_arg(args, "cwd")?,
            resume_pane: None,
            tab: None,
            resume_model: None,
            caller_role: caller_role.map(String::from),
        },
        "tako_task_list" => Request::TaskCheckpoint {
            action: "list".into(),
            task_id: None,
            pane: None,
            issue: None,
            branch: None,
            phase: str_arg(args, "phase")?,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            resume_pane: None,
            tab: None,
            resume_model: None,
            caller_role: None,
        },
        "tako_task_resume" => Request::TaskCheckpoint {
            action: "resume".into(),
            task_id: str_arg(args, "task_id")?,
            pane: None,
            issue: None,
            branch: None,
            phase: None,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            resume_pane: if u64_arg(args, "tab")?.is_some() {
                None
            } else {
                u64_arg(args, "pane")?.or(caller)
            },
            tab: u64_arg(args, "tab")?,
            resume_model: str_arg(args, "model")?,
            caller_role: caller_role.map(String::from),
        },
        "tako_task_gate" => {
            let criteria_val = args.get("criteria").ok_or("criteria を指定する")?;
            let criteria_json = serde_json::to_string(criteria_val)
                .map_err(|e| format!("criteria の JSON 変換に失敗: {e}"))?;
            Request::TaskGate {
                action: "set".into(),
                task_id: str_arg(args, "task_id")?,
                criteria_json: Some(criteria_json),
                results_json: None,
                cwd: str_arg(args, "cwd")?,
                sync_checkpoint: None,
            }
        }
        // tako_task_gate_check は call_tool で特殊処理（dispatch を経由しない）
        "tako_task_gate_show" => Request::TaskGate {
            action: "show".into(),
            task_id: str_arg(args, "task_id")?,
            criteria_json: None,
            results_json: None,
            cwd: None,
            sync_checkpoint: None,
        },
        "tako_run_interactive" => {
            let tab = u64_arg(args, "tab")?;
            Request::RunInteractive {
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
                command: str_arg(args, "command")?.ok_or("command を指定する")?,
                input_hint: str_arg(args, "input_hint")?,
                direction: direction_arg(args)?,
                ratio: f32_arg(args, "ratio")?,
                auto_close: str_arg(args, "auto_close")?,
            }
        }
        "tako_run_interactive_status" => Request::RunInteractiveStatus {
            pane: required_u64(args, "pane")?,
            no_wait: false,
        },
        "tako_run" => {
            let tab = u64_arg(args, "tab")?;
            Request::Run {
                path: str_arg(args, "path")?.ok_or("path を指定する")?,
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
                profile: str_arg(args, "profile")?,
                command: str_arg(args, "command")?,
                direction: direction_arg(args)?,
                ratio: f32_arg(args, "ratio")?,
                auto_close: str_arg(args, "auto_close")?,
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_run_resolve" => Request::RunResolve {
            path: str_arg(args, "path")?.ok_or("path を指定する")?,
            pane: u64_arg(args, "pane")?.or(caller),
        },
        "tako_run_defaults" => Request::RunnerDefaults {
            ext: str_arg(args, "ext")?,
            command: str_arg(args, "command")?,
            remove: bool_arg(args, "remove")?.unwrap_or(false),
        },
        _ => return Err(format!("不明なツール: {name}")),
    })
}

/// `pane` 引数（省略時は呼び出し元へフォールバック。FR-2.3.3 のデフォルトスコープ）
fn target_pane(args: &Value, caller: Option<u64>) -> Result<u64, String> {
    u64_arg(args, "pane")?.or(caller).ok_or_else(|| {
        "対象ペインを特定できない（pane を指定する。\
         呼び出し元ペインの自動特定には TAKO_PANE_ID / X-Tako-Pane が必要）"
            .into()
    })
}

/// ツール名 → 許可パラメータ名セットのキャッシュ（#227）。
/// `tools()` のスキーマから `inputSchema.properties` のキーを抽出して構築する。
/// 全ツールの `additionalProperties: false` を実行時に強制する
fn allowed_params_map(
) -> &'static std::collections::HashMap<String, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;
    static MAP: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for tool in tools() {
            if let (Some(name), Some(schema)) = (
                tool.get("name").and_then(Value::as_str),
                tool.get("inputSchema"),
            ) {
                let keys: HashSet<String> = schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|props| props.keys().cloned().collect())
                    .unwrap_or_default();
                map.insert(name.to_string(), keys);
            }
        }
        map
    })
}

/// 引数の全キーがツールスキーマの `properties` に含まれるか検証する。
/// 未知キーがあれば JSON-RPC InvalidParams エラーを返す
pub(super) fn validate_known_params(tool_name: &str, args: &Value) -> Result<(), (i64, String)> {
    let map = allowed_params_map();
    let Some(allowed) = map.get(tool_name) else {
        return Ok(());
    };
    if let Some(obj) = args.as_object() {
        let unknown: Vec<&String> = obj.keys().filter(|k| !allowed.contains(*k)).collect();
        if !unknown.is_empty() {
            return Err((
                -32602,
                format!(
                    "未知のパラメータ: {}（{tool_name} が受け付けるのは {} のみ）",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if allowed.is_empty() {
                        "引数なし".to_string()
                    } else {
                        let mut sorted: Vec<&str> = allowed.iter().map(String::as_str).collect();
                        sorted.sort_unstable();
                        sorted.join(", ")
                    },
                ),
            ));
        }
    }
    Ok(())
}

fn required_u64(args: &Value, key: &str) -> Result<u64, String> {
    u64_arg(args, key)?.ok_or_else(|| format!("{key} を指定する"))
}

pub(super) fn u64_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} は非負整数で指定する")),
    }
}

fn i64_arg(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{key} は整数で指定する")),
    }
}

fn f32_arg(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| format!("{key} は数値で指定する")),
    }
}

fn f64_arg(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{key} は数値で指定する")),
    }
}

pub(super) fn str_arg(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| format!("{key} は文字列で指定する")),
    }
}

pub(super) fn bool_arg(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{key} は真偽値で指定する")),
    }
}

/// #666: コマンド提案カードのコマンド配列。要素が文字列でなければ**黙って捨てずに
/// エラーにする**（提示するコマンドを取りこぼすと、ユーザーへ渡る内容が変わってしまう）。
/// 単一文字列も 1 件として受理する（配列を忘れた呼び出しを弾かない）
fn command_array_arg(args: &Value, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("{key} の要素は文字列で指定する"))
            })
            .collect(),
        Some(_) => Err(format!("{key} は文字列の配列で指定する")),
    }
}

fn str_array_arg(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn direction_arg(args: &Value) -> Result<Option<Direction>, String> {
    match str_arg(args, "direction")?.as_deref() {
        None => Ok(None),
        Some("right") => Ok(Some(Direction::Right)),
        Some("down") => Ok(Some(Direction::Down)),
        Some("left") => Ok(Some(Direction::Left)),
        Some("up") => Ok(Some(Direction::Up)),
        Some(other) => Err(format!(
            "direction が不正: {other}（right / down / left / up のいずれか）"
        )),
    }
}

fn str_vec_arg(args: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{key} は文字列の配列で指定する"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{key} は文字列の配列で指定する")),
    }
}

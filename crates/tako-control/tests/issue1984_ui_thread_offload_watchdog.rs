//! UI スレッドの同期の dispatch が子プロセスを待つ経路の残りを background へ出したかの番犬（#1984）
//!
//! # 何が残っていたか
//!
//! #1979（2026-10-10 の本番ハング = メインスレッドが `ps` の `poll` で 10 分）で UI スレッドの
//! 子プロセスの待ちは全部「上限つき」になったが、次の経路は**同期の dispatch のまま**で、
//! 上限（器 5 秒・CLI 15 秒）まで、あるいは上限の無い待ちで UI ごと止まりえた:
//!
//! - スマホの一覧（`RemoteAgents`）: `claude agents --json` + 器の `list-panes -a`
//! - 呼び出し元の解決（`resolve_caller_pane`。self / adopt / guide / handoff）と spawn の分割元:
//!   pid 祖先辿り = 器の `list-panes -a` + 親子表
//! - spawn / handoff の CLI の所在確認（`agent_cli::preflight`）と tako の実行ファイルの解決:
//!   unix はログインシェルの `command -v`（**上限なし**の `Command::output()`）
//! - セッション再起動の計画（`SessionRestart` / `StaleBinary restart`）: `claude agents --json` +
//!   器の pid 祖先辿り + 親子表
//! - tmux 一覧（IPC の `TmuxList` と右パネルの切替時の取り直し）: `tmux list-sessions` ほか数本
//! - `sleep_guard::check_qos` の `ps`（上限なしの `Command::output()`）
//!
//! # 何を縛るか
//!
//! 1. UI スレッドで走る dispatch の経路（[`UI_PATHS`]）が子プロセスの口（[`RAW`]）を直に呼ばない。
//!    呼んでよいのは読む側の関数（[`READERS`]）だけ
//! 2. 読む側の関数は、子プロセスの口を**先取りの外側（`Probed::Outside`）の腕でだけ**呼ぶ
//!    （内側 = UI スレッドの dispatch で答えが無ければ待たない側へ倒れる）
//! 3. `prepare_offload` が `RemoteAgents` / `TmuxList` を background のジョブにし、
//!    先取りの腕（`probe_needs`）を持つ。`probe_needs` は先取りが要る要求を全部挙げる
//! 4. background の側（`ProbeNeeds::fill` / `run_reply`）が子プロセスの口を実際に呼ぶ
//!    （先取りが空振りすると、読む側が「確かめられない」へ倒れ続ける）
//! 5. tako-app: 右パネルの tmux 一覧の取り直し・再起動のボタンが UI スレッドで同期の dispatch を
//!    しない。IPC の受け口と GUI のボタンは「もう一巡」（`drive_offload`）を通る
//! 6. `check_qos` の `ps` が上限つき
//! 7. #1989: コンフリクト解消エージェント（`GitResolveAgent` = `git rev-parse` / `git status` /
//!    `git name-rev` と CLI の所在確認）・`StaleBinary status`・tmux の書き込み系
//!    （kill / resize / select-window / open / cleanup）も同じ形（background のジョブ / 先取り）。
//!    agent CLI の所在確認（`agent_cli::locate`）は上限つき（修正前の上限なしは A/B のゲートの内側だけ）。
//!    GUI のボタン（window 行・復元・解消エージェント・D&D の取り込み・kill の確認）も `dispatch_off_ui`
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const SLEEP_GUARD: &str = "crates/tako-control/src/sleep_guard.rs";
const AGENT_CLI: &str = "crates/tako-control/src/orchestrator/agent_cli.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";

/// 子プロセスを待つ口（右は名指しの理由）
const RAW: &[(&str, &str)] = &[
    (
        "agents::resolve_pane_by_pid(",
        "pid 祖先辿り（器の `list-panes -a` + 親子表）",
    ),
    (
        "agent_cli::preflight(",
        "CLI の所在確認（unix はログインシェルの `command -v` を上限なしで待つ）",
    ),
    (
        "resolve_tako_binary()",
        "tako の実行ファイルの解決（安定版が無ければログインシェル）",
    ),
    (
        "resolve_session_id_for_backend",
        "`claude agents --json` + 器の pid 祖先辿り",
    ),
    (
        "ProcessSnapshot::capture()",
        "器の `list-panes -a` + 親子表",
    ),
    (
        "list_agents_with_panes(",
        "`claude agents --json` + 器の `list-panes -a`",
    ),
    ("fetch_tmux_sessions(", "`tmux list-sessions` ほか数本"),
    ("tmux::list_sessions(", "`tmux list-sessions` ほか数本"),
    ("backend_pane_pids(", "器の `list-panes -a`"),
    ("process_parent_map(", "親子表の採取"),
    ("exe::find(", "ログインシェルの `command -v`"),
    ("Command::new(", "子プロセスを起こす"),
    (".output()", "子プロセスを上限なしで待つ"),
    // #1989
    ("git::repo_root(", "`git rev-parse`（上限なし）"),
    (
        "git::conflict_state(",
        "`git status` / `git name-rev`（上限なし）",
    ),
    ("tmux::resolve_session_socket(", "`tmux has-session`"),
    ("tmux::has_session(", "`tmux has-session`"),
    ("tmux::session_group(", "`tmux display`"),
    ("tmux::select_window(", "`tmux select-window`"),
    ("tmux::kill_session(", "`tmux kill-session`"),
    ("tmux::kill_window(", "`tmux kill-window`"),
    ("tmux::resize_window(", "`tmux resize-window`"),
    ("cleanup_orphans(", "器の一覧と kill"),
    ("cleanup_servers(", "ソケットごとの器の一覧と kill"),
    (
        "find_claude_pid_for_backend(",
        "器の `list-panes -a` + 親子表",
    ),
    (
        "resolve_current_claude_binary(",
        "`claude` の所在（ログインシェル）と版の確認",
    ),
];

/// UI スレッドで走る dispatch の経路（先取りの判断を含む）。窓の頭の綴り
const UI_PATHS: &[&str] = &[
    "fn resolve_caller_pane(",
    "fn dispatch_orchestrator_self(",
    "fn dispatch_orchestrator_adopt(",
    "fn resolve_handoff_target(",
    "fn dispatch_orchestrator_handoff(",
    "fn dispatch_orchestrator_spawn(",
    "fn spawn_caller_profile(",
    "fn build_session_restart_plan(",
    "fn dispatch_session_restart(",
    "fn dispatch_stale_binary_restart(",
    "fn probe_needs(",
    "fn restart_needs(",
    "fn caller_need(",
    "pub fn collect_tmux_context(",
    "pub fn land_offload(",
    "OffloadContinuation::Probed(probed) => {",
    // #1989
    "fn dispatch_git_resolve_agent(",
    "fn dispatch_stale_binary_status(",
    "fn collect_stale_status_ctx(",
    "fn tmux_select_window_target(",
];

/// 読む側の関数（子プロセスの口は `Probed::Outside` の腕でだけ呼ぶ）
const READERS: &[&str] = &[
    "fn caller_pane_by_pid(",
    "fn agent_preflight(",
    "fn tako_binary(",
    "fn restart_scan(",
    "fn git_conflict_for(",
    "fn tmux_open_lookup(",
];

/// `probe_needs` が挙げるべき要求（先取りが要る要求）
const PROBED_REQUESTS: &[&str] = &[
    "Request::OrchestratorSelf",
    "Request::OrchestratorAdopt",
    "Request::OrchestratorGuide",
    "Request::OrchestratorSpawn",
    "Request::OrchestratorHandoff",
    "Request::SessionRestart",
    "Request::StaleBinary",
    "Request::GitResolveAgent",
    "Request::TmuxOpen",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）。
/// sleep_guard.rs はテストが厚いので下限つきの `production` ではなく素の `scan` を使う
fn read(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close || l.starts_with(&format!("{close},")))
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 窓の中で `needle` を含む**コードの**行（0-based の相対位置）
fn code_lines_with(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

/// 1. UI スレッドの経路が子プロセスの口を直に呼ばない
fn scan_ui_paths(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for head in UI_PATHS {
        let Some((at, window)) = fn_window(dispatch, head) else {
            out.push(Offender {
                file: DISPATCH,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り。改名したなら番犬も追うこと）"),
            });
            continue;
        };
        for (needle, why) in RAW {
            for i in code_lines_with(&window, needle) {
                out.push(Offender {
                    file: DISPATCH,
                    line: at + i,
                    why: format!(
                        "UI スレッドで走る `{head}` が `{needle}` を直に呼んでいる（{why}。\
                         UI スレッドの dispatch で子プロセスを待つ = #1984。読む側の関数 \
                         （`caller_pane_by_pid` / `agent_preflight` / `tako_binary` / \
                         `restart_scan`）を通し、`probe_needs` で先取りすること）"
                    ),
                });
            }
        }
    }
    out
}

/// 2. 読む側の関数は子プロセスの口を `Probed::Outside` の腕でだけ呼ぶ
fn scan_readers(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for head in READERS {
        let Some((at, window)) = fn_window(dispatch, head) else {
            out.push(Offender {
                file: DISPATCH,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            });
            continue;
        };
        let outside = code_lines_with(&window, "Probed::Outside");
        let answer = code_lines_with(&window, "Probed::Answer");
        let missing = code_lines_with(&window, "Probed::Missing");
        if outside.is_empty() || answer.is_empty() || missing.is_empty() {
            out.push(Offender {
                file: DISPATCH,
                line: at,
                why: format!(
                    "読む側の `{head}` が先取りの 3 つの腕（Answer / Missing / Outside）を持っていない \
                     （UI スレッドの dispatch で先取りを読まずにその場で問い合わせる形へ戻る = #1984）"
                ),
            });
            continue;
        }
        let outside_at = outside[0];
        for (needle, why) in RAW {
            for i in code_lines_with(&window, needle) {
                if i < outside_at {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at + i,
                        why: format!(
                            "読む側の `{head}` が `{needle}` を `Probed::Outside` の腕の外で呼んでいる \
                             （{why}。先取りの内側 = UI スレッドの dispatch で子プロセスを待つ = #1984）"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 3. `prepare_offload` が background のジョブにし、`probe_needs` が先取りの要る要求を挙げる
fn scan_prepare(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => push(0, "`prepare_offload` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            for (needle, what) in [
                (
                    "OffloadJob::RemoteAgents",
                    "スマホの一覧（`RemoteAgents`）を background のジョブにしていない \
                     （`claude agents --json` を UI スレッドで待つ）",
                ),
                (
                    "OffloadJob::TmuxList {",
                    "tmux 一覧（`TmuxList`）を background のジョブにしていない \
                     （`tmux list-sessions` ほかを UI スレッドで待つ）",
                ),
                (
                    "probe_needs(host, request)",
                    "先取りの腕（`probe_needs`）が無い（呼び出し元の pid 祖先辿り・CLI の所在確認・\
                     再起動の計画の走査を UI スレッドで待つ）",
                ),
                (
                    "OffloadJob::Probe(",
                    "先取りのジョブ（`OffloadJob::Probe`）を作っていない",
                ),
                (
                    "OffloadJob::StaleStatus { ctx }",
                    "`StaleBinary status` を background のジョブにしていない（器の走査・`claude` の \
                     所在と版の確認を UI スレッドで待つ）",
                ),
                (
                    "OffloadJob::TmuxWrite(",
                    "tmux の kill / resize を background のジョブにしていない",
                ),
                (
                    "OffloadJob::TmuxSelectWindow {",
                    "`TmuxSelectWindow` を background のジョブにしていない",
                ),
                (
                    "OffloadJob::TmuxCleanup(job)",
                    "`TmuxCleanup` の回収の本体を background のジョブにしていない",
                ),
            ] {
                if code_lines_with(&window, needle).is_empty() {
                    push(at, format!("`prepare_offload` が{what} = #1984"));
                }
            }
            // 判断そのものも空の答えの内側で回す（判断の途中で待たない）
            for i in code_lines_with(&window, "probe_needs(host, request)") {
                if !window[i].contains("with_ui_probes(") {
                    push(
                        at + i,
                        "`prepare_offload` が `probe_needs` を先取りの内側（`with_ui_probes`）の外で \
                         呼んでいる（判断の途中で読む側の関数がその場で問い合わせる = #1984）"
                            .into(),
                    );
                }
            }
        }
    }
    match fn_window(dispatch, "fn probe_needs(") {
        None => push(0, "`probe_needs` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            for req in PROBED_REQUESTS {
                // 変種名の後ろの ` {` まで照合する（`OrchestratorSpawnX` のような改名を部分一致で見逃さない）
                if code_lines_with(&window, &format!("{req} {{")).is_empty() {
                    push(
                        at,
                        format!(
                            "`probe_needs` が `{req}` を挙げていない（その要求は先取りされず、\
                             UI スレッドの dispatch で読む側が「確かめられない」へ倒れるか、\
                             `prepare_offload` が同期の腕へ落とす = #1984）"
                        ),
                    );
                }
            }
        }
    }
    out
}

/// 4. background の側が子プロセスの口を実際に呼ぶ（先取りの空振りを防ぐ）
fn scan_background(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(dispatch, "fn fill(self, probes: &mut UiProbes)") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`ProbeNeeds::fill` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for needle in [
                "agents::resolve_pane_by_pid(",
                "agent_cli::preflight(",
                "resolve_tako_binary()",
                "resolve_session_id_for_backend",
                "ProcessSnapshot::capture()",
                "git::repo_root(",
                "compute_tmux_open_lookup(",
            ] {
                if code_lines_with(&window, needle).is_empty() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: format!(
                            "background の `ProbeNeeds::fill` が `{needle}` を呼んでいない（先取りが空振りし、\
                             UI スレッドの読む側が答えの無いまま倒れ続ける = #1984）"
                        ),
                    });
                }
            }
        }
    }
    match fn_window(dispatch, "fn run_reply(self)") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`run_reply` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for needle in [
                "remote_agents_reply()",
                "fetch_tmux_sessions(",
                "finish_stale_status(",
                "tmux_kill_reply(",
                "tmux_resize_reply(",
                "tmux_select_window_reply(",
                "cleanup_tmux_servers_default(",
            ] {
                if code_lines_with(&window, needle).is_empty() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: format!(
                            "background の `run_reply` が `{needle}` を呼んでいない（ジョブにしたのに応答を組めない）"
                        ),
                    });
                }
            }
        }
    }
    match fn_window(dispatch, "OffloadContinuation::Probed(probed) => {") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`finish_offload` の先取りの続きが見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_lines_with(&window, "with_ui_probes(").is_empty() {
                out.push(Offender {
                    file: DISPATCH,
                    line: at,
                    why: "先取りの続きが `with_ui_probes` の内側で dispatch していない（読む側が先取りを \
                          読まずに UI スレッドで子プロセスを待つ = #1984）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 5. tako-app: 右パネルの tmux 一覧・再起動のボタン・受け口
fn scan_main(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: MAIN,
            line,
            why,
        })
    };
    // tmux 一覧の取り直しの入口は、A/B のゲートの腕の外で同期の取り直しをしない
    for head in ["fn refresh_tmux(", "fn refresh_tmux_data("] {
        match fn_window(main, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                let gate = code_lines_with(&window, "issue1984_legacy()");
                for needle in [
                    "Request::TmuxList",
                    "fetch_tmux_sessions(",
                    "tmux::list_sessions(",
                ] {
                    for i in code_lines_with(&window, needle) {
                        push(
                            at + i,
                            format!(
                                "UI スレッドで走る `{head}` が `{needle}` を呼んでいる（tmux を UI スレッドで \
                                 待つ = #1984。`spawn_tmux_refresh` で background へ出す）"
                            ),
                        );
                    }
                }
                for i in code_lines_with(&window, "refresh_tmux_data_blocking()") {
                    if gate.first().is_none_or(|g| *g >= i) {
                        push(
                            at + i,
                            format!(
                                "`{head}` の同期の取り直し（`refresh_tmux_data_blocking`）が A/B のゲート \
                                 （`issue1984_legacy()`）の腕の外にある（製品の既定で UI スレッドが tmux を待つ）"
                            ),
                        );
                    }
                }
            }
        }
    }
    match fn_window(main, "fn spawn_tmux_refresh(") {
        None => push(
            0,
            "`spawn_tmux_refresh` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            for i in code_lines_with(&window, "fetch_tmux_sessions(") {
                let in_background = window[i].contains("async move")
                    || i.checked_sub(1)
                        .is_some_and(|p| window[p].contains("background_executor()"));
                if !in_background {
                    push(
                        at + i,
                        "`spawn_tmux_refresh` が `fetch_tmux_sessions` を background の外で呼んでいる \
                         （UI スレッドで tmux を待つ = #1984）"
                            .into(),
                    );
                }
            }
        }
    }
    // 再起動のボタンは同期の dispatch をしない（`dispatch_off_ui` を通す）
    for head in ["fn session_restart_button(", "fn stale_binary_restart("] {
        match fn_window(main, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                for i in code_lines_with(&window, "tako_control::dispatch(") {
                    push(
                        at + i,
                        format!(
                            "GUI のボタン `{head}` が同期の dispatch をしている（セッション再起動の計画 = \
                             `claude agents --json` + 器の pid 祖先辿りを UI スレッドで待つ = #1984。\
                             `dispatch_off_ui` を通す）"
                        ),
                    );
                }
                if code_lines_with(&window, "dispatch_off_ui(").is_empty() {
                    push(
                        at,
                        format!("GUI のボタン `{head}` が `dispatch_off_ui` を通っていない"),
                    );
                }
            }
        }
    }
    // 受け口と GUI のボタンは「もう一巡」を通る（先取りの答えが足りないまま着地しない）
    match fn_window(main, "async fn drive_offload(") {
        None => push(0, "`drive_offload` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if code_lines_with(&window, "land_offload_on_ui(").is_empty()
                || code_lines_with(&window, "OffloadLanded::Again").is_empty()
            {
                push(
                    at,
                    "`drive_offload` が着地（`land_offload_on_ui`）ともう一巡（`OffloadLanded::Again`）を \
                     持っていない（handoff の後任の agent の所在確認が先取りされない = #1984）"
                        .into(),
                );
            }
        }
    }
    let calls = main
        .lines()
        .filter(|l| {
            !is_comment(l) && l.contains("drive_offload(") && !l.contains("fn drive_offload(")
        })
        .count();
    if calls < 2 {
        push(
            0,
            format!(
                "`drive_offload` の呼び出しが {calls} 本（IPC の受け口と `dispatch_off_ui` の 2 本が要る）"
            ),
        );
    }
    out
}

/// 6. `check_qos` の `ps` が上限つき
fn scan_sleep_guard(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub fn check_qos(") {
        None => out.push(Offender {
            file: SLEEP_GUARD,
            line: 0,
            why: "`check_qos` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for needle in [".output()", "Command::new("] {
                for i in code_lines_with(&window, needle) {
                    out.push(Offender {
                        file: SLEEP_GUARD,
                        line: at + i,
                        why: format!(
                            "`check_qos` が `{needle}` を使っている（`ps` を上限なしで待つ = #1979 の本番ハングと \
                             同じ型。`probe::output_with_timeout` を通す）"
                        ),
                    });
                }
            }
            if code_lines_with(&window, "output_with_timeout(").is_empty() {
                out.push(Offender {
                    file: SLEEP_GUARD,
                    line: at,
                    why:
                        "`check_qos` の `ps` が上限つきの待ち（`probe::output_with_timeout`）でない"
                            .into(),
                });
            }
        }
    }
    out
}

/// 7. #1989: `TmuxOpen` の腕（dispatch_inner の中）は tmux を直に聞かない
fn scan_tmux_open_arm(dispatch: &str) -> Vec<Offender> {
    let lines: Vec<&str> = dispatch.lines().collect();
    // 先取りの判断（`probe_needs`）にも同じ綴りがあるので、`direction,` を持つ dispatch の腕を選ぶ
    let start = lines.iter().enumerate().position(|(i, l)| {
        *l == "        Request::TmuxOpen {"
            && lines[i..(i + 8).min(lines.len())]
                .iter()
                .any(|x| x.trim() == "direction,")
    });
    let Some(start) = start else {
        return vec![Offender {
            file: DISPATCH,
            line: 0,
            why: "`TmuxOpen` の dispatch の腕が見つからない（走査が空振り）".into(),
        }];
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.starts_with("        Request::"))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    let window: Vec<String> = lines[start..end].iter().map(|l| l.to_string()).collect();
    let mut out = Vec::new();
    if code_lines_with(&window, "tmux_open_lookup(").is_empty() {
        out.push(Offender {
            file: DISPATCH,
            line: start + 1,
            why: "`TmuxOpen` の腕が読む側の関数（`tmux_open_lookup`）を通っていない".into(),
        });
    }
    for (needle, why) in RAW {
        for i in code_lines_with(&window, needle) {
            out.push(Offender {
                file: DISPATCH,
                line: start + 1 + i,
                why: format!(
                    "UI スレッドで走る `TmuxOpen` の腕が `{needle}` を直に呼んでいる（{why}。\
                     UI スレッドの dispatch で子プロセスを待つ = #1989。`tmux_open_lookup` を通す）"
                ),
            });
        }
    }
    out
}

/// 7. #1989: agent CLI の所在確認は上限つき（上限なしの `exe::find` は A/B のゲートの腕だけ）
fn scan_locate(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub fn locate(") {
        None => out.push(Offender {
            file: AGENT_CLI,
            line: 0,
            why: "`locate` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_lines_with(&window, "find_with_timeout(").is_empty() {
                out.push(Offender {
                    file: AGENT_CLI,
                    line: at,
                    why: "`locate` が上限つきの探索（`exe::find_with_timeout`）を使っていない \
                          （ログインシェルが返らないと spawn / handoff が永久に返らない = #1989）"
                        .into(),
                });
            }
            // 上限なしの口（`exe::find` と、中で `exe::find` を起こす `resolve_binary_for`）は
            // 直前 2 行以内にゲートがある腕の中だけ
            let gate = code_lines_with(&window, "legacy_unbounded_locate()");
            for needle in ["exe::find(", "resolve_binary_for("] {
                for i in code_lines_with(&window, needle) {
                    if !gate.iter().any(|g| *g < i && i - *g <= 2) {
                        out.push(Offender {
                            file: AGENT_CLI,
                            line: at + i,
                            why: format!(
                                "`locate` の上限なしの探索（`{needle}`）が A/B のゲート \
                                 （`legacy_unbounded_locate()`）の腕の外にある（製品の既定で上限が消え、\
                                 打ち切ったあとにログインシェルをもう一度上限なしで待つ = #1989）"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

/// 7. #1989: GUI のボタンと回収の計画
fn scan_gui_1989(main: &str, right: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src, head) in [
        (MAIN, main, "fn tmux_kill_confirmed("),
        (MAIN, main, "fn drop_tmux_session("),
        (RIGHT_PANEL, right, "fn tmux_window_row_clicked("),
        (RIGHT_PANEL, right, "fn tmux_restore_clicked("),
        (RIGHT_PANEL, right, "fn git_do_resolve_agent("),
    ] {
        match fn_window(src, head) {
            None => out.push(Offender {
                file,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for needle in ["tako_control::dispatch(", "dispatch::dispatch("] {
                    for i in code_lines_with(&window, needle) {
                        out.push(Offender {
                            file,
                            line: at + i,
                            why: format!(
                                "GUI のボタン `{head}` が同期の dispatch をしている（tmux / git / CLI の \
                                 所在確認を UI スレッドで待つ = #1989。`dispatch_off_ui` を通す）"
                            ),
                        });
                    }
                }
                if code_lines_with(&window, "dispatch_off_ui(").is_empty() {
                    out.push(Offender {
                        file,
                        line: at,
                        why: format!("GUI のボタン `{head}` が `dispatch_off_ui` を通っていない"),
                    });
                }
            }
        }
    }
    for head in ["fn orphan_cleanup_plan(", "fn tmux_cleanup_job("] {
        match fn_window(main, head) {
            None => out.push(Offender {
                file: MAIN,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for needle in ["cleanup_orphans(", "cleanup_servers(", ".run()"] {
                    for i in code_lines_with(&window, needle) {
                        out.push(Offender {
                            file: MAIN,
                            line: at + i,
                            why: format!(
                                "回収の計画 `{head}`（UI スレッド）が `{needle}` を呼んでいる（器の一覧と kill を \
                                 UI スレッドで待つ = #1989。本体は background の `OrphanCleanupPlan::run`）"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

struct Sources {
    dispatch: String,
    main: String,
    sleep_guard: String,
    agent_cli: String,
    right_panel: String,
}

impl Sources {
    fn load() -> Self {
        let root = workspace_root();
        Self {
            dispatch: read(&root, DISPATCH),
            main: read(&root, MAIN),
            sleep_guard: read(&root, SLEEP_GUARD),
            agent_cli: read(&root, AGENT_CLI),
            right_panel: read(&root, RIGHT_PANEL),
        }
    }

    fn offenders(&self) -> Vec<Offender> {
        let mut v = scan_ui_paths(&self.dispatch);
        v.extend(scan_readers(&self.dispatch));
        v.extend(scan_prepare(&self.dispatch));
        v.extend(scan_background(&self.dispatch));
        v.extend(scan_main(&self.main));
        v.extend(scan_sleep_guard(&self.sleep_guard));
        v.extend(scan_tmux_open_arm(&self.dispatch));
        v.extend(scan_locate(&self.agent_cli));
        v.extend(scan_gui_1989(&self.main, &self.right_panel));
        v
    }
}

#[test]
fn ui_スレッドの_dispatch_が子プロセスを待たない() {
    let offenders = Sources::load().offenders();
    if !offenders.is_empty() {
        let list: Vec<String> = offenders.iter().map(Offender::report).collect();
        panic!(
            "UI スレッドの dispatch が子プロセスを待つ経路が残っている（#1984）:\n  {}",
            list.join("\n  ")
        );
    }
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::load();
    // 窓に本体が入っている（窓の切り方が効いている）
    for (src, head, must) in [
        (
            &s.dispatch,
            "fn resolve_caller_pane(",
            "caller_pane_by_pid(",
        ),
        (
            &s.dispatch,
            "fn dispatch_orchestrator_spawn(",
            "agent_preflight(",
        ),
        (
            &s.dispatch,
            "fn dispatch_orchestrator_spawn(",
            "caller_pane_by_pid(",
        ),
        (
            &s.dispatch,
            "fn dispatch_orchestrator_spawn(",
            "tako_binary()",
        ),
        (
            &s.dispatch,
            "fn dispatch_orchestrator_handoff(",
            "agent_preflight(",
        ),
        (
            &s.dispatch,
            "fn build_session_restart_plan(",
            "restart_scan(",
        ),
        (&s.dispatch, "fn probe_needs(", "spawn_caller_profile("),
        (&s.dispatch, "fn probe_needs(", "resolve_handoff_target("),
        (
            &s.dispatch,
            "fn caller_pane_by_pid(",
            "agents::resolve_pane_by_pid(",
        ),
        (&s.dispatch, "fn agent_preflight(", "agent_cli::preflight("),
        (
            &s.dispatch,
            "fn restart_scan(",
            "ProcessSnapshot::capture()",
        ),
        (&s.dispatch, "pub fn land_offload(", "probe_needs("),
        (&s.main, "fn spawn_tmux_refresh(", "fetch_tmux_sessions("),
        (&s.main, "fn dispatch_off_ui(", "prepare_offload("),
        (&s.sleep_guard, "pub fn check_qos(", "output_with_timeout("),
        (
            &s.dispatch,
            "fn dispatch_git_resolve_agent(",
            "git_conflict_for(",
        ),
        (
            &s.dispatch,
            "fn dispatch_git_resolve_agent(",
            "agent_preflight(",
        ),
        (&s.dispatch, "fn git_conflict_for(", "git::conflict_state("),
        (
            &s.dispatch,
            "fn tmux_open_lookup(",
            "compute_tmux_open_lookup(",
        ),
        (&s.agent_cli, "pub fn locate(", "find_with_timeout("),
        (&s.main, "fn orphan_cleanup_plan(", "protected"),
    ] {
        let (_, w) = fn_window(src, head).unwrap_or_else(|| panic!("{head} の窓"));
        assert!(
            !code_lines_with(&w, must).is_empty(),
            "{head} の窓に `{must}` が入っていない（窓の切り方が壊れた?）"
        );
    }
    // 窓が関数の終わりで閉じている（次の関数まで飲み込んでいない）
    let (_, w) = fn_window(&s.dispatch, "fn caller_pane_by_pid(").expect("読む側の窓");
    assert!(
        code_lines_with(&w, "fn agent_preflight(").is_empty(),
        "読む側の窓が次の関数を飲み込んでいる"
    );
}

/// 修正前を再現した注入が file:line で名指しされる（番犬の検出力）
#[test]
fn 逆戻りを名指しできる() {
    let s = Sources::load();
    assert!(s.offenders().is_empty(), "前提: 現行は緑");

    let expect_named = |label: &str, offenders: Vec<Offender>, file: &str, needle: &str| {
        let hit = offenders
            .iter()
            .find(|o| o.file == file && o.line > 0 && o.why.contains(needle));
        assert!(
            hit.is_some(),
            "注入「{label}」を名指しできない: {:?}",
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
        eprintln!("注入「{label}」→ {}", hit.unwrap().report());
    };
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };

    // ① 呼び出し元の解決を修正前の同期の pid 祖先辿りへ戻す
    let d = inject(
        &s.dispatch,
        "        .and_then(|pid| caller_pane_by_pid(host, pid))\n        .and_then(|p| resolve_pane(host.workspace(), Some(p)).ok())",
        "        .and_then(|pid| crate::agents::resolve_pane_by_pid(pid, &collect_pane_backends(host)))\n        .and_then(|p| resolve_pane(host.workspace(), Some(p)).ok())",
        "①",
    );
    expect_named(
        "呼び出し元の pid 祖先辿りを同期へ",
        scan_ui_paths(&d),
        DISPATCH,
        "fn resolve_caller_pane(",
    );

    // ② spawn の CLI の所在確認を修正前の preflight へ戻す
    let d = inject(
        &s.dispatch,
        // 置換元は spawn の行だけ（#1989 で同じ行が GitResolveAgent にも増えたので直前のコメントで絞る）
        "    // 先取りした答えを読む\n    let agent_cli_path = agent_preflight(worker_agent)?;",
        "    // 先取りした答えを読む\n    let agent_cli_path = orchestrator::agent_cli::preflight(worker_agent)\n        .map_err(|e| DispatchError::Operation(e.message()))?;",
        "②",
    );
    expect_named(
        "spawn の preflight を同期へ",
        scan_ui_paths(&d),
        DISPATCH,
        "fn dispatch_orchestrator_spawn(",
    );

    // ③ 再起動の計画の走査を修正前の同期の採取へ戻す
    let d = inject(
        &s.dispatch,
        "    let (live_session, snapshot) = restart_scan(backend.as_deref());",
        "    let snapshot = crate::agents::ProcessSnapshot::capture();\n    let live_session = backend.as_deref().and_then(crate::agents::resolve_session_id_for_backend);",
        "③",
    );
    expect_named(
        "再起動の計画の走査を同期へ",
        scan_ui_paths(&d),
        DISPATCH,
        "fn build_session_restart_plan(",
    );

    // ④ スマホの一覧を background のジョブにしない（修正前の prepare_offload）
    let d = inject(
        &s.dispatch,
        "            Some(Ok(OffloadJob::RemoteAgents))",
        "            None",
        "④",
    );
    expect_named(
        "RemoteAgents を同期のまま",
        scan_prepare(&d),
        DISPATCH,
        "RemoteAgents",
    );

    // ⑤ 先取りの腕を外す（spawn / handoff / self / 再起動が同期のまま）
    let d = inject(
        &s.dispatch,
        "            let needs = with_ui_probes(&UiProbes::default(), || probe_needs(host, request))?;",
        "            let needs: Option<ProbeNeeds> = None;\n            let needs = needs?;",
        "⑤",
    );
    expect_named(
        "先取りの腕を外す",
        scan_prepare(&d),
        DISPATCH,
        "probe_needs",
    );

    // ⑥ 読む側の関数が先取りの内側で答えが無いときにその場で問い合わせる
    let d = inject(
        &s.dispatch,
        "            None\n        }\n        Probed::Outside => crate::agents::resolve_pane_by_pid(",
        "            crate::agents::resolve_pane_by_pid(pid, &collect_pane_backends(host))\n        }\n        Probed::Outside => crate::agents::resolve_pane_by_pid(",
        "⑥",
    );
    expect_named(
        "読む側の Missing の腕で辿る",
        scan_readers(&d),
        DISPATCH,
        "fn caller_pane_by_pid(",
    );

    // ⑦ probe_needs から spawn を落とす
    let d = inject(
        &s.dispatch,
        "        Request::OrchestratorSpawn {\n            pane,\n            tab,\n            caller_role,\n            agent,\n            caller_pid,\n            ..\n        } => {",
        "        Request::OrchestratorSpawnX {\n            pane,\n            tab,\n            caller_role,\n            agent,\n            caller_pid,\n            ..\n        } => {",
        "⑦",
    );
    expect_named(
        "probe_needs から spawn を落とす",
        scan_prepare(&d),
        DISPATCH,
        "Request::OrchestratorSpawn",
    );

    // ⑧ 右パネルの tmux 一覧を修正前の同期の dispatch へ戻す（ゲートなし）
    let m = inject(
        &s.main,
        "        if tako_core::probe::issue1984_legacy() {\n            self.refresh_tmux_data_blocking();\n        } else {\n            self.tmux_refresh_pending = true;\n        }",
        "        let value = tako_control::dispatch(self, tako_control::protocol::Request::TmuxList { socket: None }, PaneOrigin::User);\n        let _ = value;",
        "⑧",
    );
    expect_named(
        "右パネルの tmux 一覧を同期へ",
        scan_main(&m),
        MAIN,
        "fn refresh_tmux_data(",
    );

    // ⑨ 再起動のボタンを修正前の同期の dispatch へ戻す
    let m = inject(
        &s.main,
        "    fn session_restart_button(&mut self, pane_id: PaneId, mode: &str, cx: &mut Context<Self>) {\n        self.dispatch_off_ui(",
        "    fn session_restart_button(&mut self, pane_id: PaneId, mode: &str, cx: &mut Context<Self>) {\n        let _ = tako_control::dispatch(self, req, PaneOrigin::User);\n        self.dispatch_off_ui(",
        "⑨",
    );
    expect_named(
        "再起動のボタンを同期へ",
        scan_main(&m),
        MAIN,
        "fn session_restart_button(",
    );

    // ⑪ #1989: コンフリクト解消エージェントの git を修正前の同期へ戻す
    let d = inject(
        &s.dispatch,
        "    let (repo, state) = git_conflict_for(&git_pane_cwd(host, pane)?)?;",
        "    let repo = git_repo_for_pane(host, pane)?;\n    let state = tako_core::git::conflict_state(&repo);",
        "⑪",
    );
    expect_named(
        "解消エージェントの git を同期へ",
        scan_ui_paths(&d),
        DISPATCH,
        "fn dispatch_git_resolve_agent(",
    );

    // ⑫ #1989: TmuxOpen の腕を修正前の同期の問い合わせへ戻す
    let d = inject(
        &s.dispatch,
        "            let TmuxOpenLookup { socket, group } = tmux_open_lookup(socket.as_deref(), &session)?;",
        "            let socket = tako_core::tmux::resolve_session_socket(socket.as_deref(), &session).map_err(DispatchError::Operation)?;\n            let group = tako_core::tmux::session_group(socket.as_deref(), &session);",
        "⑫",
    );
    expect_named(
        "TmuxOpen の問い合わせを同期へ",
        scan_tmux_open_arm(&d),
        DISPATCH,
        "TmuxOpen",
    );

    // ⑬ #1989: StaleBinary status を background へ出さない
    let d = inject(
        &s.dispatch,
        "            Some(collect_stale_status_ctx(host, *pane).map(|ctx| OffloadJob::StaleStatus { ctx }))",
        "            None",
        "⑬",
    );
    expect_named(
        "StaleBinary status を同期のまま",
        scan_prepare(&d),
        DISPATCH,
        "StaleBinary status",
    );

    // ⑭ #1989: agent CLI の所在確認をゲートの外の上限なしへ戻す
    let a = inject(
        &s.agent_cli,
        "        let bounded = if legacy_unbounded_locate() {",
        "        let bounded = if true {",
        "⑭",
    );
    expect_named("locate を上限なしへ", scan_locate(&a), AGENT_CLI, "ゲート");
    // ⑭' 打ち切ったあとの置き場所の確認を、PATH をもう一度探す口（上限なし）へ戻す
    let a = inject(
        &s.agent_cli,
        "            let launcher = if legacy_unbounded_locate() {",
        "            let launcher = if true {",
        "⑭'",
    );
    expect_named(
        "locate の打ち切り後に上限なしの探索",
        scan_locate(&a),
        AGENT_CLI,
        "resolve_binary_for(",
    );

    // ⑮ #1989: GUI の window 行を修正前の同期の dispatch へ戻す
    let r = inject(
        &s.right_panel,
        "        let target_label = target_label.to_string();\n        self.dispatch_off_ui(",
        "        let _ = tako_control::dispatch(self, req, PaneOrigin::User);\n        let target_label = target_label.to_string();\n        self.dispatch_off_ui(",
        "⑮",
    );
    expect_named(
        "GUI の window 行を同期へ",
        scan_gui_1989(&s.main, &r),
        RIGHT_PANEL,
        "fn tmux_window_row_clicked(",
    );

    // ⑯ #1989: 回収の計画（UI スレッド）で回収の本体を走らせる
    let m = inject(
        &s.main,
        "        Ok(tako_control::OrphanCleanupPlan {\n            target,",
        "        let _ = tako_core::tmux_backend::cleanup_orphans(&target, &protected, min_idle_secs);\n        Ok(tako_control::OrphanCleanupPlan {\n            target,",
        "⑯",
    );
    expect_named(
        "回収の計画で kill",
        scan_gui_1989(&m, &s.right_panel),
        MAIN,
        "fn orphan_cleanup_plan(",
    );

    // ⑩ check_qos を修正前の上限なしの ps へ戻す
    let g = inject(
        &s.sleep_guard,
        "        match tako_core::probe::output_with_timeout(",
        "        let _ = std::process::Command::new(\"ps\").output();\n        match tako_core::probe::output_with_timeout(",
        "⑩",
    );
    expect_named(
        "check_qos の ps を上限なしへ",
        scan_sleep_guard(&g),
        SLEEP_GUARD,
        "check_qos",
    );
}

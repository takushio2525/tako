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
//! 8. #1991: git の書き込み系（commit / pull / push / stage / unstage / checkout / branch / merge /
//!    merge-abort）と conflicts を `prepare_offload` が background のジョブ（`OffloadJob::GitWrite`）にする。
//!    UI スレッドで写すのはペインの cwd だけ（`git_pane_cwd` は `git rev-parse` を起こさない）、
//!    同期の腕と background は同じ本体（`git_write_reply`）。起動時の orphan の一括回収は計画だけ
//!    UI スレッドで、器の一覧と kill は background（同期は A/B のゲートの後ろだけ）。起動時の
//!    自動復帰が UI スレッドで呼ぶ tmux の 3 口（`session_cwd` / `session_env` / `set_session_env`）は上限つき
//! 9. #2017: `orchestrator self` の会話 ID と状態（`claude agents --json` + 器の pid 祖先辿り）は
//!    先取り（`self_scan`）に載せ、UI スレッドの dispatch は読む側の関数（`self_agent_scan`）だけを通す
//! 10. #2031: 右パネル tasks の一覧更新（2 秒ループ・タブを開いた瞬間・「完了も表示」）は
//!     `spawn_user_tasks_list` を通り、置き場の読みと解釈（`user_task_list_load` / `parse_list`）は
//!     background のブロックの中だけ。UI スレッドの観測（`user_task_list_observe`）は置き場に触らない。
//!     読みは置き場の版の比較で省かれ（版は**読む前に**採る）、同期の引き直しは A/B のゲートの後ろだけ
//! 11. #2052: IPC（CLI / MCP / PWA）の `tako todo list` と完了・却下（IPC と右パネルのボタン）は
//!     `prepare_offload` の腕（`user_task_offload_prepare`）で background のジョブになる。UI スレッドの段
//!     （準備・送達フローの観測・畳む着地）は置き場に触らず、読み書きは background の段だけ。
//!     右パネルのボタンは `dispatch_off_ui` を通り、コマンドパレット（`toggle_panel_view`）と
//!     `tako panel`（`set_panel` → `after_dispatch`）は tasks を開いた瞬間に一覧を background で引き直す
//! 12. #2086: IPC の起票・詳細・更新・返答（PWA を含む）・展開 / 折りたたみも同じ腕（門は
//!     `user_task_offloads`、A/B は `TAKO_2086_LEGACY=1`）。UI スレッドの段（準備・起票元の材料の写し・
//!     通知・配送・展開の着地）は置き場にも起票元の解決（プロファイル・セッションカタログ）にも触らず、
//!     返答は届いた順の札（`RespondTurn`）の番を待ってから積み、配送の記録は着地の後の background
//!     （`OffloadLanding::Settled` → 受け口が後処理を済ませてから出す）。同期の腕も同じ段を順に呼ぶ。
//!     AI からの展開で一覧に居ない行の引き直しは background（同期は A/B のゲートの後ろだけ）。
//!     MCP の返答は経路を埋めて送る（省くと準備の段が呼び出し元を知らず同期の腕に残る）
//! 13. #2014: プレビューの変更履歴を開く切替（IPC の `PreviewChangelog` と GUI の「履歴」ボタン）は
//!     `prepare_offload` の腕で background のジョブ（`OffloadJob::PreviewChangelog`）になる。UI スレッドの段
//!     （準備 = プレビューのパスの写し・着地 = 状態へ置く）は git を起こさず、background の本体
//!     （`PreviewChangelogJob::run`）は上限つきの `git::file_history` だけを通す。同期の腕
//!     （テスト・直呼び・A/B）も同じ本体を通り、上限なしは A/B のゲートの腕だけ。tako-app の host
//!     （`set_preview_changelog`）とボタン（`toggle_preview_changelog`）は git を起こさず、ボタンは
//!     `dispatch_off_ui` を通る
//! 14. #2150: 変更履歴の行の diff の展開（IPC の `PreviewChangelog { expand }` と GUI のコミット行の
//!     クリック）も同じ形。`prepare_offload` の腕で background のジョブ（`OffloadJob::PreviewChangelogDiff`）に
//!     なり、UI スレッドの段（準備 = 変更履歴を開いたときの値の写し・着地 = 置く）は git を起こさず、
//!     background の本体（`PreviewChangelogDiffJob::run`）は上限つきの `git::commit_file_diff` だけを
//!     変更履歴と同じ上限（`changelog_timeout()`）で通す。畳むのと同期の腕（テスト・直呼び・A/B）も
//!     同じ準備・着地を通り、上限なしは A/B のゲートの腕だけ。tako-app の host（`changelog_diff_target` /
//!     `set_changelog_diff`）とクリック（`toggle_changelog_diff`）は git を起こさず、クリックは
//!     `dispatch_off_ui` を通る
//! 15. #2042: 1〜14 は窓の中の**直書き**しか見ないので、口を別名の関数で包むと見逃す（#2017 の
//!     `orchestrator self` は `resolve_session_id_for_pane_via_host` 越しに `claude agents --json` を
//!     UI スレッドで走らせていたが、緑のままだった）。ここは UI スレッドの入口（[`UI_PATHS`] の関数 +
//!     `prepare_offload` / `finish_offload`）から**呼び出しを辿って**届く関数を集め（`common/call_graph.rs`）、
//!     その本体に子プロセスの口（[`RAW`] + [`GRAPH_LEAVES`]）があれば、入口の中で次の段を呼んだ行を
//!     file:line で名指しする。辿らないもの（先取りの外側の腕・A/B の旧経路の腕・別スレッド・
//!     集約点の `dispatch`・既知の残り [`GRAPH_KNOWN`]）は理由つきで宣言する
//! 16. #2182: 返答の配送で起票 master が居ないときの起動の組み立て（`master_launch::plan` = CLI の
//!     所在確認のログインシェル・tako の実行ファイルの解決）は background のジョブ
//!     （`OffloadJob::UserTaskLaunch`）。③ 運ぶ（`user_task_respond_deliver`。UI）は宛先を決めるだけで、
//!     起動が要れば `UserTaskRespondStep::Launch` を返し、`land_offload` がもう一巡 background へ出し、
//!     起こすのは着地（`user_task_launch_land`。UI）。右パネルの「返す」も同じ段を `background_executor()`
//!     で回す。15 節の既知の残りだった辺はこれで消えた。組み立ての呼び出しはグラフが辿るので、ここは
//!     グラフの読まない形（UI スレッドの関数の `job.run()`・tako-app）を見る。A/B は `TAKO_2182_LEGACY=1`
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**が file:line で名指しされることを確かめる。15 節は
//! [`呼び出しグラフが空振りしていない`]（実コードで各解決の形と辿らない領域が効いている）・
//! [`呼び出しグラフの読み方`]（合成した小さなクレートで 1 形ずつ）・[`包んだ口を名指しできる`]
//! （包んだ口の注入を 1〜14 は見逃し、15 節が名指しする = 同じテストの中の A/B）で固定する。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const SLEEP_GUARD: &str = "crates/tako-control/src/sleep_guard.rs";
const AGENT_CLI: &str = "crates/tako-control/src/orchestrator/agent_cli.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";
const TASKS_PANEL: &str = "crates/tako-app/src/tasks_panel.rs";
const TMUX_BACKEND: &str = "crates/tako-core/src/tmux_backend.rs";
const STATUS_BAR: &str = "crates/tako-app/src/status_bar.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";

/// #1991: background のジョブ（`OffloadJob::GitWrite`）にする git の要求（`prepare_offload` の腕に
/// 全部並んでいること。1 つでも漏れると、その要求は同期の腕 = UI スレッドで git を待つ）
const GIT_WRITES: &[&str] = &[
    "Request::GitCommit",
    "Request::GitPull",
    "Request::GitPush",
    "Request::GitStage",
    "Request::GitUnstage",
    "Request::GitCheckout",
    "Request::GitBranchCreate",
    "Request::GitMerge",
    "Request::GitMergeAbort",
    "Request::GitConflicts",
];

/// 子プロセスを待つ口（右は名指しの理由）
const RAW: &[(&str, &str)] = &[
    // #1993: PC リソースの走査（libproc で 1 回 wall 11 ms）。UI は `cached_status` で覚えを読む
    ("resources::answer(", "PC リソースの走査（#1993）"),
    (
        "platform::resources::",
        "PC リソースの OS への問い合わせ（#1993）",
    ),
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
    // #2017: `orchestrator self` は会話 ID の解決と状態の照会を UI スレッドで直に呼んでいた
    // （`resolve_session_id_for_backend` を包んだ口なので上の綴りでは拾えなかった）
    (
        "resolve_session_id_for_pane_via_host(",
        "`claude agents --json` + 器の pid 祖先辿り",
    ),
    ("query_agent_status(", "`claude agents --json`"),
    // #2014: プレビューの変更履歴（`git log --follow` は上限なし。`file_history` は上限つきだが、
    // 待つのは background の `PreviewChangelogJob::run` だけ）
    ("git::log_file_commits(", "`git log --follow`（上限なし）"),
    (
        "git::file_history(",
        "`git rev-parse` + `git log --follow`（上限つきでも UI スレッドで待たない）",
    ),
    // #2150: 変更履歴の行の展開（`diff_file_commit` は上限なし。`commit_file_diff` は上限つきだが、
    // 待つのは background の `PreviewChangelogDiffJob::run` だけ）
    (
        "git::diff_file_commit(",
        "`git diff` / `git diff-tree`（上限なし）",
    ),
    (
        "git::commit_file_diff(",
        "`git diff` / `git diff-tree`（上限つきでも UI スレッドで待たない）",
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
    // #1991
    "fn git_pane_cwd(",
    // #2014
    "fn preview_changelog_prepare(",
    "fn preview_changelog_land(",
    // #2150
    "fn preview_changelog_diff_prepare(",
    "fn preview_changelog_diff_land(",
    // #2182: 返答の配送（③ 運ぶ）と、組み立てた起動で起こす着地
    "pub fn user_task_respond_deliver(",
    "pub fn user_task_launch_land(",
    "fn launch_master_for_response(",
];

/// 読む側の関数（子プロセスの口は `Probed::Outside` の腕でだけ呼ぶ）
const READERS: &[&str] = &[
    "fn caller_pane_by_pid(",
    "fn agent_preflight(",
    "fn tako_binary(",
    "fn restart_scan(",
    "fn git_conflict_for(",
    "fn tmux_open_lookup(",
    // #2017
    "fn self_agent_scan(",
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
    scan_ui_paths_with(dispatch, RAW)
}

/// [`scan_ui_paths`] の口の一覧を渡す版（#2042: #2017 より前の一覧で見逃したことを再現する）
fn scan_ui_paths_with(dispatch: &str, raw: &[(&str, &str)]) -> Vec<Offender> {
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
        for (needle, why) in raw {
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

/// 8. #1991: git の書き込み系は background のジョブで、同期の腕と同じ本体を通る
fn scan_git_write(dispatch: &str) -> Vec<Offender> {
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
            let job = code_lines_with(&window, "OffloadJob::GitWrite {");
            if job.is_empty() {
                push(
                    at,
                    "`prepare_offload` が git の書き込み系を background のジョブ（`OffloadJob::GitWrite`）に \
                     していない（commit / pull / push を UI スレッドで待つ = #1991）"
                        .into(),
                );
            }
            // 腕の頭（`Request::GitCommit` の行）から job の行までが git の腕
            let head = window
                .iter()
                .position(|l| !is_comment(l) && l.trim_start().starts_with("Request::GitCommit {"));
            match (head, job.first()) {
                (Some(head), Some(&job)) if head < job => {
                    let arm = &window[head..job];
                    for req in GIT_WRITES {
                        // 変種名の後ろの ` {` まで照合する（`GitMerge` で `GitMergeAbort` を拾わない）
                        if code_lines_with(arm, &format!("{req} {{")).is_empty() {
                            push(
                                at + head,
                                format!(
                                    "`prepare_offload` の git の腕に `{req}` が無い（その要求は同期の腕へ落ちて \
                                     UI スレッドで git を待つ = #1991）"
                                ),
                            );
                        }
                    }
                    if code_lines_with(arm, "issue1991_legacy()").is_empty() {
                        push(
                            at + head,
                            "`prepare_offload` の git の腕に A/B のゲート（`issue1991_legacy()`）が無い".into(),
                        );
                    }
                    // UI スレッドで写すのは cwd だけ（`git rev-parse` を起こさない）
                    if !window[job.saturating_sub(1)..=job]
                        .iter()
                        .any(|l| l.contains("git_pane_cwd("))
                    {
                        push(
                            at + job,
                            "git の腕が cwd を `git_pane_cwd` で写していない（UI スレッドでリポジトリを \
                             解決する形へ戻る = #1991）"
                                .into(),
                        );
                    }
                }
                _ => push(
                    at,
                    "`prepare_offload` の git の腕（`Request::GitCommit { … } … OffloadJob::GitWrite`）が \
                     見つからない"
                        .into(),
                ),
            }
        }
    }
    match fn_window(dispatch, "fn run_reply(self)") {
        None => push(0, "`run_reply` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if code_lines_with(&window, "git_write_reply(").is_empty() {
                push(
                    at,
                    "background の `run_reply` が `git_write_reply` を呼んでいない（ジョブにしたのに応答を組めない）"
                        .into(),
                );
            }
        }
    }
    // 同期の腕（テスト・直呼び・A/B）は同じ本体を通る。git を直に起こす 2 本目の実装を作らない
    let lines: Vec<&str> = dispatch.lines().collect();
    match lines
        .iter()
        .position(|l| l.trim_start().starts_with("request @ (Request::GitCommit {"))
    {
        None => push(
            0,
            "dispatch_inner の git の同期の腕（`request @ (Request::GitCommit { … }`）が見つからない".into(),
        ),
        Some(start) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, l)| l.starts_with("        Request::"))
                .map(|(i, _)| i)
                .unwrap_or(lines.len());
            let arm: Vec<String> = lines[start..end].iter().map(|l| l.to_string()).collect();
            if code_lines_with(&arm, "git_write_reply(").is_empty() {
                push(
                    start + 1,
                    "git の同期の腕が本体の 1 実装（`git_write_reply`）を通っていない".into(),
                );
            }
            for (needle, why) in RAW {
                for i in code_lines_with(&arm, needle) {
                    push(
                        start + 1 + i,
                        format!(
                            "git の同期の腕が `{needle}` を直に呼んでいる（{why}。本体を 2 本にしない = \
                             `git_write_reply` を通す。#1991）"
                        ),
                    );
                }
            }
        }
    }
    out
}

/// 8. #1991: 起動時の orphan の一括回収は計画だけ UI スレッド、本体は background
fn scan_startup_cleanup(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: MAIN,
            line,
            why,
        })
    };
    // 修正前の形（掃除本体を起動の中で同期に呼ぶ）は、明示の回収の同期の腕の外では呼ばない
    let explicit = fn_window(main, "fn cleanup_orphan_tmux(");
    for (i, l) in main.lines().enumerate() {
        if is_comment(l) || !l.contains("cleanup_orphan_tmux_with(") || l.contains("fn ") {
            continue;
        }
        let inside_explicit = explicit
            .as_ref()
            .is_some_and(|(at, w)| i + 1 >= *at && i + 1 < *at + w.len());
        if !inside_explicit {
            push(
                i + 1,
                "`cleanup_orphan_tmux_with`（器の一覧と kill を同期に待つ）を明示の回収の同期の腕の外で \
                 呼んでいる（起動時の回収を UI スレッドで待つ形へ戻る = #1991。計画 `orphan_cleanup_plan` + \
                 background の `run`）"
                    .into(),
            );
        }
    }
    match fn_window(main, "match app.orphan_cleanup_plan(") {
        None => push(
            0,
            "起動時の回収（`match app.orphan_cleanup_plan(`）が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "CleanupScope::Startup").is_empty() {
                push(
                    at,
                    "起動時の回収の窓に `CleanupScope::Startup` が無い（窓違い）".into(),
                );
            }
            let gate = code_lines_with(&window, "issue1991_legacy()");
            let runs = code_lines_with(&window, ".run()");
            let off_ui: Vec<usize> = runs
                .iter()
                .copied()
                .filter(|&i| window[i].contains("async move"))
                .collect();
            if off_ui.is_empty() {
                push(
                    at,
                    "起動時の回収の本体（`plan.run()`）を background（`spawn(async move …)`）で走らせていない \
                     （器の一覧と kill を UI スレッドで待つ = #1991）"
                        .into(),
                );
            }
            for i in runs.iter().copied().filter(|i| !off_ui.contains(i)) {
                if gate.first().is_none_or(|g| *g >= i) {
                    push(
                        at + i,
                        "起動時の回収の同期の `run()` が A/B のゲート（`issue1991_legacy()`）の後ろに無い \
                         （製品の既定で UI スレッドが器の一覧と kill を待つ = #1991）"
                            .into(),
                    );
                }
            }
        }
    }
    out
}

/// 8. #1991: 起動時の自動復帰が UI スレッドで呼ぶ tmux の 3 口は上限つき
fn scan_tmux_backend(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for head in [
        "pub fn session_cwd(",
        "pub fn session_env(",
        "pub fn set_session_env(",
    ] {
        match fn_window(src, head) {
            None => out.push(Offender {
                file: TMUX_BACKEND,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for needle in [".output()", ".status()", "Command::new("] {
                    for i in code_lines_with(&window, needle) {
                        out.push(Offender {
                            file: TMUX_BACKEND,
                            line: at + i,
                            why: format!(
                                "`{head}` が `{needle}` で器を待っている（上限なし。起動時の自動復帰が \
                                 UI スレッドで呼ぶ = #1991。上限つきの `tmux::run_tmux` を通す）"
                            ),
                        });
                    }
                }
                if code_lines_with(&window, "run_tmux(").is_empty() {
                    out.push(Offender {
                        file: TMUX_BACKEND,
                        line: at,
                        why: format!("`{head}` が上限つきの `tmux::run_tmux` を通っていない"),
                    });
                }
            }
        }
    }
    out
}

/// 9. #2031: 右パネル tasks の一覧更新は置き場を UI スレッドで読まない
fn scan_user_tasks_list(tasks: &str, dispatch: &str, right: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    // 2 秒ループの入口は background の入口だけを呼ぶ（同期の引き直し = 修正前の形へ戻さない）
    match fn_window(tasks, "pub(crate) fn tick_user_tasks(") {
        None => push(
            TASKS_PANEL,
            0,
            "`tick_user_tasks` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "spawn_user_tasks_list(").is_empty() {
                push(
                    TASKS_PANEL,
                    at,
                    "2 秒ごとの一覧更新（`tick_user_tasks`）が background の入口（`spawn_user_tasks_list`）を \
                     通っていない = #2031"
                        .into(),
                );
            }
            for needle in [
                "refresh_user_tasks(",
                "tako_control::dispatch(",
                "user_task_list_load(",
            ] {
                for i in code_lines_with(&window, needle) {
                    push(
                        TASKS_PANEL,
                        at + i,
                        format!(
                            "2 秒ごとの一覧更新（`tick_user_tasks`）が `{needle}` を UI スレッドで呼んでいる \
                             （置き場の読みと解釈を UI スレッドで待つ。本番で 1 回 551 ms = #2031）"
                        ),
                    );
                }
            }
        }
    }
    // background の入口: 読みと解釈は background のブロック（`background_executor()` から
    // 着地の `cx.spawn(` まで）の中だけ。同期の引き直しは A/B のゲートの腕の中だけ
    match fn_window(tasks, "pub(crate) fn spawn_user_tasks_list(") {
        None => push(
            TASKS_PANEL,
            0,
            "`spawn_user_tasks_list` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let bg = code_lines_with(&window, "background_executor()");
            let land = code_lines_with(&window, "cx.spawn(");
            match (bg.first(), land.first()) {
                (Some(&bg), Some(&land)) if bg < land => {
                    for needle in ["user_task_list_load(", "parse_list("] {
                        let hits = code_lines_with(&window, needle);
                        if hits.is_empty() {
                            push(
                                TASKS_PANEL,
                                at,
                                format!("`spawn_user_tasks_list` が `{needle}` を呼んでいない（窓違い）"),
                            );
                        }
                        for i in hits.into_iter().filter(|i| !(bg..land).contains(i)) {
                            push(
                                TASKS_PANEL,
                                at + i,
                                format!(
                                    "`spawn_user_tasks_list` が `{needle}` を background のブロックの外 \
                                     （= UI スレッド）で呼んでいる（置き場の読みと解釈を UI スレッドで待つ = #2031）"
                                ),
                            );
                        }
                    }
                    if code_lines_with(&window, "user_task_list_observe(")
                        .first()
                        .is_none_or(|&i| i >= bg)
                    {
                        push(
                            TASKS_PANEL,
                            at,
                            "`spawn_user_tasks_list` が配送の観測（`user_task_list_observe`）を UI スレッドで \
                             済ませていない（host の送達フローを読まないと配送が `sent` のまま固まる）"
                                .into(),
                        );
                    }
                }
                _ => push(
                    TASKS_PANEL,
                    at,
                    "`spawn_user_tasks_list` に background のブロック（`background_executor()` → 着地の \
                     `cx.spawn(`）が無い（置き場を UI スレッドで読む = #2031）"
                        .into(),
                ),
            }
            let gate = code_lines_with(&window, "legacy_2031()");
            for needle in ["refresh_user_tasks(", "tako_control::dispatch("] {
                for i in code_lines_with(&window, needle) {
                    if !gate.iter().any(|g| *g < i && i - *g <= 2) {
                        push(
                            TASKS_PANEL,
                            at + i,
                            format!(
                                "`spawn_user_tasks_list` の同期の引き直し（`{needle}`）が A/B のゲート \
                                 （`legacy_2031()`）の腕の外にある（製品の既定で UI スレッドが置き場を読む = #2031）"
                            ),
                        );
                    }
                }
            }
        }
    }
    // 観測（UI スレッド）は置き場に触らない
    match fn_window(dispatch, "pub fn user_task_list_observe(") {
        None => push(
            DISPATCH,
            0,
            "`user_task_list_observe` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            for needle in [
                "load_from(",
                "list_at(",
                "reconcile_at(",
                "store_stamp(",
                "std::fs::",
            ] {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "UI スレッドで走る `user_task_list_observe` が `{needle}` で置き場に触っている \
                             （観測は host の送達フローを読むだけ。読み書きは background の \
                             `user_task_list_load` = #2031）"
                        ),
                    );
                }
            }
        }
    }
    // 読みは版の比較で省かれる。版は読む前に採る（読んだ後に採ると、その間の書き込みを取り逃がす）
    match fn_window(dispatch, "pub fn user_task_list_load(") {
        None => push(
            DISPATCH,
            0,
            "`user_task_list_load` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            for needle in ["ControlHost", "host."] {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "background で走る `user_task_list_load` が `{needle}` に触っている \
                             （host は UI スレッドのもの。観測は `user_task_list_observe` へ）"
                        ),
                    );
                }
            }
            if code_lines_with(&window, "cache.stamp == stamp").is_empty() {
                push(
                    DISPATCH,
                    at,
                    "`user_task_list_load` が置き場の版を前回と比べていない（版が同じでも 2 秒ごとに \
                     YAML を読み直す = #2031）"
                        .into(),
                );
            }
            let stamp = code_lines_with(&window, "store_stamp(");
            let load = code_lines_with(&window, "load_from(");
            match (stamp.first(), load.first()) {
                (Some(&s), Some(&l)) if s < l => {}
                (_, Some(&l)) => push(
                    DISPATCH,
                    at + l,
                    "`user_task_list_load` が置き場を版（`store_stamp`）より先に読んでいる（読んだ後に版を \
                     採ると、その間の書き込みを新しい版のまま古い中身で覚えて取り逃がす = #2031）"
                        .into(),
                ),
                _ => push(
                    DISPATCH,
                    at,
                    "`user_task_list_load` に版の採取（`store_stamp`）と読み（`load_from`）が揃っていない".into(),
                ),
            }
        }
    }
    // 画面を触った直後の引き直し（タブを開いた瞬間・「完了も表示」）も background の入口
    for (file, src, anchor) in [
        (RIGHT_PANEL, right, "this.panel_view = PanelView::Tasks;"),
        (
            TASKS_PANEL,
            tasks,
            "this.user_tasks.show_done = !this.user_tasks.show_done;",
        ),
    ] {
        let lines: Vec<&str> = src.lines().collect();
        let Some(start) = lines
            .iter()
            .position(|l| !is_comment(l) && l.contains(anchor))
        else {
            push(
                file,
                0,
                format!("`{anchor}` が見つからない（走査が空振り）"),
            );
            continue;
        };
        let near: Vec<String> = lines[start..(start + 6).min(lines.len())]
            .iter()
            .map(|l| l.to_string())
            .collect();
        for i in code_lines_with(&near, "refresh_user_tasks(") {
            push(
                file,
                start + 1 + i,
                format!(
                    "`{anchor}` の直後の引き直しが同期（`refresh_user_tasks`）に戻っている（置き場を UI \
                     スレッドで読む = #2031。`spawn_user_tasks_list` を通す）"
                ),
            );
        }
        if code_lines_with(&near, "spawn_user_tasks_list(").is_empty() {
            push(
                file,
                start + 1,
                format!(
                    "`{anchor}` の直後に background の引き直し（`spawn_user_tasks_list`）が無い"
                ),
            );
        }
    }
    out
}

/// 置き場（`user-tasks.yaml`）に触る口。UI スレッドの段がこれを呼ぶと置き場の読み書きを待つ（#2052）
const USER_TASK_STORE: &[&str] = &[
    "load_from(",
    "list_at(",
    "show_at(",
    "set_status_at(",
    "mutate_at(",
    "reconcile_at(",
    "reconcile_user_task_flows(",
    "user_task_list_reply(",
    "user_task_status_store(",
    "store_stamp(",
    "std::fs::",
];

/// 11. #2052: IPC の一覧と完了・却下は置き場を UI スレッドで読み書きしない
fn scan_user_tasks_ipc(dispatch: &str, tasks: &str, status_bar: &str, main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    // `prepare_offload` の腕: 門（`user_task_offloads`）を通った要求を background の準備へ渡し、
    // 最後の先取りの腕（`_ if …`）より前にある（後ろだと先取りの腕が拾って同期の dispatch になる）
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => push(
            DISPATCH,
            0,
            "`prepare_offload` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let arm = code_lines_with(&window, "Request::UserTask {");
            let call = code_lines_with(&window, "user_task_offload_prepare(");
            let catch_all = code_lines_with(&window, "_ if !tako_core::probe::issue1984_legacy()");
            match (arm.first(), call.first()) {
                (Some(&a), Some(&c)) if a < c && c - a <= 6 => {
                    let guard = window[a..c].join("\n");
                    if !guard.contains("user_task_offloads(") {
                        push(
                            DISPATCH,
                            at + a,
                            "`prepare_offload` の UserTask の腕が門（`user_task_offloads`）を通っていない \
                             （A/B のゲートと経路を省いた返答の扱いが腕ごとに割れる = #2052 / #2086）"
                                .into(),
                        );
                    }
                    if catch_all.first().is_some_and(|&k| k < a) {
                        push(
                            DISPATCH,
                            at + a,
                            "`prepare_offload` の UserTask の腕が先取りの腕（`_ if …`）より後ろにある \
                             （先取りの腕が拾って同期の dispatch になる = #2052）"
                                .into(),
                        );
                    }
                }
                _ => push(
                    DISPATCH,
                    at,
                    "`prepare_offload` が `tako todo list` / 完了・却下を background の準備 \
                     （`user_task_offload_prepare`）へ渡していない（UI スレッドの同期の dispatch で \
                     置き場を読み書きする = #2052）"
                        .into(),
                ),
            }
        }
    }
    // 門: 一覧・完了・却下は #2052 の A/B の軸（`issue2052_legacy()`）で background へ出す
    match fn_window(dispatch, "fn user_task_offloads(") {
        None => push(
            DISPATCH,
            0,
            "`user_task_offloads` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => match code_lines_with(&window, "issue2052_legacy()").first() {
            None => push(
                DISPATCH,
                at,
                "門（`user_task_offloads`）に A/B のゲート（`issue2052_legacy()`）が無い".into(),
            ),
            Some(&i) => {
                for action in ["\"list\"", "\"done\"", "\"dismiss\""] {
                    if !window[i].contains(action) {
                        push(
                            DISPATCH,
                            at + i,
                            format!(
                                "門（`user_task_offloads`）の #2052 の腕が {action} を挙げていない \
                                 （A/B の軸がずれる・その要求が UI スレッドの同期の dispatch へ戻りうる = #2052）"
                            ),
                        );
                    }
                }
            }
        },
    }
    // UI スレッドの段（準備・観測・畳む着地）は置き場に触らない
    for head in [
        "fn user_task_offload_prepare(",
        "pub fn user_task_flows_observe(",
        "fn user_task_status_finish(",
    ] {
        let Some((at, window)) = fn_window(dispatch, head) else {
            push(
                DISPATCH,
                0,
                format!("`{head}` が見つからない（走査が空振り）"),
            );
            continue;
        };
        for needle in USER_TASK_STORE {
            for i in code_lines_with(&window, needle) {
                push(
                    DISPATCH,
                    at + i,
                    format!(
                        "UI スレッドで走る `{head}` が `{needle}` で置き場に触っている（読み書きは \
                         background の段 = `user_task_list_offload` / `user_task_status_store` へ。#2052）"
                    ),
                );
            }
        }
    }
    // background の段は host に触らず、置き場の読み書きを実際に行う
    for (head, must) in [
        ("fn user_task_list_offload(", "user_task_list_reply("),
        ("fn user_task_status_store(", "set_status_at("),
    ] {
        let Some((at, window)) = fn_window(dispatch, head) else {
            push(
                DISPATCH,
                0,
                format!("`{head}` が見つからない（走査が空振り）"),
            );
            continue;
        };
        for needle in ["ControlHost", "host."] {
            for i in code_lines_with(&window, needle) {
                push(
                    DISPATCH,
                    at + i,
                    format!(
                        "background で走る `{head}` が `{needle}` に触っている（host は UI スレッドのもの。\
                         観測は `user_task_flows_observe`、畳むのは `user_task_status_finish` へ）"
                    ),
                );
            }
        }
        for needle in ["reconcile_user_task_flows(", must] {
            if code_lines_with(&window, needle).is_empty() {
                push(
                    DISPATCH,
                    at,
                    format!(
                        "background の `{head}` が `{needle}` を呼んでいない（配送が `sent` のまま固まるか、\
                         読み書きが UI スレッドへ戻った = #2052）"
                    ),
                );
            }
        }
    }
    // 右パネルのボタン: IPC と同じ `dispatch_off_ui` を通り、同期の引き直しは A/B のゲートの後ろだけ
    match fn_window(tasks, "pub(crate) fn user_task_set_status(") {
        None => push(
            TASKS_PANEL,
            0,
            "`user_task_set_status` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "dispatch_off_ui(").is_empty() {
                push(
                    TASKS_PANEL,
                    at,
                    "完了・却下のボタン（`user_task_set_status`）が `dispatch_off_ui` を通っていない \
                     （書き換えを UI スレッドで待つ = #2052）"
                        .into(),
                );
            }
            for i in code_lines_with(&window, "tako_control::dispatch(") {
                push(
                    TASKS_PANEL,
                    at + i,
                    "完了・却下のボタン（`user_task_set_status`）が `tako_control::dispatch(` を UI スレッドで \
                     呼んでいる（YAML の書き換えを待つ = #2052）"
                        .into(),
                );
            }
            let gate = code_lines_with(&window, "issue2052_legacy()");
            for i in code_lines_with(&window, "refresh_user_tasks(") {
                if !gate.iter().any(|g| *g < i && i - *g <= 2) {
                    push(
                        TASKS_PANEL,
                        at + i,
                        "完了・却下の後の引き直し（`refresh_user_tasks`）が A/B のゲート（`issue2052_legacy()`）の \
                         腕の外にある（置き場を UI スレッドで読む = #2052。`spawn_user_tasks_list` を通す）"
                            .into(),
                    );
                }
            }
            if code_lines_with(&window, "spawn_user_tasks_list(").is_empty() {
                push(
                    TASKS_PANEL,
                    at,
                    "完了・却下の後に background の引き直し（`spawn_user_tasks_list`）が無い"
                        .into(),
                );
            }
        }
    }
    // コマンドパレット（`panel-tasks`）と `tako panel` は tasks を開いた瞬間に background で引き直す
    for (file, src, head, must) in [
        (
            STATUS_BAR,
            status_bar,
            "pub(crate) fn toggle_panel_view(",
            "spawn_user_tasks_list(",
        ),
        (MAIN, main, "fn set_panel(", "list_poll.pending = true"),
        (MAIN, main, "fn after_dispatch(", "spawn_user_tasks_list("),
    ] {
        let Some((at, window)) = fn_window(src, head) else {
            push(file, 0, format!("`{head}` が見つからない（走査が空振り）"));
            continue;
        };
        if code_lines_with(&window, must).is_empty() {
            push(
                file,
                at,
                format!(
                    "`{head}` に `{must}` が無い（tasks を開いても一覧を引き直さず、最大 2 秒古い一覧が出る = #2052）"
                ),
            );
        }
        for i in code_lines_with(&window, "refresh_user_tasks(") {
            push(
                file,
                at + i,
                format!(
                    "`{head}` が同期の引き直し（`refresh_user_tasks`）を呼んでいる（置き場を UI スレッドで読む = #2052）"
                ),
            );
        }
    }
    out
}

/// 残りの action の置き場の口と、起票元の解決（プロファイル・セッションカタログの読み）の口（#2086）。
/// UI スレッドの段がこれを呼ぶと、置き場か設定ファイルの読み書きを待つ
const USER_TASK_REST: &[&str] = &[
    "add_at(",
    "update_at(",
    "respond_at(",
    "record_delivery_at(",
    "user_task_add_store(",
    "respond_store_at(",
    "user_task_expand_check(",
    "user_task_respond_finish(",
    "user_task_respond_record(",
    "user_task_origin(",
    "profile_governing_project",
    "resolve_session_for_pane(",
];

/// 12. #2086: IPC の起票・詳細・更新・返答・展開 / 折りたたみも置き場を UI スレッドで読み書きしない
fn scan_user_tasks_ipc_rest(dispatch: &str, main: &str, mcp: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    let missing = |head: &str| format!("`{head}` が見つからない（走査が空振り）");
    // 門: 残りの action は #2086 の A/B の軸で background へ出る（同期の腕へ倒すのはゲートだけ）
    match fn_window(dispatch, "fn user_task_offloads(") {
        None => push(DISPATCH, 0, missing("fn user_task_offloads(")),
        Some((at, window)) => {
            if code_lines_with(&window, "_ => !tako_core::probe::issue2086_legacy()").is_empty() {
                push(
                    DISPATCH,
                    at,
                    "門（`user_task_offloads`）が起票・詳細・更新・返答・展開を `issue2086_legacy()` の \
                     ゲートで background へ出していない（UI スレッドの同期の dispatch で置き場を読み書きする = #2086）"
                        .into(),
                );
            }
        }
    }
    // 準備: 残りの action を全部受け（受けないと門を通った要求が「不明な action」で断られる）、
    // 返答の札は準備（= 届いた順）で配る
    match fn_window(dispatch, "fn user_task_offload_prepare(") {
        None => push(DISPATCH, 0, missing("fn user_task_offload_prepare(")),
        Some((at, window)) => {
            for action in [
                "\"show\"",
                "\"update\"",
                "\"add\"",
                "\"respond\"",
                "\"expand\"",
                "\"collapse\"",
            ] {
                if code_lines_with(&window, action).is_empty() {
                    push(
                        DISPATCH,
                        at,
                        format!(
                            "準備（`user_task_offload_prepare`）が {action} を受けていない（門を通った要求が \
                             「不明な action」で断られる = #2086）"
                        ),
                    );
                }
            }
            if code_lines_with(&window, "RespondTurn::take()").is_empty() {
                push(
                    DISPATCH,
                    at,
                    "準備（`user_task_offload_prepare`）が返答の札（`RespondTurn::take()`）を配っていない \
                     （届いた順に積む・運ぶ・記録する、が保たれない = #2086）"
                        .into(),
                );
            }
        }
    }
    // UI スレッドの段は置き場にも起票元の解決（プロファイル・セッションカタログ）にも触らない
    for head in [
        "fn user_task_offload_prepare(",
        "fn user_task_origin_facts(",
        "fn user_task_add_finish(",
        "fn user_task_expand_finish(",
        "pub fn user_task_respond_deliver(",
    ] {
        let Some((at, window)) = fn_window(dispatch, head) else {
            push(DISPATCH, 0, missing(head));
            continue;
        };
        for needle in USER_TASK_STORE.iter().chain(USER_TASK_REST) {
            for i in code_lines_with(&window, needle) {
                push(
                    DISPATCH,
                    at + i,
                    format!(
                        "UI スレッドで走る `{head}` が `{needle}` で置き場か設定ファイルに触っている（読み書きと \
                         起票元の解決は background の段 = `user_task_store_staged` へ。#2086）"
                    ),
                );
            }
        }
    }
    // background の段は host に触らず、読み書きを実際に行う
    for (head, musts) in [
        (
            "fn user_task_store_staged(",
            &[
                "reconcile_user_task_flows(",
                "show_at(",
                "update_at(",
                "user_task_add_store(",
                "respond_store_at(",
                "user_task_expand_check(",
            ][..],
        ),
        (
            "fn user_task_add_store(",
            &["add_at(", "user_task_origin("][..],
        ),
        (
            "fn user_task_origin(",
            &["profile_governing_project", "resolve_session_for_pane("][..],
        ),
        (
            "fn user_task_respond_record(",
            &["user_task_respond_finish("][..],
        ),
    ] {
        let Some((at, window)) = fn_window(dispatch, head) else {
            push(DISPATCH, 0, missing(head));
            continue;
        };
        for needle in ["ControlHost", "host."] {
            for i in code_lines_with(&window, needle) {
                push(
                    DISPATCH,
                    at + i,
                    format!(
                        "background で走る `{head}` が `{needle}` に触っている（host は UI スレッドのもの。\
                         材料は準備の段が写す = `user_task_origin_facts`。#2086）"
                    ),
                );
            }
        }
        for must in musts {
            if code_lines_with(&window, must).is_empty() {
                push(
                    DISPATCH,
                    at,
                    format!(
                        "background の `{head}` が `{must}` を呼んでいない（読み書きが UI スレッドへ戻ったか、\
                         配送が `sent` のまま固まる = #2086）"
                    ),
                );
            }
        }
    }
    // 返答は番を待ってから積む（待たずに積むと、続けて届いた返答が段の途中で追い越す）
    if let Some((at, window)) = fn_window(dispatch, "fn user_task_store_staged(") {
        let wait = code_lines_with(&window, "turn.wait()");
        let store = code_lines_with(&window, "respond_store_at(");
        match (wait.first(), store.first()) {
            (Some(&w), Some(&s)) if w < s => {}
            (_, Some(&s)) => push(
                DISPATCH,
                at + s,
                "返答を番（`turn.wait()`）を待たずに積んでいる（続けて届いた返答が段の途中で追い越し、\
                 返答の番号の順と master の入力欄へ届く順が入れ替わる = #2086）"
                    .into(),
            ),
            _ => {}
        }
    }
    // 着地（IPC の受け口と GUI のボタンが通る `land_offload`）: 返答は ③ 運ぶだけを UI スレッドで行い、
    // ④ 記録は後処理を済ませた後の background の続き（`Settled`）
    match fn_window(dispatch, "pub fn land_offload(") {
        None => push(DISPATCH, 0, missing("pub fn land_offload(")),
        Some((at, window)) => {
            for must in [
                "UserTaskLanding::Responded",
                "user_task_respond_deliver(",
                "OffloadLanding::Settled(",
            ] {
                if code_lines_with(&window, must).is_empty() {
                    push(
                        DISPATCH,
                        at,
                        format!(
                            "着地（`land_offload`）に `{must}` が無い（返答の ③ 運ぶ → ④ 記録する が分かれず、\
                             記録を UI スレッドで書く = #2086）"
                        ),
                    );
                }
            }
            for needle in [
                "user_task_respond_finish(",
                "user_task_respond_record(",
                "record_delivery_at(",
            ] {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "着地（`land_offload` = UI スレッド）が返答の配送の記録 `{needle}` を書いている \
                             （④ は background の続き = `OffloadLanding::Settled`。#2086）"
                        ),
                    );
                }
            }
        }
    }
    // 同期の腕も同じ段を順に呼ぶ（2 本目の実装を作らない）
    match fn_window(dispatch, "fn dispatch_user_task(") {
        None => push(DISPATCH, 0, missing("fn dispatch_user_task(")),
        Some((at, window)) => {
            for must in [
                "user_task_origin_facts(",
                "user_task_add_store(",
                "user_task_add_finish(",
                "user_task_patch(",
                "user_task_expand_check(",
                "user_task_expand_finish(",
            ] {
                if code_lines_with(&window, must).is_empty() {
                    push(
                        DISPATCH,
                        at,
                        format!(
                            "同期の腕（`dispatch_user_task`）が段 `{must}` を通っていない（background の経路と \
                             2 本の実装に割れる = #2086）"
                        ),
                    );
                }
            }
            for needle in ["add_at(", "profile_governing_project"] {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "同期の腕（`dispatch_user_task`）が段を通らずに `{needle}` を直に呼んでいる \
                             （background の経路と 2 本の実装に割れる = #2086）"
                        ),
                    );
                }
            }
        }
    }
    // tako-app: 着地で host を変えた続き（`Settled`）は、受け口が後処理（master を起こしたペインの
    // PTY の起動）を済ませてから background へ出す
    match fn_window(main, "fn land_offload_on_ui(") {
        None => push(MAIN, 0, missing("fn land_offload_on_ui(")),
        Some((at, window)) => match code_lines_with(&window, "OffloadLanding::Settled(").first() {
            None => push(
                MAIN,
                at,
                "`land_offload_on_ui` が `OffloadLanding::Settled` を受けていない（返答の ④ 記録が出ない = #2086）"
                    .into(),
            ),
            Some(&s) => {
                let end = window
                    .iter()
                    .enumerate()
                    .skip(s + 1)
                    .find(|(_, l)| !is_comment(l) && l.contains("OffloadLanding::"))
                    .map_or(window.len(), |(i, _)| i);
                if code_lines_with(&window[s..end], "after_dispatch(").is_empty() {
                    push(
                        MAIN,
                        at + s,
                        "返答を運んだ着地（`OffloadLanding::Settled`）の後処理（`after_dispatch`）が無い \
                         （master を起こしたペインの PTY が次の IPC まで立たない = #2086）"
                            .into(),
                    );
                }
            }
        },
    }
    // tako-app: AI からの展開で一覧に居ない行の引き直しは background（同期は A/B のゲートの後ろだけ）
    match fn_window(main, "fn set_user_task_expanded(") {
        None => push(MAIN, 0, missing("fn set_user_task_expanded(")),
        Some((at, window)) => {
            let gate = code_lines_with(&window, "issue2086_legacy()");
            for i in code_lines_with(&window, "refresh_user_tasks(") {
                if !gate.iter().any(|g| *g < i && i - *g <= 2) {
                    push(
                        MAIN,
                        at + i,
                        "AI からの展開の引き直し（`refresh_user_tasks`）が A/B のゲート（`issue2086_legacy()`）の \
                         腕の外にある（置き場を UI スレッドで読む = #2086。`list_poll.pending` で background へ）"
                            .into(),
                    );
                }
            }
            if code_lines_with(&window, "list_poll.pending = true").is_empty() {
                push(
                    MAIN,
                    at,
                    "AI からの展開で一覧に居ない行を background で引き直していない（`list_poll.pending` が無い = #2086）"
                        .into(),
                );
            }
        }
    }
    // MCP: 返答の経路を省いたら `mcp` と明示して送る（省いたままだと準備の段が呼び出し元を知らず、
    // MCP の返答が UI スレッドの同期の腕に残る）
    let lines: Vec<&str> = mcp.lines().collect();
    match lines
        .iter()
        .position(|l| !is_comment(l) && l.contains("\"tako_todo\" => Request::UserTask {"))
    {
        None => push(
            MCP_REQUEST,
            0,
            missing("\"tako_todo\" => Request::UserTask {"),
        ),
        Some(i) => {
            let window: Vec<String> = lines[i..(i + 25).min(lines.len())]
                .iter()
                .map(|l| l.to_string())
                .collect();
            if code_lines_with(&window, "via: todo_via(args)?").is_empty() {
                push(
                    MCP_REQUEST,
                    i + 1,
                    "MCP の `tako_todo` が返答の経路を省いたまま送っている（受け口の準備の段は呼び出し元を \
                     知らないので、MCP の返答が UI スレッドの同期の腕に残る = #2086）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 13. #2014: プレビューの変更履歴を開く切替は git を UI スレッドで待たない
fn scan_preview_changelog(dispatch: &str, main: &str, render: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    // (a) `prepare_offload` の腕が開く切替を background のジョブにする（A/B のゲートつき）
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => push(
            DISPATCH,
            0,
            "`prepare_offload` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let head = window
                .iter()
                .position(|l| !is_comment(l) && l.trim() == "Request::PreviewChangelog {");
            let job = code_lines_with(&window, "OffloadJob::PreviewChangelog(");
            match (head, job.first()) {
                (Some(head), Some(&job)) if head < job => {
                    let arm = &window[head..=job];
                    for (needle, what) in [
                        (
                            "enabled: Some(true),",
                            "開く切替（`enabled: Some(true)`）に限っていない",
                        ),
                        ("issue2014_legacy()", "A/B のゲート（`issue2014_legacy()`）が無い"),
                        (
                            "preview_changelog_prepare(",
                            "UI スレッドの写しを `preview_changelog_prepare` で作っていない",
                        ),
                    ] {
                        if code_lines_with(arm, needle).is_empty() {
                            push(
                                DISPATCH,
                                at + head,
                                format!("`prepare_offload` の変更履歴の腕が{what}（#2014）"),
                            );
                        }
                    }
                }
                _ => push(
                    DISPATCH,
                    at,
                    "`prepare_offload` が変更履歴を開く切替を background のジョブ \
                     （`Request::PreviewChangelog { … } … OffloadJob::PreviewChangelog`）にしていない \
                     （`git rev-parse` / `git log --follow` を UI スレッドで待つ = #2014）"
                        .into(),
                ),
            }
        }
    }
    // (b) background の段は上限つきの本体を上限つきで呼ぶ
    match fn_window(dispatch, "pub fn run_staged(self)") {
        None => push(
            DISPATCH,
            0,
            "`run_staged` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let arm = code_lines_with(&window, "OffloadJob::PreviewChangelog(job) =>");
            let bounded = arm.first().is_some_and(|&i| {
                window[i..(i + 4).min(window.len())]
                    .iter()
                    .any(|l| l.contains("job.run(tako_core::git::changelog_timeout())"))
            });
            if !bounded {
                push(
                    DISPATCH,
                    arm.first().map_or(at, |&i| at + i),
                    "`run_staged` が変更履歴のジョブを上限（`changelog_timeout()`）つきで走らせていない \
                     （#2014）"
                        .into(),
                );
            }
        }
    }
    match fn_window(dispatch, "-> PreviewChangelogLanding {") {
        None => push(
            DISPATCH,
            0,
            "`PreviewChangelogJob::run` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "git::file_history(").is_empty() {
                push(
                    DISPATCH,
                    at,
                    "`PreviewChangelogJob::run` が上限つきの `git::file_history` を通っていない（#2014）".into(),
                );
            }
            for needle in ["git::repo_root(", "git::log_file_commits(", ".output()"] {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "`PreviewChangelogJob::run` が上限なしの `{needle}` を呼んでいる \
                             （返らない git を永久に待つ = #2014）"
                        ),
                    );
                }
            }
        }
    }
    // (c) 同期の腕（テスト・直呼び・A/B）は同じ本体を通り、git を直に起こさない
    let lines: Vec<&str> = dispatch.lines().collect();
    let sync = lines.iter().enumerate().position(|(i, l)| {
        l.trim() == "Request::PreviewChangelog {"
            && lines.get(i + 2).is_some_and(|n| n.trim() == "enabled,")
    });
    match sync {
        None => push(
            DISPATCH,
            0,
            "dispatch_inner の変更履歴の同期の腕（`Request::PreviewChangelog { pane, enabled, … }`）が \
             見つからない"
                .into(),
        ),
        Some(start) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, l)| l.starts_with("        Request::"))
                .map(|(i, _)| i)
                .unwrap_or(lines.len());
            let arm: Vec<String> = lines[start..end].iter().map(|l| l.to_string()).collect();
            for (needle, what) in [
                ("preview_changelog_land(", "着地の 1 実装（`preview_changelog_land`）"),
                ("preview_changelog_prepare(", "準備の 1 実装（`preview_changelog_prepare`）"),
                ("issue2014_legacy()", "上限なしを A/B のゲートの腕に限る判定"),
            ] {
                if code_lines_with(&arm, needle).is_empty() {
                    push(
                        DISPATCH,
                        start + 1,
                        format!("変更履歴の同期の腕が{what}を通っていない（本体を 2 本にしない = #2014）"),
                    );
                }
            }
            for (needle, why) in RAW {
                for i in code_lines_with(&arm, needle) {
                    push(
                        DISPATCH,
                        start + 1 + i,
                        format!(
                            "変更履歴の同期の腕が `{needle}` を直に呼んでいる（{why}。本体は \
                             `PreviewChangelogJob::run` の 1 実装 = #2014）"
                        ),
                    );
                }
            }
        }
    }
    // (d) tako-app: host とボタンは git を起こさず、ボタンは background の口を通る
    for (file, src, head, must) in [
        (MAIN, main, "fn set_preview_changelog(", None),
        (
            PREVIEW_RENDER,
            render,
            "pub(crate) fn toggle_preview_changelog(",
            Some("dispatch_off_ui("),
        ),
    ] {
        let Some((at, window)) = fn_window(src, head) else {
            push(file, 0, format!("`{head}` が見つからない（走査が空振り）"));
            continue;
        };
        if let Some(must) = must {
            if code_lines_with(&window, must).is_empty() {
                push(
                    file,
                    at,
                    format!(
                        "`{head}` が `{must}` を通っていない（「履歴」ボタンが UI スレッドで同期に \
                         dispatch する = #2014）"
                    ),
                );
            }
        }
        for (needle, why) in RAW {
            for i in code_lines_with(&window, needle) {
                push(
                    file,
                    at + i,
                    format!(
                        "UI スレッドで走る `{head}` が `{needle}` を直に呼んでいる（{why}。変更履歴の \
                         git は dispatch の background のジョブで取る = #2014）"
                    ),
                );
            }
        }
    }
    out
}

/// 14. #2150: 変更履歴の行の diff の展開は git を UI スレッドで待たない
fn scan_preview_changelog_diff(dispatch: &str, main: &str, render: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    // (a) `prepare_offload` の腕が展開を background のジョブにする（A/B のゲートつき）
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => push(
            DISPATCH,
            0,
            "`prepare_offload` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            // 頭 = `Request::PreviewChangelog {` の直後 3 行に `expand: Some(` がある腕
            let head = window.iter().enumerate().position(|(i, l)| {
                !is_comment(l)
                    && l.trim() == "Request::PreviewChangelog {"
                    && window[i + 1..(i + 4).min(window.len())]
                        .iter()
                        .any(|n| n.contains("expand: Some("))
            });
            let job = head.and_then(|h| {
                code_lines_with(&window[h..], "OffloadJob::PreviewChangelogDiff(")
                    .first()
                    .map(|&j| h + j)
            });
            match (head, job) {
                (Some(head), Some(job)) => {
                    let arm = &window[head..=job];
                    for (needle, what) in [
                        ("issue2150_legacy()", "A/B のゲート（`issue2150_legacy()`）が無い"),
                        (
                            "preview_changelog_diff_prepare(",
                            "UI スレッドの写しを `preview_changelog_diff_prepare` で作っていない",
                        ),
                    ] {
                        if code_lines_with(arm, needle).is_empty() {
                            push(
                                DISPATCH,
                                at + head,
                                format!("`prepare_offload` の行の展開の腕が{what}（#2150）"),
                            );
                        }
                    }
                }
                _ => push(
                    DISPATCH,
                    head.map_or(at, |h| at + h),
                    "`prepare_offload` が変更履歴の行の展開を background のジョブ \
                     （`Request::PreviewChangelog { expand: Some(..), .. } … OffloadJob::PreviewChangelogDiff`）\
                     にしていない（`git diff` を UI スレッドで待つ = #2150）"
                        .into(),
                ),
            }
        }
    }
    // (b) background の段は上限つきの本体を変更履歴と同じ上限で呼ぶ
    match fn_window(dispatch, "pub fn run_staged(self)") {
        None => push(
            DISPATCH,
            0,
            "`run_staged` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let arm = code_lines_with(&window, "OffloadJob::PreviewChangelogDiff(job) =>");
            let bounded = arm.first().is_some_and(|&i| {
                window[i..(i + 4).min(window.len())]
                    .iter()
                    .any(|l| l.contains("job.run(tako_core::git::changelog_timeout())"))
            });
            if !bounded {
                push(
                    DISPATCH,
                    arm.first().map_or(at, |&i| at + i),
                    "`run_staged` が行の展開のジョブを上限（`changelog_timeout()`）つきで走らせていない \
                     （#2150）"
                        .into(),
                );
            }
        }
    }
    match fn_window(dispatch, "-> PreviewChangelogDiffLanding {") {
        None => push(
            DISPATCH,
            0,
            "`PreviewChangelogDiffJob::run` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "git::commit_file_diff(").is_empty() {
                push(
                    DISPATCH,
                    at,
                    "`PreviewChangelogDiffJob::run` が上限つきの `git::commit_file_diff` を通っていない \
                     （#2150）"
                        .into(),
                );
            }
            for (needle, _) in RAW.iter().filter(|(n, _)| *n != "git::commit_file_diff(") {
                for i in code_lines_with(&window, needle) {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "`PreviewChangelogDiffJob::run` が上限つきの本体を通らずに `{needle}` を \
                             呼んでいる（返らない git を永久に待つ = #2150）"
                        ),
                    );
                }
            }
        }
    }
    // (c) 同期の腕（畳む・テスト・直呼び・A/B）は同じ準備・着地を通り、git を直に起こさない
    let lines: Vec<&str> = dispatch.lines().collect();
    let sync = lines.iter().enumerate().position(|(i, l)| {
        l.trim() == "Request::PreviewChangelog {"
            && lines.get(i + 2).is_some_and(|n| n.trim() == "enabled,")
    });
    let block = sync.and_then(|start| {
        lines
            .iter()
            .enumerate()
            .skip(start)
            .take(40)
            .find(|(_, l)| !is_comment(l) && l.trim() == "if let Some(hash) = expand {")
            .map(|(i, _)| i)
    });
    match block {
        None => push(
            DISPATCH,
            0,
            "dispatch_inner の変更履歴の同期の腕の行の展開（`if let Some(hash) = expand {`）が \
             見つからない"
                .into(),
        ),
        Some(start) => {
            let indent = lines[start].len() - lines[start].trim_start().len();
            let close = format!("{}}}", " ".repeat(indent));
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, l)| **l == close)
                .map(|(i, _)| i)
                .unwrap_or(lines.len() - 1);
            let arm: Vec<String> = lines[start..=end].iter().map(|l| l.to_string()).collect();
            for (needle, what) in [
                (
                    "preview_changelog_diff_prepare(",
                    "準備の 1 実装（`preview_changelog_diff_prepare`）",
                ),
                (
                    "preview_changelog_diff_land(",
                    "着地の 1 実装（`preview_changelog_diff_land`）",
                ),
                (
                    "issue2150_legacy()",
                    "上限なしを A/B のゲートの腕に限る判定",
                ),
                ("changelog_timeout()", "変更履歴と同じ上限"),
            ] {
                if code_lines_with(&arm, needle).is_empty() {
                    push(
                        DISPATCH,
                        start + 1,
                        format!(
                            "行の展開の同期の腕が{what}を通っていない（本体を 2 本にしない = #2150）"
                        ),
                    );
                }
            }
            for (needle, why) in RAW {
                for i in code_lines_with(&arm, needle) {
                    push(
                        DISPATCH,
                        start + 1 + i,
                        format!(
                            "行の展開の同期の腕が `{needle}` を直に呼んでいる（{why}。本体は \
                             `PreviewChangelogDiffJob::run` の 1 実装 = #2150）"
                        ),
                    );
                }
            }
        }
    }
    // (d) tako-app: host の 2 口とクリックは git を起こさず、クリックは background の口を通る
    for (file, src, head, must) in [
        (MAIN, main, "fn changelog_diff_target(", None),
        (MAIN, main, "fn set_changelog_diff(", None),
        (
            PREVIEW_RENDER,
            render,
            "pub(crate) fn toggle_changelog_diff(",
            Some("dispatch_off_ui("),
        ),
    ] {
        let Some((at, window)) = fn_window(src, head) else {
            push(file, 0, format!("`{head}` が見つからない（走査が空振り）"));
            continue;
        };
        if let Some(must) = must {
            if code_lines_with(&window, must).is_empty() {
                push(
                    file,
                    at,
                    format!(
                        "`{head}` が `{must}` を通っていない（コミット行のクリックが UI スレッドで \
                         同期に dispatch する = #2150）"
                    ),
                );
            }
        }
        for (needle, why) in RAW {
            for i in code_lines_with(&window, needle) {
                push(
                    file,
                    at + i,
                    format!(
                        "UI スレッドで走る `{head}` が `{needle}` を直に呼んでいる（{why}。行の diff は \
                         dispatch の background のジョブで取る = #2150）"
                    ),
                );
            }
        }
    }
    // (e) tako-app のどこからも行の diff の git を直に呼ばない（クリック以外の経路を足しても逃さない）
    for (file, src) in [(MAIN, main), (PREVIEW_RENDER, render)] {
        let all: Vec<String> = src.lines().map(str::to_string).collect();
        for needle in ["git::diff_file_commit(", "git::commit_file_diff("] {
            for i in code_lines_with(&all, needle) {
                push(
                    file,
                    i + 1,
                    format!(
                        "tako-app が `{needle}` を直に呼んでいる（GPUI のコールバックは UI スレッド。\
                         行の diff は dispatch の background のジョブで取る = #2150）"
                    ),
                );
            }
        }
    }
    out
}

/// 16 節（#2182）: 返答の配送で起票 master が居ないときの起動の組み立て（`master_launch::plan` = CLI の
/// 所在確認のログインシェルと tako の実行ファイルの解決）は background のジョブ
/// （`OffloadJob::UserTaskLaunch` / 右パネルは `background_executor()`）。
///
/// 組み立ての呼び出しそのもの（`plan_master_for_response` → `master_launch::plan`）は 15 節の
/// 呼び出しグラフが辿る。ここが見るのはグラフの読まない形: 受け手の型が字面から決まらない
/// メソッド呼び出し（`job.run()`）を UI スレッドの関数で書く逆戻りと、tako-app（グラフの範囲外）
fn scan_user_task_launch(dispatch: &str, tasks: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push =
        |file: &'static str, line: usize, why: String| out.push(Offender { file, line, why });
    // (a) UI スレッドの関数はジョブを走らせない（`job.run()` = 組み立てを待つ）
    for head in [
        "pub fn land_offload(",
        "pub fn finish_offload(",
        "fn user_task_land(",
        "pub fn user_task_respond_deliver(",
        "pub fn user_task_launch_land(",
        "fn launch_master_for_response(",
    ] {
        match fn_window(dispatch, head) {
            None => push(
                DISPATCH,
                0,
                format!("`{head}` が見つからない（走査が空振り。改名したなら番犬も追うこと）"),
            ),
            Some((at, window)) => {
                for i in code_lines_with(&window, ".run()") {
                    push(
                        DISPATCH,
                        at + i,
                        format!(
                            "UI スレッドで走る `{head}` がジョブを走らせている（`.run()`。返答の配送の \
                             起動の組み立て = ログインシェルの `command -v` を UI スレッドで待つ = #2182）"
                        ),
                    );
                }
            }
        }
    }
    // (b) background の段（`run_staged`）が組み立てのジョブを走らせ、着地を UI スレッドへ戻す
    match fn_window(dispatch, "pub fn run_staged(self)") {
        None => push(
            DISPATCH,
            0,
            "`run_staged` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let arm = code_lines_with(&window, "OffloadJob::UserTaskLaunch(job) =>");
            let land = code_lines_with(&window, "OffloadContinuation::UserTaskLaunched(");
            if arm.is_empty() || land.is_empty() {
                push(
                    DISPATCH,
                    at,
                    "`run_staged` が返答の配送の起動の組み立て（`OffloadJob::UserTaskLaunch`）を \
                     background で走らせて `OffloadContinuation::UserTaskLaunched` で戻していない（#2182）"
                        .into(),
                );
            }
        }
    }
    // (c) IPC の着地は起動が要るとき組み立てのジョブを background へ出す（もう一巡）
    if let Some((at, window)) = fn_window(dispatch, "pub fn land_offload(") {
        if code_lines_with(&window, "OffloadJob::UserTaskLaunch(").is_empty() {
            push(
                DISPATCH,
                at,
                "`land_offload` が返答の配送で起票 master が居ないとき、起動の組み立てを background \
                 のジョブ（`OffloadJob::UserTaskLaunch`）へ出していない（#2182）"
                    .into(),
            );
        }
    }
    // (d) tako-app: 右パネルの「返す」は組み立てを background のブロックの中でだけ走らせる
    match fn_window(tasks, "pub(crate) fn pump_user_task_sends(") {
        None => push(
            TASKS_PANEL,
            0,
            "`pump_user_task_sends` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let runs = code_lines_with(&window, ".run()");
            if runs.is_empty() {
                push(
                    TASKS_PANEL,
                    at,
                    "`pump_user_task_sends` が起票 master の起動の組み立て（`job.run()`）を回していない \
                     （master が居ないときの返答が届かない。#2182）"
                        .into(),
                );
            }
            for i in runs {
                if !window[i].contains(".spawn(async move {") {
                    push(
                        TASKS_PANEL,
                        at + i,
                        "右パネルの「返す」が起票 master の起動の組み立て（`.run()`）を background の \
                         ブロック（`background_executor().spawn(async move { … })`）の外 = UI スレッドで \
                         走らせている（ログインシェルの `command -v` を待つ = #2182）"
                            .into(),
                    );
                }
            }
            if code_lines_with(&window, "user_task_launch_land(").is_empty() {
                push(
                    TASKS_PANEL,
                    at,
                    "`pump_user_task_sends` が組み立てた起動を `user_task_launch_land` で起こしていない \
                     （画面だけ別の起こし方になる。#2182）"
                        .into(),
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 15. #2042: 包んだ口を見逃さない（UI スレッドの入口から呼び出しを辿る）
// ---------------------------------------------------------------------------

/// 呼び出しグラフを組む範囲（dispatch が呼ぶ製品コードの全部）
const GRAPH_ROOTS: &[&str] = &["crates/tako-control/src", "crates/tako-core/src"];

/// グラフの入口に足す UI スレッドの関数（[`UI_PATHS`] の外。右は UI スレッドで走る理由）
const GRAPH_EXTRA_ENTRIES: &[(&str, &str)] = &[
    (
        "prepare_offload",
        "要求を background のジョブにするかの判断そのもの（受け口と `dispatch_off_ui` が UI スレッドで呼ぶ）",
    ),
    (
        "finish_offload",
        "background の答えの着地（`land_offload` の続き = UI スレッド。先取りの続きの腕もここ）",
    ),
];

/// [`UI_PATHS`] のうちグラフの入口にしない関数（製品の既定では UI スレッドで走らない同期の腕。右は理由）
const GRAPH_SYNC_ARMS: &[(&str, &str)] = &[(
    "dispatch_stale_binary_status",
    "`prepare_offload` が `OffloadJob::StaleStatus` で background へ出す。UI スレッドで走るのは \
     `collect_stale_status_ctx`（入口に挙げてある）だけで、同期の腕はテスト・直呼び・A/B \
     （`TAKO_1989_LEGACY=1`）用",
)];

/// 辿らない集約点（右は理由）
const GRAPH_STOPS: &[(&str, &str)] = &[
    (
        "dispatch",
        "要求ごとに UI スレッドか background かが分かれる。`prepare_offload` が background へ出す要求の \
         同期の腕まで UI スレッドの経路と読まないため、UI スレッドで走る経路は入口として挙げる",
    ),
    ("dispatch_inner", "`dispatch` の本体（同上）"),
];

/// 既知の残り: 本物の UI スレッドの子プロセス待ちだがまだ直していない辺（`(呼び手, 行き先, 理由)`）。
/// 直したら外すこと（辺が消えると [`既知の残りがまだ残っている`] が落ちる）。
/// #2042 で見つけた返答の配送の `launch_master_for_response` → `plan` は #2182 で直して外した
/// （16 節と [`返答の配送の起動の組み立てを戻すと名指す`]）
const GRAPH_KNOWN: &[(&str, &str, &str)] = &[];

/// 到達した関数で探す葉（[`RAW`] に足す、子プロセスの口の素の形。右は名指しの理由）
const GRAPH_LEAVES: &[(&str, &str)] = &[
    (
        "process::Command",
        "子プロセスを起こす（`std::process::Command` を直に書いた形）",
    ),
    (".spawn()", "子プロセスを起こす"),
    ("wait_with_output(", "子プロセスを待つ"),
    (
        "output_with_timeout(",
        "子プロセスを上限つきで待つ（`probe::output_with_timeout` / `command_output_with_timeout`）",
    ),
    (
        "find_with_timeout(",
        "ログインシェルの `command -v` を上限つきで待つ",
    ),
];

/// 葉の一覧（`raw` = 直書きの口の一覧 + [`GRAPH_LEAVES`]）
fn graph_leaves<'a>(raw: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    raw.iter().chain(GRAPH_LEAVES).copied().collect()
}

/// グラフの入口の関数名（[`UI_PATHS`] の関数から同期の腕を除き、[`GRAPH_EXTRA_ENTRIES`] を足す）
fn graph_entry_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = UI_PATHS
        .iter()
        .filter_map(|head| {
            // 窓の頭（`fn 名前(` / `pub fn 名前(`）の最後の語が関数名。match の腕の頭は関数ではない
            let mut words = head.split_whitespace().rev();
            let name = words.next()?.strip_suffix('(')?;
            (words.next() == Some("fn")).then_some(name)
        })
        .filter(|name| !GRAPH_SYNC_ARMS.iter().any(|(arm, _)| arm == name))
        .collect();
    names.extend(GRAPH_EXTRA_ENTRIES.iter().map(|(name, _)| *name));
    names
}

/// [`GRAPH_ROOTS`] の `.rs` を全部（テスト領域を潰した本文と、その眺め）。読むのは 1 回だけ
fn graph_sources() -> &'static [(String, call_graph::Source)] {
    static CACHE: OnceLock<Vec<(String, call_graph::Source)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let entries =
                std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        let root = workspace_root();
        let mut paths = Vec::new();
        for dir in GRAPH_ROOTS {
            walk(&root.join(dir), &mut paths);
        }
        paths.sort();
        paths
            .iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(&root)
                    .expect("ワークスペースの中")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = read(&root, &rel);
                let source = call_graph::Source::new(&rel, &text);
                (text, source)
            })
            .collect()
    })
}

/// `overrides`（`(相対パス, テスト領域を潰した本文)` = 注入）で差し替えたグラフ
fn graph_with(overrides: &[(&str, &str)]) -> call_graph::Graph {
    let sources = graph_sources()
        .iter()
        .map(
            |(text, source)| match overrides.iter().find(|(rel, _)| *rel == source.rel) {
                Some((rel, injected)) if injected != text => call_graph::Source::new(rel, injected),
                _ => source.clone(),
            },
        )
        .collect();
    call_graph::Graph::build(sources)
}

/// dispatch.rs の関数 `name`（ちょうど 1 つでなければ理由）
fn one_fn(graph: &call_graph::Graph, name: &str) -> Result<usize, String> {
    match graph.find(DISPATCH, name).as_slice() {
        [f] => Ok(*f),
        [] => Err(format!(
            "`fn {name}` が見つからない（走査が空振り。改名したなら番犬も追うこと）"
        )),
        many => Err(format!(
            "`fn {name}` が {} 個ある（入口が 1 つに決まらない）",
            many.len()
        )),
    }
}

/// 既知の残りの辺（`(呼び手, 行き先)`。辺が見つからないものは入れない）
fn graph_known_edges(graph: &call_graph::Graph) -> Vec<(usize, usize)> {
    GRAPH_KNOWN
        .iter()
        .filter_map(|(caller, callee, _)| {
            let from = one_fn(graph, caller).ok()?;
            let to = graph
                .callees(from)
                .into_iter()
                .find(|&g| graph.fns[g].name == *callee)?;
            Some((from, to))
        })
        .collect()
}

/// 15. UI スレッドの入口から呼び出しを辿って届く関数が子プロセスの口を呼ばない（#2042）。
///
/// 名指しの行は「入口の本体で次の段を呼んだ行」（入口そのものに口があればその行）。
/// 口を包んだ関数を UI スレッドの経路に足した人が、まさに書き足した行になる
fn scan_call_graph(
    graph: &call_graph::Graph,
    leaves: &[(&str, &str)],
    known: &[(usize, usize)],
) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut entries = Vec::new();
    for name in graph_entry_names() {
        match one_fn(graph, name) {
            Ok(f) => entries.push(f),
            Err(why) => out.push(Offender {
                file: DISPATCH,
                line: 0,
                why,
            }),
        }
    }
    let mut stops = Vec::new();
    for (name, _) in GRAPH_STOPS {
        match one_fn(graph, name) {
            Ok(f) => stops.push(f),
            Err(why) => out.push(Offender {
                file: DISPATCH,
                line: 0,
                why: format!("集約点: {why}"),
            }),
        }
    }
    let reach = graph.reach(&entries, &|from, to| {
        stops.contains(&to) || known.contains(&(from, to))
    });
    // 名指しの行ごとに 1 件へまとめる（包んだ口 1 つの先に口のある関数がいくつあっても 1 件）。
    // 値は口のある関数ごとの（道筋の段数, 関数, 届いた口の一覧）
    type Leaf = (usize, usize, Vec<String>);
    let mut by_site: std::collections::BTreeMap<usize, Vec<Leaf>> =
        std::collections::BTreeMap::new();
    let mut reached: Vec<usize> = reach.keys().copied().collect();
    reached.sort();
    for f in reached {
        let leaf_rel = &graph.sources[graph.fns[f].file].rel;
        let mut found: Vec<(usize, String)> = Vec::new();
        for (needle, why) in leaves {
            for line in graph.hits(f, needle) {
                found.push((
                    line,
                    format!(
                        "`{}` の {leaf_rel}:{line} `{needle}`（{why}）",
                        graph.fns[f].name
                    ),
                ));
            }
        }
        found.sort();
        let Some(&(first_line, _)) = found.first() else {
            continue;
        };
        let path = graph.path_to(&reach, f);
        let line = path.get(1).and_then(|(_, at)| *at).unwrap_or(first_line);
        by_site.entry(line).or_default().push((
            path.len(),
            f,
            found.into_iter().map(|(_, what)| what).collect(),
        ));
    }
    for (line, mut leaves_at) in by_site {
        // 道筋の短い順（包んだ口に近い関数の口から先に見せる）
        leaves_at.sort();
        let path = graph.path_to(&reach, leaves_at[0].1);
        let what: Vec<String> = leaves_at.into_iter().flat_map(|(_, _, w)| w).collect();
        let route: String = path
            .windows(2)
            .map(|w| {
                let caller_rel = &graph.sources[graph.fns[w[0].0].file].rel;
                format!(
                    " → `{}`（{caller_rel}:{} で呼ぶ）",
                    graph.fns[w[1].0].name,
                    w[1].1.unwrap_or(0)
                )
            })
            .collect();
        // 口の一覧は先頭の数件だけ（残りは件数。全部は同じ道筋を辿れば見える）
        const SHOWN: usize = 4;
        let rest = what.len().saturating_sub(SHOWN);
        let mut shown = what.into_iter().take(SHOWN).collect::<Vec<_>>().join(" / ");
        if rest > 0 {
            shown.push_str(&format!(" ほか {rest} 件"));
        }
        out.push(Offender {
            file: DISPATCH,
            line,
            why: format!(
                "UI スレッドの入口 `{}`{route} と辿った先が子プロセスを待つ: {shown}。\
                 口を別名の関数で包んでも UI スレッドで待つことは変わらない（#1984 / #2042）。\
                 先取り（`probe_needs` + 読む側の関数の `Probed::Outside` の腕）か background のジョブ \
                 （`prepare_offload`）へ出すこと。製品の既定で通らない A/B の腕なら `if …legacy() {{` の形で書く",
                graph.fns[path[0].0].name,
            ),
        });
    }
    out
}

#[derive(Clone)]
struct Sources {
    dispatch: String,
    main: String,
    sleep_guard: String,
    agent_cli: String,
    right_panel: String,
    tmux_backend: String,
    tasks_panel: String,
    status_bar: String,
    mcp_request: String,
    preview_render: String,
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
            tmux_backend: read(&root, TMUX_BACKEND),
            tasks_panel: read(&root, TASKS_PANEL),
            status_bar: read(&root, STATUS_BAR),
            mcp_request: read(&root, MCP_REQUEST),
            preview_render: read(&root, PREVIEW_RENDER),
        }
    }

    /// 1〜15 の全部
    fn offenders(&self) -> Vec<Offender> {
        let mut v = self.window_offenders();
        let graph = self.graph();
        let known = graph_known_edges(&graph);
        v.extend(scan_call_graph(&graph, &graph_leaves(RAW), &known));
        v
    }

    /// 15 節のグラフ（このファイル群の本文で差し替えたもの = 注入が効く）
    fn graph(&self) -> call_graph::Graph {
        graph_with(&[
            (DISPATCH, &self.dispatch),
            (SLEEP_GUARD, &self.sleep_guard),
            (AGENT_CLI, &self.agent_cli),
            (TMUX_BACKEND, &self.tmux_backend),
            (MCP_REQUEST, &self.mcp_request),
        ])
    }

    /// 1〜14（窓の中の直書きを見る節。#2042 の A/B で「見逃す側」）
    fn window_offenders(&self) -> Vec<Offender> {
        let mut v = scan_ui_paths(&self.dispatch);
        v.extend(scan_readers(&self.dispatch));
        v.extend(scan_prepare(&self.dispatch));
        v.extend(scan_background(&self.dispatch));
        v.extend(scan_main(&self.main));
        v.extend(scan_sleep_guard(&self.sleep_guard));
        v.extend(scan_tmux_open_arm(&self.dispatch));
        v.extend(scan_locate(&self.agent_cli));
        v.extend(scan_gui_1989(&self.main, &self.right_panel));
        v.extend(scan_git_write(&self.dispatch));
        v.extend(scan_startup_cleanup(&self.main));
        v.extend(scan_tmux_backend(&self.tmux_backend));
        v.extend(scan_user_tasks_list(
            &self.tasks_panel,
            &self.dispatch,
            &self.right_panel,
        ));
        v.extend(scan_user_tasks_ipc(
            &self.dispatch,
            &self.tasks_panel,
            &self.status_bar,
            &self.main,
        ));
        v.extend(scan_user_tasks_ipc_rest(
            &self.dispatch,
            &self.main,
            &self.mcp_request,
        ));
        v.extend(scan_preview_changelog(
            &self.dispatch,
            &self.main,
            &self.preview_render,
        ));
        v.extend(scan_preview_changelog_diff(
            &self.dispatch,
            &self.main,
            &self.preview_render,
        ));
        v.extend(scan_user_task_launch(&self.dispatch, &self.tasks_panel));
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
        // #1991
        (
            &s.dispatch,
            "pub fn prepare_offload(",
            "OffloadJob::GitWrite {",
        ),
        (&s.dispatch, "fn run_reply(self)", "git_write_reply("),
        (&s.dispatch, "fn git_write_reply(", "git::push("),
        (
            &s.main,
            "match app.orphan_cleanup_plan(",
            "CleanupScope::Startup",
        ),
        (
            &s.main,
            "fn cleanup_orphan_tmux(",
            "cleanup_orphan_tmux_with(",
        ),
        (&s.tmux_backend, "pub fn session_cwd(", "run_tmux("),
        (&s.tmux_backend, "pub fn set_session_env(", "run_tmux("),
        // #2031
        (
            &s.tasks_panel,
            "pub(crate) fn tick_user_tasks(",
            "spawn_user_tasks_list(",
        ),
        (
            &s.tasks_panel,
            "pub(crate) fn spawn_user_tasks_list(",
            "user_task_list_load(",
        ),
        (
            &s.dispatch,
            "pub fn user_task_list_observe(",
            "observe_user_task_deliveries(",
        ),
        (&s.dispatch, "pub fn user_task_list_load(", "load_from("),
        // #2052
        (
            &s.dispatch,
            "pub fn prepare_offload(",
            "user_task_offload_prepare(",
        ),
        (
            &s.dispatch,
            "fn user_task_offload_prepare(",
            "user_task_flows_observe(",
        ),
        (
            &s.dispatch,
            "pub fn user_task_flows_observe(",
            "prompt_delivery_state(",
        ),
        (
            &s.dispatch,
            "fn user_task_status_finish(",
            "set_user_task_expanded(",
        ),
        (
            &s.dispatch,
            "fn user_task_list_offload(",
            "user_task_list_reply(",
        ),
        (&s.dispatch, "fn user_task_status_store(", "set_status_at("),
        (
            &s.tasks_panel,
            "pub(crate) fn user_task_set_status(",
            "dispatch_off_ui(",
        ),
        (
            &s.status_bar,
            "pub(crate) fn toggle_panel_view(",
            "PanelView::Git",
        ),
        (&s.main, "fn set_panel(", "PanelView::Fleet"),
        (&s.main, "fn after_dispatch(", "spawn_tmux_refresh("),
        // #2086
        (&s.dispatch, "fn user_task_offloads(", "issue2086_legacy()"),
        (
            &s.dispatch,
            "fn user_task_offload_prepare(",
            "RespondTurn::take()",
        ),
        (
            &s.dispatch,
            "fn user_task_origin_facts(",
            "spawn_chain_master_profile(",
        ),
        (&s.dispatch, "fn user_task_store_staged(", "turn.wait()"),
        (&s.dispatch, "fn user_task_add_store(", "add_at("),
        (
            &s.dispatch,
            "fn user_task_origin(",
            "resolve_session_for_pane(",
        ),
        (
            &s.dispatch,
            "pub fn land_offload(",
            "OffloadLanding::Settled(",
        ),
        (
            &s.dispatch,
            "fn dispatch_user_task(",
            "user_task_expand_finish(",
        ),
        (&s.main, "fn land_offload_on_ui(", "after_dispatch("),
        (&s.main, "fn set_user_task_expanded(", "scroll_to_task("),
        // #2014
        (
            &s.dispatch,
            "pub fn prepare_offload(",
            "OffloadJob::PreviewChangelog(",
        ),
        (
            &s.dispatch,
            "-> PreviewChangelogLanding {",
            "git::file_history(",
        ),
        (
            &s.dispatch,
            "fn preview_changelog_land(",
            "set_preview_changelog(",
        ),
        (&s.main, "fn set_preview_changelog(", "from_history("),
        (
            &s.preview_render,
            "pub(crate) fn toggle_preview_changelog(",
            "dispatch_off_ui(",
        ),
        // #2150
        (
            &s.dispatch,
            "pub fn prepare_offload(",
            "OffloadJob::PreviewChangelogDiff(",
        ),
        (
            &s.dispatch,
            "-> PreviewChangelogDiffLanding {",
            "git::commit_file_diff(",
        ),
        (
            &s.dispatch,
            "fn preview_changelog_diff_land(",
            "set_changelog_diff(",
        ),
        (
            &s.dispatch,
            "fn preview_changelog_diff_prepare(",
            "changelog_diff_target(",
        ),
        (&s.main, "fn changelog_diff_target(", "expanded_diff"),
        (&s.main, "fn set_changelog_diff(", "expanded_diff"),
        (
            &s.preview_render,
            "pub(crate) fn toggle_changelog_diff(",
            "dispatch_off_ui(",
        ),
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

    // ①' 呼び出し元の解決の途中で PC リソースを走査する（#1993）
    let d = inject(
        &s.dispatch,
        "        .and_then(|pid| caller_pane_by_pid(host, pid))\n        .and_then(|p| resolve_pane(host.workspace(), Some(p)).ok())",
        "        .and_then(|pid| crate::resources::answer(None).ok().and(caller_pane_by_pid(host, pid)))\n        .and_then(|p| resolve_pane(host.workspace(), Some(p)).ok())",
        "①'",
    );
    expect_named(
        "呼び出し元の解決で PC リソースを走査",
        scan_ui_paths(&d),
        DISPATCH,
        "resources::answer(",
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

    // ⑰ #1991: git の書き込み系を background のジョブにしない（修正前の prepare_offload = 腕が無い）
    let d = inject(
        &s.dispatch,
        "            Some(git_pane_cwd(host, *pane).map(|cwd| OffloadJob::GitWrite {",
        "            Some(git_pane_cwd(host, *pane).map(|cwd| OffloadJob::GitLog {",
        "⑰",
    );
    expect_named(
        "git の書き込み系を同期のまま",
        scan_git_write(&d),
        DISPATCH,
        "OffloadJob::GitWrite",
    );

    // ⑱ #1991: push だけ git の腕から漏れる（その要求は同期の腕 = UI スレッドでネットワークを待つ）
    let d = inject(
        &s.dispatch,
        "        Request::GitCommit { pane, .. }\n        | Request::GitPull { pane }\n        | Request::GitPush { pane }\n",
        "        Request::GitCommit { pane, .. }\n        | Request::GitPull { pane }\n",
        "⑱",
    );
    expect_named(
        "push を git の腕から落とす",
        scan_git_write(&d),
        DISPATCH,
        "Request::GitPush",
    );

    // ⑲ #1991: UI スレッドの cwd の写しでリポジトリまで解決する（`git rev-parse` を UI で待つ）
    let d = inject(
        &s.dispatch,
        "    let (_, target) = resolve_pane(host.workspace(), pane)?;\n    host.session(target)\n        .and_then(|s| s.cwd())",
        "    let (_, target) = resolve_pane(host.workspace(), pane)?;\n    let _ = tako_core::git::repo_root(Path::new(\".\"));\n    host.session(target)\n        .and_then(|s| s.cwd())",
        "⑲",
    );
    expect_named(
        "cwd の写しで git rev-parse",
        scan_ui_paths(&d),
        DISPATCH,
        "fn git_pane_cwd(",
    );

    // ⑳ #1991: 同期の腕に修正前の本体（git を直に起こす 2 本目の実装）を戻す
    let d = inject(
        &s.dispatch,
        "            let cwd = git_pane_cwd(host, pane)?;\n            git_write_reply(&cwd, request)",
        "            let cwd = git_pane_cwd(host, pane)?;\n            let repo = tako_core::git::repo_root(&cwd).unwrap();\n            git_write_reply(&cwd, request)",
        "⑳",
    );
    expect_named(
        "git の同期の腕に本体の 2 本目",
        scan_git_write(&d),
        DISPATCH,
        "git の同期の腕",
    );

    // ㉑ #1991: 起動時の回収の本体を UI スレッドで走らせる（background の腕を外す）
    let m = inject(
        &s.main,
        "                .spawn(async move { log_startup_cleanup(&plan.run()) })",
        "                .spawn(async move { log_startup_cleanup(&skipped_now()) })",
        "㉑",
    );
    expect_named(
        "起動時の回収を UI スレッドで",
        scan_startup_cleanup(&m),
        MAIN,
        "background",
    );
    // ㉑' 同期の run を A/B のゲートより前（= 製品の既定で必ず通る腕）へ置く
    let m = inject(
        &s.main,
        "            Ok(plan) if !tako_core::probe::issue1991_legacy() => cx",
        "            Ok(plan) if plan.min_idle_secs.is_some() => log_startup_cleanup(&plan.run()),\n            Ok(plan) if !tako_core::probe::issue1991_legacy() => cx",
        "㉑'",
    );
    expect_named(
        "起動時の回収の同期の run をゲートの前へ",
        scan_startup_cleanup(&m),
        MAIN,
        "ゲート",
    );

    // ㉒ #1991: 起動時の回収を修正前の形（掃除本体の同期の呼び出し）へ戻す
    let m = inject(
        &s.main,
        "        match app.orphan_cleanup_plan(\n            None,\n            tako_core::tmux_cleanup::CleanupScope::Startup,",
        "        let _ = app.cleanup_orphan_tmux_with(None, tako_core::tmux_cleanup::CleanupScope::Startup, None);\n        match app.orphan_cleanup_plan(\n            None,\n            tako_core::tmux_cleanup::CleanupScope::Startup,",
        "㉒",
    );
    expect_named(
        "起動時に掃除本体を同期で",
        scan_startup_cleanup(&m),
        MAIN,
        "cleanup_orphan_tmux_with",
    );

    // ㉓ #1991: 自動復帰の tmux の口を修正前の上限なしの output() へ戻す
    let t = inject(
        &s.tmux_backend,
        "    let stdout = crate::tmux::run_tmux(\n        Some(socket),\n        &[\n            \"show-environment\",",
        "    let _ = crate::tmux::tmux_command(Some(socket)).output();\n    let stdout = crate::tmux::run_tmux(\n        Some(socket),\n        &[\n            \"show-environment\",",
        "㉓",
    );
    expect_named(
        "自動復帰の session_env を上限なしへ",
        scan_tmux_backend(&t),
        TMUX_BACKEND,
        "pub fn session_env(",
    );

    // ㉔ #2017: self の会話 ID と状態を修正前の UI スレッドの走査へ戻す
    let d = inject(
        &s.dispatch,
        "        _ => self_agent_scan(host, pane_id),",
        "        _ => {\n            let sid = resolve_session_id_for_pane_via_host(host, pane_id);\n            let status = sid\n                .as_deref()\n                .map(|s| orchestrator::query_agent_status(s).status)\n                .unwrap_or_default();\n            (sid, status)\n        }",
        "㉔",
    );
    expect_named(
        "self の走査を UI スレッドへ",
        scan_ui_paths(&d),
        DISPATCH,
        "fn dispatch_orchestrator_self(",
    );

    // ㉕ #2017: self の読み口が先取りを読む前に走査する
    let d = inject(
        &s.dispatch,
        "    let Some(backend) = host.backend_session(pane_id) else {\n        return unknown();",
        "    let _ = resolve_session_id_for_pane_via_host(host, pane_id);\n    let Some(backend) = host.backend_session(pane_id) else {\n        return unknown();",
        "㉕",
    );
    expect_named(
        "self の読み口が Outside の外で走査",
        scan_readers(&d),
        DISPATCH,
        "fn self_agent_scan(",
    );

    // ㉖ #2031: 2 秒ごとの一覧更新を修正前の同期の引き直しへ戻す（UI スレッドで YAML を読む）
    let tp = inject(
        &s.tasks_panel,
        "        self.spawn_user_tasks_list(cx);\n    }\n\n    /// 一覧の要求",
        "        self.refresh_user_tasks();\n    }\n\n    /// 一覧の要求",
        "㉖",
    );
    expect_named(
        "2 秒ごとの一覧更新を同期へ",
        scan_user_tasks_list(&tp, &s.dispatch, &s.right_panel),
        TASKS_PANEL,
        "`refresh_user_tasks(` を UI スレッドで呼んでいる",
    );

    // ㉗ #2031: 置き場の読みを background のブロックの外（UI スレッド）で済ませる
    let tp = inject(
        &s.tasks_panel,
        "        let job = cx.background_executor().spawn(async move {",
        "        let _now = tako_control::dispatch::user_task_list_load(&request, &cache, Default::default());\n        let job = cx.background_executor().spawn(async move {",
        "㉗",
    );
    expect_named(
        "一覧の読みを UI スレッドで",
        scan_user_tasks_list(&tp, &s.dispatch, &s.right_panel),
        TASKS_PANEL,
        "background のブロックの外",
    );

    // ㉘ #2031: A/B のゲートを外し、製品の既定で同期の引き直しをする
    let tp = inject(
        &s.tasks_panel,
        "        if legacy_2031() {\n            self.refresh_user_tasks();",
        "        if true {\n            self.refresh_user_tasks();",
        "㉘",
    );
    expect_named(
        "同期の引き直しをゲートの外へ",
        scan_user_tasks_list(&tp, &s.dispatch, &s.right_panel),
        TASKS_PANEL,
        "ゲート",
    );

    // ㉙ #2031: 右パネルのタブを開いた瞬間の引き直しを修正前の同期へ戻す
    let r = inject(
        &s.right_panel,
        "                                this.spawn_user_tasks_list(cx);",
        "                                this.refresh_user_tasks();",
        "㉙",
    );
    expect_named(
        "タブを開いた瞬間を同期へ",
        scan_user_tasks_list(&s.tasks_panel, &s.dispatch, &r),
        RIGHT_PANEL,
        "this.panel_view = PanelView::Tasks;",
    );

    // ㉚ #2031: UI スレッドの観測で置き場を読む
    let d = inject(
        &s.dispatch,
        "    UserTaskListObserved(observe_user_task_deliveries(host, &cache.pending))",
        "    let _ = crate::user_tasks::load_from(Path::new(\"user-tasks.yaml\"));\n    UserTaskListObserved(observe_user_task_deliveries(host, &cache.pending))",
        "㉚",
    );
    expect_named(
        "観測で置き場を読む",
        scan_user_tasks_list(&s.tasks_panel, &d, &s.right_panel),
        DISPATCH,
        "`user_task_list_observe` が `load_from(`",
    );

    // ㉛ #2031: 版が同じでも読み直す（前回の正本を使い回さない）
    let d = inject(
        &s.dispatch,
        "        Some(store) if cache.stamp == stamp => (store.clone(), false),",
        "        Some(store) if false => (store.clone(), false),",
        "㉛",
    );
    expect_named(
        "版が同じでも読む",
        scan_user_tasks_list(&s.tasks_panel, &d, &s.right_panel),
        DISPATCH,
        "版を前回と比べていない",
    );

    // ㉜ #2031: 版を読んだ後に採る（その間の書き込みを取り逃がす）
    let d = inject(
        &s.dispatch,
        "    // 版は**読む前に**採る（`store_stamp` の注記）\n    let stamp = crate::user_tasks::store_stamp(&path);",
        "    let _first = crate::user_tasks::load_from(&path);\n    let stamp = crate::user_tasks::store_stamp(&path);",
        "㉜",
    );
    expect_named(
        "版を読んだ後に採る",
        scan_user_tasks_list(&s.tasks_panel, &d, &s.right_panel),
        DISPATCH,
        "版（`store_stamp`）より先に読んでいる",
    );

    // ㉝ #2052: `tako todo list` / 完了・却下を修正前の同期の dispatch へ戻す（腕が None を返す）
    let d = inject(
        &s.dispatch,
        "            Some(user_task_offload_prepare(host, request))",
        "            None",
        "㉝",
    );
    expect_named(
        "一覧と完了・却下を同期の dispatch へ",
        scan_user_tasks_ipc(&d, &s.tasks_panel, &s.status_bar, &s.main),
        DISPATCH,
        "`user_task_offload_prepare`）へ渡していない",
    );

    // ㉞ #2052: 完了・却下を #2052 の A/B の軸から外す（門の腕が list しか拾わない）
    let d = inject(
        &s.dispatch,
        "        \"list\" | \"done\" | \"dismiss\" => !tako_core::probe::issue2052_legacy(),",
        "        \"list\" => !tako_core::probe::issue2052_legacy(),",
        "㉞",
    );
    expect_named(
        "完了・却下を #2052 の軸から外す",
        scan_user_tasks_ipc(&d, &s.tasks_panel, &s.status_bar, &s.main),
        DISPATCH,
        "\"done\" を挙げていない",
    );

    // ㉟ #2052: 準備（UI スレッド）で一覧を組む（置き場を UI スレッドで読む）
    let d = inject(
        &s.dispatch,
        "            )?;\n            return Ok(OffloadJob::UserTaskList(",
        "            )?;\n            let _now = user_task_list_reply(&path, &filter, None);\n            return Ok(OffloadJob::UserTaskList(",
        "㉟",
    );
    expect_named(
        "準備で一覧を組む",
        scan_user_tasks_ipc(&d, &s.tasks_panel, &s.status_bar, &s.main),
        DISPATCH,
        "`fn user_task_offload_prepare(` が `user_task_list_reply(`",
    );

    // ㊱ #2052: 送達フローの観測（UI スレッド）で置き場を読む（修正前の決着待ちの宛先を知る形）
    let d = inject(
        &s.dispatch,
        "    UserTaskFlows(\n        host.workspace()",
        "    let _ = crate::user_tasks::load_from(Path::new(\"user-tasks.yaml\"));\n    UserTaskFlows(\n        host.workspace()",
        "㊱",
    );
    expect_named(
        "観測で置き場を読む",
        scan_user_tasks_ipc(&d, &s.tasks_panel, &s.status_bar, &s.main),
        DISPATCH,
        "`pub fn user_task_flows_observe(` が `load_from(`",
    );

    // ㊲ #2052: 書き換えを着地（UI スレッド）へ移す
    let d = inject(
        &s.dispatch,
        "    let UserTaskStatusLanding { id, value } = landing;",
        "    let UserTaskStatusLanding { id, value } = landing;\n    let _ = crate::user_tasks::set_status_at(Path::new(\"user-tasks.yaml\"), &id, Default::default());",
        "㊲",
    );
    expect_named(
        "書き換えを着地へ",
        scan_user_tasks_ipc(&d, &s.tasks_panel, &s.status_bar, &s.main),
        DISPATCH,
        "`fn user_task_status_finish(` が `set_status_at(`",
    );

    // ㊳ #2052: 右パネルの完了・却下のボタンを修正前の同期の dispatch へ戻す
    let tp = inject(
        &s.tasks_panel,
        "        self.user_tasks.status_inflight += 1;\n        self.dispatch_off_ui(",
        "        let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n        self.user_tasks.status_inflight += 1;\n        self.dispatch_off_ui(",
        "㊳",
    );
    expect_named(
        "完了・却下のボタンを同期へ",
        scan_user_tasks_ipc(&s.dispatch, &tp, &s.status_bar, &s.main),
        TASKS_PANEL,
        "`tako_control::dispatch(` を UI スレッドで",
    );

    // ㊴ #2052: 完了・却下の後の引き直しを A/B のゲートの外で同期にする
    let tp = inject(
        &s.tasks_panel,
        "                    if tako_core::probe::issue2052_legacy() {\n                        app.refresh_user_tasks();",
        "                    let _ = 0;\n                    {\n                        app.refresh_user_tasks();",
        "㊴",
    );
    expect_named(
        "完了・却下の後の引き直しを同期へ",
        scan_user_tasks_ipc(&s.dispatch, &tp, &s.status_bar, &s.main),
        TASKS_PANEL,
        "ゲート（`issue2052_legacy()`）の 腕の外",
    );

    // ㊵ #2052: コマンドパレットで tasks を開いても引き直さない（修正前の形）
    let sb = inject(
        &s.status_bar,
        "            if view == PanelView::Tasks && !tako_core::probe::issue2052_legacy() {\n                self.spawn_user_tasks_list(cx);\n            }",
        "",
        "㊵",
    );
    expect_named(
        "パレットで開いても引き直さない",
        scan_user_tasks_ipc(&s.dispatch, &s.tasks_panel, &sb, &s.main),
        STATUS_BAR,
        "`pub(crate) fn toggle_panel_view(` に `spawn_user_tasks_list(` が無い",
    );

    // ㊶ #2052: パレットの引き直しを同期（UI スレッドで置き場を読む）にする
    let sb = inject(
        &s.status_bar,
        "                self.spawn_user_tasks_list(cx);\n            }\n        }\n        cx.notify();",
        "                self.spawn_user_tasks_list(cx);\n                self.refresh_user_tasks();\n            }\n        }\n        cx.notify();",
        "㊶",
    );
    expect_named(
        "パレットの引き直しを同期へ",
        scan_user_tasks_ipc(&s.dispatch, &s.tasks_panel, &sb, &s.main),
        STATUS_BAR,
        "同期の引き直し（`refresh_user_tasks`）を呼んでいる",
    );

    // ㊷ #2052: `tako panel` で tasks を開いても引き直さない
    let m = inject(
        &s.main,
        "        {\n            self.user_tasks.list_poll.pending = true;",
        "        {\n            let _ = ();",
        "㊷",
    );
    expect_named(
        "tako panel で開いても引き直さない",
        scan_user_tasks_ipc(&s.dispatch, &s.tasks_panel, &s.status_bar, &m),
        MAIN,
        "`fn set_panel(` に `list_poll.pending = true` が無い",
    );

    let rest = |d: &str, m: &str, mcp: &str| scan_user_tasks_ipc_rest(d, m, mcp);

    // ㊸ #2086: 起票・詳細・更新・返答・展開を修正前の同期の dispatch へ戻す（門が false を返す）
    let d = inject(
        &s.dispatch,
        "        _ => !tako_core::probe::issue2086_legacy(),",
        "        _ => false,",
        "㊸",
    );
    expect_named(
        "残りの action を同期の dispatch へ",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "ゲートで background へ出していない",
    );

    // ㊹ #2086: 準備が詳細を受けない（門を通った `show` が「不明な action」で断られる）
    let d = inject(
        &s.dispatch,
        "        \"show\" => UserTaskStoreOp::Show(user_task_id(&params)?),\n",
        "",
        "㊹",
    );
    expect_named(
        "準備が詳細を受けない",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "\"show\" を受けていない",
    );

    // ㊺ #2086: 起票元の材料の写し（UI スレッド）で管轄（プロファイルの読み）を解く
    let d = inject(
        &s.dispatch,
        "    UserTaskOriginFacts {\n        caller,",
        "    let _ = crate::orchestrator::profile_governing_project(\"x\");\n    UserTaskOriginFacts {\n        caller,",
        "㊺",
    );
    expect_named(
        "起票元の材料の写しで設定ファイルを読む",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "`fn user_task_origin_facts(` が `profile_governing_project`",
    );

    // ㊻ #2086: 起票の書き込みを着地（UI スレッド）へ移す
    let d = inject(
        &s.dispatch,
        "    let UserTaskAdded { id, title, value } = added;",
        "    let UserTaskAdded { id, title, value } = added;\n    let _ = crate::user_tasks::add_at(Path::new(\"x\"), Default::default());",
        "㊻",
    );
    expect_named(
        "起票の書き込みを着地へ",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "`fn user_task_add_finish(` が `add_at(`",
    );

    // ㊼ #2086: 返答の配送の記録を着地（UI スレッド）で書く（#2182 で ③ は `match` で結末を分ける形）
    let d = inject(
        &s.dispatch,
        "                match user_task_respond_deliver(host, *stored) {",
        "                let _ = user_task_respond_finish(todo!());\n                match user_task_respond_deliver(host, *stored) {",
        "㊼",
    );
    expect_named(
        "返答の記録を着地へ",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "配送の記録 `user_task_respond_finish(` を書いている",
    );

    // ㊽ #2086: 返答を番を待たずに積む（続けて届いた返答が追い越す）
    let d = inject(&s.dispatch, "            turn.wait();\n", "", "㊽");
    expect_named(
        "返答を番を待たずに積む",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "番（`turn.wait()`）を待たずに積んでいる",
    );

    // ㊾ #2086: 同期の腕が起票を段を通らずに直に書く（2 本目の実装）
    let d = inject(
        &s.dispatch,
        "            let added = user_task_add_store(&path, &params, &facts)?;",
        "            let _ = crate::user_tasks::add_at(&path, Default::default());\n            let added = user_task_add_store(&path, &params, &facts)?;",
        "㊾",
    );
    expect_named(
        "同期の腕が起票を直に書く",
        rest(&d, &s.main, &s.mcp_request),
        DISPATCH,
        "段を通らずに `add_at(` を直に呼んでいる",
    );

    // ㊿ #2086: 返答を運んだ着地の後処理（PTY の起動）を外す
    let m = inject(
        &s.main,
        "                let redraw = self.after_dispatch(&mut settled, needs_frame, cx);\n                OffloadLanded::Again(job, settled.err(), redraw)",
        "                let _ = (&mut settled, needs_frame);\n                OffloadLanded::Again(job, settled.err(), Vec::new())",
        "㊿",
    );
    expect_named(
        "返答を運んだ着地の後処理を外す",
        rest(&s.dispatch, &m, &s.mcp_request),
        MAIN,
        "後処理（`after_dispatch`）が無い",
    );

    // (51) #2086: AI からの展開の引き直しを同期（UI スレッドで置き場を読む）へ戻す
    let m = inject(
        &s.main,
        "                    if tako_core::probe::issue2086_legacy() {",
        "                    if true {",
        "(51)",
    );
    expect_named(
        "AI からの展開の引き直しを同期へ",
        rest(&s.dispatch, &m, &s.mcp_request),
        MAIN,
        "ゲート（`issue2086_legacy()`）の 腕の外",
    );

    // (52) #2086: MCP の返答が経路を省いたまま送る（同期の腕に残る）
    let mcp = inject(
        &s.mcp_request,
        "            via: todo_via(args)?,",
        "            via: str_arg(args, \"via\")?,",
        "(52)",
    );
    expect_named(
        "MCP の返答が経路を省いたまま",
        rest(&s.dispatch, &s.main, &mcp),
        MCP_REQUEST,
        "返答の経路を省いたまま送っている",
    );

    // (53)〜(61) #2014: プレビューの変更履歴を開く切替を UI スレッドの git の待ちへ戻す
    let cl = |d: &str, m: &str, r: &str| scan_preview_changelog(d, m, r);
    // (53) 「履歴」ボタンが修正前と同じく UI スレッドでリポジトリを解決する
    let r = inject(
        &s.preview_render,
        "        let request = tako_control::protocol::Request::PreviewChangelog {\n            pane: Some(pane_id.as_u64()),",
        "        let _ = tako_core::git::repo_root(std::path::Path::new(\".\"));\n        let request = tako_control::protocol::Request::PreviewChangelog {\n            pane: Some(pane_id.as_u64()),",
        "(53)",
    );
    expect_named(
        "「履歴」ボタンで git rev-parse",
        cl(&s.dispatch, &s.main, &r),
        PREVIEW_RENDER,
        "toggle_preview_changelog",
    );
    // (54) 「履歴」ボタンが background の口（`dispatch_off_ui`）を通らず同期に dispatch する
    let r = inject(
        &s.preview_render,
        "        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {\n            app.preview_changelog_pending.remove(&pane_id);",
        "        let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n        self.dispatch_later(request, PaneOrigin::User, cx, move |app, result, cx| {\n            app.preview_changelog_pending.remove(&pane_id);",
        "(54)",
    );
    expect_named(
        "「履歴」ボタンを同期の dispatch へ",
        cl(&s.dispatch, &s.main, &r),
        PREVIEW_RENDER,
        "dispatch_off_ui(",
    );
    // (55) host（UI スレッド）が修正前と同じく履歴を自分で取る
    let m = inject(
        &s.main,
        "        match history {\n            None => {\n                self.preview_changelogs.remove(&pane);",
        "        let _ = tako_core::git::log_file_commits(std::path::Path::new(\".\"), \"x\", 50);\n        match history {\n            None => {\n                self.preview_changelogs.remove(&pane);",
        "(55)",
    );
    expect_named(
        "host が git log --follow",
        cl(&s.dispatch, &m, &s.preview_render),
        MAIN,
        "set_preview_changelog",
    );
    // (56) `prepare_offload` が開く切替をジョブにしない（同期の腕 = UI スレッドで git を待つ）
    let d = inject(
        &s.dispatch,
        "                .map(|job| OffloadJob::PreviewChangelog(Box::new(job))),",
        "                .map(|_| OffloadJob::RemoteAgents),",
        "(56)",
    );
    expect_named(
        "開く切替を同期の腕へ",
        cl(&d, &s.main, &s.preview_render),
        DISPATCH,
        "OffloadJob::PreviewChangelog",
    );
    // (57) A/B のゲートが外れる（修正前の形へ戻せない = 前後を同じバイナリで測れない）
    let d = inject(
        &s.dispatch,
        "        } if !tako_core::probe::issue2014_legacy() => Some(\n            preview_changelog_prepare(",
        "        } => Some(\n            preview_changelog_prepare(",
        "(57)",
    );
    expect_named(
        "変更履歴の腕から A/B のゲートを外す",
        cl(&d, &s.main, &s.preview_render),
        DISPATCH,
        "issue2014_legacy()",
    );
    // (58) UI スレッドの準備がリポジトリまで解決する（`git rev-parse` を UI で待つ）
    let d = inject(
        &s.dispatch,
        "    let (_, target) = resolve_pane(host.workspace(), pane)?;\n    let (path, _) = host.preview_state(target).ok_or_else(|| {",
        "    let (_, target) = resolve_pane(host.workspace(), pane)?;\n    let _ = tako_core::git::file_history(Path::new(\".\"), 1, tako_core::git::changelog_timeout());\n    let (path, _) = host.preview_state(target).ok_or_else(|| {",
        "(58)",
    );
    expect_named(
        "準備で git を起こす",
        scan_ui_paths(&d),
        DISPATCH,
        "fn preview_changelog_prepare(",
    );
    // (59) background の本体が上限なしの `repo_root` を通る
    let d = inject(
        &s.dispatch,
        "        let history = tako_core::git::file_history(&self.path, self.max_count, budget);",
        "        let _ = tako_core::git::repo_root(&self.path);\n        let history = tako_core::git::file_history(&self.path, self.max_count, budget);",
        "(59)",
    );
    expect_named(
        "本体を上限なしへ",
        cl(&d, &s.main, &s.preview_render),
        DISPATCH,
        "PreviewChangelogJob::run",
    );
    // (60) background が上限を外して走らせる
    let d = inject(
        &s.dispatch,
        "                    job.run(tako_core::git::changelog_timeout()),",
        "                    job.run(std::time::Duration::MAX),",
        "(60)",
    );
    expect_named(
        "background の上限を外す",
        cl(&d, &s.main, &s.preview_render),
        DISPATCH,
        "changelog_timeout()",
    );
    // (61) 同期の腕が git を直に起こす 2 本目の実装を持つ
    let d = inject(
        &s.dispatch,
        "                    let job = preview_changelog_prepare(host, Some(target.as_u64()), max_count)?;",
        "                    let _ = tako_core::git::log_file_commits(Path::new(\".\"), \"x\", 50);\n                    let job = preview_changelog_prepare(host, Some(target.as_u64()), max_count)?;",
        "(61)",
    );
    expect_named(
        "同期の腕が git を直に",
        cl(&d, &s.main, &s.preview_render),
        DISPATCH,
        "同期の腕",
    );

    // (62)〜(73) #2150: 変更履歴の行の diff の展開を UI スレッドの git の待ちへ戻す
    let cd = |d: &str, m: &str, r: &str| scan_preview_changelog_diff(d, m, r);
    // (62) コミット行のクリックが修正前と同じく UI スレッドで diff を取る
    let r = inject(
        &s.preview_render,
        "            expand: Some(hash.to_string()),\n        };\n        self.dispatch_off_ui(",
        "            expand: Some(hash.to_string()),\n        };\n        let _ = tako_core::git::commit_file_diff(std::path::Path::new(\".\"), hash, \"x\", std::time::Duration::MAX);\n        self.dispatch_off_ui(",
        "(62)",
    );
    expect_named(
        "コミット行のクリックで git diff",
        cd(&s.dispatch, &s.main, &r),
        PREVIEW_RENDER,
        "toggle_changelog_diff",
    );
    // (63) クリックが background の口（`dispatch_off_ui`）を通らず同期に dispatch する
    let r = inject(
        &s.preview_render,
        "        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {\n            app.preview_changelog_diff_pending.remove(&key);",
        "        let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n        self.dispatch_later(request, PaneOrigin::User, cx, move |app, result, cx| {\n            app.preview_changelog_diff_pending.remove(&key);",
        "(63)",
    );
    expect_named(
        "コミット行のクリックを同期の dispatch へ",
        cd(&s.dispatch, &s.main, &r),
        PREVIEW_RENDER,
        "dispatch_off_ui(",
    );
    // (64) host（UI スレッド）が修正前と同じく diff を自分で取る
    let m = inject(
        &s.main,
        "        entry.expanded_diff = hunks;\n        Ok(())",
        "        let _ = tako_core::git::diff_file_commit(std::path::Path::new(\".\"), hash, \"x\");\n        entry.expanded_diff = hunks;\n        Ok(())",
        "(64)",
    );
    expect_named(
        "host が git diff",
        cd(&s.dispatch, &m, &s.preview_render),
        MAIN,
        "set_changelog_diff",
    );
    // (65) host の材料の写しがリポジトリを git で解き直す
    let m = inject(
        &s.main,
        "            expanded: entry.expanded_diff.is_some(),\n            repo_root: data.repo_root.clone(),",
        "            expanded: entry.expanded_diff.is_some(),\n            repo_root: tako_core::git::repo_root(std::path::Path::new(\".\")),",
        "(65)",
    );
    expect_named(
        "host の写しで git rev-parse",
        cd(&s.dispatch, &m, &s.preview_render),
        MAIN,
        "changelog_diff_target",
    );
    // (66) `prepare_offload` が展開をジョブにしない（同期の腕 = UI スレッドで git を待つ）
    let d = inject(
        &s.dispatch,
        "                    Some(Ok(OffloadJob::PreviewChangelogDiff(Box::new(job))))",
        "                    Some(Ok(OffloadJob::RemoteAgents))",
        "(66)",
    );
    expect_named(
        "展開を同期の腕へ",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "OffloadJob::PreviewChangelogDiff",
    );
    // (67) A/B のゲートが外れる（修正前の形へ戻せない = 前後を同じバイナリで測れない）
    let d = inject(
        &s.dispatch,
        "        } if !tako_core::probe::issue2150_legacy() => {\n            match preview_changelog_diff_prepare(",
        "        } => {\n            match preview_changelog_diff_prepare(",
        "(67)",
    );
    expect_named(
        "行の展開の腕から A/B のゲートを外す",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "issue2150_legacy()",
    );
    // (68) UI スレッドの準備が diff まで取る
    let d = inject(
        &s.dispatch,
        "    let row = host\n        .changelog_diff_target(target, requested)",
        "    let _ = tako_core::git::commit_file_diff(Path::new(\".\"), requested, \"x\", tako_core::git::changelog_timeout());\n    let row = host\n        .changelog_diff_target(target, requested)",
        "(68)",
    );
    expect_named(
        "準備で git diff",
        scan_ui_paths(&d),
        DISPATCH,
        "fn preview_changelog_diff_prepare(",
    );
    // (69) UI スレッドの着地が diff を取り直す
    let d = inject(
        &s.dispatch,
        "    let row = host\n        .changelog_diff_target(job.pane, &job.hash)",
        "    let _ = tako_core::git::diff_file_commit(&job.repo, &job.hash, &job.rel);\n    let row = host\n        .changelog_diff_target(job.pane, &job.hash)",
        "(69)",
    );
    expect_named(
        "着地で git diff",
        scan_ui_paths(&d),
        DISPATCH,
        "fn preview_changelog_diff_land(",
    );
    // (70) background の本体が上限なしの口を通る
    let d = inject(
        &s.dispatch,
        "        let hunks = tako_core::git::commit_file_diff(&self.repo, &self.hash, &self.rel, budget);",
        "        let _ = tako_core::git::diff_file_commit(&self.repo, &self.hash, &self.rel);\n        let hunks = tako_core::git::commit_file_diff(&self.repo, &self.hash, &self.rel, budget);",
        "(70)",
    );
    expect_named(
        "本体を上限なしへ",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "PreviewChangelogDiffJob::run",
    );
    // (71) background が上限を外して走らせる
    let d = inject(
        &s.dispatch,
        "OffloadContinuation::PreviewChangelogDiff(Box::new(\n                    job.run(tako_core::git::changelog_timeout()),",
        "OffloadContinuation::PreviewChangelogDiff(Box::new(\n                    job.run(std::time::Duration::MAX),",
        "(71)",
    );
    expect_named(
        "background の上限を外す",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "changelog_timeout()",
    );
    // (72) 同期の腕が git を直に起こす 2 本目の実装を持つ
    let d = inject(
        &s.dispatch,
        "                return match preview_changelog_diff_prepare(host, Some(target.as_u64()), &hash)? {",
        "                let _ = tako_core::git::diff_file_commit(Path::new(\".\"), &hash, \"x\");\n                return match preview_changelog_diff_prepare(host, Some(target.as_u64()), &hash)? {",
        "(72)",
    );
    expect_named(
        "同期の腕が git diff を直に",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "行の展開の同期の腕",
    );
    // (73) 同期の腕が A/B のゲートの外でも上限なしで待つ
    let d = inject(
        &s.dispatch,
        "                        let budget = if tako_core::probe::issue2150_legacy() {\n                            std::time::Duration::MAX\n                        } else {\n                            tako_core::git::changelog_timeout()\n                        };",
        "                        let budget = std::time::Duration::MAX;",
        "(73)",
    );
    expect_named(
        "同期の腕の上限を外す",
        cd(&d, &s.main, &s.preview_render),
        DISPATCH,
        "A/B のゲートの腕に限る",
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

// ---------------------------------------------------------------------------
// 15 節（#2042）の検出力
// ---------------------------------------------------------------------------

/// 関数 `head` の窓の中で `needle` を含むコードの行（1 始まり。ちょうど 1 行であること）
fn line_in(text: &str, head: &str, needle: &str) -> usize {
    let (at, window) = fn_window(text, head).unwrap_or_else(|| panic!("`{head}` の窓"));
    let lines = code_lines_with(&window, needle);
    assert_eq!(
        lines.len(),
        1,
        "`{head}` の中で `{needle}` を含む行がちょうど 1 つ: {lines:?}"
    );
    at + lines[0]
}

/// `caller` が生きた範囲で呼ぶ関数に、`callee_rel` の `callee` が居るか
fn calls_into(graph: &call_graph::Graph, caller: usize, callee_rel: &str, callee: &str) -> bool {
    graph
        .callees(caller)
        .into_iter()
        .any(|g| graph.fns[g].name == callee && graph.sources[graph.fns[g].file].rel == callee_rel)
}

/// 実コードで、各解決の形と辿らない領域が効いている（グラフが空振りしていない）
#[test]
fn 呼び出しグラフが空振りしていない() {
    let graph = graph_with(&[]);
    // 実測（2026-10-16）: 309 ファイル・7,959 関数。下限は桁で置く（増えて落ちることはない）
    assert!(
        graph.sources.len() >= 150 && graph.fns.len() >= 4000,
        "走査範囲が小さすぎる（{} ファイル・{} 関数）。GRAPH_ROOTS の読み先を確かめること",
        graph.sources.len(),
        graph.fns.len()
    );
    // 入口は UI_PATHS の関数の頭（`(` で終わる綴り）を 1 つも落としていない
    assert_eq!(
        graph_entry_names().len(),
        UI_PATHS.iter().filter(|h| h.ends_with('(')).count() - GRAPH_SYNC_ARMS.len()
            + GRAPH_EXTRA_ENTRIES.len(),
        "UI_PATHS の頭から関数名を取り落としている: {:?}",
        graph_entry_names()
    );
    // 入口・集約点・同期の腕はどれも dispatch.rs にちょうど 1 つ
    let names = graph_entry_names()
        .into_iter()
        .chain(GRAPH_STOPS.iter().map(|(n, _)| *n))
        .chain(GRAPH_SYNC_ARMS.iter().map(|(n, _)| *n));
    for name in names {
        if let Err(why) = one_fn(&graph, name) {
            panic!("{why}");
        }
    }
    let f = |name: &str| one_fn(&graph, name).expect("dispatch.rs の関数");

    // 届く範囲が入口だけで閉じていない（実測 793 関数）
    let entries: Vec<usize> = graph_entry_names().into_iter().map(f).collect();
    let stops: Vec<usize> = GRAPH_STOPS.iter().map(|(n, _)| f(n)).collect();
    let reach = graph.reach(&entries, &|_, to| stops.contains(&to));
    assert!(reach.len() >= 300, "届いた関数が {} しかない", reach.len());

    // 各解決の形（実コードの 1 例ずつ）
    let me = f("dispatch_orchestrator_self");
    for (caller, rel, callee, form) in [
        (
            me,
            DISPATCH,
            "resolve_caller_pane",
            "同じファイルの素の関数",
        ),
        (
            me,
            "crates/tako-control/src/claude_mod.rs",
            "lookup_pane",
            "`crate::` のパス",
        ),
        (
            me,
            "crates/tako-core/src/claude_mod.rs",
            "monitor_plan",
            "`tako_core::` のパス（別クレート）",
        ),
        (
            me,
            "crates/tako-control/src/orchestrator/handoff_store.rs",
            "ensure_migrated",
            "`use crate::orchestrator;` で取り込んだモジュール名のパス",
        ),
        (
            f("launch_master_for_response"),
            "crates/tako-control/src/user_tasks.rs",
            "delivery_record",
            "`use` で取り込んだ関数を素の名前で",
        ),
    ] {
        assert!(
            calls_into(&graph, caller, rel, callee),
            "{form}の解決が効いていない: `{}` → {rel} の `{callee}`",
            graph.fns[caller].name
        );
    }
    // 同名の別モジュール（migrations.rs の `ensure_migrated`）へは繋がない
    assert!(
        !calls_into(
            &graph,
            me,
            "crates/tako-control/src/migrations.rs",
            "ensure_migrated"
        ),
        "モジュールのパスを末尾の名前だけで照合している（同名の別関数へ繋がる）"
    );
    // `Type::f(`（`orchestrator::Profile::load`）。#2233 で self は表示用の読み込みへ移ったので、
    // 標本は `Profile::load` を直接呼ぶ採用（adopt）にする
    assert!(
        graph
            .callees(f("dispatch_orchestrator_adopt"))
            .into_iter()
            .any(|g| graph.fns[g].name == "load"
                && graph.fns[g].owner == call_graph::Owner::Type("Profile".into())),
        "`Type::f(` の解決が効いていない（`Profile::load`）"
    );

    // 辿らない領域が実コードで効いている（本文には呼び出しがあるのに、辺は無い）
    let body_has = |rel: &str, name: &str, needle: &str| {
        let g = graph.find(rel, name)[0];
        let def = &graph.fns[g];
        graph.sources[def.file].code[def.body.0..=def.body.1].contains(needle)
    };
    let agents = "crates/tako-control/src/agents.rs";
    for (rel, caller, callee, region) in [
        (
            DISPATCH,
            "self_agent_scan",
            "resolve_session_id_for_pane_via_host",
            "先取りの外側の腕（`Probed::Outside`）",
        ),
        (
            agents,
            "capture_process_table_uncached",
            "ps_table_unbounded_legacy",
            "A/B の旧経路の腕（`if tako_core::probe::issue1979_legacy() {`）",
        ),
    ] {
        assert!(
            body_has(rel, caller, &format!("{callee}(")),
            "前提: `{caller}` の本文に `{callee}(` がある（実コードが変わったなら例を選び直すこと）"
        );
        let g = graph.find(rel, caller)[0];
        assert!(
            !calls_into(&graph, g, rel, callee),
            "{region}を辿っている: `{caller}` → `{callee}`"
        );
    }
    // 集約点は実際に呼ばれている（止めないと全要求の同期の腕へ広がる）
    assert!(
        calls_into(&graph, f("finish_offload"), DISPATCH, "dispatch"),
        "`finish_offload` が `dispatch` を呼んでいない（集約点の宣言が空振り）"
    );

    // 入口が消えた・2 つになったら黙って緑にならない（入口を辿らない = 空振り）
    let s = Sources::load();
    for (label, from, to, why) in [
        (
            "入口の改名",
            "fn collect_stale_status_ctx(",
            "fn collect_stale_status_ctx_renamed(",
            "`fn collect_stale_status_ctx` が見つからない",
        ),
        (
            "入口の重複",
            "fn git_pane_cwd(",
            "fn git_pane_cwd() {}\nfn git_pane_cwd(",
            "`fn git_pane_cwd` が 2 個ある",
        ),
    ] {
        let mut injected = s.clone();
        injected.dispatch = s.dispatch.replacen(from, to, 1);
        assert_ne!(injected.dispatch, s.dispatch, "{label} の置換元がある");
        let graph = injected.graph();
        let named = scan_call_graph(&graph, &graph_leaves(RAW), &graph_known_edges(&graph));
        assert!(
            named.iter().any(|o| o.line == 0 && o.why.contains(why)),
            "{label}を名指さない: {:?}",
            named.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}

/// 合成した小さなクレートで、読み方を 1 形ずつ固定する
#[test]
fn 呼び出しグラフの読み方() {
    let files = [
        (
            "crates/tako-control/src/dispatch.rs",
            r#"
use crate::agents::{list as agent_list, other};
use crate::orchestrator;
use tako_core::probe;

pub fn ui(host: &dyn Host, req: Req) -> u32 {
    same_file();
    agent_list();
    orchestrator::agent_cli::locate();
    crate::agents::by_path();
    agents::tail_match();
    tako_core::probe::cross();
    probe::via_use();
    Snapshot::capture();
    Wrapper::make();
    parse::<u32>(1);
    host.method_on_host();
    host
        .chained();
    let closure = || 1;
    closure();
    let _ = Req::Make(1);
    nested();
    fn nested() {
        nested_target();
    }
    fn unused_nested() {
        std::process::Command::new("x");
    }
    // std::process::Command::new("in comment")
    let s = "std::process::Command::new(";
    let _ = match probed() {
        Probed::Answer(x) => x,
        Probed::Missing => 0,
        Probed::Outside => outside_only(),
    };
    if tako_core::probe::issue1_legacy() {
        legacy_only();
    }
    if !tako_core::probe::issue2_legacy() {
        new_side();
    } else {
        legacy_else_only();
    }
    if tako_core::probe::issue3_legacy() && cond() {
        compound_side();
    }
    std::thread::spawn(move || {
        thread_only();
    });
    match req {
        Req::A if crate::diag::issue4_legacy() => guard_legacy_only(),
        Req::A => guard_new(),
    }
}

pub struct Job;
impl Job {
    pub fn run_ui(&self) {
        self.helper();
        Self::assoc();
    }
    fn helper(&self) {}
    fn assoc() {}
}
impl<T: Clone> Make for Wrapper<T> {
    fn make() -> Self {
        loop {}
    }
}
trait Decl {
    fn decl_only(&self);
}

fn same_file() {}
fn closure() {}
fn chained() {}
fn method_on_host() {}
fn nested_target() {}
fn parse<T>(x: u32) -> T { todo!() }
fn probed() -> Probed { todo!() }
fn outside_only() {}
fn legacy_only() {}
fn new_side() {}
fn legacy_else_only() {}
fn cond() -> bool { true }
fn compound_side() {}
fn thread_only() {}
fn guard_legacy_only() {}
fn guard_new() {}

pub fn ui2(cb: impl Fn(), x: tyname::Type) {
    cb();
    [1].iter().for_each(|step| step());
    [1].iter().for_each(|a, last| last());
    tyname();
}
fn cb() {}
fn step() {}
fn last() {}
fn tyname() {}
"#,
        ),
        (
            "crates/tako-control/src/agents.rs",
            "pub fn list() {}\npub fn other() {}\npub fn by_path() {\n    self::inner();\n}\nfn inner() {}\npub fn tail_match() {}\n",
        ),
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "pub fn parent_helper() {}\n",
        ),
        (
            "crates/tako-control/src/orchestrator/agent_cli.rs",
            "pub fn locate() {\n    super::parent_helper();\n}\n",
        ),
        (
            "crates/tako-core/src/probe.rs",
            "pub fn cross() {}\npub fn via_use() {}\npub struct Snapshot;\nimpl Snapshot {\n    pub fn capture() -> Self {\n        Snapshot\n    }\n}\n",
        ),
        // 同名の関数を別クレートに置く（パスの照合がクレートを見ていること）
        ("crates/tako-core/src/agents.rs", "pub fn by_path() {}\n"),
    ];
    let graph = call_graph::Graph::build(
        files
            .iter()
            .map(|(rel, text)| call_graph::Source::new(rel, text))
            .collect(),
    );
    let reached_names = |rel: &str, entry: &str| -> Vec<(String, String)> {
        let e = graph.find(rel, entry)[0];
        let mut v: Vec<(String, String)> = graph
            .reach(&[e], &|_, _| false)
            .keys()
            .map(|&g| {
                (
                    graph.sources[graph.fns[g].file].rel.clone(),
                    graph.fns[g].name.clone(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let d = "crates/tako-control/src/dispatch.rs";
    let got = reached_names(d, "ui");
    let has = |rel: &str, name: &str| got.iter().any(|(r, n)| r == rel && n == name);
    // 辿る形
    for (rel, name, form) in [
        (d, "same_file", "同じファイルの素の関数"),
        (
            "crates/tako-control/src/agents.rs",
            "list",
            "`use … as` の別名",
        ),
        (
            "crates/tako-control/src/orchestrator/agent_cli.rs",
            "locate",
            "`use` したモジュール名のパス",
        ),
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "parent_helper",
            "`super::`",
        ),
        ("crates/tako-control/src/agents.rs", "by_path", "`crate::`"),
        ("crates/tako-control/src/agents.rs", "inner", "`self::`"),
        (
            "crates/tako-control/src/agents.rs",
            "tail_match",
            "起点の見えないパス（末尾の一致）",
        ),
        ("crates/tako-core/src/probe.rs", "cross", "`tako_core::`"),
        (
            "crates/tako-core/src/probe.rs",
            "via_use",
            "`use` した別クレートのモジュール名",
        ),
        ("crates/tako-core/src/probe.rs", "capture", "`Type::f(`"),
        (d, "make", "`impl<T> Trait for Type<T>` の `Type::f(`"),
        (d, "parse", "ターボフィッシュ"),
        (d, "nested", "入れ子の関数"),
        (d, "nested_target", "入れ子の関数の中の呼び出し"),
        (d, "new_side", "`if !…legacy()` の then 側（新しい経路）"),
        (d, "cond", "組み合わせた条件"),
        (
            d,
            "compound_side",
            "組み合わせた条件の腕（読まない = 辿る）",
        ),
        (d, "guard_new", "ガードの無い腕"),
    ] {
        assert!(
            has(rel, name),
            "{form}を辿っていない: {rel} `{name}`\n{got:?}"
        );
    }
    // 辿らない形
    for (rel, name, form) in [
        (
            "crates/tako-core/src/agents.rs",
            "by_path",
            "`crate::` を別クレートの同名関数へ",
        ),
        (
            d,
            "method_on_host",
            "受け手の型が決まらないメソッド呼び出し",
        ),
        (d, "chained", "行をまたぐメソッドの鎖"),
        (d, "closure", "`let` で束ねたクロージャ"),
        (d, "unused_nested", "呼ばれない入れ子の関数"),
        (d, "outside_only", "先取りの外側の腕"),
        (d, "legacy_only", "`if …legacy() {` の then 側"),
        (
            d,
            "legacy_else_only",
            "`if !…legacy() {…} else {…}` の else 側",
        ),
        (d, "thread_only", "別スレッドへ渡すクロージャ"),
        (d, "guard_legacy_only", "`… if …legacy() =>` の腕"),
        (d, "other", "取り込んだが呼ばない関数"),
    ] {
        assert!(!has(rel, name), "{form}を辿っている: {rel} `{name}`");
    }
    // 引数・クロージャの引数で束ねた名前は同名の関数へ繋がない（型のパス `tyname::Type` は束ねではない）
    let ui2 = reached_names(d, "ui2");
    for name in ["cb", "step", "last"] {
        assert!(
            !ui2.iter().any(|(_, n)| n == name),
            "束ねた `{name}` を同名の関数へ繋いでいる: {ui2:?}"
        );
    }
    assert!(
        ui2.iter().any(|(_, n)| n == "tyname"),
        "型のパス `tyname::Type` を引数の束ねと読んでいる: {ui2:?}"
    );
    // 持ち主: `self.f(` / `Self::f(` は同じ型の関数へ。本体の無い宣言は関数ではない
    let job = reached_names(d, "run_ui");
    for name in ["helper", "assoc"] {
        assert!(
            job.iter().any(|(_, n)| n == name),
            "`self.` / `Self::` の `{name}` を辿っていない: {job:?}"
        );
    }
    assert!(
        graph.find(d, "decl_only").is_empty(),
        "本体の無い宣言を関数にしている"
    );
    // 葉はコメント・文字列・入れ子の関数の中では数えない
    let ui = graph.find(d, "ui")[0];
    assert!(
        graph.hits(ui, "process::Command").is_empty(),
        "コメント・文字列・入れ子の関数の中の口を数えている: {:?}",
        graph.hits(ui, "process::Command")
    );
    let unused = graph.find(d, "unused_nested")[0];
    assert_eq!(
        graph.hits(unused, "process::Command").len(),
        1,
        "入れ子の関数の中の口は、その関数の口として数える"
    );
}

/// 既知の残りは今も残っている（直ったら [`GRAPH_KNOWN`] から外す）。外したら名指しされる = 本物の違反
#[test]
fn 既知の残りがまだ残っている() {
    let graph = graph_with(&[]);
    let known = graph_known_edges(&graph);
    assert_eq!(
        known.len(),
        GRAPH_KNOWN.len(),
        "既知の残りの辺が見つからない（直ったなら GRAPH_KNOWN から外すこと）: {GRAPH_KNOWN:?}"
    );
    for (i, (caller, callee, _)) in GRAPH_KNOWN.iter().enumerate() {
        let rest: Vec<(usize, usize)> = known
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, e)| *e)
            .collect();
        let named = scan_call_graph(&graph, &graph_leaves(RAW), &rest);
        let hit = named.iter().find(|o| {
            o.why.contains(&format!("`{caller}`")) && o.why.contains(&format!("`{callee}`"))
        });
        assert!(
            hit.is_some(),
            "既知の残り `{caller}` → `{callee}` を外しても名指しされない（もう違反ではない = 一覧から外すこと）: {:?}",
            named.iter().map(Offender::report).collect::<Vec<_>>()
        );
        eprintln!("既知の残り（外すと）→ {}", hit.unwrap().report());
    }
}

/// 16 節（#2182）の逆戻り（返答の配送の起動の組み立てを UI スレッドへ戻す注入）を名指す。
/// 組み立ての呼び出しを戻す形は 15 節（呼び出しグラフ）が、グラフの読まない形（UI スレッドの
/// 関数の `job.run()`・tako-app の右パネル）は 16 節の窓が名指す
#[test]
fn 返答の配送の起動の組み立てを戻すと名指す() {
    let s = Sources::load();
    assert!(s.offenders().is_empty(), "前提: 現行は緑");
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let graph_of = |d: &str| {
        let mut injected = s.clone();
        injected.dispatch = d.to_string();
        let graph = injected.graph();
        scan_call_graph(&graph, &graph_leaves(RAW), &graph_known_edges(&graph))
    };

    // (L1) 修正前の形: ③ 運ぶが、起動が要ると組み立てをその場で待って起こす
    let d = inject(
        &s.dispatch,
        "            None => return Routed::NeedsPlan(profile),",
        "            None => launch_master_for_response(host, task, index, &profile, plan_master_for_response(&profile)),",
        "(L1)",
    );
    let named = graph_of(&d);
    let hit = named.iter().find(|o| {
        o.why.contains("`deliver_user_task_response`")
            && o.why.contains("`plan_master_for_response`")
            && o.why.contains("agent_cli::preflight(")
    });
    assert!(
        hit.is_some(),
        "注入「(L1) 組み立てをその場で待つ」を 15 節が名指せない: {:?}",
        named.iter().map(Offender::report).collect::<Vec<_>>()
    );
    eprintln!("注入「(L1)」→ {}", hit.unwrap().report());

    // (L2) A/B のゲートを外す（`TAKO_2182_LEGACY=1` の腕が製品の既定になる）。ゲートの中なら
    // 名指さない（現行が緑 = 旧経路の腕を辿っていない）ことの裏返し
    let d = inject(
        &s.dispatch,
        "    if tako_core::probe::issue2182_legacy() {\n        let plan = plan_master_for_response(&profile);",
        "    {\n        let plan = plan_master_for_response(&profile);",
        "(L2)",
    );
    let named = graph_of(&d);
    let line = line_in(
        &d,
        "pub fn user_task_respond_deliver(",
        "let plan = plan_master_for_response(&profile);",
    );
    let hit = named
        .iter()
        .find(|o| o.line == line && o.why.contains("`plan_master_for_response`"));
    assert!(
        hit.is_some(),
        "注入「(L2) A/B のゲートを外す」を 15 節が {DISPATCH}:{line} で名指せない: {:?}",
        named.iter().map(Offender::report).collect::<Vec<_>>()
    );
    eprintln!("注入「(L2)」→ {}", hit.unwrap().report());

    // (L3) もう一巡を出せない着地がジョブを走らせる（`job.run()` = 受け手の型が字面から決まらない
    // メソッド呼び出し）。15 節は辿らないので見逃し、16 節の窓が名指す
    let d = inject(
        &s.dispatch,
        "UserTaskRespondStep::Launch(job) => job.refused_on_ui(),",
        "UserTaskRespondStep::Launch(job) => user_task_launch_land(host, job.run()).0,",
        "(L3)",
    );
    assert!(
        graph_of(&d).is_empty(),
        "前提: 15 節は `job.run()` を辿らない（16 節の窓が要る理由）"
    );
    let line = line_in(&d, "fn user_task_land(", "job.run()");
    let window = scan_user_task_launch(&d, &s.tasks_panel);
    assert!(
        window
            .iter()
            .any(|o| o.file == DISPATCH && o.line == line && o.why.contains("#2182")),
        "注入「(L3) 着地がジョブを走らせる」を 16 節が {DISPATCH}:{line} で名指せない: {:?}",
        window.iter().map(Offender::report).collect::<Vec<_>>()
    );

    // (L4) 右パネルの「返す」が組み立てを UI スレッドで走らせる（tako-app = グラフの範囲外）
    let t = inject(
        &s.tasks_panel,
        "                    let landing = cx\n                        .background_executor()\n                        .spawn(async move { job.run() })\n                        .await;",
        "                    let landing = job.run();",
        "(L4)",
    );
    let line = line_in(
        &t,
        "pub(crate) fn pump_user_task_sends(",
        "let landing = job.run();",
    );
    let window = scan_user_task_launch(&s.dispatch, &t);
    assert!(
        window
            .iter()
            .any(|o| o.file == TASKS_PANEL && o.line == line && o.why.contains("#2182")),
        "注入「(L4) 右パネルが組み立てを UI スレッドで走らせる」を 16 節が {TASKS_PANEL}:{line} で名指せない: {:?}",
        window.iter().map(Offender::report).collect::<Vec<_>>()
    );

    // (L5) IPC の着地が組み立てのジョブを background へ出さない（もう一巡が消える）
    let d = inject(
        &s.dispatch,
        "OffloadJob::UserTaskLaunch(job.holding(turn))",
        "OffloadJob::UserTaskRespondRecord(job.holding(turn))",
        "(L5)",
    );
    let window = scan_user_task_launch(&d, &s.tasks_panel);
    assert!(
        window.iter().any(|o| o
            .why
            .contains("`land_offload` が返答の配送で起票 master が居ないとき")),
        "注入「(L5) 着地が組み立てを background へ出さない」を 16 節が名指せない: {:?}",
        window.iter().map(Offender::report).collect::<Vec<_>>()
    );
}

/// 包んだ口を通して UI スレッドで子プロセスを待つ注入を、1〜14 は見逃し、15 が file:line で名指しする
#[test]
fn 包んだ口を名指しできる() {
    let s = Sources::load();
    assert!(s.offenders().is_empty(), "前提: 現行は緑");

    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    // A/B: A = 1〜14（窓の中の直書き）は見逃す / B = 15（呼び出しグラフ）が注入した行を名指す
    let a_b = |label: &str, d: &str, head: &str, line_text: &str, why: &str| {
        let mut injected = s.clone();
        injected.dispatch = d.to_string();
        let old = injected.window_offenders();
        assert!(
            old.is_empty(),
            "注入「{label}」を 1〜14 が名指した（包んだ口の注入になっていない）: {:?}",
            old.iter().map(Offender::report).collect::<Vec<_>>()
        );
        let graph = injected.graph();
        let new = scan_call_graph(&graph, &graph_leaves(RAW), &graph_known_edges(&graph));
        let line = line_in(d, head, line_text);
        let hit = new
            .iter()
            .find(|o| o.file == DISPATCH && o.line == line && o.why.contains(why));
        assert!(
            hit.is_some(),
            "注入「{label}」を 15 節が {DISPATCH}:{line} で名指せない: {:?}",
            new.iter().map(Offender::report).collect::<Vec<_>>()
        );
        assert_eq!(
            new.iter().filter(|o| o.line == line).count(),
            1,
            "注入「{label}」の名指しが 1 件にまとまっていない（先の口の数だけ並ぶ）"
        );
        eprintln!("注入「{label}」→ {}", hit.unwrap().report());
    };

    // (G1) 同じファイルの 2 段の包んだ口（名前はどれも RAW に無い）
    let d = inject(
        &s.dispatch,
        "        _ => self_agent_scan(host, pane_id),",
        "        _ => (pane_session_hint(host, pane_id), \"unknown\".to_string()),",
        "(G1)",
    ) + "\nfn pane_session_hint(host: &dyn ControlHost, pane_id: PaneId) -> Option<String> {\n    host.backend_session(pane_id).and_then(|b| backend_owner_pid(&b))\n}\n\nfn backend_owner_pid(backend: &str) -> Option<String> {\n    let out = std::process::Command::new(\"tmux\").arg(backend).output().ok()?;\n    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())\n}\n";
    a_b(
        "同じファイルの 2 段の包んだ口",
        &d,
        "fn dispatch_orchestrator_self(",
        "_ => (pane_session_hint(host, pane_id)",
        "`backend_owner_pid`",
    );

    // (G2) 別モジュールの既存の別名（`agents::list_agents` = `claude agents --json`。RAW は
    // `list_agents_with_panes(` しか持たない）
    let d = inject(
        &s.dispatch,
        "    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "    let _ = crate::agents::list_agents();\n    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "(G2)",
    );
    a_b(
        "別モジュールの既存の別名",
        &d,
        "fn collect_stale_status_ctx(",
        "let _ = crate::agents::list_agents();",
        "`list_agents`",
    );

    // (G3) `use … as` の別名を素の名前で呼ぶ
    let d = inject(
        &s.dispatch,
        "    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "    use crate::agents::list_agents as agents_now;\n    let _ = agents_now();\n    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "(G3)",
    );
    a_b(
        "`use … as` の別名",
        &d,
        "fn collect_stale_status_ctx(",
        "let _ = agents_now();",
        "`list_agents`",
    );

    // (G4) #2017 の穴の再現: #2017 より前の口の一覧（包んだ口の 2 つを持たない）で、
    // 修正前の `orchestrator self`（㉔ と同じ）を 1 節は見逃し、15 節は名指す
    let raw_before_2017: Vec<(&str, &str)> = RAW
        .iter()
        .copied()
        .filter(|(needle, _)| {
            !matches!(
                *needle,
                "resolve_session_id_for_pane_via_host(" | "query_agent_status("
            )
        })
        .collect();
    assert_eq!(
        raw_before_2017.len() + 2,
        RAW.len(),
        "#2017 の 2 つを外した"
    );
    let d = inject(
        &s.dispatch,
        "        _ => self_agent_scan(host, pane_id),",
        "        _ => {\n            let sid = resolve_session_id_for_pane_via_host(host, pane_id);\n            let status = sid\n                .as_deref()\n                .map(|s| orchestrator::query_agent_status(s).status)\n                .unwrap_or_default();\n            (sid, status)\n        }",
        "(G4)",
    );
    let old = scan_ui_paths_with(&d, &raw_before_2017);
    assert!(
        old.is_empty(),
        "前提: #2017 より前の 1 節は修正前の self を見逃す: {:?}",
        old.iter().map(Offender::report).collect::<Vec<_>>()
    );
    let mut injected = s.clone();
    injected.dispatch = d.clone();
    let graph = injected.graph();
    let new = scan_call_graph(
        &graph,
        &graph_leaves(&raw_before_2017),
        &graph_known_edges(&graph),
    );
    for (line_text, why) in [
        (
            "let sid = resolve_session_id_for_pane_via_host(host, pane_id);",
            "`resolve_session_id_for_backend`",
        ),
        (
            ".map(|s| orchestrator::query_agent_status(s).status)",
            "`query_agent_status`",
        ),
    ] {
        let line = line_in(&d, "fn dispatch_orchestrator_self(", line_text);
        let hit = new.iter().find(|o| o.line == line && o.why.contains(why));
        assert!(
            hit.is_some(),
            "#2017 の穴（{line_text}）を 15 節が {DISPATCH}:{line} で名指せない: {:?}",
            new.iter().map(Offender::report).collect::<Vec<_>>()
        );
        eprintln!("注入「#2017 の穴」→ {}", hit.unwrap().report());
    }

    // (G5) 誤検知しない側: 同じ包んだ口でも、先取りの外側の腕・A/B の旧経路の腕の中なら名指さない。
    // 他の条件と組んだ腕は読まない（見逃す側へ倒さない）ので名指す
    let quiet = |label: &str, d: &str| {
        let mut injected = s.clone();
        injected.dispatch = d.to_string();
        let graph = injected.graph();
        let new = scan_call_graph(&graph, &graph_leaves(RAW), &graph_known_edges(&graph));
        assert!(
            new.is_empty(),
            "注入「{label}」は UI スレッドで走らないのに名指した: {:?}",
            new.iter().map(Offender::report).collect::<Vec<_>>()
        );
    };
    quiet(
        "先取りの外側の腕",
        &inject(
            &s.dispatch,
            "        Probed::Outside => {\n            let sid = resolve_session_id_for_pane_via_host(host, pane_id);",
            "        Probed::Outside => {\n            let _ = crate::agents::list_agents();\n            let sid = resolve_session_id_for_pane_via_host(host, pane_id);",
            "(G5a)",
        ),
    );
    quiet(
        "A/B の旧経路の腕",
        &inject(
            &s.dispatch,
            "    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
            "    if tako_core::probe::issue1989_legacy() {\n        let _ = crate::agents::list_agents();\n    }\n    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
            "(G5b)",
        ),
    );
    let d = inject(
        &s.dispatch,
        "    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "    if tako_core::probe::issue1989_legacy() || pane.is_some() {\n        let _ = crate::agents::list_agents();\n    }\n    Ok(StaleStatusCtx {\n        pane: pane_id.as_u64(),",
        "(G5c)",
    );
    a_b(
        "他の条件と組んだ A/B の条件",
        &d,
        "fn collect_stale_status_ctx(",
        "let _ = crate::agents::list_agents();",
        "`list_agents`",
    );
}

//! dispatch — プロトコルリクエストを tako-core ドメイン API へ写す一元ディスパッチャ
//!
//! 設計原則 5「AI フルコントロール」の実装基盤: UI（tako-app）の IPC 受け口と
//! 将来の MCP サーバー（Phase 3）が**同じ dispatch** を呼ぶことで、操作セマンティクスを
//! 一箇所に保つ。各操作は `PaneTree` / `Workspace` の API と 1:1 対応（FR-2.5）。
//!
//! GPUI に依存する処理（セッション起動時のイベント中継、再描画通知）は
//! [`ControlHost`] trait の向こう側（UI 層）に置く。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tako_core::{
    CommandState, Pane, PaneId, PaneNode, PaneOrigin, PaneTreeError, PreviewViewUpdate,
    PreviewZoomCommand, Rect, SpawnCommand, SpawnOptions, SplitAxis, SplitDirection, TabId,
    Workspace,
};

use crate::protocol::{error_code, Direction, FileOpKind, PreviewModeWire, Request};

// ControlHost とサブトレイトは host.rs で定義（Issue #86）
pub use crate::host::{
    ControlHost, PinnedView, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost,
    UiStateHost, WebViewHost, WorkspaceHost,
};

// ControlHost trait の定義は host.rs に移動済み（Issue #86）。
// 旧 trait 定義（74 メソッド）は 8 つのサブトレイトへ分割された。
// dispatch のシグネチャ（&mut dyn ControlHost）は不変

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum DispatchError {
    #[error("ペイン {0} が見つからない")]
    PaneNotFound(u64),
    #[error("タブ {0} が見つからない")]
    TabNotFound(u64),
    #[error("対象ペインが未指定（--pane 指定か TAKO_PANE_ID が必要）")]
    NoTargetPane,
    #[error("ペイン {0} にはターミナルセッションがない")]
    NoSession(u64),
    #[error("無効なパラメータ: {0}")]
    InvalidParams(String),
    #[error("{0}")]
    Operation(String),
}

impl DispatchError {
    /// JSON-RPC エラーコードへの対応付け
    pub fn code(&self) -> i64 {
        match self {
            DispatchError::InvalidParams(_) => error_code::INVALID_PARAMS,
            _ => error_code::OPERATION,
        }
    }
}

/// リクエストを実行し、成功時の `result` 値を返す。
/// `origin` は新規生成ペインの生成主体（Layer 1 CLI なら `Cli`、Phase 3 の MCP なら `Mcp`）
/// dispatch は UI スレッド（GPUI のイベントループ）で実行されるため、ここでの遅延は
/// そのまま UI 全体の固まりになる。処理時間を計測し、しきい値超えを perf.log へ残す
/// （Issue #113: 多ペイン・多 worker 時の無応答の犯人特定。種別名のみ記録し
/// ペイロードは書かない）
pub fn dispatch(
    host: &mut dyn ControlHost,
    request: Request,
    origin: PaneOrigin,
) -> Result<Value, DispatchError> {
    // Issue #168: 計測は diag::perf_span に一元化（32ms 超えを記録 + 2 秒超え継続の
    // ハング級は watchdog が drop を待たず記録。verbose 時はタグ別分布も出る）
    let _span = crate::diag::perf_span(format!("dispatch:{}", request.kind_name()));
    dispatch_inner(host, request, origin)
}

/// UI スレッドを離れて完了できる重い read-only リクエストの分割実行ジョブ（Issue #168 / #115）。
/// `prepare_offload` が UI スレッドで文脈（workspace / ライブ画面）を収集して返し、
/// `run()` は任意のスレッド（GPUI background executor 等）でサブプロセス実行を行う。
/// dispatch と同じ応答形が得られる（操作セマンティクスの一元化は保たれる）
pub enum OffloadJob {
    WorkerStatus {
        ctx: WorkerStatusCtx,
        session_id: Option<String>,
        tmux_session: Option<String>,
    },
    Workers {
        live_panes: Vec<(u64, Option<String>)>,
        include_closed: bool,
        /// 死んだ active エントリの GC を同時に行うか（#658）。セカンダリインスタンスは
        /// プライマリのペインを持たないため false（他人の worker を殺さない）
        sweep: bool,
    },
    GitLog {
        cwd: PathBuf,
        max_count: Option<usize>,
    },
    GitDiff {
        cwd: PathBuf,
        target: Option<String>,
    },
    GitShow {
        cwd: PathBuf,
        hash: String,
        file: Option<String>,
    },
}

/// リクエストが offload 対象なら UI スレッド必須の文脈を収集してジョブ化する。
/// 対象外は None（従来どおり dispatch を同期実行する）。
/// 対象: サブプロセス実行（claude CLI / git / tmux）を伴い、workspace を変更しない
/// リクエストのみ（UI スレッド専有の実測上位。perf.log: OrchestratorWorkerStatus
/// avg 687ms / GitLog 2431ms）
pub fn prepare_offload(
    host: &dyn ControlHost,
    request: &Request,
) -> Option<Result<OffloadJob, DispatchError>> {
    match request {
        Request::OrchestratorWorkerStatus {
            pane_id,
            session_id,
            tmux_session,
            worker,
        } => {
            let q = match resolve_worker_query(
                *pane_id,
                worker.as_deref(),
                session_id.clone(),
                tmux_session.clone(),
            ) {
                Ok(q) => q,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(OffloadJob::WorkerStatus {
                ctx: verify_ctx_pane_identity(
                    collect_worker_status_ctx(host, q.pane_id),
                    q.tmux_session.as_deref(),
                ),
                session_id: q.session_id,
                tmux_session: q.tmux_session,
            }))
        }
        Request::OrchestratorWorkers { all } => Some(Ok(OffloadJob::Workers {
            live_panes: collect_live_panes(host),
            include_closed: all.unwrap_or(false),
            sweep: !host.is_secondary(),
        })),
        Request::GitLog { pane, max_count } => {
            Some(git_pane_cwd(host, *pane).map(|cwd| OffloadJob::GitLog {
                cwd,
                max_count: *max_count,
            }))
        }
        Request::GitDiff { pane, target } => {
            Some(git_pane_cwd(host, *pane).map(|cwd| OffloadJob::GitDiff {
                cwd,
                target: target.clone(),
            }))
        }
        Request::GitShow { pane, hash, file } => {
            Some(git_pane_cwd(host, *pane).map(|cwd| OffloadJob::GitShow {
                cwd,
                hash: hash.clone(),
                file: file.clone(),
            }))
        }
        _ => None,
    }
}

impl OffloadJob {
    /// ジョブ本体（サブプロセス実行）。UI スレッドで呼ばないこと
    pub fn run(self) -> Result<Value, DispatchError> {
        match self {
            OffloadJob::WorkerStatus {
                ctx,
                session_id,
                tmux_session,
            } => finish_worker_status(ctx, session_id.as_deref(), tmux_session.as_deref()),
            OffloadJob::Workers {
                live_panes,
                include_closed,
                sweep,
            } => finish_workers_list(&live_panes, include_closed, sweep),
            OffloadJob::GitLog { cwd, max_count } => run_git_log(&cwd, max_count),
            OffloadJob::GitDiff { cwd, target } => run_git_diff(&cwd, target.as_deref()),
            OffloadJob::GitShow { cwd, hash, file } => run_git_show(&cwd, &hash, file.as_deref()),
        }
    }
}

/// worker_status / report のクエリ正規化結果（#390）
struct WorkerQuery {
    pane_id: u64,
    session_id: Option<String>,
    tmux_session: Option<String>,
}

/// worker_status / report の対象解決（#390）。
/// `worker`（レジストリ ID）指定ならレジストリからペイン・追跡キーを解決し、
/// pane_id 指定でも session_id / tmux_session の欠けをレジストリの active エントリで
/// 補完する（master が pane_id しか知らなくても pane 消失後の追跡が切れない）。
/// レジストリの読み取り失敗は補完スキップ（既存動作を壊さない。フォールバック層）
fn resolve_worker_query(
    pane_id: Option<u64>,
    worker: Option<&str>,
    session_id: Option<String>,
    tmux_session: Option<String>,
) -> Result<WorkerQuery, DispatchError> {
    use crate::orchestrator::registry::WorkerRegistry;
    if let Some(worker_id) = worker {
        let reg = WorkerRegistry::load()
            .map_err(|e| DispatchError::Operation(format!("worker レジストリを読めない: {e}")))?;
        let (_, entry) = reg
            .resolve(worker_id)
            .map_err(DispatchError::InvalidParams)?;
        return Ok(WorkerQuery {
            pane_id: entry.pane,
            session_id: session_id.or_else(|| entry.session_id.clone()),
            tmux_session: tmux_session.or_else(|| entry.tmux_session.clone()),
        });
    }
    let Some(pane_id) = pane_id else {
        return Err(DispatchError::InvalidParams(
            "pane_id または worker を指定してください".into(),
        ));
    };
    // pane_id 指定: 欠けている追跡キーだけをレジストリで補完（明示指定は常に優先）
    if session_id.is_none() || tmux_session.is_none() {
        if let Ok(reg) = WorkerRegistry::load() {
            if let Some((_, entry)) = reg.find_active_by_pane(pane_id) {
                return Ok(WorkerQuery {
                    pane_id,
                    session_id: session_id.or_else(|| entry.session_id.clone()),
                    tmux_session: tmux_session.or_else(|| entry.tmux_session.clone()),
                });
            }
        }
    }
    Ok(WorkerQuery {
        pane_id,
        session_id,
        tmux_session,
    })
}

/// pane ID 再利用の誤マッチ検証（#390）。復元なし再起動では新プロセスが同じ
/// pane 番号を別ペインへ振り直すため、レジストリ由来の pane ID が「現 GUI の
/// 無関係なペイン」を指すことがある。期待する tmux バックエンドセッションと
/// 現ペインの backend が食い違えば別物とみなし、ライブ画面を破棄して
/// tmux セッションフォールバック（正しい worker の実体）だけで判定させる
fn verify_ctx_pane_identity(
    mut ctx: WorkerStatusCtx,
    expected_tmux: Option<&str>,
) -> WorkerStatusCtx {
    if let Some(expect) = expected_tmux {
        if ctx.pane_exists && ctx.backend_session.as_deref() != Some(expect) {
            ctx.pane_exists = false;
            ctx.backend_session = None;
            ctx.live_tail = None;
            ctx.full_screen = None;
            ctx.has_running_children = false;
            // #592: pid も別ペインのものなので捨てる（残すと無関係な worker の
            // claude セッションを「このペインの状態」として解決してしまう）
            ctx.pane_pid = None;
        }
    }
    ctx
}

/// OrchestratorWorkers の UI スレッド必須部分: GUI に現存するペイン ID と
/// backend セッション名の収集（backend は pane ID 再利用の同一性検証に使う。#390）
fn collect_live_panes(host: &dyn ControlHost) -> Vec<(u64, Option<String>)> {
    let mut panes: Vec<(u64, Option<String>)> = Vec::new();
    for tab in host.workspace().tabs() {
        for p in tab.tree().panes() {
            panes.push((p.id().as_u64(), host.backend_session(p.id())));
        }
    }
    for s in host.workspace().shelved_panes() {
        panes.push((s.id().as_u64(), host.backend_session(s.id())));
    }
    panes
}

/// OrchestratorWorkers のサブプロセス実行部分（tmux ls + レジストリ読み）。
/// UI スレッドで呼ばないこと（OffloadJob::run / dispatch 同期経路用）。
///
/// `sweep` = true なら列挙のついでに死んだ active エントリを GC する（#658）。
/// ペインも器も見えない状態が `DEAD_CONFIRM_SECS` 続いたものだけが closed になる
/// （倒すのに 2 回以上の観測が要るので、この 1 回の列挙で生き物を落とすことはない）
fn finish_workers_list(
    live_panes: &[(u64, Option<String>)],
    include_closed: bool,
    sweep: bool,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::registry;
    // 現存するバックエンドセッションを列挙する（器が無い / サーバー未起動は空 = 全て dead 扱い）
    let live_backends: Vec<String> = tako_core::backend::backend()
        .list()
        .into_iter()
        .map(|s| s.session.into_string())
        .collect();
    let reg = if sweep {
        // GC の失敗（ロック競合・書き込み不能）で一覧まで落とさない。読むだけへ縮退する
        registry::sweep_dead(&live_backends, live_panes).or_else(|e| {
            eprintln!("warning: worker レジストリの GC に失敗: {e}");
            registry::WorkerRegistry::load()
        })
    } else {
        registry::WorkerRegistry::load()
    }
    .map_err(|e| DispatchError::Operation(format!("worker レジストリを読めない: {e}")))?;
    Ok(registry::list_payload(
        &reg,
        &live_backends,
        live_panes,
        include_closed,
    ))
}

/// GitLog / GitDiff の UI スレッド必須部分: ペインの cwd 解決（キャッシュ済み値の読み取り）
fn git_pane_cwd(host: &dyn ControlHost, pane: Option<u64>) -> Result<PathBuf, DispatchError> {
    let (_, target) = resolve_pane(host.workspace(), pane)?;
    host.session(target)
        .and_then(|s| s.cwd())
        .map(Path::to_path_buf)
        .ok_or(DispatchError::Operation("cwd が取得できない".into()))
}

/// ペインの cwd から git リポジトリのルートを解決する（#496）
fn git_repo_for_pane(host: &dyn ControlHost, pane: Option<u64>) -> Result<PathBuf, DispatchError> {
    let cwd = git_pane_cwd(host, pane)?;
    tako_core::git::repo_root(&cwd).ok_or_else(|| op_err("git リポジトリが見つかりません"))
}

/// コンフリクト状態の JSON 表現（#496。CLI / MCP / UI で同じ形を使う）
fn conflict_state_json(repo: &Path, state: &tako_core::ConflictState) -> Value {
    json!({
        "repo": repo.display().to_string(),
        "operation": state.operation.as_str(),
        "conflicted": state.is_active(),
        "files": state.files,
        "ours": state.ours,
        "theirs": state.theirs,
        "abort_command": state.operation.abort_args().map(|a| format!("git {} {}", a[0], a[1])),
    })
}

/// GitCheckout（#496）。`confirm` = false のときは破壊的になり得る場合に実行せず提示を返す
fn run_git_checkout(repo: &Path, branch: &str, confirm: bool) -> Result<Value, DispatchError> {
    let preview = tako_core::git::checkout_preview(repo, branch).map_err(op_err)?;
    let preview_json = json!({
        "target": preview.target,
        "current": preview.current,
        "dirty_files": preview.dirty_files,
        "blocking_files": preview.blocking_files,
        "carried_files": preview.carried_files,
        "changed_files": preview.changed_files,
        "creates_local_branch": preview.creates_local_branch,
        "blockers": preview.blockers,
    });
    // blockers は confirm でも越えられない（コンフリクト進行中など、git 自体が拒否する状態）
    if !preview.blockers.is_empty() {
        return Ok(json!({
            "checked_out": false,
            "requires_confirmation": false,
            "blocked": true,
            "preview": preview_json,
        }));
    }
    if !confirm && preview.needs_confirmation() {
        return Ok(json!({
            "checked_out": false,
            "requires_confirmation": true,
            "blocked": false,
            "preview": preview_json,
        }));
    }
    let out = tako_core::git::checkout(repo, branch).map_err(op_err)?;
    Ok(json!({
        "checked_out": true,
        "requires_confirmation": false,
        "blocked": false,
        "branch": tako_core::git::status(repo).branch,
        "preview": preview_json,
        "output": out,
    }))
}

/// GitMerge（#496）。マージは常に事前提示する（`confirm` = true で実行）
fn run_git_merge(
    repo: &Path,
    branch: &str,
    confirm: bool,
    no_ff: bool,
) -> Result<Value, DispatchError> {
    let preview = tako_core::git::merge_preview(repo, branch).map_err(op_err)?;
    let preview_json = json!({
        "target": preview.target,
        "current": preview.current,
        "kind": preview.kind.as_str(),
        "incoming_commits": preview.incoming_commits,
        "changed_files": preview.changed_files,
        "predicted_conflicts": preview.predicted_conflicts,
        "prediction_available": preview.prediction_available,
        "dirty_files": preview.dirty_files,
        "blockers": preview.blockers,
    });
    if !preview.blockers.is_empty() {
        return Ok(json!({
            "merged": false,
            "requires_confirmation": false,
            "blocked": true,
            "preview": preview_json,
        }));
    }
    if !confirm {
        return Ok(json!({
            "merged": false,
            "requires_confirmation": true,
            "blocked": false,
            "preview": preview_json,
        }));
    }
    let outcome = tako_core::git::merge(repo, branch, no_ff).map_err(op_err)?;
    let state = tako_core::git::conflict_state(repo);
    Ok(json!({
        "merged": !outcome.conflicted,
        "requires_confirmation": false,
        "blocked": false,
        "conflicted": outcome.conflicted,
        "conflicts": outcome.conflicts,
        "preview": preview_json,
        "output": outcome.output,
        "state": conflict_state_json(repo, &state),
    }))
}

/// git log + branches + status の取得と応答整形（サブプロセス実行を伴う）
fn run_git_log(cwd: &Path, max_count: Option<usize>) -> Result<Value, DispatchError> {
    let repo = tako_core::git::repo_root(cwd)
        .ok_or(DispatchError::Operation("git リポジトリではない".into()))?;
    let max = max_count.unwrap_or(200);
    let commits = tako_core::git::log_commits(&repo, max);
    let branches = tako_core::git::list_branches(&repo);
    let status = tako_core::git::status(&repo);
    Ok(json!({
        "repo": repo.display().to_string(),
        "branch": status.branch,
        "upstream": status.upstream,
        "commits": commits.iter().map(|c| json!({
            "hash": c.hash,
            "short_hash": c.short_hash,
            "author": c.author,
            "date": c.date_relative,
            "subject": c.subject,
            "refs": c.refs,
            "parents": c.parents,
        })).collect::<Vec<_>>(),
        "branches": branches.iter().map(|b| json!({
            "name": b.name,
            "current": b.is_current,
            "remote": b.is_remote,
            "hash": b.commit_hash,
            "subject": b.subject,
        })).collect::<Vec<_>>(),
        "status": status.entries.iter().map(|e| json!({
            "path": e.path,
            "index": e.index.to_string(),
            "worktree": e.worktree.to_string(),
        })).collect::<Vec<_>>(),
    }))
}

/// git diff の取得と応答整形（サブプロセス実行を伴う）
fn run_git_diff(cwd: &Path, target: Option<&str>) -> Result<Value, DispatchError> {
    let repo = tako_core::git::repo_root(cwd)
        .ok_or(DispatchError::Operation("git リポジトリではない".into()))?;
    let diff_target = match target {
        None | Some("unstaged") => tako_core::git::DiffTarget::Unstaged,
        Some("staged") => tako_core::git::DiffTarget::Staged,
        Some(hash) => tako_core::git::DiffTarget::Commit(hash.to_string()),
    };
    let files = tako_core::git::diff(&repo, &diff_target);
    Ok(json!({
        "repo": repo.display().to_string(),
        "files": files.iter().map(|f| json!({
            "path": f.path,
            "hunks": f.hunks.iter().map(|h| json!({
                "header": h.header,
                "lines": h.lines.iter().map(|l| json!({
                    "kind": match l.kind {
                        tako_core::DiffLineKind::Context => "context",
                        tako_core::DiffLineKind::Add => "add",
                        tako_core::DiffLineKind::Remove => "remove",
                    },
                    "content": l.content,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

fn run_git_show(cwd: &Path, hash: &str, file: Option<&str>) -> Result<Value, DispatchError> {
    let repo = tako_core::git::repo_root(cwd)
        .ok_or(DispatchError::Operation("git リポジトリではない".into()))?;
    let detail = tako_core::git::show_commit(&repo, hash).map_err(DispatchError::Operation)?;
    let mut result = json!({
        "hash": detail.hash,
        "author_name": detail.author_name,
        "author_email": detail.author_email,
        "author_date": detail.author_date,
        "committer_name": detail.committer_name,
        "committer_email": detail.committer_email,
        "committer_date": detail.committer_date,
        "subject": detail.subject,
        "body": detail.body,
        "parents": detail.parents,
        "files": detail.files.iter().map(|f| json!({
            "path": f.path,
            "kind": String::from(match f.kind {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                _ => "modified",
            }),
            "additions": f.additions,
            "deletions": f.deletions,
            "old_path": f.old_path,
        })).collect::<Vec<_>>(),
    });
    if let Some(file_path) = file {
        let hunks = tako_core::git::diff_file_commit(&repo, hash, file_path);
        result["diff"] = json!(hunks
            .iter()
            .map(|h| json!({
                "header": h.header,
                "lines": h.lines.iter().map(|l| json!({
                    "kind": match l.kind {
                        tako_core::DiffLineKind::Context => "context",
                        tako_core::DiffLineKind::Add => "add",
                        tako_core::DiffLineKind::Remove => "remove",
                    },
                    "content": l.content,
                })).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>());
    }
    Ok(result)
}

fn dispatch_inner(
    host: &mut dyn ControlHost,
    request: Request,
    origin: PaneOrigin,
) -> Result<Value, DispatchError> {
    match request {
        Request::Split {
            pane,
            tab,
            direction,
            ratio,
            command,
            cwd,
            focus,
        } => {
            // tab 指定時はそのタブのフォーカス中ペインを基準にする（active tab 非依存）
            let (tab, target) = if let Some(tab_raw) = tab {
                let tab_id = find_tab(host.workspace(), tab_raw)?;
                let focused = host
                    .workspace()
                    .get_tab(tab_id)
                    .expect("find_tab で存在確認済み")
                    .tree()
                    .focused();
                (tab_id, focused)
            } else {
                resolve_pane(host.workspace(), pane)?
            };
            let new_pane = Pane::new(origin);
            let new_id = new_pane.id();
            // 呼び出し元（target）と同じタブに生える（FR-2.1.2）
            tree_mut(host.workspace_mut(), tab)
                .split_with_ratio(
                    target,
                    direction.unwrap_or(Direction::Right).to_core(),
                    ratio.unwrap_or(0.5),
                    new_pane,
                )
                .map_err(op_err)?;
            let options = SpawnOptions {
                command: command.filter(|c| !c.is_empty()).map(|mut c| SpawnCommand {
                    program: c.remove(0),
                    args: c,
                }),
                // cwd 未指定なら分割元ペインの cwd（OSC 7 通知）を継承する。
                // ssh 先などローカルに存在しないパスは無視しホーム既定に任せる
                cwd: cwd.map(Into::into).or_else(|| {
                    host.session(target)
                        .and_then(|s| s.cwd())
                        .filter(|p| p.is_dir())
                        .map(|p| p.to_path_buf())
                }),
                env: Vec::new(),
            };
            host.attach_session(new_id, options);
            // MCP/CLI 経由のデフォルトはフォーカスを移さない（ユーザーの入力を奪わない）
            if !focus.unwrap_or(false) {
                let _ = tree_mut(host.workspace_mut(), tab).focus(target);
            }
            Ok(json!({ "pane": new_id.as_u64() }))
        }

        Request::Close { pane, force } => {
            let (tab, target) = resolve_pane(host.workspace(), pane)?;

            // worker 保護: orchestrator-worker role のペインが busy なら拒否
            let target_pane = host
                .workspace()
                .get_tab(tab)
                .and_then(|t| t.tree().get(target));
            let is_worker = target_pane
                .and_then(|p| p.role())
                .is_some_and(|r| r.starts_with("orchestrator-worker"));
            if !force && is_worker {
                let busy = is_worker_busy(host, target);
                if busy {
                    return Err(DispatchError::Operation(format!(
                        "Worker is still active. Use force: true to close anyway. pane_id={}",
                        target.as_u64()
                    )));
                }
            }
            // Issue #165: worker close 後のリフロー用に spawn 元を close 前に記録する
            let reflow_anchor = if is_worker {
                target_pane.and_then(|p| p.spawned_by())
            } else {
                None
            };

            let closed = tree_mut(host.workspace_mut(), tab).close(target);
            match closed {
                Ok(_) => {
                    // Issue #165: worker が抜けた領域を残りの worker で再配分する
                    // （master・ユーザー由来ペインの矩形は変わらない）
                    if let Some(anchor) = reflow_anchor {
                        let layout = crate::setup::spawn_layout_config();
                        if layout.policy != tako_core::SpawnLayoutPolicy::Legacy {
                            let _ = tree_mut(host.workspace_mut(), tab)
                                .reflow_workers(anchor, layout.algorithm);
                        }
                    }
                }
                Err(PaneTreeError::LastPane) => {
                    // タブ最後の 1 ペイン → タブごと閉じる。最後のタブなら拒否する
                    // （アプリ終了に等しい操作は AI / CLI からは行わせない。UI の cmd+W のみ）
                    host.workspace_mut().close_tab(tab).map_err(op_err)?;
                }
                Err(e) => return Err(op_err(e)),
            }
            host.detach_session(target);
            // #390: worker レジストリの該当エントリを closed へ（worker でなければ no-op。
            // PTY 死亡（Exited）はここを通らないため「pane が消えても worker は生存」の
            // 追跡は維持される）
            if let Err(e) = crate::orchestrator::registry::mark_closed_by_pane(
                target.as_u64(),
                "explicit_close",
            ) {
                eprintln!("warning: worker レジストリの close 記録に失敗: {e}");
            }
            Ok(json!({ "closed": target.as_u64() }))
        }

        Request::Focus { pane, direction } => {
            if let Some(direction) = direction {
                // 方向指定はアクティブタブ内の隣接移動（FR-2.5.5）
                let moved = host
                    .workspace_mut()
                    .active_tab_mut()
                    .tree_mut()
                    .focus_direction(direction.to_core());
                Ok(json!({ "focused": moved.map(|id| id.as_u64()) }))
            } else {
                let (tab, target) = resolve_pane(host.workspace(), pane)?;
                let ws = host.workspace_mut();
                tree_mut(ws, tab).focus(target).map_err(op_err)?;
                // 別タブのペインへのフォーカスはタブ切替も伴う
                ws.activate_tab(tab).map_err(op_err)?;
                Ok(json!({ "focused": target.as_u64() }))
            }
        }

        Request::Resize {
            pane,
            axis,
            delta,
            share,
        } => {
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let tree = tree_mut(host.workspace_mut(), tab);
            let new_share = match (delta, share) {
                (Some(d), None) => tree.resize_by(target, axis.to_core(), d).map_err(op_err)?,
                (None, Some(s)) => tree.set_share(target, axis.to_core(), s).map_err(op_err)?,
                _ => {
                    return Err(DispatchError::InvalidParams(
                        "delta か share のどちらか一方を指定する".into(),
                    ))
                }
            };
            Ok(json!({ "share": new_share }))
        }

        Request::Equalize { pane, tab } => {
            let tab_id = match tab {
                Some(raw) => find_tab(host.workspace(), raw)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            tree_mut(host.workspace_mut(), tab_id).equalize();
            Ok(Value::Null)
        }

        Request::List => Ok(list_json(host)),

        Request::Send {
            pane,
            text,
            newline,
            tmux_session,
            await_prompt,
        } => {
            // await_prompt: claude TUI の起動（❯ 表示）を待ってから送達確認つきで送信する。
            // pane が解決できず tmux_session がある場合はバックグラウンドの tmux 経路で同等を行う
            if await_prompt {
                return match resolve_pane(host.workspace(), pane) {
                    Ok((_, target)) => {
                        host.queue_prompt_flow(target, text.clone());
                        Ok(json!({ "queued": true }))
                    }
                    Err(e) => match tmux_session {
                        Some(ref ts) => {
                            spawn_tmux_delivery(ts.clone(), text.clone(), true);
                            Ok(json!({ "queued": true }))
                        }
                        None => Err(e),
                    },
                };
            }

            // pane ID で解決を試み、失敗時に tmux session フォールバック
            match resolve_pane(host.workspace(), pane) {
                Ok((_, target)) => {
                    let session = host
                        .session(target)
                        .ok_or(DispatchError::NoSession(target.as_u64()))?;
                    // #623: どちらの送達経路へ落ちたかを残す。alt screen なら貼り付け +
                    // 分離 Enter（PromptFlow）、そうでなければ改行を CR へ正規化した
                    // 即時書き込み（= 複数行が「行数ぶんの送信」に化ける）
                    crate::diag::flow_log(&format!(
                        "Send: pane={} alt_screen={} newline={} 行数={} バイト={}",
                        target.as_u64(),
                        session.is_alt_screen(),
                        newline,
                        text.lines().count(),
                        text.len()
                    ));
                    if session.is_alt_screen() {
                        // Enter 単独送信（text が空 / 改行のみ）は送達確認つき Enter フローへ
                        // （Issue #95: 素の CR 1 発は claude TUI に取りこぼされることがあり、
                        // LF は「改行挿入」と解釈され送信にならない）
                        if send_is_enter_only(&text, newline) {
                            host.queue_enter_flow(target);
                            return Ok(json!({ "queued": true }));
                        }
                        // 全画面 TUI（claude 等）への改行つき送信は送達確認フローへ（Issue #32:
                        // 一括書き込みは改行が「送信」と解釈されず入力欄に残留する）
                        if newline {
                            host.queue_send_flow(target, text.clone());
                            return Ok(json!({ "queued": true }));
                        }
                    }
                    // シェルへの送信は従来どおり即時書き込み（挙動・レイテンシ据え置き）。
                    // キーボード入力の意味論で書くため LF は Enter（CR）へ正規化する
                    // （Issue #95: 端末の Enter は CR。LF のままだと claude 等の TUI で
                    // 送信にならない）
                    let normalized = normalize_newlines_for_keys(&text);
                    let payload = if newline {
                        format!("{normalized}\r")
                    } else {
                        normalized
                    };
                    session.write(payload.into_bytes());
                    Ok(Value::Null)
                }
                Err(e) => {
                    if let Some(ref ts) = tmux_session {
                        if newline {
                            // 改行つき送信は送達確認つき配送（対象が claude TUI なら
                            // 貼り付け + 分離 Enter + 検証、シェルなら即時に無害劣化。
                            // text が空 / 改行のみなら Enter 単独送達 = Issue #95）。
                            // 到達手段の有無は先に境界へ問う: 無いまま投げると
                            // バックグラウンドスレッドの中で黙って失敗する（縮退が見えない）
                            if crate::reach::detached_session(ts).is_none() {
                                return Err(DispatchError::Operation(
                                    crate::reach::UnreachableReason::NoDetachedAccess {
                                        session: ts.clone(),
                                        note: crate::reach::no_detached_access_note(),
                                    }
                                    .note(),
                                ));
                            }
                            spawn_tmux_delivery(ts.clone(), text.clone(), false);
                            Ok(json!({ "queued": true }))
                        } else {
                            let (session, access) =
                                crate::reach::detached_session(ts).ok_or_else(|| {
                                    DispatchError::Operation(
                                        crate::reach::UnreachableReason::NoDetachedAccess {
                                            session: ts.clone(),
                                            note: crate::reach::no_detached_access_note(),
                                        }
                                        .note(),
                                    )
                                })?;
                            access
                                .send_text(&session, &normalize_newlines_for_keys(&text))
                                .map_err(|e| DispatchError::Operation(e.to_string()))?;
                            Ok(Value::Null)
                        }
                    } else {
                        Err(e)
                    }
                }
            }
        }

        // #662: 特殊キー送出（PromptFlow を通さない生キー）
        Request::SendKeys {
            pane,
            keys,
            delay_ms,
            tmux_session,
        } => dispatch_send_keys(host, pane, &keys, delay_ms, tmux_session.as_deref()),

        Request::Read {
            pane,
            lines,
            tmux_session,
        } => {
            // pane ID で解決を試み、失敗時に tmux session フォールバック
            let read_result = resolve_pane(host.workspace(), pane)
                .ok()
                .and_then(|(_, target)| {
                    host.session(target).map(|session| {
                        let lines = session.visible_lines();
                        let input = session.analyze_input();
                        (target.as_u64(), lines, input)
                    })
                });

            let (pane_id, mut all, input_status) = match read_result {
                Some(r) => r,
                None => {
                    if let Some(ref ts) = tmux_session {
                        // 読むだけなので**採取の到達手段**を問う（#519）。
                        // 送出を問うと psmux のように「読めるが送れない」器で
                        // 読めるはずの経路まで塞がる
                        let (session, capture) =
                            crate::reach::detached_capture(ts).ok_or_else(|| {
                                DispatchError::Operation(
                                    crate::reach::UnreachableReason::NoDetachedAccess {
                                        session: ts.clone(),
                                        note: crate::reach::no_detached_capture_note(),
                                    }
                                    .note(),
                                )
                            })?;
                        let captured = capture
                            .capture_screen(&session)
                            .map_err(|e| DispatchError::Operation(e.to_string()))?;
                        (pane.unwrap_or(0), captured, None)
                    } else {
                        let (_, target) = resolve_pane(host.workspace(), pane)?;
                        return Err(DispatchError::NoSession(target.as_u64()));
                    }
                }
            };

            while all.last().is_some_and(|l| l.is_empty()) {
                all.pop();
            }
            if let Some(n) = lines {
                if all.len() > n {
                    all.drain(..all.len() - n);
                }
            }
            let input_json = input_status.map(|s| {
                json!({
                    "line": s.line,
                    "text": s.text,
                    "style": match s.style {
                        tako_core::InputStyle::Ghost => "ghost",
                        tako_core::InputStyle::User => "user",
                        tako_core::InputStyle::Mixed => "mixed",
                        tako_core::InputStyle::None => "none",
                    },
                })
            });
            Ok(json!({ "pane": pane_id, "text": all.join("\n"), "input_status": input_json }))
        }

        Request::Scroll { pane, to, delta } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .session(target)
                .ok_or(DispatchError::NoSession(target.as_u64()))?;
            if matches!((to, delta), (Some(_), Some(_)) | (None, None)) {
                return Err(DispatchError::InvalidParams(
                    "to（絶対位置。0 = 最下部）か delta（相対行数）のどちらか一方を指定する".into(),
                ));
            }
            // バックエンドペイン（Phase 5.5）・TmuxOpen ビューペイン（#181）の
            // スクロールバックは tmux 側にあり、表示はホスト UI のローカルミラー
            // （#159。UI のホイール / スクロールバーと同じ層。開発不変条件）。
            // 旧 copy-mode 駆動は廃止した（行単位 + tmux 往復 + キー飲まれの 3 制約のため）
            if host.is_mirror_scroll_pane(target) {
                let (offset, history) = host
                    .backend_scroll_view(target, to.map(|t| t as usize), delta)
                    .ok_or_else(|| {
                        DispatchError::Operation(
                            "このホストはバックエンドペインのスクロール表示に対応していない".into(),
                        )
                    })?;
                return Ok(json!({
                    "pane": target.as_u64(),
                    "offset": offset,
                    "history": history,
                }));
            }
            // 器がスクロールを持つペイン（psmux。#687）。外側 alacritty は器の
            // attach クライアントの alt screen なので履歴が 1 行も積まれず、
            // 下の `scroll_to` / `scroll_display` には動かせる余地が無い
            // （応答だけ offset 0 / history 0 で成功を装っていた）
            if let Some(backend_session) = host.backend_session(target) {
                if let Some(result) =
                    scroll_backend_owned_pane(session, &backend_session, target.as_u64(), to, delta)
                {
                    return Ok(result);
                }
            }
            match (to, delta) {
                (Some(offset), None) => session.scroll_to(offset as usize),
                (None, Some(lines)) => session.scroll_display(lines),
                _ => unreachable!("引数は上で検証済み"),
            }
            Ok(json!({
                "pane": target.as_u64(),
                "offset": session.display_offset(),
                "history": session.history_size(),
            }))
        }

        Request::Title { pane, title, role } => {
            if title.is_none() && role.is_none() {
                return Err(DispatchError::InvalidParams(
                    "title か role の少なくとも一方を指定する".into(),
                ));
            }
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let pane = tree_mut(host.workspace_mut(), tab)
                .get_mut(target)
                .expect("resolve_pane で存在確認済み");
            if let Some(t) = title {
                pane.set_title((!t.is_empty()).then_some(t));
            }
            if let Some(r) = role {
                pane.set_role((!r.is_empty()).then_some(r));
            }
            Ok(Value::Null)
        }

        Request::TmuxList { socket } => {
            // tako ペインとの対応付け: attach クライアントの tty とペインの tty を
            // 突き合わせる（FR-2.13.2。一致しないクライアントは tako 外 = 外部ターミナル由来。
            // tmux バックエンドのペインは tty がバックエンド側ペイン tty に差し替わっており、
            // その中でユーザーが開いたネスト tmux のクライアントもこの突き合わせで対応付く）
            let ws = host.workspace();
            let pane_of_tty: Vec<(String, u64, u64)> = ws
                .tabs()
                .iter()
                .flat_map(|tab| {
                    tab.tree().panes().into_iter().filter_map(|p| {
                        let tty = host.session(p.id())?.tty_name()?;
                        Some((tty.to_string(), p.id().as_u64(), tab.id().as_u64()))
                    })
                })
                .collect();
            // tako 自身のバックエンドセッション（Phase 5.5）の対応表: セッション名 → ペイン
            let backend_of: Vec<(String, u64, u64)> = ws
                .tabs()
                .iter()
                .flat_map(|tab| {
                    tab.tree().panes().into_iter().filter_map(|p| {
                        let name = host.backend_session(p.id())?;
                        Some((name, p.id().as_u64(), tab.id().as_u64()))
                    })
                })
                .collect();
            let session_json = |s: &tako_core::TmuxSession, backend: bool, socket: &Value| {
                let clients: Vec<Value> = s
                    .client_ttys
                    .iter()
                    .map(|tty| {
                        let hit = pane_of_tty.iter().find(|(t, _, _)| t == tty);
                        json!({
                            "tty": tty,
                            // tako のどのペインで表示中か（null = tako 外のターミナル）
                            "pane": hit.map(|(_, pane, _)| *pane),
                            "tab": hit.map(|(_, _, tab)| *tab),
                        })
                    })
                    .collect();
                let owner = backend_of.iter().find(|(name, _, _)| *name == s.name);
                json!({
                    "name": s.name,
                    "created": s.created,
                    "attached": s.attached,
                    // tako のバックエンド永続化セッションか（FR-5。kill すると
                    // 対応ペインの中身が消えるため、UI / AI は区別して扱うこと）
                    "backend": backend,
                    "socket": socket,
                    // backend セッションを保持している tako ペイン（orphan なら null）
                    "backend_pane": owner.map(|(_, pane, _)| *pane),
                    "backend_tab": owner.map(|(_, _, tab)| *tab),
                    "pane_pid": s.pane_pid,
                    "pane_command": s.pane_command,
                    "pane_current_path": s.pane_current_path,
                    "last_activity": s.last_activity,
                    "windows": s.windows.iter().map(|w| json!({
                        "index": w.index,
                        "name": w.name,
                        "active": w.active,
                        "panes": w.panes,
                    })).collect::<Vec<_>>(),
                    "clients": clients,
                })
            };
            let backend_socket = tako_core::tmux_backend::socket_name();
            let explicit_backend = socket.as_deref() == Some(backend_socket.as_str());
            let mut sessions: Vec<Value> = tako_core::tmux::list_sessions(socket.as_deref())
                .iter()
                .map(|s| {
                    session_json(
                        s,
                        explicit_backend,
                        &socket.as_deref().map(Into::into).unwrap_or(Value::Null),
                    )
                })
                .collect();
            // 既定サーバーの一覧には tako バックエンドのセッションも併記する
            // （消し忘れの発見が FR-2.13 の目的。バックエンドの orphan も見えるべき）
            if socket.is_none() {
                sessions.extend(
                    tako_core::tmux::list_sessions(Some(&backend_socket))
                        .iter()
                        .map(|s| session_json(s, true, &backend_socket.clone().into())),
                );
            }
            Ok(json!({ "sessions": sessions }))
        }

        Request::TmuxKill {
            socket,
            session,
            window,
        } => {
            match window {
                Some(index) => tako_core::tmux::kill_window(socket.as_deref(), &session, index),
                None => tako_core::tmux::kill_session(socket.as_deref(), &session),
            }
            .map_err(DispatchError::Operation)?;
            Ok(json!({ "killed": session, "window": window }))
        }

        Request::TmuxResize {
            socket,
            session,
            window,
            cols,
            rows,
            reset,
        } => {
            if reset {
                tako_core::tmux::reset_window_size(socket.as_deref(), &session, window)
                    .map_err(DispatchError::Operation)?;
                return Ok(json!({ "session": session, "window": window, "reset": true }));
            }
            let (Some(cols), Some(rows)) = (cols, rows) else {
                return Err(DispatchError::InvalidParams(
                    "cols と rows の両方を指定するか、reset を使うこと".into(),
                ));
            };
            tako_core::tmux::resize_window(socket.as_deref(), &session, window, cols, rows)
                .map_err(DispatchError::Operation)?;
            Ok(json!({
                "session": session,
                "window": window,
                "cols": cols,
                "rows": rows,
            }))
        }

        Request::TmuxOpen {
            socket,
            session,
            window,
            pane,
            direction,
        } => {
            // 存在しないセッション名は分割前に弾く（D&D 経路では起こらないが、
            // CLI / MCP からのタイポで空ペインだけが生えるのを防ぐ）。
            // has-session（1 コマンド）で確認（旧 list_sessions は 3 コマンドで重かった）
            if !tako_core::tmux::has_session(socket.as_deref(), &session) {
                return Err(DispatchError::Operation(format!(
                    "tmux セッション {session} が見つからない（socket: {}）",
                    socket.as_deref().unwrap_or("既定")
                )));
            }
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let new_pane = Pane::new(origin);
            let new_id = new_pane.id();
            tree_mut(host.workspace_mut(), tab)
                .split_with_ratio(
                    target,
                    direction.unwrap_or(Direction::Right).to_core(),
                    0.5,
                    new_pane,
                )
                .map_err(op_err)?;
            // MCP/CLI 経由ではフォーカスを分割元に維持（ユーザーの入力を奪わない）
            let _ = tree_mut(host.workspace_mut(), tab).focus(target);
            // 元セッションの解決（無限ネスト防止 = 今回の根治）。tmux はグループ名を
            // 「最初に作られた元セッション名」にするため、`tako-view-*` ラッパーや grouped
            // session を開こうとしても group を辿れば必ず元へ戻る。
            // 例: `tako-view-tako-view-master-tako-2-0`（group=master-tako）→ `master-tako`
            let group = tako_core::tmux::session_group(socket.as_deref(), &session);
            let original = group.unwrap_or_else(|| session.clone());
            // tako 自身が作ったラッパーを開き直す場合（バックグラウンドからの復帰・再オープン等）は、
            // **新しいラッパーを作らず元セッションをそのまま直接 attach** する（ユーザー指示）。
            // この経路で開いたペインは元セッションそのものなので close 時に kill しない
            let reopen = session.starts_with("tako-view-");
            // `TMUX=` はネストガードの回避（tako バックエンドペイン内からでも実行可）
            let mut command = vec!["env".to_string(), "TMUX=".to_string(), "tmux".to_string()];
            if let Some(socket) = &socket {
                command.push("-L".into());
                command.push(socket.clone());
            }
            let wrapper = if reopen {
                // 復帰/再オープン: 元セッションを直接 attach（ラッパーを作らない）。
                // window 選択は元セッション全体に効く（独立ラッパーが無いため）
                command.extend([
                    "attach-session".to_string(),
                    "-t".to_string(),
                    format!("={original}"),
                ]);
                if let Some(w) = window {
                    command.extend([
                        ";".to_string(),
                        "select-window".to_string(),
                        "-t".to_string(),
                        format!("{w}"),
                    ]);
                }
                None
            } else {
                // 新規取り込み: grouped session で独立表示する（FR-2.16.10）。
                // `new-session -t <original>` は同じ window 群を共有しつつ表示 window は
                // 独立なので、元クライアント（親）の表示を巻き込まない。ラッパー名はペイン
                // ID で一意化し、同一セッションを複数ペインで開いても衝突しない。元では
                // なくこの **ラッパー** を close 時に kill する（元セッションは無傷）
                let name = format!("tako-view-{original}-{}", new_id.as_u64());
                command.extend([
                    "new-session".to_string(),
                    "-t".to_string(),
                    format!("={original}"),
                    "-s".to_string(),
                    name.clone(),
                ]);
                if let Some(w) = window {
                    // new-session -t では window 指定不可。作成後に select-window を ; で繋ぐ
                    command.extend([
                        ";".to_string(),
                        "select-window".to_string(),
                        "-t".to_string(),
                        format!("{w}"),
                    ]);
                }
                // クライアント切断時の自動破棄（残骸防止の保険。明示 kill が主経路）
                command.extend([
                    ";".to_string(),
                    "set".to_string(),
                    "destroy-unattached".to_string(),
                    "on".to_string(),
                ]);
                Some(name)
            };
            host.track_tmux_view(new_id, original.clone(), wrapper.clone(), socket.clone());
            let mut command = command.into_iter();
            host.attach_session(
                new_id,
                SpawnOptions {
                    command: Some(SpawnCommand {
                        program: command.next().expect("env が先頭にある"),
                        args: command.collect(),
                    }),
                    cwd: None,
                    env: Vec::new(),
                },
            );
            Ok(json!({
                "pane": new_id.as_u64(),
                // 解決後の元セッション名（ラッパー名を渡されても元へ正規化して返す）
                "session": original,
                // 表示用ラッパー名（直接 attach した復帰経路では null）
                "wrapper": wrapper,
                "socket": socket,
            }))
        }

        Request::TmuxSelectWindow { pane, window } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .backend_session(target)
                .ok_or_else(|| DispatchError::Operation(format!(
                    "ペイン {target} にバックエンドセッションがない（tmux 永続化が無効 or 直接 spawn）"
                )))?;
            let socket = tako_core::tmux_backend::socket_name();
            tako_core::tmux::select_window(Some(&socket), &session, window)
                .map_err(DispatchError::Operation)?;
            Ok(json!({
                "pane": target.as_u64(),
                "session": session,
                "window": window,
            }))
        }

        Request::TmuxCleanup { socket } => {
            // socket 省略時は tako バックエンドサーバーを対象にする（取り残しの主因）
            let _ = socket; // 現状は backend socket 固定（host が protected を解決して実行）
            let killed = host.cleanup_orphan_tmux();
            Ok(json!({ "killed": killed }))
        }

        Request::TabRename {
            pane,
            tab,
            title,
            source,
        } => {
            let tab_id = match tab {
                Some(raw) => find_tab(host.workspace(), raw)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            let tab = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .expect("find_tab / resolve_pane で存在確認済み");
            let is_auto = source.as_deref() == Some("auto");
            if title.is_empty() {
                tab.clear_manual_title();
            } else if is_auto {
                tab.set_title_auto(&title);
            } else {
                tab.set_title_manual(title);
            }
            Ok(
                json!({ "tab": tab_id.as_u64(), "title": tab.title(), "source": tab.title_source().as_str() }),
            )
        }

        Request::TabNew { title, focus } => {
            let prev_active = host.workspace().active_tab_id();
            let pane = Pane::new(origin);
            let pane_id = pane.id();
            let explicit = title.is_some();
            let title = title.unwrap_or_else(|| (host.workspace().tabs().len() + 1).to_string());
            let tab_id = host.workspace_mut().create_tab(title, pane);
            if explicit {
                // 明示タイトル付きの作成は手動リネーム扱い（自動リネームに上書きさせない）
                if let Some(tab) = host.workspace_mut().get_tab_mut(tab_id) {
                    let title = tab.title().to_string();
                    tab.set_title_manual(title);
                }
            }
            // CLI/MCP 経由ではアクティブタブを維持（ユーザーの入力を奪わない）
            if !focus.unwrap_or(false) {
                let _ = host.workspace_mut().activate_tab(prev_active);
            }
            host.attach_session(pane_id, SpawnOptions::default());
            Ok(json!({ "tab": tab_id.as_u64(), "pane": pane_id.as_u64() }))
        }

        Request::TabSelect { tab } => {
            let tab_id = find_tab(host.workspace(), tab)?;
            host.workspace_mut().activate_tab(tab_id).map_err(op_err)?;
            Ok(Value::Null)
        }

        Request::WindowList => Ok(windows_json(host.workspace())),

        Request::WindowNew { tab } => match tab {
            // 既存タブを新しいウィンドウへ分離
            Some(t) => {
                let tab_id = find_tab(host.workspace(), t)?;
                let (wid, closed) = host
                    .workspace_mut()
                    .move_tab_to_new_window(tab_id)
                    .map_err(op_err)?;
                host.request_viewport_open(wid);
                Ok(json!({
                    "window": wid.as_u64(),
                    "tab": tab_id.as_u64(),
                    "closed_window": closed.map(|w| w.as_u64()),
                }))
            }
            // 新規タブ 1 つ付きの新しいウィンドウ
            None => {
                let pane = Pane::new(origin);
                let pane_id = pane.id();
                let title = (host.workspace().tabs().len() + 1).to_string();
                let (wid, tab_id) = host.workspace_mut().create_window(title, pane);
                host.attach_session(pane_id, SpawnOptions::default());
                host.request_viewport_open(wid);
                Ok(json!({
                    "window": wid.as_u64(),
                    "tab": tab_id.as_u64(),
                    "pane": pane_id.as_u64(),
                }))
            }
        },

        Request::WindowClose { window } => {
            let wid = find_window(host.workspace(), window)?;
            let moved = host.workspace_mut().close_window(wid).map_err(op_err)?;
            // GPUI ウィンドウの実 close は UI 層の同期（sync_viewports）が拾う
            Ok(json!({
                "window": wid.as_u64(),
                "moved_tabs": moved.iter().map(|t| t.as_u64()).collect::<Vec<_>>(),
            }))
        }

        Request::WindowMoveTab { tab, window } => {
            let tab_id = find_tab(host.workspace(), tab)?;
            let wid = find_window(host.workspace(), window)?;
            let closed = host
                .workspace_mut()
                .move_tab_to_window(tab_id, wid)
                .map_err(op_err)?;
            Ok(json!({
                "tab": tab_id.as_u64(),
                "window": wid.as_u64(),
                "closed_window": closed.map(|w| w.as_u64()),
            }))
        }

        Request::WindowFocus { window } => {
            let wid = find_window(host.workspace(), window)?;
            host.workspace_mut().activate_window(wid).map_err(op_err)?;
            Ok(Value::Null)
        }

        Request::WindowMinimize { window } => {
            window_state_op(host, window, crate::protocol::WindowStateOp::Minimize)
        }
        Request::WindowMaximize { window } => {
            window_state_op(host, window, crate::protocol::WindowStateOp::Maximize)
        }
        Request::WindowRestore { window } => {
            window_state_op(host, window, crate::protocol::WindowStateOp::Restore)
        }

        Request::MenuList => Ok(menu_bar_json(&host.menu_bar_snapshot())),

        Request::MenuOpen { menu } => {
            let snapshot = host.menu_bar_snapshot();
            require_in_window_menu(&snapshot)?;
            let index = resolve_menu_index(&snapshot, &menu)?;
            let name = snapshot.menus[index].name.clone();
            host.request_menu_op(crate::protocol::MenuOp::Open(index));
            Ok(json!({ "menu": name, "index": index }))
        }

        Request::MenuClose => {
            let snapshot = host.menu_bar_snapshot();
            require_in_window_menu(&snapshot)?;
            host.request_menu_op(crate::protocol::MenuOp::Close);
            Ok(Value::Null)
        }

        Request::MenuInvoke { path } => {
            let snapshot = host.menu_bar_snapshot();
            let hit = resolve_menu_item(&snapshot, &path)?;
            host.request_menu_op(crate::protocol::MenuOp::Invoke(hit.action.clone()));
            Ok(json!({
                "path": hit.path,
                "action": hit.action,
                "shortcut": hit.shortcut,
            }))
        }

        Request::TabReorder { tab, index } => {
            let tab_id = find_tab(host.workspace(), tab)?;
            let actual = host
                .workspace_mut()
                .move_tab(tab_id, index)
                .map_err(op_err)?;
            Ok(json!({ "tab": tab_id.as_u64(), "index": actual }))
        }

        Request::MovePane {
            pane,
            tab,
            target,
            direction,
            focus,
        } => {
            let prev_active = host.workspace().active_tab_id();
            let prev_focused = host.workspace().active_tab().tree().focused();
            let (_, source) = resolve_pane(host.workspace(), pane)?;
            match (tab, target) {
                // 従来動作: 別タブの末尾（フォーカス右）へ移送
                (Some(tab), None) => {
                    if direction.is_some() {
                        return Err(DispatchError::InvalidParams(
                            "direction は target 指定時のみ使える".into(),
                        ));
                    }
                    let dest = find_tab(host.workspace(), tab)?;
                    host.workspace_mut()
                        .move_pane(source, dest)
                        .map_err(op_err)?;
                }
                // FR-1.10: target ペインの隣（direction 側）へ挿し直す
                (None, Some(raw)) => {
                    let (_, target) = resolve_pane(host.workspace(), Some(raw))?;
                    host.workspace_mut()
                        .move_pane_to(
                            source,
                            target,
                            direction.unwrap_or(Direction::Right).to_core(),
                        )
                        .map_err(op_err)?;
                }
                // Issue #209: 両方 None → 新タブ化
                (None, None) => {
                    if direction.is_some() {
                        return Err(DispatchError::InvalidParams(
                            "direction は target 指定時のみ使える".into(),
                        ));
                    }
                    host.workspace_mut()
                        .move_pane_to_new_tab(source)
                        .map_err(op_err)?;
                }
                (Some(_), Some(_)) => {
                    return Err(DispatchError::InvalidParams(
                        "tab と target は同時に指定できない".into(),
                    ))
                }
            }
            // CLI/MCP 経由ではアクティブタブ・フォーカスペインを維持（ユーザーの入力を奪わない）
            if !focus.unwrap_or(false) {
                // 移動元タブが閉じていなければ元のアクティブ状態を復元
                if host.workspace().get_tab(prev_active).is_some() {
                    let _ = host.workspace_mut().activate_tab(prev_active);
                    // フォーカスペインがまだ同タブにいれば復元（移動対象だった場合はスキップ）
                    if host
                        .workspace()
                        .get_tab(prev_active)
                        .unwrap()
                        .tree()
                        .contains(prev_focused)
                    {
                        let _ = tree_mut(host.workspace_mut(), prev_active).focus(prev_focused);
                    }
                }
            }
            Ok(Value::Null)
        }

        Request::AutoRename { enabled } => {
            if let Some(enabled) = enabled {
                host.set_auto_rename(enabled);
            }
            Ok(json!({ "enabled": host.auto_rename_enabled() }))
        }

        Request::PortDetect { enabled } => {
            if let Some(enabled) = enabled {
                host.set_port_detect(enabled);
            }
            Ok(json!({ "enabled": host.port_detect_enabled() }))
        }

        Request::ConfirmClose { enabled } => {
            if let Some(val) = enabled {
                host.set_confirm_close(val);
                let _ = crate::setup::mutate_config(|c| c.confirm_close = val);
            }
            Ok(json!({ "enabled": host.confirm_close_enabled() }))
        }

        Request::Persist { enabled } => {
            if let Some(enabled) = enabled {
                host.set_tmux_persist(enabled);
            }
            Ok(json!({
                "enabled": host.tmux_persist_enabled(),
                // セカンダリモード（Issue #113: 多重起動の後発）では復元・保存・切替が
                // 無効。AI / CLI が「切替したのに enabled が変わらない」理由を判別できる
                "secondary": host.is_secondary(),
                // 器の有無（tmux 不在環境では PTY が直接 spawn へ劣化する）。
                // その場合もタブ構成の保存・復元は機能する（復元は新シェル）。
                // 後方互換のため available は残し、詳細は backend に載せる（設計 §6）
                "available": tako_core::backend::capabilities().survives_app_exit,
                "backend": tako_core::backend::capabilities().describe(),
                // 診断（Issue #30）: 保存先の実パスと存在有無・起動時の復元結果・ログ
                "layout_path": crate::layout::layout_path()
                    .map(|p| p.display().to_string()),
                "layout_exists": crate::layout::layout_path()
                    .map(|p| p.is_file())
                    .unwrap_or(false),
                "last_restore": host.persist_restore_report(),
                // 起動時に orphan 自動復帰した tmux セッション数（Issue #191）
                "recovered_count": host.recovered_sessions_count(),
                "log_path": crate::diag::persist_log_path()
                    .map(|p| p.display().to_string()),
            }))
        }

        Request::Panel {
            visible,
            width,
            view,
            filetree,
            sidebar_width,
        } => {
            if let Some(w) = width {
                if !w.is_finite() || w <= 0.0 {
                    return Err(DispatchError::InvalidParams(
                        "width は正の数（px）を指定する".into(),
                    ));
                }
            }
            if let Some(sw) = sidebar_width {
                if !sw.is_finite() || sw <= 0.0 {
                    return Err(DispatchError::InvalidParams(
                        "sidebar_width は正の数（px）を指定する".into(),
                    ));
                }
            }
            host.set_panel(visible, width, view);
            if let Some(filetree) = filetree {
                host.set_filetree(filetree);
            }
            if let Some(sw) = sidebar_width {
                host.set_sidebar_width(sw);
                let mut settings = crate::settings::load();
                settings.sidebar_width = sw as u32;
                let _ = crate::settings::save(&settings);
            }
            let (visible, width, view) = host.panel_state();
            Ok(json!({
                "visible": visible,
                "width": width,
                "view": view.as_str(),
                "filetree": host.filetree_visible(),
                "sidebar_width": host.sidebar_width(),
            }))
        }

        Request::OpenFile {
            pane,
            path,
            mode,
            direction,
            focus,
        } => {
            let (tab, target) = match pane {
                Some(_) => resolve_pane(host.workspace(), pane)?,
                None => {
                    let ws = host.workspace();
                    let active = ws.active_tab_id();
                    let focused = ws.active_tab().tree().focused();
                    (active, focused)
                }
            };
            // 相対パスは対象ペインの cwd（OSC 7。無ければプロセスの cwd）基準で解決する
            let mut resolved = std::path::PathBuf::from(&path);
            if resolved.is_relative() {
                if let Some(cwd) = host.session(target).and_then(|s| s.cwd()) {
                    resolved = cwd.join(resolved);
                }
            }
            let resolved = resolved.canonicalize().map_err(|e| {
                DispatchError::Operation(format!("ファイルを開けない（{path}: {e}）"))
            })?;
            if !resolved.is_file() {
                return Err(DispatchError::Operation(format!(
                    "ファイルではない: {}",
                    resolved.display()
                )));
            }
            let mode =
                mode.unwrap_or_else(|| match resolved.extension().and_then(|e| e.to_str()) {
                    Some(ext) if ext.eq_ignore_ascii_case("md") => PreviewModeWire::Markdown,
                    Some(ext) if ext.eq_ignore_ascii_case("markdown") => PreviewModeWire::Markdown,
                    Some(ext)
                        if matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
                        ) =>
                    {
                        PreviewModeWire::Image
                    }
                    Some(ext) if ext.eq_ignore_ascii_case("pdf") => PreviewModeWire::Pdf,
                    Some(ext)
                        if matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "mp4" | "webm" | "mov" | "avi" | "mkv"
                        ) =>
                    {
                        PreviewModeWire::Video
                    }
                    _ => PreviewModeWire::Code,
                });
            // 表示先の解決: direction 指定（FR-3.11 = D&D のドロップ位置）なら再利用せず
            // 必ずその方向へ分割。省略時は 対象自身がプレビュー > 同タブの既存プレビュー
            // （再利用）> 右分割で新設（ターミナルセッションは起動しない = attach なし）
            let (view_pane, created) = if let Some(direction) = direction {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, direction.to_core(), 0.5, new_pane)
                    .map_err(op_err)?;
                (new_id, true)
            } else if host.preview_state(target).is_some() {
                (target, false)
            } else if let Some(existing) = host.preview_pane_of_tab(tab) {
                (existing, false)
            } else {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, SplitDirection::Right, 0.5, new_pane)
                    .map_err(op_err)?;
                (new_id, true)
            };
            let path_str = resolved.display().to_string();
            host.set_preview(view_pane, &path_str, mode)
                .map_err(DispatchError::Operation)?;
            // CLI/MCP 経由のデフォルトはフォーカスを移さない（ユーザーの入力を奪わない）
            if focus.unwrap_or(false) {
                tree_mut(host.workspace_mut(), tab)
                    .focus(view_pane)
                    .map_err(op_err)?;
            }
            Ok(json!({
                "pane": view_pane.as_u64(),
                "path": path_str,
                "mode": mode.as_str(),
                "created": created,
            }))
        }
        Request::PreviewView {
            pane,
            zoom,
            zoom_in,
            zoom_out,
            reset,
            page,
            pan_x,
            pan_y,
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let controls = usize::from(zoom.is_some())
                + usize::from(zoom_in)
                + usize::from(zoom_out)
                + usize::from(reset);
            if controls > 1 {
                return Err(DispatchError::InvalidParams(
                    "zoom / zoom_in / zoom_out / reset は同時に指定できない".into(),
                ));
            }
            let zoom_command = if let Some(percent) = zoom {
                Some(PreviewZoomCommand::Set(percent / 100.0))
            } else if zoom_in {
                Some(PreviewZoomCommand::In)
            } else if zoom_out {
                Some(PreviewZoomCommand::Out)
            } else if reset {
                Some(PreviewZoomCommand::Reset)
            } else {
                None
            };
            let has_update =
                zoom_command.is_some() || page.is_some() || pan_x.is_some() || pan_y.is_some();
            let state = if has_update {
                host.update_preview_view(
                    target,
                    PreviewViewUpdate {
                        zoom: zoom_command,
                        page,
                        pan_delta: (pan_x.is_some() || pan_y.is_some())
                            .then_some((pan_x.unwrap_or(0.0), pan_y.unwrap_or(0.0))),
                    },
                )
                .map_err(DispatchError::Operation)?
            } else {
                host.preview_view_state(target).ok_or_else(|| {
                    DispatchError::Operation(format!(
                        "PDF・画像プレビューペインではない: {}",
                        target.as_u64()
                    ))
                })?
            };
            Ok(json!({
                "pane": target.as_u64(),
                "zoom": (state.zoom * 100.0).round(),
                "page": state.page,
                "pan_x": state.pan_x,
                "pan_y": state.pan_y,
            }))
        }
        Request::PreviewOutline { pane, item } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let outline = host.preview_outline(target).ok_or_else(|| {
                DispatchError::Operation(format!(
                    "Markdown・PDF プレビューペインではない: {}",
                    target.as_u64()
                ))
            })?;
            let selected = if let Some(item) = item {
                Some(
                    host.navigate_preview_outline(target, item)
                        .map_err(DispatchError::Operation)?,
                )
            } else {
                None
            };
            Ok(json!({
                "pane": target.as_u64(),
                "item": item,
                "selected": selected,
                "outline": outline.items,
            }))
        }
        Request::PreviewLinkList { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            // Markdown（#680）と PDF（#271）でリンクの持ち方が違うので、表示中の
            // 内容に合わせて一覧を出し分ける（応答の `kind` でどちらか分かる）
            if let Some(links) = host.preview_md_links(target) {
                return Ok(json!({
                    "pane": target.as_u64(),
                    "kind": "markdown",
                    "links": links,
                }));
            }
            let links = host.preview_pdf_links(target).ok_or_else(|| {
                DispatchError::Operation(format!(
                    "Markdown・PDF プレビューペインではない: {}",
                    target.as_u64()
                ))
            })?;
            Ok(json!({
                "pane": target.as_u64(),
                "kind": "pdf",
                "links": links.links,
            }))
        }
        Request::PreviewFollowLink { pane, index } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let result = if host.preview_md_links(target).is_some() {
                host.follow_preview_md_link(target, index)
            } else {
                host.follow_preview_pdf_link(target, index)
            }
            .map_err(DispatchError::Operation)?;
            Ok(result)
        }
        Request::PreviewCopyCode { pane, index } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let result = host
                .copy_preview_code_block(target, index)
                .map_err(DispatchError::Operation)?;
            Ok(result)
        }
        Request::PreviewReload { enabled } => {
            if let Some(enabled) = enabled {
                host.set_preview_reload(enabled);
            }
            Ok(json!({ "enabled": host.preview_reload_enabled() }))
        }
        Request::PreviewCache { max_mb } => {
            if let Some(max_mb) = max_mb {
                let max_bytes =
                    tako_core::preview_cache_bytes(max_mb).map_err(DispatchError::InvalidParams)?;
                host.set_preview_cache_budget(max_bytes);
            }
            let stats = host.preview_cache_stats();
            Ok(json!({
                "max_mb": stats.max_bytes / 1024 / 1024,
                "used_bytes": stats.used_bytes,
                "entries": stats.entries,
            }))
        }
        Request::PreviewEdit { pane, enabled } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            if host.preview_state(target).is_none() {
                return Err(DispatchError::Operation(format!(
                    "プレビューペインではない: {}",
                    target.as_u64()
                )));
            }
            if let Some(enabled) = enabled {
                host.set_preview_editing(target, enabled)
                    .map_err(DispatchError::Operation)?;
            }
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
            }))
        }
        Request::PreviewApply { pane, text } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            host.apply_preview_text(target, text)
                .map_err(DispatchError::Operation)?;
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
            }))
        }
        Request::PreviewSave { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            host.save_preview(target)
                .map_err(DispatchError::Operation)?;
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
                "saved": true,
            }))
        }
        Request::PreviewUndo { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let undone = host
                .preview_undo(target)
                .map_err(DispatchError::Operation)?;
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
                "undone": undone,
            }))
        }
        Request::PreviewRedo { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let redone = host
                .preview_redo(target)
                .map_err(DispatchError::Operation)?;
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
                "redone": redone,
            }))
        }
        Request::PreviewAutosave { pane, enabled } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            if let Some(enabled) = enabled {
                host.set_preview_autosave(target, enabled)
                    .map_err(DispatchError::Operation)?;
            }
            let autosave = host.preview_autosave(target).unwrap_or(true);
            Ok(json!({
                "pane": target.as_u64(),
                "autosave": autosave,
            }))
        }
        Request::PreviewSearch {
            pane,
            query,
            direction,
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let result = host
                .preview_search(target, query, direction.as_deref())
                .map_err(DispatchError::Operation)?;
            Ok(json!({
                "pane": target.as_u64(),
                "search": result,
            }))
        }
        Request::PreviewReplace {
            pane,
            query,
            replacement,
            all,
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let result = host
                .preview_replace(target, &query, &replacement, all.unwrap_or(false))
                .map_err(DispatchError::Operation)?;
            let (editing, dirty) = host.preview_edit_state(target).unwrap_or((false, false));
            Ok(json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
                "replace": result,
            }))
        }
        Request::PreviewChangelog {
            pane,
            enabled,
            max_count,
            expand,
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            if host.preview_state(target).is_none() {
                return Err(DispatchError::Operation(format!(
                    "プレビューペインではない: {}",
                    target.as_u64()
                )));
            }
            if let Some(hash) = expand {
                return host
                    .toggle_changelog_diff(target, &hash)
                    .map_err(DispatchError::Operation);
            }
            if let Some(enabled) = enabled {
                let count = max_count.unwrap_or(50);
                return host
                    .set_preview_changelog(target, enabled, count)
                    .map_err(DispatchError::Operation);
            }
            let changelog_on = host.preview_changelog_state(target).unwrap_or(false);
            Ok(json!({
                "pane": target.as_u64(),
                "changelog": changelog_on,
            }))
        }
        Request::FileOp {
            op,
            path,
            name,
            pane,
        } => {
            let path = std::path::PathBuf::from(&path);
            match op {
                FileOpKind::CopyAbsolutePath => {
                    let abs = if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    Ok(json!({ "path": abs.display().to_string() }))
                }
                FileOpKind::CopyRelativePath => {
                    let abs = if path.is_absolute() {
                        path.clone()
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    let rel = if let Some(pane_id) = pane {
                        let (_, target) = resolve_pane(host.workspace(), Some(pane_id))?;
                        host.session(target)
                            .and_then(|s| s.cwd())
                            .and_then(|cwd| pathdiff::diff_paths(&abs, cwd))
                            .unwrap_or_else(|| abs.clone())
                    } else {
                        abs.clone()
                    };
                    Ok(json!({ "path": rel.display().to_string() }))
                }
                FileOpKind::Reveal => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    crate::platform::os_integration::reveal(&path)
                        .map_err(DispatchError::Operation)?;
                    Ok(json!({ "revealed": path.display().to_string() }))
                }
                FileOpKind::OpenTerminal => {
                    let dir = dir_of(&path);
                    let (_, target) = resolve_pane(host.workspace(), pane)?;
                    host.session(target)
                        .ok_or(DispatchError::NoSession(target.as_u64()))?;
                    let cd_text = format!("cd {}\r", shell_escape(&dir.display().to_string()));
                    if let Some(session) = host.session(target) {
                        session.write(cd_text.as_bytes().to_vec());
                    }
                    Ok(json!({ "pane": target.as_u64(), "cwd": dir.display().to_string() }))
                }
                FileOpKind::Rename => {
                    let new_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&new_name)?;
                    let parent = path.parent().ok_or(DispatchError::Operation(
                        "親ディレクトリが取得できない".into(),
                    ))?;
                    let new_path = parent.join(&new_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::rename(&path, &new_path)
                        .map_err(|e| DispatchError::Operation(format!("リネームに失敗: {e}")))?;
                    Ok(
                        json!({ "old": path.display().to_string(), "new": new_path.display().to_string() }),
                    )
                }
                FileOpKind::CreateFile => {
                    let file_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&file_name)?;
                    let new_path = dir_of(&path).join(&file_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::File::create(&new_path).map_err(|e| {
                        DispatchError::Operation(format!("ファイル作成に失敗: {e}"))
                    })?;
                    Ok(json!({ "created": new_path.display().to_string() }))
                }
                FileOpKind::CreateDir => {
                    let dir_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&dir_name)?;
                    let new_path = dir_of(&path).join(&dir_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::create_dir(&new_path).map_err(|e| {
                        DispatchError::Operation(format!("フォルダ作成に失敗: {e}"))
                    })?;
                    Ok(json!({ "created": new_path.display().to_string() }))
                }
                FileOpKind::Trash => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    crate::platform::os_integration::move_to_trash(&path)
                        .map_err(DispatchError::Operation)?;
                    Ok(json!({ "trashed": path.display().to_string() }))
                }
                FileOpKind::OpenDefault => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    crate::platform::os_integration::open_default(&path)
                        .map_err(DispatchError::Operation)?;
                    Ok(json!({ "opened": path.display().to_string() }))
                }
                FileOpKind::OpenWith => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    let app_name = name.ok_or(DispatchError::InvalidParams(
                        "name（アプリ名）を指定する".into(),
                    ))?;
                    crate::platform::os_integration::open_with(&app_name, &path)
                        .map_err(DispatchError::Operation)?;
                    Ok(json!({ "opened": path.display().to_string(), "app": app_name }))
                }
            }
        }
        Request::GitLog { pane, max_count } => {
            // 同期経路（テスト・直呼び用）。IPC / MCP 経由は prepare_offload が
            // cwd 解決（UI）と git 実行（background）に分割する（Issue #115 / #168）
            let cwd = git_pane_cwd(host, pane)?;
            run_git_log(&cwd, max_count)
        }
        Request::GitDiff { pane, target } => {
            let cwd = git_pane_cwd(host, pane)?;
            run_git_diff(&cwd, target.as_deref())
        }
        Request::GitShow { pane, hash, file } => {
            let cwd = git_pane_cwd(host, pane)?;
            run_git_show(&cwd, &hash, file.as_deref())
        }
        Request::GitCommit { pane, message, all } => {
            let cwd = git_pane_cwd(host, pane)?;
            let repo = tako_core::git::repo_root(&cwd)
                .ok_or_else(|| op_err("git リポジトリが見つかりません"))?;
            tako_core::git::commit(&repo, &message, all)
                .map(|out| json!({ "committed": true, "output": out.trim() }))
                .map_err(op_err)
        }
        Request::GitPull { pane } => {
            let cwd = git_pane_cwd(host, pane)?;
            let repo = tako_core::git::repo_root(&cwd)
                .ok_or_else(|| op_err("git リポジトリが見つかりません"))?;
            tako_core::git::pull(&repo)
                .map(|out| json!({ "pulled": true, "output": out.trim() }))
                .map_err(op_err)
        }
        Request::GitPush { pane } => {
            let cwd = git_pane_cwd(host, pane)?;
            let repo = tako_core::git::repo_root(&cwd)
                .ok_or_else(|| op_err("git リポジトリが見つかりません"))?;
            tako_core::git::push(&repo)
                .map(|out| json!({ "pushed": true, "output": out.trim() }))
                .map_err(op_err)
        }
        Request::GitStage { pane, paths } => {
            let cwd = git_pane_cwd(host, pane)?;
            let repo = tako_core::git::repo_root(&cwd)
                .ok_or_else(|| op_err("git リポジトリが見つかりません"))?;
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            tako_core::git::stage(&repo, &path_refs)
                .map(|_| json!({ "staged": true, "paths": paths }))
                .map_err(op_err)
        }
        Request::GitUnstage { pane, paths } => {
            let cwd = git_pane_cwd(host, pane)?;
            let repo = tako_core::git::repo_root(&cwd)
                .ok_or_else(|| op_err("git リポジトリが見つかりません"))?;
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            tako_core::git::unstage(&repo, &path_refs)
                .map(|_| json!({ "unstaged": true, "paths": paths }))
                .map_err(op_err)
        }
        Request::GitCheckout {
            pane,
            branch,
            confirm,
        } => {
            let repo = git_repo_for_pane(host, pane)?;
            run_git_checkout(&repo, &branch, confirm)
        }
        Request::GitBranchCreate {
            pane,
            name,
            start_point,
            checkout,
        } => {
            let repo = git_repo_for_pane(host, pane)?;
            // 既定は「作って切り替える」（#322: 既定動作を賢くする）
            let switch = checkout.unwrap_or(true);
            tako_core::git::create_branch(&repo, &name, start_point.as_deref(), switch)
                .map(|out| {
                    json!({
                        "created": name,
                        "start_point": start_point,
                        "checked_out": switch,
                        "branch": tako_core::git::status(&repo).branch,
                        "output": out,
                    })
                })
                .map_err(op_err)
        }
        Request::GitMerge {
            pane,
            branch,
            confirm,
            no_ff,
        } => {
            let repo = git_repo_for_pane(host, pane)?;
            run_git_merge(&repo, &branch, confirm, no_ff)
        }
        Request::GitMergeAbort { pane } => {
            let repo = git_repo_for_pane(host, pane)?;
            tako_core::git::abort_operation(&repo)
                .map(|(op, out)| {
                    json!({
                        "aborted": op.as_str(),
                        "branch": tako_core::git::status(&repo).branch,
                        "output": out,
                    })
                })
                .map_err(op_err)
        }
        Request::GitConflicts { pane } => {
            let repo = git_repo_for_pane(host, pane)?;
            let state = tako_core::git::conflict_state(&repo);
            Ok(conflict_state_json(&repo, &state))
        }
        Request::GitResolveAgent { pane, agent, tab } => {
            dispatch_git_resolve_agent(host, origin, pane, agent.as_deref(), tab)
        }

        Request::Background { pane, tab } => {
            if let Some(t) = tab {
                let tab_id = find_tab(host.workspace(), t)?;
                let ids = host.workspace_mut().shelve_tab(tab_id).map_err(op_err)?;
                let pane_ids: Vec<u64> = ids.iter().map(|p| p.as_u64()).collect();
                Ok(json!({ "backgrounded_tab": t, "panes": pane_ids }))
            } else {
                let (_, target) = resolve_pane(host.workspace(), pane)?;
                host.workspace_mut().shelve_pane(target).map_err(op_err)?;
                Ok(json!({ "backgrounded": target.as_u64() }))
            }
        }

        Request::Foreground {
            pane,
            target,
            direction,
        } => {
            let pane_id = PaneId::from_raw(pane);
            if !host.workspace().is_shelved(pane_id) {
                return Err(DispatchError::PaneNotFound(pane));
            }
            let target_id = if let Some(t) = target {
                let (_, id) = resolve_pane(host.workspace(), Some(t))?;
                id
            } else {
                let ws = host.workspace();
                ws.shelved_origin_tab(pane_id)
                    .and_then(|tab| ws.get_tab(tab))
                    .map(|tab| tab.tree().focused())
                    .unwrap_or_else(|| ws.active_tab().tree().focused())
            };
            let dir = direction
                .map(|d| d.to_core())
                .unwrap_or(SplitDirection::Right);
            host.workspace_mut()
                .unshelve_pane(pane_id, target_id, dir)
                .map_err(op_err)?;
            host.reattach_backgrounded(pane_id);
            Ok(json!({ "foregrounded": pane, "target": target_id.as_u64() }))
        }

        Request::BackgroundList => {
            let items: Vec<serde_json::Value> = host
                .workspace()
                .shelved_panes()
                .iter()
                .map(|p| {
                    let preview = host.preview_state(p.id());
                    let state = if preview.is_some() {
                        CommandState::Idle
                    } else {
                        host.session(p.id())
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown)
                    };
                    let cwd = host
                        .session(p.id())
                        .and_then(|s| s.cwd())
                        .map(|p| p.display().to_string());
                    let mut entry = json!({
                        "pane": p.id().as_u64(),
                        "title": p.title(),
                        "role": p.role(),
                        "state": format!("{state:?}").to_lowercase(),
                        "cwd": cwd,
                        "origin_tab": p.origin_tab().as_u64(),
                        "origin_tab_title": p.origin_tab_title(),
                        "surface": "background",
                    });
                    if let Some((path, mode)) = preview {
                        entry["preview"] = json!({
                            "path": path,
                            "mode": mode.as_str(),
                        });
                    }
                    entry
                })
                .collect();
            Ok(json!({ "backgrounded": items }))
        }

        Request::CollapseTab {
            pane,
            tab,
            collapsed,
        } => {
            let tab_id = match tab {
                Some(t) => find_tab(host.workspace(), t)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            host.set_tmux_tab_collapsed(tab_id, collapsed);
            Ok(json!({
                "tab": tab_id.as_u64(),
                "collapsed": host.tmux_tab_collapsed(tab_id),
            }))
        }

        Request::Pin {
            pane,
            group_tab,
            pinned,
        } => {
            if let Some(t) = group_tab {
                // 閉じたタブグループ: tab は閉じているので tabs() に無い。バックグラウンドペインの由来で検証
                let tab = TabId::from_raw(t);
                if !host
                    .workspace()
                    .shelved_panes()
                    .iter()
                    .any(|p| p.origin_tab() == tab)
                {
                    return Err(DispatchError::TabNotFound(t));
                }
                host.set_pin_group(tab, pinned);
                Ok(json!({ "pinned": pinned_json(host), "group_tab": t }))
            } else {
                let (_, target) = resolve_pane(host.workspace(), pane)?;
                host.set_pin_pane(target, pinned);
                Ok(json!({ "pinned": pinned_json(host), "pane": target.as_u64() }))
            }
        }

        Request::BackgroundKill { pane } => {
            let pane_id = PaneId::from_raw(pane);
            if host.workspace_mut().remove_shelved(pane_id).is_none() {
                return Err(DispatchError::PaneNotFound(pane));
            }
            host.detach_session(pane_id);
            Ok(json!({ "killed": pane }))
        }

        Request::CheckHealth => Ok(check_health(host)),

        Request::SetupMcp { scope, pane } => {
            let scope_str = scope.as_deref().unwrap_or("global");
            let mcp_scope = match scope_str {
                "project" => {
                    let (_, target) = resolve_pane(host.workspace(), pane)?;
                    let cwd = host
                        .session(target)
                        .and_then(|s| s.cwd())
                        .ok_or(DispatchError::Operation("cwd が取得できない".into()))?;
                    McpScope::Project(cwd.to_path_buf())
                }
                _ => McpScope::User,
            };
            let tako_bin = resolve_tako_binary();
            let result = setup_mcp(&tako_bin, &mcp_scope)?;
            let mut resp = json!({
                "configured": result.configured,
                "already_existed": result.already_existed,
                "target_path": result.target_path.display().to_string(),
                "command": tako_bin,
            });
            if result.repaired {
                resp["repaired"] = json!(true);
                if let Some(old) = &result.old_command {
                    resp["old_command"] = json!(old);
                }
            }
            if result.legacy_cleaned {
                resp["legacy_cleaned"] = json!(true);
            }
            Ok(resp)
        }

        Request::VideoPlayback { pane, action } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            require_video_preview(host, target)?;
            // "status" は状態を変えずに現在値だけを返す（UI 表示との突き合わせ用。#484）
            if action != "status" {
                host.video_playback(target, &action)
                    .map_err(DispatchError::Operation)?;
            }
            Ok(video_response(host, target))
        }

        Request::VideoSeek { pane, seconds } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            require_video_preview(host, target)?;
            let actual = host
                .video_seek(target, seconds)
                .map_err(DispatchError::Operation)?;
            let mut resp = video_response(host, target);
            resp["seconds"] = json!(actual);
            Ok(resp)
        }

        Request::VideoVolume { pane, volume } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            require_video_preview(host, target)?;
            let actual = host
                .video_volume(target, volume)
                .map_err(DispatchError::Operation)?;
            let mut resp = video_response(host, target);
            resp["volume"] = json!(actual);
            Ok(resp)
        }

        Request::OrchestratorProjects {
            action,
            key,
            cwd,
            description,
        } => dispatch_orchestrator_projects(&action, key, cwd, description),

        Request::OrchestratorProfiles {
            action,
            name,
            master_agent,
            clear_master_agent,
            model,
            worker_model,
            effort,
            worker_effort,
            clear_model,
            clear_worker_model,
            worker_agent,
            clear_worker_agent,
            agent,
            agent_model,
            clear_agent_model,
            agent_effort,
            clear_agent_effort,
            agent_skip_permissions,
            agent_args,
            worker_model_policy,
            tab_naming_convention,
            env_set,
            env_unset,
            master_account,
            clear_master_account,
            worker_account,
            clear_worker_account,
        } => dispatch_orchestrator_profiles(ProfilesParams {
            action,
            name,
            master_agent,
            clear_master_agent,
            model,
            worker_model,
            effort,
            worker_effort,
            clear_model,
            clear_worker_model,
            worker_agent,
            clear_worker_agent,
            agent,
            agent_model,
            clear_agent_model,
            agent_effort,
            clear_agent_effort,
            agent_skip_permissions,
            worker_model_policy,
            agent_args,
            tab_naming_convention,
            env_set,
            env_unset,
            master_account,
            clear_master_account,
            worker_account,
            clear_worker_account,
        }),

        // #504: アカウントレジストリの CRUD
        Request::OrchestratorAccounts {
            action,
            name,
            config_dir,
            inherit,
            description,
            default_model,
            default_effort,
            master,
            worker,
            profile,
            pane,
            tab,
        } => dispatch_orchestrator_accounts(
            Some(host),
            origin,
            AccountParams {
                action: &action,
                name: name.as_deref(),
                config_dir: config_dir.as_deref(),
                inherit,
                description: description.as_deref(),
                default_model: default_model.as_deref(),
                default_effort: default_effort.as_deref(),
                master,
                worker,
                profile: profile.as_deref(),
                pane,
                tab,
            },
        ),

        Request::OrchestratorLayout {
            policy,
            master_ratio,
            algorithm,
        } => dispatch_orchestrator_layout(policy.as_deref(), master_ratio, algorithm.as_deref()),

        Request::OrchestratorSelf {
            pane,
            caller_role,
            caller_pid,
        } => dispatch_orchestrator_self(host, pane, caller_role.as_deref(), caller_pid),

        Request::OrchestratorHandoff {
            pane,
            caller_role,
            tab,
            caller_pid,
        } => dispatch_orchestrator_handoff(
            host,
            origin,
            pane,
            caller_role.as_deref(),
            tab,
            caller_pid,
        ),

        Request::OrchestratorSpawn {
            project,
            prompt,
            label,
            model,
            effort,
            pane,
            tab,
            caller_role,
            agent,
            caller_pid,
            task_type,
            account,
        } => dispatch_orchestrator_spawn(
            host,
            origin,
            SpawnParams {
                project: &project,
                prompt: &prompt,
                label: label.as_deref(),
                model: model.as_deref(),
                effort: effort.as_deref(),
                pane,
                tab,
                caller_role: caller_role.as_deref(),
                agent: agent.as_deref(),
                caller_pid,
                task_type: task_type.as_deref(),
                account: account.as_deref(),
            },
        ),

        // 通常は UI 層（tako-app の IPC ループ）が snapshot / compute を二段で実行して
        // ここへ来ない（#181: compute の claude CLI 起動が UI を専有するため background 化）。
        // CLI 直呼びやテストなど ControlHost が UI スレッドに縛られない経路のフォールバック
        Request::OrchestratorWorkerStatus {
            pane_id,
            session_id,
            tmux_session,
            worker,
        } => {
            // 同期経路（テスト・直呼び用）。IPC / MCP 経由は prepare_offload が
            // collect（UI）と finish（background）に分割して実行する（#168 / #181）
            let q = resolve_worker_query(pane_id, worker.as_deref(), session_id, tmux_session)?;
            let ctx = verify_ctx_pane_identity(
                collect_worker_status_ctx(host, q.pane_id),
                q.tmux_session.as_deref(),
            );
            finish_worker_status(ctx, q.session_id.as_deref(), q.tmux_session.as_deref())
        }

        // #390: worker レジストリの一覧（同期経路。IPC / MCP 経由は prepare_offload 側）
        Request::OrchestratorWorkers { all } => {
            let live_panes = collect_live_panes(host);
            let sweep = !host.is_secondary();
            finish_workers_list(&live_panes, all.unwrap_or(false), sweep)
        }

        // 非同期 run の進捗照会・結果回収（#121）。レジストリはプロセス内グローバルで
        // ControlHost 不要のため dispatch で直接呼ぶ
        Request::OrchestratorRunStatus { run_id } => match run_id {
            Some(id) => {
                crate::orchestrator::wait::run_status(&id).map_err(DispatchError::Operation)
            }
            None => Ok(crate::orchestrator::wait::run_list()),
        },
        Request::OrchestratorRunResult { run_id } => {
            let exec: &mut dyn FnMut(Request) -> Result<Value, String> =
                &mut |req| dispatch(host, req, origin).map_err(|e| e.to_string());
            crate::orchestrator::wait::run_result(&run_id, exec).map_err(DispatchError::Operation)
        }

        // #319 / #662: permission ダイアログ・対話ダイアログへの構造化応答
        Request::OrchestratorRespond {
            pane_id,
            choice,
            answers,
            dry_run,
            caller_role,
        } => dispatch_orchestrator_respond(
            host,
            pane_id,
            choice.as_deref(),
            answers.as_deref(),
            dry_run,
            caller_role.as_deref(),
        ),

        // #662: worker が表示中の対話ダイアログの内容取得
        Request::OrchestratorDialog { pane_id, worker } => {
            let q = resolve_worker_query(pane_id, worker.as_deref(), None, None)?;
            dispatch_orchestrator_dialog(host, q)
        }

        // #364: worker の報告内容を scrollback + transcript から取得
        // #390: worker 指定 / pane 消失時はレジストリの追跡キーで継続
        Request::OrchestratorReport {
            pane_id,
            lines,
            messages,
            worker,
        } => {
            let q = resolve_worker_query(pane_id, worker.as_deref(), None, None)?;
            dispatch_orchestrator_report(host, q, lines.unwrap_or(2000), messages.unwrap_or(1))
        }

        Request::OrchestratorLaunchStatus { pane, worker } => {
            dispatch_orchestrator_launch_status(host, pane, worker.as_deref())
        }

        Request::OrchestratorSupervisor {
            action,
            mode,
            auto_resume_dead,
            max_retries,
            lines,
        } => dispatch_orchestrator_supervisor(
            &action,
            mode.as_deref(),
            auto_resume_dead,
            max_retries,
            lines,
        ),

        Request::OrchestratorLedger {
            action,
            id,
            outcome,
            rounds,
            note,
            project,
            task_type,
            limit,
        } => dispatch_orchestrator_ledger(LedgerParams {
            action,
            id,
            outcome,
            rounds,
            note,
            project,
            task_type,
            limit,
        }),

        Request::RemoteStart {} => host.remote_start().map_err(DispatchError::Operation),
        Request::RemoteStop { force } => {
            if force {
                crate::remote::daemon_force_stop().map_err(DispatchError::Operation)
            } else {
                host.remote_stop().map_err(DispatchError::Operation)
            }
        }
        Request::RemoteStatus => Ok(host.remote_status()),

        // エージェント一覧と会話ログはどのプロセスでも取得できる（ControlHost 不要）
        Request::RemoteAgents => {
            crate::agents::list_agents_with_panes(None).map_err(DispatchError::Operation)
        }

        Request::RemoteMessages { session_id, tail } => {
            crate::transcript::read_messages(&session_id, tail.unwrap_or(30))
                .map_err(DispatchError::Operation)
        }

        // ペアリング済み端末の管理（#283）。承認・role 変更はここに存在しない
        // （Mac 画面の GUI ダイアログ限定 = AI フルコントロール不変条件の例外）
        Request::RemoteDevices { action, device_id } => match action.as_str() {
            "list" => crate::remote::devices_list().map_err(DispatchError::Operation),
            "revoke" => {
                let id = device_id.ok_or_else(|| {
                    DispatchError::Operation("revoke には device_id が必要".to_string())
                })?;
                crate::remote::devices_revoke(&id).map_err(DispatchError::Operation)
            }
            other => Err(DispatchError::Operation(format!(
                "不明な action: {other}（list / revoke）"
            ))),
        },

        Request::RemoteScrollback { pane_id, lines } => {
            let result = crate::remote::scrollback(&pane_id, lines.unwrap_or(1000))
                .map_err(DispatchError::Operation)?;
            Ok(json!({ "lines": result }))
        }

        Request::RemoteSetup { action, answers } => match action.as_str() {
            "run" => {
                let parsed: crate::remote_setup::RemoteSetupAnswers = answers
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| {
                        DispatchError::InvalidParams(format!("remote setup answers が不正: {e}"))
                    })?
                    .unwrap_or_default();
                crate::remote_setup::run_noninteractive(&parsed).map_err(DispatchError::Operation)
            }
            "check" => Ok(crate::remote_setup::check_status()),
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other}（run / check）"
            ))),
        },

        Request::Web {
            action,
            url,
            id,
            pane,
            direction,
            to,
            js,
            token,
            focus,
        } => {
            // ペイン分割を伴う action（open / show）の共通処理。
            // 分割 → host フック → 失敗なら巻き戻し、成功なら focus 指定時のみフォーカス移動
            let should_focus = focus.unwrap_or(false);
            let split_and =
                |host: &mut dyn ControlHost,
                 pane: Option<u64>,
                 attach: &dyn Fn(&mut dyn ControlHost, PaneId) -> Result<Value, String>|
                 -> Result<Value, DispatchError> {
                    let (tab, target) = match pane {
                        Some(_) => resolve_pane(host.workspace(), pane)?,
                        None => {
                            let ws = host.workspace();
                            (ws.active_tab_id(), ws.active_tab().tree().focused())
                        }
                    };
                    let dir = direction
                        .map(|d| d.to_core())
                        .unwrap_or(SplitDirection::Right);
                    let new_pane = Pane::new(origin);
                    let new_id = new_pane.id();
                    tree_mut(host.workspace_mut(), tab)
                        .split_with_ratio(target, dir, 0.5, new_pane)
                        .map_err(op_err)?;
                    match attach(host, new_id) {
                        Ok(v) => {
                            // CLI/MCP 経由のデフォルトはフォーカスを移さない（ユーザーの入力を奪わない）
                            if should_focus {
                                tree_mut(host.workspace_mut(), tab)
                                    .focus(new_id)
                                    .map_err(op_err)?;
                            } else {
                                let _ = tree_mut(host.workspace_mut(), tab).focus(target);
                            }
                            Ok(v)
                        }
                        Err(e) => {
                            let _ = tree_mut(host.workspace_mut(), tab).close(new_id);
                            Err(DispatchError::Operation(e))
                        }
                    }
                };
            // 表示中の Web ビューをペインから外す共通処理（hide / close）。
            // Request::Close と同じ後始末（LastPane はタブごと閉じる + detach_session）
            let close_pane_of =
                |host: &mut dyn ControlHost, pane_id: PaneId| -> Result<(), DispatchError> {
                    let (tab, target) = resolve_pane(host.workspace(), Some(pane_id.as_u64()))?;
                    match tree_mut(host.workspace_mut(), tab).close(target) {
                        Ok(_) => {}
                        Err(PaneTreeError::LastPane) => {
                            host.workspace_mut().close_tab(tab).map_err(op_err)?;
                        }
                        Err(e) => return Err(op_err(e)),
                    }
                    host.detach_session(target);
                    Ok(())
                };
            match action.as_str() {
                "open" => {
                    let url = url.ok_or(DispatchError::InvalidParams("url は必須".into()))?;
                    if url.trim().is_empty() {
                        return Err(DispatchError::InvalidParams(
                            "URL が空です".into(),
                        ));
                    }
                    split_and(host, pane, &|h, new_id| h.web_open(new_id, &url))
                }
                "show" => {
                    let id =
                        id.ok_or(DispatchError::InvalidParams("id は必須（web list で確認）".into()))?;
                    // 既に表示中なら分割しない。focus 指定時のみフォーカス移動
                    let (_, showing) = host.web_target(Some(id), None).map_err(op_err)?;
                    if let Some(p) = showing {
                        let (tab, target) = resolve_pane(host.workspace(), Some(p.as_u64()))?;
                        if should_focus {
                            let ws = host.workspace_mut();
                            tree_mut(ws, tab).focus(target).map_err(op_err)?;
                            ws.activate_tab(tab).map_err(op_err)?;
                        }
                        return Ok(json!({ "id": id, "pane": target.as_u64(), "already_shown": true }));
                    }
                    split_and(host, pane, &|h, new_id| h.web_show(new_id, id))
                }
                "list" => Ok(host.web_list()),
                "hide" => {
                    let (id, showing) = host.web_target(id, pane).map_err(op_err)?;
                    let shown = showing.ok_or(DispatchError::Operation(format!(
                        "Web ビュー {id} は既に dock 退避中"
                    )))?;
                    close_pane_of(host, shown)?;
                    Ok(json!({ "id": id, "hidden": true }))
                }
                "close" => {
                    let (id, _) = host.web_target(id, pane).map_err(op_err)?;
                    if let Some(shown) = host.web_destroy(id) {
                        close_pane_of(host, shown)?;
                    }
                    Ok(json!({ "id": id, "closed": true }))
                }
                "navigate" => {
                    let to =
                        to.ok_or(DispatchError::InvalidParams(
                            "to は必須（back / forward / reload / URL）".into(),
                        ))?;
                    let (id, _) = host.web_target(id, pane).map_err(op_err)?;
                    host.web_navigate(id, &to).map_err(op_err)
                }
                "eval" => {
                    let js = js.ok_or(DispatchError::InvalidParams("js は必須".into()))?;
                    let (id, _) = host.web_target(id, pane).map_err(op_err)?;
                    host.web_eval(id, &js).map_err(op_err)
                }
                "eval_result" => {
                    let token =
                        token.ok_or(DispatchError::InvalidParams("token は必須".into()))?;
                    let (id, _) = host.web_target(id, pane).map_err(op_err)?;
                    host.web_eval_result(id, token).map_err(op_err)
                }
                "read" => {
                    let (id, _) = host.web_target(id, pane).map_err(op_err)?;
                    host.web_read(id).map_err(op_err)
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "未知の action: {other}（open / list / show / hide / close / navigate / eval / eval_result / read）"
                ))),
            }
        }

        Request::Update {
            action,
            channel,
            dry_run,
        } => {
            let action = action.as_deref().unwrap_or("status");
            let ch = channel.as_deref();
            let dry = dry_run.unwrap_or(false);
            // dry_run は apply でしか意味を持たない。黙って無視すると
            // 「実行しないつもりで叩いたのに実行された」を招くので、はっきり断る
            if dry && action != "apply" {
                return Err(DispatchError::InvalidParams(format!(
                    "dry_run は action=apply でのみ指定できる（渡された action: {action:?}）"
                )));
            }
            match action {
                "status" => Ok(host.update_status()),
                "check" => Ok(host.update_check(ch)),
                "apply" if dry => host
                    .update_apply_dry_run(ch)
                    .map_err(DispatchError::Operation),
                "apply" => host.update_apply(ch).map_err(DispatchError::Operation),
                "apply-zip" => host.update_apply_zip(ch).map_err(DispatchError::Operation),
                "repair" => host.update_repair().map_err(DispatchError::Operation),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / check / apply / apply-zip / repair のいずれか）"
                ))),
            }
        }

        Request::Fda { action } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => Ok(crate::fda::status_info().to_json()),
                "open" => {
                    crate::fda::open_settings().map_err(DispatchError::Operation)?;
                    Ok(serde_json::json!({ "opened": true }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / open のいずれか）"
                ))),
            }
        }

        Request::SetupChanges => {
            // 読み取り専用・プロセス内完結（アプリ状態に依存しない）。
            // 追従の適用は `tako setup` の自動適用フロー側の責務（Issue #94）
            crate::setup::changes_status().map_err(DispatchError::Operation)
        }

        Request::SetupRun { answers } => {
            let answers_value = answers.clone().unwrap_or_else(|| serde_json::json!({}));
            let parsed: crate::setup::SetupAnswers = serde_json::from_value(answers_value)
                .map_err(|e| DispatchError::InvalidParams(format!("setup answers が不正: {e}")))?;
            parsed.validate().map_err(DispatchError::InvalidParams)?;
            let answers_json = serde_json::to_string(&parsed).map_err(|e| {
                DispatchError::Operation(format!("setup answers の JSON 化に失敗: {e}"))
            })?;
            let tako_bin = resolve_tako_binary();
            run_setup_cli(&tako_bin, &answers_json)
        }

        Request::AgentsSyncRules {
            action,
            source,
            targets,
        } => {
            let action = action.as_deref().unwrap_or("sync");
            match action {
                "sync" => crate::agents_sync::run_sync(source.as_deref(), targets.as_deref())
                    .map_err(DispatchError::Operation),
                "status" => crate::agents_sync::status().map_err(DispatchError::Operation),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（sync / status のいずれか）"
                ))),
            }
        }

        Request::SleepGuard {
            action,
            mode,
            power_condition,
            lid_sleep_mode,
        } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => {
                    let settings = crate::settings::load();
                    Ok(crate::sleep_guard::status(
                        settings.sleep_guard_mode,
                        settings.sleep_guard_power,
                        settings.lid_sleep_mode,
                    )
                    .to_json())
                }
                "set" => {
                    let mut settings = crate::settings::load();
                    if let Some(m) = mode.as_deref() {
                        settings.sleep_guard_mode =
                            crate::sleep_guard::SleepGuardMode::from_str_opt(m).ok_or_else(
                                || {
                                    DispatchError::InvalidParams(format!(
                                    "不明な mode: {m:?}（off / on / while-agents-running のいずれか）"
                                ))
                                },
                            )?;
                    }
                    if let Some(pc) = power_condition.as_deref() {
                        settings.sleep_guard_power =
                            crate::sleep_guard::PowerCondition::from_str_opt(pc).ok_or_else(
                                || {
                                    DispatchError::InvalidParams(format!(
                                        "不明な power_condition: {pc:?}（ac-only / always のいずれか）"
                                    ))
                                },
                            )?;
                    }
                    if let Some(lsm) = lid_sleep_mode.as_deref() {
                        settings.lid_sleep_mode =
                            crate::sleep_guard::LidSleepMode::from_str_opt(lsm).ok_or_else(
                                || {
                                    DispatchError::InvalidParams(format!(
                                        "不明な lid_sleep_mode: {lsm:?}（off / while-agents-running のいずれか）"
                                    ))
                                },
                            )?;
                    }
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                    Ok(crate::sleep_guard::status(
                        settings.sleep_guard_mode,
                        settings.sleep_guard_power,
                        settings.lid_sleep_mode,
                    )
                    .to_json())
                }
                "install-lid-sleep" => {
                    let result = crate::sleep_guard::install_sudoers()
                        .map_err(DispatchError::Operation)?;
                    let mut settings = crate::settings::load();
                    settings.lid_sleep_mode =
                        crate::sleep_guard::LidSleepMode::WhileAgentsRunning;
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                    Ok(serde_json::json!({
                        "result": result,
                        "lid_sleep_mode": "while-agents-running",
                        "sudoers_installed": true,
                    }))
                }
                "remove-lid-sleep" => {
                    let result = crate::sleep_guard::remove_sudoers()
                        .map_err(DispatchError::Operation)?;
                    let mut settings = crate::settings::load();
                    settings.lid_sleep_mode = crate::sleep_guard::LidSleepMode::Off;
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                    Ok(serde_json::json!({
                        "result": result,
                        "lid_sleep_mode": "off",
                        "sudoers_installed": false,
                    }))
                }
                "open-battery-settings" => {
                    crate::sleep_guard::open_battery_settings()
                        .map_err(DispatchError::Operation)?;
                    Ok(serde_json::json!({
                        "result": "System Settings の Battery を開きました",
                    }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / set / install-lid-sleep / remove-lid-sleep / open-battery-settings のいずれか）"
                ))),
            }
        }

        Request::Theme {
            action,
            mode,
            target,
            key,
            value,
            name,
            font_family,
            font_size,
        } => {
            use tako_core::theme::{parse_hex_color, Theme, ThemeMode};
            let action = action.as_deref().unwrap_or("status");
            let should_save = !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none();
            let make_status = |host: &dyn ControlHost| {
                let settings = crate::settings::load();
                let presets: Vec<String> = settings.theme_presets.keys().cloned().collect();
                serde_json::json!({
                    "theme": settings.theme,
                    "mode": host.theme_mode().as_str(),
                    "available": ["dark", "light"],
                    "presets": presets,
                })
            };
            match action {
                "status" => Ok(make_status(host)),
                "set" | "toggle" => {
                    let m = mode.as_deref();
                    let next_theme: String = match action {
                        "set" => {
                            let m = m.ok_or_else(|| {
                                DispatchError::InvalidParams(
                                    "set には mode が必要（dark / light / プリセット名）".into(),
                                )
                            })?;
                            m.to_string()
                        }
                        _ => {
                            match host.theme_mode() {
                                ThemeMode::Dark => "light".into(),
                                ThemeMode::Light => "dark".into(),
                            }
                        }
                    };
                    if should_save {
                        let mut settings = crate::settings::load();
                        settings.theme = next_theme.clone();
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    let mode = ThemeMode::parse(&next_theme).unwrap_or_default();
                    host.set_theme_mode(mode);
                    if should_save {
                        // 保存した設定（プリセット・色オーバーライド）を読み直して適用する。
                        // 保存をスキップしたとき（セルフテスト・単体テスト）に呼ぶと、
                        // ディスク上の古いテーマを読み直して今の適用を即座に巻き戻す
                        host.reload_theme();
                    }
                    let mut status = make_status(host);
                    if !should_save {
                        // settings.json を書いていないので、status の theme は
                        // ディスクの古い値になる。今適用した値で上書きして返す
                        status["theme"] = serde_json::json!(next_theme);
                    }
                    Ok(status)
                }
                "colors" => {
                    let settings = crate::settings::load();
                    let (theme, _) = settings.resolve_theme();
                    let empty = std::collections::BTreeMap::new();
                    let overrides = match settings.theme_presets.get(&settings.theme) {
                        Some(p) => &p.colors,
                        None => settings.theme_colors.get(&settings.theme)
                            .or_else(|| settings.theme_colors.get(host.theme_mode().as_str()))
                            .unwrap_or(&empty),
                    };
                    let colors: serde_json::Value = Theme::COLOR_KEYS.iter().map(|&k| {
                        let source = if overrides.contains_key(k) { "override" } else { "builtin" };
                        (k.to_string(), serde_json::json!({
                            "hex": theme.color(k).map(|c| c.to_hex()).unwrap_or_default(),
                            "source": source,
                        }))
                    }).collect::<serde_json::Map<String, serde_json::Value>>().into();
                    Ok(serde_json::json!({ "theme": settings.theme, "colors": colors }))
                }
                "set-color" => {
                    let k = key.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("set-color には key が必要".into())
                    })?;
                    let v = value.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("set-color には value が必要（#RRGGBB）".into())
                    })?;
                    if !Theme::COLOR_KEYS.contains(&k) {
                        return Err(DispatchError::InvalidParams(format!("未知の色キー: {k}")));
                    }
                    parse_hex_color(v).ok_or_else(|| {
                        DispatchError::InvalidParams(format!("不正な色値: {v}（#RRGGBB 形式が必要）"))
                    })?;
                    if should_save {
                        let mut settings = crate::settings::load();
                        let tgt = target.as_deref().unwrap_or(&settings.theme).to_string();
                        if settings.theme_presets.contains_key(&tgt) {
                            settings.theme_presets.get_mut(&tgt).unwrap().colors.insert(k.to_string(), v.to_string());
                        } else {
                            let mode_key = ThemeMode::parse(&tgt).unwrap_or(host.theme_mode()).as_str().to_string();
                            settings.theme_colors.entry(mode_key).or_default().insert(k.to_string(), v.to_string());
                        }
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true, "key": k, "value": v }))
                }
                "reset-color" => {
                    let k = key.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("reset-color には key が必要".into())
                    })?;
                    if should_save {
                        let mut settings = crate::settings::load();
                        let tgt = target.as_deref().unwrap_or(&settings.theme).to_string();
                        if let Some(p) = settings.theme_presets.get_mut(&tgt) {
                            p.colors.remove(k);
                        } else {
                            let mode_key = ThemeMode::parse(&tgt).unwrap_or(host.theme_mode()).as_str().to_string();
                            if let Some(m) = settings.theme_colors.get_mut(&mode_key) {
                                m.remove(k);
                            }
                        }
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true, "key": k }))
                }
                "reset-colors" => {
                    if should_save {
                        let mut settings = crate::settings::load();
                        let tgt = target.as_deref().unwrap_or(&settings.theme).to_string();
                        if let Some(p) = settings.theme_presets.get_mut(&tgt) {
                            p.colors.clear();
                        } else {
                            let mode_key = ThemeMode::parse(&tgt).unwrap_or(host.theme_mode()).as_str().to_string();
                            settings.theme_colors.remove(&mode_key);
                        }
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true }))
                }
                "save-preset" => {
                    let n = name.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("save-preset には name が必要".into())
                    })?;
                    if n == "dark" || n == "light" {
                        return Err(DispatchError::InvalidParams(format!("{n} は予約名")));
                    }
                    if !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || n.is_empty() || n.len() > 32 {
                        return Err(DispatchError::InvalidParams("名前は [a-z0-9-]{1,32}".into()));
                    }
                    if should_save {
                        let mut settings = crate::settings::load();
                        let (resolved, _) = settings.resolve_theme();
                        let base_mode = resolved.mode;
                        let builtin = Theme::for_mode(base_mode);
                        let mut diff = std::collections::BTreeMap::new();
                        for &k in Theme::COLOR_KEYS {
                            if let (Some(cur), Some(blt)) = (resolved.color(k), builtin.color(k)) {
                                if cur != blt {
                                    diff.insert(k.to_string(), cur.to_hex());
                                }
                            }
                        }
                        settings.theme_presets.insert(n.to_string(), crate::settings::ThemePreset {
                            base: base_mode.as_str().into(),
                            colors: diff,
                        });
                        settings.theme = n.to_string();
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true, "name": n }))
                }
                "delete-preset" => {
                    let n = name.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("delete-preset には name が必要".into())
                    })?;
                    if should_save {
                        let mut settings = crate::settings::load();
                        if let Some(p) = settings.theme_presets.remove(n) {
                            if settings.theme == n {
                                settings.theme = p.base;
                            }
                        }
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true }))
                }
                "set-font" => {
                    if should_save {
                        let mut settings = crate::settings::load();
                        if let Some(ref f) = font_family {
                            settings.font_family = Some(f.clone());
                        }
                        if let Some(s) = font_size {
                            settings.font_size = Some(s.clamp(8.0, 32.0));
                        }
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.reload_theme();
                    Ok(serde_json::json!({ "ok": true }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / set / toggle / colors / set-color / reset-color / reset-colors / save-preset / delete-preset / set-font のいずれか）"
                ))),
            }
        }

        // 対応マトリクスの参照（#515）。静的な表を引くだけなのでホスト状態に触れない。
        // CLI・MCP とも crate::platform::report を通るので表示が食い違わない
        Request::Platform { platform, status } => {
            crate::platform::report(platform.as_deref(), status.as_deref())
                .map_err(DispatchError::InvalidParams)
        }

        Request::Lang { action, value } => {
            use tako_core::i18n::{self, LangSetting};
            let action = action.as_deref().unwrap_or("status");
            let status_json = |setting: LangSetting| {
                serde_json::json!({
                    "language": setting.as_str(),
                    "resolved": i18n::lang().as_str(),
                    "available": ["system", "ja", "en"],
                })
            };
            match action {
                "status" => Ok(status_json(host.ui_lang_setting())),
                "set" => {
                    let v = value.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams(
                            "set には value が必要（system / ja / en）".into(),
                        )
                    })?;
                    let setting = LangSetting::parse(v).ok_or_else(|| {
                        DispatchError::InvalidParams(format!(
                            "不明な value: {v:?}（system / ja / en のいずれか）"
                        ))
                    })?;
                    // 永続化（テスト・セルフテスト中はユーザー設定を汚さない。Theme と同方針）
                    if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                        let mut settings = crate::settings::load();
                        settings.language = setting.as_str().into();
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    let resolved = setting.resolve();
                    host.set_ui_lang(setting, resolved);
                    Ok(serde_json::json!({
                        "language": setting.as_str(),
                        "resolved": resolved.as_str(),
                        "available": ["system", "ja", "en"],
                    }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / set のいずれか）"
                ))),
            }
        }

        Request::Telemetry { action } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => {
                    let enabled = crate::telemetry::is_enabled();
                    let recent = crate::telemetry::recent_count();
                    let queued = crate::telemetry::queue_count();
                    let log_path =
                        crate::telemetry::log_file_path().map(|p| p.display().to_string());
                    Ok(serde_json::json!({
                        "telemetry": enabled,
                        "recent_reports": recent,
                        "queued_reports": queued,
                        "log_path": log_path,
                    }))
                }
                "on" | "off" => {
                    let enabled = action == "on";
                    crate::telemetry::set_enabled(enabled);
                    if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                        let mut settings = crate::settings::load();
                        settings.telemetry = enabled;
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    Ok(serde_json::json!({
                        "telemetry": enabled,
                    }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / on / off のいずれか）"
                ))),
            }
        }

        Request::LimitService { action, service } => {
            use tako_core::LimitService as LS;
            let action = action.as_deref().unwrap_or("status");
            let status_json = |svc: LS| {
                serde_json::json!({
                    "limit_service": svc.as_str(),
                    "available": LS::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                })
            };
            match action {
                "status" => Ok(status_json(host.limit_service())),
                "set" => {
                    let s = service.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams(
                            "set には service が必要（claude / codex / agy）".into(),
                        )
                    })?;
                    let next = LS::parse(s).ok_or_else(|| {
                        DispatchError::InvalidParams(format!(
                            "不明な service: {s:?}（claude / codex / agy のいずれか）"
                        ))
                    })?;
                    if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                        let mut settings = crate::settings::load();
                        settings.limit_service = next.as_str().into();
                        crate::settings::save(&settings).map_err(|e| {
                            DispatchError::Operation(format!("設定の保存に失敗: {e}"))
                        })?;
                    }
                    host.set_limit_service(next);
                    Ok(status_json(next))
                }
                "refresh" => Ok(host.refresh_limits()),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / set / refresh のいずれか）"
                ))),
            }
        }

        Request::TreeFolder {
            action,
            path,
            tab,
            pane,
        } => dispatch_tree_folder(host, &action, path, tab, pane),

        Request::Sessions {
            action,
            id,
            role,
            project,
            limit,
            pane,
            tab,
            direction,
        } => match action.as_str() {
            "list" => crate::sessions::list_payload(
                role.as_deref(),
                project.as_deref(),
                limit.unwrap_or(30),
            )
            .map_err(DispatchError::Operation),
            "show" => {
                let id =
                    id.ok_or_else(|| DispatchError::InvalidParams("show には id が必要".into()))?;
                crate::sessions::show_payload(&id).map_err(DispatchError::Operation)
            }
            "resume" => {
                let id =
                    id.ok_or_else(|| DispatchError::InvalidParams("resume には id が必要".into()))?;
                dispatch_sessions_resume(host, origin, &id, pane, tab, direction)
            }
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（list / show / resume のいずれか）"
            ))),
        },

        Request::Logs {
            action,
            pane,
            session_id,
            lines,
            enabled,
            max_mb,
            total_max_mb,
        } => match action.as_str() {
            "list" => {
                let dir = tako_core::pane_log::log_dir().ok_or_else(|| {
                    DispatchError::Operation("データディレクトリを解決できない".into())
                })?;
                let files: Vec<Value> = tako_core::pane_log::list_files(&dir)
                    .into_iter()
                    .map(|f| {
                        json!({
                            "path": f.path,
                            "pane": f.pane,
                            "tab": f.tab,
                            "size": f.size,
                            "modified": f.modified,
                        })
                    })
                    .collect();
                Ok(json!({ "dir": dir, "files": files }))
            }
            "read" => dispatch_logs_read(host, pane, session_id.as_deref(), lines),
            "status" => {
                let config = host.pane_log_config();
                Ok(pane_log_status_json(&config))
            }
            "set" => {
                let mut settings = crate::settings::load();
                if let Some(e) = enabled {
                    settings.pane_logs = e;
                }
                if let Some(m) = max_mb {
                    if m == 0 {
                        return Err(DispatchError::InvalidParams(
                            "max_mb は 1 以上を指定する".into(),
                        ));
                    }
                    settings.pane_log_max_mb = m;
                }
                if let Some(t) = total_max_mb {
                    if t == 0 {
                        return Err(DispatchError::InvalidParams(
                            "total_max_mb は 1 以上を指定する".into(),
                        ));
                    }
                    settings.pane_log_total_max_mb = t;
                }
                crate::settings::save(&settings)
                    .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                let config = settings.pane_log_config();
                host.apply_pane_log_config(config);
                Ok(pane_log_status_json(&config))
            }
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（list / read / status / set のいずれか）"
            ))),
        },

        Request::OpenDir { path, focus } => {
            let dir = PathBuf::from(&path);
            if !dir.is_dir() {
                return Err(DispatchError::InvalidParams(format!(
                    "ディレクトリが存在しない: {path}"
                )));
            }
            let dir = dir.canonicalize().unwrap_or(dir);
            let label = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());

            let prev_active = host.workspace().active_tab_id();
            let pane = Pane::new(origin);
            let pane_id = pane.id();
            let tab_id = host.workspace_mut().create_tab(label, pane);

            if !focus.unwrap_or(true) {
                let _ = host.workspace_mut().activate_tab(prev_active);
            }

            host.attach_session(
                pane_id,
                SpawnOptions {
                    cwd: Some(dir.clone()),
                    ..Default::default()
                },
            );
            // ファイルツリーにフォルダを追加
            if let Some(tab) = host.workspace_mut().get_tab_mut(tab_id) {
                tab.add_pinned_folder(dir.clone());
            }
            host.sync_filetree();

            // Recent に記録
            let mut recent = tako_core::recent::RecentList::load();
            recent.push(tako_core::recent::RecentEntry::Directory {
                path: dir.to_string_lossy().to_string(),
            });
            recent.save();

            Ok(json!({ "tab": tab_id.as_u64(), "pane": pane_id.as_u64() }))
        }

        Request::OpenRemote {
            host: ssh_host,
            focus,
        } => {
            let hosts = match tako_core::ssh_config::default_ssh_config_path() {
                Some(p) => tako_core::ssh_config::parse_ssh_config(&p),
                None => Vec::new(),
            };
            let entry = hosts.iter().find(|h| h.name == ssh_host);
            let cmd = match entry {
                Some(h) => h.ssh_command(),
                None => vec!["ssh".to_string(), ssh_host.clone()],
            };

            let prev_active = host.workspace().active_tab_id();
            let pane = Pane::new(origin);
            let pane_id = pane.id();
            let tab_title = format!("ssh:{ssh_host}");
            let tab_id = host.workspace_mut().create_tab(tab_title, pane);
            if let Some(tab) = host.workspace_mut().get_tab_mut(tab_id) {
                let t = tab.title().to_string();
                tab.set_title_manual(t);
            }

            if !focus.unwrap_or(true) {
                let _ = host.workspace_mut().activate_tab(prev_active);
            }

            host.attach_session(
                pane_id,
                SpawnOptions {
                    command: Some(SpawnCommand {
                        program: cmd[0].clone(),
                        args: cmd[1..].to_vec(),
                    }),
                    ..Default::default()
                },
            );

            // Recent に記録
            let mut recent = tako_core::recent::RecentList::load();
            recent.push(tako_core::recent::RecentEntry::Ssh {
                host: ssh_host.clone(),
            });
            recent.save();

            Ok(json!({ "tab": tab_id.as_u64(), "pane": pane_id.as_u64() }))
        }

        Request::SshHosts => {
            let hosts = match tako_core::ssh_config::default_ssh_config_path() {
                Some(p) => tako_core::ssh_config::parse_ssh_config(&p),
                None => Vec::new(),
            };
            let list: Vec<Value> = hosts
                .iter()
                .map(|h| {
                    json!({
                        "name": h.name,
                        "hostname": h.hostname,
                        "user": h.user,
                        "port": h.port,
                    })
                })
                .collect();
            Ok(json!({ "hosts": list }))
        }

        Request::RecentItems { action } => match action.as_str() {
            "list" => {
                let recent = tako_core::recent::RecentList::load();
                let entries: Vec<Value> = recent
                    .entries
                    .iter()
                    .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                    .collect();
                Ok(json!({ "entries": entries }))
            }
            "clear" => {
                let mut recent = tako_core::recent::RecentList::load();
                recent.clear();
                recent.save();
                Ok(json!({ "cleared": true }))
            }
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（list / clear のいずれか）"
            ))),
        },

        Request::TaskCheckpoint {
            action,
            task_id,
            pane,
            issue,
            branch,
            phase,
            last_commit,
            agent,
            model,
            prompt_head,
            suspended_reason,
            project,
            cwd,
            resume_pane,
            tab,
            resume_model,
            caller_role,
        } => match action.as_str() {
            "checkpoint" => crate::task_checkpoints::checkpoint_payload(
                task_id.as_deref(),
                pane,
                issue,
                branch.as_deref(),
                phase.as_deref(),
                last_commit.as_deref(),
                agent.as_deref(),
                model.as_deref(),
                prompt_head.as_deref(),
                suspended_reason.as_deref(),
                project.as_deref(),
                cwd.as_deref(),
            )
            .map_err(DispatchError::Operation),
            "list" => crate::task_checkpoints::list_payload(phase.as_deref())
                .map_err(DispatchError::Operation),
            "update" => {
                let tid = task_id.ok_or_else(|| {
                    DispatchError::InvalidParams("update には task_id が必要".into())
                })?;
                let ph = phase.ok_or_else(|| {
                    DispatchError::InvalidParams("update には phase が必要".into())
                })?;
                crate::task_checkpoints::update_phase_payload(
                    &tid,
                    &ph,
                    suspended_reason.as_deref(),
                )
                .map_err(DispatchError::Operation)
            }
            "resume" => {
                let tid = task_id.ok_or_else(|| {
                    DispatchError::InvalidParams("resume には task_id が必要".into())
                })?;
                dispatch_task_resume(
                    host,
                    origin,
                    &tid,
                    resume_pane,
                    tab,
                    resume_model.as_deref(),
                    caller_role.as_deref(),
                )
            }
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（checkpoint / list / update / resume のいずれか）"
            ))),
        },

        Request::TaskGate {
            action,
            task_id,
            criteria_json,
            results_json,
            cwd,
            sync_checkpoint,
        } => match action.as_str() {
            "set" => {
                let tid = task_id.ok_or_else(|| {
                    DispatchError::InvalidParams("set には task_id が必要".into())
                })?;
                let cj = criteria_json.ok_or_else(|| {
                    DispatchError::InvalidParams("set には criteria_json が必要".into())
                })?;
                crate::acceptance_gates::set_gate_payload(&tid, &cj, cwd.as_deref())
                    .map_err(DispatchError::Operation)
            }
            "show" => {
                let tid = task_id.ok_or_else(|| {
                    DispatchError::InvalidParams("show には task_id が必要".into())
                })?;
                crate::acceptance_gates::show_gate_payload(&tid).map_err(DispatchError::Operation)
            }
            "record_results" => {
                let tid = task_id.ok_or_else(|| {
                    DispatchError::InvalidParams("record_results には task_id が必要".into())
                })?;
                let rj = results_json.ok_or_else(|| {
                    DispatchError::InvalidParams("record_results には results_json が必要".into())
                })?;
                crate::acceptance_gates::record_results_payload(
                    &tid,
                    &rj,
                    sync_checkpoint.unwrap_or(false),
                )
                .map_err(DispatchError::Operation)
            }
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（set / show / record_results のいずれか）"
            ))),
        },

        Request::RunInteractive {
            pane,
            tab,
            command,
            input_hint,
            direction,
            ratio,
            auto_close,
        } => {
            let ac = auto_close.as_deref().unwrap_or("success");
            if !matches!(ac, "success" | "always" | "never") {
                return Err(DispatchError::InvalidParams(format!(
                    "auto_close は success / always / never のいずれか（指定: {ac:?}）"
                )));
            }

            let (tab_id, target) = if let Some(tab_raw) = tab {
                let tid = find_tab(host.workspace(), tab_raw)?;
                let focused = host
                    .workspace()
                    .get_tab(tid)
                    .expect("find_tab で存在確認済み")
                    .tree()
                    .focused();
                (tid, focused)
            } else {
                resolve_pane(host.workspace(), pane)?
            };

            let cwd = host
                .session(target)
                .and_then(|s| s.cwd())
                .filter(|p| p.is_dir())
                .map(|p| p.to_path_buf());

            let new_id = spawn_command_pane(
                host,
                origin,
                tab_id,
                target,
                direction.unwrap_or(Direction::Right),
                ratio.unwrap_or(0.3),
                cwd,
                &command,
                ac,
                true, // focus
            )?;

            // タイトルとメタデータを設定
            let hint = input_hint.as_deref().unwrap_or(&command);
            if let Some(p) = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .and_then(|t| t.tree_mut().get_mut(new_id))
            {
                p.set_title(Some(format!("(!) {hint}")));
            }

            Ok(json!({
                "pane": new_id.as_u64(),
                "status": "running",
                "auto_close": ac,
            }))
        }

        Request::RunInteractiveStatus { pane, no_wait: _ } => {
            let (tab_id, target) = resolve_pane(host.workspace(), Some(pane))?;

            // ペインの画面からマーカーを探す
            let lines = host
                .session(target)
                .map(|s| s.visible_lines())
                .unwrap_or_default();

            let exit_code = find_exit_marker(&lines);

            let meta = host
                .workspace()
                .get_tab(tab_id)
                .and_then(|t| t.tree().get(target))
                .and_then(|p| p.interactive_meta())
                .cloned();

            match exit_code {
                Some(code) => {
                    let should_close = match meta.as_ref().map(|(ac, _)| ac.as_str()) {
                        Some("always") => true,
                        Some("success") => code == 0,
                        _ => false,
                    };
                    let cmd = meta.map(|(_, c)| c).unwrap_or_default();

                    if should_close {
                        let _ = tree_mut(host.workspace_mut(), tab_id).close(target);
                        host.detach_session(target);
                    }

                    Ok(json!({
                        "pane": pane,
                        "status": "exited",
                        "exit_code": code,
                        "command": cmd,
                        "closed": should_close,
                    }))
                }
                None => {
                    let cmd = meta.map(|(_, c)| c).unwrap_or_default();
                    Ok(json!({
                        "pane": pane,
                        "status": "running",
                        "command": cmd,
                    }))
                }
            }
        }

        // --- Code Runner (FR-3.18, #453) ---
        Request::Run {
            path,
            pane,
            tab,
            profile,
            command: cmd_override,
            direction,
            ratio,
            auto_close,
            focus,
        } => {
            let ac = auto_close.as_deref().unwrap_or("never");
            if !matches!(ac, "success" | "always" | "never") {
                return Err(DispatchError::InvalidParams(format!(
                    "auto_close は success / always / never のいずれか（指定: {ac:?}）"
                )));
            }

            let (tab_id, target) = if let Some(tab_raw) = tab {
                let tid = find_tab(host.workspace(), tab_raw)?;
                let focused = host
                    .workspace()
                    .get_tab(tid)
                    .expect("find_tab で存在確認済み")
                    .tree()
                    .focused();
                (tid, focused)
            } else {
                resolve_pane(host.workspace(), pane)?
            };

            // パス解決（OpenFile と同一: 相対パスは対象ペインの cwd 基準 + canonicalize）
            let mut resolved = PathBuf::from(&path);
            if resolved.is_relative() {
                if let Some(cwd) = host.session(target).and_then(|s| s.cwd()) {
                    resolved = cwd.join(resolved);
                }
            }
            let resolved = resolved.canonicalize().map_err(|e| {
                DispatchError::Operation(format!("ファイルを開けない（{path}: {e}）"))
            })?;
            if !resolved.is_file() {
                return Err(DispatchError::Operation(format!(
                    "ファイルではない: {}",
                    resolved.display()
                )));
            }

            // ファイル先頭 16 KiB を読む
            let head = read_file_head(&resolved)?;

            // 拡張子既定のマージ
            let settings = crate::settings::load();
            let ext_defaults = tako_core::merged_defaults(&settings.runner_defaults);

            // 解決
            let resolution = tako_core::resolve(
                &resolved,
                &head,
                &ext_defaults,
                profile.as_deref(),
                cmd_override.as_deref(),
            )
            .map_err(|e| DispatchError::Operation(e.to_string()))?;

            let plan = &resolution.plan;

            // cwd 存在検査
            let cwd = plan.cwd.clone();
            if !cwd.is_dir() {
                return Err(DispatchError::Operation(format!(
                    "cwd が存在しない: {}",
                    cwd.display()
                )));
            }

            // シェル指定時はコマンドを包む（包み方は境界 B1 が持つ。
            // 引用符の書き方が POSIX シェルと PowerShell で違うため）
            let final_command = if let Some(shell) = &plan.shell {
                tako_core::platform::shell::declared_shell_command(shell, &plan.command)
            } else {
                plan.command.clone()
            };

            let new_id = spawn_command_pane(
                host,
                origin,
                tab_id,
                target,
                direction.unwrap_or(Direction::Down),
                ratio.unwrap_or(0.3),
                Some(cwd.clone()),
                &final_command,
                ac,
                focus.unwrap_or(false),
            )?;

            // タイトル設定
            let file_base = resolved
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let title = if plan.profile == "default" {
                format!("(>) {file_base}")
            } else {
                format!("(>) {file_base} [{}]", plan.profile)
            };
            if let Some(p) = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .and_then(|t| t.tree_mut().get_mut(new_id))
            {
                p.set_title(Some(title));
            }

            Ok(json!({
                "pane": new_id.as_u64(),
                "path": resolved.display().to_string(),
                "profile": plan.profile,
                "command": plan.command,
                "cwd": cwd.display().to_string(),
                "auto_close": ac,
            }))
        }

        Request::RunResolve { path, pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;

            let mut resolved = PathBuf::from(&path);
            if resolved.is_relative() {
                if let Some(cwd) = host.session(target).and_then(|s| s.cwd()) {
                    resolved = cwd.join(resolved);
                }
            }
            let resolved = resolved.canonicalize().map_err(|e| {
                DispatchError::Operation(format!("ファイルを開けない（{path}: {e}）"))
            })?;
            if !resolved.is_file() {
                return Err(DispatchError::Operation(format!(
                    "ファイルではない: {}",
                    resolved.display()
                )));
            }

            let head = read_file_head(&resolved)?;
            let settings = crate::settings::load();
            let ext_defaults = tako_core::merged_defaults(&settings.runner_defaults);

            let resolution = tako_core::resolve(&resolved, &head, &ext_defaults, None, None)
                .map_err(|e| DispatchError::Operation(e.to_string()))?;

            let profiles: Vec<Value> = resolution
                .all_profiles
                .iter()
                .map(|p| {
                    json!({
                        "profile": p.profile,
                        "command": p.command,
                        "cwd": p.cwd.display().to_string(),
                        "source": match p.source {
                            tako_core::RunSource::Declaration => "declaration",
                            tako_core::RunSource::ExtensionDefault => "extension_default",
                            tako_core::RunSource::Override => "override",
                        },
                    })
                })
                .collect();

            Ok(json!({
                "path": resolved.display().to_string(),
                "profiles": profiles,
                "warnings": resolution.warnings,
                "default_profile": resolution.plan.profile,
            }))
        }

        Request::RunnerDefaults {
            ext,
            command,
            remove,
        } => {
            let mut settings = crate::settings::load();
            let builtins: std::collections::BTreeMap<String, String> =
                tako_core::builtin_defaults()
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();

            if let Some(ext_key) = ext {
                let ext_lower = ext_key.to_ascii_lowercase();
                if remove {
                    settings.runner_defaults.remove(&ext_lower);
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(e.to_string()))?;
                    let effective = builtins.get(&ext_lower).cloned();
                    Ok(json!({
                        "ext": ext_lower,
                        "removed": true,
                        "effective_command": effective,
                    }))
                } else if let Some(cmd) = command {
                    settings
                        .runner_defaults
                        .insert(ext_lower.clone(), cmd.clone());
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(e.to_string()))?;
                    Ok(json!({
                        "ext": ext_lower,
                        "command": cmd,
                        "source": "user",
                    }))
                } else {
                    // 単一拡張子の情報
                    let user_cmd = settings.runner_defaults.get(&ext_lower);
                    let builtin_cmd = builtins.get(&ext_lower);
                    let effective = user_cmd.or(builtin_cmd);
                    Ok(json!({
                        "ext": ext_lower,
                        "command": effective,
                        "source": if user_cmd.is_some() { "user" } else if builtin_cmd.is_some() { "builtin" } else { "none" },
                        "user_override": user_cmd,
                        "builtin": builtin_cmd,
                    }))
                }
            } else {
                // 全一覧
                let merged = tako_core::merged_defaults(&settings.runner_defaults);
                let entries: Vec<Value> = merged
                    .iter()
                    .map(|(k, v)| {
                        let source = if settings.runner_defaults.contains_key(k) {
                            "user"
                        } else {
                            "builtin"
                        };
                        json!({ "ext": k, "command": v, "source": source })
                    })
                    .collect();
                Ok(json!({ "defaults": entries }))
            }
        }

        Request::Settings { action, tab } => {
            let action = action.as_deref().unwrap_or("open");
            match action {
                "open" => {
                    host.open_settings_window(tab.as_deref());
                    Ok(json!({ "ok": true }))
                }
                "status" => {
                    let is_open = host.settings_window_open();
                    Ok(json!({ "open": is_open }))
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（open / status のいずれか）"
                ))),
            }
        }

        Request::StaleBinary { action, pane } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => dispatch_stale_binary_status(host, origin, pane),
                "restart" => dispatch_stale_binary_restart(host, origin, pane),
                "dismiss" => dispatch_stale_binary_dismiss(host, origin, pane),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / restart / dismiss のいずれか）"
                ))),
            }
        }
    }
}

/// RunInteractive / Run 共通: 分割 → コマンド付きセッション起動 → exit マーカーラップ
#[allow(clippy::too_many_arguments)]
fn spawn_command_pane(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    tab_id: TabId,
    target: PaneId,
    direction: Direction,
    ratio: f32,
    cwd: Option<PathBuf>,
    command: &str,
    auto_close: &str,
    focus: bool,
) -> Result<PaneId, DispatchError> {
    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();

    tree_mut(host.workspace_mut(), tab_id)
        .split_with_ratio(target, direction.to_core(), ratio, new_pane)
        .map_err(op_err)?;

    // 実行ペインの組み立てはプラットフォーム境界 B1 に委ねる（POSIX は /bin/sh -c、
    // Windows は PowerShell）。ここに `#[cfg]` は書かない
    host.attach_session(
        new_id,
        SpawnOptions {
            command: Some(tako_core::platform::shell::run_pane_command(
                command,
                EXIT_MARKER_PREFIX,
            )),
            cwd,
            env: Vec::new(),
        },
    );

    if focus {
        let _ = tree_mut(host.workspace_mut(), tab_id).focus(new_id);
    }

    // interactive_meta を設定（RunInteractiveStatus で exit code 回収 + auto_close に使う）
    if let Some(p) = host
        .workspace_mut()
        .get_tab_mut(tab_id)
        .and_then(|t| t.tree_mut().get_mut(new_id))
    {
        p.set_interactive_meta(auto_close.to_string(), command.to_string());
    }

    Ok(new_id)
}

/// ファイル先頭 16 KiB を読む（Code Runner の宣言スキャン用）
fn read_file_head(path: &Path) -> Result<String, DispatchError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| {
        DispatchError::Operation(format!("ファイルを読めない（{}: {e}）", path.display()))
    })?;
    let mut buf = vec![0u8; 16 * 1024];
    let n = file.read(&mut buf).map_err(|e| {
        DispatchError::Operation(format!(
            "ファイルの読み取りに失敗（{}: {e}）",
            path.display()
        ))
    })?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// ペインログ設定の状態ペイロード（status / set 共通）
fn pane_log_status_json(config: &tako_core::pane_log::PaneLogConfig) -> Value {
    json!({
        "enabled": config.enabled,
        "max_mb": config.max_bytes_per_pane / (1024 * 1024),
        "total_max_mb": config.max_total_bytes / (1024 * 1024),
        "dir": tako_core::pane_log::log_dir(),
    })
}

/// `tako logs read` の対象ログファイルを解決して末尾を返す。
/// 対象解決: `session_id`（カタログの log_file → 記録ペインの最新ファイル）→
/// `pane`（ライブペインの現行ファイル → クローズ済みでもファイル名から検索）
fn dispatch_logs_read(
    host: &dyn ControlHost,
    pane: Option<u64>,
    session_id: Option<&str>,
    lines: Option<usize>,
) -> Result<Value, DispatchError> {
    let dir = tako_core::pane_log::log_dir()
        .ok_or_else(|| DispatchError::Operation("データディレクトリを解決できない".into()))?;
    let (path, resolved_pane) = if let Some(sid) = session_id {
        let catalog = crate::sessions::SessionCatalog::load().map_err(DispatchError::Operation)?;
        let (_, entry) = catalog.resolve_id(sid).map_err(DispatchError::Operation)?;
        let from_entry = entry
            .log_file
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file());
        let path = from_entry
            .or_else(|| {
                entry
                    .pane
                    .and_then(|p| tako_core::pane_log::latest_for_pane(&dir, p))
            })
            .ok_or_else(|| {
                DispatchError::Operation(format!(
                    "セッション '{}' の端末ログが見つからない（ログ保存が OFF だったか、上限で削除済み）",
                    crate::sessions::short_id(sid)
                ))
            })?;
        (path, entry.pane)
    } else if let Some(p) = pane {
        // ライブペインなら現行ファイル、無ければファイル名から検索（クローズ済み対応）
        let path = host
            .pane_log_file(PaneId::from_raw(p))
            .filter(|f| f.is_file())
            .or_else(|| tako_core::pane_log::latest_for_pane(&dir, p))
            .ok_or_else(|| {
                DispatchError::Operation(format!(
                    "ペイン {p} のログが見つからない（ログ保存が OFF だったか、上限で削除済み）"
                ))
            })?;
        (path, Some(p))
    } else {
        return Err(DispatchError::InvalidParams(
            "read には pane または session_id が必要".into(),
        ));
    };
    let max_lines = lines.unwrap_or(200).clamp(1, 100_000);
    let content =
        tako_core::pane_log::read_tail(&path, max_lines).map_err(DispatchError::Operation)?;
    Ok(json!({
        "path": path,
        "pane": resolved_pane,
        "lines": max_lines,
        "content": content,
    }))
}

/// セッションカタログからの会話復元（Issue #112 A）。
/// 該当エントリの cwd でシェルペインを分割起動し、`claude --resume <session_id>` を
/// 注入する（#30 の復元経路と同方式。Claude 終了後もシェルが残る）
fn dispatch_sessions_resume(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    id_prefix: &str,
    pane: Option<u64>,
    tab: Option<u64>,
    direction: Option<Direction>,
) -> Result<Value, DispatchError> {
    let catalog = crate::sessions::SessionCatalog::load().map_err(DispatchError::Operation)?;
    let (session_id, entry) = catalog
        .resolve_id(id_prefix)
        .map_err(DispatchError::Operation)?;
    let session_id = session_id.clone();
    let entry = entry.clone();

    // エージェント種別の検証を先に行う（codex / agy は resume 非対応の明示メッセージ）
    let resume_cmd =
        crate::sessions::resume_command(&session_id, &entry).map_err(DispatchError::Operation)?;
    // 会話ログ（claude transcript）の実在確認。無ければ resume は成立しない
    if crate::transcript::find_transcript(&session_id).is_none() {
        return Err(DispatchError::Operation(format!(
            "セッション {} の会話ログ（~/.claude/projects/ の transcript）が見つからない。\
             claude 側で削除された可能性がある",
            crate::sessions::short_id(&session_id)
        )));
    }

    // 分割元の解決: pane > tab > 呼び出し元 > アクティブタブ。
    // 消失復旧が主用途のため、tako 外の CLI（TAKO_PANE_ID 無し）からも
    // アクティブタブへのフォールバックで実行できるようにする
    let (tab_id, target) = if let Some(tab_raw) = tab {
        let tab_id = find_tab(host.workspace(), tab_raw)?;
        let focused = host
            .workspace()
            .get_tab(tab_id)
            .expect("find_tab で存在確認済み")
            .tree()
            .focused();
        (tab_id, focused)
    } else {
        match resolve_pane(host.workspace(), pane) {
            Ok(resolved) => resolved,
            Err(_) if pane.is_none() => {
                let active = host.workspace().active_tab();
                (active.id(), active.tree().focused())
            }
            Err(e) => return Err(e),
        }
    };

    let cwd = entry
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir());
    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();
    tree_mut(host.workspace_mut(), tab_id)
        .split_with_ratio(
            target,
            direction.unwrap_or(Direction::Right).to_core(),
            0.5,
            new_pane,
        )
        .map_err(op_err)?;
    // フォーカスは分割元を維持（ユーザーの入力を奪わない。spawn と同方針）
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(target);
    let options = SpawnOptions {
        command: None,
        cwd: cwd.clone(),
        env: Vec::new(),
    };
    host.attach_session(new_id, options);
    // シェル起動後に resume コマンドを注入する（送達確認つき。#640）
    host.queue_command_flow(new_id, resume_cmd.clone());

    // タイトル・role をカタログのメタから復元する
    let title = match (&entry.project, &entry.label) {
        (Some(p), Some(l)) => Some(format!("{p}: {l}")),
        (_, Some(l)) => Some(l.clone()),
        (Some(p), None) => Some(format!("{p}-resumed")),
        _ => None,
    };
    let role = match entry.kind.as_str() {
        "worker" => {
            let project = entry.project.as_deref().unwrap_or("resumed");
            Some(match entry.label.as_deref() {
                Some(l) => format!("orchestrator-worker:{project}:{l}"),
                None => format!("orchestrator-worker:{project}"),
            })
        }
        "master" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("orchestrator-master:{p}"),
            _ => "orchestrator-master".into(),
        }),
        "solo" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("solo:{p}"),
            _ => "solo".into(),
        }),
        _ => None,
    };
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    if title.is_some() {
        pane_obj.set_title(title.clone());
    }
    pane_obj.set_role(role);

    Ok(json!({
        "pane": new_id.as_u64(),
        "session_id": session_id,
        "cwd": cwd,
        "command": resume_cmd,
        "title": title,
    }))
}

// --- オーケストレーター dispatch ---

fn dispatch_orchestrator_projects(
    action: &str,
    key: Option<String>,
    cwd: Option<String>,
    description: Option<String>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    match action {
        "list" => {
            let config = orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
            let projects: Vec<Value> = config
                .list_resolved()
                .into_iter()
                .map(|p| json!({ "key": p.key, "cwd": p.cwd, "description": p.description }))
                .collect();
            Ok(json!({ "projects": projects }))
        }
        "add" => {
            let key = key.ok_or(DispatchError::InvalidParams("key を指定する".into()))?;
            let cwd = cwd.ok_or(DispatchError::InvalidParams("cwd を指定する".into()))?;
            orchestrator::ensure_defaults().map_err(DispatchError::Operation)?;
            // ロック付き read-modify-write（#169: 並行 add で他エントリを消さない）
            orchestrator::ProjectsConfig::mutate(|config| {
                config.add(key.clone(), cwd.clone(), description);
            })
            .map_err(DispatchError::Operation)?;
            Ok(json!({ "added": key, "cwd": cwd }))
        }
        "remove" => {
            let key = key.ok_or(DispatchError::InvalidParams("key を指定する".into()))?;
            let removed = orchestrator::ProjectsConfig::mutate(|config| config.remove(&key))
                .map_err(DispatchError::Operation)?;
            if !removed {
                return Err(DispatchError::Operation(format!(
                    "プロジェクト '{key}' が見つからない"
                )));
            }
            Ok(json!({ "removed": key }))
        }
        _ => Err(DispatchError::InvalidParams(format!(
            "action が不正: {action}（list / add / remove）"
        ))),
    }
}

/// OrchestratorProfiles のパラメータ（Request と 1:1）。
/// ファイル直読みで tako-core の状態に依存しないため、CLI からも直接呼べるよう公開する
#[derive(Default)]
pub struct ProfilesParams {
    pub action: String,
    pub name: Option<String>,
    /// master のエージェント種別（claude / codex。agy は master 非対応。#127）
    pub master_agent: Option<String>,
    pub clear_master_agent: bool,
    pub model: Option<String>,
    pub worker_model: Option<String>,
    pub effort: Option<String>,
    pub worker_effort: Option<String>,
    pub clear_model: bool,
    pub clear_worker_model: bool,
    /// worker の既定エージェント種別（claude / codex / agy。#120）
    pub worker_agent: Option<String>,
    pub clear_worker_agent: bool,
    /// `worker_agents.<agent>` を編集する対象エージェント名
    pub agent: Option<String>,
    pub agent_model: Option<String>,
    pub clear_agent_model: bool,
    pub agent_effort: Option<String>,
    pub clear_agent_effort: bool,
    pub agent_skip_permissions: Option<bool>,
    pub agent_args: Option<Vec<String>>,
    /// worker_model_policy（inherit / delegate / fixed）
    pub worker_model_policy: Option<String>,
    /// タブ名の命名規則
    pub tab_naming_convention: Option<String>,
    /// env を設定する（key=value 形式。Issue #500）
    pub env_set: Option<Vec<String>>,
    /// env からキーを削除する（Issue #500）
    pub env_unset: Option<Vec<String>>,
    /// master の既定アカウント名（Issue #504）
    pub master_account: Option<String>,
    pub clear_master_account: bool,
    /// worker の既定アカウント名（Issue #504）
    pub worker_account: Option<String>,
    pub clear_worker_account: bool,
}

/// プロファイルを JSON 化する（list / show / set の共通形）。
/// model が null のときは claude CLI の既定モデルで起動することを表す
fn profile_to_json(name: &str, profile: &crate::orchestrator::Profile) -> Value {
    use crate::orchestrator;
    let mut v = json!({
        "name": name,
        "model": profile.model,
        "effort": profile.effort,
        "worker_model_policy": profile.worker_model_policy,
        "worker_model": profile.worker_model,
        "worker_effort": profile.worker_effort,
        "resolved_worker_model": profile.resolve_worker_model(),
        "resolved_worker_effort": profile.resolve_worker_effort(),
        "path": orchestrator::profiles_dir()
            .map(|d| d.join(format!("{name}.yaml")).display().to_string()),
    });
    // worker エージェント設定（#120）は使用時のみ出力（既存出力形の互換維持）
    if profile.worker_agent.is_some() || !profile.worker_agents.is_empty() {
        v["worker_agent"] = json!(profile.worker_agent.as_deref().unwrap_or("claude"));
        v["worker_agents"] = serde_json::to_value(&profile.worker_agents).unwrap_or_default();
    }
    // master エージェント設定（#127）も使用時のみ出力
    if profile.master_agent.is_some() {
        v["master_agent"] = json!(profile.master_agent);
    }
    if profile.tab_naming_convention.is_some() {
        v["tab_naming_convention"] = json!(profile.tab_naming_convention);
    }
    // env はキー名のみ表示（値はマスク。Issue #500）
    if !profile.env.is_empty() {
        let masked: serde_json::Map<String, Value> = profile
            .env
            .keys()
            .map(|k| (k.clone(), json!("***")))
            .collect();
        v["env"] = Value::Object(masked);
        // CLAUDE_CONFIG_DIR が設定されている場合、config dir パスだけは表示する
        // （アカウントの判別に必要。値自体は秘密ではない）
        if let Some(config_dir) = profile.env.get("CLAUDE_CONFIG_DIR") {
            v["config_dir"] = json!(orchestrator::expand_tilde(config_dir));
        }
    }
    if profile.cwd.is_some() {
        v["cwd"] = json!(profile.cwd.as_deref().map(orchestrator::expand_tilde));
    }
    if profile.projects.is_some() {
        v["projects"] = json!(profile.projects);
    }
    // アカウント設定（#504）は使用時のみ出力
    if profile.master_account.is_some() {
        v["master_account"] = json!(profile.master_account);
    }
    if profile.worker_account.is_some() {
        v["worker_account"] = json!(profile.worker_account);
    }
    v
}

/// プロファイル管理（list / show / set）。ファイル直読みなので tako-core の状態に依存しない
pub fn dispatch_orchestrator_profiles(params: ProfilesParams) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    match params.action.as_str() {
        "list" => {
            let names = orchestrator::list_profiles().map_err(DispatchError::Operation)?;
            let profiles: Vec<Value> = names
                .iter()
                .map(|n| {
                    let p = orchestrator::Profile::load(n).unwrap_or_default();
                    profile_to_json(n, &p)
                })
                .collect();
            Ok(json!({ "profiles": profiles }))
        }
        "show" => {
            let name = params.name.as_deref().unwrap_or("default");
            let profile = match orchestrator::Profile::load(name) {
                Ok(p) => p,
                Err(_) if name == "default" => orchestrator::Profile::default(),
                Err(e) => return Err(DispatchError::Operation(e)),
            };
            Ok(profile_to_json(name, &profile))
        }
        "set" => {
            let name = params
                .name
                .ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            if params.model.is_some() && params.clear_model {
                return Err(DispatchError::InvalidParams(
                    "model と clear_model は同時に指定できない".into(),
                ));
            }
            if params.worker_model.is_some() && params.clear_worker_model {
                return Err(DispatchError::InvalidParams(
                    "worker_model と clear_worker_model は同時に指定できない".into(),
                ));
            }
            if params.worker_agent.is_some() && params.clear_worker_agent {
                return Err(DispatchError::InvalidParams(
                    "worker_agent と clear_worker_agent は同時に指定できない".into(),
                ));
            }
            if params.master_agent.is_some() && params.clear_master_agent {
                return Err(DispatchError::InvalidParams(
                    "master_agent と clear_master_agent は同時に指定できない".into(),
                ));
            }
            // agent_* 系の指定には対象エージェント名（agent）が必須
            let has_agent_edit = params.agent_model.is_some()
                || params.clear_agent_model
                || params.agent_effort.is_some()
                || params.clear_agent_effort
                || params.agent_skip_permissions.is_some()
                || params.agent_args.is_some();
            if has_agent_edit && params.agent.is_none() {
                return Err(DispatchError::InvalidParams(
                    "agent_* 系の設定には agent（対象エージェント名）を指定する".into(),
                ));
            }
            // エージェント名は設定時点で検証する（spawn / master 起動時の不意のエラーを防ぐ）
            if let Some(a) = params.worker_agent.as_deref() {
                orchestrator::WorkerAgent::parse(a).map_err(DispatchError::InvalidParams)?;
            }
            if let Some(a) = params.agent.as_deref() {
                orchestrator::WorkerAgent::parse(a).map_err(DispatchError::InvalidParams)?;
            }
            // master は claude / codex のみ（agy は非対応。#127）
            if let Some(a) = params.master_agent.as_deref() {
                orchestrator::validate_master_agent(a).map_err(DispatchError::InvalidParams)?;
            }
            // worker_model_policy は mutate 閉包内から early return できないため事前に解析
            let policy = match params.worker_model_policy.as_deref() {
                Some("inherit") => Some(orchestrator::WorkerModelPolicy::Inherit),
                Some("delegate") => Some(orchestrator::WorkerModelPolicy::Delegate),
                Some("fixed") => Some(orchestrator::WorkerModelPolicy::Fixed),
                Some(p) => {
                    return Err(DispatchError::InvalidParams(format!(
                        "worker_model_policy が不正: '{p}'（inherit / delegate / fixed）"
                    )));
                }
                None => None,
            };
            // env_set の形式と内部変数チェックを事前検証（クロージャ内から
            // DispatchError を返せないため。Issue #500）
            if let Some(ref env_set) = params.env_set {
                for entry in env_set {
                    match entry.split_once('=') {
                        None => {
                            return Err(DispatchError::InvalidParams(format!(
                                "env の形式が不正（KEY=VALUE が必要）: {entry}"
                            )));
                        }
                        Some((k, _)) => {
                            // 一時的に Profile で内部変数チェック
                            let mut tmp = orchestrator::Profile::default();
                            tmp.env.insert(k.to_string(), String::new());
                            if let Err(e) = tmp.validate_env() {
                                return Err(DispatchError::Operation(e));
                            }
                        }
                    }
                }
            }
            let env_set_clone = params.env_set.clone();
            let env_unset_clone = params.env_unset.clone();
            let master_account_clone = params.master_account.clone();
            let worker_account_clone = params.worker_account.clone();
            let clear_master_account = params.clear_master_account;
            let clear_worker_account = params.clear_worker_account;
            // ロック付き read-modify-write（#169）。パースできない既存プロファイルを
            // default に丸めて上書き保存すると設定が消えるため、Err で中断する
            let (path, profile) = orchestrator::Profile::mutate_named(&name, |profile| {
                if let Some(a) = params.master_agent {
                    profile.master_agent = Some(a);
                } else if params.clear_master_agent {
                    profile.master_agent = None;
                }
                if let Some(m) = params.model {
                    profile.model = Some(m);
                } else if params.clear_model {
                    profile.model = None;
                }
                if let Some(m) = params.worker_model {
                    profile.worker_model = Some(m);
                } else if params.clear_worker_model {
                    profile.worker_model = None;
                }
                if let Some(e) = params.effort {
                    profile.effort = e;
                }
                if let Some(e) = params.worker_effort {
                    profile.worker_effort = Some(e);
                }
                if let Some(a) = params.worker_agent {
                    profile.worker_agent = Some(a);
                } else if params.clear_worker_agent {
                    profile.worker_agent = None;
                }
                if let Some(policy) = policy {
                    profile.worker_model_policy = policy;
                }
                if let Some(conv) = params.tab_naming_convention {
                    if conv.is_empty() {
                        profile.tab_naming_convention = None;
                    } else {
                        profile.tab_naming_convention = Some(conv);
                    }
                }
                if let Some(agent_name) = params.agent {
                    let cfg = profile.worker_agents.entry(agent_name).or_default();
                    if let Some(m) = params.agent_model {
                        cfg.model = Some(m);
                    } else if params.clear_agent_model {
                        cfg.model = None;
                    }
                    if let Some(e) = params.agent_effort {
                        cfg.effort = Some(e);
                    } else if params.clear_agent_effort {
                        cfg.effort = None;
                    }
                    if let Some(s) = params.agent_skip_permissions {
                        cfg.skip_permissions = s;
                    }
                    if let Some(a) = params.agent_args {
                        cfg.args = a;
                    }
                }
                // env の設定・削除（Issue #500）
                if let Some(ref env_set) = env_set_clone {
                    for entry in env_set {
                        if let Some((k, v)) = entry.split_once('=') {
                            profile.env.insert(k.to_string(), v.to_string());
                        }
                        // 不正形式は事前検証済み
                    }
                }
                if let Some(ref env_unset) = env_unset_clone {
                    for key in env_unset {
                        profile.env.remove(key);
                    }
                }
                // アカウントの設定・解除（#504）
                if let Some(a) = master_account_clone.as_deref() {
                    if a.is_empty() {
                        profile.master_account = None;
                    } else {
                        profile.master_account = Some(a.to_string());
                    }
                } else if clear_master_account {
                    profile.master_account = None;
                }
                if let Some(a) = worker_account_clone.as_deref() {
                    if a.is_empty() {
                        profile.worker_account = None;
                    } else {
                        profile.worker_account = Some(a.to_string());
                    }
                } else if clear_worker_account {
                    profile.worker_account = None;
                }
                // 既定値のみになったエントリは掃除する（YAML を汚さない）
                profile
                    .worker_agents
                    .retain(|_, c| *c != orchestrator::AgentWorkerConfig::default());
                profile.clone()
            })
            .map_err(DispatchError::Operation)?;
            let mut result = profile_to_json(&name, &profile);
            result["path"] = json!(path.display().to_string());
            // [1m] は Max / API プラン限定 → 明示 opt-in は許容しつつ警告を返す
            // （inherit で master と同一モデルの場合は master 分のみ警告。
            //  claude 以外の master の model は claude 表記でないため対象外。#127）
            let warnings: Vec<String> = [
                profile
                    .model
                    .as_deref()
                    .filter(|_| profile.master_agent_is_claude())
                    .and_then(|m| orchestrator::one_m_model_warning(m, "master")),
                profile
                    .resolve_worker_model()
                    .filter(|m| Some(*m) != profile.model.as_deref())
                    .and_then(|m| orchestrator::one_m_model_warning(m, "worker")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !warnings.is_empty() {
                result["warnings"] = json!(warnings);
            }
            Ok(result)
        }
        other => Err(DispatchError::InvalidParams(format!(
            "action が不正: {other}（list / show / set）"
        ))),
    }
}

/// 「Enter 単独送信」の意図判定（Issue #95）: text が空 / 改行のみなら、意図は
/// テキスト入力ではなく Enter キー（入力欄に残ったテキストの送信代行等）。
/// `text:"" + newline:true`（Enter 代行）と `text:"\n"`（改行そのもの）の両方を拾う。
/// `text:"" + newline:false` は「何も送らない」なので対象外
fn send_is_enter_only(text: &str, newline: bool) -> bool {
    text.chars().all(|c| c == '\n' || c == '\r') && (newline || !text.is_empty())
}

/// 実行ペインが終了コードを知らせるマーカー。
/// **出力側（`platform::shell::run_pane_command`）と読み取り側（`find_exit_marker`）の
/// 唯一の接点**なので、片方だけ変えられないよう定数を共有する
const EXIT_MARKER_PREFIX: &str = "__TAKO_EXIT=";

/// `__TAKO_EXIT=<code>` マーカーを画面行から検索する。
/// 行頭以外の位置（read プロンプトと同一行等）にも対応する（#325）
fn find_exit_marker(lines: &[String]) -> Option<i32> {
    lines.iter().rev().find_map(|line| {
        line.find(EXIT_MARKER_PREFIX).and_then(|pos| {
            let after = &line[pos + EXIT_MARKER_PREFIX.len()..];
            after.trim().parse::<i32>().ok()
        })
    })
}

/// キーボード入力の意味論での改行正規化（Issue #95）: 端末の Enter キーは CR であり、
/// LF は claude TUI で「改行挿入」と解釈され送信にならない。PTY へ直接書く経路では
/// LF / CRLF を CR へ揃える（bracketed paste 経由の貼り付けは対象外）
fn normalize_newlines_for_keys(text: &str) -> String {
    text.replace("\r\n", "\r").replace('\n', "\r")
}

/// tmux セッションへの送達確認つき配送をバックグラウンドスレッドで実行する
/// （Issue #32）。`deliver_via_tmux` は内部で sleep するブロッキング関数のため、
/// UI スレッド上の dispatch から直接呼ばない。結果はログのみ（fire-and-forget）
///
/// **到達可否の判断は呼び出し側が `crate::reach` へ問う**。ここは tmux 固有の
/// 送達手順（capture → 貼り付け → 分離 Enter → 空検証）そのものであり、
/// 案 B-1（器だけの ConPTY セッションホスト）が入ったら同等の手順を
/// その実装向けに用意して `DetachedAccess` 側へ載せる（設計 §5.1）
fn spawn_tmux_delivery(session: String, text: String, wait_ready: bool) {
    std::thread::spawn(move || {
        let socket = tako_core::tmux_backend::socket_name();
        match crate::claude_tui::deliver_via_tmux(Some(&socket), &session, &text, wait_ready) {
            Ok(report) if !report.verified => {
                eprintln!("warning: tmux 経由のプロンプト送達を検証できない（session={session}）");
            }
            Err(e) => {
                eprintln!("warning: tmux 経由のプロンプト送達に失敗（session={session}）: {e}");
            }
            Ok(_) => {}
        }
    });
}

/// spawn レイアウト設定の取得・変更（Issue #165）。host 非依存（config.yaml の読み書きのみ）
/// のため pub にし、CLI `tako orchestrator layout` からもローカル呼び出しで共用する
/// アカウント操作のパラメータ（Request と 1:1。Issue #504 / #709）
pub struct AccountParams<'a> {
    pub action: &'a str,
    pub name: Option<&'a str>,
    pub config_dir: Option<&'a str>,
    /// `CLAUDE_CONFIG_DIR` を設定しない（既定の資格情報を使う。#512）
    pub inherit: bool,
    pub description: Option<&'a str>,
    pub default_model: Option<&'a str>,
    pub default_effort: Option<&'a str>,
    /// `use` の割り当て先: master
    pub master: bool,
    /// `use` の割り当て先: worker
    pub worker: bool,
    /// `use` の対象プロファイル（省略時 default）
    pub profile: Option<&'a str>,
    /// `login` でペインを生やす基準（省略時は呼び出し元ペイン）
    pub pane: Option<u64>,
    pub tab: Option<u64>,
}

/// アカウント 1 件の JSON 表現（一覧・詳細で同じ形にする）
fn account_view_json(view: &crate::orchestrator::accounts::AccountView) -> Value {
    json!({
        "name": view.name,
        "config_dir": view.config_dir,
        "inherit": view.inherit,
        "description": view.description,
        "default_model": view.default_model,
        "default_effort": view.default_effort,
        "status": view.status.as_str(),
        "email": view.status.email(),
        "error": match &view.status {
            crate::orchestrator::accounts::AccountStatus::Invalid { error } => Some(error.clone()),
            _ => None,
        },
        "master_of": view.master_of,
        "worker_of": view.worker_of,
    })
}

/// アカウントレジストリの操作（Issue #504 / #709）。
///
/// `login` 以外は accounts.yaml / profiles の読み書きだけで完結するので host 非依存。
/// CLI `tako account` と MCP `tako_orchestrator_accounts` が同じ関数を呼ぶ
/// （表示・検証を二重実装しない。#83 の教訓）
pub fn dispatch_orchestrator_accounts(
    host: Option<&mut dyn ControlHost>,
    origin: PaneOrigin,
    params: AccountParams<'_>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::{self, accounts as acct};
    let AccountParams {
        action,
        name,
        config_dir,
        inherit,
        description,
        default_model,
        default_effort,
        master,
        worker,
        profile,
        pane,
        tab,
    } = params;

    match action {
        "list" => {
            let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
            let views = acct::collect_views(&config);
            Ok(json!({
                "accounts": views.iter().map(account_view_json).collect::<Vec<_>>(),
            }))
        }
        "show" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
            if !config.accounts.contains_key(name) {
                return Err(DispatchError::Operation(format!(
                    "アカウント '{name}' が accounts.yaml に見つからない"
                )));
            }
            let view = acct::collect_views(&config)
                .into_iter()
                .find(|v| v.name == name)
                .expect("直前に存在確認済み");
            Ok(account_view_json(&view))
        }
        "add" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            // #512: config_dir と inherit は排他。どちらも無いと解決できない
            if inherit && config_dir.is_some() {
                return Err(DispatchError::InvalidParams(
                    "config_dir と inherit は同時に指定できない（既定の資格情報を使うなら inherit のみ）"
                        .into(),
                ));
            }
            if !inherit && config_dir.is_none() {
                return Err(DispatchError::InvalidParams(
                    "config_dir を指定する（既定の資格情報を使うなら inherit: true）".into(),
                ));
            }
            // #512: 既定パスの明示は「別アカウント扱い」になる罠なので警告する
            let warning = config_dir
                .filter(|d| orchestrator::is_claude_default_config_dir(d))
                .map(|d| {
                    format!(
                        "警告: {d} は claude の既定 config ディレクトリです。\
                         明示指定すると既定ログインとは別の資格情報エントリになります。\
                         既定のログインを使いたい場合は inherit を指定してください（#512）"
                    )
                });
            let entry = orchestrator::AccountEntry {
                config_dir: config_dir.map(str::to_string),
                inherit,
                description: description.map(str::to_string),
                default_model: default_model.map(str::to_string),
                default_effort: default_effort.map(str::to_string),
            };
            orchestrator::AccountsConfig::mutate(|config| {
                config.accounts.insert(name.to_string(), entry);
            })
            .map_err(DispatchError::Operation)?;
            let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
            let view = acct::collect_views(&config)
                .into_iter()
                .find(|v| v.name == name)
                .ok_or_else(|| DispatchError::Operation("追加直後の読み直しに失敗".into()))?;
            let mut out = account_view_json(&view);
            if let (Some(obj), Some(w)) = (out.as_object_mut(), warning) {
                obj.insert("warning".into(), json!(w));
            }
            Ok(out)
        }
        "remove" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            // 割り当て済みのまま消すと master / worker の起動が未登録キーで失敗するので、
            // 消す前に使用箇所を数えて応答へ載せる（#709）
            let assignments = acct::load_profile_assignments();
            let (master_of, worker_of) = acct::assignments_for(name, &assignments);
            let removed = orchestrator::AccountsConfig::mutate(|config| {
                config.accounts.remove(name).is_some()
            })
            .map_err(DispatchError::Operation)?;
            if !removed {
                return Err(DispatchError::Operation(format!(
                    "アカウント '{name}' は登録されていない"
                )));
            }
            let mut out = json!({ "removed": name });
            if !master_of.is_empty() || !worker_of.is_empty() {
                out.as_object_mut().expect("object").insert(
                    "warning".into(),
                    json!(format!(
                        "このアカウントはまだプロファイルに割り当てられています\
                         （master: {}, worker: {}）。該当プロファイルの割り当てを解除しないと、\
                         master 起動 / worker spawn が未登録キーで失敗します",
                        if master_of.is_empty() {
                            "-".into()
                        } else {
                            master_of.join(", ")
                        },
                        if worker_of.is_empty() {
                            "-".into()
                        } else {
                            worker_of.join(", ")
                        },
                    )),
                );
            }
            Ok(out)
        }
        "use" => dispatch_account_use(name, profile, master, worker),
        "login" => {
            let host = host.ok_or_else(|| {
                DispatchError::Operation(
                    "login は tako の GUI が動いているときだけ使えます（ペインを生やすため）"
                        .into(),
                )
            })?;
            dispatch_account_login(host, origin, name, pane, tab)
        }
        other => Err(DispatchError::InvalidParams(format!(
            "action が不正: {other}（list / show / add / remove / use / login）"
        ))),
    }
}

/// `use` — プロファイルの master_account / worker_account を書き換える（Issue #709）
fn dispatch_account_use(
    name: Option<&str>,
    profile: Option<&str>,
    master: bool,
    worker: bool,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::{self, accounts as acct};

    let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
    let roles = acct::AssignRoles { master, worker };
    if roles.is_empty() {
        return Err(DispatchError::InvalidParams(
            "割り当て先を指定する（master / worker の少なくとも一方）".into(),
        ));
    }
    let profile_name = profile.unwrap_or("default");

    // 書き込む前に必ず解決する（未登録キーや壊れたエントリを profiles へ書かない）
    let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
    let resolved = config
        .resolve(name)
        .map_err(|e| DispatchError::Operation(format!("{e}（{}）", config.known_names_hint())))?;

    orchestrator::Profile::mutate_named(profile_name, |p| {
        if master {
            p.master_account = Some(name.to_string());
        }
        if worker {
            p.worker_account = Some(name.to_string());
        }
    })
    .map_err(DispatchError::Operation)?;

    let status = acct::probe_status(resolved.config_dir.path());
    Ok(json!({
        "name": name,
        "profile": profile_name,
        "master": master,
        "worker": worker,
        "config_dir": resolved.config_dir.path(),
        "inherit": resolved.config_dir.is_inherit(),
        "status": status.as_str(),
        "email": status.email(),
        "applies_when": roles.applies_when(),
    }))
}

/// `login` — config dir を用意し、そのアカウントで claude を起動するペインを生やす（Issue #709）
fn dispatch_account_login(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    name: Option<&str>,
    pane: Option<u64>,
    tab: Option<u64>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::{self, accounts as acct};

    let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
    let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
    let resolved = config
        .resolve(name)
        .map_err(|e| DispatchError::Operation(format!("{e}（{}）", config.known_names_hint())))?;
    // inherit のアカウントは tako が作る config dir が無い（理由つきで断る）
    let plan = acct::login_plan(&resolved).map_err(DispatchError::Operation)?;

    let status_before = acct::probe_status(Some(&plan.config_dir));
    if plan.needs_create {
        std::fs::create_dir_all(&plan.config_dir).map_err(|e| {
            DispatchError::Operation(format!(
                "config ディレクトリの作成に失敗: {} ({e})",
                plan.config_dir
            ))
        })?;
    }

    // 分割先の解決（tab 指定 > 呼び出し元ペイン）。git resolve（#496）と同じ方式
    let (tab_id, split_target) = if let Some(raw_tab) = tab {
        let tid = find_tab(host.workspace(), raw_tab)?;
        let focused = host
            .workspace()
            .get_tab(tid)
            .ok_or_else(|| op_err("タブが見つかりません"))?
            .tree()
            .focused();
        (tid, focused)
    } else {
        resolve_pane(host.workspace(), pane)?
    };

    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();
    let layout = crate::setup::spawn_layout_config();
    tree_mut(host.workspace_mut(), tab_id)
        .spawn_worker(split_target, new_pane, &layout)
        .map_err(op_err)?;
    // フォーカスは新しいペインへ移す（ユーザーがそのままログイン操作を続けるため）
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(new_id);

    host.attach_session(
        new_id,
        SpawnOptions {
            command: None,
            cwd: None,
            env: vec![(
                orchestrator::CLAUDE_CONFIG_DIR_ENV.to_string(),
                plan.config_dir.clone(),
            )],
        },
    );

    // セッション env に加えてコマンド先頭でも代入する。ログインシェルの direnv 等が
    // CLAUDE_CONFIG_DIR を設定してくる環境で後勝ちにするため（master / worker と同じ方針）
    let mut command =
        orchestrator::agent::env_assign(orchestrator::CLAUDE_CONFIG_DIR_ENV, &plan.config_dir);
    command.push_str("claude");
    host.queue_command_flow(new_id, command.clone());

    // 未ログインなら claude 自身がログインを促すので何も送らない。
    // すでにログイン済みの config dir を「別アカウントへ切り替える」ときだけ /login を送る
    let sent_slash_command = status_before.is_logged_in();
    if sent_slash_command {
        host.queue_prompt_flow(new_id, acct::LOGIN_SLASH_COMMAND.to_string());
    }

    let title = format!("login: {name}");
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    pane_obj.set_title(Some(title.clone()));
    pane_obj.set_spawned_by(Some(split_target));

    let tmux_session = host
        .reserve_backend_session(new_id)
        .or_else(|| host.backend_session(new_id));

    Ok(json!({
        "name": name,
        "config_dir": plan.config_dir,
        "created": plan.needs_create,
        "status_before": status_before.as_str(),
        "email_before": status_before.email(),
        "pane_id": new_id.as_u64(),
        "tab_id": tab_id.as_u64(),
        "title": title,
        "command": command,
        "sent_slash_command": sent_slash_command,
        "next_step": if sent_slash_command {
            "ペインで /login の案内に従ってアカウントを切り替えてください"
        } else {
            "ペインで claude のログイン手順（ブラウザ認証）を完了してください"
        },
        "tmux_session": tmux_session,
    }))
}

/// （二重実装を作らない。#83 の教訓）。
/// 全パラメータ None = 取得、いずれか Some = 検証して更新。更新はロック付き
/// read-modify-write（#169。並行する他プロセスの設定更新を巻き戻さない）。
/// 応答は解決済みの現在値
pub fn dispatch_orchestrator_layout(
    policy: Option<&str>,
    master_ratio: Option<f32>,
    algorithm: Option<&str>,
) -> Result<Value, DispatchError> {
    // 検証は書き込み前に完了させる（不正値ではロックを取らない）
    let policy = policy
        .map(tako_core::SpawnLayoutPolicy::parse)
        .transpose()
        .map_err(DispatchError::InvalidParams)?;
    if let Some(r) = master_ratio {
        if !r.is_finite() || !(0.1..=0.9).contains(&r) {
            return Err(DispatchError::InvalidParams(format!(
                "master_ratio は 0.1〜0.9 で指定してください（指定値: {r}）"
            )));
        }
    }
    let algorithm = algorithm
        .map(tako_core::WorkerLayoutAlgorithm::parse)
        .transpose()
        .map_err(DispatchError::InvalidParams)?;

    let changed = policy.is_some() || master_ratio.is_some() || algorithm.is_some();
    let resolved = if changed {
        crate::setup::mutate_config(|config| {
            if let Some(p) = policy {
                config.spawn_layout.policy = Some(p.as_str().to_string());
            }
            if let Some(r) = master_ratio {
                config.spawn_layout.master_ratio = Some(r);
            }
            if let Some(a) = algorithm {
                config.spawn_layout.algorithm = Some(a.as_str().to_string());
            }
            config.spawn_layout.resolve()
        })
        .map_err(DispatchError::Operation)?
    } else {
        crate::setup::load_config()
            .map_err(DispatchError::Operation)?
            .spawn_layout
            .resolve()
    };
    // f32 → f64 の昇格ノイズ（0.6 → 0.6000000238…）を応答から除く
    let ratio = (f64::from(resolved.master_ratio) * 1000.0).round() / 1000.0;
    Ok(json!({
        "policy": resolved.policy.as_str(),
        "master_ratio": ratio,
        "algorithm": resolved.algorithm.as_str(),
        "updated": changed,
        "config_path": crate::setup::config_yaml_path().ok(),
    }))
}

/// OrchestratorSelf — master が自身の pane/tab/ctx% を取得する（#123 / #193 / #210）
fn dispatch_orchestrator_self(
    host: &dyn ControlHost,
    pane: Option<u64>,
    caller_role: Option<&str>,
    caller_pid: Option<u32>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    let role_suffix = caller_role
        .and_then(|r| r.strip_prefix("master:"))
        .or_else(|| caller_role.and_then(|r| r.strip_prefix("solo:")))
        .map(str::to_string);

    // #288: pid 祖先辿り → pane env → stale map → role（複数時エラー）
    let (tab_id, pane_id) = resolve_caller_pane(host, pane, caller_role, caller_pid)?;

    let profile_name = role_suffix
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("default");

    // session_id の自動解決（バックエンドセッション → pid 祖先辿り）
    let session_id = resolve_session_id_for_pane_via_host(host, pane_id);

    let (status, ctx_percent) = if let Some(sid) = &session_id {
        let agent_status = orchestrator::query_agent_status(sid);
        (agent_status.status, agent_status.ctx_percent)
    } else {
        ("unknown".to_string(), None)
    };

    let ctx_threshold = crate::setup::load_config()
        .map(|c| c.ctx_threshold)
        .unwrap_or(60);

    let handoff_path = orchestrator::handoff_path(profile_name);
    let handoff_exists = handoff_path.as_ref().is_some_and(|p| p.is_file());

    Ok(json!({
        "pane_id": pane_id.as_u64(),
        "tab_id": tab_id.as_u64(),
        "profile": profile_name,
        "role": caller_role,
        "session_id": session_id,
        "status": status,
        "ctx_percent": ctx_percent,
        "ctx_threshold": ctx_threshold,
        "ctx_over_threshold": ctx_percent.map(|c| c >= ctx_threshold),
        "handoff_path": handoff_path,
        "handoff_exists": handoff_exists,
    }))
}

/// #288: caller のペインを解決する共通関数
fn resolve_caller_pane(
    host: &dyn ControlHost,
    pane: Option<u64>,
    caller_role: Option<&str>,
    caller_pid: Option<u32>,
) -> Result<(TabId, PaneId), DispatchError> {
    if let Some(pid) = caller_pid {
        let pane_backends = collect_pane_backends(host);
        if let Some(resolved_pane) = crate::agents::resolve_pane_by_pid(pid, &pane_backends) {
            if let Ok(resolved) = resolve_pane(host.workspace(), Some(resolved_pane)) {
                return Ok(resolved);
            }
        }
    }
    if let Some(resolved) = pane.and_then(|p| resolve_pane(host.workspace(), Some(p)).ok()) {
        return Ok(resolved);
    }
    if let Some(resolved) = pane
        .map(PaneId::from_raw)
        .and_then(|stale| host.resolve_stale_pane(stale))
        .and_then(|new_id| resolve_pane(host.workspace(), Some(new_id.as_u64())).ok())
    {
        return Ok(resolved);
    }
    let role_suffix = caller_role
        .and_then(|r| r.strip_prefix("master:"))
        .or_else(|| caller_role.and_then(|r| r.strip_prefix("solo:")))
        .unwrap_or("");
    find_master_pane_strict(host.workspace(), role_suffix, caller_role)
}

/// #288 B: role 検索で master/solo ペインを探す。複数マッチ時は曖昧エラー
fn find_master_pane_strict(
    ws: &tako_core::Workspace,
    suffix: &str,
    caller_role: Option<&str>,
) -> Result<(TabId, PaneId), DispatchError> {
    let is_solo = caller_role.is_some_and(|r| r.starts_with("solo"));
    let prefix = if is_solo {
        "orchestrator-solo"
    } else {
        "orchestrator-master"
    };
    let target_role = if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}:{suffix}")
    };
    let mut exact: Vec<(TabId, PaneId)> = Vec::new();
    for t in ws.tabs() {
        for p in t.tree().panes() {
            if p.role().is_some_and(|r| r == target_role) {
                exact.push((t.id(), p.id()));
            }
        }
    }
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        let ids: Vec<String> = exact.iter().map(|(_, p)| p.as_u64().to_string()).collect();
        return Err(DispatchError::Operation(format!(
            "role '{target_role}' が複数ペインに存在（pane: {}）。--pane で明示指定してください",
            ids.join(", ")
        )));
    }
    let mut fb: Vec<(TabId, PaneId)> = Vec::new();
    for t in ws.tabs() {
        for p in t.tree().panes() {
            if p.role().is_some_and(|r| r.starts_with(prefix)) {
                fb.push((t.id(), p.id()));
            }
        }
    }
    match fb.len() {
        0 => Err(DispatchError::Operation(
            "master/solo ペインが見つからない（pane を明示指定するか、TAKO_ORCHESTRATOR_ROLE を確認してください）".into()
        )),
        1 => Ok(fb[0]),
        _ => {
            let ids: Vec<String> = fb.iter().map(|(_, p)| p.as_u64().to_string()).collect();
            Err(DispatchError::Operation(format!(
                "master/solo ペインが複数（pane: {}）。pid / env / stale map では解決できず、role も曖昧。--pane で明示指定してください",
                ids.join(", ")
            )))
        }
    }
}

fn collect_pane_backends(host: &dyn ControlHost) -> Vec<(u64, String)> {
    let mut result = Vec::new();
    for tab in host.workspace().tabs() {
        for pane in tab.tree().panes() {
            if let Some(session) = host.backend_session(pane.id()) {
                result.push((pane.id().as_u64(), session));
            }
        }
    }
    result
}

/// #288: spawn の分割元ペイン解決（pid 以外のフォールバック）
fn resolve_spawn_pane_fallback(
    host: &dyn ControlHost,
    pane: Option<u64>,
    tab: Option<u64>,
    caller_role: Option<&str>,
    role_suffix: &Option<String>,
) -> Result<(TabId, PaneId), DispatchError> {
    if let Some(resolved) = pane.and_then(|p| resolve_pane(host.workspace(), Some(p)).ok()) {
        return Ok(resolved);
    }
    if let Some(resolved) = pane
        .map(PaneId::from_raw)
        .and_then(|stale| host.resolve_stale_pane(stale))
        .and_then(|new_id| resolve_pane(host.workspace(), Some(new_id.as_u64())).ok())
    {
        return Ok(resolved);
    }
    if let Some(raw_tab) = tab {
        let tid = find_tab(host.workspace(), raw_tab)?;
        let focused = host.workspace().get_tab(tid).unwrap().tree().focused();
        return Ok((tid, focused));
    }
    let suffix = role_suffix.as_deref().unwrap_or("");
    find_master_pane_strict(host.workspace(), suffix, caller_role)
}

/// ペインの session_id をバックエンドセッション → pid 祖先辿りで解決する。
/// 既存の agents::resolve_session_id_for_backend を流用
fn resolve_session_id_for_pane_via_host(host: &dyn ControlHost, pane_id: PaneId) -> Option<String> {
    let backend = host.backend_session(pane_id)?;
    crate::agents::resolve_session_id_for_backend(&backend)
}

/// #364: worker の報告内容を取得する。
/// 第 1 層: tmux scrollback（capture-pane -p -J -S。全 agent 共通）。
/// 第 2 層: 構造化ソース（claude transcript。アダプタ拡張可能）。
/// source フィールドで判別。transcript 利用時は scrollback も併記し対比可能にする
fn dispatch_orchestrator_report(
    host: &dyn ControlHost,
    query: WorkerQuery,
    lines: usize,
    messages: usize,
) -> Result<Value, DispatchError> {
    let pane_id = query.pane_id;
    let target = PaneId::from_raw(pane_id);
    let mut result = json!({ "pane_id": pane_id });

    // 第 1 層: scrollback（全 agent 共通の主ソース）。
    // pane が GUI から消えていても、レジストリ由来の tmux_session が生きていれば
    // そこから capture する（#390: ペイン消失後の追跡継続）。
    // pane ID 再利用の誤マッチ検証: 期待 tmux_session と現ペインの backend が
    // 食い違えば別ペインなので、backend ではなく期待セッション側を読む
    let backend = host.backend_session(target).filter(|b| {
        query
            .tmux_session
            .as_deref()
            .is_none_or(|expect| expect == b)
    });
    let pane_identity_ok = backend.is_some();
    let scrollback = if let Some(ref backend) = backend {
        capture_scrollback_joined(backend, lines)
    } else if let Some(ref ts) = query.tmux_session {
        if crate::reach::session_alive(ts) {
            result["source_fallback"] = json!("registry_tmux");
            capture_scrollback_joined(ts, lines)
        } else {
            None
        }
    } else {
        None
    };

    // 第 2 層: transcript アダプタ（claude のみ。将来 codex 等を追加する拡張点）
    // messages パラメータで直近 N 件を取得（#374。既定 1 件で後方互換）。
    // pane 経由の session 解決は pane の同一性が確認できた場合のみ
    // （pane ID 再利用時に別 worker の transcript を返さない。#390）。
    // pane から解決できなければレジストリ由来の session_id で継続
    let msg_count = messages.max(1);
    let pane_sid = if pane_identity_ok {
        resolve_session_id_for_pane_via_host(host, target)
    } else {
        None
    };
    let transcript = pane_sid.or(query.session_id).and_then(|sid| {
        let texts = crate::transcript::last_assistant_texts(&sid, msg_count).ok()?;
        if texts.is_empty() {
            return None;
        }
        result["session_id"] = json!(sid);
        Some(texts)
    });

    match (&transcript, &scrollback) {
        (Some(texts), _) => {
            result["source"] = json!("transcript");
            if msg_count == 1 {
                result["text"] = json!(texts.join("\n"));
            } else {
                result["text"] = json!(texts.last().unwrap_or(&String::new()));
                result["messages"] = json!(texts);
            }
            if let Some(ref sb) = scrollback {
                result["scrollback_text"] = json!(sb);
            }
        }
        (None, Some(sb)) => {
            result["source"] = json!("scrollback");
            result["text"] = json!(sb);
        }
        (None, None) => {
            return Err(DispatchError::Operation(format!(
                "pane {pane_id} の報告を取得できない（backend session 不在または scrollback 空）"
            )));
        }
    }

    Ok(result)
}

// --- 器がスクロールを持つペインの CLI / MCP スクロール（#687） -----------------

/// 1 リクエストで許す位置読み戻し（サブプロセス）の回数。
/// **UI スレッドで動く**ので、正確さより上限の固さを優先する
const BACKEND_SCROLL_MAX_PROBES: u32 = 4;

/// 送ったホイール報告が器へ届いて位置に反映されるまでの猶予。
///
/// 経路は tako → 外側 PTY → 器のクライアント（別プロセス）→ 器のサーバー、と
/// **プロセスを 2 回跨ぐ**。40ms では反映前に読んでしまい「動かなかった」と誤判定して
/// 余分に撃つ（隔離実測で確認）。とくに**初回は器が copy mode へ入る分だけ遅い**ので、
/// 動きが見えないときは [`BACKEND_SCROLL_SETTLE_RETRIES`] 回まで待ち直す
const BACKEND_SCROLL_SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// 1 発ぶんの反映を待ち直す回数。動きが見えたら即抜けるので、
/// 追加コストを払うのは「まだ届いていない」ときだけ
const BACKEND_SCROLL_SETTLE_RETRIES: u32 = 2;

/// 最下部へ戻すときに上乗せするイベント数。
///
/// **最下部で止まりきらないと器が copy mode に居残り、以後の打鍵が食われる**（#686）。
/// 下方向へ行き過ぎるのは無害（器が最下部でクランプして copy mode を抜ける）なので、
/// ユーザーがホイールを余分に回すのと同じことを明示的にやる
const BACKEND_SCROLL_BOTTOM_MARGIN: i32 = 12;

/// 器が 1 ホイールイベントで何行動くか（ミリ単位。プロセス内で学習してキャッシュする）。
///
/// 器の実装で決まる定数（tmux 既定は 3 行 / 5 行。**psmux 3.3.7 は実測 3 行**）だが、
/// 器に問い合わせる手段が無いので**実際に動かして測る**。1 度測れば以降の
/// リクエストは 1 発で寄るので、往復（= サブプロセスと待ち）が減る。
/// 初期値を 1.0 にしてあるのは、粒度が分かるまでは**行き過ぎない側**に倒すため
static BACKEND_SCROLL_LINES_PER_EVENT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(1000);

fn learned_lines_per_event() -> f32 {
    BACKEND_SCROLL_LINES_PER_EVENT.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
}

fn learn_lines_per_event(value: f32) {
    let milli = (value.clamp(0.05, 50.0) * 1000.0) as u32;
    BACKEND_SCROLL_LINES_PER_EVENT.store(milli, std::sync::atomic::Ordering::Relaxed);
}

/// 器がスクロールを持つペインを CLI / MCP から動かす（#687）。
///
/// psmux の attach クライアントは alt screen なので、外側 alacritty には履歴が
/// 1 行も積まれない。`TerminalSession::scroll_display` / `scroll_to` には動かせる
/// 余地が無く、これまでは応答だけ `offset: 0 / history: 0` で成功を装っていた。
///
/// ここでやること:
///
/// 1. **書きはユーザーのホイールと同じ経路**（in-process の PTY への SGR 報告）。
///    器へサブプロセスで書き込まない（#212 / #168 で UI スレッドから排除した
///    種類の同期サブプロセスを打鍵経路へ戻さない）。#654 が確立した
///    「psmux 自身に copy mode / 内側アプリ転送を振り分けさせる」形をそのまま使う
/// 2. **読みは読み取り専用の到達手段**（`DetachedCapture::scroll_probe`）。
///    応答に器の実位置を載せるので、もう嘘をつかない
/// 3. 1 ホイールイベントが何行動くかは器の実装依存（tmux 既定は 3 / 5 行）なので、
///    1 往復目の実測から学習して 1 回だけ補正する
///
/// `None` を返す = この経路の対象外（呼び出し側は従来の直接ペイン経路へ倒す）
fn scroll_backend_owned_pane(
    session: &tako_core::TerminalSession,
    backend_session: &str,
    pane: u64,
    to: Option<u64>,
    delta: Option<i32>,
) -> Option<Value> {
    // 外側 alacritty が履歴を持っているなら、そちらが正（macOS の直接ペイン・
    // persist OFF のペインはここへ来ない）
    if session.history_size() > 0 {
        return None;
    }
    // 器へホイールを届けられないペインでは何もできない。**サブプロセスを起こす前に**
    // 弾く（`send_wheel_report` の判定と同じ条件を先取りする粗いフィルタ）
    if !session.mouse_reporting() && !session.is_alt_screen() {
        return None;
    }
    let (sref, capture) = crate::reach::detached_capture(backend_session)?;
    let probe = capture.scroll_probe(&sref)?;

    // 内側アプリが alt screen（claude 等の TUI）= スクロール位置は内側アプリが持つ。
    // ホイール報告は器が内側へ転送するのでスクロール自体は効くが、位置は読めない
    if probe.alternate {
        let lines = delta.unwrap_or(0);
        let sent = session.send_wheel_report(lines, 0, 0);
        return Some(json!({
            "pane": pane,
            "offset": 0,
            "history": 0,
            "via": "app",
            "sent_events": sent.unsigned_abs(),
            "note": "ペイン内のアプリが全画面（alt screen）のため、スクロール位置はそのアプリが持つ。\
                     ホイール報告は転送したが位置は読み取れない（to での絶対指定はできない）",
        }));
    }

    let goal = scroll_goal(probe.position, probe.history, to, delta);
    let mut current = probe;
    // 1 イベント = 何行か。プロセス内で学習済みの値から始め、実測で更新する
    let mut lines_per_event = learned_lines_per_event();
    let mut sent_total: u32 = 0;
    let mut probes: u32 = 1;
    while probes < BACKEND_SCROLL_MAX_PROBES {
        let mut events = wheel_events_for(goal as i64 - current.position as i64, lines_per_event);
        // 最下部へ戻すときは撃ち足す（止まりきらないと copy mode に居残る。#686）
        if goal == 0 && events < 0 {
            events -= BACKEND_SCROLL_BOTTOM_MARGIN;
        }
        if events == 0 {
            break;
        }
        let sent = session.send_wheel_report(events, 0, 0);
        if sent == 0 {
            // 転送経路ではない = 器へホイールを届けられない。嘘をつかず対象外にする
            return None;
        }
        sent_total += sent.unsigned_abs();
        // 反映を待つ。**動きが見えるまで待ち直す**のが要点で、1 回読んで動いていない
        // だけで諦めると、まだ届いていない位置をそのまま応答に載せてしまう（#687 の嘘）
        let mut moved = 0i64;
        for _ in 0..BACKEND_SCROLL_SETTLE_RETRIES {
            if probes >= BACKEND_SCROLL_MAX_PROBES {
                break;
            }
            std::thread::sleep(BACKEND_SCROLL_SETTLE);
            let next = capture.scroll_probe(&sref)?;
            probes += 1;
            moved = next.position as i64 - current.position as i64;
            current = next;
            if moved != 0 {
                break;
            }
        }
        if moved == 0 {
            break; // これ以上寄らない（履歴端 / 器が受け取っていない）
        }
        // 最下部へのクランプが混ざった往復は粒度の材料にしない（実際より小さく見える）
        if !(goal == 0 && current.position == 0) {
            lines_per_event = (moved.unsigned_abs() as f32 / sent.unsigned_abs() as f32).max(0.05);
            learn_lines_per_event(lines_per_event);
        }
    }

    Some(json!({
        "pane": pane,
        "offset": current.position,
        "history": current.history,
        "via": "backend_wheel",
        "requested": goal,
        "sent_events": sent_total,
        "in_mode": current.in_mode,
        "lines_per_event": lines_per_event,
    }))
}

/// スクロールの目標位置（最下部からの遡り行数）を決める（純関数）。
/// `to` は絶対、`delta` は現在位置からの相対。どちらも履歴の範囲へクランプする
fn scroll_goal(position: usize, history: usize, to: Option<u64>, delta: Option<i32>) -> usize {
    let raw = match (to, delta) {
        (Some(t), _) => t as i64,
        (None, Some(d)) => position as i64 + d as i64,
        (None, None) => position as i64,
    };
    raw.clamp(0, history as i64) as usize
}

/// 残り行数を埋めるために撃つホイールイベント数（純関数）。
///
/// **四捨五入**して、器の粒度より細かい残りでは撃たない（0 を返す）のが要点。
/// 器が 1 イベントで複数行動く実装（tmux 既定は 3 / 5 行）では、残り 1 行のために
/// 1 イベント撃つと必ず行き過ぎ、次の往復で逆へ撃ち返して振動する。
/// **器の粒度で表せない位置には寄せられない**ので、そこで止めて実位置を正直に返す
fn wheel_events_for(remaining: i64, lines_per_event: f32) -> i32 {
    if remaining == 0 {
        return 0;
    }
    let per = lines_per_event.max(0.05);
    let raw = (remaining.unsigned_abs() as f32 / per).round();
    // 粒度が 1 行以下なら必ず 1 イベントは撃てる（残り 1 行を埋められる）
    let raw = raw.max(if per <= 1.0 { 1.0 } else { 0.0 });
    let events = raw.min(tako_core::terminal::PROGRAMMATIC_WHEEL_MAX as f32) as i32;
    if remaining > 0 {
        events
    } else {
        -events
    }
}

/// スクロールバックを 1 本のテキストで取得する（報告の第 1 層）。
///
/// 実際の採取は永続バックエンドの**採取**手段（`DetachedCapture`）が担う（#519）。
/// tmux は折返し行を結合して返し、psmux は結合せず行のまま返す（中身は落ちない）
fn capture_scrollback_joined(session: &str, lines: usize) -> Option<String> {
    let (session, capture) = crate::reach::detached_capture(session)?;
    capture.capture_history_joined(&session, lines)
}

/// OrchestratorHandoff — master の引き継ぎ（#193）。
/// handoff ファイルを読み、同プロファイルの新 master を同タブに spawn し、
/// handoff 内容を含むプロンプトを注入する。旧 master は閉じない（ユーザー判断）。
/// spawn には既存の OrchestratorSpawn（project 経由）を使わず、直接 Split + attach
/// を行う（handoff は「プロジェクト」ではないため projects.yaml に依存しない）
fn dispatch_orchestrator_handoff(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    pane: Option<u64>,
    caller_role: Option<&str>,
    tab: Option<u64>,
    caller_pid: Option<u32>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    let role_suffix = caller_role
        .and_then(|r| r.strip_prefix("master:"))
        .map(str::to_string);

    let profile_name = role_suffix
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("default");

    // handoff ファイルの存在確認
    let handoff_content = orchestrator::read_handoff(profile_name)
        .ok_or_else(|| {
            let path = orchestrator::handoff_path(profile_name)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            DispatchError::Operation(format!(
                "handoff ファイルが見つからない: {path}\nmaster は引き継ぎ前にこのファイルに状態を書き込む必要がある"
            ))
        })?;

    // #288: 分割元ペインの解決
    let (tab_id, split_target) = if let Some(raw_tab) = tab {
        let tid = find_tab(host.workspace(), raw_tab)?;
        let focused = host.workspace().get_tab(tid).unwrap().tree().focused();
        (tid, focused)
    } else {
        resolve_caller_pane(host, pane, caller_role, caller_pid)?
    };

    // プロファイルの読み込みとエージェント解決
    let profile = orchestrator::Profile::load(profile_name).unwrap_or_default();
    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env().map_err(DispatchError::Operation)?;
    let profile_env = profile.resolved_env();

    let master_agent = profile
        .resolve_master_agent()
        .map_err(DispatchError::InvalidParams)?;
    let launch = profile.resolve_agent_launch(master_agent, None, None);

    // 新 master の role
    let new_role = if profile_name == "default" {
        "orchestrator-master".to_string()
    } else {
        format!("orchestrator-master:{profile_name}")
    };

    // cwd はホームディレクトリ
    let cwd = orchestrator::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    // 新ペインを分割（右方向、spawn_worker レイアウト使用）
    let new_pane = tako_core::Pane::new(origin);
    let new_id = new_pane.id();
    let layout = crate::setup::spawn_layout_config();
    tree_mut(host.workspace_mut(), tab_id)
        .spawn_worker(split_target, new_pane, &layout)
        .map_err(op_err)?;
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(split_target);

    // セッション起動（cwd をホームに、プロファイル env を注入。Issue #500）
    let options = SpawnOptions {
        command: None,
        cwd: Some(cwd.clone()),
        env: profile_env.clone(),
    };
    host.attach_session(new_id, options);

    // master エージェント CLI コマンドを構築して送信
    let master_cmd = orchestrator::agent::build_worker_cmd(&orchestrator::agent::WorkerLaunch {
        agent: master_agent,
        role: &new_role,
        model: launch.model.as_deref(),
        effort: launch.effort.as_deref(),
        skip_permissions: master_agent.default_skip_permissions(),
        extra_args: &launch.extra_args,
        env: &profile_env,
        env_unsets: &[],
    });

    // 事前信頼
    let _ = orchestrator::agent::ensure_trusted(master_agent, &cwd.to_string_lossy())
        .unwrap_or_else(|e| {
            eprintln!("warning: handoff 事前信頼失敗（ダイアログ検出で継続）: {e}");
            false
        });

    // コマンド送信（送達確認つき。#640）
    host.queue_command_flow(new_id, master_cmd);

    // handoff プロンプトの構成と送信
    let handoff_prompt = format!(
        "あなたは前任 master から引き継ぎを受けた新しい master です。\n\
         以下の引き継ぎファイルの内容を読み、前任の状態を把握してから業務を開始してください。\n\n\
         --- handoff/{profile_name}.md ---\n\
         {handoff_content}\n\
         --- end ---\n\n\
         引き継ぎ内容を把握したら「引き継ぎ完了」と報告し、待機してください。"
    );
    host.queue_prompt_flow(new_id, handoff_prompt.clone());

    // タイトルと role 設定
    let window_title = format!("master-{profile_name}");
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    pane_obj.set_title(Some(window_title));
    pane_obj.set_spawned_by(Some(split_target));
    pane_obj.set_role(Some(new_role.clone()));

    let handoff_path = orchestrator::handoff_path(profile_name);
    Ok(json!({
        "new_master_pane_id": new_id.as_u64(),
        "new_master_tab_id": tab_id.as_u64(),
        "profile": profile_name,
        "role": new_role,
        "handoff_file": handoff_path,
        "handoff_prompt_length": handoff_prompt.len(),
    }))
}

/// GitResolveAgent — コンフリクト解消エージェントの起動（#496 Part 2）。
///
/// 既存の spawn 基盤（`orchestrator::agent` のコマンド構築 + 事前信頼 + PromptFlow）を
/// そのまま使い、新しい系統は作らない。`OrchestratorSpawn` を経由しないのは、
/// コンフリクトの起きたリポジトリが projects.yaml に登録済みとは限らないため
/// （handoff と同じ理由・同じ直接 Split + attach の形）。
fn dispatch_git_resolve_agent(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    pane: Option<u64>,
    agent: Option<&str>,
    tab: Option<u64>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    let repo = git_repo_for_pane(host, pane)?;
    let state = tako_core::git::conflict_state(&repo);
    if !state.is_active() {
        return Err(op_err(
            "コンフリクトが発生していません（解消エージェントを起動する状況ではありません）",
        ));
    }

    // エージェント種別はプロファイル既定を土台にし、明示指定で上書きする（新系統を作らない）
    let caller_pane = pane.map(PaneId::from_raw);
    let profile = resolve_caller_profile_with_role(host.workspace(), caller_pane, &None);
    profile.validate_env().map_err(DispatchError::Operation)?;
    let profile_env = profile.resolved_env();
    let worker_agent = profile
        .resolve_worker_agent(agent)
        .map_err(DispatchError::InvalidParams)?;
    let launch = profile.resolve_agent_launch(worker_agent, None, None);

    // 分割先タブの解決（tab 指定 > 呼び出し元ペインのタブ）。
    // Issue #496「押すと**同じタブ内に**エージェントのペインを立て」
    let (tab_id, split_target) = if let Some(raw_tab) = tab {
        let tid = find_tab(host.workspace(), raw_tab)?;
        let focused = host
            .workspace()
            .get_tab(tid)
            .ok_or_else(|| op_err("タブが見つかりません"))?
            .tree()
            .focused();
        (tid, focused)
    } else {
        resolve_pane(host.workspace(), pane)?
    };

    let cwd = repo.display().to_string();
    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();
    let layout = crate::setup::spawn_layout_config();
    tree_mut(host.workspace_mut(), tab_id)
        .spawn_worker(split_target, new_pane, &layout)
        .map_err(op_err)?;
    // フォーカスは呼び出し元に残す（UI 操作中のユーザーの入力を奪わない）
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(split_target);

    let options = SpawnOptions {
        command: None,
        cwd: Some(repo.clone()),
        env: profile_env.clone(),
    };
    host.attach_session(new_id, options);

    let role_value = "conflict-resolver";
    let agent_cmd = orchestrator::agent::build_worker_cmd(&orchestrator::agent::WorkerLaunch {
        agent: worker_agent,
        role: role_value,
        model: launch.model.as_deref(),
        effort: launch.effort.as_deref(),
        skip_permissions: launch.skip_permissions,
        extra_args: &launch.extra_args,
        env: &profile_env,
        env_unsets: &[],
    });

    // 事前信頼（未信頼フォルダの確認ダイアログにプロンプトが食われるのを防ぐ。Issue #32）
    let pre_trusted = orchestrator::agent::ensure_trusted(worker_agent, &cwd).unwrap_or_else(|e| {
        eprintln!("warning: 事前信頼の書き込みに失敗（ダイアログ検出で継続）: {e}");
        false
    });
    if launch.skip_permissions && worker_agent == orchestrator::agent::WorkerAgent::Claude {
        let _ = crate::claude_tui::ensure_bypass_accepted().map_err(|e| {
            eprintln!("warning: Bypass 事前承認の書き込みに失敗（ダイアログ検出で継続）: {e}");
        });
    }

    // 起動コマンドは送達確認つきで送る（#640）
    host.queue_command_flow(new_id, agent_cmd.clone());

    // プロンプトは雛形から生成する（文面は conflict-resolver.md で差し替え可能）
    let template = orchestrator::conflict_resolver_template();
    let prompt = orchestrator::render_conflict_prompt(
        &template,
        &orchestrator::ConflictPromptVars {
            repo: &cwd,
            operation: state.operation.as_str(),
            ours: if state.ours.is_empty() {
                "(detached HEAD)"
            } else {
                &state.ours
            },
            theirs: state.theirs.as_deref().unwrap_or("(不明)"),
            files: &state.files,
        },
    );
    host.queue_prompt_flow(new_id, prompt.clone());

    let window_title = format!("conflict: {}", state.operation.as_str());
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    pane_obj.set_title(Some(window_title.clone()));
    pane_obj.set_spawned_by(Some(split_target));
    pane_obj.set_role(Some(role_value.to_string()));

    let tmux_session = host
        .reserve_backend_session(new_id)
        .or_else(|| host.backend_session(new_id));

    Ok(json!({
        "pane_id": new_id.as_u64(),
        "tab_id": tab_id.as_u64(),
        "spawned_by": split_target.as_u64(),
        "agent": worker_agent.as_str(),
        "model": launch.model,
        "effort": launch.effort,
        "title": window_title,
        "cwd": cwd,
        "command": agent_cmd,
        "prompt": prompt,
        "prompt_template": orchestrator::conflict_resolver_prompt_path()
            .map(|p| p.display().to_string()),
        "pre_trusted": pre_trusted,
        "tmux_session": tmux_session,
        "state": conflict_state_json(&repo, &state),
    }))
}

/// 起動コマンドの最大送信回数（初回 + 再送 2 回。Issue #665）。
/// 再送は「シェルプロンプトが見えている = 届いていない」ときだけ行うため、
/// 多くしても実害は小さいが、壊れた環境で延々と打ち込み続けないよう上限を置く
const LAUNCH_MAX_ATTEMPTS: u32 = 3;

/// OrchestratorSpawn のパラメータ（Request と 1:1）
struct SpawnParams<'a> {
    project: &'a str,
    prompt: &'a str,
    label: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    pane: Option<u64>,
    tab: Option<u64>,
    caller_role: Option<&'a str>,
    /// worker のエージェント種別（claude / codex / agy。省略時はプロファイル既定。#120）
    agent: Option<&'a str>,
    caller_pid: Option<u32>,
    /// 委任台帳の task_type（Issue #292。統制語彙。省略時は investigation）
    task_type: Option<&'a str>,
    /// アカウント名（accounts.yaml のキー。この worker だけ該当 config dir で起動。#504）
    account: Option<&'a str>,
}

fn dispatch_orchestrator_spawn(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    params: SpawnParams,
) -> Result<Value, DispatchError> {
    let SpawnParams {
        project,
        prompt,
        label,
        model,
        effort,
        pane,
        tab,
        caller_role,
        agent,
        caller_pid,
        task_type: _task_type,
        account,
    } = params;
    if pane.is_none() && tab.is_none() {
        return Err(DispatchError::Operation(
            "pane または tab を指定してください".into(),
        ));
    }

    use crate::orchestrator;

    let config = orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
    let cwd = config
        .resolve_cwd(project)
        .map_err(DispatchError::Operation)?;

    // caller_role から master suffix を抽出する（#109: pane が stale でも正しい master を特定）
    let role_suffix = caller_role
        .and_then(|r| r.strip_prefix("master:"))
        .map(str::to_string);

    // エージェント種別と model / effort を解決する（#120）。明示指定 → プロファイル。
    // agent=claude は従来の worker_model_policy 解決を維持し、model が None に解決された
    // 場合は --model を付けず CLI の既定に委ねる（Issue #27）。
    // 検証はペイン分割の**前**に行う（不正 agent でペインだけ生える事故を防ぐ）
    let caller_pane = pane.map(PaneId::from_raw);
    let profile = resolve_caller_profile_with_role(host.workspace(), caller_pane, &role_suffix);

    // Part 2: projects 制限の強制（Issue #500）。プロファイルに projects が設定されている場合、
    // 範囲外のプロジェクトへの spawn を拒否する
    if let Some(ref allowed) = profile.projects {
        if !allowed.iter().any(|p| p == project) {
            return Err(DispatchError::Operation(format!(
                "プロファイルの projects 制限により、プロジェクト '{project}' への spawn は許可されていない（許可: {}）",
                allowed.join(", ")
            )));
        }
    }

    // Part 1: env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env().map_err(DispatchError::Operation)?;

    // #504: アカウント解決（spawn 指定 > worker_account > master_account）
    let account_name = profile.resolve_worker_account_name(account);
    let resolved_account = if let Some(acct_name) = account_name {
        let accounts = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
        Some(
            accounts
                .resolve(acct_name)
                .map_err(DispatchError::Operation)?,
        )
    } else {
        None
    };
    let env_plan = profile.resolved_env_with_account(resolved_account.as_ref());
    let profile_env = env_plan.exports.clone();

    let worker_agent = profile
        .resolve_worker_agent(agent)
        .map_err(DispatchError::InvalidParams)?;
    // アカウントの default_model / default_effort をフォールバックに使う（#504）
    let effective_model = model.or(resolved_account
        .as_ref()
        .and_then(|a| a.default_model.as_deref()));
    let effective_effort = effort.or(resolved_account
        .as_ref()
        .and_then(|a| a.default_effort.as_deref()));
    let launch = profile.resolve_agent_launch(worker_agent, effective_model, effective_effort);
    let window_title = match label {
        Some(l) => format!("{project}: {l}"),
        None => format!("{project}-worker"),
    };

    // #288: 分割元ペインの解決。pid 祖先辿り → pane → stale → tab → role（複数時エラー）
    let (tab_id, target) = if let Some(pid) = caller_pid {
        let pane_backends = collect_pane_backends(host);
        if let Some(rp) = crate::agents::resolve_pane_by_pid(pid, &pane_backends) {
            if let Ok(resolved) = resolve_pane(host.workspace(), Some(rp)) {
                resolved
            } else {
                resolve_spawn_pane_fallback(host, pane, tab, caller_role, &role_suffix)?
            }
        } else {
            resolve_spawn_pane_fallback(host, pane, tab, caller_role, &role_suffix)?
        }
    } else {
        resolve_spawn_pane_fallback(host, pane, tab, caller_role, &role_suffix)?
    };
    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();
    // spawn レイアウトエンジン（Issue #165）: 配置は config.yaml の spawn_layout に従う。
    // 既定 = master-reserved（spawn 元の取り分を維持し、worker は右側の worker 領域内へ
    // grid 配置）。領域判定は既存 worker の spawned_by チェーンによる
    let layout = crate::setup::spawn_layout_config();
    tree_mut(host.workspace_mut(), tab_id)
        .spawn_worker(target, new_pane, &layout)
        .map_err(op_err)?;
    // MCP/CLI 経由ではフォーカスを分割元に維持（ユーザーの入力を奪わない）
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(target);
    let options = SpawnOptions {
        command: None,
        cwd: Some(std::path::PathBuf::from(&cwd)),
        env: profile_env.clone(),
    };
    host.attach_session(new_id, options);

    let role_value = match label {
        Some(l) => format!("worker:{project}:{l}"),
        None => format!("worker:{project}"),
    };
    let worker_cmd = orchestrator::agent::build_worker_cmd(&orchestrator::agent::WorkerLaunch {
        agent: worker_agent,
        role: &role_value,
        model: launch.model.as_deref(),
        effort: launch.effort.as_deref(),
        skip_permissions: launch.skip_permissions,
        extra_args: &launch.extra_args,
        env: &profile_env,
        // inherit のアカウントは CLAUDE_CONFIG_DIR を明示 unset する（#512）
        env_unsets: &env_plan.unsets,
    });

    // 事前信頼: 未信頼フォルダでエージェント CLI を起動すると信頼ダイアログが出て、
    // 送信したプロンプトがダイアログへの応答として消費される（Issue #32 問題 1）。
    // 起動前に各 CLI の設定ファイル（claude: ~/.claude.json / codex: ~/.codex/config.toml /
    // agy: ~/.gemini/antigravity-cli/settings.json）へ信頼済みを書き込んでダイアログ自体を
    // 出さない。失敗しても PromptFlow のダイアログ検出 → 承諾がフォールバックするため継続する
    let pre_trusted = orchestrator::agent::ensure_trusted(worker_agent, &cwd).unwrap_or_else(|e| {
        eprintln!("warning: 事前信頼の書き込みに失敗（ダイアログ検出で継続）: {e}");
        false
    });

    // Bypass Permissions 事前承認（#407）: skip_permissions=true の claude worker は
    // --dangerously-skip-permissions で起動する。初回は確認ダイアログが出て既定選択
    // 「No, exit」で即終了するため、起動前に ~/.claude.json へ承認済みを書き込む。
    // フォールバック: deliver_via_tmux の bypass ダイアログ検出 → 承諾
    if launch.skip_permissions && worker_agent == orchestrator::agent::WorkerAgent::Claude {
        let _ = crate::claude_tui::ensure_bypass_accepted().map_err(|e| {
            eprintln!("warning: Bypass 事前承認の書き込みに失敗（ダイアログ検出で継続）: {e}");
        });
    }

    // 起動コマンドとプロンプトの送出は**起動保証の状態機械**へ委ねる（Issue #665）。
    // 従来はここで queue_write（起動コマンド）+ queue_prompt_flow（プロンプト）を
    // 投げっぱなしにしており、どちらも届いたか誰も確認していなかった。
    // 起動コマンドの送達そのものは #640 の送達確認フロー（queue_command_flow）を
    // そのまま使い、その上に「エージェントが実際に起動したか」「プロンプトが
    // 入力欄から消えたか」の検証と再送を重ねる。実際の登録はレジストリの
    // worker_id が決まってから行う（段階を workers.yaml へ記録するため）

    // タイトルと role 設定
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    pane_obj.set_title(Some(window_title.clone()));
    pane_obj.set_spawned_by(Some(target));
    let pane_role = match label {
        Some(l) => format!("orchestrator-worker:{project}:{l}"),
        None => format!("orchestrator-worker:{project}"),
    };
    pane_obj.set_role(Some(pane_role));

    // attach は非同期のため backend セッション名をここで事前予約する（Issue #112。
    // 従来の `backend_session(new_id)` は spawn 時点で常に None = 応答の tmux_session が
    // 空で、pane 消失時の tmux フォールバック用の値を master へ渡せていなかった）
    let tmux_session = host
        .reserve_backend_session(new_id)
        .or_else(|| host.backend_session(new_id));

    // セッションカタログへ spawn 記録を残す（Issue #112 A）。session_id は claude 起動後に
    // GUI の定期スキャンが検出して昇格する。失敗してもカタログの都合で spawn は止めない
    if let Some(ref ts) = tmux_session {
        let issues =
            crate::sessions::extract_issues(&format!("{} {prompt}", label.unwrap_or_default()));
        let record = crate::sessions::PendingSpawn {
            tmux_session: ts.clone(),
            kind: "worker".into(),
            label: label.map(str::to_string),
            project: Some(project.to_string()),
            agent: Some(worker_agent.as_str().to_string()),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
            issues,
            prompt_head: Some(crate::sessions::prompt_head(prompt, 200)),
            cwd: Some(cwd.clone()),
            tab: Some(tab_id.as_u64()),
            pane: Some(new_id.as_u64()),
            recorded_at: crate::sessions::now_iso(),
        };
        if let Err(e) = crate::sessions::record_spawn(record) {
            eprintln!("warning: セッションカタログへの spawn 記録に失敗: {e}");
        }
    }

    // 委任台帳への自動記録（Issue #292 層1）。失敗しても spawn は止めない
    let issue_num =
        crate::sessions::extract_issues(&format!("{} {prompt}", label.unwrap_or_default()))
            .into_iter()
            .next();
    let issue_str = issue_num.map(|n| format!("#{n}"));
    let ledger_id = crate::orchestrator::ledger::record_spawn(
        project,
        label,
        issue_str.as_deref(),
        _task_type,
        launch.model.as_deref().unwrap_or("(default)"),
        launch.effort.as_deref(),
        Some(worker_agent.as_str()),
    )
    .unwrap_or_else(|e| {
        eprintln!("warning: 委任台帳への記録に失敗: {e}");
        String::new()
    });

    // worker レジストリへ登録（Issue #390）。ペイン消失・tako 再起動後も
    // watch / status / report が追跡を継続するための永続キー。失敗しても spawn は止めない
    let worker_id =
        crate::orchestrator::registry::record_spawn(crate::orchestrator::registry::RegisterSpawn {
            label: label.map(str::to_string),
            project: project.to_string(),
            agent: worker_agent.as_str().to_string(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
            pane: new_id.as_u64(),
            tab: Some(tab_id.as_u64()),
            tmux_session: tmux_session.clone(),
            issues: crate::sessions::extract_issues(&format!(
                "{} {prompt}",
                label.unwrap_or_default()
            )),
            ledger_id: Some(ledger_id.clone()).filter(|s| !s.is_empty()),
            cwd: Some(cwd.clone()),
            prompt_head: Some(crate::sessions::prompt_head(prompt, 200)),
        })
        .unwrap_or_else(|e| {
            eprintln!("warning: worker レジストリへの登録に失敗: {e}");
            String::new()
        });

    // 起動保証の登録（Issue #665）。シェル起動 → 起動コマンド送出 →
    // エージェント CLI 起動の確認 → プロンプト送達 を段階ごとに検証し、
    // 届いていなければ起動コマンドを再送する。段階は workers.yaml へ記録され、
    // `OrchestratorLaunchStatus`（CLI / MCP）からプロセス外で読める
    host.queue_launch_assurance(
        new_id,
        crate::host::LaunchAssuranceSpec {
            worker_id: worker_id.clone(),
            command: worker_cmd.clone(),
            prompt: prompt.to_string(),
            agent: worker_agent.as_str().to_string(),
            max_attempts: LAUNCH_MAX_ATTEMPTS,
        },
    );

    // 監視役を確実に立てる（Issue #665）。master が watch を張り忘れる余地を無くすため、
    // spawn した時点で supervisor が居なければ起こす。既に居れば何もしない
    // （シングルトン）。プロセス起動だけなので UI スレッドを塞がない
    {
        let profile_mode = profile
            .supervisor_mode
            .unwrap_or(crate::orchestrator::supervisor::SupervisorMode::Auto);
        crate::orchestrator::supervisor::ensure_running(profile_mode);
    }

    // #368: spawn 完了 → claude session スキャンを即時トリガー
    crate::request_claude_scan();

    // Part 4: env のキー一覧（値はマスク。Issue #500）
    let env_keys: Vec<&str> = profile_env.iter().map(|(k, _)| k.as_str()).collect();
    // CLAUDE_CONFIG_DIR が設定されている場合、config dir パスを応答に含める
    let config_dir_value = profile_env
        .iter()
        .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        .map(|(_, v)| v.as_str());

    Ok(json!({
        "pane_id": new_id.as_u64(),
        "spawned_by": target.as_u64(),
        "title": window_title,
        "cwd": cwd,
        "agent": worker_agent.as_str(),
        "model": launch.model,
        "effort": launch.effort,
        "command": worker_cmd,
        // 旧フィールド名の互換（#120 以前のクライアント / ドキュメント向け）
        "claude_command": worker_cmd,
        "prompt": prompt,
        "pre_trusted": pre_trusted,
        "tmux_session": tmux_session,
        "ledger_id": ledger_id,
        "worker_id": Some(worker_id).filter(|s| !s.is_empty()),
        "env_keys": env_keys,
        "config_dir": config_dir_value,
        // inherit のアカウントでは config_dir が null になるので、
        // 「未設定に戻した」ことを別フィールドで示す（#512 / #709）
        "env_unsets": env_plan.unsets,
        "account": account_name,
        // 起動保証（Issue #665）。この時点では「登録した」だけで、実際の到達段階は
        // 状態機械が進める。呼び出し側（MCP ハンドラスレッド / CLI プロセス）は
        // OrchestratorLaunchStatus をポーリングして prompt_delivered / failed を待つ
        "assurance": {
            "level": crate::orchestrator::launch::LaunchPhase::Queued.as_str(),
            "attempts": 0,
            "max_attempts": LAUNCH_MAX_ATTEMPTS,
            "poll": "tako orchestrator launch-status --pane <pane_id>",
        },
    }))
}

/// OrchestratorLaunchStatus の dispatch（Issue #665）。
/// UI スレッドで即座に返る軽量照会。稼働中の状態機械があればそれを、
/// 無ければレジストリの永続記録を返す（状態機械は完了後に破棄されるため）
fn dispatch_orchestrator_launch_status(
    host: &dyn ControlHost,
    pane: Option<u64>,
    worker: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::launch::LaunchPhase;
    use crate::orchestrator::registry::WorkerRegistry;

    // worker 指定ならレジストリから pane を引く（ペイン消失後も照会できる）
    let (pane_id, worker_id) = match (pane, worker) {
        (_, Some(w)) => {
            let reg = WorkerRegistry::load().map_err(DispatchError::Operation)?;
            let (id, entry) = reg.resolve(w).map_err(DispatchError::Operation)?;
            (entry.pane, Some(id.clone()))
        }
        (Some(p), None) => {
            let worker_id = WorkerRegistry::load()
                .ok()
                .and_then(|reg| reg.find_active_by_pane(p).map(|(id, _)| id.clone()));
            (p, worker_id)
        }
        (None, None) => {
            return Err(DispatchError::InvalidParams(
                "pane または worker を指定してください".into(),
            ))
        }
    };

    // ① 稼働中の状態機械（最新・権威）
    if let Some(status) = host.launch_assurance_status(PaneId::from_raw(pane_id)) {
        return Ok(json!({
            "pane": pane_id,
            "worker_id": worker_id,
            "level": status.phase.as_str(),
            "attempts": status.attempts,
            "elapsed_ms": status.elapsed_ms,
            "detail": status.detail,
            "describe": status.phase.describe(),
            "settled": status.phase.is_terminal(),
            "agent_running": status.phase.agent_running(),
            "source": "live",
        }));
    }

    // ② 完了・破棄済み → レジストリの永続記録
    let record = worker_id.as_deref().and_then(|id| {
        WorkerRegistry::load()
            .ok()
            .and_then(|reg| reg.workers.get(id).and_then(|e| e.launch.clone()))
    });
    match record {
        Some(rec) => {
            let phase = rec.phase().unwrap_or(LaunchPhase::Queued);
            Ok(json!({
                "pane": pane_id,
                "worker_id": worker_id,
                "level": phase.as_str(),
                "attempts": rec.attempts,
                "detail": rec.detail,
                "describe": phase.describe(),
                "settled": phase.is_terminal(),
                "agent_running": phase.agent_running(),
                "updated_at": rec.updated_at,
                "source": "registry",
            }))
        }
        // 記録が無い = #665 以前に spawn された / レジストリ登録に失敗した worker。
        // 「保証の対象外」であることを明示する（不明を成功と誤読させない）
        None => Ok(json!({
            "pane": pane_id,
            "worker_id": worker_id,
            "level": Value::Null,
            "settled": true,
            "agent_running": Value::Null,
            "source": "none",
            "detail": "起動保証の記録が無い（#665 以前に spawn された worker、\
                       またはレジストリ登録に失敗した）",
        })),
    }
}

/// OrchestratorWorkerStatus の UI スレッド必須部分（workspace / ライブ画面の読み取り）の
/// 収集結果。残り（claude CLI / tmux / ps のサブプロセス実行）はこの文脈だけで
/// UI スレッド外で完了できる（#168 / #181: UI 非ブロック化の分割点。
/// #181 の worker_status_snapshot/compute と同時期に同じ分割で実装され、
/// GitLog / GitDiff も扱う OffloadJob 機構へ一本化した）
pub struct WorkerStatusCtx {
    /// 照会対象のペイン ID（#390: レジストリ突き合わせ用）
    pane_id: u64,
    pane_exists: bool,
    backend_session: Option<String>,
    /// ライブ画面の末尾（空行除去 + 最大 30 行に整形済み）。ペインが GUI に無ければ None
    live_tail: Option<String>,
    /// ライブ画面全体のテキスト（折りたたみ検出用。ペインが GUI に無ければ None）
    full_screen: Option<String>,
    /// tmux セッション配下に実行中の子プロセスがあるか（#224）
    has_running_children: bool,
    /// ペインの PTY 子プロセス（シェル）の pid（#592）。
    /// 器を持たないバックエンド（Windows の backend=none）で、pane → claude セッションを
    /// 辿る唯一の起点になる。GUI に無いペイン（レジストリ照会のみ）では None
    pane_pid: Option<u32>,
}

/// 末尾の空行を除去し、最大 30 行に切り詰めて 1 本のテキストへ
fn tail_join(mut lines: Vec<String>) -> String {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.len() > 30 {
        lines.drain(..lines.len() - 30);
    }
    lines.join("\n")
}

fn collect_worker_status_ctx(host: &dyn ControlHost, pane_id: u64) -> WorkerStatusCtx {
    // ペインの存在確認（ツリー上 + shelved の両方を走査）
    let target = PaneId::from_raw(pane_id);
    let in_tree = host.workspace().tabs().iter().any(|tab| {
        tab.tree()
            .panes()
            .iter()
            .any(|p| p.id().as_u64() == pane_id)
    });
    let lines = host.session(target).map(|session| session.visible_lines());
    let full_screen = lines.as_ref().map(|l| l.join("\n"));
    let backend_session = host.backend_session(target);
    let pane_pid = host.session(target).and_then(|session| session.child_pid());
    // 器があればセッション名から、無ければペインの PTY 子プロセスから子孫を数える（#592）。
    // 器ありのときの経路は従来どおり（macOS の既定は tmux バックエンド）
    let has_running_children = match backend_session.as_ref() {
        Some(bs) => crate::agents::has_running_children(bs),
        None => pane_pid.is_some_and(crate::agents::has_running_children_of_pid),
    };
    WorkerStatusCtx {
        pane_id,
        pane_exists: in_tree || host.workspace().is_shelved(target),
        backend_session,
        has_running_children,
        live_tail: lines.map(tail_join),
        full_screen,
        pane_pid,
    }
}

/// `claude agents --json` の生 status を dispatch の語彙へ正規化する（#267）。
///
/// 正規化しないと watch ループの unknown フォールバック（画面推定）に落ち、
/// 一次シグナルを持っているのに捨てることになる。
///
/// #571: claude の実出力は `idle` / `busy`（2026-07-27 実測。旧実装が想定していた
/// `active` は現れない）。**未知の値は "unknown" に落ちて画面推定へ回る**ので、
/// 語彙がずれても壊れはしないが検知精度が落ちる。実測で確認した値を並べる
fn normalize_agent_status(raw: &str) -> &'static str {
    match raw {
        "idle" => "idle",
        "busy" | "active" | "running" => "busy",
        "waiting" | "waiting_for_input" => "waiting",
        "gone" => "gone",
        _ => "unknown",
    }
}

/// pane → claude セッションの自動解決（#44 / #592）。
///
/// 2 つの起点を順に試す。どちらも「実プロセスの祖先を辿って `claude agents --json` の
/// pid に一致するか」を見るので、role やカタログの stale 記録には依存しない:
///
/// 1. **バックエンドセッション名**（器あり = macOS の既定 tmux）。従来経路で不変
/// 2. **ペインの PTY 子プロセス pid**（器なし = Windows の backend=none / tmux 不在の macOS）。
///    器が無いとペインのシェルは tako-app の直接の子になり、セッション名では辿れない
///
/// ここで解決できないと `status_source` が "screen" のままになり、worker の完了判定が
/// 画面推定だけになる（#592 の実害: Windows で常に画面推定 = WORKER_IDLE の誤検知）
fn auto_resolve_session(backend_session: Option<&str>, pane_pid: Option<u32>) -> Option<String> {
    if let Some(backend) = backend_session {
        if let Some(sid) = crate::agents::resolve_session_id_for_backend(backend) {
            return Some(sid);
        }
    }
    crate::agents::resolve_session_id_for_pane_pid(pane_pid?)
}

fn finish_worker_status(
    ctx: WorkerStatusCtx,
    session_id: Option<&str>,
    tmux_session: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    let WorkerStatusCtx {
        pane_id,
        pane_exists,
        backend_session,
        live_tail,
        full_screen,
        has_running_children: has_children,
        pane_pid,
    } = ctx;

    // #390: レジストリの active エントリ（prompt 未達判定 + lazy 昇格用）。
    // 読めなくても既存動作は変えない（フォールバック層）
    let registry_worker: Option<(String, orchestrator::registry::WorkerEntry)> =
        orchestrator::registry::WorkerRegistry::load()
            .ok()
            .and_then(|reg| {
                reg.find_active_by_pane(pane_id)
                    .map(|(id, e)| (id.clone(), e.clone()))
            });

    // session_id の解決: 明示指定 > pane→session 自動解決 > フォールバック
    let (resolved_sid, status_source);
    if let Some(sid) = session_id {
        resolved_sid = Some(sid.to_string());
        status_source = "agents";
    } else if pane_exists {
        match auto_resolve_session(backend_session.as_deref(), pane_pid) {
            Some(sid) => {
                resolved_sid = Some(sid);
                status_source = "agents-auto";
            }
            None => {
                resolved_sid = None;
                status_source = "screen";
            }
        }
    } else {
        resolved_sid = None;
        status_source = "none";
    };

    // #267: agents の生ステータスを dispatch の語彙に正規化する。
    // 正規化しないと watch ループの unknown フォールバック（スクリーン末尾 5 行判定）に
    // 落ち、長時間ツール出力で busy パターンが流れた瞬間に偽 IDLE が出る
    let (status, ctx_percent) = if let Some(ref sid) = resolved_sid {
        let agent = orchestrator::query_agent_status(sid);
        (
            normalize_agent_status(&agent.status).to_string(),
            agent.ctx_percent,
        )
    } else if pane_exists {
        ("unknown".to_string(), None)
    } else {
        ("gone".to_string(), None)
    };

    // ペインの最近の出力（pane のライブ画面 → backend session フォールバック）。
    // 読むだけなので採取の到達手段で足りる（#519。psmux でも通る）
    let recent_output = live_tail.or_else(|| {
        let ts = tmux_session?;
        if !crate::reach::session_alive(ts) {
            return None;
        }
        let (session, capture) = crate::reach::detached_capture(ts)?;
        Some(tail_join(capture.capture_screen(&session).ok()?))
    });

    // #390: エージェントプロセスの生存シグナル（突然死判定専用）。
    // ctx の has_children（pane 健在時のみ計算）に加え、pane 消失中は
    // レジストリ由来の tmux_session で子プロセスを再計算する。
    // busy / stalled 補正の has_children には**混ぜない**: idle 中のエージェント
    // TUI プロセス自体も「子」に数えられるため、補正へ流すと pane 消失中の
    // 完了 worker が永遠に busy 判定になり IDLE を検知できない（run6 実測で確認）
    let agent_process_alive = has_children
        || (backend_session.is_none()
            && tmux_session.is_some_and(|ts| {
                crate::reach::session_alive(ts) && crate::agents::has_running_children(ts)
            }));

    // #390: エージェント突然死判定の材料と復旧コマンド（apply 側で最終判定）。
    // 判定は「画面を観測できている」ことを前提にする（recent_output なし = tmux も
    // 引けない状態で dead と断定しない）
    let registry_session_detected = registry_worker
        .as_ref()
        .is_some_and(|(_, e)| e.session_id.is_some())
        && recent_output.is_some();
    let registry_resume_command = registry_worker
        .as_ref()
        .and_then(|(_, e)| orchestrator::registry::resume_command(e));

    // #390: session_id が今回の照会で解決できたらレジストリへ書き戻す（lazy 昇格）。
    // GUI の定期スキャンが止まっていても（セカンダリモード等）prompt 到達の証跡が残り、
    // 未達の誤検知を防ぐ。best-effort（失敗は無視）
    let prompt_delivery = registry_worker.as_ref().map(|(worker_id, entry)| {
        if entry.session_id.is_none() {
            if let Some(sid) = resolved_sid.as_deref() {
                match entry.tmux_session.as_deref() {
                    Some(ts) => {
                        let _ = orchestrator::registry::record_session_detected(ts, sid);
                    }
                    // #592: 器を持たないバックエンド（Windows の backend=none）は
                    // tmux_session が None なので、セッション名キーの記録は 1 件も
                    // 刺さらない（送達済みでも prompt_delivery が undelivered のまま
                    // 残る）。器がないときだけ worker ID で直接書く
                    None => {
                        let _ = orchestrator::registry::record_session_detected_for_worker(
                            worker_id, sid,
                        );
                    }
                }
            }
        }
        let mut effective = entry.clone();
        if effective.session_id.is_none() {
            effective.session_id = resolved_sid.clone();
        }
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap_or(0);
        let spawned_epoch = crate::sessions::parse_iso(&effective.spawned_at).unwrap_or(now_epoch);
        (
            orchestrator::registry::prompt_delivery_assessment(&effective, now_epoch),
            now_epoch - spawned_epoch,
        )
    });

    apply_worker_status_corrections(ResolvedWorkerStatus {
        status,
        status_source: status_source.to_string(),
        ctx_percent,
        resolved_sid,
        pane_exists,
        has_children,
        recent_output,
        full_screen,
        tmux_session: tmux_session.map(String::from),
        registry_worker_id: registry_worker.map(|(id, _)| id),
        prompt_delivery,
        registry_session_detected,
        registry_resume_command,
        agent_process_alive,
    })
}

/// `apply_worker_status_corrections` への入力（agents / screen 解決後の初期状態）
#[derive(Default)]
struct ResolvedWorkerStatus {
    status: String,
    status_source: String,
    ctx_percent: Option<u32>,
    resolved_sid: Option<String>,
    pane_exists: bool,
    has_children: bool,
    recent_output: Option<String>,
    full_screen: Option<String>,
    tmux_session: Option<String>,
    /// #390: レジストリ上の worker ID（登録済み worker のみ）
    registry_worker_id: Option<String>,
    /// #390: prompt 送達判定（レジストリ登録済み worker のみ）と spawn からの経過秒
    prompt_delivery: Option<(crate::orchestrator::registry::PromptDelivery, i64)>,
    /// #390: レジストリに session_id が記録済み（= エージェントは一度起動して走った）
    registry_session_detected: bool,
    /// #390: レジストリの session ID から組み立てた復旧コマンド（claude のみ）
    registry_resume_command: Option<String>,
    /// #390: エージェントプロセスの生存シグナル（突然死判定専用。pane 消失中は
    /// tmux フォールバックで再計算済み。busy / stalled 補正には使わない）
    agent_process_alive: bool,
}

/// worker_status の初期状態に補正ロジックを適用し、最終的な JSON 応答を構築する。
/// `finish_worker_status` から分離した内部関数（テスト時に初期状態を直接制御するため）
fn apply_worker_status_corrections(resolved: ResolvedWorkerStatus) -> Result<Value, DispatchError> {
    let ResolvedWorkerStatus {
        mut status,
        status_source,
        ctx_percent,
        resolved_sid,
        pane_exists,
        has_children,
        recent_output,
        full_screen,
        tmux_session,
        registry_worker_id,
        prompt_delivery,
        registry_session_detected,
        registry_resume_command,
        agent_process_alive,
    } = resolved;
    // #267: agents が "gone" を返しても pane が workspace にある場合は
    // セッション未発見なだけで worker は健在 → unknown に降格
    if status == "gone" && pane_exists {
        status = "unknown".to_string();
    }
    // tmux session が生きていれば gone を取り消す（pane は無いが worker は健在）
    if status == "gone" {
        if let Some(ref ts) = tmux_session {
            if crate::reach::session_alive(ts) {
                status = "unknown".to_string();
            }
        }
    }

    // #571: agents に問い合わせたのに状態を返せなかった（セッションが一覧に無い =
    // gone → unknown へ降格 / コマンド自体が失敗 = unknown）なら、以降の根拠は画面しかない。
    // status_source を "agents*" のままにすると、画面推定の結果を watch が
    // 一次シグナル扱い（idle 連続 3 回で確定）してしまう。source を実態に合わせる
    let mut status_source = status_source;
    if status == "unknown" && status_source.starts_with("agents") {
        status_source = "screen".to_string();
    }

    // agents API（status_source = agents / agents-auto）はセッション状態の
    // 一次情報なので、idle を返したらプロセスツリー heuristic で覆さない。
    // バックグラウンドシェル（tailscaled 等）の常駐子プロセスが IDLE 検知を
    // 永久にブロックする問題を根治する（#289）。
    // 一時的な idle（サブエージェント完了瞬間）は watch の idle_streak（3 回連続）で防ぐ
    let agents_authoritative = status_source == "agents" || status_source == "agents-auto";
    if status == "idle" {
        let screen_busy = recent_output
            .as_ref()
            .is_some_and(|out| crate::orchestrator::wait::screen_looks_busy(out));
        if screen_busy || (has_children && !agents_authoritative) {
            status = "busy".to_string();
        }
    }

    // agents シグナルの無い worker（codex / agy、または claude の解決失敗）は
    // 画面推定で busy / idle を判定する（#120。wait_for_worker の unknown ブランチと
    // 同じロジックを単発クエリの応答にも反映する。status_source=screen のため
    // watch / run 側は idle 連続 8 回を要求し、単発の誤判定では完了しない）。
    //
    // #571: `has_children` を画面より優先してはいけない。エージェント CLI（claude /
    // codex / agy）の TUI プロセス自身がペインシェルの子なので、**生きている限り
    // 必ず true** になる。以前は画面が入力欄を映していても busy に上書きされ、
    // agents 解決に失敗した worker は永久に完了しなかった（watch が 40 分以上不発）。
    // プロセスツリーは「画面から判断できないとき」の補助に留める
    if status == "unknown" {
        if let Some(ref out) = recent_output {
            if crate::orchestrator::wait::screen_looks_busy(out) {
                status = "busy".to_string();
            } else if crate::orchestrator::wait::screen_looks_idle(out) {
                status = "idle".to_string();
            } else if has_children {
                status = "busy".to_string();
            }
        }
    }

    // 停止（idle）した worker の画面に既知のエラーパターン（API エラー・usage limit・
    // rate limit ダイアログ）があれば error へ細分類する（#157）。busy 中は判定しない
    // （自動リトライ・ツール実行ログへの誤検知防止。busy が明ければ idle 経由で判定される）
    let mut error_info: Option<Value> = None;
    if status == "idle" {
        if let Some((kind, detail)) = recent_output
            .as_ref()
            .and_then(|out| crate::orchestrator::wait::detect_worker_error(out))
        {
            status = "error".to_string();
            error_info = Some(json!({
                "kind": kind.as_str(),
                "detail": detail,
                "recommended_action": kind.recommended_action(),
            }));
        }
    }

    // #224 停滞検知: busy だが実行中子プロセスなし → stalled（停滞）
    let mut stalled_info: Option<Value> = None;
    if status == "busy" && !has_children {
        let screen_busy = recent_output
            .as_ref()
            .is_some_and(|out| crate::orchestrator::wait::screen_looks_busy(out));
        if !screen_busy {
            status = "stalled".to_string();
            stalled_info = Some(json!({
                "detail": "busy と判定されたが実行中の子プロセスが無く、画面の busy パターンも無い",
                "recommended_action": "check_and_resume",
            }));
        }
    }

    // #224 折りたたみ検出: TUI が「N new messages (click) ↓」で折りたたまれている
    let collapsed = full_screen
        .as_ref()
        .is_some_and(|s| crate::orchestrator::wait::screen_is_collapsed(s));

    // #243: events 配列（question / model_switched / context_high / permission_dialog）
    let mut events: Vec<Value> = crate::orchestrator::wait::collect_worker_events(
        &status,
        recent_output.as_deref(),
        ctx_percent,
    )
    .iter()
    .map(|e| e.to_json())
    .collect();

    // #390: prompt 未達検知（保守的な複合条件で誤検知を防ぐ）。
    // レジストリ判定が「猶予超過 + transcript 未観測」でも、画面が busy なら
    // 作業中とみなして発火しない（transcript 検出遅延の可能性）。
    // has_children は抑制条件にしない: プロンプト未達の claude は welcome 画面の
    // まま「プロセスとしては生存」しているため（当日の実害 2 件目 = ペイン健在の
    // 未達がまさにこの状態）、子プロセスの有無は未達かどうかの判別に使えない
    let prompt_delivery_final = prompt_delivery.map(|(assessment, since_spawn)| {
        use crate::orchestrator::registry::PromptDelivery;
        if assessment == PromptDelivery::OverdueSuspect {
            let screen_busy = recent_output
                .as_ref()
                .is_some_and(|out| crate::orchestrator::wait::screen_looks_busy(out));
            if screen_busy || status == "busy" {
                (PromptDelivery::Pending, since_spawn)
            } else {
                (assessment, since_spawn)
            }
        } else {
            (assessment, since_spawn)
        }
    });
    if let Some((crate::orchestrator::registry::PromptDelivery::OverdueSuspect, since_spawn)) =
        prompt_delivery_final
    {
        events.push(
            crate::orchestrator::wait::WorkerEvent {
                kind: crate::orchestrator::wait::WorkerEventKind::PromptUndelivered {
                    seconds_since_spawn: since_spawn,
                },
            }
            .to_json(),
        );
    }

    // #390: エージェント CLI プロセスの突然死検知（SIGSEGV 等）。
    // 条件: レジストリに session_id 記録済み（= 一度は起動して走った）+ 実行中子プロセス
    // なし + agents API がプロセス生存を確認していない + gone / busy でない。
    // busy は「動いている」判定を尊重して発火しない（誤爆ゼロ優先。SIGSEGV 残骸の
    // 偽 busy は has_children=false により stalled へ転換されてから本判定に入る）。
    // 単発照会では「疑い」であり、watch は 2 回連続観測で WORKER_DEAD を確定する。
    // 自動 resume はしない（クラッシュループの危険 + master の判断を奪わないため、
    // resume_command の提示まで）
    let alive_by_agents =
        status_source.starts_with("agents") && status != "unknown" && status != "gone";
    let agent_dead = registry_session_detected
        && !agent_process_alive
        && !alive_by_agents
        && status != "gone"
        && status != "busy";
    if agent_dead {
        events.push(
            crate::orchestrator::wait::WorkerEvent {
                kind: crate::orchestrator::wait::WorkerEventKind::AgentDead {
                    resume_command: registry_resume_command.clone(),
                },
            }
            .to_json(),
        );
    }

    // #319: waiting + 画面に permission ダイアログがあれば構造化情報を付与
    let permission_dialog: Option<Value> = if status == "waiting" {
        recent_output.as_ref().and_then(|out| {
            let lines: Vec<String> = out.lines().map(|l| l.to_string()).collect();
            let dialog = crate::claude_tui::detect_permission_dialog(&lines)?;
            Some(json!({
                "command": dialog.command,
                "options": dialog.options,
                "highlighted": dialog.highlighted,
            }))
        })
    } else {
        None
    };

    // #364: 履歴サイズ計測（agent 非依存の busy シグナル布石）。
    // 採取の到達手段で足りる（#519）。`bytes` は器によっては観測できず 0 になる
    let history_info = tmux_session
        .as_ref()
        .and_then(|ts| {
            let (session, capture) = crate::reach::detached_capture(ts)?;
            capture.history_probe(&session)
        })
        .map(|p| json!({ "lines": p.history, "bytes": p.bytes }));

    Ok(json!({
        "status": status,
        "ctx_percent": ctx_percent,
        "recent_output": recent_output,
        "status_source": status_source,
        "resolved_session_id": resolved_sid,
        "error": error_info,
        "stalled": stalled_info,
        "has_running_children": has_children,
        "collapsed": collapsed,
        "events": events,
        "permission_dialog": permission_dialog,
        "history": history_info,
        // #390: worker レジストリ由来の情報（未登録ペインは null）
        "worker_id": registry_worker_id,
        "prompt_delivery": prompt_delivery_final.map(|(d, _)| d.as_str()),
        "resume_command": registry_resume_command,
    }))
}

/// ダイアログ操作の到達手段（#662）。
///
/// **in-process を必ず先に試す**のが要点。旧実装は `reach::detached_session` だけを見ており、
/// tako-app が当該ペインを保持していても「アウトオブプロセス到達手段が無い」で失敗していた
/// （Windows の psmux backend は `DetachedAccess` を持たないので**常に**失敗した）。
/// 画面採取もキー送出も in-process なら tmux に一切依存せず完結する
enum DialogReach<'a> {
    InProcess(&'a tako_core::TerminalSession),
    Detached(
        tako_core::backend::SessionRef,
        &'static dyn tako_core::backend::DetachedAccess,
    ),
}

impl DialogReach<'_> {
    /// 可視画面の採取
    fn capture(&self) -> Result<Vec<String>, DispatchError> {
        match self {
            Self::InProcess(session) => Ok(session.visible_lines()),
            Self::Detached(s, access) => access
                .capture_screen(s)
                .map_err(|e| DispatchError::Operation(format!("画面の取得に失敗: {e}"))),
        }
    }

    /// キー名 1 個の送出
    fn send_key(&self, key: &str) -> Result<(), DispatchError> {
        match self {
            Self::InProcess(session) => {
                let bytes = tako_core::keys::encode_key(key, session.key_encoding())
                    .ok_or_else(|| DispatchError::InvalidParams(unknown_key_message(key)))?;
                session.write(bytes);
                Ok(())
            }
            // detached はバックエンドのキー名語彙（`Enter` / `Down`）へ渡す。
            // tmux は先頭大文字の名前を取るので合わせる
            Self::Detached(s, access) => access
                .send_key(s, &detached_key_name(key))
                .map_err(|e| DispatchError::Operation(format!("キー送出に失敗: {e}"))),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::InProcess(_) => "in-process",
            Self::Detached(..) => "detached",
        }
    }
}

fn unknown_key_message(key: &str) -> String {
    format!(
        "不明なキー名 '{key}'（使えるキー: {} / ctrl-<英字> / 1 文字リテラル）",
        tako_core::keys::KEY_NAMES.join(" / ")
    )
}

/// in-process のキー名を detached バックエンド（tmux 系）の語彙へ写す。
/// 1 文字リテラルはそのまま（tmux の send-keys はリテラルを受ける）
fn detached_key_name(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter".to_string(),
        "escape" | "esc" => "Escape".to_string(),
        "tab" => "Tab".to_string(),
        "backtab" | "shift-tab" => "BTab".to_string(),
        "space" => "Space".to_string(),
        "backspace" => "BSpace".to_string(),
        "delete" | "del" => "DC".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" | "pgup" => "PageUp".to_string(),
        "pagedown" | "pgdn" => "PageDown".to_string(),
        other => {
            // 修飾前置を tmux 記法へ（ctrl-c → C-c / shift-up → S-Up / alt-v → M-v）。
            // 基底名は再帰で写す（`shift-up` の `up` を `Up` にするため）
            for (prefix, tmux) in [
                ("ctrl-", "C-"),
                ("control-", "C-"),
                ("shift-", "S-"),
                ("alt-", "M-"),
                ("option-", "M-"),
                ("meta-", "M-"),
            ] {
                if let Some(rest) = other.strip_prefix(prefix) {
                    if !rest.is_empty() {
                        return format!("{tmux}{}", detached_key_name(rest));
                    }
                }
            }
            key.to_string()
        }
    }
}

/// ダイアログ操作の到達手段を解決する。in-process 優先（#662）
fn resolve_dialog_reach(
    host: &dyn ControlHost,
    pane_id: u64,
) -> Result<DialogReach<'_>, DispatchError> {
    let target = PaneId::from_raw(pane_id);
    if let Some(session) = host.session(target) {
        return Ok(DialogReach::InProcess(session));
    }
    // tako-app が保持していない = ペイン消失 or GUI 不在。バックエンドの到達手段に頼る
    let backend_session = host.backend_session(target).ok_or_else(|| {
        DispatchError::Operation(crate::reach::UnreachableReason::NoSession(pane_id).note())
    })?;
    match crate::reach::detached_session(&backend_session) {
        Some((s, access)) => Ok(DialogReach::Detached(s, access)),
        None => Err(DispatchError::Operation(
            crate::reach::UnreachableReason::NoDetachedAccess {
                session: backend_session.clone(),
                note: crate::reach::no_detached_access_note(),
            }
            .note(),
        )),
    }
}

/// #319: permission ダイアログへの構造化応答
fn dispatch_orchestrator_respond_permission(
    host: &dyn ControlHost,
    pane_id: u64,
    choice: &str,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    // #662: in-process 優先で到達手段を解決する（旧実装は detached 固定で
    // Windows/psmux では必ず失敗していた）
    let reach = resolve_dialog_reach(host, pane_id)?;
    let lines = reach.capture()?;
    let dialog = crate::claude_tui::detect_permission_dialog(&lines).ok_or_else(|| {
        DispatchError::Operation(
            "ペイン画面に permission ダイアログが見つからない（既に解消済みか、別の画面状態です）"
                .to_string(),
        )
    })?;

    // choice を番号に解決
    let choice_num: usize = match choice.to_lowercase().as_str() {
        "yes" | "allow" => 1,
        "no" | "deny" => {
            // Deny は最後の選択肢（通常は 3 番目）
            dialog
                .options
                .iter()
                .position(|o| o.to_lowercase().contains("deny") || o.to_lowercase() == "no")
                .map(|i| i + 1)
                .unwrap_or(dialog.options.len())
        }
        n => n.parse().map_err(|_| {
            DispatchError::Operation(format!(
                "choice は番号（1-{}）または yes/no/allow/deny を指定してください: {choice}",
                dialog.options.len()
            ))
        })?,
    };
    if choice_num == 0 || choice_num > dialog.options.len() {
        return Err(DispatchError::Operation(format!(
            "choice {choice_num} は範囲外です（1-{}）",
            dialog.options.len()
        )));
    }

    // 選択キーを送信: 番号キー → 短い待ち → Enter
    reach.send_key(&choice_num.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(200));
    reach.send_key("enter")?;

    let chosen_text = dialog
        .options
        .get(choice_num - 1)
        .cloned()
        .unwrap_or_default();

    // 監査記録（persist.log。ペイン出力自体はキー入力の結果として画面に残る）
    let caller = caller_role.unwrap_or("unknown");
    crate::diag::persist_log(&format!(
        "[permission-respond] caller={caller} pane={pane_id} choice={choice_num} ({chosen_text}) command={} reach={}",
        dialog.command,
        reach.label(),
    ));

    Ok(json!({
        "pane_id": pane_id,
        "responded": true,
        "kind": "permission",
        "choice": choice_num,
        "choice_text": chosen_text,
        "command": dialog.command,
        "reach": reach.label(),
    }))
}

/// ダイアログ操作の 1 ステップに許す待ち時間（#662）。
///
/// dispatch は tako-app の UI スレッドで走るため、ここで長く眠ると画面が固まる。
/// 期待する変化が出た瞬間に進むポーリング方式にして、実測（数字キーの反映は
/// 概ね 100ms 以内）に対して十分な上限だけを置く
const DIALOG_STEP_BUDGET_MS: u64 = 800;
/// ポーリング間隔
const DIALOG_POLL_MS: u64 = 40;

/// 画面が条件を満たすまで待つ。満たさないまま予算を使い切ったら最後の画面を返す。
/// 戻り値は (画面, 条件を満たしたか)
fn wait_for_screen(
    reach: &DialogReach<'_>,
    budget_ms: u64,
    cond: impl Fn(&[String]) -> bool,
) -> Result<(Vec<String>, bool), DispatchError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
    let mut last = reach.capture()?;
    if cond(&last) {
        return Ok((last, true));
    }
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(DIALOG_POLL_MS));
        last = reach.capture()?;
        if cond(&last) {
            return Ok((last, true));
        }
    }
    Ok((last, false))
}

/// #662: worker が表示中の対話ダイアログの内容を取得する。
///
/// **保留中のダイアログの正はライブ画面**。claude は
/// ダイアログを表示している間 transcript に何も書かず、`tool_use:AskUserQuestion` は
/// 回答が確定してから `tool_result` と一緒に現れる（隔離 E2E で実測）。
/// transcript は「回答後の照会」と将来の claude 変更に備えた補助ソースとして併せて返す
fn dispatch_orchestrator_dialog(
    host: &dyn ControlHost,
    q: WorkerQuery,
) -> Result<Value, DispatchError> {
    let pane_id = q.pane_id;
    let target = PaneId::from_raw(pane_id);

    // transcript（補助ソース。回答確定後にだけ埋まる）
    let backend_session = host.backend_session(target);
    let pane_pid = host.session(target).and_then(|s| s.child_pid());
    let session_id = q
        .session_id
        .clone()
        .or_else(|| auto_resolve_session(backend_session.as_deref(), pane_pid));
    let pending = session_id
        .as_deref()
        .and_then(crate::dialog::pending_for_session);

    // ライブ画面（第二ソース。現在位置・ハイライト・回答済みマーカー）。
    // ペインへ届かない場合も transcript 側は返せるので、ここは失敗を許容する
    let (screen_lines, reach_label) = match resolve_dialog_reach(host, pane_id) {
        Ok(reach) => {
            let label = reach.label();
            (reach.capture().ok(), Some(label))
        }
        Err(_) => (None, None),
    };
    let dialog_screen = screen_lines
        .as_deref()
        .and_then(crate::dialog::parse_dialog_screen);
    let permission = screen_lines.as_deref().and_then(|lines| {
        let d = crate::claude_tui::detect_permission_dialog(lines)?;
        Some(json!({
            "command": d.command,
            "options": d.options,
            "highlighted": d.highlighted,
        }))
    });

    // 種別の判定: AskUserQuestion（画面にダイアログが見えている or transcript が保留中）
    // > permission ダイアログ > なし
    let kind = if dialog_screen.is_some() || pending.is_some() {
        "ask_user_question"
    } else if permission.is_some() {
        "permission"
    } else {
        "none"
    };

    Ok(json!({
        "pane_id": pane_id,
        "kind": kind,
        "session_id": session_id,
        "reach": reach_label,
        // transcript 由来の全文（幅に依存しない。master はこれを読んで判断する）
        "questions": pending.as_ref().map(|p| p.to_json()),
        // ライブ画面由来の現在位置
        "screen": dialog_screen.as_ref().map(|s| s.to_json()),
        "permission_dialog": permission,
    }))
}

/// #662: AskUserQuestion への構造化応答。
///
/// 実測した TUI 操作モデル（`crate::dialog` の doc）に沿って
/// 「番号キー → 画面で反映を確認 → 次の質問」を繰り返し、確認画面で
/// **選んだ内容が意図と一致することを検証してから** submit する
fn dispatch_orchestrator_respond_dialog(
    host: &dyn ControlHost,
    pane_id: u64,
    answers: &[crate::protocol::DialogAnswer],
    dry_run: bool,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::dialog;

    let reach = resolve_dialog_reach(host, pane_id)?;

    // ダイアログが実際に表示されていることを確認（誤爆防止。#319 から維持）
    let lines = reach.capture()?;
    let mut current = dialog::parse_dialog_screen(&lines).ok_or_else(|| {
        DispatchError::Operation(
            "ペイン画面に対話ダイアログ（AskUserQuestion）が見つからない\
             （既に回答済みか、別の画面状態です）"
                .to_string(),
        )
    })?;

    let specs: Vec<dialog::AnswerSpec> = answers
        .iter()
        .map(|a| dialog::AnswerSpec {
            question: a.question.clone(),
            options: a.option_list(),
        })
        .collect();

    // 全問ぶんの回答が揃っているかは**タブの数**で先に確かめる
    // （画面は 1 問ずつしか映さないので、選択肢の解決はその質問が出てから）
    let assigned =
        dialog::assign_answers(&current.tabs, &specs).map_err(DispatchError::InvalidParams)?;

    // 選んだラベル（確認画面との照合と応答に使う）
    let mut chosen: Vec<Value> = Vec::new();
    let mut chosen_labels: Vec<String> = Vec::new();

    // 表示中の質問から順に答える。TUI は先頭から前進するので、
    // 現在位置より前の質問には戻れない（＝既に回答済みのはず）
    while current.stage == dialog::DialogStage::Question {
        let Some(qi) = current.current_question_index() else {
            break;
        };
        let Some((_, spec)) = assigned.iter().find(|(q, _)| *q == qi) else {
            return Err(DispatchError::InvalidParams(format!(
                "質問 {} への回答が指定されていない",
                qi + 1
            )));
        };
        // **今映っている画面**に対して選択肢を解決する（#662 実測: transcript は
        // ダイアログ表示中には書かれないので、内容の正は画面しかない）
        let (keys, labels) =
            dialog::keys_for_screen_answer(&current, spec).map_err(DispatchError::InvalidParams)?;
        let header = current
            .tabs
            .get(qi)
            .map(|t| t.header.clone())
            .unwrap_or_default();
        chosen.push(json!({
            "index": qi + 1,
            "header": header,
            "question": current.question,
            "labels": labels,
        }));
        chosen_labels.extend(labels);

        for key in &keys {
            reach.send_key(key)?;
            // 各キーの反映を待つ。multiSelect のトグルは同一画面なので
            // 「画面が変わった」だけでは判定できず、短い固定待ちで足りる
            std::thread::sleep(std::time::Duration::from_millis(DIALOG_POLL_MS));
        }
        // この質問が回答済みになったこと（タブが ☒ / 確認画面へ遷移）を待つ
        let (after, ok) = wait_for_screen(&reach, DIALOG_STEP_BUDGET_MS, |lines| {
            match dialog::parse_dialog_screen(lines) {
                Some(s) => {
                    s.stage == dialog::DialogStage::Review
                        || s.tabs.get(qi).is_some_and(|t| t.answered)
                }
                // ダイアログが消えた = 想定外（Esc 等）。待ち続けない
                None => true,
            }
        })?;
        if !ok {
            return Err(DispatchError::Operation(format!(
                "質問 {} への選択が画面に反映されない（送ったキー: {}）。\
                 tako_orchestrator_dialog で状態を確認すること",
                qi + 1,
                keys.join(" ")
            )));
        }
        current = dialog::parse_dialog_screen(&after).ok_or_else(|| {
            DispatchError::Operation(
                "回答中にダイアログが消えた（取消されたか、別の画面へ遷移した）".to_string(),
            )
        })?;
    }

    // 確認画面へ遷移するのを待つ（単一選択の最終問は自動遷移、multiSelect は Tab 済み）
    if current.stage != dialog::DialogStage::Review {
        let (after, _) = wait_for_screen(&reach, DIALOG_STEP_BUDGET_MS, |lines| {
            dialog::parse_dialog_screen(lines)
                .is_some_and(|s| s.stage == dialog::DialogStage::Review)
        })?;
        if let Some(s) = dialog::parse_dialog_screen(&after) {
            current = s;
        }
    }

    // **送信前の検証**: 確認画面に写っている選択結果が意図と一致するか。
    // 一致しなければ submit しない（撃ちっぱなしにしないための要）。
    //
    // この呼び出しで 1 問も答えていない = 呼び出し時点で既に確認画面だった
    // （前回の dry_run 等）。そのときは「自分が選んだラベル」を持っていないので、
    // 検証を呼び出し側の指定と画面テキストの突き合わせへ切り替える。
    // 空の `chosen_labels` で `verify_review` を通すと**検証が素通り**になり、
    // 画面に出ている別の回答をそのまま確定させてしまう
    let verified = if chosen_labels.is_empty() {
        dialog::verify_specs_against_review(&specs, &current)
    } else {
        dialog::verify_review(&chosen_labels, &current)
    };
    if let Err(e) = verified {
        return Err(DispatchError::Operation(format!(
            "確認画面の内容が指定と一致しないため送信しませんでした: {e}"
        )));
    }

    // 既に確認画面だった場合、応答の chosen は画面から埋める（空を返さない）
    if chosen.is_empty() {
        chosen = current
            .review_answers
            .iter()
            .map(|a| json!({ "labels": [a] }))
            .collect();
    }

    if dry_run {
        return Ok(json!({
            "pane_id": pane_id,
            "kind": "ask_user_question",
            "responded": false,
            "dry_run": true,
            "stage": "review",
            "chosen": chosen,
            "review_answers": current.review_answers,
            "reach": reach.label(),
        }));
    }

    // 送信
    for key in dialog::keys_for_submit(&current) {
        reach.send_key(&key)?;
    }
    // ダイアログが消えたことを確認（送達の確認。消えなければ未送信の可能性を返す）
    let (_, gone) = wait_for_screen(&reach, DIALOG_STEP_BUDGET_MS, |lines| {
        dialog::parse_dialog_screen(lines).is_none()
    })?;

    let caller = caller_role.unwrap_or("unknown");
    crate::diag::persist_log(&format!(
        "[dialog-respond] caller={caller} pane={pane_id} answers={} submitted={gone} reach={}",
        chosen.len(),
        reach.label(),
    ));

    Ok(json!({
        "pane_id": pane_id,
        "kind": "ask_user_question",
        "responded": true,
        "submitted": gone,
        "chosen": chosen,
        "review_answers": current.review_answers,
        "reach": reach.label(),
    }))
}

/// #319 / #662: ダイアログへの応答。`answers` があれば AskUserQuestion、
/// `choice` だけなら permission ダイアログへ振り分ける
fn dispatch_orchestrator_respond(
    host: &dyn ControlHost,
    pane_id: u64,
    choice: Option<&str>,
    answers: Option<&[crate::protocol::DialogAnswer]>,
    dry_run: bool,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    match (answers, choice) {
        // 両方指定は「どのダイアログに答えたいのか」が決まらない。
        // 黙ってどちらかを選ぶと誤爆になるので拒否する
        (Some(_), Some(_)) => Err(DispatchError::InvalidParams(
            "choice と answers は同時に指定できない（choice = permission ダイアログ / \
             answers = AskUserQuestion）。tako_orchestrator_dialog の kind で\
             どちらが出ているか確認すること"
                .into(),
        )),
        (Some(answers), None) => {
            if answers.is_empty() {
                return Err(DispatchError::InvalidParams(
                    "answers が空。質問ごとに 1 要素を指定すること".into(),
                ));
            }
            dispatch_orchestrator_respond_dialog(host, pane_id, answers, dry_run, caller_role)
        }
        (None, Some(choice)) => {
            dispatch_orchestrator_respond_permission(host, pane_id, choice, caller_role)
        }
        (None, None) => Err(DispatchError::InvalidParams(
            "choice（permission ダイアログ）または answers（AskUserQuestion）が必要".into(),
        )),
    }
}

/// #662: ペインへの特殊キー送出。送達確認ループ（PromptFlow）を通らない
fn dispatch_send_keys(
    host: &dyn ControlHost,
    pane: Option<u64>,
    keys: &[String],
    delay_ms: Option<u64>,
    tmux_session: Option<&str>,
) -> Result<Value, DispatchError> {
    if keys.is_empty() {
        return Err(DispatchError::InvalidParams("keys が空".into()));
    }
    // **キー名の妥当性はペインの状態より先に見る**。壊れた指定は届く / 届かないに
    // 関係なく InvalidParams であり、先に見ておけば「途中まで撃ってから失敗」も防げる。
    // 名前が有効かどうかは端末モードに依存しないので、ここは既定の符号化で判定してよい
    for key in keys {
        if tako_core::keys::encode_key(key, Default::default()).is_none() {
            return Err(DispatchError::InvalidParams(unknown_key_message(key)));
        }
    }

    // 到達手段の解決。in-process 優先（`PaneReach` と同じ規律）
    let reach = match crate::reach::PaneReach::resolve(host, pane, tmux_session) {
        crate::reach::PaneReach::InProcess(target) => {
            let session = host
                .session(target)
                .ok_or(DispatchError::NoSession(target.as_u64()))?;
            DialogReach::InProcess(session)
        }
        crate::reach::PaneReach::Detached(s, access) => DialogReach::Detached(s, access),
        crate::reach::PaneReach::Unreachable(reason) => {
            return Err(DispatchError::Operation(reason.note()))
        }
    };

    let delay = delay_ms.unwrap_or(30).min(2000);
    for (i, key) in keys.iter().enumerate() {
        if i > 0 && delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        reach.send_key(key)?;
    }

    Ok(json!({
        "sent": keys.len(),
        "keys": keys,
        "reach": reach.label(),
    }))
}

/// worker が busy かどうかを画面出力で判定する。
/// false negative より false positive を優先（殺すより残す方が安全）。
/// 判定は orchestrator::wait の完了監視ヒューリスティックと共通（#83）
fn is_worker_busy(host: &dyn ControlHost, target: PaneId) -> bool {
    let Some(session) = host.session(target) else {
        return true; // 画面取得不可 = busy 寄りに倒す
    };
    !crate::orchestrator::wait::screen_looks_idle(&session.visible_lines().join("\n"))
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `setup_mcp` の結果
pub struct SetupMcpResult {
    pub configured: bool,
    pub already_existed: bool,
    /// 既存の登録パスが死んでいたため付け替えた
    pub repaired: bool,
    /// 修復前の旧パス（repaired=true のときのみ）
    pub old_command: Option<String>,
    /// 書き込み先ファイルパス
    pub target_path: std::path::PathBuf,
    /// 旧 settings.json の誤設定を掃除した
    pub legacy_cleaned: bool,
}

/// MCP 設定のスコープ
pub enum McpScope {
    /// ユーザーグローバル（~/.claude.json 相当）
    User,
    /// プロジェクト単位（cwd/.mcp.json）
    Project(std::path::PathBuf),
}

/// Claude Code に tako MCP サーバーの接続設定を登録する。
///
/// 1. `claude` CLI があれば `claude mcp add` を使う（公式経路）
/// 2. なければ設定ファイルを直接マージ編集（user → ~/.claude.json、project → cwd/.mcp.json）
/// 3. 旧バージョンが残した ~/.claude/settings.json の mcpServers.tako を検出・掃除
pub fn setup_mcp(tako_binary: &str, scope: &McpScope) -> Result<SetupMcpResult, DispatchError> {
    let existing = read_mcp_registration(scope);
    if let Some(ref cmd) = existing {
        if !cmd.is_empty() && std::path::Path::new(cmd).is_file() {
            let legacy_cleaned = clean_legacy_settings_json();
            return Ok(SetupMcpResult {
                configured: false,
                already_existed: true,
                repaired: false,
                old_command: None,
                target_path: mcp_target_path(scope),
                legacy_cleaned,
            });
        }
    }

    let old_command = existing.filter(|c| !c.is_empty());
    let repaired = old_command.is_some();

    let claude_bin = which_claude();
    if let Some(ref claude) = claude_bin {
        setup_mcp_via_cli(claude, tako_binary, scope)?;
    } else {
        setup_mcp_direct(tako_binary, scope)?;
    }

    let legacy_cleaned = clean_legacy_settings_json();

    Ok(SetupMcpResult {
        configured: true,
        already_existed: repaired,
        repaired,
        old_command,
        target_path: mcp_target_path(scope),
        legacy_cleaned,
    })
}

fn mcp_target_path(scope: &McpScope) -> std::path::PathBuf {
    match scope {
        McpScope::User => home_dir()
            .map(|h| h.join(".claude.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("~/.claude.json")),
        McpScope::Project(cwd) => cwd.join(".mcp.json"),
    }
}

fn read_mcp_registration(scope: &McpScope) -> Option<String> {
    let path = mcp_target_path(scope);
    let content = std::fs::read_to_string(&path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&content).ok()?;
    data.get("mcpServers")?
        .get("tako")?
        .get("command")?
        .as_str()
        .map(String::from)
}

/// claude CLI のパスを検出
fn which_claude() -> Option<String> {
    which("claude")
}

fn setup_mcp_via_cli(
    claude_bin: &str,
    tako_binary: &str,
    scope: &McpScope,
) -> Result<(), DispatchError> {
    let scope_arg = match scope {
        McpScope::User => "user",
        McpScope::Project(_) => "project",
    };

    // 既存の登録を先に除去（claude mcp add は上書きを許さないため）
    let mut rm = std::process::Command::new(claude_bin);
    // #628: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut rm);
    rm.args(["mcp", "remove", "--scope", scope_arg, "tako"]);
    if let McpScope::Project(cwd) = scope {
        rm.current_dir(cwd);
    }
    let _ = rm.output(); // 未登録なら失敗するが無視

    let mut cmd = std::process::Command::new(claude_bin);
    tako_core::platform::process::no_console_window(&mut cmd);
    cmd.args([
        "mcp",
        "add",
        "--scope",
        scope_arg,
        "--transport",
        "stdio",
        "tako",
        "--",
    ]);
    cmd.arg(tako_binary);
    cmd.args(["mcp", "serve"]);
    if let McpScope::Project(cwd) = scope {
        cmd.current_dir(cwd);
    }
    let output = cmd
        .output()
        .map_err(|e| DispatchError::Operation(format!("claude mcp add の実行に失敗: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DispatchError::Operation(format!(
            "claude mcp add が失敗 (exit {}): {stderr}",
            output.status
        )));
    }
    Ok(())
}

fn setup_mcp_direct(tako_binary: &str, scope: &McpScope) -> Result<(), DispatchError> {
    let path = mcp_target_path(scope);
    let mut data: serde_json::Map<String, Value> = if path.is_file() {
        let content = std::fs::read_to_string(&path).map_err(|e| {
            DispatchError::Operation(format!("{} の読み取りに失敗: {e}", path.display()))
        })?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    let servers = data.entry("mcpServers").or_insert_with(|| json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| DispatchError::Operation("mcpServers がオブジェクトでない".into()))?;
    servers_obj.insert(
        "tako".to_string(),
        json!({
            "type": "stdio",
            "command": tako_binary,
            "args": ["mcp", "serve"],
            "env": {},
        }),
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            DispatchError::Operation(format!("{} の作成に失敗: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| DispatchError::Operation(format!("JSON のシリアライズに失敗: {e}")))?;
    std::fs::write(&path, json).map_err(|e| {
        DispatchError::Operation(format!("{} への書き込みに失敗: {e}", path.display()))
    })?;
    Ok(())
}

/// 旧バージョンが ~/.claude/settings.json の mcpServers.tako に残した誤設定を掃除する。
/// 掃除した場合 true を返す。
pub fn clean_legacy_settings_json() -> bool {
    let Some(home) = home_dir() else {
        return false;
    };
    let path = home.join(".claude").join("settings.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut settings: serde_json::Map<String, Value> = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let removed = if let Some(servers) = settings.get_mut("mcpServers") {
        if let Some(obj) = servers.as_object_mut() {
            obj.remove("tako").is_some()
        } else {
            false
        }
    } else {
        false
    };
    if !removed {
        return false;
    }
    if let Some(servers) = settings.get("mcpServers") {
        if servers.as_object().is_some_and(|o| o.is_empty()) {
            settings.remove("mcpServers");
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(&path, json);
    }
    true
}

/// MCP 登録に使う安定パス。/Applications/tako.app がある場合に最優先
pub const STABLE_APP_BINARY: &str = "/Applications/tako.app/Contents/MacOS/tako";

/// tako CLI バイナリのパスを解決する。
/// ① /Applications/tako.app（安定パス）
/// ② `which tako`
/// ③ 実行中バイナリの隣（.app バンドル想定）
/// ④ フォールバック "tako"
pub fn resolve_tako_binary() -> String {
    if std::path::Path::new(STABLE_APP_BINARY).is_file() {
        return STABLE_APP_BINARY.to_string();
    }
    if let Some(path) = which("tako") {
        return path;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Windows は `tako.exe`（EXE_SUFFIX で吸収）
            let sibling = dir.join(format!("tako{}", std::env::consts::EXE_SUFFIX));
            if sibling.is_file() {
                return sibling.display().to_string();
            }
        }
    }
    "tako".to_string()
}

/// dispatch / MCP の回答 JSON を CLI の非対話 stdin 経路へ渡す。
/// 回答本文を argv に含めず、プロセス一覧や診断情報へ露出させない。
fn run_setup_cli(tako_bin: &str, answers_json: &str) -> Result<Value, DispatchError> {
    use std::io::Write as _;

    // #628: GUI プロセスから到達するのでコンソールウィンドウを出させない
    let mut child =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new(tako_bin))
            .args(["setup", "--yes", "--answers", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| DispatchError::Operation(format!("tako setup の起動に失敗: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| DispatchError::Operation("tako setup の標準入力を開けない".into()))?
        .write_all(answers_json.as_bytes())
        .map_err(|e| DispatchError::Operation(format!("setup answers の送信に失敗: {e}")))?;
    let output = child
        .wait_with_output()
        .map_err(|e| DispatchError::Operation(format!("tako setup の完了待ちに失敗: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(DispatchError::Operation(format!(
            "tako setup が失敗しました (exit={}): {}",
            output.status.code().unwrap_or(-1),
            if stderr.is_empty() { &stdout } else { &stderr }
        )));
    }
    Ok(serde_json::json!({
        "completed": true,
        "output": if stderr.is_empty() { stdout } else { stderr },
    }))
}

fn check_health(host: &dyn ControlHost) -> Value {
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let mut issues: Vec<Value> = Vec::new();

    // tako CLI が PATH に通っているか
    let cli_path = which("tako");
    let cli_in_path = cli_path.is_some();
    if !cli_in_path {
        issues.push(json!({
            "level": "error",
            "check": "cli_in_path",
            "message": "tako CLI が PATH に見つからない。.app バンドル内の CLI を PATH に追加するか、\
                scripts/build-app.sh --install でインストールすること",
        }));
    }

    // CLI バージョンとアプリバージョンの一致
    let cli_version = cli_path
        .as_ref()
        .and_then(|path| {
            // #586: GUI プロセス経由の診断でもコンソールウィンドウを出させない
            tako_core::platform::process::no_console_window(&mut std::process::Command::new(path))
                .arg("--version")
                .output()
                .ok()
        })
        .and_then(|out| {
            String::from_utf8(out.stdout)
                .ok()
                .and_then(|s| s.split_whitespace().last().map(|v| v.to_string()))
        });
    let version_match = cli_version.as_deref() == Some(&app_version);
    if cli_in_path && !version_match {
        issues.push(json!({
            "level": "warning",
            "check": "version_match",
            "message": format!(
                "CLI バージョン ({}) とアプリバージョン ({}) が不一致。\
                 build-app.sh --install で最新の CLI をインストールすること",
                cli_version.as_deref().unwrap_or("不明"),
                app_version,
            ),
        }));
    }

    // tmux の有無
    let tmux_available = which("tmux").is_some();
    if !tmux_available {
        issues.push(json!({
            "level": "warning",
            "check": "tmux",
            "message": "tmux がインストールされていない。タブ構成の保存・復元は機能するが、\
                実行中プロセス・画面内容の復元（完全復元）は使えない。\
                brew install tmux でインストール可能",
        }));
    }

    // セッション永続化の状態
    let persist_enabled = host.tmux_persist_enabled();
    let persist_available = tako_core::backend::capabilities().survives_app_exit;
    if tmux_available && !persist_enabled {
        issues.push(json!({
            "level": "info",
            "check": "persist",
            "message": "セッション永続化が無効。tako persist on で有効にすると、\
                tako 再起動時にプロセスと画面内容が復元される",
        }));
    }

    // ワークスペースの状態サマリ
    let ws = host.workspace();
    let tab_count = ws.tabs().len();
    let pane_count: usize = ws.tabs().iter().map(|t| t.tree().len()).sum();
    let bg_count = ws.shelved_panes().len();

    let healthy = issues.is_empty();

    json!({
        "healthy": healthy,
        "app_version": app_version,
        "cli_version": cli_version,
        "cli_in_path": cli_in_path,
        "version_match": version_match,
        "tmux_available": tmux_available,
        "persist_enabled": persist_enabled,
        "persist_available": persist_available,
        "workspace": {
            "tabs": tab_count,
            "panes": pane_count,
            "backgrounded": bg_count,
        },
        "issues": issues,
    })
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// コマンドを探す。探索の作法は抽象境界 B16 が持つ。
/// `which` は Windows に無いため、直呼びすると MCP 自動登録が丸ごと失敗する（#525）
fn which(name: &str) -> Option<String> {
    tako_core::platform::exe::find(name)
}

/// 対象ペインが動画プレビューであることを確かめる（#484）
fn require_video_preview(
    host: &(impl ControlHost + ?Sized),
    target: PaneId,
) -> Result<(), DispatchError> {
    if host.preview_state(target).map(|(_, m)| m) != Some(PreviewModeWire::Video) {
        return Err(DispatchError::Operation(
            "対象ペインは動画プレビューではない".into(),
        ));
    }
    Ok(())
}

/// 動画操作の共通応答。UI のシークバー・時刻表示と同じ値を返し、
/// CLI / MCP と UI の一致を観測できるようにする（#484）
fn video_response(host: &(impl ControlHost + ?Sized), target: PaneId) -> serde_json::Value {
    let mut resp = json!({ "pane": target.as_u64(), "started": false });
    if let Ok(status) = host.video_status(target) {
        resp["started"] = json!(true);
        resp["state"] = json!(status.state);
        resp["position"] = json!(status.position);
        resp["duration"] = json!(status.duration);
        resp["rate"] = json!(status.rate);
        resp["volume"] = json!(status.volume);
        resp["muted"] = json!(status.muted);
        resp["looping"] = json!(status.looping);
        resp["ended"] = json!(status.ended);
    }
    resp
}

/// `pane` 省略はエラー（呼び出し元解決はクライアント側の責務。FR-2.2.7）
pub(crate) fn resolve_pane(
    ws: &Workspace,
    pane: Option<u64>,
) -> Result<(TabId, PaneId), DispatchError> {
    let raw = pane.ok_or(DispatchError::NoTargetPane)?;
    for tab in ws.tabs() {
        if let Some(p) = tab.tree().panes().iter().find(|p| p.id().as_u64() == raw) {
            return Ok((tab.id(), p.id()));
        }
    }
    Err(DispatchError::PaneNotFound(raw))
}

fn find_tab(ws: &Workspace, raw: u64) -> Result<TabId, DispatchError> {
    ws.tabs()
        .iter()
        .map(|t| t.id())
        .find(|t| t.as_u64() == raw)
        .ok_or(DispatchError::TabNotFound(raw))
}

fn find_window(ws: &Workspace, raw: u64) -> Result<tako_core::WindowId, DispatchError> {
    ws.windows()
        .iter()
        .map(|w| w.id())
        .find(|w| w.as_u64() == raw)
        .ok_or_else(|| DispatchError::Operation(format!("ウィンドウ {raw} が見つからない")))
}

/// ウィンドウの最小化 / 最大化 / 復元（Issue #584）。`window` 省略でアクティブウィンドウ。
/// 実適用は GPUI の Context を持つ UI 層（`request_window_state`）に委ねる
fn window_state_op(
    host: &mut dyn ControlHost,
    window: Option<u64>,
    op: crate::protocol::WindowStateOp,
) -> Result<Value, DispatchError> {
    let wid = match window {
        Some(raw) => find_window(host.workspace(), raw)?,
        None => host.workspace().active_window_id(),
    };
    host.request_window_state(wid, op);
    Ok(json!({ "window": wid.as_u64(), "state": op.as_str() }))
}

/// ウィンドウ一覧（Issue #339）。`WindowList` 応答と `list` の windows フィールドで共用
fn windows_json(ws: &Workspace) -> Value {
    json!({
        "active_window": ws.active_window_id().as_u64(),
        "windows": ws.windows().iter().map(|w| json!({
            "id": w.id().as_u64(),
            "active": w.id() == ws.active_window_id(),
            "active_tab": w.active_tab().as_u64(),
            "tabs": ws.window_tab_ids(w.id()).iter().map(|t| t.as_u64()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

// --- メニューバー（Issue #657）------------------------------------------------

/// メニューバーのスナップショットを JSON へ（Issue #657）
fn menu_bar_json(snapshot: &crate::protocol::MenuBarSnapshot) -> Value {
    json!({
        "in_window": snapshot.in_window,
        "open": snapshot.open,
        "menus": snapshot.menus.iter().map(|menu| json!({
            "name": menu.name,
            "items": menu.items.iter().map(menu_item_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn menu_item_json(item: &crate::protocol::MenuItemSnapshot) -> Value {
    use crate::protocol::MenuItemSnapshot as I;
    match item {
        I::Separator => json!({ "kind": "separator" }),
        I::Action {
            label,
            action,
            shortcut,
        } => json!({
            "kind": "action",
            "label": label,
            "action": action,
            "shortcut": shortcut,
        }),
        I::Submenu { label, items } => json!({
            "kind": "submenu",
            "label": label,
            "items": items.iter().map(menu_item_json).collect::<Vec<_>>(),
        }),
    }
}

/// in-window メニューバーを持たない環境（macOS）で open / close を拒否する。
///
/// **「そんな機能は無い」ではなく理由を返す**（対応マトリクスの方針と同じ）。
/// macOS でもメニュー自体は存在するので `list` と `invoke` は動く
fn require_in_window_menu(
    snapshot: &crate::protocol::MenuBarSnapshot,
) -> Result<(), DispatchError> {
    if snapshot.in_window {
        return Ok(());
    }
    Err(DispatchError::Operation(
        "この環境の menu open / close は使えません（メニューは OS のメニューバーに載るため \
         tako が開閉できない）。項目の実行は menu invoke が使えます"
            .into(),
    ))
}

/// メニュー名を添字へ解決する（完全一致 → 前方一致 → 部分一致。大小文字は無視）。
///
/// 曖昧なときは候補を並べて拒否する（黙って先頭を採らない）
fn resolve_menu_index(
    snapshot: &crate::protocol::MenuBarSnapshot,
    query: &str,
) -> Result<usize, DispatchError> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Err(DispatchError::InvalidParams("メニュー名が空".into()));
    }
    // 添字での直接指定も許す（`menu open 0`）
    if let Ok(index) = q.parse::<usize>() {
        if index < snapshot.menus.len() {
            return Ok(index);
        }
    }
    let names: Vec<String> = snapshot
        .menus
        .iter()
        .map(|m| m.name.to_lowercase())
        .collect();
    for matcher in [
        |name: &str, q: &str| name == q,
        |name: &str, q: &str| name.starts_with(q),
        |name: &str, q: &str| name.contains(q),
    ] {
        let hits: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, name)| matcher(name, &q))
            .map(|(i, _)| i)
            .collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(hits[0]),
            _ => {
                let candidates: Vec<&str> = hits
                    .iter()
                    .map(|i| snapshot.menus[*i].name.as_str())
                    .collect();
                return Err(DispatchError::InvalidParams(format!(
                    "メニュー名 '{query}' が曖昧です: {}",
                    candidates.join(" / ")
                )));
            }
        }
    }
    let all: Vec<&str> = snapshot.menus.iter().map(|m| m.name.as_str()).collect();
    Err(DispatchError::InvalidParams(format!(
        "メニュー '{query}' が見つかりません（候補: {}）",
        all.join(" / ")
    )))
}

/// `resolve_menu_item` の戻り
pub(crate) struct MenuHit {
    /// 解決したフルパス（`ファイル/新規タブ`）
    pub path: String,
    /// アクション名（`tako::NewTab`）
    pub action: String,
    pub shortcut: Option<String>,
}

/// メニュー項目のパスを解決する（Issue #657）。
///
/// `path` は `/` 区切りで「メニュー/項目」「メニュー/サブメニュー/項目」または
/// 項目名のみ（全メニュー横断）。各段の照合は `resolve_menu_index` と同じ
/// 完全 → 前方 → 部分の順で、曖昧なら候補を並べて拒否する
fn resolve_menu_item(
    snapshot: &crate::protocol::MenuBarSnapshot,
    path: &str,
) -> Result<MenuHit, DispatchError> {
    use crate::protocol::MenuItemSnapshot as I;

    if snapshot.menus.is_empty() {
        return Err(DispatchError::Operation(
            "メニュー定義がありません（GUI が起動していない可能性があります）".into(),
        ));
    }
    let segments: Vec<&str> = path
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(DispatchError::InvalidParams("項目のパスが空".into()));
    }

    // 「メニュー名を省略した項目名だけ」の指定を全メニュー横断で拾う。
    // 平坦化した候補（フルパス付き）を作って 1 段目と同じ照合をかける
    let mut flat: Vec<(String, &str, &String, &Option<String>)> = Vec::new();
    for menu in &snapshot.menus {
        for item in &menu.items {
            match item {
                I::Action {
                    label,
                    action,
                    shortcut,
                } => flat.push((
                    format!("{}/{}", menu.name, label),
                    label.as_str(),
                    action,
                    shortcut,
                )),
                I::Submenu { label, items } => {
                    for child in items {
                        if let I::Action {
                            label: child_label,
                            action,
                            shortcut,
                        } = child
                        {
                            flat.push((
                                format!("{}/{}/{}", menu.name, label, child_label),
                                child_label.as_str(),
                                action,
                                shortcut,
                            ));
                        }
                    }
                }
                I::Separator => {}
            }
        }
    }

    // 指定が 2 段以上ならフルパスの末尾一致で絞る（`ファイル/新規タブ`）。
    // 1 段なら項目ラベルだけで探す
    let needle = segments.join("/").to_lowercase();
    let last = segments[segments.len() - 1].to_lowercase();
    let multi = segments.len() > 1;

    for stage in 0..3 {
        let hits: Vec<usize> = flat
            .iter()
            .enumerate()
            .filter(|(_, (full, label, _, _))| {
                let haystack = if multi {
                    full.to_lowercase()
                } else {
                    label.to_lowercase()
                };
                let target = if multi { &needle } else { &last };
                match stage {
                    0 => haystack == *target,
                    1 => haystack.ends_with(target) || haystack.starts_with(target),
                    _ => haystack.contains(target),
                }
            })
            .map(|(i, _)| i)
            .collect();
        match hits.len() {
            0 => continue,
            1 => {
                let (full, _, action, shortcut) = &flat[hits[0]];
                return Ok(MenuHit {
                    path: full.clone(),
                    action: (*action).clone(),
                    shortcut: (*shortcut).clone(),
                });
            }
            _ => {
                let candidates: Vec<&str> =
                    hits.iter().map(|i| flat[*i].0.as_str()).take(8).collect();
                return Err(DispatchError::InvalidParams(format!(
                    "項目 '{path}' が曖昧です: {}",
                    candidates.join(" / ")
                )));
            }
        }
    }
    Err(DispatchError::InvalidParams(format!(
        "メニュー項目 '{path}' が見つかりません（`tako menu list` で一覧できます）"
    )))
}

fn tree_mut(ws: &mut Workspace, tab: TabId) -> &mut tako_core::PaneTree {
    ws.get_tab_mut(tab)
        .expect("呼び出し前に存在確認済みのタブ")
        .tree_mut()
}

fn op_err(e: impl std::fmt::Display) -> DispatchError {
    DispatchError::Operation(e.to_string())
}

fn validate_name(name: &str) -> Result<(), DispatchError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(DispatchError::InvalidParams("無効なファイル名".into()));
    }
    Ok(())
}

fn dir_of(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

/// ワークスペース全体の構造化スナップショット（FR-2.5.1〜2）。
/// ツリー構造 + 単位矩形ジオメトリ + 各ペインの状態を返す
fn list_json(host: &dyn ControlHost) -> Value {
    let ws = host.workspace();
    // いずれかのウィンドウで表示中のタブ集合（Issue #339。surface 判定に使う）
    let displayed: std::collections::HashSet<TabId> =
        ws.windows().iter().map(|w| w.active_tab()).collect();
    let tabs: Vec<Value> = ws
        .tabs()
        .iter()
        .map(|tab| {
            let tree = tab.tree();
            let rects = tree.layout(Rect::UNIT);
            // 前面表示中か裏で動いているか（FR-2.16.12）。tako は表示タブの全ペインを
            // タイル表示するので、表示中 = いずれかのウィンドウの表示タブ所属（Issue #339）
            let tab_active = displayed.contains(&tab.id());
            let panes: Vec<Value> = tree
                .panes()
                .iter()
                .map(|p| {
                    let rect = rects
                        .iter()
                        .find(|(id, _)| *id == p.id())
                        .map(|(_, r)| *r)
                        .expect("panes と layout は同じツリー由来");
                    let session = host.session(p.id());
                    json!({
                        "id": p.id().as_u64(),
                        // 表示分類（FR-2.16.12）。foreground = 前面表示中 / background = 裏で実行中
                        "surface": if tab_active { "foreground" } else { "background" },
                        "title": p.title(),
                        // title の出どころ（FR-2.12.3。manual は自動リネームに上書きされない）
                        "title_source": title_source_str(p.title_source()),
                        "osc_title": session.and_then(|s| s.title()),
                        "role": p.role(),
                        "spawned_by": p.spawned_by().map(|id| id.as_u64()),
                        "origin": origin_str(p.origin()),
                        "focused": p.id() == tree.focused(),
                        // OSC 7 / 133 シェル統合由来（未検知なら null / "unknown"。FR-2.1.4）
                        "cwd": session.and_then(|s| s.cwd()).map(|p| p.display().to_string()),
                        "state": session.map(|s| command_state_str(s.command_state())),
                        "exit_code": session.and_then(|s| match s.command_state() {
                            tako_core::CommandState::Failed(code) => Some(code),
                            _ => None,
                        }),
                        // ペイン配下プロセスの listen 中 TCP ポート（FR-2.4.2。
                        // tty 突き合わせのポーリング検知。未対応環境では空配列）
                        "listen_ports": session.map(|s| s.listen_ports().iter().map(|p| json!({
                            "port": p.port,
                            "pid": p.pid,
                            "process": p.process,
                        })).collect::<Vec<_>>()),
                        "rect": {
                            "x": rect.x,
                            "y": rect.y,
                            "width": rect.width,
                            "height": rect.height,
                        },
                        "cols": session.map(|s| s.size().0),
                        "rows": session.map(|s| s.size().1),
                        // スクロールバック表示の状態（FR-2.5.13。alt_screen 中は無効）
                        "scroll": session.map(|s| json!({
                            "offset": s.display_offset(),
                            "history": s.history_size(),
                            "alt_screen": s.is_alt_screen(),
                        })),
                        // プレビューペイン（FR-3.2 / FR-3.3）。ターミナルペインでは null
                        "preview": host.preview_state(p.id()).map(|(path, mode)| {
                            let (editing, dirty) =
                                host.preview_edit_state(p.id()).unwrap_or((false, false));
                            json!({
                                "path": path,
                                "mode": mode.as_str(),
                                "editing": editing,
                                "dirty": dirty,
                            })
                        }),
                        "tmux_session": host.backend_session(p.id()),
                        "backend_windows": host.backend_windows(p.id()).map(|ws| ws.iter().map(|w| json!({
                            "index": w.index,
                            "name": w.name,
                            "active": w.active,
                            "panes": w.panes,
                        })).collect::<Vec<_>>()),
                    })
                })
                .collect();
            json!({
                "id": tab.id().as_u64(),
                "title": tab.title(),
                "title_source": title_source_str(tab.title_source()),
                "active": tab.id() == ws.active_tab_id(),
                // 所属ウィンドウ（Issue #339。後方互換: 単一ウィンドウでは常に同じ値）
                "window": ws.window_of_tab(tab.id()).map(|w| w.as_u64()),
                // サイドバー tmux ビューでこのタブ枠が折りたたまれているか（FR-2.16.14）
                "collapsed": host.tmux_tab_collapsed(tab.id()),
                "focused_pane": tree.focused().as_u64(),
                "panes": panes,
                "tree": tree_json(tree.root()),
            })
        })
        .collect();
    let shelved: Vec<Value> = ws
        .shelved_panes()
        .iter()
        .map(|bp| {
            json!({
                "id": bp.id().as_u64(),
                "title": bp.title(),
                "role": bp.role(),
                "origin": origin_str(bp.pane().origin()),
                "spawned_by": bp.pane().spawned_by().map(|id| id.as_u64()),
                "origin_tab": bp.origin_tab().as_u64(),
                "origin_tab_title": bp.origin_tab_title(),
            })
        })
        .collect();
    json!({
        "active_tab": ws.active_tab_id().as_u64(),
        // 複数ウィンドウ（Issue #339）。後方互換: 既存フィールドは維持し追加のみ
        "active_window": ws.active_window_id().as_u64(),
        "windows": windows_json(ws)["windows"].clone(),
        "tabs": tabs,
        "shelved_panes": shelved,
        // ピン留め中のプレビューウィンドウ（FR-2.16.15。AI が現在のピンを把握できる）
        "pinned": pinned_json(host),
    })
}

/// ピン留め中のプレビュー一覧を JSON 配列へ（list / Pin 応答で共用。FR-2.16.15）
fn pinned_json(host: &dyn ControlHost) -> Value {
    Value::Array(
        host.pinned_previews()
            .into_iter()
            .map(|p| {
                json!({
                    "kind": if p.group { "group" } else { "pane" },
                    "id": p.id,
                    "x": p.x,
                    "y": p.y,
                })
            })
            .collect(),
    )
}

/// タイトルの出どころの文字列表現（list / MCP 公開用。FR-2.12.1）
fn title_source_str(source: tako_core::TitleSource) -> &'static str {
    source.as_str()
}

/// コマンド実行状態の文字列表現（list / MCP 公開用）
fn command_state_str(state: tako_core::CommandState) -> &'static str {
    match state {
        tako_core::CommandState::Unknown => "unknown",
        tako_core::CommandState::Idle => "idle",
        tako_core::CommandState::Running => "running",
        tako_core::CommandState::Failed(_) => "failed",
    }
}

fn tree_json(node: &PaneNode) -> Value {
    match node {
        PaneNode::Leaf(p) => json!({ "type": "pane", "id": p.id().as_u64() }),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => json!({
            "type": "split",
            "axis": match axis {
                SplitAxis::Horizontal => "x",
                SplitAxis::Vertical => "y",
            },
            "ratio": ratio,
            "first": tree_json(first),
            "second": tree_json(second),
        }),
    }
}

fn origin_str(origin: PaneOrigin) -> &'static str {
    match origin {
        PaneOrigin::User => "user",
        PaneOrigin::Cli => "cli",
        PaneOrigin::Mcp => "mcp",
        PaneOrigin::Suggestion => "suggestion",
    }
}

/// UI スレッドで収集した pane/backend 対応表。`fetch_tmux_sessions` に渡す
pub struct TmuxContext {
    pub pane_of_tty: Vec<(String, u64, u64)>,
    pub backend_of: Vec<(String, u64, u64)>,
}

/// tmux セッション一覧を取得して JSON 配列を返す。
/// tmux コマンド実行（重い）を含むため、**background thread で呼ぶこと**。
/// dispatch の TmuxList と同じ JSON 構造を返す
pub fn fetch_tmux_sessions(ctx: &TmuxContext) -> Vec<Value> {
    let session_json = |s: &tako_core::TmuxSession, backend: bool, socket: &Value| {
        let clients: Vec<Value> = s
            .client_ttys
            .iter()
            .map(|tty| {
                let hit = ctx.pane_of_tty.iter().find(|(t, _, _)| t == tty);
                json!({
                    "tty": tty,
                    "pane": hit.map(|(_, pane, _)| *pane),
                    "tab": hit.map(|(_, _, tab)| *tab),
                })
            })
            .collect();
        let owner = ctx.backend_of.iter().find(|(name, _, _)| *name == s.name);
        json!({
            "name": s.name,
            "created": s.created,
            "attached": s.attached,
            "backend": backend,
            "socket": socket,
            "backend_pane": owner.map(|(_, pane, _)| *pane),
            "backend_tab": owner.map(|(_, _, tab)| *tab),
            "pane_pid": s.pane_pid,
            "pane_command": s.pane_command,
            "pane_current_path": s.pane_current_path,
            "last_activity": s.last_activity,
            "windows": s.windows.iter().map(|w| json!({
                "index": w.index,
                "name": w.name,
                "active": w.active,
                "panes": w.panes,
            })).collect::<Vec<_>>(),
            "clients": clients,
        })
    };
    let backend_socket = tako_core::tmux_backend::socket_name();
    let mut sessions: Vec<Value> = tako_core::tmux::list_sessions(None)
        .iter()
        .map(|s| session_json(s, false, &Value::Null))
        .collect();
    sessions.extend(
        tako_core::tmux::list_sessions(Some(&backend_socket))
            .iter()
            .map(|s| session_json(s, true, &backend_socket.clone().into())),
    );
    sessions
}

/// 呼び出し元ペインに紐づく master プロファイルを解決する。
/// caller の role（orchestrator-master:X）から直接、または spawned_by チェーンを辿って
/// master を見つけ、suffix からプロファイルを引く。
/// 見つからなければ default プロファイルにフォールバック。
/// 呼び出し元 master のプロファイル解決。pane が stale でも role_suffix（TAKO_ORCHESTRATOR_ROLE
/// 由来）があれば正しいプロファイルを読む（#109）
fn resolve_caller_profile_with_role(
    workspace: &tako_core::Workspace,
    caller: Option<PaneId>,
    role_suffix: &Option<String>,
) -> crate::orchestrator::Profile {
    let _ = crate::orchestrator::migrate_legacy_default_profile();
    let suffix = role_suffix
        .clone()
        .or_else(|| caller.and_then(|pid| find_master_suffix_from(workspace, pid)))
        .unwrap_or_default();
    let name = if suffix.is_empty() {
        "default"
    } else {
        &suffix
    };
    crate::orchestrator::Profile::load(name).unwrap_or_default()
}

/// caller ペインから master の role suffix を検索する。
/// caller 自身が master なら直接返し、そうでなければ spawned_by を辿る。
fn find_master_suffix_from(workspace: &tako_core::Workspace, start: PaneId) -> Option<String> {
    if let Some(suffix) = pane_master_suffix(workspace, start) {
        return Some(suffix);
    }
    let mut current = start;
    for _ in 0..10 {
        let parent = workspace.tabs().iter().find_map(|t| {
            t.tree()
                .panes()
                .iter()
                .find(|p| p.id() == current)
                .and_then(|p| p.spawned_by())
        })?;
        if let Some(suffix) = pane_master_suffix(workspace, parent) {
            return Some(suffix);
        }
        current = parent;
    }
    None
}

fn pane_master_suffix(workspace: &tako_core::Workspace, pane_id: PaneId) -> Option<String> {
    workspace.tabs().iter().find_map(|t| {
        t.tree().panes().iter().find_map(|p| {
            if p.id() != pane_id {
                return None;
            }
            let role = p.role()?;
            if let Some(suffix) = role.strip_prefix("orchestrator-master:") {
                Some(suffix.to_string())
            } else if role == "orchestrator-master" {
                Some(String::new())
            } else {
                None
            }
        })
    })
}

// --- ファイルツリーフォルダ操作 (#134) ---

fn dispatch_tree_folder(
    host: &mut dyn ControlHost,
    action: &str,
    path: Option<String>,
    tab: Option<u64>,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    use std::path::PathBuf;

    let tab_id = resolve_tab(host.workspace(), tab, pane)?;

    match action {
        "add" => {
            let path_str = path.ok_or(DispatchError::InvalidParams("path を指定する".into()))?;
            let abs = PathBuf::from(&path_str);
            if !abs.is_absolute() {
                return Err(DispatchError::InvalidParams("絶対パスを指定する".into()));
            }
            if !abs.is_dir() {
                return Err(DispatchError::Operation(format!(
                    "ディレクトリが存在しない: {path_str}"
                )));
            }
            let canonical = abs.canonicalize().unwrap_or_else(|_| abs.clone());
            let tab_mut = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .ok_or(DispatchError::InvalidParams("タブが見つからない".into()))?;
            if !tab_mut.add_pinned_folder(canonical.clone()) {
                return Ok(
                    json!({ "status": "already_exists", "path": canonical.display().to_string() }),
                );
            }
            host.sync_filetree();
            Ok(json!({ "status": "added", "path": canonical.display().to_string() }))
        }
        "remove" => {
            let path_str = path.ok_or(DispatchError::InvalidParams("path を指定する".into()))?;
            let abs = PathBuf::from(&path_str);
            let canonical = abs.canonicalize().unwrap_or_else(|_| abs.clone());
            let tab_mut = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .ok_or(DispatchError::InvalidParams("タブが見つからない".into()))?;
            if !tab_mut.remove_pinned_folder(&canonical) {
                return Err(DispatchError::Operation(format!(
                    "指定フォルダはピン留めされていない: {}",
                    canonical.display()
                )));
            }
            host.sync_filetree();
            Ok(json!({ "status": "removed", "path": canonical.display().to_string() }))
        }
        "list" => {
            // 実体が消えたエントリを自動除去してから返す（#171）
            if let Some(tab_mut) = host.workspace_mut().get_tab_mut(tab_id) {
                tab_mut.prune_dead_folders();
            }
            let tab_ref = host
                .workspace()
                .get_tab(tab_id)
                .ok_or(DispatchError::InvalidParams("タブが見つからない".into()))?;
            let folders: Vec<String> = tab_ref
                .pinned_folders()
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            Ok(json!({ "folders": folders, "tab": tab_id.as_u64() }))
        }
        _ => Err(DispatchError::InvalidParams(format!(
            "action は add / remove / list のいずれか（受け取った値: {action}）"
        ))),
    }
}

/// タブ ID を解決する（tab 明示 > pane のタブ > アクティブタブ）
fn resolve_tab(
    ws: &Workspace,
    tab: Option<u64>,
    pane: Option<u64>,
) -> Result<TabId, DispatchError> {
    if let Some(t) = tab {
        let tid = TabId::from_raw(t);
        if ws.get_tab(tid).is_none() {
            return Err(DispatchError::InvalidParams(format!(
                "タブ {t} が見つからない"
            )));
        }
        return Ok(tid);
    }
    if let Some(p) = pane {
        let pid = PaneId::from_raw(p);
        for t in ws.tabs() {
            if t.tree().contains(pid) {
                return Ok(t.id());
            }
        }
    }
    Ok(ws.active_tab().id())
}

/// task checkpoint resume: チェックポイントから worker を再起動する（Issue #242）。
/// checkpoint の branch / cwd / issue を復元し、OrchestratorSpawn と同じ経路で
/// 新ペインを生やしてプロンプトを注入する
fn dispatch_task_resume(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    task_id: &str,
    resume_pane: Option<u64>,
    tab: Option<u64>,
    resume_model: Option<&str>,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    let store =
        crate::task_checkpoints::TaskCheckpointStore::load().map_err(DispatchError::Operation)?;
    let cp = store
        .find(task_id)
        .ok_or_else(|| {
            DispatchError::InvalidParams(format!("チェックポイントが見つからない: {task_id}"))
        })?
        .clone();

    let project = cp.project.as_deref().unwrap_or("default");
    let model = resume_model.map(String::from).or_else(|| cp.model.clone());
    let agent_str = cp.agent.as_deref().unwrap_or("claude");

    // resume プロンプトを組み立てる
    let mut prompt_lines = vec![format!(
        "Resume task {task_id}. Continue the work from where it was interrupted."
    )];
    if let Some(issue) = cp.issue {
        prompt_lines.push(format!("GitHub Issue: #{issue}"));
    }
    if let Some(ref branch) = cp.branch {
        prompt_lines.push(format!("Branch: {branch} (checkout this branch first)"));
    }
    if let Some(ref sha) = cp.last_commit {
        prompt_lines.push(format!("Last commit: {sha}"));
    }
    if let Some(ref head) = cp.prompt_head {
        prompt_lines.push(format!("Context: {head}"));
    }
    if let Some(ref reason) = cp.suspended_reason {
        prompt_lines.push(format!(
            "Previous suspension reason: {reason}. \
             The issue may have been resolved — check before acting on it."
        ));
    }
    prompt_lines.push(
        "Read the codebase state, verify the current branch and last commit, \
         then continue implementation."
            .into(),
    );
    let prompt = prompt_lines.join("\n");
    let label = format!(
        "resume-{}",
        cp.issue
            .map(|i| format!("#{i}"))
            .unwrap_or_else(|| task_id.to_string())
    );

    // OrchestratorSpawn と同じ経路で spawn する
    let spawn_result = dispatch_orchestrator_spawn(
        host,
        origin,
        SpawnParams {
            project,
            prompt: &prompt,
            label: Some(&label),
            model: model.as_deref(),
            effort: None,
            pane: resume_pane.or(cp.pane_id),
            tab,
            caller_role,
            agent: Some(agent_str),
            caller_pid: None,
            task_type: None,
            account: None,
        },
    )?;

    // チェックポイントの phase を Running に更新し、新しい pane_id を記録する
    let new_pane_id = spawn_result["pane_id"].as_u64();
    crate::task_checkpoints::TaskCheckpointStore::mutate(|store| {
        if let Some(existing) = store.find_mut(task_id) {
            existing.phase = tako_core::task_checkpoint::TaskPhase::Running;
            existing.pane_id = new_pane_id;
            existing.suspended_reason = None;
            if let Some(ref m) = model {
                existing.model = Some(m.clone());
            }
            existing.touch();
        }
    })
    .map_err(DispatchError::Operation)?;

    let mut result = spawn_result;
    result
        .as_object_mut()
        .unwrap()
        .insert("task_id".into(), json!(task_id));
    result
        .as_object_mut()
        .unwrap()
        .insert("resumed".into(), json!(true));
    Ok(result)
}

struct LedgerParams {
    action: String,
    id: Option<String>,
    outcome: Option<String>,
    rounds: Option<u32>,
    note: Option<String>,
    project: Option<String>,
    task_type: Option<String>,
    limit: Option<usize>,
}

/// OrchestratorSupervisor の dispatch（Issue #401）
fn dispatch_orchestrator_supervisor(
    action: &str,
    mode: Option<&str>,
    auto_resume_dead: Option<bool>,
    max_retries: Option<u32>,
    lines: Option<usize>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::supervisor;

    match action {
        "status" => {
            let profile = crate::orchestrator::Profile::load("default").unwrap_or_default();
            let sv_mode = profile
                .supervisor_mode
                .unwrap_or(supervisor::SupervisorMode::Auto);
            let auto_dead = profile.auto_resume_dead.unwrap_or(false);
            let retries = profile.supervisor_max_retries.unwrap_or(3);
            let log = supervisor::read_audit_log(lines.unwrap_or(20));
            // latest_cursor: master が「ここから読めば取りこぼさない」起点として使う
            let (_, cursor, _) = supervisor::read_events(0, 1).unwrap_or_default();
            Ok(json!({
                "mode": sv_mode.as_str(),
                "auto_resume_dead": auto_dead,
                "max_retries": retries,
                "audit_log": log,
                // #665: 常駐しているか。false なら誰も監視していない
                "running": supervisor::is_running(),
                "latest_cursor": cursor,
            }))
        }
        "set_mode" => {
            let sv_mode = mode
                .and_then(supervisor::SupervisorMode::parse_mode)
                .ok_or_else(|| {
                    DispatchError::Operation(
                        "mode は auto / notify_only / off のいずれかを指定".to_string(),
                    )
                })?;
            crate::orchestrator::Profile::mutate_named("default", |p| {
                p.supervisor_mode = Some(sv_mode);
                if let Some(ard) = auto_resume_dead {
                    p.auto_resume_dead = Some(ard);
                }
                if let Some(mr) = max_retries {
                    p.supervisor_max_retries = Some(mr);
                }
            })
            .map_err(DispatchError::Operation)?;
            Ok(json!({
                "mode": sv_mode.as_str(),
                "auto_resume_dead": auto_resume_dead.unwrap_or(false),
                "max_retries": max_retries.unwrap_or(3),
                "updated": true,
            }))
        }
        "history" => {
            let log = supervisor::read_audit_log(lines.unwrap_or(50));
            Ok(json!({ "audit_log": log }))
        }
        _ => Err(DispatchError::Operation(format!(
            "supervisor の action は status / set_mode / history のいずれか（不明: '{action}'）"
        ))),
    }
}

/// OrchestratorLedger の dispatch（Issue #292）。ControlHost 不要のためスタンドアロン
fn dispatch_orchestrator_ledger(p: LedgerParams) -> Result<Value, DispatchError> {
    let LedgerParams {
        action,
        id,
        outcome,
        rounds,
        note,
        project,
        task_type,
        limit,
    } = p;
    use crate::orchestrator::ledger;
    match action.as_str() {
        "list" => {
            let ledger = ledger::Ledger::load().map_err(DispatchError::Operation)?;
            let mut entries: Vec<&ledger::LedgerEntry> = ledger.entries.iter().collect();
            if let Some(ref p) = project {
                entries.retain(|e| e.project == *p);
            }
            if let Some(ref t) = task_type {
                entries.retain(|e| e.task_type == *t);
            }
            let limit = limit.unwrap_or(50);
            if entries.len() > limit {
                entries = entries[entries.len() - limit..].to_vec();
            }
            Ok(json!({
                "entries": entries,
                "total": ledger.entries.len(),
                "unevaluated": ledger.unevaluated_count(),
            }))
        }
        "stats" => {
            let ledger = ledger::Ledger::load().map_err(DispatchError::Operation)?;
            let stats = ledger.stats();
            Ok(json!({
                "stats": stats,
                "total_entries": ledger.entries.len(),
                "unevaluated": ledger.unevaluated_count(),
            }))
        }
        "record" => {
            let id = id.ok_or_else(|| DispatchError::InvalidParams("id は必須".into()))?;
            let outcome =
                outcome.ok_or_else(|| DispatchError::InvalidParams("outcome は必須".into()))?;
            ledger::record_outcome(&id, &outcome, rounds, note.as_deref())
                .map_err(DispatchError::Operation)?;
            Ok(json!({"ok": true, "id": id, "outcome": outcome}))
        }
        "amend" => {
            let id = id.ok_or_else(|| DispatchError::InvalidParams("id は必須".into()))?;
            let note = note.ok_or_else(|| DispatchError::InvalidParams("note は必須".into()))?;
            ledger::amend_entry(&id, &note).map_err(DispatchError::Operation)?;
            Ok(json!({"ok": true, "id": id, "post_issue": true}))
        }
        "prune" => {
            let prefix = project.ok_or_else(|| {
                DispatchError::InvalidParams("project（前方一致プレフィックス）は必須".into())
            })?;
            let removed = ledger::Ledger::mutate(|l| l.prune_by_project_prefix(&prefix))
                .map_err(DispatchError::Operation)?;
            Ok(json!({"ok": true, "prefix": prefix, "removed": removed}))
        }
        _ => Err(DispatchError::InvalidParams(format!(
            "不正な action '{action}'。使用可能: list, stats, record, amend, prune"
        ))),
    }
}

// ---------------------------------------------------------------------------
// StaleBinary — stale claude バイナリの検知と張り直し（Issue #498）
// ---------------------------------------------------------------------------

/// status: 指定ペインの stale 判定情報を返す
fn dispatch_stale_binary_status(
    host: &dyn ControlHost,
    _origin: PaneOrigin,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    let (tab_id, pane_id) = resolve_pane(host.workspace(), pane)?;
    let pane_obj = host
        .workspace()
        .get_tab(tab_id)
        .and_then(|t| t.tree().get(pane_id))
        .ok_or(DispatchError::PaneNotFound(pane_id.as_u64()))?;

    let role = pane_obj.role().unwrap_or("");
    let is_master =
        role.contains("orchestrator-master") || role == "master" || role.starts_with("master:");
    let is_worker = role.contains("orchestrator-worker") || role.starts_with("worker");

    // バックエンドセッション名を取得
    let backend = host.backend_session(pane_id);
    let backend_session = match backend.as_deref() {
        Some(s) => s,
        None => {
            return Ok(json!({
                "stale": false,
                "reason": "バックエンドセッションなし（stale 検知対象外）",
                "pane": pane_id.as_u64(),
            }));
        }
    };

    // claude PID を解決
    let pid = crate::stale_binary::find_claude_pid_for_backend(backend_session);

    // 起動時バイナリパス: PID が取れたら pidpath で取得
    let spawned = pid.and_then(crate::stale_binary::pidpath);
    let current = crate::stale_binary::resolve_current_claude_binary();

    let spawned_version = spawned
        .as_ref()
        .map(|p| crate::stale_binary::extract_version_from_path(p))
        .unwrap_or_default();
    let current_version = current
        .as_ref()
        .map(|p| crate::stale_binary::extract_version_from_path(p))
        .unwrap_or_default();

    let stale = match (&spawned, &current) {
        (Some(s), Some(c)) => s != c,
        _ => false,
    };

    Ok(json!({
        "stale": stale,
        "pane": pane_id.as_u64(),
        "role": role,
        "is_master": is_master,
        "is_worker": is_worker,
        "spawned_binary": spawned.as_ref().map(|p| p.to_string_lossy().to_string()),
        "current_binary": current.as_ref().map(|p| p.to_string_lossy().to_string()),
        "spawned_version": spawned_version,
        "current_version": current_version,
        "pid": pid,
    }))
}

/// restart: stale ペインを張り直す。worker は resume、master は handoff
fn dispatch_stale_binary_restart(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    let (tab_id, pane_id) = resolve_pane(host.workspace(), pane)?;
    let pane_obj = host
        .workspace()
        .get_tab(tab_id)
        .and_then(|t| t.tree().get(pane_id))
        .ok_or(DispatchError::PaneNotFound(pane_id.as_u64()))?;

    let role = pane_obj.role().unwrap_or("").to_string();
    let is_master =
        role.contains("orchestrator-master") || role == "master" || role.starts_with("master:");

    // busy チェック: Running 状態なら拒否
    let cmd_state = host
        .session(pane_id)
        .map(|s| s.command_state())
        .unwrap_or(CommandState::Unknown);
    if matches!(cmd_state, CommandState::Running) {
        return Err(DispatchError::Operation(
            "ペインが実行中です。アイドルになってから張り直してください".into(),
        ));
    }

    if is_master {
        // master: handoff 経由で新 master を spawn
        dispatch_orchestrator_handoff(host, origin, pane, Some(&role), None, None)
    } else {
        // worker: session_id を解決して resume
        let backend = host.backend_session(pane_id);
        let backend_session = backend.as_deref().ok_or_else(|| {
            DispatchError::Operation("バックエンドセッションが見つからない".into())
        })?;

        let session_id = crate::agents::resolve_session_id_for_backend(backend_session)
            .ok_or_else(|| {
                DispatchError::Operation(
                    "claude の session_id を解決できません。プロセスが終了しているか、\
                     claude agents --json で情報を取得できません"
                        .into(),
                )
            })?;

        // 旧プロセスを終了: Ctrl-C を送信
        host.queue_write(pane_id, b"\x03".to_vec());

        // 少し待ってから resume コマンドを送信（prompt_flow で送達確認）
        let resume_cmd = format!("claude --resume {session_id}");
        host.queue_prompt_flow(pane_id, String::new());
        // prompt_flow の空文字列はプロンプト待ちのみ。その後にコマンドを書く
        let mut cmd_bytes = resume_cmd.into_bytes();
        cmd_bytes.push(b'\r');
        host.queue_write_on_alt_screen(pane_id, cmd_bytes);

        Ok(json!({
            "restarted": true,
            "pane": pane_id.as_u64(),
            "method": "resume",
            "session_id": session_id,
        }))
    }
}

/// dismiss: バナーを閉じる（UI 側の状態を変更する指示。GUI 層で使用）
fn dispatch_stale_binary_dismiss(
    host: &dyn ControlHost,
    _origin: PaneOrigin,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    let (_, pane_id) = resolve_pane(host.workspace(), pane)?;
    Ok(json!({
        "dismissed": true,
        "pane": pane_id.as_u64(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Axis;
    use tako_core::TerminalSession;

    // --- #687: 器がスクロールを持つペインのスクロール計算 ---

    #[test]
    fn スクロール目標は履歴の範囲へクランプされる() {
        // to = 絶対位置（0 = 最下部）
        assert_eq!(scroll_goal(0, 279, Some(100), None), 100);
        assert_eq!(scroll_goal(50, 279, Some(0), None), 0);
        // 履歴より先へは行けない（器が持っていない行は表示できない）
        assert_eq!(scroll_goal(0, 279, Some(9999), None), 279);
        // delta = 現在位置からの相対
        assert_eq!(scroll_goal(10, 279, None, Some(5)), 15);
        assert_eq!(scroll_goal(10, 279, None, Some(-5)), 5);
        // 最下部より下へは行けない
        assert_eq!(scroll_goal(3, 279, None, Some(-99)), 0);
        assert_eq!(scroll_goal(3, 279, None, Some(9999)), 279);
        // 履歴 0（alt screen 直後など）はどう指定しても最下部
        assert_eq!(scroll_goal(0, 0, Some(50), None), 0);
        // 引数なしは現在位置のまま（呼び出し側が上流で弾くが、ここでも動かさない）
        assert_eq!(scroll_goal(7, 279, None, None), 7);
    }

    #[test]
    fn ホイールイベント数は器の粒度で決まる() {
        // 1 イベント = 1 行の器: 残りをそのままイベント数にできる
        assert_eq!(wheel_events_for(10, 1.0), 10);
        assert_eq!(wheel_events_for(-10, 1.0), -10);
        assert_eq!(wheel_events_for(0, 1.0), 0);

        // 1 イベント = 3 行の器: 四捨五入で寄せる
        assert_eq!(wheel_events_for(9, 3.0), 3);
        assert_eq!(wheel_events_for(10, 3.0), 3);
        assert_eq!(wheel_events_for(11, 3.0), 4);

        // **器の粒度より細かい残りでは撃たない**（撃つと行き過ぎて振動する）
        assert_eq!(wheel_events_for(1, 5.0), 0);
        assert_eq!(wheel_events_for(-2, 5.0), 0);
        // 粒度が 1 行以下なら残り 1 行も埋められる
        assert_eq!(wheel_events_for(1, 1.0), 1);
        assert_eq!(wheel_events_for(1, 0.5), 2);

        // 1 リクエストの上限で頭打ちにする（単一 write のサイズを抑える）
        let max = tako_core::terminal::PROGRAMMATIC_WHEEL_MAX;
        assert_eq!(wheel_events_for(100_000, 1.0), max);
        assert_eq!(wheel_events_for(-100_000, 1.0), -max);
    }

    /// 学習した粒度で 2 往復目が目標へ寄ることの確認（閉ループの算数だけを検証する）
    #[test]
    fn 粒度を学習すると2往復目で目標へ寄る() {
        // 器は 1 イベント 5 行動く。1 往復目は「1 行 = 1 イベント」と仮置きするので
        // 行き過ぎるが、実測から粒度を学習して 2 往復目で戻る
        let per_actual = 5.0f32;
        let goal = 40i64;
        let mut position = 0i64;

        let events = wheel_events_for(goal - position, 1.0);
        assert_eq!(events, 40, "1 往復目は仮置きの粒度で撃つ");
        position += (events as f32 * per_actual) as i64;
        assert_eq!(position, 200, "行き過ぎる");

        let learned = (position as f32 / events as f32).max(0.05);
        assert_eq!(learned, per_actual, "1 往復の実測から粒度が分かる");
        let events2 = wheel_events_for(goal - position, learned);
        assert_eq!(events2, -32);
        position += (events2 as f32 * per_actual) as i64;
        assert_eq!(position, goal, "2 往復目で目標へ乗る");
    }

    /// セッションを起動しないテスト用ホスト（レイアウト操作の検証に使う）
    struct MockHost {
        ws: Workspace,
        attached: Vec<u64>,
        attached_options: std::collections::HashMap<u64, SpawnOptions>,
        /// 書きっぱなしの遅延書き込み（pane, バイト列）。#640 の回帰検査で使う
        queued_writes: Vec<(u64, Vec<u8>)>,
        /// 送達確認つきの起動コマンド（pane, 本文）。#640
        command_flows: Vec<(u64, String)>,
        detached: Vec<u64>,
        previews: std::collections::HashMap<u64, (String, PreviewModeWire)>,
        preview_views: std::collections::HashMap<u64, tako_core::PreviewViewState>,
        preview_outlines: std::collections::HashMap<u64, tako_core::PreviewOutline>,
        last_outline_target: Option<tako_core::PreviewOutlineTarget>,
        preview_edits: std::collections::HashMap<u64, (bool, bool, String)>,
        collapsed: std::collections::HashSet<u64>,
        /// ピン留め: (group, id)
        pins: Vec<(bool, u64)>,
        /// #210: 旧 pane ID → 新 pane ID マッピング
        stale_pane_map: std::collections::HashMap<PaneId, PaneId>,
        /// #217: UI テーマモード
        theme_mode: tako_core::theme::ThemeMode,
        lang_setting: tako_core::i18n::LangSetting,
        lang_resolved: Option<tako_core::i18n::Lang>,
        /// #321: 利用制限表示サービス
        limit_service: tako_core::LimitService,
        preview_reload: tako_core::PreviewReloadState,
        preview_cache: tako_core::PreviewCacheStats,
        /// #584: UI 層へ依頼したウィンドウ表示状態の操作（window ID, 操作）
        window_state_ops: Vec<(u64, crate::protocol::WindowStateOp)>,
        /// ペイン → バックエンド tmux セッション名（#571 の e2e で実セッションを差す）
        backend_sessions: std::collections::HashMap<u64, String>,
        /// #657: メニューバーの構成（UI 層が持つものの代役）
        menu_bar: crate::protocol::MenuBarSnapshot,
        /// #657: UI 層へ依頼したメニュー操作
        menu_ops: Vec<crate::protocol::MenuOp>,
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                ws: Workspace::new("t1", Pane::new(PaneOrigin::User)),
                attached: Vec::new(),
                attached_options: std::collections::HashMap::new(),
                queued_writes: Vec::new(),
                command_flows: Vec::new(),
                detached: Vec::new(),
                previews: std::collections::HashMap::new(),
                preview_views: std::collections::HashMap::new(),
                preview_outlines: std::collections::HashMap::new(),
                last_outline_target: None,
                preview_edits: std::collections::HashMap::new(),
                collapsed: std::collections::HashSet::new(),
                pins: Vec::new(),
                stale_pane_map: std::collections::HashMap::new(),
                theme_mode: tako_core::theme::ThemeMode::Dark,
                lang_setting: tako_core::i18n::LangSetting::System,
                lang_resolved: None,
                limit_service: tako_core::LimitService::Claude,
                preview_reload: tako_core::PreviewReloadState::default(),
                preview_cache: tako_core::PreviewCacheStats {
                    max_bytes: 512 * 1024 * 1024,
                    used_bytes: 32 * 1024 * 1024,
                    entries: 2,
                },
                window_state_ops: Vec::new(),
                backend_sessions: std::collections::HashMap::new(),
                menu_bar: sample_menu_bar(),
                menu_ops: Vec::new(),
            }
        }

        fn toggle_pin(&mut self, group: bool, id: u64, pinned: Option<bool>) {
            let pos = self.pins.iter().position(|p| *p == (group, id));
            let want = pinned.unwrap_or(pos.is_none());
            match (want, pos) {
                (true, None) => self.pins.push((group, id)),
                (false, Some(i)) => {
                    self.pins.remove(i);
                }
                _ => {}
            }
        }

        fn root_pane(&self) -> u64 {
            self.ws.active_tab().tree().focused().as_u64()
        }
    }

    impl WorkspaceHost for MockHost {
        fn workspace(&self) -> &Workspace {
            &self.ws
        }
        fn workspace_mut(&mut self) -> &mut Workspace {
            &mut self.ws
        }
    }

    impl SessionHost for MockHost {
        fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
            None
        }
        fn attach_session(&mut self, pane: PaneId, options: SpawnOptions) {
            self.attached.push(pane.as_u64());
            self.attached_options.insert(pane.as_u64(), options);
        }
        fn detach_session(&mut self, pane: PaneId) {
            self.detached.push(pane.as_u64());
            self.previews.remove(&pane.as_u64());
            self.preview_views.remove(&pane.as_u64());
            self.preview_outlines.remove(&pane.as_u64());
            self.preview_edits.remove(&pane.as_u64());
        }
        fn queue_write(&mut self, pane: PaneId, data: Vec<u8>) {
            self.queued_writes.push((pane.as_u64(), data));
        }
        fn queue_command_flow(&mut self, pane: PaneId, command: String) {
            self.command_flows.push((pane.as_u64(), command));
        }
    }

    impl TmuxHost for MockHost {
        fn backend_session(&self, pane: PaneId) -> Option<String> {
            self.backend_sessions.get(&pane.as_u64()).cloned()
        }
        fn tmux_tab_collapsed(&self, tab: TabId) -> bool {
            self.collapsed.contains(&tab.as_u64())
        }
        fn set_tmux_tab_collapsed(&mut self, tab: TabId, collapsed: Option<bool>) {
            let now = collapsed.unwrap_or_else(|| !self.collapsed.contains(&tab.as_u64()));
            if now {
                self.collapsed.insert(tab.as_u64());
            } else {
                self.collapsed.remove(&tab.as_u64());
            }
        }
    }

    impl UiStateHost for MockHost {
        fn request_window_state(
            &mut self,
            window: tako_core::WindowId,
            op: crate::protocol::WindowStateOp,
        ) {
            self.window_state_ops.push((window.as_u64(), op));
        }

        fn menu_bar_snapshot(&self) -> crate::protocol::MenuBarSnapshot {
            self.menu_bar.clone()
        }

        fn request_menu_op(&mut self, op: crate::protocol::MenuOp) {
            self.menu_ops.push(op);
        }

        fn pinned_previews(&self) -> Vec<PinnedView> {
            self.pins
                .iter()
                .map(|(group, id)| PinnedView {
                    group: *group,
                    id: *id,
                    x: 0.0,
                    y: 0.0,
                })
                .collect()
        }
        fn set_pin_pane(&mut self, pane: PaneId, pinned: Option<bool>) {
            self.toggle_pin(false, pane.as_u64(), pinned);
        }
        fn set_pin_group(&mut self, tab: TabId, pinned: Option<bool>) {
            self.toggle_pin(true, tab.as_u64(), pinned);
        }
        fn theme_mode(&self) -> tako_core::theme::ThemeMode {
            self.theme_mode
        }
        fn set_theme_mode(&mut self, mode: tako_core::theme::ThemeMode) {
            self.theme_mode = mode;
        }
        fn ui_lang_setting(&self) -> tako_core::i18n::LangSetting {
            self.lang_setting
        }
        fn set_ui_lang(
            &mut self,
            setting: tako_core::i18n::LangSetting,
            resolved: tako_core::i18n::Lang,
        ) {
            self.lang_setting = setting;
            self.lang_resolved = Some(resolved);
        }
        fn limit_service(&self) -> tako_core::LimitService {
            self.limit_service
        }
        fn set_limit_service(&mut self, service: tako_core::LimitService) {
            self.limit_service = service;
        }
    }

    impl PreviewHost for MockHost {
        fn preview_reload_enabled(&self) -> bool {
            self.preview_reload.enabled()
        }
        fn set_preview_reload(&mut self, enabled: bool) {
            self.preview_reload.set_enabled(enabled);
        }
        fn preview_cache_stats(&self) -> tako_core::PreviewCacheStats {
            self.preview_cache
        }
        fn set_preview_cache_budget(&mut self, max_bytes: u64) {
            self.preview_cache.max_bytes = max_bytes;
            self.preview_cache.used_bytes = self.preview_cache.used_bytes.min(max_bytes);
        }
        fn preview_state(&self, pane: PaneId) -> Option<(String, PreviewModeWire)> {
            self.previews.get(&pane.as_u64()).cloned()
        }
        fn set_preview(
            &mut self,
            pane: PaneId,
            path: &str,
            mode: PreviewModeWire,
        ) -> Result<(), String> {
            if self
                .preview_edits
                .get(&pane.as_u64())
                .is_some_and(|(_, dirty, _)| *dirty)
            {
                return Err("未保存の変更があるため別ファイルを開けない".into());
            }
            self.previews.insert(pane.as_u64(), (path.into(), mode));
            self.preview_outlines.remove(&pane.as_u64());
            if matches!(mode, PreviewModeWire::Pdf | PreviewModeWire::Image) {
                self.preview_views
                    .insert(pane.as_u64(), tako_core::PreviewViewState::default());
            } else {
                self.preview_views.remove(&pane.as_u64());
            }
            self.preview_edits.remove(&pane.as_u64());
            Ok(())
        }
        fn preview_view_state(&self, pane: PaneId) -> Option<tako_core::PreviewViewState> {
            self.preview_views.get(&pane.as_u64()).copied()
        }
        fn update_preview_view(
            &mut self,
            pane: PaneId,
            update: PreviewViewUpdate,
        ) -> Result<tako_core::PreviewViewState, String> {
            let state = self
                .preview_views
                .get_mut(&pane.as_u64())
                .ok_or_else(|| "ズーム対象のプレビューペインではない".to_string())?;
            state.apply(update)?;
            Ok(*state)
        }
        fn preview_outline(&self, pane: PaneId) -> Option<tako_core::PreviewOutline> {
            self.preview_outlines.get(&pane.as_u64()).cloned()
        }
        fn navigate_preview_outline(
            &mut self,
            pane: PaneId,
            item: usize,
        ) -> Result<tako_core::PreviewOutlineTarget, String> {
            let target = self
                .preview_outlines
                .get(&pane.as_u64())
                .ok_or_else(|| "アウトラインがない".to_string())?
                .target(item)?;
            self.last_outline_target = Some(target);
            Ok(target)
        }
        fn preview_edit_state(&self, pane: PaneId) -> Option<(bool, bool)> {
            self.previews.get(&pane.as_u64())?;
            Some(
                self.preview_edits
                    .get(&pane.as_u64())
                    .map(|(editing, dirty, _)| (*editing, *dirty))
                    .unwrap_or((false, false)),
            )
        }
        fn set_preview_editing(&mut self, pane: PaneId, enabled: bool) -> Result<(), String> {
            if !self.previews.contains_key(&pane.as_u64()) {
                return Err("プレビューペインではない".into());
            }
            let edit =
                self.preview_edits
                    .entry(pane.as_u64())
                    .or_insert((false, false, String::new()));
            edit.0 = enabled;
            Ok(())
        }
        fn apply_preview_text(&mut self, pane: PaneId, text: String) -> Result<(), String> {
            self.set_preview_editing(pane, true)?;
            let edit = self.preview_edits.get_mut(&pane.as_u64()).unwrap();
            edit.1 = true;
            edit.2 = text;
            Ok(())
        }
        fn save_preview(&mut self, pane: PaneId) -> Result<(), String> {
            let edit = self
                .preview_edits
                .get_mut(&pane.as_u64())
                .ok_or_else(|| "編集セッションがない".to_string())?;
            edit.1 = false;
            Ok(())
        }
        fn preview_pane_of_tab(&self, tab: TabId) -> Option<PaneId> {
            self.ws
                .get_tab(tab)?
                .tree()
                .panes()
                .into_iter()
                .map(|p| p.id())
                .find(|p| self.previews.contains_key(&p.as_u64()))
        }
    }

    impl WebViewHost for MockHost {}
    impl RemoteHost for MockHost {}
    impl SystemHost for MockHost {
        fn resolve_stale_pane(&self, stale: PaneId) -> Option<PaneId> {
            self.stale_pane_map.get(&stale).copied()
        }
    }

    fn split(host: &mut MockHost, pane: u64) -> u64 {
        dispatch(
            host,
            Request::Split {
                pane: Some(pane),
                tab: None,
                direction: None,
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap()["pane"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn splitで同じタブに新ペインが生えattachされる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        assert_eq!(host.attached, vec![new_id]);
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // 生成主体は Cli（FR-2.3.5 のポリシー制御に使う）
        let tree = host.ws.active_tab().tree();
        let origin = tree
            .panes()
            .iter()
            .find(|p| p.id().as_u64() == new_id)
            .unwrap()
            .origin();
        assert_eq!(origin, PaneOrigin::Cli);
    }

    #[test]
    fn splitのtab指定は別タブ内に分割する() {
        let mut host = MockHost::new();
        let _root = host.root_pane();
        // タブ 2 を作り、タブ 1 に戻る
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        let tab2_pane = result["pane"].as_u64().unwrap();
        let tab1 = host.ws.tabs()[0].id().as_u64();
        dispatch(&mut host, Request::TabSelect { tab: tab1 }, PaneOrigin::Cli).unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
        // tab 指定でタブ 2 内に分割（active tab はタブ 1 のまま）
        let result = dispatch(
            &mut host,
            Request::Split {
                pane: None,
                tab: Some(tab2),
                direction: Some(Direction::Down),
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        let new_pane = result["pane"].as_u64().unwrap();
        // 新ペインはタブ 2 内にある
        let t2 = host.ws.get_tab(find_tab(&host.ws, tab2).unwrap()).unwrap();
        assert_eq!(t2.tree().len(), 2);
        assert!(t2
            .tree()
            .panes()
            .iter()
            .any(|p| p.id().as_u64() == new_pane));
        // active tab は変わっていない
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
        let _ = tab2_pane;
    }

    #[test]
    fn closeでペインが消えdetachされる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::Close {
                pane: Some(new_id),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["closed"].as_u64(), Some(new_id));
        assert_eq!(host.detached, vec![new_id]);
        assert_eq!(host.ws.active_tab().tree().len(), 1);
    }

    #[test]
    fn タブ最後のペインのcloseはタブごと閉じる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs().len(), 2);
        dispatch(
            &mut host,
            Request::Close {
                pane: Some(root),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs().len(), 1);
        assert_eq!(host.detached, vec![root]);
    }

    #[test]
    fn 最後のタブの最後のペインは閉じられない() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: Some(root),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
        assert_eq!(host.ws.tabs().len(), 1);
        assert!(host.detached.is_empty());
    }

    #[test]
    fn focusはタブ切替も伴う() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab2);
        // タブ 1 のペインへフォーカス → アクティブタブも戻る
        dispatch(
            &mut host,
            Request::Focus {
                pane: Some(root),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_ne!(host.ws.active_tab_id().as_u64(), tab2);
        assert_eq!(host.ws.active_tab().tree().focused().as_u64(), root);
    }

    #[test]
    fn tab_newはfocus無しでアクティブタブを変えない() {
        let mut host = MockHost::new();
        let tab1 = host.ws.active_tab_id();
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let _tab2 = result["tab"].as_u64().unwrap();
        assert_eq!(host.ws.active_tab_id(), tab1);
    }

    #[test]
    fn tab_newはfocus指定でアクティブタブを切り替える() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab2);
    }

    #[test]
    fn move_paneはfocus無しでアクティブタブを変えない() {
        let mut host = MockHost::new();
        let tab1 = host.ws.active_tab_id();
        let root = host.root_pane();
        let p2 = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        // tab1 に戻る
        host.ws.activate_tab(tab1).unwrap();
        // p2 を tab2 へ移動（focus 無し）
        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(p2),
                tab: Some(tab2),
                target: None,
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        // アクティブタブは tab1 のまま
        assert_eq!(host.ws.active_tab_id(), tab1);
    }

    #[test]
    fn 方向フォーカスはアクティブタブ内で動く() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        // dispatch 経由の split はフォーカスを分割元（左側 = root）に維持する。
        // 右へ移動すると新ペインにフォーカスが移る
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Right),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["focused"].as_u64(), Some(new_id));
        // 左へ戻ると root に戻る
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Left),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["focused"].as_u64(), Some(root));
        // さらに左には何もない → null
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Left),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["focused"].is_null());
    }

    #[test]
    fn resizeはdeltaとshareの排他指定() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.2),
                share: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!((result["share"].as_f64().unwrap() - 0.7).abs() < 1e-5);
        let result = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: None,
                share: Some(0.4),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!((result["share"].as_f64().unwrap() - 0.4).abs() < 1e-5);
        let err = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.1),
                share: Some(0.5),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
    }

    #[test]
    fn equalizeはpaneからタブを解決する() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.3),
                share: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        dispatch(
            &mut host,
            Request::Equalize {
                pane: Some(root),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let rects = host.ws.active_tab().tree().layout(Rect::UNIT);
        for (_, r) in rects {
            assert!((r.width - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn listはペインの表示分類surfaceを返す() {
        // FR-2.16.12: 表示中 = アクティブタブ所属、それ以外は裏で実行中
        let mut host = MockHost::new();
        let root = host.root_pane(); // t1 のペイン
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User)); // t2 がアクティブに
        let result = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let tabs = result["tabs"].as_array().unwrap();
        for tab in tabs {
            let active = tab["active"].as_bool().unwrap();
            for p in tab["panes"].as_array().unwrap() {
                let surface = p["surface"].as_str().unwrap();
                let want = if active { "foreground" } else { "background" };
                assert_eq!(surface, want);
            }
        }
        // root（非アクティブな t1）は background
        let root_surface = tabs
            .iter()
            .flat_map(|t| t["panes"].as_array().unwrap())
            .find(|p| p["id"].as_u64() == Some(root))
            .unwrap()["surface"]
            .as_str()
            .unwrap();
        assert_eq!(root_surface, "background");
    }

    #[test]
    fn backgroundリストは由来タブとbackgroundを返す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let t1 = host.ws.active_tab_id();
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background {
                pane: Some(root),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(&mut host, Request::BackgroundList, PaneOrigin::Cli).unwrap();
        let items = result["backgrounded"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["pane"].as_u64(), Some(root));
        assert_eq!(items[0]["origin_tab"].as_u64(), Some(t1.as_u64()));
        assert_eq!(items[0]["origin_tab_title"].as_str(), Some("t1"));
        assert_eq!(items[0]["surface"].as_str(), Some("background"));
    }

    #[test]
    fn foregroundはtarget省略で由来タブへ戻す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let t1 = host.ws.active_tab_id();
        let p2 = split(&mut host, root);
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background {
                pane: Some(p2),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(host.ws.is_shelved(PaneId::from_raw(p2)));
        let result = dispatch(
            &mut host,
            Request::Foreground {
                pane: p2,
                target: None,
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["foregrounded"].as_u64(), Some(p2));
        assert!(!host.ws.is_shelved(PaneId::from_raw(p2)));
        assert_eq!(host.ws.find_tab_of_pane(PaneId::from_raw(p2)), Some(t1));
    }

    #[test]
    fn background_tabでタブ内全ペインがバックグラウンドへ移る() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let t1 = host.ws.active_tab_id();
        let p2 = split(&mut host, root);
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        let result = dispatch(
            &mut host,
            Request::Background {
                pane: None,
                tab: Some(t1.as_u64()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["backgrounded_tab"].as_u64(), Some(t1.as_u64()));
        let panes = result["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 2);
        assert!(host.ws.is_shelved(PaneId::from_raw(root)));
        assert!(host.ws.is_shelved(PaneId::from_raw(p2)));
    }

    #[test]
    fn background_tabで最後の1タブはエラー() {
        let mut host = MockHost::new();
        let t1 = host.ws.active_tab_id();
        let result = dispatch(
            &mut host,
            Request::Background {
                pane: None,
                tab: Some(t1.as_u64()),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn background_tabで存在しないタブはエラー() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::Background {
                pane: None,
                tab: Some(99999),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn backgroundリストにプレビュー情報が含まれる() {
        // #230: プレビューペインを BG 退避したとき BackgroundList にプレビュー情報が載る
        let mut host = MockHost::new();
        let root = host.root_pane();
        host.previews
            .insert(root, ("/tmp/test.md".into(), PreviewModeWire::Markdown));
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background {
                pane: Some(root),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(&mut host, Request::BackgroundList, PaneOrigin::Cli).unwrap();
        let items = result["backgrounded"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["state"].as_str(), Some("idle"));
        let preview = &items[0]["preview"];
        assert_eq!(preview["path"].as_str(), Some("/tmp/test.md"));
        assert_eq!(preview["mode"].as_str(), Some("markdown"));
    }

    #[test]
    fn プレビューペインのforeground復帰() {
        // #230: プレビューペインの退避 → 復帰でツリーに戻り、プレビュー情報を保持
        let mut host = MockHost::new();
        let root = host.root_pane();
        let p2 = split(&mut host, root);
        host.previews
            .insert(p2, ("/tmp/test.rs".into(), PreviewModeWire::Code));
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background {
                pane: Some(p2),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(host.ws.is_shelved(PaneId::from_raw(p2)));
        assert!(host.previews.contains_key(&p2));
        let result = dispatch(
            &mut host,
            Request::Foreground {
                pane: p2,
                target: None,
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["foregrounded"].as_u64(), Some(p2));
        assert!(!host.ws.is_shelved(PaneId::from_raw(p2)));
        assert!(host.previews.contains_key(&p2));
    }

    #[test]
    fn collapsetabはトグルとset両方ができ_listに出る() {
        // FR-2.16.14: 折りたたみは tab 指定 / 呼び出し元タブの両方で操作でき、
        // collapsed 省略でトグル、list の各タブ collapsed で状態取得できる
        let mut host = MockHost::new();
        let t1 = host.ws.active_tab_id();
        // 初期は折りたたまれていない
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["collapsed"].as_bool(), Some(false));
        // collapsed 省略 = トグルで折りたたむ
        let r = dispatch(
            &mut host,
            Request::CollapseTab {
                pane: None,
                tab: Some(t1.as_u64()),
                collapsed: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["collapsed"].as_bool(), Some(true));
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["collapsed"].as_bool(), Some(true));
        // collapsed=false で明示展開
        let r = dispatch(
            &mut host,
            Request::CollapseTab {
                pane: None,
                tab: Some(t1.as_u64()),
                collapsed: Some(false),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["collapsed"].as_bool(), Some(false));
        // tab 省略時は pane（呼び出し元）の属するタブを畳む
        let root = host.root_pane();
        dispatch(
            &mut host,
            Request::CollapseTab {
                pane: Some(root),
                tab: None,
                collapsed: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(host.tmux_tab_collapsed(t1));
    }

    #[test]
    fn pinはトグルとunpinができ_listのpinnedに出る() {
        // FR-2.16.15: pane のピン留め / 解除が list の pinned に反映される
        let mut host = MockHost::new();
        let root = host.root_pane();
        // 初期は空
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["pinned"].as_array().unwrap().len(), 0);
        // pinned 省略 = トグルでピン留め
        dispatch(
            &mut host,
            Request::Pin {
                pane: Some(root),
                group_tab: None,
                pinned: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let pinned = list["pinned"].as_array().unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0]["kind"].as_str(), Some("pane"));
        assert_eq!(pinned[0]["id"].as_u64(), Some(root));
        // pinned=false で解除
        dispatch(
            &mut host,
            Request::Pin {
                pane: Some(root),
                group_tab: None,
                pinned: Some(false),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["pinned"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn pinのgroup_tabはバックグラウンドの由来が無いと弾く() {
        // 閉じたタブグループのピンは、その由来を持つバックグラウンドペインが居るときだけ通る
        let mut host = MockHost::new();
        let err = dispatch(
            &mut host,
            Request::Pin {
                pane: None,
                group_tab: Some(9999),
                pinned: Some(true),
            },
            PaneOrigin::Cli,
        );
        assert!(matches!(err, Err(DispatchError::TabNotFound(9999))));
    }

    #[test]
    fn listはツリーとジオメトリと状態を返す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(new_id),
                title: Some("worker".into()),
                role: Some("dev-server".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let tabs = result["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 1);
        let panes = tabs[0]["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 2);
        let new_pane = panes
            .iter()
            .find(|p| p["id"].as_u64() == Some(new_id))
            .unwrap();
        assert_eq!(new_pane["title"].as_str(), Some("worker"));
        assert_eq!(new_pane["role"].as_str(), Some("dev-server"));
        assert_eq!(new_pane["origin"].as_str(), Some("cli"));
        // dispatch 経由の split はフォーカスを移さない（分割元を維持）
        assert_eq!(new_pane["focused"].as_bool(), Some(false));
        assert!((new_pane["rect"]["x"].as_f64().unwrap() - 0.5).abs() < 1e-5);
        // ツリー構造（ルートが split で leaf を 2 つ持つ）
        assert_eq!(tabs[0]["tree"]["type"].as_str(), Some("split"));
        assert_eq!(tabs[0]["tree"]["second"]["id"].as_u64(), Some(new_id));
    }

    #[test]
    fn enter単独送信の意図判定() {
        // Enter 代行（text 空 + newline）と改行のみのテキスト（Issue #95）
        assert!(send_is_enter_only("", true));
        assert!(send_is_enter_only("\n", false));
        assert!(send_is_enter_only("\n", true));
        assert!(send_is_enter_only("\r", false));
        assert!(send_is_enter_only("\r\n", false));
        // 通常テキストは対象外
        assert!(!send_is_enter_only("ls", true));
        assert!(!send_is_enter_only("a\nb", true));
        // text 空 + newline なしは「何も送らない」指示のため対象外
        assert!(!send_is_enter_only("", false));
    }

    #[test]
    fn キーボード改行正規化はlfをcrへ揃える() {
        // 端末の Enter は CR。LF のままだと claude TUI で送信にならない（Issue #95）
        assert_eq!(normalize_newlines_for_keys("ls\n"), "ls\r");
        assert_eq!(normalize_newlines_for_keys("a\r\nb\nc"), "a\rb\rc");
        assert_eq!(normalize_newlines_for_keys("そのまま"), "そのまま");
        assert_eq!(normalize_newlines_for_keys("\n"), "\r");
    }

    #[test]
    fn sendとreadはセッションが無ければエラー() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::Send {
                pane: Some(root),
                text: "ls".into(),
                newline: true,
                tmux_session: None,
                await_prompt: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoSession(root));
        let err = dispatch(
            &mut host,
            Request::Read {
                pane: Some(root),
                lines: None,
                tmux_session: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoSession(root));
    }

    #[test]
    fn タブ操作とペイン移送() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: Some("agents".into()),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        // TabNew のペインも attach される
        assert_eq!(host.attached.len(), 2);

        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(new_id),
                tab: Some(tab2),
                target: None,
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws
                .find_tab_of_pane(
                    host.ws
                        .get_tab(find_tab(&host.ws, tab2).unwrap())
                        .unwrap()
                        .tree()
                        .focused()
                )
                .unwrap()
                .as_u64(),
            tab2
        );
        assert_eq!(
            host.ws
                .get_tab(find_tab(&host.ws, tab2).unwrap())
                .unwrap()
                .tree()
                .len(),
            2
        );

        // タブ切替
        let tab1 = host.ws.tabs()[0].id().as_u64();
        dispatch(&mut host, Request::TabSelect { tab: tab1 }, PaneOrigin::Cli).unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
    }

    #[test]
    fn move_paneのtarget指定は同タブ内で挿し直す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root); // [root | new]
                                             // root を new の下へ（FR-1.10 = タイトルバー D&D の同等操作）
        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: Some(new_id),
                direction: Some(Direction::Down),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let rects = host.ws.active_tab().tree().layout(Rect::UNIT);
        let rect_of = |raw: u64| {
            rects
                .iter()
                .find(|(p, _)| p.as_u64() == raw)
                .map(|(_, r)| *r)
                .unwrap()
        };
        assert!(rect_of(new_id).y < rect_of(root).y, "root が下に入る");
        assert!((rect_of(root).width - 1.0).abs() < 1e-5, "縦分割 = 全幅");
        // tab と target の同時指定・両方省略・target + tab なし direction はエラー
        let tab1 = host.ws.tabs()[0].id().as_u64();
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: Some(tab1),
                target: Some(new_id),
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
        // tab=None, target=None は新タブ化（Issue #209）
        let tab_count_before = host.ws.tabs().len();
        let active_before = host.ws.active_tab_id();
        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: None,
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs().len(), tab_count_before + 1);
        // focus: None なのでアクティブタブは変わらない（#211: フォーカス非奪取）。
        // ただし元タブが閉じた（最後のペインを移動）場合は close_tab の移行先になる
        let root_tab = host.ws.find_tab_of_pane(PaneId::from_raw(root)).unwrap();
        if host.ws.get_tab(active_before).is_some() {
            assert_eq!(host.ws.active_tab_id(), active_before);
        } else {
            // 元タブが閉じた場合は close_tab の自動移行で root_tab がアクティブになる
            assert_eq!(host.ws.active_tab_id(), root_tab);
        }

        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: Some(tab1),
                target: None,
                direction: Some(Direction::Down),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
        // 自分自身へはドメイン層が拒否する
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: Some(root),
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
    }

    #[test]
    fn タブのリネームと手動優先() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        // pane からタブを解決してリネーム（FR-2.12.1）
        let result = dispatch(
            &mut host,
            Request::TabRename {
                pane: Some(root),
                tab: None,
                title: "実験".into(),
                source: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["title"].as_str(), Some("実験"));
        let tab = &host.ws.tabs()[0];
        assert_eq!(tab.title(), "実験");
        assert_eq!(tab.title_source(), tako_core::TitleSource::Manual);
        // list に title_source が公開される
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["title_source"].as_str(), Some("manual"));
        assert_eq!(
            list["tabs"][0]["panes"][0]["title_source"].as_str(),
            Some("default")
        );
        // 空文字で手動指定を解除（タイトルは保持）
        let tab_id = host.ws.tabs()[0].id().as_u64();
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: String::new(),
                source: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab = &host.ws.tabs()[0];
        assert_eq!(tab.title(), "実験");
        assert_eq!(tab.title_source(), tako_core::TitleSource::Default);
    }

    #[test]
    fn タブの自動リネームは手動リネーム済みを上書きしない() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let tab_id = host.ws.tabs()[0].id().as_u64();
        // 手動リネーム
        dispatch(
            &mut host,
            Request::TabRename {
                pane: Some(root),
                tab: None,
                title: "手動名".into(),
                source: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws.tabs()[0].title_source(),
            tako_core::TitleSource::Manual
        );
        // source=auto で上書きを試みる → 手動が優先されタイトル変わらず
        let result = dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: "自動名".into(),
                source: Some("auto".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["title"].as_str(), Some("手動名"));
        assert_eq!(result["source"].as_str(), Some("manual"));
        // 手動解除後は自動リネームが通る
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: String::new(),
                source: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: "自動名".into(),
                source: Some("auto".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["title"].as_str(), Some("自動名"));
        assert_eq!(result["source"].as_str(), Some("auto"));
    }

    #[test]
    fn タブの並べ替え() {
        let mut host = MockHost::new();
        let t1 = host.ws.active_tab_id();
        let t2 = host.ws.create_tab(
            "t2",
            tako_core::Pane::new(tako_core::pane::PaneOrigin::User),
        );
        let t3 = host.ws.create_tab(
            "t3",
            tako_core::Pane::new(tako_core::pane::PaneOrigin::User),
        );
        let result = dispatch(
            &mut host,
            Request::TabReorder {
                tab: t3.as_u64(),
                index: 0,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["tab"], t3.as_u64());
        assert_eq!(result["index"], 0);
        let ids: Vec<_> = host.ws.tabs().iter().map(|t| t.id()).collect();
        assert_eq!(ids, vec![t3, t1, t2]);
    }

    #[test]
    fn 明示タイトル付きのタブ作成は手動扱い() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: Some("agents".into()),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let new_tab_id = TabId::from_raw(result["tab"].as_u64().unwrap());
        assert_eq!(
            host.ws.get_tab(new_tab_id).unwrap().title_source(),
            tako_core::TitleSource::Manual
        );
        // 連番の既定タイトルは Default のまま（自動リネーム対象）
        let result2 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let new_tab_id2 = TabId::from_raw(result2["tab"].as_u64().unwrap());
        assert_eq!(
            host.ws.get_tab(new_tab_id2).unwrap().title_source(),
            tako_core::TitleSource::Default
        );
    }

    #[test]
    fn open_fileはプレビューペインを生やし再利用する() {
        let dir = std::env::temp_dir().join(format!("tako-dispatch-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("b.md"), "# 見出し").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let open = |host: &mut MockHost, path: String, mode: Option<PreviewModeWire>| {
            dispatch(
                host,
                Request::OpenFile {
                    pane: Some(root),
                    path,
                    mode,
                    direction: None,
                    focus: None,
                },
                PaneOrigin::Mcp,
            )
        };
        // 新設: ペインが生え、ターミナルは attach されない。mode は拡張子から code
        let result = open(&mut host, dir.join("a.rs").display().to_string(), None).unwrap();
        let preview_pane = result["pane"].as_u64().unwrap();
        assert_ne!(preview_pane, root);
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(result["mode"].as_str(), Some("code"));
        assert!(host.attached.is_empty(), "プレビューは PTY を起動しない");
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // フォーカスはプレビューペインへ
        assert_eq!(host.ws.active_tab().tree().focused().as_u64(), preview_pane);
        // 再利用: 同タブの 2 ファイル目は同じペインに差し替わる。.md は markdown 既定
        let result = open(&mut host, dir.join("b.md").display().to_string(), None).unwrap();
        assert_eq!(result["pane"].as_u64(), Some(preview_pane));
        assert_eq!(result["created"].as_bool(), Some(false));
        assert_eq!(result["mode"].as_str(), Some("markdown"));
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // mode の明示指定（トグルの CLI / MCP 経路）: 同じファイルを code 表示へ
        let result = open(
            &mut host,
            dir.join("b.md").display().to_string(),
            Some(PreviewModeWire::Code),
        )
        .unwrap();
        assert_eq!(result["mode"].as_str(), Some("code"));
        // list に preview が公開される
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let panes = list["tabs"][0]["panes"].as_array().unwrap();
        let preview = panes
            .iter()
            .find(|p| p["id"].as_u64() == Some(preview_pane))
            .unwrap();
        assert_eq!(preview["preview"]["mode"].as_str(), Some("code"));
        assert!(preview["preview"]["path"]
            .as_str()
            .unwrap()
            .ends_with("b.md"));
        // 存在しないパス・ディレクトリはエラー
        assert!(open(&mut host, dir.join("no-such").display().to_string(), None).is_err());
        assert!(open(&mut host, dir.display().to_string(), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_viewはpdfをページ指定してズームとパンできる() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        host.previews
            .insert(pane, ("/tmp/a.pdf".into(), PreviewModeWire::Pdf));
        host.preview_views
            .insert(pane, tako_core::PreviewViewState::default());

        let result = dispatch(
            &mut host,
            Request::PreviewView {
                pane: Some(pane),
                zoom: Some(150.0),
                zoom_in: false,
                zoom_out: false,
                reset: false,
                page: Some(3),
                pan_x: Some(24.0),
                pan_y: Some(48.0),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();

        assert_eq!(result["pane"], pane);
        assert_eq!(result["zoom"], 150.0);
        assert_eq!(result["page"], 3);
        assert_eq!(result["pan_x"], 24.0);
        assert_eq!(result["pan_y"], 48.0);
    }

    #[test]
    fn preview_outlineは一覧取得と一始まり項目ジャンプを共有する() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        host.previews
            .insert(pane, ("/tmp/a.md".into(), PreviewModeWire::Markdown));
        host.preview_outlines.insert(
            pane,
            tako_core::PreviewOutline::new(vec![
                tako_core::PreviewOutlineItem {
                    title: "概要".into(),
                    level: 1,
                    target: tako_core::PreviewOutlineTarget::MarkdownBlock { block: 0 },
                },
                tako_core::PreviewOutlineItem {
                    title: "詳細".into(),
                    level: 2,
                    target: tako_core::PreviewOutlineTarget::MarkdownBlock { block: 4 },
                },
            ]),
        );

        let listed = dispatch(
            &mut host,
            Request::PreviewOutline {
                pane: Some(pane),
                item: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(listed["outline"].as_array().map(Vec::len), Some(2));
        assert_eq!(listed["outline"][1]["title"], "詳細");

        let jumped = dispatch(
            &mut host,
            Request::PreviewOutline {
                pane: Some(pane),
                item: Some(2),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(jumped["item"], 2);
        assert_eq!(jumped["selected"]["kind"], "markdown_block");
        assert_eq!(
            host.last_outline_target,
            Some(tako_core::PreviewOutlineTarget::MarkdownBlock { block: 4 })
        );
        assert!(dispatch(
            &mut host,
            Request::PreviewOutline {
                pane: Some(pane),
                item: Some(3),
            },
            PaneOrigin::Cli,
        )
        .is_err());
    }

    #[test]
    fn preview_viewは複数のズーム指定を拒否する() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let error = dispatch(
            &mut host,
            Request::PreviewView {
                pane: Some(pane),
                zoom: Some(150.0),
                zoom_in: true,
                zoom_out: false,
                reset: false,
                page: None,
                pan_x: None,
                pan_y: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(error, DispatchError::InvalidParams(_)));
    }

    #[test]
    fn preview_reloadはcore状態を取得変更できる() {
        let mut host = MockHost::new();
        let initial = dispatch(
            &mut host,
            Request::PreviewReload { enabled: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(initial["enabled"], true);

        let changed = dispatch(
            &mut host,
            Request::PreviewReload {
                enabled: Some(false),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(changed["enabled"], false);
        assert!(!host.preview_reload.enabled());
    }

    #[test]
    fn preview_cacheは予算と利用状況を取得変更できる() {
        let mut host = MockHost::new();
        let initial = dispatch(
            &mut host,
            Request::PreviewCache { max_mb: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(initial["max_mb"], 512);
        assert_eq!(initial["used_bytes"], 32 * 1024 * 1024);
        assert_eq!(initial["entries"], 2);

        let changed = dispatch(
            &mut host,
            Request::PreviewCache { max_mb: Some(256) },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(changed["max_mb"], 256);
        assert_eq!(host.preview_cache.max_bytes, 256 * 1024 * 1024);

        let error = dispatch(
            &mut host,
            Request::PreviewCache { max_mb: Some(8) },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(error, DispatchError::InvalidParams(_)));
    }

    #[test]
    fn open_fileのdirection指定は再利用せず分割する() {
        let dir =
            std::env::temp_dir().join(format!("tako-dispatch-open-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let open = |host: &mut MockHost, direction: Option<Direction>| {
            dispatch(
                host,
                Request::OpenFile {
                    pane: Some(root),
                    path: dir.join("a.rs").display().to_string(),
                    mode: None,
                    direction,
                    focus: Some(true),
                },
                PaneOrigin::User,
            )
            .unwrap()
        };
        // 1 枚目（direction なし）でプレビューが生える
        let first = open(&mut host, None)["pane"].as_u64().unwrap();
        // direction 指定（D&D のドロップ位置。FR-3.11）は既存プレビューを再利用しない
        let result = open(&mut host, Some(Direction::Down));
        let second = result["pane"].as_u64().unwrap();
        assert_ne!(second, first, "再利用せず新ペインに開く");
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(host.ws.active_tab().tree().len(), 3);
        assert!(host.attached.is_empty(), "プレビューは PTY を起動しない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview編集の開始適用保存を同じdispatchで操作できる() {
        let dir =
            std::env::temp_dir().join(format!("tako-dispatch-preview-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("a.rs");
        let second = dir.join("b.rs");
        std::fs::write(&first, "before").unwrap();
        std::fs::write(&second, "second").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let opened = dispatch(
            &mut host,
            Request::OpenFile {
                pane: Some(root),
                path: first.display().to_string(),
                mode: Some(PreviewModeWire::Code),
                direction: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let pane = opened["pane"].as_u64().unwrap();
        let started = dispatch(
            &mut host,
            Request::PreviewEdit {
                pane: Some(pane),
                enabled: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(started["editing"].as_bool(), Some(true));
        assert_eq!(started["dirty"].as_bool(), Some(false));

        let applied = dispatch(
            &mut host,
            Request::PreviewApply {
                pane: Some(pane),
                text: "日本語\n".into(),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(applied["dirty"].as_bool(), Some(true));
        let blocked = dispatch(
            &mut host,
            Request::OpenFile {
                pane: Some(pane),
                path: second.display().to_string(),
                mode: None,
                direction: None,
                focus: None,
            },
            PaneOrigin::User,
        );
        assert!(
            blocked.is_err(),
            "未保存変更があるペインの差し替えを拒否する"
        );

        let saved = dispatch(
            &mut host,
            Request::PreviewSave { pane: Some(pane) },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(saved["saved"].as_bool(), Some(true));
        assert_eq!(saved["dirty"].as_bool(), Some(false));
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let preview = list["tabs"][0]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"].as_u64() == Some(pane))
            .unwrap();
        assert_eq!(preview["preview"]["editing"].as_bool(), Some(true));
        assert_eq!(preview["preview"]["dirty"].as_bool(), Some(false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn preview_changelogはプレビューペイン以外を拒否する() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::PreviewChangelog {
                pane: Some(root),
                enabled: Some(true),
                max_count: None,
                expand: None,
            },
            PaneOrigin::Cli,
        );
        assert!(err.is_err());
    }

    #[test]
    fn preview_changelogはプレビューペインで状態取得できる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        host.previews
            .insert(root, ("/tmp/test.rs".into(), PreviewModeWire::Code));
        let result = dispatch(
            &mut host,
            Request::PreviewChangelog {
                pane: Some(root),
                enabled: None,
                max_count: None,
                expand: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["changelog"].as_bool(), Some(false));
    }

    #[test]
    fn tmux_openは存在しないセッションを分割前に弾く() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::TmuxOpen {
                socket: Some(format!("tako-test-no-such-server-{}", std::process::id())),
                session: "no-such-session".into(),
                window: None,
                pane: Some(root),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
        // 分割もセッション起動も起きていない
        assert_eq!(host.ws.active_tab().tree().len(), 1);
        assert!(host.attached.is_empty());
    }

    #[test]
    fn 不正な対象はエラー() {
        let mut host = MockHost::new();
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: None,
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoTargetPane);
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: Some(99999),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::PaneNotFound(99999));
        let err = dispatch(
            &mut host,
            Request::TabSelect { tab: 99999 },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::TabNotFound(99999));
    }

    // --- #109: 複数 master 並行時の caller_role による正しい master 特定 ---

    /// with_test_project の直列化ロック。共有キーを並列テストが同時に
    /// 追加・削除すると解決失敗のレースが起きるため（#120 でテストが増えて顕在化）
    static TEST_PROJECT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// テスト用に一時プロジェクトを projects.yaml に追加し、テスト後に削除する。
    /// config_dir を隔離ディレクトリへ差し替え、実運用の projects.yaml と
    /// その世代バックアップには絶対に触らない（#169）
    fn with_test_project<F: FnOnce()>(f: F) {
        use crate::orchestrator;
        // panic したテストの poison は無視して続行する（後続テストを巻き込まない）
        let _guard = TEST_PROJECT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        orchestrator::test_config_dir_override().get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("tako-dispatch-test-config-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            dir
        });
        let _ = orchestrator::ensure_defaults();
        let key = "_tako_test_109_";
        let mut config = orchestrator::ProjectsConfig::load().unwrap();
        let had = config.projects.contains_key(key);
        if !had {
            // cwd は実在するディレクトリならなんでもよい（/tmp 直書きは Windows に無い）
            let cwd = std::env::temp_dir().display().to_string();
            config.add(key.to_string(), cwd, None);
            config.save().unwrap();
        }
        f();
        if !had {
            let mut config = orchestrator::ProjectsConfig::load().unwrap();
            config.projects.remove(key);
            config.save().unwrap();
        }
    }

    const TEST_PROJECT: &str = "_tako_test_109_";

    /// caller_role 系テストの共通 SpawnParams（stale pane 99999 + effort 明示）
    fn test_spawn_params<'a>(prompt: &'a str, caller_role: Option<&'a str>) -> SpawnParams<'a> {
        SpawnParams {
            project: TEST_PROJECT,
            prompt,
            label: None,
            model: None,
            effort: Some("high"),
            pane: Some(99999),
            tab: None,
            caller_role,
            agent: None,
            caller_pid: None,
            task_type: None,
            account: None,
        }
    }

    /// spawn の起動コマンドが「書きっぱなし」ではなく送達確認フローで送られること（#640）。
    ///
    /// 旧実装は `queue_write(pane, 本文 + \r)` を PTY 起動直後に積むだけで、器（psmux）が
    /// 入力を読み始める前に書いたバイトが落ちても誰も気づけなかった（実機で 5/5 未達）。
    /// **起動コマンドが queue_write に積まれていないこと**まで見て、経路の逆戻りを止める
    #[test]
    fn spawnの起動コマンドは送達確認フローで送られる() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let master = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(master),
                    title: None,
                    role: Some("orchestrator-master:test".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let params = test_spawn_params("テスト", None);
            let result = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params)
                .expect("spawn は成功する");
            let pane = result["pane_id"].as_u64().expect("pane_id が返る");
            let cmd = result["command"]
                .as_str()
                .expect("command が返る")
                .to_string();

            assert_eq!(
                host.command_flows,
                vec![(pane, cmd)],
                "起動コマンドは送達確認つきフローへ登録される"
            );
            assert!(
                host.queued_writes.is_empty(),
                "起動コマンドを書きっぱなしのキューへ積んではいけない（#640 の再発）: {:?}",
                host.queued_writes
                    .iter()
                    .map(|(p, d)| (p, d.len()))
                    .collect::<Vec<_>>()
            );
        });
    }

    /// 送達確認フローには**改行を含めない**（Enter は分離して送る）。
    /// 本文と Enter を 1 回の書き込みにまとめると、届いた分だけが実行される
    #[test]
    fn spawnの送達確認フローには改行を含めない() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let master = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(master),
                    title: None,
                    role: Some("orchestrator-master:test".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト", None),
            )
            .expect("spawn は成功する");
            let (_, cmd) = host.command_flows.first().expect("フローが 1 件登録される");
            assert!(
                !cmd.contains('\r') && !cmd.contains('\n'),
                "本文に改行を混ぜない: {cmd:?}"
            );
        });
    }

    /// 複数 master が存在するとき、caller_role の suffix で正しい master のタブに
    /// worker が配置されることを検証する（#109 の根本修正）
    #[test]
    fn spawn_caller_roleで正しいmasterを特定する() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let tab1_pane = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(tab1_pane),
                    title: None,
                    role: Some("orchestrator-master:fable".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let tab2_result = dispatch(
                &mut host,
                Request::TabNew {
                    title: None,
                    focus: None,
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let tab2_pane = tab2_result["pane"].as_u64().unwrap();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(tab2_pane),
                    title: None,
                    role: Some("orchestrator-master:aram".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            // stale な pane を caller_pane として渡し、caller_role でフォールバック
            let result = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト", Some("master:aram")),
            );
            let value = result.expect("caller_role フォールバックで spawn 成功するべき");
            assert_eq!(
                value["spawned_by"].as_u64().unwrap(),
                tab2_pane,
                "worker は caller_role が示す master:aram のペイン（tab2）から分割されるべき"
            );
        });
    }

    /// caller_role がない場合の旧来フォールバック（最初の master を使う）が維持されること
    #[test]
    fn spawn_caller_roleなしはフォールバックで最初のmasterを使う() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let tab1_pane = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(tab1_pane),
                    title: None,
                    role: Some("orchestrator-master".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            let result = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト", None),
            );
            let value = result.expect("caller_role なしでも既存フォールバックで成功するべき");
            assert_eq!(value["spawned_by"].as_u64().unwrap(), tab1_pane);
        });
    }

    /// caller_role の suffix が prefix 付きで正しくマッチすること
    #[test]
    fn spawn_caller_roleのsuffix抽出が正しい() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let tab1_pane = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(tab1_pane),
                    title: None,
                    role: Some("orchestrator-master:hck".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let tab2_result = dispatch(
                &mut host,
                Request::TabNew {
                    title: None,
                    focus: None,
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let tab2_pane = tab2_result["pane"].as_u64().unwrap();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(tab2_pane),
                    title: None,
                    role: Some("orchestrator-master:fable".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            let result = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト", Some("master:hck")),
            )
            .unwrap();
            assert_eq!(result["spawned_by"].as_u64().unwrap(), tab1_pane);

            let result = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト 2", Some("master:fable")),
            )
            .unwrap();
            assert_eq!(result["spawned_by"].as_u64().unwrap(), tab2_pane);
        });
    }

    // --- #120: worker エージェント種別（claude / codex / agy） ---

    fn pane_count(host: &MockHost) -> usize {
        host.workspace()
            .tabs()
            .iter()
            .map(|t| t.tree().panes().len())
            .sum()
    }

    /// 不正なエージェント種別はペイン分割の前に拒否される（ペインが生えない）
    #[test]
    fn spawn_不正なagent種別はエラーでペインが生えない() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let root = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(root),
                    title: None,
                    role: Some("orchestrator-master".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let before = pane_count(&host);

            let mut params = test_spawn_params("テスト", None);
            params.agent = Some("gemini");
            let err = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params)
                .expect_err("不正 agent はエラーになるべき");
            assert!(
                err.to_string().contains("claude / codex / agy"),
                "対応一覧つきの診断: {err}"
            );
            assert_eq!(pane_count(&host), before, "エラー時にペインが生えない");
        });
    }

    /// agent=codex / agy の spawn は各 CLI のコマンドを組み立て、応答に agent を含む
    #[test]
    fn spawn_agent種別ごとのコマンド組み立て() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let root = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(root),
                    title: None,
                    role: Some("orchestrator-master".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            let mut params = test_spawn_params("テスト", None);
            params.agent = Some("codex");
            params.model = Some("gpt-5.6-terra");
            params.effort = Some("medium");
            let result = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params).unwrap();
            assert_eq!(result["agent"], "codex");
            let cmd = result["command"].as_str().unwrap();
            assert!(cmd.contains(" codex"), "codex を起動する: {cmd}");
            assert!(cmd.contains("--model gpt-5.6-terra"), "{cmd}");
            assert!(
                cmd.contains("model_reasoning_effort=medium"),
                "effort は codex の config へ写像: {cmd}"
            );
            assert_eq!(
                result["command"], result["claude_command"],
                "旧フィールド名の互換を維持"
            );

            // agy は effort を無視し、モデル表示名をクオートして渡す
            let mut params = test_spawn_params("テスト", None);
            params.agent = Some("agy");
            params.model = Some("Gemini 3.5 Flash (High)");
            params.effort = Some("high");
            let result = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params).unwrap();
            assert_eq!(result["agent"], "agy");
            let cmd = result["command"].as_str().unwrap();
            assert!(cmd.contains(" agy"), "{cmd}");
            assert!(cmd.contains("--model 'Gemini 3.5 Flash (High)'"), "{cmd}");
            assert!(!cmd.contains("effort"), "agy に effort は渡さない: {cmd}");
        });
    }

    /// agent 省略時は claude で従来のコマンド形式（回帰なし）
    #[test]
    fn spawn_agent省略はclaude既定() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let root = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(root),
                    title: None,
                    role: Some("orchestrator-master".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            let result = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                test_spawn_params("テスト", None),
            )
            .unwrap();
            assert_eq!(result["agent"], "claude");
            let cmd = result["command"].as_str().unwrap();
            assert!(cmd.contains(" claude"), "{cmd}");
            assert!(cmd.contains("--effort high"), "{cmd}");
        });
    }

    // --- TreeFolder テスト (#134) ---

    #[test]
    fn tree_folder_追加と一覧と削除() {
        let mut host = MockHost::new();
        let pane = host.root_pane();

        // 一覧: 初期は空
        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 0);

        // 追加: /tmp（存在するディレクトリ）
        let added = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(added["status"], "added");

        // 一覧: 1 件
        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 1);

        // 二重追加: already_exists
        let dup = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(dup["status"], "already_exists");

        // 削除
        let removed = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "remove".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(removed["status"], "removed");

        // 一覧: 0 件に戻る
        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn tree_folder_存在しないパスはエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/nonexistent_path_xyz_12345".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn tree_folder_ファイルはエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        // /etc/hosts は macOS に存在するファイル
        let result = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/etc/hosts".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn tree_folder_相対パスはエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("relative/path".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn tree_folder_削除対象なしはエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "remove".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    // --- #171: 重複排除・プルーニング ---

    #[test]
    fn tree_folder_symlink経由の重複追加は1エントリに畳まれる() {
        // macOS: /tmp は /private/tmp へのシンボリックリンク
        let mut host = MockHost::new();
        let pane = host.root_pane();

        // /tmp で追加（canonicalize → /private/tmp）
        let r1 = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r1["status"], "added");

        // /private/tmp で追加（同じ正規パス → already_exists）
        let r2 = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/private/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r2["status"], "already_exists");

        // list は 1 件
        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 1);

        // 表示名は basename（/private/tmp の file_name = "tmp"）
        let folder_path = list["folders"][0].as_str().unwrap();
        let basename = std::path::Path::new(folder_path)
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert_eq!(basename, "tmp");
    }

    #[test]
    fn tree_folder_symlink経由でも削除できる() {
        let mut host = MockHost::new();
        let pane = host.root_pane();

        // /tmp で追加
        dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some("/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // /private/tmp で削除（同じ正規パスなので成功する）
        let removed = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "remove".into(),
                path: Some("/private/tmp".into()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(removed["status"], "removed");

        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn tree_folder_実体消失エントリはlistで自動プルーニングされる() {
        let mut host = MockHost::new();
        let pane = host.root_pane();

        // 一時ディレクトリを作って追加
        let tmp = std::env::temp_dir().join("tako_test_prune_171");
        std::fs::create_dir_all(&tmp).unwrap();
        dispatch(
            &mut host,
            Request::TreeFolder {
                action: "add".into(),
                path: Some(tmp.display().to_string()),
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // 追加されたことを確認
        let list = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list["folders"].as_array().unwrap().len(), 1);

        // ディレクトリを削除
        std::fs::remove_dir_all(&tmp).unwrap();

        // list で自動プルーニング → 0 件に
        let list2 = dispatch(
            &mut host,
            Request::TreeFolder {
                action: "list".into(),
                path: None,
                tab: None,
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(list2["folders"].as_array().unwrap().len(), 0);
    }

    // --- worker_status の collect / finish 分離（#181 → #168 で OffloadJob へ一本化）---
    // 以下は backend_session = None / session_id = None / tmux_session = None に固定し、
    // claude CLI / tmux のサブプロセスを一切呼ばない決定的な範囲だけを検証する

    #[test]
    fn collect_worker_status_ctxがui状態を写し取る() {
        let host = MockHost::new();
        let pane = host.root_pane();
        let ctx = collect_worker_status_ctx(&host, pane);
        assert!(ctx.pane_exists);
        // MockHost は backend / セッション画面を持たない
        assert_eq!(ctx.backend_session, None);
        assert!(ctx.live_tail.is_none());
        // 存在しないペイン
        let gone = collect_worker_status_ctx(&host, 999_999);
        assert!(!gone.pane_exists);
    }

    // --- #662: ダイアログ操作の入力検証と到達解決 ---

    /// choice と answers の同時指定は「どのダイアログに答えるか」が決まらない。
    /// 黙ってどちらかを選ぶと誤爆になるので拒否する
    #[test]
    fn respondはchoiceとanswersの同時指定を拒否する() {
        let host = MockHost::new();
        let answer = crate::protocol::DialogAnswer {
            question: None,
            option: Some("1".into()),
            options: None,
        };
        let err = dispatch_orchestrator_respond(
            &host,
            host.root_pane(),
            Some("1"),
            Some(&[answer]),
            false,
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("同時に指定できない"),
            "err={err:?}"
        );
    }

    #[test]
    fn respondはchoiceもanswersも無ければ拒否する() {
        let host = MockHost::new();
        let err = dispatch_orchestrator_respond(&host, host.root_pane(), None, None, false, None)
            .unwrap_err();
        assert!(err.to_string().contains("answers"), "err={err:?}");
    }

    #[test]
    fn respondは空のanswersを拒否する() {
        let host = MockHost::new();
        let err =
            dispatch_orchestrator_respond(&host, host.root_pane(), None, Some(&[]), false, None)
                .unwrap_err();
        assert!(err.to_string().contains("answers が空"), "err={err:?}");
    }

    /// キー名の妥当性はキーを 1 つも撃つ前に検査する
    /// （途中まで送ってから失敗すると TUI が中途半端な状態で残る）
    #[test]
    fn send_keysは不明なキー名を送信前に弾く() {
        let host = MockHost::new();
        let err = dispatch_send_keys(
            &host,
            Some(host.root_pane()),
            &["enter".into(), "nosuchkey".into()],
            None,
            None,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nosuchkey"), "msg={msg}");
        // 使えるキー名の一覧を添える（master が撃ち直せるように）
        assert!(msg.contains("enter"), "msg={msg}");
    }

    #[test]
    fn send_keysは空のkeysを拒否する() {
        let host = MockHost::new();
        let err = dispatch_send_keys(&host, Some(host.root_pane()), &[], None, None).unwrap_err();
        assert!(err.to_string().contains("keys が空"), "err={err:?}");
    }

    /// MockHost は `TerminalSession` を持たないので in-process では届かない。
    /// バックエンドのヒントも無いので「セッションが無い」を返す
    /// （「到達手段が無い」と混同しない = #519 の区別を保つ）
    #[test]
    fn ダイアログ到達はセッション不在を構造化して返す() {
        let host = MockHost::new();
        // `DialogReach` は `&dyn DetachedAccess` を持ち Debug を実装できないので
        // unwrap_err ではなく match で取り出す
        match resolve_dialog_reach(&host, host.root_pane()) {
            Err(e) => assert!(e.to_string().contains("セッション"), "err={e:?}"),
            Ok(_) => panic!("MockHost はセッションを持たないので到達できないはず"),
        }
    }

    /// detached バックエンド（tmux 系）へのキー名の写し。
    /// in-process 側と語彙が違う（tmux は `Enter` / `S-Up` / `C-c`）
    #[test]
    fn detachedのキー名はtmux記法へ写す() {
        assert_eq!(detached_key_name("enter"), "Enter");
        assert_eq!(detached_key_name("escape"), "Escape");
        assert_eq!(detached_key_name("down"), "Down");
        assert_eq!(detached_key_name("backtab"), "BTab");
        assert_eq!(detached_key_name("ctrl-c"), "C-c");
        assert_eq!(detached_key_name("shift-up"), "S-Up");
        assert_eq!(detached_key_name("alt-v"), "M-v");
        // 1 文字リテラルはそのまま（tmux の send-keys が受ける）
        assert_eq!(detached_key_name("2"), "2");
    }

    // --- #592: 器を持たないペイン（Windows の backend=none）の pane→session 解決 ---

    #[test]
    fn auto_resolve_sessionはpane_pidが無ければ解決しない() {
        // 器も pane_pid も無い = 辿る起点が無い（旧 Windows の状態）
        assert_eq!(auto_resolve_session(None, None), None);
    }

    /// 実機確認用（#592）。稼働中の tako ペインのシェル pid を渡すと、
    /// worker_status が `status_source: agents-auto` + 非 null の
    /// `resolved_session_id` を返すことを、本番と同じ経路で確かめる。
    ///
    /// ```text
    /// TAKO_TEST_PANE_PID=<ペインのシェル pid> cargo test -p tako-control \
    ///   --lib 実機_pane_pidでagents_autoに解決する -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "実 claude + 稼働中の tako ペインが要る（手動実行専用）"]
    fn 実機_pane_pidでagents_autoに解決する() {
        let pid: u32 = std::env::var("TAKO_TEST_PANE_PID")
            .expect("TAKO_TEST_PANE_PID に対象ペインのシェル pid を渡す")
            .trim()
            .parse()
            .expect("pid は数値");
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: None,
                full_screen: None,
                has_running_children: crate::agents::has_running_children_of_pid(pid),
                pane_pid: Some(pid),
            },
            None,
            None,
        )
        .expect("worker_status が返る");
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        assert_eq!(
            v["status_source"], "agents-auto",
            "pid 祖先辿りで一次シグナル（claude agents）へ到達する"
        );
        assert!(
            v["resolved_session_id"].is_string(),
            "session_id が解決される"
        );
    }

    #[test]
    fn finish_worker_statusがペイン不在でgoneを返す() {
        let ctx = WorkerStatusCtx {
            pane_id: 0,
            pane_exists: false,
            backend_session: None,
            live_tail: None,
            full_screen: None,
            has_running_children: false,
            pane_pid: None,
        };
        let v = finish_worker_status(ctx, None, None).unwrap();
        assert_eq!(v["status"], "gone");
        assert_eq!(v["status_source"], "none");
        assert!(v["recent_output"].is_null());
    }

    #[test]
    fn finish_worker_statusが画面からidle_busyを推定する() {
        // ❯ プロンプト行 = idle（backend 無しなので status_source は screen）
        let idle = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("done\n❯ ".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(idle["status"], "idle");
        assert_eq!(idle["status_source"], "screen");
        // busy マーカー行 = busy
        let busy = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("Thinking…\nesc to interrupt".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(busy["status"], "busy");
        // 画面なし = unknown のまま
        let unknown = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: None,
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(unknown["status"], "unknown");
    }

    #[test]
    fn finish_worker_statusがエラー停止をerrorへ細分類する() {
        // #157: idle（❯ プロンプト表示）+ 画面に API Error → status=error + error オブジェクト
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "  ⎿  API Error: Connection closed mid-response. The response above may be incomplete.\n\n❯ ".into(),
                ),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"]["kind"], "api_error");
        assert_eq!(v["error"]["recommended_action"], "resume");
        assert!(v["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("Connection closed mid-response"));

        // usage limit 停止（codex の実採取文言）
        let limited = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "■ You've hit your usage limit. Upgrade to Pro or try again at 4:24 AM.\n\n› 1. Switch to gpt-5.4-mini".into(),
                ),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(limited["status"], "error");
        assert_eq!(limited["error"]["kind"], "usage_limit");
        assert_eq!(limited["error"]["recommended_action"], "wait_reset");

        // 正常 idle では error が付かない（誤発火しない）
        let clean = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("done\n❯ ".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(clean["status"], "idle");
        assert!(clean["error"].is_null());

        // busy 中はエラー行が見えていても busy のまま（自動リトライへの誤検知防止）
        let retrying = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "  ⎿  API Error (Connection error.) · Retrying in 4 seconds… (attempt 3/10)\nesc to interrupt".into(),
                ),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(retrying["status"], "busy");
        assert!(retrying["error"].is_null());
    }

    #[test]
    fn finish_worker_statusがevents配列を返す() {
        // #243: 質問画面で question イベント
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "テストを追加しますか？\n❯ 1. はい\n  2. いいえ\n❯ \n──────".into(),
                ),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["status"], "idle");
        let events = v["events"].as_array().expect("events は配列");
        assert!(
            events.iter().any(|e| e["kind"] == "question"),
            "question イベントが含まれる: {events:?}"
        );

        // モデル切替画面で model_switched + context_high（ctx 65%）
        let v2 = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "⎿ Claude Opus 4.6 limit reached, now using Claude Sonnet 4.5\n\n❯ \n──────"
                        .into(),
                ),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        // この画面は error 判定されない（limit reached, now using は除外）
        assert_eq!(v2["status"], "idle");
        let events2 = v2["events"].as_array().expect("events は配列");
        assert!(
            events2.iter().any(|e| e["kind"] == "model_switched"),
            "model_switched: {events2:?}"
        );

        // 正常完了画面では events が空
        let v3 = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 0,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("done\n❯ ".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v3["status"], "idle");
        let events3 = v3["events"].as_array().expect("events は配列");
        assert!(events3.is_empty(), "正常完了で events が空: {events3:?}");
    }

    // --- #289: バックグラウンドシェルが IDLE 検知をブロックする問題の根治 ---
    // apply_worker_status_corrections を直接呼び、agents の初期状態を制御する

    fn resolved(status: &str, source: &str, has_children: bool) -> ResolvedWorkerStatus {
        ResolvedWorkerStatus {
            status: status.into(),
            status_source: source.into(),
            ctx_percent: None,
            resolved_sid: if source.starts_with("agents") {
                Some("test-session".into())
            } else {
                None
            },
            pane_exists: true,
            has_children,
            recent_output: Some("done\n❯ \n──────".into()),
            full_screen: None,
            tmux_session: None,
            ..Default::default()
        }
    }

    #[test]
    fn issue289_agents_idleはhas_childrenで覆されない() {
        let v = apply_worker_status_corrections(resolved("idle", "agents", true)).unwrap();
        assert_eq!(v["status"], "idle");
        assert_eq!(v["has_running_children"], true);
        assert_eq!(v["status_source"], "agents");
    }

    #[test]
    fn issue289_agents_auto経路でもidleが尊重される() {
        let v = apply_worker_status_corrections(resolved("idle", "agents-auto", true)).unwrap();
        assert_eq!(v["status"], "idle");
        assert_eq!(v["status_source"], "agents-auto");
    }

    #[test]
    fn issue289_screenフォールバックではhas_childrenが効く() {
        let v = apply_worker_status_corrections(resolved("idle", "screen", true)).unwrap();
        assert_eq!(v["status"], "busy");
        assert_eq!(v["status_source"], "screen");
    }

    #[test]
    fn issue289_agents_busyはhas_childrenに関係なくbusy維持() {
        let mut r = resolved("busy", "agents", true);
        r.recent_output = Some("Thinking…\nesc to interrupt".into());
        let v = apply_worker_status_corrections(r).unwrap();
        assert_eq!(v["status"], "busy");
    }

    #[test]
    fn issue289_screen_looks_busyはagents_idleでも効く() {
        let mut r = resolved("idle", "agents", false);
        r.recent_output = Some("Thinking…\nesc to interrupt".into());
        let v = apply_worker_status_corrections(r).unwrap();
        assert_eq!(v["status"], "busy");
    }

    #[test]
    fn issue289_unknownでは画面が判断不能なときだけhas_childrenが効く() {
        // #571: 画面が入力欄を映していれば idle。エージェント TUI 自身が常に
        // 「子プロセス」なので、これを優先すると worker は永久に完了しない
        let mut inconclusive = resolved("unknown", "screen", true);
        inconclusive.recent_output = Some("... 出力の途中 ...".into());
        let v = apply_worker_status_corrections(inconclusive).unwrap();
        assert_eq!(
            v["status"], "busy",
            "画面から判断できなければ busy 側に倒す"
        );
    }

    // --- #571: agents 解決に失敗した worker が永久 busy になる問題の根治 ---

    #[test]
    fn issue571_claudeの実status語彙をbusyへ正規化する() {
        // 2026-07-27 実測: claude agents --json は idle / busy を返す。
        // busy が unknown に落ちると一次シグナルを捨てて画面推定に回ってしまう
        assert_eq!(normalize_agent_status("busy"), "busy");
        assert_eq!(normalize_agent_status("idle"), "idle");
        // 旧語彙も引き続き受ける
        assert_eq!(normalize_agent_status("active"), "busy");
        assert_eq!(normalize_agent_status("waiting_for_input"), "waiting");
        assert_eq!(normalize_agent_status("gone"), "gone");
        // 未知の値は unknown（画面推定へフォールバック）
        assert_eq!(normalize_agent_status("nonsense"), "unknown");
    }

    #[test]
    fn issue571_画面が入力欄ならhas_childrenがあってもidle() {
        // claude / codex / agy の TUI プロセスはペインシェルの子なので常に has_children=true。
        // 旧実装はこれで busy に上書きし、agents 解決失敗時に WORKER_IDLE が永久に出なかった
        let v = apply_worker_status_corrections(resolved("unknown", "screen", true)).unwrap();
        assert_eq!(v["status"], "idle");
        assert_eq!(v["has_running_children"], true);
    }

    #[test]
    fn issue571_画面がbusyならhas_childrenの有無によらずbusy() {
        let mut r = resolved("unknown", "screen", false);
        r.recent_output = Some(
            "✽ Misting… (10m 49s · ↓ 35.8k tokens)\n──────\n❯ \n──────\n  ctx 23%\n  5h 20%\n  7d 16%\n  auto mode on"
                .into(),
        );
        let v = apply_worker_status_corrections(r).unwrap();
        assert_eq!(v["status"], "busy", "スピナーが見えていれば busy が勝つ");
    }

    #[test]
    fn issue571_agentsが状態を返せなければstatus_sourceはscreenへ降格する() {
        // agents 一覧にセッションが無い（= "gone" → pane 健在で "unknown" へ降格）ケース。
        // source を agents のままにすると watch が画面推定を一次シグナル扱いして
        // idle 連続 3 回で確定してしまう（本来は 8 回）
        let mut r = resolved("gone", "agents-auto", true);
        r.resolved_sid = Some("test-session".into());
        let v = apply_worker_status_corrections(r).unwrap();
        assert_eq!(v["status_source"], "screen");
        assert_eq!(v["status"], "idle");

        // agents コマンド自体が失敗（unknown）でも同じ
        let v2 = apply_worker_status_corrections(resolved("unknown", "agents", true)).unwrap();
        assert_eq!(v2["status_source"], "screen");
    }

    // --- #571 E2E: 実 tmux + 実 claude で busy → idle の検知を通しで確認する ---
    //
    // 本番事故の条件（tako 側プロセスが別アカウントの `CLAUDE_CONFIG_DIR` を継承した状態）を
    // 再現し、既定 config dir で走る worker の完了を watch が検知できることを確かめる。
    // 手動実行:
    //
    // ```sh
    // cargo test -p tako-control --lib issue571_e2e -- --ignored --nocapture --test-threads=1
    // ```
    //
    // 前提: `claude` CLI がログイン済み / `tmux` がある / ネットワーク接続。
    // tmux ソケットと data ディレクトリ（worker レジストリ）はどちらも隔離するので
    // 本番の tako / tmux には触れない

    const E2E_SOCKET_571: &str = "tako-e2e-571";

    fn e2e_571_base() -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/private/tmp/tako-e2e-571-{}", std::process::id()))
    }

    /// テスト用の一時ディレクトリだけを消す（実ディレクトリの巻き添え削除を構造で防ぐ）。
    /// `/private/tmp` 直下の `tako-e2e-571-<pid>` 以外は panic して止める
    fn remove_e2e_571_dir(dir: &std::path::Path) {
        let allowed = dir.parent() == Some(std::path::Path::new("/private/tmp"))
            && dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tako-e2e-571-"));
        assert!(
            allowed,
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    struct E2e571Guard {
        dir: std::path::PathBuf,
    }

    impl Drop for E2e571Guard {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-L", E2E_SOCKET_571, "kill-server"])
                .output();
            remove_e2e_571_dir(&self.dir);
        }
    }

    #[test]
    #[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
    fn issue571_e2e_実claudeのbusyからidleへの遷移をwatchが検知する() {
        use std::time::{Duration, Instant};

        let base = e2e_571_base();
        remove_e2e_571_dir(&base);
        let work = base.join("work");
        let data = base.join("data");
        let alt_config = base.join("claude-alt");
        for d in [&work, &data, &alt_config] {
            std::fs::create_dir_all(d).expect("一時ディレクトリを作れる");
        }
        let guard = E2e571Guard { dir: base.clone() };

        // 隔離: tmux ソケットと data ディレクトリ（worker レジストリ / accounts.yaml）
        std::env::set_var("TAKO_TMUX_SOCKET", E2E_SOCKET_571);
        std::env::set_var("TAKO_DATA_DIR", &data);
        // 本番事故の再現: tako 側プロセスが別アカウントの config dir を継承している。
        // この状態で `claude agents --json` を素で実行すると worker が一覧に出ない
        std::env::set_var(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV, &alt_config);

        // worker は既定 config dir で走らせるので、事前信頼もそちらへ書く
        crate::claude_tui::ensure_trusted(&work.display().to_string()).expect("事前信頼を書ける");

        let _ = std::process::Command::new("tmux")
            .args(["-L", E2E_SOCKET_571, "kill-server"])
            .output();
        let session = "w571";
        let status = std::process::Command::new("tmux")
            // tmux サーバーへ汚染を伝播させない（worker は既定 config dir で動く必要がある）
            .env_remove(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV)
            .args([
                "-L",
                E2E_SOCKET_571,
                "new-session",
                "-d",
                "-s",
                session,
                "-x",
                "100",
                "-y",
                "35",
                "-c",
                work.to_str().expect("テストパスは UTF-8"),
                "unset CLAUDE_CONFIG_DIR; exec claude --model haiku",
            ])
            .status()
            .expect("tmux を実行できる");
        assert!(status.success(), "tmux new-session が失敗した");

        let mut host = MockHost::new();
        let pane = host.root_pane();
        host.backend_sessions.insert(pane, session.to_string());

        // プロンプト送達。ここから worker は busy になる
        let report = crate::claude_tui::deliver_via_tmux(
            Some(E2E_SOCKET_571),
            session,
            "What is 40 + 2? Reply with only the answer spelled out in English words, lowercase.",
            true,
        )
        .expect("送達が完了する");
        assert!(
            report.verified,
            "プロンプトが入力欄へ反映される: {report:?}"
        );

        let opts = crate::orchestrator::wait::WatchOptions {
            pane_id: pane,
            session_id: None,
            tmux_session: Some(session.to_string()),
            timeout: Some(Duration::from_secs(60)),
            initial_delay: Duration::ZERO,
            interval: Duration::from_secs(2),
        };

        // ① busy を実際に観測する（「最初から idle」で素通りしていないことの担保）。
        //    判定はワッチ結果の後で行う（本命は ② なので、失敗時にそちらを先に見せる）
        let started = Instant::now();
        let mut saw_busy = None;
        while started.elapsed() < Duration::from_secs(30) {
            let v = dispatch(
                &mut host,
                Request::OrchestratorWorkerStatus {
                    pane_id: Some(pane),
                    session_id: None,
                    tmux_session: Some(session.to_string()),
                    worker: None,
                },
                PaneOrigin::Cli,
            )
            .expect("worker_status が引ける");
            if v["status"] == "busy" {
                saw_busy = Some(v);
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        // ② busy → idle の遷移を watch が検知して完了すること（本命）
        let outcome = {
            let mut exec =
                |req: Request| dispatch(&mut host, req, PaneOrigin::Cli).map_err(|e| e.to_string());
            crate::orchestrator::wait::wait_for_worker(&mut exec, &opts, None)
        };
        drop(guard);
        assert!(
            matches!(
                outcome,
                crate::orchestrator::wait::WatchOutcome::Idle { .. }
            ),
            "busy → idle 遷移で Idle を返す（修正前は永久 busy のまま Timeout になる）: {outcome:?}"
        );

        let busy = saw_busy.expect("送達後に busy を観測できる");
        assert_eq!(
            busy["status_source"], "agents-auto",
            "busy 中も config dir を跨いで claude セッションを解決できている（#571 の根因）: {busy}"
        );
    }

    #[test]
    fn issue571_agentsが状態を返せたらstatus_sourceは維持される() {
        for source in ["agents", "agents-auto"] {
            for status in ["idle", "busy", "waiting"] {
                let v = apply_worker_status_corrections(resolved(status, source, false)).unwrap();
                assert_eq!(v["status_source"], source, "{status} / {source}");
            }
        }
    }

    #[test]
    fn tail_joinが末尾空行を刈り30行に制限する() {
        let mut lines: Vec<String> = (1..=40).map(|i| format!("L{i}")).collect();
        lines.push(String::new());
        lines.push(String::new());
        let out = tail_join(lines);
        assert!(out.starts_with("L11"), "先頭 10 行が刈られる: {out}");
        assert!(out.ends_with("L40"), "末尾の空行が刈られる: {out}");
        assert_eq!(out.lines().count(), 30);
    }

    // --- #123 / #193: OrchestratorSelf + OrchestratorHandoff ---

    #[test]
    fn orchestrator_selfがmaster_paneを返す() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane),
                title: None,
                role: Some("orchestrator-master:test".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        let result = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(pane),
                caller_role: Some("master:test".into()),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(result["pane_id"].as_u64(), Some(pane));
        assert_eq!(result["profile"].as_str(), Some("test"));
        assert!(result["ctx_threshold"].as_u64().is_some());
    }

    #[test]
    fn orchestrator_selfがcaller_roleから自動解決する() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane),
                title: None,
                role: Some("orchestrator-master".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // pane を渡さず caller_role だけで解決
        let result = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: None,
                caller_role: Some("master:".into()),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        );
        // 「master:」は空 suffix → default → pane_id が一致
        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["pane_id"].as_u64(), Some(pane));
        assert_eq!(val["profile"].as_str(), Some("default"));
    }

    #[test]
    fn orchestrator_handoffがファイル不在でエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane),
                title: None,
                role: Some("orchestrator-master".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        let result = dispatch(
            &mut host,
            Request::OrchestratorHandoff {
                pane: Some(pane),
                caller_role: Some("master:".into()),
                tab: None,
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        );
        assert!(result.is_err(), "handoff ファイル不在はエラー");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("handoff ファイルが見つからない"), "{err}");
    }

    #[test]
    fn find_master_paneがsuffix一致を優先する() {
        let mut host = MockHost::new();
        let tab1_pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(tab1_pane),
                title: None,
                role: Some("orchestrator-master:alpha".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2_pane = tab2["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(tab2_pane),
                title: None,
                role: Some("orchestrator-master:beta".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        let found = find_master_pane_strict(host.workspace(), "beta", Some("master:beta"));
        assert_eq!(
            found.ok().map(|(_, p)| p.as_u64()),
            Some(tab2_pane),
            "suffix beta のペインが返る"
        );

        let found_alpha = find_master_pane_strict(host.workspace(), "alpha", Some("master:alpha"));
        assert_eq!(
            found_alpha.ok().map(|(_, p)| p.as_u64()),
            Some(tab1_pane),
            "suffix alpha のペインが返る"
        );
    }

    // --- #210: 同一プロファイル複数 master で self が自分を返す ---

    #[test]
    fn orchestrator_self_同一profile_2体が自分を返す() {
        let mut host = MockHost::new();
        let master_a = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(master_a),
                title: None,
                role: Some("orchestrator-master:exam".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        let tab2 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let master_b = tab2["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(master_b),
                title: None,
                role: Some("orchestrator-master:exam".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // master A が caller_pane=master_a で self を呼ぶ → 自分を返す
        let result_a = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(master_a),
                caller_role: Some("master:exam".into()),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(result_a["pane_id"].as_u64(), Some(master_a));

        // master B が caller_pane=master_b で self を呼ぶ → 自分を返す
        let result_b = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(master_b),
                caller_role: Some("master:exam".into()),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(result_b["pane_id"].as_u64(), Some(master_b));
    }

    #[test]
    fn orchestrator_self_stale_pane_mapで旧pane_idを解決する() {
        let mut host = MockHost::new();
        let actual_pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(actual_pane),
                title: None,
                role: Some("orchestrator-master:exam".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // orphan 復元で旧 pane 99999 → 実 pane へのマッピングを登録
        let stale_id = 99999_u64;
        host.stale_pane_map
            .insert(PaneId::from_raw(stale_id), PaneId::from_raw(actual_pane));

        // 旧 pane ID で self を呼ぶ → stale_pane_map 経由で実ペインに解決
        let result = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(stale_id),
                caller_role: Some("master:exam".into()),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(
            result["pane_id"].as_u64(),
            Some(actual_pane),
            "stale pane ID が新 pane ID に解決される"
        );
    }

    #[test]
    fn orchestrator_spawn_stale_paneから分割元を解決する() {
        with_test_project(|| {
            let mut host = MockHost::new();
            let master_pane = host.root_pane();
            dispatch(
                &mut host,
                Request::Title {
                    pane: Some(master_pane),
                    title: None,
                    role: Some("orchestrator-master".into()),
                },
                PaneOrigin::Cli,
            )
            .unwrap();

            let stale_id = 88888_u64;
            host.stale_pane_map
                .insert(PaneId::from_raw(stale_id), PaneId::from_raw(master_pane));

            let params = SpawnParams {
                project: TEST_PROJECT,
                prompt: "hello",
                label: None,
                model: None,
                effort: Some("max"),
                pane: Some(stale_id),
                tab: None,
                caller_role: Some("master:"),
                agent: None,
                caller_pid: None,
                task_type: None,
                account: None,
            };
            let result = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params);
            assert!(
                result.is_ok(),
                "stale pane からの spawn が成功する: {:?}",
                result.err()
            );
            let val = result.unwrap();
            assert_eq!(
                val["spawned_by"].as_u64(),
                Some(master_pane),
                "spawned_by が実ペインを指す"
            );
        });
    }

    #[test]
    fn テーマのstatus_set_toggleが機能する() {
        use tako_core::theme::ThemeMode;
        let mut host = MockHost::new();
        // status: 既定はダーク
        let v = dispatch(
            &mut host,
            Request::Theme {
                action: None,
                mode: None,
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["theme"], "dark");
        // set light → host へ反映
        let v = dispatch(
            &mut host,
            Request::Theme {
                action: Some("set".into()),
                mode: Some("light".into()),
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        // cfg!(test) では save しないため v["theme"] は "dark" のまま
        // ホスト状態の反映は v["mode"] で確認する
        assert_eq!(v["mode"], "light");
        assert_eq!(host.theme_mode, ThemeMode::Light);
        // toggle → dark へ反転
        let v = dispatch(
            &mut host,
            Request::Theme {
                action: Some("toggle".into()),
                mode: None,
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["mode"], "dark");
        assert_eq!(host.theme_mode, ThemeMode::Dark);
        // 不明 mode はプリセット名として受容される（エラーにならない）
        // mode 無しの set はエラー
        assert!(dispatch(
            &mut host,
            Request::Theme {
                action: Some("set".into()),
                mode: None,
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        )
        .is_err());
    }

    #[test]
    fn 表示言語のstatus_setが機能する() {
        use tako_core::i18n::{Lang, LangSetting};
        let mut host = MockHost::new();
        // status: 既定は system
        let v = dispatch(
            &mut host,
            Request::Lang {
                action: None,
                value: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["language"], "system");
        assert_eq!(v["available"].as_array().unwrap().len(), 3);
        // set en → host へ設定値と解決済み表示言語が渡る
        let v = dispatch(
            &mut host,
            Request::Lang {
                action: Some("set".into()),
                value: Some("en".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["language"], "en");
        assert_eq!(v["resolved"], "en");
        assert_eq!(host.lang_setting, LangSetting::En);
        assert_eq!(host.lang_resolved, Some(Lang::En));
        // set ja
        let v = dispatch(
            &mut host,
            Request::Lang {
                action: Some("set".into()),
                value: Some("ja".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["resolved"], "ja");
        assert_eq!(host.lang_setting, LangSetting::Ja);
        assert_eq!(host.lang_resolved, Some(Lang::Ja));
        // set の不明 value / value 無し / 不明 action はエラー
        assert!(dispatch(
            &mut host,
            Request::Lang {
                action: Some("set".into()),
                value: Some("fr".into()),
            },
            PaneOrigin::Cli,
        )
        .is_err());
        assert!(dispatch(
            &mut host,
            Request::Lang {
                action: Some("set".into()),
                value: None,
            },
            PaneOrigin::Cli,
        )
        .is_err());
        assert!(dispatch(
            &mut host,
            Request::Lang {
                action: Some("toggle".into()),
                value: None,
            },
            PaneOrigin::Cli,
        )
        .is_err());
    }

    #[test]
    fn 利用制限サービスのstatus_setが機能する() {
        use tako_core::LimitService;
        let mut host = MockHost::new();
        // status: 既定は claude
        let v = dispatch(
            &mut host,
            Request::LimitService {
                action: None,
                service: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["limit_service"], "claude");
        assert_eq!(v["available"].as_array().unwrap().len(), 3);
        // set codex → host へ反映
        let v = dispatch(
            &mut host,
            Request::LimitService {
                action: Some("set".into()),
                service: Some("codex".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["limit_service"], "codex");
        assert_eq!(host.limit_service, LimitService::Codex);
        // set agy
        let v = dispatch(
            &mut host,
            Request::LimitService {
                action: Some("set".into()),
                service: Some("agy".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["limit_service"], "agy");
        assert_eq!(host.limit_service, LimitService::Agy);
        // 不明 service はエラー
        assert!(dispatch(
            &mut host,
            Request::LimitService {
                action: Some("set".into()),
                service: Some("unknown".into()),
            },
            PaneOrigin::Cli,
        )
        .is_err());
        // service 無しの set はエラー
        assert!(dispatch(
            &mut host,
            Request::LimitService {
                action: Some("set".into()),
                service: None,
            },
            PaneOrigin::Cli,
        )
        .is_err());
        // refresh はデフォルト実装で null 値を返す
        let v = dispatch(
            &mut host,
            Request::LimitService {
                action: Some("refresh".into()),
                service: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(v["claude"].is_object());
        assert!(v["codex"].is_object());
        assert_eq!(v["agy"]["status"], "unsupported");
    }

    // --- OpenDir / OpenRemote / SshHosts / RecentItems テスト (#20) ---

    #[test]
    fn open_dir_存在しないパスはエラー() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::OpenDir {
                path: "/nonexistent/path/12345".into(),
                focus: None,
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[test]
    fn open_dir_存在するパスは新タブを作成() {
        let mut host = MockHost::new();
        let dir = std::env::temp_dir();
        let result = dispatch(
            &mut host,
            Request::OpenDir {
                path: dir.display().to_string(),
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["tab"].as_u64().is_some());
        assert!(result["pane"].as_u64().is_some());
    }

    #[test]
    fn open_remote_は新タブを作成() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::OpenRemote {
                host: "nonexistent-host".into(),
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["tab"].as_u64().is_some());
        assert!(result["pane"].as_u64().is_some());
    }

    #[test]
    fn ssh_hosts_は配列を返す() {
        let mut host = MockHost::new();
        let result = dispatch(&mut host, Request::SshHosts, PaneOrigin::Cli).unwrap();
        assert!(result["hosts"].is_array());
    }

    #[test]
    fn recent_items_list_とclear() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::RecentItems {
                action: "list".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["entries"].is_array());

        let result = dispatch(
            &mut host,
            Request::RecentItems {
                action: "clear".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["cleared"], true);
    }

    #[test]
    fn recent_items_不明なactionはエラー() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::RecentItems {
                action: "invalid".into(),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn setup_runはanswersをargvでなくstdinからcliへ渡す() {
        use std::os::unix::fs::PermissionsExt as _;

        let script = std::env::temp_dir().join(format!(
            "tako-dispatch-setup-{}-{}.sh",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             payload=$(/bin/cat)\n\
             if [ \"$1\" != setup ] || [ \"$2\" != --yes ] || \
                [ \"$3\" != --answers ] || [ \"$4\" != - ] || \
                [ \"$payload\" != '{\"selected_agent\":\"claude\"}' ]; then\n\
               exit 2\n\
             fi\n\
             printf 'dispatch-setup-ok'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let result =
            run_setup_cli(script.to_str().unwrap(), r#"{"selected_agent":"claude"}"#).unwrap();
        let _ = std::fs::remove_file(&script);
        assert_eq!(result["completed"], true);
        assert_eq!(result["output"], "dispatch-setup-ok");
    }

    /// 受け入れ条件 1: stale env + 同一 role 3 ペイン + 実ペイン role=null で
    /// pane env が現存する場合に正しい pane を返す（pid 祖先辿りは tmux 不在で
    /// フォールバック。env の現存 pane が第 2 解決手段として正しく機能することを検証）。
    /// Issue #288 の実事故再現: pane 400 が role=null なのに role 検索で 443 を誤返答した構図
    #[test]
    fn orchestrator_self_stale_env_同一role3体_roleなし実ペインで正しいpaneを返す() {
        let mut host = MockHost::new();

        // master A: tab 1（実ペイン = role なし。実事故の pane 400 相当）
        let actual_pane = host.root_pane();

        // master B: tab 2（role あり。実事故の pane 443 相当）
        let tab2 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let master_b = tab2["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(master_b),
                title: None,
                role: Some("orchestrator-master:fable".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // master C: tab 3（role あり。同一 role の 2 体目）
        let tab3 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let master_c = tab3["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(master_c),
                title: None,
                role: Some("orchestrator-master:fable".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // master D: tab 4（role あり。同一 role の 3 体目）
        let tab4 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let master_d = tab4["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(master_d),
                title: None,
                role: Some("orchestrator-master:fable".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // 状態確認: actual_pane は role=null、master_b/c/d は同一 role
        assert!(
            host.ws
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes())
                .find(|p| p.id().as_u64() == actual_pane)
                .unwrap()
                .role()
                .is_none(),
            "actual_pane は role=null であること"
        );

        // ケース 1: pane env が現存 ID（actual_pane）を持つ場合 → pid 解決失敗でも
        // pane env のフォールバックで actual_pane を返す（role 検索に落ちない）
        let result = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(actual_pane),
                caller_role: Some("master:fable".into()),
                caller_pid: Some(99999), // tmux 不在で pid 解決は失敗する
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(
            result["pane_id"].as_u64(),
            Some(actual_pane),
            "pane env が現存する場合は role 検索に落ちず正しい pane を返す"
        );

        // ケース 2: pane env が stale（現存しない ID 305）→ stale map もなし →
        // role 検索に落ちるが、同一 role が 3 体あるため曖昧エラーになる
        // （旧実装では先頭の master_b を黙って返していた = 実事故の再現）
        let result_stale = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(305), // 現存しない stale ID
                caller_role: Some("master:fable".into()),
                caller_pid: Some(99999), // pid 解決も失敗
            },
            PaneOrigin::Mcp,
        );
        assert!(
            result_stale.is_err(),
            "stale env + pid 解決失敗 + 同一 role 3 体 → 曖昧エラーになること（旧実装では master_b を誤返答）"
        );
        let err_msg = result_stale.unwrap_err().to_string();
        assert!(
            err_msg.contains("複数ペインに存在"),
            "エラーメッセージに「複数ペインに存在」を含むこと: {err_msg}"
        );
    }

    /// 受け入れ条件 2: 曖昧 role のみで確定不能な場合にエラーとなる
    /// find_master_pane_strict が複数マッチ時に先頭を返さず曖昧エラーを返すことの検証
    #[test]
    fn find_master_pane_strict_複数マッチで曖昧エラー() {
        let mut host = MockHost::new();

        // 同一 role の master を 2 体作成
        let pane_a = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane_a),
                title: None,
                role: Some("orchestrator-master:dup".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        let tab2 = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let pane_b = tab2["pane"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane_b),
                title: None,
                role: Some("orchestrator-master:dup".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // suffix 一致（"dup"）で 2 体マッチ → 曖昧エラー
        let result = find_master_pane_strict(host.workspace(), "dup", Some("master:dup"));
        assert!(result.is_err(), "複数マッチで Err を返すこと");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(&pane_a.to_string()) && err_msg.contains(&pane_b.to_string()),
            "エラーメッセージに両方のペイン ID を含むこと: {err_msg}"
        );

        // suffix なし（prefix フォールバック）でも 2 体マッチ → 曖昧エラー
        let result_fb = find_master_pane_strict(host.workspace(), "", None);
        assert!(
            result_fb.is_err(),
            "prefix フォールバックでも複数マッチは Err"
        );

        // 1 体だけなら成功
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane_b),
                title: None,
                role: Some("worker:test".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result_single = find_master_pane_strict(host.workspace(), "dup", Some("master:dup"));
        assert!(result_single.is_ok(), "1 体なら成功");
        assert_eq!(result_single.unwrap().1.as_u64(), pane_a);
    }

    fn mcp_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-mcp-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn setup_mcp_direct_新規登録() {
        let dir = mcp_test_dir("direct-new");
        let scope = McpScope::Project(dir.clone());
        let target = dir.join(".mcp.json");
        setup_mcp_direct("/usr/local/bin/tako", &scope).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["tako"]["command"],
            "/usr/local/bin/tako"
        );
        assert_eq!(content["mcpServers"]["tako"]["type"], "stdio");
    }

    #[test]
    fn setup_mcp_direct_既存キーを保全() {
        let dir = mcp_test_dir("direct-merge");
        let target = dir.join(".mcp.json");
        let existing = serde_json::json!({
            "mcpServers": {
                "other-server": { "command": "other", "args": [] }
            }
        });
        std::fs::write(&target, serde_json::to_string_pretty(&existing).unwrap()).unwrap();
        let scope = McpScope::Project(dir.clone());
        setup_mcp_direct("/usr/local/bin/tako", &scope).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["tako"]["command"],
            "/usr/local/bin/tako"
        );
        assert_eq!(
            content["mcpServers"]["other-server"]["command"], "other",
            "既存の other-server が保全されること"
        );
    }

    #[test]
    fn read_mcp_registration_既存登録を読める() {
        let dir = mcp_test_dir("read-reg");
        let target = dir.join(".mcp.json");
        let exe = std::env::current_exe().unwrap();
        let existing = serde_json::json!({
            "mcpServers": {
                "tako": {
                    "type": "stdio",
                    "command": exe.display().to_string(),
                    "args": ["mcp", "serve"]
                }
            }
        });
        std::fs::write(&target, serde_json::to_string_pretty(&existing).unwrap()).unwrap();
        let scope = McpScope::Project(dir.clone());
        let cmd = read_mcp_registration(&scope);
        assert_eq!(cmd, Some(exe.display().to_string()));
    }

    #[test]
    fn setup_mcp_direct_死んだパスを上書き() {
        let dir = mcp_test_dir("repair");
        let target = dir.join(".mcp.json");
        let dead_path = "/nonexistent/old/path/tako";
        let existing = serde_json::json!({
            "mcpServers": {
                "tako": {
                    "type": "stdio",
                    "command": dead_path,
                    "args": ["mcp", "serve"]
                }
            }
        });
        std::fs::write(&target, serde_json::to_string_pretty(&existing).unwrap()).unwrap();
        let scope = McpScope::Project(dir.clone());
        setup_mcp_direct("/new/stable/tako", &scope).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["tako"]["command"], "/new/stable/tako");
    }

    #[test]
    fn clean_legacy_settings_json_takoキーを掃除() {
        let dir = mcp_test_dir("legacy");
        let settings = dir.join("settings.json");
        let legacy = serde_json::json!({
            "mcpServers": {
                "tako": { "command": "/old/tako", "args": ["mcp", "serve"] },
                "other": { "command": "other" }
            },
            "otherKey": true
        });
        std::fs::write(&settings, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        // HOME を一時的に dir に向ける
        let orig_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(
            dir.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let cleaned = clean_legacy_settings_json();
        assert!(cleaned, "tako キーがある場合は true を返す");

        let content: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(".claude").join("settings.json")).unwrap(),
        )
        .unwrap();
        assert!(
            content.get("mcpServers").unwrap().get("tako").is_none(),
            "tako キーが除去されていること"
        );
        assert!(
            content.get("mcpServers").unwrap().get("other").is_some(),
            "other キーは保全されること"
        );
        assert!(
            content.get("otherKey").is_some(),
            "他のトップレベルキーは保全されること"
        );

        // 2 回目は false
        let cleaned2 = clean_legacy_settings_json();
        assert!(!cleaned2, "既に掃除済みなら false");

        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn resolve_tako_binary_はapplicationsを優先() {
        // /Applications/tako.app が存在する場合のみこのテストが意味を持つ
        if std::path::Path::new(STABLE_APP_BINARY).is_file() {
            assert_eq!(resolve_tako_binary(), STABLE_APP_BINARY);
        }
    }

    #[test]
    fn run_interactiveはsplitとtitleとmetaを設定する() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::RunInteractive {
                pane: Some(root.as_u64()),
                tab: None,
                command: "sudo systemctl start foo".into(),
                input_hint: Some("sudo パスワード".into()),
                direction: None,
                ratio: None,
                auto_close: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();

        let pane_id = result["pane"].as_u64().unwrap();
        assert_eq!(result["status"], "running");
        assert_eq!(result["auto_close"], "success");

        // 新ペインが生成された
        assert!(host.attached.contains(&pane_id));

        // タイトルにヒントが設定された
        let new_pane_id = PaneId::from_raw(pane_id);
        let pane = host
            .ws
            .active_tab()
            .tree()
            .get(new_pane_id)
            .expect("新ペインが存在する");
        assert_eq!(pane.title(), Some("(!) sudo パスワード"));

        // interactive_meta が設定された
        let (ac, cmd) = pane.interactive_meta().expect("interactive_meta がある");
        assert_eq!(ac, "success");
        assert_eq!(cmd, "sudo systemctl start foo");
    }

    #[test]
    fn run_interactive_statusはrunningを返す_session未接続時() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::RunInteractive {
                pane: Some(root.as_u64()),
                tab: None,
                command: "read -p 'input: ' val".into(),
                input_hint: None,
                direction: Some(Direction::Down),
                ratio: Some(0.4),
                auto_close: Some("never".into()),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        let pane_id = result["pane"].as_u64().unwrap();

        // session() が None のため、status は running（マーカー未検出）
        let status = dispatch(
            &mut host,
            Request::RunInteractiveStatus {
                pane: pane_id,
                no_wait: false,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(status["status"], "running");
        assert_eq!(status["pane"], pane_id);
    }

    #[test]
    fn run_interactiveのauto_close不正値はエラー() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::RunInteractive {
                pane: Some(root.as_u64()),
                tab: None,
                command: "echo hi".into(),
                input_hint: None,
                direction: None,
                ratio: None,
                auto_close: Some("invalid".into()),
            },
            PaneOrigin::Mcp,
        );
        assert!(result.is_err());
    }

    #[test]
    fn exit_markerは行頭でも途中でも検知できる() {
        assert_eq!(find_exit_marker(&["__TAKO_EXIT=0".into()]), Some(0));
        assert_eq!(
            find_exit_marker(&["続行しますか? (y/n): __TAKO_EXIT=1".into()]),
            Some(1)
        );
        assert_eq!(find_exit_marker(&["  __TAKO_EXIT=42  ".into()]), Some(42));
        assert_eq!(find_exit_marker(&["just some output".into()]), None);
        assert_eq!(
            find_exit_marker(&[
                "__TAKO_EXIT=0".into(),
                "some output".into(),
                "prompt: __TAKO_EXIT=2".into(),
            ]),
            Some(2),
        );
    }

    #[test]
    fn run_interactiveはコマンドをspawn_commandで渡す() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::RunInteractive {
                pane: Some(root.as_u64()),
                tab: None,
                command: r#"read "ans?input: ""#.into(),
                input_hint: None,
                direction: None,
                ratio: None,
                auto_close: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        let pane_id = result["pane"].as_u64().unwrap();
        let opts = host.attached_options.get(&pane_id).expect("options 記録");
        let cmd = opts.command.as_ref().expect("command が設定されている");
        assert_run_pane_shape(cmd, r#"read "ans?input: ""#);

        // #453 の回帰検査（POSIX）: 複合シェルコードは /bin/sh -c の引数へ。
        // program 1 語に詰めると login_shell_command のクォートで 127 即死する
        #[cfg(unix)]
        {
            assert_eq!(cmd.program, "/bin/sh");
            assert_eq!(cmd.args.first().map(String::as_str), Some("-c"));
            let sh_code = cmd.args.get(1).expect("-c の引数");
            assert!(sh_code.contains("__TAKO_EXIT="), "{sh_code}");
            assert!(sh_code.contains(r#"read "ans?input: ""#), "{sh_code}");
            assert!(
                sh_code.ends_with("read -r __TAKO_DUMMY__ 2>/dev/null || true"),
                "{sh_code}"
            );
        }
    }

    /// 実行ペインの組み立てそのものは境界 B1（`platform::shell`）の責務なので、
    /// dispatch 側で見るのは**渡したコマンド文字列が正しいか**だけにする（#525）。
    /// 「その組み立てが実際に走るか」は `tako-core` 側で実 PowerShell / 実 sh に対して検証する
    fn assert_run_pane_shape(cmd: &SpawnCommand, expected_command: &str) {
        let want = tako_core::platform::shell::run_pane_command(expected_command, "__TAKO_EXIT=");
        assert_eq!(cmd.program, want.program);
        assert_eq!(cmd.args, want.args);
    }

    #[test]
    fn runはコマンドをプラットフォームの実行ペイン構造でspawnする() {
        // #453: Run ペインの SpawnCommand が /bin/sh -c 構造であること
        //（program に複合コマンド全文を詰めると login_shell_command のクォートで
        // 1 コマンド名扱いになり command not found で即死する）。
        // #525: Windows は PowerShell の -EncodedCommand になる
        let dir = std::env::temp_dir().join(format!("tako-run-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.command");
        std::fs::write(&file, "#!/usr/bin/env bash\necho hello\n").unwrap();

        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::Run {
                path: file.display().to_string(),
                pane: Some(root.as_u64()),
                tab: None,
                profile: None,
                command: None,
                direction: None,
                ratio: None,
                auto_close: None,
                focus: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        let pane_id = result["pane"].as_u64().unwrap();
        let opts = host.attached_options.get(&pane_id).expect("options 記録");
        let cmd = opts.command.as_ref().expect("command が設定されている");
        // 拡張子既定（`.command`）から組み立てたコマンドが、そのまま実行ペインへ渡る
        let expected_command = tako_core::merged_defaults(&Default::default())
            .get("command")
            .expect("組み込み既定に command がある")
            .replace("${fileBase}", "hello.command");
        assert_run_pane_shape(cmd, &expected_command);

        #[cfg(unix)]
        {
            assert_eq!(cmd.program, "/bin/sh");
            assert_eq!(cmd.args.first().map(String::as_str), Some("-c"));
            let sh_code = cmd.args.get(1).expect("-c の引数");
            assert!(sh_code.starts_with("bash hello.command"), "{sh_code}");
            assert!(sh_code.contains("__TAKO_EXIT="), "{sh_code}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // === 複数ウィンドウ（Issue #339） ===

    /// #584: 最小化 / 最大化 / 復元は UI 層への依頼として積まれる。
    /// window 省略でアクティブウィンドウ、明示指定でそのウィンドウ、
    /// 存在しない ID はエラー（無言で別ウィンドウを操作しない）
    #[test]
    fn windowの表示状態操作はui層へ依頼される() {
        use crate::protocol::WindowStateOp;
        let mut host = MockHost::new();
        let w1 = host.workspace().active_window_id().as_u64();

        // window 省略 = アクティブウィンドウ
        let r = dispatch(
            &mut host,
            Request::WindowMinimize { window: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["window"].as_u64(), Some(w1));
        assert_eq!(r["state"].as_str(), Some("minimize"));

        // 別ウィンドウを作り、明示指定でそちらを操作する
        let r = dispatch(&mut host, Request::WindowNew { tab: None }, PaneOrigin::Cli).unwrap();
        let w2 = r["window"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::WindowMaximize { window: Some(w1) },
            PaneOrigin::Cli,
        )
        .unwrap();
        dispatch(
            &mut host,
            Request::WindowRestore { window: None },
            PaneOrigin::Cli,
        )
        .unwrap();

        assert_eq!(
            host.window_state_ops,
            vec![
                (w1, WindowStateOp::Minimize),
                (w1, WindowStateOp::Maximize),
                // 直前の WindowNew で w2 がアクティブなので、省略は w2 に解決される
                (w2, WindowStateOp::Restore),
            ]
        );

        // 存在しないウィンドウ ID はエラーで、依頼は積まれない
        let before = host.window_state_ops.len();
        assert!(dispatch(
            &mut host,
            Request::WindowMinimize {
                window: Some(9_999)
            },
            PaneOrigin::Cli,
        )
        .is_err());
        assert_eq!(host.window_state_ops.len(), before);
    }

    /// #657 のテスト用メニュー構成（Windows 版の並びを模したもの）
    fn sample_menu_bar() -> crate::protocol::MenuBarSnapshot {
        use crate::protocol::{MenuBarSnapshot, MenuItemSnapshot as I, MenuSnapshot};
        MenuBarSnapshot {
            in_window: true,
            open: None,
            menus: vec![
                MenuSnapshot {
                    name: "ファイル".into(),
                    items: vec![
                        I::Action {
                            label: "新規タブ".into(),
                            action: "tako::NewTab".into(),
                            shortcut: Some("Ctrl+Shift+T".into()),
                        },
                        I::Separator,
                        I::Action {
                            label: "設定…".into(),
                            action: "tako::OpenSettings".into(),
                            shortcut: None,
                        },
                    ],
                },
                MenuSnapshot {
                    name: "表示".into(),
                    items: vec![
                        I::Action {
                            label: "コマンドパレット…".into(),
                            action: "tako::OpenCommandPalette".into(),
                            shortcut: None,
                        },
                        I::Submenu {
                            label: "パネル".into(),
                            items: vec![I::Action {
                                label: "git ビュー".into(),
                                action: "tako::ShowGitPanel".into(),
                                shortcut: None,
                            }],
                        },
                    ],
                },
            ],
        }
    }

    /// #657: list はメニュー構成をそのまま返し、サブメニューも入れ子で出す
    #[test]
    fn menu_listがメニュー構成を返す() {
        let mut host = MockHost::new();
        let r = dispatch(&mut host, Request::MenuList, PaneOrigin::Cli).unwrap();
        assert_eq!(r["in_window"], true);
        assert_eq!(r["menus"][0]["name"], "ファイル");
        assert_eq!(r["menus"][0]["items"][0]["action"], "tako::NewTab");
        assert_eq!(r["menus"][0]["items"][0]["shortcut"], "Ctrl+Shift+T");
        assert_eq!(r["menus"][0]["items"][1]["kind"], "separator");
        assert_eq!(r["menus"][1]["items"][1]["kind"], "submenu");
        assert_eq!(
            r["menus"][1]["items"][1]["items"][0]["action"],
            "tako::ShowGitPanel"
        );
    }

    /// #657: open は名前の部分一致で解決し、UI 層へ添字で依頼する
    #[test]
    fn menu_openは名前を添字へ解決する() {
        let mut host = MockHost::new();
        let r = dispatch(
            &mut host,
            Request::MenuOpen {
                menu: "表示".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["index"], 1);
        assert_eq!(host.menu_ops, vec![crate::protocol::MenuOp::Open(1)]);

        // 存在しない名前は候補つきで拒否し、依頼は積まれない
        let before = host.menu_ops.len();
        let e = dispatch(
            &mut host,
            Request::MenuOpen {
                menu: "存在しない".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(e.to_string().contains("ファイル"), "候補を出す: {e}");
        assert_eq!(host.menu_ops.len(), before);
    }

    /// #657: in-window メニューバーが無い環境（macOS）では open / close を理由つきで拒否。
    /// **「機能が無い」ではなく「なぜ使えないか + 代わりに何が使えるか」を返す**
    #[test]
    fn in_windowメニューが無い環境ではopenを理由つきで拒否する() {
        let mut host = MockHost::new();
        host.menu_bar.in_window = false;
        let e = dispatch(
            &mut host,
            Request::MenuOpen {
                menu: "ファイル".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(e.to_string().contains("menu invoke"), "代替を案内する: {e}");
        assert!(host.menu_ops.is_empty());

        // invoke は macOS でも動く（アクションの発火は OS メニューと同じ経路）
        dispatch(
            &mut host,
            Request::MenuInvoke {
                path: "新規タブ".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.menu_ops,
            vec![crate::protocol::MenuOp::Invoke("tako::NewTab".into())]
        );
    }

    /// #657: invoke のパス解決（メニュー省略 / 2 段 / サブメニュー 3 段）
    #[test]
    fn menu_invokeのパス解決() {
        let mut host = MockHost::new();
        for (path, action) in [
            ("新規タブ", "tako::NewTab"),
            ("ファイル/新規タブ", "tako::NewTab"),
            ("表示/パネル/git ビュー", "tako::ShowGitPanel"),
            // 部分一致でも 1 つに絞れれば通る
            ("パレット", "tako::OpenCommandPalette"),
        ] {
            let r = dispatch(
                &mut host,
                Request::MenuInvoke { path: path.into() },
                PaneOrigin::Cli,
            )
            .unwrap_or_else(|e| panic!("{path} を解決できない: {e}"));
            assert_eq!(r["action"], action, "path={path}");
        }
        // 見つからないパスはエラー（発火させない）
        let before = host.menu_ops.len();
        assert!(dispatch(
            &mut host,
            Request::MenuInvoke {
                path: "存在しない項目".into()
            },
            PaneOrigin::Cli,
        )
        .is_err());
        assert_eq!(host.menu_ops.len(), before);
    }

    /// #657: 区切り線は invoke の対象にならない（押せない行を発火させない）
    #[test]
    fn menu_invokeは区切り線を対象にしない() {
        let mut host = MockHost::new();
        assert!(dispatch(
            &mut host,
            Request::MenuInvoke {
                path: "ファイル/".into()
            },
            PaneOrigin::Cli,
        )
        .is_err());
        assert!(host.menu_ops.is_empty());
    }

    /// #657: メニュー定義が空（GUI 未起動）なら invoke は理由つきで失敗する
    #[test]
    fn メニュー定義が無いときのinvokeは失敗する() {
        let mut host = MockHost::new();
        host.menu_bar = crate::protocol::MenuBarSnapshot::default();
        let e = dispatch(
            &mut host,
            Request::MenuInvoke {
                path: "新規タブ".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(e.to_string().contains("メニュー定義"), "{e}");
    }

    #[test]
    fn window系の一連操作とlist反映() {
        let mut host = MockHost::new();
        let w1 = host.workspace().active_window_id().as_u64();
        // 新規タブ付きの新ウィンドウ
        let r = dispatch(&mut host, Request::WindowNew { tab: None }, PaneOrigin::Cli).unwrap();
        let w2 = r["window"].as_u64().unwrap();
        let t2 = r["tab"].as_u64().unwrap();
        assert!(r["pane"].as_u64().is_some());
        assert_ne!(w2, w1);
        // 一覧: 2 ウィンドウ + 新ウィンドウがアクティブ
        let r = dispatch(&mut host, Request::WindowList, PaneOrigin::Cli).unwrap();
        assert_eq!(r["active_window"].as_u64(), Some(w2));
        assert_eq!(r["windows"].as_array().unwrap().len(), 2);
        // タブを w1 へ移動 → w2 が空になり除去される
        let r = dispatch(
            &mut host,
            Request::WindowMoveTab {
                tab: t2,
                window: w1,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["closed_window"].as_u64(), Some(w2));
        // list に windows / active_window / tabs[].window が載る（後方互換の追加フィールド）
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["active_window"].as_u64(), Some(w1));
        assert_eq!(list["windows"].as_array().unwrap().len(), 1);
        let tabs = list["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert!(tabs.iter().all(|t| t["window"].as_u64() == Some(w1)));
        // タブ分離（tab 指定の WindowNew）
        let r = dispatch(
            &mut host,
            Request::WindowNew { tab: Some(t2) },
            PaneOrigin::Cli,
        )
        .unwrap();
        let w3 = r["window"].as_u64().unwrap();
        assert_eq!(r["tab"].as_u64(), Some(t2));
        assert_eq!(r["closed_window"], Value::Null);
        // focus で w1 へ戻す
        dispatch(
            &mut host,
            Request::WindowFocus { window: w1 },
            PaneOrigin::Cli,
        )
        .unwrap();
        let r = dispatch(&mut host, Request::WindowList, PaneOrigin::Cli).unwrap();
        assert_eq!(r["active_window"].as_u64(), Some(w1));
        // close で合流（タブは残る）
        let tab_count = host.workspace().tabs().len();
        let r = dispatch(
            &mut host,
            Request::WindowClose { window: w3 },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["moved_tabs"].as_array().unwrap().len(), 1);
        assert_eq!(host.workspace().tabs().len(), tab_count);
        // 存在しないウィンドウはエラー / 最後の 1 ウィンドウは閉じられない
        assert!(dispatch(
            &mut host,
            Request::WindowFocus { window: 99999 },
            PaneOrigin::Cli
        )
        .is_err());
        assert!(dispatch(
            &mut host,
            Request::WindowClose { window: w1 },
            PaneOrigin::Cli
        )
        .is_err());
    }

    #[test]
    fn listのsurfaceは全ウィンドウの表示タブを前面扱いする() {
        let mut host = MockHost::new();
        // タブ 2 枚目を作って新ウィンドウへ分離 → 両タブとも表示中になる
        let r = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let t2 = r["tab"].as_u64().unwrap();
        dispatch(
            &mut host,
            Request::WindowNew { tab: Some(t2) },
            PaneOrigin::Cli,
        )
        .unwrap();
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        for tab in list["tabs"].as_array().unwrap() {
            for pane in tab["panes"].as_array().unwrap() {
                assert_eq!(
                    pane["surface"].as_str(),
                    Some("foreground"),
                    "タブ {} のペインは表示中のはず",
                    tab["id"]
                );
            }
        }
    }

    // --- #390: worker レジストリ ---

    #[test]
    fn issue390_spawnがレジストリ登録しcloseでclosedになる() {
        use crate::orchestrator::registry::WorkerRegistry;
        with_test_project(|| {
            let mut host = MockHost::new();
            let master = host.root_pane();
            let params = SpawnParams {
                project: TEST_PROJECT,
                prompt: "registry test",
                label: Some("reg-test"),
                model: None,
                effort: Some("high"),
                pane: Some(master),
                tab: None,
                caller_role: None,
                agent: None,
                caller_pid: None,
                task_type: None,
                account: None,
            };
            let val = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params).unwrap();
            let worker_id = val["worker_id"]
                .as_str()
                .expect("spawn 応答に worker_id が入る")
                .to_string();
            let worker_pane = val["pane_id"].as_u64().unwrap();

            let reg = WorkerRegistry::load().unwrap();
            let (_, entry) = reg.resolve(&worker_id).expect("レジストリに登録済み");
            assert!(entry.is_active());
            assert_eq!(entry.pane, worker_pane);
            assert_eq!(entry.project, TEST_PROJECT);
            assert_eq!(entry.label.as_deref(), Some("reg-test"));
            assert_eq!(entry.agent, "claude");
            // pane→worker の逆引き（フォールバック解決の中核）
            assert!(reg.find_active_by_pane(worker_pane).is_some());

            // 明示 close → closed へ（PTY 死亡はここを通らない = 追跡は維持される）
            dispatch(
                &mut host,
                Request::Close {
                    pane: Some(worker_pane),
                    force: true,
                },
                PaneOrigin::Cli,
            )
            .unwrap();
            let reg = WorkerRegistry::load().unwrap();
            let (_, entry) = reg.resolve(&worker_id).unwrap();
            assert_eq!(entry.status, "closed");
            assert_eq!(entry.close_reason.as_deref(), Some("explicit_close"));
            assert!(reg.find_active_by_pane(worker_pane).is_none());
        });
    }

    #[test]
    fn issue390_resolve_worker_queryが解決と補完をする() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.next_id += 1;
            reg.workers.insert(
                "q7701".into(),
                WorkerEntry {
                    pane: 7701,
                    tmux_session: Some("tako-q7701".into()),
                    session_id: Some("sid-7701".into()),
                    agent: "claude".into(),
                    status: "active".into(),
                    spawned_at: crate::sessions::now_iso(),
                    ..Default::default()
                },
            );
        })
        .unwrap();

        // worker 指定: pane / session / tmux をレジストリから解決
        let q = resolve_worker_query(None, Some("q7701"), None, None).unwrap();
        assert_eq!(q.pane_id, 7701);
        assert_eq!(q.session_id.as_deref(), Some("sid-7701"));
        assert_eq!(q.tmux_session.as_deref(), Some("tako-q7701"));

        // pane 指定: 欠けたキーだけ補完、明示指定は優先
        let q = resolve_worker_query(Some(7701), None, Some("explicit-sid".into()), None).unwrap();
        assert_eq!(q.pane_id, 7701);
        assert_eq!(q.session_id.as_deref(), Some("explicit-sid"));
        assert_eq!(q.tmux_session.as_deref(), Some("tako-q7701"));

        // 未登録 pane: 補完なしでそのまま
        let q = resolve_worker_query(Some(60660), None, None, None).unwrap();
        assert_eq!(q.pane_id, 60660);
        assert!(q.session_id.is_none() && q.tmux_session.is_none());

        // どちらも無し / 未知 worker はエラー
        assert!(resolve_worker_query(None, None, None, None).is_err());
        assert!(resolve_worker_query(None, Some("zz-unknown"), None, None).is_err());
    }

    #[test]
    fn issue390_finish_worker_statusがprompt未達を検知する() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        // spawn から十分経過（猶予 240 秒超）した claude worker。session_id 未検出
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.workers.insert(
                "q7801".into(),
                WorkerEntry {
                    pane: 7801,
                    agent: "claude".into(),
                    status: "active".into(),
                    spawned_at: "2026-01-01T00:00:00Z".into(),
                    ..Default::default()
                },
            );
        })
        .unwrap();

        // idle 画面（welcome のまま）→ prompt_undelivered イベント発火
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 7801,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("Welcome to Claude Code\n❯ ".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["worker_id"], "q7801");
        assert_eq!(v["prompt_delivery"], "undelivered");
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"prompt_undelivered"),
            "events に prompt_undelivered: {kinds:?}"
        );

        // busy 画面なら発火しない（誤検知防止: 検出遅延の可能性）
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 7801,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("Thinking…\nesc to interrupt".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["prompt_delivery"], "pending");
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(
            !kinds.contains(&"prompt_undelivered"),
            "busy 中は発火しない"
        );

        // session_id を渡した照会（agents 解決相当）は delivered へ倒れ、
        // レジストリへも lazy 昇格される
        WorkerRegistry::mutate_at(&path, |reg| {
            if let Some(e) = reg.workers.get_mut("q7801") {
                e.tmux_session = Some("tako-q7801".into());
            }
        })
        .unwrap();
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 7801,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("done\n❯ ".into()),
                full_screen: None,
                has_running_children: false,
                pane_pid: None,
            },
            Some("sid-7801-detected"),
            None,
        )
        .unwrap();
        assert_eq!(v["prompt_delivery"], "delivered");
        let reg = WorkerRegistry::load().unwrap();
        let entry = &reg.workers["q7801"];
        assert_eq!(
            entry.session_id.as_deref(),
            Some("sid-7801-detected"),
            "解決済み session_id がレジストリへ書き戻される"
        );
        assert!(entry.prompt_delivered_at.is_some());
    }

    #[test]
    fn issue390_agent突然死をagent_deadイベントで検知しresumeを提示する() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        // session_id 記録済み（= 一度は走った）claude worker
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.workers.insert(
                "q7802".into(),
                WorkerEntry {
                    pane: 7802,
                    agent: "claude".into(),
                    status: "active".into(),
                    session_id: Some("sid-7802".into()),
                    cwd: Some("/tmp/proj".into()),
                    spawned_at: crate::sessions::now_iso(),
                    ..Default::default()
                },
            );
        })
        .unwrap();

        let dead_ctx = |has_children: bool| WorkerStatusCtx {
            pane_id: 7802,
            pane_exists: true,
            backend_session: None,
            // SIGSEGV 後のシェルプロンプト画面（claude TUI の ❯ ではない）
            live_tail: Some("zsh: segmentation fault  claude\n% ".into()),
            full_screen: None,
            has_running_children: has_children,
            pane_pid: None,
        };

        // 子プロセスなし → agent_dead イベント + resume_command
        let v = finish_worker_status(dead_ctx(false), None, None).unwrap();
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"agent_dead"), "events: {kinds:?}");
        assert_eq!(
            v["resume_command"],
            "cd '/tmp/proj' && claude --resume sid-7802"
        );
        let dead_ev = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["kind"] == "agent_dead")
            .unwrap();
        assert_eq!(
            dead_ev["resume_command"],
            "cd '/tmp/proj' && claude --resume sid-7802"
        );
        assert_eq!(dead_ev["recommended_action"], "resume_session");
        // session 検出済みなので prompt_undelivered は出ない
        assert!(!kinds.contains(&"prompt_undelivered"));

        // 実行中子プロセスあり（生存中）→ 発火しない
        let v = finish_worker_status(dead_ctx(true), None, None).unwrap();
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(!kinds.contains(&"agent_dead"), "生存中は発火しない");
    }

    #[test]
    fn issue390_wait_for_workerがagent_dead2連続でworker_deadを確定する() {
        use crate::orchestrator::wait::{wait_for_worker, WatchOptions, WatchOutcome};
        let mut calls = 0u32;
        let mut exec = |_req: Request| -> Result<Value, String> {
            calls += 1;
            Ok(json!({
                "status": "unknown",
                "recent_output": "% ",
                "status_source": "screen",
                "events": [{
                    "kind": "agent_dead",
                    "resume_command": "claude --resume sid-x",
                    "recommended_action": "resume_session",
                }],
            }))
        };
        let outcome = wait_for_worker(
            &mut exec,
            &WatchOptions {
                pane_id: 7803,
                session_id: None,
                tmux_session: None,
                timeout: Some(std::time::Duration::from_secs(30)),
                initial_delay: std::time::Duration::ZERO,
                interval: std::time::Duration::ZERO,
            },
            None,
        );
        assert_eq!(
            outcome,
            WatchOutcome::AgentDead {
                resume_command: Some("claude --resume sid-x".into())
            }
        );
        assert_eq!(calls, 2, "2 回連続観測で確定（単発の取りこぼし誤爆を防ぐ）");
    }

    #[test]
    fn issue390_workersリストがdispatchで返る() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.workers.insert(
                "q7901".into(),
                WorkerEntry {
                    pane: 7901,
                    agent: "claude".into(),
                    status: "active".into(),
                    spawned_at: crate::sessions::now_iso(),
                    ..Default::default()
                },
            );
            reg.workers.insert(
                "q7902".into(),
                WorkerEntry {
                    pane: 7902,
                    agent: "claude".into(),
                    status: "closed".into(),
                    spawned_at: crate::sessions::now_iso(),
                    ..Default::default()
                },
            );
        })
        .unwrap();
        let mut host = MockHost::new();
        // 既定: active のみ
        let v = dispatch(
            &mut host,
            Request::OrchestratorWorkers { all: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        let ids: Vec<&str> = v["workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w["worker_id"].as_str())
            .collect();
        assert!(ids.contains(&"q7901"));
        assert!(!ids.contains(&"q7902"), "closed は既定で出ない");
        // all = true で closed も
        let v = dispatch(
            &mut host,
            Request::OrchestratorWorkers { all: Some(true) },
            PaneOrigin::Cli,
        )
        .unwrap();
        let ids: Vec<&str> = v["workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w["worker_id"].as_str())
            .collect();
        assert!(ids.contains(&"q7901") && ids.contains(&"q7902"));
        // GUI にペインが無いので pane_alive = false（レジストリ追跡は継続）
        let w = v["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["worker_id"] == "q7901")
            .unwrap();
        assert_eq!(w["pane_alive"], false);
    }

    #[test]
    fn stale_binary_statusはバックエンドなしで対象外を返す() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let v = dispatch(
            &mut host,
            Request::StaleBinary {
                action: Some("status".into()),
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["stale"], false);
        assert!(v["reason"].as_str().unwrap().contains("対象外"));
    }

    #[test]
    fn stale_binary_restartはバックエンドなしでエラー() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::StaleBinary {
                action: Some("restart".into()),
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        );
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("バックエンド") || msg.contains("session_id"),
            "エラーメッセージが想定と異なる: {msg}"
        );
    }

    #[test]
    fn stale_binary_dismissは正常応答() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let v = dispatch(
            &mut host,
            Request::StaleBinary {
                action: Some("dismiss".into()),
                pane: Some(pane),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["dismissed"], true);
        assert_eq!(v["pane"], pane);
    }

    // --- #723: update の dry_run 分岐 ---
    //
    // 「試すだけのつもりが本当に置き換わった」が最悪の事故なので、
    // **本物の apply が呼ばれていないこと**を型ではなく実測で固定する

    /// どちらの apply が呼ばれたかだけを記録する host。
    /// ControlHost は多くの上位 trait をまとめたものなので、素の struct ではなく
    /// 実装済みの MockHost を包んで update 系だけ差し替える
    struct UpdateSpy {
        inner: MockHost,
        applied: Vec<Option<String>>,
        staged: Vec<Option<String>>,
    }

    impl UpdateSpy {
        fn new() -> Self {
            Self {
                inner: MockHost::new(),
                applied: Vec::new(),
                staged: Vec::new(),
            }
        }
    }

    impl WorkspaceHost for UpdateSpy {
        fn workspace(&self) -> &Workspace {
            self.inner.workspace()
        }
        fn workspace_mut(&mut self) -> &mut Workspace {
            self.inner.workspace_mut()
        }
    }
    impl SessionHost for UpdateSpy {
        fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
            self.inner.session(pane)
        }
        fn attach_session(&mut self, pane: PaneId, options: SpawnOptions) {
            self.inner.attach_session(pane, options)
        }
        fn detach_session(&mut self, pane: PaneId) {
            self.inner.detach_session(pane)
        }
    }
    impl TmuxHost for UpdateSpy {}
    impl UiStateHost for UpdateSpy {}
    impl PreviewHost for UpdateSpy {}
    impl WebViewHost for UpdateSpy {}
    impl RemoteHost for UpdateSpy {}
    impl SystemHost for UpdateSpy {
        fn update_apply(&mut self, channel: Option<&str>) -> Result<serde_json::Value, String> {
            self.applied.push(channel.map(|s| s.to_string()));
            Ok(serde_json::json!({ "updated": true }))
        }
        fn update_apply_dry_run(
            &mut self,
            channel: Option<&str>,
        ) -> Result<serde_json::Value, String> {
            self.staged.push(channel.map(|s| s.to_string()));
            Ok(serde_json::json!({ "dry_run": true, "applied": false }))
        }
    }

    fn update_req(action: &str, channel: Option<&str>, dry_run: Option<bool>) -> Request {
        Request::Update {
            action: Some(action.to_string()),
            channel: channel.map(|s| s.to_string()),
            dry_run,
        }
    }

    #[test]
    fn update_applyのdry_runは本物のapplyを呼ばない() {
        let mut host = UpdateSpy::new();
        let v = dispatch(
            &mut host,
            update_req("apply", Some("test"), Some(true)),
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["applied"], false);
        assert!(
            host.applied.is_empty(),
            "dry_run なのに本物の apply が走った"
        );
        assert_eq!(host.staged, vec![Some("test".to_string())]);
    }

    #[test]
    fn update_applyはdry_run無指定なら本物を呼ぶ() {
        for dry in [None, Some(false)] {
            let mut host = UpdateSpy::new();
            let v = dispatch(&mut host, update_req("apply", None, dry), PaneOrigin::Cli).unwrap();
            assert_eq!(v["updated"], true, "dry={dry:?}");
            assert_eq!(host.applied, vec![None], "dry={dry:?}");
            assert!(host.staged.is_empty(), "dry={dry:?}");
        }
    }

    #[test]
    fn update_dry_runはapply以外では拒否する() {
        // 黙って無視すると「実行しないつもりだったのに実行された」を招く
        for action in ["status", "check", "apply-zip", "repair"] {
            let mut host = UpdateSpy::new();
            let err = dispatch(
                &mut host,
                update_req(action, None, Some(true)),
                PaneOrigin::Cli,
            )
            .unwrap_err();
            assert!(
                matches!(err, DispatchError::InvalidParams(_)),
                "{action} で dry_run が素通りした: {err:?}"
            );
            assert!(host.applied.is_empty() && host.staged.is_empty());
        }
    }
}

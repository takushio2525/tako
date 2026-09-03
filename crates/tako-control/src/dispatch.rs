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
        /// 利用上限後の自動復帰が有効なペイン（#822。一覧に載せるだけで判定には使わない）
        limit_resume_panes: Vec<u64>,
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
    /// ファイルツリーの git ステータス（#1009）。
    /// ルートの解決は UI スレッドで済ませ、`git` の実行だけをここへ出す
    TreeGitStatus {
        tab: u64,
        roots: Vec<PathBuf>,
        limit: Option<usize>,
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
            limit_resume_panes: collect_limit_resume_panes(host),
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
        // #1009: `git status` は巨大なリポジトリだと数百 ms かかりうるので、
        // ルートの解決（workspace を読む = UI スレッド必須）だけをここで済ませて
        // 実行は background へ出す（#168 の GitLog / GitDiff と同じ扱い）
        Request::TreeFolder {
            action,
            path,
            tab,
            pane,
            limit,
        } if action == "git-status" => {
            let tab_id = match resolve_tab(host.workspace(), *tab, *pane) {
                Ok(id) => id,
                Err(e) => return Some(Err(e)),
            };
            let roots = match tree_git_status_roots(host, tab_id, path.clone()) {
                Ok(roots) => roots,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(OffloadJob::TreeGitStatus {
                tab: tab_id.as_u64(),
                roots,
                limit: *limit,
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
                limit_resume_panes,
                include_closed,
                sweep,
            } => finish_workers_list(&live_panes, &limit_resume_panes, include_closed, sweep),
            OffloadJob::GitLog { cwd, max_count } => run_git_log(&cwd, max_count),
            OffloadJob::GitDiff { cwd, target } => run_git_diff(&cwd, target.as_deref()),
            OffloadJob::GitShow { cwd, hash, file } => run_git_show(&cwd, &hash, file.as_deref()),
            OffloadJob::TreeGitStatus { tab, roots, limit } => {
                Ok(tree_git_status_payload(tab, &roots, limit))
            }
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

/// 利用上限後の自動復帰（FR-2.27 / #813）が有効なペイン ID（#822）。
/// `workers` 一覧に「その worker が自動復帰の対象か」を載せるための材料
fn collect_limit_resume_panes(host: &dyn ControlHost) -> Vec<u64> {
    host.workspace()
        .tabs()
        .iter()
        .flat_map(|t| t.tree().panes())
        .filter(|p| p.limit_autoresume())
        .map(|p| p.id().as_u64())
        .chain(
            host.workspace()
                .shelved_panes()
                .iter()
                .filter(|s| s.pane().limit_autoresume())
                .map(|s| s.id().as_u64()),
        )
        .collect()
}

/// OrchestratorWorkers のサブプロセス実行部分（tmux ls + レジストリ読み）。
/// UI スレッドで呼ばないこと（OffloadJob::run / dispatch 同期経路用）。
///
/// `sweep` = true なら列挙のついでに死んだ active エントリを GC する（#658）。
/// ペインも器も見えない状態が `DEAD_CONFIRM_SECS` 続いたものだけが closed になる
/// （倒すのに 2 回以上の観測が要るので、この 1 回の列挙で生き物を落とすことはない）
fn finish_workers_list(
    live_panes: &[(u64, Option<String>)],
    limit_resume_panes: &[u64],
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
        limit_resume_panes,
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

        Request::Close {
            pane,
            force,
            caller_role,
        } => {
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
            // #566: 発生源（CLI / MCP + 呼び出し元 role）をペインログへ残す。
            // dispatch 経由の close は確認を挟まない（AI フルコントロール維持）ぶん、
            // 「誰が閉じたか」を事後に追える形にしておく
            host.detach_session(target, close_origin_of(origin), caller_role.as_deref());
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

        Request::ResolvePane { pane, caller_pid } => {
            Ok(resolve_pane_lenient_json(host, pane, caller_pid))
        }

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

            // #748: 選択肢ダイアログ表示中の送信を**入口で断る**。
            // ダイアログは入力欄を奪っているので、テキストを書けば 1 文字ずつが
            // ダイアログのキー操作として食われ（数字なら選択が確定してしまう）、
            // Enter は「今ハイライトされている選択肢の確定」になる。
            // 実際に観測されたのは limit ダイアログの選択肢テキストが入力欄に
            // 残ったまま混線する状態（#748 の観測 1 / 4）。
            // 正しい操作は respond（番号 / ラベル指定 + 実在再検証）なので、
            // 選択肢一覧つきの明示エラーでそちらへ誘導する
            if let Some(err) = dialog_blocks_send(host, pane, tmux_session.as_deref(), &text) {
                return Err(err);
            }

            // pane ID で解決を試み、失敗時に tmux session フォールバック
            match resolve_pane(host.workspace(), pane) {
                Ok((_, target)) => {
                    let session = host
                        .session(target)
                        .ok_or(DispatchError::NoSession(target.as_u64()))?;
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
                    // #907: 器の client が ASCII しか運べない（psmux）なら、本文は
                    // 打鍵ではなく**器の注入口**へ入れる。実機実測で打鍵経路は
                    // cp932 に無い文字（`─` / `❯` 等）を黙って落とす。Enter は ASCII なので
                    // 従来どおり「貼り付けと分離した単独キー」として打鍵で送る（#95 / #32）
                    let injected = crate::delivery::inject_non_ascii(
                        host.backend_session(target).as_deref(),
                        &normalized,
                    );
                    let payload = match (&injected, newline) {
                        // 注入に成功したら本文は打鍵しない（Enter だけ送る）
                        (Ok(true), true) => "\r".to_string(),
                        (Ok(true), false) => String::new(),
                        (_, true) => format!("{normalized}\r"),
                        (_, false) => normalized.clone(),
                    };
                    if !payload.is_empty() {
                        session.write(payload.into_bytes());
                    }
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

            // #572: claude のメッセージキューに未送信の指示が残っているか
            // （画面を切り詰める前の全行で判定する）
            let queued_pending = crate::claude_tui::queued_messages_pending(&all);

            // #748: 選択肢ダイアログが実在するか（画面を切り詰める前の全行で判定）。
            // ダイアログの選択カーソル（`❯ 1. Stop and wait for limit to reset`）は
            // 入力欄と同じ字面なので、旧実装は `input_status.style=user` として
            // 「入力欄にテキストが残っている」と報告していた（#748 の観測 1）。
            // ダイアログ中は**入力欄が存在しない**ので input_status は null にし、
            // 代わりに構造化した選択肢を返す
            let dialog = crate::claude_tui::detect_choice_dialog(&all);

            while all.last().is_some_and(|l| l.is_empty()) {
                all.pop();
            }
            if let Some(n) = lines {
                if all.len() > n {
                    all.drain(..all.len() - n);
                }
            }
            let input_json = input_status.filter(|_| dialog.is_none()).map(|s| {
                json!({
                    "line": s.line,
                    "text": s.text,
                    "style": match s.style {
                        tako_core::InputStyle::Ghost => "ghost",
                        tako_core::InputStyle::User => "user",
                        tako_core::InputStyle::Mixed => "mixed",
                        tako_core::InputStyle::None => "none",
                    },
                    // #572: true = busy 中に打たれた指示が claude のキューにあり未送信。
                    // 入力欄自体は空なので Enter 単独送達では発火しない
                    "queued_messages_pending": queued_pending,
                })
            });
            Ok(json!({
                "pane": pane_id,
                "text": all.join("\n"),
                "input_status": input_json,
                "queued_messages_pending": queued_pending,
                // #748: 選択肢ダイアログ（null = ダイアログなし）。
                // 非 null のときは入力欄が無いので send ではなく respond で応答する
                "choice_dialog": dialog.as_ref().map(|d| d.to_json()),
                // #813: 利用上限後の自動復帰。enabled = ペインのオプトイン、
                // state = いま上限で止まっているか・いつ復帰するか（GUI 稼働時のみ）
                "limit_resume": limit_resume_entry(host, PaneId::from_raw(pane_id)),
                // #1010: SSH の接続待ち / 失敗（null = どちらでもない）。
                // 画面がまだ空でも「何を待っているか」が応答から分かる
                "ssh_connect": host.ssh_connect_state(PaneId::from_raw(pane_id)),
            }))
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
                    tako_core::tmux::exact_target(&original),
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
                    tako_core::tmux::exact_target(&original),
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

        // #552 案 4: GUI の「この名前を固定」（自動命名直後に出るピン印）と 1:1
        Request::TabPinTitle { pane, tab, pinned } => {
            let tab_id = match tab {
                Some(raw) => find_tab(host.workspace(), raw)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            let tab = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .expect("find_tab / resolve_pane で存在確認済み");
            match pinned {
                Some(true) => {
                    tab.pin_title();
                }
                Some(false) => tab.clear_manual_title(),
                None => {}
            }
            Ok(json!({
                "tab": tab_id.as_u64(),
                "title": tab.title(),
                "source": tab.title_source().as_str(),
                "pinned": tab.title_source() == tako_core::TitleSource::Manual,
            }))
        }

        Request::TabNew { title, focus, cwd } => {
            // cwd はシェルを起動する前に検査する（存在しない場所で起動すると
            // シェルの既定へ黙って落ちるので、頼まれた場所と違う場所が開く）
            let cwd = match cwd {
                Some(raw) => {
                    let path = std::path::PathBuf::from(&raw);
                    let path = path.canonicalize().map_err(|e| {
                        DispatchError::Operation(format!("フォルダを開けない（{raw}: {e}）"))
                    })?;
                    if !path.is_dir() {
                        return Err(DispatchError::Operation(format!(
                            "フォルダではない: {}",
                            path.display()
                        )));
                    }
                    Some(path)
                }
                None => None,
            };
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
            let cwd_json = cwd.as_ref().map(|p| p.display().to_string());
            host.attach_session(
                pane_id,
                SpawnOptions {
                    cwd,
                    ..SpawnOptions::default()
                },
            );
            Ok(json!({
                "tab": tab_id.as_u64(),
                "pane": pane_id.as_u64(),
                "cwd": cwd_json,
            }))
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

        Request::Autosuggest { enabled, hint, tab } => {
            if let Some(enabled) = enabled {
                host.set_autosuggest(enabled);
            }
            if let Some(hint) = hint {
                host.set_autosuggest_hint(hint);
            }
            if let Some(tab) = tab {
                host.set_autosuggest_tab(tab);
            }
            let enabled = host.autosuggest_enabled();
            let hint = host.autosuggest_hint_enabled();
            let tab = host.autosuggest_tab_enabled();
            // 残り回数は zsh 側がコマンドラインごとに 1 減らす。恒久 OFF と読めない環境は null
            let hint_remaining = tako_core::shell_integration::integration_root()
                .and_then(|r| tako_core::shell_integration::autosuggest_hint_state_in(&r));
            Ok(json!({
                "enabled": enabled,
                // #614: 確定キーの案内と、確定に使えるキー
                "hint": hint,
                "hint_remaining": hint_remaining,
                "tab_accept": tab,
                "accept_keys": if tab { vec!["Right", "Tab"] } else { vec!["Right"] },
                // 何が同梱されているか / どのシェルに効くかを AI にも見せる
                "shell": "zsh",
                "provider": "zsh-autosuggestions",
                "version": tako_core::shell_integration::AUTOSUGGEST_VERSION,
                "applies_to": "既存ペインを含む tako 内の zsh（次のプロンプトから反映）",
                "note": "ユーザーが自前で zsh-autosuggestions を導入しているペインでは \
                    tako は注入せず、この設定も効かない（二重注入ガード）。\
                    Tab 確定が働くのはゴースト表示中かつカーソルが行末のときだけで、\
                    それ以外の Tab は従来の補完のまま（tab=false にすると常に補完）",
            }))
        }

        Request::ConfirmClose { enabled } => {
            if let Some(val) = enabled {
                host.set_confirm_close(val);
                let _ = crate::setup::mutate_config(|c| c.confirm_close = val);
            }
            Ok(json!({ "enabled": host.confirm_close_enabled() }))
        }

        Request::LimitResume { pane, enabled, all } => {
            // 一覧（#813。有効なペインがどれかを 1 回で把握する）
            if all.unwrap_or(false) {
                if enabled.is_some() {
                    return Err(DispatchError::InvalidParams(
                        "all と enabled は併用できない（一覧か設定のどちらか）".into(),
                    ));
                }
                return Ok(json!({ "panes": limit_resume_panes(host) }));
            }
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            if let Some(val) = enabled {
                tree_mut(host.workspace_mut(), tab)
                    .get_mut(target)
                    .expect("resolve_pane で存在確認済み")
                    .set_limit_autoresume(val);
                // 有効・無効はペイン属性なので layout.json の保存で永続化される
                // （保存は UI 層が dispatch 後に回す。CLI / MCP / 右クリックで同じ経路）
                crate::diag::persist_log(&format!(
                    "[limit-autoresume] pane={} enabled={val}",
                    target.as_u64()
                ));
            }
            Ok(limit_resume_entry(host, target))
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
            show_hidden,
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
                // 上限・下限のクランプは host 側（`tako_core::sidebar` の 1 実装 =
                // GUI のドラッグ経路と同じ規則。#789）
                host.set_sidebar_width(sw);
                // #789: 永続化するのは要求値ではなく**実際に適用された幅**
                // （旧実装は要求値を書いていたので、クランプ後の画面の幅と
                // settings.json の値が食い違っていた）
                let mut settings = crate::settings::load();
                settings.sidebar_width = host.sidebar_width() as u32;
                let _ = crate::settings::save(&settings);
            }
            if let Some(sh) = show_hidden {
                host.set_filetree_show_hidden(sh);
                let mut settings = crate::settings::load();
                settings.show_hidden_files = sh;
                let _ = crate::settings::save(&settings);
            }
            let (visible, width, view) = host.panel_state();
            Ok(json!({
                "visible": visible,
                "width": width,
                "view": view.as_str(),
                "filetree": host.filetree_visible(),
                // #789: 画面に出ている実効幅と、その時点の上限（ウィンドウ幅の 50%。
                // GUI のドラッグと同じ規則。ウィンドウ未描画なら null）
                "sidebar_width": host.sidebar_width(),
                "sidebar_width_max": host.sidebar_width_max(),
                "sidebar_width_min": tako_core::sidebar::MIN_WIDTH,
                "show_hidden": host.filetree_show_hidden(),
            }))
        }

        Request::OpenFile {
            pane,
            path,
            mode,
            direction,
            focus,
            new_tab,
        } => {
            if new_tab && direction.is_some() {
                return Err(DispatchError::Operation(
                    "new_tab と direction は同時に指定できない（新しいタブには分割元が無い）"
                        .into(),
                ));
            }
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
            // 表示先の解決: new_tab 指定（FR-3.22 = Finder の「このアプリケーションで
            // 開く」）なら新しいタブ 1 枚をそのファイル専用にする。direction 指定
            // （FR-3.11 = D&D のドロップ位置）なら再利用せず必ずその方向へ分割。
            // どちらも省略時は 対象自身がプレビュー > 同タブの既存プレビュー（再利用）
            // > 右分割で新設。いずれの経路でもターミナルセッションは起動しない
            let (tab, view_pane, created) = if new_tab {
                let prev_active = host.workspace().active_tab_id();
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                let title = resolved
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| resolved.display().to_string());
                let tab_id = host.workspace_mut().create_tab(title, new_pane);
                // ファイル名は「このタブが何か」そのものなので、自動リネーム（FR-2.12）に
                // 奪わせない。プレビュー専用タブには命名材料になる端末出力も無い
                if let Some(t) = host.workspace_mut().get_tab_mut(tab_id) {
                    let title = t.title().to_string();
                    t.set_title_manual(title);
                }
                // CLI/MCP 経由のデフォルトはアクティブタブを維持（ユーザーの入力を奪わない）
                if !focus.unwrap_or(false) {
                    let _ = host.workspace_mut().activate_tab(prev_active);
                }
                (tab_id, new_id, true)
            } else if let Some(direction) = direction {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, direction.to_core(), 0.5, new_pane)
                    .map_err(op_err)?;
                (tab, new_id, true)
            } else if host.preview_state(target).is_some() {
                (tab, target, false)
            } else if let Some(existing) = host.preview_pane_of_tab(tab) {
                (tab, existing, false)
            } else {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, SplitDirection::Right, 0.5, new_pane)
                    .map_err(op_err)?;
                (tab, new_id, true)
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
                "tab": tab.as_u64(),
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
        Request::ChatCopy {
            pane,
            list,
            message,
            code,
            markdown,
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let result = host
                .chat_copy(target, list, message, code, markdown)
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
            let mut out = json!({
                "pane": target.as_u64(),
                "editing": editing,
                "dirty": dirty,
                "saved": true,
            });
            // #966: リモート由来なら「リモートへ書けたのか」まで応答に載せる
            // （ローカルの写しへ書けただけで saved=true とは言わせない）
            if let Some(remote) = host.preview_remote_state(target) {
                out["remote"] = remote;
            }
            Ok(out)
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
            host.detach_session(pane_id, close_origin_of(origin), None);
            Ok(json!({ "killed": pane }))
        }

        Request::CheckHealth => Ok(check_health(host)),

        Request::SetupMcp { scope, pane, agent } => {
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
            setup_mcp_agents(agent.as_deref(), &mcp_scope, scope_str)
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
            kind,
            from,
            projects,
            clear_projects,
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
            ctx_threshold,
            clear_ctx_threshold,
            auto_handoff,
            clear_auto_handoff,
            limit_resume,
            clear_limit_resume,
            bypass_sandbox,
            remote_control,
        } => dispatch_orchestrator_profiles(ProfilesParams {
            action,
            name,
            kind,
            from,
            projects,
            clear_projects,
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
            ctx_threshold,
            clear_ctx_threshold,
            auto_handoff,
            clear_auto_handoff,
            limit_resume,
            clear_limit_resume,
            bypass_sandbox,
            remote_control,
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
        } => dispatch_orchestrator_accounts(
            &action,
            name.as_deref(),
            config_dir.as_deref(),
            inherit,
            description.as_deref(),
            default_model.as_deref(),
            default_effort.as_deref(),
        ),

        // #666: AI コマンド提案カード
        Request::ShowCommand {
            action,
            commands,
            label,
            pane,
            card,
            index,
            focus,
        } => dispatch_show_command(
            host,
            origin,
            action.as_deref().unwrap_or("show"),
            &commands,
            label.as_deref(),
            pane,
            card,
            index,
            focus,
        ),

        // #513: AI 系設定の git ベース共有
        Request::ConfigShare {
            action,
            target,
            path,
            remote,
            message,
            no_push,
        } => dispatch_config_share(
            action.as_deref().unwrap_or("status"),
            target.as_deref(),
            path.as_deref(),
            remote.as_deref(),
            message.as_deref(),
            no_push,
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
            projects,
        } => dispatch_orchestrator_handoff(
            host,
            origin,
            pane,
            caller_role.as_deref(),
            tab,
            caller_pid,
            projects,
        ),

        Request::OrchestratorHandoffFiles {
            action,
            project,
            profile,
            content,
        } => dispatch_orchestrator_handoff_files(
            &action,
            project.as_deref(),
            profile.as_deref(),
            content.as_deref(),
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
            limit_resume,
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
                limit_resume,
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
            let limit_resume_panes = collect_limit_resume_panes(host);
            let sweep = !host.is_secondary();
            finish_workers_list(
                &live_panes,
                &limit_resume_panes,
                all.unwrap_or(false),
                sweep,
            )
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

        // #319: permission ダイアログへの構造化応答
        Request::OrchestratorRespond {
            pane_id,
            choice,
            caller_role,
        } => {
            dispatch_orchestrator_respond(host, pane_id, choice.as_deref(), caller_role.as_deref())
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
            let mut result =
                crate::agents::list_agents_with_panes(None).map_err(DispatchError::Operation)?;
            // #1069: 公式リンクは HTTP の /api/agents と同じ 1 実装で付ける
            // （AI が見る値とスマホの一覧が食い違わない = 開発不変条件）
            crate::claude_remote_link::attach_to_agents(&mut result);
            Ok(result)
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
                    host.detach_session(target, close_origin_of(origin), None);
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

        Request::Update { action, channel } => {
            let action = action.as_deref().unwrap_or("status");
            let ch = channel.as_deref();
            match action {
                "status" => {
                    let mut json = host.update_status();
                    // #616: 専用画面が開いているか（設定画面の status と同じ役割）
                    json["window_open"] = json!(host.update_window_open());
                    Ok(json)
                }
                "check" => Ok(host.update_check(ch)),
                "apply" => host.update_apply(ch).map_err(DispatchError::Operation),
                "apply-zip" => host.update_apply_zip(ch).map_err(DispatchError::Operation),
                "repair" => host.update_repair().map_err(DispatchError::Operation),
                // #616: アップデート専用画面 + 上部通知カード
                "open" => {
                    host.open_update_window();
                    Ok(json!({ "ok": true, "requested": true }))
                }
                "card" => Ok(host.update_card_status()),
                "card-dismiss" | "card-show" => {
                    let dismissed = action == "card-dismiss";
                    // 閉じる対象のキーは「いま案内している内容」。set より先に読む
                    let key = dismissed
                        .then(|| {
                            host.update_card_status()["key"]
                                .as_str()
                                .map(str::to_string)
                        })
                        .flatten();
                    host.set_update_card_dismissed(dismissed);
                    // ユーザー設定を汚さない（Theme / Welcome と同方針）
                    if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                        let mut settings = crate::settings::load();
                        settings.update_card_dismissed = key;
                        if let Err(e) = crate::settings::save(&settings) {
                            return Err(DispatchError::Operation(format!(
                                "更新通知カードの状態を保存できない: {e}"
                            )));
                        }
                    }
                    Ok(host.update_card_status())
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / check / apply / apply-zip / repair / \
                     open / card / card-dismiss / card-show のいずれか）"
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

        Request::SetupBootstrap {
            action,
            dry_run,
            reason,
        } => {
            // 読み取り・書き込みともプロセス内で完結する（アプリ状態に依存しない）
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => crate::setup_bootstrap::status()
                    .map(|s| s.to_json())
                    .map_err(DispatchError::Operation),
                "install" => {
                    crate::setup_bootstrap::install(crate::setup_bootstrap::InstallOptions {
                        dry_run: dry_run.unwrap_or(false),
                        // GUI 内 dispatch には端末が無いので出力は捕捉して応答へ載せる
                        interactive: false,
                    })
                    .map_err(DispatchError::Operation)
                }
                "path" => crate::setup_bootstrap::ensure_path().map_err(DispatchError::Operation),
                "undo-path" => {
                    crate::setup_bootstrap::undo_path().map_err(DispatchError::Operation)
                }
                // 自動導入が通らないときの引き継ぎ計画（#1057。**読み取り専用**）。
                // 実際に相手を起こすのは端末を持つ側（CLI）か、AI なら
                // `tako_orchestrator_spawn` / `tako_run` の仕事
                "handoff" => crate::setup_bootstrap::handoff_plan(reason.as_deref())
                    .map_err(DispatchError::Operation),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / install / path / undo-path / handoff のいずれか）"
                ))),
            }
        }

        Request::SetupDeps {
            action,
            dep,
            dry_run,
        } => {
            // 依存の検出と brew / winget での導入。プロセス内で完結する（#88 / #1057）
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => Ok(serde_json::json!({
                    "deps": crate::setup_deps::status_json(),
                    "install_command": "tako setup deps install",
                })),
                "install" => crate::setup_deps::install(
                    dep.as_deref(),
                    crate::setup_deps::DepInstallOptions {
                        dry_run: dry_run.unwrap_or(false),
                        // GUI 内 dispatch には端末が無いので出力は捕捉して応答へ載せる
                        interactive: false,
                    },
                )
                .map_err(DispatchError::Operation),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / install のいずれか）"
                ))),
            }
        }

        Request::SetupModels { agent } => {
            // 読み取り専用・プロセス内完結（アプリ状態に依存しない）。
            // 実取得は各 CLI の一覧コマンドで、選んだ値の反映は
            // `OrchestratorProfiles`（--model / --effort）が担当する（#1002）
            use crate::agent_models;
            let catalogs = match agent.as_deref() {
                None | Some("") | Some("all") => agent_models::catalog_all(),
                Some(name) => {
                    let kind = crate::orchestrator::agent::WorkerAgent::parse(name)
                        .map_err(DispatchError::InvalidParams)?;
                    vec![agent_models::catalog(kind)]
                }
            };
            Ok(serde_json::json!({
                "agents": catalogs
                    .iter()
                    .map(agent_models::ModelCatalog::to_json)
                    .collect::<Vec<_>>(),
                "apply_command": "tako orchestrator profiles set <プロファイル> --model <id> --effort <値>",
            }))
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
                // 手段（macOS = sudoers 登録 / Windows = 権限不要）は `sleep_guard` 側に
                // 閉じているので、ここは OS を意識しない単一経路にする（#697）。
                // CLI（`tako sleep-guard install-lid-sleep`）と同じ関数を通るので
                // 3 経路で挙動が食い違わない
                "install-lid-sleep" => {
                    let result = crate::sleep_guard::prepare_lid_control()
                        .map_err(DispatchError::Operation)?;
                    let mut settings = crate::settings::load();
                    settings.lid_sleep_mode =
                        crate::sleep_guard::LidSleepMode::WhileAgentsRunning;
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                    Ok(serde_json::json!({
                        "result": result,
                        "lid_sleep_mode": "while-agents-running",
                        // 手段が sudoers ではない OS もあるので、実態を読んで返す
                        // （固定の true を返すと Windows で嘘になる）
                        "sudoers_installed": crate::sleep_guard::is_sudoers_installed(),
                        "lid_setup_required": crate::sleep_guard::lid_setup_pending(),
                    }))
                }
                "remove-lid-sleep" => {
                    let result = crate::sleep_guard::teardown_lid_control()
                        .map_err(DispatchError::Operation)?;
                    let mut settings = crate::settings::load();
                    settings.lid_sleep_mode = crate::sleep_guard::LidSleepMode::Off;
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                    Ok(serde_json::json!({
                        "result": result,
                        "lid_sleep_mode": "off",
                        "sudoers_installed": crate::sleep_guard::is_sudoers_installed(),
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

        // agent 能力マトリクスの参照（#982）。#515 と同じく静的な表を引くだけ。
        // CLI・MCP・docs 生成が crate::agent_support::report の 1 本を通る
        Request::AgentSupport { agent, status } => {
            crate::agent_support::report(agent.as_deref(), status.as_deref())
                .map_err(DispatchError::InvalidParams)
        }

        // 対応マトリクスの参照（#515）。静的な表を引くだけなのでホスト状態に触れない。
        // CLI・MCP とも crate::platform::report を通るので表示が食い違わない
        Request::Platform {
            platform,
            status,
            known_limitations,
        } => crate::platform::report(platform.as_deref(), status.as_deref(), known_limitations)
            .map_err(DispatchError::InvalidParams),

        Request::ShellIntegration { action } => {
            crate::shell_integration::run(action.as_deref()).map_err(DispatchError::InvalidParams)
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

        // UI 表示モード（#691 / #694）。テーマと同型の「状態確認 / 設定 / 反転」に、
        // スターターの「コマンド入力へ」に相当するペイン単位の揮発解除を足したもの。
        // GUI のトグル・カードも同じここを通るので UI と AI の操作が構造的に一致する
        Request::UiMode { action, mode, pane } => {
            use tako_core::ui_mode::UiMode;
            let action = action.as_deref().unwrap_or("status");
            let status_json = |host: &dyn ControlHost| {
                let mut released: Vec<u64> = host
                    .starter_released_panes()
                    .iter()
                    .map(|p| p.as_u64())
                    .collect();
                released.sort_unstable();
                // #720: いま各ペインが何として描かれているか（terminal / starter / chat /
                // preparing）。揮発なので永続化しない。「チャットがまだ出ない」理由
                // （= 過渡期の preparing なのか、判定がターミナルに倒れたのか）が分かる
                let mut displays: Vec<(u64, tako_core::ui_mode::PaneDisplayStatus)> = host
                    .pane_displays()
                    .into_iter()
                    .map(|(pane, status)| (pane.as_u64(), status))
                    .collect();
                displays.sort_by_key(|(pane, _)| *pane);
                let pane_display: serde_json::Map<String, Value> = displays
                    .iter()
                    .map(|(pane, status)| {
                        (pane.to_string(), serde_json::json!(status.display.as_str()))
                    })
                    .collect();
                // #1058: 「なぜスターター / チャットにならないのか」を材料つきで出す。
                // 黙ってターミナル表示へ落ちるのを止めるための診断で、判定は変えていない
                let pane_display_reason: serde_json::Map<String, Value> = displays
                    .iter()
                    .map(|(pane, status)| (pane.to_string(), pane_display_reason_json(status)))
                    .collect();
                serde_json::json!({
                    "ui_mode": host.ui_mode().as_str(),
                    "available": UiMode::VALUES,
                    "released_panes": released,
                    "pane_display": pane_display,
                    "pane_display_reason": pane_display_reason,
                })
            };
            let apply = |host: &mut dyn ControlHost,
                         next: UiMode|
             -> Result<Value, DispatchError> {
                // 永続化（テスト・セルフテスト中はユーザー設定を汚さない。Theme と同方針）
                if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                    let mut settings = crate::settings::load();
                    settings.ui_mode = next.as_str().into();
                    crate::settings::save(&settings)
                        .map_err(|e| DispatchError::Operation(format!("設定の保存に失敗: {e}")))?;
                }
                host.set_ui_mode(next);
                Ok(status_json(host))
            };
            match action {
                "status" => Ok(status_json(host)),
                "set" => {
                    let raw = mode.as_deref().ok_or_else(|| {
                        DispatchError::InvalidParams("set には mode が必要（terminal / gui）".into())
                    })?;
                    let next = UiMode::parse(raw).ok_or_else(|| {
                        DispatchError::InvalidParams(format!(
                            "不明な mode: {raw:?}（terminal / gui のいずれか）"
                        ))
                    })?;
                    apply(host, next)
                }
                "toggle" => {
                    let next = host.ui_mode().toggled();
                    apply(host, next)
                }
                // ペイン単位の揮発解除（スターターの「コマンド入力へ」と同経路）。
                // 永続化しないので再起動すると GUI 表示に戻る（仕様 §1.3）
                "release" | "restore" => {
                    let (_, target) = resolve_pane(host.workspace(), pane)?;
                    host.set_starter_released(target, action == "release");
                    let mut json = status_json(host);
                    json["pane"] = serde_json::json!(target.as_u64());
                    json["released"] = serde_json::json!(action == "release");
                    Ok(json)
                }
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / set / toggle / release / restore のいずれか）"
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
            limit,
        } => dispatch_tree_folder(host, &action, path, tab, pane, limit),

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
            // #1069: Claude 公式 Remote Control の session URL
            "link" => dispatch_sessions_link(host, origin, id.as_deref(), pane),
            other => Err(DispatchError::InvalidParams(format!(
                "不明な action: {other:?}（list / show / resume / link のいずれか）"
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
            remote_dir,
            target,
            pane,
            tab,
            direction,
        } => {
            // #919: **ssh を素のペインのプログラムにしない**。
            //
            // 旧実装は `SpawnCommand { program: "ssh", args: [host] }` だったので
            // ① 接続待ちのあいだ画面が完全に空（実測: TCP ブラックホールで 25 秒間
            // 1 文字も出ない = 「何も入力できない」に見える）② ssh が即死すると
            // PTY の死とともにペインが消え、タブごと閉じて理由が残らない
            // （実測: 名前解決できないホストは 1 秒でタブが消滅）。
            //
            // `ssh_pane_script` で包むと、接続前にバナーが出て、ssh 自身の失敗
            // （exit 255）だけ理由 + 次の一手を出して入力待ちで止まる
            //
            // #1006: 開き先は 3 通り（既定 = 現在タブへ新ペイン）。語彙の正本は
            // `tako_core::remote_open`（CLI の値一覧・MCP の enum・GUI の分岐が同じ表を引く）
            let open_target = target.unwrap_or_default();
            // #1040: `<data_dir>/ssh/` が無いと ssh が
            // `unix_listener: cannot bind to path …: No such file or directory` で
            // **必ず exit 255**（まっさらな data_dir での初回接続 = 実測）。
            // 作っていたのは `ensure_master`（ツリー側）だけだったので、
            // ペインを起こす 3 経路の手前にも同じ 1 実装を通す
            if let Err(e) = tako_core::remote_fs::ensure_control_dir(&ssh_host) {
                crate::diag::persist_log(&format!("ssh の ControlPath 置き場を作れない: {e}"));
            }
            let argv = remote_ssh_argv(&ssh_host);
            let dir = remote_dir
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string);

            // Recent への記録は 3 経路で共通（どの開き方でも「最近つないだ相手」）
            let record_recent = |host_name: &str| {
                let mut recent = tako_core::recent::RecentList::load();
                recent.push(tako_core::recent::RecentEntry::Ssh {
                    host: host_name.to_string(),
                });
                recent.save();
            };

            // 既存ペインをそのまま SSH にする（#1006）。
            // **ペインを作らない・閉じない**ので pane ID が変わらず、ssh が失敗しても
            // 下のシェルがそのまま残る（= シェルのプロンプトへ戻る）
            if open_target == tako_core::remote_open::RemoteOpenTarget::Pane {
                let (tab_id, pane_id) = resolve_pane_or_active(host.workspace(), pane)?;
                let role = host
                    .workspace()
                    .get_tab(tab_id)
                    .and_then(|t| t.tree().get(pane_id))
                    .and_then(|p| p.role())
                    .map(str::to_string);
                // 器（tmux）つきのペインでは**外側**の alt screen を信じてはいけない
                // （tmux クライアント自身が alt screen に入る = 中身が素のシェルでも
                // 常に true。#694 と同じ罠で、隔離セルフテストで実測した）
                let backend = host.backend_session(pane_id).is_some();
                let (has_session, alt, state) = match host.session(pane_id) {
                    Some(s) => (true, !backend && s.is_alt_screen(), s.command_state()),
                    None => (false, false, tako_core::terminal::CommandState::Unknown),
                };
                tako_core::remote_open::can_ssh_pane(has_session, alt, state, role.as_deref())
                    .map_err(|b| DispatchError::Operation(b.message(pane_id.as_u64())))?;

                // 素のシェルへ打つので**スクリプトでは包まない**（包むと入れ子の
                // シェルが増え、失敗時に「戻る先」がプロンプトでなくなる）。
                // ssh 自身の失敗はシェルがそのまま表示し、プロンプトが戻る
                // 引用は「必要なときだけ」（#322 = 人が打つ形と同じにする。
                // 方言の解決は `launch_cmd` の 1 本 = #873）
                let dialect = crate::launch_cmd::launch_dialect();
                let line = argv
                    .iter()
                    .map(|a| crate::launch_cmd::quote(dialect, a))
                    .collect::<Vec<_>>()
                    .join(" ");
                // 器（psmux）は起動直後だけでなく高負荷時も入力を落とすので、
                // 送達確認つきの経路（#640）へ載せる
                // #1010: 打ち始める前から「接続中…」を出す。#640 の送達フローは
                // シェルの準備を待つので、ここを起点にしないと**打たれるまでの沈黙**が
                // そのまま「何も起きない」に見える
                host.begin_ssh_connect(pane_id, &ssh_host, false, &line);
                host.queue_command_flow(pane_id, line.clone());
                if let Some(d) = &dir {
                    // 接続後にリモートで `cd`。相手のシェルが不明なので両方言で通る形
                    // （同一ペインの 2 本目は先行フローの完了まで待つ = UI 側で直列化）
                    let native = tako_core::remote_fs::shell_path(d);
                    host.queue_command_flow(pane_id, format!("cd \"{native}\""));
                }
                if focus.unwrap_or(true) {
                    let _ = host.workspace_mut().activate_tab(tab_id);
                    let _ = tree_mut(host.workspace_mut(), tab_id).focus(pane_id);
                }
                record_recent(&ssh_host);
                return Ok(json!({
                    "tab": tab_id.as_u64(),
                    "pane": pane_id.as_u64(),
                    "host": ssh_host,
                    "remote_dir": dir,
                    "target": open_target.as_str(),
                    "command": line,
                }));
            }

            // #1040: 切れたときに打ち直す 1 行（`pane` 経路と同じ形・同じ引用規則）
            let reconnect_line = {
                let dialect = crate::launch_cmd::launch_dialect();
                argv.iter()
                    .map(|a| crate::launch_cmd::quote(dialect, a))
                    .collect::<Vec<_>>()
                    .join(" ")
            };

            let script = tako_core::remote_fs::ssh_pane_script(
                tako_core::platform::shell::script_dialect(),
                &argv,
                &ssh_host,
                dir.as_deref(),
                tako_core::i18n::lang(),
            );
            let command = tako_core::platform::shell::script_pane_command(&script);

            let prev_active = host.workspace().active_tab_id();
            let new_pane = Pane::new(origin);
            let pane_id = new_pane.id();

            let tab_id = if open_target == tako_core::remote_open::RemoteOpenTarget::Tab {
                // #20 の従来動作: 新しいタブを立てて `ssh:<host>` と名付ける
                let tab_title = format!("ssh:{ssh_host}");
                let tab_id = host.workspace_mut().create_tab(tab_title, new_pane);
                if let Some(tab) = host.workspace_mut().get_tab_mut(tab_id) {
                    let t = tab.title().to_string();
                    tab.set_title_manual(t);
                }
                if !focus.unwrap_or(true) {
                    let _ = host.workspace_mut().activate_tab(prev_active);
                }
                tab_id
            } else {
                // #1006 の既定: いま開いているタブへ新ペインを作る。
                // 分割元は `pane` → 呼び出し元 → （`tab` 指定なら）そのタブの
                // フォーカス中ペイン、の順で解決する（`Request::Split` と同じ規則）
                let (tab_id, base) = if let Some(tab_raw) = tab {
                    let tab_id = find_tab(host.workspace(), tab_raw)?;
                    let focused = host
                        .workspace()
                        .get_tab(tab_id)
                        .expect("find_tab で存在確認済み")
                        .tree()
                        .focused();
                    (tab_id, focused)
                } else {
                    resolve_pane_or_active(host.workspace(), pane)?
                };
                let focused_before = host.workspace().get_tab(tab_id).map(|t| t.tree().focused());
                tree_mut(host.workspace_mut(), tab_id)
                    .split_with_ratio(
                        base,
                        direction.unwrap_or(Direction::Right).to_core(),
                        0.5,
                        new_pane,
                    )
                    .map_err(op_err)?;
                if focus.unwrap_or(true) {
                    let _ = host.workspace_mut().activate_tab(tab_id);
                    let _ = tree_mut(host.workspace_mut(), tab_id).focus(pane_id);
                } else if let Some(prev) = focused_before.filter(|p| *p != pane_id) {
                    // 分割の副作用で移ったフォーカスを元へ戻す（#676 と同じ配慮）
                    let _ = tree_mut(host.workspace_mut(), tab_id).focus(prev);
                }
                tab_id
            };

            host.attach_session(
                pane_id,
                SpawnOptions {
                    command: Some(command),
                    ..Default::default()
                },
            );
            // #1010: 新しいペインは tako が印字したバナーしか載っていない
            // （= `fresh_pane`）ので、判定は「tako 以外の行が出たか」で足りる。
            // #1040: 打ち直す行は**スクリプトではなく素の ssh 1 行**。切断後のペインは
            // ローカルシェルのプロンプトに居るので、`pane` 経路とまったく同じ形で戻せる
            host.begin_ssh_connect(pane_id, &ssh_host, true, &reconnect_line);

            // フォルダ指定つきは接続後に `cd` を打つ。相手のシェルが不明（POSIX とも
            // PowerShell とも限らない）なので、**両方で通る `cd "<path>"`** を
            // シェル準備待ち + エコー確認つきの経路（#640）で送る
            if let Some(dir) = &dir {
                let native = tako_core::remote_fs::shell_path(dir);
                host.queue_command_flow(pane_id, format!("cd \"{native}\""));
            }

            record_recent(&ssh_host);

            Ok(json!({
                "tab": tab_id.as_u64(),
                "pane": pane_id.as_u64(),
                "host": ssh_host,
                "remote_dir": dir,
                "target": open_target.as_str(),
            }))
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

        Request::RemoteFolder {
            action,
            host: ssh_host,
            path,
            tab,
            focus,
            all,
            force,
            enabled,
            terminal,
        } => dispatch_remote_folder(
            host, origin, &action, ssh_host, path, tab, focus, all, force, enabled, terminal,
        ),

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
                        host.detach_session(target, close_origin_of(origin), None);
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

            // シェル指定時はコマンドを包む（包み方は宣言されたシェルの方言で決まる。#875）
            let final_command = match &plan.shell {
                Some(shell) => {
                    tako_core::platform::shell::declared_shell_command(shell, &plan.command)
                }
                None => plan.command.clone(),
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

        // #1067: エージェントペインを「会話を引き継いで」建て直す。
        // mode 省略 = 下見（何も起こさない）
        Request::SessionRestart { pane, mode } => {
            dispatch_session_restart(host, origin, pane, mode.as_deref())
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

        // 移行はファイル操作だけで完結する（GUI の状態に触らない）。
        // GUI が壊れた設定で起動できない状況でも CLI から同じ経路が使えることが本質
        Request::Migrate { action, schema } => {
            crate::migrations::report_json(action.as_deref().unwrap_or("status"), schema.as_deref())
                .map_err(DispatchError::InvalidParams)
        }
        Request::Welcome { action } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => {}
                "show" => host.set_welcome_banner_visible(true),
                "dismiss" => {
                    host.set_welcome_banner_visible(false);
                    // ユーザー設定を汚さない（Theme / LimitService と同方針）
                    let should_save = !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none();
                    if should_save {
                        if let Err(e) = crate::welcome::mark_dismissed() {
                            return Err(DispatchError::Operation(format!(
                                "ウェルカムバナーの状態を保存できない: {e}"
                            )));
                        }
                    }
                }
                other => {
                    return Err(DispatchError::InvalidParams(format!(
                        "不明な action: {other:?}（status / show / dismiss のいずれか）"
                    )))
                }
            }
            Ok(welcome_status(host))
        }
    }
}

/// ウェルカムバナーの状態 + 案内コマンド（Issue #549）。
/// コマンドは #322 の最簡形で返す（AI がユーザーへそのまま提示できる）
fn welcome_status(host: &dyn ControlHost) -> Value {
    json!({
        "visible": host.welcome_banner_visible(),
        "dismissed": crate::settings::load().welcome_dismissed,
        "first_launch": crate::welcome::is_first_launch(),
        "setup_command": crate::welcome::SETUP_COMMAND,
        "master_command": crate::welcome::MASTER_COMMAND,
    })
}

// --- AI コマンド提案カード (#666) ---

/// カード 1 枚の JSON 表現。`commands` は**AI が渡した論理文字列そのもの**
/// （画面の折り返しは一切混ざらない）
fn command_card_json(card: &tako_core::CommandCard) -> Value {
    json!({
        "id": card.id().as_u64(),
        "pane": card.pane().as_u64(),
        "label": card.label(),
        "commands": card.commands(),
        "count": card.commands().len(),
    })
}

/// カード保管庫を持たないホスト（GUI 不在）向けのエラー
fn cards_unsupported() -> DispatchError {
    DispatchError::Operation(
        "コマンド提案カードは GUI（tako-app）が必要（この接続先には保管庫が無い）".into(),
    )
}

/// `CommandCardError` → `DispatchError`。入力の不備は InvalidParams、
/// 対象が見つからないのは Operation（呼び出し方は正しいが状態が合わない）
fn command_card_err(e: tako_core::CommandCardError) -> DispatchError {
    match e {
        tako_core::CommandCardError::CardNotFound { id } => DispatchError::Operation(if id == 0 {
            "このペインに表示中のコマンドカードが無い".into()
        } else {
            e.to_string()
        }),
        _ => DispatchError::InvalidParams(e.to_string()),
    }
}

/// AI コマンド提案カードの操作（FR-2.22 / #666）。
///
/// **UI のボタンもここを通る**（カードのコピー / 実行は UI から dispatch を呼ぶ）。
/// UI 層に独自のコピー・実行ロジックを置かないことで、CLI / MCP と挙動が一致する
#[allow(clippy::too_many_arguments)]
fn dispatch_show_command(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    action: &str,
    commands: &[String],
    label: Option<&str>,
    pane: Option<u64>,
    card: Option<u64>,
    index: Option<usize>,
    focus: Option<bool>,
) -> Result<Value, DispatchError> {
    let card_id = card.map(tako_core::CommandCardId::from_raw);

    // 対象カードの所在ペイン。カード ID が明示されていればそこから引ける
    // （ペイン指定なしの CLI からでも copy / run / dismiss ができる）
    let card_pane = card_id.and_then(|id| {
        host.command_cards()
            .and_then(|c| c.get(id))
            .map(|c| c.pane().as_u64())
    });

    match action {
        "show" => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let store = host.command_cards_mut().ok_or_else(cards_unsupported)?;
            let id = store
                .show(target, commands, label)
                .map_err(command_card_err)?;
            let store = host.command_cards().ok_or_else(cards_unsupported)?;
            let shown = store
                .get(id)
                .ok_or_else(|| DispatchError::Operation("カードの登録直後に見失った".into()))?;
            let value = json!({
                "card": command_card_json(shown),
                "pane_cards": store.list(Some(shown.pane())).len(),
                "max_cards_per_pane": tako_core::command_card::MAX_CARDS_PER_PANE,
            });
            Ok(value)
        }

        "list" => {
            let (_, target) = resolve_pane(host.workspace(), pane.or(card_pane))?;
            let store = host.command_cards().ok_or_else(cards_unsupported)?;
            Ok(json!({
                "pane": target.as_u64(),
                "cards": store
                    .list(Some(target))
                    .into_iter()
                    .map(command_card_json)
                    .collect::<Vec<_>>(),
                "total": store.len(),
            }))
        }

        "copy" | "run" => {
            // カード ID を指定したのに見つからない = 既に閉じたカード（UI のボタンが
            // 古い ID を握っている等）。「ペイン未指定」より先にこれを言う
            if let (Some(id), None) = (card_id, card_pane) {
                return Err(command_card_err(
                    tako_core::CommandCardError::CardNotFound { id: id.as_u64() },
                ));
            }
            let (tab_id, target) = resolve_pane(host.workspace(), card_pane.or(pane))?;
            let idx = index.unwrap_or(1);
            let (resolved_id, command) = {
                let store = host.command_cards().ok_or_else(cards_unsupported)?;
                let card = store.resolve(target, card_id).map_err(command_card_err)?;
                (
                    card.id().as_u64(),
                    card.command(idx).map_err(command_card_err)?.to_string(),
                )
            };
            if action == "copy" {
                if !host.queue_clipboard_copy(command.clone()) {
                    return Err(DispatchError::Operation(
                        "クリップボードへ書き込めない（GUI が必要）".into(),
                    ));
                }
                return Ok(json!({
                    "copied": true,
                    "card": resolved_id,
                    "index": idx,
                    // 論理文字列をそのまま返す（AI 側でも同一性を検証できる）
                    "command": command,
                    "bytes": command.len(),
                }));
            }
            // run: 同じタブに新しいペインを分割してそこで実行する。
            // 手元のペイン（AI と対話中のペイン）には一切書き込まない
            let cwd = host
                .session(target)
                .and_then(|s| s.cwd())
                .filter(|p| p.is_dir())
                .map(|p| p.to_path_buf());
            // focus=false のときのフォーカス保持は spawn_command_pane が担う（#676）。
            // カードの実行は「手元のペインに触らない」が要件（FR-2.22.4）
            let focus = focus.unwrap_or(false);
            let new_id = spawn_command_pane(
                host,
                origin,
                tab_id,
                target,
                Direction::Down,
                0.35,
                cwd.clone(),
                &command,
                "never",
                focus,
            )?;
            // タイトルは Code Runner (#453) と同じ `(>)` 接頭辞 + コマンド先頭
            let head: String = command
                .lines()
                .next()
                .unwrap_or(&command)
                .chars()
                .take(24)
                .collect();
            if let Some(p) = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .and_then(|t| t.tree_mut().get_mut(new_id))
            {
                p.set_title(Some(format!("(>) {head}")));
            }
            Ok(json!({
                "pane": new_id.as_u64(),
                "from_pane": target.as_u64(),
                "card": resolved_id,
                "index": idx,
                "command": command,
                "cwd": cwd.map(|p| p.display().to_string()),
                "focus": focus,
            }))
        }

        "dismiss" => {
            // カード ID 指定なら所在ペインを問わず消せる。省略時は対象ペインの全件
            let target = if card_id.is_some() {
                None
            } else {
                Some(resolve_pane(host.workspace(), pane)?.1)
            };
            let store = host.command_cards_mut().ok_or_else(cards_unsupported)?;
            let removed = store.dismiss(target, card_id);
            Ok(json!({
                "dismissed": removed,
                "pane": target.map(|p| p.as_u64()),
                "card": card,
                "remaining": store.len(),
            }))
        }

        other => Err(DispatchError::InvalidParams(format!(
            "不明な action: {other:?}（show / list / copy / run / dismiss のいずれか）"
        ))),
    }
}

/// RunInteractive / Run / ShowCommand 共通: 分割 → コマンド付きセッション起動 →
/// exit マーカーラップ。
///
/// **`focus` の規約は `Request::Split` と同じ**: false なら分割前のフォーカスを保つ
/// （ユーザーの入力を奪わない）。`PaneTree::split_with_ratio` は無条件で新ペインへ
/// フォーカスを移すので、ここで戻さないと「既定 false」が効かない（#676）
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

    // 分割前のフォーカス（focus=false のときここへ戻す。#676）
    let focused_before = host.workspace().get_tab(tab_id).map(|t| t.tree().focused());

    tree_mut(host.workspace_mut(), tab_id)
        .split_with_ratio(target, direction.to_core(), ratio, new_pane)
        .map_err(op_err)?;

    // 実行ペインの起動コマンドはシェルの方言差があるので境界（B1）へ委ねる。
    // ここで `/bin/sh -c` を直書きしていたため Windows では PTY が立たなかった（#875）
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
    } else if let Some(prev) = focused_before.filter(|p| *p != new_id) {
        // 分割の副作用で移ったフォーカスを元へ戻す（#676）
        let _ = tree_mut(host.workspace_mut(), tab_id).focus(prev);
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
/// #1069: ペイン（または session id）の Claude 公式リンクを返す。
///
/// **`/api/agents` / `/api/v2/panes` と同じ 1 実装**（`link_for_agent_session`）を
/// 通すので 3 経路の値が食い違わない。解決順は
/// `id` 明示 → `pane` 明示 → 呼び出し元ペイン → アクティブタブのフォーカスペイン。
///
/// ペイン → セッションは live 解決（`claude agents --json` の pid 祖先辿り）→
/// セッションカタログの順（`/api/v2/panes` と同じ規則）
fn dispatch_sessions_link(
    host: &mut dyn ControlHost,
    _origin: PaneOrigin,
    id_prefix: Option<&str>,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    // id 明示: カタログで前方一致を解いてから引く（agent 種別もカタログから取る）
    if let Some(prefix) = id_prefix {
        let catalog = crate::sessions::SessionCatalog::load().map_err(DispatchError::Operation)?;
        let (session_id, entry) = catalog
            .resolve_id(prefix)
            .map_err(DispatchError::Operation)?;
        let agent = entry.agent.as_deref().unwrap_or("claude");
        let link = crate::claude_remote_link::link_for_agent_session(agent, Some(session_id));
        return Ok(json!({
            "session_id_resolved": session_id,
            "agent": agent,
            "pane": entry.pane,
            "remote_link": link.to_json(),
        }));
    }

    // ペイン解決（`sessions resume` と同じフォールバック = tako 外の CLI からも引ける）
    let (_tab_id, pane_id) = match resolve_pane(host.workspace(), pane) {
        Ok(resolved) => resolved,
        Err(_) if pane.is_none() => {
            let active = host.workspace().active_tab();
            (active.id(), active.tree().focused())
        }
        Err(e) => return Err(e),
    };

    // agent 種別はペインの role から推定する（`list_to_api_v2` と同じ規則）。
    // role が無いペインは claude 前提で引き、繋がっていなければ not_connected になる
    let role = host
        .workspace()
        .get_tab(_tab_id)
        .and_then(|tab| {
            tab.tree()
                .panes()
                .into_iter()
                .find(|p| p.id() == pane_id)
                .and_then(|p| p.role().map(str::to_string))
        })
        .unwrap_or_default();
    let agent = if role.contains("codex") {
        "codex"
    } else if role.contains("agy") {
        "agy"
    } else {
        "claude"
    };

    // live 解決 → 器なしの pid 経路 → カタログ。
    // 2 段目が要るのは、**器（tmux / psmux）を持たない構成**では
    // `backend_session` が無く 1 段目が必ず空振りするから（#728 と同じ二段構え）。
    // 器なしの手がかりは PTY 直下の子 pid で、`list_agents_for_scan` が
    // 祖先辿りで `tako_pane` を付けてくれる（同じ 1 実装を通す）
    let session_id = resolve_session_id_for_pane_via_host(host, pane_id)
        .or_else(|| resolve_session_id_for_pane_via_pid(host, pane_id))
        .or_else(|| crate::sessions::resolve_session_for_pane(&pane_id.as_u64().to_string()));
    let link = crate::claude_remote_link::link_for_agent_session(agent, session_id.as_deref());
    // #1077: 開けない理由に添える opt-in コマンドを具体形にする
    // （PWA / CLI / MCP が同じ 1 実装から同じ値を得る）
    let hint = crate::claude_remote_link::ProfileHint::from_role(&role);
    Ok(json!({
        "pane": pane_id.as_u64(),
        "agent": agent,
        "session_id_resolved": session_id,
        "remote_link": link.to_json_with_profile(hint),
    }))
}

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
    /// プロファイル種別（"master" = tako master / "solo" = tako solo。省略時 master。#721）
    pub kind: Option<String>,
    /// copy の複製元プロファイル名（#721）
    pub from: Option<String>,
    /// このプロファイルに割り当てるプロジェクトキー（丸ごと置き換え。#721）
    pub projects: Option<Vec<String>>,
    /// projects の指定を解除する（#721）
    pub clear_projects: bool,
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
    /// 引き継ぎを始める ctx 閾値（%。50〜60。Issue #749）
    pub ctx_threshold: Option<u32>,
    pub clear_ctx_threshold: bool,
    /// 閾値超過時に tako が master へ引き継ぎを促すか（Issue #749）
    pub auto_handoff: Option<bool>,
    pub clear_auto_handoff: bool,
    /// spawn した worker で利用上限後の自動復帰を既定 ON にするか（Issue #822）
    pub limit_resume: Option<bool>,
    pub clear_limit_resume: bool,
    /// codex の `--dangerously-bypass-approvals-and-sandbox` を許可するか（Issue #981）。
    /// bool 1 つで表せる方針なので clear は要らない（false が「既定へ戻す」）
    pub bypass_sandbox: Option<bool>,
    /// claude の会話を Claude 公式の Remote Control へ繋ぐか（Issue #1068）。
    /// bypass_sandbox と同じ理由で clear は要らない（false が「繋がない」= 既定）
    pub remote_control: Option<bool>,
}

/// プロファイルを JSON 化する（list / show / set の共通形）。
/// model が null のときは claude CLI の既定モデルで起動することを表す。
/// `kind` は保存先ディレクトリ（master = profiles/ / solo = solo-profiles/。#721）
fn profile_to_json(
    kind: crate::orchestrator::ProfileKind,
    name: &str,
    profile: &crate::orchestrator::Profile,
) -> Value {
    use crate::orchestrator;
    let mut v = json!({
        "name": name,
        "kind": kind.as_str(),
        "model": profile.model,
        "effort": profile.effort,
        "worker_model_policy": profile.worker_model_policy,
        "worker_model": profile.worker_model,
        "worker_effort": profile.worker_effort,
        "resolved_worker_model": profile.resolve_worker_model(),
        "resolved_worker_effort": profile.resolve_worker_effort(),
        "path": kind.dir()
            .map(|d| d.join(format!("{name}.yaml")).display().to_string()),
    });
    // 参照整合性の警告（未登録 project / アカウント / [1m] モデル）は list / show /
    // set のすべてに載せる。GUI は保存前の確認に、CLI / MCP は起動前の気づきに使う（#721）
    let warnings = orchestrator::profile_warnings(profile);
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
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
    // 自動ハンドオフ設定（#749）。実効値（config.yaml / 既定へのフォールバック込み）も
    // 併記する = master が「今どの閾値で動くのか」を 1 回の呼び出しで確定できる
    if profile.ctx_threshold.is_some() {
        v["ctx_threshold"] = json!(profile.ctx_threshold);
    }
    if profile.auto_handoff.is_some() {
        v["auto_handoff"] = json!(profile.auto_handoff);
    }
    let resolved = profile.resolved_ctx_threshold();
    v["resolved_ctx_threshold"] = json!(resolved.value);
    v["ctx_threshold_source"] = json!(resolved.source.as_str());
    v["resolved_auto_handoff"] = json!(orchestrator::auto_handoff_enabled(profile));
    // worker の自動復帰の既定（#822）。実効値も併記して「今 spawn したらどうなるか」を
    // 1 回の呼び出しで確定できるようにする（ctx_threshold と同じ流儀）
    if profile.limit_resume.is_some() {
        v["limit_resume"] = json!(profile.limit_resume);
    }
    let limit_resume_resolved = orchestrator::resolve_worker_limit_resume(profile, None);
    v["resolved_limit_resume"] = json!(limit_resume_resolved);
    // solo は worker を spawn しない（solo prompt が禁止している）ので、ON にしても
    // 効く先が無い。黙って死んだ設定にしないため警告として見せる
    if limit_resume_resolved && kind == orchestrator::ProfileKind::Solo {
        let mut warnings: Vec<Value> = v["warnings"].as_array().cloned().unwrap_or_default();
        warnings.push(json!(
            "limit_resume は spawn した worker ペインへ適用される設定ですが、solo プロファイルは worker を spawn しません（この設定は効きません）。\n  ペイン単位で有効にするには `tako limit-resume on --pane <id>` を使ってください"
        ));
        v["warnings"] = Value::Array(warnings);
    }
    // codex のサンドボックス解除（#981）。値そのものは常に返す（既定 false でも
    // 「今どうなっているか」が読めるようにする = 安全に関わる設定なので隠さない）
    v["bypass_sandbox"] = json!(profile.bypass_sandbox);
    // codex worker の skip_permissions を頼んでいるのにサンドボックス解除が無いと
    // フラグが 1 つも付かない（codex は両者が同一フラグ）。効かない設定を黙って
    // 抱えさせないため警告で見せる
    let codex_skip_requested = profile
        .worker_agents
        .get(orchestrator::WorkerAgent::Codex.as_str())
        .map(|c| c.skip_permissions)
        .unwrap_or_else(|| orchestrator::WorkerAgent::Codex.default_skip_permissions());
    let codex_in_use = profile.worker_agent.as_deref() == Some("codex")
        || profile.master_agent.as_deref() == Some("codex")
        || profile
            .worker_agents
            .contains_key(orchestrator::WorkerAgent::Codex.as_str());
    if codex_in_use && codex_skip_requested && !profile.bypass_sandbox {
        let mut warnings: Vec<Value> = v["warnings"].as_array().cloned().unwrap_or_default();
        warnings.push(json!(
            "codex は承認スキップとサンドボックス解除が同じフラグ（--dangerously-bypass-approvals-and-sandbox）なので、bypass_sandbox が false のあいだ skip_permissions は効きません（コマンド実行に承認プロンプトが出ます）。\n  外す場合は `--bypass-sandbox true`（サンドボックスと承認が両方無効になります）"
        ));
        v["warnings"] = Value::Array(warnings);
    }
    // Remote Control の opt-in（#1068）。値そのものは常に返す（bypass_sandbox と同じ理由 =
    // 会話が外へ同期されるかどうかは安全に関わるので隠さない）
    v["remote_control"] = json!(profile.remote_control_enabled());
    // **今 spawn したらどうなるか**も返す。opt-in していても環境が不適格なら
    // フラグは付かないので、設定値だけを見せると「繋がっているはず」の誤解を作る
    if profile.remote_control_enabled() {
        let decision = orchestrator::master_remote_control_decision(profile);
        match decision {
            Ok(d) => {
                v["remote_control_effective"] = json!(d.enabled());
                if let Some(blocked) = d.blocked {
                    v["remote_control_blocked"] = json!({
                        "kind": blocked.kind(),
                        "detail": blocked.detail(),
                        "reason": blocked.reason().text(),
                        "next_step": blocked.next_step().text(),
                    });
                    let mut warnings: Vec<Value> =
                        v["warnings"].as_array().cloned().unwrap_or_default();
                    warnings.push(json!(format!(
                        "remote_control: true ですが、この環境では有効にできません（{}）。\n  {} / {}",
                        blocked.detail(),
                        blocked.reason().text(),
                        blocked.next_step().text()
                    )));
                    v["warnings"] = Value::Array(warnings);
                }
            }
            // アカウント / master_agent の解決に失敗する状態は既存の警告が出す領分。
            // ここでは「判断できなかった」ことだけを残す（嘘の true を書かない）
            Err(_) => v["remote_control_effective"] = Value::Null,
        }
    } else {
        v["remote_control_effective"] = json!(false);
    }
    v
}

/// プロファイル管理（list / show / set / create / copy / delete）。
/// ファイル直読みなので tako-core の状態に依存しない。kind で master / solo を切り替える（#721）
pub fn dispatch_orchestrator_profiles(params: ProfilesParams) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    // 種別（master / solo）。省略時は従来どおり master（完全後方互換。#721）
    let kind = match params.kind.as_deref() {
        Some(k) => orchestrator::ProfileKind::parse(k).map_err(DispatchError::InvalidParams)?,
        None => orchestrator::ProfileKind::Master,
    };
    match params.action.as_str() {
        "list" => {
            let names = orchestrator::list_profiles_of(kind).map_err(DispatchError::Operation)?;
            let profiles: Vec<Value> = names
                .iter()
                .map(|n| match orchestrator::load_profile_of(kind, n) {
                    Ok(p) => profile_to_json(kind, n, &p),
                    // 壊れた yaml も一覧から隠さない（直し方は error 文言に入っている）。
                    // default に丸めて表示すると「壊れていない」と誤認させる
                    Err(e) => json!({ "name": n, "kind": kind.as_str(), "error": e }),
                })
                .collect();
            Ok(json!({ "kind": kind.as_str(), "profiles": profiles }))
        }
        "show" => {
            let name = params.name.as_deref().unwrap_or("default");
            let profile = match orchestrator::load_profile_of(kind, name) {
                Ok(p) => p,
                Err(_) if name == "default" => kind.default_profile(),
                Err(e) => return Err(DispatchError::Operation(e)),
            };
            Ok(profile_to_json(kind, name, &profile))
        }
        "create" => {
            let name = params
                .name
                .ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let (path, profile) = orchestrator::create_profile_of(kind, &name, None)
                .map_err(DispatchError::Operation)?;
            let mut result = profile_to_json(kind, &name, &profile);
            result["path"] = json!(path.display().to_string());
            result["created"] = json!(true);
            Ok(result)
        }
        "copy" => {
            let name = params.name.ok_or(DispatchError::InvalidParams(
                "name（複製先）を指定する".into(),
            ))?;
            let from = params.from.ok_or(DispatchError::InvalidParams(
                "from（複製元）を指定する".into(),
            ))?;
            let base =
                orchestrator::load_profile_of(kind, &from).map_err(DispatchError::Operation)?;
            let (path, profile) = orchestrator::create_profile_of(kind, &name, Some(base))
                .map_err(DispatchError::Operation)?;
            let mut result = profile_to_json(kind, &name, &profile);
            result["path"] = json!(path.display().to_string());
            result["copied_from"] = json!(from);
            Ok(result)
        }
        "delete" => {
            let name = params
                .name
                .ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let path =
                orchestrator::delete_profile_of(kind, &name).map_err(DispatchError::Operation)?;
            Ok(json!({
                "kind": kind.as_str(),
                "name": name,
                "deleted": true,
                "path": path.display().to_string(),
            }))
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
            if params.projects.is_some() && params.clear_projects {
                return Err(DispatchError::InvalidParams(
                    "projects と clear_projects は同時に指定できない".into(),
                ));
            }
            // #749: 閾値は範囲外を黙って丸めず、設定時点でエラーにする
            if let Some(v) = params.ctx_threshold {
                tako_core::handoff::parse_ctx_threshold(v).map_err(DispatchError::InvalidParams)?;
            }
            if params.ctx_threshold.is_some() && params.clear_ctx_threshold {
                return Err(DispatchError::InvalidParams(
                    "ctx_threshold と clear_ctx_threshold は同時に指定できない".into(),
                ));
            }
            if params.auto_handoff.is_some() && params.clear_auto_handoff {
                return Err(DispatchError::InvalidParams(
                    "auto_handoff と clear_auto_handoff は同時に指定できない".into(),
                ));
            }
            if params.limit_resume.is_some() && params.clear_limit_resume {
                return Err(DispatchError::InvalidParams(
                    "limit_resume と clear_limit_resume は同時に指定できない".into(),
                ));
            }
            let env_set_clone = params.env_set.clone();
            let env_unset_clone = params.env_unset.clone();
            let master_account_clone = params.master_account.clone();
            let worker_account_clone = params.worker_account.clone();
            let clear_master_account = params.clear_master_account;
            let clear_worker_account = params.clear_worker_account;
            let projects_clone = params.projects.clone();
            let clear_projects = params.clear_projects;
            // ロック付き read-modify-write（#169）。パースできない既存プロファイルを
            // default に丸めて上書き保存すると設定が消えるため、Err で中断する
            let (path, profile) = orchestrator::mutate_profile_of(kind, &name, |profile| {
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
                // プロジェクト割り当て（丸ごと置き換え。空配列はクリアと同義。#721）
                if let Some(keys) = projects_clone {
                    profile.projects = if keys.is_empty() { None } else { Some(keys) };
                } else if clear_projects {
                    profile.projects = None;
                }
                // 自動ハンドオフ設定（#749）
                if let Some(v) = params.ctx_threshold {
                    profile.ctx_threshold = Some(v);
                } else if params.clear_ctx_threshold {
                    profile.ctx_threshold = None;
                }
                if let Some(v) = params.auto_handoff {
                    profile.auto_handoff = Some(v);
                } else if params.clear_auto_handoff {
                    profile.auto_handoff = None;
                }
                // worker の自動復帰の既定（#822）
                if let Some(v) = params.limit_resume {
                    profile.limit_resume = Some(v);
                } else if params.clear_limit_resume {
                    profile.limit_resume = None;
                }
                // codex のサンドボックス解除（#981）
                if let Some(v) = params.bypass_sandbox {
                    profile.bypass_sandbox = v;
                }
                // Remote Control の opt-in（#1068）。false は「明示的に繋がない」なので
                // None（未設定）と区別せず false を書く（既定と同じ挙動 = 冪等）
                if let Some(v) = params.remote_control {
                    profile.remote_control = Some(v);
                }
                // 既定値のみになったエントリは掃除する（YAML を汚さない）
                profile
                    .worker_agents
                    .retain(|_, c| *c != orchestrator::AgentWorkerConfig::default());
                profile.clone()
            })
            .map_err(DispatchError::Operation)?;
            // 参照整合性の警告（[1m] モデル・未登録 project / アカウント）は
            // profile_to_json が profile_warnings から載せる（GUI / CLI / MCP 共通。#721）
            let mut result = profile_to_json(kind, &name, &profile);
            result["path"] = json!(path.display().to_string());
            Ok(result)
        }
        other => Err(DispatchError::InvalidParams(format!(
            "action が不正: {other}（list / show / set / create / copy / delete）"
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

/// 実行ペインが終了コードを報告する行の接頭辞。**実行ペインの唯一の契約**で、
/// 組み立てる側（`platform::shell::run_pane_command`）と読む側（`find_exit_marker`）が
/// この 1 個を共有する。POSIX は `echo "<prefix>$?"`、PowerShell は
/// `Write-Host ('<prefix>' + $__tako_code)` と書き方が違うが、**出る行は同じ形**（#875）
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
/// #790: ペインが解決できない経路なので、「エージェント管理下の worker か」は
/// worker レジストリ（#390）へ問う。worker なら peer 送達（Cross-Session Messaging）を
/// 先に試し、使えなければ従来のキー操作経路へ落ちる
///
/// **到達可否の判断は呼び出し側が `crate::reach` へ問う**。ここは tmux 固有の
/// 送達手順（capture → 貼り付け → 分離 Enter → 空検証）そのものであり、
/// 案 B-1（器だけの ConPTY セッションホスト）が入ったら同等の手順を
/// その実装向けに用意して `DetachedAccess` 側へ載せる（設計 §5.1）
fn spawn_tmux_delivery(session: String, text: String, wait_ready: bool) {
    std::thread::spawn(move || {
        // Enter 単独送達（#95。入力欄に残留したテキストの送信代行）は
        // 「キーを送れ」という要求そのものなので peer 送達の対象外
        if !text.trim().is_empty() {
            let agent_managed = crate::peer_messaging::backend_is_registered_worker(&session);
            match crate::delivery::try_peer(&session, &text, agent_managed) {
                crate::delivery::PeerAttempt::Sent(outcome) => {
                    if outcome.verification.is_some_and(|v| !v.is_received()) {
                        eprintln!("warning: peer 送達の受信を確認できない（session={session}）");
                    }
                    return; // 送り切っている = 従来経路へ落ちない（二重投函の防止）
                }
                crate::delivery::PeerAttempt::Refused { note } => {
                    eprintln!("warning: {note}（session={session}）");
                    return;
                }
                crate::delivery::PeerAttempt::Fallback { reason, .. } => {
                    crate::delivery::log_fallback(&session, reason);
                }
            }
        }
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

/// 解決済みアカウント 1 件の JSON 表現（list / show / add で共通。#504 / #512）
fn account_json(a: &crate::orchestrator::ResolvedAccount) -> Value {
    json!({
        "name": a.name,
        // inherit のアカウントは CLAUDE_CONFIG_DIR を設定しない = パスを持たない
        "config_dir": a.config_dir.path(),
        "inherit": a.config_dir.is_inherit(),
        "description": a.description,
        "default_model": a.default_model,
        "default_effort": a.default_effort,
    })
}

/// AI 系設定の git ベース共有（Issue #513）。host 非依存（ファイルと git だけ）のため
/// pub にし、CLI `tako config` からもローカル呼び出しで共用する
/// （MCP `tako_config_share` と 1:1。GUI が動いていなくても使える）
pub fn dispatch_config_share(
    action: &str,
    target: Option<&str>,
    path: Option<&str>,
    remote: Option<&str>,
    message: Option<&str>,
    no_push: bool,
) -> Result<Value, DispatchError> {
    use crate::config_share;
    let result = match action {
        "status" => config_share::status(),
        "list" => Ok(config_share::list()),
        "init" => config_share::init(path, remote),
        "link" => {
            let target = target.ok_or_else(|| {
                DispatchError::Operation(
                    "link には対象（リポジトリのパスまたは URL）が必要です".into(),
                )
            })?;
            config_share::link(target, path)
        }
        "unlink" => config_share::unlink(),
        "push" => config_share::push(message, no_push),
        "pull" => config_share::pull(),
        other => {
            return Err(DispatchError::Operation(format!(
                "不明な action: {other}。status | init | link | unlink | push | pull | list"
            )))
        }
    };
    result.map_err(DispatchError::Operation)
}

/// SSH ペインの argv（#919）。
///
/// `~/.ssh/config` の Host 設定を反映しつつ、**ツリー側と同じ ControlPath** を通す
/// （`remote_fs::ssh_pane_argv`）。ここで対話ログインした接続がそのまま共有されるので、
/// パスワード認証しか無い相手でも一度入れば以後ツリーが追加認証なしで開く（#65）
fn remote_ssh_argv(ssh_host: &str) -> Vec<String> {
    let hosts = match tako_core::ssh_config::default_ssh_config_path() {
        Some(p) => tako_core::ssh_config::parse_ssh_config(&p),
        None => Vec::new(),
    };
    // `~/.ssh/config` の Host に無い名前もそのまま ssh へ渡す（従来どおり）
    let extra: Vec<String> = match hosts.iter().find(|h| h.name == ssh_host) {
        // `ssh_command()` は `["ssh", "-p", port, "user@host"]` の形。
        // 先頭の `ssh` と末尾の宛先は `ssh_pane_argv` 側が組むので、間だけ貰う
        Some(h) => {
            let cmd = h.ssh_command();
            cmd[1..cmd.len().saturating_sub(1)].to_vec()
        }
        None => Vec::new(),
    };
    let mut argv = tako_core::remote_fs::ssh_pane_argv(ssh_host, &extra);
    // 宛先は config の `User` を反映した形へ差し替える（`ssh_command()` と同じ規則）
    if let Some(h) = hosts.iter().find(|h| h.name == ssh_host) {
        if let Some(user) = &h.user {
            if let Some(last) = argv.last_mut() {
                *last = format!("{user}@{ssh_host}");
            }
        }
    }
    argv
}

/// リモートファイルを**取得したあと**の共通経路（#966 / #1010）。
///
/// SFTP の取得（ネットワーク I/O）だけを分離してあるので、CLI / MCP は同期で、
/// GUI は背景で取ってからここへ入る。**開いたあとの扱い（読み取り専用の判定・
/// プレビューの割り当て・応答の形）は 1 実装**なので経路で食い違わない。
///
/// `fetched` は [`tako_core::remote_fs::fetch_file`] の戻り値
/// （ローカルの写しと、読めた場合の stat）
pub fn remote_open_file_fetched(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    ssh_host: &str,
    file: &str,
    fetched: (std::path::PathBuf, Option<tako_core::remote_fs::RemoteStat>),
    focus: Option<bool>,
) -> Result<Value, DispatchError> {
    use tako_core::remote_fs::{self, RemoteRef};

    let (local, stat) = fetched;
    // #966: 編集は開放したが、**確実に書けないものは読み取り専用のまま**にする
    // （mode のどこにも `w` が無いときだけ。Windows は権限欄が `*` 埋めで
    // 判定材料が無いので「書けるかもしれない」側 = 編集できる）
    let read_only = stat
        .as_ref()
        .and_then(|s| s.writable_hint())
        .map(|writable| !writable)
        .unwrap_or(false);
    let result = dispatch(
        host,
        Request::OpenFile {
            pane: None,
            path: local.display().to_string(),
            mode: None,
            direction: None,
            focus,
            new_tab: false,
        },
        origin,
    )?;
    if let Some(pane) = result["pane"].as_u64() {
        host.set_preview_remote_origin(
            PaneId::from_raw(pane),
            RemoteRef::new(ssh_host.to_string(), file.to_string()),
            read_only,
        );
    }
    let mut out = result;
    out["host"] = json!(ssh_host);
    out["remote_path"] = json!(file);
    out["cached_path"] = json!(local.display().to_string());
    out["read_only"] = json!(read_only);
    if let Some(stat) = &stat {
        out["size"] = json!(stat.size);
        out["mode"] = json!(stat.mode);
        out["mtime"] = json!(stat.mtime);
    }
    out["pending_write"] = json!(remote_fs::has_pending(ssh_host, file));
    Ok(out)
}

/// リモート（SSH 先）フォルダの操作（#919 / #65）。
///
/// GUI の「リモートからフォルダを開く」・CLI `tako remote-folder`・MCP
/// `tako_remote_folder` が**すべてここを通る**（開発不変条件: UI でできることは
/// AI からもできる）。
///
/// # UI スレッドを止めない約束
///
/// ネットワーク I/O（接続・一覧・取得）のうち、**dispatch の中で待つのは
/// `open` / `ls` / `open-file` の 1 回だけ**にしてある。`ssh` には
/// `ConnectTimeout` / `BatchMode` / `ServerAliveInterval` が付いているので上限は
/// 十数秒で、しかも 2 回目以降は ControlMaster に相乗りするので即返る。
/// ツリーの展開（未知のディレクトリを何枚も読む方）は `request_remote_dir` で
/// 背景へ投げ、ここでは待たない
#[allow(clippy::too_many_arguments)]
fn dispatch_remote_folder(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    action: &str,
    ssh_host: Option<String>,
    path: Option<String>,
    tab: Option<u64>,
    focus: Option<bool>,
    all: bool,
    force: bool,
    enabled: Option<bool>,
    terminal: Option<bool>,
) -> Result<Value, DispatchError> {
    use tako_core::remote_fs::{self, RemoteRef};

    /// `RemoteError` を dispatch のエラーへ。**理由と次の一手を落とさない**
    fn to_err(e: remote_fs::RemoteError) -> DispatchError {
        DispatchError::Operation(format!(
            "{} / {} / {}",
            e.summary(),
            e.next_step(),
            e.detail.replace('\n', " ")
        ))
    }

    fn need_host(ssh_host: &Option<String>) -> Result<String, DispatchError> {
        ssh_host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .ok_or_else(|| DispatchError::InvalidParams("host が必要".into()))
    }

    fn need_path(path: &Option<String>) -> Result<String, DispatchError> {
        path.as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .ok_or_else(|| DispatchError::InvalidParams("path が必要".into()))
    }

    /// 対象タブ（省略時はアクティブタブ）
    fn target_tab(host: &dyn ControlHost, tab: Option<u64>) -> Result<TabId, DispatchError> {
        match tab {
            Some(id) => host
                .workspace()
                .tabs()
                .iter()
                .find(|t| t.id().as_u64() == id)
                .map(|t| t.id())
                .ok_or_else(|| DispatchError::Operation(format!("タブ {id} が見つからない"))),
            None => Ok(host.workspace().active_tab_id()),
        }
    }

    match action {
        // 接続してからルートを開く。**失敗したら開かない**（開いてから失敗すると
        // #919 の「タブだけできて中身が無い」に戻る）
        "open" => {
            let ssh_host = need_host(&ssh_host)?;
            // path 省略時はリモートのホーム（sftp の初期 cwd）
            let home = remote_fs::connect(&ssh_host).map_err(to_err)?;
            let dir = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                Some(p) => p.to_string(),
                None => home.clone(),
            };
            // 開く前に「本当にディレクトリとして読めるか」を確かめる。
            // ここで弾くと、ツリーに開けないルートが並ぶのを防げる
            let entries = remote_fs::list_dir(&ssh_host, &dir).map_err(to_err)?;
            let tab_id = target_tab(host, tab)?;
            let remote = RemoteRef::new(ssh_host.clone(), dir.clone());
            // #1041: この経路（メニュー / パレット / CLI / MCP の `open`）は
            // **明示 open** = ユーザーの主作業対象なのでツリーの先頭へ出す。
            // `ssh` 検知の自動追加（#976）は `Auto` のまま後ろに並ぶ
            let added = attach_remote_root(
                host,
                tako_core::remote_fs::RemoteFolder::explicit(remote.clone()),
                tab_id,
            );

            // #1041: フォルダを開いたらターミナルも繋ぐ（VSCode Remote 相当）
            let terminal_json = auto_connect_terminal(
                host,
                origin,
                &ssh_host,
                &dir,
                tab_id,
                focus,
                terminal.unwrap_or(true),
            );

            Ok(json!({
                "opened": added,
                "host": ssh_host,
                "path": dir,
                "home": home,
                "entries": entries.len(),
                "tab": tab_id.as_u64(),
                "label": remote.label(),
                // #1041: この経路は常に明示 open。前後どちらに出るかは
                // 並び規則（`remote_root_placement`）から出すので A/B の env でも
                // 応答と画面が食い違わない
                "origin": tako_core::remote_fs::RemoteOrigin::Explicit.as_str(),
                "placement": tako_core::sidebar::remote_root_order(
                    &[tako_core::remote_fs::RemoteFolder::explicit(remote.clone())],
                    host.remote_root_placement(),
                )
                .placement_of(&remote),
                "terminal": terminal_json,
            }))
        }

        // 閉じるのは**全タブ横断が既定**。`--tab` で 1 タブへ絞る。
        //
        // タブ単位に閉じ込めると「開いたのに閉じられない」（開いたあと別タブへ
        // 移っていると空振りする）ので、host / path という**グローバルな指定**へ
        // 素直な意味を持たせる（実測でこの取り違えを踏んだ）
        "close" => {
            let scope: Vec<TabId> = match tab {
                Some(_) => vec![target_tab(host, tab)?],
                None => host.workspace().tabs().iter().map(|t| t.id()).collect(),
            };
            let want_host = if all {
                None
            } else {
                Some(need_host(&ssh_host)?)
            };
            let want_path = path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string);
            let mut closed: Vec<String> = Vec::new();
            let mut touched_hosts: Vec<String> = Vec::new();
            for tab_id in scope {
                let Some(t) = host.workspace_mut().get_tab_mut(tab_id) else {
                    continue;
                };
                let targets: Vec<RemoteRef> = t
                    .remote_folders()
                    .iter()
                    .map(|f| &f.remote)
                    .filter(|r| want_host.as_deref().map(|h| r.host == h).unwrap_or(true))
                    .filter(|r| want_path.as_deref().map(|p| r.path == p).unwrap_or(true))
                    .cloned()
                    .collect();
                for remote in targets {
                    if t.remove_remote_folder(&remote) {
                        if !touched_hosts.contains(&remote.host) {
                            touched_hosts.push(remote.host.clone());
                        }
                        closed.push(remote.label());
                    }
                }
            }
            if closed.is_empty() {
                let what = match (want_host.as_deref(), want_path.as_deref()) {
                    (Some(h), Some(p)) => format!("{h}:{p}"),
                    (Some(h), None) => h.to_string(),
                    (None, _) => "リモートフォルダ".to_string(),
                };
                return Err(DispatchError::Operation(format!(
                    "開いていない: {what}（`list` で開いているものを確認できる）"
                )));
            }
            // どのタブからも参照されなくなったホストは接続（ControlMaster）も畳む。
            // 開いたままにすると `ControlPersist` の間ずっと ssh が居座る
            let still_open: Vec<String> = host
                .workspace()
                .tabs()
                .iter()
                .flat_map(|t| t.remote_folders().iter().map(|f| f.remote.host.clone()))
                .collect();
            for h in touched_hosts {
                if !still_open.contains(&h) {
                    remote_fs::close_master(&h);
                }
            }
            host.sync_filetree();
            Ok(json!({ "closed": closed }))
        }

        "list" => {
            let states = host.remote_folder_states();
            let placement = host.remote_root_placement();
            let tabs: Vec<Value> = host
                .workspace()
                .tabs()
                .iter()
                .map(|t| {
                    // #1041: **ツリーに出ている並び**で返す（設計メモ「CLI
                    // `remote-folder list` にも並び順が反映されること」）。
                    // タブは新しく開いたものを先頭に持つので、表示順（開いた順）へ
                    // 直してから `remote_root_order`（並び規則の正本）へ渡す
                    let opened_order: Vec<tako_core::remote_fs::RemoteFolder> =
                        t.remote_folders().iter().rev().cloned().collect();
                    let order =
                        tako_core::sidebar::remote_root_order(&opened_order, placement);
                    let origin_of = |remote: &RemoteRef| {
                        opened_order
                            .iter()
                            .find(|f| &f.remote == remote)
                            .map(|f| f.origin)
                            .unwrap_or_default()
                    };
                    let folders: Vec<Value> = order
                        .display_order()
                        .into_iter()
                        .map(|r| {
                            let found = states.iter().find(|(sr, _, _)| sr == r);
                            json!({
                                "host": r.host,
                                "path": r.path,
                                "label": r.label(),
                                // loaded / loading / pending / not_displayed / error: <理由>
                                // （not_displayed = 裏タブなのでまだ読んでいない。異常ではない）
                                "state": found.map(|(_, s, _)| s.clone()),
                                "entries": found.map(|(_, _, n)| *n),
                                "connected": remote_fs::master_alive(&r.host),
                                // #1041: どの経路で載ったか / ローカルルートの前か後ろか
                                "origin": origin_of(r).as_str(),
                                "placement": order.placement_of(r),
                            })
                        })
                        .collect();
                    json!({
                        "tab": t.id().as_u64(),
                        "title": t.title(),
                        "remote_folders": folders,
                    })
                })
                .filter(|t| {
                    !t["remote_folders"]
                        .as_array()
                        .map(|a| a.is_empty())
                        .unwrap_or(true)
                })
                .collect();
            // #1010: いま読み込み中のリモートファイル（ツリーがスピナーを出しているもの）。
            // **GUI で回っているものが応答からも読める**ので、AI は「まだ来ていない」と
            // 「開けなかった」を取り違えない
            let loading: Vec<Value> = host
                .remote_files_loading()
                .iter()
                .map(|r| json!({ "host": r.host, "path": r.path, "label": r.label() }))
                .collect();
            Ok(json!({ "tabs": tabs, "loading_files": loading }))
        }

        // ツリーを開かずに覗く（#65 要件 5: AI がリモートの構造を把握する経路）
        "ls" => {
            let ssh_host = need_host(&ssh_host)?;
            let dir = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                Some(p) => p.to_string(),
                None => remote_fs::connect(&ssh_host).map_err(to_err)?,
            };
            let entries = remote_fs::list_dir(&ssh_host, &dir).map_err(to_err)?;
            let list: Vec<Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "path": e.path,
                        "kind": match e.kind {
                            remote_fs::RemoteKind::Dir => "dir",
                            remote_fs::RemoteKind::File => "file",
                            remote_fs::RemoteKind::Symlink => "symlink",
                            remote_fs::RemoteKind::Unknown => "unknown",
                        },
                        "size": e.size,
                    })
                })
                .collect();
            Ok(json!({ "host": ssh_host, "path": dir, "entries": list }))
        }

        // SFTP で取得 → 既存のプレビュー経路（OpenFile）へ流す。
        // 構文色・md・画像・PDF・目次・リンクの実装を二重に持たない
        "open-file" => {
            let ssh_host = need_host(&ssh_host)?;
            let file = need_path(&path)?;
            // CLI / MCP は**同期**（#966 の切り分けと同じ。AI が応答だけで
            // 「読めたのか」を判断できることのほうが重要）。GUI は取得だけを背景へ出し、
            // 取れたところで下の `remote_open_file_fetched` を呼ぶ（#1010）
            let fetched = remote_fs::fetch_file(&ssh_host, &file, remote_fs::MAX_PREVIEW_BYTES)
                .map_err(to_err)?;
            remote_open_file_fetched(host, origin, &ssh_host, &file, fetched, focus)
        }

        // 押し出せていない保存の一覧（#966 受け入れ条件 3）。
        // **切断中の保存が無言で消えない**ことを見せる窓口
        "pending" => {
            let entries: Vec<Value> = remote_fs::list_pending()
                .iter()
                .filter(|e| ssh_host.as_deref().map(|h| e.host == h).unwrap_or(true))
                .filter(|e| path.as_deref().map(|p| e.path == p).unwrap_or(true))
                .map(|e| {
                    json!({
                        "host": e.host,
                        "path": e.path,
                        "label": e.label(),
                        "kind": e.kind,
                        "error": e.error,
                        "at": e.at,
                        "attempts": e.attempts,
                        "size": e.size,
                    })
                })
                .collect();
            Ok(json!({ "pending": entries }))
        }

        // 押し出せていない保存を再試行する（#966 受け入れ条件 3）。
        // host / path 省略で全件。`force` は競合を承知で上書きする
        "push" => {
            let want_host = ssh_host.as_deref().map(str::trim).filter(|h| !h.is_empty());
            let want_path = path.as_deref().map(str::trim).filter(|p| !p.is_empty());
            let targets: Vec<(String, String)> = remote_fs::list_pending()
                .iter()
                .filter(|e| want_host.map(|h| e.host == h).unwrap_or(true))
                .filter(|e| want_path.map(|p| e.path == p).unwrap_or(true))
                .filter(|e| !e.host.is_empty())
                .map(|e| (e.host.clone(), e.path.clone()))
                .collect();
            if targets.is_empty() {
                return Err(DispatchError::Operation(
                    "押し出せていない保存はありません（`pending` で確認できます）".into(),
                ));
            }
            let mut pushed: Vec<Value> = Vec::new();
            let mut failed: Vec<Value> = Vec::new();
            for (h, pth) in targets {
                match remote_fs::push_pending(&h, &pth, force) {
                    Ok(report) => pushed.push(json!({
                        "host": h,
                        "path": pth,
                        "bytes": report.bytes,
                        "atomic": report.atomic,
                        "verified": report.verified,
                        "mode_restored": report.mode_restored,
                    })),
                    Err(e) => failed.push(json!({
                        "host": h,
                        "path": pth,
                        "kind": e.kind.as_str(),
                        "error": e.summary(),
                        "next_step": e.next_step(),
                        "detail": e.detail.replace('\n', " "),
                    })),
                }
            }
            // **失敗を成功に混ぜて 200 で返さない**（無言で失われるのと同じになる）
            if pushed.is_empty() {
                let first = failed
                    .first()
                    .map(|f| {
                        format!(
                            "{} / {} / {}",
                            f["error"].as_str().unwrap_or_default(),
                            f["next_step"].as_str().unwrap_or_default(),
                            f["detail"].as_str().unwrap_or_default()
                        )
                    })
                    .unwrap_or_default();
                return Err(DispatchError::Operation(first));
            }
            Ok(json!({ "pushed": pushed, "failed": failed }))
        }

        // そのフォルダを cwd にした SSH ペイン（#919 要件 4）
        "ssh-pane" => {
            let ssh_host = need_host(&ssh_host)?;
            let dir = path.as_deref().map(str::trim).filter(|p| !p.is_empty());
            dispatch(
                host,
                Request::OpenRemote {
                    host: ssh_host,
                    focus,
                    remote_dir: dir.map(str::to_string),
                    // #1006: 開き先は既定（現在タブへ新ペイン）に従う。
                    // ツリーの「このフォルダで SSH ペイン」も同じ体験になる
                    target: None,
                    pane: None,
                    tab,
                    direction: None,
                },
                origin,
            )
        }

        // #976: ペインの ssh を検知した自動追加の照会・切替。
        // **状態は GUI（tako-app）側が持つ**ので ControlHost 経由で読む
        // （検知はプロセス表の走査を伴うため CLI プロセス側では再現できない）
        "auto" => {
            if let Some(enabled) = enabled {
                host.set_ssh_auto_folders(enabled);
            }
            let mut payload = host.ssh_auto_folder_status();
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("enabled".into(), json!(host.ssh_auto_folders_enabled()));
            }
            Ok(payload)
        }

        other => Err(DispatchError::InvalidParams(format!(
            "不明な action: {other:?}（open / close / list / ls / open-file / ssh-pane / pending / push / auto のいずれか）"
        ))),
    }
}

/// リモートフォルダをタブのワークスペースへ足してツリーへ出す（#919 / #976）。
///
/// **接続の確認は呼び出し側の責務**。`open` は先に `connect` + `list_dir` で確かめ、
/// #976 の自動追加は同じ確認を background で済ませてからここへ来る
/// （UI スレッドでネットワークを待たせない = #212 / #772 の教訓）。
/// 器づけを 1 実装にしておくのは、片方だけ `sync_filetree` を忘れると
/// 「開いたのにツリーに出ない」形で壊れるため。
///
/// #1041: `folder` は経路（明示 open / `ssh` 検知）を持つ。ツリーの並びは
/// `tako_core::sidebar::remote_root_order` がこの経路から決める
pub fn attach_remote_root(
    host: &mut dyn ControlHost,
    folder: tako_core::remote_fs::RemoteFolder,
    tab_id: TabId,
) -> bool {
    let remote = folder.remote.clone();
    let added = host
        .workspace_mut()
        .get_tab_mut(tab_id)
        .map(|t| t.add_remote_folder(folder))
        .unwrap_or(false);
    host.set_filetree(true);
    host.sync_filetree();
    host.request_remote_dir(&remote);
    added
}

/// フォルダを開いたときにターミナルも同じホストへ繋ぐ（#1041）。
///
/// VSCode Remote / Zed の「リモートで開く」と同じ体験（SSH 済み + そのフォルダへ
/// `cd` 済みのペインが同じタブに用意される）。**既定 ON**（#322「既定を賢く」）で、
/// 同じタブに同じホストへ繋がった生きたペインがあれば作らない。
///
/// 戻り値は `open` 応答の `terminal` オブジェクト。**繋げなかったときは必ず理由**を
/// 載せる（黙って何もしない状態を作らない = #919 の原則）。ペインを作れなくても
/// フォルダは開いたまま残す（#1041 受け入れ条件 3）。
///
/// `pub` にしているのは、GUI のセルフテストが**この 1 実装をそのまま**叩いて
/// 「新しいペインが立つ / 2 回目は増えない」を機械検証するため
/// （`open` そのものは実 SFTP 接続が要るのでセルフテストから通せない）。
pub fn auto_connect_terminal(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    ssh_host: &str,
    dir: &str,
    tab_id: TabId,
    focus: Option<bool>,
    requested: bool,
) -> Value {
    use tako_core::remote_open::{AutoTerminal, AutoTerminalSkip};

    let existing = host.live_ssh_pane(tab_id, ssh_host).map(|p| p.as_u64());
    match tako_core::remote_open::decide_auto_terminal(requested, existing) {
        AutoTerminal::Connect => {
            let opened = dispatch(
                host,
                Request::OpenRemote {
                    host: ssh_host.to_string(),
                    focus,
                    remote_dir: Some(dir.to_string()),
                    // 自動経路は**常に新しいペイン**（既存ペインを乗っ取らない。
                    // 理由は `tako_core::remote_open` の #1041 の節）
                    target: Some(tako_core::remote_open::auto_terminal_target()),
                    pane: None,
                    tab: Some(tab_id.as_u64()),
                    direction: None,
                },
                origin,
            );
            match opened {
                Ok(v) => json!({
                    "connected": true,
                    "pane": v["pane"],
                    "tab": v["tab"],
                    "target": v["target"],
                    "remote_dir": v["remote_dir"],
                }),
                Err(e) => json!({
                    "connected": false,
                    "reason": "failed",
                    "note": e.to_string(),
                }),
            }
        }
        AutoTerminal::Skip(skip) => {
            let mut v = json!({
                "connected": false,
                "reason": skip.as_str(),
                "note": skip.note(),
            });
            if let AutoTerminalSkip::AlreadyConnected { pane } = skip {
                v["pane"] = json!(pane);
            }
            v
        }
    }
}

/// アカウントレジストリの CRUD（Issue #504 / #512）。host 非依存
/// （accounts.yaml の読み書きのみ）のため pub にし、CLI `tako orchestrator accounts`
/// からもローカル呼び出しで共用する（MCP `tako_orchestrator_accounts` と 1:1。
/// 表示・警告・検証を二重実装しない。Issue #548）
pub fn dispatch_orchestrator_accounts(
    action: &str,
    name: Option<&str>,
    config_dir: Option<&str>,
    inherit: Option<bool>,
    description: Option<&str>,
    default_model: Option<&str>,
    default_effort: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    match action {
        "list" => {
            let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
            let accounts: Vec<Value> = config
                .list_resolved()
                .into_iter()
                .map(|(name, resolved)| match resolved {
                    Ok(a) => account_json(&a),
                    // 壊れたエントリも隠さず出す（直し方は error 文言に入っている）
                    Err(e) => json!({ "name": name, "error": e }),
                })
                .collect();
            Ok(json!({ "accounts": accounts }))
        }
        "show" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let config = orchestrator::AccountsConfig::load().map_err(DispatchError::Operation)?;
            let a = config.resolve(name).map_err(DispatchError::Operation)?;
            Ok(account_json(&a))
        }
        "add" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let inherit = inherit.unwrap_or(false);
            // config_dir / inherit の排他は登録時に弾く（壊れたエントリを作らせない。#512）
            match (inherit, config_dir) {
                (false, None) => {
                    return Err(DispatchError::InvalidParams(
                        "config_dir か inherit のどちらかを指定する\
                         （別 config dir なら config_dir、既定の資格情報をそのまま使うなら inherit=true）"
                            .into(),
                    ))
                }
                (true, Some(_)) => {
                    return Err(DispatchError::InvalidParams(
                        "config_dir と inherit=true は同時に指定できない".into(),
                    ))
                }
                _ => {}
            }
            // #512: 既定パスの明示指定は「未設定」と等価ではない（Keychain エントリが分かれ、
            // 既存ログインが見えなくなる）。登録は通すが必ず警告する
            let warning = config_dir
                .filter(|cd| orchestrator::is_claude_default_config_dir(cd))
                .map(|cd| {
                    format!(
                        "config_dir '{cd}' は claude の既定パスです。既定パスを明示指定すると \
                         CLAUDE_CONFIG_DIR が設定された状態になり、claude が Keychain の別エントリ\
                         （ハッシュ付き）を見に行くため既存ログインが未ログイン扱いになります。\
                         既定の資格情報をそのまま使うなら inherit=true で登録してください（#512）"
                    )
                });
            if let Some(w) = &warning {
                eprintln!("warning: {w}");
            }
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
            let a = config.resolve(name).map_err(DispatchError::Operation)?;
            let mut result = account_json(&a);
            if let Some(w) = warning {
                result["warning"] = json!(w);
            }
            Ok(result)
        }
        "remove" => {
            let name = name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            let removed = orchestrator::AccountsConfig::mutate(|config| {
                config.accounts.remove(name).is_some()
            })
            .map_err(DispatchError::Operation)?;
            if removed {
                Ok(json!({ "removed": name }))
            } else {
                Err(DispatchError::Operation(format!(
                    "アカウント '{name}' は登録されていない"
                )))
            }
        }
        other => Err(DispatchError::InvalidParams(format!(
            "action が不正: {other}（list / show / add / remove）"
        ))),
    }
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

    // #288: pid 祖先辿り → pane env → stale map → role（複数時エラー）
    let (tab_id, pane_id) = resolve_caller_pane(host, pane, caller_role, caller_pid)?;

    // #854: master のプロファイルは呼び出し元 env と**ペインの role ラベル**の両方から
    // 解決する（env が失われていても tako 自身の記録から取り戻す）。solo は handoff を
    // 持たないので従来どおり env の `solo:` 接頭辞だけを見る
    let pane_role = host
        .workspace()
        .get_tab(tab_id)
        .and_then(|t| t.tree().get(pane_id))
        .and_then(|p| p.role())
        .map(str::to_string);
    let solo_suffix = caller_role
        .and_then(|r| r.strip_prefix("solo:"))
        .filter(|s| !s.is_empty());
    let (profile_owned, profile_source) = match solo_suffix {
        Some(s) => (s.to_string(), tako_core::handoff::ProfileSource::CallerRole),
        None => tako_core::handoff::resolve_master_profile(caller_role, pane_role.as_deref()),
    };
    let profile_name = profile_owned.as_str();

    // session_id の自動解決（バックエンドセッション → pid 祖先辿り）
    let session_id = resolve_session_id_for_pane_via_host(host, pane_id);

    let (status, ctx_percent) = if let Some(sid) = &session_id {
        let agent_status = orchestrator::query_agent_status(sid);
        (agent_status.status, agent_status.ctx_percent)
    } else {
        ("unknown".to_string(), None)
    };

    // #749: 閾値はプロファイル → config.yaml → 既定 60 の順で解決し 50〜60 へ丸める
    let profile = orchestrator::Profile::load(profile_name).unwrap_or_default();
    let threshold = profile.resolved_ctx_threshold();
    let ctx_threshold = threshold.value;

    // #915 / #916: 読む前に旧形式を自動移行する（実行時の差分検出。冪等）
    let migration = orchestrator::handoff_store::ensure_migrated(profile_name);

    // #915: 管轄プロジェクトと、その引き継ぎファイルのパス群
    let jurisdiction =
        tako_core::handoff::resolve_jurisdiction(&tako_core::handoff::JurisdictionInput {
            explicit: None,
            profile_projects: profile.projects.clone().unwrap_or_default(),
            worker_projects: worker_projects_in_tab(host, tab_id),
        });
    let project_handoffs: Vec<Value> = jurisdiction
        .projects
        .iter()
        .map(|key| {
            let path = orchestrator::handoff_store::project_handoff_path(key);
            let body = orchestrator::handoff_store::read_project_handoff(key);
            let doc = body.as_deref().map(tako_core::handoff::split_handoff);
            json!({
                "project": key,
                "path": path.map(|p| p.display().to_string()),
                "exists": body.is_some(),
                "format": doc.as_ref().map(|d| d.format().as_str()),
                "sections": doc.as_ref().map(|d| d.section_labels()),
            })
        })
        .collect();

    let handoff_path = orchestrator::handoff_path(profile_name);
    let handoff_exists = handoff_path.as_ref().is_some_and(|p| p.is_file());
    // #792: 自分の引き継ぎファイルが新書式（知識 / 実行状態の 2 節）かどうかを master 自身が
    // 確認できるようにする。不在なら null（「まだ書いていない」と「旧書式」を混ぜない）
    let memo_content = orchestrator::read_handoff(profile_name);
    let handoff_doc = memo_content
        .as_deref()
        .map(tako_core::handoff::split_handoff);

    let mut result = json!({
        "pane_id": pane_id.as_u64(),
        "tab_id": tab_id.as_u64(),
        "profile": profile_name,
        // #854: プロファイルの出どころ。pane_role なら呼び出し元の env が失われている
        "profile_source": profile_source.as_str(),
        "role": caller_role,
        "session_id": session_id,
        "status": status,
        "ctx_percent": ctx_percent,
        "ctx_threshold": ctx_threshold,
        "ctx_threshold_source": threshold.source.as_str(),
        "ctx_over_threshold": ctx_percent.map(|c| c >= ctx_threshold),
        "auto_handoff": orchestrator::auto_handoff_enabled(&profile),
        // #915: `handoff_path` は**プロファイル運用メモ**（共通置き場）のパス。
        // プロジェクト固有の引き継ぎは `project_handoffs` の各 path へ書く
        "handoff_path": handoff_path,
        "handoff_exists": handoff_exists,
        "handoff_format": handoff_doc.as_ref().map(|d| d.format().as_str()),
        "handoff_sections": handoff_doc.as_ref().map(|d| d.section_labels()),
        "handoff_projects_dir": orchestrator::handoff_store::projects_handoff_dir()
            .map(|p| p.display().to_string()),
        "project_handoffs": project_handoffs,
        "jurisdiction_source": jurisdiction.source.as_str(),
        // #915: 旧形式からの自動移行の実施（可視化。migrated=false なら何もしていない）
        "handoff_migration": migration,
    });
    // 運用メモが膨らんでいたら肥大の再発なので警告する（#915 要件 3）
    let mut warnings: Vec<String> = migration.warnings.clone();
    warnings.extend(
        memo_content
            .as_deref()
            .and_then(|m| tako_core::handoff::profile_memo_warning(profile_name, m)),
    );
    // 範囲外の手書き設定は黙って丸めず、丸めたことを応答に出す（#749）
    if threshold.clamped() {
        result["ctx_threshold_raw"] = json!(threshold.raw);
        warnings.push(format!(
            "ctx_threshold={} は値域外のため {} へ丸めた（{}〜{}）",
            threshold.raw,
            ctx_threshold,
            tako_core::handoff::CTX_THRESHOLD_MIN,
            tako_core::handoff::CTX_THRESHOLD_MAX
        ));
    }
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
    Ok(result)
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

/// 器を持たないペインの session_id を PTY 直下の子 pid から解決する（#728 の二段構え）。
///
/// `backend_session` が無い構成（Windows で psmux 未導入 / tmux 不在の macOS /
/// `TAKO_PERSIST=0`）では器のセッション名で引けないので、
/// `list_agents_for_scan` に「(tako のペイン ID, PTY 子 pid)」を渡して
/// 祖先辿りで対応付けてもらう
fn resolve_session_id_for_pane_via_pid(host: &dyn ControlHost, pane_id: PaneId) -> Option<String> {
    let child = host.session(pane_id)?.child_pid()?;
    let agents = crate::agents::list_agents_for_scan(&[(pane_id.as_u64(), child)]).ok()?;
    agents["agents"]
        .as_array()?
        .iter()
        .find(|a| a["tako_pane"].as_u64() == Some(pane_id.as_u64()))
        // **正規化後のキー**（`agents::list_agents_*` が `sessionId` を `session_id` へ直す）
        .and_then(|a| a["session_id"].as_str())
        .map(str::to_string)
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

    // 第 2 層: transcript アダプタ（claude / codex）。
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
        result["transcript_agent"] = json!("claude");
        Some(texts)
    });
    // #984: claude の会話ログが無ければ codex の実況を読む（`$CODEX_HOME/sessions/` の
    // rollout JSONL。`response_item` の `role == "assistant"` が発話本文）。
    // これで `report --messages N` が codex worker でも実データを返す
    let transcript = transcript.or_else(|| {
        let backend = backend.as_deref()?;
        let tid = crate::codex_session::resolve_thread_id_for_backend(backend)?;
        let texts = crate::codex_session::last_assistant_texts(&tid, msg_count).ok()?;
        if texts.is_empty() {
            return None;
        }
        result["session_id"] = json!(tid);
        result["transcript_agent"] = json!("codex");
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

/// 折返し結合済みのスクロールバックを取得する（報告の第 1 層）。
/// 実際の採取は永続バックエンドの到達手段（`DetachedAccess`）が担う
fn capture_scrollback_joined(session: &str, lines: usize) -> Option<String> {
    let (session, capture) = crate::reach::detached_capture(session)?;
    capture.capture_history_joined(&session, lines)
}

/// OrchestratorHandoffFiles — 引き継ぎファイルの管理（#915）。
///
/// GUI を持たないローカル処理なので CLI もこの関数を直接呼ぶ（`layout` / `accounts` と同じ形）。
/// action: list（一覧）/ show（1 件）/ write（1 件を書く）/ migrate（旧形式の自動移行）
pub fn dispatch_orchestrator_handoff_files(
    action: &str,
    project: Option<&str>,
    profile: Option<&str>,
    content: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator::handoff_store as store;
    use tako_core::handoff as ho;

    let describe = |path: &std::path::Path| {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        let doc = ho::split_handoff(&body);
        json!({
            "path": path.display().to_string(),
            "lines": body.lines().count(),
            "bytes": body.len(),
            "format": doc.format().as_str(),
            "sections": doc.section_labels(),
        })
    };

    match action {
        "list" => {
            let projects: Vec<Value> = store::list_project_handoffs()
                .into_iter()
                .map(|(key, path)| {
                    let mut v = describe(&path);
                    v["project"] = json!(key);
                    v
                })
                .collect();
            let memos: Vec<Value> = store::list_profile_memos()
                .into_iter()
                .map(|(name, path)| {
                    let mut v = describe(&path);
                    v["profile"] = json!(name);
                    let body = std::fs::read_to_string(&path).unwrap_or_default();
                    v["warning"] = json!(ho::profile_memo_warning(&name, &body));
                    v
                })
                .collect();
            Ok(json!({
                "projects_dir": store::projects_handoff_dir().map(|p| p.display().to_string()),
                "project_handoffs": projects,
                "profile_memos": memos,
                "archive_dir": store::archive_dir().map(|p| p.display().to_string()),
            }))
        }
        "show" => match (project, profile) {
            (Some(key), None) => {
                let path = store::project_handoff_path(key).ok_or_else(|| {
                    DispatchError::InvalidParams(format!(
                        "プロジェクトキーがファイル名として使えない: {key}"
                    ))
                })?;
                let body = store::read_project_handoff(key);
                let doc = body.as_deref().map(ho::split_handoff);
                Ok(json!({
                    "project": key,
                    "path": path.display().to_string(),
                    "exists": body.is_some(),
                    "format": doc.as_ref().map(|d| d.format().as_str()),
                    "sections": doc.as_ref().map(|d| d.section_labels()),
                    "content": body,
                    "template": body.is_none().then(|| ho::project_handoff_template(key)),
                }))
            }
            (None, Some(name)) => {
                let path = crate::orchestrator::handoff_path(name)
                    .ok_or_else(|| op_err("ホームディレクトリが取得できない"))?;
                let body = crate::orchestrator::read_handoff(name);
                let doc = body.as_deref().map(ho::split_handoff);
                Ok(json!({
                    "profile": name,
                    "path": path.display().to_string(),
                    "exists": body.is_some(),
                    "format": doc.as_ref().map(|d| d.format().as_str()),
                    "sections": doc.as_ref().map(|d| d.section_labels()),
                    "content": body,
                    "warning": body.as_deref().and_then(|b| ho::profile_memo_warning(name, b)),
                }))
            }
            _ => Err(DispatchError::InvalidParams(
                "show は project か profile のどちらか一方を指定する".into(),
            )),
        },
        "write" => {
            let body = content.ok_or_else(|| {
                DispatchError::InvalidParams("write は content を指定する".into())
            })?;
            let text = if body.ends_with('\n') {
                body.to_string()
            } else {
                format!("{body}\n")
            };
            match (project, profile) {
                (Some(key), None) => {
                    let path = store::write_project_handoff(key, &text)
                        .map_err(DispatchError::Operation)?;
                    Ok(json!({
                        "project": key,
                        "path": path.display().to_string(),
                        "bytes": text.len(),
                        "format": ho::split_handoff(&text).format().as_str(),
                    }))
                }
                (None, Some(name)) => {
                    let path =
                        store::write_profile_memo(name, &text).map_err(DispatchError::Operation)?;
                    Ok(json!({
                        "profile": name,
                        "path": path.display().to_string(),
                        "bytes": text.len(),
                        "warning": ho::profile_memo_warning(name, &text),
                    }))
                }
                _ => Err(DispatchError::InvalidParams(
                    "write は project か profile のどちらか一方を指定する".into(),
                )),
            }
        }
        // #916 の段 2 を手でも撃てるようにしたもの（通常は自動で走る）
        "migrate" => {
            let outcomes = match profile {
                Some(name) => vec![store::ensure_migrated(name)],
                None => store::migrate_all(),
            };
            let summaries: Vec<String> = outcomes.iter().filter_map(|o| o.summary()).collect();
            Ok(json!({
                "migrations": outcomes,
                "summaries": summaries,
                "migrated": !summaries.is_empty(),
            }))
        }
        other => Err(DispatchError::InvalidParams(format!(
            "未知の action: {other}（list | show | write | migrate）"
        ))),
    }
}

/// このタブで稼働している worker のプロジェクト集合（#915 の管轄推定の材料）。
///
/// レジストリの active エントリのうち、**このタブに実在するペイン**のものだけを拾う
/// （tako の運用は「1 グループ = 1 タブ」で、master が spawn した worker は同じタブに
/// 並ぶ。番号再利用で他タブのペインを拾わないよう、タブの実在で絞る）
fn worker_projects_in_tab(host: &dyn ControlHost, tab_id: TabId) -> Vec<String> {
    let Ok(registry) = crate::orchestrator::registry::WorkerRegistry::load() else {
        return Vec::new();
    };
    let Some(tab) = host.workspace().get_tab(tab_id) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in registry.workers.values().filter(|e| e.is_active()) {
        if entry.project.is_empty() {
            continue;
        }
        if tab.tree().get(PaneId::from_raw(entry.pane)).is_none() {
            continue;
        }
        if !out.contains(&entry.project) {
            out.push(entry.project.clone());
        }
    }
    out
}

/// OrchestratorHandoff — master の引き継ぎ（#193 / #749）。
/// handoff ファイルを読み、同プロファイルの新 master を同タブに spawn し、
/// handoff 内容を含むプロンプトを注入する。
///
/// #749: 旧 master のペインは **新 master が引き継ぎを確認したあとに新 master 自身が
/// 閉じる**（初期プロンプトにその手順を埋め込む）。ここで旧ペインを閉じないのは、
/// 新 master の起動が失敗したときに旧 master を失わないため（順序が安全側に倒れる）。
/// spawn には既存の OrchestratorSpawn（project 経由）を使わず、直接 Split + attach
/// を行う（handoff は「プロジェクト」ではないため projects.yaml に依存しない）
fn dispatch_orchestrator_handoff(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    pane: Option<u64>,
    caller_role: Option<&str>,
    tab: Option<u64>,
    caller_pid: Option<u32>,
    projects: Option<Vec<String>>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    // #288: 分割元ペインの解決。`tab` 指定時はそのタブのフォーカスペインを分割元にする
    // ので、呼び出し元（= 退役する master）とは別物になりうる
    let caller = resolve_caller_pane(host, pane, caller_role, caller_pid).ok();
    let (tab_id, split_target) = match tab {
        Some(raw_tab) => {
            let tid = find_tab(host.workspace(), raw_tab)?;
            let focused = host.workspace().get_tab(tid).unwrap().tree().focused();
            (tid, focused)
        }
        // caller が解決できないときは従来どおり解決エラーをそのまま返す
        None => resolve_caller_pane(host, pane, caller_role, caller_pid)?,
    };

    // #749: 退役する旧 master のペイン。**role が master のペインだけ**を対象にし、
    // ユーザーが開いたペインを後任に閉じさせる事故を構造的に防ぐ
    let previous_pane = caller.map(|(_, p)| p).filter(|p| {
        host.workspace()
            .get_tab(tab_id)
            .and_then(|t| t.tree().get(*p))
            .and_then(|pane| pane.role())
            .is_some_and(|r| r.starts_with("orchestrator-master"))
    });

    // #854: プロファイルは呼び出し元の env（`master:<profile>`）と**ペインの role ラベル**
    // （`orchestrator-master:<profile>`）の両方から解決する。インライン前置きで注入した
    // env はシェルへ export されないので、claude を撃ち直した master は env を失う
    // （実発の症状 `TAKO_ORCHESTRATOR_ROLE=[master]` と一致）。tako 自身が持つ role
    // ラベルを第 2 の出どころにすることで、後任がアカウント・モデル・引き継ぎファイルを
    // 取り違えなくなる
    let pane_role = previous_pane.or(caller.map(|(_, p)| p)).and_then(|p| {
        host.workspace()
            .get_tab(tab_id)
            .and_then(|t| t.tree().get(p))
            .and_then(|pane| pane.role())
            .map(str::to_string)
    });
    let (profile_owned, profile_source) =
        tako_core::handoff::resolve_master_profile(caller_role, pane_role.as_deref());
    let profile_name = profile_owned.as_str();

    // #915 / #916: 読む前に旧形式を自動移行する（実行時の差分検出。冪等）
    let migration = orchestrator::handoff_store::ensure_migrated(profile_name);

    // プロファイルの読み込みとエージェント解決
    let profile = orchestrator::Profile::load(profile_name).unwrap_or_default();

    // #915: 管轄プロジェクトの解決。明示引数 → プロファイル担当 + 稼働 worker → worker のみ
    let jurisdiction =
        tako_core::handoff::resolve_jurisdiction(&tako_core::handoff::JurisdictionInput {
            explicit: projects.clone(),
            profile_projects: profile.projects.clone().unwrap_or_default(),
            worker_projects: worker_projects_in_tab(host, tab_id),
        });
    let bundle = orchestrator::handoff_store::collect_bundle(profile_name, jurisdiction);
    if !bundle.as_successor().has_content() && bundle.catalog.is_empty() {
        let memo_path = orchestrator::handoff_path(profile_name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let projects_dir = orchestrator::handoff_store::projects_handoff_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        // #792 / #915: 書けと言うだけでなく**どこへどう書くか**まで返す。
        // これを最初に読むのは AI なので、ここが書式を知る唯一の機会になりうる
        let template = tako_core::handoff::project_handoff_template("<project-key>");
        return Err(DispatchError::Operation(format!(
            "引き継ぎの材料が無い（プロジェクト単位のファイルも運用メモも空）。\n\
             プロジェクト固有の引き継ぎ: {projects_dir}/<project-key>.md\n\
             プロジェクトに紐付かない運用メモ: {memo_path}\n\
             master は引き継ぎ前に前者へ状態を書き込む必要がある。書式:\n{template}"
        )));
    }
    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env().map_err(DispatchError::Operation)?;
    // 引き継ぎ先の master も master_account を反映する（#547。CLI の master 起動と同じ規則）
    let profile_env = profile
        .resolved_env_plan_for_master()
        .map_err(DispatchError::Operation)?;

    let master_agent = profile
        .resolve_master_agent()
        .map_err(DispatchError::InvalidParams)?;
    // #983: 後任 master の CLI が無ければ**ペインを分割する前に**落とす
    // （引き継ぎで後任が黙って死ぬと、前任を閉じる主体そのものが居なくなる）
    orchestrator::agent_cli::preflight(master_agent)
        .map_err(|e| DispatchError::Operation(e.message()))?;

    // #761: role には語彙が 2 つある。ペインに貼る表示用ラベルと、起動コマンドが注入する
    // `TAKO_ORCHESTRATOR_ROLE`（`master:<profile>`）。以前は表示用を env にも入れていたため、
    // 後任の caller_role が解決できず self / handoff / profiles がすべて default に落ちていた
    let new_role = tako_core::handoff::master_pane_role(profile_name);
    let role_env = tako_core::handoff::master_role_env(profile_name);

    // #761: master の起動コマンドは CLI の `tako master -<profile>` と**同一経路**で作る。
    // worker 用の `resolve_agent_launch`（worker_agents.<agent> を見る）を使っていたため、
    // 後任が worker 用モデルで起動し、master system prompt も付いていなかった
    let prompt_content = profile.build_system_prompt(profile_name);
    let prompt_path = orchestrator::config_dir()
        .ok_or_else(|| op_err("ホームディレクトリが取得できない"))?
        .join(format!("_system_prompt_{profile_name}.md"));
    if let Some(parent) = prompt_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| op_err(format!("system prompt の保存先を作れない: {e}")))?;
    }
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| op_err(format!("system prompt の書き出しに失敗: {e}")))?;
    let tako_bin = resolve_tako_binary();
    // ペインを分割する**前**に組み立てる（失敗したときに空ペインだけ残さない）
    let master_cmd = orchestrator::build_master_cmd(&role_env, &profile, &prompt_path, &tako_bin)
        .map_err(DispatchError::Operation)?;

    // cwd はホームディレクトリ
    let cwd = orchestrator::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    // 新ペインを分割。#917: 退役する master が同じタブに居るなら**その master のペインを
    // 分割する**。旧ペインが閉じられた時点で残った後任が旧ペインの矩形をそのまま継ぐので、
    // 交代の前後でレイアウトが変わらない（「場所が入れ替わる」体験）。周囲の worker /
    // ユーザーペインには一切触らない。旧 master を特定できないときだけ従来の
    // worker 領域レイアウトへ落とす
    let new_pane = tako_core::Pane::new(origin);
    let new_id = new_pane.id();
    match previous_pane {
        Some(prev) => {
            tree_mut(host.workspace_mut(), tab_id)
                .split(prev, tako_core::SplitDirection::Right, new_pane)
                .map_err(op_err)?;
        }
        None => {
            let layout = crate::setup::spawn_layout_config();
            tree_mut(host.workspace_mut(), tab_id)
                .spawn_worker(split_target, new_pane, &layout)
                .map_err(op_err)?;
        }
    }
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(split_target);

    // セッション起動（cwd をホームに、プロファイル env を注入。Issue #500）
    let options = SpawnOptions {
        command: None,
        cwd: Some(cwd.clone()),
        env: profile_env.exports.clone(),
    };
    host.attach_session(new_id, options);

    // 事前信頼。claude は config dir 配下の .claude.json を読むので、アカウント指定で
    // CLAUDE_CONFIG_DIR を注入する場合はその config dir へ書く（#558）
    let _ = orchestrator::agent::ensure_trusted_in(
        master_agent,
        profile_env.claude_config_dir().as_deref(),
        &cwd.to_string_lossy(),
    )
    .unwrap_or_else(|e| {
        eprintln!("warning: handoff 事前信頼失敗（ダイアログ検出で継続）: {e}");
        false
    });

    // コマンド送信（送達確認つき。#640）
    host.queue_command_flow(new_id, master_cmd);

    // handoff プロンプトの構成と送信（#749: 文面は tako-core の純粋関数が正）。
    // #792: 新書式（2 節）なら節ごとの扱い、旧書式なら「番号は実態で確認 + 次回は書き直せ」が
    // 文面に付く。**引き継ぎ内容そのものは書式に関係なく全文が渡る**（後方互換）
    let successor = bundle.as_successor();
    let handoff_prompt =
        tako_core::handoff::successor_prompt(&successor, previous_pane.map(PaneId::as_u64));
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
    let mut warnings: Vec<String> = migration.warnings.clone();
    warnings.extend(bundle.memo_warning.clone());
    Ok(json!({
        "new_master_pane_id": new_id.as_u64(),
        "new_master_tab_id": tab_id.as_u64(),
        "profile": profile_name,
        // #854: プロファイルをどこから決めたか（caller_role / pane_role / default）。
        // pane_role が出たら呼び出し元の env が失われていたということ
        "profile_source": profile_source.as_str(),
        "role": new_role,
        "handoff_file": handoff_path,
        "handoff_prompt_length": handoff_prompt.len(),
        // #915: 渡した管轄プロジェクトと、その決め方
        "projects": bundle.jurisdiction.projects,
        "jurisdiction_source": bundle.jurisdiction.source.as_str(),
        "project_files": bundle.projects.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        "missing_project_files": bundle.missing_projects,
        // #915: 旧形式からの自動移行の実施（可視化。migrated=false なら何もしていない）
        "handoff_migration": migration,
        "warnings": warnings,
        // #792: 渡した引き継ぎの書式。新旧が混ざっていたら "mixed"
        "handoff_format": successor.format(),
        "handoff_sections": successor.section_labels(),
        // #749: 退役するペイン。null なら後任に kill を指示していない
        // （旧 master を特定できなかった = 安全側に倒した）
        "previous_master_pane_id": previous_pane.map(PaneId::as_u64),
        "previous_master_close": if previous_pane.is_some() {
            "後任 master が引き継ぎ確認後に閉じる"
        } else {
            "旧 master ペインを特定できなかったため閉じない"
        },
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
    let profile_env = profile.resolved_env_plan();
    let worker_agent = profile
        .resolve_worker_agent(agent)
        .map_err(DispatchError::InvalidParams)?;
    // #983: CLI が無ければ**ペインを作る前に**落とす（作ってから流すと
    // ペインに `command not found` が出るだけで、tako は成功と報告してしまう）
    let agent_cli_path = orchestrator::agent_cli::preflight(worker_agent)
        .map_err(|e| DispatchError::Operation(e.message()))?;
    let launch = profile.resolve_agent_launch(worker_agent, None, None);
    // Remote Control（#1068）。判定は起動コマンドの組み立てと同じ 1 実装を通す
    let remote_control =
        orchestrator::remote_control_decision(&profile, worker_agent, &profile_env);

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
        env: profile_env.exports.clone(),
    };
    host.attach_session(new_id, options);

    let role_value = "conflict-resolver";
    let agent_cmd = orchestrator::agent::build_worker_cmd(&orchestrator::agent::WorkerLaunch {
        agent: worker_agent,
        role: role_value,
        model: launch.model.as_deref(),
        effort: launch.effort.as_deref(),
        skip_permissions: launch.skip_permissions,
        allow_sandbox_bypass: launch.allow_sandbox_bypass,
        // Remote Control（#1068）: opt-in かつ適格なときだけ付く。
        // 不適格なら flag は None になり、理由は `remote_control` に残る
        remote_control: remote_control.enabled(),
        extra_args: &launch.extra_args,
        env: &profile_env,
    });

    // 事前信頼（未信頼フォルダの確認ダイアログにプロンプトが食われるのを防ぐ。Issue #32）。
    // 書き先は起動する claude の config dir 配下（#558）
    let claude_config_dir = profile_env.claude_config_dir();
    let pre_trusted =
        orchestrator::agent::ensure_trusted_in(worker_agent, claude_config_dir.as_deref(), &cwd)
            .unwrap_or_else(|e| {
                // #983: 分類済みの理由 + 次の一手で残す（無言の劣化を作らない）
                let _ = launch_warning(
                    worker_agent,
                    crate::orchestrator::agent_cli::AgentCliProblem::TrustWriteFailed,
                    &e,
                );
                false
            });
    if launch.skip_permissions && worker_agent == orchestrator::agent::WorkerAgent::Claude {
        let _ = crate::claude_tui::ensure_bypass_accepted_in(claude_config_dir.as_deref()).map_err(
            |e| {
                eprintln!("warning: Bypass 事前承認の書き込みに失敗（ダイアログ検出で継続）: {e}");
            },
        );
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
        // #983: tako がどの実行ファイルを起動したか（無ければ preflight で落ちている）
        "agent_path": agent_cli_path,
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
        // #1068: この解消エージェントの会話が Remote Control へ繋がるか。
        // opt-in していても環境が不適格なら false で、理由は `blocked` に入る
        // （spawn と同じ扱い = 無言で「繋がっているはず」にしない）
        "remote_control": remote_control.enabled(),
        "remote_control_blocked": remote_control.blocked.as_ref().map(|b| json!({
            "kind": b.kind(),
            "detail": b.detail(),
            "reason": b.reason().text(),
            "next_step": b.next_step().text(),
        })),
        "state": conflict_state_json(&repo, &state),
    }))
}

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
    /// この worker だけ利用上限後の自動復帰を明示指定する（#822。
    /// 省略時はプロファイルの `limit_resume` → false）
    limit_resume: Option<bool>,
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
        limit_resume,
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
    let profile_env = profile.resolved_env_plan_with_account(resolved_account.as_ref());

    let worker_agent = profile
        .resolve_worker_agent(agent)
        .map_err(DispatchError::InvalidParams)?;
    // #983: agent CLI の実在検査。**ペイン分割・レジストリ登録より前**に落とすので、
    // 失敗しても空ペインも active エントリも残らない（不正 agent の検証と同じ位置）。
    // 無ければ「理由 + 次の一手」を返す（無言死を作らない）
    let agent_cli_path = orchestrator::agent_cli::preflight(worker_agent)
        .map_err(|e| DispatchError::Operation(e.message()))?;
    // アカウントの default_model / default_effort をフォールバックに使う（#504）
    let effective_model = model.or(resolved_account
        .as_ref()
        .and_then(|a| a.default_model.as_deref()));
    let effective_effort = effort.or(resolved_account
        .as_ref()
        .and_then(|a| a.default_effort.as_deref()));
    let launch = profile.resolve_agent_launch(worker_agent, effective_model, effective_effort);
    // Remote Control（#1068）。opt-in が無ければ何も起きない。
    // opt-in なのに不適格なら、フラグは付けずに理由を spawn 応答の warnings へ載せる
    // （無言で「繋がっているはず」にしない）
    let remote_control =
        orchestrator::remote_control_decision(&profile, worker_agent, &profile_env);
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
        env: profile_env.exports.clone(),
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
        allow_sandbox_bypass: launch.allow_sandbox_bypass,
        // Remote Control（#1068）: opt-in かつ適格なときだけ付く。
        // 不適格なら flag は None になり、理由は `remote_control` に残る
        remote_control: remote_control.enabled(),
        extra_args: &launch.extra_args,
        env: &profile_env,
    });

    // 事前信頼: 未信頼フォルダでエージェント CLI を起動すると信頼ダイアログが出て、
    // 送信したプロンプトがダイアログへの応答として消費される（Issue #32 問題 1）。
    // 起動前に各 CLI の設定ファイル（claude: <config dir>/.claude.json /
    // codex: ~/.codex/config.toml / agy: ~/.gemini/antigravity-cli/settings.json）へ
    // 信頼済みを書き込んでダイアログ自体を出さない。claude の書き先は起動する
    // config dir 配下でなければ効かない（#558。アカウント指定で変わる）。
    // 失敗しても PromptFlow のダイアログ検出 → 承諾がフォールバックするため継続する
    let claude_config_dir = profile_env.claude_config_dir();
    // #983 の変更 3: 事前信頼の書き込み失敗を**黙って握りつぶさない**。
    // 致命ではない（tako がダイアログを検出して承諾する）が、そのぶん最初の指示が
    // 届くまで遅くなる。理由と次の一手を応答の warnings へ載せる
    let mut launch_warnings: Vec<Value> = Vec::new();
    let pre_trusted =
        orchestrator::agent::ensure_trusted_in(worker_agent, claude_config_dir.as_deref(), &cwd)
            .unwrap_or_else(|e| {
                launch_warnings.push(launch_warning(
                    worker_agent,
                    crate::orchestrator::agent_cli::AgentCliProblem::TrustWriteFailed,
                    &e,
                ));
                false
            });

    // Bypass Permissions 事前承認（#407）: skip_permissions=true の claude worker は
    // --dangerously-skip-permissions で起動する。初回は確認ダイアログが出て既定選択
    // 「No, exit」で即終了するため、起動前に config dir 配下の .claude.json へ
    // 承認済みを書き込む。フォールバック: deliver_via_tmux の bypass ダイアログ検出 → 承諾
    if launch.skip_permissions && worker_agent == orchestrator::agent::WorkerAgent::Claude {
        let _ = crate::claude_tui::ensure_bypass_accepted_in(claude_config_dir.as_deref()).map_err(
            |e| {
                eprintln!("warning: Bypass 事前承認の書き込みに失敗（ダイアログ検出で継続）: {e}");
            },
        );
    }

    // attach_session は非同期（pending_attach）なのでセッションはまだ存在しない。
    // かつ、起動した直後の PTY へ書いたバイトは器（psmux）に落とされる（#640 実測:
    // PTY 起動から 0〜500ms の書き込みは全損、1500〜3000ms は途中欠落）。
    // 「シェルの準備待ち → エコー確認 → 分離 Enter → 実行確認」を回す送達確認フローで送る
    host.queue_command_flow(new_id, worker_cmd.clone());

    // プロンプトは claude TUI の起動完了を画面内容で確認してから送達確認つきで送る。
    // ステートマシン駆動: alt_screen 遷移 → 信頼ダイアログ承諾 → ❯ 表示待ち →
    // bracketed paste → 分離 Enter → 入力欄の空検証 + Enter 再送（Issue #32）。
    // マルチラインは bracketed paste でそのまま渡るため改行の平坦化はしない
    host.queue_spawn_prompt_flow(new_id, prompt.to_string());

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
    // 利用上限後の自動復帰（FR-2.27 / #813）の既定を worker ペインへ適用する（#822）。
    // 解決順は spawn 引数 → プロファイル → false。ON のときだけ監査行を残す
    // （既定 OFF の spawn で persist.log を埋めない）
    let limit_resume_applied = orchestrator::resolve_worker_limit_resume(&profile, limit_resume);
    pane_obj.set_limit_autoresume(limit_resume_applied);
    if limit_resume_applied {
        let source = if limit_resume.is_some() {
            "spawn"
        } else {
            "profile"
        };
        crate::diag::persist_log(&format!(
            "[limit-autoresume] pane={} enabled=true 発生源 spawn:{source}",
            new_id.as_u64()
        ));
    }

    // attach は非同期のため backend セッション名をここで事前予約する（Issue #112。
    // 従来の `backend_session(new_id)` は spawn 時点で常に None = 応答の tmux_session が
    // 空で、pane 消失時の tmux フォールバック用の値を master へ渡せていなかった）
    let tmux_session = host
        .reserve_backend_session(new_id)
        .or_else(|| host.backend_session(new_id));

    // セッションカタログへ spawn 記録を残す（Issue #112 A）。session_id は claude 起動後に
    // GUI の定期スキャンが検出して昇格する。失敗してもカタログの都合で spawn は止めない。
    //
    // #728: 器が無い構成（Windows で psmux 未導入 / tmux 不在の macOS）では
    // `tmux_session` が None になる。以前はこのブロックごと飛ばしていたので、
    // spawn 時のメタ（プロンプト・Issue 番号・model）が永久に記録されず、
    // カタログが「発見性」という存在意義を果たせなかった。キーはペイン ID へ倒す
    {
        let issues =
            crate::sessions::extract_issues(&format!("{} {prompt}", label.unwrap_or_default()));
        let record = crate::sessions::PendingSpawn {
            tmux_session: tmux_session.clone(),
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

    // #368: spawn 完了 → claude session スキャンを即時トリガー
    crate::request_claude_scan();

    // #1068: opt-in しているのに Remote Control が有効にならなかったときは理由を残す。
    // ここで黙ると「スマホの一覧に出ない」の原因が辿れなくなる
    if let Some(blocked) = &remote_control.blocked {
        let message = format!(
            "remote_control: true ですが有効にできませんでした（{}）。{} / {}",
            blocked.detail(),
            blocked.reason().text(),
            blocked.next_step().text()
        );
        eprintln!("warning: {message}");
        launch_warnings.push(json!({
            "kind": format!("remote_control_{}", blocked.kind()),
            "agent": worker_agent.as_str(),
            "message": message,
            "detail": blocked.detail(),
            "next_step": blocked.next_step().text(),
        }));
    }

    // Part 4: env のキー一覧（値はマスク。Issue #500）
    let env_keys: Vec<&str> = profile_env.export_keys();
    // CLAUDE_CONFIG_DIR が設定されている場合、config dir パスを応答に含める。
    // inherit のアカウントでは設定しない = null + env_unset に現れる（#512）
    let config_dir_value = profile_env.export_value(orchestrator::CLAUDE_CONFIG_DIR_ENV);

    Ok(json!({
        "pane_id": new_id.as_u64(),
        "spawned_by": target.as_u64(),
        "title": window_title,
        "cwd": cwd,
        "agent": worker_agent.as_str(),
        // #983: tako がどの実行ファイルを起動したか（無ければ preflight で落ちている）
        "agent_path": agent_cli_path,
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
        // #512: 明示 unset する変数（inherit アカウントの CLAUDE_CONFIG_DIR 等）
        "env_unset": profile_env.unsets,
        "config_dir": config_dir_value,
        "account": account_name,
        // #822: この worker に適用された利用上限後の自動復帰（FR-2.27）。
        // true なら `tako limit-resume --pane <id>` で切らない限り自動復帰の対象
        "limit_resume": limit_resume_applied,
        // #1068: この worker の会話が Claude 公式の Remote Control へ繋がるか。
        // opt-in していても環境が不適格ならここは false（理由は warnings に入る）
        "remote_control": remote_control.enabled(),
        // #983: spawn は成立したが完全ではなかったもの（分類 + 理由 + 次の一手）。
        // 空配列 = 何も問題が無かった
        "warnings": launch_warnings,
    }))
}

/// 起動時の非致命な失敗を「分類 + 理由 + 次の一手 + 生の詳細」で返す（#983 の変更 3）。
/// **spawn を止めない失敗でも黙らない**ための 1 箇所
fn launch_warning(
    agent: crate::orchestrator::agent::WorkerAgent,
    problem: crate::orchestrator::agent_cli::AgentCliProblem,
    detail: &str,
) -> Value {
    let err = crate::orchestrator::agent_cli::AgentCliError { agent, problem };
    let message = err.message();
    eprintln!("warning: {message}（{detail}）");
    json!({
        "kind": problem.kind(),
        "agent": agent.as_str(),
        "message": message,
        "detail": detail,
    })
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
    /// 利用上限後の自動復帰の状態（#813。UI スレッドで写し取る）
    limit_resume: Value,
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
    let has_running_children = backend_session
        .as_ref()
        .is_some_and(|bs| crate::agents::has_running_children(bs));
    WorkerStatusCtx {
        pane_id,
        pane_exists: in_tree || host.workspace().is_shelved(target),
        backend_session,
        has_running_children,
        live_tail: lines.map(tail_join),
        full_screen,
        limit_resume: limit_resume_entry(host, target),
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

/// codex の `rate_limits` を応答 JSON へ落とす（#985）。
/// **キー名は codex の rollout と同じ**にして、上流のドキュメントと突き合わせられるようにする
fn rate_limits_json(rl: &crate::codex_session::RateLimits) -> serde_json::Value {
    let window = |w: Option<crate::codex_session::RateWindow>| {
        w.map(|w| {
            json!({
                "used_percent": w.used_percent,
                "window_minutes": w.window_minutes,
                "resets_at": w.resets_at,
            })
        })
    };
    json!({
        "primary": window(rl.primary),
        "secondary": window(rl.secondary),
        "plan_type": rl.plan_type,
        // 上限に当たっている枠（`x-codex-rate-limit-reached-type` 由来。通常は null）
        "reached": rl.reached,
        "limited": rl.limited(),
        // 止まっている枠が空く時刻（unix 秒）。上限でなければ null
        "reset_at": rl.reset_at(),
    })
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
        limit_resume,
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

    // session_id の解決: 明示指定 > pane→session 自動解決 > codex の実況 > フォールバック
    //
    // #984: claude の一次シグナル（`claude agents --json`）が取れないペインでも、
    // codex なら**セッションの実況が構造化ソースになる**（`$CODEX_HOME/sessions/` の
    // rollout JSONL に `task_started` / `task_complete` が逐次書かれる。実測済み）。
    // 画面推定へ落ちるのは「claude でも codex でもない」ときだけになる
    let mut codex_thread: Option<String> = None;
    let (resolved_sid, status_source);
    if let Some(sid) = session_id {
        resolved_sid = Some(sid.to_string());
        status_source = "agents";
    } else if pane_exists {
        // pane→session 自動解決: backend_session から pid 祖先辿り
        if let Some(ref backend) = backend_session {
            if let Some(sid) = crate::agents::resolve_session_id_for_backend(backend) {
                resolved_sid = Some(sid);
                status_source = "agents-auto";
            } else if let Some(tid) = crate::codex_session::resolve_thread_id_for_backend(backend) {
                codex_thread = Some(tid);
                resolved_sid = None;
                status_source = "codex-session";
            } else {
                resolved_sid = None;
                status_source = "screen";
            }
        } else {
            resolved_sid = None;
            status_source = "screen";
        }
    } else {
        resolved_sid = None;
        status_source = "none";
    };

    // #267: agents の生ステータスを dispatch の語彙に正規化する。
    // 正規化しないと watch ループの unknown フォールバック（スクリーン末尾 5 行判定）に
    // 落ち、長時間ツール出力で busy パターンが流れた瞬間に偽 IDLE が出る
    let mut codex_rate_limits: Option<crate::codex_session::RateLimits> = None;
    // #983 の変更 2: 「ターンが走った」= プロンプトが届いた、という**送達の一次シグナル**。
    // 画面の送達確認より強い証拠なので、送達判定へ渡して未達の誤検知を潰す（#1015）
    let mut codex_turn_observed = false;
    let (status, ctx_percent) = if let Some(ref sid) = resolved_sid {
        let agent = orchestrator::query_agent_status(sid);
        (
            normalize_agent_status(&agent.status).to_string(),
            agent.ctx_percent,
        )
    } else if let Some(ref tid) = codex_thread {
        // #984: rollout のターン状態。**まだ 1 ターンも走っていなければ何も言わない**
        // （`status()` が None）。ここで idle と言うとプロンプト投入前を完了と誤認する
        match crate::codex_session::read_turn_state(tid) {
            Some(st) => {
                // #985: 同じ読み取りにレート制限も載っている（追加の I/O ゼロ）
                codex_rate_limits = st.rate_limits.clone();
                codex_turn_observed = st.prompt_arrived();
                match st.status() {
                    Some(s) => (s.to_string(), st.ctx_percent),
                    None => ("unknown".to_string(), st.ctx_percent),
                }
            }
            None => ("unknown".to_string(), None),
        }
    } else if pane_exists {
        ("unknown".to_string(), None)
    } else {
        ("gone".to_string(), None)
    };

    // ペインの最近の出力（pane のライブ画面 → tmux session フォールバック）
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
    let prompt_delivery = registry_worker.as_ref().map(|(_, entry)| {
        if entry.session_id.is_none() {
            if let (Some(sid), Some(ts)) = (resolved_sid.as_deref(), entry.tmux_session.as_deref())
            {
                let _ = orchestrator::registry::record_session_detected(ts, sid);
            }
        }
        let mut effective = entry.clone();
        if effective.session_id.is_none() {
            effective.session_id = resolved_sid.clone();
        }
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap_or(0);
        let spawned_epoch = crate::sessions::parse_iso(&effective.spawned_at).unwrap_or(now_epoch);
        (
            orchestrator::registry::prompt_delivery_assessment_with(
                &effective,
                now_epoch,
                orchestrator::registry::DeliveryEvidence {
                    turn_observed: codex_turn_observed,
                },
            ),
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
        registry_agent: registry_worker.as_ref().map(|(_, e)| e.agent.clone()),
        registry_worker_id: registry_worker.map(|(id, _)| id),
        prompt_delivery,
        registry_session_detected,
        registry_resume_command,
        agent_process_alive,
        limit_resume,
        codex_rate_limits,
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
    /// #983: レジストリ上の agent 系統（claude / codex / agy）。
    /// 起動失敗の分類と、送達の観測手段の解決に使う
    registry_agent: Option<String>,
    /// #390: prompt 送達判定（レジストリ登録済み worker のみ）と spawn からの経過秒
    prompt_delivery: Option<(crate::orchestrator::registry::PromptDelivery, i64)>,
    /// #390: レジストリに session_id が記録済み（= エージェントは一度起動して走った）
    registry_session_detected: bool,
    /// #390: レジストリの session ID から組み立てた復旧コマンド（claude のみ）
    registry_resume_command: Option<String>,
    /// #390: エージェントプロセスの生存シグナル（突然死判定専用。pane 消失中は
    /// tmux フォールバックで再計算済み。busy / stalled 補正には使わない）
    agent_process_alive: bool,
    /// #813: 利用上限後の自動復帰の状態（UI スレッドで写し取った値をそのまま載せる）
    limit_resume: Value,
    /// #985: codex の構造化されたレート制限（rollout の `rate_limits`。他 agent は None）
    codex_rate_limits: Option<crate::codex_session::RateLimits>,
}

/// worker_status の初期状態に補正ロジックを適用し、最終的な JSON 応答を構築する。
/// `finish_worker_status` から分離した内部関数（テスト時に初期状態を直接制御するため）
fn apply_worker_status_corrections(resolved: ResolvedWorkerStatus) -> Result<Value, DispatchError> {
    let ResolvedWorkerStatus {
        mut status,
        status_source,
        codex_rate_limits,
        ctx_percent,
        resolved_sid,
        pane_exists,
        has_children,
        recent_output,
        full_screen,
        tmux_session,
        registry_worker_id,
        registry_agent,
        prompt_delivery,
        registry_session_detected,
        registry_resume_command,
        agent_process_alive,
        limit_resume,
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
    //
    // #984: codex の実況（codex-session）も同じ扱い。rollout がまだ無い＝ターン未実行の
    // ときは構造化ソースとして何も言えないので、根拠を画面へ落とす
    let mut status_source = status_source;
    if status == "unknown"
        && (status_source.starts_with("agents") || status_source == "codex-session")
    {
        status_source = "screen".to_string();
    }

    // agents API（status_source = agents / agents-auto）はセッション状態の
    // 一次情報なので、idle を返したらプロセスツリー heuristic で覆さない。
    // バックグラウンドシェル（tailscaled 等）の常駐子プロセスが IDLE 検知を
    // 永久にブロックする問題を根治する（#289）。
    // 一時的な idle（サブエージェント完了瞬間）は watch の idle_streak（3 回連続）で防ぐ
    //
    // #984: codex の実況も一次情報なので同じ権威を持たせる。**これを入れないと
    // 構造化ソースを足した意味が無い**: エージェント CLI の TUI 自身がペインシェルの
    // 子なので `has_children` は生きている限り必ず true で、idle が必ず busy へ
    // 上書きされてしまう（#571 で claude について踏んだのと同じ形）
    let agents_authoritative = status_source == "agents"
        || status_source == "agents-auto"
        || status_source == "codex-session";
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

    // #577: 画面に permission ダイアログ（ツール実行の承認要求）が**実在すれば**
    // waiting へ格上げする。旧実装は「agents の生 status が waiting」だけを根拠に
    // していたため、**agents がその worker を見られない状況で丸ごと落ちていた**。
    //
    // 実測（2026-07-27 / claude v2.1.x。証拠は #577 の e2e）:
    // - agents 解決に成功していれば生 status は `waiting` を返す（Issue 本文の
    //   「claude は idle / busy しか返さない」は permission 待ちには当てはまらない）
    // - ところが `claude agents --json` に載らない worker（別 config dir の継承・
    //   `CLAUDE_CODE_CHILD_SESSION` つき起動・一覧の取りこぼし = #571 の環境）は
    //   status_source=screen へ落ち、画面推定は `❯ 1. Yes` を入力欄と見なして idle。
    //   結果 permission 待ちが「idle + question」として通知され、
    //   `permission_dialog` は常に null だった（#577 の観測がこれ）
    // - codex / agy には agents 相当の API が無く、常にこの画面推定経路を通る
    //
    // そこで判定を agents の語彙から切り離し、画面の実在検査
    // （`detect_permission_dialog`）を一次の根拠にする。ダイアログは入力欄を
    // 奪っている（= 応答するまで先へ進めない）ので、agents / 画面推定が
    // どちらの状態を出していても停止側が正
    let permission_dialog = recent_output
        .as_deref()
        .and_then(crate::orchestrator::wait::permission_dialog_json);
    // #748: permission 以外の選択肢ダイアログ（usage limit の対処選択・モデル選択・
    // plan 確認・AskUserQuestion・`/mcp` の一覧等）も同じ構造検知から拾う。
    // どの種別でも**入力欄を奪っている = 応答するまで先へ進めない**ので waiting へ格上げする。
    // 旧実装は permission だけを見ており、それ以外は idle（= 完了）として通知されていた
    // （#748 の観測 2。master は WORKER_IDLE を受けて報告を待ち続ける）
    let choice_dialog = recent_output
        .as_deref()
        .and_then(crate::orchestrator::wait::choice_dialog_json);
    // usage limit の対処ダイアログだけは waiting へ格上げしない（#748）。
    // 「解除まで待ってから続行」という復旧は error 側（#157 の WorkerErrorKind +
    // #401 の supervisor）が持っているので、そこを迂回させない。
    // 選択肢の構造は下の `choice_dialog` フィールドに載るので respond もできる
    let limit_dialog = choice_dialog
        .as_ref()
        .and_then(|d| d["kind"].as_str())
        .is_some_and(|k| k == "usage_limit");
    if (permission_dialog.is_some() || choice_dialog.is_some()) && status != "gone" && !limit_dialog
    {
        status = "waiting".to_string();
    }

    // 停止（idle）した worker の画面に既知のエラーパターン（API エラー・usage limit・
    // rate limit ダイアログ）があれば error へ細分類する（#157）。busy 中は判定しない
    // （自動リトライ・ツール実行ログへの誤検知防止。busy が明ければ idle 経由で判定される）
    let mut error_info: Option<Value> = None;

    // #983: **エージェント CLI の起動そのものが失敗している**画面（CLI 不在で
    // `command not found` が出た / 未認証で止まっている）。旧実装ではこの状態が
    // 「idle = 完了」に見えていた = 無言死。既存のエラー検知より先に見る。
    //
    // **送達の証拠がまだ無い worker に限る**のが誤検知しないための鍵:
    // 一度でも仕事が始まった worker の scrollback には、agent 自身が実行した
    // コマンドの `command not found` が普通に流れる
    use crate::orchestrator::registry::PromptDelivery;
    let never_delivered = !registry_session_detected
        && !matches!(
            prompt_delivery.map(|(a, _)| a),
            Some(PromptDelivery::Delivered)
        );
    if never_delivered
        && (status == "idle" || status == "unknown")
        && !crate::orchestrator::agent_cli::legacy_mode()
    {
        if let (Some(agent_name), Some(out)) = (registry_agent.as_deref(), recent_output.as_deref())
        {
            if let Ok(agent) = crate::orchestrator::agent::WorkerAgent::parse(agent_name) {
                if let Some(problem) =
                    crate::orchestrator::agent_cli::detect_launch_failure(agent, out)
                {
                    let kind = crate::orchestrator::wait::WorkerErrorKind::LaunchFailed;
                    let err = crate::orchestrator::agent_cli::AgentCliError { agent, problem };
                    status = "error".to_string();
                    error_info = Some(json!({
                        "kind": kind.as_str(),
                        // 「理由 + 次の一手」をそのまま載せる（watch は detail を出す）
                        "detail": err.message(),
                        "recommended_action": kind.recommended_action(),
                        // 機械が分岐できる細分類（cli_not_found / not_authenticated / …）
                        "launch_problem": problem.kind(),
                    }));
                }
            }
        }
    }

    if status == "idle" && error_info.is_none() {
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
    //
    // #983: `Unverified`（一次シグナルを持たない系統の猶予超過）も**同じ画面で裏取りする**。
    // 動いているものを「届いていないかも」と言わないための、これが代替判定の実体
    let prompt_delivery_final = prompt_delivery.map(|(assessment, since_spawn)| {
        if matches!(
            assessment,
            PromptDelivery::OverdueSuspect | PromptDelivery::Unverified
        ) {
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
    // #983 の受け入れ条件 5: 送達を裏づける一次シグナルが無い系統でも**黙らない**。
    // 未達と断定せず「未確認 + 確かめてから再送」を出す（自動再送は撃たれない）
    if let Some((PromptDelivery::Unverified, since_spawn)) = prompt_delivery_final {
        events.push(
            crate::orchestrator::wait::WorkerEvent {
                kind: crate::orchestrator::wait::WorkerEventKind::PromptDeliveryUnverified {
                    seconds_since_spawn: since_spawn,
                    agent: registry_agent.clone().unwrap_or_default(),
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

    // #572: busy 中に人間が打った指示が claude のキューに未送信のまま残っていないか。
    // 入力欄は空に見えるので、これが無いと master は「何も残っていない」と読み違える
    let queued_messages_pending = recent_output.as_ref().is_some_and(|out| {
        let lines: Vec<String> = out.lines().map(|l| l.to_string()).collect();
        crate::claude_tui::queued_messages_pending(&lines)
    });
    if queued_messages_pending {
        events.push(
            crate::orchestrator::wait::WorkerEvent {
                kind: crate::orchestrator::wait::WorkerEventKind::QueuedMessagesPending,
            }
            .to_json(),
        );
    }

    // #364: 履歴サイズ計測（agent 非依存の busy シグナル布石）
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
        // #985: codex の**構造化された**利用制限（rollout の `rate_limits`）。
        // 画面スクレイピングと違い used_percent は数値、resets_at は epoch 秒なので、
        // AI が「いつ解けるか」を書式に依存せず読める（claude / agy では null）
        "rate_limits": codex_rate_limits.as_ref().map(rate_limits_json),
        "resolved_session_id": resolved_sid,
        "error": error_info,
        "stalled": stalled_info,
        "has_running_children": has_children,
        "collapsed": collapsed,
        "events": events,
        // #572: true = 人間が busy 中に打った指示がキューに未送信で残っている
        "queued_messages_pending": queued_messages_pending,
        "permission_dialog": permission_dialog,
        // #748: 種別つきの選択肢ダイアログ（permission も含む全種別）。
        // `tako orchestrator respond` に渡す番号 / ラベルはここから読む
        "choice_dialog": choice_dialog,
        "history": history_info,
        // #390: worker レジストリ由来の情報（未登録ペインは null）
        "worker_id": registry_worker_id,
        "prompt_delivery": prompt_delivery_final.map(|(d, _)| d.as_str()),
        "resume_command": registry_resume_command,
        // #813: 利用上限後の自動復帰（enabled = ペインのオプトイン / state = 実行状態）
        "limit_resume": limit_resume,
    }))
}

/// 選択肢ダイアログへの構造化応答（#319 permission → #748 で全種別に一般化）。
///
/// `choice` を省略すると**送信せずに構造だけ返す**（下見。#322 の「最簡形」に沿って
/// 新しいツールを増やさず、同じコマンドで一覧と応答の両方を賄う）。
///
/// キー送出は実測に基づく（#748。claude v2.1.220 の permission / AskUserQuestion で観測）:
/// - 番号つきダイアログは**番号キーだけで確定する**（Enter 不要）。旧実装は番号 + Enter を
///   送っていたので、余分な Enter がダイアログ解消後の入力欄へ抜けていた
/// - 番号なしダイアログ（`/mcp` 等）では番号キーは**無反応**。`↑`/`↓` で移動して Enter。
///   移動後は**ラベル一致で着地を検証**してから Enter を送る（見出し行が選択肢に混ざる
///   TUI でも誤選択を confirm しない）
fn dispatch_orchestrator_respond(
    host: &dyn ControlHost,
    pane_id: u64,
    choice: Option<&str>,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    let target = PaneId::from_raw(pane_id);

    // バックエンドセッションの取得
    let backend_session = host.backend_session(target).ok_or_else(|| {
        DispatchError::Operation(format!(
            "ペイン {pane_id} のバックエンドセッションが見つからない"
        ))
    })?;
    respond_to_choice_dialog(&backend_session, pane_id, choice, caller_role)
}

/// 選択肢ダイアログへの応答本体（ホスト非依存）。
///
/// `ControlHost` を必要とするのはバックエンドセッション名の解決だけなので、
/// そこを引数で受け取る形に切り出してある。おかげで GUI の**バックグラウンド
/// スレッド**からも同じ経路（同じ検証・同じ persist.log 監査）で応答できる
/// （#813 の自動復帰。この関数はキー送出のたびに数百 ms スリープするので
/// UI スレッドから呼んではいけない）
pub fn respond_to_choice_dialog(
    backend_session: &str,
    pane_id: u64,
    choice: Option<&str>,
    caller_role: Option<&str>,
) -> Result<Value, DispatchError> {
    let backend_session = backend_session.to_string();
    // 画面からダイアログの存在を検証。
    // GUI 不在でも応答できることが本 API の意義なので、到達手段は backend 側に依存する
    let (session, access) = crate::reach::detached_session(&backend_session).ok_or_else(|| {
        DispatchError::Operation(
            crate::reach::UnreachableReason::NoDetachedAccess {
                session: backend_session.clone(),
                note: crate::reach::no_detached_access_note(),
            }
            .note(),
        )
    })?;
    let capture = || -> Result<Vec<String>, DispatchError> {
        access
            .capture_screen(&session)
            .map_err(|e| DispatchError::Operation(format!("画面の取得に失敗: {e}")))
    };
    let lines = capture()?;
    let dialog = crate::claude_tui::detect_choice_dialog(&lines).ok_or_else(|| {
        DispatchError::Operation(
            "ペイン画面に選択肢ダイアログが見つからない（既に解消済みか、別の画面状態です）"
                .to_string(),
        )
    })?;

    // choice 省略 = 下見（構造だけ返す。送信しない）
    let Some(choice) = choice else {
        let mut result = dialog.to_json();
        result["pane_id"] = json!(pane_id);
        result["responded"] = json!(false);
        result["hint"] = json!(format!(
            "応答するには choice に番号（1-{}）かラベルの一部を渡す",
            dialog.options.len()
        ));
        return Ok(result);
    };

    let index = resolve_choice_index(&dialog, choice)?;
    let chosen = dialog.options[index].clone();

    // --- キー送出（種別ではなく「番号つきか」で分岐する） ---
    let mut keys_sent: Vec<String> = Vec::new();
    let send = |key: &str| -> Result<(), DispatchError> {
        access
            .send_key(&session, key)
            .map_err(|e| DispatchError::Operation(format!("キー {key} の送信に失敗: {e}")))
    };
    if dialog.numbered {
        let number = chosen.number.unwrap_or((index + 1) as u32).to_string();
        send(&number)?;
        keys_sent.push(number);
        std::thread::sleep(std::time::Duration::from_millis(400));
        // 番号キーで確定しない TUI（codex の「Press enter to confirm」等）のための
        // フォールバック。**ダイアログが残っているときだけ** Enter を送る
        // （確定済みの画面へ送ると入力欄の残留テキストを送信してしまう）
        if dialog_still_open(&capture()?, &dialog) {
            send("Enter")?;
            keys_sent.push("Enter".to_string());
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    } else {
        // 番号なし: ハイライトから目標までカーソルを動かし、着地をラベルで検証する
        let mut current = dialog.highlighted.ok_or_else(|| {
            DispatchError::Operation(
                "番号なしダイアログでハイライト位置を特定できない（手動で応答してください）"
                    .to_string(),
            )
        })?;
        for _ in 0..NAV_ATTEMPTS {
            if current == index {
                break;
            }
            let (key, steps) = if index > current {
                ("Down", index - current)
            } else {
                ("Up", current - index)
            };
            for _ in 0..steps {
                send(key)?;
                keys_sent.push(key.to_string());
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            // 移動後の実画面から現在位置を読み直す（キーを飲まれた場合の再試行）
            let after = crate::claude_tui::detect_choice_dialog(&capture()?).ok_or_else(|| {
                DispatchError::Operation(
                    "カーソル移動中にダイアログが消えた（応答は送っていません）".to_string(),
                )
            })?;
            current = after
                .highlighted
                .ok_or_else(|| DispatchError::Operation("ハイライト位置を再取得できない".into()))?;
            if after.options.get(current).map(|o| o.label.as_str()) == Some(chosen.label.as_str()) {
                break;
            }
        }
        // ラベル一致を確認できないまま Enter は押さない（誤選択の確定を構造的に防ぐ）
        let landed = crate::claude_tui::detect_choice_dialog(&capture()?)
            .and_then(|d| d.highlighted.and_then(|i| d.options.get(i).cloned()))
            .is_some_and(|o| o.label == chosen.label);
        if !landed {
            return Err(DispatchError::Operation(format!(
                "選択肢「{}」へカーソルを移動できなかったため応答を中止した（Enter は送っていません）",
                chosen.label
            )));
        }
        send("Enter")?;
        keys_sent.push("Enter".to_string());
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    // 解消の検証（ダイアログが残っていれば responded=true でも resolved=false を返す）
    let after = capture()?;
    let resolved = !dialog_still_open(&after, &dialog);
    // 検知と送出のあいだにダイアログが自然消滅していた場合、送ったキーは入力欄へ
    // 素通りする。入力欄にそれが残っていたら**黙って成功と言わずに**報告する
    // （番号キーだけで Enter を送らない設計なので、送信されることはない）
    let stray_input = keys_sent
        .iter()
        .any(|k| crate::claude_tui::input_line(&after).is_some_and(|c| c.trim() == k));

    // 監査記録（persist.log。ペイン出力自体はキー入力の結果として画面に残る）
    let caller = caller_role.unwrap_or("unknown");
    crate::diag::persist_log(&format!(
        "[dialog-respond] caller={caller} pane={pane_id} kind={} choice={} ({}) keys={} resolved={resolved} stray={stray_input} title={}",
        dialog.kind.as_str(),
        index + 1,
        chosen.label,
        keys_sent.join("+"),
        dialog.title
    ));

    Ok(json!({
        "pane_id": pane_id,
        "responded": true,
        "resolved": resolved,
        "kind": dialog.kind.as_str(),
        "choice": index + 1,
        "choice_number": chosen.number,
        "choice_text": chosen.label,
        "keys_sent": keys_sent,
        "numbered": dialog.numbered,
        // true = ダイアログが応答直前に消えており、送ったキーが入力欄に残っている
        // （選択は成立していない。入力欄を消してから再試行する）
        "stray_input": stray_input,
        // 後方互換（#319 の応答フィールド。permission 以外では本文の説明が入る）
        "command": dialog.title,
    }))
}

/// 番号なしダイアログでカーソル移動を試みる回数（キーを飲まれたときの再試行込み）
const NAV_ATTEMPTS: u32 = 3;

/// `choice` 文字列を選択肢の添字（0-based）へ解決する（#748）。
///
/// 受け付ける形:
/// - 番号（画面に出ている番号を優先。番号なしダイアログでは 1-based の順番）
/// - ラベルの部分一致（大小無視。複数一致は曖昧としてエラー）
/// - `yes` / `allow` / `no` / `deny` のエイリアス（#319 の互換）
fn resolve_choice_index(
    dialog: &crate::claude_tui::ChoiceDialog,
    choice: &str,
) -> Result<usize, DispatchError> {
    let labels = || {
        dialog
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("{}. {}", o.number.unwrap_or((i + 1) as u32), o.label))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let lower = choice.trim().to_lowercase();
    if lower.is_empty() {
        return Err(DispatchError::Operation(format!(
            "choice が空。選択肢: {}",
            labels()
        )));
    }
    // 1. 番号
    if let Ok(n) = lower.parse::<u32>() {
        if let Some(i) = dialog.options.iter().position(|o| o.number == Some(n)) {
            return Ok(i);
        }
        let i = (n as usize)
            .checked_sub(1)
            .filter(|i| *i < dialog.options.len());
        return i.ok_or_else(|| {
            DispatchError::Operation(format!(
                "choice {n} は範囲外（1-{}）。選択肢: {}",
                dialog.options.len(),
                labels()
            ))
        });
    }
    // 2. yes / no エイリアス（#319 の互換。permission ダイアログの実文言に合わせる）
    let alias: Option<Vec<&str>> = match lower.as_str() {
        "yes" | "allow" => Some(vec!["yes", "allow once", "allow"]),
        "no" | "deny" => Some(vec!["no,", "deny", "no"]),
        _ => None,
    };
    if let Some(candidates) = alias {
        for needle in candidates {
            if let Some(i) = dialog
                .options
                .iter()
                .position(|o| o.label.to_lowercase().starts_with(needle))
            {
                return Ok(i);
            }
        }
        return Err(DispatchError::Operation(format!(
            "{choice} に対応する選択肢が見つからない。選択肢: {}",
            labels()
        )));
    }
    // 3. ラベルの部分一致
    let hits: Vec<usize> = dialog
        .options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.label.to_lowercase().contains(&lower))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(DispatchError::Operation(format!(
            "「{choice}」に一致する選択肢が無い。選択肢: {}",
            labels()
        ))),
        _ => Err(DispatchError::Operation(format!(
            "「{choice}」が複数の選択肢に一致する（{}件）。番号で指定する。選択肢: {}",
            hits.len(),
            labels()
        ))),
    }
}

/// 応答後もダイアログが残っているか（同じ選択肢構成のダイアログが見えているか）。
/// 別のダイアログへ遷移した場合（承認 → 次の承認）は「解消済み」と扱う
fn dialog_still_open(lines: &[String], before: &crate::claude_tui::ChoiceDialog) -> bool {
    crate::claude_tui::detect_choice_dialog(lines)
        .is_some_and(|now| now.labels() == before.labels())
}

/// #748: 選択肢ダイアログ表示中の `Send` を断る。
///
/// 返り値が `Some` なら送信せずそのエラーを返す。生のエスケープシーケンス
/// （矢印キー等の低レベルなキー送信）は意図的な TUI 操作として通す
fn dialog_blocks_send(
    host: &dyn ControlHost,
    pane: Option<u64>,
    tmux_session: Option<&str>,
    text: &str,
) -> Option<DispatchError> {
    if text.contains('\u{1b}') {
        return None;
    }
    let (pane_id, lines) = send_target_screen(host, pane, tmux_session)?;
    let dialog = crate::claude_tui::detect_choice_dialog(&lines)?;
    dialog_send_refusal(&dialog, pane_id).map(DispatchError::Operation)
}

/// 送信拒否の文面（純関数。`None` = 送信して良い）。
/// trust / bypass は tako 自身が承諾する（送達フローが承諾 → 貼り付けまで面倒を見る）ので通す
fn dialog_send_refusal(
    dialog: &crate::claude_tui::ChoiceDialog,
    pane_id: Option<u64>,
) -> Option<String> {
    if dialog.kind.auto_accepted() {
        return None;
    }
    let options = dialog
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| format!("{}. {}", o.number.unwrap_or((i + 1) as u32), o.label))
        .collect::<Vec<_>>()
        .join(" / ");
    let pane_arg = pane_id
        .map(|p| format!("--pane {p}"))
        .unwrap_or_else(|| "--pane <N>".to_string());
    Some(format!(
        "選択肢ダイアログ（{}）が表示中のため送信を中止した。\
         入力欄が奪われているので、テキストや Enter はダイアログのキー操作として食われる\
         （数字なら選択が確定してしまう）。選択肢: {options}。\
         応答は `tako orchestrator respond {pane_arg} --choice <番号|ラベル>`\
         （choice を省略すると構造だけ確認できる）",
        dialog.kind.as_str()
    ))
}

/// `Send` の対象ペインの画面を採る（in-process セッション優先、無ければ detached 経由）
fn send_target_screen(
    host: &dyn ControlHost,
    pane: Option<u64>,
    tmux_session: Option<&str>,
) -> Option<(Option<u64>, Vec<String>)> {
    if let Ok((_, target)) = resolve_pane(host.workspace(), pane) {
        if let Some(session) = host.session(target) {
            return Some((Some(target.as_u64()), session.visible_lines()));
        }
    }
    let ts = tmux_session?;
    let (session, capture) = crate::reach::detached_capture(ts)?;
    capture
        .capture_screen(&session)
        .ok()
        .map(|lines| (pane, lines))
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

/// `Request::SetupMcp` の実処理。**対象エージェントを 1 つに絞らない**のが既定（#979）。
///
/// `agent` を省略すると claude + この環境に導入済みの codex / agy へまとめて登録する
/// （未導入は理由つきの skip）。明示指定したエージェントが未導入・非対応スコープの
/// ときだけ分類済みエラーで止める（#979 スコープ 3・受け入れ条件 4）。
///
/// 応答は `agents` 配列が正で、claude ぶんは**従来のキーを平置きしたまま**残す
/// （既存の CLI 表示・MCP 呼び出し側を壊さないため）。
pub fn setup_mcp_agents(
    agent: Option<&str>,
    scope: &McpScope,
    scope_label: &str,
) -> Result<Value, DispatchError> {
    use crate::agent_mcp;
    use crate::orchestrator::agent::WorkerAgent;

    let explicit = match agent {
        Some(name) => Some(WorkerAgent::parse(name).map_err(DispatchError::Operation)?),
        None => None,
    };
    let targets: Vec<WorkerAgent> = match explicit {
        Some(a) => vec![a],
        // 未導入の CLI も列挙して「なぜ登録されていないか」を応答に残す
        // （黙って消すと AI / 利用者が原因を追えない。#979 スコープ 3）
        None => WorkerAgent::ALL.to_vec(),
    };

    let tako_bin = resolve_tako_binary();
    let project_scope = matches!(scope, McpScope::Project(_));
    let mut entries: Vec<Value> = Vec::new();
    let mut claude_flat: Option<Value> = None;

    for target in targets {
        if target == WorkerAgent::Claude {
            let result = setup_mcp(&tako_bin, scope)?;
            let mut entry = json!({
                "agent": "claude",
                "configured": result.configured,
                "already_existed": result.already_existed,
                "target_path": result.target_path.display().to_string(),
                "command": tako_bin,
            });
            if result.repaired {
                entry["repaired"] = json!(true);
                if let Some(old) = &result.old_command {
                    entry["old_command"] = json!(old);
                }
            }
            if result.legacy_cleaned {
                entry["legacy_cleaned"] = json!(true);
            }
            claude_flat = Some(entry.clone());
            entries.push(entry);
            continue;
        }

        // codex / agy は各 CLI の `mcp add` が user スコープしか持たない（実測）。
        // project を頼まれたら黙って global へ倒さず、理由を出す
        if project_scope {
            let err = agent_mcp::AgentMcpError::ScopeUnsupported {
                agent: target,
                scope: "project",
            };
            if explicit.is_some() {
                return Err(DispatchError::Operation(err.to_string()));
            }
            entries.push(json!({
                "agent": target.as_str(),
                "skipped": true,
                "error_kind": err.kind(),
                "message": err.to_string(),
            }));
            continue;
        }

        match agent_mcp::register(target, &tako_bin) {
            Ok(result) => {
                let mut entry = json!({
                    "agent": result.agent.as_str(),
                    "configured": result.configured,
                    "already_existed": result.already_existed,
                    "command": result.command,
                });
                if let Some(path) = &result.target_path {
                    entry["target_path"] = json!(path.display().to_string());
                }
                if result.repaired {
                    entry["repaired"] = json!(true);
                    if let Some(old) = &result.old_command {
                        entry["old_command"] = json!(old);
                    }
                }
                entries.push(entry);
            }
            Err(err) => {
                if explicit.is_some() {
                    return Err(DispatchError::Operation(err.to_string()));
                }
                entries.push(json!({
                    "agent": target.as_str(),
                    "skipped": true,
                    "error_kind": err.kind(),
                    "message": err.to_string(),
                }));
            }
        }
    }

    let mut resp = claude_flat.unwrap_or_else(|| json!({}));
    if let Some(obj) = resp.as_object_mut() {
        // 平置きは claude の結果なので、claude が対象外のときは残さない
        obj.remove("agent");
    }
    resp["agents"] = json!(entries);
    resp["scope"] = json!(scope_label);
    Ok(resp)
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

/// claude CLI のパスを検出（境界 B16。`which` を起こさない = #898）
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
    // #586: GUI プロセス（dispatch）から到達するのでコンソールウィンドウを出させない
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
        // **既定値へ落として書き戻してはいけない**（#916）: ここは claude 自身の
        // 設定ファイル（`~/.claude.json` / `.mcp.json`）で、空 map から書き直すと
        // 利用者の MCP 登録・信頼済みフォルダ・履歴がまとめて消える。
        // 読めないなら手を出さず理由を返す（旧実装は unwrap_or_default で全消しだった）。
        // 中身が空（`touch` しただけ）のときだけは失うものが無いので新規扱いにする
        if content.trim().is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_str(&content).map_err(|e| {
                DispatchError::Operation(format!(
                    "{} を JSON として解釈できないので書き換えを中止した（{e}）。\
                     内容を直すか退避してからやり直してください",
                    path.display()
                ))
            })?
        }
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

/// tako CLI の実行ファイル名。Windows は `tako.exe`（`EXE_SUFFIX` は空文字なので
/// unix では従来どおり `tako`）。**`cfg` を書かずに済ませるための std 定数**
fn tako_cli_file_name() -> String {
    format!("tako{}", std::env::consts::EXE_SUFFIX)
}

/// tako CLI バイナリのパスを解決する。
/// ① /Applications/tako.app（安定パス）
/// ② 境界 B16 のコマンド解決（`which tako` 相当）
/// ③ 実行中バイナリの隣（.app バンドル / zip 展開想定）
/// ④ フォールバック "tako"
pub fn resolve_tako_binary() -> String {
    resolve_tako_binary_with(
        &|p| std::path::Path::new(p).is_file(),
        &|| which("tako"),
        std::env::current_exe().ok().as_deref(),
        &tako_cli_file_name(),
    )
}

/// [`resolve_tako_binary`] の判定順（純粋関数。**macOS 上から Windows の形も検査できる**）。
///
/// ③ を `cfg` ではなく `file_name` の引数で表すのが要点。旧実装は `dir.join("tako")`
/// 決め打ちで、Windows の隣は `tako.exe` なので**常に空振り**していた（#898）
fn resolve_tako_binary_with(
    is_file: &dyn Fn(&str) -> bool,
    resolve_in_path: &dyn Fn() -> Option<String>,
    current_exe: Option<&std::path::Path>,
    file_name: &str,
) -> String {
    if is_file(STABLE_APP_BINARY) {
        return STABLE_APP_BINARY.to_string();
    }
    if let Some(path) = resolve_in_path() {
        return path;
    }
    if let Some(dir) = current_exe.and_then(std::path::Path::parent) {
        let sibling = dir.join(file_name).display().to_string();
        if is_file(&sibling) {
            return sibling;
        }
    }
    "tako".to_string()
}

/// dispatch / MCP の回答 JSON を CLI の非対話 stdin 経路へ渡す。
/// 回答本文を argv に含めず、プロセス一覧や診断情報へ露出させない。
fn run_setup_cli(tako_bin: &str, answers_json: &str) -> Result<Value, DispatchError> {
    use std::io::Write as _;

    // #586: GUI プロセス（dispatch）から到達するのでコンソールウィンドウを出させない
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

    // tako CLI が PATH に通っているか。境界 B16 経由なので、見ているのは
    // **ログインシェル（= 外部ターミナル）から引けるか**（Windows は PATH + `PATHEXT` +
    // ユーザーが入れがちな場所）。下の案内が「外部ターミナルでも使いたい場合は」と
    // 言っているのと同じ地平で測る。#898 より前は `which` をプロセス PATH で叩いていたので
    // Dock 起動では常に「無い」に見え、Windows では `which` 自体が無く必ず失敗していた。
    // 「tako 内のシェルから打てるか」は別物（そちらは `injected_cli_dir` が答える。#601）
    let cli_path = which("tako");
    let cli_in_path = cli_path.is_some();
    // #601: tako が開くシェルへ自動注入している CLI ディレクトリ（解決できなければ null）
    let injected_cli_dir = tako_core::shell_integration::cli_dir();
    if !cli_in_path {
        // 注入が効いていれば tako の中では打てる = 致命ではない。外部ターミナルでも
        // 使いたい人向けの案内に落とす（level を下げても対処法は示し続ける）
        let (level, message) = match injected_cli_dir.as_deref() {
            Some(dir) => (
                "info",
                format!(
                    "tako CLI は PATH に無いが、tako が開くシェルには {} を自動で追加するので \
                     tako の中では `tako` が使える（#601）。外部ターミナルでも使いたい場合は\
                     このディレクトリを PATH に追加すること",
                    dir.display()
                ),
            ),
            None => (
                "error",
                "tako CLI が PATH に見つからない。.app バンドル内の CLI を PATH に追加するか、\
                 scripts/build-app.sh --install でインストールすること"
                    .to_string(),
            ),
        };
        issues.push(json!({
            "level": level,
            "check": "cli_in_path",
            "message": message,
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

    // プロセスの DPI 認識レベル（#1063）。Windows はマニフェストで PerMonitorV2 を
    // 宣言している前提で gpui のレイアウトが組まれているので、そこから落ちたら
    // 「描画倍率とレイアウト寸法が食い違う」= 黙って縮退させない
    let dpi_awareness = tako_core::platform::dpi::process_awareness();
    if let Some(note) = tako_core::platform::dpi::degraded_note_here() {
        issues.push(json!({
            "level": "error",
            "check": "dpi_awareness",
            "message": note.text(),
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
        // tako 内のシェルへ自動で PATH 追加している CLI ディレクトリ（#601）
        "injected_cli_dir": injected_cli_dir.map(|d| d.display().to_string()),
        "version_match": version_match,
        "tmux_available": tmux_available,
        "persist_enabled": persist_enabled,
        "persist_available": persist_available,
        // #1063: 実測値（unaware / system / per_monitor / per_monitor_v2 /
        // not_applicable = macOS / unknown）。「1.22 倍あふれている」の類の報告は
        // まずここを読む。macOS は常に not_applicable
        "dpi_awareness": dpi_awareness.as_str(),
        "dpi_awareness_expected": tako_core::platform::dpi::expected_here().as_str(),
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

/// コマンド名から実行ファイルを解決する（境界 B16 = [`tako_core::platform::exe::find`]）。
///
/// **`which` コマンドを起こしてはいけない**（#898）。`which` は Windows に存在せず、
/// 旧実装は Windows で例外なく `None` を返していた（tako.exe が PATH 上に居ても
/// tako 自身には「無い」ように見える状態）。境界は unix ではログインシェル経由の
/// `command -v`（`.app` を Dock から起動したときの痩せた PATH でも解決できる）、
/// Windows では PATH + `PATHEXT` + ユーザーが入れがちな場所の走査（サブプロセスなし）。
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

/// 1 ペインぶんの自動復帰の状態（#813）。
///
/// `enabled` はペイン属性（layout.json 永続化）、`state` は GUI が持つ実行状態。
/// GUI 以外のホスト（テスト・CLI 単体）では `state` は null になる
fn limit_resume_entry(host: &dyn ControlHost, target: PaneId) -> Value {
    let enabled = host
        .workspace()
        .tabs()
        .iter()
        .flat_map(|t| t.tree().panes())
        .find(|p| p.id() == target)
        .map(|p| p.limit_autoresume())
        .unwrap_or(false);
    json!({
        "pane": target.as_u64(),
        "enabled": enabled,
        "state": host.limit_resume_state(target),
    })
}

/// 全ペインの自動復帰状態（`all` 指定時）
fn limit_resume_panes(host: &dyn ControlHost) -> Vec<Value> {
    let targets: Vec<PaneId> = host
        .workspace()
        .tabs()
        .iter()
        .flat_map(|t| t.tree().panes().into_iter().map(|p| p.id()))
        .collect();
    targets
        .into_iter()
        .map(|id| limit_resume_entry(host, id))
        .collect()
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

/// 呼び出し元ペインが分からないときは「いま見えているタブのフォーカス中ペイン」へ倒す（#1006）。
///
/// SSH の開き先（既定 = 現在タブへ新ペイン）は、GUI のファイルメニューや tako の外から
/// 叩いた CLI のように**呼び出し元ペインが無い**経路でも成立しなければならない。
/// ただし**存在しないペインを渡された場合は倒さない**（黙って別のペインを対象にすると
/// 「言ったつもりの場所と違う場所が SSH になる」事故になる）
fn resolve_pane_or_active(
    ws: &Workspace,
    pane: Option<u64>,
) -> Result<(TabId, PaneId), DispatchError> {
    match resolve_pane(ws, pane) {
        Err(DispatchError::NoTargetPane) => {
            let tab = ws.active_tab_id();
            let focused = ws
                .get_tab(tab)
                .ok_or(DispatchError::NoTargetPane)?
                .tree()
                .focused();
            Ok((tab, focused))
        }
        other => other,
    }
}

/// 呼び出し元ペインの解決に使った手段（Issue #567。応答の `method` フィールド）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneResolveMethod {
    /// caller_pid の祖先辿りで実ペインを特定した（環境変数より確か）
    Pid,
    /// 渡された pane ID がそのまま現世代に存在した
    Pane,
    /// stale pane map（#210）で旧 ID → 新 ID に読み替えた
    Stale,
}

impl PaneResolveMethod {
    fn as_str(self) -> &'static str {
        match self {
            PaneResolveMethod::Pid => "pid",
            PaneResolveMethod::Pane => "pane",
            PaneResolveMethod::Stale => "stale",
        }
    }
}

/// Issue #567: 呼び出し元ペインの寛容な解決。`resolve_pane` と違い**エラーにしない**。
///
/// 解決順は #288 の `resolve_caller_pane` と揃える（pid 祖先辿り → pane そのまま →
/// stale pane map）。pid を先に見るのは、環境変数はシェルの再利用で古くなるのに対し
/// プロセスの祖先関係は「今どのペインで動いているか」の実態だから。
/// role 検索へのフォールバックは**しない**: `tako master` の起動先が無関係な master
/// ペインになると実行中のエージェントを潰すため、呼び出し元が新規タブへ逃がす方が安全。
fn resolve_pane_lenient(
    host: &dyn ControlHost,
    pane: Option<u64>,
    caller_pid: Option<u32>,
    pid_resolver: impl Fn(u32, &[(u64, String)]) -> Option<u64>,
) -> Option<(PaneResolveMethod, TabId, PaneId)> {
    if let Some(pid) = caller_pid {
        let pane_backends = collect_pane_backends(host);
        if let Some(raw) = pid_resolver(pid, &pane_backends) {
            if let Ok((tab, target)) = resolve_pane(host.workspace(), Some(raw)) {
                return Some((PaneResolveMethod::Pid, tab, target));
            }
        }
    }
    if let Ok((tab, target)) = resolve_pane(host.workspace(), pane) {
        return Some((PaneResolveMethod::Pane, tab, target));
    }
    if let Some((tab, target)) = pane
        .map(PaneId::from_raw)
        .and_then(|stale| host.resolve_stale_pane(stale))
        .and_then(|new_id| resolve_pane(host.workspace(), Some(new_id.as_u64())).ok())
    {
        return Some((PaneResolveMethod::Stale, tab, target));
    }
    None
}

/// `Request::ResolvePane` の応答（Issue #567）
fn resolve_pane_lenient_json(
    host: &dyn ControlHost,
    pane: Option<u64>,
    caller_pid: Option<u32>,
) -> Value {
    let resolved = resolve_pane_lenient(host, pane, caller_pid, crate::agents::resolve_pane_by_pid);
    // 渡された ID が現世代のものと食い違っていたか（呼び出し元の案内文用）
    let stale = match (pane, resolved) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(requested), Some((_, _, p))) => p.as_u64() != requested,
    };
    json!({
        "requested": pane,
        "pane": resolved.map(|(_, _, p)| p.as_u64()),
        "tab": resolved.map(|(_, t, _)| t.as_u64()),
        "method": resolved.map(|(m, _, _)| m.as_str()),
        "stale": stale,
    })
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

/// dispatch の呼び出し経路をペインログのクローズ発生源へ写す（Issue #566）。
/// `PaneOrigin::User` は GUI が dispatch を直接呼ぶ経路（Web ビュー close 等）で、
/// キーバインドや × ボタンとは区別できないため一般の dispatch として記録する
fn close_origin_of(origin: PaneOrigin) -> tako_core::pane_log::CloseOrigin {
    use tako_core::pane_log::CloseOrigin;
    match origin {
        PaneOrigin::Cli => CloseOrigin::Cli,
        PaneOrigin::Mcp => CloseOrigin::Mcp,
        PaneOrigin::User | PaneOrigin::Suggestion => CloseOrigin::Dispatch,
    }
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

/// ペインをそのまま SSH 化できるか（#1006 の `can_ssh_pane` を `list` に載せる。#1080）。
///
/// `{ "ok": true }` か `{ "ok": false, "reason": <slug>, "note": <日本語の理由 + 次の一手> }`。
/// リモート UI（#1080）と AI は**この 1 箇所の答え**を読むので、
/// 「メニューに出たのに実行すると断られる」食い違いが構造的に起きない
fn can_ssh_json(host: &dyn ControlHost, pane: &Pane) -> Value {
    let pane_id = pane.id();
    let backend = host.backend_session(pane_id).is_some();
    let session = host.session(pane_id);
    // 引数を**その場で**組むのは番犬（remote_open_watchdog）に見せるため:
    // 変数へ退避してから渡すと `is_alt_screen()` が呼び出し式から消え、
    // 「器つきペインの外側 alt screen を渡していないか」の検査が素通りする
    // （このヘルパを足したときに実際に素通りするのを A/B で確認した）
    match tako_core::remote_open::can_ssh_pane(
        session.is_some(),
        // 器（tmux）つきペインの**外側** alt screen は常に true（tmux クライアント
        // 自身が alt screen へ入る）ので中身を見る（#694 / #1006）
        !backend && session.is_some_and(|s| s.is_alt_screen()),
        session.map(|s| s.command_state()).unwrap_or_default(),
        pane.role(),
    ) {
        Ok(()) => json!({ "ok": true }),
        Err(block) => json!({
            "ok": false,
            "reason": block.as_str(),
            "note": block.message(pane_id.as_u64()),
        }),
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
                                // #966 のリモート由来（SFTP で落とした写しを出している）なら
                                // その位置と書き戻し状態。ローカルのファイルでは null。
                                // **`path` は写しのローカルパスなので、これが無いと
                                // 「このプレビューがどのリモートファイルか」を外から言えない**
                                // （#1085: 切断中の保存を退避へ回すのにこれを使う）
                                "remote": host.preview_remote_state(p.id()),
                            })
                        }),
                        // 利用上限後の自動復帰のオプトイン（#813。既定 false）
                        "limit_autoresume": p.limit_autoresume(),
                        // SSH の接続待ち / 失敗（#1010。null = どちらでもない）
                        "ssh_connect": host.ssh_connect_state(p.id()),
                        // このペインをそのまま SSH にできるか（#1006 の判定。#1080）。
                        // **判定材料を集めるのはここ 1 箇所**にする: 器つきペインの
                        // 外側 alt screen を渡してはいけない（#694 / #1006 の罠）といった
                        // 事情を知っているのは実セッションを持つこの層だけで、
                        // 応答を読むだけのリモート daemon（#1080）が再現すると必ずずれる
                        "can_ssh": can_ssh_json(host, p),
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
                "limit_autoresume": bp.pane().limit_autoresume(),
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

/// ペイン表示の理由 + 材料の JSON（#1058）。**理由と次の一手を対で出す**規約に合わせ、
/// 日英どちらも載せる（`note` は現在の表示言語で解決したもの）。
///
/// `reason` が null = スターター / チャット / 準備中が出ている = 説明する必要が無い状態
fn pane_display_reason_json(status: &tako_core::ui_mode::PaneDisplayStatus) -> Value {
    let m = status.materials;
    let mut out = json!({
        "display": status.display.as_str(),
        "reason": status.reason.map(|r| r.as_str()),
        "materials": {
            "command_state": command_state_str(m.command_state),
            "has_role": m.has_role,
            "busy_children": m.busy_children,
            "released": m.released,
            "alt_screen": m.alt_screen,
            "claude_chat": m.claude_chat,
            "settling": m.settling,
        },
    });
    if let Some(reason) = status.reason {
        let note = reason.note();
        out["note"] = json!(note.text());
        out["note_ja"] = json!(note.ja());
        out["note_en"] = json!(note.en());
        if let Some(step) = reason.next_step() {
            out["next_step"] = json!(step.text());
            out["next_step_ja"] = json!(step.ja());
            out["next_step_en"] = json!(step.en());
        }
    }
    out
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
    // 旧形式のプロファイル・設定を実行時に検知して直す（#916 の二段構え 2 段目）。
    // 1 プロセス 1 回しか実際には走らないので、この頻繁な経路から呼んでも軽い
    let _ = crate::migrations::ensure_migrated();
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
    limit: Option<usize>,
) -> Result<Value, DispatchError> {
    use std::path::PathBuf;

    // #1079: 全タブのツリールート（各ペインの cwd + ピン留めフォルダ）。
    // リモートのファイル API はこれを**認可の正**として使うので、サイドバーと同じ
    // 1 実装（`tree_roots_of_tab`）を通す = 画面に出ていないフォルダが読めることは
    // 構造的に起きない。タブ横断なので `resolve_tab` より前で処理する
    if action == "roots" {
        let tab_ids: Vec<TabId> = host.workspace().tabs().iter().map(|t| t.id()).collect();
        let mut tabs: Vec<Value> = Vec::new();
        for tid in tab_ids {
            if let Some(tab_mut) = host.workspace_mut().get_tab_mut(tid) {
                tab_mut.prune_dead_folders();
            }
            let title = host
                .workspace()
                .get_tab(tid)
                .map(|t| t.title().to_string())
                .unwrap_or_default();
            let roots: Vec<String> = tree_roots_of_tab(host, tid)
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            tabs.push(json!({
                "tab": tid.as_u64(),
                "title": title,
                "roots": roots,
            }));
        }
        return Ok(json!({ "tabs": tabs }));
    }

    let tab_id = resolve_tab(host.workspace(), tab, pane)?;

    match action {
        // #1009: ツリーに出ている git ステータスをそのまま返す。
        // ルートの解決も分類も UI と同じ 1 実装（`tako_core::sidebar::workspace_roots` /
        // `tako_core::git_tree`）を通るので、画面と応答がずれない
        "git-status" => dispatch_tree_git_status(host, tab_id, path, limit),
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
            "action は add / remove / list / roots / git-status のいずれか（受け取った値: {action}）"
        ))),
    }
}

/// ファイルツリーの git ステータス（#1009）。
///
/// 走査の起点は**そのタブのワークスペースフォルダ**（各ペインの cwd + 明示追加フォルダ）で、
/// サイドバーのルート行と同じ並び。`path` を渡すとそのフォルダ 1 件へ絞る。
/// git 管理外のフォルダは黙って対象外（誤検知しない）
fn dispatch_tree_git_status(
    host: &mut dyn ControlHost,
    tab_id: TabId,
    path: Option<String>,
    limit: Option<usize>,
) -> Result<Value, DispatchError> {
    // 通常は `prepare_offload` が background で走らせる。ここへ来るのは
    // offload を使わない呼び出し元（セルフテスト等）だけ
    let roots = tree_git_status_roots(host, tab_id, path)?;
    Ok(tree_git_status_payload(tab_id.as_u64(), &roots, limit))
}

/// 走査の起点（UI スレッド必須。workspace とペインの cwd を読む）
fn tree_git_status_roots(
    host: &dyn ControlHost,
    tab_id: TabId,
    path: Option<String>,
) -> Result<Vec<PathBuf>, DispatchError> {
    match path {
        Some(p) => {
            let dir = PathBuf::from(&p);
            if !dir.is_dir() {
                return Err(DispatchError::InvalidParams(format!(
                    "ディレクトリが存在しない: {p}"
                )));
            }
            Ok(vec![dir])
        }
        None => Ok(tree_roots_of_tab(host, tab_id)),
    }
}

/// `git` を実行して応答を組む（**UI スレッドで呼ばないこと**）
fn tree_git_status_payload(tab: u64, roots: &[PathBuf], limit: Option<usize>) -> Value {
    /// 応答に載せるエントリ数の既定上限（巨大な差分でも応答が壊れない大きさ）
    const DEFAULT_LIMIT: usize = 500;

    let map = tako_core::git_tree::scan(roots);
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let all = map.sorted_entries();
    let total = all.len();
    let truncated = total > limit;
    let entries: Vec<Value> = all
        .into_iter()
        .take(limit)
        .map(|(path, status)| {
            json!({
                "path": path.display().to_string(),
                "state": status.state.code(),
                "badge": status.badge(),
                "staged": status.staged.map(|c| c.to_string()),
                "unstaged": status.unstaged.map(|c| c.to_string()),
                // 配下からの伝播 = ディレクトリ行（そのフォルダ自身の変更ではない）
                "propagated": status.from_children,
                "changed": status.changed,
            })
        })
        .collect();
    let repos: Vec<Value> = map
        .repos()
        .iter()
        .map(|r| {
            json!({
                "root": r.root.display().to_string(),
                "branch": r.branch,
                "changed": r.changed,
                "truncated": r.truncated,
            })
        })
        .collect();
    json!({
        "tab": tab,
        "roots": roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "repos": repos,
        "entries": entries,
        "total": total,
        "truncated": truncated,
    })
}

/// そのタブのファイルツリーのルート行（#1009）。UI の `sync_filetree_roots` と
/// **同じ関数**（`tako_core::sidebar::workspace_roots`）を通す
fn tree_roots_of_tab(host: &dyn ControlHost, tab_id: TabId) -> Vec<PathBuf> {
    let mut pane_cwds: Vec<PathBuf> = Vec::new();
    let mut pinned: Vec<PathBuf> = Vec::new();
    if let Some(tab) = host.workspace().get_tab(tab_id) {
        for pane in tab.tree().panes() {
            if let Some(cwd) = host.session(pane.id()).and_then(|s| s.cwd()) {
                pane_cwds.push(cwd.to_path_buf());
            }
        }
        pinned = tab.pinned_folders().to_vec();
    }
    for bp in host.workspace().shelved_panes() {
        if bp.origin_tab() != tab_id {
            continue;
        }
        if let Some(cwd) = host.session(bp.id()).and_then(|s| s.cwd()) {
            pane_cwds.push(cwd.to_path_buf());
        }
    }
    tako_core::sidebar::workspace_roots(pane_cwds, pinned, tako_core::paths::home_dir())
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
            // #822: resume はプロファイルの limit_resume をそのまま引き継ぐ
            limit_resume: None,
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
            Ok(json!({
                "mode": sv_mode.as_str(),
                "auto_resume_dead": auto_dead,
                "max_retries": retries,
                "audit_log": log,
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
// SessionRestart — 会話を引き継いだままエージェントペインを建て直す（Issue #1067）
// ---------------------------------------------------------------------------
//
// #498 の張り直し（`tako stale-binary restart`）は同じことをしようとしていたが、
// 実装が 3 点で成立していなかった:
//
//   1. Ctrl+C を 1 回だけ送っていた（claude の対話終了は 2 回。1 回では入力行を捨てるだけ）
//   2. resume の行を `queue_write_on_alt_screen` で書いていた。器（tmux）つきペインの
//      **外側は常に代替画面**なので、条件が即座に成立して**動いている claude の入力欄へ**
//      流れ込む（#694 / #1006 で踏んだのと同じ罠）
//   3. resume コマンドが素の `claude --resume <id>` で、アカウント（`CLAUDE_CONFIG_DIR`）・
//      role・モデル・effort が全部落ちていた
//
// ここでは「終了させたことを確かめてから、`sessions::resume_command` が組んだ行を
// #640 の送達確認つき経路で送る」形にし、#498 のボタンもこの 1 実装へ寄せた。

/// ペインの状態を集めて [`session_restart::RestartFacts`] を作る（#1067）。
///
/// 併せて、実行に要る材料（会話 ID・resume コマンド・終了させる pid）も返す。
/// **同じ材料で下見と実行の両方を賄う**ので、下見で見えた可否と実行結果がずれない
struct SessionRestartPlan {
    facts: tako_core::session_restart::RestartFacts,
    agent: crate::orchestrator::agent::WorkerAgent,
    role: Option<String>,
    /// claude の会話 ID（解決できたときだけ）
    session_id: Option<String>,
    /// 会話 ID の出どころ（`agents-live` / `catalog` / null）
    session_source: Option<&'static str>,
    /// `--resume` の起動コマンド（組めたときだけ）
    resume_command: Option<String>,
    /// 会話 ID を解決できたのに resume コマンドを組めなかった理由
    resume_error: Option<String>,
    /// 終了させるエージェント CLI のプロセス（見つからなければ None）
    agent_pid: Option<u32>,
    /// 引き継ぎ運用メモのパス（handoff モードの案内に使う）
    handoff_path: Option<String>,
}

/// メニューの出し分けに使う**軽い**材料だけを集める（#1067）。
///
/// **重い解決（`claude agents --json` の起動・`ps` / 器の採取）を含めない**のが要点。
/// これは右クリックのたびに呼ばれるので、そこに Node 起動を入れると #772 と同じ
/// 「メインスレッド専有」を作る。会話 ID の解決は実行時（[`build_session_restart_plan`]）
/// だけが行い、判断は `session_restart::is_eligible` が `session_resolved` を見ない形で
/// 成立させてある
pub fn session_restart_menu_facts(
    host: &dyn ControlHost,
    pane: PaneId,
) -> tako_core::session_restart::RestartFacts {
    use tako_core::session_restart::RestartFacts;

    let role = pane_role_of(host, pane);
    let parsed = parsed_role_of(role.as_deref());
    let session = host.session(pane);
    let lines: Vec<String> = session.map(|s| s.visible_lines()).unwrap_or_default();
    let input = session.and_then(|s| s.analyze_input());
    let dialog = crate::claude_tui::detect_choice_dialog(&lines).is_some();
    RestartFacts {
        has_session: session.is_some(),
        is_agent: role.is_some(),
        is_master: parsed.kind == "master",
        agent: pane_agent_kind(pane, None).into(),
        // 会話の解決とプロセスの特定は重いのでここでは見ない（実行時に確かめる）
        session_resolved: false,
        agent_process_found: false,
        // 生成中の材料は**画面の中断ヒントだけ**。`is_busy` は完了行
        // （`Brewed for 2s · done`）も busy と読み、OSC 133 の `Running` は
        // エージェントが立っている間ずっと真になる（どちらも #1067 で実測）
        agent_busy: crate::claude_tui::interrupt_hint_visible(&lines),
        queued_messages: crate::claude_tui::queued_messages_pending(&lines),
        // ダイアログの選択カーソルは入力欄と同じ字面なので、ダイアログ中は下書きと読まない（#748）
        user_draft: !dialog
            && input.is_some_and(|s| {
                matches!(
                    s.style,
                    tako_core::InputStyle::User | tako_core::InputStyle::Mixed
                )
            }),
        dialog,
    }
}

/// ペインに貼られている role（空文字は「無い」と同じ扱い）
fn pane_role_of(host: &dyn ControlHost, pane: PaneId) -> Option<String> {
    host.workspace()
        .tabs()
        .iter()
        .flat_map(|t| t.tree().panes())
        .find(|p| p.id() == pane)
        .and_then(|p| p.role())
        .filter(|r| !r.trim().is_empty())
        .map(str::to_string)
}

fn parsed_role_of(role: Option<&str>) -> crate::sessions::ParsedRole {
    role.map(crate::sessions::parse_role)
        .unwrap_or(crate::sessions::ParsedRole {
            kind: "pane",
            project: None,
            label: None,
            profile: None,
        })
}

/// このペインで動いている agent 系統（#1067）。
///
/// worker レジストリ → セッションカタログ → 既定（claude）の順。
/// **どちらも知らなければ claude として扱う**（会話 ID が解決できなければ結局
/// `SessionUnresolved` で断るので、ここで誤って別系統を名乗らせない）
fn pane_agent_kind(
    pane: PaneId,
    catalog_entry: Option<&crate::sessions::SessionEntry>,
) -> crate::orchestrator::agent::WorkerAgent {
    use crate::orchestrator::agent::WorkerAgent;
    let registry_agent = crate::orchestrator::registry::WorkerRegistry::load()
        .ok()
        .and_then(|reg| {
            reg.workers
                .values()
                .filter(|e| e.pane == pane.as_u64() && !e.agent.is_empty())
                // 同一ペイン番号には世代が堆積するので最後に spawn したものを採る（#466 と同型）
                .max_by_key(|e| e.spawned_at.clone())
                .and_then(|e| WorkerAgent::parse(&e.agent).ok())
        });
    let entry_agent = catalog_entry
        .and_then(|e| e.agent.as_deref())
        .and_then(|a| WorkerAgent::parse(a).ok());
    registry_agent
        .or(entry_agent)
        .unwrap_or(WorkerAgent::Claude)
}

fn build_session_restart_plan(host: &dyn ControlHost, pane_id: PaneId) -> SessionRestartPlan {
    let facts = session_restart_menu_facts(host, pane_id);
    let role = pane_role_of(host, pane_id);
    let parsed = parsed_role_of(role.as_deref());

    // 会話 ID: 生きた claude（agents 経由）→ カタログの逆引き。
    // 生きた側を優先するのは、同一ペイン番号に世代が堆積する（#466 の実測）ため
    let backend = host.backend_session(pane_id);
    let catalog = crate::sessions::SessionCatalog::load().ok();
    let (session_id, session_source) = backend
        .as_deref()
        .and_then(crate::agents::resolve_session_id_for_backend)
        .map(|id| (Some(id), Some("agents-live")))
        .unwrap_or_else(|| {
            let from_catalog = catalog
                .as_ref()
                .and_then(|c| {
                    crate::sessions::resolve_session_for_pane_in(c, &pane_id.as_u64().to_string())
                })
                .filter(|id| crate::transcript::find_transcript(id).is_some());
            match from_catalog {
                Some(id) => (Some(id), Some("catalog")),
                None => (None, None),
            }
        });

    // カタログのメタ（モデル・effort・アカウント・role）で resume コマンドを組む。
    // カタログに無い会話でも role から最小のメタを合成して**同じ 1 実装**を通す
    // （コマンドの形が 2 系統に分かれると、片方だけモデルが落ちる事故になる）
    let catalog_entry = session_id
        .as_deref()
        .and_then(|id| catalog.as_ref().and_then(|c| c.entries.get(id)).cloned());
    let agent = pane_agent_kind(pane_id, catalog_entry.as_ref());

    let entry = catalog_entry.unwrap_or_else(|| crate::sessions::SessionEntry {
        kind: parsed.kind.to_string(),
        label: parsed.label.clone(),
        project: parsed.project.clone(),
        profile: parsed.profile.clone(),
        agent: Some(agent.as_str().to_string()),
        ..Default::default()
    });
    let (resume_command, resume_error) = match session_id.as_deref() {
        Some(id) => match crate::sessions::resume_command(id, &entry) {
            Ok(cmd) => (Some(cmd), None),
            Err(e) => (None, Some(e)),
        },
        None => (None, None),
    };

    // 終了させる相手。器があればそのセッション配下、無ければ PTY 直下から辿る（#728 と同じ二段）
    let snapshot = crate::agents::ProcessSnapshot::capture();
    let pids = match backend.as_deref() {
        Some(b) => snapshot.descendant_pids(b),
        None => host
            .session(pane_id)
            .and_then(|s| s.child_pid())
            .map(|pid| snapshot.descendants_with_root(pid))
            .unwrap_or_default(),
    };
    let agent_pid = crate::stale_binary::find_agent_pid_among(&snapshot, &pids, agent.as_str());

    let handoff_path = parsed
        .profile
        .as_deref()
        .and_then(crate::orchestrator::handoff_path)
        .map(|p| p.display().to_string());

    SessionRestartPlan {
        facts: tako_core::session_restart::RestartFacts {
            agent: agent.into(),
            session_resolved: resume_command.is_some(),
            agent_process_found: agent_pid.is_some(),
            ..facts
        },
        agent,
        role,
        session_id,
        session_source,
        resume_command,
        resume_error,
        agent_pid,
        handoff_path,
    }
}

/// SessionRestart — `mode` 省略で下見、指定で実行（#1067）
fn dispatch_session_restart(
    host: &mut dyn ControlHost,
    _origin: PaneOrigin,
    pane: Option<u64>,
    mode: Option<&str>,
) -> Result<Value, DispatchError> {
    use tako_core::session_restart::{self as sr, SessionRestartMode};

    let mode = match mode {
        None => None,
        Some(raw) => Some(SessionRestartMode::parse(raw).ok_or_else(|| {
            DispatchError::InvalidParams(format!(
                "mode が不正: {raw:?}（{}。省略すると下見だけを返す）",
                SessionRestartMode::values_hint()
            ))
        })?),
    };
    let (_tab_id, pane_id) = resolve_pane(host.workspace(), pane)?;
    let plan = build_session_restart_plan(host, pane_id);
    let pane_raw = pane_id.as_u64();

    // 下見の共通部分（実行時の応答にも載せる。同じ材料から作る）
    let available: Vec<&str> = sr::menu_modes(&plan.facts)
        .into_iter()
        .map(|m| m.as_str())
        .collect();
    let modes: Vec<Value> = [SessionRestartMode::Harness, SessionRestartMode::Handoff]
        .into_iter()
        .map(|m| {
            let eligible = sr::is_eligible(m, &plan.facts);
            let ready = sr::can_restart(m, &plan.facts);
            json!({
                "mode": m.as_str(),
                // メニューに出るか（構造的な可否）
                "eligible": eligible.is_ok(),
                // 今この瞬間に実行できるか（一時的な状態も見る）
                "ready": ready.is_ok(),
                "reason": ready.err().map(|b| b.as_str()),
                "message": ready.err().map(|b| b.message(pane_raw, m)),
            })
        })
        .collect();
    let mut resp = json!({
        "pane": pane_raw,
        "role": plan.role,
        "agent": plan.agent.as_str(),
        "session_id": plan.session_id,
        "session_source": plan.session_source,
        "resume_command": plan.resume_command,
        "resume_error": plan.resume_error,
        "agent_pid": plan.agent_pid,
        "available_modes": available,
        "modes": modes,
    });

    let Some(mode) = mode else {
        // 下見: 何も起こさない（#748 の respond と同じ「引数を省いたら状態だけ」）
        resp["applied"] = json!(false);
        return Ok(resp);
    };

    sr::can_restart(mode, &plan.facts)
        .map_err(|b| DispatchError::Operation(b.message(pane_raw, mode)))?;

    match mode {
        SessionRestartMode::Harness => {
            let command = plan
                .resume_command
                .clone()
                .ok_or_else(|| op_err("resume コマンドを組めなかった"))?;
            // 終了要求はここで同期的に出す（応答へ結果を載せるため）。
            // 落ちたかの確認と resume の送達は host 側の段取りが引き受ける
            let mut terminate_error: Option<String> = None;
            if let Some(pid) = plan.agent_pid {
                if let Err(e) = crate::platform::process::terminate(pid, false) {
                    terminate_error = Some(e);
                }
            }
            if let Some(e) = &terminate_error {
                return Err(DispatchError::Operation(format!(
                    "エージェントのプロセス（pid {}）を終了できなかったので建て直さない: {e}",
                    plan.agent_pid.unwrap_or(0)
                )));
            }
            host.queue_agent_relaunch(pane_id, plan.agent_pid, command.clone());
            crate::diag::flow_log(&format!(
                "セッション再起動: pane={pane_raw} mode=harness agent={} pid={:?} 会話={}",
                plan.agent.as_str(),
                plan.agent_pid,
                plan.session_id
                    .as_deref()
                    .map(crate::sessions::short_id)
                    .unwrap_or_else(|| "?".into())
            ));
            resp["applied"] = json!(true);
            resp["mode"] = json!(mode.as_str());
            resp["command"] = json!(command);
            resp["terminated_pid"] = json!(plan.agent_pid);
            resp["note"] = json!(
                "エージェントのプロセスを終了させ、落ちたことを確かめてから \
                 --resume の行を送達確認つきで送る（会話はそのまま続く）。\
                 進行は tako read / tako session-restart で確認できる"
            );
            Ok(resp)
        }
        SessionRestartMode::Handoff => {
            // #749 の自動ナッジと同じ手順を、同じ 1 実装の文面で頼む。
            // tako が引き継ぎファイルを勝手に読んで後任を立てるのではなく、
            // **エージェント自身に書き直させてから** tako_orchestrator_handoff を呼ばせる
            let prompt = tako_core::handoff::restart_prompt(plan.handoff_path.as_deref());
            host.queue_prompt_flow(pane_id, prompt.clone());
            crate::diag::flow_log(&format!(
                "セッション再起動: pane={pane_raw} mode=handoff role={}",
                plan.role.as_deref().unwrap_or("?")
            ));
            resp["applied"] = json!(true);
            resp["mode"] = json!(mode.as_str());
            resp["prompt_len"] = json!(prompt.len());
            resp["handoff_path"] = json!(plan.handoff_path);
            resp["note"] = json!(
                "引き継ぎの書き直しと tako_orchestrator_handoff の呼び出しを master へ依頼した。\
                 後任 master が同じタブに立ち、引き継ぎを確認してから前任のペインを閉じる \
                 （前任を閉じるのは後任なので、後任の起動が失敗しても master を失わない）"
            );
            Ok(resp)
        }
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

/// restart: stale ペインを張り直す（#498）。
///
/// **中身は #1067 の [`dispatch_session_restart`] 1 実装へ寄せてある**。旧実装は
/// ここに独自の手順（Ctrl+C 1 回 → `queue_write_on_alt_screen`）を持っていたが、
/// 器つきペインの外側は常に代替画面なので resume の行が**動いている claude の
/// 入力欄へ**流れ込み、しかも組んでいたコマンドが素の `claude --resume <id>` で
/// アカウント・role・モデル・effort を全部落としていた（#1067 の調査）。
///
/// **ハーネス更新（会話を保つ）を優先**する: stale の解決に必要なのはプロセスの
/// 建て直しだけで、引き継ぎは会話を要約に落としてしまう。会話を解決できない
/// ペイン（カタログにも agents にも無い）だけ引き継ぎへ落とす
fn dispatch_stale_binary_restart(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    use tako_core::session_restart::{self as sr, SessionRestartMode};

    let (_tab_id, pane_id) = resolve_pane(host.workspace(), pane)?;
    let plan = build_session_restart_plan(host, pane_id);
    // **いま実行できる方**を選ぶ（両方使えるなら会話を保つ側）。
    // `menu_modes`（構造的な可否）で選ぶと、会話 ID を解決できない master でも
    // harness を選んでしまい handoff への落ちが起きない
    let mode = [SessionRestartMode::Harness, SessionRestartMode::Handoff]
        .into_iter()
        .find(|m| sr::can_restart(*m, &plan.facts).is_ok())
        // どちらも駄目なら harness の理由を返す（会話を保つ側の理由の方が手掛かりになる）
        .unwrap_or(SessionRestartMode::Harness);
    let mut resp =
        dispatch_session_restart(host, origin, Some(pane_id.as_u64()), Some(mode.as_str()))?;
    // #498 の応答互換（`restarted` / `method` を見ている呼び出し側のため）
    resp["restarted"] = json!(true);
    resp["method"] = json!(match mode {
        SessionRestartMode::Harness => "resume",
        SessionRestartMode::Handoff => "handoff",
    });
    Ok(resp)
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
    use tako_core::pane_log::CloseOrigin;
    use tako_core::TerminalSession;

    /// セッションを起動しないテスト用ホスト（レイアウト操作の検証に使う）
    struct MockHost {
        ws: Workspace,
        attached: Vec<u64>,
        attached_options: std::collections::HashMap<u64, SpawnOptions>,
        detached: Vec<u64>,
        /// #566: close の発生源マーカー（ペインログへ書かれる文字列と同一）
        detached_markers: Vec<String>,
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
        /// #694: UI 表示モードとペイン単位の揮発解除
        ui_mode: tako_core::ui_mode::UiMode,
        starter_released: std::collections::HashSet<u64>,
        lang_setting: tako_core::i18n::LangSetting,
        lang_resolved: Option<tako_core::i18n::Lang>,
        /// #321: 利用制限表示サービス
        limit_service: tako_core::LimitService,
        preview_reload: tako_core::PreviewReloadState,
        preview_cache: tako_core::PreviewCacheStats,
        /// #584: UI 層へ依頼したウィンドウ表示状態の操作（window ID, 操作）
        window_state_ops: Vec<(u64, crate::protocol::WindowStateOp)>,
        /// #657: メニューバーの構成（UI 層が持つものの代役）
        menu_bar: crate::protocol::MenuBarSnapshot,
        /// #657: UI 層へ依頼したメニュー操作
        menu_ops: Vec<crate::protocol::MenuOp>,
        /// ペイン → バックエンド tmux セッション名（#571 の e2e で実セッションを差す）
        backend_sessions: std::collections::HashMap<u64, String>,
        /// #549: ウェルカムバナーの表示状態
        welcome_banner: bool,
        /// #600: 入力予測（既定 ON）
        autosuggest: bool,
        /// #614: 確定キーのヒント表示 / ゴースト表示中の Tab 確定（どちらも既定 ON）
        autosuggest_hint: bool,
        autosuggest_tab: bool,
        /// #666: コマンド提案カードの保管庫
        command_cards: tako_core::CommandCards,
        /// #666: クリップボードへ書いた内容（コピー検証用）
        clipboard: Vec<String>,
        /// #749: 積まれたプロンプト送達フロー（後任 master への初期プロンプト検証用）
        prompt_flows: Vec<(PaneId, String)>,
        /// #761: 積まれた遅延書き込みの検証用（#640 以降、起動コマンドはここへ来ない）
        writes: Vec<(PaneId, String)>,
        /// #640: 送達確認つきで積まれた起動コマンド（pane, 本文）
        command_flows: Vec<(PaneId, String)>,
        /// #1006: 実 PTY のセッション（既存ペインの SSH 化は「素のシェルか」を
        /// セッションから判定するので、モックでは実物を持たせる）
        sessions: std::collections::HashMap<u64, TerminalSession>,
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                ws: Workspace::new("t1", Pane::new(PaneOrigin::User)),
                attached: Vec::new(),
                attached_options: std::collections::HashMap::new(),
                detached: Vec::new(),
                detached_markers: Vec::new(),
                previews: std::collections::HashMap::new(),
                preview_views: std::collections::HashMap::new(),
                preview_outlines: std::collections::HashMap::new(),
                last_outline_target: None,
                preview_edits: std::collections::HashMap::new(),
                collapsed: std::collections::HashSet::new(),
                pins: Vec::new(),
                stale_pane_map: std::collections::HashMap::new(),
                theme_mode: tako_core::theme::ThemeMode::Dark,
                ui_mode: tako_core::ui_mode::UiMode::Terminal,
                starter_released: std::collections::HashSet::new(),
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
                menu_bar: sample_menu_bar(),
                menu_ops: Vec::new(),
                backend_sessions: std::collections::HashMap::new(),
                sessions: std::collections::HashMap::new(),
                welcome_banner: false,
                autosuggest: true,
                autosuggest_hint: true,
                autosuggest_tab: true,
                command_cards: tako_core::CommandCards::new(),
                clipboard: Vec::new(),
                prompt_flows: Vec::new(),
                writes: Vec::new(),
                command_flows: Vec::new(),
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
        fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
            self.sessions.get(&pane.as_u64())
        }
        fn attach_session(&mut self, pane: PaneId, options: SpawnOptions) {
            self.attached.push(pane.as_u64());
            self.attached_options.insert(pane.as_u64(), options);
        }
        fn queue_prompt_flow(&mut self, pane: PaneId, prompt: String) {
            self.prompt_flows.push((pane, prompt));
        }
        fn queue_write(&mut self, pane: PaneId, data: Vec<u8>) {
            self.writes
                .push((pane, String::from_utf8_lossy(&data).to_string()));
        }
        fn queue_command_flow(&mut self, pane: PaneId, command: String) {
            self.command_flows.push((pane, command));
        }
        fn detach_session(&mut self, pane: PaneId, origin: CloseOrigin, caller: Option<&str>) {
            self.detached.push(pane.as_u64());
            self.detached_markers
                .push(origin.marker_with_caller(caller));
            self.previews.remove(&pane.as_u64());
            self.preview_views.remove(&pane.as_u64());
            self.preview_outlines.remove(&pane.as_u64());
            self.preview_edits.remove(&pane.as_u64());
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
        // #694: UI 表示モード（GUI ライク表示）
        fn ui_mode(&self) -> tako_core::ui_mode::UiMode {
            self.ui_mode
        }
        fn set_ui_mode(&mut self, mode: tako_core::ui_mode::UiMode) {
            self.ui_mode = mode;
        }
        fn starter_released_panes(&self) -> Vec<PaneId> {
            self.starter_released
                .iter()
                .map(|id| PaneId::from_raw(*id))
                .collect()
        }
        fn set_starter_released(&mut self, pane: PaneId, released: bool) {
            if released {
                self.starter_released.insert(pane.as_u64());
            } else {
                self.starter_released.remove(&pane.as_u64());
            }
        }
        // #549: ウェルカムバナー
        fn welcome_banner_visible(&self) -> bool {
            self.welcome_banner
        }
        fn set_welcome_banner_visible(&mut self, visible: bool) {
            self.welcome_banner = visible;
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
        fn autosuggest_enabled(&self) -> bool {
            self.autosuggest
        }
        fn set_autosuggest(&mut self, enabled: bool) {
            self.autosuggest = enabled;
        }
        fn autosuggest_hint_enabled(&self) -> bool {
            self.autosuggest_hint
        }
        fn set_autosuggest_hint(&mut self, enabled: bool) {
            self.autosuggest_hint = enabled;
        }
        fn autosuggest_tab_enabled(&self) -> bool {
            self.autosuggest_tab
        }
        fn set_autosuggest_tab(&mut self, enabled: bool) {
            self.autosuggest_tab = enabled;
        }
        fn command_cards(&self) -> Option<&tako_core::CommandCards> {
            Some(&self.command_cards)
        }
        fn command_cards_mut(&mut self) -> Option<&mut tako_core::CommandCards> {
            Some(&mut self.command_cards)
        }
        fn queue_clipboard_copy(&mut self, text: String) -> bool {
            self.clipboard.push(text);
            true
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

    /// #1002: モデル一覧は dispatch を通るので CLI・MCP・GUI が同じペイロードを見る。
    /// **書き込みツールを増やしていない**ことも応答の `apply_command` で示す
    #[test]
    fn issue1002_モデル一覧はdispatchから同じ形で読める() {
        let mut host = MockHost::new();

        // 省略 = 3 系統ぶん（並びは WorkerAgent::ALL と同じ）
        let all = dispatch(
            &mut host,
            Request::SetupModels { agent: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        let agents = all["agents"].as_array().expect("agents 配列");
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0]["agent"], "claude");
        assert_eq!(agents[1]["agent"], "codex");
        assert_eq!(agents[2]["agent"], "agy");
        // claude は一覧コマンドを持たないので静的リスト + 取得不可の明示
        assert_eq!(agents[0]["failure"]["kind"], "no_list_command");
        assert_eq!(agents[0]["source"], "builtin");
        assert!(!agents[0]["models"].as_array().unwrap().is_empty());
        // 取得元のコマンドは正本 1 本から出る（表示と実行が食い違わない）
        assert_eq!(agents[1]["list_command"], "codex debug models");
        assert_eq!(agents[2]["list_command"], "agy models");
        // 反映は既存のプロファイル経路に任せる
        assert!(all["apply_command"]
            .as_str()
            .unwrap()
            .contains("orchestrator profiles set"));

        // 系統を絞れる（CLI `--agent` / MCP `agent` と 1:1）
        let one = dispatch(
            &mut host,
            Request::SetupModels {
                agent: Some("agy".into()),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(one["agents"].as_array().unwrap().len(), 1);
        assert_eq!(one["agents"][0]["agent"], "agy");

        // 不正な系統は分類済みエラー（対応値を挙げる）
        let err = dispatch(
            &mut host,
            Request::SetupModels {
                agent: Some("gemini".into()),
            },
            PaneOrigin::Mcp,
        )
        .expect_err("未対応の系統は拒否する");
        assert!(
            format!("{err:?}").contains("claude / codex / agy"),
            "{err:?}"
        );
    }

    /// #813: 自動復帰のオプトインは既定 OFF で、dispatch から読み書きでき、
    /// list / read にも同じ値が出る（UI・CLI・MCP はすべてこの dispatch を通る）
    #[test]
    fn issue813_自動復帰の設定と参照が全経路で同じ値を返す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let other = split(&mut host, root);

        // 既定は OFF
        let q = dispatch(
            &mut host,
            Request::LimitResume {
                pane: Some(root),
                enabled: None,
                all: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(q["pane"], root);
        assert_eq!(q["enabled"], false);

        // ON にすると応答・list の両方に出る（他のペインには波及しない）
        let on = dispatch(
            &mut host,
            Request::LimitResume {
                pane: Some(root),
                enabled: Some(true),
                all: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(on["enabled"], true);
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let panes = list["tabs"][0]["panes"].as_array().unwrap();
        let flag = |id: u64| {
            panes
                .iter()
                .find(|p| p["id"] == id)
                .map(|p| p["limit_autoresume"].clone())
                .unwrap()
        };
        assert_eq!(flag(root), json!(true));
        assert_eq!(flag(other), json!(false), "他のペインへ波及しない");

        // all で一覧できる
        let all = dispatch(
            &mut host,
            Request::LimitResume {
                pane: None,
                enabled: None,
                all: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let entries = all["panes"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e["pane"] == root && e["enabled"] == true));

        // OFF に戻せる
        let off = dispatch(
            &mut host,
            Request::LimitResume {
                pane: Some(root),
                enabled: Some(false),
                all: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(off["enabled"], false);

        // all と enabled の併用は拒否（設定と一覧の取り違えを構造的に防ぐ）
        let bad = dispatch(
            &mut host,
            Request::LimitResume {
                pane: None,
                enabled: Some(true),
                all: Some(true),
            },
            PaneOrigin::Cli,
        );
        assert!(
            matches!(bad, Err(DispatchError::InvalidParams(_))),
            "{bad:?}"
        );

        // 存在しないペインはエラー
        let missing = dispatch(
            &mut host,
            Request::LimitResume {
                pane: Some(9999),
                enabled: Some(true),
                all: None,
            },
            PaneOrigin::Cli,
        );
        assert!(matches!(missing, Err(DispatchError::PaneNotFound(9999))));
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
                cwd: None,
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
                caller_role: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["closed"].as_u64(), Some(new_id));
        assert_eq!(host.detached, vec![new_id]);
        assert_eq!(host.ws.active_tab().tree().len(), 1);
    }

    /// #566: dispatch 経由の close は確認を挟まない（AI フルコントロール維持）が、
    /// 発生源は必ず記録する。CLI / MCP / 呼び出し元 role が事後に区別できること
    #[test]
    fn dispatch_closeは経路と呼び出し元をペインログへ記録する() {
        // CLI 経由（role なし）
        let mut host = MockHost::new();
        let root = host.root_pane();
        let cli_pane = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Close {
                pane: Some(cli_pane),
                force: false,
                caller_role: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.detached_markers,
            vec!["close:dispatch(cli)".to_string()]
        );

        // MCP 経由（呼び出し元 role つき = どのエージェントが閉じたか）
        let mcp_pane = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Close {
                pane: Some(mcp_pane),
                force: false,
                caller_role: Some("orchestrator-master:takodev".into()),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(
            host.detached_markers[1],
            "close:dispatch(mcp, caller=orchestrator-master:takodev)"
        );
    }

    /// #566: BackgroundKill（たまり場からの kill）も発生源が残る
    #[test]
    fn background_killも発生源を記録する() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let pane = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Background {
                pane: Some(pane),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        dispatch(&mut host, Request::BackgroundKill { pane }, PaneOrigin::Mcp).unwrap();
        assert_eq!(
            host.detached_markers,
            vec!["close:dispatch(mcp)".to_string()]
        );
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
                cwd: None,
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
                caller_role: None,
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
                caller_role: None,
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
                cwd: None,
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
                cwd: None,
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
                cwd: None,
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
                cwd: None,
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
                cwd: None,
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

    /// #552 案 4「この名前を固定」: 自動命名された名前を打ち直さずに固定でき、
    /// 固定後は自動リネームの対象外になる（GUI のピン印と同じ経路）
    #[test]
    fn タブ名の固定と解除() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let tab_id = host.ws.tabs()[0].id().as_u64();
        // 自動命名された状態を作る
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: "tako 検証".into(),
                source: Some("auto".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();

        // 変更せずに状態だけ取得できる
        let status = dispatch(
            &mut host,
            Request::TabPinTitle {
                pane: Some(root),
                tab: None,
                pinned: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(status["pinned"].as_bool(), Some(false));
        assert_eq!(status["source"].as_str(), Some("auto"));

        // 固定: 名前は変えずに手動指定へ
        let pinned = dispatch(
            &mut host,
            Request::TabPinTitle {
                pane: Some(root),
                tab: None,
                pinned: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(pinned["title"].as_str(), Some("tako 検証"));
        assert_eq!(pinned["pinned"].as_bool(), Some(true));
        assert_eq!(
            host.ws.tabs()[0].title_source(),
            tako_core::TitleSource::Manual
        );
        // 固定後は自動リネームが通らない
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: "別の自動名".into(),
                source: Some("auto".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs()[0].title(), "tako 検証");

        // 解除すると自動リネームが再開する（タイトルは保持）
        let released = dispatch(
            &mut host,
            Request::TabPinTitle {
                pane: None,
                tab: Some(tab_id),
                pinned: Some(false),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(released["pinned"].as_bool(), Some(false));
        assert_eq!(released["title"].as_str(), Some("tako 検証"));
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: "別の自動名".into(),
                source: Some("auto".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs()[0].title(), "別の自動名");
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
                cwd: None,
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
                cwd: None,
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
                    new_tab: false,
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

    /// FR-3.22 / #835: `new_tab` は「そのファイルだけが載った 1 枚」を作る。
    /// Finder の「このアプリケーションで開く」がこの経路を通る
    #[test]
    fn open_fileのnew_tabはファイル専用のタブを作る() {
        let dir = std::env::temp_dir().join(format!("tako-dispatch-newtab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("b.md"), "# 見出し").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let first_tab = host.ws.active_tab_id();
        let open_new_tab = |host: &mut MockHost, path: String, focus: Option<bool>| {
            dispatch(
                host,
                Request::OpenFile {
                    pane: Some(root),
                    path,
                    mode: None,
                    direction: None,
                    focus,
                    new_tab: true,
                },
                PaneOrigin::Mcp,
            )
        };

        let result = open_new_tab(
            &mut host,
            dir.join("a.rs").display().to_string(),
            Some(true),
        )
        .unwrap();
        let tab_a = TabId::from_raw(result["tab"].as_u64().unwrap());
        assert_ne!(tab_a, first_tab, "新しいタブが作られる");
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(result["mode"].as_str(), Some("code"));
        // 元のタブは 1 ペインのまま = 既存の作業を一切動かさない
        assert_eq!(host.ws.get_tab(first_tab).unwrap().tree().len(), 1);
        // 新しいタブは「プレビュー 1 枚だけ」。ターミナルは起動しない
        assert_eq!(host.ws.get_tab(tab_a).unwrap().tree().len(), 1);
        assert!(
            host.attached.is_empty(),
            "プレビュー専用タブは PTY を持たない"
        );
        // タブ名はファイル名で、自動リネームに奪われない手動扱い
        assert_eq!(host.ws.get_tab(tab_a).unwrap().title(), "a.rs");
        assert_eq!(
            host.ws.get_tab(tab_a).unwrap().title_source(),
            tako_core::TitleSource::Manual
        );
        // focus=true なので新しいタブが前に出る
        assert_eq!(host.ws.active_tab_id(), tab_a);

        // 2 ファイル目も再利用せず別のタブになる（複数選択で全部が同時に見える）
        let result = open_new_tab(
            &mut host,
            dir.join("b.md").display().to_string(),
            Some(true),
        )
        .unwrap();
        let tab_b = TabId::from_raw(result["tab"].as_u64().unwrap());
        assert_ne!(tab_b, tab_a);
        assert_eq!(result["mode"].as_str(), Some("markdown"));
        assert_eq!(host.ws.tabs().len(), 3);

        // focus 省略（CLI / MCP の既定）はアクティブタブを奪わない
        let before = host.ws.active_tab_id();
        let result = open_new_tab(&mut host, dir.join("a.rs").display().to_string(), None).unwrap();
        assert_eq!(
            host.ws.active_tab_id(),
            before,
            "既定はユーザーの表示を奪わない"
        );
        assert_ne!(
            TabId::from_raw(result["tab"].as_u64().unwrap()),
            before,
            "タブ自体は裏で作られる"
        );

        // direction とは排他（新しいタブには分割元が無い）
        let conflict = dispatch(
            &mut host,
            Request::OpenFile {
                pane: Some(root),
                path: dir.join("a.rs").display().to_string(),
                mode: None,
                direction: Some(Direction::Right),
                focus: None,
                new_tab: true,
            },
            PaneOrigin::Mcp,
        );
        assert!(conflict.is_err(), "new_tab + direction はエラー");

        // ディレクトリ・不在パスは new_tab でもエラー（タブを作り散らかさない）
        let tabs_before = host.ws.tabs().len();
        assert!(open_new_tab(&mut host, dir.display().to_string(), None).is_err());
        assert!(open_new_tab(&mut host, dir.join("no-such").display().to_string(), None).is_err());
        assert_eq!(host.ws.tabs().len(), tabs_before, "失敗時にタブは増えない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #835: フォルダを渡されたときの受け皿。`tako tab new --cwd` = Finder から
    /// フォルダを開いたときの「そのフォルダでシェルを起動する」経路
    #[test]
    fn tab_newのcwdはそのフォルダでシェルを起動する() {
        let dir = std::env::temp_dir().join(format!("tako-dispatch-tabcwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: Some("proj".into()),
                focus: Some(true),
                cwd: Some(dir.display().to_string()),
            },
            PaneOrigin::User,
        )
        .unwrap();
        let pane = result["pane"].as_u64().unwrap();
        let spawned = host
            .attached_options
            .get(&pane)
            .expect("シェルが起動依頼される");
        assert_eq!(
            spawned.cwd.as_deref(),
            Some(dir.canonicalize().unwrap().as_path()),
            "頼まれたフォルダでシェルが立つ"
        );
        assert!(spawned.command.is_none(), "既定シェルを起動する");

        // 存在しない・フォルダでないパスは起動前にエラー（黙って別の場所で開かない）
        assert!(dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
                cwd: Some(dir.join("no-such").display().to_string()),
            },
            PaneOrigin::User,
        )
        .is_err());
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        assert!(dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
                cwd: Some(dir.join("f.txt").display().to_string()),
            },
            PaneOrigin::User,
        )
        .is_err());
        // cwd 省略は従来どおり継承（回帰防止）
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: None,
                focus: None,
                cwd: None,
            },
            PaneOrigin::User,
        )
        .unwrap();
        let pane = result["pane"].as_u64().unwrap();
        assert!(host.attached_options.get(&pane).unwrap().cwd.is_none());
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

    /// #600: 入力予測は既定 ON で、取得と切替が往復する
    #[test]
    fn autosuggestは状態を取得変更できる() {
        let mut host = MockHost::new();
        let initial = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: None,
                hint: None,
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(initial["enabled"], true, "既定 ON");
        // 何が効くのかを AI が判断できる情報を必ず返す
        assert_eq!(initial["shell"], "zsh");
        assert_eq!(initial["provider"], "zsh-autosuggestions");
        assert_eq!(
            initial["version"],
            tako_core::shell_integration::AUTOSUGGEST_VERSION
        );

        let off = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: Some(false),
                hint: None,
                tab: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(off["enabled"], false);
        assert!(!host.autosuggest);
        // #614: 本体だけ触ったときにヒント / Tab 確定を巻き込まない
        assert_eq!(off["hint"], true);
        assert_eq!(off["tab_accept"], true);

        let on = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: Some(true),
                hint: None,
                tab: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(on["enabled"], true);
        assert!(host.autosuggest);
    }

    /// #614: 確定キーのヒントと Tab 確定は本体と独立に切り替えられる。
    /// 「Tab 確定を切ったのに『Tab で確定』と案内する」矛盾も起こさない
    #[test]
    fn autosuggestのヒントとtab確定は独立に切り替えられる() {
        let mut host = MockHost::new();
        let initial = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: None,
                hint: None,
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(initial["hint"], true, "ヒントは既定 ON");
        assert_eq!(initial["tab_accept"], true, "Tab 確定は既定 ON");
        assert_eq!(initial["accept_keys"], json!(["Right", "Tab"]));

        // Tab 確定だけ切る（本体とヒントは維持）
        let no_tab = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: None,
                hint: None,
                tab: Some(false),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(no_tab["enabled"], true);
        assert_eq!(no_tab["hint"], true);
        assert_eq!(no_tab["tab_accept"], false);
        assert_eq!(
            no_tab["accept_keys"],
            json!(["Right"]),
            "Tab 確定 OFF なのに Tab を確定キーとして案内している"
        );
        assert!(!host.autosuggest_tab);

        // ヒントだけ恒久 OFF（Tab 確定は触らない）
        let no_hint = dispatch(
            &mut host,
            Request::Autosuggest {
                enabled: None,
                hint: Some(false),
                tab: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(no_hint["hint"], false);
        assert_eq!(no_hint["tab_accept"], false, "Tab 確定を巻き戻していない");
        assert!(!host.autosuggest_hint);
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
                    new_tab: false,
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
                new_tab: false,
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
                new_tab: false,
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
                caller_role: None,
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
                caller_role: None,
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
            // 存在するディレクトリならどこでもよい。`/tmp` を決め打ちすると Windows で
            // 「cwd が存在しない」で spawn 系テストが一族まるごと落ちる（#467 / #583）
            let cwd = std::env::temp_dir().to_string_lossy().to_string();
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
            limit_resume: None,
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
                host.command_flows
                    .iter()
                    .map(|(p, c)| (p.as_u64(), c.clone()))
                    .collect::<Vec<_>>(),
                vec![(pane, cmd)],
                "起動コマンドは送達確認つきフローへ登録される"
            );
            assert!(
                host.writes.is_empty(),
                "起動コマンドを書きっぱなしのキューへ積んではいけない（#640 の再発）: {:?}",
                host.writes
                    .iter()
                    .map(|(p, d)| (p.as_u64(), d.len()))
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
                    cwd: None,
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
                    cwd: None,
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
            // クォートの有無はシェルの方言で変わる（#867）ので値だけを見る
            assert!(cmd.contains("gpt-5.6-terra"), "{cmd}");
            assert!(
                cmd.contains("model_reasoning_effort=medium")
                    || cmd.contains("model_reasoning_effort='medium'"),
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
            // クォートの形はシェルの方言で変わる（#867）。ここが見たいのは
            // 「空白入りのモデル名が 1 引数として渡る」こと
            assert!(
                cmd.contains("--model 'Gemini 3.5 Flash (High)'")
                    || cmd.contains("--model \"Gemini 3.5 Flash (High)\""),
                "{cmd}"
            );
            // agy にも `--effort low|medium|high` が実在する（#1002 で実測）。
            // 値のクォートは方言で変わるので、フラグと値が並ぶことだけを見る
            assert!(
                cmd.contains("--effort high") || cmd.contains("--effort 'high'"),
                "agy へ effort を渡す: {cmd}"
            );
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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
                limit: None,
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

    #[test]
    fn finish_worker_statusがペイン不在でgoneを返す() {
        let ctx = WorkerStatusCtx {
            pane_id: 0,
            pane_exists: false,
            backend_session: None,
            live_tail: None,
            full_screen: None,
            has_running_children: false,
            limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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

        // worker は既定 config dir で走らせるので、事前信頼もそちらへ書く（#558）
        let default_cfg = crate::orchestrator::claude_default_config_dir()
            .expect("既定 config dir")
            .display()
            .to_string();
        crate::claude_tui::ensure_trusted_in(Some(&default_cfg), &work.display().to_string())
            .expect("事前信頼を書ける");

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

    // --- #577 E2E: 実 tmux + 実 claude で permission ダイアログの検知を通しで確認する ---
    //
    // Issue #577 の再現手順（brace expansion を含む Bash 実行 = 自動承認されない）を
    // そのまま流し、watch が WORKER_QUESTION ではなく WORKER_PERMISSION を出すこと、
    // `worker_status.permission_dialog` が構造化情報を返すことを確かめる。手動実行:
    //
    // ```sh
    // cargo test -p tako-control --lib issue577_e2e -- --ignored --nocapture --test-threads=1
    // ```
    //
    // 前提: `claude` CLI がログイン済み / `tmux` がある / ネットワーク接続。
    // tmux ソケットと data ディレクトリは隔離するので本番の tako / tmux には触れない

    const E2E_SOCKET_577: &str = "tako-e2e-577";

    fn e2e_577_base() -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/private/tmp/tako-e2e-577-{}", std::process::id()))
    }

    /// テスト用の一時ディレクトリだけを消す（#512 の事故を構造で防ぐ）
    fn remove_e2e_577_dir(dir: &std::path::Path) {
        let allowed = dir.parent() == Some(std::path::Path::new("/private/tmp"))
            && dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tako-e2e-577-"));
        assert!(
            allowed,
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    struct E2e577Guard {
        dir: std::path::PathBuf,
    }

    impl Drop for E2e577Guard {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-L", E2E_SOCKET_577, "kill-server"])
                .output();
            remove_e2e_577_dir(&self.dir);
            remove_e2e_trust_entry(&self.dir.join("work"));
        }
    }

    /// e2e が書いた事前信頼エントリを claude の `.claude.json` から除去する（best-effort）。
    /// 消さないと実行のたびに `/private/tmp/tako-e2e-577-<pid>/work` が溜まり続ける
    /// （claude_tui_e2e の `remove_trust_entry` と同じ後始末）
    fn remove_e2e_trust_entry(dir: &std::path::Path) {
        let key = dir.display().to_string();
        for path in crate::claude_tui::config_json_paths(None) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut root) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) else {
                continue;
            };
            if projects.remove(&key).is_some() {
                if let Ok(serialized) = serde_json::to_string_pretty(&root) {
                    let _ = std::fs::write(&path, serialized);
                }
            }
        }
    }

    #[test]
    #[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
    fn issue577_e2e_実claudeのpermissionダイアログをwatchが検知する() {
        use std::time::{Duration, Instant};

        let base = e2e_577_base();
        remove_e2e_577_dir(&base);
        let work = base.join("work");
        let data = base.join("data");
        for d in [&work, &data] {
            std::fs::create_dir_all(d).expect("一時ディレクトリを作れる");
        }
        let guard = E2e577Guard { dir: base.clone() };

        std::env::set_var("TAKO_TMUX_SOCKET", E2E_SOCKET_577);
        std::env::set_var("TAKO_DATA_DIR", &data);

        // 信頼ダイアログを出さない（出ると permission ダイアログまで到達しない）
        let default_cfg = crate::orchestrator::claude_default_config_dir()
            .expect("既定 config dir")
            .display()
            .to_string();
        crate::claude_tui::ensure_trusted_in(Some(&default_cfg), &work.display().to_string())
            .expect("事前信頼を書ける");

        let _ = std::process::Command::new("tmux")
            .args(["-L", E2E_SOCKET_577, "kill-server"])
            .output();
        let session = "w577";
        let status = std::process::Command::new("tmux")
            // 親（tako の worker ペイン）の env を持ち込むと agents 一覧に載らない
            .env_remove(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV)
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .args([
                "-L",
                E2E_SOCKET_577,
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
                // permission ダイアログを出させるので --dangerously-skip-permissions は付けない
                "unset CLAUDE_CONFIG_DIR CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID; \
                 exec claude --model haiku",
            ])
            .status()
            .expect("tmux を実行できる");
        assert!(status.success(), "tmux new-session が失敗した");

        let mut host = MockHost::new();
        let pane = host.root_pane();
        host.backend_sessions.insert(pane, session.to_string());

        // Issue #577 の再現プロンプト: brace expansion を含む Bash は
        // 「Contains brace_expression」で必ず承認を求められる
        let report = crate::claude_tui::deliver_via_tmux(
            Some(E2E_SOCKET_577),
            session,
            "Use the Bash tool to run exactly this command, without asking me first: \
             for i in {1..3}; do echo $i; done",
            true,
        )
        .expect("送達が完了する");
        assert!(
            report.verified,
            "プロンプトが入力欄へ反映される: {report:?}"
        );

        // ダイアログの実在は **検知関数に頼らず** 画面の文言で確認する
        let dialog_deadline = Instant::now() + Duration::from_secs(180);
        let mut screen = String::new();
        while Instant::now() < dialog_deadline {
            screen = tako_core::tmux::capture_session(Some(E2E_SOCKET_577), session)
                .map(|l| l.join("\n"))
                .unwrap_or_default();
            if screen.contains("Do you want to proceed?") {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            screen.contains("Do you want to proceed?"),
            "承認ダイアログが出るはず。画面:\n{screen}"
        );

        let worker_status = |host: &mut MockHost, session_id: Option<String>| -> Value {
            dispatch(
                host,
                Request::OrchestratorWorkerStatus {
                    pane_id: Some(pane),
                    session_id,
                    tmux_session: Some(session.to_string()),
                    worker: None,
                },
                PaneOrigin::Cli,
            )
            .expect("worker_status が引ける")
        };

        // ① agents 解決に成功する経路。この claude 版は permission 待ちで生 status
        //    `waiting` を返すので、修正前からここは waiting になる（非回帰の確認）
        let auto = worker_status(&mut host, None);
        println!(
            "[#577 e2e] agents 経路: status={} source={}",
            auto["status"], auto["status_source"]
        );
        assert_eq!(auto["status"], "waiting", "{auto}");
        assert!(auto["permission_dialog"].is_object(), "{auto}");

        // ② **#577 の本体**: agents 一覧に載らない worker（別 config dir 継承・
        //    CLAUDE_CODE_CHILD_SESSION つき起動・codex / agy）を模して、解決できない
        //    session ID を渡し status_source=screen へ落とす。修正前はこの経路が
        //    「idle + question」で、permission_dialog は常に null だった
        const MISSING_SID: &str = "00000000-0000-4000-8000-000000000577";
        let screened = worker_status(&mut host, Some(MISSING_SID.to_string()));
        println!(
            "[#577 e2e] 画面推定経路: status={} source={} events={}",
            screened["status"], screened["status_source"], screened["events"]
        );
        assert_eq!(
            screened["status_source"], "screen",
            "agents が解決できない状況を作れている: {screened}"
        );
        assert_eq!(
            screened["status"], "waiting",
            "修正前は idle（`❯ 1. Yes` を入力欄と見なす）: {screened}"
        );
        assert!(
            screened["permission_dialog"].is_object(),
            "修正前は常に null: {screened}"
        );
        let options = screened["permission_dialog"]["options"]
            .as_array()
            .expect("選択肢が取れる");
        assert!(options.len() >= 2, "選択肢が構造化される: {screened}");
        let kinds: Vec<&str> = screened["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"permission_dialog"), "{kinds:?}");
        assert!(
            !kinds.contains(&"question"),
            "question は出さない: {kinds:?}"
        );

        // ③ watch（画面推定経路）: WORKER_PERMISSION（修正前は WORKER_QUESTION）
        let opts = crate::orchestrator::wait::WatchOptions {
            pane_id: pane,
            session_id: Some(MISSING_SID.to_string()),
            tmux_session: Some(session.to_string()),
            timeout: Some(Duration::from_secs(90)),
            initial_delay: Duration::ZERO,
            interval: Duration::from_secs(2),
        };
        let outcome = {
            let mut exec =
                |req: Request| dispatch(&mut host, req, PaneOrigin::Cli).map_err(|e| e.to_string());
            crate::orchestrator::wait::wait_for_worker(&mut exec, &opts, None)
        };
        let crate::orchestrator::wait::WatchOutcome::PermissionWaiting {
            ref permission_dialog,
        } = outcome
        else {
            panic!("WORKER_PERMISSION を返す（修正前は Question）: {outcome:?}\n画面:\n{screen}");
        };
        println!("[#577 e2e] permission_dialog = {permission_dialog}");

        // ④ 応答（choice 1 = Yes 一回だけ）で解除でき、以後は通常の完了検知に戻る
        //    （#571 の非回帰。ダイアログを永続 waiting に固定していない）
        dispatch(
            &mut host,
            Request::OrchestratorRespond {
                pane_id: pane,
                choice: Some("1".into()),
                caller_role: None,
            },
            PaneOrigin::Cli,
        )
        .expect("permission ダイアログへ応答できる");

        let after = {
            let mut exec =
                |req: Request| dispatch(&mut host, req, PaneOrigin::Cli).map_err(|e| e.to_string());
            crate::orchestrator::wait::wait_for_worker(&mut exec, &opts, None)
        };
        drop(guard);
        assert!(
            matches!(after, crate::orchestrator::wait::WatchOutcome::Idle { .. }),
            "承認後は通常どおり Idle で完了する: {after:?}"
        );
    }

    // --- #577: permission ダイアログ待ちを waiting へ格上げする ---

    /// permission ダイアログ待ちの実画面（Issue #577 の再現時に採取した形。
    /// brace expansion を含む Bash 実行の承認要求）
    const PERMISSION_SCREEN_577: &str = "\
⏺ Running 1 shell command…\n\
────────────────────────────────────────────────\n\
 Bash command\n\
   for i in {1..12}; do echo $i; sleep 1; done; echo done\n\
 Contains brace_expression\n\
 Do you want to proceed?\n\
 ❯ 1. Yes\n\
   2. Yes, and don't ask again for echo commands\n\
   3. No, and tell Claude what to do differently (esc)\n\
 Esc to cancel · Tab to amend · ctrl+e to explain";

    /// worker が **本文で** 質問して入力待ちになった画面（入力欄は最下部に健在）
    const QUESTION_SCREEN_577: &str = "\
⏺ 2 通りの直し方があります。どちらにしますか?\n\
  1. 既存 API を変えずに互換レイヤを足す\n\
  2. 破壊的変更として一気に置き換える\n\
────────────────────────────────────────────────\n\
❯ \n\
────────────────────────────────────────────────\n\
  claude-opus-5 · ctx 23%";

    fn resolved_with_screen(status: &str, source: &str, screen: &str) -> ResolvedWorkerStatus {
        let mut r = resolved(status, source, true);
        r.recent_output = Some(screen.into());
        r
    }

    #[test]
    fn issue577_agentsがidleでも画面のダイアログでwaitingへ格上げする() {
        // agents が idle を返す（一覧の取りこぼし・claude 以外・古い版）状況でも、
        // 画面にダイアログが実在すれば停止側が正。旧実装は agents の waiting だけを
        // 根拠にしていたので permission_dialog が null のままだった
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            PERMISSION_SCREEN_577,
        ))
        .unwrap();
        assert_eq!(v["status"], "waiting");
        let dialog = &v["permission_dialog"];
        assert!(dialog.is_object(), "構造化情報が付く: {dialog}");
        assert!(
            dialog["command"]
                .as_str()
                .unwrap_or_default()
                .contains("for i in {1..12}"),
            "承認対象のコマンドを抽出する: {dialog}"
        );
        let options = dialog["options"].as_array().expect("選択肢が配列");
        assert_eq!(options.len(), 3);
        assert_eq!(options[0], "Yes");
        assert_eq!(dialog["highlighted"], 0);

        // events は permission_dialog のみ（question は出さない = master が
        // respond ではなく通常の質問応答へ流れるのを防ぐ）
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"permission_dialog"), "{kinds:?}");
        assert!(!kinds.contains(&"question"), "{kinds:?}");
    }

    #[test]
    fn issue577_画面推定経路でもwaitingへ格上げする() {
        // codex / agy や agents 解決失敗時（status_source=screen）。
        // `❯ 1. Yes` を入力欄と見なして idle になっていた経路
        let v = apply_worker_status_corrections(resolved_with_screen(
            "unknown",
            "screen",
            PERMISSION_SCREEN_577,
        ))
        .unwrap();
        assert_eq!(v["status"], "waiting");
        assert!(v["permission_dialog"].is_object());
    }

    #[test]
    fn issue577_本物の質問はidleのままでpermission_dialogはnull() {
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            QUESTION_SCREEN_577,
        ))
        .unwrap();
        assert_eq!(v["status"], "idle", "格上げしない");
        assert!(v["permission_dialog"].is_null());
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"question"),
            "question は従来どおり: {kinds:?}"
        );
    }

    #[test]
    fn issue577_通常のidleはidleのまま() {
        // #571 の非回帰: ダイアログの無い停止画面を waiting に化けさせない
        let v = apply_worker_status_corrections(resolved("idle", "agents-auto", true)).unwrap();
        assert_eq!(v["status"], "idle");
        assert!(v["permission_dialog"].is_null());
    }

    #[test]
    fn issue577_生成中はダイアログと判定しない() {
        // busy 中の claude は入力欄が最下部に見えている（= 入力を奪われていない）。
        // 会話ログ上流にダイアログの残骸があっても格上げしない
        let mut screen = PERMISSION_SCREEN_577.to_string();
        screen.push_str("\n✽ Misting… (1m 4s · ↓ 2.1k tokens)\n────────\n❯ \n────────\n  ctx 23%");
        let v =
            apply_worker_status_corrections(resolved_with_screen("busy", "agents-auto", &screen))
                .unwrap();
        assert_eq!(v["status"], "busy");
        assert!(v["permission_dialog"].is_null());
    }

    // --- #748: permission 以外の選択肢ダイアログ ---

    /// `/model` のモデル選択（実採取。#748 の screens/02）。
    /// **raw string で書く**こと: `"\<改行>"` の継続は次行の行頭空白を落とすので、
    /// 桁揃えが意味を持つダイアログ画面は再現できない
    const MODEL_SELECT_748: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models.

     1. Default (recommended)  Opus 5
   ❯ 2. Opus (1M context)      Opus 5 with 1M context
     3. Sonnet                 Sonnet 5

   Enter to set as default · Esc to cancel"#;

    /// claude の usage limit 対処ダイアログ（実文言 = バイナリ内文字列。#748）
    const LIMIT_DIALOG_748: &str = r#"⏺ 実装を続けます
  ⎿  Claude usage limit reached. Your limit will reset at 3am.

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   What do you want to do?

   ❯ 1. Stop and wait for limit to reset
     2. Upgrade to Max 20x for higher session limits every month

   Enter to confirm · Esc to cancel"#;

    /// `/mcp` の一覧（実採取。**番号なし** + セクション見出し混在）
    const MCP_LIST_748: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Manage MCP servers

     User MCPs
   ❯ context7 · ✔ connected · 2 tools
     filesystem · ✔ connected · 14 tools
     tako · ✔ connected · 133 tools

   ↑/↓ to navigate · Enter to confirm · Esc to cancel"#;

    #[test]
    fn issue748_モデル選択ダイアログでwaitingとchoice_dialogを返す() {
        // 旧実装は permission ダイアログしか見ておらず、この画面は
        // 「idle + question」= 完了扱いで通知されていた（#748 の観測 2）
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            MODEL_SELECT_748,
        ))
        .unwrap();
        assert_eq!(v["status"], "waiting");
        assert!(v["permission_dialog"].is_null(), "permission ではない");
        let dialog = &v["choice_dialog"];
        assert_eq!(dialog["kind"], "select");
        assert_eq!(dialog["numbered"], true);
        assert_eq!(dialog["highlighted"], 1);
        assert_eq!(dialog["options"].as_array().unwrap().len(), 3);
        assert_eq!(dialog["recommended_action"], "respond");
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"choice_dialog"), "{kinds:?}");
        assert!(
            !kinds.contains(&"question"),
            "ダイアログ待ちを質問として出さない: {kinds:?}"
        );
    }

    #[test]
    fn issue748_limitダイアログはerrorのままchoice_dialogが付く() {
        // 「解除まで待つ」復旧は error 側（#157 / #401 の supervisor）が持っている。
        // waiting へ格上げしてその経路を迂回させない。ただし選択肢の構造は返す
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            LIMIT_DIALOG_748,
        ))
        .unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"]["kind"], "usage_limit");
        assert_eq!(v["error"]["recommended_action"], "wait_reset");
        let dialog = &v["choice_dialog"];
        assert_eq!(dialog["kind"], "usage_limit");
        assert_eq!(
            dialog["options"][0]["label"],
            "Stop and wait for limit to reset"
        );
        assert_eq!(dialog["recommended_action"], "respond_wait");
    }

    #[test]
    fn issue748_permissionダイアログはchoice_dialogにも載る() {
        // 既存の permission_dialog は互換のまま、種別つきの構造も並べて返す
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            PERMISSION_SCREEN_577,
        ))
        .unwrap();
        assert_eq!(v["status"], "waiting");
        assert!(v["permission_dialog"].is_object());
        assert_eq!(v["choice_dialog"]["kind"], "permission");
        // events は permission_dialog のみ（choice_dialog と二重に出さない）
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"permission_dialog"), "{kinds:?}");
        assert!(!kinds.contains(&"choice_dialog"), "{kinds:?}");
    }

    #[test]
    fn issue748_ダイアログの無い画面ではchoice_dialogはnull() {
        let v = apply_worker_status_corrections(resolved_with_screen(
            "idle",
            "agents-auto",
            QUESTION_SCREEN_577,
        ))
        .unwrap();
        assert_eq!(v["status"], "idle");
        assert!(v["choice_dialog"].is_null());
    }

    fn dialog_of(screen: &str) -> crate::claude_tui::ChoiceDialog {
        let lines: Vec<String> = screen.lines().map(|l| l.to_string()).collect();
        crate::claude_tui::detect_choice_dialog(&lines).expect("ダイアログが検知される")
    }

    #[test]
    fn issue748_choiceは番号とラベルとエイリアスで解決する() {
        let dialog = dialog_of(PERMISSION_SCREEN_577);
        // 番号
        assert_eq!(resolve_choice_index(&dialog, "1").unwrap(), 0);
        assert_eq!(resolve_choice_index(&dialog, "3").unwrap(), 2);
        // エイリアス（#319 互換）
        assert_eq!(resolve_choice_index(&dialog, "yes").unwrap(), 0);
        assert_eq!(resolve_choice_index(&dialog, "no").unwrap(), 2);
        // ラベルの部分一致（大小無視）
        assert_eq!(resolve_choice_index(&dialog, "don't ask again").unwrap(), 1);
        // 範囲外・不一致はエラー（選択肢一覧を添える）
        let err = resolve_choice_index(&dialog, "9").unwrap_err().to_string();
        assert!(err.contains("範囲外") && err.contains("1. Yes"), "{err}");
        let err = resolve_choice_index(&dialog, "存在しない")
            .unwrap_err()
            .to_string();
        assert!(err.contains("一致する選択肢が無い"), "{err}");
    }

    #[test]
    fn issue748_曖昧なラベルは確定させずエラーにする() {
        // モデル選択の「opus」は 2 つの選択肢に一致する（Default … Opus 5 /
        // Opus (1M context)）。勝手にどちらかを選ぶと worker のモデルが変わるので
        // 番号を要求する（黙って推測しない）
        let dialog = dialog_of(MODEL_SELECT_748);
        let err = resolve_choice_index(&dialog, "opus")
            .unwrap_err()
            .to_string();
        assert!(err.contains("複数の選択肢に一致"), "{err}");
    }

    #[test]
    fn issue748_番号なしダイアログはラベルで選べる() {
        let dialog = dialog_of(MCP_LIST_748);
        assert!(!dialog.numbered);
        let i = resolve_choice_index(&dialog, "tako").unwrap();
        assert_eq!(dialog.options[i].label, "tako · ✔ connected · 133 tools");
        // 1 始まりの順番でも指定できる
        let by_number = resolve_choice_index(&dialog, "1").unwrap();
        assert_eq!(by_number, 0);
    }

    #[test]
    fn issue748_ダイアログ中のsendは選択肢つきで断る() {
        // #748 の観測 1 / 4: テキストや Enter はダイアログのキー操作として食われる
        let refusal = dialog_send_refusal(&dialog_of(LIMIT_DIALOG_748), Some(5)).expect("断る");
        assert!(refusal.contains("usage_limit"), "{refusal}");
        assert!(
            refusal.contains("1. Stop and wait for limit to reset"),
            "選択肢を提示する: {refusal}"
        );
        assert!(
            refusal.contains("tako orchestrator respond --pane 5"),
            "respond へ誘導する: {refusal}"
        );
        // trust / bypass は tako 自身が承諾するので送信を止めない（送達フローが面倒を見る）
        let trust = r#" Quick safety check: Is this a project you created or one you trust?
 ❯ 1. Yes, I trust this folder
   2. No, exit

 Enter to confirm · Esc to cancel"#;
        assert!(dialog_send_refusal(&dialog_of(trust), Some(5)).is_none());
    }

    #[test]
    fn issue748_応答後の解消判定は選択肢構成で見る() {
        let before = dialog_of(PERMISSION_SCREEN_577);
        let same: Vec<String> = PERMISSION_SCREEN_577
            .lines()
            .map(|l| l.to_string())
            .collect();
        assert!(dialog_still_open(&same, &before), "同じダイアログは残存");
        let cleared: Vec<String> = "⏺ 実行しました\n────\n❯ \n────\n  ctx 5%"
            .lines()
            .map(|l| l.to_string())
            .collect();
        assert!(!dialog_still_open(&cleared, &before), "消えたら解消");
        // 別のダイアログへ遷移した場合も「このダイアログは解消」と扱う
        let next: Vec<String> = MODEL_SELECT_748.lines().map(|l| l.to_string()).collect();
        assert!(!dialog_still_open(&next, &before));
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
        // config_dir はプロセス共有なので、handoff ファイルを作るテスト（#749）と直列化する
        let _guard = TEST_PROJECT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                projects: None,
            },
            PaneOrigin::Mcp,
        );
        assert!(result.is_err(), "引き継ぎの材料が無ければエラー");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("引き継ぎの材料が無い"), "{err}");
        // #915: プロジェクト単位の置き場と運用メモの**両方**のパスを返す
        assert!(err.contains("projects/<project-key>.md"), "{err}");
        // #792: 書式（2 節の雛形）まで返す。ここが AI が書式を知る唯一の機会になりうる
        let lang = tako_core::i18n::lang();
        for section in [
            tako_core::handoff::HandoffSection::Knowledge,
            tako_core::handoff::HandoffSection::Runtime,
        ] {
            assert!(err.contains(section.heading(lang)), "{err}");
        }
    }

    // --- #749: 自動ハンドオフ（閾値反映 + 後任への kill 手順） ---

    /// 隔離した config_dir に handoff ファイルを置いて f を呼ぶ（実運用の設定に触らない）
    fn with_handoff_file<F: FnOnce()>(profile: &str, content: &str, f: F) {
        use crate::orchestrator;
        let _guard = TEST_PROJECT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        orchestrator::test_config_dir_override().get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("tako-dispatch-test-config-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            dir
        });
        let path = orchestrator::handoff_path(profile).expect("override 済み");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("handoff ディレクトリ");
        }
        std::fs::write(&path, content).expect("handoff ファイル");
        f();
        let _ = std::fs::remove_file(&path);
    }

    /// role 付きの master ペインを 1 つ持つ MockHost
    fn master_host(role: &str) -> (MockHost, u64) {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(pane),
                title: None,
                role: Some(role.into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        (host, pane)
    }

    #[test]
    fn handoffは後任へ旧ペインの確認とkillを指示する() {
        let profile = "_tako_749_ho_";
        with_handoff_file(
            profile,
            "## 状態\n進行中: worker A（pane 7）",
            || {
                let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
                let tab_id = host.workspace().tabs()[0].id();
                let result = dispatch(
                    &mut host,
                    Request::OrchestratorHandoff {
                        pane: Some(pane),
                        caller_role: Some(format!("master:{profile}")),
                        tab: None,
                        caller_pid: None,
                        projects: None,
                    },
                    PaneOrigin::Mcp,
                )
                .expect("handoff は成功する");

                // 退役予定のペインが応答に出る（後任の close 対象）
                assert_eq!(result["previous_master_pane_id"].as_u64(), Some(pane));
                // role / プロファイル / タブは旧 master と同一を引き継ぐ（#210 の維持）
                assert_eq!(
                    result["role"].as_str(),
                    Some(format!("orchestrator-master:{profile}").as_str())
                );
                assert_eq!(result["profile"].as_str(), Some(profile));
                let new_pane = result["new_master_pane_id"].as_u64().expect("新 master");
                assert_ne!(new_pane, pane, "新旧は別ペイン");
                assert_eq!(
                    result["new_master_tab_id"].as_u64(),
                    Some(tab_id.as_u64()),
                    "同じタブに立つ"
                );
                assert_eq!(
                    host.workspace()
                        .get_tab(tab_id)
                        .and_then(|t| t.tree().get(PaneId::from_raw(new_pane)))
                        .and_then(|p| p.role())
                        .map(str::to_string),
                    Some(format!("orchestrator-master:{profile}")),
                    "後任ペインの role も引き継がれる"
                );

                // 旧 master ペインはこの呼び出しでは閉じない（後任の起動失敗で master を失わない）
                assert!(
                    host.workspace()
                        .get_tab(tab_id)
                        .and_then(|t| t.tree().get(PaneId::from_raw(pane)))
                        .is_some(),
                    "旧 master は生きたまま"
                );

                // 後任へ送るプロンプトに handoff 内容 + 確認 → kill の手順が入る
                let prompt = host
                    .prompt_flows
                    .iter()
                    .find(|(p, _)| p.as_u64() == new_pane)
                    .map(|(_, text)| text.clone())
                    .expect("後任へのプロンプトが積まれる");
                assert!(prompt.contains("worker A（pane 7）"), "{prompt}");
                let read_at = prompt.find("tako_read_pane").expect("入力欄の確認手順");
                let close_at = prompt.find("tako_close_pane").expect("kill 手順");
                assert!(read_at < close_at, "確認より先に kill を書かない: {prompt}");
                assert!(
                    prompt.contains(&pane.to_string()),
                    "閉じる対象の pane ID が入る: {prompt}"
                );
            },
        );
    }

    // --- #792: 引き継ぎファイルの書式（新書式 / 旧書式の後方互換）---

    /// 後任へ積まれたプロンプトを取り出す
    fn successor_prompt_of(host: &MockHost, new_pane: u64) -> String {
        host.prompt_flows
            .iter()
            .find(|(p, _)| p.as_u64() == new_pane)
            .map(|(_, text)| text.clone())
            .expect("後任へのプロンプトが積まれる")
    }

    /// master ペインから handoff を実行して (応答, 後任プロンプト) を返す
    fn run_handoff(profile: &str) -> (Value, String) {
        let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
        let result = dispatch(
            &mut host,
            Request::OrchestratorHandoff {
                pane: Some(pane),
                caller_role: Some(format!("master:{profile}")),
                tab: None,
                caller_pid: None,
                projects: None,
            },
            PaneOrigin::Mcp,
        )
        .expect("handoff は成功する");
        let new_pane = result["new_master_pane_id"].as_u64().expect("新 master");
        let prompt = successor_prompt_of(&host, new_pane);
        (result, prompt)
    }

    /// 表示言語に依存しない見出し（判定側の定数をそのまま使う）
    fn heading_now(section: tako_core::handoff::HandoffSection) -> &'static str {
        section.heading(tako_core::i18n::lang())
    }

    #[test]
    fn handoffは新書式の2節を認識して後任へ扱いを伝える() {
        let profile = "_tako_792_new_";
        let content = "# master 引き継ぎ\n\n\
                       ## 知識（マシン非依存）\n\
                       - 方針: 検証は隔離 data dir で行う\n\n\
                       ## 実行状態（このマシン限定）\n\
                       - worker A: pane 7（#792 の実装）\n";
        with_handoff_file(profile, content, || {
            let (result, prompt) = run_handoff(profile);
            // 機械可読な書式判定（言語非依存）
            assert_eq!(result["handoff_format"].as_str(), Some("sectioned"));
            assert_eq!(
                result["handoff_sections"].as_array().map(Vec::len),
                Some(2),
                "知識 / 実行状態の 2 節: {result}"
            );
            // 内容は全文そのまま渡る（節に切って渡すと認識漏れが黙って落ちる）
            assert!(
                prompt.contains("- 方針: 検証は隔離 data dir で行う"),
                "{prompt}"
            );
            assert!(
                prompt.contains("- worker A: pane 7（#792 の実装）"),
                "{prompt}"
            );
            // 節ごとの扱いが説明される（実行状態は実態で確認）
            assert!(
                prompt.contains(heading_now(tako_core::handoff::HandoffSection::Runtime)),
                "{prompt}"
            );
            assert!(
                prompt.contains(heading_now(tako_core::handoff::HandoffSection::Knowledge)),
                "{prompt}"
            );
            // #749 の手順（確認 → kill）は書式に関係なく入る
            let read_at = prompt.find("tako_read_pane").expect("入力欄の確認手順");
            let close_at = prompt.find("tako_close_pane").expect("kill 手順");
            assert!(read_at < close_at, "{prompt}");
        });
    }

    /// **後方互換の核**: 節分離前のファイル（pane / tab 参照が本文に混在）でも
    /// 従来どおり全文が後任へ渡り、#749 の手順もそのまま入る
    #[test]
    fn handoffは旧書式のファイルでも従来どおり動く() {
        let profile = "_tako_792_legacy_";
        let content = "# master (default プロファイル) 引き継ぎ\n\n\
                       ## 【サンプル案件 移行】担当 master（tab 136 / pane 884）\n\
                       - 進行中: 客の追加 5 点\n\n\
                       ## 残キュー（優先順）\n\
                       - #801 の残件を Issue 化\n";
        with_handoff_file(profile, content, || {
            let (result, prompt) = run_handoff(profile);
            assert_eq!(result["handoff_format"].as_str(), Some("legacy"));
            assert_eq!(
                result["handoff_sections"].as_array().map(Vec::len),
                Some(0),
                "旧書式は節を持たない: {result}"
            );
            // 全文が 1 文字も欠けずに渡る
            assert!(
                prompt.contains(content.trim()),
                "旧書式の全文が渡らなかった: {prompt}"
            );
            // 従来の手順（#749 の不変条件）は維持
            let read_at = prompt.find("tako_read_pane").expect("入力欄の確認手順");
            let close_at = prompt.find("tako_close_pane").expect("kill 手順");
            assert!(read_at < close_at, "{prompt}");
            // 次の更新で新書式へ書き直す指示が付く（自然な移行の駆動源）
            assert!(
                prompt.contains(heading_now(tako_core::handoff::HandoffSection::Knowledge))
                    && prompt.contains(heading_now(tako_core::handoff::HandoffSection::Runtime)),
                "書き直し先の見出しが案内されていない: {prompt}"
            );
        });
    }

    /// master 自身が「自分のファイルが新書式か」を確認できる（#792）
    #[test]
    fn selfが引き継ぎファイルの書式を返す() {
        let profile = "_tako_792_self_";
        with_handoff_file(
            profile,
            "## 知識（マシン非依存）\n- 方針\n",
            || {
                let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
                let result = dispatch(
                    &mut host,
                    Request::OrchestratorSelf {
                        pane: Some(pane),
                        caller_role: Some(format!("master:{profile}")),
                        caller_pid: None,
                    },
                    PaneOrigin::Mcp,
                )
                .unwrap();
                assert_eq!(result["handoff_exists"].as_bool(), Some(true));
                assert_eq!(result["handoff_format"].as_str(), Some("sectioned"));
                assert_eq!(
                    result["handoff_sections"].as_array().map(Vec::len),
                    Some(1),
                    "知識節だけ: {result}"
                );
            },
        );
    }

    /// ファイル不在は「旧書式」と混ぜない（null で「まだ書いていない」を表す）
    #[test]
    fn selfは引き継ぎファイル不在で書式をnullにする() {
        let profile = "_tako_792_none_";
        // with_handoff_file は作ってしまうので、作らずに config_dir だけ揃える
        let _guard = TEST_PROJECT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::orchestrator::test_config_dir_override().get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("tako-dispatch-test-config-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            dir
        });
        let path = crate::orchestrator::handoff_path(profile).expect("override 済み");
        let _ = std::fs::remove_file(&path);

        let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
        let result = dispatch(
            &mut host,
            Request::OrchestratorSelf {
                pane: Some(pane),
                caller_role: Some(format!("master:{profile}")),
                caller_pid: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(result["handoff_exists"].as_bool(), Some(false));
        assert!(result["handoff_format"].is_null(), "{result}");
        assert!(result["handoff_sections"].is_null(), "{result}");
    }

    #[test]
    fn handoffはmaster以外のペインをkill対象にしない() {
        let profile = "_tako_749_nk_";
        with_handoff_file(profile, "state", || {
            // role 無しのユーザーペインを分割元にして呼ぶ（旧 master が特定できない状況）
            let mut host = MockHost::new();
            let user_pane = host.root_pane();
            let tab_id = host.workspace().tabs()[0].id().as_u64();
            let result = dispatch(
                &mut host,
                Request::OrchestratorHandoff {
                    pane: Some(user_pane),
                    caller_role: Some(format!("master:{profile}")),
                    tab: Some(tab_id),
                    caller_pid: None,
                    projects: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("handoff 自体は成功する");
            assert!(
                result["previous_master_pane_id"].is_null(),
                "master でないペインは kill 対象にしない: {result}"
            );
            let new_pane = result["new_master_pane_id"].as_u64().unwrap();
            let prompt = host
                .prompt_flows
                .iter()
                .find(|(p, _)| p.as_u64() == new_pane)
                .map(|(_, text)| text.clone())
                .unwrap();
            assert!(!prompt.contains("tako_close_pane"), "{prompt}");
        });
    }

    // --- #761: 後任 master の起動パラメータ（モデル / effort / role env）---

    /// takodev で実際に起きた構成を最小再現したプロファイル:
    /// master は fable / xhigh、worker は opus[1m] / high。後任がどちらで立つかを測る
    fn save_761_profile(name: &str) {
        use crate::orchestrator::{AgentWorkerConfig, Profile};
        let mut p = Profile {
            model: Some("claude-fable-5-761master".into()),
            effort: "xhigh".into(),
            ..Default::default()
        };
        p.worker_agents.insert(
            "claude".into(),
            AgentWorkerConfig {
                model: Some("claude-opus-4-6-761worker[1m]".into()),
                effort: Some("high".into()),
                ..Default::default()
            },
        );
        p.save(name).expect("プロファイルの保存");
    }

    /// 後任へ積まれた起動コマンドを取り出す。
    ///
    /// #640 以降、起動コマンドは書きっぱなしの `queue_write` ではなく送達確認つきの
    /// `queue_command_flow` を通る（器が起動直後の入力を落とすため）。本文に Enter は
    /// 含まれない（分離して送る）ので、照合はコマンド本体だけを見る
    fn successor_launch_cmd(host: &MockHost, new_pane: u64) -> String {
        host.command_flows
            .iter()
            .find(|(p, _)| p.as_u64() == new_pane)
            .map(|(_, cmd)| cmd.clone())
            .expect("後任ペインへ起動コマンドが積まれる")
    }

    #[test]
    fn handoffの後任はmaster用のモデルとeffortで起動する() {
        let profile = "_tako_761_model_";
        with_handoff_file(profile, "state", || {
            save_761_profile(profile);
            let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
            let result = dispatch(
                &mut host,
                Request::OrchestratorHandoff {
                    pane: Some(pane),
                    caller_role: Some(format!("master:{profile}")),
                    tab: None,
                    caller_pid: None,
                    projects: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("handoff は成功する");
            let new_pane = result["new_master_pane_id"].as_u64().expect("後任ペイン");
            let cmd = successor_launch_cmd(&host, new_pane);

            // master は profile.model / profile.effort で起動する（CLI の tako master と同じ）
            assert!(
                cmd.contains("--model 'claude-fable-5-761master'"),
                "master のモデルで起動していない: {cmd}"
            );
            assert!(cmd.contains("--effort xhigh"), "{cmd}");
            // worker 用の解決（worker_agents.claude）が混ざらない = #761 バグ 1 の回帰検査
            assert!(
                !cmd.contains("761worker"),
                "worker 用モデルで起動している: {cmd}"
            );
            assert!(!cmd.contains("--effort high"), "{cmd}");
            // master system prompt が付く（worker 用コマンド構築では付いていなかった）
            assert!(
                cmd.contains("--append-system-prompt-file '"),
                "master の system prompt が付いていない: {cmd}"
            );
            assert!(
                cmd.contains(&format!("_system_prompt_{profile}.md")),
                "{cmd}"
            );

            let _ = std::fs::remove_file(
                crate::orchestrator::profiles_dir()
                    .expect("override 済み")
                    .join(format!("{profile}.yaml")),
            );
        });
    }

    #[test]
    fn handoffの後任のrole_envはmaster形式でselfが同じプロファイルを返す() {
        let profile = "_tako_761_role_";
        with_handoff_file(profile, "state", || {
            save_761_profile(profile);
            let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
            let result = dispatch(
                &mut host,
                Request::OrchestratorHandoff {
                    pane: Some(pane),
                    caller_role: Some(format!("master:{profile}")),
                    tab: None,
                    caller_pid: None,
                    projects: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("handoff は成功する");
            let new_pane = result["new_master_pane_id"].as_u64().expect("後任ペイン");
            let cmd = successor_launch_cmd(&host, new_pane);

            // 起動コマンドが注入する TAKO_ORCHESTRATOR_ROLE を**コマンド文字列から取り出す**
            let role_env = cmd
                .split("TAKO_ORCHESTRATOR_ROLE='")
                .nth(1)
                .and_then(|rest| rest.split('\'').next())
                .expect("role env が注入されている")
                .to_string();
            assert_eq!(
                role_env,
                format!("master:{profile}"),
                "env 用 role は master:<profile> 形式（#761 バグ 2）: {cmd}"
            );
            // 表示用ラベルは従来どおり orchestrator-master:<profile>（両者を混ぜない）
            assert_eq!(
                result["role"].as_str(),
                Some(format!("orchestrator-master:{profile}").as_str())
            );

            // その env をそのまま caller_role にして self を引く = 後任が実際にたどる経路
            let self_result = dispatch(
                &mut host,
                Request::OrchestratorSelf {
                    pane: Some(new_pane),
                    caller_role: Some(role_env),
                    caller_pid: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("後任の self は成功する");
            assert_eq!(
                self_result["profile"].as_str(),
                Some(profile),
                "後任の self が default に落ちている: {self_result}"
            );
            assert!(
                self_result["handoff_path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with(&format!("{profile}.md"))),
                "handoff_path が引き継がれていない: {self_result}"
            );
            assert_eq!(self_result["pane_id"].as_u64(), Some(new_pane));

            let _ = std::fs::remove_file(
                crate::orchestrator::profiles_dir()
                    .expect("override 済み")
                    .join(format!("{profile}.yaml")),
            );
        });
    }

    /// #547 の規則（master_account が master の CLAUDE_CONFIG_DIR を決める）が
    /// 起動経路の差し替え後も維持されていること
    #[test]
    fn handoffの後任はmaster_accountを反映する() {
        let profile = "_tako_761_acct_";
        with_handoff_file(profile, "state", || {
            use crate::orchestrator::{AccountEntry, AccountsConfig, Profile};
            let mut accounts = AccountsConfig::load().unwrap_or(AccountsConfig {
                accounts: Default::default(),
            });
            accounts.accounts.insert(
                "_tako_761_univ_".into(),
                AccountEntry {
                    config_dir: Some("/tmp/_tako_761_cfg_".into()),
                    ..Default::default()
                },
            );
            accounts.save().expect("accounts.yaml の保存");
            let p = Profile {
                master_account: Some("_tako_761_univ_".into()),
                ..Default::default()
            };
            p.save(profile).expect("プロファイルの保存");

            let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
            let result = dispatch(
                &mut host,
                Request::OrchestratorHandoff {
                    pane: Some(pane),
                    caller_role: Some(format!("master:{profile}")),
                    tab: None,
                    caller_pid: None,
                    projects: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("handoff は成功する");
            let new_pane = result["new_master_pane_id"].as_u64().expect("後任ペイン");
            let cmd = successor_launch_cmd(&host, new_pane);
            assert!(
                // 前置きの構文はシェルの方言で変わる（#867）。config dir が
                // コマンド行で注入されていることを見る
                cmd.contains("CLAUDE_CONFIG_DIR") && cmd.contains("/tmp/_tako_761_cfg_"),
                "master_account の config dir が反映されていない: {cmd}"
            );

            // 後始末。**一時ディレクトリ配下であることを確認してから**消す
            let dir = crate::orchestrator::config_dir().expect("override 済み");
            assert!(
                dir.starts_with(std::env::temp_dir()),
                "テストの config_dir が一時ディレクトリ配下でない: {}",
                dir.display()
            );
            let _ = std::fs::remove_file(dir.join("accounts.yaml"));
            let _ = std::fs::remove_file(dir.join("profiles").join(format!("{profile}.yaml")));
        });
    }

    /// caller_role にペインの role ラベル（表示用）が来る内部呼び出し
    /// （`tako_stale_binary restart` の master 経路）でも default に落ちない
    #[test]
    fn handoffは表示用roleのcaller_roleでもプロファイルを解決する() {
        let profile = "_tako_761_disp_";
        with_handoff_file(profile, "state", || {
            let (mut host, pane) = master_host(&format!("orchestrator-master:{profile}"));
            let result = dispatch(
                &mut host,
                Request::OrchestratorHandoff {
                    pane: Some(pane),
                    caller_role: Some(format!("orchestrator-master:{profile}")),
                    tab: None,
                    caller_pid: None,
                    projects: None,
                },
                PaneOrigin::Mcp,
            )
            .expect("handoff は成功する");
            assert_eq!(result["profile"].as_str(), Some(profile), "{result}");
        });
    }

    #[test]
    fn selfの閾値はプロファイル設定を反映する() {
        use crate::orchestrator;
        with_handoff_file("_tako_749_", "state", || {
            let (mut host, pane) = master_host("orchestrator-master:_tako_749_");
            let call = |host: &mut MockHost| {
                dispatch(
                    host,
                    Request::OrchestratorSelf {
                        pane: Some(pane),
                        caller_role: Some("master:_tako_749_".into()),
                        caller_pid: None,
                    },
                    PaneOrigin::Mcp,
                )
                .unwrap()
            };

            // 未設定は既定 60
            let before = call(&mut host);
            assert_eq!(before["ctx_threshold"].as_u64(), Some(60));
            assert_eq!(before["auto_handoff"].as_bool(), Some(true));
            assert!(before["handoff_exists"].as_bool().unwrap_or(false));

            // プロファイルで 50 に下げると self にも発動判定にも反映される
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some("_tako_749_".into()),
                ctx_threshold: Some(50),
                auto_handoff: Some(false),
                ..Default::default()
            })
            .expect("set は成功する");
            let after = call(&mut host);
            assert_eq!(after["ctx_threshold"].as_u64(), Some(50));
            assert_eq!(after["ctx_threshold_source"].as_str(), Some("profile"));
            assert_eq!(after["auto_handoff"].as_bool(), Some(false));

            // 同じ設定が tako-core の発動判定へそのまま渡る（55% は 50 で発動 / 60 では未発動）
            let input = |threshold: u32| tako_core::handoff::NudgeInput {
                auto_handoff: true,
                ctx_percent: Some(55),
                threshold,
                pane_age: tako_core::handoff::NUDGE_GRACE * 2,
                since_last_nudge: None,
                sent_count: 0,
                handoff_started: false,
            };
            let effective = after["ctx_threshold"].as_u64().unwrap() as u32;
            assert!(tako_core::handoff::nudge_decision(&input(effective)).should_send());
            assert!(!tako_core::handoff::nudge_decision(&input(60)).should_send());

            // 後始末（プロファイルファイルを残さない）
            let _ = dispatch_orchestrator_profiles(ProfilesParams {
                action: "delete".into(),
                name: Some("_tako_749_".into()),
                ..Default::default()
            });
            let _ = orchestrator::handoff_path("_tako_749_");
        });
    }

    #[test]
    fn プロファイルのctx閾値は範囲外を拒否する() {
        for bad in [0u32, 49, 61, 100] {
            let err = dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some("_tako_749_range_".into()),
                ctx_threshold: Some(bad),
                ..Default::default()
            })
            .expect_err("範囲外は拒否する");
            assert!(err.to_string().contains("50〜60"), "{err}");
        }
    }

    /// #981: サンドボックス解除は CLI / MCP / GUI が同じ dispatch を通り、
    /// 既定は false・set した値が show へ往復する（開発不変条件の 1:1）
    #[test]
    fn issue981_サンドボックス解除は既定offで往復する() {
        let profile = "_tako_981_";
        let show = || {
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "show".into(),
                name: Some(profile.into()),
                ..Default::default()
            })
            .expect("show は成功する")
        };
        let set = |bypass: Option<bool>, master_agent: Option<String>| {
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some(profile.into()),
                bypass_sandbox: bypass,
                master_agent,
                ..Default::default()
            })
            .expect("set は成功する")
        };

        // 作った直後は false（既定が安全側）。値は常に応答へ載る
        set(None, Some("codex".into()));
        assert_eq!(show()["bypass_sandbox"].as_bool(), Some(false));
        // codex を使うのに解除していないと「skip_permissions が効かない」旨の警告が出る
        let warnings = show()["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|w| w.as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            warnings.contains("bypass_sandbox"),
            "効かない設定を黙って抱えさせない: {warnings}"
        );

        // 明示 opt-in → 往復する（ファイルにも書かれる）
        assert_eq!(
            set(Some(true), None)["bypass_sandbox"].as_bool(),
            Some(true)
        );
        assert_eq!(show()["bypass_sandbox"].as_bool(), Some(true));
        let path = show()["path"].as_str().expect("path が返る").to_string();
        assert!(std::fs::read_to_string(&path)
            .expect("読める")
            .contains("bypass_sandbox: true"));
        // false へ戻せる（clear は要らない = bool 1 つで表せる方針）
        assert_eq!(
            set(Some(false), None)["bypass_sandbox"].as_bool(),
            Some(false)
        );

        let _ = dispatch_orchestrator_profiles(ProfilesParams {
            action: "delete".into(),
            name: Some(profile.into()),
            ..Default::default()
        });
    }

    /// #983: agent CLI が無い環境の spawn は**ペインを作る前に**分類済みエラーで落ちる。
    /// 「spawn は成功したと言われたのに worker が何もしない」= 無言死を作らないこと
    #[test]
    fn issue983_cli不在のspawnはペインを作らずに落ちる() {
        with_test_project(|| {
            // 代表 1 系統だけを dispatch 層で確かめる（agent ごとの文言は
            // `agent_cli` の unit テストが 3 系統ぶん見ている）。ここで系統数ぶん
            // 回すと設定ファイルの読み書きが増え、プロセス全体の fd 数を見ている
            // `ipc::連続接続でfdが漏れない` を揺らす（既に #916 で一度踏んでいる罠）
            {
                let agent = crate::orchestrator::WorkerAgent::Codex;
                let _guard = crate::orchestrator::agent_cli::test_force_missing(&[agent]);
                let mut host = MockHost::new();
                let master = host.root_pane();
                let panes_before = host
                    .workspace()
                    .tabs()
                    .iter()
                    .flat_map(|t| t.tree().panes())
                    .count();
                let err = dispatch_orchestrator_spawn(
                    &mut host,
                    PaneOrigin::Mcp,
                    SpawnParams {
                        project: TEST_PROJECT,
                        prompt: "cli missing test",
                        label: None,
                        model: None,
                        effort: None,
                        pane: Some(master),
                        tab: None,
                        caller_role: None,
                        agent: Some(agent.as_str()),
                        caller_pid: None,
                        task_type: None,
                        account: None,
                        limit_resume: None,
                    },
                )
                .expect_err("CLI が無ければ spawn は失敗する");
                let msg = err.to_string();
                assert!(
                    msg.contains(agent.as_str()),
                    "どの CLI が無いのか名指しすること: {msg}"
                );
                assert!(
                    msg.contains("tako setup"),
                    "次の一手が付いていること: {msg}"
                );
                let panes_after = host
                    .workspace()
                    .tabs()
                    .iter()
                    .flat_map(|t| t.tree().panes())
                    .count();
                assert_eq!(
                    panes_after,
                    panes_before,
                    "{}: 失敗した spawn がペインを残してはいけない",
                    agent.as_str()
                );
            }
        });
    }

    /// 正常系（CLI が在る）の spawn は従来どおり成功し、どの実行ファイルを
    /// 起動したかが応答に載る（#983 の可視化）
    #[test]
    fn issue983_cliが在れば従来どおりspawnできて実行ファイルが応答に載る() {
        with_test_project(|| {
            // 実探索（ログインシェルの起動）はここでは通さない。この経路の関心事は
            // 「見つかったら従来どおり spawn できて、解決したパスが応答に載る」ことで、
            // 探索そのものは agent_cli の unit テストが担保する（重い経路を本筋から
            // 外す。プロセス全体の fd 数を見る `ipc::連続接続でfdが漏れない` が
            // 一時的な子プロセスで揺れるため）
            let _guard = crate::orchestrator::agent_cli::test_force_found(&[(
                crate::orchestrator::WorkerAgent::Claude,
                "/usr/local/bin/claude",
            )]);
            let mut host = MockHost::new();
            let master = host.root_pane();
            let val = dispatch_orchestrator_spawn(
                &mut host,
                PaneOrigin::Mcp,
                SpawnParams {
                    project: TEST_PROJECT,
                    prompt: "cli present test",
                    label: None,
                    model: None,
                    effort: None,
                    pane: Some(master),
                    tab: None,
                    caller_role: None,
                    agent: None,
                    caller_pid: None,
                    task_type: None,
                    account: None,
                    limit_resume: None,
                },
            )
            .expect("claude が在る環境では成功する");
            assert_eq!(val["agent"].as_str(), Some("claude"));
            assert_eq!(
                val["agent_path"].as_str(),
                Some("/usr/local/bin/claude"),
                "解決した実行ファイルがそのまま応答に載る"
            );
        });
    }

    /// #822: プロファイルの limit_resume が spawn 先の worker ペインへ適用される。
    /// spawn 引数は両方向でプロファイルに勝つ（Some(false) は明示 OFF）
    #[test]
    fn issue822_プロファイルのlimit_resumeがspawn先ペインへ適用される() {
        let profile = "_tako_822_";
        let set = |limit_resume: Option<bool>, clear: bool| {
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some(profile.into()),
                limit_resume,
                clear_limit_resume: clear,
                ..Default::default()
            })
            .expect("set は成功する")
        };
        // spawn して「新ペインに適用された値」と「応答の値」を返す
        let spawn = |override_value: Option<bool>| -> (bool, bool) {
            let mut host = MockHost::new();
            let master = host.root_pane();
            let params = SpawnParams {
                project: TEST_PROJECT,
                prompt: "limit resume test",
                label: None,
                model: None,
                effort: Some("high"),
                pane: Some(master),
                tab: None,
                caller_role: Some(&format!("master:{profile}")),
                agent: None,
                caller_pid: None,
                task_type: None,
                account: None,
                limit_resume: override_value,
            };
            let val = dispatch_orchestrator_spawn(&mut host, PaneOrigin::Mcp, params)
                .expect("spawn は成功する");
            let new_pane = PaneId::from_raw(val["pane_id"].as_u64().unwrap());
            let applied = host
                .workspace()
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes())
                .find(|p| p.id() == new_pane)
                .expect("新ペインが存在する")
                .limit_autoresume();
            (applied, val["limit_resume"].as_bool().unwrap())
        };

        with_test_project(|| {
            // 既定（プロファイル未設定）は OFF = #813 のペイン単位オプトインのまま
            set(None, true);
            assert_eq!(spawn(None), (false, false));
            // プロファイル ON → spawn した worker に自動適用される
            set(Some(true), false);
            assert_eq!(spawn(None), (true, true));
            // spawn 引数で個別に打ち消せる（明示 OFF）
            assert_eq!(spawn(Some(false)), (false, false));
            // プロファイル OFF でも spawn 引数で個別に有効化できる
            set(Some(false), false);
            assert_eq!(spawn(None), (false, false));
            assert_eq!(spawn(Some(true)), (true, true));

            // 後始末（プロファイルファイルを残さない）
            let _ = dispatch_orchestrator_profiles(ProfilesParams {
                action: "delete".into(),
                name: Some(profile.into()),
                ..Default::default()
            });
        });
    }

    /// #822: set / clear / 排他エラーと、実効値の併記（GUI のトグルはこれを読む）
    #[test]
    fn issue822_limit_resumeのsetとclearと排他() {
        let name = "_tako_822_set_";
        let set = |limit_resume: Option<bool>, clear: bool| {
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some(name.into()),
                limit_resume,
                clear_limit_resume: clear,
                ..Default::default()
            })
        };

        // 未設定 = 生値は出さず実効値だけ false
        let v = set(None, false).expect("set は成功する");
        assert!(v.get("limit_resume").is_none(), "{v}");
        assert_eq!(v["resolved_limit_resume"].as_bool(), Some(false), "{v}");

        // ON → 生値と実効値の両方が true
        let v = set(Some(true), false).expect("set は成功する");
        assert_eq!(v["limit_resume"].as_bool(), Some(true), "{v}");
        assert_eq!(v["resolved_limit_resume"].as_bool(), Some(true), "{v}");

        // clear → 生値が消え実効値は false へ戻る
        let v = set(None, true).expect("clear は成功する");
        assert!(v.get("limit_resume").is_none(), "{v}");
        assert_eq!(v["resolved_limit_resume"].as_bool(), Some(false), "{v}");

        // 同時指定は拒否（auto_handoff と同じ規約）
        let err = set(Some(true), true).expect_err("同時指定は拒否する");
        assert!(err.to_string().contains("同時に指定できない"), "{err}");

        let _ = dispatch_orchestrator_profiles(ProfilesParams {
            action: "delete".into(),
            name: Some(name.into()),
            ..Default::default()
        });
    }

    /// #822: solo は worker を spawn しないので、ON にしても効く先が無い。
    /// 黙って死んだ設定にせず警告として見せる（GUI / CLI / MCP 共通）
    #[test]
    fn issue822_soloプロファイルのlimit_resumeは効かない旨を警告する() {
        let name = "_tako_822_solo_";
        let set = |kind: &str, on: bool| {
            dispatch_orchestrator_profiles(ProfilesParams {
                action: "set".into(),
                name: Some(name.into()),
                kind: Some(kind.into()),
                limit_resume: Some(on),
                ..Default::default()
            })
            .expect("set は成功する")
        };
        let warned = |v: &Value| {
            v["warnings"]
                .as_array()
                .map(|w| {
                    w.iter()
                        .any(|x| x.as_str().is_some_and(|s| s.contains("limit_resume")))
                })
                .unwrap_or(false)
        };

        // solo + ON は警告する
        assert!(warned(&set("solo", true)), "solo ON で警告が出ない");
        // solo + OFF は警告しない（効かない設定を持っていない）
        assert!(!warned(&set("solo", false)), "solo OFF で警告が出る");
        // master + ON は警告しない（worker へ効く）
        assert!(!warned(&set("master", true)), "master ON で警告が出る");

        for kind in ["solo", "master"] {
            let _ = dispatch_orchestrator_profiles(ProfilesParams {
                action: "delete".into(),
                name: Some(name.into()),
                kind: Some(kind.into()),
                ..Default::default()
            });
        }
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
                cwd: None,
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
                cwd: None,
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

    // ---- Issue #567: ResolvePane（stale な TAKO_PANE_ID の救済） ----

    #[test]
    fn resolve_pane_現存ペインはそのまま返る() {
        let mut host = MockHost::new();
        let actual = host.root_pane();
        let result = dispatch(
            &mut host,
            Request::ResolvePane {
                pane: Some(actual),
                caller_pid: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["pane"].as_u64(), Some(actual));
        assert_eq!(result["method"], "pane");
        assert_eq!(result["stale"], false);
    }

    #[test]
    fn resolve_pane_stale_mapで新idへ読み替える() {
        let mut host = MockHost::new();
        let actual = host.root_pane();
        let stale_id = 99999_u64;
        host.stale_pane_map
            .insert(PaneId::from_raw(stale_id), PaneId::from_raw(actual));

        let result = dispatch(
            &mut host,
            Request::ResolvePane {
                pane: Some(stale_id),
                caller_pid: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            result["pane"].as_u64(),
            Some(actual),
            "stale ID が現世代のペインへ読み替わる"
        );
        assert_eq!(result["method"], "stale");
        assert_eq!(result["stale"], true);
        assert_eq!(result["requested"].as_u64(), Some(stale_id));
    }

    #[test]
    fn resolve_pane_解決不能でもエラーにせずnullを返す() {
        let mut host = MockHost::new();
        // stale map に登録の無い旧 ID（#567 の実事象: ペイン 305 は既に存在しない）
        let result = dispatch(
            &mut host,
            Request::ResolvePane {
                pane: Some(305),
                caller_pid: None,
            },
            PaneOrigin::Cli,
        )
        .expect("解決できなくてもエラーにしない（呼び出し元がフォールバックを選ぶ）");
        assert!(result["pane"].is_null());
        assert!(result["tab"].is_null());
        assert!(result["method"].is_null());
        assert_eq!(result["stale"], true);
    }

    #[test]
    fn resolve_pane_pane未指定は解決不能かつstaleではない() {
        let mut host = MockHost::new();
        let result = dispatch(
            &mut host,
            Request::ResolvePane {
                pane: None,
                caller_pid: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["pane"].is_null());
        assert_eq!(
            result["stale"], false,
            "自称 ID が無いのだから「古い」わけではない"
        );
    }

    #[test]
    fn resolve_pane_lenient_pid解決が環境変数より優先される() {
        let host = MockHost::new();
        let actual = host.root_pane();
        // pane env は現存する別 ID を騙る（ID 再利用で他人のペインを掴む事故の再現）。
        // pid 祖先辿りが実ペインを返せばそちらが勝つ
        let resolved = resolve_pane_lenient(&host, Some(4242), Some(1234), |pid, _backends| {
            assert_eq!(pid, 1234);
            Some(actual)
        })
        .expect("pid で解決できる");
        assert_eq!(resolved.0, PaneResolveMethod::Pid);
        assert_eq!(resolved.2.as_u64(), actual);
    }

    #[test]
    fn resolve_pane_lenient_pid不明ならpane指定へ落ちる() {
        let host = MockHost::new();
        let actual = host.root_pane();
        let resolved = resolve_pane_lenient(&host, Some(actual), Some(1234), |_, _| None)
            .expect("pane 指定で解決できる");
        assert_eq!(resolved.0, PaneResolveMethod::Pane);
        assert_eq!(resolved.2.as_u64(), actual);
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
                limit_resume: None,
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

    /// #694: UI 表示モードの status / set / toggle と、ペイン単位の揮発解除
    #[test]
    fn ui_modeのstatus_set_toggleが機能する() {
        use tako_core::ui_mode::UiMode;
        let mut host = MockHost::new();
        let ui_mode = |host: &mut MockHost, action: Option<&str>, mode: Option<&str>| {
            dispatch(
                host,
                Request::UiMode {
                    action: action.map(str::to_string),
                    mode: mode.map(str::to_string),
                    pane: None,
                },
                PaneOrigin::Cli,
            )
        };
        // status: 既定は terminal（既存ユーザーの表示は変わらない）
        let v = ui_mode(&mut host, None, None).unwrap();
        assert_eq!(v["ui_mode"], "terminal");
        assert_eq!(v["available"].as_array().unwrap().len(), 2);
        assert_eq!(v["released_panes"].as_array().unwrap().len(), 0);
        // set gui → host へ反映
        let v = ui_mode(&mut host, Some("set"), Some("gui")).unwrap();
        assert_eq!(v["ui_mode"], "gui");
        assert_eq!(host.ui_mode, UiMode::Gui);
        // toggle → terminal へ戻る
        let v = ui_mode(&mut host, Some("toggle"), None).unwrap();
        assert_eq!(v["ui_mode"], "terminal");
        assert_eq!(host.ui_mode, UiMode::Terminal);
        // set の不明 mode / mode 無し / 不明 action はエラー
        assert!(ui_mode(&mut host, Some("set"), Some("simple")).is_err());
        assert!(ui_mode(&mut host, Some("set"), None).is_err());
        assert!(ui_mode(&mut host, Some("kiosk"), None).is_err());
    }

    /// #694: スターターの「コマンド入力へ」= ペイン単位の揮発解除も
    /// dispatch から操作できる（開発不変条件: UI でできることは AI からもできる）
    #[test]
    fn ui_modeのペイン単位解除が機能する() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let release = |host: &mut MockHost, action: &str, pane: Option<u64>| {
            dispatch(
                host,
                Request::UiMode {
                    action: Some(action.into()),
                    mode: None,
                    pane,
                },
                PaneOrigin::Cli,
            )
        };
        let v = release(&mut host, "release", Some(pane)).unwrap();
        assert_eq!(v["pane"], pane);
        assert_eq!(v["released"], true);
        assert_eq!(v["released_panes"], serde_json::json!([pane]));
        assert!(host.starter_released.contains(&pane));
        // restore で戻る
        let v = release(&mut host, "restore", Some(pane)).unwrap();
        assert_eq!(v["released"], false);
        assert_eq!(v["released_panes"].as_array().unwrap().len(), 0);
        assert!(host.starter_released.is_empty());
        // 対象ペインを解決できないときはエラー（黙って別ペインへ効かせない）
        assert!(release(&mut host, "release", Some(9999)).is_err());
        assert!(release(&mut host, "release", None).is_err());
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

    /// #1006 用のリクエスト組み立て（省略値だらけなのでテストの意図が読めるように）
    #[cfg(test)]
    fn open_remote_req(
        target: Option<tako_core::remote_open::RemoteOpenTarget>,
        pane: Option<u64>,
    ) -> Request {
        Request::OpenRemote {
            host: "nonexistent-host".into(),
            focus: Some(true),
            remote_dir: None,
            target,
            pane,
            tab: None,
            direction: None,
        }
    }

    #[test]
    fn open_remote_の既定は現在タブへの新ペイン() {
        // #1006: 既定の開き先を「新タブ」から「いまのタブへ新ペイン」へ変えた。
        // タブが増えず、そのタブのペインが 1 枚増えるのが正
        let mut host = MockHost::new();
        let tabs_before = host.workspace().tabs().len();
        let tab_id = host.workspace().active_tab_id();
        let panes_before = host
            .workspace()
            .get_tab(tab_id)
            .unwrap()
            .tree()
            .panes()
            .len();

        let result = dispatch(&mut host, open_remote_req(None, None), PaneOrigin::Cli).unwrap();

        assert_eq!(
            host.workspace().tabs().len(),
            tabs_before,
            "タブは増えない（#1006 の要望そのもの）"
        );
        assert_eq!(result["tab"].as_u64(), Some(tab_id.as_u64()));
        assert_eq!(result["target"], "split");
        let panes_after = host
            .workspace()
            .get_tab(tab_id)
            .unwrap()
            .tree()
            .panes()
            .len();
        assert_eq!(
            panes_after,
            panes_before + 1,
            "同じタブにペインが 1 枚生える"
        );
        let new_pane = result["pane"].as_u64().unwrap();
        assert!(
            host.attached.contains(&new_pane),
            "新ペインへ ssh のセッションが張られる"
        );
    }

    #[test]
    fn open_remote_はtarget_tabで新タブを作成() {
        // 従来動作（#20）は target=tab で明示すれば残る
        let mut host = MockHost::new();
        let tabs_before = host.workspace().tabs().len();
        let result = dispatch(
            &mut host,
            open_remote_req(Some(tako_core::remote_open::RemoteOpenTarget::Tab), None),
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.workspace().tabs().len(), tabs_before + 1);
        assert!(result["tab"].as_u64().is_some());
        assert!(result["pane"].as_u64().is_some());
        assert_eq!(result["target"], "tab");
    }

    #[test]
    fn listにcan_sshが載り_open_remoteの可否と一致する() {
        // #1080: リモート（スマホ）は判定材料（セッション・器・OSC 133・role）を
        // 持たないので、`list` に載った答えをそのまま読む。**その答えが
        // 実際の OpenRemote の可否と食い違わない**ことをここで縛る
        // （食い違うと「メニューに出たのに押すと断られる」= 受け入れ条件 ③ が壊れる）
        let mut host = MockHost::new();
        let tab_id = host.workspace().active_tab_id();
        let pane_id = host.workspace().get_tab(tab_id).unwrap().tree().focused();

        // ① セッションが無いペイン（プレビュー相当）は理由つきで false
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let can = &list["tabs"][0]["panes"][0]["can_ssh"];
        assert_eq!(can["ok"], false, "セッションが無ければ SSH 化できない");
        assert_eq!(can["reason"], "no_session");
        assert!(
            can["note"].as_str().unwrap_or("").contains("target=split"),
            "断るなら次の一手を必ず添える: {can}"
        );
        // 実際に呼んでも断られる（判定と実行が一致する）
        assert!(
            dispatch(
                &mut host,
                open_remote_req(
                    Some(tako_core::remote_open::RemoteOpenTarget::Pane),
                    Some(pane_id.as_u64()),
                ),
                PaneOrigin::Cli,
            )
            .is_err(),
            "list が false と言ったなら実行も断られる"
        );

        // ② 素のシェルのセッションを張ると true になり、実行も通る
        let (session, _rx) = TerminalSession::spawn(80, 24, SpawnOptions::default())
            .expect("既定シェルの PTY を張れる");
        host.sessions.insert(pane_id.as_u64(), session);
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["panes"][0]["can_ssh"]["ok"], true);
        assert!(
            list["tabs"][0]["panes"][0]["can_ssh"]["reason"].is_null(),
            "通るときは理由を出さない"
        );
        assert!(dispatch(
            &mut host,
            open_remote_req(
                Some(tako_core::remote_open::RemoteOpenTarget::Pane),
                Some(pane_id.as_u64()),
            ),
            PaneOrigin::Cli,
        )
        .is_ok());

        // ③ role が付く（AI エージェントのペイン）と false へ戻る
        let ws = host.workspace_mut();
        let tab = ws.get_tab_mut(tab_id).unwrap();
        tab.tree_mut()
            .get_mut(pane_id)
            .unwrap()
            .set_role(Some("orchestrator-worker:1".to_string()));
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let can = &list["tabs"][0]["panes"][0]["can_ssh"];
        assert_eq!(can["ok"], false, "エージェントのペインは対象外");
        assert_eq!(can["reason"], "agent_role");
    }

    #[test]
    fn open_remote_はtarget_paneで既存ペインをssh化する() {
        // #1006 の本題: ペインを増やさず・タブも増やさず・**pane ID も変えず**に
        // そのペインへ ssh の行を送る（素のシェルなので失敗すればプロンプトへ戻る）
        let mut host = MockHost::new();
        let tab_id = host.workspace().active_tab_id();
        let pane_id = host.workspace().get_tab(tab_id).unwrap().tree().focused();
        // 素のシェルのセッションを実際に張る（判定材料がセッション由来のため）
        let (session, _rx) = TerminalSession::spawn(80, 24, SpawnOptions::default())
            .expect("既定シェルの PTY を張れる");
        host.sessions.insert(pane_id.as_u64(), session);

        let tabs_before = host.workspace().tabs().len();
        let panes_before = host
            .workspace()
            .get_tab(tab_id)
            .unwrap()
            .tree()
            .panes()
            .len();

        let result = dispatch(
            &mut host,
            open_remote_req(
                Some(tako_core::remote_open::RemoteOpenTarget::Pane),
                Some(pane_id.as_u64()),
            ),
            PaneOrigin::Cli,
        )
        .unwrap();

        assert_eq!(
            result["pane"].as_u64(),
            Some(pane_id.as_u64()),
            "pane ID が変わらない（受け入れ条件）"
        );
        assert_eq!(result["target"], "pane");
        assert_eq!(host.workspace().tabs().len(), tabs_before, "タブは増えない");
        assert_eq!(
            host.workspace()
                .get_tab(tab_id)
                .unwrap()
                .tree()
                .panes()
                .len(),
            panes_before,
            "ペインも増えない"
        );
        assert!(
            host.attached.is_empty(),
            "セッションを張り直さない（実行中のシェルを殺さない）"
        );
        // 送達確認つきの経路（#640）へ ssh の行が積まれる
        let (flow_pane, line) = host
            .command_flows
            .first()
            .cloned()
            .expect("ssh の行が積まれる");
        assert_eq!(flow_pane, pane_id);
        assert!(line.contains("nonexistent-host"), "宛先が入る: {line}");
        assert_eq!(
            line, result["command"],
            "応答の command と実際に送る行は同一（AI が何が起きるかを読める）"
        );
    }

    #[test]
    fn open_remote_のtarget_paneは素のシェルでないペインを断る() {
        // セッションが無いペイン（プレビュー等）は理由 + 次の一手つきで断る。
        // **黙って新タブへ逃げない**（何が起きたか分からなくなる）
        let mut host = MockHost::new();
        let tab_id = host.workspace().active_tab_id();
        let pane_id = host.workspace().get_tab(tab_id).unwrap().tree().focused();
        let tabs_before = host.workspace().tabs().len();
        let err = dispatch(
            &mut host,
            open_remote_req(
                Some(tako_core::remote_open::RemoteOpenTarget::Pane),
                Some(pane_id.as_u64()),
            ),
            PaneOrigin::Cli,
        )
        .expect_err("セッションが無いので断る");
        let msg = format!("{err}");
        assert!(msg.contains("target=split"), "次の一手を添える: {msg}");
        assert_eq!(host.workspace().tabs().len(), tabs_before);
        assert!(host.command_flows.is_empty(), "何も送らない");
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
                cwd: None,
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
                cwd: None,
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
                cwd: None,
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
                cwd: None,
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

    /// #916: 壊れた JSON は**上書きしない**。旧実装は unwrap_or_default で
    /// 空 map から書き直し、利用者の MCP 登録・信頼済みフォルダ・履歴を消していた
    #[test]
    fn setup_mcp_direct_壊れたjsonは書き換えない() {
        let dir = mcp_test_dir("direct-broken");
        let target = dir.join(".mcp.json");
        let broken = "{ \"mcpServers\": { \"other\": ";
        std::fs::write(&target, broken).unwrap();
        let scope = McpScope::Project(dir.clone());
        let err =
            setup_mcp_direct("/usr/local/bin/tako", &scope).expect_err("壊れた JSON では中止する");
        assert!(
            format!("{err:?}").contains("解釈できない"),
            "理由を返す: {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            broken,
            "1 バイトも触らない"
        );
    }

    /// 中身が空のファイル（`touch` しただけ）は失うものが無いので新規扱いにする
    #[test]
    fn setup_mcp_direct_空ファイルは新規扱い() {
        let dir = mcp_test_dir("direct-empty");
        let target = dir.join(".mcp.json");
        std::fs::write(&target, "  \n").unwrap();
        let scope = McpScope::Project(dir.clone());
        setup_mcp_direct("/usr/local/bin/tako", &scope).expect("空なら登録できる");
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["tako"]["command"],
            "/usr/local/bin/tako"
        );
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

    // --- #898: 解決順を純粋関数で固定する（**macOS 上から Windows の形も検査できる**）---

    /// 与えた集合だけを「実在するファイル」と見なす判定
    fn only_files(files: &[&str]) -> impl Fn(&str) -> bool + use<> {
        let owned: Vec<String> = files.iter().map(|f| (*f).to_string()).collect();
        move |p: &str| owned.iter().any(|f| f == p)
    }

    #[test]
    fn 解決順は安定パスが最優先() {
        let got = resolve_tako_binary_with(
            &only_files(&[STABLE_APP_BINARY, "/usr/local/bin/tako"]),
            &|| Some("/usr/local/bin/tako".to_string()),
            Some(std::path::Path::new("/tmp/bundle/tako-app")),
            "tako",
        );
        assert_eq!(got, STABLE_APP_BINARY);
    }

    #[test]
    fn 安定パスが無ければコマンド解決が隣より優先() {
        let got = resolve_tako_binary_with(
            &only_files(&["/tmp/bundle/tako"]),
            &|| Some("/opt/homebrew/bin/tako".to_string()),
            Some(std::path::Path::new("/tmp/bundle/tako-app")),
            "tako",
        );
        assert_eq!(got, "/opt/homebrew/bin/tako");
    }

    #[test]
    fn コマンド解決が空振りしたら実行中バイナリの隣を見る() {
        // 期待値は**同じ `join` から作る**。`Path::join` の区切りは実行中 OS のもの
        // （Windows は `\`）なので、連結後の形をリテラルで書くと実機だけ落ちる
        // （#920 と同じ型の罠。実際にこのテストで Windows 実機を 1 度落とした）
        let dir = std::path::Path::new("/tmp/tako-898-fixture");
        let exe = dir.join("tako-app");
        let sibling = dir.join("tako").display().to_string();
        let got = resolve_tako_binary_with(
            &only_files(&[sibling.as_str()]),
            &|| None,
            Some(&exe),
            "tako",
        );
        assert_eq!(got, sibling);
    }

    /// **#898 の本体**: 旧実装は隣を `tako`（拡張子なし）決め打ちで探していたので、
    /// zip 展開だけで導入した Windows では**常に空振りして裸の `tako`** へ落ちていた
    /// （スターター #694 / welcome バナー #549 が PATH 依存のコマンドを書く原因）。
    /// `EXE_SUFFIX` を渡す形にしたので `tako.exe` を見つける
    #[test]
    fn 実行ファイル名が拡張子つきかどうかで隣の見つかり方が変わる() {
        // 区切り文字は**実行中 OS のもの**を使う（`Path` は unix では `\` を区切りと
        // 見ないので、Windows 形のリテラルを macOS から検査することはできない）。
        // 期待値も同じ `join` で作るので、両プラットフォームで同じ検査になる
        let dir = std::path::Path::new("/tmp/tako-898-fixture");
        let exe = dir.join("tako-app.exe");
        let sibling = dir.join("tako.exe").display().to_string();
        let files = only_files(&[sibling.as_str()]);
        // 新: 名前に拡張子が付いているので隣が見つかる
        assert_eq!(
            resolve_tako_binary_with(&files, &|| None, Some(&exe), "tako.exe"),
            sibling
        );
        // 旧（拡張子なし決め打ち）は空振りして裸の `tako` に落ちる = 直った差分の実証。
        // Windows の隣は `tako.exe` なので、旧実装はこの枝を必ず通っていた
        assert_eq!(
            resolve_tako_binary_with(&files, &|| None, Some(&exe), "tako"),
            "tako"
        );
    }

    #[test]
    fn どれも無ければ裸のtako() {
        let got = resolve_tako_binary_with(&only_files(&[]), &|| None, None, "tako");
        assert_eq!(got, "tako");
    }

    /// 実行中の OS に合った実行ファイル名を組み立てていること
    /// （unix は拡張子なし / Windows は `.exe`）
    #[test]
    fn cli実行ファイル名はプラットフォームの拡張子を持つ() {
        let name = tako_cli_file_name();
        assert!(name.starts_with("tako"));
        assert_eq!(name, format!("tako{}", std::env::consts::EXE_SUFFIX));
        if cfg!(windows) {
            assert_eq!(name, "tako.exe");
        } else {
            assert_eq!(name, "tako");
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
    fn exit_markerは両方言の実出力から拾える() {
        // 実行ペインの唯一の契約は「マーカー行の形」。組み立て方は方言で違うのに
        // **画面に出る行は同じ**であることを、両方言の実出力の形で固定する（#875）。
        //
        // POSIX: `echo "__TAKO_EXIT=$?"` → 行末に余分な空白は無い
        assert_eq!(find_exit_marker(&["__TAKO_EXIT=7".into()]), Some(7));
        // PowerShell: `Write-Host ('__TAKO_EXIT=' + $__tako_code)`。
        // ConPTY は行を端末幅まで空白で埋めて返すことがあるので、右の空白を許す
        assert_eq!(
            find_exit_marker(&["__TAKO_EXIT=7                    ".into()]),
            Some(7)
        );
        // PowerShell の `$LASTEXITCODE` は cmdlet 失敗時に 1 を返す設計（負値は来ない）が、
        // ネイティブ exe は負値を返しうる（`exit -1` → 4294967295 ではなく -1 で表示される）
        assert_eq!(find_exit_marker(&["__TAKO_EXIT=-1".into()]), Some(-1));
        // どちらの方言でも「マーカーの後ろに数字以外」は採らない
        assert_eq!(find_exit_marker(&["__TAKO_EXIT=$__tako_code".into()]), None);
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
        // 起動コマンドの形は方言境界が決める（#875）。dispatch の責任は
        // 「ユーザーのコマンドをそのまま境界へ渡す」ことなので、境界の出力と突き合わせる
        assert_run_pane_command(cmd, r#"read "ans?input: ""#);
        // POSIX 側は #453 の回帰（program 1 語詰めは 127 即死）をここでも見えるようにする
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

    /// 実行ペインの `SpawnCommand` が「そのコマンドを境界へ通した結果」と一致することを見る。
    ///
    /// **OS ごとに期待値を書き分けない**。書き分けると Windows で決め打ちの
    /// テストが増える（作法 11）。境界そのものの出力は
    /// `platform::shell` 側の単体テストがバイト単位で固定している
    fn assert_run_pane_command(got: &SpawnCommand, command: &str) {
        // 接頭辞は `find_exit_marker` が読むのと同じ定数から採る
        // （組み立て側と読む側がずれたら実機ではなくここで落ちる）
        let want = tako_core::platform::shell::run_pane_command(command, EXIT_MARKER_PREFIX);
        assert_eq!(
            (&got.program, &got.args),
            (&want.program, &want.args),
            "実行ペインの起動コマンドが境界の出力と食い違う"
        );
    }

    #[test]
    fn runはコマンドをsh_c構造でspawnする() {
        // #453: Run ペインの SpawnCommand が /bin/sh -c 構造であること
        //（program に複合コマンド全文を詰めると login_shell_command のクォートで
        // 1 コマンド名扱いになり command not found で即死する）
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
        // 解決したコマンドがそのまま境界へ渡る（`bash hello.command` = 拡張子既定）
        assert_eq!(result["command"], "bash hello.command");
        assert_run_pane_command(cmd, "bash hello.command");
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

    #[test]
    fn runのtako_shell宣言は方言境界で包まれる() {
        // `# tako:shell: pwsh` は Windows で `pwsh -Command '…'` へ、
        // それ以外（bash / fish 等）は従来どおり `<shell> -c '…'` へ包まれる。
        // 直書きの `{shell} -c '{escaped}'` だと Windows で pwsh が `-c` を解さない（#875）
        let dir = std::env::temp_dir().join(format!(
            "tako-875-declshell-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("decl.txt");
        std::fs::write(
            &file,
            "# tako:shell: pwsh
# tako:run: echo it's
",
        )
        .unwrap();

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
        // 宣言シェルの包み方は OS に依らない（判定は宣言された名前だけで決まる）
        assert_run_pane_command(cmd, "pwsh -Command 'echo it''s'");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #676 の実行対象ファイル（`tako run` のテスト用。呼び出しごとに別ディレクトリ）
    fn issue676_run_target(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tako-676-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.command");
        std::fs::write(&file, "#!/usr/bin/env bash\necho hello\n").unwrap();
        (dir, file)
    }

    /// #676 受け入れ条件 1 / 3: `tako run` は focus 未指定（既定 false）で
    /// **手元のペインのフォーカスを奪わない**。CLI と MCP は同じ dispatch を通るので
    /// 1 本で両経路を担保する（origin だけ差し替えて 2 回検証する）
    #[test]
    fn issue676_runはfocus未指定でフォーカスを奪わない() {
        let (dir, file) = issue676_run_target("default");
        for origin in [PaneOrigin::Cli, PaneOrigin::Mcp] {
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
                    focus: None, // 既定 = false
                },
                origin,
            )
            .unwrap();
            let new_pane = result["pane"].as_u64().unwrap();
            assert_ne!(new_pane, root.as_u64());
            assert_eq!(
                host.ws.active_tab().tree().focused(),
                root,
                "{origin:?}: focus 未指定ではフォーカスが動かない（#676）"
            );
            // 明示 false でも同じ
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
                    focus: Some(false),
                },
                origin,
            )
            .unwrap();
            assert_ne!(result["pane"].as_u64().unwrap(), root.as_u64());
            assert_eq!(
                host.ws.active_tab().tree().focused(),
                root,
                "{origin:?}: focus=false でもフォーカスが動かない（#676）"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #676 受け入れ条件 2: `--focus` / `focus: true` を明示したときは新ペインへ移る
    #[test]
    fn issue676_runはfocus指定で新ペインへ移る() {
        let (dir, file) = issue676_run_target("explicit");
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
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws.active_tab().tree().focused().as_u64(),
            result["pane"].as_u64().unwrap(),
            "focus=true では新ペインへ移る"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #676 受け入れ条件 4: `run_interactive` は従来どおり新ペインへフォーカスを移す
    /// （ユーザーの入力を待つペインなので、こちらは移すのが正しい）
    #[test]
    fn issue676_run_interactiveは新ペインへフォーカスを移す() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let result = dispatch(
            &mut host,
            Request::RunInteractive {
                pane: Some(root.as_u64()),
                tab: None,
                command: "sudo true".into(),
                input_hint: None,
                direction: None,
                ratio: None,
                auto_close: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(
            host.ws.active_tab().tree().focused().as_u64(),
            result["pane"].as_u64().unwrap(),
            "run_interactive は入力待ちのため新ペインへ移る（回帰させない）"
        );
    }

    /// #676 受け入れ条件 5: `split` の既定（フォーカスを移さない）に回帰がないこと。
    /// `spawn_command_pane` と同じ規約であることを 1 本で並べて固定する
    #[test]
    fn issue676_splitの既定はフォーカスを移さない() {
        let mut host = MockHost::new();
        let root = host.ws.active_tab().tree().focused();
        let r = dispatch(
            &mut host,
            Request::Split {
                pane: Some(root.as_u64()),
                tab: None,
                direction: None,
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_ne!(r["pane"].as_u64().unwrap(), root.as_u64());
        assert_eq!(host.ws.active_tab().tree().focused(), root);
        // 明示 true では移る（既存仕様）
        let r = dispatch(
            &mut host,
            Request::Split {
                pane: Some(root.as_u64()),
                tab: None,
                direction: None,
                ratio: None,
                command: None,
                cwd: None,
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws.active_tab().tree().focused().as_u64(),
            r["pane"].as_u64().unwrap()
        );
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
                cwd: None,
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
                limit_resume: None,
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
                    caller_role: None,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
                limit_resume: Value::Null,
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
    fn issue983_観測手段の無い系統でも送達判定が黙らない() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        // 猶予（240 秒）を大きく超えた agy worker。agy は送達の一次シグナルを持たない
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.workers.insert(
                "q9831".into(),
                WorkerEntry {
                    pane: 9831,
                    agent: "agy".into(),
                    status: "active".into(),
                    spawned_at: "2026-01-01T00:00:00Z".into(),
                    ..Default::default()
                },
            );
        })
        .unwrap();

        // 入力待ちの画面 → 「未確認」を言う（旧実装は n/a = 何も言わなかった）
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 9831,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("agy\n> ".into()),
                full_screen: None,
                has_running_children: false,
                limit_resume: Value::Null,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            v["prompt_delivery"], "unverified",
            "黙らない（n/a にしない）"
        );
        let kinds: Vec<&str> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"prompt_delivery_unverified"),
            "未確認イベントが出ること: {kinds:?}"
        );
        assert!(
            !kinds.contains(&"prompt_undelivered"),
            "未達とは断定しないこと（supervisor に自動再送を撃たせない）: {kinds:?}"
        );
        // 次の一手は「確かめてから再送」（そのまま再送させない）
        let ev = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["kind"] == "prompt_delivery_unverified")
            .unwrap()
            .clone();
        assert_eq!(ev["recommended_action"], "verify_then_resend");
        assert_eq!(ev["agent"], "agy");

        // 画面が busy なら「未確認」も言わない（動いているものを疑わない）
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 9831,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("Thinking…\nesc to interrupt".into()),
                full_screen: None,
                has_running_children: false,
                limit_resume: Value::Null,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["prompt_delivery"], "pending");
    }

    #[test]
    fn issue983_起動失敗を分類してerrorにする() {
        use crate::orchestrator::registry::{registry_path, WorkerEntry, WorkerRegistry};
        let path = registry_path().unwrap();
        WorkerRegistry::mutate_at(&path, |reg| {
            reg.workers.insert(
                "q9832".into(),
                WorkerEntry {
                    pane: 9832,
                    agent: "codex".into(),
                    status: "active".into(),
                    spawned_at: "2026-01-01T00:00:00Z".into(),
                    ..Default::default()
                },
            );
        })
        .unwrap();

        // CLI が無い環境で起動コマンドが流れた画面（#983 の無言死そのもの）
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 9832,
                pane_exists: true,
                backend_session: None,
                live_tail: Some(
                    "testuser@host tmp % TAKO_ORCHESTRATOR_ROLE='worker:p983' codex\n\
                     zsh: command not found: codex\n\
                     testuser@host tmp % "
                        .into(),
                ),
                full_screen: None,
                has_running_children: false,
                limit_resume: Value::Null,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["status"], "error", "idle（= 完了）に見せない");
        assert_eq!(v["error"]["kind"], "launch_failed");
        assert_eq!(v["error"]["launch_problem"], "cli_not_found");
        assert_eq!(v["error"]["recommended_action"], "fix_launch");
        let detail = v["error"]["detail"].as_str().unwrap();
        assert!(detail.contains("codex"), "どの CLI の話か: {detail}");
        assert!(
            detail.contains("tako setup"),
            "次の一手が入っていること: {detail}"
        );

        // 未認証の画面も分類される
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 9832,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("Not logged in\n".into()),
                full_screen: None,
                has_running_children: false,
                limit_resume: Value::Null,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["error"]["launch_problem"], "not_authenticated");
        assert!(v["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("codex login"));

        // 作業が始まっている worker の scrollback にある command not found は
        // 起動失敗ではない（session 検出済み = 一度は走った）
        WorkerRegistry::mutate_at(&path, |reg| {
            if let Some(e) = reg.workers.get_mut("q9832") {
                e.prompt_delivered_at = Some(crate::sessions::now_iso());
            }
        })
        .unwrap();
        let v = finish_worker_status(
            WorkerStatusCtx {
                pane_id: 9832,
                pane_exists: true,
                backend_session: None,
                live_tail: Some("zsh: command not found: codex\n".into()),
                full_screen: None,
                has_running_children: false,
                limit_resume: Value::Null,
            },
            None,
            None,
        )
        .unwrap();
        assert_ne!(v["status"], "error", "動いている worker を落とさない");
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
            limit_resume: Value::Null,
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
        // #1067 で判断を session_restart へ寄せた。MockHost のペインは
        // セッションも role も持たないので、構造的な理由で断られる
        // （旧文面は「バックエンドが無い」だった）。**理由と次の一手が入っている**ことを見る
        assert!(
            msg.contains("ターミナルセッションが無い")
                || msg.contains("エージェントのペインではない"),
            "エラーメッセージが想定と異なる: {msg}"
        );
        assert!(msg.contains("pane"), "対象ペインを名指しする: {msg}");
    }

    // --- #549 ウェルカムバナー ---

    #[test]
    fn welcomeのstatusは表示状態と案内コマンドを返す() {
        let mut host = MockHost::new();
        let v = dispatch(
            &mut host,
            Request::Welcome { action: None },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["visible"], false);
        // #322: 案内は最簡形（絶対パスや既定オプションを見せない）
        assert_eq!(v["setup_command"], "tako setup");
        assert_eq!(v["master_command"], "tako master");
        assert!(v["first_launch"].is_boolean());
        assert!(v["dismissed"].is_boolean());
    }

    #[test]
    fn welcomeのshowとdismissが表示状態を切り替える() {
        let mut host = MockHost::new();
        let v = dispatch(
            &mut host,
            Request::Welcome {
                action: Some("show".into()),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(v["visible"], true);
        assert!(host.welcome_banner_visible());

        let v = dispatch(
            &mut host,
            Request::Welcome {
                action: Some("dismiss".into()),
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(v["visible"], false);
        assert!(!host.welcome_banner_visible());
    }

    #[test]
    fn welcomeの不明actionはエラー() {
        let mut host = MockHost::new();
        let err = dispatch(
            &mut host,
            Request::Welcome {
                action: Some("explode".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("status / show / dismiss"),
            "選べる値を案内すること: {err:?}"
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

    // --- #666: AI コマンド提案カード ---

    /// テスト用のリクエスト組み立て（既定値ばかりなので毎回書くと読めない）
    fn show_command_req(action: &str, commands: &[&str], pane: Option<u64>) -> Request {
        Request::ShowCommand {
            action: Some(action.into()),
            commands: commands.iter().map(|c| c.to_string()).collect(),
            label: None,
            pane,
            card: None,
            index: None,
            focus: None,
        }
    }

    #[test]
    fn issue666_showしたコマンドは論理文字列のまま返る() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        // ペイン幅より確実に長い 1 行（画面から拾うと物理改行が入る種類の文字列）
        let long = format!("cargo test --workspace -- --nocapture {}", "x".repeat(240));
        let v = dispatch(
            &mut host,
            Request::ShowCommand {
                action: None, // 既定は show
                commands: vec![long.clone()],
                label: Some("テストを回す".into()),
                pane: Some(pane),
                card: None,
                index: None,
                focus: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(v["card"]["pane"], pane);
        assert_eq!(v["card"]["count"], 1);
        assert_eq!(v["card"]["label"], "テストを回す");
        assert_eq!(v["card"]["commands"][0], long);
        assert_eq!(v["pane_cards"], 1);

        // list でも同じ論理文字列が返る（AI 側で同一性を検証できる）
        let listed = dispatch(
            &mut host,
            show_command_req("list", &[], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        assert_eq!(listed["cards"][0]["commands"][0], long);
        assert_eq!(listed["total"], 1);
    }

    #[test]
    fn issue666_copyは論理文字列をクリップボードへ渡す() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let multi = "cd /tmp \\\n  && ls -la";
        dispatch(
            &mut host,
            show_command_req("show", &["echo one", multi], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        // index 省略で 1 件目
        let v = dispatch(
            &mut host,
            show_command_req("copy", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["copied"], true);
        assert_eq!(v["index"], 1);
        assert_eq!(host.clipboard, vec!["echo one".to_string()]);
        // index=2 は改行込みでそのまま渡る
        let v = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("copy".into()),
                commands: Vec::new(),
                label: None,
                pane: Some(pane),
                card: None,
                index: Some(2),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["command"], multi);
        assert_eq!(host.clipboard.last().unwrap(), multi);
    }

    #[test]
    fn issue666_runは同じタブに新ペインを分割して実行する() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let before_panes = host.ws.active_tab().tree().panes().len();
        let before_tabs = host.ws.tabs().len();
        dispatch(
            &mut host,
            show_command_req("show", &["echo 'カード実行' && pwd"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        let v = dispatch(
            &mut host,
            show_command_req("run", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap();
        let new_pane = v["pane"].as_u64().unwrap();
        assert_ne!(new_pane, pane, "手元のペインで実行してはならない");
        assert_eq!(v["from_pane"], pane);
        assert_eq!(v["focus"], false, "既定でフォーカスを奪わない");
        assert_eq!(host.ws.tabs().len(), before_tabs, "タブを増やさない");
        assert_eq!(
            host.ws.active_tab().tree().panes().len(),
            before_panes + 1,
            "同じタブにペインが 1 枚増える"
        );
        // 起動コマンドの形は方言境界が決める（#875）。カードの論理文字列がそのまま
        // 境界へ渡ることを、境界の出力と突き合わせて見る
        let opts = host
            .attached_options
            .get(&new_pane)
            .expect("セッション起動");
        let cmd = opts.command.as_ref().expect("コマンド付き起動");
        assert_run_pane_command(cmd, "echo 'カード実行' && pwd");
        // POSIX 側は /bin/sh -c で構造化して渡る（#453 の 127 即死を避ける形）
        #[cfg(unix)]
        {
            assert_eq!(cmd.program, "/bin/sh");
            assert_eq!(cmd.args[0], "-c");
            assert!(
                cmd.args[1].starts_with("echo 'カード実行' && pwd"),
                "論理文字列がそのまま渡る: {:?}",
                cmd.args[1]
            );
        }
        // 手元のペインのフォーカスを奪わない（split は新ペインへフォーカスを移す仕様なので
        // 明示的に戻している。この assert を消すと退行が見えなくなる）
        assert_eq!(
            host.ws.active_tab().tree().focused().as_u64(),
            pane,
            "既定ではフォーカスが手元のペインに残る"
        );
        // カードは実行後も残る（他のコマンドを続けて実行できる）
        let listed = dispatch(
            &mut host,
            show_command_req("list", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(listed["cards"].as_array().unwrap().len(), 1);

        // focus=true を明示したときだけ新ペインへ移る
        let v = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("run".into()),
                commands: Vec::new(),
                label: None,
                pane: Some(pane),
                card: None,
                index: None,
                focus: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["focus"], true);
        assert_eq!(
            host.ws.active_tab().tree().focused().as_u64(),
            v["pane"].as_u64().unwrap(),
            "focus=true では新ペインへ移る"
        );
    }

    #[test]
    fn issue666_カードid指定はペイン指定なしでも解決できる() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let shown = dispatch(
            &mut host,
            show_command_req("show", &["echo hi"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        let card = shown["card"]["id"].as_u64().unwrap();
        // pane 省略 + card 指定（TAKO_PANE_ID が無い外部シェルからの操作）
        let v = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("copy".into()),
                commands: Vec::new(),
                label: None,
                pane: None,
                card: Some(card),
                index: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["card"], card);
        assert_eq!(host.clipboard, vec!["echo hi".to_string()]);
    }

    #[test]
    fn issue666_dismissはカード単位とペイン単位で効く() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let first = dispatch(
            &mut host,
            show_command_req("show", &["echo 1"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap()["card"]["id"]
            .as_u64()
            .unwrap();
        dispatch(
            &mut host,
            show_command_req("show", &["echo 2"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        let v = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("dismiss".into()),
                commands: Vec::new(),
                label: None,
                pane: None,
                card: Some(first),
                index: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["dismissed"], 1);
        assert_eq!(v["remaining"], 1);
        // card 省略 = そのペインの全件
        let v = dispatch(
            &mut host,
            show_command_req("dismiss", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(v["dismissed"], 1);
        assert_eq!(v["remaining"], 0);
    }

    #[test]
    fn issue666_不正な入力は理由つきで拒否される() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        // コマンド無し
        let err = dispatch(
            &mut host,
            show_command_req("show", &[], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("コマンドが 1 件も"), "{err}");
        // 空文字列
        let err = dispatch(
            &mut host,
            show_command_req("show", &["   "], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("空のコマンド"), "{err}");
        // エスケープシーケンス混入
        let err = dispatch(
            &mut host,
            show_command_req("show", &["echo \x1b[2J"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("制御文字"), "{err}");
        // 不明 action は選べる値を案内する
        let err = dispatch(
            &mut host,
            show_command_req("explode", &["echo hi"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("show / list / copy / run / dismiss"),
            "{err}"
        );
        // カードが無いペインでの copy / run
        let err = dispatch(
            &mut host,
            show_command_req("copy", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("表示中のコマンドカードが無い"),
            "{err}"
        );
        // 範囲外のコマンド番号
        dispatch(
            &mut host,
            show_command_req("show", &["echo hi"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap();
        let err = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("run".into()),
                commands: Vec::new(),
                label: None,
                pane: Some(pane),
                card: None,
                index: Some(5),
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("コマンド番号が範囲外"), "{err}");
        assert!(
            host.clipboard.is_empty(),
            "失敗した操作で副作用を起こさない"
        );
    }

    #[test]
    fn issue666_消えたカードやペインへの操作はエラーで返る() {
        let mut host = MockHost::new();
        let pane = host.root_pane();
        let card = dispatch(
            &mut host,
            show_command_req("show", &["echo hi"], Some(pane)),
            PaneOrigin::Mcp,
        )
        .unwrap()["card"]["id"]
            .as_u64()
            .unwrap();
        dispatch(
            &mut host,
            show_command_req("dismiss", &[], Some(pane)),
            PaneOrigin::Cli,
        )
        .unwrap();
        // 閉じたカードのボタンを押した相当（panic せずエラー）
        let err = dispatch(
            &mut host,
            Request::ShowCommand {
                action: Some("run".into()),
                commands: Vec::new(),
                label: None,
                pane: None,
                card: Some(card),
                index: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("見つからない"), "{err}");
        // 存在しないペインへの show
        let err = dispatch(
            &mut host,
            show_command_req("show", &["echo hi"], Some(999_999)),
            PaneOrigin::Mcp,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::PaneNotFound(999_999)));
    }
}

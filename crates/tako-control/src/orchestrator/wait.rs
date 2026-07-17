//! orchestrator::wait — worker 完了待ちポーリングの一元実装（#83）
//!
//! MCP（`tako_orchestrator_run`）と CLI（`tako orchestrator run` / `watch`）に
//! 重複していたポーリング状態機械をここへ一本化する。Request の実行手段は
//! トランスポートごとに異なる（MCP = dispatch チャネル往復、CLI = IPC 往復）ため、
//! `exec` クロージャとして注入する。
//!
//! sleep を含む同期ブロッキング関数群なので **UI スレッドから呼ばないこと**
//! （MCP ハンドラスレッド / CLI プロセスで呼ぶ）。
//!
//! 判定ヒューリスティック（`screen_looks_busy` / `screen_looks_idle`）は
//! worker「完了」監視用で、`claude_tui` の送達確認用パターンとは目的が異なるため
//! ここに置く（dispatch の idle 補正も本モジュールを参照する）。
//!
//! ## 非同期 run レジストリ（#121）
//!
//! `RunRegistry` は進行中・完了済みの run をプロセス内でグローバルに追跡する。
//! `run_start` で spawn + バックグラウンドポーリングを開始し、`run_status` で
//! 進捗を照会、`run_result` で完了した結果を回収（+ auto_close）する。
//! MCP コール中断で run が孤児化しない（ポーリングスレッドが独立して完走する）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol::Request;

/// Request 実行係（MCP: `McpSession::exec`、CLI: `send_request`）。
/// Err は「tako へ届かなかった」（IPC 断・再起動中）を含む
pub type Exec<'a> = &'a mut dyn FnMut(Request) -> Result<Value, String>;

/// 完了待ちポーリングの設定
pub struct WatchOptions {
    /// 監視対象 worker のペイン ID
    pub pane_id: u64,
    /// claude の session ID（あれば agents 一次シグナルの精度が上がる）
    pub session_id: Option<String>,
    /// tmux session 名（pane 消滅・IPC 断時のフォールバック追跡。
    /// 生存していれば gone を取り消す = tako 再起動時の誤検知防止）
    pub tmux_session: Option<String>,
    /// None = 無期限（watch の既定）
    pub timeout: Option<Duration>,
    /// ループ開始前の初期待機（run は claude 起動 + プロンプト送達の 20 秒、watch は 0）
    pub initial_delay: Duration,
    /// ポーリング間隔（本番 5 秒。テストは ZERO）
    pub interval: Duration,
}

/// 完了待ちの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchOutcome {
    /// worker が入力待ち（= 完了）になった。`ctx_percent` は取得できた場合のみ
    Idle { ctx_percent: Option<u64> },
    /// worker が質問/確認ダイアログ待ちで停止した（#267）。
    /// Idle と同じく入力待ちだが、master 側の対応が異なる
    Question { ctx_percent: Option<u64> },
    /// worker が permission ダイアログ（ツール実行の承認要求）で停止した（#319）。
    /// master は `tako orchestrator respond` で応答する
    PermissionWaiting { permission_dialog: Value },
    /// worker が異常（API エラー・usage limit 等）で停止した（#157）。
    /// `detail` は検知パターンにマッチした画面上の行
    Error {
        kind: WorkerErrorKind,
        detail: String,
    },
    /// worker が停滞: 実行中子プロセスなし + 画面不変（#224）
    Stalled { detail: String },
    /// ペインも tmux session も消滅した
    Gone,
    /// タイムアウトに達した（worker は動き続けている可能性がある）
    Timeout,
}

/// worker 画面から検知した異常停止の種別（#157）。
/// 種別ごとの復帰手段が異なるため、master の自動リカバリの分岐点になる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerErrorKind {
    /// API エラー（接続断・タイムアウト等）で停止。続行指示の再送で復帰できることが多い
    ApiError,
    /// usage limit 到達で停止（claude の 5h/週次、codex のクレジット）。解除時刻まで待つ必要がある
    UsageLimit,
    /// rate limit 起因の選択ダイアログ（codex のモデル切替提案等）で停止。選択肢への応答で復帰できる
    LimitDialog,
}

impl WorkerErrorKind {
    /// JSON / イベント行に載せる機械可読 slug
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiError => "api_error",
            Self::UsageLimit => "usage_limit",
            Self::LimitDialog => "limit_dialog",
        }
    }

    /// slug からの復元（watch が dispatch 応答の error.kind を読む用）
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "api_error" => Some(Self::ApiError),
            "usage_limit" => Some(Self::UsageLimit),
            "limit_dialog" => Some(Self::LimitDialog),
            _ => None,
        }
    }

    /// master 向けの推奨リカバリアクション（worker_status の JSON にそのまま載せる）
    pub fn recommended_action(self) -> &'static str {
        match self {
            // 続行指示（「続けて」等）を send_input すれば復帰できることが多い
            Self::ApiError => "resume",
            // 解除時刻まで待ってから続行指示する（即時再送しても弾かれる）
            Self::UsageLimit => "wait_reset",
            // ダイアログの選択肢に応答する（send_input でキー送信）
            Self::LimitDialog => "respond_dialog",
        }
    }
}

/// worker が完了（idle）または消滅（gone）するまでブロックする。
///
/// 判定は `OrchestratorWorkerStatus` を `interval` ごとに呼び、agents 一次シグナル
/// （明示 session_id or 自動解決）なら 3 回、画面推定フォールバックなら 8 回の
/// idle 連続で完了とみなす（サブエージェント完了瞬間の一時 idle 誤検知対策）。
/// gone は 2 回連続で確定するが、`tmux_session` が生存していれば取り消す
/// （tako 再起動中はペイン一覧が空になるため）。
///
/// 停止確定時に画面へ既知のエラーパターン（API エラー・usage limit 等）があれば
/// `Idle` ではなく `Error` を返す（#157）。error 状態（status = "error"）も
/// 「worker が止まっている」ことに変わりはないため idle と同じ streak で確定する。
///
/// `progress` が Some のとき、ポーリングごとに中間状態を更新する（#121 の非同期 run 用）
pub fn wait_for_worker(
    exec: Exec,
    opts: &WatchOptions,
    progress: Option<&Arc<Mutex<RunSnapshot>>>,
) -> WatchOutcome {
    let start = Instant::now();
    let deadline = opts.timeout.map(|t| start + t);
    std::thread::sleep(opts.initial_delay);

    let mut idle_streak: u32 = 0;
    let mut gone_streak: u32 = 0;
    let mut stalled_streak: u32 = 0;

    loop {
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                return WatchOutcome::Timeout;
            }
        }

        let result = exec(Request::OrchestratorWorkerStatus {
            pane_id: opts.pane_id,
            session_id: opts.session_id.clone(),
            tmux_session: opts.tmux_session.clone(),
        });

        match result {
            Ok(val) => {
                let status = val["status"].as_str().unwrap_or("unknown");
                let recent = val["recent_output"].as_str().unwrap_or("");
                let source = val["status_source"].as_str().unwrap_or("screen");
                // agents 一次シグナル（明示 or 自動解決）は streak 3、画面推定は streak 8
                let need_streak: u32 = if source == "screen" { 8 } else { 3 };

                // 非同期 run 用: 中間スナップショットを更新（#121）
                if let Some(snap) = progress {
                    if let Ok(mut s) = snap.lock() {
                        s.worker_status = status.to_string();
                        s.elapsed_secs = start.elapsed().as_secs();
                    }
                }

                match status {
                    "gone" => {
                        // tmux session が生きていれば pane 消滅は tako 再起動中とみなす
                        if tmux_session_alive(opts.tmux_session.as_deref()) {
                            gone_streak = 0;
                            idle_streak = 0;
                        } else {
                            gone_streak += 1;
                            if gone_streak >= 3 {
                                return WatchOutcome::Gone;
                            }
                        }
                        stalled_streak = 0;
                    }
                    // "error" は「idle + 画面にエラーパターン」の細分類（#157）。
                    // 停止していることは同じなので idle と同じ streak で確定させ、
                    // 確定時にどちらの outcome かを画面から再判定する
                    "idle" | "error" => {
                        gone_streak = 0;
                        stalled_streak = 0;
                        // 画面内容で busy パターンがあれば idle を取り消す
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else {
                            idle_streak += 1;
                        }
                    }
                    // #224: dispatch が stalled と判定した場合。3 回連続で確定
                    "stalled" => {
                        gone_streak = 0;
                        idle_streak = 0;
                        stalled_streak += 1;
                        if stalled_streak >= 3 {
                            let detail = val["stalled"]
                                .as_object()
                                .and_then(|s| s.get("detail"))
                                .and_then(|d| d.as_str())
                                .unwrap_or(
                                    "busy だが実行中の子プロセスが無く、画面の busy パターンも無い",
                                )
                                .to_string();
                            return WatchOutcome::Stalled { detail };
                        }
                    }
                    // #267: "waiting" は permission ダイアログ等の待機状態。
                    // idle_streak を加算しない（IDLE として発火させない）。
                    // #319: permission_dialog が応答に含まれていれば即発火する
                    // （ダイアログは安定状態で一時的に現れて消えることはない）
                    "waiting" => {
                        gone_streak = 0;
                        idle_streak = 0;
                        stalled_streak = 0;
                        if val.get("permission_dialog").is_some_and(|v| !v.is_null()) {
                            return WatchOutcome::PermissionWaiting {
                                permission_dialog: val["permission_dialog"].clone(),
                            };
                        }
                    }
                    "busy" => {
                        gone_streak = 0;
                        idle_streak = 0;
                        stalled_streak = 0;
                    }
                    _ => {
                        // unknown: 画面内容から推定（判定不能は busy 扱い = 誤 idle 防止）
                        gone_streak = 0;
                        stalled_streak = 0;
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else if screen_looks_idle(recent) {
                            idle_streak += 1;
                        } else {
                            idle_streak = 0;
                        }
                    }
                }

                if idle_streak >= need_streak {
                    // 停止確定。dispatch が判定済みの error（新 tako-app）を優先し、
                    // 無ければ画面から自力検知する（tako-app 更新前でも watch 単体で
                    // WORKER_ERROR を出せるようにするフォールバック。判定関数は同一）
                    let error = val["error"]
                        .as_object()
                        .and_then(|e| {
                            let kind = WorkerErrorKind::from_slug(e.get("kind")?.as_str()?)?;
                            let detail = e
                                .get("detail")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();
                            Some((kind, detail))
                        })
                        .or_else(|| detect_worker_error(recent));
                    if let Some((kind, detail)) = error {
                        return WatchOutcome::Error { kind, detail };
                    }
                    // #267: idle 確定後に events から question を判定。
                    // question がある場合は Question で通知（master の対応が異なるため）
                    let has_question = val["events"].as_array().is_some_and(|evts| {
                        evts.iter().any(|e| e["kind"].as_str() == Some("question"))
                    });
                    if has_question {
                        return WatchOutcome::Question {
                            ctx_percent: val["ctx_percent"].as_u64(),
                        };
                    }
                    return WatchOutcome::Idle {
                        ctx_percent: val["ctx_percent"].as_u64(),
                    };
                }
            }
            Err(_) => {
                // 実行エラー = tako が再起動中の可能性。tmux で実在確認
                if tmux_session_alive(opts.tmux_session.as_deref()) {
                    gone_streak = 0;
                } else {
                    gone_streak += 1;
                    // #267: 閾値を 2→3 に引き上げ（一時的な IPC 断での偽 GONE 防止）
                    if gone_streak >= 3 {
                        return WatchOutcome::Gone;
                    }
                }
            }
        }

        std::thread::sleep(opts.interval);
    }
}

/// `orchestrator run` の設定（spawn + 完了待ち + 出力取得 + close）
pub struct RunOptions {
    pub project: String,
    pub prompt: String,
    pub label: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// worker のエージェント種別（claude / codex / agy。省略時はプロファイル既定。#120）
    pub agent: Option<String>,
    /// 分割元ペイン ID（`tab` と排他。両方 None は spawn 側でエラー）
    pub pane: Option<u64>,
    /// 子を出すタブ ID
    pub tab: Option<u64>,
    /// 呼び出し元の TAKO_ORCHESTRATOR_ROLE（#109: 複数 master 識別）
    pub caller_role: Option<String>,
    /// 完了待ちタイムアウト
    pub timeout: Duration,
    /// 完了後にペインを自動 close するか
    pub auto_close: bool,
    /// 返す出力の末尾行数
    pub output_lines: usize,
    /// claude 起動 + プロンプト送達の初期待機（本番 20 秒。テストは ZERO）
    pub initial_delay: Duration,
    /// ポーリング間隔（本番 5 秒。テストは ZERO）
    pub interval: Duration,
    /// 委任台帳の task_type（Issue #292。統制語彙。省略時は investigation）
    pub task_type: Option<String>,
}

/// spawn + 完了待ち + 出力取得 + close を 1 回で行う（`orchestrator run` の本体）。
/// `on_spawned(pane_id, tmux_session)` は spawn 直後に呼ばれる進捗フック
/// （CLI の経過表示用。不要なら no-op を渡す）。
/// 返り値は `{pane_id, spawned_by, status, output, duration_seconds, closed}`
pub fn run_worker(
    exec: Exec,
    opts: &RunOptions,
    on_spawned: &mut dyn FnMut(u64, Option<&str>),
) -> Result<Value, String> {
    // --- 1. Spawn ---
    let spawn_result = exec(Request::OrchestratorSpawn {
        project: opts.project.clone(),
        prompt: opts.prompt.clone(),
        label: opts.label.clone(),
        model: opts.model.clone(),
        effort: opts.effort.clone(),
        pane: opts.pane,
        tab: opts.tab,
        caller_role: opts.caller_role.clone(),
        agent: opts.agent.clone(),
        caller_pid: None,
        task_type: opts.task_type.clone(),
    })?;
    let pane_id = spawn_result["pane_id"].as_u64().unwrap_or(0);
    let spawned_by = spawn_result["spawned_by"].as_u64().unwrap_or(0);
    let tmux_session = spawn_result["tmux_session"].as_str().map(String::from);
    on_spawned(pane_id, tmux_session.as_deref());

    // --- 2. 完了待ち ---
    let start = Instant::now();
    let outcome = wait_for_worker(
        exec,
        &WatchOptions {
            pane_id,
            session_id: None,
            tmux_session: tmux_session.clone(),
            timeout: Some(opts.timeout),
            initial_delay: opts.initial_delay,
            interval: opts.interval,
        },
        None,
    );
    let final_status = match outcome {
        WatchOutcome::Idle { .. } | WatchOutcome::Question { .. } => "completed",
        WatchOutcome::Error { .. } => "worker_error",
        WatchOutcome::Stalled { .. } => "worker_stalled",
        WatchOutcome::PermissionWaiting { .. } => "permission_waiting",
        WatchOutcome::Gone => "error",
        WatchOutcome::Timeout => "timeout",
    };

    // --- 3. 出力取得（dispatch の Read 応答は {"pane", "text"}。#82: 旧実装は
    // "content" を読んでいて常に空だった） ---
    let output = exec(Request::Read {
        pane: Some(pane_id),
        lines: Some(opts.output_lines),
        tmux_session: tmux_session.clone(),
    })
    .ok()
    .and_then(|v| v["text"].as_str().map(String::from))
    .unwrap_or_default();

    // --- 4. 自動 close（run の完了後なので force: true）。
    // エラー / stalled / question / permission 時は close しない ---
    let closed = if opts.auto_close
        && !matches!(
            outcome,
            WatchOutcome::Error { .. }
                | WatchOutcome::Stalled { .. }
                | WatchOutcome::Question { .. }
                | WatchOutcome::PermissionWaiting { .. }
        ) {
        exec(Request::Close {
            pane: Some(pane_id),
            force: true,
        })
        .is_ok()
    } else {
        false
    };

    let mut result = json!({
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "status": final_status,
        "output": output,
        "duration_seconds": start.elapsed().as_secs(),
        "closed": closed,
    });
    if let WatchOutcome::Error { kind, detail } = &outcome {
        result["error"] = json!({
            "kind": kind.as_str(),
            "detail": detail,
            "recommended_action": kind.recommended_action(),
        });
    }
    if let WatchOutcome::Stalled { detail } = &outcome {
        result["stalled"] = json!({
            "detail": detail,
            "recommended_action": "check_and_resume",
        });
    }
    if matches!(outcome, WatchOutcome::Question { .. }) {
        result["question"] = json!(true);
    }
    if let WatchOutcome::PermissionWaiting {
        ref permission_dialog,
    } = outcome
    {
        result["permission_dialog"] = permission_dialog.clone();
    }

    // Issue #242: usage_limit / crash / gone でチェックポイントを Suspended に遷移させる
    let suspend_reason = match &outcome {
        WatchOutcome::Error { kind, .. } => Some(kind.as_str().to_string()),
        WatchOutcome::Gone => Some("gone".to_string()),
        _ => None,
    };
    if let Some(reason) = suspend_reason {
        if let Ok(Some(task_id)) = crate::task_checkpoints::suspend_by_pane(pane_id, &reason) {
            result["task_suspended"] = json!({
                "task_id": task_id,
                "reason": reason,
            });
        }
    }

    Ok(result)
}

// ==========================================================================
// 非同期 run レジストリ（#121）
// ==========================================================================

/// 進行中の run エントリの中間状態（ポーリングスレッドが更新する）
#[derive(Debug, Clone)]
pub struct RunSnapshot {
    /// 直近の worker_status ポーリング結果（busy / idle / error / gone / unknown）
    worker_status: String,
    /// 経過秒数
    elapsed_secs: u64,
}

/// 完了した run の結果
#[derive(Debug, Clone)]
struct RunCompleted {
    outcome: WatchOutcome,
    elapsed_secs: u64,
}

/// 1 件の run エントリ
struct RunEntry {
    pane_id: u64,
    spawned_by: u64,
    tmux_session: Option<String>,
    auto_close: bool,
    output_lines: usize,
    started_at: Instant,
    /// ポーリングスレッドが定期更新する中間状態
    snapshot: Arc<Mutex<RunSnapshot>>,
    /// 完了時にセットされる
    completed: Arc<Mutex<Option<RunCompleted>>>,
}

/// グローバルな非同期 run レジストリ
struct RunRegistry {
    entries: HashMap<String, RunEntry>,
    next_id: u64,
}

const MAX_COMPLETED_RUNS: usize = 256;

impl RunRegistry {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("run-{id}")
    }

    /// 結果未回収クライアントがいても完了履歴を無制限保持しない（Issue #258）。
    /// 実行中エントリは対象外とし、完了時刻の代わりに開始時刻が古い順で落とす。
    fn prune_completed(&mut self) {
        let mut completed: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .completed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some()
                    .then_some((id.clone(), entry.started_at))
            })
            .collect();
        if completed.len() <= MAX_COMPLETED_RUNS {
            return;
        }
        completed.sort_by_key(|(_, started_at)| *started_at);
        let remove_count = completed.len() - MAX_COMPLETED_RUNS;
        for (id, _) in completed.into_iter().take(remove_count) {
            self.entries.remove(&id);
        }
    }
}

fn registry() -> &'static Mutex<RunRegistry> {
    use std::sync::OnceLock;
    static REG: OnceLock<Mutex<RunRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(RunRegistry::new()))
}

/// 非同期 run を開始する: spawn してバックグラウンドでポーリングを開始し、
/// `{run_id, pane_id, spawned_by, tmux_session}` を即座に返す。
/// exec はこのスレッドで spawn のみ実行する（ポーリングは別スレッド）。
/// `exec_factory` はポーリングスレッド用の新しい exec を生成する
pub fn run_start(
    exec: Exec,
    opts: &RunOptions,
    exec_factory: impl FnOnce() -> Box<dyn FnMut(Request) -> Result<Value, String> + Send>
        + Send
        + 'static,
) -> Result<Value, String> {
    // spawn（メインスレッドで実行）
    let spawn_result = exec(Request::OrchestratorSpawn {
        project: opts.project.clone(),
        prompt: opts.prompt.clone(),
        label: opts.label.clone(),
        model: opts.model.clone(),
        effort: opts.effort.clone(),
        pane: opts.pane,
        tab: opts.tab,
        caller_role: opts.caller_role.clone(),
        agent: opts.agent.clone(),
        caller_pid: None,
        task_type: opts.task_type.clone(),
    })?;
    let pane_id = spawn_result["pane_id"].as_u64().unwrap_or(0);
    let spawned_by = spawn_result["spawned_by"].as_u64().unwrap_or(0);
    let tmux_session = spawn_result["tmux_session"].as_str().map(String::from);

    let snapshot = Arc::new(Mutex::new(RunSnapshot {
        worker_status: "starting".into(),
        elapsed_secs: 0,
    }));
    let completed = Arc::new(Mutex::new(None));

    let run_id = {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        let id = reg.alloc_id();
        reg.entries.insert(
            id.clone(),
            RunEntry {
                pane_id,
                spawned_by,
                tmux_session: tmux_session.clone(),
                auto_close: opts.auto_close,
                output_lines: opts.output_lines,
                started_at: Instant::now(),
                snapshot: Arc::clone(&snapshot),
                completed: Arc::clone(&completed),
            },
        );
        id
    };

    // ポーリングスレッドを起動
    let timeout = opts.timeout;
    let initial_delay = opts.initial_delay;
    let interval = opts.interval;
    let session_id: Option<String> = None;
    let tmux_for_thread = tmux_session.clone();

    std::thread::Builder::new()
        .name(format!("run-{pane_id}"))
        .spawn(move || {
            let mut exec_fn = exec_factory();
            let start = Instant::now();
            let outcome = wait_for_worker(
                &mut *exec_fn,
                &WatchOptions {
                    pane_id,
                    session_id,
                    tmux_session: tmux_for_thread,
                    timeout: Some(timeout),
                    initial_delay,
                    interval,
                },
                Some(&snapshot),
            );
            let elapsed_secs = start.elapsed().as_secs();
            {
                *completed.lock().unwrap_or_else(|e| e.into_inner()) = Some(RunCompleted {
                    outcome,
                    elapsed_secs,
                });
            }
            let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
            // 自分自身を含め最新 256 件だけ残す。run_result と競合しても registry lock
            // によりエントリ参照中の除去は起きない。
            reg.prune_completed();
        })
        .map_err(|e| format!("ポーリングスレッドの起動に失敗: {e}"))?;

    Ok(json!({
        "run_id": run_id,
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "tmux_session": tmux_session,
    }))
}

/// 非同期 run の進捗を返す。run_id が不明なら Err
pub fn run_status(run_id: &str) -> Result<Value, String> {
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let entry = reg
        .entries
        .get(run_id)
        .ok_or_else(|| format!("run_id '{run_id}' が見つからない"))?;

    let completed = entry.completed.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = completed.as_ref() {
        let status = match &c.outcome {
            WatchOutcome::Idle { .. } | WatchOutcome::Question { .. } => "completed",
            WatchOutcome::Error { .. } => "worker_error",
            WatchOutcome::Stalled { .. } => "worker_stalled",
            WatchOutcome::PermissionWaiting { .. } => "permission_waiting",
            WatchOutcome::Gone => "error",
            WatchOutcome::Timeout => "timeout",
        };
        let mut result = json!({
            "run_id": run_id,
            "pane_id": entry.pane_id,
            "status": status,
            "phase": "finished",
            "elapsed_seconds": c.elapsed_secs,
        });
        if let WatchOutcome::Error { kind, detail } = &c.outcome {
            result["error"] = json!({
                "kind": kind.as_str(),
                "detail": detail,
                "recommended_action": kind.recommended_action(),
            });
        }
        if let WatchOutcome::Stalled { detail } = &c.outcome {
            result["stalled"] = json!({
                "detail": detail,
                "recommended_action": "check_and_resume",
            });
        }
        return Ok(result);
    }
    drop(completed);

    let snap = entry.snapshot.lock().unwrap_or_else(|e| e.into_inner());
    Ok(json!({
        "run_id": run_id,
        "pane_id": entry.pane_id,
        "status": snap.worker_status,
        "phase": "running",
        "elapsed_seconds": entry.started_at.elapsed().as_secs(),
    }))
}

/// 完了した run の結果を回収する。未完了なら `phase: "running"` を返す。
/// 完了済みなら出力取得 + auto_close を行い、レジストリから除去する
pub fn run_result(run_id: &str, exec: Exec) -> Result<Value, String> {
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let entry = reg
        .entries
        .get(run_id)
        .ok_or_else(|| format!("run_id '{run_id}' が見つからない"))?;

    let completed = entry.completed.lock().unwrap_or_else(|e| e.into_inner());
    if completed.is_none() {
        let snap = entry.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        return Ok(json!({
            "run_id": run_id,
            "pane_id": entry.pane_id,
            "status": snap.worker_status,
            "phase": "running",
            "elapsed_seconds": entry.started_at.elapsed().as_secs(),
        }));
    }
    let c = completed.as_ref().unwrap().clone();
    drop(completed);

    let pane_id = entry.pane_id;
    let spawned_by = entry.spawned_by;
    let tmux_session = entry.tmux_session.clone();
    let auto_close = entry.auto_close;
    let output_lines = entry.output_lines;
    drop(reg);

    let final_status = match &c.outcome {
        WatchOutcome::Idle { .. } | WatchOutcome::Question { .. } => "completed",
        WatchOutcome::Error { .. } => "worker_error",
        WatchOutcome::Stalled { .. } => "worker_stalled",
        WatchOutcome::PermissionWaiting { .. } => "permission_waiting",
        WatchOutcome::Gone => "error",
        WatchOutcome::Timeout => "timeout",
    };

    // 出力取得
    let output = exec(Request::Read {
        pane: Some(pane_id),
        lines: Some(output_lines),
        tmux_session: tmux_session.clone(),
    })
    .ok()
    .and_then(|v| v["text"].as_str().map(String::from))
    .unwrap_or_default();

    // auto_close（エラー / stalled / question / permission 時は close しない）
    let closed = if auto_close
        && !matches!(
            c.outcome,
            WatchOutcome::Error { .. }
                | WatchOutcome::Stalled { .. }
                | WatchOutcome::Question { .. }
                | WatchOutcome::PermissionWaiting { .. }
        ) {
        exec(Request::Close {
            pane: Some(pane_id),
            force: true,
        })
        .is_ok()
    } else {
        false
    };

    // レジストリから除去
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .remove(run_id);

    let mut result = json!({
        "run_id": run_id,
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "status": final_status,
        "output": output,
        "duration_seconds": c.elapsed_secs,
        "closed": closed,
    });
    if let WatchOutcome::Error { kind, detail } = &c.outcome {
        result["error"] = json!({
            "kind": kind.as_str(),
            "detail": detail,
            "recommended_action": kind.recommended_action(),
        });
    }
    if let WatchOutcome::Stalled { detail } = &c.outcome {
        result["stalled"] = json!({
            "detail": detail,
            "recommended_action": "check_and_resume",
        });
    }
    if matches!(c.outcome, WatchOutcome::Question { .. }) {
        result["question"] = json!(true);
    }
    if let WatchOutcome::PermissionWaiting {
        ref permission_dialog,
    } = c.outcome
    {
        result["permission_dialog"] = permission_dialog.clone();
    }
    Ok(result)
}

/// 全 run の一覧を返す（run_status 相当の情報をまとめて）
pub fn run_list() -> Value {
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let runs: Vec<Value> = reg
        .entries
        .iter()
        .map(|(run_id, entry)| {
            let completed = entry.completed.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = completed.as_ref() {
                let status = match &c.outcome {
                    WatchOutcome::Idle { .. } | WatchOutcome::Question { .. } => "completed",
                    WatchOutcome::Error { .. } => "worker_error",
                    WatchOutcome::Stalled { .. } => "worker_stalled",
                    WatchOutcome::PermissionWaiting { .. } => "permission_waiting",
                    WatchOutcome::Gone => "error",
                    WatchOutcome::Timeout => "timeout",
                };
                json!({
                    "run_id": run_id,
                    "pane_id": entry.pane_id,
                    "status": status,
                    "phase": "finished",
                    "elapsed_seconds": c.elapsed_secs,
                })
            } else {
                let snap = entry.snapshot.lock().unwrap_or_else(|e| e.into_inner());
                json!({
                    "run_id": run_id,
                    "pane_id": entry.pane_id,
                    "status": snap.worker_status,
                    "phase": "running",
                    "elapsed_seconds": entry.started_at.elapsed().as_secs(),
                })
            }
        })
        .collect();
    json!({ "runs": runs })
}

/// tmux session が生きているか（tako バックエンドサーバー上）。None は常に false
fn tmux_session_alive(session: Option<&str>) -> bool {
    let Some(session) = session else {
        return false;
    };
    let socket = tako_core::tmux_backend::socket_name();
    tako_core::tmux::session_alive(Some(&socket), session)
}

// --- worker 画面の完了判定ヒューリスティック ---

/// 空行を除いた末尾 N 行を返す（新しい行が先頭）
pub fn tail_lines(output: &str, n: usize) -> Vec<&str> {
    output
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .collect()
}

/// worker の画面が busy（作業中）を示すパターンを含むか（末尾 5 行に限定）。
/// claude / codex は「esc to interrupt」、agy は「esc to cancel」＋
/// スピナー行「Generating」を拾う（Issue #120。実採取画面より）。
/// 「Thinking」は素のままだと agy フッターのモデル名表記
/// 「Claude Opus 4.6 (Thinking)」（常時表示）に誤爆して永遠に busy 判定になるため、
/// claude スピナーの実表示「Thinking…」に限定する（実機検証 2026-07-10 で発見）
pub fn screen_looks_busy(output: &str) -> bool {
    tail_lines(output, 5).iter().any(|l| {
        l.contains("esc to interrupt")
            || l.contains("esc to cancel")
            || l.contains("Generating")
            || l.contains("Working (")
            || l.contains("ing… (")
            || l.contains("Thinking…")
            || l.contains("Thinking...")
            || l.contains("Reading")
            || l.contains("Editing")
            || l.contains("Running")
            || l.contains("Writing")
            || l.contains("Searching")
    })
}

/// worker の画面が idle（入力欄プロンプト表示 = 入力待ち）を示すか。
/// プロンプト文字は claude `❯` / codex `›` / agy `>` の和集合（Issue #120）。
/// ASCII の `>` は「`>` 単独 or `> `＋内容」のみ入力欄とみなす（PS2 等との誤検知対策）。
/// いずれの TUI もプロンプトの下にフッター（区切り線・モデル情報・ctx% 等）が
/// 1〜6 行あるため、末尾 10 行の範囲でチェックする
pub fn screen_looks_idle(output: &str) -> bool {
    tail_lines(output, 10).iter().any(|l| {
        let t = l.trim_start();
        t.starts_with('❯') || t.starts_with('›') || t == ">" || t.starts_with("> ")
    })
}

/// TUI が折りたたみ状態（「N new messages (click) ↓」等で途中省略されている）かを検出する。
/// claude TUI は長いツール実行後に出力を折りたたみ、最終的に「95 new messages (click) ↓」
/// のような表示になる。この状態では read_pane / capture-pane のどちらでも最新の会話が取れない
pub fn screen_is_collapsed(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.contains("new messages") && l.contains("click"))
}

/// worker 画面から異常停止パターンを検知する（#157）。
/// 返り値は（種別, マッチした行の trim 済みテキスト）。
/// **idle（停止）確定後の画面に対して使う**こと — busy 中の呼び出しは
/// 自動リトライやツール実行ログへの誤検知を招く。パターンはすべて
/// 実採取画面由来（claude / codex。2026-07-12〜13 の夜間バッチ等）。
///
/// 検知の優先順位は復帰コストの高い順: usage_limit > limit_dialog > api_error
/// （codex は limit 到達時に limit メッセージとモデル切替ダイアログが同時に出るため、
/// 本質である limit 到達を優先する）
pub fn detect_worker_error(output: &str) -> Option<(WorkerErrorKind, String)> {
    // tail_lines は新しい行が先頭 = 最初のマッチが画面最下部に最も近い
    let lines = tail_lines(output, 30);

    // 1. usage limit 到達（claude / codex）
    //    - codex: 「■ You've hit your usage limit. ... try again at 4:24 AM.」
    //    - claude: 「Claude usage limit reached. Your limit will reset at …」
    //    - 「5-hour limit reached ∙ resets 3am」系は limit reached + reset の共起で拾う
    //    - 「Claude Opus 4.6 limit reached, now using …」は自動モデル切替の告知で
    //      worker は停止しないため除外する
    for l in &lines {
        if l.contains("limit reached, now using") {
            continue;
        }
        if l.contains("hit your usage limit")
            || l.contains("usage limit reached")
            || (l.contains("limit reached") && l.contains("reset"))
        {
            return Some((WorkerErrorKind::UsageLimit, l.trim().to_string()));
        }
    }

    // 2. rate limit 起因の選択ダイアログ（codex のモデル切替提案。実採取画面:
    //    「Approaching rate limits / Switch to gpt-… / Press enter to confirm」）。
    //    「Press enter to confirm」単独は通常の確認ダイアログにもあるため使わない
    for l in &lines {
        if l.contains("Approaching rate limits") {
            return Some((WorkerErrorKind::LimitDialog, l.trim().to_string()));
        }
    }

    // 3. API エラー（claude。「API Error: Connection closed mid-response. …」等）。
    //    エラー行はプロンプト直上に出るため末尾 15 行に限定する
    //    （復帰後の作業完了時にスクロールバック上へ残った古いエラー行への誤検知を抑える）。
    //    「Retrying in 4 seconds… (attempt 3/10)」が見えている間は自動リトライ中 =
    //    まだ停止と確定しない
    let tail15 = tail_lines(output, 15);
    if tail15.iter().any(|l| l.contains("Retrying")) {
        return None;
    }
    for l in &tail15 {
        if l.contains("API Error") {
            return Some((WorkerErrorKind::ApiError, l.trim().to_string()));
        }
    }

    None
}

// --- #243: 質問検知・モデル切替検知・context_high ---

/// worker イベントの種別（worker_status の events 配列に載せる。#243）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerEventKind {
    /// worker が質問している（idle + 画面末尾に質問パターン）
    Question,
    /// worker が permission ダイアログで停止している（#319）
    PermissionDialog,
    /// claude の自動モデル切替が発生した
    ModelSwitched { from: String, to: String },
    /// ctx 使用率が閾値（60%）を超えた
    ContextHigh { percent: u64 },
}

/// worker_status の events 配列に載せる 1 イベント
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerEvent {
    pub kind: WorkerEventKind,
}

impl WorkerEvent {
    pub fn to_json(&self) -> Value {
        match &self.kind {
            WorkerEventKind::Question => json!({ "kind": "question" }),
            WorkerEventKind::PermissionDialog => json!({ "kind": "permission_dialog" }),
            WorkerEventKind::ModelSwitched { from, to } => {
                json!({ "kind": "model_switched", "from": from, "to": to })
            }
            WorkerEventKind::ContextHigh { percent } => {
                json!({ "kind": "context_high", "percent": percent })
            }
        }
    }
}

/// worker の画面が「質問している」パターンを含むか検出する（#243）。
/// **idle 確定後の画面に対して使う**こと。busy 中の質問は質問ではなく作業中。
///
/// パターンは実採取画面由来:
/// - claude: 「? 」で始まる行（確認質問）、末尾が「?」の行（自由質問）
/// - claude: 選択肢パターン（「1.」「2.」が連続する行）
/// - claude / codex: 「Should I」「Would you like」「Do you want」等の決まり文句
/// - 誤発火防止: 「? for shortcuts」（agy フッター）は除外
pub fn detect_worker_question(output: &str) -> bool {
    let lines = tail_lines(output, 15);

    for l in &lines {
        let t = l.trim();
        // agy フッターの「? for shortcuts」を除外
        if t.contains("? for shortcuts") {
            continue;
        }
        // 「? 」で始まる行（claude の yes/no 確認）
        if t.starts_with("? ") {
            return true;
        }
    }

    // 質問の決まり文句（末尾 15 行に 1 つでもあれば）
    for l in &lines {
        let t = l.trim();
        if t.contains("Should I ")
            || t.contains("Would you like")
            || t.contains("Do you want")
            || t.contains("Which ")
            || t.contains("Shall I ")
        {
            return true;
        }
        // 末尾が ? または ？ の行（「?」単独はフッター由来の可能性があるので除外）
        if t.len() > 1
            && (t.ends_with('?') || t.ends_with('\u{ff1f}'))
            && !t.contains("? for shortcuts")
        {
            return true;
        }
    }

    // 選択肢パターン: 「1. ...」「2. ...」が末尾 10 行に複数行ある
    let choice_count = tail_lines(output, 10)
        .iter()
        .filter(|l| {
            let t = l.trim().trim_start_matches(['›', ' ']);
            // 「N. テキスト」パターン（N は 1〜9）
            t.len() > 2
                && t.as_bytes()[0].is_ascii_digit()
                && t.as_bytes()[1] == b'.'
                && t.as_bytes()[2] == b' '
        })
        .count();
    if choice_count >= 2 {
        return true;
    }

    false
}

/// 自動モデル切替の告知行を検出し、from/to を返す（#243）。
/// パターン: 「{model} limit reached, now using {model}」
/// 既存の detect_worker_error では除外（worker は止まらない）されていた情報を、
/// events として master に通知する
pub fn detect_model_switched(output: &str) -> Option<(String, String)> {
    let lines = tail_lines(output, 30);
    for l in &lines {
        // 「Claude Opus 4.6 limit reached, now using Claude Sonnet 4.5」
        // 「5-hour limit reached, now using Claude Sonnet 4.5」
        if let Some(pos) = l.find("limit reached, now using") {
            let from = l[..pos].trim().trim_start_matches("⎿ ").trim().to_string();
            let to = l[pos + "limit reached, now using".len()..]
                .trim()
                .to_string();
            if !to.is_empty() {
                return Some((from, to));
            }
        }
    }
    None
}

/// context_high の閾値（%）
pub const CONTEXT_HIGH_THRESHOLD: u32 = 60;

/// worker_status の応答から events 配列を構築する（#243）。
/// dispatch の finish_worker_status から呼ばれる
pub fn collect_worker_events(
    status: &str,
    recent_output: Option<&str>,
    ctx_percent: Option<u32>,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();

    // question: idle / error / waiting 時のみ（busy 中の質問文言はまだ作業途中）
    if status == "idle" || status == "error" || status == "waiting" {
        if let Some(out) = recent_output {
            if detect_worker_question(out) {
                events.push(WorkerEvent {
                    kind: WorkerEventKind::Question,
                });
            }
        }
    }

    // permission_dialog: waiting 時に画面に permission ダイアログがあれば記録（#319）
    if status == "waiting" {
        if let Some(out) = recent_output {
            let lines: Vec<String> = out.lines().map(|l| l.to_string()).collect();
            if crate::claude_tui::detect_permission_dialog(&lines).is_some() {
                events.push(WorkerEvent {
                    kind: WorkerEventKind::PermissionDialog,
                });
            }
        }
    }

    // model_switched: status に関係なく画面に告知があれば記録
    if let Some(out) = recent_output {
        if let Some((from, to)) = detect_model_switched(out) {
            events.push(WorkerEvent {
                kind: WorkerEventKind::ModelSwitched { from, to },
            });
        }
    }

    // context_high: 閾値超えなら
    if let Some(pct) = ctx_percent {
        if pct >= CONTEXT_HIGH_THRESHOLD {
            events.push(WorkerEvent {
                kind: WorkerEventKind::ContextHigh {
                    percent: pct as u64,
                },
            });
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[test]
    fn run_registryは実行中を残して完了履歴を上限まで削減する() {
        let mut registry = RunRegistry::new();
        for index in 0..(MAX_COMPLETED_RUNS + 4) {
            registry.entries.insert(
                format!("done-{index}"),
                RunEntry {
                    pane_id: index as u64,
                    spawned_by: 0,
                    tmux_session: None,
                    auto_close: false,
                    output_lines: 1,
                    started_at: Instant::now(),
                    snapshot: Arc::new(Mutex::new(RunSnapshot {
                        worker_status: "idle".into(),
                        elapsed_secs: 0,
                    })),
                    completed: Arc::new(Mutex::new(Some(RunCompleted {
                        outcome: WatchOutcome::Idle { ctx_percent: None },
                        elapsed_secs: 0,
                    }))),
                },
            );
        }
        registry.entries.insert(
            "running".into(),
            RunEntry {
                pane_id: 999,
                spawned_by: 0,
                tmux_session: None,
                auto_close: false,
                output_lines: 1,
                started_at: Instant::now(),
                snapshot: Arc::new(Mutex::new(RunSnapshot {
                    worker_status: "busy".into(),
                    elapsed_secs: 0,
                })),
                completed: Arc::new(Mutex::new(None)),
            },
        );

        registry.prune_completed();

        assert_eq!(registry.entries.len(), MAX_COMPLETED_RUNS + 1);
        assert!(registry.entries.contains_key("running"));
    }

    /// ❯ プロンプトつきの idle 画面
    const IDLE_SCREEN: &str = "作業が完了しました\n❯ \n──────\nmodel: opus";
    /// busy パターンつきの画面
    const BUSY_SCREEN: &str = "Thinking…\nesc to interrupt";

    /// 応答列を順に返す exec モック。受け取った Request も記録する
    struct ExecScript {
        responses: VecDeque<Result<Value, String>>,
        seen: Vec<Request>,
    }

    impl ExecScript {
        fn new(responses: Vec<Result<Value, String>>) -> Self {
            Self {
                responses: responses.into(),
                seen: Vec::new(),
            }
        }
    }

    fn status(status: &str, recent: &str, source: &str) -> Result<Value, String> {
        Ok(json!({
            "status": status,
            "recent_output": recent,
            "status_source": source,
            "ctx_percent": 42,
        }))
    }

    fn watch_opts(pane_id: u64, timeout: Option<Duration>) -> WatchOptions {
        WatchOptions {
            pane_id,
            session_id: None,
            // None にして実 tmux への生存確認をスキップする（単体テスト用）
            tmux_session: None,
            timeout,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
        }
    }

    fn run_wait(script: &mut ExecScript, opts: &WatchOptions) -> WatchOutcome {
        let mut exec = |req: Request| {
            script.seen.push(req);
            script
                .responses
                .pop_front()
                .expect("スクリプトの応答が尽きた")
        };
        wait_for_worker(&mut exec, opts, None)
    }

    #[test]
    fn agentsソースのidleは3回連続で完了と判定する() {
        let mut script = ExecScript::new(vec![
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert_eq!(
            outcome,
            WatchOutcome::Idle {
                ctx_percent: Some(42)
            }
        );
        assert_eq!(script.seen.len(), 3);
    }

    #[test]
    fn 画面推定のidleは8回連続を要求する() {
        let mut responses: Vec<_> = (0..8)
            .map(|_| status("unknown", IDLE_SCREEN, "screen"))
            .collect();
        responses.push(status("unknown", IDLE_SCREEN, "screen")); // 予備（呼ばれないはず）
        let mut script = ExecScript::new(responses);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(matches!(outcome, WatchOutcome::Idle { .. }));
        assert_eq!(script.seen.len(), 8);
    }

    #[test]
    fn busy画面はidle判定を取り消す() {
        // idle(busy 画面) が挟まると streak が振り出しに戻る
        let mut script = ExecScript::new(vec![
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", BUSY_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(matches!(outcome, WatchOutcome::Idle { .. }));
        assert_eq!(script.seen.len(), 6);
    }

    #[test]
    fn goneは3回連続で消滅と判定する() {
        // #267: 閾値を 2→3 に引き上げ（一時的な IPC 断での偽 GONE 防止）
        let mut script = ExecScript::new(vec![
            status("gone", "", "none"),
            status("gone", "", "none"),
            status("gone", "", "none"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert_eq!(outcome, WatchOutcome::Gone);
    }

    #[test]
    fn 実行エラーも3回連続でgoneと判定する() {
        let mut script = ExecScript::new(vec![
            Err("IPC 断".into()),
            Err("IPC 断".into()),
            Err("IPC 断".into()),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert_eq!(outcome, WatchOutcome::Gone);
    }

    #[test]
    fn タイムアウトで打ち切る() {
        let mut script = ExecScript::new(vec![]);
        let outcome = run_wait(&mut script, &watch_opts(7, Some(Duration::ZERO)));
        assert_eq!(outcome, WatchOutcome::Timeout);
        assert!(script.seen.is_empty());
    }

    #[test]
    fn run_workerはreadのtextをoutputへ写す() {
        // #82 回帰テスト: dispatch の Read 応答（{"pane","text"}）の text が
        // output に入ること。spawn → idle×3 → read → close の順で応答を組む
        let mut script = ExecScript::new(vec![
            Ok(json!({ "pane_id": 9, "spawned_by": 1, "tmux_session": "tako-w9" })),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            Ok(json!({ "pane": 9, "text": "worker の成果テキスト" })),
            Ok(json!({ "closed": 9 })),
        ]);
        let opts = RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::from_secs(60),
            auto_close: true,
            output_lines: 200,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
            task_type: None,
        };
        let mut spawned = None;
        let result = {
            let mut exec = |req: Request| {
                script.seen.push(req);
                script
                    .responses
                    .pop_front()
                    .expect("スクリプトの応答が尽きた")
            };
            run_worker(&mut exec, &opts, &mut |pane, tmux| {
                spawned = Some((pane, tmux.map(String::from)));
            })
            .expect("run_worker は成功する")
        };
        assert_eq!(result["status"], "completed");
        assert_eq!(result["output"], "worker の成果テキスト");
        assert_eq!(result["pane_id"], 9);
        assert_eq!(result["closed"], true);
        assert_eq!(spawned, Some((9, Some("tako-w9".to_string()))));
        // Read は spawn の tmux_session を引き継ぎ、末尾行数を渡す
        assert!(script.seen.iter().any(|r| matches!(
            r,
            Request::Read { pane: Some(9), lines: Some(200), tmux_session: Some(s) } if s == "tako-w9"
        )));
        // 完了後は force close
        assert!(script.seen.iter().any(|r| matches!(
            r,
            Request::Close {
                pane: Some(9),
                force: true
            }
        )));
    }

    #[test]
    fn run_workerはタイムアウト時も途中結果を返す() {
        let mut script = ExecScript::new(vec![
            Ok(json!({ "pane_id": 9, "spawned_by": 1, "tmux_session": null })),
            Ok(json!({ "pane": 9, "text": "途中経過" })),
            Ok(json!({ "closed": 9 })),
        ]);
        let opts = RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::ZERO,
            auto_close: true,
            output_lines: 50,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
            task_type: None,
        };
        let result = {
            let mut exec = |req: Request| {
                script.seen.push(req);
                script
                    .responses
                    .pop_front()
                    .expect("スクリプトの応答が尽きた")
            };
            run_worker(&mut exec, &opts, &mut |_, _| {}).expect("成功する")
        };
        assert_eq!(result["status"], "timeout");
        assert_eq!(result["output"], "途中経過");
    }

    #[test]
    fn 画面判定ヒューリスティックはフッター越しのプロンプトを拾う() {
        assert!(screen_looks_idle(IDLE_SCREEN));
        assert!(!screen_looks_idle("Thinking…\nまだ作業中"));
        assert!(screen_looks_busy(BUSY_SCREEN));
        assert!(!screen_looks_busy(IDLE_SCREEN));
        // 末尾 5 行より前の busy パターンは無視される
        let old_busy = format!("esc to interrupt\n{}", "行\n".repeat(6));
        assert!(!screen_looks_busy(&old_busy));
    }

    /// codex 0.144.1 の実採取画面（Issue #120）
    const CODEX_IDLE_SCREEN: &str =
        "• DONE_PROBE\n› Summarize recent commits\n  gpt-5.6-sol high · /work/dir";
    const CODEX_BUSY_SCREEN: &str = "• Working (3s • esc to interrupt) · 1 background terminal running\n› Summarize recent commits\n  gpt-5.6-sol high · /work/dir";

    /// agy 1.1.0 の実採取画面（Issue #120）。フッターのモデル名表記
    /// 「Claude Opus 4.6 (Thinking)」は**常時表示**のため、busy 判定が
    /// これに誤爆しないことが完了検知の生命線（実機検証 2026-07-10 で発見した回帰）
    const AGY_IDLE_SCREEN: &str =
        "● Bash(echo done)\n  完了しました\n────\n>\n────\n? for shortcuts   Claude Opus 4.6 (Thinking)";
    const AGY_BUSY_SCREEN: &str =
        "▸ Thought Process\n⣻  Generating...\n────\n>\n────\nesc to cancel   Claude Opus 4.6 (Thinking)";

    #[test]
    fn codexとagyの画面判定() {
        assert!(screen_looks_idle(CODEX_IDLE_SCREEN), "codex の › を拾う");
        assert!(!screen_looks_busy(CODEX_IDLE_SCREEN));
        assert!(screen_looks_busy(CODEX_BUSY_SCREEN), "Working ( を拾う");

        assert!(screen_looks_idle(AGY_IDLE_SCREEN), "agy の > 単独行を拾う");
        assert!(
            !screen_looks_busy(AGY_IDLE_SCREEN),
            "フッターのモデル名 (Thinking) に誤爆しない"
        );
        assert!(
            screen_looks_busy(AGY_BUSY_SCREEN),
            "Generating / esc to cancel を拾う"
        );
        // busy 中も > は見えるが busy 判定が優先される（wait_for_worker の構造）
        assert!(screen_looks_idle(AGY_BUSY_SCREEN));
        // claude スピナーの実表示（Thinking…）は引き続き busy と判定する
        assert!(screen_looks_busy("✻ Thinking…\n出力中"));
    }

    #[test]
    fn ascii山括弧はリダイレクト行を入力欄と誤認しない() {
        assert!(!screen_looks_idle("$ echo foo >file\ndone"));
        assert!(!screen_looks_idle(">>append"));
        assert!(screen_looks_idle("some output\n> "));
    }

    // --- #157: 異常検知（detect_worker_error / WatchOutcome::Error） ---

    /// claude の API エラー停止画面（2026-07-12〜13 夜間バッチの実採取メッセージ）
    const API_ERROR_SCREEN: &str = "⏺ 実装を続けます\n\n  ⎿  API Error: Connection closed mid-response. The response above may be incomplete.\n\n──────\n❯ \n──────\n  ctx 42%";
    /// claude の API エラー自動リトライ中（まだ停止と確定しない）
    const API_RETRYING_SCREEN: &str =
        "  ⎿  API Error (Connection error.) · Retrying in 4 seconds… (attempt 3/10)\n\n❯ \n──────";
    /// codex の usage limit 停止画面（実採取。limit メッセージ + モデル切替ダイアログ同時表示）
    const CODEX_LIMIT_SCREEN: &str = "■ You've hit your usage limit. Upgrade to Pro\n(https://chatgpt.com/explore/pro), visit\nhttps://chatgpt.com/codex/settings/usage to purchase more credits or\ntry again at 4:24 AM.\n\n\n  Approaching rate limits\n  Switch to gpt-5.4-mini for lower credit usage?\n\n› 1. Switch to gpt-5.4-mini\n  2. Keep current model\n  3. Keep current model (never show again)\n\n  Press enter to confirm or esc to go back";
    /// claude の usage limit 停止画面（旧形式の文言）
    const CLAUDE_LIMIT_SCREEN: &str =
        "Claude usage limit reached. Your limit will reset at 4pm (Asia/Tokyo).\n\n❯ \n──────";

    #[test]
    fn detect_worker_errorはapiエラー停止を検知する() {
        let (kind, detail) = detect_worker_error(API_ERROR_SCREEN).expect("検知される");
        assert_eq!(kind, WorkerErrorKind::ApiError);
        assert!(detail.contains("API Error: Connection closed mid-response"));
        assert_eq!(kind.as_str(), "api_error");
        assert_eq!(kind.recommended_action(), "resume");
    }

    #[test]
    fn detect_worker_errorは自動リトライ中を停止と確定しない() {
        assert_eq!(detect_worker_error(API_RETRYING_SCREEN), None);
    }

    #[test]
    fn detect_worker_errorはusage_limitをダイアログより優先する() {
        // codex は limit 到達時に limit メッセージと切替ダイアログが同時に出る。
        // 本質は limit 到達なので usage_limit として報告する
        let (kind, detail) = detect_worker_error(CODEX_LIMIT_SCREEN).expect("検知される");
        assert_eq!(kind, WorkerErrorKind::UsageLimit);
        assert!(detail.contains("hit your usage limit"));
        assert_eq!(kind.recommended_action(), "wait_reset");

        let (kind, _) = detect_worker_error(CLAUDE_LIMIT_SCREEN).expect("検知される");
        assert_eq!(kind, WorkerErrorKind::UsageLimit);
    }

    #[test]
    fn detect_worker_errorはダイアログ単独をlimit_dialogとして検知する() {
        let dialog_only = "  Approaching rate limits\n  Switch to gpt-5.4-mini for lower credit usage?\n\n› 1. Switch to gpt-5.4-mini\n  2. Keep current model\n\n  Press enter to confirm or esc to go back";
        let (kind, _) = detect_worker_error(dialog_only).expect("検知される");
        assert_eq!(kind, WorkerErrorKind::LimitDialog);
        assert_eq!(kind.recommended_action(), "respond_dialog");
    }

    #[test]
    fn detect_worker_errorは正常画面と自動モデル切替に誤検知しない() {
        // 正常な完了画面
        assert_eq!(detect_worker_error(IDLE_SCREEN), None);
        assert_eq!(detect_worker_error(CODEX_IDLE_SCREEN), None);
        assert_eq!(detect_worker_error(AGY_IDLE_SCREEN), None);
        // 自動モデル切替の告知（worker は止まらない）
        assert_eq!(
            detect_worker_error(
                "⎿ Claude Opus 4.6 limit reached, now using Claude Sonnet 4.5\n\n❯ \n──────"
            ),
            None
        );
        // 「Press enter to confirm」単独（通常の確認ダイアログ）
        assert_eq!(
            detect_worker_error("Do you trust this folder?\n❯ 1. Yes\n  Press enter to confirm"),
            None
        );
    }

    #[test]
    fn detect_worker_errorは復帰後にスクロールバックへ流れた古いapiエラーを無視する() {
        // エラー行の後に 15 行以上の新しい出力 → 末尾 15 行から外れて検知しない
        let recovered = format!(
            "  ⎿  API Error: Connection closed mid-response. The response above may be incomplete.\n{}❯ \n──────",
            "後続の作業出力行\n".repeat(15)
        );
        assert_eq!(detect_worker_error(&recovered), None);
    }

    #[test]
    fn watchはエラー停止でworker_errorを返す() {
        // dispatch が status="error" + error オブジェクトを返す（新 tako-app 経路）。
        // agents ソース相当なので 3 回で確定
        let error_resp = || -> Result<Value, String> {
            Ok(json!({
                "status": "error",
                "recent_output": API_ERROR_SCREEN,
                "status_source": "agents-auto",
                "ctx_percent": 42,
                "error": {
                    "kind": "api_error",
                    "detail": "API Error: Connection closed mid-response. The response above may be incomplete.",
                    "recommended_action": "resume",
                },
            }))
        };
        let mut script = ExecScript::new(vec![error_resp(), error_resp(), error_resp()]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        match outcome {
            WatchOutcome::Error { kind, detail } => {
                assert_eq!(kind, WorkerErrorKind::ApiError);
                assert!(detail.contains("Connection closed mid-response"));
            }
            other => panic!("Error になるはず: {other:?}"),
        }
        assert_eq!(script.seen.len(), 3);
    }

    #[test]
    fn watchは旧appのidle応答でも画面からエラーを自力検知する() {
        // 旧 tako-app は error 判定を知らず status="idle" を返す。
        // watch 側の detect_worker_error フォールバックで WORKER_ERROR になる
        let mut script = ExecScript::new(vec![
            status("idle", API_ERROR_SCREEN, "agents"),
            status("idle", API_ERROR_SCREEN, "agents"),
            status("idle", API_ERROR_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(matches!(
            outcome,
            WatchOutcome::Error {
                kind: WorkerErrorKind::ApiError,
                ..
            }
        ));
    }

    #[test]
    fn watchは正常idleでエラーを誤発火しない() {
        // 受け入れ条件 3: 既存 WORKER_IDLE の挙動が壊れていない
        let mut script = ExecScript::new(vec![
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert_eq!(
            outcome,
            WatchOutcome::Idle {
                ctx_percent: Some(42)
            }
        );
    }

    #[test]
    fn run_workerはエラー停止でworker_errorを返しauto_closeしない() {
        let mut script = ExecScript::new(vec![
            Ok(json!({ "pane_id": 9, "spawned_by": 1, "tmux_session": null })),
            status("error", API_ERROR_SCREEN, "agents"),
            status("error", API_ERROR_SCREEN, "agents"),
            status("error", API_ERROR_SCREEN, "agents"),
            Ok(json!({ "pane": 9, "text": "エラー直前までの出力" })),
            // auto_close: true でも Close は呼ばれない（応答を積まないことで検証）
        ]);
        let opts = RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::from_secs(60),
            auto_close: true,
            output_lines: 200,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
            task_type: None,
        };
        let result = {
            let mut exec = |req: Request| {
                script.seen.push(req);
                script
                    .responses
                    .pop_front()
                    .expect("スクリプトの応答が尽きた")
            };
            run_worker(&mut exec, &opts, &mut |_, _| {}).expect("成功する")
        };
        assert_eq!(result["status"], "worker_error");
        assert_eq!(result["error"]["kind"], "api_error");
        assert_eq!(result["error"]["recommended_action"], "resume");
        assert_eq!(result["closed"], false);
        assert!(
            !script
                .seen
                .iter()
                .any(|r| matches!(r, Request::Close { .. })),
            "エラー停止時は auto_close しない"
        );
    }

    // --- #121: 非同期 run レジストリのテスト ---

    #[test]
    fn run_startは即座にrun_idを返す() {
        let spawn_resp = Ok(json!({ "pane_id": 42, "spawned_by": 1, "tmux_session": "tako-w42" }));
        let script = std::sync::Arc::new(std::sync::Mutex::new(ExecScript::new(vec![spawn_resp])));
        let script_for_factory = script.clone();

        let opts = RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::from_millis(100),
            auto_close: true,
            output_lines: 50,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
            task_type: None,
        };

        let result = {
            let mut s = script.lock().unwrap();
            let mut exec = |req: Request| {
                s.seen.push(req);
                s.responses.pop_front().expect("応答が尽きた")
            };
            run_start(&mut exec, &opts, move || {
                // ポーリング用: すぐに idle を返す
                let idle_responses: Vec<Result<Value, String>> = (0..3)
                    .map(|_| status("idle", IDLE_SCREEN, "agents"))
                    .collect();
                let inner_script = std::sync::Arc::clone(&script_for_factory);
                let mut responses: VecDeque<Result<Value, String>> = idle_responses.into();
                Box::new(move |_req: Request| -> Result<Value, String> {
                    let _ = inner_script; // keep alive
                    responses.pop_front().unwrap_or(Ok(json!({})))
                })
            })
            .expect("run_start は成功する")
        };

        assert_eq!(result["pane_id"], 42);
        assert!(result["run_id"].as_str().unwrap().starts_with("run-"));
        assert_eq!(result["tmux_session"], "tako-w42");
    }

    #[test]
    fn run_statusはrunning中にphase_runningを返す() {
        let spawn_resp = Ok(json!({ "pane_id": 99, "spawned_by": 1, "tmux_session": null }));
        let opts = RunOptions {
            project: "demo".into(),
            prompt: "test".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::from_secs(60),
            auto_close: true,
            output_lines: 50,
            initial_delay: Duration::from_secs(9999),
            interval: Duration::from_secs(9999),
            task_type: None,
        };

        let result = {
            let mut responses: VecDeque<Result<Value, String>> = vec![spawn_resp].into();
            let mut exec = |_: Request| responses.pop_front().unwrap_or(Ok(json!({})));
            run_start(&mut exec, &opts, || {
                Box::new(|_: Request| -> Result<Value, String> { Ok(json!({})) })
            })
            .unwrap()
        };
        let run_id = result["run_id"].as_str().unwrap();

        // initial_delay が 9999 秒なのでまだ running
        std::thread::sleep(Duration::from_millis(10));
        let status = run_status(run_id).unwrap();
        assert_eq!(status["phase"], "running");
        assert_eq!(status["pane_id"], 99);
    }

    #[test]
    fn run_listは全run一覧を返す() {
        let list = run_list();
        assert!(list["runs"].is_array());
    }

    // --- #224: stalled 検出 ---

    fn stalled_resp() -> Result<Value, String> {
        Ok(json!({
            "status": "stalled",
            "recent_output": "❯ \n──────\nmodel: opus",
            "status_source": "agents-auto",
            "ctx_percent": 42,
            "stalled": {
                "detail": "busy だが実行中の子プロセスが無く、画面の busy パターンも無い",
                "recommended_action": "check_and_resume",
            },
        }))
    }

    #[test]
    fn stalledは3回連続で確定する() {
        let mut script = ExecScript::new(vec![stalled_resp(), stalled_resp(), stalled_resp()]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        match outcome {
            WatchOutcome::Stalled { detail } => {
                assert!(detail.contains("子プロセス"), "detail に根拠が含まれる");
            }
            other => panic!("Stalled になるはず: {other:?}"),
        }
        assert_eq!(script.seen.len(), 3);
    }

    #[test]
    fn stalledはbusyで中断してリセットされる() {
        let mut script = ExecScript::new(vec![
            stalled_resp(),
            stalled_resp(),
            status("busy", BUSY_SCREEN, "agents"),
            stalled_resp(),
            stalled_resp(),
            stalled_resp(),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(matches!(outcome, WatchOutcome::Stalled { .. }));
        assert_eq!(script.seen.len(), 6, "busy で中断後に 3 回で確定");
    }

    #[test]
    fn stalledはidleで中断してリセットされる() {
        let mut script = ExecScript::new(vec![
            stalled_resp(),
            stalled_resp(),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(outcome, WatchOutcome::Idle { .. }),
            "idle の streak が優先して確定する"
        );
    }

    // --- #224: 折りたたみ検出 ---

    #[test]
    fn 折りたたみ画面を検出する() {
        assert!(screen_is_collapsed(
            "⏺ some output\n\n95 new messages (click) ↓\n\n❯ \n──────"
        ));
        assert!(!screen_is_collapsed(IDLE_SCREEN));
        assert!(!screen_is_collapsed(BUSY_SCREEN));
    }

    // --- #224: run_worker で stalled を扱う ---

    #[test]
    fn run_workerはstalledでworker_stalledを返しauto_closeしない() {
        let mut script = ExecScript::new(vec![
            Ok(json!({ "pane_id": 9, "spawned_by": 1, "tmux_session": null })),
            stalled_resp(),
            stalled_resp(),
            stalled_resp(),
            Ok(json!({ "pane": 9, "text": "停滞中の出力" })),
        ]);
        let opts = RunOptions {
            project: "demo".into(),
            prompt: "やって".into(),
            label: None,
            model: None,
            effort: None,
            agent: None,
            pane: Some(1),
            tab: None,
            caller_role: None,
            timeout: Duration::from_secs(60),
            auto_close: true,
            output_lines: 200,
            initial_delay: Duration::ZERO,
            interval: Duration::ZERO,
            task_type: None,
        };
        let result = {
            let mut exec = |req: Request| {
                script.seen.push(req);
                script
                    .responses
                    .pop_front()
                    .expect("スクリプトの応答が尽きた")
            };
            run_worker(&mut exec, &opts, &mut |_, _| {}).expect("成功する")
        };
        assert_eq!(result["status"], "worker_stalled");
        assert_eq!(result["stalled"]["recommended_action"], "check_and_resume");
        assert_eq!(result["closed"], false);
        assert!(
            !script
                .seen
                .iter()
                .any(|r| matches!(r, Request::Close { .. })),
            "stalled 時は auto_close しない"
        );
    }

    // --- #243: 質問検知・モデル切替検知・context_high ---

    /// claude が質問している画面（実採取。AskUserQuestion 相当の選択肢表示）
    const QUESTION_CHOICE_SCREEN: &str = "\
このリポジトリにはテストが見つかりませんでした。\n\
テストを追加しますか？\n\n\
❯ 1. はい、ユニットテストを追加する\n\
  2. いいえ、スキップする\n\
  3. 後で自分で追加する\n\n\
❯ \n──────\n  ctx 42%";

    /// claude が自由形式の質問をしている画面（実採取。全角？で終わる行）
    const QUESTION_FREE_SCREEN: &str = "\
実装を進めるにあたり確認があります。\n\n\
データベースのマイグレーションは先に実行しておくべきですか\u{ff1f}\n\n\
❯ \n──────\n  ctx 28%";

    /// claude の ? で始まる確認質問（実採取。信頼確認ダイアログ相当）
    const QUESTION_CONFIRM_SCREEN: &str = "\
? Do you trust the files in this folder?\n\
❯ 1. Yes\n\
  2. No\n\n\
  Press enter to confirm";

    /// claude の Should I 質問パターン
    const QUESTION_SHOULD_SCREEN: &str = "\
変更をコミットしました。\n\n\
Should I also update the documentation?\n\n\
❯ \n──────";

    /// 自動モデル切替の告知画面（実採取。worker は停止しない）
    const MODEL_SWITCHED_SCREEN: &str = "\
⎿ Claude Opus 4.6 limit reached, now using Claude Sonnet 4.5\n\n\
⏺ 続けて実装します\n\n\
❯ \n──────\n  ctx 65%";

    /// 正常完了画面（質問なし。既存 IDLE_SCREEN と同等）
    const NORMAL_DONE_SCREEN: &str = "\
実装が完了しました。変更をコミットし、テストも全緑です。\n\n\
❯ \n──────\nmodel: opus · ctx 42%";

    #[test]
    fn detect_worker_questionは選択肢パターンを検知する() {
        assert!(detect_worker_question(QUESTION_CHOICE_SCREEN));
    }

    #[test]
    fn detect_worker_questionは自由形式の質問を検知する() {
        assert!(detect_worker_question(QUESTION_FREE_SCREEN));
    }

    #[test]
    fn detect_worker_questionは確認質問を検知する() {
        assert!(detect_worker_question(QUESTION_CONFIRM_SCREEN));
    }

    #[test]
    fn detect_worker_questionはshould_iパターンを検知する() {
        assert!(detect_worker_question(QUESTION_SHOULD_SCREEN));
    }

    #[test]
    fn detect_worker_questionは正常完了画面で誤発火しない() {
        assert!(!detect_worker_question(NORMAL_DONE_SCREEN));
        assert!(!detect_worker_question(IDLE_SCREEN));
        assert!(!detect_worker_question(CODEX_IDLE_SCREEN));
    }

    #[test]
    fn detect_worker_questionはagyフッターの問号に誤発火しない() {
        assert!(!detect_worker_question(AGY_IDLE_SCREEN));
        assert!(!detect_worker_question(AGY_BUSY_SCREEN));
    }

    #[test]
    fn detect_worker_questionはbusy画面で誤発火しない() {
        assert!(!detect_worker_question(BUSY_SCREEN));
    }

    #[test]
    fn detect_model_switchedはモデル切替を検出する() {
        let (from, to) = detect_model_switched(MODEL_SWITCHED_SCREEN).expect("検知される");
        assert_eq!(from, "Claude Opus 4.6");
        assert_eq!(to, "Claude Sonnet 4.5");
    }

    #[test]
    fn detect_model_switchedは5h_limit形式も検出する() {
        let screen = "5-hour limit reached, now using Claude Sonnet 4.5\n\n❯ \n──────";
        let (from, to) = detect_model_switched(screen).expect("検知される");
        assert_eq!(from, "5-hour");
        assert_eq!(to, "Claude Sonnet 4.5");
    }

    #[test]
    fn detect_model_switchedは正常画面で誤発火しない() {
        assert_eq!(detect_model_switched(IDLE_SCREEN), None);
        assert_eq!(detect_model_switched(CODEX_IDLE_SCREEN), None);
        assert_eq!(detect_model_switched(AGY_IDLE_SCREEN), None);
        assert_eq!(detect_model_switched(NORMAL_DONE_SCREEN), None);
    }

    #[test]
    fn collect_worker_eventsは質問イベントを返す() {
        let events = collect_worker_events("idle", Some(QUESTION_CHOICE_SCREEN), Some(42u32));
        assert!(events.iter().any(|e| e.kind == WorkerEventKind::Question));
        assert!(!events
            .iter()
            .any(|e| matches!(e.kind, WorkerEventKind::ContextHigh { .. })));
    }

    #[test]
    fn collect_worker_eventsはbusy時に質問を検出しない() {
        let events = collect_worker_events("busy", Some(QUESTION_CHOICE_SCREEN), Some(42u32));
        assert!(!events.iter().any(|e| e.kind == WorkerEventKind::Question));
    }

    #[test]
    fn collect_worker_eventsはモデル切替を返す() {
        let events = collect_worker_events("idle", Some(MODEL_SWITCHED_SCREEN), Some(65u32));
        assert!(events
            .iter()
            .any(|e| matches!(&e.kind, WorkerEventKind::ModelSwitched { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e.kind, WorkerEventKind::ContextHigh { percent: 65 })));
    }

    #[test]
    fn collect_worker_eventsはcontext_highを返す() {
        let events = collect_worker_events("idle", Some(IDLE_SCREEN), Some(75u32));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].kind,
            WorkerEventKind::ContextHigh { percent: 75 }
        ));
    }

    #[test]
    fn collect_worker_eventsは閾値未満でcontext_highを返さない() {
        let events = collect_worker_events("idle", Some(IDLE_SCREEN), Some(59u32));
        assert!(events.is_empty());
    }

    #[test]
    fn collect_worker_eventsは正常完了でイベントなし() {
        let events = collect_worker_events("idle", Some(NORMAL_DONE_SCREEN), Some(42u32));
        assert!(events.is_empty());
    }

    #[test]
    fn collect_worker_eventsは複数イベントを同時に返す() {
        let events = collect_worker_events("idle", Some(MODEL_SWITCHED_SCREEN), Some(65u32));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn collect_worker_eventsはoutput_noneで空() {
        let events: Vec<WorkerEvent> = collect_worker_events("idle", None, None);
        assert!(events.is_empty());
    }

    #[test]
    fn worker_eventのto_jsonは期待する形式を出力する() {
        let q = WorkerEvent {
            kind: WorkerEventKind::Question,
        };
        assert_eq!(q.to_json()["kind"], "question");

        let ms = WorkerEvent {
            kind: WorkerEventKind::ModelSwitched {
                from: "Opus".into(),
                to: "Sonnet".into(),
            },
        };
        let j = ms.to_json();
        assert_eq!(j["kind"], "model_switched");
        assert_eq!(j["from"], "Opus");
        assert_eq!(j["to"], "Sonnet");

        let ch = WorkerEvent {
            kind: WorkerEventKind::ContextHigh { percent: 70 },
        };
        let j = ch.to_json();
        assert_eq!(j["kind"], "context_high");
        assert_eq!(j["percent"], 70);
    }

    // --- #267: watch 誤検知修正のテスト ---

    /// ツール実行中の画面（Docker latexmk）。"Running" が末尾 5 行より上に流れた状態。
    /// 実採取: 長い出力で busy パターンが tail 5 から外れるケース
    const TOOL_RUNNING_SCROLLED_SCREEN: &str = "\
Running 1 shell command...\n\
\n\
  $ docker run --rm -v \"$(pwd):/work\" texlive:latest latexmk -pdf main.tex\n\
\n\
  This is XeTeX, Version 3.141592653\n\
  Output written on main.pdf (10 pages)\n\
  Latexmk: All targets (main.pdf) are up to date\n\
  Transcript written on main.log\n\
  Build complete.\n\
  Done.";

    /// permission ダイアログ待ちの画面（実採取相当）
    const PERMISSION_DIALOG_SCREEN: &str = "\
? Claude requested permissions to write to .../main.aux\n\
  (suspicious Windows path pattern)\n\
❯ 1. Allow once\n\
  2. Always allow\n\
  3. Deny\n\n\
  Press enter to confirm";

    #[test]
    fn 症状2_ツール実行中に偽idleを出さない() {
        // dispatch が "busy"（has_children=true）を返す場面を再現。
        // screen_looks_busy が tail 5 で Running を拾えなくても dispatch が busy なら OK
        let mut script = ExecScript::new(vec![
            status("busy", TOOL_RUNNING_SCROLLED_SCREEN, "agents-auto"),
            status("busy", TOOL_RUNNING_SCROLLED_SCREEN, "agents-auto"),
            status("busy", TOOL_RUNNING_SCROLLED_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(outcome, WatchOutcome::Idle { .. }),
            "busy 中は idle_streak が上がらず、idle 3 連続で初めて完了する"
        );
        assert_eq!(script.seen.len(), 6);
    }

    #[test]
    fn 症状2_screen_looks_busyはtail5より上のrunningを拾えない() {
        // busy パターンが tail 5 から外れた画面で screen_looks_busy が false になることを確認
        // （このため dispatch 側の正規化が必要）
        assert!(
            !screen_looks_busy(TOOL_RUNNING_SCROLLED_SCREEN),
            "tail 5 に Running が入っていないため false"
        );
    }

    #[test]
    fn 症状3_waitingステータスはidle_streakを加算しない() {
        // dispatch が "waiting" を返す場面。idle_streak が上がらないことを確認
        let mut script = ExecScript::new(vec![
            status("waiting", PERMISSION_DIALOG_SCREEN, "agents-auto"),
            status("waiting", PERMISSION_DIALOG_SCREEN, "agents-auto"),
            status("waiting", PERMISSION_DIALOG_SCREEN, "agents-auto"),
            status("waiting", PERMISSION_DIALOG_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
            status("idle", IDLE_SCREEN, "agents-auto"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(outcome, WatchOutcome::Idle { .. }),
            "waiting 中は idle_streak が加算されず、waiting 解消後の idle 3 連続で完了"
        );
        assert_eq!(script.seen.len(), 7);
    }

    #[test]
    fn 症状3_questionイベント付きidleはquestionを返す() {
        // dispatch が idle + events に question を含む応答を返す場面
        let question_resp = || -> Result<Value, String> {
            Ok(json!({
                "status": "idle",
                "recent_output": QUESTION_CONFIRM_SCREEN,
                "status_source": "agents-auto",
                "ctx_percent": 42,
                "events": [{"kind": "question"}],
            }))
        };
        let mut script = ExecScript::new(vec![question_resp(), question_resp(), question_resp()]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(
                outcome,
                WatchOutcome::Question {
                    ctx_percent: Some(42)
                }
            ),
            "events に question があれば Question を返す: {outcome:?}"
        );
    }

    #[test]
    fn 症状4_has_promptなしでもagents_idleで完了を検出する() {
        // agents が "idle" を返すが画面にプロンプトが見えない場面。
        // 旧実装では !has_prompt で busy に上書きされ完了を拾えなかった
        // → dispatch 側の修正で idle のまま通る（テストは dispatch 経由でなく
        //   watch ループの idle 解釈を検証。dispatch 側は dispatch テストで担保）
        let no_prompt_screen = "実装が完了しました\n\n95 new messages (click)";
        let mut script = ExecScript::new(vec![
            status("idle", no_prompt_screen, "agents-auto"),
            status("idle", no_prompt_screen, "agents-auto"),
            status("idle", no_prompt_screen, "agents-auto"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(outcome, WatchOutcome::Idle { .. }),
            "dispatch が idle を返せば watch は素直に idle_streak を加算する"
        );
    }

    #[test]
    fn 症状1_gone2連続ではまだ発火しない() {
        // #267: 閾値 3。2 連続の後に busy に戻れば gone しない
        let mut script = ExecScript::new(vec![
            status("gone", "", "none"),
            status("gone", "", "none"),
            status("busy", BUSY_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
            status("idle", IDLE_SCREEN, "agents"),
        ]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert!(
            matches!(outcome, WatchOutcome::Idle { .. }),
            "2 連続の gone で発火せず、復帰後に idle で完了する"
        );
    }

    #[test]
    fn collect_worker_eventsはwaiting時も質問を検出する() {
        let events = collect_worker_events("waiting", Some(QUESTION_CONFIRM_SCREEN), Some(42u32));
        assert!(events.iter().any(|e| e.kind == WorkerEventKind::Question));
    }
}

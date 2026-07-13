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
    /// worker が異常（API エラー・usage limit 等）で停止した（#157）。
    /// `detail` は検知パターンにマッチした画面上の行
    Error {
        kind: WorkerErrorKind,
        detail: String,
    },
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
/// 「worker が止まっている」ことに変わりはないため idle と同じ streak で確定する
pub fn wait_for_worker(exec: Exec, opts: &WatchOptions) -> WatchOutcome {
    let deadline = opts.timeout.map(|t| Instant::now() + t);
    std::thread::sleep(opts.initial_delay);

    let mut idle_streak: u32 = 0;
    let mut gone_streak: u32 = 0;

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

                match status {
                    "gone" => {
                        // tmux session が生きていれば pane 消滅は tako 再起動中とみなす
                        if tmux_session_alive(opts.tmux_session.as_deref()) {
                            gone_streak = 0;
                            idle_streak = 0;
                        } else {
                            gone_streak += 1;
                            if gone_streak >= 2 {
                                return WatchOutcome::Gone;
                            }
                        }
                    }
                    // "error" は「idle + 画面にエラーパターン」の細分類（#157）。
                    // 停止していることは同じなので idle と同じ streak で確定させ、
                    // 確定時にどちらの outcome かを画面から再判定する
                    "idle" | "error" => {
                        gone_streak = 0;
                        // 画面内容で busy パターンがあれば idle を取り消す
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else {
                            idle_streak += 1;
                        }
                    }
                    "busy" => {
                        gone_streak = 0;
                        idle_streak = 0;
                    }
                    _ => {
                        // unknown: 画面内容から推定（判定不能は busy 扱い = 誤 idle 防止）
                        gone_streak = 0;
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
                    if gone_streak >= 2 {
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
    );
    let final_status = match outcome {
        WatchOutcome::Idle { .. } => "completed",
        // worker がエラー停止（#157）。ペイン消滅の "error" と区別する
        WatchOutcome::Error { .. } => "worker_error",
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
    // エラー停止時は close しない: 続行指示・解除待ちで復帰できる余地を残す（#157） ---
    let closed = if opts.auto_close && !matches!(outcome, WatchOutcome::Error { .. }) {
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
    Ok(result)
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

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
        wait_for_worker(&mut exec, opts)
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
    fn goneは2回連続で消滅と判定する() {
        let mut script =
            ExecScript::new(vec![status("gone", "", "none"), status("gone", "", "none")]);
        let outcome = run_wait(&mut script, &watch_opts(7, None));
        assert_eq!(outcome, WatchOutcome::Gone);
    }

    #[test]
    fn 実行エラーも2回連続でgoneと判定する() {
        let mut script = ExecScript::new(vec![Err("IPC 断".into()), Err("IPC 断".into())]);
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
}

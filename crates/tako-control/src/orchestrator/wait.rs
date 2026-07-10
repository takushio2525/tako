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
    /// ペインも tmux session も消滅した
    Gone,
    /// タイムアウトに達した（worker は動き続けている可能性がある）
    Timeout,
}

/// worker が完了（idle）または消滅（gone）するまでブロックする。
///
/// 判定は `OrchestratorWorkerStatus` を `interval` ごとに呼び、agents 一次シグナル
/// （明示 session_id or 自動解決）なら 3 回、画面推定フォールバックなら 8 回の
/// idle 連続で完了とみなす（サブエージェント完了瞬間の一時 idle 誤検知対策）。
/// gone は 2 回連続で確定するが、`tmux_session` が生存していれば取り消す
/// （tako 再起動中はペイン一覧が空になるため）
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
                    "idle" => {
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

    // --- 4. 自動 close（run の完了後なので force: true） ---
    let closed = if opts.auto_close {
        exec(Request::Close {
            pane: Some(pane_id),
            force: true,
        })
        .is_ok()
    } else {
        false
    };

    Ok(json!({
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "status": final_status,
        "output": output,
        "duration_seconds": start.elapsed().as_secs(),
        "closed": closed,
    }))
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
/// スピナー行「Generating」を拾う（Issue #120。実採取画面より）
pub fn screen_looks_busy(output: &str) -> bool {
    tail_lines(output, 5).iter().any(|l| {
        l.contains("esc to interrupt")
            || l.contains("esc to cancel")
            || l.contains("Generating")
            || l.contains("Working (")
            || l.contains("ing… (")
            || l.contains("Thinking")
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

    /// agy 1.1.0 の実採取画面（Issue #120）
    const AGY_IDLE_SCREEN: &str =
        "● Bash(echo done)\n  完了しました\n────\n>\n────\n? for shortcuts";
    const AGY_BUSY_SCREEN: &str =
        "▸ Thought Process\n⣻  Generating...\n────\n>\n────\nesc to cancel";

    #[test]
    fn codexとagyの画面判定() {
        assert!(screen_looks_idle(CODEX_IDLE_SCREEN), "codex の › を拾う");
        assert!(!screen_looks_busy(CODEX_IDLE_SCREEN));
        assert!(screen_looks_busy(CODEX_BUSY_SCREEN), "Working ( を拾う");

        assert!(screen_looks_idle(AGY_IDLE_SCREEN), "agy の > 単独行を拾う");
        assert!(!screen_looks_busy(AGY_IDLE_SCREEN));
        assert!(
            screen_looks_busy(AGY_BUSY_SCREEN),
            "Generating / esc to cancel を拾う"
        );
        // busy 中も > は見えるが busy 判定が優先される（wait_for_worker の構造）
        assert!(screen_looks_idle(AGY_BUSY_SCREEN));
    }

    #[test]
    fn ascii山括弧はリダイレクト行を入力欄と誤認しない() {
        assert!(!screen_looks_idle("$ echo foo >file\ndone"));
        assert!(!screen_looks_idle(">>append"));
        assert!(screen_looks_idle("some output\n> "));
    }
}

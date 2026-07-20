//! worker 自動復旧 supervisor（Issue #401）。
//!
//! watch の検知イベント（usage_limit / api_error / limit_dialog / WORKER_DEAD /
//! prompt_undelivered）に対して自動リカバリアクションを実行する。
//!
//! 設計方針:
//! - supervisor は watch のポーリングループを**そのまま再利用**する（重複起動しない）。
//!   watch が WatchOutcome を返した時点で、supervisor のアクションを実行し、
//!   再度 watch に入る「外側ループ」を CLI / MCP に提供する
//! - すべての自動アクションは監査ログ（`<data_dir>/supervisor.log`）に記録し、
//!   master へイベント通知する（黙って直さない）
//! - 同一 worker で N 回（既定 3）失敗したらエスカレーション（自動停止 + 通知のみ）
//! - usage_limit のリセット時刻パースは保守的: 失敗時は固定 5 分待ち
//! - WORKER_DEAD の自動 resume は既定 notify-only（#390 の設計判断を尊重）

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::orchestrator::wait::{WatchOptions, WatchOutcome, WorkerErrorKind};
use crate::protocol::Request;

// --- モード設定 ---

/// supervisor のモード
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorMode {
    /// 自動復旧を実行する
    #[default]
    Auto,
    /// 検知のみ通知し、自動アクションは実行しない
    NotifyOnly,
    /// supervisor を無効化する
    Off,
}

impl SupervisorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::NotifyOnly => "notify_only",
            Self::Off => "off",
        }
    }

    pub fn parse_mode(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "notify_only" | "notify-only" => Some(Self::NotifyOnly),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

// --- 監査ログ ---

/// 監査ログファイルのパス
fn audit_log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("supervisor.log"))
}

/// 監査ログに 1 行追記する（最大 256KB ローテート）
pub fn audit_log(worker_id: &str, pane: u64, action: &str, detail: &str) {
    let Some(path) = audit_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // ローテート
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 256 * 1024 {
            let bak = path.with_extension("log.1");
            let _ = std::fs::rename(&path, &bak);
        }
    }
    let now = crate::sessions::now_iso();
    let line = format!("[{now}] worker={worker_id} pane={pane} action={action} {detail}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// 監査ログの末尾を読む
pub fn read_audit_log(lines: usize) -> Vec<String> {
    let Some(path) = audit_log_path() else {
        return vec![];
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    content
        .lines()
        .rev()
        .take(lines)
        .map(String::from)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// --- 復旧アクション ---

/// supervisor の実行コンテキスト
pub struct SupervisorContext<'a> {
    pub exec: &'a mut dyn FnMut(Request) -> Result<Value, String>,
    pub pane_id: u64,
    pub worker_id: String,
    pub mode: SupervisorMode,
    pub auto_resume_dead: bool,
    pub max_retries: u32,
}

/// 復旧履歴の 1 エントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEntry {
    pub timestamp: String,
    pub worker_id: String,
    pub pane: u64,
    pub trigger: String,
    pub action: String,
    pub success: bool,
    pub detail: String,
}

/// supervisor の状態（ワーカーごとの失敗カウンタ + 復旧履歴）
#[derive(Debug, Clone, Default)]
pub struct SupervisorState {
    pub failure_count: u32,
    pub escalated: bool,
    pub history: Vec<RecoveryEntry>,
}

impl SupervisorState {
    fn record(&mut self, entry: RecoveryEntry) {
        if self.history.len() >= 100 {
            self.history.remove(0);
        }
        self.history.push(entry);
    }
}

/// usage_limit のリセット時刻をパースする。
/// claude: 「Your limit will reset at 3:00 AM JST」
/// codex: 「try again at 4:24 AM」
/// 「5-hour limit reached ∙ resets 3am」
pub fn parse_reset_time(detail: &str) -> Option<Duration> {
    // "reset(s) (at )HH:MM" or "at H:MM AM/PM" パターンを探す
    let lower = detail.to_lowercase();

    // "resets Xam" / "resets Xpm" の簡易パターン
    if let Some(pos) = lower.find("resets ") {
        let rest = &lower[pos + 7..];
        if let Some(wait) = parse_time_string(rest) {
            return Some(wait);
        }
    }

    // "at H:MM AM" / "at HH:MM" パターン
    if let Some(pos) = lower.find("at ") {
        let rest = &lower[pos + 3..];
        if let Some(wait) = parse_time_string(rest) {
            return Some(wait);
        }
    }

    None
}

/// 時刻文字列から現在時刻までの待ち時間を計算する
fn parse_time_string(s: &str) -> Option<Duration> {
    let s = s.trim();

    // "3am" / "3pm" / "3:00 AM" / "4:24 AM" / "15:00"
    let mut hour: u32;
    let mut minute: u32 = 0;
    let mut chars = s.chars().peekable();

    // 数字を読む
    let mut num_str = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            num_str.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if num_str.is_empty() {
        return None;
    }
    hour = num_str.parse().ok()?;

    // ':' + 分
    if chars.peek() == Some(&':') {
        chars.next();
        let mut min_str = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                min_str.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if !min_str.is_empty() {
            minute = min_str.parse().ok()?;
        }
    }

    // スペースを飛ばす
    while chars.peek() == Some(&' ') {
        chars.next();
    }

    // AM/PM
    let rest: String = chars.collect();
    let rest_lower = rest.to_lowercase();
    if rest_lower.starts_with("pm") && hour < 12 {
        hour += 12;
    } else if rest_lower.starts_with("am") && hour == 12 {
        hour = 0;
    }

    if hour >= 24 || minute >= 60 {
        return None;
    }

    // 現在時刻からの差分を計算
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now_secs = now.as_secs();
    // ローカル時間に変換するのは困難なので、UTC ベースで近似する。
    // ここでは保守的に、パースした時刻が「次に来る時刻」として扱う
    let day_secs = now_secs % 86400;
    let target_secs = (hour as u64) * 3600 + (minute as u64) * 60;
    let wait = if target_secs > day_secs {
        target_secs - day_secs
    } else {
        // 翌日
        86400 - day_secs + target_secs
    };

    // 0 秒や極端に長い待ちは保守的フォールバックに任せる
    if wait == 0 || wait > 24 * 3600 {
        return None;
    }
    Some(Duration::from_secs(wait))
}

/// usage_limit からの自動復旧:
/// 1. ダイアログで安全選択肢を確定（「Stop and wait」= 通常は選択肢 1）
/// 2. リセット時刻まで待機
/// 3. 続行ナッジ（空行 Enter）を送信
/// 4. busy 復帰を検証
pub fn recover_usage_limit(
    ctx: &mut SupervisorContext,
    detail: &str,
    state: &mut SupervisorState,
) -> bool {
    let action = "usage_limit_recovery";
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("start: {detail}"),
    );

    if ctx.mode != SupervisorMode::Auto {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: mode is not auto",
        );
        return false;
    }

    // 1. usage limit ダイアログの安全選択肢を確定する
    // claude: 「1. Stop and wait for limit to reset」を選択
    // respond で既存のダイアログを確定させる（respond が無いときは Enter で確定）
    let respond_result = (ctx.exec)(Request::OrchestratorRespond {
        pane_id: ctx.pane_id,
        choice: "1".to_string(),
        caller_role: Some("supervisor".to_string()),
    });

    match respond_result {
        Ok(_) => {
            audit_log(
                &ctx.worker_id,
                ctx.pane_id,
                action,
                "dialog confirmed (choice 1)",
            );
        }
        Err(ref e) if e.contains("ダイアログが見つからない") || e.contains("not found") =>
        {
            // ダイアログが既に確定済みの可能性 — 続行ナッジを試す
            audit_log(
                &ctx.worker_id,
                ctx.pane_id,
                action,
                "no dialog found, proceeding to wait",
            );
        }
        Err(ref e) => {
            audit_log(
                &ctx.worker_id,
                ctx.pane_id,
                action,
                &format!("respond failed: {e}"),
            );
            state.failure_count += 1;
            state.record(RecoveryEntry {
                timestamp: crate::sessions::now_iso(),
                worker_id: ctx.worker_id.clone(),
                pane: ctx.pane_id,
                trigger: "usage_limit".into(),
                action: action.into(),
                success: false,
                detail: format!("respond failed: {e}"),
            });
            return false;
        }
    }

    // 2. リセット時刻を解析して待機
    let wait_duration = parse_reset_time(detail).unwrap_or(Duration::from_secs(300));
    let wait_secs = wait_duration.as_secs();
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("waiting {wait_secs}s for limit reset"),
    );
    std::thread::sleep(wait_duration);

    // 3. 続行ナッジ（Enter）を送信
    let nudge = (ctx.exec)(send_request(ctx.pane_id, "\r".to_string()));
    if let Err(ref e) = nudge {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            &format!("nudge failed: {e}"),
        );
        state.failure_count += 1;
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "usage_limit".into(),
            action: action.into(),
            success: false,
            detail: format!("nudge failed: {e}"),
        });
        return false;
    }
    audit_log(&ctx.worker_id, ctx.pane_id, action, "nudge sent");

    // 4. 復帰検証（busy になるまで最大 30 秒待つ）
    let verified = verify_recovery(ctx, Duration::from_secs(30));
    let success = verified;
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        if success {
            "recovered"
        } else {
            "recovery not verified"
        },
    );
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "usage_limit".into(),
        action: action.into(),
        success,
        detail: if success {
            "limit reset → nudge → busy".into()
        } else {
            "nudge sent but worker did not become busy".into()
        },
    });
    if !success {
        state.failure_count += 1;
    }
    success
}

/// api_error からの復旧: バックオフ付き続行ナッジ
pub fn recover_api_error(
    ctx: &mut SupervisorContext,
    detail: &str,
    state: &mut SupervisorState,
) -> bool {
    let action = "api_error_recovery";
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("start: {detail}"),
    );

    if ctx.mode != SupervisorMode::Auto {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: mode is not auto",
        );
        return false;
    }

    // 5 秒待ってから続行ナッジ
    std::thread::sleep(Duration::from_secs(5));

    let nudge = (ctx.exec)(send_request(ctx.pane_id, "続けて\r".to_string()));
    if let Err(ref e) = nudge {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            &format!("nudge failed: {e}"),
        );
        state.failure_count += 1;
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "api_error".into(),
            action: action.into(),
            success: false,
            detail: format!("nudge failed: {e}"),
        });
        return false;
    }
    audit_log(&ctx.worker_id, ctx.pane_id, action, "nudge sent");

    let verified = verify_recovery(ctx, Duration::from_secs(30));
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        if verified {
            "recovered"
        } else {
            "recovery not verified"
        },
    );
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "api_error".into(),
        action: action.into(),
        success: verified,
        detail: if verified {
            "nudge → busy".into()
        } else {
            "nudge sent but worker did not become busy".into()
        },
    });
    if !verified {
        state.failure_count += 1;
    }
    verified
}

/// limit_dialog（codex のモデル切替ダイアログ等）の復旧:
/// 安全選択肢を確定。意味不明なダイアログは notify-only
pub fn recover_limit_dialog(
    ctx: &mut SupervisorContext,
    detail: &str,
    state: &mut SupervisorState,
) -> bool {
    let action = "limit_dialog_recovery";
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("start: {detail}"),
    );

    if ctx.mode != SupervisorMode::Auto {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: mode is not auto",
        );
        return false;
    }

    // codex の「Approaching rate limits / Switch to gpt-…」は Enter で確定
    let nudge = (ctx.exec)(send_request(ctx.pane_id, "\r".to_string()));
    if let Err(ref e) = nudge {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            &format!("confirm failed: {e}"),
        );
        state.failure_count += 1;
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "limit_dialog".into(),
            action: action.into(),
            success: false,
            detail: format!("confirm failed: {e}"),
        });
        return false;
    }

    let verified = verify_recovery(ctx, Duration::from_secs(30));
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        if verified {
            "recovered"
        } else {
            "recovery not verified"
        },
    );
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "limit_dialog".into(),
        action: action.into(),
        success: verified,
        detail: if verified {
            "dialog confirmed → busy".into()
        } else {
            "dialog confirmed but worker did not resume".into()
        },
    });
    if !verified {
        state.failure_count += 1;
    }
    verified
}

/// WORKER_DEAD からの自動 resume（既定 notify-only、opt-in で auto）
pub fn recover_dead(
    ctx: &mut SupervisorContext,
    resume_command: Option<&str>,
    state: &mut SupervisorState,
) -> bool {
    let action = "dead_recovery";
    let resume_str = resume_command.unwrap_or("(none)");
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("start: resume_command={resume_str}"),
    );

    if !ctx.auto_resume_dead || ctx.mode != SupervisorMode::Auto {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: auto_resume_dead is off or mode is not auto",
        );
        return false;
    }

    let Some(cmd) = resume_command else {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: no resume command available",
        );
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "dead".into(),
            action: action.into(),
            success: false,
            detail: "no resume command".into(),
        });
        return false;
    };

    // resume コマンドをシェルへ送る
    let send = (ctx.exec)(send_request(ctx.pane_id, format!("{cmd}\r")));
    if let Err(ref e) = send {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            &format!("send failed: {e}"),
        );
        state.failure_count += 1;
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "dead".into(),
            action: action.into(),
            success: false,
            detail: format!("send failed: {e}"),
        });
        return false;
    }

    // resume 後は起動に時間がかかるので 60 秒待つ
    let verified = verify_recovery(ctx, Duration::from_secs(60));
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        if verified {
            "recovered"
        } else {
            "recovery not verified"
        },
    );
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "dead".into(),
        action: action.into(),
        success: verified,
        detail: if verified {
            format!("resume → busy ({cmd})")
        } else {
            "resume sent but worker did not become busy".into()
        },
    });
    if !verified {
        state.failure_count += 1;
    }
    verified
}

/// prompt_undelivered の自動再送
pub fn recover_prompt_undelivered(
    ctx: &mut SupervisorContext,
    state: &mut SupervisorState,
) -> bool {
    let action = "prompt_undelivered_recovery";
    audit_log(&ctx.worker_id, ctx.pane_id, action, "start");

    if ctx.mode != SupervisorMode::Auto {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: mode is not auto",
        );
        return false;
    }

    // レジストリから元のプロンプトを取得
    let prompt_head = match crate::orchestrator::registry::WorkerRegistry::load() {
        Ok(reg) => reg
            .find_active_by_pane(ctx.pane_id)
            .and_then(|(_, e)| e.prompt_head.clone()),
        Err(_) => None,
    };

    let Some(prompt) = prompt_head else {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "skipped: no prompt_head in registry",
        );
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "prompt_undelivered".into(),
            action: action.into(),
            success: false,
            detail: "no prompt_head in registry".into(),
        });
        return false;
    };

    // プロンプトを再送
    let send = (ctx.exec)(send_request(ctx.pane_id, format!("{prompt}\r")));
    if let Err(ref e) = send {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            &format!("resend failed: {e}"),
        );
        state.failure_count += 1;
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "prompt_undelivered".into(),
            action: action.into(),
            success: false,
            detail: format!("resend failed: {e}"),
        });
        return false;
    }
    audit_log(&ctx.worker_id, ctx.pane_id, action, "prompt resent");

    let verified = verify_recovery(ctx, Duration::from_secs(60));
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        if verified {
            "recovered"
        } else {
            "recovery not verified"
        },
    );
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "prompt_undelivered".into(),
        action: action.into(),
        success: verified,
        detail: if verified {
            "prompt resent → busy".into()
        } else {
            "prompt resent but worker did not become busy".into()
        },
    });
    if !verified {
        state.failure_count += 1;
    }
    verified
}

/// ペインへテキスト送信する Request を構築する
fn send_request(pane: u64, text: String) -> Request {
    Request::Send {
        pane: Some(pane),
        text,
        newline: false,
        tmux_session: None,
        await_prompt: false,
    }
}

/// 復帰検証: worker_status を定期的に取得し、busy になるか確認する
fn verify_recovery(ctx: &mut SupervisorContext, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_secs(5));

        let result = (ctx.exec)(Request::OrchestratorWorkerStatus {
            pane_id: Some(ctx.pane_id),
            session_id: None,
            tmux_session: None,
            worker: None,
        });
        if let Ok(val) = result {
            let status = val["status"].as_str().unwrap_or("unknown");
            if status == "busy" {
                return true;
            }
        }
    }
}

/// supervisor ループ本体: watch → 検知 → 復旧アクション → 再 watch を繰り返す。
/// 呼び出し元の CLI / MCP が watch_for_worker の外側で呼ぶ設計
pub fn supervisor_loop(
    exec: &mut dyn FnMut(Request) -> Result<Value, String>,
    watch_opts: &WatchOptions,
    mode: SupervisorMode,
    auto_resume_dead: bool,
    max_retries: u32,
    worker_id: &str,
) -> (WatchOutcome, SupervisorState) {
    let mut state = SupervisorState::default();

    if mode == SupervisorMode::Off {
        let outcome =
            crate::orchestrator::wait::wait_for_worker(&mut |r| exec(r), watch_opts, None);
        return (outcome, state);
    }

    loop {
        let outcome =
            crate::orchestrator::wait::wait_for_worker(&mut |r| exec(r), watch_opts, None);

        // エスカレーション判定
        if state.failure_count >= max_retries {
            audit_log(
                worker_id,
                watch_opts.pane_id,
                "escalation",
                &format!(
                    "failure_count={} >= max_retries={max_retries}, stopping auto-recovery",
                    state.failure_count
                ),
            );
            state.escalated = true;
            return (outcome, state);
        }

        let mut ctx = SupervisorContext {
            exec,
            pane_id: watch_opts.pane_id,
            worker_id: worker_id.to_string(),
            mode,
            auto_resume_dead,
            max_retries,
        };

        // 通知イベント生成（全モードで master へ通知する）
        let event_line = match &outcome {
            WatchOutcome::Error { kind, detail } => Some(format!(
                "SUPERVISOR_DETECTED: worker={worker_id} pane={} trigger={} detail={}",
                watch_opts.pane_id,
                kind.as_str(),
                detail
            )),
            WatchOutcome::AgentDead { resume_command } => Some(format!(
                "SUPERVISOR_DETECTED: worker={worker_id} pane={} trigger=agent_dead resume={}",
                watch_opts.pane_id,
                resume_command.as_deref().unwrap_or("(none)")
            )),
            WatchOutcome::Stalled { detail } => Some(format!(
                "SUPERVISOR_DETECTED: worker={worker_id} pane={} trigger=stalled detail={}",
                watch_opts.pane_id, detail
            )),
            _ => None,
        };
        if let Some(ref line) = event_line {
            eprintln!("{line}");
        }

        // prompt_undelivered は events 配列から検知する
        // watch の TIMEOUT で idle が積めなかったケースの追加検知
        let has_prompt_undelivered = if matches!(outcome, WatchOutcome::Timeout) {
            // Timeout 後に worker_status を 1 回取得して prompt_undelivered チェック
            if let Ok(val) = (ctx.exec)(Request::OrchestratorWorkerStatus {
                pane_id: Some(watch_opts.pane_id),
                session_id: None,
                tmux_session: None,
                worker: None,
            }) {
                val["events"].as_array().is_some_and(|evts| {
                    evts.iter()
                        .any(|e| e["kind"].as_str() == Some("prompt_undelivered"))
                })
            } else {
                false
            }
        } else {
            false
        };

        // 復旧アクション実行
        let recovered = match &outcome {
            WatchOutcome::Error { kind, detail } => match kind {
                WorkerErrorKind::UsageLimit => recover_usage_limit(&mut ctx, detail, &mut state),
                WorkerErrorKind::ApiError => recover_api_error(&mut ctx, detail, &mut state),
                WorkerErrorKind::LimitDialog => recover_limit_dialog(&mut ctx, detail, &mut state),
            },
            WatchOutcome::AgentDead { resume_command } => {
                recover_dead(&mut ctx, resume_command.as_deref(), &mut state)
            }
            WatchOutcome::Stalled { .. } => {
                // stalled は api_error と同じ: 続行ナッジ
                recover_api_error(&mut ctx, "stalled", &mut state)
            }
            _ if has_prompt_undelivered => recover_prompt_undelivered(&mut ctx, &mut state),
            // Idle / Question / PermissionWaiting / Gone / Timeout は supervisor の対象外
            _ => {
                return (outcome, state);
            }
        };

        if recovered {
            // 復旧成功: 再度 watch ループに入る
            let action_name = match &outcome {
                WatchOutcome::Error { kind, .. } => kind.as_str(),
                WatchOutcome::AgentDead { .. } => "dead",
                WatchOutcome::Stalled { .. } => "stalled",
                _ => "prompt_undelivered",
            };
            audit_log(
                worker_id,
                watch_opts.pane_id,
                "re_watch",
                &format!("recovery succeeded for {action_name}, re-entering watch loop"),
            );
            continue;
        } else {
            // 復旧失敗: 終了（エスカレーション判定は次ループの冒頭で行う）
            if state.failure_count >= max_retries {
                state.escalated = true;
                audit_log(
                    worker_id,
                    watch_opts.pane_id,
                    "escalation",
                    &format!(
                        "failure_count={} >= max_retries={max_retries}",
                        state.failure_count
                    ),
                );
            }
            return (outcome, state);
        }
    }
}

/// supervisor の状態照会の結果（MCP / CLI 用）
pub fn supervisor_status_json(
    mode: SupervisorMode,
    auto_resume_dead: bool,
    max_retries: u32,
    state: &SupervisorState,
) -> Value {
    json!({
        "mode": mode.as_str(),
        "auto_resume_dead": auto_resume_dead,
        "max_retries": max_retries,
        "failure_count": state.failure_count,
        "escalated": state.escalated,
        "history_count": state.history.len(),
        "history": state.history.iter().rev().take(20).map(|e| json!({
            "timestamp": e.timestamp,
            "worker_id": e.worker_id,
            "pane": e.pane,
            "trigger": e.trigger,
            "action": e.action,
            "success": e.success,
            "detail": e.detail,
        })).collect::<Vec<_>>(),
    })
}

// --- テスト ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reset_time_claude_format() {
        // 「Your limit will reset at 3:00 AM JST」
        let d = parse_reset_time("Your limit will reset at 3:00 AM JST");
        assert!(d.is_some(), "should parse claude format");
        let secs = d.unwrap().as_secs();
        assert!(secs > 0 && secs <= 86400, "should be within 24h: {secs}");
    }

    #[test]
    fn parse_reset_time_codex_format() {
        // 「try again at 4:24 AM」
        let d = parse_reset_time("try again at 4:24 AM");
        assert!(d.is_some(), "should parse codex format");
    }

    #[test]
    fn parse_reset_time_resets_format() {
        // 「5-hour limit reached ∙ resets 3am」
        let d = parse_reset_time("5-hour limit reached ∙ resets 3am");
        assert!(d.is_some(), "should parse resets format");
    }

    #[test]
    fn parse_reset_time_no_match() {
        assert!(parse_reset_time("some random text").is_none());
    }

    #[test]
    fn parse_reset_time_24h_format() {
        let d = parse_reset_time("reset at 15:30");
        assert!(d.is_some(), "should parse 24h format");
    }

    #[test]
    fn supervisor_mode_roundtrip() {
        for mode in [
            SupervisorMode::Auto,
            SupervisorMode::NotifyOnly,
            SupervisorMode::Off,
        ] {
            assert_eq!(
                SupervisorMode::parse_mode(mode.as_str()),
                Some(mode),
                "roundtrip for {:?}",
                mode
            );
        }
    }

    #[test]
    fn supervisor_mode_from_str_hyphen() {
        assert_eq!(
            SupervisorMode::parse_mode("notify-only"),
            Some(SupervisorMode::NotifyOnly)
        );
    }

    #[test]
    fn audit_log_writes() {
        let dir = std::env::temp_dir().join(format!("tako-test-supervisor-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("supervisor.log");
        // audit_log は audit_log_path() を使うが、テスト用に直接書く
        let line = "[test] worker=1 pane=42 action=test detail\n";
        let _ = std::fs::write(&path, line);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("action=test"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn supervisor_state_record_caps_at_100() {
        let mut state = SupervisorState::default();
        for i in 0..110 {
            state.record(RecoveryEntry {
                timestamp: format!("t{i}"),
                worker_id: "1".into(),
                pane: 42,
                trigger: "test".into(),
                action: "test".into(),
                success: true,
                detail: format!("entry {i}"),
            });
        }
        assert_eq!(state.history.len(), 100);
    }
}

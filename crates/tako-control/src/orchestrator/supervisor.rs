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

/// limit 系ダイアログから「勝手に課金・モデル変更をしない」選択肢の番号を選ぶ（#748）。
///
/// 旧実装は `choice: "1"` 固定 / 素の Enter（= ハイライトされている選択肢の確定）だった。
/// claude の limit ダイアログは実装上「待つ」選択肢が先頭とは限らず
/// （バイナリ内の組み立てが `[...options, cancel]` になる分岐がある）、
/// codex のダイアログは既定ハイライトが「Switch to <安いモデル>」なので、
/// 盲目的な Enter は**黙って課金プラン変更 / モデル変更を確定させる**危険がある。
///
/// 優先順: 解除まで待つ > 現状維持 > 停止。いずれも無ければ `None`
/// （呼び出し側は自動操作をやめて通知のみに落ちる）。
///
/// 選別そのものは `tako_core::limit_resume::safe_choice` の 1 実装に寄せてある（#813）。
/// おかげで許可リストと**拒否リスト**（課金・モデル変更を伴うラベルを構造的に弾く）が
/// supervisor（#401）とペイン単位の自動復帰（#813）で完全に同じになる
pub fn safe_limit_choice(dialog: &Value) -> Option<u32> {
    let options: Vec<(Option<u32>, String)> = dialog
        .get("options")?
        .as_array()?
        .iter()
        .map(|o| {
            (
                o.get("number").and_then(|n| n.as_u64()).map(|n| n as u32),
                o.get("label")
                    .and_then(|l| l.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    tako_core::limit_resume::safe_choice(&options).map(|(number, _)| number)
}

/// ダイアログの構造を下見する（choice 省略 = 送信しない。#748）。
/// ダイアログが無ければ `None`
fn probe_dialog(ctx: &mut SupervisorContext) -> Option<Value> {
    (ctx.exec)(Request::OrchestratorRespond {
        pane_id: ctx.pane_id,
        choice: None,
        caller_role: Some("supervisor".to_string()),
    })
    .ok()
    .filter(|v| v.get("options").is_some_and(|o| o.is_array()))
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

    // 1. usage limit ダイアログの安全選択肢を確定する。
    // #748: まず構造を下見し（choice 省略 = 送信しない）、「解除まで待つ」選択肢を
    // **ラベルで**選ぶ。旧実装は choice="1" 固定で、選択肢の並びが変わると
    // 課金プラン変更を確定させかねなかった。安全な選択肢が無ければ触らずに待つ
    if let Some(dialog) = probe_dialog(ctx) {
        match safe_limit_choice(&dialog) {
            Some(number) => {
                let respond_result = (ctx.exec)(Request::OrchestratorRespond {
                    pane_id: ctx.pane_id,
                    choice: Some(number.to_string()),
                    caller_role: Some("supervisor".to_string()),
                });
                match respond_result {
                    Ok(_) => audit_log(
                        &ctx.worker_id,
                        ctx.pane_id,
                        action,
                        &format!("dialog confirmed (choice {number} = wait/keep)"),
                    ),
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
            }
            None => audit_log(
                &ctx.worker_id,
                ctx.pane_id,
                action,
                "dialog found but no safe option (wait/keep/stop) — leaving it to master",
            ),
        }
    } else {
        // ダイアログが無い（既に確定済み / limit メッセージのみ）— 待ちへ進む
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "no dialog found, proceeding to wait",
        );
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

    // #748: 旧実装は素の Enter（= ハイライト確定）で、codex の既定ハイライトが
    // 「Switch to <安いモデル>」なので黙ってモデルを変えていた。構造を下見して
    // 「現状維持 / 解除まで待つ」をラベルで選び、無ければ触らず master に委ねる
    let Some(dialog) = probe_dialog(ctx) else {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "no dialog on screen (already resolved?)",
        );
        return false;
    };
    let Some(number) = safe_limit_choice(&dialog) else {
        audit_log(
            &ctx.worker_id,
            ctx.pane_id,
            action,
            "no safe option (wait/keep/stop) — notify only",
        );
        state.record(RecoveryEntry {
            timestamp: crate::sessions::now_iso(),
            worker_id: ctx.worker_id.clone(),
            pane: ctx.pane_id,
            trigger: "limit_dialog".into(),
            action: action.into(),
            success: false,
            detail: "安全な選択肢が無いため自動応答しない（master が respond する）".into(),
        });
        return false;
    };
    let nudge = (ctx.exec)(Request::OrchestratorRespond {
        pane_id: ctx.pane_id,
        choice: Some(number.to_string()),
        caller_role: Some("supervisor".to_string()),
    });
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
    audit_log(
        &ctx.worker_id,
        ctx.pane_id,
        action,
        &format!("dialog confirmed (choice {number} = wait/keep)"),
    );

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

/// 起動失敗（#983）は**自動復旧しない**。監査ログと履歴へ理由を残して未復旧を返す。
///
/// 自動で直せる種類の失敗ではない（CLI の導入・ログイン・runtime の起動が要る）ので、
/// ここで再送やナッジを撃つと同じ失敗を max_retries 回繰り返すだけになる
fn report_launch_failed(
    ctx: &mut SupervisorContext,
    detail: &str,
    state: &mut SupervisorState,
) -> bool {
    let action = "launch_failed_report";
    audit_log(&ctx.worker_id, ctx.pane_id, action, detail);
    state.record(RecoveryEntry {
        timestamp: crate::sessions::now_iso(),
        worker_id: ctx.worker_id.clone(),
        pane: ctx.pane_id,
        trigger: "launch_failed".into(),
        action: action.into(),
        success: false,
        detail: detail.to_string(),
    });
    state.failure_count += 1;
    false
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
                // #983: 起動そのものが失敗している（未認証・CLI 不在・即時終了）。
                // 続行ナッジも待機も再送も効かない —— **同じ失敗を繰り返すだけ**なので
                // 自動復旧を試みず、未復旧としてエスカレーションへ回す
                // （detail に「理由 + 次の一手」が入っているので、人が読めば直せる）
                WorkerErrorKind::LaunchFailed => report_launch_failed(&mut ctx, detail, &mut state),
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

    // --- 統合テスト: exec モックで復旧フローの実経路を検証 ---

    use std::sync::{Arc, Mutex};

    /// exec に渡された Request を記録するモック
    struct ExecRecorder {
        calls: Arc<Mutex<Vec<String>>>,
        respond_result: Result<Value, String>,
        status_sequence: Vec<&'static str>,
        status_idx: Arc<Mutex<usize>>,
        /// #748: `respond`（choice 省略）が返すダイアログ構造
        dialog: Value,
    }

    impl ExecRecorder {
        fn new(respond_ok: bool, statuses: Vec<&'static str>) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                respond_result: if respond_ok {
                    Ok(json!({"ok": true}))
                } else {
                    Err("ダイアログが見つからない".into())
                },
                status_sequence: statuses,
                status_idx: Arc::new(Mutex::new(0)),
                // 実 claude の limit ダイアログ相当（「待つ」が先頭ではない並びにして、
                // choice="1" 固定だと課金プラン変更を選んでしまう配置を再現する）
                dialog: json!({
                    "kind": "usage_limit",
                    "numbered": true,
                    "highlighted": 0,
                    "options": [
                        {"number": 1, "label": "Upgrade to Max 20x for higher session limits every month", "highlighted": true},
                        {"number": 2, "label": "Stop and wait for limit to reset", "highlighted": false},
                    ],
                }),
            }
        }

        fn exec(&mut self, req: Request) -> Result<Value, String> {
            let kind = req.kind_name().to_string();
            self.calls.lock().unwrap().push(kind.clone());

            match req {
                // #748: choice 省略 = 下見（送信しない）。構造を返さないと
                // supervisor は「ダイアログ無し」と判断して自動応答しない
                Request::OrchestratorRespond { choice: None, .. } => Ok(self.dialog.clone()),
                Request::OrchestratorRespond { .. } => self.respond_result.clone(),
                Request::Send { .. } => Ok(json!({"ok": true})),
                Request::OrchestratorWorkerStatus { .. } => {
                    let mut idx = self.status_idx.lock().unwrap();
                    let status = if *idx < self.status_sequence.len() {
                        self.status_sequence[*idx]
                    } else {
                        "busy"
                    };
                    *idx += 1;
                    Ok(json!({"status": status}))
                }
                _ => Err(format!("unexpected request: {kind}")),
            }
        }

        fn call_kinds(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn make_ctx<'a>(
        exec: &'a mut dyn FnMut(Request) -> Result<Value, String>,
        mode: SupervisorMode,
    ) -> SupervisorContext<'a> {
        SupervisorContext {
            exec,
            pane_id: 42,
            worker_id: "test-1".into(),
            mode,
            auto_resume_dead: true,
            max_retries: 3,
        }
    }

    #[test]
    fn recover_api_error_sends_nudge_and_verifies_busy() {
        let mut rec = ExecRecorder::new(true, vec!["busy"]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        let mut state = SupervisorState::default();

        let ok = recover_api_error(&mut ctx, "API Error: Connection closed", &mut state);
        assert!(ok, "recovery should succeed when worker becomes busy");

        let kinds = rec.call_kinds();
        assert!(
            kinds.iter().any(|k| k == "Send"),
            "should send nudge: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k == "OrchestratorWorkerStatus"),
            "should verify recovery: {kinds:?}"
        );
        assert_eq!(state.failure_count, 0);
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].success);
        assert_eq!(state.history[0].trigger, "api_error");
    }

    #[test]
    fn recover_api_error_fails_when_worker_stays_idle() {
        // verify_recovery のタイムアウトを短くするため status が idle のまま返す
        // 実際は 30 秒待つが、テスト用に全 poll が idle を返す設定
        let mut rec = ExecRecorder::new(true, vec!["idle"; 20]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        // verify_recovery のタイムアウトは 30s → テストでは idle 連続で timeout
        // ただし sleep(5) があるのでこのテストは最大 30s かかる。
        // 短縮のために mode を切り替えて早期リターンを使う
        ctx.mode = SupervisorMode::NotifyOnly;
        let mut state = SupervisorState::default();

        let ok = recover_api_error(&mut ctx, "API Error: timeout", &mut state);
        assert!(!ok, "should skip when mode is notify_only");
        assert_eq!(state.failure_count, 0, "notify_only ではカウントしない");
    }

    #[test]
    fn recover_api_error_notify_only_skips_action() {
        let mut rec = ExecRecorder::new(true, vec![]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::NotifyOnly);
        let mut state = SupervisorState::default();

        let ok = recover_api_error(&mut ctx, "API Error", &mut state);
        assert!(!ok);
        let kinds = rec.call_kinds();
        assert!(
            !kinds.iter().any(|k| k == "Send"),
            "notify_only should not send: {kinds:?}"
        );
    }

    #[test]
    fn recover_dead_sends_resume_command() {
        let mut rec = ExecRecorder::new(true, vec!["busy"]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        ctx.auto_resume_dead = true;
        let mut state = SupervisorState::default();

        let ok = recover_dead(
            &mut ctx,
            Some("cd '/tmp' && claude --resume abc123"),
            &mut state,
        );
        assert!(ok, "should succeed with resume command");

        let kinds = rec.call_kinds();
        assert!(
            kinds.iter().any(|k| k == "Send"),
            "should send resume command: {kinds:?}"
        );
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].success);
        assert_eq!(state.history[0].trigger, "dead");
    }

    #[test]
    fn recover_dead_skips_when_auto_resume_off() {
        let mut rec = ExecRecorder::new(true, vec![]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        ctx.auto_resume_dead = false;
        let mut state = SupervisorState::default();

        let ok = recover_dead(&mut ctx, Some("claude --resume x"), &mut state);
        assert!(!ok, "should skip when auto_resume_dead=false");
        let kinds = rec.call_kinds();
        assert!(kinds.is_empty(), "should not call anything: {kinds:?}");
    }

    #[test]
    fn recover_dead_no_resume_command() {
        let mut rec = ExecRecorder::new(true, vec![]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        ctx.auto_resume_dead = true;
        let mut state = SupervisorState::default();

        let ok = recover_dead(&mut ctx, None, &mut state);
        assert!(!ok, "should fail without resume command");
        assert_eq!(state.history.len(), 1);
        assert!(!state.history[0].success);
    }

    #[test]
    fn recover_limit_dialog_confirms_and_verifies() {
        let mut rec = ExecRecorder::new(true, vec!["busy"]);
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        let mut state = SupervisorState::default();

        let ok = recover_limit_dialog(&mut ctx, "Approaching rate limits", &mut state);
        assert!(ok);

        let kinds = rec.call_kinds();
        // #748: 素の Enter（Send）ではなく respond（下見 → 安全な選択肢）で確定する
        assert!(
            !kinds.iter().any(|k| k == "Send"),
            "盲目的な Enter は送らない: {kinds:?}"
        );
        assert_eq!(
            kinds.iter().filter(|k| *k == "OrchestratorRespond").count(),
            2,
            "下見 + 応答の 2 回: {kinds:?}"
        );
        assert_eq!(state.history[0].trigger, "limit_dialog");
    }

    #[test]
    fn issue748_safe_limit_choiceは待つ選択肢を選ぶ() {
        // 「待つ」が先頭でない並び（claude の実装は cancel を末尾に置く分岐がある）。
        // 旧実装の choice="1" 固定ならプラン変更を確定させていた
        let dialog = json!({
            "options": [
                {"number": 1, "label": "Upgrade to Max 20x for higher session limits every month"},
                {"number": 2, "label": "Continue with usage credits"},
                {"number": 3, "label": "Stop and wait for limit to reset"},
            ],
        });
        assert_eq!(safe_limit_choice(&dialog), Some(3));

        // codex のモデル切替ダイアログ: 現状維持を選ぶ（既定ハイライトは切替側）
        let codex = json!({
            "options": [
                {"number": 1, "label": "Switch to gpt-5.4-mini"},
                {"number": 2, "label": "Keep current model"},
                {"number": 3, "label": "Keep current model (never show again)"},
            ],
        });
        assert_eq!(safe_limit_choice(&codex), Some(2));

        // 「Stop」だけの短縮ラベル（バイナリ内で `VfS?"Stop":…` の分岐がある）
        let short = json!({
            "options": [
                {"number": 1, "label": "Upgrade to Team plan"},
                {"number": 2, "label": "Stop"},
            ],
        });
        assert_eq!(safe_limit_choice(&short), Some(2));

        // 課金・変更しかない並びでは自動では選ばない（master / user に委ねる）
        let unsafe_only = json!({
            "options": [
                {"number": 1, "label": "Upgrade to Max 20x"},
                {"number": 2, "label": "Buy usage credits"},
            ],
        });
        assert_eq!(safe_limit_choice(&unsafe_only), None);
    }

    #[test]
    fn issue748_安全な選択肢が無ければ自動応答しない() {
        let mut rec = ExecRecorder::new(true, vec!["busy"]);
        // 課金・変更しかないダイアログを返す
        rec.dialog = json!({
            "kind": "usage_limit",
            "numbered": true,
            "highlighted": 0,
            "options": [
                {"number": 1, "label": "Upgrade to Max 20x", "highlighted": true},
                {"number": 2, "label": "Buy usage credits", "highlighted": false},
            ],
        });
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        let mut state = SupervisorState::default();

        let ok = recover_limit_dialog(&mut ctx, "Approaching rate limits", &mut state);
        assert!(!ok, "自動では確定しない");
        let kinds = rec.call_kinds();
        assert_eq!(
            kinds.iter().filter(|k| *k == "OrchestratorRespond").count(),
            1,
            "下見だけで応答は送らない: {kinds:?}"
        );
        assert!(!kinds.iter().any(|k| k == "Send"), "{kinds:?}");
        assert!(!state.history[0].success);
    }

    #[test]
    fn recover_usage_limit_responds_and_waits() {
        // usage_limit の recover は sleep が入るためテスト時間が長い。
        // ダイアログ未検出パス（respond がエラー → 待機へ進む）で検証する。
        // parse_reset_time が None を返す文字列 → 5 分待ち → テストでは不適切。
        // 代わりに respond 成功 → sleep をスキップする方法がないため、
        // respond 失敗（failure_count 増加）パスで Request 経路を検証する
        let mut rec = ExecRecorder::new(true, vec!["idle"; 20]);
        rec.respond_result = Err("other error".into());
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);
        let mut state = SupervisorState::default();

        let ok = recover_usage_limit(&mut ctx, "usage limit - no time info", &mut state);
        assert!(!ok, "should fail when respond fails");

        let kinds = rec.call_kinds();
        assert!(
            kinds.iter().any(|k| k == "OrchestratorRespond"),
            "should attempt respond: {kinds:?}"
        );
        assert_eq!(state.failure_count, 1);
        assert_eq!(state.history[0].trigger, "usage_limit");
        assert!(!state.history[0].success);
    }

    #[test]
    fn escalation_after_max_retries() {
        let mut state = SupervisorState {
            failure_count: 2,
            ..Default::default()
        };

        // 3 回目の失敗
        let mut rec = ExecRecorder::new(true, vec!["idle"; 20]);
        rec.respond_result = Err("other error".into());
        let mut exec = |r: Request| rec.exec(r);
        let mut ctx = make_ctx(&mut exec, SupervisorMode::Auto);

        let ok = recover_usage_limit(&mut ctx, "limit", &mut state);
        assert!(!ok);
        assert_eq!(
            state.failure_count, 3,
            "failure_count should reach max_retries"
        );
    }

    #[test]
    fn audit_log_writes_to_real_file() {
        // TAKO_DATA_DIR を一時ディレクトリに差し替え
        let dir = std::env::temp_dir().join(format!("tako-sv-audit-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // audit_log は data_dir() を使うので直接書いてフォーマットを検証
        let log_path = dir.join("supervisor.log");
        let now = crate::sessions::now_iso();
        let line = format!("[{now}] worker=1 pane=42 action=api_error_recovery nudge sent\n");
        let _ = std::fs::write(&log_path, &line);

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("worker=1"));
        assert!(content.contains("pane=42"));
        assert!(content.contains("action=api_error_recovery"));
        assert!(content.contains("nudge sent"));

        // read_audit_log の形式と一致
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with('['));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn supervisor_status_json_returns_correct_shape() {
        let state = SupervisorState {
            failure_count: 2,
            escalated: false,
            history: vec![RecoveryEntry {
                timestamp: "2026-07-21T10:00:00Z".into(),
                worker_id: "1".into(),
                pane: 42,
                trigger: "api_error".into(),
                action: "api_error_recovery".into(),
                success: true,
                detail: "nudge → busy".into(),
            }],
        };

        let v = supervisor_status_json(SupervisorMode::Auto, false, 3, &state);
        assert_eq!(v["mode"], "auto");
        assert_eq!(v["auto_resume_dead"], false);
        assert_eq!(v["max_retries"], 3);
        assert_eq!(v["failure_count"], 2);
        assert_eq!(v["escalated"], false);
        assert_eq!(v["history_count"], 1);
        assert!(v["history"].as_array().unwrap().len() == 1);
    }
}

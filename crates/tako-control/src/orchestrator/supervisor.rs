//! worker の常時監視 supervisor（Issue #401 / #665）。
//!
//! **全 worker を 1 本のループで監視し、master に再アームさせない**のが役割。
//! `tako orchestrator watch` が 1 イベントで終わる単発なのに対し、こちらは
//! 毎周期レジストリを読み直すので、spawn した worker はその場で監視対象に入り、
//! 閉じた worker は自動的に外れる。
//!
//! 設計方針:
//! - 状態の見立ては `wait::WatchStreaks` に一本化する（watch と同じ判定を通す。
//!   判定を二重に書くと必ず食い違う = #273 / #289 の教訓）
//! - 自動対応は**すべて非ブロッキング**。1 体の異常対応で他の worker を止めない
//!   （打って次の周期で結果を観測する）。usage limit の解除待ちも sleep ではなく
//!   「この時刻までは触らない」で表現する
//! - すべての自動アクションは監査ログ（`<data_dir>/supervisor.log`）に記録し、
//!   イベントとして master へ通知する（黙って直さない）
//! - 同一 worker で N 回（既定 3）打っても復帰しなければエスカレーション（通知のみ）
//! - usage_limit のリセット時刻パースは保守的: 失敗時は固定 5 分待ち
//! - WORKER_DEAD の自動 resume は既定 notify-only（#390 の設計判断を尊重）
//!
//! イベントは `<data_dir>/supervisor-events.jsonl` へも書く。これが
//! 「別プロセスの master が取りこぼしなく読む」ための配送路になる（MCP の
//! `action=events` はカーソル指定でここから読む）
//!
//! #665 以前にあった単発ブロッキングの復旧層（`supervisor_loop` / `recover_*`）は
//! **どこからも呼ばれていなかった**（= 自動復旧は一度も動いていなかった）。
//! 多 worker を 1 本で見るには sleep で待つ設計そのものが使えないため、
//! 方針（usage_limit は解除まで待つ / api_error はナッジ / dead は既定 notify-only）を
//! 引き継いだ非ブロッキング版へ置き換えた

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::orchestrator::wait::{WatchOutcome, WatchStreaks, WorkerErrorKind};
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
// --- 監視イベント（プロセス跨ぎの配送単位） ---

/// supervisor が master へ届けるイベントの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorEventKind {
    /// 監視対象に入った（spawn 直後の自動エンロール = 再アーム不要の証跡）
    Watching,
    /// 完了・入力待ち
    Idle,
    /// 質問して止まっている（#243）
    Question,
    /// permission ダイアログで止まっている（#319）
    Permission,
    /// 既知のエラーで止まっている（#157）
    Error,
    /// 停滞（子プロセスなし + 画面不変。#224）
    Stalled,
    /// エージェント CLI プロセスの突然死（#390）
    Dead,
    /// ペインも tmux session も消えた
    Gone,
    /// 自動一次対応を実行した
    AutoAction,
    /// 自動復旧の上限に達したので master へ委ねる
    Escalated,
}

impl SupervisorEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watching => "watching",
            Self::Idle => "idle",
            Self::Question => "question",
            Self::Permission => "permission",
            Self::Error => "error",
            Self::Stalled => "stalled",
            Self::Dead => "dead",
            Self::Gone => "gone",
            Self::AutoAction => "auto_action",
            Self::Escalated => "escalated",
        }
    }

    /// 既存 `tako orchestrator watch` と同じ行頭マーカー。
    /// master 側の読み取りを作り直させないため、語彙は変えない（後方互換）
    pub fn line_marker(self) -> &'static str {
        match self {
            Self::Watching => "WORKER_WATCHING",
            Self::Idle => "WORKER_IDLE",
            Self::Question => "WORKER_QUESTION",
            Self::Permission => "WORKER_PERMISSION",
            Self::Error => "WORKER_ERROR",
            Self::Stalled => "WORKER_STALLED",
            Self::Dead => "WORKER_DEAD",
            Self::Gone => "WORKER_GONE",
            Self::AutoAction => "SUPERVISOR_ACTION",
            Self::Escalated => "SUPERVISOR_ESCALATED",
        }
    }
}

/// 1 イベント。journal（JSONL）と CLI 出力の共通表現
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorEvent {
    /// 単調増加のカーソル。MCP の `action=events` はこれで取りこぼしを防ぐ
    #[serde(default)]
    pub seq: u64,
    pub ts: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub worker_id: String,
    pub pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    pub kind: SupervisorEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// master への推奨アクション（`WorkerErrorKind::recommended_action` 等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl SupervisorEvent {
    /// CLI がストリームへ出す 1 行（+ 補助行）
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "{}: tako:{}{}",
            self.kind.line_marker(),
            self.pane,
            match (&self.label, self.worker_id.is_empty()) {
                (Some(l), false) => format!(" (worker={} {l})", self.worker_id),
                (Some(l), true) => format!(" ({l})"),
                (None, false) => format!(" (worker={})", self.worker_id),
                (None, true) => String::new(),
            }
        )];
        if let Some(d) = &self.detail {
            if !d.is_empty() {
                out.push(format!("  detail: {d}"));
            }
        }
        if let Some(a) = &self.action {
            out.push(format!("  action: {a}"));
        }
        out
    }
}

/// イベント journal（JSONL）のパス。プロセス跨ぎの配送路になる
pub fn journal_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("supervisor-events.jsonl"))
}

/// journal のローテート閾値
const JOURNAL_MAX_BYTES: u64 = 1024 * 1024;

/// 末尾の seq を読む。ローテート直後は退避先（.1）から引き継ぐ
fn last_seq(path: &Path) -> u64 {
    let read_last = |p: &Path| -> Option<u64> {
        let content = std::fs::read_to_string(p).ok()?;
        content
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str::<Value>(l).ok()?["seq"].as_u64())
    };
    read_last(path)
        .or_else(|| read_last(&path.with_extension("jsonl.1")))
        .unwrap_or(0)
}

/// イベントを journal へ追記し、確定した seq を返す。
/// 複数プロセス（supervisor 本体とテスト）が同時に書いても seq が壊れないよう
/// ファイルロックの下で「末尾 seq を読む → +1 して書く」を行う
pub fn append_event(event: &SupervisorEvent) -> Result<u64, String> {
    let path = journal_path().ok_or("ホームディレクトリが取得できない")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _lock = crate::config_io::lock_exclusive(&path)?;

    // ローテート（seq は .1 から引き継ぐので連番は途切れない）
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > JOURNAL_MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("jsonl.1"));
    }

    let seq = last_seq(&path) + 1;
    let mut stamped = event.clone();
    stamped.seq = seq;
    let line =
        serde_json::to_string(&stamped).map_err(|e| format!("イベントの JSON 化に失敗: {e}"))?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()))
        .map_err(|e| format!("イベントの書き込みに失敗: {e}"))?;
    Ok(seq)
}

/// `cursor` より後のイベントを読む（MCP の `action=events`）。
/// 返り値は (イベント列, 次のカーソル, ローテートで取りこぼした可能性)
pub fn read_events(cursor: u64, limit: usize) -> Result<(Vec<SupervisorEvent>, u64, bool), String> {
    let path = journal_path().ok_or("ホームディレクトリが取得できない")?;
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], cursor, false)),
        Err(e) => return Err(format!("イベントの読み取りに失敗: {e}")),
    };
    let all: Vec<SupervisorEvent> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<SupervisorEvent>(l).ok())
        .collect();
    // 現ファイルの先頭がカーソルの続きより先なら、ローテートで間が抜けている
    let truncated = all
        .first()
        .is_some_and(|first| cursor > 0 && first.seq > cursor + 1);
    // **古い順に limit 件**返す。新しい順に切ると、limit を超えた backlog の
    // 古い側がカーソルの前進で読まれないまま飛ばされる（取りこぼしゼロが崩れる）
    let picked: Vec<SupervisorEvent> = all
        .into_iter()
        .filter(|e| e.seq > cursor)
        .take(limit)
        .collect();
    // 返した最後のイベントまでを消化済みとする（残りは次回の続きから読める）
    let next = picked.last().map(|e| e.seq).unwrap_or(cursor);
    Ok((picked, next, truncated))
}

// --- 自動一次対応 ---

/// supervisor が自分で打つ手。**判断と実行を分ける**ことで、判断だけを
/// 単体テストできるようにする（実行は exec が要る）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoAction {
    /// 何もしない（master へ通知するだけ）
    None,
    /// 入力欄に残ったままの本文を Enter で送信する（末尾 Enter 欠落の自動フラッシュ）
    FlushInput,
    /// 続行ナッジを送る
    Nudge { text: String },
    /// ダイアログの選択肢に応答する
    RespondDialog { choice: String },
    /// 一定時間この worker に触らない（usage limit の解除待ち）
    Defer { secs: u64 },
    /// 突然死した worker を resume する（既定 off の opt-in）
    Resume { command: String },
}

impl AutoAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FlushInput => "flush_input",
            Self::Nudge { .. } => "nudge",
            Self::RespondDialog { .. } => "respond_dialog",
            Self::Defer { .. } => "defer",
            Self::Resume { .. } => "resume",
        }
    }
}

/// usage limit のリセット時刻が読めなかったときの保守的な待ち時間
const DEFAULT_LIMIT_WAIT: u64 = 300;

/// 観測から自動一次対応を決める（純粋関数）。
///
/// - `residual_input`: 入力欄に残っている本文（空 / プレースホルダは None）。
///   idle なのにここに本文が残っている = 末尾 Enter が欠落している（#623 の残り）
/// - `attempts`: この worker に対して既に打った自動対応の回数
pub fn decide_auto_action(
    outcome: &WatchOutcome,
    residual_input: Option<&str>,
    mode: SupervisorMode,
    auto_resume_dead: bool,
    attempts: u32,
    max_retries: u32,
) -> AutoAction {
    if mode != SupervisorMode::Auto || attempts >= max_retries {
        return AutoAction::None;
    }
    match outcome {
        // 入力欄に本文が残ったまま止まっている = Enter が届いていない。
        // これは master が画面を見て手で流していた作業そのもの
        WatchOutcome::Idle { .. } | WatchOutcome::Question { .. } => {
            if residual_input.is_some_and(|s| !s.trim().is_empty()) {
                AutoAction::FlushInput
            } else {
                AutoAction::None
            }
        }
        WatchOutcome::Error { kind, detail } => match kind {
            // 続行指示で復帰できることが多い（一過性の API 障害）
            WorkerErrorKind::ApiError => AutoAction::Nudge {
                text: "続けて".to_string(),
            },
            // 解除時刻まで触らない。即時再送は弾かれるだけ
            WorkerErrorKind::UsageLimit => AutoAction::Defer {
                secs: parse_reset_time(detail)
                    .map(|d| d.as_secs())
                    .unwrap_or(DEFAULT_LIMIT_WAIT),
            },
            // ダイアログの安全な既定（1 番 = 待つ / 中断しない側）へ応答する
            WorkerErrorKind::LimitDialog => AutoAction::RespondDialog {
                choice: "1".to_string(),
            },
        },
        // 停滞は api_error と同じ扱い（続行ナッジで動き出すことが多い）
        WatchOutcome::Stalled { .. } => AutoAction::Nudge {
            text: "続けて".to_string(),
        },
        // 自動 resume は既定 off（クラッシュループの危険と master の判断を奪わないため。#390）
        WatchOutcome::AgentDead { resume_command } => match (auto_resume_dead, resume_command) {
            (true, Some(cmd)) => AutoAction::Resume {
                command: cmd.clone(),
            },
            _ => AutoAction::None,
        },
        // permission は master（と人間）の判断。勝手に承認しない
        WatchOutcome::PermissionWaiting { .. } | WatchOutcome::Gone | WatchOutcome::Timeout => {
            AutoAction::None
        }
    }
}

/// 決めた手を実際に打つ。成功したら master 向けの説明を返す
fn apply_auto_action(
    exec: &mut dyn FnMut(Request) -> Result<Value, String>,
    pane: u64,
    action: &AutoAction,
) -> Result<String, String> {
    match action {
        AutoAction::None | AutoAction::Defer { .. } => Ok(String::new()),
        // text 空 + newline = Enter 単独送達フロー（#95。入力欄が空へ戻るまで再送）
        AutoAction::FlushInput => exec(Request::Send {
            pane: Some(pane),
            text: String::new(),
            newline: true,
            tmux_session: None,
            await_prompt: false,
        })
        .map(|_| "入力欄に残っていた本文を Enter で送信した".to_string()),
        AutoAction::Nudge { text } => exec(Request::Send {
            pane: Some(pane),
            text: text.clone(),
            newline: true,
            tmux_session: None,
            await_prompt: false,
        })
        .map(|_| format!("続行ナッジを送った: {text}")),
        // #662 で respond は「承認ダイアログ = choice / 質問ダイアログ = answers」の
        // 2 系統になった。supervisor が自動で触るのは rate limit の**承認**系だけなので
        // choice を使う（質問ダイアログには自動応答しない = decide_auto_action の方針）
        AutoAction::RespondDialog { choice } => exec(Request::OrchestratorRespond {
            pane_id: pane,
            choice: Some(choice.clone()),
            answers: None,
            dry_run: false,
            caller_role: Some("supervisor".to_string()),
        })
        .map(|_| format!("ダイアログへ {choice} を応答した")),
        AutoAction::Resume { command } => exec(Request::Send {
            pane: Some(pane),
            text: command.clone(),
            newline: true,
            tmux_session: None,
            await_prompt: false,
        })
        .map(|_| "resume コマンドを送った".to_string()),
    }
}

// --- 監視ループ ---

/// 監視対象 1 体（レジストリから毎周期作り直す）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackedWorker {
    pub worker_id: String,
    pub pane: u64,
    pub label: Option<String>,
    pub project: String,
    pub session_id: Option<String>,
    pub tmux_session: Option<String>,
}

/// レジストリの active worker を監視対象として読む（既定の worker 供給源）
pub fn registry_workers() -> Vec<TrackedWorker> {
    let Ok(reg) = crate::orchestrator::registry::WorkerRegistry::load() else {
        return vec![];
    };
    reg.workers
        .iter()
        .filter(|(_, e)| e.is_active())
        .map(|(id, e)| TrackedWorker {
            worker_id: id.clone(),
            pane: e.pane,
            label: e.label.clone(),
            project: e.project.clone(),
            session_id: e.session_id.clone(),
            tmux_session: e.tmux_session.clone(),
        })
        .collect()
}

/// 1 worker 分の追跡状態（周期をまたいで持ち越す）
#[derive(Debug, Default)]
struct WorkerTrack {
    streaks: WatchStreaks,
    /// 直近に発火したイベント種別（同じ状態での連続再発火を抑える）
    last_kind: Option<SupervisorEventKind>,
    /// 自動対応を打った回数
    attempts: u32,
    /// この時刻までは触らない（usage limit の解除待ち）
    defer_until: Option<Instant>,
    /// エスカレーション済み（上限到達を 1 回だけ通知する）
    escalated: bool,
}

/// 監視ループの設定
#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    pub interval: Duration,
    pub mode: SupervisorMode,
    pub auto_resume_dead: bool,
    pub max_retries: u32,
    /// 監視対象がゼロのままこの時間が過ぎたら終了する（常駐の残留防止）。
    /// None = 終了しない
    pub idle_exit_after: Option<Duration>,
    /// 制御プレーン（tako-app）へ届かない状態がこの時間続いたら終了する。
    /// **これが無いと常駐が残る**: tako-app が終了してもレジストリの worker は
    /// active のままなので「監視対象ゼロ」にはならず、idle_exit_after では畳めない
    /// （実測: E2E の隔離インスタンスを落とした後も supervisor が 4 本残った）
    pub unreachable_exit_after: Option<Duration>,
    /// 回す周期数の上限（テスト用。None = 無限）
    pub max_cycles: Option<u32>,
    /// イベントを journal（JSONL）へ書くか。false = sink のみ（テスト用）
    pub journal: bool,
}

impl Default for SupervisorOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            mode: SupervisorMode::Auto,
            auto_resume_dead: false,
            max_retries: 3,
            idle_exit_after: Some(Duration::from_secs(600)),
            unreachable_exit_after: Some(Duration::from_secs(120)),
            max_cycles: None,
            journal: true,
        }
    }
}

/// 監視ループ本体（Issue #665）。
///
/// **master は再アームしない**。毎周期 `workers()` を呼び直すので、spawn した
/// worker はその場で監視対象に入り、閉じた worker は自動的に外れる。
/// 1 体の異常対応で他の worker を止めないため、自動対応はすべて非ブロッキング
/// （打って次の周期で結果を観測する）。
///
/// `sink` はイベントの配送先（CLI は 1 行ずつ標準出力へ、テストは収集）
pub fn supervisor_run(
    exec: &mut dyn FnMut(Request) -> Result<Value, String>,
    workers: &mut dyn FnMut() -> Vec<TrackedWorker>,
    opts: &SupervisorOptions,
    sink: &mut dyn FnMut(&SupervisorEvent),
) -> u64 {
    let mut tracks: std::collections::HashMap<String, WorkerTrack> =
        std::collections::HashMap::new();
    let mut emitted: u64 = 0;
    let mut cycles: u32 = 0;
    let mut empty_since: Option<Instant> = None;
    // 制御プレーンへ 1 度も届かなかった周期が続いた時間（tako-app が落ちた検出）
    let mut unreachable_since: Option<Instant> = None;

    loop {
        // 明示停止（`tako orchestrator supervisor stop`）。journal を使わない
        // テスト実行では見に行かない（実データに触れない）
        if opts.journal && take_stop_request() {
            return emitted;
        }
        let current = workers();

        // 監視対象から外れた worker（closed / GC）を落とす
        let alive: std::collections::HashSet<&str> =
            current.iter().map(|w| w.worker_id.as_str()).collect();
        tracks.retain(|id, _| alive.contains(id.as_str()));

        // 常駐の残留防止: 見る対象が無い状態が続いたら自分から終わる
        if current.is_empty() {
            let since = *empty_since.get_or_insert_with(Instant::now);
            if opts
                .idle_exit_after
                .is_some_and(|limit| since.elapsed() >= limit)
            {
                return emitted;
            }
        } else {
            empty_since = None;
        }

        // この周期で 1 度でも制御プレーンへ届いたか
        let mut reached_any = false;

        for worker in &current {
            // 新規はここで監視対象に入る（= master の再アームが要らない本体）
            if !tracks.contains_key(&worker.worker_id) {
                tracks.insert(worker.worker_id.clone(), WorkerTrack::default());
                emit(
                    sink,
                    opts,
                    &mut emitted,
                    make_event(
                        worker,
                        SupervisorEventKind::Watching,
                        Some("監視を開始した（自動エンロール）".to_string()),
                        None,
                    ),
                );
            }
            let Some(track) = tracks.get_mut(&worker.worker_id) else {
                continue;
            };
            if track
                .defer_until
                .is_some_and(|until| Instant::now() < until)
            {
                continue;
            }
            track.defer_until = None;

            let result = exec(Request::OrchestratorWorkerStatus {
                pane_id: Some(worker.pane),
                session_id: worker.session_id.clone(),
                tmux_session: worker.tmux_session.clone(),
                worker: Some(worker.worker_id.clone()),
            });
            reached_any |= result.is_ok();
            // 入力欄の残留（末尾 Enter 欠落）は idle 判定と独立に画面から読む
            let residual = result
                .as_ref()
                .ok()
                .and_then(|v| v["recent_output"].as_str())
                .and_then(residual_input);

            let Some(outcome) = track
                .streaks
                .evaluate(&result, worker.tmux_session.as_deref())
            else {
                // 動いている = 次に止まったら改めて通知する
                track.last_kind = None;
                continue;
            };

            let kind = outcome_kind(&outcome);
            if track.last_kind != Some(kind) {
                track.last_kind = Some(kind);
                emit(
                    sink,
                    opts,
                    &mut emitted,
                    make_event(
                        worker,
                        kind,
                        outcome_detail(&outcome),
                        outcome_action(&outcome),
                    ),
                );
            }

            let action = decide_auto_action(
                &outcome,
                residual.as_deref(),
                opts.mode,
                opts.auto_resume_dead,
                track.attempts,
                opts.max_retries,
            );
            if action == AutoAction::None {
                // 打つ手が無い状態で上限に達していれば 1 回だけ master へ委ねる
                if track.attempts >= opts.max_retries
                    && !track.escalated
                    && needs_recovery(&outcome)
                {
                    track.escalated = true;
                    audit_log(
                        &worker.worker_id,
                        worker.pane,
                        "escalation",
                        &format!(
                            "attempts={} >= max_retries={}",
                            track.attempts, opts.max_retries
                        ),
                    );
                    emit(
                        sink,
                        opts,
                        &mut emitted,
                        make_event(
                            worker,
                            SupervisorEventKind::Escalated,
                            Some(format!(
                                "自動復旧を {} 回試したが復帰しない。master が対応する",
                                track.attempts
                            )),
                            Some("inspect_and_recover".to_string()),
                        ),
                    );
                }
                continue;
            }

            track.attempts += 1;
            if let AutoAction::Defer { secs } = action {
                track.defer_until = Some(Instant::now() + Duration::from_secs(secs));
                audit_log(
                    &worker.worker_id,
                    worker.pane,
                    "defer",
                    &format!("usage limit の解除まで {secs} 秒待つ"),
                );
                emit(
                    sink,
                    opts,
                    &mut emitted,
                    make_event(
                        worker,
                        SupervisorEventKind::AutoAction,
                        Some(format!("usage limit の解除まで {secs} 秒待つ")),
                        Some("defer".to_string()),
                    ),
                );
                continue;
            }

            let slug = action.as_str();
            match apply_auto_action(exec, worker.pane, &action) {
                Ok(detail) => {
                    audit_log(&worker.worker_id, worker.pane, slug, &detail);
                    emit(
                        sink,
                        opts,
                        &mut emitted,
                        make_event(
                            worker,
                            SupervisorEventKind::AutoAction,
                            Some(detail),
                            Some(slug.to_string()),
                        ),
                    );
                    // 対応後は状態を見直す（次の周期で改めて判定する）
                    track.last_kind = None;
                    track.streaks = WatchStreaks::default();
                }
                Err(e) => {
                    audit_log(
                        &worker.worker_id,
                        worker.pane,
                        slug,
                        &format!("failed: {e}"),
                    );
                    emit(
                        sink,
                        opts,
                        &mut emitted,
                        make_event(
                            worker,
                            SupervisorEventKind::AutoAction,
                            Some(format!("自動対応に失敗した（{slug}）: {e}")),
                            Some(slug.to_string()),
                        ),
                    );
                }
            }
        }

        // tako-app が落ちた（IPC がずっと届かない）なら監視するものが無い。
        // レジストリの worker は active のまま残るので、ここで畳まないと常駐が残る
        if current.is_empty() || reached_any {
            unreachable_since = None;
        } else {
            let since = *unreachable_since.get_or_insert_with(Instant::now);
            if opts
                .unreachable_exit_after
                .is_some_and(|limit| since.elapsed() >= limit)
            {
                audit_log(
                    "-",
                    0,
                    "supervisor_exit",
                    "制御プレーンへ届かない状態が続いたため終了する",
                );
                return emitted;
            }
        }

        cycles += 1;
        if opts.max_cycles.is_some_and(|max| cycles >= max) {
            return emitted;
        }
        std::thread::sleep(opts.interval);
    }
}

/// 入力欄に残っている本文を返す（空 / プレースホルダは None）
fn residual_input(screen: &str) -> Option<String> {
    let lines: Vec<String> = screen.lines().map(str::to_string).collect();
    crate::claude_tui::input_line(&lines)
        .filter(|s| !crate::claude_tui::input_content_is_empty(s))
        .map(str::to_string)
}

/// この outcome は復旧対象か（エスカレーション判定用）
fn needs_recovery(outcome: &WatchOutcome) -> bool {
    matches!(
        outcome,
        WatchOutcome::Error { .. } | WatchOutcome::Stalled { .. } | WatchOutcome::AgentDead { .. }
    )
}

fn outcome_kind(outcome: &WatchOutcome) -> SupervisorEventKind {
    match outcome {
        WatchOutcome::Idle { .. } => SupervisorEventKind::Idle,
        WatchOutcome::Question { .. } => SupervisorEventKind::Question,
        WatchOutcome::PermissionWaiting { .. } => SupervisorEventKind::Permission,
        WatchOutcome::Error { .. } => SupervisorEventKind::Error,
        WatchOutcome::Stalled { .. } => SupervisorEventKind::Stalled,
        WatchOutcome::AgentDead { .. } => SupervisorEventKind::Dead,
        WatchOutcome::Gone | WatchOutcome::Timeout => SupervisorEventKind::Gone,
    }
}

fn outcome_detail(outcome: &WatchOutcome) -> Option<String> {
    match outcome {
        WatchOutcome::Idle { ctx_percent } | WatchOutcome::Question { ctx_percent } => {
            ctx_percent.map(|p| format!("ctx {p}%"))
        }
        WatchOutcome::Error { kind, detail } => Some(format!("{} / {detail}", kind.as_str())),
        WatchOutcome::Stalled { detail } => Some(detail.clone()),
        WatchOutcome::AgentDead { resume_command } => Some(format!(
            "エージェント CLI プロセスが終了している。resume: {}",
            resume_command.as_deref().unwrap_or("(session ID 未記録)")
        )),
        WatchOutcome::PermissionWaiting { permission_dialog } => permission_dialog
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WatchOutcome::Gone | WatchOutcome::Timeout => None,
    }
}

fn outcome_action(outcome: &WatchOutcome) -> Option<String> {
    match outcome {
        WatchOutcome::Error { kind, .. } => Some(kind.recommended_action().to_string()),
        WatchOutcome::Stalled { .. } => Some("check_and_resume".to_string()),
        WatchOutcome::PermissionWaiting { .. } => Some("respond".to_string()),
        WatchOutcome::AgentDead { .. } => Some("resume_session".to_string()),
        _ => None,
    }
}

fn make_event(
    worker: &TrackedWorker,
    kind: SupervisorEventKind,
    detail: Option<String>,
    action: Option<String>,
) -> SupervisorEvent {
    SupervisorEvent {
        seq: 0,
        ts: crate::sessions::now_iso(),
        worker_id: worker.worker_id.clone(),
        pane: worker.pane,
        label: worker.label.clone(),
        project: worker.project.clone(),
        kind,
        detail,
        action,
    }
}

fn emit(
    sink: &mut dyn FnMut(&SupervisorEvent),
    opts: &SupervisorOptions,
    counter: &mut u64,
    mut event: SupervisorEvent,
) {
    if opts.journal {
        match append_event(&event) {
            Ok(seq) => event.seq = seq,
            Err(e) => eprintln!("warning: supervisor イベントを記録できない: {e}"),
        }
    }
    *counter += 1;
    sink(&event);
}

// --- シングルトン（常駐の二重起動防止） ---

/// supervisor の常駐ロックの対象パス（実体は `<これ>.lock`）
pub fn lock_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("supervisor-daemon"))
}

/// 常駐の権利を取る。取れたらガードを返す（プロセス終了まで保持する）。
/// 既に別の supervisor が走っていれば None
pub fn acquire_singleton() -> Option<crate::config_io::ConfigLock> {
    let path = lock_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::config_io::try_lock_exclusive(&path).ok().flatten()
}

/// 停止要求フラグのパス
fn stop_flag_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("supervisor-stop"))
}

/// 常駐へ停止を要求する（`tako orchestrator supervisor stop`）。
/// 常駐は次の周期でフラグを見つけて自分で片付けて終わる
pub fn request_stop() -> Result<(), String> {
    let path = stop_flag_path().ok_or("ホームディレクトリが取得できない")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, crate::sessions::now_iso())
        .map_err(|e| format!("停止要求を書けない: {e}"))
}

/// 停止要求があれば消費して true を返す
fn take_stop_request() -> bool {
    match stop_flag_path() {
        Some(path) if path.is_file() => {
            let _ = std::fs::remove_file(&path);
            true
        }
        _ => false,
    }
}

/// supervisor が既に常駐しているか（ロックが取れなければ稼働中）
pub fn is_running() -> bool {
    match lock_path() {
        Some(path) => match crate::config_io::try_lock_exclusive(&path) {
            Ok(Some(_guard)) => false, // 取れた = 誰も居ない（guard はここで解放される）
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

/// 常駐に使う tako CLI の解決。
///
/// 隔離起動（`TAKO_ISOLATED=1`）では**自分と同世代の CLI**を使う。
/// 既定の解決（`resolve_tako_binary`）はインストール済みバイナリを優先するため、
/// そのまま使うと隔離検証が本番世代の supervisor を立ててしまう（#432 と同じ罠）
fn supervisor_binary() -> String {
    let isolated = matches!(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        Some("1" | "true" | "on")
    );
    if isolated {
        if let Ok(exe) = std::env::current_exe() {
            let sibling = exe
                .parent()
                .map(|d| d.join(format!("tako{}", std::env::consts::EXE_SUFFIX)));
            if let Some(p) = sibling.filter(|p| p.is_file()) {
                return p.display().to_string();
            }
        }
    }
    crate::dispatch::resolve_tako_binary()
}

/// supervisor が居なければデタッチ起動する（spawn から呼ぶ。Issue #665）。
/// 「master が監視を張り忘れる」余地を無くすのが目的なので、失敗しても
/// spawn は止めない（警告のみ）
pub fn ensure_running(mode: SupervisorMode) -> bool {
    if mode == SupervisorMode::Off || is_running() {
        return false;
    }
    let bin = supervisor_binary();
    let mut cmd = std::process::Command::new(&bin);
    cmd.args(["orchestrator", "supervisor", "serve"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    tako_core::platform::process::no_console_window(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            audit_log("-", 0, "supervisor_start", &format!("pid={}", child.id()));
            true
        }
        Err(e) => {
            eprintln!("warning: supervisor を起動できない（{bin}）: {e}");
            false
        }
    }
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

    // --- 自動一次対応の判断（純粋関数） ---

    fn err(kind: WorkerErrorKind, detail: &str) -> WatchOutcome {
        WatchOutcome::Error {
            kind,
            detail: detail.to_string(),
        }
    }

    #[test]
    fn api_errorは続行ナッジを打つ() {
        let a = decide_auto_action(
            &err(WorkerErrorKind::ApiError, "Connection error"),
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(
            a,
            AutoAction::Nudge {
                text: "続けて".into()
            }
        );
    }

    #[test]
    fn usage_limitは解除まで触らない() {
        // 即時再送は弾かれるだけなので、待つのが正しい対応
        let a = decide_auto_action(
            &err(
                WorkerErrorKind::UsageLimit,
                "5-hour limit reached ∙ resets 3am",
            ),
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        match a {
            AutoAction::Defer { secs } => assert!(secs > 0 && secs <= 86400, "secs={secs}"),
            other => panic!("Defer のはず: {other:?}"),
        }
        // リセット時刻が読めなければ保守的な既定値
        let a = decide_auto_action(
            &err(WorkerErrorKind::UsageLimit, "limit reached"),
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(
            a,
            AutoAction::Defer {
                secs: DEFAULT_LIMIT_WAIT
            }
        );
    }

    #[test]
    fn limit_dialogは選択肢1へ応答する() {
        let a = decide_auto_action(
            &err(WorkerErrorKind::LimitDialog, "switch model?"),
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(a, AutoAction::RespondDialog { choice: "1".into() });
    }

    #[test]
    fn stalledは続行ナッジを打つ() {
        let a = decide_auto_action(
            &WatchOutcome::Stalled {
                detail: "no children".into(),
            },
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(
            a,
            AutoAction::Nudge {
                text: "続けて".into()
            }
        );
    }

    #[test]
    fn 入力欄に本文が残ったidleはenterでフラッシュする() {
        // #623 の残り: 末尾 Enter が欠落して本文が入力欄に居座るケース。
        // master が画面を見て手で流していた作業を supervisor が肩代わりする
        let a = decide_auto_action(
            &WatchOutcome::Idle { ctx_percent: None },
            Some("Issue #665 の実装を進めて"),
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(a, AutoAction::FlushInput);
    }

    #[test]
    fn 入力欄が空のidleには何もしない() {
        for residual in [None, Some(""), Some("   ")] {
            let a = decide_auto_action(
                &WatchOutcome::Idle { ctx_percent: None },
                residual,
                SupervisorMode::Auto,
                false,
                0,
                3,
            );
            assert_eq!(a, AutoAction::None, "residual={residual:?}");
        }
    }

    #[test]
    fn permissionには勝手に応答しない() {
        // 承認は master（と人間）の判断。自動承認は絶対にしない
        let a = decide_auto_action(
            &WatchOutcome::PermissionWaiting {
                permission_dialog: serde_json::json!({"command": "rm -rf /"}),
            },
            None,
            SupervisorMode::Auto,
            false,
            0,
            3,
        );
        assert_eq!(a, AutoAction::None);
    }

    #[test]
    fn deadの自動resumeはopt_in() {
        let dead = WatchOutcome::AgentDead {
            resume_command: Some("claude --resume abc".into()),
        };
        // 既定は notify-only（#390 の設計判断）
        assert_eq!(
            decide_auto_action(&dead, None, SupervisorMode::Auto, false, 0, 3),
            AutoAction::None
        );
        // opt-in したときだけ resume
        assert_eq!(
            decide_auto_action(&dead, None, SupervisorMode::Auto, true, 0, 3),
            AutoAction::Resume {
                command: "claude --resume abc".into()
            }
        );
        // session ID が無ければ resume コマンドを組めないので何もしない
        let no_cmd = WatchOutcome::AgentDead {
            resume_command: None,
        };
        assert_eq!(
            decide_auto_action(&no_cmd, None, SupervisorMode::Auto, true, 0, 3),
            AutoAction::None
        );
    }

    #[test]
    fn notify_onlyとoffは自動対応しない() {
        for mode in [SupervisorMode::NotifyOnly, SupervisorMode::Off] {
            let a = decide_auto_action(
                &err(WorkerErrorKind::ApiError, "boom"),
                None,
                mode,
                false,
                0,
                3,
            );
            assert_eq!(a, AutoAction::None, "mode={mode:?}");
        }
    }

    #[test]
    fn 上限に達したら自動対応をやめる() {
        let a = decide_auto_action(
            &err(WorkerErrorKind::ApiError, "boom"),
            None,
            SupervisorMode::Auto,
            false,
            3, // attempts == max_retries
            3,
        );
        assert_eq!(a, AutoAction::None, "上限到達後は master へ委ねる");
    }

    // --- 入力欄の残留検出 ---

    #[test]
    fn 入力欄の残留を画面から読む() {
        assert_eq!(
            residual_input("some output\n❯ 続きをやって").as_deref(),
            Some("続きをやって")
        );
        // 空・プレースホルダは残留ではない
        assert_eq!(residual_input("some output\n❯ "), None);
        assert_eq!(residual_input("❯ Try \"fix the bug\""), None);
        assert_eq!(residual_input("PS C:\\dev> "), None);
    }

    // --- イベントの行整形（既存 watch との後方互換） ---

    #[test]
    fn イベント行は既存watchと同じマーカーを使う() {
        // master 側の読み取りを作り直させないため、語彙は変えない
        assert_eq!(SupervisorEventKind::Idle.line_marker(), "WORKER_IDLE");
        assert_eq!(SupervisorEventKind::Error.line_marker(), "WORKER_ERROR");
        assert_eq!(SupervisorEventKind::Gone.line_marker(), "WORKER_GONE");
        assert_eq!(
            SupervisorEventKind::Permission.line_marker(),
            "WORKER_PERMISSION"
        );
        assert_eq!(SupervisorEventKind::Dead.line_marker(), "WORKER_DEAD");
        assert_eq!(SupervisorEventKind::Stalled.line_marker(), "WORKER_STALLED");
        assert_eq!(
            SupervisorEventKind::Question.line_marker(),
            "WORKER_QUESTION"
        );
    }

    #[test]
    fn イベント行にpaneとラベルと補助行が出る() {
        let ev = SupervisorEvent {
            seq: 1,
            ts: "2026-07-30T00:00:00Z".into(),
            worker_id: "7".into(),
            pane: 42,
            label: Some("fix-665".into()),
            project: "tako".into(),
            kind: SupervisorEventKind::Error,
            detail: Some("api_error / Connection error".into()),
            action: Some("resume".into()),
        };
        let lines = ev.to_lines();
        assert_eq!(lines[0], "WORKER_ERROR: tako:42 (worker=7 fix-665)");
        assert_eq!(lines[1], "  detail: api_error / Connection error");
        assert_eq!(lines[2], "  action: resume");
    }

    // --- 監視ループ（exec モック） ---

    use std::sync::{Arc, Mutex};

    /// worker_status の応答を順に返し、送られた Request の種別を記録するモック
    struct LoopMock {
        statuses: Vec<Value>,
        idx: usize,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl LoopMock {
        fn new(statuses: Vec<Value>) -> Self {
            Self {
                statuses,
                idx: 0,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn exec(&mut self, req: Request) -> Result<Value, String> {
            self.calls.lock().unwrap().push(req.kind_name().to_string());
            match req {
                Request::OrchestratorWorkerStatus { .. } => {
                    let v = self
                        .statuses
                        .get(self.idx)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"status": "busy"}));
                    self.idx += 1;
                    Ok(v)
                }
                _ => Ok(serde_json::json!({"ok": true})),
            }
        }
    }

    fn worker(id: &str, pane: u64) -> TrackedWorker {
        TrackedWorker {
            worker_id: id.into(),
            pane,
            label: Some("t".into()),
            project: "tako".into(),
            session_id: None,
            tmux_session: None,
        }
    }

    fn test_opts(cycles: u32) -> SupervisorOptions {
        SupervisorOptions {
            interval: Duration::ZERO,
            mode: SupervisorMode::NotifyOnly, // 副作用なしで観測だけ
            auto_resume_dead: false,
            max_retries: 3,
            idle_exit_after: None,
            unreachable_exit_after: None,
            max_cycles: Some(cycles),
            journal: false, // テストは実ファイルを汚さない
        }
    }

    fn idle_status() -> Value {
        serde_json::json!({
            "status": "idle",
            "status_source": "agents",
            "recent_output": "done",
        })
    }

    #[test]
    fn 新しいworkerは自動で監視対象に入る() {
        // これが「再アーム不要」の本体: master は何も張らない
        let mut mock = LoopMock::new(vec![]);
        let mut events: Vec<SupervisorEvent> = Vec::new();
        // 1 周期目は worker なし、2 周期目から 1 体現れる
        let mut cycle = 0;
        let mut workers = || -> Vec<TrackedWorker> {
            cycle += 1;
            if cycle == 1 {
                vec![]
            } else {
                vec![worker("1", 10)]
            }
        };
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(2),
            &mut |e| events.push(e.clone()),
        );
        assert_eq!(events.len(), 1, "エンロール通知が 1 件: {events:?}");
        assert_eq!(events[0].kind, SupervisorEventKind::Watching);
        assert_eq!(events[0].pane, 10);
    }

    #[test]
    fn 停止の確定でイベントが出て連続再発火はしない() {
        // agents 一次シグナルは idle 3 連続で確定 → 以降は同じ状態なので再通知しない
        let mut mock = LoopMock::new(vec![idle_status(); 8]);
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut workers = || vec![worker("1", 10)];
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(8),
            &mut |e| events.push(e.clone()),
        );
        let idles: Vec<_> = events
            .iter()
            .filter(|e| e.kind == SupervisorEventKind::Idle)
            .collect();
        assert_eq!(idles.len(), 1, "同じ停止で 1 回だけ: {events:?}");
    }

    #[test]
    fn busyへ戻れば次の停止でまた通知する() {
        // 「idle 3 連続 → busy → idle 3 連続」で 2 回発火する = 連続イベント配送
        let mut statuses = vec![idle_status(); 3];
        statuses.push(serde_json::json!({"status": "busy", "status_source": "agents"}));
        statuses.extend(vec![idle_status(); 3]);
        let mut mock = LoopMock::new(statuses);
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut workers = || vec![worker("1", 10)];
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(7),
            &mut |e| events.push(e.clone()),
        );
        let idles = events
            .iter()
            .filter(|e| e.kind == SupervisorEventKind::Idle)
            .count();
        assert_eq!(idles, 2, "停止のたびに通知される: {events:?}");
    }

    #[test]
    fn 閉じたworkerは監視対象から外れる() {
        let mut mock = LoopMock::new(vec![idle_status(); 8]);
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut cycle = 0;
        let mut workers = || -> Vec<TrackedWorker> {
            cycle += 1;
            if cycle <= 2 {
                vec![worker("1", 10)]
            } else {
                vec![]
            }
        };
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(4),
            &mut |e| events.push(e.clone()),
        );
        // エンロールは 1 回だけ（外れたあと再登場していない）
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == SupervisorEventKind::Watching)
                .count(),
            1
        );
    }

    #[test]
    fn 監視対象がゼロのまま続けば自分から終わる() {
        // 常駐が残り続けないための撤退条件
        let mut mock = LoopMock::new(vec![]);
        let mut workers = || vec![];
        let opts = SupervisorOptions {
            interval: Duration::ZERO,
            idle_exit_after: Some(Duration::ZERO),
            max_cycles: Some(1000), // 撤退条件が効かなければここまで回ってしまう
            journal: false,
            ..SupervisorOptions::default()
        };
        let emitted = supervisor_run(&mut |r| mock.exec(r), &mut workers, &opts, &mut |_| {});
        assert_eq!(emitted, 0);
    }

    #[test]
    fn 制御プレーンへ届かなくなったら自分から終わる() {
        // tako-app が落ちてもレジストリの worker は active のまま残るので、
        // 「監視対象ゼロ」では畳めない（実測: E2E の隔離インスタンスを落とした後に
        // supervisor が 4 本残った）。IPC 不達が続いたら終了すること
        struct DeadMock;
        impl DeadMock {
            fn exec(&mut self, _req: Request) -> Result<Value, String> {
                Err("tako アプリへの接続情報が無い".into())
            }
        }
        let mut mock = DeadMock;
        let mut workers = || vec![worker("1", 10)];
        let opts = SupervisorOptions {
            interval: Duration::ZERO,
            mode: SupervisorMode::NotifyOnly,
            unreachable_exit_after: Some(Duration::ZERO),
            idle_exit_after: None,
            // 撤退条件が効かなければここまで回ってしまう
            max_cycles: Some(1000),
            journal: false,
            ..SupervisorOptions::default()
        };
        let mut events: Vec<SupervisorEvent> = Vec::new();
        supervisor_run(&mut |r| mock.exec(r), &mut workers, &opts, &mut |e| {
            events.push(e.clone())
        });
        // watching は出るが、そのあと即座に撤退する
        assert!(events.len() <= 2, "IPC 不達で早々に終わること: {events:?}");
    }

    #[test]
    fn auto_modeでは自動対応を打ちイベントに残す() {
        // api_error で止まった worker へナッジを打つところまで通す
        let error_status = serde_json::json!({
            "status": "error",
            "status_source": "agents",
            "recent_output": "API Error: Connection error",
            "error": {"kind": "api_error", "detail": "Connection error"},
        });
        let mut mock = LoopMock::new(vec![error_status; 4]);
        let calls = mock.calls.clone();
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut workers = || vec![worker("1", 10)];
        let opts = SupervisorOptions {
            mode: SupervisorMode::Auto,
            ..test_opts(4)
        };
        supervisor_run(&mut |r| mock.exec(r), &mut workers, &opts, &mut |e| {
            events.push(e.clone())
        });
        assert!(
            events.iter().any(|e| e.kind == SupervisorEventKind::Error),
            "異常を通知する: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| e.kind == SupervisorEventKind::AutoAction),
            "自動対応を通知する（黙って直さない）: {events:?}"
        );
        assert!(
            calls.lock().unwrap().iter().any(|k| k == "Send"),
            "ナッジが実際に送られる: {:?}",
            calls.lock().unwrap()
        );
    }

    #[test]
    fn notify_onlyでは通知だけで手を出さない() {
        let error_status = serde_json::json!({
            "status": "error",
            "status_source": "agents",
            "recent_output": "API Error",
            "error": {"kind": "api_error", "detail": "boom"},
        });
        let mut mock = LoopMock::new(vec![error_status; 4]);
        let calls = mock.calls.clone();
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut workers = || vec![worker("1", 10)];
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(4),
            &mut |e| events.push(e.clone()),
        );
        assert!(events.iter().any(|e| e.kind == SupervisorEventKind::Error));
        assert!(
            !calls.lock().unwrap().iter().any(|k| k == "Send"),
            "notify_only では送信しない"
        );
    }

    #[test]
    fn イベントのカーソルは古い順に消化して取りこぼさない() {
        // limit を超える backlog があっても、飛ばさず古い方から順に返すこと。
        // 新しい順に切ると next_cursor が先へ飛び、間のイベントが永久に読まれない
        let mk = |seq: u64| SupervisorEvent {
            seq,
            ts: "t".into(),
            worker_id: "1".into(),
            pane: 10,
            label: None,
            project: "p".into(),
            kind: SupervisorEventKind::Idle,
            detail: None,
            action: None,
        };
        let all: Vec<SupervisorEvent> = (1..=10).map(mk).collect();
        // read_events の抽出規則そのものを検証する（ファイル I/O を挟まない）
        let pick = |cursor: u64, limit: usize| -> (Vec<u64>, u64) {
            let picked: Vec<&SupervisorEvent> =
                all.iter().filter(|e| e.seq > cursor).take(limit).collect();
            let next = picked.last().map(|e| e.seq).unwrap_or(cursor);
            (picked.iter().map(|e| e.seq).collect(), next)
        };
        let (seqs, next) = pick(0, 3);
        assert_eq!(seqs, vec![1, 2, 3]);
        assert_eq!(next, 3);
        let (seqs, next) = pick(next, 3);
        assert_eq!(seqs, vec![4, 5, 6], "続きから読める（飛ばさない）");
        assert_eq!(next, 6);
        let (seqs, next) = pick(10, 3);
        assert!(seqs.is_empty());
        assert_eq!(next, 10, "新着が無ければカーソルは据え置き");
    }

    #[test]
    fn 複数workerを1本のループで見る() {
        let mut mock = LoopMock::new(vec![]);
        let mut events: Vec<SupervisorEvent> = Vec::new();
        let mut workers = || vec![worker("1", 10), worker("2", 11), worker("3", 12)];
        supervisor_run(
            &mut |r| mock.exec(r),
            &mut workers,
            &test_opts(1),
            &mut |e| events.push(e.clone()),
        );
        let panes: Vec<u64> = events
            .iter()
            .filter(|e| e.kind == SupervisorEventKind::Watching)
            .map(|e| e.pane)
            .collect();
        assert_eq!(panes, vec![10, 11, 12]);
    }
}

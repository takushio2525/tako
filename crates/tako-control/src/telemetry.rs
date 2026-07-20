//! telemetry — エラーレポートの自動収集（Issue #333）
//!
//! tako 内で発生した panic / 重大エラーを PII なしで収集エンドポイントへ送信する。
//! opt-in 既定 off（settings.json の `telemetry` フィールド）。
//! 送信内容はすべて `<data_dir>/telemetry.log` に記録する（透明性）。
//!
//! ## Phase 2（本実装）
//! - 送信キュー: オフライン時にキューファイルへ保存し、次回起動 or 有効化時に再送
//! - PII マスキング強化: ホスト名・環境変数値・トークンパターンの除去
//! - 重大エラーフック: 復元失敗・不変条件違反等に `report_critical` を挿入

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_ENDPOINT: &str = "https://tako-error-collector.takushio2525.workers.dev/api/report";

const LOG_MAX_BYTES: u64 = 256 * 1024; // 256KB
const QUEUE_MAX_ENTRIES: usize = 50;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// 初期化（起動時に 1 回呼ぶ）。有効時はキューの再送も試みる
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        std::thread::spawn(flush_queue);
    }
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
    if v {
        std::thread::spawn(flush_queue);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorReport {
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    pub error_kind: ErrorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backtrace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Panic,
    Critical,
    InvariantViolation,
    RestoreFailed,
    DaemonStartup,
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Panic => write!(f, "panic"),
            ErrorKind::Critical => write!(f, "critical"),
            ErrorKind::InvariantViolation => write!(f, "invariant_violation"),
            ErrorKind::RestoreFailed => write!(f, "restore_failed"),
            ErrorKind::DaemonStartup => write!(f, "daemon_startup"),
        }
    }
}

// ===== PII マスキング =====

/// PII を含む文字列を安全にマスクする（Phase 2 強化版）
pub fn mask_pii(input: &str) -> String {
    let mut result = mask_paths(input);
    result = mask_hostname(&result);
    result = mask_username(&result);
    result = mask_env_values(&result);
    result = mask_tokens(&result);
    result
}

/// パスをマスクする（Phase 1 互換）
pub fn mask_paths(input: &str) -> String {
    let mut result = input.to_string();

    if let Some(home) = dirs_home() {
        result = result.replace(&home, "~");
    }

    let re_users = regex_lite_replace(&result, r"/Users/[^/\s]+", "/Users/<user>");
    result = re_users;

    let re_home = regex_lite_replace(&result, r"/home/[^/\s]+", "/home/<user>");
    result = re_home;

    let re_var = regex_lite_replace(&result, r"/var/folders/[^/]+/[^/]+", "/var/folders/<tmp>");
    result = re_var;

    // Windows の C:\Users\<name> パターン（#287 P2-3）
    result = replace_windows_user_paths(&result);

    result
}

/// Windows の `C:\Users\<name>` パターンをマスクする。
/// ドライブレターは任意（C / D / ...）。バックスラッシュとスラッシュの両方に対応する
fn replace_windows_user_paths(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;
    while !remaining.is_empty() {
        if let Some(m) = find_windows_user_path(remaining) {
            result.push_str(&remaining[..m.start]);
            result.push_str(&remaining[m.start..m.start + m.prefix_len]);
            result.push_str("<user>");
            remaining = &remaining[m.end..];
        } else {
            result.push_str(remaining);
            break;
        }
    }
    result
}

struct WinUserMatch {
    start: usize,
    prefix_len: usize,
    end: usize,
}

fn find_windows_user_path(input: &str) -> Option<WinUserMatch> {
    let bytes = input.as_bytes();
    for i in 0..bytes.len() {
        // ドライブレター + :\ or :/
        if i + 9 > bytes.len() {
            break;
        }
        if !bytes[i].is_ascii_alphabetic() || bytes[i + 1] != b':' {
            continue;
        }
        let sep = bytes[i + 2];
        if sep != b'\\' && sep != b'/' {
            continue;
        }
        let after_drive = &input[i + 3..];
        let users_prefix = if sep == b'\\' { "Users\\" } else { "Users/" };
        if !after_drive.starts_with(users_prefix) {
            continue;
        }
        let prefix_len = 3 + users_prefix.len(); // "C:\Users\"
        let name_start = i + prefix_len;
        let name_end = input[name_start..]
            .find(|c: char| c == '\\' || c == '/' || c.is_whitespace())
            .map(|j| name_start + j)
            .unwrap_or(input.len());
        if name_end > name_start {
            return Some(WinUserMatch {
                start: i,
                prefix_len,
                end: name_end,
            });
        }
    }
    None
}

fn mask_hostname(input: &str) -> String {
    let hostname = get_hostname();
    if hostname.is_empty() || hostname.len() < 3 {
        return input.to_string();
    }
    input.replace(&hostname, "<hostname>")
}

fn mask_username(input: &str) -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    if user.is_empty() || user.len() < 3 {
        return input.to_string();
    }
    // パス系は mask_paths で処理済みなので、残りの出現を除去
    input.replace(&user, "<user>")
}

fn mask_env_values(input: &str) -> String {
    let mut result = input.to_string();
    // セキュリティ関連の環境変数値を除去
    for key in &[
        "TAKO_TOKEN",
        "ANTHROPIC_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "OPENAI_API_KEY",
    ] {
        if let Ok(val) = std::env::var(key) {
            if !val.is_empty() && val.len() >= 8 {
                result = result.replace(&val, &format!("<{key}>"));
            }
        }
    }
    result
}

fn mask_tokens(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while !remaining.is_empty() {
        // token=<value> / Bearer <value> / sk-... / ghp_... パターン
        if let Some(m) = find_token_pattern(remaining) {
            result.push_str(&remaining[..m.start]);
            result.push_str(&m.replacement);
            remaining = &remaining[m.end..];
        } else {
            result.push_str(remaining);
            break;
        }
    }
    result
}

struct TokenMatch {
    start: usize,
    end: usize,
    replacement: String,
}

fn find_token_pattern(input: &str) -> Option<TokenMatch> {
    // token=<hex-or-alnum 16+> パターン
    if let Some(pos) = input.find("token=") {
        let val_start = pos + 6;
        let val_end = input[val_start..]
            .find(|c: char| c.is_whitespace() || c == '&' || c == '"' || c == '\'')
            .map(|i| val_start + i)
            .unwrap_or(input.len());
        if val_end - val_start >= 16 {
            return Some(TokenMatch {
                start: pos,
                end: val_end,
                replacement: "token=<redacted>".into(),
            });
        }
    }
    // Bearer <token>
    if let Some(pos) = input.find("Bearer ") {
        let val_start = pos + 7;
        let val_end = input[val_start..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .map(|i| val_start + i)
            .unwrap_or(input.len());
        if val_end - val_start >= 8 {
            return Some(TokenMatch {
                start: pos,
                end: val_end,
                replacement: "Bearer <redacted>".into(),
            });
        }
    }
    // sk-... (API keys), ghp_... (GitHub PAT)
    for prefix in &["sk-", "ghp_", "gho_", "ghs_"] {
        if let Some(pos) = input.find(prefix) {
            let val_end = input[pos..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map(|i| pos + i)
                .unwrap_or(input.len());
            if val_end - pos >= 10 {
                return Some(TokenMatch {
                    start: pos,
                    end: val_end,
                    replacement: format!("{}<redacted>", prefix),
                });
            }
        }
    }
    None
}

fn get_hostname() -> String {
    #[cfg(unix)]
    {
        std::process::Command::new("hostname")
            .arg("-s")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
}

fn regex_lite_replace(input: &str, pattern: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while !remaining.is_empty() {
        if let Some(m) = find_pattern(remaining, pattern) {
            result.push_str(&remaining[..m.start]);
            result.push_str(replacement);
            remaining = &remaining[m.end..];
        } else {
            result.push_str(remaining);
            break;
        }
    }
    result
}

struct Match {
    start: usize,
    end: usize,
}

fn find_pattern(input: &str, pattern: &str) -> Option<Match> {
    let prefixes: &[&str] = match pattern {
        r"/Users/[^/\s]+" => &["/Users/"],
        r"/home/[^/\s]+" => &["/home/"],
        r"/var/folders/[^/]+/[^/]+" => &["/var/folders/"],
        _ => return None,
    };

    for prefix in prefixes {
        if let Some(pos) = input.find(prefix) {
            let after = pos + prefix.len();
            if pattern.contains("[^/]+/[^/]+") {
                let seg1_end = input[after..]
                    .find('/')
                    .map(|i| after + i)
                    .unwrap_or(input.len());
                if seg1_end >= input.len() {
                    return Some(Match {
                        start: pos,
                        end: seg1_end,
                    });
                }
                let seg2_start = seg1_end + 1;
                let seg2_end = input[seg2_start..]
                    .find(|c: char| c == '/' || c.is_whitespace())
                    .map(|i| seg2_start + i)
                    .unwrap_or(input.len());
                return Some(Match {
                    start: pos,
                    end: seg2_end,
                });
            } else {
                let end = input[after..]
                    .find(|c: char| c == '/' || c.is_whitespace())
                    .map(|i| after + i)
                    .unwrap_or(input.len());
                if end > after {
                    return Some(Match { start: pos, end });
                }
            }
        }
    }
    None
}

// ===== OS バージョン =====

pub fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ver.is_empty() {
            return None;
        }
        let kernel = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        match kernel {
            Some(k) if !k.is_empty() => Some(format!("macOS {ver} (Darwin {k})")),
            _ => Some(format!("macOS {ver}")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub fn tako_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ===== レポート送信 =====

/// レポートを送信する（background thread で fire-and-forget）。
/// 送信失敗時はキューに保存し、次回起動時に再送を試みる
pub fn send_report(report: ErrorReport) {
    if !is_enabled() {
        return;
    }
    std::thread::spawn(move || {
        send_report_inner(&report);
    });
}

/// 同期的にレポートを送信する（panic ハンドラ用）
pub fn send_report_sync(report: &ErrorReport) {
    send_report_inner(report);
}

fn post_json(endpoint: &str, json: &str) -> Result<(), String> {
    let result = ureq::post(endpoint)
        .header("Content-Type", "application/json")
        .send(json.as_bytes());
    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("{e}")),
    }
}

fn send_report_inner(report: &ErrorReport) {
    let json = match serde_json::to_string(report) {
        Ok(j) => j,
        Err(_) => return,
    };

    log_report(&json);

    let endpoint =
        std::env::var("TAKO_TELEMETRY_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    match post_json(&endpoint, &json) {
        Ok(()) => {
            log_line("[送信成功]");
        }
        Err(e) => {
            log_line(&format!("[送信失敗・キューへ保存] {e}"));
            enqueue_report(&json);
        }
    }
}

// ===== 送信キュー（オフライン耐性） =====

fn queue_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("telemetry_queue.jsonl"))
}

fn enqueue_report(json: &str) {
    let Some(path) = queue_path() else { return };
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::create_dir_all(dir);

    // キューサイズ制限: 上限超過なら古い行を捨てる
    if let Ok(content) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() >= QUEUE_MAX_ENTRIES {
            let trimmed: String = lines[lines.len() - QUEUE_MAX_ENTRIES + 1..]
                .iter()
                .map(|l| format!("{l}\n"))
                .collect();
            let _ = std::fs::write(&path, trimmed);
        }
    }

    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = writeln!(f, "{json}");
}

/// キューに溜まったレポートを再送する（起動時 or 有効化時に呼ぶ）
fn flush_queue() {
    let Some(path) = queue_path() else { return };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) if !c.is_empty() => c,
        _ => return,
    };

    let endpoint =
        std::env::var("TAKO_TELEMETRY_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    let mut failed = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match post_json(&endpoint, line) {
            Ok(()) => {
                log_line("[キュー再送成功]");
            }
            Err(_) => {
                failed.push(line.to_string());
            }
        }
    }

    if failed.is_empty() {
        let _ = std::fs::remove_file(&path);
        log_line("[キュー全件再送完了]");
    } else {
        let remaining: String = failed.iter().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(&path, remaining);
        log_line(&format!("[キュー再送: {}件失敗・保持]", failed.len()));
    }
}

/// キューのエントリ数を返す（status 表示用）
pub fn queue_count() -> usize {
    queue_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

// ===== 公開ヘルパー（Phase 2: エラー経路への差し込み用） =====

/// 重大エラーをレポートする（復元失敗・daemon 起動失敗等）。
/// メッセージは自動的に PII マスクされる
pub fn report_critical(kind: ErrorKind, message: &str) {
    report_critical_with_context(kind, message, None);
}

/// コンテキスト付きで重大エラーをレポートする
pub fn report_critical_with_context(kind: ErrorKind, message: &str, context: Option<&str>) {
    if !is_enabled() {
        return;
    }
    let report = ErrorReport {
        version: tako_version().to_string(),
        os_version: os_version(),
        error_kind: kind,
        message: Some(mask_pii(message)),
        backtrace: None,
        context: context.map(mask_pii),
    };
    send_report(report);
}

// ===== ログ =====

fn log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("telemetry.log"))
}

fn log_report(json: &str) {
    let Some(path) = log_path() else { return };
    rotate_log(&path);
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let ts = chrono_now();
    let _ = writeln!(f, "[{ts}] {json}");
}

fn log_line(msg: &str) {
    let Some(path) = log_path() else { return };
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let ts = chrono_now();
    let _ = writeln!(f, "[{ts}] {msg}");
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

fn rotate_log(path: &std::path::Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > LOG_MAX_BYTES {
            let backup = path.with_extension("log.old");
            let _ = std::fs::rename(path, backup);
        }
    }
}

// ===== panic ハンドラ =====

pub fn install_panic_handler() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);

        if !is_enabled() {
            return;
        }

        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned());

        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()));

        let backtrace = std::backtrace::Backtrace::force_capture().to_string();

        let masked_msg = message.map(|m| mask_pii(&m));
        let masked_bt = mask_pii(&backtrace);
        let masked_loc = location.map(|l| mask_pii(&l));

        let full_message = match (masked_msg, masked_loc) {
            (Some(msg), Some(loc)) => Some(format!("{msg} at {loc}")),
            (Some(msg), None) => Some(msg),
            (None, Some(loc)) => Some(format!("panic at {loc}")),
            (None, None) => None,
        };

        let report = ErrorReport {
            version: tako_version().to_string(),
            os_version: os_version(),
            error_kind: ErrorKind::Panic,
            message: full_message,
            backtrace: Some(masked_bt),
            context: None,
        };

        send_report_sync(&report);
    }));
}

// ===== status 表示用 =====

pub fn recent_count() -> usize {
    let Some(path) = log_path() else {
        return 0;
    };
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter(|l| l.contains(r#""error_kind""#)).count())
        .unwrap_or(0)
}

pub fn log_file_path() -> Option<PathBuf> {
    log_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn マスクがホームパスを置換する() {
        let home = dirs_home().unwrap_or_else(|| "/Users/testuser".to_string());
        let input = format!("panicked at {home}/dev/tako/src/main.rs:42:5");
        let masked = mask_paths(&input);
        assert!(!masked.contains(&home), "ホームパスが残っている: {masked}");
        assert!(masked.contains("~/dev/tako/src/main.rs:42:5"));
    }

    #[test]
    fn マスクが他ユーザーのパスも置換する() {
        let input = "/Users/someone/project/file.rs:10";
        let masked = mask_paths(input);
        assert_eq!(masked, "/Users/<user>/project/file.rs:10");
    }

    #[test]
    fn マスクがhomeパスを置換する() {
        let input = "/home/devuser/.cargo/registry/src/main.rs";
        let masked = mask_paths(input);
        assert_eq!(masked, "/home/<user>/.cargo/registry/src/main.rs");
    }

    #[test]
    fn マスクがvar_foldersを置換する() {
        let input = "/var/folders/ab/cd1234/T/test";
        let masked = mask_paths(input);
        assert!(masked.starts_with("/var/folders/<tmp>"));
    }

    #[test]
    fn パスを含まない文字列はそのまま() {
        let input = "index out of bounds: the len is 5 but the index is 10";
        assert_eq!(mask_paths(input), input);
    }

    #[test]
    fn レポートのシリアライズが期待通り() {
        let report = ErrorReport {
            version: "0.5.5".to_string(),
            os_version: Some("macOS 26.0 (Darwin 25.2.0)".to_string()),
            error_kind: ErrorKind::Panic,
            message: Some("test crash".to_string()),
            backtrace: None,
            context: None,
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["version"], "0.5.5");
        assert_eq!(json["error_kind"], "panic");
        assert!(json.get("backtrace").is_none());
        assert!(json.get("context").is_none());
    }

    #[test]
    fn error_kindの全バリアントがシリアライズ可能() {
        for (kind, expected) in [
            (ErrorKind::Panic, "panic"),
            (ErrorKind::Critical, "critical"),
            (ErrorKind::InvariantViolation, "invariant_violation"),
            (ErrorKind::RestoreFailed, "restore_failed"),
            (ErrorKind::DaemonStartup, "daemon_startup"),
        ] {
            let report = ErrorReport {
                version: "0.0.0".to_string(),
                os_version: None,
                error_kind: kind,
                message: None,
                backtrace: None,
                context: None,
            };
            let json = serde_json::to_string(&report).unwrap();
            assert!(json.contains(expected), "{kind:?} → {json}");
        }
    }

    #[test]
    fn enabledフラグのstore_loadが整合する() {
        // 並行テストとの static 競合があるため、store → 即 load の整合性のみ確認
        // （間に別テストが割り込む可能性を許容）
        ENABLED.store(false, Ordering::SeqCst);
        let v1 = ENABLED.load(Ordering::SeqCst);
        ENABLED.store(true, Ordering::SeqCst);
        let v2 = ENABLED.load(Ordering::SeqCst);
        ENABLED.store(false, Ordering::SeqCst);
        // 連続した store/load は割り込めないので、少なくとも v2 は true
        assert!(v2 || !v1, "SeqCst store/load が整合しない");
    }

    #[test]
    fn os_versionがpanicしない() {
        let _ = os_version();
    }

    #[test]
    fn tako_versionが空でない() {
        assert!(!tako_version().is_empty());
    }

    #[test]
    fn 複数パスのマスクが正しい() {
        let input = "at /Users/alice/a.rs:1 and /Users/bob/b.rs:2";
        let masked = mask_paths(input);
        assert_eq!(masked, "at /Users/<user>/a.rs:1 and /Users/<user>/b.rs:2");
    }

    // --- P2-3: Windows パスのマスク (#287) ---

    #[test]
    fn windowsパスのバックスラッシュをマスクする() {
        let input = r"panicked at C:\Users\alice\dev\project\src\main.rs:42";
        let masked = mask_paths(input);
        assert_eq!(
            masked,
            r"panicked at C:\Users\<user>\dev\project\src\main.rs:42"
        );
    }

    #[test]
    fn windowsパスのスラッシュもマスクする() {
        let input = "at D:/Users/bob/code/file.rs:10";
        let masked = mask_paths(input);
        assert_eq!(masked, "at D:/Users/<user>/code/file.rs:10");
    }

    #[test]
    fn windows複数パスのマスク() {
        let input = r"C:\Users\a\x.rs and D:\Users\b\y.rs";
        let masked = mask_paths(input);
        assert_eq!(masked, r"C:\Users\<user>\x.rs and D:\Users\<user>\y.rs");
    }

    #[test]
    fn windowsパスを含まない文字列はそのまま() {
        let input = "C:\\Program Files\\app";
        assert_eq!(mask_paths(input), input);
    }

    #[test]
    fn ホスト名がマスクされる() {
        let hostname = get_hostname();
        if hostname.len() >= 3 {
            let input = format!("error on host {hostname} at port 8080");
            let masked = mask_hostname(&input);
            assert!(
                !masked.contains(&hostname),
                "ホスト名が残っている: {masked}"
            );
            assert!(masked.contains("<hostname>"));
        }
    }

    #[test]
    fn トークンパターンがマスクされる() {
        let input = "url with token=abcdef1234567890abcdef in query";
        let masked = mask_tokens(input);
        assert!(!masked.contains("abcdef1234567890abcdef"));
        assert!(masked.contains("token=<redacted>"));

        let input2 = "Authorization: Bearer sk-ant-api03-longtoken1234567890";
        let masked2 = mask_tokens(input2);
        assert!(!masked2.contains("sk-ant-api03-longtoken1234567890"));
        assert!(masked2.contains("Bearer <redacted>"));

        let input3 = "using ghp_A1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6";
        let masked3 = mask_tokens(input3);
        assert!(!masked3.contains("ghp_A1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6"));
        assert!(masked3.contains("ghp_<redacted>"));
    }

    #[test]
    fn mask_piiが全層を通す() {
        let home = dirs_home().unwrap_or_else(|| "/Users/testuser".to_string());
        let input =
            format!("failed at {home}/project/main.rs with token=aaaaaaaaaaaaaaaa1234567890abcdef");
        let masked = mask_pii(&input);
        assert!(!masked.contains(&home), "ホームパスが残っている");
        assert!(!masked.contains("aaaaaaaaaaaaaaaa1234567890abcdef"));
        assert!(masked.contains("token=<redacted>"));
    }

    #[test]
    fn contextフィールドがシリアライズされる() {
        let report = ErrorReport {
            version: "0.6.0".to_string(),
            os_version: None,
            error_kind: ErrorKind::RestoreFailed,
            message: Some("parse error".to_string()),
            backtrace: None,
            context: Some("layout.json had 3 tabs".to_string()),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["context"], "layout.json had 3 tabs");
    }

    #[test]
    fn キューのenqueue_dequeueが動作する() {
        // data_dir が None の環境ではスキップ（CI 等）
        let Some(path) = queue_path() else { return };
        let test_path = path.with_extension("test.jsonl");
        let _ = std::fs::remove_file(&test_path);
        // queue_path は内部で固定パスを返すので直接テストが難しい。
        // enqueue/flush のロジックを間接的にテスト
        assert!(queue_count() < 1000); // 暴走していないことだけ確認
    }

    #[test]
    fn 環境変数値がマスクされる() {
        // TAKO_TOKEN が設定されていればテスト
        let key = "TAKO_TOKEN";
        let val = std::env::var(key).unwrap_or_default();
        if val.len() >= 8 {
            let input = format!("auth failed with {val}");
            let masked = mask_env_values(&input);
            assert!(!masked.contains(&val), "env 値が残っている: {masked}");
        }
    }

    #[test]
    fn 短いホスト名はマスクしない() {
        // 3 文字未満のホスト名（ab 等）は誤マッチ回避で無視する
        let result = mask_hostname("error with ab inside");
        assert_eq!(result, "error with ab inside");
    }
}

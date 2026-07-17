//! telemetry — エラーレポートの自動収集（Issue #333）
//!
//! tako 内で発生した panic / 重大エラーを PII なしで収集エンドポイントへ送信する。
//! opt-in 既定 off（settings.json の `telemetry` フィールド）。
//! 送信内容はすべて `<data_dir>/telemetry.log` に記録する（透明性）。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_ENDPOINT: &str = "https://tako-error-collector.takushio2525.workers.dev/api/report";

const LOG_MAX_BYTES: u64 = 256 * 1024; // 256KB

static ENABLED: AtomicBool = AtomicBool::new(false);

/// 初期化（起動時に 1 回呼ぶ）
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Panic,
    Critical,
    InvariantViolation,
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Panic => write!(f, "panic"),
            ErrorKind::Critical => write!(f, "critical"),
            ErrorKind::InvariantViolation => write!(f, "invariant_violation"),
        }
    }
}

/// PII を含むパスをマスクする
pub fn mask_paths(input: &str) -> String {
    let mut result = input.to_string();

    // ホームディレクトリをマスク
    if let Some(home) = dirs_home() {
        result = result.replace(&home, "~");
    }

    // /Users/<name> パターンをマスク（他ユーザーのパスも）
    let re_users = regex_lite_replace(&result, r"/Users/[^/\s]+", "/Users/<user>");
    result = re_users;

    // /home/<name> パターン
    let re_home = regex_lite_replace(&result, r"/home/[^/\s]+", "/home/<user>");
    result = re_home;

    // /var/folders 内のユーザー固有パス
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
    // /Users/<name> と /home/<name> と /var/folders/<tmp> の 3 パターンだけなので手書きで十分
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
                // /var/folders/ は 2 セグメント
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
                // 1 セグメント
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

/// OS バージョン文字列を取得（macOS のみ。PII を含まない）
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
        // カーネルバージョンも付加
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

/// tako バージョン（Cargo.toml から）
pub fn tako_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// レポートを送信する（background thread で fire-and-forget）。
/// 送信内容は telemetry.log にも記録する。
pub fn send_report(report: ErrorReport) {
    if !is_enabled() {
        return;
    }
    std::thread::spawn(move || {
        send_report_sync(&report);
    });
}

/// 同期的にレポートを送信する（テスト / panic ハンドラ用）
pub fn send_report_sync(report: &ErrorReport) {
    let json = match serde_json::to_string(report) {
        Ok(j) => j,
        Err(_) => return,
    };

    // ローカルログに記録（透明性）
    log_report(&json);

    let endpoint =
        std::env::var("TAKO_TELEMETRY_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    let result = ureq::post(&endpoint)
        .header("Content-Type", "application/json")
        .send(json.as_bytes());

    match result {
        Ok(_) => {
            log_line("[送信成功]");
        }
        Err(e) => {
            log_line(&format!("[送信失敗] {e}"));
        }
    }
}

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
    // ISO 8601 風のタイムスタンプ（簡易。依存を増やさない）
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

/// panic ハンドラを設定する。既存の hook を chain する。
pub fn install_panic_handler() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 既存 hook を先に呼ぶ（stderr 出力等）
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

        let masked_msg = message.map(|m| mask_paths(&m));
        let masked_bt = mask_paths(&backtrace);
        let masked_loc = location.map(|l| mask_paths(&l));

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
        };

        // panic ハンドラ内では同期送信（プロセスが終わる前に送る）
        send_report_sync(&report);
    }));
}

/// telemetry.log の直近エントリ数を返す（status 表示用）
pub fn recent_count() -> usize {
    let Some(path) = log_path() else {
        return 0;
    };
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter(|l| l.contains(r#""error_kind""#)).count())
        .unwrap_or(0)
}

/// telemetry.log のパスを返す
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
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["version"], "0.5.5");
        assert_eq!(json["error_kind"], "panic");
        assert!(json.get("backtrace").is_none());
    }

    #[test]
    fn error_kindの全バリアントがシリアライズ可能() {
        for (kind, expected) in [
            (ErrorKind::Panic, "panic"),
            (ErrorKind::Critical, "critical"),
            (ErrorKind::InvariantViolation, "invariant_violation"),
        ] {
            let report = ErrorReport {
                version: "0.0.0".to_string(),
                os_version: None,
                error_kind: kind,
                message: None,
                backtrace: None,
            };
            let json = serde_json::to_string(&report).unwrap();
            assert!(json.contains(expected), "{kind:?} → {json}");
        }
    }

    #[test]
    fn enabledフラグの初期値はfalse() {
        // テスト間の状態汚染を避けるためリセット
        ENABLED.store(false, Ordering::Relaxed);
        assert!(!is_enabled());
    }

    #[test]
    fn set_enabledが反映される() {
        set_enabled(true);
        assert!(is_enabled());
        set_enabled(false);
        assert!(!is_enabled());
    }

    #[test]
    fn os_versionがpanicしない() {
        // 環境依存だが panic しないことを確認
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
}

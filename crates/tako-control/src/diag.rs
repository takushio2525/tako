//! diag — 永続化まわりの診断ログ（Issue #30）
//!
//! Dock 起動の .app は stderr がユーザーから見えないため、レイアウト永続化
//! （保存・復元・明示削除）の結果を `<data_dir>/persist.log` に残す。
//! 「再起動したらタブが全部消えた」ときに、保存されなかったのか・復元に
//! 失敗したのか・明示削除されたのかを後から特定できるようにする。
//!
//! - 書き込みは best-effort（ログ失敗で本体機能を止めない）
//! - 肥大化防止: 上限超過で `.old` へローテート（世代は 1 つ）
//! - ペイン内容・送信テキスト・トークンは書かない（規約）

use std::io::Write;
use std::path::PathBuf;

/// ローテート閾値（これを超えたら `.old` へ退避して新しいファイルを始める）
const ROTATE_BYTES: u64 = 256 * 1024;

/// 診断ログのパス（`<data_dir>/persist.log`）
pub fn persist_log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("persist.log"))
}

/// パフォーマンス診断ログのパス（`<data_dir>/perf.log`。Issue #113）
pub fn perf_log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("perf.log"))
}

/// 1 行追記する（UTC タイムスタンプ付き）。失敗は握りつぶす（診断ログの失敗で
/// 本体を巻き込まない）。呼び出し頻度は起動・終了・エラー時のみを想定し、
/// 定期保存の成功のような高頻度イベントは書かないこと
pub fn persist_log(msg: &str) {
    append_log(persist_log_path(), msg);
}

/// UI ストール・dispatch 遅延など性能異常の記録（Issue #113: 多ペイン時の無応答の
/// 犯人特定用）。**しきい値超えのときだけ**呼ぶこと（正常時は何も書かない = 高頻度
/// 呼び出し禁止は persist_log と同じ方針）。セルフテスト中はユーザーのログを汚さない
pub fn perf_log(msg: &str) {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return;
    }
    append_log(perf_log_path(), msg);
}

fn append_log(path: Option<PathBuf>, msg: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // ローテート: 上限超過なら .old へ退避（rename 失敗は無視して追記継続）
    if std::fs::metadata(&path)
        .map(|m| m.len() > ROTATE_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{}] {msg}", format_utc(now));
    }
}

/// unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`。外部クレート（chrono 等）を増やさないための
/// 自前変換（civil_from_days アルゴリズム。うるう年対応・グレゴリオ暦）
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3_600,
        (tod % 3_600) / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utcタイムスタンプが正しく変換される() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        // 2026-07-02 00:00:00 UTC = 1_782_950_400
        assert_eq!(format_utc(1_782_950_400), "2026-07-02T00:00:00Z");
        // うるう日 2024-02-29 12:34:56 UTC = 1_709_210_096
        assert_eq!(format_utc(1_709_210_096), "2024-02-29T12:34:56Z");
        // 年末境界 2023-12-31 23:59:59 UTC = 1_704_067_199
        assert_eq!(format_utc(1_704_067_199), "2023-12-31T23:59:59Z");
    }
}

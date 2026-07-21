//! アプリ内更新（#36 / #358）バナー・確認・進行表示の文言（キー: update.*）
//!
//! CLI / MCP `tako update` と共有されるエラーメッセージ（brew 実行結果等）は
//! 技術情報のため対象外（現状維持）。ここは GUI ステータスバーに出る文言のみ

pub fn banner_both() -> &'static str {
    tr!(
        "更新あり（安定版 + テスト版）",
        "Updates available (stable + test)"
    )
}
pub fn banner_stable(ver: &str) -> String {
    tr!(
        format!("v{ver} (安定版) が利用可能"),
        format!("v{ver} (stable) available")
    )
}
pub fn banner_test(ver: &str) -> String {
    tr!(
        format!("v{ver} (test) が利用可能"),
        format!("v{ver} (test) available")
    )
}
pub fn test_warning(ver: &str) -> String {
    tr!(
        format!("v{ver} はテスト版です（不安定な可能性があります）。更新しますか？"),
        format!("v{ver} is a test build (may be unstable). Update?")
    )
}
pub fn cont() -> &'static str {
    tr!("続行", "Continue")
}
pub fn confirm(ver: &str, channel: &str, method: &str) -> String {
    tr!(
        format!(
            "v{ver} ({channel}) に更新して再起動しますか？（{method}。実行中のプロセスは失われます）"
        ),
        format!(
            "Update to v{ver} ({channel}) and restart? ({method}; running processes will be lost)"
        )
    )
}
pub fn run() -> &'static str {
    tr!("実行", "Update")
}
pub fn method_zip() -> &'static str {
    tr!("ZIP 差し替え", "ZIP replacement")
}
pub fn method_zip_broken() -> &'static str {
    tr!("zip (brew 破損)", "zip (broken brew)")
}
pub fn brew_failed(err: &str) -> String {
    tr!(
        format!("brew 更新失敗: {err}"),
        format!("brew update failed: {err}")
    )
}
pub fn update_via_zip() -> &'static str {
    tr!("zip で更新", "Update via zip")
}
pub fn updating() -> &'static str {
    tr!("更新中...", "Updating...")
}
pub fn updating_zip_fallback() -> &'static str {
    tr!(
        "zip フォールバックで更新中...",
        "Updating via zip fallback..."
    )
}
pub fn restarting(msg: &str) -> String {
    tr!(
        format!("{msg} — 再起動中..."),
        format!("{msg} — restarting...")
    )
}
pub fn restart_failed(e: &str) -> String {
    tr!(
        format!("更新は完了しましたが再起動に失敗: {e}"),
        format!("Update finished but restart failed: {e}")
    )
}
pub fn current_line(ver: &str, channel: &str, method: &str) -> String {
    tr!(
        format!("現在: v{ver} ({channel}) / {method}"),
        format!("Current: v{ver} ({channel}) / {method}")
    )
}
pub fn latest() -> &'static str {
    tr!("最新版です", "Up to date")
}
pub fn no_test_build() -> &'static str {
    tr!("テスト版なし", "No test build")
}
pub fn channel_stable() -> &'static str {
    tr!("安定版", "stable")
}
pub fn channel_test() -> &'static str {
    tr!("テスト版", "test")
}
pub fn eta_minutes(minutes: u64) -> String {
    tr!(format!("約{minutes}分後"), format!("in ~{minutes} min"))
}
pub fn eta_seconds(secs: u64) -> String {
    tr!(format!("約{secs}秒後"), format!("in ~{secs}s"))
}
pub fn eta_soon() -> &'static str {
    tr!("まもなく", "soon")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                banner_both().to_string(),
                banner_stable("1.0.0"),
                banner_test("1.0.1"),
                test_warning("1.0.1"),
                cont().to_string(),
                confirm("1.0.0", "stable", "Homebrew"),
                run().to_string(),
                method_zip().to_string(),
                method_zip_broken().to_string(),
                brew_failed("timeout"),
                update_via_zip().to_string(),
                updating().to_string(),
                updating_zip_fallback().to_string(),
                restarting("done"),
                restart_failed("spawn error"),
                current_line("0.6.0", "stable", "Homebrew"),
                latest().to_string(),
                no_test_build().to_string(),
                channel_stable().to_string(),
                channel_test().to_string(),
                eta_minutes(5),
                eta_seconds(30),
                eta_soon().to_string(),
            ]
        });
    }
}

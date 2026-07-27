//! アプリ内更新（#36 / #358 / #616）の通知カード・専用画面の文言（キー: update.*）
//!
//! CLI / MCP `tako update` と共有されるエラーメッセージ（brew 実行結果等）は
//! 技術情報のため対象外（現状維持）。ここは GUI に出る文言のみ。
//!
//! #616 で表示先がステータスバー → 「上部通知カード + 専用ウィンドウ」へ移った。
//! `banner_*` は案内の 1 行サマリとしてカード・専用画面の双方で使い続けている

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
pub fn latest() -> &'static str {
    tr!("最新版です", "Up to date")
}
/// 手動チェック中の表示（#485。tako メニュー / About の「アップデートを確認」）
pub fn checking() -> &'static str {
    tr!("アップデートを確認中...", "Checking for updates...")
}
/// 手動チェックの結果「更新なし」（#485）
pub fn up_to_date(ver: &str) -> String {
    tr!(
        format!("最新版です (v{ver})"),
        format!("Up to date (v{ver})")
    )
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

// --- #616: 上部通知カード ---

pub fn card_title() -> &'static str {
    tr!("アップデートがあります", "An update is available")
}
/// カードの主ボタン（専用画面へ飛ぶ）
pub fn card_details() -> &'static str {
    tr!("詳細を見る", "View details")
}
/// × の意味を明示する短い添え書き（黙って消えると出し直し方が分からない。#549 と同方針）
pub fn card_dismiss_hint() -> &'static str {
    tr!(
        "このバージョンは通知しない",
        "Stop notifying for this version"
    )
}

// --- #616: アップデート専用ウィンドウ ---

pub fn window_title() -> &'static str {
    tr!("tako のアップデート", "tako Update")
}
pub fn section_current() -> &'static str {
    tr!("現在のバージョン", "Current version")
}
pub fn section_available() -> &'static str {
    tr!("利用できるアップデート", "Available updates")
}
pub fn section_notes() -> &'static str {
    tr!("リリースノート", "Release notes")
}
pub fn label_version() -> &'static str {
    tr!("バージョン", "Version")
}
pub fn label_channel() -> &'static str {
    tr!("チャンネル", "Channel")
}
pub fn label_install_method() -> &'static str {
    tr!("配布系統", "Install method")
}
/// #595: どの配布物を掴むか（「更新が出ない」の診断に効く）
pub fn label_asset() -> &'static str {
    tr!("配布物", "Asset")
}
pub fn label_platform() -> &'static str {
    tr!("実行環境", "Environment")
}
pub fn check_button() -> &'static str {
    tr!("アップデートを確認", "Check for updates")
}
pub fn update_now() -> &'static str {
    tr!("今すぐ更新", "Update now")
}
pub fn open_release_page() -> &'static str {
    tr!("リリースページを開く", "Open release page")
}
pub fn no_updates() -> &'static str {
    tr!(
        "新しいバージョンはありません（最新版を使っています）",
        "No newer version (you are up to date)"
    )
}
pub fn not_checked_yet() -> &'static str {
    tr!(
        "まだ確認していません。「アップデートを確認」を押してください",
        "Not checked yet. Press \"Check for updates\""
    )
}
pub fn no_notes() -> &'static str {
    tr!("リリースノートはありません", "No release notes")
}
/// 配布系統の表示名（zip / broken-brew は既存の method_zip* と一貫させる）
pub fn install_method_display(method: &str) -> String {
    match method {
        "homebrew" => "Homebrew".to_string(),
        "broken-brew" => method_zip_broken().to_string(),
        _ => method_zip().to_string(),
    }
}
pub fn repair_button() -> &'static str {
    tr!("brew の登録を修復", "Repair brew registration")
}
pub fn broken_brew_note() -> &'static str {
    tr!(
        "brew の台帳と .app の実体が食い違っています。修復するか zip で更新してください",
        "The brew ledger and the installed .app disagree. Repair it, or update via zip"
    )
}
/// 更新は再起動を伴う（実行中プロセスが失われる）ことの常設の注意書き
pub fn restart_warning() -> &'static str {
    tr!(
        "更新すると tako が再起動します（実行中のプロセスは失われます）",
        "Updating restarts tako (running processes will be lost)"
    )
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
                latest().to_string(),
                checking().to_string(),
                up_to_date("0.6.0"),
                no_test_build().to_string(),
                channel_stable().to_string(),
                channel_test().to_string(),
                eta_minutes(5),
                eta_seconds(30),
                eta_soon().to_string(),
                // #616: 通知カード + 専用ウィンドウ
                card_title().to_string(),
                card_details().to_string(),
                card_dismiss_hint().to_string(),
                window_title().to_string(),
                section_current().to_string(),
                section_available().to_string(),
                section_notes().to_string(),
                label_version().to_string(),
                label_channel().to_string(),
                label_install_method().to_string(),
                label_asset().to_string(),
                label_platform().to_string(),
                check_button().to_string(),
                update_now().to_string(),
                open_release_page().to_string(),
                no_updates().to_string(),
                not_checked_yet().to_string(),
                no_notes().to_string(),
                repair_button().to_string(),
                broken_brew_note().to_string(),
                restart_warning().to_string(),
            ]
        });
    }

    /// 配布系統の表示名は 3 系統すべてに解があり、未知の値でも空にならない（#616）
    #[test]
    fn install_method_display_covers_all_kinds() {
        for m in ["homebrew", "zip", "broken-brew", "unknown"] {
            assert!(
                !install_method_display(m).is_empty(),
                "{m} の表示名が空になっている"
            );
        }
        assert_eq!(install_method_display("homebrew"), "Homebrew");
        assert_eq!(install_method_display("broken-brew"), method_zip_broken());
        // 未知の値は zip 扱い（表示が消えるより「zip です」と言い切るほうが安全）
        assert_eq!(install_method_display("unknown"), method_zip());
    }
}

//! About ウィンドウの文言（キー: about.*。Issue #485）

/// プロダクト名（言語非依存）
pub const PRODUCT: &str = "tako";
/// ライセンス識別子（SPDX。言語非依存）
pub const LICENSE: &str = "GPL-3.0-or-later";

pub fn window_title() -> &'static str {
    tr!("tako について", "About tako")
}
pub fn tagline() -> &'static str {
    tr!(
        "AI エージェントのための GUI ターミナル",
        "A GUI terminal built for AI agents"
    )
}
pub fn version(v: &str) -> String {
    tr!(format!("バージョン {v}"), format!("Version {v}"))
}
pub fn install_method(method: &str) -> String {
    tr!(
        format!("インストール方法: {method}"),
        format!("Installed via: {method}")
    )
}
pub fn license_line(id: &str) -> String {
    tr!(format!("ライセンス: {id}"), format!("License: {id}"))
}
pub fn check_updates() -> &'static str {
    tr!("アップデートを確認", "Check for Updates")
}
pub fn copy_info() -> &'static str {
    tr!("情報をコピー", "Copy Info")
}
pub fn copied() -> &'static str {
    tr!("コピーしました", "Copied")
}
pub fn repository() -> &'static str {
    tr!("リポジトリ", "Repository")
}
pub fn documentation() -> &'static str {
    tr!("ドキュメント", "Documentation")
}
pub fn releases() -> &'static str {
    tr!("リリースノート", "Release Notes")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                window_title().to_string(),
                tagline().to_string(),
                version("0.0.0"),
                install_method("homebrew"),
                license_line(LICENSE),
                check_updates().to_string(),
                copy_info().to_string(),
                copied().to_string(),
                repository().to_string(),
                documentation().to_string(),
                releases().to_string(),
            ]
        });
    }
}

//! プレビューペイン（コード / Markdown / PDF / 画像 / 動画 / 履歴）の文言（キー: preview.*）
//!
//! dispatch / CLI へ返すエラー文字列は対象外（技術情報のため現状維持）。
//! ここはプレビュー画面に描画される文言のみ

// --- ヘッダー・トグル（キー: preview.header_*） ---

pub fn view_as_code() -> &'static str {
    tr!("コードとして表示", "View as code")
}
pub fn view_as_markdown() -> &'static str {
    tr!("md レンダリング表示", "Render as Markdown")
}
pub fn history() -> &'static str {
    tr!("履歴", "History")
}
pub fn editing() -> &'static str {
    tr!("編集中", "Editing")
}
pub fn edit() -> &'static str {
    tr!("編集", "Edit")
}
pub fn save_cmd_s() -> &'static str {
    tr!("保存 ⌘S", "Save ⌘S")
}
pub fn outline_button() -> &'static str {
    tr!("目次", "Outline")
}

// --- 編集ステータス（キー: preview.status_*） ---

pub fn saved_suffix() -> &'static str {
    tr!(" \u{00B7} 保存済", " \u{00B7} saved")
}
pub fn conflict_suffix() -> &'static str {
    tr!(" \u{00B7} 競合", " \u{00B7} conflict")
}
pub fn error_suffix() -> &'static str {
    tr!(" \u{00B7} エラー", " \u{00B7} error")
}
pub fn saved_message() -> &'static str {
    tr!("保存しました", "Saved")
}

// --- 目次ポップオーバー（#232。キー: preview.outline_*） ---

pub fn outline_section() -> &'static str {
    tr!("アウトライン", "Outline")
}
pub fn goto_page_section() -> &'static str {
    tr!("ページへ移動", "Go to page")
}
pub fn page_n(page: usize) -> String {
    tr!(format!("ページ {page}"), format!("Page {page}"))
}
pub fn item_count(n: usize) -> String {
    tr!(format!("{n} 件"), format!("{n} items"))
}

// --- 本文・状態表示（キー: preview.body_*） ---

pub fn loading() -> &'static str {
    tr!("読み込み中…", "Loading…")
}
pub fn tail_omitted() -> &'static str {
    tr!(
        "…（大きいファイルのため末尾を省略して表示）",
        "… (tail omitted for large file)"
    )
}
pub fn video_play() -> &'static str {
    tr!("\u{25b6}\u{fe0e} 再生", "\u{25b6}\u{fe0e} Play")
}
pub fn video_resolution(w: u32, h: u32) -> String {
    tr!(
        format!("解像度: {w} x {h}"),
        format!("Resolution: {w} x {h}")
    )
}
pub fn video_duration(mins: u64, secs: u64) -> String {
    tr!(
        format!("長さ: {mins}:{secs:02}"),
        format!("Duration: {mins}:{secs:02}")
    )
}
pub fn video_codec(codec: &str) -> String {
    tr!(format!("コーデック: {codec}"), format!("Codec: {codec}"))
}
pub fn video_size_mb(mb: f64) -> String {
    tr!(format!("サイズ: {mb:.1} MB"), format!("Size: {mb:.1} MB"))
}
pub fn video_size_kb(kb: f64) -> String {
    tr!(format!("サイズ: {kb:.0} KB"), format!("Size: {kb:.0} KB"))
}

// --- 履歴（チェンジログ）ビュー（#338。キー: preview.changelog_*） ---

pub fn not_in_git() -> &'static str {
    tr!(
        "git 管理外のファイルです",
        "This file is not tracked by git"
    )
}
pub fn no_history() -> &'static str {
    tr!(
        "このファイルの変更履歴はありません",
        "No change history for this file"
    )
}
pub fn no_diff() -> &'static str {
    tr!("(変更なし)", "(no changes)")
}

// --- プレビューエラー表示（キー: preview.error_*） ---

pub fn unsupported_image() -> &'static str {
    tr!("対応していない画像形式", "Unsupported image format")
}
pub fn image_too_large(mb: f64) -> String {
    tr!(
        format!("画像が大きすぎる（{mb:.1} MB、上限 50 MB）"),
        format!("Image too large ({mb:.1} MB, limit 50 MB)")
    )
}
pub fn cannot_read(e: &str) -> String {
    tr!(format!("読み込めない: {e}"), format!("Cannot read: {e}"))
}
/// 使用箇所は #[cfg(not(target_os = "macos"))] のみ（macOS ビルドでは未使用が正常）
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn pdf_macos_only() -> &'static str {
    tr!(
        "PDF プレビューは macOS のみ対応",
        "PDF preview is only supported on macOS"
    )
}
pub fn binary_file() -> &'static str {
    tr!(
        "バイナリファイル（テキストとして表示できない）",
        "Binary file (cannot display as text)"
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
                view_as_code().to_string(),
                view_as_markdown().to_string(),
                history().to_string(),
                editing().to_string(),
                edit().to_string(),
                save_cmd_s().to_string(),
                outline_button().to_string(),
                saved_suffix().to_string(),
                conflict_suffix().to_string(),
                error_suffix().to_string(),
                saved_message().to_string(),
                outline_section().to_string(),
                goto_page_section().to_string(),
                page_n(3),
                item_count(12),
                loading().to_string(),
                tail_omitted().to_string(),
                video_play().to_string(),
                video_resolution(1920, 1080),
                video_duration(3, 5),
                video_codec("h264"),
                video_size_mb(1.5),
                video_size_kb(200.0),
                not_in_git().to_string(),
                no_history().to_string(),
                no_diff().to_string(),
                unsupported_image().to_string(),
                image_too_large(60.0),
                cannot_read("io error"),
                pdf_macos_only().to_string(),
                binary_file().to_string(),
            ]
        });
    }
}

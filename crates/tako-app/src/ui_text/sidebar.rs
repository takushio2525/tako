//! 左サイドバー（ファイルツリー）の文言（キー: sidebar.*）

use tako_control::platform::os_integration::FileManager;

// --- コンテキストメニュー（FR-3.12 / #314。キー: sidebar.menu_*） ---

pub fn menu_copy_rel() -> &'static str {
    tr!("相対パスをコピー", "Copy relative path")
}
pub fn menu_copy_abs() -> &'static str {
    tr!("絶対パスをコピー", "Copy absolute path")
}
/// ファイルマネージャの呼び名は OS ごとに違う（#617）。
/// 「Finder で表示」を Windows で出すと、押しても何も起きないうえに何を指すか伝わらない。
/// 判定は境界 B8（`os_integration::file_manager`）の 1 か所で、ここは値を受け取るだけ
pub fn menu_reveal(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("Finder で表示", "Reveal in Finder"),
        FileManager::Explorer => tr!("エクスプローラーで表示", "Reveal in Explorer"),
    }
}
pub fn menu_open_term() -> &'static str {
    tr!("ターミナルで開く", "Open in terminal")
}
pub fn menu_open_default() -> &'static str {
    tr!("デフォルトアプリで開く", "Open with default app")
}
pub fn menu_open_with() -> &'static str {
    tr!("このアプリで開く...", "Open with...")
}
/// ターミナル内のパスを cmd+クリックして開けなかったときの理由（#1283）。
///
/// この経路は以前 `eprintln!("warning: パスを開けない")` だけで、GUI の stderr は
/// 誰も読めないので**押しても無言**だった。サイドバーの通知（`remote_notice` =
/// リモート専用ではなく共有の通知欄）へ出して読める形にする
pub fn notice_open_failed(path: &str, reason: &str) -> String {
    tr!(
        format!("パスを開けません（{path}）: {reason}"),
        format!("Cannot open path ({path}): {reason}")
    )
}
/// ファイルツリーの**ローカル行**の操作が失敗したときの理由（#1399）。
///
/// この経路は以前 `let _ = dispatch(..)` / `if result.is_ok()` / `eprintln!` で
/// 結果を捨てていたので、**ごみ箱移動やリネームが失敗しても画面が無反応**だった
/// （同じファイルのリモート行は #919 から通知欄へ出していた）。`op` はユーザーが
/// 実際に押したメニュー項目の文言（`menu_*` / `hidden_*` の戻り値）をそのまま渡す =
/// 「押したもの」と「失敗したもの」の名前が必ず一致する。
/// `target` が `None` なのは対象パスを持たない操作（隠しファイル表示トグル）
pub fn notice_op_failed(op: &str, target: Option<&str>, reason: &str) -> String {
    match target {
        Some(t) => tr!(
            format!("{op} に失敗しました（{t}）: {reason}"),
            format!("{op} failed ({t}): {reason}")
        ),
        None => tr!(
            format!("{op} に失敗しました: {reason}"),
            format!("{op} failed: {reason}")
        ),
    }
}
/// Finder の「このアプリケーションで開く」・`tako open` で渡されたパスを
/// 開けなかったときの**操作名**（#1432）。
///
/// この入口は以前 `eprintln!` 止まりで、消えたファイル・別ボリュームのパスを
/// 渡すと **tako が前面に出ることすらなく無反応**だった
pub fn op_open_path() -> &'static str {
    tr!("パスを開く", "Open path")
}
/// 渡されたパスが実在しないときの理由（#1432。ファイルでもフォルダでもない）
pub fn reason_path_missing() -> &'static str {
    tr!("見つかりません", "Not found")
}
pub fn menu_rename() -> &'static str {
    tr!("名前変更", "Rename")
}
pub fn menu_new_file() -> &'static str {
    tr!("新しいファイル", "New file")
}
pub fn menu_new_dir() -> &'static str {
    tr!("新しいフォルダ", "New folder")
}
/// ごみ箱の呼び名も OS ごとに違う（ファイルマネージャと同じ「OS のシェル」の軸なので
/// 同じ値で決める）。**この操作が復元可能であること**をラベルで約束しているので、
/// 実装（B8 の `move_to_trash`）と表記を必ず揃える（#617）
pub fn menu_trash(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("削除", "Move to Trash"),
        FileManager::Explorer => tr!("ごみ箱に移動", "Move to Recycle Bin"),
    }
}
pub fn menu_remove_root() -> &'static str {
    tr!("ツリーから除去", "Remove from tree")
}

// --- ヘッダのトグル（#550。キー: sidebar.hidden_*） ---

pub fn hidden_show() -> &'static str {
    tr!("隠しファイルを表示", "Show hidden files")
}
pub fn hidden_hide() -> &'static str {
    tr!("隠しファイルを隠す", "Hide hidden files")
}

// --- 新規作成のインライン入力（#559。キー: sidebar.new_*） ---

pub fn new_file_placeholder() -> &'static str {
    tr!("ファイル名", "File name")
}
pub fn new_dir_placeholder() -> &'static str {
    tr!("フォルダ名", "Folder name")
}
pub fn rename_placeholder() -> &'static str {
    tr!("新しい名前", "New name")
}

// --- プレビュー編集の通知（FR-3.5。キー: sidebar.note_*） ---

pub fn note_save_before_mode_switch() -> &'static str {
    tr!(
        "未保存の変更を保存してから表示モードを切り替えてください",
        "Save your changes before switching the view mode"
    )
}
pub fn note_external_change() -> &'static str {
    tr!(
        "外部変更を検知しました。編集中の内容を保持し、自動更新は行いません",
        "External changes detected. Your edits are kept; auto-reload is paused"
    )
}

/// ツリーの展開をシンボリックリンクの循環で打ち切ったときの説明行（#1398）。
///
/// #1398 でリンクを辿るようになり、`a/link -> a` のように**同じ実体へ戻る展開**が
/// 起こり得るようになった。打ち切りを黙って行うと「押しても何も出ない」= #1398 で
/// 直した症状に戻るので、行として理由を出す。差し込む `real` は言語非依存のパス
pub fn note_symlink_loop(real: &str) -> String {
    tr!(
        format!("シンボリックリンクの循環のため展開を打ち切りました（同じ場所: {real}）"),
        format!("Stopped expanding: symlink loops back to {real}")
    )
}

/// 1 ディレクトリの表示を上限で切り詰めたときの説明行（#1402）。
///
/// 上限（`filetree::MAX_ENTRIES`）を超えたぶんを黙って捨てると、ユーザーからは
/// 「そのファイルが存在しない」ように見える（機械可読側の `tree git-status` は
/// `truncated` を返していたので、画面だけが黙っていた）。**総数まで出す**のが要点:
/// 「何件隠れているか」が分からないと、ツリーで探すのを諦める判断ができない
pub fn note_truncated(shown: usize, total: usize) -> String {
    tr!(
        format!("{shown} 件まで表示（全 {total} 件）"),
        format!("Showing {shown} of {total} entries")
    )
}

/// ディレクトリを読めなかったときの説明行（#1402）。
///
/// `read_dir` の失敗（権限なし・消滅）を空 Vec へ落としていたので、**権限の無い
/// フォルダと空のフォルダが同じ見え方**だった。`reason` は OS が返した理由をそのまま
/// 渡す（言語非依存の詳細。リモート行が生の詳細を出すのと同じ作法 = #919）
pub fn note_read_failed(reason: &str) -> String {
    tr!(
        format!("読み込めませんでした: {reason}"),
        format!("Cannot read this folder: {reason}")
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;
    use tako_core::i18n::Lang;

    /// ローカル操作の失敗の文言が**押した項目の名前を先頭に置いて日英で出る**（#1399）。
    ///
    /// `op` はメニューの文言そのもの（`menu_*` の戻り値）を渡す約束なので、
    /// 「押したもの」と「失敗したもの」が必ず一致する。対象を持たない操作
    /// （隠しファイルトグル）では括弧ごと落ちることも固定する
    #[test]
    fn ローカル操作の失敗文言が日英で出る() {
        tests_support::with_lang(Lang::Ja, || {
            assert_eq!(
                notice_op_failed(
                    menu_trash(FileManager::Finder),
                    Some("/tmp/x"),
                    "パスが存在しない"
                ),
                "削除 に失敗しました（/tmp/x）: パスが存在しない"
            );
            assert_eq!(
                notice_op_failed(hidden_show(), None, "無効なパラメータ"),
                "隠しファイルを表示 に失敗しました: 無効なパラメータ"
            );
        });
        tests_support::with_lang(Lang::En, || {
            assert_eq!(
                notice_op_failed(
                    menu_trash(FileManager::Explorer),
                    Some("/tmp/x"),
                    "no such path"
                ),
                "Move to Recycle Bin failed (/tmp/x): no such path"
            );
            assert_eq!(
                notice_op_failed(hidden_hide(), None, "invalid params"),
                "Hide hidden files failed: invalid params"
            );
            assert_eq!(
                notice_open_failed("/tmp/x", "not a file"),
                "Cannot open path (/tmp/x): not a file"
            );
        });
        // 対象の有無で形が変わる（同じ文字列を 2 回返す形へ退化していない）
        tests_support::for_each_lang(|| {
            assert_ne!(
                notice_op_failed("Op", Some("/tmp/x"), "why"),
                notice_op_failed("Op", None, "why"),
                "target の有無が文言に出ていない"
            );
        });
    }

    /// 切り詰めの説明は**見せた件数と総数の両方**を出す（#1402）。
    ///
    /// 「一部だけ表示しています」のような件数の無い文言だと、探すのを諦める判断が
    /// できない（総数が分かって初めて「ターミナルで見る」へ切り替えられる）
    #[test]
    fn 切り詰めと読み取り失敗の文言が日英で出る() {
        tests_support::with_lang(Lang::Ja, || {
            assert_eq!(note_truncated(500, 560), "500 件まで表示（全 560 件）");
            assert_eq!(
                note_read_failed("Permission denied (os error 13)"),
                "読み込めませんでした: Permission denied (os error 13)"
            );
        });
        tests_support::with_lang(Lang::En, || {
            assert_eq!(note_truncated(500, 560), "Showing 500 of 560 entries");
            assert_eq!(
                note_read_failed("Permission denied (os error 13)"),
                "Cannot read this folder: Permission denied (os error 13)"
            );
        });
        // 件数が両方とも文言に出ている（片方だけ出す形へ退化していない）
        tests_support::for_each_lang(|| {
            let text = note_truncated(500, 560);
            assert!(text.contains("500") && text.contains("560"), "{text}");
            assert_ne!(
                note_truncated(500, 560),
                note_truncated(500, 999),
                "総数が文言に出ていない"
            );
        });
    }

    /// ファイルマネージャ / ごみ箱の呼び名が **OS ごとに実際に分かれている**（#617）。
    ///
    /// `cfg!(windows)` で分岐していた頃は macOS 側の CI で Windows の文言を 1 文字も
    /// 検査できなかった。`FileManager` を値で受け取る形にしたので、**macOS 上から
    /// Windows 側の表記も固定できる**（`settings_sleep::Device` と同じ作法。#905）。
    /// 「Windows で押しても何も起きない Finder ラベル」の再発をここで止める
    #[test]
    fn ファイルマネージャの呼び名がosごとに分かれている() {
        use super::super::{pane_menu, settings};
        tests_support::with_lang(Lang::Ja, || {
            assert_eq!(menu_reveal(FileManager::Finder), "Finder で表示");
            assert_eq!(menu_reveal(FileManager::Explorer), "エクスプローラーで表示");
            assert_eq!(menu_trash(FileManager::Finder), "削除");
            assert_eq!(menu_trash(FileManager::Explorer), "ごみ箱に移動");
            assert_eq!(
                pane_menu::reveal_cwd(FileManager::Explorer),
                "エクスプローラーで開く"
            );
            assert_eq!(
                settings::advanced_reveal(FileManager::Explorer),
                "エクスプローラーで表示"
            );
        });
        tests_support::with_lang(Lang::En, || {
            assert_eq!(menu_reveal(FileManager::Finder), "Reveal in Finder");
            assert_eq!(menu_reveal(FileManager::Explorer), "Reveal in Explorer");
            assert_eq!(menu_trash(FileManager::Finder), "Move to Trash");
            assert_eq!(menu_trash(FileManager::Explorer), "Move to Recycle Bin");
        });
        // 出し分けが「同じ文字列を 2 回返す」形へ退化していないこと（両言語で）
        tests_support::for_each_lang(|| {
            for (finder, explorer) in [
                (
                    menu_reveal(FileManager::Finder),
                    menu_reveal(FileManager::Explorer),
                ),
                (
                    menu_trash(FileManager::Finder),
                    menu_trash(FileManager::Explorer),
                ),
                (
                    pane_menu::reveal(FileManager::Finder),
                    pane_menu::reveal(FileManager::Explorer),
                ),
                (
                    pane_menu::reveal_cwd(FileManager::Finder),
                    pane_menu::reveal_cwd(FileManager::Explorer),
                ),
                (
                    settings::advanced_reveal(FileManager::Finder),
                    settings::advanced_reveal(FileManager::Explorer),
                ),
            ] {
                assert_ne!(finder, explorer, "OS 別の出し分けが効いていない");
            }
        });
    }

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                menu_copy_rel().to_string(),
                op_open_path().to_string(),
                reason_path_missing().to_string(),
                menu_copy_abs().to_string(),
                menu_reveal(FileManager::Finder).to_string(),
                menu_reveal(FileManager::Explorer).to_string(),
                menu_open_term().to_string(),
                menu_open_default().to_string(),
                menu_open_with().to_string(),
                // #1283 / #1399: 失敗の理由（cmd+クリック / ツリーのローカル操作）。
                // 差し込む値は**言語非依存の placeholder** にする（`op` / `reason` は
                // 呼び出し側が渡す文字列で、英語側に日本語が残っていないかの検査対象外）
                notice_open_failed("/tmp/x", "no such path"),
                notice_op_failed("Trash", Some("/tmp/x"), "no such path"),
                notice_op_failed("Toggle", None, "no such path"),
                menu_rename().to_string(),
                menu_new_file().to_string(),
                menu_new_dir().to_string(),
                menu_trash(FileManager::Finder).to_string(),
                menu_trash(FileManager::Explorer).to_string(),
                menu_remove_root().to_string(),
                hidden_show().to_string(),
                hidden_hide().to_string(),
                new_file_placeholder().to_string(),
                new_dir_placeholder().to_string(),
                rename_placeholder().to_string(),
                note_save_before_mode_switch().to_string(),
                note_external_change().to_string(),
                // #1398: 打ち切りの説明（差し込むパスは言語非依存）
                note_symlink_loop("/tmp/x"),
                // #1402: 切り詰め・読み取り失敗の説明（件数と OS の理由は言語非依存）
                note_truncated(500, 560),
                note_read_failed("Permission denied (os error 13)"),
            ]
        });
    }
}

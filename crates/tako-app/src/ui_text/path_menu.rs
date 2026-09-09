//! ターミナル内のパスリンクの cmd+右クリックメニューの項目名（キー: path_menu.*。#1182）
//!
//! **共通の項目はファイルツリー（`sidebar::menu_*`）をそのまま引く**。同じ操作に
//! 別の言い回しを与えると、ユーザーは「別のことが起きる」と読む（#617 の
//! 「表記と実装を揃える」と同じ理由）。ここで新しく定義するのは、ツリーには無い
//! 「tako で開く」だけ。

use tako_control::platform::os_integration::FileManager;

/// tako の中で開く（= cmd+クリックと同じ動作）。
///
/// **ファイルとディレクトリで文言を変える**。同じ「tako で開く」でも実際に起きることが
/// プレビューとターミナルで別物なので、押す前に分かるようにしておく
/// （ユーザーの要望の出発点が「ディレクトリだとターミナルが出る」だった。#1182）
pub fn open_in_tako_file() -> &'static str {
    tr!("プレビューで開く", "Open in preview")
}
pub fn open_in_tako_dir() -> &'static str {
    tr!("ターミナルで開く", "Open in terminal")
}

/// 以下はツリー（#314）と同一の文言を引くだけの委譲。
/// **ここで文字列を書き直さない**（表記が 2 つに割れる）
pub fn open_default() -> &'static str {
    super::sidebar::menu_open_default()
}
pub fn open_with() -> &'static str {
    super::sidebar::menu_open_with()
}
pub fn reveal(fm: FileManager) -> &'static str {
    super::sidebar::menu_reveal(fm)
}
pub fn copy_rel() -> &'static str {
    super::sidebar::menu_copy_rel()
}
pub fn copy_abs() -> &'static str {
    super::sidebar::menu_copy_abs()
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                open_in_tako_file().to_string(),
                open_in_tako_dir().to_string(),
                open_default().to_string(),
                open_with().to_string(),
                reveal(FileManager::Finder).to_string(),
                reveal(FileManager::Explorer).to_string(),
                copy_rel().to_string(),
                copy_abs().to_string(),
            ]
        });
    }

    /// ツリー（#314）と**同じ文言**を出していること。
    /// 片方だけ言い回しを変える改変をここで落とす。
    ///
    /// 比較は `for_each_lang` の中で行う（#1274）。表示言語はプロセス全体の
    /// グローバルなので、ロックの外で 2 点を読むと**読み取りのあいだに別スレッドの
    /// テストが言語を切り替え、別言語同士を比べて落ちる**。日英それぞれで見るので
    /// 検査としても強くなる
    #[test]
    fn 共通項目はファイルツリーと同一文言() {
        tests_support::for_each_lang(|| {
            assert_eq!(open_default(), super::super::sidebar::menu_open_default());
            assert_eq!(open_with(), super::super::sidebar::menu_open_with());
            assert_eq!(copy_rel(), super::super::sidebar::menu_copy_rel());
            assert_eq!(copy_abs(), super::super::sidebar::menu_copy_abs());
            for fm in [FileManager::Finder, FileManager::Explorer] {
                assert_eq!(reveal(fm), super::super::sidebar::menu_reveal(fm));
            }
        });
    }
}

//! シェル向けのパスクオートユーティリティ（POSIX 準拠）

use std::path::Path;

/// 単語を POSIX シェルの単引用符で安全にクオートする。
/// 英数と無害な記号のみの場合はそのまま返す。
/// 先頭 `=` は zsh の equals 展開を踏むため必ず包む
pub fn quote_for_shell(word: &str) -> String {
    let safe = !word.is_empty()
        && !word.starts_with('=')
        && !word.starts_with('~')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// 複数のパスをシェル安全にクオートしてスペース区切りで結合する。
/// ターミナルへの D&D パス挿入用
pub fn quote_paths_for_shell(paths: &[impl AsRef<Path>]) -> String {
    paths
        .iter()
        .map(|p| quote_for_shell(&p.as_ref().display().to_string()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn 安全なパスはそのまま() {
        assert_eq!(quote_for_shell("/usr/bin/ls"), "/usr/bin/ls");
        assert_eq!(quote_for_shell("file.txt"), "file.txt");
        assert_eq!(quote_for_shell("/tmp/a-b_c.d"), "/tmp/a-b_c.d");
    }

    #[test]
    fn 空文字列はクオート() {
        assert_eq!(quote_for_shell(""), "''");
    }

    #[test]
    fn スペース含みはクオート() {
        assert_eq!(quote_for_shell("my file.txt"), "'my file.txt'");
        assert_eq!(
            quote_for_shell("/Users/me/my documents/file.txt"),
            "'/Users/me/my documents/file.txt'"
        );
    }

    #[test]
    fn 日本語パスはクオート() {
        assert_eq!(
            quote_for_shell("/Users/me/ドキュメント/ファイル.txt"),
            "'/Users/me/ドキュメント/ファイル.txt'"
        );
    }

    #[test]
    fn シングルクオート含みのエスケープ() {
        assert_eq!(quote_for_shell("it's"), "'it'\\''s'");
        assert_eq!(quote_for_shell("a'b'c"), "'a'\\''b'\\''c'");
        assert_eq!(quote_for_shell("'"), "''\\'''");
    }

    #[test]
    fn ダブルクオート含み() {
        assert_eq!(quote_for_shell(r#"say "hello""#), r#"'say "hello"'"#);
    }

    #[test]
    fn シェル特殊文字のクオート() {
        assert_eq!(quote_for_shell("$HOME"), "'$HOME'");
        assert_eq!(quote_for_shell("a*b"), "'a*b'");
        assert_eq!(quote_for_shell("a?b"), "'a?b'");
        assert_eq!(quote_for_shell("a[0]"), "'a[0]'");
        assert_eq!(quote_for_shell("`cmd`"), "'`cmd`'");
        assert_eq!(quote_for_shell("a!b"), "'a!b'");
        assert_eq!(quote_for_shell("a#b"), "'a#b'");
        assert_eq!(quote_for_shell("a;b"), "'a;b'");
        assert_eq!(quote_for_shell("a|b"), "'a|b'");
        assert_eq!(quote_for_shell("a&b"), "'a&b'");
        assert_eq!(quote_for_shell("a(b)"), "'a(b)'");
    }

    #[test]
    fn 先頭チルダはクオート() {
        assert_eq!(quote_for_shell("~/Documents"), "'~/Documents'");
    }

    #[test]
    fn 先頭イコールはクオート() {
        assert_eq!(quote_for_shell("=dnd-src"), "'=dnd-src'");
        // 途中の = は安全
        assert_eq!(quote_for_shell("KEY=val"), "KEY=val");
    }

    #[test]
    fn バックスラッシュ含み() {
        assert_eq!(quote_for_shell("a\\b"), "'a\\b'");
    }

    #[test]
    fn 複数パスの結合() {
        let paths = vec![
            PathBuf::from("/tmp/a.txt"),
            PathBuf::from("/tmp/my file.txt"),
            PathBuf::from("/tmp/it's.txt"),
        ];
        assert_eq!(
            quote_paths_for_shell(&paths),
            "/tmp/a.txt '/tmp/my file.txt' '/tmp/it'\\''s.txt'"
        );
    }

    #[test]
    fn 空パスリスト() {
        let paths: Vec<PathBuf> = vec![];
        assert_eq!(quote_paths_for_shell(&paths), "");
    }

    #[test]
    fn 単一パス() {
        let paths = vec![PathBuf::from("/usr/local/bin")];
        assert_eq!(quote_paths_for_shell(&paths), "/usr/local/bin");
    }
}

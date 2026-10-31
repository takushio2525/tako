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

/// [`quote_for_shell`] の csh / tcsh 版（#2597）。csh は単引用符の中でも `!`（履歴置換 = `'!x'` は
/// `x: Event not found.`）と改行（`Unmatched '''.`）を解釈するので、その 2 つの前にだけ `\` を置く。
/// 単引用符の中の `\!` は `!`、`\` + 改行は改行になり、それ以外の `\` はそのまま残る（tcsh 6.21 / csh で実測）。
/// どちらも含まない語は [`quote_for_shell`] とバイト一致（`!` と改行は素のままの語に入らない）
pub fn quote_for_csh(word: &str) -> String {
    quote_for_shell(word)
        .replace('!', "\\!")
        .replace('\n', "\\\n")
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

    /// #2597: csh の引用は `!` と改行の前にだけ `\` を足す。どちらも無い語は POSIX とバイト一致
    #[test]
    fn cshの引用はびっくりと改行だけを足す() {
        assert_eq!(quote_for_csh("a!b"), "'a\\!b'");
        assert_eq!(quote_for_csh("a\nb"), "'a\\\nb'");
        assert_eq!(quote_for_csh("it's!\n"), "'it'\\''s\\!\\\n'");
        for word in [
            "/usr/bin/ls",
            "",
            "my file",
            "it's",
            "$HOME `x`",
            "a\\b",
            "日本語",
            "~/x",
        ] {
            assert_eq!(quote_for_csh(word), quote_for_shell(word), "{word:?}");
        }
    }

    /// #2597 の実経路: 本物の tcsh / csh が `-c` の 1 行を解釈して元の語へ戻す（`-f` = rc を読まない）
    #[cfg(unix)]
    #[test]
    fn cshの引用は本物のtcshで元の語へ戻る() {
        let words = [
            "plain",
            "a b",
            "it's",
            "!x",
            "a!b",
            "a\\!b",
            "line1\nline2\n",
            "end\\",
            "a\\\nb",
            "$HOME `date`",
            "日本語!",
            "q\\\\!",
            "'",
            "",
        ];
        let line = |quote: fn(&str) -> String| {
            std::iter::once("printf '%s\\0'".to_string())
                .chain(words.iter().map(|w| quote(w)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let want: Vec<u8> = words
            .iter()
            .flat_map(|w| [w.as_bytes(), b"\0"].concat())
            .collect();
        let run = |shell: &str, line: &str| {
            std::process::Command::new(shell)
                .args(["-f", "-c", line])
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::null())
                .output()
                .expect("csh を起こせる")
        };
        let mut tried = 0;
        for shell in ["/bin/tcsh", "/bin/csh"] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            tried += 1;
            let out = run(shell, &line(quote_for_csh));
            assert!(
                out.status.success(),
                "{shell}: {:?} {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(out.stdout, want, "{shell}");
            // 修正前の POSIX の引用は同じ語で崩れる（同じテストの中の A/B）
            let before = run(shell, &line(quote_for_shell));
            assert!(
                !before.status.success() || before.stdout != want,
                "{shell}: POSIX の引用でも通ってしまう = 検査が csh の差を見ていない"
            );
        }
        if tried == 0 {
            eprintln!("tcsh / csh が無いので実経路は飛ばす（macOS は同梱）");
        }
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

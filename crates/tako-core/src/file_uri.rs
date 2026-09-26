//! `file://` URI からローカルパスへ（抽象境界。#913）
//!
//! **RFC 8089 の「Windows 形式」を実装するのはここ 1 箇所**。同じ規則が
//! 2 か所に散ると、片方だけが POSIX 専用のまま取り残される
//! （実際そうなっていた: OSC 7 の cwd 追従（[`crate::osc_tap`]）は
//! ドライブレターを扱えるのに、Finder の「このアプリケーションで開く」の
//! 受け口（`tako-app::open_files`）は `/` 始まりでないと弾いていた。#913）。
//!
//! パーセントデコードはここに置かない。**不正入力の方針が用途で違う**ためで、
//! これは意図的な分岐:
//!
//! - OSC 7（`osc_tap`）は `%zz` を**拒否**する（端末から来る壊れたバイト列で
//!   cwd を誤って移さない）
//! - 「このアプリケーションで開く」（`open_files`）は `%zz` を**素通り**させる
//!   （落として別のファイルを開くより「開けない」で止まるほうが安全）

/// file URI の**パス部**（`file://<host>` を剥がした残り）をローカルパスへ。
///
/// `file:///C:/Users/x` の先頭 `/` を落とす（RFC 8089 の Windows 形式 file URI）。
/// これを付けたまま渡すと Windows では**存在しないパス**になる
/// （cwd 追従なら全滅、開く経路なら「4 本のうち 1 本しかパスへ戻らない」）。
///
/// 落とす条件は「`/` + ASCII 英字 + `:` の直後が `/` か終端」に限るので、
/// `C:` という名前のディレクトリを持つ POSIX の絶対パスとは取り違えない
/// （それでも当たるのは `/C:` ちょうどか `/C:/…` だけ。実在すれば異常な構成）。
///
/// **プラットフォームで分岐しない**のが要点: 判定は URI の形だけで決まるので
/// macOS 上から Windows 形式の入力を通したテストが書ける（#515 の方針）
pub fn strip_drive_slash(path: &str) -> &str {
    let bytes = path.as_bytes();
    let looks_like_drive = bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && (bytes.len() == 3 || bytes[3] == b'/');
    if looks_like_drive {
        &path[1..]
    } else {
        path
    }
}

/// `/` 区切りで来たローカルパスを、そのプラットフォームの区切りへ寄せる（#1073 / #1102）。
///
/// なぜ要るか: `file://` URI のパスは常に `/` 区切りなので、Windows でそのまま
/// `PathBuf` にすると**表示だけが `C:/Users/…` になる**。`Path` の比較は
/// 成分単位なので動作は変わらないが、`display()` の結果が `tako list` / MCP の
/// 応答・ペインヘッダの cwd チップへそのまま出るため、**同じ機に同じパスの
/// 2 通りの表記が混ざる**（他の経路は OS の区切りで出る）。
///
/// **`/` 区切りで来る入口は URI だけではない**ので、変換の実装はここ 1 本にして
/// 呼び出し側が個別に区切り文字を触らないようにする。いまの呼び出し元は
/// OSC 7 の cwd 追従（[`crate::osc_tap`]。#1073）と、git の
/// `rev-parse --show-toplevel`（[`crate::git::normalize_repo_root`]。#1102。
/// git は Windows でも `/` で返す）の 2 つ。
///
/// unix では `\` はファイル名に使える普通の文字なので**触らない**。判定は
/// `MAIN_SEPARATOR` を引数で受けるので、macOS 上から Windows の挙動を検証できる
/// （#515 の方針）
pub fn native_separators(path: &str) -> std::borrow::Cow<'_, str> {
    native_separators_with(path, std::path::MAIN_SEPARATOR)
}

/// [`native_separators`] の区切り明示版（テスト用に両プラットフォームぶんを回す）
pub fn native_separators_with(path: &str, main_separator: char) -> std::borrow::Cow<'_, str> {
    if main_separator == '/' || !path.contains('/') {
        return std::borrow::Cow::Borrowed(path);
    }
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        out.push(if ch == '/' { main_separator } else { ch });
    }
    std::borrow::Cow::Owned(out)
}

/// ローカルパスを `file://` URI へ（#1678。LSP の `rootUri` / `textDocument.uri`）。
///
/// 上の関数群と**逆向き**の変換で、RFC 8089 の Windows 形式もここ 1 箇所で持つ
/// （`C:\a\b` → `file:///C:/a/b`、UNC `\\host\share\x` → `file://host/share/x`、
/// verbatim の `\\?\` は剥がす）。パスは RFC 3986 の unreserved と `/` 以外を
/// `%XX`（UTF-8 のバイト単位・大文字）へ符号化する。`canonicalize` はしない
/// （受け取ったパスをそのまま写す。`.agent/conventions.md`「Issue #970」）
pub fn from_path(path: &std::path::Path) -> String {
    from_path_str(&path.to_string_lossy(), cfg!(windows))
}

/// [`from_path`] の OS 明示版（**macOS 上から Windows 形式を検証できる**よう純粋関数にする）
pub fn from_path_str(path: &str, windows: bool) -> String {
    if !windows {
        return format!("file://{}", percent_encode_path(path));
    }
    let path = path
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(path).to_string());
    let slashed = path.replace('\\', "/");
    if let Some(unc) = slashed.strip_prefix("//") {
        let (host, rest) = unc.split_once('/').unwrap_or((unc, ""));
        return format!("file://{host}/{}", percent_encode_path(rest));
    }
    // ドライブ文字の `:` はそのまま残す（`file:///C:/…`）
    let bytes = slashed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return format!(
            "file:///{}:{}",
            &slashed[..1],
            percent_encode_path(&slashed[2..])
        );
    }
    format!("file://{}", percent_encode_path(&slashed))
}

/// unreserved（英数字と `-._~`）と `/` 以外を `%XX` にする
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows形式のドライブレターは先頭スラッシュを落とす() {
        assert_eq!(strip_drive_slash("/C:/Users/x"), "C:/Users/x");
        assert_eq!(strip_drive_slash("/d:/"), "d:/");
        // ドライブだけ（末尾なし）も対象
        assert_eq!(strip_drive_slash("/C:"), "C:");
    }

    #[test]
    fn posixの絶対パスは触らない() {
        assert_eq!(
            strip_drive_slash("/Users/me/notes.md"),
            "/Users/me/notes.md"
        );
        assert_eq!(strip_drive_slash("/tmp/a.md"), "/tmp/a.md");
        assert_eq!(strip_drive_slash("/"), "/");
        assert_eq!(strip_drive_slash(""), "");
        // 末尾スラッシュは保つ（呼び出し側の is_dir 判定に回す）
        assert_eq!(strip_drive_slash("/Users/me/proj/"), "/Users/me/proj/");
    }

    #[test]
    fn ドライブに見えて違うものは触らない() {
        // 英字 1 文字 + `:` の直後が `/` でも終端でもない
        assert_eq!(strip_drive_slash("/C:x/y"), "/C:x/y");
        // 2 文字以上はドライブレターではない
        assert_eq!(strip_drive_slash("/AB:/x"), "/AB:/x");
        // 英字でない
        assert_eq!(strip_drive_slash("/1:/x"), "/1:/x");
        // 先頭が `/` でない（パス部は必ず `/` 始まり = 想定外の入力）
        assert_eq!(strip_drive_slash("C:/x"), "C:/x");
    }

    #[test]
    fn uri由来のパスはプラットフォームの区切りへ寄せる() {
        // Windows: URI の `/` を `\` へ（表示が他の経路と揃う）
        assert_eq!(
            native_separators_with("C:/Users/x/dev", '\\'),
            "C:\\Users\\x\\dev"
        );
        assert_eq!(native_separators_with("C:/", '\\'), "C:\\");
        // 区切りが `/` のプラットフォームでは 1 文字も触らない（`\` は普通の文字）
        assert_eq!(
            native_separators_with("/Users/me/a\\b", '/'),
            "/Users/me/a\\b"
        );
        // 区切りを含まない入力はそのまま借用で返る（無駄な確保をしない）
        assert!(matches!(
            native_separators_with("C:", '\\'),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn 区切りを寄せてもpathとしての同一性は変わらない() {
        // 動作（Path の比較）は元から同じで、変わるのは表示だけ
        // = この変換で「別のパスを指す」ことは起きない
        let native = native_separators_with("C:/Users/x/dev", std::path::MAIN_SEPARATOR);
        assert_eq!(
            std::path::Path::new(native.as_ref()),
            std::path::Path::new("C:/Users/x/dev")
        );
    }

    #[test]
    fn パスから_file_uri_を組む_posix() {
        assert_eq!(
            from_path_str("/w/src/main.rs", false),
            "file:///w/src/main.rs"
        );
        assert_eq!(
            from_path_str("/w/a b/日本.rs", false),
            "file:///w/a%20b/%E6%97%A5%E6%9C%AC.rs"
        );
        assert_eq!(from_path_str("/w/#x%y", false), "file:///w/%23x%25y");
    }

    #[test]
    fn パスから_file_uri_を組む_windows() {
        assert_eq!(
            from_path_str(r"C:\w\src\main.rs", true),
            "file:///C:/w/src/main.rs"
        );
        assert_eq!(
            from_path_str(r"\\?\C:\w\a b.rs", true),
            "file:///C:/w/a%20b.rs"
        );
        assert_eq!(
            from_path_str(r"\\host\share\x.rs", true),
            "file://host/share/x.rs"
        );
        assert_eq!(
            from_path_str(r"\\?\UNC\host\share\x.rs", true),
            "file://host/share/x.rs"
        );
    }
}

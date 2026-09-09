//! テキストの前処理（読み込んだ本文を解釈へ渡す前に整える。Issue #1202）

/// 先頭の UTF-8 BOM（`EF BB BF` = `U+FEFF`）を落とす。
///
/// **なぜ要るか**: BOM は「これは UTF-8 です」という印であって本文ではないのに、
/// `String::from_utf8_lossy` は文字として残す。行頭を見て解釈するもの
/// （Markdown の `#` 見出し・`tako:run` 宣言）は、BOM が 1 文字挟まるだけで
/// **先頭行だけ認識に失敗する**。
///
/// **なぜ Windows で効くか**: BOM 付き UTF-8 は Windows では珍しくない既定値
/// （PowerShell 5.1 の `Set-Content -Encoding UTF8` / `Out-File -Encoding utf8`、
/// メモ帳の「UTF-8 (BOM 付き)」、Visual Studio）。macOS 側の道具はまず付けないので、
/// **macOS だけで触っていると踏まない**（#1202 の起票者は Windows でレビュー用の
/// サンプルを PowerShell から作っただけで踏んだ）。
///
/// **剥がすのは先頭 1 個だけ**。文中の `U+FEFF`（ゼロ幅ノーブレークスペース）は
/// 本文の一部かもしれないので触らない。
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 先頭のbomだけを落とす() {
        assert_eq!(strip_bom("\u{feff}# 見出し"), "# 見出し");
        // BOM 無しは素通し（1 文字も変えない）
        assert_eq!(strip_bom("# 見出し"), "# 見出し");
        // 空文字・BOM だけ
        assert_eq!(strip_bom(""), "");
        assert_eq!(strip_bom("\u{feff}"), "");
    }

    #[test]
    fn 文中と2個目のbomは残す() {
        // 2 個目は本文の一部として扱う（剥がすのは先頭 1 個だけ）
        assert_eq!(strip_bom("\u{feff}\u{feff}# 見出し"), "\u{feff}# 見出し");
        // 文中のゼロ幅ノーブレークスペースは触らない
        assert_eq!(strip_bom("a\u{feff}b"), "a\u{feff}b");
        // 行頭でも先頭行でなければ触らない
        assert_eq!(
            strip_bom("# 見出し\n\u{feff}## 節"),
            "# 見出し\n\u{feff}## 節"
        );
    }
}

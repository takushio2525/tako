//! テキストの前処理（読み込んだ本文を解釈へ渡す前に整える。Issue #1202）と、
//! 表示用の切り詰め（文字境界で切る。Issue #1746）

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

/// 文字数ベースの切り詰め（`…` を含めて `max_chars` 文字以内）。
///
/// **表示用の切り詰めはこれを通す**。`&s[..N]` の `N` はバイト位置なので、多バイト文字の
/// 途中に当たると panic する（#1746: `tako task gate show` が日本語を含む証拠を
/// `&ev[..120]` で切って落ちた。#1728 は同じ形がプレビューの描画中に起きた）。
///
/// 数えるのは `char`（Unicode スカラー値）なので、結合文字（濁点の `U+3099` など）は
/// 基底文字と別の 1 文字として数える。基底文字と結合文字のあいだで切れることはあるが
/// （見た目の欠け）、結果は常に正しい UTF-8 で落ちはしない。表示幅（全角 = 2 桁）は見ない。
/// `max_chars` が 0 のときも、空でなければ `…` の 1 文字を返す。
///
/// **文字数で切る実装はこれ 1 本**（#1757 で tako-app の `truncate` と context_budget の
/// 私有版を寄せた。transcript の 1 行要約 `summary_line` も数える部分はここを通す）。
/// 規約は `.agent/conventions.md`「文字列をバイト位置で切らない」
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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

    /// 切り詰めの結果が「文字境界で終わり、上限以内で、切ったなら `…` で終わる」こと
    fn assert_truncated(src: &str, max: usize, out: &str) {
        assert!(
            src.starts_with(out.trim_end_matches('…')),
            "元の文字列の先頭になっていない: {out:?}"
        );
        assert!(
            out.chars().count() <= max.max(1),
            "{max} 文字を超えた（{} 文字）: {out:?}",
            out.chars().count()
        );
        if out != src {
            assert!(out.ends_with('…'), "切ったのに `…` が付いていない: {out:?}");
        }
    }

    #[test]
    fn 上限ちょうどは切らず_1文字あふれたら上限へ収める() {
        let exact = "あ".repeat(120);
        assert_eq!(truncate_chars(&exact, 120), exact);
        let over = "あ".repeat(121);
        let out = truncate_chars(&over, 120);
        assert_truncated(&over, 120, &out);
        assert_eq!(out.chars().count(), 120);
        assert_eq!(out, format!("{}…", "あ".repeat(119)));
    }

    /// #1746 の実物: `exit 0; stdout: `（16 バイト）+ 3 バイト文字で、
    /// 120 バイト目が「あ」（118..121）の途中に当たる
    #[test]
    fn バイト位置が文字の途中でも落ちない() {
        let ev = format!("exit 0; stdout: {}", "あ".repeat(150));
        assert!(!ev.is_char_boundary(120), "場面が #1746 の形になっていない");
        let out = truncate_chars(&ev, 120);
        assert_truncated(&ev, 120, &out);
        assert_eq!(out, format!("exit 0; stdout: {}…", "あ".repeat(103)));
    }

    #[test]
    fn asciiだけでも上限内で切る() {
        assert_eq!(truncate_chars("exit 0", 120), "exit 0");
        let long = "x".repeat(200);
        assert_eq!(truncate_chars(&long, 120), format!("{}…", "x".repeat(119)));
    }

    #[test]
    fn 空は空のまま() {
        assert_eq!(truncate_chars("", 120), "");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn 四バイト文字の途中で切らない() {
        // 絵文字・CJK 拡張 B（どちらも UTF-8 で 4 バイト）
        let src = format!("ok {}{}", "🎉".repeat(10), "𠮷".repeat(10));
        let out = truncate_chars(&src, 8);
        assert_truncated(&src, 8, &out);
        assert_eq!(out, "ok 🎉🎉🎉🎉…");
    }

    #[test]
    fn 結合文字でも落ちない() {
        // 濁点を合成文字で書いた「が」（macOS のファイル名は NFD で届くことがある）。
        // 基底文字と濁点のあいだで切れることはあるが、結果は正しい UTF-8
        let nfd = "か\u{3099}".repeat(20);
        for max in [1, 2, 3, 10, 11, 39, 40, 41] {
            let out = truncate_chars(&nfd, max);
            assert_truncated(&nfd, max, &out);
        }
        // 上限 4 は「か」の直後（濁点の手前）で切れる = 欠けはするが panic しない
        assert_eq!(truncate_chars(&nfd, 4), "か\u{3099}か…");
    }

    /// 全上限の総当たり: どの上限で切っても落ちず、文字境界で終わる
    #[test]
    fn どの上限でも文字境界で終わる() {
        let nfd = "か\u{3099}".repeat(8);
        let mixed = format!("exit 1; stderr: 失敗 🎉 x\ny {nfd}");
        for src in [mixed.as_str(), nfd.as_str(), "abc", ""] {
            for max in 0..=src.len() + 1 {
                let out = truncate_chars(src, max);
                assert_truncated(src, max, &out);
            }
        }
    }

    /// #1757 で tako-app の `truncate`（タブ名・URL・実行コマンドの見出し）と
    /// context_budget の私有版をここへ寄せた。寄せる前の 2 つと同じ固定値を持つ
    #[test]
    fn 寄せる前のtruncateと同じ固定値() {
        // `…` は上限の内側に数える（24 文字の上限で 23 文字 + `…`）
        let title = "a".repeat(30);
        assert_eq!(truncate_chars(&title, 24), format!("{}…", "a".repeat(23)));
        // 幅は表示幅ではなく文字数（全角も 1 文字。3 文字の上限で表示幅 5 桁）
        assert_eq!(truncate_chars("ああああ", 3), "ああ…");
        // 上限 0 / 1 は空でなければ `…` だけ、2 は 1 文字 + `…`
        assert_eq!(truncate_chars("abc", 0), "…");
        assert_eq!(truncate_chars("abc", 1), "…");
        assert_eq!(truncate_chars("abc", 2), "a…");
        // ちょうど上限は切らない（4 バイト文字でも数えるのは 1 文字）
        assert_eq!(truncate_chars("🎉🎉🎉", 3), "🎉🎉🎉");
        assert_eq!(truncate_chars("🎉🎉🎉🎉", 3), "🎉🎉…");
    }
}

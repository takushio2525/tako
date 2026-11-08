//! テキストの前処理（読み込んだ本文を解釈へ渡す前に整える。Issue #1202）・
//! バイト列の符号化の判定と復号（BOM 付きの UTF-8 / UTF-16。Issue #2789）と、
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

/// [`decode_text`] が読んだ符号化
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// BOM の無い UTF-8
    Utf8,
    /// BOM 付きの UTF-8（PowerShell 5.1 の `Out-File -Encoding utf8`）
    Utf8Bom,
    /// BOM 付きの UTF-16LE（PowerShell 5.1 の `>` / `Out-File` の既定・メモ帳の「UTF-16 LE」）
    Utf16Le,
    /// BOM 付きの UTF-16BE（PowerShell の `-Encoding bigendianunicode`）
    Utf16Be,
}

/// [`decode_text`] が読めない形。`offset` はファイル先頭からのバイト位置（BOM を含めて数える）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDecodeError {
    /// BOM の無い UTF-16 に見える（NUL のバイトを含む = テキストの UTF-8 ではない）。
    /// どちらの順かは NUL が偶数位置と奇数位置のどちらに多いかで決める
    Utf16WithoutBom { big_endian: bool },
    /// UTF-16 の BOM の後ろが奇数バイト（途中で切れている）。`len` は全体のバイト数
    Utf16OddLength { big_endian: bool, len: usize },
    /// UTF-16 の BOM はあるが、対にならないサロゲートがある
    Utf16Invalid { big_endian: bool, offset: usize },
    /// UTF-32 の BOM（読まない）
    Utf32 { big_endian: bool },
    /// UTF-8 として不正なバイトがある（Shift_JIS などの ANSI の書き出し・壊れたファイル）
    InvalidUtf8 { offset: usize },
}

impl TextDecodeError {
    /// 何の形と見たか（表示言語を明示。網羅 match = バリアントを足すと日英の両方を書くまで通らない）。
    /// 直し方は読み元ごとに違うので呼び手が添える
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        let utf16 = |big_endian: bool| if big_endian { "UTF-16BE" } else { "UTF-16LE" };
        let utf32 = |big_endian: bool| if big_endian { "UTF-32BE" } else { "UTF-32LE" };
        match (self, lang) {
            (Self::Utf16WithoutBom { big_endian }, Ja) => format!(
                "BOM の無い {} に見えます（NUL のバイトを含みます）",
                utf16(*big_endian)
            ),
            (Self::Utf16WithoutBom { big_endian }, En) => format!(
                "it looks like {} without a byte order mark (it contains NUL bytes)",
                utf16(*big_endian)
            ),
            (Self::Utf16OddLength { big_endian, len }, Ja) => format!(
                "BOM 付きの {} ですが、長さが奇数（{len} バイト）で途中で切れています",
                utf16(*big_endian)
            ),
            (Self::Utf16OddLength { big_endian, len }, En) => format!(
                "it is {} with a byte order mark, but its length is odd ({len} bytes), so it is cut off",
                utf16(*big_endian)
            ),
            (Self::Utf16Invalid { big_endian, offset }, Ja) => format!(
                "BOM 付きの {} ですが、{offset} バイト目に対にならないサロゲートがあります",
                utf16(*big_endian)
            ),
            (Self::Utf16Invalid { big_endian, offset }, En) => format!(
                "it is {} with a byte order mark, but has an unpaired surrogate at byte {offset}",
                utf16(*big_endian)
            ),
            (Self::Utf32 { big_endian }, Ja) => {
                format!("{}（BOM 付き）は読めません", utf32(*big_endian))
            }
            (Self::Utf32 { big_endian }, En) => format!(
                "{} (with a byte order mark) is not supported",
                utf32(*big_endian)
            ),
            (Self::InvalidUtf8 { offset }, Ja) => format!(
                "{offset} バイト目が UTF-8 として不正です（Shift_JIS などの ANSI の符号化で書かれているかもしれません）"
            ),
            (Self::InvalidUtf8 { offset }, En) => format!(
                "it is not valid UTF-8 at byte {offset} (it may be written in an ANSI code page such as Shift_JIS)"
            ),
        }
    }
}

impl std::fmt::Display for TextDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl std::error::Error for TextDecodeError {}

/// 読み込んだバイト列を文字列にする。BOM で符号化を決め、BOM は本文に残さない（Issue #2789）。
///
/// **なぜ要るか**: Windows PowerShell 5.1 の `>` / `Out-File` は既定で **BOM 付きの UTF-16LE** を書き、
/// `-Encoding utf8` を付けても **BOM 付きの UTF-8** になる。`read_to_string` は前者を
/// 「UTF-8 でない」で断り、後者は BOM が 1 文字残って JSON の解析が 1 文字目で落ちる。
/// 手順書どおりに作った答えのファイルが Windows でだけ通らない形になる（macOS の道具はどちらも書かない）。
///
/// 受けるのは UTF-8（BOM の有無を問わない）と BOM 付きの UTF-16LE / BE。読めない形は
/// [`TextDecodeError`] で何の形と見たかを返す: BOM の無い UTF-16（NUL を含む = テキストの UTF-8 では
/// ない）・奇数長の UTF-16・対にならないサロゲート・UTF-32・不正な UTF-8。剥がす BOM は先頭の 1 個だけで、
/// 2 個目以降は [`strip_bom`] と同じく本文として残す
pub fn decode_text(bytes: &[u8]) -> Result<(String, TextEncoding), TextDecodeError> {
    const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
    // UTF-32LE の BOM（FF FE 00 00）は UTF-16LE の BOM（FF FE）を頭に含むので先に見る
    if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        return Err(TextDecodeError::Utf32 { big_endian: false });
    }
    if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        return Err(TextDecodeError::Utf32 { big_endian: true });
    }
    if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
        return utf8(rest, UTF8_BOM.len()).map(|s| (s, TextEncoding::Utf8Bom));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return utf16(rest, false, bytes.len()).map(|s| (s, TextEncoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return utf16(rest, true, bytes.len()).map(|s| (s, TextEncoding::Utf16Be));
    }
    // NUL は UTF-8 としては正しいが、テキストのファイルには入らない。入っていれば BOM の無い UTF-16
    // （ASCII の文字の上位バイトが NUL）と見る。LE なら NUL は奇数位置、BE なら偶数位置に並ぶ
    if bytes.contains(&0) {
        let (even, odd) = bytes.iter().enumerate().filter(|(_, b)| **b == 0).fold(
            (0usize, 0usize),
            |(even, odd), (i, _)| {
                if i.is_multiple_of(2) {
                    (even + 1, odd)
                } else {
                    (even, odd + 1)
                }
            },
        );
        return Err(TextDecodeError::Utf16WithoutBom {
            big_endian: even > odd,
        });
    }
    utf8(bytes, 0).map(|s| (s, TextEncoding::Utf8))
}

/// UTF-8 の本文（`head` は前に付いていた BOM のバイト数 = 位置の数え始め）
fn utf8(body: &[u8], head: usize) -> Result<String, TextDecodeError> {
    std::str::from_utf8(body)
        .map(str::to_string)
        .map_err(|e| TextDecodeError::InvalidUtf8 {
            offset: head + e.valid_up_to(),
        })
}

/// BOM の後ろの UTF-16 の本文（`total` は BOM を含む全体のバイト数）
fn utf16(body: &[u8], big_endian: bool, total: usize) -> Result<String, TextDecodeError> {
    if !body.len().is_multiple_of(2) {
        return Err(TextDecodeError::Utf16OddLength {
            big_endian,
            len: total,
        });
    }
    let units = body.chunks_exact(2).map(|pair| {
        let pair = [pair[0], pair[1]];
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    });
    let mut out = String::with_capacity(body.len() / 2);
    // 何個目の符号単位まで読めたか（壊れた位置をバイトで名指すため）
    let mut unit = 0usize;
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(c) => {
                out.push(c);
                unit += c.len_utf16();
            }
            Err(_) => {
                return Err(TextDecodeError::Utf16Invalid {
                    big_endian,
                    offset: 2 + unit * 2,
                })
            }
        }
    }
    Ok(out)
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
/// 私有版を、#1790 で `truncate_path_middle` の末尾省略・`trim_detail`・`prompt_head` を、
/// #2217 で右パネルの `git_feedback_text` を寄せた）。`…` を上限の外に付ける呼び手
/// （transcript の `summary_line`・`trim_detail`・`prompt_head`・`git_feedback_text`）は
/// 上限を `max + 1` で渡す。
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

    /// UTF-16 の BOM + 本文（`big_endian` で順を選ぶ）
    fn utf16_with_bom(text: &str, big_endian: bool) -> Vec<u8> {
        let mut out = if big_endian {
            vec![0xfe, 0xff]
        } else {
            vec![0xff, 0xfe]
        };
        for unit in text.encode_utf16() {
            out.extend(if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            });
        }
        out
    }

    /// #2789: PowerShell 5.1 が書く 2 形（`Out-File` の既定 = UTF-16LE + BOM / `-Encoding utf8` = UTF-8 + BOM）と
    /// BE・素の UTF-8 が同じ本文になる。サロゲート対（絵文字・CJK 拡張 B）と CRLF・末尾の改行もそのまま
    #[test]
    fn issue2789_bom付きのutf8とutf16は素のutf8と同じ本文になる() {
        let text = "{\"instruction_content\":\"日本語 🎉 𠮷\"}\r\n";
        let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
        utf8_bom.extend(text.as_bytes());
        let cases = [
            (text.as_bytes().to_vec(), TextEncoding::Utf8),
            (utf8_bom, TextEncoding::Utf8Bom),
            (utf16_with_bom(text, false), TextEncoding::Utf16Le),
            (utf16_with_bom(text, true), TextEncoding::Utf16Be),
        ];
        for (bytes, encoding) in cases {
            assert_eq!(
                decode_text(&bytes),
                Ok((text.to_string(), encoding)),
                "{encoding:?} の本文が素の UTF-8 と違う"
            );
        }
    }

    #[test]
    fn issue2789_空とbomだけは空の本文になる() {
        assert_eq!(decode_text(b""), Ok((String::new(), TextEncoding::Utf8)));
        assert_eq!(
            decode_text(&[0xef, 0xbb, 0xbf]),
            Ok((String::new(), TextEncoding::Utf8Bom))
        );
        assert_eq!(
            decode_text(&[0xff, 0xfe]),
            Ok((String::new(), TextEncoding::Utf16Le))
        );
        assert_eq!(
            decode_text(&[0xfe, 0xff]),
            Ok((String::new(), TextEncoding::Utf16Be))
        );
    }

    #[test]
    fn issue2789_剥がすbomは先頭の1個だけ() {
        // strip_bom と同じ: 2 個目は本文の一部
        assert_eq!(
            decode_text("\u{feff}\u{feff}{}".as_bytes()),
            Ok(("\u{feff}{}".to_string(), TextEncoding::Utf8Bom))
        );
        assert_eq!(
            decode_text(&utf16_with_bom("\u{feff}{}", false)),
            Ok(("\u{feff}{}".to_string(), TextEncoding::Utf16Le))
        );
    }

    #[test]
    fn issue2789_読めない形は何の形と見たかを返す() {
        // BOM の無い UTF-16（`[Text.UnicodeEncoding]::new($false, $false)` の書き出し）
        let le: Vec<u8> = "{}".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = "{}".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(
            decode_text(&le),
            Err(TextDecodeError::Utf16WithoutBom { big_endian: false })
        );
        assert_eq!(
            decode_text(&be),
            Err(TextDecodeError::Utf16WithoutBom { big_endian: true })
        );
        // 奇数バイトの UTF-16（BOM + `{` の 1 バイト目だけ）
        assert_eq!(
            decode_text(&[0xff, 0xfe, 0x7b]),
            Err(TextDecodeError::Utf16OddLength {
                big_endian: false,
                len: 3
            })
        );
        assert_eq!(
            decode_text(&[0xfe, 0xff, 0x00, 0x7b, 0x00]),
            Err(TextDecodeError::Utf16OddLength {
                big_endian: true,
                len: 5
            })
        );
        // 対にならないサロゲート（`{` の後ろの上位サロゲート = 4 バイト目）
        assert_eq!(
            decode_text(&[0xff, 0xfe, 0x7b, 0x00, 0x00, 0xd8, 0x7d, 0x00]),
            Err(TextDecodeError::Utf16Invalid {
                big_endian: false,
                offset: 4
            })
        );
        // UTF-32 の BOM（UTF-16LE の BOM と取り違えない）
        assert_eq!(
            decode_text(&[0xff, 0xfe, 0x00, 0x00, 0x7b, 0x00, 0x00, 0x00]),
            Err(TextDecodeError::Utf32 { big_endian: false })
        );
        assert_eq!(
            decode_text(&[0x00, 0x00, 0xfe, 0xff, 0x00, 0x00, 0x00, 0x7b]),
            Err(TextDecodeError::Utf32 { big_endian: true })
        );
        // Shift_JIS（CP932）の「日本」= PowerShell 5.1 の `Set-Content` の既定（ANSI）
        let sjis = b"{\"a\":\"\x93\xfa\x96\x7b\"}";
        assert_eq!(
            decode_text(sjis),
            Err(TextDecodeError::InvalidUtf8 { offset: 6 })
        );
        // BOM の後ろで壊れていれば、位置は BOM を含めて数える
        let mut bom_sjis = vec![0xef, 0xbb, 0xbf];
        bom_sjis.extend(sjis);
        assert_eq!(
            decode_text(&bom_sjis),
            Err(TextDecodeError::InvalidUtf8 { offset: 9 })
        );
    }

    #[test]
    fn issue2789_読めない形の文は日英で英語に日本語が混ざらない() {
        let all = [
            TextDecodeError::Utf16WithoutBom { big_endian: false },
            TextDecodeError::Utf16OddLength {
                big_endian: true,
                len: 3,
            },
            TextDecodeError::Utf16Invalid {
                big_endian: false,
                offset: 4,
            },
            TextDecodeError::Utf32 { big_endian: false },
            TextDecodeError::InvalidUtf8 { offset: 6 },
        ];
        for e in all {
            let (ja, en) = (
                e.text_in(crate::i18n::Lang::Ja),
                e.text_in(crate::i18n::Lang::En),
            );
            assert_ne!(ja, en, "{e:?}: 日英が同じ文");
            assert!(
                en.is_ascii(),
                "{e:?}: 英語に ASCII の外の文字が混ざる: {en}"
            );
            assert!(!ja.is_ascii(), "{e:?}: 日本語が英語のまま: {ja}");
        }
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

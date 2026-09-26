//! 診断（エラー・警告）のモデル（#1679 / エピック #1007 の S2）
//!
//! LSP の `Diagnostic` を **tako の座標**（0 起点の行 + 行内の UTF-8 バイト桁。
//! `TextBuffer::line_byte_col` と同じ流儀）へ写した形。GUI の波線・右パネルの一覧・
//! `tako lsp diagnostics` / MCP `tako_lsp` はすべてこれを読む。
//!
//! LSP の型（`lsp-types`）には依存しない。UTF-16 の桁からここへ写すのは受信側
//! （`tako_control::lsp::diagnostics`）で、変換は [`super::position::LineIndex`] の 1 実装を通る。

use std::ops::Range;

/// 重大度（LSP の 1〜4 と同じ並び。**小さいほど重い**）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    /// 重い順
    pub const ALL: [Severity; 4] = [
        Severity::Error,
        Severity::Warning,
        Severity::Information,
        Severity::Hint,
    ];

    /// CLI `--severity` / MCP `severity` の値と、応答の `severity` の綴り（重い順）
    pub const NAMES: [&'static str; 4] = ["error", "warning", "info", "hint"];

    /// LSP の数値（1〜4）から。**省略・範囲外はエラーとして扱う**。
    /// LSP 3.17 は省略時の解釈をクライアントに委ねていて、VS Code / Zed は重い側へ倒す
    /// （見落とさない側。軽い側へ倒すと `--severity error` から漏れる）
    pub fn from_lsp(value: Option<i64>) -> Self {
        match value {
            Some(2) => Self::Warning,
            Some(3) => Self::Information,
            Some(4) => Self::Hint,
            _ => Self::Error,
        }
    }

    pub fn slug(self) -> &'static str {
        Self::NAMES[self.rank()]
    }

    /// 綴りから（[`Self::NAMES`] だけを受ける。綴り違いを「絞り込み 0 件」に見せない）
    pub fn parse(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| Self::ALL[i])
    }

    /// 重い順の位置（0 = エラー）
    pub fn rank(self) -> usize {
        self as usize
    }

    /// `self` が `min` と同じかそれより重いか（`--severity warning` = エラーと警告）
    pub fn at_least(self, min: Severity) -> bool {
        self <= min
    }
}

/// 本文の位置（0 起点の行 + 行内の UTF-8 バイト桁）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

/// 診断 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub start: Point,
    /// `start` より手前にはならない（写すときに揃える）
    pub end: Point,
    pub message: String,
    /// 出所（`rustc` / `clippy` / `rust-analyzer` …）。サーバが付けなければ `None`
    pub source: Option<String>,
    /// コード（`E0308` 等）。数値のコードも文字列で持つ
    pub code: Option<String>,
}

/// 1 行の中で波線を引く範囲
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpan {
    /// 行内の UTF-8 バイト範囲。`past_end` のときは `行長..行長 + 1`
    /// （描き手が行末に空白を 1 つ足してその下へ引く）
    pub range: Range<usize>,
    pub severity: Severity,
    /// 行末（または空行）を指す幅 0 の診断。文字が無いので行末の外に 1 文字ぶん引く
    pub past_end: bool,
}

/// 重大度ごとの数（[`Severity::ALL`] の順）
pub fn counts<'a>(diagnostics: impl IntoIterator<Item = &'a Diagnostic>) -> [usize; 4] {
    let mut out = [0; 4];
    for d in diagnostics {
        out[d.severity.rank()] += 1;
    }
    out
}

/// 並べ方の正本（位置の順 → 同じ位置なら重い順）。一覧・CLI・MCP・描画が同じ順で読む
pub fn sort(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        (a.start, a.severity, a.end, &a.message).cmp(&(b.start, b.severity, b.end, &b.message))
    });
}

/// `line` 行目（本文は `line_text`。改行を含まない）に引く波線の範囲。
///
/// **軽い順に並べて返す**（描き手は後から積んだ装飾を上に重ねるので、同じ文字に
/// エラーと警告が重なったらエラーの色が勝つ）。
///
/// - 複数行にまたがる範囲は、始まりの行は始点から行末まで・途中の行は行全体・
///   終わりの行は行頭から終点まで
/// - 終点がその行の行頭（`col == 0`）なら、その行には引かない（前の行の行末までで終わっている）
/// - 途中の空行には引かない（引く文字が無い）
/// - **幅 0 の診断**（`start == end`。「ここに `;` が要る」型）は、その位置の 1 文字ぶんへ
///   広げる。行末・空行なら [`LineSpan::past_end`] で行末の外に 1 文字ぶん引く
/// - 範囲は行長へ丸め、文字の途中は手前の境界へ寄せる（サーバの版とすれ違った位置でも
///   描き手が文字の途中を切らない）
pub fn line_spans(diagnostics: &[Diagnostic], line: usize, line_text: &str) -> Vec<LineSpan> {
    let len = line_text.len();
    let snap = |mut at: usize| {
        at = at.min(len);
        while !line_text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let mut out: Vec<LineSpan> = Vec::new();
    for d in diagnostics {
        let last = d.end.line.max(d.start.line);
        if line < d.start.line || line > last {
            continue;
        }
        let on_start = line == d.start.line;
        let start = if on_start { snap(d.start.col) } else { 0 };
        let end = if line == d.end.line {
            snap(d.end.col)
        } else {
            len
        };
        if start < end {
            out.push(LineSpan {
                range: start..end,
                severity: d.severity,
                past_end: false,
            });
            continue;
        }
        // ここから幅 0。広げるのは診断の始まりの行だけ（途中の空行・行頭で終わる行は引かない）
        if !on_start {
            continue;
        }
        if start < len {
            let next = line_text[start..]
                .chars()
                .next()
                .map_or(len, |c| start + c.len_utf8());
            out.push(LineSpan {
                range: start..next,
                severity: d.severity,
                past_end: false,
            });
        } else {
            out.push(LineSpan {
                range: len..len + 1,
                severity: d.severity,
                past_end: true,
            });
        }
    }
    // 軽い順（重い装飾を最後に積む = 重なったら重い色が勝つ）
    out.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.range.start.cmp(&b.range.start))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: Severity, start: (usize, usize), end: (usize, usize)) -> Diagnostic {
        Diagnostic {
            severity,
            start: Point {
                line: start.0,
                col: start.1,
            },
            end: Point {
                line: end.0,
                col: end.1,
            },
            message: "m".into(),
            source: None,
            code: None,
        }
    }

    #[test]
    fn 重大度は_lsp_の数値と綴りを往復し省略はエラーに倒す() {
        assert_eq!(Severity::from_lsp(Some(1)), Severity::Error);
        assert_eq!(Severity::from_lsp(Some(2)), Severity::Warning);
        assert_eq!(Severity::from_lsp(Some(3)), Severity::Information);
        assert_eq!(Severity::from_lsp(Some(4)), Severity::Hint);
        assert_eq!(Severity::from_lsp(None), Severity::Error);
        assert_eq!(Severity::from_lsp(Some(9)), Severity::Error);
        for s in Severity::ALL {
            assert_eq!(Severity::parse(s.slug()), Some(s));
        }
        // 綴り違いは受けない（「0 件」に見せない）
        assert_eq!(Severity::parse("warn"), None);
        assert_eq!(Severity::parse("Error"), None);
    }

    /// 絞り込みは「その重大度以上」。境界の 2 件（同じ重さは入る / 1 段軽いのは入らない）
    #[test]
    fn その重大度以上に絞る() {
        assert!(Severity::Error.at_least(Severity::Error));
        assert!(!Severity::Warning.at_least(Severity::Error));
        assert!(Severity::Warning.at_least(Severity::Warning));
        assert!(!Severity::Information.at_least(Severity::Warning));
        for s in Severity::ALL {
            assert!(s.at_least(Severity::Hint), "hint は全部を含む");
        }
    }

    #[test]
    fn 数は重大度ごとに数える() {
        let d = [
            diag(Severity::Error, (0, 0), (0, 1)),
            diag(Severity::Error, (1, 0), (1, 1)),
            diag(Severity::Hint, (2, 0), (2, 1)),
        ];
        assert_eq!(counts(&d), [2, 0, 0, 1]);
        assert_eq!(counts(&[]), [0, 0, 0, 0]);
    }

    #[test]
    fn 並びは位置の順で同じ位置なら重い順() {
        let mut d = vec![
            diag(Severity::Warning, (3, 0), (3, 1)),
            diag(Severity::Hint, (1, 4), (1, 5)),
            diag(Severity::Error, (1, 4), (1, 5)),
        ];
        sort(&mut d);
        let got: Vec<_> = d.iter().map(|d| (d.start.line, d.severity)).collect();
        assert_eq!(
            got,
            vec![
                (1, Severity::Error),
                (1, Severity::Hint),
                (3, Severity::Warning)
            ]
        );
    }

    #[test]
    fn 一行の中の範囲はそのまま() {
        let d = [diag(Severity::Error, (0, 4), (0, 7))];
        assert_eq!(
            line_spans(&d, 0, "let abc = 1;"),
            vec![LineSpan {
                range: 4..7,
                severity: Severity::Error,
                past_end: false
            }]
        );
        assert!(line_spans(&d, 1, "let abc = 1;").is_empty());
    }

    #[test]
    fn 複数行の範囲は始まり_途中_終わりで切る() {
        let d = [diag(Severity::Warning, (0, 3), (2, 2))];
        let r = |line: usize, text: &str| -> Vec<Range<usize>> {
            line_spans(&d, line, text)
                .into_iter()
                .map(|s| s.range)
                .collect()
        };
        assert_eq!(r(0, "abcdef"), vec![3..6]);
        assert_eq!(r(1, "xyz"), vec![0..3]);
        assert_eq!(r(2, "pqrs"), vec![0..2]);
        // 途中の空行には引かない
        assert!(r(1, "").is_empty());
    }

    #[test]
    fn 次の行の行頭で終わる範囲はその行に引かない() {
        let d = [diag(Severity::Error, (0, 0), (1, 0))];
        assert_eq!(line_spans(&d, 0, "fn a(")[0].range, 0..5);
        assert!(line_spans(&d, 1, "next").is_empty());
    }

    #[test]
    fn 行長を超える終点は行末へ丸める() {
        let d = [diag(Severity::Error, (0, 2), (0, 99))];
        assert_eq!(line_spans(&d, 0, "abcd")[0].range, 2..4);
    }

    #[test]
    fn 幅0は1文字ぶんへ広げ_行末なら行末の外へ引く() {
        let mid = [diag(Severity::Error, (0, 1), (0, 1))];
        assert_eq!(
            line_spans(&mid, 0, "a日b")[0].range,
            1..4,
            "多バイト文字 1 つぶん"
        );
        let eol = [diag(Severity::Error, (0, 3), (0, 3))];
        let span = &line_spans(&eol, 0, "abc")[0];
        assert_eq!((span.range.clone(), span.past_end), (3..4, true));
        // 空行でも引ける（行末の外の 1 文字ぶん）
        let empty = [diag(Severity::Hint, (0, 0), (0, 0))];
        assert!(line_spans(&empty, 0, "")[0].past_end);
    }

    #[test]
    fn 文字の途中を指す位置は手前の境界へ寄せる() {
        // 「日」は 3 バイト。2 は文字の途中
        let d = [diag(Severity::Error, (0, 2), (0, 5))];
        assert_eq!(line_spans(&d, 0, "日本")[0].range, 0..3);
    }

    #[test]
    fn 重なったら重い色が最後に来る() {
        let d = [
            diag(Severity::Error, (0, 0), (0, 3)),
            diag(Severity::Hint, (0, 1), (0, 2)),
            diag(Severity::Warning, (0, 0), (0, 2)),
        ];
        let order: Vec<Severity> = line_spans(&d, 0, "abc")
            .into_iter()
            .map(|s| s.severity)
            .collect();
        assert_eq!(
            order,
            vec![Severity::Hint, Severity::Warning, Severity::Error]
        );
    }
}

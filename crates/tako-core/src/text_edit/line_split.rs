//! 全文を 1 つの書き換えで返す整形を、行ごとの小さな置き換えへ割る（Issue #1899）。
//!
//! 言語サーバのなかには、整形の答えを行単位の差分ではなく「文書全体を 1 つの `TextEdit` で
//! 置き換える」形で返すものがある。[`super::order_changes`] の最小化（同じ頭と尻を削る）だけでは
//! 最初に変わった所から最後に変わった所までの 1 か所が残り、10 万行の先頭と末尾を直す答えでも
//! undo へ本文 2 本ぶん（10 MB の文書なら約 20 MB = 予算 8 MB を超えて、その前の履歴が消える）を積む。
//!
//! ここでは改行をまたぐ書き換えを `text_diff::changed_regions` で行単位に突き合わせ、
//! 行ごとの置き換えへ割る。突き合わせの鍵は**行の前後の空白を除いた中身**（整形の書き換えの
//! ほとんどは字下げ・行末の空白 = 中身の同じ行どうしを対にすれば数バイトの差で済む）。
//! 中身の違う行が続く区間は頭から 1 行ずつ対にし、余った行は 1 か所で消す / 足す。
//!
//! 割った後の本文は割る前と**バイト一致**する（どの置き換えも置換後の本文を順に並べた形 =
//! つなげば元の 1 つの書き換えと同じ）。割ると書き換えの中のカーソルも同じ行の同じ中身に
//! 留まる（割らないと書き換えの頭からの距離を保つだけなので、上の行が縮むと下の行へずれる）。
//!
//! 割ると箇所ごとに表（[`SPAN_COST`]）が増えるので、隣とのあいだ（変わらない本文）が表の半分より
//! 短い置き換えはまとめ（[`coalesce`]）、それでも**割って undo の履歴が軽くならなければ割らない**
//! （中身の違う短い行ばかりの書き換えや、もともと細かい差分を返すサーバ = rust-analyzer の
//! 改行をまたぐ数文字の書き換えは割らないほうが軽い。実測は `.agent/plans/2026-09-lsp-s1.md` の 30 節）。
//! `TAKO_1899_LEGACY=1` で割らない（同一バイナリで A/B を取る入口）

use std::ops::Range;

use crate::text_diff::{changed_regions, LineRegion};

use super::{changed_span, DeltaSpan, TextChange};

/// 箇所を 1 つ増やすごとに undo の差分 1 件が持つ表の重さ（[`DeltaSpan`]。#1651 の予算の勘定と同じ）
const SPAN_COST: usize = std::mem::size_of::<DeltaSpan>();

/// `TAKO_1899_LEGACY=1` で **#1899 前の当て方**へ戻す（改行をまたぐ書き換えを割らずに当てる）
pub(super) fn legacy_1899() -> bool {
    matches!(
        std::env::var("TAKO_1899_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 並べて最小にした書き換え（`order_changes` の後）のうち、改行をまたぐものを行ごとの置き換えへ割る。
///
/// 並び（始まりの順・重ならない）と、当てた後の本文は変えない。範囲は `text`（書き換える前の
/// 本文）のバイト位置
pub(super) fn split_by_lines(text: &str, ordered: Vec<TextChange>) -> Vec<TextChange> {
    let mut out = Vec::with_capacity(ordered.len());
    for change in ordered {
        let before = &text[change.range.clone()];
        // 1 行の中の書き換えはそれ以上割れない
        if !before.contains('\n') || !change.text.contains('\n') {
            out.push(change);
            continue;
        }
        let pieces = coalesce(line_pieces(before, &change.text));
        let split_cost: usize = pieces
            .iter()
            .map(|(old, new)| old.len() + new.len() + SPAN_COST)
            .sum();
        let whole_cost = before.len() + change.text.len() + SPAN_COST;
        if pieces.len() <= 1 || split_cost >= whole_cost {
            out.push(change);
            continue;
        }
        let base = change.range.start;
        out.extend(pieces.into_iter().map(|(old, new)| TextChange {
            range: base + old.start..base + old.end,
            text: change.text[new].to_string(),
        }));
    }
    out
}

/// 隣の置き換えとのあいだ（変わらない本文）が短ければ 1 つにまとめる。
///
/// あいだの本文は置換前と置換後の両方に写るので、まとめると履歴が 2 × あいだ 増え、表が 1 つ
/// 減る = **あいだが表の半分より短ければまとめたほうが軽い**。決めはあいだごとに独立なので、
/// 前から貪欲にまとめれば最小になり、全部まとめた形（= 割らない 1 つの書き換え）より重くならない
/// （中身の違う行が続く区間は 1 か所へ戻り、字下げだけの行が続く所は行ごとに残る）
fn coalesce(pieces: Vec<(Range<usize>, Range<usize>)>) -> Vec<(Range<usize>, Range<usize>)> {
    let mut out: Vec<(Range<usize>, Range<usize>)> = Vec::with_capacity(pieces.len());
    for (old, new) in pieces {
        if let Some((last_old, last_new)) = out.last_mut() {
            // あいだは変わらない本文 = 置換前と置換後で同じ長さ
            let gap = old.start - last_old.end;
            debug_assert_eq!(gap, new.start - last_new.end, "あいだが両側で違う");
            if 2 * gap < SPAN_COST {
                last_old.end = old.end;
                last_new.end = new.end;
                continue;
            }
        }
        out.push((old, new));
    }
    out
}

/// `before` を `after` にする行ごとの置き換え（範囲はそれぞれ `before` / `after` の中のバイト位置）。
/// 始まりの順・重ならない。中身の同じ対（置き換えない）は含めない
fn line_pieces(before: &str, after: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let a: Vec<&str> = before.split_inclusive('\n').collect();
    let b: Vec<&str> = after.split_inclusive('\n').collect();
    // 突き合わせの鍵は前後の空白（改行を含む）を除いた中身
    let key_a: Vec<&str> = a.iter().map(|line| line.trim()).collect();
    let key_b: Vec<&str> = b.iter().map(|line| line.trim()).collect();
    let (at_a, at_b) = (line_offsets(&a), line_offsets(&b));
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let end = LineRegion {
        old: a.len()..a.len(),
        new: b.len()..b.len(),
    };
    for region in changed_regions(&key_a, &key_b)
        .into_iter()
        .chain(std::iter::once(end))
    {
        // 区間の手前は中身の同じ行（字下げ・行末の空白・改行だけが違いうる）: 頭の空白と
        // 尻の空白（改行を含む）を別々に置き換える（間の中身は写さない）
        while i < region.old.start {
            let (lead_a, lead_b) = (leading_space(a[i]), leading_space(b[j]));
            let body = key_a[i].len();
            let (head_a, head_b) = (at_a[i] + lead_a, at_b[j] + lead_b);
            push_piece(&mut out, before, after, at_a[i]..head_a, at_b[j]..head_b);
            push_piece(
                &mut out,
                before,
                after,
                head_a + body..at_a[i + 1],
                head_b + body..at_b[j + 1],
            );
            (i, j) = (i + 1, j + 1);
        }
        // 中身の違う行の区間: 頭から 1 行ずつ対にし、余った行は 1 か所で消す / 足す
        let paired = region.old.len().min(region.new.len());
        for k in 0..paired {
            let (old, new) = (i + k, j + k);
            push_piece(
                &mut out,
                before,
                after,
                at_a[old]..at_a[old + 1],
                at_b[new]..at_b[new + 1],
            );
        }
        let (old, new) = (i + paired, j + paired);
        push_piece(
            &mut out,
            before,
            after,
            at_a[old]..at_a[region.old.end],
            at_b[new]..at_b[region.new.end],
        );
        (i, j) = (region.old.end, region.new.end);
    }
    out
}

/// `before[old]` を `after[new]` へ置き換える 1 か所を足す（同じ頭と尻を削る。同じなら足さない）
fn push_piece(
    out: &mut Vec<(Range<usize>, Range<usize>)>,
    before: &str,
    after: &str,
    old: Range<usize>,
    new: Range<usize>,
) {
    let (span, inserted) = changed_span(&before[old.clone()], &after[new.clone()]);
    if span.is_empty() && inserted.is_empty() {
        return;
    }
    // 削った頭の長さは両側で同じ
    let from = new.start + span.start;
    out.push((
        old.start + span.start..old.start + span.end,
        from..from + inserted.len(),
    ));
}

/// 行頭の空白のバイト数（行が空白だけなら改行まで含めて全部）
fn leading_space(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// 各行の頭のバイト位置と、末尾に全長（`offsets[k]..offsets[k + 1]` が k 行目）
fn line_offsets(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::with_capacity(lines.len() + 1);
    let mut at = 0;
    out.push(at);
    for line in lines {
        at += line.len();
        out.push(at);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 割った置き換えを `before` へ当てた本文（後ろから当てる = 前の位置は動かない）
    fn apply(before: &str, after: &str, pieces: &[(Range<usize>, Range<usize>)]) -> String {
        let mut text = before.to_string();
        for (old, new) in pieces.iter().rev() {
            text.replace_range(old.clone(), &after[new.clone()]);
        }
        text
    }

    fn assert_ordered(pieces: &[(Range<usize>, Range<usize>)]) {
        for pair in pieces.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start, "重なり: {pieces:?}");
            assert!(pair[0].1.end <= pair[1].1.start, "新の側の順: {pieces:?}");
        }
    }

    #[test]
    fn 字下げと行末の空白だけの直しは行ごとの数バイトになる() {
        let before = "fn a() {\nlet x = 1;  \nif x {\nx\n}\n}\n";
        let after = "fn a() {\n    let x = 1;\n    if x {\n        x\n    }\n}\n";
        let pieces = line_pieces(before, after);
        assert_ordered(&pieces);
        assert_eq!(apply(before, after, &pieces), after);
        // 2 行目は頭へ 4 つ足して行末の 2 つを消す・3〜5 行目は頭へ 4 / 8 / 4 つ足す
        // （行の中身は写さない）
        let total: usize = pieces.iter().map(|(o, n)| o.len() + n.len()).sum();
        assert_eq!(total, 4 + 2 + 4 + 8 + 4, "{pieces:?}");
        assert_eq!(pieces.len(), 5);
    }

    #[test]
    fn 行を割る直しと足す直しも往復でバイト一致する() {
        let before = "fn main(){\nlet v = foo(aaaa, bbbb, cccc);\nlet y=2;\n}\n";
        let after = "fn main() {\n    let v = foo(\n        aaaa,\n        bbbb,\n        cccc,\n    );\n    let y = 2;\n}\n";
        let pieces = line_pieces(before, after);
        assert_ordered(&pieces);
        assert_eq!(apply(before, after, &pieces), after);
    }

    /// 割って undo の履歴が軽くなるときだけ割る: 中身の違う短い行ばかりは割らない
    /// （1 行 1 + 1 バイト + 表 24 バイト × 5 万 = 1,300,000 > 割らない 700,024）・
    /// 行末の空白だけの直しは割る（1 行 2 + 24 バイト × 5 万 = 1,300,000 < 割らない 1,700,024）
    #[test]
    fn 割って軽くなるときだけ割る() {
        let lines = 50_000;
        let text: String = (0..lines).map(|i| format!("a{i:05}\n")).collect();
        let after: String = (0..lines).map(|i| format!("b{i:05}\n")).collect();
        let change = TextChange {
            range: 0..text.len(),
            text: after,
        };
        assert_eq!(split_by_lines(&text, vec![change.clone()]), vec![change]);
        // 小さくても同じ: 割ると 3 × (2 + 24) = 78 > 割らない 9 + 9 + 24 = 42
        let change = TextChange {
            range: 0..9,
            text: "x2\ny2\nz2\n".into(),
        };
        assert_eq!(
            split_by_lines("a1\nb1\nc1\n", vec![change.clone()]),
            vec![change]
        );

        let text: String = (0..lines)
            .map(|i| format!("let v{i:05} = 1;  \n"))
            .collect();
        let after: String = (0..lines).map(|i| format!("let v{i:05} = 1;\n")).collect();
        let change = TextChange {
            range: 0..text.len(),
            text: after,
        };
        let split = split_by_lines(&text, vec![change]);
        assert_eq!(split.len(), lines);
        assert!(split
            .iter()
            .all(|c| c.range.len() == 2 && c.text.is_empty()));
    }

    /// 字下げだけを直す長い行は行ごとに残り、中身の違う行が続く区間（あいだは改行 1 つ）は
    /// 1 か所へまとまる。当てた本文は変わらない
    #[test]
    fn あいだの短い置き換えはまとめ長い行の字下げは行ごとに残す() {
        let before = "a1\nb1\nc1\nlet long_line_one = 1;\nlet long_line_two = 2;\n";
        let after = "x2\ny2\nz2\n    let long_line_one = 1;\n    let long_line_two = 2;\n";
        let pieces = coalesce(line_pieces(before, after));
        assert_ordered(&pieces);
        assert_eq!(apply(before, after, &pieces), after);
        let shown: Vec<(&str, &str)> = pieces
            .iter()
            .map(|(o, n)| (&before[o.clone()], &after[n.clone()]))
            .collect();
        // 先頭 3 行（中身が違う・あいだは改行 1 つ）は 1 か所。4 行目の字下げも 3 行目とのあいだが
        // 改行 1 つなのでそこへまとまり、5 行目の字下げはあいだ（4 行目の中身）が長いので別の 1 か所
        assert_eq!(
            shown,
            vec![("a1\nb1\nc1\n", "x2\ny2\nz2\n    "), ("", "    ")],
            "{pieces:?}"
        );
    }

    #[test]
    fn 片側に改行の無い書き換えと空の書き換えはそのまま() {
        let text = "abc\ndef\n";
        let changes = vec![
            TextChange {
                range: 0..2,
                text: "x\ny\n".into(),
            },
            TextChange {
                range: 4..4,
                text: String::new(),
            },
        ];
        assert_eq!(split_by_lines(text, changes.clone()), changes);
    }
}

//! 2 つの本文の行単位の差分（Issue #1659）。
//!
//! 編集中のバッファとディスク上のファイルを突き合わせ、「上書き保存すると何が変わるか」を
//! unified diff の形で見せるために使う（GUI の競合の帯・`tako edit diff`・MCP
//! `tako_preview_save` の `action=diff` が同じここを通る）。
//!
//! 依存を増やさないよう自前で持つ。仕組みは **共通の先頭・末尾の行を削ってから、残った
//! 中央だけを Myers の O(ND) で比べる**。外部変更は局所的なことがほとんどなので、
//! 10 万行のファイルでも比べるのは変わった数行の周りだけで済む。編集距離が
//! [`MAX_EDIT_DISTANCE`] を超える（ファイル全体を整形し直した等）ときは最小の差分を
//! 諦め、中央を「消して足した」1 塊にする（差分としては正しいまま = `approximate`）。
//!
//! 行は改行コードまで含めて比べる（CRLF → LF の書き換えも差分として出る）。
//! 表示する行（[`DiffLine::content`]）からは改行コードを落とす

use crate::git::{DiffHunk, DiffLine, DiffLineKind};

/// 変更の前後に添える文脈の行数（git の既定と同じ）
pub const CONTEXT_LINES: usize = 3;

/// Myers で探す編集距離の上限。これを超える差は中央を丸ごと 1 塊にする。
///
/// 経路の復元に距離の 2 乗ぶんの記録を持つので、上限は記憶量で決めた
/// （1,000 で `i32` 約 100 万個 = 4 MB）。ふつうの競合（数行〜数十行の食い違い）は届かない
pub const MAX_EDIT_DISTANCE: usize = 1_000;

/// 返す差分の行数の上限（ハンクの見出しを除く）。超えたぶんは落として `truncated` を立てる。
///
/// 10 MB のファイルが丸ごと違うと差分も 20 MB になり、応答（JSON）と画面の両方が詰まる
pub const MAX_OUTPUT_LINES: usize = 5_000;

/// 行単位の差分
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextDiff {
    pub hunks: Vec<DiffHunk>,
    /// 足された行の数（`truncated` でも全体を数える）
    pub added: usize,
    /// 消された行の数（`truncated` でも全体を数える）
    pub removed: usize,
    /// [`MAX_OUTPUT_LINES`] で打ち切ったか
    pub truncated: bool,
    /// 編集距離の上限に当たり、最小でない（中央を丸ごと置き換えた）差分へ落としたか
    pub approximate: bool,
}

impl TextDiff {
    /// 差が無いか
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    /// unified diff の文字列（`---` / `+++` の見出しつき）。差が無ければ空文字
    pub fn unified(&self, old_label: &str, new_label: &str) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = format!("--- {old_label}\n+++ {new_label}\n");
        for hunk in &self.hunks {
            out.push_str(&hunk.header);
            out.push('\n');
            for line in &hunk.lines {
                out.push(match line.kind {
                    DiffLineKind::Context => ' ',
                    DiffLineKind::Add => '+',
                    DiffLineKind::Remove => '-',
                });
                out.push_str(&line.content);
                out.push('\n');
            }
        }
        if self.truncated {
            out.push_str(&format!(
                "（差分が {MAX_OUTPUT_LINES} 行を超えたので以降を省略した）\n"
            ));
        }
        out
    }
}

/// 1 行ぶんの編集操作。添字は比べる範囲の中の行番号（0 始まり）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal { old: usize, new: usize },
    Delete { old: usize },
    Insert { new: usize },
}

/// `old` から `new` への行単位の差分
pub fn diff_lines(old: &str, new: &str) -> TextDiff {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    // 共通の先頭・末尾を削る（外部変更は局所的なので、ここで大半が消える）
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let (mid_ops, approximate) = match myers(a_mid, b_mid, MAX_EDIT_DISTANCE) {
        Some(ops) => (ops, false),
        None => (replace_all(a_mid.len(), b_mid.len()), true),
    };

    // 文脈に使う分だけ先頭・末尾の共通行を戻す（10 万行の共通部分を操作列に展開しない）
    let lead = prefix.min(CONTEXT_LINES);
    let trail = suffix.min(CONTEXT_LINES);
    let base_old = prefix - lead;
    let base_new = prefix - lead;
    let mut ops: Vec<Op> = Vec::with_capacity(lead + mid_ops.len() + trail);
    ops.extend((0..lead).map(|i| Op::Equal {
        old: base_old + i,
        new: base_new + i,
    }));
    ops.extend(mid_ops.into_iter().map(|op| match op {
        Op::Equal { old, new } => Op::Equal {
            old: old + prefix,
            new: new + prefix,
        },
        Op::Delete { old } => Op::Delete { old: old + prefix },
        Op::Insert { new } => Op::Insert { new: new + prefix },
    }));
    let (tail_old, tail_new) = (a.len() - suffix, b.len() - suffix);
    ops.extend((0..trail).map(|i| Op::Equal {
        old: tail_old + i,
        new: tail_new + i,
    }));

    let mut diff = TextDiff {
        approximate,
        ..TextDiff::default()
    };
    for op in &ops {
        match op {
            Op::Delete { .. } => diff.removed += 1,
            Op::Insert { .. } => diff.added += 1,
            Op::Equal { .. } => {}
        }
    }
    build_hunks(&ops, &a, &b, &mut diff);
    diff
}

/// 中央を丸ごと「全部消して全部足す」操作列（編集距離の上限に当たったときの劣化）
fn replace_all(old_len: usize, new_len: usize) -> Vec<Op> {
    (0..old_len)
        .map(|old| Op::Delete { old })
        .chain((0..new_len).map(|new| Op::Insert { new }))
        .collect()
}

/// Myers の O(ND) 差分。編集距離が `max_d` を超えたら `None`。
///
/// 経路の復元のため、各距離 `d` の探索を始める前の「k 線ごとの到達 x」を
/// `[-d-1, d+1]` の範囲だけ記録する（配列全体を毎回写すと距離の 2 乗 × 全長になる）
fn myers(a: &[&str], b: &[&str], max_d: usize) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = (a.len() + b.len()).min(max_d) as isize;
    // k は -limit-1..=limit+1 を取る
    let offset = limit + 1;
    let mut v = vec![0i32; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<i32>> = Vec::new();
    let idx = |k: isize| (k + offset) as usize;
    let mut found = None;
    'search: for d in 0..=limit {
        trace.push(v[idx(-d - 1)..=idx(d + 1)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                v[idx(k + 1)] as isize
            } else {
                v[idx(k - 1)] as isize + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x as i32;
            if x >= n && y >= m {
                found = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    let found = found?;

    // 終点から始点へ辿り戻す
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=found).rev() {
        let saved = &trace[d as usize];
        // saved[0] が k = -d-1
        let at = |k: isize| saved[(k + d + 1) as usize] as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        // 距離 0 の手前は始点 (0, 0)（そこまでは斜め = 共通行だけ）
        let (prev_x, prev_y) = if d == 0 {
            (0, 0)
        } else {
            let prev_x = at(prev_k);
            (prev_x, prev_x - prev_k)
        };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(Op::Equal {
                old: x as usize,
                new: y as usize,
            });
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert {
                    new: prev_y as usize,
                });
            } else {
                ops.push(Op::Delete {
                    old: prev_x as usize,
                });
            }
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    Some(ops)
}

/// 表示する行（改行コードを落とす）
fn display(line: &str) -> String {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line).to_string()
}

/// 操作列を文脈つきのハンクへまとめる（git と同じく、間の共通行が文脈 2 つ分以下なら 1 つにする）
fn build_hunks(ops: &[Op], a: &[&str], b: &[&str], diff: &mut TextDiff) {
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Equal { .. }))
        .map(|(i, _)| i)
        .collect();
    let mut emitted = 0usize;
    let mut i = 0;
    while i < changes.len() {
        let first = changes[i];
        let mut last = first;
        while i + 1 < changes.len() && changes[i + 1] - last <= 2 * CONTEXT_LINES + 1 {
            i += 1;
            last = changes[i];
        }
        i += 1;
        let from = first.saturating_sub(CONTEXT_LINES);
        let to = (last + CONTEXT_LINES + 1).min(ops.len());
        let span = &ops[from..to];

        // 見出しの行番号は、ハンクの最初の行が旧・新それぞれで何行目か
        let old_start = span.iter().find_map(|op| match op {
            Op::Equal { old, .. } | Op::Delete { old } => Some(*old),
            Op::Insert { .. } => None,
        });
        let new_start = span.iter().find_map(|op| match op {
            Op::Equal { new, .. } | Op::Insert { new } => Some(*new),
            Op::Delete { .. } => None,
        });
        let old_count = span
            .iter()
            .filter(|op| !matches!(op, Op::Insert { .. }))
            .count();
        let new_count = span
            .iter()
            .filter(|op| !matches!(op, Op::Delete { .. }))
            .count();
        let header = format!(
            "@@ -{} +{} @@",
            range_label(old_start, old_count),
            range_label(new_start, new_count)
        );

        let mut lines = Vec::with_capacity(span.len());
        for op in span {
            if emitted >= MAX_OUTPUT_LINES {
                diff.truncated = true;
                break;
            }
            let (kind, content) = match *op {
                Op::Equal { old, .. } => (DiffLineKind::Context, display(a[old])),
                Op::Delete { old } => (DiffLineKind::Remove, display(a[old])),
                Op::Insert { new } => (DiffLineKind::Add, display(b[new])),
            };
            lines.push(DiffLine { kind, content });
            emitted += 1;
        }
        if !lines.is_empty() {
            diff.hunks.push(DiffHunk { header, lines });
        }
        if diff.truncated {
            break;
        }
    }
}

/// ハンク見出しの片側（`12,4` / 1 行なら `12` / 0 行なら `0,0`）。git と同じ書き方。
///
/// 片側に行が 1 つも無いハンクは、文脈行も持たない（持てば両側に行がある）。
/// 文脈を [`CONTEXT_LINES`] 行添えるので、それが起きるのは**その側の本文が空**のときだけ
/// = 位置は常に 0（下の `const` の表明がこの前提を固定する）
fn range_label(start: Option<usize>, count: usize) -> String {
    match (start, count) {
        (Some(start), 1) => format!("{}", start + 1),
        (Some(start), count) => format!("{},{count}", start + 1),
        (None, _) => "0,0".to_string(),
    }
}

// 文脈を 0 行にすると「片側 0 行のハンク = その側が空」の前提が崩れる（`range_label`）
const _: () = assert!(CONTEXT_LINES > 0);

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &TextDiff) -> Vec<String> {
        diff.hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .map(|l| {
                let c = match l.kind {
                    DiffLineKind::Context => ' ',
                    DiffLineKind::Add => '+',
                    DiffLineKind::Remove => '-',
                };
                format!("{c}{}", l.content)
            })
            .collect()
    }

    /// 旧本文へ差分を当てると新本文になるか（往復の検査。見出しの旧側の位置と行の中身を使う）
    fn apply(old: &str, diff: &TextDiff) -> Vec<String> {
        let a: Vec<String> = old.split_inclusive('\n').map(display).collect();
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for hunk in &diff.hunks {
            // "@@ -12,4 +12,5 @@" の旧側 = "12,4"
            let old_side = hunk.header[4..].split(' ').next().unwrap();
            let mut parts = old_side.split(',');
            let start: usize = parts.next().unwrap().parse().unwrap();
            let count: usize = parts.next().map_or(1, |c| c.parse().unwrap());
            // 0 行の側は「その行の後ろ」、それ以外は 1 始まりの行番号
            let at = if count == 0 { start } else { start - 1 };
            out.extend(a[cursor..at].iter().cloned());
            cursor = at;
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Context => {
                        assert_eq!(a[cursor], line.content, "文脈行が旧本文と一致する");
                        out.push(a[cursor].clone());
                        cursor += 1;
                    }
                    DiffLineKind::Remove => {
                        assert_eq!(a[cursor], line.content, "消す行が旧本文と一致する");
                        cursor += 1;
                    }
                    DiffLineKind::Add => out.push(line.content.clone()),
                }
            }
        }
        out.extend(a[cursor..].iter().cloned());
        out
    }

    #[test]
    fn 同じ本文は差が無い() {
        let diff = diff_lines("a\nb\n", "a\nb\n");
        assert!(diff.is_empty());
        assert!(diff.hunks.is_empty());
        assert_eq!(diff.unified("disk", "buffer"), "");
    }

    #[test]
    fn 一行の書き換えは文脈つきの一塊() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\nX\n6\n7\n8\n9\n";
        let diff = diff_lines(old, new);
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].header, "@@ -2,7 +2,7 @@");
        assert_eq!(
            kinds(&diff),
            vec![" 2", " 3", " 4", "-5", "+X", " 6", " 7", " 8"]
        );
        assert!(!diff.approximate);
    }

    #[test]
    fn 離れた変更は別のハンクになる() {
        let mut old = String::new();
        for i in 0..40 {
            old.push_str(&format!("{i}\n"));
        }
        let new = old
            .replace("\n5\n", "\nfive\n")
            .replace("\n30\n", "\nthirty\n");
        let diff = diff_lines(&old, &new);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.hunks[0].header, "@@ -3,7 +3,7 @@");
        assert_eq!(diff.hunks[1].header, "@@ -28,7 +28,7 @@");
        let expected: Vec<String> = new.lines().map(str::to_string).collect();
        assert_eq!(apply(&old, &diff), expected);
    }

    #[test]
    fn 末尾への追加と先頭の削除() {
        let diff = diff_lines("a\nb\nc\n", "b\nc\nd\n");
        assert_eq!(kinds(&diff), vec!["-a", " b", " c", "+d"]);
        assert_eq!(diff.hunks[0].header, "@@ -1,3 +1,3 @@");
    }

    #[test]
    fn 空からの差分と空への差分() {
        let diff = diff_lines("", "a\nb\n");
        assert_eq!(kinds(&diff), vec!["+a", "+b"]);
        assert_eq!(diff.hunks[0].header, "@@ -0,0 +1,2 @@");
        let diff = diff_lines("a\n", "");
        assert_eq!(kinds(&diff), vec!["-a"]);
        assert_eq!(diff.hunks[0].header, "@@ -1 +0,0 @@");
    }

    #[test]
    fn 改行コードだけの違いも差として出す() {
        let diff = diff_lines("a\r\nb\r\n", "a\nb\n");
        assert_eq!((diff.added, diff.removed), (2, 2));
        // 表示からは改行コードを落とす
        assert!(kinds(&diff).iter().all(|l| !l.contains('\r')));
    }

    #[test]
    fn 共通の先頭末尾が長くても文脈は3行だけ() {
        let mut old = String::new();
        for i in 0..100_000 {
            old.push_str(&format!("line {i}\n"));
        }
        let new = old.replace("line 50000\n", "changed\n");
        let diff = diff_lines(&old, &new);
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].lines.len(), 8);
        assert_eq!(diff.hunks[0].header, "@@ -49998,7 +49998,7 @@");
    }

    #[test]
    fn 編集距離の上限を超えたら中央を丸ごと置き換える() {
        let old: String = (0..3_000).map(|i| format!("a{i}\n")).collect();
        let new: String = (0..3_000).map(|i| format!("b{i}\n")).collect();
        let diff = diff_lines(&old, &new);
        assert!(diff.approximate);
        assert_eq!((diff.added, diff.removed), (3_000, 3_000));
        // 行数の上限で打ち切っても数は全体を数える
        assert!(diff.truncated);
        let shown: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(shown, MAX_OUTPUT_LINES);
        assert!(diff
            .unified("disk", "buffer")
            .ends_with("以降を省略した）\n"));
    }

    #[test]
    fn ランダムな編集の往復は新本文に戻る() {
        // 疑似乱数（依存を足さない）
        let mut seed: u64 = 0x1659;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        for _ in 0..200 {
            let old: Vec<String> = (0..next(30)).map(|_| format!("{}", next(5))).collect();
            let mut new = old.clone();
            for _ in 0..next(6) {
                match next(3) {
                    0 if !new.is_empty() => {
                        let at = next(new.len());
                        new.remove(at);
                    }
                    1 => {
                        let at = next(new.len() + 1);
                        new.insert(at, format!("n{}", next(5)));
                    }
                    _ if !new.is_empty() => {
                        let at = next(new.len());
                        new[at] = format!("r{}", next(5));
                    }
                    _ => {}
                }
            }
            let old_text: String = old.iter().map(|l| format!("{l}\n")).collect();
            let new_text: String = new.iter().map(|l| format!("{l}\n")).collect();
            let diff = diff_lines(&old_text, &new_text);
            assert_eq!(apply(&old_text, &diff), new, "old={old:?} new={new:?}");
            assert!(!diff.approximate);
        }
    }
}

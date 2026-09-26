//! `didChange`（incremental）の中身を作る（#1678）
//!
//! サーバが持っている本文（= 最後に送った本文の写し）と今の本文を比べ、
//! **共通の頭と尻尾を除いた 1 範囲**を LSP の位置（UTF-16）で返す。
//!
//! `text_edit::EditDelta`（#1651）を直接写さないのは、同期の合間に複数の編集が
//! まとまる経路があるから（`replace_all`・外部変更の取り込み・送信キューが満杯で
//! 1 回送れなかったとき）。写しとの差分なら、何回ぶん溜まっても 1 件で正しく送れ、
//! サーバの本文と食い違う経路が構造的に無い。
//!
//! 全体が入れ替わったとき（頭も尻尾も共通しない）は範囲を省いた全文を返す
//! （外部変更での全文差し替え = `.agent/plans/2026-09-lsp-s1.md` §8）。

use super::position::lsp_position_of;

/// 1 件の変更。`range` が `None` なら全文の差し替え
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentChange {
    /// `((開始行, 開始桁), (終了行, 終了桁))`。位置は**変更前の本文**での LSP 座標
    pub range: Option<((usize, usize), (usize, usize))>,
    pub text: String,
}

/// `old` → `new` の変更。同じなら `None`
pub fn diff_change(old: &str, new: &str) -> Option<ContentChange> {
    if old == new {
        return None;
    }
    let (start, old_end, new_end) = changed_span(old, new);
    if start == 0 && old_end == old.len() {
        return Some(ContentChange {
            range: None,
            text: new.to_string(),
        });
    }
    Some(ContentChange {
        range: Some((lsp_position_of(old, start), lsp_position_of(old, old_end))),
        text: new[start..new_end].to_string(),
    })
}

/// 変わった範囲 `(開始, 変更前の終わり, 変更後の終わり)`（バイト位置）
fn changed_span(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    // `\r` と `\n` のあいだから始めない（LSP の位置では `\r` の手前と区別できない）
    if start > 0 && a[start - 1] == b'\r' && a.get(start) == Some(&b'\n') {
        start -= 1;
    }
    let max_suffix = (a.len() - start).min(b.len() - start);
    let mut suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    loop {
        let (old_end, new_end) = (a.len() - suffix, b.len() - suffix);
        let splits_crlf =
            old_end > start && a[old_end - 1] == b'\r' && a.get(old_end) == Some(&b'\n');
        if old.is_char_boundary(old_end) && new.is_char_boundary(new_end) && !splits_crlf {
            return (start, old_end, new_end);
        }
        suffix -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::position::byte_offset_of_lsp_position;

    /// サーバ側で範囲を当てたら新しい本文になる（LSP の適用を tako の変換で再現する）
    fn apply(old: &str, change: &ContentChange) -> String {
        match change.range {
            None => change.text.clone(),
            Some(((sl, sc), (el, ec))) => {
                let start = byte_offset_of_lsp_position(old, sl, sc);
                let end = byte_offset_of_lsp_position(old, el, ec);
                format!("{}{}{}", &old[..start], change.text, &old[end..])
            }
        }
    }

    #[test]
    fn 同じなら何も送らない() {
        assert_eq!(diff_change("abc", "abc"), None);
    }

    #[test]
    fn 一文字の挿入は位置と文字だけ() {
        let change = diff_change("fn main() {}\n", "fn main() { }\n").unwrap();
        assert_eq!(
            change,
            ContentChange {
                range: Some(((0, 11), (0, 11))),
                text: " ".into(),
            }
        );
    }

    #[test]
    fn 日本語と絵文字の後ろの編集は_utf16_の桁で送る() {
        let old = "// 日本😀\nlet x = 1;\n";
        let new = "// 日本😀語\nlet x = 1;\n";
        let change = diff_change(old, new).unwrap();
        // `// ` = 3 桁、日本 = 2 桁、😀 = 2 桁
        assert_eq!(change.range, Some(((0, 7), (0, 7))));
        assert_eq!(change.text, "語");
        assert_eq!(apply(old, &change), new);
    }

    #[test]
    fn 共通部分が文字の途中で切れても境界へ寄せる() {
        // 「あ」E3 81 82 と「ぃ」E3 81 83 は先頭 2 バイトが同じ
        let change = diff_change("xあy", "xぃy").unwrap();
        assert_eq!(change.range, Some(((0, 1), (0, 2))));
        assert_eq!(change.text, "ぃ");
    }

    #[test]
    fn 全体が入れ替わったら全文を送る() {
        let change = diff_change("abc", "xyz").unwrap();
        assert_eq!(change.range, None);
        assert_eq!(change.text, "xyz");
    }

    #[test]
    fn 空から書き始めても空へ消しても当てられる() {
        for (old, new) in [("", "abc\n"), ("abc\n", ""), ("a", "")] {
            let change = diff_change(old, new).unwrap();
            assert_eq!(apply(old, &change), new, "{old:?} → {new:?}");
        }
    }

    #[test]
    fn crlf_の途中から始まる変更でも当てると一致する() {
        let cases = [
            ("a\r\nb", "a\r\r\nb"),
            ("a\r\nb", "a\rb"),
            ("a\r\nb\r\n", "a\r\nX\r\nb\r\n"),
            ("x\r\n", "x\n"),
            ("x\n", "x\r\n"),
        ];
        for (old, new) in cases {
            let change = diff_change(old, new).unwrap();
            assert_eq!(apply(old, &change), new, "{old:?} → {new:?} / {change:?}");
        }
    }

    /// 同期の合間に複数の編集が溜まっても、1 件で正しく当たる
    #[test]
    fn 溜まった複数の編集も一件で当たる() {
        let old = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let new = "fn a() { 1 }\nfn b() {}\nfn cc() {}\n";
        let change = diff_change(old, new).unwrap();
        assert_eq!(apply(old, &change), new);
        assert!(change.range.is_some(), "頭が共通なので範囲で送る");
    }
}

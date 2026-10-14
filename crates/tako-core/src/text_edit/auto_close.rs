//! 括弧の自動閉じ（Issue #1759 / エピック #1647 の B レーン）。純粋関数と小さな状態だけで I/O をしない
//!
//! 開き括弧（`(` `[` `{`）を打つと閉じ括弧も入れ、カーソルは間に置く。入れた閉じ括弧の
//! 手前で同じ閉じ括弧を打つと、二重にせず**飛び越す**（type-over）。`(|)` の間の ⌫ は
//! 対で消す。#1654 が自動閉じを見送った理由（`.agent/requirements.md` FR-3.5）への答えは次の 4 つ:
//!
//! - **飛び越すのは自分で入れた閉じ括弧だけ**（[`AutoPairs`]。VS Code の
//!   `autoClosingOvertype: "auto"` と同じ）。手で打った `)` の手前で `)` を打てば普通に入る =
//!   数が狂わない。カーソルが対の外へ出たら追跡をやめる
//! - **文字列・コメントの中では閉じない**。見分け方は対応括弧の強調（#1691）の走査
//!   （[`super::brackets::opens_in_code`]）と同じ 1 実装で、記号は言語の同定の正本
//!   `file_type` の `comment` 列と `lexical` 列が決める（Rust の `'('` / Python の `'(…'`）
//! - **次の文字が空白・行末・閉じ記号のときだけ閉じる**（[`closes_before`]。VS Code の
//!   `autoCloseBefore` の既定と同じ）。`foo|bar` で `(` を打っても `()bar` にしない
//! - **形式と設定で切れる**。形式は `file_type` の `auto_close` 列（文書 = Markdown・
//!   プレーンテキストは閉じない）、設定は settings.json の `editor_auto_close`（既定 ON）で、
//!   どちらかが OFF なら #1654 と同じく 1 文字を入れるだけ（飛び越しも対の削除もしない）
//!
//! 打鍵 1 文字の口は [`super::TextBuffer::type_char`] の 1 つで、GUI の文字入力と CLI
//! `tako edit type` / MCP `tako_preview_edit` の `command: "type"` がここを通る

use std::ops::Range;

use super::Splice;

/// 自動で閉じてよい「カーソルの直後の文字」（#1759）。
///
/// VS Code の `editor.autoCloseBefore` の既定（`;:.,=}])>` + 空白）と同じ。
/// 識別子の手前（`foo|bar`）・開き括弧の手前・引用符の手前では閉じない
const CLOSE_BEFORE: &str = ";:.,=}])>";

/// カーソルの直後が `next`（`None` = 文書の終わり）のとき閉じてよいか
pub(super) fn closes_before(next: Option<char>) -> bool {
    next.is_none_or(|c| c.is_whitespace() || CLOSE_BEFORE.contains(c))
}

/// 自動閉じの対象の開き括弧と、その閉じ括弧
pub(super) fn closing_for(open: char) -> Option<char> {
    super::closing_bracket(open)
}

/// 閉じ括弧か（飛び越しの対象）
pub(super) fn is_closing(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

/// 自動で入れた閉じ括弧の追跡（#1759）。
///
/// 1 件 = (開き括弧の位置, 閉じ括弧の位置)。どちらも 1 バイトの ASCII なのでバイト位置で持つ。
/// 本文が変わるたびに [`Self::follow`] で位置を追従させ、対のどちらかに触れた編集が来たら
/// その件は捨てる。undo / redo は位置の対応が取れないので全部捨てる
#[derive(Debug, Clone, Default)]
pub(super) struct AutoPairs {
    pairs: Vec<(usize, usize)>,
}

impl AutoPairs {
    pub(super) fn push(&mut self, open: usize, close: usize) {
        self.pairs.push((open, close));
    }

    pub(super) fn clear(&mut self) {
        self.pairs.clear();
    }

    /// カーソルが対の内側（開き括弧の後ろ〜閉じ括弧の手前）に無い件を捨てる。
    ///
    /// VS Code は対の外へ出た時点で追跡をやめる（戻ってきても飛び越さない）。
    /// 呼ぶのはカーソルが実際に動いたとき（`set_cursor` / `set_selection`）と打鍵の前
    pub(super) fn retain_around(&mut self, cursor: usize) {
        self.pairs
            .retain(|(open, close)| *open < cursor && cursor <= *close);
    }

    /// `at` にある閉じ括弧が自動で入れたものなら、その件を外して `true`（飛び越す）
    pub(super) fn take_closing_at(&mut self, at: usize) -> bool {
        match self.pairs.iter().position(|(_, close)| *close == at) {
            Some(ix) => {
                self.pairs.remove(ix);
                true
            }
            None => false,
        }
    }

    /// カーソルが `(|)` の間にある件（開き括弧の直後 = 閉じ括弧の手前）
    pub(super) fn pair_around(&self, cursor: usize) -> Option<Range<usize>> {
        self.pairs
            .iter()
            .find(|(open, close)| open + 1 == cursor && *close == cursor)
            .map(|(open, close)| *open..*close + 1)
    }

    /// 本文の置き換え（置き換える前の位置。始まりの順・重ならない）に位置を追従させる。
    /// 括弧の 1 バイトに触れた置き換えがあればその件は捨てる
    pub(super) fn follow(&mut self, splices: &[Splice<'_>]) {
        if self.pairs.is_empty() {
            return;
        }
        self.pairs.retain_mut(|(open, close)| {
            match (map_byte(*open, splices), map_byte(*close, splices)) {
                (Some(o), Some(c)) => {
                    (*open, *close) = (o, c);
                    true
                }
                _ => false,
            }
        });
    }
}

/// 位置 `at` の 1 バイトが置き換え後にどこへ行くか（置き換えに飲まれたら `None`）。
///
/// 手前（終わりが `at` 以前。`at` ちょうどへの挿入も含む = 括弧は後ろへずれる）の置き換えだけ
/// 長さの差を足す。後ろ（始まりが `at + 1` 以降）は関係ない
fn map_byte(at: usize, splices: &[Splice<'_>]) -> Option<usize> {
    let mut shifted = at as isize;
    for splice in splices {
        if splice.range.end <= at {
            shifted += splice.text.len() as isize - splice.range.len() as isize;
        } else if splice.range.start > at {
            break;
        } else {
            return None;
        }
    }
    usize::try_from(shifted).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 閉じてよい次の文字() {
        for next in [None, Some(' '), Some('\t'), Some('\n'), Some('\r')] {
            assert!(closes_before(next), "{next:?}");
        }
        for c in ";:.,=}])>".chars() {
            assert!(closes_before(Some(c)), "{c:?}");
        }
        // 識別子・開き括弧・引用符の手前では閉じない
        for c in ['a', 'Z', '_', '0', '(', '[', '{', '"', '\'', 'あ'] {
            assert!(!closes_before(Some(c)), "{c:?}");
        }
    }

    #[test]
    fn 置き換えに位置が追従し触れた件は捨てる() {
        let mut pairs = AutoPairs::default();
        // `f(x)` の ( = 1・) = 3
        pairs.push(1, 3);
        // 手前へ 2 バイト挿入 → 2 つずれる
        pairs.follow(&[Splice::new(0..0, "ab")]);
        assert_eq!(pairs.pairs, vec![(3, 5)]);
        // 間（x）を置き換えても括弧は残り、閉じ括弧だけずれる
        pairs.follow(&[Splice::new(4..5, "xyz")]);
        assert_eq!(pairs.pairs, vec![(3, 7)]);
        // 閉じ括弧の手前ちょうどへの挿入は閉じ括弧を後ろへ送る
        pairs.follow(&[Splice::new(7..7, "!")]);
        assert_eq!(pairs.pairs, vec![(3, 8)]);
        // 後ろの置き換えは関係ない
        pairs.follow(&[Splice::new(9..9, "tail")]);
        assert_eq!(pairs.pairs, vec![(3, 8)]);
        // 閉じ括弧を消す置き換えが来たら捨てる
        pairs.follow(&[Splice::new(8..9, "")]);
        assert!(pairs.pairs.is_empty());
        // 離れた複数の箇所（始まりの順）は手前のぶんだけ足す
        let mut pairs = AutoPairs::default();
        pairs.push(5, 6);
        pairs.follow(&[Splice::new(0..2, ""), Splice::new(10..10, "zz")]);
        assert_eq!(pairs.pairs, vec![(3, 4)]);
    }

    #[test]
    fn 対の内側を出たら追跡をやめる() {
        let mut pairs = AutoPairs::default();
        pairs.push(1, 3);
        pairs.retain_around(2);
        pairs.retain_around(3);
        assert_eq!(pairs.pairs.len(), 1);
        assert_eq!(pairs.pair_around(2), None);
        pairs.push(4, 5);
        assert_eq!(pairs.pair_around(5), Some(4..6));
        pairs.retain_around(1);
        assert!(pairs.pairs.is_empty());
        // 飛び越すと外れる
        pairs.push(1, 2);
        assert!(!pairs.take_closing_at(1));
        assert!(pairs.take_closing_at(2));
        assert!(pairs.pairs.is_empty());
    }
}

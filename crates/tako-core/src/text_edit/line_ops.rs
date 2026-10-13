//! 行単位の操作（Issue #1692 / エピック #1647 の SE-4）。
//!
//! 行の複製（⇧⌥↑↓）・移動（⌥↑↓）・行インデント / アンインデント（⌘] ⌘[）・
//! 行コメントの付け外し（⌘/）を [`TextBuffer`] の操作として持つ。打鍵の割り当ては
//! `platform::editor_keys` の表、CLI `tako edit line <操作>` と MCP `tako_preview_edit` の
//! `command`（`line-<操作>`）は [`LineOperation::name`] の綴りを引く = GUI と AI が同じ口を通る。
//!
//! ## 守っている契約
//!
//! - **1 操作 = 1 回の編集**（`apply_edit` を 1 回だけ呼ぶ）なので undo 1 回で元の全文に戻る
//!   （#1651。種類は `Replace` で、直前の打鍵の塊とはまとまらない）。何も変わらない操作
//!   （先頭行の上移動・末尾行の下移動・コメントの無い行のアンコメント）は本文も版も変えず、
//!   履歴も積まない
//! - **改行コードを混ぜない**（#1650）。移動は既存の改行を並べ替えるだけで新しい改行を作らない。
//!   複製で足す改行 1 つだけがこのファイルの多数派（[`LineEnding`](super::LineEnding)）になる
//! - 対象の行は「選択が触れる行（無ければカーソルの行）」。選択の終わりが行頭にあれば
//!   その行は含めない（行を丸ごと選ぶと終わりが次の行頭に来る）。Tab の行インデントと同じ規則で、
//!   決めるのは [`TextBuffer::line_block`] の 1 実装
//! - コメント記号は言語の同定の正本 `file_type`（#1948）の**行の `comment` 列**で決まる
//!   （[`comment_syntax_in`]。別の表は持たない）。ここのロジックは記号の文字列を 1 つも持たない
//!   （番犬 `issue1692_line_ops_watchdog` が名指す）

use std::ops::Range;
use std::path::Path;

use thiserror::Error;

use crate::file_type::{self, FileType};

use super::{
    byte_for_display_column, display_width, line_break_len_after, line_break_len_before, Edit,
    EditKind, Splice, TextBuffer,
};

/// 行単位の操作（#1692）。外（CLI / MCP）から指すときの綴りは [`Self::name`] の 1 か所
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOperation {
    /// 対象の行を複製し、カーソルは上の写し（元の位置）に残す（⇧⌥↑）
    DuplicateUp,
    /// 対象の行を複製し、カーソルは下の写しへ移る（⇧⌥↓）
    DuplicateDown,
    /// 対象の行を 1 行上へ（⌥↑）。先頭行なら何もしない
    MoveUp,
    /// 対象の行を 1 行下へ（⌥↓）。末尾行なら何もしない
    MoveDown,
    /// 対象の行を 1 段深く（⌘]）。選択が無くても**行ごと**深くする
    /// （Tab は選択が無ければカーソル位置へ 1 段ぶん挿す = 別の操作）
    Indent,
    /// 対象の行を 1 段浅く（⌘[。⇧Tab と同じ）
    Outdent,
    /// 行コメントを付ける（付いている行にも 1 段重ねる）
    Comment,
    /// 行コメントを外す（付いていない行は触らない）
    Uncomment,
    /// 空行を除く全行に付いていれば外し、そうでなければ付ける（⌘/）
    ToggleComment,
}

impl LineOperation {
    /// 全種類（名前の表・単体の網羅に使う）
    pub const ALL: [Self; 9] = [
        Self::DuplicateUp,
        Self::DuplicateDown,
        Self::MoveUp,
        Self::MoveDown,
        Self::Indent,
        Self::Outdent,
        Self::Comment,
        Self::Uncomment,
        Self::ToggleComment,
    ];

    /// 外（CLI `tako edit line <名前>`）から指すときの綴り
    pub fn name(self) -> &'static str {
        match self {
            Self::DuplicateUp => "duplicate-up",
            Self::DuplicateDown => "duplicate-down",
            Self::MoveUp => "move-up",
            Self::MoveDown => "move-down",
            Self::Indent => "indent",
            Self::Outdent => "outdent",
            Self::Comment => "comment",
            Self::Uncomment => "uncomment",
            Self::ToggleComment => "toggle-comment",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }

    /// 綴りの一覧（エラー文の候補）
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|op| op.name()).collect()
    }
}

/// 行操作を当てられない理由（#1692）。**本文は 1 バイトも変えずに**返す
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LineOpError {
    #[error("{name} のコメント記号を知らない（コメントの記号を持たない形式）")]
    NoCommentSyntax { name: String },
}

/// コメントの書き方（#1692）。言語ごとの値は言語の同定の正本（[`file_type::TABLE`]）の
/// `comment` 列が持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentSyntax {
    /// 行コメント。インデントの直後へ記号と空白 1 つを挿す
    Line(&'static str),
    /// 行コメントを持たない形式（HTML・CSS・Markdown）。**行ごとに**始まりと終わりで囲む
    /// （行単位の操作なので、複数行を 1 組で囲む形にはしない = 付け外しが行ごとに閉じる）
    Block {
        open: &'static str,
        close: &'static str,
    },
}

impl CommentSyntax {
    /// 行の中身（改行を除く）がこの書き方のコメントか
    fn is_commented(self, content: &str) -> bool {
        self.uncomment_cuts(content).is_some()
    }

    /// コメントを外すために消す範囲（行の中身の中のバイト位置。始まりの順・重ならない）。
    /// コメントでなければ `None`
    fn uncomment_cuts(self, content: &str) -> Option<Vec<Range<usize>>> {
        let ws = leading_ws(content).len();
        let rest = &content[ws..];
        match self {
            Self::Line(token) => {
                if !rest.starts_with(token) {
                    return None;
                }
                // 付けるときに挿した空白 1 つも一緒に外す（2 つ目以降は本文のインデント）
                let mut end = ws + token.len();
                if content[end..].starts_with(' ') {
                    end += 1;
                }
                let cut = ws..end;
                Some(vec![cut])
            }
            Self::Block { open, close } => {
                let trimmed = content.trim_end_matches([' ', '\t']).len();
                if !rest.starts_with(open)
                    || !content[..trimmed].ends_with(close)
                    || trimmed < ws + open.len() + close.len()
                {
                    return None;
                }
                let mut open_end = ws + open.len();
                let close_start = trimmed - close.len();
                if open_end < close_start && content[open_end..].starts_with(' ') {
                    open_end += 1;
                }
                let mut cut_from = close_start;
                if cut_from > open_end && content[..cut_from].ends_with(' ') {
                    cut_from -= 1;
                }
                Some(vec![ws..open_end, cut_from..trimmed])
            }
        }
    }
}

/// パスのコメント記号（言語の同定の正本 [`file_type::TABLE`] の `comment` 列を引く）。
/// 表に無い形式・記号を持たない形式は `None`
pub fn comment_syntax_for(path: &Path) -> Option<CommentSyntax> {
    comment_syntax_in(file_type::TABLE, path)
}

/// 渡した形式の表からパスのコメント記号を引く（同定は [`file_type::identify_in`] の 1 実装 =
/// ファイル名 → 拡張子の順・大文字小文字を問わない）。表を引数で受けるのは、単体が
/// 「表に 1 行足すだけで結果が変わる」を表のコピーで確かめるため
pub fn comment_syntax_in(table: &[FileType], path: &Path) -> Option<CommentSyntax> {
    file_type::identify_in(table, path).and_then(|row| row.comment)
}

/// 行頭の空白（半角スペースとタブだけ。インデントとして数えるもの）
fn leading_ws(content: &str) -> &str {
    &content[..content.len() - content.trim_start_matches([' ', '\t']).len()]
}

/// 置き換えの並び（始まりの順・重ならない）を当てた後、元の位置 `p` がどこへ移るか。
///
/// 置き換えより前はそのまま、後ろは長さの差だけずらす。消した範囲の中は範囲の頭へ寄せる。
/// 挿入とちょうど同じ位置の点は、`after_insert` なら挿入の後ろへ、そうでなければ手前に残す
/// （選択の始まりは手前 = 挿した記号も選択に入る。キャレットと選択の終わりは後ろ）
fn follow(splices: &[(Range<usize>, String)], p: usize, after_insert: bool) -> usize {
    let mut shift: isize = 0;
    for (range, text) in splices {
        let at_insert = range.is_empty() && p == range.start;
        if p < range.start || (p == range.start && !(at_insert && after_insert)) {
            break;
        }
        if range.is_empty() || p >= range.end {
            shift += text.len() as isize - range.len() as isize;
            continue;
        }
        let start = (range.start as isize + shift) as usize;
        return start + (p - range.start).min(text.len());
    }
    (p as isize + shift) as usize
}

impl TextBuffer {
    /// 行操作 1 つを当てる（#1692）。**GUI の打鍵と CLI / MCP が同じここを通る**
    /// （`EditorCommand::Line` → [`EditorCommand::apply`](crate::platform::editor_keys::EditorCommand::apply)）
    pub fn apply_line_operation(&mut self, op: LineOperation) -> Result<(), LineOpError> {
        let target = self.target_range();
        match op {
            LineOperation::DuplicateUp => self.duplicate_lines(false),
            LineOperation::DuplicateDown => self.duplicate_lines(true),
            LineOperation::MoveUp => self.move_lines(false),
            LineOperation::MoveDown => self.move_lines(true),
            LineOperation::Indent => self.shift_lines(target, true),
            LineOperation::Outdent => self.shift_lines(target, false),
            LineOperation::Comment | LineOperation::Uncomment | LineOperation::ToggleComment => {
                let syntax = self.comment_syntax();
                return self.comment_lines(op, syntax);
            }
        }
        Ok(())
    }

    /// このバッファのコメント記号（パスで表を引く。表に無い形式は `None`）
    pub fn comment_syntax(&self) -> Option<CommentSyntax> {
        comment_syntax_for(&self.path)
    }

    /// 操作の対象になる範囲（選択。無ければキャレットの位置）
    fn target_range(&self) -> Range<usize> {
        self.selection().unwrap_or(self.cursor..self.cursor)
    }

    /// `range` が触れる行の塊（最初の行の行頭 .. 最後の行の行末 = 改行の手前）。
    ///
    /// 選択の終わりが行頭（桁 0）にあるとき、その行は選んでいないものとして扱う
    /// （行を丸ごと選ぶと終わりが次の行頭に来る。VS Code / Sublime と同じ）。
    /// Tab / ⇧Tab の行インデント（#1654）と行操作（#1692）が同じこれを引く
    pub(super) fn line_block(&self, range: Range<usize>) -> Range<usize> {
        let first = self.line_start(range.start);
        let mut last_point = range.end;
        if range.end > range.start && self.line_start(range.end) == range.end {
            last_point -= line_break_len_before(&self.text, range.end);
        }
        first..self.line_end(last_point.max(first))
    }

    /// 行の塊を複製する。足すのは「改行 + 塊」を塊の後ろへ 1 つだけ
    /// （上へ / 下へで本文は同じになり、違うのはカーソルがどちらの写しに残るか）
    fn duplicate_lines(&mut self, down: bool) {
        let block = self.line_block(self.target_range());
        let eol = self.line_ending.as_str();
        let copy = format!("{eol}{}", &self.text[block.clone()]);
        let map = |p: usize| -> usize {
            if down {
                // 塊の中も、含めなかった次の行頭も、下の写しの同じ所へ
                p + copy.len()
            } else if p <= block.end {
                p
            } else {
                // 含めなかった次の行頭 = 下の写しの行頭（上の写しの選択はそこで終わる）
                block.end + eol.len()
            }
        };
        let cursor = map(self.cursor);
        let anchor = self.anchor.map(map);
        self.apply_edit(Edit {
            splices: &[Splice::new(block.end..block.end, &copy)],
            cursor,
            anchor,
            kind: EditKind::Replace,
            line_ending: None,
        });
    }

    /// 行の塊を隣の行と入れ替える。**既存の改行を並べ替えるだけ**で新しい改行を作らない
    /// （CRLF のファイルで LF を混ぜない = #1650）。文書の端なら何もしない
    fn move_lines(&mut self, down: bool) {
        let block = self.line_block(self.target_range());
        let moved = &self.text[block.clone()];
        let (region, replacement, new_start, after) = if down {
            let gap = line_break_len_after(&self.text, block.end);
            if gap == 0 {
                return;
            }
            let next_start = block.end + gap;
            let next_end = self.line_end(next_start);
            let brk = &self.text[block.end..next_start];
            let next = &self.text[next_start..next_end];
            let new_start = block.start + next.len() + brk.len();
            // 移した塊の後ろに来るのは、元は下の行の後ろにあった改行
            let after = new_start + moved.len() + line_break_len_after(&self.text, next_end);
            (
                block.start..next_end,
                format!("{next}{brk}{moved}"),
                new_start,
                after,
            )
        } else {
            if block.start == 0 {
                return;
            }
            let prev_end = block.start - line_break_len_before(&self.text, block.start);
            let prev_start = self.line_start(prev_end);
            let brk = &self.text[prev_end..block.start];
            let prev = &self.text[prev_start..prev_end];
            (
                prev_start..block.end,
                format!("{moved}{brk}{prev}"),
                prev_start,
                prev_start + moved.len() + brk.len(),
            )
        };
        // 塊の中の点は塊と一緒に動く。含めなかった次の行頭（選択の終わり）は、
        // 動いた後の塊の次の行頭へ
        let map = |p: usize| -> usize {
            if p <= block.end {
                new_start + (p - block.start)
            } else {
                after
            }
        };
        let cursor = map(self.cursor);
        let anchor = self.anchor.map(map);
        self.apply_edit(Edit {
            splices: &[Splice::new(region, &replacement)],
            cursor,
            anchor,
            kind: EditKind::Replace,
            line_ending: None,
        });
    }

    /// 行コメントを付ける / 外す / 切り替える。`syntax` が無ければ本文を触らずに断る。
    ///
    /// 付けるときは空行を飛ばし、全行のいちばん浅いインデントの桁へ揃えて挿す
    /// （VS Code と同じ。深い行の記号だけ右へずれない）。対象が空行だけなら空行へ挿す
    /// （空行で ⌘/ を押してコメントを書き始められる）。各行の書き換えは離れた箇所の並びとして
    /// 1 回の編集にまとめる（#1869 の口 = undo 1 回）
    fn comment_lines(
        &mut self,
        op: LineOperation,
        syntax: Option<CommentSyntax>,
    ) -> Result<(), LineOpError> {
        let Some(syntax) = syntax else {
            return Err(LineOpError::NoCommentSyntax {
                name: self
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            });
        };
        let block = self.line_block(self.target_range());
        // (行頭, 行の中身 = 改行を除く)
        let mut lines: Vec<(usize, &str)> = Vec::new();
        let mut start = block.start;
        for piece in self.text[block.clone()].split('\n') {
            lines.push((start, piece.strip_suffix('\r').unwrap_or(piece)));
            start += piece.len() + 1;
        }
        let filled: Vec<(usize, &str)> = lines
            .iter()
            .copied()
            .filter(|(_, content)| !content.trim().is_empty())
            .collect();
        let uncomment = match op {
            LineOperation::Uncomment => true,
            LineOperation::ToggleComment => {
                !filled.is_empty() && filled.iter().all(|(_, c)| syntax.is_commented(c))
            }
            _ => false,
        };
        let mut splices: Vec<(Range<usize>, String)> = Vec::new();
        if uncomment {
            for (start, content) in &filled {
                for cut in syntax.uncomment_cuts(content).unwrap_or_default() {
                    splices.push((start + cut.start..start + cut.end, String::new()));
                }
            }
        } else {
            let targets = if filled.is_empty() { lines } else { filled };
            let column = targets
                .iter()
                .map(|(_, c)| display_width(leading_ws(c)))
                .min()
                .unwrap_or(0);
            for (start, content) in targets {
                let at =
                    start + byte_for_display_column(content, column).min(leading_ws(content).len());
                match syntax {
                    CommentSyntax::Line(token) => splices.push((at..at, format!("{token} "))),
                    CommentSyntax::Block { open, close } => {
                        let end =
                            start + content.trim_end_matches([' ', '\t']).len().max(at - start);
                        splices.push((at..at, format!("{open} ")));
                        splices.push((end..end, format!(" {close}")));
                    }
                }
            }
        }
        if splices.is_empty() {
            return Ok(());
        }
        let selection = self.selection();
        let place = |p: usize| -> usize {
            // 選択の始まりだけは挿した記号の手前に残す（記号も選択に入る）
            let after_insert = selection.as_ref().is_none_or(|range| p != range.start);
            follow(&splices, p, after_insert)
        };
        let cursor = place(self.cursor);
        let anchor = self.anchor.map(place);
        let borrowed: Vec<Splice<'_>> = splices
            .iter()
            .map(|(range, text)| Splice::new(range.clone(), text))
            .collect();
        self.apply_edit(Edit {
            splices: &borrowed,
            cursor,
            anchor,
            kind: EditKind::Replace,
            line_ending: None,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const RUST: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";

    fn buffer(name: &str, text: &str) -> TextBuffer {
        TextBuffer::from_text(PathBuf::from(name), text.to_string())
    }

    /// 0 始まりの行・桁のバイト位置
    fn at(buffer: &TextBuffer, line: usize, col: usize) -> usize {
        buffer.offset_for_line_byte_col(line, col)
    }

    /// `text` の 2 行目（0 始まり 1）にキャレット / 2〜3 行目を選択した状態で `op` を当てる
    fn run(text: &str, op: LineOperation, select: bool) -> TextBuffer {
        let mut b = buffer("sample.rs", text);
        if select {
            let (anchor, head) = (at(&b, 1, 4), at(&b, 2, 6));
            b.set_selection(anchor, head);
        } else {
            let caret = at(&b, 1, 8);
            b.set_cursor(caret, false);
        }
        b.apply_line_operation(op)
            .expect("Rust には行コメントがある");
        b
    }

    /// 受け入れ条件 1: 7 操作 × 選択なし / 複数行選択の全文が固定値と一致する
    #[test]
    fn 各操作の全文は固定値と一致する() {
        use LineOperation as L;
        let commented = "fn main() {\n    // let a = 1;\n    // let b = 2;\n}\n";
        // (入力, 操作, 選択するか, 期待する全文)
        let cases: &[(&str, LineOperation, bool, &str)] = &[
            // --- 選択なし（カーソルは 2 行目）---
            (
                RUST,
                L::DuplicateUp,
                false,
                "fn main() {\n    let a = 1;\n    let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::DuplicateDown,
                false,
                "fn main() {\n    let a = 1;\n    let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::MoveUp,
                false,
                "    let a = 1;\nfn main() {\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::MoveDown,
                false,
                "fn main() {\n    let b = 2;\n    let a = 1;\n}\n",
            ),
            (
                RUST,
                L::Indent,
                false,
                "fn main() {\n        let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::Outdent,
                false,
                "fn main() {\nlet a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::Comment,
                false,
                "fn main() {\n    // let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                commented,
                L::Uncomment,
                false,
                "fn main() {\n    let a = 1;\n    // let b = 2;\n}\n",
            ),
            // --- 2〜3 行目を選択 ---
            (
                RUST,
                L::DuplicateUp,
                true,
                "fn main() {\n    let a = 1;\n    let b = 2;\n    let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::DuplicateDown,
                true,
                "fn main() {\n    let a = 1;\n    let b = 2;\n    let a = 1;\n    let b = 2;\n}\n",
            ),
            (
                RUST,
                L::MoveUp,
                true,
                "    let a = 1;\n    let b = 2;\nfn main() {\n}\n",
            ),
            (
                RUST,
                L::MoveDown,
                true,
                "fn main() {\n}\n    let a = 1;\n    let b = 2;\n",
            ),
            (
                RUST,
                L::Indent,
                true,
                "fn main() {\n        let a = 1;\n        let b = 2;\n}\n",
            ),
            (
                RUST,
                L::Outdent,
                true,
                "fn main() {\nlet a = 1;\nlet b = 2;\n}\n",
            ),
            (RUST, L::Comment, true, commented),
            (commented, L::Uncomment, true, RUST),
            // トグルは「全部付いていれば外す / そうでなければ付ける」
            (RUST, L::ToggleComment, true, commented),
            (commented, L::ToggleComment, true, RUST),
        ];
        for (input, op, select, want) in cases {
            let b = run(input, *op, *select);
            assert_eq!(b.text(), *want, "{op:?}（選択 {select}）");
        }
    }

    /// カーソルと選択は動かした行・写しについていく
    #[test]
    fn カーソルと選択は対象の行についていく() {
        use LineOperation as L;
        // 複製: 上 = 元の行に残る / 下 = 下の写しへ
        let b = run(RUST, L::DuplicateUp, false);
        assert_eq!(b.line_byte_col(b.cursor()), (1, 8));
        let b = run(RUST, L::DuplicateDown, false);
        assert_eq!(b.line_byte_col(b.cursor()), (2, 8));
        // 選択した 2 行を下へ複製すると、選択は下の写しの 2 行
        let b = run(RUST, L::DuplicateDown, true);
        let (from, to) = b.selection_positions().expect("選択が残る");
        assert_eq!((from.line, from.column, to.line, to.column), (4, 4, 5, 6));
        // 移動: 選択ごと動く
        let b = run(RUST, L::MoveUp, true);
        let (from, to) = b.selection_positions().expect("選択が残る");
        assert_eq!((from.line, from.column, to.line, to.column), (1, 4, 2, 6));
        let b = run(RUST, L::MoveDown, false);
        assert_eq!(b.line_byte_col(b.cursor()), (2, 8));
        // コメント: キャレットは同じ文字を指し続ける（`let ` の後ろ = 3 バイト右へ）
        let b = run(RUST, L::Comment, false);
        assert_eq!(b.line_byte_col(b.cursor()), (1, 11));
    }

    /// 行を丸ごと選んだ（終わりが次の行頭）ときは、その次の行を対象に含めず、
    /// 選択は移した / 写した行を丸ごと覆ったまま
    #[test]
    fn 行を丸ごと選んだ選択は次の行を含めない() {
        let mut b = buffer("x.rs", "a\nb\nc\n");
        b.set_selection(2, 4); // "b\n" を丸ごと
        b.apply_line_operation(LineOperation::MoveUp).unwrap();
        assert_eq!(b.text(), "b\na\nc\n");
        assert_eq!(b.selection(), Some(0..2));
        b.apply_line_operation(LineOperation::MoveDown).unwrap();
        assert_eq!(b.text(), "a\nb\nc\n");
        assert_eq!(b.selection(), Some(2..4));
        b.apply_line_operation(LineOperation::DuplicateDown)
            .unwrap();
        assert_eq!(b.text(), "a\nb\nb\nc\n");
        assert_eq!(b.selection(), Some(4..6));
        b.apply_line_operation(LineOperation::DuplicateUp).unwrap();
        assert_eq!(b.text(), "a\nb\nb\nb\nc\n");
        assert_eq!(b.selection(), Some(4..6));
    }

    /// 受け入れ条件 2: 先頭行の上移動・末尾行の下移動は本文も版も履歴も変えない
    #[test]
    fn 文書の端の移動は何も変えない() {
        for (text, line, op) in [
            (RUST, 0, LineOperation::MoveUp),
            // 末尾が改行で終わる文書の最後の行は、改行の後ろの空行
            (RUST, 4, LineOperation::MoveDown),
            ("a\nb", 1, LineOperation::MoveDown),
            ("only", 0, LineOperation::MoveUp),
            ("only", 0, LineOperation::MoveDown),
            ("", 0, LineOperation::MoveUp),
            ("", 0, LineOperation::MoveDown),
        ] {
            let mut b = buffer("edge.rs", text);
            let caret = at(&b, line, 0);
            b.set_cursor(caret, false);
            let version = b.version();
            b.apply_line_operation(op).unwrap();
            assert_eq!(b.text(), text, "{text:?} の {line} 行目で {op:?}");
            assert_eq!(b.version(), version, "{text:?} の {op:?} で版が進んだ");
            assert!(!b.can_undo(), "{text:?} の {op:?} が履歴を積んだ");
        }
    }

    /// 受け入れ条件 3: 言語の同定の表（file_type）に 1 行足すだけで結果が変わる
    /// （表以外は同じコードを通る。足す行は `comment` 列だけを持ち、他は既定）
    #[test]
    fn 表に1行足すだけでコメントが効くようになる() {
        let path = Path::new("notes.zz");
        let mut rows = file_type::TABLE.to_vec();
        let mut b = buffer("notes.zz", "alpha\n");
        assert_eq!(comment_syntax_in(&rows, path), None);
        assert_eq!(
            b.comment_lines(LineOperation::ToggleComment, comment_syntax_in(&rows, path)),
            Err(LineOpError::NoCommentSyntax {
                name: "notes.zz".into()
            })
        );
        assert_eq!(b.text(), "alpha\n", "断ったら本文は変えない");
        rows.push(FileType {
            id: "zz",
            extensions: &["zz"],
            comment: Some(CommentSyntax::Line("%%")),
            ..file_type::CODE
        });
        b.comment_lines(LineOperation::ToggleComment, comment_syntax_in(&rows, path))
            .unwrap();
        assert_eq!(b.text(), "%% alpha\n");
        b.comment_lines(LineOperation::ToggleComment, comment_syntax_in(&rows, path))
            .unwrap();
        assert_eq!(b.text(), "alpha\n");
    }

    /// 受け入れ条件 4: どの操作も undo 1 回で元の全文へバイト一致で戻り、redo で戻る。
    /// 直前の打鍵の塊（#1651）とはまとまらない
    #[test]
    fn 各操作はundo1回で元に戻る() {
        for op in LineOperation::ALL {
            for select in [false, true] {
                let mut b = run(RUST, op, select);
                let after = b.text().to_string();
                if after == RUST {
                    continue; // 変わらない操作（アンコメント）は履歴を積まない
                }
                assert!(b.undo(), "{op:?} を undo できない");
                assert_eq!(
                    b.text(),
                    RUST,
                    "{op:?}（選択 {select}）が undo 1 回で戻らない"
                );
                assert!(b.redo());
                assert_eq!(b.text(), after, "{op:?} の redo");
            }
        }
        // 打った直後の行操作: undo 1 回目で操作だけ、2 回目で打った塊が戻る
        let mut b = buffer("typed.rs", RUST);
        let end_of_line = at(&b, 1, 14);
        b.set_cursor(end_of_line, false);
        b.insert("xy");
        let typed = b.text().to_string();
        b.apply_line_operation(LineOperation::MoveUp).unwrap();
        assert!(b.undo());
        assert_eq!(b.text(), typed);
        assert!(b.undo());
        assert_eq!(b.text(), RUST);
    }

    /// 受け入れ条件 5: CRLF のファイルでは、どの操作の後も LF だけの改行・裸の CR が無い
    #[test]
    fn crlfのファイルで改行コードが混ざらない() {
        let crlf = RUST.replace('\n', "\r\n");
        let commented = "fn main() {\r\n    // let a = 1;\r\n    // let b = 2;\r\n}\r\n";
        for op in LineOperation::ALL {
            for select in [false, true] {
                for input in [crlf.as_str(), commented] {
                    let b = run(input, op, select);
                    let text = b.text();
                    let lf = text.matches('\n').count();
                    let pairs = text.matches("\r\n").count();
                    assert_eq!(
                        lf, pairs,
                        "{op:?}（選択 {select}）で LF だけの改行: {text:?}"
                    );
                    assert_eq!(
                        text.matches('\r').count(),
                        pairs,
                        "{op:?}（選択 {select}）で裸の CR: {text:?}"
                    );
                }
            }
        }
        // 最後の行（改行の無い行）を複製しても、足す改行はファイルの流儀
        let mut b = buffer("tail.rs", "a\r\nb");
        b.set_cursor(4, false);
        b.apply_line_operation(LineOperation::DuplicateDown)
            .unwrap();
        assert_eq!(b.text(), "a\r\nb\r\nb");
    }

    /// 空ファイル・1 行・末尾改行なし・空行を含む選択・コメント記号の無い形式
    #[test]
    fn 端のケース() {
        // 空ファイル: 複製は空行を 1 つ増やす（足す改行はそのバッファの流儀）
        let mut b = buffer("empty.rs", "");
        b.apply_line_operation(LineOperation::DuplicateDown)
            .unwrap();
        assert_eq!(b.text(), b.line_ending().as_str());
        // 空ファイルのインデントは何もしない（空行は深くしない = #1654 の規則）
        let mut b = buffer("empty.rs", "");
        b.apply_line_operation(LineOperation::Indent).unwrap();
        assert_eq!(b.text(), "");
        // 空行で ⌘/ を押すとコメントを書き始められる
        let mut b = buffer("empty.py", "");
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "# ");
        assert_eq!(b.cursor(), 2);
        // 末尾改行なしの 1 行
        let mut b = buffer("one.rs", "abc");
        b.apply_line_operation(LineOperation::DuplicateUp).unwrap();
        assert_eq!(b.text(), format!("abc{}abc", b.line_ending().as_str()));
        // 空行はコメントを付けず、外すときの判定にも数えない
        let mut b = buffer("blank.py", "a\n\n    b\n");
        b.select_all();
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "# a\n\n#     b\n");
        b.select_all();
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "a\n\n    b\n");
        // いちばん浅いインデントの桁へ揃える
        let mut b = buffer("x.rs", "    a\n        b\n");
        b.select_all();
        b.apply_line_operation(LineOperation::Comment).unwrap();
        assert_eq!(b.text(), "    // a\n    //     b\n");
        // 一部だけ付いていればトグルは付ける側（付いていた行には重ねる）
        let mut b = buffer("x.rs", "// a\nb\n");
        b.select_all();
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "// // a\n// b\n");
        // 記号の後ろに空白が無いコメントも外せる
        let mut b = buffer("x.rs", "//a\n");
        b.apply_line_operation(LineOperation::Uncomment).unwrap();
        assert_eq!(b.text(), "a\n");
        // コメント記号の無い形式は断り、本文も版も変えない
        let mut b = buffer("notes.txt", "a\n");
        let version = b.version();
        assert_eq!(
            b.apply_line_operation(LineOperation::ToggleComment),
            Err(LineOpError::NoCommentSyntax {
                name: "notes.txt".into()
            })
        );
        assert_eq!((b.text(), b.version()), ("a\n", version));
        // 記号が無くても、コメント以外の行操作は効く
        b.apply_line_operation(LineOperation::DuplicateDown)
            .unwrap();
        assert_eq!(b.text(), "a\na\n");
    }

    /// 行コメントを持たない形式は行ごとに囲み、囲みを外す。始まりは行コメントと同じく
    /// いちばん浅いインデントの桁、終わりは行末の空白の手前
    #[test]
    fn ブロック形式は行ごとに囲む() {
        let mut b = buffer("page.html", "<p>a</p>\n  <p>b</p>  \n");
        b.select_all();
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "<!-- <p>a</p> -->\n<!--   <p>b</p> -->  \n");
        b.select_all();
        b.apply_line_operation(LineOperation::ToggleComment)
            .unwrap();
        assert_eq!(b.text(), "<p>a</p>\n  <p>b</p>  \n");
        // 空白を挟まない囲みも外せる
        let mut b = buffer("a.css", "/*x*/\n");
        b.apply_line_operation(LineOperation::Uncomment).unwrap();
        assert_eq!(b.text(), "x\n");
        // 空行だけなら空の囲みを置く（同じ位置へ 2 つ挿す = undo / redo でも往復する）
        let mut b = buffer("a.md", "");
        b.apply_line_operation(LineOperation::Comment).unwrap();
        assert_eq!(b.text(), "<!--  -->");
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(b.redo());
        assert_eq!(b.text(), "<!--  -->");
        // 囲みの付け外しも undo 1 回で戻る
        let mut b = buffer("page.html", "<p>a</p>\n  <p>b</p>  \n");
        b.select_all();
        b.apply_line_operation(LineOperation::Comment).unwrap();
        assert!(b.undo());
        assert_eq!(b.text(), "<p>a</p>\n  <p>b</p>  \n");
    }

    #[test]
    fn 名前は往復する() {
        for op in LineOperation::ALL {
            assert_eq!(LineOperation::from_name(op.name()), Some(op));
        }
        let mut names = LineOperation::names();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), LineOperation::ALL.len());
        assert_eq!(LineOperation::from_name("move_up"), None);
    }

    /// 記号は file_type の行で引く（重複の検査は file_type の単体 `表の値は重複しない` が持つ）
    #[test]
    fn 記号はfile_typeの行で引く() {
        let hash = Some(CommentSyntax::Line("#"));
        let slashes = Some(CommentSyntax::Line("//"));
        // ファイル名は拡張子より先に引く（`CMakeLists.txt` は txt = 記号なしにならない）
        assert_eq!(comment_syntax_for(Path::new("proj/CMakeLists.txt")), hash);
        assert_eq!(comment_syntax_for(Path::new("notes.txt")), None);
        assert_eq!(comment_syntax_for(Path::new("Main.RS")), slashes);
        assert_eq!(comment_syntax_for(Path::new("Makefile")), hash);
        assert_eq!(comment_syntax_for(Path::new("sketch.ino")), slashes);
        assert_eq!(
            comment_syntax_for(Path::new("q.sql")),
            Some(CommentSyntax::Line("--"))
        );
        // 表に無い形式・記号を持たない形式（JSON）は記号なし
        assert_eq!(comment_syntax_for(Path::new("data.json")), None);
        assert_eq!(comment_syntax_for(Path::new("a.unknownext")), None);
        // 行コメントを持たない形式は囲み
        assert!(matches!(
            comment_syntax_for(Path::new("index.html")),
            Some(CommentSyntax::Block { .. })
        ));
        // コードとして編集する形式の行（言語）は、記号を持たない理由が無い限り記号を持つ
        // （行を足すときに `comment` を書き忘れると、⌘/ が理由を返すだけになる）
        let without: Vec<&str> = file_type::TABLE
            .iter()
            .filter(|row| {
                row.preview == crate::open_plan::PreviewRoute::Code
                    && row.enter != file_type::EnterRule::InheritOnly
                    && row.comment.is_none()
            })
            .map(|row| row.id)
            .collect();
        assert_eq!(without, ["json", "jupyter"], "記号を持たないコードの行");
    }
}

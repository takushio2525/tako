//! 軽量テキスト編集モデル（FR-3.5）。
//!
//! UTF-8 バイト境界を不変条件としてカーソル・選択を管理し、保存時は読み込み時の
//! 内容と現在のファイルを比較して外部変更を検知する。GPUI に依存しないため、GUI・
//! dispatch・CLI・MCP の全経路が同じ編集セマンティクスを使える。
//!
//! undo/redo（#195）: 編集操作前のスナップショットをスタックに積む（上限 1000）。
//! 検索（#195）: バイト位置ベースのインクリメンタル検索と置換。

use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use thiserror::Error;

const UNDO_LIMIT: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMovement {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Debug, Error)]
pub enum TextEditError {
    #[error("ファイルを読み込めない: {0}")]
    Read(#[source] std::io::Error),
    #[error("UTF-8 テキストではないため編集できない")]
    InvalidUtf8,
    #[error("ファイルが外部で変更されたため保存しなかった")]
    ExternalChanged,
    #[error("ファイルへ保存できない: {0}")]
    Write(#[source] std::io::Error),
}

/// undo/redo 用のスナップショット
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

/// 検索ヒット 1 件（バイト範囲）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub start: usize,
    pub end: usize,
}

/// 1 ファイル分の編集バッファ。カーソルと選択端は常に UTF-8 バイト境界に置く。
#[derive(Debug, Clone)]
pub struct TextBuffer {
    path: PathBuf,
    text: String,
    baseline: Vec<u8>,
    cursor: usize,
    anchor: Option<usize>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
}

impl TextBuffer {
    pub fn open(path: &Path) -> Result<Self, TextEditError> {
        let bytes = std::fs::read(path).map_err(TextEditError::Read)?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| TextEditError::InvalidUtf8)?;
        Ok(Self {
            path: path.to_path_buf(),
            text,
            baseline: bytes,
            cursor: 0,
            anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub fn from_text(path: PathBuf, text: String) -> Self {
        let baseline = text.as_bytes().to_vec();
        Self {
            path,
            text,
            baseline,
            cursor: 0,
            anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn dirty(&self) -> bool {
        self.text.as_bytes() != self.baseline
    }

    pub fn set_text(&mut self, text: String) {
        self.push_undo();
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
    }

    pub fn set_cursor(&mut self, offset: usize, extend_selection: bool) {
        let offset = snap_boundary(&self.text, offset.min(self.text.len()));
        if extend_selection {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    pub fn insert(&mut self, text: &str) {
        self.push_undo();
        self.delete_selection_inner();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub fn newline(&mut self) {
        self.insert("\n");
    }

    pub fn delete_backward(&mut self) {
        if self.anchor.is_some() && self.selection().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            return;
        }
        if self.cursor == 0 {
            self.anchor = None;
            return;
        }
        self.push_undo();
        self.anchor = None;
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
    }

    pub fn delete_forward(&mut self) {
        if self.anchor.is_some() && self.selection().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            return;
        }
        if self.cursor == self.text.len() {
            self.anchor = None;
            return;
        }
        self.push_undo();
        self.anchor = None;
        let next = self.cursor
            + self.text[self.cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
        self.text.drain(self.cursor..next);
    }

    // --- undo / redo ---

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn push_undo(&mut self) {
        self.redo_stack.clear();
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(self.snapshot());
        self.text = snap.text;
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(self.snapshot());
        self.text = snap.text;
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    // --- 検索・置換 ---

    /// 大文字小文字を区別しない全ヒットを返す
    pub fn find_all(&self, query: &str) -> Vec<SearchHit> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower_query = query.to_lowercase();
        let lower_text = self.text.to_lowercase();
        let mut hits = Vec::new();
        let mut start = 0;
        while let Some(pos) = lower_text[start..].find(&lower_query) {
            let abs = start + pos;
            hits.push(SearchHit {
                start: abs,
                end: abs + query.len(),
            });
            start = abs + query.len();
        }
        hits
    }

    /// `from` 以降で最初のヒットを返す（ラップ検索）
    pub fn find_next(&self, query: &str, from: usize) -> Option<SearchHit> {
        let hits = self.find_all(query);
        if hits.is_empty() {
            return None;
        }
        hits.iter()
            .find(|h| h.start >= from)
            .or_else(|| hits.first())
            .cloned()
    }

    /// `from` より前で最後のヒットを返す（逆ラップ検索）
    pub fn find_prev(&self, query: &str, from: usize) -> Option<SearchHit> {
        let hits = self.find_all(query);
        if hits.is_empty() {
            return None;
        }
        hits.iter()
            .rev()
            .find(|h| h.start < from)
            .or_else(|| hits.last())
            .cloned()
    }

    /// 指定範囲を置換文字列で置き換える（1 件置換）
    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        self.push_undo();
        self.text.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
        self.anchor = None;
    }

    /// 全置換。戻り値は置換件数
    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        let hits = self.find_all(query);
        if hits.is_empty() {
            return 0;
        }
        self.push_undo();
        let mut offset: isize = 0;
        let count = hits.len();
        for hit in &hits {
            let start = (hit.start as isize + offset) as usize;
            let end = (hit.end as isize + offset) as usize;
            self.text.replace_range(start..end, replacement);
            offset += replacement.len() as isize - (hit.end - hit.start) as isize;
        }
        self.cursor = self.cursor.min(self.text.len());
        self.anchor = None;
        count
    }

    pub fn move_cursor(&mut self, movement: CursorMovement, extend_selection: bool) {
        let target = match movement {
            CursorMovement::Left => self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0),
            CursorMovement::Right => {
                self.cursor
                    + self.text[self.cursor..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0)
            }
            CursorMovement::Up => self.vertical_target(-1),
            CursorMovement::Down => self.vertical_target(1),
            CursorMovement::LineStart => self.line_start(self.cursor),
            CursorMovement::LineEnd => self.line_end(self.cursor),
            CursorMovement::DocumentStart => 0,
            CursorMovement::DocumentEnd => self.text.len(),
        };
        self.set_cursor(target, extend_selection);
    }

    /// 0 起点の行と、その行内 UTF-8 バイト位置を返す。
    pub fn line_byte_col(&self, offset: usize) -> (usize, usize) {
        let offset = snap_boundary(&self.text, offset.min(self.text.len()));
        let prefix = &self.text[..offset];
        let line = prefix.bytes().filter(|b| *b == b'\n').count();
        let start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        (line, offset - start)
    }

    /// 行番号 + 行内バイト位置を文書全体の UTF-8 バイト位置へ変換する。
    pub fn offset_for_line_byte_col(&self, line: usize, byte_col: usize) -> usize {
        let start = line_start_offset(&self.text, line).unwrap_or(self.text.len());
        let end = self.text[start..]
            .find('\n')
            .map(|i| start + i)
            .unwrap_or(self.text.len());
        snap_boundary(&self.text, (start + byte_col).min(end))
    }

    pub fn save(&mut self) -> Result<(), TextEditError> {
        let metadata = std::fs::metadata(&self.path).map_err(TextEditError::Read)?;
        if metadata.permissions().readonly() {
            return Err(TextEditError::Write(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "読み取り専用ファイル",
            )));
        }
        let current = std::fs::read(&self.path).map_err(TextEditError::Read)?;
        if current != self.baseline {
            return Err(TextEditError::ExternalChanged);
        }
        write_file(&self.path, self.text.as_bytes()).map_err(TextEditError::Write)?;
        self.baseline = self.text.as_bytes().to_vec();
        Ok(())
    }

    fn delete_selection_inner(&mut self) -> bool {
        let Some(range) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.drain(range.clone());
        self.cursor = range.start;
        self.anchor = None;
        true
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(self.text.len())
    }

    fn vertical_target(&self, delta: isize) -> usize {
        let (line, _) = self.line_byte_col(self.cursor);
        let char_col = self.text[self.line_start(self.cursor)..self.cursor]
            .chars()
            .count();
        let target_line = line.saturating_add_signed(delta);
        if target_line == line && delta != 0 {
            return self.cursor;
        }
        let Some(start) = line_start_offset(&self.text, target_line) else {
            return self.cursor;
        };
        let line_text = &self.text[start..self.line_end(start)];
        let relative = line_text
            .char_indices()
            .nth(char_col)
            .map(|(i, _)| i)
            .unwrap_or(line_text.len());
        start + relative
    }
}

fn line_start_offset(text: &str, target: usize) -> Option<usize> {
    if target == 0 {
        return Some(0);
    }
    text.match_indices('\n').nth(target - 1).map(|(i, _)| i + 1)
}

fn snap_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(unix)]
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".{name}.tako-save-{}-{nonce}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        let permissions = std::fs::metadata(path)?.permissions();
        std::fs::set_permissions(&temp, permissions)?;
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(unix))]
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-text-edit-{}-{name}", std::process::id()))
    }

    #[test]
    fn utf8の入力削除とカーソル移動は文字境界を保つ() {
        let mut buffer = TextBuffer::from_text(path("utf8"), "a日本語z".into());
        buffer.move_cursor(CursorMovement::Right, false);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), "a日".len());
        buffer.delete_backward();
        assert_eq!(buffer.text(), "a本語z");
        buffer.delete_forward();
        assert_eq!(buffer.text(), "a語z");
        buffer.insert("界\n");
        assert_eq!(buffer.text(), "a界\n語z");
    }

    #[test]
    fn 選択置換と上下移動を扱える() {
        let mut buffer = TextBuffer::from_text(path("selection"), "abc\n日本語\nxy".into());
        buffer.set_cursor(1, false);
        buffer.set_cursor("abc\n日本".len(), true);
        buffer.insert("Z");
        assert_eq!(buffer.text(), "aZ語\nxy");
        buffer.move_cursor(CursorMovement::DocumentStart, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 0));
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (0, 2));

        buffer.move_cursor(CursorMovement::LineEnd, false);
        buffer.move_cursor(CursorMovement::Left, true);
        buffer.move_cursor(CursorMovement::Left, true);
        assert_eq!(buffer.anchor(), Some("aZ語".len()));
        assert_eq!(buffer.selection(), Some(1.."aZ語".len()));
    }

    #[test]
    fn 空ファイルを編集して保存できる() {
        let path = path("empty");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("こんにちは\n");
        assert!(buffer.dirty());
        buffer.save().unwrap();
        assert!(!buffer.dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "こんにちは\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 外部変更を検知して上書きしない() {
        let path = path("external");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("mine".into());
        std::fs::write(&path, "external").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert!(buffer.dirty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み取り専用ファイルの保存は失敗して内容を保つ() {
        let path = path("readonly");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("after".into());
        assert!(matches!(buffer.save(), Err(TextEditError::Write(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(false);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 数千行の日本語バッファを末尾で編集できる() {
        let text = (0..5_000)
            .map(|i| format!("{i}: 日本語"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut buffer = TextBuffer::from_text(path("large"), text);
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.newline();
        buffer.insert("末尾");
        buffer.delete_backward();
        assert!(buffer.text().ends_with("末"));
        assert!(buffer.dirty());
    }

    #[test]
    fn undoとredoで編集を往復できる() {
        let mut buffer = TextBuffer::from_text(path("undo"), "abc".into());
        assert!(!buffer.can_undo());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("X");
        assert_eq!(buffer.text(), "abcX");
        assert!(buffer.can_undo());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abc");
        assert!(buffer.can_redo());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "abcX");
        // 新しい編集で redo スタックがクリアされる
        buffer.insert("Y");
        assert!(!buffer.can_redo());
    }

    #[test]
    fn undo上限を超えると古いスナップショットが消える() {
        let mut buffer = TextBuffer::from_text(path("undo-limit"), String::new());
        for i in 0..UNDO_LIMIT + 10 {
            buffer.insert(&i.to_string());
        }
        assert!(buffer.undo_stack.len() <= UNDO_LIMIT);
    }

    #[test]
    fn delete_backwardのundoが正しく復元する() {
        let mut buffer = TextBuffer::from_text(path("undo-del"), "日本語".into());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "日本");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "日本語");
    }

    #[test]
    fn 選択削除のundoが正しく復元する() {
        let mut buffer = TextBuffer::from_text(path("undo-sel"), "abcdef".into());
        buffer.set_cursor(1, false);
        buffer.set_cursor(4, true);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "aef");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn find_allで大文字小文字を無視して検索できる() {
        let buffer = TextBuffer::from_text(path("search"), "Hello hello HELLO".into());
        let hits = buffer.find_all("hello");
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].start, 0);
        assert_eq!(hits[0].end, 5);
    }

    #[test]
    fn find_nextはラップ検索する() {
        let buffer = TextBuffer::from_text(path("search-wrap"), "aXbXc".into());
        let hit = buffer.find_next("x", 3).unwrap();
        assert_eq!(hit.start, 3);
        // from を末尾にするとラップして先頭へ
        let hit = buffer.find_next("x", 5).unwrap();
        assert_eq!(hit.start, 1);
    }

    #[test]
    fn find_prevは逆ラップ検索する() {
        let buffer = TextBuffer::from_text(path("search-prev"), "aXbXc".into());
        let hit = buffer.find_prev("x", 2).unwrap();
        assert_eq!(hit.start, 1);
        // from を先頭にするとラップして末尾へ
        let hit = buffer.find_prev("x", 0).unwrap();
        assert_eq!(hit.start, 3);
    }

    #[test]
    fn 空クエリの検索は空を返す() {
        let buffer = TextBuffer::from_text(path("search-empty"), "abc".into());
        assert!(buffer.find_all("").is_empty());
        assert!(buffer.find_next("", 0).is_none());
    }

    #[test]
    fn replace_rangeは1件を置き換えてundoできる() {
        let mut buffer = TextBuffer::from_text(path("replace1"), "foo bar foo".into());
        buffer.replace_range(0..3, "baz");
        assert_eq!(buffer.text(), "baz bar foo");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "foo bar foo");
    }

    #[test]
    fn replace_allは全件を置き換える() {
        let mut buffer = TextBuffer::from_text(path("replace-all"), "aXbXcX".into());
        let count = buffer.replace_all("x", "YY");
        assert_eq!(count, 3);
        assert_eq!(buffer.text(), "aYYbYYcYY");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "aXbXcX");
    }

    #[test]
    fn 自動保存で外部変更を上書きしない() {
        let path = path("autosave-conflict");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "original").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("edit");
        // 外部変更を模擬
        std::fs::write(&path, "external_change").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        // ファイルの中身は外部変更のまま
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external_change");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn set_textのundoでテキストが復元する() {
        let mut buffer = TextBuffer::from_text(path("set-text-undo"), "old".into());
        buffer.set_text("new".into());
        assert_eq!(buffer.text(), "new");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "old");
    }
}

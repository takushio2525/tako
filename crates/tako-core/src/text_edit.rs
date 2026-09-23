//! 軽量テキスト編集モデル（FR-3.5）。
//!
//! UTF-8 バイト境界を不変条件としてカーソル・選択を管理し、保存時は読み込み時の
//! 内容と現在のファイルを比較して外部変更を検知する。GPUI に依存しないため、GUI・
//! dispatch・CLI・MCP の全経路が同じ編集セマンティクスを使える。
//!
//! undo/redo（#195）: 編集操作前のスナップショットをスタックに積む（上限 1000）。
//! 検索（#195）: バイト位置ベースのインクリメンタル検索と置換。
//!
//! 改行コード（#1650）: バッファは**ファイルのバイト列をそのまま**持つ。`\r\n` は
//! 「1 つの行区切り」として扱い、カーソルは CR と LF のあいだに入らない。新しく足す
//! 改行（Enter・挿入テキスト・置換テキスト）だけが [`LineEnding`] の多数派に揃うので、
//! 既存行の改行は 1 バイトも書き換わらない（混在ファイルを開いて閉じてもバイト一致）。

use std::borrow::Cow;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::platform::support::Platform;

const UNDO_LIMIT: usize = 1000;

/// 改行コード（#1650）。
///
/// tako が行区切りとして扱うのは `\n` と `\r\n` の 2 つだけ。単独の `\r`
/// （Classic Mac OS の流儀）は行内の制御文字として素通しする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    /// 本文の**多数派**を返す。改行が 1 つも無ければ `None`。
    ///
    /// 同数のときは LF を選ぶ（移植性の高い側へ倒す）。多数派で決めるのは、
    /// 1 行だけ流儀の違う行が混ざっているファイルで Enter の挿入文字が
    /// その 1 行に引きずられないようにするため
    pub fn detect(text: &str) -> Option<Self> {
        let lf = text.matches('\n').count();
        if lf == 0 {
            return None;
        }
        let crlf = text.matches("\r\n").count();
        Some(if crlf > lf - crlf {
            Self::Crlf
        } else {
            Self::Lf
        })
    }

    /// 改行を 1 つも持たないファイルで使う改行コード（#1650）。
    ///
    /// ここに来るのは**空ファイルと「改行 0 の 1 行ファイル」だけ**（改行が 1 つでも
    /// あれば [`Self::detect`] が勝つ）。本文の中に手掛かりが無いので、主要エディタと
    /// 同じく **OS の流儀に従う**（VS Code の `files.eol: auto` / Visual Studio /
    /// メモ帳）。Windows の利用者が tako で作った新規ファイルを他の Windows ツールで
    /// 開いたときに驚きが無いことを優先する（tako はゼロコンフィグが最優先の設計原則）。
    ///
    /// `platform` を**引数で受ける**ので、macOS 上からでも Windows 側の腕を検査できる
    /// （`platform::keys` と同じ型。これが無いと「Windows でどうなるか」を
    /// 実機無しに固定できない = #1650 で実際に CI だけが落ちた）
    pub fn for_new_file(platform: Platform) -> Self {
        match platform {
            Platform::MacOs => Self::Lf,
            Platform::Windows => Self::Crlf,
        }
    }

    /// 実行中のプラットフォームでの [`Self::for_new_file`]
    pub fn platform_default() -> Self {
        Self::for_new_file(Platform::current())
    }

    /// 検出できなければ [`Self::platform_default`] へ倒す
    pub fn detect_or_default(text: &str) -> Self {
        Self::detect(text).unwrap_or_else(Self::platform_default)
    }
}

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
    /// 新しく足す改行に使うコード（#1650）。既存行の改行は書き換えない
    line_ending: LineEnding,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
}

impl TextBuffer {
    pub fn open(path: &Path) -> Result<Self, TextEditError> {
        let bytes = std::fs::read(path).map_err(TextEditError::Read)?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| TextEditError::InvalidUtf8)?;
        Ok(Self {
            line_ending: LineEnding::detect_or_default(&text),
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
            line_ending: LineEnding::detect_or_default(&text),
            path,
            text,
            baseline,
            cursor: 0,
            anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// このバッファが新しい改行に使うコード（#1650）
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
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

    /// 本文を丸ごと差し替える（外部変更の取り込み・dispatch からの適用）。
    ///
    /// 渡された本文の改行コードを取り直す（#1650）。呼び出し側は「保存されるべき
    /// 全文」を渡しているので、そこに書かれている流儀がこのファイルの流儀になる。
    /// 改行を 1 つも含まない本文では直前の流儀を保つ
    pub fn set_text(&mut self, text: String) {
        self.push_undo();
        self.line_ending = LineEnding::detect(&text).unwrap_or(self.line_ending);
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
    }

    pub fn set_cursor(&mut self, offset: usize, extend_selection: bool) {
        let offset = snap_cursor(&self.text, offset.min(self.text.len()));
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

    /// 本文を差し込む。**入ってくる改行はこのバッファの流儀へ揃える**（#1650）。
    ///
    /// 打鍵・IME・貼り付け・dispatch（CLI / MCP）の挿入がすべてここを通るので、
    /// 揃えるのをここ 1 か所にしておけば、どの経路から入れても混在改行にならない
    pub fn insert(&mut self, text: &str) {
        let text = normalize_line_endings(text, self.line_ending);
        self.push_undo();
        self.delete_selection_inner();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    /// Enter。挿す文字はこのファイルの改行コード（#1650）
    pub fn newline(&mut self) {
        self.insert(self.line_ending.as_str());
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
        // `\r\n` は 1 つの行区切り。片方だけ消すと裸の CR が残る（#1650）
        let previous = if self.text[..self.cursor].ends_with("\r\n") {
            self.cursor - 2
        } else {
            self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
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
        // `\r\n` は 1 つの行区切り。片方だけ消すと裸の CR が残る（#1650）
        let next = if self.text[self.cursor..].starts_with("\r\n") {
            self.cursor + 2
        } else {
            self.cursor
                + self.text[self.cursor..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0)
        };
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
    ///
    /// 返す位置は**元テキストのバイト位置**。小文字化はバイト長を変えうる
    /// （`İ` U+0130 は 2 → 3 バイト、`ẞ` U+1E9E は 3 → 2 バイト）ので、
    /// 小文字化した写しのバイト位置をそのまま元テキストの位置として使うとずれる（#1016）。
    /// 本文全体のバイト長が一致していても安全ではない（`İ` と `ẞ` が両方あると
    /// 伸縮が打ち消しあって総和だけ一致し、途中の位置は食い違う）。
    ///
    /// そこで探索そのものは小文字化した写しに対する高速な部分文字列探索のまま残し、
    /// 見つかった位置を [`Lowered::to_original`] で元テキストへ戻す。
    /// 戻せない位置（展開された文字の途中で始まる／終わる一致）はヒットにしない。
    pub fn find_all(&self, query: &str) -> Vec<SearchHit> {
        let lower_query = lowercase_per_char(query);
        if lower_query.is_empty() {
            return Vec::new();
        }
        let lowered = Lowered::build(&self.text);
        let mut hits = Vec::new();
        let mut cursor = 0;
        while let Some(pos) = lowered.text[cursor..].find(&lower_query) {
            let lower_start = cursor + pos;
            let lower_end = lower_start + lower_query.len();
            match (
                lowered.to_original(lower_start),
                lowered.to_original(lower_end),
            ) {
                (Some(start), Some(end)) => {
                    hits.push(SearchHit { start, end });
                    cursor = lower_end;
                }
                // 元テキストに対応する位置が無い一致は返せない
                // （返すと slice が文字境界を割って panic する）。次の文字境界から探し直す
                _ => {
                    let step = lowered.text[lower_start..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1);
                    cursor = lower_start + step;
                }
            }
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

    /// 指定範囲を置換文字列で置き換える（1 件置換）。
    /// 置換文字列の改行もこのバッファの流儀へ揃える（#1650）
    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        let replacement = normalize_line_endings(replacement, self.line_ending);
        self.push_undo();
        self.text.replace_range(range.clone(), &replacement);
        self.cursor = range.start + replacement.len();
        self.anchor = None;
    }

    /// 全置換。戻り値は置換件数
    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        let hits = self.find_all(query);
        if hits.is_empty() {
            return 0;
        }
        let replacement = normalize_line_endings(replacement, self.line_ending);
        self.push_undo();
        let mut offset: isize = 0;
        let count = hits.len();
        for hit in &hits {
            let start = (hit.start as isize + offset) as usize;
            let end = (hit.end as isize + offset) as usize;
            self.text.replace_range(start..end, &replacement);
            offset += replacement.len() as isize - (hit.end - hit.start) as isize;
        }
        self.cursor = self.cursor.min(self.text.len());
        self.anchor = None;
        count
    }

    pub fn move_cursor(&mut self, movement: CursorMovement, extend_selection: bool) {
        let target = match movement {
            // `\r\n` は 1 つの行区切りなので、あいだで止まらずにまたぐ（#1650）
            CursorMovement::Left if self.text[..self.cursor].ends_with("\r\n") => self.cursor - 2,
            CursorMovement::Left => self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0),
            CursorMovement::Right if self.text[self.cursor..].starts_with("\r\n") => {
                self.cursor + 2
            }
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
        let end = line_end_offset(&self.text, start);
        snap_cursor(&self.text, (start + byte_col).min(end))
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
        line_end_offset(&self.text, offset)
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

/// 1 文字ずつ小文字化して連結する（#1016）
///
/// `str::to_lowercase` を使わないのは、あれが**文脈依存**だから。ギリシャ語の Σ は
/// 語末だけ ς になるので、本文とクエリを別々に丸ごと小文字化すると
/// 「本文にそのまま在る部分文字列を、それ自身で検索しても見つからない」ことが起きる
/// （本文 `ΟΔΟΣΧ` → `οδοσχ` / クエリ `ΟΔΟΣ` → `οδος`）。
/// 突き合わせる両側を同じ 1 文字単位の写像へ揃えることで、この食い違いを構造的に消す。
fn lowercase_per_char(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// 小文字化でバイト長が変わった文字 1 個ぶんの記録
struct Shift {
    /// 小文字化した写しでのその文字の開始位置
    lower_start: usize,
    /// 元テキストでのその文字の開始位置
    orig_start: usize,
    /// 小文字化後のバイト長
    lower_len: usize,
    /// 元のバイト長
    orig_len: usize,
}

/// 小文字化した本文の写しと、その位置を元テキストへ戻すための情報（#1016）
///
/// `shifts` は**バイト長が変わった文字だけ**を開始位置の昇順で持つ。
/// ASCII・日本語・ほとんどのラテン文字では空なので、位置の変換は恒等になる。
struct Lowered {
    text: String,
    shifts: Vec<Shift>,
}

impl Lowered {
    fn build(text: &str) -> Self {
        let mut lower = String::with_capacity(text.len());
        let mut shifts = Vec::new();
        for (orig_start, ch) in text.char_indices() {
            // ASCII の小文字化は必ず 1 バイト → 1 バイトなのでずれない
            if ch.is_ascii() {
                lower.push(ch.to_ascii_lowercase());
                continue;
            }
            let lower_start = lower.len();
            for lc in ch.to_lowercase() {
                lower.push(lc);
            }
            let lower_len = lower.len() - lower_start;
            let orig_len = ch.len_utf8();
            if lower_len != orig_len {
                shifts.push(Shift {
                    lower_start,
                    orig_start,
                    lower_len,
                    orig_len,
                });
            }
        }
        Self {
            text: lower,
            shifts,
        }
    }

    /// 写しのバイト位置 `pos` に対応する元テキストのバイト位置を返す。
    ///
    /// `pos` が「小文字化で複数文字へ展開された文字」の途中を指すときは `None`
    /// （元テキストに対応するバイト位置が存在しない）。
    fn to_original(&self, pos: usize) -> Option<usize> {
        if self.shifts.is_empty() {
            return Some(pos);
        }
        // `pos` 以下で始まる最後のずれを探す。それより後ろは 1:1 対応に戻る
        let idx = self.shifts.partition_point(|s| s.lower_start <= pos);
        let Some(shift) = idx.checked_sub(1).map(|i| &self.shifts[i]) else {
            return Some(pos);
        };
        if pos == shift.lower_start {
            return Some(shift.orig_start);
        }
        let lower_end = shift.lower_start + shift.lower_len;
        if pos < lower_end {
            return None;
        }
        Some(shift.orig_start + shift.orig_len + (pos - lower_end))
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

/// 行末（改行の手前）のバイト位置。`\r\n` で終わる行では **CR の手前**を返す（#1650）。
///
/// `\n` の位置を返していたのが Issue #1650 の本体で、CRLF ファイルで End を押すと
/// カーソルが画面上の行末より右（CR の後ろ）へ行き、そこで打つと `abc\r!\n` になった
fn line_end_offset(text: &str, offset: usize) -> usize {
    match text[offset..].find('\n') {
        Some(i) => {
            let newline = offset + i;
            if text[..newline].ends_with('\r') {
                newline - 1
            } else {
                newline
            }
        }
        None => text.len(),
    }
}

/// カーソルを置いてよい位置へ丸める。
///
/// UTF-8 の文字境界に加えて、**`\r\n` のあいだ**（CR の後ろ）を除く（#1650）。
/// そこは画面上どこにも対応しない位置で、行末判定を直しても外から
/// [`TextBuffer::set_cursor`]（マウスクリック・IME・dispatch）で指されうる
fn snap_cursor(text: &str, offset: usize) -> usize {
    let offset = snap_boundary(text, offset);
    let bytes = text.as_bytes();
    if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
        offset - 1
    } else {
        offset
    }
}

/// 差し込む本文の改行を `ending` へ揃える（#1650）。
///
/// 単独の `\r` は行区切りではないので触らない。揃える必要が無いときは
/// 借用のまま返すので、1 文字ずつの打鍵では何も確保しない
fn normalize_line_endings(text: &str, ending: LineEnding) -> Cow<'_, str> {
    match ending {
        LineEnding::Lf if text.contains('\r') => Cow::Owned(text.replace("\r\n", "\n")),
        LineEnding::Crlf if text.contains('\n') => {
            // 一度 LF へ潰してから広げる（既に `\r\n` の場所を二重化しないため）
            Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n"))
        }
        _ => Cow::Borrowed(text),
    }
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
        // 本文に改行が 1 つも無いので改行コードは新規ファイルの既定（OS で変わる。#1650）。
        // 期待値は**製品の正**から作る（`\n` を直書きすると Windows でだけ落ちる）。
        // 既定そのものの腕は `新規ファイルの改行コードはプラットフォームで決まる` が固定する
        let eol = buffer.line_ending().as_str();
        buffer.insert("界\n");
        assert_eq!(buffer.text(), format!("a界{eol}語z"));
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
        // 空ファイルには改行コードの手掛かりが無いので新規ファイルの既定（OS で変わる。#1650）
        let eol = buffer.line_ending().as_str();
        assert_eq!(buffer.line_ending(), LineEnding::platform_default());
        buffer.insert("こんにちは\n");
        assert!(buffer.dirty());
        buffer.save().unwrap();
        assert!(!buffer.dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("こんにちは{eol}")
        );
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

    // --- エッジケーステスト（#195 受け入れ条件の検証） ---

    #[test]
    fn 空バッファでのundo_redoは安全にfalseを返す() {
        let mut buffer = TextBuffer::from_text(path("edge-empty-undo"), String::new());
        assert!(!buffer.undo());
        assert!(!buffer.redo());
        assert!(!buffer.can_undo());
        assert!(!buffer.can_redo());
    }

    #[test]
    fn 空バッファへの検索は空を返す() {
        let buffer = TextBuffer::from_text(path("edge-empty-search"), String::new());
        assert!(buffer.find_all("abc").is_empty());
        assert!(buffer.find_next("abc", 0).is_none());
        assert!(buffer.find_prev("abc", 0).is_none());
    }

    #[test]
    fn 空バッファへの全置換は0件を返す() {
        let mut buffer = TextBuffer::from_text(path("edge-empty-replace"), String::new());
        assert_eq!(buffer.replace_all("a", "b"), 0);
    }

    #[test]
    fn 五千行バッファでundo_redoが動く() {
        let text = (0..5_000)
            .map(|i| format!("{i}: テスト行"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut buffer = TextBuffer::from_text(path("edge-large-undo"), text.clone());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("追加");
        assert!(buffer.text().ends_with("追加"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), text);
    }

    #[test]
    fn 五千行バッファで検索が動く() {
        let text = (0..5_000)
            .map(|i| format!("{i}: テスト行"))
            .collect::<Vec<_>>()
            .join("\n");
        let buffer = TextBuffer::from_text(path("edge-large-search"), text);
        let hits = buffer.find_all("テスト行");
        assert_eq!(hits.len(), 5_000);
        let hit = buffer.find_next("4999", 0).unwrap();
        assert!(hit.start > 0);
    }

    #[test]
    fn 読み取り専用ファイルの自動保存は失敗してバッファを保つ() {
        let path = path("edge-readonly-autosave");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "readonly content").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("edit");
        assert!(buffer.dirty());
        assert!(buffer.save().is_err());
        assert!(buffer.dirty());
        assert!(buffer.text().contains("edit"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_file(path);
    }

    // --- #1016: 小文字化でバイト長が変わる文字でも位置が元テキスト基準であること ---

    #[test]
    fn ascii高速路の前提が成り立つ() {
        // `Lowered::build` は「ASCII の小文字化は必ず 1 バイト → 1 バイト」を前提に
        // ずれの記録を省いている。前提が崩れたらここで落ちる
        for byte in 0u8..=127 {
            let ch = byte as char;
            let unicode: String = ch.to_lowercase().collect();
            assert_eq!(
                unicode,
                ch.to_ascii_lowercase().to_string(),
                "ASCII {byte:#04x} の小文字化が ASCII 版と食い違う"
            );
        }
    }

    #[test]
    fn 位置の変換が展開された文字の途中を弾く() {
        // `İ`(2 バイト) → `i` + U+0307(3 バイト)。写しの 1 バイト目は展開の途中
        let lowered = Lowered::build("aİb");
        assert_eq!(lowered.text, "ai\u{307}b");
        assert_eq!(lowered.to_original(0), Some(0)); // 'a'
        assert_eq!(lowered.to_original(1), Some(1)); // 'İ' の先頭
        assert_eq!(lowered.to_original(2), None); // U+0307 の途中 = 対応する位置が無い
        assert_eq!(lowered.to_original(4), Some(3)); // 'b'
        assert_eq!(lowered.to_original(5), Some(4)); // 末尾
    }

    #[test]
    fn ずれが無い本文では位置の変換が恒等になる() {
        let lowered = Lowered::build("Abc あいう");
        assert!(lowered.shifts.is_empty());
        for pos in 0..=lowered.text.len() {
            assert_eq!(lowered.to_original(pos), Some(pos));
        }
    }

    #[test]
    fn 小文字化で伸びる文字があってもヒット位置が元テキスト基準になる() {
        // `İ`（U+0130）は小文字化で 2 → 3 バイトに伸びる。小文字化した本文の
        // バイト位置を元テキストの位置として流用すると、後続のヒットがずれる
        let text = "İstanbul needle";
        let buffer = TextBuffer::from_text(path("u1016-grow"), text.into());
        let hits = buffer.find_all("needle");
        assert_eq!(hits.len(), 1);
        let expected = text.find("needle").unwrap();
        assert_eq!(hits[0].start, expected);
        assert_eq!(hits[0].end, expected + "needle".len());
        assert_eq!(&text[hits[0].start..hits[0].end], "needle");
    }

    #[test]
    fn 小文字化で縮む文字があってもヒット位置が元テキスト基準になる() {
        // `ẞ`（U+1E9E）は小文字化で 3 → 2 バイトに縮む
        let text = "ẞ needle";
        let buffer = TextBuffer::from_text(path("u1016-shrink"), text.into());
        let hits = buffer.find_all("needle");
        assert_eq!(hits.len(), 1);
        let expected = text.find("needle").unwrap();
        assert_eq!(hits[0].start, expected);
        assert_eq!(&text[hits[0].start..hits[0].end], "needle");
    }

    #[test]
    fn 伸縮が打ち消しあって総バイト長が同じでも位置がずれない() {
        // `İ` は +1 / `ẞ` は -1 なので本文全体のバイト長は変わらないが、
        // 途中のバイト位置は食い違う（= 総バイト長の一致は健全性の根拠にならない）
        let text = "İ needle ẞ";
        assert_eq!(text.len(), text.to_lowercase().len());
        let buffer = TextBuffer::from_text(path("u1016-cancel"), text.into());
        let hits = buffer.find_all("needle");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, text.find("needle").unwrap());
    }

    #[test]
    fn 行頭と行末と複数ヒットでも位置が元テキスト基準になる() {
        let text = "needle İ\nneedle İ needle";
        let buffer = TextBuffer::from_text(path("u1016-multi"), text.into());
        let hits = buffer.find_all("NEEDLE");
        assert_eq!(hits.len(), 3);
        let expected: Vec<usize> = text.match_indices("needle").map(|(i, _)| i).collect();
        assert_eq!(
            hits.iter().map(|h| h.start).collect::<Vec<_>>(),
            expected,
            "行頭・行中・行末のヒットがすべて元テキスト基準であること"
        );
        for hit in &hits {
            assert_eq!(&text[hit.start..hit.end], "needle");
        }
    }

    #[test]
    fn ヒットの終端が本文の範囲を超えない() {
        // 本文 `İ`（2 バイト）は小文字化すると `i` + U+0307（3 バイト）。
        // 終端に「元クエリのバイト長」を足すと本文の範囲外を指し、置換が panic する
        let text = "İ";
        let mut buffer = TextBuffer::from_text(path("u1016-range"), text.into());
        let hits = buffer.find_all("i\u{307}");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, 0);
        assert_eq!(
            hits[0].end,
            text.len(),
            "終端は元テキストのバイト長で決まる"
        );
        buffer.replace_range(hits[0].start..hits[0].end, "I");
        assert_eq!(buffer.text(), "I");
    }

    #[test]
    fn 小文字化した文字の途中から始まるクエリはヒットしない() {
        // `İ` の小文字化は `i` + U+0307。U+0307 から始まるクエリは元テキストの
        // 文字境界に対応しないので、返せる位置が存在しない（返すと slice が panic する）
        let buffer = TextBuffer::from_text(path("u1016-mid"), "İstanbul".into());
        assert!(buffer.find_all("\u{307}stanbul").is_empty());
    }

    #[test]
    fn 本文の部分文字列はそれ自身をクエリにすれば必ず見つかる() {
        // `str::to_lowercase` は文脈依存（語末の Σ だけ ς になる）なので、本文と
        // クエリを別々に丸ごと小文字化すると同じ文字列同士が一致しないことがある
        let text = "ΟΔΟΣΧ";
        let buffer = TextBuffer::from_text(path("u1016-sigma"), text.into());
        let hits = buffer.find_all("ΟΔΟΣ");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, 0);
        assert_eq!(&text[hits[0].start..hits[0].end], "ΟΔΟΣ");
    }

    #[test]
    fn 小文字化で伸びる文字を含む本文を壊さずに全置換できる() {
        let mut buffer =
            TextBuffer::from_text(path("u1016-replace-all"), "İstanbul foo İzmir foo".into());
        let count = buffer.replace_all("foo", "bar");
        assert_eq!(count, 2);
        assert_eq!(buffer.text(), "İstanbul bar İzmir bar");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "İstanbul foo İzmir foo");
    }

    #[test]
    fn 伸びる文字そのものを全置換しても本文が壊れない() {
        let mut buffer = TextBuffer::from_text(path("u1016-replace-char"), "aİbİc".into());
        let count = buffer.replace_all("İ", "-");
        assert_eq!(count, 2);
        assert_eq!(buffer.text(), "a-b-c");
    }

    #[test]
    fn asciiのみと日本語のみの検索置換は従来どおり動く() {
        // 回帰確認: 小文字化でバイト長が変わらない文字だけの本文
        let buffer = TextBuffer::from_text(path("u1016-ascii"), "Foo foo FOO".into());
        let hits = buffer.find_all("foo");
        assert_eq!(
            hits.iter().map(|h| (h.start, h.end)).collect::<Vec<_>>(),
            vec![(0, 3), (4, 7), (8, 11)]
        );

        let mut jp = TextBuffer::from_text(path("u1016-jp"), "あいうえお かきくけこ".into());
        let hits = jp.find_all("うえ");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, "あい".len());
        assert_eq!(hits[0].end, "あいうえ".len());
        assert_eq!(jp.replace_all("かき", "サシ"), 1);
        assert_eq!(jp.text(), "あいうえお サシくけこ");
    }

    #[test]
    fn 伸びる文字を含む本文でも空クエリは空を返す() {
        let buffer = TextBuffer::from_text(path("u1016-empty"), "İstanbul".into());
        assert!(buffer.find_all("").is_empty());
        assert!(buffer.find_next("", 0).is_none());
        assert!(buffer.find_prev("", 0).is_none());
    }

    // --- #1650: 改行コード（LF / CRLF）の検出・保持 ---

    /// 往復保存の実測を 1 か所で取る（開く → 行末 → Enter → 文字 → 保存）。
    /// 戻り値は保存後の生バイト
    fn crlf_round_trip(name: &str, original: &[u8], go_to_line: usize) -> Vec<u8> {
        let path = path(name);
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, original).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        for _ in 0..go_to_line {
            buffer.move_cursor(CursorMovement::Down, false);
        }
        buffer.move_cursor(CursorMovement::LineEnd, false);
        buffer.newline();
        buffer.insert("X");
        buffer.save().unwrap();
        let saved = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(path);
        saved
    }

    #[test]
    fn crlfファイルの行末はcrの手前へ止まる() {
        // HL3 の再現: `line_end` が `\n` の位置を返すと、カーソルが CR の後ろ
        // （画面上の行末より右）に来る。そこで打つと `abc\r!\n` になる
        let mut buffer = TextBuffer::from_text(path("crlf-end"), "abc\r\ndef\r\n".into());
        buffer.move_cursor(CursorMovement::LineEnd, false);
        assert_eq!(buffer.cursor(), 3, "行末は CR の手前（見た目の行末）");
        buffer.insert("!");
        assert_eq!(buffer.text(), "abc!\r\ndef\r\n");
    }

    #[test]
    fn crlfファイルの往復編集で改行コードが保たれる() {
        // E3 の再現: Enter が常に `\n` を挿すと CR 2 / LF 3 の混在になる
        let saved = crlf_round_trip("crlf-round", b"abc\r\ndef\r\n", 0);
        assert_eq!(saved, b"abc\r\nX\r\ndef\r\n");
        let cr = saved.iter().filter(|b| **b == b'\r').count();
        let lf = saved.iter().filter(|b| **b == b'\n').count();
        assert_eq!((cr, lf), (3, 3), "CRLF ファイルは CR の数 = LF の数");
    }

    #[test]
    fn lfファイルの往復編集でcrが混ざらない() {
        let saved = crlf_round_trip("lf-round", b"abc\ndef\n", 0);
        assert_eq!(saved, b"abc\nX\ndef\n");
        assert_eq!(saved.iter().filter(|b| **b == b'\r').count(), 0);
    }

    #[test]
    fn 混在ファイルは既存の改行を残し新しい行だけ多数派になる() {
        // `\r\n` が 2 / 裸の `\n` が 1 なので多数派は CRLF。
        // 2 行目（`b\n`）の行末で Enter を打っても、既存の `\n` は書き換えない
        let saved = crlf_round_trip("mixed-round", b"a\r\nb\nc\r\n", 1);
        assert_eq!(saved, b"a\r\nb\r\nX\nc\r\n");
    }

    #[test]
    fn 編集しなければどの改行コードでもバイト一致で保存される() {
        for (name, original) in [
            ("noop-crlf", &b"a\r\nb\r\n"[..]),
            ("noop-lf", &b"a\nb\n"[..]),
            ("noop-mixed", &b"a\r\nb\nc\r\n"[..]),
            ("noop-no-eol", &b"a\r\nb"[..]),
        ] {
            let path = path(name);
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, original).unwrap();
            let mut buffer = TextBuffer::open(&path).unwrap();
            assert!(!buffer.dirty(), "{name}: 開いただけで dirty にならない");
            buffer.save().unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                original,
                "{name}: バイト一致"
            );
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn 末尾改行の有無が往復で保たれる() {
        // 末尾に改行が無いファイルは、行末 Enter → 文字入力のあとも末尾改行なしのまま
        let saved = crlf_round_trip("no-eol-crlf", b"abc\r\ndef", 1);
        assert_eq!(saved, b"abc\r\ndef\r\nX");
        let saved = crlf_round_trip("no-eol-lf", b"abc\ndef", 1);
        assert_eq!(saved, b"abc\ndef\nX");
    }

    #[test]
    fn 改行コードの検出は多数派を選び改行が無ければ既定へ倒れる() {
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), Some(LineEnding::Crlf));
        assert_eq!(LineEnding::detect("a\nb\n"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("a\r\nb\nc\r\n"), Some(LineEnding::Crlf));
        assert_eq!(LineEnding::detect("a\r\nb\nc\n"), Some(LineEnding::Lf));
        // 同数は LF（移植性の高い側へ倒す）
        assert_eq!(LineEnding::detect("a\r\nb\n"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("abc"), None);
        assert_eq!(LineEnding::detect(""), None);
        // 改行を 1 つも持たないファイルはそのプラットフォームの流儀
        let buffer = TextBuffer::from_text(path("no-newline"), "abc".into());
        assert_eq!(buffer.line_ending(), LineEnding::platform_default());
    }

    #[test]
    fn 新規ファイルの改行コードはプラットフォームで決まる() {
        // **macOS 上からでも Windows の腕を固定する**（#1650。実機が無いと
        // 「Windows でどうなるか」をテストできず、CI だけが落ちる形になる）
        assert_eq!(LineEnding::for_new_file(Platform::MacOs), LineEnding::Lf);
        assert_eq!(
            LineEnding::for_new_file(Platform::Windows),
            LineEnding::Crlf
        );
        // 実行中の既定はこの純関数と同じ答えでなければならない
        assert_eq!(
            LineEnding::platform_default(),
            LineEnding::for_new_file(Platform::current())
        );
        // 改行を持たない本文へ入る `\n` は、その腕どおりに揃う
        for platform in [Platform::MacOs, Platform::Windows] {
            let ending = LineEnding::for_new_file(platform);
            assert_eq!(
                normalize_line_endings("a\nb", ending),
                format!("a{}b", ending.as_str()),
                "{} の新規ファイルへ入る改行",
                platform.as_str()
            );
        }
    }

    #[test]
    fn crlfバッファでは行区切りをひとまとまりとして越える() {
        let mut buffer = TextBuffer::from_text(path("crlf-move"), "abc\r\ndef".into());
        buffer.set_cursor(3, false);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), 5, "CR と LF のあいだで止まらない");
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), 3);
        // 外から CR と LF のあいだを指されても見た目の行末へ丸める
        buffer.set_cursor(4, false);
        assert_eq!(buffer.cursor(), 3);
        assert_eq!(buffer.offset_for_line_byte_col(0, 99), 3);
    }

    #[test]
    fn crlfバッファの削除は行区切りをひとまとまりで消す() {
        let mut buffer = TextBuffer::from_text(path("crlf-del-fwd"), "abc\r\ndef".into());
        buffer.set_cursor(3, false);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "abcdef", "前方削除が裸の CR を残さない");

        let mut buffer = TextBuffer::from_text(path("crlf-del-back"), "abc\r\ndef".into());
        buffer.set_cursor(5, false);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "abcdef", "後方削除が裸の CR を残さない");
    }

    #[test]
    fn 挿入テキストの改行はバッファの改行コードへ揃う() {
        let mut crlf = TextBuffer::from_text(path("crlf-paste"), "a\r\n".into());
        crlf.move_cursor(CursorMovement::DocumentEnd, false);
        crlf.insert("x\ny");
        assert_eq!(crlf.text(), "a\r\nx\r\ny", "裸の LF が混ざらない");
        crlf.insert("\r\nz");
        assert_eq!(
            crlf.text(),
            "a\r\nx\r\ny\r\nz",
            "既に CRLF なら二重化しない"
        );

        let mut lf = TextBuffer::from_text(path("lf-paste"), "a\n".into());
        lf.move_cursor(CursorMovement::DocumentEnd, false);
        lf.insert("x\r\ny");
        assert_eq!(lf.text(), "a\nx\ny", "LF ファイルへ CR を持ち込まない");
    }

    #[test]
    fn 置換の改行もバッファの改行コードへ揃う() {
        let mut buffer = TextBuffer::from_text(path("crlf-replace"), "a\r\nQ\r\n".into());
        assert_eq!(buffer.replace_all("q", "1\n2"), 1);
        assert_eq!(buffer.text(), "a\r\n1\r\n2\r\n");

        let mut buffer = TextBuffer::from_text(path("crlf-replace-range"), "a\r\nQ\r\n".into());
        buffer.replace_range(3..4, "1\n2");
        assert_eq!(buffer.text(), "a\r\n1\r\n2\r\n");
    }

    #[test]
    fn set_textは渡された本文から改行コードを取り直す() {
        let mut buffer = TextBuffer::from_text(path("set-text-eol"), "a\r\nb\r\n".into());
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        buffer.set_text("a\nb\n".into());
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
        buffer.newline();
        assert_eq!(buffer.text(), "a\nb\n\n");
        // 改行を持たない本文では直前の改行コードを保つ
        buffer.set_text("abc".into());
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
    }

    #[test]
    fn crlfバッファでも上下移動が見た目の行内へ収まる() {
        let mut buffer = TextBuffer::from_text(path("crlf-vertical"), "abcdef\r\ngh\r\nij".into());
        buffer.move_cursor(CursorMovement::LineEnd, false);
        assert_eq!(buffer.cursor(), 6);
        buffer.move_cursor(CursorMovement::Down, false);
        // 2 行目は 2 文字しかないので行末（CR の手前）で止まる
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 2));
        buffer.insert("!");
        assert_eq!(buffer.text(), "abcdef\r\ngh!\r\nij");
    }

    #[test]
    fn 行頭行末とカーソル位置がcrを行内文字に数えない() {
        let buffer = TextBuffer::from_text(path("crlf-col"), "abc\r\ndef\r\n".into());
        // 2 行目の先頭は LF の次
        assert_eq!(buffer.line_byte_col(5), (1, 0));
        // 2 行目の行末（CR の手前）は列 3
        assert_eq!(buffer.line_byte_col(8), (1, 3));
        assert_eq!(buffer.offset_for_line_byte_col(1, 3), 8);
    }

    #[test]
    fn 日本語を含む検索と置換が正しいバイト位置で動く() {
        let mut buffer = TextBuffer::from_text(path("edge-jp-search"), "あいうえお".into());
        let hits = buffer.find_all("うえ");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, "あい".len());
        assert_eq!(hits[0].end, "あいうえ".len());
        buffer.replace_range(hits[0].start..hits[0].end, "カキ");
        assert_eq!(buffer.text(), "あいカキお");
    }
}

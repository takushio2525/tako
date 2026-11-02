//! ワークスペース横断置換（FR-3.43 の続き / #2045。⇧⌘F の置換欄・`tako search --replace`・
//! MCP `tako_search` の action=replace / undo）
//!
//! 横断検索（[`crate::workspace_search`]）の結果の一覧を土台に、置換のプレビュー → 1 行 /
//! ファイル単位 / 全部を当てる → 取り消す。GUI・CLI・MCP は同じこの 1 実装を通る
//! （プレビューは「当てる手前までの同じ判定」= [`plan`] と [`check_disk`] / [`check_buffer`]）。
//!
//! ## 決めごと（受け入れ条件で固定するもの）
//!
//! - **当てるのは一覧に出た行だけ**（1 件 = 1 行。その行の一致をすべて置き換える）。上限で
//!   打ち切った検索でも、一覧に出ていない行は書き換えない
//! - 照合は検索と同じ照合器・同じ行の切り方（`\n` で割り、行末の `\r` を外し、先頭の BOM を外す。
//!   幅 0 の一致は数えない）。書き換えるのは一致の範囲だけなので、改行コード・BOM はそのまま残る
//! - 置換後の文字列（[`Template`]）: 文字どおりの検索ではそのまま。正規表現の検索では `$1`〜`$99`・
//!   `$0` / `$&`（一致全体）・`${名前}` / `${1}`・`$$`（`$` そのもの）を展開する。`$1a` は `$1` + `a`
//!   （VS Code と同じ読み）。無いグループを指したら当てずに理由を返す。改行は入れられない
//! - 検索した時点の中身の指紋（[`Stamp`]）と今の中身が違うファイルは当てない（[`Skip::Stale`]）
//! - **開いている編集バッファはバッファへ当てる**（`TextBuffer::apply_changes` = undo 1 回で戻る。
//!   保存はしない）。バッファの中身が期待と違う = 検索が見ていない編集があるなら当てない
//!   （保存していなければ [`Skip::Unsaved`]）
//! - 開いていないファイルは**書く前に退避**し（[`BACKUP_DIR`]）、取り消し（[`restore_disk`]）で戻す。
//!   書き込みは保存と同じ口（一時ファイル + rename・権限を引き継ぐ。シンボリックリンクは実体へ書く）
//! - UTF-8 として読めない・読み取り専用のファイルは当てない（文字コードを壊さない）
//! - 取り消しの履歴は新しい順に [`HISTORY_CAP`] 件（GUI が起きているあいだ。退避は
//!   [`BACKUP_KEEP`] 残す）。戻すのは最後に当てた 1 回ぶんで、その後に変わったファイルは戻さない

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::i18n::Lang;
use crate::text_edit::TextChange;
use crate::workspace_search::{excerpt, excerpt_marked, Matcher, Query};

/// 退避の置き場（data dir の下）
pub const BACKUP_DIR: &str = "search-replace";
/// 取り消しの履歴の上限（超えたら古いものから退避ごと捨てる）
pub const HISTORY_CAP: usize = 20;
/// 前のプロセスが残した退避を残す期間（履歴はメモリなので tako からは戻せない。手で拾える期間）
pub const BACKUP_KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// 「当てている最中」の印をあきらめるまで（background の回が着地せずに終わっても、次の置換を
/// 永久に断り続けない。2000 行・8 MB のファイルでも数十秒で終わる量の 10 倍以上）
pub const BUSY_EXPIRE: Duration = Duration::from_secs(10 * 60);

// --- 指紋 -------------------------------------------------------------------

/// 中身の指紋（長さ + 64 bit のハッシュ）。検索の後 / 当てた後に外で変わったかを見る
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stamp {
    pub len: u64,
    pub hash: u64,
}

impl Stamp {
    pub fn of(bytes: &[u8]) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self {
            len: bytes.len() as u64,
            hash: hasher.finish(),
        }
    }
}

// --- 置換後の文字列 ---------------------------------------------------------

/// 置換後の文字列を組めない理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// 改行を含む（行ごとに置き換えるので入れられない）
    Newline,
    /// 無いグループを指した（`$5` / `${name}`。字面をそのまま持つ）
    UnknownGroup(String),
    /// `${` が閉じていない
    Unclosed,
}

impl TemplateError {
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::Newline, Lang::Ja) => "置換後の文字列に改行は使えない（1 行ずつ置き換える）".into(),
            (Self::Newline, Lang::En) => {
                "the replacement cannot contain a line break (lines are replaced one by one)".into()
            }
            (Self::UnknownGroup(group), Lang::Ja) => {
                format!("置換の {group} に当たるグループが検索語に無い（`$` そのものは `$$`）")
            }
            (Self::UnknownGroup(group), Lang::En) => format!(
                "the search has no group for {group} in the replacement (write `$$` for a literal `$`)"
            ),
            (Self::Unclosed, Lang::Ja) => "置換の `${` が閉じていない".into(),
            (Self::Unclosed, Lang::En) => "`${` in the replacement is not closed".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// 番号のグループ（0 = 一致全体）
    Group(usize),
    Named(String),
}

/// 組み上げた置換後の文字列
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

impl Template {
    /// 文字どおりの検索なら `text` をそのまま、正規表現なら `$` の参照を読む
    pub fn new(text: &str, query: &Query, matcher: &Matcher) -> Result<Self, TemplateError> {
        if text.contains(['\n', '\r']) {
            return Err(TemplateError::Newline);
        }
        if !query.regex {
            return Ok(Self {
                pieces: vec![Piece::Text(text.to_string())],
            });
        }
        let regex = matcher.line_regex();
        let groups = regex.captures_len().saturating_sub(1);
        let names: Vec<&str> = regex.capture_names().flatten().collect();
        let mut pieces = Vec::new();
        let mut literal = String::new();
        let flush = |literal: &mut String, pieces: &mut Vec<Piece>| {
            if !literal.is_empty() {
                pieces.push(Piece::Text(std::mem::take(literal)));
            }
        };
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            i += 1;
            if c != '$' {
                literal.push(c);
                continue;
            }
            match chars.get(i).copied() {
                Some('$') => {
                    i += 1;
                    literal.push('$');
                }
                Some('&') => {
                    i += 1;
                    flush(&mut literal, &mut pieces);
                    pieces.push(Piece::Group(0));
                }
                Some('{') => {
                    let close = chars[i + 1..]
                        .iter()
                        .position(|c| *c == '}')
                        .ok_or(TemplateError::Unclosed)?;
                    let name: String = chars[i + 1..i + 1 + close].iter().collect();
                    i += close + 2;
                    let piece = match name.parse::<usize>() {
                        Ok(n) if n <= groups => Piece::Group(n),
                        Err(_) if names.contains(&name.as_str()) => Piece::Named(name),
                        _ => return Err(TemplateError::UnknownGroup(format!("${{{name}}}"))),
                    };
                    flush(&mut literal, &mut pieces);
                    pieces.push(piece);
                }
                Some(d) if d.is_ascii_digit() => {
                    let first = d.to_digit(10).unwrap_or(0) as usize;
                    i += 1;
                    // JS / VS Code と同じ読み: 2 桁がグループとして在ればそれ、無ければ 1 桁
                    let two = chars
                        .get(i)
                        .and_then(|c| c.to_digit(10))
                        .map(|second| first * 10 + second as usize)
                        .filter(|n| (1..=groups).contains(n));
                    let n = match two {
                        Some(n) => {
                            i += 1;
                            n
                        }
                        None if first <= groups => first,
                        None => return Err(TemplateError::UnknownGroup(format!("${first}"))),
                    };
                    flush(&mut literal, &mut pieces);
                    pieces.push(Piece::Group(n));
                }
                // `$` のあとが記号・末尾 = `$` そのもの
                _ => literal.push('$'),
            }
        }
        flush(&mut literal, &mut pieces);
        Ok(Self { pieces })
    }

    fn expand(&self, caps: &regex::Captures<'_>, out: &mut String) {
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Group(n) => out.push_str(caps.get(*n).map_or("", |m| m.as_str())),
                Piece::Named(name) => out.push_str(caps.name(name).map_or("", |m| m.as_str())),
            }
        }
    }
}

// --- 1 ファイルぶんの計画（プレビューと当てるの共通） -----------------------

/// 差分の抜粋の印（GUI は消す字に取り消し線、足す字に色を付ける）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Deleted,
    Inserted,
}

/// 1 行ぶんの置換
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineReplace {
    /// 行（1 始まり）
    pub line: usize,
    /// その行で置き換える一致の数
    pub occurrences: usize,
    /// 置換後の行の抜粋（[`excerpt`] と同じ切り方。CLI / MCP の `replaced`）
    pub after: String,
    /// 置換後の抜粋の中の足した字（文字単位の半開区間）
    pub after_ranges: Vec<(usize, usize)>,
    /// 前後を並べた差分の抜粋（一致の字のすぐ後に置換後の字。GUI の行の表示）
    pub diff: String,
    pub diff_marks: Vec<(usize, usize, Mark)>,
}

/// 1 ファイルに当てる計画
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePlan {
    pub lines: Vec<LineReplace>,
    /// 全文（BOM を含む）のバイト範囲での書き換え（範囲の順・重ならない）
    pub changes: Vec<TextChange>,
    pub occurrences: usize,
    /// 当てても中身が変わらない（置換後が元と同じ）
    pub noop: bool,
}

impl FilePlan {
    /// 計画を当てた全文
    pub fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for change in &self.changes {
            // 切り出し安全: 範囲は plan が str の正規表現の一致から作った文字境界（昇順・重ならない）
            out.push_str(&text[at..change.range.start]);
            out.push_str(&change.text);
            at = change.range.end;
        }
        out.push_str(&text[at..]);
        out
    }
}

/// 全文の `lines` の行（`None` = 全部）に置換を当てる計画を組む（ファイルは読まない）
pub fn plan(
    matcher: &Matcher,
    template: &Template,
    text: &str,
    lines: Option<&BTreeSet<usize>>,
) -> FilePlan {
    let body = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut offset = text.len() - body.len();
    let mut out = FilePlan {
        noop: true,
        ..FilePlan::default()
    };
    for (index, raw) in body.split('\n').enumerate() {
        let start = offset;
        offset += raw.len() + 1;
        let number = index + 1;
        if lines.is_some_and(|wanted| !wanted.contains(&number)) {
            continue;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let mut segments: Vec<(usize, usize, String)> = Vec::new();
        for caps in matcher.line_regex().captures_iter(line) {
            let Some(found) = caps.get(0).filter(|m| m.start() < m.end()) else {
                continue;
            };
            let mut replacement = String::new();
            template.expand(&caps, &mut replacement);
            segments.push((found.start(), found.end(), replacement));
        }
        if segments.is_empty() {
            continue;
        }
        out.occurrences += segments.len();
        out.lines.push(line_replace(line, number, &segments));
        for (s, e, replacement) in segments {
            // 切り出し安全: s..e は captures_iter の一致 = 文字境界
            if line[s..e] != replacement {
                out.noop = false;
            }
            out.changes.push(TextChange {
                range: start + s..start + e,
                text: replacement,
            });
        }
    }
    out
}

/// 1 行ぶんの抜粋（置換後 / 差分）を組む
fn line_replace(line: &str, number: usize, segments: &[(usize, usize, String)]) -> LineReplace {
    let mut after = String::with_capacity(line.len());
    let mut after_ranges = Vec::new();
    let mut diff = String::with_capacity(line.len() * 2);
    let mut diff_marks = Vec::new();
    let mut at = 0;
    for (s, e, replacement) in segments {
        // 切り出し安全: 一致の範囲は文字境界（昇順・重ならない）
        let between = &line[at..*s];
        after.push_str(between);
        diff.push_str(between);
        let a = after.len();
        after.push_str(replacement);
        after_ranges.push((a, after.len()));
        let d = diff.len();
        diff.push_str(&line[*s..*e]);
        diff_marks.push((d, diff.len(), Mark::Deleted));
        let d = diff.len();
        diff.push_str(replacement);
        if !replacement.is_empty() {
            diff_marks.push((d, diff.len(), Mark::Inserted));
        }
        at = *e;
    }
    after.push_str(&line[at..]);
    diff.push_str(&line[at..]);
    let (after, after_ranges) = excerpt(&after, &after_ranges);
    let (diff, diff_marks) = excerpt_marked(&diff, &diff_marks);
    LineReplace {
        line: number,
        occurrences: segments.len(),
        after,
        after_ranges,
        diff,
        diff_marks,
    }
}

// --- ファイル 1 つの判定 ----------------------------------------------------

/// 当てなかった理由（プレビューでも同じ判定で先に言う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// 検索の後に中身が変わった
    Stale,
    /// 開いているペインに保存していない編集がある（その中身は検索が見ていない）
    Unsaved,
    /// UTF-8 として読めない
    NotUtf8,
    /// 読めない・見つからない
    Unreadable,
    /// 読み取り専用
    ReadOnly,
    /// 2 つ以上のペインで編集中
    OpenInSeveral,
    /// 置換しても中身が変わらない
    Unchanged,
    /// 退避を書けなかった（書き換えていない）
    BackupFailed,
    /// 書き込みに失敗した（元の中身のまま）
    WriteFailed,
    /// その行は当て済み
    AlreadyApplied,
    /// 当てる直前にペインで編集された
    Edited,
}

impl Skip {
    pub const ALL: [Skip; 11] = [
        Self::Stale,
        Self::Unsaved,
        Self::NotUtf8,
        Self::Unreadable,
        Self::ReadOnly,
        Self::OpenInSeveral,
        Self::Unchanged,
        Self::BackupFailed,
        Self::WriteFailed,
        Self::AlreadyApplied,
        Self::Edited,
    ];

    /// 応答の `status`
    pub fn slug(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::Unsaved => "unsaved",
            Self::NotUtf8 => "not-utf8",
            Self::Unreadable => "unreadable",
            Self::ReadOnly => "read-only",
            Self::OpenInSeveral => "open-in-several",
            Self::Unchanged => "unchanged",
            Self::BackupFailed => "backup-failed",
            Self::WriteFailed => "write-failed",
            Self::AlreadyApplied => "already-applied",
            Self::Edited => "edited",
        }
    }

    pub fn text_in(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Stale, Lang::Ja) => "検索の後に中身が変わった（検索し直す）",
            (Self::Stale, Lang::En) => "changed after the search (search again)",
            (Self::Unsaved, Lang::Ja) => {
                "開いているペインに保存していない編集がある（保存してから検索し直す）"
            }
            (Self::Unsaved, Lang::En) => {
                "the open pane has unsaved edits (save, then search again)"
            }
            (Self::NotUtf8, Lang::Ja) => "UTF-8 として読めない（文字コードを壊さないよう当てない）",
            (Self::NotUtf8, Lang::En) => "not valid UTF-8 (left untouched to keep its encoding)",
            (Self::Unreadable, Lang::Ja) => "読めない・見つからない",
            (Self::Unreadable, Lang::En) => "cannot be read or no longer exists",
            (Self::ReadOnly, Lang::Ja) => "読み取り専用",
            (Self::ReadOnly, Lang::En) => "read-only",
            (Self::OpenInSeveral, Lang::Ja) => {
                "2 つ以上のペインで編集中（どれへ当てるか決められない）"
            }
            (Self::OpenInSeveral, Lang::En) => {
                "being edited in two or more panes (cannot pick one)"
            }
            (Self::Unchanged, Lang::Ja) => "置換しても中身が変わらない",
            (Self::Unchanged, Lang::En) => "the replacement leaves it unchanged",
            (Self::BackupFailed, Lang::Ja) => "退避を書けなかった（書き換えていない）",
            (Self::BackupFailed, Lang::En) => "could not write the backup (left untouched)",
            (Self::WriteFailed, Lang::Ja) => "書き込みに失敗した（元の中身のまま）",
            (Self::WriteFailed, Lang::En) => "the write failed (the original content is kept)",
            (Self::AlreadyApplied, Lang::Ja) => "当て済み",
            (Self::AlreadyApplied, Lang::En) => "already replaced",
            (Self::Edited, Lang::Ja) => "当てる直前にペインで編集された（検索し直す）",
            (Self::Edited, Lang::En) => "edited in the pane just before applying (search again)",
        }
    }
}

/// ディスクのファイルを読んで計画を組む（**UI スレッドで呼ばない**）。返すのは
/// （書く実体のパス, 今の中身, 計画）。シンボリックリンクは実体へ書く（リンクを普通のファイルに
/// 置き換えない）
pub fn check_disk(
    path: &Path,
    expect: Stamp,
    matcher: &Matcher,
    template: &Template,
    lines: &BTreeSet<usize>,
) -> Result<(PathBuf, Vec<u8>, FilePlan), Skip> {
    let real = crate::platform::path::canonicalize(path).map_err(|_| Skip::Unreadable)?;
    let bytes = std::fs::read(&real).map_err(|_| Skip::Unreadable)?;
    if Stamp::of(&bytes) != expect {
        return Err(Skip::Stale);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Skip::NotUtf8)?;
    let plan = plan(matcher, template, text, Some(lines));
    if plan.noop {
        return Err(Skip::Unchanged);
    }
    if std::fs::metadata(&real).is_ok_and(|m| m.permissions().readonly()) {
        return Err(Skip::ReadOnly);
    }
    Ok((real, bytes, plan))
}

/// 開いている編集バッファの中身で計画を組む（ファイルは読まない）。`dirty` は保存していない
/// 編集があるか（中身が期待と違うときの理由を分ける）
pub fn check_buffer(
    text: &str,
    dirty: bool,
    expect: Stamp,
    matcher: &Matcher,
    template: &Template,
    lines: &BTreeSet<usize>,
) -> Result<FilePlan, Skip> {
    if Stamp::of(text.as_bytes()) != expect {
        return Err(if dirty { Skip::Unsaved } else { Skip::Stale });
    }
    let plan = plan(matcher, template, text, Some(lines));
    if plan.noop {
        return Err(Skip::Unchanged);
    }
    Ok(plan)
}

/// 退避の置き場の 1 回ぶん（`<data_dir>/search-replace/<pid>-<通し番号>`）。番号はプロセス全体で
/// 1 つの数え上げ（記録の番号は置き場ごとに 1 から数えるので、置き場が 2 つあると取り合う）
pub fn next_backup_dir(root: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    root.join(format!("{}-{seq}", std::process::id()))
}

/// 退避のファイル名（同じ名前のファイルが並んでも取り合わない）
pub fn backup_path(dir: &Path, index: usize, real: &Path) -> PathBuf {
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    dir.join(format!("{index:04}-{name}"))
}

/// 中身を退避する（**UI スレッドで呼ばない**）
pub fn write_backup(backup: &Path, bytes: &[u8]) -> Result<(), Skip> {
    let parent = backup.parent().ok_or(Skip::BackupFailed)?;
    std::fs::create_dir_all(parent).map_err(|_| Skip::BackupFailed)?;
    std::fs::write(backup, bytes).map_err(|_| Skip::BackupFailed)
}

/// 退避してから計画を書く（**UI スレッドで呼ばない**）。返すのは書いた後の中身の指紋
pub fn write_disk(
    real: &Path,
    before: &[u8],
    plan: &FilePlan,
    backup: &Path,
) -> Result<Stamp, Skip> {
    let text = std::str::from_utf8(before).map_err(|_| Skip::NotUtf8)?;
    let after = plan.apply(text);
    write_backup(backup, before)?;
    if crate::text_edit::write_file(real, after.as_bytes()).is_err() {
        // 一時ファイル + rename の口（unix）は失敗しても元の中身のまま。その場で書く口（Windows）が
        // 途中まで書いていたら、元の中身で書き戻す
        if std::fs::read(real).map_or(true, |now| now != before) {
            let _ = crate::text_edit::write_file(real, before);
        }
        return Err(Skip::WriteFailed);
    }
    Ok(Stamp::of(after.as_bytes()))
}

/// 前のプロセスが残した古い退避を消す（**UI スレッドで呼ばない**）。返すのは消した数
pub fn prune_backups(root: &Path, keep: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mine = format!("{}-", std::process::id());
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&mine) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > keep);
        if old && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// --- 取り消し ---------------------------------------------------------------

/// 取り消しの 1 ファイルの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    /// 戻せる（プレビュー）
    Restorable,
    Restored,
    /// 既に置換前の中身
    AlreadyRestored,
    /// 置換の後に中身が変わった（上書きになるので戻さない）
    Changed,
    /// 置換の後にペインで編集した（そのペインの undo で戻す）
    Edited,
    /// 退避が見つからない・壊れている
    BackupMissing,
    /// 書き戻しに失敗した
    WriteFailed,
}

impl Restore {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Restorable => "restorable",
            Self::Restored => "restored",
            Self::AlreadyRestored => "already-restored",
            Self::Changed => "changed",
            Self::Edited => "edited",
            Self::BackupMissing => "backup-missing",
            Self::WriteFailed => "write-failed",
        }
    }

    /// 戻した / 戻せる / 既に戻っている（= 履歴から下ろしてよい）
    pub fn settled(self) -> bool {
        matches!(
            self,
            Self::Restorable | Self::Restored | Self::AlreadyRestored
        )
    }

    pub fn text_in(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Restorable, Lang::Ja) => "戻せる",
            (Self::Restorable, Lang::En) => "can be restored",
            (Self::Restored, Lang::Ja) => "戻した",
            (Self::Restored, Lang::En) => "restored",
            (Self::AlreadyRestored, Lang::Ja) => "既に置換前の中身",
            (Self::AlreadyRestored, Lang::En) => "already back to the content before the replacement",
            (Self::Changed, Lang::Ja) => {
                "置換の後に中身が変わった（上書きになるので戻さない。退避から手で戻せる）"
            }
            (Self::Changed, Lang::En) => {
                "changed after the replacement (not restored to avoid overwriting; the backup is kept)"
            }
            (Self::Edited, Lang::Ja) => "置換の後にペインで編集した（そのペインの undo で戻す）",
            (Self::Edited, Lang::En) => "edited in the pane after the replacement (undo it there)",
            (Self::BackupMissing, Lang::Ja) => "退避が見つからない・壊れている",
            (Self::BackupMissing, Lang::En) => "the backup is missing or damaged",
            (Self::WriteFailed, Lang::Ja) => "書き戻しに失敗した",
            (Self::WriteFailed, Lang::En) => "could not write it back",
        }
    }
}

/// ディスクのファイルを退避から戻す（`confirm` が偽なら戻せるかだけを見る。**UI スレッドで呼ばない**）
pub fn restore_disk(
    real: &Path,
    before: Stamp,
    after: Stamp,
    backup: &Path,
    confirm: bool,
) -> Restore {
    let current = match std::fs::read(real) {
        Ok(bytes) => Stamp::of(&bytes),
        // 消されていたら戻さない（作り直すと消した操作を打ち消す）
        Err(_) => return Restore::Changed,
    };
    if current == before {
        return Restore::AlreadyRestored;
    }
    if current != after {
        return Restore::Changed;
    }
    let Ok(saved) = std::fs::read(backup) else {
        return Restore::BackupMissing;
    };
    if Stamp::of(&saved) != before {
        return Restore::BackupMissing;
    }
    if !confirm {
        return Restore::Restorable;
    }
    match crate::text_edit::write_file(real, &saved) {
        Ok(()) => Restore::Restored,
        Err(_) => Restore::WriteFailed,
    }
}

// --- 置き場（SearchStore が持つ） -------------------------------------------

/// 当てた 1 ファイルの記録（取り消しの材料）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFile {
    /// 結果の行のパス（右パネルの行と突き合わせる）
    pub path: PathBuf,
    /// 書いた実体（シンボリックリンクを辿った先）
    pub real: PathBuf,
    /// 当てた行（結果の添字。`Record::seq` の結果のもの）
    pub hits: Vec<usize>,
    pub before: Stamp,
    pub after: Stamp,
    /// 置換前の中身の退避（バッファへ当てたファイルも退避する = ペインを閉じた後でも戻せる）
    pub backup: PathBuf,
    /// バッファへ当てたなら、そのペインと当てた直後の版
    pub buffer: Option<BufferRef>,
}

/// バッファへ当てた先（取り消しで「同じペインが同じファイルを同じ版のまま開いているか」を見る材料）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferRef {
    pub pane: u64,
    pub version: u64,
    /// そのペインが開いていたパス（ペインの表示のまま。UI スレッドで実体を引き直さずに突き合わせる）
    pub path: String,
}

/// 当てた 1 回ぶんの記録
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: u64,
    /// 当てた結果の番号（右パネルの結果に当てたとき。出さない検索は `None`）
    pub seq: Option<u64>,
    pub query: String,
    pub replacement: String,
    pub files: Vec<RecordFile>,
    pub backup_dir: PathBuf,
}

/// 右パネルの 1 行のプレビュー
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowPreview {
    Ready(LineReplace),
    Skipped(Skip),
}

/// 置換欄の文字列で作ったプレビュー（右パネルの行に並べる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub seq: u64,
    /// 作った時点の世代（当てる / 戻すたびに進む。古い世代のプレビューは置かない）
    pub generation: u64,
    pub replacement: String,
    /// 結果の添字 → その行のプレビュー
    pub rows: BTreeMap<usize, RowPreview>,
    /// 当てなかった理由（置換欄の文字列そのものが組めない = 正規表現の参照の誤り）
    pub error: Option<TemplateError>,
}

/// 出している結果に付く置換の状態
#[derive(Debug, Default)]
struct Session {
    seq: u64,
    generation: u64,
    applied: BTreeSet<usize>,
    /// 当てた後の中身の指紋（結果の指紋より優先）
    expect: HashMap<PathBuf, Stamp>,
    preview: Option<Preview>,
}

/// 置換の状態（`SearchStore` が 1 つ持つ）
#[derive(Debug, Default)]
pub struct ReplaceState {
    session: Session,
    history: Vec<Record>,
    next_id: u64,
    /// 当てている / 戻している最中の 1 回と立てた時刻（その間は次を受けない = 同じファイルを
    /// 取り合わない。[`BUSY_EXPIRE`] を過ぎた印は見ない）
    busy: Option<(u64, std::time::Instant)>,
    /// 置いたプレビューの数（GUI が「外から置かれた」プレビューを置換欄へ写す目印）
    previews_placed: u64,
    /// 戻し終えて履歴から下ろした記録の退避の置き場（次の background の回で消す）
    trash: Vec<PathBuf>,
}

impl ReplaceState {
    /// 出している結果が変わった / 閉じた（当て済みとプレビューを手放す。履歴は残す）
    pub fn reset_session(&mut self, seq: u64) {
        self.session = Session {
            seq,
            generation: self.session.generation + 1,
            ..Session::default()
        };
    }

    pub fn seq(&self) -> u64 {
        self.session.seq
    }

    pub fn generation(&self) -> u64 {
        self.session.generation
    }

    /// その結果のそのファイルが今あるべき中身（当てた後なら当てた後、でなければ検索の時点）
    pub fn expected(&self, seq: u64, path: &Path, searched: Stamp) -> Stamp {
        if seq == self.session.seq {
            if let Some(stamp) = self.session.expect.get(path) {
                return *stamp;
            }
        }
        searched
    }

    pub fn is_applied(&self, seq: u64, hit: usize) -> bool {
        seq == self.session.seq && self.session.applied.contains(&hit)
    }

    /// 出している結果の当て済みの行
    pub fn applied(&self) -> &BTreeSet<usize> {
        &self.session.applied
    }

    /// 当て始める（最中なら断る = `Err(最中の番号)`）。返すのは記録の番号
    pub fn begin(&mut self) -> Result<u64, u64> {
        if let Some(busy) = self.busy() {
            return Err(busy);
        }
        self.next_id += 1;
        self.busy = Some((self.next_id, std::time::Instant::now()));
        Ok(self.next_id)
    }

    /// 当て終えた / 戻し終えた（`id` が最中の番号なら最中を解く）
    pub fn end(&mut self, id: u64) {
        if self.busy.is_some_and(|(busy, _)| busy == id) {
            self.busy = None;
        }
    }

    /// 最中の番号（[`BUSY_EXPIRE`] を過ぎた印は無いものとして扱う）
    pub fn busy(&self) -> Option<u64> {
        self.busy
            .filter(|(_, since)| since.elapsed() < BUSY_EXPIRE)
            .map(|(id, _)| id)
    }

    /// 上限まで履歴を下ろし、下ろしたぶん（と戻し終えた記録）の退避の置き場を返す
    /// （呼び手が background で消す。UI スレッドでファイルを消さない）
    pub fn take_evicted(&mut self) -> Vec<PathBuf> {
        let mut out = std::mem::take(&mut self.trash);
        while self.history.len() >= HISTORY_CAP {
            out.push(self.history.remove(0).backup_dir);
        }
        out
    }

    /// 当てた 1 回を記録する。出している結果に当てたなら当て済みの行と指紋も進める
    pub fn push(&mut self, record: Record) {
        if record.seq == Some(self.session.seq) {
            for file in &record.files {
                self.session.applied.extend(file.hits.iter().copied());
                self.session.expect.insert(file.path.clone(), file.after);
            }
        }
        self.session.generation += 1;
        self.session.preview = None;
        self.history.push(record);
    }

    /// 取り消しの履歴（古い順）
    pub fn history(&self) -> &[Record] {
        &self.history
    }

    /// 最後に当てた 1 回
    pub fn last(&self) -> Option<&Record> {
        self.history.last()
    }

    /// 最後の 1 回を取り消した。戻したファイル（`restored` の添字）の当て済みと指紋を戻し、
    /// 全部片付いたら履歴から下ろす（下ろしたら `true`）。戻せなかったファイルが残る記録は
    /// 戻したファイルを外して残す（もう一度戻すと残りだけを見る）
    pub fn settle_last(&mut self, id: u64, settled: &[usize]) -> bool {
        let Some(record) = self.history.last_mut().filter(|r| r.id == id) else {
            return false;
        };
        let mut kept = Vec::new();
        for (index, file) in std::mem::take(&mut record.files).into_iter().enumerate() {
            if !settled.contains(&index) {
                kept.push(file);
                continue;
            }
            if record.seq == Some(self.session.seq) {
                for hit in &file.hits {
                    self.session.applied.remove(hit);
                }
                self.session.expect.insert(file.path.clone(), file.before);
            }
        }
        record.files = kept;
        self.session.generation += 1;
        self.session.preview = None;
        if record.files.is_empty() {
            if let Some(done) = self.history.pop() {
                self.trash.push(done.backup_dir);
            }
            true
        } else {
            false
        }
    }

    /// プレビューを置く（出している結果・同じ世代のものだけ）。置いたら `true`
    pub fn set_preview(&mut self, preview: Preview) -> bool {
        if preview.seq != self.session.seq || preview.generation != self.session.generation {
            return false;
        }
        self.session.preview = Some(preview);
        self.previews_placed += 1;
        true
    }

    /// 出している結果のプレビュー
    pub fn preview(&self) -> Option<&Preview> {
        self.session.preview.as_ref()
    }

    pub fn previews_placed(&self) -> u64 {
        self.previews_placed
    }

    /// プレビューを下ろす（置換欄を閉じた）
    pub fn clear_preview(&mut self) {
        self.session.preview = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_residue::ScratchDir;
    use crate::workspace_search::{search, Options};
    use std::sync::atomic::AtomicBool;

    fn query(text: &str, regex: bool) -> Query {
        Query {
            text: text.into(),
            regex,
            case_sensitive: true,
        }
    }

    fn template(q: &Query, replacement: &str) -> Result<Template, TemplateError> {
        Template::new(replacement, q, &Matcher::new(q).unwrap())
    }

    /// 全文へ全行の置換を当てた結果
    fn replace_all(text: &str, q: &Query, replacement: &str) -> String {
        let matcher = Matcher::new(q).unwrap();
        let template = Template::new(replacement, q, &matcher).unwrap();
        plan(&matcher, &template, text, None).apply(text)
    }

    fn lines(list: &[usize]) -> BTreeSet<usize> {
        list.iter().copied().collect()
    }

    /// 受け入れ条件 1-a: 文字どおりの置換の全文が固定値と一致する（`$` も `.` もただの文字）
    #[test]
    fn 文字どおりの置換の全文が固定値と一致する() {
        let text = "let a = foo.bar;\nfoo.bar($1)\nfooXbar\n";
        assert_eq!(
            replace_all(text, &query("foo.bar", false), "$1.baz"),
            "let a = $1.baz;\n$1.baz($1)\nfooXbar\n"
        );
    }

    /// 受け入れ条件 1-b: 正規表現とキャプチャ（番号・名前・一致全体・`$$`・`$1a` の読み）
    #[test]
    fn 正規表現とキャプチャの全文が固定値と一致する() {
        let q = query(r"(\w+)\.get\((?P<key>\d+)\)", true);
        let text = "a.get(1) + bb.get(22)\nnone\n";
        assert_eq!(replace_all(text, &q, "${key}@$1"), "1@a + 22@bb\nnone\n");
        assert_eq!(
            replace_all(text, &q, "[$0]"),
            "[a.get(1)] + [bb.get(22)]\nnone\n"
        );
        assert_eq!(
            replace_all(text, &q, "$&!"),
            "a.get(1)! + bb.get(22)!\nnone\n"
        );
        assert_eq!(replace_all(text, &q, "$$1"), "$1 + $1\nnone\n");
        // `$1a` は `$1` + `a`（Rust の regex の `$1a` = 名前 1a ではない）
        assert_eq!(replace_all(text, &q, "$1a"), "aa + bba\nnone\n");
        // `$` のあとが記号・末尾なら `$` そのもの
        assert_eq!(replace_all("x1\n", &query(r"x(\d)", true), "$-$"), "$-$\n");
        // 2 桁は在れば 2 桁、無ければ 1 桁 + 字
        assert_eq!(replace_all("ab\n", &query("(a)(b)", true), "$12"), "a2\n");
    }

    #[test]
    fn 無いグループと改行と閉じていない括弧は組めない() {
        let q = query(r"(a)", true);
        assert_eq!(
            template(&q, "$2").unwrap_err(),
            TemplateError::UnknownGroup("$2".into())
        );
        assert_eq!(
            template(&q, "${nope}").unwrap_err(),
            TemplateError::UnknownGroup("${nope}".into())
        );
        assert_eq!(template(&q, "${1").unwrap_err(), TemplateError::Unclosed);
        assert_eq!(template(&q, "a\nb").unwrap_err(), TemplateError::Newline);
        assert_eq!(
            template(&query("a", false), "x\r").unwrap_err(),
            TemplateError::Newline
        );
        // 文字どおりの検索では `$2` もただの文字
        assert!(template(&query("a", false), "$2").is_ok());
        // 理由は日英とも 1 行
        for e in [TemplateError::Newline, TemplateError::Unclosed] {
            for lang in [Lang::Ja, Lang::En] {
                assert!(!e.text_in(lang).contains('\n'));
            }
        }
    }

    /// CRLF・BOM・多バイト文字を壊さない（書き換えるのは一致の範囲だけ）
    #[test]
    fn crlfとbomと多バイト文字を壊さない() {
        let text = "\u{FEFF}一行目 foo\r\n二行目 foo です\r\n";
        let out = replace_all(text, &query("foo", false), "ばー");
        assert_eq!(out, "\u{FEFF}一行目 ばー\r\n二行目 ばー です\r\n");
        // `$` は `\r` の手前（検索と同じ行の切り方）
        let out = replace_all("a foo\r\nb foo\r\n", &query("foo$", true), "X");
        assert_eq!(out, "a X\r\nb X\r\n");
    }

    #[test]
    fn 行を絞ると他の行は触らない_幅0の一致は数えない() {
        let q = query("x*", true);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("-", &q, &matcher).unwrap();
        let text = "axb\nxx\naxb\n";
        let p = plan(&matcher, &t, text, Some(&lines(&[1, 3])));
        assert_eq!(p.apply(text), "a-b\nxx\na-b\n");
        assert_eq!(p.occurrences, 2);
        assert_eq!(
            p.lines.iter().map(|l| l.line).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn プレビューの抜粋は置換後と差分を持つ() {
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("quux", &q, &matcher).unwrap();
        let p = plan(&matcher, &t, "    let foo = foo;\n", None);
        let line = &p.lines[0];
        assert_eq!(line.after, "let quux = quux;");
        assert_eq!(line.after_ranges, vec![(4, 8), (11, 15)]);
        assert_eq!(line.diff, "let fooquux = fooquux;");
        assert_eq!(
            line.diff_marks,
            vec![
                (4, 7, Mark::Deleted),
                (7, 11, Mark::Inserted),
                (14, 17, Mark::Deleted),
                (17, 21, Mark::Inserted)
            ]
        );
        // 消すだけ（空の置換）は足す字の印を持たない
        let t = Template::new("", &q, &matcher).unwrap();
        let p = plan(&matcher, &t, "a foo b\n", None);
        assert_eq!(p.lines[0].after, "a  b");
        assert_eq!(p.lines[0].diff_marks, vec![(2, 5, Mark::Deleted)]);
    }

    #[test]
    fn 置換後が元と同じなら変わらない() {
        let q = query("(a)", true);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("$1", &q, &matcher).unwrap();
        let text = "a\n";
        assert!(plan(&matcher, &t, text, None).noop);
        assert_eq!(
            check_buffer(
                text,
                false,
                Stamp::of(text.as_bytes()),
                &matcher,
                &t,
                &lines(&[1])
            )
            .unwrap_err(),
            Skip::Unchanged
        );
    }

    /// 受け入れ条件 3 の土台: 検索の後に外で変わったファイルは当てない（ディスク / バッファ）
    #[test]
    fn 検索の後に変わったファイルは当てない() {
        let dir = ScratchDir::new("2045-stale");
        let path = dir.join("a.txt");
        std::fs::write(&path, "foo\n").unwrap();
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("bar", &q, &matcher).unwrap();
        let searched = Stamp::of(b"foo\n");
        assert!(check_disk(&path, searched, &matcher, &t, &lines(&[1])).is_ok());
        std::fs::write(&path, "foo\nfoo\n").unwrap();
        assert_eq!(
            check_disk(&path, searched, &matcher, &t, &lines(&[1])).unwrap_err(),
            Skip::Stale
        );
        // バッファ: 中身が違えば、保存していなければ Unsaved・保存済みなら Stale
        assert_eq!(
            check_buffer("foo\nedit\n", true, searched, &matcher, &t, &lines(&[1])).unwrap_err(),
            Skip::Unsaved
        );
        assert_eq!(
            check_buffer("foo\nedit\n", false, searched, &matcher, &t, &lines(&[1])).unwrap_err(),
            Skip::Stale
        );
        assert!(check_buffer("foo\n", true, searched, &matcher, &t, &lines(&[1])).is_ok());
    }

    #[test]
    fn utf8でないファイルは当てない() {
        let dir = ScratchDir::new("2045-sjis");
        let path = dir.join("sjis.txt");
        // Shift_JIS の「あ」+ foo
        let bytes = b"\x82\xa0 foo\n".to_vec();
        std::fs::write(&path, &bytes).unwrap();
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("bar", &q, &matcher).unwrap();
        assert_eq!(
            check_disk(&path, Stamp::of(&bytes), &matcher, &t, &lines(&[1])).unwrap_err(),
            Skip::NotUtf8
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "1 バイトも触らない");
    }

    /// 受け入れ条件 4 の土台: 書く前に退避し、取り消しで戻す。戻した後・変わった後は戻さない
    #[test]
    fn 退避して書き_退避から戻す() {
        let dir = ScratchDir::new("2045-backup");
        let path = dir.join("a.txt");
        let before = b"x foo\r\ny\r\n".to_vec();
        std::fs::write(&path, &before).unwrap();
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("bar", &q, &matcher).unwrap();
        let (real, bytes, p) =
            check_disk(&path, Stamp::of(&before), &matcher, &t, &lines(&[1])).unwrap();
        let backup = backup_path(&dir.join("bk"), 1, &real);
        let after = write_disk(&real, &bytes, &p, &backup).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"x bar\r\ny\r\n");
        assert_eq!(std::fs::read(&backup).unwrap(), before);
        assert_eq!(after, Stamp::of(b"x bar\r\ny\r\n"));
        let b = Stamp::of(&before);
        assert_eq!(
            restore_disk(&real, b, after, &backup, false),
            Restore::Restorable
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"x bar\r\ny\r\n",
            "プレビューは書かない"
        );
        assert_eq!(
            restore_disk(&real, b, after, &backup, true),
            Restore::Restored
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            restore_disk(&real, b, after, &backup, true),
            Restore::AlreadyRestored
        );
        // 置換の後に変わったら戻さない（上書きしない）
        std::fs::write(&path, "x bar\r\nmine\r\n").unwrap();
        assert_eq!(
            restore_disk(&real, b, after, &backup, true),
            Restore::Changed
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"x bar\r\nmine\r\n");
    }

    #[cfg(unix)]
    #[test]
    fn シンボリックリンクは実体へ書き_リンクは残す() {
        let dir = ScratchDir::new("2045-link");
        let target = dir.join("real.txt");
        std::fs::write(&target, "foo\n").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("bar", &q, &matcher).unwrap();
        let (real, bytes, p) =
            check_disk(&link, Stamp::of(b"foo\n"), &matcher, &t, &lines(&[1])).unwrap();
        write_disk(&real, &bytes, &p, &dir.join("bk/0001")).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "bar\n");
    }

    #[cfg(unix)]
    #[test]
    fn 読み取り専用は当てない() {
        use std::os::unix::fs::PermissionsExt;
        let dir = ScratchDir::new("2045-ro");
        let path = dir.join("ro.txt");
        std::fs::write(&path, "foo\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let q = query("foo", false);
        let matcher = Matcher::new(&q).unwrap();
        let t = Template::new("bar", &q, &matcher).unwrap();
        let verdict = check_disk(&path, Stamp::of(b"foo\n"), &matcher, &t, &lines(&[1]));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(verdict.unwrap_err(), Skip::ReadOnly);
    }

    /// 検索の指紋は置換が読み直す中身と同じ単位（BOM を含む全バイト）
    #[test]
    fn 検索の指紋はファイルの全バイト() {
        let dir = ScratchDir::new("2045-stamp");
        std::fs::write(dir.join("a.txt"), "\u{FEFF}foo\r\n").unwrap();
        std::fs::write(dir.join("b.txt"), "nothing\n").unwrap();
        let q = query("foo", false);
        let mut options = Options::new(vec![dir.to_path_buf()]);
        options.git_global = false;
        let outcome = search(
            &Matcher::new(&q).unwrap(),
            &options,
            &AtomicBool::new(false),
        );
        let stamps: Vec<(String, Stamp)> = outcome
            .stamps
            .iter()
            .map(|(p, s)| (p.file_name().unwrap().to_string_lossy().into_owned(), *s))
            .collect();
        assert_eq!(
            stamps,
            vec![("a.txt".into(), Stamp::of("\u{FEFF}foo\r\n".as_bytes()))]
        );
    }

    fn record(id: u64, seq: Option<u64>, hits: Vec<usize>) -> Record {
        Record {
            id,
            seq,
            query: "q".into(),
            replacement: "r".into(),
            files: vec![RecordFile {
                path: "/x".into(),
                real: "/x".into(),
                hits,
                before: Stamp::of(b"a"),
                after: Stamp::of(b"b"),
                backup: "/bk/x".into(),
                buffer: None,
            }],
            backup_dir: format!("/bk/{id}").into(),
        }
    }

    #[test]
    fn 置き場は当て済みと指紋を進め_取り消しで戻す() {
        let mut state = ReplaceState::default();
        state.reset_session(3);
        let id = state.begin().unwrap();
        assert_eq!(state.begin(), Err(id), "当てている最中は次を受けない");
        state.push(record(id, Some(3), vec![0, 2]));
        state.end(id);
        assert!(state.is_applied(3, 2) && !state.is_applied(3, 1));
        assert_eq!(
            state.expected(3, Path::new("/x"), Stamp::of(b"a")),
            Stamp::of(b"b")
        );
        // 別の結果の番号では効かない
        assert!(!state.is_applied(4, 2));
        assert!(state.settle_last(id, &[0]));
        assert_eq!(
            state.take_evicted(),
            vec![PathBuf::from("/bk/1")],
            "戻し終えた退避は次の回で消す"
        );
        assert!(!state.is_applied(3, 2));
        assert_eq!(
            state.expected(3, Path::new("/x"), Stamp::of(b"z")),
            Stamp::of(b"a")
        );
        assert!(state.history().is_empty());
        // 次の結果で当て済みは手放すが、履歴は残る
        let id = state.begin().unwrap();
        state.push(record(id, Some(3), vec![1]));
        state.end(id);
        state.reset_session(4);
        assert!(state.applied().is_empty());
        assert_eq!(state.history().len(), 1);
        // 古い世代のプレビューは置かない
        let preview = |generation| Preview {
            seq: 4,
            generation,
            replacement: "r".into(),
            rows: BTreeMap::new(),
            error: None,
        };
        assert!(!state.set_preview(preview(state.generation() - 1)));
        assert!(state.set_preview(preview(state.generation())));
        assert_eq!(state.previews_placed(), 1);
    }

    #[test]
    fn 履歴は上限で古いものから下ろす() {
        let mut state = ReplaceState::default();
        for _ in 0..HISTORY_CAP {
            let id = state.begin().unwrap();
            state.push(record(id, None, vec![]));
            state.end(id);
        }
        let evicted = state.take_evicted();
        assert_eq!(
            evicted,
            vec![PathBuf::from(format!("/bk/{}", state.history()[0].id - 1))]
        );
        assert_eq!(state.history().len(), HISTORY_CAP - 1);
    }

    #[test]
    fn 理由は日英で網羅する() {
        for skip in Skip::ALL {
            assert!(!skip.text_in(Lang::Ja).is_empty() && !skip.text_in(Lang::En).is_empty());
            assert!(
                !skip.text_in(Lang::En).chars().any(|c| c > '\u{2000}'),
                "{skip:?}"
            );
        }
    }
}

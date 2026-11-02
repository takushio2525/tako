//! ワークスペース横断置換（#2045）の制御プレーン側: ジョブと応答の形
//!
//! 置換の当て方の本体は `tako_core::workspace_replace`。ここは dispatch の 3 段
//! （`prepare_offload` → background → `finish_offload`）が受け渡すジョブと、CLI / MCP / GUI が読む
//! 応答の JSON を持つ。
//!
//! - **ファイルの読み書き（照合・退避・書き込み・書き戻し）は [`ReplaceJob::run`] /
//!   [`UndoJob::run`] の中だけ**（UI スレッドで呼ばない。番犬 `issue1694_workspace_search_watchdog`）
//! - 開いている編集バッファへ当てる（`apply_preview_changes`）・戻す（`preview_undo`）のは
//!   UI スレッドの着地（dispatch の `workspace_replace_land` / `workspace_undo_land`）。
//!   background はバッファの写しで計画を組むだけで、バッファに触らない

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};
use tako_core::i18n::Lang;
use tako_core::platform::support::Note;
use tako_core::text_edit::TextChange;
use tako_core::workspace_replace::{
    self as replace, FilePlan, Restore, RowPreview, Skip, Stamp, Template, TemplateError,
};
use tako_core::workspace_search::{self, Hit, Matcher, Options, Outcome, Query, QueryError, Stop};
use tako_core::PaneId;

/// 当てる範囲（CLI `--item N` / `--file <path>` / 省略で全部）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    /// 結果の `item` 番目（1 始まり）の 1 行
    Item(usize),
    /// そのファイルの行（結果の `path` か表示の相対パス）
    File(String),
}

impl Scope {
    pub fn slug(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Item(_) => "item",
            Self::File(_) => "file",
        }
    }

    /// 結果の行（添字 0 始まり）がこの範囲に入るか
    pub fn takes_hit(&self, index: usize, hit: &Hit) -> bool {
        match self {
            Self::All => true,
            Self::Item(item) => index + 1 == *item,
            Self::File(file) => {
                Path::new(file) == hit.path || hit.rel.replace('\\', "/") == file.replace('\\', "/")
            }
        }
    }
}

/// 開いている編集バッファの写し（UI スレッドで採る。background はこれで計画を組む）
#[derive(Debug, Clone)]
pub struct BufferSnap {
    pub pane: PaneId,
    pub path: String,
    pub text: String,
    pub version: u64,
    pub dirty: bool,
}

/// 当てる対象の出どころ
pub enum Source {
    /// 検索してから当てる（`query` 付き。`show` なら右パネルへ出す検索の番号）
    Fresh {
        query: Query,
        options: Options,
        cancel: Arc<AtomicBool>,
        show: Option<u64>,
    },
    /// 右パネルに出している結果に当てる
    Shown {
        seq: u64,
        query: Query,
        roots: Vec<PathBuf>,
        /// 範囲に入る行（結果の添字, 行）。当て済みの行は含めない
        hits: Vec<(usize, Hit)>,
        /// 範囲に入るが当て済みの行（応答に `already-applied` で並べる）
        already: Vec<(usize, Hit)>,
        /// 今あるべき中身（当てた後なら当てた後の指紋）
        expect: HashMap<PathBuf, Stamp>,
        stopped: Option<Stop>,
        total: usize,
    },
}

/// background で走らせる 1 回の置換（プレビューも同じ形 = `confirm: false`）
pub struct ReplaceJob {
    pub(crate) source: Source,
    pub(crate) replacement: String,
    pub(crate) scope: Scope,
    pub(crate) confirm: bool,
    pub(crate) buffers: Vec<BufferSnap>,
    /// 当てるときの記録の番号と、この回の退避の置き場（`<data_dir>/search-replace/<pid>-<n>`）
    pub(crate) record: Option<(u64, PathBuf)>,
    /// 履歴の上限で下ろした記録の退避（background で消す）
    pub(crate) evicted: Vec<PathBuf>,
    pub(crate) limit: Option<usize>,
    /// 作った時点の置換の世代（プレビューを置くかの判定）
    pub(crate) generation: u64,
}

/// 1 ファイルの行き先
#[derive(Debug, Clone)]
pub enum FileStatus {
    /// プレビュー: 当てられる（`pane` はバッファへ当てるときのペイン）
    Ready {
        pane: Option<PaneId>,
    },
    /// ディスクへ書いた
    Written {
        real: PathBuf,
        before: Stamp,
        after: Stamp,
        backup: PathBuf,
    },
    /// バッファへ当てる（UI スレッドの着地で `apply_preview_changes`）
    Buffer {
        pane: PaneId,
        /// そのペインが開いているパス（ペインの表示のまま）
        pane_path: String,
        version: u64,
        changes: Vec<TextChange>,
        real: PathBuf,
        before: Stamp,
        after: Stamp,
        backup: PathBuf,
    },
    /// バッファへ当てた（着地の後）
    BufferApplied {
        pane: PaneId,
        pane_path: String,
        version: u64,
        real: PathBuf,
        before: Stamp,
        after: Stamp,
        backup: PathBuf,
    },
    Skipped(Skip),
}

impl FileStatus {
    fn slug(&self, confirm: bool) -> &'static str {
        match self {
            Self::Ready { .. } => {
                if confirm {
                    "ready"
                } else {
                    "preview"
                }
            }
            Self::Written { .. } | Self::BufferApplied { .. } | Self::Buffer { .. } => "replaced",
            Self::Skipped(skip) => skip.slug(),
        }
    }

    fn pane(&self) -> Option<PaneId> {
        match self {
            Self::Ready { pane } => *pane,
            Self::Buffer { pane, .. } | Self::BufferApplied { pane, .. } => Some(*pane),
            _ => None,
        }
    }

    fn applied(&self) -> bool {
        matches!(self, Self::Written { .. } | Self::BufferApplied { .. })
    }
}

/// 1 ファイルの結果
#[derive(Debug, Clone)]
pub struct FileResult {
    /// 結果の行のパス
    pub path: PathBuf,
    /// 範囲に入る行の添字
    pub hits: Vec<usize>,
    pub status: FileStatus,
    /// 当てる / 当てた計画（理由つきで当てなかったファイルは `None`）
    pub plan: Option<FilePlan>,
}

/// 置換の付帯情報（応答に載せる・着地で置き場へ反映する）
pub struct ReplaceMeta {
    pub(crate) query: Query,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) replacement: String,
    pub(crate) scope: Scope,
    pub(crate) confirm: bool,
    pub(crate) limit: Option<usize>,
    /// 作った時点の置換の世代（プレビューを置くかの判定）
    pub(crate) generation: u64,
    /// どの結果に当てたか（右パネルの結果 / 出す検索の番号）
    pub(crate) seq: Option<u64>,
    /// 当てるときの記録の番号と退避の置き場
    pub(crate) record: Option<(u64, PathBuf)>,
}

/// 置換の結果（UI スレッドの `workspace_replace_land` が置き場とバッファへ反映する）
pub struct ReplaceLanding {
    pub(crate) meta: ReplaceMeta,
    /// 出す検索（`Fresh` + `show`）の答え（着地で置き場へ置く）
    pub(crate) fresh: Option<Result<Outcome, QueryError>>,
    pub(crate) result: Result<ReplaceBody, ReplaceFailure>,
}

/// 置換の本体の結果
pub struct ReplaceBody {
    /// 範囲に入る行（結果の添字, 行）。当て済みの行も含む（`already`）
    pub hits: Vec<(usize, Hit)>,
    pub already: BTreeSet<usize>,
    pub files: Vec<FileResult>,
    pub stopped: Option<Stop>,
    /// 結果の行の数（範囲の外も含む）
    pub total: usize,
}

/// 置換を始められなかった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceFailure {
    Query(QueryError),
    Template(TemplateError),
}

impl ReplaceFailure {
    pub fn text_in(&self, lang: Lang) -> String {
        match self {
            // 検索語の誤りは検索と同じ文面（#1694 の `QueryError::reason`）
            Self::Query(e) => e.reason(),
            Self::Template(e) => e.text_in(lang),
        }
    }
}

impl ReplaceJob {
    /// 照合し、プレビューなら計画を、当てるならディスクへ書く（**UI スレッドで呼ばない**）。
    /// 開いているバッファへは書かない（計画を着地へ渡す）
    pub fn run(self) -> ReplaceLanding {
        let ReplaceJob {
            source,
            replacement,
            scope,
            confirm,
            buffers,
            record,
            evicted,
            limit,
            generation,
        } = self;
        for dir in &evicted {
            remove_backup_dir(dir);
        }
        let (query, roots, seq, fresh, prepared) = match source {
            Source::Fresh {
                query,
                options,
                cancel,
                show,
            } => {
                let roots = options.roots.clone();
                let searched = Matcher::new(&query).map(|matcher| {
                    (
                        workspace_search::search(&matcher, &options, &cancel),
                        matcher,
                    )
                });
                match searched {
                    Err(e) => {
                        let failure = ReplaceFailure::Query(e.clone());
                        return ReplaceLanding {
                            meta: ReplaceMeta {
                                query,
                                roots,
                                replacement,
                                scope,
                                confirm,
                                limit,
                                generation,
                                seq: show,
                                record,
                            },
                            fresh: show.map(|_| Err(e)),
                            result: Err(failure),
                        };
                    }
                    Ok((outcome, matcher)) => {
                        // 取り消された検索（次の検索に追い越された）の途中までの行には当てない
                        let cancelled = outcome.stopped == Some(Stop::Cancelled);
                        let hits: Vec<(usize, Hit)> = outcome
                            .hits
                            .iter()
                            .cloned()
                            .enumerate()
                            .filter(|(i, h)| !cancelled && scope.takes_hit(*i, h))
                            .collect();
                        let expect: HashMap<PathBuf, Stamp> = outcome
                            .stamps
                            .iter()
                            .map(|(p, s)| (p.clone(), *s))
                            .collect();
                        let prepared = Prepared {
                            matcher,
                            hits,
                            already: Vec::new(),
                            expect,
                            stopped: outcome.stopped,
                            total: outcome.hits.len(),
                        };
                        (query, roots, show, show.map(|_| Ok(outcome)), Ok(prepared))
                    }
                }
            }
            Source::Shown {
                seq,
                query,
                roots,
                hits,
                already,
                expect,
                stopped,
                total,
            } => {
                let prepared = Matcher::new(&query)
                    .map(|matcher| Prepared {
                        matcher,
                        hits,
                        already,
                        expect,
                        stopped,
                        total,
                    })
                    .map_err(ReplaceFailure::Query);
                (query, roots, Some(seq), None, prepared)
            }
        };
        let result = prepared.and_then(|prepared| {
            let template = Template::new(&replacement, &query, &prepared.matcher)
                .map_err(ReplaceFailure::Template)?;
            Ok(prepared.apply(&template, confirm, &buffers, record.as_ref()))
        });
        if let Some(root) = record.as_ref().and_then(|(_, dir)| dir.parent()) {
            replace::prune_backups(root, replace::BACKUP_KEEP);
        }
        ReplaceLanding {
            meta: ReplaceMeta {
                query,
                roots,
                replacement,
                scope,
                confirm,
                limit,
                generation,
                seq,
                record,
            },
            fresh,
            result,
        }
    }
}

/// 照合器を組み、範囲を決めたところ
struct Prepared {
    matcher: Matcher,
    hits: Vec<(usize, Hit)>,
    already: Vec<(usize, Hit)>,
    expect: HashMap<PathBuf, Stamp>,
    stopped: Option<Stop>,
    total: usize,
}

impl Prepared {
    fn apply(
        self,
        template: &Template,
        confirm: bool,
        buffers: &[BufferSnap],
        record: Option<&(u64, PathBuf)>,
    ) -> ReplaceBody {
        // 開いているバッファ（実体のパスで突き合わせる）
        let mut open: HashMap<PathBuf, Vec<&BufferSnap>> = HashMap::new();
        for snap in buffers {
            let real = tako_core::platform::path::canonicalize_or_self(Path::new(&snap.path));
            open.entry(real).or_default().push(snap);
        }
        // ファイルごとに束ねる（結果は（ルート, パス, 行）順 = 初出の順がファイルの順）
        let mut order: Vec<PathBuf> = Vec::new();
        let mut by_file: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        let mut lines: HashMap<PathBuf, BTreeSet<usize>> = HashMap::new();
        for (index, hit) in &self.hits {
            if !by_file.contains_key(&hit.path) {
                order.push(hit.path.clone());
            }
            by_file.entry(hit.path.clone()).or_default().push(*index);
            lines.entry(hit.path.clone()).or_default().insert(hit.line);
        }
        let backup_dir = record.map(|(_, dir)| dir.clone());
        let mut files = Vec::new();
        for (n, path) in order.into_iter().enumerate() {
            let hits = by_file.remove(&path).unwrap_or_default();
            let wanted = lines.remove(&path).unwrap_or_default();
            let (status, plan) = match self.expect.get(&path) {
                None => (FileStatus::Skipped(Skip::Stale), None),
                Some(expect) => one_file(
                    &path,
                    *expect,
                    &self.matcher,
                    template,
                    &wanted,
                    &open,
                    confirm,
                    backup_dir.as_deref(),
                    n + 1,
                ),
            };
            files.push(FileResult {
                path,
                hits,
                status,
                plan,
            });
        }
        // 1 つも書かなかった回の空の置き場を残さない
        if let Some(dir) = &backup_dir {
            let _ = std::fs::remove_dir(dir);
        }
        let already: BTreeSet<usize> = self.already.iter().map(|(i, _)| *i).collect();
        let mut hits = self.hits;
        hits.extend(self.already);
        hits.sort_by_key(|(i, _)| *i);
        ReplaceBody {
            hits,
            already,
            files,
            stopped: self.stopped,
            total: self.total,
        }
    }
}

/// ファイル 1 つ（バッファへ当てるなら計画を組むだけ、ディスクなら退避して書く）
#[allow(clippy::too_many_arguments)]
fn one_file(
    path: &Path,
    expect: Stamp,
    matcher: &Matcher,
    template: &Template,
    lines: &BTreeSet<usize>,
    open: &HashMap<PathBuf, Vec<&BufferSnap>>,
    confirm: bool,
    backup_dir: Option<&Path>,
    index: usize,
) -> (FileStatus, Option<FilePlan>) {
    let real = tako_core::platform::path::canonicalize_or_self(path);
    match open.get(&real).map(Vec::as_slice) {
        Some([snap]) => {
            let plan = match replace::check_buffer(
                &snap.text, snap.dirty, expect, matcher, template, lines,
            ) {
                Ok(plan) => plan,
                Err(skip) => return (FileStatus::Skipped(skip), None),
            };
            let Some(dir) = backup_dir.filter(|_| confirm) else {
                return (
                    FileStatus::Ready {
                        pane: Some(snap.pane),
                    },
                    Some(plan),
                );
            };
            // バッファへ当てるファイルも置換前の中身を退避する（ペインを閉じた後でも戻せる）
            let backup = replace::backup_path(dir, index, &real);
            if let Err(skip) = replace::write_backup(&backup, snap.text.as_bytes()) {
                return (FileStatus::Skipped(skip), None);
            }
            let after = Stamp::of(plan.apply(&snap.text).as_bytes());
            (
                FileStatus::Buffer {
                    pane: snap.pane,
                    pane_path: snap.path.clone(),
                    version: snap.version,
                    changes: plan.changes.clone(),
                    real,
                    before: expect,
                    after,
                    backup,
                },
                Some(plan),
            )
        }
        Some(_) => (FileStatus::Skipped(Skip::OpenInSeveral), None),
        None => {
            let (real, bytes, plan) =
                match replace::check_disk(path, expect, matcher, template, lines) {
                    Ok(found) => found,
                    Err(skip) => return (FileStatus::Skipped(skip), None),
                };
            let Some(dir) = backup_dir.filter(|_| confirm) else {
                return (FileStatus::Ready { pane: None }, Some(plan));
            };
            let backup = replace::backup_path(dir, index, &real);
            match replace::write_disk(&real, &bytes, &plan, &backup) {
                Ok(after) => (
                    FileStatus::Written {
                        real,
                        before: expect,
                        after,
                        backup,
                    },
                    Some(plan),
                ),
                Err(skip) => {
                    // 元の中身のままだと確かめられたときだけ退避を消す（確かめられなければ手で拾えるよう残す）
                    if std::fs::read(&real).is_ok_and(|now| now == bytes) {
                        let _ = std::fs::remove_file(&backup);
                    }
                    (FileStatus::Skipped(skip), None)
                }
            }
        }
    }
}

/// 退避の置き場を消す（background だけ）。data dir の `search-replace` の下に限る
pub(crate) fn remove_backup_dir(dir: &Path) {
    let inside = dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == replace::BACKUP_DIR);
    if inside {
        let _ = std::fs::remove_dir_all(dir);
    }
}

impl ReplaceBody {
    /// 当てたファイルの記録（着地でバッファへ当てた後に組む）
    pub fn record(
        &self,
        id: u64,
        seq: Option<u64>,
        query: &str,
        replacement: &str,
        dir: PathBuf,
    ) -> Option<replace::Record> {
        let files: Vec<replace::RecordFile> = self
            .files
            .iter()
            .filter_map(|f| match &f.status {
                FileStatus::Written {
                    real,
                    before,
                    after,
                    backup,
                } => Some(replace::RecordFile {
                    path: f.path.clone(),
                    real: real.clone(),
                    hits: f.hits.clone(),
                    before: *before,
                    after: *after,
                    backup: backup.clone(),
                    buffer: None,
                }),
                FileStatus::BufferApplied {
                    pane,
                    pane_path,
                    version,
                    real,
                    before,
                    after,
                    backup,
                } => Some(replace::RecordFile {
                    path: f.path.clone(),
                    real: real.clone(),
                    hits: f.hits.clone(),
                    before: *before,
                    after: *after,
                    backup: backup.clone(),
                    buffer: Some(replace::BufferRef {
                        pane: pane.as_u64(),
                        version: *version,
                        path: pane_path.clone(),
                    }),
                }),
                _ => None,
            })
            .collect();
        (!files.is_empty()).then(|| replace::Record {
            id,
            seq,
            query: query.to_string(),
            replacement: replacement.to_string(),
            files,
            backup_dir: dir,
        })
    }

    /// 右パネルの行のプレビュー（結果の添字 → 行）
    pub fn rows(&self) -> BTreeMap<usize, RowPreview> {
        let line_of: HashMap<usize, usize> = self.hits.iter().map(|(i, h)| (*i, h.line)).collect();
        let mut rows = BTreeMap::new();
        for file in &self.files {
            for index in &file.hits {
                let row = match (&file.status, &file.plan) {
                    (FileStatus::Skipped(skip), _) => RowPreview::Skipped(*skip),
                    (_, Some(plan)) => match plan
                        .lines
                        .iter()
                        .find(|l| Some(&l.line) == line_of.get(index))
                    {
                        Some(line) => RowPreview::Ready(line.clone()),
                        None => RowPreview::Skipped(Skip::Unchanged),
                    },
                    (_, None) => RowPreview::Skipped(Skip::Unchanged),
                };
                rows.insert(*index, row);
            }
        }
        for index in &self.already {
            rows.insert(*index, RowPreview::Skipped(Skip::AlreadyApplied));
        }
        rows
    }
}

/// 応答の文面
pub mod text {
    use super::Note;

    pub const PREVIEW_NOTE: Note = Note::new(
        "{files} ファイル・{lines} 行・{occurrences} 箇所を置き換えます（まだ何も書き換えていない）",
        "This replaces {occurrences} place(s) on {lines} line(s) in {files} file(s) (nothing has been changed yet)",
    );
    pub const PREVIEW_NEXT_STEP: Note = Note::new(
        "当てるなら同じ指定に --yes（MCP は confirm=true）を付ける",
        "To apply it, repeat with --yes (MCP: confirm=true)",
    );
    pub const DONE_NOTE: Note = Note::new(
        "{files} ファイル・{lines} 行・{occurrences} 箇所を置き換えた",
        "Replaced {occurrences} place(s) on {lines} line(s) in {files} file(s)",
    );
    pub const UNDO_NEXT_STEP: Note = Note::new(
        "戻すなら tako search --undo --yes（MCP は action=undo・confirm=true）",
        "To revert it, run tako search --undo --yes (MCP: action=undo, confirm=true)",
    );
    pub const BUFFER_NOTE: Note = Note::new(
        "開いているペインのファイルは編集に当てた（保存はしていない。そのペインの undo 1 回でも戻る）",
        "Files open in a pane were changed in the editor (not saved; one undo in that pane also reverts it)",
    );
    pub const LIMIT_NOTE: Note = Note::new(
        "検索が上限で打ち切られている。当てるのは一覧に出た行だけ",
        "The search stopped at its limit; only the listed lines are replaced",
    );
    pub const NOTHING_NOTE: Note = Note::new("置き換える行が無い", "There is nothing to replace");
    pub const UNDO_PREVIEW_NEXT_STEP: Note = Note::new(
        "戻すなら同じ指定に --yes（MCP は confirm=true）を付ける",
        "To restore it, repeat with --yes (MCP: confirm=true)",
    );
    pub const UNDO_NOTHING: Note = Note::new(
        "戻す置換が無い（取り消しの履歴は GUI を起動しているあいだだけ持つ）",
        "There is no replacement to revert (the history lasts while the GUI is running)",
    );
}

fn fill(note: Note, values: &[(&str, &str)]) -> String {
    crate::lsp::text::fill(note, values)
}

/// 置換の応答（プレビューと当てるの共通）。`files[]` は範囲に入るファイルごとの行き先と理由、
/// `items[]` は範囲に入る行（`replaced` = 置換後の抜粋・`status`）
pub fn replace_json(landing: &ReplaceMeta, body: &ReplaceBody) -> Value {
    let lang = tako_core::i18n::lang();
    let confirm = landing.confirm;
    let status_of: HashMap<usize, (&FileResult, Option<&replace::LineReplace>)> = body
        .files
        .iter()
        .flat_map(|f| {
            f.hits.iter().map(move |i| {
                let line = body
                    .hits
                    .iter()
                    .find(|(index, _)| index == i)
                    .map(|(_, h)| h.line);
                let replaced = f
                    .plan
                    .as_ref()
                    .and_then(|p| p.lines.iter().find(|l| Some(l.line) == line));
                (*i, (f, replaced))
            })
        })
        .collect();
    let total = body.hits.len();
    let limit = match landing
        .limit
        .unwrap_or(crate::workspace_search::DEFAULT_LIMIT)
    {
        0 => total,
        n => n.min(total),
    };
    let items: Vec<Value> = body
        .hits
        .iter()
        .take(limit)
        .map(|(index, hit)| {
            let mut item = json!({
                "id": index + 1,
                "path": hit.path.display().to_string(),
                "line": hit.line,
                "column": hit.column,
                "excerpt": hit.excerpt,
            });
            if body.already.contains(index) {
                item["status"] = json!(Skip::AlreadyApplied.slug());
            } else if let Some((file, replaced)) = status_of.get(index) {
                match (&file.status, replaced) {
                    (FileStatus::Skipped(skip), _) => item["status"] = json!(skip.slug()),
                    (status, Some(line)) => {
                        item["status"] = json!(status.slug(confirm));
                        item["replaced"] = json!(line.after);
                    }
                    (_, None) => item["status"] = json!(Skip::Unchanged.slug()),
                }
            }
            item
        })
        .collect();
    let mut sums = [0usize; 3];
    let mut done = [0usize; 3];
    let mut skipped = 0;
    let mut buffers = false;
    let files: Vec<Value> = body
        .files
        .iter()
        .map(|f| {
            let (lines, occurrences) = f
                .plan
                .as_ref()
                .map_or((0, 0), |p| (p.lines.len(), p.occurrences));
            let mut out = json!({
                "path": f.path.display().to_string(),
                "status": f.status.slug(confirm),
                "lines": lines,
                "occurrences": occurrences,
            });
            match &f.status {
                FileStatus::Skipped(skip) => {
                    if !matches!(skip, Skip::Unchanged | Skip::AlreadyApplied) {
                        skipped += 1;
                    }
                    out["reason"] = json!(skip.text_in(lang));
                }
                status => {
                    sums[0] += 1;
                    sums[1] += lines;
                    sums[2] += occurrences;
                    if status.applied() {
                        done[0] += 1;
                        done[1] += lines;
                        done[2] += occurrences;
                    }
                    out["via"] = json!(if status.pane().is_some() {
                        "buffer"
                    } else {
                        "disk"
                    });
                    if let Some(pane) = status.pane() {
                        out["pane"] = json!(pane.as_u64());
                        buffers |= status.applied();
                    }
                }
            }
            out
        })
        .collect();
    let counts = |c: [usize; 3]| {
        [
            ("files", c[0].to_string()),
            ("lines", c[1].to_string()),
            ("occurrences", c[2].to_string()),
        ]
    };
    let filled = |note: Note, c: [usize; 3]| {
        let values = counts(c);
        let refs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        fill(note, &refs)
    };
    let mut out = json!({
        "query": landing.query.text,
        "regex": landing.query.regex,
        "case_sensitive": landing.query.case_sensitive,
        "replacement": landing.replacement,
        "roots": landing.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
        "scope": landing.scope.slug(),
        "confirm": confirm,
        "total": body.total,
        "targets": total,
        "files": files,
        "items": items,
        "truncated": total - limit,
        "stopped": body.stopped.map(|s| s.slug()),
    });
    let mut notes = Vec::new();
    if body.stopped == Some(Stop::Limit) {
        notes.push(text::LIMIT_NOTE.text_in(lang).to_string());
    }
    if !confirm {
        out["status"] = json!(if sums[0] > 0 { "preview" } else { "none" });
        out["will_replace"] = json!({ "files": sums[0], "lines": sums[1], "occurrences": sums[2] });
        if sums[0] > 0 {
            notes.insert(0, filled(text::PREVIEW_NOTE, sums));
            out["next_step"] = json!(text::PREVIEW_NEXT_STEP.text_in(lang));
        } else {
            notes.insert(0, text::NOTHING_NOTE.text_in(lang).to_string());
        }
    } else {
        out["status"] = json!(match (done[0], skipped) {
            (0, _) => "not-applied",
            (_, 0) => "replaced",
            _ => "partial",
        });
        out["replaced"] = json!({ "files": done[0], "lines": done[1], "occurrences": done[2] });
        if done[0] > 0 {
            notes.insert(0, filled(text::DONE_NOTE, done));
            if buffers {
                notes.push(text::BUFFER_NOTE.text_in(lang).to_string());
            }
            out["next_step"] = json!(text::UNDO_NEXT_STEP.text_in(lang));
        } else {
            notes.insert(0, text::NOTHING_NOTE.text_in(lang).to_string());
        }
    }
    out["note"] = json!(notes.join(" / "));
    out
}

// --- 取り消し ---------------------------------------------------------------

/// 取り消しの 1 ファイルの行き先
#[derive(Debug, Clone)]
pub enum UndoTarget {
    /// ディスクの中身を退避から戻す（background）
    Disk,
    /// バッファの undo 1 回（UI スレッドの着地。版がこのままなら）
    Buffer { pane: PaneId, version: u64 },
    /// 準備の段で決まった（バッファの中身を見て）
    Decided(Restore),
}

/// background で走らせる 1 回の取り消し（プレビューも同じ形 = `confirm: false`）
pub struct UndoJob {
    pub(crate) record: replace::Record,
    pub(crate) targets: Vec<UndoTarget>,
    pub(crate) confirm: bool,
    pub(crate) remaining: usize,
    /// 戻す回の「最中」の番号（プレビューは `None`）
    pub(crate) busy: Option<u64>,
    /// 下ろした記録の退避（background で消す）
    pub(crate) evicted: Vec<PathBuf>,
}

/// 取り消しの結果（UI スレッドの `workspace_undo_land` がバッファを戻して置き場を進める）
pub struct UndoLanding {
    pub(crate) record: replace::Record,
    pub(crate) targets: Vec<UndoTarget>,
    pub(crate) confirm: bool,
    pub(crate) remaining: usize,
    pub(crate) busy: Option<u64>,
}

impl UndoJob {
    /// ディスクのファイルを戻す（**UI スレッドで呼ばない**）
    pub fn run(self) -> UndoLanding {
        for dir in &self.evicted {
            remove_backup_dir(dir);
        }
        let targets = self
            .record
            .files
            .iter()
            .zip(self.targets)
            .map(|(file, target)| match target {
                UndoTarget::Disk => UndoTarget::Decided(replace::restore_disk(
                    &file.real,
                    file.before,
                    file.after,
                    &file.backup,
                    self.confirm,
                )),
                other => other,
            })
            .collect();
        UndoLanding {
            record: self.record,
            targets,
            confirm: self.confirm,
            remaining: self.remaining,
            busy: self.busy,
        }
    }
}

/// 取り消しの応答
pub fn undo_json(landing: &UndoLanding, outcomes: &[Restore], remaining: usize) -> Value {
    let lang = tako_core::i18n::lang();
    let files: Vec<Value> = landing
        .record
        .files
        .iter()
        .zip(outcomes)
        .map(|(file, outcome)| {
            let mut out = json!({
                "path": file.path.display().to_string(),
                "status": outcome.slug(),
                "via": if file.buffer.is_some() { "buffer" } else { "disk" },
                "reason": outcome.text_in(lang),
            });
            if let Some(buffer) = &file.buffer {
                out["pane"] = json!(buffer.pane);
            }
            if !outcome.settled() {
                out["backup"] = json!(file.backup.display().to_string());
            }
            out
        })
        .collect();
    let settled = outcomes.iter().filter(|o| o.settled()).count();
    let status = if !landing.confirm {
        "preview"
    } else if settled == outcomes.len() {
        "restored"
    } else if settled > 0 {
        "partial"
    } else {
        "not-restored"
    };
    let mut out = json!({
        "status": status,
        "id": landing.record.id,
        "query": landing.record.query,
        "replacement": landing.record.replacement,
        "confirm": landing.confirm,
        "files": files,
        "remaining": remaining,
    });
    if !landing.confirm {
        out["next_step"] = json!(text::UNDO_PREVIEW_NEXT_STEP.text_in(lang));
    }
    out
}

/// 戻す置換が無いときの応答
pub fn undo_nothing_json() -> Value {
    json!({
        "status": "nothing",
        "files": [],
        "remaining": 0,
        "note": text::UNDO_NOTHING.text(),
    })
}

/// 置き場の取り消しの履歴（`results` の応答に載せる）
pub fn history_json(state: &replace::ReplaceState) -> Value {
    json!(state
        .history()
        .iter()
        .rev()
        .map(|r| json!({
            "id": r.id,
            "query": r.query,
            "replacement": r.replacement,
            "files": r.files.len(),
        }))
        .collect::<Vec<_>>())
}

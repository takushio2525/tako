//! ワークスペース横断検索（FR-3.43 / #1694。⇧⌘F・`tako search`・MCP `tako_search`）
//!
//! ⌘F（#195）は開いている 1 ファイルの中だけを探す。ここは「この文字列がどのファイルの何行目に
//! あるか」をワークスペース（タブのファイルツリーのルート / 指定したフォルダ）の全体から探す。
//! LSP の `workspace/symbol`（#1689）とは別物（あちらはシンボル、こちらは生の文字列）。
//!
//! ## 置くもの
//!
//! - [`Query`] / [`Matcher`] — 検索語と条件（正規表現・大文字小文字）と、それを組んだ照合器
//! - [`search`] — 走査と照合の本体（**UI スレッドで呼ばない**。呼び手は dispatch の
//!   `prepare_offload` → background → `finish_offload` の 3 段）
//! - [`SearchStore`] — 右パネルの search ビューに出している結果の正本（GUI の行のクリックも
//!   CLI / MCP の `open` / `results` / `clear` も同じ 1 つを読み書きする = 設計原則 5）
//!
//! ## 決めごと（受け入れ条件で固定するもの）
//!
//! - **走査は ripgrep の walker（`ignore`）**。`.gitignore`（入れ子・否定・`.git/info/exclude`・
//!   git の global excludes）は **git のリポジトリの中でだけ**効く（ripgrep の既定と同じ。
//!   リポジトリの外の `~/.gitignore` が全部を隠す事故を作らない）
//! - 隠しファイル（`.agent/` `.github/` 等）は**探す**。VCS の中身（`.git` / `.hg` / `.svn` /
//!   `.jj`）だけは常に入らない
//! - **バイナリは飛ばす**: 先頭 [`BINARY_SNIFF_BYTES`] バイトに NUL があるファイル（git と同じ判定）。
//!   [`MAX_FILE_BYTES`] を超えるファイルも読まない（数は応答の `skipped` に載る）
//! - シンボリックリンクは**ファイルだけ辿る**。ディレクトリへのリンクは辿らない
//!   （ループもワークスペースの外への脱出も起きない。ripgrep の既定は両方辿らない）
//! - 照合は**行ごと**（改行を跨がない）。検索語に改行は入れられない。幅 0 の一致（`x*` の空）は
//!   一致に数えない
//! - 結果は 1 行 1 件（その行の最初の一致の桁へ着地し、抜粋の中の全一致を強調する）。
//!   並びは（ルートの順, パス, 行）で、走査の並列度に依らない
//! - 上限: 保持は [`MAX_RESULTS`] 行・抜粋は [`MAX_EXCERPT_CHARS`] 字・所要は
//!   [`DEFAULT_TIME_LIMIT`]。上限で止めたら `stopped` に理由が載る（黙って欠けない）

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 1 回の検索で保持する行の上限（超えたら走査を止めて `stopped = limit`）。
/// 1 行は抜粋 200 字 + パスなので、上限いっぱいでも保持は 2 MB に収まる
pub const MAX_RESULTS: usize = 2000;
/// 抜粋の字数の上限（長い行・minify された 1 行は一致の周りだけを切り出す）
pub const MAX_EXCERPT_CHARS: usize = 200;
/// 一致の前に残す字数（抜粋を切り出すとき、一致が左端に貼り付かないように）
const EXCERPT_LEAD_CHARS: usize = 40;
/// 読むファイルの大きさの上限（超えたら `skipped.too_large`）
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// バイナリ判定に見る先頭のバイト数（git の `buffer_is_binary` と同じ 8000）
pub const BINARY_SNIFF_BYTES: usize = 8000;
/// 所要の上限（ホームのような巨大なルートでも有限で返る。止めたら `stopped = time`）
pub const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(30);
/// 走査のスレッド数の上限（CPU を占有しない。端末の描画と取り合わない数）
pub const MAX_THREADS: usize = 4;
/// 中身に入らない VCS の内部（隠しファイルは探すのでここで明示的に外す。worktree の `.git` は
/// ファイルなので、ディレクトリに限らず名前で外す）
pub const VCS_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];

/// 検索語と条件（CLI `tako search <query> [--regex] [--case-sensitive]` / MCP `tako_search` /
/// GUI の検索欄とトグル）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    /// 正規表現として読む（偽 = 文字どおり）
    pub regex: bool,
    /// 大文字小文字を区別する（既定は区別しない = VS Code / Zed の既定と同じ）
    pub case_sensitive: bool,
}

/// 検索語を照合器にできない理由（CLI / MCP は `reason` で、GUI は検索欄の下に出す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// 空（空白だけも含む）
    Empty,
    /// 改行を含む（行ごとに照合するので一致しえない）
    Newline,
    /// 正規表現として読めない（`regex` の説明をそのまま持つ）
    Regex(String),
}

impl QueryError {
    /// CLI / MCP の応答に載せる理由（日本語。GUI は `ui_text` の訳を使う）
    pub fn reason(&self) -> String {
        match self {
            Self::Empty => "検索語が空".into(),
            Self::Newline => "検索語に改行は使えない（1 行ずつ照合する）".into(),
            Self::Regex(detail) => format!("正規表現として読めない: {detail}"),
        }
    }
}

/// 組み上げた照合器
#[derive(Debug, Clone)]
pub struct Matcher {
    /// 1 行に当てるもの
    line: regex::Regex,
    /// ファイル全体に 1 回だけ当てる前置き（一致しうるかの粗い判定。`None` = 前置きしない）。
    /// 大半のファイルは一致しないので、行に分ける前にここで落とす
    whole: Option<regex::Regex>,
}

impl Matcher {
    pub fn new(query: &Query) -> Result<Self, QueryError> {
        if query.text.trim().is_empty() {
            return Err(QueryError::Empty);
        }
        if query.text.contains(['\n', '\r']) {
            return Err(QueryError::Newline);
        }
        let pattern = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        let build = |crlf: bool| {
            regex::RegexBuilder::new(&pattern)
                .case_insensitive(!query.case_sensitive)
                // 行ごとに当てるので `^` / `$` は行の端（前置きでも同じ意味にするため複数行モード）
                .multi_line(true)
                .crlf(crlf)
                .build()
        };
        let line = build(false).map_err(|e| QueryError::Regex(regex_error_text(&e)))?;
        // `\A` / `\z` は「ファイルの端」と「行の端」で意味が割れるので前置きしない
        // （前置きが「一致しない」と誤って落とす側へ倒れない）
        let whole = if query.regex && (pattern.contains("\\A") || pattern.contains("\\z")) {
            None
        } else {
            Some(build(true).map_err(|e| QueryError::Regex(regex_error_text(&e)))?)
        };
        Ok(Self { line, whole })
    }

    /// その行の一致（バイト範囲。幅 0 の一致は数えない）
    fn line_matches(&self, line: &str) -> Vec<(usize, usize)> {
        self.line
            .find_iter(line)
            .filter(|m| m.start() < m.end())
            .map(|m| (m.start(), m.end()))
            .collect()
    }

    fn may_match(&self, text: &str) -> bool {
        self.whole.as_ref().is_none_or(|w| w.is_match(text))
    }
}

/// `regex` の説明は複数行（パターンと ^ の位置の図）なので最後の 1 行（理由）だけにする
fn regex_error_text(error: &regex::Error) -> String {
    let text = error.to_string();
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("error: ").to_string())
        .unwrap_or(text)
}

/// 結果の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 絶対パス（着地に使う）
    pub path: PathBuf,
    /// 表示用のパス（ルートからの相対。ルートが複数ならルートのフォルダ名を前に付ける）
    pub rel: String,
    /// 行（1 始まり）
    pub line: usize,
    /// その行の最初の一致の桁（1 始まりの文字数。`tako open --line --column` と同じ単位）
    pub column: usize,
    /// 抜粋（行頭の空白を落とし、長い行は一致の周りだけ。切った側に `…`）
    pub excerpt: String,
    /// 抜粋の中の一致（文字単位の半開区間。強調に使う）
    pub ranges: Vec<(usize, usize)>,
}

/// 探さなかったファイルの数
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skipped {
    /// 先頭に NUL がある
    pub binary: usize,
    /// [`MAX_FILE_BYTES`] を超える
    pub too_large: usize,
    /// 読めない（権限・途中で消えた・走査の失敗）
    pub unreadable: usize,
}

/// 走査を途中で止めた理由
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// [`MAX_RESULTS`] に達した
    Limit,
    /// 所要の上限に達した
    Time,
    /// 取り消された（次の検索が始まった / 結果を閉じた）
    Cancelled,
}

impl Stop {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Limit => "limit",
            Self::Time => "time",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 1 回の検索の結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    /// 中身を照合したファイルの数
    pub files_searched: usize,
    /// 1 行以上一致したファイルの数
    pub files_matched: usize,
    /// 一致の数（1 行に 2 つあれば 2）
    pub occurrences: usize,
    pub skipped: Skipped,
    /// 途中で止めた理由（`None` = 最後まで探した）。止めたときの件数は止めるまでに数えたぶん
    pub stopped: Option<Stop>,
    pub elapsed: Duration,
}

/// 走査の条件
#[derive(Debug, Clone)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    pub max_results: usize,
    pub max_file_bytes: u64,
    pub time_limit: Duration,
    pub threads: usize,
    /// git の global excludes（`core.excludesFile`）を効かせる。製品は真、単体は偽
    /// （開発機の設定で答えが変わらないように）
    pub git_global: bool,
}

impl Options {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, MAX_THREADS);
        Self {
            roots: distinct_roots(roots),
            max_results: MAX_RESULTS,
            max_file_bytes: MAX_FILE_BYTES,
            time_limit: DEFAULT_TIME_LIMIT,
            threads,
            git_global: true,
        }
    }
}

/// ルートを重複なく並べる。別のルートの配下にあるルートは落とす（同じファイルを 2 回出さない）。
/// 渡された順は保つ（ルートの順は結果の並びの第 1 キー）
pub fn distinct_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let canon: Vec<PathBuf> = roots
        .iter()
        .map(|r| crate::platform::path::canonicalize_or_self(r))
        .collect();
    let mut out = Vec::new();
    let mut kept: Vec<&PathBuf> = Vec::new();
    for (root, c) in roots.iter().zip(&canon) {
        let inside = canon.iter().any(|other| other != c && c.starts_with(other));
        if inside || kept.contains(&c) {
            continue;
        }
        kept.push(c);
        out.push(root.clone());
    }
    out
}

/// ファイル 1 つを照合した結果（走査のスレッドから集めるもの）
struct FileHits {
    root: usize,
    path: PathBuf,
    hits: Vec<Hit>,
    occurrences: usize,
}

/// 走査を共有する数え上げ
struct Shared {
    collected: Mutex<Vec<FileHits>>,
    hits: AtomicUsize,
    files_searched: AtomicUsize,
    binary: AtomicUsize,
    too_large: AtomicUsize,
    unreadable: AtomicUsize,
    /// 止めた理由（最初の 1 つ。0 = 止めていない）
    stop: AtomicUsize,
}

impl Shared {
    fn stop_with(&self, stop: Stop) {
        let code = match stop {
            Stop::Limit => 1,
            Stop::Time => 2,
            Stop::Cancelled => 3,
        };
        let _ = self
            .stop
            .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn stopped(&self) -> Option<Stop> {
        match self.stop.load(Ordering::SeqCst) {
            1 => Some(Stop::Limit),
            2 => Some(Stop::Time),
            3 => Some(Stop::Cancelled),
            _ => None,
        }
    }
}

/// 探す（**UI スレッドで呼ばない**）。`cancel` を立てると次のファイルで止まる
pub fn search(matcher: &Matcher, options: &Options, cancel: &AtomicBool) -> Outcome {
    let started = Instant::now();
    let roots = options.roots.clone();
    let Some((first, rest)) = roots.split_first() else {
        return Outcome::default();
    };
    let mut builder = ignore::WalkBuilder::new(first);
    for root in rest {
        builder.add(root);
    }
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(options.git_global)
        .require_git(true)
        .follow_links(false)
        .threads(options.threads.max(1))
        // 種類は問わない: git の worktree では `.git` がファイル（gitdir の在処の 1 行）
        .filter_entry(|entry| !VCS_DIRS.iter().any(|d| entry.file_name() == *d));
    let shared = Shared {
        collected: Mutex::new(Vec::new()),
        hits: AtomicUsize::new(0),
        files_searched: AtomicUsize::new(0),
        binary: AtomicUsize::new(0),
        too_large: AtomicUsize::new(0),
        unreadable: AtomicUsize::new(0),
        stop: AtomicUsize::new(0),
    };
    builder.build_parallel().run(|| {
        let shared = &shared;
        let roots = &roots;
        Box::new(move |entry| {
            use ignore::WalkState;
            if shared.stopped().is_some() {
                return WalkState::Quit;
            }
            if cancel.load(Ordering::Relaxed) {
                shared.stop_with(Stop::Cancelled);
                return WalkState::Quit;
            }
            if started.elapsed() > options.time_limit {
                shared.stop_with(Stop::Time);
                return WalkState::Quit;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    shared.unreadable.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                }
            };
            let Some(kind) = entry.file_type() else {
                return WalkState::Continue;
            };
            // ディレクトリへのリンクは辿らない。ファイルへのリンクは中身を探す
            let metadata = if kind.is_file() {
                entry.metadata().map_err(|_| ())
            } else if kind.is_symlink() {
                std::fs::metadata(entry.path()).map_err(|_| ())
            } else {
                return WalkState::Continue;
            };
            let metadata = match metadata {
                Ok(m) if m.is_file() => m,
                Ok(_) => return WalkState::Continue,
                Err(()) => {
                    shared.unreadable.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                }
            };
            if metadata.len() > options.max_file_bytes {
                shared.too_large.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
            let path = entry.path();
            let root = root_index(roots, path);
            match search_file(matcher, path, roots, root, options, shared, cancel) {
                FileVerdict::Searched(Some(found)) => {
                    shared.collected.lock().unwrap().push(found);
                }
                FileVerdict::Searched(None) => {}
                FileVerdict::Binary => {
                    shared.binary.fetch_add(1, Ordering::Relaxed);
                }
                FileVerdict::Unreadable => {
                    shared.unreadable.fetch_add(1, Ordering::Relaxed);
                }
            }
            if shared.stopped().is_some() {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });
    let stopped = shared.stopped();
    let mut files = shared.collected.into_inner().unwrap_or_default();
    // 並びは走査の並列度に依らない（ルートの順 → パス）。ファイルの中は行の順に積んである
    files.sort_by(|a, b| (a.root, &a.path).cmp(&(b.root, &b.path)));
    let files_matched = files.len();
    let occurrences = files.iter().map(|f| f.occurrences).sum();
    let mut hits: Vec<Hit> = files.into_iter().flat_map(|f| f.hits).collect();
    hits.truncate(options.max_results);
    Outcome {
        hits,
        files_searched: shared.files_searched.load(Ordering::Relaxed),
        files_matched,
        occurrences,
        skipped: Skipped {
            binary: shared.binary.load(Ordering::Relaxed),
            too_large: shared.too_large.load(Ordering::Relaxed),
            unreadable: shared.unreadable.load(Ordering::Relaxed),
        },
        stopped,
        elapsed: started.elapsed(),
    }
}

enum FileVerdict {
    Searched(Option<FileHits>),
    Binary,
    Unreadable,
}

fn root_index(roots: &[PathBuf], path: &Path) -> usize {
    roots
        .iter()
        .position(|r| path.starts_with(r))
        .unwrap_or_default()
}

/// 表示用のパス（ルートからの相対。ルートが複数ならルートのフォルダ名を前に付ける）
fn display_rel(roots: &[PathBuf], root: usize, path: &Path) -> String {
    let Some(base) = roots.get(root) else {
        return path.display().to_string();
    };
    let rel = path.strip_prefix(base).unwrap_or(path);
    if roots.len() > 1 {
        let name = base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| base.display().to_string());
        Path::new(&name).join(rel).display().to_string()
    } else {
        rel.display().to_string()
    }
}

fn search_file(
    matcher: &Matcher,
    path: &Path,
    roots: &[PathBuf],
    root: usize,
    options: &Options,
    shared: &Shared,
    cancel: &AtomicBool,
) -> FileVerdict {
    let Ok(bytes) = std::fs::read(path) else {
        return FileVerdict::Unreadable;
    };
    // 切り出し安全: bytes は fs::read のバイト列（Vec<u8>）
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return FileVerdict::Binary;
    }
    shared.files_searched.fetch_add(1, Ordering::Relaxed);
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(body);
    if !matcher.may_match(&text) {
        return FileVerdict::Searched(None);
    }
    let mut hits = Vec::new();
    let mut occurrences = 0;
    for (index, raw) in text.split('\n').enumerate() {
        // 巨大なファイルの途中でも取り消し・上限に気付く
        if index % 4096 == 4095 && cancel.load(Ordering::Relaxed) {
            shared.stop_with(Stop::Cancelled);
            break;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let found = matcher.line_matches(line);
        if found.is_empty() {
            continue;
        }
        // 保持の上限（全スレッドで数える）。超えたぶんは数えない
        if shared.hits.fetch_add(1, Ordering::SeqCst) >= options.max_results {
            shared.stop_with(Stop::Limit);
            break;
        }
        occurrences += found.len();
        let column = line[..found[0].0].chars().count() + 1;
        let (excerpt, ranges) = excerpt(line, &found);
        hits.push(Hit {
            path: path.to_path_buf(),
            rel: display_rel(roots, root, path),
            line: index + 1,
            column,
            excerpt,
            ranges,
        });
    }
    if hits.is_empty() {
        return FileVerdict::Searched(None);
    }
    FileVerdict::Searched(Some(FileHits {
        root,
        path: path.to_path_buf(),
        hits,
        occurrences,
    }))
}

/// 抜粋を切り出す。行頭の空白を落とし、[`MAX_EXCERPT_CHARS`] を超える行は最初の一致の
/// [`EXCERPT_LEAD_CHARS`] 字前から切る（切った側に `…`）。一致の範囲は抜粋の中の文字単位へ直し、
/// 抜粋の外に出たものは落とす / はみ出たものは端で切る
pub fn excerpt(line: &str, byte_ranges: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let lead_bytes = line.len() - line.trim_start().len();
    // 切り出し安全: lead_bytes は trim_start が残した後ろ側の始まり = 文字境界
    let body = &line[lead_bytes..];
    // バイト → 文字の位置（本文の中）
    let to_char = |byte: usize| {
        // 切り出し安全: 一致は str の正規表現（find_iter）の位置 = 文字境界、lead_bytes も境界
        body[..byte.saturating_sub(lead_bytes).min(body.len())]
            .chars()
            .count()
    };
    let total = body.chars().count();
    let mut ranges: Vec<(usize, usize)> = byte_ranges
        .iter()
        .map(|&(s, e)| (to_char(s), to_char(e)))
        .collect();
    if total <= MAX_EXCERPT_CHARS {
        return (body.to_string(), ranges);
    }
    let first = ranges.first().map_or(0, |r| r.0);
    let start = first
        .saturating_sub(EXCERPT_LEAD_CHARS)
        .min(total - MAX_EXCERPT_CHARS);
    let end = start + MAX_EXCERPT_CHARS;
    let mut out = String::new();
    let prefix = usize::from(start > 0);
    if start > 0 {
        out.push('\u{2026}');
    }
    out.extend(body.chars().skip(start).take(end - start));
    if end < total {
        out.push('\u{2026}');
    }
    ranges = ranges
        .into_iter()
        .filter(|&(s, e)| e > start && s < end)
        .map(|(s, e)| (s.max(start) - start + prefix, e.min(end) - start + prefix))
        .collect();
    (out, ranges)
}

/// 走っている検索（右パネルに出す側。次の検索 / 閉じるで取り消す）
#[derive(Debug, Clone)]
pub struct Running {
    pub seq: u64,
    pub query: Query,
    pub roots: Vec<PathBuf>,
    pub cancel: Arc<AtomicBool>,
}

/// 右パネルに出している結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub seq: u64,
    pub query: Query,
    pub roots: Vec<PathBuf>,
    pub outcome: Outcome,
}

/// search ビューの結果の正本（GUI が 1 つ持つ。CLI / MCP の `results` / `open` / `clear` も
/// dispatch 経由でこれを読み書きする）。保持は最後に出した 1 回ぶんだけで、`clear` で手放す
#[derive(Debug, Default)]
pub struct SearchStore {
    next_seq: u64,
    running: Option<Running>,
    shown: Option<Shown>,
    /// 出す側の検索が照合器を組めなかった理由（壊れた正規表現。次の検索 / 閉じるで消える）
    error: Option<(Query, QueryError)>,
}

impl SearchStore {
    /// 出す側の検索を始める。走っている前の検索は取り消す（その答えは [`Self::finish`] で捨てる）
    pub fn begin(&mut self, query: Query, roots: Vec<PathBuf>) -> (u64, Arc<AtomicBool>) {
        if let Some(prev) = self.running.take() {
            prev.cancel.store(true, Ordering::SeqCst);
        }
        self.next_seq += 1;
        self.error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running = Some(Running {
            seq: self.next_seq,
            query,
            roots,
            cancel: cancel.clone(),
        });
        (self.next_seq, cancel)
    }

    /// 答えを置く。待っている検索の答えでなければ置かずに `false`（追い越された / 閉じられた）
    pub fn finish(&mut self, seq: u64, outcome: Outcome) -> bool {
        match &self.running {
            Some(running) if running.seq == seq => {
                let running = self.running.take().expect("直前に確かめた");
                self.shown = Some(Shown {
                    seq,
                    query: running.query,
                    roots: running.roots,
                    outcome,
                });
                true
            }
            _ => false,
        }
    }

    /// 照合器を組めなかった（壊れた正規表現）。待っている検索の失敗なら前の結果も下ろして理由を置く
    pub fn fail(&mut self, seq: u64, error: QueryError) -> bool {
        match &self.running {
            Some(running) if running.seq == seq => {
                let running = self.running.take().expect("直前に確かめた");
                self.shown = None;
                self.error = Some((running.query, error));
                true
            }
            _ => false,
        }
    }

    /// 結果を閉じる（走っている検索も取り消す）。何か手放したら `true`
    pub fn clear(&mut self) -> bool {
        let had = self.running.is_some() || self.shown.is_some() || self.error.is_some();
        if let Some(running) = self.running.take() {
            running.cancel.store(true, Ordering::SeqCst);
        }
        self.shown = None;
        self.error = None;
        had
    }

    /// 照合器を組めなかった検索（検索語と理由）
    pub fn error(&self) -> Option<&(Query, QueryError)> {
        self.error.as_ref()
    }

    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    /// 保持している行の数（閉じたら 0）
    pub fn retained(&self) -> usize {
        self.shown.as_ref().map_or(0, |s| s.outcome.hits.len())
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_none() && self.shown.is_none() && self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない）
    struct Tree(PathBuf);

    impl Tree {
        fn new(label: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "tako-1694-{label}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, body: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            path
        }

        /// .gitignore が効くよう git のリポジトリにする（`.git` があればよい）
        fn git(&self) {
            std::fs::create_dir_all(self.0.join(".git")).unwrap();
        }

        fn options(&self) -> Options {
            let mut options = Options::new(vec![self.0.clone()]);
            options.git_global = false;
            options
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn query(text: &str, regex: bool, case_sensitive: bool) -> Query {
        Query {
            text: text.into(),
            regex,
            case_sensitive,
        }
    }

    fn run(tree: &Tree, q: &Query) -> Outcome {
        let matcher = Matcher::new(q).expect("照合器が組める");
        search(&matcher, &tree.options(), &AtomicBool::new(false))
    }

    /// （相対パス, 行, 桁, 抜粋）の並び
    fn rows(outcome: &Outcome) -> Vec<(String, usize, usize, String)> {
        outcome
            .hits
            .iter()
            .map(|h| {
                (
                    h.rel.replace('\\', "/"),
                    h.line,
                    h.column,
                    h.excerpt.clone(),
                )
            })
            .collect()
    }

    /// 受け入れ条件 1: 既知のツリーに対する結果（パス / 行 / 桁 / 抜粋）が固定値と一致する
    #[test]
    fn 既知のツリーの結果が固定値と一致する() {
        let tree = Tree::new("fixed");
        tree.write("src/main.rs", "fn main() {\n    let token = helper();\n}\n");
        tree.write(
            "src/lib/helper.rs",
            "// helper の本体\npub fn helper() -> u32 {\n\t42 // HELPER\n}\n",
        );
        tree.write("README.md", "# demo\n\nhelper を呼ぶ。\n");
        tree.write("notes.txt", "nothing here\n");
        let outcome = run(&tree, &query("helper", false, false));
        assert_eq!(
            rows(&outcome),
            vec![
                ("README.md".into(), 3, 1, "helper を呼ぶ。".into()),
                ("src/lib/helper.rs".into(), 1, 4, "// helper の本体".into()),
                (
                    "src/lib/helper.rs".into(),
                    2,
                    8,
                    "pub fn helper() -> u32 {".into()
                ),
                ("src/lib/helper.rs".into(), 3, 8, "42 // HELPER".into()),
                ("src/main.rs".into(), 2, 17, "let token = helper();".into()),
            ]
        );
        assert_eq!(outcome.files_matched, 3);
        assert_eq!(outcome.files_searched, 4);
        assert_eq!(outcome.occurrences, 5);
        assert_eq!(outcome.stopped, None);
        // 絶対パスはルートの下（着地に使う）
        assert!(outcome.hits.iter().all(|h| h.path.starts_with(&tree.0)));
        // 抜粋の強調は抜粋の中の文字位置（行頭のタブ / 空白を落としたあと）
        let main = outcome.hits.last().unwrap();
        assert_eq!(main.ranges, vec![(12, 18)]);
        assert_eq!(
            main.excerpt.chars().skip(12).take(6).collect::<String>(),
            "helper"
        );
    }

    /// 受け入れ条件 2-a: `.gitignore` の対象は探さない（リポジトリの中で。入れ子・否定も）
    #[test]
    fn gitignoreの対象は探さない() {
        let tree = Tree::new("gitignore");
        tree.git();
        tree.write(".gitignore", "target/\n*.log\n!keep.log\n");
        tree.write("src/a.rs", "needle\n");
        tree.write("target/debug/out.rs", "needle\n");
        tree.write("build.log", "needle\n");
        tree.write("keep.log", "needle\n");
        tree.write("sub/.gitignore", "local.txt\n");
        tree.write("sub/local.txt", "needle\n");
        tree.write("sub/kept.txt", "needle\n");
        // 隠しファイルは探すが、VCS の中身は探さない
        tree.write(".agent/notes.md", "needle\n");
        tree.write(".git/HEAD", "needle\n");
        let outcome = run(&tree, &query("needle", false, false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        assert_eq!(
            paths,
            vec![".agent/notes.md", "keep.log", "src/a.rs", "sub/kept.txt"]
        );
    }

    /// git の worktree（`.git` がファイル）でも .gitignore が効き、`.git` の 1 行は探さない
    #[test]
    fn worktreeのgitファイルも探さず_gitignoreは効く() {
        let tree = Tree::new("worktree");
        tree.write(".git", "gitdir: /somewhere/needle/worktrees/x\n");
        tree.write(".gitignore", "out/\n");
        tree.write("out/gen.txt", "needle\n");
        tree.write("src.txt", "needle\n");
        let outcome = run(&tree, &query("needle", false, false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, vec!["src.txt"]);
    }

    /// git のリポジトリの外の `.gitignore` は効かない（ripgrep の既定と同じ。
    /// ホームの `.gitignore` が全部を隠す事故を作らない）
    #[test]
    fn リポジトリの外のgitignoreは効かない() {
        let tree = Tree::new("nogit");
        tree.write(".gitignore", "*\n");
        tree.write("a.txt", "needle\n");
        let outcome = run(&tree, &query("needle", false, false));
        assert_eq!(rows(&outcome).len(), 1);
    }

    /// 受け入れ条件 2-b: バイナリ（先頭に NUL）は探さず数だけ残す。大きすぎるファイルも同じ
    #[test]
    fn バイナリと大きすぎるファイルは探さない() {
        let tree = Tree::new("binary");
        let mut blob = b"needle\0\x01\x02".to_vec();
        blob.extend(std::iter::repeat_n(b'x', 100));
        tree.write("image.bin", &blob);
        tree.write("text.txt", "needle\n");
        // NUL が先頭 8000 バイトより後ろにあるだけならテキストとして探す（git と同じ判定）
        let mut late = b"needle\n".to_vec();
        late.extend(std::iter::repeat_n(b'a', BINARY_SNIFF_BYTES));
        late.push(0);
        tree.write("late.txt", &late);
        tree.write("huge.txt", "needle\n");
        let matcher = Matcher::new(&query("needle", false, false)).unwrap();
        let mut options = tree.options();
        options.max_file_bytes = 1024;
        let outcome = search(&matcher, &options, &AtomicBool::new(false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        // late.txt は 1024 バイトを超えるので大きすぎる側
        assert_eq!(paths, vec!["huge.txt", "text.txt"]);
        assert_eq!(outcome.skipped.binary, 1);
        assert_eq!(outcome.skipped.too_large, 1);
        // 上限を戻せば NUL が後ろにあるファイルは探す
        let outcome = run(&tree, &query("needle", false, false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, vec!["huge.txt", "late.txt", "text.txt"]);
        assert_eq!(outcome.skipped.binary, 1);
    }

    /// 受け入れ条件 3: 大文字小文字・正規表現のトグルで件数が変わる（4 通り）
    #[test]
    fn 大文字小文字と正規表現のトグルで件数が変わる() {
        let tree = Tree::new("toggles");
        tree.write("a.txt", "Foo.bar\nfoo.bar\nFOO_BAR\nfooXbar\nnothing\n");
        let count = |regex: bool, case_sensitive: bool| {
            run(&tree, &query("foo.bar", regex, case_sensitive))
                .hits
                .iter()
                .map(|h| h.line)
                .collect::<Vec<_>>()
        };
        // 文字どおり・区別しない: `.` は文字の `.` = Foo.bar / foo.bar
        assert_eq!(count(false, false), vec![1, 2]);
        // 文字どおり・区別する: foo.bar だけ
        assert_eq!(count(false, true), vec![2]);
        // 正規表現・区別しない: `.` は任意の 1 字 = Foo.bar / foo.bar / FOO_BAR / fooXbar
        assert_eq!(count(true, false), vec![1, 2, 3, 4]);
        // 正規表現・区別する: foo.bar / fooXbar
        assert_eq!(count(true, true), vec![2, 4]);
    }

    #[test]
    fn 壊れた正規表現と空と改行は照合器にならない() {
        assert_eq!(
            Matcher::new(&query("", false, false)).unwrap_err(),
            QueryError::Empty
        );
        assert_eq!(
            Matcher::new(&query("   ", true, false)).unwrap_err(),
            QueryError::Empty
        );
        assert_eq!(
            Matcher::new(&query("a\nb", false, false)).unwrap_err(),
            QueryError::Newline
        );
        let QueryError::Regex(detail) = Matcher::new(&query("(unclosed", true, false)).unwrap_err()
        else {
            panic!("正規表現の誤りになる");
        };
        assert!(!detail.contains('\n'), "理由は 1 行: {detail}");
        assert!(!detail.is_empty());
        // 文字どおりなら同じ字面でも組める（括弧はただの文字）
        assert!(Matcher::new(&query("(unclosed", false, false)).is_ok());
    }

    #[test]
    fn 上限で打ち切って理由を残す() {
        let tree = Tree::new("limit");
        for i in 0..30 {
            tree.write(&format!("f{i:02}.txt"), "hit\nhit\n");
        }
        let matcher = Matcher::new(&query("hit", false, false)).unwrap();
        let mut options = tree.options();
        options.max_results = 25;
        let outcome = search(&matcher, &options, &AtomicBool::new(false));
        assert_eq!(outcome.hits.len(), 25);
        assert_eq!(outcome.stopped, Some(Stop::Limit));
        // 並びは崩れない（パス → 行）
        let mut sorted = outcome.hits.clone();
        sorted.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        assert_eq!(sorted, outcome.hits);
    }

    #[test]
    fn 取り消したら止まる() {
        let tree = Tree::new("cancel");
        tree.write("a.txt", "hit\n");
        let matcher = Matcher::new(&query("hit", false, false)).unwrap();
        let outcome = search(&matcher, &tree.options(), &AtomicBool::new(true));
        assert_eq!(outcome.stopped, Some(Stop::Cancelled));
        assert!(outcome.hits.is_empty());
    }

    #[test]
    fn 所要の上限で止まる() {
        let tree = Tree::new("time");
        tree.write("a.txt", "hit\n");
        let matcher = Matcher::new(&query("hit", false, false)).unwrap();
        let mut options = tree.options();
        options.time_limit = Duration::ZERO;
        let outcome = search(&matcher, &options, &AtomicBool::new(false));
        assert_eq!(outcome.stopped, Some(Stop::Time));
    }

    #[test]
    fn 長い行は一致の周りだけを抜粋する() {
        let line = format!("{}needle{}", "a".repeat(500), "b".repeat(500));
        let (text, ranges) = excerpt(&line, &[(500, 506)]);
        assert_eq!(text.chars().count(), MAX_EXCERPT_CHARS + 2, "前後に …");
        assert!(text.starts_with('\u{2026}') && text.ends_with('\u{2026}'));
        let (s, e) = ranges[0];
        assert_eq!(
            text.chars().skip(s).take(e - s).collect::<String>(),
            "needle"
        );
        // 短い行は行頭の空白だけ落としてそのまま
        assert_eq!(
            excerpt("\t  x = needle;", &[(7, 13)]),
            ("x = needle;".to_string(), vec![(4, 10)])
        );
    }

    #[test]
    fn crlfと多バイト文字の桁() {
        let tree = Tree::new("crlf");
        tree.write("w.txt", "一行目\r\n日本語の needle です\r\n");
        let outcome = run(&tree, &query("needle$", true, false));
        assert!(
            outcome.hits.is_empty(),
            "行末は needle の後ろに「 です」がある"
        );
        let outcome = run(&tree, &query("です$", true, false));
        assert_eq!(
            rows(&outcome),
            vec![("w.txt".into(), 2, 13, "日本語の needle です".into())]
        );
        let outcome = run(&tree, &query("needle", false, false));
        assert_eq!(outcome.hits[0].column, 6);
    }

    #[cfg(unix)]
    #[test]
    fn ディレクトリへのリンクは辿らずループしない() {
        let tree = Tree::new("loop");
        tree.write("real/a.txt", "needle\n");
        // 自分の親へのリンク（辿るとループ）とファイルへのリンク（中身を探す）
        std::os::unix::fs::symlink(&tree.0, tree.0.join("real/up")).unwrap();
        std::os::unix::fs::symlink(tree.0.join("real/a.txt"), tree.0.join("link.txt")).unwrap();
        let outcome = run(&tree, &query("needle", false, false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, vec!["link.txt", "real/a.txt"]);
        assert_eq!(outcome.stopped, None);
    }

    #[cfg(unix)]
    #[test]
    fn 読めないファイルは数えて飛ばす() {
        use std::os::unix::fs::PermissionsExt;
        let tree = Tree::new("unreadable");
        tree.write("ok.txt", "needle\n");
        let locked = tree.write("locked.txt", "needle\n");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root で走るとパーミッションを無視して読めてしまう（CI のコンテナ等）。そのときは数を見ない
        let readable_anyway = std::fs::read(&locked).is_ok();
        let outcome = run(&tree, &query("needle", false, false));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        if !readable_anyway {
            assert_eq!(rows(&outcome).len(), 1);
            assert_eq!(outcome.skipped.unreadable, 1);
        }
    }

    #[test]
    fn 入れ子のルートは1つにまとめて同じファイルを2回出さない() {
        let tree = Tree::new("roots");
        tree.write("sub/a.txt", "needle\n");
        let roots = distinct_roots(vec![tree.0.join("sub"), tree.0.clone(), tree.0.clone()]);
        assert_eq!(roots, vec![tree.0.clone()]);
        let other = Tree::new("roots-b");
        other.write("b.txt", "needle\n");
        let matcher = Matcher::new(&query("needle", false, false)).unwrap();
        let mut options = Options::new(vec![tree.0.clone(), other.0.clone()]);
        options.git_global = false;
        let outcome = search(&matcher, &options, &AtomicBool::new(false));
        let paths: Vec<String> = rows(&outcome).into_iter().map(|r| r.0).collect();
        let name = |t: &Tree| t.0.file_name().unwrap().to_string_lossy().into_owned();
        // ルートが複数なら表示にルートのフォルダ名が付き、ルートの順に並ぶ
        assert_eq!(
            paths,
            vec![
                format!("{}/sub/a.txt", name(&tree)),
                format!("{}/b.txt", name(&other))
            ]
        );
    }

    #[test]
    fn 置き場は次の検索で前を取り消し閉じたら手放す() {
        let mut store = SearchStore::default();
        let (first, first_cancel) = store.begin(query("a", false, false), vec![]);
        let (second, _) = store.begin(query("b", false, false), vec![]);
        assert!(
            first_cancel.load(Ordering::SeqCst),
            "前の検索は取り消される"
        );
        // 追い越された答えは置かない
        assert!(!store.finish(first, Outcome::default()));
        let outcome = Outcome {
            hits: vec![Hit {
                path: "/x".into(),
                rel: "x".into(),
                line: 1,
                column: 1,
                excerpt: "b".into(),
                ranges: vec![(0, 1)],
            }],
            ..Outcome::default()
        };
        assert!(store.finish(second, outcome));
        assert_eq!(store.retained(), 1);
        assert_eq!(store.shown().unwrap().query.text, "b");
        assert!(store.clear());
        assert_eq!(store.retained(), 0);
        assert!(store.is_empty());
        assert!(!store.clear(), "2 回目は手放すものが無い");
        // 壊れた正規表現: 待っている検索の失敗だけを置き、前の結果は下ろす
        let (third, _) = store.begin(query("(", true, false), vec![]);
        assert!(
            !store.fail(third + 1, QueryError::Empty),
            "待っていない番号は置かない"
        );
        assert!(store.fail(third, QueryError::Regex("閉じていない".into())));
        assert_eq!(
            store.error().unwrap().1,
            QueryError::Regex("閉じていない".into())
        );
        assert!(store.shown().is_none() && store.running().is_none());
        // 次の検索で理由は消える
        store.begin(query("ok", false, false), vec![]);
        assert!(store.error().is_none());
    }
}

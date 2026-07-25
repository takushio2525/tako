//! git — git リポジトリのデータ取得層（FR-3.6 git graph / FR-3.9 diff ビューア）
//!
//! git CLI 子プロセスで `git log --format` / `git branch` / `git diff` /
//! `git status --porcelain=v2` をパースする（VS Code / lazygit と同方式。
//! architecture.md「コンセプト②の実現」）。
//! パースは純関数（ユニットテスト対象）、コマンド実行は薄いラッパに分離。
//! git 不在・リポ外は空/エラーで無害に劣化する。

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use crate::theme::Rgb;

/// git バイナリの場所（tmux_bin と同パターン、プロセス内 1 回解決）
pub fn git_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(resolve_git_bin)
}

fn resolve_git_bin() -> String {
    crate::resolve_bin(
        "TAKO_GIT_BIN",
        "git",
        "--version",
        &[
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/usr/bin/git",
        ],
    )
}

// ──────────────────────── データ構造 ────────────────────────

/// コミットグラフ 1 エントリ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date_relative: String,
    pub subject: String,
    /// デコレーション（ブランチ名, タグ等。`HEAD -> main, origin/main` のような文字列）
    pub refs: String,
    /// 親コミットのハッシュ（マージコミットは 2 つ以上）
    pub parents: Vec<String>,
}

/// ブランチ 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub commit_hash: String,
    pub subject: String,
}

/// ワーキングツリーの変更ファイル 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusEntry {
    pub path: String,
    /// index（staging）側の状態（M/A/D/R 等。'.' は変更なし。untracked は '?'）
    pub index: char,
    /// worktree 側の状態
    pub worktree: char,
}

/// コンフリクト（unmerged）行のバッジ文字（#494）。
/// 「ステージ済み」でも単純な「変更」でもない第 3 の状態なので専用の記号を使う。
pub const CONFLICT_BADGE: char = '!';

impl GitStatusEntry {
    /// マージ未解決（porcelain v2 の `u` レコード）か（#494）。
    /// コンフリクトは解決してステージするまでコミットできないので、
    /// ステージ済みにも通常の変更にも分類しない
    pub fn is_conflicted(&self) -> bool {
        self.index == 'u' || self.worktree == 'u'
    }

    /// index にステージ済みの変更があるか（#487。untracked '?' は未ステージ扱い、
    /// #494: コンフリクトは「解決前」なのでステージ済みに含めない）
    pub fn is_staged(&self) -> bool {
        !self.is_conflicted() && self.index != '.' && self.index != '?' && self.index != ' '
    }

    /// worktree 側に未ステージの変更があるか（#487。untracked も未ステージ側に出す。
    /// #494: コンフリクトも「これから手を入れる側」として未ステージへ出す）
    pub fn is_unstaged(&self) -> bool {
        self.is_conflicted()
            || self.index == '?'
            || (self.worktree != '.' && self.worktree != '?' && self.worktree != ' ')
    }

    /// 未追跡ファイルか
    pub fn is_untracked(&self) -> bool {
        self.index == '?'
    }

    /// ステージ済み側に表示するバッジ文字（#487）
    pub fn staged_badge(&self) -> char {
        if self.is_conflicted() {
            return CONFLICT_BADGE;
        }
        match self.index {
            '.' | ' ' => ' ',
            c => c,
        }
    }

    /// 未ステージ側に表示するバッジ文字（#487 / #494）
    pub fn unstaged_badge(&self) -> char {
        if self.is_conflicted() {
            return CONFLICT_BADGE;
        }
        if self.index == '?' {
            return 'U';
        }
        match self.worktree {
            '.' | ' ' => ' ',
            c => c,
        }
    }
}

/// git status のサマリ
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitStatus {
    pub branch: String,
    pub upstream: String,
    pub entries: Vec<GitStatusEntry>,
}

/// diff のファイル単位
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
}

/// diff のハンク 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// diff の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
}

// ──────────────────────── コマンド実行 ────────────────────────

fn git_command(repo: &Path) -> Command {
    let mut cmd = Command::new(git_bin());
    cmd.current_dir(repo);
    cmd.env_remove("LC_ALL").env("LC_CTYPE", "UTF-8");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

fn run_git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = run_git_raw(repo, args)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(out.stderr.trim().to_string())
    }
}

/// git の生の実行結果（#496）。
/// `git merge` は「コンフリクトで終了コード 1」を正常な結果として返すため、
/// 成功/失敗を潰さずに終了コードと両ストリームを見る必要がある。
struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn run_git_raw(repo: &Path, args: &[&str]) -> Result<GitOutput, String> {
    let output = git_command(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git を実行できない: {e}"))?;
    Ok(GitOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// ──────────────────────── パス表記の可搬性（#520） ────────────────────────
//
// git は**プラットフォームを問わず常に `/` 区切り**でパスを出し入れする。
// 一方 Windows のファイルシステムパスは `\` 区切りで、ドライブレター（`C:`）や
// UNC（`\\server\share`）もある。この差を放置すると、
// `path.strip_prefix(repo)` の結果をそのまま `git log -- <path>` に渡したときに
// `src\foo.rs` となって git が一致を見つけられない（履歴が空で返る）。
//
// 変換はここに集約し、呼び出し側が個別に区切り文字を触らないようにする。

/// ファイルシステムのパスを git が期待する表記（区切りは常に `/`）へ直す。
/// unix では区切りが元から `/` なので実質そのまま
pub fn to_git_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// リポジトリルートからの相対パスを **git 表記**で返す。
///
/// `strip_prefix` に失敗した場合（リポ外・別ドライブ等）は `None`。
/// 呼び出し側はフルパスへフォールバックするのではなく、
/// 「このファイルはリポジトリ管理外」として扱うこと
pub fn repo_relative(repo: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(repo).ok()?;
    let s = to_git_path(rel);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// git が返す相対パス（常に `/` 区切り）を実ファイルパスへ直す
pub fn from_git_path(repo: &Path, rel: &str) -> std::path::PathBuf {
    let mut out = repo.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty() && *s != ".") {
        out.push(seg);
    }
    out
}

/// `rev-parse --show-toplevel` の出力を実パスへ直す。
///
/// git は Windows でも `C:/Users/x/repo` のように `/` で返す。`PathBuf` は
/// Windows でも `/` を区切りとして解釈するのでそのままでも動くが、
/// UNC（git は `//server/share` と返す）だけは `\\server\share` に直さないと解決できない
pub fn normalize_repo_root(raw: &str) -> std::path::PathBuf {
    let trimmed = raw.trim();
    if cfg!(windows) && trimmed.starts_with("//") && !trimmed.starts_with("///") {
        return std::path::PathBuf::from(trimmed.replace('/', "\\"));
    }
    std::path::PathBuf::from(trimmed)
}

/// パスから git リポジトリのルートを解決する（`git rev-parse --show-toplevel`）。
/// ファイルパスが渡された場合は親ディレクトリで解決する。リポ外なら None。
pub fn repo_root(path: &Path) -> Option<std::path::PathBuf> {
    let dir = if path.is_file() { path.parent()? } else { path };
    let output = git_command(dir)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        let root = String::from_utf8_lossy(&output.stdout);
        let root = root.trim();
        if !root.is_empty() {
            // git は Windows でも `/` 区切りで返す。UNC だけは実パス表記へ直す
            return Some(normalize_repo_root(root));
        }
    }
    None
}

// ──────────────────────── git log ────────────────────────

const LOG_FORMAT: &str = "%H\x01%h\x01%an\x01%cr\x01%s\x01%D\x01%P";
const FIELD_SEP: char = '\x01';

pub fn log_commits(repo: &Path, max_count: usize) -> Vec<GitCommit> {
    let out = run_git(
        repo,
        &[
            "log",
            "--all",
            &format!("--max-count={max_count}"),
            &format!("--format={LOG_FORMAT}"),
        ],
    )
    .unwrap_or_default();
    parse_log(&out)
}

fn parse_log(raw: &str) -> Vec<GitCommit> {
    raw.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(FIELD_SEP).collect();
            if f.len() < 7 {
                return None;
            }
            Some(GitCommit {
                hash: f[0].to_string(),
                short_hash: f[1].to_string(),
                author: f[2].to_string(),
                date_relative: f[3].to_string(),
                subject: f[4].to_string(),
                refs: f[5].to_string(),
                parents: f[6].split_whitespace().map(|s| s.to_string()).collect(),
            })
        })
        .collect()
}

/// 特定ファイルのコミット履歴を取得する（`git log --follow -- <file>`）。
/// リネームも追跡する。repo_root からの相対パスで指定する。
pub fn log_file_commits(repo: &Path, file_path: &str, max_count: usize) -> Vec<GitCommit> {
    let out = run_git(
        repo,
        &[
            "log",
            "--follow",
            &format!("--max-count={max_count}"),
            &format!("--format={LOG_FORMAT}"),
            "--",
            file_path,
        ],
    )
    .unwrap_or_default();
    parse_log(&out)
}

/// 特定コミットでの特定ファイルの diff を取得する。
/// `git diff <hash>^..<hash> -- <file>`（親を持たない初期コミットは
/// `git diff-tree --root -p` フォールバック。#487 で `git diff --root` から修正 =
/// 旧実装は「コミット vs 作業ツリー」を返していた）
pub fn diff_file_commit(repo: &Path, hash: &str, file_path: &str) -> Vec<DiffHunk> {
    let out = run_git(
        repo,
        &["diff", &format!("{hash}^..{hash}"), "--", file_path],
    );
    let raw = match out {
        Ok(s) => s,
        Err(_) => {
            run_git(repo, &["diff-tree", "--root", "-p", hash, "--", file_path]).unwrap_or_default()
        }
    };
    let files = parse_diff(&raw);
    files.into_iter().flat_map(|f| f.hunks).collect()
}

// ──────────────────────── git branch ────────────────────────

pub fn list_branches(repo: &Path) -> Vec<GitBranch> {
    let out = run_git(
        repo,
        &[
            "branch",
            "-a",
            "--sort=-committerdate",
            "--format=%(HEAD)\t%(refname:short)\t%(objectname:short)\t%(subject)",
        ],
    )
    .unwrap_or_default();
    parse_branches(&out)
}

fn parse_branches(raw: &str) -> Vec<GitBranch> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\t');
            let head = f.next()?;
            let name = f.next()?.to_string();
            let hash = f.next()?.to_string();
            let subject = f.next().unwrap_or("").to_string();
            Some(GitBranch {
                is_current: head == "*",
                is_remote: name.starts_with("remotes/") || name.contains('/'),
                name,
                commit_hash: hash,
                subject,
            })
        })
        .collect()
}

// ──────────────────────── git status ────────────────────────

pub fn status(repo: &Path) -> GitStatus {
    let out = run_git(repo, &["status", "--porcelain=v2", "--branch", "-uall"]).unwrap_or_default();
    parse_status(&out)
}

fn parse_status(raw: &str) -> GitStatus {
    let mut result = GitStatus::default();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            result.branch = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            result.upstream = rest.to_string();
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // 通常変更 / リネーム
            let bytes = line.as_bytes();
            if bytes.len() > 4 {
                let xy_str = &line[2..4];
                let mut chars = xy_str.chars();
                let index = chars.next().unwrap_or('.');
                let worktree = chars.next().unwrap_or('.');
                // パスはフィールド 9 以降（タブ区切りではなくスペース区切り）
                let path = line.splitn(9, ' ').last().unwrap_or("").to_string();
                // リネーム (2 ...) はタブ区切りで new\told になる
                let path = path.split('\t').next().unwrap_or(&path).to_string();
                result.entries.push(GitStatusEntry {
                    path,
                    index,
                    worktree,
                });
            }
        } else if line.starts_with("u ") {
            // #494: マージ未解決（unmerged）。
            // `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>` の 11 フィールド。
            // これを無視していたため、コンフリクト中のファイルが git パネルから
            // まるごと消え「変更はありません」と表示されていた
            if line.len() > 4 {
                let path = line.splitn(11, ' ').last().unwrap_or("").to_string();
                if !path.is_empty() {
                    result.entries.push(GitStatusEntry {
                        path,
                        // XY はコンフリクトの種類（UU / AA / DU 等）だが、
                        // UI では一律「未解決」として扱えば足りる
                        index: 'u',
                        worktree: 'u',
                    });
                }
            }
        } else if let Some(rest) = line.strip_prefix("? ") {
            let path = rest.to_string();
            result.entries.push(GitStatusEntry {
                path,
                index: '?',
                worktree: '?',
            });
        }
    }
    result
}

/// working tree 全体の変更行数（追加, 削除）。`git diff --shortstat HEAD` の
/// 「2 files changed, 126 insertions(+), 41 deletions(-)」をパースする（#217 サイドバー用。
/// HEAD が無い空リポジトリ等では (0, 0)）
pub fn diff_shortstat(repo: &Path) -> (usize, usize) {
    let out = run_git(repo, &["diff", "--shortstat", "HEAD"]).unwrap_or_default();
    parse_shortstat(&out)
}

fn parse_shortstat(raw: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for part in raw.split(',') {
        let part = part.trim();
        if let Some(n) = part
            .strip_suffix(" insertions(+)")
            .or_else(|| part.strip_suffix(" insertion(+)"))
        {
            added = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part
            .strip_suffix(" deletions(-)")
            .or_else(|| part.strip_suffix(" deletion(-)"))
        {
            removed = n.trim().parse().unwrap_or(0);
        }
    }
    (added, removed)
}

// ──────────────────────── git diff ────────────────────────

/// `git diff` の種別
pub enum DiffTarget {
    /// ワーキングツリー vs index（`git diff`）
    Unstaged,
    /// index vs HEAD（`git diff --cached`）
    Staged,
    /// 特定コミットの diff（`git diff <commit>^..<commit>`。親を持たない初期コミットは
    /// `git diff-tree --root -p <commit>` へフォールバック）
    Commit(String),
}

pub fn diff(repo: &Path, target: &DiffTarget) -> Vec<DiffFile> {
    let args: Vec<String> = match target {
        DiffTarget::Unstaged => vec!["diff".into()],
        DiffTarget::Staged => vec!["diff".into(), "--cached".into()],
        DiffTarget::Commit(hash) => vec!["diff".into(), format!("{hash}^..{hash}")],
    };
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = match run_git(repo, &arg_refs) {
        Ok(out) => out,
        Err(_) if matches!(target, DiffTarget::Commit(_)) => {
            // 初期コミット: 親がないので diff-tree --root でフォールバックする（#487。
            // 旧実装の `git diff --root <hash>` は「そのコミット vs 作業ツリー」を返すため、
            // 初期コミットを選ぶと作業ツリーの差分が表示されてしまっていた）
            if let DiffTarget::Commit(hash) = target {
                run_git(repo, &["diff-tree", "--root", "-p", hash]).unwrap_or_default()
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    };
    parse_diff(&out)
}

fn parse_diff(raw: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_file: Option<DiffFile> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            // 前のハンクを閉じる
            if let Some(hunk) = current_hunk.take() {
                if let Some(file) = current_file.as_mut() {
                    file.hunks.push(hunk);
                }
            }
            // 前のファイルを閉じる
            if let Some(file) = current_file.take() {
                files.push(file);
            }
            // パスは `b/path` から取る（リネーム時は b 側が新パス）
            let path = line
                .rsplit_once(" b/")
                .map(|(_, p)| p)
                .unwrap_or("")
                .to_string();
            current_file = Some(DiffFile {
                path,
                hunks: Vec::new(),
            });
        } else if line.starts_with("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(file) = current_file.as_mut() {
                    file.hunks.push(hunk);
                }
            }
            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(hunk) = current_hunk.as_mut() {
            let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
                (DiffLineKind::Add, rest)
            } else if let Some(rest) = line.strip_prefix('-') {
                (DiffLineKind::Remove, rest)
            } else if let Some(rest) = line.strip_prefix(' ') {
                (DiffLineKind::Context, rest)
            } else {
                (DiffLineKind::Context, line)
            };
            hunk.lines.push(DiffLine {
                kind,
                content: content.to_string(),
            });
        }
    }
    if let Some(hunk) = current_hunk {
        if let Some(file) = current_file.as_mut() {
            file.hunks.push(hunk);
        }
    }
    if let Some(file) = current_file {
        files.push(file);
    }
    files
}

// ──────────────────────── グラフレイアウト ────────────────────────

/// グラフ色パレット（Catppuccin Mocha ベース、8 色ローテーション）
pub const GRAPH_PALETTE: [Rgb; 8] = [
    Rgb::from_hex(0x89b4fa), // Blue
    Rgb::from_hex(0xa6e3a1), // Green
    Rgb::from_hex(0xf9e2af), // Yellow
    Rgb::from_hex(0xf38ba8), // Red
    Rgb::from_hex(0xcba6f7), // Mauve
    Rgb::from_hex(0x94e2d5), // Teal
    Rgb::from_hex(0xfab387), // Peach
    Rgb::from_hex(0xf5c2e7), // Pink
];

/// グラフレイアウトの計算結果
#[derive(Debug, Clone)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// ref 名 → 色パレットインデックスの対応（バッジ色用）
    pub ref_colors: std::collections::HashMap<String, usize>,
    /// 全行での最大レーン数（グラフ列の幅計算用）
    pub max_lanes: usize,
}

/// 1 行分のグラフレイアウト
#[derive(Debug, Clone)]
pub struct GraphRow {
    /// このコミットが配置されるレーン（0-indexed）
    pub lane: usize,
    /// 色パレットのインデックス
    pub color_index: usize,
    /// この行で使われるレーン数
    pub num_lanes: usize,
    /// 描画指示のリスト
    pub lines: Vec<GraphLine>,
}

/// 1 本の描画指示
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphLine {
    /// 縦線（行全体を貫通。パススルーまたは継続）
    Vertical { lane: usize, color_index: usize },
    /// 縦線上半分（上端→中央。到着側、下に親がない場合）
    VerticalTop { lane: usize, color_index: usize },
    /// 縦線下半分（中央→下端。新しいブランチの先端）
    VerticalBottom { lane: usize, color_index: usize },
    /// S 字カーブ（中央→下端。分岐 or マージの接続線）
    CurveDown {
        from_lane: usize,
        to_lane: usize,
        color_index: usize,
    },
}

/// コミット列からグラフレイアウトを計算する（newest-first 順の入力を想定）
pub fn compute_graph_layout(commits: &[GitCommit]) -> GraphLayout {
    use std::collections::HashMap;

    let mut active: Vec<Option<String>> = Vec::new();
    let mut lane_colors: Vec<usize> = Vec::new();
    let mut next_color: usize = 0;
    let mut rows = Vec::with_capacity(commits.len());
    let mut ref_colors: HashMap<String, usize> = HashMap::new();
    let mut max_lanes: usize = 0;

    for commit in commits {
        // 1. このコミットのレーンを決定
        let found = active
            .iter()
            .position(|s| s.as_deref() == Some(&*commit.hash));
        let has_line_above = found.is_some();

        let lane = if let Some(l) = found {
            l
        } else {
            // ブランチの先端（まだどこにも予約されていない）→ 空きレーンを確保
            let l = first_empty(&active);
            if l >= active.len() {
                active.push(Some(commit.hash.clone()));
                lane_colors.push(next_color);
            } else {
                active[l] = Some(commit.hash.clone());
                lane_colors[l] = next_color;
            }
            next_color = (next_color + 1) % GRAPH_PALETTE.len();
            l
        };

        let color_index = lane_colors[lane];

        // 2. ref 名 → 色の対応を記録
        if !commit.refs.is_empty() {
            for r in commit.refs.split(", ") {
                ref_colors.insert(r.to_string(), color_index);
            }
        }

        // 3. エッジを構築
        struct Edge {
            from: usize,
            to: usize,
            color: usize,
        }
        let mut edges: Vec<Edge> = Vec::new();

        // 他のアクティブレーンのパススルーエッジ
        for (i, slot) in active.iter().enumerate() {
            if i != lane && slot.is_some() {
                edges.push(Edge {
                    from: i,
                    to: i,
                    color: lane_colors[i],
                });
            }
        }

        // コミットのレーンをクリア
        active[lane] = None;

        // 各親のエッジを処理
        for (pi, parent) in commit.parents.iter().enumerate() {
            let existing = active.iter().position(|s| s.as_deref() == Some(&**parent));
            if let Some(pl) = existing {
                // 親が既に別レーンにいる → マージエッジ
                edges.push(Edge {
                    from: lane,
                    to: pl,
                    color: lane_colors[pl],
                });
            } else if pi == 0 {
                // 第 1 親はコミットのレーンを継承（直線継続）
                active[lane] = Some(parent.clone());
                edges.push(Edge {
                    from: lane,
                    to: lane,
                    color: color_index,
                });
            } else {
                // 第 2 親以降 → 新しいレーンを確保
                let nl = first_empty(&active);
                let c = next_color;
                next_color = (next_color + 1) % GRAPH_PALETTE.len();
                if nl >= active.len() {
                    active.push(Some(parent.clone()));
                    lane_colors.push(c);
                } else {
                    active[nl] = Some(parent.clone());
                    lane_colors[nl] = c;
                }
                edges.push(Edge {
                    from: lane,
                    to: nl,
                    color: c,
                });
            }
        }

        // 4. 末尾の空きレーンを除去してコンパクト化
        while active.last() == Some(&None) {
            active.pop();
            lane_colors.pop();
        }

        let num_lanes = active.len().max(lane + 1);
        if num_lanes > max_lanes {
            max_lanes = num_lanes;
        }

        // 5. エッジを描画指示に変換
        let has_continuation = edges.iter().any(|e| e.from == lane && e.to == lane);
        let mut lines = Vec::new();

        // パススルー縦線（他のレーンの直線通過）
        for e in &edges {
            if e.from == e.to && e.from != lane {
                lines.push(GraphLine::Vertical {
                    lane: e.from,
                    color_index: e.color,
                });
            }
        }

        // コミット自身のレーンの縦線
        if has_continuation {
            if has_line_above {
                lines.push(GraphLine::Vertical { lane, color_index });
            } else {
                lines.push(GraphLine::VerticalBottom { lane, color_index });
            }
        } else if has_line_above {
            lines.push(GraphLine::VerticalTop { lane, color_index });
        }

        // 分岐・マージのカーブ線
        for e in &edges {
            if e.from != e.to {
                lines.push(GraphLine::CurveDown {
                    from_lane: e.from,
                    to_lane: e.to,
                    color_index: e.color,
                });
            }
        }

        rows.push(GraphRow {
            lane,
            color_index,
            num_lanes,
            lines,
        });
    }

    GraphLayout {
        rows,
        ref_colors,
        max_lanes,
    }
}

/// アクティブレーン配列で最初の空きスロットを返す（無ければ末尾の次のインデックス）
fn first_empty(active: &[Option<String>]) -> usize {
    active
        .iter()
        .position(|s| s.is_none())
        .unwrap_or(active.len())
}

// ──────────────────────── コミット詳細（#495）────────────────────────

/// コミットの変更ファイル 1 件（`git show --numstat` + `--diff-filter` から）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFileChange {
    pub path: String,
    /// 変更種別: A(dd) / M(odify) / D(elete) / R(ename) / C(opy)
    pub kind: char,
    pub additions: usize,
    pub deletions: usize,
    /// リネーム元のパス（kind=R のときのみ有値）
    pub old_path: Option<String>,
}

/// コミットの詳細情報（#495。`git show` 相当）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
    pub files: Vec<CommitFileChange>,
}

/// 特定コミットの詳細を取得する（#495）。
/// `git log -1 --format=...` でメタ情報、`git diff-tree --no-commit-id -r --numstat --diff-filter` で
/// 変更ファイル一覧を取る（`git show --stat` より機械可読）。
/// 初期コミット（親なし）は `--root` を付けて対応する
pub fn show_commit(repo: &Path, hash: &str) -> Result<CommitDetail, String> {
    const DETAIL_FORMAT: &str = "%H\x01%an\x01%ae\x01%ai\x01%cn\x01%ce\x01%ci\x01%s\x01%b\x01%P";
    let meta_raw = run_git(
        repo,
        &["log", "-1", &format!("--format={DETAIL_FORMAT}"), hash],
    )?;
    let meta_raw = meta_raw.trim_end();
    // %b（本文）に改行が含まれ得るので、最後の \x01 以降を parents として取る
    // フォーマット: hash\x01author\x01email\x01date\x01cn\x01ce\x01cd\x01subject\x01body\x01parents
    // body 内に \x01 が入ることは現実的に無いが、分割は先頭 7 + 末尾 1 で固定して安全に取る
    let fields: Vec<&str> = meta_raw.splitn(8, '\x01').collect();
    if fields.len() < 8 {
        return Err(format!("コミット {hash} の情報を解析できない"));
    }
    // fields[7] = "subject\x01body\x01parents" — 末尾から parents を分離
    let rest = fields[7];
    let (subject_body, parents_str) = rest.rsplit_once('\x01').unwrap_or((rest, ""));
    let (subject, body_raw) = subject_body
        .split_once('\x01')
        .unwrap_or((subject_body, ""));
    let body = body_raw.trim_end().to_string();
    let parents: Vec<String> = parents_str
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    // 変更ファイル一覧: diff-tree --numstat + diff-tree --diff-filter で種別を取る
    let numstat_args = if parents.is_empty() {
        vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "--numstat",
            hash,
        ]
    } else {
        vec!["diff-tree", "--no-commit-id", "-r", "--numstat", hash]
    };
    let numstat_raw = run_git(repo, &numstat_args).unwrap_or_default();
    let filter_args = if parents.is_empty() {
        vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "--diff-filter=AMDRC",
            "--name-status",
            hash,
        ]
    } else {
        vec![
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--diff-filter=AMDRC",
            "--name-status",
            hash,
        ]
    };
    let filter_raw = run_git(repo, &filter_args).unwrap_or_default();

    // name-status からパス→種別のマップを作る
    let mut kind_map: std::collections::HashMap<String, (char, Option<String>)> =
        std::collections::HashMap::new();
    for line in filter_raw.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let k = parts[0].chars().next().unwrap_or('M');
            if k == 'R' || k == 'C' {
                // リネーム/コピー: "R100\told\tnew" のように来る
                if parts.len() >= 3 {
                    kind_map.insert(parts[2].to_string(), (k, Some(parts[1].to_string())));
                }
            } else {
                kind_map.insert(parts[1].to_string(), (k, None));
            }
        }
    }

    // numstat を走査（タブ区切り: "追加\t削除\tパス"）
    let mut files = Vec::new();
    for line in numstat_raw.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        // バイナリファイルは "-\t-\tpath" になる
        let additions = parts[0].parse().unwrap_or(0);
        let deletions = parts[1].parse().unwrap_or(0);
        // リネームは "old => new" や "{prefix => suffix}" の形。最後のパスを取る
        let path = if parts.len() > 3 {
            // タブ区切り 4 列目以降がある = パス内にタブ（極めて稀）
            parts[2..].join("\t")
        } else {
            parts[2].to_string()
        };
        let (kind, old_path) = kind_map.get(&path).cloned().unwrap_or(('M', None));
        files.push(CommitFileChange {
            path,
            kind,
            additions,
            deletions,
            old_path,
        });
    }

    Ok(CommitDetail {
        hash: fields[0].to_string(),
        author_name: fields[1].to_string(),
        author_email: fields[2].to_string(),
        author_date: fields[3].to_string(),
        committer_name: fields[4].to_string(),
        committer_email: fields[5].to_string(),
        committer_date: fields[6].to_string(),
        subject: subject.to_string(),
        body,
        parents,
        files,
    })
}

// ──────────────────────── 操作系 API ────────────────────────

/// git add: 指定パスをステージングする。パスが空なら全変更（`git add -A`）
pub fn stage(repo: &Path, paths: &[&str]) -> Result<String, String> {
    if paths.is_empty() {
        run_git(repo, &["add", "-A"])
    } else {
        let mut args = vec!["add", "--"];
        args.extend(paths);
        run_git(repo, &args)
    }
}

/// git reset HEAD: 指定パスをアンステージする。パスが空なら全アンステージ
pub fn unstage(repo: &Path, paths: &[&str]) -> Result<String, String> {
    if paths.is_empty() {
        run_git(repo, &["reset", "HEAD"])
    } else {
        let mut args = vec!["reset", "HEAD", "--"];
        args.extend(paths);
        run_git(repo, &args)
    }
}

/// コミットメッセージの最大長（バイト。#494）。
/// git 自体に上限は無いが、1 行入力欄へ巨大なテキストが貼られたときに
/// 描画・差分計算が重くなるのを防ぐための実用上の歯止め。
pub const COMMIT_MESSAGE_MAX: usize = 4096;

/// git パネルの 1 行入力欄向けにコミットメッセージを正規化する（#494）。
///
/// - 改行・タブを含む制御文字は半角空白へ潰す（1 行入力欄に改行は入れられないため、
///   複数行メッセージを貼られても壊れた表示にならないようにする）
/// - サロゲートペア・結合文字などの通常の文字はそのまま通す（Rust の String は
///   常に妥当な UTF-8 なので不正バイトは原理的に混入しない）
/// - `COMMIT_MESSAGE_MAX` を超える分は**文字境界で**切り捨てる
pub fn sanitize_commit_message(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(COMMIT_MESSAGE_MAX));
    for ch in input.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        if out.len() + ch.len_utf8() > COMMIT_MESSAGE_MAX {
            break;
        }
        out.push(ch);
    }
    out
}

/// コミットを実行できない理由（#494）。UI・CLI で共通の判定にするため core に置く。
/// 文言化は呼び出し側（UI は日英、CLI は日本語）で行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitBlock {
    /// メッセージが空（空白のみを含む）
    EmptyMessage,
    /// コミット対象の変更が無い
    NoChanges,
}

/// コミットの実行可否を判定する（#494）。`None` なら実行可能。
pub fn commit_block(message: &str, has_changes: bool) -> Option<CommitBlock> {
    if message.trim().is_empty() {
        Some(CommitBlock::EmptyMessage)
    } else if !has_changes {
        Some(CommitBlock::NoChanges)
    } else {
        None
    }
}

/// git commit -m: メッセージ付きコミット。`all` = true で `-a`（tracked のみ auto stage）
pub fn commit(repo: &Path, message: &str, all: bool) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("コミットメッセージが空です".to_string());
    }
    let mut args = vec!["commit"];
    if all {
        args.push("-a");
    }
    args.push("-m");
    args.push(message);
    run_git(repo, &args)
}

/// git pull
pub fn pull(repo: &Path) -> Result<String, String> {
    run_git(repo, &["pull"])
}

/// git push
pub fn push(repo: &Path) -> Result<String, String> {
    run_git(repo, &["push"])
}

// ──────────────── ブランチ操作 / マージ / コンフリクト（#496）────────────────

/// 進行中のマルチステップ操作（#496）。コンフリクトを起こし得るものだけを扱う。
/// 「今どの操作の途中で止まっているか」で中止コマンドが変わるため区別する
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoOperation {
    /// 進行中の操作なし（通常状態）
    #[default]
    None,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

impl RepoOperation {
    /// CLI / MCP の JSON へ出す安定した識別子
    pub fn as_str(&self) -> &'static str {
        match self {
            RepoOperation::None => "none",
            RepoOperation::Merging => "merging",
            RepoOperation::Rebasing => "rebasing",
            RepoOperation::CherryPicking => "cherry-picking",
            RepoOperation::Reverting => "reverting",
        }
    }

    /// 中止に使う git サブコマンド。`None` は中止対象が無い
    pub fn abort_args(&self) -> Option<[&'static str; 2]> {
        match self {
            RepoOperation::None => None,
            RepoOperation::Merging => Some(["merge", "--abort"]),
            RepoOperation::Rebasing => Some(["rebase", "--abort"]),
            RepoOperation::CherryPicking => Some(["cherry-pick", "--abort"]),
            RepoOperation::Reverting => Some(["revert", "--abort"]),
        }
    }

    pub fn is_active(&self) -> bool {
        *self != RepoOperation::None
    }
}

/// コンフリクト状態（#496 Part 2）。
/// 進行中の操作・未解決ファイル・マージ元/先を 1 まとめにして UI / CLI / MCP へ返す
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictState {
    pub operation: RepoOperation,
    /// 未解決（unmerged）ファイルのパス
    pub files: Vec<String>,
    /// マージ先 = 現在のブランチ（detached HEAD なら空）
    pub ours: String,
    /// マージ元（`MERGE_HEAD` 等の ref 名。解決できなければ短縮ハッシュ）
    pub theirs: Option<String>,
}

impl ConflictState {
    /// コンフリクトカードを出すべき状態か。
    /// 操作が進行中なら未解決ファイルが 0 でも（= 解決済みでコミット待ち）カードは出す
    pub fn is_active(&self) -> bool {
        self.operation.is_active()
    }
}

/// `.git` ディレクトリの実体を解決する（worktree では `.git` がファイルなので rev-parse を使う）
fn git_dir(repo: &Path) -> Option<std::path::PathBuf> {
    let out = run_git(repo, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(trimmed))
    }
}

/// 進行中の操作とコンフリクトファイルを取得する（#496）
pub fn conflict_state(repo: &Path) -> ConflictState {
    let st = status(repo);
    let files: Vec<String> = st
        .entries
        .iter()
        .filter(|e| e.is_conflicted())
        .map(|e| e.path.clone())
        .collect();

    let Some(dir) = git_dir(repo) else {
        return ConflictState {
            operation: RepoOperation::None,
            files,
            ours: st.branch,
            theirs: None,
        };
    };
    // 判定順はリカバリの緊急度が高い順。rebase は 2 種類のディレクトリ形式がある
    let operation = if dir.join("MERGE_HEAD").exists() {
        RepoOperation::Merging
    } else if dir.join("rebase-merge").exists() || dir.join("rebase-apply").exists() {
        RepoOperation::Rebasing
    } else if dir.join("CHERRY_PICK_HEAD").exists() {
        RepoOperation::CherryPicking
    } else if dir.join("REVERT_HEAD").exists() {
        RepoOperation::Reverting
    } else {
        RepoOperation::None
    };

    let theirs = match operation {
        RepoOperation::Merging => describe_ref(repo, "MERGE_HEAD"),
        RepoOperation::CherryPicking => describe_ref(repo, "CHERRY_PICK_HEAD"),
        RepoOperation::Reverting => describe_ref(repo, "REVERT_HEAD"),
        // rebase 中は「取り込み中のコミット」が theirs 側になる
        RepoOperation::Rebasing => describe_ref(repo, "REBASE_HEAD"),
        RepoOperation::None => None,
    };

    ConflictState {
        operation,
        files,
        ours: st.branch,
        theirs,
    }
}

/// ref を人が読める名前へ解決する（ブランチ名が取れなければ短縮ハッシュ）
fn describe_ref(repo: &Path, refname: &str) -> Option<String> {
    if let Ok(name) = run_git(
        repo,
        &["name-rev", "--name-only", "--no-undefined", refname],
    ) {
        let name = name.trim();
        if !name.is_empty() {
            // `remotes/origin/foo~1` のような表現はそのまま出す（何を取り込んでいるかが分かる）
            return Some(name.to_string());
        }
    }
    run_git(repo, &["rev-parse", "--short", refname])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// ブランチ名として使えるかを検証する（#496）。
///
/// git 自身の `check-ref-format` に相当する規則を純関数で持つ。目的は 2 つ:
/// - **引数注入の遮断**: `-d` のような名前をそのまま `git branch` へ渡すとオプションとして
///   解釈される。`git checkout <branch>` は `--` でオプション終端を作れない
///   （`git checkout -- x` は「ファイルの復元」になる）ので、名前側で弾くしかない
/// - 実行前に理由を日本語で返す（git のエラーを読ませない）
pub fn validate_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("ブランチ名が空です".into());
    }
    if name.starts_with('-') {
        return Err("ブランチ名を - で始めることはできません".into());
    }
    if name.chars().any(|c| c.is_control() || c == ' ') {
        return Err("ブランチ名に空白・制御文字は使えません".into());
    }
    if let Some(c) = name.chars().find(|c| "~^:?*[\\".contains(*c)) {
        return Err(format!("ブランチ名に {c} は使えません"));
    }
    if name.contains("..") || name.contains("@{") || name == "@" {
        return Err("ブランチ名に .. や @{ は使えません".into());
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err("ブランチ名の / の使い方が不正です".into());
    }
    if name.ends_with('.') || name.ends_with(".lock") {
        return Err("ブランチ名を . や .lock で終わらせることはできません".into());
    }
    if name.split('/').any(|part| part.starts_with('.')) {
        return Err("ブランチ名の各要素を . で始めることはできません".into());
    }
    Ok(())
}

/// ref（ブランチ・タグ・ハッシュ）が実在するか
pub fn ref_exists(repo: &Path, refname: &str) -> bool {
    if refname.starts_with('-') {
        return false;
    }
    run_git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ],
    )
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false)
}

/// ローカルブランチが存在するか
pub fn local_branch_exists(repo: &Path, name: &str) -> bool {
    if name.starts_with('-') {
        return false;
    }
    run_git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

/// 追跡対象ファイルのうち未コミット変更があるものを返す（untracked は切替・マージを阻害しない）
fn dirty_tracked_files(status: &GitStatus) -> Vec<String> {
    status
        .entries
        .iter()
        .filter(|e| !e.is_untracked())
        .map(|e| e.path.clone())
        .collect()
}

/// ブランチ切替の事前提示（#496）。
/// 「黙って強制切替 / stash しない」ため、実行前に何が起きるかを機械可読で返す
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutPreview {
    pub target: String,
    pub current: String,
    /// 未コミット変更のある追跡ファイル
    pub dirty_files: Vec<String>,
    /// 切替で上書きされるため git が切替を**拒否する**ファイル（dirty ∩ 切替差分）
    pub blocking_files: Vec<String>,
    /// 切替後もそのまま持ち越される変更ファイル（dirty − blocking）
    pub carried_files: Vec<String>,
    /// 切替で内容が入れ替わるファイル数
    pub changed_files: usize,
    /// リモート追跡ブランチから同名のローカルブランチを新規作成する切替か
    pub creates_local_branch: bool,
    /// 実行を止めるべき理由（進行中のコンフリクト等）
    pub blockers: Vec<String>,
}

impl CheckoutPreview {
    /// 事前提示なしで実行してよいか。未コミット変更が 1 件でもあれば必ず提示する
    pub fn needs_confirmation(&self) -> bool {
        !self.blockers.is_empty() || !self.dirty_files.is_empty()
    }
}

/// 切替対象の解決結果（ローカル / リモート追跡）
fn resolve_checkout_target(repo: &Path, target: &str) -> Result<(String, bool), String> {
    if local_branch_exists(repo, target) {
        return Ok((target.to_string(), false));
    }
    // `origin/foo` / `remotes/origin/foo` をクリックしたときは、detached HEAD にせず
    // 同名のローカル追跡ブランチを作る（VS Code / lazygit と同じ挙動）
    let short = target.strip_prefix("remotes/").unwrap_or(target);
    if ref_exists(repo, short) {
        let local = short.split_once('/').map(|(_, rest)| rest).unwrap_or(short);
        if !local.is_empty() && !local_branch_exists(repo, local) && short.contains('/') {
            return Ok((short.to_string(), true));
        }
        return Ok((short.to_string(), false));
    }
    Err(format!("ブランチ '{target}' が見つかりません"))
}

pub fn checkout_preview(repo: &Path, target: &str) -> Result<CheckoutPreview, String> {
    validate_branch_name(target)?;
    let (resolved, creates_local_branch) = resolve_checkout_target(repo, target)?;
    let st = status(repo);
    let current = st.branch.clone();
    let dirty_files = dirty_tracked_files(&st);

    // 切替で内容が変わるファイル = HEAD と切替先の差分
    let changed: Vec<String> = run_git(repo, &["diff", "--name-only", "HEAD", &resolved])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // git が切替を拒否するのは「未コミット変更 かつ 切替で上書きされる」ファイルだけ。
    // それ以外の変更は切替後も残る（= 消えない）ので、両者を分けて提示する
    let blocking_files: Vec<String> = dirty_files
        .iter()
        .filter(|p| changed.contains(p))
        .cloned()
        .collect();
    let carried_files: Vec<String> = dirty_files
        .iter()
        .filter(|p| !changed.contains(p))
        .cloned()
        .collect();

    let mut blockers = Vec::new();
    let conflict = conflict_state(repo);
    if conflict.is_active() {
        blockers.push(format!(
            "{} が進行中です。先に解消するか中止してください",
            conflict.operation.as_str()
        ));
    }
    if !current.is_empty() && current == resolved {
        blockers.push(format!("すでに '{current}' にいます"));
    }

    Ok(CheckoutPreview {
        target: resolved,
        current,
        dirty_files,
        blocking_files,
        carried_files,
        changed_files: changed.len(),
        creates_local_branch,
        blockers,
    })
}

/// ブランチを切り替える（#496）。`preview` の内容を承諾済みである前提で実行する
pub fn checkout(repo: &Path, target: &str) -> Result<String, String> {
    validate_branch_name(target)?;
    let (resolved, creates_local_branch) = resolve_checkout_target(repo, target)?;
    let out = if creates_local_branch {
        // リモート追跡ブランチ → 同名ローカルブランチを作って追跡設定つきで切り替える
        run_git_raw(repo, &["checkout", "--track", &resolved])?
    } else {
        run_git_raw(repo, &["checkout", &resolved])?
    };
    if out.success {
        // git checkout は進捗を stderr に出す。成功時はそちらが本文になる
        Ok(merge_streams(&out.stdout, &out.stderr))
    } else {
        Err(out.stderr.trim().to_string())
    }
}

/// 新規ブランチを作成する（#496）。`start_point` 省略時は現在の HEAD が基点。
/// `checkout` = true でそのまま切り替える
pub fn create_branch(
    repo: &Path,
    name: &str,
    start_point: Option<&str>,
    switch: bool,
) -> Result<String, String> {
    validate_branch_name(name)?;
    if local_branch_exists(repo, name) {
        return Err(format!("ブランチ '{name}' はすでに存在します"));
    }
    if let Some(start) = start_point {
        validate_branch_name(start)?;
        if !ref_exists(repo, start) {
            return Err(format!("基点 '{start}' が見つかりません"));
        }
    }
    let mut args: Vec<&str> = if switch {
        vec!["checkout", "-b", name]
    } else {
        vec!["branch", name]
    };
    if let Some(start) = start_point {
        args.push(start);
    }
    let out = run_git_raw(repo, &args)?;
    if out.success {
        Ok(merge_streams(&out.stdout, &out.stderr))
    } else {
        Err(out.stderr.trim().to_string())
    }
}

/// 2 本のストリームを表示用に 1 本へまとめる（空側は落とす）
fn merge_streams(stdout: &str, stderr: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        parts.push(stderr.trim());
    }
    parts.join("\n")
}

/// マージの種類（#496）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeKind {
    /// すでに取り込み済み（何も起きない）
    UpToDate,
    /// 早送り（コンフリクトは起こり得ない）
    FastForward,
    /// 3-way マージ（コンフリクトが起こり得る）
    ThreeWay,
    /// 共通祖先が無い（`--allow-unrelated-histories` が必要）
    Unrelated,
}

impl MergeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeKind::UpToDate => "up-to-date",
            MergeKind::FastForward => "fast-forward",
            MergeKind::ThreeWay => "three-way",
            MergeKind::Unrelated => "unrelated",
        }
    }
}

/// マージの事前提示（#496）。**作業ツリーに一切触れずに**結果を予測する
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePreview {
    pub target: String,
    pub current: String,
    pub kind: MergeKind,
    /// 取り込まれるコミット数
    pub incoming_commits: usize,
    /// 変更されるファイル
    pub changed_files: Vec<String>,
    /// コンフリクトすると予測されるファイル（`git merge-tree` による事前計算）
    pub predicted_conflicts: Vec<String>,
    /// 予測が実行できたか（古い git では false = 予測なしで実行することになる）
    pub prediction_available: bool,
    pub dirty_files: Vec<String>,
    pub blockers: Vec<String>,
}

impl MergePreview {
    /// マージは常に事前提示する（Issue #496: 破壊的になり得る操作は必ず何が起きるかを示す）
    pub fn needs_confirmation(&self) -> bool {
        true
    }
}

pub fn merge_preview(repo: &Path, target: &str) -> Result<MergePreview, String> {
    validate_branch_name(target)?;
    if !ref_exists(repo, target) {
        return Err(format!("ブランチ '{target}' が見つかりません"));
    }
    let st = status(repo);
    let current = st.branch.clone();
    let dirty_files = dirty_tracked_files(&st);

    let has_base = run_git(repo, &["merge-base", "HEAD", target]).is_ok();
    let kind = if !has_base {
        MergeKind::Unrelated
    } else if run_git(repo, &["merge-base", "--is-ancestor", target, "HEAD"]).is_ok() {
        MergeKind::UpToDate
    } else if run_git(repo, &["merge-base", "--is-ancestor", "HEAD", target]).is_ok() {
        MergeKind::FastForward
    } else {
        MergeKind::ThreeWay
    };

    let incoming_commits = run_git(repo, &["rev-list", "--count", &format!("HEAD..{target}")])
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let changed_files: Vec<String> = run_git(repo, &["diff", "--name-only", "HEAD", target])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // コンフリクト予測。`git merge-tree --write-tree` は作業ツリー・index を
    // 一切変更せずにマージ結果を計算する（git 2.38+）。使えない git では予測なしと明示する
    let (predicted_conflicts, prediction_available) = if kind == MergeKind::ThreeWay {
        predict_merge_conflicts(repo, target)
    } else {
        (Vec::new(), true)
    };

    let mut blockers = Vec::new();
    let conflict = conflict_state(repo);
    if conflict.is_active() {
        blockers.push(format!(
            "{} が進行中です。先に解消するか中止してください",
            conflict.operation.as_str()
        ));
    }
    if !current.is_empty() && current == target {
        blockers.push("現在のブランチを自分自身へマージすることはできません".into());
    }
    // 未コミット変更がマージ対象と重なると git はマージを拒否する
    let overlapping: Vec<&String> = dirty_files
        .iter()
        .filter(|p| changed_files.contains(p))
        .collect();
    if !overlapping.is_empty() {
        blockers.push(format!(
            "未コミットの変更がマージ対象と重なっています（{} 件）。先にコミットするか退避してください",
            overlapping.len()
        ));
    }
    if kind == MergeKind::Unrelated {
        blockers.push("共通の祖先がありません（無関係な履歴のマージ）".into());
    }

    Ok(MergePreview {
        target: target.to_string(),
        current,
        kind,
        incoming_commits,
        changed_files,
        predicted_conflicts,
        prediction_available,
        dirty_files,
        blockers,
    })
}

/// `git merge-tree --write-tree` でコンフリクトを事前計算する。
/// 戻り値の 2 番目は「予測が実行できたか」（古い git では false）
fn predict_merge_conflicts(repo: &Path, target: &str) -> (Vec<String>, bool) {
    let Ok(out) = run_git_raw(
        repo,
        &["merge-tree", "--write-tree", "--name-only", "HEAD", target],
    ) else {
        return (Vec::new(), false);
    };
    if out.success {
        // 成功 = コンフリクトなし（1 行目はマージ結果のツリー OID）
        return (Vec::new(), true);
    }
    // `--write-tree` 非対応の古い git は使い方エラーで落ちる。この場合は「予測不能」
    if out.stdout.trim().is_empty() {
        return (Vec::new(), false);
    }
    (parse_merge_tree_conflicts(&out.stdout), true)
}

/// `git merge-tree --write-tree --name-only` の出力からコンフリクトファイルを取り出す。
/// 形式: 1 行目 = ツリー OID / 2 ブロック目 = 衝突ファイル名 / 空行 / 以降は説明メッセージ
fn parse_merge_tree_conflicts(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// マージの実行結果（#496）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// コンフリクトで停止したか（= エラーではなく「解消待ち」状態）
    pub conflicted: bool,
    pub output: String,
    /// 未解決ファイル（conflicted のときのみ）
    pub conflicts: Vec<String>,
}

/// ブランチを現在のブランチへマージする（#496）。
/// コンフリクトは**エラーではなく結果**として返す（解消カードへ繋ぐため）
pub fn merge(repo: &Path, target: &str, no_ff: bool) -> Result<MergeOutcome, String> {
    validate_branch_name(target)?;
    if !ref_exists(repo, target) {
        return Err(format!("ブランチ '{target}' が見つかりません"));
    }
    // --no-edit: エディタが無い環境（GUI から起動した tako）でマージコミットを止めない
    let mut args = vec!["merge", "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    args.push(target);
    let out = run_git_raw(repo, &args)?;
    let text = merge_streams(&out.stdout, &out.stderr);
    if out.success {
        return Ok(MergeOutcome {
            conflicted: false,
            output: text,
            conflicts: Vec::new(),
        });
    }
    // 終了コード != 0 のうち、コンフリクト停止だけは正常系として扱う
    let state = conflict_state(repo);
    if state.is_active() && !state.files.is_empty() {
        return Ok(MergeOutcome {
            conflicted: true,
            output: text,
            conflicts: state.files,
        });
    }
    Err(if text.is_empty() {
        "マージに失敗しました".to_string()
    } else {
        text
    })
}

/// 進行中の操作を中止する（#496）。merge / rebase / cherry-pick / revert に対応
pub fn abort_operation(repo: &Path) -> Result<(RepoOperation, String), String> {
    let state = conflict_state(repo);
    let Some(args) = state.operation.abort_args() else {
        return Err("中止できる操作が進行中ではありません".into());
    };
    let out = run_git_raw(repo, &args)?;
    if out.success {
        Ok((state.operation, merge_streams(&out.stdout, &out.stderr)))
    } else {
        Err(out.stderr.trim().to_string())
    }
}

// ──────────────────────── テスト ────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_log_基本() {
        let raw =
            "abc123full\x01abc123\x01Alice\x012 hours ago\x01fix bug\x01HEAD -> main\x01def456\n\
                   def456full\x01def456\x01Bob\x013 hours ago\x01init\x01\x01\n";
        let commits = parse_log(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_hash, "abc123");
        assert_eq!(commits[0].author, "Alice");
        assert_eq!(commits[0].refs, "HEAD -> main");
        assert_eq!(commits[0].parents, vec!["def456"]);
        assert!(commits[1].parents.is_empty());
    }

    #[test]
    fn parse_branches_基本() {
        let raw = "*\tmain\tabc1234\tlatest commit\n \tfeature/x\tdef5678\twip\n \tremotes/origin/main\tabc1234\tlatest commit\n";
        let branches = parse_branches(raw);
        assert_eq!(branches.len(), 3);
        assert!(branches[0].is_current);
        assert_eq!(branches[0].name, "main");
        assert!(!branches[1].is_current);
        assert!(branches[2].is_remote);
    }

    #[test]
    fn parse_status_基本() {
        let raw = "# branch.head main\n# branch.upstream origin/main\n1 .M N... 100644 100644 100644 abc def src/main.rs\n? new_file.txt\n";
        let status = parse_status(raw);
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream, "origin/main");
        assert_eq!(status.entries.len(), 2);
        assert_eq!(status.entries[0].index, '.');
        assert_eq!(status.entries[0].worktree, 'M');
        assert_eq!(status.entries[1].index, '?');
    }

    /// #494: マージ未解決（porcelain v2 の `u` レコード）を拾えること。
    /// 拾えていなかったため、コンフリクト中は git パネルが「変更はありません」になっていた
    #[test]
    fn parse_statusはマージ未解決を拾う() {
        let raw = "# branch.head main\n\
                   u UU N... 100644 100644 100644 100644 aaa bbb ccc f.txt\n\
                   1 .M N... 100644 100644 100644 abc def other.rs\n";
        let status = parse_status(raw);
        assert_eq!(status.entries.len(), 2);
        let conflict = &status.entries[0];
        assert_eq!(conflict.path, "f.txt");
        assert!(conflict.is_conflicted());
        // 解決前なのでステージ済みには出さず、未ステージ側にだけ出す
        assert!(!conflict.is_staged());
        assert!(conflict.is_unstaged());
        assert_eq!(conflict.unstaged_badge(), CONFLICT_BADGE);
        // 通常の変更は従来どおり
        assert!(!status.entries[1].is_conflicted());
        assert!(status.entries[1].is_unstaged());
    }

    /// #494: 1 行入力欄向けの正規化。改行・タブ・制御文字は空白へ潰す
    #[test]
    fn コミットメッセージの正規化() {
        assert_eq!(sanitize_commit_message("fix: bug"), "fix: bug");
        // 改行・タブ・その他の制御文字はすべて半角空白へ
        assert_eq!(
            sanitize_commit_message("一行目\n二行目\tタブ\u{7}ベル"),
            "一行目 二行目 タブ ベル"
        );
        // 絵文字・サロゲートペア相当の文字はそのまま通す
        let emoji = "修正 \u{1F600}\u{1F1EF}\u{1F1F5}";
        assert_eq!(sanitize_commit_message(emoji), emoji);
        // 前後の空白は入力途中を壊さないため保持する（コミット時に git 側が扱う）
        assert_eq!(sanitize_commit_message("  a  "), "  a  ");
    }

    /// #494: 上限超過は**文字境界**で切る（バイト境界で切ると String が壊れる）
    #[test]
    fn コミットメッセージの上限は文字境界で切る() {
        // 3 バイト文字を上限ちょうどより 1 文字分多く並べる
        let long = "あ".repeat(COMMIT_MESSAGE_MAX / 3 + 10);
        let out = sanitize_commit_message(&long);
        assert!(out.len() <= COMMIT_MESSAGE_MAX);
        // 文字境界で切れている = そのまま chars で走査できて全部「あ」
        assert!(out.chars().all(|c| c == 'あ'));
        assert_eq!(out.len() % 3, 0);
        // 4 バイト文字（絵文字）でも境界を割らない
        let emoji = "\u{1F600}".repeat(COMMIT_MESSAGE_MAX / 4 + 10);
        let out = sanitize_commit_message(&emoji);
        assert!(out.len() <= COMMIT_MESSAGE_MAX);
        assert!(out.chars().all(|c| c == '\u{1F600}'));
    }

    /// #494: コミット可否の判定は UI・CLI で共通
    #[test]
    fn コミット可否の判定() {
        assert_eq!(commit_block("", true), Some(CommitBlock::EmptyMessage));
        // 空白のみ（全角空白・タブを含む）も「空」扱い
        assert_eq!(
            commit_block("   \u{3000}\t ", true),
            Some(CommitBlock::EmptyMessage)
        );
        assert_eq!(commit_block("fix", false), Some(CommitBlock::NoChanges));
        assert_eq!(commit_block("fix", true), None);
        // メッセージが空なら変更の有無に関わらず「空」を優先して案内する
        assert_eq!(commit_block("", false), Some(CommitBlock::EmptyMessage));
    }

    /// #487: ステージ済み / 未ステージの分類（porcelain v2 の XY 2 文字を分離する）
    #[test]
    fn ステージ状態の分類() {
        // "M." = index だけ変更（= ステージ済み）
        let staged_only = GitStatusEntry {
            path: "a.rs".into(),
            index: 'M',
            worktree: '.',
        };
        assert!(staged_only.is_staged());
        assert!(!staged_only.is_unstaged());
        assert_eq!(staged_only.staged_badge(), 'M');

        // ".M" = worktree だけ変更（= 未ステージ）
        let unstaged_only = GitStatusEntry {
            path: "b.rs".into(),
            index: '.',
            worktree: 'M',
        };
        assert!(!unstaged_only.is_staged());
        assert!(unstaged_only.is_unstaged());
        assert_eq!(unstaged_only.unstaged_badge(), 'M');

        // "MM" = 両方に出る（VSCode 同様、ステージ済みと未ステージの両セクションに並ぶ）
        let both = GitStatusEntry {
            path: "c.rs".into(),
            index: 'M',
            worktree: 'M',
        };
        assert!(both.is_staged());
        assert!(both.is_unstaged());

        // untracked は未ステージ側のみ・バッジは U
        let untracked = GitStatusEntry {
            path: "d.rs".into(),
            index: '?',
            worktree: '?',
        };
        assert!(!untracked.is_staged());
        assert!(untracked.is_unstaged());
        assert!(untracked.is_untracked());
        assert_eq!(untracked.unstaged_badge(), 'U');

        // 追加をステージ済み ("A.")
        let added = GitStatusEntry {
            path: "e.rs".into(),
            index: 'A',
            worktree: '.',
        };
        assert!(added.is_staged());
        assert_eq!(added.staged_badge(), 'A');
    }

    /// #487: 実際の porcelain v2 出力からの分類（混在状態）
    #[test]
    fn parse_status_からのステージ分類() {
        let raw = concat!(
            "# branch.head main\n",
            "1 M. N... 100644 100644 100644 aaa bbb staged.rs\n",
            "1 .M N... 100644 100644 100644 ccc ddd unstaged.rs\n",
            "1 MM N... 100644 100644 100644 eee fff both.rs\n",
            "? new.rs\n"
        );
        let status = parse_status(raw);
        let staged: Vec<&str> = status
            .entries
            .iter()
            .filter(|e| e.is_staged())
            .map(|e| e.path.as_str())
            .collect();
        let unstaged: Vec<&str> = status
            .entries
            .iter()
            .filter(|e| e.is_unstaged())
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(staged, vec!["staged.rs", "both.rs"]);
        assert_eq!(unstaged, vec!["unstaged.rs", "both.rs", "new.rs"]);
    }

    /// #487: 初期コミットのフォールバックが diff-tree になっていること（作業ツリー混入の回帰防止）
    #[test]
    fn 初期コミットのdiffはコミット内容そのもの() {
        let dir = std::env::temp_dir().join(format!("tako-git-root-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let repo = dir.as_path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git")
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        let hash = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        // 作業ツリーを汚す: 誤実装（git diff --root）だとこの変更が混ざる
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();

        let files = diff(repo, &DiffTarget::Commit(hash));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "a.txt");
        let lines: Vec<&str> = files[0].hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Add)
            .map(|l| l.content.as_str())
            .collect();
        // 初期コミットの中身 = "one" の追加のみ。"two" が出たら作業ツリーが混入している
        assert_eq!(lines, vec!["one"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_diff_基本() {
        let raw = "diff --git a/src/main.rs b/src/main.rs\nindex abc..def 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n fn main() {\n-    old();\n+    new();\n+    extra();\n }\n";
        let files = parse_diff(raw);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 5);
        assert_eq!(files[0].hunks[0].lines[1].kind, DiffLineKind::Remove);
        assert_eq!(files[0].hunks[0].lines[2].kind, DiffLineKind::Add);
    }

    #[test]
    fn parse_diff_複数ファイル() {
        let raw = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/b.rs b/b.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let files = parse_diff(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "b.rs");
    }

    // ──────────────────────── グラフレイアウトテスト ────────────────────────

    fn test_commit(hash: &str, parents: &[&str], refs: &str) -> GitCommit {
        GitCommit {
            hash: hash.to_string(),
            short_hash: hash[..1].to_string(),
            author: String::new(),
            date_relative: String::new(),
            subject: String::new(),
            refs: refs.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn graph_layout_線形() {
        let commits = vec![
            test_commit("A", &["B"], "HEAD -> main"),
            test_commit("B", &["C"], ""),
            test_commit("C", &[], ""),
        ];
        let layout = compute_graph_layout(&commits);
        assert_eq!(layout.rows.len(), 3);
        assert_eq!(layout.rows[0].lane, 0);
        assert_eq!(layout.rows[1].lane, 0);
        assert_eq!(layout.rows[2].lane, 0);
        assert_eq!(layout.max_lanes, 1);
        assert!(layout.ref_colors.contains_key("HEAD -> main"));
    }

    #[test]
    fn graph_layout_ブランチとマージ() {
        // A は B と C をマージ。B→D, C→D
        let commits = vec![
            test_commit("A", &["B", "C"], ""),
            test_commit("B", &["D"], ""),
            test_commit("C", &["D"], ""),
            test_commit("D", &[], ""),
        ];
        let layout = compute_graph_layout(&commits);
        assert_eq!(layout.rows[0].lane, 0); // A at lane 0
        assert_eq!(layout.rows[1].lane, 0); // B inherits lane 0
        assert_eq!(layout.rows[2].lane, 1); // C at lane 1
        assert_eq!(layout.rows[3].lane, 0); // D at lane 0
        assert!(layout.max_lanes >= 2);
    }

    #[test]
    fn graph_layout_並行ブランチ() {
        // A→C, B→C（独立した 2 ブランチがマージ）
        let commits = vec![
            test_commit("A", &["C"], ""),
            test_commit("B", &["C"], ""),
            test_commit("C", &[], ""),
        ];
        let layout = compute_graph_layout(&commits);
        assert_eq!(layout.rows[0].lane, 0); // A
        assert_eq!(layout.rows[1].lane, 1); // B（C は既にレーン 0）
        assert_eq!(layout.rows[2].lane, 0); // C
    }

    #[test]
    fn graph_layout_ルートコミット() {
        let commits = vec![test_commit("A", &[], "")];
        let layout = compute_graph_layout(&commits);
        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0].lane, 0);
        // ルートコミット（上に線なし・親なし）→ 描画指示なし
        assert!(layout.rows[0].lines.is_empty());
    }

    #[test]
    fn log_file_commitsは自リポの実ファイルで履歴を取れる() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        if repo_root(repo).is_none() {
            return;
        }
        let commits = log_file_commits(repo, "Cargo.toml", 5);
        assert!(!commits.is_empty(), "Cargo.toml に履歴がある");
        assert!(commits.len() <= 5);
        assert!(!commits[0].hash.is_empty());
    }

    #[test]
    fn ファイルパス経由のrepo_rootでlog_file_commitsが動く() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("Cargo.toml");
        let repo = match repo_root(&file) {
            Some(r) => r,
            None => return,
        };
        let rel = file
            .strip_prefix(&repo)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let commits = log_file_commits(&repo, &rel, 5);
        assert!(!commits.is_empty(), "ファイルパス経由でも履歴が取れる");
    }

    #[test]
    fn diff_file_commitは特定コミットのファイル差分を取れる() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        if repo_root(repo).is_none() {
            return;
        }
        let commits = log_file_commits(repo, "Cargo.toml", 2);
        if commits.len() < 2 {
            return; // コミット不足ならスキップ
        }
        let hunks = diff_file_commit(repo, &commits[0].hash, "Cargo.toml");
        // 最新コミットが Cargo.toml を変更していなければ空
        // 変更していれば hunk が取れる。どちらもパニックしない
        let _ = hunks;
    }

    #[test]
    fn repo_rootはファイルパスでも親ディレクトリから解決できる() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(file.is_file());
        let root = repo_root(&file);
        assert!(root.is_some(), "ファイルパスでも repo_root が取れる");
    }

    #[test]
    fn repo_rootはディレクトリパスでも従来どおり解決できる() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(dir.is_dir());
        let root = repo_root(dir);
        assert!(root.is_some(), "ディレクトリパスで repo_root が取れる");
    }

    #[test]
    fn repo_rootはリポ外のファイルで空を返す() {
        let file = std::path::Path::new("/tmp/.tako_test_nonexistent_file");
        let root = repo_root(file);
        assert!(root.is_none());
    }

    #[test]
    fn shortstatのパース() {
        assert_eq!(
            parse_shortstat(" 2 files changed, 126 insertions(+), 41 deletions(-)\n"),
            (126, 41)
        );
        // 単数形・片側のみ・空出力
        assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)\n"), (1, 0));
        assert_eq!(parse_shortstat(" 1 file changed, 3 deletions(-)\n"), (0, 3));
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    #[test]
    fn show_commitは通常コミットの詳細を取れる() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        if repo_root(repo).is_none() {
            return;
        }
        let commits = log_commits(repo, 3);
        if commits.is_empty() {
            return;
        }
        let detail = show_commit(repo, &commits[0].hash).expect("show_commit に成功する");
        assert_eq!(detail.hash, commits[0].hash);
        assert!(!detail.author_name.is_empty());
        assert!(!detail.author_email.is_empty());
        assert!(!detail.author_date.is_empty());
        assert!(!detail.subject.is_empty());
    }

    // ──────────────── ブランチ操作 / マージ（#496）────────────────

    /// 使い捨てリポジトリを作るヘルパ（#496）。**実リポジトリは絶対に触らない**
    struct TempRepo {
        dir: std::path::PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            // 同一プロセス内の複数テストが衝突しないよう、タグ + pid + カウンタで一意化する
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("tako-git-496-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            let repo = TempRepo { dir };
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            &self.dir
        }

        fn git(&self, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&self.dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.dir.join(name), content).expect("write");
        }

        fn commit(&self, name: &str, content: &str, message: &str) {
            self.write(name, content);
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        fn current_branch(&self) -> String {
            self.git(&["branch", "--show-current"])
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 3-way マージでコンフリクトする状態を作る（main と feat が同じ行を書き換える）
    fn setup_conflicting(repo: &TempRepo) {
        repo.commit("f.txt", "a\nb\nc\n", "init");
        repo.git(&["checkout", "-qb", "feat"]);
        repo.commit("f.txt", "a\nFEAT\nc\n", "feat change");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("f.txt", "a\nMAIN\nc\n", "main change");
    }

    #[test]
    fn ブランチ名の検証() {
        assert!(validate_branch_name("feature/x-1").is_ok());
        assert!(validate_branch_name("").is_err());
        // 引数注入の遮断（git checkout は -- でオプション終端を作れない）
        assert!(validate_branch_name("-d").is_err());
        assert!(validate_branch_name("--force").is_err());
        assert!(validate_branch_name("a b").is_err());
        assert!(validate_branch_name("a..b").is_err());
        assert!(validate_branch_name("a~1").is_err());
        assert!(validate_branch_name("a^").is_err());
        assert!(validate_branch_name("a:b").is_err());
        assert!(validate_branch_name("HEAD@{1}").is_err());
        assert!(validate_branch_name("/x").is_err());
        assert!(validate_branch_name("x/").is_err());
        assert!(validate_branch_name("a//b").is_err());
        assert!(validate_branch_name("x.lock").is_err());
        assert!(validate_branch_name("x.").is_err());
        assert!(validate_branch_name(".hidden").is_err());
        assert!(validate_branch_name("a/.hidden").is_err());
    }

    #[test]
    fn ブランチ作成と切替() {
        let repo = TempRepo::new("create");
        repo.commit("a.txt", "one\n", "init");

        // HEAD 基点で作成 + 切替
        let out = create_branch(repo.path(), "topic", None, true).expect("作成できる");
        assert!(!out.is_empty());
        assert_eq!(repo.current_branch(), "topic");

        // 既存名は拒否（git のエラーに落ちる前に日本語で返す）
        let err = create_branch(repo.path(), "topic", None, false).unwrap_err();
        assert!(err.contains("すでに存在"), "{err}");

        // 任意の基点から作成（切替なし）
        repo.commit("b.txt", "two\n", "second");
        create_branch(repo.path(), "from-main", Some("main"), false).expect("基点指定で作成できる");
        assert_eq!(
            repo.current_branch(),
            "topic",
            "switch=false では切り替わらない"
        );
        // 基点が main なので b.txt を含まない
        let files = repo.git(&["ls-tree", "-r", "--name-only", "from-main"]);
        assert!(!files.contains("b.txt"), "{files}");

        // 存在しない基点
        let err = create_branch(repo.path(), "bad", Some("nope"), false).unwrap_err();
        assert!(err.contains("見つかりません"), "{err}");

        // 切替
        checkout(repo.path(), "main").expect("切替できる");
        assert_eq!(repo.current_branch(), "main");

        // 存在しないブランチ
        let err = checkout(repo.path(), "ghost").unwrap_err();
        assert!(err.contains("見つかりません"), "{err}");
    }

    #[test]
    fn 切替の事前提示はクリーンなら確認不要() {
        let repo = TempRepo::new("preview-clean");
        repo.commit("a.txt", "one\n", "init");
        repo.git(&["branch", "topic"]);

        let p = checkout_preview(repo.path(), "topic").expect("preview");
        assert_eq!(p.target, "topic");
        assert_eq!(p.current, "main");
        assert!(p.dirty_files.is_empty());
        assert!(p.blockers.is_empty());
        assert!(!p.needs_confirmation(), "クリーンなら確認不要");
    }

    #[test]
    fn 切替の事前提示は未コミット変更を持ち越しと衝突に分ける() {
        let repo = TempRepo::new("preview-dirty");
        repo.commit("shared.txt", "base\n", "init");
        repo.commit("kept.txt", "keep\n", "second");
        repo.git(&["checkout", "-qb", "topic"]);
        // topic 側だけ shared.txt を変更 = 切替で入れ替わるファイル
        repo.commit("shared.txt", "topic\n", "topic change");
        repo.git(&["checkout", "-q", "main"]);

        // 未コミット変更を 2 つ作る: 片方は切替対象と重なる
        repo.write("shared.txt", "local edit\n");
        repo.write("kept.txt", "local keep\n");

        let p = checkout_preview(repo.path(), "topic").expect("preview");
        assert!(
            p.needs_confirmation(),
            "未コミット変更があるなら必ず提示する"
        );
        assert_eq!(p.dirty_files.len(), 2);
        assert_eq!(p.blocking_files, vec!["shared.txt".to_string()]);
        assert_eq!(p.carried_files, vec!["kept.txt".to_string()]);
        assert!(p.changed_files >= 1);

        // 提示どおり git は切替を拒否する（= 黙って壊さない）
        let err = checkout(repo.path(), "topic").unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(repo.current_branch(), "main", "切替は起きていない");
    }

    #[test]
    fn マージの事前提示は種別とコンフリクトを予測する() {
        let repo = TempRepo::new("merge-preview");
        setup_conflicting(&repo);

        let p = merge_preview(repo.path(), "feat").expect("preview");
        assert_eq!(p.kind, MergeKind::ThreeWay);
        assert_eq!(p.current, "main");
        assert_eq!(p.incoming_commits, 1);
        assert!(p.changed_files.contains(&"f.txt".to_string()));
        assert!(p.prediction_available, "git 2.38+ なら予測できる");
        assert_eq!(p.predicted_conflicts, vec!["f.txt".to_string()]);
        assert!(p.blockers.is_empty());
        assert!(p.needs_confirmation(), "マージは常に事前提示する");

        // 予測しただけでは作業ツリーもリポ状態も変わっていない
        assert!(!conflict_state(repo.path()).is_active());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
            "a\nMAIN\nc\n"
        );
    }

    #[test]
    fn マージの事前提示は早送りと取り込み済みを見分ける() {
        let repo = TempRepo::new("merge-kind");
        repo.commit("a.txt", "one\n", "init");
        repo.git(&["checkout", "-qb", "ahead"]);
        repo.commit("b.txt", "two\n", "ahead commit");
        repo.git(&["checkout", "-q", "main"]);

        let ff = merge_preview(repo.path(), "ahead").expect("preview");
        assert_eq!(ff.kind, MergeKind::FastForward);
        assert_eq!(ff.incoming_commits, 1);
        assert!(ff.predicted_conflicts.is_empty());

        // 逆向き（main は ahead に含まれている）は up-to-date
        repo.git(&["checkout", "-q", "ahead"]);
        let up = merge_preview(repo.path(), "main").expect("preview");
        assert_eq!(up.kind, MergeKind::UpToDate);
        assert_eq!(up.incoming_commits, 0);
    }

    #[test]
    fn マージ成功とコンフリクトと中止() {
        let repo = TempRepo::new("merge-run");
        setup_conflicting(&repo);

        // コンフリクトは Err ではなく「解消待ち」の結果として返る
        let outcome = merge(repo.path(), "feat", false).expect("コンフリクトは正常系");
        assert!(outcome.conflicted);
        assert_eq!(outcome.conflicts, vec!["f.txt".to_string()]);

        let state = conflict_state(repo.path());
        assert_eq!(state.operation, RepoOperation::Merging);
        assert!(state.is_active());
        assert_eq!(state.files, vec!["f.txt".to_string()]);
        assert_eq!(state.ours, "main");
        assert_eq!(state.theirs.as_deref(), Some("feat"));

        // コンフリクト中は切替もマージも事前に止める
        let p = checkout_preview(repo.path(), "feat").expect("preview");
        assert!(
            p.blockers.iter().any(|b| b.contains("merging")),
            "{:?}",
            p.blockers
        );

        // 中止で元へ戻る
        let (op, _) = abort_operation(repo.path()).expect("中止できる");
        assert_eq!(op, RepoOperation::Merging);
        let after = conflict_state(repo.path());
        assert!(!after.is_active());
        assert!(after.files.is_empty());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
            "a\nMAIN\nc\n"
        );

        // 中止対象が無ければエラー
        assert!(abort_operation(repo.path()).is_err());

        // 衝突しないマージは成功として返る
        repo.git(&["checkout", "-qb", "other", "main"]);
        repo.commit("g.txt", "other\n", "other change");
        repo.git(&["checkout", "-q", "main"]);
        let ok = merge(repo.path(), "other", false).expect("マージ成功");
        assert!(!ok.conflicted);
        assert!(repo.path().join("g.txt").exists());
    }

    #[test]
    fn リモート追跡ブランチの切替はローカルブランチを作る() {
        // 「リモート」役の裸リポジトリを用意して clone する（ネットワーク不要）
        let origin = TempRepo::new("remote-origin");
        origin.commit("a.txt", "one\n", "init");
        origin.git(&["checkout", "-qb", "release"]);
        origin.commit("b.txt", "two\n", "release commit");
        origin.git(&["checkout", "-q", "main"]);

        let clone_dir =
            std::env::temp_dir().join(format!("tako-git-496-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&clone_dir);
        let out = std::process::Command::new("git")
            .args([
                "clone",
                "-q",
                &origin.path().display().to_string(),
                &clone_dir.display().to_string(),
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("clone");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let p = checkout_preview(&clone_dir, "origin/release").expect("preview");
        assert!(
            p.creates_local_branch,
            "リモート追跡はローカルブランチを作る"
        );
        assert_eq!(p.target, "origin/release");

        checkout(&clone_dir, "origin/release").expect("切替できる");
        let branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&clone_dir)
            .output()
            .expect("branch");
        assert_eq!(
            String::from_utf8_lossy(&branch.stdout).trim(),
            "release",
            "detached HEAD ではなくローカル追跡ブランチになる"
        );
        let _ = std::fs::remove_dir_all(&clone_dir);
    }

    #[test]
    fn merge_treeの出力からコンフリクトファイルを取り出す() {
        let raw = "f02c3990f5aaff78cb586e4ed423760394f2c5aa\n\
                   src/a.rs\n\
                   src/b.rs\n\
                   \n\
                   Auto-merging src/a.rs\n\
                   CONFLICT (content): Merge conflict in src/a.rs\n";
        assert_eq!(
            parse_merge_tree_conflicts(raw),
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
        // コンフリクトなし（ツリー OID のみ）
        assert!(parse_merge_tree_conflicts("f02c399\n").is_empty());
    }

    #[test]
    fn show_commitは初期コミットでも破綻しない() {
        let dir = std::env::temp_dir().join(format!("tako-git-show-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let repo = dir.as_path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git")
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        std::fs::write(repo.join("b.txt"), "world\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "initial commit\n\nbody text here"]);
        let hash = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        let detail = show_commit(repo, &hash).expect("初期コミットの show_commit に成功する");
        assert!(detail.parents.is_empty());
        assert_eq!(detail.subject, "initial commit");
        assert_eq!(detail.body, "body text here");
        assert_eq!(detail.files.len(), 2);
        assert!(detail.files.iter().all(|f| f.kind == 'A'));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// パス表記の可搬性と改行コード耐性（#520）。
/// **Windows 実機が無くても macOS 上で検証できる形**にしてある
#[cfg(test)]
mod portability_tests {
    use super::*;

    #[test]
    fn to_git_pathは区切りを常にスラッシュにする() {
        // unix では元から `/` なので不変
        assert_eq!(to_git_path(Path::new("src/foo.rs")), "src/foo.rs");
        assert_eq!(to_git_path(Path::new("a/b/c.txt")), "a/b/c.txt");
    }

    #[test]
    fn repo_relativeはgit表記の相対パスを返す() {
        let repo = Path::new("/tmp/repo");
        assert_eq!(
            repo_relative(repo, Path::new("/tmp/repo/src/foo.rs")).as_deref(),
            Some("src/foo.rs")
        );
        // リポジトリ自身は相対パスにならない
        assert_eq!(repo_relative(repo, repo), None);
        // リポ外は None（フルパスへ勝手にフォールバックしない）
        assert_eq!(repo_relative(repo, Path::new("/etc/hosts")), None);
    }

    #[test]
    fn from_git_pathはスラッシュ区切りを実パスへ戻す() {
        let repo = Path::new("/tmp/repo");
        assert_eq!(
            from_git_path(repo, "src/foo.rs"),
            Path::new("/tmp/repo/src/foo.rs")
        );
        // 余計な区切り・カレント指定は畳む
        assert_eq!(
            from_git_path(repo, "./src//foo.rs"),
            Path::new("/tmp/repo/src/foo.rs")
        );
        assert_eq!(from_git_path(repo, ""), repo);
    }

    #[test]
    fn to_git_pathとfrom_git_pathは往復する() {
        let repo = Path::new("/tmp/repo");
        for rel in ["src/foo.rs", "a/b/c/d.txt", "README.md"] {
            let abs = from_git_path(repo, rel);
            assert_eq!(repo_relative(repo, &abs).as_deref(), Some(rel));
        }
    }

    #[test]
    fn normalize_repo_rootは末尾の改行を落とす() {
        // git の出力は改行付き。CRLF でも壊れないこと
        assert_eq!(normalize_repo_root("/tmp/repo\n"), Path::new("/tmp/repo"));
        assert_eq!(normalize_repo_root("/tmp/repo\r\n"), Path::new("/tmp/repo"));
        assert_eq!(normalize_repo_root("  /tmp/repo  "), Path::new("/tmp/repo"));
    }

    #[test]
    fn normalize_repo_rootはドライブレター表記をそのまま扱える() {
        // git は Windows でも `/` で返す。PathBuf は `/` を区切りとして解釈するので
        // そのまま渡してよい（分解できることを確認する）
        let p = normalize_repo_root("C:/Users/dev/repo\n");
        assert!(p.to_string_lossy().starts_with("C:/") || p.to_string_lossy().starts_with("C:\\"));
        assert!(p.to_string_lossy().ends_with("repo"));
    }

    /// CRLF のリポジトリで git 出力を受けても各パーサが壊れないこと。
    /// `str::lines()` が `\r` を落とすことに依存しているので、退行の検出用に固定する
    #[test]
    fn 各パーサはcrlf出力でも壊れない() {
        let log = "h1\u{1}s1\u{1}alice\u{1}2 days ago\u{1}件名 A\u{1}\u{1}p1\r\n\
                   h2\u{1}s2\u{1}bob\u{1}3 days ago\u{1}件名 B\u{1}HEAD -> main\u{1}\r\n";
        let commits = parse_log(log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "件名 A");
        assert_eq!(commits[0].parents, vec!["p1".to_string()]);
        // \r が subject / refs に紛れ込んでいないこと
        assert!(!commits[1].refs.contains('\r'));
        assert_eq!(commits[1].subject, "件名 B");

        let branches =
            parse_branches("*\tmain\tabc1234\t最新のコミット\r\n \tfeat/x\tdef5678\t作業中\r\n");
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].is_current);
        // 末尾フィールドに CR が残らないこと（subject は行末なので最も危ない）
        assert_eq!(branches[0].subject, "最新のコミット");
        assert_eq!(branches[1].subject, "作業中");
        assert!(!branches[1].name.contains('\r'));

        let status = parse_status(
            "# branch.head main\r\n\
             1 .M N... 100644 100644 100644 aaa bbb src/foo.rs\r\n\
             ? untracked.txt\r\n",
        );
        assert_eq!(status.branch, "main");
        assert!(
            status.entries.iter().all(|f| !f.path.contains('\r')),
            "パスに CR が残っている: {:?}",
            status.entries
        );
        assert!(status.entries.iter().any(|f| f.path == "src/foo.rs"));
        assert!(status.entries.iter().any(|f| f.path == "untracked.txt"));
    }

    /// git はパスを常に `/` で返す。CRLF が混ざってもパスが壊れないこと
    #[test]
    fn parse_diffはcrlfでもファイルパスを取り違えない() {
        let raw = "diff --git a/src/foo.rs b/src/foo.rs\r\n\
                   index 111..222 100644\r\n\
                   --- a/src/foo.rs\r\n\
                   +++ b/src/foo.rs\r\n\
                   @@ -1,2 +1,2 @@\r\n\
                   -old line\r\n\
                   +new line\r\n";
        let files = parse_diff(raw);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/foo.rs");
        assert!(!files[0].path.contains('\r'));
        let lines: Vec<&str> = files[0]
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect();
        assert!(
            lines.iter().all(|l| !l.contains('\r')),
            "diff 行に CR が残っている: {lines:?}"
        );
    }
}

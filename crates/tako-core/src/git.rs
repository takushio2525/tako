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
pub(crate) fn git_bin() -> &'static str {
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
    /// index（staging）側の状態（M/A/D/R 等。'.' は変更なし）
    pub index: char,
    /// worktree 側の状態
    pub worktree: char,
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
    let output = git_command(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git を実行できない: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// cwd から git リポジトリのルートを解決する（`git rev-parse --show-toplevel`）。
/// リポ外なら None
pub fn repo_root(cwd: &Path) -> Option<std::path::PathBuf> {
    let output = git_command(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !root.is_empty() {
            return Some(std::path::PathBuf::from(root));
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
/// `git diff <hash>^..<hash> -- <file>`（初期コミットは `--root` フォールバック）
pub fn diff_file_commit(repo: &Path, hash: &str, file_path: &str) -> Vec<DiffHunk> {
    let out = run_git(
        repo,
        &["diff", &format!("{hash}^..{hash}"), "--", file_path],
    );
    let raw = match out {
        Ok(s) => s,
        Err(_) => run_git(repo, &["diff", "--root", hash, "--", file_path]).unwrap_or_default(),
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
    /// 特定コミットの diff（`git diff <commit>^..<commit>`。初期コミットは
    /// `git diff --root <commit>` へフォールバック）
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
            // 初期コミット: 親がないので --root でフォールバック
            if let DiffTarget::Commit(hash) = target {
                run_git(repo, &["diff", "--root", hash]).unwrap_or_default()
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
            return; // git リポ外（CI 等）ではスキップ
        }
        let commits = log_file_commits(repo, "Cargo.toml", 5);
        assert!(!commits.is_empty(), "Cargo.toml に履歴がある");
        assert!(commits.len() <= 5);
        assert!(!commits[0].hash.is_empty());
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
}

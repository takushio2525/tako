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

/// git バイナリの場所（tmux_bin と同パターン、プロセス内 1 回解決）
pub fn git_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(resolve_git_bin)
}

fn resolve_git_bin() -> String {
    if let Some(bin) = std::env::var_os("TAKO_GIT_BIN") {
        if !bin.is_empty() {
            return bin.to_string_lossy().into_owned();
        }
    }
    if Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "git".into();
    }
    for candidate in [
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
        "/usr/bin/git",
    ] {
        if Path::new(candidate).is_file() {
            return candidate.into();
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = Command::new(shell)
            .args(["-l", "-c", "command -v git"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && Path::new(&path).is_file() {
                    return path;
                }
            }
        }
    }
    "git".into()
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
}

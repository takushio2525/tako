//! git_tree — ファイルツリーに出す git ステータス（#1009）
//!
//! サイドバーのファイルツリーで「未コミット・未ステージ・新規」をひと目で見せるための
//! **正本**。GPUI 非依存で、判定は全部ここの純粋関数に閉じる（UI は色を塗るだけ・
//! CLI / MCP は同じ表を JSON にするだけ）。
//!
//! - 分類は `git status --porcelain=v2`（`git::status_tree`）の index / worktree の 2 列から作る。
//!   **ステージ済みと未ステージを畳まない**のがここの肝で、`MM` のように git 自身の
//!   `--short` と同じ 2 桁で見せられるようにしてある
//! - ディレクトリには配下の変更が**伝播**する（VSCode 同様）。伝播は git が返したパスの
//!   祖先を辿って作るので、**折りたたまれたディレクトリでも中身を読まずに色が付く**
//! - `.gitignore` 対象は `Ignored`。丸ごと無視されたディレクトリは 1 行（`target/`）で
//!   返るので、その配下は「祖先が無視ならば無視」で解決する（配下を列挙しない）

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::git::{self, GitStatus, GitStatusEntry};

/// 1 リポジトリあたりに取り込むエントリ数の上限。
/// `.gitignore` に `*.log` のようなパターンがあって無視ファイルが数万件ある
/// リポジトリでも、ツリーの表示のために全部を持たない（表示は画面ぶんしか要らない）
pub const MAX_ENTRIES_PER_REPO: usize = 20_000;

/// ツリー 1 行の git 状態の分類。**色を決めるのはこれ 1 つ**（バッジ文字とは別）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TreeGitState {
    /// `.gitignore` 対象（変更ではない = 薄く見せる）
    Ignored,
    /// 未追跡（新規ファイル）
    Untracked,
    /// index に追加済みの新規
    Added,
    /// 変更
    Modified,
    /// リネーム / コピー
    Renamed,
    /// 削除
    Deleted,
    /// マージ未解決
    Conflicted,
}

impl TreeGitState {
    /// ディレクトリへ伝播させるときの優先度（大きいほど強い = そちらの色になる）。
    /// 「まず手を入れないといけないもの」から順に強い:
    /// 未解決 > 削除 > リネーム > 変更 > 追加 > 未追跡 > 無視
    pub fn severity(self) -> u8 {
        match self {
            TreeGitState::Ignored => 0,
            TreeGitState::Untracked => 1,
            TreeGitState::Added => 2,
            TreeGitState::Modified => 3,
            TreeGitState::Renamed => 4,
            TreeGitState::Deleted => 5,
            TreeGitState::Conflicted => 6,
        }
    }

    /// 機械可読な種別名（CLI / MCP の JSON。UI の文言とは別物なので翻訳しない）
    pub fn code(self) -> &'static str {
        match self {
            TreeGitState::Ignored => "ignored",
            TreeGitState::Untracked => "untracked",
            TreeGitState::Added => "added",
            TreeGitState::Modified => "modified",
            TreeGitState::Renamed => "renamed",
            TreeGitState::Deleted => "deleted",
            TreeGitState::Conflicted => "conflicted",
        }
    }

    /// git の XY 1 文字から分類する（`git status --short` の記号と同じ読み方）
    fn from_code(c: char) -> TreeGitState {
        match c {
            'D' => TreeGitState::Deleted,
            'R' | 'C' => TreeGitState::Renamed,
            'A' => TreeGitState::Added,
            '?' => TreeGitState::Untracked,
            _ => TreeGitState::Modified,
        }
    }
}

/// ツリー 1 行の git 状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeGitStatus {
    /// 色を決める分類（ディレクトリなら配下でいちばん強いもの）
    pub state: TreeGitState,
    /// index 側（ステージ済み）のバッジ文字。`None` = ステージ済みの変更なし
    pub staged: Option<char>,
    /// worktree 側（未ステージ）のバッジ文字。`None` = 未ステージの変更なし
    pub unstaged: Option<char>,
    /// 配下からの伝播（= ディレクトリ行）。ファイル自身の状態なら false
    pub from_children: bool,
    /// 変更ファイル数（ファイルは 1、ディレクトリは配下の変更ファイル数）
    pub changed: usize,
}

impl TreeGitStatus {
    /// `.gitignore` 対象の行
    fn ignored() -> Self {
        TreeGitStatus {
            state: TreeGitState::Ignored,
            staged: None,
            unstaged: None,
            from_children: false,
            changed: 0,
        }
    }

    /// git の 1 エントリ（ファイル）から作る。無視・パス空なら `None`
    fn from_entry(entry: &GitStatusEntry) -> Option<Self> {
        if entry.is_ignored() {
            return None;
        }
        if entry.is_conflicted() {
            return Some(TreeGitStatus {
                state: TreeGitState::Conflicted,
                staged: None,
                unstaged: Some(git::CONFLICT_BADGE),
                from_children: false,
                changed: 1,
            });
        }
        if entry.is_untracked() {
            return Some(TreeGitStatus {
                state: TreeGitState::Untracked,
                staged: None,
                unstaged: Some('U'),
                from_children: false,
                changed: 1,
            });
        }
        let staged = (entry.index != '.' && entry.index != ' ').then_some(entry.index);
        let unstaged = (entry.worktree != '.' && entry.worktree != ' ').then_some(entry.worktree);
        if staged.is_none() && unstaged.is_none() {
            return None;
        }
        // 色は index / worktree のうち強い方（例: index=A / worktree=D なら削除色）
        let state = [staged, unstaged]
            .into_iter()
            .flatten()
            .map(TreeGitState::from_code)
            .max_by_key(|s| s.severity())
            .unwrap_or(TreeGitState::Modified);
        Some(TreeGitStatus {
            state,
            staged,
            unstaged,
            from_children: false,
            changed: 1,
        })
    }

    /// ディレクトリ行として子の状態を取り込む
    fn absorb_child(&mut self, child: &TreeGitStatus) {
        if child.state.severity() > self.state.severity() {
            self.state = child.state;
        }
        self.changed = self.changed.saturating_add(child.changed);
    }

    /// 伝播由来のディレクトリ行の初期値
    fn propagated(child: &TreeGitStatus) -> Self {
        TreeGitStatus {
            state: child.state,
            staged: None,
            unstaged: None,
            from_children: true,
            changed: child.changed,
        }
    }

    /// バッジに出す文字列（ファイル = git の XY 2 桁 / ディレクトリ = 変更件数）。
    /// UI は色を 2 色に塗り分けるので描画では使わないが、CLI / MCP と
    /// テストが「何が見えるか」を 1 箇所から引くための正本
    pub fn badge(&self) -> String {
        if self.state == TreeGitState::Ignored {
            return String::new();
        }
        if self.from_children {
            return if self.changed > 99 {
                "99+".to_string()
            } else {
                self.changed.to_string()
            };
        }
        let mut out = String::new();
        if let Some(c) = self.staged {
            out.push(c);
        }
        if let Some(c) = self.unstaged {
            out.push(c);
        }
        out
    }
}

/// リポジトリ 1 件の要約（CLI / MCP の応答に載せる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    pub root: PathBuf,
    pub branch: String,
    /// 変更ファイル数（無視は含まない）
    pub changed: usize,
    /// 取り込みを上限で打ち切ったか（打ち切ると表示が実態より少なくなる）
    pub truncated: bool,
}

/// ツリー全体の git 状態の表
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeGitMap {
    entries: HashMap<PathBuf, TreeGitStatus>,
    /// 丸ごと無視されたディレクトリ（配下は祖先照合で無視と判定する）
    ignored_dirs: HashSet<PathBuf>,
    repos: Vec<RepoSummary>,
}

impl TreeGitMap {
    /// この行の状態。完全一致が無ければ「祖先が丸ごと無視か」を見る
    /// （`target/` の 1 行で `target/debug/foo` まで薄くするため）
    pub fn get(&self, path: &Path) -> Option<TreeGitStatus> {
        if let Some(hit) = self.entries.get(path) {
            return Some(*hit);
        }
        if self.ignored_dirs.is_empty() {
            return None;
        }
        let mut cursor = path.parent();
        while let Some(dir) = cursor {
            if self.ignored_dirs.contains(dir) {
                return Some(TreeGitStatus::ignored());
            }
            cursor = dir.parent();
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn repos(&self) -> &[RepoSummary] {
        &self.repos
    }

    /// 完全一致で登録されている行（パス順にソート済み）。CLI / MCP の出力用
    pub fn sorted_entries(&self) -> Vec<(PathBuf, TreeGitStatus)> {
        let mut out: Vec<(PathBuf, TreeGitStatus)> =
            self.entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// リポジトリ 1 件ぶんの `git status` を取り込む（**純粋関数**。IO はしない）
    pub fn merge_repo(&mut self, repo_root: &Path, status: &GitStatus) {
        let mut changed = 0usize;
        let mut truncated = false;
        for (taken, entry) in status.entries.iter().enumerate() {
            if taken >= MAX_ENTRIES_PER_REPO {
                truncated = true;
                break;
            }
            // リネームの `new\told` は新しい名前を採る（`git::parse_status` が new を
            // 取り出しているが、v1 形式の " -> " 区切りが混ざっても壊れないようにする）
            let rel = entry
                .path
                .rsplit(" -> ")
                .next()
                .unwrap_or(&entry.path)
                .trim();
            if rel.is_empty() {
                continue;
            }
            if entry.is_ignored() {
                let is_dir = rel.ends_with('/');
                let abs = git::from_git_path(repo_root, rel.trim_end_matches('/'));
                if is_dir {
                    self.ignored_dirs.insert(abs.clone());
                }
                // 変更エントリが先に入っていたらそちらを勝たせる
                // （無視パターンに当たっていても追跡済みなら変更が正）
                self.entries
                    .entry(abs)
                    .or_insert_with(TreeGitStatus::ignored);
                continue;
            }
            let Some(status) = TreeGitStatus::from_entry(entry) else {
                continue;
            };
            let abs = git::from_git_path(repo_root, rel);
            changed += 1;
            self.entries.insert(abs.clone(), status);
            // 祖先へ伝播（リポジトリルートまで。ルート自身にも付ける）
            let mut cursor = abs.parent();
            while let Some(dir) = cursor {
                self.entries
                    .entry(dir.to_path_buf())
                    .and_modify(|e| {
                        if e.from_children {
                            e.absorb_child(&status);
                        }
                    })
                    .or_insert_with(|| TreeGitStatus::propagated(&status));
                if dir == repo_root {
                    break;
                }
                cursor = dir.parent();
            }
        }
        self.repos.push(RepoSummary {
            root: repo_root.to_path_buf(),
            branch: status.branch.clone(),
            changed,
            truncated,
        });
    }
}

/// ルートディレクトリ群の git ステータスを取る（background executor 向け。IO する）。
///
/// **`roots` にはワークスペースフォルダだけを渡すこと**（展開済みディレクトリを全部渡すと
/// `git rev-parse --show-toplevel` がその数だけ起動する = #1009 で潰した常駐コスト）。
/// 同じリポジトリに属するルートは 1 回の `git status` にまとめる
pub fn scan(roots: &[PathBuf]) -> TreeGitMap {
    let mut map = TreeGitMap::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let Some(repo_root) = git::repo_root(root) else {
            // git 管理外のフォルダ: 何も出さない（誤検知しない）
            continue;
        };
        if !visited.insert(repo_root.clone()) {
            continue;
        }
        let status = git::status_tree(&repo_root);
        map.merge_repo(&repo_root, &status);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, index: char, worktree: char) -> GitStatusEntry {
        GitStatusEntry {
            path: path.to_string(),
            index,
            worktree,
        }
    }

    fn build(entries: Vec<GitStatusEntry>) -> TreeGitMap {
        let mut map = TreeGitMap::default();
        map.merge_repo(
            Path::new("/repo"),
            &GitStatus {
                branch: "main".into(),
                upstream: String::new(),
                entries,
            },
        );
        map
    }

    #[test]
    fn ステージ済みと未ステージを畳まずに2桁で表す() {
        let map = build(vec![
            entry("staged.rs", 'M', '.'),
            entry("unstaged.rs", '.', 'M'),
            entry("both.rs", 'M', 'M'),
        ]);
        let staged = map.get(Path::new("/repo/staged.rs")).unwrap();
        assert_eq!((staged.staged, staged.unstaged), (Some('M'), None));
        assert_eq!(staged.badge(), "M");

        let unstaged = map.get(Path::new("/repo/unstaged.rs")).unwrap();
        assert_eq!((unstaged.staged, unstaged.unstaged), (None, Some('M')));
        assert_eq!(unstaged.badge(), "M");

        let both = map.get(Path::new("/repo/both.rs")).unwrap();
        assert_eq!((both.staged, both.unstaged), (Some('M'), Some('M')));
        assert_eq!(both.badge(), "MM");
    }

    #[test]
    fn 新規_削除_リネーム_未追跡_コンフリクトを区別する() {
        let map = build(vec![
            entry("added.rs", 'A', '.'),
            entry("deleted.rs", '.', 'D'),
            entry("renamed.rs", 'R', '.'),
            entry("new.rs", '?', '?'),
            entry("conflict.rs", 'u', 'u'),
        ]);
        assert_eq!(
            map.get(Path::new("/repo/added.rs")).unwrap().state,
            TreeGitState::Added
        );
        assert_eq!(
            map.get(Path::new("/repo/deleted.rs")).unwrap().state,
            TreeGitState::Deleted
        );
        assert_eq!(
            map.get(Path::new("/repo/renamed.rs")).unwrap().state,
            TreeGitState::Renamed
        );
        let untracked = map.get(Path::new("/repo/new.rs")).unwrap();
        assert_eq!(untracked.state, TreeGitState::Untracked);
        assert_eq!(untracked.badge(), "U");
        let conflict = map.get(Path::new("/repo/conflict.rs")).unwrap();
        assert_eq!(conflict.state, TreeGitState::Conflicted);
        assert_eq!(conflict.badge(), "!");
    }

    #[test]
    fn ディレクトリへ配下の変更が伝播する() {
        let map = build(vec![
            entry("src/a/deep.rs", '.', 'M'),
            entry("src/b.rs", '?', '?'),
        ]);
        // 折りたたまれていても親ディレクトリに色が付く
        let deep_dir = map.get(Path::new("/repo/src/a")).unwrap();
        assert!(deep_dir.from_children);
        assert_eq!(deep_dir.changed, 1);
        assert_eq!(deep_dir.state, TreeGitState::Modified);

        let src = map.get(Path::new("/repo/src")).unwrap();
        assert!(src.from_children);
        assert_eq!(src.changed, 2);
        // 変更 > 未追跡 なので src は変更色
        assert_eq!(src.state, TreeGitState::Modified);
        assert_eq!(src.badge(), "2");

        // リポジトリルート自身にも伝播する（ワークスペースのルート行に出す）
        assert_eq!(map.get(Path::new("/repo")).unwrap().changed, 2);
    }

    #[test]
    fn 伝播はいちばん強い状態を採る() {
        let map = build(vec![
            entry("src/x.rs", '.', 'M'),
            entry("src/y.rs", 'u', 'u'),
            entry("src/z.rs", '?', '?'),
        ]);
        let src = map.get(Path::new("/repo/src")).unwrap();
        assert_eq!(src.state, TreeGitState::Conflicted);
        assert_eq!(src.changed, 3);
    }

    #[test]
    fn ファイル自身の状態は伝播で上書きされない() {
        // ディレクトリと同名のファイルは無いが、「親に伝播した後に
        // その親自身の変更が来る」順序でもファイル側が勝つことを固定する
        let map = build(vec![
            entry("src/a/deep.rs", '.', 'M'),
            entry("src/a", '.', 'D'), // submodule のようにディレクトリ名で来る場合
        ]);
        let a = map.get(Path::new("/repo/src/a")).unwrap();
        assert!(!a.from_children);
        assert_eq!(a.state, TreeGitState::Deleted);
    }

    #[test]
    fn 無視されたディレクトリは配下まで薄くなる() {
        let map = build(vec![
            entry("target/", '!', '!'),
            entry(".envrc", '!', '!'),
            entry("src/main.rs", '.', 'M'),
        ]);
        assert_eq!(
            map.get(Path::new("/repo/target")).unwrap().state,
            TreeGitState::Ignored
        );
        // 配下は 1 行も返ってこないが祖先照合で無視になる
        assert_eq!(
            map.get(Path::new("/repo/target/debug/tako")).unwrap().state,
            TreeGitState::Ignored
        );
        assert_eq!(
            map.get(Path::new("/repo/.envrc")).unwrap().state,
            TreeGitState::Ignored
        );
        // 無視は変更として数えない
        assert_eq!(map.repos()[0].changed, 1);
        // 無視のバッジは出さない
        assert_eq!(map.get(Path::new("/repo/target")).unwrap().badge(), "");
    }

    #[test]
    fn 無視パターンに当たっていても追跡済みの変更が勝つ() {
        // `!` が先に来ても後に来ても、変更エントリが優先される
        let map = build(vec![
            entry("keep.log", '!', '!'),
            entry("keep.log", '.', 'M'),
        ]);
        assert_eq!(
            map.get(Path::new("/repo/keep.log")).unwrap().state,
            TreeGitState::Modified
        );
        let map = build(vec![
            entry("keep.log", '.', 'M'),
            entry("keep.log", '!', '!'),
        ]);
        assert_eq!(
            map.get(Path::new("/repo/keep.log")).unwrap().state,
            TreeGitState::Modified
        );
    }

    #[test]
    fn 変更の無いパスは何も返さない() {
        let map = build(vec![entry("src/main.rs", '.', 'M')]);
        assert!(map.get(Path::new("/repo/src/other.rs")).is_none());
        assert!(map.get(Path::new("/elsewhere/x.rs")).is_none());
    }

    #[test]
    fn 空のマップは祖先照合をしても何も返さない() {
        let map = TreeGitMap::default();
        assert!(map.get(Path::new("/repo/src/main.rs")).is_none());
        assert!(map.is_empty());
    }

    #[test]
    fn 伝播はリポジトリルートより上には広がらない() {
        let map = build(vec![entry("src/main.rs", '.', 'M')]);
        assert!(map.get(Path::new("/")).is_none());
    }

    #[test]
    fn ディレクトリのバッジは件数で99超は省略する() {
        let entries: Vec<GitStatusEntry> = (0..120)
            .map(|i| entry(&format!("src/f{i}.rs"), '.', 'M'))
            .collect();
        let map = build(entries);
        assert_eq!(map.get(Path::new("/repo/src")).unwrap().badge(), "99+");
    }

    #[test]
    fn 深刻度の順序は色の優先順位と一致する() {
        let mut states = [
            TreeGitState::Modified,
            TreeGitState::Ignored,
            TreeGitState::Conflicted,
            TreeGitState::Untracked,
            TreeGitState::Deleted,
            TreeGitState::Added,
            TreeGitState::Renamed,
        ];
        states.sort_by_key(|s| s.severity());
        assert_eq!(
            states,
            [
                TreeGitState::Ignored,
                TreeGitState::Untracked,
                TreeGitState::Added,
                TreeGitState::Modified,
                TreeGitState::Renamed,
                TreeGitState::Deleted,
                TreeGitState::Conflicted,
            ]
        );
    }
}

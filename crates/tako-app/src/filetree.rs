//! filetree — 左サイドバーのファイルツリー（FR-3.1 / FR-3.7）
//!
//! フォーカス中ペインの cwd（OSC 7 検知）をルートに表示する VSCode 風ツリー。
//! 状態・読み込み・フラット化は GPUI 非依存（描画は main.rs 側）。
//! 内容の更新はポーリング（表示中のみ。notify クレートは必要になったら再判断 =
//! `architecture.md`「コンセプト②の実現」）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 1 ディレクトリの最大表示エントリ数（巨大ディレクトリの暴走防止）
const MAX_ENTRIES: usize = 500;
/// 展開を辿る最大深さ（シンボリックリンクループ等の暴走防止）
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// 表示用にフラット化した 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
}

/// ファイルツリーの状態。`visible` は FR-3.7（折りたたみで純粋なターミナルに戻る）
#[derive(Default)]
pub struct FileTree {
    pub visible: bool,
    root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
}

impl FileTree {
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// cwd 連動（FR-3.1）。root が変わったら展開状態とキャッシュを畳んで読み直す。
    /// 変化があれば true（再描画判断用）
    pub fn set_root(&mut self, root: Option<PathBuf>) -> bool {
        if self.root == root {
            return false;
        }
        self.root = root;
        self.expanded.clear();
        self.cache.clear();
        if let Some(root) = self.root.clone() {
            self.cache.insert(root.clone(), read_dir_sorted(&root));
        }
        true
    }

    /// ディレクトリ行のクリック: 展開 ⇄ 折りたたみ
    pub fn toggle_dir(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_path_buf());
            self.cache
                .entry(path.to_path_buf())
                .or_insert_with(|| read_dir_sorted(path));
        }
    }

    /// 表示行（root 直下から展開状態に従った深さ優先）
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(root) = &self.root {
            self.collect_rows(root, 0, &mut rows);
        }
        rows
    }

    fn collect_rows(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(entries) = self.cache.get(dir) else {
            return;
        };
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            rows.push(Row {
                entry: entry.clone(),
                depth,
                expanded,
            });
            if expanded {
                self.collect_rows(&entry.path, depth + 1, rows);
            }
        }
    }

    /// root + 展開中ディレクトリの内容を読み直す（ポーリング更新）。
    /// 変化があれば true。消えたディレクトリはキャッシュ・展開状態から落とす
    pub fn refresh(&mut self) -> bool {
        let mut targets: Vec<PathBuf> = self.root.iter().cloned().collect();
        targets.extend(self.expanded.iter().cloned());
        let mut changed = false;
        for dir in targets {
            if !dir.is_dir() {
                changed |= self.cache.remove(&dir).is_some();
                changed |= self.expanded.remove(&dir);
                continue;
            }
            let fresh = read_dir_sorted(&dir);
            if self.cache.get(&dir) != Some(&fresh) {
                self.cache.insert(dir, fresh);
                changed = true;
            }
        }
        changed
    }
}

/// ディレクトリを読んで「ディレクトリ先・名前（大文字小文字無視）順」に並べる。
/// 読めない場合は空（権限・消滅は正常系として無害に劣化）
fn read_dir_sorted(path: &Path) -> Vec<Entry> {
    let Ok(reader) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = reader
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().ok()?.is_dir();
            Some(Entry {
                path: e.path(),
                name,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries.truncate(MAX_ENTRIES);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-filetree-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        std::fs::write(dir.join("src/main.rs"), "x").unwrap();
        dir
    }

    #[test]
    fn ルート直下はディレクトリ先の名前順で並ぶ() {
        let dir = fixture("t1");
        let mut tree = FileTree::default();
        assert!(tree.set_root(Some(dir.clone())));
        // 同じ root の再設定は変化なし
        assert!(!tree.set_root(Some(dir.clone())));
        let names: Vec<(String, usize)> = tree
            .rows()
            .iter()
            .map(|r| (r.entry.name.clone(), r.depth))
            .collect();
        assert_eq!(
            names,
            vec![
                ("docs".to_string(), 0),
                ("src".to_string(), 0),
                ("README.md".to_string(), 0),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 展開で子が出て折りたたみで消える() {
        let dir = fixture("t2");
        let mut tree = FileTree::default();
        tree.set_root(Some(dir.clone()));
        tree.toggle_dir(&dir.join("src"));
        let rows = tree.rows();
        let main_rs = rows.iter().find(|r| r.entry.name == "main.rs").unwrap();
        assert_eq!(main_rs.depth, 1);
        assert!(rows.iter().any(|r| r.entry.name == "src" && r.expanded));
        tree.toggle_dir(&dir.join("src"));
        assert!(!tree.rows().iter().any(|r| r.entry.name == "main.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refreshは追加と消滅を拾う() {
        let dir = fixture("t3");
        let mut tree = FileTree::default();
        tree.set_root(Some(dir.clone()));
        assert!(!tree.refresh(), "変化が無ければ false");
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert!(tree.refresh());
        assert!(tree.rows().iter().any(|r| r.entry.name == "new.txt"));
        // 展開中ディレクトリの消滅 → 展開状態ごと畳まれる
        tree.toggle_dir(&dir.join("docs"));
        std::fs::remove_dir_all(dir.join("docs")).unwrap();
        assert!(tree.refresh());
        assert!(!tree
            .rows()
            .iter()
            .any(|r| r.entry.name == "docs" && r.expanded));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root変更で展開状態が畳まれ読めないパスは空になる() {
        let dir = fixture("t4");
        let mut tree = FileTree::default();
        tree.set_root(Some(dir.clone()));
        tree.toggle_dir(&dir.join("src"));
        tree.set_root(Some(dir.join("docs")));
        assert!(tree.rows().is_empty(), "docs は空ディレクトリ");
        tree.set_root(Some(PathBuf::from("/no/such/dir")));
        assert!(tree.rows().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

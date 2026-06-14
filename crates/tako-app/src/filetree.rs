//! filetree — 左サイドバーのファイルツリー（FR-3.1 / FR-3.7）
//!
//! 「タブ = ワークスペース」: アクティブタブ内の**全ペインの cwd**（OSC 7 検知）を
//! ワークスペースフォルダとして並べるマルチルートツリー（VSCode の
//! 「フォルダをワークスペースに追加」相当。2026-06-13 にフォーカスペイン連動から変更）。
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

/// 表示用にフラット化した 1 行。`root` = ワークスペースフォルダの見出し行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
    pub root: bool,
}

/// ファイルツリーの状態。`visible` は FR-3.7（折りたたみで純粋なターミナルに戻る）
#[derive(Default)]
pub struct FileTree {
    pub visible: bool,
    roots: Vec<PathBuf>,
    /// 展開中ディレクトリ（ルート自身も含む。絶対パスがキーなのでルート間で共有できる）
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
}

impl FileTree {
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// ワークスペースフォルダ列の同期（FR-3.1。呼び出し側がタブ内ペインの cwd を集める）。
    /// 重複は除き、既存ルートの展開状態は維持する。変化があれば true（再描画判断用）
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) -> bool {
        let mut deduped: Vec<PathBuf> = Vec::new();
        for root in roots {
            if !deduped.contains(&root) {
                deduped.push(root);
            }
        }
        if self.roots == deduped {
            return false;
        }
        // 消えたルートの状態は畳む（配下の展開・キャッシュは refresh が掃除する）
        for old in &self.roots {
            if !deduped.contains(old) {
                self.expanded.remove(old);
                self.cache.remove(old);
            }
        }
        for root in &deduped {
            // 新規ルートだけ展開済みで読み込む（VSCode のワークスペースフォルダ同様。
            // 既存ルートはユーザーが畳んだ状態を尊重する）
            if !self.roots.contains(root) {
                self.expanded.insert(root.clone());
                self.cache
                    .entry(root.clone())
                    .or_insert_with(|| read_dir_sorted(root));
            }
        }
        self.roots = deduped;
        true
    }

    /// ディレクトリを展開する（既に展開中なら何もしない）
    pub fn expand_dir(&mut self, path: &Path) {
        if !self.expanded.contains(path) {
            self.expanded.insert(path.to_path_buf());
            self.cache
                .entry(path.to_path_buf())
                .or_insert_with(|| read_dir_sorted(path));
        }
    }

    /// ディレクトリ行（ルート見出し行を含む）のクリック: 展開 ⇄ 折りたたみ
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

    /// 表示行: ルート見出し行 + 展開状態に従った深さ優先の中身
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for root in &self.roots {
            let expanded = self.expanded.contains(root);
            rows.push(Row {
                entry: Entry {
                    path: root.clone(),
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string()),
                    is_dir: true,
                },
                depth: 0,
                expanded,
                root: true,
            });
            if expanded {
                self.collect_rows(root, 1, &mut rows);
            }
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
                root: false,
            });
            if expanded {
                self.collect_rows(&entry.path, depth + 1, rows);
            }
        }
    }

    /// 同期 refresh（テスト用。本番は refresh_targets → scan_dirs → apply_refresh）
    #[cfg(test)]
    pub fn refresh(&mut self) -> bool {
        let results = scan_dirs(&self.refresh_targets());
        self.apply_refresh(results)
    }

    /// background executor 向け: スキャン対象のディレクトリ一覧を返す
    pub fn refresh_targets(&self) -> Vec<PathBuf> {
        let mut targets: Vec<PathBuf> = self.roots.clone();
        targets.extend(self.expanded.iter().cloned());
        targets.dedup();
        targets
    }

    /// background executor の結果を適用する。変化があれば true
    pub fn apply_refresh(&mut self, results: Vec<(PathBuf, Option<Vec<Entry>>)>) -> bool {
        let mut changed = false;
        for (dir, entries) in results {
            if let Some(fresh) = entries {
                if self.cache.get(&dir) != Some(&fresh) {
                    self.cache.insert(dir, fresh);
                    changed = true;
                }
            } else {
                changed |= self.cache.remove(&dir).is_some();
                changed |= self.expanded.remove(&dir);
            }
        }
        changed
    }
}

/// ディレクトリ列をスキャンする（background executor で呼べる純粋 I/O）。
/// 存在しないディレクトリは None を返す
pub fn scan_dirs(targets: &[PathBuf]) -> Vec<(PathBuf, Option<Vec<Entry>>)> {
    targets
        .iter()
        .map(|dir| {
            if dir.is_dir() {
                (dir.clone(), Some(read_dir_sorted(dir)))
            } else {
                (dir.clone(), None)
            }
        })
        .collect()
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

    /// (name, depth, root) のタプル列に写す（検証用）
    fn names(tree: &FileTree) -> Vec<(String, usize, bool)> {
        tree.rows()
            .iter()
            .map(|r| (r.entry.name.clone(), r.depth, r.root))
            .collect()
    }

    #[test]
    fn ルート見出しの下にディレクトリ先の名前順で並ぶ() {
        let dir = fixture("t1");
        let mut tree = FileTree::default();
        assert!(tree.set_roots(vec![dir.clone()]));
        // 同じルート列の再設定は変化なし
        assert!(!tree.set_roots(vec![dir.clone()]));
        let root_name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            names(&tree),
            vec![
                (root_name, 0, true),
                ("docs".to_string(), 1, false),
                ("src".to_string(), 1, false),
                ("README.md".to_string(), 1, false),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 複数ルートが順序と重複除去つきで並ぶ() {
        let dir = fixture("t2");
        let mut tree = FileTree::default();
        assert!(tree.set_roots(vec![
            dir.join("src"),
            dir.join("docs"),
            dir.join("src"), // 重複は除かれる
        ]));
        assert_eq!(tree.roots(), &[dir.join("src"), dir.join("docs")]);
        let rows = names(&tree);
        let roots: Vec<_> = rows.iter().filter(|(_, _, root)| *root).collect();
        assert_eq!(roots.len(), 2);
        assert!(rows.contains(&("main.rs".to_string(), 1, false)));
        // ルート見出しの折りたたみで中身が消える
        tree.toggle_dir(&dir.join("src"));
        assert!(!names(&tree).contains(&("main.rs".to_string(), 1, false)));
        // ルートが減っても残りの展開状態は維持される
        assert!(tree.set_roots(vec![dir.join("docs")]));
        assert_eq!(names(&tree).len(), 1, "docs は空ディレクトリ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 展開で子が出て折りたたみで消える() {
        let dir = fixture("t3");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        tree.toggle_dir(&dir.join("src"));
        let rows = tree.rows();
        let main_rs = rows.iter().find(|r| r.entry.name == "main.rs").unwrap();
        assert_eq!(main_rs.depth, 2);
        assert!(rows.iter().any(|r| r.entry.name == "src" && r.expanded));
        tree.toggle_dir(&dir.join("src"));
        assert!(!tree.rows().iter().any(|r| r.entry.name == "main.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refreshは追加と消滅を拾う() {
        let dir = fixture("t4");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
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

    /// 性能計測（通常テストでは走らせない）: `cargo test -p tako-app --release -- --ignored --nocapture perf_`
    #[test]
    #[ignore]
    fn perf_ツリー計測() {
        use std::time::Instant;
        // 合成: 5000 ファイルの大ディレクトリ（node_modules / target 相当）
        let big = std::env::temp_dir().join(format!("tako-filetree-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&big);
        std::fs::create_dir_all(&big).unwrap();
        for i in 0..5000 {
            std::fs::write(big.join(format!("file-{i:05}.txt")), "x").unwrap();
        }

        let t0 = Instant::now();
        let entries = read_dir_sorted(&big);
        eprintln!(
            "[perf] read_dir_sorted 5000 エントリ: {:?}（{} 行に切り詰め）",
            t0.elapsed(),
            entries.len()
        );

        // 実リポジトリ相当: tako リポジトリルートを root に、複数ディレクトリ展開
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut tree = FileTree::default();
        tree.set_roots(vec![repo.clone(), big.clone()]);
        for sub in ["crates", ".agent", "scripts", "poc"] {
            tree.toggle_dir(&repo.join(sub));
        }
        let t1 = Instant::now();
        let changed = tree.refresh();
        eprintln!(
            "[perf] refresh（root 2 + 展開 4）: {:?} changed={}",
            t1.elapsed(),
            changed
        );
        let t2 = Instant::now();
        let rows = tree.rows();
        eprintln!("[perf] rows(): {:?}（{} 行）", t2.elapsed(), rows.len());
        let _ = std::fs::remove_dir_all(&big);
    }

    #[test]
    fn 読めないルートは見出しだけ残り中身は空() {
        let mut tree = FileTree::default();
        tree.set_roots(vec![PathBuf::from("/no/such/dir")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].root);
    }
}

//! テスト用の偽のファイルシステム（実ディスクを触らずに、どちらの OS の置き場でも組める）

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::FsProbe;

#[derive(Debug, Default)]
pub struct FakeFs {
    files: BTreeMap<PathBuf, String>,
    dirs: BTreeSet<PathBuf>,
}

impl FakeFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// ファイルを置く（親ディレクトリも全部できる）
    pub fn file(mut self, path: impl AsRef<Path>, content: &str) -> Self {
        let path = path.as_ref().to_path_buf();
        self.add_ancestors(&path);
        self.files.insert(path, content.to_string());
        self
    }

    /// 空のディレクトリを置く
    pub fn dir(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        self.add_ancestors(&path);
        self.dirs.insert(path);
        self
    }

    fn add_ancestors(&mut self, path: &Path) {
        let mut cur = path.parent();
        while let Some(p) = cur {
            if p.as_os_str().is_empty() {
                break;
            }
            self.dirs.insert(p.to_path_buf());
            cur = p.parent();
        }
    }
}

impl FsProbe for FakeFs {
    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(path)
    }

    fn read_head(&self, path: &Path, max: usize) -> Option<String> {
        let text = self.files.get(path)?;
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(text[..end].to_string())
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let children = self.files.keys().chain(self.dirs.iter());
        let mut out: Vec<String> = children
            .filter(|p| p.parent() == Some(path))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

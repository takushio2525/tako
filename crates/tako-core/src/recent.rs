//! 最近開いたディレクトリ/SSH ホストの記録と永続化。
//! `<data_dir>/recent.json` に保存する。

use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_ENTRIES: usize = 20;
const FILENAME: &str = "recent.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum RecentEntry {
    #[serde(rename = "directory")]
    Directory { path: String },
    #[serde(rename = "repository")]
    Repository { path: String },
    #[serde(rename = "ssh")]
    Ssh { host: String },
}

impl RecentEntry {
    pub fn label(&self) -> &str {
        match self {
            RecentEntry::Directory { path } => path,
            RecentEntry::Repository { path } => path,
            RecentEntry::Ssh { host } => host,
        }
    }

    fn key(&self) -> (&str, &str) {
        match self {
            RecentEntry::Directory { path } => ("directory", path),
            RecentEntry::Repository { path } => ("repository", path),
            RecentEntry::Ssh { host } => ("ssh", host),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecentList {
    pub entries: Vec<RecentEntry>,
}

impl RecentList {
    pub fn load() -> Self {
        let path = match recent_path() {
            Some(p) => p,
            None => return Self::default(),
        };
        Self::load_from(&path)
    }

    /// 指定パスから読む。**解釈できない内容は既定値へ落とす前に退避する**（#916）。
    /// 退避しておかないと、直後の [`save`](Self::save) が元の内容を上書きして消す
    pub fn load_from(path: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(list) => list,
            Err(_) => {
                let _ = crate::migration::quarantine_unreadable(path, &crate::migration::FsIo);
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let path = match recent_path() {
            Some(p) => p,
            None => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// エントリを先頭に追加（既存は移動）。MAX_ENTRIES を超えたら古いものを削除
    pub fn push(&mut self, entry: RecentEntry) {
        let key = entry.key();
        self.entries.retain(|e| e.key() != key);
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn recent_path() -> Option<PathBuf> {
    paths::data_dir().map(|d| d.join(FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #916: 壊れた recent.json も既定値へ落ちる前に退避される
    #[test]
    fn 壊れた最近使った場所は退避されてから空になる() {
        let dir = std::env::temp_dir().join(format!("tako-recent-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        let path = dir.join("recent.json");
        std::fs::write(&path, "[こわれた").expect("書ける");
        let list = RecentList::load_from(&path);
        assert!(list.entries.is_empty());
        assert_eq!(
            std::fs::read_to_string(crate::migration::quarantine_path(&path)).expect("読める"),
            "[こわれた"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_deduplicates() {
        let mut list = RecentList::default();
        list.push(RecentEntry::Directory {
            path: "/a".to_string(),
        });
        list.push(RecentEntry::Directory {
            path: "/b".to_string(),
        });
        list.push(RecentEntry::Directory {
            path: "/a".to_string(),
        });
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].label(), "/a");
        assert_eq!(list.entries[1].label(), "/b");
    }

    #[test]
    fn truncates_at_max() {
        let mut list = RecentList::default();
        for i in 0..25 {
            list.push(RecentEntry::Directory {
                path: format!("/dir{i}"),
            });
        }
        assert_eq!(list.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn clear_empties() {
        let mut list = RecentList::default();
        list.push(RecentEntry::Ssh {
            host: "myhost".to_string(),
        });
        list.clear();
        assert!(list.entries.is_empty());
    }

    #[test]
    fn different_types_coexist() {
        let mut list = RecentList::default();
        list.push(RecentEntry::Directory {
            path: "/a".to_string(),
        });
        list.push(RecentEntry::Repository {
            path: "/a".to_string(),
        });
        list.push(RecentEntry::Ssh {
            host: "server".to_string(),
        });
        assert_eq!(list.entries.len(), 3);
    }

    #[test]
    fn serialization_roundtrip() {
        let mut list = RecentList::default();
        list.push(RecentEntry::Directory {
            path: "/test".to_string(),
        });
        list.push(RecentEntry::Ssh {
            host: "myhost".to_string(),
        });
        let json = serde_json::to_string(&list).unwrap();
        let parsed: RecentList = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.entries.len(), 2);
    }
}

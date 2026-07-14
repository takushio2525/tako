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
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
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

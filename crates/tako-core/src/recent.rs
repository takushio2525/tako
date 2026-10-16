//! 最近開いたディレクトリ/SSH ホストの記録と永続化。
//! `<data_dir>/recent.json` に保存する。

use crate::load_issue::LoadIssue;
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
    /// 指定パスから読む。**解釈できない内容は既定値へ落とす前に退避する**（#916）。
    /// 退避しておかないと、直後の [`save`](Self::save) が元の内容を上書きして消す。
    ///
    /// 既定の置き場から読むのは `tako_control::data_files::load_recent`（落とした理由を
    /// persist.log へ残す口。#1824）。ここを直接呼ぶと記録が残らない
    pub fn load_from(path: &std::path::Path) -> Self {
        Self::load_from_checked(path).0
    }

    /// [`load_from`](Self::load_from) の本体。既定値へ落としたなら理由も返す（#1824）
    pub fn load_from_checked(path: &std::path::Path) -> (Self, Option<LoadIssue>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => return (Self::default(), LoadIssue::from_read_error(&e)),
        };
        match serde_json::from_str(&text) {
            Ok(list) => (list, None),
            Err(e) => {
                let quarantine =
                    crate::migration::quarantine_unreadable(path, &crate::migration::FsIo);
                let issue = LoadIssue::Unparsable {
                    reason: crate::load_issue::json_reason(&e),
                    quarantine,
                };
                (Self::default(), Some(issue))
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

/// 既定の置き場（`<data_dir>/recent.json`）
pub fn recent_path() -> Option<PathBuf> {
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

    /// #1824: 既定値へ落とした理由を返す（不在と正常は理由にしない）
    #[test]
    fn 落とした理由を返す() {
        let dir = std::env::temp_dir().join(format!("tako-recent-issue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        let path = dir.join("recent.json");
        assert!(path.starts_with(std::env::temp_dir()), "一時 dir の中だけ");

        assert_eq!(RecentList::load_from_checked(&path).1, None, "不在");
        for (body, slot) in [("", 1), (r#"{"entries": [{"type": "dire"#, 2)] {
            std::fs::write(&path, body).expect("書ける");
            let (list, issue) = RecentList::load_from_checked(&path);
            assert!(list.entries.is_empty());
            match issue {
                Some(LoadIssue::Unparsable {
                    quarantine: Some(q),
                    ..
                }) => {
                    assert!(q.copied, "{body:?} は初めて見る中身");
                    assert_eq!(q.path, crate::migration::quarantine_slot_path(&path, slot));
                }
                other => panic!("{body:?} の理由が違う: {other:?}"),
            }
        }
        std::fs::write(&path, r#"{"entries": []}"#).expect("書ける");
        assert_eq!(RecentList::load_from_checked(&path).1, None, "正常");
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

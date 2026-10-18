//! 最近開いたディレクトリ/SSH ホストの記録と永続化。
//! `<data_dir>/recent.json` に保存する。

use crate::load_issue::LoadIssue;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
            Err(e) => return (Self::default(), LoadIssue::read_failed(path, &e)),
        };
        match serde_json::from_str(&text) {
            Ok(list) => (list, None),
            Err(e) => (Self::default(), Some(LoadIssue::unparsable(path, &e))),
        }
    }

    pub fn save(&self) {
        if let Some(path) = recent_path() {
            self.save_to(&path);
        }
    }

    /// 指定パスへ書く（[`save`](Self::save) の本体。テストは一時 dir のパスを渡す）
    pub fn save_to(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// ファイル・フォルダの移動（#1842）で場所が変わったフォルダ・リポジトリの項目を
    /// 付け替える。`moved` は記録されたパスを受け、移したもの（またはその配下）なら移した後の
    /// パスを返す（`file_move::moved_path`）。付け替えた件数を返す。
    ///
    /// 並び（新しい順）は変えない。付け替えで同じ項目が 2 つになったら前（新しい方）を残す。
    /// SSH ホストは対象外
    pub fn retarget(&mut self, mut moved: impl FnMut(&Path) -> Option<PathBuf>) -> usize {
        let mut count = 0;
        for entry in &mut self.entries {
            let path = match entry {
                RecentEntry::Directory { path } | RecentEntry::Repository { path } => path,
                RecentEntry::Ssh { .. } => continue,
            };
            if let Some(to) = moved(Path::new(path.as_str())) {
                *path = to.to_string_lossy().into_owned();
                count += 1;
            }
        }
        if count > 0 {
            let mut seen = std::collections::HashSet::new();
            self.entries.retain(|e| {
                let (kind, label) = e.key();
                seen.insert((kind.to_string(), label.to_string()))
            });
        }
        count
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

    /// #1842: 移したフォルダとその配下の項目だけが付け替わり、並びは変わらない。
    /// SSH ホストと、名前が前方一致するだけの別のフォルダは触らない
    #[test]
    fn 移したフォルダと配下の項目だけを並びを保って付け替える() {
        let dir = |p: &str| RecentEntry::Directory { path: p.into() };
        let mut list = RecentList {
            entries: vec![
                dir("/w/proj/sub"),
                RecentEntry::Ssh {
                    host: "/w/proj".into(),
                },
                RecentEntry::Repository {
                    path: "/w/proj".into(),
                },
                dir("/w/project"),
                dir("/w/other"),
            ],
        };
        let moved =
            |p: &Path| crate::file_move::remap(p, Path::new("/w/proj"), Path::new("/x/proj"));
        assert_eq!(list.retarget(moved), 2);
        // 配下は `join` で組む（Windows では区切りが `\` になる）
        let moved_sub = Path::new("/x/proj").join("sub");
        assert_eq!(
            list.entries,
            vec![
                dir(&moved_sub.to_string_lossy()),
                RecentEntry::Ssh {
                    host: "/w/proj".into()
                },
                RecentEntry::Repository {
                    path: "/x/proj".into()
                },
                dir("/w/project"),
                dir("/w/other"),
            ]
        );
        // 当たらなければ何も変えない
        let before = list.entries.clone();
        assert_eq!(list.retarget(|_| None), 0);
        assert_eq!(list.entries, before);
    }

    /// 付け替え先に古い同じ項目があれば、新しい方（前）を残して重複させない
    #[test]
    fn 付け替えで重なった項目は新しい方を残す() {
        let dir = |p: &str| RecentEntry::Directory { path: p.into() };
        let mut list = RecentList {
            entries: vec![dir("/w/a"), dir("/w/b"), dir("/x/a")],
        };
        let moved = |p: &Path| crate::file_move::remap(p, Path::new("/w/a"), Path::new("/x/a"));
        assert_eq!(list.retarget(moved), 1);
        assert_eq!(list.entries, vec![dir("/x/a"), dir("/w/b")]);
    }

    #[test]
    fn 指定パスへ書いて読み戻せる() {
        let dir = std::env::temp_dir().join(format!("tako-recent-save-to-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("recent.json");
        assert!(path.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
        let mut list = RecentList::default();
        list.push(RecentEntry::Directory {
            path: "/w/a".into(),
        });
        list.save_to(&path);
        assert_eq!(RecentList::load_from(&path).entries, list.entries);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

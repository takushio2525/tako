//! Tab — エージェントグループの単位（1 グループ = 1 タブ、`.agent/concept.md`）

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pane::Pane;
use crate::pane_tree::PaneTree;

/// プロセス生存期間中ユニークなタブ ID（`TAKO_TAB_ID` として外部公開される）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        TabId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// タブ。常に 1 つ以上のペインを持つ PaneTree を内包する
#[derive(Debug)]
pub struct Tab {
    id: TabId,
    title: String,
    tree: PaneTree,
}

impl Tab {
    pub fn new(title: impl Into<String>, root_pane: Pane) -> Self {
        Self {
            id: TabId::next(),
            title: title.into(),
            tree: PaneTree::new(root_pane),
        }
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn tree(&self) -> &PaneTree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut PaneTree {
        &mut self.tree
    }
}

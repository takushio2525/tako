//! Tab — エージェントグループの単位（1 グループ = 1 タブ、`.agent/concept.md`）

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pane::{Pane, TitleSource};
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
    /// `title` の出どころ（FR-2.12.3。Default = 初期連番のまま）
    title_source: TitleSource,
    tree: PaneTree,
}

impl Tab {
    pub fn new(title: impl Into<String>, root_pane: Pane) -> Self {
        Self {
            id: TabId::next(),
            title: title.into(),
            title_source: TitleSource::Default,
            tree: PaneTree::new(root_pane),
        }
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn title_source(&self) -> TitleSource {
        self.title_source
    }

    /// 初期タイトルの差し替え（出どころは変えない。UI の連番付け直し等）
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// 明示リネーム（`tako tab rename` / MCP / UI。FR-2.12.3 で自動より優先される）
    pub fn set_title_manual(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.title_source = TitleSource::Manual;
    }

    /// 手動リネームの解除。タイトルは保持しつつ Default に戻し、自動リネームを再開させる
    pub fn clear_manual_title(&mut self) {
        self.title_source = TitleSource::Default;
    }

    /// 自動リネーム（FR-2.12）。Manual 設定済みなら上書きせず false を返す
    pub fn set_title_auto(&mut self, title: impl Into<String>) -> bool {
        if self.title_source == TitleSource::Manual {
            return false;
        }
        self.title = title.into();
        self.title_source = TitleSource::Auto;
        true
    }

    pub fn tree(&self) -> &PaneTree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut PaneTree {
        &mut self.tree
    }

    /// タブを消費してペインツリーを取り出す（ペインの別タブ移送で使う）
    pub fn into_tree(self) -> PaneTree {
        self.tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneOrigin;

    #[test]
    fn 手動タイトルは自動に上書きされずクリアで再開する() {
        let mut tab = Tab::new("1", Pane::new(PaneOrigin::User));
        assert_eq!(tab.title_source(), TitleSource::Default);
        assert!(tab.set_title_auto("ビルド"));
        assert_eq!(tab.title(), "ビルド");
        assert_eq!(tab.title_source(), TitleSource::Auto);
        tab.set_title_manual("agents");
        assert!(!tab.set_title_auto("別名"));
        assert_eq!(tab.title(), "agents");
        tab.clear_manual_title();
        assert!(tab.set_title_auto("再開"));
        assert_eq!(tab.title(), "再開");
    }
}

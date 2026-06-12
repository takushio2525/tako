//! Workspace — タブ一覧とアクティブタブの管理（FR-1.2）

use crate::pane::{Pane, PaneId};
use crate::pane_tree::{PaneTreeError, SplitDirection};
use crate::tab::{Tab, TabId};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("タブ {0} が見つからない")]
    TabNotFound(TabId),
    #[error("ペイン {0} が見つからない")]
    PaneNotFound(PaneId),
    #[error("最後の 1 タブは閉じられない（アプリ終了は UI 層の責務）")]
    LastTab,
    #[error("ペイン {0} は既にタブ {1} にある")]
    AlreadyInTab(PaneId, TabId),
}

/// アプリ全体の状態のルート。常に 1 つ以上のタブを持つ
#[derive(Debug)]
pub struct Workspace {
    tabs: Vec<Tab>,
    active: TabId,
}

impl Workspace {
    /// 最初のタブ（とそのルートペイン）込みで生成する
    pub fn new(initial_tab_title: impl Into<String>, root_pane: Pane) -> Self {
        let tab = Tab::new(initial_tab_title, root_pane);
        let active = tab.id();
        Self {
            tabs: vec![tab],
            active,
        }
    }

    /// レイアウト復元用（Phase 5.5）。tabs が空なら None（呼び出し側が新規作成へ
    /// フォールバックする）。active が見つからなければ先頭タブをアクティブにする
    pub fn restore(tabs: Vec<Tab>, active: TabId) -> Option<Self> {
        if tabs.is_empty() {
            return None;
        }
        let active = if tabs.iter().any(|t| t.id() == active) {
            active
        } else {
            tabs[0].id()
        };
        Some(Self { tabs, active })
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_tab_id(&self) -> TabId {
        self.active
    }

    pub fn active_tab(&self) -> &Tab {
        // 不変条件: active は常に tabs 内に存在する（論理的に到達不能）
        self.tabs
            .iter()
            .find(|t| t.id() == self.active)
            .expect("active タブは常に存在する")
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let active = self.active;
        self.tabs
            .iter_mut()
            .find(|t| t.id() == active)
            .expect("active タブは常に存在する")
    }

    pub fn get_tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id() == id)
    }

    pub fn get_tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id() == id)
    }

    /// 新しいタブを末尾に作成し、アクティブにする
    pub fn create_tab(&mut self, title: impl Into<String>, root_pane: Pane) -> TabId {
        let tab = Tab::new(title, root_pane);
        let id = tab.id();
        self.tabs.push(tab);
        self.active = id;
        id
    }

    /// タブを閉じる。アクティブタブを閉じた場合は左隣（先頭なら新しい先頭）へ移る
    pub fn close_tab(&mut self, id: TabId) -> Result<Tab, WorkspaceError> {
        let index = self
            .tabs
            .iter()
            .position(|t| t.id() == id)
            .ok_or(WorkspaceError::TabNotFound(id))?;
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::LastTab);
        }
        let tab = self.tabs.remove(index);
        if self.active == id {
            self.active = self.tabs[index.saturating_sub(1)].id();
        }
        Ok(tab)
    }

    pub fn activate_tab(&mut self, id: TabId) -> Result<(), WorkspaceError> {
        if self.get_tab(id).is_none() {
            return Err(WorkspaceError::TabNotFound(id));
        }
        self.active = id;
        Ok(())
    }

    /// 次のタブへ巡回切替
    pub fn activate_next_tab(&mut self) -> TabId {
        self.activate_by_offset(1)
    }

    /// 前のタブへ巡回切替
    pub fn activate_prev_tab(&mut self) -> TabId {
        self.activate_by_offset(self.tabs.len() - 1)
    }

    /// ペインが属するタブを探す（FR-2.2.7 の呼び出し元特定や IPC のペイン解決に使う）
    pub fn find_tab_of_pane(&self, pane: PaneId) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|t| t.tree().contains(pane))
            .map(|t| t.id())
    }

    /// ペインを別タブへ移送する（FR-2.5.10）。移動先ではフォーカス中ペインの右に生える。
    /// 移動元タブが空になる場合はタブごと閉じる
    pub fn move_pane(&mut self, pane: PaneId, dest: TabId) -> Result<(), WorkspaceError> {
        let src = self
            .find_tab_of_pane(pane)
            .ok_or(WorkspaceError::PaneNotFound(pane))?;
        if self.get_tab(dest).is_none() {
            return Err(WorkspaceError::TabNotFound(dest));
        }
        if src == dest {
            return Err(WorkspaceError::AlreadyInTab(pane, dest));
        }
        let src_tree = self
            .get_tab_mut(src)
            .expect("find_tab_of_pane で存在確認済み")
            .tree_mut();
        let moved = match src_tree.close(pane) {
            Ok(pane) => pane,
            Err(PaneTreeError::LastPane) => {
                // 1 ペインだけのタブ → タブごと閉じてペインを取り出す
                // （dest が別タブとして存在するので LastTab にはならない）
                let tab = self.close_tab(src).expect("dest タブが他に存在する");
                let mut panes = tab.into_tree().into_panes();
                panes.pop().expect("タブは常に 1 ペイン以上を持つ")
            }
            // contains 確認済みのため PaneNotFound は論理的に到達不能
            Err(e) => unreachable!("move_pane の close で想定外のエラー: {e}"),
        };
        let dest_tab = self.get_tab_mut(dest).expect("存在確認済み");
        let focused = dest_tab.tree().focused();
        dest_tab
            .tree_mut()
            .split(focused, SplitDirection::Right, moved)
            .expect("focused ペインは常に存在する");
        Ok(())
    }

    fn activate_by_offset(&mut self, offset: usize) -> TabId {
        let index = self
            .tabs
            .iter()
            .position(|t| t.id() == self.active)
            .expect("active タブは常に存在する");
        self.active = self.tabs[(index + offset) % self.tabs.len()].id();
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneOrigin;

    fn pane() -> Pane {
        Pane::new(PaneOrigin::User)
    }

    #[test]
    fn 初期状態は1タブでアクティブ() {
        let ws = Workspace::new("main", pane());
        assert_eq!(ws.tabs().len(), 1);
        assert_eq!(ws.active_tab().title(), "main");
        assert_eq!(ws.active_tab().tree().len(), 1);
    }

    #[test]
    fn タブの作成と切替() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        assert_eq!(ws.active_tab_id(), t2);
        ws.activate_tab(t1).unwrap();
        assert_eq!(ws.active_tab_id(), t1);
        // 巡回切替
        assert_eq!(ws.activate_next_tab(), t2);
        assert_eq!(ws.activate_next_tab(), t1);
        assert_eq!(ws.activate_prev_tab(), t2);
    }

    #[test]
    fn アクティブタブを閉じると左隣に移る() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let t3 = ws.create_tab("t3", pane());
        ws.activate_tab(t2).unwrap();
        ws.close_tab(t2).unwrap();
        assert_eq!(ws.active_tab_id(), t1);
        assert_eq!(ws.tabs().len(), 2);
        // 先頭タブを閉じたら新しい先頭へ
        ws.close_tab(t1).unwrap();
        assert_eq!(ws.active_tab_id(), t3);
    }

    #[test]
    fn 最後のタブは閉じられない() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        assert_eq!(ws.close_tab(t1).unwrap_err(), WorkspaceError::LastTab);
    }

    #[test]
    fn ペインからタブを逆引きできる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        let t2 = ws.create_tab("t2", p2);
        assert_eq!(ws.find_tab_of_pane(p1), Some(t1));
        assert_eq!(ws.find_tab_of_pane(p2_id), Some(t2));
        let ghost = pane().id();
        assert_eq!(ws.find_tab_of_pane(ghost), None);
    }

    #[test]
    fn ペインを別タブへ移送できる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        // t1 に 2 ペイン目を生やしてから t2 へ移送する
        let extra = pane();
        let extra_id = extra.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, extra)
            .unwrap();
        let t2 = ws.create_tab("t2", pane());
        ws.move_pane(extra_id, t2).unwrap();
        assert_eq!(ws.find_tab_of_pane(extra_id), Some(t2));
        assert_eq!(ws.get_tab(t1).unwrap().tree().len(), 1);
        assert_eq!(ws.get_tab(t2).unwrap().tree().len(), 2);
    }

    #[test]
    fn 最後のペインの移送はタブごと閉じる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        let t2 = ws.create_tab("t2", pane());
        ws.move_pane(p1, t2).unwrap();
        assert_eq!(ws.tabs().len(), 1);
        assert!(ws.get_tab(t1).is_none());
        assert_eq!(ws.get_tab(t2).unwrap().tree().len(), 2);
        assert_eq!(ws.find_tab_of_pane(p1), Some(t2));
    }

    #[test]
    fn 同じタブへの移送はエラー() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        assert_eq!(
            ws.move_pane(p1, t1).unwrap_err(),
            WorkspaceError::AlreadyInTab(p1, t1)
        );
    }

    #[test]
    fn 存在しないタブの操作はエラー() {
        let mut ws = Workspace::new("t1", pane());
        let ghost = {
            let mut other = Workspace::new("x", pane());
            let id = other.create_tab("ghost", pane());
            other.close_tab(id).unwrap().id()
        };
        assert_eq!(
            ws.close_tab(ghost).unwrap_err(),
            WorkspaceError::TabNotFound(ghost)
        );
        assert_eq!(
            ws.activate_tab(ghost),
            Err(WorkspaceError::TabNotFound(ghost))
        );
    }
}

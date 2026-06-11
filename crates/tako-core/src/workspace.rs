//! Workspace — タブ一覧とアクティブタブの管理（FR-1.2）

use crate::pane::Pane;
use crate::tab::{Tab, TabId};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("タブ {0} が見つからない")]
    TabNotFound(TabId),
    #[error("最後の 1 タブは閉じられない（アプリ終了は UI 層の責務）")]
    LastTab,
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

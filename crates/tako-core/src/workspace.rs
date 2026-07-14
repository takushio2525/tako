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
    #[error("ペイン {0} を自分自身の隣へは移動できない")]
    MoveOntoSelf(PaneId),
}

/// アプリ全体の状態のルート。常に 1 つ以上のタブを持つ
#[derive(Debug)]
pub struct Workspace {
    tabs: Vec<Tab>,
    active: TabId,
    /// バックグラウンド（FR-2.15）: タブから外したがプロセスは生きているペイン。
    /// 由来タブごとに分離表示する（FR-2.15.6）ため `BackgroundPane` で由来を保持する
    shelved: Vec<BackgroundPane>,
}

/// バックグラウンドへバックグラウンドしたペイン（FR-2.15）。「タブ別分離」表示（タブツリー・ドロワー）と
/// 由来タブへの復帰のため由来タブを記録する。バックグラウンドでタブごと閉じることがあり
/// 由来タブが実在しないこともあるため、ID に加えてタブ名をスナップショットしておく
#[derive(Debug)]
pub struct BackgroundPane {
    pane: Pane,
    /// バックグラウンド元タブの ID（既に閉じられている場合もある）
    origin_tab: TabId,
    /// バックグラウンド時点のタブ名（閉じたタブ由来でも親タブを明記できるよう保持する）
    origin_tab_title: String,
}

impl BackgroundPane {
    /// バックグラウンドペインを由来タブ情報とともに包む（バックグラウンド時・レイアウト復元時の両方で使う）
    pub fn from_pane(pane: Pane, origin_tab: TabId, origin_tab_title: String) -> Self {
        Self {
            pane,
            origin_tab,
            origin_tab_title,
        }
    }

    /// バックグラウンドペイン本体（タイトル・role・origin・title_source 等の参照に使う）
    pub fn pane(&self) -> &Pane {
        &self.pane
    }

    pub fn id(&self) -> PaneId {
        self.pane.id()
    }

    pub fn title(&self) -> Option<&str> {
        self.pane.title()
    }

    pub fn role(&self) -> Option<&str> {
        self.pane.role()
    }

    /// バックグラウンド元タブの ID（実在を保証しない。表示・復帰先解決に使う）
    pub fn origin_tab(&self) -> TabId {
        self.origin_tab
    }

    /// バックグラウンド時点のタブ名（親タブの明記に使う）
    pub fn origin_tab_title(&self) -> &str {
        &self.origin_tab_title
    }
}

impl Workspace {
    /// 最初のタブ（とそのルートペイン）込みで生成する
    pub fn new(initial_tab_title: impl Into<String>, root_pane: Pane) -> Self {
        let tab = Tab::new(initial_tab_title, root_pane);
        let active = tab.id();
        Self {
            tabs: vec![tab],
            active,
            shelved: Vec::new(),
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
        Some(Self {
            tabs,
            active,
            shelved: Vec::new(),
        })
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

    /// ペインを新しいタブとして分離する（Issue #209。タブバーへのペイン D&D）。
    /// 移動元タブが空になる場合はタブごと閉じる。最後のタブの最後のペインの場合は
    /// 分離先がないため LastTab を返す
    pub fn move_pane_to_new_tab(&mut self, pane: PaneId) -> Result<TabId, WorkspaceError> {
        let src = self
            .find_tab_of_pane(pane)
            .ok_or(WorkspaceError::PaneNotFound(pane))?;
        let src_tree = self
            .get_tab_mut(src)
            .expect("find_tab_of_pane で存在確認済み")
            .tree_mut();
        let moved = match src_tree.close(pane) {
            Ok(pane) => pane,
            Err(PaneTreeError::LastPane) => {
                if self.tabs.len() == 1 {
                    return Err(WorkspaceError::LastTab);
                }
                let tab = self.close_tab(src).expect("タブは 2 つ以上存在する");
                let mut panes = tab.into_tree().into_panes();
                panes.pop().expect("タブは常に 1 ペイン以上を持つ")
            }
            Err(e) => unreachable!("move_pane_to_new_tab の close で想定外のエラー: {e}"),
        };
        let new_tab = self.create_tab("", moved);
        Ok(new_tab)
    }

    /// ペインを別ペインの隣へ移動する（FR-1.10。タイトルバー D&D 移動の同等操作）。
    /// `target` を `direction` 側へ分割した位置に `pane` を挿し直す。
    /// 同タブ内の並べ替えとタブをまたぐ移動の両方に使え、移動元タブが空になる場合は
    /// タブごと閉じる
    pub fn move_pane_to(
        &mut self,
        pane: PaneId,
        target: PaneId,
        direction: SplitDirection,
    ) -> Result<(), WorkspaceError> {
        if pane == target {
            return Err(WorkspaceError::MoveOntoSelf(pane));
        }
        let src = self
            .find_tab_of_pane(pane)
            .ok_or(WorkspaceError::PaneNotFound(pane))?;
        let dst = self
            .find_tab_of_pane(target)
            .ok_or(WorkspaceError::PaneNotFound(target))?;
        let moved = if src == dst {
            // 同タブ: target が別ペインとして居るので LastPane にはならない
            self.get_tab_mut(src)
                .expect("find_tab_of_pane で存在確認済み")
                .tree_mut()
                .close(pane)
                .expect("pane と target が同居するツリーの close は成功する")
        } else {
            let src_tree = self
                .get_tab_mut(src)
                .expect("find_tab_of_pane で存在確認済み")
                .tree_mut();
            match src_tree.close(pane) {
                Ok(pane) => pane,
                Err(PaneTreeError::LastPane) => {
                    // 1 ペインだけのタブ → タブごと閉じてペインを取り出す（move_pane と同型）
                    let tab = self.close_tab(src).expect("dst タブが他に存在する");
                    let mut panes = tab.into_tree().into_panes();
                    panes.pop().expect("タブは常に 1 ペイン以上を持つ")
                }
                Err(e) => unreachable!("move_pane_to の close で想定外のエラー: {e}"),
            }
        };
        self.get_tab_mut(dst)
            .expect("find_tab_of_pane で存在確認済み")
            .tree_mut()
            .split_with_ratio(target, direction, 0.5, moved)
            .expect("target は dst のツリーに存在する");
        Ok(())
    }

    /// レイアウト復元用（FR-2.15.5）。shelved ペインも含めて復元する
    pub fn restore_with_shelved(
        tabs: Vec<Tab>,
        active: TabId,
        shelved: Vec<BackgroundPane>,
    ) -> Option<Self> {
        if tabs.is_empty() {
            return None;
        }
        let active = if tabs.iter().any(|t| t.id() == active) {
            active
        } else {
            tabs[0].id()
        };
        Some(Self {
            tabs,
            active,
            shelved,
        })
    }

    pub fn shelved_panes(&self) -> &[BackgroundPane] {
        &self.shelved
    }

    /// バックグラウンドペインを 1 件引く（由来タブの参照・復帰先解決に使う）
    pub fn shelved(&self, pane_id: PaneId) -> Option<&BackgroundPane> {
        self.shelved.iter().find(|p| p.id() == pane_id)
    }

    /// バックグラウンドペインの由来タブ ID（タブ別分離表示・復帰先の解決に使う。FR-2.15.6）
    pub fn shelved_origin_tab(&self, pane_id: PaneId) -> Option<TabId> {
        self.shelved(pane_id).map(|p| p.origin_tab())
    }

    /// ペインをバックグラウンドへバックグラウンドする（FR-2.15.1）。ペインをツリーから外してバックグラウンドに移す。
    /// タブが空になる場合はタブを閉じる。最後のタブの最後のペインの場合は LastTab を返す
    /// （アプリ層で新ペインを生やしてからリトライする判断は呼び出し側の責務）。
    /// 由来タブ（FR-2.15.6 のタブ別分離表示用）はバックグラウンド時点の ID とタブ名を記録する
    pub fn shelve_pane(&mut self, pane_id: PaneId) -> Result<(), WorkspaceError> {
        let tab_id = self
            .find_tab_of_pane(pane_id)
            .ok_or(WorkspaceError::PaneNotFound(pane_id))?;
        let origin_title = self
            .get_tab(tab_id)
            .map(|t| t.title().to_string())
            .unwrap_or_default();
        let tab = self
            .get_tab_mut(tab_id)
            .expect("find_tab_of_pane で存在確認済み");
        match tab.tree_mut().close(pane_id) {
            Ok(pane) => {
                self.shelved
                    .push(BackgroundPane::from_pane(pane, tab_id, origin_title));
                Ok(())
            }
            Err(PaneTreeError::LastPane) => {
                if self.tabs.len() > 1 {
                    let tab = self.close_tab(tab_id).expect("複数タブが存在する");
                    let mut panes = tab.into_tree().into_panes();
                    let pane = panes.pop().expect("タブは常に 1 ペイン以上を持つ");
                    self.shelved
                        .push(BackgroundPane::from_pane(pane, tab_id, origin_title));
                    Ok(())
                } else {
                    Err(WorkspaceError::LastTab)
                }
            }
            Err(e) => unreachable!("shelve_pane の close で想定外のエラー: {e}"),
        }
    }

    /// バックグラウンドからペインを復帰させる（FR-2.15.3）。target を direction 側に分割して挿入する
    pub fn unshelve_pane(
        &mut self,
        pane_id: PaneId,
        target: PaneId,
        direction: SplitDirection,
    ) -> Result<(), WorkspaceError> {
        let idx = self
            .shelved
            .iter()
            .position(|p| p.id() == pane_id)
            .ok_or(WorkspaceError::PaneNotFound(pane_id))?;
        let tab_id = self
            .find_tab_of_pane(target)
            .ok_or(WorkspaceError::PaneNotFound(target))?;
        let pane = self.shelved.remove(idx).pane;
        self.get_tab_mut(tab_id)
            .expect("find_tab_of_pane で存在確認済み")
            .tree_mut()
            .split_with_ratio(target, direction, 0.5, pane)
            .expect("target は存在するペイン");
        Ok(())
    }

    /// バックグラウンドからペインを削除する（FR-2.15.2 の kill 時に使う）
    pub fn remove_shelved(&mut self, pane_id: PaneId) -> Option<BackgroundPane> {
        let idx = self.shelved.iter().position(|p| p.id() == pane_id)?;
        Some(self.shelved.remove(idx))
    }

    /// ペインがバックグラウンドにあるか
    pub fn is_shelved(&self, pane_id: PaneId) -> bool {
        self.shelved.iter().any(|p| p.id() == pane_id)
    }

    /// タブ内の全ペインをバックグラウンドへバックグラウンドする（FR-2.15 タブ単位バックグラウンド）。
    /// タブを閉じて全ペインを shelved に移す。最後の 1 タブの場合は LastTab を返す
    /// （呼び出し側で新ペインを生やしてからリトライする想定）
    pub fn shelve_tab(&mut self, tab_id: TabId) -> Result<Vec<PaneId>, WorkspaceError> {
        let origin_title = match self.get_tab(tab_id) {
            Some(t) => t.title().to_string(),
            None => return Err(WorkspaceError::TabNotFound(tab_id)),
        };
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::LastTab);
        }
        let tab = self.close_tab(tab_id).expect("複数タブ確認済み");
        let panes = tab.into_tree().into_panes();
        let ids: Vec<PaneId> = panes.iter().map(|p| p.id()).collect();
        self.shelved.extend(
            panes
                .into_iter()
                .map(|p| BackgroundPane::from_pane(p, tab_id, origin_title.clone())),
        );
        Ok(ids)
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
    fn 同タブ内でペインを別ペインの隣へ挿し直せる() {
        // [p1 | p2] の横並びから p1 を p2 の下へ → 縦分割になる（FR-1.10）
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, p2)
            .unwrap();
        ws.move_pane_to(p1, p2_id, SplitDirection::Down).unwrap();
        let tree = ws.active_tab().tree();
        assert_eq!(tree.len(), 2);
        let rects = tree.layout(crate::Rect::UNIT);
        let rect_of = |id| {
            rects
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, r)| *r)
                .unwrap()
        };
        // p2 が上半分、p1 が下半分（幅は全幅）
        assert!(rect_of(p2_id).y < rect_of(p1).y);
        assert!((rect_of(p1).width - 1.0).abs() < 1e-5);
        // 自分自身の隣へは移動できない
        assert_eq!(
            ws.move_pane_to(p1, p1, SplitDirection::Right).unwrap_err(),
            WorkspaceError::MoveOntoSelf(p1)
        );
    }

    #[test]
    fn ペインを別タブの指定位置へ移動でき空タブは閉じる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        let t2 = ws.create_tab("t2", p2);
        // t1 が 1 ペインだけの状態から target = p2 の左へ → t1 はタブごと閉じる
        ws.move_pane_to(p1, p2_id, SplitDirection::Left).unwrap();
        assert!(ws.get_tab(t1).is_none());
        assert_eq!(ws.find_tab_of_pane(p1), Some(t2));
        let rects = ws.get_tab(t2).unwrap().tree().layout(crate::Rect::UNIT);
        let x_of = |id| rects.iter().find(|(p, _)| *p == id).map(|(_, r)| r.x);
        assert!(x_of(p1) < x_of(p2_id), "p1 が p2 の左に入る");
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
    fn ペインを新タブとして分離できる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, p2)
            .unwrap();
        assert_eq!(ws.get_tab(t1).unwrap().tree().len(), 2);
        let new_tab = ws.move_pane_to_new_tab(p2_id).unwrap();
        assert_ne!(new_tab, t1);
        assert_eq!(ws.find_tab_of_pane(p2_id), Some(new_tab));
        assert_eq!(ws.get_tab(t1).unwrap().tree().len(), 1);
        assert_eq!(ws.get_tab(new_tab).unwrap().tree().len(), 1);
        assert_eq!(ws.active_tab_id(), new_tab);
        assert_eq!(ws.tabs().len(), 2);
    }

    #[test]
    fn 最後のペインの新タブ化はタブごと移動() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let p1 = ws.active_tab().tree().focused();
        let _t2 = ws.create_tab("t2", pane());
        let new_tab = ws.move_pane_to_new_tab(p1).unwrap();
        assert!(ws.get_tab(t1).is_none());
        assert_eq!(ws.find_tab_of_pane(p1), Some(new_tab));
        assert_eq!(ws.tabs().len(), 2);
    }

    #[test]
    fn 唯一タブの唯一ペインの新タブ化はエラー() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        assert_eq!(
            ws.move_pane_to_new_tab(p1).unwrap_err(),
            WorkspaceError::LastTab
        );
    }

    #[test]
    fn ペインをバックグラウンドに送って復帰できる() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, p2)
            .unwrap();
        // p2 をバックグラウンド
        ws.shelve_pane(p2_id).unwrap();
        assert_eq!(ws.shelved_panes().len(), 1);
        assert_eq!(ws.shelved_panes()[0].id(), p2_id);
        assert_eq!(ws.active_tab().tree().len(), 1);
        assert!(ws.is_shelved(p2_id));
        // p2 を p1 の右に復帰
        ws.unshelve_pane(p2_id, p1, SplitDirection::Right).unwrap();
        assert_eq!(ws.shelved_panes().len(), 0);
        assert_eq!(ws.active_tab().tree().len(), 2);
        assert!(!ws.is_shelved(p2_id));
    }

    #[test]
    fn バックグラウンド送りでタブが空になるとタブを閉じる() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        let t2_pane = pane();
        let t2_pane_id = t2_pane.id();
        let t2 = ws.create_tab("t2", t2_pane);
        // t2 の唯一のペインをバックグラウンド → t2 はタブごと閉じる
        ws.shelve_pane(t2_pane_id).unwrap();
        assert_eq!(ws.tabs().len(), 1);
        assert_eq!(ws.shelved_panes().len(), 1);
        assert_eq!(ws.active_tab().tree().focused(), p1);
        // 由来タブは閉じても ID とタブ名がスナップショットされる（FR-2.15.6）
        let shelved = ws.shelved(t2_pane_id).unwrap();
        assert_eq!(shelved.origin_tab(), t2);
        assert_eq!(shelved.origin_tab_title(), "t2");
        assert!(ws.get_tab(t2).is_none(), "由来タブは閉じている");
        assert_eq!(ws.shelved_origin_tab(t2_pane_id), Some(t2));
    }

    #[test]
    fn 最後のタブの最後のペインはバックグラウンドに送れない() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        assert_eq!(ws.shelve_pane(p1).unwrap_err(), WorkspaceError::LastTab);
    }

    #[test]
    fn バックグラウンドのペインをkillできる() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, p2)
            .unwrap();
        ws.shelve_pane(p2_id).unwrap();
        let removed = ws.remove_shelved(p2_id);
        assert!(removed.is_some());
        assert_eq!(ws.shelved_panes().len(), 0);
    }

    #[test]
    fn タブごとバックグラウンドに送れる() {
        let mut ws = Workspace::new("t1", pane());
        let p1 = ws.active_tab().tree().focused();
        let p2 = pane();
        let p2_id = p2.id();
        ws.active_tab_mut()
            .tree_mut()
            .split(p1, SplitDirection::Right, p2)
            .unwrap();
        let t2 = ws.create_tab("t2", pane());
        // t1（p1, p2）をまとめてバックグラウンド
        let t1 = ws.tabs()[0].id();
        let shelved_ids = ws.shelve_tab(t1).unwrap();
        assert_eq!(shelved_ids.len(), 2);
        assert!(shelved_ids.contains(&p1));
        assert!(shelved_ids.contains(&p2_id));
        assert_eq!(ws.tabs().len(), 1);
        assert_eq!(ws.active_tab_id(), t2);
        assert_eq!(ws.shelved_panes().len(), 2);
        // タブ単位バックグラウンドでは全ペインが同じ由来タブ（t1）を共有する（FR-2.15.6）
        assert!(ws
            .shelved_panes()
            .iter()
            .all(|p| p.origin_tab() == t1 && p.origin_tab_title() == "t1"));
    }

    #[test]
    fn 最後のタブはタブごとバックグラウンドに送れない() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        assert_eq!(ws.shelve_tab(t1).unwrap_err(), WorkspaceError::LastTab);
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

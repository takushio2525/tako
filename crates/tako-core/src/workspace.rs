//! Workspace — タブ一覧とアクティブタブの管理（FR-1.2）
//!
//! 複数ウィンドウ（Issue #339・ビューポート方式）: タブ・ペインの実体は Workspace が
//! 単一で持ち、論理ウィンドウ（`WorkspaceWindow`）は「どのタブを表示するか」だけを持つ。
//! 各タブはちょうど 1 つのウィンドウに属する（同一タブの複数ウィンドウ同時表示はしない）。

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

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
    #[error("ウィンドウ {0} が見つからない")]
    WindowNotFound(WindowId),
    #[error("最後の 1 ウィンドウは閉じられない")]
    LastWindow,
}

/// 論理ウィンドウ ID（Issue #339）。GPUI 非依存の連番で、OS ウィンドウとの対応は
/// UI 層（tako-app）が持つ。CLI / MCP にはこの ID を公開する
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(u64);

static WINDOW_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl WindowId {
    fn next() -> Self {
        WindowId(WINDOW_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// 既知の ID から構築する（レイアウト復元用）。採番カウンタを ID の先へ進めて
    /// 後続の新規ウィンドウとの ID 再利用衝突を防ぐ（`TabId::from_raw` と同様）
    pub fn from_raw(id: u64) -> Self {
        WINDOW_ID_COUNTER.fetch_max(id.saturating_add(1), Ordering::Relaxed);
        WindowId(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ビューポート方式の論理ウィンドウ（Issue #339）。表示タブの参照だけを持ち、
/// タブ → ウィンドウの所属は `Workspace::assignments` が正
#[derive(Debug)]
pub struct WorkspaceWindow {
    id: WindowId,
    /// このウィンドウが表示中のタブ
    active: TabId,
}

impl WorkspaceWindow {
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// このウィンドウが表示中のタブ
    pub fn active_tab(&self) -> TabId {
        self.active
    }
}

/// アプリ全体の状態のルート。常に 1 つ以上のタブと 1 つ以上のウィンドウを持つ
#[derive(Debug)]
pub struct Workspace {
    tabs: Vec<Tab>,
    active: TabId,
    /// バックグラウンド（FR-2.15）: タブから外したがプロセスは生きているペイン。
    /// 由来タブごとに分離表示する（FR-2.15.6）ため `BackgroundPane` で由来を保持する
    shelved: Vec<BackgroundPane>,
    /// 論理ウィンドウ（Issue #339）。常に 1 つ以上。空になったウィンドウは即座に除去する
    windows: Vec<WorkspaceWindow>,
    /// タブ → 所属ウィンドウ。不変条件: 全タブがちょうど 1 ウィンドウに属する
    assignments: HashMap<TabId, WindowId>,
    /// フォーカスされている論理ウィンドウ。不変条件: `active` は常にこのウィンドウの表示タブ
    active_window: WindowId,
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
        Self::single_window(vec![tab], active, Vec::new())
    }

    /// 全タブを 1 つの論理ウィンドウに載せて構築する（新規作成・後方互換復元の共通経路）
    fn single_window(tabs: Vec<Tab>, active: TabId, shelved: Vec<BackgroundPane>) -> Self {
        let wid = WindowId::next();
        let assignments = tabs.iter().map(|t| (t.id(), wid)).collect();
        Self {
            tabs,
            active,
            shelved,
            windows: vec![WorkspaceWindow { id: wid, active }],
            assignments,
            active_window: wid,
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
        Some(Self::single_window(tabs, active, Vec::new()))
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

    /// 新しいタブを末尾に作成し、アクティブにする（アクティブウィンドウに属する）
    pub fn create_tab(&mut self, title: impl Into<String>, root_pane: Pane) -> TabId {
        let wid = self.active_window;
        self.create_tab_in_window(title, root_pane, wid)
            .expect("アクティブウィンドウは常に存在する")
    }

    /// 指定ウィンドウに新しいタブを作り、そのウィンドウの表示タブにする（Issue #339）。
    /// アクティブウィンドウに作った場合はグローバルのアクティブタブも切り替わる
    pub fn create_tab_in_window(
        &mut self,
        title: impl Into<String>,
        root_pane: Pane,
        wid: WindowId,
    ) -> Result<TabId, WorkspaceError> {
        if self.get_window(wid).is_none() {
            return Err(WorkspaceError::WindowNotFound(wid));
        }
        let tab = Tab::new(title, root_pane);
        let id = tab.id();
        self.tabs.push(tab);
        self.assignments.insert(id, wid);
        self.window_mut(wid).active = id;
        if self.active_window == wid {
            self.active = id;
        }
        Ok(id)
    }

    /// タブを閉じる。ウィンドウの表示タブを閉じた場合は同一ウィンドウ内の左隣
    /// （先頭なら新しい先頭）へ移る。ウィンドウが空になったら除去する（Issue #339）
    pub fn close_tab(&mut self, id: TabId) -> Result<Tab, WorkspaceError> {
        let index = self
            .tabs
            .iter()
            .position(|t| t.id() == id)
            .ok_or(WorkspaceError::TabNotFound(id))?;
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::LastTab);
        }
        let wid = self
            .assignments
            .get(&id)
            .copied()
            .unwrap_or(self.active_window);
        let tab = self.tabs.remove(index);
        self.assignments.remove(&id);
        let remaining = self.window_tab_ids(wid);
        if remaining.is_empty() {
            // ウィンドウが空 → 除去（LastTab チェック済みなので他ウィンドウにタブが必ず残る）
            self.remove_empty_window(wid);
        } else {
            // 同一ウィンドウ内の左隣: remove 後の tabs[..index] = 元の並びで閉じたタブより前
            let fallback = self.tabs[..index]
                .iter()
                .map(|t| t.id())
                .rfind(|t| self.assignments.get(t) == Some(&wid))
                .unwrap_or(remaining[0]);
            let w = self.window_mut(wid);
            if w.active == id {
                w.active = fallback;
            }
            if self.active == id {
                self.active = fallback;
            }
        }
        Ok(tab)
    }

    pub fn activate_tab(&mut self, id: TabId) -> Result<(), WorkspaceError> {
        if self.get_tab(id).is_none() {
            return Err(WorkspaceError::TabNotFound(id));
        }
        // タブの所属ウィンドウごとアクティブにする（Issue #339。ウィンドウ切替に追随）
        let wid = self
            .assignments
            .get(&id)
            .copied()
            .unwrap_or(self.active_window);
        if self.get_window(wid).is_some() {
            self.window_mut(wid).active = id;
            self.active_window = wid;
        }
        self.active = id;
        Ok(())
    }

    /// 次のタブへ巡回切替（アクティブウィンドウ内で巡回する。Issue #339）
    pub fn activate_next_tab(&mut self) -> TabId {
        self.activate_by_offset(true)
    }

    /// 前のタブへ巡回切替（アクティブウィンドウ内で巡回する。Issue #339）
    pub fn activate_prev_tab(&mut self) -> TabId {
        self.activate_by_offset(false)
    }

    /// タブを指定インデックスへ移動する（D&D 並べ替え / CLI / MCP。#308）。
    /// `target_index` はクランプされる（範囲外なら末尾）
    pub fn move_tab(&mut self, id: TabId, target_index: usize) -> Result<usize, WorkspaceError> {
        let from = self
            .tabs
            .iter()
            .position(|t| t.id() == id)
            .ok_or(WorkspaceError::TabNotFound(id))?;
        let to = target_index.min(self.tabs.len().saturating_sub(1));
        if from != to {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
        }
        Ok(to)
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
        Some(Self::single_window(tabs, active, shelved))
    }

    /// レイアウト復元用（Issue #339）。保存済みのウィンドウ割当も復元する。
    /// `windows` は (ウィンドウ ID, 所属タブ, 表示タブ)。存在しないタブ・二重割当は
    /// 読み飛ばし、未割当タブは先頭ウィンドウへ、タブを持たないウィンドウは除去する
    /// （壊れた保存値でも必ず不変条件を満たした状態で起動する）
    pub fn restore_with_windows(
        tabs: Vec<Tab>,
        active: TabId,
        shelved: Vec<BackgroundPane>,
        windows: Vec<(u64, Vec<TabId>, TabId)>,
    ) -> Option<Self> {
        if tabs.is_empty() {
            return None;
        }
        let active = if tabs.iter().any(|t| t.id() == active) {
            active
        } else {
            tabs[0].id()
        };
        let mut ws_windows: Vec<WorkspaceWindow> = Vec::new();
        let mut assignments: HashMap<TabId, WindowId> = HashMap::new();
        for (raw_id, tab_ids, win_active) in windows {
            let owned: Vec<TabId> = tab_ids
                .into_iter()
                .filter(|t| tabs.iter().any(|tab| tab.id() == *t) && !assignments.contains_key(t))
                .collect();
            if owned.is_empty() {
                continue;
            }
            let wid = WindowId::from_raw(raw_id);
            let win_active = if owned.contains(&win_active) {
                win_active
            } else {
                owned[0]
            };
            for t in &owned {
                assignments.insert(*t, wid);
            }
            ws_windows.push(WorkspaceWindow {
                id: wid,
                active: win_active,
            });
        }
        if ws_windows.is_empty() {
            return Some(Self::single_window(tabs, active, shelved));
        }
        // 保存値に載っていないタブは先頭ウィンドウへ
        let first = ws_windows[0].id;
        for t in &tabs {
            assignments.entry(t.id()).or_insert(first);
        }
        // アクティブウィンドウ = アクティブタブの所属。表示タブも同期する
        let active_window = assignments[&active];
        if let Some(w) = ws_windows.iter_mut().find(|w| w.id == active_window) {
            w.active = active;
        }
        Some(Self {
            tabs,
            active,
            shelved,
            windows: ws_windows,
            assignments,
            active_window,
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

    fn activate_by_offset(&mut self, forward: bool) -> TabId {
        // アクティブウィンドウ内で巡回する（Issue #339。単一ウィンドウなら従来どおり全タブ巡回）
        let win_tabs = self.window_tab_ids(self.active_window);
        let len = win_tabs.len();
        let index = win_tabs.iter().position(|t| *t == self.active).unwrap_or(0);
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        let id = win_tabs[next];
        self.window_mut(self.active_window).active = id;
        self.active = id;
        self.active
    }

    // === 論理ウィンドウ（Issue #339・ビューポート方式） ===

    /// 全論理ウィンドウ（常に 1 つ以上）
    pub fn windows(&self) -> &[WorkspaceWindow] {
        &self.windows
    }

    /// フォーカスされている論理ウィンドウ
    pub fn active_window_id(&self) -> WindowId {
        self.active_window
    }

    pub fn get_window(&self, id: WindowId) -> Option<&WorkspaceWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// タブが属するウィンドウ（不変条件により全タブで Some）
    pub fn window_of_tab(&self, tab: TabId) -> Option<WindowId> {
        self.assignments.get(&tab).copied()
    }

    /// ウィンドウに属するタブ ID（`tabs` 全体リストの順序を保つ）
    pub fn window_tab_ids(&self, id: WindowId) -> Vec<TabId> {
        self.tabs
            .iter()
            .map(|t| t.id())
            .filter(|t| self.assignments.get(t) == Some(&id))
            .collect()
    }

    /// ウィンドウをアクティブにする（OS のウィンドウフォーカス変化に追随する。UI 層から呼ぶ）
    pub fn activate_window(&mut self, id: WindowId) -> Result<(), WorkspaceError> {
        let win_active = self
            .get_window(id)
            .ok_or(WorkspaceError::WindowNotFound(id))?
            .active;
        self.active_window = id;
        self.active = win_active;
        Ok(())
    }

    /// 新しい論理ウィンドウを新規タブ 1 つ付きで作りアクティブにする（New Window。
    /// ウィンドウは常に 1 タブ以上を持つため必ずタブを伴う）
    pub fn create_window(
        &mut self,
        title: impl Into<String>,
        root_pane: Pane,
    ) -> (WindowId, TabId) {
        let wid = WindowId::next();
        let tab = Tab::new(title, root_pane);
        let id = tab.id();
        self.tabs.push(tab);
        self.assignments.insert(id, wid);
        self.windows.push(WorkspaceWindow {
            id: wid,
            active: id,
        });
        self.active_window = wid;
        self.active = id;
        (wid, id)
    }

    /// タブを既存ウィンドウへ移動し、移動先の表示タブにする（Issue #339）。
    /// アクティブタブを移した場合はフォーカスもタブについて行く。
    /// 移動元ウィンドウが空になったら除去し、その ID を返す（UI 層が OS ウィンドウを閉じる）
    pub fn move_tab_to_window(
        &mut self,
        tab: TabId,
        dest: WindowId,
    ) -> Result<Option<WindowId>, WorkspaceError> {
        if self.get_tab(tab).is_none() {
            return Err(WorkspaceError::TabNotFound(tab));
        }
        if self.get_window(dest).is_none() {
            return Err(WorkspaceError::WindowNotFound(dest));
        }
        let src = self
            .assignments
            .get(&tab)
            .copied()
            .unwrap_or(self.active_window);
        if src == dest {
            // 冪等: 既に居るウィンドウなら表示タブにするだけ
            self.window_mut(dest).active = tab;
            if self.active_window == dest {
                self.active = tab;
            }
            return Ok(None);
        }
        self.assignments.insert(tab, dest);
        self.window_mut(dest).active = tab;
        if self.active == tab {
            // アクティブタブを移した → フォーカスはタブについて行く
            self.active_window = dest;
        } else if self.active_window == dest {
            // アクティブウィンドウへ移してきた → 表示タブが変わるので active も同期
            self.active = tab;
        }
        // 移動元の後始末: 空なら除去、表示タブを失ったら残タブの先頭へ
        let remaining = self.window_tab_ids(src);
        if remaining.is_empty() {
            self.remove_empty_window(src);
            Ok(Some(src))
        } else {
            let w = self.window_mut(src);
            if w.active == tab {
                w.active = remaining[0];
            }
            Ok(None)
        }
    }

    /// タブを新しい論理ウィンドウへ分離する（Issue #339）。
    /// 戻り値は (新ウィンドウ, 空になって除去された移動元ウィンドウ)
    pub fn move_tab_to_new_window(
        &mut self,
        tab: TabId,
    ) -> Result<(WindowId, Option<WindowId>), WorkspaceError> {
        if self.get_tab(tab).is_none() {
            return Err(WorkspaceError::TabNotFound(tab));
        }
        let wid = WindowId::next();
        self.windows.push(WorkspaceWindow {
            id: wid,
            active: tab,
        });
        let removed = self.move_tab_to_window(tab, wid)?;
        Ok((wid, removed))
    }

    /// 論理ウィンドウを閉じ、所属タブを残存ウィンドウの末尾へ合流させる（Issue #339。
    /// ビューポートを閉じるだけでタブ・プロセスは殺さない）。合流先の表示タブは変えない。
    /// 最後の 1 ウィンドウは閉じられない。戻り値は合流したタブ
    pub fn close_window(&mut self, wid: WindowId) -> Result<Vec<TabId>, WorkspaceError> {
        if self.get_window(wid).is_none() {
            return Err(WorkspaceError::WindowNotFound(wid));
        }
        if self.windows.len() == 1 {
            return Err(WorkspaceError::LastWindow);
        }
        let moved = self.window_tab_ids(wid);
        let dest = if self.active_window != wid {
            self.active_window
        } else {
            self.windows
                .iter()
                .map(|w| w.id)
                .find(|w| *w != wid)
                .expect("2 ウィンドウ以上を確認済み")
        };
        for t in &moved {
            self.assignments.insert(*t, dest);
        }
        self.windows.retain(|w| w.id != wid);
        if self.active_window == wid {
            self.active_window = dest;
            self.active = self.get_window(dest).expect("dest は存在する").active;
        }
        Ok(moved)
    }

    /// 空になったウィンドウを除去し、アクティブウィンドウを失った場合は残存先頭へ移す
    fn remove_empty_window(&mut self, wid: WindowId) {
        self.windows.retain(|w| w.id != wid);
        if self.active_window == wid {
            let nw = self.windows[0].id;
            self.active_window = nw;
            self.active = self.windows[0].active;
        }
    }

    fn window_mut(&mut self, id: WindowId) -> &mut WorkspaceWindow {
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .expect("呼び出し前に存在確認済みのウィンドウ")
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

    #[test]
    fn タブの並べ替え_前方移動() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let t3 = ws.create_tab("t3", pane());
        // t3(末尾) を先頭へ
        assert_eq!(ws.move_tab(t3, 0).unwrap(), 0);
        let ids: Vec<_> = ws.tabs().iter().map(|t| t.id()).collect();
        assert_eq!(ids, vec![t3, t1, t2]);
    }

    #[test]
    fn タブの並べ替え_後方移動() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let t3 = ws.create_tab("t3", pane());
        // t1(先頭) を末尾へ
        assert_eq!(ws.move_tab(t1, 2).unwrap(), 2);
        let ids: Vec<_> = ws.tabs().iter().map(|t| t.id()).collect();
        assert_eq!(ids, vec![t2, t3, t1]);
    }

    #[test]
    fn タブの並べ替え_範囲外クランプ() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        // 範囲外 → 末尾へクランプ
        assert_eq!(ws.move_tab(t1, 100).unwrap(), 1);
        let ids: Vec<_> = ws.tabs().iter().map(|t| t.id()).collect();
        assert_eq!(ids, vec![t2, t1]);
    }

    #[test]
    fn タブの並べ替え_存在しないタブ() {
        let mut ws = Workspace::new("t1", pane());
        let ghost = TabId::from_raw(9999);
        assert_eq!(
            ws.move_tab(ghost, 0),
            Err(WorkspaceError::TabNotFound(ghost))
        );
    }

    // === 論理ウィンドウ（Issue #339） ===

    #[test]
    fn 初期状態は1ウィンドウで全タブが属する() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        assert_eq!(ws.windows().len(), 1);
        let wid = ws.active_window_id();
        assert_eq!(ws.window_of_tab(t1), Some(wid));
        assert_eq!(ws.window_of_tab(t2), Some(wid));
        assert_eq!(ws.window_tab_ids(wid), vec![t1, t2]);
        assert_eq!(ws.get_window(wid).unwrap().active_tab(), t2);
    }

    #[test]
    fn create_windowで新規タブ付きウィンドウがアクティブになる() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, t2) = ws.create_window("t2", pane());
        assert_eq!(ws.windows().len(), 2);
        assert_eq!(ws.active_window_id(), w2);
        assert_eq!(ws.active_tab_id(), t2);
        assert_eq!(ws.window_of_tab(t2), Some(w2));
        // 元ウィンドウは無傷
        assert_eq!(ws.window_tab_ids(w1), vec![t1]);
        assert_eq!(ws.get_window(w1).unwrap().active_tab(), t1);
    }

    #[test]
    fn activate_windowで表示タブごと切り替わる() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, t2) = ws.create_window("t2", pane());
        ws.activate_window(w1).unwrap();
        assert_eq!(ws.active_window_id(), w1);
        assert_eq!(ws.active_tab_id(), t1);
        ws.activate_window(w2).unwrap();
        assert_eq!(ws.active_tab_id(), t2);
        let ghost = WindowId::from_raw(9999);
        assert_eq!(
            ws.activate_window(ghost),
            Err(WorkspaceError::WindowNotFound(ghost))
        );
    }

    #[test]
    fn activate_tabは所属ウィンドウごとアクティブにする() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, _t2) = ws.create_window("t2", pane());
        assert_eq!(ws.active_window_id(), w2);
        // 別ウィンドウのタブをアクティブにするとウィンドウも切り替わる
        ws.activate_tab(t1).unwrap();
        assert_eq!(ws.active_window_id(), w1);
        assert_eq!(ws.active_tab_id(), t1);
    }

    #[test]
    fn タブ巡回はアクティブウィンドウ内で閉じる() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let (_w2, t3) = ws.create_window("t3", pane());
        // ウィンドウ 2（タブ 1 個）で巡回しても t3 のまま
        assert_eq!(ws.activate_next_tab(), t3);
        assert_eq!(ws.activate_prev_tab(), t3);
        // ウィンドウ 1 に切り替えると t1 ⇔ t2 で巡回し t3 を跨がない
        ws.activate_tab(t1).unwrap();
        assert_eq!(ws.activate_next_tab(), t2);
        assert_eq!(ws.activate_next_tab(), t1);
        assert_eq!(ws.activate_prev_tab(), t2);
    }

    #[test]
    fn move_tab_to_windowで移動先の表示タブになる() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let (w2, t3) = ws.create_window("t3", pane());
        // t2 を w2 へ（w1 には t1 が残る）
        assert_eq!(ws.move_tab_to_window(t2, w2).unwrap(), None);
        assert_eq!(ws.window_of_tab(t2), Some(w2));
        assert_eq!(ws.window_tab_ids(w1), vec![t1]);
        assert_eq!(ws.window_tab_ids(w2), vec![t2, t3]);
        // 移動先では移動タブが表示タブになる（アクティブウィンドウなので active も追随）
        assert_eq!(ws.get_window(w2).unwrap().active_tab(), t2);
        assert_eq!(ws.active_window_id(), w2);
        assert_eq!(ws.active_tab_id(), t2);
    }

    #[test]
    fn move_tab_to_windowで空になった移動元は除去される() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, _t2) = ws.create_window("t2", pane());
        // w1 の唯一のタブを w2 へ → w1 は除去される
        assert_eq!(ws.move_tab_to_window(t1, w2).unwrap(), Some(w1));
        assert_eq!(ws.windows().len(), 1);
        assert!(ws.get_window(w1).is_none());
        // アクティブウィンドウへ移したので表示タブ = active も追随する
        assert_eq!(ws.active_window_id(), w2);
        assert_eq!(ws.active_tab_id(), t1);
    }

    #[test]
    fn move_tab_to_new_windowで分離できる() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let (w2, removed) = ws.move_tab_to_new_window(t2).unwrap();
        assert_eq!(removed, None);
        assert_eq!(ws.windows().len(), 2);
        assert_eq!(ws.window_of_tab(t2), Some(w2));
        assert_eq!(ws.window_tab_ids(w1), vec![t1]);
        // アクティブタブ（t2）を分離したのでフォーカスは新ウィンドウへ
        assert_eq!(ws.active_window_id(), w2);
        // w1 の表示タブは残タブへ落ちている
        assert_eq!(ws.get_window(w1).unwrap().active_tab(), t1);
    }

    #[test]
    fn close_windowでタブは残存ウィンドウへ合流する() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, t2) = ws.create_window("t2", pane());
        let t3 = ws.create_tab("t3", pane());
        // w2（t2, t3）を閉じる → 両タブが w1 へ合流、タブ・実体は残る
        let moved = ws.close_window(w2).unwrap();
        assert_eq!(moved, vec![t2, t3]);
        assert_eq!(ws.windows().len(), 1);
        assert_eq!(ws.tabs().len(), 3);
        assert_eq!(ws.window_tab_ids(w1), vec![t1, t2, t3]);
        // 合流先の表示タブは変えない（w1 は t1 を表示していた）
        assert_eq!(ws.active_window_id(), w1);
        assert_eq!(ws.active_tab_id(), t1);
    }

    #[test]
    fn 最後のウィンドウは閉じられない() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        assert_eq!(ws.close_window(w1), Err(WorkspaceError::LastWindow));
    }

    #[test]
    fn close_tabでウィンドウの表示タブは同窓の左隣へ移る() {
        let mut ws = Workspace::new("t1", pane());
        let t1 = ws.active_tab_id();
        let t2 = ws.create_tab("t2", pane());
        let (w2, t3) = ws.create_window("t3", pane());
        // w1 の t2（表示タブ）を閉じる → w1 の表示タブは t1 へ（t3 は別窓なので跨がない）
        let w1 = ws.window_of_tab(t1).unwrap();
        ws.close_tab(t2).unwrap();
        assert_eq!(ws.get_window(w1).unwrap().active_tab(), t1);
        // アクティブウィンドウ（w2）は無関係のまま
        assert_eq!(ws.active_window_id(), w2);
        assert_eq!(ws.active_tab_id(), t3);
    }

    #[test]
    fn close_tabで空になったウィンドウは除去される() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let t1 = ws.active_tab_id();
        let (w2, t2) = ws.create_window("t2", pane());
        // w2 の唯一のタブを閉じる → w2 除去 + フォーカスは w1 へ戻る
        ws.close_tab(t2).unwrap();
        assert_eq!(ws.windows().len(), 1);
        assert!(ws.get_window(w2).is_none());
        assert_eq!(ws.active_window_id(), w1);
        assert_eq!(ws.active_tab_id(), t1);
    }

    #[test]
    fn create_tab_in_windowは非アクティブウィンドウのグローバルactiveを奪わない() {
        let mut ws = Workspace::new("t1", pane());
        let w1 = ws.active_window_id();
        let (w2, t2) = ws.create_window("t2", pane());
        // アクティブは w2。w1 に新規タブを作っても w2 のフォーカスは奪われない
        let t3 = ws.create_tab_in_window("t3", pane(), w1).unwrap();
        assert_eq!(ws.active_window_id(), w2);
        assert_eq!(ws.active_tab_id(), t2);
        // w1 の表示タブは新タブに切り替わる
        assert_eq!(ws.get_window(w1).unwrap().active_tab(), t3);
        let ghost = WindowId::from_raw(88888);
        assert_eq!(
            ws.create_tab_in_window("x", pane(), ghost),
            Err(WorkspaceError::WindowNotFound(ghost))
        );
    }

    #[test]
    fn restore_with_windowsで割当を復元する() {
        let t1 = Tab::new("t1", pane());
        let t2 = Tab::new("t2", pane());
        let t3 = Tab::new("t3", pane());
        let (i1, i2, i3) = (t1.id(), t2.id(), t3.id());
        let ws = Workspace::restore_with_windows(
            vec![t1, t2, t3],
            i2,
            Vec::new(),
            vec![(101, vec![i1, i2], i2), (102, vec![i3], i3)],
        )
        .unwrap();
        assert_eq!(ws.windows().len(), 2);
        let w1 = ws.window_of_tab(i1).unwrap();
        let w2 = ws.window_of_tab(i3).unwrap();
        assert_eq!(w1.as_u64(), 101);
        assert_eq!(w2.as_u64(), 102);
        assert_eq!(ws.window_of_tab(i2), Some(w1));
        assert_eq!(ws.active_window_id(), w1);
        assert_eq!(ws.active_tab_id(), i2);
        assert_eq!(ws.get_window(w2).unwrap().active_tab(), i3);
    }

    #[test]
    fn restore_with_windowsは壊れた保存値でも安全に復元する() {
        let t1 = Tab::new("t1", pane());
        let t2 = Tab::new("t2", pane());
        let (i1, i2) = (t1.id(), t2.id());
        let ghost = TabId::from_raw(77777);
        // 存在しないタブだけのウィンドウ・二重割当・未割当タブ・存在しない表示タブが混在
        let ws = Workspace::restore_with_windows(
            vec![t1, t2],
            i1,
            Vec::new(),
            vec![
                (201, vec![ghost], ghost), // 実在タブなし → ウィンドウごと読み飛ばし
                (202, vec![i1], ghost),    // 表示タブ不正 → 先頭タブへ
                (203, vec![i1], i1),       // i1 は 202 に割当済み → 空になり読み飛ばし
            ],
        )
        .unwrap();
        assert_eq!(ws.windows().len(), 1);
        let w = ws.active_window_id();
        assert_eq!(w.as_u64(), 202);
        // 未割当の i2 は先頭ウィンドウへ合流
        assert_eq!(ws.window_of_tab(i2), Some(w));
        assert_eq!(ws.window_tab_ids(w), vec![i1, i2]);
        assert_eq!(ws.active_tab_id(), i1);
    }

    #[test]
    fn restore_with_windowsの空保存値は単一ウィンドウへフォールバック() {
        let t1 = Tab::new("t1", pane());
        let i1 = t1.id();
        let ws = Workspace::restore_with_windows(vec![t1], i1, Vec::new(), Vec::new()).unwrap();
        assert_eq!(ws.windows().len(), 1);
        assert_eq!(ws.window_of_tab(i1), Some(ws.active_window_id()));
    }
}

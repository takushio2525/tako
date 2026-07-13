//! PaneTree — タブ内のペイン分割を表す二分木（GPUI 非依存）
//!
//! `.agent/architecture.md` のドメインモデルに対応する。
//! 操作 API は FR-2.5（AI レイアウト操作セット）と 1:1 対応させる前提で設計する:
//! 分割（split）/ 削除と再構成（close）/ フォーカス移動（focus / focus_direction）/
//! サイズ調整（resize_by / set_share）/ 一括調整（equalize）/ 読み取り（layout / panes）。
//!
//! 座標系は抽象的な矩形（`Rect`）で持ち、ピクセルへの対応付けは UI 層が行う。

use crate::pane::{Pane, PaneId};
use crate::spawn_layout::{SpawnLayoutConfig, SpawnLayoutPolicy, WorkerLayoutAlgorithm};

/// 分割の軸。`Horizontal` は子が左右に並ぶ（縦の境界線）、`Vertical` は上下に並ぶ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// 分割・フォーカス移動の方向。新ペインは指定方向側に生える
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
    Left,
    Up,
}

impl SplitDirection {
    pub fn axis(self) -> SplitAxis {
        match self {
            SplitDirection::Right | SplitDirection::Left => SplitAxis::Horizontal,
            SplitDirection::Down | SplitDirection::Up => SplitAxis::Vertical,
        }
    }
}

/// 抽象矩形。layout() は単位矩形ベースでも実寸ベースでも計算できる
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
}

/// ツリーのノード。Split の `ratio` は first 側の取り分（0.0–1.0）
#[derive(Debug)]
pub enum PaneNode {
    Leaf(Pane),
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PaneTreeError {
    #[error("ペイン {0} が見つからない")]
    PaneNotFound(PaneId),
    #[error("最後の 1 ペインは閉じられない（タブごと閉じる操作は Workspace 側で行う）")]
    LastPane,
    #[error("ペイン {0} には {1:?} 軸でリサイズできる分割がない")]
    NoResizableSplit(PaneId, SplitAxis),
}

/// 分割比率のクランプ範囲。極端な比率でペインが潰れるのを防ぐ
pub(crate) const MIN_SHARE: f32 = 0.1;
pub(crate) const MAX_SHARE: f32 = 0.9;

/// 浮動小数の比較誤差吸収（focus_direction の隣接判定に使う）
const EPS: f32 = 1e-4;

/// 復元ツリーの比率を妥当域へ再帰的にクランプする（`from_root` 用）
fn clamp_ratios(node: &mut PaneNode) {
    if let PaneNode::Split {
        ratio,
        first,
        second,
        ..
    } = node
    {
        if !ratio.is_finite() {
            *ratio = 0.5;
        }
        *ratio = ratio.clamp(MIN_SHARE, MAX_SHARE);
        clamp_ratios(first);
        clamp_ratios(second);
    }
}

/// タブ内のペイン分割ツリー。常に 1 つ以上のペインを持つ
#[derive(Debug)]
pub struct PaneTree {
    /// 不変条件: 操作の途中以外は常に Some
    root: Option<PaneNode>,
    focused: PaneId,
}

impl PaneTree {
    pub fn new(root_pane: Pane) -> Self {
        let focused = root_pane.id();
        Self {
            root: Some(PaneNode::Leaf(root_pane)),
            focused,
        }
    }

    /// レイアウト復元用（Phase 5.5）。保存済みツリーから構築する。
    /// focused がツリーに無ければ先頭ペインへ、比率は妥当域へクランプする
    /// （壊れた保存値からの防御）
    pub fn from_root(root: PaneNode, focused: PaneId) -> Self {
        let mut tree = Self {
            root: Some(root),
            focused,
        };
        clamp_ratios(tree.root_mut());
        if !tree.contains(focused) {
            tree.focused = tree.panes()[0].id();
        }
        tree
    }

    fn root_ref(&self) -> &PaneNode {
        // 不変条件: root は操作の途中でのみ take される（論理的に到達不能）
        self.root.as_ref().expect("PaneTree.root は常に Some")
    }

    fn root_mut(&mut self) -> &mut PaneNode {
        self.root.as_mut().expect("PaneTree.root は常に Some")
    }

    /// UI 層がレンダリングに使うツリー構造への参照
    pub fn root(&self) -> &PaneNode {
        self.root_ref()
    }

    pub fn focused(&self) -> PaneId {
        self.focused
    }

    pub fn len(&self) -> usize {
        self.panes().len()
    }

    pub fn is_empty(&self) -> bool {
        false // 不変条件: 常に 1 ペイン以上
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: PaneId) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id() == id)
    }

    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        fn rec(node: &mut PaneNode, id: PaneId) -> Option<&mut Pane> {
            match node {
                PaneNode::Leaf(pane) => (pane.id() == id).then_some(pane),
                PaneNode::Split { first, second, .. } => rec(first, id).or_else(|| rec(second, id)),
            }
        }
        rec(self.root_mut(), id)
    }

    /// 全ペインを木の左上から順（in-order）で返す
    pub fn panes(&self) -> Vec<&Pane> {
        fn rec<'a>(node: &'a PaneNode, out: &mut Vec<&'a Pane>) {
            match node {
                PaneNode::Leaf(pane) => out.push(pane),
                PaneNode::Split { first, second, .. } => {
                    rec(first, out);
                    rec(second, out);
                }
            }
        }
        let mut out = Vec::new();
        rec(self.root_ref(), &mut out);
        out
    }

    /// target を direction 方向に分割し、新ペインを挿入する（比率は等分）
    pub fn split(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        pane: Pane,
    ) -> Result<PaneId, PaneTreeError> {
        self.split_with_ratio(target, direction, 0.5, pane)
    }

    /// 比率指定つき分割。`ratio` は新ペイン側の取り分（クランプされる）
    pub fn split_with_ratio(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        ratio: f32,
        pane: Pane,
    ) -> Result<PaneId, PaneTreeError> {
        if !self.contains(target) {
            return Err(PaneTreeError::PaneNotFound(target));
        }
        let new_id = pane.id();
        let new_share = ratio.clamp(MIN_SHARE, MAX_SHARE);

        fn rec(
            node: PaneNode,
            target: PaneId,
            direction: SplitDirection,
            new_share: f32,
            slot: &mut Option<Pane>,
        ) -> PaneNode {
            match node {
                PaneNode::Leaf(existing) if existing.id() == target => {
                    // contains 確認済みのため slot は必ず残っている（論理的に到達不能）
                    let new_pane = slot.take().expect("新ペインは一度だけ挿入される");
                    let existing = Box::new(PaneNode::Leaf(existing));
                    let new_leaf = Box::new(PaneNode::Leaf(new_pane));
                    // Right / Down は新ペインが second 側、Left / Up は first 側
                    let (first, second, first_share) = match direction {
                        SplitDirection::Right | SplitDirection::Down => {
                            (existing, new_leaf, 1.0 - new_share)
                        }
                        SplitDirection::Left | SplitDirection::Up => {
                            (new_leaf, existing, new_share)
                        }
                    };
                    PaneNode::Split {
                        axis: direction.axis(),
                        ratio: first_share,
                        first,
                        second,
                    }
                }
                leaf @ PaneNode::Leaf(_) => leaf,
                PaneNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => PaneNode::Split {
                    axis,
                    ratio,
                    first: Box::new(rec(*first, target, direction, new_share, slot)),
                    second: Box::new(rec(*second, target, direction, new_share, slot)),
                },
            }
        }

        let mut slot = Some(pane);
        let root = self.root.take().expect("PaneTree.root は常に Some");
        self.root = Some(rec(root, target, direction, new_share, &mut slot));
        self.focused = new_id;
        Ok(new_id)
    }

    /// target ペインを閉じる。兄弟サブツリーが親の位置へ昇格する（ツリー再構成）。
    /// フォーカス中のペインを閉じた場合は昇格側の先頭ペインへフォーカスが移る
    pub fn close(&mut self, target: PaneId) -> Result<Pane, PaneTreeError> {
        if !self.contains(target) {
            return Err(PaneTreeError::PaneNotFound(target));
        }
        if matches!(self.root_ref(), PaneNode::Leaf(_)) {
            return Err(PaneTreeError::LastPane);
        }

        /// 戻り値: (再構成後のサブツリー。target の Leaf 自体なら None, 取り除いた Pane)
        fn rec(node: PaneNode, target: PaneId) -> (Option<PaneNode>, Option<Pane>) {
            match node {
                PaneNode::Leaf(pane) if pane.id() == target => (None, Some(pane)),
                leaf @ PaneNode::Leaf(_) => (Some(leaf), None),
                PaneNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    let (first, removed) = rec(*first, target);
                    if let Some(pane) = removed {
                        let node = match first {
                            // first の深部から取り除けた → 構造を保つ
                            Some(first) => PaneNode::Split {
                                axis,
                                ratio,
                                first: Box::new(first),
                                second,
                            },
                            // first そのものが消えた → second を昇格
                            None => *second,
                        };
                        return (Some(node), Some(pane));
                    }
                    let first = first.expect("removed が None なら first は残っている");
                    let (second, removed) = rec(*second, target);
                    let node = match second {
                        Some(second) => PaneNode::Split {
                            axis,
                            ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        },
                        None => first,
                    };
                    (Some(node), removed)
                }
            }
        }

        let root = self.root.take().expect("PaneTree.root は常に Some");
        let (root, removed) = rec(root, target);
        self.root = Some(root.expect("root が Split なら除去後もノードが残る"));
        let pane = removed.expect("contains 確認済み");

        if self.focused == target {
            // 昇格後ツリーの先頭ペインへフォーカスを引き継ぐ
            self.focused = self.panes()[0].id();
        }
        Ok(pane)
    }

    pub fn focus(&mut self, target: PaneId) -> Result<(), PaneTreeError> {
        if !self.contains(target) {
            return Err(PaneTreeError::PaneNotFound(target));
        }
        self.focused = target;
        Ok(())
    }

    /// フォーカス中ペインから見て direction 方向の隣接ペインへフォーカス移動する。
    /// 隣接ペインが無ければ None（フォーカスは動かない）
    pub fn focus_direction(&mut self, direction: SplitDirection) -> Option<PaneId> {
        let rects = self.layout(Rect::UNIT);
        let (_, cur) = rects.iter().find(|(id, _)| *id == self.focused)?;
        let cur = *cur;

        let mut best: Option<(f32, PaneId)> = None;
        for (id, r) in &rects {
            if *id == self.focused {
                continue;
            }
            // direction 側にあり、直交方向に重なりがあるペインだけが候補
            let (beyond, overlap, dist) = match direction {
                SplitDirection::Right => (
                    r.x >= cur.right() - EPS,
                    overlap_len(r.y, r.bottom(), cur.y, cur.bottom()),
                    r.center_x() - cur.center_x(),
                ),
                SplitDirection::Left => (
                    r.right() <= cur.x + EPS,
                    overlap_len(r.y, r.bottom(), cur.y, cur.bottom()),
                    cur.center_x() - r.center_x(),
                ),
                SplitDirection::Down => (
                    r.y >= cur.bottom() - EPS,
                    overlap_len(r.x, r.right(), cur.x, cur.right()),
                    r.center_y() - cur.center_y(),
                ),
                SplitDirection::Up => (
                    r.bottom() <= cur.y + EPS,
                    overlap_len(r.x, r.right(), cur.x, cur.right()),
                    cur.center_y() - r.center_y(),
                ),
            };
            if beyond && overlap > EPS && best.is_none_or(|(d, _)| dist < d) {
                best = Some((dist, *id));
            }
        }
        let (_, id) = best?;
        self.focused = id;
        Some(id)
    }

    /// target が属する側の取り分を delta だけ相対的に増減する（FR-2.5.6）。
    /// 対象は target を含む最も近い axis 軸の祖先分割。戻り値は変更後の取り分
    pub fn resize_by(
        &mut self,
        target: PaneId,
        axis: SplitAxis,
        delta: f32,
    ) -> Result<f32, PaneTreeError> {
        self.adjust_share(target, axis, |share| share + delta)
    }

    /// target が属する側の取り分を絶対値で指定する（FR-2.5.6）
    pub fn set_share(
        &mut self,
        target: PaneId,
        axis: SplitAxis,
        share: f32,
    ) -> Result<f32, PaneTreeError> {
        self.adjust_share(target, axis, |_| share)
    }

    fn adjust_share(
        &mut self,
        target: PaneId,
        axis: SplitAxis,
        f: impl FnOnce(f32) -> f32,
    ) -> Result<f32, PaneTreeError> {
        if !self.contains(target) {
            return Err(PaneTreeError::PaneNotFound(target));
        }

        /// target を含む最も近い（最深の）axis 軸の祖先分割の
        /// (ratio への可変参照, target が first 側か) を返す
        fn find(node: &mut PaneNode, target: PaneId, axis: SplitAxis) -> Option<(&mut f32, bool)> {
            fn contains(node: &PaneNode, target: PaneId) -> bool {
                match node {
                    PaneNode::Leaf(pane) => pane.id() == target,
                    PaneNode::Split { first, second, .. } => {
                        contains(first, target) || contains(second, target)
                    }
                }
            }
            let PaneNode::Split {
                axis: node_axis,
                ratio,
                first,
                second,
            } = node
            else {
                return None;
            };
            let in_first = contains(first, target);
            if !in_first && !contains(second, target) {
                return None;
            }
            let child = if in_first { first } else { second };
            // 子側により近い祖先があればそちらを優先
            if let Some(found) = find(child, target, axis) {
                return Some(found);
            }
            (*node_axis == axis).then_some((ratio, in_first))
        }

        let Some((ratio, in_first)) = find(self.root_mut(), target, axis) else {
            return Err(PaneTreeError::NoResizableSplit(target, axis));
        };
        // ratio は first 側の取り分なので、target が second 側なら反転して扱う
        let share = if in_first { *ratio } else { 1.0 - *ratio };
        let new_share = f(share).clamp(MIN_SHARE, MAX_SHARE);
        *ratio = if in_first { new_share } else { 1.0 - new_share };
        Ok(new_share)
    }

    /// ツリーを消費して全ペインを取り出す（タブを閉じてペインを移送する操作で使う）
    pub fn into_panes(mut self) -> Vec<Pane> {
        fn rec(node: PaneNode, out: &mut Vec<Pane>) {
            match node {
                PaneNode::Leaf(pane) => out.push(pane),
                PaneNode::Split { first, second, .. } => {
                    rec(*first, out);
                    rec(*second, out);
                }
            }
        }
        let mut out = Vec::new();
        rec(
            self.root.take().expect("PaneTree.root は常に Some"),
            &mut out,
        );
        out
    }

    /// 全分割の比率をリーフ数に応じて均等化する（FR-2.5.7 のプリセット相当）
    pub fn equalize(&mut self) {
        fn rec(node: &mut PaneNode) -> usize {
            match node {
                PaneNode::Leaf(_) => 1,
                PaneNode::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    let a = rec(first);
                    let b = rec(second);
                    *ratio = a as f32 / (a + b) as f32;
                    a + b
                }
            }
        }
        rec(self.root_mut());
    }

    /// bounds をツリーの比率で再帰分割し、各ペインの矩形を返す（FR-2.5.1 の読み取り基盤）
    pub fn layout(&self, bounds: Rect) -> Vec<(PaneId, Rect)> {
        fn rec(node: &PaneNode, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
            match node {
                PaneNode::Leaf(pane) => out.push((pane.id(), r)),
                PaneNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    let (r1, r2) = split_rects(r, *axis, *ratio);
                    rec(first, r1, out);
                    rec(second, r2, out);
                }
            }
        }
        let mut out = Vec::new();
        rec(self.root_ref(), bounds, &mut out);
        out
    }

    /// 全分割の仕切り線を pre-order（`set_split_ratio` の index と同順）で列挙する。
    /// UI 層の境界ヒットテスト・カーソル変更・ドラッグリサイズに使う。
    /// 座標は `layout` に渡した bounds と同じ空間で返す
    pub fn borders(&self, bounds: Rect) -> Vec<PaneBorder> {
        fn rec(node: &PaneNode, r: Rect, idx: &mut usize, out: &mut Vec<PaneBorder>) {
            let PaneNode::Split {
                axis,
                ratio,
                first,
                second,
            } = node
            else {
                return;
            };
            let my_index = *idx;
            *idx += 1;
            let (r1, r2) = split_rects(r, *axis, *ratio);
            // 仕切り線は first と second の境目。線が走る範囲は分割領域の長辺
            let border = match axis {
                SplitAxis::Horizontal => PaneBorder {
                    axis: *axis,
                    area: r,
                    index: my_index,
                    ratio: *ratio,
                    position: r1.right(),
                    span_start: r.y,
                    span_end: r.bottom(),
                },
                SplitAxis::Vertical => PaneBorder {
                    axis: *axis,
                    area: r,
                    index: my_index,
                    ratio: *ratio,
                    position: r1.bottom(),
                    span_start: r.x,
                    span_end: r.right(),
                },
            };
            out.push(border);
            rec(first, r1, idx, out);
            rec(second, r2, idx, out);
        }
        let mut out = Vec::new();
        let mut idx = 0;
        rec(self.root_ref(), bounds, &mut idx, &mut out);
        out
    }

    /// `borders` の `index` が指す分割の first 側取り分を絶対設定する（ドラッグ反映用）。
    /// `MIN_SHARE..=MAX_SHARE` にクランプし、設定後の ratio を返す。index が無効なら None
    pub fn set_split_ratio(&mut self, index: usize, ratio: f32) -> Option<f32> {
        fn rec(node: &mut PaneNode, target: usize, idx: &mut usize, ratio: f32) -> Option<f32> {
            let PaneNode::Split {
                ratio: r,
                first,
                second,
                ..
            } = node
            else {
                return None;
            };
            let my_index = *idx;
            *idx += 1;
            if my_index == target {
                let new = ratio.clamp(MIN_SHARE, MAX_SHARE);
                *r = new;
                return Some(new);
            }
            rec(first, target, idx, ratio).or_else(|| rec(second, target, idx, ratio))
        }
        let mut idx = 0;
        rec(self.root_mut(), index, &mut idx, ratio)
    }
}

// --- spawn レイアウトエンジン（Issue #165、FR-2.20） ---

impl PaneTree {
    /// spawn レイアウトポリシーに従い worker ペインを配置する（Issue #165）。
    ///
    /// - `Legacy`: anchor の右に等分割（従来挙動）
    /// - `MasterReserved`: anchor に「worker 領域」（anchor から spawn された worker だけの
    ///   サブツリー）が無ければ anchor を右分割して新設し、anchor 側に `master_ratio` を
    ///   残す。既にあれば領域内を `algorithm` で再構築して新ペインを加える
    ///   （anchor と領域外ペインの矩形は変わらない）
    ///
    /// フォーカスは新ペインへ移る（`split` と同じ。呼び出し側が必要なら戻す）。
    /// worker 領域の判定は各ペインの `spawned_by` チェーンに依るため、
    /// 呼び出し側は配置後に新ペインへ `set_spawned_by(anchor)` を設定すること
    pub fn spawn_worker(
        &mut self,
        anchor: PaneId,
        pane: Pane,
        config: &SpawnLayoutConfig,
    ) -> Result<PaneId, PaneTreeError> {
        if config.policy == SpawnLayoutPolicy::Legacy {
            return self.split_with_ratio(
                anchor,
                SplitDirection::Right,
                crate::spawn_layout::LEGACY_WORKER_SHARE,
                pane,
            );
        }
        if !self.contains(anchor) {
            return Err(PaneTreeError::PaneNotFound(anchor));
        }
        let new_id = pane.id();
        let mut slot = Some(pane);
        if self.rebuild_worker_area(anchor, &mut slot, config.algorithm) {
            self.focused = new_id;
            Ok(new_id)
        } else {
            // worker 領域がまだ無い → anchor を右分割して新設。
            // 新ペイン（worker 領域）側の取り分 = 1 - master_ratio
            let worker_share = 1.0 - crate::spawn_layout::clamp_master_ratio(config.master_ratio);
            let pane = slot.take().expect("領域未発見時は消費されない");
            self.split_with_ratio(anchor, SplitDirection::Right, worker_share, pane)
        }
    }

    /// worker close 後のリフロー（Issue #165）。anchor の worker 領域を `algorithm` で
    /// 組み直し、空いた場所を残りの worker で再配分する。master・ユーザー由来ペインの
    /// 矩形は変わらない。worker 領域が無い（全 worker が閉じた・anchor が消えた等）
    /// 場合は何もせず false を返す
    pub fn reflow_workers(&mut self, anchor: PaneId, algorithm: WorkerLayoutAlgorithm) -> bool {
        if !self.contains(anchor) {
            return false;
        }
        self.rebuild_worker_area(anchor, &mut None, algorithm)
    }

    /// anchor の worker 領域を見つけ、（あれば `extra` を末尾に加えて）`algorithm` で
    /// 再構築する。worker 領域 = anchor Leaf から根へのパス上で anchor と反対側にあり、
    /// 全リーフの `spawned_by` チェーンが anchor へ到達するサブツリー
    /// （anchor に最も近い祖先を優先）。見つからなければ何もせず false
    fn rebuild_worker_area(
        &mut self,
        anchor: PaneId,
        extra: &mut Option<Pane>,
        algorithm: WorkerLayoutAlgorithm,
    ) -> bool {
        use std::collections::{HashMap, HashSet};

        // spawned_by チェーン判定表を先に作る（木の再帰中に self を再借用しないため）
        let spawn_map: HashMap<PaneId, Option<PaneId>> = self
            .panes()
            .iter()
            .map(|p| (p.id(), p.spawned_by()))
            .collect();
        let mut workers: HashSet<PaneId> = HashSet::new();
        for &id in spawn_map.keys() {
            let mut cur = id;
            let mut seen = HashSet::new();
            while let Some(Some(parent)) = spawn_map.get(&cur).copied() {
                if !seen.insert(cur) {
                    break; // 保存データ破損などによる循環の防御
                }
                if parent == anchor {
                    workers.insert(id);
                    break;
                }
                cur = parent;
            }
        }
        if workers.is_empty() {
            return false;
        }

        fn subtree_contains(node: &PaneNode, id: PaneId) -> bool {
            match node {
                PaneNode::Leaf(p) => p.id() == id,
                PaneNode::Split { first, second, .. } => {
                    subtree_contains(first, id) || subtree_contains(second, id)
                }
            }
        }

        fn all_workers(node: &PaneNode, workers: &std::collections::HashSet<PaneId>) -> bool {
            match node {
                PaneNode::Leaf(p) => workers.contains(&p.id()),
                PaneNode::Split { first, second, .. } => {
                    all_workers(first, workers) && all_workers(second, workers)
                }
            }
        }

        fn collect_leaves(node: PaneNode, out: &mut Vec<Pane>) {
            match node {
                PaneNode::Leaf(p) => out.push(p),
                PaneNode::Split { first, second, .. } => {
                    collect_leaves(*first, out);
                    collect_leaves(*second, out);
                }
            }
        }

        /// anchor を含む側を先に深く処理し（= anchor に最も近い祖先を優先）、
        /// 見つからなければ自分の反対側サブツリーが worker 領域かを判定する
        fn rec(
            node: PaneNode,
            anchor: PaneId,
            workers: &std::collections::HashSet<PaneId>,
            extra: &mut Option<Pane>,
            algorithm: WorkerLayoutAlgorithm,
            done: &mut bool,
        ) -> PaneNode {
            let PaneNode::Split {
                axis,
                ratio,
                first,
                second,
            } = node
            else {
                return node;
            };
            let in_first = subtree_contains(&first, anchor);
            if !in_first && !subtree_contains(&second, anchor) {
                return PaneNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                };
            }
            let (near, far) = if in_first {
                (first, second)
            } else {
                (second, first)
            };
            let near = Box::new(rec(*near, anchor, workers, extra, algorithm, done));
            let far = if !*done && all_workers(&far, workers) {
                let mut panes = Vec::new();
                collect_leaves(*far, &mut panes);
                if let Some(p) = extra.take() {
                    panes.push(p);
                }
                *done = true;
                Box::new(crate::spawn_layout::build_worker_area(panes, algorithm))
            } else {
                far
            };
            let (first, second) = if in_first { (near, far) } else { (far, near) };
            PaneNode::Split {
                axis,
                ratio,
                first,
                second,
            }
        }

        let mut done = false;
        let root = self.root.take().expect("PaneTree.root は常に Some");
        self.root = Some(rec(root, anchor, &workers, extra, algorithm, &mut done));
        done
    }
}

/// 矩形 `r` を `axis` 軸・`ratio`（first 側取り分）で 2 分割する
fn split_rects(r: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect) {
    match axis {
        SplitAxis::Horizontal => {
            let w = r.width * ratio;
            (
                Rect::new(r.x, r.y, w, r.height),
                Rect::new(r.x + w, r.y, r.width - w, r.height),
            )
        }
        SplitAxis::Vertical => {
            let h = r.height * ratio;
            (
                Rect::new(r.x, r.y, r.width, h),
                Rect::new(r.x, r.y + h, r.width, r.height - h),
            )
        }
    }
}

/// 仕切り領域 `area` 内のポインタ座標 `(x, y)` を first 側取り分比率へ換算する（純粋関数）。
/// Horizontal は x、Vertical は y を使う。`MIN_SHARE..=MAX_SHARE` にクランプする
pub fn ratio_for_position(area: Rect, axis: SplitAxis, x: f32, y: f32) -> f32 {
    let raw = match axis {
        SplitAxis::Horizontal if area.width > 0.0 => (x - area.x) / area.width,
        SplitAxis::Vertical if area.height > 0.0 => (y - area.y) / area.height,
        _ => 0.5,
    };
    raw.clamp(MIN_SHARE, MAX_SHARE)
}

/// ドラッグ可能なペイン境界線（分割の仕切り）。座標は `borders` に渡した bounds と同じ空間
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneBorder {
    /// この境界を生む分割の軸。Horizontal = 左右分割の縦線、Vertical = 上下分割の横線
    pub axis: SplitAxis,
    /// 分割対象の領域全体（`ratio_for_position` での座標→比率換算に使う）
    pub area: Rect,
    /// `set_split_ratio` に渡す分割インデックス（pre-order の Split 順）
    pub index: usize,
    /// 現在の first 側取り分
    pub ratio: f32,
    /// 仕切り線の位置。Horizontal なら x 座標、Vertical なら y 座標
    pub position: f32,
    /// 仕切り線が走る範囲の始点（Horizontal なら y、Vertical なら x）
    pub span_start: f32,
    /// 仕切り線が走る範囲の終点
    pub span_end: f32,
}

/// 区間 [a1, a2) と [b1, b2) の重なり長
fn overlap_len(a1: f32, a2: f32, b1: f32, b2: f32) -> f32 {
    (a2.min(b2) - a1.max(b1)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneOrigin;

    fn tree() -> (PaneTree, PaneId) {
        let pane = Pane::new(PaneOrigin::User);
        let id = pane.id();
        (PaneTree::new(pane), id)
    }

    fn rect_of(tree: &PaneTree, id: PaneId) -> Rect {
        tree.layout(Rect::UNIT)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
            .expect("ペインがレイアウトに存在する")
    }

    fn assert_close_to(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "期待値 {b} に対して実際は {a}");
    }

    #[test]
    fn 右分割で新ペインが右側に生えフォーカスが移る() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::Cli))
            .unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.focused(), new);
        let (r_root, r_new) = (rect_of(&t, root), rect_of(&t, new));
        assert_close_to(r_root.x, 0.0);
        assert_close_to(r_root.width, 0.5);
        assert_close_to(r_new.x, 0.5);
        assert_close_to(r_new.width, 0.5);
        // 高さは変わらない
        assert_close_to(r_new.height, 1.0);
    }

    #[test]
    fn 上分割で新ペインが上側_first側_に生える() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Up, Pane::new(PaneOrigin::User))
            .unwrap();
        let r_new = rect_of(&t, new);
        assert_close_to(r_new.y, 0.0);
        assert_close_to(r_new.height, 0.5);
        let r_root = rect_of(&t, root);
        assert_close_to(r_root.y, 0.5);
    }

    #[test]
    fn 比率指定の分割は新ペイン側の取り分になる() {
        let (mut t, root) = tree();
        let new = t
            .split_with_ratio(root, SplitDirection::Right, 0.7, Pane::new(PaneOrigin::Mcp))
            .unwrap();
        assert_close_to(rect_of(&t, new).width, 0.7);
        assert_close_to(rect_of(&t, root).width, 0.3);
    }

    #[test]
    fn 比率はクランプされる() {
        let (mut t, root) = tree();
        let new = t
            .split_with_ratio(
                root,
                SplitDirection::Right,
                0.99,
                Pane::new(PaneOrigin::User),
            )
            .unwrap();
        assert_close_to(rect_of(&t, new).width, 0.9);
    }

    #[test]
    fn ネスト分割の構造とレイアウト() {
        // [root | new1] の new1 を下分割 → [root | new1 / new2]
        let (mut t, root) = tree();
        let new1 = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let new2 = t
            .split(new1, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        assert_eq!(t.len(), 3);
        let r2 = rect_of(&t, new2);
        assert_close_to(r2.x, 0.5);
        assert_close_to(r2.y, 0.5);
        assert_close_to(r2.width, 0.5);
        assert_close_to(r2.height, 0.5);
        // root は左半分のまま
        assert_close_to(rect_of(&t, root).width, 0.5);
        assert_close_to(rect_of(&t, root).height, 1.0);
    }

    #[test]
    fn closeで兄弟が昇格しツリーが再構成される() {
        let (mut t, root) = tree();
        let new1 = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let new2 = t
            .split(new1, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        // new1 を閉じると new2 が右半分全体へ昇格する
        let closed = t.close(new1).unwrap();
        assert_eq!(closed.id(), new1);
        assert_eq!(t.len(), 2);
        let r2 = rect_of(&t, new2);
        assert_close_to(r2.x, 0.5);
        assert_close_to(r2.y, 0.0);
        assert_close_to(r2.height, 1.0);
    }

    #[test]
    fn フォーカス中ペインを閉じるとフォーカスが引き継がれる() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        assert_eq!(t.focused(), new);
        t.close(new).unwrap();
        assert_eq!(t.focused(), root);
    }

    #[test]
    fn 最後のペインは閉じられない() {
        let (mut t, root) = tree();
        assert_eq!(t.close(root).unwrap_err(), PaneTreeError::LastPane);
    }

    #[test]
    fn 存在しないペインの操作はエラー() {
        let (mut t, root) = tree();
        let ghost = Pane::new(PaneOrigin::User).id();
        assert_eq!(
            t.split(ghost, SplitDirection::Right, Pane::new(PaneOrigin::User)),
            Err(PaneTreeError::PaneNotFound(ghost))
        );
        assert_eq!(t.focus(ghost), Err(PaneTreeError::PaneNotFound(ghost)));
        assert_eq!(
            t.close(ghost).unwrap_err(),
            PaneTreeError::PaneNotFound(ghost)
        );
        // root は無傷
        assert_eq!(t.len(), 1);
        assert!(t.contains(root));
    }

    #[test]
    fn idでのフォーカス移動() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        t.focus(root).unwrap();
        assert_eq!(t.focused(), root);
        t.focus(new).unwrap();
        assert_eq!(t.focused(), new);
    }

    #[test]
    fn 方向フォーカス移動_2x2グリッド() {
        // 左上(a) 右上(b) 左下(c) 右下(d) の 2x2 を組む
        let (mut t, a) = tree();
        let b = t
            .split(a, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let c = t
            .split(a, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        let d = t
            .split(b, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();

        t.focus(a).unwrap();
        assert_eq!(t.focus_direction(SplitDirection::Right), Some(b));
        assert_eq!(t.focus_direction(SplitDirection::Down), Some(d));
        assert_eq!(t.focus_direction(SplitDirection::Left), Some(c));
        assert_eq!(t.focus_direction(SplitDirection::Up), Some(a));
        // 端では動かない
        assert_eq!(t.focus_direction(SplitDirection::Up), None);
        assert_eq!(t.focused(), a);
    }

    #[test]
    fn リサイズで取り分が変わりクランプされる() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        // new（second 側）を 0.2 広げる
        let share = t.resize_by(new, SplitAxis::Horizontal, 0.2).unwrap();
        assert_close_to(share, 0.7);
        assert_close_to(rect_of(&t, new).width, 0.7);
        assert_close_to(rect_of(&t, root).width, 0.3);
        // 過大な delta はクランプ
        let share = t.resize_by(new, SplitAxis::Horizontal, 10.0).unwrap();
        assert_close_to(share, 0.9);
    }

    #[test]
    fn set_shareで取り分を直接指定できる() {
        let (mut t, root) = tree();
        let _ = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        // root（first 側）の取り分を 0.8 に
        let share = t.set_share(root, SplitAxis::Horizontal, 0.8).unwrap();
        assert_close_to(share, 0.8);
        assert_close_to(rect_of(&t, root).width, 0.8);
    }

    #[test]
    fn 軸が合わない分割しかなければリサイズはエラー() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        assert_eq!(
            t.resize_by(new, SplitAxis::Vertical, 0.1),
            Err(PaneTreeError::NoResizableSplit(new, SplitAxis::Vertical))
        );
    }

    #[test]
    fn ネスト時は最も近い祖先分割がリサイズ対象() {
        // [root | new1 / new2] で new2 を Vertical リサイズ → 内側の分割が変わる
        let (mut t, root) = tree();
        let new1 = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let new2 = t
            .split(new1, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        t.resize_by(new2, SplitAxis::Vertical, 0.25).unwrap();
        assert_close_to(rect_of(&t, new2).height, 0.75);
        // 外側の Horizontal 分割は無傷
        assert_close_to(rect_of(&t, root).width, 0.5);
        // new2 の Horizontal リサイズは外側の分割に効く
        t.resize_by(new2, SplitAxis::Horizontal, 0.2).unwrap();
        assert_close_to(rect_of(&t, root).width, 0.3);
    }

    #[test]
    fn equalizeでリーフ数に応じた均等割になる() {
        // 右に 2 回連続分割 → 入れ子の比率を均して 1/3 ずつにする
        let (mut t, a) = tree();
        let b = t
            .split(a, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let c = t
            .split(b, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        t.equalize();
        for id in [a, b, c] {
            assert_close_to(rect_of(&t, id).width, 1.0 / 3.0);
        }
    }

    #[test]
    fn 境界列挙は分割ごとに仕切り線を返す() {
        // [root | new]（Horizontal, ratio 0.5）を縦線 1 本で返す
        let (mut t, root) = tree();
        let _ = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let borders = t.borders(Rect::UNIT);
        assert_eq!(borders.len(), 1);
        let b = borders[0];
        assert_eq!(b.axis, SplitAxis::Horizontal);
        assert_eq!(b.index, 0);
        assert_close_to(b.position, 0.5); // 縦線の x 位置
        assert_close_to(b.span_start, 0.0);
        assert_close_to(b.span_end, 1.0);
    }

    #[test]
    fn ネスト分割は境界をpre_orderで列挙しindexで個別にリサイズできる() {
        // [root | (new1 / new2)]: 外側 Horizontal(index 0) + 内側 Vertical(index 1)
        let (mut t, root) = tree();
        let new1 = t
            .split(root, SplitDirection::Right, Pane::new(PaneOrigin::User))
            .unwrap();
        let new2 = t
            .split(new1, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        let borders = t.borders(Rect::UNIT);
        assert_eq!(borders.len(), 2);
        assert_eq!(borders[0].axis, SplitAxis::Horizontal);
        assert_eq!(borders[1].axis, SplitAxis::Vertical);
        // 内側の横線は右半分（x: 0.5..1.0）に走る
        assert_close_to(borders[1].span_start, 0.5);
        assert_close_to(borders[1].span_end, 1.0);

        // index 0（外側）を 0.7 へ → root が広がり、内側分割は無傷
        let set = t.set_split_ratio(0, 0.7).unwrap();
        assert_close_to(set, 0.7);
        assert_close_to(rect_of(&t, root).width, 0.7);
        assert_close_to(rect_of(&t, new2).height, 0.5);

        // index 1（内側）を 0.25 へ → new1 が縮み new2 が広がる
        t.set_split_ratio(1, 0.25).unwrap();
        assert_close_to(rect_of(&t, new1).height, 0.25);
        assert_close_to(rect_of(&t, new2).height, 0.75);

        // 無効 index は None
        assert_eq!(t.set_split_ratio(9, 0.5), None);
    }

    #[test]
    fn 座標から比率への換算はクランプされる() {
        let area = Rect::new(0.0, 0.0, 1.0, 1.0);
        // Horizontal は x を使う
        assert_close_to(
            ratio_for_position(area, SplitAxis::Horizontal, 0.3, 0.9),
            0.3,
        );
        // Vertical は y を使う
        assert_close_to(ratio_for_position(area, SplitAxis::Vertical, 0.9, 0.6), 0.6);
        // 範囲外はクランプ（MIN_SHARE..=MAX_SHARE）
        assert_close_to(
            ratio_for_position(area, SplitAxis::Horizontal, -1.0, 0.5),
            MIN_SHARE,
        );
        assert_close_to(
            ratio_for_position(area, SplitAxis::Horizontal, 2.0, 0.5),
            MAX_SHARE,
        );
        // オフセットのある領域でも相対換算
        let area2 = Rect::new(0.5, 0.0, 0.5, 1.0);
        assert_close_to(
            ratio_for_position(area2, SplitAxis::Horizontal, 0.75, 0.0),
            0.5,
        );
    }

    #[test]
    fn ペインidはユニーク() {
        let (mut t, root) = tree();
        let mut ids = vec![root];
        for _ in 0..10 {
            let id = t
                .split(
                    *ids.last().unwrap(),
                    SplitDirection::Right,
                    Pane::new(PaneOrigin::User),
                )
                .unwrap();
            ids.push(id);
        }
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), ids.len());
    }

    #[test]
    fn layoutは実寸の矩形にも対応する() {
        let (mut t, root) = tree();
        let new = t
            .split(root, SplitDirection::Down, Pane::new(PaneOrigin::User))
            .unwrap();
        let rects = t.layout(Rect::new(0.0, 0.0, 800.0, 600.0));
        let r_new = rects.iter().find(|(id, _)| *id == new).unwrap().1;
        assert_close_to(r_new.y, 300.0);
        assert_close_to(r_new.width, 800.0);
        assert_close_to(r_new.height, 300.0);
    }

    #[test]
    fn タイトルとロールを後から設定できる() {
        let (mut t, root) = tree();
        let pane = t.get_mut(root).unwrap();
        pane.set_title(Some("dev".into()));
        pane.set_role(Some("dev-server".into()));
        assert_eq!(t.get(root).unwrap().title(), Some("dev"));
        assert_eq!(t.get(root).unwrap().role(), Some("dev-server"));
    }

    // --- spawn レイアウトエンジン（Issue #165） ---

    mod spawn_layout_engine {
        use super::*;
        use crate::spawn_layout::{SpawnLayoutConfig, SpawnLayoutPolicy, WorkerLayoutAlgorithm};

        fn grid_config() -> SpawnLayoutConfig {
            SpawnLayoutConfig {
                policy: SpawnLayoutPolicy::MasterReserved,
                master_ratio: 0.5,
                algorithm: WorkerLayoutAlgorithm::Grid,
            }
        }

        fn spiral_config() -> SpawnLayoutConfig {
            SpawnLayoutConfig {
                algorithm: WorkerLayoutAlgorithm::Spiral,
                ..grid_config()
            }
        }

        /// spawn_worker + spawned_by 設定（dispatch 側の実処理と同じ手順）
        fn spawn(t: &mut PaneTree, anchor: PaneId, config: &SpawnLayoutConfig) -> PaneId {
            let id = t
                .spawn_worker(anchor, Pane::new(PaneOrigin::Mcp), config)
                .unwrap();
            t.get_mut(id).unwrap().set_spawned_by(Some(anchor));
            id
        }

        fn assert_rect(t: &PaneTree, id: PaneId, x: f32, y: f32, w: f32, h: f32) {
            let r = rect_of(t, id);
            for (actual, expected, name) in [
                (r.x, x, "x"),
                (r.y, y, "y"),
                (r.width, w, "width"),
                (r.height, h, "height"),
            ] {
                assert!(
                    (actual - expected).abs() < 1e-4,
                    "ペイン {id} の {name}: 期待 {expected} に対して実際 {actual}"
                );
            }
        }

        #[test]
        fn grid_spawn1から4のrect() {
            let (mut t, master) = tree();
            let config = grid_config();

            // 1 体: master 左半分維持、worker は右半分全面
            let w1 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 1.0);

            // 2 体: 右半分が上下に割れる。master 不変
            let w2 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.5, 0.5);

            // 3 体: 左列 2（w1/w2）+ 右列 1（w3 全高）
            let w3 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.25, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.25, 0.5);
            assert_rect(&t, w3, 0.75, 0.0, 0.25, 1.0);

            // 4 体: 十字四分割
            let w4 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.25, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.25, 0.5);
            assert_rect(&t, w3, 0.75, 0.0, 0.25, 0.5);
            assert_rect(&t, w4, 0.75, 0.5, 0.25, 0.5);

            // フォーカスは最後に spawn した worker
            assert_eq!(t.focused(), w4);
        }

        #[test]
        fn grid_close後に右領域がリフローされる() {
            let (mut t, master) = tree();
            let config = grid_config();
            let w1 = spawn(&mut t, master, &config);
            let w2 = spawn(&mut t, master, &config);
            let w3 = spawn(&mut t, master, &config);
            let w4 = spawn(&mut t, master, &config);

            // w2 を閉じてリフロー → 残り 3 体が左列 2 + 右列 1 の形へ戻る
            t.close(w2).unwrap();
            assert!(t.reflow_workers(master, config.algorithm));
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.25, 0.5);
            assert_rect(&t, w3, 0.5, 0.5, 0.25, 0.5);
            assert_rect(&t, w4, 0.75, 0.0, 0.25, 1.0);

            // さらに 2 体閉じて 1 体 → 右半分全面
            t.close(w3).unwrap();
            assert!(t.reflow_workers(master, config.algorithm));
            t.close(w4).unwrap();
            assert!(t.reflow_workers(master, config.algorithm));
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 1.0);

            // 最後の worker を閉じると領域ごと消え master が全面へ（リフローは no-op）
            t.close(w1).unwrap();
            assert!(!t.reflow_workers(master, config.algorithm));
            assert_rect(&t, master, 0.0, 0.0, 1.0, 1.0);
        }

        #[test]
        fn ユーザーペインのrectはspawnとcloseで変わらない() {
            let (mut t, master) = tree();
            let config = grid_config();
            // ユーザーが master の下に手動で開いたペイン（下半分）
            let user = t
                .split(master, SplitDirection::Down, Pane::new(PaneOrigin::User))
                .unwrap();
            let user_rect = rect_of(&t, user);

            // spawn 1→3 体 + close リフローを通してユーザーペインの矩形は不変
            let w1 = spawn(&mut t, master, &config);
            let w2 = spawn(&mut t, master, &config);
            let _w3 = spawn(&mut t, master, &config);
            assert_eq!(rect_of(&t, user), user_rect);
            // master は上半分の中で左 50% を維持し、worker 領域は右上 1/4 に収まる
            assert_rect(&t, master, 0.0, 0.0, 0.5, 0.5);
            assert_rect(&t, w1, 0.5, 0.0, 0.25, 0.25);

            t.close(w2).unwrap();
            assert!(t.reflow_workers(master, config.algorithm));
            assert_eq!(rect_of(&t, user), user_rect);
        }

        #[test]
        fn 混在サブツリーはworker領域と見なされない() {
            let (mut t, master) = tree();
            let config = grid_config();
            let w1 = spawn(&mut t, master, &config);
            // ユーザーが worker 領域内に手動でペインを開いた（w1 の右）
            let user = t
                .split(w1, SplitDirection::Right, Pane::new(PaneOrigin::User))
                .unwrap();
            let user_rect = rect_of(&t, user);
            let w1_rect = rect_of(&t, w1);

            // 次の spawn は混在領域を再構築せず、master をさらに右分割して新設する
            let w2 = spawn(&mut t, master, &config);
            assert_eq!(rect_of(&t, user), user_rect, "ユーザーペインは不変");
            assert_eq!(rect_of(&t, w1), w1_rect, "混在領域内の worker も不変");
            // master は自身の残り幅（0.5）の中で 50% を維持
            assert_rect(&t, master, 0.0, 0.0, 0.25, 1.0);
            assert_rect(&t, w2, 0.25, 0.0, 0.25, 1.0);
        }

        #[test]
        fn spiral_spawn1から4のrect() {
            let (mut t, master) = tree();
            let config = spiral_config();

            let w1 = spawn(&mut t, master, &config);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 1.0);

            // 2 体: 上下半分
            let w2 = spawn(&mut t, master, &config);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.5, 0.5);

            // 3 体: 下半分が左右に割れる
            let w3 = spawn(&mut t, master, &config);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.25, 0.5);
            assert_rect(&t, w3, 0.75, 0.5, 0.25, 0.5);

            // 4 体: 右下がさらに上下へ（縦横交互）
            let w4 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 0.5);
            assert_rect(&t, w2, 0.5, 0.5, 0.25, 0.5);
            assert_rect(&t, w3, 0.75, 0.5, 0.25, 0.25);
            assert_rect(&t, w4, 0.75, 0.75, 0.25, 0.25);
        }

        #[test]
        fn legacyポリシーは従来の右等分割() {
            let (mut t, master) = tree();
            let config = SpawnLayoutConfig {
                policy: SpawnLayoutPolicy::Legacy,
                ..grid_config()
            };
            let w1 = spawn(&mut t, master, &config);
            // 従来の spawn 比率（新ペイン側 0.45）
            assert_rect(&t, master, 0.0, 0.0, 0.55, 1.0);
            assert_rect(&t, w1, 0.55, 0.0, 0.45, 1.0);
            // 2 体目は master ではなく直前の分割先の右ではなく、同じ anchor の右
            let w2 = spawn(&mut t, master, &config);
            assert_close_to(rect_of(&t, master).width, 0.55 * 0.55);
            assert_close_to(rect_of(&t, w2).width, 0.55 * 0.45);
        }

        #[test]
        fn master_ratioが反映される() {
            let (mut t, master) = tree();
            let config = SpawnLayoutConfig {
                master_ratio: 0.7,
                ..grid_config()
            };
            let w1 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.7, 1.0);
            assert_rect(&t, w1, 0.7, 0.0, 0.3, 1.0);
            // 2 体目以降は領域内の再構築なので master_ratio は影響しない
            let _w2 = spawn(&mut t, master, &config);
            assert_rect(&t, master, 0.0, 0.0, 0.7, 1.0);
        }

        #[test]
        fn 孫workerもチェーンで領域に含まれる() {
            let (mut t, master) = tree();
            let config = grid_config();
            let w1 = spawn(&mut t, master, &config);
            // w1 が孫 worker を spawn（anchor は w1）
            let w1a = spawn(&mut t, w1, &config);
            // master 起点のリフロー: w1a も spawned_by チェーンで master に到達するため
            // 領域内に含まれ、grid で再配置される
            assert!(t.reflow_workers(master, config.algorithm));
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            assert_rect(&t, w1, 0.5, 0.0, 0.5, 0.5);
            assert_rect(&t, w1a, 0.5, 0.5, 0.5, 0.5);
        }

        #[test]
        fn 存在しないanchorはエラーまたはfalse() {
            let (mut t, master) = tree();
            let ghost = Pane::new(PaneOrigin::User).id();
            assert_eq!(
                t.spawn_worker(ghost, Pane::new(PaneOrigin::Mcp), &grid_config()),
                Err(PaneTreeError::PaneNotFound(ghost))
            );
            assert!(!t.reflow_workers(ghost, WorkerLayoutAlgorithm::Grid));
            // master に worker がいなければリフローは no-op
            assert!(!t.reflow_workers(master, WorkerLayoutAlgorithm::Grid));
        }

        #[test]
        fn grid_5体以上は列が増える() {
            let (mut t, master) = tree();
            let config = grid_config();
            let ws: Vec<PaneId> = (0..5).map(|_| spawn(&mut t, master, &config)).collect();
            // 5 体: rows=3, cols=2 → 左列 3 + 右列 2
            assert_rect(&t, master, 0.0, 0.0, 0.5, 1.0);
            let h3 = 1.0 / 3.0;
            assert_rect(&t, ws[0], 0.5, 0.0, 0.25, h3);
            assert_rect(&t, ws[1], 0.5, h3, 0.25, h3);
            assert_rect(&t, ws[2], 0.5, 2.0 * h3, 0.25, h3);
            assert_rect(&t, ws[3], 0.75, 0.0, 0.25, 0.5);
            assert_rect(&t, ws[4], 0.75, 0.5, 0.25, 0.5);
        }
    }
}

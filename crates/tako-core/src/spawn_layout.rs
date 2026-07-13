//! spawn_layout — worker spawn のペイン配置ポリシー（Issue #165、FR-2.20）
//!
//! master（spawn 元）を見やすく保ちつつ、worker を「worker 領域」
//! （spawn 由来ペインだけのサブツリー）内へ配置する。
//! ここにはポリシー・アルゴリズムの型と、worker 領域サブツリーを組み立てる
//! 純関数を置く。PaneTree への適用（`spawn_worker` / `reflow_workers`）は
//! `pane_tree.rs` 側にある。

use crate::pane::Pane;
use crate::pane_tree::{PaneNode, SplitAxis, MAX_SHARE, MIN_SHARE};

/// spawn 時のペイン配置ポリシー
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpawnLayoutPolicy {
    /// master（spawn 元）の取り分を維持し、worker は右側の worker 領域内に配置する
    #[default]
    MasterReserved,
    /// 従来挙動: spawn 元ペインの右に等分割を繰り返す（worker が増えるほど横に圧縮される）
    Legacy,
}

impl SpawnLayoutPolicy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "master-reserved" => Ok(Self::MasterReserved),
            "legacy" => Ok(Self::Legacy),
            other => Err(format!(
                "不明なレイアウトポリシー: {other}（master-reserved / legacy）"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MasterReserved => "master-reserved",
            Self::Legacy => "legacy",
        }
    }
}

/// worker 領域内の配置アルゴリズム
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerLayoutAlgorithm {
    /// 格子配置: 1 体 = 全面 → 2 体 = 上下 → 3〜4 体 = 十字四分割 → 以降は列を増やす
    #[default]
    Grid,
    /// 渦巻き配置: 先頭の worker が半分を取り、残りを縦横交互に半分ずつ再帰分割（黄金比風）
    Spiral,
}

impl WorkerLayoutAlgorithm {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "grid" => Ok(Self::Grid),
            "spiral" => Ok(Self::Spiral),
            other => Err(format!("不明な配置アルゴリズム: {other}（grid / spiral）")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Spiral => "spiral",
        }
    }
}

/// master-reserved 時に master 側へ残す既定の取り分（画面半分）
pub const DEFAULT_MASTER_RATIO: f32 = 0.5;

/// Legacy ポリシーでの新ペイン側取り分（従来の spawn 分割比率）
pub const LEGACY_WORKER_SHARE: f32 = 0.45;

/// spawn レイアウト設定（config.yaml の `spawn_layout` セクションに対応）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnLayoutConfig {
    pub policy: SpawnLayoutPolicy,
    /// master-reserved 時に master 側へ残す取り分（`MIN_SHARE`〜`MAX_SHARE`。既定 0.5）
    pub master_ratio: f32,
    pub algorithm: WorkerLayoutAlgorithm,
}

impl Default for SpawnLayoutConfig {
    fn default() -> Self {
        Self {
            policy: SpawnLayoutPolicy::default(),
            master_ratio: DEFAULT_MASTER_RATIO,
            algorithm: WorkerLayoutAlgorithm::default(),
        }
    }
}

/// master_ratio を妥当域へクランプする（非有限値は既定へ）
pub fn clamp_master_ratio(ratio: f32) -> f32 {
    if !ratio.is_finite() {
        return DEFAULT_MASTER_RATIO;
    }
    ratio.clamp(MIN_SHARE, MAX_SHARE)
}

/// worker 領域のサブツリーを構築する（in-order = spawn 順を保つ）。panes は 1 枚以上
pub(crate) fn build_worker_area(panes: Vec<Pane>, algorithm: WorkerLayoutAlgorithm) -> PaneNode {
    debug_assert!(!panes.is_empty());
    match algorithm {
        WorkerLayoutAlgorithm::Grid => build_grid(panes),
        // worker 領域は縦長（画面右側）のため、最初の分割は上下から始める
        WorkerLayoutAlgorithm::Spiral => build_spiral(panes, SplitAxis::Vertical),
    }
}

/// 格子配置。行を先に増やす（rows = ceil(sqrt(n)), cols = ceil(n / rows)）:
/// 1 → 1x1 / 2 → 上下 / 3 → 左列 2 + 右列 1 / 4 → 2x2 十字 / 5 → 左列 3 + 右列 2。
/// 列は等幅、列内は等高。余りは先頭（左）の列から埋める
fn build_grid(panes: Vec<Pane>) -> PaneNode {
    let n = panes.len();
    let rows = (n as f32).sqrt().ceil() as usize;
    let cols = n.div_ceil(rows);
    let base = n / cols;
    let extra = n % cols;
    let mut iter = panes.into_iter();
    let mut columns: Vec<PaneNode> = Vec::with_capacity(cols);
    for c in 0..cols {
        let take = base + usize::from(c < extra);
        let col: Vec<PaneNode> = iter.by_ref().take(take).map(PaneNode::Leaf).collect();
        columns.push(build_even(col, SplitAxis::Vertical));
    }
    build_even(columns, SplitAxis::Horizontal)
}

/// nodes を axis 方向へ均等比率の入れ子 Split にする（first 側の取り分 = 1/残り個数）
fn build_even(mut nodes: Vec<PaneNode>, axis: SplitAxis) -> PaneNode {
    debug_assert!(!nodes.is_empty());
    let mut node = nodes.pop().expect("nodes は 1 個以上");
    let mut count = 1usize;
    while let Some(prev) = nodes.pop() {
        count += 1;
        node = PaneNode::Split {
            axis,
            ratio: (1.0 / count as f32).clamp(MIN_SHARE, MAX_SHARE),
            first: Box::new(prev),
            second: Box::new(node),
        };
    }
    node
}

/// 渦巻き配置。先頭ペインが領域の半分を取り、残り半分を直交軸で再帰分割する
fn build_spiral(mut panes: Vec<Pane>, axis: SplitAxis) -> PaneNode {
    if panes.len() == 1 {
        return PaneNode::Leaf(panes.pop().expect("1 枚以上"));
    }
    let head = panes.remove(0);
    let next_axis = match axis {
        SplitAxis::Vertical => SplitAxis::Horizontal,
        SplitAxis::Horizontal => SplitAxis::Vertical,
    };
    PaneNode::Split {
        axis,
        ratio: 0.5,
        first: Box::new(PaneNode::Leaf(head)),
        second: Box::new(build_spiral(panes, next_axis)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ポリシーとアルゴリズムの文字列往復() {
        for p in [SpawnLayoutPolicy::MasterReserved, SpawnLayoutPolicy::Legacy] {
            assert_eq!(SpawnLayoutPolicy::parse(p.as_str()), Ok(p));
        }
        for a in [WorkerLayoutAlgorithm::Grid, WorkerLayoutAlgorithm::Spiral] {
            assert_eq!(WorkerLayoutAlgorithm::parse(a.as_str()), Ok(a));
        }
        assert!(SpawnLayoutPolicy::parse("golden").is_err());
        assert!(WorkerLayoutAlgorithm::parse("mosaic").is_err());
    }

    #[test]
    fn master_ratioのクランプ() {
        assert_eq!(clamp_master_ratio(0.5), 0.5);
        assert_eq!(clamp_master_ratio(0.05), MIN_SHARE);
        assert_eq!(clamp_master_ratio(0.95), MAX_SHARE);
        assert_eq!(clamp_master_ratio(f32::NAN), DEFAULT_MASTER_RATIO);
    }
}

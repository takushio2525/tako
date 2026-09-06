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

/// worker ペイン 1 枚に保証する最小の桁数（#1132）の既定値。
///
/// # なぜ 60 桁か
///
/// 1. **tako が読む通知が 3 行以内に収まる幅**。claude / codex の停止通知のうち最長は
///    `You hit your spend cap set by the owner of your workspace. …`（111 文字）で、
///    claude TUI の折り返し（先頭 `  ⎿  ` + 5 桁の字下げ）だと 60 桁では 3 行。
///    折り返しの結合（[`crate::limit_resume::unwrap_wrapped_lines`]）の予算
///    （`MAX_WRAP_JOIN_LINES` = 12）に十分収まり、人間も 3 行で読める。
///    実害が出た 21〜25 桁では同じ通知が 5〜7 行に割れる（#1123 の実採取）
/// 2. **master の隣に worker の列が 1 本残る上限**。実測（#1132）のタブは約 124 桁で、
///    master が半分（53 桁）・worker 領域が約 62 桁だった。60 桁ならこの領域に
///    worker 列が 1 本立つ（= master の隣に worker が見える）が、これより大きくすると
///    ノート PC では **どの worker も同タブに置けなくなる**
///
/// 大きくすれば読みやすくなるが、同タブに置ける worker は減る（残りは別タブへ出る）
pub const DEFAULT_MIN_WORKER_COLS: u16 = 60;

/// `min_worker_cols` に指定できる上限。これ以上は実質どのタブにも置けなくなる
pub const MIN_WORKER_COLS_MAX: u16 = 400;

/// `min_worker_cols` に指定できる下限（0 = 保証しない、を除く）。
/// これ未満は #1123 の実害が出た幅なので、指定させても意味が無い
pub const MIN_WORKER_COLS_FLOOR: u16 = 20;

/// `min_worker_cols` を妥当域へ寄せる。0 は「保証しない」としてそのまま通す
pub fn clamp_min_worker_cols(cols: u16) -> u16 {
    if cols == 0 {
        return 0;
    }
    cols.clamp(MIN_WORKER_COLS_FLOOR, MIN_WORKER_COLS_MAX)
}

/// spawn レイアウト設定（config.yaml の `spawn_layout` セクションに対応）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnLayoutConfig {
    pub policy: SpawnLayoutPolicy,
    /// master-reserved 時に master 側へ残す取り分（`MIN_SHARE`〜`MAX_SHARE`。既定 0.5）
    pub master_ratio: f32,
    pub algorithm: WorkerLayoutAlgorithm,
    /// worker ペイン 1 枚に保証する最小の桁数（#1132。0 = 保証しない）。
    /// 割ってしまう spawn は同じタブへ割らず別のタブへ出す
    pub min_worker_cols: u16,
}

impl Default for SpawnLayoutConfig {
    fn default() -> Self {
        Self {
            policy: SpawnLayoutPolicy::default(),
            master_ratio: DEFAULT_MASTER_RATIO,
            algorithm: WorkerLayoutAlgorithm::default(),
            min_worker_cols: DEFAULT_MIN_WORKER_COLS,
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

// --- 幅の見積もり（#1132） ---
//
// レイアウトは比率で決まるので、「worker を 1 体足したときにいちばん狭い worker が
// タブ幅の何割になるか」は純関数で分かる。桁数への変換だけが実測（セル幅・枠）に
// 依るので、そこは呼び出し側（GUI）が受け持つ。
//
// ここの式は [`build_even`] / [`build_spiral`] の**写し**なので、
// 実際に木を組んで `PaneTree::layout` と突き合わせるテストで拘束してある
// （アルゴリズムを変えたらテストが落ちる = 見積もりだけ古くなることを防ぐ）。

/// [`build_even`] が作る取り分（先頭から順に）。
/// `MIN_SHARE`..=`MAX_SHARE` のクランプまで含めて同じ値を返す
fn even_shares(count: usize) -> Vec<f32> {
    debug_assert!(count > 0);
    let mut out = Vec::with_capacity(count);
    let mut remaining = 1.0f32;
    for k in 0..count {
        if k + 1 == count {
            out.push(remaining);
            break;
        }
        let ratio = (1.0 / (count - k) as f32).clamp(MIN_SHARE, MAX_SHARE);
        let share = remaining * ratio;
        out.push(share);
        remaining -= share;
    }
    out
}

/// [`build_spiral`] が作る各ペインの**幅**の取り分（先頭から順に）。
/// 最初の分割は上下（`SplitAxis::Vertical`）なので、幅が半分になるのは 1 つ飛ばし
fn spiral_width_shares(count: usize) -> Vec<f32> {
    debug_assert!(count > 0);
    let mut out = Vec::with_capacity(count);
    let mut scale = 1.0f32;
    // build_spiral は Vertical から始める（worker 領域は縦長のため）
    let mut horizontal = false;
    for i in 0..count {
        if i + 1 == count {
            out.push(scale);
            break;
        }
        // head は分割の first 側。Horizontal（左右）なら幅が半分になる
        out.push(if horizontal { scale * 0.5 } else { scale });
        if horizontal {
            scale *= 0.5;
        }
        horizontal = !horizontal;
    }
    out
}

/// worker 領域に `count` 枚を並べたときの、各ペインの幅（**領域幅に対する比率**）。
/// 並び順は spawn 順（[`build_worker_area`] の in-order）
pub fn worker_width_shares(algorithm: WorkerLayoutAlgorithm, count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    match algorithm {
        WorkerLayoutAlgorithm::Grid => {
            // build_grid と同じ行数・列数。列は等幅（列内は等高）なので、
            // 各ペインの幅 = その列の幅
            let rows = (count as f32).sqrt().ceil() as usize;
            let cols = count.div_ceil(rows);
            let widths = even_shares(cols);
            let base = count / cols;
            let extra = count % cols;
            let mut out = Vec::with_capacity(count);
            for (c, w) in widths.iter().enumerate().take(cols) {
                let take = base + usize::from(c < extra);
                for _ in 0..take {
                    out.push(*w);
                }
            }
            out
        }
        WorkerLayoutAlgorithm::Spiral => spiral_width_shares(count),
    }
}

/// worker 領域に `count` 枚を並べたときの、いちばん狭いペインの幅（領域幅に対する比率）
pub fn narrowest_worker_width_share(algorithm: WorkerLayoutAlgorithm, count: usize) -> f32 {
    worker_width_shares(algorithm, count)
        .into_iter()
        .fold(f32::INFINITY, f32::min)
}

/// 既存の worker 領域（タブ幅に対する幅の比率と、いま入っている worker の枚数）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkerArea {
    /// 領域の幅（タブ内容領域の幅に対する比率）
    pub width: f32,
    /// いま領域に入っている worker の枚数（1 以上）
    pub count: usize,
}

/// worker を 1 体足したとき、worker ペインの中でいちばん狭くなるものの幅
/// （**タブ内容領域の幅に対する比率**）を見積もる（#1132）。
///
/// - `anchor_width`: spawn 元（master）ペインのいまの幅比
/// - `area`: すでに worker 領域があればその幅比と枚数。無ければ `None`
///   （= anchor を分割して領域が新設される）
///
/// `Legacy` は anchor を右分割するだけなので、新ペインの幅がそのまま答えになる
/// （既存の worker の矩形は変わらない）
pub fn prospective_narrowest_worker_share(
    config: &SpawnLayoutConfig,
    anchor_width: f32,
    area: Option<WorkerArea>,
) -> f32 {
    match config.policy {
        SpawnLayoutPolicy::Legacy => anchor_width * LEGACY_WORKER_SHARE,
        SpawnLayoutPolicy::MasterReserved => match area {
            Some(area) => {
                area.width * narrowest_worker_width_share(config.algorithm, area.count + 1)
            }
            // 領域の新設: anchor の取り分を master_ratio 残し、残りが領域まるごと 1 枚になる
            None => anchor_width * (1.0 - clamp_master_ratio(config.master_ratio)),
        },
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

    /// 見積もり（[`worker_width_shares`]）を**実際に組んだ木の layout** と突き合わせる。
    /// ここが落ちたら「レイアウトを変えたのに見積もりが古い」= #1132 の保証が嘘になる
    #[test]
    fn issue1132_幅の見積もりが実レイアウトと一致する() {
        use crate::pane::{Pane, PaneOrigin};
        use crate::pane_tree::{PaneTree, Rect};

        for algorithm in [WorkerLayoutAlgorithm::Grid, WorkerLayoutAlgorithm::Spiral] {
            for count in 1..=16usize {
                let panes: Vec<Pane> = (0..count).map(|_| Pane::new(PaneOrigin::Mcp)).collect();
                let ids: Vec<_> = panes.iter().map(|p| p.id()).collect();
                let root = build_worker_area(panes, algorithm);
                let tree = PaneTree::from_root(root, ids[0]);
                let layout = tree.layout(Rect::UNIT);
                let want = worker_width_shares(algorithm, count);
                assert_eq!(want.len(), count, "{algorithm:?} count={count}");
                for (i, id) in ids.iter().enumerate() {
                    let got = layout
                        .iter()
                        .find(|(pid, _)| pid == id)
                        .map(|(_, r)| r.width)
                        .unwrap_or_else(|| panic!("{algorithm:?} count={count} の {i} 番が無い"));
                    assert!(
                        (got - want[i]).abs() < 1e-4,
                        "{algorithm:?} count={count} の {i} 番: 実 {got} / 見積もり {}",
                        want[i]
                    );
                }
                let narrowest = layout
                    .iter()
                    .map(|(_, r)| r.width)
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    (narrowest_worker_width_share(algorithm, count) - narrowest).abs() < 1e-4,
                    "{algorithm:?} count={count} の最狭"
                );
            }
        }
    }

    /// 新しく足す worker が**いちばん狭い側**であること（= 新ペインだけ見れば足りる、
    /// という近道が成り立つこと）を両アルゴリズムで固定する
    #[test]
    fn issue1132_足した_worker_が最狭になる() {
        for algorithm in [WorkerLayoutAlgorithm::Grid, WorkerLayoutAlgorithm::Spiral] {
            for count in 1..=16usize {
                let shares = worker_width_shares(algorithm, count);
                let last = *shares.last().expect("1 枚以上");
                let narrowest = narrowest_worker_width_share(algorithm, count);
                assert!(
                    (last - narrowest).abs() < 1e-4,
                    "{algorithm:?} count={count}: 末尾 {last} / 最狭 {narrowest}"
                );
            }
        }
    }

    #[test]
    fn issue1132_見積もりは領域の有無で切り替わる() {
        let grid = SpawnLayoutConfig::default();
        // 領域が無い = anchor を分割して worker 領域が新設される（master_ratio の残り）
        assert!(
            (prospective_narrowest_worker_share(&grid, 1.0, None) - 0.5).abs() < 1e-4,
            "領域新設は master_ratio の残り"
        );
        // 既存領域 0.5 に 2 枚目 → grid(2) は 1 列（上下）なので幅は変わらない
        let area = WorkerArea {
            width: 0.5,
            count: 1,
        };
        assert!((prospective_narrowest_worker_share(&grid, 0.5, Some(area)) - 0.5).abs() < 1e-4);
        // 3 枚目 → grid(3) は 2 列なので半分になる
        let area = WorkerArea {
            width: 0.5,
            count: 2,
        };
        assert!((prospective_narrowest_worker_share(&grid, 0.5, Some(area)) - 0.25).abs() < 1e-4);
        // legacy は anchor の右分割なので新ペインの取り分がそのまま
        let legacy = SpawnLayoutConfig {
            policy: SpawnLayoutPolicy::Legacy,
            ..SpawnLayoutConfig::default()
        };
        assert!(
            (prospective_narrowest_worker_share(&legacy, 0.8, Some(area))
                - 0.8 * LEGACY_WORKER_SHARE)
                .abs()
                < 1e-4
        );
    }

    #[test]
    fn issue1132_min_worker_colsのクランプ() {
        assert_eq!(clamp_min_worker_cols(0), 0, "0 は保証しない");
        assert_eq!(clamp_min_worker_cols(60), 60);
        assert_eq!(clamp_min_worker_cols(1), MIN_WORKER_COLS_FLOOR);
        assert_eq!(clamp_min_worker_cols(9999), MIN_WORKER_COLS_MAX);
        assert_eq!(
            SpawnLayoutConfig::default().min_worker_cols,
            DEFAULT_MIN_WORKER_COLS
        );
    }

    #[test]
    fn master_ratioのクランプ() {
        assert_eq!(clamp_master_ratio(0.5), 0.5);
        assert_eq!(clamp_master_ratio(0.05), MIN_SHARE);
        assert_eq!(clamp_master_ratio(0.95), MAX_SHARE);
        assert_eq!(clamp_master_ratio(f32::NAN), DEFAULT_MASTER_RATIO);
    }
}

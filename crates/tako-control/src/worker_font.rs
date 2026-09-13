//! worker ペインのフォント自動縮小（Issue #1439）
//!
//! # なぜ要るか
//!
//! #1132 は worker ペイン 1 枚の下限桁数（既定 60 桁）を割る spawn を
//! **別のタブ**へ逃がしていた。狭いペインで claude TUI がハード折り返しして
//! 検知が壊れる問題（#1123）には効いていたが、「1 グループ = 1 タブで集約監視する」
//! という tako のコンセプトを壊す（master のタブから worker が見えない）。
//!
//! #1439 では**必ず同じタブに置き**、足りない桁数は worker ペインの
//! フォントを縮めて確保する。床（既定 60%）まで縮めても届かないときは
//! 床のサイズで置き、応答に `cols_short` を立てて呼び出し元へ伝える
//! （縮めないより広い桁数が入るので、届かなくても縮める）。
//!
//! # どこから呼ぶか
//!
//! worker 領域の形が変わるたびに当て直す。呼び口はこの 1 実装だけ:
//!
//! - spawn 直後（`dispatch_orchestrator_spawn`）— 応答に載せる値もここから採る
//! - worker の close 後のリフロー（dispatch `Close` と tako-app の × / exit）
//! - 復元・ウィンドウのリサイズ（tako-app が幅の実測を採り直したとき）
//!
//! **当て直しは領域まるごと**を見る。grid は worker を 1 体足すと既存の列も
//! 細くなるので、新しいペインだけ縮めても残りが下限を割ったままになる。
//! 逆に worker が閉じて幅が戻ったときは `scale = 1.0` が選ばれ、
//! ホスト側で自動縮小が外れる（= 元のサイズへ戻る）。

use tako_core::spawn_layout::{fit_worker_font, SpawnLayoutConfig, WorkerFontFit};
use tako_core::{PaneId, TabId};

use crate::host::ControlHost;

// --- A/B の腕（#1132 / #1439） ---
//
// 配置（どのタブへ置くか）とフォント（縮めるか）は**同じ方針の両輪**なので、
// 腕の判断はここに 1 つだけ置く。dispatch（spawn / close）も tako-app（復元・
// リサイズの取りこぼし）も同じ答えを見るので、腕が経路ごとにズレることが無い。

/// #1132 の A/B。`TAKO_1132_LEGACY=1` で下限幅の保証をせず、どんなに狭くなっても
/// 同じタブへ割る（= #1132 前の挙動。worker が 17〜25 桁まで潰れる）
fn legacy_worker_min_width() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1132_LEGACY").is_some())
}

/// #1439 の A/B。`TAKO_1439_LEGACY=1` で下限幅を**別のタブへ逃がして**確保する
/// （= #1132 の挙動。worker が master のタブから見えなくなる）
fn legacy_split_tabs() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1439_LEGACY").is_some())
}

/// worker の配置とフォントの腕（#1439 の A/B）。env の読み取りと**判断**を
/// 分けるので、テストから 3 腕とも決定的に回せる（`OnceLock` は途中で変えられない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementArm {
    /// 既定（#1439）: 必ず spawn 元と同じタブへ置き、足りない桁数は
    /// worker ペインのフォントを縮めて確保する
    SameTab,
    /// `TAKO_1439_LEGACY=1`（= #1132）: 下限を割るなら別のタブへ逃がす。縮めない
    SplitTabs,
    /// `TAKO_1132_LEGACY=1`（= #1132 前）: 下限を保証しない（狭いまま同じタブ）。縮めない
    NoGuarantee,
}

impl PlacementArm {
    /// 両方立っているときは「保証しない」が勝つ（いちばん古い挙動へ全部倒す）
    pub fn from_env() -> Self {
        if legacy_worker_min_width() {
            Self::NoGuarantee
        } else if legacy_split_tabs() {
            Self::SplitTabs
        } else {
            Self::SameTab
        }
    }
}

/// 1 枚ぶんの適用結果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedFit {
    pub pane: PaneId,
    pub fit: WorkerFontFit,
    /// 実際に効いている絶対フォントサイズ（pt。ホストが持たなければ None）
    pub font_size: Option<f32>,
}

/// 自動縮小が有効か（`min_worker_cols == 0` は「桁数を保証しない」= 縮めない）。
///
/// A/B の対照（#1132 = 別タブで確保する / #1132 前 = 保証しない）では
/// **どちらも縮めない**。ここで腕を見ておくことで、spawn / close / 復元・リサイズの
/// どの経路から来ても同じ答えになる（腕ごとの挙動が経路で食い違わない）
pub fn enabled(layout: &SpawnLayoutConfig) -> bool {
    layout.auto_shrink_font
        && layout.min_worker_cols > 0
        && PlacementArm::from_env() == PlacementArm::SameTab
}

/// `anchor`（master）の worker 領域に居るペインのフォントを当て直す（#1439）。
///
/// 自動縮小が無効なら、当てていた縮小を**外して**回る（設定を戻したら見た目も戻る）。
/// 幅を実測できないホスト（GUI 以外・未描画）では何もしない
pub fn refit_worker_area(
    host: &mut dyn ControlHost,
    tab: TabId,
    anchor: PaneId,
    layout: &SpawnLayoutConfig,
) -> Vec<AppliedFit> {
    // 領域のペインと、それぞれの幅比（タブ内容領域に対する比率）を先に採る
    let Some(shares) = worker_area_shares(host, tab, anchor) else {
        return Vec::new();
    };
    if !enabled(layout) {
        // 縮小を外す（`scale = 1.0` 相当）。応答には載せない
        for (pane, _) in &shares {
            host.set_worker_font_scale(*pane, None);
        }
        return Vec::new();
    }
    let min_cols = layout.min_worker_cols;
    let floor = layout.min_worker_font_scale;
    // 測る（&self）→ 当てる（&mut self）の順に分ける
    let fits: Vec<(PaneId, WorkerFontFit)> = shares
        .iter()
        .filter_map(|(pane, share)| {
            let host = &*host;
            let fit = fit_worker_font(min_cols, floor, |scale| {
                host.pane_cols_for_width_fraction(tab, *share, scale)
            })?;
            Some((*pane, fit))
        })
        .collect();
    fits.into_iter()
        .map(|(pane, fit)| {
            // 1.0 = 縮小なし。当てていたぶんは外す（幅が戻ったら元のサイズへ）
            let scale = (fit.scale < 1.0).then_some(fit.scale);
            let font_size = host.set_worker_font_scale(pane, scale);
            AppliedFit {
                pane,
                fit,
                font_size,
            }
        })
        .collect()
}

/// worker 領域のペインと、その幅比（タブ内容領域の幅に対する比率）
fn worker_area_shares(
    host: &dyn ControlHost,
    tab: TabId,
    anchor: PaneId,
) -> Option<Vec<(PaneId, f32)>> {
    let tree = host.workspace().get_tab(tab)?.tree();
    let ids = tree.worker_area_panes(anchor)?;
    let rects = tree.layout(tako_core::Rect::UNIT);
    Some(
        ids.into_iter()
            .filter_map(|id| {
                rects
                    .iter()
                    .find(|(pid, _)| *pid == id)
                    .map(|(_, r)| (id, r.width))
            })
            .collect(),
    )
}

/// タブに居る**すべての** worker 領域を当て直す（#1439。復元・リサイズ用）。
///
/// 領域の起点（master）は「spawn 由来ペインの spawned_by 先で、自分は
/// spawn 由来でないもの」。タブに master が複数居ても取りこぼさない
pub fn refit_tab(host: &mut dyn ControlHost, tab: TabId, layout: &SpawnLayoutConfig) -> usize {
    let anchors = worker_anchors(host, tab);
    anchors
        .into_iter()
        .map(|anchor| refit_worker_area(host, tab, anchor, layout).len())
        .sum()
}

/// タブ内の worker 領域の起点（= spawn 元で、自身は spawn 由来でないペイン）
fn worker_anchors(host: &dyn ControlHost, tab: TabId) -> Vec<PaneId> {
    let Some(tab_ref) = host.workspace().get_tab(tab) else {
        return Vec::new();
    };
    let panes = tab_ref.tree().panes();
    let spawned: std::collections::HashSet<PaneId> = panes
        .iter()
        .filter(|p| p.spawned_by().is_some())
        .map(|p| p.id())
        .collect();
    let mut anchors: Vec<PaneId> = panes
        .iter()
        .filter_map(|p| p.spawned_by())
        .filter(|id| !spawned.contains(id))
        .collect();
    anchors.sort_unstable_by_key(|id| id.as_u64());
    anchors.dedup();
    anchors
}

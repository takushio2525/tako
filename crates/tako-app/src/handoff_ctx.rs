//! handoff_ctx — #749 の閾値判定に使う ctx% を background で用意する（Issue #1021）
//!
//! ## なぜ要るか
//!
//! `drive_handoff_nudge` は画面（TUI フッター）の `ctx NN%` を見ている。ところが
//!
//! 1. ユーザーが `statusLine` を設定していなければ、claude の**組み込み**表示は
//!    `warn` 閾値（窓 − 33000 トークン）を超えるまで出ない = 50〜60% では画面が黙る
//! 2. master の statusline が worker 一覧で伸びると、`ctx` 行が画面走査の窓
//!    （末尾 8 行）から押し出される（本番で worker 4 体 = ちょうど 8 行を実測）
//!
//! どちらでも「master が閾値を超えても tako が気づけない」= #1021 の症状になる。
//! そこで transcript から算出したぶんを控えておき、画面が黙っていたら使う。
//!
//! ## なぜ background か
//!
//! 判定は 2 秒 tick（UI スレッド）で回る。transcript は数 MB になるので、
//! セッション解決とファイル読みを UI スレッドへ置くと #168 / #212 / #772 で
//! 潰した「画面が固まる」を作り直す。**プロセスは 1 つも起こさない**
//! （セッションカタログと transcript のファイル読みだけ）ので走査コストは低いが、
//! それでも I/O は background へ出し、間隔も空ける（ctx% はゆっくり動く）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tako_core::PaneId;

/// 読み直す間隔。ctx% は 1 ターンで数 % しか動かないので粗くてよい
/// （#749 側にも送信のクールダウンがある）
const INTERVAL: Duration = Duration::from_secs(30);

/// 間引きの状態（`CodexLimitsScan` と同じ形。#985）
#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptCtxScan {
    last: Option<Instant>,
}

impl TranscriptCtxScan {
    /// 読み直す時期か（対象が 1 つも無ければ常に false = 何も起こさない）
    pub(crate) fn due(&self, targets: usize, now: Instant) -> bool {
        targets > 0 && self.last.is_none_or(|t| now.duration_since(t) >= INTERVAL)
    }

    pub(crate) fn mark(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// background: セッションカタログ → transcript → ctx%。
///
/// カタログは**1 回だけ**読む（対象ペインごとに読み直さない）。
/// `TAKO_1021_LEGACY=1` のときは何も返さない（= #1021 前の挙動）
pub(crate) fn scan(targets: &[PaneId]) -> HashMap<PaneId, u32> {
    let mut out = HashMap::new();
    if targets.is_empty() || tako_core::ctx_usage::legacy_env() {
        return out;
    }
    let Ok(catalog) = tako_control::sessions::SessionCatalog::load() else {
        return out;
    };
    for pane in targets {
        let key = pane.as_u64().to_string();
        let Some(sid) = tako_control::sessions::resolve_session_for_pane_in(&catalog, &key) else {
            continue;
        };
        let Some(usage) = tako_control::transcript::last_context_usage(&sid) else {
            continue;
        };
        // 画面は渡さない（画面が読めているならそもそもこの経路へ来ない）
        if let Some(pct) = tako_core::ctx_usage::resolve(None, Some(&usage)).percent {
            out.insert(*pane, pct);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 対象が無ければ何も起こさない() {
        let scan = TranscriptCtxScan::default();
        let now = Instant::now();
        assert!(!scan.due(0, now), "対象ゼロでは読み直さない");
        assert!(scan.due(1, now), "初回は読む");
        assert!(super::scan(&[]).is_empty());
    }

    #[test]
    fn 間隔を空けてから読み直す() {
        let mut scan = TranscriptCtxScan::default();
        let now = Instant::now();
        scan.mark(now);
        assert!(!scan.due(3, now), "直後は読まない");
        assert!(!scan.due(3, now + INTERVAL - Duration::from_secs(1)));
        assert!(scan.due(3, now + INTERVAL), "間隔を過ぎたら読む");
    }
}

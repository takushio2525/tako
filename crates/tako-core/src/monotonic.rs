//! 単調時計（`Instant`）の巻き戻し（Issue #1627）
//!
//! ## なぜ要るか
//!
//! `Instant::now() - Duration` は**マシンの稼働時間がその `Duration` 未満のあいだ panic する**
//! （`overflow when subtracting duration from instant`）。`Instant` の起点はブートなので
//! （Windows は QueryPerformanceCounter・macOS は `CLOCK_UPTIME_RAW`）、引いた結果が
//! 起点より前になると表現できない。**OS 依存ではない**。
//!
//! 実際に踏んだ形（#1627）: `PaneMapping::new()` が「最初は必ず期限切れ」を
//! `Instant::now() - Duration::from_secs(999)` で表していたので、**ブートから 16 分 39 秒**
//! 以内に `tako remote serve` が立つと panic した。再起動直後の自動復帰（#1485）は
//! 普通に起きる並びで、CI の Windows でも uptime が閾値を跨ぐかどうかだけで結果が反転した
//! （実測: 746 秒 = panic / 1012 秒 = ok）。
//!
//! ## 書き方の選び分け
//!
//! - **「まだ一度も起きていない」** を表したいなら `Option<Instant>` の `None` を使う。
//!   期限切れの判定は `opt.is_none_or(|t| t.elapsed() > TTL)`。**時刻を捏造しない**のが正
//! - **「N 前に起きたことにする」**（自己検査でデバウンス窓を空ける等）なら [`rewound`]。
//!   巻き戻せないときは飽和して「今」を返すので panic しない

use std::time::{Duration, Instant};

/// いまから `d` だけ巻き戻した [`Instant`]。**巻き戻せないときは「今」**を返す。
///
/// 稼働時間が `d` 未満のマシンでは `Instant::now() - d` が panic する（モジュールの解説）。
/// ここは飽和させるので、呼び出し側は「少なくとも `d` 前」ではなく
/// 「**`d` 前か、起点まで遡れるところまで**」を受け取ると考えること。
///
/// 「まだ一度も起きていない」の表現には使わない（`Option<Instant>` の `None` を使う）。
/// 飽和した値は `elapsed()` が 0 に近いので、**期限切れの初期値としては逆の意味になる**
pub fn rewound(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_sub(d).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 巻き戻した分だけ経過して見える() {
        let d = Duration::from_millis(50);
        let t = rewound(d);
        // 起点まで遡れない環境（ブート直後）では飽和するので「少なくとも」では測れない。
        // **未来にならないこと**と、遡れたときは巻き戻し分が経過していることを見る
        assert!(t <= Instant::now(), "未来の時刻を返している");
        if Instant::now().checked_sub(d).is_some() {
            assert!(
                t.elapsed() >= d,
                "巻き戻せる環境なのに経過が足りない: {:?}",
                t.elapsed()
            );
        }
    }

    /// **稼働時間を超える巻き戻しでも panic しない**（#1627 の本体）。
    /// `Instant::now() - d` に戻すとこのテストが落ちる
    #[test]
    fn 稼働時間を超えて巻き戻しても飽和する() {
        for secs in [999, 86_400, 3_600_000] {
            let d = Duration::from_secs(secs);
            let t = rewound(d);
            assert!(t <= Instant::now(), "{secs} 秒の巻き戻しが未来を返した");
        }
        // 極端な値でも panic しない（飽和して「今」になる）。
        // 飽和は**実時間の絶対予算では測らない**（負荷で反転する。#1220 / #962）:
        // 呼び出しの前後で挟めば「今を返した」ことが負荷に依らず言える
        let before = Instant::now();
        let t = rewound(Duration::MAX);
        let after = Instant::now();
        assert!(t >= before && t <= after, "飽和して「今」を返していない");
    }

    #[test]
    fn ゼロの巻き戻しは今のまま() {
        let before = Instant::now();
        let t = rewound(Duration::ZERO);
        let after = Instant::now();
        assert!(t >= before && t <= after);
    }
}

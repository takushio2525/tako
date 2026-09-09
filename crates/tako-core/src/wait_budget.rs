//! 状態待ちの上限を決める**政策**（純関数。#1162 / #1252）
//!
//! フレークの一次の対策は「状態で待つ」ことだが、上限そのものも混んだ機では
//! 素直に伸ばす。その計算を**リポジトリで 1 実装**にするための置き場。
//! `tako-app` の `self_test::state_wait_budget` はここへ委譲し、
//! `tako-core` のテスト（`tmux_backend` の器 e2e など）も同じ関数を通す
//! （政策を 2 か所に書かない = `.agent/conventions.md`）。

use std::time::Duration;

/// **状態待ちの上限を機の混み具合で伸ばす**（純関数。#1162）。
///
/// **伸ばすだけで縮めない**のが肝: `busy` が読めない環境（`load=unknown` の
/// Windows 実機など）でも予算は `base` のまま残るので、判定が環境で緩くなることはない。
/// 係数は 4 倍で打ち切る（本物の回帰があるときに待ち続けないため）。
///
/// `busy` は **1 CPU あたりの混み具合**（1.0 = 全コアが埋まっている）。`None` = 取れない環境
pub fn state_wait_budget(base: Duration, busy: Option<f64>) -> Duration {
    let factor = busy
        .map(|b| (1.0 + b.max(0.0)).clamp(1.0, 4.0))
        .unwrap_or(1.0);
    base.mul_f64(factor)
}

/// **テスト専用**の混み具合の読み手（1 CPU あたりの load1。`None` = 取れない環境）。
///
/// 製品の読み手は `tako_control::platform::sysload` だが、クレートの依存の向きは
/// core ← control ← app なので **tako-core からは参照できない**。ここは
/// 「上限を決めるためだけ」の最小の読み手で、**政策**（伸ばすだけ・4 倍で打ち切り）は
/// 上の [`state_wait_budget`] の 1 実装を共有する。
///
/// 使い道は待ちの上限を決めることだけで、**判定そのものには使わない**
/// （混んでいるかどうかで合否が変わってはいけない）
#[cfg(all(test, unix))]
pub(crate) fn machine_busy() -> Option<f64> {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0)
        .max(1.0);
    let mut avg = [0f64; 3];
    // SAFETY: 要素 3 の配列へ最大 3 要素を書かせる。libc の getloadavg は
    // 書けた要素数（< 0 = 失敗）を返す
    let filled = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
    (filled >= 1).then(|| avg[0] / cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 予算は混み具合が読めなければbaseのまま() {
        assert_eq!(
            state_wait_budget(Duration::from_secs(20), None),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn 予算は縮まない() {
        // 空いていても（0.0）・負の値が来ても base 未満にはしない
        for busy in [0.0, -1.0] {
            assert_eq!(
                state_wait_budget(Duration::from_secs(20), Some(busy)),
                Duration::from_secs(20),
                "busy={busy} で縮んだ"
            );
        }
    }

    #[test]
    fn 予算は混み具合で伸びる() {
        assert_eq!(
            state_wait_budget(Duration::from_secs(20), Some(1.0)),
            Duration::from_secs(40)
        );
    }

    #[test]
    fn 予算は4倍で打ち切る() {
        assert_eq!(
            state_wait_budget(Duration::from_secs(20), Some(10.0)),
            Duration::from_secs(80)
        );
    }

    #[cfg(unix)]
    #[test]
    fn 混み具合は読めるなら非負() {
        if let Some(busy) = machine_busy() {
            assert!(busy >= 0.0, "load が負: {busy}");
        }
    }
}

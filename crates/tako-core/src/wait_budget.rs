//! 状態待ちの上限を決める**政策**（純関数。#1162 / #1252 / #1114）
//!
//! フレークの一次の対策は「状態で待つ」ことだが、上限そのものも混んだ機では
//! 素直に伸ばす。その計算を**リポジトリで 1 実装**にするための置き場で、
//! `tako-app` の `self_test::state_wait_budget` と `tako-core` のテスト
//! （`tmux_backend` / `psmux_backend` の器 e2e など）が同じ関数を通す
//! （政策を 2 か所に書かない = `.agent/conventions.md`）。

use std::time::Duration;

/// **状態待ちの上限を機の混み具合で伸ばす**（純関数。#1162）。
///
/// **伸ばすだけで縮めない**のが肝: `busy` が読めない環境でも予算は `base` のまま
/// 残るので、判定が環境で緩くなることはない。係数は 4 倍で打ち切る
/// （本物の回帰があるときに待ち続けないため）。
///
/// `busy` は **1 CPU あたりの混み具合**（1.0 = 全コアが埋まっている）
pub fn state_wait_budget(base: Duration, busy: Option<f64>) -> Duration {
    let factor = busy
        .map(|b| (1.0 + b.max(0.0)).clamp(1.0, 4.0))
        .unwrap_or(1.0);
    base.mul_f64(factor)
}

/// **1 CPU あたりの混み具合**（1.0 = 全コアが埋まっている。`None` = 取れない環境）。
///
/// 製品の読み手は `tako_control::platform::sysload` だが、クレートの依存の向きは
/// core ← control ← app なので **tako-core からは参照できない**。ここは
/// 「上限を決めるためだけ」の最小の読み手で、**政策**（伸ばすだけ・4 倍で打ち切り）は
/// [`state_wait_budget`] の 1 実装を共有する。
///
/// OS をまたいで同じ係数へ通せるよう「1.0 = 全コアが埋まっている」へ揃える
/// （unix は load1 / 論理 CPU 数、Windows は CPU 使用率 / 100）。
/// **Windows は約 [`SAMPLE_WINDOW`] ぶんブロックする**（累積カウンタの差分でしか
/// 使用率が出ないため）ので、待ちの上限を決めるときに 1 回だけ呼ぶ。
///
/// 使い道は待ちの上限を決めることだけで、**判定そのものには使わない**
/// （混んでいるかどうかで合否が変わってはいけない）
pub fn machine_busy() -> Option<f64> {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0)
        .max(1.0);
    imp::busy(cpus)
}

/// Windows で CPU 使用率を測る窓（`sysload::SAMPLE_WINDOW` と同じ理由・同じ値）
pub const SAMPLE_WINDOW: Duration = Duration::from_millis(120);

#[cfg(unix)]
mod imp {
    pub(super) fn busy(cpus: f64) -> Option<f64> {
        let mut avg = [0f64; 3];
        // getloadavg(3): 埋められた要素数を返す（負値は失敗）
        let filled = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
        (filled >= 1).then(|| avg[0] / cpus)
    }
}

#[cfg(windows)]
mod imp {
    use super::SAMPLE_WINDOW;

    /// `FILETIME`（`minwinbase.h`）。ここでは 100ns 刻みの累積時間として使う
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Filetime {
        low: u32,
        high: u32,
    }

    impl Filetime {
        fn ticks(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// 全論理 CPU を合計した idle / kernel / user の累積時間。
        /// **kernel には idle が含まれる**（`GetSystemTimes` の仕様）
        fn GetSystemTimes(idle: *mut Filetime, kernel: *mut Filetime, user: *mut Filetime) -> i32;
    }

    /// (idle, total) を 100ns 刻みで採る
    fn snapshot() -> Option<(u64, u64)> {
        let mut idle = Filetime::default();
        let mut kernel = Filetime::default();
        let mut user = Filetime::default();
        // SAFETY: どれも呼び出し側が所有する `FILETIME` 3 つへ書かせるだけ
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        // kernel は idle を含むので、total = kernel + user で全時間になる
        Some((idle.ticks(), kernel.ticks().saturating_add(user.ticks())))
    }

    /// Windows は「全コアの合計」で出るので `cpus` で割る必要は無い
    /// （`percent / 100` が既に「1.0 = 全コアが埋まっている」）
    pub(super) fn busy(_cpus: f64) -> Option<f64> {
        let (idle0, total0) = snapshot()?;
        std::thread::sleep(SAMPLE_WINDOW);
        let (idle1, total1) = snapshot()?;
        let total = total1.checked_sub(total0)?;
        let idle = idle1.saturating_sub(idle0);
        // 窓の中で 1 tick も進まなかった（時計の粒度に負けた）ときは黙って諦める
        if total == 0 {
            return None;
        }
        Some((total.saturating_sub(idle) as f64 / total as f64).clamp(0.0, 1.0))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn busy(_cpus: f64) -> Option<f64> {
        None
    }
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

    #[test]
    fn 混み具合は読めるなら非負() {
        if let Some(busy) = machine_busy() {
            assert!(busy >= 0.0, "混み具合が負: {busy}");
        }
    }
}

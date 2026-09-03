//! マシンの混み具合（境界 B25。**診断専用**。#796 / #1073）
//!
//! セルフテストの失敗ログに「そのときマシンがどれだけ混んでいたか」を残すためだけの
//! 観測値で、製品の判断には使わない。「同じコードなのに回によって落ちる項目が
//! 変わる」の切り分けに要る（実測: load 6〜16 の帯で落ちる項目が入れ替わる）。
//!
//! **OS が実際に持っている指標だけを返す**。1 / 5 / 15 分の load average は unix の
//! 概念で Windows には無いので、3 つ組をでっち上げず「短い窓の CPU 使用率」を返す
//! （#982 の「過大にも過小にも申告しない」と同じ方針）。Windows で `None` を返し
//! 続けていたため、実機では 1 か月ぶん `load=unknown` しか残らず**負荷依存の
//! フレークを切り分ける材料が無かった**（#1073）。

/// マシンの混み具合。表記は [`crate::diag::format_machine_load`] が持つ
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MachineLoad {
    /// unix: `getloadavg(3)` の 1 / 5 / 15 分平均
    Average([f64; 3]),
    /// Windows: 短い窓で測った CPU 使用率（%）と論理 CPU 数。
    /// 100% は「全コアが埋まっている」= unix の load ≒ コア数 に相当する
    CpuBusy { percent: f64, cpus: u32 },
}

/// いまの混み具合を採る。取得できなければ `None`。
///
/// **Windows は約 [`SAMPLE_WINDOW`] ぶんブロックする**（累積カウンタの差分でしか
/// 使用率が出ないため）。診断 1 行を作るときにしか呼ばない前提の関数で、
/// 高頻度の経路から呼んではいけない
pub fn sample() -> Option<MachineLoad> {
    imp::sample()
}

/// Windows で CPU 使用率を測る窓。短いほど「いまの混み具合」に近いが、
/// 短すぎると tick の粒度（既定 15.6ms）で量子化される
pub const SAMPLE_WINDOW: std::time::Duration = std::time::Duration::from_millis(120);

#[cfg(unix)]
mod imp {
    use super::MachineLoad;

    pub(super) fn sample() -> Option<MachineLoad> {
        let mut out = [0f64; 3];
        // getloadavg(3): 埋められた要素数を返す（負値は失敗）
        let filled = unsafe { libc::getloadavg(out.as_mut_ptr(), 3) };
        (filled == 3).then_some(MachineLoad::Average(out))
    }
}

#[cfg(windows)]
mod imp {
    use super::{MachineLoad, SAMPLE_WINDOW};

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
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        // kernel は idle を含むので、total = kernel + user で全時間になる
        Some((idle.ticks(), kernel.ticks().saturating_add(user.ticks())))
    }

    pub(super) fn sample() -> Option<MachineLoad> {
        let (idle0, total0) = snapshot()?;
        std::thread::sleep(SAMPLE_WINDOW);
        let (idle1, total1) = snapshot()?;
        let total = total1.checked_sub(total0)?;
        let idle = idle1.saturating_sub(idle0);
        // 窓の中で 1 tick も進まなかった（時計の粒度に負けた）ときは黙って諦める
        if total == 0 {
            return None;
        }
        let busy = total.saturating_sub(idle) as f64 / total as f64 * 100.0;
        Some(MachineLoad::CpuBusy {
            percent: busy.clamp(0.0, 100.0),
            cpus: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実測できる環境では**必ず**採れること（`load=unknown` が既定にならない）。
    /// unix / Windows のどちらでも走る
    #[test]
    fn この環境の混み具合が採れて値域に収まる() {
        let load = sample().expect("この OS には対応する指標がある");
        match load {
            MachineLoad::Average(values) => {
                for value in values {
                    assert!(value.is_finite() && value >= 0.0, "load が異常: {values:?}");
                }
            }
            MachineLoad::CpuBusy { percent, cpus } => {
                assert!(
                    percent.is_finite() && (0.0..=100.0).contains(&percent),
                    "使用率が値域外: {percent}"
                );
                assert!(cpus >= 1, "論理 CPU 数が採れていない: {cpus}");
            }
        }
    }
}

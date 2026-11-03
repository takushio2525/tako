//! spawn の時点の機の余裕（純関数。FR-2.43.16 / Issue #1997。エピック #1992 の S5）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§9.1・§9.8）。
//!
//! 起動待ち（#2413）は「いま起こすか待たせるか」を決め、順番待ち（#1994）は「いま重い処理を入れるか」を
//! 決める。ここは**待たせるほどではないが余裕が少ない**ことを master へ伝える助言（spawn / run の応答の
//! `advice`）の判断だけを持つ。**閾値は持たない**: 「この 1 本を起こした後にもう 1 本起こしたら」
//! 「いま重い処理を 1 件受け付けたら」を、**同じ [`admit`]**（agent は [`launch_input`] の入力）の待ちの
//! 最後に仮の 1 件を並べて問う（[`probe`]）。政策を 2 か所に書かない。
//!
//! 文（[`Advice::text_in`]）は表示言語で組む。日本語の理由は既存の 1 文（[`launch_reason_text`] /
//! [`WaitReason::text`]）をそのまま使い、英語は理由の型の値から組む

use crate::i18n::Lang;

use super::{
    admit, launch_input, launch_reason_text, AdmitInput, Limits, MachineView, Priority, RunningJob,
    WaitReason, WaitingJob, GIB_MB,
};

/// 仮の 1 件の受付番号（実在の受付番号 = wrapper の pid / ペイン番号と重ならない）
pub const PROBE_ID: u64 = u64::MAX;

/// 待ちの最後に見込み `estimate_mb` の仮の 1 件（段は normal・受付はいま = 同じ段の最後）を並べたら
/// [`admit`] が入れるか。入れなければその件を止めている理由（前に並んでいて待つなら、並びを止めている理由）
pub fn probe(input: &AdmitInput<'_>, estimate_mb: u64) -> Option<WaitReason> {
    let mut waiting: Vec<WaitingJob> = input.waiting.to_vec();
    waiting.push(WaitingJob {
        id: PROBE_ID,
        estimate_mb,
        target_dir: None,
        priority: Priority::Normal,
        enqueued_at_ms: u64::MAX,
        overtaken: 0,
    });
    let probed = AdmitInput {
        machine: input.machine,
        limits: input.limits,
        running: input.running,
        waiting: &waiting,
        unmanaged: input.unmanaged,
        regathering: input.regathering,
        overtake: input.overtake,
        release_after_peak: input.release_after_peak,
        min_progress: input.min_progress,
    };
    let admission = admit(&probed);
    if admission.grants.contains(&PROBE_ID) {
        return None;
    }
    admission.holding_of(PROBE_ID).cloned()
}

/// この 1 本を起こした後（`starting` に含めて渡す）に、同じ見込みの agent をもう 1 本起こそうとしたら
/// 起動待ち（[`super::admit_launches`]）が待たせるか。待たせるならその理由
pub fn next_launch(
    machine: Option<MachineView>,
    limits: Limits,
    starting: &[RunningJob],
    estimate_mb: u64,
) -> Option<WaitReason> {
    probe(&launch_input(machine, limits, starting, &[]), estimate_mb)
}

/// spawn / run の応答の助言の 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advice {
    /// この 1 本は起こしたが、もう 1 本起こすと起動待ちになる（#2413 の待つ条件の手前）
    NextLaunchWaits { reason: WaitReason },
    /// いま重い処理を 1 件受け付けると待つ（#1994 の順番待ち。`running` / `waiting` は順番待ちの件数）
    HeavyQueueWaits {
        reason: WaitReason,
        running: u32,
        waiting: u32,
    },
}

impl Advice {
    /// 起動待ちの側の助言。調停役の立ち上げ直後（一時の状態）は出さない
    pub fn next_launch(reason: Option<WaitReason>) -> Option<Self> {
        reason
            .filter(|r| *r != WaitReason::Regather)
            .map(|reason| Self::NextLaunchWaits { reason })
    }

    /// 順番待ちの側の助言。調停役の立ち上げ直後（一時の状態）は出さない
    pub fn heavy_queue(reason: Option<WaitReason>, running: u32, waiting: u32) -> Option<Self> {
        reason
            .filter(|r| *r != WaitReason::Regather)
            .map(|reason| Self::HeavyQueueWaits {
                reason,
                running,
                waiting,
            })
    }

    /// 応答の `code`
    pub fn code(&self) -> &'static str {
        match self {
            Self::NextLaunchWaits { .. } => "next_launch_waits",
            Self::HeavyQueueWaits { .. } => "heavy_queue_waits",
        }
    }

    /// 理由の型（応答の `reason`）
    pub fn reason(&self) -> &WaitReason {
        match self {
            Self::NextLaunchWaits { reason } | Self::HeavyQueueWaits { reason, .. } => reason,
        }
    }

    /// 人が読む 1 文（表示言語）
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::NextLaunchWaits { reason }, Lang::Ja) => format!(
                "もう 1 本 agent を起こすと起動待ちになる見込み（{}）。急がない依頼は、走っている worker が終わってから spawn する",
                launch_reason_text(reason)
            ),
            (Self::NextLaunchWaits { reason }, Lang::En) => format!(
                "Launching one more agent now would be held ({}). Spawn non-urgent work after a running worker finishes",
                reason_en(reason, true)
            ),
            (
                Self::HeavyQueueWaits {
                    reason,
                    running,
                    waiting,
                },
                Lang::Ja,
            ) => format!(
                "いま重い処理を順番待ちへ入れると待つ見込み（{}・実行 {running}・待ち {waiting}）。この worker のビルド・テストは `tako queue` で順番を待つ",
                reason.text()
            ),
            (
                Self::HeavyQueueWaits {
                    reason,
                    running,
                    waiting,
                },
                Lang::En,
            ) => format!(
                "A heavy job queued now would wait ({}; running {running}, waiting {waiting}). This worker's builds and tests will wait their turn in `tako queue`",
                reason_en(reason, false)
            ),
        }
    }
}

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / GIB_MB as f64)
}

/// 待つ理由の英語の 1 文（`launch` = agent の起動の言葉。日本語は [`launch_reason_text`] / [`WaitReason::text`]）
fn reason_en(reason: &WaitReason, launch: bool) -> String {
    match reason {
        WaitReason::Regather => {
            "the arbiter just started and is re-registering running jobs".into()
        }
        WaitReason::TargetDir if launch => {
            "restores start one at a time (waiting for the previous agent to start)".into()
        }
        WaitReason::TargetDir => "a job using the same target dir is running".into(),
        WaitReason::PressureCritical if launch => {
            "memory pressure critical (agent launches are held)".into()
        }
        WaitReason::PressureCritical => "memory pressure critical (new jobs are held)".into(),
        WaitReason::PressureWarn if launch => {
            "memory pressure warn (no agent launches until it is back to normal)".into()
        }
        WaitReason::PressureWarn => {
            "memory pressure warn (one at a time after running jobs finish)".into()
        }
        WaitReason::Swap { used_mb } if launch => format!(
            "swap in use {} (no agent launches until it drops below the guard line)",
            gb(*used_mb)
        ),
        WaitReason::Swap { used_mb } => format!(
            "swap in use {} (one at a time after running jobs finish)",
            gb(*used_mb)
        ),
        WaitReason::MaxJobs {
            running,
            unmanaged,
            max,
        } => {
            if *unmanaged > 0 {
                format!(
                    "the concurrent job limit is reached (running {running} + outside tako {unmanaged} / limit {max})"
                )
            } else {
                format!("the concurrent job limit is reached (running {running} / limit {max})")
            }
        }
        WaitReason::Memory {
            available_mb,
            need_mb,
            reserve_mb,
            pending_mb,
            estimate_mb,
        } => {
            let breakdown = if *pending_mb > 0 {
                let starting = if launch {
                    "not yet used by starting agents"
                } else {
                    "not yet used by running jobs"
                };
                let own = if launch { "this agent" } else { "this job" };
                format!(
                    " = {} {starting} + {} for {own}",
                    gb(*pending_mb),
                    gb(*estimate_mb)
                )
            } else {
                String::new()
            };
            format!(
                "not enough free memory (free {}, needed {}{breakdown}, reserve {})",
                gb(*available_mb),
                gb(*need_mb),
                gb(*reserve_mb)
            )
        }
        WaitReason::Ahead if launch => "behind launches already in line".into(),
        WaitReason::Ahead => "behind jobs already in line".into(),
        WaitReason::OvertakeLimit { max } => {
            format!("a job waiting for the same target dir reached the overtake limit ({max})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        admit_launches, effective_limits, LimitSettings, AGENT_ESTIMATE_MB, BUILD_ESTIMATE_MB,
    };
    use super::*;
    use crate::resources::Pressure;

    fn limits_36gb() -> Limits {
        effective_limits(&LimitSettings::default(), Some(36 * GIB_MB))
    }

    fn machine(available_mb: u64, pressure: Pressure) -> Option<MachineView> {
        Some(MachineView {
            total_mb: 36 * GIB_MB,
            available_mb: Some(available_mb),
            pressure: Some(pressure),
            swap_used_mb: Some(0),
        })
    }

    /// 起こした直後の 1 本（走査がまだ = 見込みを丸ごとまだ使っていない）
    fn just_started(id: u64) -> RunningJob {
        RunningJob {
            id,
            estimate_mb: AGENT_ESTIMATE_MB,
            used_mb: None,
            ..RunningJob::default()
        }
    }

    fn agent_wait(id: u64) -> WaitingJob {
        WaitingJob {
            id,
            estimate_mb: AGENT_ESTIMATE_MB,
            target_dir: None,
            priority: Priority::Normal,
            enqueued_at_ms: 1,
            overtaken: 0,
        }
    }

    /// 境界: この 1 本を起こした後、空き = 予約 + 立ち上がり中の見込み + もう 1 本の見込み なら助言なし、
    /// 1 MB 欠けると空きの予約割れ。判定は本物の 2 本目を起動待ちへ問うたとき（[`admit_launches`]）と
    /// 空きのどの値でも一致する = 閾値を 2 か所に持たない
    #[test]
    fn issue1997_もう1本の判定は起動待ちと同じ境界() {
        let limits = limits_36gb();
        let starting = [just_started(7)];
        let edge = limits.reserve_mb + 2 * AGENT_ESTIMATE_MB;
        assert_eq!(
            next_launch(
                machine(edge, Pressure::Normal),
                limits,
                &starting,
                AGENT_ESTIMATE_MB
            ),
            None
        );
        let short = next_launch(
            machine(edge - 1, Pressure::Normal),
            limits,
            &starting,
            AGENT_ESTIMATE_MB,
        );
        assert_eq!(
            short.as_ref().map(WaitReason::code),
            Some("memory"),
            "{short:?}"
        );
        for available in (edge - 3 * AGENT_ESTIMATE_MB..edge + 2 * AGENT_ESTIMATE_MB).step_by(7) {
            let m = machine(available, Pressure::Normal);
            let real = admit_launches(m, limits, &starting, &[agent_wait(8)]);
            let probed = next_launch(m, limits, &starting, AGENT_ESTIMATE_MB);
            assert_eq!(
                probed.is_none(),
                real.grants == [8],
                "空き {available} MB で助言（{probed:?}）と起動待ち（{real:?}）が食い違う"
            );
            if let Some(reason) = probed {
                assert_eq!(Some(&reason), real.holding_of(8), "{available}");
            }
        }
    }

    #[test]
    fn issue1997_もう1本の判定はwarnとswapでも起動待ちと同じ() {
        let limits = limits_36gb();
        let warn = next_launch(
            machine(30 * GIB_MB, Pressure::Warn),
            limits,
            &[],
            AGENT_ESTIMATE_MB,
        );
        assert_eq!(warn, Some(WaitReason::PressureWarn));
        let swapped = Some(MachineView {
            swap_used_mb: Some(limits.swap_guard_mb),
            ..machine(30 * GIB_MB, Pressure::Normal).unwrap()
        });
        assert_eq!(
            next_launch(swapped, limits, &[], AGENT_ESTIMATE_MB).map(|r| r.code()),
            Some("swap")
        );
        // 機の値が採れない（Windows は #2000 まで）: 起動待ちも時差だけなので助言も出さない
        assert_eq!(
            next_launch(None, limits, &[just_started(1)], AGENT_ESTIMATE_MB),
            None
        );
    }

    fn heavy(id: u64) -> RunningJob {
        RunningJob {
            id,
            estimate_mb: BUILD_ESTIMATE_MB,
            used_mb: Some(BUILD_ESTIMATE_MB),
            peak_mb: BUILD_ESTIMATE_MB,
            ..RunningJob::default()
        }
    }

    fn heavy_input<'a>(
        machine: Option<MachineView>,
        running: &'a [RunningJob],
        waiting: &'a [WaitingJob],
        regathering: bool,
    ) -> AdmitInput<'a> {
        AdmitInput {
            machine,
            limits: limits_36gb(),
            running,
            waiting,
            unmanaged: 0,
            regathering,
            overtake: true,
            release_after_peak: true,
            min_progress: true,
        }
    }

    #[test]
    fn issue1997_重い処理の判定は同時数と前の待ちとメモリで止まる() {
        let free = machine(30 * GIB_MB, Pressure::Normal);
        // 36 GB の既定の同時数は 3。2 件なら入る・3 件なら同時数の上限
        let two = [heavy(1), heavy(2)];
        assert_eq!(
            probe(&heavy_input(free, &two, &[], false), BUILD_ESTIMATE_MB),
            None
        );
        let three = [heavy(1), heavy(2), heavy(3)];
        let full = probe(&heavy_input(free, &three, &[], false), BUILD_ESTIMATE_MB);
        assert_eq!(full.as_ref().map(WaitReason::code), Some("max_jobs"));
        // 前にメモリで待つ件が居れば、仮の 1 件はその理由で待つ（前に並んでいるだけ = Ahead を解いた理由）
        let tight = machine(5 * GIB_MB, Pressure::Normal);
        let waiting = [WaitingJob {
            estimate_mb: 7 * GIB_MB,
            ..agent_wait(9)
        }];
        let behind = probe(
            &heavy_input(tight, &two, &waiting, false),
            BUILD_ESTIMATE_MB,
        );
        assert_eq!(behind.as_ref().map(WaitReason::code), Some("memory"));
        // 立ち上げ直後は理由が Regather（一時の状態）= 助言にしない
        let regather = probe(&heavy_input(free, &[], &[], true), BUILD_ESTIMATE_MB);
        assert_eq!(regather, Some(WaitReason::Regather));
        assert_eq!(Advice::heavy_queue(regather, 0, 0), None);
        assert!(Advice::heavy_queue(full, 3, 0).is_some());
    }

    #[test]
    fn issue1997_助言の文は表示言語で理由の値を持つ() {
        let memory = WaitReason::Memory {
            available_mb: 4 * GIB_MB,
            need_mb: 2 * AGENT_ESTIMATE_MB,
            reserve_mb: 3 * GIB_MB,
            pending_mb: AGENT_ESTIMATE_MB,
            estimate_mb: AGENT_ESTIMATE_MB,
        };
        let a = Advice::next_launch(Some(memory.clone())).unwrap();
        assert_eq!(
            (a.code(), a.reason().code()),
            ("next_launch_waits", "memory")
        );
        let ja = a.text_in(Lang::Ja);
        assert!(ja.contains(&launch_reason_text(&memory)), "{ja}");
        let en = a.text_in(Lang::En);
        assert!(
            en.contains("free 4.0 GB") && en.contains("starting agents") && en.is_ascii(),
            "{en}"
        );
        let h = Advice::heavy_queue(
            Some(WaitReason::MaxJobs {
                running: 3,
                unmanaged: 1,
                max: 3,
            }),
            3,
            2,
        )
        .unwrap();
        assert_eq!(h.code(), "heavy_queue_waits");
        assert!(h.text_in(Lang::Ja).contains("実行 3・待ち 2"));
        let en = h.text_in(Lang::En);
        assert!(
            en.contains("outside tako 1") && en.contains("running 3, waiting 2") && en.is_ascii(),
            "{en}"
        );
        // 英語の文に日本語が混ざらない（理由の全種）
        for r in [
            WaitReason::Regather,
            WaitReason::TargetDir,
            WaitReason::PressureCritical,
            WaitReason::PressureWarn,
            WaitReason::Swap { used_mb: 2048 },
            WaitReason::Ahead,
            WaitReason::OvertakeLimit { max: 6 },
        ] {
            for launch in [true, false] {
                let t = reason_en(&r, launch);
                assert!(t.is_ascii() && !t.is_empty(), "{r:?}: {t}");
            }
        }
    }
}

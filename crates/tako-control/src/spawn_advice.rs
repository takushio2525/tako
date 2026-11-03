//! spawn / run の応答の `machine` と `advice`（FR-2.43.16 / Issue #1997。エピック #1992 の S5）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§9.1・§9.8）。
//!
//! master が spawn の時点で機の余裕を知る口。逼迫しているあいだの起動待ちは #2413（[`crate::launch_gate`]）
//! が持ち、ここは**助言と見える化だけ**（spawn を止めない・待たせない）:
//!
//! - `machine`: 起動待ちの判断に使う機の値（メモリ圧・空き・swap。注入があれば注入）・生きている agent の
//!   本数と常駐の合計（走査の覚え）・順番待ちの実行と待ちの件数・起動待ちの待ちと立ち上がり中の件数
//! - `advice`: 待つほどではないが余裕が少ないときの助言（[`Advice`]）。閾値は持たず、起動待ちと順番待ちの
//!   **同じ `admit`** に仮の 1 件を並べて問う（`tako_core::queue::headroom`）
//!
//! **採らない**（UI スレッドの dispatch の spawn が呼ぶ。覚えを読むだけ）。番犬は
//! `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs` の 9 番

use serde_json::{json, Value};
use tako_core::i18n::Lang;
use tako_core::queue::headroom::Advice;
use tako_core::queue::MachineView;

use crate::launch_gate::{self, Headroom, Mode};
use crate::queue::HeavyHeadroom;
use crate::resources::PaneResident;

/// 生きている agent の本数と常駐の合計（走査の覚えから）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentsResident {
    pub count: u32,
    /// agent 本体 + 補助プロセス（MCP サーバー等）の合計（MiB）
    pub memory_mb: u64,
    /// 走査の古さ（秒）
    pub age_s: u64,
}

impl AgentsResident {
    /// ペインごとの覚えから（純関数）。agent の居ないペインは数えない
    pub fn from_panes<'a>(panes: impl IntoIterator<Item = &'a PaneResident>, age_s: u64) -> Self {
        let (count, memory_mb) = panes
            .into_iter()
            .filter(|p| p.agent.is_some())
            .fold((0u32, 0u64), |(n, mb), p| (n + 1, mb + p.agent_mb));
        Self {
            count,
            memory_mb,
            age_s,
        }
    }
}

/// 機の値が無いときの理由（表示言語）
fn unavailable_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Ja => "機の値がまだ無い（この OS では採れない = Windows は #2000、または採取の前）",
        Lang::En => "Machine values are not available yet (not collected on this OS = Windows is #2000, or before the first sample)",
    }
}

fn machine_fields(machine: Option<&MachineView>, injected: bool, lang: Lang) -> Value {
    match machine {
        Some(m) => json!({
            "pressure": m.pressure.map(|p| p.as_str()),
            "total_mb": m.total_mb,
            "available_mb": m.available_mb,
            "swap_used_mb": m.swap_used_mb,
            "source": if injected { "injected" } else { "measured" },
        }),
        None => json!({
            "pressure": Value::Null,
            "total_mb": Value::Null,
            "available_mb": Value::Null,
            "swap_used_mb": Value::Null,
            "unavailable": unavailable_text(lang),
        }),
    }
}

/// 応答の `machine` と `advice` を組む（純関数）。`launch_waiting` = この spawn が起動待ちになった
/// （もう 1 本の助言は `launch_gate` が言い尽くしているので出さない）
pub fn build(
    gate: &Headroom,
    heavy: Option<&HeavyHeadroom>,
    agents: Option<AgentsResident>,
    launch_waiting: bool,
    lang: Lang,
) -> (Value, Value) {
    let mut machine = machine_fields(gate.machine.as_ref(), gate.injected, lang);
    machine["reserve_mb"] = json!(gate.reserve_mb);
    machine["agents"] = match agents {
        Some(a) => json!({
            "count": a.count,
            "memory_mb": a.memory_mb,
            "age_s": a.age_s,
        }),
        None => Value::Null,
    };
    machine["queue"] = match heavy {
        Some(h) => json!({ "running": h.running, "waiting": h.waiting }),
        None => Value::Null,
    };
    machine["launches"] = match gate.mode {
        Some(m) if !matches!(m, Mode::Off(_)) => json!({
            "waiting": gate.waiting,
            "starting": gate.starting,
            "estimate_mb": gate.estimate_mb,
        }),
        _ => Value::Null,
    };
    let mut advice: Vec<Advice> = Vec::new();
    if !launch_waiting {
        advice.extend(Advice::next_launch(gate.next_launch.clone()));
    }
    if let Some(h) = heavy {
        advice.extend(Advice::heavy_queue(h.next.clone(), h.running, h.waiting));
    }
    let advice: Vec<Value> = advice
        .iter()
        .map(|a| {
            json!({
                "code": a.code(),
                "reason": a.reason().code(),
                "text": a.text_in(lang),
            })
        })
        .collect();
    (machine, Value::Array(advice))
}

/// spawn の直後に呼ぶ（UI スレッド。**採らない**）。`agent` は起こした系統（見込みの過去の記録を引く）
pub fn for_spawn(agent: &str, launch_waiting: bool, lang: Lang) -> (Value, Value) {
    let gate = launch_gate::headroom(agent);
    let heavy = crate::queue::heavy_headroom();
    let agents = crate::resources::pane_resident(None)
        .map(|(at, panes)| AgentsResident::from_panes(panes.values(), at.elapsed().as_secs()));
    build(&gate, heavy.as_ref(), agents, launch_waiting, lang)
}

/// 助言を人が読む行（CLI の `orchestrator run` が stderr へ出す）。無ければ空
pub fn advice_lines(advice: &Value) -> Vec<String> {
    advice
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["text"].as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::queue::WaitReason;
    use tako_core::resources::Pressure;

    fn gate(next: Option<WaitReason>) -> Headroom {
        Headroom {
            mode: Some(Mode::On),
            machine: Some(MachineView {
                total_mb: 36 * 1024,
                available_mb: Some(8 * 1024),
                pressure: Some(Pressure::Normal),
                swap_used_mb: Some(0),
            }),
            injected: false,
            waiting: 0,
            starting: 1,
            reserve_mb: 3686,
            estimate_mb: 640,
            next_launch: next,
        }
    }

    #[test]
    fn issue1997_余裕があれば助言は空で機の値は出す() {
        let (m, a) = build(
            &gate(None),
            Some(&HeavyHeadroom {
                running: 1,
                waiting: 0,
                next: None,
            }),
            Some(AgentsResident {
                count: 3,
                memory_mb: 1800,
                age_s: 4,
            }),
            false,
            Lang::Ja,
        );
        assert_eq!(a, json!([]));
        assert_eq!(m["pressure"], "normal");
        assert_eq!(m["available_mb"], 8 * 1024);
        assert_eq!(m["source"], "measured");
        assert_eq!(
            m["agents"],
            json!({ "count": 3, "memory_mb": 1800, "age_s": 4 })
        );
        assert_eq!(m["queue"], json!({ "running": 1, "waiting": 0 }));
        assert_eq!(
            m["launches"],
            json!({ "waiting": 0, "starting": 1, "estimate_mb": 640 })
        );
        assert_eq!(m["reserve_mb"], 3686);
    }

    #[test]
    fn issue1997_助言は起動待ちと順番待ちの理由を持ち_待った起動にはもう1本を言わない() {
        let heavy = HeavyHeadroom {
            running: 3,
            waiting: 2,
            next: Some(WaitReason::MaxJobs {
                running: 3,
                unmanaged: 0,
                max: 3,
            }),
        };
        let (_, a) = build(
            &gate(Some(WaitReason::PressureWarn)),
            Some(&heavy),
            None,
            false,
            Lang::En,
        );
        let codes: Vec<(&str, &str)> = a
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["code"].as_str().unwrap(), x["reason"].as_str().unwrap()))
            .collect();
        assert_eq!(
            codes,
            [
                ("next_launch_waits", "pressure_warn"),
                ("heavy_queue_waits", "max_jobs")
            ]
        );
        assert!(a[0]["text"].as_str().unwrap().is_ascii(), "{a}");
        assert_eq!(advice_lines(&a).len(), 2);
        // この spawn 自体が起動待ち = もう 1 本の助言は出さない（launch_gate が言い尽くしている）
        let (_, a) = build(
            &gate(Some(WaitReason::PressureWarn)),
            Some(&heavy),
            None,
            true,
            Lang::Ja,
        );
        assert_eq!(a.as_array().unwrap().len(), 1);
        assert_eq!(a[0]["code"], "heavy_queue_waits");
    }

    #[test]
    fn issue1997_機の値が無い_調停役でない_起動待ちが無効なら欄はnull() {
        let off = Headroom {
            mode: Some(Mode::Off("A/B")),
            machine: None,
            injected: false,
            waiting: 0,
            starting: 0,
            reserve_mb: 2048,
            estimate_mb: 640,
            next_launch: None,
        };
        let (m, a) = build(&off, None, None, false, Lang::En);
        assert_eq!(a, json!([]));
        assert!(m["pressure"].is_null() && m["queue"].is_null() && m["launches"].is_null());
        assert!(m["agents"].is_null());
        assert!(m["unavailable"].as_str().unwrap().is_ascii());
        let (m, _) = build(&off, None, None, false, Lang::Ja);
        assert!(m["unavailable"].as_str().unwrap().contains("#2000"));
    }

    #[test]
    fn issue1997_agentの本数と常駐はagentの居るペインだけ() {
        let panes = [
            PaneResident {
                memory_mb: 900,
                agent_mb: 700,
                heavy_mb: 0,
                agent: Some("claude"),
            },
            PaneResident {
                memory_mb: 40,
                agent_mb: 0,
                heavy_mb: 0,
                agent: None,
            },
            PaneResident {
                memory_mb: 4000,
                agent_mb: 500,
                heavy_mb: 3400,
                agent: Some("codex"),
            },
        ];
        assert_eq!(
            AgentsResident::from_panes(&panes, 7),
            AgentsResident {
                count: 2,
                memory_mb: 1200,
                age_s: 7
            }
        );
    }
}

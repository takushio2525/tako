//! 順番待ちの優先度の既定とエイジング（純関数。FR-2.43.14 / Issue #1996。エピック #1992 の S4）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§5・§4.10）。
//!
//! ## 段と既定
//!
//! | 段 | 既定で付くもの |
//! |---|---|
//! | `top` | 人が直接（役割の無い素のシェルのペインで、端末から打った） |
//! | `high` | master / solo |
//! | `normal` | worker（と、役割の無いペインで端末の無い呼び出し = 背景のスクリプト等） |
//! | `low` | ペインの外（launchd の夜間リリース・別の端末・CI スクリプト） |
//!
//! **付ける場所の優先**: 処理（受付の `--priority` / env `TAKO_QUEUE_PRIORITY` / 後からの
//! `tako queue priority <受付番号>`）> ペイン（`tako queue priority --pane N`。layout.json のペインに残る）
//! > 役割の既定（上の表）。[`resolve`] がこの順で決める。
//!
//! ## エイジング
//!
//! 待ち [`AGING_STEP_MS`]（15 分）ごとに 1 段上げる（[`aged`]）。上げるのは `high` まで
//! （`top` へは自動で上げない = 人が直接打ったものの前へ AI の処理が割り込まない）。
//! 同じ段の中では古いほうが必ず同じか多く上がるので、**同じ段の受付の順は崩れない**。
//! 横取りはしない（走っている処理は止めない。効くのは「次に誰を入れるか」だけ）

use serde::{Deserialize, Serialize};

use super::Priority;

/// 1 段上げるまでの待ち（15 分）
pub const AGING_STEP_MS: u64 = 15 * 60 * 1000;

impl Priority {
    /// 1 段上（`top` はそのまま）
    pub fn raised(self) -> Self {
        match self {
            Self::Low => Self::Normal,
            Self::Normal => Self::High,
            Self::High | Self::Top => Self::Top,
        }
    }
}

/// エイジング（純関数）: `base` の処理が `waited_ms` 待ったときの実効の段。
/// [`AGING_STEP_MS`] ごとに 1 段上げ、`high` で止める。`top` / `high` はそのまま
pub fn aged(base: Priority, waited_ms: u64) -> Priority {
    let mut p = base;
    for _ in 0..waited_ms / AGING_STEP_MS {
        if p <= Priority::High {
            break;
        }
        p = p.raised();
    }
    p
}

/// 優先度の既定を決める役割（ペインの役割ラベル / env `TAKO_ORCHESTRATOR_ROLE` から読む）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerRole {
    Master,
    Solo,
    Worker,
}

impl CallerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Solo => "solo",
            Self::Worker => "worker",
        }
    }

    /// ペインの役割ラベル（`orchestrator-master[:<p>]` / `orchestrator-solo[:<p>]` /
    /// `orchestrator-worker:<project>[:<label>]`）から。それ以外（`dev-server` 等・無し）は `None`
    pub fn from_pane_role(label: &str) -> Option<Self> {
        let rest = label.strip_prefix("orchestrator-")?;
        Self::from_env_role(rest)
    }

    /// env `TAKO_ORCHESTRATOR_ROLE` の値（`master[:<p>]` / `solo[:<p>]` / `worker[:<project>…]`）から
    pub fn from_env_role(value: &str) -> Option<Self> {
        let head = value.split(':').next().unwrap_or(value);
        match head {
            "master" => Some(Self::Master),
            "solo" => Some(Self::Solo),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }
}

/// 優先度の手がかり（wrapper → 調停役。#1996）。**旧い wrapper は送らない**
/// （そのときは `JobWire::priority` をそのまま使う = S2 の振る舞い）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PriorityHint {
    /// 処理に付けた段（受付の `--priority` / env `TAKO_QUEUE_PRIORITY` / 後からの受付番号での変更を
    /// wrapper が覚えた値）。無ければ既定（ペイン > 役割）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Priority>,
    /// stdin が端末（人が打った）
    #[serde(default)]
    pub tty: bool,
    /// env `TAKO_ORCHESTRATOR_ROLE` から読んだ役割（ペインの役割ラベルが引けないときの代わり）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<CallerRole>,
}

/// 段がどこで決まったか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityFrom {
    /// 処理に付けた（受付 / 受付番号での変更）
    Job,
    /// ペインの既定（`tako queue priority --pane N`）
    Pane,
    /// 役割の既定
    Role,
}

impl PriorityFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::Pane => "pane",
            Self::Role => "role",
        }
    }
}

/// [`resolve`] の入力
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PriorityInput {
    /// 処理に付けた段（受付番号での変更が受付の指定より強い = 呼び手が先に選んで渡す）
    pub job: Option<Priority>,
    /// ペインの既定
    pub pane_default: Option<Priority>,
    /// tako のペインから呼ばれた（`TAKO_PANE_ID` がある）
    pub in_pane: bool,
    /// ペインの役割（ラベルが引けなければ env の役割）
    pub role: Option<CallerRole>,
    /// 端末から打った
    pub tty: bool,
}

/// 役割の既定（純関数。モジュールの説明の表）
pub fn role_default(in_pane: bool, role: Option<CallerRole>, tty: bool) -> Priority {
    if !in_pane {
        return Priority::Low;
    }
    match role {
        Some(CallerRole::Master | CallerRole::Solo) => Priority::High,
        Some(CallerRole::Worker) => Priority::Normal,
        // 役割の無い素のシェルのペイン: 人が端末から打ったものだけ top
        None if tty => Priority::Top,
        None => Priority::Normal,
    }
}

/// 処理の段（エイジングの前）を決める（純関数）: 処理 > ペイン > 役割
pub fn resolve(input: &PriorityInput) -> (Priority, PriorityFrom) {
    if let Some(p) = input.job {
        return (p, PriorityFrom::Job);
    }
    if let Some(p) = input.pane_default {
        return (p, PriorityFrom::Pane);
    }
    (
        role_default(input.in_pane, input.role, input.tty),
        PriorityFrom::Role,
    )
}

/// `tako queue priority` / MCP `tako_queue` の `priority` の値を読む。`default` は「既定へ戻す」= `Ok(None)`
pub fn parse_setting(s: &str) -> Option<Option<Priority>> {
    if s == "default" {
        return Some(None);
    }
    Priority::parse(s).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60 * 1000;

    #[test]
    fn エイジングは15分ごとに1段でhighまで() {
        assert_eq!(aged(Priority::Low, 0), Priority::Low);
        assert_eq!(
            aged(Priority::Low, 15 * MIN - 1),
            Priority::Low,
            "境界の手前"
        );
        assert_eq!(
            aged(Priority::Low, 15 * MIN),
            Priority::Normal,
            "境界ちょうど"
        );
        assert_eq!(aged(Priority::Low, 30 * MIN), Priority::High);
        assert_eq!(
            aged(Priority::Low, 10 * 60 * MIN),
            Priority::High,
            "top へは上げない"
        );
        assert_eq!(aged(Priority::Normal, 15 * MIN), Priority::High);
        assert_eq!(aged(Priority::High, 10 * 60 * MIN), Priority::High);
        assert_eq!(aged(Priority::Top, 10 * 60 * MIN), Priority::Top);
    }

    #[test]
    fn 同じ段の中では受付の順が崩れない() {
        // 古い方は同じか多く上がる = 実効の段の並びで後の受付が先に来ることは無い
        for base in [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::Top,
        ] {
            for older in (0..120).map(|m| m * MIN) {
                for newer in (0..=older / MIN).map(|m| m * MIN) {
                    assert!(
                        aged(base, older) <= aged(base, newer),
                        "{base:?} の {older} ms 待ちが {newer} ms 待ちより下がった"
                    );
                }
            }
        }
    }

    #[test]
    fn 役割の既定() {
        assert_eq!(role_default(false, None, true), Priority::Low, "ペインの外");
        assert_eq!(
            role_default(false, Some(CallerRole::Master), false),
            Priority::Low,
            "ペインの外は役割を問わない"
        );
        assert_eq!(
            role_default(true, Some(CallerRole::Master), false),
            Priority::High
        );
        assert_eq!(
            role_default(true, Some(CallerRole::Solo), true),
            Priority::High
        );
        assert_eq!(
            role_default(true, Some(CallerRole::Worker), true),
            Priority::Normal
        );
        assert_eq!(role_default(true, None, true), Priority::Top, "人が直接");
        assert_eq!(
            role_default(true, None, false),
            Priority::Normal,
            "端末の無い呼び出し"
        );
    }

    #[test]
    fn 処理はペインより強くペインは役割より強い() {
        let base = PriorityInput {
            in_pane: true,
            role: Some(CallerRole::Worker),
            ..PriorityInput::default()
        };
        assert_eq!(resolve(&base), (Priority::Normal, PriorityFrom::Role));
        let pane = PriorityInput {
            pane_default: Some(Priority::High),
            ..base
        };
        assert_eq!(resolve(&pane), (Priority::High, PriorityFrom::Pane));
        let job = PriorityInput {
            job: Some(Priority::Low),
            ..pane
        };
        assert_eq!(resolve(&job), (Priority::Low, PriorityFrom::Job));
    }

    #[test]
    fn 役割はラベルとenvの両方から読む() {
        assert_eq!(
            CallerRole::from_pane_role("orchestrator-master"),
            Some(CallerRole::Master)
        );
        assert_eq!(
            CallerRole::from_pane_role("orchestrator-master:proj"),
            Some(CallerRole::Master)
        );
        assert_eq!(
            CallerRole::from_pane_role("orchestrator-solo:fast"),
            Some(CallerRole::Solo)
        );
        assert_eq!(
            CallerRole::from_pane_role("orchestrator-worker:tako:l"),
            Some(CallerRole::Worker)
        );
        assert_eq!(CallerRole::from_pane_role("dev-server"), None);
        assert_eq!(
            CallerRole::from_pane_role("worker-1"),
            None,
            "前置きの無い名前は役割ではない"
        );
        assert_eq!(
            CallerRole::from_env_role("master:proj"),
            Some(CallerRole::Master)
        );
        assert_eq!(
            CallerRole::from_env_role("worker:tako"),
            Some(CallerRole::Worker)
        );
        assert_eq!(CallerRole::from_env_role("mastery"), None);
    }

    #[test]
    fn 設定の値はdefaultで既定へ戻す() {
        assert_eq!(parse_setting("high"), Some(Some(Priority::High)));
        assert_eq!(parse_setting("default"), Some(None));
        assert_eq!(parse_setting("urgent"), None);
        assert_eq!(
            parse_setting("HIGH"),
            None,
            "大文字は受けない（CLI / MCP と同じ綴り）"
        );
    }
}

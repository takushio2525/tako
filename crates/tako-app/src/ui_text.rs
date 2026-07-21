//! UI 文字列カタログ（#440。#435 i18n 案 B のキー化準備構造）
//!
//! UI に出す文章を render コードへ直書きせず、ここへ機能別モジュールで集約する。
//! 定数・関数名がそのまま将来のロケールキーに対応する
//! （例: `sleep_guard::CHIP_ACTIVE` → `sleep_guard.chip_active`）。
//! #435 でロケールファイル（日/英）へ移すときは、このモジュールの中身を
//! ルックアップ実装へ差し替えるだけで呼び出し側は変わらない。
//! 現時点の表示言語は日本語のみ。

/// sleep-guard 状態チップ + 詳細ポップオーバー（#440）
pub mod sleep_guard {
    use tako_control::sleep_guard::{SleepGuardMode, SleepGuardState};

    // --- チップ（キー: sleep_guard.chip_*） ---

    pub const CHIP_ACTIVE: &str = "スリープ防止中";
    pub const CHIP_ACTIVE_LID: &str = "スリープ防止中・蓋閉じOK";
    pub const CHIP_ACTIVE_THERMAL: &str = "スリープ防止中・高温注意";

    /// チップの表示文言。非表示（スリープ防止が何も働いていない）なら None
    pub fn chip_label(state: &SleepGuardState) -> Option<&'static str> {
        if state.lid_sleep_disabled {
            if state.thermal_state.is_warning() {
                Some(CHIP_ACTIVE_THERMAL)
            } else {
                Some(CHIP_ACTIVE_LID)
            }
        } else if state.assertion_held {
            Some(CHIP_ACTIVE)
        } else {
            None
        }
    }

    // --- ポップオーバー（キー: sleep_guard.popover_*） ---

    pub const POPOVER_TITLE: &str = "スリープ防止";
    pub const LABEL_MODE: &str = "モード";
    pub const LABEL_STATUS: &str = "いまの状態";
    pub const LABEL_LID: &str = "蓋を閉じたら";
    pub const LABEL_CHANGE: &str = "変更するには";

    pub const MODE_OFF: &str = "オフ（スリープを防止しない）";
    pub const MODE_ON: &str = "常時オン（tako 起動中はスリープしない）";
    pub const MODE_WHILE_AGENTS: &str = "自動（エージェント稼働中だけ防止）";

    /// モード表示文言
    pub fn mode_label(mode: SleepGuardMode) -> &'static str {
        match mode {
            SleepGuardMode::Off => MODE_OFF,
            SleepGuardMode::On => MODE_ON,
            SleepGuardMode::WhileAgentsRunning => MODE_WHILE_AGENTS,
        }
    }

    pub const REASON_ALWAYS_ON: &str = "常時オンの設定のため、Mac を自動スリープさせていません";
    pub const REASON_AGENTS_FINISHING: &str =
        "エージェントの処理が終わったため、まもなく防止を解除します";
    pub const REASON_SYSTEM_DISABLED: &str =
        "スリープ無効化（pmset disablesleep）が有効のため、Mac はスリープしません";
    pub const REASON_IDLE: &str = "スリープ防止はいま働いていません（スリープは通常どおり）";

    /// エージェント稼働による防止理由（キー: sleep_guard.reason_agents_running）
    pub fn reason_agents_running(n: usize) -> String {
        format!("エージェント {n} 体が稼働中のため、Mac を自動スリープさせていません")
    }

    /// いま防止が効いている理由の文言
    pub fn reason(state: &SleepGuardState) -> String {
        if state.assertion_held {
            match state.mode {
                SleepGuardMode::On => REASON_ALWAYS_ON.to_string(),
                SleepGuardMode::WhileAgentsRunning if state.busy_agents == 0 => {
                    REASON_AGENTS_FINISHING.to_string()
                }
                SleepGuardMode::WhileAgentsRunning => reason_agents_running(state.busy_agents),
                // Off でアサーション保持は起きない（update が解放する）。防御的フォールバック
                SleepGuardMode::Off => REASON_IDLE.to_string(),
            }
        } else if state.lid_sleep_disabled {
            REASON_SYSTEM_DISABLED.to_string()
        } else {
            REASON_IDLE.to_string()
        }
    }

    pub const LID_KEEPS_RUNNING: &str = "スリープせず処理は続きます（画面は自動で消灯します）";
    pub const LID_SLEEPS: &str = "通常どおりスリープし、実行中の処理は止まります";

    /// 蓋を閉じたときの挙動の文言
    pub fn lid_behavior(state: &SleepGuardState) -> &'static str {
        if state.lid_sleep_disabled {
            LID_KEEPS_RUNNING
        } else {
            LID_SLEEPS
        }
    }

    pub const THERMAL_NOTE: &str = "本体が高温になっています。蓋を開けて放熱してください";

    pub const CHANGE_COMMAND: &str = "tako sleep-guard set --mode off";
    pub const CHANGE_HINT_AI: &str = "AI に「スリープ防止をオフにして」と頼んでも変更できます";
}

#[cfg(test)]
mod tests {
    use super::sleep_guard as sg;
    use tako_control::sleep_guard::{
        LidSleepMode, PowerCondition, SleepGuardMode, SleepGuardState, ThermalState,
    };

    fn state(
        assertion_held: bool,
        mode: SleepGuardMode,
        busy_agents: usize,
        lid_sleep_disabled: bool,
        thermal_state: ThermalState,
    ) -> SleepGuardState {
        SleepGuardState {
            assertion_held,
            mode,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            thermal_state,
            display_sleep_forced: false,
        }
    }

    #[test]
    fn chip_hidden_when_nothing_active() {
        let s = state(
            false,
            SleepGuardMode::WhileAgentsRunning,
            0,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(sg::chip_label(&s), None);
    }

    #[test]
    fn chip_active_only() {
        let s = state(
            true,
            SleepGuardMode::WhileAgentsRunning,
            2,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(sg::chip_label(&s), Some(sg::CHIP_ACTIVE));
    }

    #[test]
    fn chip_lid_ok() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(sg::chip_label(&s), Some(sg::CHIP_ACTIVE_LID));
    }

    #[test]
    fn chip_lid_without_assertion_still_shows() {
        // 手動 pmset disablesleep 等でアサーション無しでも防止は効いている
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(sg::chip_label(&s), Some(sg::CHIP_ACTIVE_LID));
    }

    #[test]
    fn chip_thermal_warning() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Serious);
        assert_eq!(sg::chip_label(&s), Some(sg::CHIP_ACTIVE_THERMAL));
    }

    #[test]
    fn chip_thermal_without_lid_is_plain_active() {
        // 高温でも蓋閉じ防止が効いていなければ通常表示（警告は蓋閉じ継続の文脈でのみ意味を持つ）
        let s = state(
            true,
            SleepGuardMode::WhileAgentsRunning,
            1,
            false,
            ThermalState::Critical,
        );
        assert_eq!(sg::chip_label(&s), Some(sg::CHIP_ACTIVE));
    }

    #[test]
    fn mode_labels() {
        assert_eq!(sg::mode_label(SleepGuardMode::Off), sg::MODE_OFF);
        assert_eq!(sg::mode_label(SleepGuardMode::On), sg::MODE_ON);
        assert_eq!(
            sg::mode_label(SleepGuardMode::WhileAgentsRunning),
            sg::MODE_WHILE_AGENTS
        );
    }

    #[test]
    fn reason_agents_running_includes_count() {
        let s = state(
            true,
            SleepGuardMode::WhileAgentsRunning,
            3,
            false,
            ThermalState::Nominal,
        );
        assert!(sg::reason(&s).contains("3 体"));
    }

    #[test]
    fn reason_always_on() {
        let s = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(sg::reason(&s), sg::REASON_ALWAYS_ON);
    }

    #[test]
    fn reason_agents_finishing_when_held_but_zero_busy() {
        let s = state(
            true,
            SleepGuardMode::WhileAgentsRunning,
            0,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(sg::reason(&s), sg::REASON_AGENTS_FINISHING);
    }

    #[test]
    fn reason_system_disabled_without_assertion() {
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(sg::reason(&s), sg::REASON_SYSTEM_DISABLED);
    }

    #[test]
    fn reason_idle() {
        let s = state(
            false,
            SleepGuardMode::WhileAgentsRunning,
            0,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(sg::reason(&s), sg::REASON_IDLE);
    }

    #[test]
    fn lid_behavior_variants() {
        let with_lid = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(sg::lid_behavior(&with_lid), sg::LID_KEEPS_RUNNING);
        let without_lid = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(sg::lid_behavior(&without_lid), sg::LID_SLEEPS);
    }

    #[test]
    fn strings_have_no_emoji() {
        // 絵文字禁止方針（#217 / #440）。カタログ内の全文字列を機械検査する
        let all = [
            sg::CHIP_ACTIVE,
            sg::CHIP_ACTIVE_LID,
            sg::CHIP_ACTIVE_THERMAL,
            sg::POPOVER_TITLE,
            sg::LABEL_MODE,
            sg::LABEL_STATUS,
            sg::LABEL_LID,
            sg::LABEL_CHANGE,
            sg::MODE_OFF,
            sg::MODE_ON,
            sg::MODE_WHILE_AGENTS,
            sg::REASON_ALWAYS_ON,
            sg::REASON_AGENTS_FINISHING,
            sg::REASON_SYSTEM_DISABLED,
            sg::REASON_IDLE,
            sg::LID_KEEPS_RUNNING,
            sg::LID_SLEEPS,
            sg::THERMAL_NOTE,
            sg::CHANGE_COMMAND,
            sg::CHANGE_HINT_AI,
        ];
        for s in all {
            for c in s.chars() {
                let cp = c as u32;
                assert!(
                    !(0x1F000..=0x1FAFF).contains(&cp)
                        && !(0x2600..=0x27BF).contains(&cp)
                        && !(0xFE00..=0xFE0F).contains(&cp),
                    "絵文字らしき文字 {c:?} (U+{cp:04X}) が文字列 {s:?} に含まれている"
                );
            }
        }
    }
}

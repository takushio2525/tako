//! sleep-guard 状態チップ + 詳細ポップオーバーの文言（#440 → #435 で日英化）

use super::tr;
use tako_control::sleep_guard::{SleepGuardMode, SleepGuardState};

// --- チップ（キー: sleep_guard.chip_*） ---

pub fn chip_active() -> &'static str {
    tr!("スリープ防止中", "Keeping Mac awake")
}
pub fn chip_active_lid() -> &'static str {
    tr!("スリープ防止中・蓋閉じOK", "Keeping awake / lid-close OK")
}
pub fn chip_active_thermal() -> &'static str {
    tr!("スリープ防止中・高温注意", "Keeping awake / running hot")
}

/// チップの表示文言。非表示（スリープ防止が何も働いていない）なら None
pub fn chip_label(state: &SleepGuardState) -> Option<&'static str> {
    if state.lid_sleep_disabled {
        if state.thermal_state.is_warning() {
            Some(chip_active_thermal())
        } else {
            Some(chip_active_lid())
        }
    } else if state.assertion_held {
        Some(chip_active())
    } else {
        None
    }
}

// --- ポップオーバー（キー: sleep_guard.popover_*） ---

pub fn popover_title() -> &'static str {
    tr!("スリープ防止", "Sleep Prevention")
}
pub fn label_mode() -> &'static str {
    tr!("モード", "Mode")
}
pub fn label_status() -> &'static str {
    tr!("いまの状態", "Status")
}
pub fn label_lid() -> &'static str {
    tr!("蓋を閉じたら", "On lid close")
}
pub fn label_change() -> &'static str {
    tr!("変更するには", "To change")
}

pub fn mode_off() -> &'static str {
    tr!("オフ（スリープを防止しない）", "Off (do not prevent sleep)")
}
pub fn mode_on() -> &'static str {
    tr!(
        "常時オン（tako 起動中はスリープしない）",
        "Always on (no sleep while tako is running)"
    )
}
pub fn mode_while_agents() -> &'static str {
    tr!(
        "自動（エージェント稼働中だけ防止）",
        "Auto (prevent sleep only while agents are running)"
    )
}

/// モード表示文言
pub fn mode_label(mode: SleepGuardMode) -> &'static str {
    match mode {
        SleepGuardMode::Off => mode_off(),
        SleepGuardMode::On => mode_on(),
        SleepGuardMode::WhileAgentsRunning => mode_while_agents(),
    }
}

pub fn reason_always_on() -> &'static str {
    tr!(
        "常時オンの設定のため、Mac を自動スリープさせていません",
        "Always-on is enabled, so the Mac is kept from sleeping"
    )
}
pub fn reason_agents_finishing() -> &'static str {
    tr!(
        "エージェントの処理が終わったため、まもなく防止を解除します",
        "Agents have finished; sleep prevention will be released shortly"
    )
}
pub fn reason_system_disabled() -> &'static str {
    tr!(
        "スリープ無効化（pmset disablesleep）が有効のため、Mac はスリープしません",
        "System sleep is disabled (pmset disablesleep), so the Mac will not sleep"
    )
}
pub fn reason_idle() -> &'static str {
    tr!(
        "スリープ防止はいま働いていません（スリープは通常どおり）",
        "Sleep prevention is not active right now (normal sleep behavior)"
    )
}

/// エージェント稼働による防止理由（キー: sleep_guard.reason_agents_running）
pub fn reason_agents_running(n: usize) -> String {
    tr!(
        format!("エージェント {n} 体が稼働中のため、Mac を自動スリープさせていません"),
        format!("{n} agent(s) running — keeping the Mac awake")
    )
}

/// いま防止が効いている理由の文言
pub fn reason(state: &SleepGuardState) -> String {
    if state.assertion_held {
        match state.mode {
            SleepGuardMode::On => reason_always_on().to_string(),
            SleepGuardMode::WhileAgentsRunning if state.busy_agents == 0 => {
                reason_agents_finishing().to_string()
            }
            SleepGuardMode::WhileAgentsRunning => reason_agents_running(state.busy_agents),
            // Off でアサーション保持は起きない（update が解放する）。防御的フォールバック
            SleepGuardMode::Off => reason_idle().to_string(),
        }
    } else if state.lid_sleep_disabled {
        reason_system_disabled().to_string()
    } else {
        reason_idle().to_string()
    }
}

pub fn lid_keeps_running() -> &'static str {
    tr!(
        "スリープせず処理は続きます（画面は自動で消灯します）",
        "Processes keep running without sleep (the display still turns off)"
    )
}
pub fn lid_sleeps() -> &'static str {
    tr!(
        "通常どおりスリープし、実行中の処理は止まります",
        "The Mac sleeps as usual, stopping running processes"
    )
}

/// 蓋を閉じたときの挙動の文言
pub fn lid_behavior(state: &SleepGuardState) -> &'static str {
    if state.lid_sleep_disabled {
        lid_keeps_running()
    } else {
        lid_sleeps()
    }
}

pub fn thermal_note() -> &'static str {
    tr!(
        "本体が高温になっています。蓋を開けて放熱してください",
        "The Mac is running hot. Open the lid to let it cool down"
    )
}

/// 変更コマンド（言語非依存。キー: sleep_guard.change_command）
pub const CHANGE_COMMAND: &str = "tako sleep-guard set --mode off";

pub fn change_hint_ai() -> &'static str {
    tr!(
        "AI に「スリープ防止をオフにして」と頼んでも変更できます",
        "You can also ask the AI: \"turn off sleep prevention\""
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;
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

    // 分岐テストは相対比較（chip_label の結果 == 対応する文言関数の結果）なので、
    // 表示言語グローバルがどちらでも成立する

    #[test]
    fn chip_hidden_when_nothing_active() {
        let s = state(
            false,
            SleepGuardMode::WhileAgentsRunning,
            0,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(chip_label(&s), None);
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
        assert_eq!(chip_label(&s), Some(chip_active()));
    }

    #[test]
    fn chip_lid_ok() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(chip_label(&s), Some(chip_active_lid()));
    }

    #[test]
    fn chip_lid_without_assertion_still_shows() {
        // 手動 pmset disablesleep 等でアサーション無しでも防止は効いている
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(chip_label(&s), Some(chip_active_lid()));
    }

    #[test]
    fn chip_thermal_warning() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Serious);
        assert_eq!(chip_label(&s), Some(chip_active_thermal()));
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
        assert_eq!(chip_label(&s), Some(chip_active()));
    }

    #[test]
    fn mode_labels() {
        assert_eq!(mode_label(SleepGuardMode::Off), mode_off());
        assert_eq!(mode_label(SleepGuardMode::On), mode_on());
        assert_eq!(
            mode_label(SleepGuardMode::WhileAgentsRunning),
            mode_while_agents()
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
        assert!(reason(&s).contains('3'));
    }

    #[test]
    fn reason_always_on_selected() {
        let s = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(reason(&s), reason_always_on());
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
        assert_eq!(reason(&s), reason_agents_finishing());
    }

    #[test]
    fn reason_system_disabled_without_assertion() {
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(reason(&s), reason_system_disabled());
    }

    #[test]
    fn reason_idle_selected() {
        let s = state(
            false,
            SleepGuardMode::WhileAgentsRunning,
            0,
            false,
            ThermalState::Nominal,
        );
        assert_eq!(reason(&s), reason_idle());
    }

    #[test]
    fn lid_behavior_variants() {
        let with_lid = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(lid_behavior(&with_lid), lid_keeps_running());
        let without_lid = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(lid_behavior(&without_lid), lid_sleeps());
    }

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        // 日英カタログの機械検査（#435）。言語グローバルの切替を伴うため
        // tests_support::check_ja_en に集約（他の lang 依存テストは相対比較のみで安全）
        tests_support::check_ja_en(|| {
            vec![
                chip_active().to_string(),
                chip_active_lid().to_string(),
                chip_active_thermal().to_string(),
                popover_title().to_string(),
                label_mode().to_string(),
                label_status().to_string(),
                label_lid().to_string(),
                label_change().to_string(),
                mode_off().to_string(),
                mode_on().to_string(),
                mode_while_agents().to_string(),
                reason_always_on().to_string(),
                reason_agents_finishing().to_string(),
                reason_system_disabled().to_string(),
                reason_idle().to_string(),
                reason_agents_running(2),
                lid_keeps_running().to_string(),
                lid_sleeps().to_string(),
                thermal_note().to_string(),
                change_hint_ai().to_string(),
            ]
        });
    }
}

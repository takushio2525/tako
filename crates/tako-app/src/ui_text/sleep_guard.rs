//! sleep-guard 状態チップ + 詳細ポップオーバーの文言（#440 → #435 で日英化）

use tako_control::sleep_guard::{SleepGuardMode, SleepGuardState};

use crate::settings_sleep::Device;

// --- チップ（キー: sleep_guard.chip_*） ---

/// スリープ防止中のチップ（#905）。
///
/// 英語側が機械を名指すので呼び名を受け取る。`Device` を値で持ち回すのは
/// **macOS 上から Windows 側の文言を検証できる**ようにするため（#727 / #515）
pub fn chip_active(device: Device) -> &'static str {
    match device {
        Device::Mac => tr!("スリープ防止中", "Keeping Mac awake"),
        Device::Pc => tr!("スリープ防止中", "Keeping this PC awake"),
    }
}
pub fn chip_active_lid() -> &'static str {
    tr!("スリープ防止中・蓋閉じOK", "Keeping awake / lid-close OK")
}
pub fn chip_active_thermal() -> &'static str {
    tr!("スリープ防止中・高温注意", "Keeping awake / running hot")
}

/// チップの表示文言。非表示（スリープ防止が何も働いていない）なら None
pub fn chip_label(state: &SleepGuardState, device: Device) -> Option<&'static str> {
    if state.lid_sleep_disabled {
        if state.thermal_state.is_warning() {
            Some(chip_active_thermal())
        } else {
            Some(chip_active_lid())
        }
    } else if state.assertion_held {
        Some(chip_active(device))
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

pub fn reason_always_on(device: Device) -> &'static str {
    match device {
        Device::Mac => tr!(
            "常時オンの設定のため、Mac を自動スリープさせていません",
            "Always-on is enabled, so the Mac is kept from sleeping"
        ),
        Device::Pc => tr!(
            "常時オンの設定のため、この PC を自動スリープさせていません",
            "Always-on is enabled, so this PC is kept from sleeping"
        ),
    }
}
pub fn reason_agents_finishing() -> &'static str {
    tr!(
        "エージェントの処理が終わったため、まもなく防止を解除します",
        "Agents have finished; sleep prevention will be released shortly"
    )
}
/// 蓋閉じ継続（システム側のスリープ無効化）が効いている理由（#727）。
///
/// 手段は OS で違う（macOS = `pmset disablesleep` / Windows = 電源プランの lid action。
/// #697）。Windows でも `lid_sleep_disabled` は真になりうるので、macOS 固有の
/// コマンド名をそのまま出さない。どちらの文面かは呼び名から決める
pub fn reason_system_disabled(device: Device) -> &'static str {
    match device {
        Device::Mac => tr!(
            "スリープ無効化（pmset disablesleep）が有効のため、Mac はスリープしません",
            "System sleep is disabled (pmset disablesleep), so the Mac will not sleep"
        ),
        Device::Pc => tr!(
            "蓋閉じ継続が有効のため、この PC はスリープしません",
            "Lid-close prevention is on, so this PC will not sleep"
        ),
    }
}
pub fn reason_idle() -> &'static str {
    tr!(
        "スリープ防止はいま働いていません（スリープは通常どおり）",
        "Sleep prevention is not active right now (normal sleep behavior)"
    )
}

/// エージェント稼働による防止理由（キー: sleep_guard.reason_agents_running）
pub fn reason_agents_running(n: usize, device: Device) -> String {
    match device {
        Device::Mac => tr!(
            format!("エージェント {n} 体が稼働中のため、Mac を自動スリープさせていません"),
            format!("{n} agent(s) running — keeping the Mac awake")
        ),
        Device::Pc => tr!(
            format!("エージェント {n} 体が稼働中のため、この PC を自動スリープさせていません"),
            format!("{n} agent(s) running — keeping this PC awake")
        ),
    }
}

/// いま防止が効いている理由の文言
pub fn reason(state: &SleepGuardState, device: Device) -> String {
    if state.assertion_held {
        match state.mode {
            SleepGuardMode::On => reason_always_on(device).to_string(),
            SleepGuardMode::WhileAgentsRunning if state.busy_agents == 0 => {
                reason_agents_finishing().to_string()
            }
            SleepGuardMode::WhileAgentsRunning => reason_agents_running(state.busy_agents, device),
            // Off でアサーション保持は起きない（update が解放する）。防御的フォールバック
            SleepGuardMode::Off => reason_idle().to_string(),
        }
    } else if state.lid_sleep_disabled {
        reason_system_disabled(device).to_string()
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
pub fn lid_sleeps(device: Device) -> &'static str {
    match device {
        Device::Mac => tr!(
            "通常どおりスリープし、実行中の処理は止まります",
            "The Mac sleeps as usual, stopping running processes"
        ),
        Device::Pc => tr!(
            "通常どおりスリープし、実行中の処理は止まります",
            "This PC sleeps as usual, stopping running processes"
        ),
    }
}

/// 蓋を閉じたときの挙動の文言
pub fn lid_behavior(state: &SleepGuardState, device: Device) -> &'static str {
    if state.lid_sleep_disabled {
        lid_keeps_running()
    } else {
        lid_sleeps(device)
    }
}

pub fn thermal_note(device: Device) -> &'static str {
    match device {
        Device::Mac => tr!(
            "本体が高温になっています。蓋を開けて放熱してください",
            "The Mac is running hot. Open the lid to let it cool down"
        ),
        // thermal は macOS しか観測できない（Windows は常に nominal）ので実際には
        // 出ないが、文言だけ macOS 固有のまま残すと将来観測できた OS で嘘になる
        Device::Pc => tr!(
            "本体が高温になっています。蓋を開けて放熱してください",
            "This PC is running hot. Open the lid to let it cool down"
        ),
    }
}

/// チップ + ポップオーバーがこの状態で見せる文字列すべて（#905）。
///
/// **`status_bar::render_sleep_guard_overlay` が描くものと同じ集合**を返す
/// （どちらかに文字列を足したらもう一方にも足す）。状態を受け取るので、
/// 高温注記のような条件つきの行も実際に出るときだけ入る。こう並べておくと
/// 「この OS で通じない語が出ない」を GUI を起こさずに機械検査できる
/// （#727 の `settings_sleep::SleepTabPlan::visible_texts` と同じ形）
pub fn popover_texts(state: &SleepGuardState, device: Device) -> Vec<String> {
    let mut out: Vec<String> = vec![
        popover_title().into(),
        label_mode().into(),
        mode_label(state.mode).into(),
        label_status().into(),
        reason(state, device),
        label_lid().into(),
        lid_behavior(state, device).into(),
        label_change().into(),
        CHANGE_COMMAND.into(),
        change_hint_ai().into(),
    ];
    // 高温注記は「蓋閉じ継続が効いている文脈」でだけ描かれる（renderer の thermal 条件）
    if state.lid_sleep_disabled && state.thermal_state.is_warning() {
        out.push(thermal_note(device).into());
    }
    // チップはポップオーバーの入口（同じ機能の表示面なので一緒に見る）
    if let Some(chip) = chip_label(state, device) {
        out.push(chip.into());
    }
    out
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

    /// この機械の呼び名（相対比較のテストはどちらでも成立する）
    fn dev() -> Device {
        Device::detect()
    }

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
            lid_setup_required: false,
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
        assert_eq!(chip_label(&s, dev()), None);
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
        assert_eq!(chip_label(&s, dev()), Some(chip_active(dev())));
    }

    #[test]
    fn chip_lid_ok() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(chip_label(&s, dev()), Some(chip_active_lid()));
    }

    #[test]
    fn chip_lid_without_assertion_still_shows() {
        // 手動 pmset disablesleep 等でアサーション無しでも防止は効いている
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(chip_label(&s, dev()), Some(chip_active_lid()));
    }

    #[test]
    fn chip_thermal_warning() {
        let s = state(true, SleepGuardMode::On, 0, true, ThermalState::Serious);
        assert_eq!(chip_label(&s, dev()), Some(chip_active_thermal()));
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
        assert_eq!(chip_label(&s, dev()), Some(chip_active(dev())));
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
        assert!(reason(&s, dev()).contains('3'));
    }

    #[test]
    fn reason_always_on_selected() {
        let s = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(reason(&s, dev()), reason_always_on(dev()));
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
        assert_eq!(reason(&s, dev()), reason_agents_finishing());
    }

    #[test]
    fn reason_system_disabled_without_assertion() {
        let s = state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal);
        assert_eq!(reason(&s, dev()), reason_system_disabled(Device::detect()));
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
        assert_eq!(reason(&s, dev()), reason_idle());
    }

    #[test]
    fn lid_behavior_variants() {
        let with_lid = state(true, SleepGuardMode::On, 0, true, ThermalState::Nominal);
        assert_eq!(lid_behavior(&with_lid, dev()), lid_keeps_running());
        let without_lid = state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal);
        assert_eq!(lid_behavior(&without_lid, dev()), lid_sleeps(dev()));
    }

    /// macOS 固有の語。Windows の画面に出たらそれが症状そのもの（#905）
    const MAC_ONLY_WORDS: &[&str] = &["Mac", "pmset", "sudoers"];

    /// チップ + ポップオーバーが取り得る状態（renderer の分岐を全部通る組み合わせ）
    fn representative_states() -> Vec<(&'static str, SleepGuardState)> {
        vec![
            (
                "常時オンで保持中",
                state(true, SleepGuardMode::On, 0, false, ThermalState::Nominal),
            ),
            (
                "エージェント稼働中",
                state(
                    true,
                    SleepGuardMode::WhileAgentsRunning,
                    3,
                    false,
                    ThermalState::Nominal,
                ),
            ),
            (
                "エージェント終了直後",
                state(
                    true,
                    SleepGuardMode::WhileAgentsRunning,
                    0,
                    false,
                    ThermalState::Nominal,
                ),
            ),
            (
                "蓋閉じ継続が効いている",
                state(false, SleepGuardMode::Off, 0, true, ThermalState::Nominal),
            ),
            (
                "蓋閉じ継続 + 高温",
                state(true, SleepGuardMode::On, 1, true, ThermalState::Serious),
            ),
            (
                "何も効いていない",
                state(
                    false,
                    SleepGuardMode::WhileAgentsRunning,
                    0,
                    false,
                    ThermalState::Nominal,
                ),
            ),
        ]
    }

    #[test]
    fn windowsのチップとポップオーバーにmacos固有の語が出ない() {
        // #905: 「Mac」は macOS でしか通じない呼び名。Windows でも同じ状態になりうる
        // （アイドル防止も蓋閉じ継続も #524 / #697 で実装済み）ので、そのまま出すと
        // 存在しない設定や別 OS の話を読ませてしまう
        tests_support::for_each_lang(|| {
            for (name, st) in representative_states() {
                let texts = popover_texts(&st, Device::Pc);
                assert!(texts.len() >= 10, "{name}: 文言が少なすぎる: {texts:?}");
                for t in &texts {
                    for w in MAC_ONLY_WORDS {
                        assert!(
                            !t.contains(w),
                            "{name}: Windows の画面に macOS 固有の語 {w:?} が出ている: {t:?}"
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn macosの文言は従来どおり() {
        // #905 の変更で macOS 側が 1 文字も動いていないことを固定する。
        // 相対比較では「両方いっしょに壊れた」を検出できないので、実文字列で押さえる
        let expected_ja: &[&str] = &[
            "スリープ防止中",
            "常時オンの設定のため、Mac を自動スリープさせていません",
            "エージェント 2 体が稼働中のため、Mac を自動スリープさせていません",
            "通常どおりスリープし、実行中の処理は止まります",
            "本体が高温になっています。蓋を開けて放熱してください",
        ];
        let expected_en: &[&str] = &[
            "Keeping Mac awake",
            "Always-on is enabled, so the Mac is kept from sleeping",
            "2 agent(s) running — keeping the Mac awake",
            "The Mac sleeps as usual, stopping running processes",
            "The Mac is running hot. Open the lid to let it cool down",
        ];
        let collect = || {
            vec![
                chip_active(Device::Mac).to_string(),
                reason_always_on(Device::Mac).to_string(),
                reason_agents_running(2, Device::Mac),
                lid_sleeps(Device::Mac).to_string(),
                thermal_note(Device::Mac).to_string(),
            ]
        };
        use tako_core::i18n::Lang;
        tests_support::with_lang(Lang::Ja, || assert_eq!(collect(), expected_ja));
        tests_support::with_lang(Lang::En, || assert_eq!(collect(), expected_en));
    }

    #[test]
    fn ポップオーバーが描く文言はpopover_textsに全部載っている() {
        // #905: `popover_texts` は「画面に出る集合」を名乗るので、renderer が
        // 別の文言関数を足したら検査が空振りする。ソースを走査して名指しで落とす
        // （コメントで「同じ集合にする」と書くだけでは片方を直したときに気づけない）
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let bar = std::fs::read_to_string(dir.join("src/status_bar.rs")).expect("status_bar.rs");
        let start = bar
            .find("fn render_sleep_guard_overlay")
            .expect("renderer が見つからない");
        // 次のメソッド定義までが renderer 本体（インデント 4 = impl 直下の境界）。
        // ここを緩めると後続の関数まで拾ってしまう（別の `text` 別名を使う関数がある）
        let end = ["\n    fn ", "\n    pub fn ", "\n    pub(crate) fn "]
            .iter()
            .filter_map(|pat| bar[start..].find(pat).map(|i| start + i))
            .min()
            .unwrap_or(bar.len());
        let body = &bar[start..end];

        let mut used: Vec<String> = Vec::new();
        for (i, _) in body.match_indices("text::") {
            // `ui_text::sleep_guard` の中にも `text::` があるので、別名 `text` として
            // 使われている（直前が識別子文字でない）ときだけ拾う
            let preceded_by_ident = body[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if preceded_by_ident {
                continue;
            }
            let rest = &body[i + "text::".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !used.contains(&name) {
                used.push(name);
            }
        }
        assert!(
            used.len() >= 8,
            "renderer の文言呼び出しを拾えていない: {used:?}"
        );

        let me = std::fs::read_to_string(dir.join("src/ui_text/sleep_guard.rs")).expect("self");
        let helper_start = me
            .find("pub fn popover_texts")
            .expect("popover_texts が見つからない");
        let helper_end = me[helper_start..]
            .find("\n}\n")
            .map(|i| helper_start + i)
            .expect("popover_texts の終端");
        let helper = &me[helper_start..helper_end];
        for name in &used {
            assert!(
                helper.contains(name.as_str()),
                "renderer が描く {name} が popover_texts に載っていない（#905）"
            );
        }
    }

    #[test]
    fn mac以外の理由文にmacos固有の語を出さない() {
        // #727: Windows でも lid_sleep_disabled は真になりうる。macOS のコマンド名
        // （pmset）と呼び名（Mac）が出ると、存在しない設定を探しに行かせてしまう
        tests_support::for_each_lang(|| {
            let t = reason_system_disabled(Device::Pc);
            assert!(!t.contains("pmset"), "pmset が残っている: {t:?}");
            assert!(!t.contains("Mac"), "Mac が残っている: {t:?}");
        });
    }

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        // 日英カタログの機械検査（#435）。言語グローバルの切替を伴うため
        // tests_support::check_ja_en に集約（他の lang 依存テストは相対比較のみで安全）
        tests_support::check_ja_en(|| {
            vec![
                chip_active(Device::Mac).to_string(),
                chip_active(Device::Pc).to_string(),
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
                reason_always_on(Device::Mac).to_string(),
                reason_always_on(Device::Pc).to_string(),
                reason_agents_finishing().to_string(),
                reason_system_disabled(Device::Mac).to_string(),
                reason_system_disabled(Device::Pc).to_string(),
                reason_idle().to_string(),
                reason_agents_running(2, Device::Mac),
                reason_agents_running(2, Device::Pc),
                lid_keeps_running().to_string(),
                lid_sleeps(Device::Mac).to_string(),
                lid_sleeps(Device::Pc).to_string(),
                thermal_note(Device::Mac).to_string(),
                thermal_note(Device::Pc).to_string(),
                change_hint_ai().to_string(),
            ]
        });
    }
}

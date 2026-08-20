//! 設定画面「スリープ防止」タブの表示構成（#727）
//!
//! 「どの行・どのボタンを描くか」「いまどういう状態か」を **OS 名ではなくこの OS の能力**
//! から決める。判定はすべて純粋関数なので、macOS 上からでも Windows 側の分岐を
//! `cargo test` で検証できる（#515 と同じ方針）。
//!
//! なぜ要るか: 蓋閉じ継続の実現手段は OS で違う（macOS = sudoers + `pmset disablesleep`、
//! Windows = 電源プランの `GUID_LIDCLOSE_ACTION`。#697）。手段を UI に直接書くと
//! 「sudoers を登録」という macOS 専用の案内が Windows にも出てしまうので、
//! UI は `sleep_guard` が公開する能力（初回セットアップが要るか等）だけを見る。

use tako_control::sleep_guard::{self, LidSleepMode, PowerCondition, SleepGuardMode};

/// 「アイドルスリープ防止」のいまの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleStatus {
    /// この OS ではアイドルスリープ防止そのものが使えない
    Unsupported,
    /// モードが off
    Off,
    /// いま防止が効いている
    Active,
    /// 効かせたいが AC 未接続で見送り中
    PausedNoAc,
    /// エージェント稼働中だけ防止する設定で、いまは待機中
    WaitingAgents,
    /// 効かせるべき条件は揃っているが、まだ保持できていない。
    ///
    /// 設定変更の直後は必ずここを通る（設定を書くのは dispatch、実際に電源要求を出すのは
    /// 2 秒ごとの tick なので、その隙間がある）。「無効」と出すと、いま自分で
    /// 「常時オン」にした人に嘘をつくことになる
    Applying,
}

/// 「蓋閉じ継続」のいまの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidStatus {
    /// この OS では蓋閉じ継続に対応していない
    Unsupported,
    /// モードが off
    Off,
    /// 初回セットアップが済んでいない（macOS の sudoers 未登録）
    SetupRequired,
    /// いま蓋閉じ継続が効いている
    Active,
    /// エージェント待機中で効いていない
    WaitingAgents,
    /// AC 未接続で効いていない
    PausedNoAc,
    /// 本体が高温のため見送り中（macOS のみ観測できる）
    PausedThermal,
    /// 効かせるべき条件は揃っているが、まだ倒せていない（[`IdleStatus::Applying`] と同じ隙間）
    Applying,
}

/// 設定画面が表示に使うスリープ防止の状態。
///
/// `tako sleep-guard status`（dispatch）の JSON から作る。JSON を UI へ直接持ち込むと
/// 「どのキーをどう読むか」が描画コードへ散るので、ここで 1 度だけ型へ落とす。
///
/// 既定値は「まだ何も分かっていない」= 使えない側へ倒す（モード等の既定は
/// `tako_control` の定義に従う）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SleepSnapshot {
    /// アイドルスリープ防止がこの OS で使えるか
    pub idle_supported: bool,
    pub mode: SleepGuardMode,
    pub power_condition: PowerCondition,
    /// いま電源アサーション（Windows は電源要求）を保持しているか
    pub assertion_held: bool,
    pub busy_agents: usize,
    pub on_ac_power: bool,
    /// 蓋閉じ継続がこの OS で使えるか
    pub lid_supported: bool,
    pub lid_mode: LidSleepMode,
    /// いま蓋閉じ継続が効いているか
    pub lid_active: bool,
    /// この機械でまだ初回セットアップが要るか
    pub lid_setup_required: bool,
    /// この OS の蓋閉じ継続が、管理者権限を伴う初回登録を要する仕組みか。
    /// **OS 固定の性質**なので JSON には無く、`sleep_guard` の判定関数から入れる
    pub lid_needs_privileged_setup: bool,
    /// 本体が高温か（macOS のみ観測できる。Windows は常に false）
    pub thermal_warning: bool,
}

impl SleepSnapshot {
    /// dispatch（`Request::SleepGuard { action: "status" }`）の応答から作る。
    ///
    /// 状態をまだ取れていない（None）ときは「この OS では使えない」ではなく
    /// **既定値**を返す。使えないと言い切ると、取得に失敗しただけの環境で
    /// 「非対応」と嘘をつくことになる
    pub fn from_status_json(value: Option<&serde_json::Value>) -> Self {
        let needs_setup_step = sleep_guard::lid_requires_privileged_setup();
        let Some(v) = value else {
            return Self {
                lid_needs_privileged_setup: needs_setup_step,
                ..Self::default()
            };
        };
        let b = |key: &str| v.get(key).and_then(|x| x.as_bool()).unwrap_or(false);
        let s = |key: &str| v.get(key).and_then(|x| x.as_str()).unwrap_or("");
        Self {
            idle_supported: b("platform_supported"),
            mode: SleepGuardMode::from_str_opt(s("mode")).unwrap_or_default(),
            power_condition: PowerCondition::from_str_opt(s("power_condition")).unwrap_or_default(),
            assertion_held: b("assertion_held"),
            busy_agents: v.get("busy_agents").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            on_ac_power: b("on_ac_power"),
            lid_supported: b("lid_control_supported"),
            lid_mode: LidSleepMode::from_str_opt(s("lid_sleep_mode")).unwrap_or_default(),
            lid_active: b("lid_sleep_disabled"),
            lid_setup_required: b("lid_setup_required"),
            lid_needs_privileged_setup: needs_setup_step,
            thermal_warning: matches!(s("thermal_state"), "serious" | "critical"),
        }
    }

    /// 蓋閉じ継続の行（モード切替）を描くか。制御できない OS では出さない
    /// （出しても倒せないので、切り替えられる顔をしてはいけない）
    pub fn show_lid_row(&self) -> bool {
        self.lid_supported
    }

    /// 初回セットアップのボタンを描くか。
    ///
    /// **権限が要らない OS では出さない**。Windows は電源プランを非管理者で書けるので
    /// 登録も解除も無く、ボタンを出すと「押さないと使えない」と誤解させるうえ、
    /// dispatch 側は macOS 専用の経路（`osascript`）なので押しても必ず失敗する
    pub fn show_setup_buttons(&self) -> bool {
        self.lid_supported && self.lid_needs_privileged_setup
    }

    /// アイドルスリープ防止のいまの状態
    pub fn idle_status(&self) -> IdleStatus {
        if !self.idle_supported {
            return IdleStatus::Unsupported;
        }
        if self.mode == SleepGuardMode::Off {
            return IdleStatus::Off;
        }
        if self.assertion_held {
            return IdleStatus::Active;
        }
        // 以下は「効かせたい設定なのに保持していない」理由の切り分け。
        // 判定の真理値は sleep_guard::should_hold_assertion と揃える
        let wants = match self.mode {
            SleepGuardMode::Off => false,
            SleepGuardMode::On => true,
            SleepGuardMode::WhileAgentsRunning => self.busy_agents > 0,
        };
        if wants && self.power_condition == PowerCondition::AcOnly && !self.on_ac_power {
            return IdleStatus::PausedNoAc;
        }
        if self.mode == SleepGuardMode::WhileAgentsRunning && self.busy_agents == 0 {
            return IdleStatus::WaitingAgents;
        }
        // ここまで来たら「保持すべきなのに保持していない」= 反映待ち
        IdleStatus::Applying
    }

    /// 蓋閉じ継続のいまの状態
    pub fn lid_status(&self) -> LidStatus {
        if !self.lid_supported {
            return LidStatus::Unsupported;
        }
        // 効いているかどうかを最優先で見る。tako の設定が off でも、
        // 手動で倒されている（macOS の `pmset disablesleep` 等）なら「効いている」が事実
        if self.lid_active {
            return LidStatus::Active;
        }
        if self.lid_mode == LidSleepMode::Off {
            return LidStatus::Off;
        }
        if self.lid_setup_required {
            return LidStatus::SetupRequired;
        }
        // 判定の真理値は sleep_guard::should_disable_lid_sleep と揃える
        if self.thermal_warning {
            return LidStatus::PausedThermal;
        }
        if self.busy_agents == 0 {
            return LidStatus::WaitingAgents;
        }
        if !self.on_ac_power {
            return LidStatus::PausedNoAc;
        }
        LidStatus::Applying
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> SleepSnapshot {
        // Windows 相当: アイドル防止も蓋閉じ継続も使えて、初回セットアップは不要
        SleepSnapshot {
            idle_supported: true,
            lid_supported: true,
            lid_needs_privileged_setup: false,
            lid_setup_required: false,
            on_ac_power: true,
            ..SleepSnapshot::default()
        }
    }

    fn mac() -> SleepSnapshot {
        // macOS 相当: 蓋閉じ継続に sudoers 登録が要る
        SleepSnapshot {
            idle_supported: true,
            lid_supported: true,
            lid_needs_privileged_setup: true,
            lid_setup_required: true,
            on_ac_power: true,
            ..SleepSnapshot::default()
        }
    }

    // --- 表示構成（この 2 本が #727 の本体） ---

    #[test]
    fn windowsでは初回セットアップのボタンを出さない() {
        assert!(!win().show_setup_buttons());
        assert!(win().show_lid_row(), "モード切替そのものは出す");
    }

    #[test]
    fn macosでは初回セットアップのボタンを出す() {
        assert!(mac().show_setup_buttons());
        // 登録が済んでも「解除」ができないと困るので、出し続ける
        let installed = SleepSnapshot {
            lid_setup_required: false,
            ..mac()
        };
        assert!(installed.show_setup_buttons());
    }

    #[test]
    fn 蓋閉じ継続に非対応なら行もボタンも出さない() {
        let other = SleepSnapshot {
            idle_supported: true,
            lid_supported: false,
            ..SleepSnapshot::default()
        };
        assert!(!other.show_lid_row());
        assert!(!other.show_setup_buttons());
    }

    // --- アイドルスリープ防止の状態 ---

    #[test]
    fn idle非対応os() {
        let s = SleepSnapshot {
            idle_supported: false,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::Unsupported);
    }

    #[test]
    fn idleオフ() {
        let s = SleepSnapshot {
            mode: SleepGuardMode::Off,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::Off);
    }

    #[test]
    fn idle保持中は理由に関わらずactive() {
        let s = SleepSnapshot {
            mode: SleepGuardMode::On,
            assertion_held: true,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::Active);
    }

    #[test]
    fn idle_ac未接続で見送り() {
        let s = SleepSnapshot {
            mode: SleepGuardMode::On,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: false,
            assertion_held: false,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::PausedNoAc);
    }

    #[test]
    fn idle_エージェント待機中() {
        let s = SleepSnapshot {
            mode: SleepGuardMode::WhileAgentsRunning,
            busy_agents: 0,
            assertion_held: false,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::WaitingAgents);
    }

    #[test]
    fn idle_エージェント稼働中でac未接続なら待機ではなくac理由() {
        // 「エージェントは動いているのに効かない」理由は AC なので、そちらを優先する
        let s = SleepSnapshot {
            mode: SleepGuardMode::WhileAgentsRunning,
            busy_agents: 2,
            on_ac_power: false,
            assertion_held: false,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::PausedNoAc);
    }

    #[test]
    fn idle_設定直後は反映中を出す() {
        // 設定を書くのは dispatch、電源要求を出すのは 2 秒 tick なので隙間がある。
        // ここで「無効」と出すと、いま自分で常時オンにした人に嘘をつくことになる
        let s = SleepSnapshot {
            mode: SleepGuardMode::On,
            assertion_held: false,
            on_ac_power: true,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::Applying);
    }

    #[test]
    fn lid_設定直後は反映中を出す() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            lid_active: false,
            busy_agents: 1,
            on_ac_power: true,
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::Applying);
    }

    #[test]
    fn idle_alwaysならバッテリーでも見送りにならない() {
        let s = SleepSnapshot {
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::Always,
            busy_agents: 0,
            on_ac_power: false,
            ..win()
        };
        assert_eq!(s.idle_status(), IdleStatus::WaitingAgents);
    }

    // --- 蓋閉じ継続の状態 ---

    #[test]
    fn lid非対応os() {
        let s = SleepSnapshot {
            lid_supported: false,
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::Unsupported);
    }

    #[test]
    fn lid効いていれば設定に関わらずactive() {
        // 手動で倒されている場合も事実として「効いている」と出す
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::Off,
            lid_active: true,
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::Active);
    }

    #[test]
    fn lidオフ() {
        assert_eq!(
            SleepSnapshot {
                lid_mode: LidSleepMode::Off,
                ..win()
            }
            .lid_status(),
            LidStatus::Off
        );
    }

    #[test]
    fn lid_macosはセットアップ待ちを出す() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            busy_agents: 1,
            ..mac()
        };
        assert_eq!(s.lid_status(), LidStatus::SetupRequired);
    }

    #[test]
    fn lid_windowsはセットアップ待ちにならない() {
        // 権限が要らないので、有効にした瞬間から「エージェント待ち」へ進む
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            busy_agents: 0,
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::WaitingAgents);
    }

    #[test]
    fn lid_ac未接続() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            busy_agents: 1,
            on_ac_power: false,
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::PausedNoAc);
    }

    #[test]
    fn lid_高温は最優先で見送り() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            busy_agents: 1,
            thermal_warning: true,
            lid_setup_required: false,
            ..mac()
        };
        assert_eq!(s.lid_status(), LidStatus::PausedThermal);
    }

    // --- JSON の取り込み ---

    #[test]
    fn statusのjsonを読める() {
        let v = serde_json::json!({
            "assertion_held": true,
            "mode": "while-agents-running",
            "power_condition": "ac-only",
            "on_ac_power": true,
            "busy_agents": 3,
            "platform_supported": true,
            "lid_sleep_disabled": true,
            "lid_sleep_mode": "while-agents-running",
            "lid_control_supported": true,
            "lid_setup_required": false,
            "thermal_state": "serious",
        });
        let s = SleepSnapshot::from_status_json(Some(&v));
        assert!(s.idle_supported);
        assert_eq!(s.mode, SleepGuardMode::WhileAgentsRunning);
        assert_eq!(s.power_condition, PowerCondition::AcOnly);
        assert!(s.assertion_held);
        assert_eq!(s.busy_agents, 3);
        assert!(s.on_ac_power);
        assert!(s.lid_supported);
        assert_eq!(s.lid_mode, LidSleepMode::WhileAgentsRunning);
        assert!(s.lid_active);
        assert!(!s.lid_setup_required);
        assert!(s.thermal_warning);
        assert_eq!(s.idle_status(), IdleStatus::Active);
        assert_eq!(s.lid_status(), LidStatus::Active);
    }

    #[test]
    fn 状態を取れていなければ非対応と断定しない() {
        // 「まだ取れていない」を「この OS では使えない」と混同すると、
        // 取得に失敗しただけの環境へ嘘の案内を出すことになる
        let s = SleepSnapshot::from_status_json(None);
        assert_eq!(s.idle_status(), IdleStatus::Unsupported);
        assert!(!s.show_setup_buttons(), "行もボタンも出さない側へ倒す");
    }

    #[test]
    fn 欠けたキーがあっても既定値で読める() {
        let v = serde_json::json!({ "mode": "on", "platform_supported": true });
        let s = SleepSnapshot::from_status_json(Some(&v));
        assert_eq!(s.mode, SleepGuardMode::On);
        assert!(!s.lid_supported);
        assert!(!s.thermal_warning);
    }

    #[test]
    fn 初回セットアップの要否はosから入る() {
        // JSON には無い（登録済みかどうかでは変わらない OS 固定の性質）
        let s = SleepSnapshot::from_status_json(Some(&serde_json::json!({})));
        assert_eq!(
            s.lid_needs_privileged_setup,
            cfg!(target_os = "macos"),
            "macOS だけが sudoers 登録という初回セットアップを持つ"
        );
    }
}

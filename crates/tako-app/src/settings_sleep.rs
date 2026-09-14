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

use tako_control::sleep_guard::{
    self, LidGuardInput, LidSkipReason, LidSleepMode, PowerCondition, SleepGuardMode, ThermalState,
};

use crate::ui_text::settings as txt;

/// 文言がこの機械を何と呼ぶか（#727）。
///
/// 「Mac」も「pmset」も能力ではなく**呼び名**なので、`sleep_guard` の能力関数
/// （`lid_requires_privileged_setup` 等）では表せない。かといって `cfg!` を文言側へ
/// 散らすと **macOS 上から Windows 側の分岐を検証できなくなる**（#515 の方針に反する）。
/// そこで呼び名を値として持ち回し、OS を見るのは [`Device::detect`] の 1 か所だけにする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// macOS。「Mac」と呼び、蓋制御の手段は `pmset disablesleep`
    Mac,
    /// それ以外。「この PC」と呼ぶ
    Pc,
}

impl Device {
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Pc
        }
    }
}

impl Default for Device {
    fn default() -> Self {
        Self::detect()
    }
}

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
    /// バッテリー残量が下限以下・または残量が読めないため見送り中（#1473）
    PausedBatteryFloor,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// 本体の温度（macOS のみ観測できる。Windows は常に `Nominal`）。
    ///
    /// **真偽値ではなく状態そのもの**を持つ（#1473）。バッテリーで蓋を閉じ続けるときは
    /// `fair` でも降りるので、「警告かどうか」に潰すと判定が表せない
    pub thermal_state: ThermalState,
    /// 蓋閉じ継続の電源条件（#1473）
    pub lid_power_condition: PowerCondition,
    /// バッテリーで続けるときの残量下限（%。#1473）
    pub lid_battery_floor: u8,
    /// バッテリー残量（%。読めない環境は None。#1473）
    pub battery_percent: Option<u8>,
    /// 蓋閉じ継続が効いていない理由（#1473）。**判断したプロセス（アプリ）が
    /// 載せたものをそのまま読む**ので、画面が自分で計算し直すことはない
    pub lid_skip_reason: Option<LidSkipReason>,
    /// 文言がこの機械を何と呼ぶか
    pub device: Device,
}

impl Default for SleepSnapshot {
    fn default() -> Self {
        Self {
            idle_supported: false,
            mode: SleepGuardMode::default(),
            power_condition: PowerCondition::default(),
            assertion_held: false,
            busy_agents: 0,
            on_ac_power: false,
            lid_supported: false,
            lid_mode: LidSleepMode::default(),
            lid_active: false,
            lid_setup_required: false,
            lid_needs_privileged_setup: false,
            thermal_state: ThermalState::default(),
            lid_power_condition: PowerCondition::default(),
            // 0 にすると「下限なし」という別の意味になるので、既定は正本から引く
            lid_battery_floor: sleep_guard::DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: None,
            lid_skip_reason: None,
            device: Device::detect(),
        }
    }
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
            thermal_state: ThermalState::from_str_opt(s("thermal_state")).unwrap_or_default(),
            lid_power_condition: PowerCondition::from_str_opt(s("lid_power_condition"))
                .unwrap_or_default(),
            lid_battery_floor: v
                .get("lid_battery_floor")
                .and_then(|x| x.as_u64())
                .and_then(|x| u8::try_from(x).ok())
                .unwrap_or(sleep_guard::DEFAULT_LID_BATTERY_FLOOR),
            battery_percent: v
                .get("battery_percent")
                .and_then(|x| x.as_u64())
                .and_then(|x| u8::try_from(x).ok()),
            lid_skip_reason: v.get("lid_skip_reason").and_then(LidSkipReason::from_json),
            device: Device::detect(),
        }
    }

    /// 蓋閉じ継続の判定材料（#1473）。
    ///
    /// **画面は自前で条件を書かない**。`sleep_guard::lid_decision` へ渡して
    /// 返ってきた理由を表示へ写すだけにしておくと、安全弁を足しても
    /// 「実際は残量で降りているのに画面は AC 未接続と言う」がそもそも作れない
    fn lid_input(&self) -> LidGuardInput {
        LidGuardInput {
            lid_sleep_mode: self.lid_mode,
            setup_done: !self.lid_setup_required,
            busy_agents: self.busy_agents,
            on_ac: self.on_ac_power,
            thermal: self.thermal_state,
            lid_power_condition: self.lid_power_condition,
            battery_percent: self.battery_percent,
            battery_floor: self.lid_battery_floor,
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
    /// 登録も解除も無く、ボタンを出すと「押さないと使えない」と誤解させる
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
        // 真理値そのものは sleep_guard の 1 本を借りる（`該当理由と Applying の
        // 食い違い` テストが両者の一致を固定する）
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
        // 効いていない理由は**アプリが判断したもの**を読む（#1473）。ここで条件を
        // 書き直すと、安全弁を足したときに画面だけ古い理由を出す。
        // 理由が入っていない応答（取得前・古いアプリ）だけ材料から計算へ落ちる
        let decision = match self.lid_skip_reason {
            Some(reason) => Err(reason),
            None => sleep_guard::lid_decision(&self.lid_input()),
        };
        match decision {
            Err(LidSkipReason::ModeOff) => LidStatus::Off,
            Err(LidSkipReason::SetupRequired) => LidStatus::SetupRequired,
            Err(LidSkipReason::Thermal(_)) => LidStatus::PausedThermal,
            Err(LidSkipReason::NoAgents) => LidStatus::WaitingAgents,
            Err(LidSkipReason::NoAcPower) => LidStatus::PausedNoAc,
            Err(LidSkipReason::BatteryFloor { .. } | LidSkipReason::BatteryUnknown) => {
                LidStatus::PausedBatteryFloor
            }
            // 倒すべき条件は揃っているのにまだ倒れていない = 反映待ち
            Ok(()) => LidStatus::Applying,
        }
    }

    /// このタブの表示構成
    pub fn plan(&self) -> SleepTabPlan {
        let idle = self.idle_status();
        let lid = self.lid_status();
        let tone = |active: bool, known: bool| match (active, known) {
            (true, _) => StatusTone::Active,
            (false, true) => StatusTone::Known,
            (false, false) => StatusTone::Unknown,
        };
        SleepTabPlan {
            device: self.device,
            needs_privileged_setup: self.lid_needs_privileged_setup,
            show_lid_row: self.show_lid_row(),
            show_setup_buttons: self.show_setup_buttons(),
            status_rows: vec![
                StatusRow {
                    label: txt::sleep_status_idle_label(),
                    // エージェント数は「なぜその状態なのか」の補足なので、いるときだけ添える
                    note: if self.busy_agents > 0 {
                        txt::sleep_status_agents(self.busy_agents)
                    } else {
                        String::new()
                    },
                    value: txt::sleep_idle_status(idle).to_string(),
                    tone: tone(idle == IdleStatus::Active, idle != IdleStatus::Unsupported),
                },
                StatusRow {
                    label: txt::sleep_lid_header(),
                    note: String::new(),
                    value: txt::sleep_lid_status(lid).to_string(),
                    tone: tone(lid == LidStatus::Active, lid != LidStatus::Unsupported),
                },
                StatusRow {
                    label: txt::sleep_status_power_label(),
                    note: String::new(),
                    value: if self.on_ac_power {
                        txt::sleep_status_on_ac().to_string()
                    } else {
                        txt::sleep_status_on_battery().to_string()
                    },
                    tone: StatusTone::Known,
                },
            ]
            .into_iter()
            // 残量は読める機械でだけ出す（#1473）。読めない環境で「不明」の行を
            // 足しても情報がなく、デスクトップ機には意味のない行が増える
            .chain(self.battery_percent.map(|percent| StatusRow {
                label: txt::sleep_status_battery_label(),
                note: String::new(),
                value: txt::sleep_status_battery_value(percent, self.lid_battery_floor),
                // 下限以下は「いつ落ちるか」ではなく「もう効かない」の合図なので薄く出さない
                tone: if percent <= self.lid_battery_floor {
                    StatusTone::Unknown
                } else {
                    StatusTone::Known
                },
            }))
            .collect(),
        }
    }
}

/// 設定画面が並べる残量下限の候補（%。#1473）。
///
/// CLI / MCP は [`sleep_guard::parse_battery_floor`] の範囲（5〜90）を自由に受けるが、
/// 画面はよく使う値だけを並べる（数値入力を増やさない）。候補に無い値が設定されている
/// ときはどれも選択されない代わりに、状態セクションの残量行へ現在の下限が出る
/// 組の 2 つ目は**セグメントが送る値の綴り**（`Request` の `lid_battery_floor` は
/// 整数なので、画面側は文字列と数値の両方を要る）
pub const BATTERY_FLOOR_CHOICES: &[(u8, &str)] = &[(10, "10"), (20, "20"), (30, "30"), (50, "50")];

/// 状態の値をどの濃さで出すか（実際の色は描画側が Theme から引く）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// いま効いている
    Active,
    /// 効いていないが理由は分かっている
    Known,
    /// この OS では使えない・まだ分からない
    Unknown,
}

/// 「いまの状態」セクションの 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub label: &'static str,
    /// 補足（無ければ空文字）
    pub note: String,
    /// 表示値。**固定文言とは限らない**（バッテリー残量のような実測値も出る。#1473）
    pub value: String,
    pub tone: StatusTone,
}

/// スリープ防止タブの表示構成
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepTabPlan {
    pub device: Device,
    /// 蓋閉じ継続に管理者権限を伴う初回登録が要る仕組みか（説明文の出し分けに使う）
    pub needs_privileged_setup: bool,
    pub show_lid_row: bool,
    pub show_setup_buttons: bool,
    pub status_rows: Vec<StatusRow>,
}

impl SleepTabPlan {
    /// この構成でタブに出る文字列すべて。
    ///
    /// **`settings_window::render_sleep_tab` が描く文字列と同じ集合**を返す
    /// （どちらかに文字列を足したらもう一方にも足す）。こう並べておくと
    /// 「Windows に macOS 固有の語が出ない」を GUI を起こさずに機械検査できる
    pub fn visible_texts(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![
            txt::sleep_status_header().into(),
            txt::button_refresh().into(),
            txt::sleep_mode_header().into(),
            txt::desc_sleep_mode(self.device).into(),
            txt::sleep_mode_off().into(),
            txt::sleep_mode_on().into(),
            txt::sleep_mode_agents().into(),
            txt::sleep_power_header().into(),
            txt::desc_sleep_power().into(),
            txt::sleep_power_ac().into(),
            txt::sleep_power_always().into(),
        ];
        for row in &self.status_rows {
            out.push(row.label.into());
            out.push(row.value.clone());
            if !row.note.is_empty() {
                out.push(row.note.clone());
            }
        }
        if self.show_lid_row {
            out.push(txt::sleep_lid_header().into());
            out.push(txt::desc_sleep_lid(self.needs_privileged_setup).into());
            // #1473: 蓋閉じ継続の電源条件と残量下限（バッテリーで続けるかどうか）
            out.push(txt::sleep_lid_power_header().into());
            out.push(txt::desc_sleep_lid_power().into());
            out.push(txt::sleep_power_ac().into());
            out.push(txt::sleep_lid_power_always().into());
            out.push(txt::sleep_lid_floor_header().into());
            out.push(txt::desc_sleep_lid_floor().into());
            for (percent, _) in BATTERY_FLOOR_CHOICES {
                out.push(txt::sleep_lid_floor_choice(*percent));
            }
        }
        if self.show_setup_buttons {
            out.push(txt::sleep_lid_install().into());
            out.push(txt::sleep_lid_remove().into());
            // ボタンを押した結果として出る文言もこの構成でしか見えない
            out.push(txt::msg_lid_installed().into());
            out.push(txt::msg_lid_removed().into());
        }
        out
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
            device: Device::Pc,
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
            device: Device::Mac,
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
            thermal_state: ThermalState::Serious,
            lid_setup_required: false,
            ..mac()
        };
        assert_eq!(s.lid_status(), LidStatus::PausedThermal);
    }

    // --- 状態の切り分けが sleep_guard の判定と食い違わない ---
    //
    // 「反映中」と「AC 未接続 / エージェント待ち」の境目は sleep_guard が持つ
    // `should_hold_assertion` / `should_disable_lid_sleep` と同じでなければならない。
    // コメントで「揃える」と書くだけでは片方を直したときに気づけないので、
    // 状態空間を総当たりして固定する

    #[test]
    fn idleの反映中はsleep_guardの判定と一致する() {
        for mode in [
            SleepGuardMode::Off,
            SleepGuardMode::On,
            SleepGuardMode::WhileAgentsRunning,
        ] {
            for power in [PowerCondition::AcOnly, PowerCondition::Always] {
                for on_ac in [false, true] {
                    for busy in [0usize, 1, 3] {
                        let s = SleepSnapshot {
                            mode,
                            power_condition: power,
                            on_ac_power: on_ac,
                            busy_agents: busy,
                            assertion_held: false,
                            ..win()
                        };
                        let should = sleep_guard::should_hold_assertion(mode, power, on_ac, busy);
                        assert_eq!(
                            s.idle_status() == IdleStatus::Applying,
                            should,
                            "保持すべきかの判定と表示が食い違う: {mode:?} {power:?} ac={on_ac} busy={busy} -> {:?}",
                            s.idle_status()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lidの反映中はsleep_guardの判定と一致する() {
        // #1473 で電源条件と残量も判定に入ったので、総当たりの軸もそれに合わせる
        for lid_mode in [LidSleepMode::Off, LidSleepMode::WhileAgentsRunning] {
            for setup_required in [false, true] {
                for busy in [0usize, 2] {
                    for on_ac in [false, true] {
                        for thermal in [
                            ThermalState::Nominal,
                            ThermalState::Fair,
                            ThermalState::Serious,
                        ] {
                            for lid_power in [PowerCondition::AcOnly, PowerCondition::Always] {
                                for battery in [None, Some(5u8), Some(20), Some(80)] {
                                    let s = SleepSnapshot {
                                        lid_mode,
                                        lid_setup_required: setup_required,
                                        busy_agents: busy,
                                        on_ac_power: on_ac,
                                        thermal_state: thermal,
                                        lid_power_condition: lid_power,
                                        battery_percent: battery,
                                        lid_active: false,
                                        ..mac()
                                    };
                                    let should =
                                        sleep_guard::should_disable_lid_sleep(&s.lid_input());
                                    assert_eq!(
                                        s.lid_status() == LidStatus::Applying,
                                        should,
                                        "倒すべきかの判定と表示が食い違う: {lid_mode:?} setup_required={setup_required} busy={busy} ac={on_ac} thermal={thermal:?} lid_power={lid_power:?} battery={battery:?} -> {:?}",
                                        s.lid_status()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // --- #1473: バッテリー駆動の蓋閉じ継続 ---

    /// 既定（`ac-only`）はバッテリーで降りる。表示も従来どおり「AC 未接続」
    #[test]
    fn 既定ではバッテリーで蓋閉じ継続に入らない() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            busy_agents: 1,
            on_ac_power: false,
            battery_percent: Some(90),
            ..win()
        };
        assert_eq!(
            s.lid_power_condition,
            PowerCondition::AcOnly,
            "既定は AC のみ"
        );
        assert_eq!(s.lid_status(), LidStatus::PausedNoAc);
    }

    #[test]
    fn alwaysならバッテリーでも反映中まで進む() {
        let s = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            lid_power_condition: PowerCondition::Always,
            busy_agents: 1,
            on_ac_power: false,
            battery_percent: Some(90),
            ..win()
        };
        assert_eq!(s.lid_status(), LidStatus::Applying);
    }

    #[test]
    fn 残量が下限以下なら残量の理由で止まる() {
        let base = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            lid_power_condition: PowerCondition::Always,
            busy_agents: 1,
            on_ac_power: false,
            lid_battery_floor: 20,
            ..win()
        };
        // ちょうど下限は「割った」側（下限に達したら降りる）
        for percent in [0u8, 19, 20] {
            let s = SleepSnapshot {
                battery_percent: Some(percent),
                ..base
            };
            assert_eq!(
                s.lid_status(),
                LidStatus::PausedBatteryFloor,
                "残量 {percent}% で止まらない"
            );
        }
        assert_eq!(
            SleepSnapshot {
                battery_percent: Some(21),
                ..base
            }
            .lid_status(),
            LidStatus::Applying,
            "下限より上なら続ける"
        );
        // 読めない環境も同じ枝（続けられないことに変わりはない）
        assert_eq!(
            SleepSnapshot {
                battery_percent: None,
                ..base
            }
            .lid_status(),
            LidStatus::PausedBatteryFloor
        );
    }

    #[test]
    fn バッテリー継続中はfairでも温度で降りる() {
        let battery = SleepSnapshot {
            lid_mode: LidSleepMode::WhileAgentsRunning,
            lid_power_condition: PowerCondition::Always,
            busy_agents: 1,
            on_ac_power: false,
            battery_percent: Some(90),
            thermal_state: ThermalState::Fair,
            ..win()
        };
        assert_eq!(battery.lid_status(), LidStatus::PausedThermal);
        // AC 接続中は従来どおり fair では降りない
        let on_ac = SleepSnapshot {
            on_ac_power: true,
            ..battery
        };
        assert_eq!(on_ac.lid_status(), LidStatus::Applying);
    }

    #[test]
    fn 残量の行は読める機械にだけ出る() {
        let with_battery = SleepSnapshot {
            battery_percent: Some(42),
            lid_battery_floor: 20,
            ..win()
        };
        let rows = with_battery.plan().status_rows;
        assert!(
            rows.iter().any(|r| r.value.contains("42")),
            "残量の行が無い: {rows:?}"
        );
        let without = SleepSnapshot {
            battery_percent: None,
            ..win()
        };
        assert_eq!(
            without.plan().status_rows.len(),
            rows.len() - 1,
            "読めない機械に残量の行を足してはいけない"
        );
    }

    // --- 画面に出る文字列（#727 の受け入れ条件 1 を GUI 無しで固定する） ---

    /// macOS 固有の語。Windows の画面に出たらそれが症状そのもの
    const MAC_ONLY_WORDS: &[&str] = &["sudoers", "pmset", "Mac"];

    #[test]
    fn windowsの画面にmacos固有の語が出ない() {
        crate::ui_text::tests_support::for_each_lang(|| {
            let texts = win().plan().visible_texts();
            for t in &texts {
                for w in MAC_ONLY_WORDS {
                    assert!(
                        !t.contains(w),
                        "Windows の設定画面に macOS 固有の語 {w:?} が出ている: {t:?}"
                    );
                }
            }
            // 「何も出ていないから通った」を防ぐ: 状態と切替の文字列は必ずある
            assert!(texts.len() >= 14, "表示文字列が少なすぎる: {texts:?}");
        });
    }

    #[test]
    fn macosの画面にはsudoersの案内が残る() {
        crate::ui_text::tests_support::for_each_lang(|| {
            let texts = mac().plan().visible_texts();
            assert!(
                texts.iter().any(|t| t.contains("sudoers")),
                "macOS では sudoers 登録の案内が要る: {texts:?}"
            );
        });
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
        assert_eq!(s.thermal_state, ThermalState::Serious);
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
        assert_eq!(s.thermal_state, ThermalState::Nominal);
        // #1473: 欠けていても下限は 0（= 安全弁なし）へ落とさない
        assert_eq!(s.lid_battery_floor, sleep_guard::DEFAULT_LID_BATTERY_FLOOR);
        assert_eq!(s.lid_power_condition, PowerCondition::AcOnly);
    }

    #[test]
    fn 初回セットアップの要否と呼び名はosから入る() {
        // JSON には無い（登録済みかどうかでは変わらない OS 固定の性質）
        let s = SleepSnapshot::from_status_json(Some(&serde_json::json!({})));
        assert_eq!(
            s.lid_needs_privileged_setup,
            cfg!(target_os = "macos"),
            "macOS だけが sudoers 登録という初回セットアップを持つ"
        );
        assert_eq!(
            s.device == Device::Mac,
            cfg!(target_os = "macos"),
            "呼び名も OS から入る"
        );
    }
}

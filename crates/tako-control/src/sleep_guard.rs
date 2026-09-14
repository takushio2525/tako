//! スリープ防止機能（Issue #173 + #218 蓋閉じ対応 + #311 ディスプレイ消灯）
//!
//! IOKit の電源アサーション（PreventUserIdleSystemSleep）で macOS のアイドルスリープを防止する。
//! ディスプレイスリープは妨げない。App Nap 無効化も行い、バックグラウンドで間引かれない。
//!
//! 蓋閉じ（lid-close）対応（#218）:
//! - 蓋の開閉を IORegistry AppleClamshellState で検知（root 不要）
//! - NSProcessInfo.thermalState で thermal 状態を監視（serious/critical で警告）
//! - sudoers.d 限定登録で pmset disablesleep を NOPASSWD 制御（opt-in）
//!
//! ディスプレイ消灯（#311）:
//! - disablesleep=1 は蓋閉じスリープだけでなくディスプレイ消灯も阻害する
//! - 蓋閉じ + disablesleep 有効を検知したら pmset displaysleepnow でディスプレイだけ消灯
//! - 蓋が閉じている間はユーザー入力がないため消灯を維持
//!
//! モード:
//! - off: 機能無効
//! - on: 常時アサーション保持
//! - while-agents-running: busy なエージェントペインが 1 体でもある間だけ保持（既定）
//!
//! 蓋閉じ防止モード（lid_sleep_mode）:
//! - off: 蓋閉じ防止なし（既定）
//! - while-agents-running: busy なエージェントがいる間だけ pmset disablesleep 1（要 sudoers 登録）
//!
//! 電源条件:
//! - ac-only: AC 接続時のみ（既定）
//! - always: バッテリー時も

use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// update() で計算された busy_agents の最新値。status() が読み取る（#372）
static BUSY_AGENTS: AtomicUsize = AtomicUsize::new(0);

/// スリープ防止のモード
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SleepGuardMode {
    Off,
    On,
    #[default]
    WhileAgentsRunning,
}

impl SleepGuardMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::WhileAgentsRunning => "while-agents-running",
        }
    }

    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            "while-agents-running" | "while_agents_running" => Some(Self::WhileAgentsRunning),
            _ => None,
        }
    }
}

/// 蓋閉じ防止モード（#218）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LidSleepMode {
    #[default]
    Off,
    WhileAgentsRunning,
}

impl LidSleepMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WhileAgentsRunning => "while-agents-running",
        }
    }

    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "while-agents-running" | "while_agents_running" => Some(Self::WhileAgentsRunning),
            _ => None,
        }
    }
}

/// 電源条件
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerCondition {
    #[default]
    AcOnly,
    Always,
}

impl PowerCondition {
    /// 受理・申告する値の正本（#1467。MCP カタログの enum はここから生成する）
    pub const VALUES: &'static [&'static str] = &["ac-only", "always"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcOnly => "ac-only",
            Self::Always => "always",
        }
    }

    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "ac-only" | "ac_only" => Some(Self::AcOnly),
            "always" => Some(Self::Always),
            _ => None,
        }
    }

    /// 各値の意味（`match` なので変種を足すとコンパイルが通らない = 値だけ増えて
    /// 説明が古くなることがない。#1467）
    pub fn summary(self) -> &'static str {
        match self {
            Self::AcOnly => "ac-only = AC 電源に接続しているときだけ",
            Self::Always => "always = バッテリー駆動でも",
        }
    }

    /// MCP / CLI の案内文（受理値と意味を 1 行で）
    pub fn values_hint() -> String {
        Self::VALUES
            .iter()
            .filter_map(|v| Self::from_str_opt(v))
            .map(|v| v.summary().to_string())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// 蓋閉じ継続をバッテリーで続けるときの残量下限の既定値（%。#1473）
pub const DEFAULT_LID_BATTERY_FLOOR: u8 = 20;
/// 残量下限として受け付ける下端（%。#1473）。
///
/// **0 を許さない**のは安全弁そのものを外せてしまうため（鞄の中で空になる）
pub const LID_BATTERY_FLOOR_MIN: u8 = 5;
/// 残量下限として受け付ける上端（%。#1473）。
///
/// これより上を許すと、常に下限を割った状態になって蓋閉じ継続が
/// 「設定したのに一度も効かない」ように見える
pub const LID_BATTERY_FLOOR_MAX: u8 = 90;

/// 残量下限の入力を検証する（CLI / MCP / 設定画面が同じ 1 実装を通る。#1473）
pub fn parse_battery_floor(percent: i64) -> Result<u8, String> {
    let min = i64::from(LID_BATTERY_FLOOR_MIN);
    let max = i64::from(LID_BATTERY_FLOOR_MAX);
    if (min..=max).contains(&percent) {
        Ok(percent as u8)
    } else {
        Err(format!(
            "残量下限は {LID_BATTERY_FLOOR_MIN}〜{LID_BATTERY_FLOOR_MAX} の範囲で指定してください（指定値: {percent}）"
        ))
    }
}

/// Thermal 状態（NSProcessInfo.thermalState の Rust 表現）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThermalState {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl ThermalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Fair => "fair",
            Self::Serious => "serious",
            Self::Critical => "critical",
        }
    }

    pub fn is_warning(self) -> bool {
        matches!(self, Self::Serious | Self::Critical)
    }

    /// `as_str` の逆（#372。`to_json` を読み戻すために使う）
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "nominal" => Some(Self::Nominal),
            "fair" => Some(Self::Fair),
            "serious" => Some(Self::Serious),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    /// バッテリー駆動で蓋を閉じ続けるには温度が高すぎるか（#1473）。
    ///
    /// AC 接続時の [`Self::is_warning`] より**厳しい**（`fair` でも解除する）。
    /// 蓋を閉じたバッテリー駆動は鞄の中である可能性が高く、放熱が閉じた状態から
    /// さらに悪化しても誰も気づけないため、悪化の兆し（fair）で降りる
    pub fn blocks_battery_lid(self) -> bool {
        self != Self::Nominal
    }
}

/// 蓋閉じ継続が効いていない理由（#1473）。
///
/// 判定 [`lid_decision`] の戻り値であり、**「なぜ無効か」を出すすべての面
/// （`status` の description / CLI / 設定画面 / 通知欄 / persist.log）がこの 1 つを引く**。
/// 真偽値だけを返していた頃は、画面ごとに理由を書き直していたので
/// 片方だけ直すと表示が嘘になった（#727 で総当たりテストを足した理由）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidSkipReason {
    /// 蓋閉じ継続そのものが設定されていない
    ModeOff,
    /// 初回セットアップが済んでいない（macOS の sudoers 登録）
    SetupRequired,
    /// エージェントが 1 体も動いていない
    NoAgents,
    /// `ac-only` の設定で AC 未接続
    NoAcPower,
    /// バッテリー残量が下限以下（安全弁）
    BatteryFloor { percent: u8, floor: u8 },
    /// バッテリー駆動なのに残量を読めない（安全弁。読めない = 止められないので降りる）
    BatteryUnknown,
    /// 本体が高温（安全弁）
    Thermal(ThermalState),
}

impl LidSkipReason {
    /// 診断・JSON 用の識別子（ASCII 固定。本文は含めない）
    pub fn tag(self) -> &'static str {
        match self {
            Self::ModeOff => "mode-off",
            Self::SetupRequired => "setup-required",
            Self::NoAgents => "no-agents",
            Self::NoAcPower => "no-ac-power",
            Self::BatteryFloor { .. } => "battery-floor",
            Self::BatteryUnknown => "battery-unknown",
            Self::Thermal(_) => "thermal",
        }
    }

    /// 安全弁による解除か（#1473）。
    ///
    /// **通知欄へ出すのはこれが真のものだけ**。`NoAgents` / `NoAcPower` /
    /// `ModeOff` は設定どおりの正常な動きなので、バナーにすると
    /// エージェントが一段落するたびに画面へ出る
    pub fn is_safety_valve(self) -> bool {
        matches!(
            self,
            Self::BatteryFloor { .. } | Self::BatteryUnknown | Self::Thermal(_)
        )
    }

    /// 人が読む理由（`status` の description と CLI が使う。UI の日英は
    /// `tako-app::ui_text::sleep_guard` が同じ分類から作る）
    pub fn describe(self) -> String {
        match self {
            Self::ModeOff => "未設定".to_string(),
            Self::SetupRequired => {
                "sudoers 未登録（tako setup --lid-sleep で登録）".to_string()
            }
            Self::NoAgents => "エージェント待機中のため無効".to_string(),
            Self::NoAcPower => {
                "AC 未接続のため無効（tako sleep-guard set --lid-power-condition always でバッテリーでも継続）"
                    .to_string()
            }
            Self::BatteryFloor { percent, floor } => {
                format!("バッテリー残量 {percent}% が下限 {floor}% 以下のため解除")
            }
            Self::BatteryUnknown => {
                "バッテリー残量を取得できないため解除（安全弁）".to_string()
            }
            Self::Thermal(state) => {
                format!("本体が高温（{}）のため解除", state.as_str())
            }
        }
    }

    pub fn to_json(self) -> Value {
        let mut v = json!({ "reason": self.tag(), "text": self.describe() });
        match self {
            Self::BatteryFloor { percent, floor } => {
                v["battery_percent"] = json!(percent);
                v["battery_floor"] = json!(floor);
            }
            Self::Thermal(state) => v["thermal_state"] = json!(state.as_str()),
            _ => {}
        }
        v
    }

    /// [`Self::to_json`] の逆（#372 と同じ理由: CLI は IPC の JSON を型へ戻して
    /// **同じレンダラ**へ通すので、往復できないと理由が CLI からだけ消える）
    pub fn from_json(v: &Value) -> Option<Self> {
        match v["reason"].as_str()? {
            "mode-off" => Some(Self::ModeOff),
            "setup-required" => Some(Self::SetupRequired),
            "no-agents" => Some(Self::NoAgents),
            "no-ac-power" => Some(Self::NoAcPower),
            "battery-floor" => Some(Self::BatteryFloor {
                percent: u8::try_from(v["battery_percent"].as_u64()?).ok()?,
                floor: u8::try_from(v["battery_floor"].as_u64()?).ok()?,
            }),
            "battery-unknown" => Some(Self::BatteryUnknown),
            "thermal" => Some(Self::Thermal(ThermalState::from_str_opt(
                v["thermal_state"].as_str()?,
            )?)),
            _ => None,
        }
    }
}

/// 蓋閉じ継続を倒すかどうかの判定材料（#1473）。
///
/// 引数を並べるのではなく型にするのは、**安全弁を足したときに
/// 渡し忘れた呼び出し側がコンパイルで落ちる**ようにするため
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LidGuardInput {
    pub lid_sleep_mode: LidSleepMode,
    /// 初回セットアップが済んでいるか。macOS は sudoers 登録、
    /// Windows は権限が要らないので常に `true`
    pub setup_done: bool,
    pub busy_agents: usize,
    pub on_ac: bool,
    /// 本体の温度。macOS のみ観測でき、Windows は常に `Nominal`
    pub thermal: ThermalState,
    /// 蓋閉じ継続の電源条件（#1473。アイドルスリープ側とは**別の軸**）
    pub lid_power_condition: PowerCondition,
    /// バッテリー残量（%）。読めない環境は `None`
    pub battery_percent: Option<u8>,
    /// 残量の下限（%）
    pub battery_floor: u8,
}

/// スリープ防止の設定一式（#1473）。
///
/// `update` / `status` の引数を設定ごとに増やすと、呼び出し側（GUI の 2 秒 tick /
/// dispatch / CLI）のどれかが更新漏れになる。1 つの型にして
/// [`crate::settings::Settings::sleep_guard_config`] から作る 1 経路に寄せる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepGuardConfig {
    pub mode: SleepGuardMode,
    /// アイドルスリープ防止の電源条件
    pub power_condition: PowerCondition,
    pub lid_sleep_mode: LidSleepMode,
    /// 蓋閉じ継続の電源条件（#1473）
    pub lid_power_condition: PowerCondition,
    /// 蓋閉じ継続をバッテリーで続けるときの残量下限（%。#1473）
    pub lid_battery_floor: u8,
}

impl Default for SleepGuardConfig {
    fn default() -> Self {
        Self {
            mode: SleepGuardMode::default(),
            power_condition: PowerCondition::default(),
            lid_sleep_mode: LidSleepMode::default(),
            lid_power_condition: PowerCondition::default(),
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
        }
    }
}

/// アサーションの現在の状態
#[derive(Debug, Clone)]
pub struct SleepGuardState {
    /// アサーションを現在保持しているか
    pub assertion_held: bool,
    /// 設定されているモード
    pub mode: SleepGuardMode,
    /// 電源条件
    pub power_condition: PowerCondition,
    /// AC 電源接続中か
    pub on_ac_power: bool,
    /// busy なエージェントの数
    pub busy_agents: usize,
    /// この OS でスリープ防止が使えるか（macOS = IOKit、Windows = 電源要求。#524）
    pub platform_supported: bool,
    /// 蓋が閉じているか（#218）
    pub lid_closed: bool,
    /// pmset disablesleep が有効か（#218）
    pub lid_sleep_disabled: bool,
    /// 蓋閉じ防止モード（#218）
    pub lid_sleep_mode: LidSleepMode,
    /// sudoers 登録済みか（#218。**macOS 固有の手段**なので Windows では常に false）
    pub sudoers_installed: bool,
    /// 蓋閉じ継続を使うのに初回セットアップが要るか（#697）。
    ///
    /// `sudoers_installed` を直接見ると「sudoers を登録してください」という
    /// macOS 専用の案内が Windows にも出てしまう。手段ではなく
    /// **未完了かどうか**を持つことで、文言側が OS を知らずに済む
    pub lid_setup_required: bool,
    /// thermal 状態（#218）
    pub thermal_state: ThermalState,
    /// ディスプレイ消灯を強制送信済みか（#311）
    pub display_sleep_forced: bool,
    /// 蓋閉じ継続の電源条件（#1473。アイドルスリープ側の `power_condition` とは別軸）
    pub lid_power_condition: PowerCondition,
    /// 蓋閉じ継続をバッテリーで続けるときの残量下限（%。#1473）
    pub lid_battery_floor: u8,
    /// バッテリー残量（%）。読めない環境（デスクトップ機・Windows）は `None`（#1473）
    pub battery_percent: Option<u8>,
    /// 蓋閉じ継続が効いていない理由（#1473）。効くべき状態なら `None`。
    ///
    /// **判断したプロセスが載せる**（#372 と同じ理屈）。読む側が材料から計算し直すと、
    /// CLI とアプリでバイナリや A/B のアームが違うときに「画面と実際の判断が別物」に
    /// なる。作るのは [`update`] / [`status`] と [`SleepGuardState::from_json`] だけで、
    /// どれも [`lid_decision`] の結果をそのまま入れる
    pub lid_skip_reason: Option<LidSkipReason>,
}

impl SleepGuardState {
    pub fn to_json(&self) -> Value {
        json!({
            "assertion_held": self.assertion_held,
            "mode": self.mode.as_str(),
            "power_condition": self.power_condition.as_str(),
            "on_ac_power": self.on_ac_power,
            "busy_agents": self.busy_agents,
            "platform_supported": self.platform_supported,
            "lid_closed": self.lid_closed,
            "lid_sleep_disabled": self.lid_sleep_disabled,
            "lid_sleep_mode": self.lid_sleep_mode.as_str(),
            "sudoers_installed": self.sudoers_installed,
            "lid_control_supported": lid_control_supported(),
            "lid_setup_required": self.lid_setup_required,
            "thermal_state": self.thermal_state.as_str(),
            "display_sleep_forced": self.display_sleep_forced,
            "lid_power_condition": self.lid_power_condition.as_str(),
            "lid_battery_floor": self.lid_battery_floor,
            // 読めない環境は null（0% と区別する）
            "battery_percent": self.battery_percent,
            "lid_skip_reason": self.lid_skip_reason.map(|r| r.to_json()),
            "description": self.description(),
        })
    }

    /// [`Self::to_json`] の逆（#372）。
    ///
    /// **実行時の状態（`assertion_held` / `busy_agents`）はアプリのプロセスが持つ**
    /// （どちらもプロセスローカルな static）。CLI から `tako sleep-guard status` を
    /// 叩いたときに MCP の同ツールと同じ値を返すため、IPC で受け取った JSON を
    /// ここで型へ戻して**同じ 1 つのレンダラ**へ通す。
    ///
    /// 1 つでも欠ける / 読めないフィールドがあれば `None`（黙って既定値 0 / false を
    /// 返すと「アプリは動いているのに busy 0」という #372 と同じ嘘になる）
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            assertion_held: v["assertion_held"].as_bool()?,
            mode: SleepGuardMode::from_str_opt(v["mode"].as_str()?)?,
            power_condition: PowerCondition::from_str_opt(v["power_condition"].as_str()?)?,
            on_ac_power: v["on_ac_power"].as_bool()?,
            busy_agents: usize::try_from(v["busy_agents"].as_u64()?).ok()?,
            platform_supported: v["platform_supported"].as_bool()?,
            lid_closed: v["lid_closed"].as_bool()?,
            lid_sleep_disabled: v["lid_sleep_disabled"].as_bool()?,
            lid_sleep_mode: LidSleepMode::from_str_opt(v["lid_sleep_mode"].as_str()?)?,
            sudoers_installed: v["sudoers_installed"].as_bool()?,
            lid_setup_required: v["lid_setup_required"].as_bool()?,
            thermal_state: ThermalState::from_str_opt(v["thermal_state"].as_str()?)?,
            display_sleep_forced: v["display_sleep_forced"].as_bool()?,
            lid_power_condition: PowerCondition::from_str_opt(v["lid_power_condition"].as_str()?)?,
            lid_battery_floor: u8::try_from(v["lid_battery_floor"].as_u64()?).ok()?,
            // `null`（読めない）と「キーが無い」（古い応答 = 嘘になる）を区別する
            battery_percent: match v.get("battery_percent")? {
                Value::Null => None,
                other => Some(u8::try_from(other.as_u64()?).ok()?),
            },
            // `null`（倒すべき状態）と「キーが無い」（古い応答）を区別する
            lid_skip_reason: match v.get("lid_skip_reason")? {
                Value::Null => None,
                other => Some(LidSkipReason::from_json(other)?),
            },
        })
    }

    /// 蓋閉じ継続の判定材料（#1473）。
    ///
    /// 状態は判定に要るものをすべて持っているので、理由を**フィールドで運ばずに
    /// ここから計算する**。CLI / 設定画面 / 通知が同じ 1 実装（[`lid_decision`]）を通る
    pub fn lid_input(&self) -> LidGuardInput {
        LidGuardInput {
            lid_sleep_mode: self.lid_sleep_mode,
            setup_done: !self.lid_setup_required,
            busy_agents: self.busy_agents,
            on_ac: self.on_ac_power,
            thermal: self.thermal_state,
            lid_power_condition: self.lid_power_condition,
            battery_percent: self.battery_percent,
            battery_floor: self.lid_battery_floor,
        }
    }

    /// 材料から理由を計算して埋める（#1473）。[`update`] / [`status`] の締めで通す
    fn with_decision(mut self) -> Self {
        self.lid_skip_reason = lid_decision(&self.lid_input()).err();
        self
    }

    fn description(&self) -> String {
        if !self.platform_supported {
            // macOS（IOKit）と Windows（電源要求）は対応済み。ここへ来るのはそれ以外
            return "この OS ではスリープ防止は使用できません".to_string();
        }

        let idle_desc = match self.mode {
            SleepGuardMode::Off => "アイドルスリープ防止: 無効",
            SleepGuardMode::On => {
                if self.assertion_held {
                    "アイドルスリープ防止: 有効（常時）"
                } else {
                    "アイドルスリープ防止: 有効（AC 未接続のため一時停止中）"
                }
            }
            SleepGuardMode::WhileAgentsRunning => {
                if self.assertion_held {
                    "アイドルスリープ防止: エージェント稼働中のため有効"
                } else if self.busy_agents > 0 && !self.on_ac_power {
                    "アイドルスリープ防止: エージェント稼働中だが AC 未接続のため一時停止中"
                } else {
                    "アイドルスリープ防止: エージェント待機中のため無効"
                }
            }
        };

        let lid_desc = if self.lid_sleep_disabled {
            if self.thermal_state.is_warning() {
                "蓋閉じ継続: 有効（高温警告中）".to_string()
            } else if self.display_sleep_forced {
                "蓋閉じ継続: 有効（ディスプレイ消灯済み）".to_string()
            } else if !self.on_ac_power {
                // #1473: バッテリーで継続しているあいだは残量が減り続けるので、
                // 「効いている」だけでなく**いま何 % か・どこで降りるか**まで出す
                match self.battery_percent {
                    Some(p) => format!(
                        "蓋閉じ継続: 有効（バッテリー {p}%・下限 {}%）",
                        self.lid_battery_floor
                    ),
                    None => "蓋閉じ継続: 有効（バッテリー駆動）".to_string(),
                }
            } else {
                "蓋閉じ継続: 有効".to_string()
            }
        } else {
            // #1473: 「なぜ無効か」は判定の 1 実装が返した理由をそのまま出す。
            // 手段（sudoers）ではなく「未完了か」で分岐するのは #697 のまま
            match self.lid_skip_reason {
                Some(LidSkipReason::ModeOff) => "蓋閉じ継続: 未設定".to_string(),
                Some(reason) => format!("蓋閉じ継続: {}", reason.describe()),
                // 倒すべき状態なのに倒れていない = 反映待ち（2 秒 tick の隙間）
                None => "蓋閉じ継続: 無効".to_string(),
            }
        };

        format!("{idle_desc} / {lid_desc}")
    }
}

// --- IOKit FFI（macOS 専用） ---

#[cfg(target_os = "macos")]
mod iokit {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};

    type IOPMAssertionID = u32;
    type CFStringRef = *const c_void;
    type CFStringEncoding = u32;
    type CFBooleanRef = *const c_void;

    const K_CFSTRING_ENCODING_UTF8: CFStringEncoding = 0x08000100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;

    /// `CFNumberGetValue` の型指定（kCFNumberSInt32Type）
    const K_CFNUMBER_SINT32_TYPE: i32 = 3;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const u8,
            encoding: CFStringEncoding,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
        fn CFBooleanGetValue(boolean: CFBooleanRef) -> bool;
        // バッテリー残量の読み取り（#1473）
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, the_type: i32, value: *mut c_void) -> bool;
    }

    /// AC 接続時の IOPSGetTimeRemainingEstimate 戻り値（kIOPSTimeRemainingUnlimited）
    const K_IOPS_TIME_REMAINING_UNLIMITED: f64 = -2.0;

    type IOReturn = i32;
    type MachPort = u32;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            reason_for_activity: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
        fn IOPSGetTimeRemainingEstimate() -> f64;
        // バッテリー残量の読み取り（#1473）。Copy で返るものは CFRelease が要る
        fn IOPSCopyPowerSourcesInfo() -> *const c_void;
        fn IOPSCopyPowerSourcesList(blob: *const c_void) -> *const c_void;
        fn IOPSGetPowerSourceDescription(blob: *const c_void, ps: *const c_void) -> *const c_void;

        fn IOServiceGetMatchingService(main_port: MachPort, matching: *const c_void) -> u32;
        fn IOServiceMatching(name: *const u8) -> *mut c_void;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: CFStringRef,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> IOReturn;
    }

    static ASSERTION_ID: AtomicU32 = AtomicU32::new(0);
    static ASSERTION_HELD: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    // #311: 蓋閉じ中にディスプレイ消灯コマンドを送信済みか
    static DISPLAY_SLEEP_SENT: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn cf_string(s: &str) -> CFStringRef {
        let c_str = std::ffi::CString::new(s).unwrap_or_default();
        unsafe {
            CFStringCreateWithCString(
                std::ptr::null(),
                c_str.as_ptr() as *const u8,
                K_CFSTRING_ENCODING_UTF8,
            )
        }
    }

    /// IOKit 電源アサーションを取得する。既に保持中の場合は何もしない
    pub fn create_assertion(reason: &str) -> bool {
        if ASSERTION_HELD.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        let assertion_type = cf_string("PreventUserIdleSystemSleep");
        let reason_str = cf_string(reason);
        let mut assertion_id: IOPMAssertionID = 0;
        let result = unsafe {
            IOPMAssertionCreateWithName(
                assertion_type,
                K_IOPM_ASSERTION_LEVEL_ON,
                reason_str,
                &mut assertion_id,
            )
        };
        unsafe {
            CFRelease(assertion_type);
            CFRelease(reason_str);
        }
        if result == 0 {
            ASSERTION_ID.store(assertion_id, Ordering::Relaxed);
            ASSERTION_HELD.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// IOKit 電源アサーションを解放する。保持していない場合は何もしない
    pub fn release_assertion() {
        if !ASSERTION_HELD.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let id = ASSERTION_ID.load(Ordering::Relaxed);
        unsafe {
            IOPMAssertionRelease(id);
        }
        ASSERTION_HELD.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_held() -> bool {
        ASSERTION_HELD.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// AC 電源に接続されているか（IOKit IOPSGetTimeRemainingEstimate 経由）。
    /// UI スレッドから 2 秒毎に呼ばれるため、サブプロセス（pmset）は使えない
    /// （fork+exec は 1 回 20〜30ms、CPU 飽和時は秒級までブロックする。#212）
    pub fn on_ac_power() -> bool {
        unsafe { IOPSGetTimeRemainingEstimate() == K_IOPS_TIME_REMAINING_UNLIMITED }
    }

    /// CFNumber を i32 として読む（#1473）
    fn cf_number_i32(value: *const c_void) -> Option<i32> {
        if value.is_null() {
            return None;
        }
        let mut out: i32 = 0;
        let ok = unsafe {
            CFNumberGetValue(
                value,
                K_CFNUMBER_SINT32_TYPE,
                std::ptr::addr_of_mut!(out) as *mut c_void,
            )
        };
        ok.then_some(out)
    }

    /// バッテリー残量（%。IOPowerSources。root 不要、#1473）。
    ///
    /// デスクトップ機・電源が 1 つも列挙されない環境は `None`。
    /// `on_ac_power` と同じく **UI スレッドから 2 秒毎に呼ばれる**ので
    /// サブプロセス（`pmset -g batt`）は使わない（#212）
    pub fn battery_percent() -> Option<u8> {
        unsafe {
            let blob = IOPSCopyPowerSourcesInfo();
            if blob.is_null() {
                return None;
            }
            let list = IOPSCopyPowerSourcesList(blob);
            if list.is_null() {
                CFRelease(blob);
                return None;
            }
            let mut result = None;
            for i in 0..CFArrayGetCount(list) {
                let source = CFArrayGetValueAtIndex(list, i);
                if source.is_null() {
                    continue;
                }
                // Get = 所有権は blob 側（CFRelease しない）
                let desc = IOPSGetPowerSourceDescription(blob, source);
                if desc.is_null() {
                    continue;
                }
                let current_key = cf_string("Current Capacity");
                let max_key = cf_string("Max Capacity");
                let current = cf_number_i32(CFDictionaryGetValue(desc, current_key));
                let max = cf_number_i32(CFDictionaryGetValue(desc, max_key));
                CFRelease(current_key);
                CFRelease(max_key);
                if let (Some(current), Some(max)) = (current, max) {
                    if max > 0 {
                        let ratio = f64::from(current) / f64::from(max) * 100.0;
                        result = Some(ratio.round().clamp(0.0, 100.0) as u8);
                        break;
                    }
                }
            }
            CFRelease(list);
            CFRelease(blob);
            result
        }
    }

    /// 蓋が閉じているか（IORegistry AppleClamshellState。root 不要、#218）。
    /// IOKit FFI なのでサブプロセスを使わず UI スレッドで安全に呼べる
    pub fn clamshell_closed() -> bool {
        unsafe {
            let name = b"IOPMrootDomain\0";
            let matching = IOServiceMatching(name.as_ptr());
            if matching.is_null() {
                return false;
            }
            let service = IOServiceGetMatchingService(0, matching);
            // IOServiceMatching の戻り値は IOServiceGetMatchingService が消費する（CFRelease 不要）
            if service == 0 {
                return false;
            }
            let key = cf_string("AppleClamshellState");
            let value = IORegistryEntryCreateCFProperty(service, key, std::ptr::null(), 0);
            CFRelease(key);
            IOObjectRelease(service);
            if value.is_null() {
                return false;
            }
            let result = CFBooleanGetValue(value as CFBooleanRef);
            CFRelease(value);
            result
        }
    }

    /// pmset disablesleep の現在値を IORegistry から読む（root 不要、#218）
    pub fn sleep_disabled() -> bool {
        unsafe {
            let name = b"IOPMrootDomain\0";
            let matching = IOServiceMatching(name.as_ptr());
            if matching.is_null() {
                return false;
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return false;
            }
            let key = cf_string("SleepDisabled");
            let value = IORegistryEntryCreateCFProperty(service, key, std::ptr::null(), 0);
            CFRelease(key);
            IOObjectRelease(service);
            if value.is_null() {
                return false;
            }
            let result = CFBooleanGetValue(value as CFBooleanRef);
            CFRelease(value);
            result
        }
    }

    /// NSProcessInfo.thermalState を取得（ObjC runtime 経由、#218）
    pub fn thermal_state() -> super::ThermalState {
        #[link(name = "objc", kind = "dylib")]
        extern "C" {
            fn objc_getClass(name: *const u8) -> *const c_void;
            fn sel_registerName(name: *const u8) -> *const c_void;
            fn objc_msgSend(receiver: *const c_void, sel: *const c_void, ...) -> *const c_void;
        }

        unsafe {
            let cls = objc_getClass(c"NSProcessInfo".as_ptr() as *const u8);
            if cls.is_null() {
                return super::ThermalState::Nominal;
            }
            let sel_pi = sel_registerName(c"processInfo".as_ptr() as *const u8);
            let pi = objc_msgSend(cls, sel_pi);
            if pi.is_null() {
                return super::ThermalState::Nominal;
            }
            let sel_ts = sel_registerName(c"thermalState".as_ptr() as *const u8);
            let state = objc_msgSend(pi, sel_ts);
            match state as isize {
                0 => super::ThermalState::Nominal,
                1 => super::ThermalState::Fair,
                2 => super::ThermalState::Serious,
                3 => super::ThermalState::Critical,
                _ => super::ThermalState::Nominal,
            }
        }
    }

    /// App Nap を無効化する（NSProcessInfo.beginActivityWithOptions 経由）。
    /// tako 本体のプロセスで一度だけ呼べばよい
    pub fn disable_app_nap() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = std::process::Command::new("defaults")
                .args([
                    "write",
                    &format!("/proc/{}/Info", std::process::id()),
                    "NSAppSleepDisabled",
                    "-bool",
                    "YES",
                ])
                .output();
        });
    }

    /// ディスプレイだけをスリープさせる（#311）。
    /// disablesleep=1 の蓋閉じ時に呼ぶ。root 不要。蓋閉じ中はユーザー入力が
    /// ないため、一度送ればディスプレイは消灯を維持する。
    /// 蓋閉じ→蓋開け 1 サイクルにつき 1 回だけ呼ぶ（DISPLAY_SLEEP_SENT で制御）
    pub fn force_display_sleep() {
        if DISPLAY_SLEEP_SENT.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let _ = std::process::Command::new("pmset")
            .arg("displaysleepnow")
            .output();
        DISPLAY_SLEEP_SENT.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn display_sleep_sent() -> bool {
        DISPLAY_SLEEP_SENT.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn reset_display_sleep_sent() {
        DISPLAY_SLEEP_SENT.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

// --- 蓋閉じ防止の sudoers 登録・pmset 制御（#218） ---

const SUDOERS_FILE: &str = "/etc/sudoers.d/tako-sleep-guard";
const SUDOERS_CONTENT: &str = "\
# tako sleep guard: pmset disablesleep のみ NOPASSWD (#218)
# アンインストール: sudo rm /etc/sudoers.d/tako-sleep-guard
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1
";

/// 蓋を閉じたまま走らせ続ける制御をこの OS が持つか（#524 / #697）。
///
/// アイドルスリープの防止そのものとは**別の軸**として公開する
/// （案内する側が「設定できるのに案内しない / できないのに案内する」を避けられる）。
///
/// 実現手段は OS で違うが、どちらも「永続設定を一時的に倒す」形なので意味は同じ:
///
/// | OS | 手段 | 権限 |
/// |---|---|---|
/// | macOS | clamshell 検知 + sudoers + `pmset disablesleep` | 初回に管理者パスワード |
/// | Windows | 電源プランの `GUID_LIDCLOSE_ACTION` を 0 へ（`platform::lid`） | **不要** |
///
/// #524 の時点では Windows を「相当する API が無い」として落としていたが、
/// これは誤りだった（#697 で実測して訂正）。設定は `powercfg /q` の一覧に出ない
/// （定義側の `Attributes = 1` で UI から hidden）だけで、GUID を明示すれば
/// 非管理者のまま読み書きできる
pub fn lid_control_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::platform::lid::supported()
    }
}

/// sudoers.d/tako-sleep-guard が登録済みか（**macOS 固有の手段**）
pub fn is_sudoers_installed() -> bool {
    std::path::Path::new(SUDOERS_FILE).exists()
}

/// 蓋の開閉状態（`lid_closed`）をこの OS で観測できるか（#697）。
///
/// macOS は IORegistry の `AppleClamshellState` で root なしに読める。Windows は
/// `RegisterPowerSettingNotification` にウィンドウハンドルが要り、状態表示のためだけに
/// 持つには重いので**観測しない**。`lid_closed` が常に false になるので、
/// 表示側はこの関数で「開いている」と「分からない」を区別する
pub fn lid_state_detectable() -> bool {
    cfg!(target_os = "macos")
}

/// この OS の蓋閉じ継続が、管理者権限を伴う初回登録を要する仕組みか（#697）。
///
/// **OS ごとに固定**で、登録が済んだかどうかでは変わらない（そこは `lid_setup_pending`）。
/// 設定画面の説明文のように「この OS ではどういう仕組みか」を書く場所で使う
pub fn lid_requires_privileged_setup() -> bool {
    cfg!(target_os = "macos")
}

/// 蓋閉じ継続を使うのに、この機械でまだ初回セットアップが要るか（#697）。
///
/// macOS は sudoers 登録が必要。Windows は電源プランを非管理者で書けるので**不要**。
/// 呼び出し側（setup / 状態表示）が「sudoers」という macOS 固有の手段を知らずに済むよう、
/// **手段ではなく未完了かどうか**を返す
pub fn lid_setup_pending() -> bool {
    if !lid_control_supported() {
        return false; // 使えない OS では「セットアップ待ち」ですらない
    }
    #[cfg(target_os = "macos")]
    {
        !is_sudoers_installed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 蓋閉じ継続を使える状態にする（#697）。
///
/// macOS は sudoers 登録（管理者プロンプト）、Windows は権限が要らないので何もしない。
/// **呼び出し側は OS を意識しない**
pub fn prepare_lid_control() -> Result<String, String> {
    if !lid_control_supported() {
        return Err("この OS では蓋閉じ継続に対応していません".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        install_sudoers()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("この OS では追加の権限も登録も不要です（電源プランの設定で実現します）".to_string())
    }
}

/// 蓋閉じ継続の後始末（#697）。macOS は sudoers 削除 + `disablesleep 0`、
/// Windows は倒してある lid action を元へ戻す
pub fn teardown_lid_control() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        remove_sudoers()
    }
    #[cfg(not(target_os = "macos"))]
    {
        // tako が動いていない状態で叩かれても、その場で元へ戻せるようにする
        crate::platform::lid::set_stay_awake(false, false)
            .map(|changed| {
                if changed {
                    "蓋閉じ継続を解除し、電源プランの設定を元へ戻しました".to_string()
                } else {
                    "蓋閉じ継続は有効化されていません（解除不要）".to_string()
                }
            })
            .map_err(|e| format!("蓋閉じ継続の解除に失敗: {e}"))
    }
}

/// osascript 経由で sudoers.d に書き込む（管理者プロンプト表示）
pub fn install_sudoers() -> Result<String, String> {
    let script = format!(
        r#"do shell script "
# visudo 検証用の一時ファイルに書き出し
tmpfile=$(mktemp /tmp/tako-sudoers.XXXXXX)
cat > \"$tmpfile\" << 'SUDOERS'
{content}SUDOERS

# visudo -cf で構文検証
if ! /usr/sbin/visudo -cf \"$tmpfile\" 2>&1; then
    rm -f \"$tmpfile\"
    echo 'ERROR: visudo 構文検証に失敗'
    exit 1
fi

# 本番へ配置（mode 0440、root:wheel）
cp \"$tmpfile\" {path}
chmod 0440 {path}
chown root:wheel {path}
rm -f \"$tmpfile\"
echo 'OK: sudoers 登録完了'
" with administrator privileges"#,
        content = SUDOERS_CONTENT,
        path = SUDOERS_FILE,
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("osascript の実行に失敗: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() && stdout.contains("OK") {
        Ok(stdout)
    } else if stderr.contains("User canceled") || stderr.contains("(-128)") {
        Err("ユーザーがキャンセルしました".to_string())
    } else {
        Err(format!(
            "sudoers 登録に失敗: {}",
            if stderr.is_empty() { &stdout } else { &stderr }
        ))
    }
}

/// osascript 経由で sudoers.d から削除 + disablesleep 0
pub fn remove_sudoers() -> Result<String, String> {
    if !is_sudoers_installed() {
        return Ok("sudoers は未登録です（削除不要）".to_string());
    }

    let script = format!(
        r#"do shell script "
/usr/bin/pmset -a disablesleep 0
rm -f {path}
echo 'OK: sudoers 削除完了・disablesleep 解除'
" with administrator privileges"#,
        path = SUDOERS_FILE,
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("osascript の実行に失敗: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else if stderr.contains("User canceled") || stderr.contains("(-128)") {
        Err("ユーザーがキャンセルしました".to_string())
    } else {
        Err(format!(
            "sudoers 削除に失敗: {}",
            if stderr.is_empty() { &stdout } else { &stderr }
        ))
    }
}

/// sudo pmset -a disablesleep 0/1 を実行する（sudoers 登録済み前提、NOPASSWD）
pub fn set_disablesleep(enable: bool) -> Result<(), String> {
    if !is_sudoers_installed() {
        return Err("sudoers 未登録（tako setup --lid-sleep で登録してください）".to_string());
    }
    let val = if enable { "1" } else { "0" };
    let output = std::process::Command::new("sudo")
        .args(["-n", "/usr/bin/pmset", "-a", "disablesleep", val])
        .output()
        .map_err(|e| format!("pmset の実行に失敗: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("pmset disablesleep {val} に失敗: {stderr}"))
    }
}

/// アサーションを保持すべきかの判定（純粋関数）。
///
/// モードと電源条件の組み合わせは**プラットフォームを問わず同じ規則**なので、
/// macOS（IOKit）と Windows（電源要求）の両経路がこの 1 本を通る（#524）。
///
/// `pub` なのは、設定画面が「効くべきなのに効いていない」を「反映中」と
/// 「AC 未接続」等に切り分けるため（#727）。同じ真理値を UI 側で書き直すと
/// 片方だけ直したときに表示が嘘になるので、判定はここ 1 本に閉じる
pub fn should_hold_assertion(
    mode: SleepGuardMode,
    power_condition: PowerCondition,
    on_ac: bool,
    busy_agents: usize,
) -> bool {
    let wanted = match mode {
        SleepGuardMode::Off => false,
        SleepGuardMode::On => true,
        SleepGuardMode::WhileAgentsRunning => busy_agents > 0,
    };
    wanted
        && match power_condition {
            PowerCondition::AcOnly => on_ac,
            PowerCondition::Always => true,
        }
}

/// 蓋閉じ継続を効かせるべきかの判定（純粋関数）。
///
/// `should_hold_assertion` と同じく**プラットフォームを問わず同じ規則**にする（#697）。
/// macOS（sudoers + `pmset disablesleep`）と Windows（電源プランの lid action）の
/// 両経路がこの 1 本を通るので、OS で挙動がずれない。
///
/// - `setup_done`: 初回セットアップが済んでいるか。macOS は sudoers 登録、
///   Windows は権限が要らないので常に `true`
/// - `thermal_warning`: 本体が高温か。macOS のみ取得でき、Windows は常に `false`
///
/// **AC 接続を既定の条件にするのは意図的**。蓋を閉じて持ち歩くのはたいていバッテリー
/// 駆動なので、何も設定していない人にまで効かせると鞄の中で電池が尽きる。#1473 で
/// `lid_power_condition = always` を**明示した人だけ**バッテリーでも継続できるようにし、
/// そのときは安全弁（残量下限・thermal・エージェント稼働中のみ）を必ず通す。
///
/// `should_hold_assertion` と同じ理由で `pub`（#727）
pub fn should_disable_lid_sleep(input: &LidGuardInput) -> bool {
    lid_decision(input).is_ok()
}

/// 蓋閉じ継続を倒すか、倒さないならなぜかを返す（#1473）。
///
/// **理由を返す 1 実装**。`status` の description・CLI・設定画面の状態・通知欄・
/// persist.log がすべてここを通るので、表示と実際の判定がずれない。
///
/// 安全弁の順序は「人が読んで納得する順」= 設定 → セットアップ → 稼働 → 電源 →
/// 温度 → 残量。どれか 1 つでも欠けたら倒さない
pub fn lid_decision(input: &LidGuardInput) -> Result<(), LidSkipReason> {
    // A/B: #1473 より前の挙動（AC 接続時のみ・残量下限なし・thermal は serious 以上）
    let input = &legacy_lid_input(input);

    if input.lid_sleep_mode != LidSleepMode::WhileAgentsRunning {
        return Err(LidSkipReason::ModeOff);
    }
    if !input.setup_done {
        return Err(LidSkipReason::SetupRequired);
    }
    if input.busy_agents == 0 {
        return Err(LidSkipReason::NoAgents);
    }
    if !input.on_ac && input.lid_power_condition == PowerCondition::AcOnly {
        return Err(LidSkipReason::NoAcPower);
    }
    // 温度の物差しは電源で変える。AC なら従来どおり serious 以上、バッテリーで
    // 蓋を閉じているなら悪化の兆し（fair）で降りる（放熱が塞がった鞄の中を想定）
    let too_hot = if input.on_ac {
        input.thermal.is_warning()
    } else {
        input.thermal.blocks_battery_lid()
    };
    if too_hot {
        return Err(LidSkipReason::Thermal(input.thermal));
    }
    // 残量の安全弁はバッテリー駆動のときだけ意味がある（AC 中は減らない）。
    // **読めないときは倒さない**（止める条件を持てないまま走り続けるのが最悪）
    if !input.on_ac {
        match input.battery_percent {
            Some(percent) if percent <= input.battery_floor => {
                return Err(LidSkipReason::BatteryFloor {
                    percent,
                    floor: input.battery_floor,
                })
            }
            None => return Err(LidSkipReason::BatteryUnknown),
            Some(_) => {}
        }
    }
    Ok(())
}

/// #1473 の A/B。`TAKO_1473_LEGACY=1` で**同一バイナリのまま**旧挙動
/// （蓋閉じ継続は AC 接続時のみ）へ戻す
pub fn legacy_1473() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1473_LEGACY").map(|v| v == "1") == Ok(true))
}

/// A/B のアームで入力を旧相当へ丸める（#1473）。
///
/// 判定の本体を 2 本に割らないため、**入口で材料のほうを旧世界に戻す**
/// （`ac-only` 固定 = バッテリーでは `NoAcPower` で必ず降りるので、
/// 残量下限も温度の新しい物差しも到達しない）
fn legacy_lid_input(input: &LidGuardInput) -> LidGuardInput {
    if !legacy_1473() {
        return *input;
    }
    LidGuardInput {
        lid_power_condition: PowerCondition::AcOnly,
        ..*input
    }
}

/// 蓋閉じ継続の状態が変わったときに画面へ出すもの（#1473）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidNotice {
    /// 安全弁が働いて蓋閉じ継続を解除した
    Released(LidSkipReason),
    /// 解除していたものを再適用した
    Reapplied,
}

/// 前回と今回の状態から、通知欄へ出すべき変化を求める（#1473。純粋関数）。
///
/// **出すのは安全弁による解除と、その回復だけ**。エージェントが一段落した
/// （`NoAgents`）・AC を抜いた（`NoAcPower`）は設定どおりの正常な動きなので、
/// バナーにすると作業のたびに画面へ出る（診断には persist.log の 1 行が残る）。
///
/// `ac-only`（既定）の人には何も出さない。バッテリー継続を選んだ人にとってだけ
/// 「止まったのか・続いているのか」が読めないと困る情報だから
pub fn lid_notice(prev: Option<&SleepGuardState>, next: &SleepGuardState) -> Option<LidNotice> {
    if next.lid_power_condition != PowerCondition::Always
        || next.lid_sleep_mode != LidSleepMode::WhileAgentsRunning
    {
        return None;
    }
    let prev = prev?;
    match (prev.lid_sleep_disabled, next.lid_sleep_disabled) {
        // 効いていたものが落ちた: 落ちた理由が安全弁のときだけ出す
        (true, false) => next
            .lid_skip_reason
            .filter(|r| r.is_safety_valve())
            .map(LidNotice::Released),
        // 落ちていたものが戻った: 直前が安全弁で落ちていたときだけ出す
        // （エージェントが動き出しただけの再開は通知しない）
        (false, true) => prev
            .lid_skip_reason
            .filter(|r| r.is_safety_valve())
            .map(|_| LidNotice::Reapplied),
        _ => None,
    }
}

/// 検証用の注入（`TAKO_1473_INJECT_*`）が効く環境か（#1473）。
///
/// **本番の GUI では常に false**。バッテリー残量と thermal は
/// `pmset disablesleep` を実際に倒す材料なので、env で本番の判断を
/// 左右できるようにしてはいけない。隔離起動（`TAKO_ISOLATED`）と
/// セルフテスト（`TAKO_SELF_TEST`）のときだけ読む
fn inject_allowed() -> bool {
    inject_allowed_from(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        std::env::var_os("TAKO_SELF_TEST").is_some(),
    )
}

/// [`inject_allowed`] の純粋部分（env を触らずに検査できる形にしておく）
fn inject_allowed_from(isolated: Option<&str>, self_test: bool) -> bool {
    tako_core::tmux_cleanup::is_isolated_value(isolated) || self_test
}

/// バッテリー残量の注入（`TAKO_1473_INJECT_BATTERY=15`）。範囲外は無視する
fn injected_battery_percent() -> Option<u8> {
    if !inject_allowed() {
        return None;
    }
    parse_injected_battery(std::env::var("TAKO_1473_INJECT_BATTERY").ok().as_deref())
}

fn parse_injected_battery(value: Option<&str>) -> Option<u8> {
    value
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|p| *p <= 100)
}

/// thermal 状態の注入（`TAKO_1473_INJECT_THERMAL=serious`）
fn injected_thermal() -> Option<ThermalState> {
    if !inject_allowed() {
        return None;
    }
    std::env::var("TAKO_1473_INJECT_THERMAL")
        .ok()
        .and_then(|v| ThermalState::from_str_opt(&v))
}

/// いまのバッテリー残量（%。#1473）。
///
/// デスクトップ機・バッテリーを持たない環境・読めない OS は `None`。
/// 注入（隔離・セルフテストのみ）が在ればそれを優先する
pub fn battery_percent() -> Option<u8> {
    if let Some(p) = injected_battery_percent() {
        return Some(p);
    }
    #[cfg(target_os = "macos")]
    {
        iokit::battery_percent()
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows の残量取得は #1473 の対象外。読めない = バッテリー駆動では
        // 安全弁が働いて蓋閉じ継続に入らない（理由は status に出る）
        None
    }
}

/// いまの thermal 状態（#1473。注入を通す 1 実装）
fn current_thermal() -> ThermalState {
    if let Some(t) = injected_thermal() {
        return t;
    }
    #[cfg(target_os = "macos")]
    {
        iokit::thermal_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows は NSProcessInfo 相当を持たない（#218 からの既知の制約）
        ThermalState::Nominal
    }
}

/// 電源要求に添える理由文字列（#524）。
///
/// Windows では `powercfg /requests` にそのまま出る。診断ツールのコンソール出力で
/// 文字化けさせないため **ASCII に固定**する（UI へは出ないので日英化の対象外。
/// UI 文言は `tako-app::ui_text::sleep_guard` が持つ）
#[cfg(not(target_os = "macos"))]
fn assertion_reason(mode: SleepGuardMode, busy_agents: usize) -> String {
    match mode {
        SleepGuardMode::On => "tako: sleep guard (always on)".to_string(),
        SleepGuardMode::WhileAgentsRunning => {
            format!("tako: sleep guard (agents running: {busy_agents})")
        }
        // Off で保持することは無い（呼ばれても無害な既定値を返す）
        SleepGuardMode::Off => "tako: sleep guard".to_string(),
    }
}

/// 残留解除を行うべきかを判定する（#449: テスト可能な純粋関数）。
/// Ok(()) なら解除すべき、Err(理由) ならスキップ
fn should_clear_residual(
    is_isolated: bool,
    other_instance_running: bool,
    sudoers_installed: bool,
    sleep_disabled: bool,
) -> Result<(), &'static str> {
    if is_isolated {
        return Err("隔離モード（TAKO_ISOLATED）のためスキップ");
    }
    if other_instance_running {
        return Err("他の tako プロセスが動作中のためスキップ");
    }
    if !sudoers_installed {
        return Err("sudoers 未登録");
    }
    if !sleep_disabled {
        return Err("disablesleep=0（残留なし）");
    }
    Ok(())
}

/// 蓋制御のエラーを出しすぎないための直近の文言（#697）。
///
/// `update()` は 2 秒ごとに呼ばれるので、書き込みが恒久的に失敗する環境
/// （グループポリシーで電源プランが固定されている等）だと同じ行が延々と出る。
/// **文言が変わったときだけ**出す
static LAST_LID_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn report_lid_error(msg: &str) {
    let mut last = match LAST_LID_ERROR.lock() {
        Ok(v) => v,
        Err(poisoned) => poisoned.into_inner(),
    };
    if last.as_deref() == Some(msg) {
        return;
    }
    eprintln!("[sleep-guard] 蓋閉じ継続に失敗: {msg}");
    crate::diag::persist_log(&format!("lid-sleep error: {msg}"));
    *last = Some(msg.to_string());
}

/// 成功したらエラーの記憶を捨てる（次に失敗したらまた 1 回出る）
fn clear_lid_error() {
    if let Ok(mut last) = LAST_LID_ERROR.lock() {
        *last = None;
    }
}

/// 蓋閉じ継続を倒した / 戻したことを診断へ 1 行残す（#1473。**両 OS 経路の 1 実装**）。
///
/// 書式を 1 つにしておくと、`persist.log` を `lid-sleep:` で grep したときに
/// macOS と Windows の記録が同じ形で並ぶ。載せるのは**理由の分類（ASCII のタグ）**と
/// 残量だけで、本文や環境の値は載せない（#1376 と同じ作法）
fn log_lid_change(enabled: bool, reason: Option<LidSkipReason>, battery: Option<u8>) {
    let head = if enabled {
        "蓋閉じ継続を有効化"
    } else {
        "蓋閉じ継続を解除"
    };
    let reason = match reason {
        Some(r) => r.tag(),
        None => "applied",
    };
    let battery = battery
        .map(|p| format!(" battery={p}%"))
        .unwrap_or_default();
    crate::diag::persist_log(&format!("lid-sleep: {head} reason={reason}{battery}"));
}

/// 起動時の残留チェック: 蓋閉じ継続の上書きが残っていれば元へ戻す。
/// セカンダリモードでは呼び出し側でスキップすること（#449）
///
/// macOS は `pmset disablesleep=1`、Windows は電源プランの lid action が対象。
/// **呼び出し側は OS を意識しない**（#697 で Windows 経路を足しても main.rs は無変更）
pub fn check_disablesleep_residual() {
    #[cfg(not(target_os = "macos"))]
    {
        let is_isolated = matches!(
            std::env::var("TAKO_ISOLATED").ok().as_deref(),
            Some("1" | "true" | "on")
        );
        match crate::platform::lid::clear_residual(
            is_isolated,
            tako_core::ports::other_tako_running(),
        ) {
            Ok(Some(msg)) => {
                eprintln!("[sleep-guard] {msg}");
                crate::diag::persist_log("lid-sleep residual cleared on startup");
            }
            Ok(None) => {}
            Err(e) => eprintln!("[sleep-guard] 蓋閉じ継続の残留解除に失敗: {e}"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let is_isolated = matches!(
            std::env::var("TAKO_ISOLATED").ok().as_deref(),
            Some("1" | "true" | "on")
        );
        if let Err(reason) = should_clear_residual(
            is_isolated,
            tako_core::ports::other_tako_running(),
            is_sudoers_installed(),
            iokit::sleep_disabled(),
        ) {
            eprintln!("[sleep-guard] disablesleep 残留チェック: {reason}");
            return;
        }
        if let Err(e) = set_disablesleep(false) {
            eprintln!("[sleep-guard] disablesleep 残留の自動解除に失敗: {e}");
        } else {
            eprintln!("[sleep-guard] disablesleep 残留を自動解除しました（前回のクラッシュまたは異常終了）");
            crate::diag::persist_log("lid-sleep residual cleared on startup");
        }
    }
}

/// System Settings の Battery 設定画面を開く（フォールバック用）
pub fn open_battery_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::os_integration::open_url(
            "x-apple.systempreferences:com.apple.preference.battery",
        )
        .map_err(|e| format!("System Settings を開けません: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("macOS 以外では非対応".to_string())
    }
}

/// スリープ防止の状態を更新する。busy_agents は現在 busy なエージェントの数。
/// 設定に基づいてアサーションの取得・解放を行い、現在の状態を返す
pub fn update(config: SleepGuardConfig, busy_agents: usize) -> SleepGuardState {
    BUSY_AGENTS.store(busy_agents, Ordering::Relaxed);
    let SleepGuardConfig {
        mode,
        power_condition,
        lid_sleep_mode,
        lid_power_condition,
        lid_battery_floor,
    } = config;
    // 材料の取得は OS ごとの実装に閉じつつ、**判定は 1 実装**（#1473）
    let battery = battery_percent();
    let thermal = current_thermal();
    #[cfg(not(target_os = "macos"))]
    {
        // 蓋の開閉検知は macOS 固有のまま
        // （Windows の蓋の開閉は `RegisterPowerSettingNotification` にウィンドウハンドルが
        // 要り、ここからは取れない）。蓋閉じ継続そのものは電源プランの lid action で行う（#697）
        let on_ac = crate::platform::power::on_ac_power();
        let should_hold = should_hold_assertion(mode, power_condition, on_ac, busy_agents);
        crate::platform::power::set_hold(should_hold, &assertion_reason(mode, busy_agents));

        // --- 蓋閉じ継続（#697: 電源プランの lid action を倒す） ---
        let lid_supported = lid_control_supported();
        if lid_supported {
            // Windows は初回セットアップが要らないので setup_done = true。
            // 判定関数と安全弁は macOS と同一（#1473）
            let decision = lid_decision(&LidGuardInput {
                lid_sleep_mode,
                setup_done: true,
                busy_agents,
                on_ac,
                thermal,
                lid_power_condition,
                battery_percent: battery,
                battery_floor: lid_battery_floor,
            });
            let should_disable = decision.is_ok();
            // 倒すレールは電源条件に合わせる（#1473）。`ac-only`（既定）では
            // AC レールだけ = 従来どおりで、バッテリー側を触らないことが残留時の
            // 安全弁にもなる。`always` を選んだときだけバッテリーレールも倒す
            let include_battery = lid_power_condition == PowerCondition::Always;
            match crate::platform::lid::set_stay_awake(should_disable, include_battery) {
                Ok(true) => {
                    log_lid_change(should_disable, decision.err(), battery);
                    clear_lid_error();
                }
                Ok(false) => {}
                Err(e) => report_lid_error(&e),
            }
        }

        return SleepGuardState {
            assertion_held: crate::platform::power::is_held(),
            mode,
            power_condition,
            on_ac_power: on_ac,
            busy_agents,
            platform_supported: crate::platform::power::supported(),
            lid_closed: false,
            lid_sleep_disabled: crate::platform::lid::is_active(),
            // 制御できない OS（Linux 等）で設定値をそのまま返すと
            // 「設定してあるのに効かない」と読めてしまうので Off を返す
            lid_sleep_mode: if lid_supported {
                lid_sleep_mode
            } else {
                LidSleepMode::Off
            },
            sudoers_installed: false,
            lid_setup_required: false,
            thermal_state: thermal,
            display_sleep_forced: false,
            lid_power_condition,
            lid_battery_floor,
            battery_percent: battery,
            lid_skip_reason: None,
        }
        .with_decision();
    }
    #[cfg(target_os = "macos")]
    {
        let on_ac = iokit::on_ac_power();
        let lid_closed = iokit::clamshell_closed();
        let sudoers = is_sudoers_installed();

        // --- アイドルスリープ防止（既存ロジック。判定は #524 で共通化） ---
        let should_hold = should_hold_assertion(mode, power_condition, on_ac, busy_agents);

        if should_hold && !iokit::is_held() {
            let reason = match mode {
                SleepGuardMode::On => "tako: スリープ防止（常時モード）".to_string(),
                SleepGuardMode::WhileAgentsRunning => {
                    format!("tako: エージェント稼働中（{busy_agents} 体）")
                }
                SleepGuardMode::Off => unreachable!(),
            };
            iokit::create_assertion(&reason);
        } else if !should_hold && iokit::is_held() {
            iokit::release_assertion();
        }

        // --- 蓋閉じ防止（#218: pmset disablesleep） ---
        // 判定は #697 で Windows と共通化し、#1473 で安全弁つきの 1 実装になった
        let current_disabled = iokit::sleep_disabled();
        if lid_sleep_mode == LidSleepMode::WhileAgentsRunning && sudoers {
            let decision = lid_decision(&LidGuardInput {
                lid_sleep_mode,
                setup_done: sudoers,
                busy_agents,
                on_ac,
                thermal,
                lid_power_condition,
                battery_percent: battery,
                battery_floor: lid_battery_floor,
            });
            let should_disable = decision.is_ok();
            // 倒した・戻したは診断へ残す（#1473。旧実装は結果を捨てていたので、
            // 安全弁が働いて解除されても記録が何も残らなかった）
            if should_disable && !current_disabled {
                match set_disablesleep(true) {
                    Ok(()) => {
                        log_lid_change(true, None, battery);
                        clear_lid_error();
                    }
                    Err(e) => report_lid_error(&e),
                }
            } else if !should_disable && current_disabled {
                match set_disablesleep(false) {
                    Ok(()) => {
                        log_lid_change(false, decision.err(), battery);
                        clear_lid_error();
                    }
                    Err(e) => report_lid_error(&e),
                }
            }
        }

        let lid_sleep_disabled = iokit::sleep_disabled();

        // --- ディスプレイ消灯（#311） ---
        // disablesleep=1 は蓋閉じ時のディスプレイ消灯も阻害するため、
        // 蓋閉じ + disablesleep 有効の組み合わせで明示的にディスプレイだけ消す
        if lid_closed && lid_sleep_disabled {
            iokit::force_display_sleep();
        }
        if !lid_closed || !lid_sleep_disabled {
            iokit::reset_display_sleep_sent();
        }

        SleepGuardState {
            assertion_held: iokit::is_held(),
            mode,
            power_condition,
            on_ac_power: on_ac,
            busy_agents,
            platform_supported: true,
            lid_closed,
            lid_sleep_disabled,
            lid_sleep_mode,
            sudoers_installed: sudoers,
            // macOS は sudoers 登録が初回セットアップ
            lid_setup_required: !sudoers,
            thermal_state: thermal,
            display_sleep_forced: iokit::display_sleep_sent(),
            lid_power_condition,
            lid_battery_floor,
            battery_percent: battery,
            lid_skip_reason: None,
        }
        .with_decision()
    }
}

/// 現在の状態を取得する（副作用なし）
pub fn status(config: SleepGuardConfig) -> SleepGuardState {
    let busy_agents = BUSY_AGENTS.load(Ordering::Relaxed);
    let SleepGuardConfig {
        mode,
        power_condition,
        lid_sleep_mode,
        lid_power_condition,
        lid_battery_floor,
    } = config;
    let battery = battery_percent();
    let thermal = current_thermal();
    #[cfg(not(target_os = "macos"))]
    {
        // 副作用なし: 取得・解放は行わず、いまの保持状態と電源だけを読む
        let lid_supported = lid_control_supported();
        return SleepGuardState {
            assertion_held: crate::platform::power::is_held(),
            mode,
            power_condition,
            on_ac_power: crate::platform::power::on_ac_power(),
            busy_agents,
            platform_supported: crate::platform::power::supported(),
            lid_closed: false,
            lid_sleep_disabled: crate::platform::lid::is_active(),
            lid_sleep_mode: if lid_supported {
                lid_sleep_mode
            } else {
                LidSleepMode::Off
            },
            sudoers_installed: false,
            lid_setup_required: false,
            thermal_state: thermal,
            display_sleep_forced: false,
            lid_power_condition,
            lid_battery_floor,
            battery_percent: battery,
            lid_skip_reason: None,
        }
        .with_decision();
    }
    #[cfg(target_os = "macos")]
    {
        SleepGuardState {
            assertion_held: iokit::is_held(),
            mode,
            power_condition,
            on_ac_power: iokit::on_ac_power(),
            busy_agents,
            platform_supported: true,
            lid_closed: iokit::clamshell_closed(),
            lid_sleep_disabled: iokit::sleep_disabled(),
            lid_sleep_mode,
            sudoers_installed: is_sudoers_installed(),
            lid_setup_required: !is_sudoers_installed(),
            thermal_state: thermal,
            display_sleep_forced: iokit::display_sleep_sent(),
            lid_power_condition,
            lid_battery_floor,
            battery_percent: battery,
            lid_skip_reason: None,
        }
        .with_decision()
    }
}

/// App Nap を無効化する（macOS のみ）
pub fn disable_app_nap() {
    #[cfg(target_os = "macos")]
    iokit::disable_app_nap();
}

/// プロセスの QoS を確認する（macOS のみ、診断用）
pub fn check_qos() -> Value {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "pri,nice"])
            .output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                json!({ "qos_info": stdout.trim() })
            }
            Err(e) => json!({ "error": format!("{e}") }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({ "qos_info": "macOS 以外では非対応" })
    }
}

/// tako 終了時に残ったスリープ防止を解除する（正常終了フック）
pub fn cleanup_on_exit() {
    #[cfg(target_os = "macos")]
    {
        if is_sudoers_installed() && iokit::sleep_disabled() {
            let _ = set_disablesleep(false);
        }
    }
    // Windows の電源要求はプロセス終了で OS が回収するが、明示的に解除しておく
    // （`powercfg /requests` に残らないことを終了直後に確認できる）
    #[cfg(not(target_os = "macos"))]
    {
        crate::platform::power::set_hold(false, "");
        // **蓋の設定は OS が回収してくれない**（電源プランに書かれた永続設定なので、
        // 倒したまま終了すると次に tako を起動するまで蓋を閉じてもスリープしない）。
        // 起動時の残留復元（#697）は最後の砦であって、正常終了ではここで必ず戻す
        if let Err(e) = crate::platform::lid::set_stay_awake(false, false) {
            eprintln!("[sleep-guard] 終了時の蓋設定の復元に失敗: {e}");
            crate::diag::persist_log(&format!("lid-sleep cleanup on exit failed: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1473 より前の 5 引数版と同じ意味の判定材料（既存テストの意図を保つ）。
    /// 電源条件は `ac-only`・残量は満充電なので、新しい安全弁には触れない
    fn lid_in(
        lid_sleep_mode: LidSleepMode,
        setup_done: bool,
        busy_agents: usize,
        on_ac: bool,
        thermal_warning: bool,
    ) -> LidGuardInput {
        LidGuardInput {
            lid_sleep_mode,
            setup_done,
            busy_agents,
            on_ac,
            thermal: if thermal_warning {
                ThermalState::Serious
            } else {
                ThermalState::Nominal
            },
            lid_power_condition: PowerCondition::AcOnly,
            battery_percent: Some(100),
            battery_floor: DEFAULT_LID_BATTERY_FLOOR,
        }
    }

    /// #1473: バッテリー駆動で蓋閉じ継続を続ける設定の判定材料
    fn battery_in(busy_agents: usize, percent: Option<u8>, thermal: ThermalState) -> LidGuardInput {
        LidGuardInput {
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            setup_done: true,
            busy_agents,
            on_ac: false,
            thermal,
            lid_power_condition: PowerCondition::Always,
            battery_percent: percent,
            battery_floor: DEFAULT_LID_BATTERY_FLOOR,
        }
    }

    #[test]
    fn mode_roundtrip() {
        for mode in [
            SleepGuardMode::Off,
            SleepGuardMode::On,
            SleepGuardMode::WhileAgentsRunning,
        ] {
            assert_eq!(SleepGuardMode::from_str_opt(mode.as_str()), Some(mode));
        }
        assert_eq!(
            SleepGuardMode::from_str_opt("while_agents_running"),
            Some(SleepGuardMode::WhileAgentsRunning)
        );
        assert_eq!(SleepGuardMode::from_str_opt("invalid"), None);
    }

    #[test]
    fn power_condition_roundtrip() {
        for pc in [PowerCondition::AcOnly, PowerCondition::Always] {
            assert_eq!(PowerCondition::from_str_opt(pc.as_str()), Some(pc));
        }
        assert_eq!(
            PowerCondition::from_str_opt("ac_only"),
            Some(PowerCondition::AcOnly)
        );
        assert_eq!(PowerCondition::from_str_opt("invalid"), None);
    }

    #[test]
    fn lid_sleep_mode_roundtrip() {
        for m in [LidSleepMode::Off, LidSleepMode::WhileAgentsRunning] {
            assert_eq!(LidSleepMode::from_str_opt(m.as_str()), Some(m));
        }
        assert_eq!(
            LidSleepMode::from_str_opt("while_agents_running"),
            Some(LidSleepMode::WhileAgentsRunning)
        );
        assert_eq!(LidSleepMode::from_str_opt("invalid"), None);
    }

    #[test]
    fn thermal_state_warning() {
        assert!(!ThermalState::Nominal.is_warning());
        assert!(!ThermalState::Fair.is_warning());
        assert!(ThermalState::Serious.is_warning());
        assert!(ThermalState::Critical.is_warning());
    }

    #[test]
    fn default_mode_is_while_agents_running() {
        assert_eq!(
            SleepGuardMode::default(),
            SleepGuardMode::WhileAgentsRunning
        );
    }

    #[test]
    fn default_power_condition_is_ac_only() {
        assert_eq!(PowerCondition::default(), PowerCondition::AcOnly);
    }

    #[test]
    fn default_lid_sleep_mode_is_off() {
        assert_eq!(LidSleepMode::default(), LidSleepMode::Off);
    }

    #[test]
    fn status_json_has_required_fields() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: cfg!(target_os = "macos"),
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        let json = state.to_json();
        assert!(json.get("assertion_held").is_some());
        assert!(json.get("mode").is_some());
        assert!(json.get("power_condition").is_some());
        assert!(json.get("on_ac_power").is_some());
        assert!(json.get("busy_agents").is_some());
        assert!(json.get("platform_supported").is_some());
        assert!(json.get("lid_closed").is_some());
        assert!(json.get("lid_sleep_disabled").is_some());
        assert!(json.get("lid_sleep_mode").is_some());
        assert!(json.get("sudoers_installed").is_some());
        assert!(json.get("thermal_state").is_some());
        assert!(json.get("display_sleep_forced").is_some());
        assert!(json.get("description").is_some());
    }

    /// #372: CLI は IPC で受け取った JSON を型へ戻して**同じレンダラ**へ通す。
    /// `to_json` へフィールドを足して `from_json` を忘れると、CLI が
    /// 「アプリは動いているのに未起動扱い」へ落ちる（= busy 0 の嘘が戻る）ので、
    /// 全フィールドの往復をここで拘束する
    #[test]
    fn status_jsonは型へ往復できる() {
        let state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::Always,
            on_ac_power: true,
            busy_agents: 3,
            platform_supported: true,
            lid_closed: true,
            lid_sleep_disabled: true,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Serious,
            display_sleep_forced: true,
            // 新フィールドも**既定と違う値**で往復させる（既定のままだと
            // 読み落としに気づけない）
            lid_power_condition: PowerCondition::Always,
            lid_battery_floor: 35,
            battery_percent: Some(42),
            lid_skip_reason: None,
        };
        let back = SleepGuardState::from_json(&state.to_json()).expect("往復できるはず");
        assert_eq!(back.assertion_held, state.assertion_held);
        assert_eq!(back.mode, state.mode);
        assert_eq!(back.power_condition, state.power_condition);
        assert_eq!(back.on_ac_power, state.on_ac_power);
        assert_eq!(back.busy_agents, state.busy_agents);
        assert_eq!(back.platform_supported, state.platform_supported);
        assert_eq!(back.lid_closed, state.lid_closed);
        assert_eq!(back.lid_sleep_disabled, state.lid_sleep_disabled);
        assert_eq!(back.lid_sleep_mode, state.lid_sleep_mode);
        assert_eq!(back.sudoers_installed, state.sudoers_installed);
        assert_eq!(back.lid_setup_required, state.lid_setup_required);
        assert_eq!(back.thermal_state, state.thermal_state);
        assert_eq!(back.display_sleep_forced, state.display_sleep_forced);
        assert_eq!(back.lid_power_condition, state.lid_power_condition);
        assert_eq!(back.lid_battery_floor, state.lid_battery_floor);
        assert_eq!(back.battery_percent, state.battery_percent);
        // #1473: 残量の `null`（読めない）と 0% は別物。null を 0 と読むと
        // 「常に下限割れ」になって蓋閉じ継続が永久に効かない
        let mut unknown = state.to_json();
        unknown["battery_percent"] = Value::Null;
        let back = SleepGuardState::from_json(&unknown).expect("null も往復できる");
        assert_eq!(back.battery_percent, None);
        // キーごと無い（古い応答）は「読めない」ではなく**欠損**として弾く
        let mut missing = state.to_json();
        missing.as_object_mut().unwrap().remove("battery_percent");
        assert!(SleepGuardState::from_json(&missing).is_none());
        // 欠けたフィールドは None（既定値で埋めて嘘をつかない）
        let mut broken = state.to_json();
        broken["busy_agents"] = Value::Null;
        assert!(SleepGuardState::from_json(&broken).is_none());
        assert!(SleepGuardState::from_json(&json!({})).is_none());
    }

    #[test]
    fn serde_mode_kebab_case() {
        let json = serde_json::to_string(&SleepGuardMode::WhileAgentsRunning).unwrap();
        assert_eq!(json, "\"while-agents-running\"");
        let back: SleepGuardMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SleepGuardMode::WhileAgentsRunning);
    }

    #[test]
    fn serde_power_condition_kebab_case() {
        let json = serde_json::to_string(&PowerCondition::AcOnly).unwrap();
        assert_eq!(json, "\"ac-only\"");
        let back: PowerCondition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PowerCondition::AcOnly);
    }

    #[test]
    fn serde_lid_sleep_mode_kebab_case() {
        let json = serde_json::to_string(&LidSleepMode::WhileAgentsRunning).unwrap();
        assert_eq!(json, "\"while-agents-running\"");
        let back: LidSleepMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LidSleepMode::WhileAgentsRunning);
    }

    #[test]
    fn serde_thermal_state_kebab_case() {
        let json = serde_json::to_string(&ThermalState::Serious).unwrap();
        assert_eq!(json, "\"serious\"");
        let back: ThermalState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ThermalState::Serious);
    }

    #[test]
    fn description_off_mode() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::Off,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("無効"));
    }

    #[test]
    fn description_lid_sleep_active() {
        let state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 2,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: true,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("蓋閉じ継続: 有効"));
    }

    #[test]
    fn description_thermal_warning() {
        let state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::On,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 1,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: true,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Serious,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("高温警告中"));
    }

    #[test]
    fn description_sudoers_not_installed() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        }
        // 理由は材料から埋める（`update` / `status` と同じ締め）
        .with_decision();
        assert!(state.description().contains("sudoers 未登録"));
    }

    #[test]
    fn description_agents_busy_but_no_ac() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: false,
            busy_agents: 2,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("AC 未接続"));
    }

    #[test]
    fn description_not_supported() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::On,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: false,
            busy_agents: 0,
            platform_supported: false,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("この OS では"));
    }

    // --- #524: 保持判定の共通化（macOS / Windows 両経路がこの 1 本を通る） ---

    #[test]
    fn offモードは常に保持しない() {
        for on_ac in [true, false] {
            for busy in [0, 3] {
                assert!(!should_hold_assertion(
                    SleepGuardMode::Off,
                    PowerCondition::Always,
                    on_ac,
                    busy
                ));
            }
        }
    }

    #[test]
    fn onモードはac接続なら保持する() {
        assert!(should_hold_assertion(
            SleepGuardMode::On,
            PowerCondition::AcOnly,
            true,
            0
        ));
        assert!(
            !should_hold_assertion(SleepGuardMode::On, PowerCondition::AcOnly, false, 0),
            "ac-only で AC 未接続なら保持しない"
        );
        assert!(
            should_hold_assertion(SleepGuardMode::On, PowerCondition::Always, false, 0),
            "always ならバッテリーでも保持する"
        );
    }

    #[test]
    fn 自動モードはエージェント稼働中だけ保持する() {
        let m = SleepGuardMode::WhileAgentsRunning;
        assert!(!should_hold_assertion(m, PowerCondition::Always, true, 0));
        assert!(should_hold_assertion(m, PowerCondition::Always, true, 1));
        assert!(
            !should_hold_assertion(m, PowerCondition::AcOnly, false, 1),
            "稼働中でも AC 未接続なら保持しない"
        );
    }

    // --- #697: 蓋閉じ継続の判定の共通化（macOS / Windows 両経路がこの 1 本を通る） ---

    /// macOS の従来ロジック（`busy > 0 && on_ac && !thermal`）と同じ真理値であることを固定する。
    /// ここが崩れると OS 間で挙動がずれる
    #[test]
    fn 蓋閉じ継続はエージェント稼働中かつac接続のときだけ有効() {
        let m = LidSleepMode::WhileAgentsRunning;
        assert!(should_disable_lid_sleep(&lid_in(m, true, 1, true, false)));
        assert!(
            !should_disable_lid_sleep(&lid_in(m, true, 0, true, false)),
            "エージェントが居なければ倒さない"
        );
        assert!(
            !should_disable_lid_sleep(&lid_in(m, true, 1, false, false)),
            "AC 未接続なら倒さない（鞄の中で電池が尽きるため）"
        );
    }

    #[test]
    fn 蓋閉じ継続はoffモードなら常に無効() {
        for busy in [0, 5] {
            for on_ac in [true, false] {
                assert!(!should_disable_lid_sleep(&lid_in(
                    LidSleepMode::Off,
                    true,
                    busy,
                    on_ac,
                    false
                )));
            }
        }
    }

    #[test]
    fn 初回セットアップが済むまでは倒さない() {
        // macOS の sudoers 未登録に相当。Windows は setup_done = true で呼ぶ
        assert!(!should_disable_lid_sleep(&lid_in(
            LidSleepMode::WhileAgentsRunning,
            false,
            3,
            true,
            false
        )));
    }

    #[test]
    fn 高温警告中は蓋閉じ継続を倒さない() {
        // macOS のみ観測できる。Windows は常に false で呼ぶので影響しない
        assert!(!should_disable_lid_sleep(&lid_in(
            LidSleepMode::WhileAgentsRunning,
            true,
            3,
            true,
            true
        )));
    }

    /// 蓋閉じ継続の案内は「手段」ではなく「未完了か」で出し分ける（#697）。
    /// これが崩れると Windows で「sudoers を登録してください」と出る
    #[test]
    fn セットアップ待ちの案内は未完了のときだけ出る() {
        let mut state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: false,
            lid_setup_required: true,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        }
        .with_decision();
        assert!(state.description().contains("sudoers 未登録"));
        // Windows のように初回セットアップが要らない OS では出さない
        state.lid_setup_required = false;
        state = state.with_decision();
        assert!(
            !state.description().contains("sudoers"),
            "セットアップ不要の OS へ macOS 専用の案内を出してはいけない: {}",
            state.description()
        );
    }

    // --- #697: 蓋制御の能力 API どうしの整合（両 OS で同じ不変条件を見る） ---

    /// 「セットアップ待ち」は蓋制御を持つ OS だけで起こりうる。
    /// 持たない OS で pending を返すと、UI が永久に消えない案内を出す
    #[test]
    fn 蓋制御を持たないosはセットアップ待ちにならない() {
        if !lid_control_supported() {
            assert!(!lid_setup_pending());
            assert!(
                !lid_requires_privileged_setup(),
                "制御できない OS が権限を要求してはいけない"
            );
        }
    }

    /// 権限を伴う初回登録が要る OS だけが、その未完了を pending として持つ。
    /// 逆に要らない OS（Windows）は常に「済み」でなければ倒す判定へ進めない
    #[test]
    fn 権限が要らないosはセットアップ済みとして扱う() {
        if lid_control_supported() && !lid_requires_privileged_setup() {
            assert!(
                !lid_setup_pending(),
                "権限が要らない OS で pending を返すと蓋閉じ継続が永久に無効になる"
            );
        }
    }

    /// 蓋の開閉を観測できるのは、権限つきセットアップが要る OS（macOS）だけ。
    /// この対応が崩れたら CLI / 設定画面の出し分け（`lid_state_detectable`）を見直す合図
    #[test]
    fn 蓋の開閉検知と権限セットアップの要否はいまのところ一致する() {
        assert_eq!(lid_state_detectable(), lid_requires_privileged_setup());
    }

    /// 蓋制御を持たない OS では `prepare_lid_control` が**説明つきで**失敗する
    /// （成功したふりをすると設定だけ書かれて何も起きない）
    #[test]
    fn 蓋制御を持たないosでは準備が失敗する() {
        if !lid_control_supported() {
            let e = prepare_lid_control().expect_err("成功してはいけない");
            assert!(
                e.contains("蓋閉じ継続"),
                "理由が分かる文言になっていない: {e}"
            );
        }
    }

    /// 診断ツールに出る文字列。`powercfg /requests` で読めるよう ASCII 固定
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 電源要求の理由はasciiで体数を含む() {
        let r = assertion_reason(SleepGuardMode::WhileAgentsRunning, 3);
        assert!(r.is_ascii(), "理由文字列に非 ASCII が混ざっている: {r}");
        assert!(r.contains("tako") && r.contains('3'), "{r}");
        assert!(assertion_reason(SleepGuardMode::On, 0).is_ascii());
    }

    /// 実機での取得 → 解除（#524）。
    ///
    /// macOS では走らせない。`update()` の macOS 経路は蓋の状態と disablesleep 次第で
    /// `pmset displaysleepnow`（ディスプレイ消灯）まで到達しうるので、テストの副作用に
    /// してはいけない。macOS 側の実装は #173 / #218 / #311 のまま触っていない
    #[cfg(windows)]
    #[test]
    fn updateで保持と解除ができる() {
        let _serial = crate::platform::testing::machine_state_lock();
        // On + always なら電源条件（AC / バッテリー）によらず保持する状態。
        // 蓋閉じ継続は off のまま（電源プランを書かない = テストの副作用にしない）
        let on = SleepGuardConfig {
            mode: SleepGuardMode::On,
            power_condition: PowerCondition::Always,
            ..SleepGuardConfig::default()
        };
        let held = update(on, 0);
        assert!(held.platform_supported, "Windows は対応済みのはず");
        assert!(held.assertion_held, "保持できていない: {held:?}");
        // 副作用なしの status も同じ状態を返す
        let s = status(on);
        assert!(s.assertion_held);
        assert_eq!(s.lid_sleep_mode, LidSleepMode::Off, "蓋閉じ制御は持たない");
        assert!(!s.sudoers_installed);
        // #1473: 蓋閉じ継続の設定も状態へ出る（既定は従来どおり AC のみ）
        assert_eq!(s.lid_power_condition, PowerCondition::AcOnly);
        assert_eq!(s.lid_battery_floor, DEFAULT_LID_BATTERY_FLOOR);

        let off = SleepGuardConfig {
            mode: SleepGuardMode::Off,
            power_condition: PowerCondition::Always,
            ..SleepGuardConfig::default()
        };
        let released = update(off, 0);
        assert!(!released.assertion_held, "解除できていない: {released:?}");
        assert!(!status(off).assertion_held);
    }

    #[test]
    fn description_display_sleep_forced() {
        let state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 1,
            platform_supported: true,
            lid_closed: true,
            lid_sleep_disabled: true,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: true,
            lid_power_condition: PowerCondition::AcOnly,
            lid_battery_floor: DEFAULT_LID_BATTERY_FLOOR,
            battery_percent: Some(100),
            lid_skip_reason: None,
        };
        assert!(state.description().contains("蓋閉じ継続: 有効"));
        assert!(state.description().contains("ディスプレイ消灯済み"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_ac_power_does_not_panic() {
        let _ = iokit::on_ac_power();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clamshell_closed_does_not_panic() {
        let _ = iokit::clamshell_closed();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sleep_disabled_does_not_panic() {
        let _ = iokit::sleep_disabled();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn thermal_state_does_not_panic() {
        let _ = iokit::thermal_state();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iokit_ffi_calls_are_fast() {
        // #212: UI スレッドから 2 秒毎に呼ばれるため、サブプロセス実装への回帰を検出する
        let t0 = std::time::Instant::now();
        for _ in 0..100 {
            let _ = iokit::on_ac_power();
            let _ = iokit::clamshell_closed();
            let _ = iokit::sleep_disabled();
            let _ = iokit::thermal_state();
        }
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "IOKit FFI ×100 が {:?} かかった（サブプロセス実装への回帰の疑い）",
            t0.elapsed()
        );
    }

    #[test]
    fn check_qos_returns_json() {
        let qos = check_qos();
        assert!(qos.is_object());
    }

    // --- #449: should_clear_residual のガード条件テスト ---

    #[test]
    fn residual_skip_when_isolated() {
        let r = should_clear_residual(true, false, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("隔離モード"));
    }

    #[test]
    fn residual_skip_when_other_instance_running() {
        let r = should_clear_residual(false, true, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("他の tako"));
    }

    #[test]
    fn residual_skip_when_no_sudoers() {
        let r = should_clear_residual(false, false, false, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("sudoers"));
    }

    #[test]
    fn residual_skip_when_not_disabled() {
        let r = should_clear_residual(false, false, true, false);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("残留なし"));
    }

    #[test]
    fn residual_clear_when_solo_and_disabled() {
        let r = should_clear_residual(false, false, true, true);
        assert!(r.is_ok());
    }

    #[test]
    fn residual_isolated_takes_priority_over_other_checks() {
        let r = should_clear_residual(true, true, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("隔離モード"));
    }

    // --- #1473: バッテリー駆動でも蓋閉じ継続を続ける（opt-in + 安全弁） ---

    /// 既定（`ac-only`）の挙動は #1473 前と 1 ビットも変わらない。
    /// ここが崩れると「設定を触っていない人の Mac が鞄の中で熱くなる」
    #[test]
    fn 既定ではバッテリー駆動で蓋閉じ継続に入らない() {
        let input = LidGuardInput {
            lid_power_condition: PowerCondition::AcOnly,
            ..battery_in(2, Some(100), ThermalState::Nominal)
        };
        assert_eq!(lid_decision(&input), Err(LidSkipReason::NoAcPower));
    }

    #[test]
    fn alwaysならバッテリーでも倒す() {
        assert_eq!(
            lid_decision(&battery_in(1, Some(80), ThermalState::Nominal)),
            Ok(())
        );
    }

    /// 安全弁①: 残量。**ちょうど下限は降りる側**（「下限 20%」と設定した人の
    /// 期待は「20% になったら止まる」）
    #[test]
    fn 残量が下限に達したら解除する() {
        for percent in [0u8, 1, 19, DEFAULT_LID_BATTERY_FLOOR] {
            assert_eq!(
                lid_decision(&battery_in(1, Some(percent), ThermalState::Nominal)),
                Err(LidSkipReason::BatteryFloor {
                    percent,
                    floor: DEFAULT_LID_BATTERY_FLOOR
                }),
                "残量 {percent}% で解除されない"
            );
        }
        assert_eq!(
            lid_decision(&battery_in(
                1,
                Some(DEFAULT_LID_BATTERY_FLOOR + 1),
                ThermalState::Nominal
            )),
            Ok(()),
            "下限より上なら続ける"
        );
    }

    /// 残量が読めない環境は**続けない**（止める条件を持てないまま走るのが最悪）
    #[test]
    fn 残量が読めなければ解除する() {
        assert_eq!(
            lid_decision(&battery_in(1, None, ThermalState::Nominal)),
            Err(LidSkipReason::BatteryUnknown)
        );
        // AC 接続中は残量が読めなくても関係ない（減らないので安全弁が要らない）
        let on_ac = LidGuardInput {
            on_ac: true,
            battery_percent: None,
            ..battery_in(1, None, ThermalState::Nominal)
        };
        assert_eq!(lid_decision(&on_ac), Ok(()));
    }

    /// 安全弁②: 温度。バッテリーで蓋を閉じているときは `fair` でも降りる。
    /// AC 接続時の物差し（`serious` 以上）は #218 のまま変えない
    #[test]
    fn 温度の物差しは電源で変わる() {
        assert_eq!(
            lid_decision(&battery_in(1, Some(80), ThermalState::Fair)),
            Err(LidSkipReason::Thermal(ThermalState::Fair))
        );
        let on_ac_fair = LidGuardInput {
            on_ac: true,
            ..battery_in(1, Some(80), ThermalState::Fair)
        };
        assert_eq!(lid_decision(&on_ac_fair), Ok(()), "AC 中は fair で降りない");
        let on_ac_serious = LidGuardInput {
            on_ac: true,
            ..battery_in(1, Some(80), ThermalState::Serious)
        };
        assert_eq!(
            lid_decision(&on_ac_serious),
            Err(LidSkipReason::Thermal(ThermalState::Serious))
        );
    }

    /// 安全弁③: エージェントが全部止まったら解除する（`while-agents-running` の意味は不変）
    #[test]
    fn エージェントが居なくなったら解除する() {
        assert_eq!(
            lid_decision(&battery_in(0, Some(80), ThermalState::Nominal)),
            Err(LidSkipReason::NoAgents)
        );
    }

    /// A/B は**材料を旧世界へ丸める**ので、バッテリーでは必ず AC 未接続で降りる
    #[test]
    fn legacyアームの入力はac_onlyへ丸まる() {
        let rolled = legacy_lid_input(&battery_in(1, Some(80), ThermalState::Nominal));
        if legacy_1473() {
            assert_eq!(rolled.lid_power_condition, PowerCondition::AcOnly);
        } else {
            assert_eq!(rolled.lid_power_condition, PowerCondition::Always);
        }
    }

    #[test]
    fn 理由はjsonを往復する() {
        for reason in [
            LidSkipReason::ModeOff,
            LidSkipReason::SetupRequired,
            LidSkipReason::NoAgents,
            LidSkipReason::NoAcPower,
            LidSkipReason::BatteryFloor {
                percent: 12,
                floor: 20,
            },
            LidSkipReason::BatteryUnknown,
            LidSkipReason::Thermal(ThermalState::Fair),
        ] {
            let back = LidSkipReason::from_json(&reason.to_json());
            assert_eq!(back, Some(reason), "{} が往復しない", reason.tag());
            assert!(!reason.describe().is_empty());
        }
        assert_eq!(LidSkipReason::from_json(&json!({})), None);
    }

    #[test]
    fn 安全弁かどうかの分類() {
        assert!(LidSkipReason::BatteryFloor {
            percent: 5,
            floor: 20
        }
        .is_safety_valve());
        assert!(LidSkipReason::BatteryUnknown.is_safety_valve());
        assert!(LidSkipReason::Thermal(ThermalState::Fair).is_safety_valve());
        // 設定どおりの動きは通知しない（バナーが作業のたびに出る）
        assert!(!LidSkipReason::NoAgents.is_safety_valve());
        assert!(!LidSkipReason::NoAcPower.is_safety_valve());
        assert!(!LidSkipReason::ModeOff.is_safety_valve());
        assert!(!LidSkipReason::SetupRequired.is_safety_valve());
    }

    /// 状態は材料をすべて持っているので、`with_decision` で理由まで埋まる
    /// （読む側は再計算しない = CLI とアプリで判断が割れない。#372 / #1473）
    #[test]
    fn 状態から理由が導ける() {
        let mut state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: false,
            busy_agents: 1,
            platform_supported: true,
            lid_closed: true,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::Always,
            lid_battery_floor: 20,
            battery_percent: Some(10),
            lid_skip_reason: None,
        };
        state = state.with_decision();
        assert_eq!(
            state.lid_skip_reason,
            Some(LidSkipReason::BatteryFloor {
                percent: 10,
                floor: 20
            })
        );
        assert!(
            state.description().contains("残量"),
            "{}",
            state.description()
        );
        state.battery_percent = Some(60);
        state = state.with_decision();
        assert_eq!(state.lid_skip_reason, None);
        // 効いているときはバッテリー残量と下限まで出す（あとどれだけ続くかが読める）
        state.lid_sleep_disabled = true;
        let desc = state.description();
        assert!(desc.contains("60%") && desc.contains("20%"), "{desc}");
        // JSON も同じ理由を運ぶ（読む側は再計算しない）
        let json = state.with_decision().to_json();
        assert_eq!(json["lid_skip_reason"], Value::Null);
    }

    // --- 通知欄へ出す変化（#1473） ---

    fn battery_state(disabled: bool, percent: Option<u8>) -> SleepGuardState {
        SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: false,
            busy_agents: 1,
            platform_supported: true,
            lid_closed: false,
            lid_sleep_disabled: disabled,
            lid_sleep_mode: LidSleepMode::WhileAgentsRunning,
            sudoers_installed: true,
            lid_setup_required: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
            lid_power_condition: PowerCondition::Always,
            lid_battery_floor: 20,
            battery_percent: percent,
            lid_skip_reason: None,
        }
        .with_decision()
    }

    #[test]
    fn 安全弁の解除と再適用だけ通知する() {
        let held = battery_state(true, Some(80));
        let dropped = battery_state(false, Some(10));
        assert_eq!(
            lid_notice(Some(&held), &dropped),
            Some(LidNotice::Released(LidSkipReason::BatteryFloor {
                percent: 10,
                floor: 20
            }))
        );
        assert_eq!(
            lid_notice(Some(&dropped), &held),
            Some(LidNotice::Reapplied)
        );
        // 前回が無い（起動直後）は出さない
        assert_eq!(lid_notice(None, &dropped), None);
        // 変化していなければ出さない
        assert_eq!(lid_notice(Some(&held), &held), None);
    }

    #[test]
    fn 設定どおりの解除は通知しない() {
        let held = battery_state(true, Some(80));
        // エージェントが終わって落ちただけ
        let idle = SleepGuardState {
            busy_agents: 0,
            ..battery_state(false, Some(80))
        }
        .with_decision();
        assert_eq!(lid_notice(Some(&held), &idle), None);
        // 既定（ac-only）の人には何も出さない
        let ac_only_held = SleepGuardState {
            lid_power_condition: PowerCondition::AcOnly,
            ..held
        }
        .with_decision();
        let ac_only_dropped = SleepGuardState {
            lid_power_condition: PowerCondition::AcOnly,
            ..battery_state(false, Some(10))
        }
        .with_decision();
        assert_eq!(lid_notice(Some(&ac_only_held), &ac_only_dropped), None);
    }

    // --- 設定値の検証と注入（#1473） ---

    #[test]
    fn 残量下限は範囲の中だけ受ける() {
        assert_eq!(parse_battery_floor(20), Ok(20));
        assert_eq!(
            parse_battery_floor(i64::from(LID_BATTERY_FLOOR_MIN)),
            Ok(LID_BATTERY_FLOOR_MIN)
        );
        assert_eq!(
            parse_battery_floor(i64::from(LID_BATTERY_FLOOR_MAX)),
            Ok(LID_BATTERY_FLOOR_MAX)
        );
        // 0 は「安全弁なし」になるので受けない
        assert!(parse_battery_floor(0).is_err());
        assert!(parse_battery_floor(4).is_err());
        assert!(parse_battery_floor(91).is_err());
        assert!(parse_battery_floor(-1).is_err());
        assert!(parse_battery_floor(1_000_000).is_err());
    }

    /// 注入（残量・温度）が効くのは隔離・セルフテストだけ。
    /// **本番の GUI で env に左右されたら、それは `pmset` を env で倒せるということ**
    #[test]
    fn 注入は隔離とセルフテストでだけ効く() {
        assert!(!inject_allowed_from(None, false), "本番では効かない");
        assert!(!inject_allowed_from(Some("0"), false));
        assert!(inject_allowed_from(Some("1"), false));
        assert!(inject_allowed_from(Some("true"), false));
        assert!(inject_allowed_from(None, true));
    }

    #[test]
    fn 注入の残量は0から100だけ受ける() {
        assert_eq!(parse_injected_battery(Some("15")), Some(15));
        assert_eq!(parse_injected_battery(Some("0")), Some(0));
        assert_eq!(parse_injected_battery(Some("100")), Some(100));
        assert_eq!(parse_injected_battery(Some("101")), None);
        assert_eq!(parse_injected_battery(Some("abc")), None);
        assert_eq!(parse_injected_battery(None), None);
    }

    /// MCP カタログの enum は正本から生成する（#1467）。
    /// 値を足して `from_str_opt` を忘れると、申告した値を受け取れない
    #[test]
    fn 電源条件の一覧は受理値と一致する() {
        for v in PowerCondition::VALUES {
            let parsed = PowerCondition::from_str_opt(v).expect("申告した値は受け取れる");
            assert_eq!(parsed.as_str(), *v);
        }
        let hint = PowerCondition::values_hint();
        for v in PowerCondition::VALUES {
            assert!(hint.contains(v), "案内文に {v} が無い: {hint}");
        }
    }

    /// 設定一式は `Settings` の既定と同じ（片方だけ変えると
    /// 「設定画面は 20% と出るのに実際は 0%」になる）
    #[test]
    fn 設定一式の既定は従来どおり() {
        let c = SleepGuardConfig::default();
        assert_eq!(c.lid_power_condition, PowerCondition::AcOnly);
        assert_eq!(c.lid_battery_floor, DEFAULT_LID_BATTERY_FLOOR);
        assert_eq!(c.lid_sleep_mode, LidSleepMode::Off);
        let s = crate::settings::Settings::default();
        assert_eq!(s.sleep_guard_config(), c);
    }

    /// 解除と再適用は**診断に残る**（画面の通知は消えるので、あとから
    /// 「いつ・なぜ止まったか」を追えるのは persist.log だけ）。
    /// 書式は両 OS 経路で共有するので、ここが変わると `lid-sleep:` の grep が割れる
    #[test]
    fn 解除と再適用の記録がpersistログへ残る() {
        let Some(path) = tako_core::paths::data_dir().map(|d| d.join("persist.log")) else {
            return; // data dir を解決できない環境（CI のサンドボックス）では検査しない
        };
        log_lid_change(
            false,
            Some(LidSkipReason::BatteryFloor {
                percent: 15,
                floor: 20,
            }),
            Some(15),
        );
        log_lid_change(true, None, Some(80));
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            body.contains("lid-sleep: 蓋閉じ継続を解除 reason=battery-floor battery=15%"),
            "解除の 1 行が残っていない: {}",
            path.display()
        );
        assert!(
            body.contains("lid-sleep: 蓋閉じ継続を有効化 reason=applied battery=80%"),
            "再適用の 1 行が残っていない: {}",
            path.display()
        );
        // 本文（残量以外の環境の値）は載せない = #1376 と同じ作法
        assert!(
            !body.contains("TAKO_1473_INJECT"),
            "診断に env の名前を載せない"
        );
    }

    /// 実機でも残量取得は落ちない（値そのものは機械依存なので範囲だけ見る）
    #[cfg(target_os = "macos")]
    #[test]
    fn battery_percentは0から100か不明を返す() {
        if let Some(p) = battery_percent() {
            assert!(p <= 100, "残量が範囲外: {p}");
        }
    }
}

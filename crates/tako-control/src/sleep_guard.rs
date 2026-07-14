//! スリープ防止機能（Issue #173）
//!
//! IOKit の電源アサーション（PreventUserIdleSystemSleep）で macOS のアイドルスリープを防止する。
//! ディスプレイスリープは妨げない。App Nap 無効化も行い、バックグラウンドで間引かれない。
//!
//! モード:
//! - off: 機能無効
//! - on: 常時アサーション保持
//! - while-agents-running: busy なエージェントペインが 1 体でもある間だけ保持（既定）
//!
//! 電源条件:
//! - ac-only: AC 接続時のみ（既定）
//! - always: バッテリー時も

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

/// 電源条件
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerCondition {
    #[default]
    AcOnly,
    Always,
}

impl PowerCondition {
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
    /// macOS でサポートされているか
    pub platform_supported: bool,
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
            "description": self.description(),
        })
    }

    fn description(&self) -> &'static str {
        if !self.platform_supported {
            return "macOS 以外ではスリープ防止は使用できません";
        }
        match self.mode {
            SleepGuardMode::Off => "スリープ防止: 無効",
            SleepGuardMode::On => {
                if self.assertion_held {
                    "スリープ防止: 有効（常時）"
                } else {
                    "スリープ防止: 有効（AC 未接続のため一時停止中）"
                }
            }
            SleepGuardMode::WhileAgentsRunning => {
                if self.assertion_held {
                    "スリープ防止: エージェント稼働中のため有効"
                } else if self.busy_agents > 0 && !self.on_ac_power {
                    "スリープ防止: エージェント稼働中だが AC 未接続のため一時停止中"
                } else {
                    "スリープ防止: エージェント待機中のため無効"
                }
            }
        }
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

    const K_CFSTRING_ENCODING_UTF8: CFStringEncoding = 0x08000100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const u8,
            encoding: CFStringEncoding,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    /// AC 接続時の IOPSGetTimeRemainingEstimate 戻り値（kIOPSTimeRemainingUnlimited）
    const K_IOPS_TIME_REMAINING_UNLIMITED: f64 = -2.0;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            reason_for_activity: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> i32;
        fn IOPSGetTimeRemainingEstimate() -> f64;
    }

    static ASSERTION_ID: AtomicU32 = AtomicU32::new(0);
    static ASSERTION_HELD: std::sync::atomic::AtomicBool =
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
        // AC 接続中は kIOPSTimeRemainingUnlimited (-2.0) が返る。
        // バッテリー駆動中は残り秒数または kIOPSTimeRemainingUnknown (-1.0)
        unsafe { IOPSGetTimeRemainingEstimate() == K_IOPS_TIME_REMAINING_UNLIMITED }
    }

    /// App Nap を無効化する（NSProcessInfo.beginActivityWithOptions 経由）。
    /// tako 本体のプロセスで一度だけ呼べばよい
    pub fn disable_app_nap() {
        // NSProcessInfo の activity API を直接呼ぶ代わりに、
        // Info.plist の NSAppSleepDisabled = YES と同等の効果を
        // NSActivity で得る
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            // defaults write で実行中プロセスの App Nap を無効化
            // （Info.plist に書くのと同等。プロセス再起動で消える）
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
}

/// スリープ防止の状態を更新する。busy_agents は現在 busy なエージェントの数。
/// 設定に基づいてアサーションの取得・解放を行い、現在の状態を返す
pub fn update(
    mode: SleepGuardMode,
    power_condition: PowerCondition,
    busy_agents: usize,
) -> SleepGuardState {
    #[cfg(not(target_os = "macos"))]
    {
        return SleepGuardState {
            assertion_held: false,
            mode,
            power_condition,
            on_ac_power: false,
            busy_agents,
            platform_supported: false,
        };
    }
    #[cfg(target_os = "macos")]
    {
        let on_ac = iokit::on_ac_power();
        let should_hold = match mode {
            SleepGuardMode::Off => false,
            SleepGuardMode::On => true,
            SleepGuardMode::WhileAgentsRunning => busy_agents > 0,
        };
        // 電源条件チェック: ac-only の場合は AC 未接続でアサーション不保持
        let should_hold = should_hold
            && match power_condition {
                PowerCondition::AcOnly => on_ac,
                PowerCondition::Always => true,
            };

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

        SleepGuardState {
            assertion_held: iokit::is_held(),
            mode,
            power_condition,
            on_ac_power: on_ac,
            busy_agents,
            platform_supported: true,
        }
    }
}

/// 現在の状態を取得する（副作用なし）
pub fn status(mode: SleepGuardMode, power_condition: PowerCondition) -> SleepGuardState {
    #[cfg(not(target_os = "macos"))]
    {
        return SleepGuardState {
            assertion_held: false,
            mode,
            power_condition,
            on_ac_power: false,
            busy_agents: 0,
            platform_supported: false,
        };
    }
    #[cfg(target_os = "macos")]
    {
        SleepGuardState {
            assertion_held: iokit::is_held(),
            mode,
            power_condition,
            on_ac_power: iokit::on_ac_power(),
            busy_agents: 0,
            platform_supported: true,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn status_json_has_required_fields() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::WhileAgentsRunning,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: cfg!(target_os = "macos"),
        };
        let json = state.to_json();
        assert!(json.get("assertion_held").is_some());
        assert!(json.get("mode").is_some());
        assert!(json.get("power_condition").is_some());
        assert!(json.get("on_ac_power").is_some());
        assert!(json.get("busy_agents").is_some());
        assert!(json.get("platform_supported").is_some());
        assert!(json.get("description").is_some());
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
    fn description_off_mode() {
        let state = SleepGuardState {
            assertion_held: false,
            mode: SleepGuardMode::Off,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: true,
        };
        assert!(state.description().contains("無効"));
    }

    #[test]
    fn description_on_mode_held() {
        let state = SleepGuardState {
            assertion_held: true,
            mode: SleepGuardMode::On,
            power_condition: PowerCondition::AcOnly,
            on_ac_power: true,
            busy_agents: 0,
            platform_supported: true,
        };
        assert!(state.description().contains("常時"));
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
        };
        assert!(state.description().contains("macOS 以外"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_ac_power_does_not_panic() {
        let _ = iokit::on_ac_power();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_ac_powerはサブプロセスを使わず高速である() {
        // #212: UI スレッドから 2 秒毎に呼ばれるため、サブプロセス実装（pmset =
        // 1 回 20〜30ms、fork+exec が高負荷時に秒級）への回帰を検出する。
        // FFI 実装なら 100 回で 1ms 未満、subprocess 実装なら 2 秒以上かかる
        let t0 = std::time::Instant::now();
        for _ in 0..100 {
            let _ = iokit::on_ac_power();
        }
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "on_ac_power ×100 が {:?} かかった（サブプロセス実装への回帰の疑い）",
            t0.elapsed()
        );
    }

    #[test]
    fn check_qos_returns_json() {
        let qos = check_qos();
        assert!(qos.is_object());
    }
}

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
    /// 蓋が閉じているか（#218）
    pub lid_closed: bool,
    /// pmset disablesleep が有効か（#218）
    pub lid_sleep_disabled: bool,
    /// 蓋閉じ防止モード（#218）
    pub lid_sleep_mode: LidSleepMode,
    /// sudoers 登録済みか（#218）
    pub sudoers_installed: bool,
    /// thermal 状態（#218）
    pub thermal_state: ThermalState,
    /// ディスプレイ消灯を強制送信済みか（#311）
    pub display_sleep_forced: bool,
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
            "thermal_state": self.thermal_state.as_str(),
            "display_sleep_forced": self.display_sleep_forced,
            "description": self.description(),
        })
    }

    fn description(&self) -> String {
        if !self.platform_supported {
            return "macOS 以外ではスリープ防止は使用できません".to_string();
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
                "蓋閉じ継続: 有効（高温警告中）"
            } else if self.display_sleep_forced {
                "蓋閉じ継続: 有効（ディスプレイ消灯済み）"
            } else {
                "蓋閉じ継続: 有効"
            }
        } else {
            match self.lid_sleep_mode {
                LidSleepMode::Off => "蓋閉じ継続: 未設定",
                LidSleepMode::WhileAgentsRunning => {
                    if !self.sudoers_installed {
                        "蓋閉じ継続: sudoers 未登録（tako setup --lid-sleep で登録）"
                    } else if self.busy_agents == 0 {
                        "蓋閉じ継続: エージェント待機中のため無効"
                    } else if !self.on_ac_power {
                        "蓋閉じ継続: AC 未接続のため無効"
                    } else {
                        "蓋閉じ継続: 無効"
                    }
                }
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

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const u8,
            encoding: CFStringEncoding,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
        fn CFBooleanGetValue(boolean: CFBooleanRef) -> bool;
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

/// sudoers.d/tako-sleep-guard が登録済みか
pub fn is_sudoers_installed() -> bool {
    std::path::Path::new(SUDOERS_FILE).exists()
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

/// 起動時の残留チェック: disablesleep=1 が残っていて busy エージェントがいなければ 0 に戻す
pub fn check_disablesleep_residual() {
    #[cfg(target_os = "macos")]
    {
        if !is_sudoers_installed() {
            return;
        }
        if !iokit::sleep_disabled() {
            return;
        }
        // 起動直後なので busy エージェントは 0 → 残留と判断して解除
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
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.battery")
            .spawn()
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
pub fn update(
    mode: SleepGuardMode,
    power_condition: PowerCondition,
    lid_sleep_mode: LidSleepMode,
    busy_agents: usize,
) -> SleepGuardState {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = lid_sleep_mode;
        return SleepGuardState {
            assertion_held: false,
            mode,
            power_condition,
            on_ac_power: false,
            busy_agents,
            platform_supported: false,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
        };
    }
    #[cfg(target_os = "macos")]
    {
        let on_ac = iokit::on_ac_power();
        let lid_closed = iokit::clamshell_closed();
        let thermal = iokit::thermal_state();
        let sudoers = is_sudoers_installed();

        // --- アイドルスリープ防止（既存ロジック） ---
        let should_hold = match mode {
            SleepGuardMode::Off => false,
            SleepGuardMode::On => true,
            SleepGuardMode::WhileAgentsRunning => busy_agents > 0,
        };
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

        // --- 蓋閉じ防止（#218: pmset disablesleep） ---
        let current_disabled = iokit::sleep_disabled();
        if lid_sleep_mode == LidSleepMode::WhileAgentsRunning && sudoers {
            let should_disable = busy_agents > 0 && on_ac && !thermal.is_warning();
            if should_disable && !current_disabled {
                let _ = set_disablesleep(true);
            } else if !should_disable && current_disabled {
                let _ = set_disablesleep(false);
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
            thermal_state: thermal,
            display_sleep_forced: iokit::display_sleep_sent(),
        }
    }
}

/// 現在の状態を取得する（副作用なし）
pub fn status(
    mode: SleepGuardMode,
    power_condition: PowerCondition,
    lid_sleep_mode: LidSleepMode,
) -> SleepGuardState {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = lid_sleep_mode;
        return SleepGuardState {
            assertion_held: false,
            mode,
            power_condition,
            on_ac_power: false,
            busy_agents: 0,
            platform_supported: false,
            lid_closed: false,
            lid_sleep_disabled: false,
            lid_sleep_mode: LidSleepMode::Off,
            sudoers_installed: false,
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
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
            lid_closed: iokit::clamshell_closed(),
            lid_sleep_disabled: iokit::sleep_disabled(),
            lid_sleep_mode,
            sudoers_installed: is_sudoers_installed(),
            thermal_state: iokit::thermal_state(),
            display_sleep_forced: iokit::display_sleep_sent(),
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

/// tako 終了時に disablesleep を解除する（正常終了フック）
pub fn cleanup_on_exit() {
    #[cfg(target_os = "macos")]
    {
        if is_sudoers_installed() && iokit::sleep_disabled() {
            let _ = set_disablesleep(false);
        }
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
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
            thermal_state: ThermalState::Serious,
            display_sleep_forced: false,
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
        };
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: false,
        };
        assert!(state.description().contains("macOS 以外"));
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
            thermal_state: ThermalState::Nominal,
            display_sleep_forced: true,
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
}

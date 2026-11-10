//! スリープ防止（境界 B9）の macOS の腕が使う IOKit / CoreFoundation / ObjC runtime の FFI（#2857）。
//!
//! #2857 で `sleep_guard.rs` の `mod iokit` をそのまま移した（中身は移設の前後でトークン列が一致）。
//! 電源アサーション（#173）・AC 判定（#212）・バッテリー残量（#1473）・thermal（#218）は
//! [`super`]（`platform::power`）の macOS の腕が、蓋の開閉・`disablesleep` の現在値（#218）と
//! ディスプレイ消灯（#311）は `platform::lid` の macOS の腕が呼ぶ。
//! **UI スレッドから 2 秒ごとに呼ばれる**ので、読み取りはサブプロセスを起こさない（#212）

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
static ASSERTION_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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

/// ディスプレイだけをスリープさせる（#311）。
/// disablesleep=1 の蓋閉じ時に呼ぶ。root 不要。蓋閉じ中はユーザー入力が
/// ないため、一度送ればディスプレイは消灯を維持する。
/// 蓋閉じ→蓋開け 1 サイクルにつき 1 回だけ呼ぶ（DISPLAY_SLEEP_SENT で制御）
pub fn force_display_sleep() {
    if DISPLAY_SLEEP_SENT.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // GUI のスリープ防止の周期から起こすので PATH で引いた絶対パスで起こす（#2519）
    let _ = std::process::Command::new(tako_core::platform::exe::resolve_for_spawn("pmset"))
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

// 実機の FFI が落ちない・速いこと（#2857 で `sleep_guard.rs` のテストから移した。中身は同じ）
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_ac_power_does_not_panic() {
        let _ = on_ac_power();
    }

    #[test]
    fn clamshell_closed_does_not_panic() {
        let _ = clamshell_closed();
    }

    #[test]
    fn sleep_disabled_does_not_panic() {
        let _ = sleep_disabled();
    }

    #[test]
    fn thermal_state_does_not_panic() {
        let _ = thermal_state();
    }

    #[test]
    fn iokit_ffi_calls_are_fast() {
        // #212: UI スレッドから 2 秒毎に呼ばれるため、サブプロセス実装への回帰を検出する
        let t0 = std::time::Instant::now();
        for _ in 0..100 {
            let _ = on_ac_power();
            let _ = clamshell_closed();
            let _ = sleep_disabled();
            let _ = thermal_state();
        }
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "IOKit FFI ×100 が {:?} かかった（サブプロセス実装への回帰の疑い）",
            t0.elapsed()
        );
    }
}

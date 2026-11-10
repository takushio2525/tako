//! スリープ防止（抽象境界 B9）のうちアイドルスリープの防止と電源の状態。#524 / #2857
//!
//! OS の差はこのモジュールの内側（`imp` の選択と各関数の `cfg`）に閉じている。呼び手の
//! `sleep_guard` は「いつ握る・いつ外す・記録」だけを持ち、OS を意識しない（#2857 で
//! macOS の腕を `sleep_guard.rs` からここへ移した。中身は移設の前後でトークン列が一致）。
//!
//! | OS | 電源要求 | AC 判定 | 残量・thermal |
//! |---|---|---|---|
//! | macOS | IOKit の `PreventUserIdleSystemSleep`（`iokit`。#173） | `IOPSGetTimeRemainingEstimate`（#212） | IOPowerSources（#1473）・`NSProcessInfo.thermalState`（#218） |
//! | Windows | `PowerCreateRequest` + `PowerSetRequest`（下の節） | `GetSystemPowerStatus` | 取らない（`None` / nominal） |
//! | その他 | 持たない（[`supported`] が false） | false | 取らない |
//!
//! 蓋を閉じたまま走らせ続ける制御（蓋の開閉・`pmset disablesleep`・電源プランの lid action）は
//! [`crate::platform::lid`] が持つ。
//!
//! ## なぜ `SetThreadExecutionState` を使わないか
//!
//! Windows の実現手段は 2 つある。
//!
//! | API | 効く範囲 | 診断 |
//! |---|---|---|
//! | `SetThreadExecutionState` | **呼んだスレッド**。そのスレッドが終わると解除される | `powercfg /requests` に出ない |
//! | `PowerCreateRequest` + `PowerSetRequest` | プロセス（ハンドル） | 理由文字列つきで `powercfg /requests` に出る |
//!
//! tako はスリープ防止の更新（`sleep_guard::update`）を UI の定期処理から呼ぶが、
//! 状態取得（`status`）は IPC の別スレッドからも来る。スレッドに縛られる前者だと
//! 「取得したスレッドが死んで無言で解除される」事故が起きうるので、
//! **プロセススコープで、しかも外から観測できる**後者を採る。
//!
//! ## 何が macOS より落ちるか
//!
//! thermal 監視（`NSProcessInfo.thermalState`）・ディスプレイ強制消灯（#311）・
//! 蓋の開閉検知（`AppleClamshellState`）は macOS 固有で、Windows には対応する
//! 仕組みが無い（蓋の開閉は `RegisterPowerSettingNotification` にウィンドウ
//! ハンドルが要り、`tako-control` からは取れない）。
//!
//! **蓋を閉じたまま実行を継続する制御そのものは Windows にもある**
//! （電源プランの `GUID_LIDCLOSE_ACTION` を倒す。[`crate::platform::lid`]）。
//! sudoers 連携は macOS が管理者権限を要するための手段であって、
//! Windows では権限そのものが不要なので「落ちている」わけではない。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Thermal 状態（NSProcessInfo.thermalState の Rust 表現）
///
/// #2857 で `sleep_guard.rs` から移した（境界の [`thermal_state`] が返す型を境界に置き、
/// 境界から `sleep_guard` を参照しないため。`sleep_guard::ThermalState` は re-export）
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

/// この環境でスリープ防止（アイドルスリープの抑止）が使えるか
pub fn supported() -> bool {
    imp::SUPPORTED
}

/// アイドルスリープ防止を保持 / 解除する。同じ状態への再要求は何もしない。
///
/// `reason` は診断ツール（Windows = `powercfg /requests` / macOS = `pmset -g assertions`）に出る文字列。
/// 非 ASCII を書いてよいかは [`reason_accepts_non_ascii`]
pub fn set_hold(hold: bool, reason: &str) {
    imp::set_hold(hold, reason);
}

/// 現在アサーションを保持しているか
pub fn is_held() -> bool {
    imp::is_held()
}

/// AC 電源に接続されているか。判定できない環境では `true`
/// （既定の電源条件が `ac-only` なので、判定不能を「バッテリー」と誤ると
/// スリープ防止が一切効かなくなる。デスクトップ機がこれに当たる）
pub fn on_ac_power() -> bool {
    imp::on_ac_power()
}

/// 電源要求の理由（[`set_hold`] の `reason`）に非 ASCII を書いてよいか（#524 / #2857）。
///
/// macOS は `pmset -g assertions` に UTF-8 のまま出る（#173 から日本語の文）。Windows は
/// `powercfg /requests` のコンソール出力で化けるので ASCII に固定する。文そのものは呼び手
/// （`sleep_guard::assertion_reason`）が持つ
pub fn reason_accepts_non_ascii() -> bool {
    cfg!(target_os = "macos")
}

/// いまのバッテリー残量（%）を OS から読む（#1473）。
///
/// デスクトップ機・バッテリーを持たない環境・読めない OS は `None`。
/// 検証用の注入は呼び手（`sleep_guard::battery_percent`）が先に通す。
/// **UI スレッドから 2 秒ごとに呼ばれる**のでサブプロセスは使わない（#212）
pub fn battery_percent() -> Option<u8> {
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

/// いまの thermal 状態を OS から読む（#218 / #1473）。
///
/// 検証用の注入は呼び手（`sleep_guard::current_thermal`）が先に通す
pub fn thermal_state() -> ThermalState {
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

/// tako の終了時に電源要求を外す（正常終了フック。#524）。
///
/// Windows の電源要求はプロセス終了で OS が回収するが、明示的に解除しておく
/// （`powercfg /requests` に残らないことを終了直後に確認できる）。macOS のアサーションは
/// #2857 より前と同じく触らない（プロセスの終了で OS が回収する）
pub fn release_on_exit() {
    #[cfg(not(target_os = "macos"))]
    {
        crate::platform::power::set_hold(false, "");
    }
}

/// バッテリーの設定画面を開く URL（蓋閉じ継続の案内のフォールバック）。開けない OS は `None`
pub fn battery_settings_url() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        Some("x-apple.systempreferences:com.apple.preference.battery")
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// [`check_qos`] の `ps` 1 回の上限（#1984）。自プロセス 1 行を引くだけなので桁で足りる
#[cfg(target_os = "macos")]
const QOS_PS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// プロセスの QoS を確認する（macOS のみ、診断用）。
///
/// `ps` は**上限つきで**待つ（#1984。`Command::output()` は子が終わらない・パイプの書き手が
/// 長生きのプロセスへ漏れると永久に返らない = #1979 の本番ハングと同じ型）
pub fn check_qos() -> Value {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id().to_string();
        match tako_core::probe::output_with_timeout(
            "ps",
            &["-p", &pid, "-o", "pri,nice"],
            QOS_PS_TIMEOUT,
        ) {
            tako_core::probe::Outcome::Done { stdout, .. } => {
                let stdout = String::from_utf8_lossy(&stdout);
                json!({ "qos_info": stdout.trim() })
            }
            tako_core::probe::Outcome::TimedOut { label, waited, .. } => {
                json!({ "error": format!("{label} が {} 秒で返らなかった", waited.as_secs()) })
            }
            tako_core::probe::Outcome::Failed { reason, .. } => json!({ "error": reason }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({ "qos_info": "macOS 以外では非対応" })
    }
}

/// `SYSTEM_POWER_STATUS` の 2 フィールドから AC 接続を判定する純粋関数。
///
/// - `ac_line`: 0 = バッテリー、1 = AC、255 = 不明
/// - `battery_flag`: 128 = システムにバッテリーが無い（= 据え置き機）
///
/// **Windows 実機が無くてもテストできる**ように `cfg` の外に置く
/// （`platform::locale` の parse 関数と同じ方針）
#[cfg_attr(not(windows), allow(dead_code))]
fn on_ac_from_status(ac_line: u8, battery_flag: u8) -> bool {
    const AC_OFFLINE: u8 = 0;
    const NO_SYSTEM_BATTERY: u8 = 128;
    if battery_flag == NO_SYSTEM_BATTERY {
        return true; // バッテリーが無い機械は常に AC
    }
    ac_line != AC_OFFLINE // 1 = AC、255 = 不明はどちらも AC 扱い
}

/// macOS の FFI（#2857 で `sleep_guard.rs` の `mod iokit` を移した）。
/// 蓋の側（clamshell・`disablesleep`・ディスプレイ消灯）は `platform::lid` の macOS の腕が使う
#[cfg(target_os = "macos")]
pub(super) mod iokit;

/// macOS の腕（IOKit の電源アサーション。#173）。
/// 握る / 外すの形は #2857 より前の `sleep_guard::update` の macOS の経路のまま
#[cfg(target_os = "macos")]
mod imp {
    use super::iokit;

    pub(super) const SUPPORTED: bool = true;

    pub(super) fn is_held() -> bool {
        iokit::is_held()
    }

    pub(super) fn set_hold(hold: bool, reason: &str) {
        if hold && !iokit::is_held() {
            iokit::create_assertion(reason);
        } else if !hold && iokit::is_held() {
            iokit::release_assertion();
        }
    }

    /// UI スレッドから 2 秒毎に呼ばれるので IOKit の FFI で読む（サブプロセスは使わない。#212）
    pub(super) fn on_ac_power() -> bool {
        iokit::on_ac_power()
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    pub(super) const SUPPORTED: bool = true;

    type Handle = *mut c_void;

    /// `POWER_REQUEST_CONTEXT_VERSION`
    const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;
    /// `POWER_REQUEST_CONTEXT_SIMPLE_STRING`。理由を文字列 1 本で渡す
    const POWER_REQUEST_CONTEXT_SIMPLE_STRING: u32 = 0x0000_0001;
    /// `PowerRequestSystemRequired`（`POWER_REQUEST_TYPE`）。
    /// アイドルスリープだけを止める。ディスプレイ消灯は妨げない
    /// （macOS 実装が `PreventUserIdleSystemSleep` を使うのと同じ粒度）。
    ///
    /// **値を間違えると別種の要求になる**。列挙は宣言順に
    /// `DisplayRequired = 0` / `SystemRequired = 1` / `AwayModeRequired = 2` /
    /// `ExecutionRequired = 3`。3 を渡すと「プロセスを中断・終了させない」要求になり、
    /// アイドルスリープは止まらない（#524 の実機検証で実際に踏んだ。
    /// `powercfg /requests` の出力欄が SYSTEM ではなく EXECUTION になるのが目印）。
    /// 正しく効いていれば SYSTEM の欄に `[PROCESS] …\tako-app.exe` が並ぶ
    const POWER_REQUEST_SYSTEM_REQUIRED: u32 = 1;

    /// `REASON_CONTEXT`（winnt.h）。`Flags` が SIMPLE_STRING のときは
    /// union の先頭 `SimpleReasonString`（LPWSTR）だけが使われる。
    ///
    /// **union の実サイズぶんを確保する**こと。最大の変体は `Detailed`
    /// （HMODULE + ULONG + ULONG + LPWSTR\* = 24 バイト）なので、
    /// ポインタ 1 本だけの構造体にすると API が構造体ごと読んだときに
    /// 確保外へはみ出す。x64 で 32 バイトになることをコンパイル時に固定する
    #[repr(C)]
    struct ReasonContext {
        version: u32,
        flags: u32,
        simple_reason_string: *const u16,
        _detailed_tail: [usize; 2],
    }
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(std::mem::size_of::<ReasonContext>() == 32);

    #[link(name = "kernel32")]
    extern "system" {
        fn PowerCreateRequest(context: *const ReasonContext) -> Handle;
        fn PowerSetRequest(request: Handle, request_type: u32) -> i32;
        fn PowerClearRequest(request: Handle, request_type: u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    /// `SYSTEM_POWER_STATUS`（winbase.h）
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    /// 保持中の電源要求ハンドル（`isize` で持つ。生ポインタは Send でないため）。
    /// 0 = 保持していない
    static REQUEST: Mutex<isize> = Mutex::new(0);
    /// ロックを取らずに読めるようにした保持状態（`status()` は毎回引かれる）
    static HELD: AtomicBool = AtomicBool::new(false);

    pub(super) fn is_held() -> bool {
        HELD.load(Ordering::Relaxed)
    }

    pub(super) fn set_hold(hold: bool, reason: &str) {
        let mut slot = match REQUEST.lock() {
            Ok(slot) => slot,
            // 別スレッドが panic してもスリープ防止は続けたい（毒された値をそのまま使う）
            Err(poisoned) => poisoned.into_inner(),
        };
        if hold == (*slot != 0) {
            return; // 既に目的の状態
        }
        if hold {
            let wide: Vec<u16> = reason.encode_utf16().chain(std::iter::once(0)).collect();
            let context = ReasonContext {
                version: POWER_REQUEST_CONTEXT_VERSION,
                flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                simple_reason_string: wide.as_ptr(),
                _detailed_tail: [0; 2],
            };
            // SAFETY: context と wide は呼び出しの間だけ生きていればよい
            // （PowerCreateRequest は文字列を内部へコピーする）。
            // 戻り値は INVALID_HANDLE_VALUE / NULL を検査してから使う
            let handle = unsafe { PowerCreateRequest(&context) };
            if handle.is_null() || handle == (-1isize as Handle) {
                return; // 取得失敗。次の tick で再挑戦する
            }
            // SAFETY: handle は直前に妥当性を検査済み
            if unsafe { PowerSetRequest(handle, POWER_REQUEST_SYSTEM_REQUIRED) } == 0 {
                // SAFETY: 失敗した要求のハンドルは即座に閉じる（リークさせない）
                unsafe { CloseHandle(handle) };
                return;
            }
            *slot = handle as isize;
            HELD.store(true, Ordering::Relaxed);
        } else {
            let handle = *slot as Handle;
            // SAFETY: slot が 0 でないときだけ来るので、handle は生きている
            unsafe {
                PowerClearRequest(handle, POWER_REQUEST_SYSTEM_REQUIRED);
                CloseHandle(handle);
            }
            *slot = 0;
            HELD.store(false, Ordering::Relaxed);
        }
    }

    pub(super) fn on_ac_power() -> bool {
        let mut status = SystemPowerStatus::default();
        // SAFETY: 出力先はスタック上の生存する構造体
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return true; // 問い合わせ自体が失敗 = 判定不能
        }
        super::on_ac_from_status(status.ac_line_status, status.battery_flag)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    /// Linux 等は防止手段を持たない（従来どおり `platform_supported: false`）
    pub(super) const SUPPORTED: bool = false;

    pub(super) fn is_held() -> bool {
        false
    }

    pub(super) fn set_hold(_hold: bool, _reason: &str) {}

    pub(super) fn on_ac_power() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac接続の判定() {
        assert!(on_ac_from_status(1, 1), "AC 接続");
        assert!(!on_ac_from_status(0, 1), "バッテリー駆動");
    }

    #[test]
    fn バッテリーの無い機械は常にac扱い() {
        // 据え置き機は ac_line_status が 255（不明）で返ることがある
        assert!(on_ac_from_status(255, 128));
        assert!(on_ac_from_status(0, 128), "バッテリーが無いなら 0 でも AC");
    }

    #[test]
    fn 判定不能はac扱いにする() {
        // ac-only が既定なので、不明を「バッテリー」に倒すと機能が死ぬ
        assert!(on_ac_from_status(255, 255));
    }

    /// 実機での取得・解除。Windows と macOS は実際に握って外す（どちらもプロセスに結びついた要求で、
    /// 電源設定は書かない。#2857 から macOS も腕を通る）。その他の OS は素通りする（`supported()` が false）
    #[test]
    fn 保持と解除で状態が戻る() {
        let _serial = crate::platform::testing::machine_state_lock();
        assert!(!is_held(), "テスト開始時は保持していない");
        set_hold(true, "tako: self test");
        assert_eq!(is_held(), supported(), "対応環境なら保持されている");
        // 同じ状態への再要求は冪等
        set_hold(true, "tako: self test");
        assert_eq!(is_held(), supported());
        set_hold(false, "");
        assert!(!is_held(), "解除で元に戻る");
    }

    #[test]
    fn 保持していない状態からの解除は無害() {
        let _serial = crate::platform::testing::machine_state_lock();
        set_hold(false, "");
        assert!(!is_held());
    }
}

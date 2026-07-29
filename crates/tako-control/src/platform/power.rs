//! スリープ防止（抽象境界 B9）のうち **macOS 以外**の実装。#524
//!
//! macOS は IOKit 電源アサーション実装が `sleep_guard::iokit` にあり、そちらが正
//! （#173 / #218 / #311 / #212 の積み重ねがあるので、Windows 実装を入れるために
//! 移設して壊すことはしない）。このモジュールは `sleep_guard` の非 macOS 経路が
//! 呼ぶ差し込み口で、`cfg` はこのファイルの内側に閉じている。
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
//! 蓋閉じ継続（clamshell）・sudoers 連携・thermal 監視・ディスプレイ強制消灯は
//! macOS 固有で、Windows には対応する仕組みが無い（蓋閉じ時の動作は電源プランの
//! ポリシーで、アプリから一時的に上書きする API は無い）。マトリクスの
//! `tako_sleep_guard` はこれを理由に Windows で `Degraded`。

/// この環境でスリープ防止（アイドルスリープの抑止）が使えるか
pub fn supported() -> bool {
    imp::SUPPORTED
}

/// アイドルスリープ防止を保持 / 解除する。同じ状態への再要求は何もしない。
///
/// `reason` は診断ツール（`powercfg /requests`）に出る文字列
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

/// `SYSTEM_POWER_STATUS` の 2 フィールドから AC 接続を判定する純粋関数。
///
/// - `ac_line`: 0 = バッテリー、1 = AC、255 = 不明
/// - `battery_flag`: 128 = システムにバッテリーが無い（= 据え置き機）
///
/// **Windows 実機が無くてもテストできる**ように `cfg` の外に置く
/// （`platform::locale` の parse 関数と同じ方針）
fn on_ac_from_status(ac_line: u8, battery_flag: u8) -> bool {
    const AC_OFFLINE: u8 = 0;
    const NO_SYSTEM_BATTERY: u8 = 128;
    if battery_flag == NO_SYSTEM_BATTERY {
        return true; // バッテリーが無い機械は常に AC
    }
    ac_line != AC_OFFLINE // 1 = AC、255 = 不明はどちらも AC 扱い
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

#[cfg(not(windows))]
mod imp {
    /// macOS は `sleep_guard::iokit` が担当するのでここへは来ない。
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

    /// 実機での取得・解除。Windows 以外では素通りする（`supported()` が false）
    #[test]
    fn 保持と解除で状態が戻る() {
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
        set_hold(false, "");
        assert!(!is_held());
    }
}

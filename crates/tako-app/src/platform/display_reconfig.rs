//! 画面構成の変更の通知（境界 B33。**診断専用**。#2666）
//!
//! 蓋の開閉・画面スリープで内蔵ディスプレイが画面構成から出入りする瞬間（macOS の画面再構成）に
//! UI が 0.5〜1.8 秒止まる。その塞がりは tako の計測区間の外なので、UI ストールの分類が名指せるよう
//! OS の通知の時刻だけを `tako_control::diag` へ渡す（分類・perf.log の行・窓の決め方は diag 側）。
//!
//! - macOS: `CGDisplayRegisterReconfigurationCallback` を GUI の起動時に 1 回だけ登録する。
//!   コールバックは時刻を取って畳むだけ（I/O しない・重い処理をしない）。登録は外さない（プロセスの寿命と同じ）
//! - それ以外: 何もしない（Windows の画面構成の変更のストールは未実測。#643 の追跡対象）
//!
//! コールバックが来るスレッド: 本番の統合ログ（11/01 15:03・tako-app）で、CoreGraphics の再構成の通知
//! （`CGSDisplayNotifyProc` の kCGSDisplayWillReconfigure / DidReconfigure2 / ReconfigureHappened）は
//! どれもプロセスで最も若い TID（= メインスレッド）で処理されていた。コールバックはその処理の中から
//! 呼ばれるので、メインスレッドで来る見込み。実際にどのスレッドだったかは perf.log の再構成の行
//! （「コールバックはメインスレッド」）に毎回残す（本番の確認用）。
//!
//! 検証では蓋の開閉を起こせず仮想ディスプレイも触らないので、[`synthesize`] が本物のコールバックの
//! 関数を合成の印つきで呼ぶ（`tako_control::diag::Issue2666Inject` から。検証プロセスでだけ効く）。
//! A/B は `TAKO_2666_LEGACY=1`（登録しない・記録しない）

use tako_control::diag::ReconfigPhase;

/// 画面構成の変更の通知を登録する（GUI のメインスレッドで呼ぶ。多重呼び出しは無視）
pub fn watch() {
    imp::watch();
}

/// 通知を合成する（検証専用の注入から呼ぶ。OS の通知と同じ関数を通る）
pub fn synthesize(phase: ReconfigPhase, display: u32) {
    imp::synthesize(phase, display);
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tako_control::diag::{self, ReconfigEvent, ReconfigPhase};

    /// `kCGDisplayBeginConfigurationFlag`（変更の前の通知）
    const BEGIN_CONFIGURATION_FLAG: u32 = 1 << 0;
    /// `kCGDisplayDesktopShapeChangedFlag`（合成の end に載せる変更の内容）
    const DESKTOP_SHAPE_CHANGED_FLAG: u32 = 1 << 12;

    /// 合成の印（`userInfo` にこの番地を渡す。OS は登録時の NULL を渡す）
    static SYNTHETIC_MARK: u8 = 0;

    /// `CGDisplayReconfigurationCallBack`
    type Callback = extern "C" fn(display: u32, flags: u32, user_info: *mut c_void);

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayRegisterReconfigurationCallback(
            callback: Callback,
            user_info: *mut c_void,
        ) -> i32;
    }

    // libSystem（常に結ばれている）
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    /// 本物のコールバック。**panic させない**（C から呼ばれるので巻き戻せない）。時刻を取って畳むだけ
    extern "C" fn on_reconfig(display: u32, flags: u32, user_info: *mut c_void) {
        let phase = if flags & BEGIN_CONFIGURATION_FLAG != 0 {
            ReconfigPhase::Begin
        } else {
            ReconfigPhase::End
        };
        diag::record_display_reconfig(ReconfigEvent {
            phase,
            display,
            flags,
            main_thread: unsafe { pthread_main_np() } != 0,
            synthetic: std::ptr::eq(user_info.cast_const().cast::<u8>(), &SYNTHETIC_MARK),
        });
    }

    pub(super) fn watch() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        // A/B（`TAKO_2666_LEGACY=1`）: 修正前は登録しない
        if diag::issue2666_legacy() || STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        let err =
            unsafe { CGDisplayRegisterReconfigurationCallback(on_reconfig, std::ptr::null_mut()) };
        if err != 0 {
            diag::perf_log(&format!(
                "画面構成の変更の通知を登録できなかった（CGError {err}）。UI ストールの分類は\
                 画面構成の変更を名指さない。#2666"
            ));
        }
    }

    pub(super) fn synthesize(phase: ReconfigPhase, display: u32) {
        let flags = match phase {
            ReconfigPhase::Begin => BEGIN_CONFIGURATION_FLAG,
            ReconfigPhase::End => DESKTOP_SHAPE_CHANGED_FLAG,
        };
        on_reconfig(
            display,
            flags,
            std::ptr::addr_of!(SYNTHETIC_MARK).cast_mut().cast(),
        );
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use tako_control::diag::ReconfigPhase;

    pub(super) fn watch() {}

    pub(super) fn synthesize(_phase: ReconfigPhase, _display: u32) {}
}

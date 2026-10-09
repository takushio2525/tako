//! 利用者が待っている重い background 処理を、OS の省電力（macOS の App Nap）に
//! 間引かせない（#1916）。
//!
//! ## 何が起きていたか（実測・release・Apple M5 Max）
//!
//! macOS は見えていない / 前面でないアプリを App Nap で間引く。間引かれたプロセスは
//! スレッドの QoS（GCD の既定キューでも専用スレッドでも）に関わらず E コアへ寄せられ、
//! プロセスの優先度が 46 → 4 に落ちる。10 MB の構文の塗り（243.6 G 命令）は、
//! 間引かれる前は 12.0 秒（P コア 99.7%）、間引かれた後は 26.9〜33.4 秒（P コア 0〜9%）
//! だった。命令数は同じ（243.6〜244.2 G）なので、塗りの中身ではなく走る場所の差。
//! 隔離 GUI は前面に出ないので起動から約 30 秒で必ず間引かれ、「回を追うごとに遅くなる」
//! ように見えていた。利用者でも、別のアプリを前面にしている間に AI が `tako open` で
//! 大きいファイルを開くと同じことが起きる。
//!
//! ## 何をするか
//!
//! [`UserWork::begin`] で握ってから落とすまで、`NSProcessInfo` の
//! `beginActivityWithOptions:reason:`（利用者が始めた処理。アイドルスリープは妨げない）を
//! 保つ。間引かれた後に握っても戻る（実測: 優先度 4 のプロセスで 12.1〜12.2 秒・P コア 99%）。
//! 重なった処理は数で束ね、OS への依頼は常に 1 本だけ（最初の 1 つで始め、最後の 1 つで終える）。
//! macOS 以外は何もしない（Windows の電力スロットリングは未実測）。
//!
//! 握るのは構文の塗り（[`UserWork::begin`]。#1916）とプレビューの読み込み
//! （[`UserWork::begin_load`]。PDF のラスタライズ・Markdown の組み立て。#1926）。
//! 117 ページの PDF は間引かれたままだと 7.2〜8.0 秒（E コア）、握ると 3.0 秒（P コア）。
//!
//! ## アプリ全体では止めない（#1926）
//!
//! 起動時の `sleep_guard::disable_app_nap`（#173）は `defaults write /proc/<pid>/Info …` で、
//! macOS には `/proc` が無いので一度も効いていなかった。直して寿命の間止めるのではなく消した。
//! 実測（release・隔離 GUI）で、止めて得をするのは利用者が待つ重い処理だけ（上の 2 つで足りる）で、
//! アイドル時は 341〜365 → 856〜1,154 µW、誰も待っていない裏の処理（ペインへ流れる
//! 30 万行の出力）は P コアへ載る分だけ 110〜319 → 550〜575 mJ の電力になる。ペインの子プロセス
//! （エージェント本体）は tako が間引かれても優先度 31 のままで、影響を受けない。
//! エージェント稼働中は #173 のスリープ防止が tako 自身のプロセスで電源アサーション
//! （PreventUserIdleSystemSleep）を握り、握っている間は OS が App Nap の対象から外す
//! （実測: 優先度 28 のまま）。
//!
//! A/B の腕: `TAKO_1916_LEGACY=1` で塗りの間も握らない（`scripts/test-highlight-app-nap-1916.sh`）、
//! `TAKO_1926_LEGACY=1` で読み込みの間も握らない（`scripts/test-preview-load-app-nap-1926.sh`）

use std::sync::{Mutex, MutexGuard, OnceLock};

/// 握っている間、OS に間引かせない（落とすと終わる）
#[must_use = "落とした時点で依頼が終わる（処理の終わりまで束縛しておく）"]
pub struct UserWork {
    /// 数に入れたか（旧挙動の腕では入れない）
    counted: bool,
}

impl UserWork {
    /// 構文の塗りの間握る（#1916）。重い処理の closure の先頭で `let _work = UserWork::begin();` とする
    pub fn begin() -> Self {
        if legacy() {
            return Self { counted: false };
        }
        Self::hold()
    }

    /// プレビューの読み込み（PDF のラスタライズ・Markdown の組み立て）の間握る（#1926）。
    /// 使い方は [`UserWork::begin`] と同じで、A/B の腕だけが別
    pub fn begin_load() -> Self {
        if legacy_1926() {
            return Self { counted: false };
        }
        Self::hold()
    }

    fn hold() -> Self {
        let mut state = state();
        if state.holders.acquire() {
            state.token = sys::begin();
        }
        Self { counted: true }
    }
}

impl Drop for UserWork {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        let mut state = state();
        if state.holders.release() {
            if let Some(token) = state.token.take() {
                sys::end(token);
            }
        }
    }
}

/// 塗りの間も握らない旧挙動へ戻す A/B の腕（`TAKO_1916_LEGACY=1`）
fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1916_LEGACY").is_some_and(|v| v == "1"))
}

/// 読み込みの間も握らない旧挙動へ戻す A/B の腕（`TAKO_1926_LEGACY=1`）
fn legacy_1926() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1926_LEGACY").is_some_and(|v| v == "1"))
}

/// いま握っている数を数える（純粋な部分。OS への依頼の始め / 終わりを決める）
#[derive(Default)]
struct Holders {
    count: usize,
}

impl Holders {
    /// 1 つ増やす。`true` = 0 → 1（OS への依頼を始める番）
    fn acquire(&mut self) -> bool {
        self.count += 1;
        self.count == 1
    }

    /// 1 つ減らす。`true` = 1 → 0（OS への依頼を終える番）。0 から減らしても何もしない
    fn release(&mut self) -> bool {
        if self.count == 0 {
            return false;
        }
        self.count -= 1;
        self.count == 0
    }
}

#[derive(Default)]
struct State {
    holders: Holders,
    token: Option<sys::Token>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};

    // 宣言の形は crate 内の他の objc 呼び出し（`platform::pdf::macos` 等）と揃える
    // （食い違うと clashing_extern_declarations）。引数の形は呼ぶ側で関数型へ写して決める
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> *const c_void;
        fn sel_registerName(name: *const u8) -> *const c_void;
        fn objc_msgSend(receiver: *const c_void, sel: *const c_void, ...) -> *const c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// `NSActivityUserInitiatedAllowingIdleSystemSleep`
    /// （= `NSActivityUserInitiated` 0x00FF_FFFF から `NSActivityIdleSystemSleepDisabled` を外す）。
    /// 利用者が始めた処理として App Nap を止め、システムのアイドルスリープは妨げない
    pub(super) const OPTIONS: u64 = 0x00FF_FFFF & !(1 << 20);

    /// `beginActivityWithOptions:reason:` が返した印（retain 済み。`end` で手放す）
    pub(super) struct Token(pub(super) usize);

    type Send0 = unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void;
    type SendCStr =
        unsafe extern "C" fn(*const c_void, *const c_void, *const c_char) -> *const c_void;
    type SendBegin =
        unsafe extern "C" fn(*const c_void, *const c_void, u64, *const c_void) -> *const c_void;
    type SendObj = unsafe extern "C" fn(*const c_void, *const c_void, *const c_void);

    /// `objc_msgSend` を引数の形どおりの関数として呼ぶ（可変長のまま呼ぶと arm64 で引数が崩れる）
    unsafe fn msg<F: Copy>() -> F {
        debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
        let raw = objc_msgSend as *const c_void;
        std::mem::transmute_copy(&raw)
    }

    unsafe fn sel(name: &std::ffi::CStr) -> *const c_void {
        sel_registerName(name.as_ptr().cast())
    }

    unsafe fn process_info() -> *const c_void {
        let class = objc_getClass(c"NSProcessInfo".as_ptr().cast());
        if class.is_null() {
            return std::ptr::null();
        }
        msg::<Send0>()(class, sel(c"processInfo"))
    }

    unsafe fn begin_in_pool() -> Option<Token> {
        let info = process_info();
        let string = objc_getClass(c"NSString".as_ptr().cast());
        if info.is_null() || string.is_null() {
            return None;
        }
        let reason = msg::<SendCStr>()(
            string,
            sel(c"stringWithUTF8String:"),
            c"tako: background work the user is waiting for".as_ptr(),
        );
        let token = msg::<SendBegin>()(
            info,
            sel(c"beginActivityWithOptions:reason:"),
            OPTIONS,
            reason,
        );
        if token.is_null() {
            return None;
        }
        // 印は autorelease で返る。pool を抜けても `end` まで生かす
        Some(Token(msg::<Send0>()(token, sel(c"retain")) as usize))
    }

    pub(super) fn begin() -> Option<Token> {
        // background のスレッドから呼ぶので、autorelease された一時物はここで片付ける
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let token = begin_in_pool();
            objc_autoreleasePoolPop(pool);
            token
        }
    }

    pub(super) fn end(token: Token) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let token = token.0 as *const c_void;
            let info = process_info();
            if !info.is_null() {
                msg::<SendObj>()(info, sel(c"endActivity:"), token);
            }
            msg::<Send0>()(token, sel(c"release"));
            objc_autoreleasePoolPop(pool);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    /// 印は作らない（依頼する先が無い）
    pub(super) enum Token {}

    pub(super) fn begin() -> Option<Token> {
        None
    }

    pub(super) fn end(token: Token) {
        match token {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 重なった処理でもosへの依頼は最初に始め最後に終える() {
        let mut holders = Holders::default();
        assert!(holders.acquire(), "0 → 1 で始める");
        assert!(!holders.acquire(), "2 つ目は始めない");
        assert!(!holders.acquire(), "3 つ目も始めない");
        assert!(!holders.release(), "3 → 2 では終えない");
        assert!(!holders.release(), "2 → 1 では終えない");
        assert!(holders.release(), "1 → 0 で終える");
        assert!(!holders.release(), "0 から減らしても終える番は来ない");
        assert!(holders.acquire(), "終えた後の次は始め直す");
    }

    /// 実物の `NSProcessInfo` で始めて終えられる（印が取れる・二重に終えない）
    #[cfg(target_os = "macos")]
    #[test]
    fn macosではactivityの印が取れて終えられる() {
        assert_eq!(
            sys::OPTIONS,
            0x00EF_FFFF,
            "UserInitiatedAllowingIdleSystemSleep"
        );
        let token = sys::begin().expect("NSProcessInfo が印を返す");
        assert_ne!(token.0, 0);
        sys::end(token);
        // 握って落とす口も通す（他のテストと並走しても数は 0 へ戻る形だけを見る）
        let first = UserWork::begin();
        let second = UserWork::begin_load();
        drop(first);
        drop(second);
    }
}

//! Web ビュー ホストの OS 差（抽象境界 B11。#2429 / #2430）。
//!
//! Web ビュー（wry）は gpui の窓の**子**としてネイティブのビュー / 窓を置く。「子を gpui の描画の上に
//! 見せる」と「キー入力の宛先を読む・gpui へ戻す」の実体が OS ごとに違うので、その Windows の腕を
//! ここへ置く。macOS の腕（AppKit の first responder）は `webview.rs` の `key_monitor` に残っている
//! （#1538 の K521 で `platform/` へ寄せる予定の塊。この Issue では動かさない）。
//! `.agent/plans/2026-07-windows-port-architecture.md` 原則 1 のとおり、
//! **`#[cfg(target_os)]` / `cfg!` を書いてよいのはこのファイルの実装選択だけ**で、呼び手
//! （`main.rs` の起動の最初・`webview.rs` の宛先の口）は OS を名指さない。`webview.rs` は窓の
//! ハンドルの種類（Win32 か）で振り分ける（`WindowHandleBox::attached` と同じ形）。
//!
//! ## プラットフォームごとの実体
//!
//! | | macOS | Windows | その他 |
//! |---|---|---|---|
//! | 描画の合成（[`prepare_compositor`]。#2429） | 触らない（WKWebView は描画ビューの子ビュー = 合成の上に乗る） | gpui を作る前に `GPUI_DISABLE_DIRECT_COMPOSITION=1` を立て、作った後に外す | 触らない |
//! | 宛先が Web ビュー側か（[`win32_child_holds_key_focus`]。#2430） | 呼ばれない（`key_monitor`） | gpui の窓のスレッドの宛先（`GetGUIThreadInfo`）が窓の子孫（`IsChild`） | 偽 |
//! | 宛先を gpui へ戻す（[`win32_return_key_focus`]） | 呼ばれない | `SetFocus(gpui の窓)`（wry の `focus_parent` と同じ） | 偽 |
//! | 宛先の素性（[`win32_key_focus_owner`]。診断・セルフテスト） | 呼ばれない | `クラス名@HWND` | 空 |
//!
//! ## なぜ DirectComposition を切るか（#2429）
//!
//! gpui_windows（rev `cafbf4b`）は既定で DirectComposition の合成を使い、トップレベルに
//! `WS_EX_NOREDIRECTIONBITMAP` を立てて（`window.rs:472`）、合成のターゲットを
//! `CreateTargetForHwnd(hwnd, true)`（`directx_renderer.rs:901`）で作る。第 2 引数 `topmost = TRUE` は
//! 「合成の木を**子窓より上**に重ねる」なので、WebView2 の子 HWND は gpui の swap chain の下に入り、
//! tako がペインの本文を不透明に塗る矩形に覆われる（`state: ready` なのに暗い）。gpui の
//! `GPUI_DISABLE_DIRECT_COMPOSITION`（Zed が RDP / 一部の GPU 向けに持つ正規の逃げ道）を立てると
//! swap chain が HWND 直結（`CreateSwapChainForHwnd`）になり、子窓は DWM が上に重ねる。
//! gpui がこの env を読むのは platform を作る 1 回だけ（`platform.rs:167` → 窓ごとに同じ値を渡す）
//! なので、立てるのは `application()` の前、外すのはその後でよい。外すのはペインの子
//! （シェル・別の gpui アプリ）へ tako の内部の選択を渡さないため。
//!
//! 採らなかった手: (b) Web ビューの矩形を gpui のすべての層で α = 0 に抜く（`topmost = TRUE` のままでは
//! 描くすべての要素が守る必要があり手が大きい）、(c) gpui の側で `topmost = FALSE` にする（fork か
//! 上流への変更が要る）、(d) WebView2 を所有付きのポップアップ窓に載せる（窓の移動・Z 順・活性化を
//! tako が追う必要がある）。比べた数字と理由は `.agent/architecture.md`「Windows の合成と子窓」節。

/// gpui_windows が platform を作るときに読む env（`directx_renderer.rs:26`）。`1` / `true` で切る
pub(crate) const GPUI_DISABLE_DIRECT_COMPOSITION: &str = "GPUI_DISABLE_DIRECT_COMPOSITION";

/// #2429 の修正前へ戻す A/B の口（空でなければ DirectComposition を切らない）
pub(crate) const LEGACY_2429_ENV: &str = "TAKO_2429_LEGACY";

/// 起動の最初に決めた描画の合成（#2429）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompositorChoice {
    /// 触らない（Windows 以外）
    NotApplicable,
    /// tako が `GPUI_DISABLE_DIRECT_COMPOSITION=1` を立てた（gpui を作った後に外す）
    Disabled,
    /// 利用者が外から渡した値に従う（上書きしない・外さない）
    UserSet(String),
    /// A/B（`TAKO_2429_LEGACY=1`）: 修正前と同じく DirectComposition のまま
    Legacy,
}

/// [`prepare_compositor`] の判断（純関数）。`user_value` は外から渡された
/// `GPUI_DISABLE_DIRECT_COMPOSITION`（空・空白だけは渡していないのと同じ扱い）
pub(crate) fn compositor_choice(
    windows: bool,
    legacy: bool,
    user_value: Option<&str>,
) -> CompositorChoice {
    if !windows {
        return CompositorChoice::NotApplicable;
    }
    if let Some(value) = user_value.filter(|v| !v.trim().is_empty()) {
        return CompositorChoice::UserSet(value.to_string());
    }
    if legacy {
        return CompositorChoice::Legacy;
    }
    CompositorChoice::Disabled
}

/// gpui を作る前に描画の合成を決める（#2429）。**`main` の最初の方・スレッドを起こす前**に 1 回呼ぶ
/// （env の書き換えは別スレッドの読みと競る）。Windows で切ると決めたらここで env を立てる
pub(crate) fn prepare_compositor() -> CompositorChoice {
    let legacy = std::env::var_os(LEGACY_2429_ENV).is_some_and(|v| !v.is_empty());
    let user = std::env::var(GPUI_DISABLE_DIRECT_COMPOSITION).ok();
    let choice = compositor_choice(cfg!(windows), legacy, user.as_deref());
    if choice == CompositorChoice::Disabled {
        std::env::set_var(GPUI_DISABLE_DIRECT_COMPOSITION, "1");
    }
    choice
}

impl CompositorChoice {
    /// gpui の platform を作った後（`application()` の直後）に呼ぶ。tako が立てた env だけを外し、
    /// ペインの子（シェル・別の gpui アプリ）へ渡さない。Windows の env の書き換えは OS が直列にする
    /// ので、platform が起こしたスレッドの後でも読みと競らない（立てるのは Windows のときだけ）
    pub(crate) fn after_platform_created(&self) {
        if *self == CompositorChoice::Disabled {
            std::env::remove_var(GPUI_DISABLE_DIRECT_COMPOSITION);
        }
    }

    /// persist.log の 1 行（Windows 以外は None）。値は利用者の env の写しだけでトークン等は含まない
    pub(crate) fn log_line(&self) -> Option<String> {
        match self {
            CompositorChoice::NotApplicable => None,
            CompositorChoice::Disabled => Some(format!(
                "描画の合成（#2429）: DirectComposition を切った（{GPUI_DISABLE_DIRECT_COMPOSITION}=1 を \
                 gpui の初期化の間だけ立てた。Web ビューの子窓を gpui の描画の上に見せるため）"
            )),
            CompositorChoice::UserSet(value) => Some(format!(
                "描画の合成（#2429）: 外から渡された {GPUI_DISABLE_DIRECT_COMPOSITION}={value} に従う（tako は触らない）"
            )),
            CompositorChoice::Legacy => Some(format!(
                "描画の合成（#2429）: {LEGACY_2429_ENV} により DirectComposition のまま（修正前。\
                 Web ビューの中身は gpui の描画の下に隠れる）"
            )),
        }
    }
}

// --- キー入力の宛先（#2430） --------------------------------------------------------

/// キー入力の宛先が `parent`（gpui の窓の HWND）の子孫 = Web ビュー側か（#2430）。
///
/// WebView2 の子窓（別プロセスの `Chrome_WidgetWin_1` ほか）は親子関係で gpui の窓と入力の状態を
/// 共有するので、gpui の窓のスレッドの宛先（`GetGUIThreadInfo` の `hwndFocus`）に現れる。
/// **状態ではなく OS の実体を読む**（#503 / #1481 と同じ。印は降ろし忘れる）
pub(crate) fn win32_child_holds_key_focus(parent: isize) -> bool {
    #[cfg(windows)]
    {
        let focus = win32::focus_of(parent);
        focus != 0 && focus != parent && win32::is_child(parent, focus)
    }
    #[cfg(not(windows))]
    {
        let _ = parent;
        false
    }
}

/// キー入力の宛先を `parent`（gpui の窓）へ戻す。戻せたら true（#2430）。
///
/// wry 0.57.1 の Windows の `focus_parent` と同じ `SetFocus(親)`（`webview2/mod.rs:1678`）。
/// 実機で「タブを切り替えて戻す」（= `sync_frame(None)` の `focus_parent`）で宛先が戻ることを確かめた
/// 経路をそのまま使う。gpui の窓を持つ UI スレッドから呼ぶ（`SetFocus` は呼び手のスレッドの入力の状態を
/// 動かす）。gpui_windows は `WM_SETFOCUS` / `WM_KILLFOCUS` を扱わないので、活性の状態は動かない
pub(crate) fn win32_return_key_focus(parent: isize) -> bool {
    #[cfg(windows)]
    {
        win32::set_focus(parent);
        win32::focus_of(parent) == parent
    }
    #[cfg(not(windows))]
    {
        let _ = parent;
        false
    }
}

/// キー入力の宛先の素性（`クラス名@HWND`。診断・セルフテスト項目 71 の #1481 群）。
/// gpui の窓 = `Zed::Window@…` / WebView2 = `Chrome_…@…`。HWND まで出すのは 2 枚のどちらかを言い分けるため
pub(crate) fn win32_key_focus_owner(parent: isize) -> String {
    #[cfg(windows)]
    {
        let focus = win32::focus_of(parent);
        format!("{}@{:#x}", win32::class_name(focus), focus)
    }
    #[cfg(not(windows))]
    {
        let _ = parent;
        String::new()
    }
}

#[cfg(windows)]
mod win32 {
    /// `RECT`（winuser.h）。OS が書く領域で、tako は読まない
    #[repr(C)]
    #[allow(dead_code)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    /// `GUITHREADINFO`（winuser.h）。`cbSize` を入れて渡す。OS が全体を書き、tako が読むのは
    /// `hwnd_focus` だけ（並びと大きさを OS の定義に合わせるために残りも持つ）
    #[repr(C)]
    #[allow(dead_code)]
    struct GuiThreadInfo {
        cb_size: u32,
        flags: u32,
        hwnd_active: isize,
        hwnd_focus: isize,
        hwnd_capture: isize,
        hwnd_menu_owner: isize,
        hwnd_move_size: isize,
        hwnd_caret: isize,
        rc_caret: Rect,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn GetGUIThreadInfo(thread: u32, info: *mut GuiThreadInfo) -> i32;
        fn IsChild(parent: isize, child: isize) -> i32;
        fn SetFocus(hwnd: isize) -> isize;
        fn GetClassNameW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    }

    /// `parent` を持つスレッドのキー入力の宛先（読めなければ 0）
    pub(super) fn focus_of(parent: isize) -> isize {
        // SAFETY: GetWindowThreadProcessId は窓でない値に 0 を返すだけ。GetGUIThreadInfo へは
        // cbSize を入れた自前の領域を渡し、失敗なら 0 を返す
        unsafe {
            let thread = GetWindowThreadProcessId(parent, std::ptr::null_mut());
            if thread == 0 {
                return 0;
            }
            let mut info = GuiThreadInfo {
                cb_size: std::mem::size_of::<GuiThreadInfo>() as u32,
                flags: 0,
                hwnd_active: 0,
                hwnd_focus: 0,
                hwnd_capture: 0,
                hwnd_menu_owner: 0,
                hwnd_move_size: 0,
                hwnd_caret: 0,
                rc_caret: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
            };
            if GetGUIThreadInfo(thread, &mut info) == 0 {
                return 0;
            }
            info.hwnd_focus
        }
    }

    /// `child` が `parent` の子孫か（別プロセスの子窓も親の連なりで辿る）
    pub(super) fn is_child(parent: isize, child: isize) -> bool {
        // SAFETY: IsChild は任意の値を受け取り、窓でなければ 0 を返すだけ
        unsafe { IsChild(parent, child) != 0 }
    }

    /// `hwnd` を呼び手のスレッドのキー入力の宛先にする
    pub(super) fn set_focus(hwnd: isize) {
        // SAFETY: SetFocus は窓でない値・別の入力の状態の窓には NULL を返すだけ
        unsafe {
            SetFocus(hwnd);
        }
    }

    /// 窓のクラス名（読めなければ空）
    pub(super) fn class_name(hwnd: isize) -> String {
        if hwnd == 0 {
            return String::new();
        }
        let mut buf = [0u16; 256];
        // SAFETY: 長さを渡した自前の領域へ書かせる。戻り値は書いた文字数（終端を除く）
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows以外は合成に触らない() {
        assert_eq!(
            compositor_choice(false, false, None),
            CompositorChoice::NotApplicable
        );
        assert_eq!(
            compositor_choice(false, true, Some("0")),
            CompositorChoice::NotApplicable
        );
        assert_eq!(CompositorChoice::NotApplicable.log_line(), None);
    }

    #[test]
    fn windowsの既定はdirect_compositionを切る() {
        assert_eq!(
            compositor_choice(true, false, None),
            CompositorChoice::Disabled
        );
        let line = CompositorChoice::Disabled.log_line().unwrap();
        assert!(line.contains("#2429") && line.contains(GPUI_DISABLE_DIRECT_COMPOSITION));
    }

    #[test]
    fn 利用者が外から渡した値は上書きしない() {
        // 切らないための明示（0）も、切る明示（1）も、そのまま従う。A/B より利用者の値が先
        for (value, legacy) in [("0", false), ("1", false), ("true", true), ("false", true)] {
            assert_eq!(
                compositor_choice(true, legacy, Some(value)),
                CompositorChoice::UserSet(value.to_string()),
                "value={value} legacy={legacy}"
            );
        }
        // 空・空白だけは渡していないのと同じ（gpui も切らない扱いの値。tako の既定を当てる）
        for value in ["", "  "] {
            assert_eq!(
                compositor_choice(true, false, Some(value)),
                CompositorChoice::Disabled,
                "value={value:?}"
            );
        }
    }

    #[test]
    fn ab_の口で修正前へ戻る() {
        assert_eq!(
            compositor_choice(true, true, None),
            CompositorChoice::Legacy
        );
        assert!(CompositorChoice::Legacy
            .log_line()
            .unwrap()
            .contains(LEGACY_2429_ENV));
    }

    /// macOS / その他では宛先の Win32 の腕は何も読まない・何も動かさない（呼ばれても偽 / 空）
    #[cfg(not(windows))]
    #[test]
    fn windows以外の宛先の腕は空() {
        assert!(!win32_child_holds_key_focus(0x1234));
        assert!(!win32_return_key_focus(0x1234));
        assert_eq!(win32_key_focus_owner(0x1234), "");
    }
}

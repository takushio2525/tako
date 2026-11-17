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
//! | ページが宛先のあいだの tako のショートカット（[`watch_app_shortcuts`]。#326 / #2948） | `webview.rs` の `key_monitor`（NSEvent の local monitor。キーの名前は [`macos_ansi_letter`]） | WebView2 の `AcceleratorKeyPressed` で先取りして `Handled` | 付けない |
//!
//! ## ページが宛先のあいだの tako のショートカット（#2948）
//!
//! WebView2 の子窓は別プロセスなので、ページが宛先のあいだの打鍵は gpui の窓プロシージャを
//! 通らない（Ctrl+Shift+T がページに飲まれる）。wry 0.57.1 は `AcceleratorKeyPressed` を公開して
//! いないので、`WebViewExtWindows::controller()` から直に付ける。コールバックの中では
//! **打鍵をキー名へ直して判定し、tako のものだけ `Handled` にして前景タスクへ渡すだけ**にする。
//! 宛先を gpui へ戻す（#1481 の 1 実装）・同じ打鍵を gpui のキーバインドへ配る
//! （`Window::dispatch_keystroke`）のはコールバックを抜けた後の gpui の前景タスク（`main.rs`）で、
//! 理由は 2 つ: ① WebView2 はコールバックの答え（`Handled`）を待って打鍵を止めているので、
//! その中で `SetFocus` を呼ぶと別プロセスの窓への `WM_KILLFOCUS` の同期送信になり待ち合う恐れがある、
//! ② 窓へ `WM_KEYDOWN` を投げ直す形は、gpui が届いた時点の修飾キーの状態（`GetKeyState`）を
//! 読み直すので、その前に指を離すと修飾の無い文字としてターミナルへ流れる。
//! 判定（どの打鍵を先取りするか）は macOS と同じ 1 実装 `keybindings::webview_key_route`
//! （表は `tako_core::platform::keys::WEBVIEW_PAGE_KEYS`）。キー名は gpui_windows（rev `cafbf4b`）の
//! `parse_immutable` / `get_keystroke_key` と同じ規則・同じ Win32 の呼び出しで作る
//! （[`win32_named_key`] / [`win32_shifts_to_symbol`]）ので、判定した打鍵と gpui が実キーから作る
//! 打鍵が一致する。
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

// --- ページが宛先のあいだの tako のショートカット（#326 / #2948） ------------------------------

/// Web ビューのページがキー入力の宛先のあいだ、tako の割り当てに当たる打鍵を先取りする口を
/// 付ける（#2948。Windows = WebView2 の `AcceleratorKeyPressed`）。
///
/// `steal` は打鍵（gpui の `Keystroke`）を受けて「tako が先取りするか」を答え、先取りするなら
/// tako への配送まで引き受ける（呼び手 = `webview.rs`。判定は `keybindings::webview_key_route` の
/// 1 実装）。true を返した打鍵だけページへ渡さない。付けたら true（macOS は `key_monitor` が
/// 受け持つので付けない = false）
pub(crate) fn watch_app_shortcuts(view: &wry::WebView, steal: fn(gpui::Keystroke) -> bool) -> bool {
    #[cfg(windows)]
    {
        win32::watch_accelerators(view, steal)
    }
    #[cfg(not(windows))]
    {
        let _ = (view, steal);
        false
    }
}

/// NSEvent の `modifierFlags` の各修飾（AppKit の `NSEventModifierFlag*`）
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const NS_SHIFT_KEY_MASK: u64 = 1 << 17;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const NS_CONTROL_KEY_MASK: u64 = 1 << 18;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const NS_ALTERNATE_KEY_MASK: u64 = 1 << 19;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const NS_COMMAND_KEY_MASK: u64 = 1 << 20;

/// macOS の NSEvent の local monitor（`webview.rs` の `key_monitor`。#326）が、打鍵
/// （virtual key code と `modifierFlags`）をページへ渡すか。判定は Windows の
/// `AcceleratorKeyPressed` と同じ 1 実装 `tako_core::platform::keys::webview_key_route`。
/// macOS の規則は tako の割り当てかを問わない（⌘ の打鍵は PTY へ届かない）ので `app_bound` は偽で渡す
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn macos_key_routes_to_page(key_code: u16, flags: u64) -> bool {
    use tako_core::platform::keys::{webview_key_route, WebviewKeyMods, WebviewKeyRoute};
    use tako_core::platform::support::Platform;
    let mods = WebviewKeyMods {
        platform_key: flags & NS_COMMAND_KEY_MASK != 0,
        control: flags & NS_CONTROL_KEY_MASK != 0,
        alt: flags & NS_ALTERNATE_KEY_MASK != 0,
        shift: flags & NS_SHIFT_KEY_MASK != 0,
    };
    let key = macos_ansi_letter(key_code).unwrap_or("");
    webview_key_route(Platform::MacOs, key, mods, false) == WebviewKeyRoute::Page
}

/// macOS の ANSI 配列の英字の virtual key code（Carbon の `kVK_ANSI_*`）→ キー名。
/// #326 の表（`WEBVIEW_PASSTHROUGH_KEYS`）と同じく**配列によらない物理位置**で引く
/// （Dvorak でも ⌘ + 物理の C の位置は `c`）。英字以外は None
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn macos_ansi_letter(code: u16) -> Option<&'static str> {
    Some(match code {
        0x00 => "a",
        0x01 => "s",
        0x02 => "d",
        0x03 => "f",
        0x04 => "h",
        0x05 => "g",
        0x06 => "z",
        0x07 => "x",
        0x08 => "c",
        0x09 => "v",
        0x0B => "b",
        0x0C => "q",
        0x0D => "w",
        0x0E => "e",
        0x0F => "r",
        0x10 => "y",
        0x11 => "t",
        0x1F => "o",
        0x20 => "u",
        0x22 => "i",
        0x23 => "p",
        0x25 => "l",
        0x26 => "j",
        0x28 => "k",
        0x2D => "n",
        0x2E => "m",
        _ => return None,
    })
}

/// 文字ではなく名前で呼ぶ Win32 の virtual key code → gpui のキー名
/// （gpui_windows rev `cafbf4b` の `events.rs` の `parse_immutable` の写し）
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn win32_named_key(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x20 => "space",
        0x08 => "backspace",
        0x0D => "enter",
        0x09 => "tab",
        0x26 => "up",
        0x28 => "down",
        0x27 => "right",
        0x25 => "left",
        0x24 => "home",
        0x23 => "end",
        0x21 => "pageup",
        0x22 => "pagedown",
        0xA6 => "back",
        0xA7 => "forward",
        0x1B => "escape",
        0x2D => "insert",
        0x2E => "delete",
        0x5D => "menu",
        0x70 => "f1",
        0x71 => "f2",
        0x72 => "f3",
        0x73 => "f4",
        0x74 => "f5",
        0x75 => "f6",
        0x76 => "f7",
        0x77 => "f8",
        0x78 => "f9",
        0x79 => "f10",
        0x7A => "f11",
        0x7B => "f12",
        0x7C => "f13",
        0x7D => "f14",
        0x7E => "f15",
        0x7F => "f16",
        0x80 => "f17",
        0x81 => "f18",
        0x82 => "f19",
        0x83 => "f20",
        0x84 => "f21",
        0x85 => "f22",
        0x86 => "f23",
        0x87 => "f24",
        _ => return None,
    })
}

/// ⇧ と組むと gpui が「⇧ を足した字面 + ⇧ なし」へ正規化するキー（記号・数字の列）
/// （gpui_windows rev `cafbf4b` の `keyboard.rs` の `need_to_convert_to_shifted_key` の写し）
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn win32_shifts_to_symbol(vk: u32) -> bool {
    matches!(
        vk,
        0xC0 // VK_OEM_3
            | 0xBD // VK_OEM_MINUS
            | 0xBB // VK_OEM_PLUS
            | 0xDB // VK_OEM_4
            | 0xDC // VK_OEM_5
            | 0xDD // VK_OEM_6
            | 0xBA // VK_OEM_1
            | 0xDE // VK_OEM_7
            | 0xBC // VK_OEM_COMMA
            | 0xBE // VK_OEM_PERIOD
            | 0xBF // VK_OEM_2
            | 0xE2 // VK_OEM_102
            | 0xDF // VK_OEM_8
            | 0xC1 // VK_ABNT_C1
            | 0x30..=0x39 // VK_0..VK_9
    )
}

/// 修飾キーそのもの・打鍵として数えないキー（gpui_windows の `handle_key_event` が
/// `ModifiersChanged` にする / 捨てるもの: Shift / Ctrl / Alt / Win / CapsLock / VK_PACKET）
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn win32_is_modifier_key(vk: u32) -> bool {
    // VK_SHIFT / VK_CONTROL / VK_MENU・VK_LSHIFT..VK_RMENU・VK_LWIN / VK_RWIN・VK_CAPITAL・VK_PACKET
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C | 0x14 | 0xE7)
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
        fn GetKeyState(vk: i32) -> i16;
        fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
        fn ToUnicode(
            vk: u32,
            scan_code: u32,
            key_state: *const u8,
            buf: *mut u16,
            buf_len: i32,
            flags: u32,
        ) -> i32;
    }

    // --- ページが宛先のあいだの tako のショートカット（#2948） ---

    /// `AcceleratorKeyPressed` を付ける（[`super::watch_app_shortcuts`] の Windows の腕）
    pub(super) fn watch_accelerators(
        view: &wry::WebView,
        steal: fn(gpui::Keystroke) -> bool,
    ) -> bool {
        use webview2_com::AcceleratorKeyPressedEventHandler;
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        };
        use wry::WebViewExtWindows;

        let controller = view.controller();
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            let mut vk = 0u32;
            let mut lparam = 0i32;
            // SAFETY: WebView2 が呼び出しの間だけ有効な args から、自前の領域へ書かせる
            unsafe {
                args.KeyEventKind(&mut kind)?;
                args.VirtualKey(&mut vk)?;
                args.KeyEventLParam(&mut lparam)?;
            }
            // 押した瞬間だけ見る（先取りした打鍵を離すときは宛先が gpui へ戻った後なので gpui に届く）
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
            {
                return Ok(());
            }
            let Some(keystroke) = keystroke(vk, lparam) else {
                return Ok(());
            };
            if steal(keystroke) {
                // SAFETY: 同上（ページ・ブラウザの既定の動作へ渡さない）
                unsafe { args.SetHandled(true)? };
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: controller は wry が持つ生きた WebView2 の controller。handler の参照は COM が持ち、
        // controller が閉じるときに一緒に放す（Web ビューと同じ寿命なので外さない）
        unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }.is_ok()
    }

    /// 打鍵（WebView2 の virtual key と lParam）を gpui の `Keystroke` にする。gpui_windows
    /// （rev `cafbf4b`）が実キーの `WM_KEYDOWN` から作るのと同じ規則・同じ Win32 の呼び出し
    /// （`handle_key_event` → `parse_normal_key` → `get_keystroke_key`）。修飾キーそのものは None。
    /// `key_char` は持たない（先取りするのは割り当てに当たる打鍵だけで、文字入力には使わない）
    fn keystroke(vk: u32, lparam: i32) -> Option<gpui::Keystroke> {
        if super::win32_is_modifier_key(vk) {
            return None;
        }
        let mut modifiers = gpui::Modifiers {
            control: key_down(0x11),                    // VK_CONTROL
            alt: key_down(0x12),                        // VK_MENU
            shift: key_down(0x10),                      // VK_SHIFT
            platform: key_down(0x5B) || key_down(0x5C), // VK_LWIN / VK_RWIN
            function: false,
        };
        let scan_code = ((lparam as u32) >> 16) & 0xFF;
        let key = match super::win32_named_key(vk) {
            Some(name) => name.to_string(),
            None if modifiers.shift && super::win32_shifts_to_symbol(vk) => {
                let shifted = shifted_key(vk, scan_code)?;
                modifiers.shift = false;
                shifted
            }
            None => key_from_vkey(vk)?,
        };
        Some(gpui::Keystroke {
            modifiers,
            key,
            key_char: None,
        })
    }

    /// いま押されているか（gpui_windows の `is_virtual_key_pressed` と同じ `GetKeyState`。
    /// WebView2 の子窓は親子関係で gpui の窓と入力の状態を共有する = #2430 と同じ前提）
    fn key_down(vk: i32) -> bool {
        // SAFETY: GetKeyState は任意の値を受け取り、状態を返すだけ
        unsafe { GetKeyState(vk) < 0 }
    }

    /// ⇧ だけを押した状態の字面（gpui_windows の `get_shifted_key` = `generate_key_char` の写し）
    fn shifted_key(vk: u32, scan_code: u32) -> Option<String> {
        let mut state = [0u8; 256];
        state[0x10] = 0x80; // VK_SHIFT
        let mut buf = [0u16; 8];
        // SAFETY: 長さを渡した自前の領域へ書かせる。0x5 = キーボードの状態を変えない（gpui と同じ）
        let len = unsafe {
            ToUnicode(
                vk,
                scan_code,
                state.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as i32,
                0x5,
            )
        };
        match len {
            len if len > 0 => String::from_utf16(&buf[..len as usize])
                .ok()
                .filter(|s| s.chars().next().is_some_and(|c| !c.is_control())),
            len if len < 0 => String::from_utf16(&buf[..(-len) as usize]).ok(),
            _ => None,
        }
    }

    /// 修飾なしの字面（gpui_windows の `get_key_from_vkey` の写し。英字は小文字）
    fn key_from_vkey(vk: u32) -> Option<String> {
        // SAFETY: MapVirtualKeyW は任意の値を受け取り、対応が無ければ 0 を返すだけ。2 = MAPVK_VK_TO_CHAR
        let data = unsafe { MapVirtualKeyW(vk, 2) };
        if data == 0 {
            return None;
        }
        let c = char::from_u32(data & 0xFFFF)?;
        Some(c.to_ascii_lowercase().to_string())
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

    /// #2948 の前の macOS の表（#326 の `WEBVIEW_PASSTHROUGH_KEYS` の写し。比較の基準として凍結）
    const LEGACY_326_PASSTHROUGH: [u16; 10] =
        [0x00, 0x08, 0x03, 0x25, 0x23, 0x0F, 0x01, 0x09, 0x07, 0x06];

    /// #2948 の前の macOS の判定（`monitor_invoke`: ⌘ なし → ページ、表のキー → ページ、
    /// それ以外 → 宛先を tako へ戻す）
    fn legacy_326_routes_to_page(key_code: u16, flags: u64) -> bool {
        flags & NS_COMMAND_KEY_MASK == 0 || LEGACY_326_PASSTHROUGH.contains(&key_code)
    }

    /// 受け入れ条件 2: macOS の Web ビューの振る舞いは #2948 の前後で同じ。virtual key code の
    /// 全域 × 修飾の全組み合わせ（CapsLock・テンキー・fn の印も混ぜる）で 1 つも違わない
    #[test]
    fn macosの判定は326の表と全キーコードで一致する() {
        let extra = [0u64, 1 << 16, 1 << 21, 1 << 23];
        let mut checked = 0usize;
        for code in 0u16..=0x7F {
            for bits in 0u64..16 {
                let mut flags = 0u64;
                for (i, mask) in [
                    NS_SHIFT_KEY_MASK,
                    NS_CONTROL_KEY_MASK,
                    NS_ALTERNATE_KEY_MASK,
                    NS_COMMAND_KEY_MASK,
                ]
                .into_iter()
                .enumerate()
                {
                    if bits & (1 << i) != 0 {
                        flags |= mask;
                    }
                }
                for e in extra {
                    let f = flags | e;
                    assert_eq!(
                        macos_key_routes_to_page(code, f),
                        legacy_326_routes_to_page(code, f),
                        "key_code={code:#04x} flags={f:#x}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 128 * 16 * 4);
    }

    /// ANSI の英字の位置は 26 文字ちょうどで重ならない（#326 の 10 文字もここから引く）
    #[test]
    fn macosの英字の位置は26文字で重ならない() {
        let mut letters: Vec<&str> = (0u16..=0x7F).filter_map(macos_ansi_letter).collect();
        assert_eq!(letters.len(), 26);
        letters.sort();
        letters.dedup();
        assert_eq!(letters.len(), 26);
        for (code, letter) in LEGACY_326_PASSTHROUGH
            .into_iter()
            .zip(["a", "c", "f", "l", "p", "r", "s", "v", "x", "z"])
        {
            assert_eq!(macos_ansi_letter(code), Some(letter), "{code:#04x}");
        }
    }

    /// Win32 の写し（gpui_windows の `parse_immutable` / `need_to_convert_to_shifted_key`）:
    /// tako の Windows の割り当てに出てくるキーの名前が gpui と同じに引ける
    #[test]
    fn win32のキー名はgpuiと同じ規則() {
        for (vk, name) in [
            (0x09, "tab"),
            (0x25, "left"),
            (0x27, "right"),
            (0x26, "up"),
            (0x28, "down"),
            (0x2D, "insert"),
            (0x7A, "f11"),
            (0x1B, "escape"),
            (0x08, "backspace"),
        ] {
            assert_eq!(win32_named_key(vk), Some(name), "{vk:#04x}");
        }
        // 英字・数字・記号は名前で呼ばない（字面を MapVirtualKeyW / ToUnicode で引く）
        for vk in (0x41..=0x5A).chain(0x30..=0x39).chain([0xBB, 0xBD, 0xBC]) {
            assert_eq!(win32_named_key(vk), None, "{vk:#04x}");
        }
        // ⇧ と組むと字面へ正規化されるのは数字と記号の列だけ（英字は ⇧ を残す = ctrl-shift-t）
        for vk in (0x30..=0x39).chain([0xBB, 0xBD, 0xBC, 0xC0, 0xDB, 0xDD]) {
            assert!(win32_shifts_to_symbol(vk), "{vk:#04x}");
        }
        for vk in 0x41..=0x5A {
            assert!(!win32_shifts_to_symbol(vk), "{vk:#04x}");
        }
        // 修飾キーそのものは打鍵として数えない
        for vk in [0x10, 0x11, 0x12, 0xA0, 0xA3, 0xA5, 0x5B, 0x5C, 0x14, 0xE7] {
            assert!(win32_is_modifier_key(vk), "{vk:#04x}");
        }
        for vk in [0x41, 0x54, 0x09, 0x2D, 0x7A] {
            assert!(!win32_is_modifier_key(vk), "{vk:#04x}");
        }
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

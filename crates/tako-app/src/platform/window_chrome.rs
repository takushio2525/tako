//! 窓の枠まわりの OS 差（抽象境界 B36。#2858）。
//!
//! 窓の実体（GPUI の `Window` と OS の窓のハンドル）を触る腕と、枠の寸法・表記の定数を置く。
//! `.agent/plans/2026-07-windows-port-architecture.md` 原則 1 のとおり、
//! **`#[cfg(target_os)]` / `cfg!` を書いてよいのはこのファイルの実装選択だけ**で、
//! 呼び出し側（`main.rs` の窓の操作とメニュー、`menu_bar` / `tab_bar` の描画）は
//! ここの関数・定数を名指す 1 経路を持つ。
//!
//! ## プラットフォームごとの実体
//!
//! | | macOS | Windows | その他 |
//! |---|---|---|---|
//! | 位置・寸法の適用（[`set_window_frame`]。#1442） | NSWindow `setFrame:display:`（CoreGraphics のグローバル矩形を挟んで Cocoa の座標へ） | user32 `SetWindowPos`（論理 → 物理ピクセル） | 何もしない |
//! | 最大化のトグル（[`toggle_window_zoom`]。#657） | `zoom_window()`（NSWindow の zoom がトグル） | 最大化中なら [`restore_window`]、でなければ `zoom_window()` | macOS と同じ |
//! | 元に戻す（[`restore_window`]。#584） | 最大化中なら `zoom_window()` | user32 `ShowWindowAsync(SW_RESTORE)` | macOS と同じ |
//! | アプリメニュー（[`app_menus`]。#485 / #657） | アプリ名メニューあり（OS のグローバルメニューバー） | アプリ名メニューなし（`menu_bar.rs` が窓の中に 1 行で描く） | Windows と同じ |
//! | メニューバー行の高さ（[`MENU_BAR_HEIGHT`]） | 0 | 30 | 30 |
//! | 自前のウィンドウコントロールの幅（[`WINDOW_CONTROLS_PX`]） | 0 | 102 | 102 |
//! | traffic lights の余白（[`TRAFFIC_LIGHTS_SPACER`]） | 68 | 0 | 0 |
//! | ズーム項目のラベル（[`zoom_label`]） | 「拡大・縮小」 | 「最大化・元のサイズに戻す」 | Windows と同じ |
//! | platform 修飾の表記（[`platform_modifier_prefix`]） | `Cmd+` | `Win+` | Windows と同じ |
//!
//! 窓の位置・寸法を**外から**決める値（座標の解釈・検査）は GPUI に依存しない値の層
//! `tako_core::platform::window_bounds`（#1442）にあり、ここはその結果の矩形を OS へ当てるだけ。
//! 呼び手の `apply_window_geometry` / `apply_window_state`（CLI / MCP の `tako window`）は
//! `main.rs` に残る（OS 差を持たない）。
//!
//! ## 移設の由来（#2858）
//!
//! `main.rs` / `menu_bar.rs` / `tab_bar.rs` に `#[cfg]` で散っていた腕を、本体のトークン列を
//! 変えずにここへ移した（挙動は不変。足したのは境界の外から呼ぶための `pub(crate)` だけ）。
//! 番犬は `crates/tako-control/tests/platform_parity.rs` の `窓の枠のos差が境界の外に戻っていない`。
//!
//! macOS のアプリ名メニューだけが使う文言（`ui_text::menu` の `APP` / `services` ほか 5 つ）は
//! i18n カタログの検査対象なので `ui_text` に残し、Windows で未使用になる分の
//! `allow(dead_code)` も向こうに残る（コードの分岐ではなく lint の属性。理由は向こうの注記）。

use gpui::Window;

use crate::keybindings::{
    AboutTako, CheckForUpdates, OpenDocumentation, OpenSettings, Quit, ReportIssue,
};
#[cfg(target_os = "macos")]
use crate::keybindings::{HideApp, HideOthers, ShowAllApps};
#[cfg(not(target_os = "macos"))]
use crate::menu_bar::WINDOW_CONTROL_SIZE;
#[cfg(target_os = "windows")]
use crate::native_window_handle;
use crate::{edit_menu, file_menu_common_items, view_menu, window_menu};

// --- 枠の寸法 ----------------------------------------------------------------

/// in-window メニューバー行の高さ（px。Issue #657）。
///
/// macOS はメニューが OS のグローバルメニューバーへ載るのでこの行を持たない（= 0）。
/// Windows は `gpui_windows` がウィンドウメニューを一切作らないため、tako が
/// 自前の 1 行として描く（`menu_bar.rs`）
#[cfg(target_os = "macos")]
pub(crate) const MENU_BAR_HEIGHT: f32 = 0.0;
#[cfg(not(target_os = "macos"))]
pub(crate) const MENU_BAR_HEIGHT: f32 = 30.0;

/// 自前ウィンドウコントロール群の概算幅（3 ボタン + gap + 左マージン）。
/// macOS は native traffic lights が同居するので描かない = 0
#[cfg(target_os = "macos")]
pub(crate) const WINDOW_CONTROLS_PX: f32 = 0.0;
#[cfg(not(target_os = "macos"))]
pub(crate) const WINDOW_CONTROLS_PX: f32 = WINDOW_CONTROL_SIZE * 3.0 + 12.0;

/// traffic lights（12px × 3 + gap 8px × 2 = 52px）+ 右余白 16px。
/// native traffic lights を持つのは macOS だけなので、他プラットフォームでは
/// 意味のない左端余白になる（#584）。0 にして左端からタブを並べる
#[cfg(target_os = "macos")]
pub(crate) const TRAFFIC_LIGHTS_SPACER: f32 = 68.0;
#[cfg(not(target_os = "macos"))]
pub(crate) const TRAFFIC_LIGHTS_SPACER: f32 = 0.0;

// --- 表記 --------------------------------------------------------------------

/// 「ウインドウ」メニューのズーム項目のラベル（#657）。
/// macOS = 「拡大・縮小」のトグル、Windows = 「最大化・元のサイズに戻す」。
///
/// 選択は `#[cfg]` ではなく**この中の `cfg!`** で行う。`#[cfg]` で片方の呼び出しを消すと、
/// 使われなくなった `ui_text::menu` の関数が **dead_code になり `clippy -D warnings` の CI が
/// 落ちる**（macOS で実際に落ちた）。#2858 で `main.rs` の `window_menu` の中から移した
pub(crate) fn zoom_label() -> &'static str {
    use crate::ui_text::menu as m;
    if cfg!(target_os = "macos") {
        m::zoom_window()
    } else {
        m::maximize_restore()
    }
}

/// GPUI の platform 修飾をメニューの右端へ書くときの表記（in-window メニューバーの
/// `menu_bar::format_keystroke`。#657）。GPUI の platform 修飾は macOS = ⌘ / Windows = Win キー。
/// #2858 で `format_keystroke` の中から移した
pub(crate) fn platform_modifier_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+"
    } else {
        "Win+"
    }
}

// --- アプリメニュー ----------------------------------------------------------

/// アプリメニューバーを構成する（macOS: tako / File / Edit / View / Window / Help。#485）。
///
/// **すべての項目は実在の動作に配線する**（無反応のダミーを置かない）。項目名は
/// `ui_text::menu` が言語別に解決し、言語切替時は `TakoApp::render` が貼り直す。
/// ショートカット表示は GPUI が `keybindings::key_bindings()` から自動で引く
#[cfg(target_os = "macos")]
pub(crate) fn app_menus() -> Vec<gpui::Menu> {
    use crate::ui_text::menu as m;
    use gpui::{Menu, MenuItem, SystemMenuType};
    vec![
        Menu::new(m::APP).items(vec![
            MenuItem::action(m::about(), AboutTako),
            MenuItem::action(m::check_updates(), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(m::settings(), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(m::services(), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(m::hide_app(), HideApp),
            MenuItem::action(m::hide_others(), HideOthers),
            MenuItem::action(m::show_all(), ShowAllApps),
            MenuItem::separator(),
            MenuItem::action(m::quit(), Quit),
        ]),
        Menu::new(m::file()).items(file_menu_common_items()),
        edit_menu(),
        view_menu(),
        window_menu(),
        Menu::new(m::help()).items(vec![
            MenuItem::action(m::documentation(), OpenDocumentation),
            MenuItem::action(m::report_issue(), ReportIssue),
        ]),
    ]
}

/// アプリメニューバーを構成する（Windows: ファイル / 編集 / 表示 / ウインドウ / ヘルプ。#657）。
///
/// `gpui_windows` の `Platform::set_menus` は渡された `Menu` を内部に保存するだけで
/// ウィンドウメニューを作らないため、この構成は tako が自前で 1 行として描く
/// （`menu_bar.rs`）。並びは Explorer / VSCode の作法に合わせ、**アプリ名メニューを
/// 持たない**。About・アップデート確認は「ヘルプ」、設定・終了は「ファイル」末尾。
///
/// # macOS 固有項目を構造的に持たない（**これは安全装置**）
///
/// - `HideOthers` / `ShowAllApps` は `gpui_windows` の `hide_other_apps` /
///   `unhide_other_apps` が `unimplemented!()` なので、**到達させると panic =
///   アプリごと abort**（器の無いペインは全滅）する。`keybindings::macos_only_bindings`
///   がバインドを張らないことで塞いでいるのと同じ地雷を、メニュー側でも塞ぐ
/// - `HideApp` は Windows の `hide()` が no-op なので置いても無反応（ダミー禁止）
/// - `Services` は macOS の OS サブメニュー
///
/// この不変条件は `windowsのメニューにmacos固有の項目が無い` が固定している
#[cfg(not(target_os = "macos"))]
pub(crate) fn app_menus() -> Vec<gpui::Menu> {
    use crate::ui_text::menu as m;
    use gpui::{Menu, MenuItem};
    let mut file_items = file_menu_common_items();
    file_items.push(MenuItem::separator());
    file_items.push(MenuItem::action(m::settings(), OpenSettings));
    file_items.push(MenuItem::action(m::quit(), Quit));
    vec![
        Menu::new(m::file()).items(file_items),
        edit_menu(),
        view_menu(),
        window_menu(),
        Menu::new(m::help()).items(vec![
            MenuItem::action(m::about(), AboutTako),
            MenuItem::action(m::check_updates(), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(m::documentation(), OpenDocumentation),
            MenuItem::action(m::report_issue(), ReportIssue),
        ]),
    ]
}

// --- 窓の操作 ----------------------------------------------------------------

/// 窓の矩形を OS へ当てる（macOS）。
///
/// GPUI の窓矩形は「**そのディスプレイの左上が原点**・y は下向き」（`MacDisplay::bounds`
/// の原点が常に `(0,0)` で、窓の生成・読み戻しの両方が画面の原点を足し引きしている）。
/// NSWindow の frame は「**主ディスプレイの左下が原点**・y は上向き」なので、
/// CoreGraphics のグローバル矩形（左上原点）を挟んで変換する。
#[cfg(target_os = "macos")]
pub(crate) fn set_window_frame(
    window: &Window,
    geometry: tako_control::protocol::WindowGeometry,
    display_id: Option<u64>,
) {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgRect {
        origin: CgPoint,
        size: CgSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CgRect;
    }
    // 宣言はクレート内の既存（`platform/pdf/macos.rs`）と**同じ形**にする。
    // 別の型で宣言し直すと `clashing_extern_declarations` で落ちる
    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn sel_registerName(name: *const u8) -> *const c_void;
        fn objc_msgSend(receiver: *const c_void, sel: *const c_void, ...) -> *const c_void;
    }

    /// `setFrame:display:` を呼ぶための型。**可変長で呼んではいけない**:
    /// Apple ARM64 の可変長引数はスタックへ積まれるので、NSRect をそのまま渡すと
    /// 引数が壊れる。同じシンボルを非可変長の型で呼び直す
    type SetFrame = unsafe extern "C" fn(*const c_void, *const c_void, CgRect, bool);

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let raw_window_handle::RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: NSView は GPUI が窓を持っているあいだ有効。ここはメインスレッド
    // （`cx.defer` の中）なので AppKit を直接叩いてよい
    unsafe {
        let ns_view = appkit.ns_view.as_ptr() as *const c_void;
        let ns_window = objc_msgSend(ns_view, sel_registerName(c"window".as_ptr() as *const u8));
        if ns_window.is_null() {
            return;
        }
        let display = display_id
            .map(|id| id as u32)
            .unwrap_or_else(|| CGMainDisplayID());
        let db = CGDisplayBounds(display);
        let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
        // 置き先ディスプレイ内の座標 → グローバル（左上原点）→ Cocoa（左下原点）
        let global_x = db.origin.x + f64::from(geometry.x);
        let global_top = db.origin.y + f64::from(geometry.y);
        let frame = CgRect {
            origin: CgPoint {
                x: global_x,
                y: main_height - (global_top + f64::from(geometry.height)),
            },
            size: CgSize {
                width: f64::from(geometry.width),
                height: f64::from(geometry.height),
            },
        };
        let set_frame: SetFrame = std::mem::transmute(objc_msgSend as *const ());
        set_frame(
            ns_window,
            sel_registerName(c"setFrame:display:".as_ptr() as *const u8),
            frame,
            true,
        );
    }
}

/// 窓の矩形を OS へ当てる（Windows。Issue #1442）。
///
/// GPUI Windows の窓矩形は仮想デスクトップのグローバル座標を DPI 倍率で割った
/// **論理ピクセル**（`logical_point(x, y, scale_factor)`）なので、`SetWindowPos` へ
/// 渡すときは倍率を掛けて物理ピクセルへ戻す。
#[cfg(target_os = "windows")]
pub(crate) fn set_window_frame(
    window: &Window,
    geometry: tako_control::protocol::WindowGeometry,
    _display_id: Option<u64>,
) {
    /// `SWP_NOZORDER | SWP_NOACTIVATE`（winuser.h）。前後関係と入力焦点は動かさない
    const SWP_FLAGS: u32 = 0x0004 | 0x0010;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }

    let Some(hwnd) = native_window_handle(window) else {
        return;
    };
    let scale = window.scale_factor();
    // SAFETY: HWND は GPUI が生きているあいだ有効。SetWindowPos は対象ウィンドウの
    // スレッドへ送るだけで、この場でブロックしない
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            (geometry.x * scale) as i32,
            (geometry.y * scale) as i32,
            (geometry.width * scale) as i32,
            (geometry.height * scale) as i32,
            SWP_FLAGS,
        );
    }
}

/// 窓の矩形を OS へ当てる（macOS / Windows 以外）。移動の口が無いので寸法だけ当てる
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn set_window_frame(
    _window: &Window,
    _geometry: tako_control::protocol::WindowGeometry,
    _display_id: Option<u64>,
) {
}

/// メニューの「拡大・縮小」（`ZoomWindow`）の実体（macOS。Issue #657）。
///
/// NSWindow の zoom はトグルなので**現状のまま**素直に呼ぶ（挙動不変）
#[cfg(not(target_os = "windows"))]
pub(crate) fn toggle_window_zoom(window: &mut Window) {
    window.zoom_window();
}

/// メニューの「最大化・元のサイズに戻す」（`ZoomWindow`）の実体（Windows。Issue #657）。
///
/// GPUI Windows の `zoom()` は `SW_MAXIMIZE` 固定でトグルにならない（#584 で判明）。
/// メニュー項目が「最大化しかできない」のは動作の欠けなので、復元は #584 で用意した
/// `restore_window`（user32 の `SW_RESTORE`）へ回して状態で出し分ける
#[cfg(target_os = "windows")]
pub(crate) fn toggle_window_zoom(window: &mut Window) {
    if window.is_maximized() {
        restore_window(window);
    } else {
        window.zoom_window();
    }
}

/// 最大化を解除して元のサイズへ戻す（Issue #584）。
///
/// macOS は NSWindow の zoom がトグルなので `zoom_window()` がそのまま復元になる。
#[cfg(not(target_os = "windows"))]
pub(crate) fn restore_window(window: &mut Window) {
    if window.is_maximized() {
        window.zoom_window();
    }
}

/// Windows の復元（Issue #584）。
///
/// GPUI Windows の `zoom()` は `SW_MAXIMIZE` 固定で、復元手段を公開していない
/// （rev `cafbf4b5`）。GUI のキャプションボタンは `WM_NCLBUTTONUP` の中で GPUI が
/// `IsZoomed` を見て `SW_NORMAL` を送っているが、その経路は NC ヒットテスト由来で
/// しか通らない。CLI / MCP からも復元できるよう、ここだけ user32 の
/// `ShowWindowAsync` を直接呼ぶ（`windows` クレートを足さない最小 FFI。
/// 既存の CoreGraphics / PDFKit 呼び出しと同じ作法）。
#[cfg(target_os = "windows")]
pub(crate) fn restore_window(window: &mut Window) {
    /// `SW_RESTORE`（winuser.h）。最小化・最大化を解除して元のサイズ・位置へ戻す
    const SW_RESTORE: i32 = 9;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindowAsync(hwnd: isize, n_cmd_show: i32) -> i32;
    }

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    // SAFETY: HWND は GPUI が生きている間有効。ShowWindowAsync は
    // 対象ウィンドウのスレッドへポストするだけで、この場でブロックしない
    unsafe {
        ShowWindowAsync(win32.hwnd.get(), SW_RESTORE);
    }
}

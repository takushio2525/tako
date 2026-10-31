//! ネイティブ Web ビューペイン（FR-3.8）
//!
//! wry（macOS = WKWebView / Windows = WebView2）を GPUI ウィンドウの子ビューとして
//! 統合する。GPUI は枠・タイトルバー・URL バーだけを描き、本文矩形へ wry の bounds を
//! フレーム同期で追従させる。クリック・スクロール・キー入力・IME は OS がネイティブ
//! webview に直接配送するため、tako 側での入力中継は不要。
//!
//! ページは `WebViewEntry` として PaneId から独立に管理する。ペインから外しても
//! （dock 退避）インスタンスが生きるため、SPA の状態・ログイン・スクロール位置が
//! 維持される = 「ブラウザタブの維持」（Issue #155）。
//!
//! 既知の制約: ネイティブビューは GPUI の GPU 合成レイヤの**上**に乗るため、
//! tako のオーバーレイ UI（ドロワー・ピン留め窓・ホバープレビュー等）は webview の
//! 下に隠れる。重なりが生じる UI の表示中は呼び出し側が `sync_frame(None)` で隠すこと
//! （`.agent/architecture.md`「Web ビューペイン」節）。

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tako_control::HostRefusal;
use tako_core::PaneId;

/// Web ビューの生成を **gpui の App を借用していないところ**で 1 本ずつ行うか（#724 の W1）。
///
/// Windows の wry（WebView2）は環境・コントローラ・初期化スクリプトの作成完了を
/// `webview2_com::wait_with_pump`（`GetMessageA` + `DispatchMessageA` の入れ子のループ）で待つ
/// （wry-0.57.1 `src/webview2/mod.rs:377 / :435 / :1414`。0.55 から変わらない）。App を借用したまま
/// （クリックハンドラ・render・dispatch のどれも借用の内側）生成すると、ポンプが走らせた
/// gpui の前景タスクの `AsyncApp::update_entity` が `borrow_mut()` で二重借用の panic を起こし、
/// 窓プロシージャを跨ぐので巻き戻せず abort する（#724 の実機の panic.log と隔離環境の再現で
/// 同じ署名）。真のときは `TakoApp::create_webview` が待ち行列へ積むだけにし、生成は
/// 常駐の生成タスク（`TakoApp::spawn_webview_builder`）が `update` の**外**で行う。
///
/// macOS の WKWebView の生成はポンプを回さないので偽（従来どおり同期で生成）。
/// A/B: Windows で `TAKO_724_LEGACY=1` = 修正前の同期生成（W2 の ipc も戻る）/
/// macOS で `TAKO_724_ASYNC=1` = Windows と同じ非同期の経路を通す（経路の検証用）
pub fn async_build_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        async_build_decision(
            cfg!(windows),
            legacy_724(),
            std::env::var_os("TAKO_724_ASYNC").is_some(),
        )
    })
}

/// [`async_build_enabled`] の判断（純関数）
pub(crate) fn async_build_decision(windows: bool, legacy: bool, forced: bool) -> bool {
    !legacy && (windows || forced)
}

/// ページから tako へ ipc（`window.ipc.postMessage`）でタイトル・URL を送らせるか（#724 の W2）。
///
/// wry 0.55 の WebView2 は ipc ハンドラが付いていると、COM のコールバックの中で
/// `Request::builder().uri(url).body(js).unwrap()`（wry-0.55.1 `src/webview2/mod.rs:910`）を呼んだ。
/// `url` はメッセージを送った文書の URI で、data: / about:blank や URI が未確定のときは
/// `http::Uri` にならず `InvalidUri(Empty)` で panic → 巻き戻せず abort した（#724 のコメントの実機の
/// スタック）。上流は 0.56.1 でこれを `match` に変え（wry#1772）、いまの 0.57.1 は不正な URI の
/// メッセージを捨てるだけになった（`attach_ipc_handler`。番犬が registry の実ソースで確かめる）。
/// それでも Windows で付けないままにしているのは、付けた形を Windows の実機でまだ通していないため
/// （#2445。戻すのは実機で確かめてから）。付けなければ wry はハンドラの登録そのものをしない
/// （`if let Some(ipc_handler) = attributes.ipc_handler.take()`）。
/// JS の側で http(s) のときだけ送る形は、`http::Uri` が受け付ける文字列を tako が当てる形になり
/// 根拠が弱いので採らない。タイトル・URL は 2 秒ごとの [`WebViewEntry::poll_state`]
/// （eval の経路。もともと正本）が拾う。Windows で失うのは http(s) のページの即時の更新だけ
pub(crate) fn ipc_hook_enabled() -> bool {
    ipc_hook_decision(cfg!(windows), legacy_724())
}

/// [`ipc_hook_enabled`] の判断（純関数）
pub(crate) fn ipc_hook_decision(windows: bool, legacy: bool) -> bool {
    !windows || legacy
}

/// 注入（#724 の検証用）: 生成タスクが仕事を取り出してから wry を呼ぶまで、ほかのタスクを
/// 止めずに待つ（`TAKO_724_INJECT_BUILD_DELAY_MS=<ms>`）。Windows の最初の生成
/// （msedgewebview2 の起動待ち = 数秒のあいだ前景タスクが走り続ける）を macOS で再現し、
/// 生成中のもう 1 本・生成中に閉じる・生成中に落ちる（W4）を実経路で見る
pub fn inject_build_delay() -> Option<std::time::Duration> {
    std::env::var("TAKO_724_INJECT_BUILD_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(std::time::Duration::from_millis)
}

/// #724 の修正前へ戻す（`TAKO_724_LEGACY=1`。Windows の実機の A/B 用）
fn legacy_724() -> bool {
    std::env::var_os("TAKO_724_LEGACY").is_some()
}

/// ページに要素の全画面表示（Fullscreen API。動画の全画面ボタン等）を許すか（#2445）。
///
/// wry 0.56 から macOS の WKWebView は要素の全画面表示が常に入る（wry#1779 / #1780。0.55 は
/// `fullscreen` feature を入れたときだけで、tako は入れていなかった）。全画面のあいだ WebKit は
/// WKWebView を描画ビューから外して全画面の窓へ移し、元の場所へ代わりのビュー（スナップショット）を
/// 置く（WebKit の `WKFullScreenWindowController.mm`）。tako の前提「WKWebView の superview は
/// gpui の描画ビュー」（#1481 / #1837）がそのあいだ崩れるので、同じソースから次が読める:
/// ①位置合わせ（`set_bounds`）が全画面の窓の中の WKWebView を動かし、戻った後は代わりのビューの
/// 古い位置に残る ②閉じると wry の Drop が全画面の窓から外し、WebKit の戻す処理が tako の知らない
/// WKWebView を tako の窓へ戻しうる ③全画面のあいだ tako の窓の宛先が NSWindow 自身になり、
/// ターミナルへ打鍵が届かない。手当てと実機での確かめが済むまでは 0.55 と同じく切る（依存の更新で
/// 製品の挙動を変えない）。A/B: `TAKO_2445_FULLSCREEN=1` = wry の既定（入れる）
pub(crate) fn element_fullscreen_enabled() -> bool {
    std::env::var_os("TAKO_2445_FULLSCREEN").is_some()
}

/// 作った Web ビューへ [`element_fullscreen_enabled`] の判断を当てる（macOS だけ。WebView2 の
/// 全画面は WebView2 の窓の中で閉じるので触らない）
fn apply_element_fullscreen(view: &wry::WebView) {
    #[cfg(target_os = "macos")]
    {
        use wry::WebViewExtMacOS;
        let wk = view.webview();
        key_monitor::set_element_fullscreen(
            &*wk as *const _ as *const std::ffi::c_void,
            element_fullscreen_enabled(),
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = view;
    }
}

/// Web ビューへ渡す WebContext（#2431）。Windows だけユーザーデータの置き場を明示し、
/// macOS は None（WebContext を渡さない = 従来と同じ）。置き場は Web ビューを作るたびに決め直す
/// （旧い置き場から移せていないあいだは旧い置き場を使い続け、移し終えたら次の Web ビューから
/// 新しい置き場になる。決め方は `platform::webview2::data_dir_choice` の 1 実装）。
/// wry の WebView2 が置き場を読むのは生成の中（`create_environment`）だけなので、生成が返れば捨ててよい
fn web_context() -> Option<wry::WebContext> {
    let choice = tako_core::platform::webview2::data_dir_choice();
    note_data_dir_choice(&choice);
    choice
        .path()
        .map(|dir| wry::WebContext::new(Some(dir.to_path_buf())))
}

/// どの置き場を渡したかを persist.log へ（変わったときだけ 1 行。#2431）
fn note_data_dir_choice(choice: &tako_core::platform::webview2::DataDirChoice) {
    use tako_core::platform::webview2::DataDirChoice;
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let line = match choice {
        DataDirChoice::NotApplicable => return,
        DataDirChoice::Unresolved => {
            "Web ビューのユーザーデータの置き場が決まらない（LOCALAPPDATA も USERPROFILE も無い）ので \
             WebView2 の既定に任せる (#2431)"
                .to_string()
        }
        DataDirChoice::New(dir) => {
            format!("Web ビューのユーザーデータ: {} (#2431)", dir.display())
        }
        DataDirChoice::PendingLegacy(dir) => format!(
            "Web ビューのユーザーデータ: 旧い置き場 {} を使う（使用中で移せていない。次の起動で移す。#2431）",
            dir.display()
        ),
    };
    if let Ok(mut last) = LAST.lock() {
        if last.as_deref() == Some(line.as_str()) {
            return;
        }
        *last = Some(line.clone());
    }
    tako_control::diag::persist_log(&line);
}

/// 生成の失敗の文言（#2431）。Windows は「ランタイムが無い / 古い / その他」に分けて次にやることを
/// 言い（種別は `create_failure` へ）、macOS は従来の文言のまま
fn create_failure_reason(raw: &str, shared: &Arc<Mutex<WebShared>>) -> String {
    let Some(failure) = tako_core::platform::webview2::classify_here(raw) else {
        return format!("webview 生成失敗: {raw}");
    };
    if let Ok(mut s) = shared.lock() {
        s.create_failure = Some(failure.kind());
    }
    failure.message(raw)
}

/// Web ビューの段階（#724）。`Ready` だけが wry の Web ビューを持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebViewPhase {
    /// wry の Web ビューがある（表示・操作できる）
    Ready,
    /// 生成待ち（Windows では生成を App の借用の外で 1 本ずつ行う）
    Queued,
    /// 生成中（wry が入れ子のメッセージループを回している）
    Building,
    /// 前回この Web ビューの生成中に tako が落ちたので、自動では開かずに保留している（W4）
    Held,
    /// 生成に失敗した（理由はエラー表示。再読み込みで生成し直す）
    Failed,
}

impl WebViewPhase {
    /// CLI / MCP の `web list` / `read` / `open` の `state`
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::Building => "creating",
            Self::Held => "held",
            Self::Failed => "failed",
        }
    }
}

/// GPUI ウィンドウの生ハンドルを保持して wry の親にするラッパー。
/// `gpui::Window` は render 中しか触れないため、render で採取した
/// `RawWindowHandle` を保持し、dispatch（IPC / MCP）からの webview 生成でも使う。
///
/// **採った窓が閉じたら採り直す**（#1837）。以前は「tako は単一ウィンドウ構成で、
/// ウィンドウ破棄 = アプリ終了」を前提に初回 render で 1 度だけ採っていたが、
/// 複数ウィンドウ（#339 で最初の窓を閉じる）と macOS の Dock 復帰（#381 で最後の窓を
/// 閉じて開き直す）でこの前提は崩れていた。閉じた窓の描画ビューを親にすると、wry は
/// 親の NSWindow を `ns_view.window().unwrap()`（wry-0.57.1 wkwebview/mod.rs:434）で引くので、
/// 窓が解放されていれば **panic でプロセスごと落ちる**。解放されていなければ
/// 閉じた窓の中に見えない Web ビューを作る。採り直しの判定は呼び出し側
/// （`TakoApp::refresh_webview_parent`）が [`Self::owner`] と [`Self::attached`] で行う
pub struct WindowHandleBox {
    raw: raw_window_handle::RawWindowHandle,
    /// 採った GPUI の窓（#1837。この窓が閉じたら採り直す）
    owner: gpui::AnyWindowHandle,
}

impl WindowHandleBox {
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        // gpui::Window には同名の inherent メソッド（AnyWindowHandle を返す）があるため、
        // raw-window-handle の trait メソッドを明示的に呼ぶ
        let raw = raw_window_handle::HasWindowHandle::window_handle(window)
            .ok()?
            .as_raw();
        let handle = Self {
            raw,
            owner: window.window_handle(),
        };
        // 描画ビューを tako も retain する（#1837）。gpui は窓を閉じると描画ビューを
        // 窓ごと手放すので、持っていないと採り直すまでのあいだ解放済みのポインタになり、
        // 「窓に載っているか」を問うこと自体ができない（Drop で release する）
        key_monitor::retain_view(handle.ns_view_ptr());
        Some(handle)
    }

    /// wry の親ビュー（macOS では gpui の描画ビュー `GPUIView` の NSView）。
    /// キー入力の宛先を戻す先の正本（#1481）。macOS 以外では null
    fn ns_view_ptr(&self) -> *const std::ffi::c_void {
        #[cfg(target_os = "macos")]
        {
            if let raw_window_handle::RawWindowHandle::AppKit(h) = &self.raw {
                return h.ns_view.as_ptr() as *const std::ffi::c_void;
            }
        }
        std::ptr::null()
    }

    /// 親を言い分ける値（macOS = 描画ビュー / Windows = HWND。#1837）。
    /// Web ビューが「いまの親の子か」を突き合わせるのに使う
    pub fn key(&self) -> usize {
        match &self.raw {
            raw_window_handle::RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as usize,
            raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get() as usize,
            _ => 0,
        }
    }

    /// 採った GPUI の窓（#1837）
    pub fn owner(&self) -> gpui::AnyWindowHandle {
        self.owner
    }

    /// 親がいまも窓に載っているか（#1837。macOS = 描画ビューの `window` が nil でない /
    /// Windows = HWND が生きている）。偽のまま wry へ渡すと macOS は panic する
    pub fn attached(&self) -> bool {
        match &self.raw {
            raw_window_handle::RawWindowHandle::AppKit(_) => {
                key_monitor::view_has_window(self.ns_view_ptr())
            }
            raw_window_handle::RawWindowHandle::Win32(h) => win32_window_alive(h.hwnd.get()),
            _ => true,
        }
    }
}

impl Drop for WindowHandleBox {
    fn drop(&mut self) {
        key_monitor::release_view(self.ns_view_ptr());
    }
}

/// 生成タスクへ親を持ち出すための写し（#724）。描画ビューを持つ数を揃える
/// （macOS は写しの分も retain し、Drop が release する。Windows の親は HWND で何もしない）
impl Clone for WindowHandleBox {
    fn clone(&self) -> Self {
        key_monitor::retain_view(self.ns_view_ptr());
        Self {
            raw: self.raw,
            owner: self.owner,
        }
    }
}

impl raw_window_handle::HasWindowHandle for WindowHandleBox {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: 描画ビューは from_window で retain しているので、このラッパーが
        // 生きているあいだは解放されない（窓から外れていることはある = 呼び出し側が
        // `attached` を見てから wry へ渡す。#1837）
        unsafe { Ok(raw_window_handle::WindowHandle::borrow_raw(self.raw)) }
    }
}

/// HWND が生きているか（Windows。#1837）
#[cfg(windows)]
fn win32_window_alive(hwnd: isize) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn IsWindow(hwnd: isize) -> i32;
    }
    // SAFETY: IsWindow は任意の値を受け取り、窓でなければ 0 を返すだけ
    unsafe { IsWindow(hwnd) != 0 }
}

#[cfg(not(windows))]
fn win32_window_alive(_hwnd: isize) -> bool {
    true
}

/// 親が窓に載っていないので Web ビューを作らない理由（#1837。CLI / MCP の応答と
/// persist.log の同じ 1 文）
pub fn detached_parent_error() -> String {
    "Web ビューの親（tako の窓）が閉じているので Web ビューを作れない。\
     tako の窓を開いてから開き直す（#1837）"
        .to_string()
}

/// フレーム同期をルート render で行う新経路（#838）を切る逃げ道
/// （`TAKO_838_NO_ROOT_WEBVIEW_SYNC=1`）。
///
/// 1 の間は #838 以前の「ペイン本体の render が印を付け、ルートの掃き出しが
/// 印の無いものを隠す」経路に戻る。同じバイナリでちらつきを A/B するためのもの
pub(crate) fn root_sync_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_838_NO_ROOT_WEBVIEW_SYNC").is_some())
}

/// dock 管理用の永続 ID。ペインとは独立で、ペインを閉じてもページは生きる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WebViewId(pub u64);

impl WebViewId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// JS 側（ipc ハンドラ / eval コールバック）から書き戻される共有状態。
/// wry のコールバックは `'static` を要求し TakoApp を掴めないため、
/// Arc<Mutex> 経由で受け渡して UI は render 時に読む
#[derive(Default)]
pub struct WebShared {
    /// 現在の URL（ナビゲーション・SPA 遷移で更新）
    pub url: String,
    /// ページタイトル（<title> の変化を初期化スクリプトが通知）
    pub title: String,
    /// ページ読み込み中か（PageLoadEvent::Started..Finished）
    pub loading: bool,
    /// evaluate_script_with_callback の結果置き場（token → 結果 JSON 文字列）
    pub eval_results: HashMap<u64, String>,
    /// ナビゲーション失敗時のエラーメッセージ（None = 正常）
    pub error: Option<String>,
    /// 読み込みに失敗した URL（リトライ用に保持）
    pub failed_url: Option<String>,
    /// ナビゲーション開始時刻（タイムアウト検知用）
    pub nav_started_at: Option<std::time::Instant>,
    /// wry の didCommitNavigation（PageLoadEvent::Started）が発火したか。
    /// DNS 失敗等では発火しないため、タイムアウトと組み合わせて失敗を検知する
    pub nav_committed: bool,
    /// ナビゲーション先の URL（poll_state の JS 評価で url が上書きされるため別途保持）
    pub nav_target_url: Option<String>,
    /// target=_blank / window.open で要求された新規ウィンドウ URL（メインループで消費）
    pub pending_new_window_urls: Vec<String>,
    /// ブラウザ履歴で「戻る」が可能か（history.length > 1 かつ現在位置が先頭でない）
    pub can_go_back: bool,
    /// ブラウザ履歴で「進む」が可能か
    pub can_go_forward: bool,
    /// 最初の読み込みが終わったか（PageLoadEvent::Finished。#724 の W4 の印を外す合図）
    pub first_load_done: bool,
    /// 直近の生成の失敗の種別（#2431。`runtime_missing` / `runtime_outdated` / `other`。
    /// Windows だけ = macOS は従来どおり None。`web list` / `read` の `create_failure`）
    pub create_failure: Option<&'static str>,
}

/// タイトル・URL の変化を Rust 側へ通知する初期化スクリプト。
/// `window.ipc.postMessage` は wry が全プラットフォームで注入する
const STATE_HOOK_JS: &str = r#"
(function () {
  if (window.__takoHooked) { return; }
  window.__takoHooked = true;
  function report() {
    try {
      window.ipc.postMessage(JSON.stringify({
        kind: "state",
        title: document.title || "",
        url: location.href
      }));
    } catch (e) {}
  }
  addEventListener("load", function () {
    report();
    var te = document.querySelector("title");
    if (te) {
      new MutationObserver(report).observe(te, {
        childList: true, characterData: true, subtree: true
      });
    }
  });
  addEventListener("popstate", report);
  addEventListener("hashchange", report);
  var hp = history.pushState, hr = history.replaceState;
  history.pushState = function () { hp.apply(this, arguments); report(); };
  history.replaceState = function () { hr.apply(this, arguments); report(); };
  setTimeout(report, 0);
})();
"#;

/// 1 ページ = 1 エントリ。ペイン表示中（`pane = Some`）と dock 退避中（`None`）の
/// 両状態を通じて wry WebView インスタンスを保持し続ける。
///
/// **wry の Web ビューをまだ持たないエントリもある**（#724）: Windows では生成を App の借用の外で
/// 1 本ずつ行うので、生成待ち・生成中・保留（W4）・生成失敗のあいだもエントリは先に置く。
/// ペインの描画・`web list`・layout.json の保存・dock・閉じる経路は既存のまま URL を扱える
pub struct WebViewEntry {
    pub id: WebViewId,
    /// wry の Web ビュー。None = [`WebViewPhase::Ready`] 以外（#724）
    view: Option<wry::WebView>,
    /// 段階（#724）。`&self` の経路（navigate / retry）から生成し直しを頼めるよう `Cell`
    phase: Cell<WebViewPhase>,
    pub shared: Arc<Mutex<WebShared>>,
    /// Some = このペインに表示中 / None = dock 退避中
    pub pane: Option<PaneId>,
    /// 直近フレームの可視状態（差分呼び出しで AppKit 往復を減らす）
    pub(crate) visible_now: bool,
    /// 直近フレームの bounds（論理 px。差分呼び出し用）
    bounds_now: Option<(f64, f64, f64, f64)>,
    /// 可視 ⇔ 不可視が実際に切り替わった回数（#838 のちらつき定量。診断専用）
    pub(crate) visible_flips: u64,
    /// bounds を実際に張り替えた回数（#838 の診断専用）
    pub(crate) bounds_sets: u64,
    /// eval トークン採番
    next_eval_token: u64,
    /// いまの親（[`WindowHandleBox::key`]。#1837）。親を採り直したら
    /// [`Self::adopt_parent`] がこれと突き合わせて付け替える。生成前は 0
    parent: usize,
}

/// 生成タスクへ渡す 1 本ぶんの仕事（#724 の W1）。借用の内側で
/// [`WebViewEntry::begin_build`] が作り、**借用の外**で [`Self::build`] を呼ぶ
pub struct BuildJob {
    pub id: WebViewId,
    url: String,
    shared: Arc<Mutex<WebShared>>,
    parent: WindowHandleBox,
}

impl BuildJob {
    /// wry の Web ビューを作る。**gpui の App を借用していないところで呼ぶ**（#724）。
    /// Windows の wry はこの中で入れ子のメッセージループを回し、gpui の前景タスクが走る
    pub fn build(self) -> BuiltWebView {
        let result = WebViewEntry::build_view(&self.parent, &self.url, &self.shared);
        BuiltWebView {
            id: self.id,
            parent: self.parent.key(),
            result,
        }
    }
}

/// [`BuildJob::build`] の結果。借用の内側で [`WebViewEntry::install_built`] が取り付ける
pub struct BuiltWebView {
    pub id: WebViewId,
    parent: usize,
    result: Result<wry::WebView, String>,
}

impl WebViewEntry {
    /// wry WebView を GPUI ウィンドウの子ビューとして生成する（その場で生成する経路。macOS）。
    /// 生成直後は不可視（次フレームの `sync_frame` で表示位置が決まる）
    pub fn build(window: &WindowHandleBox, id: WebViewId, url: &str) -> Result<Self, String> {
        let mut entry = Self::queued(id, url)?;
        let view = Self::build_view(window, url, &entry.shared)?;
        entry.install(view, window.key());
        Ok(entry)
    }

    /// 生成待ちのエントリ（#724。wry の Web ビューはまだ無い）
    pub fn queued(id: WebViewId, url: &str) -> Result<Self, String> {
        validate_url(url)?;
        Ok(Self::placeholder(id, url, WebViewPhase::Queued))
    }

    /// 保留のエントリ（#724 の W4。前回この Web ビューの生成中に tako が落ちた）。
    /// 開くのは利用者が頼んだとき（[`Self::navigate`] / [`Self::retry`]）だけ
    pub fn held(id: WebViewId, url: &str) -> Self {
        Self::placeholder(id, url, WebViewPhase::Held)
    }

    fn placeholder(id: WebViewId, url: &str, phase: WebViewPhase) -> Self {
        let shared = Arc::new(Mutex::new(WebShared {
            url: url.to_string(),
            ..Default::default()
        }));
        Self {
            id,
            view: None,
            phase: Cell::new(phase),
            shared,
            pane: None,
            visible_now: false,
            bounds_now: None,
            visible_flips: 0,
            bounds_sets: 0,
            next_eval_token: 1,
            parent: 0,
        }
    }

    /// wry の Web ビューを作る（親の検査・ハンドラの取り付け・`build_as_child`）。
    /// 状態の書き戻し先は呼び出し側のエントリの `shared`
    fn build_view(
        window: &WindowHandleBox,
        url: &str,
        shared: &Arc<Mutex<WebShared>>,
    ) -> Result<wry::WebView, String> {
        validate_url(url)?;
        // 親が窓に載っていなければ wry へ渡さない（#1837。#334 と同じく、wry の unwrap に
        // 届く前に断る = tako 全体を落とさない）。macOS の wry は親の NSWindow を
        // `ns_view.window().unwrap()` で引くので、渡すと panic する
        if !window.attached() {
            return Err(detached_parent_error());
        }
        if let Ok(mut s) = shared.lock() {
            s.create_failure = None;
        }
        let load_shared = Arc::clone(shared);
        let new_win_shared = Arc::clone(shared);
        // #2431: Windows はユーザーデータの置き場を明示する（WebContext を渡さないと wry は
        // 実行ファイルの隣の `tako-app.exe.WebView2` を使う）。macOS は渡さない = 挙動を変えない
        let mut context = web_context();
        let builder = match context.as_mut() {
            Some(context) => wry::WebViewBuilder::new_with_web_context(context),
            None => wry::WebViewBuilder::new(),
        };
        let mut builder = builder
            .with_url(url)
            .with_visible(false)
            .with_focused(false)
            .with_new_window_req_handler(move |url, _features| {
                if let Ok(mut s) = new_win_shared.lock() {
                    s.pending_new_window_urls.push(url);
                }
                wry::NewWindowResponse::Deny
            })
            .with_on_page_load_handler(move |event, url| {
                if let Ok(mut s) = load_shared.lock() {
                    match event {
                        wry::PageLoadEvent::Started => {
                            s.loading = true;
                            s.url = url;
                            s.nav_committed = true;
                            s.nav_target_url = None;
                            s.error = None;
                            s.failed_url = None;
                        }
                        wry::PageLoadEvent::Finished => {
                            s.loading = false;
                            s.url = url;
                            s.nav_started_at = None;
                            s.nav_target_url = None;
                            s.error = None;
                            s.failed_url = None;
                            s.first_load_done = true;
                        }
                    }
                }
            });
        // #724 の W2: Windows では ipc ハンドラを付けない（wry 0.55 は data: / about:blank で
        // mod.rs:910 の unwrap に届いた。0.57.1 は捨てるだけだが実機で未確認。理由は
        // `ipc_hook_enabled`）。ハンドラが無いと postMessage は捨てられるだけなので、
        // 送り手の初期化スクリプトも入れない
        if ipc_hook_enabled() {
            let ipc_shared = Arc::clone(shared);
            builder = builder
                .with_initialization_script(STATE_HOOK_JS)
                .with_ipc_handler(move |req| {
                    let body: &str = req.body();
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(body) else {
                        return;
                    };
                    if msg.get("kind").and_then(|v| v.as_str()) == Some("state") {
                        if let Ok(mut s) = ipc_shared.lock() {
                            if let Some(t) = msg.get("title").and_then(|v| v.as_str()) {
                                s.title = t.to_string();
                            }
                            if let Some(u) = msg.get("url").and_then(|v| v.as_str()) {
                                s.url = u.to_string();
                            }
                        }
                    }
                });
        }
        let view = builder
            .build_as_child(window)
            .map_err(|e| create_failure_reason(&e.to_string(), shared))?;
        // #2445: 要素の全画面表示は切る（理由は `element_fullscreen_enabled`）
        apply_element_fullscreen(&view);
        Ok(view)
    }

    /// 作った wry の Web ビューを取り付ける。ナビゲーションの見張り（10 秒）はここから数える
    /// （Windows の最初の生成は msedgewebview2 の起動を待つので、生成の前から数えると
    /// 生成だけで期限を越える）
    fn install(&mut self, view: wry::WebView, parent: usize) {
        self.view = Some(view);
        self.parent = parent;
        self.phase.set(WebViewPhase::Ready);
        self.bounds_now = None;
        // 読み込みのイベント（nav_committed / first_load_done）はここでは消さない: wry は
        // `Navigate` を最後のポンプの後に呼ぶので生成中には届かないが、先に届いていても失わない
        if let Ok(mut s) = self.shared.lock() {
            let url = s.url.clone();
            if should_track_nav(&url) && !s.nav_committed {
                s.nav_started_at = Some(std::time::Instant::now());
                s.nav_target_url = Some(url);
            }
        }
    }

    /// いまの段階（#724）
    pub fn phase(&self) -> WebViewPhase {
        self.phase.get()
    }

    /// 生成待ちのエントリから生成の仕事を取り出す（#724 の W1。借用の内側で呼ぶ）。
    /// 生成待ちでなければ None。取り出したら [`WebViewPhase::Building`] になる
    pub fn begin_build(&self, parent: &WindowHandleBox) -> Option<BuildJob> {
        if self.phase.get() != WebViewPhase::Queued {
            return None;
        }
        self.phase.set(WebViewPhase::Building);
        Some(BuildJob {
            id: self.id,
            url: self.current_url(),
            shared: Arc::clone(&self.shared),
            parent: parent.clone(),
        })
    }

    /// [`BuildJob::build`] の結果を取り付ける（借用の内側で呼ぶ）。失敗したら理由を
    /// エラー表示（再読み込みで生成し直す）に出して Err を返す
    pub fn install_built(&mut self, built: BuiltWebView) -> Result<(), String> {
        match built.result {
            Ok(view) => {
                self.install(view, built.parent);
                Ok(())
            }
            Err(reason) => {
                self.fail(&reason);
                Err(reason)
            }
        }
    }

    /// 生成に入れなかった・失敗した（#724）。理由はエラー表示に出し、再読み込みで生成し直す
    pub fn fail(&self, reason: &str) {
        self.phase.set(WebViewPhase::Failed);
        if let Ok(mut s) = self.shared.lock() {
            s.error = Some(reason.to_string());
            s.failed_url = Some(s.url.clone());
            s.loading = false;
            s.nav_started_at = None;
            s.nav_target_url = None;
        }
    }

    /// まだ wry の Web ビューが無いあいだの断り（#724。CLI / MCP の eval 等の応答）
    fn not_ready_error(&self) -> HostRefusal {
        HostRefusal::WebViewNotReady {
            id: self.id.as_u64(),
            state: self.phase.get().as_str().to_string(),
        }
    }

    /// 最初の読み込みが終わったか（#724 の W4 の印を外す合図）
    pub fn first_load_done(&self) -> bool {
        self.shared
            .lock()
            .map(|s| s.first_load_done)
            .unwrap_or(false)
    }

    /// いまの親（`parent`）の子か（#1837。`tako web list` の `attached`）
    pub fn parented_to(&self, parent: &WindowHandleBox) -> bool {
        self.parent == parent.key()
    }

    /// 親を採り直したとき、閉じた窓の描画ビューの子のままの Web ビューを新しい親へ
    /// 付け替える（#1837）。付け替えたら true。
    ///
    /// macOS は描画ビューへ `addSubview:` する（AppKit が古い親から外す）。wry の
    /// `reparent` は窓の `contentView` へ足すので使わない: キー入力の宛先を戻す先は
    /// WKWebView の superview = gpui の描画ビューでなければならない（#326 / #1481）。
    /// Windows は親の HWND が壊れると子の WebView2 の窓も壊れるので付け替えられない
    /// （窓を閉じた時点でそのページは失われる。生成と位置合わせは落ちない）
    pub fn adopt_parent(&mut self, parent: &WindowHandleBox) -> bool {
        // 生成前（#724）は付け替えるものが無い。生成は採り直した親で行われる
        if self.view.is_none() || self.parented_to(parent) || !parent.attached() {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            let Some(view) = self.view.as_ref() else {
                return false;
            };
            let wk = view.webview();
            let wk_ptr = &*wk as *const _ as *const std::ffi::c_void;
            if !key_monitor::add_subview(parent.ns_view_ptr(), wk_ptr) {
                return false;
            }
            self.parent = parent.key();
            // 窓が変わると寸法・倍率も変わりうるので、次のフレームで必ず張り直す
            self.bounds_now = None;
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// WKWebView がいま窓に載っているか（#1837。macOS の wry は位置合わせ・focus で
    /// `webview.window().unwrap()` を引くので、偽のまま呼ぶと panic する）。生成前は偽（#724）
    fn in_window(&self) -> bool {
        let Some(view) = self.view.as_ref() else {
            return false;
        };
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            let wk = view.webview();
            key_monitor::view_has_window(&*wk as *const _ as *const std::ffi::c_void)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = view;
            true
        }
    }

    /// キー入力の宛先を親（**いまの** superview = gpui の描画ビュー）へ返す（#1837）。
    ///
    /// wry の `focus_parent` は**生成時の**親ビューを宛先にする（wry が retain した `ns_view`）。
    /// 親を付け替えた後にそれを使うと、窓に居ないビューを first responder にしようとして
    /// 失敗し、**NSWindow 自身**が宛先になる（実測: その状態で #1481 の宛先読みが
    /// first responder へ `superview` を送り、NSWindow は応答しないので unrecognized
    /// selector の例外で abort した）。「WKWebView の superview が常に正しい宛先」（#1481）を
    /// その場で AppKit に聞く。付け替えていない Web ビューでは wry と同じ宛先になる
    fn focus_parent(&self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            let wk = view.webview();
            key_monitor::focus_superview(&*wk as *const _ as *const std::ffi::c_void);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = view.focus_parent();
        }
    }

    /// ページへキー入力の宛先を渡す（#1481）。窓に載っていなければ渡さない（#1837）。
    /// 生成前は理由つきで断る（#724）
    pub fn focus(&self) -> Result<(), String> {
        let Some(view) = self.view.as_ref() else {
            return Err(self.not_ready_error().into());
        };
        if !self.in_window() {
            return Err(detached_parent_error());
        }
        view.focus().map_err(|e| e.to_string())
    }

    /// フレーム同期: 表示すべきなら bounds（論理 px）を渡し、隠すなら None。
    /// 実際の AppKit / Win32 呼び出しは値が変わったときだけ行う。
    /// 生成前（#724）は置くものが無い（ペインの本文は GPUI が「準備中」「保留」を描く）
    pub fn sync_frame(&mut self, bounds: Option<(f64, f64, f64, f64)>) {
        if self.view.is_none() {
            return;
        }
        match bounds {
            // 窓に載っていない Web ビューは置けない（#1837。wry の set_bounds が
            // `webview.window().unwrap()` で panic する）。親を採り直して付け替えれば
            // 次のフレームで置かれる
            Some(_) if !self.in_window() => {}
            Some((x, y, w, h)) => {
                let Some(view) = self.view.as_ref() else {
                    return;
                };
                if self.bounds_now != Some((x, y, w, h)) {
                    let _ = view.set_bounds(wry::Rect {
                        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(x, y)),
                        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(w, h)),
                    });
                    self.bounds_now = Some((x, y, w, h));
                    self.bounds_sets = self.bounds_sets.saturating_add(1);
                }
                if !self.visible_now {
                    let _ = view.set_visible(true);
                    self.visible_now = true;
                    self.visible_flips = self.visible_flips.saturating_add(1);
                }
            }
            None => {
                if self.visible_now {
                    // 隠す前にフォーカスを親（GPUI ウィンドウ）へ返す。
                    // 返さないとキー入力が不可視 webview に吸われ続ける
                    self.focus_parent();
                    if let Some(view) = self.view.as_ref() {
                        let _ = view.set_visible(false);
                    }
                    self.visible_now = false;
                    self.visible_flips = self.visible_flips.saturating_add(1);
                }
            }
        }
    }

    /// bounds をまだ決められない状態で表示だけ先行させる（#442 の即時可視化）。
    /// 位置は次の render の `sync_frame` が補正する。
    /// 可視状態の変化はここも数える（#838 の計器が実態とずれないように）
    pub fn show_without_bounds(&mut self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        if !self.visible_now {
            let _ = view.set_visible(true);
            self.visible_now = true;
            self.visible_flips = self.visible_flips.saturating_add(1);
        }
    }

    /// タイトル・URL を JS 評価で採取して shared へ書き戻す（2 秒ポーリングから呼ぶ）。
    /// ipc（WKScriptMessageHandler）は data: URL 等で通知が届かないケースを実機で
    /// 確認したため（#155 セルフテスト診断: eval は届くが ipc の title が空のまま）、
    /// 実証済みの evaluate_script_with_callback 経路を正とし、ipc は http(s) ページでの
    /// 即時更新用に併存させる（Windows は ipc を付けないのでこれだけ。#724 の W2）
    pub fn poll_state(&self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        let _ = view.evaluate_script_with_callback(
            "JSON.stringify({t:document.title||'',u:location.href||'',b:navigation&&navigation.canGoBack||false,f:navigation&&navigation.canGoForward||false,h:history.length||0})",
            move |result| {
                let Ok(serde_json::Value::String(inner)) =
                    serde_json::from_str::<serde_json::Value>(&result)
                else {
                    return;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&inner) else {
                    return;
                };
                if let Ok(mut s) = shared.lock() {
                    if let Some(t) = v.get("t").and_then(|x| x.as_str()) {
                        s.title = t.to_string();
                    }
                    if let Some(u) = v.get("u").and_then(|x| x.as_str()) {
                        s.url = u.to_string();
                    }
                    // Navigation API（Safari 17.4+）が使えればそちらを優先
                    if let Some(b) = v.get("b").and_then(|x| x.as_bool()) {
                        s.can_go_back = b;
                        s.can_go_forward =
                            v.get("f").and_then(|x| x.as_bool()).unwrap_or(false);
                    } else {
                        // フォールバック: history.length > 1 なら back 可能と推定
                        let h = v.get("h").and_then(|x| x.as_u64()).unwrap_or(0);
                        s.can_go_back = h > 1;
                    }
                }
            },
        );
    }

    /// ナビゲーション操作。`to` は "back" / "forward" / "reload" / URL 文字列。
    /// wry の Web ビューがまだ無いとき（#724）は [`Self::navigate_pending`]
    ///
    /// 断りは表示言語の [`HostRefusal`]（#2365。CLI / MCP の応答へは GUI の口が `?` で載せる）
    pub fn navigate(&self, to: &str) -> Result<(), HostRefusal> {
        let Some(view) = self.view.as_ref() else {
            return self.navigate_pending(to);
        };
        match to {
            "back" => view
                .evaluate_script("history.back();")
                .map_err(|e| HostRefusal::WebBackFailed(e.to_string())),
            "forward" => view
                .evaluate_script("history.forward();")
                .map_err(|e| HostRefusal::WebForwardFailed(e.to_string())),
            "reload" => {
                // エラー状態なら失敗した URL をリトライ
                if let Ok(s) = self.shared.lock() {
                    if let Some(url) = s.failed_url.clone() {
                        drop(s);
                        return self.navigate(&url);
                    }
                }
                view.reload()
                    .map_err(|e| HostRefusal::WebReloadFailed(e.to_string()))
            }
            url => {
                let url = normalize_url(url);
                validate_url(&url)?;
                let track = should_track_nav(&url);
                if let Ok(mut s) = self.shared.lock() {
                    s.url = url.clone();
                    s.loading = true;
                    s.error = None;
                    s.failed_url = None;
                    s.nav_committed = false;
                    if track {
                        s.nav_started_at = Some(std::time::Instant::now());
                        s.nav_target_url = Some(url.clone());
                    }
                }
                view.load_url(&url)
                    .map_err(|e| HostRefusal::WebLoadFailed(e.to_string()))
            }
        }
    }

    /// wry の Web ビューがまだ無いときのナビゲーション（#724）。
    ///
    /// - 保留（W4）・生成失敗: **ここで生成し直しを頼む**（生成待ちへ戻す）。URL を渡せば
    ///   その URL で、back / forward / reload なら元の URL で開く。生成タスクを起こすのは
    ///   呼び出し側（`TakoApp::wake_webview_builder`）
    /// - 生成待ち: URL なら生成する URL を差し替える（back / forward / reload は何もしない）
    /// - 生成中: 断る（作り終えてから navigate する）
    fn navigate_pending(&self, to: &str) -> Result<(), HostRefusal> {
        let phase = self.phase.get();
        let target = match to {
            "back" | "forward" | "reload" => None,
            url => {
                let url = normalize_url(url);
                validate_url(&url)?;
                Some(url)
            }
        };
        match phase {
            WebViewPhase::Building | WebViewPhase::Ready => Err(self.not_ready_error()),
            WebViewPhase::Queued | WebViewPhase::Held | WebViewPhase::Failed => {
                if let Ok(mut s) = self.shared.lock() {
                    if let Some(url) = target {
                        s.url = url;
                    }
                    s.error = None;
                    s.failed_url = None;
                    s.loading = false;
                }
                self.phase.set(WebViewPhase::Queued);
                Ok(())
            }
        }
    }

    /// JS を非同期評価する。結果は後で `take_eval_result(token)` で回収する。
    /// dispatch は UI スレッドで走るため、ここで結果を同期待ちしてはならない
    /// （wry のコールバックも UI スレッド配送 = 待つとデッドロック）
    pub fn eval(&mut self, js: &str) -> Result<u64, HostRefusal> {
        let Some(view) = self.view.as_ref() else {
            return Err(self.not_ready_error());
        };
        let token = self.next_eval_token;
        self.next_eval_token += 1;
        let shared = Arc::clone(&self.shared);
        view.evaluate_script_with_callback(js, move |result| {
            if let Ok(mut s) = shared.lock() {
                // 回収されない結果が無限に溜まらないよう上限を設ける
                if s.eval_results.len() > 64 {
                    s.eval_results.clear();
                }
                s.eval_results.insert(token, result);
            }
        })
        .map_err(|e| HostRefusal::WebEvalFailed(e.to_string()))?;
        Ok(token)
    }

    /// eval 結果を取り出す（あれば削除して返す）
    pub fn take_eval_result(&self, token: u64) -> Option<String> {
        self.shared.lock().ok()?.eval_results.remove(&token)
    }

    /// 現在の URL（ipc フックが追跡した最新値）
    pub fn current_url(&self) -> String {
        self.shared
            .lock()
            .map(|s| s.url.clone())
            .unwrap_or_default()
    }

    /// 現在のページタイトル（未取得なら空文字）
    pub fn current_title(&self) -> String {
        self.shared
            .lock()
            .map(|s| s.title.clone())
            .unwrap_or_default()
    }

    /// 「戻る」が可能か
    pub fn can_go_back(&self) -> bool {
        self.shared.lock().map(|s| s.can_go_back).unwrap_or(false)
    }

    /// 「進む」が可能か
    pub fn can_go_forward(&self) -> bool {
        self.shared
            .lock()
            .map(|s| s.can_go_forward)
            .unwrap_or(false)
    }

    /// ページ読み込み中か
    pub fn is_loading(&self) -> bool {
        self.shared.lock().map(|s| s.loading).unwrap_or(false)
    }

    /// target=_blank / window.open で溜まった新規ウィンドウ URL をドレインする
    pub fn drain_new_window_urls(&self) -> Vec<String> {
        self.shared
            .lock()
            .ok()
            .map(|mut s| std::mem::take(&mut s.pending_new_window_urls))
            .unwrap_or_default()
    }

    /// 直近の生成の失敗の種別（#2431。None = 失敗していない・Windows 以外）
    pub fn create_failure(&self) -> Option<&'static str> {
        self.shared.lock().ok()?.create_failure
    }

    /// エラー状態を返す（None = 正常）
    pub fn error_state(&self) -> Option<(String, String)> {
        let s = self.shared.lock().ok()?;
        let error = s.error.as_ref()?.clone();
        let url = s.failed_url.as_ref().cloned().unwrap_or_default();
        Some((error, url))
    }

    /// ナビゲーションのタイムアウトを検査する。
    /// wry は WKWebView の didFailProvisionalNavigation: を公開しないため、
    /// 「Started（didCommitNavigation）が一定時間来ない」ことで失敗を推定する。
    /// 状態が変わった（エラー検知した）場合に true を返す
    pub fn check_nav_timeout(&self) -> bool {
        if let Ok(mut s) = self.shared.lock() {
            if s.error.is_some() {
                return false;
            }
            if let Some(started) = s.nav_started_at {
                let elapsed = started.elapsed();
                if !s.nav_committed && elapsed > std::time::Duration::from_secs(NAV_TIMEOUT_SECS) {
                    let target = s.nav_target_url.clone().unwrap_or_else(|| s.url.clone());
                    s.error = Some(crate::ui_text::webdock::load_failed().to_string());
                    s.failed_url = Some(target);
                    s.loading = false;
                    s.nav_started_at = None;
                    s.nav_target_url = None;
                    return true;
                }
            }
        }
        false
    }

    /// エラー状態をクリアして失敗した URL を再読み込みする
    pub fn retry(&self) -> Result<(), String> {
        let url = {
            let s = self.shared.lock().map_err(|_| "lock error".to_string())?;
            s.failed_url.clone().unwrap_or_else(|| s.url.clone())
        };
        Ok(self.navigate(&url)?)
    }
}

// ---------------------------------------------------------------------------
// macOS: NSEvent local monitor（#326）
//
// WKWebView が first responder のとき、tako のグローバルショートカット（⌘K 等）が
// webview に飲まれて GPUI に届かない問題を解決する。
// addLocalMonitorForEventsMatchingMask: でキーダウンを先取りし、tako が処理すべき
// ⌘ ショートカットなら first responder を content view（GPUI）へ戻してから
// イベントを通す。webview 用の編集系（⌘C/⌘V 等）はそのまま webview へ渡す。
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod key_monitor {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    extern "C" {
        fn objc_getClass(name: *const u8) -> *const c_void;
        fn sel_registerName(name: *const u8) -> *const c_void;
        fn objc_msgSend(receiver: *const c_void, selector: *const c_void, ...) -> *const c_void;
        fn object_getClassName(obj: *const c_void) -> *const std::ffi::c_char;
        static _NSConcreteGlobalBlock: c_void;
    }

    fn cls(name: &str) -> *const c_void {
        let cstr = std::ffi::CString::new(name).unwrap();
        unsafe { objc_getClass(cstr.as_ptr() as *const u8) }
    }
    fn sel(name: &str) -> *const c_void {
        let cstr = std::ffi::CString::new(name).unwrap();
        unsafe { sel_registerName(cstr.as_ptr() as *const u8) }
    }

    // Objective-C global block（キャプチャなし）の ABI レイアウト
    #[repr(C)]
    struct GlobalBlock {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*const GlobalBlock, *const c_void) -> *const c_void,
        descriptor: *const BlockDescriptor,
    }
    unsafe impl Sync for GlobalBlock {}
    unsafe impl Send for GlobalBlock {}

    #[repr(C)]
    struct BlockDescriptor {
        reserved: u64,
        size: u64,
    }

    static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
        reserved: 0,
        size: std::mem::size_of::<GlobalBlock>() as u64,
    };

    // webview が存在しない ⌘ キーイベントまで first responder を切り替えると
    // 通常操作に副作用が出るため、webview が 1 個以上ある場合のみ動作させる
    static HAS_WEBVIEW: AtomicBool = AtomicBool::new(false);

    /// webview が存在するか。main.rs 側の webview 作成/破棄で呼ぶ
    pub(super) fn set_has_webview(v: bool) {
        HAS_WEBVIEW.store(v, Ordering::Relaxed);
    }

    const NS_COMMAND_KEY_MASK: u64 = 1 << 20;

    /// webview で処理させるキー（⌘C/⌘V 等）の virtual key code 集合。
    /// これに含まれるキーは first responder を切り替えず webview にそのまま渡す
    const WEBVIEW_PASSTHROUGH_KEYS: &[u16] = &[
        0x00, // A  (⌘A select all)
        0x08, // C  (⌘C copy)
        0x03, // F  (⌘F find)
        0x25, // L  (⌘L address bar)
        0x23, // P  (⌘P print)
        0x0F, // R  (⌘R reload)
        0x01, // S  (⌘S save)
        0x09, // V  (⌘V paste)
        0x07, // X  (⌘X cut)
        0x06, // Z  (⌘Z undo / ⌘Shift+Z redo)
    ];

    /// NSEvent の virtual key code が webview 用かどうか
    fn is_webview_key(key_code: u16, _flags: u64) -> bool {
        WEBVIEW_PASSTHROUGH_KEYS.contains(&key_code)
    }

    // ── キー入力の宛先（first responder）を読む・戻す 1 実装（#326 / #1481） ──
    //
    // 「Web ビューがキー入力を持っているか」は**状態ではなく AppKit の実体**で答える
    // （#503 の教訓 = フラグは必ずどこかで降ろし忘れる）。
    //
    // 戻す先は `contentView` では**ない**（#326 の取り違え）。gpui は自前の描画ビューを
    // ウィンドウの `contentView` の**サブビュー**として足し、そこへ
    // `makeFirstResponder:` する（gpui_macos/src/window.rs の
    // `content_view.addSubview_(native_view)` → `makeFirstResponder_(native_view)`）。
    // つまり `keyDown:` を実装しているのはサブビューの方で、`contentView` は別物
    // （実測 = `AccessKitSubclassOfNSView`）。`makeFirstResponder:contentView` は
    // **成功する**（true が返る）のに打鍵は tako へ来ない = 直っていないのに直ったように
    // 見える形。#326 で ⌘K が効いていたのは `performKeyEquivalent:`（first responder に
    // 関係なくビュー木を降りる）経路のため。
    // wry は `build_as_child` で渡した親ビュー（= その描画ビュー）の子として WKWebView を
    // 足すので、**宛先を持っている WKWebView の superview** が常に正しい宛先になる

    /// NSView の所属 NSWindow（無ければ null）
    unsafe fn window_of_view(view: *const c_void) -> *const c_void {
        if view.is_null() {
            return std::ptr::null();
        }
        let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
            std::mem::transmute(objc_msgSend as *const c_void);
        f_id(view, sel("window"))
    }

    /// オブジェクトのクラス名（診断用。読めなければ空文字）
    unsafe fn class_name(obj: *const c_void) -> String {
        if obj.is_null() {
            return String::new();
        }
        let p = object_getClassName(obj);
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }

    /// window のキー入力の宛先が Web ビュー側なら、その WKWebView を返す。
    /// first responder は WKWebView 本体でも内側のサブビュー（WKContentView 等）でも
    /// ありうるので 2 段で見る
    unsafe fn webview_holding_focus(window: *const c_void) -> Option<*const c_void> {
        if window.is_null() {
            return None;
        }
        let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
            std::mem::transmute(objc_msgSend as *const c_void);
        let f_bool: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> bool =
            std::mem::transmute(objc_msgSend as *const c_void);
        let first_resp = f_id(window, sel("firstResponder"));
        if first_resp.is_null() {
            return None;
        }
        let wk_cls = cls("WKWebView");
        if wk_cls.is_null() {
            return None;
        }
        let ik_sel = sel("isKindOfClass:");
        if f_bool(first_resp, ik_sel, wk_cls) {
            return Some(first_resp);
        }
        // first responder は NSView とは限らない（宛先が無いと **NSWindow 自身**になる）。
        // NSView でないものへ `superview` を送ると unrecognized selector の例外で落ちる（#1837）
        let view_cls = cls("NSView");
        if view_cls.is_null() || !f_bool(first_resp, ik_sel, view_cls) {
            return None;
        }
        let parent = f_id(first_resp, sel("superview"));
        if !parent.is_null() && f_bool(parent, ik_sel, wk_cls) {
            return Some(parent);
        }
        None
    }

    /// キー入力の宛先が Web ビュー側なら、GPUI の描画ビューへ戻す。戻したら true。
    /// `preferred` が非 null ならそれを宛先にする（フレーム同期は自ウィンドウの
    /// 描画ビューを知っている）。null なら実体から引く（monitor は window だけ持つ）
    unsafe fn restore_from_window(window: *const c_void, preferred: *const c_void) -> bool {
        let Some(wk) = webview_holding_focus(window) else {
            return false;
        };
        let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
            std::mem::transmute(objc_msgSend as *const c_void);
        let f_mk: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> bool =
            std::mem::transmute(objc_msgSend as *const c_void);
        let target = if preferred.is_null() {
            f_id(wk, sel("superview"))
        } else {
            preferred
        };
        if target.is_null() {
            return false;
        }
        f_mk(window, sel("makeFirstResponder:"), target)
    }

    /// キー入力の宛先が Web ビュー側か（`view` = gpui の描画ビュー）
    pub(super) fn holds_key_focus(view: *const c_void) -> bool {
        unsafe { webview_holding_focus(window_of_view(view)).is_some() }
    }

    /// キー入力の宛先の素性（診断用）。`クラス名@アドレス` の形で、
    /// **アドレスまで出すのは「宛先が別のオブジェクトへ移ったか」を言い分けるため**
    /// （Web ビューが 2 枚あるとき、クラス名はどちらも `WryWebView` で区別できない）
    pub(super) fn owner_class(view: *const c_void) -> String {
        unsafe {
            let window = window_of_view(view);
            if window.is_null() {
                return String::new();
            }
            let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            let first_resp = f_id(window, sel("firstResponder"));
            format!("{}@{:p}", class_name(first_resp), first_resp)
        }
    }

    /// キー入力の宛先を `view`（gpui の描画ビュー）へ戻す。戻したら true
    pub(super) fn return_to_app(view: *const c_void) -> bool {
        unsafe { restore_from_window(window_of_view(view), view) }
    }

    // ── wry の親（gpui の描画ビュー）の生存と付け替え（#1837） ──

    /// `retain` / `release` を送る（null なら何もしない）
    fn send_void(obj: *const c_void, selector: &str) {
        if obj.is_null() {
            return;
        }
        unsafe {
            let f: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(obj, sel(selector));
        }
    }

    /// 親ビューを tako も持つ（窓が閉じても解放済みのポインタにしない）
    pub(super) fn retain_view(view: *const c_void) {
        send_void(view, "retain");
    }

    /// [`retain_view`] の対
    pub(super) fn release_view(view: *const c_void) {
        send_void(view, "release");
    }

    /// NSView がいま窓に載っているか（`window` が nil でない）
    pub(super) fn view_has_window(view: *const c_void) -> bool {
        unsafe { !window_of_view(view).is_null() }
    }

    /// `view` の superview を、その窓のキー入力の宛先にする（#1837）。宛先にできたら true
    pub(super) fn focus_superview(view: *const c_void) -> bool {
        unsafe {
            let window = window_of_view(view);
            if window.is_null() {
                return false;
            }
            let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            let f_mk: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> bool =
                std::mem::transmute(objc_msgSend as *const c_void);
            let parent = f_id(view, sel("superview"));
            if parent.is_null() {
                return false;
            }
            f_mk(window, sel("makeFirstResponder:"), parent)
        }
    }

    /// WKWebView の要素の全画面表示を入れる / 切る（#2445）。設定は WKWebView の
    /// `configuration.preferences`（複製の設定もページと同じ WebPreferences を指すので、生成後に
    /// 書いてもそのページへ効く）。macOS 12.3 以降は公開 API の `elementFullscreenEnabled`、
    /// それより前は wry が入れるのと同じ非公開の KVC 鍵 `fullScreenEnabled`。送れたら true
    pub(super) fn set_element_fullscreen(webview: *const c_void, enabled: bool) -> bool {
        if webview.is_null() {
            return false;
        }
        unsafe {
            let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            let config = f_id(webview, sel("configuration"));
            if config.is_null() {
                return false;
            }
            let prefs = f_id(config, sel("preferences"));
            if prefs.is_null() {
                return false;
            }
            let setter = sel("setElementFullscreenEnabled:");
            let f_responds: unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const c_void,
            ) -> bool = std::mem::transmute(objc_msgSend as *const c_void);
            if f_responds(prefs, sel("respondsToSelector:"), setter) {
                let f_set: unsafe extern "C" fn(*const c_void, *const c_void, bool) =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f_set(prefs, setter, enabled);
                return true;
            }
            let f_num: unsafe extern "C" fn(*const c_void, *const c_void, bool) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            let f_str: unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const std::ffi::c_char,
            ) -> *const c_void = std::mem::transmute(objc_msgSend as *const c_void);
            let f_kv: unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const c_void,
                *const c_void,
            ) = std::mem::transmute(objc_msgSend as *const c_void);
            let value = f_num(cls("NSNumber"), sel("numberWithBool:"), enabled);
            let key = f_str(
                cls("NSString"),
                sel("stringWithUTF8String:"),
                c"fullScreenEnabled".as_ptr(),
            );
            if value.is_null() || key.is_null() {
                return false;
            }
            f_kv(prefs, sel("setValue:forKey:"), value, key);
            true
        }
    }

    /// `child` を `parent` の子へ移す（AppKit が古い親から外す）。送ったら true
    pub(super) fn add_subview(parent: *const c_void, child: *const c_void) -> bool {
        if parent.is_null() || child.is_null() {
            return false;
        }
        unsafe {
            let f: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(parent, sel("addSubview:"), child);
        }
        true
    }

    /// NSEvent local monitor のコールバック。
    /// キー入力の宛先が Web ビューで、tako が処理すべき ⌘ キーなら、宛先を
    /// GPUI の描画ビューへ戻す（#1481 の 1 実装）。イベント自体は常にそのまま返す
    unsafe extern "C" fn monitor_invoke(
        _block: *const GlobalBlock,
        event: *const c_void,
    ) -> *const c_void {
        if !HAS_WEBVIEW.load(Ordering::Relaxed) {
            return event;
        }
        // modifier flags を取得
        let flags_sel = sel("modifierFlags");
        let f: unsafe extern "C" fn(*const c_void, *const c_void) -> u64 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let flags = f(event, flags_sel);
        if flags & NS_COMMAND_KEY_MASK == 0 {
            return event;
        }
        // key code を取得
        let kc_sel = sel("keyCode");
        let f_kc: unsafe extern "C" fn(*const c_void, *const c_void) -> u16 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let key_code = f_kc(event, kc_sel);

        if is_webview_key(key_code, flags) {
            return event;
        }
        // NSApp.keyWindow
        let nsapp_cls = cls("NSApplication");
        let f_id: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
            std::mem::transmute(objc_msgSend as *const c_void);
        let nsapp = f_id(nsapp_cls, sel("sharedApplication"));
        if nsapp.is_null() {
            return event;
        }
        let window = f_id(nsapp, sel("keyWindow"));
        if window.is_null() {
            return event;
        }
        // キー入力の宛先を GPUI の描画ビューへ戻す（判定・復帰は #1481 の 1 実装）
        restore_from_window(window, std::ptr::null());
        event
    }

    static MONITOR_BLOCK: GlobalBlock = GlobalBlock {
        isa: unsafe { &_NSConcreteGlobalBlock as *const c_void },
        flags: 1 << 28, // BLOCK_IS_GLOBAL
        reserved: 0,
        invoke: monitor_invoke,
        descriptor: &DESCRIPTOR,
    };

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    /// NSEvent local monitor を設置する。二重登録防止済み。アプリ起動時に 1 回呼ぶ
    pub(super) fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let ns_event = cls("NSEvent");
            let mask: u64 = 1 << 10; // NSEventMaskKeyDown
            let add_sel = sel("addLocalMonitorForEventsMatchingMask:handler:");
            let f: unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                u64,
                *const c_void,
            ) -> *const c_void = std::mem::transmute(objc_msgSend as *const c_void);
            let _monitor = f(
                ns_event,
                add_sel,
                mask,
                &MONITOR_BLOCK as *const _ as *const c_void,
            );
            // monitor オブジェクトはアプリ終了まで保持（リーク許容。removeMonitor: は不要）
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod key_monitor {
    use std::ffi::c_void;

    pub(super) fn install() {}
    pub(super) fn set_has_webview(_: bool) {}
    /// Windows（WebView2）のキー入力の宛先は #521 で扱う。ここでは「Web ビューが
    /// 奪っていない」ことにして何もしない（誤って毎フレーム触らないため）
    pub(super) fn holds_key_focus(_view: *const c_void) -> bool {
        false
    }
    pub(super) fn owner_class(_view: *const c_void) -> String {
        String::new()
    }
    pub(super) fn return_to_app(_view: *const c_void) -> bool {
        false
    }
    /// 親ビューの retain は macOS だけ（#1837。Windows の親は HWND で、生存は IsWindow で見る）
    pub(super) fn retain_view(_view: *const c_void) {}
    pub(super) fn release_view(_view: *const c_void) {}
    pub(super) fn view_has_window(_view: *const c_void) -> bool {
        true
    }
}

/// NSEvent local monitor を設置する（macOS のみ。#326）。
/// アプリ起動時に 1 回呼ぶ。webview が 1 個以上存在する間だけ有効
pub fn install_key_monitor() {
    key_monitor::install();
}

/// webview の存在状態を更新する。webview 作成/全破棄のたびに呼ぶ
pub fn set_has_webview(v: bool) {
    key_monitor::set_has_webview(v);
}

/// キー入力の宛先（macOS では NSWindow の first responder）が Web ビュー側か（#1481）。
///
/// **状態ではなく AppKit の実体を読む**。「さっき Web ビューを触ったか」という印を
/// 持つと必ずどこかで降ろし忘れる（#503）ので、問うたその場で実体へ聞く
pub fn webview_holds_key_focus(handle: &WindowHandleBox) -> bool {
    key_monitor::holds_key_focus(handle.ns_view_ptr())
}

/// キー入力の宛先の素性（診断用。`クラス名@アドレス`。gpui の描画ビュー = tako に届く /
/// `WKWebView` 系 = ページに奪われている。アドレスは 2 枚のどちらかを言い分けるため）
pub fn key_focus_owner(handle: &WindowHandleBox) -> String {
    key_monitor::owner_class(handle.ns_view_ptr())
}

/// キー入力の宛先を GPUI の描画ビューへ戻す。実際に戻したら true（#1481）
pub fn return_key_focus_to_app(handle: &WindowHandleBox) -> bool {
    key_monitor::return_to_app(handle.ns_view_ptr())
}

/// ナビゲーションのタイムアウト秒数。
/// DNS 失敗（macOS 既定: 約 5〜10 秒）+ マージンを見込んだ値
const NAV_TIMEOUT_SECS: u64 = 10;

/// タイムアウト追跡が必要な URL か。data: / about: は即座に読み込まれるため不要
fn should_track_nav(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
}

/// スキーム無しの入力を URL に正規化する（アドレスバー入力・CLI の省略記法）。
/// ドットも空白も無い単語はそのまま https 扱いにせず検索はしない（ローカル開発が主用途）
pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("file://")
        || s.starts_with("data:")
        || s.starts_with("about:")
    {
        return s.to_string();
    }
    // localhost:3000 / 127.0.0.1:8080 のような開発サーバー指定は http に倒す
    if s.starts_with("localhost") || s.starts_with("127.0.0.1") || s.starts_with("0.0.0.0") {
        return format!("http://{s}");
    }
    format!("https://{s}")
}

/// 正規化済み URL を wry へ渡す前に検証する。
/// NSURL(string:) が nil を返す文字列（空白・制御文字を含む等）を弾き、
/// wry 内部の unwrap による panic（#334）を防ぐ
pub fn validate_url(url: &str) -> Result<(), HostRefusal> {
    if url.is_empty() {
        return Err(HostRefusal::UrlEmpty);
    }
    if url.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(HostRefusal::UrlHasWhitespace(url.to_string()));
    }
    if url.bytes().any(|b| b < 0x20) {
        return Err(HostRefusal::UrlHasControlChar(url.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_はスキーム付きをそのまま返す() {
        assert_eq!(normalize_url("https://example.com"), "https://example.com");
        assert_eq!(
            normalize_url("http://localhost:3000"),
            "http://localhost:3000"
        );
        assert_eq!(normalize_url("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }

    #[test]
    fn normalize_url_はローカル開発ホストを_http_に倒す() {
        assert_eq!(normalize_url("localhost:5173"), "http://localhost:5173");
        assert_eq!(normalize_url("127.0.0.1:8080"), "http://127.0.0.1:8080");
    }

    #[test]
    fn normalize_url_は裸ドメインを_https_に倒す() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("  docs.rs/wry "), "https://docs.rs/wry");
    }

    #[test]
    fn validate_url_は正常な_url_を通す() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://localhost:3000").is_ok());
        assert!(validate_url("data:text/html,hello").is_ok());
        assert!(validate_url("about:blank").is_ok());
        assert!(validate_url("https://example.com/path?q=1&r=2#frag").is_ok());
    }

    #[test]
    fn validate_url_は空白入り_url_を拒否する() {
        assert!(validate_url("https://github .com").is_err());
        assert!(validate_url("a b").is_err());
        assert!(validate_url("hello world").is_err());
        assert!(validate_url("https://example.com/path with spaces").is_err());
    }

    #[test]
    fn validate_url_は空文字を拒否する() {
        assert!(validate_url("").is_err());
    }

    #[test]
    fn validate_url_はスキーム無しのドメインを通す() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://127.0.0.1:8080").is_ok());
    }

    #[test]
    fn validate_url_は制御文字を拒否する() {
        assert!(validate_url("https://example.com/\x01bad").is_err());
        assert!(validate_url("https://\x00evil.com").is_err());
    }

    #[test]
    fn validate_url_は非_ascii_を許容する() {
        assert!(validate_url("https://xn--example.com/%E6%97%A5%E6%9C%AC%E8%AA%9E").is_ok());
    }

    #[test]
    fn validate_url_はタブや改行を拒否する() {
        assert!(validate_url("https://example.com/\there").is_err());
        assert!(validate_url("https://example.com/\nhere").is_err());
    }

    #[test]
    fn should_track_nav_はリモート_url_のみ追跡する() {
        assert!(should_track_nav("https://example.com"));
        assert!(should_track_nav("http://localhost:3000"));
        assert!(should_track_nav("file:///tmp/test.html"));
        assert!(!should_track_nav("data:text/html,hello"));
        assert!(!should_track_nav("about:blank"));
    }

    #[test]
    fn web_shared_のデフォルトはエラーなし() {
        let s = WebShared::default();
        assert!(s.error.is_none());
        assert!(s.failed_url.is_none());
        assert!(!s.nav_committed);
        assert!(s.nav_started_at.is_none());
    }

    #[test]
    fn web_shared_のデフォルトはナビ状態が初期値() {
        let s = WebShared::default();
        assert!(!s.can_go_back);
        assert!(!s.can_go_forward);
        assert!(s.pending_new_window_urls.is_empty());
    }

    /// Cargo.lock に載っている wry の版（#724 / #2445 の番犬が根拠を版に結び付ける）
    fn wry_version() -> String {
        lock_version("wry")
    }

    /// Cargo.lock に載っている crate の版（同じ名前が 2 版あれば先の方）
    fn lock_version(name: &str) -> String {
        let head = format!("\nname = \"{name}\"\n");
        include_str!("../../../Cargo.lock")
            .split("[[package]]")
            .find(|p| p.contains(&head))
            .and_then(|p| p.lines().find_map(|l| l.strip_prefix("version = \"")))
            .map(|v| v.trim_end_matches('"').to_string())
            .unwrap_or_else(|| panic!("Cargo.lock に {name} が無い"))
    }

    /// registry にある wry の実ソース（`rel` は crate 内の相対パス）。取得していない機では None
    fn wry_source(ver: &str, rel: &str) -> Option<String> {
        crate_source("wry", ver, rel)
    }

    /// registry にある crate の実ソース。取得していない機（Windows 専用の crate を macOS で）では None
    fn crate_source(name: &str, ver: &str, rel: &str) -> Option<String> {
        // cargo の置き場（`CARGO_HOME`、無ければホームの `.cargo`。ホームは paths の 1 実装 = #893）
        let home = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| tako_core::paths::home_dir().map(|h| h.join(".cargo")))?;
        let path = std::fs::read_dir(home.join("registry").join("src"))
            .ok()?
            .flatten()
            .map(|d| d.path().join(format!("{name}-{ver}")).join(rel))
            .find(|p| p.is_file())?;
        std::fs::read_to_string(path).ok()
    }

    /// `src` の中の関数 `sig` の本文（次の同じ深さの `fn` の手前まで）
    fn fn_body<'a>(src: &'a str, sig: &str) -> &'a str {
        let start = src
            .find(sig)
            .unwrap_or_else(|| panic!("{sig} が見つからない"));
        let rest = &src[start + sig.len()..];
        let end = [
            "\n    pub fn ",
            "\n    fn ",
            "\n    pub(crate) fn ",
            "\n    pub(super) fn ",
            "\n    unsafe fn ",
        ]
        .iter()
        .filter_map(|next| rest.find(next))
        .min()
        .unwrap_or(rest.len());
        &rest[..end]
    }

    /// 本文の中で `guard` が `call` より前に出るか（どちらも無ければ偽）
    fn guarded(body: &str, guard: &str, call: &str) -> bool {
        matches!((body.find(guard), body.find(call)), (Some(g), Some(c)) if g < c)
    }

    /// **wry の `window().unwrap()` に届く前に、親・自分が窓に載っているかを見る**（#1837）。
    ///
    /// macOS の wry は生成（wry-0.57.1 wkwebview/mod.rs:434 / 663）・位置合わせ（:1104）・focus（:1130）で
    /// 親や WKWebView の NSWindow を `unwrap()` で引くので、窓から外れたまま呼ぶと panic で
    /// tako 全体が落ちる（セルフテスト項目 79 → 155 の実測）。#334 と同じく、tako 側で
    /// 先に断る形を固定する。どれかの検査を消すと落ちる
    #[test]
    fn issue1837_wryのunwrapに届く前に窓に載っているかを見る() {
        let src = include_str!("webview.rs");
        // #724: wry を呼ぶのは `build_view` の 1 か所（同期の `build` も生成タスクの `BuildJob::build` も通る）
        let build = fn_body(src, "fn build_view(");
        assert!(
            guarded(build, "window.attached()", ".build_as_child("),
            "WebViewEntry::build_view が親の生存を見ずに wry へ渡している（#1837）"
        );
        let sync = fn_body(src, "pub fn sync_frame(");
        assert!(
            guarded(sync, "self.in_window()", "view.set_bounds("),
            "sync_frame が窓に載っているかを見ずに set_bounds を呼んでいる（#1837）"
        );
        let focus = fn_body(src, "pub fn focus(&self)");
        assert!(
            guarded(focus, "self.in_window()", "view.focus()"),
            "focus が窓に載っているかを見ずに wry の focus を呼んでいる（#1837）"
        );
        // 隠すときの宛先の戻しは wry の focus_parent（生成時の親）ではなく、いまの superview へ
        assert!(
            !sync.contains("view.focus_parent()"),
            "sync_frame が wry の focus_parent を直に呼んでいる。付け替えた後は生成時の親が\
             窓に居ないので NSWindow 自身が宛先になる（#1837）。self.focus_parent() を通すこと"
        );
        // first responder へ superview を送る前に NSView かを見る
        let holding = fn_body(src, "unsafe fn webview_holding_focus(");
        assert!(
            guarded(holding, "cls(\"NSView\")", "sel(\"superview\")"),
            "webview_holding_focus が NSView か確かめずに first responder へ superview を送っている。\
             宛先が NSWindow 自身のとき unrecognized selector の例外で落ちる（#1837）"
        );
        // 呼び出し側は wry の focus / set_bounds を直に呼ばない（上の検査を素通りする）
        let main = include_str!("main.rs");
        for direct in [".view.focus()", ".view.set_bounds(", ".view.focus_parent()"] {
            assert!(
                !main.contains(direct),
                "main.rs が `{direct}` を直に呼んでいる。WebViewEntry の focus / sync_frame を通すこと（#1837）"
            );
        }
    }

    /// 親が閉じているときの理由は CLI / MCP の応答と persist.log で同じ 1 文（#1837）
    #[test]
    fn issue1837_親が閉じているときの理由は何をすればよいかを言う() {
        let reason = detached_parent_error();
        assert!(reason.contains("Web ビューの親"));
        assert!(reason.contains("窓を開いて"));
        assert!(reason.contains("#1837"));
    }

    /// `src` の `offset` が何行目か（1 始まり。番犬の名指し用）
    fn line_of(src: &str, offset: usize) -> usize {
        src[..offset].matches('\n').count() + 1
    }

    /// `offset` を含む impl のメソッド（4 字下げの `fn`）の名前。関数の頭の判定は
    /// 共有ヘルパ（#1496。`pub(crate) fn` / `async fn` も頭と見なす）
    fn enclosing_method(src: &str, offset: usize) -> Option<&str> {
        src[..offset]
            .lines()
            .filter(|line| line.starts_with("    ") && !line[4..].starts_with(' '))
            .filter_map(tako_core::source_scan::fn_head_name)
            .next_back()
    }

    /// `open`（`(` の位置）に対応する `)` の位置。文字列リテラルと行コメントは飛ばす
    fn matching_paren(src: &str, open: usize) -> Option<usize> {
        let b = src.as_bytes();
        let mut depth = 0i32;
        let mut i = open;
        while i < b.len() {
            match b[i] {
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// **Windows の Web ビューの生成は gpui の App の借用の外で行う**（#724 の W1）。
    ///
    /// 借用の内側（dispatch・クリックハンドラ・render）で wry を呼ぶと、WebView2 の入れ子の
    /// メッセージループが gpui の前景タスクを走らせ、`update_entity` の `borrow_mut()` が
    /// 二重借用の panic → 窓プロシージャを跨いで abort する（#724 の実機の panic.log）。
    /// 固定する形:
    /// ① main.rs の同期の生成（`WebViewEntry::build(`）は `create_webview` の中で、
    ///   `async_build_enabled()` の早期 return の**後**にしか無い
    /// ② 生成タスクの wry 呼び出し（`job.build()`）は `spawn_webview_builder` の中で、
    ///   どの `.update(` の引数（= 借用の内側）にも入っていない
    /// ③ main.rs は wry を直に呼ばない（`build_view` / `WebViewBuilder` / `build_as_child`）
    /// ④ Windows は既定で非同期の経路（A/B の env が無ければ）
    #[test]
    fn issue724_webviewの生成をappの借用の外で行う() {
        let main = include_str!("main.rs");
        let here = "crates/tako-app/src/main.rs";
        let mut bad = Vec::new();
        // ①
        for (at, _) in main.match_indices("WebViewEntry::build(") {
            let method = enclosing_method(main, at);
            let ok = method == Some("create_webview") && {
                let sig = "fn create_webview(";
                let body = fn_body(main, sig);
                // fn_body はシグネチャの直後から始まる
                let rel = at.saturating_sub(main.find(sig).unwrap_or(0) + sig.len());
                let gate = body.find("if webview::async_build_enabled() {");
                let ret = gate.and_then(|g| body[g..].find("return Ok(id);").map(|r| g + r));
                matches!(ret, Some(r) if r < rel)
            };
            if !ok {
                bad.push(format!(
                    "{here}:{} 同期の生成が非同期の経路の早期 return の外にある（{}）",
                    line_of(main, at),
                    method.unwrap_or("?")
                ));
            }
        }
        // ②
        let calls: Vec<usize> = main.match_indices("job.build()").map(|(i, _)| i).collect();
        if calls.is_empty() {
            bad.push(format!("{here}: 生成タスクの job.build() が見つからない"));
        }
        for at in calls {
            let method = enclosing_method(main, at);
            if method != Some("spawn_webview_builder") {
                bad.push(format!(
                    "{here}:{} job.build() が生成タスクの外（{}）にある",
                    line_of(main, at),
                    method.unwrap_or("?")
                ));
                continue;
            }
            let start = main.find("fn spawn_webview_builder(").unwrap_or(0);
            for (u, _) in main[start..at].match_indices(".update(") {
                let open = start + u + ".update".len();
                if matching_paren(main, open).is_none_or(|close| close > at) {
                    bad.push(format!(
                        "{here}:{} job.build() が {here}:{} の .update( の内側 = App の借用の中にある",
                        line_of(main, at),
                        line_of(main, open)
                    ));
                }
            }
        }
        // ③
        for direct in ["build_view(", "wry::WebViewBuilder", "build_as_child("] {
            for (at, _) in main.match_indices(direct) {
                bad.push(format!(
                    "{here}:{} main.rs が wry の生成（{direct}）を直に呼んでいる",
                    line_of(main, at)
                ));
            }
        }
        // webview.rs の側: wry の生成は build_view の 1 か所（テストの中の文字列は数えない）
        let src = include_str!("webview.rs");
        let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap_or(src.len())];
        let builds: Vec<usize> = code
            .match_indices(".build_as_child(")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            builds.len(),
            1,
            "wry の build_as_child は build_view の 1 か所だけ"
        );
        assert_eq!(
            enclosing_method(src, builds[0]),
            Some("build_view"),
            "crates/tako-app/src/webview.rs:{} build_as_child が build_view の外にある",
            line_of(src, builds[0])
        );
        assert!(
            bad.is_empty(),
            "#724 の W1 が崩れている:\n{}",
            bad.join("\n")
        );
        // ④
        assert!(
            async_build_decision(true, false, false),
            "Windows は既定で非同期"
        );
        assert!(
            !async_build_decision(false, false, false),
            "macOS は既定で同期"
        );
        assert!(
            !async_build_decision(true, true, false),
            "TAKO_724_LEGACY=1 は修正前"
        );
        assert!(
            async_build_decision(false, false, true),
            "TAKO_724_ASYNC=1 は macOS でも非同期"
        );
        assert!(!async_build_decision(false, true, true), "LEGACY が勝つ");
    }

    /// 番犬そのものに検出力がある（#724）。修正前の形を写しへ注入すると名指しで落ちる
    #[test]
    fn issue724_番犬は借用の内側の生成を名指しする() {
        // 借用の内側で job.build() を呼ぶ写し
        let injected = "impl X {\n    fn spawn_webview_builder(cx: C) {\n        \
                        let built = this.update(cx, |app, _| app.take().map(|job| job.build()));\n    }\n}\n";
        let at = injected.find("job.build()").unwrap();
        assert_eq!(
            enclosing_method(injected, at),
            Some("spawn_webview_builder")
        );
        let open = injected.find(".update(").unwrap() + ".update".len();
        let close = matching_paren(injected, open).unwrap();
        assert!(close > at, "update の引数の内側にあると読めること");
        assert_eq!(line_of(injected, at), 3);
        // 外にある形は内側と読まない
        let fixed = "    fn spawn_webview_builder() {\n        let job = this.update(cx, |a, _| a.take());\n        \
                     let built = job.build();\n    }\n";
        let at = fixed.find("job.build()").unwrap();
        let open = fixed.find(".update(").unwrap() + ".update".len();
        assert!(matching_paren(fixed, open).unwrap() < at);
        // 文字列の中の括弧に惑わされない
        let quoted = r#"x.update(cx, |_, _| println!("(")); job.build()"#;
        let open = quoted.find(".update(").unwrap() + ".update".len();
        assert!(matching_paren(quoted, open).unwrap() < quoted.find("job.build()").unwrap());
    }

    /// **Windows では ipc ハンドラを付けない**（#724 の W2）。
    ///
    /// wry 0.55 の WebView2 は ipc ハンドラが付いていると、COM のコールバックの中で送り主の文書の
    /// URI を `http::Uri` へ unwrap し（`src/webview2/mod.rs:910`）、data: / about:blank で abort した。
    /// 0.57（#2445）は ①ハンドラが無ければ登録そのものをせず ②付いていても不正な URI のメッセージは
    /// `match` で捨てる（wry#1772）。ここでは ①判断 ②`build_view` が判断の内側でだけ付けること
    /// ③ Cargo.lock の wry が 0.57 系であること（版が変わったら止まる）と、registry に wry の実ソースが
    /// あれば上の 2 点がその版のソースで成り立つことを固定する
    #[test]
    fn issue724_windowsではipcハンドラを付けない() {
        // ①
        assert!(!ipc_hook_decision(true, false), "Windows は ipc を付けない");
        assert!(ipc_hook_decision(false, false), "macOS は従来どおり付ける");
        assert!(ipc_hook_decision(true, true), "TAKO_724_LEGACY=1 は修正前");
        // ②
        let src = include_str!("webview.rs");
        let body = fn_body(src, "fn build_view(");
        for call in [
            ".with_ipc_handler(",
            ".with_initialization_script(STATE_HOOK_JS)",
        ] {
            assert!(
                guarded(body, "if ipc_hook_enabled() {", call),
                "build_view が ipc_hook_enabled() の外で {call} を付けている（#724 の W2）"
            );
            assert_eq!(
                body.matches(call).count(),
                1,
                "{call} は判断の内側の 1 か所だけ"
            );
        }
        // ③ 根拠は wry の版に結び付いている。版が変わったら読み直す（#2445 で 0.55 → 0.57）
        let ver = wry_version();
        assert!(
            ver.starts_with("0.57."),
            "wry が {ver} になった。#724 の W2（Windows で ipc を付けない）の根拠は 0.57 の \
             src/webview2/mod.rs（ハンドラが無ければ登録しない・不正な URI のメッセージは捨てる）。\
             新しい版の ipc の扱いを読み直して、この検査と ipc_hook_decision を合わせる"
        );
        let Some(wry) = wry_source(&ver, "src/webview2/mod.rs") else {
            println!("TAKO_724: wry {ver} の実ソースが registry に無いので ③ は見送る");
            return;
        };
        // ハンドラが無ければ登録そのものをしない（付けない Windows では COM のコールバックが無い）
        let cond = wry
            .find("if let Some(ipc_handler) = attributes.ipc_handler.take() {")
            .expect("wry の ipc ハンドラの登録の条件が無い（形が変わった）");
        let attach = wry
            .find("Self::attach_ipc_handler(&webview, ipc_handler")
            .expect("wry の ipc ハンドラの登録が無い（形が変わった）");
        assert!(
            cond < attach && wry[cond..attach].lines().count() <= 2,
            "wry {ver} は ipc ハンドラが無いときにも登録へ進む"
        );
        // 付いていても、送り主の URI が http::Uri にならないメッセージは unwrap せずに捨てる
        let start = wry
            .find("unsafe fn attach_ipc_handler(")
            .expect("wry の attach_ipc_handler が無い（形が変わった）");
        let handler = &wry[start..];
        let handler = &handler[..handler.find("\n  #[inline]").unwrap_or(handler.len())];
        assert!(
            handler.contains("match Request::builder().uri(url).body(js) {")
                && handler.contains("Ok(request) => ipc_handler(request),"),
            "wry {ver} の ipc ハンドラが不正な URI を match で捨てる形でない（形が変わった）"
        );
        assert!(
            !handler.contains(".unwrap()"),
            "wry {ver} の ipc ハンドラに unwrap が戻った（#724 の abort の形）"
        );
    }

    #[test]
    fn issue724_段階の名前はcliの値() {
        assert_eq!(WebViewPhase::Ready.as_str(), "ready");
        assert_eq!(WebViewPhase::Queued.as_str(), "queued");
        assert_eq!(WebViewPhase::Building.as_str(), "creating");
        assert_eq!(WebViewPhase::Held.as_str(), "held");
        assert_eq!(WebViewPhase::Failed.as_str(), "failed");
    }

    /// **作った Web ビューはすぐに要素の全画面表示の判断を当てる**（#2445）。
    ///
    /// wry 0.56 から macOS の WKWebView は要素の全画面表示が常に入り、全画面のあいだは WKWebView が
    /// gpui の描画ビューから外れる（理由の全文は `element_fullscreen_enabled`）。wry を呼ぶ 1 か所
    /// （`build_view`。同期の生成も生成タスクも通る）が、作った直後・返す前に当てていることと、
    /// 既定を変える口が A/B の env 1 つだけであることを固定する
    #[test]
    fn issue2445_作ったwebviewは要素の全画面表示を切る() {
        let src = include_str!("webview.rs");
        let build = fn_body(src, "fn build_view(");
        assert!(
            guarded(
                build,
                ".build_as_child(",
                "apply_element_fullscreen(&view);"
            ),
            "build_view が wry の生成の後に要素の全画面表示の判断を当てていない（#2445）"
        );
        assert!(
            guarded(build, "apply_element_fullscreen(&view);", "Ok(view)"),
            "build_view が判断を当てる前に Web ビューを返している（#2445）"
        );
        // どちらもモジュール直下の関数なので、閉じ括弧（行頭の `}`）までを本文とする
        let top_fn = |sig: &str| {
            let rest = &src[src
                .find(sig)
                .unwrap_or_else(|| panic!("{sig} が見つからない"))..];
            &rest[..rest.find("\n}\n").unwrap_or(rest.len())]
        };
        let apply = top_fn("fn apply_element_fullscreen(");
        assert!(
            apply.contains("element_fullscreen_enabled()"),
            "apply_element_fullscreen が判断の 1 か所を通っていない（#2445）"
        );
        let decide = top_fn("pub(crate) fn element_fullscreen_enabled(");
        assert_eq!(
            decide.matches("std::env::var").count(),
            1,
            "要素の全画面表示を入れる口は A/B の env 1 つだけ（#2445）"
        );
        assert!(decide.contains("\"TAKO_2445_FULLSCREEN\""));
    }

    /// **背景で Web ビューを作ってもアプリを前面に出さない**（#2445 / wry#1866）。
    ///
    /// wry 0.55 の macOS は生成のたびに `NSApplication::activate` を呼んだので、別のアプリを
    /// 使っているあいだの `tako web open`（AI の操作・起動時の復元）で tako が前面へ出た。
    /// 0.57 は「見えていて focus を取る」生成のときだけ呼ぶ。tako は不可視・focus なしで作る
    /// （次のフレームの `sync_frame` が見せる）ので、その 2 つと、registry に wry の実ソースが
    /// あれば activate がその条件の内側にしか無いことを固定する
    #[test]
    fn issue2445_背景で作ってもアプリを前面に出さない() {
        let src = include_str!("webview.rs");
        let build = fn_body(src, "fn build_view(");
        for opt in [".with_visible(false)", ".with_focused(false)"] {
            assert!(
                build.contains(opt),
                "build_view が {opt} を外した（wry が生成でアプリを前面へ出す。#2445）"
            );
        }
        let ver = wry_version();
        let Some(wry) = wry_source(&ver, "src/wkwebview/mod.rs") else {
            println!(
                "TAKO_2445: wry {ver} の実ソースが registry に無いので activate の検査は見送る"
            );
            return;
        };
        // activate はどれも「見えていて focus を取る」条件のブロックの内側にあること
        // （条件の文字列は makeFirstResponder 用にも出るので、各呼び出しの直前の条件で見る）
        const GUARD: &str = "if attributes.visible && attributes.focused {";
        let calls: Vec<usize> = ["NSApplication::activate(", "activateIgnoringOtherApps("]
            .iter()
            .flat_map(|c| wry.match_indices(c).map(|(i, _)| i))
            .collect();
        assert!(
            !calls.is_empty(),
            "wry {ver} の activate が無い（形が変わった）"
        );
        for at in calls {
            let line = wry[..at].lines().count();
            let inside = wry[..at].rfind(GUARD).is_some_and(|guard| {
                // ブロックの終わり = 条件の行の字下げで閉じる `}`
                let indent = wry[..guard].rsplit('\n').next().unwrap_or("");
                let close = format!("\n{indent}}}");
                wry[guard..]
                    .find(&close)
                    .is_some_and(|len| at < guard + len)
            });
            assert!(
                inside,
                "wry {ver} の wkwebview/mod.rs:{line} の activate が「見えていて focus を取る」条件の外にある\
                 （0.55 は生成のたびに前面へ出した。#2445）"
            );
        }
    }

    /// **Windows の Web ビューへユーザーデータの置き場を渡す**（#2431）。
    ///
    /// WebContext を渡さないと wry は実行ファイルの隣（`tako-app.exe.WebView2`）を使う。wry を呼ぶ 1 か所
    /// （`build_view`）が、置き場の判断（`web_context` → `platform::webview2::data_dir_choice`）を通して
    /// `new_with_web_context` で作ることと、生成の失敗が分類の 1 か所（`create_failure_reason` →
    /// `classify_here`）を通ることを固定する
    #[test]
    fn issue2431_webviewへユーザーデータの置き場を渡す() {
        let src = include_str!("webview.rs");
        let build = fn_body(src, "fn build_view(");
        assert!(
            guarded(
                build,
                "let mut context = web_context();",
                "new_with_web_context(context)"
            ),
            "build_view が置き場の判断を通さずに wry の Web ビューを作っている（#2431）"
        );
        assert!(
            guarded(build, "new_with_web_context(context)", ".build_as_child("),
            "build_view が WebContext を渡さずに生成している（#2431）"
        );
        assert!(
            build.contains(".map_err(|e| create_failure_reason(&e.to_string(), shared))?;"),
            "生成の失敗が分類を通っていない（#2431）"
        );
        let top_fn = |sig: &str| {
            let rest = &src[src
                .find(sig)
                .unwrap_or_else(|| panic!("{sig} が見つからない"))..];
            &rest[..rest.find("\n}\n").unwrap_or(rest.len())]
        };
        assert!(top_fn("fn web_context(").contains("platform::webview2::data_dir_choice()"));
        assert!(
            top_fn("fn create_failure_reason(").contains("platform::webview2::classify_here(raw)")
        );
    }

    /// macOS は WebContext を渡さず、失敗の文言も従来のまま（#2431 で macOS の挙動を変えない）
    #[test]
    #[cfg(not(windows))]
    fn issue2431_macosでは置き場を渡さず文言も変えない() {
        assert!(web_context().is_none());
        let shared = Arc::new(Mutex::new(WebShared::default()));
        assert_eq!(
            create_failure_reason("boom", &shared),
            "webview 生成失敗: boom"
        );
        assert_eq!(shared.lock().unwrap().create_failure, None);
    }

    /// Windows はテストの data dir の下の WebView2 を渡し（本番の置き場に書かない）、
    /// 失敗は分類して種別を残す
    #[test]
    #[cfg(windows)]
    fn issue2431_windowsでは置き場を渡し失敗を分類する() {
        let context = web_context().expect("Windows は WebContext を渡す");
        assert_eq!(
            context.data_directory().map(std::path::Path::to_path_buf),
            tako_core::platform::webview2::user_data_dir()
        );
        let shared = Arc::new(Mutex::new(WebShared::default()));
        let raw =
            "WebView2 error: WindowsError(Error { code: HRESULT(0x80070002), message: \"\" })";
        let text = create_failure_reason(raw, &shared);
        assert!(text.contains(raw), "{text}");
        let kind = shared.lock().unwrap().create_failure;
        assert!(
            matches!(kind, Some("runtime_missing" | "runtime_outdated")),
            "{kind:?}"
        );
    }

    /// wry の原文から HRESULT を拾えること（#2431）。wry の `WebView2Error` は webview2-com の
    /// Error の Display（= Debug）で、その中の windows の Error の Debug が `HRESULT(0x…)` を出す。
    /// Windows では本物の windows の Error で確かめる
    #[test]
    #[cfg(windows)]
    fn issue2431_本物のwindowsのエラーからhresultを拾える() {
        let error = windows::core::Error::from(windows::core::HRESULT(0x8007_0002_u32 as i32));
        let raw = format!("WebView2 error: WindowsError({error:?})");
        assert_eq!(
            tako_core::platform::webview2::hresult_in(&raw),
            Some(tako_core::platform::webview2::HRESULT_FILE_NOT_FOUND),
            "{raw}"
        );
    }

    /// **求める WebView2 の版は wry の既定と同じ**（#2431）。
    ///
    /// wry は環境のオプションを `CoreWebView2EnvironmentOptions::default()` で作り、その
    /// `TargetCompatibleBrowserVersion` は webview2-com-sys の `CORE_WEBVIEW_TARGET_PRODUCT_VERSION`。
    /// これより古いランタイムでは生成が `0x80070002` で失敗するので、`REQUIRED_VERSION` はこの値に
    /// 揃っていなければならない。版が変わったら止まり、registry に実ソースがあれば値と表示の形
    /// （HRESULT を拾う手がかり）をその版のソースで確かめる
    #[test]
    fn issue2431_求めるwebview2の版はwryの既定と同じ() {
        use tako_core::platform::webview2::REQUIRED_VERSION;
        let sys = lock_version("webview2-com-sys");
        let com = lock_version("webview2-com");
        assert!(
            sys == "0.39.1" && com == "0.39.1",
            "webview2-com が {com} / webview2-com-sys が {sys} になった。REQUIRED_VERSION（{REQUIRED_VERSION}）は \
             0.39.1 の CORE_WEBVIEW_TARGET_PRODUCT_VERSION。新しい版の値を読み直して合わせる"
        );
        let Some(bindings) = crate_source("webview2-com-sys", &sys, "src/bindings.rs") else {
            println!("TAKO_2431: webview2-com-sys {sys} の実ソースが registry に無いので値の検査は見送る");
            return;
        };
        let at = bindings
            .find("pub const CORE_WEBVIEW_TARGET_PRODUCT_VERSION")
            .expect("CORE_WEBVIEW_TARGET_PRODUCT_VERSION が無い（形が変わった）");
        assert!(
            bindings[at..]
                .lines()
                .take(3)
                .any(|l| l.contains(&format!("w!(\"{REQUIRED_VERSION}\")"))),
            "webview2-com-sys {sys} の対象の版が REQUIRED_VERSION（{REQUIRED_VERSION}）と違う"
        );
        if let Some(options) = crate_source("webview2-com", &com, "src/options.rs") {
            assert!(
                options.contains("CORE_WEBVIEW_TARGET_PRODUCT_VERSION.to_string()"),
                "webview2-com {com} の既定が対象の版を使っていない（形が変わった）"
            );
        }
        if let Some(lib) = crate_source("webview2-com", &com, "src/lib.rs") {
            assert!(
                lib.contains("write!(f, \"{self:?}\")"),
                "webview2-com {com} の Error の表示が Debug でない = HRESULT を拾う手がかりが変わった"
            );
        }
        let ver = wry_version();
        if let Some(wry) = wry_source(&ver, "src/webview2/mod.rs") {
            assert!(
                wry.contains("CoreWebView2EnvironmentOptions::default()")
                    && !wry.contains("set_target_compatible_browser_version"),
                "wry {ver} が対象の版を既定から変えている（REQUIRED_VERSION を読み直す）"
            );
            assert!(
                wry.contains(".and_then(|context| context.data_directory())"),
                "wry {ver} が WebContext の置き場を WebView2 へ渡していない（形が変わった）"
            );
        }
        if let Some(error) = wry_source(&ver, "src/error.rs") {
            assert!(
                error.contains("#[error(\"WebView2 error: {0}\")]"),
                "wry {ver} の WebView2Error の表示が変わった（HRESULT を拾う手がかり）"
            );
        }
    }
}

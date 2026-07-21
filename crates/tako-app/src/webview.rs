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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tako_core::PaneId;

/// GPUI ウィンドウの生ハンドルを保持して wry の親にするラッパー。
/// `gpui::Window` は render 中しか触れないため、初回 render で採取した
/// `RawWindowHandle` を保持し、dispatch（IPC / MCP）からの webview 生成でも使う。
///
/// SAFETY: tako は単一ウィンドウ構成で、ウィンドウ破棄 = アプリ終了。
/// ハンドル（macOS では NSView ポインタ）はアプリ生存中ずっと有効
pub struct WindowHandleBox(raw_window_handle::RawWindowHandle);

impl WindowHandleBox {
    pub fn from_window(window: &gpui::Window) -> Option<Self> {
        // gpui::Window には同名の inherent メソッド（AnyWindowHandle を返す）があるため、
        // raw-window-handle の trait メソッドを明示的に呼ぶ
        raw_window_handle::HasWindowHandle::window_handle(window)
            .ok()
            .map(|h| Self(h.as_raw()))
    }
}

impl raw_window_handle::HasWindowHandle for WindowHandleBox {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: 上記のとおり単一ウィンドウでアプリと同寿命
        unsafe { Ok(raw_window_handle::WindowHandle::borrow_raw(self.0)) }
    }
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
/// 両状態を通じて wry WebView インスタンスを保持し続ける
pub struct WebViewEntry {
    pub id: WebViewId,
    pub view: wry::WebView,
    pub shared: Arc<Mutex<WebShared>>,
    /// Some = このペインに表示中 / None = dock 退避中
    pub pane: Option<PaneId>,
    /// 直近フレームの可視状態（差分呼び出しで AppKit 往復を減らす）
    pub(crate) visible_now: bool,
    /// 直近フレームの bounds（論理 px。差分呼び出し用）
    bounds_now: Option<(f64, f64, f64, f64)>,
    /// eval トークン採番
    next_eval_token: u64,
}

impl WebViewEntry {
    /// wry WebView を GPUI ウィンドウの子ビューとして生成する。
    /// 生成直後は不可視（次フレームの `sync_frame` で表示位置が決まる）
    pub fn build(
        window: &impl raw_window_handle::HasWindowHandle,
        id: WebViewId,
        url: &str,
    ) -> Result<Self, String> {
        validate_url(url)?;
        let track_nav = should_track_nav(url);
        let shared = Arc::new(Mutex::new(WebShared {
            url: url.to_string(),
            nav_started_at: if track_nav {
                Some(std::time::Instant::now())
            } else {
                None
            },
            nav_target_url: if track_nav {
                Some(url.to_string())
            } else {
                None
            },
            ..Default::default()
        }));
        let ipc_shared = Arc::clone(&shared);
        let load_shared = Arc::clone(&shared);
        let new_win_shared = Arc::clone(&shared);
        let view = wry::WebViewBuilder::new()
            .with_url(url)
            .with_visible(false)
            .with_focused(false)
            .with_initialization_script(STATE_HOOK_JS)
            .with_new_window_req_handler(move |url, _features| {
                if let Ok(mut s) = new_win_shared.lock() {
                    s.pending_new_window_urls.push(url);
                }
                wry::NewWindowResponse::Deny
            })
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
                        }
                    }
                }
            })
            .build_as_child(window)
            .map_err(|e| format!("webview 生成失敗: {e}"))?;
        Ok(Self {
            id,
            view,
            shared,
            pane: None,
            visible_now: false,
            bounds_now: None,
            next_eval_token: 1,
        })
    }

    /// フレーム同期: 表示すべきなら bounds（論理 px）を渡し、隠すなら None。
    /// 実際の AppKit / Win32 呼び出しは値が変わったときだけ行う
    pub fn sync_frame(&mut self, bounds: Option<(f64, f64, f64, f64)>) {
        match bounds {
            Some((x, y, w, h)) => {
                if self.bounds_now != Some((x, y, w, h)) {
                    let _ = self.view.set_bounds(wry::Rect {
                        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(x, y)),
                        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(w, h)),
                    });
                    self.bounds_now = Some((x, y, w, h));
                }
                if !self.visible_now {
                    let _ = self.view.set_visible(true);
                    self.visible_now = true;
                }
            }
            None => {
                if self.visible_now {
                    // 隠す前にフォーカスを親（GPUI ウィンドウ）へ返す。
                    // 返さないとキー入力が不可視 webview に吸われ続ける
                    let _ = self.view.focus_parent();
                    let _ = self.view.set_visible(false);
                    self.visible_now = false;
                }
            }
        }
    }

    /// タイトル・URL を JS 評価で採取して shared へ書き戻す（2 秒ポーリングから呼ぶ）。
    /// ipc（WKScriptMessageHandler）は data: URL 等で通知が届かないケースを実機で
    /// 確認したため（#155 セルフテスト診断: eval は届くが ipc の title が空のまま）、
    /// 実証済みの evaluate_script_with_callback 経路を正とし、ipc は http(s) ページでの
    /// 即時更新用に併存させる
    pub fn poll_state(&self) {
        let shared = Arc::clone(&self.shared);
        let _ = self.view.evaluate_script_with_callback(
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

    /// ナビゲーション操作。`to` は "back" / "forward" / "reload" / URL 文字列
    pub fn navigate(&self, to: &str) -> Result<(), String> {
        match to {
            "back" => self
                .view
                .evaluate_script("history.back();")
                .map_err(|e| format!("back 失敗: {e}")),
            "forward" => self
                .view
                .evaluate_script("history.forward();")
                .map_err(|e| format!("forward 失敗: {e}")),
            "reload" => {
                // エラー状態なら失敗した URL をリトライ
                if let Ok(s) = self.shared.lock() {
                    if let Some(url) = s.failed_url.clone() {
                        drop(s);
                        return self.navigate(&url);
                    }
                }
                self.view.reload().map_err(|e| format!("reload 失敗: {e}"))
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
                self.view
                    .load_url(&url)
                    .map_err(|e| format!("URL 読み込み失敗: {e}"))
            }
        }
    }

    /// JS を非同期評価する。結果は後で `take_eval_result(token)` で回収する。
    /// dispatch は UI スレッドで走るため、ここで結果を同期待ちしてはならない
    /// （wry のコールバックも UI スレッド配送 = 待つとデッドロック）
    pub fn eval(&mut self, js: &str) -> Result<u64, String> {
        let token = self.next_eval_token;
        self.next_eval_token += 1;
        let shared = Arc::clone(&self.shared);
        self.view
            .evaluate_script_with_callback(js, move |result| {
                if let Ok(mut s) = shared.lock() {
                    // 回収されない結果が無限に溜まらないよう上限を設ける
                    if s.eval_results.len() > 64 {
                        s.eval_results.clear();
                    }
                    s.eval_results.insert(token, result);
                }
            })
            .map_err(|e| format!("eval 失敗: {e}"))?;
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
                    s.error = Some("ページの読み込みに失敗しました".to_string());
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
        self.navigate(&url)
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

    /// NSEvent local monitor のコールバック。
    /// first responder が WKWebView で、tako が処理すべき ⌘ キーなら
    /// first responder を content view へ戻す。イベント自体は常にそのまま返す
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
        let first_resp = f_id(window, sel("firstResponder"));
        if first_resp.is_null() {
            return event;
        }
        // first responder が WKWebView（またはそのサブクラス）かチェック
        let wk_cls = cls("WKWebView");
        if wk_cls.is_null() {
            return event;
        }
        let ik_sel = sel("isKindOfClass:");
        let f_bool: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> bool =
            std::mem::transmute(objc_msgSend as *const c_void);
        let is_wk = f_bool(first_resp, ik_sel, wk_cls);
        if !is_wk {
            // WKWebView のサブビュー（WKContentView 等）が first responder の場合もある
            let sv_sel = sel("superview");
            let parent = f_id(first_resp, sv_sel);
            if parent.is_null() || !f_bool(parent, ik_sel, wk_cls) {
                return event;
            }
        }
        // first responder を content view（GPUI のカスタム NSView）へ戻す
        let content_view = f_id(window, sel("contentView"));
        if !content_view.is_null() {
            let mk_sel = sel("makeFirstResponder:");
            let f_mk: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> bool =
                std::mem::transmute(objc_msgSend as *const c_void);
            f_mk(window, mk_sel, content_view);
        }
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
    pub(super) fn install() {}
    pub(super) fn set_has_webview(_: bool) {}
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
pub fn validate_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL が空です".into());
    }
    if url.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(format!(
            "URL に空白文字が含まれています（NSURL が解釈できない）: {url}"
        ));
    }
    if url.bytes().any(|b| b < 0x20) {
        return Err(format!(
            "URL に制御文字が含まれています（NSURL が解釈できない）: {url}"
        ));
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
}

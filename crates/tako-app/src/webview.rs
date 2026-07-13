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
    visible_now: bool,
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
        let shared = Arc::new(Mutex::new(WebShared {
            url: url.to_string(),
            ..Default::default()
        }));
        let ipc_shared = Arc::clone(&shared);
        let load_shared = Arc::clone(&shared);
        let view = wry::WebViewBuilder::new()
            .with_url(url)
            .with_visible(false)
            .with_focused(false)
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
            })
            .with_on_page_load_handler(move |event, url| {
                if let Ok(mut s) = load_shared.lock() {
                    match event {
                        wry::PageLoadEvent::Started => {
                            s.loading = true;
                            s.url = url;
                        }
                        wry::PageLoadEvent::Finished => {
                            s.loading = false;
                            s.url = url;
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
            "reload" => self.view.reload().map_err(|e| format!("reload 失敗: {e}")),
            url => {
                let url = normalize_url(url);
                if let Ok(mut s) = self.shared.lock() {
                    s.url = url.clone();
                    s.loading = true;
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

    /// ページ読み込み中か
    pub fn is_loading(&self) -> bool {
        self.shared.lock().map(|s| s.loading).unwrap_or(false)
    }
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
}

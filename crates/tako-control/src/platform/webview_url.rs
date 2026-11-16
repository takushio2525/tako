//! Web ビュー（wry）が URL を解釈できるかの確かめ（抽象境界 B38。Issue #2937）。
//!
//! - **macOS**: wry 0.57.1 の `navigate_to_url`（生成時の `with_url` も読み込みの `load_url` も
//!   ここを通る）は `NSURL::URLWithString(…).unwrap()` で URL を作るので、NSURL が nil を返す
//!   文字列を渡すと GUI ごと abort する（#334 の空白・#2937 の `https://javascript:alert(1)` =
//!   ポートが数字でない形）。渡す前に**同じプロセスで同じ `+[NSURL URLWithString:]`** を呼び、
//!   nil なら「解釈できない」と答える。同じプロセス・同じ OS・同じリンク時の SDK の印なので、
//!   wry が受ける結果と一致する（macOS 14 から SDK の印で解釈が変わる API なので、
//!   規則を手で写すと OS と SDK の組み合わせでずれる）
//! - **それ以外（Windows = WebView2）**: 確かめない（`true`）。WebView2 の `Navigate` は解釈できない
//!   URL を `E_INVALIDARG` の Err で返し、wry もそれを Err で返す（落ちない）。tako はその Err を
//!   「URL 読み込み失敗」・生成失敗の理由として返す
//!
//! 外部クレートを足さず、必要な関数だけを手書きで宣言する（`finder_reveal` / `file_clipboard` と同じ方針）

/// `url` を Web ビュー（wry）へ渡して落ちないか。macOS は NSURL の解釈、それ以外は常に `true`
pub fn webview_parses(url: &str) -> bool {
    imp::webview_parses(url)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CStr, CString};

    type Id = *const c_void;
    type Sel = *const c_void;

    // 宣言は `finder_reveal` / `file_clipboard` / `sleep_guard` の同名の宣言と同じ形にそろえる
    // （食い違うと `clashing_extern_declarations` で警告になる）
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> *const c_void;
        fn sel_registerName(name: *const u8) -> *const c_void;
        fn objc_msgSend(receiver: *const c_void, sel: *const c_void, ...) -> *const c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// 自動解放プール（UI スレッドの外 = テストから呼ばれても漏らさない）
    struct Pool(*mut c_void);

    impl Pool {
        fn new() -> Self {
            // SAFETY: 引数なし。返り値は Drop で 1 回だけ戻す
            Self(unsafe { objc_autoreleasePoolPush() })
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            // SAFETY: new で積んだプールを積んだスレッドで 1 回だけ戻す
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }

    fn class(name: &CStr) -> Id {
        // SAFETY: NUL 終端の名前を渡すだけ（無ければ null）
        unsafe { objc_getClass(name.as_ptr() as *const u8) }
    }

    fn sel(name: &CStr) -> Sel {
        // SAFETY: NUL 終端の名前を渡すだけ
        unsafe { sel_registerName(name.as_ptr() as *const u8) }
    }

    /// Foundation を読み込む（GUI では wry が読み込み済み。テスト・CLI から呼ばれたときだけ効く）
    fn ensure_foundation() -> bool {
        static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LOADED.get_or_init(|| {
            if !class(c"NSURL").is_null() {
                return true;
            }
            // SAFETY: システムのフレームワークを名前で開くだけ（閉じない = プロセスの寿命まで）
            let handle = unsafe {
                libc::dlopen(
                    c"/System/Library/Frameworks/Foundation.framework/Foundation".as_ptr(),
                    libc::RTLD_LAZY | libc::RTLD_GLOBAL,
                )
            };
            !handle.is_null() && !class(c"NSURL").is_null()
        })
    }

    pub(super) fn webview_parses(url: &str) -> bool {
        // NUL を含む文字列は C 文字列にできない。呼び手の形の検査（制御文字）が先に断るので
        // ここへは来ないが、来たら「解釈できない」側へ倒す（wry へ渡さない）
        let Ok(c) = CString::new(url) else {
            return false;
        };
        // Foundation が読めない = 確かめられない。wry も同じ NSURL を使うので、この場合は
        // wry の側でも Web ビューを作れない（ここで断る理由を作らない）
        if !ensure_foundation() {
            return true;
        }
        let _pool = Pool::new();
        // ARM64 では可変長引数の呼び出しは引数をスタックへ置くので、`objc_msgSend` は
        // 型付きの関数ポインタへ写してから呼ぶ（レジスタ渡しを保証する。finder_reveal と同じ）
        // SAFETY: 受け手はクラス、引数は NUL 終端の UTF-8（`c` はこの関数の終わりまで生きる）と
        // NSString（自動解放 = 上のプールが戻す）。返り値は nil かどうかだけを見る
        unsafe {
            let with_cstr: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            let with_id: unsafe extern "C" fn(Id, Sel, Id) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            // wry は `NSString::from_str`（UTF-8 のバイト列から作る）。NUL を含まない &str なら
            // `stringWithUTF8String:` と同じ文字列になる
            let string = with_cstr(
                class(c"NSString"),
                sel(c"stringWithUTF8String:"),
                c.as_ptr(),
            );
            if string.is_null() {
                return false;
            }
            !with_id(class(c"NSURL"), sel(c"URLWithString:"), string).is_null()
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    /// WebView2 は解釈できない URL を Err で返す（wry が落ちない）ので確かめない
    pub(super) fn webview_parses(_url: &str) -> bool {
        true
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::webview_parses;

    #[test]
    fn 今開けるurlの形はnsurlが解釈する() {
        for url in [
            "https://example.com",
            "http://localhost:3000",
            "http://127.0.0.1:8080",
            "http://0.0.0.0:8000",
            "https://example.com/path?q=1&r=2#frag",
            "https://xn--example.com/%E6%97%A5%E6%9C%AC%E8%AA%9E",
            "https://user:pass@example.com",
            "file:///tmp/tako-2937/index.html",
            "data:text/html,hello",
            "about:blank",
        ] {
            assert!(webview_parses(url), "NSURL が解釈できるはずの形: {url}");
        }
    }

    #[test]
    fn 修正前にwryを落とした形はnsurlが解釈しない() {
        // #2937 の実測の入力（`javascript:alert(2936)` を修正前の正規化が https:// 付きにした形）
        assert!(!webview_parses("https://javascript:alert(2936)"));
        // ポートが数字でない（`foo:bar` の正規化の結果と同じ形）
        assert!(!webview_parses("https://foo:bar"));
        // ホストに使えない文字・閉じていない IPv6・負のポート（アドレスバーの打ち間違いの形。
        // どれも修正前は wry の unwrap で abort した。SDK 26.2 の NSURL の実測）
        for url in [
            "https://exa|mple.com",
            "https://[::1",
            "https://example.com:-1",
            "https://ex%zzample.com",
        ] {
            assert!(!webview_parses(url), "NSURL が nil を返すはずの形: {url}");
        }
    }

    #[test]
    fn 符号化で直る文字はnsurlが解釈する() {
        // macOS 14 以降の SDK でリンクしたアプリでは `URLWithString:` が不正な文字を符号化してから
        // 解釈する（tako は SDK 26.2 でリンク）。パスの空白・非 ASCII・data: の `<>` は通る
        for url in [
            "https://example.com/path with spaces",
            "https://例え.jp/日本語",
            "data:text/html,<title>t2937</title>ok",
        ] {
            assert!(webview_parses(url), "NSURL が解釈できるはずの形: {url}");
        }
    }

    #[test]
    fn nulを含む文字列は解釈できない側へ倒す() {
        assert!(!webview_parses("https://example.com/\0x"));
    }
}

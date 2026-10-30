//! 「Finder で表示」の macOS の実体（抽象境界 B8 の内側。Issue #2247）
//!
//! tako 自身（押された・操作を受けたアプリ）から `NSWorkspace activateFileViewerSelectingURLs:`
//! を呼び、Finder の窓で対象を選んで前面へ出す。呼ぶ前に、今の前面のアプリである tako から
//! Finder へ前面を譲る（`NSApplication yieldActivationToApplicationWithBundleIdentifier:`。
//! macOS 14 以降だけが応答するので、応答しない OS では呼ばない）。
//!
//! 以前は子プロセスの `open -R` に任せていた。本番では `open` が成功（終了状態 0）していたのに
//! 「Finder が出てこない」と報告された（#2247）。macOS 14 からのアプリの協調的な
//! アクティベーション（cooperative activation）では、前面へ出られるのは前面のアプリから
//! 譲られたアプリだけで、tako の子の `open` は tako ではないので譲れない、という見立て
//! （本物の Finder での確認は install 後。旧の形は `TAKO_2247_LEGACY=1` で残す）。
//!
//! `record` を渡すと、AppKit へ渡す**直前**に「渡す URL が指すパス」と「前面を譲れるか」を
//! 1 行書き、Finder は開かない（検証の口 = `TAKO_REVEAL_RECORD`。本物の Finder の窓を
//! ユーザーの画面に出さずに、どの入口がこの実体まで届いたかを数える）。
//!
//! 外部クレートを足さず、必要な関数だけを手書きで宣言する（`file_clipboard` と同じ方針）。

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// 記録の 1 行の頭（OS に依らず読めるよう `os_integration` に置く）
pub use super::os_integration::REVEAL_RECORD_MARK as RECORD_MARK;

use super::os_integration::OsIntegrationError;

/// 前面を譲る相手（ファイルビューア = Finder）
const FINDER_BUNDLE_ID: &str = "com.apple.finder";

type Id = *const c_void;
type Sel = *const c_void;

// 宣言は `file_clipboard` / `sleep_guard` の同名の宣言と同じ形にそろえる（食い違うと
// `clashing_extern_declarations` で警告になる）
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

// ARM64 では可変長引数の呼び出しは引数をスタックへ置くので、`objc_msgSend` は
// 型付きの関数ポインタへ写してから呼ぶ（レジスタ渡しを保証する。file_clipboard と同じ）
unsafe fn send(recv: Id, name: &CStr) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(recv, sel(name))
}

unsafe fn send1(recv: Id, name: &CStr, a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(recv, sel(name), a)
}

unsafe fn send_cstr(recv: Id, name: &CStr, a: *const c_char) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(recv, sel(name), a)
}

/// `recv` がそのセレクタに応答するか（新しい OS にだけある API を呼ぶ前の確かめ）
unsafe fn responds(recv: Id, name: &CStr) -> bool {
    let f: unsafe extern "C" fn(Id, Sel, Sel) -> u8 =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(recv, sel(c"respondsToSelector:"), sel(name)) != 0
}

/// AppKit を読み込む（GUI では読み込み済み。テストから呼ばれたときだけ効く）
fn ensure_appkit() -> bool {
    static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOADED.get_or_init(|| {
        if !class(c"NSWorkspace").is_null() {
            return true;
        }
        // SAFETY: システムのフレームワークを名前で開くだけ（閉じない = プロセスの寿命まで）
        let handle = unsafe {
            libc::dlopen(
                c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(),
                libc::RTLD_LAZY | libc::RTLD_GLOBAL,
            )
        };
        !handle.is_null() && !class(c"NSWorkspace").is_null()
    })
}

/// このプロセスの `NSApp`（GUI でなければ null）。`sharedApplication` は呼ばない
/// （GUI でないプロセスにアプリを作ってしまう）
unsafe fn ns_app() -> Id {
    let slot = libc::dlsym(libc::RTLD_DEFAULT, c"NSApp".as_ptr()) as *const Id;
    if slot.is_null() {
        std::ptr::null()
    } else {
        *slot
    }
}

unsafe fn ns_string(text: &str) -> Result<Id, OsIntegrationError> {
    let c = CString::new(text).map_err(|_| OsIntegrationError::NulInString)?;
    let s = send_cstr(class(c"NSString"), c"stringWithUTF8String:", c.as_ptr());
    if s.is_null() {
        return Err(OsIntegrationError::NsStringFailed);
    }
    Ok(s)
}

/// パスのバイト列そのままでファイルの URL を作る（空白・日本語・UTF-8 でない名前も崩さない）
unsafe fn file_url(path: &Path) -> Result<Id, OsIntegrationError> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| OsIntegrationError::NulInPath(path.display().to_string()))?;
    let is_dir = std::fs::metadata(path).is_ok_and(|m| m.is_dir());
    let f: unsafe extern "C" fn(Id, Sel, *const c_char, u8, Id) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    let url = f(
        class(c"NSURL"),
        sel(c"fileURLWithFileSystemRepresentation:isDirectory:relativeToURL:"),
        c.as_ptr(),
        u8::from(is_dir),
        std::ptr::null(),
    );
    if url.is_null() {
        return Err(OsIntegrationError::FileUrlFailed(
            path.display().to_string(),
        ));
    }
    Ok(url)
}

/// Finder の窓で `path` を選んで前面へ出す（`record` があれば記録だけして開かない）
pub(crate) fn reveal(path: &Path, record: Option<&Path>) -> Result<(), OsIntegrationError> {
    let _pool = Pool::new();
    // SAFETY: AppKit の公開 API を、各セレクタの宣言どおりの型で呼ぶ。作ったものは
    // すべて自動解放（上のプールが戻す）
    unsafe {
        if !ensure_appkit() {
            return Err(OsIntegrationError::AppKitUnavailable);
        }
        let url = file_url(path)?;
        let urls = send1(class(c"NSArray"), c"arrayWithObject:", url);
        let workspace = send(class(c"NSWorkspace"), c"sharedWorkspace");
        if urls.is_null() || workspace.is_null() {
            return Err(OsIntegrationError::WorkspaceUnavailable);
        }
        let app = ns_app();
        let can_yield =
            !app.is_null() && responds(app, c"yieldActivationToApplicationWithBundleIdentifier:");
        if let Some(record) = record {
            return write_record(record, url, can_yield);
        }
        if can_yield {
            send1(
                app,
                c"yieldActivationToApplicationWithBundleIdentifier:",
                ns_string(FINDER_BUNDLE_ID)?,
            );
        }
        // 戻り値は void（Finder へ頼むだけで、開けたかどうかは返らない）
        send1(workspace, c"activateFileViewerSelectingURLs:", urls);
    }
    Ok(())
}

/// 検証の記録: `<RECORD_MARK>\t<URL が指すパス>\tyield=<0|1>` を 1 行足す。
/// パスは作った NSURL から読み戻したバイト列（AppKit へ渡すものそのもの）
unsafe fn write_record(record: &Path, url: Id, can_yield: bool) -> Result<(), OsIntegrationError> {
    let repr = send(url, c"fileSystemRepresentation") as *const c_char;
    if repr.is_null() {
        return Err(OsIntegrationError::RecordPathUnreadable);
    }
    let mut line = Vec::new();
    line.extend_from_slice(RECORD_MARK.as_bytes());
    line.push(b'\t');
    line.extend_from_slice(CStr::from_ptr(repr).to_bytes());
    line.extend_from_slice(format!("\tyield={}\n", u8::from(can_yield)).as_bytes());
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record)
        .and_then(|mut f| f.write_all(&line))
        .map_err(|e| OsIntegrationError::RecordWrite {
            path: record.display().to_string(),
            error: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとの一時 dir（終わったら消す）
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tako-reveal-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("一時 dir");
            Self(tako_core::platform::path::canonicalize(&dir).expect("正規化"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 記録の行を (パス, yield) で読む
    fn records(log: &Path) -> Vec<(String, String)> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let mut cols = l.split('\t');
                assert_eq!(cols.next(), Some(RECORD_MARK), "{l}");
                (
                    cols.next().unwrap_or_default().to_string(),
                    cols.next().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn 渡すurlは空白と日本語の名前とディレクトリをそのまま指す() {
        let dir = Scratch::new("url");
        let root = dir.0.clone();
        let spaced = root.join("空白 と 日本語.md");
        std::fs::write(&spaced, "x").expect("fixture");
        let sub = root.join("sub dir");
        std::fs::create_dir(&sub).expect("fixture の dir");
        let log = root.join("reveal.log");
        reveal(&spaced, Some(&log)).expect("記録できる");
        reveal(&sub, Some(&log)).expect("記録できる");
        // テストのプロセスは GUI ではない（NSApp が無い）ので前面は譲らない
        assert_eq!(
            records(&log),
            vec![
                (spaced.display().to_string(), "yield=0".to_string()),
                (sub.display().to_string(), "yield=0".to_string()),
            ]
        );
    }

    #[test]
    fn 記録を書けないときは理由を返す() {
        let dir = Scratch::new("unwritable");
        let target = dir.0.join("a.md");
        std::fs::write(&target, "x").expect("fixture");
        let err = reveal(&target, Some(&dir.0.join("無い dir").join("reveal.log")))
            .expect_err("書けない記録先");
        assert!(
            matches!(err, OsIntegrationError::RecordWrite { .. }),
            "{err:?}"
        );
        let ja = err.text_in(tako_core::i18n::Lang::Ja);
        assert!(ja.starts_with("記録を書けない"), "{ja}");
        let en = err.text_in(tako_core::i18n::Lang::En);
        assert!(en.starts_with("cannot write the record"), "{en}");
    }
}

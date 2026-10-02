//! OS のファイルのクリップボード（抽象境界 B28。Issue #1860）
//!
//! ファイルツリーの ⌘C / ⌘X が**Finder / エクスプローラーへ貼れる形**で書き、Finder /
//! エクスプローラーでコピーしたファイルを ⌘V が読む。gpui のクリップボードは読みだけ
//! ファイルを返し、書きはファイルを黙って捨てる（`ClipboardEntry::ExternalPaths` を
//! 書かない）ので、書きと読みをここに 1 組で持つ。
//!
//! - **macOS**: NSPasteboard。ファイルごとに `public.file-url` の項目を書き、1 つ目の項目に
//!   `public.utf8-plain-text`（パス）も載せる（ターミナルへ貼るとパスが入る）。読みは
//!   `readObjectsForClasses:[NSURL] options:{FileURLsOnly}`（Finder の ⌘C が置く形）。
//!   パスは `fileSystemRepresentation` で往復させる（UTF-8 でない名前も崩さない）
//! - **Windows**: `CF_HDROP`（DROPFILES + 二重 NUL 終端の UTF-16 パス）+
//!   `Preferred DropEffect`（コピー = `DROPEFFECT_COPY` / 切り取り = `DROPEFFECT_MOVE`。
//!   エクスプローラーはこれを見て移動する）+ `CF_UNICODETEXT`。読みは `CF_HDROP` と
//!   `Preferred DropEffect`（エクスプローラーの切り取りを移動として受ける）
//! - **その他**: 未対応（tako の中だけのクリップボードとして動く）
//!
//! 変更番号（macOS の `changeCount` / Windows の `GetClipboardSequenceNumber`）を返すので、
//! 呼び手は「tako が書いた後に誰かが書き換えたか」を比べられる
//! （[`tako_core::file_clipboard::resolve`]）。
//!
//! `TAKO_FILE_PASTEBOARD=<名前>`（**macOS のみ**）で一般のペーストボードの代わりに名前付きの
//! ペーストボードを使う。検証で**ユーザーのクリップボードを上書きしない**ための口
//! （Windows のクリップボードは 1 つしか無いので効かない）。
//!
//! 外部クレートを足さず、必要な関数だけを手書きで宣言する（`os_integration` の Windows 実装・
//! `sleep_guard` の objc 呼び出しと同じ方針）。

use std::path::PathBuf;

use tako_core::file_clipboard::OsFiles;

/// OS のクリップボードへファイルを書く（`cut` = 切り取り）。返り値は書いた後の変更番号
pub fn write(paths: &[PathBuf], cut: bool) -> Result<u64, String> {
    if paths.is_empty() {
        return Err("書くファイルが無い".into());
    }
    imp::write(pasteboard_name().as_deref(), paths, cut)
}

/// OS のクリップボードのファイルを読む（無ければ `paths` が空）
pub fn read() -> OsFiles {
    imp::read(pasteboard_name().as_deref())
}

/// 変更番号が `stamp` のまま（= tako が書いたまま）なら空にする。空にしたら真
pub fn clear_if(stamp: u64) -> bool {
    imp::clear_if(pasteboard_name().as_deref(), stamp)
}

/// この OS で読み書きできるか（できない OS では tako の中だけのクリップボードになる）
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// 検証用の名前付きペーストボード（macOS のみ。空は無指定と同じ）
fn pasteboard_name() -> Option<String> {
    std::env::var("TAKO_FILE_PASTEBOARD")
        .ok()
        .filter(|name| !name.trim().is_empty())
}

/// テキストとして一緒に載せる中身（ターミナル・テキスト欄へ貼るとパスが入る）
fn text_of(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// DROPFILES の頭（`pFiles` / `pt.x` / `pt.y` / `fNC` / `fWide` の 5 × 4 バイト）
// Windows の実装だけが使う（他の OS ではテストだけが通る = 形を macOS の上から検査する）
#[cfg_attr(not(windows), allow(dead_code))]
const DROPFILES_HEADER: usize = 20;

/// `CF_HDROP` の中身を組む（UTF-16 のパスを NUL で区切り、最後にもう 1 つ NUL）。
///
/// プラットフォームに依らない純粋関数（macOS の上から Windows の形を検査できる）
// Windows の実装だけが使う（他の OS ではテストだけが通る = 形を macOS の上から検査する）
#[cfg_attr(not(windows), allow(dead_code))]
fn dropfiles_bytes(wide_paths: &[Vec<u16>]) -> Vec<u8> {
    let body: usize = wide_paths.iter().map(|p| (p.len() + 1) * 2).sum::<usize>() + 2;
    let mut out = Vec::with_capacity(DROPFILES_HEADER + body);
    out.extend_from_slice(&(DROPFILES_HEADER as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    for path in wide_paths {
        for unit in path.iter().chain([&0u16]) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// `CF_HDROP` の中身を読む（`fWide` が偽の ANSI 形は 1 バイト = 1 文字として読む）
// Windows の実装だけが使う（他の OS ではテストだけが通る = 形を macOS の上から検査する）
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_dropfiles(bytes: &[u8]) -> Vec<Vec<u16>> {
    if bytes.len() < DROPFILES_HEADER {
        return Vec::new();
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let offset = word(0) as usize;
    let wide = word(16) != 0;
    let Some(body) = bytes.get(offset..) else {
        return Vec::new();
    };
    let units: Vec<u16> = if wide {
        body.chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    } else {
        body.iter().map(|&b| u16::from(b)).collect()
    };
    units
        .split(|&unit| unit == 0)
        .take_while(|path| !path.is_empty())
        .map(<[u16]>::to_vec)
        .collect()
}

/// `Preferred DropEffect` の値が切り取り（移動だけを求めている）か
// Windows の実装だけが使う（他の OS ではテストだけが通る = 形を macOS の上から検査する）
#[cfg_attr(not(windows), allow(dead_code))]
fn effect_is_cut(effect: u32) -> bool {
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;
    effect & DROPEFFECT_MOVE != 0 && effect & DROPEFFECT_COPY == 0
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use tako_core::file_clipboard::OsFiles;

    type Id = *const c_void;
    type Sel = *const c_void;

    // 宣言は `sleep_guard` の同名の宣言と同じ形にそろえる（食い違うと
    // `clashing_extern_declarations` で警告になる）
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> *const c_void;
        fn sel_registerName(name: *const u8) -> *const c_void;
        fn objc_msgSend(receiver: *const c_void, sel: *const c_void, ...) -> *const c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// 自動解放プール（UI スレッドの外 = テスト・CLI から呼ばれても漏らさない）
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
    // 型付きの関数ポインタへ写してから呼ぶ（レジスタ渡しを保証する。video_player と同じ）
    unsafe fn send(recv: Id, name: &CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name))
    }

    unsafe fn send1(recv: Id, name: &CStr, a: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), a)
    }

    unsafe fn send2(recv: Id, name: &CStr, a: Id, b: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), a, b)
    }

    unsafe fn send_isize(recv: Id, name: &CStr) -> isize {
        let f: unsafe extern "C" fn(Id, Sel) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name))
    }

    unsafe fn send_index(recv: Id, name: &CStr, index: usize) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), index)
    }

    unsafe fn send_bool1(recv: Id, name: &CStr, a: Id) -> bool {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> u8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), a) != 0
    }

    unsafe fn send_bool2(recv: Id, name: &CStr, a: Id, b: Id) -> bool {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id) -> u8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), a, b) != 0
    }

    /// AppKit を読み込む（GUI では読み込み済み。CLI・テストから呼ばれたときだけ効く）
    fn ensure_appkit() -> bool {
        static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LOADED.get_or_init(|| {
            if !class(c"NSPasteboard").is_null() {
                return true;
            }
            // SAFETY: システムのフレームワークを名前で開くだけ（閉じない = プロセスの寿命まで）
            let handle = unsafe {
                libc::dlopen(
                    c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(),
                    libc::RTLD_LAZY | libc::RTLD_GLOBAL,
                )
            };
            !handle.is_null() && !class(c"NSPasteboard").is_null()
        })
    }

    unsafe fn ns_string(text: &str) -> Result<Id, String> {
        let c = CString::new(text).map_err(|_| "NUL を含む文字列".to_string())?;
        let s = send_cstr(class(c"NSString"), c"stringWithUTF8String:", c.as_ptr());
        if s.is_null() {
            return Err("NSString を作れない".into());
        }
        Ok(s)
    }

    unsafe fn send_cstr(recv: Id, name: &CStr, a: *const c_char) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(recv, sel(name), a)
    }

    unsafe fn pasteboard(name: Option<&str>) -> Result<Id, String> {
        if !ensure_appkit() {
            return Err("NSPasteboard が使えない（AppKit を読み込めない）".into());
        }
        let cls = class(c"NSPasteboard");
        let pb = match name {
            Some(name) => send1(cls, c"pasteboardWithName:", ns_string(name)?),
            None => send(cls, c"generalPasteboard"),
        };
        if pb.is_null() {
            return Err("ペーストボードを開けない".into());
        }
        Ok(pb)
    }

    /// パスのバイト列そのままでファイルの URL を作る（UTF-8 でない名前も崩さない）
    unsafe fn file_url(path: &Path) -> Result<Id, String> {
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("NUL を含むパス: {}", path.display()))?;
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
            return Err(format!("ファイルの URL を作れない: {}", path.display()));
        }
        Ok(url)
    }

    pub(super) fn write(name: Option<&str>, paths: &[PathBuf], _cut: bool) -> Result<u64, String> {
        let _pool = Pool::new();
        // SAFETY: AppKit の公開 API を、各セレクタの宣言どおりの型で呼ぶ。作ったものは
        // 自動解放（`alloc`/`init` の 1 つは配列へ入れた後に release する）
        unsafe {
            let pb = pasteboard(name)?;
            let url_type = ns_string("public.file-url")?;
            let text_type = ns_string("public.utf8-plain-text")?;
            let text = ns_string(&super::text_of(paths))?;
            let items = send(class(c"NSMutableArray"), c"array");
            for (i, path) in paths.iter().enumerate() {
                let url = file_url(path)?;
                let absolute = send(url, c"absoluteString");
                let item = send(send(class(c"NSPasteboardItem"), c"alloc"), c"init");
                if item.is_null() || absolute.is_null() {
                    return Err("ペーストボードの項目を作れない".into());
                }
                let mut ok = send_bool2(item, c"setString:forType:", absolute, url_type);
                if i == 0 {
                    ok &= send_bool2(item, c"setString:forType:", text, text_type);
                }
                send1(items, c"addObject:", item);
                send(item, c"release");
                if !ok {
                    return Err("ペーストボードの項目に書けない".into());
                }
            }
            send_isize(pb, c"clearContents");
            if !send_bool1(pb, c"writeObjects:", items) {
                return Err("ペーストボードへ書けない".into());
            }
            Ok(send_isize(pb, c"changeCount") as u64)
        }
    }

    pub(super) fn read(name: Option<&str>) -> OsFiles {
        let _pool = Pool::new();
        // SAFETY: write と同じ（読むだけ）
        unsafe {
            let Ok(pb) = pasteboard(name) else {
                return OsFiles::default();
            };
            let stamp = Some(send_isize(pb, c"changeCount") as u64);
            let classes = send1(class(c"NSArray"), c"arrayWithObject:", class(c"NSURL"));
            let yes = {
                let f: unsafe extern "C" fn(Id, Sel, u8) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(class(c"NSNumber"), sel(c"numberWithBool:"), 1)
            };
            let Ok(key) = file_urls_only_key() else {
                return OsFiles {
                    stamp,
                    ..OsFiles::default()
                };
            };
            let options = send2(
                class(c"NSDictionary"),
                c"dictionaryWithObject:forKey:",
                yes,
                key,
            );
            let urls = send2(pb, c"readObjectsForClasses:options:", classes, options);
            let mut paths = Vec::new();
            if !urls.is_null() {
                let count = send_isize(urls, c"count").max(0) as usize;
                for i in 0..count {
                    let url = send_index(urls, c"objectAtIndex:", i);
                    let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
                        std::mem::transmute(objc_msgSend as *const c_void);
                    let rep = f(url, sel(c"fileSystemRepresentation"));
                    if !rep.is_null() {
                        let bytes = CStr::from_ptr(rep).to_bytes();
                        paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)));
                    }
                }
            }
            OsFiles {
                stamp,
                paths,
                cut: false,
            }
        }
    }

    /// `NSPasteboardURLReadingFileURLsOnlyKey`（AppKit の定数。読み込み済みの AppKit から引く）
    unsafe fn file_urls_only_key() -> Result<Id, String> {
        let sym = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"NSPasteboardURLReadingFileURLsOnlyKey".as_ptr(),
        );
        if sym.is_null() {
            return ns_string("NSPasteboardURLReadingFileURLsOnlyKey");
        }
        Ok(*(sym as *const Id))
    }

    pub(super) fn clear_if(name: Option<&str>, stamp: u64) -> bool {
        let _pool = Pool::new();
        // SAFETY: write と同じ
        unsafe {
            let Ok(pb) = pasteboard(name) else {
                return false;
            };
            if send_isize(pb, c"changeCount") as u64 != stamp {
                return false;
            }
            send_isize(pb, c"clearContents");
            true
        }
    }

    /// テスト専用: 一意な名前のペーストボードを作って名前を返す（**一般のペーストボード =
    /// ユーザーのクリップボードには触らない**）。片付けは [`release_unique`]
    #[cfg(test)]
    pub(super) fn unique_name() -> Option<String> {
        let _pool = Pool::new();
        // SAFETY: write と同じ
        unsafe {
            if !ensure_appkit() {
                return None;
            }
            let pb = send(class(c"NSPasteboard"), c"pasteboardWithUniqueName");
            if pb.is_null() {
                return None;
            }
            let name = send(pb, c"name");
            let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
                std::mem::transmute(objc_msgSend as *const c_void);
            let utf8 = f(name, sel(c"UTF8String"));
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }

    #[cfg(test)]
    pub(super) fn release_unique(name: &str) {
        let _pool = Pool::new();
        // SAFETY: write と同じ
        unsafe {
            if let Ok(pb) = pasteboard(Some(name)) {
                send(pb, c"releaseGlobally");
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    use tako_core::file_clipboard::OsFiles;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn GetClipboardSequenceNumber() -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
        fn GlobalSize(mem: *mut c_void) -> usize;
    }

    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;

    /// 開いている間だけ持つ（ほかのアプリが開いていれば少し待って開き直す）
    struct Opened;

    impl Opened {
        fn open() -> Result<Self, String> {
            for _ in 0..10 {
                // SAFETY: 所有者なし（gpui の書き込みと同じ開き方）
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Self);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err("クリップボードを開けない（ほかのアプリが使っている）".into())
        }
    }

    impl Drop for Opened {
        fn drop(&mut self) {
            // SAFETY: open で開いたものを 1 回だけ閉じる
            unsafe { CloseClipboard() };
        }
    }

    fn drop_effect_format() -> u32 {
        let name: Vec<u16> = "Preferred DropEffect".encode_utf16().chain([0]).collect();
        // SAFETY: NUL 終端の UTF-16 を渡すだけ（同じ名前は同じ番号が返る）
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    /// 開いている間に呼ぶ。成功したら中身の持ち主は OS（解放しない）
    unsafe fn set_bytes(format: u32, data: &[u8]) -> Result<(), String> {
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
        if mem.is_null() {
            return Err("GlobalAlloc に失敗".into());
        }
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            GlobalFree(mem);
            return Err("GlobalLock に失敗".into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem).is_null() {
            GlobalFree(mem);
            return Err(format!(
                "SetClipboardData に失敗: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// 開いている間に呼ぶ
    unsafe fn get_bytes(format: u32) -> Option<Vec<u8>> {
        let mem = GetClipboardData(format);
        if mem.is_null() {
            return None;
        }
        let size = GlobalSize(mem);
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        GlobalUnlock(mem);
        Some(bytes)
    }

    pub(super) fn write(_name: Option<&str>, paths: &[PathBuf], cut: bool) -> Result<u64, String> {
        let wide: Vec<Vec<u16>> = paths
            .iter()
            .map(|p| p.as_os_str().encode_wide().collect())
            .collect();
        let drop_files = super::dropfiles_bytes(&wide);
        let effect = if cut {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_COPY
        }
        .to_le_bytes();
        let text: Vec<u8> = super::text_of(paths)
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        {
            let _opened = Opened::open()?;
            // SAFETY: 開いている間に、宣言どおりの型で呼ぶ
            unsafe {
                if EmptyClipboard() == 0 {
                    return Err(format!(
                        "クリップボードを空にできない: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                set_bytes(CF_HDROP, &drop_files)?;
                let format = drop_effect_format();
                if format != 0 {
                    set_bytes(format, &effect)?;
                }
                set_bytes(CF_UNICODETEXT, &text)?;
            }
        }
        // SAFETY: 引数なし
        Ok(u64::from(unsafe { GetClipboardSequenceNumber() }))
    }

    pub(super) fn read(_name: Option<&str>) -> OsFiles {
        // SAFETY: 引数なし
        let stamp = Some(u64::from(unsafe { GetClipboardSequenceNumber() }));
        let Ok(_opened) = Opened::open() else {
            return OsFiles {
                stamp,
                ..OsFiles::default()
            };
        };
        // SAFETY: 開いている間に読む
        let paths: Vec<PathBuf> = unsafe { get_bytes(CF_HDROP) }
            .map(|bytes| {
                super::parse_dropfiles(&bytes)
                    .into_iter()
                    .map(|wide| PathBuf::from(OsString::from_wide(&wide)))
                    .collect()
            })
            .unwrap_or_default();
        let format = drop_effect_format();
        let cut = format != 0
            // SAFETY: 開いている間に読む
            && unsafe { get_bytes(format) }
                .and_then(|b| b.get(..4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])))
                .is_some_and(super::effect_is_cut);
        OsFiles { stamp, paths, cut }
    }

    pub(super) fn clear_if(_name: Option<&str>, stamp: u64) -> bool {
        // SAFETY: 引数なし
        if u64::from(unsafe { GetClipboardSequenceNumber() }) != stamp {
            return false;
        }
        let Ok(_opened) = Opened::open() else {
            return false;
        };
        // SAFETY: 開いている間に呼ぶ
        unsafe { EmptyClipboard() != 0 }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::path::PathBuf;

    use tako_core::file_clipboard::OsFiles;

    pub(super) fn write(
        _name: Option<&str>,
        _paths: &[PathBuf],
        _cut: bool,
    ) -> Result<u64, String> {
        Err("この OS ではファイルのクリップボードに未対応（tako の中だけで貼れる）".into())
    }

    pub(super) fn read(_name: Option<&str>) -> OsFiles {
        OsFiles::default()
    }

    pub(super) fn clear_if(_name: Option<&str>, _stamp: u64) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn dropfilesは頭20バイトと二重nul終端のutf16で組み読み戻せる() {
        let paths = vec![wide(r"C:\w\a.txt"), wide(r"C:\日本 語\b")];
        let bytes = dropfiles_bytes(&paths);
        assert_eq!(&bytes[0..4], &20u32.to_le_bytes(), "pFiles は頭の直後");
        assert_eq!(&bytes[16..20], &1i32.to_le_bytes(), "fWide = UTF-16");
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0], "最後は二重 NUL");
        assert_eq!(
            bytes.len(),
            20 + (paths[0].len() + 1 + paths[1].len() + 1 + 1) * 2
        );
        assert_eq!(parse_dropfiles(&bytes), paths);
        // ANSI 形（fWide = 0）と壊れた中身
        let mut ansi = bytes[..20].to_vec();
        ansi[16..20].copy_from_slice(&0i32.to_le_bytes());
        ansi.extend_from_slice(b"C:\\x.txt\0\0");
        assert_eq!(parse_dropfiles(&ansi), vec![wide(r"C:\x.txt")]);
        assert!(parse_dropfiles(&bytes[..10]).is_empty());
        let mut far = bytes.clone();
        far[0..4].copy_from_slice(&9999u32.to_le_bytes());
        assert!(parse_dropfiles(&far).is_empty());
    }

    #[test]
    fn エクスプローラーの切り取りだけを移動と読む() {
        // エクスプローラーのコピーは COPY | LINK（5）、切り取りは MOVE（2）
        assert!(effect_is_cut(2));
        assert!(!effect_is_cut(5));
        assert!(!effect_is_cut(1));
        assert!(!effect_is_cut(3));
    }

    #[test]
    fn テキストはパスを改行で並べる() {
        assert_eq!(
            text_of(&[PathBuf::from("/w/a.txt"), PathBuf::from("/w/b c")]),
            "/w/a.txt\n/w/b c"
        );
    }

    /// 実 NSPasteboard の往復（**一意な名前のペーストボード**で行う = ユーザーの
    /// クリップボードには触らない）。書いた URL が Finder の読む形（NSURL のファイル URL）で
    /// 読み戻せること・変更番号が進むこと・自分が書いたままのときだけ空にすること
    #[cfg(target_os = "macos")]
    #[test]
    fn macosのペーストボードへファイルを書いて読み戻せる() {
        let Some(name) = imp::unique_name() else {
            eprintln!("NSPasteboard が使えない環境（検査の対象外）");
            return;
        };
        assert_ne!(
            name, "Apple CFPasteboard general",
            "一般のペーストボードを使っていない"
        );
        let dir = std::env::temp_dir().join(format!("tako-pb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("日本 語"));
        let file = dir.join("日本 語/a b.txt");
        std::fs::write(&file, "x").unwrap();
        let paths = vec![file.clone(), dir.join("日本 語")];
        let stamp = imp::write(Some(&name), &paths, false).expect("書ける");
        let back = imp::read(Some(&name));
        assert_eq!(back.stamp, Some(stamp));
        assert_eq!(back.paths, paths);
        assert!(!back.cut);
        // 自分が書いたままなら空にする・誰かが書き換えた後は空にしない
        assert!(!imp::clear_if(Some(&name), stamp + 100));
        assert_eq!(imp::read(Some(&name)).paths.len(), 2);
        assert!(imp::clear_if(Some(&name), stamp));
        assert!(imp::read(Some(&name)).paths.is_empty());
        imp::release_unique(&name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

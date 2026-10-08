//! ファイルを写すときの OS 差（抽象境界 B28 の core 分。Issue #1860 / #1895）
//!
//! シンボリックリンクを**リンクそのもの**として写す口（#1860）と、1 つのファイルを
//! **排他的に作って写す**口（[`copy_file_exclusive`]。#1895）を置く。
//!
//! - リンク: unix は `symlink(2)` 1 つで済むが、Windows はリンクの指す先がフォルダかファイルかで
//!   作る API が分かれ（`CreateSymbolicLinkW` の `SYMBOLIC_LINK_FLAG_DIRECTORY`）、しかも
//!   **開発者モードか管理者でないと作れない**（`ERROR_PRIVILEGE_NOT_HELD` = 1314）。後者は権限の無い
//!   書き込み先と区別して理由を出すため、判定もここへ置く
//! - ファイル: `std::fs::copy` は進み具合を返さず途中で止められない（巨大な 1 ファイルの途中で
//!   取り消せない）うえ、既存のファイルを黙って上書きする。ここでは OS の写す口を直に呼び、
//!   写す単位ごとに進み具合を渡して、偽が返ったらそのファイルの途中で止める
//!   （macOS = `copyfile(3)` の `fcopyfile` + 状態コールバック / Windows = `CopyFileExW` +
//!   進み具合ルーチン）。同じ APFS ボリュームなら先に OS の複製（`fclonefileat`）を試す
//!
//! 呼び出し側（`tako_core::file_copy`）に `cfg` を書かない（`platform` の原則）。

use std::path::Path;

/// `src`（シンボリックリンク）と同じ先を指すリンクを `dst` に作る。
///
/// `dst` が既にあれば `AlreadyExists` で失敗する（`symlink(2)` / `CreateSymbolicLinkW` は
/// 上書きしない = 名前を押さえる操作として使える）
pub fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    imp::symlink(&target, src, dst)
}

/// リンクを作る権限が無い失敗か（Windows の `ERROR_PRIVILEGE_NOT_HELD`）
pub fn symlink_privilege_missing(error: &std::io::Error) -> bool {
    imp::privilege_missing(error)
}

// --- 1 つのファイルを写す（#1895） ------------------------------------------------

/// 1 つのファイルをどう写したか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRoute {
    /// OS の複製（macOS の APFS clone）。中身を写さず一瞬で終わる = 途中が無い
    Clone,
    /// 中身を写した（macOS = `fcopyfile` / Windows = `CopyFileExW`）。進み具合が届き途中で止まる
    Data,
}

/// 1 つのファイルを写せなかった理由。**どちらも作りかけは消してある**
#[derive(Debug)]
pub enum FileCopyError {
    /// 進み具合の口が偽を返した（取り消し）
    Cancelled,
    /// OS が断った。写す先が既にあれば `AlreadyExists`（上書きしない・触らない）
    Io(std::io::Error),
}

/// `src` の中身と属性（権限・拡張属性）を `dst` へ写し、写したバイト数とどう写したかを返す。
///
/// - **`dst` は排他的に作る**: 既にあれば `AlreadyExists` で断り上書きしない（名前を決めてから
///   作るまでに誰かが同じ名前を作っても潰さない = #1860 の不変条件）
/// - `allow_clone` が真なら先に OS の複製を試す（macOS = `fclonefileat`。同じ APFS ボリュームなら
///   中身を写さず一瞬で終わる = Finder の複製と同じ）。できなければ（別のボリューム・APFS 以外）
///   中身を写す。Windows は `CopyFileExW` が ReFS のブロック複製を自分で選ぶので見ない
/// - 中身を写す間は OS の写す単位ごと（macOS は 1 MiB）に `on_progress(このファイルで写した
///   バイトの累計)` を呼ぶ（累計は減らない）。**偽を返すとそのファイルの途中で止まり**
///   [`FileCopyError::Cancelled`]
/// - 失敗・取り消しのときは、**この呼び出しで作った `dst`** を消してから返す（作りかけを残さない）
pub fn copy_file_exclusive(
    src: &Path,
    dst: &Path,
    allow_clone: bool,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(u64, FileRoute), FileCopyError> {
    file_imp::copy(src, dst, allow_clone, on_progress)
}

/// 中身を写す途中の状態（OS のコールバックへ生ポインタで渡す）
struct ProgressCtx<'a> {
    on_progress: &'a mut dyn FnMut(u64) -> bool,
    /// 進み具合の口が偽を返して止めた
    stopped: bool,
    /// 届いた累計の最大（OS が同じ値や小さい値を返しても累計は戻さない）
    copied: u64,
}

impl ProgressCtx<'_> {
    /// 続けるなら真
    fn report(&mut self, copied: u64) -> bool {
        self.copied = self.copied.max(copied);
        if (self.on_progress)(self.copied) {
            true
        } else {
            self.stopped = true;
            false
        }
    }
}

#[cfg(target_os = "macos")]
mod file_imp {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{FileCopyError, FileRoute, ProgressCtx};

    pub(super) fn copy(
        src: &Path,
        dst: &Path,
        allow_clone: bool,
        on_progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(u64, FileRoute), FileCopyError> {
        let reader = std::fs::File::open(src).map_err(FileCopyError::Io)?;
        let len = reader.metadata().map_err(FileCopyError::Io)?.len();
        if allow_clone {
            let to = CString::new(dst.as_os_str().as_bytes()).map_err(|_| {
                FileCopyError::Io(std::io::Error::from(std::io::ErrorKind::InvalidInput))
            })?;
            // SAFETY: 開いている fd と NUL 終端のパスを渡すだけ。`fclonefileat` は写す先が
            // 既にあれば EEXIST で断る（= 排他的に作る）
            let rc =
                unsafe { libc::fclonefileat(reader.as_raw_fd(), libc::AT_FDCWD, to.as_ptr(), 0) };
            if rc == 0 {
                return Ok((len, FileRoute::Clone));
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                // 別のボリューム・APFS 以外・複製の無い OS は中身を写す（`std::fs::copy` と同じ見分け）
                Some(libc::ENOTSUP | libc::EXDEV | libc::ENOSYS) => {}
                // EEXIST（同名がある）を含め、それ以外は断る。作っていないので消すものは無い
                _ => return Err(FileCopyError::Io(error)),
            }
        }
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)
            .map_err(FileCopyError::Io)?;
        let mut ctx = ProgressCtx {
            on_progress,
            stopped: false,
            copied: 0,
        };
        let result = fcopyfile(&reader, &writer, &mut ctx);
        drop(writer);
        match result {
            Ok(copied) => Ok((copied, FileRoute::Data)),
            Err(error) => {
                // ここまで来たら `dst` はこの呼び出しが `create_new` で作ったもの
                let _ = std::fs::remove_file(dst);
                Err(if ctx.stopped {
                    FileCopyError::Cancelled
                } else {
                    FileCopyError::Io(error)
                })
            }
        }
    }

    /// `copyfile_state_t` を落とし忘れない器
    struct State(libc::copyfile_state_t);

    impl Drop for State {
        fn drop(&mut self) {
            // SAFETY: `copyfile_state_alloc` が返した非 NULL のものだけを持つ
            unsafe { libc::copyfile_state_free(self.0) };
        }
    }

    /// 中身と属性を写す（`std::fs::copy` の `fcopyfile` の段と同じフラグ）。写す単位ごとに
    /// `ctx` へ進み具合を渡し、偽が返ったら `COPYFILE_QUIT` で止める（errno = ECANCELED）
    fn fcopyfile(
        reader: &std::fs::File,
        writer: &std::fs::File,
        ctx: &mut ProgressCtx<'_>,
    ) -> std::io::Result<u64> {
        // SAFETY: 引数の無い確保。NULL は失敗
        let raw = unsafe { libc::copyfile_state_alloc() };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let state = State(raw);
        let callback: extern "C" fn(
            libc::c_int,
            libc::c_int,
            libc::copyfile_state_t,
            *const libc::c_char,
            *const libc::c_char,
            *mut libc::c_void,
        ) -> libc::c_int = status;
        // SAFETY: `COPYFILE_STATE_STATUS_CB` は関数ポインタそのもの、`_CTX` はコールバックへ
        // 渡すポインタそのものを受け取る（ポインタのポインタではない。copyfile(3)）。
        // `ctx` はこの関数を抜けるまで生きている
        unsafe {
            libc::copyfile_state_set(
                state.0,
                libc::COPYFILE_STATE_STATUS_CB as u32,
                callback as *const libc::c_void,
            );
            libc::copyfile_state_set(
                state.0,
                libc::COPYFILE_STATE_STATUS_CTX as u32,
                (ctx as *mut ProgressCtx<'_>).cast::<libc::c_void>(),
            );
        }
        let flags = libc::COPYFILE_METADATA | libc::COPYFILE_DATA;
        // SAFETY: 開いている fd 2 つと確保済みの state
        let rc = unsafe { libc::fcopyfile(reader.as_raw_fd(), writer.as_raw_fd(), state.0, flags) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut copied: libc::off_t = 0;
        // SAFETY: `COPYFILE_STATE_COPIED` は off_t を書く
        unsafe {
            libc::copyfile_state_get(
                state.0,
                libc::COPYFILE_STATE_COPIED as u32,
                (&mut copied as *mut libc::off_t).cast::<libc::c_void>(),
            );
        }
        Ok(u64::try_from(copied).unwrap_or(0).max(ctx.copied))
    }

    /// `copyfile(3)` の状態コールバック。中身を写す段（`COPYFILE_COPY_DATA` の
    /// `COPYFILE_PROGRESS`）だけを見て、累計を渡し、偽なら止める
    extern "C" fn status(
        what: libc::c_int,
        stage: libc::c_int,
        state: libc::copyfile_state_t,
        _src: *const libc::c_char,
        _dst: *const libc::c_char,
        ctx: *mut libc::c_void,
    ) -> libc::c_int {
        if what != libc::COPYFILE_COPY_DATA || stage != libc::COPYFILE_PROGRESS || ctx.is_null() {
            return libc::COPYFILE_CONTINUE;
        }
        let mut copied: libc::off_t = 0;
        // SAFETY: `ctx` は `fcopyfile` が渡した `ProgressCtx`（呼び出しの間だけ生きる）。
        // `COPYFILE_STATE_COPIED` は off_t を書く
        let ctx = unsafe {
            libc::copyfile_state_get(
                state,
                libc::COPYFILE_STATE_COPIED as u32,
                (&mut copied as *mut libc::off_t).cast::<libc::c_void>(),
            );
            &mut *ctx.cast::<ProgressCtx<'_>>()
        };
        if ctx.report(u64::try_from(copied).unwrap_or(0)) {
            libc::COPYFILE_CONTINUE
        } else {
            libc::COPYFILE_QUIT
        }
    }
}

#[cfg(windows)]
mod file_imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{FileCopyError, FileRoute, ProgressCtx};

    type Bool = i32;
    type Handle = *mut std::ffi::c_void;
    /// `LPPROGRESS_ROUTINE`（LARGE_INTEGER は値渡しの i64）
    type ProgressRoutine = unsafe extern "system" fn(
        total_file_size: i64,
        total_bytes_transferred: i64,
        stream_size: i64,
        stream_bytes_transferred: i64,
        stream_number: u32,
        callback_reason: u32,
        source: Handle,
        destination: Handle,
        data: *const std::ffi::c_void,
    ) -> u32;

    /// 写す先が既にあれば断る（上書きしない = 排他的に作る）
    const COPY_FILE_FAIL_IF_EXISTS: u32 = 0x0000_0001;
    const PROGRESS_CONTINUE: u32 = 0;
    /// 止めて写す先を消す（CopyFileExW が自分で消す）
    const PROGRESS_CANCEL: u32 = 1;
    const ERROR_FILE_EXISTS: i32 = 80;
    const ERROR_ALREADY_EXISTS: i32 = 183;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CopyFileExW(
            existing: *const u16,
            new: *const u16,
            progress: Option<ProgressRoutine>,
            data: *const std::ffi::c_void,
            cancel: *mut Bool,
            flags: u32,
        ) -> Bool;
    }

    /// 長いパス（`MAX_PATH` 超え）でも通る形（`\\?\` の verbatim）の NUL 終端ワイド文字列
    fn wide(path: &Path) -> std::io::Result<Vec<u16>> {
        let abs = std::path::absolute(path)?;
        let text = abs.as_os_str().encode_wide().collect::<Vec<u16>>();
        let starts = |prefix: &str| {
            let p: Vec<u16> = prefix.encode_utf16().collect();
            text.starts_with(&p)
        };
        let mut out: Vec<u16> = if starts(r"\\?\") || starts(r"\\.\") {
            text
        } else if starts(r"\\") {
            // UNC（\\server\share）は \\?\UNC\server\share
            r"\\?\UNC"
                .encode_utf16()
                .chain(text[1..].iter().copied())
                .collect()
        } else {
            r"\\?\".encode_utf16().chain(text).collect()
        };
        out.push(0);
        Ok(out)
    }

    pub(super) fn copy(
        src: &Path,
        dst: &Path,
        _allow_clone: bool,
        on_progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(u64, FileRoute), FileCopyError> {
        let from = wide(src).map_err(FileCopyError::Io)?;
        let to = wide(dst).map_err(FileCopyError::Io)?;
        // 呼ぶ前から在ったものは、どんな失敗でも消さない（作ったものだけを戻す）
        let existed = std::fs::symlink_metadata(dst).is_ok();
        let mut ctx = ProgressCtx {
            on_progress,
            stopped: false,
            copied: 0,
        };
        // SAFETY: NUL 終端のワイド文字列 2 つと、この呼び出しの間だけ生きる `ctx`
        let ok = unsafe {
            CopyFileExW(
                from.as_ptr(),
                to.as_ptr(),
                Some(routine),
                (&mut ctx as *mut ProgressCtx<'_>).cast::<std::ffi::c_void>(),
                std::ptr::null_mut(),
                COPY_FILE_FAIL_IF_EXISTS,
            )
        };
        if ok != 0 {
            let len = std::fs::metadata(dst)
                .map(|m| m.len())
                .unwrap_or(ctx.copied);
            return Ok((len.max(ctx.copied), FileRoute::Data));
        }
        let error = std::io::Error::last_os_error();
        let exists = matches!(
            error.raw_os_error(),
            Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS)
        );
        if !existed && !exists {
            // PROGRESS_CANCEL は CopyFileExW が自分で消す。それ以外の途中の失敗も残さない
            let _ = std::fs::remove_file(dst);
        }
        Err(if ctx.stopped {
            FileCopyError::Cancelled
        } else if exists {
            FileCopyError::Io(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        } else {
            FileCopyError::Io(error)
        })
    }

    /// `CopyFileExW` の進み具合ルーチン。累計（全ストリーム）を渡し、偽なら止める
    unsafe extern "system" fn routine(
        _total_file_size: i64,
        total_bytes_transferred: i64,
        _stream_size: i64,
        _stream_bytes_transferred: i64,
        _stream_number: u32,
        _callback_reason: u32,
        _source: Handle,
        _destination: Handle,
        data: *const std::ffi::c_void,
    ) -> u32 {
        if data.is_null() {
            return PROGRESS_CONTINUE;
        }
        // SAFETY: `data` は `copy` が渡した `ProgressCtx`（CopyFileExW の呼び出しの間だけ生きる）
        let ctx = unsafe { &mut *(data as *mut ProgressCtx<'_>) };
        if ctx.report(u64::try_from(total_bytes_transferred).unwrap_or(0)) {
            PROGRESS_CONTINUE
        } else {
            PROGRESS_CANCEL
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod file_imp {
    use std::io::{Read, Write};
    use std::path::Path;

    use super::{FileCopyError, FileRoute, ProgressCtx};

    /// 対象外の OS（tako の配布先ではない）。読んで書くだけ（1 MiB ずつ・途中で止まる）
    pub(super) fn copy(
        src: &Path,
        dst: &Path,
        _allow_clone: bool,
        on_progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(u64, FileRoute), FileCopyError> {
        let mut reader = std::fs::File::open(src).map_err(FileCopyError::Io)?;
        let permissions = reader.metadata().map_err(FileCopyError::Io)?.permissions();
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)
            .map_err(FileCopyError::Io)?;
        let mut ctx = ProgressCtx {
            on_progress,
            stopped: false,
            copied: 0,
        };
        let mut buf = vec![0u8; 1 << 20];
        let result = loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => n,
                Err(e) => break Err(FileCopyError::Io(e)),
            };
            if let Err(e) = writer.write_all(&buf[..n]) {
                break Err(FileCopyError::Io(e));
            }
            if !ctx.report(ctx.copied + n as u64) {
                break Err(FileCopyError::Cancelled);
            }
        };
        drop(writer);
        match result {
            Ok(()) => {
                let _ = std::fs::set_permissions(dst, permissions);
                Ok((ctx.copied, FileRoute::Data))
            }
            Err(e) => {
                let _ = std::fs::remove_file(dst);
                Err(e)
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub(super) fn symlink(target: &Path, _src: &Path, dst: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, dst)
    }

    pub(super) fn privilege_missing(_error: &std::io::Error) -> bool {
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    /// `ERROR_PRIVILEGE_NOT_HELD`
    const PRIVILEGE_NOT_HELD: i32 = 1314;

    pub(super) fn symlink(target: &Path, src: &Path, dst: &Path) -> std::io::Result<()> {
        // 指す先がフォルダならディレクトリのリンク（辿れない = 壊れたリンクはファイル扱い）
        if std::fs::metadata(src).is_ok_and(|m| m.is_dir()) {
            std::os::windows::fs::symlink_dir(target, dst)
        } else {
            std::os::windows::fs::symlink_file(target, dst)
        }
    }

    pub(super) fn privilege_missing(error: &std::io::Error) -> bool {
        error.raw_os_error() == Some(PRIVILEGE_NOT_HELD)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    pub(super) fn symlink(_target: &Path, _src: &Path, _dst: &Path) -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }

    pub(super) fn privilege_missing(_error: &std::io::Error) -> bool {
        false
    }
}

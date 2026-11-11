//! ファイルのメタデータの OS 差（抽象境界 B37。Issue #2861 / #2921。B28 の `fs_copy` / `fs_move` の隣）
//!
//! ファイルの**同一性**・**状態変更時刻**・**実行ビット**・**symlink**・**置き換えて保存する**の
//! 5 つは、unix ではファイルの属性（`st_dev` / `st_ino` / `st_ctime` / mode / `symlink(2)` /
//! 同じフォルダの rename）で表せるが、Windows には同じ形のものが無い。呼び出し側
//! （usage の索引・利用者タスクの置き場の版・remote daemon の世代・言語サーバの取得・
//! `tako setup bootstrap`・`tako setup` の CLI の置き場・エディタの保存・テストの偽の実行ファイル）に
//! `cfg` を書かず、この境界の口を 1 本呼ぶ（`platform` の原則）。
//!
//! ## OS ごとの中身（Windows の同一性・状態変更時刻・置き換え保存は #2921）
//!
//! | 口 | unix | Windows |
//! |---|---|---|
//! | [`identity`] | `Some((st_dev, st_ino))` | `Some((ボリュームのシリアル, ファイル ID))`。属性だけを読むハンドル（中身の読み書きの共有を妨げない）を開いて `GetFileInformationByHandleEx(FileIdInfo)`、対応しないファイルシステムは `GetFileInformationByHandle`。開けない = **`None`**（呼び手は 0 で埋める = 比較に使わない） |
//! | [`change_time_secs`] | `Some(st_ctime)` | `Some(FileBasicInfo の ChangeTime)`（同じハンドルの問い合わせ）。開けない・0 を返すファイルシステム = **`None`** |
//! | [`is_executable`] | 通常ファイル かつ mode に実行ビット | 通常ファイルか**だけ**（実行ビットが無い。拡張子は見ない） |
//! | [`make_executable`] / [`EXECUTABLE_BY_MODE`] | mode を `0o755` に / `true` | **何もしない** / `false`（拡張子で決まる。shebang の台本は起こせない） |
//! | [`make_symlink`] | `Some(symlink(2) の結果)` | **`None`**（張らない。開発者モードか管理者の権限が要る） |
//! | [`save_replacing`] | 一時ファイル + 権限の引き継ぎ + rename（途中で落ちても元のファイルは残る） | 一時ファイル + `ReplaceFileW`（元のファイルの ACL・作成時刻・別のストリームを引き継ぐ。途中で落ちても元のファイルは残る）。元のファイルが無ければ rename。**置き換えられない**（他のプロセスが削除の共有を許さずに開いている等。短い間をおいて 5 回まで試す）・**一時ファイルを作れない**ときは #2921 前と同じその場の上書きへ倒す |
//!
//! Windows の置き換え保存がその場の上書きへ倒すのは、#2921 前に保存できた場面で保存を
//! 失敗させないため。その場の上書きが通るのは、開いている相手が書き込みの共有を許している
//! （= 同時の書き込みを受け入れている）ときだけなので、相手の前提も崩さない。
//! **一時ファイルへの書き込み自体の失敗（容量不足など）は倒さずに返す**（元のファイルを残す。
//! 倒すと元のファイルを切り詰めてから同じ理由で失敗しうる）。
//!
//! A/B: `TAKO_2921_LEGACY=1`（[`LEGACY_ENV`]）で Windows の 3 つの口を #2921 前（`None` /
//! `None` / その場の上書き）へ戻す。unix の腕は env を読まない。判定の本体は値を引数で受ける
//! （lib のテストは env を立てない = #2395）。

use std::path::{Path, PathBuf};

/// A/B: Windows の同一性・状態変更時刻・置き換え保存を #2921 前（`None` / `None` / その場の上書き）へ戻す。
/// unix の腕は読まない（変わらない）
pub const LEGACY_ENV: &str = "TAKO_2921_LEGACY";

/// #2921 前の挙動を再現するか（Windows の腕だけが読む）
#[cfg(windows)]
fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// ファイルの同一性（unix の `(st_dev, st_ino)` / Windows の `(ボリュームのシリアル, ファイル ID)`）。
/// 同じパスのまま中身が差し替わった（一時ファイルからの rename・別のファイルの移し込み）ことを、
/// 大きさと更新時刻が同じでも言える
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    /// ボリューム（unix の `st_dev` / Windows のボリュームのシリアル）
    pub dev: u64,
    /// inode（unix の `st_ino` / Windows のファイル ID。128 ビットの ID は 64 ビットへ畳む）
    pub ino: u64,
}

/// ファイルの同一性。
///
/// `path` と `m` は同じファイルを指すこと。`m` は `std::fs::metadata`（symlink を辿る）でも
/// `symlink_metadata`（辿らない）でも `DirEntry::metadata` でもよい（どちらの属性を読むかは
/// 呼び手が決める）。unix は `m` だけを読む（`path` は使わない）
#[cfg(unix)]
pub fn identity(_path: &Path, m: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt as _;
    Some(FileIdentity {
        dev: m.dev(),
        ino: m.ino(),
    })
}

/// ファイルの同一性。Windows は `path` の属性だけを読むハンドルを開いて問い合わせる
/// （`m` が symlink なら辿らずに開く = `symlink_metadata` と同じものを指す）。
/// 開けない・問い合わせが通らない = `None`（呼び手は 0 で埋める = 比較に使わない）
#[cfg(windows)]
pub fn identity(path: &Path, m: &std::fs::Metadata) -> Option<FileIdentity> {
    win::identity_with(path, m, legacy_mode())
}

/// 状態変更時刻（unix の `st_ctime`。epoch 秒）。`path` / `m` の約束は [`identity`] と同じ
#[cfg(unix)]
pub fn change_time_secs(_path: &Path, m: &std::fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt as _;
    Some(m.ctime())
}

/// 状態変更時刻（Windows の `FileBasicInfo` の `ChangeTime`。epoch 秒）。
/// 更新時刻と違って利用者の側から戻せない（中身・属性・時刻の書き換えで進む）。
/// 開けない・0 を返すファイルシステム = `None`
#[cfg(windows)]
pub fn change_time_secs(path: &Path, m: &std::fs::Metadata) -> Option<i64> {
    win::change_time_with(path, m, legacy_mode())
}

/// 128 ビットのファイル ID（Windows の `FILE_ID_128`。little-endian）を [`FileIdentity::ino`] へ畳む。
///
/// NTFS は 64 ビットのファイル参照番号を上位 0 で広げたもの = 下位 64 ビットをそのまま返す
/// （`GetFileInformationByHandle` の `nFileIndexHigh:Low` と同じ値）。ReFS の上位が 0 でない ID は
/// 上位を混ぜて畳む（同じパスの前後の 2 つを比べるだけなので、畳んだ値が偶然一致する確率は無視できる）
#[cfg(any(windows, test))]
fn fold_file_id(id: [u8; 16]) -> u64 {
    let mut low = [0u8; 8];
    let mut high = [0u8; 8];
    low.copy_from_slice(&id[..8]);
    high.copy_from_slice(&id[8..]);
    let (low, high) = (u64::from_le_bytes(low), u64::from_le_bytes(high));
    if high == 0 {
        low
    } else {
        low ^ high.rotate_left(29).wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }
}

/// Windows の時刻（1601-01-01 からの 100 ns 単位）を unix epoch 秒へ（unix の `st_ctime` と同じく切り捨て）。
/// 0 以下は「記録が無い」（ChangeTime を持たないファイルシステムは 0 を返す）= `None`
#[cfg(any(windows, test))]
fn filetime_to_unix_secs(filetime: i64) -> Option<i64> {
    /// 1601-01-01 から 1970-01-01 までの 100 ns の数
    const UNIX_EPOCH_AS_FILETIME: i64 = 116_444_736_000_000_000;
    (filetime > 0).then(|| (filetime - UNIX_EPOCH_AS_FILETIME).div_euclid(10_000_000))
}

/// 実行できるかをファイルの mode（実行ビット）で決める OS か。
///
/// `true`（unix）の OS では [`make_executable`] の後のファイル（shebang の台本を含む）を
/// そのまま起こせる。`false`（Windows）は拡張子で決まり、[`make_executable`] は何もしない
#[cfg(unix)]
pub const EXECUTABLE_BY_MODE: bool = true;

/// 実行できるかをファイルの mode（実行ビット）で決める OS か（Windows は `false` = 拡張子で決まる）
#[cfg(not(unix))]
pub const EXECUTABLE_BY_MODE: bool = false;

/// 実行できる通常ファイルか（symlink は辿る）。unix は mode の実行ビットを見る
#[cfg(unix)]
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// 実行できる通常ファイルか。**Windows は通常ファイルかだけを見る**（実行ビットが無い。拡張子は見ない）
#[cfg(not(unix))]
pub fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 書いたファイルを実行できるようにする（unix だけ。Windows は拡張子で決まる）
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// `target` を指す symlink を `link` に張る。
///
/// **Windows は `None`**（張らない = 開発者モードか管理者の権限が要る）。呼び手は `None` を
/// 「この OS では張らない」と読み、黙って成功させないこと
#[cfg(unix)]
pub fn make_symlink(target: &Path, link: &Path) -> Option<std::io::Result<()>> {
    Some(std::os::unix::fs::symlink(target, link))
}

/// `target` を指す symlink を `link` に張る。**Windows は `None`**（張らない）
#[cfg(not(unix))]
pub fn make_symlink(_target: &Path, _link: &Path) -> Option<std::io::Result<()>> {
    None
}

/// 置き換え保存の一時ファイルの置き場（同じフォルダ = 同じボリューム。名前に pid と時刻を入れて
/// 並走する保存と取り合わない）
fn save_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(".{name}.tako-save-{}-{nonce}", std::process::id()))
}

/// `path` の中身を `bytes` に置き換えて保存する（エディタの保存・ワークスペースの置換）。
///
/// unix は同じフォルダの一時ファイルへ書いて `sync` し、元のファイルの権限を引き継いでから
/// rename する（途中で落ちても元のファイルは残る）。元のファイルが無ければ一時ファイルの既定の
/// 権限のまま置く（#1659）
#[cfg(unix)]
pub fn save_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = save_temp_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        match std::fs::metadata(path) {
            Ok(metadata) => std::fs::set_permissions(&temp, metadata.permissions())?,
            // 外で消されたファイルを上書き保存で作り直す（#1659）。引き継ぐ権限が無いので
            // 一時ファイルの既定（新規ファイルと同じ）のまま置く
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// `path` の中身を `bytes` に置き換えて保存する。Windows は同じフォルダの一時ファイルへ書いて
/// `sync` し、`ReplaceFileW` で置き換える（元のファイルが無ければ rename。#1659）。
/// 置き換えられない・一時ファイルを作れないときはその場の上書きへ倒す（モジュールの doc）
#[cfg(windows)]
pub fn save_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    win::save_replacing_with(path, bytes, legacy_mode(), None)
}

/// Windows の腕の本体（同一性・状態変更時刻・置き換え保存）。A/B（`legacy`）と書き込みの失敗の
/// 注入（`fault`）は引数で受ける（lib のテストが env を立てずに両方の腕を回せる = #2395）
#[cfg(windows)]
mod win {
    use super::FileIdentity;
    use std::io::Write as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;

    type Bool = i32;
    type Handle = *mut std::ffi::c_void;

    /// 属性だけを読む（中身の読み書き・削除の共有の判定に入らない = 他のプロセスと共有違反を起こさない）
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    /// 読み・書き・削除のすべてを共有する（こちらが相手の開き方を妨げない）
    const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    /// フォルダも開ける
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    /// symlink を辿らずにそのものを開く
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    /// `FILE_INFO_BY_HANDLE_CLASS::FileBasicInfo`
    const FILE_BASIC_INFO_CLASS: i32 = 0;
    /// `FILE_INFO_BY_HANDLE_CLASS::FileIdInfo`
    const FILE_ID_INFO_CLASS: i32 = 18;
    /// 置き換えで別のストリーム・属性を移せなくても置き換える（中身の保存を優先する）
    const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;
    /// 置き換えで ACL を移せなくても置き換える（ACL を書き換える権限の無いファイルも保存できる）
    const REPLACEFILE_IGNORE_ACL_ERRORS: u32 = 0x4;

    /// 置き換えを試し直す前の間（ミリ秒。合計 150 ms）。ウイルス対策・索引・同期のクライアントが
    /// 一瞬だけ削除の共有を許さずに開いている間を待つ
    pub(super) const REPLACE_WAITS_MS: [u64; 4] = [10, 20, 40, 80];
    /// 試し直す失敗（`ERROR_ACCESS_DENIED` / `ERROR_SHARING_VIOLATION` / `ERROR_LOCK_VIOLATION` /
    /// `ERROR_UNABLE_TO_REMOVE_REPLACED` / `ERROR_UNABLE_TO_MOVE_REPLACEMENT`）
    const TRANSIENT_ERRORS: [i32; 5] = [5, 32, 33, 1175, 1176];

    /// `FILE_BASIC_INFO`
    #[repr(C)]
    #[derive(Default)]
    struct FileBasicInfo {
        _creation_time: i64,
        _last_access_time: i64,
        _last_write_time: i64,
        change_time: i64,
        _file_attributes: u32,
    }

    /// `FILE_ID_INFO`
    #[repr(C)]
    #[derive(Default)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    /// `FILETIME`
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        _low: u32,
        _high: u32,
    }

    /// `BY_HANDLE_FILE_INFORMATION`
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        _file_attributes: u32,
        _creation_time: FileTime,
        _last_access_time: FileTime,
        _last_write_time: FileTime,
        volume_serial_number: u32,
        _file_size_high: u32,
        _file_size_low: u32,
        _number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: Handle,
            class: i32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> Bool;
        fn GetFileInformationByHandle(file: Handle, info: *mut ByHandleFileInformation) -> Bool;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> Bool;
    }

    /// 属性だけを読むハンドル。`follow` が偽なら symlink を辿らずにそのものを開く
    fn open_attributes(path: &Path, follow: bool) -> std::io::Result<std::fs::File> {
        let reparse = if follow {
            0
        } else {
            FILE_FLAG_OPEN_REPARSE_POINT
        };
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | reparse)
            .open(path)
    }

    /// `GetFileInformationByHandleEx` で `T` を読む（通らなければ `None`）
    fn query<T: Default>(file: &std::fs::File, class: i32) -> Option<T> {
        let mut info = T::default();
        let size = u32::try_from(std::mem::size_of::<T>()).ok()?;
        // SAFETY: `file` は開いたままのハンドル、`info` は `class` が書く大きさ（`size`）の
        // `repr(C)` の値で、呼び出しのあいだ生きている
        let ok = unsafe {
            GetFileInformationByHandleEx(file.as_raw_handle(), class, (&raw mut info).cast(), size)
        };
        (ok != 0).then_some(info)
    }

    /// 開いたハンドルの同一性（`FileIdInfo`、対応しないファイルシステムは `GetFileInformationByHandle`）
    fn identity_of(file: &std::fs::File) -> Option<FileIdentity> {
        if let Some(info) = query::<FileIdInfo>(file, FILE_ID_INFO_CLASS) {
            return Some(FileIdentity {
                dev: info.volume_serial_number,
                ino: super::fold_file_id(info.file_id),
            });
        }
        let mut info = ByHandleFileInformation::default();
        // SAFETY: `file` は開いたままのハンドル、`info` は `BY_HANDLE_FILE_INFORMATION` と同じ配置
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut info) };
        (ok != 0).then(|| FileIdentity {
            dev: u64::from(info.volume_serial_number),
            ino: (u64::from(info.file_index_high) << 32) | u64::from(info.file_index_low),
        })
    }

    /// [`super::identity`] の本体（`legacy` = #2921 前の `None`）
    pub(super) fn identity_with(
        path: &Path,
        m: &std::fs::Metadata,
        legacy: bool,
    ) -> Option<FileIdentity> {
        if legacy {
            return None;
        }
        let file = open_attributes(path, !m.file_type().is_symlink()).ok()?;
        identity_of(&file)
    }

    /// [`super::change_time_secs`] の本体（`legacy` = #2921 前の `None`）
    pub(super) fn change_time_with(
        path: &Path,
        m: &std::fs::Metadata,
        legacy: bool,
    ) -> Option<i64> {
        if legacy {
            return None;
        }
        let file = open_attributes(path, !m.file_type().is_symlink()).ok()?;
        let info = query::<FileBasicInfo>(&file, FILE_BASIC_INFO_CLASS)?;
        super::filetime_to_unix_secs(info.change_time)
    }

    /// 中身を書く。`fault = Some(n)` は n バイト書いたところで失敗させる（テストの注入。本番は `None`）
    fn write_with_fault(
        file: &mut std::fs::File,
        bytes: &[u8],
        fault: Option<usize>,
    ) -> std::io::Result<()> {
        match fault {
            Some(n) if n < bytes.len() => {
                file.write_all(&bytes[..n])?;
                Err(std::io::Error::other(
                    "注入した書き込みの失敗（#2921 のテスト）",
                ))
            }
            _ => file.write_all(bytes),
        }
    }

    /// その場で上書きする（#2921 前の Windows の腕。途中で落ちると中身が欠ける）
    fn overwrite_in_place(path: &Path, bytes: &[u8], fault: Option<usize>) -> std::io::Result<()> {
        // `create`: 外で消されたファイルを上書き保存で作り直す（#1659）。
        // 消えたファイルへの通常の保存は `save_inner` が書く前に断る
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        write_with_fault(&mut file, bytes, fault)?;
        file.sync_all()
    }

    /// 書き終えた一時ファイルで `path` を置き換える（共有違反は短い間をおいて試し直す）
    fn replace(path: &Path, temp: &Path) -> std::io::Result<()> {
        // 元のファイルが無い（外で消された = #1659 の作り直し）: 置き換える相手が無いので rename
        if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return std::fs::rename(temp, path);
        }
        let replaced = crate::platform::fs_copy::wide(path)?;
        let replacement = crate::platform::fs_copy::wide(temp)?;
        let mut waits = REPLACE_WAITS_MS.iter();
        loop {
            // SAFETY: どちらも NUL 終端のワイド文字列で呼び出しのあいだ生きている。
            // 退避先・除外・予約は使わない（null）
            let ok = unsafe {
                ReplaceFileW(
                    replaced.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            // 確かめた後に消された: 置き換える相手が無いので rename
            if error.kind() == std::io::ErrorKind::NotFound {
                return std::fs::rename(temp, path);
            }
            let transient = error
                .raw_os_error()
                .is_some_and(|code| TRANSIENT_ERRORS.contains(&code));
            match waits.next() {
                Some(ms) if transient => std::thread::sleep(std::time::Duration::from_millis(*ms)),
                _ => return Err(error),
            }
        }
    }

    /// [`super::save_replacing`] の本体（`legacy` = #2921 前のその場の上書き・`fault` = テストの注入）
    pub(super) fn save_replacing_with(
        path: &Path,
        bytes: &[u8],
        legacy: bool,
        fault: Option<usize>,
    ) -> std::io::Result<()> {
        if legacy {
            return overwrite_in_place(path, bytes, fault);
        }
        let temp = super::save_temp_path(path);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            // フォルダにファイルを作れない（ファイルには書ける）= 書き込む前に分かる失敗。
            // #2921 前と同じその場の上書きへ倒す（モジュールの doc）
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return overwrite_in_place(path, bytes, fault);
            }
            Err(e) => return Err(e),
        };
        let written = write_with_fault(&mut file, bytes, fault).and_then(|()| file.sync_all());
        // 置き換えの前に閉じる（開いたままの一時ファイルは置き換えに使えない）
        drop(file);
        if let Err(e) = written {
            // 容量不足など: 倒さずに返す = 元のファイルはそのまま残る
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        if replace(path, &temp).is_ok() {
            return Ok(());
        }
        // 置き換えられない（削除の共有を許さずに開かれている等）: 一時ファイルを片付けて
        // #2921 前と同じその場の上書きへ倒す（一時ファイルの分の容量はここで空く）
        let _ = std::fs::remove_file(&temp);
        overwrite_in_place(path, bytes, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_residue::ScratchDir;

    #[test]
    fn 同一性は差し替えで変わり同じファイルでは変わらない() {
        let dir = ScratchDir::new("fs-meta-identity");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let first = identity(&path, &std::fs::metadata(&path).unwrap());
        assert_eq!(first, identity(&path, &std::fs::metadata(&path).unwrap()));
        // 同じ大きさのまま一時ファイルから差し替える
        let temp = dir.path().join("b.txt");
        std::fs::write(&temp, "y").unwrap();
        std::fs::rename(&temp, &path).unwrap();
        let second = identity(&path, &std::fs::metadata(&path).unwrap());
        // unix も Windows（#2921）も同一性を持つ
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second, "差し替えたのに同一性が変わらない");
    }

    #[test]
    fn 状態変更時刻を読める() {
        let dir = ScratchDir::new("fs-meta-ctime");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let ctime = change_time_secs(&path, &std::fs::metadata(&path).unwrap());
        // unix も Windows（#2921）も読める
        assert!(ctime.is_some());
    }

    #[test]
    fn issue2921_128ビットのファイルidの畳み方() {
        // NTFS（上位 0）は下位 64 ビットそのもの = `nFileIndexHigh:Low` と同じ値
        let mut ntfs = [0u8; 16];
        ntfs[..8].copy_from_slice(&0x0001_0000_0000_1234u64.to_le_bytes());
        assert_eq!(fold_file_id(ntfs), 0x0001_0000_0000_1234);
        // 上位が違えば下位が同じでも別の値（ReFS）
        let mut refs_a = ntfs;
        let mut refs_b = ntfs;
        refs_a[8] = 1;
        refs_b[8] = 2;
        assert_ne!(fold_file_id(refs_a), fold_file_id(refs_b));
        assert_ne!(fold_file_id(refs_a), fold_file_id(ntfs));
    }

    #[test]
    fn issue2921_windowsの時刻をunixの秒へ() {
        // 1970-01-01T00:00:00Z
        assert_eq!(filetime_to_unix_secs(116_444_736_000_000_000), Some(0));
        // 秒未満は切り捨て（unix の st_ctime と同じ）
        assert_eq!(
            filetime_to_unix_secs(116_444_736_000_000_000 + 19_999_999),
            Some(1)
        );
        // 1970 より前は負の秒（切り捨て = 床）
        assert_eq!(filetime_to_unix_secs(116_444_736_000_000_000 - 1), Some(-1));
        // 記録が無い（ChangeTime を持たないファイルシステム）
        assert_eq!(filetime_to_unix_secs(0), None);
        assert_eq!(filetime_to_unix_secs(-5), None);
    }

    #[test]
    fn 実行ビットを付けると実行できると判定される() {
        let dir = ScratchDir::new("fs-meta-exec");
        let path = dir.path().join("run.sh");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        assert_eq!(is_executable(&path), !EXECUTABLE_BY_MODE, "付ける前");
        make_executable(&path).unwrap();
        assert!(is_executable(&path), "付けた後");
        // フォルダと存在しないパスは実行できない
        assert!(!is_executable(dir.path()));
        assert!(!is_executable(&dir.path().join("none")));
    }

    #[test]
    fn symlinkはunixだけ張る() {
        let dir = ScratchDir::new("fs-meta-symlink");
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("link");
        let made = make_symlink(&target, &link);
        assert_eq!(made.is_some(), cfg!(unix), "張るのは unix だけ");
        if let Some(result) = made {
            result.unwrap();
            assert_eq!(std::fs::read_link(&link).unwrap(), target);
            // 既に在れば失敗する（上書きしない）
            assert!(make_symlink(&target, &link).unwrap().is_err());
        }
    }

    #[test]
    fn 置き換え保存は中身を替え権限を引き継ぐ() {
        let dir = ScratchDir::new("fs-meta-save");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "old").unwrap();
        make_executable(&path).unwrap();
        save_replacing(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(is_executable(&path), "権限を引き継いでいない");
        // 一時ファイルを残さない
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // 消えたファイルは作り直す（#1659）
        std::fs::remove_file(&path).unwrap();
        save_replacing(&path, b"again").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "again");
    }

    /// Windows の腕（#2921）を Windows の上で実際に回す。CI の Windows ジョブが
    /// `cargo test -p tako-core --lib platform::fs_meta -- --nocapture` で名指しで走らせ、
    /// 結果の行（`TAKO2921_*`）をログへ残す。
    ///
    /// 「他のプロセスが開いている」は、同じプロセスの別のハンドルを共有モードを決めて開いて作る
    /// （共有の判定はハンドルごとでプロセスを問わない = 別のプロセスと同じ判定を通る）
    #[cfg(windows)]
    mod issue2921 {
        use super::super::win::{
            change_time_with, identity_with, save_replacing_with, REPLACE_WAITS_MS,
        };
        use super::super::*;
        use crate::test_residue::ScratchDir;
        use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};

        const SHARE_READ: u32 = 0x1;
        const SHARE_WRITE: u32 = 0x2;
        const SHARE_DELETE: u32 = 0x4;

        /// 「他のプロセス」が `share` の共有モードで読みに開いたまま持つハンドル
        fn hold(path: &Path, share: u32) -> std::fs::File {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(path)
                .unwrap_or_else(|e| panic!("共有モード {share:#x} で開けない: {e}"))
        }

        /// 置き場に残っている名前（一時ファイルの残りを見る）
        fn names(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        }

        #[test]
        fn issue2921_同じ大きさと更新時刻のまま差し替えたファイルを同一性で見分ける() {
            let dir = ScratchDir::new("fs-meta-2921-swap");
            let path = dir.path().join("a.txt");
            std::fs::write(&path, "x").unwrap();
            let before = std::fs::metadata(&path).unwrap();
            let first = identity(&path, &before).expect("Windows でも同一性を持つ");
            assert_eq!(
                Some(first),
                identity(&path, &std::fs::metadata(&path).unwrap()),
                "同じファイルを読み直しただけで同一性が変わった"
            );
            // 走査（usage の索引）の形 = `DirEntry::metadata` でも同じ値
            let entry = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap())
                .find(|e| e.path() == path)
                .unwrap();
            assert_eq!(
                identity(&entry.path(), &entry.metadata().unwrap()),
                Some(first)
            );
            // 他のプロセスが共有なし（0）で開いていても読める（属性だけのハンドルは共有の判定に入らない）
            {
                let _held = hold(&path, 0);
                assert_eq!(
                    identity(&path, &std::fs::metadata(&path).unwrap()),
                    Some(first),
                    "共有なしで開かれたファイルの同一性を読めない"
                );
            }

            // 同じ大きさ・同じ更新時刻の別のファイルを rename で差し替える（install の contents-swap /
            // 置き場の atomic write と同じ形）
            let staged = dir.path().join("b.txt");
            std::fs::write(&staged, "y").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&staged)
                .unwrap()
                .set_modified(before.modified().unwrap())
                .unwrap();
            std::fs::rename(&staged, &path).unwrap();
            let after = std::fs::metadata(&path).unwrap();
            assert_eq!(
                (after.len(), after.modified().unwrap()),
                (before.len(), before.modified().unwrap()),
                "前提: 大きさと更新時刻が同じ"
            );
            let second = identity(&path, &after).expect("差し替えた後も同一性を持つ");
            assert_ne!(
                first, second,
                "大きさと更新時刻が同じまま差し替わったファイルを見分けられない"
            );
            assert_eq!(first.dev, second.dev, "同じボリューム");
            // A/B: #2921 前の腕は `None` = 呼び手が 0 で埋めると前後が等しい = 見分けられない
            assert_eq!(identity_with(&path, &after, true), None);
            assert_eq!(identity_with(&path, &after, false), Some(second));
            eprintln!(
                "TAKO2921_IDENTITY same_len_mtime=true swap_detected={} same_dev={} legacy=None",
                first != second,
                first.dev == second.dev
            );
        }

        #[test]
        fn issue2921_symlinkは辿るかを属性の読み方に合わせる() {
            let dir = ScratchDir::new("fs-meta-2921-link");
            let target = dir.path().join("target.txt");
            std::fs::write(&target, "x").unwrap();
            let link = dir.path().join("link.txt");
            // symlink を張るには開発者モードか管理者の権限が要る。張れない環境ではこの節だけ見送る
            if let Err(e) = std::os::windows::fs::symlink_file(&target, &link) {
                eprintln!("TAKO2921_SYMLINK skipped（symlink を張れない: {e}）");
                return;
            }
            let of_target = identity(&target, &std::fs::metadata(&target).unwrap());
            let followed = identity(&link, &std::fs::metadata(&link).unwrap());
            let itself = identity(&link, &std::fs::symlink_metadata(&link).unwrap());
            assert!(of_target.is_some() && itself.is_some());
            assert_eq!(followed, of_target, "metadata は辿った先 = 実体");
            assert_ne!(itself, of_target, "symlink_metadata は symlink そのもの");
            eprintln!("TAKO2921_SYMLINK followed=target itself=link");
        }

        #[test]
        fn issue2921_状態変更時刻をwindowsでも読み_更新時刻を戻しても戻らない() {
            let dir = ScratchDir::new("fs-meta-2921-ctime");
            let path = dir.path().join("a.txt");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            std::fs::write(&path, "x").unwrap();
            // 更新時刻を 2001 年へ戻す（`ditto` / 展開ツールが更新時刻を保つのと同じ形）
            let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old)
                .unwrap();
            let m = std::fs::metadata(&path).unwrap();
            assert_eq!(m.modified().unwrap(), old, "前提: 更新時刻は 2001 年");
            let ctime = change_time_secs(&path, &m).expect("Windows でも状態変更時刻を読める");
            assert!(
                (ctime - now).abs() < 600,
                "状態変更時刻が今の前後 10 分に無い（更新時刻を読んでいる疑い）: ctime={ctime} now={now}"
            );
            // A/B: #2921 前の腕は `None`（remote daemon の世代の ctime の判定が Unknown になる）
            assert_eq!(change_time_with(&path, &m, true), None);
            eprintln!(
                "TAKO2921_CTIME some=true near_now={} mtime_rewound=true legacy=None",
                (ctime - now).abs() < 600
            );
        }

        #[test]
        fn issue2921_置き換え保存は途中で失敗しても元のファイルを残す() {
            let dir = ScratchDir::new("fs-meta-2921-fault");
            let path = dir.path().join("a.txt");
            let original = "元の中身（保存の途中で失敗しても残る）";
            let new = "新しい中身。こちらは途中で書けなくなる";
            std::fs::write(&path, original).unwrap();
            // 新しい腕: 一時ファイルへ 3 バイト書いたところで失敗させる
            let error = save_replacing_with(&path, new.as_bytes(), false, Some(3)).unwrap_err();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                original,
                "途中の失敗で元のファイルが欠けた: {error}"
            );
            assert_eq!(names(dir.path()), vec!["a.txt"], "一時ファイルが残った");
            // 失敗しなければ置き換わる（一時ファイルは残さない）
            save_replacing_with(&path, new.as_bytes(), false, None).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), new);
            assert_eq!(names(dir.path()), vec!["a.txt"]);
            // A/B: #2921 前の腕（その場の上書き）は同じ失敗で中身が欠ける
            std::fs::write(&path, original).unwrap();
            let _ = save_replacing_with(&path, new.as_bytes(), true, Some(3)).unwrap_err();
            let legacy = std::fs::read(&path).unwrap();
            assert_eq!(
                legacy,
                new.as_bytes()[..3].to_vec(),
                "#2921 前の腕は切り詰めてから書く（この差を名指すのがこのテスト）"
            );
            eprintln!("TAKO2921_SAVE_FAULT new=original_kept legacy=truncated_to_3_bytes");
        }

        #[test]
        fn issue2921_置き換え保存は元のファイルの作成時刻を引き継ぎ_消えたファイルは作り直す() {
            let dir = ScratchDir::new("fs-meta-2921-replace");
            let path = dir.path().join("a.txt");
            std::fs::write(&path, "old").unwrap();
            let before = std::fs::metadata(&path).unwrap();
            let id_before = identity(&path, &before).unwrap();
            // 一時ファイルの作成時刻が元と確かに違うように間を置く（NTFS の時刻の刻み）
            std::thread::sleep(std::time::Duration::from_millis(50));
            save_replacing(&path, b"new").unwrap();
            let after = std::fs::metadata(&path).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
            assert_eq!(
                after.creation_time(),
                before.creation_time(),
                "ReplaceFileW は元のファイルの作成時刻を引き継ぐ"
            );
            assert_eq!(names(dir.path()), vec!["a.txt"]);
            let id_after = identity(&path, &after).unwrap();
            // 消えたファイルは作り直す（#1659。置き換える相手が無いので rename）
            std::fs::remove_file(&path).unwrap();
            save_replacing(&path, b"again").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "again");
            assert_eq!(names(dir.path()), vec!["a.txt"]);
            eprintln!(
                "TAKO2921_SAVE_REPLACE creation_time_kept=true identity_changed={} recreated=true",
                id_before != id_after
            );
        }

        #[test]
        fn issue2921_他のプロセスが開いているときの置き換え保存() {
            let dir = ScratchDir::new("fs-meta-2921-held");
            let path = dir.path().join("a.txt");

            // (a) 読み・書き・削除のすべてを共有して開いている（std の既定・多くのツール）:
            //     置き換わるか、置き換えられなくてもその場の上書きで保存は通る
            std::fs::write(&path, "old-a").unwrap();
            let id_a = identity(&path, &std::fs::metadata(&path).unwrap()).unwrap();
            {
                let _held = hold(&path, SHARE_READ | SHARE_WRITE | SHARE_DELETE);
                save_replacing(&path, b"new-a").unwrap();
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-a");
            let a_identity_changed =
                identity(&path, &std::fs::metadata(&path).unwrap()) != Some(id_a);
            assert_eq!(names(dir.path()), vec!["a.txt"], "(a) 一時ファイルが残った");

            // (b) 削除の共有を許さずに開いている（書き込みは許す）: 置き換えられないので、
            //     試し直しの後に #2921 前と同じその場の上書きへ倒す = 保存は通り、同じファイルのまま
            std::fs::write(&path, "old-b").unwrap();
            let id_b = identity(&path, &std::fs::metadata(&path).unwrap()).unwrap();
            let started = std::time::Instant::now();
            {
                let _held = hold(&path, SHARE_READ | SHARE_WRITE);
                save_replacing(&path, b"new-b").unwrap();
            }
            let waited = started.elapsed();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-b");
            assert_eq!(
                identity(&path, &std::fs::metadata(&path).unwrap()),
                Some(id_b),
                "(b) その場の上書きへ倒したなら同じファイルのまま"
            );
            assert_eq!(names(dir.path()), vec!["a.txt"], "(b) 一時ファイルが残った");

            // (c) 読みしか共有しない（書き込みも削除も許さない）: 置き換えもその場の上書きも
            //     通らない = 失敗を返し、元のファイルはそのまま
            std::fs::write(&path, "old-c").unwrap();
            let error = {
                let _held = hold(&path, SHARE_READ);
                save_replacing(&path, b"new-c").unwrap_err()
            };
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "old-c",
                "(c) 保存に失敗したのに元のファイルが変わった: {error}"
            );
            assert_eq!(names(dir.path()), vec!["a.txt"], "(c) 一時ファイルが残った");
            let budget: u64 = REPLACE_WAITS_MS.iter().sum();
            eprintln!(
                "TAKO2921_SAVE_HELD share_all=saved(identity_changed={a_identity_changed}) \
                 share_rw=in_place_fallback({}ms/budget {budget}ms) share_r=refused({:?}) original_kept=true",
                waited.as_millis(),
                error.raw_os_error()
            );
        }

        #[test]
        fn issue2921_一時ファイルを作れないフォルダではその場の上書きへ倒す() {
            let dir = ScratchDir::new("fs-meta-2921-nodir");
            let path = dir.path().join("a.txt");
            std::fs::write(&path, "old").unwrap();
            // フォルダ「だけ」にファイルの追加（WD）を拒む ACE を足す（継承しない = a.txt には書ける）
            let icacls = |args: &[&str]| {
                let out = std::process::Command::new("icacls")
                    .arg(dir.path())
                    .args(args)
                    .output()
                    .expect("icacls を起こせない");
                assert!(
                    out.status.success(),
                    "icacls {args:?} が失敗: {}",
                    String::from_utf8_lossy(&out.stdout)
                );
            };
            icacls(&["/deny", "*S-1-1-0:(WD)"]);
            let probe = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.path().join("probe"));
            let saved = save_replacing(&path, b"new");
            icacls(&["/remove:d", "*S-1-1-0"]);
            let probe_kind = probe.as_ref().err().map(std::io::Error::kind);
            assert_eq!(
                probe_kind,
                Some(std::io::ErrorKind::PermissionDenied),
                "前提: フォルダにファイルを作れない"
            );
            saved.expect("一時ファイルを作れなくてもその場の上書きで保存できる（#2921 前と同じ）");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
            assert_eq!(names(dir.path()), vec!["a.txt"]);
            eprintln!("TAKO2921_SAVE_NODIR temp=denied in_place_fallback=saved");
        }
    }
}

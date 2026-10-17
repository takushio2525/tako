//! ファイル・フォルダを**上書きせずに**移すときの OS 差（抽象境界 B28 の core 分。Issue #1842）
//!
//! `rename(2)` / `std::fs::rename` は移動先に同じ名前があると**黙って置き換える**（既存の
//! ファイル・空のフォルダ。Windows の std も `MOVEFILE_REPLACE_EXISTING` 相当）。
//! 「同名が無いことを確かめてから rename する」の 2 段では、確かめてから rename までの
//! あいだに別のプロセスが同名を作ると上書きしてしまう（TOCTOU）。ここでは OS の
//! 「上書きしない rename」を直に呼び、**同名の有無の判定と移動を 1 つの操作**にする:
//!
//! - macOS: `renamex_np(2)` + `RENAME_EXCL`
//! - Linux: `renameat2(2)` + `RENAME_NOREPLACE`
//! - Windows: `MoveFileExW` を `MOVEFILE_REPLACE_EXISTING` を付けずに（フラグ 0。
//!   `MOVEFILE_COPY_ALLOWED` も付けないので、別のボリュームは `ERROR_NOT_SAME_DEVICE` で断られる）
//!
//! 移動先に同じ名前があれば `ErrorKind::AlreadyExists` で失敗し、**どちらも触らない**。
//! 別のボリュームは `ErrorKind::CrossesDevices`（std が EXDEV / `ERROR_NOT_SAME_DEVICE` を寄せる先）。
//!
//! ファイルシステムが排他の rename を持たないとき（macOS の `ENOTSUP`・Linux の古いカーネルや
//! 一部のファイルシステムの `EINVAL` / `ENOSYS`）は、**確かめてから rename する従来の形**へ落とす
//! （移動そのものは止めない = #1834 と同じ。隙間はそのファイルシステムでだけ残る）。
//!
//! 呼び出し側（`tako_core::file_move`）に `cfg` を書かない（`platform` の原則）。

use std::path::Path;

/// `from` を `to` へ移す。`to` に何か（ファイル・フォルダ・壊れたリンクも）があれば
/// `AlreadyExists` で断り、`from` も `to` も触らない
pub fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    match imp::rename_excl(from, to) {
        Err(e) if imp::unsupported(&e) => rename_checked(from, to),
        other => other,
    }
}

/// 排他の rename を持たないファイルシステム向けの従来の形（確かめてから rename）。
/// 確かめてから rename までの隙間は残る
fn rename_checked(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_path(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }

    pub(super) fn rename_excl(from: &Path, to: &Path) -> std::io::Result<()> {
        let (from, to) = (c_path(from)?, c_path(to)?);
        // SAFETY: どちらも NUL 終端の C 文字列で、呼び出しのあいだ生きている
        let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// `RENAME_EXCL` を持たないファイルシステム（man renamex_np: ENOTSUP）
    pub(super) fn unsupported(error: &std::io::Error) -> bool {
        error.raw_os_error() == Some(libc::ENOTSUP)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_path(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }

    pub(super) fn rename_excl(from: &Path, to: &Path) -> std::io::Result<()> {
        let (from, to) = (c_path(from)?, c_path(to)?);
        // SAFETY: どちらも NUL 終端の C 文字列で、呼び出しのあいだ生きている
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// `RENAME_NOREPLACE` を持たない（古いカーネル = ENOSYS・対応しないファイルシステム = EINVAL）。
    /// 自分の配下へ移すときの EINVAL もここへ来るが、従来の形の rename が同じ EINVAL を返す
    pub(super) fn unsupported(error: &std::io::Error) -> bool {
        matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL))
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    type Bool = i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> Bool;
    }

    /// `MOVEFILE_REPLACE_EXISTING`（1）も `MOVEFILE_COPY_ALLOWED`（2）も付けない
    const NO_REPLACE: u32 = 0;

    pub(super) fn rename_excl(from: &Path, to: &Path) -> std::io::Result<()> {
        let from = crate::platform::fs_copy::wide(from)?;
        let to = crate::platform::fs_copy::wide(to)?;
        // SAFETY: どちらも NUL 終端のワイド文字列で、呼び出しのあいだ生きている
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), NO_REPLACE) };
        if ok != 0 {
            Ok(())
        } else {
            // ERROR_ALREADY_EXISTS / ERROR_FILE_EXISTS は std が AlreadyExists へ、
            // ERROR_NOT_SAME_DEVICE は CrossesDevices へ寄せる
            Err(std::io::Error::last_os_error())
        }
    }

    /// `MoveFileExW` は上書きしない移動をどのファイルシステムでも持つ
    pub(super) fn unsupported(_error: &std::io::Error) -> bool {
        false
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use std::path::Path;

    /// 対象外の OS（tako の配布先ではない）。従来の形へ落とす
    pub(super) fn rename_excl(_from: &Path, _to: &Path) -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }

    pub(super) fn unsupported(_error: &std::io::Error) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// テストの置き場。**必ず一時 dir の中**（本物のファイルを動かさない = #1811 の教訓）
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "tako-fs-move-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        let real = crate::platform::path::canonicalize(&base).unwrap();
        let tmp = crate::platform::path::canonicalize(&std::env::temp_dir()).unwrap();
        assert!(
            real.starts_with(&tmp),
            "テストの置き場が一時 dir の外: {}",
            real.display()
        );
        real
    }

    #[test]
    fn 同名があればファイルもフォルダも上書きせずに断る() {
        let base = scratch("taken");
        std::fs::write(base.join("a.txt"), "元").unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        std::fs::write(base.join("dst/a.txt"), "既存").unwrap();
        let err = rename_noreplace(&base.join("a.txt"), &base.join("dst/a.txt")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert_eq!(std::fs::read_to_string(base.join("a.txt")).unwrap(), "元");
        assert_eq!(
            std::fs::read_to_string(base.join("dst/a.txt")).unwrap(),
            "既存"
        );
        // 空のフォルダの上へ（`rename(2)` は黙って置き換える）
        std::fs::create_dir_all(base.join("d/inner")).unwrap();
        std::fs::create_dir_all(base.join("dst/d")).unwrap();
        let err = rename_noreplace(&base.join("d"), &base.join("dst/d")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert!(base.join("d/inner").is_dir(), "元のフォルダが残る");
        assert!(base.join("dst/d").is_dir(), "移動先の空のフォルダが残る");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 同名が無ければファイルもフォルダも移す() {
        let base = scratch("move");
        std::fs::write(base.join("a.txt"), "A").unwrap();
        std::fs::create_dir_all(base.join("d/inner")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        rename_noreplace(&base.join("a.txt"), &base.join("dst/a.txt")).unwrap();
        rename_noreplace(&base.join("d"), &base.join("dst/d")).unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join("dst/a.txt")).unwrap(),
            "A"
        );
        assert!(base.join("dst/d/inner").is_dir());
        assert!(!base.join("a.txt").exists() && !base.join("d").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 移す元が無ければ理由つきで失敗し何も作らない() {
        let base = scratch("missing");
        let err = rename_noreplace(&base.join("nope"), &base.join("x")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
        assert!(!base.join("x").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn 壊れたリンクの名前も同名として断る() {
        let base = scratch("dangling");
        std::fs::write(base.join("a.txt"), "A").unwrap();
        std::os::unix::fs::symlink(base.join("nowhere"), base.join("b.txt")).unwrap();
        let err = rename_noreplace(&base.join("a.txt"), &base.join("b.txt")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert!(std::fs::symlink_metadata(base.join("b.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 排他の rename を持たないファイルシステムへ落ちたときの形も、同名は断る
    #[test]
    fn 従来の形へ落ちても同名は断る() {
        let base = scratch("checked");
        std::fs::write(base.join("a.txt"), "元").unwrap();
        std::fs::write(base.join("b.txt"), "既存").unwrap();
        let err = rename_checked(&base.join("a.txt"), &base.join("b.txt")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(base.join("b.txt")).unwrap(), "既存");
        rename_checked(&base.join("a.txt"), &base.join("c.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("c.txt")).unwrap(), "元");
        std::fs::remove_dir_all(&base).unwrap();
    }
}

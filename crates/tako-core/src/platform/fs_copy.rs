//! ファイルを写すときの OS 差（抽象境界 B28 の core 分。Issue #1860）
//!
//! シンボリックリンクを**リンクそのもの**として写す口だけを置く。unix は `symlink(2)`
//! 1 つで済むが、Windows はリンクの指す先がフォルダかファイルかで作る API が分かれ
//! （`CreateSymbolicLinkW` の `SYMBOLIC_LINK_FLAG_DIRECTORY`）、しかも**開発者モードか
//! 管理者でないと作れない**（`ERROR_PRIVILEGE_NOT_HELD` = 1314）。後者は権限の無い
//! 書き込み先と区別して理由を出すため、判定もここへ置く。
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

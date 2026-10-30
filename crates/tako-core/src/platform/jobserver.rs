//! cargo の共有 jobserver の fifo（境界 B31。Issue #1998 = エピック #1992 の S6）
//!
//! tako の GUI が fifo を作って開いたまま持ち、札（1 バイト = 1 枚）を入れておく。ペインの
//! `CARGO_MAKEFLAGS=-j<N> --jobserver-auth=fifo:<path>` を見た cargo（とその rustc・ビルドスクリプト）は
//! 自分のパスで同じ fifo を開き、並列の 2 本目以降の仕事の前に札を 1 枚読み、終わったら書き戻す。
//! 解釈（パスの決め方・env の値・数え直しの計画）は `tako_core::jobserver` の純関数で、ここは OS への口だけ。
//!
//! ## 形
//!
//! - **開き方は読み書き両方 + 待たない**（`O_RDWR | O_NONBLOCK | O_CLOEXEC`）。読みだけ・書きだけで開くと
//!   相手が開くまで open が止まる。`O_CLOEXEC` なのでペインの子へは渡らない（ペインの cargo は
//!   env のパスで自分で開く）。`O_NONBLOCK` は開いたファイル記述ごとの印なので、cargo の開き方は変えない
//! - **札の数は読まずに数える**（`ioctl(FIONREAD)`）。読むと cargo の手から札を奪うことになる。
//!   **数えるのは読みだけで開いた別の fd**: macOS の fifo は読み側と書き側の 2 つのソケットで、読み書き両方で
//!   開いた fd への `FIONREAD` は読み側の値を書き側の値（常に 0）で上書きして返す（XNU の `fifo_ioctl`。
//!   2026-10-31 に 5 枚入れて 0 と読めた実測）。読みだけ + 待たないの open は書き手が居なくても止まらない
//! - fifo の中身は**誰かが開いている間だけ**残る（最後の 1 本が閉じるとカーネルが捨てる）。
//!   GUI の再起動の間は走っている cargo が開いているので札は残る（数え直しの規則は呼び手）
//! - 置き場に fifo 以外のもの（普通のファイル・dir）があれば**触らずに断る**（消して作り直さない）
//!
//! ## Windows は対象外（#2000）
//!
//! jobserver の Windows 側は名前付きセマフォ（`--jobserver-auth=<名前>`）で、cargo が外から渡した
//! セマフォに従うかは未実測。[`SUPPORTED`] は false で、[`Fifo::open_or_create`] は `Unsupported` を返す
//! （呼び手は注入しない = cargo は従来どおり自前の並列で走る）。

use std::io;
use std::path::Path;

/// この OS で共有 jobserver（fifo）を持てるか
pub const SUPPORTED: bool = cfg!(unix);

/// 札を表す 1 バイト（GNU make と同じ `+`。cargo は読んだバイトをそのまま書き戻す）
pub const TOKEN: u8 = b'+';

/// GUI が開いたまま持つ fifo
#[derive(Debug)]
pub struct Fifo {
    /// 札の出し入れ（読み書き両方。開いている間は fifo の中身が残る）
    #[cfg(unix)]
    file: std::fs::File,
    /// 札を数えるだけの口（読みだけ。`FIONREAD` が読み側の値を返す）
    #[cfg(unix)]
    count: std::fs::File,
}

#[cfg(unix)]
impl Fifo {
    /// 在れば開き、無ければ作って（0600）開く。2 つ目の値は「いま作った」か。
    ///
    /// 親 dir は作る。置き場に fifo 以外のものがあれば触らずに `AlreadyExists` で断る
    pub fn open_or_create(path: &Path) -> io::Result<(Self, bool)> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::FileTypeExt;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut created = false;
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_fifo() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("fifo ではないものが置き場にある: {}", path.display()),
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let c = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                // SAFETY: c は NUL 終端の有効な C 文字列
                let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
                if rc != 0 {
                    let err = io::Error::last_os_error();
                    // 同時に作られた（別の tako）なら、それを開く
                    if err.kind() != io::ErrorKind::AlreadyExists {
                        return Err(err);
                    }
                } else {
                    created = true;
                }
            }
            Err(e) => return Err(e),
        }
        let file = Self::open_existing(path, true)?;
        let count = Self::open_existing(path, false)?;
        Ok((Self { file, count }, created))
    }

    fn open_existing(path: &Path, write: bool) -> io::Result<std::fs::File> {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        // 開いた後にも確かめる（確かめてから開くまでの間に差し替えられた形を受けない）
        if !file.metadata()?.file_type().is_fifo() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("fifo ではないものが置き場にある: {}", path.display()),
            ));
        }
        Ok(file)
    }

    /// いま fifo に入っている札の数（読まない）
    pub fn available(&self) -> io::Result<usize> {
        use std::os::unix::io::AsRawFd;
        let mut n: libc::c_int = 0;
        // SAFETY: fd は開いている fifo、n は c_int の書き先
        let rc = unsafe { libc::ioctl(self.count.as_raw_fd(), libc::FIONREAD, &mut n) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// 札を `n` 枚入れる（入った枚数。fifo の容量で途中までのことがある）
    pub fn put(&self, n: usize) -> io::Result<usize> {
        use std::io::Write;
        let buf = vec![TOKEN; n];
        let mut done = 0;
        while done < n {
            match (&self.file).write(&buf[done..]) {
                Ok(0) => break,
                Ok(k) => done += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    /// 札を最大 `n` 枚抜く（抜けた枚数。待たない）
    pub fn take(&self, n: usize) -> io::Result<usize> {
        use std::io::Read;
        let mut buf = vec![0u8; n];
        let mut done = 0;
        while done < n {
            match (&self.file).read(&mut buf[done..]) {
                Ok(0) => break,
                Ok(k) => done += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    /// パスがまだこの fifo を指しているか（消された・別のものに置き換えられた = false）
    pub fn is_at(&self, path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (self.file.metadata(), std::fs::symlink_metadata(path)) {
            (Ok(open), Ok(on_disk)) => open.dev() == on_disk.dev() && open.ino() == on_disk.ino(),
            _ => false,
        }
    }

    /// パスがこの fifo を指しているときだけ消す（消したか）。別の誰かが作り直したものは消さない
    pub fn unlink_if_at(&self, path: &Path) -> bool {
        self.is_at(path) && std::fs::remove_file(path).is_ok()
    }
}

#[cfg(not(unix))]
impl Fifo {
    pub fn open_or_create(_path: &Path) -> io::Result<(Self, bool)> {
        Err(unsupported())
    }

    pub fn available(&self) -> io::Result<usize> {
        Err(unsupported())
    }

    pub fn put(&self, _n: usize) -> io::Result<usize> {
        Err(unsupported())
    }

    pub fn take(&self, _n: usize) -> io::Result<usize> {
        Err(unsupported())
    }

    pub fn is_at(&self, _path: &Path) -> bool {
        false
    }

    pub fn unlink_if_at(&self, _path: &Path) -> bool {
        false
    }
}

#[cfg(not(unix))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "共有 jobserver（fifo）は unix だけ（Windows は #2000）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 対応はunixだけ() {
        assert_eq!(SUPPORTED, cfg!(unix));
    }

    #[cfg(unix)]
    #[test]
    fn 作って札を出し入れし数えられる() {
        let dir = crate::test_residue::ScratchDir::new("jobserver");
        let path = dir.path().join("resources/jobserver.fifo");
        let (fifo, created) = Fifo::open_or_create(&path).unwrap();
        assert!(created, "無ければ作る（親 dir も）");
        assert_eq!(fifo.available().unwrap(), 0);
        assert_eq!(fifo.put(5).unwrap(), 5);
        assert_eq!(fifo.available().unwrap(), 5, "読まずに数える");
        assert_eq!(fifo.take(2).unwrap(), 2);
        assert_eq!(fifo.available().unwrap(), 3);
        assert_eq!(fifo.take(10).unwrap(), 3, "在る分だけ抜けて待たない");
        assert_eq!(fifo.available().unwrap(), 0);
        assert!(fifo.is_at(&path));
    }

    #[cfg(unix)]
    #[test]
    fn 開いている間は中身が残り別の口からも同じ札が見える() {
        let dir = crate::test_residue::ScratchDir::new("jobserver");
        let path = dir.path().join("j.fifo");
        let (a, _) = Fifo::open_or_create(&path).unwrap();
        a.put(4).unwrap();
        // 2 本目の口（再起動後の GUI と同じ開き方）は作らずに同じ fifo を開く
        let (b, created) = Fifo::open_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(b.available().unwrap(), 4);
        drop(a);
        assert_eq!(b.available().unwrap(), 4, "誰かが開いている間は残る");
        drop(b);
        let (c, _) = Fifo::open_or_create(&path).unwrap();
        assert_eq!(c.available().unwrap(), 0, "全員が閉じたら中身は捨てられる");
    }

    #[cfg(unix)]
    #[test]
    fn fifoでないものは触らずに断る() {
        let dir = crate::test_residue::ScratchDir::new("jobserver");
        let path = dir.path().join("j.fifo");
        std::fs::write(&path, b"keep").unwrap();
        let err = Fifo::open_or_create(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"keep", "消して作り直さない");
    }

    #[cfg(unix)]
    #[test]
    fn 消されたら気づき作り直されたものは消さない() {
        let dir = crate::test_residue::ScratchDir::new("jobserver");
        let path = dir.path().join("j.fifo");
        let (old, _) = Fifo::open_or_create(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!old.is_at(&path), "消された");
        let (new, created) = Fifo::open_or_create(&path).unwrap();
        assert!(created);
        assert!(!old.unlink_if_at(&path), "別の inode は消さない");
        assert!(path.exists());
        assert!(new.unlink_if_at(&path));
        assert!(!path.exists());
    }

    #[cfg(not(unix))]
    #[test]
    fn unix以外は断る() {
        let err = Fifo::open_or_create(Path::new("x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}

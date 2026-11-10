//! unix ドメインソケットのパスの長さの上限と、収まらないときの短い置き場（抽象境界 B35。#1441 / #2855）
//!
//! ## なぜ境界が要るか
//!
//! unix ドメインソケットのパスは `sockaddr_un.sun_path` に入る長さまでしか使えない
//! （macOS / BSD は 104・Linux は 108 バイト。**終端 NUL を含む**）。tako はソケットの置き場を
//! 2 か所で data dir の下に決めるので、data dir が深いとそこへ置けない:
//!
//! - IPC の受け口（[`crate::ipc_socket`]。#1441）
//! - ssh の接続多重化の ControlPath（[`crate::remote_fs::control_path`]。#2855）
//!
//! どちらも「第一候補（data dir の下）→ `$TMPDIR` → [`FALLBACK_TEMP`] の順に、上限に収まる
//! 最初の 1 つ」で決める。この順と長さの測り方はここ 1 か所に置く（[`place_with`]）。
//! 上限は OS で違い、`cfg` を書いてよいのは `platform` 配下だけなので、ここが境界になる。
//!
//! ## 呼び手ごとに違うもの（引数で受ける）
//!
//! | 呼び手 | 上限 | 短い置き場が使えるか | どの候補も使えないとき |
//! |---|---|---|---|
//! | IPC | [`max_path_bytes`] | 見ない（bind の失敗は IPC の側が扱う） | 最も短い候補へ bind して失敗し、理由を申告する |
//! | ssh | [`max_path_bytes`] − OpenSSH の一時の接尾辞（[`crate::remote_fs::OPENSSH_MUX_TEMP_SUFFIX`]） | 書けるか（[`dir_accepts_socket`]） | ControlPath を渡さない = 多重化なしで繋ぐ |
//!
//! ssh が書けるかを先に見るのは、ソケットを作るのが tako ではなく ssh だから。置き場に
//! 書けないと ssh はログインの後に exit 255 で落ちる（#1040 の実測と同じ形）ので、
//! 渡す前に外す。短い置き場（`$TMPDIR` / `/tmp`）は tako が作らない dir なので、在ることも見る。
//!
//! Windows では IPC は名前付きパイプで、ssh は多重化を使わない（境界 B26
//! [`super::ssh_client`]）ので、ここの答えを使う経路が無い。判断は純関数なので、
//! macOS から Linux の上限も検査できる（#515 / #905 と同じ作法）。

use std::path::{Path, PathBuf};

/// `sockaddr_un.sun_path` の要素数（**終端 NUL を含む**）。macOS / BSD = 104
pub const SUN_PATH_BSD: usize = 104;
/// 同上。Linux = 108
pub const SUN_PATH_LINUX: usize = 108;

/// `$TMPDIR` 自体が上限に収まらない機のための最後の候補
pub const FALLBACK_TEMP: &str = "/tmp";

/// この OS でパスに使える最大バイト数（終端 NUL を除く）。
///
/// 実測（macOS 25.4 / 2026-09-13）: 103 バイトまで bind 成功・104 バイトで
/// `AF_UNIX path too long`。std も `bytes.len() >= sun_path.len()` で弾く。
///
/// 純関数側（[`place_with`]）が上限を**引数で受ける**ので、
/// macOS から Linux の上限も検査できる
pub const fn max_path_bytes() -> usize {
    if cfg!(target_os = "linux") {
        SUN_PATH_LINUX - 1
    } else {
        SUN_PATH_BSD - 1
    }
}

/// パスのバイト長（`sun_path` へ入るのはこのバイト列）
pub fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// 置き場の決まり方（[`place_with`] の結果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// 第一候補（data dir の下）が上限に収まった
    Primary(PathBuf),
    /// 第一候補が収まらない（または無い）ので短い置き場へ倒した
    Shortened(PathBuf),
    /// どの候補も使えない（上限に収まらない・置き場が使えない）。
    /// 中身は短い置き場の候補のうち最も短いもの（理由を申告するための材料）
    Unplaceable(PathBuf),
}

impl Placement {
    /// 決まったパス（[`Placement::Unplaceable`] では最も短い候補）
    pub fn path(&self) -> &Path {
        match self {
            Self::Primary(p) | Self::Shortened(p) | Self::Unplaceable(p) => p,
        }
    }

    /// 使えるパス（[`Placement::Unplaceable`] は `None`）
    pub fn into_usable(self) -> Option<PathBuf> {
        match self {
            Self::Primary(p) | Self::Shortened(p) => Some(p),
            Self::Unplaceable(_) => None,
        }
    }
}

/// 置き場を決める（**純関数**。env も実ファイルも見ない）。
///
/// 1. `primary` が `limit` に収まればそれ（短い置き場の可否は見ない。呼び手が作る dir のため）
/// 2. `<temp_dir>/<short_name>` → `<FALLBACK_TEMP>/<short_name>` の順に、`limit` に収まり、
///    置き場の dir が `short_dir_usable` を満たす最初の 1 つ
/// 3. どれも使えなければ [`Placement::Unplaceable`]
///
/// `short_name` は呼び手ごとに安定した名前（pid ではなくハッシュ）にすること。
/// 再起動をまたいで同じパスへ落ちる、が固定パスの存在理由なので
pub fn place_with(
    primary: Option<PathBuf>,
    short_name: &str,
    temp_dir: &Path,
    limit: usize,
    short_dir_usable: &dyn Fn(&Path) -> bool,
) -> Placement {
    if let Some(primary) = primary {
        if path_bytes(&primary) <= limit {
            return Placement::Primary(primary);
        }
    }
    let bases = [temp_dir, Path::new(FALLBACK_TEMP)];
    for base in bases {
        let candidate = base.join(short_name);
        if path_bytes(&candidate) <= limit && short_dir_usable(base) {
            return Placement::Shortened(candidate);
        }
    }
    // 同じ長さなら先の候補（`$TMPDIR` の側）を申告する
    let [first, second] = bases.map(|base| base.join(short_name));
    if path_bytes(&second) < path_bytes(&first) {
        Placement::Unplaceable(second)
    } else {
        Placement::Unplaceable(first)
    }
}

/// 既に在る dir に、このプロセスがソケットを作れるか。
///
/// unix は `access(2)` の `W_OK | X_OK`（dir の中にファイルを作るには書き込みと検索の両方が要る）。
/// Windows は dir であることだけを見る（ここの答えを使う経路が無い。モジュール doc）
pub fn dir_accepts_socket(dir: &Path) -> bool {
    dir.is_dir() && imp::writable(dir)
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub fn writable(dir: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `path` は NUL 終端の有効な C 文字列で、呼び出しの間は生きている
        unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::Path;

    pub fn writable(_dir: &Path) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: usize = SUN_PATH_BSD - 1;

    fn always(_: &Path) -> bool {
        true
    }

    fn never(_: &Path) -> bool {
        false
    }

    #[test]
    fn 上限はosごとの_sun_path_から終端の1バイトを引いた長さ() {
        assert_eq!(SUN_PATH_BSD - 1, 103);
        assert_eq!(SUN_PATH_LINUX - 1, 107);
        let expected = if cfg!(target_os = "linux") { 107 } else { 103 };
        assert_eq!(max_path_bytes(), expected);
    }

    #[test]
    fn 第一候補が収まればそのまま() {
        let primary = PathBuf::from("/d/ssh/x.sock");
        let got = place_with(
            Some(primary.clone()),
            "s.sock",
            Path::new("/t"),
            MAC,
            &never,
        );
        // 第一候補は短い置き場の可否を見ない（呼び手が作る dir）
        assert_eq!(got, Placement::Primary(primary));
    }

    #[test]
    fn 第一候補の長さは上限ちょうどまで使う() {
        let at = PathBuf::from(format!("/{}", "a".repeat(MAC - 1)));
        assert_eq!(path_bytes(&at), MAC);
        assert_eq!(
            place_with(Some(at.clone()), "s", Path::new("/t"), MAC, &always),
            Placement::Primary(at)
        );
        let over = PathBuf::from(format!("/{}", "a".repeat(MAC)));
        assert_eq!(
            place_with(Some(over), "s", Path::new("/t"), MAC, &always),
            Placement::Shortened(PathBuf::from("/t/s"))
        );
    }

    #[test]
    fn 第一候補が無ければ短い置き場から選ぶ() {
        assert_eq!(
            place_with(None, "s.sock", Path::new("/t"), MAC, &always),
            Placement::Shortened(PathBuf::from("/t/s.sock"))
        );
    }

    #[test]
    fn tmpdir_が深ければ_tmp_へ倒す() {
        let deep = PathBuf::from(format!("/{}", "d".repeat(120)));
        assert_eq!(
            place_with(None, "s.sock", &deep, MAC, &always),
            Placement::Shortened(PathBuf::from("/tmp/s.sock"))
        );
    }

    #[test]
    fn 使えない短い置き場は飛ばす() {
        let usable_only_tmp = |dir: &Path| dir == Path::new(FALLBACK_TEMP);
        assert_eq!(
            place_with(None, "s.sock", Path::new("/t"), MAC, &usable_only_tmp),
            Placement::Shortened(PathBuf::from("/tmp/s.sock"))
        );
    }

    #[test]
    fn どれも使えなければ最も短い候補を申告する() {
        let deep = PathBuf::from(format!("/{}", "d".repeat(120)));
        // 長すぎる（`/tmp/<名前>` も上限を超える名前）
        let long_name = "n".repeat(MAC);
        let got = place_with(None, &long_name, &deep, MAC, &always);
        assert_eq!(
            got,
            Placement::Unplaceable(Path::new(FALLBACK_TEMP).join(&long_name))
        );
        assert_eq!(got.clone().into_usable(), None);
        // 書けない（長さは収まる）。同じ長さなら先の候補（`$TMPDIR`）
        let got = place_with(None, "s.sock", Path::new("/abc"), MAC, &never);
        assert_eq!(got, Placement::Unplaceable(PathBuf::from("/abc/s.sock")));
        assert_eq!(got.path(), Path::new("/abc/s.sock"));
        assert_eq!(got.into_usable(), None);
    }

    #[test]
    fn 置き場の可否は在る_dir_で書けることを見る() {
        let scratch = crate::test_residue::ScratchDir::new("unix-socket-2855");
        assert!(dir_accepts_socket(scratch.path()));
        // 無い dir・ファイルは使えない
        assert!(!dir_accepts_socket(&scratch.path().join("missing")));
        let file = scratch.path().join("file");
        std::fs::write(&file, b"").expect("ファイルを作れる");
        assert!(!dir_accepts_socket(&file));
    }

    #[cfg(unix)]
    #[test]
    fn 書けない_dir_は使えない() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::test_residue::ScratchDir::new("unix-socket-2855-ro");
        let ro = scratch.path().join("ro");
        std::fs::create_dir(&ro).expect("dir を作れる");
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555))
            .expect("権限を変えられる");
        // root は access(2) が常に通るので、書けないことを作れる環境でだけ見る
        let probe = std::fs::write(ro.join("probe"), b"").is_err();
        let got = dir_accepts_socket(&ro);
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755))
            .expect("権限を戻せる");
        if probe {
            assert!(!got, "書き込めない dir を使えると判定した");
        }
    }
}

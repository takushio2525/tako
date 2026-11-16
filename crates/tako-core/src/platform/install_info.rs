//! インストーラーが記録した「実際に入っている版数」と「入れた先」の問い合わせ（抽象境界 B20）
//!
//! ## なぜ境界が要るか（#723）
//!
//! アプリ内更新はまず「自分は今どの版か」を知る必要がある。ふつうは
//! `CARGO_PKG_VERSION` で足りるが、Windows の `-win.N`（`v0.5.13-win.3` のような
//! **同じ Cargo バージョンのまま反復するプレビュー配布**）ではこれが足りない。
//! Cargo.toml の version は `0.5.13` のままなので、win.1 も win.3 も同じ
//! `0.5.13` を名乗ってしまい、`-win.N` 同士の新旧を区別できない。
//!
//! ビルド時に `TAKO_WIN_NUM` を渡せば以後のビルドは正しい版数を埋め込めるが、
//! **すでに配布済みの win.1〜win.3 は作り直せない**。それらは一律 `0.5.13` を
//! 名乗るため、「最新は win.3」と比べると永遠に「更新あり」になり、更新しても
//! 同じ版が入るだけの無限ループに陥る。
//!
//! Inno Setup のインストーラーは `AppVersion`（= 完全なタグ `v0.5.13-win.3`）を
//! アンインストール情報の `DisplayVersion` に書く。これは **win.1 の時点から
//! 書かれている**ので、既存インストールに対しても遡って正確な版数が取れる。
//! ここはその問い合わせだけに責任を持つ（版数の比較は呼び出し側の仕事）。
//!
//! ## 取得できないとき
//!
//! ポータブル zip 版・開発ビルド・macOS では `None` を返す。呼び出し側は
//! ビルド時に埋め込んだ版数へフォールバックする。

/// インストーラーが記録した版数（例: `"v0.5.13-win.3"`）。
///
/// 記録が無い / 読めない場合は `None`。**先頭の `v` は落とさずそのまま返す**
/// （タグ表記のまま返し、正規化は呼び出し側に委ねる）。
pub fn installed_version() -> Option<String> {
    imp::installed_version()
}

/// インストーラーが記録した**入れた先のフォルダ**（Inno Setup の `InstallLocation`。#2434）。
///
/// アプリ内更新はインストーラーを無言で起こして上書きさせる。インストーラーは前回の
/// 入れた先（`UsePreviousAppDir` の既定）へ入れるので、**いま動いている exe がその中に
/// 居るときだけ**この経路で更新できる。ポータブル zip・開発ビルドの exe は別の場所に居るので、
/// 呼び出し側はこの値と `current_exe` の置き場を比べて断る。
///
/// 記録が無い / 読めない場合は `None`（macOS では常に `None`）
pub fn install_location() -> Option<std::path::PathBuf> {
    imp::install_location()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

/// 動いている exe がインストーラーで入れた先（[`install_location`]）に居るか（#2434 / #2931）。
///
/// 「インストーラーで入れた本体か」の判定の**正本**。アプリ内更新（別の場所の exe を更新すると
/// 「別の場所に新しい版が入り、動いている方は古いまま」になる = #2434）と、tako の CLI の解決
/// （入れた本体なら隣の `tako.exe` を PATH の別の `tako.exe` より先に使う = #2931）が同じ答えを使う。
/// ポータブル zip・開発ビルドの exe は入れた先の外に居るので `false`（記録が無ければ常に `false` =
/// macOS は常に `false`）。
///
/// 比べ方は Windows のパスの流儀（大文字小文字を区別しない・`/` と `\` を同一視・
/// 末尾の区切りを無視・`\\?\` を剥がす）
pub fn runs_from_install_dir(
    install_location: Option<&std::path::Path>,
    exe: &std::path::Path,
) -> bool {
    let Some(install) = install_location else {
        return false;
    };
    // `Path::parent` は実行環境の区切りで切るので使わない（macOS の単体からも
    // Windows のパスを同じ規則で比べられるよう、文字列のまま Windows の流儀で切る）
    let norm = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        s.trim_end_matches('\\').to_lowercase()
    };
    let exe = norm(exe);
    let Some((exe_dir, _)) = exe.rsplit_once('\\') else {
        return false;
    };
    let install = norm(install);
    !install.is_empty() && install == exe_dir
}

#[cfg(windows)]
mod imp {
    /// Inno Setup が per-user インストールで作るアンインストールキー。
    /// `_is1` 接尾辞は Inno Setup の規約、GUID は `installer/windows/tako.iss` の
    /// `AppId` と 1:1（**変えると別アプリ扱いになる**ので両方同時に直すこと）
    const UNINSTALL_SUBKEY: &str = concat!(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\",
        "{95CA86D9-AAFB-4ABC-8D9B-2C66F75CF739}_is1"
    );

    const HKEY_CURRENT_USER: isize = -2147483647; // 0x80000001
    const KEY_READ: u32 = 0x2_0019;
    const ERROR_SUCCESS: i32 = 0;
    const REG_SZ: u32 = 1;

    // `windows-sys` を足さず必要な 3 関数だけ宣言する方針は
    // `platform::locale` の GetUserPreferredUILanguages と同じ
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            lpsubkey: *const u16,
            uloptions: u32,
            samdesired: u32,
            phkresult: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            lpvaluename: *const u16,
            lpreserved: *mut u32,
            lptype: *mut u32,
            lpdata: *mut u8,
            lpcbdata: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn installed_version() -> Option<String> {
        read_uninstall_value("DisplayVersion")
    }

    /// Inno Setup は `InstallLocation` に `{app}\`（末尾の区切り付き）を書く
    pub(super) fn install_location() -> Option<String> {
        read_uninstall_value("InstallLocation")
    }

    fn read_uninstall_value(name: &str) -> Option<String> {
        let subkey = wide(UNINSTALL_SUBKEY);
        let mut hkey: isize = 0;
        // SAFETY: subkey は NUL 終端の生存するバッファ、hkey はスタック上の変数
        let rc =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
        if rc != ERROR_SUCCESS {
            // インストーラー経由で入っていない（ポータブル zip / 開発ビルド）
            return None;
        }
        let value = read_string(hkey, name);
        // SAFETY: RegOpenKeyExW が成功したハンドルだけを閉じる
        unsafe { RegCloseKey(hkey) };
        value
    }

    /// 開いたキーから REG_SZ の値を読む。2 段階呼び出し（長さ問い合わせ → 実取得）
    fn read_string(hkey: isize, name: &str) -> Option<String> {
        let name_w = wide(name);
        let mut kind: u32 = 0;
        let mut len: u32 = 0;
        // SAFETY: 出力ポインタはスタック上の生存する変数。データバッファに null を
        // 渡すのは「必要なバイト数だけ問い合わせる」規定の呼び出し方
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc != ERROR_SUCCESS || kind != REG_SZ || len == 0 {
            return None;
        }
        // len はバイト数。UTF-16 なので 2 で割って要素数にする（奇数長は不正なので弾く）。
        // `% 2` ではなく下位ビットを見るのは clippy の manual_is_multiple_of を踏まないため
        // （`is_multiple_of` は比較的新しい安定化 API なので MSRV を上げたくない）
        if len & 1 != 0 {
            return None;
        }
        let mut buf = vec![0u16; (len / 2) as usize];
        let mut got = len;
        // SAFETY: buf は len バイト分を確保済みで、got にそのバイト数を入れて渡している
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr() as *mut u8,
                &mut got,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        Some(super::trim_nul(&buf))
    }
}

#[cfg(not(windows))]
mod imp {
    /// macOS の配布系統（Homebrew Cask / zip）は版数をバンドルの
    /// `CFBundleShortVersionString`（= `CARGO_PKG_VERSION`）に持つので、
    /// 別途の記録を読む必要が無い
    pub(super) fn installed_version() -> Option<String> {
        None
    }

    /// macOS の置き場は `.app` バンドルで決まる（`update_checker::app_bundle_path`）
    pub(super) fn install_location() -> Option<String> {
        None
    }
}

/// UTF-16 バッファから末尾の NUL 以降を落として `String` にする。
/// レジストリの REG_SZ は NUL 終端が値に含まれることも含まれないこともある。
///
/// 呼ぶのは Windows の `imp` だけだが、**純粋関数なので macOS からも検証できるように**
/// cfg で切らず常にコンパイルする（#515 と同じ方針）。非 Windows では
/// テストからしか呼ばれず dead_code になるので、そこだけ黙らせる
#[cfg_attr(not(windows), allow(dead_code))]
fn trim_nul(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_nul_handles_terminated_and_bare() {
        // NUL 終端あり
        let buf: Vec<u16> = "v0.5.13-win.3\0".encode_utf16().collect();
        assert_eq!(trim_nul(&buf), "v0.5.13-win.3");
        // NUL 終端なし
        let buf: Vec<u16> = "v0.5.13-win.3".encode_utf16().collect();
        assert_eq!(trim_nul(&buf), "v0.5.13-win.3");
        // 前後の空白は落とす
        let buf: Vec<u16> = "  v0.6.0  \0".encode_utf16().collect();
        assert_eq!(trim_nul(&buf), "v0.6.0");
        // 空
        assert_eq!(trim_nul(&[0u16]), "");
        assert_eq!(trim_nul(&[]), "");
    }

    #[test]
    fn installed_version_does_not_panic() {
        // インストーラー経由でなければ None。どちらでも panic しないことだけ担保する
        let v = installed_version();
        if let Some(ref s) = v {
            assert!(!s.is_empty(), "空文字を Some で返してはいけない");
        }
        // 非 Windows では常に None
        #[cfg(not(windows))]
        assert!(v.is_none());
    }

    #[test]
    fn install_location_does_not_panic() {
        // 記録が在れば空ではない。非 Windows では常に None
        if let Some(dir) = install_location() {
            assert!(!dir.as_os_str().is_empty());
        }
        #[cfg(not(windows))]
        assert!(install_location().is_none());
    }

    /// #2434 で tako-app の `installer_update` に置いた検査を、正本と一緒に移した（#2931）
    #[test]
    fn インストーラーで入れた場所から動いているかを比べる() {
        use std::path::Path;
        let install = Path::new(r"C:\Users\testuser\AppData\Local\Programs\tako\");
        for exe in [
            r"C:\Users\testuser\AppData\Local\Programs\tako\tako-app.exe",
            r"c:\USERS\testuser\APPDATA\local\programs\TAKO\tako-app.exe",
            r"\\?\C:\Users\testuser\AppData\Local\Programs\tako\tako-app.exe",
            "C:/Users/testuser/AppData/Local/Programs/tako/tako-app.exe",
            // CLI 自身（`tako setup` を打った本体）も同じ置き場（#2931）
            r"C:\Users\testuser\AppData\Local\Programs\tako\tako.exe",
        ] {
            assert!(
                runs_from_install_dir(Some(install), Path::new(exe)),
                "{exe}"
            );
        }
        for exe in [
            // ポータブル zip を展開した先
            r"C:\Users\testuser\Downloads\tako-v0.8.39-windows-x86_64\tako-app.exe",
            // 開発ビルド
            r"C:\src\tako\target\debug\tako-app.exe",
            r"C:\src\tako\target\release\tako.exe",
            // 入れた先の下の別フォルダ
            r"C:\Users\testuser\AppData\Local\Programs\tako\sub\tako-app.exe",
        ] {
            assert!(
                !runs_from_install_dir(Some(install), Path::new(exe)),
                "{exe}"
            );
        }
        // 記録が無い = インストーラーで入れていない
        assert!(!runs_from_install_dir(
            None,
            Path::new(r"C:\x\tako-app.exe")
        ));
        assert!(!runs_from_install_dir(
            Some(Path::new("")),
            Path::new(r"C:\x\tako-app.exe")
        ));
        // 区切りの無い exe（親の無いパス）は入れた先に居ない
        assert!(!runs_from_install_dir(
            Some(install),
            Path::new("tako-app.exe")
        ));
    }
}

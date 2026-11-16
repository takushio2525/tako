//! 既知フォルダ（プロファイル・デスクトップ・ダウンロード・ドキュメント）の OS が答える置き場
//! （抽象境界 B39。#2939）。
//!
//! ## なぜ境界が要るか
//!
//! Windows のデスクトップ・ダウンロード・ドキュメントの実際の置き場は Known Folder
//! （`SHGetKnownFolderPath`）で決まり、OneDrive への移動（Known Folder Move。Windows 11 では多い構成）や
//! 利用者のリダイレクトで `%USERPROFILE%\Desktop` から外れる（#2939 の実機: デスクトップは
//! `~\OneDrive\デスクトップ` で `~\Desktop` は無い）。ホームの下の名前を決め打ちすると、その機械では
//! 「無い」と読んでしまう。
//!
//! macOS / unix に既知フォルダの登録は無い（macOS のデスクトップは iCloud へ寄せても `~/Desktop` のまま）
//! ので [`os_path`] は常に `None` を返し、呼び手はホームの下の慣例の名前（[`KnownFolder::home_relative`]）
//! で組む。
//!
//! ## 呼び手
//!
//! | 呼び手 | 引く既知フォルダ | 答えが無いとき |
//! |---|---|---|
//! | スマホのファイル閲覧の既定のショートカット（`remote_shortcuts::defaults_for`。#2939） | Desktop / Downloads（検証プロセスでは Profile も） | `<ホーム>/<慣例の名前>` |
//! | 以前の版が書いた `$PROFILE` の置き場（`shell_integration` の Windows の腕。#2432） | Documents | `%USERPROFILE%\Documents` |
//!
//! 答えは**利用者の本物の置き場**で、`HOME` / `USERPROFILE` を砂場へ差し替えても追従しない。検証プロセスで
//! どう扱うかは呼び手が決める（#2432 は本物の「ドキュメント」を見ない、#2939 は渡されたホームが OS の
//! プロファイルと同じときだけ使う）。
//!
//! `windows-sys` は足さず、必要な関数だけを宣言する（[`super::install_info`] と同じ方針）。
//! GUID の表は OS に依らないデータとして持つので、正規の文字列の形との一致を macOS からも検査できる。

use std::path::PathBuf;

/// 引ける既知フォルダ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownFolder {
    /// 利用者のプロファイル（`%USERPROFILE%`）。`FOLDERID_Profile`
    Profile,
    /// デスクトップ。`FOLDERID_Desktop`
    Desktop,
    /// ダウンロード。`FOLDERID_Downloads`
    Downloads,
    /// ドキュメント（PowerShell の `$PROFILE` の親）。`FOLDERID_Documents`
    Documents,
}

impl KnownFolder {
    /// ホームの下の慣例の名前（OS が答えないときに呼び手が組む）。プロファイルはホームそのものなので `None`
    pub fn home_relative(self) -> Option<&'static str> {
        match self {
            Self::Profile => None,
            Self::Desktop => Some("Desktop"),
            Self::Downloads => Some("Downloads"),
            Self::Documents => Some("Documents"),
        }
    }

    /// `SHGetKnownFolderPath` へ渡す `KNOWNFOLDERID`（`KnownFolders.h` の値）
    fn folder_id(self) -> Guid {
        match self {
            // {5E6C858F-0E22-4760-9AFE-EA3317B67173}
            Self::Profile => Guid {
                data1: 0x5E6C_858F,
                data2: 0x0E22,
                data3: 0x4760,
                data4: [0x9A, 0xFE, 0xEA, 0x33, 0x17, 0xB6, 0x71, 0x73],
            },
            // {B4BFCC3A-DB2C-424C-B029-7FE99A87C641}
            Self::Desktop => Guid {
                data1: 0xB4BF_CC3A,
                data2: 0xDB2C,
                data3: 0x424C,
                data4: [0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41],
            },
            // {374DE290-123F-4565-9164-39C4925E467B}
            Self::Downloads => Guid {
                data1: 0x374D_E290,
                data2: 0x123F,
                data3: 0x4565,
                data4: [0x91, 0x64, 0x39, 0xC4, 0x92, 0x5E, 0x46, 0x7B],
            },
            // {FDD39AD0-238F-46AF-ADB4-6C85480369C7}
            Self::Documents => Guid {
                data1: 0xFDD3_9AD0,
                data2: 0x238F,
                data3: 0x46AF,
                data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
            },
        }
    }
}

/// Win32 の `GUID` と同じ並び（OS に依らないデータ。Windows の腕がそのまま API へ渡す）。
/// macOS / unix の腕は中身を読まない（表の検査はテストが読む）
#[cfg_attr(not(windows), allow(dead_code))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// OS が答える既知フォルダの置き場。
///
/// Windows は `SHGetKnownFolderPath`（フラグなし = 置き場が無ければ失敗する）。失敗・空の答えは `None`。
/// macOS / unix は既知フォルダの登録が無いので常に `None`（呼び手がホームの下の慣例の名前で組む）
pub fn os_path(folder: KnownFolder) -> Option<PathBuf> {
    imp::os_path(folder.folder_id()).filter(|p| !p.as_os_str().is_empty())
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    /// 既知フォルダの登録は Windows にしか無い
    pub(super) fn os_path(_id: super::Guid) -> Option<PathBuf> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::Guid;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            rfid: *const Guid,
            dwflags: u32,
            htoken: isize,
            ppszpath: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(pv: *const std::ffi::c_void);
    }

    pub(super) fn os_path(id: Guid) -> Option<PathBuf> {
        let mut raw: *mut u16 = std::ptr::null_mut();
        // SAFETY: GUID と受け取り先は有効なポインタ。htoken = 0 は「このプロセスのユーザー」
        let hr = unsafe { SHGetKnownFolderPath(&id, 0, 0, &mut raw) };
        let dir = if hr >= 0 && !raw.is_null() {
            let mut len = 0usize;
            // SAFETY: 成功時の raw は NUL 終端の UTF-16 文字列
            let wide = unsafe {
                while *raw.add(len) != 0 {
                    len += 1;
                }
                std::slice::from_raw_parts(raw, len)
            };
            Some(PathBuf::from(std::ffi::OsString::from_wide(wide)))
        } else {
            None
        };
        // 受け取ったバッファは成否によらず返す（API の契約）
        if !raw.is_null() {
            // SAFETY: SHGetKnownFolderPath が CoTaskMemAlloc で確保したもの
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [KnownFolder; 4] = [
        KnownFolder::Profile,
        KnownFolder::Desktop,
        KnownFolder::Downloads,
        KnownFolder::Documents,
    ];

    /// GUID を `KnownFolders.h` の正規の文字列の形（`{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`）へ
    fn guid_text(g: Guid) -> String {
        let d = g.data4;
        format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }

    /// GUID の表は Microsoft の文書（`KnownFolders.h`）の値と一字一句同じ。打ち間違えると OS は
    /// 「無い」と答えるだけで落ちないので、macOS でも表そのものを検査する
    #[test]
    fn guidの表は既知フォルダの正規の値と一致する() {
        let expected = [
            (
                KnownFolder::Profile,
                "{5E6C858F-0E22-4760-9AFE-EA3317B67173}",
            ),
            (
                KnownFolder::Desktop,
                "{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}",
            ),
            (
                KnownFolder::Downloads,
                "{374DE290-123F-4565-9164-39C4925E467B}",
            ),
            (
                KnownFolder::Documents,
                "{FDD39AD0-238F-46AF-ADB4-6C85480369C7}",
            ),
        ];
        for (folder, text) in expected {
            assert_eq!(guid_text(folder.folder_id()), text, "{folder:?}");
        }
        // 16 バイト = Win32 の GUID と同じ大きさ（そのまま API へ渡す）
        assert_eq!(std::mem::size_of::<Guid>(), 16);
    }

    /// 答えるなら空ではない（空の答えは `None` へ寄せる = 呼び手が `""` を置き場と読まない）
    #[test]
    fn 既知フォルダの答えは空にならない() {
        for folder in ALL {
            assert!(
                os_path(folder).is_none_or(|p| !p.as_os_str().is_empty()),
                "{folder:?}"
            );
        }
    }

    #[test]
    fn ホームの下の慣例の名前() {
        assert_eq!(KnownFolder::Profile.home_relative(), None);
        assert_eq!(KnownFolder::Desktop.home_relative(), Some("Desktop"));
        assert_eq!(KnownFolder::Downloads.home_relative(), Some("Downloads"));
        assert_eq!(KnownFolder::Documents.home_relative(), Some("Documents"));
    }

    /// macOS / unix は OS に尋ねない（呼び手がホームの下の慣例の名前で組む = #2939 の前と同じ）
    #[cfg(not(windows))]
    #[test]
    fn unixでは既知フォルダを答えない() {
        for folder in ALL {
            assert_eq!(os_path(folder), None, "{folder:?}");
        }
    }

    /// Windows の既知フォルダを .NET / シェルへ尋ねた答え（UTF-8 の 16 進で受ける = 5.1 のリダイレクト出力は
    /// OEM コードページなので、生の文字列で受けると日本語のパスが壊れる）
    #[cfg(windows)]
    fn powershell_answer(expr: &str) -> String {
        let script = format!(
            "$p = {expr}; if ($p) {{ -join ([System.Text.Encoding]::UTF8.GetBytes($p) | ForEach-Object {{ '{{0:x2}}' -f $_ }}) }}"
        );
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]);
        crate::platform::process::no_console_window(&mut cmd);
        let out = cmd.output().expect("PowerShell を起動できる");
        let hex: String = String::from_utf8_lossy(&out.stdout)
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).expect("ASCII の 16 進");
                u8::from_str_radix(pair, 16).expect("16 進")
            })
            .collect();
        String::from_utf8(bytes).expect("UTF-8")
    }

    /// 実機（Windows）の既知フォルダが、.NET（`[Environment]::GetFolderPath`）とシェル（`shell:Downloads`）の
    /// 答えと一致する。**読むだけ**（フォルダは作らない・触らない）
    #[cfg(windows)]
    #[test]
    fn 実機の既知フォルダはnetとシェルの答えと一致する() {
        let cases = [
            (
                KnownFolder::Profile,
                "[Environment]::GetFolderPath('UserProfile')",
            ),
            (
                KnownFolder::Desktop,
                "[Environment]::GetFolderPath('Desktop')",
            ),
            (
                KnownFolder::Downloads,
                "(New-Object -ComObject Shell.Application).NameSpace('shell:Downloads').Self.Path",
            ),
            (
                KnownFolder::Documents,
                "[Environment]::GetFolderPath('MyDocuments')",
            ),
        ];
        for (folder, expr) in cases {
            let ours = os_path(folder);
            let answered = powershell_answer(expr);
            // 値は出さない（実ホームパスが入る）。比べた結果と在るかだけを残す
            println!(
                "TAKO2939_KNOWN_FOLDER folder={folder:?} ours={} answered={} is_dir={}",
                ours.is_some(),
                !answered.is_empty(),
                ours.as_deref().is_some_and(std::path::Path::is_dir)
            );
            match ours {
                Some(ours) => assert_eq!(
                    ours.to_string_lossy().to_lowercase(),
                    answered.to_lowercase(),
                    "{folder:?}: SHGetKnownFolderPath と {expr} の答えが違う"
                ),
                // フラグなしの SHGetKnownFolderPath は置き場が無いと失敗する。そのときは向こうの答えも無い置き場
                None => assert!(
                    answered.is_empty() || !std::path::Path::new(&answered).is_dir(),
                    "{folder:?}: {expr} は在る置き場を答えたのに SHGetKnownFolderPath が答えない"
                ),
            }
        }
        // プロファイルとデスクトップは利用者が居れば必ず在る（ランナーでも在る）
        assert!(os_path(KnownFolder::Profile).is_some_and(|p| p.is_dir()));
        assert!(os_path(KnownFolder::Desktop).is_some_and(|p| p.is_dir()));
    }
}

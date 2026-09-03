//! 実行ファイルの探索（抽象境界 B16）
//!
//! 「コマンド名から実行ファイルを見つける」ことのプラットフォーム差を閉じ込める。
//!
//! ## なぜ境界が要るか（#525）
//!
//! 従来の実装は各所で `$SHELL -l -c "command -v <name>"` を直接叩いていた。
//! **macOS ではこれでないと困る**: `.app` を Dock から起動するとプロセスの PATH が
//! 最小構成（`/usr/bin:/bin:…`）になり、Homebrew や `~/.local/bin` のコマンドが
//! 一切見つからない。ログインシェルを経由して初めてユーザーの PATH で解決できる。
//!
//! 一方 **Windows には `$SHELL` も `command -v` も無い**ので、この実装は例外なく
//! `None` を返す。実測（2026-07-27・Windows 11）では claude / git が導入済みでも
//! `tako setup` が「claude / codex / agy のいずれも見つかりません」で停止した。
//!
//! ## Windows 側の作法
//!
//! - PATH を `PATHEXT` の拡張子と組み合わせて走査する（`where.exe` と同じ意味論。
//!   外部プロセスを起こさないのでコンソールウィンドウが明滅しない）
//! - PATH に無くても**ユーザーが手で入れがちな場所**を追って探す。Windows は
//!   インストーラが PATH を書き換えても**再ログインするまで実行中プロセスへ伝播しない**。
//!   「入れたのに見つからない」を避けるための保険
//!
//! ## 「実行できるファイルか」と「版はいくつか」（#936）
//!
//! [`is_executable_file`] と [`file_version`] も同じ境界に置く。どちらも
//! **実行ファイルという対象そのものの性質**で、判定材料が OS によって変わる:
//!
//! - 実行できるか: unix は mode の実行ビット、Windows は**拡張子が `PATHEXT` に
//!   在るか**（実行ビットという概念が無い）。旧実装は非 unix で無条件 `true` を
//!   返しており、`stale_binary` の PATH 走査がディレクトリでない任意のファイルを
//!   ランチャとして拾いうる状態だった
//! - 版はいくつか: Windows の exe は**版をリソースとして持つ**
//!   （`claude.exe` は `FileVersion=2.1.247.0` = 実測）。Windows の claude は
//!   ランチャが symlink ではなく**実体のコピー**なので、パスから版を読む手が
//!   使えない（`…\.local\bin\claude.exe`）。ここが `None` を返すと
//!   `claude --version` の起動へ落ちるが、claude の実行ファイルは 253MB あるので
//!   定期走査でそれを起こすのは避けたい（#772）

/// コマンド名から実行ファイルの絶対パスを解決する。見つからなければ `None`。
///
/// 返り値はそのまま [`std::process::Command::new`] に渡せる
/// （Windows の `.cmd` / `.bat` シムも Rust 標準ライブラリが解釈する）
pub fn find(name: &str) -> Option<String> {
    imp::find(name)
}

/// 「実行できる通常ファイル」か。symlink は追う（`which` と同じ判定）。
///
/// unix は mode の実行ビット、Windows は拡張子が `PATHEXT` に在るかを見る
/// （**Windows に実行ビットは無い**）。ディレクトリと存在しないパスは常に false
pub fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    imp::is_executable(path, &meta)
}

/// 実行ファイルが自分で名乗っている版（Windows の版リソース）。
/// 持たない形式・取得手段が無いプラットフォームでは `None`
pub fn file_version(path: &std::path::Path) -> Option<String> {
    imp::file_version(path)
}

#[cfg(unix)]
mod imp {
    /// ログインシェル経由で探す。`.app`（Dock 起動）の痩せた PATH でも
    /// ユーザーの PATH で解決できるようにするため（この経路を外すと Homebrew が全滅する）
    pub fn find(name: &str) -> Option<String> {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let output = std::process::Command::new(shell)
            .args(["-l", "-c", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }

    pub fn is_executable(_path: &std::path::Path, meta: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }

    /// Mach-O / ELF に Windows の版リソース相当は無い。パスから読めなければ
    /// `claude --version` へ落ちる（macOS のランチャは symlink なので
    /// 実際にはパスから読める）
    pub fn file_version(_path: &std::path::Path) -> Option<String> {
        None
    }
}

#[cfg(windows)]
mod imp {
    pub fn find(name: &str) -> Option<String> {
        super::find_in_windows_path(
            name,
            &split_path_list(std::env::var_os("PATH")),
            &pathext(),
            &user_install_dirs(),
            &|p| std::path::Path::new(p).is_file(),
        )
    }

    fn split_path_list(value: Option<std::ffi::OsString>) -> Vec<String> {
        value
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default()
            .split(';')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// `PATHEXT` は通常 `.COM;.EXE;.BAT;.CMD;…`。並び順がそのまま優先順位になる
    /// （`.exe` が `.cmd` シムより先に来るのはこの順序のおかげ）
    fn pathext() -> Vec<String> {
        let configured = split_path_list(std::env::var_os("PATHEXT"));
        if configured.is_empty() {
            [".COM", ".EXE", ".BAT", ".CMD"]
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            configured
        }
    }

    /// PATH に載っていなくても探しに行く場所。
    /// 「インストールしたのに再ログインしていない」ケースを拾うための保険
    fn user_install_dirs() -> Vec<String> {
        let mut dirs = Vec::new();
        let mut push = |base: Option<std::ffi::OsString>, rel: &str| {
            if let Some(base) = base.filter(|b| !b.is_empty()) {
                dirs.push(std::path::Path::new(&base).join(rel).display().to_string());
            }
        };
        let home = || std::env::var_os("USERPROFILE");
        // claude ネイティブインストーラ
        push(home(), ".local\\bin");
        // scoop
        push(home(), "scoop\\shims");
        // npm のグローバルシム（claude / agy を npm で入れた場合）
        push(std::env::var_os("APPDATA"), "npm");
        // winget が張るシム
        push(std::env::var_os("LOCALAPPDATA"), "Microsoft\\WinGet\\Links");
        // Git for Windows の既定インストール先
        push(std::env::var_os("ProgramFiles"), "Git\\cmd");
        dirs
    }

    pub fn is_executable(path: &std::path::Path, _meta: &std::fs::Metadata) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        super::has_executable_extension(&name, &pathext())
    }

    // --- 版リソース（version.dll） ---

    /// `VS_FIXEDFILEINFO`（verrsrc.h）。13 個の DWORD
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FixedFileInfo {
        signature: u32,
        struc_version: u32,
        file_version_ms: u32,
        file_version_ls: u32,
        product_version_ms: u32,
        product_version_ls: u32,
        file_flags_mask: u32,
        file_flags: u32,
        file_os: u32,
        file_type: u32,
        file_subtype: u32,
        file_date_ms: u32,
        file_date_ls: u32,
    }
    const _: () = assert!(std::mem::size_of::<FixedFileInfo>() == 52);
    /// `VS_FFI_SIGNATURE`
    const FIXED_FILE_INFO_SIGNATURE: u32 = 0xFEEF_04BD;

    #[link(name = "version")]
    extern "system" {
        fn GetFileVersionInfoSizeW(file_name: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(
            file_name: *const u16,
            handle: u32,
            len: u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
        fn VerQueryValueW(
            block: *const std::ffi::c_void,
            sub_block: *const u16,
            buffer: *mut *mut std::ffi::c_void,
            len: *mut u32,
        ) -> i32;
    }

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn file_version(path: &std::path::Path) -> Option<String> {
        let file = wide(path.as_os_str());
        // ルート（`\\`）の固定情報だけを読む。言語ごとの文字列表は使わない
        let root = wide(std::ffi::OsStr::new("\\"));
        // SAFETY: size は API に問い合わせた必要量で、buf はその長さぶん確保している。
        // VerQueryValue が返すポインタは buf の内側を指し、len で長さが分かる
        unsafe {
            let mut handle: u32 = 0;
            let size = GetFileVersionInfoSizeW(file.as_ptr(), &mut handle);
            if size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            if GetFileVersionInfoW(file.as_ptr(), handle, size, buf.as_mut_ptr().cast()) == 0 {
                return None;
            }
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut len: u32 = 0;
            if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut ptr, &mut len) == 0 {
                return None;
            }
            if ptr.is_null() || (len as usize) < std::mem::size_of::<FixedFileInfo>() {
                return None;
            }
            let info = std::ptr::read_unaligned(ptr.cast::<FixedFileInfo>());
            if info.signature != FIXED_FILE_INFO_SIGNATURE {
                return None;
            }
            Some(super::format_file_version(
                info.file_version_ms,
                info.file_version_ls,
            ))
        }
    }
}

/// Windows で「実行できる拡張子か」（純粋関数。**macOS 上でもテストできる**）。
///
/// `PATHEXT` は慣習的に大文字（`.EXE;.CMD;…`）でパスの大小は区別されないので、
/// 突き合わせは大小無視で行う。拡張子を持たない名前は false
/// （Windows のシェルはそれを実行対象として探さない）
#[cfg_attr(not(windows), allow(dead_code))]
fn has_executable_extension(file_name: &str, pathext: &[String]) -> bool {
    // `claude.exe` の `.exe`。`.` を含まない名前・`.` で終わる名前は対象外
    let Some(dot) = file_name.rfind('.') else {
        return false;
    };
    let ext = &file_name[dot..];
    if ext.len() <= 1 {
        return false;
    }
    pathext
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(ext))
}

/// Windows の版リソースの 2 つの DWORD を `major.minor.patch[.build]` へ整える
/// （純粋関数。**macOS 上でもテストできる**）。
///
/// claude の版は 3 成分（`2.1.247`）で 4 つめは 0 なので、0 のときは付けない
/// （`claude --version` の表記と `versions/<版>` のディレクトリ名に揃える）
#[cfg_attr(not(windows), allow(dead_code))]
fn format_file_version(file_version_ms: u32, file_version_ls: u32) -> String {
    let major = file_version_ms >> 16;
    let minor = file_version_ms & 0xffff;
    let patch = file_version_ls >> 16;
    let build = file_version_ls & 0xffff;
    if build == 0 {
        format!("{major}.{minor}.{patch}")
    } else {
        format!("{major}.{minor}.{patch}.{build}")
    }
}

/// Windows の PATH 探索（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 各ディレクトリについて「名前そのまま → `PATHEXT` の各拡張子」の順に見る。
/// ディレクトリを外側・拡張子を内側にするのが Windows の探索順で、
/// これを逆にすると PATH の後方にある `.exe` が前方の `.cmd` に勝ってしまう
#[cfg_attr(not(windows), allow(dead_code))]
fn find_in_windows_path(
    name: &str,
    path_dirs: &[String],
    pathext: &[String],
    extra_dirs: &[String],
    is_file: &dyn Fn(&str) -> bool,
) -> Option<String> {
    // 区切りを含む場合はコマンド名ではなくパス指定。PATH 探索の対象外
    if name.contains('\\') || name.contains('/') {
        return is_file(name).then(|| name.to_string());
    }
    for dir in path_dirs.iter().chain(extra_dirs.iter()) {
        let base = dir.trim_end_matches(['\\', '/']);
        if base.is_empty() {
            continue;
        }
        let bare = format!("{base}\\{name}");
        if is_file(&bare) {
            return Some(bare);
        }
        for ext in pathext {
            // `PATHEXT` は慣習的に大文字（`.EXE`）。Windows のパスは大小を区別しないので
            // 解決には影響しないが、そのまま連結すると `git.EXE` という見慣れない
            // パスを表示することになるため小文字へ寄せる
            let candidate = format!("{bare}{}", ext.to_ascii_lowercase());
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn ext() -> Vec<String> {
        dirs(&[".COM", ".EXE", ".BAT", ".CMD"])
    }

    #[test]
    fn pathextを補って実行ファイルを見つける() {
        let got = find_in_windows_path(
            "git",
            &dirs(&["C:\\Program Files\\Git\\cmd"]),
            &ext(),
            &[],
            &|p| p == "C:\\Program Files\\Git\\cmd\\git.exe",
        );
        assert_eq!(got.as_deref(), Some("C:\\Program Files\\Git\\cmd\\git.exe"));
    }

    #[test]
    fn 同じディレクトリではpathextの順序が優先度になる() {
        // npm シム（.cmd）と実体（.exe）が同居しても .exe を選ぶ
        let got = find_in_windows_path("claude", &dirs(&["C:\\bin"]), &ext(), &[], &|p| {
            p == "C:\\bin\\claude.exe" || p == "C:\\bin\\claude.cmd"
        });
        assert_eq!(got.as_deref(), Some("C:\\bin\\claude.exe"));
    }

    #[test]
    fn pathに無ければユーザー導入先を追って探す() {
        // PATH 更新が実行中プロセスへ伝播していないケース
        let got = find_in_windows_path(
            "claude",
            &dirs(&["C:\\Windows\\System32"]),
            &ext(),
            &dirs(&["C:\\Users\\u\\.local\\bin"]),
            &|p| p == "C:\\Users\\u\\.local\\bin\\claude.exe",
        );
        assert_eq!(
            got.as_deref(),
            Some("C:\\Users\\u\\.local\\bin\\claude.exe")
        );
    }

    #[test]
    fn pathの前方が後方より優先される() {
        let got = find_in_windows_path(
            "psmux",
            &dirs(&["C:\\first", "C:\\second"]),
            &ext(),
            &[],
            &|p| p.ends_with("psmux.exe"),
        );
        assert_eq!(got.as_deref(), Some("C:\\first\\psmux.exe"));
    }

    #[test]
    fn 拡張子つきの名前もそのまま解決できる() {
        let got = find_in_windows_path("psmux.exe", &dirs(&["C:\\bin"]), &ext(), &[], &|p| {
            p == "C:\\bin\\psmux.exe"
        });
        assert_eq!(got.as_deref(), Some("C:\\bin\\psmux.exe"));
    }

    #[test]
    fn 見つからなければnone() {
        let got = find_in_windows_path("nope", &dirs(&["C:\\bin"]), &ext(), &[], &|_| false);
        assert_eq!(got, None);
    }

    #[test]
    fn 区切りを含む名前はpath探索の対象にしない() {
        let found = find_in_windows_path(
            "C:\\tools\\psmux.exe",
            &dirs(&["C:\\bin"]),
            &ext(),
            &[],
            &|p| p == "C:\\tools\\psmux.exe",
        );
        assert_eq!(found.as_deref(), Some("C:\\tools\\psmux.exe"));
        let missing = find_in_windows_path(
            "C:\\tools\\psmux.exe",
            &dirs(&["C:\\bin"]),
            &ext(),
            &[],
            &|_| false,
        );
        assert_eq!(missing, None);
    }

    #[test]
    fn pathextに在る拡張子だけを実行できると見なす() {
        let ext = ext();
        assert!(has_executable_extension("claude.exe", &ext));
        // `PATHEXT` は大文字だがパスの大小は区別されない
        assert!(has_executable_extension("CLAUDE.EXE", &ext));
        assert!(has_executable_extension("claude.cmd", &ext));
        // 版つきの名前でも最後の拡張子だけを見る
        assert!(has_executable_extension("claude.exe", &ext));
        // 自己更新で改名された旧 exe は `.exe` で終わらない = 実行対象ではない
        assert!(!has_executable_extension(
            "claude.exe.old.1787816114562",
            &ext
        ));
        assert!(!has_executable_extension("claude", &ext));
        assert!(!has_executable_extension("readme.txt", &ext));
        assert!(!has_executable_extension("trailing.", &ext));
        assert!(!has_executable_extension("", &ext));
    }

    #[test]
    fn 版リソースの4成分目が0なら落とす() {
        // claude.exe の実測値（2.1.247.0）
        let ms = (2 << 16) | 1;
        let ls = 247 << 16;
        assert_eq!(format_file_version(ms, ls), "2.1.247");
        // 4 成分目があるものは残す
        assert_eq!(format_file_version(ms, (247 << 16) | 3), "2.1.247.3");
        assert_eq!(format_file_version(0, 0), "0.0.0");
    }

    /// 実環境の実行ファイル判定（**両プラットフォームで走る**）。
    /// #936 で Windows が無条件 `true` を返していたのを塞いだところ
    #[test]
    fn 実環境で実行ファイルとそれ以外を見分けられる() {
        let root = std::env::temp_dir().join(format!("tako-exe-936-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("テスト用ディレクトリ");

        // ディレクトリは拾わない / 存在しないものは拾わない
        assert!(!is_executable_file(&root));
        assert!(!is_executable_file(&root.join("nope")));

        // 実行できないファイル（Windows は拡張子が PATHEXT 外、unix は実行ビット無し）
        let plain = root.join("plain.txt");
        std::fs::write(&plain, b"x").unwrap();
        assert!(!is_executable_file(&plain));

        // 実環境の実行ファイル（`find` が解決したもの）は実行できると判定される
        let name = if cfg!(windows) { "cmd" } else { "sh" };
        let resolved = find(name).expect("基本コマンドを解決できない");
        assert!(
            is_executable_file(std::path::Path::new(&resolved)),
            "{resolved} を実行ファイルと見なせない"
        );

        assert!(
            root.starts_with(std::env::temp_dir())
                && root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("tako-exe-936-")),
            "テスト用ディレクトリ以外を消そうとした: {}",
            root.display()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 版リソースは Windows の exe だけが持つ。取れないときに `None` へ落ちること
    /// （呼び出し側は `claude --version` へ落ちる）まで含めて実環境で確かめる
    #[test]
    fn 版リソースは取れないときnoneを返す() {
        let missing = std::env::temp_dir().join("tako-936-does-not-exist.exe");
        assert_eq!(file_version(&missing), None);
        if !cfg!(windows) {
            let name = find("sh").expect("sh を解決できない");
            assert_eq!(file_version(std::path::Path::new(&name)), None);
        }
    }

    /// 実環境で必ず存在するコマンドを引けること（両プラットフォームの実装が動く証明）
    #[test]
    fn 実環境の基本コマンドを解決できる() {
        let name = if cfg!(windows) { "cmd" } else { "sh" };
        let found = find(name);
        assert!(found.is_some(), "{name} を解決できない");
        assert!(
            std::path::Path::new(found.as_deref().unwrap()).is_file(),
            "解決結果が実ファイルでない: {found:?}"
        );
    }
}

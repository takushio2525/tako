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
//! - **拡張子を持たない同名ファイルは解決結果にしない**（#1372）。npm の cmd-shim は
//!   `<name>`（`#!/bin/sh` のスクリプト）/ `<name>.cmd` / `<name>.ps1` の 3 つを置くので、
//!   「在れば採る」にすると PE でない裸のスクリプトが `.cmd` より先に採られる
//!   = **見つかるのに起動できない**（`Command::new` は `.exe` を足して探し、無ければ
//!   足す前のパスをそのまま `CreateProcessW` へ渡す = `std/src/sys/process/windows.rs`
//!   の `resolve_exe`）。採否の判定は [`is_executable_file`] と同じ `PATHEXT` の
//!   突き合わせ 1 本にしてあるので、**境界の中で答えが食い違わない**
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
/// （Windows の `.cmd` / `.bat` シムも Rust 標準ライブラリが解釈する）。
/// 返り値は必ず [`is_executable_file`] を満たす（#1372）
pub fn find(name: &str) -> Option<String> {
    imp::find(name)
}

/// [`find_with_timeout`] の結果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundedFind {
    /// 見つかった実行ファイル（[`is_executable_file`] を満たすものだけ）
    pub path: Option<String>,
    /// 上限を超えて打ち切った（= 見つからなかったのではなく**確かめられなかった**）
    pub timeout: Option<crate::probe::TimeoutNotice>,
}

/// [`find`] の**待ちに上限を持つ**版（#1730。`.agent/conventions.md`「外部コマンドを待つときは
/// 上限を持つ（Issue #1503）」）。
///
/// unix の [`find`] はログインシェルを起こして `command -v` を聞く = rc ファイル次第で
/// いくらでも待ちうる子プロセスで、待ちに上限が無い。Code Runner の実行環境の検出
/// （Tier P）はここを通し、待ちは `probe::output_with_timeout` の 1 実装に掛ける。
/// Windows は PATH の走査だけ（子プロセスを起こさない）なので [`find`] と同じ。
///
/// `name` はコマンド名だけを受ける（`[A-Za-z0-9._+-]`。シェルへ文字列として渡すため、
/// それ以外は探さずに `None`）。ログインシェルが別名・関数の定義を返したとき
/// （`alias uv=…`）や rc が余計な行を出したときは、**絶対パスの行だけ**を採る
pub fn find_with_timeout(name: &str, budget: std::time::Duration) -> BoundedFind {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    if !valid {
        return BoundedFind::default();
    }
    imp::find_with_timeout(name, budget)
}

/// ログインシェルの出力から「実行できる絶対パス」の行を選ぶ（純粋に近い部分。rc が
/// 挨拶文を出しても、`command -v` の答えは最後の行に来る）
#[cfg_attr(windows, allow(dead_code))]
fn pick_found_path(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .rev()
        .find(|l| l.starts_with('/') && is_executable_file(std::path::Path::new(l)))
        .map(str::to_string)
}

/// [`lookup`] の答え（#1769）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// 見つかった（[`is_executable_file`] を満たすパス）
    Found(String),
    NotFound,
    /// 探すのに起こしたログインシェルが上限までに返らなかった（打ち切った。子は kill 済み）
    TimedOut(std::time::Duration),
}

/// 上限つきの [`find`]（#1769）。unix のログインシェルは `probe::output_with_timeout` の
/// 1 実装で待つ（#1503 / #1532 の上限の作法。profile が入力を待つ形でも呼び手が固まらず、
/// 打ち切ったことを [`Lookup::TimedOut`] で知らせる）。Windows はシェルを起こさない
/// （PATH を自分で走査する）ので上限に掛かることは無い。
///
/// 返すパスは [`find`] と同じく必ず [`is_executable_file`] を満たす（`command -v` が
/// エイリアスや関数の定義を返しても「見つかった」にしない = #1372）
pub fn lookup(name: &str, budget: std::time::Duration) -> Lookup {
    imp::lookup(name, budget)
}

/// ログインシェル `shell` で `name` を探す（unix の [`lookup`] の本体。シェルを差し替えて
/// 上限・判定をテストで固定するために分けてある）
#[cfg(unix)]
fn login_shell_lookup(shell: &str, name: &str, budget: std::time::Duration) -> Lookup {
    let script = format!("command -v {}", crate::shell::quote_for_shell(name));
    match crate::probe::output_with_timeout(shell, &["-l", "-c", &script], budget) {
        crate::probe::Outcome::Done { status, stdout, .. } => {
            let path = String::from_utf8_lossy(&stdout).trim().to_string();
            if status.success() && is_executable_file(std::path::Path::new(&path)) {
                Lookup::Found(path)
            } else {
                Lookup::NotFound
            }
        }
        crate::probe::Outcome::TimedOut { waited, .. } => Lookup::TimedOut(waited),
        crate::probe::Outcome::Failed { .. } => Lookup::NotFound,
    }
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
        let output = std::process::Command::new(user_shell())
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

    /// [`find`] と同じ問い合わせを `probe::output_with_timeout`（待ちの 1 実装）で行う
    pub fn find_with_timeout(name: &str, budget: std::time::Duration) -> super::BoundedFind {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let lookup = format!("command -v {name}");
        let outcome = crate::probe::output_with_timeout(&shell, &["-l", "-c", &lookup], budget);
        let timeout = outcome.timeout_notice();
        let path = outcome
            .into_output()
            .filter(|o| o.status.success())
            .and_then(|o| super::pick_found_path(&String::from_utf8_lossy(&o.stdout)));
        super::BoundedFind { path, timeout }
    }

    /// 上限つきの探索（#1769。[`super::lookup`]）
    pub fn lookup(name: &str, budget: std::time::Duration) -> super::Lookup {
        super::login_shell_lookup(&user_shell(), name, budget)
    }

    fn user_shell() -> String {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
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
    /// シェルを起こさない（PATH を自分で走査する）ので上限に掛かることは無い（#1769）
    pub fn lookup(name: &str, _budget: std::time::Duration) -> super::Lookup {
        match find(name) {
            Some(path) => super::Lookup::Found(path),
            None => super::Lookup::NotFound,
        }
    }

    pub fn find(name: &str) -> Option<String> {
        super::find_in_windows_path(
            name,
            &split_path_list(std::env::var_os("PATH")),
            &pathext(),
            &user_install_dirs(),
            &|p| std::path::Path::new(p).is_file(),
        )
    }

    /// Windows の探索は PATH の走査だけ（子プロセスを起こさない）なので待ちが無い
    pub fn find_with_timeout(name: &str, _budget: std::time::Duration) -> super::BoundedFind {
        super::BoundedFind {
            path: find(name),
            timeout: None,
        }
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
    // 区切りを含む場合はコマンド名ではなくパス指定なので PATH 探索の対象外。
    // ただし**拡張子を補うぶんは同じ**（cmd.exe も完全修飾のパスへ `PATHEXT` を足す。
    // `Command::new` は `.exe` だけを足し、無ければ足す前のパスをそのまま渡す）
    if name.contains('\\') || name.contains('/') {
        return resolve_with_pathext(name, pathext, is_file);
    }
    for dir in path_dirs.iter().chain(extra_dirs.iter()) {
        let base = dir.trim_end_matches(['\\', '/']);
        if base.is_empty() {
            continue;
        }
        if let Some(found) = resolve_with_pathext(&format!("{base}\\{name}"), pathext, is_file) {
            return Some(found);
        }
    }
    None
}

/// 1 つの土台（`<dir>\<name>` かパス指定そのもの）を `PATHEXT` で補って解決する
/// （純粋関数。**macOS 上でもテストできる**）。
///
/// **土台をそのまま採るのは、名前が既に `PATHEXT` の拡張子を持つときだけ**（#1372）。
/// 判定を名前によらず「在れば採る」にすると、npm の cmd-shim
/// （`<name>` = `#!/bin/sh` のスクリプト / `<name>.cmd` / `<name>.ps1` の 3 つを置く）で
/// **PE ではない裸のスクリプトが `.cmd` より先に採られる** = 見つかるのに起動できない。
/// 同じモジュールの [`is_executable_file`] は同じパスに false を返すので、
/// 境界の中で答えが食い違う状態でもあった
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_with_pathext(
    stem: &str,
    pathext: &[String],
    is_file: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let file_name = stem.rsplit(['\\', '/']).next().unwrap_or(stem);
    if has_executable_extension(file_name, pathext) && is_file(stem) {
        return Some(stem.to_string());
    }
    for ext in pathext {
        // `PATHEXT` は慣習的に大文字（`.EXE`）。Windows のパスは大小を区別しないので
        // 解決には影響しないが、そのまま連結すると `git.EXE` という見慣れない
        // パスを表示することになるため小文字へ寄せる
        let candidate = format!("{stem}{}", ext.to_ascii_lowercase());
        if is_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 偽のログインシェル（`-l -c <script>` を無視して本文を実行する）を置く
    #[cfg(unix)]
    fn fake_shell(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// #1769: profile が入力を待つ形（返らないログインシェル）でも上限で打ち切って知らせる
    #[cfg(unix)]
    #[test]
    fn ログインシェルの探索は上限で打ち切る() {
        let dir = crate::test_residue::ScratchDir::new("exe-lookup-timeout");
        // `exec` で置き換える = 打ち切りの kill が眠る本人に当たる（孫を残さない = #1748）
        let shell = fake_shell(dir.path(), "hang", "exec sleep 30");
        // 状態で見る（実時間の予算は assert しない = conventions「効果を測る単体テストは実時間で
        // 比べない」）。上限が無ければ 30 秒眠ったあと空の出力 = NotFound で返るので、
        // TimedOut が返ったこと自体が「上限で打ち切った」の証拠
        let got = login_shell_lookup(&shell, "x", std::time::Duration::from_millis(300));
        assert!(matches!(got, Lookup::TimedOut(_)), "{got:?}");
    }

    /// #1769: 見つかったことにするのは実行できるファイルだけ（エイリアス・失敗・空は見つからない）
    #[cfg(unix)]
    #[test]
    fn ログインシェルの答えは実行できるファイルだけを採る() {
        let dir = crate::test_residue::ScratchDir::new("exe-lookup-answer");
        let budget = std::time::Duration::from_secs(10);
        let found = fake_shell(dir.path(), "found", "echo /bin/sh");
        assert_eq!(
            login_shell_lookup(&found, "x", budget),
            Lookup::Found("/bin/sh".into())
        );
        for (name, body) in [
            ("alias", "echo \"alias x='y'\""),
            ("fail", "exit 1"),
            ("empty", "true"),
            ("missing", "echo /nonexistent/tako-1769"),
        ] {
            let shell = fake_shell(dir.path(), name, body);
            assert_eq!(
                login_shell_lookup(&shell, "x", budget),
                Lookup::NotFound,
                "{name}"
            );
        }
    }

    fn dirs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn ext() -> Vec<String> {
        dirs(&[".COM", ".EXE", ".BAT", ".CMD"])
    }

    /// #1730: 探す名前はコマンド名だけ（シェルへ文字列で渡すので、それ以外は起こさずに None）
    #[test]
    fn find_with_timeoutはコマンド名以外を探さない() {
        for bad in ["", "uv; rm -rf x", "a b", "$(id)", "../uv", "u`v`"] {
            let got = find_with_timeout(bad, std::time::Duration::from_secs(1));
            assert_eq!(got, BoundedFind::default(), "{bad:?}");
        }
    }

    /// #1730: rc が挨拶文を出しても `command -v` の答え（実行できる絶対パスの最後の行）だけを採る。
    /// 別名・関数の定義は採らない
    #[cfg(unix)]
    #[test]
    fn ログインシェルの出力から実行できる絶対パスだけを採る() {
        let sh = "/bin/sh";
        assert_eq!(
            pick_found_path(&format!("Welcome!\n{sh}\n")).as_deref(),
            Some(sh)
        );
        assert_eq!(pick_found_path("alias uv='uvx'\n"), None);
        assert_eq!(pick_found_path("/no/such/tako-1730-bin\n"), None);
        assert_eq!(pick_found_path(""), None);
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

    /// **#1372**: npm でグローバル導入した CLI の形。cmd-shim は
    /// `<name>`（`#!/bin/sh` のスクリプト）/ `<name>.cmd` / `<name>.ps1` の 3 つを置く。
    /// 裸のスクリプトを採ると `Command::new` が
    /// 「有効な Win32 アプリケーションではありません」で落ちる（PE ではないので）
    #[test]
    fn npmのシム構成では裸のスクリプトではなくcmdを採る() {
        let npm = "C:\\Users\\testuser\\AppData\\Roaming\\npm";
        let script = format!("{npm}\\claude");
        let cmd = format!("{npm}\\claude.cmd");
        let ps1 = format!("{npm}\\claude.ps1");
        let got = find_in_windows_path("claude", &dirs(&[npm]), &ext(), &[], &|p| {
            p == script || p == cmd || p == ps1
        });
        assert_eq!(got.as_deref(), Some(cmd.as_str()));
    }

    /// **#1372**: 裸のスクリプトだけが在る導入では `None` を返す。
    /// 「見つかったのに起動できない」より「未検出」のほうが原因に辿れる
    /// （`tako setup` の案内・`stale_binary` の版取得が空振りせず「無い」と言える）
    #[test]
    fn 拡張子を持たない実体だけなら見つけたことにしない() {
        let got = find_in_windows_path("claude", &dirs(&["C:\\bin"]), &ext(), &[], &|p| {
            p == "C:\\bin\\claude"
        });
        assert_eq!(got, None);
    }

    /// **#1372 の整合テスト**: 境界 B16 の 2 つの答えを食い違わせない。
    /// `find` が返したパスは必ず [`is_executable_file`] を満たす
    /// （Windows の判定は `has_executable_extension` そのものなので、
    /// この純粋関数の組で macOS 上から固定できる）
    #[test]
    fn findの戻り値は常に実行できる拡張子を持つ() {
        let ext = ext();
        // 「拡張子を持たない実体が同居する」構成を並べる（1 つめが npm の cmd-shim）
        let layouts: &[&[&str]] = &[
            &[
                "C:\\bin\\claude",
                "C:\\bin\\claude.cmd",
                "C:\\bin\\claude.ps1",
            ],
            &["C:\\bin\\claude"],
            &["C:\\bin\\claude", "C:\\bin\\claude.exe"],
            &["C:\\bin\\psmux.exe"],
            &["C:\\tools\\claude", "C:\\tools\\claude.cmd"],
        ];
        const NAMES: &[&str] = &[
            "claude",
            "claude.exe",
            "claude.ps1",
            "psmux",
            "psmux.exe",
            "C:\\tools\\claude",
            "C:\\tools\\claude.cmd",
        ];
        for files in layouts {
            for name in NAMES {
                let got = find_in_windows_path(name, &dirs(&["C:\\bin"]), &ext, &[], &|p| {
                    files.contains(&p)
                });
                let Some(path) = got else {
                    continue;
                };
                assert!(
                    files.contains(&path.as_str()),
                    "在りもしないパスを返した: {path}（構成 {files:?} / 名前 {name}）"
                );
                let file_name = path.rsplit(['\\', '/']).next().unwrap_or_default();
                assert!(
                    has_executable_extension(file_name, &ext),
                    "find が起動できないパスを返した: {path}（構成 {files:?} / 名前 {name}）\n\
                     → is_executable_file が false を返すものを解決結果にしてはいけない（#1372）"
                );
            }
        }
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

//! 「新しく開いたターミナルが見る PATH」の読み書き（抽象境界 B23。#1057）
//!
//! ## なぜ境界が要るか
//!
//! macOS で「PATH を通す」＝**ログインシェルの profile へ 1 行足す**
//! （[`crate::shell_profile`]）。Windows には**ログインシェルの profile という概念が無い**:
//! PATH はレジストリ（`HKCU\Environment\Path` = ユーザー、
//! `HKLM\…\Session Manager\Environment\Path` = マシン）に持たれ、プロセス起動時に
//! そこから組まれた値が渡る。だから profile へ書いても cmd.exe や VS Code の
//! 統合ターミナルには効かない。
//!
//! ここは**その差だけ**を閉じ込める。unix では [`is_supported`] が false を返し、
//! 呼び出し側（`setup_bootstrap::ensure_path`）は従来どおり profile 経路へ行く。
//!
//! 「新しく開いたターミナル」には **tako の新しいペイン**も含む（#2428）。Windows では
//! ペインを起こすたびにレジストリの PATH を読み直して渡す（[`pane_path`]。Windows Terminal と同じ）。
//! unix のペインはログインシェルとして profile を読み直すので、ここは何もしない。
//!
//! ## 値の算術は純粋関数
//!
//! 追記・除去・突き合わせは環境変数を読まない純粋関数として書く
//! （lookup を引数で受ける）。**macOS の `cargo test` から Windows の値の形を
//! 検証できる**のが要点で、レジストリを触らずに冪等性まで確かめられる。
//!
//! ## 実測メモ（2026-09-01・Windows 11。#1057）
//!
//! - `HKCU\Environment\Path` の値の種別は **`ExpandString`（REG_EXPAND_SZ）**。
//!   `setx` は 1024 文字で切り落とすうえ種別を `String` へ変えてしまうので使わない
//! - 読み出しは `GetValue(..., 'DoNotExpandEnvironmentNames')` で**生の値**を取る。
//!   素の `Get-ItemProperty` は `%USERPROFILE%` を展開してしまい、書き戻すと
//!   ユーザーの可搬な記述が失われる
//! - Claude Code の公式インストーラは `~\.local\bin` をここへ足す（実機の値で確認）。
//!   つまり通常の導入では tako の追記は**何もしない**（冪等）

use std::path::Path;

/// レジストリから読んだ生の PATH 値
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPathValue {
    /// 展開前の値（`%USERPROFILE%\.local\bin` のような記述がそのまま入る）
    pub raw: String,
    /// レジストリ値の種別（`ExpandString` / `String`）。**書き戻すときも同じ種別を使う**
    pub kind: String,
}

impl UserPathValue {
    /// 種別が分からないときの既定（Windows の `Path` は REG_EXPAND_SZ が慣習）
    pub fn new(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            kind: "ExpandString".to_string(),
        }
    }
}

/// このプラットフォームに「プロセスの外に永続する PATH」の概念があるか。
/// unix は profile ファイルが担うので false
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// PATH エントリの区切り
const SEP: char = ';';

/// エントリを 1 つずつ取り出す（空要素と前後の空白・引用符は落とす）
pub fn split_entries(raw: &str) -> Vec<&str> {
    raw.split(SEP)
        .map(|e| e.trim().trim_matches('"'))
        .filter(|e| !e.is_empty())
        .collect()
}

/// `%VAR%` を展開する（**純粋関数**。lookup が None を返す変数はそのまま残す）
pub fn expand_in(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match lookup(name) {
                    Some(v) => out.push_str(&v),
                    // 解決できない（`%%` や未定義変数）ときは元の記述を保つ
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Windows のパス比較用に正規化する。
///
/// **`Path` の比較を使わない**のが要点: Rust の `Path` は Windows でも
/// 大小を区別するので `C:\Users\…` と `c:\users\…` が別物になり、
/// macOS のテストでは逆に大小を無視してしまう（環境で答えが変わる）
fn normalize(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// エントリが `dir` を指しているか（`%VAR%` を展開してから比べる）
pub fn entry_matches_in(entry: &str, dir: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    let entry = expand_in(entry, lookup);
    normalize(&entry) == normalize(&dir.to_string_lossy())
}

/// PATH に `dir` が入っているか
pub fn contains_entry_in(raw: &str, dir: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    split_entries(raw)
        .into_iter()
        .any(|e| entry_matches_in(e, dir, lookup))
}

/// `dir` を末尾へ足した値。**既に入っていれば `None`**（= 何もしない = 冪等）
///
/// 先頭ではなく末尾へ足すのは、ユーザーが自分で並べた優先順位を動かさないため。
/// tako が入れるのはランチャーの置き場所だけで、同名コマンドの奪い合いはしない
pub fn append_entry_in(
    raw: &str,
    dir: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if contains_entry_in(raw, dir, lookup) {
        return None;
    }
    let dir = dir.to_string_lossy().replace('/', "\\");
    let trimmed = raw.trim_end();
    if trimmed.is_empty() {
        return Some(dir);
    }
    // 末尾の `;` は Windows の PATH では珍しくない（実機の値も付いている）。
    // **二重にせず、かつ有無を保つ**: 保たないと `remove_entry_in` で戻したとき
    // 元のバイト列と 1 文字違う（実機で踏んだ = `identical_to_before=false`）
    let had_trailing = trimmed.ends_with(SEP);
    let base = trimmed.trim_end_matches(SEP);
    let mut next = format!("{base}{SEP}{dir}");
    if had_trailing {
        next.push(SEP);
    }
    Some(next)
}

/// `dir` のエントリを取り除いた値。**入っていなければ `None`**
///
/// 区切りの見た目（末尾の `;` の有無）は元の値に合わせる
pub fn remove_entry_in(
    raw: &str,
    dir: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if !contains_entry_in(raw, dir, lookup) {
        return None;
    }
    let had_trailing = raw.trim_end().ends_with(SEP);
    let kept: Vec<&str> = raw
        .split(SEP)
        .filter(|e| {
            let trimmed = e.trim();
            !trimmed.is_empty() && !entry_matches_in(trimmed, dir, lookup)
        })
        .collect();
    let mut joined = kept.join(&SEP.to_string());
    if had_trailing && !joined.is_empty() {
        joined.push(SEP);
    }
    Some(joined)
}

/// 実環境の `%VAR%` 解決
fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// 実環境で `dir` が入っているか
pub fn contains_entry(raw: &str, dir: &Path) -> bool {
    contains_entry_in(raw, dir, &env_lookup)
}

/// 実環境で `dir` を末尾へ足した値（既に入っていれば `None`）
pub fn append_entry(raw: &str, dir: &Path) -> Option<String> {
    append_entry_in(raw, dir, &env_lookup)
}

/// 実環境で `dir` のエントリを取り除いた値（入っていなければ `None`）
pub fn remove_entry(raw: &str, dir: &Path) -> Option<String> {
    remove_entry_in(raw, dir, &env_lookup)
}

// --- 新しいペインへ渡す PATH（#2428）---
//
// ## なぜ要るか
//
// tako のペインは tako-app を起動したときの env を継ぐ。Windows は PATH をレジストリに持ち、
// プロセスへは**起動時に 1 回だけ**渡す。だから tako を開いたままインストーラが
// `HKCU\Environment\Path` へ `~\.local\bin` を足しても、新しいタブの PATH には載らず、
// `tako setup` が案内した「次の 1 手」（`claude auth login`）が tako の再起動まで通らなかった。
// macOS はペインがログインシェルとして起き、`.zprofile` を読み直すので起きない。
//
// ## この形（Windows Terminal と同じく、ペインを起こすたびに読み直す）
//
// 起動時の PATH は**1 バイトも変えず**、そこに無いレジストリのエントリだけを末尾へ足す。
// 起動時の PATH にしか無いもの（tako を起こした側が足したもの）を落とさず、
// 起動時に引けたコマンドの引き先も動かさない（後ろへ足すだけなので順位は変わらない）。

/// Windows の環境変数 1 本の上限（UTF-16 の要素数）。足した結果がこれを超えるエントリは足さない
pub const MAX_VAR_LEN: usize = 32_767;

/// レジストリに持たれた PATH の生の値（`None` = 値が無い・読めなかった）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryPaths {
    /// `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment\Path`
    pub machine: Option<String>,
    /// `HKCU\Environment\Path`
    pub user: Option<String>,
}

/// 新しいペインへ渡す PATH を組む（**純粋関数**。#2428）。
///
/// `process` は tako-app が起動時に受け取った PATH。レジストリの値はマシン → ユーザーの順に
/// 読み（Windows が組む順と同じ）、`%VAR%` は `lookup` で展開してから突き合わせる
/// （展開できない変数は Windows と同じく記述のまま）。大小・`/`・末尾の `\` の違いは同じ場所として扱う。
///
/// 戻り値は**足すものが在るときだけ** `Some`。`None` なら呼び手は PATH を渡さず、
/// 起動時の値をそのまま継がせる（レジストリが読めない・値が無いときも同じ）
pub fn pane_path_in(
    process: &str,
    registry: &RegistryPaths,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut seen: std::collections::HashSet<String> =
        split_entries(process).into_iter().map(normalize).collect();
    // 起動時の値は区切りの見た目も含めてそのまま使う。末尾の `;` と空白だけは、足す区切りと
    // 二重にしない（空白と区切りだけの値から空の項目を作らない）
    let base = process.trim_end_matches(|c: char| c == SEP || c.is_whitespace());
    let mut len = base.encode_utf16().count();
    let mut added: Vec<String> = Vec::new();
    for raw in [registry.machine.as_deref(), registry.user.as_deref()]
        .into_iter()
        .flatten()
    {
        for entry in split_entries(raw) {
            let expanded = expand_in(entry, lookup);
            if !seen.insert(normalize(&expanded)) {
                continue;
            }
            let extra = expanded.encode_utf16().count() + usize::from(len > 0);
            // 上限を超えるぶんは足さない（起動時の値は削らない）
            if len + extra > MAX_VAR_LEN {
                continue;
            }
            len += extra;
            added.push(expanded);
        }
    }
    if added.is_empty() {
        return None;
    }
    let mut out = base.to_string();
    for entry in added {
        if !out.is_empty() {
            out.push(SEP);
        }
        out.push_str(&entry);
    }
    Some(out)
}

/// `dir` がどこから引けるか（#2428）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach {
    /// いまのプロセスの PATH に入っている（= このプロセスを起こしたタブで引ける）
    pub this_process: bool,
    /// 新しいペインの PATH に入る（= tako の新しいタブで引ける）
    pub new_pane: bool,
}

/// [`Reach`] の判定（**純粋関数**）。新しいペインの PATH は [`pane_path_in`] と同じ組み方で見る
/// （判定とペインの実物が同じ材料・同じ規則を通る）
pub fn reach_in(
    dir: &Path,
    process: &str,
    registry: &RegistryPaths,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Reach {
    let this_process = contains_entry_in(process, dir, lookup);
    let new_pane = this_process
        || pane_path_in(process, registry, lookup)
            .is_some_and(|path| contains_entry_in(&path, dir, lookup));
    Reach {
        this_process,
        new_pane,
    }
}

/// ペインの env へ PATH を載せる（**純粋関数**）。呼び出し側が PATH を明示していれば
/// （大小を問わず）触らない。`path` は明示が無いときだけ呼ぶ
pub fn apply_pane_path(
    env: &mut std::collections::HashMap<String, String>,
    path: impl FnOnce() -> Option<(String, String)>,
) {
    if env.keys().any(|key| key.eq_ignore_ascii_case("PATH")) {
        return;
    }
    if let Some((key, value)) = path() {
        env.insert(key, value);
    }
}

/// `TAKO_2428_LEGACY=1` で #2428 前へ戻す（ペインは起動時の PATH をそのまま継ぎ、
/// setup の判定と文面も前のまま）。同一バイナリで A/B を取るための逃げ道
pub fn legacy_2428() -> bool {
    std::env::var_os("TAKO_2428_LEGACY").is_some()
}

/// 新しいペインがレジストリの PATH を読み直すか（Windows かつ A/B の旧アームでない）
pub fn rereads_for_new_panes() -> bool {
    is_supported() && !legacy_2428()
}

/// 起動時の PATH（読めない形なら `None` = 触らない。空にして継承の値ごと落とさないため）
fn process_path() -> Option<String> {
    match std::env::var_os("PATH") {
        Some(value) => value.into_string().ok(),
        None => Some(String::new()),
    }
}

/// マシン / ユーザーの `Path` の生の値（展開しない。unix は両方 `None`）
pub fn registry_paths() -> RegistryPaths {
    imp::registry_paths()
}

/// 新しいペインへ渡す PATH（`(変数名, 値)`）。足すものが無ければ `None`（= 起動時の値を継ぐ）。
/// unix は常に `None`（ペインがログインシェルとして profile を読み直す）
pub fn pane_path() -> Option<(String, String)> {
    if !rereads_for_new_panes() {
        return None;
    }
    let value = pane_path_in(&process_path()?, &registry_paths(), &env_lookup)?;
    // 変数名は継いでいる表記に揃える（Windows の慣習は `Path`。名前の大小は区別されない）
    let key = std::env::vars_os()
        .find_map(|(key, _)| {
            key.to_str()
                .filter(|key| key.eq_ignore_ascii_case("PATH"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Path".to_string());
    Some((key, value))
}

/// 実環境の [`Reach`]。[`rereads_for_new_panes`] が false の環境では呼ばない
/// （unix はレジストリを持たず、新しいペインの PATH はログインシェルが決める）
pub fn reach(dir: &Path) -> Reach {
    let process = process_path().unwrap_or_default();
    reach_in(dir, &process, &registry_paths(), &env_lookup)
}

/// ユーザー PATH（`HKCU\Environment\Path`）の生の値を読む
pub fn read() -> Result<UserPathValue, String> {
    imp::read()
}

/// ユーザー PATH を書く（**種別を保って**書き戻す）
pub fn write(value: &UserPathValue) -> Result<(), String> {
    imp::write(value)
}

/// 新しく開いたターミナルが見る PATH（マシン + ユーザー）。
/// 判定にだけ使うので、順序は Windows の実際の組み立てに合わせなくてよい
pub fn effective() -> Result<String, String> {
    imp::effective()
}

#[cfg(not(windows))]
mod imp {
    use super::UserPathValue;

    const UNSUPPORTED: &str =
        "このプラットフォームにはプロセス外に永続する PATH がありません（profile 経路を使ってください）";

    pub fn read() -> Result<UserPathValue, String> {
        Err(UNSUPPORTED.to_string())
    }

    pub fn write(_value: &UserPathValue) -> Result<(), String> {
        Err(UNSUPPORTED.to_string())
    }

    pub fn effective() -> Result<String, String> {
        Err(UNSUPPORTED.to_string())
    }

    pub fn registry_paths() -> super::RegistryPaths {
        super::RegistryPaths::default()
    }
}

#[cfg(windows)]
mod imp {
    use super::UserPathValue;

    /// 読み出しスクリプト。`DoNotExpandEnvironmentNames` で**生の値**を取る。
    ///
    /// 出力は行頭の目印つき 1 行ずつにする（PATH の値に改行は入らないので、
    /// JSON を組むより素直で ConvertTo-Json の版差も踏まない）
    const READ_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'\n\
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n\
$key = Get-Item -LiteralPath 'HKCU:\\Environment'\n\
$raw = ''\n\
$kind = 'ExpandString'\n\
if ($key.GetValueNames() -contains 'Path') {\n\
  $raw = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')\n\
  $kind = [string]$key.GetValueKind('Path')\n\
}\n\
Write-Output ('TAKO_KIND=' + $kind)\n\
Write-Output ('TAKO_RAW=' + $raw)\n";

    /// 書き込みスクリプト。**値と種別は env で渡す**（スクリプト本文へ埋めないので
    /// 引用符の入れ子もコードページも問題にならない）。
    ///
    /// 最後の `SendMessageTimeout(WM_SETTINGCHANGE)` は、Explorer に環境変数の
    /// 再読み込みを促して**次に開くターミナル**へ反映させるためのもの。
    /// 失敗しても書き込み自体は成立しているので握って続ける
    const WRITE_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'\n\
New-ItemProperty -LiteralPath 'HKCU:\\Environment' -Name 'Path' \
-Value $env:TAKO_USER_PATH_VALUE -PropertyType $env:TAKO_USER_PATH_KIND -Force | Out-Null\n\
try {\n\
  $sig = '[DllImport(\"user32.dll\", SetLastError=true, CharSet=CharSet.Auto)] \
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, \
uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'\n\
  $api = Add-Type -MemberDefinition $sig -Name 'TakoEnvBroadcast' -Namespace 'Tako' -PassThru\n\
  $res = [UIntPtr]::Zero\n\
  $api::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 200, [ref]$res) | Out-Null\n\
} catch { }\n";

    /// PowerShell を 1 回起こす。`-EncodedCommand` なので引用符もコードページも関与しない
    fn run(script: &str, env: &[(&str, &str)]) -> Result<String, String> {
        let program = crate::platform::exe::find("powershell")
            .or_else(|| crate::platform::exe::find("pwsh"))
            .unwrap_or_else(|| "powershell.exe".to_string());
        let mut command = std::process::Command::new(&program);
        crate::platform::process::no_console_window(&mut command);
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &crate::platform::shell::encode_powershell_command(script),
        ]);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{program} を起動できません: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "PowerShell がユーザー PATH の操作に失敗しました（exit {}）: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn read() -> Result<UserPathValue, String> {
        let out = run(READ_SCRIPT, &[])?;
        super::parse_read_output(&out)
    }

    pub fn write(value: &UserPathValue) -> Result<(), String> {
        run(
            WRITE_SCRIPT,
            &[
                ("TAKO_USER_PATH_VALUE", value.raw.as_str()),
                ("TAKO_USER_PATH_KIND", value.kind.as_str()),
            ],
        )?;
        Ok(())
    }

    pub fn effective() -> Result<String, String> {
        let user = read()?.raw;
        let machine = std::env::var("PATH").unwrap_or_default();
        Ok(format!("{machine};{user}"))
    }

    // ペインを起こすたびに読む（#2428）ので PowerShell は起こさず API で直接読む（1 回あたり µs 単位）。
    // `windows-sys` を足さず必要な 3 関数だけ宣言する方針は `platform::install_info` と同じ
    const HKEY_CURRENT_USER: isize = -2147483647; // 0x80000001
    const HKEY_LOCAL_MACHINE: isize = -2147483646; // 0x80000002
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_MORE_DATA: i32 = 234;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const USER_SUBKEY: &str = "Environment";
    const MACHINE_SUBKEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

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

    /// マシン / ユーザーの `Path` の生の値（REG_EXPAND_SZ も展開しない = `RegQueryValueExW` の既定）
    pub fn registry_paths() -> super::RegistryPaths {
        super::RegistryPaths {
            machine: read_path_value(HKEY_LOCAL_MACHINE, MACHINE_SUBKEY),
            user: read_path_value(HKEY_CURRENT_USER, USER_SUBKEY),
        }
    }

    /// キーを開いて `Path` を読む。キーや値が無い・読めないときは `None`（呼び手は継承の値へ落ちる）
    fn read_path_value(root: isize, subkey: &str) -> Option<String> {
        let subkey = wide(subkey);
        let mut hkey: isize = 0;
        // SAFETY: subkey は NUL 終端の生存するバッファ、hkey はスタック上の変数
        let rc = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let value = query_string(hkey, "Path");
        // SAFETY: RegOpenKeyExW が成功したハンドルだけを閉じる
        unsafe { RegCloseKey(hkey) };
        value
    }

    /// REG_SZ / REG_EXPAND_SZ の値を読む。長さの問い合わせと読み出しのあいだに値が伸びたら
    /// （setup の書き込みと重なったとき）数回だけ取り直す
    fn query_string(hkey: isize, name: &str) -> Option<String> {
        let name = wide(name);
        for _ in 0..3 {
            let mut kind: u32 = 0;
            let mut len: u32 = 0;
            // SAFETY: 出力ポインタはスタック上の生存する変数。データバッファに null を
            // 渡すのは「必要なバイト数だけ問い合わせる」規定の呼び出し方
            let rc = unsafe {
                RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut len,
                )
            };
            if rc != ERROR_SUCCESS || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
                return None;
            }
            // len はバイト数。奇数長でも溢れないよう 2 要素ぶん余裕を持たせる
            let mut buf = vec![0u16; len as usize / 2 + 2];
            let mut got = (buf.len() * 2) as u32;
            // SAFETY: buf は got バイト分を確保済みで、そのバイト数を got に入れて渡している
            let rc = unsafe {
                RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut kind,
                    buf.as_mut_ptr() as *mut u8,
                    &mut got,
                )
            };
            match rc {
                ERROR_SUCCESS => {
                    let units = (got as usize / 2).min(buf.len());
                    return Some(super::decode_registry_string(&buf[..units]));
                }
                ERROR_MORE_DATA => continue,
                _ => return None,
            }
        }
        None
    }
}

/// レジストリの文字列値（UTF-16）を NUL の手前までで `String` にする。
/// 値に NUL 終端が含まれることも含まれないこともある。**前後の空白は落とさない**（生の値を保つ）。
///
/// 呼ぶのは Windows の `imp` だけだが、純粋関数なので macOS からも検証できるよう cfg で切らない
/// （`platform::install_info::trim_nul` と同じ方針）
#[cfg_attr(not(windows), allow(dead_code))]
fn decode_registry_string(units: &[u16]) -> String {
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// 読み出しスクリプトの出力をほどく（**純粋関数**なので macOS からも検証できる）
pub fn parse_read_output(stdout: &str) -> Result<UserPathValue, String> {
    let mut kind = None;
    let mut raw = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("TAKO_KIND=") {
            kind = Some(rest.trim_end_matches('\r').to_string());
        } else if let Some(rest) = line.strip_prefix("TAKO_RAW=") {
            raw = Some(rest.trim_end_matches('\r').to_string());
        }
    }
    match (raw, kind) {
        (Some(raw), Some(kind)) => Ok(UserPathValue { raw, kind }),
        _ => Err(format!(
            "ユーザー PATH の読み出し結果を解釈できません: {:?}",
            stdout.trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 実機（2026-09-01・Windows 11）の生の値を模したもの。
    /// **実ユーザー名は置かない**（#927。`winuser` はプレースホルダ）
    const REAL_RAW: &str = "C:\\Users\\winuser\\.cargo\\bin;C:\\WINDOWS\\system32;\
C:\\Program Files\\Git\\cmd;C:\\Users\\winuser\\AppData\\Roaming\\npm;\
C:\\Users\\winuser\\.local\\bin;C:\\Users\\winuser\\dev\\tako\\target\\debug;";

    /// 実機の値からランチャーの置き場所だけを抜いた形（末尾の `;` はそのまま）
    const REAL_RAW_WITHOUT_LAUNCHER: &str =
        "C:\\Users\\winuser\\.cargo\\bin;C:\\WINDOWS\\system32;\
C:\\Program Files\\Git\\cmd;C:\\Users\\winuser\\AppData\\Roaming\\npm;\
C:\\Users\\winuser\\dev\\tako\\target\\debug;";

    fn lookup(name: &str) -> Option<String> {
        match name {
            "USERPROFILE" => Some("C:\\Users\\winuser".to_string()),
            _ => None,
        }
    }

    fn dir(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn 実機の値からランチャーディレクトリを見つける() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        assert!(contains_entry_in(REAL_RAW, &launcher, &lookup));
        // 追記は何もしない（**冪等**）
        assert_eq!(append_entry_in(REAL_RAW, &launcher, &lookup), None);
    }

    #[test]
    fn 大小と区切りの違いを同じ場所として扱う() {
        let launcher = dir("c:/users/winuser/.local/bin/");
        assert!(
            contains_entry_in(REAL_RAW, &launcher, &lookup),
            "Windows のパスは大小を区別せず `/` も区切り"
        );
    }

    #[test]
    fn 変数記述のエントリも展開して突き合わせる() {
        let raw = "C:\\WINDOWS\\system32;%USERPROFILE%\\.local\\bin";
        assert!(contains_entry_in(
            raw,
            &dir("C:\\Users\\winuser\\.local\\bin"),
            &lookup
        ));
        // 展開できない変数は残す（値を壊さない）
        assert_eq!(expand_in("%NOPE%\\bin", &lookup), "%NOPE%\\bin".to_string());
        assert_eq!(expand_in("a%%b", &lookup), "a%%b".to_string());
        assert_eq!(expand_in("50% done", &lookup), "50% done".to_string());
    }

    #[test]
    fn 追記は末尾へ入り二重の区切りを作らない() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        // 末尾の `;` は二重にせず、**有無はそのまま保つ**（往復で元へ戻すため）
        let raw = "C:\\WINDOWS\\system32;";
        let appended = append_entry_in(raw, &launcher, &lookup).expect("追記される");
        assert_eq!(
            appended,
            "C:\\WINDOWS\\system32;C:\\Users\\winuser\\.local\\bin;"
        );
        // `;` で終わっていない値には付けない
        assert_eq!(
            append_entry_in("C:\\WINDOWS\\system32", &launcher, &lookup).as_deref(),
            Some("C:\\WINDOWS\\system32;C:\\Users\\winuser\\.local\\bin")
        );
        // 2 回目は None（冪等）
        assert_eq!(append_entry_in(&appended, &launcher, &lookup), None);
        // 空の値でも壊れない
        assert_eq!(
            append_entry_in("", &launcher, &lookup).as_deref(),
            Some("C:\\Users\\winuser\\.local\\bin")
        );
    }

    #[test]
    fn 除去は元の見た目を保ちつつ該当エントリだけ落とす() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        let removed = remove_entry_in(REAL_RAW, &launcher, &lookup).expect("除去される");
        assert!(!contains_entry_in(&removed, &launcher, &lookup));
        // 他のエントリは 1 つも消えていない
        for entry in split_entries(REAL_RAW) {
            if entry_matches_in(entry, &launcher, &lookup) {
                continue;
            }
            assert!(
                split_entries(&removed).contains(&entry),
                "{entry} が消えている: {removed}"
            );
        }
        // 末尾の `;` の有無は元に合わせる
        assert!(removed.ends_with(';'), "元の末尾 `;` を保つ: {removed}");
        // 入っていなければ None
        assert_eq!(remove_entry_in(&removed, &launcher, &lookup), None);
    }

    /// **往復で 1 バイトも変えない**。末尾の `;` の有無まで保つ
    /// （実機の値は `;` で終わっており、保たないと `undo-path` の復帰が
    /// `identical_to_before=false` になる = #1057 の実測で踏んだ）
    #[test]
    fn 追記と除去の往復で元のバイト列へ戻る() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        for base in [
            "C:\\WINDOWS\\system32;C:\\Program Files\\Git\\cmd",
            // 実機と同じ「末尾に `;` が付く」形
            "C:\\WINDOWS\\system32;C:\\Program Files\\Git\\cmd;",
            REAL_RAW_WITHOUT_LAUNCHER,
        ] {
            let appended = append_entry_in(base, &launcher, &lookup).expect("追記される");
            let back = remove_entry_in(&appended, &launcher, &lookup).expect("除去される");
            assert_eq!(back, base, "往復で元のバイト列へ戻る（base={base:?}）");
        }
    }

    #[test]
    fn 読み出し出力をほどく() {
        let out = "TAKO_KIND=ExpandString\r\nTAKO_RAW=C:\\a;C:\\b\r\n";
        let value = parse_read_output(out).expect("解釈できる");
        assert_eq!(value.kind, "ExpandString");
        assert_eq!(value.raw, "C:\\a;C:\\b");
        // 空の PATH も正しく空として読む（欠落と区別する）
        let empty = parse_read_output("TAKO_KIND=ExpandString\nTAKO_RAW=\n").expect("解釈できる");
        assert_eq!(empty.raw, "");
        // 目印が無ければエラー（黙って空として扱わない）
        assert!(parse_read_output("なにか別の出力").is_err());
    }

    #[test]
    fn 種別の既定はexpandstring() {
        assert_eq!(UserPathValue::new("C:\\a").kind, "ExpandString");
    }

    // --- 新しいペインへ渡す PATH（#2428）---

    /// tako-app が起動時に受け取った PATH（Windows が展開済みの形。ランチャーの置き場所はまだ無い）
    const PROCESS: &str = "C:\\WINDOWS\\system32;C:\\WINDOWS;\
C:\\Program Files\\Git\\cmd;C:\\Users\\winuser\\AppData\\Roaming\\npm";

    fn registry(machine: Option<&str>, user: Option<&str>) -> RegistryPaths {
        RegistryPaths {
            machine: machine.map(str::to_string),
            user: user.map(str::to_string),
        }
    }

    #[test]
    fn インストーラが足した置き場所が新しいペインのpathに入る() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        // ユーザーの値は展開前の記述（REG_EXPAND_SZ）。インストーラの後に `~\.local\bin` が増えた
        let reg = registry(
            Some("%SystemRoot%\\system32;%SystemRoot%;C:\\Program Files\\Git\\cmd"),
            Some("%USERPROFILE%\\AppData\\Roaming\\npm;%USERPROFILE%\\.local\\bin"),
        );
        let lookup = |name: &str| match name {
            "USERPROFILE" => Some("C:\\Users\\winuser".to_string()),
            "SystemRoot" => Some("C:\\WINDOWS".to_string()),
            _ => None,
        };
        let path = pane_path_in(PROCESS, &reg, &lookup).expect("増えたぶんがある");
        // 起動時の値はそのまま先頭に残り、増えたぶんだけが展開済みの形で末尾に付く
        assert_eq!(path, format!("{PROCESS};C:\\Users\\winuser\\.local\\bin"));
        assert!(contains_entry_in(&path, &launcher, &lookup));
        // 起動時の PATH からは引けない（= setup の「いまのタブ」の判定が false になる材料）
        assert!(!contains_entry_in(PROCESS, &launcher, &lookup));
    }

    #[test]
    fn 起動時にしか無い項目を落とさず順番も変えない() {
        // tako を起こした側が足した（レジストリに無い）項目が先頭と途中にある
        let process = "C:\\tako\\bin;C:\\WINDOWS\\system32;D:\\tools;C:\\WINDOWS";
        let reg = registry(
            Some("C:\\WINDOWS\\system32;C:\\WINDOWS"),
            Some("C:\\Users\\winuser\\.local\\bin"),
        );
        let path = pane_path_in(process, &reg, &lookup).expect("増えたぶんがある");
        assert_eq!(path, format!("{process};C:\\Users\\winuser\\.local\\bin"));
        let entries = split_entries(&path);
        assert_eq!(entries[0], "C:\\tako\\bin", "起動時の先頭はそのまま");
        assert_eq!(entries[2], "D:\\tools", "起動時の途中の項目もそのまま");
    }

    #[test]
    fn 重複は大小と区切りの違いも含めて足さない() {
        // 起動時の値とレジストリが同じ場所を別の書き方で持つだけなら、足すものは無い
        let reg = registry(
            Some("c:/windows/system32/;C:\\Windows\\;\"C:\\Program Files\\Git\\cmd\""),
            // 名前の部分はプレースホルダのまま（#927 の番犬は名前を大小込みで見る）
            Some("%USERPROFILE%\\AppData\\Roaming\\npm;C:\\USERS\\winuser\\APPDATA\\ROAMING\\NPM"),
        );
        assert_eq!(pane_path_in(PROCESS, &reg, &lookup), None);
        // レジストリの中の重複も 1 回だけ足す（マシンとユーザーに同じものがある）
        let reg = registry(Some("C:\\new"), Some("c:\\NEW\\;C:/new"));
        assert_eq!(
            pane_path_in("C:\\a", &reg, &lookup).as_deref(),
            Some("C:\\a;C:\\new")
        );
    }

    #[test]
    fn マシンが先でユーザーが後() {
        let reg = registry(Some("C:\\machine"), Some("C:\\user"));
        assert_eq!(
            pane_path_in("C:\\a", &reg, &lookup).as_deref(),
            Some("C:\\a;C:\\machine;C:\\user")
        );
    }

    #[test]
    fn レジストリに値が無いか読めないときは継承のまま() {
        // 両方 None（キーが無い・読み出しに失敗した）
        assert_eq!(
            pane_path_in(PROCESS, &RegistryPaths::default(), &lookup),
            None
        );
        // 空の値・区切りだけの値
        assert_eq!(
            pane_path_in(PROCESS, &registry(Some(""), Some(";;")), &lookup),
            None
        );
        // 片方だけ読めたら、読めたほうから足す
        assert_eq!(
            pane_path_in("C:\\a", &registry(None, Some("C:\\u")), &lookup).as_deref(),
            Some("C:\\a;C:\\u")
        );
    }

    #[test]
    fn 空の項目と末尾の区切りを扱う() {
        // 起動時の値の末尾の `;` は二重にしない。途中の空の項目は起動時の値の見た目のまま残す
        let reg = registry(None, Some(";;C:\\u1;; ;C:\\u2;"));
        assert_eq!(
            pane_path_in("C:\\a;;C:\\b;", &reg, &lookup).as_deref(),
            Some("C:\\a;;C:\\b;C:\\u1;C:\\u2")
        );
        // 起動時の PATH が空・区切りだけでも、先頭に `;` を作らない
        for process in ["", ";", " ; "] {
            assert_eq!(
                pane_path_in(process, &reg, &lookup).as_deref(),
                Some("C:\\u1;C:\\u2"),
                "process={process:?}"
            );
        }
    }

    #[test]
    fn 展開できない変数は記述のまま足す() {
        // Windows も未定義の変数は展開せずに残す（同じ振る舞い）
        let reg = registry(None, Some("%NOPE%\\bin"));
        assert_eq!(
            pane_path_in("C:\\a", &reg, &lookup).as_deref(),
            Some("C:\\a;%NOPE%\\bin")
        );
    }

    #[test]
    fn 極端に長いpathでも上限を超えずに起動時の値を削らない() {
        // 起動時の値が上限の手前まで埋まっている: 入らないエントリは足さず、入るものだけ足す
        let long = format!("C:\\{}", "x".repeat(MAX_VAR_LEN - 10));
        let reg = registry(None, Some("C:\\too-long-entry;C:\\ok"));
        let path = pane_path_in(&long, &reg, &lookup).expect("入るものは足す");
        assert_eq!(path, format!("{long};C:\\ok"));
        assert!(path.encode_utf16().count() <= MAX_VAR_LEN);
        // 1 つも入らなければ継承のまま（起動時の値は 1 文字も削らない）
        let full = format!("C:\\{}", "x".repeat(MAX_VAR_LEN - 3));
        assert_eq!(pane_path_in(&full, &reg, &lookup), None);
        // レジストリ側が極端に長くても、上限までで止める
        let many: Vec<String> = (0..5000).map(|i| format!("C:\\dir{i:04}")).collect();
        let reg = registry(None, Some(&many.join(";")));
        let path = pane_path_in("C:\\a", &reg, &lookup).expect("入るぶんは足す");
        assert!(path.encode_utf16().count() <= MAX_VAR_LEN);
        assert!(path.starts_with("C:\\a;C:\\dir0000;"));
    }

    #[test]
    fn 届く先の判定はペインのpathと同じ組み方で見る() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        // レジストリにだけある = このタブでは引けないが、新しいタブでは引ける
        let reg = registry(None, Some("%USERPROFILE%\\.local\\bin"));
        assert_eq!(
            reach_in(&launcher, PROCESS, &reg, &lookup),
            Reach {
                this_process: false,
                new_pane: true
            }
        );
        // 起動時の値にある = どちらでも引ける
        let process = format!("{PROCESS};C:\\Users\\winuser\\.local\\bin");
        assert_eq!(
            reach_in(&launcher, &process, &RegistryPaths::default(), &lookup),
            Reach {
                this_process: true,
                new_pane: true
            }
        );
        // どこにも無い・レジストリが読めない = どちらでも引けない
        assert_eq!(
            reach_in(&launcher, PROCESS, &RegistryPaths::default(), &lookup),
            Reach {
                this_process: false,
                new_pane: false
            }
        );
    }

    #[test]
    fn ペインのenvは呼び出し側のpathの明示を上書きしない() {
        use std::collections::HashMap;
        let refreshed = || Some(("Path".to_string(), "C:\\a;C:\\new".to_string()));
        // 明示が無ければ載る
        let mut env = HashMap::new();
        apply_pane_path(&mut env, refreshed);
        assert_eq!(env.get("Path").map(String::as_str), Some("C:\\a;C:\\new"));
        // 大小を問わず明示があれば触らず、組み立ても呼ばない
        for key in ["PATH", "Path", "path"] {
            let mut env = HashMap::from([(key.to_string(), "X:\\mine".to_string())]);
            apply_pane_path(&mut env, || panic!("明示があるのに組み立てを呼んだ"));
            assert_eq!(env.len(), 1);
            assert_eq!(env[key], "X:\\mine");
        }
        // 足すものが無ければ何も載せない（= 起動時の値を継ぐ）
        let mut env = HashMap::new();
        apply_pane_path(&mut env, || None);
        assert!(env.is_empty());
    }

    #[test]
    fn レジストリの文字列はnulの手前までを空白ごと保つ() {
        let units: Vec<u16> = " C:\\a;C:\\b \0garbage".encode_utf16().collect();
        assert_eq!(decode_registry_string(&units), " C:\\a;C:\\b ");
        let units: Vec<u16> = "C:\\a".encode_utf16().collect();
        assert_eq!(decode_registry_string(&units), "C:\\a");
        assert_eq!(decode_registry_string(&[]), "");
        assert_eq!(decode_registry_string(&[0]), "");
    }

    /// unix のペインはログインシェルとして profile を読み直すので、PATH には触らない
    #[test]
    fn unixでは新しいペインのpathを触らない() {
        if !cfg!(windows) {
            assert!(!rereads_for_new_panes());
            assert_eq!(pane_path(), None);
        }
    }

    /// unix では概念が無いことを明示する（profile 経路へ落ちる判断の根拠）
    #[test]
    fn unixでは非対応と答える() {
        assert_eq!(is_supported(), cfg!(windows));
        if !cfg!(windows) {
            assert!(read().is_err());
            assert!(effective().is_err());
            assert!(write(&UserPathValue::new("x")).is_err());
        }
    }
}

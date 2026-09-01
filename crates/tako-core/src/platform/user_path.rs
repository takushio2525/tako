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
pub fn append_entry(
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
    // 末尾の `;` は Windows の PATH では珍しくない。二重にしない
    let base = trimmed.trim_end_matches(SEP);
    Some(format!("{base}{SEP}{dir}"))
}

/// `dir` のエントリを取り除いた値。**入っていなければ `None`**
///
/// 区切りの見た目（末尾の `;` の有無）は元の値に合わせる
pub fn remove_entry(
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
        assert_eq!(append_entry(REAL_RAW, &launcher, &lookup), None);
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
        let raw = "C:\\WINDOWS\\system32;";
        let appended = append_entry(raw, &launcher, &lookup).expect("追記される");
        assert_eq!(
            appended,
            "C:\\WINDOWS\\system32;C:\\Users\\winuser\\.local\\bin"
        );
        // 2 回目は None（冪等）
        assert_eq!(append_entry(&appended, &launcher, &lookup), None);
        // 空の値でも壊れない
        assert_eq!(
            append_entry("", &launcher, &lookup).as_deref(),
            Some("C:\\Users\\winuser\\.local\\bin")
        );
    }

    #[test]
    fn 除去は元の見た目を保ちつつ該当エントリだけ落とす() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        let removed = remove_entry(REAL_RAW, &launcher, &lookup).expect("除去される");
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
        assert_eq!(remove_entry(&removed, &launcher, &lookup), None);
    }

    #[test]
    fn 追記と除去の往復で元へ戻る() {
        let launcher = dir("C:\\Users\\winuser\\.local\\bin");
        let base = "C:\\WINDOWS\\system32;C:\\Program Files\\Git\\cmd";
        let appended = append_entry(base, &launcher, &lookup).expect("追記される");
        let back = remove_entry(&appended, &launcher, &lookup).expect("除去される");
        assert_eq!(back, base, "往復で元のバイト列へ戻る");
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

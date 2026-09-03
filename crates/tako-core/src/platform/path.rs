//! `canonicalize` の結果を「他のプログラムへ渡せる形」へ寄せる（抽象境界 B26。#970）
//!
//! ## なぜ境界が要るか
//!
//! `Path::canonicalize` は Windows で **verbatim 形式**（`\\?\C:\Users\…`）を返す。
//! これは Win32 のパス正規化と `MAX_PATH` 制限を**無効にする入口指定**であって、
//! 他のプログラムへ渡したり画面に出したりする形ではない。tako がこれを持ち回ると:
//!
//! - シェル統合（`shell-integration/tako.ps1`）は `$loc.ProviderPath` を
//!   `\` → `/` に置換して `file:///` を付けるので、`\\?\C:\…` が
//!   `file:///` + `//?/C:/…` になる。tako が OSC 7 を解いたペインの cwd は
//!   **`///?/C:/Users/…`（実在しないパス）**へ壊れ、`git rev-parse --show-toplevel` を
//!   回す `Command::current_dir` が起動に失敗する → そのペインの git タブと
//!   `tako git *` が全滅する（#970 の Windows 11 実測）
//! - `tako list` の cwd・`tako recent list`・`pane_current_path`・ファイルツリーの
//!   ルート表示にも `\\?\` がそのまま出る
//!
//! **git 自身は verbatim を扱える**（`git -C \\?\C:\… rev-parse` は exit 0）。
//! 壊れているのは prefix そのものではなく **`\` を `/` へ潰した後の `///?/`** の形。
//! だから直す場所は「潰す側」ではなく **`canonicalize` の出口 1 箇所**:
//! prefix は cwd 以外（recent / ツリー / `pane_current_path`）にも漏れているので、
//! tako.ps1 側で剥がしても塞ぎきれない（あちらは防御として剥がす）。
//!
//! ## プラットフォームで分岐しない
//!
//! 判定は**文字列の形だけ**で決まる（`\\?\` で始まるか）。unix の `canonicalize` は
//! 必ず `/` 始まりの絶対パスを返すのでこの形にならず、恒等になる。`cfg` を書かないので
//! **macOS 上から Windows 形式の入力を通したテストが書ける**（#515 / #913 と同じ方針）。
//!
//! ## [`crate::file_uri`] との役割分担
//!
//! あちらは **URI → パス**（`file:///C:/…` の先頭 `/` を落とす。#913）。こちらは
//! **`canonicalize` の戻り → 渡せる形**。方向が逆なので混ぜない。
//!
//! ## 剥がさない場合がある（意味が変わるとき）
//!
//! verbatim を外すと Win32 のパス正規化が復活するので、**同じファイルを指さなくなる**
//! 形は verbatim のまま返す（[`strip_verbatim_str`] の判定表を参照）。この場合は
//! #970 の症状が残るが、**剥がして別の場所を指すより「今と同じ」ほうが安全**。

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// verbatim prefix（`\\?\`）
const VERBATIM: &str = r"\\?\";
/// verbatim UNC prefix（`\\?\UNC\server\share` = `\\server\share`）
const VERBATIM_UNC: &str = r"\\?\UNC\";

/// Win32 の `MAX_PATH`。終端 NUL を含む値なので、使えるパス長は 259 文字まで。
///
/// **数えるのは UTF-16 の単位**（Win32 のパス長はワイド文字数）。Rust の
/// `str::len()` はバイト数なので、そのまま使うと日本語のパスを過大に見積もり、
/// 剥がせるものを verbatim のまま残してしまう（`…\プロジェクト\…` は
/// 1 文字 = 3 バイト / 1 単位）
const MAX_PATH: usize = 260;

/// Win32 が数えるパス長（UTF-16 の単位数）
fn utf16_len(path: &str) -> usize {
    path.chars().map(char::len_utf16).sum()
}

/// Win32 が「デバイス」として解釈する予約名。`C:\dir\NUL` は**実ファイルではなく
/// NUL デバイス**を指すので、そういう成分を含むパスから verbatim を外すと別物になる
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// **パス解決の入口**。`canonicalize` してから verbatim prefix を落とす。
///
/// 保存する・子プロセスへ渡す・応答へ出すパスは**必ずここを通す**
/// （番犬テスト `canonicalizeの直呼びが境界の外に残っていない`）。
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?; // watchdog-allow: 境界の実装本体
    if legacy() {
        return Ok(canonical);
    }
    Ok(strip_verbatim(&canonical).into_owned())
}

/// [`canonicalize`] の「失敗したら入力をそのまま返す」版。
///
/// 比較キーを作る用途（ピン留めフォルダの重複判定など）で使う。存在しないパスや
/// 権限の無いパスでも比較が成立するので、呼び出し側に `unwrap_or_else` を散らさない
pub fn canonicalize_or_self(path: &Path) -> PathBuf {
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// verbatim prefix を落とす（[`strip_verbatim_str`] の `Path` 版）。
///
/// 非 UTF-8 のパス（Windows の不対サロゲート等）は触らない。
///
/// **`Cow::Borrowed` は「変えていない」を意味しない**: ドライブ形は入力の
/// 部分スライス（prefix を除いた残り）を借用で返すので、変わったかを知りたい
/// なら入力と比べること
pub fn strip_verbatim(path: &Path) -> Cow<'_, Path> {
    let Some(text) = path.to_str() else {
        return Cow::Borrowed(path);
    };
    match strip_verbatim_str(text) {
        Cow::Borrowed(simplified) => Cow::Borrowed(Path::new(simplified)),
        Cow::Owned(simplified) => Cow::Owned(PathBuf::from(simplified)),
    }
}

/// Windows の verbatim パスを、**同じ場所を指す**非 verbatim 形へ。
///
/// | 入力 | 出力 | 理由 |
/// |---|---|---|
/// | `\\?\C:\Users\x` | `C:\Users\x` | ドライブ形は等価な非 verbatim 形を持つ |
/// | `\\?\UNC\srv\share\x` | `\\srv\share\x` | UNC 形も同様 |
/// | `\\?\Volume{…}\x` | そのまま | ボリューム GUID 形に非 verbatim の等価形が無い |
/// | `\\.\PhysicalDrive0` | そのまま | デバイス名前空間（`\\?\` ではない） |
/// | 260 文字以上になるもの | そのまま | prefix 無しでは Win32 が受け付けない |
/// | `/` を含むもの | そのまま | verbatim では `/` は**普通の文字**。剥がすと区切りに化ける |
/// | `.` / `..` / 空の成分を含むもの | そのまま | Win32 の正規化が解決・圧縮して別の場所を指す |
/// | 末尾が `.` か空白の成分を含むもの | そのまま | Win32 の正規化がそれを削って別名になる |
/// | `NUL` などの予約デバイス名を含むもの | そのまま | 実ファイルではなくデバイスを指してしまう |
///
/// 剥がさない側に倒した判定は、**#970 の症状が残るほうがマシ**という判断
/// （剥がして別の場所を指すのは黙って壊れる。残るのは既知の不具合）。
pub fn strip_verbatim_str(path: &str) -> Cow<'_, str> {
    // UNC を先に見る（`\\?\` は `\\?\UNC\` の接頭辞なので順序が意味を持つ）
    if let Some(rest) = strip_prefix_ascii_case(path, VERBATIM_UNC) {
        // `\\` の 2 文字が増えるので、長さの検査にはそれを足して渡す
        if is_safe_to_simplify(rest, 2) {
            return Cow::Owned(format!(r"\\{rest}"));
        }
        return Cow::Borrowed(path);
    }
    if let Some(rest) = strip_prefix_ascii_case(path, VERBATIM) {
        if starts_with_drive(rest) && is_safe_to_simplify(rest, 0) {
            return Cow::Borrowed(rest);
        }
    }
    Cow::Borrowed(path)
}

/// 大小を無視した `strip_prefix`（`\\?\unc\` も受ける。`UNC` は Win32 が
/// 大小を区別しないので、`canonicalize` の戻り以外の入力でも取り落とさない）
fn strip_prefix_ascii_case<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path.len() < prefix.len() {
        return None;
    }
    let (head, rest) = path.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

/// `C:\…` の形か。**区切りまで要求する**のが要点: `\\?\C:` は C: の**ルート**だが
/// 素の `C:` は「ドライブ C の現在位置」なので、剥がすと意味が変わる
fn starts_with_drive(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// prefix を外しても同じ場所を指すか（判定表は [`strip_verbatim_str`]）
fn is_safe_to_simplify(rest: &str, extra_len: usize) -> bool {
    if rest.is_empty() || rest.contains('/') {
        return false;
    }
    if utf16_len(rest) + extra_len >= MAX_PATH {
        return false;
    }
    // 末尾の区切り 1 個は成分ではない（`C:\` を「空の成分つき」と読まない）
    let body = rest.strip_suffix('\\').unwrap_or(rest);
    if body.is_empty() {
        return false;
    }
    body.split('\\').all(is_safe_component)
}

fn is_safe_component(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    !RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// #970 の修正を入れる前へ戻す逃げ道（`TAKO_970_LEGACY=1`）。A/B 計測専用
fn legacy() -> bool {
    std::env::var_os("TAKO_970_LEGACY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    #[test]
    fn ドライブ形のverbatimは剥がす() {
        assert_eq!(
            strip_verbatim_str(r"\\?\C:\Users\testuser"),
            r"C:\Users\testuser"
        );
        // ドライブのルート（末尾区切りは残す）
        assert_eq!(strip_verbatim_str(r"\\?\C:\"), r"C:\");
        // 小文字ドライブ
        assert_eq!(strip_verbatim_str(r"\\?\d:\dev\repo"), r"d:\dev\repo");
        // 末尾区切りつきの通常パス
        assert_eq!(strip_verbatim_str(r"\\?\C:\dev\repo\"), r"C:\dev\repo\");
    }

    #[test]
    fn unc形のverbatimは素のuncへ() {
        assert_eq!(
            strip_verbatim_str(r"\\?\UNC\server\share\dir"),
            r"\\server\share\dir"
        );
        // Win32 は `UNC` の大小を区別しない
        assert_eq!(
            strip_verbatim_str(r"\\?\unc\server\share"),
            r"\\server\share"
        );
    }

    #[test]
    fn verbatimでないものは触らない() {
        assert_eq!(
            strip_verbatim_str(r"C:\Users\testuser"),
            r"C:\Users\testuser"
        );
        assert_eq!(strip_verbatim_str("/Users/me/dev"), "/Users/me/dev");
        assert_eq!(strip_verbatim_str(r"\\server\share"), r"\\server\share");
        assert_eq!(strip_verbatim_str(""), "");
        // デバイス名前空間（`\\.\`）は `\\?\` ではない
        assert_eq!(
            strip_verbatim_str(r"\\.\PhysicalDrive0"),
            r"\\.\PhysicalDrive0"
        );
    }

    #[test]
    fn 非verbatimの等価形を持たない形は剥がさない() {
        // ボリューム GUID
        let guid = r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\dir";
        assert_eq!(strip_verbatim_str(guid), guid);
        // ドライブに区切りが続かない（`\\?\C:` はルート / 素の `C:` は相対）
        assert_eq!(strip_verbatim_str(r"\\?\C:"), r"\\?\C:");
        // prefix だけ
        assert_eq!(strip_verbatim_str(r"\\?\"), r"\\?\");
    }

    #[test]
    fn 正規化で意味が変わる成分があれば剥がさない() {
        // verbatim では `/` は普通の文字。剥がすと区切りへ化ける
        let slash = r"\\?\C:\dir\a/b";
        assert_eq!(strip_verbatim_str(slash), slash);
        // `.` / `..` は Win32 が解決してしまう
        let dots = r"\\?\C:\dir\..\other";
        assert_eq!(strip_verbatim_str(dots), dots);
        let dot = r"\\?\C:\dir\.\other";
        assert_eq!(strip_verbatim_str(dot), dot);
        // 連続区切りは圧縮される
        let empty = r"\\?\C:\dir\\other";
        assert_eq!(strip_verbatim_str(empty), empty);
        // 末尾の `.` / 空白は削られて別名になる
        let trailing_dot = r"\\?\C:\dir.\file";
        assert_eq!(strip_verbatim_str(trailing_dot), trailing_dot);
        let trailing_space = r"\\?\C:\dir \file";
        assert_eq!(strip_verbatim_str(trailing_space), trailing_space);
        // 予約デバイス名（拡張子つきも同じ扱い）
        let nul = r"\\?\C:\dir\NUL";
        assert_eq!(strip_verbatim_str(nul), nul);
        let nul_ext = r"\\?\C:\dir\nul.txt";
        assert_eq!(strip_verbatim_str(nul_ext), nul_ext);
        let com = r"\\?\C:\dir\COM1\file";
        assert_eq!(strip_verbatim_str(com), com);
    }

    #[test]
    fn 予約名に見えて違うものは剥がす() {
        // 予約名は完全一致（拡張子の前まで）でのみ判定する
        assert_eq!(
            strip_verbatim_str(r"\\?\C:\dir\NULL\console.txt"),
            r"C:\dir\NULL\console.txt"
        );
        assert_eq!(strip_verbatim_str(r"\\?\C:\dir\COM10"), r"C:\dir\COM10");
    }

    #[test]
    fn max_pathを超えるものは剥がさない() {
        // prefix を外すと Win32 が受け付けない長さ
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(strip_verbatim_str(&long), long);
        // 境界: 剥がした結果が 259 文字なら剥がす / 260 文字なら剥がさない
        let head = r"C:\";
        let ok = format!(r"\\?\{head}{}", "a".repeat(259 - head.len()));
        assert_eq!(strip_verbatim_str(&ok).len(), 259);
        let ng = format!(r"\\?\{head}{}", "a".repeat(260 - head.len()));
        assert_eq!(strip_verbatim_str(&ng), ng);
        // UNC は `\\` の 2 文字ぶんを足して数える
        let unc_body = format!(
            r"server\share\{}",
            "a".repeat(259 - r"\\server\share\".len())
        );
        let unc_ok = format!(r"\\?\UNC\{unc_body}");
        assert_eq!(strip_verbatim_str(&unc_ok).len(), 259);
        let unc_ng = format!(r"\\?\UNC\{unc_body}a");
        assert_eq!(strip_verbatim_str(&unc_ng), unc_ng);
    }

    #[test]
    fn 長さはutf16の単位で数える() {
        // 日本語のパスはバイト数で数えると 3 倍に見える。バイトで判定すると
        // 「剥がせるのに verbatim のまま」= #970 が日本語のフォルダで直らない
        let head = r"C:\Users\testuser\";
        // 87 文字（UTF-16 単位）× … で 259 単位ちょうどに収まる形
        let body = "プ".repeat(259 - utf16_len(head));
        let simplified = format!("{head}{body}");
        assert_eq!(utf16_len(&simplified), 259);
        assert!(
            simplified.len() > MAX_PATH,
            "バイト数では 260 を超えている前提のテスト（len={}）",
            simplified.len()
        );
        assert_eq!(
            strip_verbatim_str(&format!(r"\\?\{simplified}")),
            simplified,
            "UTF-16 で 259 単位なら剥がす"
        );
        // 1 単位増えると剥がさない
        let over = format!("{simplified}プ");
        let verbatim = format!(r"\\?\{over}");
        assert_eq!(strip_verbatim_str(&verbatim), verbatim);
    }

    #[test]
    fn 剥がしても同じ場所を指す() {
        // 判定が「意味を変えない」ことの確認: 成分列が prefix ぶんだけ違う
        let verbatim = Path::new(r"\\?\C:\Users\testuser\dev");
        let simplified = strip_verbatim(verbatim);
        assert_eq!(simplified.as_ref(), Path::new(r"C:\Users\testuser\dev"));
    }

    #[test]
    fn 触らない入力は借用のまま返る() {
        // 無駄な確保をしない（recent / ツリーの表示経路で毎回通る）
        assert!(matches!(
            strip_verbatim_str("/Users/me/dev"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            strip_verbatim_str(r"\\?\C:\dev"),
            Cow::Borrowed(_)
        ));
    }

    /// `TAKO_970_LEGACY` はプロセス全体のグローバルなので、触るテストは直列化する
    /// （#608 / #807 / #1042 と同じ形）
    static LEGACY_ENV: Mutex<()> = Mutex::new(());

    fn legacy_guard() -> MutexGuard<'static, ()> {
        LEGACY_ENV.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct LegacyEnv;

    impl LegacyEnv {
        fn set() -> Self {
            // SAFETY: 呼び出し側が `legacy_guard()` で直列化している
            unsafe { std::env::set_var("TAKO_970_LEGACY", "1") };
            Self
        }
    }

    impl Drop for LegacyEnv {
        fn drop(&mut self) {
            // SAFETY: 同上
            unsafe { std::env::remove_var("TAKO_970_LEGACY") };
        }
    }

    #[test]
    fn 実パスの解決は入口を通っても同じ場所を指す() {
        let _guard = legacy_guard();
        let dir = std::env::temp_dir();
        let via_boundary = canonicalize(&dir).expect("temp_dir は解決できる");
        let raw = dir.canonicalize().expect("temp_dir は解決できる"); // watchdog-allow: 対照
                                                                      // unix は恒等。Windows は prefix だけが落ちる（どちらも同じ場所）
        assert!(
            via_boundary.is_dir(),
            "解決したパスが使える: {via_boundary:?}"
        );
        assert_eq!(
            via_boundary,
            strip_verbatim(&raw).into_owned(),
            "入口は canonicalize + strip_verbatim と一致する"
        );
    }

    #[test]
    fn legacy_envで剥がさない旧挙動へ戻る() {
        let _guard = legacy_guard();
        let dir = std::env::temp_dir();
        let raw = dir.canonicalize().expect("temp_dir は解決できる"); // watchdog-allow: 対照
        let _legacy = LegacyEnv::set();
        assert_eq!(
            canonicalize(&dir).expect("temp_dir は解決できる"),
            raw,
            "TAKO_970_LEGACY=1 では canonicalize の戻りをそのまま返す"
        );
    }

    #[test]
    fn 解決できないパスは入力をそのまま返す() {
        let missing = Path::new("/tako-970-does-not-exist/nope");
        assert!(canonicalize(missing).is_err());
        assert_eq!(canonicalize_or_self(missing), missing);
    }
}

//! 8.3 の短い形（`C:\Users\<利用者名の頭>~1\…`）の成分を長い形へ戻す（抽象境界 B34。#2843）
//!
//! ## なぜ境界が要るか
//!
//! Windows は同じフォルダに長い名前と 8.3 の短い名前を持たせる（ボリュームで無効にもできる）。
//! 利用者名に空白・日本語・9 文字以上を含む PC では `TEMP` が `C:\Users\<利用者名の頭>~1\AppData\Local\Temp`
//! のような短い形になり、そこから作ったパスや、利用者が短い形で書いたパスが作業 dir として tako に届く。
//!
//! claude は `.claude.json` の `projects` の鍵を**長い形**の作業 dir で引く（#2843 の Windows CI の実測。
//! 短い形の cwd から起こしても、信頼の確認を通したときに claude が書く鍵は長い形）。tako が短い形のまま
//! 事前信頼の鍵を書くと信頼済みと読まれず、信頼の確認が出て、確認が通るまで tako mod も読まれない。
//! だから鍵を作る前にここで長い形へ戻す（呼び手は `tako-control::claude_tui::claude_project_key`）。
//!
//! ## 戻さない場合（入力をそのまま返す）
//!
//! | 入力 | 理由 |
//! |---|---|
//! | `~` を含まない | 自動で作られる 8.3 の名前は必ず `~` を含む（OS へ問い合わせない） |
//! | 存在しない・読めない | OS が長い形を返さない（`GetLongPathNameW` が 0） |
//! | 8.3 の名前が無いボリューム・`~` が長い名前の一部 | OS が同じ形を返す |
//! | unix | 8.3 の名前が無い（OS へ問い合わせない） |
//!
//! 戻すのは成分の名前だけで、symlink / junction は辿らない（`canonicalize` と違い指す場所の表記を変えない）。
//! 判断は純関数 [`choose_long_form`] にしてあるので、**macOS からも Windows 側の分岐を検査できる**
//! （#881 の [`super::program_path`] と同じ方針）。A/B は `TAKO_2843_LEGACY=1`（戻さない = 修正前）。

use std::borrow::Cow;

/// #2843 の A/B（同一バイナリで修正前 = 短い形をそのまま返す）
pub const LEGACY_ENV: &str = "TAKO_2843_LEGACY";

/// 短い形の成分を長い形へ戻す。戻せないとき・戻す必要が無いときは入力をそのまま返す。
///
/// 戻したときは OS が返した形（区切りは `\`。verbatim の前置き `\\?\` は入力にあれば残る）を返す
pub fn long_form(path: &str) -> Cow<'_, str> {
    if legacy() || !may_have_short_component(path) {
        return Cow::Borrowed(path);
    }
    let asked = os_input(path);
    let answer = imp::long_path(&asked);
    choose_long_form(path, &asked, answer.as_deref())
}

/// 短い形の成分を持ちうるか（自動で作られる 8.3 の名前は必ず `~` を含む）
pub(crate) fn may_have_short_component(path: &str) -> bool {
    path.contains('~')
}

/// OS へ渡す形。verbatim でなければ `/` を `\` にそろえる（Win32 の正規化では同じ区切り。
/// verbatim の中の `/` は普通の文字なので触らない = [`super::path::strip_verbatim_str`] の判定表と同じ理由）
fn os_input(path: &str) -> Cow<'_, str> {
    if path.starts_with(r"\\?\") || !path.contains('/') {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(path.replace('/', "\\"))
    }
}

/// 戻すかの判断（純関数。`asked` は OS へ渡した形、`answer` は OS が返した長い形）。
/// 答えが無い・空・渡した形と同じなら入力をそのまま返す（区切りだけ違う形も「変えていない」と読む）
pub(crate) fn choose_long_form<'a>(
    path: &'a str,
    asked: &str,
    answer: Option<&str>,
) -> Cow<'a, str> {
    match answer {
        Some(long) if !long.is_empty() && long != asked => Cow::Owned(long.to_string()),
        _ => Cow::Borrowed(path),
    }
}

fn legacy() -> bool {
    std::env::var_os(LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

#[cfg(not(windows))]
mod imp {
    /// unix に 8.3 の名前は無い
    pub(super) fn long_path(_path: &str) -> Option<String> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    unsafe extern "system" {
        fn GetLongPathNameW(
            lpszShortPath: *const u16,
            lpszLongPath: *mut u16,
            cchBuffer: u32,
        ) -> u32;
    }

    /// 長い形。取れなければ `None`（存在しない・読めないパスは 0 が返る）
    pub(super) fn long_path(path: &str) -> Option<String> {
        let wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // 1 回目は長さの問い合わせ（返り値は終端 NUL を含む文字数）
        let len = unsafe { GetLongPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        // 2 回目の返り値は終端 NUL を含まない文字数（足りなければ必要な長さ = 間に変わった）
        let written = unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
        if written == 0 || written >= len {
            return None;
        }
        buf.truncate(written as usize);
        std::ffi::OsString::from_wide(&buf).into_string().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn チルダを含まないパスはosへ問い合わせずそのまま() {
        for p in [
            r"C:\work\long folder\proj",
            "C:/Users/winuser/work",
            r"\\?\C:\work\long folder\proj",
            "/home/testuser/work",
            "",
        ] {
            assert!(!may_have_short_component(p), "{p}");
            assert!(matches!(long_form(p), Cow::Borrowed(q) if q == p), "{p}");
        }
    }

    #[test]
    fn 長い形が返れば置き換える() {
        let short = r"C:\work\LONGFO~1\proj";
        assert_eq!(
            choose_long_form(short, short, Some(r"C:\work\long folder\proj")),
            r"C:\work\long folder\proj"
        );
        // `/` 区切りの入力は `\` にそろえて問い合わせ、戻した形は OS の形（`\`）
        let slash = "C:/work/LONGFO~1/proj";
        assert_eq!(os_input(slash), short);
        assert_eq!(
            choose_long_form(slash, &os_input(slash), Some(r"C:\work\long folder\proj")),
            r"C:\work\long folder\proj"
        );
    }

    #[test]
    fn 答えが無い空か同じなら入力のまま() {
        let short = r"C:\work\LONGFO~1\proj";
        // 存在しない・読めない（GetLongPathNameW が 0）
        assert!(matches!(choose_long_form(short, short, None), Cow::Borrowed(p) if p == short));
        assert!(matches!(choose_long_form(short, short, Some("")), Cow::Borrowed(p) if p == short));
        // 8.3 の名前が無いボリューム・`~` が長い名前の一部（OS が同じ形を返す）
        assert!(
            matches!(choose_long_form(short, short, Some(short)), Cow::Borrowed(p) if p == short)
        );
        // 区切りだけ違う答えも「変えていない」（入力の区切りを保つ）
        let slash = "C:/Users/a~b/work";
        assert!(matches!(
            choose_long_form(slash, &os_input(slash), Some(r"C:\Users\a~b\work")),
            Cow::Borrowed(p) if p == slash
        ));
    }

    #[test]
    fn verbatimの中のスラッシュは区切りにしない() {
        // verbatim では `/` は普通の文字。そろえると別の名前を問い合わせてしまう
        let p = r"\\?\C:\work\LONGFO~1\a/b";
        assert_eq!(os_input(p), p);
        assert_eq!(os_input(r"C:\work\LONGFO~1\proj"), r"C:\work\LONGFO~1\proj");
    }

    #[cfg(not(windows))]
    #[test]
    fn unixではチルダがあってもそのまま() {
        for p in ["/home/testuser/a~1/work", "~/work"] {
            assert!(matches!(long_form(p), Cow::Borrowed(q) if q == p), "{p}");
        }
    }

    /// 実機（Windows）の 8.3 の短い形。取れなければ `None`（このボリュームで 8.3 の名前が作られない）
    #[cfg(windows)]
    fn short_of(path: &std::path::Path) -> Option<String> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        unsafe extern "system" {
            fn GetShortPathNameW(
                lpszLongPath: *const u16,
                lpszShortPath: *mut u16,
                cchBuffer: u32,
            ) -> u32;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let len = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
        if written == 0 || written >= len {
            return None;
        }
        buf.truncate(written as usize);
        std::ffi::OsString::from_wide(&buf).into_string().ok()
    }

    /// 空白と日本語を含む置き場（8.3 の名前が作られるボリュームなら短い形を持つ）
    #[cfg(windows)]
    fn odd_dir(scratch: &crate::test_residue::ScratchDir) -> String {
        let dir = scratch.path().join("tako 2843 データ").join("work dir");
        std::fs::create_dir_all(&dir).unwrap();
        crate::platform::path::canonicalize(&dir)
            .unwrap()
            .display()
            .to_string()
    }

    #[cfg(windows)]
    #[test]
    fn 実機では短い形が長い形へ戻る() {
        let scratch = crate::test_residue::ScratchDir::new("issue2843");
        let long = odd_dir(&scratch);
        let Some(short) = short_of(std::path::Path::new(&long)) else {
            panic!("GetShortPathNameW が答えない: {long}");
        };
        println!(
            "TAKO2843_UNIT short_differs={} short_has_tilde={}",
            short != long,
            short.contains('~')
        );
        if short == long {
            // このボリュームで 8.3 の名前が作られない = 短い形の入力が作れない（戻す対象が無い）
            assert_eq!(long_form(&long), long);
            return;
        }
        assert_eq!(long_form(&short), long, "短い形 {short}");
        // `/` 区切りの短い形も同じ長い形（区切りは OS の `\`）
        assert_eq!(long_form(&short.replace('\\', "/")), long);
        // verbatim の短い形は前置きを残して戻る
        assert_eq!(long_form(&format!(r"\\?\{short}")), format!(r"\\?\{long}"));
        // 長い形はそのまま（#2834 の鍵の元が変わらない）
        assert!(matches!(long_form(&long), Cow::Borrowed(p) if p == long));
    }

    #[cfg(windows)]
    #[test]
    fn 実機でも存在しないパスとチルダを含む長い名前はそのまま() {
        let scratch = crate::test_residue::ScratchDir::new("issue2843");
        let base = crate::platform::path::canonicalize(scratch.path())
            .unwrap()
            .display()
            .to_string();
        // 存在しない（GetLongPathNameW が 0）
        let missing = format!(r"{base}\NOEXIS~1\work");
        assert!(matches!(long_form(&missing), Cow::Borrowed(p) if p == missing));
        // `~` が長い名前の一部（8.3 の名前が無いボリュームと同じく OS が同じ形を返す）
        let literal = std::path::Path::new(&base).join("a~1 データ");
        std::fs::create_dir_all(&literal).unwrap();
        let literal = literal.display().to_string();
        assert_eq!(long_form(&literal), literal);
        // 空白と日本語を含む長い形はそのまま
        let long = odd_dir(&scratch);
        assert_eq!(long_form(&long), long);
    }
}

//! プログラムパスを「空白を含まない 1 語」で書けるようにする（抽象境界 B18。#881）
//!
//! ## なぜ境界が要るか
//!
//! 永続バックエンド psmux（Windows の器）は `new-session` の内側コマンドを
//! **1 本の文字列**で受け取り、POSIX 風に単語分割してから引用符を落として
//! `CreateProcess` へ渡す。ところが **第 1 語（プログラム）は引用符で括れない**ため、
//! `C:\Program Files\...\pwsh.exe` のような空白入りのパスをそのままでは表現できない。
//!
//! 従来はその場合だけ `cmd.exe /c '<Windows 形式のコマンドライン>'` に包んでいたが、
//! **実測（2026-08-21・psmux 3.3.7）でこの包みは動かない**。psmux が単語分割の
//! 過程で引用符を落とすので、包んだ中の `"C:\Program Files\..."` の引用符も一緒に
//! 消え、`cmd.exe` が `'C:\Program' は…認識されていません` で即死する
//! （`/s` 付き・引用符二重化・第 1 語を `"` / `'` で括る、のどれも同じ結末）。
//!
//! ## 逃げ道は「そもそも空白を含まない表記にする」こと
//!
//! Windows は同じファイルに **8.3 短縮名**（`C:\PROGRA~1\POWERS~1\7\pwsh.exe`）を
//! 持っているので、これを第 1 語にすれば引用符が要らない。実測でも器の中で生存する。
//! 8.3 名はボリューム単位で無効化できる（`fsutil 8dot3name`）ため、取れなかったときは
//! **実行ファイル名だけ**にして PATH 探索へ賭ける（`powershell.exe` のように
//! System32 に在るものはこれで解決する）。
//!
//! 判断そのものは純粋関数 [`choose_single_token`] にしてあるので、
//! **macOS からも Windows 側の分岐を全部テストできる**。

/// 空白を含まない 1 語で書けるプログラム表記へ落とす。
///
/// 空白が無ければ**何も変えない**（フルパスの正確さを捨てない）。
/// 空白があるときだけ 8.3 短縮名 → 実行ファイル名 の順で 1 語へ寄せる
pub fn single_token(program: &str) -> String {
    if !has_space(program) {
        return program.to_string();
    }
    let short = imp::short_path(program);
    choose_single_token(program, short.as_deref())
}

/// 1 語化の判断（純粋関数。`short` は OS が返した 8.3 短縮名。**macOS でも全分岐テスト可**）
pub(crate) fn choose_single_token(program: &str, short: Option<&str>) -> String {
    if !has_space(program) {
        return program.to_string();
    }
    // 8.3 名が取れて空白が消えていればそれが最良（実体を取り違えない）
    if let Some(s) = short.filter(|s| !s.is_empty() && !has_space(s)) {
        return s.to_string();
    }
    // 取れないボリュームでは実行ファイル名だけにして PATH 探索へ賭ける。
    // ここでも空白が残る（`my prog.exe`）ことはあり得るが、呼び出し側が警告を出す
    program
        .rsplit(['\\', '/'])
        .next()
        .filter(|f| !f.is_empty())
        .unwrap_or(program)
        .to_string()
}

/// 器の内側コマンドの第 1 語として書けるか（空白・引用符を含まない）
pub fn is_single_token(program: &str) -> bool {
    !program.is_empty()
        && !program
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
}

fn has_space(s: &str) -> bool {
    s.chars().any(char::is_whitespace)
}

#[cfg(not(windows))]
mod imp {
    /// unix に 8.3 短縮名は無い。空白入りのパスは実行ファイル名へ落ちる
    pub(super) fn short_path(_program: &str) -> Option<String> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    unsafe extern "system" {
        fn GetShortPathNameW(
            lpszLongPath: *const u16,
            lpszShortPath: *mut u16,
            cchBuffer: u32,
        ) -> u32;
    }

    /// 8.3 短縮名。取れなければ `None`（ボリュームで無効化されていると
    /// **長いパスがそのまま返る**ので、呼び出し側が「空白が消えたか」で採否を決める）
    pub(super) fn short_path(program: &str) -> Option<String> {
        let wide: Vec<u16> = std::ffi::OsStr::new(program)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // 1 回目は長さの問い合わせ（返り値は終端 NUL を含まない文字数）
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
        Some(
            std::ffi::OsString::from_wide(&buf)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PWSH: &str = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";
    const PWSH_83: &str = "C:\\PROGRA~1\\POWERS~1\\7\\pwsh.exe";

    #[test]
    fn 空白が無ければ何も変えない() {
        // フルパスの正確さを捨てない（実体の取り違えを作らない）
        for p in [
            "pwsh.exe",
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            "/bin/sh",
        ] {
            assert_eq!(single_token(p), p);
            assert_eq!(choose_single_token(p, Some("使わない")), p);
        }
    }

    #[test]
    fn 空白があれば83短縮名を使う() {
        assert_eq!(choose_single_token(PWSH, Some(PWSH_83)), PWSH_83);
    }

    #[test]
    fn 短縮名が取れないときは実行ファイル名へ落ちる() {
        // 8.3 が無効なボリュームでは GetShortPathNameW が長いパスをそのまま返す
        assert_eq!(choose_single_token(PWSH, Some(PWSH)), "pwsh.exe");
        assert_eq!(choose_single_token(PWSH, None), "pwsh.exe");
        assert_eq!(choose_single_token(PWSH, Some("")), "pwsh.exe");
        // POSIX 区切りでも同じ
        assert_eq!(choose_single_token("/opt/my app/bin/foo", None), "foo");
    }

    #[test]
    fn 実行ファイル名にも空白が残ることはある() {
        // ここは器へ渡しても失敗する。呼び出し側が警告を出す責務（黙って壊さない）
        assert_eq!(
            choose_single_token("C:\\dir\\my prog.exe", None),
            "my prog.exe"
        );
        assert!(!is_single_token("my prog.exe"));
    }

    #[test]
    fn 末尾が区切りのような壊れた入力でもpanicしない() {
        assert_eq!(choose_single_token("C:\\a b\\", None), "C:\\a b\\");
        assert_eq!(choose_single_token("", None), "");
    }

    #[test]
    fn 単一語の判定は空白と引用符を弾く() {
        assert!(is_single_token("pwsh.exe"));
        assert!(is_single_token(PWSH_83));
        assert!(!is_single_token(PWSH));
        assert!(!is_single_token("'quoted'"));
        assert!(!is_single_token("\"quoted\""));
        assert!(!is_single_token(""));
    }

    #[cfg(windows)]
    #[test]
    fn 実機では8_3短縮名が引ける環境なら空白が消える() {
        // 8.3 が無効なボリュームでは実行ファイル名へ落ちる。どちらでも
        // 「1 語になっている」ことだけを見る（環境で答えが変わる部分は固定しない）
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let ps = format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        assert!(is_single_token(&single_token(&ps)), "{ps}");
        let with_space = format!("{system_root}\\a b\\c.exe");
        // 実在しないパスでも panic せず落とし先が決まる
        let _ = single_token(&with_space);
    }
}

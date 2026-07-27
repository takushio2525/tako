//! OS の優先表示言語（抽象境界 B10）
//!
//! ## なぜ境界が要るか（#604）
//!
//! 設定 `language: "system"`（**既定値**）の解決に使う。GUI プロセスは POSIX の
//! ロケール環境変数（`LANG` / `LC_ALL` 等）を持たないため、OS へ直接問い合わせるしかない。
//!
//! 移設前は macOS 経路だけが `i18n.rs` に `#[cfg(target_os = "macos")]` で直書きされており、
//! **Windows には問い合わせ経路そのものが無かった**。結果、表示言語が日本語の Windows でも
//! 既定値のまま UI が英語になっていた（同じ設定値の初回起動体験がプラットフォーム間で食い違う）。
//!
//! ## Windows で「表示言語」と「地域形式」を取り違えないこと
//!
//! Windows は UI の表示言語（設定 > 時刻と言語 > 言語）と、数値・日付の書式ロケール
//! （地域形式）を**独立に**設定できる。取得 API も別で、
//! `GetUserDefaultLocaleName` が返すのは後者の書式ロケールなので、
//! 「表示言語 = 英語 / 地域形式 = 日本語」のユーザーを日本語 UI と誤判定してしまう（逆も同様）。
//!
//! ここでは表示言語を返す `GetUserPreferredUILanguages` を使う。順序付きリストを返す点も
//! macOS の `AppleLanguages` と意味論が揃う（#604 の API 選択表）。

/// OS の優先表示言語を優先順に返す（先頭が最優先）。判定不能なら空。
///
/// 返り値は OS がそのまま返す文字列（`ja-JP` / `ja_JP` 等）で、**言語の判定はしない**
/// （それは `i18n::lang_from_locale` の仕事。ここは取得だけに責任を持つ）。
///
/// macOS 実装は `defaults` の起動を伴うため、呼び出しは起動時と明示的な言語切替時に限る。
pub fn system_languages() -> Vec<String> {
    imp::system_languages()
}

#[cfg(target_os = "macos")]
mod imp {
    /// `AppleLanguages`（優先順の配列）→ 取れなければ `AppleLocale`。
    ///
    /// 移設前の `i18n::macos_preferred_language` と**取得元も優先順も同じ**。
    /// 唯一の違いは先頭 1 件ではなく配列全体を返すことだが、呼び出し側は
    /// 先頭から順に判定して最初に決まったものを採るので判定結果は変わらない
    /// （移設前も「先頭要素」で決めていた）。
    pub(super) fn system_languages() -> Vec<String> {
        if let Some(raw) = defaults_read("AppleLanguages") {
            let langs = super::parse_plist_string_array(&raw);
            if !langs.is_empty() {
                return langs;
            }
        }
        defaults_read("AppleLocale")
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn defaults_read(key: &str) -> Option<String> {
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", key])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(windows)]
mod imp {
    /// `MUI_LANGUAGE_NAME`（winnls.h）。BCP-47 名（`ja-JP`）で受け取る。
    /// `MUI_LANGUAGE_ID`（0x4）にすると 16 進 LANGID（`0411`）が返り、
    /// ロケール文字列としては解釈できなくなる
    const MUI_LANGUAGE_NAME: u32 = 0x8;

    // `windows-sys` を足さず必要な 1 関数だけ宣言する方針は
    // `platform::ime` の IMM32 / `tako-control::agents` の Toolhelp32 と同じ
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserPreferredUILanguages(
            dwFlags: u32,
            pulNumLanguages: *mut u32,
            pwszLanguagesBuffer: *mut u16,
            pcchLanguagesBuffer: *mut u32,
        ) -> i32;
    }

    /// ユーザーの優先**表示言語**を優先順で取得する。
    ///
    /// 2 段階呼び出し（1 回目 = 必要な長さの問い合わせ、2 回目 = 実取得）。
    /// 結果は NUL 区切り + 末尾二重 NUL のマルチ文字列で返る
    pub(super) fn system_languages() -> Vec<String> {
        let mut count: u32 = 0;
        let mut chars: u32 = 0;
        // SAFETY: 出力ポインタはいずれもスタック上の生存する変数を指す。
        // バッファに null を渡すのは「長さだけ問い合わせる」規定の呼び出し方
        let ok = unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                std::ptr::null_mut(),
                &mut chars,
            )
        };
        if ok == 0 || chars == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; chars as usize];
        // SAFETY: buf は直前に chars 要素で確保済みで、`chars` にその長さを入れて渡している
        // （この API は入力として受け取ったバッファ長を超えて書き込まない）
        let ok = unsafe {
            GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut chars)
        };
        if ok == 0 {
            return Vec::new();
        }
        super::parse_multi_string(&buf)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    /// 問い合わせ先を持たないプラットフォーム（Linux 等）。
    /// POSIX のロケール環境変数は呼び出し側（`i18n::detect_os_lang`）が先に見ているので、
    /// ここが空を返しても環境変数による判定は効く
    pub(super) fn system_languages() -> Vec<String> {
        Vec::new()
    }
}

/// `defaults read -g AppleLanguages` が吐く plist 配列表記から文字列要素を取り出す純粋関数。
///
/// 表記は `(\n    "ja-JP",\n    "en-JP"\n)`。引用符で囲まれた要素だけを拾うので、
/// 引用符が無い形（`defaults` の出力形式が変わった場合）では空を返し、
/// 呼び出し側が `AppleLocale` へフォールバックする（移設前と同じ挙動）。
///
/// **Windows 上でもテストできる**ように `cfg` の外へ出してある（`platform::exe` と同じ方針）
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_plist_string_array(raw: &str) -> Vec<String> {
    raw.split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Windows の `PZZWSTR`（NUL 区切り + 末尾二重 NUL のマルチ文字列）を分解する純粋関数。
///
/// 空要素は終端・余白なので読み飛ばす。**macOS 上でもテストできる**ように `cfg` の外に置く
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_multi_string(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist配列から優先順のまま言語を取り出す() {
        let raw = "(\n    \"ja-JP\",\n    \"en-JP\"\n)";
        assert_eq!(parse_plist_string_array(raw), vec!["ja-JP", "en-JP"]);
    }

    #[test]
    fn plist配列の要素が1件でも取り出せる() {
        assert_eq!(
            parse_plist_string_array("(\n    \"en-US\"\n)"),
            vec!["en-US"]
        );
    }

    #[test]
    fn 引用符が無い出力は空にしてapplelocaleへ譲る() {
        // 移設前も先頭要素を取り出せず AppleLocale へフォールバックしていた
        assert!(parse_plist_string_array("(\n    ja-JP\n)").is_empty());
        assert!(parse_plist_string_array("").is_empty());
    }

    #[test]
    fn マルチ文字列を優先順のまま分解する() {
        // "ja-JP\0en-US\0\0"
        let mut buf: Vec<u16> = "ja-JP".encode_utf16().collect();
        buf.push(0);
        buf.extend("en-US".encode_utf16());
        buf.push(0);
        buf.push(0);
        assert_eq!(parse_multi_string(&buf), vec!["ja-JP", "en-US"]);
    }

    #[test]
    fn マルチ文字列の余分な終端は無視する() {
        assert_eq!(parse_multi_string(&[0, 0, 0]), Vec::<String>::new());
        assert_eq!(parse_multi_string(&[]), Vec::<String>::new());
    }

    /// 受け入れ条件 3（#604）の回帰: **書式ロケールの API を使わない**ことを固定する。
    ///
    /// 「表示言語 = 英語 / 地域形式 = 日本語」に設定した実機でしか実挙動は観測できないため、
    /// ここでは取り違えの起点である API 選択そのものを固定する（#604 の論点）。
    /// 冒頭の doc コメントは禁止 API の名前を挙げて説明しているので、
    /// 宣言・呼び出しの形だけを禁じ、テストモジュール自身は走査対象から外す
    #[test]
    fn 書式ロケールのapiを使っていない() {
        let source = include_str!("locale.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        for forbidden in [
            "fn GetUserDefaultLocaleName",
            "GetUserDefaultLocaleName(",
            "fn GetLocaleInfoEx",
            "GetLocaleInfoEx(",
        ] {
            assert!(
                !production.contains(forbidden),
                "書式ロケールの API（{forbidden}）は表示言語の取得には使えない。\
                 「表示言語 = 英語 / 地域形式 = 日本語」を日本語と誤判定する（#604 の API 選択表）"
            );
        }
    }

    /// 実機の OS へ実際に問い合わせる。**この環境の表示言語に依存しない検査だけ**を書く
    /// （CI ランナーは英語なので「ja が先頭」は固定にできない。実測値は
    /// `cargo test -- --nocapture` で出す）
    #[test]
    fn 実環境の表示言語を取得できる() {
        let langs = system_languages();
        println!("system_languages() = {langs:?}");
        if cfg!(windows) {
            // Windows は表示言語が必ず 1 つ以上ある（未設定という状態が無い）
            assert!(!langs.is_empty(), "Windows で表示言語を取得できない");
        }
        for l in &langs {
            assert!(
                !l.trim().is_empty(),
                "空の言語タグが混ざっている: {langs:?}"
            );
            assert!(
                l.is_ascii(),
                "言語タグは ASCII のはず（表示名を拾っていないか）: {l}"
            );
        }
    }
}

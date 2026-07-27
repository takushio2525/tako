//! i18n — UI 表示言語の解決と保持（Issue #435。日英切替）
//!
//! 表示言語はプロセス全体のグローバル状態（AtomicU8）として持つ。
//! render 側は `lang()` を読むだけ、切替は `set_lang()` が原子的に行うので
//! ロックや Context の引き回しは不要。
//! 設定値（system / ja / en。settings.json に永続化）と表示言語（ja / en）は
//! 別の型で区別する: `LangSetting::resolve()` が OS ロケール検出を含む解決を担う。

use std::sync::atomic::{AtomicU8, Ordering};

/// 表示言語（実際に UI へ出る言語）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    Ja,
    /// 既定は英語（`set_lang` 前の初期値。GUI は起動時に設定値で上書きする）
    #[default]
    En,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ja" => Some(Lang::Ja),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

/// 言語設定（settings.json の値。system = OS ロケール追従）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LangSetting {
    #[default]
    System,
    Ja,
    En,
}

impl LangSetting {
    pub fn as_str(&self) -> &'static str {
        match self {
            LangSetting::System => "system",
            LangSetting::Ja => "ja",
            LangSetting::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "system" => Some(LangSetting::System),
            "ja" => Some(LangSetting::Ja),
            "en" => Some(LangSetting::En),
            _ => None,
        }
    }

    /// 表示言語へ解決する（System は OS ロケール検出。プロセス起動を含むため
    /// 頻繁に呼ばない: 起動時と明示的な言語切替時のみ）
    pub fn resolve(&self) -> Lang {
        match self {
            LangSetting::System => detect_os_lang(),
            LangSetting::Ja => Lang::Ja,
            LangSetting::En => Lang::En,
        }
    }
}

/// 現在の表示言語（Ja=0 / En=1 を AtomicU8 で保持）
static CURRENT: AtomicU8 = AtomicU8::new(1);

/// 現在の表示言語を返す。UI の文字列カタログ（ui_text）が毎描画で読む
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::Ja,
        _ => Lang::En,
    }
}

/// 表示言語を切り替える（GUI の再描画は呼び出し側の責務）
pub fn set_lang(l: Lang) {
    let v = match l {
        Lang::Ja => 0,
        Lang::En => 1,
    };
    CURRENT.store(v, Ordering::Relaxed);
}

/// OS ロケールから表示言語を検出する。
/// 優先順: TAKO_LANG（検証用オーバーライド）→ LC_ALL → LC_MESSAGES → LANG →
/// macOS の AppleLanguages（GUI 起動はロケール環境変数を持たないため）→ 英語
pub fn detect_os_lang() -> Lang {
    let vars: Vec<(String, String)> = ["TAKO_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    if let Some(l) = detect_from_env(&vars) {
        return l;
    }
    #[cfg(target_os = "macos")]
    if let Some(l) = macos_preferred_language().and_then(|s| lang_from_locale(&s)) {
        return l;
    }
    Lang::En
}

/// 環境変数リスト（優先順に並んだ (名前, 値)）から言語を判定する純関数
fn detect_from_env(vars: &[(String, String)]) -> Option<Lang> {
    vars.iter().find_map(|(_, v)| lang_from_locale(v))
}

/// ロケール文字列（`ja_JP.UTF-8` / `ja-JP` / `en_US` 等）から言語を判定。
/// 空・`C`・`POSIX` 系は判定不能として None（次の候補へフォールバック）
fn lang_from_locale(s: &str) -> Option<Lang> {
    let lower = s.trim().to_ascii_lowercase();
    if lower.is_empty() || lower == "c" || lower == "posix" || lower.starts_with("c.") {
        return None;
    }
    if lower.starts_with("ja") {
        Some(Lang::Ja)
    } else {
        Some(Lang::En)
    }
}

/// macOS のシステム言語（AppleLanguages 先頭 → AppleLocale の順）。
/// `defaults` の起動を伴うため detect_os_lang 経由でのみ呼ぶ
#[cfg(target_os = "macos")]
fn macos_preferred_language() -> Option<String> {
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
    // AppleLanguages は plist 配列表記: (\n    "ja-JP",\n    "en-JP"\n)。先頭要素を抜く
    if let Some(langs) = defaults_read("AppleLanguages") {
        if let Some(first) = langs
            .split('"')
            .nth(1)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
        {
            return Some(first);
        }
    }
    defaults_read("AppleLocale")
}

/// 表示言語グローバルを触るテスト用の補助（#608）。
///
/// 表示言語はプロセス全体で共有される。cargo test は同一バイナリのテストを
/// 並列スレッドで走らせるので、`set_lang` を呼ぶテストが複数あると
/// 「A が En にしている間に B が Ja 前提で読む」競合で確率的に落ちる。
///
/// **原則は「テストは言語グローバルに触らない」**（`Note::text_in` / `gate_in` /
/// `autosuggest_hint_texts_for` のように言語を引数で受ける版を使う）。
/// グローバルへの追従そのものが検査対象のときだけ、ここのガードを取って直列化する。
#[cfg(test)]
pub(crate) mod testing {
    use super::{lang, set_lang, Lang};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// 言語グローバルの排他を保持し、drop で元の言語へ戻すガード。
    /// フィールドの drop より先に `Drop::drop` が走るので、
    /// **言語を復元してからロックを解放する**（次のテストは復元後の状態から始まる）
    pub(crate) struct LangGuard {
        _lock: MutexGuard<'static, ()>,
        original: Lang,
    }

    impl Drop for LangGuard {
        fn drop(&mut self) {
            set_lang(self.original);
        }
    }

    /// 言語グローバルを排他し、スコープを抜けたら元の言語へ戻す。
    /// 途中で assert が落ちても復元されるので、後続テストへ汚染が漏れない
    pub(crate) fn lang_guard() -> LangGuard {
        // 先のテストが assert で落ちてロックが毒されても、検査自体は続行してよい
        let _lock = lock().lock().unwrap_or_else(|e| e.into_inner());
        LangGuard {
            _lock,
            original: lang(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_roundtrip() {
        for l in [Lang::Ja, Lang::En] {
            assert_eq!(Lang::parse(l.as_str()), Some(l));
        }
        assert_eq!(Lang::parse("fr"), None);
    }

    #[test]
    fn setting_roundtrip() {
        for s in [LangSetting::System, LangSetting::Ja, LangSetting::En] {
            assert_eq!(LangSetting::parse(s.as_str()), Some(s));
        }
        assert_eq!(LangSetting::parse(""), None);
        assert_eq!(LangSetting::parse("japanese"), None);
    }

    #[test]
    fn setting_resolve_fixed() {
        assert_eq!(LangSetting::Ja.resolve(), Lang::Ja);
        assert_eq!(LangSetting::En.resolve(), Lang::En);
    }

    #[test]
    fn locale_detection() {
        assert_eq!(lang_from_locale("ja_JP.UTF-8"), Some(Lang::Ja));
        assert_eq!(lang_from_locale("ja-JP"), Some(Lang::Ja));
        assert_eq!(lang_from_locale("ja"), Some(Lang::Ja));
        assert_eq!(lang_from_locale("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(lang_from_locale("de_DE"), Some(Lang::En));
        assert_eq!(lang_from_locale(""), None);
        assert_eq!(lang_from_locale("C"), None);
        assert_eq!(lang_from_locale("C.UTF-8"), None);
        assert_eq!(lang_from_locale("POSIX"), None);
    }

    #[test]
    fn env_priority_first_valid_wins() {
        let vars = vec![
            ("LC_ALL".to_string(), "C".to_string()),
            ("LANG".to_string(), "ja_JP.UTF-8".to_string()),
        ];
        assert_eq!(detect_from_env(&vars), Some(Lang::Ja));
        let vars = vec![
            ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
            ("LANG".to_string(), "ja_JP.UTF-8".to_string()),
        ];
        assert_eq!(detect_from_env(&vars), Some(Lang::En));
        assert_eq!(detect_from_env(&[]), None);
    }

    #[test]
    fn global_lang_set_and_get() {
        // 他テストとグローバルを共有するので、排他して元の言語へ戻す（#608）
        let _guard = testing::lang_guard();
        set_lang(Lang::Ja);
        assert_eq!(lang(), Lang::Ja);
        set_lang(Lang::En);
        assert_eq!(lang(), Lang::En);
    }
}

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
/// OS の優先表示言語（GUI 起動はロケール環境変数を持たないため。境界 B10）→ 英語
pub fn detect_os_lang() -> Lang {
    let vars: Vec<(String, String)> = ["TAKO_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    detect_with(&vars, crate::platform::locale::system_languages)
}

/// 検出の優先順を表す純関数（環境変数 → OS の優先表示言語 → 英語）。
///
/// `os_languages` は**環境変数で決まらなかったときだけ**評価する
/// （macOS 実装が `defaults` を起こすため。順序と遅延の両方をテストで固定する）。
/// OS の優先表示言語は順序付きリストなので、判定できた最初の 1 件を採る
/// （`C` / `POSIX` のような判定不能値は次の候補へ送られる）
fn detect_with(vars: &[(String, String)], os_languages: impl FnOnce() -> Vec<String>) -> Lang {
    if let Some(l) = detect_from_env(vars) {
        return l;
    }
    if let Some(l) = os_languages().iter().find_map(|s| lang_from_locale(s)) {
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

    fn vars(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn os_langs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// TAKO_LANG 等の環境変数が OS の表示言語より優先される（#604 の修正で変えない既存挙動）。
    /// **OS へ問い合わせないこと**も併せて固定する（macOS の実装が `defaults` を起こすため）
    #[test]
    fn 環境変数はos表示言語より優先されos問い合わせもしない() {
        let asked = std::cell::Cell::new(false);
        let got = detect_with(&vars(&[("TAKO_LANG", "en")]), || {
            asked.set(true);
            os_langs(&["ja-JP"])
        });
        assert_eq!(got, Lang::En);
        assert!(!asked.get(), "環境変数で決まったのに OS へ問い合わせている");
    }

    /// #604 の本体: 環境変数が無い（Windows GUI の常態）ときに OS の表示言語で決まる
    #[test]
    fn 環境変数が無ければos表示言語で決まる() {
        assert_eq!(detect_with(&[], || os_langs(&["ja-JP", "en-US"])), Lang::Ja);
        assert_eq!(detect_with(&[], || os_langs(&["en-US", "ja-JP"])), Lang::En);
    }

    /// 対応していない言語は英語へ落ちる（先頭の優先言語で決める）
    #[test]
    fn 対応外の表示言語は英語になる() {
        assert_eq!(detect_with(&[], || os_langs(&["fr-FR", "ja-JP"])), Lang::En);
    }

    /// 判定不能な値は次の候補へ送る。OS の優先順リストをフォールバック鎖として使う
    #[test]
    fn 判定不能な値は次の優先言語へ送る() {
        assert_eq!(detect_with(&[], || os_langs(&["C", "ja-JP"])), Lang::Ja);
    }

    /// OS から何も取れない環境（境界が空を返す）は従来どおり英語
    #[test]
    fn os表示言語が空なら英語へフォールバックする() {
        assert_eq!(detect_with(&[], Vec::new), Lang::En);
    }

    #[test]
    fn global_lang_set_and_get() {
        // 他テストとグローバルを共有するため、最後に既定へ戻す
        set_lang(Lang::Ja);
        assert_eq!(lang(), Lang::Ja);
        set_lang(Lang::En);
        assert_eq!(lang(), Lang::En);
    }
}

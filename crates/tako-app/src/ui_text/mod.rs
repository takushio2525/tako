//! UI 文字列カタログ（#440 で新設 → #435 で日英 i18n 化）
//!
//! UI に出す文章を render コードへ直書きせず、ここへ機能別モジュールで集約する。
//! 関数名がそのままロケールキーに対応する（例: `sleep_guard::chip_active()` →
//! キー `sleep_guard.chip_active`）。表示言語は `tako_core::i18n` のグローバルが正で、
//! 各関数は `tr!` マクロで現在言語の文字列を返す。
//!
//! 運用ルール（`.agent/conventions.md`「UI 文字列の i18n」）:
//! - 新機能の UI 文字列は必ず日英両方を用意する（`tr!(日本語, English)`）
//! - コマンド文字列・パス・ロゴ等の言語非依存文字列は `pub const` のまま置いてよい
//! - 絵文字は使わない（#217。`all_texts_have_both_languages_and_no_emoji` が機械検査）

/// 現在の表示言語で日英どちらかの式を返す。match 展開なので選ばれた側だけ評価される
/// （`tr!(format!(..), format!(..))` でも未選択側の format は走らない）
macro_rules! tr {
    ($ja:expr, $en:expr $(,)?) => {
        match ::tako_core::i18n::lang() {
            ::tako_core::i18n::Lang::Ja => $ja,
            ::tako_core::i18n::Lang::En => $en,
        }
    };
}
// 注: 子モジュールは textual scope（この定義が mod 宣言より前にあること）で tr! を
// 直接使える。use は不要（unused import になる）。mod 宣言をマクロ定義より前に
// 移動しないこと

pub mod common;
pub mod drawer;
pub mod palette;
pub mod panel;
pub mod remote;
pub mod sidebar;
pub mod sleep_guard;
pub mod update;

#[cfg(test)]
pub(crate) mod tests_support {
    use tako_core::i18n::{self, Lang};

    /// 日英カタログの機械検査。collect を Ja / En それぞれで実行し、
    /// 全文字列が非空・絵文字なし（#217）・英語側に日本語が残っていないことを検査する。
    /// 言語グローバルを切り替えるため、lang 依存の他テストは相対比較で書くこと
    pub(crate) fn check_ja_en(collect: impl Fn() -> Vec<String>) {
        let original = i18n::lang();
        i18n::set_lang(Lang::Ja);
        let ja = collect();
        i18n::set_lang(Lang::En);
        let en = collect();
        i18n::set_lang(original);
        assert_eq!(ja.len(), en.len());
        for (j, e) in ja.iter().zip(en.iter()) {
            assert!(!j.trim().is_empty(), "日本語文字列が空");
            assert!(!e.trim().is_empty(), "英語文字列が空: 対 {j:?}");
            assert_no_emoji(j);
            assert_no_emoji(e);
            // 訳し漏れ検出: 英語側にかな・漢字が残っていないこと
            assert!(
                !e.chars()
                    .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
                "英語文字列に日本語が残っている: {e:?}"
            );
        }
    }

    fn assert_no_emoji(s: &str) {
        for c in s.chars() {
            let cp = c as u32;
            assert!(
                !(0x1F000..=0x1FAFF).contains(&cp)
                    && !(0x2600..=0x27BF).contains(&cp)
                    && !(0xFE00..=0xFE0F).contains(&cp),
                "絵文字らしき文字 {c:?} (U+{cp:04X}) が文字列 {s:?} に含まれている"
            );
        }
    }
}

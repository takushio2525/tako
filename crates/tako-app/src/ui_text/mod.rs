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

pub mod about;
pub mod command_card;
pub mod common;
pub mod dialog;
pub mod drawer;
pub mod menu;
pub mod palette;
pub mod pane_menu;
pub mod panel;
pub mod path_menu;
pub mod ports;
pub mod preview;
pub mod remote;
pub mod remote_folder;
pub mod settings;
pub mod sidebar;
pub mod sleep_guard;
pub mod stale;
pub mod ui_mode;
pub mod update;
pub mod webdock;
pub mod welcome;

/// 言語グローバル競合の番犬（#1274）。このモジュール配下のテストが
/// 言語依存の文字列をロック外で読み比べていないことをソース走査で拘束する
#[cfg(test)]
mod lang_watchdog;

/// 言語グローバル競合の再現テスト（#1274）。
///
/// **実体を `src/ui_text/` の外に置いてある**のは、再現の legacy 経路が
/// 「ロック外で言語依存の関数を読む」形そのもので、`lang_watchdog` の走査対象へ
/// 入れると自分自身に噛みつくから。`main.rs` 側へ `mod` を足すと、本番コードを
/// 「最初の `#[cfg(test)]` まで」で切り出している番犬（`pane_content_geometry_tests`）が
/// 空振りするので、宣言はここから行う
#[cfg(test)]
#[path = "../ui_text_lang_race.rs"]
mod lang_race;

#[cfg(test)]
pub(crate) mod tests_support {
    use tako_core::i18n::{self, Lang};

    /// 言語グローバルを触るテストの直列化ロック（#496 / #1274）。
    ///
    /// `check_ja_en` はプロセス全体で共有される言語設定を Ja → En → 復元と切り替える。
    /// カタログテストは各モジュールに 1 本ずつあり、cargo test は既定で並列実行するため、
    /// ロックが無いと「A が En に切り替えている間に B が Ja 前提で collect する」
    /// 競合が起き、`英語文字列に日本語が残っている` で確率的に落ちる
    /// （実測: `cargo test -p tako-app ui_text` で毎回 2〜4 本が失敗）。
    fn lang_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    /// 言語グローバルを排他し、drop で元の言語へ戻すガード（#1274）。
    ///
    /// **言語依存の文字列を 2 つ以上読み比べるテストは、必ずこのガード
    /// （またはこれを内蔵する `check_ja_en` / `for_each_lang` / `with_lang`）の
    /// 区間の中で比較する。** 表示言語はプロセス全体の AtomicU8 なので、ロックの外で
    /// 2 回読むと**読み取りのあいだに別スレッドのテストが言語を切り替え、
    /// 別言語同士を比較して落ちる**（#1274。`共通項目はファイルツリーと同一文言` が
    /// `cargo test --workspace` でだけ低頻度に落ちていた実害）。
    ///
    /// フィールドの drop より先に `Drop::drop` が走るので、**言語を復元してから
    /// ロックを解放する**（次のテストは復元後の状態から始まる）。途中で assert が
    /// 落ちても復元されるので、後続テストへ汚染が漏れない
    pub(crate) struct LangGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        original: Lang,
    }

    impl Drop for LangGuard {
        fn drop(&mut self) {
            i18n::set_lang(self.original);
        }
    }

    /// 言語グローバルを排他する。**ガードが生きているあいだ、他のテストは
    /// 言語を切り替えられない**（切替は必ずこのロックを通るため）
    pub(crate) fn lang_guard() -> LangGuard {
        // 前のテストが assert で落ちてロックが毒されても、検査自体は続行してよい
        let lock = lang_lock().lock().unwrap_or_else(|e| e.into_inner());
        LangGuard {
            _lock: lock,
            original: i18n::lang(),
        }
    }

    /// 日英カタログの機械検査。collect を Ja / En それぞれで実行し、
    /// 全文字列が非空・絵文字なし（#217）・英語側に日本語が残っていないことを検査する。
    ///
    /// **相対比較（`結果 == カタログ関数()`）で書くだけでは足りない**（#1274）。
    /// 比較の 2 点のあいだに別スレッドがここへ入って言語を切り替えると、
    /// 別言語同士を比べて落ちる。相対比較のテストも `for_each_lang` などで
    /// 言語を固定した 1 区間の中に入れること
    pub(crate) fn check_ja_en(collect: impl Fn() -> Vec<String>) {
        let _guard = lang_guard();
        i18n::set_lang(Lang::Ja);
        let ja = collect();
        i18n::set_lang(Lang::En);
        let en = collect();
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

    /// 日英それぞれで `body` を 1 回ずつ走らせる（#727）。
    ///
    /// `check_ja_en` は「非空・絵文字なし・訳し漏れなし」を検査する専用形だが、
    /// 「この文字列が出ない / 出る」のような**独自の検査を両言語で**やりたい場所もある。
    /// 言語グローバルは共有なので、切り替えは同じロックの下で行う
    pub(crate) fn for_each_lang(body: impl Fn()) {
        let _guard = lang_guard();
        for lang in [Lang::Ja, Lang::En] {
            i18n::set_lang(lang);
            body();
        }
    }

    /// 指定した言語で `body` を 1 回走らせる（#905）。
    ///
    /// 「macOS 側の文言が 1 文字も動いていない」のように**実文字列で押さえたい**検査は
    /// 言語を固定しないと書けない。ロックは呼び出しごとに取り直すので、
    /// 続けて 2 回呼んでも他テストと競合しない
    pub(crate) fn with_lang(lang: Lang, body: impl FnOnce()) {
        let _guard = lang_guard();
        i18n::set_lang(lang);
        body();
    }

    /// macOS の platform 修飾（`⌘` / `Cmd`）。**正本から引く**（#1203。
    /// カタログテストへ記号を直書きすると、番犬が見張っている経路の外で
    /// 手書き表記が増えていく）
    pub(crate) fn mac_modifier() -> Option<tako_core::platform::keys::ModifierLabel> {
        tako_core::platform::keys::platform_modifier(tako_core::platform::support::Platform::MacOs)
    }

    /// macOS の「修飾 + Enter」表記（`Cmd+Enter`）
    pub(crate) fn mac_modifier_enter() -> Option<String> {
        tako_core::platform::keys::modifier_enter(tako_core::platform::support::Platform::MacOs)
    }

    fn assert_no_emoji(s: &str) {
        for c in s.chars() {
            let cp = c as u32;
            assert!(
                !(0x1F000..=0x1FAFF).contains(&cp)
                    && !(0x2600..=0x27BF).contains(&cp)
                    // FE0E（テキスト表示強制）は絵文字化を防ぐ側なので許可。FE0F のみ拒否
                    && cp != 0xFE0F,
                "絵文字らしき文字 {c:?} (U+{cp:04X}) が文字列 {s:?} に含まれている"
            );
        }
    }
}

//! 案内文に載せる打鍵の表記（プラットフォーム別。Issue #1203）
//!
//! **何のためにあるか**: UI 文言・CLI / MCP の案内へ `⌘K` のような macOS の記号を
//! 直書きすると、Windows では**書いてあるとおりに押しても効かない**（GPUI の
//! `cmd` は platform 修飾で、Windows では Win キーへ解決される = #585）。
//! `Win+K` は OS のキャストが開くので、案内としては嘘を通り越して有害になる。
//!
//! キーバインドそのものの正本は `tako-app` の `keybindings::key_bindings()` で、
//! ここはそこから**案内文へ埋める形**へ落とした写し。両者の一致は tako-app の番犬
//! `案内文の打鍵表記はバインド表と一致する` が **macOS / Windows の両方について**
//! 検査する（`shortcut_hint_for` が `Platform` を引数に取る純粋関数なので、
//! macOS 上からも Windows 側の表記を突き合わせられる）。
//!
//! なぜ tako-core に置くか: 案内を出すのは GUI だけではない。`tako ui-mode` の
//! `next_step`（CLI / MCP の機械可読な案内）も同じ表記を使うので、GPUI に依存しない
//! 側から引ける場所が要る。

use super::support::Platform;

/// コマンドパレットを開く打鍵。
///
/// Windows は `Ctrl+Shift+P`（VS Code / Windows Terminal 慣習）。`Ctrl+Shift+K` も
/// 同じアクションに張ってあるが、**案内には 1 本だけ出す**（#322 の「最も簡単な形」）。
pub fn command_palette(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2318}K",
        Platform::Windows => "Ctrl+Shift+P",
    }
}

/// プレビューを保存する打鍵。
pub fn save_preview(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2318}S",
        Platform::Windows => "Ctrl+Shift+S",
    }
}

/// tako を終了する打鍵。
pub fn quit(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2318}Q",
        Platform::Windows => "Ctrl+Shift+Q",
    }
}

/// GPUI の platform 修飾（macOS = command / Windows = Win キー）の表記。
///
/// **非 macOS では `None`**。キーバインド表に載っていない「修飾 + クリック」
/// 「修飾 + Enter」のような操作は `Modifiers::platform` を直接見ているので、
/// Windows では Win キーになる。Win+クリック / Win+Enter は OS 側が持っていて
/// 実質押せない（#763）ため、**案内に出さない**のが正しい振る舞いになる
/// （`keybindings::shortcut_hint_for` が非 macOS の platform 修飾バインドを
/// 落とすのと同じ規則）。呼び出し側は `None` のとき打鍵に触れない文へ落とす。
///
/// 打鍵そのものを Ctrl へ移すのは #763 の仕事で、そちらが入れば
/// ここも Windows 側の表記を返す形へ変わる。
pub fn platform_modifier(platform: Platform) -> Option<ModifierLabel> {
    match platform {
        Platform::MacOs => Some(ModifierLabel {
            symbol: "\u{2318}",
            word: "Cmd",
        }),
        Platform::Windows => None,
    }
}

/// 「修飾 + Enter」で確定する操作の打鍵表記（語形。macOS = `Cmd+Enter`）。
///
/// [`platform_modifier`] と同じ理由で **非 macOS は `None`**
pub fn modifier_enter(platform: Platform) -> Option<String> {
    platform_modifier(platform).map(|m| format!("{}+Enter", m.word))
}

/// 修飾キーの表記。日本語 UI は記号（`⌘`）、英語 UI は語（`Cmd`）を使う
/// （既存の文言がそう書き分けているので、macOS 側の見た目を 1 文字も変えない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierLabel {
    /// 記号表記（`⌘`）
    pub symbol: &'static str,
    /// 語表記（`Cmd`）
    pub word: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 案内の打鍵はプラットフォームごとに変わる() {
        assert_eq!(command_palette(Platform::MacOs), "\u{2318}K");
        assert_eq!(command_palette(Platform::Windows), "Ctrl+Shift+P");
        assert_eq!(save_preview(Platform::MacOs), "\u{2318}S");
        assert_eq!(save_preview(Platform::Windows), "Ctrl+Shift+S");
        assert_eq!(quit(Platform::MacOs), "\u{2318}Q");
        assert_eq!(quit(Platform::Windows), "Ctrl+Shift+Q");
    }

    /// Windows の案内に macOS の記号 / 語が混ざらない（#1203 の症状そのもの）
    #[test]
    fn windowsの案内にcmd表記が出ない() {
        for s in [
            command_palette(Platform::Windows),
            save_preview(Platform::Windows),
            quit(Platform::Windows),
        ] {
            assert!(!s.contains('\u{2318}'), "{s:?} に ⌘ が残っている");
            assert!(!s.contains("Cmd"), "{s:?} に Cmd が残っている");
            assert!(!s.contains("Win"), "{s:?} に Win が残っている");
        }
        assert_eq!(
            platform_modifier(Platform::Windows),
            None,
            "Windows の platform 修飾（Win キー）は押せないので案内してはいけない"
        );
    }

    #[test]
    fn 修飾_enterの表記はmacosだけ出る() {
        assert_eq!(
            modifier_enter(Platform::MacOs).as_deref(),
            Some("Cmd+Enter")
        );
        assert_eq!(modifier_enter(Platform::Windows), None);
    }

    #[test]
    fn macosのplatform修飾は記号と語を持つ() {
        let m = platform_modifier(Platform::MacOs).expect("macOS には ⌘ がある");
        assert_eq!(m.symbol, "\u{2318}");
        assert_eq!(m.word, "Cmd");
    }
}

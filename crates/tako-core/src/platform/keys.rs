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
/// 実質押せないため、**案内に出さない**のが正しい振る舞いになる
/// （`keybindings::shortcut_hint_for` が非 macOS の platform 修飾バインドを
/// 落とすのと同じ規則）。呼び出し側は `None` のとき打鍵に触れない文へ落とす。
///
/// **リンクを開く操作はここではなく [`link_modifier`] を引く**。あちらは #763 で
/// Windows 側を Ctrl へ移したので、両 OS とも押せる打鍵を返す。ここが `None` を
/// 返し続けるのは、確認スキップ・コミット確定のように **まだ platform 修飾を
/// 直接見ている**操作のぶん（それらを Ctrl へ移すのは別の仕事）。
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

/// リンクを開く「修飾 + クリック」の修飾キー表記（Issue #763）。
///
/// [`platform_modifier`] と違って **両 OS とも `Some` 相当**（`Option` を返さない）。
/// リンクだけは打鍵そのものを Windows で Ctrl へ移したので、案内を落とす必要が無い。
/// 判定側の正本は [`link_modifier_active`] で、**表記と判定は必ず同じ表**を見る。
pub fn link_modifier(platform: Platform) -> ModifierLabel {
    match platform {
        Platform::MacOs => ModifierLabel {
            symbol: "\u{2318}",
            word: "Cmd",
        },
        // Windows は記号表記を持たない（`⌃` は macOS の control 記号なので使わない）
        Platform::Windows => ModifierLabel {
            symbol: "Ctrl",
            word: "Ctrl",
        },
    }
}

/// リンクを開く「修飾 + クリック」の案内表記（macOS = `⌘+クリック` /
/// Windows = `Ctrl+クリック`）。
///
/// 案内を出す側（MCP カタログ・CLI ヘルプ・docs）が**同じ 1 文を引く**ための形。
/// `modifier_enter` が `Cmd+Enter` を組むのと同じ役割で、記号は [`link_modifier`] が持つ
pub fn link_click(platform: Platform) -> String {
    format!("{}+クリック", link_modifier(platform).symbol)
}

/// リンクを開く修飾キーが押されているか（Issue #763）。
///
/// `platform_key` / `control` には GPUI の `Modifiers::platform` /
/// `Modifiers::control` をそのまま渡す。**GPUI に依存しない形で受ける**ので、
/// macOS 上から Windows 側の分岐を単体で検証できる（#515 と同じ方針）。
///
/// なぜ `platform || control` を素で書かないか:
///
/// - **macOS で control を混ぜてはいけない**。macOS の Ctrl+クリックは右クリック
///   相当なので、混ぜるとコンテキストメニューとリンク開きが同時に走る
/// - **Windows で platform（Win キー）を混ぜてはいけない**。Win+クリックは OS の
///   シェルが持っていて tako まで届かないことがあり、「効くときと効かないときが
///   ある」という一番たちの悪い挙動になる。案内できない打鍵は受理もしない
pub fn link_modifier_active(platform: Platform, platform_key: bool, control: bool) -> bool {
    match platform {
        Platform::MacOs => platform_key,
        Platform::Windows => control,
    }
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

    /// #763: リンクを開く修飾キーは **macOS = command のみ / Windows = Ctrl のみ**。
    /// 「片方の OS では押せない打鍵を受理する」を両方向から固定する
    #[test]
    fn リンクの修飾キーはosごとに1つだけ受理する() {
        // macOS: command で開く / control では開かない（Ctrl+クリックは右クリック相当）
        assert!(link_modifier_active(Platform::MacOs, true, false));
        assert!(!link_modifier_active(Platform::MacOs, false, true));
        // Windows: control で開く / platform（Win キー）では開かない
        assert!(link_modifier_active(Platform::Windows, false, true));
        assert!(!link_modifier_active(Platform::Windows, true, false));
        // 修飾なしはどちらも開かない
        assert!(!link_modifier_active(Platform::MacOs, false, false));
        assert!(!link_modifier_active(Platform::Windows, false, false));
        // 両方押されていれば、その OS が見るほうが立っているので開く
        assert!(link_modifier_active(Platform::MacOs, true, true));
        assert!(link_modifier_active(Platform::Windows, true, true));
    }

    /// 表記と判定が同じ表を見ていること（片方だけ直すと案内が嘘になる）
    #[test]
    fn リンクの修飾キーの表記は両osとも出る() {
        assert_eq!(link_modifier(Platform::MacOs).symbol, "\u{2318}");
        assert_eq!(link_modifier(Platform::MacOs).word, "Cmd");
        assert_eq!(link_modifier(Platform::Windows).symbol, "Ctrl");
        assert_eq!(link_modifier(Platform::Windows).word, "Ctrl");
        assert_eq!(link_click(Platform::MacOs), "\u{2318}+クリック");
        assert_eq!(link_click(Platform::Windows), "Ctrl+クリック");
        for s in [
            link_modifier(Platform::Windows).symbol,
            link_modifier(Platform::Windows).word,
        ] {
            assert!(!s.contains('\u{2318}'), "{s:?} に macOS の記号が残っている");
            assert!(!s.contains("Cmd"), "{s:?} に Cmd が残っている");
            assert!(!s.contains("Win"), "{s:?} に Win が残っている");
        }
    }

    #[test]
    fn macosのplatform修飾は記号と語を持つ() {
        let m = platform_modifier(Platform::MacOs).expect("macOS には ⌘ がある");
        assert_eq!(m.symbol, "\u{2318}");
        assert_eq!(m.word, "Cmd");
    }
}

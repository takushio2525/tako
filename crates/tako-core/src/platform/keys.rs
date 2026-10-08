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

/// ジャンプ履歴を戻る打鍵（FR-3.29 / #1677）。
///
/// Windows の `Ctrl+-` は縮小（ZoomOut）に使っているので、JetBrains 系と同じ
/// `Ctrl+Alt+←` に置く（理由の全文は tako-app の `keybindings::jump_bindings`）。
pub fn jump_back(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2303}-",
        Platform::Windows => "Ctrl+Alt+\u{2190}",
    }
}

/// ジャンプ履歴を進む打鍵（FR-3.29 / #1677）。
///
/// macOS の `⌃⇧-` は配列によって届く字面が変わる（US = `ctrl-_` / JIS = `ctrl-=`）ので
/// バインドは 2 本あるが、案内は**押す形**の 1 本だけを出す。
pub fn jump_forward(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2303}\u{21e7}-",
        Platform::Windows => "Ctrl+Alt+\u{2192}",
    }
}

/// 編集中のコードを整形する打鍵（FR-3.33 / #1683）。
///
/// macOS は Zed と同じ `⇧⌘I`（メニューの表記順 ⌃⌥⇧⌘）、Windows は Zed / VS Code（Linux）と
/// 同じ `Ctrl+Shift+I`。
/// `Ctrl+Shift+<英字>` は `Ctrl+<英字>` と同じ C0 バイトへ潰れる（#585）ので、端末の
/// `Ctrl+I`（= Tab）は奪わない（理由の全文は tako-app の `keybindings::format_bindings`）
pub fn format_document(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{21e7}\u{2318}I",
        Platform::Windows => "Ctrl+Shift+I",
    }
}

/// カーソル位置のホバー情報（型・doc のカード）を出す打鍵（FR-3.37 / #1893）。
///
/// macOS は `⇧⌘H`（整形の `⇧⌘I` と同じ段。VS Code / Zed の `⌘K ⌘I` は ⌘K = パレットと
/// 衝突するので張れない）、Windows は `Ctrl+Shift+H`（`Ctrl+H` = Backspace と同じ C0 バイトへ
/// 潰れるので端末の `Ctrl+H` は奪わない。理由の全文は tako-app の `keybindings::hover_bindings`）
pub fn show_hover(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{21e7}\u{2318}H",
        Platform::Windows => "Ctrl+Shift+H",
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

/// ファイルツリーのコピー / 切り取り / 貼り付けの操作（FR-3.34 / Issue #1860）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeClipKey {
    Copy,
    Cut,
    Paste,
    /// 移動として貼る（⌥⌘V = Finder の「項目をここに移動」。Windows は Ctrl+Alt+V。
    /// FR-3.38 / Issue #1867）
    PasteMove,
}

impl TreeClipKey {
    fn letter(self) -> char {
        match self {
            Self::Copy => 'C',
            Self::Cut => 'X',
            Self::Paste | Self::PasteMove => 'V',
        }
    }
}

/// 打鍵がファイルツリーのコピー / 切り取り / 貼り付けか（Issue #1860）。
///
/// 修飾は**リンクと同じ規則**（[`link_modifier_active`]）: macOS = command のみ /
/// Windows = control のみ。ほかの修飾（shift / alt / もう片方）が混ざったら当たらない
/// （⌘⇧V などの別の割り当てを奪わない）。**ツリーの行を選んでいるときだけ**呼ぶこと:
/// Windows の Ctrl+C / Ctrl+X は端末の SIGINT / readline の入力なので、選んでいなければ
/// 端末へ流す（判定はここ、宛先の判定は GUI）。
///
/// 主修飾 + alt + V だけは**移動として貼る**（⌥⌘V / Windows は Ctrl+Alt+V。#1867）。
/// Windows の AltGr は Ctrl+Alt として届くので、**文字を生む打鍵**（`text` = 欧州配列の
/// AltGr+V = `@` 等）は文字入力のまま奪わない（macOS の ⌘ つきは文字入力にならない）
pub fn tree_clip_key(
    platform: Platform,
    key: &str,
    platform_key: bool,
    control: bool,
    alt: bool,
    shift: bool,
    text: bool,
) -> Option<TreeClipKey> {
    let only_primary = match platform {
        Platform::MacOs => platform_key && !control,
        Platform::Windows => control && !platform_key,
    };
    if !only_primary || shift {
        return None;
    }
    if alt {
        let altgr_text = platform == Platform::Windows && text;
        return (key == "v" && !altgr_text).then_some(TreeClipKey::PasteMove);
    }
    match key {
        "c" => Some(TreeClipKey::Copy),
        "x" => Some(TreeClipKey::Cut),
        "v" => Some(TreeClipKey::Paste),
        _ => None,
    }
}

/// ファイルツリーで選んでいる間のキー（コピー / 切り取り / 貼り付け以外。Issue #1895）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSelectKey {
    /// ⇧↑: 範囲を 1 行上へ伸ばす / 縮める
    ExtendUp,
    /// ⇧↓: 範囲を 1 行下へ伸ばす / 縮める
    ExtendDown,
    /// 選んだものをごみ箱へ（macOS = ⌘⌫ / Windows = Delete。Finder / エクスプローラーと同じ）
    Trash,
}

/// 打鍵がファイルツリーの範囲選択・ごみ箱か（Issue #1895）。**ツリーの行を選んでいるときだけ**
/// 呼ぶこと（[`tree_clip_key`] と同じ。選んでいなければ ⇧↑ / Delete は端末へ流す）。
///
/// - ⇧↑ / ⇧↓: shift だけ（主修飾・alt・もう片方が混ざると当たらない = ⌘⇧↑ 等の別の割り当てを奪わない）
/// - ごみ箱: macOS は ⌘⌫ だけ（⌫ 単独は奪わない）/ Windows は修飾無しの Delete だけ
///   （Shift+Delete = エクスプローラーの「完全に削除」は扱わない = 端末へ流す）
pub fn tree_select_key(
    platform: Platform,
    key: &str,
    platform_key: bool,
    control: bool,
    alt: bool,
    shift: bool,
) -> Option<TreeSelectKey> {
    let none_but_shift = shift && !platform_key && !control && !alt;
    match key {
        "up" if none_but_shift => Some(TreeSelectKey::ExtendUp),
        "down" if none_but_shift => Some(TreeSelectKey::ExtendDown),
        "backspace"
            if platform == Platform::MacOs && platform_key && !control && !alt && !shift =>
        {
            Some(TreeSelectKey::Trash)
        }
        "delete"
            if platform == Platform::Windows && !platform_key && !control && !alt && !shift =>
        {
            Some(TreeSelectKey::Trash)
        }
        _ => None,
    }
}

/// ごみ箱の打鍵表記（右クリックメニューの「削除」の右端。macOS = `⌘⌫` / Windows = `Delete`）
pub fn tree_trash_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "\u{2318}\u{232B}",
        Platform::Windows => "Delete",
    }
}

/// ファイルツリーのコピー / 切り取り / 貼り付けの打鍵表記（右クリックメニューの右端。
/// macOS = `⌘C` / Windows = `Ctrl+C`）
pub fn tree_clip_hint(platform: Platform, key: TreeClipKey) -> String {
    let alt = key == TreeClipKey::PasteMove;
    match (platform, alt) {
        (Platform::MacOs, false) => format!("\u{2318}{}", key.letter()),
        (Platform::MacOs, true) => format!("\u{2325}\u{2318}{}", key.letter()),
        (Platform::Windows, false) => format!("Ctrl+{}", key.letter()),
        (Platform::Windows, true) => format!("Ctrl+Alt+{}", key.letter()),
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
    fn ツリーの範囲選択とごみ箱の打鍵はosの作法どおりに当たる() {
        use TreeSelectKey::*;
        let mac = |key, p, c, a, s| tree_select_key(Platform::MacOs, key, p, c, a, s);
        let win = |key, p, c, a, s| tree_select_key(Platform::Windows, key, p, c, a, s);
        for f in [mac, win] {
            assert_eq!(f("up", false, false, false, true), Some(ExtendUp));
            assert_eq!(f("down", false, false, false, true), Some(ExtendDown));
            // 素の矢印・⌘⇧↑・⌥⇧↓・⌃⇧↑ は奪わない
            assert_eq!(f("up", false, false, false, false), None);
            assert_eq!(f("up", true, false, false, true), None);
            assert_eq!(f("down", false, false, true, true), None);
            assert_eq!(f("up", false, true, false, true), None);
        }
        // macOS は ⌘⌫ だけ（⌫ 単独・⌥⌫・⌘⇧⌫・Delete は奪わない）
        assert_eq!(mac("backspace", true, false, false, false), Some(Trash));
        assert_eq!(mac("backspace", false, false, false, false), None);
        assert_eq!(mac("backspace", false, false, true, false), None);
        assert_eq!(mac("backspace", true, false, false, true), None);
        assert_eq!(mac("delete", false, false, false, false), None);
        // Windows は修飾無しの Delete だけ（Shift+Delete = 完全に削除・Ctrl+Backspace は奪わない）
        assert_eq!(win("delete", false, false, false, false), Some(Trash));
        assert_eq!(win("delete", false, false, false, true), None);
        assert_eq!(win("delete", false, true, false, false), None);
        assert_eq!(win("backspace", false, true, false, false), None);
        assert_eq!(win("backspace", true, false, false, false), None);
        assert_eq!(tree_trash_hint(Platform::MacOs), "\u{2318}\u{232B}");
        assert_eq!(tree_trash_hint(Platform::Windows), "Delete");
    }

    #[test]
    fn ツリーのクリップボードの打鍵は主修飾だけで当たる() {
        use TreeClipKey::*;
        let mac = |key, p, c, a, s| tree_clip_key(Platform::MacOs, key, p, c, a, s, false);
        let win = |key, p, c, a, s| tree_clip_key(Platform::Windows, key, p, c, a, s, false);
        assert_eq!(mac("c", true, false, false, false), Some(Copy));
        assert_eq!(mac("x", true, false, false, false), Some(Cut));
        assert_eq!(mac("v", true, false, false, false), Some(Paste));
        // macOS の Ctrl+C は端末の SIGINT のまま・⌘⇧V は別の割り当て
        assert_eq!(mac("c", false, true, false, false), None);
        assert_eq!(mac("v", true, false, false, true), None);
        assert_eq!(mac("a", true, false, false, false), None);
        assert_eq!(win("c", false, true, false, false), Some(Copy));
        assert_eq!(win("x", false, true, false, false), Some(Cut));
        assert_eq!(win("v", false, true, false, false), Some(Paste));
        // Windows の Win+V はクリップボード履歴・Ctrl+Shift+C は端末のコピー
        assert_eq!(win("v", true, false, false, false), None);
        assert_eq!(win("c", false, true, false, true), None);
        assert_eq!(tree_clip_hint(Platform::MacOs, Copy), "\u{2318}C");
        assert_eq!(tree_clip_hint(Platform::Windows, Paste), "Ctrl+V");
    }

    /// #1867: ⌥⌘V（Windows は Ctrl+Alt+V）は移動として貼る。V 以外・⇧ 混じりは当たらず、
    /// Windows で文字を生む AltGr（Ctrl+Alt）は文字入力のまま
    #[test]
    fn 主修飾とaltのvは移動として貼る() {
        use TreeClipKey::*;
        let mac = |key, s, t| tree_clip_key(Platform::MacOs, key, true, false, true, s, t);
        let win = |key, s, t| tree_clip_key(Platform::Windows, key, false, true, true, s, t);
        assert_eq!(mac("v", false, false), Some(PasteMove));
        // macOS の ⌘ つきは文字入力にならない（⌥V の「√」を見ても移動のまま）
        assert_eq!(mac("v", false, true), Some(PasteMove));
        assert_eq!(mac("c", false, false), None);
        assert_eq!(mac("x", false, false), None);
        assert_eq!(mac("v", true, false), None, "⌥⇧⌘V は別の割り当て");
        assert_eq!(win("v", false, false), Some(PasteMove));
        assert_eq!(
            win("v", false, true),
            None,
            "AltGr+V の文字（@ 等）は奪わない"
        );
        assert_eq!(win("c", false, false), None);
        // 主修飾が無い ⌥V（macOS の「√」/ Windows の Alt+V = Claude Code の画像貼り付け）
        assert_eq!(
            tree_clip_key(Platform::MacOs, "v", false, false, true, false, true),
            None
        );
        assert_eq!(
            tree_clip_key(Platform::Windows, "v", false, false, true, false, false),
            None
        );
        assert_eq!(
            tree_clip_hint(Platform::MacOs, PasteMove),
            "\u{2325}\u{2318}V"
        );
        assert_eq!(tree_clip_hint(Platform::Windows, PasteMove), "Ctrl+Alt+V");
    }

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

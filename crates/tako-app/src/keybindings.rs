use gpui::{actions, KeyBinding, Keystroke, Modifiers};

actions!(
    tako,
    [
        SplitRight,
        SplitDown,
        ClosePane,
        NewTab,
        NextTab,
        PrevTab,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        WidenPane,
        NarrowPane,
        TallenPane,
        ShortenPane,
        CopySelection,
        PasteClipboard,
        SavePreview,
        ToggleSidebar,
        Quit,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        SelectAll,
        OpenDirectory,
        OpenRepository,
        OpenRemote,
        OpenRecent,
        NewWindow,
        OpenSettings,
        UndoPreview,
        RedoPreview,
        FindPreview,
        OpenCommandPalette,
        // macOS アプリケーションメニュー（#485）。すべて実在の動作に配線する
        AboutTako,
        CheckForUpdates,
        HideApp,
        HideOthers,
        ShowAllApps,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        ToggleDrawer,
        ToggleTheme,
        SwitchLanguage,
        ShowFleetPanel,
        ShowOrchPanel,
        ShowGitPanel,
        OpenDocumentation,
        ReportIssue
    ]
);

/// iTerm2 の操作感を踏襲したキーバインド
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-w", ClosePane, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("cmd-alt-left", FocusLeft, None),
        KeyBinding::new("cmd-alt-right", FocusRight, None),
        KeyBinding::new("cmd-alt-up", FocusUp, None),
        KeyBinding::new("cmd-alt-down", FocusDown, None),
        KeyBinding::new("ctrl-cmd-right", WidenPane, None),
        KeyBinding::new("ctrl-cmd-left", NarrowPane, None),
        KeyBinding::new("ctrl-cmd-down", TallenPane, None),
        KeyBinding::new("ctrl-cmd-up", ShortenPane, None),
        KeyBinding::new("cmd-c", CopySelection, None),
        KeyBinding::new("cmd-v", PasteClipboard, None),
        KeyBinding::new("cmd-s", SavePreview, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-k", OpenCommandPalette, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-1", ActivateTab1, None),
        KeyBinding::new("cmd-2", ActivateTab2, None),
        KeyBinding::new("cmd-3", ActivateTab3, None),
        KeyBinding::new("cmd-4", ActivateTab4, None),
        KeyBinding::new("cmd-5", ActivateTab5, None),
        KeyBinding::new("cmd-6", ActivateTab6, None),
        KeyBinding::new("cmd-7", ActivateTab7, None),
        KeyBinding::new("cmd-8", ActivateTab8, None),
        KeyBinding::new("cmd-9", ActivateTab9, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ResetZoom, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-o", OpenDirectory, None),
        KeyBinding::new("cmd-shift-o", OpenRepository, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-z", UndoPreview, None),
        KeyBinding::new("cmd-shift-z", RedoPreview, None),
        KeyBinding::new("cmd-f", FindPreview, None),
        // macOS 慣習のショートカット（#485。cmd 付きの未バインドキーはシェルへ流れない
        // ＝ ターミナル入力を奪わない。handle_key の platform 修飾ガードを参照）
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]
}

/// CSI u（kitty keyboard protocol）の送出範囲
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsiUMode {
    /// 修飾付き Enter / Tab / Backspace / Esc のみ CSI u。**全ペインの既定**
    /// （Issue #28: tmux バックエンド限定にしていたため、tmux 無し環境の直接 spawn
    /// ペインで Shift+Enter が素の \r に潰れ Claude Code の改行が死んでいた）。
    /// 修飾付きキーはレガシー形式だと区別不能（Shift+Enter = \r）な一方、
    /// Claude Code は kitty 要求・クエリなしでも CSI u 入力を解釈する
    /// （2026-07-02 v2.1.198 素の PTY で実測）ため、常時 CSI u で送る。
    /// Esc 単押しは素の \e のまま — tmux 3.6 は受信した CSI 27u を内側ペインの
    /// kitty 要求の有無に関係なく素通しするため、CSI u 非対応アプリの入力欄に
    /// 「27u」が文字として挿入される（2026-06-12 実機バグ）
    ModifiedOnly,
    /// Esc 単押しも CSI 27u（アプリ自身が kitty disambiguate を要求済み = 確実に解釈できる）
    Full,
}

/// 修飾キーのエンコード（xterm / kitty 共通: 1 + shift | alt<<1 | ctrl<<2 | super<<3）
pub(crate) fn encode_modifiers(m: &Modifiers) -> u8 {
    1 + (m.shift as u8)
        + ((m.alt as u8) << 1)
        + ((m.control as u8) << 2)
        + ((m.platform as u8) << 3)
}

/// キー入力 → PTY バイト列。`csi_u` は kitty keyboard protocol（disambiguate
/// フラグ。TUI が `CSI > 1 u` で有効化。Claude Code 等が Shift+Enter を
/// 区別するために使う）の送出範囲。UI 層は常に ModifiedOnly 以上を渡す。
/// それ以外のフラグ（REPORT_ALL_KEYS 等）は未対応（必要になったら拡張する）
pub(crate) fn keystroke_to_bytes(ks: &Keystroke, csi_u: CsiUMode) -> Option<Vec<u8>> {
    let mods = encode_modifiers(&ks.modifiers);
    let csi_u_code: Option<u32> = match ks.key.as_str() {
        "escape" if csi_u == CsiUMode::Full || mods > 1 => Some(27),
        "enter" if mods > 1 => Some(13),
        "tab" if mods > 1 => Some(9),
        "backspace" if mods > 1 => Some(127),
        _ => None,
    };
    if let Some(code) = csi_u_code {
        return Some(if mods > 1 {
            format!("\x1b[{code};{mods}u").into_bytes()
        } else {
            format!("\x1b[{code}u").into_bytes()
        });
    }
    // Ctrl+英字 → C0 制御コード
    if ks.modifiers.control {
        let mut chars = ks.key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_lowercase() as u8) & 0x1f]);
            }
        }
    }
    // 機能キー。修飾付きは xterm 標準の CSI 1;mod X / CSI n;mod ~ 形式
    let csi_letter = |letter: char| -> Vec<u8> {
        if mods > 1 {
            format!("\x1b[1;{mods}{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if mods > 1 {
            format!("\x1b[{n};{mods}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes: Vec<u8> = match ks.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "delete" => csi_tilde(3),
        _ => {
            let ch = ks.key_char.as_ref()?;
            if ch.is_empty() {
                return None;
            }
            return Some(ch.as_bytes().to_vec());
        }
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.into(),
            key_char: None,
        }
    }
    fn ks_char(key: &str, ch: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.into(),
            key_char: Some(ch.into()),
        }
    }
    fn ks_ctrl(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: None,
        }
    }
    fn ks_shift(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: None,
        }
    }

    /// 既定モード（ModifiedOnly = 全ペイン共通）でのバイト変換
    fn keystroke_to_bytes_default(ks: &Keystroke) -> Option<Vec<u8>> {
        keystroke_to_bytes(ks, CsiUMode::ModifiedOnly)
    }

    #[test]
    fn 特殊キーは正しいバイト列を送る() {
        assert_eq!(
            keystroke_to_bytes_default(&ks("backspace")),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("enter")),
            Some(b"\r".to_vec())
        );
        assert_eq!(keystroke_to_bytes_default(&ks("tab")), Some(b"\t".to_vec()));
        assert_eq!(
            keystroke_to_bytes_default(&ks("escape")),
            Some(b"\x1b".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("up")),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("down")),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("right")),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("left")),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("home")),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("end")),
            Some(b"\x1b[F".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("pageup")),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("pagedown")),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks("delete")),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn 修飾付き機能キーはxterm形式で送る() {
        assert_eq!(
            keystroke_to_bytes_default(&ks_shift("up")),
            Some(b"\x1b[1;2A".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks_shift("delete")),
            Some(b"\x1b[3;2~".to_vec())
        );
        // Shift+Enter は xterm 形式に修飾表現が無いため CSI u で送る
        // （既定モードのアサートは「バックエンドペインは…」テスト側）
    }

    #[test]
    fn disambiguate有効時は修飾付きenterをcsi_uで送る() {
        assert_eq!(
            keystroke_to_bytes(&ks_shift("enter"), CsiUMode::Full),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_ctrl("enter"), CsiUMode::Full),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("tab"), CsiUMode::Full),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("backspace"), CsiUMode::Full),
            Some(b"\x1b[127;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("escape"), CsiUMode::Full),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("enter"), CsiUMode::Full),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("tab"), CsiUMode::Full),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("backspace"), CsiUMode::Full),
            Some(b"\x7f".to_vec())
        );
    }

    #[test]
    fn 既定モードはesc単押しを素のescで送り修飾付きキーはcsi_uで送る() {
        assert_eq!(
            keystroke_to_bytes(&ks("escape"), CsiUMode::ModifiedOnly),
            Some(b"\x1b".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("enter"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("tab"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("escape"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[27;2u".to_vec())
        );
    }

    #[test]
    fn ctrl英字はc0制御コードを送る() {
        assert_eq!(keystroke_to_bytes_default(&ks_ctrl("a")), Some(vec![0x01]));
        assert_eq!(keystroke_to_bytes_default(&ks_ctrl("c")), Some(vec![0x03]));
        assert_eq!(keystroke_to_bytes_default(&ks_ctrl("u")), Some(vec![0x15]));
        assert_eq!(keystroke_to_bytes_default(&ks_ctrl("z")), Some(vec![0x1a]));
    }

    #[test]
    fn 印字可能文字はkey_charをそのまま送る() {
        assert_eq!(
            keystroke_to_bytes_default(&ks_char("a", "a")),
            Some(b"a".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks_char("space", " ")),
            Some(b" ".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks_char("a", "あ")),
            Some("あ".as_bytes().to_vec())
        );
        assert_eq!(keystroke_to_bytes_default(&ks("f5")), None);
    }

    #[test]
    fn imeのrange先頭は擬似ドキュメント内へ解釈する() {
        use crate::clamp_ime_range_start;
        assert_eq!(clamp_ime_range_start(0, 4, None), 0);
        assert_eq!(clamp_ime_range_start(4, 4, Some(&(2..4))), 4);
        assert_eq!(clamp_ime_range_start(100, 4, Some(&(2..4))), 2);
        assert_eq!(clamp_ime_range_start(100, 4, None), 4);
    }

    /// #103: cmd-q → Quit のバインドが存在し、コンテキスト述語なし
    /// （= フォーカス喪失で context stack が空でもマッチする）であることを固定する。
    /// 発火側（グローバル on_action）はセルフテスト最終項目が e2e で検証する
    #[test]
    fn cmd_qはコンテキスト述語なしでquitにバインドされている() {
        let bindings = key_bindings();
        let quit: Vec<_> = bindings
            .iter()
            .filter(|b| b.action().name() == "tako::Quit")
            .collect();
        assert_eq!(quit.len(), 1, "Quit のバインドはちょうど 1 個");
        let ks = quit[0].keystrokes();
        assert_eq!(ks.len(), 1, "単発キーストローク");
        assert_eq!(ks[0].inner().key, "q");
        assert!(ks[0].inner().modifiers.platform, "cmd 修飾");
        assert!(
            quit[0].predicate().is_none(),
            "コンテキスト述語なし（どのフォーカス状態でもマッチ）"
        );
    }
}

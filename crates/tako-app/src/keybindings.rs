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
    let mut bindings = vec![
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
    ];
    bindings.extend(clipboard_bindings_for_platform());
    bindings
}

/// macOS 以外（Windows / Linux）のクリップボード操作バインド（#467）
///
/// 上の一覧の `cmd-` は GPUI では **platform 修飾**に解決され、Windows では
/// Win キーになる（`gpui_windows` が VK_LWIN / VK_RWIN を platform へ写す）。
/// Win+V は OS のクリップボード履歴が奪うため、`cmd-v` だけだと tako の
/// ペースト経路（PasteClipboard）へ Windows から到達する手段が無くなる。
/// そこで OS 慣習のキーを追加で張り、入口を用意する。
///
/// トレードオフ: `ctrl-v` を張ると Ctrl+V の C0 制御コード 0x16（readline /
/// vim の逐語入力）は PTY へ届かなくなる。GPUI はキーバインドのアクションを
/// `on_key_down` より先に発火し、バブルフェーズでアクションが既定で伝播を
/// 止めるため（gpui `window.rs` の dispatch_key_event / dispatch_action_on_node）。
/// Windows Terminal も既定で Ctrl+V をペーストに割り当てており、
/// 逐語入力より「ペーストできない」ほうが実害が大きいのでこの配分を採る。
/// Ctrl+C（SIGINT）等は修飾が一致しないので影響しない
/// （`ctrl-shift-c` は Ctrl+C とは別のキーストローク）。
#[cfg(not(target_os = "macos"))]
fn clipboard_bindings_for_platform() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-v", PasteClipboard, None),
        // Linux ターミナル慣習。Ctrl+V を逐語入力へ戻したくなっても残る退路
        KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
        // 旧来の Windows 慣習。現状 keystroke_to_bytes は insert を送らないので
        // ターミナル入力を奪わない
        KeyBinding::new("shift-insert", PasteClipboard, None),
        KeyBinding::new("ctrl-shift-c", CopySelection, None),
    ]
}

/// macOS は `cmd-` が本来の Command キーに解決されるため追加は不要
#[cfg(target_os = "macos")]
fn clipboard_bindings_for_platform() -> Vec<KeyBinding> {
    Vec::new()
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
        _ => return printable_to_bytes(ks, ALT_IS_META),
    };
    Some(bytes)
}

/// Alt（Option）を meta 修飾として扱い、印字文字に ESC を前置するか（#575）
///
/// macOS の Option は**文字入力の一部**（Option+V = 「√」、Option+8 = 「•」）なので
/// false。ESC を前置すると特殊文字入力が丸ごと壊れる。
/// Windows / Linux の Alt は文字を生まないため、ターミナルの慣習どおり
/// meta = ESC 前置で送る（xterm の metaSendsEscape。Windows Terminal も同じ）。
/// Claude Code の Alt+V（クリップボード画像の貼り付け）はこの経路を要求する。
const ALT_IS_META: bool = !cfg!(target_os = "macos");

/// 印字文字キー → PTY バイト列。`alt_is_meta` は [`ALT_IS_META`]
/// （テストが macOS 相当 / 非 macOS 相当の両方を同じマシンで検証できるよう引数にしてある）
fn printable_to_bytes(ks: &Keystroke, alt_is_meta: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    // **素の Alt のみ** meta 扱いにする。Windows では AltGr が Ctrl+Alt として届き、
    // 欧州配列の @ / { 等はこの経路で key_char に入るため、ESC を前置すると
    // 入力そのものが壊れる（platform 修飾は handle_key が手前で弾いている）
    let meta = alt_is_meta && m.alt && !m.control;
    let ch = match ks.key_char.as_deref().filter(|s| !s.is_empty()) {
        Some(ch) => ch.to_string(),
        // Alt 単独押下では key_char が None になる（Windows の標準配列は Alt のみの
        // 修飾組み合わせを未定義にしており、GPUI が呼ぶ ToUnicode が 0 を返す）。
        // meta として送るときだけ key から組み立てる
        None if meta => printable_char_from_key(ks)?,
        None => return None,
    };
    Some(if meta {
        let mut bytes = Vec::with_capacity(1 + ch.len());
        bytes.push(0x1b);
        bytes.extend_from_slice(ch.as_bytes());
        bytes
    } else {
        ch.into_bytes()
    })
}

/// `key`（"v" / "@" 等の 1 文字キー名）から送出文字を作る。
/// 機能キー（"f5"）・名前つきキー（"space"）・制御文字は None
fn printable_char_from_key(ks: &Keystroke) -> Option<String> {
    let mut chars = ks.key.chars();
    let c = chars.next()?;
    if chars.next().is_some() || c.is_control() {
        return None;
    }
    // Shift+英字は key が小文字のまま届く（GPUI Windows は英字を shift 変換しない。
    // 数字・記号は変換済みで shift が下ろされるので、ここでは何も変わらない）
    Some(if ks.modifiers.shift {
        c.to_ascii_uppercase().to_string()
    } else {
        c.to_string()
    })
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

    /// 修飾を明示したキーストローク（`key_char` は実機で来る形をそのまま渡す）
    fn ks_mods(key: &str, key_char: Option<&str>, mods: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: mods,
            key: key.into(),
            key_char: key_char.map(Into::into),
        }
    }
    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::default()
        }
    }
    fn alt_shift() -> Modifiers {
        Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::default()
        }
    }
    /// AltGr（Windows では Ctrl+Alt として届く）
    fn altgr() -> Modifiers {
        Modifiers {
            alt: true,
            control: true,
            ..Modifiers::default()
        }
    }

    /// #575: 非 macOS では Alt = meta。印字文字に ESC を前置して PTY へ送る
    /// （Claude Code の Alt+V = クリップボード画像貼り付けがこの形を要求する）
    #[test]
    fn alt付き印字文字は非macosでescを前置する() {
        // Linux 等、key_char に文字が入って届く形
        assert_eq!(
            printable_to_bytes(&ks_mods("v", Some("v"), alt()), true),
            Some(b"\x1bv".to_vec())
        );
        // Windows 実機の形: Alt 単独では ToUnicode が文字を返さず key_char が None。
        // ここで key へフォールバックしないと従来どおり何も送れない（= 無反応の再現）
        assert_eq!(
            printable_to_bytes(&ks_mods("v", None, alt()), true),
            Some(b"\x1bv".to_vec())
        );
        // Shift+Alt+英字。GPUI Windows は英字を shift 変換しないので key は小文字で届く
        assert_eq!(
            printable_to_bytes(&ks_mods("v", None, alt_shift()), true),
            Some(b"\x1bV".to_vec())
        );
        // 記号は GPUI 側で shift 変換済み（shift が下ろされる）ため key をそのまま使う
        assert_eq!(
            printable_to_bytes(&ks_mods("@", None, alt()), true),
            Some(b"\x1b@".to_vec())
        );
        // 空の key_char は「文字が無い」と同じ扱い（key へフォールバックする）
        assert_eq!(
            printable_to_bytes(&ks_mods("v", Some(""), alt()), true),
            Some(b"\x1bv".to_vec())
        );
    }

    /// alt なしの印字文字は従来どおり（ESC を付けない）
    #[test]
    fn alt無しの印字文字はkey_charをそのまま送る() {
        assert_eq!(
            printable_to_bytes(&ks_mods("v", Some("v"), Modifiers::default()), true),
            Some(b"v".to_vec())
        );
        assert_eq!(
            printable_to_bytes(&ks_mods("a", Some("あ"), Modifiers::default()), true),
            Some("あ".as_bytes().to_vec())
        );
        // key へのフォールバックは meta のときだけ（key_char 無し = 送るものが無い）
        assert_eq!(
            printable_to_bytes(&ks_mods("v", None, Modifiers::default()), true),
            None
        );
        assert_eq!(
            printable_to_bytes(&ks_mods("v", Some(""), Modifiers::default()), true),
            None
        );
    }

    /// AltGr（Ctrl+Alt）は欧州配列の @ / { 等の**文字入力**。ESC を前置すると壊れる
    #[test]
    fn altgrの文字はescを前置しない() {
        assert_eq!(
            printable_to_bytes(&ks_mods("2", Some("@"), altgr()), true),
            Some(b"@".to_vec())
        );
        assert_eq!(
            printable_to_bytes(&ks_mods("7", Some("{"), altgr()), true),
            Some(b"{".to_vec())
        );
        // AltGr で文字が出ないキーは従来どおり何も送らない（key へ落ちない）
        assert_eq!(printable_to_bytes(&ks_mods("v", None, altgr()), true), None);
    }

    /// macOS の Option は文字入力の一部（Option+V = 「√」）。挙動を変えてはならない
    #[test]
    fn macos相当ではaltを素通しする() {
        assert_eq!(
            printable_to_bytes(&ks_mods("v", Some("√"), alt()), false),
            Some("√".as_bytes().to_vec())
        );
        // key へのフォールバックもしない（修正前と同じく None）
        assert_eq!(printable_to_bytes(&ks_mods("v", None, alt()), false), None);
        assert_eq!(
            printable_to_bytes(&ks_mods("v", None, alt_shift()), false),
            None
        );
    }

    /// 機能キー・名前つきキーは meta でも文字を作らない（"\x1bf5" のような化けを出さない）
    #[test]
    fn 機能キーはmetaでも文字にしない() {
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("f5", None, alt())),
            None
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("space", None, alt())),
            None
        );
    }

    /// #575: 実プラットフォームの既定（`ALT_IS_META`）が `keystroke_to_bytes` に
    /// 配線されていることを固定する。非 macOS で Alt+V が meta にならなければ
    /// Claude Code の画像ペーストは届かない
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosではalt_vがmetaで送られる() {
        const { assert!(ALT_IS_META) };
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("v", None, alt())),
            Some(b"\x1bv".to_vec()),
            "Alt+V が ESC 前置で送られない（Claude Code の画像ペーストが無反応になる）"
        );
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("v", Some("v"), alt())),
            Some(b"\x1bv".to_vec())
        );
        // AltGr は素通し（回帰防止）
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("2", Some("@"), altgr())),
            Some(b"@".to_vec())
        );
    }

    /// macOS 側は Option の文字入力（√）がそのまま PTY へ届く
    #[cfg(target_os = "macos")]
    #[test]
    fn macosではaltにescを前置しない() {
        const { assert!(!ALT_IS_META) };
        assert_eq!(
            keystroke_to_bytes_default(&ks_mods("v", Some("√"), alt())),
            Some("√".as_bytes().to_vec())
        );
        assert_eq!(keystroke_to_bytes_default(&ks_mods("v", None, alt())), None);
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

    /// 指定アクションのバインドのうち、platform（cmd / Win）修飾を使わないものを集める
    fn 非platform修飾のバインド(action: &str) -> Vec<Vec<Keystroke>> {
        key_bindings()
            .iter()
            .filter(|b| b.action().name() == action)
            .map(|b| {
                b.keystrokes()
                    .iter()
                    .map(|k| k.inner().clone())
                    .collect::<Vec<_>>()
            })
            .filter(|ks| !ks.iter().any(|k| k.modifiers.platform))
            .collect()
    }

    /// #467: Windows では GPUI の `cmd` = platform 修飾が Win キーに解決され、
    /// Win+V は OS のクリップボード履歴に奪われる。platform 修飾を使わない
    /// ペースト経路が最低 1 本残っていることを固定する
    /// （`clipboard_bindings_for_platform` を消すとこのテストが落ちる）
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosではplatform修飾なしのペーストバインドが存在する() {
        let paste = 非platform修飾のバインド("tako::PasteClipboard");
        assert!(
            !paste.is_empty(),
            "PasteClipboard に platform 修飾なしのバインドが 1 本も無い（Windows から到達不能）"
        );
        // OS 慣習の 3 経路が揃っていること
        let 単発 =
            |ks: &Vec<Keystroke>| -> Option<Keystroke> { (ks.len() == 1).then(|| ks[0].clone()) };
        let has = |key: &str, ctrl: bool, shift: bool| {
            paste.iter().filter_map(単発).any(|k| {
                k.key == key
                    && k.modifiers.control == ctrl
                    && k.modifiers.shift == shift
                    && !k.modifiers.alt
            })
        };
        assert!(has("v", true, false), "ctrl-v が無い");
        assert!(has("v", true, true), "ctrl-shift-v が無い");
        assert!(has("insert", false, true), "shift-insert が無い");

        // コピーも同様に platform 修飾なしの経路が要る（ctrl-shift-c）。
        // Ctrl+C 単独は SIGINT のままでなければならないので shift 必須
        let copy = 非platform修飾のバインド("tako::CopySelection");
        let copy_keys: Vec<Keystroke> = copy.iter().filter_map(単発).collect();
        assert!(
            copy_keys
                .iter()
                .any(|k| k.key == "c" && k.modifiers.control && k.modifiers.shift),
            "ctrl-shift-c が無い"
        );
        assert!(
            !copy_keys
                .iter()
                .any(|k| k.key == "c" && k.modifiers.control && !k.modifiers.shift),
            "ctrl-c を奪うと SIGINT が送れなくなる"
        );
    }

    /// macOS 側は従来どおり cmd- のみ（#467 の追加バインドが漏れ出していない）
    #[cfg(target_os = "macos")]
    #[test]
    fn macosではクリップボード操作はcmd修飾のみ() {
        assert!(
            非platform修飾のバインド("tako::PasteClipboard").is_empty(),
            "macOS に非 platform 修飾のペーストバインドが混入している"
        );
        assert!(
            非platform修飾のバインド("tako::CopySelection").is_empty(),
            "macOS に非 platform 修飾のコピーバインドが混入している"
        );
    }
}

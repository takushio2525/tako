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
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ];
    bindings.extend(macos_only_bindings());
    bindings.extend(platform_bindings());
    bindings
}

/// macOS 固有の概念に張るバインド（#485）。**macOS でのみ張る**（#602）
///
/// `cmd-` は GPUI では platform 修飾で、Windows では Win キーへ解決される。
/// Win+H（音声入力）・Win+M（最小化）は OS が奪うが、**Win+Alt+H は Windows の
/// 予約ショートカット一覧に無い**（Win+Alt+ は Game Bar の B / G / R / PrtScn 等が
/// 中心）。アプリまで届いた場合、`HideOthers` → `gpui_windows` の `hide_other_apps`
/// が `unimplemented!()` のため **panic ＝ アプリごと abort し、器の無いペインは
/// 全滅する**。実キーで届くかは未実測だが、届いたときの被害が全損なので経路ごと塞ぐ
/// （`unhide_other_apps` も同じ地雷。ただし `ShowAllApps` はバインドが無く、
/// Windows にはメニューバーも無いので到達経路が無い）。
///
/// 「アプリを隠す」「他を隠す」「最小化」はいずれも macOS の概念で、Windows 版に
/// 対応するアクションが無い（最小化は #584 のウィンドウコントロールと Win+Down が担う）。
/// ハンドラ側で防ぐのではなく**バインドを張らない**ことで経路ごと塞ぐ
#[cfg(target_os = "macos")]
fn macos_only_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
    ]
}

/// macOS 以外では張らない（[`macos_only_bindings`] の doc を参照）
#[cfg(not(target_os = "macos"))]
fn macos_only_bindings() -> Vec<KeyBinding> {
    Vec::new()
}

/// コマンドパレットの項目 ID から、そこに併記するショートカットの表示文字列を引く（#648）
///
/// Windows には GPUI のメニューバーが無く（#602）、ドキュメントを読まないと
/// `Ctrl+Shift+D`（分割）のような **`cmd-` から機械的に読み替えられないキー**を
/// 知る手段が無かった。パレットに併記して発見できるようにする。
///
/// 表示は必ず [`key_bindings`] から導出する。手書きの一覧を別に持つと、
/// プラットフォーム差や将来のキー変更で確実に食い違うため
/// （`パレットのショートカット表示はバインド表と一致する` テストが番犬）
pub(crate) fn palette_shortcut(command_id: &str) -> Option<String> {
    shortcut_hint(action_for_palette_command(command_id)?)
}

/// パレット項目 ID → アクション名。ショートカットを持たない項目は `None`
/// （テーマ切替・パネル系はキーバインドが無く、パレットと CLI / MCP が入口）
fn action_for_palette_command(command_id: &str) -> Option<&'static str> {
    match command_id {
        "new-tab" => Some("tako::NewTab"),
        "split-right" => Some("tako::SplitRight"),
        "split-down" => Some("tako::SplitDown"),
        "toggle-files" => Some("tako::ToggleSidebar"),
        _ => None,
    }
}

/// アクション名から、**このプラットフォームで実際に届く**バインドの表示文字列を作る
///
/// 非 macOS で `cmd-` のバインドを案内してはいけない。GPUI の `cmd` は platform
/// 修飾で Windows では Win キーへ解決され、シェルが先に奪うので届かない（#585）。
/// ＝ 案内に出すと「書いてあるのに効かない」という最悪の体験になる
fn shortcut_hint(action: &str) -> Option<String> {
    let binding = key_bindings().into_iter().find(|b| {
        b.action().name() == action
            && !(cfg!(not(target_os = "macos"))
                && b.keystrokes().iter().any(|k| k.inner().modifiers.platform))
    })?;
    let hint = binding
        .keystrokes()
        .iter()
        .map(|k| format_keystroke(k.inner()))
        .collect::<Vec<_>>()
        .join(" ");
    (!hint.is_empty()).then_some(hint)
}

/// 1 打鍵をその OS の慣習で表記する（macOS は記号、Windows / Linux は語）
fn format_keystroke(k: &Keystroke) -> String {
    let m = k.modifiers;
    let key = format_key(&k.key);
    if cfg!(target_os = "macos") {
        // macOS のメニュー表記順（⌃⌥⇧⌘）
        let mut s = String::new();
        if m.control {
            s.push('\u{2303}');
        }
        if m.alt {
            s.push('\u{2325}');
        }
        if m.shift {
            s.push('\u{21e7}');
        }
        if m.platform {
            s.push('\u{2318}');
        }
        s.push_str(&key);
        s
    } else {
        let mut parts: Vec<&str> = Vec::new();
        if m.control {
            parts.push("Ctrl");
        }
        if m.alt {
            parts.push("Alt");
        }
        if m.shift {
            parts.push("Shift");
        }
        let mut s = parts.join("+");
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(&key);
        s
    }
}

/// キー名の表示形。英字 1 文字は大文字、矢印は記号、それ以外は先頭大文字
fn format_key(key: &str) -> String {
    match key {
        "left" => "\u{2190}".into(),
        "right" => "\u{2192}".into(),
        "up" => "\u{2191}".into(),
        "down" => "\u{2193}".into(),
        "tab" => "Tab".into(),
        "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "insert" => "Insert".into(),
        _ if key.len() == 1 => key.to_uppercase(),
        // f11 → F11
        _ => {
            let mut c = key.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

/// macOS 以外（Windows / Linux）向けの追加バインド（#467 / #585）
///
/// 上の一覧の `cmd-` は GPUI では **platform 修飾**に解決され、Windows では
/// Win キーになる（`gpui_windows::current_modifiers` が VK_LWIN / VK_RWIN を
/// platform へ写す）。Win+T / Win+D / Win+V / Win+K などはシェルが先に奪うため、
/// 45 本すべてがアプリまで届かない ＝ **Windows ではショートカットが全滅する**
/// （#585 の症状「新しいタブが開けない」）。そこで OS 慣習のキーを追加で張る。
///
/// ## 割当の原則（#585 の 3 分類）
///
/// - **`ctrl-<英字>` 単独は奪わない**。C0 制御コード（Ctrl+C = SIGINT、Ctrl+A = 行頭、
///   Ctrl+D = EOF、Ctrl+Z = SIGTSTP、Ctrl+S = XOFF…）としてシェル / TUI が使う。
///   例外は `ctrl-v` だけ（#467 の意図的なトレードオフ。下記）
/// - ターミナル入力と衝突するものは **`ctrl-shift-` へ逃がす**。[`keystroke_to_bytes`]
///   は `ctrl+shift+<英字>` を `ctrl+<英字>` と同じ C0 バイトへ潰している（shift を
///   見ない）ため、奪っても**固有の入力手段は失われない**
/// - **`alt-` は矢印キーだけ**に使う。#575 で素の Alt+印字文字を meta（ESC 前置）で
///   PTY へ送るようにしたため、`alt-<文字>` を奪うとエージェント CLI の Alt+V
///   （クリップボード画像の貼り付け）などが死ぬ。Windows Terminal 由来の
///   Alt+Shift+D（ペイン複製）を分割に採らなかったのもこの理由
/// - **shift と記号 / 数字を組み合わせない**。GPUI Windows は shift 付きの記号を
///   「シフト後の文字 + shift 無し」へ正規化して届ける（`get_keystroke_key`）が、
///   `KeyBinding::new` は `DummyKeyboardMapper` なので書いたままを保持する。
///   よって `ctrl-shift-]` のようなバインドは**一致しない**（配列依存でもある）。
///   拡大は正規化後の字面 `ctrl-+` で書く
///
/// ## ペイン分割に **Ctrl+D 単独を使わない**理由（#602。ユーザーからの名指し要望）
///
/// macOS の Cmd+D をそのまま読み替えると Ctrl+D になるが、Ctrl+D は
/// **C0 制御コード 0x04 = EOF** で、[`keystroke_to_bytes`] は今も PTY へ 0x04 を
/// 送っている（`ctrl_dはeofなので分割に使えない` テストが実際の変換で固定）。
/// tako の主用途である**エージェント CLI ほど EOF を使う**（Claude Code / codex の
/// 終了、Python・Node の REPL 終了、`cat > file` の入力終端、WSL の bash ログアウト）。
/// GPUI はキーバインドのアクションを `on_key_down` より先に発火して伝播を止めるため、
/// 奪うと**代替手段の無いまま EOF が送れなくなる**。
///
/// Windows Terminal も分割に Ctrl+D は使わない（Alt+Shift+D）。よって分割は
/// 上の原則どおり `ctrl-shift-d`（= Cmd+D と同じ D。右分割）/ `ctrl-shift-e`
/// （下分割）に置く。Ctrl+Shift+D は 0x04 へ潰れる ＝ Ctrl+D 側の EOF は無傷。
///
/// 慣習の出典は Windows Terminal / VS Code / kitty / ブラウザ。キー 1 本ごとの
/// 根拠は #585 の対応表、今回の追加分（Quit）と削除分は #602。
#[cfg(not(target_os = "macos"))]
fn platform_bindings() -> Vec<KeyBinding> {
    vec![
        // --- タブ（Windows Terminal / kitty / ブラウザ）---
        KeyBinding::new("ctrl-shift-t", NewTab, None),
        KeyBinding::new("ctrl-shift-w", ClosePane, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        // Ctrl+数字（ブラウザ / VS Code のタブ切替）。Ctrl+<数字> は現状 PTY へ
        // 1 バイトも送っていない（ToUnicode が文字を返さず key_char が None）ので
        // 奪っても失うものが無い
        KeyBinding::new("ctrl-1", ActivateTab1, None),
        KeyBinding::new("ctrl-2", ActivateTab2, None),
        KeyBinding::new("ctrl-3", ActivateTab3, None),
        KeyBinding::new("ctrl-4", ActivateTab4, None),
        KeyBinding::new("ctrl-5", ActivateTab5, None),
        KeyBinding::new("ctrl-6", ActivateTab6, None),
        KeyBinding::new("ctrl-7", ActivateTab7, None),
        KeyBinding::new("ctrl-8", ActivateTab8, None),
        KeyBinding::new("ctrl-9", ActivateTab9, None),
        KeyBinding::new("ctrl-shift-n", NewWindow, None),
        // アプリの終了（#602。#585 では「Windows は Alt+F4 と閉じるボタンが慣習」として
        // 見送ったが、その前提が実測で崩れた: `TakoApp::handle_window_close` は最後の
        // 1 枚でもプロセスを終了させず（macOS の Dock 復帰前提）、`gpui_windows` も
        // `close_one_window` の「最後の 1 枚だったか」を捨てて `PostQuitMessage` を
        // 出さない。しかも `on_reopen` は Windows では**一度も発火しない**（コールバックを
        // 保存する側だけがあり呼び出し側が無い）。結果 Alt+F4 / ✕ はウィンドウ 0 枚の
        // まま復帰も終了もできないプロセスを残す ＝ **Windows には正規の終了手段が無い**。
        // Ctrl+Q は XON（フロー制御）なので shift 段へ逃がす）
        KeyBinding::new("ctrl-shift-q", Quit, None),
        // --- ペイン ---
        // 分割は D = 右（macOS の cmd-d と字面一致。Windows Terminal の
        // Ctrl+Shift+D = ペイン複製とも一致）、E = 下（shift 段が埋まるため
        // 隣接キーへ逃がす。Terminator / Tilix も分割に Ctrl+Shift+E を使う）
        KeyBinding::new("ctrl-shift-d", SplitRight, None),
        KeyBinding::new("ctrl-shift-e", SplitDown, None),
        // フォーカス移動 = Alt+矢印、リサイズ = Alt+Shift+矢印（Windows Terminal）。
        // Ctrl+矢印（readline の単語移動 \x1b[1;5C）は奪わない
        KeyBinding::new("alt-left", FocusLeft, None),
        KeyBinding::new("alt-right", FocusRight, None),
        KeyBinding::new("alt-up", FocusUp, None),
        KeyBinding::new("alt-down", FocusDown, None),
        KeyBinding::new("alt-shift-right", WidenPane, None),
        KeyBinding::new("alt-shift-left", NarrowPane, None),
        KeyBinding::new("alt-shift-down", TallenPane, None),
        KeyBinding::new("alt-shift-up", ShortenPane, None),
        // --- クリップボード（#467 で先行実装。Ctrl+V のみ単独 ctrl の例外）---
        // トレードオフ: Ctrl+V の C0 制御コード 0x16（readline / vim の逐語入力）は
        // PTY へ届かなくなる。GPUI はキーバインドのアクションを `on_key_down` より
        // 先に発火し、バブルフェーズでアクションが既定で伝播を止めるため
        // （gpui `window.rs` の dispatch_key_event / dispatch_action_on_node）。
        // Windows Terminal も既定で Ctrl+V をペーストに割り当てており、逐語入力より
        // 「ペーストできない」ほうが実害が大きいのでこの配分を採る
        KeyBinding::new("ctrl-v", PasteClipboard, None),
        // Linux ターミナル慣習。Ctrl+V を逐語入力へ戻したくなっても残る退路
        KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
        // 旧来の Windows 慣習。現状 keystroke_to_bytes は insert を送らないので
        // ターミナル入力を奪わない
        KeyBinding::new("shift-insert", PasteClipboard, None),
        KeyBinding::new("ctrl-shift-c", CopySelection, None),
        // --- 表示（ブラウザ / Windows Terminal）---
        // Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 も ToUnicode が文字を返さないため
        // PTY へは何も送っていない。`ctrl-+` は JP 配列の Ctrl+Shift+- にも一致する
        // （GPUI がシフト後の字面 "+" / "=" へ正規化して届けるため）
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ResetZoom, None),
        // 全画面は Windows 共通の F11。F キーは keystroke_to_bytes が PTY へ
        // 送らない（tako は F1〜F12 を未実装）ので奪っても失うものが無い
        KeyBinding::new("f11", ToggleFullScreen, None),
        KeyBinding::new("ctrl-shift-b", ToggleSidebar, None),
        // --- 編集・プレビュー ---
        KeyBinding::new("ctrl-shift-s", SavePreview, None),
        KeyBinding::new("ctrl-shift-a", SelectAll, None),
        // Windows の取り消し = Ctrl+Z / やり直し = Ctrl+Y を 1 段持ち上げた形。
        // Ctrl+Shift+Z が「やり直し」でない点だけ Windows 慣習と食い違うが、
        // Ctrl+Z（SIGTSTP）を奪えない以上、取り消し側が shift 段を使う
        KeyBinding::new("ctrl-shift-z", UndoPreview, None),
        KeyBinding::new("ctrl-shift-y", RedoPreview, None),
        KeyBinding::new("ctrl-shift-f", FindPreview, None),
        // --- 開く・設定 ---
        KeyBinding::new("ctrl-shift-o", OpenDirectory, None),
        // O は上で使うため R = Repository。Windows には GPUI のメニューバーが
        // 無く、この 2 つはキーとパレット以外の入口が無い
        KeyBinding::new("ctrl-shift-r", OpenRepository, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        // --- コマンドパレット（VS Code / Windows Terminal は Ctrl+Shift+P。
        // K は macOS の cmd-k と字面を合わせた退路）---
        KeyBinding::new("ctrl-shift-p", OpenCommandPalette, None),
        KeyBinding::new("ctrl-shift-k", OpenCommandPalette, None),
    ]
}

/// macOS は `cmd-` が本来の Command キーに解決されるため追加は不要
#[cfg(target_os = "macos")]
fn platform_bindings() -> Vec<KeyBinding> {
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
        // #602 で非 macOS に ctrl-shift-q を足したため、cmd 側だけを見る
        let quit: Vec<_> = bindings
            .iter()
            .filter(|b| b.action().name() == "tako::Quit")
            .filter(|b| b.keystrokes().iter().all(|k| k.inner().modifiers.platform))
            .collect();
        assert_eq!(quit.len(), 1, "cmd 修飾の Quit バインドはちょうど 1 個");
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
    /// （`platform_bindings` を消すとこのテストが落ちる）
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

    /// #585: 指定のキーストローク（"ctrl-shift-t" 等）に一致するバインドのアクション名。
    ///
    /// GPUI の一致判定は `Keystroke::should_match` の
    /// 「`target.inner.modifiers == self.modifiers && target.inner.key == self.key`」
    /// （IME 由来の key_char 経路を除く完全一致）なので、ここも inner の完全一致で
    /// 照合する。`KeyBinding::new` は `DummyKeyboardMapper` を使う ＝ inner は
    /// 書いた文字列の parse 結果そのままなので、この照合は実機の解決と同じ意味になる
    fn 一致するアクション(spec: &str) -> Vec<String> {
        let want: Vec<Keystroke> = spec
            .split_whitespace()
            .map(|s| Keystroke::parse(s).expect("キーストロークとして解釈できる"))
            .collect();
        key_bindings()
            .iter()
            .filter(|b| {
                b.keystrokes().len() == want.len()
                    && b.keystrokes().iter().zip(&want).all(|(got, want)| {
                        let got = got.inner();
                        got.key == want.key && got.modifiers == want.modifiers
                    })
            })
            .map(|b| b.action().name().to_string())
            .collect()
    }

    /// #585 の対応表（Windows / Linux 側の割当）。表に書いた割当はすべてここに載せる
    #[cfg(not(target_os = "macos"))]
    fn 非macos割当表() -> Vec<(&'static str, &'static str)> {
        vec![
            // タブ
            ("ctrl-shift-t", "tako::NewTab"),
            ("ctrl-shift-w", "tako::ClosePane"),
            ("ctrl-tab", "tako::NextTab"),
            ("ctrl-shift-tab", "tako::PrevTab"),
            ("ctrl-1", "tako::ActivateTab1"),
            ("ctrl-2", "tako::ActivateTab2"),
            ("ctrl-3", "tako::ActivateTab3"),
            ("ctrl-4", "tako::ActivateTab4"),
            ("ctrl-5", "tako::ActivateTab5"),
            ("ctrl-6", "tako::ActivateTab6"),
            ("ctrl-7", "tako::ActivateTab7"),
            ("ctrl-8", "tako::ActivateTab8"),
            ("ctrl-9", "tako::ActivateTab9"),
            ("ctrl-shift-n", "tako::NewWindow"),
            ("ctrl-shift-q", "tako::Quit"),
            // ペイン
            ("ctrl-shift-d", "tako::SplitRight"),
            ("ctrl-shift-e", "tako::SplitDown"),
            ("alt-left", "tako::FocusLeft"),
            ("alt-right", "tako::FocusRight"),
            ("alt-up", "tako::FocusUp"),
            ("alt-down", "tako::FocusDown"),
            ("alt-shift-right", "tako::WidenPane"),
            ("alt-shift-left", "tako::NarrowPane"),
            ("alt-shift-down", "tako::TallenPane"),
            ("alt-shift-up", "tako::ShortenPane"),
            // クリップボード（#467）
            ("ctrl-v", "tako::PasteClipboard"),
            ("ctrl-shift-v", "tako::PasteClipboard"),
            ("shift-insert", "tako::PasteClipboard"),
            ("ctrl-shift-c", "tako::CopySelection"),
            // 表示
            ("ctrl-=", "tako::ZoomIn"),
            ("ctrl-+", "tako::ZoomIn"),
            ("ctrl--", "tako::ZoomOut"),
            ("ctrl-0", "tako::ResetZoom"),
            ("f11", "tako::ToggleFullScreen"),
            ("ctrl-shift-b", "tako::ToggleSidebar"),
            // 編集・プレビュー
            ("ctrl-shift-s", "tako::SavePreview"),
            ("ctrl-shift-a", "tako::SelectAll"),
            ("ctrl-shift-z", "tako::UndoPreview"),
            ("ctrl-shift-y", "tako::RedoPreview"),
            ("ctrl-shift-f", "tako::FindPreview"),
            // 開く・設定
            ("ctrl-shift-o", "tako::OpenDirectory"),
            ("ctrl-shift-r", "tako::OpenRepository"),
            ("ctrl-,", "tako::OpenSettings"),
            // コマンドパレット
            ("ctrl-shift-p", "tako::OpenCommandPalette"),
            ("ctrl-shift-k", "tako::OpenCommandPalette"),
        ]
    }

    /// #585: Windows では cmd-（= Win キー）が OS に奪われて全滅するため、
    /// 対応表どおりの Windows 慣習バインドが解決できることを固定する
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosは対応表どおりのバインドで解決できる() {
        for (spec, action) in 非macos割当表() {
            let actions = 一致するアクション(spec);
            assert!(
                actions.iter().any(|a| a == action),
                "{spec} → {action} のバインドが無い（解決結果: {actions:?}）"
            );
        }
    }

    /// #585: シェル / TUI へ届くべきキーを奪っていない。
    /// Ctrl+英字は C0 制御コード（Ctrl+C = SIGINT 等）、Shift+Enter は Claude Code の
    /// 改行、Shift+Tab はモード切替、Alt+英字は #575 の meta 入力（Alt+V = 画像貼り付け）
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 端末へ流すべきキーを奪っていない() {
        for spec in [
            "ctrl-a",
            "ctrl-b",
            "ctrl-c",
            "ctrl-d",
            "ctrl-e",
            "ctrl-f",
            "ctrl-g",
            "ctrl-h",
            "ctrl-i",
            "ctrl-j",
            "ctrl-k",
            "ctrl-l",
            "ctrl-m",
            "ctrl-n",
            "ctrl-o",
            "ctrl-p",
            "ctrl-q",
            "ctrl-r",
            "ctrl-s",
            "ctrl-t",
            "ctrl-u",
            "ctrl-w",
            "ctrl-x",
            "ctrl-y",
            "ctrl-z",
            // readline の単語移動（\x1b[1;5C）
            "ctrl-left",
            "ctrl-right",
            // 素のキーと TUI が使う修飾つきキー
            "tab",
            "shift-tab",
            "enter",
            "shift-enter",
            "ctrl-enter",
            "escape",
            "space",
            "backspace",
            // #575 の meta 経路
            "alt-v",
            "alt-b",
            "alt-f",
            "alt-d",
            "alt-enter",
        ] {
            assert!(
                一致するアクション(spec).is_empty(),
                "{spec} を奪っている（端末入力が壊れる）"
            );
        }
    }

    /// #585: 追加バインドが「割当の原則」を守っているかの番犬。
    /// 原則を破るバインドを足すとここで落ちる
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macos追加バインドは割当の原則を守っている() {
        for b in key_bindings() {
            let keystrokes: Vec<Keystroke> =
                b.keystrokes().iter().map(|k| k.inner().clone()).collect();
            // platform 修飾つき = macOS 用の元バインド（Windows では Win キーで届かない）
            if keystrokes.iter().any(|k| k.modifiers.platform) {
                continue;
            }
            let action = b.action().name().to_string();
            for k in keystrokes {
                let m = k.modifiers;
                let 英字 = k.key.len() == 1 && k.key.chars().all(|c| c.is_ascii_alphabetic());
                if m.control && !m.shift && !m.alt && 英字 {
                    assert_eq!(
                        k.key, "v",
                        "{action}: ctrl-{} は C0 制御コードを奪う（例外はペーストの ctrl-v だけ）",
                        k.key
                    );
                }
                if m.alt {
                    assert!(
                        matches!(k.key.as_str(), "left" | "right" | "up" | "down"),
                        "{action}: alt-{} は #575 の meta 入力を奪う（alt は矢印だけ）",
                        k.key
                    );
                }
                if !m.control && !m.alt && !m.shift {
                    assert_eq!(
                        k.key, "f11",
                        "{action}: 修飾なしのバインドは f11 のみ（素のキーは端末入力）"
                    );
                }
                if m.shift {
                    assert!(
                        英字 || k.key.len() > 1,
                        "{action}: shift+{} は GPUI Windows の正規化（シフト後の文字 + shift 無し）で一致しない",
                        k.key
                    );
                }
            }
        }
    }

    /// #585: 奪ったキーが「元々 PTY へ何も送っていない」か「同じバイトを送る経路が
    /// 別に残る」かのどちらかであることを実際の変換で示す
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 奪ったキーは端末の入力手段を減らさない() {
        // Windows 実機で届く形（ctrl 付きは ToUnicode が文字を返さず key_char = None）
        let ctrl = |key: &str| Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: None,
        };
        // ctrl+数字 / ctrl+記号 / F キーは PTY へ 1 バイトも送っていない
        for key in ["0", "1", "5", "9", "=", "+", "-", ","] {
            assert_eq!(
                keystroke_to_bytes_default(&ctrl(key)),
                None,
                "ctrl-{key} が PTY へ送られている（奪うと入力手段が減る）"
            );
        }
        assert_eq!(keystroke_to_bytes_default(&ks("f11")), None);
        // ctrl+shift+英字 は ctrl+英字 と同じ C0 バイトに潰れる ＝ 奪っても
        // ctrl+英字 の側に同じ入力手段が残る
        for key in [
            "a", "b", "c", "d", "e", "f", "k", "n", "o", "p", "q", "r", "s", "t", "v", "w", "y",
            "z",
        ] {
            let mut shifted = ctrl(key);
            shifted.modifiers.shift = true;
            assert_eq!(
                keystroke_to_bytes_default(&shifted),
                keystroke_to_bytes_default(&ctrl(key)),
                "ctrl-shift-{key} が ctrl-{key} と別のバイトを送っている"
            );
        }
    }

    /// #602: ユーザー要望の「Ctrl+D でペイン分割」を**採らない**根拠を、
    /// 実際のバイト変換で残す。Ctrl+D は EOF（0x04）を PTY へ送っており、
    /// tako の主用途であるエージェント CLI ほどこれを使う（Claude Code / codex の
    /// 終了、REPL の終了、`cat > file` の入力終端）。GPUI はアクションを
    /// `on_key_down` より先に発火して伝播を止めるので、奪うと代替手段が消える
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_dはeofなので分割に使えない() {
        assert_eq!(
            keystroke_to_bytes_default(&ks_ctrl("d")),
            Some(vec![0x04]),
            "Ctrl+D が EOF(0x04) を送っていない（前提が変わったら判断をやり直すこと）"
        );
        assert!(
            一致するアクション("ctrl-d").is_empty(),
            "Ctrl+D を奪うと EOF の送出手段が無くなる"
        );
        // 代替として実在すべき分割キー（Cmd+D と同じ D を shift 段へ逃がした形）
        assert!(
            一致するアクション("ctrl-shift-d")
                .iter()
                .any(|a| a == "tako::SplitRight"),
            "ctrl-shift-d → SplitRight が無い"
        );
        assert!(
            一致するアクション("ctrl-shift-e")
                .iter()
                .any(|a| a == "tako::SplitDown"),
            "ctrl-shift-e → SplitDown が無い"
        );
        // Ctrl+Shift+D を奪っても Ctrl+D 側の EOF は無傷（同じ 0x04 へ潰れる）
        let mut shifted = ks_ctrl("d");
        shifted.modifiers.shift = true;
        assert_eq!(keystroke_to_bytes_default(&shifted), Some(vec![0x04]));
    }

    /// #602: Windows には正規の終了手段が無い（`handle_window_close` は最後の 1 枚でも
    /// プロセスを残し、`gpui_windows::close_one_window` は「最後の 1 枚だったか」を
    /// 捨てて `PostQuitMessage` を出さず、`on_reopen` は Windows で発火しない）。
    /// キーボードから届く Quit を 1 本確保する
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosにはplatform修飾なしのquitバインドがある() {
        assert!(
            !非platform修飾のバインド("tako::Quit").is_empty(),
            "Quit に platform 修飾なしのバインドが無い（Windows から到達不能）"
        );
        assert!(
            一致するアクション("ctrl-shift-q")
                .iter()
                .any(|a| a == "tako::Quit"),
            "ctrl-shift-q → Quit が無い"
        );
        assert!(
            一致するアクション("ctrl-q").is_empty(),
            "Ctrl+Q 単独は XON（フロー制御）なので奪わない"
        );
    }

    /// #602: macOS 固有アクションのバインドが非 macOS に漏れていない。
    /// `cmd-alt-h` は Windows で **Win+Alt+H** になる。これが届いた場合
    /// `HideOthers` → `gpui_windows::hide_other_apps` = `unimplemented!()` で
    /// **panic ＝ アプリごと abort**（器の無いペインは全滅）するため、経路ごと塞ぐ
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosにmacos固有アクションのバインドが無い() {
        for action in ["tako::HideApp", "tako::HideOthers", "tako::MinimizeWindow"] {
            assert!(
                key_bindings().iter().all(|b| b.action().name() != action),
                "{action} のバインドが非 macOS に残っている"
            );
        }
        assert!(macos_only_bindings().is_empty());
    }

    /// macOS だけに張るバインド（#602）。Windows では Win キーへ解決されて
    /// OS と衝突するうえ、`HideOthers` は `unimplemented!()` で app ごと落とす
    const MACOS_ONLY: [&str; 3] = ["cmd-h", "cmd-alt-h", "cmd-m"];

    /// #585: macOS のバインド 45 本（cmd- 40 + ctrl-cmd- 5）は Windows 対応で
    /// 1 本も変えない。両プラットフォームで実行して固定する
    /// （#602 で [`MACOS_ONLY`] の 3 本だけ非 macOS のビルドから外したので、
    /// 表そのものは 45 行のまま保ち、**どちらのプラットフォームでも表を検査する**。
    /// 非 macOS では 3 本が「存在しないこと」を逆向きに固定する）
    #[test]
    fn macos側のバインド45本は不変() {
        let macos割当表 = [
            ("cmd-d", "tako::SplitRight"),
            ("cmd-shift-d", "tako::SplitDown"),
            ("cmd-w", "tako::ClosePane"),
            ("cmd-t", "tako::NewTab"),
            ("cmd-shift-]", "tako::NextTab"),
            ("cmd-shift-[", "tako::PrevTab"),
            ("cmd-alt-left", "tako::FocusLeft"),
            ("cmd-alt-right", "tako::FocusRight"),
            ("cmd-alt-up", "tako::FocusUp"),
            ("cmd-alt-down", "tako::FocusDown"),
            ("ctrl-cmd-right", "tako::WidenPane"),
            ("ctrl-cmd-left", "tako::NarrowPane"),
            ("ctrl-cmd-down", "tako::TallenPane"),
            ("ctrl-cmd-up", "tako::ShortenPane"),
            ("cmd-c", "tako::CopySelection"),
            ("cmd-v", "tako::PasteClipboard"),
            ("cmd-s", "tako::SavePreview"),
            ("cmd-b", "tako::ToggleSidebar"),
            ("cmd-k", "tako::OpenCommandPalette"),
            ("cmd-q", "tako::Quit"),
            ("cmd-1", "tako::ActivateTab1"),
            ("cmd-2", "tako::ActivateTab2"),
            ("cmd-3", "tako::ActivateTab3"),
            ("cmd-4", "tako::ActivateTab4"),
            ("cmd-5", "tako::ActivateTab5"),
            ("cmd-6", "tako::ActivateTab6"),
            ("cmd-7", "tako::ActivateTab7"),
            ("cmd-8", "tako::ActivateTab8"),
            ("cmd-9", "tako::ActivateTab9"),
            ("cmd-=", "tako::ZoomIn"),
            ("cmd-+", "tako::ZoomIn"),
            ("cmd--", "tako::ZoomOut"),
            ("cmd-0", "tako::ResetZoom"),
            ("cmd-a", "tako::SelectAll"),
            ("cmd-o", "tako::OpenDirectory"),
            ("cmd-shift-o", "tako::OpenRepository"),
            ("cmd-shift-n", "tako::NewWindow"),
            ("cmd-,", "tako::OpenSettings"),
            ("cmd-z", "tako::UndoPreview"),
            ("cmd-shift-z", "tako::RedoPreview"),
            ("cmd-f", "tako::FindPreview"),
            ("cmd-h", "tako::HideApp"),
            ("cmd-alt-h", "tako::HideOthers"),
            ("cmd-m", "tako::MinimizeWindow"),
            ("ctrl-cmd-f", "tako::ToggleFullScreen"),
        ];
        assert_eq!(macos割当表.len(), 45, "macOS のバインドは 45 本");
        for (spec, action) in macos割当表 {
            let actions = 一致するアクション(spec);
            // macOS 固有の 3 本は非 macOS のビルドに存在しない（#602）
            if !cfg!(target_os = "macos") && MACOS_ONLY.contains(&spec) {
                assert!(
                    actions.is_empty(),
                    "{spec} が非 macOS に残っている（Win+Alt+H は unimplemented! で app ごと落ちる）"
                );
                continue;
            }
            assert!(
                actions.iter().any(|a| a == action),
                "{spec} → {action} が失われた（解決結果: {actions:?}）"
            );
        }
        let platform本数 = key_bindings()
            .iter()
            .filter(|b| b.keystrokes().iter().any(|k| k.inner().modifiers.platform))
            .count();
        let 期待 = if cfg!(target_os = "macos") {
            45
        } else {
            45 - MACOS_ONLY.len()
        };
        assert_eq!(
            platform本数, 期待,
            "platform（cmd / Win）修飾のバインド本数が変わった"
        );
    }

    /// macOS 側は従来どおり cmd- のみ（#467 / #585 の追加バインドが漏れ出していない）
    #[cfg(target_os = "macos")]
    #[test]
    fn macosには非platform修飾のバインドが無い() {
        assert!(platform_bindings().is_empty());
        assert!(
            key_bindings()
                .iter()
                .all(|b| b.keystrokes().iter().all(|k| k.inner().modifiers.platform)),
            "macOS に非 platform 修飾のバインドが混入している"
        );
    }

    /// #648: パレットに併記するショートカットが**バインド表と一致する**。
    /// 表示を手書きすると「書いてあるのに効かない」案内になるため、
    /// 導出元（[`key_bindings`]）と突き合わせて固定する
    #[test]
    fn パレットのショートカット表示はバインド表と一致する() {
        for (id, action) in [
            ("new-tab", "tako::NewTab"),
            ("split-right", "tako::SplitRight"),
            ("split-down", "tako::SplitDown"),
            ("toggle-files", "tako::ToggleSidebar"),
        ] {
            let hint = palette_shortcut(id)
                .unwrap_or_else(|| panic!("{id}: パレットにショートカットが出ていない"));
            // 表示の元になったバインドが、このプラットフォームで実際に届く形で実在する
            let matched = key_bindings().into_iter().any(|b| {
                b.action().name() == action
                    && b.keystrokes()
                        .iter()
                        .map(|k| format_keystroke(k.inner()))
                        .collect::<Vec<_>>()
                        .join(" ")
                        == hint
            });
            assert!(matched, "{id}: 表示 \"{hint}\" に対応するバインドが無い");
        }
        // バインドを持たない項目に嘘のショートカットを出さない
        for id in ["toggle-theme", "panel-git", "toggle-drawer", "存在しないid"] {
            assert_eq!(
                palette_shortcut(id),
                None,
                "{id}: バインドが無いのにショートカットを表示している"
            );
        }
    }

    /// #648: 非 macOS のパレット表示に `cmd-` 由来のキーが混ざらない。
    /// GPUI の `cmd` は platform 修飾で Windows では Win キーへ解決され、
    /// シェルが先に奪って**届かない**（#585）。案内に出したら嘘になる
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn 非macosのパレット表示はcmd由来のキーを出さない() {
        for id in ["new-tab", "split-right", "split-down", "toggle-files"] {
            let hint = palette_shortcut(id).expect("ショートカットが出ていない");
            assert!(
                !hint.contains('\u{2318}') && !hint.contains("Cmd") && !hint.contains("Win"),
                "{id}: 非 macOS で届かない修飾キーを案内している: {hint}"
            );
            assert!(
                hint.starts_with("Ctrl") || hint.starts_with("Alt") || hint.starts_with('F'),
                "{id}: Windows 慣習の表記になっていない: {hint}"
            );
        }
        // ユーザーが探していた分割キー（#648 の発端）が実際にこの字面で出る
        assert_eq!(
            palette_shortcut("split-right").as_deref(),
            Some("Ctrl+Shift+D")
        );
        assert_eq!(
            palette_shortcut("split-down").as_deref(),
            Some("Ctrl+Shift+E")
        );
    }

    /// #648: macOS のパレット表示は従来どおり ⌘ 記号。Windows 対応で
    /// macOS の見た目を変えていないことを固定する
    #[cfg(target_os = "macos")]
    #[test]
    fn macosのパレット表示はcmd記号を使う() {
        assert_eq!(
            palette_shortcut("split-right").as_deref(),
            Some("\u{2318}D")
        );
        assert_eq!(
            palette_shortcut("split-down").as_deref(),
            Some("\u{21e7}\u{2318}D")
        );
        assert_eq!(palette_shortcut("new-tab").as_deref(), Some("\u{2318}T"));
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

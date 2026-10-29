//! 実経路: dispatch の個別の断り（選択肢ダイアログへの応答・orchestrator・Web ビュー・メニュー・
//! 値の検証）が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 英語表示でも「choice 9 は範囲外（1-3）。選択肢: …」
//! 「name（複製先）を指定する」「URL が空です」のように日本語の固定文のまま返った。master が毎日叩く
//! `tako orchestrator respond` / `profiles` / `accounts` / `handoffs` と、`tako web` / `tako menu` の
//! 打ち間違いで最初に目にする断りがこれだった。
//!
//! dispatch は CLI / MCP / GUI が共有する 1 実装で、応答の文は `DispatchError` の表示（= IPC の受け口が
//! CLI / MCP へ返す文）そのもの。ここでは
//! - GUI の代わりのホストへ IPC と同じ形（`{"method", "params"}`）の要求を直接渡して断らせ、
//! - 選択肢ダイアログへの応答は in-process / detached が共有する `respond_via` へ、画面とキーを
//!   差し替えた `DialogAccess` を渡して断らせ、
//!
//! - 英語表示: 応答に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! をケースごとに見る。どのケースも断られて何も書かない（設定・置き場に触る前に断る。読むのは
//! 一時の `TAKO_DATA_DIR` の下だけ）。表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの
//! 1 本のテスト関数**の中で順に切り替える。このファイルは修正前の木でもコンパイルできる API だけを使う
//! （A/B: 修正前は英語表示の検査で落ちる）

use std::cell::Cell;

use serde_json::{json, Value};
use tako_control::protocol::{MenuBarSnapshot, MenuItemSnapshot, MenuSnapshot, Request};
use tako_control::reach::DialogAccess;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木とメニューの写しだけを持つ
struct Host {
    workspace: Workspace,
    menus: MenuBarSnapshot,
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl UiStateHost for Host {
    fn menu_bar_snapshot(&self) -> MenuBarSnapshot {
        self.menus.clone()
    }
}

impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// ウィンドウの中にメニューバーを描く環境（Windows）の写し。File / Find は前方一致で曖昧、
/// File の 2 項目は末尾一致で曖昧
fn in_window_menus() -> MenuBarSnapshot {
    let action = |label: &str| MenuItemSnapshot::Action {
        label: label.into(),
        action: format!("tako::{}", label.replace(' ', "")),
        shortcut: None,
    };
    MenuBarSnapshot {
        in_window: true,
        open: None,
        menus: vec![
            MenuSnapshot {
                name: "File".into(),
                items: vec![action("New Tab"), action("Close Tab")],
            },
            MenuSnapshot {
                name: "Find".into(),
                items: vec![action("Search")],
            },
        ],
    }
}

/// dispatch のケース: 名前・メニューの写し（`None` = 既定 = OS のメニューバー・項目なし）・要求
fn dispatch_cases() -> Vec<(&'static str, Option<MenuBarSnapshot>, Value)> {
    let req = |method: &str, params: Value| json!({ "method": method, "params": params });
    let with = |method: &str, params: Value| (None, req(method, params));
    let cases: Vec<(&'static str, (Option<MenuBarSnapshot>, Value))> = vec![
        // ---- orchestrator ----
        (
            "profiles_copy_name",
            with("orchestrator_profiles", json!({ "action": "copy" })),
        ),
        (
            "profiles_copy_from",
            with(
                "orchestrator_profiles",
                json!({ "action": "copy", "name": "x2365" }),
            ),
        ),
        (
            "profiles_set_agent",
            with(
                "orchestrator_profiles",
                json!({ "action": "set", "name": "x2365", "agent_model": "opus" }),
            ),
        ),
        (
            "profiles_set_policy",
            with(
                "orchestrator_profiles",
                json!({ "action": "set", "name": "x2365", "worker_model_policy": "bogus" }),
            ),
        ),
        (
            "profiles_set_env",
            with(
                "orchestrator_profiles",
                json!({ "action": "set", "name": "x2365", "env_set": ["NOEQ"] }),
            ),
        ),
        (
            "profiles_set_cwd",
            with(
                "orchestrator_profiles",
                json!({ "action": "set", "name": "x2365", "cwd": "rel/dir" }),
            ),
        ),
        (
            "accounts_add",
            with(
                "orchestrator_accounts",
                json!({ "action": "add", "name": "x2365" }),
            ),
        ),
        (
            "handoffs_show_none",
            with("orchestrator_handoff_files", json!({ "action": "show" })),
        ),
        (
            "handoffs_show_both",
            with(
                "orchestrator_handoff_files",
                json!({ "action": "show", "project": "a", "profile": "b" }),
            ),
        ),
        (
            "handoffs_write_content",
            with("orchestrator_handoff_files", json!({ "action": "write" })),
        ),
        (
            "handoffs_write_both",
            with(
                "orchestrator_handoff_files",
                json!({ "action": "write", "content": "x", "project": "a", "profile": "b" }),
            ),
        ),
        (
            "handoffs_bad_key",
            with(
                "orchestrator_handoff_files",
                json!({ "action": "show", "project": "a/b" }),
            ),
        ),
        (
            "spawn_no_target",
            with(
                "orchestrator_spawn",
                json!({ "project": "x2365", "prompt": "y" }),
            ),
        ),
        (
            "worker_status_none",
            with("orchestrator_worker_status", json!({})),
        ),
        // ---- Web ビュー ----
        (
            "web_open_empty",
            with("web", json!({ "action": "open", "url": " " })),
        ),
        ("web_show_no_id", with("web", json!({ "action": "show" }))),
        (
            "web_navigate_no_to",
            with("web", json!({ "action": "navigate" })),
        ),
        // ---- メニュー（OS のメニューバー = open / close は使えない・項目の写しが無い） ----
        ("menu_open_os", with("menu_open", json!({ "menu": "File" }))),
        (
            "menu_invoke_undefined",
            with("menu_invoke", json!({ "path": "File/New Tab" })),
        ),
        // ---- メニュー（ウィンドウの中のメニューバー） ----
        (
            "menu_open_empty",
            (
                Some(in_window_menus()),
                req("menu_open", json!({ "menu": " " })),
            ),
        ),
        (
            "menu_open_ambiguous",
            (
                Some(in_window_menus()),
                req("menu_open", json!({ "menu": "f" })),
            ),
        ),
        (
            "menu_open_missing",
            (
                Some(in_window_menus()),
                req("menu_open", json!({ "menu": "zzz" })),
            ),
        ),
        (
            "menu_invoke_empty",
            (
                Some(in_window_menus()),
                req("menu_invoke", json!({ "path": " / " })),
            ),
        ),
        (
            "menu_invoke_ambiguous",
            (
                Some(in_window_menus()),
                req("menu_invoke", json!({ "path": "tab" })),
            ),
        ),
        (
            "menu_invoke_missing",
            (
                Some(in_window_menus()),
                req("menu_invoke", json!({ "path": "nope" })),
            ),
        ),
        // ---- 値の検証 ----
        (
            "limit_resume_network",
            with("limit_resume", json!({ "network": true, "enabled": true })),
        ),
        ("panel_width", with("panel", json!({ "width": -1.0 }))),
        (
            "panel_sidebar_width",
            with("panel", json!({ "sidebar_width": 0.0 })),
        ),
        (
            "paste_guard_respond",
            with("paste_guard", json!({ "respond": "bogus" })),
        ),
        (
            "paste_guard_pane",
            with("paste_guard", json!({ "pane": 1 })),
        ),
        (
            "paste_guard_mode",
            with("paste_guard", json!({ "mode": "bogus" })),
        ),
        (
            "logs_max_mb",
            with("logs", json!({ "action": "set", "max_mb": 0 })),
        ),
        (
            "logs_total_max_mb",
            with("logs", json!({ "action": "set", "total_max_mb": 0 })),
        ),
        ("logs_read", with("logs", json!({ "action": "read" }))),
        (
            "run_auto_close",
            with(
                "run",
                json!({ "path": "/tmp/x2365.py", "auto_close": "bogus" }),
            ),
        ),
        (
            "run_interactive_auto_close",
            with(
                "run_interactive",
                json!({ "command": "echo", "auto_close": "bogus" }),
            ),
        ),
        (
            "runner_defaults_path_ext",
            with(
                "runner_defaults",
                json!({ "path": "/tmp/x2365.py", "ext": "py" }),
            ),
        ),
    ];
    cases
        .into_iter()
        .map(|(name, (menus, wire))| (name, menus, wire))
        .collect()
}

/// 選択肢ダイアログ（番号つき。#1263 の実画面の形）
const NUMBERED: &str = "  Teach auto mode about your environment?

  Auto mode works better when it knows your environment. Takes about a minute.

  ❯ 1. Yes
    2. Not now
    3. Don't show again

  Enter to confirm · Esc to cancel";

/// 番号なし二択の信頼ダイアログ（#1223 の実採取）
const UNNUMBERED: &str = r#" Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source
 project, or work from your team). If not, take a moment to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 > No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel"#;

/// 狭いペインでラベルが切り詰められたモデルセレクタ（#1143 の実採取）
const TRUNCATED: &str = r#"           k

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between
   Claude models. Your
   pick becomes the
   default for new
   sessions. For
   other/previous
   model names,
   specify with
   --model.

     1. Defaul…  Opus
                 5
                 with
                 1M co
                 ntext
     2. Opus (…  Opus
                 5
                 with
                 1M co
                 ntext ↓"#;

/// 画面とキーを差し替えた `DialogAccess`。`screens` を 1 回の取得ごとに 1 枚ずつ返し、
/// 尽きたら最後の 1 枚を返し続ける
struct FakeAccess {
    screens: Vec<Option<&'static str>>,
    taken: Cell<usize>,
    key_error: Option<&'static str>,
}

impl FakeAccess {
    fn new(screens: Vec<Option<&'static str>>) -> Self {
        Self {
            screens,
            taken: Cell::new(0),
            key_error: None,
        }
    }
}

impl DialogAccess for FakeAccess {
    fn capture(&self) -> Result<Vec<String>, String> {
        let i = self.taken.get().min(self.screens.len() - 1);
        self.taken.set(self.taken.get() + 1);
        match self.screens[i] {
            Some(screen) => Ok(screen.lines().map(str::to_string).collect()),
            None => Err("capture-pane: no server running".into()),
        }
    }
    fn send_key(&self, _key: &str) -> Result<(), String> {
        match self.key_error {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }
    fn route(&self) -> &'static str {
        "test-2365"
    }
}

/// 応答のケース: 名前・画面とキー・choice（`None` = 下見。応答の `hint` を見る）
fn respond_cases() -> Vec<(&'static str, FakeAccess, Option<&'static str>)> {
    let screen = |s: &'static str| FakeAccess::new(vec![Some(s)]);
    vec![
        (
            "respond_capture_failed",
            FakeAccess::new(vec![None]),
            Some("1"),
        ),
        ("respond_choice_empty", screen(NUMBERED), Some(" ")),
        ("respond_out_of_range", screen(NUMBERED), Some("9")),
        ("respond_no_match", screen(NUMBERED), Some("maybe")),
        ("respond_ambiguous", screen(NUMBERED), Some("o")),
        (
            "respond_key_failed",
            FakeAccess {
                key_error: Some("Broken pipe (os error 32)"),
                ..screen(NUMBERED)
            },
            Some("1"),
        ),
        ("respond_truncated_label", screen(TRUNCATED), Some("defaul")),
        (
            "respond_truncated_no_match",
            screen(TRUNCATED),
            Some("sonnet"),
        ),
        // 番号なし: 画面のハイライトが動かない = 着地を確かめられないので Enter を送らない
        ("respond_not_landed", screen(UNNUMBERED), Some("trust")),
        // 番号なし: カーソルを動かした後の画面からダイアログが消えた
        (
            "respond_vanished",
            FakeAccess::new(vec![Some(UNNUMBERED), Some("$ ")]),
            Some("trust"),
        ),
        ("respond_hint", screen(NUMBERED), None),
        ("respond_hint_truncated", screen(TRUNCATED), None),
    ]
}

/// 1 つのケースの観測（断りの文 / 下見の案内）
struct Seen {
    name: &'static str,
    text: String,
}

fn observe() -> Vec<Seen> {
    let mut seen = Vec::new();
    for (name, menus, wire) in dispatch_cases() {
        let mut host = Host {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
            menus: menus.unwrap_or_default(),
        };
        let request: Request = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("{name}: 要求を組めない: {e} / {wire}"));
        let text = match tako_control::dispatch(&mut host, request, PaneOrigin::Cli) {
            Err(e) => e.to_string(),
            Ok(v) => format!("<断られずに通った: {v}>"),
        };
        seen.push(Seen { name, text });
    }
    for (name, access, choice) in respond_cases() {
        let text = match tako_control::dispatch::respond_via(&access, 7, choice, Some("test")) {
            Err(e) => e.to_string(),
            Ok(v) if choice.is_none() => v["hint"].as_str().unwrap_or("<hint が無い>").to_string(),
            Ok(v) => format!("<断られずに通った: {v}>"),
        };
        seen.push(Seen { name, text });
    }
    seen
}

/// #2365 より前の日本語の文（修正前の木で [`observe`] が返した文そのもの。前置き込み）
const OLD_JA: [(&str, &str); 49] = [
    ("profiles_copy_name", "無効なパラメータ: name（複製先）を指定する"),
    ("profiles_copy_from", "無効なパラメータ: from（複製元）を指定する"),
    (
        "profiles_set_agent",
        "無効なパラメータ: agent_* 系の設定には agent（対象エージェント名）を指定する",
    ),
    (
        "profiles_set_policy",
        "無効なパラメータ: worker_model_policy が不正: 'bogus'（inherit / delegate / fixed）",
    ),
    (
        "profiles_set_env",
        "無効なパラメータ: env の形式が不正（KEY=VALUE が必要）: NOEQ",
    ),
    (
        "profiles_set_cwd",
        "無効なパラメータ: cwd は絶対パスか `~/` から始まる形で指定する（相対パスは起動時に解決できない）: rel/dir",
    ),
    (
        "accounts_add",
        "無効なパラメータ: config_dir か inherit のどちらかを指定する\
         （別 config dir なら config_dir、既定の資格情報をそのまま使うなら inherit=true）",
    ),
    (
        "handoffs_show_none",
        "無効なパラメータ: show は project か profile のどちらか一方を指定する",
    ),
    (
        "handoffs_show_both",
        "無効なパラメータ: show は project か profile のどちらか一方を指定する",
    ),
    (
        "handoffs_write_content",
        "無効なパラメータ: write は content を指定する",
    ),
    (
        "handoffs_write_both",
        "無効なパラメータ: write は project か profile のどちらか一方を指定する",
    ),
    (
        "handoffs_bad_key",
        "無効なパラメータ: プロジェクトキーがファイル名として使えない: a/b",
    ),
    ("spawn_no_target", "pane または tab を指定してください"),
    (
        "worker_status_none",
        "無効なパラメータ: pane_id または worker を指定してください",
    ),
    ("web_open_empty", "無効なパラメータ: URL が空です"),
    ("web_show_no_id", "無効なパラメータ: id は必須（web list で確認）"),
    (
        "web_navigate_no_to",
        "無効なパラメータ: to は必須（back / forward / reload / URL）",
    ),
    (
        "menu_open_os",
        "この環境の menu open / close は使えません（メニューは OS のメニューバーに載るため \
         tako が開閉できない）。項目の実行は menu invoke が使えます",
    ),
    (
        "menu_invoke_undefined",
        "メニュー定義がありません（GUI が起動していない可能性があります）",
    ),
    ("menu_open_empty", "無効なパラメータ: メニュー名が空"),
    (
        "menu_open_ambiguous",
        "無効なパラメータ: メニュー名 'f' が曖昧です: File / Find",
    ),
    (
        "menu_open_missing",
        "無効なパラメータ: メニュー 'zzz' が見つかりません（候補: File / Find）",
    ),
    ("menu_invoke_empty", "無効なパラメータ: 項目のパスが空"),
    (
        "menu_invoke_ambiguous",
        "無効なパラメータ: 項目 'tab' が曖昧です: File/New Tab / File/Close Tab",
    ),
    (
        "menu_invoke_missing",
        "無効なパラメータ: メニュー項目 'nope' が見つかりません（`tako menu list` で一覧できます）",
    ),
    (
        "limit_resume_network",
        "無効なパラメータ: network と enabled は併用できない（全体の設定かペインの設定のどちらか）",
    ),
    ("panel_width", "無効なパラメータ: width は正の数（px）を指定する"),
    (
        "panel_sidebar_width",
        "無効なパラメータ: sidebar_width は正の数（px）を指定する",
    ),
    (
        "paste_guard_respond",
        "無効なパラメータ: respond は send / cancel のどれか（指定: bogus）",
    ),
    (
        "paste_guard_pane",
        "無効なパラメータ: pane は判定する文字（check / text）と一緒に渡す",
    ),
    (
        "paste_guard_mode",
        "無効なパラメータ: mode は confirm / warn のどれか（指定: bogus）",
    ),
    ("logs_max_mb", "無効なパラメータ: max_mb は 1 以上を指定する"),
    (
        "logs_total_max_mb",
        "無効なパラメータ: total_max_mb は 1 以上を指定する",
    ),
    (
        "logs_read",
        "無効なパラメータ: read には pane または session_id が必要",
    ),
    (
        "run_auto_close",
        "無効なパラメータ: auto_close は success / always / never のいずれか（指定: \"bogus\"）",
    ),
    (
        "run_interactive_auto_close",
        "無効なパラメータ: auto_close は success / always / never のいずれか（指定: \"bogus\"）",
    ),
    (
        "runner_defaults_path_ext",
        "無効なパラメータ: path（実行設定）と ext / command / remove（拡張子既定）は同時に使えない",
    ),
    (
        "respond_capture_failed",
        "画面の取得に失敗: capture-pane: no server running",
    ),
    (
        "respond_choice_empty",
        "choice が空。選択肢: 1. Yes / 2. Not now / 3. Don't show again",
    ),
    (
        "respond_out_of_range",
        "choice 9 は範囲外（1-3）。選択肢: 1. Yes / 2. Not now / 3. Don't show again",
    ),
    (
        "respond_no_match",
        "「maybe」に一致する選択肢が無い。選択肢: 1. Yes / 2. Not now / 3. Don't show again",
    ),
    (
        "respond_ambiguous",
        "「o」が複数の選択肢に一致する（2件）。番号で指定する。選択肢: 1. Yes / 2. Not now / 3. Don't show again",
    ),
    (
        "respond_key_failed",
        "キー 1 の送信に失敗: Broken pipe (os error 32)",
    ),
    (
        "respond_truncated_label",
        "選択肢 1 のラベルは画面上で切り詰められているか説明との境目が読めない（`Defaul…`）ため、\
         ラベル指定では確定できない。番号（--choice 1）で指定する。\
         選択肢: 1. Defaul…（切り詰め） / 2. Opus (…（切り詰め）",
    ),
    (
        "respond_truncated_no_match",
        "「sonnet」に一致する選択肢が無い（この画面はラベルが切り詰められているか説明との境目が読めないので、\
         ラベルでは確定できない。番号で指定する）。選択肢: 1. Defaul…（切り詰め） / 2. Opus (…（切り詰め）",
    ),
    (
        "respond_not_landed",
        "選択肢「Yes, I trust this folder」へカーソルを移動できなかったため応答を中止した（Enter は送っていません）",
    ),
    (
        "respond_vanished",
        "カーソル移動中にダイアログが消えた（応答は送っていません）",
    ),
    (
        "respond_hint",
        "応答するには choice に番号（1-3）かラベルの一部を渡す",
    ),
    (
        "respond_hint_truncated",
        "応答するには choice に番号（1-2）を渡す\
         （この画面はラベルが切り詰められているか説明との境目が読めないので\
         ラベル指定は受け付けない）",
    ),
];

#[test]
fn dispatch_の個別の断りは表示言語に従う() {
    // 読む置き場（ペインのログの設定など）を一時の場所へ向ける。断る前に書く経路は無い
    let data = std::env::temp_dir().join(format!("tako-2365k-{}", std::process::id()));
    std::fs::create_dir_all(&data).expect("一時の data dir");
    std::env::set_var("TAKO_DATA_DIR", &data);

    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe()));
    }
    i18n::set_lang(original);
    let _ = std::fs::remove_dir_all(&data);

    let expected = &OLD_JA;
    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        assert_eq!(seen.len(), expected.len(), "ケースの数");
        for (s, (name, old)) in seen.iter().zip(expected) {
            assert_eq!(s.name, *name, "ケースの並び");
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja if s.text != *old => failures.push(format!(
                    "[Ja] {}: #2365 より前の文と違う\n  want: {old}\n  got:  {}",
                    s.name, s.text
                )),
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        failures.push(format!(
                            "[En] {}: 英語表示の断りに日本語（{c}）が混ざる: {}",
                            s.name, s.text
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    let (ja, en) = (&by_lang[0].1, &by_lang[1].1);
    for (j, e) in ja.iter().zip(en) {
        if j.text == e.text {
            failures.push(format!("{}: 日英が同じ文: {}", j.name, j.text));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

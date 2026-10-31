//! 実経路: `tako mod ui` の応答（成功の枠・断り）が、1 つの応答の中で表示言語にそろう
//! （Issue #2365 の続き。断りの UiError 一式は #2394）。
//!
//! 修正前（実測。CLI の直呼び `tako mod ui` を TAKO_LANG=en で）: 断りは英語でも、成功の応答の枠
//! （`applies_to` / `note` / `choices` の説明と `hotkey`）・引数の読み方の断り・置き場の排他の断りが
//! 日本語のままで、`problems` は枠に合わせて日本語に固定していた。
//!
//! ここでは 2 つの口を GUI 無しで通す:
//! - CLI の口: `cli_request`（引数の読み方）→ `run`（`tako mod ui` のローカル処理と同じ 2 段）
//! - MCP の口: `claude_mod::run` の `action=ui`（GUI の dispatch が `tako_mod` で呼ぶ 1 本）を
//!   GUI の代わりのホストへ渡し、断りは `DispatchError` の表示（= IPC の受け口が返す文）で見る
//!
//! 英語表示では応答に日本語（かな・漢字）が 1 字も無いこと（パスは ASCII の一時 dir）、日本語表示では
//! 枠の文が #2365 より前とバイト一致することを見る。表示言語はプロセス全体の状態なので、**専用の
//! 統合テストバイナリの 1 本のテスト関数**の中で順に切り替える

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::claude_mod_ui as ui;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::claude_mod_ui::UiRequest;
use tako_core::i18n::{self, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木と ui.json の置き場だけを持つ
struct Host {
    workspace: Workspace,
    ui_path: Option<PathBuf>,
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

impl SystemHost for Host {
    fn claude_mod_ui_path(&self) -> Option<PathBuf> {
        self.ui_path.clone()
    }
}

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

fn req(v: Value) -> UiRequest {
    serde_json::from_value(v).unwrap()
}

/// 1 つの口の 1 つのケースの観測（応答の文。成功は JSON・断りは表示の文。一時 dir は `<DIR>`）
struct Seen {
    name: String,
    text: String,
    /// 日本語表示のときの #2365 より前の文（全文の比較が要らないものは None）
    old_ja: Option<String>,
}

/// 置き場の様子ごとに一時の ui.json を作る
fn place(dir: &Path, name: &str, body: Option<&str>) -> PathBuf {
    let sub = dir.join(name).join("claude-mod");
    std::fs::create_dir_all(&sub).unwrap();
    let path = sub.join(tako_core::claude_mod_ui::FILENAME);
    if let Some(body) = body {
        std::fs::write(&path, body).unwrap();
    }
    path
}

/// CLI の口と MCP の口の両方で、代表のケースを通して応答の文を集める
fn observe() -> Vec<Seen> {
    let scratch = ScratchDir::new("issue2365-mod-ui");
    let dir = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
    let base = dir.display().to_string();
    let mut seen = Vec::new();
    let mut push = |name: String, text: String, old_ja: Option<String>| {
        seen.push(Seen {
            name,
            text: text.replace(&base, "<DIR>"),
            old_ja,
        });
    };
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
        ui_path: None,
    };
    let mcp = |host: &mut Host, path: &Path, request: UiRequest| -> String {
        host.ui_path = Some(path.to_path_buf());
        match tako_control::claude_mod::run(
            host,
            Some("ui"),
            None,
            None,
            None,
            Some(request),
            false,
        ) {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        }
    };
    let cli = |path: &Path, words: &[&str]| -> String {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        match ui::cli_request(&words, None, None, None) {
            Ok(request) => match ui::run(path, &request) {
                Ok(v) => v.to_string(),
                Err(e) => e.message(),
            },
            Err(e) => e.to_string(),
        }
    };

    // 見るだけ（choices の説明・hotkey）
    let p = place(&dir, "show", None);
    push("cli:show".into(), cli(&p, &[]), None);
    push(
        "mcp:show".into(),
        mcp(&mut host, &p, UiRequest::default()),
        None,
    );
    // 変える（applies_to）
    let p = place(&dir, "change", None);
    let out = cli(&p, &["button", "add", "split-right"]);
    push("cli:change".into(), out, None);
    let p = place(&dir, "change-mcp", None);
    let out = mcp(
        &mut host,
        &p,
        req(json!({"op": "button_add", "value": "split-right"})),
    );
    push("mcp:change".into(), out, None);
    // 新しい tako が書いた形（note）
    let p = place(&dir, "future", Some("{\"schema_version\": 99}"));
    push("cli:future".into(), cli(&p, &[]), None);
    push(
        "mcp:future".into(),
        mcp(&mut host, &p, UiRequest::default()),
        None,
    );
    // 壊れた ui.json（problems）
    let p = place(
        &dir,
        "repaired",
        Some("{\"schema_version\": 1, \"buttons\": 7}"),
    );
    push("cli:repaired".into(), cli(&p, &[]), None);
    // 置き場の排他を取れない（RunError::Lock）
    let p = place(&dir, "lock", None);
    std::fs::create_dir_all(p.with_file_name("ui.json.lock")).unwrap();
    let lock_old = format!(
        "ロックファイルを開けない ({}): ",
        p.with_file_name("ui.json.lock").display()
    )
    .replace(&base, "<DIR>");
    let out = cli(&p, &["set", "usage_bar.place", "off"]);
    push("cli:lock".into(), out, Some(lock_old.clone()));
    let out = mcp(
        &mut host,
        &p,
        req(json!({"op": "set", "key": "usage_bar.place", "value": "off"})),
    );
    push("mcp:lock".into(), out, Some(lock_old));
    // 引数の読み方の断り（CLI の口だけ。MCP は引数の形が型で決まる）
    let p = place(&dir, "args", None);
    for (name, words, old) in [
        (
            "cli:usage",
            &["bogus"][..],
            "使い方: tako mod ui [set <キー> <値> | button add [<kind>] <中身> | button remove <id> | button move <id> <先> | preset <名前> | reset]",
        ),
        (
            "cli:remove",
            &["remove"][..],
            "button remove には id を 1 つ付ける（id は tako mod ui の一覧）",
        ),
        (
            "cli:move",
            &["move", "compact"][..],
            "button move には id と先を付ける（先: 1〜8 / first / last / left / right）",
        ),
        (
            "cli:flags",
            &["set", "band.hidden", "true"][..],
            "",
        ),
    ] {
        let text = if name == "cli:flags" {
            // `--label` を button add 以外に付けた
            let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            ui::cli_request(&words, Some("x"), None, None)
                .unwrap_err()
                .to_string()
        } else {
            cli(&p, words)
        };
        let old = if old.is_empty() {
            "--label / --hotkey / --id は button add のときだけ使う"
        } else {
            old
        };
        push(name.into(), text, Some(old.into()));
    }
    // 断り（UiError = #2394）も同じ言語で前置きつき（MCP）
    let out = mcp(
        &mut host,
        &p,
        req(json!({"op": "set", "key": "colors.dim", "value": "#123456"})),
    );
    push("mcp:invalid".into(), out, None);
    seen
}

/// 日本語表示の応答の JSON に、#2365 より前の枠の文がそのまま載っているか
fn ja_frame_holds(name: &str, text: &str) -> Result<(), String> {
    let want: &[&str] = match name {
        "cli:show" | "mcp:show" => &["数字 1 字か英小文字 1 字（重複不可。省くと空いている英字を選ぶ）"],
        "cli:change" | "mcp:change" => &["tako のペインの claude へは次の報告（最大 15 秒）で届く（ui.json は tako の data dir にあり、Claude Code の設定 dir へは書かない）"],
        "cli:future" | "mcp:future" => &["新しい tako が書いた形（schema_version 99）。読める範囲で使い、書き換えない"],
        "cli:repaired" => &["buttons が配列ではない"],
        _ => &[],
    };
    for w in want {
        if !text.contains(w) {
            return Err(format!(
                "[Ja] {name}: #2365 より前の文が無い: {w}\n  got: {text}"
            ));
        }
    }
    Ok(())
}

#[test]
fn mod_ui_の応答は1つの応答の中で表示言語にそろう() {
    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe()));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        for s in seen {
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja => {
                    if let Some(old) = &s.old_ja {
                        if !s.text.starts_with(old.as_str()) {
                            failures.push(format!(
                                "[Ja] {}: #2365 より前の文と違う\n  want: {old}\n  got:  {}",
                                s.name, s.text
                            ));
                        }
                    }
                    if let Err(e) = ja_frame_holds(&s.name, &s.text) {
                        failures.push(e);
                    }
                }
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        failures.push(format!(
                            "[En] {}: 英語表示の応答に日本語（{c}）が混ざる: {}",
                            s.name, s.text
                        ));
                    }
                }
            }
        }
    }
    let (ja, en) = (&by_lang[0].1, &by_lang[1].1);
    assert_eq!(ja.len(), en.len(), "ケースの数");
    for (j, e) in ja.iter().zip(en) {
        if j.text == e.text {
            failures.push(format!("{}: 日英が同じ文: {}", j.name, j.text));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

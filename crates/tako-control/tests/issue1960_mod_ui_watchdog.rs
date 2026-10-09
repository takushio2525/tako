//! tako mod の定型の UI 設定 ui.json（FR-2.42.19〜24 / #1960）の番犬
//!
//! # なぜ要るか
//!
//! ui.json は**軽いモデルが叩いても壊れない**ことが存在意義で、それは「CLI / MCP / setup の 3 つの口が
//! 1 つの検証を通る」ことでしか保てない。どれか 1 つの口が自前で ui.json を組み立てたり、検証を
//! 飛ばして書いたりすると、その口からだけ語彙の外の値（任意のコマンド・テーマに無い色・9 個目の
//! ボタン）が mod へ届く。応答の形は変わらないので、ふつうの単体テストでは気づけない。
//!
//! # 何を縛るか
//!
//! 1. **挙動**: 3 つの口（CLI の引数の読み方 `cli_request` → `run`・MCP の実変換 `handle_message` →
//!    `run`・setup の answers の JSON → `SetupAnswers::validate` → `apply_answers`）で同じ変更をすると
//!    ui.json が**字面で一致**し、不正な値はどの口でも**書かれず同じ理由と許される値**が返る
//! 2. **構造**: 口はすべて `tako_control::claude_mod_ui` を通り、ui.json を書くのは core の
//!    `save_to` / `update_at` だけ。帯のトグルの正本は ui.json（メモリの中継に戻さない）。
//!    `#916` の登録簿に載っている
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::claude_mod_ui::{self as ui, RunError};
use tako_control::protocol::Request;
use tako_core::claude_mod_ui::{UiAnswers, UiRequest};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テストごとの ui.json（テストの data dir = #944 の隔離先の中）
fn scratch(name: &str) -> PathBuf {
    let dir = tako_core::paths::data_dir()
        .expect("テストの data dir")
        .join(format!(
            "issue1960-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(tako_core::claude_mod_ui::FILENAME)
}

// --- 3 つの口 ------------------------------------------------------------------------

/// CLI の口（`tako mod ui …` の語をそのまま渡す。`mod_ui_local` と同じ 2 段）
fn cli(path: &Path, words: &[&str]) -> Result<Value, String> {
    let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    let request = ui::cli_request(&words, None, None, None)?;
    ui::run(path, &request).map_err(|e| e.message().to_string())
}

/// MCP の口（`tools/call tako_mod` を本物の変換に通す。実行係は dispatch の `ui` の腕と同じ 1 本）
fn mcp(path: &Path, args: Value) -> Result<Value, String> {
    let mut exec = |request: Request| -> Result<Value, String> {
        match request {
            Request::Mod {
                action: Some(action),
                ui: request,
                ..
            } if action == "ui" => {
                ui::run(path, &request.unwrap_or_default()).map_err(|e| e.message().to_string())
            }
            other => Err(format!("想定外の要求: {other:?}")),
        }
    };
    let mut session = tako_control::mcp::McpSession {
        caller_pane: None,
        caller_role: None,
        connected: true,
        exec: &mut exec,
    };
    let mut call = args;
    call["action"] = json!("ui");
    let reply = tako_control::mcp::handle_message(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "tako_mod", "arguments": call}}),
        &mut session,
    )
    .expect("tools/call には応答がある");
    if let Some(err) = reply.get("error") {
        return Err(err["message"].as_str().unwrap_or_default().to_string());
    }
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        Err(text.to_string())
    } else {
        Ok(serde_json::from_str(text).expect("応答は JSON"))
    }
}

/// setup の口（`--answers` の JSON を本物の `SetupAnswers` で読み、検証してから当てる。
/// `tako setup` の `run_setup_stage` が呼ぶのと同じ `apply_answers`）
fn setup(path: &Path, mod_ui: Value) -> Result<Value, String> {
    let answers =
        tako_control::setup::SetupAnswers::from_json(&json!({ "mod_ui": mod_ui }).to_string())?;
    let mod_ui: UiAnswers = answers.mod_ui.expect("mod_ui を読んだ");
    ui::apply_answers(path, &mod_ui).map_err(|e| e.message().to_string())
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 受け入れ条件 1: 3 つの口で同じ変更 → ui.json が字面で一致する
#[test]
fn 三つの口で同じ変更をするとuijsonが字面で一致する() {
    let (a, b, c) = (scratch("cli"), scratch("mcp"), scratch("setup"));
    for words in [
        &["set", "usage_bar.place", "band"][..],
        &["set", "colors.accent", "claude"],
        &["set", "band.segments", "pane,attention,buttons"],
        &["button", "add", "context"],
        &["button", "add", "split-right"],
        &["button", "move", "split-right", "first"],
    ] {
        cli(&a, words).unwrap_or_else(|e| panic!("CLI {words:?}: {e}"));
    }
    for args in [
        json!({"op": "set", "key": "usage_bar.place", "value": "band"}),
        json!({"op": "set", "key": "colors.accent", "value": "claude"}),
        json!({"op": "set", "key": "band.segments", "value": ["pane", "attention", "buttons"]}),
        json!({"op": "button_add", "value": "context"}),
        json!({"op": "button_add", "kind": "tako", "value": "split-right"}),
        json!({"op": "button_move", "id": "split-right", "to": 1}),
    ] {
        mcp(&b, args.clone()).unwrap_or_else(|e| panic!("MCP {args}: {e}"));
    }
    setup(
        &c,
        json!({
            "set": {
                "usage_bar.place": "band",
                "colors.accent": "claude",
                "band.segments": "pane attention buttons",
            },
            "buttons": [
                {"kind": "tako", "value": "split-right"},
                {"value": "compact"},
                {"value": "context"},
            ],
        }),
    )
    .unwrap();
    let (ta, tb, tc) = (text(&a), text(&b), text(&c));
    assert_eq!(ta, tb, "CLI と MCP で ui.json が違う");
    assert_eq!(ta, tc, "CLI と setup で ui.json が違う");
    // 中身も意図どおり（並べ替えが効き、既定の compact が残っている）
    let v: Value = serde_json::from_str(&ta).unwrap();
    let ids: Vec<&str> = v["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["split-right", "compact", "context"]);
    assert_eq!(v["usage_bar"]["place"], "band");
}

/// setup の answers の 9 個の並び
fn nine_buttons() -> Vec<Value> {
    [
        "compact",
        "clear",
        "context",
        "cost",
        "model",
        "effort",
        "tako",
        "split-right",
        "split-down",
    ]
    .iter()
    .map(|v| json!({ "value": v }))
    .collect()
}

/// 受け入れ条件 2: 不正な値はどの口でも書かれず、同じ理由と許される値が返る
#[test]
fn 不正な値はどの口でも書かれず許される値が返る() {
    let (a, b, c) = (scratch("bad-cli"), scratch("bad-mcp"), scratch("bad-setup"));
    // 3 つとも同じ出発点（既定 + 7 個 = 8 個のボタン）を書いておく
    let eight = [
        "clear",
        "context",
        "cost",
        "model",
        "effort",
        "tako",
        "split-right",
    ];
    for path in [&a, &b, &c] {
        for value in eight {
            cli(path, &["button", "add", value]).unwrap();
        }
    }
    let before = text(&a);
    assert_eq!(before, text(&b));
    // (CLI の語, MCP の引数, setup の mod_ui, 文言に含まれるはずの許される値)
    let cases: Vec<(Vec<&str>, Value, Value, &str)> = vec![
        // 語彙外の action（kind）
        (
            vec!["button", "add", "javascript", "x"],
            json!({"op": "button_add", "kind": "javascript", "value": "x"}),
            json!({"buttons": [{"kind": "javascript", "value": "x"}]}),
            "slash / tako / shell / prompt",
        ),
        // 許可リスト外の slash
        (
            vec!["button", "add", "slash", "bash"],
            json!({"op": "button_add", "kind": "slash", "value": "bash"}),
            json!({"buttons": [{"kind": "slash", "value": "bash"}]}),
            "compact / clear / context",
        ),
        // テーマに無い色
        (
            vec!["set", "colors.warn", "#ff0000"],
            json!({"op": "set", "key": "colors.warn", "value": "#ff0000"}),
            json!({"set": {"colors.warn": "#ff0000"}}),
            "suggestion",
        ),
        // 9 個目のボタン（setup は 9 個の並び）
        (
            vec!["button", "add", "split-down"],
            json!({"op": "button_add", "value": "split-down"}),
            json!({ "buttons": nine_buttons() }),
            "8 個まで",
        ),
    ];
    for (words, args, answers, allowed) in cases {
        let e_cli = cli(&a, &words).unwrap_err();
        let e_mcp = mcp(&b, args.clone()).unwrap_err();
        let e_setup = setup(&c, answers.clone()).unwrap_err();
        assert!(e_cli.contains(allowed), "CLI {words:?}: {e_cli}");
        assert!(e_cli.contains("ui.json は書いていない"), "{e_cli}");
        assert_eq!(e_cli, e_mcp, "CLI と MCP で理由が違う（{words:?}）");
        assert!(e_setup.contains(allowed), "setup {answers}: {e_setup}");
        assert!(e_setup.starts_with("mod_ui: "), "{e_setup}");
    }
    // 重複 hotkey（CLI は --hotkey、MCP は hotkey、setup は並びの中の重複）
    let req = ui::cli_request(
        &["button".into(), "remove".into(), "tako".into()],
        None,
        None,
        None,
    )
    .unwrap();
    for path in [&a, &b] {
        ui::run(path, &req).unwrap();
    }
    let before_dup = text(&a);
    let e_cli = ui::run(
        &a,
        &ui::cli_request(
            &["button".into(), "add".into(), "tako".into()],
            None,
            Some("c"),
            None,
        )
        .unwrap(),
    )
    .unwrap_err();
    let e_mcp = mcp(
        &b,
        json!({"op": "button_add", "value": "tako", "hotkey": "c"}),
    )
    .unwrap_err();
    let e_setup = setup(
        &c,
        json!({"buttons": [{"value": "compact", "hotkey": "c"}, {"value": "tako", "hotkey": "c"}]}),
    )
    .unwrap_err();
    assert!(matches!(
        ui::run(
            &a,
            &UiRequest {
                op: Some("button_add".into()),
                value: Some(json!("tako")),
                hotkey: Some("c".into()),
                ..UiRequest::default()
            }
        ),
        Err(RunError::Invalid(_))
    ));
    assert_eq!(e_cli.message(), e_mcp);
    assert!(e_cli.message().contains("hotkey"), "{}", e_cli.message());
    assert!(e_setup.contains("hotkey"), "{e_setup}");
    // どの口でも書いていない
    assert_eq!(text(&a), before_dup);
    assert_eq!(text(&b), before_dup);
    assert_eq!(
        text(&c),
        before,
        "setup は検証で断るので 1 バイトも書かない"
    );
}

/// 受け入れ条件 4: 壊れた ui.json で落ちず既定で動き、退避ファイルが残る。setup の段も止まらない
#[test]
fn 壊れたuijsonでも口は落ちず既定で動き退避が残る() {
    let path = scratch("broken");
    std::fs::write(&path, "{\"schema_version\": 1, \"buttons\": [{\"id\": 1}").unwrap();
    let out = cli(&path, &[]).unwrap();
    assert_eq!(out["state"], "repaired");
    assert_eq!(
        out["ui"],
        json!(tako_core::claude_mod_ui::UiConfig::default())
    );
    let bak = tako_core::migration::quarantine_slot_path(&path, 1);
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "{\"schema_version\": 1, \"buttons\": [{\"id\": 1}"
    );
    // MCP の口も同じ（落ちない）
    let out = mcp(&path, json!({})).unwrap();
    assert_eq!(out["state"], "repaired");
    // 変更の操作は直した形で書く
    cli(&path, &["set", "chat.code_copy", "false"]).unwrap();
    assert!(tako_core::claude_mod_ui::validate_text(&text(&path)).is_ok());
}

// --- 構造 --------------------------------------------------------------------------

const CORE: &str = "crates/tako-core/src/claude_mod_ui.rs";
const CONTROL_UI: &str = "crates/tako-control/src/claude_mod_ui.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";
const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";
const CONTROL_SETUP: &str = "crates/tako-control/src/setup.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MIGRATIONS: &str = "crates/tako-control/src/migrations.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、字下げが `needle` の行と同じ `close` 行までの窓（行番号は 1 始まり）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓が `must` を含むか。無ければ窓の先頭を名指す
fn require(
    out: &mut Vec<String>,
    file: &str,
    src: &str,
    (needle, close): (&str, &str),
    must: &str,
    why: &str,
) {
    match window(src, needle, close) {
        Some((at, w)) if !w.contains(must) => out.push(report(file, at, why)),
        Some(_) => {}
        None => out.push(report(
            file,
            1,
            &format!("{needle} が見つからない（走査が空振り）"),
        )),
    }
}

struct Sources {
    core: String,
    control_ui: String,
    control_mod: String,
    cli_main: String,
    cli_setup: String,
    control_setup: String,
    mcp_request: String,
    migrations: String,
}

fn current() -> Sources {
    Sources {
        core: read(CORE),
        control_ui: read(CONTROL_UI),
        control_mod: read(CONTROL_MOD),
        cli_main: read(CLI_MAIN),
        cli_setup: read(CLI_SETUP),
        control_setup: read(CONTROL_SETUP),
        mcp_request: read(MCP_REQUEST),
        migrations: read(MIGRATIONS),
    }
}

/// ui.json を書く口（**core の `save_to` / `update_at` だけ**。他の場所から書いたら名指す）
fn scan_writers(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in [
        (CONTROL_MOD, &s.control_mod),
        (CLI_MAIN, &s.cli_main),
        (CLI_SETUP, &s.cli_setup),
        (CONTROL_SETUP, &s.control_setup),
        (MCP_REQUEST, &s.mcp_request),
    ] {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if code.contains("claude_mod_ui::save_to(")
                || code.contains("claude_mod_ui::update_at(")
                || (code.contains("ui_path()") && code.contains("write"))
            {
                out.push(report(
                    file,
                    i + 1,
                    "ui.json を口の側で書いている（書くのは tako_control::claude_mod_ui の run / apply_answers / import_band だけ）",
                ));
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    // CLI: ローカル処理の腕があり、引数の読み方も操作も tako-control の 1 実装
    if !s.cli_main.contains(
        "Command::Mod(ref args) if args.action.as_deref() == Some(\"ui\") => mod_ui_local(args),",
    ) {
        out.push(report(
            CLI_MAIN,
            1,
            "`tako mod ui` がローカル処理の腕（mod_ui_local）を通らない（GUI が無いと変えられない）",
        ));
    }
    let fn_end = ("fn mod_ui_local(", "}");
    require(
        &mut out,
        CLI_MAIN,
        &s.cli_main,
        fn_end,
        "tako_control::claude_mod_ui::cli_request(",
        "mod_ui_local が引数を自前で読んでいる（cli_request を通す = MCP と同じ形に落とす）",
    );
    require(
        &mut out,
        CLI_MAIN,
        &s.cli_main,
        fn_end,
        "tako_control::claude_mod_ui::run(",
        "mod_ui_local が tako_control::claude_mod_ui::run を通らない",
    );
    // dispatch（MCP と GUI）: ui と帯の切替が同じ 1 実装
    let run_window = ("pub fn run(", "}");
    for (must, why) in [
        (
            "crate::claude_mod_ui::run(&ui_path(host)?, &ui.unwrap_or_default())",
            "dispatch の action=ui が claude_mod_ui::run を通らない",
        ),
        (
            "crate::claude_mod_ui::run(&ui_path(host)?, &request)",
            "band-on / band-off が ui.json を通らない（帯のトグルの正本は ui.json = メモリの中継へ戻さない）",
        ),
    ] {
        require(&mut out, CONTROL_MOD, &s.control_mod, run_window, must, why);
    }
    require(
        &mut out,
        CONTROL_MOD,
        &s.control_mod,
        ("fn band_view(", "}"),
        "crate::claude_mod_ui::view(",
        "報告の応答の tako.view.ui が検証済みの ui.json から来ていない",
    );
    require(
        &mut out,
        CONTROL_MOD,
        &s.control_mod,
        ("fn accept_report(", "}"),
        "crate::claude_mod_ui::import_band(",
        "mod の中で切り替えた帯のトグルを ui.json へ取り込んでいない",
    );
    // MCP: 引数は UiRequest へそのまま写す（検証は dispatch の 1 実装）
    require(
        &mut out,
        MCP_REQUEST,
        &s.mcp_request,
        ("\"tako_mod\" => {", "        }"),
        "tako_core::claude_mod_ui::UiRequest",
        "MCP の tako_mod が action=ui の引数を UiRequest へ写していない",
    );
    // setup: 対話も answers も同じ段（apply_answers）。answers は書く前に検証する
    if !s
        .cli_setup
        .contains("tako_control::claude_mod_ui::run_setup_stage(")
    {
        out.push(report(
            CLI_SETUP,
            1,
            "setup が claude_mod_ui::run_setup_stage を呼ばない",
        ));
    }
    require(
        &mut out,
        CLI_SETUP,
        &s.cli_setup,
        ("fn review_mod_ui(", "}"),
        "check_answers(",
        "setup の対話が選んだ値を検証していない（answers と同じ検証を通す）",
    );
    require(
        &mut out,
        CONTROL_SETUP,
        &s.control_setup,
        ("    pub fn validate(&self)", "    }"),
        "claude_mod_ui::check_answers(",
        "SetupAnswers::validate が mod_ui を検証していない（setup の途中で半端に止まる）",
    );
    require(
        &mut out,
        CONTROL_UI,
        &s.control_ui,
        ("pub fn apply_answers(", "}"),
        "apply_ops(",
        "apply_answers が run と同じ書き込みの道（apply_ops）を通らない",
    );
    // core: 書き込みは一時ファイル → 差し替え。読めない中身は退避してから既定へ
    require(
        &mut out,
        CORE,
        &s.core,
        ("pub fn save_to(", "}"),
        "std::fs::rename(&tmp, path)",
        "save_to が一時ファイル → 差し替えになっていない（途中の中身が他のプロセスから見える）",
    );
    require(
        &mut out,
        CORE,
        &s.core,
        ("pub fn load_from(", "}"),
        "quarantine_unreadable(",
        "load_from が読めない中身を退避せずに既定へ落とす（直後の保存で元が消える = #916）",
    );
    // #916: 登録簿
    require(
        &mut out,
        MIGRATIONS,
        &s.migrations,
        ("pub const SPECS: &[SchemaSpec] = &[", "];"),
        "id: SchemaId::ClaudeModUi,",
        "ui.json が移行の登録簿（SPECS）に無い",
    );
    out.extend(scan_writers(s));
    out
}

#[test]
fn uijsonの口の構造が契約どおり() {
    let found = all(&current());
    assert!(
        found.is_empty(),
        "契約から外れた箇所:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    for (src, needle) in [
        (&s.cli_main, "fn mod_ui_local("),
        (&s.control_mod, "pub fn run("),
        (&s.control_mod, "fn band_view("),
        (&s.control_mod, "fn accept_report("),
        (&s.mcp_request, "\"tako_mod\" => {"),
        (&s.cli_setup, "fn review_mod_ui("),
        (&s.control_setup, "    pub fn validate(&self)"),
        (&s.core, "pub fn save_to("),
        (&s.core, "pub fn load_from("),
    ] {
        assert!(src.contains(needle), "{needle} が見つからない");
    }
    // 窓が関数の本体を含む（閉じが早すぎない）
    let (_, w) = window(&s.control_mod, "pub fn run(", "}").unwrap();
    assert!(w.contains("\"report\" => accept_report("), "{w}");
    let (_, w) = window(&s.cli_main, "fn mod_ui_local(", "}").unwrap();
    assert!(w.contains("print_mod_ui("), "{w}");
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. 帯の切替をメモリの中継へ戻す
    let found = mutate(&|s| {
        s.control_mod = s.control_mod.replacen(
            "crate::claude_mod_ui::run(&ui_path(host)?, &request)",
            "host.claude_mod_mut().unwrap().request_band(&request)",
            1,
        );
    });
    assert_named(&found, CONTROL_MOD, "帯のトグルの正本は ui.json");
    // 2. CLI が自前で書く
    let found = mutate(&|s| {
        s.cli_main = s.cli_main.replacen(
            "tako_control::claude_mod_ui::run(&path, &request)",
            "tako_core::claude_mod_ui::update_at(&path, &[], 0)",
            1,
        );
    });
    assert_named(
        &found,
        CLI_MAIN,
        "mod_ui_local が tako_control::claude_mod_ui::run",
    );
    assert_named(&found, CLI_MAIN, "ui.json を口の側で書いている");
    // 3. MCP が UiRequest を通さない
    let found = mutate(&|s| {
        s.mcp_request = s.mcp_request.replacen(
            "tako_core::claude_mod_ui::UiRequest",
            "serde_json::Value",
            1,
        );
    });
    assert_named(&found, MCP_REQUEST, "UiRequest");
    // 4. setup の検証を外す
    let found = mutate(&|s| {
        s.control_setup = s
            .control_setup
            .replacen("claude_mod_ui::check_answers(", "drop(", 1);
    });
    assert_named(&found, CONTROL_SETUP, "mod_ui を検証していない");
    // 5. 応答の ui を既定値の直書きへ
    let found = mutate(&|s| {
        s.control_mod = s.control_mod.replacen(
            ".map(|p| crate::claude_mod_ui::view(&p))",
            ".map(|_| Default::default())",
            1,
        );
    });
    assert_named(&found, CONTROL_MOD, "tako.view.ui");
    // 6. 書き込みを直書きへ（一時ファイル → 差し替えをやめる）
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "std::fs::rename(&tmp, path)",
            "std::fs::copy(&tmp, path)",
            1,
        );
    });
    assert_named(&found, CORE, "一時ファイル → 差し替え");
    // 7. 退避を外す
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "crate::migration::quarantine_unreadable(path, &crate::migration::FsIo)",
            "None::<crate::migration::Quarantine>",
            1,
        );
    });
    assert_named(&found, CORE, "#916");
    // 8. 登録簿から外す
    let found = mutate(&|s| {
        s.migrations = s.migrations.replacen(
            "id: SchemaId::ClaudeModUi,",
            "id: SchemaId::RemoteDesired,",
            1,
        );
    });
    assert_named(&found, MIGRATIONS, "登録簿");
    // 9. 帯のトグルの取り込みを外す
    let found = mutate(&|s| {
        s.control_mod =
            s.control_mod
                .replacen("crate::claude_mod_ui::import_band(&path, band);", "", 1);
    });
    assert_named(&found, CONTROL_MOD, "取り込んでいない");
    // 10. setup の口が自前で書く
    let found = mutate(&|s| {
        s.cli_setup = s.cli_setup.replacen(
            "    for line in tako_control::claude_mod_ui::run_setup_stage(mod_ui_answers.as_ref()) {",
            "    let _ = tako_core::claude_mod_ui::save_to(&p, &cfg);\n    for line in [String::new()] {",
            1,
        );
    });
    assert_named(&found, CLI_SETUP, "run_setup_stage を呼ばない");
    assert_named(&found, CLI_SETUP, "ui.json を口の側で書いている");
}

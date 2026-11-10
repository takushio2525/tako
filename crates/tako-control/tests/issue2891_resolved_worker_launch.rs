//! 番犬: `tako orchestrator profiles show` / `set` の「resolved」が、worker の spawn で実際に付く値と
//! ずれない。効かない `worker_model` / `worker_effort` を黙って受け付けない（Issue #2891）
//!
//! 修正前（コードの読みと、このテストの A/B）: 応答の `resolved_worker_effort` /
//! `resolved_worker_model` は `Profile::resolve_worker_effort()` = **ポリシーの段だけ**の値で、
//! spawn が先に見る段（アカウントの既定・`worker_agents.<agent>`）を見ていなかった。setup が作る既定の
//! プロファイル（inherit + `worker_agents.claude.effort: high`）で `--agent claude --agent-effort medium`
//! と頼むと、spawn は `--effort medium` で起こすのに応答は `"resolved_worker_effort": "high"` のまま。
//! 名前から選びやすい `--worker-effort medium` は inherit では使われないのに、断りも注記も出なかった。
//!
//! いまの扱い: 「resolved」は spawn と同じ 1 実装（`Profile::plan_worker_launch_typed`）が解決した値。
//! 系統ごとの値と出どころは `resolved_workers.<系統>`（spawn の応答の `model_source` / `effort_source` と
//! 同じ語）。既定の worker に使われない `worker_model` / `worker_effort` は応答の `warnings` に日英の注記
//! （`ReplyNote::WorkerSettingUnused`）。
//!
//! 2 本立て（片方だけでは穴が残る）:
//!
//! 1. [`resolvedはspawnの起動値と一致し効かない指定は注記が出る`] — 実際の経路で突き合わせる。MCP の
//!    `tools/call`（`mcp::handle_message` → `build_request` → `dispatch`。GUI の代わりのホスト）で
//!    プロファイルを `set` / `show` し、同じプロファイルの master から `tako_orchestrator_spawn` を
//!    系統ごとに呼ぶ。spawn の応答の `model` / `effort` / 出どころ・**起動コマンドの `--model` / `--effort`**
//!    と、show の `resolved_*` が全部一致するかをケースごとに名指しで集める（ケースの期待値も固定するので、
//!    両方が同じように間違っても落ちる）
//! 2. [`resolvedとspawnは同じ解決の1実装を通る`] — ソースの形。`profile_to_json` がポリシーの段だけの
//!    解決（`resolve_worker_effort()` / `resolve_worker_model()`）を直に載せる形と、spawn が共有の解決を
//!    通らずに段を自前で並べる形を file:line で名指す
//!
//! env（data dir・HOME・表示言語）はプロセス全体の状態なので、1 は `run_alone!` の子の中で回す（#2400）。
//! claude / codex / agy の所在確認は `TAKO_983_LEGACY=1` で外す（本物を探さない・起動しない）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{set_lang, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const PROJECT: &str = "p2891";
const AGENTS: [&str; 3] = ["claude", "codex", "agy"];

/// setup が作る既定のプロファイル（Issue の再現の土台）
const SETUP_DEFAULT: &str = "master_agent: claude\neffort: high\nworker_model_policy: inherit\n\
worker_agent: claude\nworker_agents:\n  claude:\n    effort: high\nbypass_sandbox: false\n";

/// worker の既定のアカウント（claude の語彙の既定を持つ。#504 / #1013）
const ACCOUNTS: &str = "accounts:\n  acct2891:\n    inherit: true\n    \
default_model: claude-acct-2891\n    default_effort: low\n";

/// 1 ケース: 最初のプロファイル・`set` の引数（MCP の arguments）・期待
struct Case {
    name: &'static str,
    yaml: &'static str,
    set: Option<Value>,
    /// 既定の worker の spawn の期待（系統・model・effort・effort の出どころ）
    agent: &'static str,
    model: Option<&'static str>,
    effort: Option<&'static str>,
    effort_source: &'static str,
    /// 出るべき注記（`worker_effort（…）は` の頭と、理由の目印）。空 = 出ない
    notes: &'static [(&'static str, &'static str)],
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "setup の既定",
            yaml: SETUP_DEFAULT,
            set: None,
            agent: "claude",
            model: None,
            effort: Some("high"),
            effort_source: "agent_config",
            notes: &[],
        },
        // Issue の再現 1
        Case {
            name: "再現1 inherit に --worker-effort medium",
            yaml: SETUP_DEFAULT,
            set: Some(json!({ "worker_effort": "medium" })),
            agent: "claude",
            model: None,
            effort: Some("high"),
            effort_source: "agent_config",
            notes: &[(
                "worker_effort（medium）は",
                "worker_model_policy が inherit のあいだ使われません",
            )],
        },
        // Issue の再現 2（修正前は resolved が high のまま = spawn の medium とずれる）
        Case {
            name: "再現2 --agent claude --agent-effort medium",
            yaml: SETUP_DEFAULT,
            set: Some(json!({ "agent": "claude", "agent_effort": "medium" })),
            agent: "claude",
            model: None,
            effort: Some("medium"),
            effort_source: "agent_config",
            notes: &[],
        },
        // fixed にしても worker_agents.claude.effort が先に効く（setup の既定のまま fixed にした形）
        Case {
            name: "fixed でも worker_agents が先",
            yaml: SETUP_DEFAULT,
            set: Some(json!({ "worker_model_policy": "fixed", "worker_effort": "medium" })),
            agent: "claude",
            model: None,
            effort: Some("high"),
            effort_source: "agent_config",
            notes: &[("worker_effort（medium）は", "上の段が先に決める")],
        },
        // fixed で上の段が無い = worker_effort / worker_model がそのまま使われる（注記なし）
        Case {
            name: "fixed で使われる",
            yaml: "effort: high\nworker_model_policy: fixed\nworker_effort: medium\n\
worker_model: claude-fixed-2891\n",
            set: None,
            agent: "claude",
            model: Some("claude-fixed-2891"),
            effort: Some("medium"),
            effort_source: "profile_policy",
            notes: &[],
        },
        Case {
            name: "delegate の worker_effort",
            yaml: "effort: max\nworker_model_policy: delegate\nworker_effort: low\n",
            set: None,
            agent: "claude",
            model: None,
            effort: Some("max"),
            effort_source: "profile_policy",
            notes: &[(
                "worker_effort（low）は",
                "worker_model_policy が delegate のあいだ使われません",
            )],
        },
        // アカウントの既定は worker_agents より先（claude だけ。#1013）
        Case {
            name: "worker のアカウントの既定",
            yaml: "effort: high\nworker_model_policy: fixed\nworker_effort: medium\n\
worker_account: acct2891\n",
            set: None,
            agent: "claude",
            model: Some("claude-acct-2891"),
            effort: Some("low"),
            effort_source: "account",
            notes: &[(
                "worker_effort（medium）は",
                "アカウント acct2891 の default_effort",
            )],
        },
        // master が claude 以外なら master の effort は claude worker へ継承しない（#127）
        Case {
            name: "codex の master",
            yaml: "master_agent: codex\neffort: xhigh\nworker_model_policy: inherit\n",
            set: None,
            agent: "claude",
            model: None,
            effort: Some("max"),
            effort_source: "profile_policy",
            notes: &[],
        },
        // 既定の worker が codex: claude の語彙の worker_effort は渡さない
        Case {
            name: "既定の worker が codex",
            yaml: "effort: high\nworker_model_policy: fixed\nworker_effort: high\n\
worker_agent: codex\nworker_agents:\n  codex:\n    model: gpt-2891\n    effort: medium\n",
            set: None,
            agent: "codex",
            model: Some("gpt-2891"),
            effort: Some("medium"),
            effort_source: "agent_config",
            notes: &[(
                "worker_effort（high）は",
                "既定の worker（codex）には渡しません",
            )],
        },
    ]
}

/// GUI の代わり: ペインの木だけ
struct Host {
    workspace: Workspace,
}

impl Host {
    fn new(role: &str) -> Self {
        let mut workspace = Workspace::new("1", Pane::new(PaneOrigin::User));
        let id = workspace.active_tab().tree().focused();
        workspace
            .active_tab_mut()
            .tree_mut()
            .get_mut(id)
            .expect("root")
            .set_role(Some(role.to_string()));
        Self { workspace }
    }

    fn root(&self) -> u64 {
        self.workspace.active_tab().tree().focused().as_u64()
    }
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

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

/// MCP の `tools/call` を 1 件通す（`mcp::handle_message` → `build_request` → dispatch）。
/// 応答の本文（`content[0].text` の JSON）を返す。ツールの断りは Err
fn mcp_call(
    host: &mut Host,
    caller_role: Option<&str>,
    tool: &str,
    arguments: Value,
) -> Result<Value, String> {
    let caller_pane = Some(host.root());
    let mut exec = |req: Request| {
        tako_control::dispatch::dispatch(host, req, PaneOrigin::User).map_err(|e| e.to_string())
    };
    let mut session = tako_control::mcp::McpSession {
        caller_pane,
        caller_role: caller_role.map(str::to_string),
        connected: true,
        exec: &mut exec,
    };
    let msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let reply = tako_control::mcp::handle_message(&msg, &mut session).expect("応答がある");
    if let Some(err) = reply.get("error") {
        return Err(err.to_string());
    }
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        return Err(text.to_string());
    }
    serde_json::from_str(text).map_err(|e| format!("応答が JSON でない（{e}）: {text}"))
}

/// プロファイル `name` を `yaml` で書く
fn write_profile(data: &Path, name: &str, yaml: &str) {
    let dir = data.join("orchestrator").join("profiles");
    std::fs::create_dir_all(&dir).expect("profiles/");
    std::fs::write(dir.join(format!("{name}.yaml")), yaml).expect("プロファイルを書ける");
}

fn profiles_args(action: &str, name: &str, extra: Option<&Value>) -> Value {
    let mut args = json!({ "action": action, "name": name });
    if let Some(Value::Object(m)) = extra {
        for (k, v) in m {
            args[k] = v.clone();
        }
    }
    args
}

/// プロファイル `name` の master から spawn する（`agent` = None は系統の指定なし）
fn spawn(name: &str, agent: Option<&str>) -> Result<Value, String> {
    let mut host = Host::new(&format!("orchestrator-master:{name}"));
    let mut args = json!({ "project": PROJECT, "prompt": "2891" });
    if let Some(a) = agent {
        args["agent"] = json!(a);
    }
    mcp_call(
        &mut host,
        Some(&format!("master:{name}")),
        "tako_orchestrator_spawn",
        args,
    )
}

/// 起動コマンドに載った claude の `--model` / `--effort` の値（無ければ None）
fn claude_flag(command: &str, flag: &str) -> Option<String> {
    let at = command.find(&format!(" {flag} "))?;
    let rest = &command[at + flag.len() + 2..];
    let token = rest.split_whitespace().next()?;
    Some(token.trim_matches('\'').trim_matches('"').to_string())
}

fn warnings(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str().map(str::to_string))
        .collect()
}

/// `worker_model` / `worker_effort` の注記（#2891 の文の頭）
fn unused_notes(v: &Value, lang: Lang) -> Vec<String> {
    let heads: &[&str] = match lang {
        Lang::Ja => &["worker_effort（", "worker_model（"],
        Lang::En => &["worker_effort (", "worker_model ("],
    };
    warnings(v)
        .into_iter()
        .filter(|w| heads.iter().any(|h| w.starts_with(h)))
        .collect()
}

/// 1 ケースを確かめ、ずれを `failures` へ名指しで足す
fn check_case(data: &Path, idx: usize, case: &Case, failures: &mut Vec<String>) {
    let name = format!("c{idx}");
    let tag = format!("[{}]", case.name);
    write_profile(data, &name, case.yaml);
    let mut host = Host::new("orchestrator-master");
    let set_reply = case.set.as_ref().map(|extra| {
        mcp_call(
            &mut host,
            None,
            "tako_orchestrator_profiles",
            profiles_args("set", &name, Some(extra)),
        )
        .unwrap_or_else(|e| panic!("{tag} set が断られた: {e}"))
    });
    let show = mcp_call(
        &mut host,
        None,
        "tako_orchestrator_profiles",
        profiles_args("show", &name, None),
    )
    .unwrap_or_else(|e| panic!("{tag} show が断られた: {e}"));

    // set の応答（set の直後に読む値）と show は同じ resolved を返す
    if let Some(set) = &set_reply {
        for key in [
            "resolved_worker_model",
            "resolved_worker_effort",
            "resolved_worker_agent",
            "resolved_workers",
        ] {
            if set[key] != show[key] {
                failures.push(format!(
                    "{tag} set と show の {key} が違う: set={} show={}",
                    set[key], show[key]
                ));
            }
        }
    }

    // 既定の worker（系統の指定なし）: show の resolved と spawn の応答・起動コマンド
    let sp = spawn(&name, None).unwrap_or_else(|e| panic!("{tag} spawn が断られた: {e}"));
    let pairs = [
        ("resolved_worker_agent", "agent"),
        ("resolved_worker_model", "model"),
        ("resolved_worker_effort", "effort"),
    ];
    for (shown, spawned) in pairs {
        if show[shown] != sp[spawned] {
            failures.push(format!(
                "{tag} show の {shown}={} が spawn の {spawned}={} とずれる",
                show[shown], sp[spawned]
            ));
        }
    }
    // ケースの期待（両方が同じように間違っていても落とす）
    let expect = [
        ("agent", json!(case.agent)),
        ("model", json!(case.model)),
        ("effort", json!(case.effort)),
        ("effort_source", json!(case.effort_source)),
    ];
    for (key, want) in expect {
        if sp[key] != want {
            failures.push(format!(
                "{tag} spawn の {key} が期待と違う: want={want} got={}",
                sp[key]
            ));
        }
    }
    // 実際の起動コマンド（ペインへ打つ行）の --model / --effort（claude だけ。他系統は書式が違う）
    if sp["agent"] == "claude" {
        let command = sp["command"].as_str().unwrap_or_default();
        for (flag, key) in [
            ("--model", "resolved_worker_model"),
            ("--effort", "resolved_worker_effort"),
        ] {
            let got = claude_flag(command, flag);
            let want = show[key].as_str().map(str::to_string);
            if got != want {
                failures.push(format!(
                    "{tag} 起動コマンドの {flag} が show の {key} と違う: command={got:?} show={want:?}\n  {command}"
                ));
            }
        }
    }

    // 系統ごと: show の resolved_workers.<系統> と spawn --agent <系統>
    for agent in AGENTS {
        let sp = spawn(&name, Some(agent))
            .unwrap_or_else(|e| panic!("{tag} spawn --agent {agent} が断られた: {e}"));
        let shown = &show["resolved_workers"][agent];
        for key in ["model", "model_source", "effort", "effort_source"] {
            if shown[key] != sp[key] {
                failures.push(format!(
                    "{tag} show の resolved_workers.{agent}.{key}={} が spawn --agent {agent} の {key}={} とずれる",
                    shown[key], sp[key]
                ));
            }
        }
    }

    // 注記: 期待どおりの数・理由で出る（出ないケースは 0 件）
    let notes = unused_notes(&show, Lang::Ja);
    if notes.len() != case.notes.len() {
        failures.push(format!(
            "{tag} worker_model / worker_effort の注記の数が違う: want={} got={notes:#?}",
            case.notes.len()
        ));
    }
    for (head, marker) in case.notes {
        if !notes
            .iter()
            .any(|n| n.starts_with(head) && n.contains(marker))
        {
            failures.push(format!("{tag} 注記「{head}…{marker}」が無い: {notes:#?}"));
        }
    }
    if let Some(set) = &set_reply {
        if unused_notes(set, Lang::Ja) != notes {
            failures.push(format!(
                "{tag} set の応答の注記が show と違う: set={:#?} show={notes:#?}",
                unused_notes(set, Lang::Ja)
            ));
        }
    }
}

#[test]
fn resolvedはspawnの起動値と一致し効かない指定は注記が出る() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(resolvedはspawnの起動値と一致し効かない指定は注記が出る)
    {
        return;
    }
    let scratch = ScratchDir::new("tako-2891");
    assert!(
        scratch.path().starts_with(std::env::temp_dir()),
        "一時 dir の外に書こうとしている: {}",
        scratch.path().display()
    );
    let data = scratch.path().join("data");
    let home = scratch.path().join("home");
    let work = scratch.path().join("work");
    for d in [&data, &home, &work] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
    }
    for key in [
        "TAKO_SOCKET",
        "TAKO_PANE_ID",
        "TAKO_TOKEN",
        "TAKO_ORCHESTRATOR_DIR",
        "TAKO_ORCHESTRATOR_ROLE",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "TAKO_1013_LEGACY",
        "TAKO_1002_LEGACY",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::set_var("HOME", &home);
    std::env::set_var("TAKO_ISOLATED", "1");
    // claude / codex / agy の所在確認（ログインシェル）を外す
    std::env::set_var("TAKO_983_LEGACY", "1");
    set_lang(Lang::Ja);
    let orch = data.join("orchestrator");
    std::fs::create_dir_all(&orch).expect("orchestrator/");
    std::fs::write(
        orch.join("projects.yaml"),
        format!("projects:\n  {PROJECT}:\n    cwd: '{}'\n", work.display()),
    )
    .expect("projects.yaml");
    std::fs::write(orch.join("accounts.yaml"), ACCOUNTS).expect("accounts.yaml");
    // 実行時の自動移行（#916。1 プロセス 1 回）を先に済ませる（最初のケースだけ移行が割り込まない）
    let _ = tako_control::migrations::ensure_migrated();

    let mut failures = Vec::new();
    for (idx, case) in cases().iter().enumerate() {
        check_case(&data, idx, case, &mut failures);
    }

    // 英語表示: 同じ注記が英語の文で出る（日本語が混ざらない）。fixed で使われるときは出ない
    set_lang(Lang::En);
    let mut host = Host::new("orchestrator-master");
    write_profile(&data, "en", SETUP_DEFAULT);
    let en = mcp_call(
        &mut host,
        None,
        "tako_orchestrator_profiles",
        profiles_args("set", "en", Some(&json!({ "worker_effort": "medium" }))),
    )
    .expect("英語表示の set");
    let notes = unused_notes(&en, Lang::En);
    let want = "worker_effort (medium) is not used while worker_model_policy is inherit";
    if !(notes.len() == 1 && notes[0].starts_with(want)) {
        failures.push(format!("[英語] 注記が「{want}…」でない: {notes:#?}"));
    }
    if let Some(n) = notes.iter().find(|n| {
        n.chars().any(|c| {
            ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
        })
    }) {
        failures.push(format!("[英語] 注記に日本語が混ざる: {n}"));
    }
    set_lang(Lang::Ja);

    assert!(
        failures.is_empty(),
        "resolved と spawn の起動値のずれ / 注記の過不足（#2891）:\n{}",
        failures.join("\n")
    );
}

// ---- 2. ソースの形 ----

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// `signature` で始まる関数の行の範囲（1 始まりの行番号・コメント行を除いた本文の行）。
/// 終わりは関数の字下げと同じ深さの `}` の行
fn fn_lines<'a>(src: &'a str, signature: &str) -> Vec<(usize, &'a str)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with(signature))
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + i)
        .unwrap_or(lines.len() - 1);
    (start..=end)
        .filter(|&i| !lines[i].trim_start().starts_with("//"))
        .map(|i| (i + 1, lines[i]))
        .collect()
}

/// ソースの形の規則: (関数の頭, 通るべき入口, 置いてはいけない形と理由)
type SourceRule = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

#[test]
fn resolvedとspawnは同じ解決の1実装を通る() {
    let path = "crates/tako-control/src/dispatch.rs";
    let src = std::fs::read_to_string(crate_file("src/dispatch.rs")).expect("dispatch.rs を読む");
    let mut failures = Vec::new();
    let rules: [SourceRule; 2] = [
        (
            "fn profile_to_json(",
            "plan_worker_launch_typed",
            &[
                (
                    "resolve_worker_effort(",
                    "ポリシーの段だけの effort（worker_agents / アカウントを見ない）",
                ),
                (
                    "resolve_worker_model(",
                    "ポリシーの段だけの model（worker_agents / アカウントを見ない）",
                ),
                (
                    "resolve_agent_launch",
                    "spawn のアカウント・系統の解決を通らない",
                ),
            ],
        ),
        (
            "fn spawn_place(",
            "plan_worker_launch_typed",
            &[
                (
                    "resolve_agent_launch",
                    "段を自前で並べている（profiles show の resolved とずれる）",
                ),
                (
                    "AccountsConfig::load_typed",
                    "アカウントを自前で解決している",
                ),
                ("resolve_worker_agent_typed", "系統を自前で解決している"),
            ],
        ),
    ];
    for (signature, entry, forbidden) in rules {
        let body = fn_lines(&src, signature);
        if !body.iter().any(|(_, l)| l.contains(entry)) {
            failures.push(format!(
                "{path}:{}: {signature} が共有の解決 `{entry}` を通っていない（#2891）",
                body.first().map(|(n, _)| *n).unwrap_or(0)
            ));
        }
        for (pattern, why) in forbidden {
            for (n, line) in body.iter().filter(|(_, l)| l.contains(pattern)) {
                failures.push(format!(
                    "{path}:{n}: {signature} に `{pattern}` — {why}。\
                     worker の起動値は `Profile::plan_worker_launch_typed` の 1 実装から出す（#2891）\n  {}",
                    line.trim()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

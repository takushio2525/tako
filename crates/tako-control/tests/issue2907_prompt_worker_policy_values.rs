//! 番犬: master の system prompt の「Worker Model Policy」節・Session Identity の行と、`tako master` の
//! 起動の行（`ReplyNote::WorkerPolicy*`）が、worker の spawn で実際に付く model / effort を言う（Issue #2907）
//!
//! 修正前（コードの読みと、このテストの A/B）: 3 か所とも**ポリシーの段だけ**の値
//! （`Profile::model_label()` / `effort` / `worker_model_label()` / `resolve_worker_effort()`）で組んでいた。
//! setup が作る既定のプロファイル（inherit + `worker_agents.claude.effort`）に `--agent claude
//! --agent-effort medium` を当てると、spawn は `--effort medium` で起こすのに、prompt の「Worker Model
//! Policy」は「All workers use the same model and effort as this master session … Effort: high」、同じ
//! prompt の「Available Worker Agents」は「effort medium」と書き、1 つの prompt の中で食い違った。
//! アカウントの既定（worker_agents より先に効く）も、master が claude 以外のときの非継承（#127）も、
//! 既定の worker が codex のときも、prompt の値は spawn とずれていた。
//!
//! いまの扱い: 値は spawn と同じ 1 実装（`Profile::plan_worker_launch_typed` を見せる形にした
//! `Profile::default_worker_launch`）の答え。ポリシーの文が spawn の値を言えるときは #2907 より前の文の
//! まま、言えないときは頭の 1 行を替えて上の段が決めた欄に出どころを添える。spawn が断る状態（未登録の
//! アカウント）は値の代わりに断りの理由を出し、prompt の組み立ては止めない。
//!
//! 2 本立て（片方だけでは穴が残る）:
//!
//! 1. [`promptと起動の行はspawnの起動値を言う`] — 実際の経路で突き合わせる。プロファイルを書き、
//!    `Profile::load_typed` → `build_system_prompt`（`tako master` が書く prompt と同じ組み立て）と
//!    `ReplyNote::worker_policy`（起動の行）を組み、同じプロファイルの master から MCP の
//!    `tako_orchestrator_spawn` を呼ぶ。spawn の応答・**ペインへ打つ起動コマンドの `--model` /
//!    `--effort`** と、prompt の「Worker Model Policy」の値の行・「Available Worker Agents」の行・
//!    Session Identity の行・起動の行（日英）をケースごとに名指しで突き合わせる（ケースの期待値も固定する
//!    ので、両方が同じように間違っても落ちる）
//! 2. [`promptと起動の行はspawnと同じ解決を通る`] — ソースの形。組み立ての関数がポリシーの段だけの値
//!    （`resolve_worker_effort()` / `worker_model_label()` / `self.model_label()`）を直に載せる形と、
//!    共有の解決を通らない形を file:line で名指す
//!
//! env（data dir・HOME・表示言語）はプロセス全体の状態なので、1 は `run_alone!` の子の中で回す（#2400）。
//! claude / codex / agy の所在確認は `TAKO_983_LEGACY=1` で外す（本物を探さない・起動しない）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::orchestrator::Profile;
use tako_control::protocol::Request;
use tako_control::ReplyNote;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{set_lang, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const PROJECT: &str = "p2907";

/// worker のアカウント（claude の語彙の既定を持つ。#504 / #1013）
const ACCOUNTS: &str = "accounts:\n  acct2907:\n    inherit: true\n    \
default_model: claude-acct-2907\n    default_effort: low\n";

/// 1 ケース: プロファイル・spawn の期待（None = spawn が断る）・prompt の頭の文
struct Case {
    name: &'static str,
    yaml: &'static str,
    /// 既定の worker の spawn の期待（系統・model・effort）。None = 断る（未登録のアカウント）
    spawn: Option<(&'static str, Option<&'static str>, Option<&'static str>)>,
    /// prompt の「Worker Model Policy」の頭の文（#2907 より前の文が値を言えるならその文のまま）
    lead: &'static str,
    /// 値の行に添える出どころ（model / effort。None = 添えない）
    model_from: Option<&'static str>,
    effort_from: Option<&'static str>,
}

const SAME_AS_MASTER: &str = "All workers use the same model and effort as this master session:";
const FIXED: &str = "All workers use a fixed model/effort configuration:";
const SPAWN_DEFAULTS: &str = "Workers spawned without `model` / `effort` use these values:";
const DELEGATE: &str = "If you cannot determine the appropriate model, use the default:";

fn cases() -> Vec<Case> {
    vec![
        // Issue の例: setup の既定に `--agent claude --agent-effort medium` を当てた形
        Case {
            name: "Issue の例（inherit + worker_agents.claude.effort medium）",
            yaml: "master_agent: claude\neffort: high\nworker_model_policy: inherit\n\
worker_agent: claude\nworker_agents:\n  claude:\n    effort: medium\nbypass_sandbox: false\n",
            spawn: Some(("claude", None, Some("medium"))),
            lead: SPAWN_DEFAULTS,
            model_from: None,
            effort_from: Some("(from `worker_agents.claude`)"),
        },
        // 上の段が無い inherit = #2907 より前の文のまま（値も master と同じ）
        Case {
            name: "inherit（上の段なし）",
            yaml: "model: claude-m-2907\neffort: high\nworker_model_policy: inherit\n",
            spawn: Some(("claude", Some("claude-m-2907"), Some("high"))),
            lead: SAME_AS_MASTER,
            model_from: None,
            effort_from: None,
        },
        Case {
            name: "fixed（上の段なし）",
            yaml: "effort: max\nworker_model_policy: fixed\nworker_model: claude-fixed-2907\n\
worker_effort: medium\n",
            spawn: Some(("claude", Some("claude-fixed-2907"), Some("medium"))),
            lead: FIXED,
            model_from: None,
            effort_from: None,
        },
        // fixed でも worker_agents.claude.effort が先（setup の既定のまま fixed にした形。#2891）
        Case {
            name: "fixed + worker_agents.claude.effort",
            yaml: "effort: max\nworker_model_policy: fixed\nworker_effort: medium\n\
worker_agents:\n  claude:\n    effort: high\n",
            spawn: Some(("claude", None, Some("high"))),
            lead: SPAWN_DEFAULTS,
            model_from: None,
            effort_from: Some("(from `worker_agents.claude`)"),
        },
        Case {
            name: "delegate（上の段なし）",
            yaml: "model: claude-d-2907\neffort: max\nworker_model_policy: delegate\n",
            spawn: Some(("claude", Some("claude-d-2907"), Some("max"))),
            lead: DELEGATE,
            model_from: None,
            effort_from: None,
        },
        Case {
            name: "delegate + worker_agents.claude.effort",
            yaml: "effort: max\nworker_model_policy: delegate\nworker_agents:\n  claude:\n    \
effort: low\n",
            spawn: Some(("claude", None, Some("low"))),
            lead: DELEGATE,
            model_from: None,
            effort_from: Some("(from `worker_agents.claude`)"),
        },
        // アカウントの既定は worker_agents より先（claude だけ。#1013）
        Case {
            name: "アカウントの既定（inherit + worker_agents）",
            yaml: "effort: high\nworker_model_policy: inherit\nworker_account: acct2907\n\
worker_agents:\n  claude:\n    effort: medium\n",
            spawn: Some(("claude", Some("claude-acct-2907"), Some("low"))),
            lead: SPAWN_DEFAULTS,
            model_from: Some("(from account `acct2907`)"),
            effort_from: Some("(from account `acct2907`)"),
        },
        // master が claude 以外なら master の model / effort は claude worker へ継承しない（#127）
        Case {
            name: "codex の master（inherit）",
            yaml: "master_agent: codex\nmodel: gpt-m-2907\neffort: xhigh\n\
worker_model_policy: inherit\n",
            spawn: Some(("claude", None, Some("max"))),
            lead: SPAWN_DEFAULTS,
            model_from: None,
            effort_from: None,
        },
        // 既定の worker が codex: 値は codex の設定と CLI の既定
        Case {
            name: "既定の worker が codex",
            yaml: "effort: high\nworker_model_policy: inherit\nworker_agent: codex\n\
worker_agents:\n  codex:\n    model: gpt-2907\n",
            spawn: Some(("codex", Some("gpt-2907"), None)),
            lead: SPAWN_DEFAULTS,
            model_from: Some("(from `worker_agents.codex`)"),
            effort_from: None,
        },
        // spawn が断る状態でも prompt と起動の行は組める（値の代わりに断りの理由）
        Case {
            name: "未登録のアカウント",
            yaml: "effort: high\nworker_model_policy: inherit\nworker_account: ghost2907\n",
            spawn: None,
            lead: SPAWN_DEFAULTS,
            model_from: None,
            effort_from: None,
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

/// プロファイル `name` の master から MCP の `tools/call` で spawn する（`mcp::handle_message` →
/// `build_request` → dispatch）。応答の本文（`content[0].text` の JSON）を返す。ツールの断りは Err
fn spawn(name: &str) -> Result<Value, String> {
    let mut host = Host::new(&format!("orchestrator-master:{name}"));
    let caller_pane = Some(host.root());
    let mut exec = |req: Request| {
        tako_control::dispatch::dispatch(&mut host, req, PaneOrigin::User)
            .map_err(|e| e.to_string())
    };
    let mut session = tako_control::mcp::McpSession {
        caller_pane,
        caller_role: Some(format!("master:{name}")),
        connected: true,
        exec: &mut exec,
    };
    let msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tako_orchestrator_spawn",
            "arguments": { "project": PROJECT, "prompt": "2907" },
        },
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

/// 起動コマンドに載った claude の `--model` / `--effort` の値（無ければ None）
fn claude_flag(command: &str, flag: &str) -> Option<String> {
    let at = command.find(&format!(" {flag} "))?;
    let rest = &command[at + flag.len() + 2..];
    let token = rest.split_whitespace().next()?;
    Some(token.trim_matches('\'').trim_matches('"').to_string())
}

/// 値の表示（値が無い = その系統の CLI の既定）
fn label(agent: &str, value: Option<&str>) -> String {
    value.map_or_else(|| format!("({agent} CLI default)"), str::to_string)
}

/// prompt の「Worker Model Policy」節（`## Worker Model Policy` から次の `## ` の見出しの手前まで。
/// 中の `### Available Worker Agents` を含む）
fn policy_section(prompt: &str) -> Option<&str> {
    let start = prompt.find("## Worker Model Policy")?;
    let rest = &prompt[start..];
    let end = rest[3..].find("\n## ").map(|i| i + 3).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// 節の中の `- **<key>**: <値>` の値（出どころの括弧まで含む行の残り）
fn bullet<'a>(section: &'a str, key: &str) -> Option<&'a str> {
    let head = format!("- **{key}**: ");
    section.lines().find_map(|l| l.strip_prefix(head.as_str()))
}

fn has_japanese(s: &str) -> bool {
    s.chars()
        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// 1 ケースを確かめ、ずれを `failures` へ名指しで足す
fn check_case(data: &Path, idx: usize, case: &Case, failures: &mut Vec<String>) {
    let name = format!("c{idx}");
    let tag = format!("[{}]", case.name);
    write_profile(data, &name, case.yaml);
    let profile =
        Profile::load_typed(&name).unwrap_or_else(|e| panic!("{tag} プロファイルを読めない: {e}"));
    // `tako master` が書く prompt と同じ組み立て（止まらずに組めること自体も確かめる）
    let prompt = profile.build_system_prompt(&name);
    let Some(section) = policy_section(&prompt) else {
        failures.push(format!("{tag} prompt に Worker Model Policy 節が無い"));
        return;
    };
    let identity = prompt
        .lines()
        .find_map(|l| l.strip_prefix("- **Worker model policy**: "))
        .unwrap_or_default()
        .to_string();
    let line_ja = ReplyNote::worker_policy(&profile).text_in(Lang::Ja);
    let line_en = ReplyNote::worker_policy(&profile).text_in(Lang::En);
    let delegate = case.lead == DELEGATE;
    let prefix = if delegate { "Default " } else { "" };

    // 頭の文: ポリシーの文が値を言えるときだけ #2907 より前の文
    if !section.contains(case.lead) {
        failures.push(format!(
            "{tag} Worker Model Policy の頭の文が「{}」でない:\n{section}",
            case.lead
        ));
    }
    for claim in [SAME_AS_MASTER, FIXED] {
        if claim != case.lead && section.contains(claim) {
            failures.push(format!(
                "{tag} spawn の値を言えない文「{claim}」が残っている:\n{section}"
            ));
        }
    }

    let sp = spawn(&name);
    let Some((agent, want_model, want_effort)) = case.spawn else {
        // 断る状態: spawn は断り、prompt と起動の行は断りの理由を言う
        if let Ok(v) = &sp {
            failures.push(format!("{tag} spawn が断らなかった: {v}"));
        }
        let want = "account 'ghost2907' not found in accounts.yaml";
        let row = bullet(section, &format!("{prefix}Model / Effort")).unwrap_or_default();
        if !row.contains(want) {
            failures.push(format!(
                "{tag} 値の行が断りの理由「{want}」を言わない:\n{section}"
            ));
        }
        for (what, text, reason) in [
            (
                "Session Identity",
                identity.as_str(),
                "アカウント 'ghost2907' が accounts.yaml に見つからない",
            ),
            (
                "起動の行（日）",
                line_ja.as_str(),
                "アカウント 'ghost2907' が accounts.yaml に見つからない",
            ),
            ("起動の行（英）", line_en.as_str(), want),
        ] {
            if !text.contains(reason) {
                failures.push(format!("{tag} {what} が断りの理由を言わない: {text}"));
            }
        }
        return;
    };
    let sp = match sp {
        Ok(v) => v,
        Err(e) => {
            failures.push(format!("{tag} spawn が断られた: {e}"));
            return;
        }
    };

    // spawn の実値（claude はペインへ打つ起動コマンドの --model / --effort。他系統は応答の値）
    let (got_model, got_effort) = if sp["agent"] == "claude" {
        let command = sp["command"].as_str().unwrap_or_default();
        (
            claude_flag(command, "--model"),
            claude_flag(command, "--effort"),
        )
    } else {
        (
            sp["model"].as_str().map(str::to_string),
            sp["effort"].as_str().map(str::to_string),
        )
    };
    // ケースの期待（prompt と spawn が同じように間違っていても落とす）
    let want = (
        json!(agent),
        want_model.map(str::to_string),
        want_effort.map(str::to_string),
    );
    if (sp["agent"].clone(), got_model.clone(), got_effort.clone()) != want {
        failures.push(format!(
            "{tag} spawn の起動値が期待と違う: want={want:?} got=({}, {got_model:?}, {got_effort:?})",
            sp["agent"]
        ));
    }
    let model = label(agent, got_model.as_deref());
    let effort = label(agent, got_effort.as_deref());

    // 1. Worker Model Policy の値の行 = spawn の実値（+ 上の段の出どころ）
    for (key, value, from) in [
        (format!("{prefix}Model"), &model, case.model_from),
        (format!("{prefix}Effort"), &effort, case.effort_from),
    ] {
        let want = match from {
            Some(f) => format!("{value} {f}"),
            None => value.clone(),
        };
        match bullet(section, &key) {
            Some(row) if row == want => {}
            row => failures.push(format!(
                "{tag} Worker Model Policy の {key} が spawn の値と違う: prompt={row:?} spawn={want:?}"
            )),
        }
    }

    // 2. Available Worker Agents の既定の系統の行（節がある = worker_agent / worker_agents を書いたとき）
    if let Some(row) = section
        .lines()
        .find(|l| l.starts_with(&format!("- `{agent}`: ")))
    {
        let model_ok = match &got_model {
            Some(m) => row.contains(&format!("model `{m}`")) || row.contains(&format!("model {m}")),
            None => row.contains("CLI default)"),
        };
        if !model_ok {
            failures.push(format!(
                "{tag} Available Worker Agents の {agent} の model が spawn の {model} と違う: {row}"
            ));
        }
        if let Some(at) = row.find("effort ") {
            let shown = row[at + 7..].split(',').next().unwrap_or_default().trim();
            if Some(shown) != got_effort.as_deref() {
                failures.push(format!(
                    "{tag} Available Worker Agents の {agent} の effort {shown} が spawn の {effort} と違う: {row}"
                ));
            }
        } else if case.effort_from.is_some() {
            failures.push(format!(
                "{tag} Available Worker Agents の {agent} に上の段の effort {effort} が無い: {row}"
            ));
        }
    }

    // 3. Session Identity の行と起動の行（日英）: delegate は値を出さない
    let pair = format!("{model} / {effort}");
    let same_as_master = case.lead == SAME_AS_MASTER;
    for (what, text) in [
        ("Session Identity", identity.as_str()),
        ("起動の行（日）", line_ja.as_str()),
        ("起動の行（英）", line_en.as_str()),
    ] {
        if !delegate && !text.contains(&pair) {
            failures.push(format!(
                "{tag} {what} が spawn の値「{pair}」を言わない: {text}"
            ));
        }
        if !same_as_master && (text.contains("master と同じ") || text.contains("same as master"))
        {
            failures.push(format!(
                "{tag} {what} が spawn の値を言えない「master と同じ」を名乗る: {text}"
            ));
        }
    }
    if has_japanese(&line_en) {
        failures.push(format!("{tag} 起動の行（英）に日本語が混ざる: {line_en}"));
    }
    if !line_ja.starts_with("worker モデルポリシー: ")
        || !line_en.starts_with("worker model policy: ")
    {
        failures.push(format!(
            "{tag} 起動の行の見出しが日英で違う: ja={line_ja} en={line_en}"
        ));
    }
}

#[test]
fn promptと起動の行はspawnの起動値を言う() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(promptと起動の行はspawnの起動値を言う) {
        return;
    }
    let scratch = ScratchDir::new("tako-2907");
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
        "TAKO_1477_LEGACY",
        "TAKO_1154_LEGACY",
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
    assert!(
        failures.is_empty(),
        "prompt / 起動の行の worker の値が spawn の起動値とずれる（#2907）:\n{}",
        failures.join("\n")
    );
}

// ---- 2. ソースの形 ----

fn repo_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
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

/// ソースの形の規則: (ファイル, 関数の頭, 通るべき入口, 置いてはいけない形と理由)
type SourceRule = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// ポリシーの段だけの値（worker_agents / アカウントを見ない）
const POLICY_ONLY: &[(&str, &str)] = &[
    (
        "resolve_worker_effort(",
        "ポリシーの段だけの effort（worker_agents / アカウントを見ない）",
    ),
    (
        "resolve_worker_model(",
        "ポリシーの段だけの model（worker_agents / アカウントを見ない）",
    ),
    (
        "worker_model_label(",
        "ポリシーの段だけの model（worker_agents / アカウントを見ない）",
    ),
    // `master_model_label()`（Session Identity の master の行）には当てない
    (
        "self.model_label()",
        "master の model（spawn の値ではない）",
    ),
    (
        "profile.model_label()",
        "master の model（spawn の値ではない）",
    ),
];

#[test]
fn promptと起動の行はspawnと同じ解決を通る() {
    let orch = "crates/tako-control/src/orchestrator/mod.rs";
    let note = "crates/tako-control/src/reply_note.rs";
    let cli = "crates/tako-cli/src/main.rs";
    let rules: [SourceRule; 5] = [
        (
            note,
            "pub fn worker_policy(",
            "default_worker_launch",
            POLICY_ONLY,
        ),
        (
            orch,
            "fn generate_model_policy_base(",
            "default_worker_launch",
            POLICY_ONLY,
        ),
        (
            orch,
            "fn generate_worker_agents_section(",
            "plan_worker_launch_typed",
            POLICY_ONLY,
        ),
        (
            orch,
            "fn generate_identity_section(",
            "ReplyNote::worker_policy",
            POLICY_ONLY,
        ),
        (
            cli,
            "fn orchestrator_master(",
            "ReplyNote::worker_policy",
            &[
                (
                    "resolve_worker_effort(",
                    "ポリシーの段だけの effort（worker_agents / アカウントを見ない）",
                ),
                (
                    "worker_model_label(",
                    "ポリシーの段だけの model（worker_agents / アカウントを見ない）",
                ),
                (
                    "ReplyNote::WorkerPolicy",
                    "起動の行を自前で組んでいる（system prompt の Session Identity とずれる）",
                ),
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (path, signature, entry, forbidden) in rules {
        let src = std::fs::read_to_string(repo_file(path))
            .unwrap_or_else(|e| panic!("{path} を読めない: {e}"));
        let body = fn_lines(&src, signature);
        if !body.iter().any(|(_, l)| l.contains(entry)) {
            failures.push(format!(
                "{path}:{}: {signature} が共有の解決 `{entry}` を通っていない（#2907）",
                body.first().map(|(n, _)| *n).unwrap_or(0)
            ));
        }
        for (pattern, why) in forbidden {
            for (n, line) in body.iter().filter(|(_, l)| l.contains(pattern)) {
                failures.push(format!(
                    "{path}:{n}: {signature} に `{pattern}` — {why}。\
                     worker の値は spawn と同じ 1 実装（`Profile::default_worker_launch` / \
                     `ReplyNote::worker_policy`）から出す（#2907）\n  {}",
                    line.trim()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

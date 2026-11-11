//! 番犬: worker の 1M コンテキスト版モデル（`[1m]`）の警告（`tako master` の起動の警告と
//! `orchestrator::profile_warnings` = `profiles list` / `show` / `set` の `warnings`）が、spawn が
//! 実際に付ける model を見る（Issue #2915）
//!
//! 修正前（コードの読みと、このテストの A/B）: 2 か所とも**ポリシーの段だけ**の model
//! （`Profile::resolve_worker_model()`）を見ていた。spawn が先に見る段（アカウントの `default_model`・
//! `worker_agents.claude.model`）で `[1m]` になっても警告は出ず、逆にポリシーの段だけ `[1m]` で spawn は
//! 別のモデルを使う（上の段が先に決める・既定の worker が codex）ときにも警告が出ていた。#2891 / #2907 で
//! `profiles show` の resolved と master の prompt・起動の行を直したのと同じ型のずれ。
//!
//! いまの扱い: 既定の worker の model は spawn と同じ 1 実装（`Profile::default_worker_launch` =
//! `plan_worker_launch_typed`）の答え。2 か所とも `orchestrator::worker_one_m_model_note` の 1 つを呼ぶ。
//! `[1m]` は claude の語彙なので既定の worker が claude の語彙を受け取る系統のときだけ見る（#1013）。
//! spawn が断る状態（未登録のアカウント）は `[1m]` で起動しないので言わない。master の警告と同じ
//! モデルなら master の分だけ（#27）。solo のプロファイル（worker を spawn しない）では言わない。
//!
//! 2 本立て（片方だけでは穴が残る）:
//!
//! 1. [`worker_の1m警告はspawnの実際のmodelと一致する`] — 実際の経路で突き合わせる。プロファイルを書き、
//!    同じプロファイルの master から MCP の `tako_orchestrator_spawn` を呼んで**ペインへ打つ起動コマンドの
//!    `--model`**（claude 以外は応答の `model`）を取り、MCP の `tako_orchestrator_profiles` の `show` の
//!    `warnings`（日英）と、`tako master` の起動の警告と同じ口（`worker_one_m_model_note`）の有無・
//!    名指すモデルを突き合わせる（ケースの期待値も固定するので、両方が同じように間違っても落ちる）
//! 2. [`worker_の1m警告はspawnと同じ解決を通る`] — ソースの形。警告を組む 3 つの関数がポリシーの段だけの
//!    model（`resolve_worker_model()` / `worker_model_label()`）を直に読む形と、共有の口を通らない形を
//!    file:line で名指す
//!
//! env（data dir・HOME・表示言語）はプロセス全体の状態なので、1 は `run_alone!` の子の中で回す（#2400）。
//! claude / codex / agy の所在確認は `TAKO_983_LEGACY=1` で外す（本物を探さない・起動しない）。
//! モデル名は #27 の移行が外す旧既定値（`claude-opus-4-6[1m]`）を使わない（移行が警告の前に消す）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::orchestrator::Profile;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{set_lang, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const PROJECT: &str = "p2915";

/// worker のアカウント（claude の語彙の既定を持つ。#504 / #1013）。
/// `acct2915` は `[1m]`、`acct2915plain` は `[1m]` でない既定のモデル
const ACCOUNTS: &str = "accounts:\n  acct2915:\n    inherit: true\n    \
default_model: claude-acct-2915[1m]\n    default_effort: low\n  acct2915plain:\n    \
inherit: true\n    default_model: claude-acct-plain-2915\n";

/// 1 ケース: プロファイルと期待
struct Case {
    name: &'static str,
    yaml: &'static str,
    /// 既定の worker の spawn の期待（系統・model）。None = 断る（未登録のアカウント）
    spawn: Option<(&'static str, Option<&'static str>)>,
    /// worker の `[1m]` 警告が出るか
    worker_warn: bool,
    /// master の `[1m]` 警告が出るか（#2915 では変えない。master の分との二重を見るため）
    master_warn: bool,
}

fn cases() -> Vec<Case> {
    vec![
        // Issue の形 1: 上の段（worker_agents.claude.model）だけ [1m] → spawn は [1m]。修正前は黙っていた
        Case {
            name: "上の段だけ [1m]（worker_agents.claude.model）",
            yaml: "model: claude-m-2915\neffort: high\nworker_model_policy: inherit\n\
worker_agents:\n  claude:\n    model: claude-wa-2915[1m]\n",
            spawn: Some(("claude", Some("claude-wa-2915[1m]"))),
            worker_warn: true,
            master_warn: false,
        },
        // Issue の形 2: ポリシーの段だけ [1m]・worker_agents.claude.model が先 → spawn は別のモデル。
        // 修正前は鳴っていた
        Case {
            name: "ポリシーの段だけ [1m]（worker_agents.claude.model が先）",
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\n\
worker_agents:\n  claude:\n    model: claude-wa-2915\n",
            spawn: Some(("claude", Some("claude-wa-2915"))),
            worker_warn: false,
            master_warn: false,
        },
        // アカウントの default_model が [1m] → spawn は [1m]。修正前は黙っていた
        Case {
            name: "アカウントの default_model が [1m]",
            yaml: "effort: high\nworker_model_policy: inherit\nworker_account: acct2915\n",
            spawn: Some(("claude", Some("claude-acct-2915[1m]"))),
            worker_warn: true,
            master_warn: false,
        },
        // worker_account が無ければ spawn は master_account の既定を使う → spawn は [1m]
        Case {
            name: "master_account の default_model が [1m]（worker_account なし）",
            yaml: "effort: high\nworker_model_policy: inherit\nmaster_account: acct2915\n",
            spawn: Some(("claude", Some("claude-acct-2915[1m]"))),
            worker_warn: true,
            master_warn: false,
        },
        // ポリシーの段だけ [1m]・アカウントの既定が先 → spawn は別のモデル。修正前は鳴っていた
        Case {
            name: "ポリシーの段だけ [1m]（アカウントの default_model が先）",
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\n\
worker_account: acct2915plain\n",
            spawn: Some(("claude", Some("claude-acct-plain-2915"))),
            worker_warn: false,
            master_warn: false,
        },
        // 上の段が無い fixed: ポリシーの段の [1m] がそのまま spawn に付く（修正前と同じく鳴る）
        Case {
            name: "fixed の worker_model が [1m]（上の段なし）",
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\n",
            spawn: Some(("claude", Some("claude-fixed-2915[1m]"))),
            worker_warn: true,
            master_warn: false,
        },
        // inherit で master と同じ [1m] → master の分だけ（#27 の二重を避ける）
        Case {
            name: "inherit で master と同じ [1m]",
            yaml: "model: claude-m-2915[1m]\neffort: high\nworker_model_policy: inherit\n",
            spawn: Some(("claude", Some("claude-m-2915[1m]"))),
            worker_warn: false,
            master_warn: true,
        },
        // master は [1m]・worker は worker_agents.claude.model の別のモデル → master の分だけ
        Case {
            name: "master は [1m]・worker_agents.claude.model は [1m] でない",
            yaml: "model: claude-m-2915[1m]\nworker_model_policy: inherit\n\
worker_agents:\n  claude:\n    model: claude-wa-2915\n",
            spawn: Some(("claude", Some("claude-wa-2915"))),
            worker_warn: false,
            master_warn: true,
        },
        // master も worker も別の [1m] → 両方（修正前は worker の分を「master と同じ」と見て落としていた）
        Case {
            name: "master と worker_agents.claude.model が別の [1m]",
            yaml: "model: claude-m-2915[1m]\nworker_model_policy: inherit\n\
worker_agents:\n  claude:\n    model: claude-wa-2915[1m]\n",
            spawn: Some(("claude", Some("claude-wa-2915[1m]"))),
            worker_warn: true,
            master_warn: true,
        },
        // 既定の worker が codex: claude の語彙の worker_model は継がない（#1013）→ 言わない。修正前は鳴っていた
        Case {
            name: "既定の worker が codex（fixed の worker_model が [1m]）",
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\nworker_agent: codex\n",
            spawn: Some(("codex", None)),
            worker_warn: false,
            master_warn: false,
        },
        // 既定の worker が codex: codex の model に `[1m]` の字面があっても claude の語彙ではないので見ない
        Case {
            name: "既定の worker が codex（worker_agents.codex.model に [1m] の字面）",
            yaml: "worker_model_policy: inherit\nworker_agent: codex\n\
worker_agents:\n  codex:\n    model: gpt-2915[1m]\n",
            spawn: Some(("codex", Some("gpt-2915[1m]"))),
            worker_warn: false,
            master_warn: false,
        },
        // 既定の worker が agy: アカウントの [1m] の既定は agy へ継がない（#1013）→ 言わない
        Case {
            name: "既定の worker が agy（アカウントの default_model が [1m]）",
            yaml: "worker_model_policy: inherit\nworker_agent: agy\nworker_account: acct2915\n",
            spawn: Some(("agy", None)),
            worker_warn: false,
            master_warn: false,
        },
        // spawn が断る状態: [1m] では起動しないので言わない（未登録のアカウントの警告は出る）
        Case {
            name: "未登録のアカウント（fixed の worker_model が [1m]）",
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\n\
worker_account: ghost2915\n",
            spawn: None,
            worker_warn: false,
            master_warn: false,
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

/// プロファイル `name` の master から既定の worker を spawn する
fn spawn(name: &str) -> Result<Value, String> {
    let mut host = Host::new(&format!("orchestrator-master:{name}"));
    mcp_call(
        &mut host,
        Some(&format!("master:{name}")),
        "tako_orchestrator_spawn",
        json!({ "project": PROJECT, "prompt": "2915" }),
    )
}

/// `profiles show` の `warnings`（表示言語は呼び手が `set_lang` で決める）
fn show_warnings(name: &str) -> Result<Vec<String>, String> {
    show_warnings_of(name, "master")
}

/// `profiles show`（`kind` = master / solo）の `warnings`
fn show_warnings_of(name: &str, kind: &str) -> Result<Vec<String>, String> {
    let mut host = Host::new("orchestrator-master");
    let v = mcp_call(
        &mut host,
        None,
        "tako_orchestrator_profiles",
        json!({ "action": "show", "name": name, "kind": kind }),
    )?;
    Ok(v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str().map(str::to_string))
        .collect())
}

/// 起動コマンドに載った claude の `--model` の値（無ければ None）
fn claude_flag(command: &str, flag: &str) -> Option<String> {
    let at = command.find(&format!(" {flag} "))?;
    let rest = &command[at + flag.len() + 2..];
    let token = rest.split_whitespace().next()?;
    Some(token.trim_matches('\'').trim_matches('"').to_string())
}

/// `[1m]` 警告の頭（`OneMModelWarning` の文。`source` = master / worker）
fn one_m_head(source: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("[警告] {source} のモデル '"),
        Lang::En => format!("[warning] the {source} model '"),
    }
}

/// 警告の列から `source` の `[1m]` 警告が名指すモデルを全部抜く
fn one_m_models(warnings: &[String], source: &str, lang: Lang) -> Vec<String> {
    let head = one_m_head(source, lang);
    warnings
        .iter()
        .filter_map(|w| w.strip_prefix(head.as_str()))
        .filter_map(|rest| rest.split('\'').next().map(str::to_string))
        .collect()
}

/// 1 ケースを確かめ、ずれを `failures` へ名指しで足す
fn check_case(data: &Path, idx: usize, case: &Case, failures: &mut Vec<String>) {
    let name = format!("c{idx}");
    let tag = format!("[{}]", case.name);
    write_profile(data, &name, case.yaml);

    // spawn の実際の model（claude はペインへ打つ起動コマンドの --model。他系統は応答の値）
    let actual: Option<(String, Option<String>)> = match (spawn(&name), case.spawn) {
        (Ok(sp), Some((want_agent, want_model))) => {
            let agent = sp["agent"].as_str().unwrap_or_default().to_string();
            let model = if agent == "claude" {
                claude_flag(sp["command"].as_str().unwrap_or_default(), "--model")
            } else {
                sp["model"].as_str().map(str::to_string)
            };
            // ケースの期待（警告と spawn が同じように間違っていても落とす）
            if (agent.as_str(), model.as_deref()) != (want_agent, want_model) {
                failures.push(format!(
                    "{tag} spawn の起動値が期待と違う: want=({want_agent}, {want_model:?}) \
                     got=({agent}, {model:?})"
                ));
            }
            Some((agent, model))
        }
        (Ok(sp), None) => {
            failures.push(format!("{tag} spawn が断らなかった: {sp}"));
            return;
        }
        (Err(e), Some(_)) => {
            failures.push(format!("{tag} spawn が断られた: {e}"));
            return;
        }
        (Err(_), None) => None,
    };
    // 規則から導いた期待（spawn の実値が claude の [1m] で、master の警告と同じモデルでない）
    let profile =
        Profile::load_typed(&name).unwrap_or_else(|e| panic!("{tag} プロファイルを読めない: {e}"));
    let master_one_m = profile
        .model
        .as_deref()
        .filter(|m| profile.master_agent_is_claude() && m.contains("[1m]"));
    let derived = actual.as_ref().and_then(|(agent, model)| {
        model
            .as_deref()
            .filter(|m| agent == "claude" && m.contains("[1m]") && Some(*m) != master_one_m)
            .map(str::to_string)
    });
    if derived.is_some() != case.worker_warn {
        failures.push(format!(
            "{tag} ケースの期待（worker_warn={}）が spawn の実値 {actual:?} と合わない",
            case.worker_warn
        ));
    }

    // 1. `tako master` の起動の警告と同じ口
    let cli = tako_control::orchestrator::worker_one_m_model_note(&profile);
    for lang in [Lang::Ja, Lang::En] {
        let got: Vec<String> = cli.iter().map(|n| n.text_in(lang)).collect();
        let named = one_m_models(&got, "worker", lang);
        if named != derived.iter().cloned().collect::<Vec<_>>() || got.len() != named.len() {
            failures.push(format!(
                "{tag} 起動の警告（{lang:?}）が spawn の実際の model とずれる: \
                 warning={got:?} spawn={actual:?}"
            ));
        }
    }

    // 2. profiles show の warnings（MCP。CLI の `profiles show` も同じ dispatch）
    for lang in [Lang::Ja, Lang::En] {
        set_lang(lang);
        let warnings = match show_warnings(&name) {
            Ok(w) => w,
            Err(e) => {
                failures.push(format!("{tag} show が断られた（{lang:?}）: {e}"));
                continue;
            }
        };
        let worker = one_m_models(&warnings, "worker", lang);
        if worker != derived.iter().cloned().collect::<Vec<_>>() {
            failures.push(format!(
                "{tag} show の warnings（{lang:?}）の worker の [1m] 警告が spawn の実際の model と\
                 ずれる: warning={worker:?} spawn={actual:?}"
            ));
        }
        let master = one_m_models(&warnings, "master", lang);
        if master.is_empty() == case.master_warn {
            failures.push(format!(
                "{tag} show の warnings（{lang:?}）の master の [1m] 警告の有無が期待と違う: \
                 want={} got={master:?}",
                case.master_warn
            ));
        }
        if case.spawn.is_none() && !warnings.iter().any(|w| w.contains("ghost2915")) {
            failures.push(format!(
                "{tag} show の warnings（{lang:?}）に未登録のアカウントの警告が無い: {warnings:?}"
            ));
        }
    }
    set_lang(Lang::Ja);
}

#[test]
fn worker_の1m警告はspawnの実際のmodelと一致する() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(worker_の1m警告はspawnの実際のmodelと一致する) {
        return;
    }
    let scratch = ScratchDir::new("tako-2915");
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

    // solo は worker を spawn しない: 同じ形（master_account の既定が [1m]・fixed の worker_model が [1m]）
    // でも worker の [1m] は言わない（master の kind では言う = 上のケース）
    let solo_dir = data.join("orchestrator").join("solo-profiles");
    std::fs::create_dir_all(&solo_dir).expect("solo-profiles/");
    std::fs::write(
        solo_dir.join("s2915.yaml"),
        "worker_model_policy: fixed\nworker_model: claude-fixed-2915[1m]\nmaster_account: acct2915\n",
    )
    .expect("solo のプロファイルを書ける");
    for lang in [Lang::Ja, Lang::En] {
        set_lang(lang);
        match show_warnings_of("s2915", "solo") {
            Ok(w) if one_m_models(&w, "worker", lang).is_empty() => {}
            Ok(w) => failures.push(format!(
                "[solo（{lang:?}）] worker を spawn しない solo に worker の [1m] 警告が出る: {w:?}"
            )),
            Err(e) => failures.push(format!("[solo（{lang:?}）] show が断られた: {e}")),
        }
    }
    set_lang(Lang::Ja);

    assert!(
        failures.is_empty(),
        "worker の [1m] 警告が spawn の実際の model とずれる（#2915）:\n{}",
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

/// ポリシーの段だけの model（アカウントの既定・worker_agents を見ない）
const POLICY_ONLY: [(&str, &str); 2] = [
    (
        "resolve_worker_model(",
        "ポリシーの段だけの model（アカウントの既定・worker_agents を見ない）",
    ),
    (
        "worker_model_label(",
        "ポリシーの段だけの model（アカウントの既定・worker_agents を見ない）",
    ),
];

#[test]
fn worker_の1m警告はspawnと同じ解決を通る() {
    let orch = "crates/tako-control/src/orchestrator/mod.rs";
    let cli = "crates/tako-cli/src/main.rs";
    // 自前で worker の [1m] 警告を組む（共有の口を通らない）形
    let own_note = (
        "\"worker\"",
        "worker の [1m] 警告を自前で組んでいる（`profiles show` と起動の警告がずれる）",
    );
    // (ファイル, 関数の頭, 通るべき入口, 置いてはいけない形と理由)
    let rules: [(&str, &str, &str, Vec<(&str, &str)>); 3] = [
        (
            orch,
            "pub fn worker_one_m_model_note(",
            "default_worker_launch",
            [
                POLICY_ONLY.to_vec(),
                vec![
                    (
                        "plan_worker_launch_typed(",
                        "既定の worker は `default_worker_launch` の 1 か所で問う（引数の省き方を書き写さない）",
                    ),
                    (
                        "worker_agents",
                        "spawn の解決の順を書き写している",
                    ),
                    (
                        "default_model",
                        "spawn の解決の順を書き写している",
                    ),
                ],
            ]
            .concat(),
        ),
        (
            orch,
            "pub fn profile_warnings(",
            "worker_one_m_model_note",
            [POLICY_ONLY.to_vec(), vec![own_note]].concat(),
        ),
        (
            cli,
            "fn orchestrator_master(",
            "worker_one_m_model_note",
            [POLICY_ONLY.to_vec(), vec![own_note]].concat(),
        ),
    ];
    let mut failures = Vec::new();
    for (path, signature, entry, forbidden) in rules {
        let src = std::fs::read_to_string(repo_file(path))
            .unwrap_or_else(|e| panic!("{path} を読めない: {e}"));
        let body = fn_lines(&src, signature);
        if !body.iter().any(|(_, l)| l.contains(entry)) {
            failures.push(format!(
                "{path}:{}: {signature} が共有の口 `{entry}` を通っていない（#2915）",
                body.first().map(|(n, _)| *n).unwrap_or(0)
            ));
        }
        for (pattern, why) in &forbidden {
            for (n, line) in body.iter().filter(|(_, l)| l.contains(pattern)) {
                failures.push(format!(
                    "{path}:{n}: {signature} に `{pattern}` — {why}。worker の [1m] 警告は spawn と同じ \
                     1 実装（`Profile::default_worker_launch`）の model を `worker_one_m_model_note` で\
                     見る（#2915）\n  {}",
                    line.trim()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

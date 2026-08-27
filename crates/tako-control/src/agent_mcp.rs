//! agent_mcp — claude 以外のエージェント CLI（codex / agy）へ tako MCP サーバーを登録する（Issue #979）
//!
//! claude 向けの登録は `dispatch::setup_mcp`（`~/.claude.json` / `.mcp.json`）にある。
//! ここは **codex / agy** の分で、書き込みは各 CLI の公式サブコマンド
//! （`codex mcp add` / `agy mcp add`）に任せる。
//!
//! ## なぜ設定ファイルを直接書かないか（実測 2026-08-27。codex 0.144.4 / agy 1.1.22）
//!
//! - **codex は書き戻しで自分の正規化をかける**: `codex mcp add` は
//!   `~/.codex/config.toml` を書き直す過程で env テーブルのキーを並べ替え、
//!   `startup_timeout_sec = 120` を `120.0` にし、`args = []` の行を落とす
//!   （トップレベルのコメントは保たれた）。tako が別の TOML ライブラリで書けば
//!   **正規化が二重にずれる**ので、利用者が自分で `codex mcp add` を打ったのと
//!   同じ結果になる公式経路へ寄せる
//! - **agy は「add or update」なので上書きが正規の手段**。再実行でファイルは
//!   バイト一致した（冪等性の実測は Issue #979）
//! - CLI が無いときに設定ファイルだけ書いても意味がない（そのエージェントを
//!   起動できない）。claude と違いフォールバックの直書きは用意せず、
//!   **未導入は分類済みエラー**にして次の一手を出す（#979 スコープ 3）
//!
//! ## 到達手段の限界（既知・docs にも書く）
//!
//! MCP 子プロセスへ per-pane の env（`TAKO_PANE_ID` 等）を渡す仕組みは
//! agy には無く（`env` は静的 map だけ）、codex も `mcp add` からは
//! `env_vars` 許可リストを設定できない。ただし `tako mcp serve` は
//! `TAKO_SOCKET` / `TAKO_TOKEN` が無ければ discovery（control.json + 生存確認）へ
//! 落ちるので、**ツール呼び出し自体は env なしで通る**。省略されるのは
//! 「呼び出し元ペインの既定解決」だけで、pane を明示すれば全操作できる。

use std::path::PathBuf;

use crate::orchestrator::agent::WorkerAgent;

/// 登録に使う MCP サーバー名（3 エージェントで共通）
pub const SERVER_NAME: &str = "tako";

/// 分類済みの失敗理由。`Display` が「何が起きたか + 次の一手」を出す（#979 スコープ 3）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMcpError {
    /// 対象 CLI が見つからない
    CliNotFound { agent: WorkerAgent },
    /// このエージェントは claude 専用のスコープを持たない
    ScopeUnsupported {
        agent: WorkerAgent,
        scope: &'static str,
    },
    /// CLI の起動そのものに失敗した（実行権限・壊れたバイナリ等）
    Spawn { agent: WorkerAgent, detail: String },
    /// CLI が非ゼロ終了した
    CommandFailed {
        agent: WorkerAgent,
        code: Option<i32>,
        stderr: String,
    },
    /// 登録したのに CLI 側の一覧へ現れない（書式非対応・バージョン差の疑い）
    NotReflected { agent: WorkerAgent },
    /// claude はこのモジュールの担当外（呼び出し側の配線ミス）
    NotSupportedHere { agent: WorkerAgent },
}

impl AgentMcpError {
    /// 分類名（応答 JSON の `error_kind`。UI / AI が分岐に使える安定した文字列）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CliNotFound { .. } => "cli_not_found",
            Self::ScopeUnsupported { .. } => "scope_unsupported",
            Self::Spawn { .. } => "spawn_failed",
            Self::CommandFailed { .. } => "command_failed",
            Self::NotReflected { .. } => "not_reflected",
            Self::NotSupportedHere { .. } => "not_supported_here",
        }
    }
}

impl std::fmt::Display for AgentMcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CliNotFound { agent } => write!(
                f,
                "{} CLI が見つからないので MCP 登録できません。\
                 次の一手: {} を導入して PATH を通してから tako setup-mcp --agent {} をやり直してください（導入の案内: {}）",
                agent.as_str(),
                agent.as_str(),
                agent.as_str(),
                install_hint(*agent),
            ),
            Self::ScopeUnsupported { agent, scope } => write!(
                f,
                "{} は scope={scope} の MCP 登録に対応していません（ユーザーグローバルのみ）。\
                 次の一手: --{scope} を外して tako setup-mcp --agent {} を実行してください",
                agent.as_str(),
                agent.as_str(),
            ),
            Self::Spawn { agent, detail } => write!(
                f,
                "{} mcp add を起動できませんでした（{detail}）。\
                 次の一手: which {} でパスを確認し、実行権限があるか見てください",
                agent.as_str(),
                agent.as_str(),
            ),
            Self::CommandFailed {
                agent,
                code,
                stderr,
            } => {
                let code = code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                write!(
                    f,
                    "{} mcp add が失敗しました (exit {code}): {}。\
                     次の一手: 同じコマンドを手で実行して出力を確認してください（{} mcp add --help）",
                    agent.as_str(),
                    stderr.trim(),
                    agent.as_str(),
                )
            }
            Self::NotReflected { agent } => write!(
                f,
                "{} mcp add は成功したのに {} mcp list に tako が現れません（書式非対応・バージョン差の疑い）。\
                 次の一手: {} --version と {} mcp list の出力を添えて tako の Issue へ報告してください",
                agent.as_str(),
                agent.as_str(),
                agent.as_str(),
                agent.as_str(),
            ),
            Self::NotSupportedHere { agent } => write!(
                f,
                "{} の MCP 登録はこの経路では扱いません（claude は dispatch::setup_mcp が担当）",
                agent.as_str(),
            ),
        }
    }
}

/// 登録の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMcpResult {
    pub agent: WorkerAgent,
    /// 実際に書き込んだ（既に正しく登録済みなら false）
    pub configured: bool,
    /// 登録が既にあった（死んだパスの付け替えを含む）
    pub already_existed: bool,
    /// 登録済みだったが command のパスが実在せず付け替えた
    pub repaired: bool,
    /// 付け替え前の command（repaired = true のときだけ）
    pub old_command: Option<String>,
    /// 書き込み先（表示用。実際に書くのは各 CLI）
    pub target_path: Option<PathBuf>,
    /// 登録した command
    pub command: String,
}

/// 導入の案内先（`SetupAgent::install_hint` と同じ内容を CLI 非依存の場所に置く）
pub fn install_hint(agent: WorkerAgent) -> &'static str {
    match agent {
        WorkerAgent::Claude => "https://docs.anthropic.com/en/docs/claude-code",
        WorkerAgent::Codex => "https://developers.openai.com/codex/cli",
        WorkerAgent::Agy => "agy install",
    }
}

/// この経路（各 CLI の `mcp add`）で登録できるエージェントか。
/// claude は `dispatch::setup_mcp` が担当するのでここでは false
pub fn handled_here(agent: WorkerAgent) -> bool {
    matches!(agent, WorkerAgent::Codex | WorkerAgent::Agy)
}

/// `mcp add` の argv（プログラム名を除く）。**登録書式の正本**。
///
/// 実測（2026-08-27）で確定した形:
/// - codex: `codex mcp add <name> -- <command> <args...>`（`--` は必須。help の usage が
///   `codex mcp add [OPTIONS] <NAME> (--url <URL> | -- <COMMAND>...)`）
/// - agy: `agy mcp add <name> <commandOrUrl> [args...]`。`-` 始まりを渡すときは
///   `--` を前置できる（help の Notes）ので一律で付けて取り違えを防ぐ
pub fn add_args(agent: WorkerAgent, tako_binary: &str) -> Vec<String> {
    let mut v = vec!["mcp".to_string(), "add".to_string()];
    match agent {
        WorkerAgent::Codex | WorkerAgent::Agy => {
            v.push(SERVER_NAME.to_string());
            v.push("--".to_string());
            v.push(tako_binary.to_string());
            v.push("mcp".to_string());
            v.push("serve".to_string());
        }
        // claude はこの経路を通らない（`handled_here` が false）。
        // 万一呼ばれても `claude mcp add` の実際の書式（--scope / --transport）とは
        // 違うので、ここでは claude 用の argv を組まない
        WorkerAgent::Claude => {}
    }
    v
}

/// 一覧を引くための argv（`mcp list --json` 等）。codex だけが JSON を出せる
pub fn list_args(agent: WorkerAgent) -> Vec<String> {
    match agent {
        WorkerAgent::Codex => vec!["mcp".into(), "list".into(), "--json".into()],
        WorkerAgent::Agy => vec!["mcp".into(), "list".into()],
        WorkerAgent::Claude => vec!["mcp".into(), "list".into()],
    }
}

/// 設定ファイルの置き場（表示・読み取り用）。
///
/// - codex: `$CODEX_HOME/config.toml`（未設定なら `~/.codex/config.toml`）
/// - agy: `~/.gemini/config/mcp_config.json`（実測: `agy mcp add` が実際に書いた先。
///   `HOME` を差し替えるとそちらへ付いてくる = テストで隔離できる）
pub fn config_path(agent: WorkerAgent) -> Option<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    match agent {
        WorkerAgent::Codex => {
            if let Some(dir) = std::env::var_os("CODEX_HOME") {
                Some(PathBuf::from(dir).join("config.toml"))
            } else {
                home().map(|h| h.join(".codex").join("config.toml"))
            }
        }
        WorkerAgent::Agy => {
            home().map(|h| h.join(".gemini").join("config").join("mcp_config.json"))
        }
        WorkerAgent::Claude => home().map(|h| h.join(".claude.json")),
    }
}

/// `codex mcp list --json` の出力から tako の command を読む（純粋関数）。
/// 形は実測（2026-08-27）: `[{ "name": "tako", "transport": { "command": "...", ... } }]`
pub fn codex_registered_command(listing_json: &str) -> Option<String> {
    let items: serde_json::Value = serde_json::from_str(listing_json).ok()?;
    items.as_array()?.iter().find_map(|item| {
        if item.get("name")?.as_str()? != SERVER_NAME {
            return None;
        }
        item.get("transport")?
            .get("command")?
            .as_str()
            .map(String::from)
    })
}

/// agy の `mcp_config.json` から tako の command を読む（純粋関数）。
/// 形は実測（2026-08-27）: `{ "mcpServers": { "tako": { "command": "...", "args": [...] } } }`
pub fn agy_registered_command(config_json: &str) -> Option<String> {
    let data: serde_json::Value = serde_json::from_str(config_json).ok()?;
    data.get("mcpServers")?
        .get(SERVER_NAME)?
        .get("command")?
        .as_str()
        .map(String::from)
}

/// 既存登録が「そのまま使える」か。
/// command が空でなく実ファイルとして存在するときだけ true（claude 側の判定と同じ規則）
pub fn registration_is_alive(command: Option<&str>) -> bool {
    command
        .map(|c| !c.is_empty() && std::path::Path::new(c).is_file())
        .unwrap_or(false)
}

// --- 実 CLI を起こす層 ---

fn run(
    agent: WorkerAgent,
    bin: &str,
    args: &[String],
) -> Result<std::process::Output, AgentMcpError> {
    let mut cmd = std::process::Command::new(bin);
    // #586: dispatch（GUI 内）からも到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut cmd);
    cmd.args(args);
    cmd.output().map_err(|e| AgentMcpError::Spawn {
        agent,
        detail: e.to_string(),
    })
}

/// 現在の登録 command を CLI / 設定ファイルから読む。読めなければ None
fn read_registration(agent: WorkerAgent, bin: &str) -> Option<String> {
    match agent {
        WorkerAgent::Codex => {
            let out = run(agent, bin, &list_args(agent)).ok()?;
            if !out.status.success() {
                return None;
            }
            codex_registered_command(&String::from_utf8_lossy(out.stdout.as_slice()))
        }
        // agy の `mcp list` は表形式（列幅がバージョンで動く）なので設定 JSON を読む
        WorkerAgent::Agy => {
            let path = config_path(agent)?;
            let content = std::fs::read_to_string(path).ok()?;
            agy_registered_command(&content)
        }
        WorkerAgent::Claude => None,
    }
}

/// codex / agy へ tako MCP サーバーを登録する。
///
/// 既に生きた登録があれば何もしない（`configured = false`）。
/// 死んだパスが残っていれば付け替える（`repaired = true`）。
pub fn register(agent: WorkerAgent, tako_binary: &str) -> Result<AgentMcpResult, AgentMcpError> {
    if !handled_here(agent) {
        return Err(AgentMcpError::NotSupportedHere { agent });
    }
    let bin = tako_core::platform::exe::find(agent.as_str())
        .ok_or(AgentMcpError::CliNotFound { agent })?;

    let existing = read_registration(agent, &bin);
    if registration_is_alive(existing.as_deref()) {
        // 既存が同じパスを指していればもう何もしない。別のパスでも「生きている」なら
        // 利用者が意図して別ビルドを向けている可能性があるので、tako 自身の
        // パスと違うときだけ付け替える
        if existing.as_deref() == Some(tako_binary) {
            return Ok(AgentMcpResult {
                agent,
                configured: false,
                already_existed: true,
                repaired: false,
                old_command: None,
                target_path: config_path(agent),
                command: tako_binary.to_string(),
            });
        }
    }

    let old_command = existing.filter(|c| !c.is_empty());
    let repaired = old_command.is_some();

    let args = add_args(agent, tako_binary);
    let out = run(agent, &bin, &args)?;
    if !out.status.success() {
        return Err(AgentMcpError::CommandFailed {
            agent,
            code: out.status.code(),
            stderr: {
                let e = String::from_utf8_lossy(out.stderr.as_slice()).to_string();
                if e.trim().is_empty() {
                    String::from_utf8_lossy(out.stdout.as_slice()).to_string()
                } else {
                    e
                }
            },
        });
    }

    // 書けたと言われても本当に一覧へ出るかを確認する（無言死させない。#979 スコープ 3）
    match read_registration(agent, &bin) {
        Some(cmd) if cmd == tako_binary => {}
        // 読めない = 判定材料が無いだけなので成功として扱う（agy の設定ファイルが
        // 別の場所に移った将来のバージョン等）。登録が効いていないときは
        // codex のように一覧が読める側で NotReflected が出る
        None => {}
        Some(_) => return Err(AgentMcpError::NotReflected { agent }),
    }

    Ok(AgentMcpResult {
        agent,
        configured: true,
        already_existed: repaired,
        repaired,
        old_command,
        target_path: config_path(agent),
        command: tako_binary.to_string(),
    })
}

/// いま登録されている command を返す（`tako setup` のプラン表示・診断用）。
/// CLI が無い / 未登録 / 読めないときは None
pub fn current_registration(agent: WorkerAgent) -> Option<String> {
    if !handled_here(agent) {
        return None;
    }
    let bin = tako_core::platform::exe::find(agent.as_str())?;
    read_registration(agent, &bin)
}

/// この環境に導入済みで、この経路で登録できるエージェントを列挙する
pub fn detected_agents() -> Vec<WorkerAgent> {
    WorkerAgent::ALL
        .into_iter()
        .filter(|a| handled_here(*a))
        .filter(|a| tako_core::platform::exe::find(a.as_str()).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_argsはcodexとagyで実測どおりの書式になる() {
        // 実測（2026-08-27）: どちらも `--` の後ろにコマンドと引数を並べる
        assert_eq!(
            add_args(WorkerAgent::Codex, "/opt/tako"),
            vec!["mcp", "add", "tako", "--", "/opt/tako", "mcp", "serve"]
        );
        assert_eq!(
            add_args(WorkerAgent::Agy, "/opt/tako"),
            vec!["mcp", "add", "tako", "--", "/opt/tako", "mcp", "serve"]
        );
    }

    #[test]
    fn add_argsはclaudeの書式を組まない() {
        // claude は --scope / --transport が要る別書式。取り違えて壊さないよう空にしてある
        assert_eq!(
            add_args(WorkerAgent::Claude, "/opt/tako"),
            vec!["mcp", "add"]
        );
        assert!(!handled_here(WorkerAgent::Claude));
        assert!(handled_here(WorkerAgent::Codex));
        assert!(handled_here(WorkerAgent::Agy));
    }

    #[test]
    fn codexの一覧jsonからtakoのcommandを読む() {
        // 実測の出力をそのまま fixture 化（余計なサーバーが混ざっていても拾える）
        let listing = r#"[
          {"name":"node_repl","transport":{"type":"stdio","command":"/x/node","args":[]}},
          {"name":"tako","enabled":true,"transport":{"type":"stdio","command":"/usr/local/bin/tako","args":["mcp","serve"],"env":null,"env_vars":[],"cwd":null}}
        ]"#;
        assert_eq!(
            codex_registered_command(listing).as_deref(),
            Some("/usr/local/bin/tako")
        );
        assert_eq!(codex_registered_command("[]"), None);
        assert_eq!(codex_registered_command("not json"), None);
    }

    #[test]
    fn agyの設定jsonからtakoのcommandを読む() {
        let cfg = r#"{"mcpServers":{"tako":{"args":["mcp","serve"],"command":"/usr/local/bin/tako","disabled":false}}}"#;
        assert_eq!(
            agy_registered_command(cfg).as_deref(),
            Some("/usr/local/bin/tako")
        );
        assert_eq!(agy_registered_command(r#"{"mcpServers":{}}"#), None);
        assert_eq!(agy_registered_command(""), None);
    }

    #[test]
    fn 生きている登録の判定は実ファイルの存在で決まる() {
        assert!(!registration_is_alive(None));
        assert!(!registration_is_alive(Some("")));
        assert!(!registration_is_alive(Some("/no/such/tako-979")));
        // 必ず存在する実ファイルで正例を作る（プラットフォーム非依存に
        // したいので現在の実行ファイルを使う）
        let me = std::env::current_exe().unwrap();
        assert!(registration_is_alive(Some(me.to_str().unwrap())));
    }

    #[test]
    fn 未導入と非対応スコープのエラーは理由と次の一手を両方言う() {
        let e = AgentMcpError::CliNotFound {
            agent: WorkerAgent::Codex,
        };
        let msg = e.to_string();
        assert_eq!(e.kind(), "cli_not_found");
        assert!(msg.contains("codex"), "{msg}");
        assert!(msg.contains("次の一手"), "{msg}");
        assert!(msg.contains("tako setup-mcp --agent codex"), "{msg}");

        let e = AgentMcpError::ScopeUnsupported {
            agent: WorkerAgent::Agy,
            scope: "project",
        };
        let msg = e.to_string();
        assert_eq!(e.kind(), "scope_unsupported");
        assert!(msg.contains("project"), "{msg}");
        assert!(msg.contains("次の一手"), "{msg}");
    }

    #[test]
    fn 全分類が理由と次の一手を持つ() {
        // 分類を増やしたときに文面を書き忘れないための番犬
        let all = [
            AgentMcpError::CliNotFound {
                agent: WorkerAgent::Codex,
            },
            AgentMcpError::ScopeUnsupported {
                agent: WorkerAgent::Codex,
                scope: "project",
            },
            AgentMcpError::Spawn {
                agent: WorkerAgent::Codex,
                detail: "permission denied".into(),
            },
            AgentMcpError::CommandFailed {
                agent: WorkerAgent::Codex,
                code: Some(2),
                stderr: "boom".into(),
            },
            AgentMcpError::NotReflected {
                agent: WorkerAgent::Codex,
            },
            AgentMcpError::NotSupportedHere {
                agent: WorkerAgent::Claude,
            },
        ];
        let mut kinds = std::collections::HashSet::new();
        for e in &all {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "{:?}", e);
            // NotSupportedHere は配線ミスの内部エラーなので次の一手は要らない
            if !matches!(e, AgentMcpError::NotSupportedHere { .. }) {
                assert!(msg.contains("次の一手"), "次の一手が無い: {msg}");
            }
            assert!(kinds.insert(e.kind()), "kind が重複: {}", e.kind());
        }
        assert_eq!(kinds.len(), 6);
    }

    #[test]
    fn 設定ファイルの置き場はエージェントごとに分かれる() {
        // CODEX_HOME / HOME はプロセス全体の状態なので、ここでは
        // 「同じ HOME でも 3 エージェントが別の場所を指す」ことだけを見る
        let paths: Vec<_> = WorkerAgent::ALL
            .into_iter()
            .filter_map(config_path)
            .collect();
        assert_eq!(paths.len(), 3, "3 エージェントとも置き場を持つ");
        let uniq: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(uniq.len(), 3, "置き場が重なっている: {paths:?}");
        assert!(paths
            .iter()
            .any(|p| p.ends_with("mcp_config.json") && p.to_string_lossy().contains(".gemini")));
        assert!(paths.iter().any(|p| p.ends_with("config.toml")));
        assert!(paths.iter().any(|p| p.ends_with(".claude.json")));
    }

    #[test]
    fn list_argsはcodexだけjsonを要求する() {
        assert!(list_args(WorkerAgent::Codex).contains(&"--json".to_string()));
        // agy の一覧は表形式（列幅がバージョンで動く）ので JSON を求めない =
        // 設定ファイルを読む側の実装と整合している
        assert!(!list_args(WorkerAgent::Agy).contains(&"--json".to_string()));
    }
}

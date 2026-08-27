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
//! ## env の引き継ぎ（ここが成否を分ける。実測 2026-08-27）
//!
//! `tako mcp serve` は **`TAKO_SOCKET` + `TAKO_TOKEN` が env に無いと 0 ツールを返す**
//! （FR-2.3.2「tako 外で 0 ツール」。discovery ファイルは意図的に見ない）。
//! つまり「登録できたか」ではなく「MCP 子プロセスへ env が届くか」で決まる。
//! env を吐くだけの偽 MCP サーバーを両 CLI に登録して実測した結果:
//!
//! - **agy は親プロセスの env をそのまま渡す**（`TAKO_SOCKET` / `TAKO_TOKEN` /
//!   `TAKO_PANE_ID` / `TAKO_TAB_ID` / `TAKO_ORCHESTRATOR_ROLE` が届いた）。
//!   登録に env を書く必要はない
//! - **codex は既定で 1 つも渡さない**（偽サーバーが見た TAKO_* はゼロ件）。
//!   `env_vars` 許可リストに名前を並べると、並べたものだけが届く（実測）
//!
//! `env_vars` は**値ではなく名前**の列なので、ペインごとに違う値のまま正しく届き、
//! **トークンを設定ファイルへ書き残さない**（`--env KEY=VALUE` の静的 map だと
//! `~/.gemini/config/mcp_config.json` のような 644 のファイルへ token が載る）。
//! `codex mcp add` には `env_vars` を書くフラグが無く、しかも**再 add で
//! 既存の `env_vars` を消す**（実測）ので、順序は「`codex mcp add` → `env_vars` を
//! 1 行足す → `codex mcp list --json` で届いたか確認」。行の挿入だけなので
//! ファイルの他の部分はバイト単位でそのまま残る。

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

/// codex の MCP 子プロセスへ転送する env の名前（`env_vars` 許可リスト）。
///
/// **`tako mcp serve` が実際に読むものだけ**を並べる（`main.rs` の `mcp_serve` /
/// `caller_pane`）。値ではなく名前なので、ペインごとに違う値がそのまま届き、
/// 設定ファイルへトークンを書き残さない
pub const CODEX_FORWARD_ENV: &[&str] = &[
    "TAKO_SOCKET",
    "TAKO_TOKEN",
    "TAKO_PANE_ID",
    "TAKO_ORCHESTRATOR_ROLE",
];

/// このエージェントで `env_vars` 相当の明示指定が必要か。
/// agy は親 env をそのまま渡すので不要（実測）
pub fn needs_env_allowlist(agent: WorkerAgent) -> bool {
    matches!(agent, WorkerAgent::Codex)
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

/// `codex mcp list --json` の出力から tako の `env_vars` を読む（純粋関数）
pub fn codex_registered_env_vars(listing_json: &str) -> Vec<String> {
    let Ok(items) = serde_json::from_str::<serde_json::Value>(listing_json) else {
        return Vec::new();
    };
    let Some(arr) = items.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .find(|item| item.get("name").and_then(|n| n.as_str()) == Some(SERVER_NAME))
        .and_then(|item| item.get("transport")?.get("env_vars")?.as_array().cloned())
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 必要な env の名前がすべて許可リストに入っているか（純粋関数）
pub fn env_allowlist_covers(registered: &[String], needed: &[&str]) -> bool {
    needed
        .iter()
        .all(|n| registered.iter().any(|r| r.as_str() == *n))
}

/// TOML の `[mcp_servers.<name>]` セクションへ `env_vars = [...]` を足す / 差し替える（純粋関数）。
///
/// **セクション見出しの直後に 1 行入れる / 既存の 1 行を置き換える**だけなので、
/// ファイルの他の部分（コメント・並び順・他サーバー）はバイト単位でそのまま残る。
/// TOML ライブラリで読み書きすると CLI 側の正規化と二重にずれるのでそれはしない。
/// セクションが見つからなければ `None`（呼び出し側が「反映されていない」として扱う）
pub fn upsert_env_vars_toml(text: &str, server: &str, vars: &[&str]) -> Option<String> {
    let header = format!("[mcp_servers.{server}]");
    let rendered = format!(
        "env_vars = [{}]",
        vars.iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut inserted = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if is_table_header(trimmed) {
            // 別のセクションへ移る = 対象セクションは終わり
            in_section = trimmed == header;
            out.push(line.to_string());
            if in_section {
                out.push(rendered.clone());
                inserted = true;
            }
            continue;
        }
        // 対象セクション内の既存 env_vars 行は落とす（上で入れ直したものが正）
        if in_section && trimmed.starts_with("env_vars") {
            continue;
        }
        out.push(line.to_string());
    }
    if !inserted {
        return None;
    }
    // CRLF のファイルを LF へ書き換えない（全行が差分になるのを避ける）
    let sep = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut joined = out.join(sep);
    if text.ends_with('\n') {
        joined.push_str(sep);
    }
    Some(joined)
}

/// TOML のテーブル見出し行か（純粋関数）。
///
/// **複数行配列の途中の行を見出しと誤認しない**のが要点: `matrix = [` の続きに
/// `[1, 2]` のような行が来ることがあり、素朴に `[`〜`]` で判定するとセクションの
/// 境界を取り違える（対象セクションの既存 `env_vars` を消し損ねて重複キーになり、
/// codex が設定を読めなくなる）。見出しにカンマは現れない
fn is_table_header(trimmed: &str) -> bool {
    let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let inner = inner.trim_start_matches('[').trim_end_matches(']');
    !inner.is_empty() && !inner.contains(',')
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

/// いまの登録内容（command と転送 env）。読めなければ command は None
#[derive(Debug, Default, Clone)]
struct Registration {
    command: Option<String>,
    env_vars: Vec<String>,
}

impl Registration {
    /// このまま使えるか = command が実在し、必要な env の転送も揃っている
    fn is_usable(&self, agent: WorkerAgent, tako_binary: &str) -> bool {
        if self.command.as_deref() != Some(tako_binary) {
            return false;
        }
        if !registration_is_alive(self.command.as_deref()) {
            return false;
        }
        !needs_env_allowlist(agent) || env_allowlist_covers(&self.env_vars, CODEX_FORWARD_ENV)
    }
}

/// 現在の登録を CLI / 設定ファイルから読む
fn read_registration(agent: WorkerAgent, bin: &str) -> Registration {
    match agent {
        WorkerAgent::Codex => {
            let Ok(out) = run(agent, bin, &list_args(agent)) else {
                return Registration::default();
            };
            if !out.status.success() {
                return Registration::default();
            }
            let listing = String::from_utf8_lossy(out.stdout.as_slice()).to_string();
            Registration {
                command: codex_registered_command(&listing),
                env_vars: codex_registered_env_vars(&listing),
            }
        }
        // agy の `mcp list` は表形式（列幅がバージョンで動く）なので設定 JSON を読む。
        // agy は親 env をそのまま渡すので env_vars は空のままでよい（実測）
        WorkerAgent::Agy => {
            let Some(path) = config_path(agent) else {
                return Registration::default();
            };
            let Ok(content) = std::fs::read_to_string(path) else {
                return Registration::default();
            };
            Registration {
                command: agy_registered_command(&content),
                env_vars: Vec::new(),
            }
        }
        WorkerAgent::Claude => Registration::default(),
    }
}

/// codex の `[mcp_servers.tako]` へ `env_vars` を足す。
/// `codex mcp add` にはこれを書くフラグが無く、**再 add で消える**ので add の後に呼ぶ
fn apply_env_allowlist(agent: WorkerAgent) -> Result<(), AgentMcpError> {
    if !needs_env_allowlist(agent) {
        return Ok(());
    }
    let path = config_path(agent).ok_or(AgentMcpError::NotReflected { agent })?;
    let text = std::fs::read_to_string(&path).map_err(|e| AgentMcpError::Spawn {
        agent,
        detail: format!("{} を読めない: {e}", path.display()),
    })?;
    let updated = upsert_env_vars_toml(&text, SERVER_NAME, CODEX_FORWARD_ENV)
        .ok_or(AgentMcpError::NotReflected { agent })?;
    if updated == text {
        return Ok(());
    }
    std::fs::write(&path, updated).map_err(|e| AgentMcpError::Spawn {
        agent,
        detail: format!("{} へ書けない: {e}", path.display()),
    })
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
    if existing.is_usable(agent, tako_binary) {
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

    let old_command = existing.command.clone().filter(|c| !c.is_empty());
    // 「登録はあるが command が死んでいた」ときだけ修復扱い。env の転送が
    // 足りないだけ（旧 tako が入れた登録）は付け替えではないので repaired にしない
    let repaired = old_command
        .as_deref()
        .is_some_and(|c| !registration_is_alive(Some(c)));

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
    // add の後（add は env_vars を消す）
    apply_env_allowlist(agent)?;

    // 書けたと言われても本当に効いているかを確認する（無言死させない。#979 スコープ 3）。
    // command が読めない = 判定材料が無いだけなので成功として扱うが、
    // 読めたのに食い違う・env の転送が欠けるなら NotReflected
    let after = read_registration(agent, &bin);
    if after.command.is_some() && !after.is_usable(agent, tako_binary) {
        return Err(AgentMcpError::NotReflected { agent });
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

/// いまの登録状態（`tako setup` のプラン表示・`--check` の診断用）。
///
/// **`EnvMissing` を分けているのが要点**: 登録行があっても env の転送が欠けていれば
/// `tako mcp serve` は 0 ツールを返すので、「未登録」とも「使える」とも言えない
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpState {
    /// この経路の対象でない（claude）/ CLI 未導入 / 読み取れない
    Unknown,
    /// tako の登録が無い
    NotRegistered,
    /// 登録はあるが command のパスが実在しない
    Dead { command: String },
    /// 登録はあるが env の転送指定が足りない（= ツールが 0 個になる）
    EnvMissing { command: String },
    /// そのまま使える
    Ready { command: String },
}

impl McpState {
    /// 登録行に書かれている command（あれば）
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Dead { command } | Self::EnvMissing { command } | Self::Ready { command } => {
                Some(command)
            }
            Self::Unknown | Self::NotRegistered => None,
        }
    }

    /// このままツールが見える状態か
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// 「なぜ使えないか」の短い説明（`tako setup` のプラン表示用。None = 説明不要）
    pub fn describe_gap(&self) -> Option<&'static str> {
        match self {
            Self::NotRegistered => Some("未登録"),
            Self::Dead { .. } => Some("登録パス消失"),
            Self::EnvMissing { .. } => Some("env 転送なし"),
            Self::Unknown | Self::Ready { .. } => None,
        }
    }
}

/// いまの登録状態を読む（CLI を 1 回だけ起こす）
pub fn state(agent: WorkerAgent) -> McpState {
    if !handled_here(agent) {
        return McpState::Unknown;
    }
    let Some(bin) = tako_core::platform::exe::find(agent.as_str()) else {
        return McpState::Unknown;
    };
    let reg = read_registration(agent, &bin);
    let Some(command) = reg.command.filter(|c| !c.is_empty()) else {
        return McpState::NotRegistered;
    };
    if !registration_is_alive(Some(&command)) {
        return McpState::Dead { command };
    }
    if needs_env_allowlist(agent) && !env_allowlist_covers(&reg.env_vars, CODEX_FORWARD_ENV) {
        return McpState::EnvMissing { command };
    }
    McpState::Ready { command }
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
    fn 転送するenvはmcpブリッジが読むものだけ() {
        // `tako mcp serve` が読むのは SOCKET / TOKEN / PANE_ID / ORCHESTRATOR_ROLE。
        // 値ではなく名前を並べるので、設定ファイルへトークンが残らない
        assert_eq!(
            CODEX_FORWARD_ENV,
            &[
                "TAKO_SOCKET",
                "TAKO_TOKEN",
                "TAKO_PANE_ID",
                "TAKO_ORCHESTRATOR_ROLE"
            ]
        );
        assert!(CODEX_FORWARD_ENV.iter().all(|v| !v.contains('=')));
        // codex だけが許可リストを要る（agy は親 env をそのまま渡す = 実測）
        assert!(needs_env_allowlist(WorkerAgent::Codex));
        assert!(!needs_env_allowlist(WorkerAgent::Agy));
        assert!(!needs_env_allowlist(WorkerAgent::Claude));
    }

    #[test]
    fn codexの一覧jsonからenv_varsを読む() {
        let listing = r#"[{"name":"tako","transport":{"type":"stdio","command":"/x/tako","args":["mcp","serve"],"env":null,"env_vars":["TAKO_SOCKET","TAKO_TOKEN"],"cwd":null}}]"#;
        assert_eq!(
            codex_registered_env_vars(listing),
            vec!["TAKO_SOCKET".to_string(), "TAKO_TOKEN".to_string()]
        );
        // 実測どおり `env_vars: []` が既定
        let empty = r#"[{"name":"tako","transport":{"command":"/x/tako","env_vars":[]}}]"#;
        assert!(codex_registered_env_vars(empty).is_empty());
        assert!(codex_registered_env_vars("[]").is_empty());
        assert!(codex_registered_env_vars("not json").is_empty());
    }

    #[test]
    fn 許可リストの被覆判定は部分集合では通らない() {
        let full: Vec<String> = CODEX_FORWARD_ENV.iter().map(|s| s.to_string()).collect();
        assert!(env_allowlist_covers(&full, CODEX_FORWARD_ENV));
        // 余計なものが混ざっていても通る（ユーザーが足したものを消さない）
        let mut extra = full.clone();
        extra.push("MY_OWN".into());
        assert!(env_allowlist_covers(&extra, CODEX_FORWARD_ENV));
        // 1 つ欠けたら通さない = 0 ツールになる登録を「登録済み」と言わない
        let short: Vec<String> = full[..full.len() - 1].to_vec();
        assert!(!env_allowlist_covers(&short, CODEX_FORWARD_ENV));
        assert!(!env_allowlist_covers(&[], CODEX_FORWARD_ENV));
    }

    #[test]
    fn env_varsの挿入は他の行をバイト単位で保つ() {
        // 実測の形（codex mcp add 直後）+ ユーザーのコメントと別サーバー
        let text = "# 大事なコメント\nmodel = \"gpt-5.6\"\n\n[mcp_servers.other]\ncommand = \"/bin/true\"\n\n[mcp_servers.tako]\ncommand = \"/x/tako\"\nargs = [\"mcp\", \"serve\"]\n";
        let out = upsert_env_vars_toml(text, "tako", &["A", "B"]).expect("tako セクションがある");
        assert!(out.contains("env_vars = [\"A\", \"B\"]"), "{out}");
        // 対象セクション以外は 1 文字も変わらない
        assert!(out.starts_with("# 大事なコメント\nmodel = \"gpt-5.6\"\n\n[mcp_servers.other]\ncommand = \"/bin/true\"\n"), "{out}");
        assert!(out.contains("args = [\"mcp\", \"serve\"]"));
        assert!(out.ends_with('\n'), "末尾改行を保つ");
        // 2 回目は同じ結果（冪等）= 行が増えない
        let again = upsert_env_vars_toml(&out, "tako", &["A", "B"]).unwrap();
        assert_eq!(again, out);
        assert_eq!(again.matches("env_vars").count(), 1);
    }

    #[test]
    fn env_varsの挿入は既存の値を差し替える() {
        let text = "[mcp_servers.tako]\nenv_vars = [\"OLD\"]\ncommand = \"/x/tako\"\n";
        let out = upsert_env_vars_toml(text, "tako", &["NEW"]).unwrap();
        assert!(out.contains("env_vars = [\"NEW\"]"), "{out}");
        assert!(!out.contains("OLD"), "{out}");
        assert_eq!(out.matches("env_vars").count(), 1);
    }

    #[test]
    fn 複数行配列の途中の行を見出しと誤認しない() {
        assert!(is_table_header("[mcp_servers.tako]"));
        assert!(is_table_header("[features]"));
        assert!(is_table_header("[[bin]]"));
        // 配列の要素行（カンマを含む / 空）は見出しではない
        assert!(!is_table_header("[1, 2]"));
        assert!(!is_table_header("[]"));
        assert!(!is_table_header("]"));
        assert!(!is_table_header("command = \"/x\""));

        // 実害の再現: 見出しを誤認するとセクション境界がずれて env_vars が 2 本残る
        let text =
            "[mcp_servers.tako]\nenv_vars = [\"OLD\"]\nmatrix = [\n  [1, 2]\n]\ncommand = \"/x\"\n";
        let out = upsert_env_vars_toml(text, "tako", &["NEW"]).unwrap();
        assert_eq!(out.matches("env_vars").count(), 1, "{out}");
        assert!(out.contains("env_vars = [\"NEW\"]"), "{out}");
        assert!(out.contains("[1, 2]"), "配列の中身は残す: {out}");
    }

    #[test]
    fn crlfのファイルを行末ごと書き換えない() {
        let text = "[mcp_servers.tako]\r\ncommand = \"/x\"\r\n";
        let out = upsert_env_vars_toml(text, "tako", &["A"]).unwrap();
        assert!(out.contains("\r\n"), "{out:?}");
        assert!(!out.contains("\n\n"), "LF だけの行を混ぜない: {out:?}");
        assert_eq!(out.matches('\r').count(), out.lines().count(), "{out:?}");
        // LF のファイルは LF のまま
        let lf = "[mcp_servers.tako]\ncommand = \"/x\"\n";
        let out = upsert_env_vars_toml(lf, "tako", &["A"]).unwrap();
        assert!(!out.contains('\r'), "{out:?}");
    }

    #[test]
    fn 対象セクションが無ければnoneで反映失敗を伝える() {
        let text = "[mcp_servers.other]\ncommand = \"/bin/true\"\n";
        assert!(upsert_env_vars_toml(text, "tako", &["A"]).is_none());
        assert!(upsert_env_vars_toml("", "tako", &["A"]).is_none());
        // 隣のセクションの env_vars は消さない
        let text =
            "[mcp_servers.other]\nenv_vars = [\"KEEP\"]\n\n[mcp_servers.tako]\ncommand = \"/x\"\n";
        let out = upsert_env_vars_toml(text, "tako", &["A"]).unwrap();
        assert!(out.contains("env_vars = [\"KEEP\"]"), "{out}");
        assert!(out.contains("env_vars = [\"A\"]"), "{out}");
    }

    #[test]
    fn 使えるかの判定はcommandとenv転送の両方を見る() {
        let me = std::env::current_exe().unwrap().display().to_string();
        let full: Vec<String> = CODEX_FORWARD_ENV.iter().map(|s| s.to_string()).collect();

        // codex: env の転送が揃って初めて「使える」
        let ok = Registration {
            command: Some(me.clone()),
            env_vars: full.clone(),
        };
        assert!(ok.is_usable(WorkerAgent::Codex, &me));
        let no_env = Registration {
            command: Some(me.clone()),
            env_vars: Vec::new(),
        };
        assert!(!no_env.is_usable(WorkerAgent::Codex, &me));
        // agy は env を要らない
        assert!(no_env.is_usable(WorkerAgent::Agy, &me));
        // 死んだパス・別のパスは使えない
        let dead = Registration {
            command: Some("/gone/tako".into()),
            env_vars: full,
        };
        assert!(!dead.is_usable(WorkerAgent::Codex, &me));
        assert!(!Registration::default().is_usable(WorkerAgent::Agy, &me));
    }

    #[test]
    fn 登録状態はenv転送の欠落を未登録と混ぜない() {
        assert_eq!(McpState::Unknown.command(), None);
        assert_eq!(McpState::NotRegistered.describe_gap(), Some("未登録"));
        let dead = McpState::Dead {
            command: "/gone".into(),
        };
        assert_eq!(dead.command(), Some("/gone"));
        assert_eq!(dead.describe_gap(), Some("登録パス消失"));
        let env_missing = McpState::EnvMissing {
            command: "/x/tako".into(),
        };
        // 「登録はあるが 0 ツール」を「未登録」と言わない
        assert_eq!(env_missing.describe_gap(), Some("env 転送なし"));
        assert!(!env_missing.is_ready());
        let ready = McpState::Ready {
            command: "/x/tako".into(),
        };
        assert!(ready.is_ready());
        assert_eq!(ready.describe_gap(), None);
        // claude はこの経路の対象外
        assert_eq!(state(WorkerAgent::Claude), McpState::Unknown);
    }

    #[test]
    fn list_argsはcodexだけjsonを要求する() {
        assert!(list_args(WorkerAgent::Codex).contains(&"--json".to_string()));
        // agy の一覧は表形式（列幅がバージョンで動く）ので JSON を求めない =
        // 設定ファイルを読む側の実装と整合している
        assert!(!list_args(WorkerAgent::Agy).contains(&"--json".to_string()));
    }
}

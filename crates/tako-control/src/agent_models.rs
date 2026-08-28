//! agent_models — エージェント CLI から**利用可能なモデル一覧を実取得する**（Issue #1002）
//!
//! ## なぜ要るか
//!
//! `tako setup` はモデルを「各 CLI の既定値」に任せるだけで（#27 の教訓で
//! `[1m]` 固定をやめた経緯がある）、**どのモデルが選べるのかをユーザーへ一度も見せていない**。
//! 選ぶには CLI の中に入って `/model` を叩くしかなく、tako の外の知識が要る。
//!
//! ## 取得手段（すべて実物で確認した形だけを書く。憶測は書かない）
//!
//! | 系統 | 手段 | 実測（2026-08-27） |
//! |---|---|---|
//! | codex | `codex debug models` | `Render the raw model catalog as JSON`。stdout に JSON。**未認証でも既定カタログを返す**（認証すると内容が変わる = ユーザー固有） |
//! | agy | `agy models` | `List available models`。stdout に `id<TAB>表示名` の TSV、stderr に `Fetching available models...`。**未認証は exit 1** + `Please sign in to view available models.` |
//! | claude | **無い** | サブコマンドが存在しない（`claude models` は**プロンプトとして解釈される**）。モデル指定はセッション内の `/model` ピッカーか `--model <alias\|full-name>` |
//!
//! claude は Issue #1002 スコープ 2 の後半どおり「**既知モデルの静的リスト + 取得不可の明示**」。
//! 静的リストは**エイリアス**（`opus` / `sonnet` / `fable` = `claude --model` の help が
//! documented している語）だけを持つ。バージョン付きの id を焼くと古くなるが、
//! エイリアスは「その系統の最新」を指すので陳腐化しない。
//! ローカルキャッシュ（`~/.claude/.claude.json` の `additionalModelOptionsCache`）は
//! **加算のみ**の補助として読む（組織固有の選択肢が出ることがある。読めなければ黙って無視）。
//!
//! ## 設計
//!
//! - **パーサは純粋関数**（[`parse_codex_models`] / [`parse_agy_models`]）。実 CLI の出力を
//!   fixture に固定してテストする。知らないフィールドは無視するので上流の追加に強い
//! - **取得の失敗は必ず分類する**（[`CatalogFailure`]）。「無いのか」「未認証なのか」
//!   「そもそも一覧コマンドが無いのか」を混ぜない（#982 の `Pending` と `Unsupported` を
//!   混ぜない規約と同じ思想）
//! - CLI が無い場合の「理由 + 次の一手」は **#983 の [`agent_cli`] を再利用**する
//!   （導入コマンドの文言を二重に持たない）

use serde_json::{json, Value};

use crate::orchestrator::agent::WorkerAgent;
use crate::orchestrator::agent_cli::{self, AgentCliError};
use tako_core::i18n::Lang;
use tako_core::platform::support::Note;

/// 1 モデルの出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    /// CLI の一覧コマンドから取れた
    Cli,
    /// tako 同梱の静的リスト（一覧コマンドを持たない系統）
    Builtin,
    /// CLI がローカルへ残したキャッシュ（非公式。加算のみ）
    LocalCache,
}

impl ModelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Builtin => "builtin",
            Self::LocalCache => "local_cache",
        }
    }
}

/// ピッカーへ並べる 1 項目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// `--model` へそのまま渡せる値
    pub id: String,
    /// 人が読む名前（CLI が返す表示名。無ければ id）
    pub label: String,
    pub description: Option<String>,
    /// **このモデルが受け付ける** effort 語彙（codex は per-model で違う）
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub context_window: Option<u64>,
    pub source: ModelSource,
}

impl ModelOption {
    fn simple(id: &str, label: &str, source: ModelSource) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            description: None,
            efforts: Vec::new(),
            default_effort: None,
            context_window: None,
            source,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "description": self.description,
            "efforts": self.efforts,
            "default_effort": self.default_effort,
            "context_window": self.context_window,
            "source": self.source.as_str(),
        })
    }
}

/// 一覧が取れなかった理由。**「取得不可」を 1 種類に潰さない**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogFailure {
    /// CLI が PATH（と既知の設置先）に無い。案内は #983 の [`AgentCliError`] を使う
    NotInstalled(AgentCliError),
    /// CLI はあるが未認証（実測: agy は exit 1 + `Please sign in ...`）
    NotAuthenticated { detail: String },
    /// この系統は一覧取得コマンドを持たない（claude）
    NoListCommand,
    /// コマンドは在ったが失敗した（起動不能・非ゼロ終了）
    CommandFailed { detail: String },
    /// 出力が想定の形でなかった（上流の書式変更）
    ParseFailed { detail: String },
}

impl CatalogFailure {
    /// 機械可読な種別
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotInstalled(_) => "cli_not_found",
            Self::NotAuthenticated { .. } => "not_authenticated",
            Self::NoListCommand => "no_list_command",
            Self::CommandFailed { .. } => "command_failed",
            Self::ParseFailed { .. } => "parse_failed",
        }
    }

    /// 「理由 + 次の一手」。表示言語を明示できるようにして言語グローバルへ触らない
    /// （#608 / #807 で踏んだ並列テストの競合対策と同じ作法）
    pub fn message_in(&self, agent: WorkerAgent, lang: Lang) -> String {
        let name = agent.as_str();
        match self {
            Self::NotInstalled(err) => err.message_in(lang),
            Self::NotAuthenticated { detail } => {
                let head = Note::new(
                    "はログインしていないためモデル一覧を返しません",
                    " is not signed in, so it will not return the model list",
                );
                let next = Note::new(
                    "次の一手: `{cli}` を単独で起動してログインし、`tako setup` をやり直す",
                    "Next: run `{cli}` on its own to sign in, then re-run `tako setup`",
                );
                join3(
                    &format!("{name} CLI{}", head.text_in(lang)),
                    &next.text_in(lang).replace("{cli}", &auth_command(agent)),
                    detail,
                    lang,
                )
            }
            Self::NoListCommand => {
                let head = Note::new(
                    "はモデル一覧を出すコマンドを持ちません（同梱の既知モデルを並べています）",
                    " has no command that lists models (falling back to the built-in list)",
                );
                let next = Note::new(
                    "次の一手: 一覧を確認したいときはセッション内で `/model` を開く。ここでの選択はそのまま `--model` へ渡ります",
                    "Next: open `/model` inside a session to browse them. What you pick here is passed straight to `--model`",
                );
                join3(
                    &format!("{name} CLI{}", head.text_in(lang)),
                    next.text_in(lang),
                    "",
                    lang,
                )
            }
            Self::CommandFailed { detail } => {
                let head = Note::new(
                    "のモデル一覧コマンドが失敗しました",
                    "'s model list command failed",
                );
                let next = Note::new(
                    "次の一手: 同じコマンドを手で実行して出力を確かめる",
                    "Next: run the same command by hand and inspect its output",
                );
                join3(
                    &format!("{name} CLI{}", head.text_in(lang)),
                    next.text_in(lang),
                    detail,
                    lang,
                )
            }
            Self::ParseFailed { detail } => {
                let head = Note::new(
                    "のモデル一覧の書式が想定と違いました（上流が変わった可能性）",
                    "'s model list was not in the expected format (the upstream format may have changed)",
                );
                let next = Note::new(
                    "次の一手: モデルは指定せず CLI の既定に任せ、tako へ報告する",
                    "Next: leave the model unset so the CLI default applies, and report this to tako",
                );
                join3(
                    &format!("{name} CLI{}", head.text_in(lang)),
                    next.text_in(lang),
                    detail,
                    lang,
                )
            }
        }
    }

    fn to_json(&self, agent: WorkerAgent) -> Value {
        let mut value = json!({
            "kind": self.kind(),
            "message": self.message_in(agent, tako_core::i18n::lang()),
        });
        if let Self::NotInstalled(err) = self {
            value["install"] = err.to_json();
        }
        value
    }
}

fn join3(head: &str, next: &str, detail: &str, lang: Lang) -> String {
    let end = match lang {
        Lang::Ja => "。",
        Lang::En => ".",
    };
    let mut out = format!("{head}{end}\n  {next}");
    if !detail.trim().is_empty() {
        out.push_str("\n  ");
        out.push_str(match lang {
            Lang::Ja => "詳細: ",
            Lang::En => "Detail: ",
        });
        out.push_str(detail.trim());
    }
    out
}

/// 各 CLI のログイン手順（実物で確認した形）
/// ログインのコマンド。**正本は #983 の `agent_cli::auth_command`**（未認証の案内を
/// モデル一覧側と起動側で二重管理しない）
fn auth_command(agent: WorkerAgent) -> String {
    tako_core::agent_support::Agent::parse(agent.as_str())
        .and_then(agent_cli::auth_command)
        .unwrap_or(agent.as_str())
        .to_string()
}

/// 一覧取得コマンドの argv（**正本**。表示・実行・テストがここだけを見る）。
/// claude は一覧コマンドを持たないので `None`
pub fn catalog_argv(agent: WorkerAgent) -> Option<&'static [&'static str]> {
    match agent {
        WorkerAgent::Claude => None,
        WorkerAgent::Codex => Some(&["debug", "models"]),
        WorkerAgent::Agy => Some(&["models"]),
    }
}

/// 表示用の一覧取得コマンド文字列
pub fn catalog_command(agent: WorkerAgent) -> Option<String> {
    catalog_argv(agent).map(|args| format!("{} {}", agent.as_str(), args.join(" ")))
}

/// 取得結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalog {
    pub agent: WorkerAgent,
    /// 実行した（or 実行しようとした）コマンド。持たない系統は None
    pub list_command: Option<String>,
    pub models: Vec<ModelOption>,
    /// この系統が `--effort` 相当で受け付ける語彙（モデル別に違う場合は
    /// [`ModelOption::efforts`] が正。ここは系統全体の和集合）
    pub efforts: Vec<String>,
    /// 実取得できなかった理由。**models が空でないこと（静的フォールバック）と両立する**
    pub failure: Option<CatalogFailure>,
}

impl ModelCatalog {
    /// 実 CLI から取れたか（= 受け入れ条件 2 の「実取得の証拠」が出せる状態か）
    pub fn is_live(&self) -> bool {
        self.failure.is_none() && self.models.iter().any(|m| m.source == ModelSource::Cli)
    }

    /// 一覧の出どころ（`cli` = 実取得 / `builtin` = 同梱の静的リスト /
    /// `none` = 並べられるものが無い）。**失敗して 0 件のときに builtin と言わない**
    pub fn source_label(&self) -> &'static str {
        if self.is_live() {
            "cli"
        } else if self.models.is_empty() {
            "none"
        } else {
            "builtin"
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent.as_str(),
            "list_command": self.list_command,
            "source": self.source_label(),
            "live": self.is_live(),
            "models": self.models.iter().map(ModelOption::to_json).collect::<Vec<_>>(),
            "efforts": self.efforts,
            "failure": self.failure.as_ref().map(|f| f.to_json(self.agent)),
        })
    }
}

/// 子プロセスの実行結果（テストから注入できるようにした最小の形）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 系統ごとの effort 語彙。**正本は [`WorkerAgent::effort_options`]**（起動時に実際に
/// 使われる値と食い違わせない）
fn agent_efforts(agent: WorkerAgent) -> Vec<String> {
    agent
        .effort_options()
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// claude の同梱静的リスト。**エイリアスだけ**を持つ（バージョン付き id は焼かない）。
/// 出典は `claude --model` の help（実測 2.1.232:
/// "Provide an alias for the latest model (e.g. 'fable', 'opus', or 'sonnet')"）
pub fn builtin_claude_models() -> Vec<ModelOption> {
    vec![
        ModelOption::simple("opus", "Opus（最上位）", ModelSource::Builtin),
        ModelOption::simple("sonnet", "Sonnet（バランス）", ModelSource::Builtin),
        ModelOption::simple("fable", "Fable", ModelSource::Builtin),
    ]
}

/// `~/.claude/.claude.json` の `additionalModelOptionsCache` を**加算のみ**で読む。
/// 非公式のローカルキャッシュなので、読めない・形が違うときは黙って空を返す
pub fn parse_claude_cache(config_json: &str) -> Vec<ModelOption> {
    let Ok(value) = serde_json::from_str::<Value>(config_json) else {
        return Vec::new();
    };
    let Some(items) = value
        .get("additionalModelOptionsCache")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("value").and_then(Value::as_str)?.trim();
            if id.is_empty() {
                return None;
            }
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            Some(ModelOption {
                id: id.to_string(),
                label: label.to_string(),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                efforts: Vec::new(),
                default_effort: None,
                context_window: None,
                source: ModelSource::LocalCache,
            })
        })
        .collect()
}

/// `codex debug models` の JSON をパースする。
///
/// - `visibility` が `list` 以外（実測: `hide`）は**並べない**（内部用のモデル）
/// - 並び順は `priority` 昇順（codex 自身のピッカーと同じ意味づけ）
/// - `supported_reasoning_levels[].effort` が**そのモデルの** effort 語彙
/// - 知らないフィールドは読み飛ばす（実測の JSON は 1 モデルに 38 キーある）
pub fn parse_codex_models(stdout: &str) -> Result<Vec<ModelOption>, String> {
    let value: Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("JSON として読めない: {e}"))?;
    let items = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or("`models` 配列が無い")?;
    let mut out: Vec<(i64, ModelOption)> = Vec::new();
    for item in items {
        let Some(id) = item.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if id.trim().is_empty() {
            continue;
        }
        // visibility は「ピッカーに出すか」。実測で `list` / `hide` の 2 値
        if item
            .get("visibility")
            .and_then(Value::as_str)
            .is_some_and(|v| v != "list")
        {
            continue;
        }
        let efforts = item
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(|level| level.get("effort").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let priority = item
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX);
        out.push((
            priority,
            ModelOption {
                id: id.to_string(),
                label: item
                    .get("display_name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(id)
                    .to_string(),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string),
                efforts,
                default_effort: item
                    .get("default_reasoning_level")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                context_window: item.get("context_window").and_then(Value::as_u64),
                source: ModelSource::Cli,
            },
        ));
    }
    if out.is_empty() {
        return Err("並べられるモデルが 1 件も無い".into());
    }
    out.sort_by_key(|(priority, _)| *priority);
    Ok(out.into_iter().map(|(_, model)| model).collect())
}

/// `agy models` の TSV をパースする（実測: stdout が `id<TAB>表示名` の行だけ。
/// 進捗の `Fetching available models...` は stderr なので混ざらないが、
/// 念のためタブが無い行は読み飛ばす）
pub fn parse_agy_models(stdout: &str) -> Result<Vec<ModelOption>, String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        let Some((id, label)) = line.split_once('\t') else {
            continue;
        };
        let (id, label) = (id.trim(), label.trim());
        if id.is_empty() {
            continue;
        }
        out.push(ModelOption {
            id: id.to_string(),
            label: if label.is_empty() {
                id.to_string()
            } else {
                label.to_string()
            },
            description: None,
            // agy の effort は `--effort low|medium|high`（実測。CLI 自身が
            // `invalid --effort "bogus" (valid: low, medium, high)` と言う）。
            // 表示名の "(High)" 等はモデル側の設定で、`--effort` とは別物
            efforts: agent_efforts(WorkerAgent::Agy),
            default_effort: None,
            context_window: None,
            source: ModelSource::Cli,
        });
    }
    if out.is_empty() {
        return Err("`id<TAB>表示名` の行が 1 件も無い".into());
    }
    Ok(out)
}

/// 未認証を示す出力か（実測の文言から判定。exit != 0 のときだけ見る）
fn looks_unauthenticated(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "sign in",
        "signin",
        "log in",
        "login",
        "not authenticated",
        "unauthorized",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 取得の本体。**実行だけを注入可能にして判断は純粋関数側に置く**
pub fn catalog_with(
    agent: WorkerAgent,
    locate: impl FnOnce(WorkerAgent) -> Result<String, AgentCliError>,
    run: impl FnOnce(&str, &[&str]) -> Option<CommandRun>,
    claude_config_json: Option<&str>,
) -> ModelCatalog {
    let efforts = agent_efforts(agent);
    let list_command = catalog_command(agent);

    let path = match locate(agent) {
        Ok(path) => path,
        Err(err) => {
            // CLI が無いなら一覧は取れない。claude だけは静的リストを出せる
            let models = if agent == WorkerAgent::Claude {
                builtin_claude_models()
            } else {
                Vec::new()
            };
            return ModelCatalog {
                agent,
                list_command,
                models,
                efforts,
                failure: Some(CatalogFailure::NotInstalled(err)),
            };
        }
    };

    let Some(args) = catalog_argv(agent) else {
        // claude: 一覧コマンドが無い。静的リスト + ローカルキャッシュの加算
        let mut models = builtin_claude_models();
        if let Some(json) = claude_config_json {
            for extra in parse_claude_cache(json) {
                if !models.iter().any(|m| m.id == extra.id) {
                    models.push(extra);
                }
            }
        }
        return ModelCatalog {
            agent,
            list_command,
            models,
            efforts,
            failure: Some(CatalogFailure::NoListCommand),
        };
    };

    let Some(output) = run(&path, args) else {
        return ModelCatalog {
            agent,
            list_command,
            models: Vec::new(),
            efforts,
            failure: Some(CatalogFailure::CommandFailed {
                detail: "コマンドを起動できなかった".into(),
            }),
        };
    };
    if !output.success {
        let detail = first_meaningful_line(&output.stderr)
            .or_else(|| first_meaningful_line(&output.stdout))
            .unwrap_or_default();
        let failure = if looks_unauthenticated(&format!("{}\n{}", output.stderr, output.stdout)) {
            CatalogFailure::NotAuthenticated { detail }
        } else {
            CatalogFailure::CommandFailed { detail }
        };
        return ModelCatalog {
            agent,
            list_command,
            models: Vec::new(),
            efforts,
            failure: Some(failure),
        };
    }

    let parsed = match agent {
        WorkerAgent::Codex => parse_codex_models(&output.stdout),
        WorkerAgent::Agy => parse_agy_models(&output.stdout),
        WorkerAgent::Claude => unreachable!("claude は catalog_argv が None"),
    };
    match parsed {
        Ok(models) => ModelCatalog {
            agent,
            list_command,
            models,
            efforts,
            failure: None,
        },
        Err(detail) => ModelCatalog {
            agent,
            list_command,
            models: Vec::new(),
            efforts,
            failure: Some(CatalogFailure::ParseFailed { detail }),
        },
    }
}

fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.ends_with("..."))
        .map(str::to_string)
}

/// 実環境から取る。CLI の探索は #983 の [`agent_cli::locate`]（境界 B16 経由）
pub fn catalog(agent: WorkerAgent) -> ModelCatalog {
    catalog_with(
        agent,
        agent_cli::locate,
        |path, args| {
            // #586: dispatch（GUI 内）からも呼ばれるのでコンソール窓を出さない
            let output = tako_core::platform::process::no_console_window(
                &mut std::process::Command::new(path),
            )
            .args(args)
            .output()
            .ok()?;
            Some(CommandRun {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        },
        claude_config_json().as_deref(),
    )
}

/// `~/.claude/.claude.json`（#558 で config dir 配下が正）を読む。失敗は None
fn claude_config_json() -> Option<String> {
    for path in crate::claude_tui::config_json_paths(None) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return Some(text);
        }
    }
    None
}

/// 全系統ぶん（`tako setup models` の既定表示 / MCP の一覧）
pub fn catalog_all() -> Vec<ModelCatalog> {
    WorkerAgent::ALL.into_iter().map(catalog).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実 CLI の出力を fixture として固定する（採取: 2026-08-27 / codex 0.150.1・agy 1.1.22）。
    /// codex 側は**読むフィールド + 読まないフィールド 2 つ**（`supported_in_api` / `shell_type`）を
    /// 残してあるので、「知らないキーを無視する」ことも同時に検査できる
    const CODEX_FIXTURE: &str = include_str!("../testdata/codex_debug_models.json");
    const AGY_FIXTURE: &str = include_str!("../testdata/agy_models.tsv");

    /// 未認証の agy の実出力（stderr。実測の文言そのまま）
    const AGY_UNAUTH_STDERR: &str = "Fetching available models...\n\
        Error: Please sign in to view available models. Launch the CLI without arguments to sign in.\n";

    fn run_ok(stdout: &str) -> impl FnOnce(&str, &[&str]) -> Option<CommandRun> + '_ {
        move |_, _| {
            Some(CommandRun {
                success: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            })
        }
    }

    fn found(_: WorkerAgent) -> Result<String, AgentCliError> {
        Ok("/test-stub/bin/agent".to_string())
    }

    fn never_run(_: &str, _: &[&str]) -> Option<CommandRun> {
        panic!("一覧コマンドを持たない系統で実行してはいけない");
    }

    #[test]
    fn codexのカタログから並べる候補だけを優先度順で取り出す() {
        let models = parse_codex_models(CODEX_FIXTURE).expect("パースできる");
        // visibility=hide（gpt-reserve / codex-auto-review）は並べない
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini"
            ],
            "priority 昇順で list のものだけが並ぶ"
        );
        let sol = &models[0];
        assert_eq!(sol.label, "GPT-5.6-Sol");
        assert_eq!(sol.source, ModelSource::Cli);
        assert_eq!(sol.default_effort.as_deref(), Some("low"));
        assert_eq!(sol.context_window, Some(272_000));
        // effort 語彙は**モデルごと**に違う（実測: luna は ultra を持たない）
        assert_eq!(
            sol.efforts,
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        let luna = models.iter().find(|m| m.id == "gpt-5.6-luna").unwrap();
        assert_eq!(luna.efforts, vec!["low", "medium", "high", "xhigh", "max"]);
    }

    #[test]
    fn codexのカタログが読めない形なら失敗を返す() {
        assert!(parse_codex_models("これは JSON ではない").is_err());
        assert!(parse_codex_models(r#"{"other": []}"#).is_err());
        // 並べられるモデルが 1 件も無い（全部 hide）のも失敗
        assert!(parse_codex_models(r#"{"models":[{"slug":"x","visibility":"hide"}]}"#).is_err());
    }

    #[test]
    fn agyのtsvからidと表示名を取り出す() {
        let models = parse_agy_models(AGY_FIXTURE).expect("パースできる");
        assert_eq!(models.len(), 14);
        assert_eq!(models[0].id, "gemini-3.7-flash-high");
        assert_eq!(models[0].label, "Gemini 3.7 Flash (High)");
        assert_eq!(models[0].source, ModelSource::Cli);
        // agy の effort は `--effort low|medium|high`（実測）。
        // 表示名の "(High)" はモデル側の設定なので effort 語彙とは別物
        assert_eq!(models[0].efforts, vec!["low", "medium", "high"]);
    }

    #[test]
    fn agyのtsvにタブが無い行は読み飛ばす() {
        let models = parse_agy_models("Fetching available models...\nid-a\tラベル A\n").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "id-a");
        assert!(parse_agy_models("タブが無い行だけ\n").is_err());
    }

    #[test]
    fn claudeは一覧コマンドを持たないので静的リストとキャッシュで代替する() {
        // 非公式のローカルキャッシュ（加算のみ）。実測の形をそのまま使う
        let config = r#"{"additionalModelOptionsCache":[
            {"value":"claude-fable-5[1m]","label":"Fable","description":"Most capable"}
        ],"other":1}"#;
        let catalog = catalog_with(WorkerAgent::Claude, found, never_run, Some(config));
        assert_eq!(catalog.failure, Some(CatalogFailure::NoListCommand));
        assert_eq!(catalog.list_command, None);
        assert!(!catalog.is_live(), "静的リストは実取得ではない");
        let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["opus", "sonnet", "fable", "claude-fable-5[1m]"]);
        assert_eq!(
            catalog.models.last().unwrap().source,
            ModelSource::LocalCache
        );
        // claude の effort 語彙は起動時に実際に使う値と同じ 1 本から来る
        assert_eq!(catalog.efforts, WorkerAgent::Claude.effort_options());
    }

    #[test]
    fn 壊れたキャッシュは黙って無視する() {
        assert!(parse_claude_cache("JSON ではない").is_empty());
        assert!(parse_claude_cache(r#"{"additionalModelOptionsCache":"配列でない"}"#).is_empty());
        assert!(
            parse_claude_cache(r#"{"additionalModelOptionsCache":[{"label":"値が無い"}]}"#)
                .is_empty()
        );
        // label が無ければ id を表示名に使う
        let models = parse_claude_cache(r#"{"additionalModelOptionsCache":[{"value":"m1"}]}"#);
        assert_eq!(models[0].label, "m1");
    }

    #[test]
    fn cliが無いときは983の導入案内をそのまま返す() {
        let missing = |agent: WorkerAgent| {
            Err(AgentCliError {
                agent,
                problem: agent_cli::AgentCliProblem::NotFound,
            })
        };
        let catalog = catalog_with(WorkerAgent::Codex, missing, never_run, None);
        let failure = catalog.failure.as_ref().expect("失敗する");
        assert_eq!(failure.kind(), "cli_not_found");
        assert!(catalog.models.is_empty());
        let msg = failure.message_in(WorkerAgent::Codex, Lang::Ja);
        assert!(msg.contains("次の一手"), "次の一手が無い: {msg}");
        assert!(msg.contains("install.sh"), "導入コマンドが無い: {msg}");
        // claude は CLI が無くても静的リストだけは出せる（選択肢を見せられる）
        let claude = catalog_with(WorkerAgent::Claude, missing, never_run, None);
        assert_eq!(claude.models.len(), 3);
        assert_eq!(claude.failure.as_ref().unwrap().kind(), "cli_not_found");
    }

    #[test]
    fn 未認証と単なる失敗を混ぜない() {
        let unauth = |_: &str, _: &[&str]| {
            Some(CommandRun {
                success: false,
                stdout: String::new(),
                stderr: AGY_UNAUTH_STDERR.to_string(),
            })
        };
        let catalog = catalog_with(WorkerAgent::Agy, found, unauth, None);
        let failure = catalog.failure.as_ref().unwrap();
        assert_eq!(failure.kind(), "not_authenticated");
        let msg = failure.message_in(WorkerAgent::Agy, Lang::Ja);
        // 進捗行（`...` で終わる）は詳細に採らない
        assert!(!msg.contains("Fetching available models"), "{msg}");
        assert!(msg.contains("Please sign in"), "生の詳細が要る: {msg}");
        assert!(msg.contains("agy"), "ログイン手順が要る: {msg}");

        let broken = |_: &str, _: &[&str]| {
            Some(CommandRun {
                success: false,
                stdout: String::new(),
                stderr: "panic: something else\n".to_string(),
            })
        };
        assert_eq!(
            catalog_with(WorkerAgent::Agy, found, broken, None)
                .failure
                .unwrap()
                .kind(),
            "command_failed"
        );
    }

    #[test]
    fn 実取得できたカタログはliveと判定される() {
        let catalog = catalog_with(WorkerAgent::Codex, found, run_ok(CODEX_FIXTURE), None);
        assert!(catalog.failure.is_none());
        assert!(catalog.is_live());
        assert_eq!(
            catalog.list_command.as_deref(),
            Some("codex debug models"),
            "取得元のコマンドを表示できる（受け入れ条件 2 の証拠）"
        );
        let json = catalog.to_json();
        assert_eq!(json["live"], true);
        assert_eq!(json["source"], "cli");
        // 失敗して 0 件のときは builtin と言わない（何も並べていないのだから）
        let empty = catalog_with(WorkerAgent::Agy, found, |_, _| None, None);
        assert_eq!(empty.source_label(), "none");
        assert_eq!(
            catalog_with(WorkerAgent::Claude, found, never_run, None).source_label(),
            "builtin"
        );
        assert_eq!(json["models"][0]["id"], "gpt-5.6-sol");
    }

    #[test]
    fn 書式が変わったらparse_failedで止まる() {
        let catalog = catalog_with(WorkerAgent::Codex, found, run_ok("{}"), None);
        let failure = catalog.failure.as_ref().unwrap();
        assert_eq!(failure.kind(), "parse_failed");
        assert!(catalog.models.is_empty(), "壊れた一覧を並べない");
        // 「モデル指定なしで CLI 既定に任せる」が次の一手（起動は止めない）
        let msg = failure.message_in(WorkerAgent::Codex, Lang::Ja);
        assert!(msg.contains("既定"), "{msg}");
    }

    #[test]
    fn 起動できなければcommand_failed() {
        let catalog = catalog_with(WorkerAgent::Agy, found, |_, _| None, None);
        assert_eq!(catalog.failure.unwrap().kind(), "command_failed");
    }

    #[test]
    fn 全ての失敗が日英で理由と次の一手を持つ() {
        let failures = [
            CatalogFailure::NotInstalled(AgentCliError {
                agent: WorkerAgent::Codex,
                problem: agent_cli::AgentCliProblem::NotFound,
            }),
            CatalogFailure::NotAuthenticated {
                detail: "detail".into(),
            },
            CatalogFailure::NoListCommand,
            CatalogFailure::CommandFailed {
                detail: "detail".into(),
            },
            CatalogFailure::ParseFailed {
                detail: "detail".into(),
            },
        ];
        for failure in &failures {
            for agent in WorkerAgent::ALL {
                for (lang, needle) in [(Lang::Ja, "次の一手"), (Lang::En, "Next:")] {
                    let msg = failure.message_in(agent, lang);
                    assert!(
                        msg.contains(needle),
                        "{:?} / {agent:?} / {lang:?} に次の一手が無い: {msg}",
                        failure.kind()
                    );
                }
            }
            assert!(!failure.kind().is_empty());
        }
    }

    #[test]
    fn 一覧コマンドの正本は1箇所で表示と実行が同じ形を使う() {
        // claude だけが「持たない」。持つ系統は表示文字列と argv が一致する
        assert_eq!(catalog_argv(WorkerAgent::Claude), None);
        assert_eq!(catalog_command(WorkerAgent::Claude), None);
        for agent in [WorkerAgent::Codex, WorkerAgent::Agy] {
            let args = catalog_argv(agent).expect("一覧コマンドを持つ");
            assert_eq!(
                catalog_command(agent).unwrap(),
                format!("{} {}", agent.as_str(), args.join(" "))
            );
        }
    }

    #[test]
    fn effort語彙は起動時に使う正本と同じ() {
        // ここがずれると「ピッカーで選べたのに起動で弾かれる」が起きる
        for agent in WorkerAgent::ALL {
            let catalog_efforts = agent_efforts(agent);
            assert_eq!(catalog_efforts, agent.effort_options());
        }
        // agy は #1002 で実測した 3 値（CLI 自身が valid として挙げる）
        assert_eq!(agent_efforts(WorkerAgent::Agy), ["low", "medium", "high"]);
    }
}

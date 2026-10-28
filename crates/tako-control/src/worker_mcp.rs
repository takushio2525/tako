//! worker_mcp — tako が起こす claude の worker へ持ち込む MCP サーバーを絞る（Issue #2412）
//!
//! # なぜ要るのか
//!
//! claude は起動のたびに **user scope**（`<config dir>/.claude.json` の `mcpServers`）の
//! MCP サーバーをすべて起こす。個人アカウントの設定には 8 つあり、`npx` / `uvx` 経由で
//! 1 本あたり 12〜15 プロセス・約 400 MB。2026-10-25 に worker 40 本ぶんの MCP 系
//! （650 プロセス）でメモリが尽きて PC が落ちた（記録は #1992 のコメント）。worker が
//! 実際に使うのはほとんどが tako の MCP だけなので、**worker は既定で tako の MCP だけ**を
//! 持って起動し、要るものはプロジェクト単位で足す（ユーザーの判断 2026-10-26）。
//!
//! # 方針（実測 claude 2.1.301・一時の設定 dir + 偽 API + 起動を記録する偽 MCP）
//!
//! - **`--settings <ファイル>` の `deniedMcpServers` に user scope の名前を並べる**。
//!   止まるのは名前が一致したものだけで、`.mcp.json`・local scope・plugin・claude.ai の
//!   コネクタはそのまま立つ
//! - `--strict-mcp-config` は採らない: `.mcp.json` と local scope まで止まるので、
//!   claude の併合の規則（承認・祖先の探索・git の根）を tako が写し直すことになる
//! - `--setting-sources project,local` も採らない: user scope の MCP は止まるが、
//!   user の settings.json（フック・権限・env）まで落ちる
//! - **同名は止めない**: deny は名前で効くので、`.mcp.json` / local scope と同名の
//!   user scope を止めるとプロジェクト側まで止まる（実測）。同名なら claude は
//!   プロジェクト側を採るので、user 側は元々立たない
//! - `.mcp.json` は cwd と**祖先**のものが読まれ、local scope は **git の根**を鍵に引かれる
//!   （実測）。同名の判定はどちらも広めに（cwd・正規化した cwd とそれぞれの祖先）見る。
//!   広めに見て損をするのは「同名の user 側を止め損ねる」だけで、その user 側は
//!   どのみち立たない
//! - `--settings` を 2 つ渡すと**後ろが勝つ**（前の `deniedMcpServers` は消える = 実測）。
//!   プロファイルの `worker_agents.claude.args` に `--settings` があると絞り込みは効かないので
//!   warnings で知らせる（利用者の `--settings` を消して上書きはしない）
//! - JSON をファイルにして**起動コマンドにはパスだけ**を載せる。引数の中の `"` の扱いは
//!   シェルと版で揺れる（PowerShell 7.3 前の native 引数）ので、JSON を引数へ直に書かない
//!
//! user scope は**知っている config dir をすべて**読んで和をとる（spawn が解決した
//! アカウント・tako の既定・accounts.yaml の全アカウント）。direnv がフォルダごとに
//! `CLAUDE_CONFIG_DIR` を差し替える環境では、tako には実際に読まれる dir が分からない。
//! 実在しない名前を止めても何も起きないので、和をとる側に倒す。
//!
//! # codex（Issue #2418。実測 codex-cli 0.154.0・一時の CODEX_HOME + 偽 Responses API + 起動を記録する偽 MCP）
//!
//! 判断（止める / 持ち込む / 同名は止めない）は claude と同じ [`plan`]。違うのは読む場所と渡し方だけ:
//!
//! - **user 設定**は `$CODEX_HOME/config.toml` の `[mcp_servers.*]` 1 本（[`codex_home_for`]）
//! - **起動ごとに `-c mcp_servers.<名>.enabled=false`** で止める。実行時にその server の子は立たない
//! - **知らない名前を `enabled=false` だけで渡すと codex が起動しない**（`invalid transport` で終了 1）。
//!   tako が読んだ config.toml と worker のシェルが読むもの（`CODEX_HOME` を rc や direnv が差し替える）が
//!   ずれても worker を落とさないよう、**読んだ transport と同じ種類の目印**（stdio = `command`・
//!   http = `url`）を一緒に渡して、それだけで成り立つ項目にする（[`codex_off_args`]）。止めた項目なので
//!   目印のコマンドが走ることは無い。種類を違えると（url の server に `command`）起動しない
//! - `-c` のキーは `.` で区切られ引用符も効かないので、名前に `.` や空白などを含む server は止められない
//!   （付けると起動しない）。警告して持ち込む
//! - **プロジェクトの `.codex/config.toml` と同名は止めない**: プロジェクト側の定義が勝つが、
//!   `enabled=false` は同名のプロジェクト側まで止める（claude の deny と同じ）。codex が読むのは
//!   cwd から git の根まで（git が無ければ cwd だけ）の、信頼済みのフォルダの `.codex/config.toml`。
//!   信頼は見ずに広めに数える（損をするのは同名の user 側を止め損ねるだけ）。祖先を無制限に辿ると
//!   ホームの `~/.codex/config.toml`（user 設定そのもの）を拾って何も止めなくなるので、git の根で止める
//! - config.toml が壊れていると codex 自身が起動しない。tako は何も足さずに警告する
//!
//! agy の扱いは能力マトリクス（`worker_mcp_scope`）に宣言してある。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::{json, Value};

use crate::orchestrator::agent::WorkerAgent;
use crate::orchestrator::{EnvPlan, ProjectsConfig};

/// `mcp_servers` に書くと user scope を**全部持ち込む**（絞らない）印
pub const KEEP_ALL: &str = "*";

/// codex の止めた stdio の server へ渡す目印のコマンド（#2418。止めた項目なので走らない）。
/// 引用符を使わない（TOML として読めない値は codex がそのまま文字列にする = `--help` の記述・実測。
/// PowerShell 7.3 前の native 引数で `"` が落ちても同じ値になる）
pub const CODEX_OFF_COMMAND: &str = "tako-disabled";

/// codex の止めた http の server へ渡す目印の URL（同上。繋ぎに行くことは無い）
pub const CODEX_OFF_URL: &str = "http://127.0.0.1:9/tako-disabled";

/// codex の worker の A/B（`TAKO_2418_LEGACY=1` で codex だけ #2418 前 = user 設定を全部持ち込む）
pub fn legacy_codex() -> bool {
    std::env::var("TAKO_2418_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 再開（resume）の A/B（`TAKO_2447_LEGACY=1` で #2447 前 = 再起動をまたいだ復元と
/// カタログの再開に MCP の断片を載せない。spawn と `tako sessions resume` の claude の worker は変わらない）
pub fn legacy_resume() -> bool {
    std::env::var("TAKO_2447_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 実績として読む transcript の窓（最終更新がこれより新しいもの）
pub const EVIDENCE_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// 絞り込みの設定ファイルを置く data dir 配下のフォルダ
const SETTINGS_DIR: &str = "worker-mcp";

/// A/B の逃げ道。`TAKO_2412_LEGACY=1` で**同一バイナリのまま**
/// 「user scope を全部持ち込む」#2412 前の起動へ戻す
pub fn legacy() -> bool {
    std::env::var("TAKO_2412_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// claude の MCP ツール名（`mcp__<server>__<tool>`）に入るサーバー名の形。
/// 英数字・`_`・`-` 以外を `_` へ置き換える（`claude.ai Gmail` → `claude_ai_Gmail`）
pub fn tool_server_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// claude が会話を置く `<config dir>/projects/` のフォルダ名。
/// 英数字以外を `-` へ置き換える（JavaScript の文字列 = UTF-16 の code unit ごと。
/// サロゲートペアの文字は `-` が 2 つになる）
pub fn project_slug(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('-');
            }
        }
    }
    out
}

/// tako の MCP ブリッジの登録か。名前が `tako`（`tako setup-mcp` の既定 = #979）か、
/// 中身が `<…>/tako mcp serve`（別名で登録した場合）。**絞り込みの対象から常に外す**
pub fn is_tako_server(name: &str, def: &Value) -> bool {
    if name == crate::agent_mcp::SERVER_NAME {
        return true;
    }
    let args: Vec<&str> = def
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let command_is_tako = def
        .get("command")
        .and_then(Value::as_str)
        .and_then(|c| Path::new(c).file_stem())
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "tako" || s == "tako-mcp");
    command_is_tako && args == ["mcp", "serve"]
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// user scope を読む `.claude.json` の候補。
///
/// spawn が解決した config dir（アカウント指定の `CLAUDE_CONFIG_DIR`）・tako の既定
/// （`claude_tui::config_json_paths` と同じ解決 = 検証プロセスでは隔離先）・accounts.yaml の
/// 全アカウントの和。同じ場所は 1 回だけ
pub fn user_scope_files(explicit_config_dir: Option<&str>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !files.contains(&p) {
            files.push(p);
        }
    };
    for p in crate::claude_tui::config_json_paths(explicit_config_dir) {
        push(p);
    }
    for p in crate::claude_tui::config_json_paths(None) {
        push(p);
    }
    if let Ok(accounts) = crate::orchestrator::AccountsConfig::load() {
        for (_, resolved) in accounts.list_resolved() {
            if let Some(dir) = resolved
                .ok()
                .and_then(|a| a.config_dir.path().map(PathBuf::from))
            {
                push(dir.join(".claude.json"));
            }
        }
    }
    files
}

/// user scope の MCP サーバー（名前 → 定義）。先に読んだファイルの定義を採る
pub fn user_scope(files: &[PathBuf]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for file in files {
        let Some(root) = read_json(file) else {
            continue;
        };
        if let Some(servers) = root.get("mcpServers").and_then(Value::as_object) {
            for (name, def) in servers {
                out.entry(name.clone()).or_insert_with(|| def.clone());
            }
        }
    }
    out
}

/// cwd・正規化した cwd と、それぞれの祖先
fn cwd_and_ancestors(cwd: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let canonical = tako_core::platform::path::canonicalize(cwd).ok();
    for base in std::iter::once(cwd.to_path_buf()).chain(canonical) {
        for dir in base.ancestors() {
            let dir = dir.to_path_buf();
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
    }
    out
}

/// プロジェクト側で宣言された MCP サーバー名（`.mcp.json` と local scope）。
/// これと同名の user scope は止めない（止めるとプロジェクト側まで止まる）
pub fn project_declared(cwd: &Path, files: &[PathBuf]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let dirs = cwd_and_ancestors(cwd);
    for dir in &dirs {
        if let Some(root) = read_json(&dir.join(".mcp.json")) {
            if let Some(servers) = root.get("mcpServers").and_then(Value::as_object) {
                out.extend(servers.keys().cloned());
            }
        }
    }
    for file in files {
        let Some(root) = read_json(file) else {
            continue;
        };
        let Some(projects) = root.get("projects").and_then(Value::as_object) else {
            continue;
        };
        for dir in &dirs {
            let key = dir.to_string_lossy();
            if let Some(servers) = projects
                .get(key.as_ref())
                .and_then(|p| p.get("mcpServers"))
                .and_then(Value::as_object)
            {
                out.extend(servers.keys().cloned());
            }
        }
    }
    out
}

/// 1 本の worker の MCP の構成（応答・CLI の表示と同じ形）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct McpPlan {
    /// 止める user scope の名前（`deniedMcpServers` に並ぶ）
    pub denied: Vec<String>,
    /// プロジェクトの設定で持ち込む user scope の名前
    pub kept: Vec<String>,
    /// 設定にあるが user scope にもプロジェクト側にも見つからない名前（今は効果が無い）
    pub missing: Vec<String>,
    /// user scope にあるが `.mcp.json` / local scope と同名なので止めない名前
    pub shadowed: Vec<String>,
    /// tako の MCP ブリッジ（常に持ち込む）
    pub tako: Vec<String>,
    /// 設定が `*` = user scope を全部持ち込む
    pub keep_all: bool,
}

/// 構成を決める（純関数）。`keep` はプロジェクトの `mcp_servers`
pub fn plan(
    user: &BTreeMap<String, Value>,
    declared: &BTreeSet<String>,
    keep: &[String],
) -> McpPlan {
    let keep_all = keep.iter().any(|k| k.trim() == KEEP_ALL);
    let keep: BTreeSet<&str> = keep
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty() && *k != KEEP_ALL)
        .collect();
    let mut p = McpPlan {
        keep_all,
        ..Default::default()
    };
    for (name, def) in user {
        if is_tako_server(name, def) {
            p.tako.push(name.clone());
        } else if declared.contains(name) {
            p.shadowed.push(name.clone());
        } else if keep_all || keep.contains(name.as_str()) {
            p.kept.push(name.clone());
        } else {
            p.denied.push(name.clone());
        }
    }
    p.missing = keep
        .iter()
        .filter(|k| !user.contains_key(**k) && !declared.contains(**k))
        .map(|k| k.to_string())
        .collect();
    p
}

/// `--settings` に渡す JSON（`deniedMcpServers` だけ）
pub fn settings_json(denied: &[String]) -> String {
    let list: Vec<Value> = denied.iter().map(|n| json!({ "serverName": n })).collect();
    let mut s = serde_json::to_string_pretty(&json!({ "deniedMcpServers": list }))
        .unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

/// 絞り込みの設定ファイルの置き場と中身（**書かない**）。名前は中身のハッシュ
/// （同じ構成は同じファイル = worker の本数ぶん増えない）
pub fn settings_file_for(denied: &[String]) -> Result<(PathBuf, String), String> {
    use sha2::Digest;
    let body = settings_json(denied);
    let digest = sha2::Sha256::digest(body.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let dir = tako_core::paths::data_dir()
        .ok_or("データディレクトリを特定できない")?
        .join(SETTINGS_DIR);
    Ok((dir.join(format!("deny-{hex}.json")), body))
}

/// 設定ファイルを書く。中身が同じファイルが既にあれば書かない
fn write_settings(path: &Path, body: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(body) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れない: {e}", dir.display()))?;
    }
    crate::config_io::atomic_write(path, body)
}

/// プロジェクトの `mcp_servers`（無ければ空 = tako だけ）
pub fn keep_for_project(config: &ProjectsConfig, project: &str) -> Vec<String> {
    config
        .projects
        .get(project)
        .map(|e| e.mcp_servers.clone())
        .unwrap_or_default()
}

/// cwd が一致するプロジェクトの `mcp_servers`（git resolve のように
/// プロジェクトキーを持たない起動で使う）。一致が無ければ空
pub fn keep_for_cwd(config: &ProjectsConfig, cwd: &Path) -> Vec<String> {
    let norm = |p: &Path| {
        tako_core::platform::path::canonicalize(p).unwrap_or_else(|_| p.components().collect())
    };
    let target = norm(cwd);
    config
        .list_resolved()
        .into_iter()
        .find(|p| norm(Path::new(&p.cwd)) == target)
        .map(|p| keep_for_project(config, &p.key))
        .unwrap_or_default()
}

/// 起動の 1 回ぶんの判断（spawn / git resolve / 再開が通す 1 実装）
#[derive(Debug, Clone, Default)]
pub struct WorkerMcp {
    /// どう起動するか（応答の `mcp.mode`。安定した語彙）
    pub mode: &'static str,
    pub plan: McpPlan,
    /// `--settings` に渡すファイル（止めるものが無ければ None = 引数を足さない）
    pub settings_file: Option<String>,
    /// 書き出す中身（[`WorkerMcp::materialize`] が書く）
    settings_body: Option<String>,
    /// codex の起動コマンドで止める server（`-c mcp_servers.<名>.enabled=false`。#2418）。
    /// claude / agy では常に空
    pub codex_off: Vec<CodexOff>,
    /// 読んだ codex の user 設定（応答の `config_file`。codex だけ）
    pub config_file: Option<String>,
    /// 応答の warnings と persist.log へ出す文
    pub warnings: Vec<String>,
}

/// codex の worker で止める 1 つの server（#2418）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOff {
    pub name: String,
    /// tako が読んだ定義が http（`url`）か。目印の種類を合わせる（違えると codex が起動しない）
    pub http: bool,
}

impl WorkerMcp {
    /// 応答の `mcp` 欄
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "mode": self.mode,
            "denied": self.plan.denied,
            "kept": self.plan.kept,
            "missing": self.plan.missing,
            "shadowed": self.plan.shadowed,
            "settings_file": self.settings_file,
        });
        // codex は読んだ config.toml を見せる（worker のシェルが別の CODEX_HOME を読む食い違いの手がかり）
        if let Some(file) = &self.config_file {
            v["config_file"] = json!(file);
        }
        v
    }

    /// 設定ファイルを書く（中身が同じなら書かない）。起動の直前に呼ぶ
    /// （再開の下見では呼ばない = `resume_launch::ResumeLaunch::materialize` と同じ作法）
    pub fn materialize(&self) -> Result<(), String> {
        match (&self.settings_file, &self.settings_body) {
            (Some(path), Some(body)) => write_settings(Path::new(path), body),
            _ => Ok(()),
        }
    }

    /// spawn 用: 書いてから使う。書けなければ**絞り込みをやめて全部持ち込む**
    /// （起動は止めない。理由は warnings へ）
    pub fn materialized(mut self) -> Self {
        if let Err(e) = self.materialize() {
            self.mode = "unrestricted";
            self.settings_file = None;
            self.settings_body = None;
            self.warnings.push(format!(
                "user scope の MCP を止める設定ファイルを書けなかったので、今回は全部持ち込んで起動する: {e}"
            ));
        }
        self
    }
}

/// 再開（resume）で起こす worker の MCP を決める（#2447。**spawn と同じ [`prepare`] を通す**）。
///
/// 持ち込む名前は spawn と同じくプロジェクトの `mcp_servers`（[`keep_for_project`]）。cwd は会話の
/// 作業 dir、記録に無ければプロジェクトの cwd（spawn が起動した場所）。`env` は**会話の記録がある**
/// 設定の置き場を指す計画（claude = 会話の config dir・codex = rollout を探した `CODEX_HOME`）
pub fn prepare_for_resume(
    agent: WorkerAgent,
    project: Option<&str>,
    cwd: Option<&str>,
    env: &EnvPlan,
    extra_args: &[String],
) -> WorkerMcp {
    let config = ProjectsConfig::load().ok();
    let keep = match (&config, project) {
        (Some(c), Some(p)) => keep_for_project(c, p),
        _ => Vec::new(),
    };
    let cwd = cwd
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let p = project?;
            config
                .as_ref()?
                .list_resolved()
                .into_iter()
                .find(|r| r.key == p)
                .map(|r| PathBuf::from(r.cwd))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    prepare(agent, &cwd, env, &keep, extra_args)
}

/// worker の MCP の構成を決める（ファイルは書かない。書くのは [`WorkerMcp::materialize`]）。
///
/// `env` は起動する worker の env 計画（claude の config dir・codex の `CODEX_HOME` を引く）。
/// `extra_args` はプロファイルの `worker_agents.claude.args`（`--settings` の重複の検査に使う）
pub fn prepare(
    agent: WorkerAgent,
    cwd: &Path,
    env: &EnvPlan,
    keep: &[String],
    extra_args: &[String],
) -> WorkerMcp {
    match agent {
        WorkerAgent::Claude => {
            if legacy() {
                return WorkerMcp {
                    mode: "legacy",
                    ..Default::default()
                };
            }
            prepare_from(
                cwd,
                &user_scope_files(env.claude_config_dir().as_deref()),
                keep,
                extra_args,
            )
        }
        WorkerAgent::Codex => {
            if legacy() || legacy_codex() {
                return WorkerMcp {
                    mode: "legacy",
                    ..Default::default()
                };
            }
            match codex_home_for(env) {
                Some(home) => prepare_codex_from(cwd, &home.join("config.toml"), keep),
                None => WorkerMcp {
                    mode: "unrestricted",
                    warnings: vec![
                        "codex の設定の置き場（CODEX_HOME）を決められないので、今回は user 設定の MCP を全部持ち込んで起動する"
                            .to_string(),
                    ],
                    ..Default::default()
                },
            }
        }
        // agy は能力マトリクス（worker_mcp_scope）のとおり絞らない（起動ごとの手段が CLI に無い）
        WorkerAgent::Agy => WorkerMcp {
            mode: "not_supported",
            ..Default::default()
        },
    }
}

/// [`prepare`] の本体（claude の worker・読む `.claude.json` を引数で受ける = テストが一時 dir を渡す）
pub fn prepare_from(
    cwd: &Path,
    files: &[PathBuf],
    keep: &[String],
    extra_args: &[String],
) -> WorkerMcp {
    let user = user_scope(files);
    let declared = project_declared(cwd, files);
    let plan = plan(&user, &declared, keep);
    let mut out = WorkerMcp {
        mode: if plan.keep_all {
            "keep_all"
        } else {
            "tako_only"
        },
        plan,
        ..Default::default()
    };
    for name in &out.plan.missing {
        out.warnings.push(format!(
            "プロジェクトの mcp_servers の '{name}' は user scope にもプロジェクトの .mcp.json にも見つからない（今は何も足されない）"
        ));
    }
    if out.plan.denied.is_empty() {
        return out;
    }
    if extra_args
        .iter()
        .any(|a| a == "--settings" || a.starts_with("--settings="))
    {
        out.mode = "overridden";
        out.warnings.push(
            "プロファイルの worker_agents.claude.args に --settings があるため、user scope の MCP を止める設定が効かない（claude は後ろの --settings だけを読む）"
                .to_string(),
        );
        return out;
    }
    match settings_file_for(&out.plan.denied) {
        Ok((path, body)) => {
            out.settings_file = Some(path.display().to_string());
            out.settings_body = Some(body);
        }
        Err(e) => {
            out.mode = "unrestricted";
            out.warnings.push(format!(
                "user scope の MCP を止める設定ファイルの置き場が無いので、今回は全部持ち込んで起動する: {e}"
            ));
        }
    }
    out
}

// --- codex（Issue #2418） -----------------------------------------------------

/// codex の worker が読む `CODEX_HOME`。
///
/// env 計画の export（プロファイル / アカウントの `env:`）→ 明示の unset なら既定の `~/.codex` →
/// 検証プロセスでは検証用のホームの `.codex`（本物の設定を読まない。usage の `codex_homes` と同じ）→
/// tako の環境変数 `CODEX_HOME` → `~/.codex`
pub fn codex_home_for(env: &EnvPlan) -> Option<PathBuf> {
    const KEY: &str = "CODEX_HOME";
    if let Some(v) = env.export_value(KEY).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    if env.unsets.iter().any(|k| k == KEY) || tako_core::paths::is_verification_process() {
        return crate::orchestrator::agent_config_home().map(|h| h.join(".codex"));
    }
    crate::codex_session::codex_home()
}

/// codex の設定（TOML）の `mcp_servers`（名前 → 定義を JSON にしたもの）。
/// ファイルが無ければ空。読めない・TOML として壊れていれば Err
pub fn codex_servers(path: &Path) -> Result<BTreeMap<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("{} を読めない: {e}", path.display())),
    };
    let root: Value = toml::from_str(&text)
        .map_err(|e| format!("{} を TOML として読めない: {e}", path.display()))?;
    Ok(root
        .get("mcp_servers")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default())
}

/// codex がプロジェクトの `.codex/config.toml` を探すフォルダ: cwd から git の根まで
/// （git が無ければ cwd だけ = 実測）。cwd と正規化した cwd の両方から辿る
fn codex_project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let canonical = tako_core::platform::path::canonicalize(cwd).ok();
    for base in std::iter::once(cwd.to_path_buf()).chain(canonical) {
        let chain: Vec<&Path> = base.ancestors().collect();
        let root = chain.iter().position(|d| d.join(".git").exists());
        let upto = root.map_or(1, |i| i + 1);
        for dir in &chain[..upto] {
            let dir = dir.to_path_buf();
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
    }
    out
}

/// プロジェクトの `.codex/config.toml` で宣言された server 名（これと同名の user 設定は止めない）。
/// user 設定そのもの（cwd がホームで `~/.codex/config.toml` を指す）は数えない。壊れたファイルは読み飛ばす
pub fn codex_project_declared(cwd: &Path, user_config: &Path) -> BTreeSet<String> {
    let same = |a: &Path, b: &Path| {
        let norm =
            |p: &Path| tako_core::platform::path::canonicalize(p).unwrap_or_else(|_| p.into());
        a == b || norm(a) == norm(b)
    };
    let mut out = BTreeSet::new();
    for dir in codex_project_dirs(cwd) {
        let file = dir.join(".codex").join("config.toml");
        if !file.is_file() || same(&file, user_config) {
            continue;
        }
        if let Ok(servers) = codex_servers(&file) {
            out.extend(servers.into_keys());
        }
    }
    out
}

/// codex の `-c` のキーに書ける名前か。`codex mcp add` が許す文字（英数字・`-`・`_`・`:`・`@`・`/`・`.`）から
/// `.` を除いたものに限る（キーは `.` で区切られ、引用符も効かない = 実測）。`:` `@` `/` と、手書きの
/// config.toml の Unicode の英数字は `-c` で通る（実測）。空白などは止めずに持ち込む側へ倒す
pub fn codex_name_expressible(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '@' | '/'))
}

/// [`prepare`] の codex の本体（読む config.toml を引数で受ける = テストが一時 dir を渡す）
pub fn prepare_codex_from(cwd: &Path, user_config: &Path, keep: &[String]) -> WorkerMcp {
    let config_file = Some(user_config.display().to_string());
    let user = match codex_servers(user_config) {
        Ok(u) => u,
        Err(e) => {
            return WorkerMcp {
                mode: "unrestricted",
                config_file,
                warnings: vec![format!(
                    "codex の設定を読めないので、今回は user 設定の MCP に何も足さずに起動する: {e}"
                )],
                ..Default::default()
            }
        }
    };
    let declared = codex_project_declared(cwd, user_config);
    let mut plan = plan(&user, &declared, keep);
    let mut warnings: Vec<String> = plan
        .missing
        .iter()
        .map(|name| {
            format!(
                "プロジェクトの mcp_servers の '{name}' は codex の config.toml にもプロジェクトの .codex/config.toml にも見つからない（今は何も足されない）"
            )
        })
        .collect();
    let (ok, odd): (Vec<String>, Vec<String>) = std::mem::take(&mut plan.denied)
        .into_iter()
        .partition(|n| codex_name_expressible(n));
    for name in odd {
        warnings.push(format!(
            "codex の -c は名前の '.' や空白などを表せないので、'{name}' は止めずに持ち込む"
        ));
    }
    plan.denied = ok;
    let codex_off = plan
        .denied
        .iter()
        .map(|name| CodexOff {
            name: name.clone(),
            http: user
                .get(name)
                .is_some_and(|d| d.get("command").is_none() && d.get("url").is_some()),
        })
        .collect();
    WorkerMcp {
        mode: if plan.keep_all {
            "keep_all"
        } else {
            "tako_only"
        },
        plan,
        codex_off,
        config_file,
        warnings,
        ..Default::default()
    }
}

/// codex の起動コマンドへ足す断片（`-c mcp_servers.<名>.enabled=false` + 目印。正本はこの 1 実装）。
/// 空なら 1 バイトも足さない
pub fn codex_off_args(off: &[CodexOff], dialect: crate::launch_cmd::ShellDialect) -> String {
    use crate::launch_cmd as lc;
    let mut s = String::new();
    for o in off {
        let name = &o.name;
        let (key, value) = if o.http {
            ("url", CODEX_OFF_URL)
        } else {
            ("command", CODEX_OFF_COMMAND)
        };
        s.push_str(&format!(
            " -c {}",
            lc::quote(dialect, &format!("mcp_servers.{name}.enabled=false"))
        ));
        s.push_str(&format!(
            " -c {}",
            lc::quote(dialect, &format!("mcp_servers.{name}.{key}={value}"))
        ));
    }
    s
}

// --- 実績（移行で既定に残すもの） --------------------------------------------

/// 1 プロジェクト × 1 サーバーの実績（本文は持たない = 集計だけ）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageRow {
    pub project: String,
    pub server: String,
    /// そのサーバーを 1 回以上呼んだ会話の数
    pub sessions: u32,
    /// 呼び出しの総数
    pub calls: u32,
}

/// 実績を読む config dir。**テストのプロセスでは読まない**（本物の transcript を
/// 読みに行かない。純粋版 [`usage_evidence_in`] へ一時 dir を渡して検査する）
pub fn evidence_config_dirs() -> Vec<PathBuf> {
    if tako_core::paths::is_test_process() {
        return Vec::new();
    }
    crate::transcript::claude_config_dirs()
}

/// 実績の user scope の名前を読む `.claude.json`（config dir 群 + ホーム直下の旧置き場）
fn evidence_user_files(config_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = config_dirs.iter().map(|d| d.join(".claude.json")).collect();
    if !tako_core::paths::is_test_process() {
        if let Some(home) = crate::orchestrator::agent_config_home() {
            let legacy = home.join(".claude.json");
            if !files.contains(&legacy) {
                files.push(legacy);
            }
        }
    }
    files
}

/// 1 本の transcript から `"name":"mcp__<server>__` を数える（`tako_core::mcp_calls`）。
///
/// **大きな塊のバイト列のまま、改行で切ってから探す**（行ごとに文字列を作って `str` で探すと
/// 6 GB に release でも 4 秒かかった = 実測）。塊の末尾の書きかけの行は次の塊へ持ち越すので、
/// メモリは塊 + 最長の行ぶんだけ
fn scan_transcript(path: &Path, out: &mut BTreeMap<String, u32>) {
    use std::io::Read;
    const CHUNK: usize = 4 << 20;
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let mut chunk = vec![0u8; CHUNK];
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
    loop {
        let n = match file.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => 0,
        };
        buf.extend_from_slice(&chunk[..n]);
        let at_end = n == 0;
        // 探すのは最後の改行まで（終わりなら全部）
        let upto = if at_end {
            buf.len()
        } else {
            match buf.iter().rposition(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => continue, // 1 行が塊より長い = 改行まで読み足す
            }
        };
        tako_core::mcp_calls::count_calls(&buf[..upto], out);
        buf.drain(..upto);
        if at_end {
            break;
        }
    }
}

/// プロジェクトごとに、窓の内の transcript で呼ばれた user scope の MCP を数える（純粋版）。
///
/// `projects` は（キー, cwd）。会話の置き場は claude と同じく起動した cwd の
/// スラグで引く（worker は spawn の cwd = プロジェクトの cwd で起動する）。
/// tako の MCP と、user scope に無い名前（claude.ai のコネクタ・plugin）は数えない
pub fn usage_evidence_in(
    projects: &[(String, String)],
    config_dirs: &[PathBuf],
    user_names: &BTreeSet<String>,
    since: SystemTime,
) -> Vec<UsageRow> {
    // ツール名の形 → user scope の名前
    let by_tool_name: BTreeMap<String, &String> = user_names
        .iter()
        .filter(|n| n.as_str() != crate::agent_mcp::SERVER_NAME)
        .map(|n| (tool_server_name(n), n))
        .collect();
    let mut rows = Vec::new();
    for (key, cwd) in projects {
        let slug = project_slug(cwd);
        // サーバー名 → (会話数, 呼び出し数)
        let mut per: BTreeMap<&String, (u32, u32)> = BTreeMap::new();
        for dir in config_dirs {
            let Ok(entries) = std::fs::read_dir(dir.join("projects").join(&slug)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let fresh = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t >= since);
                if !fresh {
                    continue;
                }
                let mut counts = BTreeMap::new();
                scan_transcript(&path, &mut counts);
                for (tool_name, calls) in counts {
                    if let Some(name) = by_tool_name.get(&tool_name) {
                        let e = per.entry(name).or_default();
                        e.0 += 1;
                        e.1 += calls;
                    }
                }
            }
        }
        for (server, (sessions, calls)) in per {
            rows.push(UsageRow {
                project: key.clone(),
                server: server.clone(),
                sessions,
                calls,
            });
        }
    }
    rows
}

/// 実績を projects.yaml の各プロジェクトの `mcp_servers` へ足す（既にある名前は触らない）。
/// 足した（キー, 名前）を返す
pub fn adopt_usage(config: &mut ProjectsConfig, rows: &[UsageRow]) -> Vec<(String, String)> {
    let mut added = Vec::new();
    for row in rows {
        let Some(entry) = config.projects.get_mut(&row.project) else {
            continue;
        };
        if entry
            .mcp_servers
            .iter()
            .any(|s| s == &row.server || s == KEEP_ALL)
        {
            continue;
        }
        entry.mcp_servers.push(row.server.clone());
        entry.mcp_servers.sort();
        added.push((row.project.clone(), row.server.clone()));
    }
    added
}

/// 移行の本体（純粋版 = 読む config dir を引数で受ける）。窓は今から [`EVIDENCE_WINDOW`]
pub fn adopt_usage_from(
    config: &mut ProjectsConfig,
    config_dirs: &[PathBuf],
) -> Vec<(String, String)> {
    let files = evidence_user_files(config_dirs);
    let user_names: BTreeSet<String> = user_scope(&files).into_keys().collect();
    if user_names.is_empty() {
        return Vec::new();
    }
    let projects: Vec<(String, String)> = config
        .list_resolved()
        .into_iter()
        .map(|p| (p.key, p.cwd))
        .collect();
    let since = SystemTime::now()
        .checked_sub(EVIDENCE_WINDOW)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let rows = usage_evidence_in(&projects, config_dirs, &user_names, since);
    adopt_usage(config, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srv(cmd: &str, args: &[&str]) -> Value {
        json!({ "type": "stdio", "command": cmd, "args": args })
    }

    fn users(names: &[&str]) -> BTreeMap<String, Value> {
        names
            .iter()
            .map(|n| (n.to_string(), srv("npx", &["-y", n])))
            .collect()
    }

    #[test]
    fn 既定はtakoだけを残してuser_scopeを止める() {
        let mut u = users(&["github", "playwright", "blender"]);
        u.insert(
            "tako".into(),
            srv(
                "/Applications/tako.app/Contents/MacOS/tako",
                &["mcp", "serve"],
            ),
        );
        let p = plan(&u, &BTreeSet::new(), &[]);
        assert_eq!(p.denied, ["blender", "github", "playwright"]);
        assert_eq!(p.tako, ["tako"]);
        assert!(p.kept.is_empty() && p.missing.is_empty() && !p.keep_all);
    }

    #[test]
    fn 別名で登録したtakoのブリッジも止めない() {
        let mut u = users(&["github"]);
        u.insert(
            "tako-dev".into(),
            srv("/x/target/debug/tako", &["mcp", "serve"]),
        );
        u.insert("tako-like".into(), srv("/x/tako", &["serve"]));
        let p = plan(&u, &BTreeSet::new(), &[]);
        assert_eq!(p.tako, ["tako-dev"]);
        assert_eq!(p.denied, ["github", "tako-like"]);
    }

    #[test]
    fn プロジェクトで足した名前は持ち込む() {
        let u = users(&["coplay-mcp", "github", "playwright"]);
        let p = plan(&u, &BTreeSet::new(), &["coplay-mcp".into()]);
        assert_eq!(p.kept, ["coplay-mcp"]);
        assert_eq!(p.denied, ["github", "playwright"]);
    }

    /// user scope を書いた `.claude.json` を一時 dir に置く
    fn claude_json(dir: &Path, servers: &[&str]) -> PathBuf {
        let path = dir.join(".claude.json");
        let map: serde_json::Map<String, Value> = servers
            .iter()
            .map(|n| (n.to_string(), srv("npx", &["-y", n])))
            .collect();
        std::fs::write(&path, json!({ "mcpServers": map }).to_string()).unwrap();
        path
    }

    #[test]
    fn prepareは止める名前を設定ファイルに書いてsettingsへ渡す() {
        let t = tempdir();
        let work = t.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let files = [claude_json(&t, &["github", "playwright", "tako"])];
        let mcp = prepare_from(&work, &files, &["playwright".into()], &[]);
        assert_eq!(mcp.mode, "tako_only");
        assert_eq!(mcp.plan.denied, ["github"]);
        assert_eq!(mcp.plan.kept, ["playwright"]);
        let path = mcp
            .settings_file
            .clone()
            .expect("止める名前があればファイルを渡す");
        assert!(
            path.contains("worker-mcp") && path.ends_with(".json"),
            "{path}"
        );
        // 書くのは materialize（prepare は置き場と中身を決めるだけ）
        mcp.materialize().unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({ "deniedMcpServers": [ { "serverName": "github" } ] })
        );
        // 同じ構成は同じファイル（worker の本数ぶん増えない）・2 回目は書き直さない
        let again = prepare_from(&work, &files, &["playwright".into()], &[]);
        assert_eq!(again.settings_file.as_deref(), Some(path.as_str()));
        again.materialize().unwrap();
    }

    #[test]
    fn 止めるものが無ければsettingsを付けない() {
        let t = tempdir();
        let files = [claude_json(&t, &["tako"])];
        let mcp = prepare_from(&t, &files, &[], &[]);
        assert_eq!(mcp.mode, "tako_only");
        assert!(mcp.settings_file.is_none() && mcp.plan.denied.is_empty());
        // .claude.json が無い（初めての機）も同じ
        let none = prepare_from(&t, &[t.join("missing.json")], &[], &[]);
        assert!(none.settings_file.is_none());
    }

    #[test]
    fn 利用者の_args_に_settings_があれば効かないと知らせる() {
        let t = tempdir();
        let files = [claude_json(&t, &["github"])];
        for arg in ["--settings", "--settings=/x.json"] {
            let mcp = prepare_from(&t, &files, &[], &[arg.to_string()]);
            assert_eq!(mcp.mode, "overridden", "{arg}");
            assert!(
                mcp.settings_file.is_none(),
                "利用者の --settings を消して上書きしない"
            );
            assert!(
                mcp.warnings.iter().any(|w| w.contains("--settings")),
                "{:?}",
                mcp.warnings
            );
        }
    }

    #[test]
    fn 存在しない名前を足すと警告が出る() {
        let t = tempdir();
        let files = [claude_json(&t, &["github"])];
        let mcp = prepare_from(&t, &files, &["coplay-mcp".into()], &[]);
        assert_eq!(mcp.plan.missing, ["coplay-mcp"]);
        assert!(mcp.warnings.iter().any(|w| w.contains("coplay-mcp")));
        assert_eq!(mcp.plan.denied, ["github"]);
    }

    #[test]
    fn mcp_jsonと同名の名前はprepareでも止めない() {
        let t = tempdir();
        let work = t.join("repo");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join(".mcp.json"), r#"{"mcpServers":{"github":{}}}"#).unwrap();
        let files = [claude_json(&t, &["github", "playwright"])];
        let mcp = prepare_from(&work, &files, &[], &[]);
        assert_eq!(mcp.plan.shadowed, ["github"]);
        assert_eq!(mcp.plan.denied, ["playwright"]);
    }

    #[test]
    fn agyは絞らない() {
        let t = tempdir();
        let mcp = prepare(
            WorkerAgent::Agy,
            &t,
            &crate::orchestrator::EMPTY_ENV_PLAN,
            &[],
            &[],
        );
        assert_eq!(mcp.mode, "not_supported");
        assert!(mcp.settings_file.is_none() && mcp.codex_off.is_empty());
    }

    // --- codex（#2418） ---

    /// codex の config.toml を一時 dir に置く
    fn codex_toml(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("codex-home").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    const CODEX_USER: &str = r#"
model = "gpt-x"
[mcp_servers.github]
command = "npx"
args = ["-y", "github"]
[mcp_servers.github.env]
K = "v"
[mcp_servers.web]
url = "http://127.0.0.1:1/mcp"
[mcp_servers.playwright]
command = "npx"
args = ["-y", "playwright"]
[mcp_servers.tako]
command = "/Applications/tako.app/Contents/MacOS/tako"
args = ["mcp", "serve"]
[projects."/x"]
trust_level = "trusted"
"#;

    #[test]
    fn codexは同じ判断でuser設定を止め足した名前とtakoは残す() {
        let t = tempdir();
        let cfg = codex_toml(&t, CODEX_USER);
        let mcp = prepare_codex_from(&t, &cfg, &["playwright".into()]);
        assert_eq!(mcp.mode, "tako_only");
        assert_eq!(mcp.plan.denied, ["github", "web"]);
        assert_eq!(mcp.plan.kept, ["playwright"]);
        assert_eq!(mcp.plan.tako, ["tako"]);
        // 目印は読んだ transport と同じ種類（url の server に command を足すと codex が起動しない）
        assert_eq!(
            mcp.codex_off,
            [
                CodexOff {
                    name: "github".into(),
                    http: false
                },
                CodexOff {
                    name: "web".into(),
                    http: true
                },
            ]
        );
        assert!(mcp.settings_file.is_none() && mcp.warnings.is_empty());
        assert_eq!(
            mcp.to_json()["config_file"],
            json!(cfg.display().to_string())
        );
    }

    #[test]
    fn codexのインライン表と根の点つきキーも読む() {
        let t = tempdir();
        // `[mcp_servers]` の下のインライン表（codex は実測で読む）
        let cfg = codex_toml(
            &t,
            "[mcp_servers]\ninline = { command = \"y\", args = [] }\nhttp = { url = \"http://h/mcp\" }\n",
        );
        let mcp = prepare_codex_from(&t, &cfg, &[]);
        assert_eq!(mcp.plan.denied, ["http", "inline"]);
        assert!(mcp.codex_off.iter().any(|o| o.name == "http" && o.http));
        assert!(mcp.codex_off.iter().any(|o| o.name == "inline" && !o.http));
        // 根の点つきキー（同じファイルで `[mcp_servers]` と混ぜると TOML として二重定義）
        let cfg = codex_toml(
            &t,
            "model = \"m\"\nmcp_servers.dotted.command = \"x\"\nmcp_servers.web.url = \"http://h\"\n",
        );
        let mcp = prepare_codex_from(&t, &cfg, &[]);
        assert_eq!(mcp.plan.denied, ["dotted", "web"]);
    }

    #[test]
    fn codexのプロジェクト設定と同名は止めず_git_の根まで探す() {
        let t = tempdir();
        let cfg = codex_toml(&t, CODEX_USER);
        let repo = t.join("repo");
        let sub = repo.join("sub");
        std::fs::create_dir_all(sub.join(".git-not")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".codex")).unwrap();
        std::fs::write(
            repo.join(".codex/config.toml"),
            "[mcp_servers.github]\ncommand = \"proj\"\n",
        )
        .unwrap();
        // cwd が git の根の下 → 根の .codex を読む
        let mcp = prepare_codex_from(&sub, &cfg, &[]);
        assert_eq!(mcp.plan.shadowed, ["github"]);
        assert_eq!(mcp.plan.denied, ["playwright", "web"]);
        // git が無ければ cwd の .codex だけ（祖先は読まない = codex と同じ）
        std::fs::remove_dir_all(repo.join(".git")).unwrap();
        let mcp = prepare_codex_from(&sub, &cfg, &[]);
        assert!(mcp.plan.shadowed.is_empty(), "{:?}", mcp.plan.shadowed);
        assert_eq!(mcp.plan.denied, ["github", "playwright", "web"]);
        let mcp = prepare_codex_from(&repo, &cfg, &[]);
        assert_eq!(mcp.plan.shadowed, ["github"]);
    }

    #[test]
    fn codexのuser設定そのものはプロジェクト側と数えない() {
        // cwd がホーム = `<cwd>/.codex/config.toml` が user 設定そのもの
        let t = tempdir();
        let home = t.join("home");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let cfg = home.join(".codex/config.toml");
        std::fs::write(&cfg, CODEX_USER).unwrap();
        let mcp = prepare_codex_from(&home, &cfg, &[]);
        assert!(mcp.plan.shadowed.is_empty(), "{:?}", mcp.plan.shadowed);
        assert_eq!(mcp.plan.denied, ["github", "playwright", "web"]);
    }

    #[test]
    fn codexの設定が無ければ何も足さず壊れていれば警告する() {
        let t = tempdir();
        let none = prepare_codex_from(&t, &t.join("nope/config.toml"), &[]);
        assert_eq!(none.mode, "tako_only");
        assert!(none.codex_off.is_empty() && none.warnings.is_empty());
        let cfg = codex_toml(&t, "[mcp_servers.a\ncommand = ");
        let broken = prepare_codex_from(&t, &cfg, &[]);
        assert_eq!(broken.mode, "unrestricted");
        assert!(broken.codex_off.is_empty());
        assert!(
            broken.warnings.iter().any(|w| w.contains("TOML")),
            "{:?}",
            broken.warnings
        );
    }

    #[test]
    fn codexの_c_で表せない名前は止めずに警告する() {
        let t = tempdir();
        let cfg = codex_toml(
            &t,
            "[mcp_servers.\"my.server\"]\ncommand = \"x\"\n[mcp_servers.\"a b\"]\ncommand = \"x\"\n[mcp_servers.\"名前\"]\ncommand = \"x\"\n[mcp_servers.\"@scope/x:1\"]\ncommand = \"x\"\n",
        );
        let mcp = prepare_codex_from(&t, &cfg, &[]);
        // `codex mcp add` が許す `:` `@` `/` と Unicode の英数字は通る（実測）
        assert_eq!(mcp.plan.denied, ["@scope/x:1", "名前"]);
        for n in ["my.server", "a b"] {
            assert!(mcp.warnings.iter().any(|w| w.contains(n)), "{n}");
        }
    }

    #[test]
    fn codexで足した名前がどこにも無ければ警告し星は全部持ち込む() {
        let t = tempdir();
        let cfg = codex_toml(&t, CODEX_USER);
        let mcp = prepare_codex_from(&t, &cfg, &["coplay-mcp".into()]);
        assert_eq!(mcp.plan.missing, ["coplay-mcp"]);
        assert!(mcp.warnings.iter().any(|w| w.contains("coplay-mcp")));
        let all = prepare_codex_from(&t, &cfg, &["*".into()]);
        assert_eq!(all.mode, "keep_all");
        assert!(all.codex_off.is_empty());
    }

    #[test]
    fn codexの_c_の断片は止める宣言と目印の組() {
        use crate::launch_cmd::ShellDialect::{Posix, PowerShell};
        let off = [
            CodexOff {
                name: "github".into(),
                http: false,
            },
            CodexOff {
                name: "web".into(),
                http: true,
            },
        ];
        assert_eq!(
            codex_off_args(&off, Posix),
            " -c 'mcp_servers.github.enabled=false' -c 'mcp_servers.github.command=tako-disabled' \
             -c 'mcp_servers.web.enabled=false' -c 'mcp_servers.web.url=http://127.0.0.1:9/tako-disabled'"
        );
        // PowerShell も単引用符（中に " を使わない = 7.3 前の native 引数でも値が変わらない）
        let ps = codex_off_args(&off[..1], PowerShell);
        assert_eq!(
            ps,
            " -c 'mcp_servers.github.enabled=false' -c 'mcp_servers.github.command=tako-disabled'"
        );
        assert!(!ps.contains('"'));
        assert_eq!(codex_off_args(&[], Posix), "");
    }

    #[test]
    fn codex_home_は_env_計画の_export_を先に見る() {
        let plan = EnvPlan {
            exports: vec![("CODEX_HOME".into(), "/tmp/ch".into())],
            unsets: vec![],
        };
        assert_eq!(codex_home_for(&plan), Some(PathBuf::from("/tmp/ch")));
        // テストのプロセスでは本物の CODEX_HOME / ~/.codex を読まない（検証用のホーム）
        let fallback = codex_home_for(&crate::orchestrator::EMPTY_ENV_PLAN);
        assert_eq!(
            fallback,
            crate::orchestrator::agent_config_home().map(|h| h.join(".codex"))
        );
    }

    #[test]
    fn 足す一覧が空なら全部止める() {
        let u = users(&["github"]);
        assert_eq!(plan(&u, &BTreeSet::new(), &[]).denied, ["github"]);
        assert_eq!(
            plan(&u, &BTreeSet::new(), &["  ".into()]).denied,
            ["github"]
        );
    }

    #[test]
    fn 存在しない名前はmissingで知らせ他は変えない() {
        let u = users(&["github"]);
        let p = plan(&u, &BTreeSet::new(), &["nope".into()]);
        assert_eq!(p.missing, ["nope"]);
        assert_eq!(p.denied, ["github"]);
        assert!(p.kept.is_empty());
    }

    #[test]
    fn mcp_jsonと同名のuser_scopeは止めない() {
        let u = users(&["github", "playwright"]);
        let declared: BTreeSet<String> = ["playwright".to_string()].into();
        let p = plan(&u, &declared, &[]);
        assert_eq!(p.shadowed, ["playwright"]);
        assert_eq!(p.denied, ["github"]);
        // プロジェクト側にある名前を足す一覧に書いても missing にはしない
        let p = plan(&u, &declared, &["playwright".into()]);
        assert!(p.missing.is_empty());
    }

    #[test]
    fn アスタリスクはuser_scopeを全部持ち込む() {
        let u = users(&["github", "playwright"]);
        let p = plan(&u, &BTreeSet::new(), &["*".into()]);
        assert!(p.keep_all && p.denied.is_empty());
        assert_eq!(p.kept, ["github", "playwright"]);
    }

    #[test]
    fn settingsのjsonはdenied_mcp_serversだけ() {
        let v: Value = serde_json::from_str(&settings_json(&["a b".into(), "c".into()])).unwrap();
        assert_eq!(
            v,
            json!({ "deniedMcpServers": [ { "serverName": "a b" }, { "serverName": "c" } ] })
        );
    }

    #[test]
    fn ツール名の形とスラグはclaudeと同じ規則() {
        assert_eq!(tool_server_name("claude.ai Gmail"), "claude_ai_Gmail");
        assert_eq!(tool_server_name("coplay-mcp"), "coplay-mcp");
        assert_eq!(project_slug("/Users/x/dev/tako"), "-Users-x-dev-tako");
        assert_eq!(project_slug("/a/2.38m-s"), "-a-2-38m-s");
        // 基板 = BMP の 2 文字 → 2 つ、𠮷 = サロゲートペア → 2 つ
        assert_eq!(project_slug("/03_基板"), "-03---");
        assert_eq!(project_slug("/𠮷"), "---");
    }

    #[test]
    fn mcp_jsonは祖先までlocal_scopeは祖先の鍵で同名と数える() {
        let t = tempdir();
        let root = t.join("repo");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join(".mcp.json"), r#"{"mcpServers":{"at-root":{}}}"#).unwrap();
        std::fs::write(sub.join(".mcp.json"), r#"{"mcpServers":{"at-sub":{}}}"#).unwrap();
        let cfg = t.join(".claude.json");
        let root_key = root.display().to_string();
        std::fs::write(
            &cfg,
            json!({ "projects": { root_key: { "mcpServers": { "local-root": {} } } } }).to_string(),
        )
        .unwrap();
        let got = project_declared(&sub, &[cfg]);
        for n in ["at-root", "at-sub", "local-root"] {
            assert!(got.contains(n), "{n} が無い: {got:?}");
        }
    }

    #[test]
    fn 実績はプロジェクトの会話からuser_scopeの名前だけ数える() {
        let t = tempdir();
        let cfg = t.join("ccd");
        let cwd = "/work/proj";
        let dir = cfg.join("projects").join(project_slug(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        let call = |name: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"{name}","input":{{}}}}]}}}}"#
            )
        };
        std::fs::write(
            dir.join("a.jsonl"),
            [
                call("mcp__playwright__browser_click"),
                call("mcp__playwright__browser_navigate"),
                call("mcp__tako__tako_list_panes"),
                call("mcp__claude_ai_Gmail__search"),
                // 結果の文字列の中（エスケープされた引用符）は数えない
                r#"{"type":"user","message":{"content":"\"name\":\"mcp__github__x\""}}"#
                    .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(dir.join("b.jsonl"), call("mcp__playwright__browser_click")).unwrap();
        let names: BTreeSet<String> = ["playwright", "github", "tako"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = usage_evidence_in(
            &[("proj".into(), cwd.into())],
            std::slice::from_ref(&cfg),
            &names,
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(
            rows,
            [UsageRow {
                project: "proj".into(),
                server: "playwright".into(),
                sessions: 2,
                calls: 3,
            }]
        );
        // 窓の外（未来の時刻を起点）なら数えない
        let later = SystemTime::now() + Duration::from_secs(3600);
        assert!(
            usage_evidence_in(&[("proj".into(), cwd.into())], &[cfg], &names, later).is_empty()
        );
    }

    /// 塊（4 MB）より長い行・塊の境目をまたぐ行・末尾に改行の無い行も数える
    #[test]
    fn 実績の走査は塊の境目と長い行を取りこぼさない() {
        let t = tempdir();
        let path = t.join("long.jsonl");
        let call = r#"{"type":"tool_use","name":"mcp__playwright__browser_click"}"#;
        let filler = "x".repeat(5 << 20);
        let body = format!(
            "{{\"t\":\"{filler}\",{}}}\n{}\n{{\"t\":\"{}\"}}\n{}",
            &call[1..call.len() - 1],
            call,
            "y".repeat((4 << 20) - 40),
            call
        );
        std::fs::write(&path, body).unwrap();
        let mut out = BTreeMap::new();
        scan_transcript(&path, &mut out);
        assert_eq!(out.get("playwright"), Some(&3), "{out:?}");
    }

    #[test]
    fn 実績の写しは冪等で既存と星は触らない() {
        let mut config: ProjectsConfig = serde_yaml::from_str(
            "projects:\n  a:\n    cwd: /a\n  b:\n    cwd: /b\n    mcp_servers: ['*']\n",
        )
        .unwrap();
        let rows = [
            UsageRow {
                project: "a".into(),
                server: "playwright".into(),
                sessions: 1,
                calls: 1,
            },
            UsageRow {
                project: "b".into(),
                server: "github".into(),
                sessions: 1,
                calls: 1,
            },
            UsageRow {
                project: "zz".into(),
                server: "github".into(),
                sessions: 1,
                calls: 1,
            },
        ];
        assert_eq!(
            adopt_usage(&mut config, &rows),
            [("a".to_string(), "playwright".to_string())]
        );
        assert!(adopt_usage(&mut config, &rows).is_empty());
        assert_eq!(config.projects["a"].mcp_servers, ["playwright"]);
        assert_eq!(config.projects["b"].mcp_servers, ["*"]);
    }

    /// テスト用の一時ディレクトリ（drop で削除。remote_auth のテストと同じ形）
    struct TempDir(PathBuf);
    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let d = std::env::temp_dir().join(format!(
            "tako-2412-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }
}

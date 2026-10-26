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
//! codex / agy の扱いは能力マトリクス（`worker_mcp_scope`）に宣言してある。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::{json, Value};

use crate::orchestrator::agent::WorkerAgent;
use crate::orchestrator::ProjectsConfig;

/// `mcp_servers` に書くと user scope を**全部持ち込む**（絞らない）印
pub const KEEP_ALL: &str = "*";

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
    let canonical = std::fs::canonicalize(cwd).ok();
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
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.components().collect());
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
    /// 応答の warnings と persist.log へ出す文
    pub warnings: Vec<String>,
}

impl WorkerMcp {
    /// 応答の `mcp` 欄
    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode,
            "denied": self.plan.denied,
            "kept": self.plan.kept,
            "missing": self.plan.missing,
            "shadowed": self.plan.shadowed,
            "settings_file": self.settings_file,
        })
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

/// worker の MCP の構成を決める（ファイルは書かない。書くのは [`WorkerMcp::materialize`]）。
///
/// `extra_args` はプロファイルの `worker_agents.claude.args`（`--settings` の重複の検査に使う）
pub fn prepare(
    agent: WorkerAgent,
    cwd: &Path,
    explicit_config_dir: Option<&str>,
    keep: &[String],
    extra_args: &[String],
) -> WorkerMcp {
    // codex / agy は能力マトリクス（worker_mcp_scope）のとおり今は絞らない
    if agent != WorkerAgent::Claude {
        return WorkerMcp {
            mode: "not_supported",
            ..Default::default()
        };
    }
    if legacy() {
        return WorkerMcp {
            mode: "legacy",
            ..Default::default()
        };
    }
    prepare_from(
        cwd,
        &user_scope_files(explicit_config_dir),
        keep,
        extra_args,
    )
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
    fn claude以外は絞らない() {
        let t = tempdir();
        for agent in [WorkerAgent::Codex, WorkerAgent::Agy] {
            let mcp = prepare(agent, &t, None, &[], &[]);
            assert_eq!(mcp.mode, "not_supported");
            assert!(mcp.settings_file.is_none());
        }
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
            &[cfg.clone()],
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

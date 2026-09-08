//! MCP の引数到達性の番犬（Issue #1185）。
//!
//! `validate_known_params` は catalog の `inputSchema.properties` を許可リストとして
//! 使い、そこに無いキーを `additionalProperties: false` として弾く。つまり
//! **mapper（`mcp/request.rs`）が読むのに catalog に無い引数は、どの MCP
//! クライアントからも渡せない = プロトコルにあるのに到達不能**になる。
//!
//! #1185 の `tako_tmux_open` の `window` がまさにこれで、
//! 「protocol にある / CLI にも無い / MCP は未知パラメータとして拒否」で
//! 機能が 3 経路すべてから消えていた。開発不変条件「UI でできることは
//! CLI / MCP からもできる」を人間の記憶ではなくテストで守る。
//!
//! 判定は mapper のソース走査（`args, "<名前>"` と `pane` / `direction` の
//! 専用ヘルパ）。**catalog を直さずに mapper へ引数を足すと落ちる**。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tako_control::mcp;

/// 意図して catalog に出していない引数（理由つき）。
/// **増やすときは「なぜ MCP クライアントから渡せなくて良いか」を書く**。
const INTENTIONAL_GAPS: &[(&str, &str, &str)] = &[
    (
        "tako_orchestrator_self",
        "caller_pid",
        "CLI 専用（呼び出し元プロセスの pid）。MCP クライアントの pid は無意味なので公開しない",
    ),
    (
        "tako_orchestrator_handoff",
        "caller_pid",
        "同上（CLI が std::process::id() を入れる欄）",
    ),
    (
        "tako_orchestrator_spawn",
        "caller_pid",
        "同上（CLI が std::process::id() を入れる欄）",
    ),
    (
        "tako_equalize_layout",
        "pane",
        "公開契約は tab 指定。pane は tab 省略時に呼び出し元へ落とすフォールバック専用",
    ),
    (
        "tako_rename_tab",
        "pane",
        "同上（tab 省略時に呼び出し元のタブを解決するだけ）",
    ),
    (
        "tako_pin_tab_title",
        "pane",
        "同上（tab 省略時に呼び出し元のタブを解決するだけ）",
    ),
];

fn mapper_source() -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp/request.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("mapper を読めない {}: {e}", path.display()))
}

/// mapper のソースから「ツール名 → 読んでいる引数名」を採る。
/// match の腕（`"tako_x" => ` / `"tako_x" | "tako_y" => `）で対象を切り替え、
/// `_ =>`（既定腕）で走査を終える（腕の外にあるヘルパ定義を拾わないため）
fn args_read_by_tool(source: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut reads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Vec<String> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("_ =>") {
            current.clear();
            continue;
        }
        if let Some(names) = arm_tool_names(trimmed) {
            current = names;
            for name in &current {
                reads.entry(name.clone()).or_default();
            }
        }
        if current.is_empty() {
            continue;
        }
        let mut found: Vec<String> = quoted_arg_names(line);
        // `pane` / `direction` は専用ヘルパ経由で読まれる
        if line.contains("target_pane(args") {
            found.push("pane".into());
        }
        if line.contains("direction_arg(args") {
            found.push("direction".into());
        }
        for name in found {
            for tool in &current {
                reads.entry(tool.clone()).or_default().insert(name.clone());
            }
        }
    }
    reads
}

/// `"tako_x" => ` / `"tako_x" | "tako_y" => ` ならツール名の一覧
fn arm_tool_names(trimmed: &str) -> Option<Vec<String>> {
    if !trimmed.starts_with("\"tako_") {
        return None;
    }
    let head = trimmed.split("=>").next()?;
    if !trimmed.contains("=>") {
        return None;
    }
    let names: Vec<String> = head
        .split('|')
        .map(|part| part.trim().trim_matches('"').to_string())
        .filter(|part| part.starts_with("tako_"))
        .collect();
    (!names.is_empty()).then_some(names)
}

/// `args, "name"` の `name` を拾う（`str_arg` / `u64_arg` / `bool_arg` … 共通）
fn quoted_arg_names(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = line;
    while let Some(pos) = rest.find("args, \"") {
        rest = &rest[pos + "args, \"".len()..];
        if let Some(end) = rest.find('"') {
            names.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    names
}

/// catalog の `inputSchema.properties` のキー（= 受け付ける引数の正）
fn schema_params() -> BTreeMap<String, BTreeSet<String>> {
    mcp::tools()
        .iter()
        .filter_map(|tool| {
            let name = tool["name"].as_str()?.to_string();
            let keys = tool["inputSchema"]["properties"]
                .as_object()
                .map(|props| props.keys().cloned().collect())
                .unwrap_or_default();
            Some((name, keys))
        })
        .collect()
}

#[test]
fn mapperが読む引数はすべてcatalogから渡せる() {
    let reads = args_read_by_tool(&mapper_source());
    assert!(
        reads.len() > 100,
        "mapper の走査が壊れている（腕を {} 個しか見つけられていない）",
        reads.len()
    );
    let schema = schema_params();
    let allowed: BTreeSet<(&str, &str)> = INTENTIONAL_GAPS
        .iter()
        .map(|(tool, param, _)| (*tool, *param))
        .collect();

    let mut unreachable: Vec<String> = Vec::new();
    for (tool, params) in &reads {
        let Some(known) = schema.get(tool) else {
            continue; // special_handler 側のツール（カタログに無い）は対象外
        };
        for param in params {
            if known.contains(param) || allowed.contains(&(tool.as_str(), param.as_str())) {
                continue;
            }
            unreachable.push(format!("{tool}.{param}"));
        }
    }
    assert!(
        unreachable.is_empty(),
        "mapper が読むのに catalog の inputSchema に無い引数がある\
         （additionalProperties: false なのでどの MCP クライアントからも渡せない）: {unreachable:?}。\
         catalog へ properties を足すか、意図した欠落なら INTENTIONAL_GAPS へ理由つきで載せる"
    );
}

/// 逆向き: `INTENTIONAL_GAPS` が古くなって（catalog へ公開済み / mapper が読まなく
/// なった）実態と合わなくなったら落とす。番犬の許可リストが腐るのを防ぐ
#[test]
fn 意図した欠落の表が実態と合っている() {
    let reads = args_read_by_tool(&mapper_source());
    let schema = schema_params();
    let mut stale: Vec<String> = Vec::new();
    for (tool, param, _) in INTENTIONAL_GAPS {
        let read = reads
            .get(*tool)
            .is_some_and(|params| params.contains(*param));
        let published = schema
            .get(*tool)
            .is_some_and(|params| params.contains(*param));
        if !read {
            stale.push(format!("{tool}.{param}（mapper がもう読んでいない）"));
        } else if published {
            stale.push(format!("{tool}.{param}（catalog へ公開済み）"));
        }
    }
    assert!(
        stale.is_empty(),
        "INTENTIONAL_GAPS が実態と合っていない: {stale:?}"
    );
}

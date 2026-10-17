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

// ---------------------------------------------------------------------------
// 並べた enum の値の説明から漏れた値（Issue #1905）
// ---------------------------------------------------------------------------
//
// `tako_config_share` の説明文は `action: status = … / init = … / link = … / push = … /
// pull = … / list = …` と値ごとの意味を並べていたのに、enum にある `unlink` だけが無かった。
// AI は並べた説明を「これで全部」と読むので、**説明から漏れた値は使いどころの分からない値**に
// なる（引数が渡せない #1185 の手前の、渡せるのに選べない形）。
//
// enum の値は inputSchema にもあるので、全部を説明へ写せとは言わない（方向の
// right / down / left / up のような自明な語を写すのは固定費を増やすだけ）。縛るのは
// **値の意味を `値 = 意味` / `値: 意味` の形で 2 つ以上並べたなら、残りの値も説明のどこかに
// 出す**こと。見る説明はその引数・配列の要素・親の引数・ツールの description。
// `extend_*` のような接頭辞 + `*` は、その接頭辞で始まる値を挙げたものと数える。

/// enum の値に使われる文字
fn is_value_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// `text` の中で `value` が 1 語として現れる位置（`status` は `status_source` の中に数えない）
fn word_positions(text: &str, value: &str) -> Vec<usize> {
    text.match_indices(value)
        .filter_map(|(at, _)| {
            let before = text[..at].chars().next_back();
            let rest = &text[at + value.len()..];
            let mut after = rest.chars();
            let right_ok = match after.next() {
                None => true,
                // 文末の `.` は語の外（`band.hidden` の `band` は語の途中）
                Some('.') => after.next().is_none_or(|c| !c.is_ascii_alphanumeric()),
                Some(c) => !is_value_char(c),
            };
            (before.is_none_or(|c| !is_value_char(c)) && right_ok).then_some(at)
        })
        .collect()
}

/// `値 = 意味` / `値（補足）= 意味` / `値: 意味` の形で書かれているか
fn is_defined(text: &str, value: &str) -> bool {
    word_positions(text, value).into_iter().any(|at| {
        let mut rest = &text[at + value.len()..];
        if let Some(inner) = rest.strip_prefix('（') {
            rest = inner.split_once('）').map_or("", |(_, after)| after);
        }
        let rest = rest.trim_start();
        rest.starts_with('=') || rest.starts_with(':') || rest.starts_with('：')
    })
}

/// 説明のどこかに出ているか（`extend_*` は `extend_` で始まる値を挙げたものと数える）
fn is_mentioned(text: &str, value: &str) -> bool {
    if !word_positions(text, value).is_empty() {
        return true;
    }
    text.match_indices('*').any(|(at, _)| {
        let prefix: String = text[..at]
            .chars()
            .rev()
            .take_while(|c| is_value_char(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        (prefix.ends_with('_') || prefix.ends_with('-')) && value.starts_with(&prefix)
    })
}

/// 文字列 enum を持つ引数 1 つ（入れ子の object の引数は `親.子`）
struct EnumSite {
    tool: String,
    path: String,
    values: Vec<String>,
    /// その引数・配列の要素・親の引数・ツールの description をつないだもの
    text: String,
}

fn description_of(schema: &serde_json::Value) -> &str {
    schema["description"].as_str().unwrap_or("")
}

fn collect_enum_sites(
    tool: &str,
    schema: &serde_json::Value,
    path: &str,
    context: &str,
    out: &mut Vec<EnumSite>,
) {
    let Some(props) = schema["properties"].as_object() else {
        return;
    };
    for (key, prop) in props {
        let path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let items = &prop["items"];
        let text = format!(
            "{} {} {context}",
            description_of(prop),
            description_of(items)
        );
        let values: Vec<String> = prop["enum"]
            .as_array()
            .or_else(|| items["enum"].as_array())
            .map(|vs| {
                vs.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !values.is_empty() {
            out.push(EnumSite {
                tool: tool.to_string(),
                path: path.clone(),
                values,
                text: text.clone(),
            });
        }
        collect_enum_sites(tool, prop, &path, &text, out);
        collect_enum_sites(tool, items, &path, &text, out);
    }
}

fn enum_sites(tools: &[serde_json::Value]) -> Vec<EnumSite> {
    let mut out = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("?");
        collect_enum_sites(
            name,
            &tool["inputSchema"],
            "",
            description_of(tool),
            &mut out,
        );
    }
    out
}

/// 値の意味を 2 つ以上並べたのに、説明に出てこない値がある引数を
/// `tool.path: 欠けた値（並べた値）` で返す
fn undescribed_enum_values(tools: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    for site in enum_sites(tools) {
        let defined: Vec<&str> = site
            .values
            .iter()
            .filter(|v| is_defined(&site.text, v))
            .map(String::as_str)
            .collect();
        if defined.len() < 2 {
            continue;
        }
        let missing: Vec<&str> = site
            .values
            .iter()
            .filter(|v| !is_mentioned(&site.text, v))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            out.push(format!(
                "{}.{}: {} が説明に無い（意味を並べた値: {}）",
                site.tool,
                site.path,
                missing.join(" / "),
                defined.join(" / ")
            ));
        }
    }
    out
}

/// catalog.rs のそのツールの `"name"` の行（落ちたときに直す先を名指す）
fn catalog_line(tool: &str) -> usize {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp/catalog.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("catalog.rs を読めない {}: {e}", path.display()));
    let needle = format!("\"name\": \"{tool}\"");
    src.lines()
        .position(|l| l.contains(&needle))
        .map_or(0, |i| i + 1)
}

#[test]
fn 意味を並べたenumの値はすべて説明に出る() {
    let tools = mcp::tools();
    let sites = enum_sites(&tools);
    // 走査が空振りすれば無意味に緑になる（文字列 enum は 100 か所を超える）
    assert!(
        sites.len() > 100,
        "enum を {} 個しか見つけられていない",
        sites.len()
    );
    let problems: Vec<String> = undescribed_enum_values(&tools)
        .into_iter()
        .map(|p| {
            let tool = p.split('.').next().unwrap_or("");
            format!(
                "  crates/tako-control/src/mcp/catalog.rs:{}: {p}",
                catalog_line(tool)
            )
        })
        .collect();
    assert!(
        problems.is_empty(),
        "値ごとの意味を並べた説明から漏れた enum の値がある。AI は並べた説明を全部と読むので、\
         漏れた値の意味も説明へ足す（Issue #1905）:\n{}",
        problems.join("\n")
    );
}

#[test]
fn 説明から漏れたenumの値を名指せる() {
    let tool = |description: &str, props: serde_json::Value| {
        serde_json::json!({
            "name": "tako_x",
            "description": description,
            "inputSchema": { "type": "object", "properties": props },
        })
    };
    let action = |values: &[&str], description: &str| serde_json::json!({ "action": { "type": "string", "enum": values, "description": description } });
    let cases = [
        // 意味を並べた中から 1 つ漏れた（config_share の unlink の形）
        (
            tool(
                "action: status（既定）= 状態 / init = 作る / push = 送る",
                action(&["status", "init", "unlink", "push"], ""),
            ),
            vec!["tako_x.action: unlink が説明に無い（意味を並べた値: status / init / push）"],
        ),
        // 引数の説明に並べた形も同じ
        (
            tool("", action(&["a", "b", "c"], "a = あ / b: い")),
            vec!["tako_x.action: c が説明に無い（意味を並べた値: a / b）"],
        ),
        // 並べていない（自明な語）・1 つだけ（既定値の案内）は縛らない
        (
            tool("", action(&["right", "down", "left", "up"], "方向")),
            vec![],
        ),
        (
            tool("", action(&["right", "down"], "（省略時は right）")),
            vec![],
        ),
        (tool("", action(&["on", "off"], "on = 有効")), vec![]),
        // 語の途中は出たことにならない（`status_source` の中の `status`）
        (
            tool(
                "list = 一覧 / show = 1 件。status_source を返す",
                action(&["list", "show", "status"], ""),
            ),
            vec!["tako_x.action: status が説明に無い（意味を並べた値: list / show）"],
        ),
        // 接頭辞 + `*` は挙げたものと数える
        (
            tool(
                "",
                action(
                    &["up", "enter", "extend_up", "extend_down"],
                    "up = 上 / enter = 開く / extend_* = 範囲",
                ),
            ),
            vec![],
        ),
        // 入れ子の object の引数も見る
        (
            tool(
                "",
                serde_json::json!({
                    "sleep": { "type": "object", "properties": {
                        "mode": { "type": "string", "enum": ["off", "on", "auto"], "description": "off = 無効 / on = 常時" },
                    } },
                }),
            ),
            vec!["tako_x.sleep.mode: auto が説明に無い（意味を並べた値: off / on）"],
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            undescribed_enum_values(std::slice::from_ref(&input)),
            expected,
            "{input}"
        );
    }

    // 実物への注入: config_share の説明から unlink を抜くと、その引数を名指す
    let mut tools = mcp::tools();
    let share = tools
        .iter_mut()
        .find(|t| t["name"] == "tako_config_share")
        .expect("tako_config_share がカタログに無い");
    let description = share["description"].as_str().unwrap_or("").to_string();
    assert!(
        word_positions(&description, "unlink").len() == 1,
        "注入の前提が崩れた（unlink が説明にちょうど 1 回）: {description}"
    );
    let at = word_positions(&description, "unlink")[0];
    let injected = format!(
        "{}{}",
        &description[..at],
        &description[at + "unlink".len()..]
    );
    share["description"] = serde_json::Value::String(injected);
    let found = undescribed_enum_values(&tools);
    assert!(
        found
            .iter()
            .any(|p| p.starts_with("tako_config_share.action: unlink が説明に無い")),
        "unlink を抜いた注入を名指せない: {found:?}"
    );
}

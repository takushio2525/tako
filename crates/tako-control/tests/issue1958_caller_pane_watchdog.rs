//! MCP カタログの「省略時は呼び出し元」を変換へ縛る番犬（Issue #1958）
//!
//! # なぜ要るか
//!
//! `tako_show_command` の `pane` は「省略時は呼び出し元 = 自分の会話ペイン」と説明していたのに、
//! 変換（`mcp/request.rs`）が呼び出し元（stdio: `TAKO_PANE_ID` / HTTP: `X-Tako-Pane`）で
//! 埋めておらず、pane を省いた show は dispatch で「対象ペインが未指定」と断られていた。
//! `tako_sessions` の link も同じ形で、dispatch がアクティブタブのフォーカスペインへ倒れるため、
//! 裏のタブの AI が**他人の会話のリンク**を受け取っていた。
//!
//! 説明は MCP を繋いだエージェント全員が読む契約なので、食い違うと AI は初回で失敗して
//! 読み直す（かカードを諦める）。説明と変換は別ファイルにあり、片方だけ書いても
//! 型もテストも何も言わないので、目視では気づけない。
//!
//! # 何を縛るか
//!
//! 1. **約束したツールは全部埋める** — ツールの説明かいずれかの引数の説明が「省略…呼び出し元」
//!    と言うツールは、呼び出し元ペイン付きで呼ぶと、組み上がった要求の `pane` を含むキーの
//!    どれかに呼び出し元が入る（「呼び出し元ペインのタブ」も、dispatch は pane からタブを引くので同じ検査）。
//!    action が enum のツールは全 action を試す
//! 2. **約束の範囲を限る説明は事例で縛る**（[`scoped_cases`]）— 「open / show: …」のように
//!    action や引数で約束を限っているツールは、埋める側と**埋めない側**の両方を明示する
//!    （例: `card` 指定の copy は呼び出し元を混ぜない = 別ペインのカードを取り違えない）
//! 3. **「省略時はアクティブタブ」と説明するツールは呼び出し元を混ぜない**（[`active_cases`] + 自動）—
//!    呼び出し元ではなくアクティブタブ（のフォーカス中ペイン）を対象にすると言うツールは、
//!    呼び出し元付きで呼んでも要求に呼び出し元が入らない。#1958 の棚卸しで `tako_open_remote` は
//!    説明（呼び出し元）と挙動（アクティブタブ）が食い違っていた。呼び出し元 = AI 自身のペインは
//!    agent_role で SSH 化を断られるので、埋めると失敗が増えるだけ = 説明を挙動へ寄せた
//! 4. **走査が空振りしていない** — 約束を拾えたツールの数に下限を置く
//!
//! # 見逃す側へ倒れないための作り
//!
//! 要求は `mcp::handle_message` の公開の入口から組ませる（`build_request` を直に呼ばない）ので、
//! MCP ハンドラ側で合成する特別なツール（`tako_orchestrator_run`）も同じ検査を通る。

use std::collections::BTreeSet;

use serde_json::{json, Value};
use tako_control::mcp::{self, McpSession};

/// 呼び出し元ペイン（実在し得ない大きな値 = 引数の既定値や他の数値と取り違えない）
const CALLER: u64 = 987_654;

/// 約束を拾えたツール数の下限（#1958 の棚卸し時点の実数。`tako_open_remote` は説明をアクティブタブへ
/// 寄せたので数えない。減ったら走査の空振りを疑う）
const MIN_PROMISING_TOOLS: usize = 58;

/// 「省略時はアクティブタブ」を拾えたツール数の下限（同じく棚卸し時点の実数）
const MIN_ACTIVE_TOOLS: usize = 2;

/// 説明の文に「省略 … 呼び出し元」の約束があるか（[`omitted_means`]）
fn promises_caller(text: &str) -> bool {
    omitted_means(text, "呼び出し元")
}

/// 説明の文に「省略 … アクティブタブ」の約束があるか（= 呼び出し元を混ぜない約束）
fn promises_active(text: &str) -> bool {
    omitted_means(text, "アクティブタブ")
}

/// `target` の直前 8 文字以内に「省略」がある形だけを「省略時の対象」の約束と読む
/// （「相対パスは呼び出し元ペインの cwd 基準」のような、省略時の挙動でない言及を拾わない）
fn omitted_means(text: &str, target: &str) -> bool {
    text.match_indices(target).any(|(at, _)| {
        let before: String = text[..at]
            .chars()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        before.contains("省略")
    })
}

/// 呼び出し元を約束しているか（ツールの説明・各引数の説明のどこかで約束していれば true）
fn tool_promises(tool: &Value) -> bool {
    tool_says(tool, promises_caller)
}

/// アクティブタブを約束し、呼び出し元は約束していないか
fn tool_promises_active(tool: &Value) -> bool {
    tool_says(tool, promises_active) && !tool_promises(tool)
}

fn tool_says(tool: &Value, says: fn(&str) -> bool) -> bool {
    let props = tool["inputSchema"]["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    // 約束の対象（pane / tab）を持たないツールは対象外
    if !props.contains_key("pane") && !props.contains_key("tab") {
        return false;
    }
    says(tool["description"].as_str().unwrap_or(""))
        || props
            .values()
            .any(|p| says(p["description"].as_str().unwrap_or("")))
}

/// スキーマから必須引数の仮の値を作る
fn sample(schema: &Value) -> Value {
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|e| e.first())
    {
        return first.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("integer") | Some("number") => json!(1),
        Some("boolean") => json!(false),
        Some("array") => json!(["x"]),
        Some("object") => json!({}),
        _ => json!("x"),
    }
}

/// 必須引数だけを埋めた引数
fn required_args(tool: &Value) -> serde_json::Map<String, Value> {
    let schema = &tool["inputSchema"];
    let mut args = serde_json::Map::new();
    for key in schema["required"].as_array().into_iter().flatten() {
        let key = key.as_str().unwrap();
        args.insert(key.into(), sample(&schema["properties"][key]));
    }
    args
}

/// 引数 `key` の enum（無ければ指定なしの 1 通り）
fn enum_values(tool: &Value, key: &str) -> Vec<Option<String>> {
    match tool["inputSchema"]["properties"][key]["enum"].as_array() {
        Some(values) => values
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect(),
        None => vec![None],
    }
}

/// 公開の入口（`handle_message`）から呼び、dispatch へ渡る要求を JSON で返す。
/// 要求が組めなければ応答（JSON-RPC エラー）を Err で返す
fn request_for(name: &str, args: &Value) -> Result<Value, String> {
    let mut seen = Vec::new();
    let mut exec = |request: tako_control::protocol::Request| -> Result<Value, String> {
        seen.push(request);
        Ok(json!({}))
    };
    let mut session = McpSession {
        caller_pane: Some(CALLER),
        caller_role: None,
        connected: true,
        exec: &mut exec,
    };
    let message = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args },
    });
    let response = mcp::handle_message(&message, &mut session);
    match seen.first() {
        Some(request) => Ok(serde_json::to_value(request).unwrap()),
        None => Err(format!("{response:?}")),
    }
}

/// 要求のどこかの `pane` を含むキーに呼び出し元が入っているか
fn carries_caller(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, v)| (key.contains("pane") && v == &json!(CALLER)) || carries_caller(v)),
        Value::Array(items) => items.iter().any(carries_caller),
        _ => false,
    }
}

/// 約束の範囲を action・引数で限っているツールの事例（自動の事例の代わりに使う）
struct Case {
    tool: &'static str,
    args: Value,
    /// 呼び出し元で埋めるべきか
    fills: bool,
    why: &'static str,
}

/// 操作の種別（`key` の enum）ごとに「埋める / 埋めない」を並べる
/// （enum から生成 = 値を足したら必ず判定に乗る）
fn by_kind(
    tools: &[Value],
    tool: &'static str,
    key: &str,
    extra: Value,
    fills: impl Fn(&str) -> bool,
    why: &'static str,
) -> Vec<Case> {
    let def = tools
        .iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} がカタログに無い（事例表が古い）"));
    enum_values(def, key)
        .into_iter()
        .map(|action| {
            let action = action.unwrap_or_else(|| panic!("{tool} の {key} に enum が無い"));
            let mut args = required_args(def);
            args.insert(key.into(), json!(action));
            if let Value::Object(extra) = &extra {
                args.extend(extra.clone());
            }
            Case {
                tool,
                args: Value::Object(args),
                fills: fills(&action),
                why,
            }
        })
        .collect()
}

fn scoped_cases(tools: &[Value]) -> Vec<Case> {
    let mut cases = Vec::new();
    // #1958: card 省略は呼び出し元。card 指定は dispatch がカードの所在ペインを引くので埋めない
    cases.extend(by_kind(
        tools,
        "tako_show_command",
        "action",
        json!({}),
        |_| true,
        "pane 省略は呼び出し元 = 自分の会話ペイン（#1958）",
    ));
    for action in ["list", "copy", "run", "dismiss"] {
        cases.push(Case {
            tool: "tako_show_command",
            args: json!({ "action": action, "card": 5 }),
            fills: false,
            why:
                "card 指定はカードの所在ペインを dispatch が引く（呼び出し元を混ぜると取り違える）",
        });
    }
    // 説明: 「resume の分割元 / link の対象ペイン ID（省略時は呼び出し元）」
    cases.extend(by_kind(
        tools,
        "tako_sessions",
        "action",
        json!({}),
        |a| matches!(a, "resume" | "link"),
        "約束は resume / link だけ（list / show はペインを取らない）",
    ));
    cases.push(Case {
        tool: "tako_sessions",
        args: json!({ "action": "link", "id": "abc" }),
        fills: false,
        why: "id で引く link はペインを使わない",
    });
    // 説明: 「open / show: 分割の基準ペイン ID（省略時は呼び出し元）。その他: 対象 Web ビューが表示中のペイン ID」
    cases.extend(by_kind(
        tools,
        "tako_web",
        "action",
        json!({ "url": "https://example.com" }),
        |a| matches!(a, "open" | "show"),
        "約束は open / show だけ（他は対象を名指しする = 呼び出し元を混ぜると AI 自身のペインを対象と誤解する）",
    ));
    // 説明: 「diagnostics は省略で全文書、それ以外は省略で呼び出し元」。format-on-save は設定でペインを取らない
    cases.extend(by_kind(
        tools,
        "tako_lsp",
        "action",
        // format は範囲を 4 つそろえないと断る（#1683）ので、位置の系統と同時に満たす値
        json!({ "line": 1, "column": 1, "end_line": 1, "end_column": 2 }),
        |a| !matches!(a, "diagnostics" | "format-on-save"),
        "diagnostics は全文書・format-on-save は設定（どちらもペインを取らない）",
    ));
    // 説明: 「open_terminal の cd 先 / copy_relative_path の基準 / open_in_tako の分割元。省略時は呼び出し元」
    cases.extend(by_kind(
        tools,
        "tako_file_op",
        "op",
        json!({ "path": "/tmp/x" }),
        |op| matches!(op, "open_terminal" | "copy_relative_path" | "open_in_tako"),
        "約束は open_terminal / copy_relative_path / open_in_tako だけ（他の op はペインを取らない）",
    ));
    // tab の説明「省略時は呼び出し元ペインのタブ」は selection 以外（ツリーはアクティブタブのものだけ = #1908）
    cases.extend(by_kind(
        tools,
        "tako_tree_folder",
        "action",
        json!({}),
        |a| a != "selection",
        "selection はアクティブタブのツリーだけを扱う（説明どおり。dispatch が他タブを断る）",
    ));
    cases
}

/// 「省略時はアクティブタブ」のツールで、開き方ごとに呼び出し元を混ぜないことを並べる事例
fn active_cases(tools: &[Value]) -> Vec<Case> {
    // 説明: 「省略時はアクティブタブのフォーカス中ペイン」（#1958 で挙動へ寄せた）。
    // 呼び出し元 = AI 自身のペインは agent_role で SSH 化を断られるので、埋めると失敗が増えるだけ
    by_kind(
        tools,
        "tako_open_remote",
        "target",
        json!({}),
        |_| false,
        "省略時はアクティブタブのフォーカス中ペイン（呼び出し元 = AI 自身のペインは SSH 化できない）",
    )
}

/// ツールの約束と要求の食い違い（名指しの文）
fn violation(tool: &str, args: &Value, want_fill: bool, got: &Result<Value, String>) -> String {
    let what = if want_fill {
        "説明は省略時に呼び出し元と約束しているのに、要求の pane に呼び出し元が入らない"
    } else {
        "呼び出し元を混ぜない約束なのに、要求の pane に呼び出し元が入った"
    };
    format!("{tool} {args}: {what}（要求: {got:?}）")
}

#[test]
fn 約束したツールは呼び出し元で埋める() {
    let tools = mcp::tools();
    let scoped = scoped_cases(&tools);
    let scoped_tools: BTreeSet<&str> = scoped.iter().map(|c| c.tool).collect();
    let mut problems = Vec::new();
    let mut promising = 0;

    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        if !tool_promises(tool) {
            continue;
        }
        promising += 1;
        if scoped_tools.contains(name) {
            continue;
        }
        for action in enum_values(tool, "action") {
            let mut args = required_args(tool);
            if let Some(a) = &action {
                args.insert("action".into(), json!(a));
            }
            let args = Value::Object(args);
            let got = request_for(name, &args);
            match &got {
                Ok(request) if carries_caller(request) => {}
                // 必須引数の仮の値では組めないツール = scoped_cases に正しい引数で事例を書く
                Err(e) => problems.push(format!(
                    "{name} {args}: 仮の引数で要求が組めない。scoped_cases に正しい引数の事例を足す（{e}）"
                )),
                Ok(_) => problems.push(violation(name, &args, true, &got)),
            }
        }
    }
    for case in &scoped {
        let got = request_for(case.tool, &case.args);
        let filled = matches!(&got, Ok(r) if carries_caller(r));
        if got.is_err() || filled != case.fills {
            problems.push(format!(
                "{}（{}）",
                violation(case.tool, &case.args, case.fills, &got),
                case.why
            ));
        }
    }

    assert!(
        promising >= MIN_PROMISING_TOOLS,
        "約束を拾えたツールが {promising} 本しかない（下限 {MIN_PROMISING_TOOLS}）。\
         説明の言い回しが変わって promises_caller が空振りしていないか"
    );
    assert!(
        problems.is_empty(),
        "カタログの「省略時は呼び出し元」と変換が食い違っている（{} 件）:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn アクティブタブを約束したツールは呼び出し元を混ぜない() {
    let tools = mcp::tools();
    let cases = active_cases(&tools);
    let case_tools: BTreeSet<&str> = cases.iter().map(|c| c.tool).collect();
    let mut problems = Vec::new();
    let mut promising = 0;

    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        if !tool_promises_active(tool) {
            continue;
        }
        promising += 1;
        if case_tools.contains(name) {
            continue;
        }
        for action in enum_values(tool, "action") {
            let mut args = required_args(tool);
            if let Some(a) = &action {
                args.insert("action".into(), json!(a));
            }
            let args = Value::Object(args);
            let got = request_for(name, &args);
            match &got {
                Ok(request) if !carries_caller(request) => {}
                Err(e) => problems.push(format!(
                    "{name} {args}: 仮の引数で要求が組めない。active_cases に正しい引数の事例を足す（{e}）"
                )),
                Ok(_) => problems.push(violation(name, &args, false, &got)),
            }
        }
    }
    for case in &cases {
        let got = request_for(case.tool, &case.args);
        let filled = matches!(&got, Ok(r) if carries_caller(r));
        if got.is_err() || filled != case.fills {
            problems.push(format!(
                "{}（{}）",
                violation(case.tool, &case.args, case.fills, &got),
                case.why
            ));
        }
    }

    assert!(
        promising >= MIN_ACTIVE_TOOLS,
        "「省略時はアクティブタブ」を拾えたツールが {promising} 本しかない（下限 {MIN_ACTIVE_TOOLS}）"
    );
    assert!(
        problems.is_empty(),
        "カタログの「省略時はアクティブタブ」と変換が食い違っている（{} 件）:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn 事例表は約束したツールだけを指す() {
    let tools = mcp::tools();
    let find = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} がカタログに無い（表が古い）"))
            .clone()
    };
    for case in scoped_cases(&tools) {
        assert!(
            tool_promises(&find(case.tool)),
            "{} はもう呼び出し元を約束していない（scoped_cases から外す）",
            case.tool
        );
    }
    for case in active_cases(&tools) {
        assert!(
            tool_promises_active(&find(case.tool)),
            "{} はもう「省略時はアクティブタブ」と言っていない（active_cases から外す）",
            case.tool
        );
    }
}

#[test]
fn 約束の読み取りは省略時の言及だけを拾う() {
    assert!(promises_caller("対象ペイン ID（省略時は呼び出し元）"));
    assert!(promises_caller(
        "対象タブ ID（省略時は呼び出し元ペインのタブ）"
    ));
    assert!(promises_caller(
        "diagnostics は省略で全文書、それ以外は省略で呼び出し元"
    ));
    assert!(promises_caller(
        "対象ペイン（**省略すると呼び出し元 = 自分のペイン**。"
    ));
    assert!(promises_caller("飛ぶ（pane の省略は呼び出し元）"));
    assert!(!promises_caller(
        "実行対象ファイルパス（相対パスは呼び出し元ペインの cwd 基準）"
    ));
    assert!(!promises_caller(
        "profile_source=pane_role は呼び出し元の TAKO_ORCHESTRATOR_ROLE が失われ"
    ));
    assert!(promises_active(
        "target=split は分割元。省略時はアクティブタブのフォーカス中ペイン）"
    ));
    assert!(promises_active(
        "selection はアクティブタブのツリーだけを扱う = 省略時もアクティブタブ）"
    ));
    assert!(!promises_active(
        "由来タブへ戻す（由来タブが閉じていればアクティブタブ）"
    ));
}

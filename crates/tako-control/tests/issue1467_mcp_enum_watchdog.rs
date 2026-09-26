//! MCP カタログの文字列 enum を**正本へ縛る**番犬（Issue #1467）
//!
//! # なぜ要るか
//!
//! `tako_panel` の `view` は、正本（`PanelViewWire`）へ #1450 B2 が `tasks` を足しても
//! **MCP のツール定義だけ追従しなかった**。CLI は `PanelViewWire::VALUES` から
//! possible values を組み立てるので `tako panel --view tasks` は通り、dispatch も
//! `parse` を通るので寛容なクライアントからは動く。壊れていたのは**申告**で、
//! スキーマの enum を尊重するクライアントは `tasks` を送れず、enum を読んで選ぶ
//! エージェントは**ビューの存在自体を知れない**。設計原則 5「AI フルコントロール」に反する。
//!
//! 原因は catalog.rs が正本の**手書きの写し**を持っていたこと。写しは黙って古くなる
//! （値を足した側は写しの存在を知らない）ので、目視でも単体テストでも気づけない。
//!
//! # 何を縛るか
//!
//! 1. **消費（`Consumed`）** — 正本を呼んで生成する 5 か所は、カタログに値を
//!    直書きしてはならない。直書きへ戻したら `file:line` で落ちる
//! 2. **束縛（`Bound`）** — まだ手書きだが正本が実行時に読める語彙は、値が正本と
//!    **完全一致**（並びも含む）でなければならない。正本へ値を足した瞬間に落ちる
//! 3. **登録漏れの検出** — 登録簿に載っている語彙と同じ値集合を、登録簿に無い
//!    (tool, prop) が手書きで持っていたら落ちる（新しいツールが写しを増やせない）
//! 4. **生成が本当に `tools()` へ届く** — 上の 1〜3 はソース走査なので、
//!    実際に組み上がった JSON でも正本と一致することを別途確かめる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜3 はすべて無意味に緑になるので、[`走査が空振りしていない`]
//! で採れた site 数を固定し、[`逆戻りを名指しできる`] で**注入 8 通り**が `file:line` で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。
//!
//! # ここが見ていない範囲（#1467 の棚卸し結果）
//!
//! catalog.rs の文字列 enum は 102 か所 / 相異なる値集合 75 種。そのうち正本が
//! **実行時に読める**のは登録簿の 20 site（値集合では 10 種）だけで、残りは
//! 「その場の action 動詞」か、正本が非公開（`ledger::VALID_TASK_TYPES` /
//! `agent_support::STATUSES`）か、列挙 API が無い（`TaskPhase` / `DeviceRole`）。
//! 一覧と扱いは Issue #1467 のコメントに残した。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tako_control::mcp;
use tako_control::orchestrator::ProfileKind;
use tako_control::protocol::PanelViewWire;
use tako_core::agent_support::Agent;
use tako_core::lsp::diagnostic::Severity;
use tako_core::remote_open::RemoteOpenTarget;
use tako_core::session_restart::SessionRestartMode;
use tako_core::ui_mode::UiMode;
use tako_core::user_task::{Decision, TaskKind, TaskStatus, Via};

#[path = "common/production_range.rs"]
mod production_range;

const CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";

/// カタログが正本へどう繋がっているか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// 正本を呼んで生成する（直書きは違反）
    Consumed,
    /// まだ手書きだが、値が正本と一致していることをここで縛る
    Bound,
}

/// 正本を持つ MCP の enum 1 site
struct Source {
    tool: &'static str,
    /// カタログ上のプロパティ名（配列の要素 enum は `items`）
    prop: &'static str,
    /// 正本の在処（落ちたときに直す先を名指しする）
    origin: &'static str,
    binding: Binding,
    values: Vec<String>,
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

fn agents(list: &[Agent]) -> Vec<String> {
    list.iter().map(|a| a.as_str().to_string()).collect()
}

/// 正本を持つ enum の登録簿。**ここに無い語彙は縛られていない**（上の「見ていない範囲」）
fn registry() -> Vec<Source> {
    let panel = owned(&PanelViewWire::accepted_values());
    let tui = agents(&Agent::TUI);
    let all_agents = agents(&Agent::ALL);

    let mut out = vec![
        // --- 消費: 正本を呼んで生成する（#1467 で寄せた 5 か所 + #1679 の 2 か所） ---
        Source {
            tool: "tako_panel",
            prop: "view",
            origin: "PanelViewWire::accepted_values()（tako-control/src/protocol.rs）",
            binding: Binding::Consumed,
            values: panel,
        },
        Source {
            tool: "tako_orchestrator_profiles",
            prop: "kind",
            origin: "ProfileKind::VALUES（tako-control/src/orchestrator/mod.rs）",
            binding: Binding::Consumed,
            values: owned(ProfileKind::VALUES),
        },
        Source {
            tool: "tako_session_restart",
            prop: "mode",
            origin: "SessionRestartMode::VALUES（tako-core/src/session_restart.rs）",
            binding: Binding::Consumed,
            values: owned(&SessionRestartMode::VALUES),
        },
        Source {
            tool: "tako_ui_mode",
            prop: "mode",
            origin: "UiMode::VALUES（tako-core/src/ui_mode.rs）",
            binding: Binding::Consumed,
            values: owned(&UiMode::VALUES),
        },
        Source {
            tool: "tako_open_remote",
            prop: "target",
            origin: "RemoteOpenTarget::VALUES（tako-core/src/remote_open.rs）",
            binding: Binding::Consumed,
            values: owned(&RemoteOpenTarget::VALUES),
        },
        // #1679: 言語機能の action と診断の重大度（どちらも正本から生成）
        Source {
            tool: "tako_lsp",
            prop: "action",
            origin: "dispatch::LSP_FEATURE_ACTIONS（tako-control/src/dispatch.rs）",
            binding: Binding::Consumed,
            values: owned(tako_control::dispatch::LSP_FEATURE_ACTIONS),
        },
        Source {
            tool: "tako_lsp",
            prop: "severity",
            origin: "Severity::NAMES（tako-core/src/lsp/diagnostic.rs）",
            binding: Binding::Consumed,
            values: owned(&Severity::NAMES),
        },
        // --- 束縛: 手書きのまま値だけ縛る（正本へ値を足したら落ちる） ---
        Source {
            tool: "tako_agent_support",
            prop: "agent",
            origin: "Agent::ALL（tako-core/src/agent_support.rs）",
            binding: Binding::Bound,
            values: all_agents,
        },
        Source {
            tool: "tako_todo",
            prop: "kind",
            origin: "TaskKind::all()（tako-core/src/user_task.rs）",
            binding: Binding::Bound,
            values: TaskKind::all()
                .iter()
                .map(|k| k.as_str().to_string())
                .collect(),
        },
        Source {
            tool: "tako_todo",
            prop: "status",
            origin: "TaskStatus::all()（tako-core/src/user_task.rs）",
            binding: Binding::Bound,
            values: TaskStatus::all()
                .iter()
                .map(|s| s.as_str().to_string())
                .collect(),
        },
        Source {
            tool: "tako_todo",
            prop: "decision",
            origin: "Decision::all()（tako-core/src/user_task.rs）",
            binding: Binding::Bound,
            values: Decision::all()
                .iter()
                .map(|d| d.as_str().to_string())
                .collect(),
        },
        Source {
            tool: "tako_todo",
            prop: "via",
            origin: "Via::all()（tako-core/src/user_task.rs）",
            binding: Binding::Bound,
            values: Via::all().iter().map(|v| v.as_str().to_string()).collect(),
        },
    ];

    // TUI をキー操作で駆動する 3 系統（`Agent::TUI`）を語彙とする site。
    // `tako_limit_service.service` の `LimitService` は別 enum だが、
    // 5 enum の並びは `agent_parity` テストが 1 つに縛っているので同じ正本を当てる
    for (tool, prop) in [
        ("tako_git_resolve_agent", "agent"),
        ("tako_setup_mcp", "agent"),
        ("tako_orchestrator_profiles", "worker_agent"),
        ("tako_orchestrator_profiles", "agent"),
        ("tako_orchestrator_spawn", "agent"),
        ("tako_orchestrator_run", "agent"),
        ("tako_limit_service", "service"),
        ("tako_setup_bootstrap", "agent"),
        ("tako_setup", "selected_agent"),
        ("tako_agents_sync_rules", "items"),
    ] {
        out.push(Source {
            tool,
            prop,
            origin: "Agent::TUI（tako-core/src/agent_support.rs）",
            binding: Binding::Bound,
            values: tui.clone(),
        });
    }
    out
}

// ---------------------------------------------------------------- ソース走査

/// カタログ上の enum 1 site
#[derive(Debug, Clone)]
struct Site {
    tool: String,
    prop: String,
    /// 1-based（`file:line` の名指しに使う）
    line: usize,
    /// `Some` = 値の直書き / `None` = 生成関数の呼び出し
    values: Option<Vec<String>>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn catalog_source(root: &Path) -> String {
    let raw = std::fs::read_to_string(root.join(CATALOG)).expect("catalog.rs が読める");
    production_range::production(&raw, CATALOG)
}

/// 生成関数の綴り（カタログがこれを呼んでいれば写しは無い）
const GENERATORS: [&str; 2] = ["panel_view_schema()", "enum_schema("];

/// 値を直書きしている enum の綴り
const ENUM_LITERAL: &str = "\"enum\": [";

/// `pos` より前にある「`"ident":` の直後」を読む（生成呼び出しのプロパティ名）
fn prop_before_call(line: &str, pos: usize) -> Option<String> {
    let head = line.get(..pos)?.trim_end();
    let head = head.strip_suffix(':')?;
    let head = head.trim_end();
    let inner = head.strip_suffix('"')?;
    let start = inner.rfind('"')?;
    Some(inner[start + 1..].to_string())
}

/// 直書き enum のプロパティ名（`"ident": {` を同じ行 → 手前の行の順に遡る）
fn prop_before_literal(lines: &[&str], idx: usize, pos: usize) -> Option<String> {
    for back in 0..=8usize {
        let i = idx.checked_sub(back)?;
        let hay = if back == 0 {
            lines[i].get(..pos)?
        } else {
            lines[i]
        };
        if let Some(at) = hay.rfind("\": {") {
            let inner = &hay[..at];
            let start = inner.rfind('"')?;
            return Some(inner[start + 1..].to_string());
        }
    }
    None
}

/// `"enum": [` の `[` から `]` までに並ぶ文字列を拾う（複数行に跨ってよい）
fn collect_enum(lines: &[&str], idx: usize, pos: usize) -> (Vec<String>, usize) {
    let mut values = Vec::new();
    let mut i = idx;
    let mut col = lines[idx][pos..]
        .find('[')
        .map(|o| pos + o + 1)
        .unwrap_or(pos);
    loop {
        let line = lines[i];
        let rest = line.get(col..).unwrap_or("");
        let end = rest.find(']');
        let scan = match end {
            Some(e) => &rest[..e],
            None => rest,
        };
        let mut chars = scan.char_indices().peekable();
        while let Some((s, c)) = chars.next() {
            if c != '"' {
                continue;
            }
            let tail = &scan[s + 1..];
            if let Some(e) = tail.find('"') {
                values.push(tail[..e].to_string());
                // 閉じ側の `"` まで読み飛ばす
                while let Some((j, _)) = chars.peek() {
                    if *j <= s + 1 + e {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        }
        if end.is_some() || i + 1 >= lines.len() {
            return (values, i);
        }
        i += 1;
        col = 0;
    }
}

fn scan_sites(src: &str) -> Vec<Site> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<Site> = Vec::new();
    let mut tool = String::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if let Some(at) = line.find("\"name\": \"tako_") {
            let tail = &line[at + "\"name\": \"".len()..];
            if let Some(e) = tail.find('"') {
                tool = tail[..e].to_string();
            }
        }
        for gen in GENERATORS {
            if let Some(pos) = line.find(gen) {
                if let Some(prop) = prop_before_call(line, pos) {
                    out.push(Site {
                        tool: tool.clone(),
                        prop,
                        line: i + 1,
                        values: None,
                    });
                }
            }
        }
        // 値の直書きは必ず `"enum": [`（rustfmt 済みなのでこの綴りで安定する）。
        // `enum_schema` の定義本体（`"enum": values`）はここに当たらない
        if let Some(pos) = line.find(ENUM_LITERAL) {
            let (values, last) = collect_enum(&lines, i, pos);
            let prop = prop_before_literal(&lines, i, pos).unwrap_or_else(|| "?".to_string());
            out.push(Site {
                tool: tool.clone(),
                prop,
                line: i + 1,
                values: Some(values),
            });
            i = last;
        }
        i += 1;
    }
    out
}

/// ツール名が宣言されている行（site が 1 つも無いときの名指し先）
fn tool_line(src: &str, tool: &str) -> usize {
    let needle = format!("\"name\": \"{tool}\"");
    src.lines()
        .position(|l| l.contains(&needle))
        .map(|i| i + 1)
        .unwrap_or(0)
}

#[derive(Debug)]
struct Offender {
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{CATALOG}:{} — {}", self.line, self.why)
    }
}

fn scan(src: &str) -> Vec<Offender> {
    let reg = registry();
    let sites = scan_sites(src);
    let mut out = Vec::new();

    for s in &reg {
        let here: Vec<&Site> = sites
            .iter()
            .filter(|x| x.tool == s.tool && x.prop == s.prop)
            .collect();
        if here.is_empty() {
            out.push(Offender {
                line: tool_line(src, s.tool),
                why: format!(
                    "{}.{} の enum が見つからない（正本 {} を消費する site が消えた）",
                    s.tool, s.prop, s.origin
                ),
            });
            continue;
        }
        for site in here {
            match (&site.values, s.binding) {
                (Some(values), Binding::Consumed) => out.push(Offender {
                    line: site.line,
                    why: format!(
                        "{}.{} の enum が直書きへ戻っている（正本 {} を呼んで生成すること。\
                         写しは正本への追加へ黙って追従しない = #1467 の症状）。直書き: {values:?}",
                        s.tool, s.prop, s.origin
                    ),
                }),
                (Some(values), Binding::Bound) => {
                    if values != &s.values {
                        out.push(Offender {
                            line: site.line,
                            why: format!(
                                "{}.{} の enum が正本とズレた（正本 {} = {:?} / カタログ = {values:?}）",
                                s.tool, s.prop, s.origin, s.values
                            ),
                        });
                    }
                }
                (None, Binding::Consumed) => {}
                (None, Binding::Bound) => {}
            }
        }
    }

    // 登録簿にある語彙と同じ値集合を、登録簿の外が手書きで持っていないか
    let mut vocab: BTreeMap<Vec<String>, &'static str> = BTreeMap::new();
    for s in &reg {
        let mut key = s.values.clone();
        key.sort();
        vocab.insert(key, s.origin);
    }
    for site in &sites {
        let Some(values) = &site.values else { continue };
        if reg
            .iter()
            .any(|s| s.tool == site.tool && s.prop == site.prop)
        {
            continue;
        }
        let mut key = values.clone();
        key.sort();
        if let Some(origin) = vocab.get(&key) {
            out.push(Offender {
                line: site.line,
                why: format!(
                    "{}.{} が正本 {origin} と同じ語彙を手書きで持っている（登録簿 \
                     `registry()` へ足すか、正本を呼んで生成すること。#1467）",
                    site.tool, site.prop
                ),
            });
        }
    }
    out
}

// ------------------------------------------------------------------ 検査本体

/// 走査が空振りしていないこと（ここが緑を作る土台）
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let src = catalog_source(&root);
    let sites = scan_sites(&src);

    let literals = sites.iter().filter(|s| s.values.is_some()).count();
    let generated = sites.iter().filter(|s| s.values.is_none()).count();
    // 直書きを 1 つも採り逃していないこと（綴りが変わったら黙って縮むのを防ぐ）
    let written = src.matches(ENUM_LITERAL).count();
    assert_eq!(
        literals, written,
        "直書き enum の採取漏れ（ソース上 {written} 件 / 採取 {literals} 件）"
    );
    assert!(
        literals >= 90,
        "直書き enum の採取が {literals} 件しかない（走査が壊れている。#1467 時点で 97 件）"
    );
    assert_eq!(
        generated, 7,
        "生成 site が 7 件でない（{generated} 件）。正本を消費する site を増減したら\
         `registry()` の Consumed も合わせること"
    );
    assert!(
        sites.iter().all(|s| s.tool.starts_with("tako_")),
        "ツール名が解決できていない site がある（走査が壊れている）"
    );
    assert!(
        sites.iter().all(|s| s.prop != "?"),
        "プロパティ名が解決できていない site がある: {:?}",
        sites
            .iter()
            .filter(|s| s.prop == "?")
            .map(|s| s.line)
            .collect::<Vec<_>>()
    );
}

/// 正本を持つ enum が、消費または束縛のどちらかで縛られていること
#[test]
fn カタログのenumは正本と一致する() {
    let root = workspace_root();
    let offenders = scan(&catalog_source(&root));
    assert!(
        offenders.is_empty(),
        "MCP カタログの enum が正本から外れている:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 組み上がった `tools()` でも正本と一致すること（ソース走査が空振りしても守る）
#[test]
fn 組み上がったtoolsが正本と一致する() {
    let tools = mcp::tools();
    for s in registry() {
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some(s.tool))
            .unwrap_or_else(|| panic!("{} が tools() に無い", s.tool));
        let found = enums_in(&tool["inputSchema"]);
        let actual = found
            .get(s.prop)
            .unwrap_or_else(|| panic!("{}.{} の enum が tools() に無い", s.tool, s.prop));
        assert_eq!(
            actual, &s.values,
            "{}.{} の enum が正本 {} とズレている",
            s.tool, s.prop, s.origin
        );
    }
}

/// #1467 の症状そのもの: `tasks` が enum にも説明にも載る
#[test]
fn tako_panelのviewはtasksを含み説明にも載る() {
    let tools = mcp::tools();
    let panel = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("tako_panel"))
        .expect("tako_panel が tools() に在る");

    let view = enums_in(&panel["inputSchema"]);
    let values = view.get("view").expect("view の enum が在る");
    assert_eq!(
        values,
        &PanelViewWire::accepted_values()
            .iter()
            .map(|v| (*v).to_string())
            .collect::<Vec<_>>(),
        "view の enum が受理値（正式値 + 旧称）と一致しない"
    );
    assert!(
        values.iter().any(|v| v == "tasks"),
        "view の enum に tasks が無い（#1467 の症状）"
    );
    // 旧称を落とすと既存クライアントが壊れるので、後方互換は残っていること
    assert!(values.iter().any(|v| v == "tmux"), "旧称 tmux が消えている");

    let description = panel["description"].as_str().expect("説明が在る");
    for v in PanelViewWire::VALUES {
        assert!(
            description.contains(&format!("view={v} は")),
            "ツール説明に {v} の説明が無い（#1467）: {description}"
        );
    }
    let prop_desc = panel["inputSchema"]["properties"]["view"]["description"]
        .as_str()
        .expect("view の説明が在る");
    for v in PanelViewWire::VALUES {
        assert!(
            prop_desc.contains(v),
            "view 引数の案内に {v} が無い: {prop_desc}"
        );
    }
}

/// inputSchema を辿って「プロパティ名 → 文字列 enum」を集める
/// （配列の要素 enum は親キーが `items` になる = ソース走査と同じ見え方）
fn enums_in(schema: &serde_json::Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    walk(schema, &mut out);
    out
}

fn walk(node: &serde_json::Value, out: &mut BTreeMap<String, Vec<String>>) {
    let Some(map) = node.as_object() else { return };
    for (k, v) in map {
        let Some(obj) = v.as_object() else { continue };
        if let Some(values) = obj.get("enum").and_then(|e| e.as_array()) {
            let strings: Vec<String> = values
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !strings.is_empty() {
                out.insert(k.clone(), strings);
            }
        }
        walk(v, out);
    }
}

/// **修正前を再現した注入**が `file:line` で名指しされることを確かめる。
/// 検出力の実証はここにしかない（緑を作るのは簡単だが、落とせることの証明は注入でしかできない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let clean = catalog_source(&root);
    assert!(scan(&clean).is_empty(), "注入前が既に汚れている");

    // (1) #1467 そのもの: panel の view を修正前の直書きへ戻す（tasks が抜ける）
    let injected = clean.replace(
        "                    \"view\": panel_view_schema(),",
        "                    \"view\": { \"type\": \"string\", \"enum\": [\"fleet\", \"orch\", \"git\", \"tmux\"] },",
    );
    expect_hit(
        &clean,
        &injected,
        "tako_panel.view の enum が直書きへ戻っている",
    );

    // (2) 値が合っていても写しは許さない（次の追加でまた同じことが起きる）
    let injected = clean.replace(
        "                    \"view\": panel_view_schema(),",
        "                    \"view\": { \"type\": \"string\", \"enum\": [\"fleet\", \"orch\", \"git\", \"tasks\", \"tmux\"] },",
    );
    expect_hit(
        &clean,
        &injected,
        "tako_panel.view の enum が直書きへ戻っている",
    );

    // (3) 生成呼び出しごと消える（プロパティが丸ごと落ちる）
    let injected = clean.replace("                    \"view\": panel_view_schema(),\n", "");
    expect_hit(&clean, &injected, "tako_panel.view の enum が見つからない");

    // (4) 別の消費 site（open_remote.target）が直書きへ戻る
    let injected = clean.replace(
        "                    \"target\": enum_schema(\n                        &tako_core::remote_open::RemoteOpenTarget::VALUES,",
        "                    \"target\": { \"type\": \"string\", \"enum\": [\"split\", \"tab\"] },\n                    \"_target\": enum_schema(\n                        &tako_core::remote_open::RemoteOpenTarget::VALUES,",
    );
    expect_hit(
        &clean,
        &injected,
        "tako_open_remote.target の enum が直書きへ戻っている",
    );

    // (5) 束縛側（todo.kind）が正本からズレる
    let injected = clean.replace(
        "\"enum\": [\"review\", \"confirm\", \"permission\", \"post\", \"other\"],",
        "\"enum\": [\"review\", \"confirm\", \"permission\", \"other\"],",
    );
    expect_hit(&clean, &injected, "tako_todo.kind の enum が正本とズレた");

    // (6) 束縛側（agent 3 系統）の 1 site だけが古くなる
    let injected = clean.replace(
        "                    \"agent\": { \"type\": \"string\", \"enum\": [\"claude\", \"codex\", \"agy\"], \"description\": \"エージェント種別（省略時はプロファイル既定）\" },",
        "                    \"agent\": { \"type\": \"string\", \"enum\": [\"claude\", \"codex\"], \"description\": \"エージェント種別（省略時はプロファイル既定）\" },",
    );
    expect_hit(&clean, &injected, "の enum が正本とズレた");

    // (7) 新しいツールが登録簿の外で同じ語彙の写しを持つ
    let injected = clean.replace(
        "        json!({\n            \"name\": \"tako_panel\",",
        "        json!({\n            \"name\": \"tako_fake_tool\",\n            \"description\": \"注入\",\n            \"inputSchema\": {\n                \"type\": \"object\",\n                \"properties\": {\n                    \"agent\": { \"type\": \"string\", \"enum\": [\"claude\", \"codex\", \"agy\"] },\n                },\n            },\n        }),\n        json!({\n            \"name\": \"tako_panel\",",
    );
    expect_hit(&clean, &injected, "同じ語彙を手書きで持っている");

    // (8) 束縛側（agent_support.agent）から local が落ちる
    let injected = clean.replace(
        "\"enum\": [\"claude\", \"codex\", \"agy\", \"local\"],",
        "\"enum\": [\"claude\", \"codex\", \"agy\"],",
    );
    expect_hit(&clean, &injected, "tako_agent_support.agent");
}

fn expect_hit(clean: &str, injected: &str, needle: &str) {
    assert_ne!(
        clean, injected,
        "注入が空振りしている（アンカーが古い）: {needle}"
    );
    let offenders = scan(injected);
    assert!(
        offenders.iter().any(|o| o.why.contains(needle)),
        "注入「{needle}」を名指しできていない。検出したのは:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        offenders.iter().all(|o| o.line > 0),
        "行番号が採れていない（file:line で名指しできない）"
    );
}

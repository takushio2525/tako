//! **#2912 の番犬**: tako が起こす claude（master / solo / worker）へ、tako の MCP を起動の行で渡す形を戻さない。
//!
//! ## 何が起きていたのか
//!
//! claude の master / solo / worker は、tako の MCP を起動の行では受け取らず、その claude の設定 dir の
//! user scope の登録（`<設定 dir>/.claude.json` の `mcpServers.tako`）に頼っていた。登録する側
//! （`tako setup` / `tako setup-mcp` / MCP `tako_setup_mcp`）と診断が見るのはそのプロセスの
//! `CLAUDE_CONFIG_DIR` の 1 か所だけなので、accounts.yaml のアカウントの設定 dir（#504）がそれと違うと、
//! そのアカウントで起こした claude から tako のツールが 0 本になるのに、setup / 診断は健全と言っていた。
//!
//! いまは起動の行で `--mcp-config=<data dir>/worker-mcp/tako-<hash>.json` を 1 語で渡す（codex の `-c` の
//! 一時注入 = #986 と同じ考え方）。ファイルが無い `--mcp-config` は claude ごと起動しないので、**書けたときだけ**
//! 付ける（worker = `WorkerMcp::materialized`・master / solo = `worker_mcp::ready_tako_launch_config`・
//! 再開 = `ResumeLaunch::materialize`）。診断（check-health / `tako setup --check`）はアカウントごとの
//! 項目 `mcp.claude.account.<名前>` を出す。
//!
//! ## ここで止めるもの
//!
//! 1. [`起動の行でtakoのmcpを渡す`] — 形を file:line で拘束する:
//!    - worker の断片 `worker_mcp_args` が claude に `mcp_config` から `--mcp-config=` を付け、
//!      `build_worker_cmd_in` が `launch.mcp_config` を渡す
//!    - master の組み立て `build_master_cmd_with_plan_in_typed` が claude の腕で `--mcp-config=` を付ける
//!    - 製品コードは master の組み立ての `None` を渡す口（`build_master_cmd` / `build_master_cmd_in` /
//!      `build_master_cmd_with_plan_in`）を呼ばない
//!    - `build_master_cmd_typed(` を呼ぶ関数は、その前で `ready_tako_launch_config(`（書けたファイルだけを返す）を呼ぶ。
//!      再開の `build_master_cmd_with_plan_in_typed(` の前は `tako_launch_config(`（materialize で書く）
//!    - 起動経路の `WorkerLaunch` が `mcp_config: worker_mcp.tako_config_file()` を渡し、復元の断片
//!      `resume_mcp_args` も `mcp.tako_config_file()` を渡す
//!    - 起動経路の `worker_mcp::prepare` / `prepare_for_resume` は tako の実行ファイルを `None` で渡さない
//!    - `--strict-mcp-config` を使わない（`.mcp.json` と local scope まで止まる = 実測）
//!    - 診断の `mcp_items` がアカウントごとの項目（`account_mcp_item`）を出し、`assemble`（`collect` の組み立て）が材料
//!      （`account_mcp_states()`）を渡す
//! 2. [`ab_のゲートは1か所`] — `TAKO_2912_LEGACY` を読むのは `worker_mcp::issue2912_legacy` の 1 か所
//! 3. [`修正前の形の注入を名指す`] — 1 の検査が、上の形を外した注入を file:line で名指すこと
//!
//! ## 相方
//!
//! 単体テスト（`worker_mcp::tests::issue2912_*`・`agent::tests::i2912_*`・`orchestrator::tests::i2912_*`・
//! `resume_launch::tests::issue2912_*`・`agent_mcp::tests::issue2912_*`・`diagnostics::tests::issue2912_*`）と、
//! 実経路 `bash scripts/test-account-mcp-2912.sh`（隔離 GUI + 実物の claude + 偽の Messages API）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const AGENT: &str = "crates/tako-control/src/orchestrator/agent.rs";
const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const WORKER_MCP: &str = "crates/tako-control/src/worker_mcp.rs";
const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
const DIAG: &str = "crates/tako-control/src/diagnostics.rs";
/// 起動経路の `WorkerLaunch` を持つファイル（#2412 の番犬と同じ）
const LAUNCH_FILES: &[&str] = &[
    "crates/tako-control/src/dispatch.rs",
    "crates/tako-control/src/resume_launch.rs",
];
/// 起動の組み立てを持つファイル（`--strict-mcp-config` を使わない）
const BUILDERS: &[&str] = &[
    AGENT,
    ORCH,
    WORKER_MCP,
    SESSIONS,
    "crates/tako-control/src/resume_launch.rs",
];
/// 製品コードを持つ 4 クレート
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];
/// master の組み立てのうち、起動の行の tako の MCP に `None` を渡す口（テストの比べる相手）
const NONE_WRAPPERS: &[&str] = &[
    "build_master_cmd(",
    "build_master_cmd_in(",
    "build_master_cmd_with_plan_in(",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_rel(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める（消した / 改名したなら番犬も追うこと）: {e}"))
}

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ）
fn product_text(raw: &str) -> String {
    production_range::scan(raw).text
}

fn product_files() -> Vec<(String, String)> {
    production_range::product_sources()
        .into_iter()
        .filter(|rel| ROOTS.iter().any(|r| rel.starts_with(r)))
        .map(|rel| {
            let raw = read_rel(&rel);
            (rel, raw)
        })
        .collect()
}

/// 1 ファイルだけのグラフ（関数の範囲を引く）と、テスト領域を潰した本文（文字列は残る）
struct FileView {
    rel: String,
    text: String,
    graph: call_graph::Graph,
}

impl FileView {
    fn new(rel: &str, raw: &str) -> Self {
        let text = product_text(raw);
        let graph = call_graph::Graph::build(vec![call_graph::Source::new(rel, &text)]);
        Self {
            rel: rel.to_string(),
            text,
            graph,
        }
    }

    fn code(&self) -> &str {
        &self.graph.sources[0].code
    }

    fn line_of(&self, at: usize) -> usize {
        self.graph.sources[0].line_of(at)
    }

    /// `name` の関数（最初の定義）の行と製品の経路の範囲
    fn fn_view(&self, name: &str) -> Option<(usize, Vec<(usize, usize)>)> {
        self.graph
            .find(&self.rel, name)
            .into_iter()
            .min_by_key(|&f| self.graph.fns[f].line)
            .map(|f| (self.graph.fns[f].line, self.graph.product_ranges(f)))
    }

    /// `name` の関数（最初の定義）の行と本体全体の範囲（A/B の腕も含む。`legacy()` を含む式の中の呼び出しを見る）
    fn fn_body(&self, name: &str) -> Option<(usize, Vec<(usize, usize)>)> {
        self.graph
            .find(&self.rel, name)
            .into_iter()
            .min_by_key(|&f| self.graph.fns[f].line)
            .map(|f| {
                let d = &self.graph.fns[f];
                (d.line, vec![(d.body.0, d.body.1 + 1)])
            })
    }

    /// `at` を囲むいちばん内側の関数
    fn enclosing(&self, at: usize) -> Option<usize> {
        self.graph
            .fns
            .iter()
            .enumerate()
            .filter(|(_, d)| d.body.0 <= at && at <= d.body.1)
            .min_by_key(|(_, d)| d.body.1 - d.body.0)
            .map(|(i, _)| i)
    }

    /// 範囲の中で `needle` が最初に現れる位置（`raw` = 文字列も見る本文 / それ以外は文字列を潰した眺め）
    fn first_in(&self, ranges: &[(usize, usize)], needle: &str, raw: bool) -> Option<usize> {
        let hay = if raw { self.text.as_str() } else { self.code() };
        ranges
            .iter()
            .find_map(|&(s, e)| hay.get(s..e)?.find(needle).map(|i| s + i))
    }
}

/// `at` の `(` から対応する `)` までの中身（文字列を潰した眺め）
fn call_args(code: &str, open: usize) -> &str {
    let bytes = code.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    &code[open + 1..]
}

/// `WorkerLaunch {` から対応する `}` までを (開始位置, 本文) で切り出す（波括弧の釣り合いで数える）
fn worker_launch_blocks(code: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let bytes = code.as_bytes();
    let mut from = 0;
    while let Some(rel) = code[from..].find("WorkerLaunch {") {
        let start = from + rel;
        let mut depth = 0usize;
        let mut end = start;
        for (i, b) in bytes[start..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(end > start, "WorkerLaunch の括弧が閉じていない");
        out.push((start, code[start..end].to_string()));
        from = end;
    }
    out
}

/// 1 の本体（組み立ての形）: 起動の組み立てが tako の MCP を渡す形から外れた箇所を file:line で並べる
fn builder_offenders(files: &[(String, String)]) -> Vec<String> {
    let get = |rel: &str| -> Option<FileView> {
        files
            .iter()
            .find(|(r, _)| r == rel)
            .map(|(r, raw)| FileView::new(r, raw))
    };
    let mut out = Vec::new();
    let missing =
        |rel: &str, name: &str| format!("{rel}: `fn {name}` が無い（改名したなら番犬も追うこと）");

    if let Some(v) = get(AGENT) {
        match v.fn_view("worker_mcp_args") {
            None => out.push(missing(AGENT, "worker_mcp_args")),
            Some((line, ranges)) => {
                if v.first_in(&ranges, "(agent, mcp_config)", false).is_none()
                    || v.first_in(&ranges, "--mcp-config=", true).is_none()
                {
                    out.push(format!(
                        "{AGENT}:{line}（fn worker_mcp_args）が claude に `mcp_config` から `--mcp-config=<path>` を付けていない = worker は設定 dir の登録に頼り、アカウントの設定 dir に登録が無いと tako のツールが 0 本（#2912）"
                    ));
                }
            }
        }
        match v.fn_view("build_worker_cmd_in") {
            None => out.push(missing(AGENT, "build_worker_cmd_in")),
            Some((line, ranges)) => {
                if v.first_in(&ranges, "launch.mcp_config", false).is_none() {
                    out.push(format!(
                        "{AGENT}:{line}（fn build_worker_cmd_in）が `launch.mcp_config` を断片へ渡していない（#2912）"
                    ));
                }
            }
        }
    }

    if let Some(v) = get(ORCH) {
        match v.fn_view("build_master_cmd_with_plan_in_typed") {
            None => out.push(missing(ORCH, "build_master_cmd_with_plan_in_typed")),
            Some((line, ranges)) => {
                if v.first_in(&ranges, "= claude_mcp_config", false).is_none()
                    || v.first_in(&ranges, "--mcp-config=", true).is_none()
                {
                    out.push(format!(
                        "{ORCH}:{line}（fn build_master_cmd_with_plan_in_typed）が claude の腕で `claude_mcp_config` から `--mcp-config=<path>` を付けていない = master / solo は設定 dir の登録に頼る（#2912）"
                    ));
                }
            }
        }
    }

    for (rel, raw) in files {
        let v = FileView::new(rel, raw);
        let code = v.code();
        // master の組み立ての None の口（テストの比べる相手）を製品コードが呼ばない
        if rel != ORCH {
            for needle in NONE_WRAPPERS {
                for (at, _) in code.match_indices(needle) {
                    // `fn build_master_cmd(` の定義・`build_master_cmd_typed(` などの長い名前の一部は除く
                    let before = &code[..at];
                    if before.ends_with("fn ") || before.ends_with('_') {
                        continue;
                    }
                    out.push(format!(
                        "{rel}:{}: `{needle}` は起動の行に tako の MCP を付けない口（テストの比べる相手）。`build_master_cmd_typed` / `build_master_cmd_with_plan_in_typed` に書けたファイルを渡すこと（#2912）",
                        v.line_of(at)
                    ));
                }
            }
        }
        // master の起動は、組み立ての前に設定ファイルを用意する
        for (call, prep) in [
            ("build_master_cmd_typed(", "ready_tako_launch_config("),
            (
                "build_master_cmd_with_plan_in_typed(",
                "tako_launch_config(",
            ),
        ] {
            for (at, _) in code.match_indices(call) {
                let before = &code[..at];
                if before.ends_with("fn ") || before.ends_with('_') || rel == ORCH {
                    continue;
                }
                let Some(f) = v.enclosing(at) else { continue };
                let ranges = v.graph.product_ranges(f);
                let prepared = v.first_in(&ranges, prep, false).is_some_and(|p| p < at);
                if !prepared {
                    out.push(format!(
                        "{rel}:{}（fn {}）が `{call}` の前に `{prep}` で tako の MCP の設定ファイルを用意していない = この経路の claude の master / solo は設定 dir の登録に頼る（#2912）",
                        v.line_of(at),
                        v.graph.fns[f].name
                    ));
                }
            }
        }
        // 起動経路の判断は tako の実行ファイルを None で渡さない（None = 起動の行に付かない）
        for needle in ["worker_mcp::prepare(", "worker_mcp::prepare_for_resume("] {
            for (at, _) in code.match_indices(needle) {
                let Some(f) = v.enclosing(at) else { continue };
                let name = v.graph.fns[f].name.as_str();
                let ranges = v.graph.product_ranges(f);
                let launches = v.first_in(&ranges, "WorkerLaunch {", false).is_some()
                    || name == "resume_worker_mcp";
                if !launches {
                    continue;
                }
                let args = call_args(code, at + needle.len() - 1);
                let last = args
                    .trim_end()
                    .trim_end_matches(',')
                    .rsplit(',')
                    .next()
                    .unwrap_or("")
                    .trim();
                if last == "None" {
                    out.push(format!(
                        "{rel}:{}（fn {name}）が起動経路の `{needle}` へ tako の実行ファイルを None で渡す = claude に起動の行で tako の MCP が付かない（#2912）",
                        v.line_of(at)
                    ));
                }
            }
        }
    }

    for &rel in LAUNCH_FILES {
        let Some(v) = get(rel) else { continue };
        let blocks = worker_launch_blocks(v.code());
        if blocks.is_empty() {
            out.push(format!(
                "{rel}: 起動経路の WorkerLaunch が見つからない（改名したなら番犬も追うこと）"
            ));
        }
        for (at, block) in blocks {
            if !block.contains("mcp_config: worker_mcp.tako_config_file()") {
                out.push(format!(
                    "{rel}:{}: 起動経路の WorkerLaunch が `mcp_config: worker_mcp.tako_config_file()` を渡していない = この経路の claude worker は設定 dir の登録に頼る（#2912）",
                    v.line_of(at)
                ));
            }
        }
    }

    if let Some(v) = get(SESSIONS) {
        match v.fn_view("resume_mcp_args") {
            None => out.push(missing(SESSIONS, "resume_mcp_args")),
            Some((line, ranges)) => {
                if v.first_in(&ranges, "mcp.tako_config_file()", false)
                    .is_none()
                {
                    out.push(format!(
                        "{SESSIONS}:{line}（fn resume_mcp_args）が復元の断片へ `mcp.tako_config_file()` を渡していない = 再起動をまたいだ復元で tako の MCP が外れる（#2912）"
                    ));
                }
            }
        }
    }

    for &rel in BUILDERS {
        let Some(v) = get(rel) else { continue };
        // コメント（「採らない」の説明）は見ない。文字列（付ける引数）は見る
        for (i, line) in v.text.lines().enumerate() {
            if !line.trim_start().starts_with("//") && line.contains("--strict-mcp-config") {
                out.push(format!(
                    "{rel}:{}: 起動の組み立てで `--strict-mcp-config` を使う = `.mcp.json` と local scope まで止まる（#2912 / #2412 の実測）",
                    i + 1
                ));
            }
        }
    }

    if let Some(v) = get(DIAG) {
        for (name, needle) in [
            ("mcp_items", "account_mcp_item("),
            ("assemble", "account_mcp_states()"),
        ] {
            match v.fn_body(name) {
                None => out.push(missing(DIAG, name)),
                Some((line, ranges)) => {
                    if v.first_in(&ranges, needle, false).is_none() {
                        out.push(format!(
                            "{DIAG}:{line}（fn {name}）が `{needle}` を通していない = 診断がアカウントごとの設定 dir の tako の MCP を出さない（#2912）"
                        ));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn 起動の行でtakoのmcpを渡す() {
    let hits = builder_offenders(&product_files());
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn ab_のゲートは1か所() {
    let mut found = Vec::new();
    for (rel, raw) in product_files() {
        for (i, line) in product_text(&raw).lines().enumerate() {
            if line.contains("\"TAKO_2912_LEGACY\"") {
                found.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "A/B のゲート `TAKO_2912_LEGACY` を読むのは製品コードに 1 か所（worker_mcp::issue2912_legacy）: {found:?}"
    );
    assert!(found[0].starts_with(WORKER_MCP), "{found:?}");
}

#[test]
fn 修正前の形の注入を名指す() {
    let files = product_files();
    assert!(builder_offenders(&files).is_empty(), "今の形は名指さない");
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "worker の断片が --mcp-config を付けない",
            AGENT,
            "(WorkerAgent::Claude, Some(path)) = (agent, mcp_config)",
            "(WorkerAgent::Claude, Some(path)) = (agent, None::<&str>)",
        ),
        (
            "master の組み立てが --mcp-config を付けない",
            ORCH,
            "if let Some(path) = claude_mcp_config {",
            "if let Some(path) = None::<&str> {",
        ),
        (
            "CLI の master が設定ファイルを用意しない",
            "crates/tako-cli/src/main.rs",
            "let mcp_config = tako_control::worker_mcp::ready_tako_launch_config(&tako_bin, \"master\");",
            "let mcp_config: Option<String> = None;",
        ),
        (
            "spawn の WorkerLaunch が mcp_config を渡さない",
            "crates/tako-control/src/dispatch.rs",
            "mcp_config: worker_mcp.tako_config_file(),",
            "mcp_config: None,",
        ),
        (
            "spawn の判断へ tako の実行ファイルを渡さない",
            "crates/tako-control/src/dispatch.rs",
            "            Some(&tako_bin),\n        )\n        .materialized();",
            "            None,\n        )\n        .materialized();",
        ),
        (
            "復元の断片が mcp_config を渡さない",
            SESSIONS,
            "        mcp.tako_config_file(),\n",
            "        None,\n",
        ),
        (
            "--strict-mcp-config を足す",
            AGENT,
            "        s.push_str(&lc::quote(dialect, &format!(\"--mcp-config={path}\")));",
            "        s.push_str(&lc::quote(dialect, &format!(\"--mcp-config={path}\")));\n        s.push_str(\" --strict-mcp-config\");",
        ),
        (
            "診断がアカウントごとの項目を出さない",
            DIAG,
            ".map(|account| account_mcp_item(account, launch, lang)),",
            ".map(|_account| unreachable!()),",
        ),
    ];
    for &(what, rel, from, to) in cases {
        let raw = files
            .iter()
            .find(|(r, _)| r == rel)
            .map(|(_, raw)| raw.clone())
            .unwrap_or_else(|| panic!("{rel} が製品のファイルに無い"));
        assert!(
            raw.contains(from),
            "注入の起点が {rel} に無い（{what}。形を変えたなら注入も追うこと）: {from}"
        );
        let injected: Vec<(String, String)> = files
            .iter()
            .map(|(r, text)| {
                if r == rel {
                    (r.clone(), text.replacen(from, to, 1))
                } else {
                    (r.clone(), text.clone())
                }
            })
            .collect();
        let hits = builder_offenders(&injected);
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{rel}:"))),
            "注入（{what}）を {rel} の file:line で名指せない: {hits:?}"
        );
    }
    // 製品コードが None の口を呼ぶ形
    let mut injected = files.clone();
    injected.push((
        "crates/tako-control/src/injected_2912.rs".to_string(),
        "fn f() {\n    let _ = crate::orchestrator::build_master_cmd(\"m\", &p, &pp, \"tako\");\n}\n"
            .to_string(),
    ));
    let hits = builder_offenders(&injected);
    assert!(
        hits.iter()
            .any(|h| h.starts_with("crates/tako-control/src/injected_2912.rs:2:")),
        "None の口を呼ぶ注入を file:line で名指せない: {hits:?}"
    );
}

//! **#2942 の番犬**: 認証の問い合わせ（`claude auth status --json` / `codex login status` /
//! `agy models`）を**上限なしで待つ口**へ、製品の既定の経路から届かないようにする。
//!
//! ## 何が起きていたのか
//!
//! #2255 で環境診断（`tako check-health` / `tako setup --check`）の認証確認は上限つきの
//! `setup_bootstrap::auth_state_within` へ寄せたが、同じ関数を呼ぶほかの口
//! （`tako setup bootstrap status` / `status-all`・MCP `tako_setup_bootstrap`・`tako setup` の検出と
//! 導入の段）は上限なしの `auth_state_for`（= `agent_probe::run` = `Command::output()`）のままだった。
//! Windows 実機では `auth status` に返らない claude で `tako setup bootstrap status --agent claude` が
//! 207 秒戻らず、何も出さずに固まった（#1503 の「無言にしない」に反する）。
//!
//! #2255 の番犬は「環境診断の入口から**辿って届く**関数」だけを見ていたので、診断の外の口は
//! 見逃した。ここは入口を選ばず、**本番コード全体の形の性質**として持つ（#1533 の番犬と同じ考え方）。
//!
//! ## ここで止めるもの
//!
//! 1. [`上限なしの認証の口はabの旧経路の腕からしか呼ばない`] — 本番コード（4 クレートの `src/`）の
//!    関数が、上限なしの認証の口（[`UNBOUNDED`] = `auth_state_for` / `status_all_legacy`）を
//!    A/B の旧経路の腕（`if …legacy() {`）の外から呼んだら、その行を file:line で名指す。
//!    上限なしの口を経ずに `agent_probe::run` と `auth_probe_argv` を同じ関数に並べる形
//!    （認証の問い合わせを上限なしで直書きした形）も名指す。どのスレッドで待っても呼び手は固まるので、
//!    別スレッドへ渡すクロージャの中も見る（`call_graph::Graph::product_ranges`）。
//!    上限つきの門番（`agent_probe::output_within`）が**直接の子だけを kill する口**
//!    （`probe::output_with_timeout`）で待つ形も名指す（打ち切ると孫 = `claude` の包みの先の `node`・
//!    `claude mcp list` の MCP サーバが孤児として残る。#2942 の受け入れ）
//! 2. [`利用者の口は上限つきの認証確認を通る`] — 禁止だけでなく**寄せ先も縛る**（#1496 の 2 本立て）。
//!    CLI（`run_bootstrap` / `tako setup` の検出と導入の段）・MCP（dispatch の `SetupBootstrap` が呼ぶ
//!    `status_for_typed` / `status_all_json`）から上限つきの口（`auth_state_within` →
//!    `agent_probe::output_within`）へ届き、上限が `probe::auth_probe_timeout()` から来ること・
//!    門番がグループを率いさせる口（`probe::output_with_timeout_grouped`）で待つこと
//! 3. [`修正前の形を名指しできる`] — 修正前の形の注入（`status_for_typed` を上限なしへ戻す・
//!    `status_all` の A/B の門を外す・CLI の検出を上限なしへ戻す・認証の問い合わせを直書きする・
//!    上限つきの口の門を常に旧経路へ倒す・門番を直接の子だけを kill する口へ戻す）を 1 / 2 が
//!    名指しすること（同じテストの中の A/B）
//!
//! ## 相方
//!
//! 実経路（返らない偽の `claude auth status` で、修正前は固まり・修正後は上限の秒数で「認証: 不明」を
//! 返す。CLI の人が読む形・`--json`・MCP）は `scripts/test-bootstrap-auth-timeout-2942.sh`。
//! 待ちそのものの挙動（孫まで止める）は `tako_core::probe` の単体テスト。
//! 環境診断の入口からの到達は #2255 の番犬（`issue2255_diagnostics_auth_probe_watchdog.rs`）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const SETUP_BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const AGENT_PROBE: &str = "crates/tako-control/src/agent_probe.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";

/// 走査する範囲（本番コードを持つ 4 クレート全部。`poc/` は品質基準の対象外）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 上限なしで認証を待つ口（[`SETUP_BOOTSTRAP`] の関数。右は呼んでよい場所）。
/// どちらも A/B の旧経路の腕だけが呼ぶ
const UNBOUNDED: &[(&str, &str)] = &[
    (
        "auth_state_for",
        "`auth_state_bounded` の `TAKO_2942_LEGACY` の腕と `status_all_legacy`",
    ),
    (
        "status_all_legacy",
        "`status_all` の `TAKO_2942_LEGACY` の腕と、環境診断の `TAKO_2255_LEGACY` の腕",
    ),
];

/// 利用者の口（寄せ先を縛る入口。`(ファイル, 関数, 口)`）
const ENTRIES: &[(&str, &str, &str)] = &[
    (
        CLI_SETUP,
        "run_bootstrap",
        "`tako setup bootstrap status` / `status-all` / `handoff`",
    ),
    (
        CLI_SETUP,
        "detect_agents",
        "`tako setup` の検出（`[検出]` の行）",
    ),
    (
        CLI_SETUP,
        "run_bootstrap_stage",
        "`tako setup` の導入の段（導入 → PATH → ログイン）",
    ),
    (
        SETUP_BOOTSTRAP,
        "status_for_typed",
        "MCP `tako_setup_bootstrap` の `status`（dispatch の `SetupBootstrap`）",
    ),
    (
        SETUP_BOOTSTRAP,
        "status_all_json",
        "MCP `tako_setup_bootstrap` の `status-all`（dispatch の `SetupBootstrap`）",
    ),
    (
        SETUP_BOOTSTRAP,
        "is_authenticated_for",
        "`tako setup` の認証の確かめ（`TAKO_1129_LEGACY` の後の確認）",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）
fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

/// [`ROOTS`] の `.rs` を全部（テスト領域を潰した本文と、その眺め）。読むのは 1 回だけ
fn graph_sources() -> &'static [(String, call_graph::Source)] {
    static CACHE: OnceLock<Vec<(String, call_graph::Source)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let entries =
                std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        let root = workspace_root();
        let mut paths = Vec::new();
        for dir in ROOTS {
            walk(&root.join(dir), &mut paths);
        }
        paths.sort();
        paths
            .iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(&root)
                    .expect("ワークスペースの中")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = read(&rel);
                let source = call_graph::Source::new(&rel, &text);
                (text, source)
            })
            .collect()
    })
}

/// `overrides`（`(相対パス, 本文)` = 注入）で差し替えたグラフ
fn graph_with(overrides: &[(&str, &str)]) -> call_graph::Graph {
    let sources = graph_sources()
        .iter()
        .map(
            |(text, source)| match overrides.iter().find(|(rel, _)| *rel == source.rel) {
                Some((rel, injected)) if injected != text => call_graph::Source::new(rel, injected),
                _ => source.clone(),
            },
        )
        .collect();
    call_graph::Graph::build(sources)
}

/// ファイル `rel` の素の関数 `name`（ちょうど 1 つでなければ理由つきで落ちる）
fn one_fn(graph: &call_graph::Graph, rel: &str, name: &str) -> usize {
    match graph.find(rel, name).as_slice() {
        [f] => *f,
        [] => panic!("{rel} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）"),
        many => panic!("{rel} に `fn {name}` が {} 個ある", many.len()),
    }
}

/// `code` の中で、識別子の途中ではない位置に現れる `needle` の開始位置
fn word_hits(code: &str, needle: &str) -> Vec<usize> {
    code.match_indices(needle)
        .filter(|(i, _)| {
            !code[..*i]
                .bytes()
                .last()
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(|(i, _)| i)
        .collect()
}

/// 1 の本体: 上限なしの認証の口を A/B の腕の外から呼ぶ行と、認証の問い合わせを上限なしで直書きした行
fn offenders(graph: &call_graph::Graph) -> Vec<String> {
    let unbounded: Vec<usize> = UNBOUNDED
        .iter()
        .map(|(name, _)| one_fn(graph, SETUP_BOOTSTRAP, name))
        .collect();
    let mut out = Vec::new();
    for (f, def) in graph.fns.iter().enumerate() {
        if unbounded.contains(&f) {
            continue;
        }
        let source = &graph.sources[def.file];
        let ranges = graph.product_ranges(f);
        for &(s, e) in &ranges {
            let code = &source.code[s..e];
            for (name, allowed) in UNBOUNDED {
                for i in word_hits(code, &format!("{name}(")) {
                    out.push(format!(
                        "{}:{}（fn {}）が上限なしの認証の口 `{name}` を A/B の腕（`if …legacy() {{`）の外から呼ぶ\
                         （呼んでよいのは {allowed}）",
                        source.rel,
                        source.line_of(s + i),
                        def.name
                    ));
                }
            }
        }
        // 上限なしの口を経ずに、認証の問い合わせを上限なしの門番で直に起こす形
        let probe_args: Vec<usize> = ranges
            .iter()
            .flat_map(|&(s, e)| {
                word_hits(&source.code[s..e], "auth_probe_argv(")
                    .into_iter()
                    .map(move |i| s + i)
            })
            .collect();
        let runs: Vec<usize> = ranges
            .iter()
            .flat_map(|&(s, e)| {
                source.code[s..e]
                    .match_indices("agent_probe::run(")
                    .map(move |(i, _)| s + i)
            })
            .collect();
        if !probe_args.is_empty() {
            for at in runs {
                out.push(format!(
                    "{}:{}（fn {}）が認証の問い合わせ（`auth_probe_argv`）を上限なしの `agent_probe::run` で起こす",
                    source.rel,
                    source.line_of(at),
                    def.name
                ));
            }
        }
    }
    // 上限つきの門番が、直接の子だけを kill する口で待つ形（打ち切ると孫が孤児として残る）
    let gate = one_fn(graph, AGENT_PROBE, "output_within");
    let def = &graph.fns[gate];
    let source = &graph.sources[def.file];
    for &(s, e) in &graph.product_ranges(gate) {
        for i in word_hits(&source.code[s..e], "probe::output_with_timeout(") {
            out.push(format!(
                "{}:{}（fn output_within）が直接の子だけを kill する `probe::output_with_timeout` で待つ\
                 （打ち切ると孫が残る。`probe::output_with_timeout_grouped` を通すこと = #2942）",
                source.rel,
                source.line_of(s + i)
            ));
        }
    }
    out
}

/// 1: 本番コードで、上限なしの認証の口を呼ぶのは A/B の旧経路の腕だけ
#[test]
fn 上限なしの認証の口はabの旧経路の腕からしか呼ばない() {
    let graph = graph_with(&[]);
    let hits = offenders(&graph);
    assert!(
        hits.is_empty(),
        "認証の問い合わせを上限なしで待つ形が製品の既定の経路にある（#2942）:\n  {}\n\n\
         返らない claude / codex / agy が 1 つあると、呼び手（`tako setup bootstrap status` など）が\n\
         何も出さずに固まる。認証は `setup_bootstrap::auth_state_bounded`（環境診断と同じ上限）か\n\
         `auth_state_within`（上限を呼び手が決める）で聞き、打ち切りの知らせを呼び手へ返すこと",
        hits.join("\n  ")
    );
    // 上限なしの口が本当に上限なしの門番を通っていること（番犬の前提。通っていなければ 1 は空振り）
    let gate = one_fn(&graph, AGENT_PROBE, "run");
    let auth_for = one_fn(&graph, SETUP_BOOTSTRAP, "auth_state_for");
    assert!(
        graph.callees(auth_for).contains(&gate),
        "{SETUP_BOOTSTRAP} の `auth_state_for` が `agent_probe::run` を呼んでいない。\
         上限なしの口の一覧（UNBOUNDED）が実態とずれた（番犬も直すこと）"
    );
}

/// 2: 利用者の口から上限つきの認証確認へ届き、上限は `auth_probe_timeout()` から来る
#[test]
fn 利用者の口は上限つきの認証確認を通る() {
    let graph = graph_with(&[]);
    let bounded = one_fn(&graph, SETUP_BOOTSTRAP, "auth_state_within");
    let unbounded: Vec<usize> = UNBOUNDED
        .iter()
        .map(|(name, _)| one_fn(&graph, SETUP_BOOTSTRAP, name))
        .collect();
    let mut problems = Vec::new();
    for (rel, name, entry_label) in ENTRIES {
        let entry = one_fn(&graph, rel, name);
        let reach = graph.reach(&[entry], &|_, _| false);
        if !reach.contains_key(&bounded) {
            problems.push(format!(
                "{rel} の `{name}`（{entry_label}）から上限つきの `auth_state_within` へ届かない"
            ));
        }
        for &u in &unbounded {
            if reach.contains_key(&u) {
                let route: String = graph
                    .path_to(&reach, u)
                    .iter()
                    .map(|(f, _)| format!(" → `{}`", graph.fns[*f].name))
                    .collect();
                problems.push(format!(
                    "{rel} の `{name}`（{entry_label}）から上限なしの `{}` へ届く。道筋:{route}",
                    graph.fns[u].name
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "利用者の口の認証確認が上限つきの口を通っていない（#2942）:\n  {}",
        problems.join("\n  ")
    );

    // 上限つきの既定の口は、環境診断と同じ上限（`auth_probe_timeout()`）で上限つきの口を呼ぶ
    let default_gate = one_fn(&graph, SETUP_BOOTSTRAP, "auth_state_bounded");
    assert!(
        graph.callees(default_gate).contains(&bounded),
        "{SETUP_BOOTSTRAP} の `auth_state_bounded` が `auth_state_within` を呼んでいない（#2942）"
    );
    assert!(
        !graph.hits(default_gate, "auth_probe_timeout()").is_empty(),
        "{SETUP_BOOTSTRAP} の `auth_state_bounded` が上限を `tako_core::probe::auth_probe_timeout()` \
         から取っていない（#2942。環境診断と同じ上限・同じ env `TAKO_AUTH_PROBE_TIMEOUT_SECS`）"
    );
    let status_all = one_fn(&graph, SETUP_BOOTSTRAP, "status_all");
    assert!(
        !graph.hits(status_all, "auth_probe_timeout()").is_empty(),
        "{SETUP_BOOTSTRAP} の `status_all` が上限を `auth_probe_timeout()` から取っていない（#2942）"
    );
    let output_within = one_fn(&graph, AGENT_PROBE, "output_within");
    assert!(
        graph.callees(bounded).contains(&output_within),
        "{SETUP_BOOTSTRAP} の `auth_state_within` が上限つきの門番 `agent_probe::output_within` を呼んでいない"
    );
    // 門番は子にグループを率いさせる口で待つ（打ち切ったら孫まで止める。unix。Windows は Job Object）
    let def = &graph.fns[output_within];
    let gate_code: String = graph
        .product_ranges(output_within)
        .iter()
        .map(|&(s, e)| &graph.sources[def.file].code[s..e])
        .collect();
    assert!(
        gate_code.contains("tako_core::probe::output_with_timeout_grouped(program, args, budget)"),
        "{AGENT_PROBE} の `output_within` が製品の既定の経路で `probe::output_with_timeout_grouped` を\
         通っていない（#2942。打ち切った子の孫が残る）"
    );

    // MCP の口: dispatch の `SetupBootstrap` の腕が上の 2 つの入口を呼ぶ（入口の一覧の前提）
    let dispatch = read(DISPATCH);
    let arm_at = dispatch
        .find("Request::SetupBootstrap {")
        .expect("dispatch に `Request::SetupBootstrap {` の腕がある");
    let arm: String = dispatch[arm_at..]
        .lines()
        .take(40)
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "setup_bootstrap::status_for_typed(",
        "setup_bootstrap::status_all_json(",
    ] {
        assert!(
            arm.contains(needle),
            "{DISPATCH} の `SetupBootstrap` の腕に `{needle}` が無い。MCP `tako_setup_bootstrap` の入口が\
             変わったなら、この番犬の ENTRIES も追うこと（#2942）"
        );
    }
}

/// 3: 修正前の形・直書き・門の倒しの注入を、1 / 2 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    let bootstrap = read(SETUP_BOOTSTRAP);
    let cli = read(CLI_SETUP);
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let line_of = |src: &str, needle: &str| {
        src.lines()
            .position(|l| l.contains(needle))
            .map(|i| i + 1)
            .unwrap_or_else(|| panic!("注入した行 `{needle}` がある"))
    };
    let expect_named = |label: &str, hits: &[String], needle: &str| {
        assert!(
            hits.iter().any(|h| h.contains(needle)),
            "注入「{label}」を名指しできない（`{needle}` を含む行が無い）: {hits:#?}"
        );
        for h in hits {
            eprintln!("注入「{label}」→ {h}");
        }
    };

    // ① 修正前: bootstrap の status（CLI / MCP）が上限なしの口を呼ぶ
    let b = inject(
        &bootstrap,
        "    status_with(agent, |binary| auth_state_bounded(agent, binary))\n",
        "    status_with(agent, |binary| (auth_state_for(agent, binary), None))\n",
        "①",
    );
    let line = line_of(
        &b,
        "    status_with(agent, |binary| (auth_state_for(agent, binary), None))",
    );
    let graph = graph_with(&[(SETUP_BOOTSTRAP, &b)]);
    expect_named(
        "① status_for_typed を上限なしへ戻す",
        &offenders(&graph),
        &format!("{SETUP_BOOTSTRAP}:{line}（fn status_for_typed）"),
    );

    // ② 修正前: status-all の A/B の門を外し、上限なしで 1 系統ずつ
    let b = inject(
        &bootstrap,
        "    if tako_core::probe::issue2942_legacy() {\n        return status_all_legacy();\n    }\n    status_all_within(tako_core::probe::auth_probe_timeout())\n",
        "    status_all_legacy()\n",
        "②",
    );
    let line = line_of(&b, "    status_all_legacy()");
    expect_named(
        "② status_all の門を外す",
        &offenders(&graph_with(&[(SETUP_BOOTSTRAP, &b)])),
        &format!("{SETUP_BOOTSTRAP}:{line}（fn status_all）"),
    );

    // ③ 修正前: `tako setup` の検出（CLI）が上限なしの口を呼ぶ
    let c = inject(
        &cli,
        "let (auth, timeout) = setup_bootstrap::auth_state_bounded(kind.install_kind(), &path);",
        "let auth = setup_bootstrap::auth_state_for(kind.install_kind(), &path);\n        \
         let timeout: Option<tako_core::probe::TimeoutNotice> = None;",
        "③",
    );
    let line = line_of(&c, "let auth = setup_bootstrap::auth_state_for(");
    expect_named(
        "③ CLI の検出を上限なしへ戻す",
        &offenders(&graph_with(&[(CLI_SETUP, &c)])),
        &format!("{CLI_SETUP}:{line}（fn detect_agents）"),
    );

    // ④ 上限なしの口を経ずに、認証の問い合わせを上限なしの門番で直書きする
    let b = inject(
        &bootstrap,
        "pub fn status_all_json() -> Value {\n",
        "pub fn status_all_json() -> Value {\n    let _ = crate::agent_probe::run(std::process::Command::new(\"claude\").args(auth_probe_argv(AgentKind::Claude)));\n",
        "④",
    );
    let line = line_of(
        &b,
        "let _ = crate::agent_probe::run(std::process::Command::new",
    );
    expect_named(
        "④ 認証の問い合わせの直書き",
        &offenders(&graph_with(&[(SETUP_BOOTSTRAP, &b)])),
        &format!("{SETUP_BOOTSTRAP}:{line}（fn status_all_json）"),
    );

    // ⑤ 上限つきの既定の口の門を、常に旧経路へ倒す（`if true {` は A/B の門ではない）
    let b = inject(
        &bootstrap,
        "    if tako_core::probe::issue2942_legacy() {\n        return (auth_state_for(agent, binary), None);\n    }\n",
        "    if true {\n        return (auth_state_for(agent, binary), None);\n    }\n",
        "⑤",
    );
    let line = line_of(&b, "        return (auth_state_for(agent, binary), None);");
    expect_named(
        "⑤ 門を常に旧経路へ倒す",
        &offenders(&graph_with(&[(SETUP_BOOTSTRAP, &b)])),
        &format!("{SETUP_BOOTSTRAP}:{line}（fn auth_state_bounded）"),
    );

    // ⑥ 修正前: 上限つきの門番が直接の子だけを kill する口で待つ（打ち切ると孫が残る）
    let probe = read(AGENT_PROBE);
    let p = inject(
        &probe,
        "    let outcome = if tako_core::probe::issue2942_legacy() {\n        tako_core::probe::output_with_timeout(program, args, budget)\n    } else {\n        tako_core::probe::output_with_timeout_grouped(program, args, budget)\n    };\n",
        "    let outcome = tako_core::probe::output_with_timeout(program, args, budget);\n",
        "⑥",
    );
    let line = line_of(
        &p,
        "    let outcome = tako_core::probe::output_with_timeout(program, args, budget);",
    );
    expect_named(
        "⑥ 門番を直接の子だけを kill する口へ戻す",
        &offenders(&graph_with(&[(AGENT_PROBE, &p)])),
        &format!("{AGENT_PROBE}:{line}（fn output_within）"),
    );
}

//! **#2255 の番犬**: 環境診断（`tako check-health` / `tako setup --check`）の経路から、
//! 上限なしで待つ問い合わせの口（`agent_probe::run`）へ届かないようにする。
//!
//! ## 何が起きていたのか
//!
//! 環境診断の正本 `diagnostics::collect` は、導入状況（`setup_bootstrap::status_all`）を通して
//! 認証の問い合わせ（`claude auth status --json` / `codex login status` / `agy models`）を
//! `agent_probe::run` = `Command::output()` で**上限なしに**待っていた。CLI が 1 つ返らないと
//! check-health が返らない（GUI 経由は CLI の応答の上限 300 秒の後、CLI 自身の診断が同じ問い合わせで
//! また止まる）。check-health は固まった GUI の
//! 切り分けに使う道具（#1441 / #2002）なので、それ自体が固まると切り分けができない。
//!
//! ## ここで止めるもの
//!
//! 1. [`診断の経路から上限なしの問い合わせへ届かない`] — `collect` から呼び出しを辿って届く関数
//!    （`common/call_graph.rs`。何段包んでも辿った先で見つかる）が `agent_probe::run` を呼んだら、
//!    その行を file:line で名指しし、`collect` からの道筋を添える。A/B の旧経路の腕
//!    （`if …issue2255_legacy() {`）はグラフが読まない
//! 2. [`診断の経路は上限つきの認証確認を通る`] — 禁止だけでなく**寄せ先も縛る**（#1496 の 2 本立て）。
//!    認証の確認ごと消しても 1 は緑なので、上限つきの口（`agent_probe::output_within`）へ届くこと・
//!    上限が `probe::auth_probe_timeout()` から来ることを見る
//! 3. [`修正前の形を名指しできる`] — 修正前の `status_all()` へ戻す・診断の関数へ直書きする・
//!    上限つきの関数を上限なしの口で包み直す、の 3 つの注入を 1 が名指しすること（同じテストの中の A/B）
//!
//! ## 相方
//!
//! 実経路（返らない偽の `claude auth status` で、修正前は固まり・修正後は数秒で「認証: 不明」を
//! 返す。CLI・`--json`・MCP）は `scripts/test-check-health-auth-timeout-2255.sh`。待ちそのものの挙動は
//! `tako_core::probe` の単体テスト。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const DIAGNOSTICS: &str = "crates/tako-control/src/diagnostics.rs";
const SETUP_BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const AGENT_PROBE: &str = "crates/tako-control/src/agent_probe.rs";

/// 走査する範囲。`agent_probe` は tako-control にあり、tako-core からは呼べないのでここだけで足りる
const GRAPH_ROOTS: &[&str] = &["crates/tako-control/src"];

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

/// [`GRAPH_ROOTS`] の `.rs` を全部（テスト領域を潰した本文と、その眺め）。読むのは 1 回だけ
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
        for dir in GRAPH_ROOTS {
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

/// `collect` から届く関数のうち、上限なしの口（`agent_probe::run`）を呼ぶもの。
///
/// 1 件 = 「呼んだ行（file:line と関数名）+ `collect` からの道筋」
fn unbounded_calls(graph: &call_graph::Graph) -> Vec<String> {
    let entry = one_fn(graph, DIAGNOSTICS, "collect");
    let gate = one_fn(graph, AGENT_PROBE, "run");
    let reach = graph.reach(&[entry], &|_, _| false);
    let mut reached: Vec<usize> = reach.keys().copied().filter(|&f| f != gate).collect();
    reached.sort();
    let mut out = Vec::new();
    for f in reached {
        if !graph.callees(f).contains(&gate) {
            continue;
        }
        let def = &graph.fns[f];
        let rel = &graph.sources[def.file].rel;
        // 別名で取り込んで呼んだ（`use crate::agent_probe::run as probe;`）ときは字面が無いので関数の行
        let lines = match graph.hits(f, "agent_probe::run(").as_slice() {
            [] => vec![def.line],
            hits => hits.to_vec(),
        };
        let route: String = graph
            .path_to(&reach, f)
            .windows(2)
            .map(|w| {
                let caller = &graph.sources[graph.fns[w[0].0].file].rel;
                format!(
                    " → `{}`（{caller}:{} で呼ぶ）",
                    graph.fns[w[1].0].name,
                    w[1].1.unwrap_or(0)
                )
            })
            .collect();
        for line in lines {
            out.push(format!(
                "{rel}:{line}（fn {}）が上限なしの `agent_probe::run` を呼ぶ。道筋: `collect`{route}",
                def.name
            ));
        }
    }
    out
}

/// 1: 診断の経路（`collect` から辿れる関数）は上限なしの問い合わせを呼ばない
#[test]
fn 診断の経路から上限なしの問い合わせへ届かない() {
    let hits = unbounded_calls(&graph_with(&[]));
    assert!(
        hits.is_empty(),
        "環境診断の経路から上限なしの問い合わせへ届く（#2255）:\n  {}\n\n\
         返らない CLI が 1 つあると `tako check-health` が返らなくなる。\n\
         診断の経路では上限つきの口（`agent_probe::output_within` / `output_with_timeout`）を通すこと。\n\
         認証の確認なら `setup_bootstrap::status_all_within` / `auth_state_within` がある",
        hits.join("\n  ")
    );
}

/// 2: 寄せ先も縛る。認証の確認は上限つきの口へ届き、上限は `auth_probe_timeout()` から来る
#[test]
fn 診断の経路は上限つきの認証確認を通る() {
    let graph = graph_with(&[]);
    let entry = one_fn(&graph, DIAGNOSTICS, "collect");
    let reach = graph.reach(&[entry], &|_, _| false);
    let bounded = one_fn(&graph, SETUP_BOOTSTRAP, "auth_state_within");
    assert!(
        reach.contains_key(&bounded),
        "{DIAGNOSTICS} の `collect` から `setup_bootstrap::auth_state_within` へ届かない（#2255）。\n\
         認証の確認を消したか、上限つきでない経路へ付け替えた"
    );
    let gate = one_fn(&graph, AGENT_PROBE, "output_within");
    assert!(
        graph.callees(bounded).contains(&gate),
        "{SETUP_BOOTSTRAP} の `auth_state_within` が上限つきの口 `agent_probe::output_within` を呼んでいない（#2255）"
    );
    // 上限なしの組み立て（setup の経路）は A/B の旧経路の腕からしか呼ばない
    let unbounded = one_fn(&graph, SETUP_BOOTSTRAP, "status_all");
    assert!(
        !reach.contains_key(&unbounded),
        "{DIAGNOSTICS} の `collect` から上限なしの `status_all` へ届く（#2255）。\n\
         呼んでよいのは `if tako_core::probe::issue2255_legacy() {{` の腕だけ"
    );
    let collect_hits = graph.hits(entry, "auth_probe_timeout()");
    assert!(
        !collect_hits.is_empty(),
        "{DIAGNOSTICS} の `collect` が上限を `tako_core::probe::auth_probe_timeout()` から取っていない（#2255）"
    );
    // 門番そのものが渡された上限で待ち、打ち切りを知らせる（素通しだと 1 は緑のまま症状が戻る）
    let def = &graph.fns[gate];
    let body = &graph.sources[def.file].code[def.body.0..=def.body.1];
    for needle in [
        "blocked()",
        "tako_core::probe::output_with_timeout(program, args, budget)",
        "timeout_notice()",
    ] {
        assert!(
            body.contains(needle),
            "{AGENT_PROBE} の `output_within` に `{needle}` が無い（#2255 / #1261 / #1503）"
        );
    }
}

/// 3: 修正前の形・直書き・包み直しの注入を、1 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    let diagnostics = read(DIAGNOSTICS);
    let bootstrap = read(SETUP_BOOTSTRAP);
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let expect_named = |label: &str, hits: Vec<String>, needle: &str| {
        assert!(
            hits.iter().any(|h| h.contains(needle)),
            "注入「{label}」を名指しできない（`{needle}` を含む行が無い）: {hits:#?}"
        );
        for h in &hits {
            eprintln!("注入「{label}」→ {h}");
        }
    };

    // ① 修正前: 診断が上限なしの `status_all()` を A/B のゲートの外で呼ぶ
    let d = inject(
        &diagnostics,
        "    let bootstrap = if tako_core::probe::issue2255_legacy() {\n        crate::setup_bootstrap::status_all()\n    } else {\n        crate::setup_bootstrap::status_all_within(tako_core::probe::auth_probe_timeout())\n    };",
        "    let bootstrap = crate::setup_bootstrap::status_all();",
        "①",
    );
    let hits = unbounded_calls(&graph_with(&[(DIAGNOSTICS, &d)]));
    expect_named(
        "① 修正前の status_all()",
        hits.clone(),
        &format!("{SETUP_BOOTSTRAP}:"),
    );
    expect_named(
        "① 道筋に status_all → status_for → auth_state_for",
        hits,
        "→ `status_all`",
    );

    // ② 診断の関数へ上限なしの口を直書きする
    let d = inject(
        &diagnostics,
        "fn agents_sync_item() -> Item {\n",
        "fn agents_sync_item() -> Item {\n    let _ = crate::agent_probe::run(&mut std::process::Command::new(\"claude\"));\n",
        "②",
    );
    let line = d
        .lines()
        .position(|l| l.contains("crate::agent_probe::run(&mut std::process::Command::new"))
        .map(|i| i + 1)
        .expect("注入した行");
    expect_named(
        "② 直書き",
        unbounded_calls(&graph_with(&[(DIAGNOSTICS, &d)])),
        &format!("{DIAGNOSTICS}:{line}（fn agents_sync_item）"),
    );

    // ③ 上限つきの入口を、上限なしの口で包み直す（何段包んでも辿った先で見つかる）
    let b = inject(
        &bootstrap,
        "status_with(agent, |binary| auth_state_within(agent, binary, budget))",
        "status_with(agent, |binary| (auth_state_for(agent, binary), None))",
        "③",
    );
    expect_named(
        "③ 包み直し",
        unbounded_calls(&graph_with(&[(SETUP_BOOTSTRAP, &b)])),
        "→ `status_for_within`",
    );
}

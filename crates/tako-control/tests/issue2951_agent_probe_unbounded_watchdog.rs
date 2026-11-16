//! **#2951 の番犬**: 上限なしの門番 `agent_probe::run`（`Command::output()` = 待ちに上限が無い）へ、
//! 製品の既定の経路から届かないようにする。
//!
//! ## 何が起きていたのか
//!
//! #2942 で認証の問い合わせ（`auth_state_for`）は上限つきへ寄せたが、別の問い合わせの呼び手が 2 つ
//! 上限なしの `agent_probe::run` のまま残っていた:
//!
//! - `agent_models::catalog`（`codex debug models` / `agy models`）: `tako setup models`・MCP
//!   `tako_setup_models`・`tako setup` のモデルの選択・設定画面のプロファイル。返らない codex / agy で
//!   CLI は返らず、MCP は応答の上限が無い（`default_reply_timeout` が None）ので答えない
//! - `stale_binary::extract_version_via_cli`（`claude --version`。パスにも版リソースにも版が無いときの
//!   最終手段）: MCP `tako_stale_binary`・GUI の背景の走査。返らない `claude --version` で背景のスレッドが戻らない
//!
//! 今はどちらも上限つきの門番 `agent_probe::output_with_timeout`（= `output_within`。#2942 で打ち切ると
//! 孫まで止まる）を通り、上限なしの形は A/B の旧経路の腕（`if crate::agent_probe::issue2951_legacy() {`）にだけ残る。
//!
//! ## ここで止めるもの
//!
//! 1. [`上限なしの門番はabの旧経路の腕からしか呼ばない`] — 本番コード（4 クレートの `src/`）の関数が
//!    `agent_probe::run(` を A/B の旧経路の腕の外から呼んだら、その行を file:line で名指す（どのスレッドで
//!    待っても呼び手は固まるので、別スレッドへ渡すクロージャの中も見る = `call_graph::Graph::product_ranges`）。
//!    例外は [`ALLOWED`]（関数ごと A/B の腕からしか呼ばれない口。呼び手は別の番犬が縛る）だけ。
//!    別名での取り込み（`use crate::agent_probe::run` / `agent_probe::{run…}`）も名指す（字面で辿れなくなる）
//! 2. [`問い合わせの口は上限つきの門番を通る`] — 禁止だけでなく寄せ先も縛る（#1496 の 2 本立て）。
//!    `catalog` / `extract_version_via_cli` が上限つきの門番（`agent_probe::output_with_timeout(`）を呼び、
//!    A/B の腕（`issue2951_legacy()`）を持つこと
//! 3. [`修正前の形を名指しできる`] — 修正前の形（上限なしの門番を直に呼ぶ）の注入を 1 / 2 が名指しすること
//!
//! ## 相方
//!
//! 実経路（返らない偽の codex / agy / claude で `tako setup models`・MCP `tako_setup_models`・`tako_stale_binary` が
//! 上限の秒数で返り、偽物の子と孫が残らない）は `scripts/test-agent-probe-bounded-2951.sh`。
//! 打ち切りの形（`failure.kind = timed_out`）は `agent_models` の単体テスト。認証の口の上限なしは
//! `issue2942_bootstrap_auth_probe_watchdog.rs`、環境診断からの到達は `issue2255_diagnostics_auth_probe_watchdog.rs`。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const AGENT_MODELS: &str = "crates/tako-control/src/agent_models.rs";
const STALE_BINARY: &str = "crates/tako-control/src/stale_binary.rs";
const SETUP_BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const AGENT_PROBE: &str = "crates/tako-control/src/agent_probe.rs";

/// 走査する範囲（本番コードを持つ 4 クレート全部。`poc/` は品質基準の対象外）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 関数ごと A/B の旧経路の腕からしか呼ばれない口（`(ファイル, 関数, 理由)`）。本体で `agent_probe::run` を呼んでよい
const ALLOWED: &[(&str, &str, &str)] = &[(
    SETUP_BOOTSTRAP,
    "auth_state_for",
    "#2942 の上限なしの認証の口。呼ぶのは `TAKO_2942_LEGACY` / `TAKO_2255_LEGACY` の腕だけで、\
     腕の外の呼び手は `issue2942_bootstrap_auth_probe_watchdog.rs` が名指す",
)];

/// 上限つきへ寄せた問い合わせの口（`(ファイル, 関数, 起こすコマンド)`）
const BOUNDED: &[(&str, &str, &str)] = &[
    (
        AGENT_MODELS,
        "catalog",
        "`codex debug models` / `agy models`（`tako setup models`・MCP `tako_setup_models`）",
    ),
    (
        STALE_BINARY,
        "extract_version_via_cli",
        "`claude --version`（MCP `tako_stale_binary`・GUI の背景の走査）",
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

/// 本番の `rel` の本文（テスト領域を潰したもの = 注入の元）
fn text_of(rel: &str) -> &'static str {
    graph_sources()
        .iter()
        .find(|(_, source)| source.rel == rel)
        .map(|(text, _)| text.as_str())
        .unwrap_or_else(|| panic!("{rel} が走査の範囲にある"))
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

/// ファイル `rel` の関数 `name`（ちょうど 1 つでなければ理由つきで落ちる）
fn one_fn(graph: &call_graph::Graph, rel: &str, name: &str) -> usize {
    match graph.find(rel, name).as_slice() {
        [f] => *f,
        [] => panic!("{rel} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）"),
        many => panic!("{rel} に `fn {name}` が {} 個ある", many.len()),
    }
}

/// 1: 製品の既定の経路（A/B の旧経路の腕の外）で `agent_probe::run` を呼ぶ行
fn offenders(graph: &call_graph::Graph) -> Vec<String> {
    let allowed: Vec<usize> = ALLOWED
        .iter()
        .map(|(rel, name, _)| one_fn(graph, rel, name))
        .collect();
    let mut out = Vec::new();
    for (f, def) in graph.fns.iter().enumerate() {
        let source = &graph.sources[def.file];
        if allowed.contains(&f) || source.rel == AGENT_PROBE {
            continue;
        }
        for (s, e) in graph.product_ranges(f) {
            for (i, _) in source.code[s..e].match_indices("agent_probe::run(") {
                out.push(format!(
                    "{}:{}（fn {}）が上限なしの門番 `agent_probe::run` を A/B の旧経路の腕\
                     （`if …legacy() {{`）の外から呼ぶ",
                    source.rel,
                    source.line_of(s + i),
                    def.name
                ));
            }
        }
    }
    // 別名で取り込むと字面で追えなくなる（`use crate::agent_probe::run as probe;` など）
    for source in &graph.sources {
        for needle in ["agent_probe::run;", "agent_probe::run as", "agent_probe::{"] {
            for (i, _) in source.code.match_indices(needle) {
                let line_start = source.code[..i].rfind('\n').map_or(0, |p| p + 1);
                if !source.code[line_start..i].contains("use ") {
                    continue;
                }
                out.push(format!(
                    "{}:{} が `agent_probe` の門番を別名で取り込む（`{needle}`）。`crate::agent_probe::…` の\
                     完全な名前で呼ぶこと（番犬が字面で上限なしの呼び手を名指せなくなる）",
                    source.rel,
                    source.line_of(i)
                ));
            }
        }
    }
    out
}

/// 2: 上限つきへ寄せた口が上限つきの門番を呼び、A/B の腕を持つ（本文の字面で見る）
fn target_problems(graph: &call_graph::Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, name, what) in BOUNDED {
        let f = one_fn(graph, rel, name);
        let def = &graph.fns[f];
        let source = &graph.sources[def.file];
        let live: String = graph
            .product_ranges(f)
            .iter()
            .map(|&(s, e)| &source.code[s..e])
            .collect();
        if !live.contains("agent_probe::output_with_timeout(") {
            out.push(format!(
                "{rel}:{}（fn {name}）が上限つきの門番 `agent_probe::output_with_timeout` を製品の既定の経路で呼ばない。\
                 {what} が返らない CLI で固まる（#2951）",
                def.line
            ));
        }
        let body = &source.code[def.body.0..=def.body.1];
        if !body.contains("agent_probe::issue2951_legacy()") {
            out.push(format!(
                "{rel}:{}（fn {name}）に A/B の門（`crate::agent_probe::issue2951_legacy()`）が無い（#2951）",
                def.line
            ));
        }
    }
    out
}

#[test]
fn 上限なしの門番はabの旧経路の腕からしか呼ばない() {
    let graph = graph_with(&[]);
    let hits = offenders(&graph);
    assert!(
        hits.is_empty(),
        "エージェント CLI への問い合わせを上限なしで待つ形が製品の既定の経路にある（#2951）:\n  {}\n\n\
         返らない claude / codex / agy が 1 つあると呼び手ごと固まる。`agent_probe::output_with_timeout`\n\
         （setup の上限）か `agent_probe::output_within`（上限を呼び手が決める）で聞き、打ち切りを呼び手へ返すこと",
        hits.join("\n  ")
    );
}

#[test]
fn 問い合わせの口は上限つきの門番を通る() {
    let graph = graph_with(&[]);
    let problems = target_problems(&graph);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    // 上限つきの門番は上限の正本（`probe::probe_timeout()`）で `output_within` を呼ぶ
    // （`output_within` が孫まで止める口で待つことは #2942 の番犬が縛る）
    let gate = one_fn(&graph, AGENT_PROBE, "output_with_timeout");
    let within = one_fn(&graph, AGENT_PROBE, "output_within");
    assert!(
        graph.callees(gate).contains(&within),
        "{AGENT_PROBE} の `output_with_timeout` が `output_within` を呼んでいない（番犬の前提がずれた）"
    );
    let def = &graph.fns[gate];
    let body = &graph.sources[def.file].code[def.body.0..=def.body.1];
    assert!(
        body.contains("probe_timeout()"),
        "{AGENT_PROBE} の `output_with_timeout` の上限が `probe::probe_timeout()` から来ていない"
    );
}

#[test]
fn 走査が空振りしていない() {
    let graph = graph_with(&[]);
    // A/B の腕の中の上限なしの呼び出しは今もある（腕を消したなら A/B も消えている = 番犬と仕様も追う）。
    // 生きた範囲には無く、本体には在る = 読み方（A/B の腕を除く）が効いている
    for (rel, name, _) in BOUNDED {
        let f = one_fn(&graph, rel, name);
        let def = &graph.fns[f];
        let source = &graph.sources[def.file];
        let body = &source.code[def.body.0..=def.body.1];
        assert!(
            body.contains("agent_probe::run("),
            "{rel} の `{name}` の A/B の腕に上限なしの `agent_probe::run(` が無い（A/B を消したなら番犬も追うこと）"
        );
    }
    // 例外の口は今も上限なしの門番を呼ぶ（呼ばなくなったら ALLOWED から外す）
    let run = one_fn(&graph, AGENT_PROBE, "run");
    for (rel, name, _) in ALLOWED {
        let f = one_fn(&graph, rel, name);
        assert!(
            graph.callees(f).contains(&run),
            "{rel} の `{name}` が `agent_probe::run` を呼んでいない（ALLOWED から外すこと）"
        );
    }
}

/// 3: 修正前の形を 1 / 2 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    // 3-1: モデル一覧を修正前と同じく上限なしの門番で待つ
    let text = text_of(AGENT_MODELS);
    let bounded = "RunOutcome::from(crate::agent_probe::output_with_timeout(path, args))";
    assert!(
        text.contains(bounded),
        "注入の元が {AGENT_MODELS} に無い（形を変えたなら注入も追うこと）"
    );
    let unbounded = "RunOutcome::from(crate::agent_probe::run(std::process::Command::new(path).args(args)).map(|o| CommandRun { success: o.status.success(), stdout: String::new(), stderr: String::new() }))";
    let injected = text.replacen(bounded, unbounded, 1);
    let line = injected[..injected.find(unbounded).unwrap()]
        .matches('\n')
        .count()
        + 1;
    let graph = graph_with(&[(AGENT_MODELS, &injected)]);
    let hits = offenders(&graph);
    let want = format!("{AGENT_MODELS}:{line}（fn catalog）");
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "モデル一覧を上限なしの門番へ戻した注入が `{want}` で名指しされない: {hits:#?}"
    );
    assert!(
        target_problems(&graph)
            .iter()
            .any(|p| p.contains("（fn catalog）が上限つきの門番")),
        "モデル一覧を上限なしの門番へ戻した注入を 2 が名指ししない"
    );

    // 3-2: 版の読み取りの A/B の門を外す（上限なしの腕が既定の経路になる）
    let text = text_of(STALE_BINARY);
    let gate = "let output = if crate::agent_probe::issue2951_legacy() {";
    assert!(
        text.contains(gate),
        "注入の元が {STALE_BINARY} に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(gate, "let output = if true {", 1);
    let line = injected[..injected.find("let output = if true {").unwrap()]
        .matches('\n')
        .count()
        + 2;
    let graph = graph_with(&[(STALE_BINARY, &injected)]);
    let hits = offenders(&graph);
    let want = format!("{STALE_BINARY}:{line}（fn extract_version_via_cli）");
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "版の読み取りの A/B の門を外した注入が `{want}` で名指しされない: {hits:#?}"
    );
    assert!(
        target_problems(&graph)
            .iter()
            .any(|p| p.contains("（fn extract_version_via_cli）に A/B の門")),
        "版の読み取りの A/B の門を外した注入を 2 が名指ししない"
    );

    // 3-3: 別名で取り込む
    let text = text_of(STALE_BINARY);
    let injected = format!("use crate::agent_probe::run as probe_run;\n{text}");
    let graph = graph_with(&[(STALE_BINARY, &injected)]);
    assert!(
        offenders(&graph).iter().any(|h| h.starts_with(&format!(
            "{STALE_BINARY}:1 が `agent_probe` の門番を別名で取り込む"
        ))),
        "門番の別名の取り込みを名指ししない"
    );
}

//! **#2967 / #2973 の番犬**: MCP の登録（`tako_setup_mcp` / `tako setup-mcp`）とリモートフォルダ
//! （`tako_remote_folder`）が起こす外部コマンドを、上限なしの `Command::output()` /
//! `Child::wait_with_output()` で待つ形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! - #2967: `dispatch::setup_mcp_via_cli`（`claude mcp remove` / `claude mcp add`）と `agent_mcp::run`
//!   （`codex mcp list` / `codex mcp add` / `agy mcp add`）が `.output()` で待っていた。#2961 で GUI の
//!   UI スレッドは止まらなくなったが、返らない CLI の間は MCP の応答が返らず、setup の書き込みの錠
//!   （`SETUP_WRITE_SERIAL`）を持ち続けた。CLI の `tako setup-mcp` は固まった
//! - #2973: `remote_fs::spawn_capture`（ControlMaster を張る ssh・`ssh -O check` / `-O exit`・sftp のバッチ）が
//!   `wait_with_output()` で待っていた。ssh の `ConnectTimeout` / keepalive は相手が黙る形にしか効かず、
//!   クライアント自身が返らない形（ProxyCommand・認証エージェント・多重化の master）では返らなかった
//!
//! 今は前者が上限つきの門番 `agent_probe::command_with_timeout`、後者が待ちの 1 実装
//! `probe::wait_with_timeout`（どちらも打ち切ると孫まで止める）を通り、上限なしの形は A/B の旧経路の腕
//! （`if …issue2967_legacy() {` / `if issue2973_legacy() {`）にだけ残る。
//!
//! ## ここで止めるもの
//!
//! 1. [`入口から上限なしの待ちへ届かない`] — 入口（[`ENTRIES`]）から呼び出しのグラフで辿れる関数の、製品の
//!    既定の経路（A/B の旧経路の腕の外。別スレッドへ渡すクロージャも含む = `product_ranges`）に
//!    `.output()` / `.wait_with_output()` があれば、その行を file:line と入口からの道筋で名指す。
//!    待ちを足した場所が入口から遠くても届く（#2951 の番犬は `agent_probe::run` の呼び手しか見ない）
//! 2. [`待つ口は上限つきの1実装を通る`] — 禁止だけでなく寄せ先も縛る（#1496 の 2 本立て）
//! 3. [`修正前の形を名指しできる`] — 修正前の形（A/B の門を外す・上限なしの待ちを書き足す）の注入を
//!    1 / 2 が file:line で名指すこと
//!
//! ## 相方
//!
//! 実経路（返らない偽の claude / codex / agy / ssh / sftp で MCP と CLI が上限で返り、偽物の子と孫が残らない）は
//! `scripts/test-bounded-mcp-ssh-2967.sh`。打ち切りの文（日英）は `issue1762_dispatch_error_lang.rs`、
//! 上限の算出は `remote_fs` の単体テスト。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const AGENT_MCP: &str = "crates/tako-control/src/agent_mcp.rs";
const AGENT_PROBE: &str = "crates/tako-control/src/agent_probe.rs";
const REMOTE_FS: &str = "crates/tako-core/src/remote_fs.rs";
const SETUP_CLI: &str = "crates/tako-cli/src/setup.rs";

/// 走査する範囲（本番コードを持つ 4 クレート全部。`poc/` は品質基準の対象外）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 入口（`(ファイル, 関数, 利用者の口)`）。ここから辿れる先が外部コマンドを上限なしで待ってはいけない
const ENTRIES: &[(&str, &str, &str)] = &[
    (
        DISPATCH,
        "setup_mcp_agents",
        "MCP `tako_setup_mcp`・CLI `tako setup-mcp`・設定画面の「MCP 登録」",
    ),
    (
        SETUP_CLI,
        "run_setup_mcp",
        "`tako setup` の claude への MCP 登録",
    ),
    (
        AGENT_MCP,
        "register",
        "`tako setup` の codex / agy への MCP 登録",
    ),
    (
        AGENT_MCP,
        "state",
        "`tako setup` の計画・環境診断（`codex mcp list`）",
    ),
    (
        DISPATCH,
        "remote_folder_run",
        "MCP `tako_remote_folder` の open / ls / open-file / push",
    ),
    (
        DISPATCH,
        "remote_folder_list_finish",
        "MCP `tako_remote_folder` の list（`ssh -O check`）",
    ),
    (
        DISPATCH,
        "remote_folder_close_masters",
        "MCP `tako_remote_folder` の close（`ssh -O exit`）",
    ),
    (
        REMOTE_FS,
        "liveness",
        "GUI の自動再接続（#1040）の生死の判定",
    ),
];

/// 上限なしの待ち（`std::process` の 2 つ）
const UNBOUNDED: &[&str] = &[".output()", ".wait_with_output()"];

/// 上限つきへ寄せた口（`(ファイル, 関数, 製品の経路で呼ぶべきもの, A/B の門)`）
const BOUNDED: &[(&str, &str, &str, &str)] = &[
    (
        DISPATCH,
        "claude_mcp_output",
        "agent_probe::command_with_timeout(",
        "agent_probe::issue2967_legacy()",
    ),
    (
        AGENT_MCP,
        "run",
        "agent_probe::command_with_timeout(",
        "agent_probe::issue2967_legacy()",
    ),
    (
        REMOTE_FS,
        "spawn_capture",
        "probe::wait_with_timeout(",
        "issue2973_legacy()",
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

/// 道筋の表示（`入口 → … → 関数`）
fn route(graph: &call_graph::Graph, steps: &[(usize, Option<usize>)]) -> String {
    steps
        .iter()
        .map(|(f, _)| format!("`{}`", graph.fns[*f].name))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// 1: 入口から辿れる関数の製品の既定の経路にある上限なしの待ち
fn offenders(graph: &call_graph::Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, name, door) in ENTRIES {
        let entry = one_fn(graph, rel, name);
        let reach = graph.reach(&[entry], &|_, _| false);
        let mut reached: Vec<usize> = reach.keys().copied().collect();
        reached.sort();
        for f in reached {
            let def = &graph.fns[f];
            let source = &graph.sources[def.file];
            for (s, e) in graph.product_ranges(f) {
                for needle in UNBOUNDED {
                    for (i, _) in source.code[s..e].match_indices(needle) {
                        out.push(format!(
                            "{}:{}（fn {}）が外部コマンドを上限なしの `{needle}` で待つ（A/B の旧経路の腕の外）。\
                             入口 {rel} の `{name}`（{door}）から届く: {}",
                            source.rel,
                            source.line_of(s + i),
                            def.name,
                            route(graph, &graph.path_to(&reach, f)),
                        ));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 2: 上限つきへ寄せた口が上限つきの待ちを製品の経路で呼び、A/B の門を持つ
fn target_problems(graph: &call_graph::Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, name, bounded, gate) in BOUNDED {
        let f = one_fn(graph, rel, name);
        let def = &graph.fns[f];
        let source = &graph.sources[def.file];
        let live: String = graph
            .product_ranges(f)
            .iter()
            .map(|&(s, e)| &source.code[s..e])
            .collect();
        if !live.contains(bounded) {
            out.push(format!(
                "{rel}:{}（fn {name}）が上限つきの待ち `{bounded}…)` を製品の既定の経路で呼ばない（#2967 / #2973）",
                def.line
            ));
        }
        let body = &source.code[def.body.0..=def.body.1];
        if !body.contains(gate) {
            out.push(format!(
                "{rel}:{}（fn {name}）に A/B の門（`{gate}`）が無い（#2967 / #2973）",
                def.line
            ));
        }
    }
    out
}

#[test]
fn 入口から上限なしの待ちへ届かない() {
    let graph = graph_with(&[]);
    let hits = offenders(&graph);
    assert!(
        hits.is_empty(),
        "MCP の登録・リモートフォルダの経路に、外部コマンドを上限なしで待つ形がある（#2967 / #2973）:\n  {}\n\n\
         返らない CLI（claude / codex / agy / ssh / sftp）が 1 つあると、応答と setup の書き込みの錠が止まる。\n\
         エージェント CLI は `agent_probe::command_with_timeout`（上限 `probe_timeout`）、ssh / sftp は\n\
         `probe::wait_with_timeout`（`remote_fs::spawn_capture`）で待ち、打ち切りを呼び手へ返すこと",
        hits.join("\n  ")
    );
}

#[test]
fn 待つ口は上限つきの1実装を通る() {
    let graph = graph_with(&[]);
    let problems = target_problems(&graph);
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // 門番は上限の正本（`probe::probe_timeout()`）で、孫まで止める口（グループを率いさせる待ち）を使う
    let gate = one_fn(&graph, AGENT_PROBE, "command_with_timeout");
    let def = &graph.fns[gate];
    let body = &graph.sources[def.file].code[def.body.0..=def.body.1];
    for (needle, why) in [
        (
            "probe::group_output_with_timeout(",
            "打ち切ると孫まで止める待ち（#2942）",
        ),
        ("probe::probe_timeout()", "上限の正本"),
        (
            "blocked()",
            "テストプロセスから実 CLI を起こさない門番（#1261）",
        ),
    ] {
        assert!(
            body.contains(needle),
            "{AGENT_PROBE} の `command_with_timeout` が `{needle}`（{why}）を通らない"
        );
    }
    // ssh / sftp は子にグループを率いさせてから待つ（ProxyCommand・ラッパーの孫まで止める）
    let spawn = one_fn(&graph, REMOTE_FS, "spawn_capture");
    let def = &graph.fns[spawn];
    let source = &graph.sources[def.file];
    let live: String = graph
        .product_ranges(spawn)
        .iter()
        .map(|&(s, e)| &source.code[s..e])
        .collect();
    for needle in ["lead_own_group(", "probe::spawn_for_wait("] {
        assert!(
            live.contains(needle),
            "{REMOTE_FS} の `spawn_capture` が製品の経路で `{needle}` を通らない（打ち切りで孫が残る / 入れ物に入らない）"
        );
    }
}

#[test]
fn 走査が空振りしていない() {
    let graph = graph_with(&[]);
    // A/B の腕の中の上限なしの待ちは今もある（腕を消したなら A/B も消えている = 番犬と仕様も追う）。
    // 生きた範囲には無く、本体には在る = 読み方（A/B の腕を除く）が効いている
    for (rel, name, _, _) in BOUNDED {
        let f = one_fn(&graph, rel, name);
        let def = &graph.fns[f];
        let body = &graph.sources[def.file].code[def.body.0..=def.body.1];
        assert!(
            UNBOUNDED.iter().any(|n| body.contains(n)),
            "{rel} の `{name}` の A/B の腕に上限なしの待ちが無い（A/B を消したなら番犬も追うこと）"
        );
    }
    // 入口から寄せた口まで辿れている（辿れないと 1 は何も見ずに緑になる）
    for (entry_rel, entry_name, target_rel, target_name) in [
        (DISPATCH, "setup_mcp_agents", DISPATCH, "claude_mcp_output"),
        (DISPATCH, "setup_mcp_agents", AGENT_MCP, "run"),
        (AGENT_MCP, "state", AGENT_MCP, "run"),
        (SETUP_CLI, "run_setup_mcp", DISPATCH, "claude_mcp_output"),
        (DISPATCH, "remote_folder_run", REMOTE_FS, "spawn_capture"),
        (
            DISPATCH,
            "remote_folder_list_finish",
            REMOTE_FS,
            "spawn_capture",
        ),
        (
            DISPATCH,
            "remote_folder_close_masters",
            REMOTE_FS,
            "spawn_capture",
        ),
    ] {
        let entry = one_fn(&graph, entry_rel, entry_name);
        let target = one_fn(&graph, target_rel, target_name);
        let reach = graph.reach(&[entry], &|_, _| false);
        assert!(
            reach.contains_key(&target),
            "{entry_rel} の `{entry_name}` から {target_rel} の `{target_name}` へ辿れない（呼び出しの形が変わった？）"
        );
    }
}

/// 注入した本文で `needle` が現れる行（1 始まり。最初の 1 つ）
fn line_of(text: &str, needle: &str) -> usize {
    text[..text
        .find(needle)
        .unwrap_or_else(|| panic!("注入した本文に `{needle}` が無い"))]
        .matches('\n')
        .count()
        + 1
}

/// 3: 修正前の形を 1 / 2 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    // 3-1: codex / agy の待ちの A/B の門を外す（上限なしの `.output()` が既定の経路になる）
    let text = text_of(AGENT_MCP);
    let gate = "    if crate::agent_probe::issue2967_legacy() {\n        return cmd.output()";
    assert!(
        text.contains(gate),
        "注入の元が {AGENT_MCP} に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(gate, "    if true {\n        return cmd.output()", 1);
    let line = line_of(&injected, "    if true {\n        return cmd.output()") + 1;
    let graph = graph_with(&[(AGENT_MCP, &injected)]);
    let want = format!("{AGENT_MCP}:{line}（fn run）が外部コマンドを上限なしの `.output()` で待つ");
    let hits = offenders(&graph);
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "codex / agy の待ちの A/B の門を外した注入が `{want}` で名指しされない: {hits:#?}"
    );
    assert!(
        target_problems(&graph)
            .iter()
            .any(|p| p.contains("（fn run）に A/B の門")),
        "codex / agy の待ちの A/B の門を外した注入を 2 が名指ししない"
    );

    // 3-2: ssh / sftp の待ちの A/B の門を外す
    let text = text_of(REMOTE_FS);
    let gate = "    if issue2973_legacy() {\n";
    assert!(
        text.contains(gate),
        "注入の元が {REMOTE_FS} に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(gate, "    if true {\n", 1);
    let line = line_of(&injected, "child.wait_with_output()");
    let graph = graph_with(&[(REMOTE_FS, &injected)]);
    let want = format!(
        "{REMOTE_FS}:{line}（fn spawn_capture）が外部コマンドを上限なしの `.wait_with_output()` で待つ"
    );
    let hits = offenders(&graph);
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "ssh / sftp の待ちの A/B の門を外した注入が `{want}` で名指しされない: {hits:#?}"
    );
    assert!(
        target_problems(&graph)
            .iter()
            .any(|p| p.contains("（fn spawn_capture）に A/B の門")),
        "ssh / sftp の待ちの A/B の門を外した注入を 2 が名指ししない"
    );

    // 3-3: claude の `mcp add` を修正前の上限なしの `.output()` へ戻す（入口から 2 段先の関数）
    let text = text_of(DISPATCH);
    let bounded = "let output = claude_mcp_output(&mut cmd, \"add\")?;";
    assert!(
        text.contains(bounded),
        "注入の元が {DISPATCH} に無い（形を変えたなら注入も追うこと）"
    );
    let unbounded =
        "let output = cmd.output().map_err(|e| SetupRefusal::ClaudeMcpAddSpawn(e.to_string()))?;";
    let injected = text.replacen(bounded, unbounded, 1);
    let line = line_of(&injected, unbounded);
    let graph = graph_with(&[(DISPATCH, &injected)]);
    let want = format!(
        "{DISPATCH}:{line}（fn setup_mcp_via_cli）が外部コマンドを上限なしの `.output()` で待つ"
    );
    let hits = offenders(&graph);
    assert!(
        hits.iter().any(|h| h.starts_with(&want)
            && h.contains("`setup_mcp_agents` → `setup_mcp_typed` → `setup_mcp_via_cli`")),
        "claude の `mcp add` を上限なしへ戻した注入が `{want}` と道筋で名指しされない: {hits:#?}"
    );

    // 3-4: ssh / sftp の待ちを上限つきの 1 実装から外す（2 が名指す）
    let text = text_of(REMOTE_FS);
    let bounded = "match crate::probe::wait_with_timeout(spawned, label, budget) {";
    assert!(
        text.contains(bounded),
        "注入の元が {REMOTE_FS} に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(
        bounded,
        "match crate::probe::Outcome::Failed { label: String::new(), reason: String::new() } {",
        1,
    );
    let graph = graph_with(&[(REMOTE_FS, &injected)]);
    assert!(
        target_problems(&graph)
            .iter()
            .any(|p| p.contains("（fn spawn_capture）が上限つきの待ち")),
        "ssh / sftp の待ちを上限つきの 1 実装から外した注入を 2 が名指ししない"
    );
}

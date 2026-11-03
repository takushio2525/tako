//! **#2694 の番犬**: 隔離の印（`TAKO_ISOLATED=1`）で `TAKO_DATA_DIR` を付けない CLI が、手元の処理で
//! 既定（本番）の data dir を読み書きする形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! #2531 が断るのは GUI への接続（発見ファイルの読み）だけで、CLI の中で完結する処理（`tako usage` が
//! 使用量の索引を読み進める・`tako migrate` が設定を書き換える ほか）は隔離の印のまま
//! `paths::data_dir()` の既定 = 本番の置き場で動いた（#2685 の検証で本番の索引へ実際に追記が入った）。
//!
//! いまは置き場を解決する 1 実装（`paths::data_dir` / `local_data_dir`）が、印があって `TAKO_DATA_DIR` が
//! 無ければ既定の置き場を返さない（判定の正本は `paths::data_dir_refusal`）。CLI の手元のサブコマンドは
//! 入口で同じ判定を引いて日英で断り、終了コード 1 で返す（`local_data_gate`）。例外は順番待ちの CLI だけ
//! （`paths::allow_machine_wide_data_dir`。#2531 の `read_candidates_machine_wide` と同じ扱い）。
//!
//! ## ここで止めるもの
//!
//! 1. [`置き場の解決は既定の置き場より先に門を通る`] — `data_dir` / `local_data_dir` の本体で、門
//!    （`data_dir_refusal()`）が既定の置き場（`default_data_dir()` / `default_local_data_dir()`）より先に在る
//! 2. [`既定の置き場を門の外で組まない`] — 製品コードで既定の置き場を組む関数（[`DEFAULT_RESOLVERS`]）を
//!    呼んでよいのは [`DEFAULT_CALLERS`]（理由つき）だけ。新しい呼び手は file:line で名指す
//! 3. [`手元のサブコマンドは入口で門を通る`] — CLI の `cli_main` が振り分けるハンドラのうち、呼び出しを
//!    辿って（`common/call_graph.rs`）置き場の解決へ届くものは、入口の門 `local_data_gate` を呼ぶ
//!    （例外は順番待ちの 2 つで、こちらは例外の口を呼ぶ）。表示言語の読み（[`LANG_READERS`]）と診断ログの
//!    書き手（[`DIAG_WRITERS`]）は辿らない（置き場が決まらなければ既定の言語へ落ちる・記録しないだけで、
//!    利用者へ返す結果を変えない）。門を通さないハンドラは [`NO_GATE`] に理由つきで載せる
//! 4. [`例外の口の呼び手は順番待ちだけ`] — `allow_machine_wide_data_dir` の呼び手は [`QUEUE_HANDLERS`] だけ
//! 5. [`修正前の形を名指しできる`] — 門を外す・新しい手元のハンドラが門を通らない・既定の置き場を
//!    直に組む、の 3 つの注入を 1〜3 が file:line で名指しすること
//!
//! ## 相方
//!
//! 実経路（`HOME` を一時 dir へ向けて「既定の置き場」自体を砂場にし、修正前は断らずに書く・修正後は
//! 断って 1 バイトも書かない・`TAKO_DATA_DIR` を付ければ従来どおり・`tako queue` は従来どおり）は
//! `bash scripts/test-isolated-data-dir-2694.sh`。判定の純関数は `tako_core::paths::issue2694_tests`。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const PATHS: &str = "crates/tako-core/src/paths.rs";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";
const SETTINGS: &str = "crates/tako-control/src/settings.rs";

/// 入口の門（CLI）
const GATE: &str = "local_data_gate";
/// 例外の口（順番待ち）
const MACHINE_WIDE: &str = "allow_machine_wide_data_dir";

/// 置き場の解決（ここへ届く = 手元で data dir を読み書きする）
const RESOLVERS: &[(&str, &str)] = &[(PATHS, "data_dir"), (PATHS, "local_data_dir")];

/// 既定（本番）の置き場を組む関数
const DEFAULT_RESOLVERS: &[(&str, &str)] = &[
    (PATHS, "default_data_dir"),
    (PATHS, "default_local_data_dir"),
    (PATHS, "default_data_dir_under_home"),
];

/// 既定の置き場を組む関数を呼んでよい関数（理由つき）
const DEFAULT_CALLERS: &[(&str, &str, &str)] = &[
    (
        PATHS,
        "data_dir",
        "置き場の解決の正本。門（data_dir_refusal）を先に通る",
    ),
    (
        PATHS,
        "local_data_dir",
        "Windows の LOCALAPPDATA の解決。門を先に通る",
    ),
    (
        PATHS,
        "default_local_data_dir",
        "Windows 以外は data dir と同じ置き場（呼び手は local_data_dir だけ = 門の内側）",
    ),
    (
        PATHS,
        "default_data_dir",
        "HOME から組む規則の 1 実装（default_data_dir_under_home）を引く",
    ),
];

/// 例外の口を呼んでよいハンドラ（順番待ち。#2531 と同じ扱い）
const QUEUE_HANDLERS: &[(&str, &str)] = &[(CLI_MAIN, "queue_cli"), (CLI_MAIN, "queue_run_cli")];

/// 辿らない表示言語の読み（読めなければ既定の言語へ落ちるだけ）
const LANG_READERS: &[(&str, &str)] = &[
    (SETTINGS, "client_lang"),
    (SETTINGS, "peek_lang_setting"),
    (CLI_MAIN, "init_local_lang"),
    (CLI_MAIN, "cli_lang"),
    (CLI_MAIN, "local_dispatch_err"),
];

/// 辿らない診断ログの書き手（置き場が決まらなければ書かないだけで、利用者へ返す結果を変えない）
const DIAG_WRITERS: &[(&str, &str)] = &[("crates/tako-control/src/diag.rs", "persist_log")];

/// 置き場の解決へ届いても入口の門を通さないハンドラ（理由つき）
const NO_GATE: &[(&str, &str, &str)] = &[
    (
        CLI_MAIN,
        "check_health_cli",
        "環境の診断（#1441）。置き場が決まらなければ決まらない（resolved: false）と答えるだけで書かない。\
         断ると切り分けの道具が無くなる",
    ),
    (
        "crates/tako-cli/src/mcp_bridge.rs",
        "serve",
        "MCP の stdio ブリッジ = GUI への中継（GUI への接続は #2531 が断る）。手元で走る道具は置き場が\
         決まらなければ道具ごとの断り（データディレクトリを解決できない）を返し、書かない",
    ),
];

/// 走査する範囲（置き場の解決は tako-core、手元の処理は tako-control、振り分けは tako-cli）
const GRAPH_ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
];

/// 既定の置き場の呼び手を探す範囲（GUI も含む製品コード全部）
const PRODUCT_ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
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

fn sources_under(roots: &[&str]) -> Vec<(String, call_graph::Source)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
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
    for dir in roots {
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
}

/// [`GRAPH_ROOTS`] の `.rs` を全部（読むのは 1 回だけ）
fn graph_sources() -> &'static [(String, call_graph::Source)] {
    static CACHE: OnceLock<Vec<(String, call_graph::Source)>> = OnceLock::new();
    CACHE.get_or_init(|| sources_under(GRAPH_ROOTS))
}

/// [`PRODUCT_ROOTS`] の `.rs` を全部（読むのは 1 回だけ）
fn product_sources() -> &'static [(String, call_graph::Source)] {
    static CACHE: OnceLock<Vec<(String, call_graph::Source)>> = OnceLock::new();
    CACHE.get_or_init(|| sources_under(PRODUCT_ROOTS))
}

/// `overrides`（`(相対パス, 本文)` = 注入）で差し替えたグラフ
fn build(base: &[(String, call_graph::Source)], overrides: &[(&str, &str)]) -> call_graph::Graph {
    let sources = base
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

/// ファイル `rel` の関数 `name`（cfg で複数あれば全部）
fn fns_named(graph: &call_graph::Graph, rel: &str, name: &str) -> Vec<usize> {
    let found = graph.find(rel, name);
    assert!(
        !found.is_empty(),
        "{rel} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）"
    );
    found
}

fn named(graph: &call_graph::Graph, f: usize) -> (String, String) {
    let def = &graph.fns[f];
    (graph.sources[def.file].rel.clone(), def.name.clone())
}

/// 関数 `f` の本体に `needle` の綴りが出るか（呼び出しの解決の前の安い絞り込み）
fn mentions(graph: &call_graph::Graph, f: usize, needle: &str) -> bool {
    let def = &graph.fns[f];
    graph.sources[def.file].code[def.body.0..=def.body.1].contains(needle)
}

/// 関数 `f` の本体で `needle` を最初に含む行（無ければ関数の行）
fn line_of(graph: &call_graph::Graph, f: usize, needle: &str) -> usize {
    graph
        .hits(f, needle)
        .first()
        .copied()
        .unwrap_or(graph.fns[f].line)
}

/// 1 の本体: `data_dir` / `local_data_dir` の本体で、門が既定の置き場より先に在るか
fn gate_order_offenders(graph: &call_graph::Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (name, default_call) in [
        ("data_dir", "default_data_dir()"),
        ("local_data_dir", "default_local_data_dir()"),
    ] {
        let f = one_fn(graph, PATHS, name);
        let gate = graph.hits(f, "data_dir_refusal()");
        let default = graph.hits(f, default_call);
        match (gate.first(), default.first()) {
            (Some(g), Some(d)) if g < d => {}
            (_, Some(d)) => out.push(format!(
                "{PATHS}:{d}（fn {name}）が門（data_dir_refusal()）を通らずに既定の置き場 `{default_call}` を返す"
            )),
            (_, None) => out.push(format!(
                "{PATHS}（fn {name}）に `{default_call}` が無い（形が変わったなら番犬も追うこと）"
            )),
        }
    }
    out
}

/// 2 の本体: 既定の置き場を組む関数の呼び手のうち [`DEFAULT_CALLERS`] に無いもの
fn default_resolver_offenders(graph: &call_graph::Graph) -> Vec<String> {
    let targets: Vec<usize> = DEFAULT_RESOLVERS
        .iter()
        .flat_map(|(rel, name)| fns_named(graph, rel, name))
        .collect();
    let mut out = Vec::new();
    for f in 0..graph.fns.len() {
        // 呼び出しの解決は重いので、本体に名前が出る関数だけ解決する
        if !DEFAULT_RESOLVERS
            .iter()
            .any(|(_, name)| mentions(graph, f, name))
        {
            continue;
        }
        let callees = graph.callees(f);
        let hit: Vec<usize> = targets
            .iter()
            .copied()
            .filter(|t| *t != f && callees.contains(t))
            .collect();
        if hit.is_empty() {
            continue;
        }
        let (file, func) = named(graph, f);
        if DEFAULT_CALLERS
            .iter()
            .any(|(r, n, _)| *r == file && *n == func)
        {
            continue;
        }
        for t in hit {
            let target = &graph.fns[t].name;
            let line = line_of(graph, f, &format!("{target}("));
            out.push(format!(
                "{file}:{line}（fn {func}）が既定（本番）の置き場を組む `{target}` を門の外で呼ぶ"
            ));
        }
    }
    out
}

/// `cli_main` が振り分けるハンドラ（`cli_main` が直に呼ぶ関数。GUI へ送る `run` は除く）
fn handlers(graph: &call_graph::Graph) -> Vec<usize> {
    let cli_main = one_fn(graph, CLI_MAIN, "cli_main");
    let run = one_fn(graph, CLI_MAIN, "run");
    let mut out: Vec<usize> = graph
        .callees(cli_main)
        .into_iter()
        .filter(|&f| f != run && f != cli_main)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 3 の本体: 置き場の解決へ届くのに入口の門を通らないハンドラ（file:line + 道筋）
fn ungated_handlers(graph: &call_graph::Graph) -> Vec<String> {
    let resolvers: Vec<usize> = RESOLVERS
        .iter()
        .map(|(rel, name)| one_fn(graph, rel, name))
        .collect();
    let lang: Vec<usize> = LANG_READERS
        .iter()
        .map(|(rel, name)| one_fn(graph, rel, name))
        .collect();
    let diag: Vec<usize> = DIAG_WRITERS
        .iter()
        .map(|(rel, name)| one_fn(graph, rel, name))
        .collect();
    let gate = one_fn(graph, CLI_MAIN, GATE);
    let machine_wide = one_fn(graph, PATHS, MACHINE_WIDE);
    let mut out = Vec::new();
    for h in handlers(graph) {
        if lang.contains(&h) {
            continue;
        }
        let (file, func) = named(graph, h);
        if NO_GATE.iter().any(|(r, n, _)| *r == file && *n == func) {
            continue;
        }
        let reach = graph.reach(&[h], &|_, g| {
            lang.contains(&g) || diag.contains(&g) || resolvers.contains(&g)
        });
        let Some(hit) = reach
            .keys()
            .copied()
            .filter(|&f| {
                (mentions(graph, f, "data_dir(") || mentions(graph, f, "local_data_dir("))
                    && resolvers.iter().any(|r| graph.callees(f).contains(r))
            })
            .min_by_key(|&f| graph.path_to(&reach, f).len())
        else {
            continue;
        };
        let callees = graph.callees(h);
        let queue = QUEUE_HANDLERS.iter().any(|(r, n)| *r == file && *n == func);
        let ok = if queue {
            callees.contains(&machine_wide)
        } else {
            callees.contains(&gate)
        };
        if ok {
            continue;
        }
        let route: String = graph
            .path_to(&reach, hit)
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
        let want = if queue { MACHINE_WIDE } else { GATE };
        out.push(format!(
            "{file}:{}（fn {func}）が置き場の解決へ届くのに入口で `{want}` を呼ばない。道筋: `{func}`{route}",
            graph.fns[h].line
        ));
    }
    out
}

/// 1: 置き場の解決は既定の置き場より先に門を通る
#[test]
fn 置き場の解決は既定の置き場より先に門を通る() {
    let hits = gate_order_offenders(&build(graph_sources(), &[]));
    assert!(
        hits.is_empty(),
        "置き場の解決が隔離の印の門を通らずに既定（本番）の置き場を返す（#2694）:\n  {}",
        hits.join("\n  ")
    );
}

/// 2: 既定の置き場を門の外で組まない
#[test]
fn 既定の置き場を門の外で組まない() {
    let hits = default_resolver_offenders(&build(product_sources(), &[]));
    assert!(
        hits.is_empty(),
        "既定（本番）の data dir を門の外で組む呼び手がある（#2694）:\n  {}\n\n\
         `tako_core::paths::data_dir()`（隔離の印の門を通る）を使うこと。どうしても既定の置き場が要るなら\n\
         DEFAULT_CALLERS へ理由つきで足す",
        hits.join("\n  ")
    );
}

/// 3: 手元のサブコマンドは入口で門を通る
#[test]
fn 手元のサブコマンドは入口で門を通る() {
    let graph = build(graph_sources(), &[]);
    assert!(
        handlers(&graph).len() >= 10,
        "`cli_main` の振り分けが拾えていない（{} 件）",
        handlers(&graph).len()
    );
    for (rel, name, _) in NO_GATE {
        one_fn(&graph, rel, name);
    }
    let hits = ungated_handlers(&graph);
    assert!(
        hits.is_empty(),
        "手元で data dir を読み書きするサブコマンドが入口で門を通らない（#2694）:\n  {}\n\n\
         `TAKO_ISOLATED=1` で `TAKO_DATA_DIR` が無いと置き場の解決は None を返すので、断りの文も終了コードも\n\
         無いまま黙って何もしない（または「HOME が無い」と出る）。ハンドラの最初で `{GATE}()?` を呼ぶこと",
        hits.join("\n  ")
    );
}

/// 4: 例外の口の呼び手は順番待ちだけ
#[test]
fn 例外の口の呼び手は順番待ちだけ() {
    let graph = build(graph_sources(), &[]);
    let machine_wide = one_fn(&graph, PATHS, MACHINE_WIDE);
    let mut callers = Vec::new();
    for f in 0..graph.fns.len() {
        if f != machine_wide
            && mentions(&graph, f, MACHINE_WIDE)
            && graph.callees(f).contains(&machine_wide)
        {
            callers.push(named(&graph, f));
        }
    }
    let mut want: Vec<(String, String)> = QUEUE_HANDLERS
        .iter()
        .map(|(r, n)| (r.to_string(), n.to_string()))
        .collect();
    callers.sort();
    want.sort();
    assert_eq!(
        callers, want,
        "例外の口 `{MACHINE_WIDE}`（隔離の印でも本番の data dir を使う）の呼び手が順番待ちの入口だけでない（#2694）"
    );
}

/// 5: 修正前の形・門を通らないハンドラ・門の外の既定の置き場の注入を名指す
#[test]
fn 修正前の形を名指しできる() {
    let inject = |rel: &str, from: &str, to: &str, label: &str| {
        let src = read(rel);
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let line_in = |text: &str, needle: &str| {
        text.lines()
            .position(|l| l.contains(needle))
            .map(|i| i + 1)
            .unwrap_or_else(|| panic!("注入した行 `{needle}` がある"))
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

    // ① 修正前: data_dir が門を通らずに既定の置き場を返す
    let p = inject(
        PATHS,
        "    if data_dir_refusal().is_some() {\n        return None;\n    }\n    default_data_dir()\n",
        "    default_data_dir()\n",
        "①",
    );
    let line = line_in(&p, "    default_data_dir()");
    expect_named(
        "① data_dir の門を外す",
        gate_order_offenders(&build(graph_sources(), &[(PATHS, &p)])),
        &format!("{PATHS}:{line}（fn data_dir）"),
    );

    // ② 手元のハンドラが入口の門を通らない（usage の入口から門を外す）
    let cli = read(CLI_MAIN);
    let usage_head = "fn usage_local(args: &UsageArgs) -> Result<(), String> {\n";
    let gated = format!("{usage_head}    {GATE}()?;\n");
    let c = inject(CLI_MAIN, &gated, usage_head, "②");
    assert_ne!(c, cli);
    let line = line_in(&c, usage_head.trim_end());
    expect_named(
        "② usage の入口の門を外す",
        ungated_handlers(&build(graph_sources(), &[(CLI_MAIN, &c)])),
        &format!("{CLI_MAIN}:{line}（fn usage_local）"),
    );

    // ③ 門の外で既定の置き場を組む（tako-control の手元の処理に直書き）
    let settings = inject(
        SETTINGS,
        "pub fn settings_path() -> Option<PathBuf> {\n    tako_core::paths::data_dir()",
        "pub fn settings_path() -> Option<PathBuf> {\n    tako_core::paths::default_data_dir()",
        "③",
    );
    let line = line_in(&settings, "tako_core::paths::default_data_dir()");
    expect_named(
        "③ 門の外の既定の置き場",
        default_resolver_offenders(&build(product_sources(), &[(SETTINGS, &settings)])),
        &format!("{SETTINGS}:{line}（fn settings_path）"),
    );
}

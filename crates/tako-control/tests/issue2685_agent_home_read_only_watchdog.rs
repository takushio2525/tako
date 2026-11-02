//! **#2685 の番犬**: 検証用の設定置き場（`tako-agent-config-<pid>`）を、読むだけの口が作る形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 検証プロセス（`TAKO_ISOLATED` / セルフテスト / `cargo test`）が外部エージェントの設定を置く
//! `tako_core::paths::verification_agent_home` は、パスを決めるついでに `create_dir_all` していた。
//! 読むだけの呼び手（`tako usage` の設定 dir の列挙 `usage::index::roots` → `agent_config_home`）も
//! これを引くので、隔離の CLI が 1 回動くごとにその pid の空の dir が残った（実測 3/3）。CLI 自身しか
//! 使わない pid つきの置き場なので、GUI の側では片付けられない。
//!
//! いまは口を 2 つに分けてある:
//!
//! - 読む口 `verification_agent_home` / `orchestrator::agent_config_home`: パスを決めるだけで作らない
//!   （無ければ「まだ何も書いていない = 空」として読む）
//! - 作る口 `ensure_verification_agent_home`: 置き場が先に在ることに頼る書き手だけが通す
//!   （シェルの履歴 = シェルは `HISTFILE` の親 dir を作らない）。事前信頼・mod の install のように
//!   書く瞬間に親 dir を自分で作る書き手は、作る口を通さなくても置き場ごと作る
//!
//! ## ここで止めるもの
//!
//! 1. [`読むだけの口から置き場を作る形へ届かない`] — 読むだけの口（[`READ_ONLY`]）から呼び出しを辿って
//!    届く関数（`common/call_graph.rs`。何段包んでも辿った先で見つかる）が `create_dir` を呼ぶか
//!    作る口を呼んだら、その行を file:line で名指しし、入口からの道筋を添える。別の置き場を決める関数
//!    （[`NOT_FOLLOWED`] = data dir）の中は辿らない
//! 2. [`作る口は名の載った書き手だけが通す`] — 作る口の呼び手を [`MAKERS`]（理由つき）に限る。
//!    寄せ先も縛る: 作る口の本体が `create_dir_all` を持ち、シェル履歴の書き先が作る口を通る
//!    （1 だけだと、作る口ごと消して履歴の書き先が作られなくなっても緑のまま）
//! 3. [`修正前の形を名指しできる`] — 修正前の形（読む口に `create_dir_all` を戻す）・読むだけの呼び手が
//!    作る口へ付け替える・読む口を作る helper で包む、の 3 つの注入を 1 が file:line で名指しすること
//!
//! ## 相方
//!
//! 実物の確かめ（子プロセス = 新しい pid で、`tako usage` / `--by issue` と同じ `usage_payload` を撃っても
//! 置き場が出来ない・シェル履歴と事前信頼は作る・中身が在れば読む）は
//! `usage::index::tests::issue2685_読むだけの経路は検証用の設定置き場を作らない`。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const PATHS: &str = "crates/tako-core/src/paths.rs";
const ORCHESTRATOR: &str = "crates/tako-control/src/orchestrator/mod.rs";
const USAGE_INDEX: &str = "crates/tako-control/src/usage/index.rs";

/// 作る口
const ENSURE: &str = "ensure_verification_agent_home";

/// 読むだけの口（置き場のパスを決める / 設定 dir を列挙する関数）。ここから届く関数は置き場を作らない
const READ_ONLY: &[(&str, &str)] = &[
    (PATHS, "verification_agent_home"),
    (ORCHESTRATOR, "agent_config_home"),
    (USAGE_INDEX, "roots"),
    (USAGE_INDEX, "codex_homes"),
    ("crates/tako-control/src/claude_tui.rs", "config_json_paths"),
    (
        "crates/tako-control/src/claude_tui.rs",
        "default_config_dir",
    ),
    (
        "crates/tako-control/src/claude_mod_install.rs",
        "default_config_dir",
    ),
    ("crates/tako-control/src/worker_mcp.rs", "codex_home_for"),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "evidence_user_files",
    ),
];

/// 作る口を通してよい書き手（理由つき）。増やすときは「なぜ書く前に置き場が在る必要があるか」を書く
const MAKERS: &[(&str, &str, &str)] = &[(
    PATHS,
    "verification_histfile_env",
    "シェルは HISTFILE の親 dir を作らない。在らないと検証のシェルの履歴が書けない",
)];

/// 辿らない関数（理由つき）。ここで見るのは agent の設定置き場で、**別の置き場を決める関数**の中の
/// 作成は別の規約が受け持つ
const NOT_FOLLOWED: &[(&str, &str, &str)] = &[(
    PATHS,
    "data_dir",
    "data dir は別の置き場。accounts.yaml の読み（roots → accounts_list）で届くテストプロセスの \
     tako-test-data-<pid> は #1296 の後始末つき（test_residue_watchdog が受け持つ）",
)];

/// 走査する範囲（読む口は tako-core、読むだけの呼び手は tako-control にある）
const GRAPH_ROOTS: &[&str] = &["crates/tako-core/src", "crates/tako-control/src"];

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

/// 関数 `f` の位置（`file（fn 名前）`）
fn named(graph: &call_graph::Graph, f: usize) -> (String, String) {
    let def = &graph.fns[f];
    (graph.sources[def.file].rel.clone(), def.name.clone())
}

/// 読むだけの口から届く関数のうち、置き場を作るもの（`create_dir` の直書き・作る口の呼び出し）。
///
/// 1 件 = 「作る行（file:line と関数名）+ 入口からの道筋」
fn makers_on_read_paths(graph: &call_graph::Graph) -> Vec<String> {
    let ensure = one_fn(graph, PATHS, ENSURE);
    let skip: Vec<usize> = NOT_FOLLOWED
        .iter()
        .map(|(rel, name, _)| one_fn(graph, rel, name))
        .collect();
    let mut out = Vec::new();
    for (rel, name) in READ_ONLY {
        let entry = one_fn(graph, rel, name);
        // 作る口の本体の `create_dir_all` は「作る口を呼んだ」で名指すので、その先は辿らない
        let reach = graph.reach(&[entry], &|_, g| g == ensure || skip.contains(&g));
        let mut reached: Vec<usize> = reach.keys().copied().collect();
        reached.sort();
        for f in reached {
            let mut lines = graph.hits(f, "create_dir");
            if graph.callees(f).contains(&ensure) {
                match graph.hits(f, &format!("{ENSURE}(")).as_slice() {
                    // 別名で取り込んで呼んだときは字面が無いので関数の行
                    [] => lines.push(graph.fns[f].line),
                    hits => lines.extend_from_slice(hits),
                }
            }
            if lines.is_empty() {
                continue;
            }
            lines.sort();
            lines.dedup();
            let (file, func) = named(graph, f);
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
                    "{file}:{line}（fn {func}）が置き場を作る。道筋: {rel} の `{name}`{route}"
                ));
            }
        }
    }
    out
}

/// 作る口の呼び手のうち [`MAKERS`] に無いもの（file:line）
fn unlisted_makers(graph: &call_graph::Graph) -> Vec<String> {
    let ensure = one_fn(graph, PATHS, ENSURE);
    let mut out = Vec::new();
    for f in 0..graph.fns.len() {
        if f == ensure || !graph.callees(f).contains(&ensure) {
            continue;
        }
        let (file, func) = named(graph, f);
        if MAKERS.iter().any(|(r, n, _)| *r == file && *n == func) {
            continue;
        }
        let lines = match graph.hits(f, &format!("{ENSURE}(")).as_slice() {
            [] => vec![graph.fns[f].line],
            hits => hits.to_vec(),
        };
        for line in lines {
            out.push(format!(
                "{file}:{line}（fn {func}）が作る口 `{ENSURE}` を呼ぶ"
            ));
        }
    }
    out
}

/// 1: 読むだけの口（と、そこから届く関数）は置き場を作らない
#[test]
fn 読むだけの口から置き場を作る形へ届かない() {
    let hits = makers_on_read_paths(&graph_with(&[]));
    assert!(
        hits.is_empty(),
        "読むだけの口から検証用の設定置き場を作る形へ届く（#2685）:\n  {}\n\n\
         読むだけの CLI（`tako usage` 等）が動くたびに空の `tako-agent-config-<pid>` が残る。\n\
         読む側は無ければ空として扱うこと。書く前に置き場が要る書き手は作る口 `{ENSURE}` を通し、\n\
         この番犬の MAKERS へ理由つきで載せる",
        hits.join("\n  ")
    );
}

/// 2: 作る口は名の載った書き手だけが通し、作る口は本当に作り、シェル履歴の書き先は作る口を通る
#[test]
fn 作る口は名の載った書き手だけが通す() {
    let graph = graph_with(&[]);
    let unlisted = unlisted_makers(&graph);
    assert!(
        unlisted.is_empty(),
        "作る口 `{ENSURE}` の呼び手が MAKERS に載っていない（#2685）:\n  {}\n\n\
         読むだけの呼び手なら読む口（`verification_agent_home` / `agent_config_home`）へ戻す。\n\
         書く前に置き場が要る書き手なら、MAKERS へ理由つきで足す",
        unlisted.join("\n  ")
    );
    let ensure = one_fn(&graph, PATHS, ENSURE);
    assert!(
        !graph.hits(ensure, "create_dir_all(").is_empty(),
        "{PATHS} の作る口 `{ENSURE}` が置き場を作っていない（#2685。シェル履歴の書き先が無くなる）"
    );
    let read = one_fn(&graph, PATHS, "verification_agent_home");
    assert!(
        graph.callees(ensure).contains(&read),
        "{PATHS} の作る口 `{ENSURE}` が読む口 `verification_agent_home` からパスを取っていない\
         （置き場の名前の正本が 2 つになる）"
    );
    for (rel, name, _) in MAKERS {
        let f = one_fn(&graph, rel, name);
        assert!(
            graph.callees(f).contains(&ensure),
            "{rel} の `{name}` が作る口 `{ENSURE}` を通っていない（MAKERS の理由の書き先が作られない）"
        );
    }
}

/// 3: 修正前の形・付け替え・包み直しの注入を、1 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    let inject = |rel: &str, from: &str, to: &str, label: &str| {
        let src = read(rel);
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let line_of = |text: &str, needle: &str| {
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

    // ① 修正前: 読む口がパスを決めるついでに作る
    let p = inject(
        PATHS,
        "format!(\"tako-agent-config-{}\", std::process::id()));\n",
        "format!(\"tako-agent-config-{}\", std::process::id()));\n        let _ = std::fs::create_dir_all(&dir);\n",
        "①",
    );
    let line = line_of(&p, "let _ = std::fs::create_dir_all(&dir);");
    expect_named(
        "① 読む口に create_dir_all を戻す",
        makers_on_read_paths(&graph_with(&[(PATHS, &p)])),
        &format!("{PATHS}:{line}（fn verification_agent_home）"),
    );

    // ② 読むだけの呼び手（`tako usage` の設定 dir の列挙）が作る口へ付け替える
    let u = inject(
        USAGE_INDEX,
        "    let home = crate::orchestrator::agent_config_home();\n    let default_dir",
        "    let home = Some(tako_core::paths::ensure_verification_agent_home());\n    let default_dir",
        "②",
    );
    let line = line_of(&u, "ensure_verification_agent_home()");
    let graph = graph_with(&[(USAGE_INDEX, &u)]);
    expect_named(
        "② 読むだけの呼び手が作る口を呼ぶ",
        makers_on_read_paths(&graph),
        &format!("{USAGE_INDEX}:{line}（fn roots）"),
    );
    expect_named(
        "② 作る口の呼び手が MAKERS に無い",
        unlisted_makers(&graph),
        &format!("{USAGE_INDEX}:{line}（fn roots）"),
    );

    // ③ 読む口を作る helper で包む（何段包んでも辿った先で見つかる）
    let o = inject(
        ORCHESTRATOR,
        "        return Some(tako_core::paths::verification_agent_home());\n",
        "        return Some(prepared_home(tako_core::paths::verification_agent_home()));\n",
        "③",
    );
    let o = format!(
        "{o}\nfn prepared_home(dir: PathBuf) -> PathBuf {{\n    let _ = std::fs::create_dir_all(&dir);\n    dir\n}}\n"
    );
    let line = line_of(&o, "let _ = std::fs::create_dir_all(&dir);");
    let hits = makers_on_read_paths(&graph_with(&[(ORCHESTRATOR, &o)]));
    expect_named(
        "③ 包み直し",
        hits.clone(),
        &format!("{ORCHESTRATOR}:{line}（fn prepared_home）"),
    );
    // `tako usage` の入口（roots）からの道筋に載る
    expect_named(
        "③ 道筋に roots → agent_config_home",
        hits,
        "`roots` → `agent_config_home`",
    );
}

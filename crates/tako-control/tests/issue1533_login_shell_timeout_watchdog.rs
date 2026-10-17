//! **#1532 / #1533 の番犬**: ログインシェル（`$SHELL -l -c …`）を**上限なしで待つ**形へ戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! 実行ファイルの検出（`platform::exe::find` の unix 実装 = ログインシェルの `command -v`）と、
//! setup / 環境診断がログインシェルの PATH を読む口（`setup_bootstrap::login_shell_path`）が
//! `Command::output()` で**上限なしに**待っていた。`.zprofile` などが入力を待つ・眠り続ける環境では、
//! `tako setup`・`tako check-health`・GUI の CLI 検出・stale binary の検知・自動命名・Code Runner の
//! 実行環境の検出などの呼び手が**無言で固まる**。`exe::find` の呼び手だけで約 30 か所ある。
//!
//! いまはログインシェルへ尋ねる口が `platform::exe::ask_login_shell` の 1 つだけで、待ちは
//! `probe::group_output_with_timeout`（自分のプロセスグループで起こし、打ち切ったら rc の孫ごと畳む）。
//!
//! ## ここで止めるもの
//!
//! 1. [`ログインシェルを上限なしで待つ形が無い`] — 本番コード（4 クレートの `src/`）の関数で、
//!    ログインシェルの引数（`"-l"` と `"-c"` / `"-lc"`）と上限なしの待ち（`.output()` / `.status()` /
//!    `.wait()` / `.wait_with_output()`）が同じ関数に並んだら、その行を file:line で名指す。
//!    **どのスレッドで待っても呼び手は固まる**ので、別スレッドへ渡すクロージャの中も見る
//!    （`call_graph::Graph::product_ranges`。A/B の旧経路の腕 `if …legacy() {` だけは読まない）。
//!    A/B の再現アームとして残した旧実装（名前が `_legacy` で終わる関数）は、A/B の腕の外から
//!    呼ばれたら呼んだ行を名指す
//! 2. [`寄せ先の1実装を通る`] — 禁止だけでなく**寄せ先も縛る**（#1496 の 2 本立て）。`exe::find` →
//!    `find_bounded` → 上限 `probe::login_shell_timeout()`、尋ねる口 → グループごと畳む待ち、
//!    check-health / setup の経路（導入状況・PATH の判定・gh の検出）が上限つきの口を通ること
//! 3. [`check_healthのtako_versionは上限つきで待つ`] — #1533 の最新コメントの 3 か所目
//!    （`finish_check_health` の `tako --version`）。ログインシェルではないが check-health の経路の待ち
//! 4. [`修正前の形を名指しできる`] — 修正前の形の注入（直書きの復活・別スレッドへの包み直し・
//!    旧実装の呼び戻し・`tako --version` の素の待ち）を 1 / 3 が file:line で名指すこと（同じテストの中の A/B）
//!
//! ## #2255 の番犬を広げずに新しく足した理由
//!
//! #2255 の番犬は「環境診断の入口から**辿って届く**関数が、上限なしの認証の口を呼ばない」という
//! **到達の性質**を見る。#1533 の症状は check-health に限らず `exe::find` の約 30 の呼び手（GUI の
//! 背景スレッドを含む）すべてに出るので、入口から辿る形では GUI の呼び手を取りこぼす。ここは
//! **本番コード全体の形の性質**（ログインシェルを上限なしで待つ関数が 1 つも無い）として持つ。
//! check-health の 3 か所のうち 2 か所（導入状況の検出・PATH の判定）はログインシェルなので 1 が、
//! 残り 1 か所（`tako --version`）は 3 が受け持つ。
//!
//! ## 分かっている限界
//!
//! ログインシェルの `Command` を組んで**返す**関数と、それを受け取って待つ関数が分かれていると
//! 1 は見ない（同じ関数に両方が並ぶ形だけを見る）。いまの本番コードにその形は無い
//! （argv だけを組む `child_cmd::login_shell_snippet` は待たない = CLI を走らせる口で、待つ側は
//! `probe::command_output_with_timeout` を通る）。
//!
//! ## 相方
//!
//! 実経路（入力を待つ・眠り続ける rc の偽のログインシェルで、修正前は固まり・修正後は上限で
//! 「確認できない」を返す。setup・check-health・`exe::find` の代表の呼び手）は
//! `scripts/test-login-shell-timeout-1532.sh`。待ちそのものの挙動は `tako_core::platform::exe` と
//! `tako_core::probe` の単体テスト。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const EXE: &str = "crates/tako-core/src/platform/exe.rs";
const PROBE: &str = "crates/tako-core/src/probe.rs";
const SETUP_BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CONFIG_SHARE_ENV: &str = "crates/tako-control/src/config_share/env.rs";
const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";

/// 走査する範囲（本番コードを持つ 4 クレート全部。`poc/` は品質基準の対象外）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 上限なしの待ち（`Command` のメソッド）
const UNBOUNDED_WAITS: &[&str] = &[".output()", ".status()", ".wait()", ".wait_with_output()"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ。#1420）
fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

/// [`ROOTS`] の `.rs` を全部（文字列を残した本文と、その眺め）。読むのは 1 回だけ
fn sources() -> &'static [(String, call_graph::Source)] {
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

/// 走査の対象（`overrides` = `(相対パス, 本文)` の注入で差し替えたもの）
struct Scan {
    graph: call_graph::Graph,
    /// `graph.sources` と同じ並びの、文字列を残した本文（ログインシェルの引数は文字列なので原文で見る）
    texts: Vec<String>,
}

fn scan_with(overrides: &[(&str, &str)]) -> Scan {
    let mut texts = Vec::new();
    let mut list = Vec::new();
    for (text, source) in sources() {
        match overrides.iter().find(|(rel, _)| *rel == source.rel) {
            Some((rel, injected)) if injected != text => {
                texts.push((*injected).to_string());
                list.push(call_graph::Source::new(rel, injected));
            }
            _ => {
                texts.push(text.clone());
                list.push(source.clone());
            }
        }
    }
    Scan {
        graph: call_graph::Graph::build(list),
        texts,
    }
}

/// `[from, to)` の原文の中で、ログインシェルを起こす引数の並びが始まる位置。
///
/// `"-l"` の直後（同じ文の 120 バイト以内）に `"-c"` が来る形（`["-l", "-c", …]` /
/// `["-l", "-i", "-c", …]` / `"-l".to_string(), "-c".to_string()`）と、まとめた `"-lc"` / `"-ilc"`。
/// `osascript -l JavaScript` のような `-c` を伴わない `-l` は拾わない
fn login_shell_args(text: &str, from: usize, to: usize) -> Vec<usize> {
    let slice = &text[from..to];
    let mut out = Vec::new();
    for (i, _) in slice.match_indices("\"-l\"") {
        let rest = &slice[i..];
        let mut end = rest.find(';').unwrap_or(rest.len()).min(120);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        if rest[..end].contains("\"-c\"") {
            out.push(from + i);
        }
    }
    for pat in ["\"-lc\"", "\"-ilc\"", "\"-lic\""] {
        out.extend(slice.match_indices(pat).map(|(i, _)| from + i));
    }
    out.sort();
    out
}

/// A/B の再現アームとして残した旧実装か（名前が `_legacy` で終わる）
fn is_legacy_fn(name: &str) -> bool {
    name.ends_with("_legacy")
}

/// 1 の本体: ログインシェルを上限なしで待つ関数の行と、旧実装を A/B の腕の外から呼んだ行
fn offenders(scan: &Scan) -> Vec<String> {
    let graph = &scan.graph;
    let mut out = Vec::new();
    let mut legacy_waiters: Vec<String> = Vec::new();
    for (f, def) in graph.fns.iter().enumerate() {
        let source = &graph.sources[def.file];
        let text = &scan.texts[def.file];
        let ranges = graph.product_ranges(f);
        let args: Vec<usize> = ranges
            .iter()
            .flat_map(|&(s, e)| login_shell_args(text, s, e))
            .collect();
        if args.is_empty() {
            continue;
        }
        let waits: Vec<&str> = UNBOUNDED_WAITS
            .iter()
            .copied()
            .filter(|w| ranges.iter().any(|&(s, e)| source.code[s..e].contains(w)))
            .collect();
        if waits.is_empty() {
            continue;
        }
        if is_legacy_fn(&def.name) {
            legacy_waiters.push(def.name.clone());
            continue;
        }
        for at in args {
            out.push(format!(
                "{}:{}（fn {}）がログインシェルを上限なしの `{}` で待つ",
                source.rel,
                source.line_of(at),
                def.name,
                waits.join("` / `")
            ));
        }
    }
    // 旧実装を A/B の腕の外（製品の既定の経路）から呼ぶと、上限なしの待ちが戻る
    for (f, def) in graph.fns.iter().enumerate() {
        if is_legacy_fn(&def.name) {
            continue;
        }
        let source = &graph.sources[def.file];
        for &(s, e) in &graph.product_ranges(f) {
            let code = &source.code[s..e];
            for name in &legacy_waiters {
                let needle = format!("{name}(");
                for (i, _) in code.match_indices(&needle) {
                    let before = code[..i].bytes().last();
                    if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        continue;
                    }
                    out.push(format!(
                        "{}:{}（fn {}）が上限なしの旧実装 `{name}` を A/B の腕（`if …legacy() {{`）の外から呼ぶ",
                        source.rel,
                        source.line_of(s + i),
                        def.name
                    ));
                }
            }
        }
    }
    out
}

/// ファイル `rel` の素の関数 `name`（ちょうど 1 つでなければ理由つきで落ちる）
fn one_fn(graph: &call_graph::Graph, rel: &str, name: &str) -> usize {
    match graph.find(rel, name).as_slice() {
        [f] => *f,
        [] => panic!("{rel} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）"),
        many => panic!("{rel} に `fn {name}` が {} 個ある", many.len()),
    }
}

/// ファイル `rel` の関数 `name` のうち、いちばん上に定義されたもの。
///
/// `exe.rs` は公開の口（`pub fn find`）と OS ごとの実装（`mod imp` の `fn find`）が同じ名前を持つ。
/// 公開の口がファイルの上に来る並びなので、それを選ぶ（見つからなければ理由つきで落ちる）
fn top_fn(graph: &call_graph::Graph, rel: &str, name: &str) -> usize {
    graph
        .find(rel, name)
        .into_iter()
        .min_by_key(|&f| graph.fns[f].line)
        .unwrap_or_else(|| {
            panic!("{rel} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）")
        })
}

/// 関数の製品の経路の眺め（コメント・文字列を潰したコード）
fn product_code(scan: &Scan, f: usize) -> String {
    let graph = &scan.graph;
    let source = &graph.sources[graph.fns[f].file];
    graph
        .product_ranges(f)
        .iter()
        .map(|&(s, e)| &source.code[s..e])
        .collect::<Vec<_>>()
        .join("\n")
}

/// 3 の本体: `finish_check_health` が子を上限なしで待つ行
fn check_health_unbounded(scan: &Scan) -> Vec<String> {
    let graph = &scan.graph;
    let f = one_fn(graph, DISPATCH, "finish_check_health");
    let source = &graph.sources[graph.fns[f].file];
    let mut out = Vec::new();
    for &(s, e) in &graph.product_ranges(f) {
        for wait in UNBOUNDED_WAITS {
            for (i, _) in source.code[s..e].match_indices(wait) {
                out.push(format!(
                    "{}:{}（fn finish_check_health）が子を上限なしの `{wait}` で待つ",
                    source.rel,
                    source.line_of(s + i)
                ));
            }
        }
    }
    out
}

/// 1: 本番コードにログインシェルを上限なしで待つ関数が無い
#[test]
fn ログインシェルを上限なしで待つ形が無い() {
    let scan = scan_with(&[]);
    let hits = offenders(&scan);
    assert!(
        hits.is_empty(),
        "ログインシェルを上限なしで待つ形がある（#1532 / #1533）:\n  {}\n\n\
         rc（.zprofile など）が入力を待つ・眠り続けると呼び手が無言で固まる。\n\
         実行ファイルを探すなら `tako_core::platform::exe::find` / `find_bounded`、\n\
         ログインシェルの PATH を読むなら `exe::login_shell_path` を通すこと（上限つきの 1 実装）",
        hits.join("\n  ")
    );
}

/// 2: 寄せ先も縛る（禁止だけだと、上限つきの口ごと消しても 1 は緑のまま）
#[test]
fn 寄せ先の1実装を通る() {
    let scan = scan_with(&[]);
    let graph = &scan.graph;
    let expect = |rel: &str, name: &str, needles: &[&str]| {
        let f = top_fn(graph, rel, name);
        let code = product_code(&scan, f);
        for needle in needles {
            assert!(
                code.contains(needle),
                "{rel}:{}（fn {name}）に `{needle}` が無い（#1532 / #1533 の寄せ先）",
                graph.fns[f].line
            );
        }
    };
    // 実行ファイルの検出: find → find_bounded → 上限 login_shell_timeout() → 尋ねる口 1 つ
    expect(EXE, "find", &["find_bounded(name)"]);
    expect(
        EXE,
        "find_bounded",
        &["find_with_timeout(name, crate::probe::login_shell_timeout())"],
    );
    expect(EXE, "login_shell_find", &["ask_login_shell("]);
    // ログインシェルの PATH: 上限 login_shell_timeout() → 尋ねる口 1 つ
    expect(
        EXE,
        "login_shell_path",
        &["login_shell_path_in(&user_shell(), crate::probe::login_shell_timeout())"],
    );
    expect(EXE, "login_shell_path_in", &["ask_login_shell("]);
    // 尋ねる口: 自分のプロセスグループで起こし、打ち切ったら rc の孫ごと畳む待ち
    expect(
        EXE,
        "ask_login_shell",
        &["crate::probe::group_output_with_timeout("],
    );
    expect(
        PROBE,
        "group_output_with_timeout",
        &[
            "process_group(0)",
            "wait_with_timeout(child, label, budget)",
        ],
    );
    expect(
        PROBE,
        "kill_on_timeout",
        &[
            "libc::getpgid(pid) == pid",
            "libc::kill(-pid, libc::SIGKILL)",
        ],
    );
    expect(PROBE, "wait_with_timeout", &["kill_on_timeout(&mut child)"]);
    // check-health / setup の経路: 導入状況の検出は上限つきで 1 回だけ（PATH の判定でも使い回す）
    expect(
        SETUP_BOOTSTRAP,
        "status_with",
        &[
            "tako_core::platform::exe::find_bounded(agent.as_str())",
            "launcher_dir_on_path(&launcher_dir, &found)",
        ],
    );
    expect(
        SETUP_BOOTSTRAP,
        "login_shell_sees",
        &["tako_core::platform::exe::login_shell_path()"],
    );
    expect(
        SETUP_BOOTSTRAP,
        "dir_on_path",
        &["tako_core::platform::exe::login_shell_path()"],
    );
    // 眺めは文字列を潰す（`find_bounded("gh")` の名前は見えない）ので、口の名前だけで縛る
    expect(CONFIG_SHARE_ENV, "find_gh", &["exe::find_bounded("]);
    expect(
        SETUP_DEPS,
        "resolve_bounded",
        &["exe::find_bounded(dep.bin)"],
    );
    expect(
        DISPATCH,
        "finish_check_health",
        &["tako_core::platform::exe::find_bounded("],
    );
}

/// 3: check-health の `tako --version` も上限つきで待つ（#1533 の最新コメントの 3 か所目）
#[test]
fn check_healthのtako_versionは上限つきで待つ() {
    let scan = scan_with(&[]);
    let hits = check_health_unbounded(&scan);
    assert!(
        hits.is_empty(),
        "check-health が子を上限なしで待つ（#1533）:\n  {}\n\n\
         PATH の先の tako が返らないと check-health ごと止まる。\
         `tako_core::probe::output_with_timeout` を通すこと",
        hits.join("\n  ")
    );
    let f = one_fn(&scan.graph, DISPATCH, "finish_check_health");
    assert!(
        product_code(&scan, f).contains("tako_core::probe::output_with_timeout("),
        "{DISPATCH} の `finish_check_health` が `tako --version` を上限つきの口で待っていない（#1533）"
    );
}

/// 4: 修正前の形・包み直し・旧実装の呼び戻しの注入を、1 / 3 が file:line で名指しする
#[test]
fn 修正前の形を名指しできる() {
    let inject = |rel: &str, from: &str, to: &str, label: &str| {
        let src = sources()
            .iter()
            .find(|(_, s)| s.rel == rel)
            .map(|(t, _)| t.clone())
            .unwrap_or_else(|| panic!("{rel} が走査の範囲に無い"));
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };
    let line_of = |text: &str, needle: &str| {
        text.lines()
            .position(|l| l.contains(needle))
            .map(|i| i + 1)
            .unwrap_or_else(|| panic!("注入した行 `{needle}` が無い"))
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

    // ① 修正前の setup_bootstrap::login_shell_path（PATH の読み取りを上限なしで待つ）を書き戻す
    let old_path = "fn login_shell_path_unbounded() -> Option<String> {\n    \
        let shell = std::env::var(\"SHELL\").unwrap_or_else(|_| \"/bin/sh\".into());\n    \
        let output = std::process::Command::new(shell)\n        \
        .args([\"-l\", \"-c\", \"printf %s \\\"$PATH\\\"\"])\n        \
        .env(\"PATH\", \"/usr/bin:/bin\")\n        \
        .output()\n        \
        .ok()?;\n    \
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())\n}\n\n";
    let anchor = "/// ログインシェルの PATH に `dir` が入っているか";
    let b = inject(SETUP_BOOTSTRAP, anchor, &format!("{old_path}{anchor}"), "①");
    let line = line_of(&b, ".args([\"-l\", \"-c\", \"printf %s");
    expect_named(
        "① 修正前の login_shell_path",
        offenders(&scan_with(&[(SETUP_BOOTSTRAP, &b)])),
        &format!("{SETUP_BOOTSTRAP}:{line}（fn login_shell_path_unbounded）"),
    );

    // ② 同じ待ちを別スレッドへ包む（GUI の背景スレッドの形。どのスレッドで待っても呼び手は固まる）
    let threaded = "fn find_on_thread(name: &str) -> Option<String> {\n    \
        let name = name.to_string();\n    \
        std::thread::spawn(move || {\n        \
        let out = std::process::Command::new(\"/bin/zsh\")\n            \
        .args([\"-l\", \"-c\", &format!(\"command -v {name}\")])\n            \
        .output()\n            \
        .ok()?;\n        \
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())\n    \
        })\n    \
        .join()\n    \
        .ok()\n    \
        .flatten()\n}\n\n";
    let anchor = "/// 外部バイナリの解決（tmux_bin() と同じフォールバック";
    let p = inject(
        "crates/tako-app/src/preview.rs",
        anchor,
        &format!("{threaded}{anchor}"),
        "②",
    );
    let line = line_of(
        &p,
        ".args([\"-l\", \"-c\", &format!(\"command -v {name}\")])",
    );
    expect_named(
        "② 別スレッドの中の待ち",
        offenders(&scan_with(&[("crates/tako-app/src/preview.rs", &p)])),
        &format!("crates/tako-app/src/preview.rs:{line}（fn find_on_thread）"),
    );

    // ③ exe::find が A/B の腕の外で旧実装（上限なし）を呼ぶ = 修正前の find へ戻す
    let e = inject(
        EXE,
        "        let found = find_bounded(name);\n        if let Some(notice) = &found.timeout {",
        "        let _ = imp::find_legacy(name);\n        let found = find_bounded(name);\n        if let Some(notice) = &found.timeout {",
        "③",
    );
    let line = line_of(&e, "let _ = imp::find_legacy(name);");
    expect_named(
        "③ 旧実装の呼び戻し",
        offenders(&scan_with(&[(EXE, &e)])),
        &format!("{EXE}:{line}（fn find）が上限なしの旧実装 `find_legacy`"),
    );

    // ④ check-health の `tako --version` を素の `.output()` へ戻す
    let d = inject(
        DISPATCH,
        "            let outcome = tako_core::probe::output_with_timeout(\n                path,\n                &[\"--version\"],\n                tako_core::probe::probe_timeout(),\n            );",
        "            let _ = std::process::Command::new(path).arg(\"--version\").output();\n            let outcome = tako_core::probe::output_with_timeout(\n                path,\n                &[\"--version\"],\n                tako_core::probe::probe_timeout(),\n            );",
        "④",
    );
    let line = line_of(&d, "Command::new(path).arg(\"--version\").output()");
    expect_named(
        "④ tako --version の素の待ち",
        check_health_unbounded(&scan_with(&[(DISPATCH, &d)])),
        &format!("{DISPATCH}:{line}（fn finish_check_health）"),
    );
}

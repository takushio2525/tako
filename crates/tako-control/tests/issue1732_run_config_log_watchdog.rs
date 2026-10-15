//! 番犬: 実行設定の**環境変数の値・引数・実行前コマンド**を診断ログへ出さない（#1732）。
//!
//! ## 何を止めたいのか
//!
//! #1732 で `tako run-config <file> --env K=V --args … --before …` が保存され、`Run` が
//! コマンド文字列へ埋める。値はユーザーデータ（API キーを env に入れる人もいる）なので、
//! **persist.log / perf.log / stderr へ出してはいけない**（AGENTS.md の絶対ルール）。
//! 壊れ方は「デバッグのつもりで 1 行足す」形なので、値が通る経路のソースを走査して
//! ログの呼び出しを 1 つも置かせない:
//!
//! 1. 保存と読み出し（`tako-control::run_configs` の全体・`tako-core::runner_config`）
//! 2. dispatch の `Run` / `RunResolve` / `RunnerDefaults` の腕と `finish_run_resolve`
//! 3. 解決器が層を当てる所（`runner::resolve_core` / 展開の `expand_with`）
//! 4. 境界 B1 の組み立て（`compose_run_script` / `guard_with_before`）
//!
//! **落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! 隔離 GUI の実経路 `scripts/test-run-config-1732.sh` が、目印の値で `--env` / `--args` /
//! `--before` を保存して走らせ、隔離 data dir の persist.log / perf.log と GUI の stderr を
//! grep して 0 件を確かめる（器あり = persist ON でも）。
//!
//! ## A/B（検出力の確かめ方）
//!
//! `注入した違反をfile_lineで名指しする` が、各対象へ `eprintln!` / `crate::diag::persist_log(`
//! を 1 行入れた写しで FAILED になり、file:line を名指すことを確かめる

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const RUN_CONFIGS: &str = "crates/tako-control/src/run_configs.rs";
const RUNNER_CONFIG: &str = "crates/tako-core/src/runner_config.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const RUNNER: &str = "crates/tako-core/src/runner.rs";
const SHELL: &str = "crates/tako-core/src/platform/shell.rs";

/// 診断ログ・標準出力へ書く呼び出し。**語の頭でだけ数える**（`pane_log::` / `dialog::` を
/// `log::` に数えない。`crate::diag::persist_log(` は `:` の後ろなので数える）
const LOG_CALLS: &[&str] = &[
    "persist_log(",
    "perf_log(",
    "flow_log(",
    "panic_log(",
    "eprintln!",
    "eprint!",
    "println!",
    "print!",
    "dbg!",
    "log::",
    "tracing::",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].matches('\n').count() + 1
}

/// 目印 `from` から `to` の手前まで（コメント・文字列を潰した眺めで探す）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn region(rel: &str, src: &str, from: &str, to: &str) -> Result<(usize, usize), String> {
    let start = src
        .find(from)
        .ok_or_else(|| format!("{rel}: 目印 {from:?} が消えている（走査範囲を作れない）"))?;
    let end = src[start + from.len()..]
        .find(to)
        .map(|i| start + from.len() + i)
        .ok_or_else(|| format!("{rel}:{} の後ろに目印 {to:?} が無い", line_of(src, start)))?;
    Ok((start, end))
}

/// 範囲の中のログ呼び出しを file:line で並べる（コメント・文字列の中は数えない）
fn violations(rel: &str, src: &str, (start, end): (usize, usize)) -> Vec<String> {
    let view = production_range::code_view_of(src);
    let slice = &view[start..end];
    let mut out = Vec::new();
    for call in LOG_CALLS {
        let mut from = 0;
        while let Some(i) = slice[from..].find(call) {
            let at = start + from + i;
            let head = view[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            from += i + call.len();
            if head {
                continue;
            }
            out.push(format!(
                "{rel}:{} に `{call}` がある（実行設定の値が通る経路。env の値・引数・実行前コマンドを\
                 診断ログへ出さない = AGENTS.md の絶対ルール / #1732）",
                line_of(src, at)
            ));
        }
    }
    out
}

/// 1 ファイルぶんの検査（本番コードの範囲だけ）
fn check(rel: &str, src: &str, ranges: &[(&str, &str)]) -> Result<(), String> {
    let prod = production_range::production(src, rel);
    let mut found = Vec::new();
    if ranges.is_empty() {
        found.extend(violations(rel, &prod, (0, prod.len())));
    }
    for (from, to) in ranges {
        let r = region(rel, &prod, from, to)?;
        found.extend(violations(rel, &prod, r));
    }
    if found.is_empty() {
        Ok(())
    } else {
        Err(found.join("\n"))
    }
}

/// 対象の範囲（空 = ファイルの本番コード全体）
fn targets() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (RUN_CONFIGS, vec![]),
        (RUNNER_CONFIG, vec![]),
        (
            DISPATCH,
            vec![
                (
                    "        Request::Run {\n            path,",
                    "        Request::Settings { action, tab } => {",
                ),
                ("fn finish_run_resolve(", "\n}\n"),
            ],
        ),
        (
            RUNNER,
            vec![
                ("fn expand_with(", "// --- 解決器 ---"),
                ("fn resolve_core(", "fn declared_cwd<'a>("),
            ],
        ),
        (
            SHELL,
            vec![("pub fn compose_run_script(", "/// PTY の子へ argv を")],
        ),
    ]
}

#[test]
fn 実行設定の値が通る経路に診断ログの呼び出しが無い() {
    let mut errors = Vec::new();
    for (rel, ranges) in targets() {
        if let Err(e) = check(rel, &read(rel), &ranges) {
            errors.push(e);
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

// --- 番犬そのものが効いていることの自己検査（実ファイルへの注入 A/B）---

/// 目印の直後へ 1 行足した写し
fn inject(src: &str, anchor: &str, line: &str) -> String {
    let at = src
        .find(anchor)
        .unwrap_or_else(|| panic!("注入の目印 {anchor:?} が無い"))
        + anchor.len();
    let mut out = src.to_string();
    out.insert_str(at, line);
    out
}

#[test]
fn 注入した違反をfile_lineで名指しする() {
    let cases: &[(&str, &str, &str)] = &[
        // 保存の本体（handle）
        (
            RUN_CONFIGS,
            "pub fn handle(a: PathArgs<'_>) -> Result<Value, DispatchError> {\n",
            "    eprintln!(\"{:?}\", a.env);\n",
        ),
        // 層を作る所
        (
            RUN_CONFIGS,
            "pub fn layer(&self) -> (tako_core::ConfigLayer, Vec<String>) {\n",
            "        crate::diag::persist_log(&format!(\"{:?}\", self.file));\n",
        ),
        // Run の腕で組み立てた直後
        (
            DISPATCH,
            "            let (layer, bad_env) = saved.layer();\n",
            "            crate::diag::perf_log(&format!(\"{layer:?}\"));\n",
        ),
        // 境界 B1 の組み立て
        (
            SHELL,
            "fn guard_with_before(dialect: ShellDialect, before: &str, command: &str) -> String {\n",
            "    eprintln!(\"{before}\");\n",
        ),
        // 解決器が層を当てる所
        (
            RUNNER,
            "    let args = layer.args.as_deref().unwrap_or(\"\");\n",
            "    dbg!(args);\n",
        ),
    ];
    for (rel, anchor, line) in cases {
        let injected = inject(&read(rel), anchor, line);
        let ranges = targets()
            .into_iter()
            .find(|(r, _)| r == rel)
            .map(|(_, ranges)| ranges)
            .unwrap();
        let err = check(rel, &injected, &ranges)
            .expect_err(&format!("{rel} への注入 {line:?} を検出できていない"));
        let want_line = line_of(&injected, injected.find(line.trim()).unwrap());
        assert!(
            err.contains(&format!("{rel}:{want_line}")),
            "{rel}:{want_line} を名指ししていない: {err}"
        );
    }
}

/// 目印が消えたら（関数の改名・移動）黙って緑にならない
#[test]
fn 目印が消えたら落ちる() {
    let src = read(DISPATCH).replace("fn finish_run_resolve(", "fn finish_run_resolve_renamed(");
    let err = check(DISPATCH, &src, &[("fn finish_run_resolve(", "\n}\n")])
        .expect_err("目印の消失を検出できていない");
    assert!(err.contains("目印"), "{err}");
}

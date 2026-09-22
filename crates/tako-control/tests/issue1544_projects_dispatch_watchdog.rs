//! `tako orchestrator projects` の CLI が dispatch を通ることの番犬（#1544）
//!
//! # なぜ要るか
//!
//! CLI と MCP は**同じ 1 本**（`dispatch_orchestrator_projects`）を通る約束で、
//! これは AGENTS.md の不変条件「操作 API + dispatch + CLI + MCP の 1:1」そのもの。
//!
//! ところが `orchestrator_projects_cli` は `add` / `remove` だけを #1453 で
//! dispatch へ寄せ、`list` は `ProjectsConfig::load()` の直読みを残していた。
//! 出力は一致していたので**症状が出ない**が、これは #1453 で実際に壊れたのと
//! まったく同じ形（CLI 側の写し）で、そのときは `add` の専用プロファイル自動生成が
//! **MCP からだけ効いて CLI からは効かない**状態になっていた。
//!
//! 「同じ関数に直したものと残したものが並ぶ」状態は、次に dispatch 側へ機能を足した人が
//! 片側だけ直す事故を必ず呼ぶ。直読みが**戻ってきたこと**をここで止める。
//!
//! # 何を縛るか
//!
//! 1. `orchestrator_projects_cli` の本文に `ProjectsConfig`（設定ファイルの直読み）が
//!    現れない
//! 2. 3 つの分岐（list / add / remove）がすべて `dispatch_orchestrator_projects` を通る
//! 3. CLI が渡す action の綴り（`"list"` / `"add"` / `"remove"`）が本文に揃っている
//! 4. 正本（dispatch 側）が `"list"` を受け付け、`projects` 配列で返す
//!    （CLI が渡す綴りが正本から消えたら、CLI は実行時にだけ落ちる）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜4 はすべて無意味に緑になるので、[`走査が空振りしていない`]で
//! 窓が採れていることを固定し、[`逆戻りを名指しできる`]で**修正前を再現した 4 通りの注入**が
//! file:line で名指しされることを確かめる。範囲取りは #1420 の 1 実装を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const CLI: &str = "crates/tako-cli/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

/// 写しを作らせない側（CLI の入口）
const CLI_FN: &str = "fn orchestrator_projects_cli(";
/// 正本（CLI も MCP もここ 1 本を通る）
const DISPATCH_FN: &str = "pub fn dispatch_orchestrator_projects(";
/// 正本の呼び出し
const SOURCE_OF_TRUTH: &str = "dispatch_orchestrator_projects(";
/// 設定ファイルの直読み（写しの証拠）
const DIRECT_READ: &str = "ProjectsConfig";
/// CLI の 3 分岐が 1:1 で載る action
const ACTIONS: [&str; 3] = ["list", "add", "remove"];

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる = #1420）
fn production(rel: &'static str, src: &str) -> String {
    // main.rs / dispatch.rs はテストが厚い（後半が丸ごと `#[cfg(test)]`）
    production_range::production_with_floor(src, rel, 0.20)
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中のコード行を `(ファイル内の 1 始まり行番号, 本文)` で返す。
/// 行まるごとの注釈は落とす（アンチパターンを説明した注釈で落ちないようにする）。
/// 文字列リテラルは**残す**（action の綴りを見る検査があるため）
fn code_lines(at: usize, window: &str) -> Vec<(usize, &str)> {
    window
        .lines()
        .enumerate()
        .map(|(i, l)| (at + i, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// 注釈を落とした窓（`contains` 用）
fn code_only(at: usize, window: &str) -> String {
    code_lines(at, window)
        .into_iter()
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の窓の中でだけ文字列を置き換える（注入用）。
/// ファイル全体を置換すると無関係な実装まで壊れて「何を測ったか」が濁る
fn replace_in_fn(src: &str, needle: &str, from: &str, to: &str) -> String {
    let Some((at, window)) = fn_window(src, needle) else {
        panic!("注入先の窓 `{needle}` が無い（走査が空振り）");
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = at - 1;
    let end = start + window.lines().count();
    let replaced = window.replace(from, to);
    // 見つからないまま素通しすると「注入したつもり」で緑になるので、短い理由で落とす
    // （`assert_ne!` は両辺に窓の全文を出して読めなくなるので使わない）
    if replaced == window {
        panic!(
            "注入する綴りが窓 `{needle}` に無い（本番側が既に逆戻りしているか、\
             rustfmt の折り返しが変わった）:\n{from}"
        );
    }
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.push(replaced);
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// 1〜3: CLI の入口に写しが無く、3 分岐が正本を通る
fn scan_cli(cli: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(cli, CLI_FN) else {
        out.push(Offender {
            file: CLI,
            line: 0,
            why: format!("`{CLI_FN}` が見つからない（走査が空振り）"),
        });
        return out;
    };

    // 1: 設定ファイルの直読みが戻っていないか（行で名指しする）
    for (line, text) in code_lines(at, &window) {
        if text.contains(DIRECT_READ) {
            out.push(Offender {
                file: CLI,
                line,
                why: format!(
                    "CLI が `{DIRECT_READ}` を直読みしている。CLI と MCP は \
                     `{SOURCE_OF_TRUTH}` の 1 本を通す約束で、写しを残すと dispatch 側の \
                     追加が CLI からだけ効かなくなる（#1453 で実際に踏んだ形。#1544）"
                ),
            });
        }
    }

    let code = code_only(at, &window);

    // 2: 3 分岐すべてが正本を通っているか
    let calls = code.matches(SOURCE_OF_TRUTH).count();
    if calls < ACTIONS.len() {
        out.push(Offender {
            file: CLI,
            line: at,
            why: format!(
                "`{SOURCE_OF_TRUTH}` の呼び出しが {calls} 本しかない（list / add / remove の \
                 {} 本が要る）。1 本でも外れると、その分岐だけ dispatch の更新から取り残される",
                ACTIONS.len()
            ),
        });
    }

    // 3: CLI が渡す action の綴りが揃っているか
    for action in ACTIONS {
        if !code.contains(&format!("\"{action}\"")) {
            out.push(Offender {
                file: CLI,
                line: at,
                why: format!(
                    "action `\"{action}\"` を `{SOURCE_OF_TRUTH}` へ渡していない \
                     （分岐が正本から外れたか、綴りが正本とずれた）"
                ),
            });
        }
    }
    out
}

/// 4: 正本が CLI の渡す action を受け付け、`projects` で返す
fn scan_dispatch(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(dispatch, DISPATCH_FN) else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: format!("正本 `{DISPATCH_FN}` が見つからない（走査が空振り）"),
        });
        return out;
    };
    let code = code_only(at, &window);
    for action in ACTIONS {
        if !code.contains(&format!("\"{action}\" =>")) {
            out.push(Offender {
                file: DISPATCH,
                line: at,
                why: format!(
                    "正本が action `\"{action}\"` を受け付けていない。CLI はこの綴りを渡すので、\
                     消えると実行時にだけ「action が不正」で落ちる（#1544）"
                ),
            });
        }
    }
    if !code.contains("\"projects\"") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "正本の list が `projects` 配列を返していない。CLI の一覧表示は \
                  この鍵で読むので、変えるなら CLI も同時に直す（#1544）"
                .into(),
        });
    }
    out
}

fn scan_all(cli: &str, dispatch: &str) -> Vec<Offender> {
    let mut out = scan_cli(cli);
    out.extend(scan_dispatch(dispatch));
    out
}

fn sources() -> (String, String) {
    (
        production(CLI, &read(CLI)),
        production(DISPATCH, &read(DISPATCH)),
    )
}

#[test]
fn projectsの一覧が設定ファイルを直読みしていない() {
    let (cli, dispatch) = sources();
    let offenders = scan_all(&cli, &dispatch);
    assert!(
        offenders.is_empty(),
        "#1544 の逆戻り:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let (cli, dispatch) = sources();
    for (rel, src, needle) in [(CLI, &cli, CLI_FN), (DISPATCH, &dispatch, DISPATCH_FN)] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{rel} に `{needle}` の窓が無い（走査が空振り）"));
        assert!(
            at > 0 && window.lines().count() > 10,
            "{rel} の窓が薄い: {at} 行目から {} 行",
            window.lines().count()
        );
    }
    // CLI 側の窓に 3 分岐が揃っていること（潰しすぎると検査が空振りで緑になる）
    let (at, window) = fn_window(&cli, CLI_FN).expect("窓が採れる");
    let code = code_only(at, &window);
    for arm in [
        "ProjectsCommand::List",
        "ProjectsCommand::Add",
        "ProjectsCommand::Remove",
    ] {
        assert!(
            code.contains(arm),
            "CLI の本番範囲から {arm} が消えている（走査が空振り）"
        );
    }
}

#[test]
fn 逆戻りを名指しできる() {
    let (cli, dispatch) = sources();

    // 注入 1: list が直読みへ戻る（#1544 の修正前そのもの）
    let cli_direct = replace_in_fn(
        &cli,
        CLI_FN,
        "            let res =\n                \
         tako_control::dispatch::dispatch_orchestrator_projects(\"list\", None, None, None)\n                    \
         .map_err(|e| e.to_string())?;\n            \
         let projects = res[\"projects\"].as_array().cloned().unwrap_or_default();",
        "            let config = orchestrator::ProjectsConfig::load()?;\n            \
         let projects = config.list_resolved();",
    );
    // 注入 2: list の action の綴りが正本とずれる
    let cli_typo = replace_in_fn(
        &cli,
        CLI_FN,
        "dispatch_orchestrator_projects(\"list\"",
        "dispatch_orchestrator_projects(\"ls\"",
    );
    // 注入 3: add が正本から外れる（#1453 の再発）
    let cli_add_off = replace_in_fn(&cli, CLI_FN, "\"add\",", "\"add_legacy\",");
    // 注入 4: 正本から list が消える（CLI の渡す綴りが宙に浮く）
    let dispatch_no_list = replace_in_fn(&dispatch, DISPATCH_FN, "\"list\" =>", "\"list_gone\" =>");

    let cases: Vec<(&str, Vec<Offender>)> = vec![
        ("1: list が直読みへ戻る", scan_all(&cli_direct, &dispatch)),
        ("2: action の綴りがずれる", scan_all(&cli_typo, &dispatch)),
        ("3: add が正本から外れる", scan_all(&cli_add_off, &dispatch)),
        (
            "4: 正本から list が消える",
            scan_all(&cli, &dispatch_no_list),
        ),
    ];

    for (label, offenders) in cases {
        assert!(
            !offenders.is_empty(),
            "注入 {label} を番犬が見逃した（検出力が無い）"
        );
        assert!(
            offenders.iter().any(|o| o.line > 0),
            "注入 {label} を file:line で名指しできていない: {:?}",
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}

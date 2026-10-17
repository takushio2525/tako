//! **#1621 の番犬**: `tako setup`（`run_setup`）の出力でスリープ防止の段が 2 度出る形へ戻らない。
//!
//! ## 何が起きていたか（砂場の実測で確定）
//!
//! `--answers` に `sleep_guard` があると（MCP `tako_setup` / dispatch `SetupRun` も
//! 同じ `tako setup --yes --answers -`）、スリープ防止が 2 か所から出ていた:
//!
//! 1. 依存チェック段の `run_sleep_guard_check` が**当てる前の値**と
//!    「設定変更: tako sleep-guard set …」の案内を出す
//! 2. 後段の `apply_sleep_guard_answers` が書いたあと `[input] スリープ防止: …` を出す
//!
//! 値を変える答えでは「while-agents-running → 変え方の案内 → off」と食い違った 2 行になり、
//! 今と同じ値の答えでも同じ状態が 2 度並んだ。標準 / `--yes` / `--check` / TTY / `--review`
//! は 1 度だけで、2 度出るのは答えのある経路だけ（`--review` と `--answers` は clap が
//! 同時指定を断るので、対話と答えが重なる経路は無い）。
//!
//! ## 何を縛るか
//!
//! 1. スリープ防止の状態の行（`スリープ防止: mode=`）を組むのは `sleep_guard_stage_lines` だけ
//! 2. 書き込み（`apply_sleep_guard_answers`）は何も出さない
//! 3. `run_setup` は段を依存チェック経由の 1 回だけ通り、段を直に呼ばない
//!
//! 段の見出しの中身（答えの有無・対話の有無で状態の行が 1 行）は CLI 側の単体テスト
//! `issue1621_スリープ防止の段は状態を1度だけ出す` が、実経路の回数は
//! `scripts/test-setup-sleep-guard-once-1621.sh` が見る

use std::path::{Path, PathBuf};
use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// テスト領域を潰して本番コードだけを見る（CLI 側の単体テストは禁止形を文字列で持つ）
#[path = "common/production_range.rs"]
mod production_range;

const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";
/// 状態の行の目印。今の値（`  スリープ防止: mode=`）も答え（`  [input] スリープ防止: mode=`）も拾う
const STATE_LINE: &str = "スリープ防止: mode=";
/// 状態の行を組んでよい唯一の関数
const STAGE_LINES_FN: &str = "sleep_guard_stage_lines";
const STAGE_FN: &str = "run_sleep_guard_check";
const APPLY_FN: &str = "apply_sleep_guard_answers";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードだけの眺め（行番号は保たれるので `file:line` の名指しに使える）
fn production() -> String {
    let path = repo_root().join(CLI_SETUP);
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{CLI_SETUP} を読めない: {e}"));
    production_range::production(&src, CLI_SETUP)
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// コメントでない行を (行番号, 囲んでいるトップレベル関数名, 行) で並べる
fn code_lines(src: &str) -> Vec<(usize, String, String)> {
    let mut current = String::from("(ファイル先頭)");
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if is_top_level_fn_head(line) {
            current = fn_head_name(line).unwrap_or_default().to_string();
        }
        if !is_comment(line) {
            out.push((i + 1, current.clone(), line.to_string()));
        }
    }
    out
}

/// トップレベル関数 `name` の本文（コメントを除く行）。無ければ名指しして落ちる
fn body_of(src: &str, name: &str) -> Vec<(usize, String)> {
    let body: Vec<(usize, String)> = code_lines(src)
        .into_iter()
        .filter(|(_, func, _)| func == name)
        .map(|(n, _, line)| (n, line))
        .collect();
    assert!(
        !body.is_empty(),
        "{CLI_SETUP} に `fn {name}` が無い（改名したなら番犬も追う。#1621）"
    );
    body
}

fn named(hits: &[(usize, String)], func: &str) -> String {
    hits.iter()
        .map(|(n, line)| format!("  {CLI_SETUP}:{n} fn {func}: {}", line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 1: 状態の行を組むのは段の見出しの 1 関数だけ
#[test]
fn スリープ防止の状態の行は段の見出しだけが組む() {
    let src = production();
    let hits: Vec<(usize, String, String)> = code_lines(&src)
        .into_iter()
        .filter(|(_, _, line)| line.contains(STATE_LINE))
        .collect();
    assert!(
        hits.iter().any(|(_, func, _)| func == STAGE_LINES_FN),
        "{CLI_SETUP}: `fn {STAGE_LINES_FN}` に状態の行（`{STATE_LINE}`）が無い。\
         見張る面が消えた（改名したなら番犬も追う）"
    );
    let outside: Vec<String> = hits
        .iter()
        .filter(|(_, func, _)| func != STAGE_LINES_FN)
        .map(|(n, func, line)| format!("  {CLI_SETUP}:{n} fn {func}: {}", line.trim()))
        .collect();
    assert!(
        outside.is_empty(),
        "スリープ防止の状態の行を `{STAGE_LINES_FN}` の外で組んでいる（#1621）:\n{}\n\
         `tako setup` の出力に同じ段が 2 度出る。表示は依存チェック段の 1 か所に寄せ、\
         ほかの段は値を渡すだけにすること",
        outside.join("\n")
    );
}

/// 2: `--answers` の書き込みは黙って書く（表示は段が済ませている）
#[test]
fn スリープ防止の答えの書き込みは何も出さない() {
    let src = production();
    let body = body_of(&src, APPLY_FN);
    let printing: Vec<(usize, String)> = body
        .into_iter()
        .filter(|(_, line)| {
            ["print!", "println!", "writeln!(", "write!("]
                .iter()
                .any(|m| line.contains(m))
        })
        .collect();
    assert!(
        printing.is_empty(),
        "`{APPLY_FN}` が出力している（#1621）:\n{}\n\
         書いた値は依存チェック段が `[input]` として出し済み。ここで出すと 2 度目になる",
        named(&printing, APPLY_FN)
    );
}

/// 3: `run_setup` は段を依存チェック経由の 1 回だけ通る
#[test]
fn run_setupはスリープ防止の段を1回だけ通る() {
    let src = production();
    let body = body_of(&src, "run_setup");
    let count = |needle: &str| -> Vec<(usize, String)> {
        body.iter()
            .filter(|(_, line)| line.contains(needle))
            .cloned()
            .collect()
    };
    let dep = count("run_dependency_check(");
    assert_eq!(
        dep.len(),
        1,
        "`run_setup` が依存チェック段（中でスリープ防止の段を出す）を {} 回呼んでいる（#1621）:\n{}",
        dep.len(),
        named(&dep, "run_setup")
    );
    for direct in [&format!("{STAGE_FN}("), &format!("{STAGE_LINES_FN}(")] {
        let hits = count(direct);
        assert!(
            hits.is_empty(),
            "`run_setup` がスリープ防止の段を直に出している（#1621）:\n{}\n\
             段は依存チェックの中の 1 回だけ。値を変えたいなら `sleep_guard_answer` で渡す",
            named(&hits, "run_setup")
        );
    }
    // ファイル全体でも段の呼び口は 1 つずつ（別の関数から 2 本目を生やさない）
    for (callee, caller) in [
        (STAGE_FN, "run_dependency_check"),
        (STAGE_LINES_FN, STAGE_FN),
    ] {
        let calls: Vec<(usize, String, String)> = code_lines(&src)
            .into_iter()
            .filter(|(_, func, line)| {
                line.contains(&format!("{callee}("))
                    && func != callee
                    && fn_head_name(line).is_none()
            })
            .collect();
        let wrong: Vec<String> = calls
            .iter()
            .filter(|(_, func, _)| func != caller)
            .map(|(n, func, line)| format!("  {CLI_SETUP}:{n} fn {func}: {}", line.trim()))
            .collect();
        assert!(
            calls.len() == 1 && wrong.is_empty(),
            "`{callee}` の呼び口は `{caller}` の 1 か所だけのはず（#1621。実際 {} か所）:\n{}",
            calls.len(),
            wrong.join("\n")
        );
    }
}

/// 実経路の回数を見るスクリプトが在り、CI に載っている（番犬はソースの形、こちらは出力の回数）
#[test]
fn 実経路テストが登録されている() {
    let script = "scripts/test-setup-sleep-guard-once-1621.sh";
    assert!(
        repo_root().join(script).is_file(),
        "{script} が無い（#1621 の実測がここに居る）"
    );
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml を読める");
    assert!(
        ci.contains(script),
        ".github/workflows/ci.yml に {script} の登録が無い（#1621）"
    );
}

//! **#2292 の番犬**: 新品の HOME の 1 回目の `tako setup` が tako mod を設定 dir へ入れない形、
//! 入れても最終サマリが「変更なし」と言う形へ戻らない。
//!
//! ## 何が起きていたか（砂場の実測で確定）
//!
//! 1. mod を設定 dir の `skills/tako` へ置く段（`claude_mod_install::run_setup_stage`。#1959）が
//!    `~/.claude/CLAUDE.md` を書く段（`apply_instruction`）より**前**に走っていた。置くのは在る
//!    設定 dir だけ（tako は設定 dir を作らない = FR-2.42.25）なので、`~/.claude` が無い新品の HOME の
//!    1 回目は `no_config_dir` で黙って入らず、直後の段が `~/.claude` を作っていた
//! 2. 2 回目は `~/.claude` が在るので入るが、段の結果が最終サマリ（`plan`）に積まれず
//!    「セットアップ結果: 変更なし」と出ていた
//!
//! ## 何を縛るか
//!
//! 1. `run_setup` の中で mod の段の呼び出しは `apply_instruction(` より後ろ
//! 2. mod の段には最終サマリ（`&mut plan`）を渡し、サマリの表示（`print_setup_summary(&plan)`）は
//!    その後ろ
//!
//! 段がサマリへ何を積むか（書いた・外した dir だけ・入れない条件では積まない）は tako-control の
//! 単体テスト `issue2292_*` が、新品の HOME の実経路は `scripts/test-setup-mod-first-run-2292.sh` が見る

use std::path::{Path, PathBuf};
use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// テスト領域を潰して本番コードだけを見る
#[path = "common/production_range.rs"]
mod production_range;

const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";
const SCRIPT: &str = "scripts/test-setup-mod-first-run-2292.sh";
const MOD_STAGE: &str = "claude_mod_install::run_setup_stage(";
const INSTRUCTION: &str = "apply_instruction(";
const SUMMARY_ARG: &str = "&mut plan";
const SUMMARY_PRINT: &str = "print_setup_summary(&plan)";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn production() -> String {
    let path = repo_root().join(CLI_SETUP);
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{CLI_SETUP} を読めない: {e}"));
    production_range::production(&src, CLI_SETUP)
}

/// `run_setup` の本文のコメントでない行を (行番号, 行) で
fn run_setup_body(src: &str) -> Vec<(usize, String)> {
    let mut current = String::new();
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if is_top_level_fn_head(line) {
            current = fn_head_name(line).unwrap_or_default().to_string();
        }
        if current == "run_setup" && !line.trim_start().starts_with("//") {
            out.push((i + 1, line.to_string()));
        }
    }
    out
}

/// 違反を `file:line` つきで返す（空なら合格）
fn findings(src: &str) -> Vec<String> {
    let body = run_setup_body(src);
    if body.is_empty() {
        return vec![format!(
            "{CLI_SETUP}: `fn run_setup` が無い（改名したなら番犬も追う。#2292）"
        )];
    }
    let find = |needle: &str| -> Vec<(usize, String)> {
        body.iter()
            .filter(|(_, line)| line.contains(needle))
            .cloned()
            .collect()
    };
    let at = |(n, line): &(usize, String)| format!("{CLI_SETUP}:{n}: {}", line.trim());
    let stage = find(MOD_STAGE);
    let instruction = find(INSTRUCTION);
    let print = find(SUMMARY_PRINT);
    let mut out = Vec::new();
    let [stage] = stage.as_slice() else {
        return vec![format!(
            "`run_setup` の中の mod の段（`{MOD_STAGE}`）が {} か所（1 か所のはず。#1959 / #2292）",
            stage.len()
        )];
    };
    match instruction.first() {
        None => out.push(format!(
            "`run_setup` が `{INSTRUCTION}` を呼ばない（指示ファイルの段が見えない。改名したなら番犬も追う）"
        )),
        Some(first) if stage.0 < first.0 => out.push(format!(
            "mod の段が指示ファイルの段より前にある（#2292）:\n  {}\n  {}\n\
             新品の HOME の `~/.claude` は指示ファイルの段が作るので、前に置くと 1 回目は入らない",
            at(stage),
            at(first)
        )),
        Some(_) => {}
    }
    // 呼び出しが複数行に折れていても拾えるよう、呼び出しの行から `;` までを見る
    let call: String = body
        .iter()
        .skip_while(|(n, _)| *n != stage.0)
        .scan(false, |done, (_, line)| {
            if *done {
                return None;
            }
            *done = line.contains(';') || line.trim_end().ends_with('{');
            Some(line.clone())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if !call.contains(SUMMARY_ARG) {
        out.push(format!(
            "mod の段に最終サマリ（`{SUMMARY_ARG}`）を渡していない（#2292）:\n  {}\n\
             書いた・外した設定 dir が数えられず「変更なし」と出る",
            at(stage)
        ));
    }
    match print.first() {
        None => out.push(format!(
            "`run_setup` が `{SUMMARY_PRINT}` を呼ばない（最終サマリが見えない）"
        )),
        Some(p) if p.0 < stage.0 => out.push(format!(
            "最終サマリの表示が mod の段より前にある（#2292）:\n  {}\n  {}",
            at(p),
            at(stage)
        )),
        Some(_) => {}
    }
    out
}

#[test]
fn modの段は指示ファイルの段の後ろで最終サマリへ積む() {
    let found = findings(&production());
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 番犬に検出力がある（修正前の形へ戻す注入をそれぞれ名指しで落とす）
#[test]
fn 番犬は修正前の形を名指しで落とす() {
    let src = production();
    assert!(findings(&src).is_empty());

    // 1. 段を指示ファイルの段の前へ戻す（#2292 の修正前の並び）
    let lines: Vec<&str> = src.lines().collect();
    let instr = lines
        .iter()
        .position(|l| l.contains("let instruction_coverage = apply_instruction("))
        .expect("指示ファイルの段の行");
    let mut reordered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let moved = reordered.remove(instr);
    let stage = reordered
        .iter()
        .position(|l| l.contains(MOD_STAGE))
        .expect("mod の段の行");
    // `for line in …run_setup_stage(…) {` の 3 行（for / eprintln / }）の後ろへ置く = 修正前の並び
    reordered.insert(stage + 3, moved);
    let found = findings(&reordered.join("\n"));
    assert!(
        found
            .iter()
            .any(|f| f.contains("指示ファイルの段より前") && f.contains(CLI_SETUP)),
        "並びを戻しても落ちない: {found:?}"
    );

    // 2. サマリを渡さない（使い捨てのサマリへ積む）
    let throwaway = src.replace(
        &format!("{MOD_STAGE}claude_version, {SUMMARY_ARG})"),
        &format!("{MOD_STAGE}claude_version, &mut Default::default())"),
    );
    assert_ne!(
        throwaway, src,
        "注入の置換が当たらない（呼び出しの形が変わったなら追う）"
    );
    let found = findings(&throwaway);
    assert!(
        found.iter().any(|f| f.contains("最終サマリ")),
        "サマリを渡さなくても落ちない: {found:?}"
    );

    // 3. 段を run_setup から外す
    let gone = src.replace(MOD_STAGE, "claude_mod_install::dry(");
    let found = findings(&gone);
    assert!(
        found.iter().any(|f| f.contains("0 か所")),
        "段を外しても落ちない: {found:?}"
    );
}

/// 新品の HOME の実経路テストが在り、CI に載っている（番犬はソースの形、こちらは実際の 1 回目 / 2 回目）
#[test]
fn 実経路テストが登録されている() {
    assert!(
        repo_root().join(SCRIPT).is_file(),
        "{SCRIPT} が無い（#2292 の実測がここに居る）"
    );
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml を読める");
    assert!(
        ci.contains(SCRIPT),
        ".github/workflows/ci.yml に {SCRIPT} の登録が無い（#2292）"
    );
}

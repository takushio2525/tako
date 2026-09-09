//! Windows のテストが**コンパイルできること**を CI が守っている番犬（#1264）
//!
//! `crates/tako-core/tests/shell_integration_powershell.rs` のように
//! `#![cfg(windows)]` で始まるテストファイルは、macOS では**空クレート**として
//! コンパイルされる。そのうえ CI の Windows ジョブは
//!
//! - `cargo build --workspace` が blocking — だが `tests/` のターゲットは作らない
//! - `cargo test --workspace` は `continue-on-error: true`（#583。POSIX 前提の
//!   既存テストが落ちているあいだテスト結果を非ブロッキングにしている）
//!
//! という組み合わせだったので、**Windows のテストが 1 件も走らない壊れ方が
//! 両ジョブをすり抜けた**（#1264。#1199 の検証ブロックが変数の無い関数へ入り
//! `E0425` × 3 で 102 秒後にコンパイルエラー終了。実機のベースライン照合・
//! フレーク調査が全部止まった）。
//!
//! 手当ては「テストの**コンパイル**だけを blocking にする」1 ステップ
//! （`cargo test --workspace --no-run`）。テスト結果の扱い（#583）は変えない。
//! そのステップが消える / 非ブロッキングへ倒されると同じ穴が開くので、ここで拘束する。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn ci_yaml() -> String {
    let path = repo_root().join(".github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `windows:` ジョブの本文だけを切り出す（macOS ジョブのステップと混ぜない）。
/// ジョブの終わりは「インデントが 2 未満の行」= 次のトップレベルキー
fn windows_job(ci: &str) -> String {
    let (_, rest) = ci
        .split_once("\n  windows:")
        .expect("CI に windows ジョブが無い（#1264）");
    rest.lines()
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// ステップ（`      - ` で始まる行）ごとに割る。`- name:` の無い
/// `- uses:` も 1 ステップとして数えるので、`continue-on-error` を隣のステップの
/// ものと読み違えない
fn steps(job: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in job.lines() {
        if line.starts_with("      - ") || out.is_empty() {
            out.push(String::new());
        }
        let step = out.last_mut().expect("直前に必ず 1 本ある");
        step.push_str(line);
        step.push('\n');
    }
    out
}

/// コメント行（`#` 始まり）を落とす。説明文の中の文字列に当たって誤検知しないため
fn without_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn windowsジョブがテストのコンパイルをblockingで検査する() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let steps = steps(&job);
    assert!(
        !steps.is_empty(),
        "windows ジョブのステップを切り出せていない（この番犬の読み取りが壊れている）"
    );

    let compile: Vec<&String> = steps
        .iter()
        .filter(|s| s.contains("cargo test") && s.contains("--no-run"))
        .collect();
    assert_eq!(
        compile.len(),
        1,
        "Windows ジョブに「テストのコンパイルだけを検査する」ステップが無い（#1264）。\
         `cargo build --workspace` は tests/ を作らず `cargo test` は非ブロッキング（#583）\
         なので、これが無いと Windows のテストが 1 件も走らない壊れ方がすり抜ける\n{job}"
    );
    let compile = compile[0];
    assert!(
        compile.contains("--workspace"),
        "コンパイル検査がワークスペース全体を見ていない（#1264）\n{compile}"
    );
    assert!(
        !compile.contains("continue-on-error"),
        "コンパイル検査が非ブロッキングになっている（#1264。これでは #583 と同じく\
         すり抜ける。**結果**の非ブロッキングは下のテスト実行ステップの担当）\n{compile}"
    );
}

#[test]
fn テスト結果の非ブロッキング_583_は据え置かれている() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let run: Vec<String> = steps(&job)
        .into_iter()
        .filter(|s| s.contains("cargo test") && !s.contains("--no-run"))
        .collect();
    assert_eq!(
        run.len(),
        1,
        "Windows ジョブのテスト実行ステップが 1 本でない（#583 の扱いが変わった？）\n{job}"
    );
    assert!(
        run[0].contains("continue-on-error: true"),
        "#1264 のコンパイル検査を足すついでにテスト結果まで blocking へ倒してはいけない\
         （POSIX 前提の既存失敗が残っているあいだは #583 の方針を据え置く）\n{}",
        run[0]
    );
}

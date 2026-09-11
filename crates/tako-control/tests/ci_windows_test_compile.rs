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
//!
//! ## 狭く絞った blocking なテストは許す（#1282）
//!
//! 据え置くのは「**ワークスペース全体**の結果を blocking にしない」であって、
//! 「blocking なテストを 1 本も置かない」ではない。macOS では 1 行も実行できない
//! コード（Windows の Win32 FFI 等）は、名指しの小さなステップで実際に走らせないと
//! 誰も検査できない。しかも `cargo test --workspace` は **tako-app の既知失敗で
//! そこで打ち切られ、以降のクレートは 1 件も走らない**（実測 2026-09-11:
//! `622 passed / 1 failed` で終了）ので、ぶら下げても走らない。
//!
//! そこで規則を精密化する: `--workspace` のテスト実行は 1 本だけ・非ブロッキングのまま、
//! **それ以外の `cargo test` 実行ステップは `--workspace` を付けてはいけない**
//! （付けられると #583 の方針が裏口から blocking へ倒れる）。

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
    let (workspace, scoped): (Vec<&String>, Vec<&String>) =
        run.iter().partition(|s| s.contains("--workspace"));
    assert_eq!(
        workspace.len(),
        1,
        "Windows ジョブの `cargo test --workspace` 実行ステップが 1 本でない\
         （#583 の扱いが変わった？）\n{job}"
    );
    assert!(
        workspace[0].contains("continue-on-error: true"),
        "#1264 のコンパイル検査を足すついでにテスト結果まで blocking へ倒してはいけない\
         （POSIX 前提の既存失敗が残っているあいだは #583 の方針を据え置く）\n{}",
        workspace[0]
    );
    // #1282: 名指しの小さな検査は blocking で置いてよいが、**ワークスペース全体を
    // 巻き込んではいけない**（上で 1 本に絞ってあるので、ここは念のための二重化）
    for step in scoped {
        assert!(
            !step.contains("--workspace"),
            "狭く絞ったはずのテスト実行が --workspace を持っている\n{step}"
        );
    }
}

/// #1282: macOS で 1 行も実行できない Windows 固有コードが、CI で**実際に走る**こと。
///
/// `cargo test --workspace` は tako-app の既知失敗（#583）でそこで打ち切られ
/// tako-core まで届かないので、名指しのステップが無いと Win32 FFI
/// （`NtQueryInformationProcess` でのコマンドライン / `GetProcessTimes` での起動時刻）は
/// **コンパイルされるだけで一度も呼ばれない**。構造体レイアウトの転記ミス・
/// 情報クラス番号・`FILETIME` の起点はそこでしか捕まらない
#[test]
fn windows固有ffiの実行検査がblockingで置かれている() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let ffi: Vec<String> = steps(&job)
        .into_iter()
        .filter(|s| s.contains("cargo test") && s.contains("platform::procinfo"))
        .collect();
    assert_eq!(
        ffi.len(),
        1,
        "Windows の FFI を実際に叩く検査ステップが無い（#1282）。\
         `cargo test -p tako-core --lib platform::procinfo` を windows ジョブへ置くこと\n{job}"
    );
    assert!(
        !ffi[0].contains("continue-on-error"),
        "FFI の実行検査が非ブロッキングになっている（#1282。壊れても緑のまま通る）\n{}",
        ffi[0]
    );
}

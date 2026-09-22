//! Windows のテストが**コンパイルできること**を CI が守っている番犬（#1264）
//!
//! `crates/tako-core/tests/shell_integration_powershell.rs` のように
//! `#![cfg(windows)]` で始まるテストファイルは、macOS では**空クレート**として
//! コンパイルされる。そのうえ CI の Windows ジョブは
//!
//! - `cargo build --workspace` が blocking — だが `tests/` のターゲットは作らない
//! - `cargo test --workspace` は `continue-on-error: true`（#583。POSIX 前提の
//!   既存テストが落ちているあいだテスト結果を非ブロッキングにしていた。
//!   **#1278 で解除した**ので現在は blocking）
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
//! ## 現行契約（#1278 で更新）
//!
//! 1. `cargo test --workspace --no-run` = **コンパイルだけ**を見る blocking ステップ（#1264）
//! 2. `cargo test --workspace --no-fail-fast` = **結果まで** blocking（#1278）。
//!    `--no-fail-fast` が要るのは、既定の cargo が最初に落ちたテストバイナリで打ち切り、
//!    その先のクレートを 1 件も走らせないため（実測 2026-09-22 の main: tako-app の
//!    1 件で止まり tako-control / tako-core は 0 件）。全数が採れないと
//!    「直したら次が出る」を 1 PR 1 往復でしか進められない
//! 3. 名指しの小さな実行ステップ（Win32 FFI = #1282 / psmux = #1314）は blocking のまま
//!    **`--workspace` を付けてはいけない**。全数ステップを 1 本に保ち、
//!    「どのステップが何を見たか」をログから読めるようにするため
//!
//! Windows に存在しない仕組みを見るテストは、**理由つきで名指し skip**
//! （`#[cfg(...)]` / `#[cfg_attr(windows, ignore = "理由")]`）へ寄せた。
//! 理由の無い skip が増えないことは `issue1278_ignore_reason_watchdog` が見る。

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

/// #1278: ワークスペース全体のテスト結果が **blocking** で、しかも
/// **全数が採れる形**（`--no-fail-fast`）で回っていること。
///
/// #583 はここを `continue-on-error: true` で据え置いていたが、そのあいだに
/// Windows の未検出が溜まり、実機のベースラインが 19 → 24 件まで増えた（#1278）。
/// 倒し戻されると同じことが起きるので、両方をここで拘束する
#[test]
fn windowsジョブのテスト結果がblockingで全数を採る() {
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
         （#1278 / #583 の扱いが変わった？）\n{job}"
    );
    assert!(
        !workspace[0].contains("continue-on-error"),
        "Windows のテスト結果が非ブロッキングへ戻されている（#1278 で blocking 化した。\
         非ブロッキングだと壊れても緑のまま通り、実機のベースラインが黙って増える）\n{}",
        workspace[0]
    );
    assert!(
        workspace[0].contains("--no-fail-fast"),
        "全数が採れない（#1278）。既定の cargo は最初に落ちたテストバイナリで打ち切るので、\
         その先のクレートが 1 件も走らない = 1 回の CI で 1 件しか直せない\n{}",
        workspace[0]
    );
    // #1282 / #1314: 名指しの小さな検査は blocking で置いてよいが、**ワークスペース全体を
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

//! **#1733 の番犬**: 実行ボタン横のドロップダウンは dispatch の答えを読むだけで、描画から
//! 実行環境の検出を呼ばない。
//!
//! ## なぜ止めるのか
//!
//! 実行環境の検出（`tako_core::runtime_env` の Tier F = stat と先頭読み、`tako_control::runtime_probe`
//! の Tier P = 子プロセスの問い合わせ。1 回の上限 5 秒）は、プレビューのヘッダ・メニューの描画
//! （毎フレーム）から呼ぶと UI スレッドで待つ。しかも描画ごとに自前で解くと、一覧のコマンドと
//! `Run` が走らせるコマンドの出どころが 2 つになる（#1730 の申し送り:「一覧は `python3 a.py` の
//! まま・実際は `.venv` の python」）。#1733 は一覧を dispatch `RunResolve` の答え（background で
//! 取った完成品）へ寄せ、描画はそれを読むだけにした。
//!
//! ## 何を固定するか
//!
//! 1. `preview_render.rs` / `run_menu_ui.rs` の本番コードの `fn render_*` の本文に
//!    `runtime_env::` / `runtime_probe::` が現れない
//! 2. メニューの取得は `Request::RunResolve`、書き込みは `Request::RunnerDefaults` を通る
//!    （UI 専用の解決・保存を作らない = 開発不変条件）。メモリ上だけの選択
//!    （`preview_run_selected`）を持たない
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で両ファイルの `render_*` の窓が採れていることを、
//! [`注入した呼び出しを名指しで落とす`] で**描画の本文へ入れた検出の呼び出しが `file:line` で
//! 名指しされる**ことを確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。テスト領域・コメント・文字列を空白へ潰す
#[path = "common/production_range.rs"]
mod production_range;

const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";
const RUN_MENU: &str = "crates/tako-app/src/run_menu_ui.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// 描画の本文に現れてはいけない呼び出し（実行環境の検出の 2 層）
const DETECTORS: &[&str] = &["runtime_env::", "runtime_probe::"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの「コードの眺め」（バイト長と行番号が保たれる）
fn code_of(src: &str, rel: &str) -> String {
    production_range::code_view_of(&production_range::production(src, rel))
}

/// `fn render_*` の窓（宣言の行（1 始まり）・関数名・本文）。本文は宣言の後の最初の `{` から
/// 対応する `}` まで（文字列・コメントは潰してあるので括弧を数えてよい）
fn render_fns(code: &str) -> Vec<(usize, String, String)> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = code[from..].find("fn render_") {
        let at = from + rel;
        from = at + 3;
        // 識別子の途中（`xfn render_`）は飛ばす
        if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
            continue;
        }
        let name: String = code[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let Some(open_rel) = code[at..].find('{') else {
            continue;
        };
        let open = at + open_rel;
        let mut depth = 0usize;
        let mut end = None;
        for (i, b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let line = code[..at].matches('\n').count() + 1;
        out.push((line, name, code[at..=end].to_string()));
    }
    out
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。違反は `file:line — 理由`
fn offenders(code: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (line, name, body) in render_fns(code) {
        for (i, l) in body.lines().enumerate() {
            for d in DETECTORS {
                if l.contains(d) {
                    out.push(format!(
                        "{rel}:{} — `{name}` の本文から `{d}` を呼んでいる。描画は毎フレーム走るので、\
                         実行環境の検出（Tier P は子プロセス）を UI スレッドで待つうえ、一覧と `Run` の\
                         出どころが 2 つになる。dispatch `RunResolve` の答え（`run_menu.view(pane)`）を\
                         読むだけにする（#1733）",
                        line + i
                    ));
                }
            }
        }
    }
    out
}

// --------------------------------------------------------------- テスト

#[test]
fn 描画の本文から実行環境の検出を呼ばない() {
    let mut found = Vec::new();
    for rel in [PREVIEW_RENDER, RUN_MENU] {
        let raw = read(rel);
        found.extend(offenders(&code_of(&raw, rel), rel));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていない（ファイル移動・改名や範囲取りの破損で黙って 0 件にならない）
#[test]
fn 走査が空振りしていない() {
    let menu = code_of(&read(RUN_MENU), RUN_MENU);
    let names: Vec<String> = render_fns(&menu).into_iter().map(|(_, n, _)| n).collect();
    for want in [
        "render_run_menu_overlay",
        "render_run_menu_runtime",
        "render_run_menu_fields",
        "render_run_menu_field_editor",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "{RUN_MENU} の `fn {want}` の窓が採れない（走査が空振り）: {names:?}"
        );
    }
    // プレビューの本文の描画（ヘッダの再生ボタン・シェブロンを組む関数）が窓に入っている
    let preview = code_of(&read(PREVIEW_RENDER), PREVIEW_RENDER);
    let windows = render_fns(&preview);
    assert!(
        windows.iter().any(
            |(_, _, body)| body.contains("run_menu.view(") && body.contains("toggle_run_menu(")
        ),
        "{PREVIEW_RENDER} の再生ボタンを組む `render_*` の窓が採れない（{} 個）",
        windows.len()
    );
}

/// 取得は `RunResolve`、書き込みは `RunnerDefaults`、選択はメモリに持たない
#[test]
fn 取得と保存はdispatchを通る() {
    let menu = code_of(&read(RUN_MENU), RUN_MENU);
    for (needle, why) in [
        (
            "Request::RunResolve {",
            "一覧は dispatch `RunResolve`（CLI `tako run <f> --list` / MCP `tako_run_resolve` と同じ答え）を取る",
        ),
        (
            "Request::RunnerDefaults {",
            "保存は dispatch `RunnerDefaults`（CLI `tako run-config` / MCP `tako_run_defaults` と同じ口）を直に呼ぶ",
        ),
        ("dispatch_off_ui(", "`RunResolve` は background で取る（Tier P の子プロセスを UI スレッドで待たない）"),
    ] {
        assert!(menu.contains(needle), "{RUN_MENU} に `{needle}` が無い: {why}（#1733）");
    }
    for rel in [MAIN, PREVIEW_RENDER, RUN_MENU] {
        let code = code_of(&read(rel), rel);
        assert!(
            !code.contains("preview_run_selected"),
            "{rel} にメモリ上だけのプロファイルの選択（`preview_run_selected`）が残っている。\
             選択は `RunnerDefaults {{ profile }}` で `run-configs.json` へ保存する（#1731 / #1733）"
        );
    }
}

/// 番犬に検出力があること: 描画の本文へ入れた検出の呼び出しを `file:line` で名指しする
#[test]
fn 注入した呼び出しを名指しで落とす() {
    let check = |snippet: &str| offenders(&production_range::code_view_of(snippet), "注入.rs");
    for (label, injected, line) in [
        (
            "① Tier F をヘッダの描画から",
            "fn render_preview(x: u8) {\n    let a = 1;\n    let k = tako_core::runtime_env::kind_for_ext(\"py\");\n}\n",
            3,
        ),
        (
            "② Tier P をメニューの描画から",
            "fn render_run_menu_overlay() -> u8 {\n    if true {\n        let s = tako_control::runtime_probe::for_resolve(p, false);\n    }\n    0\n}\n",
            3,
        ),
        (
            "③ use で取り込んだ短い名前",
            "fn render_x() {\n    runtime_env::detect(a, b);\n}\n",
            2,
        ),
    ] {
        let found = check(injected);
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
    }

    // ④ 描画でない関数（取得の続き・保存）からの呼び出しは対象外 / コメント・文字列は潰れる
    let ok = "fn request_run_resolve() {\n    tako_core::runtime_env::kind_for_ext(\"py\");\n}\n\
              fn render_ok() {\n    // runtime_env:: はコメント\n    let s = \"runtime_probe::\";\n}\n";
    let found = check(ok);
    assert!(found.is_empty(), "④ 安全な形まで落とした: {found:?}");
}

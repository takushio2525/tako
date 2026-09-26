//! Code Runner が「プロジェクトを見る解決」から戻らないための番犬（Issue #1656）。
//!
//! ## 何を止めるか
//!
//! 1. **入口がプロジェクトを見ない解決を呼ぶ形**。dispatch の `Run` / `RunResolve`
//!    （CLI `tako run` / `--dry-run`・MCP `tako_run` / `tako_run_resolve`）と、再生ボタンの
//!    プロファイル検出（`preview_render.rs`）が `resolve_file` 以外を通ると、cargo
//!    プロジェクトの `.rs` が `rustc` 単体へ落ちる元の症状へ戻る。プロジェクトを見ない
//!    `resolve` / `resolve_for` は `#[cfg(test)] pub(crate)` なので外からはコンパイルが
//!    通らないが、**探索を切れる `resolve_file_in(…, None)` はコンパイルが通る**。
//!    ここで止めるのはその形
//! 2. **種別の知識が表の外へ漏れる形**。印のファイル名（`Cargo.toml` / `package.json` …）を
//!    `runner.rs` / `project_root.rs` / `dispatch.rs` の本番コードへ書くと、種別の追加が
//!    「`runner_project::KINDS` への行追加だけ」で済まなくなる
//! 3. **新設 2 ファイルへの属性 cfg**（#1655 と同じ理由。属性で出し分けると macOS の
//!    単体から Windows の列を検査できない）
//!
//! 挙動そのもの（どのプロジェクトで何に解決されるか）は tako-core の単体
//! （`runner_project::tests` / `runner::tests`）と dispatch の単体が見る。ここは**配線**だけ。
//!
//! ## 眺めの選び方
//!
//! - 呼び出しの有無（規則 1）は `code_view`（コメントも文字列も潰す）。説明文に
//!   `resolve_file_in(` と書くのは違反ではない（このファイルの doc がそう）
//! - 印の直書き（規則 2）は文字列リテラルそのものを見るので `without_comments` を
//!   `production`（テスト領域を潰す）に重ねる。テスト領域には期待値として正しく存在する
//! - 属性 cfg（規則 3）は `production` に重ねた `code_view`（`project_root.rs` の
//!   シンボリックリンクのテストは unix 専用の API を使うので `#[cfg(unix)]` を正しく持つ）

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};
use production_range::{production, production_with_floor};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読める: {e}"))
}

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const PREVIEW: &str = "crates/tako-app/src/preview_render.rs";
const RUNNER: &str = "crates/tako-core/src/runner.rs";
const PROJECT: &str = "crates/tako-core/src/runner_project.rs";
const ROOT: &str = "crates/tako-core/src/project_root.rs";

fn 行番号(src: &str, byte: usize) -> usize {
    src[..byte].lines().count().max(1)
}

/// `view` の中で `start` から始まり `end` の手前で終わる区間（見つからなければ `Err`）
fn 区間<'a>(
    view: &'a str,
    rel: &str,
    start: &str,
    end: &str,
) -> Result<(usize, &'a str), String> {
    let from = view
        .find(start)
        .ok_or_else(|| format!("{rel}: `{start}` が見つからない（入口の綴りが変わった？）"))?;
    let to = view[from..]
        .find(end)
        .map(|n| from + n)
        .ok_or_else(|| format!("{rel}: `{start}` の終わり `{end}` が見つからない"))?;
    Ok((from, &view[from..to]))
}

// --- 規則 1: 入口 ---

/// dispatch の `Run` / `RunResolve` の腕がどちらも `resolve_file` を通る
fn dispatchの入口を確かめる(src: &str) -> Result<(), String> {
    let view = code_view(&production(src, DISPATCH));
    let (from, arms) = 区間(
        &view,
        DISPATCH,
        "Request::Run {\n            path,",
        "Request::RunnerDefaults {",
    )?;
    let calls = arms.matches("tako_core::resolve_file(").count();
    if calls != 2 {
        return Err(format!(
            "{DISPATCH}:{} の `Run` / `RunResolve` が `tako_core::resolve_file` を {calls} 回しか\
             通っていない（2 回 = 両方の腕が要る）。プロジェクトを見ない解決に戻すと、\
             cargo プロジェクトの `.rs` が `rustc` 単体で走って必ず失敗する（Issue #1656）",
            行番号(src, from)
        ));
    }
    Ok(())
}

/// 再生ボタンのプロファイル検出が `resolve_file` を通る（ドロップダウンと実行がずれない）
fn 再生ボタンの入口を確かめる(src: &str) -> Result<(), String> {
    let view = code_view(&production(src, PREVIEW));
    let (from, body) = 区間(
        &view,
        PREVIEW,
        "fn detect_preview_run_profiles(",
        "fn run_preview_file(",
    )?;
    if !body.contains("runner::resolve_file(") {
        return Err(format!(
            "{PREVIEW}:{} の `detect_preview_run_profiles` が `runner::resolve_file` を\
             通っていない。再生ボタンの一覧と実行（dispatch `Run`）で解決がずれる（Issue #1656）",
            行番号(src, from)
        ));
    }
    Ok(())
}

/// 探索を切れる入口（`resolve_file_in`）は runner.rs（テスト）以外の本番コードで呼ばない
fn 探索を切る入口を探す(src: &str, rel: &str) -> Result<(), String> {
    if let Some(pos) = code_view(src).find("resolve_file_in(") {
        return Err(format!(
            "{rel}:{} が `resolve_file_in` を呼んでいる。これは探索範囲を外から渡す**検査用の口**で、\
             `None` を渡すとプロジェクトを見ない #1656 以前の解決になる。入口は `resolve_file` の \
             1 本を通す（Issue #1656）",
            行番号(src, pos)
        ));
    }
    Ok(())
}

fn 本番の_rsファイル() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let crates = repo_root().join("crates");
    let mut stack: Vec<PathBuf> = std::fs::read_dir(&crates)
        .expect("crates/ を読める")
        .filter_map(Result::ok)
        .map(|e| e.path().join("src"))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(repo_root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn dispatchのrunとrun_resolveはプロジェクトを見る入口を通る() {
    dispatchの入口を確かめる(&read(DISPATCH)).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 再生ボタンのプロファイル検出はプロジェクトを見る入口を通る() {
    再生ボタンの入口を確かめる(&read(PREVIEW)).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 探索を切る入口はrunnerの外で呼ばれていない() {
    let files = 本番の_rsファイル();
    assert!(
        files.len() > 100,
        "走査したファイルが {} 本しかない（走査先の取り違え）",
        files.len()
    );
    let violations: Vec<String> = files
        .iter()
        .filter(|(rel, _)| rel != RUNNER)
        .filter_map(|(rel, src)| 探索を切る入口を探す(src, rel).err())
        .collect();
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

// --- 規則 2: 種別の知識は表の中だけ ---

/// 表（`runner_project::KINDS`）の印の綴りを、文字列リテラルとして探す形にして返す
fn 印の綴り() -> Vec<String> {
    let mut out: Vec<String> = tako_core::runner_project::KINDS
        .iter()
        .flat_map(|k| k.markers.iter())
        .map(|m| format!("\"{}\"", m.trim_start_matches('*')))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn 印の直書きを探す(src: &str, rel: &str, floor: f64) -> Result<(), String> {
    let view = without_comments_checked(&production_with_floor(src, rel, floor), rel);
    for spelled in 印の綴り() {
        if let Some(pos) = view.find(&spelled) {
            return Err(format!(
                "{rel}:{} にプロジェクトの印 {spelled} が直書きされている。種別の知識は \
                 `runner_project::KINDS` の行に置く（表の外に書くと、種別の追加が行追加だけで \
                 済まなくなる = Issue #1656）",
                行番号(src, pos)
            ));
        }
    }
    Ok(())
}

/// `project_root.rs` は本番コードよりテストが大きい（境界の検査が主）ので下限を下げる
const ROOT_FLOOR: f64 = 0.20;

#[test]
fn 印の綴りは表の外の本番コードに無い() {
    assert!(
        印の綴り().len() >= 6,
        "表から印が引けていない（{:?}）",
        印の綴り()
    );
    for (rel, floor) in [
        (RUNNER, production_range::MIN_COVERAGE),
        (ROOT, ROOT_FLOOR),
        (DISPATCH, production_range::MIN_COVERAGE),
    ] {
        印の直書きを探す(&read(rel), rel, floor).unwrap_or_else(|e| panic!("{e}"));
    }
}

// --- 規則 3: 新設 2 ファイルに属性の OS 分岐が無い ---

const BANNED_ATTRS: &[&str] = &[
    "#[cfg(windows)]",
    "#[cfg(unix)]",
    "#[cfg(target_os",
    "#[cfg(not(windows))]",
    "#[cfg(not(unix))]",
    "#[cfg_attr(windows",
    "#[cfg_attr(unix",
];

fn 属性分岐を探す(src: &str, rel: &str, floor: f64) -> Result<(), String> {
    let view = code_view(&production_with_floor(src, rel, floor));
    for banned in BANNED_ATTRS {
        if let Some(pos) = view.find(banned) {
            return Err(format!(
                "{rel}:{} に `{banned}` がある。OS 分岐は `Platform` 引数 + `cfg!` で書く\
                 （属性で分けると macOS の単体から Windows の列を検査できない = #1655 / #1656）",
                行番号(src, pos)
            ));
        }
    }
    Ok(())
}

#[test]
fn 新設の2ファイルに属性のos分岐が無い() {
    属性分岐を探す(&read(PROJECT), PROJECT, production_range::MIN_COVERAGE)
        .unwrap_or_else(|e| panic!("{e}"));
    属性分岐を探す(&read(ROOT), ROOT, ROOT_FLOOR).unwrap_or_else(|e| panic!("{e}"));
}

// --- 番犬そのものが効いていることの自己検査（実ファイルへの注入 A/B）---

#[test]
fn 注入した違反をfile_lineで名指しする() {
    // 1. dispatch の片方の腕だけ、プロジェクトを見ない解決へ戻す
    let dispatch = read(DISPATCH);
    let call = "tako_core::resolve_file(";
    let first = dispatch
        .find(call)
        .expect("dispatch に入口の呼び出しがある");
    let mut injected = dispatch.clone();
    injected.replace_range(first..first + call.len(), "tako_core::resolve_legacy(");
    let err = dispatchの入口を確かめる(&injected).expect_err("片方の腕の迂回を検出できていない");
    assert!(
        err.contains(&format!("{DISPATCH}:")),
        "名指ししていない: {err}"
    );
    assert!(err.contains("1 回"), "{err}");

    // 2. 再生ボタンの検出を探索なしへ戻す
    let preview = read(PREVIEW);
    let injected = preview.replacen("runner::resolve_file(", "runner::resolve_legacy(", 1);
    let err =
        再生ボタンの入口を確かめる(&injected).expect_err("再生ボタンの迂回を検出できていない");
    assert!(
        err.contains(&format!("{PREVIEW}:")),
        "名指ししていない: {err}"
    );

    // 3. 探索を切る入口を本番コードで呼ぶ（コンパイルは通る形）
    let injected = dispatch.replacen(
        call,
        "tako_core::resolve_file_in(tako_core::platform::support::Platform::current(), ",
        1,
    );
    let err =
        探索を切る入口を探す(&injected, DISPATCH).expect_err("探索を切る形を検出できていない");
    let line = 行番号(&injected, injected.find("resolve_file_in(").unwrap());
    assert!(
        err.contains(&format!("{DISPATCH}:{line}")),
        "行まで名指ししていない: {err}"
    );
    // 説明文に綴りがあるだけでは落ちない（このファイルの doc がその形）
    探索を切る入口を探す(
        &format!("// resolve_file_in( は使わない\n{dispatch}"),
        DISPATCH,
    )
    .expect("コメントの中の綴りを違反として拾っている");

    // 4. 印を表の外（runner.rs の本番領域）へ書く
    let runner = read(RUNNER);
    let anchor = "pub fn resolve_file(";
    assert!(runner.contains(anchor), "入口の綴りが変わった: {RUNNER}");
    let injected = runner.replacen(
        anchor,
        "const _CARGO: &str = \"Cargo.toml\";\npub fn resolve_file(",
        1,
    );
    let err = 印の直書きを探す(&injected, RUNNER, production_range::MIN_COVERAGE)
        .expect_err("印の直書きを検出できていない");
    assert!(
        err.contains(&format!("{RUNNER}:")),
        "名指ししていない: {err}"
    );
    // 同じ綴りをテスト領域へ書くのは違反ではない（期待値として正しく存在しうる）
    let test_anchor = "fn 出典の名前() {";
    assert!(
        runner.contains(test_anchor),
        "対照の置き場が変わった: {RUNNER}"
    );
    let in_tests = runner.replacen(
        test_anchor,
        &format!("{test_anchor}\n        let _ = \"Cargo.toml\";"),
        1,
    );
    印の直書きを探す(&in_tests, RUNNER, production_range::MIN_COVERAGE)
        .expect("テスト領域の綴りを違反として拾っている");
    印の直書きを探す(&runner, RUNNER, production_range::MIN_COVERAGE)
        .expect("本番領域が汚れている");

    // 5. 属性の cfg を表へ撒く
    let project = read(PROJECT);
    let anchor = "pub fn detect(";
    let injected = project.replacen(anchor, &format!("#[cfg(windows)]\n{anchor}"), 1);
    let err = 属性分岐を探す(&injected, PROJECT, production_range::MIN_COVERAGE)
        .expect_err("属性の注入を検出できていない");
    assert!(
        err.contains(&format!("{PROJECT}:")),
        "名指ししていない: {err}"
    );
    // テスト領域の `#[cfg(unix)]`（project_root.rs のシンボリックリンクのテスト）は違反ではない
    let root = read(ROOT);
    assert!(root.contains("#[cfg(unix)]"), "5 の対照が意味を失っている");
    属性分岐を探す(&root, ROOT, ROOT_FLOOR).expect("テスト領域の cfg を違反として拾っている");
}

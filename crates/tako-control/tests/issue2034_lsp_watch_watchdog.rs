//! #2034 の番犬: tako の外で変わったファイルの監視の方針を機械で固定する
//!
//! 1. **走査しない**（Issue の約束。#2011 の軽量化と衝突させない）: 監視のモジュール
//!    （`tako_core::lsp::watch` / `tako_control::lsp::watch`）の本番コードに、ディレクトリを辿る
//!    呼び出し（`read_dir` / `WalkDir` / `ignore` の walker）を書かない。変化は OS のイベントだけで知る
//! 2. **OS の監視を作るのは 1 か所**: LSP のモジュールで `notify` の監視（`recommended_watcher` /
//!    `RecommendedWatcher`）を作るのは `tako_control::lsp::watch` だけ（= manager 全体で 1 つ。
//!    サーバごとに作るとスレッドがサーバの本数に比例する）
//!
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域とコメントは見ない）。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments_checked;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const CORE_WATCH: &str = "crates/tako-core/src/lsp/watch.rs";
const CONTROL_WATCH: &str = "crates/tako-control/src/lsp/watch.rs";

/// 本番コードだけ（テスト領域とコメントを空白へ潰す。バイト長と行番号は原文のまま）
fn production(src: &str, rel: &str) -> String {
    without_comments_checked(&production_range::scan(src).text, rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

/// `needles` が本番コードに現れた所を `file:line` で
fn hits(rel: &str, src: &str, needles: &[&str]) -> Vec<String> {
    let code = production(src, rel);
    let mut out = Vec::new();
    for needle in needles {
        for (at, _) in code.match_indices(needle) {
            out.push(format!("{rel}:{} に {needle}", line_of(&code, at)));
        }
    }
    out.sort();
    out
}

const WALKERS: &[&str] = &[
    "read_dir(",
    "WalkDir",
    "walkdir::",
    "ignore::Walk",
    "WalkBuilder",
];
const WATCHERS: &[&str] = &["recommended_watcher(", "RecommendedWatcher"];

/// LSP のモジュール（`(相対パス, 中身)`）
fn lsp_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["crates/tako-core/src/lsp", "crates/tako-control/src/lsp"] {
        let mut entries: Vec<_> = std::fs::read_dir(repo_root().join(dir))
            .unwrap_or_else(|e| panic!("{dir} を読めない: {e}"))
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        entries.sort();
        for path in entries {
            let rel = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
            out.push((rel, std::fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(
        out.iter().any(|(rel, _)| rel == CORE_WATCH)
            && out.iter().any(|(rel, _)| rel == CONTROL_WATCH),
        "監視のモジュールが見つからない"
    );
    out
}

#[test]
fn 監視のモジュールはディレクトリを辿らない() {
    let found: Vec<String> = [CORE_WATCH, CONTROL_WATCH]
        .iter()
        .flat_map(|rel| hits(rel, &read(rel), WALKERS))
        .collect();
    assert!(
        found.is_empty(),
        "監視は OS のイベントだけで知る（走査しない = #2034 / #2011）: {found:?}"
    );
}

#[test]
fn os_の監視を作るのは_control_の_watch_だけ() {
    let mut found = Vec::new();
    for (rel, src) in lsp_sources() {
        if rel == CONTROL_WATCH {
            assert!(
                !hits(&rel, &src, WATCHERS).is_empty(),
                "{rel} が監視を作っていない（走査が空振りしている）"
            );
            continue;
        }
        found.extend(hits(&rel, &src, WATCHERS));
    }
    assert!(
        found.is_empty(),
        "OS の監視は manager 全体で 1 つ（tako_control::lsp::watch）: {found:?}"
    );
}

/// 検出力: 走査と監視の作成を植えると file:line で名指す（テストとコメントの中は見ない）
#[test]
fn 植えた走査と監視を名指せる() {
    let planted = "fn f(dir: &Path) {\n    for e in std::fs::read_dir(dir) {}\n    let w = notify::recommended_watcher(|_| {});\n}\n";
    assert_eq!(
        hits("x.rs", planted, WALKERS),
        vec!["x.rs:2 に read_dir(".to_string()]
    );
    assert_eq!(
        hits("x.rs", planted, WATCHERS),
        vec!["x.rs:3 に recommended_watcher(".to_string()]
    );
    let ignored = "// std::fs::read_dir(dir)\nfn f() {}\n#[cfg(test)]\nmod tests {\n    fn g() { std::fs::read_dir(\"x\"); }\n}\n";
    assert!(hits("x.rs", ignored, WALKERS).is_empty());
}

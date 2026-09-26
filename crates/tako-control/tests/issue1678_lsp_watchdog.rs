//! #1678 の番犬: LSP 基盤の方針を機械で固定する
//!
//! 1. **tokio を持ち込まない**: `Cargo.lock` に `name = "tokio"` が 0 件
//!    （`lsp-types` は型定義だけで tokio を引かない。`async-lsp` は既定で引く）
//! 2. **表以外に言語名を書かない**: 検出表（`tako_core::lsp::servers::SERVERS`）の値
//!    （ID・実行ファイル名・languageId・拡張子・ルートの印・導入コマンド）が、LSP の
//!    モジュールに文字列として現れたら file:line で名指す。言語の追加が「表への行追加だけ」で
//!    済む形を、表の行数を数える単体（`servers.rs` の `段階1は4行`）と対で守る
//! 3. **診断ログに本文を出さない**（AGENTS.md の絶対ルール。LSP の I/O はソースコードを
//!    含む）: LSP のモジュールで `println!` / `eprintln!` / `dbg!` を使わない。
//!    `persist_log` / `perf_log` / `flow_log` の引数に本文を運ぶ名前
//!    （`text` / `params` / `message` / `body` / `bytes` / `result` / `changes`）を書かない
//!
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域だけを空白へ潰す）。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments_checked;
use std::path::{Path, PathBuf};
use tako_core::lsp::servers::SERVERS;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 走査する LSP のモジュール（検出表そのもの = `servers.rs` は除く）
fn lsp_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["crates/tako-core/src/lsp", "crates/tako-control/src/lsp"] {
        let mut entries: Vec<_> = std::fs::read_dir(repo_root().join(dir))
            .unwrap_or_else(|e| panic!("{dir} を読めない: {e}"))
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .filter(|p| p.file_name().is_some_and(|n| n != "servers.rs"))
            .collect();
        entries.sort();
        for path in entries {
            let rel = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
            let src = std::fs::read_to_string(&path).unwrap();
            out.push((rel, src));
        }
    }
    assert!(
        out.len() >= 9,
        "LSP のモジュールが見つからない: {}",
        out.len()
    );
    out
}

/// 本番コードだけ（テスト領域とコメントを空白へ潰す。バイト長と行番号は原文のまま）
fn production(src: &str, rel: &str) -> String {
    without_comments_checked(&production_range::scan(src).text, rel)
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

// --- 1. tokio ------------------------------------------------------------------

fn tokio_entries(lock: &str) -> usize {
    lock.lines()
        .filter(|l| l.trim() == "name = \"tokio\"")
        .count()
}

#[test]
fn cargo_lock_に_tokio_が無い() {
    let lock = std::fs::read_to_string(repo_root().join("Cargo.lock")).unwrap();
    assert_eq!(
        tokio_entries(&lock),
        0,
        "Cargo.lock に tokio が入った（tako は tokio を持ち込まない。.agent/architecture.md「IPC トランスポート」）"
    );
    // 検出力: 入っていれば数えられる
    assert_eq!(
        tokio_entries("[[package]]\nname = \"tokio\"\nversion = \"1\"\n"),
        1
    );
}

// --- 2. 表以外に言語名を書かない ----------------------------------------------------

/// 検出表の値（文字列リテラルとして現れてはいけないもの）
fn table_values() -> Vec<&'static str> {
    let mut values = Vec::new();
    for spec in SERVERS {
        values.push(spec.id);
        values.push(spec.program);
        values.extend(
            spec.documents
                .iter()
                .flat_map(|d| [d.extension, d.language_id]),
        );
        values.extend(spec.root_markers.iter().copied());
        values.push(spec.install.macos);
        values.push(spec.install.windows);
        if let Some(ws) = spec.workspace_root {
            values.push(ws.file);
            values.push(ws.contains);
        }
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn language_literals(rel: &str, src: &str, values: &[&str]) -> Vec<String> {
    let text = production(src, rel);
    let mut hits = Vec::new();
    for value in values {
        let needle = format!("\"{value}\"");
        for (at, _) in text.match_indices(&needle) {
            hits.push(format!("{rel}:{} に表の値 {needle}", line_of(&text, at)));
        }
    }
    hits
}

#[test]
fn lsp_のモジュールに表の値を書かない() {
    let values = table_values();
    let hits: Vec<String> = lsp_sources()
        .iter()
        .flat_map(|(rel, src)| language_literals(rel, src, &values))
        .collect();
    assert!(
        hits.is_empty(),
        "言語・サーバ固有の値は検出表（tako_core::lsp::servers::SERVERS）の行に置く:\n{}",
        hits.join("\n")
    );
}

#[test]
fn 表の値の直書きを名指せる() {
    let values = table_values();
    let src = "fn f(id: &str) -> bool {\n    id == \"rust-analyzer\"\n}\n";
    let hits = language_literals("sample.rs", src, &values);
    assert_eq!(
        hits,
        vec!["sample.rs:2 に表の値 \"rust-analyzer\"".to_string()]
    );
    // テストの中とコメントは対象外
    let src = "// \"rust-analyzer\"\nfn f() {}\n#[cfg(test)]\nmod tests {\n    const X: &str = \"rust-analyzer\";\n}\n";
    assert!(language_literals("sample.rs", src, &values).is_empty());
}

// --- 3. 診断ログに本文を出さない ------------------------------------------------------

const LOG_CALLS: &[&str] = &["persist_log(", "perf_log(", "flow_log("];
const BODY_WORDS: &[&str] = &[
    "text", "params", "message", "body", "bytes", "result", "changes",
];
const PRINT_MACROS: &[&str] = &["println!", "eprintln!", "dbg!", "print!", "eprint!"];

/// 開き括弧の直後から、対応する閉じ括弧までの中身
fn call_args(text: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, ch) in text[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

fn has_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + word.len()..].chars().next();
        let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !ident(before) && !ident(after)
    })
}

fn log_leaks(rel: &str, src: &str) -> Vec<String> {
    let text = production(src, rel);
    let mut hits = Vec::new();
    for mac in PRINT_MACROS {
        for (at, _) in text.match_indices(mac) {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            hits.push(format!(
                "{rel}:{} で {mac}（stderr / stdout は診断ログ）",
                line_of(&text, at)
            ));
        }
    }
    for call in LOG_CALLS {
        for (at, _) in text.match_indices(call) {
            let args = call_args(&text, at + call.len() - 1);
            // 書式文字列の中の `{text}` 等も拾えるよう、原文（文字列を潰していない）で見る
            let raw = call_args(src, at + call.len() - 1);
            for word in BODY_WORDS {
                if has_word(args, word) || raw.contains(&format!("{{{word}")) {
                    hits.push(format!(
                        "{rel}:{} の {call}…) に本文を運ぶ名前 `{word}`",
                        line_of(&text, at)
                    ));
                }
            }
        }
    }
    hits
}

#[test]
fn lsp_のモジュールは診断ログへ本文を出さない() {
    let hits: Vec<String> = lsp_sources()
        .iter()
        .flat_map(|(rel, src)| log_leaks(rel, src))
        .collect();
    assert!(
        hits.is_empty(),
        "LSP の I/O はソースコード本文を含むので診断ログへ出さない（method 名・所要・回数だけ）:\n{}",
        hits.join("\n")
    );
}

#[test]
fn 本文をログへ出す形を名指せる() {
    let src = "fn f(text: &str) {\n    crate::diag::persist_log(&format!(\"x={}\", text));\n}\n";
    assert_eq!(
        log_leaks("sample.rs", src),
        vec!["sample.rs:2 の persist_log(…) に本文を運ぶ名前 `text`".to_string()]
    );
    let src = "fn f(params: &str) {\n    perf_log(&format!(\"x={params}\"));\n}\n";
    assert_eq!(log_leaks("sample.rs", src).len(), 1);
    let src = "fn f() {\n    eprintln!(\"x\");\n}\n";
    assert_eq!(log_leaks("sample.rs", src).len(), 1);
    // 名前の一部（`context` / `texts_len`）は当たらない
    let src = "fn f() {\n    perf_log(&format!(\"{}\", context.len()));\n}\n";
    assert!(log_leaks("sample.rs", src).is_empty());
}

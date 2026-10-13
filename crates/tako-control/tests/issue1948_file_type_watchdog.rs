//! #1948 の番犬: 言語の同定の正本を `tako_core::file_type` の 1 か所に保つ
//!
//! 1. **拡張子で分岐しない**: 色分け（`tako-app/src/preview.rs`）・インデント
//!    （`tako-core/src/text_edit.rs`）・プレビューの種別（`tako-core/src/open_plan.rs`）の
//!    本番コードに、表の拡張子・ファイル名・コードブロックの別名を文字列リテラルにした
//!    分岐（`match` の腕・`|` の並び・`==` / `eq_ignore_ascii_case` の比較・リテラルの配列）が
//!    残ったら file:line で名指す。言語を足すのが「表に 1 行」で済む形を守る
//! 2. **挙動の表が名指す形式は表に行がある**: 言語サーバ（`lsp::servers::SERVERS`）・
//!    実行の既定（`platform::runner_defaults::TABLE`）・実行のプロジェクト
//!    （`runner_project::KINDS`）の拡張子とファイル名が `file_type` に無ければ、
//!    その表の file:line で名指す（= tako が知っている形式の一覧は `file_type` の 1 か所）
//!
//! 挙動の表そのものは分けたまま（#1726 設計 §9.2）。設計は
//! `.agent/plans/2026-10-language-coverage.md` §4。
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域だけを空白へ潰す）。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments_checked;
use std::path::{Path, PathBuf};
use tako_core::file_type::{self, TABLE};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけ（テスト領域とコメントを空白へ潰す。バイト長と行番号は原文のまま）
fn production(src: &str, rel: &str) -> String {
    without_comments_checked(&production_range::production(src, rel), rel)
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

// --- 1. 拡張子で分岐しない ------------------------------------------------------------

/// 同定を `file_type` へ寄せた利用者（ここに拡張子の分岐を書かない）
const CONSUMERS: &[&str] = &[
    "crates/tako-app/src/preview.rs",
    "crates/tako-core/src/text_edit.rs",
    "crates/tako-core/src/open_plan.rs",
];

/// 表が持つ綴り（拡張子・ファイル名・コードブロックの別名。小文字）
fn table_words() -> Vec<String> {
    let mut words: Vec<String> = TABLE
        .iter()
        .flat_map(|row| {
            row.extensions
                .iter()
                .chain(row.file_names)
                .chain(row.fence_tokens)
                .map(|w| w.to_ascii_lowercase())
        })
        .collect();
    words.sort();
    words.dedup();
    words
}

/// `"md"` が分岐の位置にあるか（`match` の腕・`|` の並び・比較・リテラルの配列の先頭）
fn in_branch_position(before: &str, after: &str) -> bool {
    let before = before.trim_end();
    let after = after.trim_start();
    let or_pattern = |s: &str, at_end: bool| {
        if at_end {
            s.ends_with('|') && !s.ends_with("||")
        } else {
            s.starts_with('|') && !s.starts_with("||")
        }
    };
    after.starts_with("=>")
        || after.starts_with("if ")
        || or_pattern(after, false)
        || or_pattern(before, true)
        || after.starts_with("==")
        || after.starts_with("!=")
        || before.ends_with("==")
        || before.ends_with("!=")
        || before.ends_with("eq_ignore_ascii_case(")
        || (before.ends_with('[') && (after.starts_with(',') || after.starts_with(']')))
}

/// 本番コードの拡張子の分岐を `rel:line` で返す
fn extension_branches(rel: &str, src: &str, words: &[String]) -> Vec<String> {
    let text = production(src, rel);
    let lower = text.to_ascii_lowercase();
    let mut hits = Vec::new();
    for word in words {
        let needle = format!("\"{word}\"");
        for (at, _) in lower.match_indices(&needle) {
            let end = at + needle.len();
            if in_branch_position(&text[..at], &text[end..]) {
                hits.push(format!(
                    "{rel}:{} で {} の分岐",
                    line_of(&text, at),
                    &text[at..end]
                ));
            }
        }
    }
    hits.sort();
    hits
}

#[test]
fn 同定の利用者に拡張子の分岐を書かない() {
    let words = table_words();
    assert!(words.len() >= 60, "表の綴りが縮退している: {}", words.len());
    let hits: Vec<String> = CONSUMERS
        .iter()
        .flat_map(|rel| extension_branches(rel, &read(rel), &words))
        .collect();
    assert!(
        hits.is_empty(),
        "拡張子・ファイル名で決める判断は言語の同定の正本（tako_core::file_type::TABLE）の行に置き、\
         利用者は `file_type::identify` / `by_extension` を引く（#1948）:\n{}",
        hits.join("\n")
    );
}

/// 検出力: 寄せる前の形（`d4b83de` の 3 ファイルにあった書き方）をそれぞれ名指せる
#[test]
fn 拡張子の分岐の書き方をそれぞれ名指せる() {
    let words = table_words();
    let cases = [
        // match の腕（open_plan の種別・preview の補い）
        ("fn f(e: &str) -> u8 {\n    match e {\n        \"md\" | \"markdown\" => 1,\n        _ => 0,\n    }\n}\n", 3),
        // 1 つだけの腕
        ("fn f(e: &str) -> u8 {\n    match e {\n        \"png\" => 1,\n        _ => 0,\n    }\n}\n", 3),
        // matches! の並び（text_edit のインデント）
        ("fn f(e: &str) -> bool {\n    matches!(e, \"mk\" | \"go\")\n}\n", 2),
        // 大文字小文字を問わない比較（preview の Markdown 判定）
        ("fn f(e: &str) -> bool {\n    e.eq_ignore_ascii_case(\"md\")\n}\n", 2),
        // == の比較
        ("fn f(e: &str) -> bool {\n    e == \"ino\"\n}\n", 2),
        // リテラルの配列（preview のコードブロックの別名）
        ("fn f(t: &str) -> bool {\n    [\"ino\", \"arduino\"].contains(&t)\n}\n", 2),
        // 大文字で書いても同じ（Justfile 等のファイル名）
        ("fn f(n: &str) -> u8 {\n    match n {\n        \"Justfile\" => 1,\n        _ => 0,\n    }\n}\n", 3),
    ];
    for (src, line) in cases {
        let src = format!("{src}{}", filler());
        let hits = extension_branches("sample.rs", &src, &words);
        assert!(
            hits.first()
                .is_some_and(|h| h.starts_with(&format!("sample.rs:{line} "))),
            "名指せない: {src:?} → {hits:?}"
        );
    }
    // 分岐でない使い方・テストの中・コメントは対象外
    let src = format!(
        "// \"md\" => 1\nfn f(p: &mut std::path::PathBuf) {{\n    p.set_extension(\"md\");\n    g(\"go\");\n}}\n\
         #[cfg(test)]\nmod tests {{\n    const X: &str = match 1 {{ _ => \"md\" }};\n    fn t(e: &str) -> bool {{ e == \"md\" }}\n}}\n{}",
        filler()
    );
    assert_eq!(
        extension_branches("sample.rs", &src, &words),
        Vec::<String>::new()
    );
}

/// 走査範囲の下限（`production` は 1000 バイト未満の本番コードを「空振り」として落とす）を
/// 自己検査の小さな例でも越えるための詰め物
fn filler() -> String {
    (0..60)
        .map(|i| format!("fn filler_{i}() -> usize {{ {i} }}\n"))
        .collect()
}

// --- 2. 挙動の表が名指す形式は表に行がある ---------------------------------------------

/// 挙動の表 1 本ぶん（名指す拡張子・ファイル名と、表の置き場）
struct BehaviorTable {
    name: &'static str,
    rel: &'static str,
    extensions: Vec<&'static str>,
    file_names: Vec<&'static str>,
}

fn behavior_tables() -> Vec<BehaviorTable> {
    vec![
        BehaviorTable {
            name: "言語サーバ（lsp::servers::SERVERS）",
            rel: "crates/tako-core/src/lsp/servers.rs",
            extensions: tako_core::lsp::servers::SERVERS
                .iter()
                .flat_map(|s| s.documents.iter().map(|d| d.extension))
                .collect(),
            file_names: Vec::new(),
        },
        BehaviorTable {
            name: "実行の既定（platform::runner_defaults::TABLE）",
            rel: "crates/tako-core/src/platform/runner_defaults.rs",
            extensions: tako_core::platform::runner_defaults::TABLE
                .iter()
                .map(|e| e.ext)
                .collect(),
            file_names: Vec::new(),
        },
        BehaviorTable {
            name: "実行のプロジェクト（runner_project::KINDS）",
            rel: "crates/tako-core/src/runner_project.rs",
            extensions: tako_core::runner_project::KINDS
                .iter()
                .flat_map(|k| k.exts.iter().copied())
                .collect(),
            file_names: tako_core::runner_project::KINDS
                .iter()
                .flat_map(|k| k.file_names.iter().copied())
                .collect(),
        },
    ]
}

/// 表の値が書かれている行（本番コードの最初の出現）。見つからなければ表の置き場だけ
fn place_of(rel: &str, src: &str, value: &str) -> String {
    let text = production(src, rel);
    let needle = format!("\"{value}\"");
    match text.find(&needle) {
        Some(at) => format!("{rel}:{}", line_of(&text, at)),
        None => rel.to_string(),
    }
}

/// `file_type` に行の無い値を、挙動の表の file:line で返す
fn missing_rows(
    table: &BehaviorTable,
    src: &str,
    has_extension: impl Fn(&str) -> bool,
    has_file_name: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for ext in &table.extensions {
        if !has_extension(ext) {
            out.push(format!(
                "{} {} の拡張子 \"{ext}\" が file_type に無い",
                place_of(table.rel, src, ext),
                table.name
            ));
        }
    }
    for name in &table.file_names {
        if !has_file_name(name) {
            out.push(format!(
                "{} {} のファイル名 \"{name}\" が file_type に無い",
                place_of(table.rel, src, name),
                table.name
            ));
        }
    }
    out
}

#[test]
fn 挙動の表が名指す形式はfile_typeに行がある() {
    let mut found = Vec::new();
    let mut counted = 0usize;
    for table in behavior_tables() {
        counted += table.extensions.len() + table.file_names.len();
        found.extend(missing_rows(
            &table,
            &read(table.rel),
            |ext| file_type::by_extension(ext).is_some(),
            |name| file_type::by_file_name(name).is_some(),
        ));
    }
    // 3 本とも空でない（読み先の取り違えで黙って緑にならない）
    assert!(counted >= 80, "挙動の表の値が縮退している: {counted}");
    assert!(
        found.is_empty(),
        "挙動の表が名指す形式は言語の同定の正本（tako_core::file_type::TABLE）にも 1 行足す（#1948）:\n{}",
        found.join("\n")
    );
}

/// 検出力: 表に無い拡張子・ファイル名を、挙動の表の file:line で名指せる
#[test]
fn 表に無い形式を挙動の表の行で名指せる() {
    let table = BehaviorTable {
        name: "実行の既定（試験）",
        rel: "sample.rs",
        extensions: vec!["rs", "zzz"],
        file_names: vec!["Makefile", "Zzzfile"],
    };
    let src = format!(
        "const T: &[(&str, &str)] = &[\n    (\"rs\", \"cargo run\"),\n    (\"zzz\", \"zzz run\"),\n];\n\
         const N: [&str; 2] = [\"Makefile\", \"Zzzfile\"];\n{}",
        filler()
    );
    let found = missing_rows(
        &table,
        &src,
        |ext| file_type::by_extension(ext).is_some(),
        |name| file_type::by_file_name(name).is_some(),
    );
    assert_eq!(
        found,
        vec![
            "sample.rs:3 実行の既定（試験） の拡張子 \"zzz\" が file_type に無い".to_string(),
            "sample.rs:5 実行の既定（試験） のファイル名 \"Zzzfile\" が file_type に無い"
                .to_string(),
        ]
    );
}

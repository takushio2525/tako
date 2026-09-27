//! 番犬: **本番の置き場へ届くのは、製品の入口を通ったプロセスだけ**（Issue #1811）
//!
//! `tako_core::paths::is_test_process()` は「`deps/` 配下にある」**または**
//! 「製品の入口（`tako_core::paths::mark_product_process`）を通っていない」ときに真になる。
//! 後者が #1811 で足した安全側の規則で、テストバイナリを `deps/` の外へコピー・改名して
//! 走らせても本番の data dir / ホーム配下の設定 / 実 agent CLI に届かない。
//!
//! この規則は宣言の置き場所で両方向に壊れるので、ここで固定する:
//!
//! 1. 製品の `main` が宣言を**忘れる / 1 文目より後に置く**と、製品がテスト扱いになり
//!    本番の layout.json / settings.json を読まない（逆向きの誤判定。ユーザーには
//!    「tako が空になった」に見える）。1 文目でないと `is_verification_process` の
//!    `OnceLock` が宣言前の判定を覚える
//! 2. 製品の入口**以外**（テスト・ライブラリ・検証用ツール）が宣言すると、そのプロセスは
//!    `deps/` の外で本番へ書ける = #1811 の穴が開き直る
//! 3. 実行ファイルの入口が**増えたのに分類されていない**と、1 と 2 のどちらに倒すべきかを
//!    誰も決めないまま出荷される
//!
//! 実行時の側（製品の `tako` が本番相当の置き場を解決する / `deps/` の外のテストバイナリが
//! 何も書かない）は `crates/tako-cli/tests/issue1811_product_not_test.rs` と
//! `tako_control::test_write_isolation` が実プロセスで見る。

use std::path::Path;

#[path = "common/emoji_scan.rs"]
mod emoji_scan;

// `code_view` は `emoji_scan` が公開している 1 つを借りる（二重に宣言すると `duplicate_mod`）
use emoji_scan::production_range::code_view;

/// 宣言の呼び出し（`code_view` で文字列とコメントを潰した眺めで探す）
const MARK_CALL: &str = "mark_product_process(";

/// 製品の `main` の 1 文目に置く文
const MARK_STMT: &str = "tako_core::paths::mark_product_process();";

/// 製品の入口（配布物に入るバイナリの `main`）。`scripts/build-app.sh` と Windows
/// インストーラが名指しで同梱するのはこの 2 つだけ
const PRODUCT_ENTRIES: &[&str] = &["crates/tako-app/src/main.rs", "crates/tako-cli/src/main.rs"];

/// 製品ではない実行ファイルの入口（**宣言しない = テスト扱いのまま**）と、その理由
const NOT_PRODUCT: &[(&str, &str)] = &[
    (
        "crates/tako-control/examples/mcp_host.rs",
        "MCP / IPC の実機検証用ホスト（scripts/verify-claude-mcp.sh）。接続情報は子へ env で \
         渡すので本番の data dir は要らない = 安全側（テスト扱い）のままでよい",
    ),
    (
        "crates/tako-control/src/bin/tako-lsp-fake.rs",
        "テスト専用の偽 LSP サーバ（#1678）。配布物に入らず、tako-core を使わない",
    ),
];

/// `crates/*` の実行ファイルの入口を集める（`src/main.rs` / `src/bin/` / `examples/` /
/// `Cargo.toml` の `[[bin]] path`）。`build.rs` は含めない（ホスト側で走るビルド補助で、
/// build-dependencies に tako のクレートが無い = data dir を解決しない）
fn binary_entries() -> Vec<String> {
    let root = emoji_scan::repo_root();
    let mut out = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ を読める");
    for krate in crates.flatten() {
        let dir = krate.path();
        let rel = |p: &Path| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        };
        if dir.join("src/main.rs").is_file() {
            out.push(rel(&dir.join("src/main.rs")));
        }
        for sub in ["src/bin", "examples"] {
            let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "rs") {
                    out.push(rel(&path));
                } else if path.join("main.rs").is_file() {
                    out.push(rel(&path.join("main.rs")));
                }
            }
        }
        // `[[bin]]` の `path = "…"`（src/main.rs 以外を指す形も拾う）
        if let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            let mut in_bin = false;
            for line in manifest.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_bin = line == "[[bin]]";
                } else if in_bin {
                    if let Some(value) = line.strip_prefix("path") {
                        let value = value.trim_start().trim_start_matches('=').trim();
                        out.push(rel(&dir.join(value.trim_matches('"'))));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 桁 0 の `fn main` の本体（`{` の直後から）を、コメントを落とした眺めで返す
fn main_body(rel: &str) -> String {
    let src = std::fs::read_to_string(emoji_scan::repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    let view = code_view::without_comments_checked(&src, rel);
    let mut offset = 0;
    for line in view.split_inclusive('\n') {
        if tako_core::source_scan::is_top_level_fn_head(line)
            && tako_core::source_scan::fn_head_name(line) == Some("main")
        {
            let rest = &view[offset..];
            let open = rest
                .find('{')
                .unwrap_or_else(|| panic!("{rel}: fn main の本体の `{{` が無い"));
            return rest[open + 1..].to_string();
        }
        offset += line.len();
    }
    panic!("{rel}: 桁 0 の fn main が無い（入口の一覧か走査が壊れている）");
}

/// 1: 製品の `main` は **1 文目で** 製品と名乗る
#[test]
fn 製品のmainは1文目で製品と名乗る() {
    for rel in PRODUCT_ENTRIES {
        let body = main_body(rel);
        let first = body.trim_start();
        assert!(
            first.starts_with(MARK_STMT),
            "{rel}: fn main の 1 文目が `{MARK_STMT}` ではない（#1811）。\n\
             宣言が無い / 後ろにあると、製品がテスト扱いになり本番の layout.json・\
             settings.json を読まない。先頭:\n{}",
            first.lines().take(3).collect::<Vec<_>>().join("\n")
        );
    }
}

/// 2: 製品と名乗るのは製品の `main` の 1 文目だけ（テスト・ライブラリ・検証用ツールは名乗らない）
#[test]
fn 製品と名乗るのは製品の入口だけ() {
    let mut calls = Vec::new();
    let mut definitions = 0;
    for (rel, src) in emoji_scan::sources_under(&["crates"]) {
        // 自分の説明文・期待値の文字列を拾わないよう、コメントと文字列を潰した眺めで探す。
        // 見たいのは「実際に呼んでいるか」なので、doc コメントの言及は違反ではない
        let code = code_view::code_view(&src);
        for (idx, line) in code.lines().enumerate() {
            if !line.contains(MARK_CALL) {
                continue;
            }
            if line.contains(&format!("fn {MARK_CALL}")) {
                definitions += 1;
                continue;
            }
            calls.push((rel.clone(), idx + 1, line.trim().to_string()));
        }
    }
    assert_eq!(
        definitions, 1,
        "mark_product_process の定義が 1 つではない（走査が空振りしている / 改名した）"
    );
    let stray: Vec<String> = calls
        .iter()
        .filter(|(rel, _, _)| !PRODUCT_ENTRIES.contains(&rel.as_str()))
        .map(|(rel, line, text)| format!("{rel}:{line}: {text}"))
        .collect();
    assert!(
        stray.is_empty(),
        "製品の入口以外が製品と名乗っている（#1811）。そのプロセスは deps/ の外で\n\
         本番の data dir / ホーム配下へ書ける。名乗ってよいのは {PRODUCT_ENTRIES:?} の \
         fn main の 1 文目だけ:\n{}",
        stray.join("\n")
    );
    for rel in PRODUCT_ENTRIES {
        let n = calls.iter().filter(|(r, _, _)| r == rel).count();
        assert_eq!(
            n, 1,
            "{rel}: 製品の宣言が {n} 回ある（1 文目の 1 回だけにする）"
        );
    }
}

/// 3: 実行ファイルの入口はすべて「製品」か「製品ではない（理由つき）」に分類されている
#[test]
fn 実行ファイルの入口はすべて分類されている() {
    let entries = binary_entries();
    let known = |rel: &str| {
        PRODUCT_ENTRIES.contains(&rel) || NOT_PRODUCT.iter().any(|(path, _)| *path == rel)
    };
    let unknown: Vec<&String> = entries.iter().filter(|rel| !known(rel)).collect();
    assert!(
        unknown.is_empty(),
        "分類されていない実行ファイルの入口がある（#1811）: {unknown:?}\n\
         配布物に入る製品なら fn main の 1 文目で `{MARK_STMT}` を呼び PRODUCT_ENTRIES へ、\n\
         検証・テスト用なら宣言せず NOT_PRODUCT へ理由つきで載せる"
    );
    // 一覧の側が古くなっていない（消えた入口を載せたままにしない）
    for rel in PRODUCT_ENTRIES
        .iter()
        .copied()
        .chain(NOT_PRODUCT.iter().map(|(path, _)| *path))
    {
        assert!(
            entries.iter().any(|e| e == rel),
            "{rel} は実行ファイルの入口として見つからない（一覧が古い / 走査が空振り）: {entries:?}"
        );
    }
    for (rel, reason) in NOT_PRODUCT {
        assert!(!reason.trim().is_empty(), "{rel}: 製品ではない理由が空");
    }
}

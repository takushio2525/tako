//! **#1556 の番犬**: セルフテストの cargo はカレントディレクトリに依らず、リポジトリの根で回す。
//!
//! ## なぜ縛るのか
//!
//! セルフテスト（`TAKO_SELF_TEST=1`）の項目 14「tako CLI のビルド」は、以前 `cargo build -p tako-cli`
//! を**カレントディレクトリのまま**起こしていた。リポジトリの外（scratchpad 等）から起動すると
//! `error: could not find Cargo.toml in <cwd>` → `TAKO_APP_SELF_TEST_FAILED: tako CLI のビルド` で
//! 終わり、「リポジトリの根から起動する」という前提がどこにも出なかった。いまは根を実行ファイルの
//! 位置から辿って（1 実装は `crates/tako-app/src/self_test/repo_root_1556.rs`）そこで cargo を回し、
//! 辿れなければビルドせずに表示言語の案内で止まる。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. セルフテスト（main.rs の `mod self_test` と `src/self_test/*.rs`）が起こす cargo は、
//!    起こす（`.status()` / `.output()` / `.spawn()`）より前に `.current_dir(` で dir を決める
//! 2. 項目 14 は `repo_root_1556::cli_build_dir()` で dir を決め、根が無い（`CliBuildDir::Missing`）
//!    ときは `fail(` で案内を出して止まる（cwd のまま回して cargo の英語の 1 行で終わらせない）
//! 3. 根の解決は 1 実装: main.rs の `mod self_test` は目印（`crates/tako-app/Cargo.toml`）を
//!    自分で探さず、`repo_root_1556.rs` はビルド時のパス（`env!(`）を使わない（#1848）

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const SELF_TEST_DIR: &str = "crates/tako-app/src/self_test";
const RESOLVER: &str = "crates/tako-app/src/self_test/repo_root_1556.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号がずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す = 行番号はずれない）
fn code_of(src: &str, rel: &str, floor: bool) -> String {
    let production = if floor {
        production_range::production(src, rel)
    } else {
        production_range::scan(src).text
    };
    production_range::code_view::without_comments(&production)
}

fn read_raw(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// main.rs の `mod self_test {` から行頭の閉じ括弧までの範囲（バイト位置）
fn self_test_span(src: &str) -> (usize, usize) {
    let open = src
        .find("\nmod self_test {\n")
        .map(|i| i + 1)
        .unwrap_or_else(|| {
            panic!("{MAIN} に `mod self_test {{` が無い（構造が変わったらこの番犬も見直すこと）")
        });
    let close = src[open..]
        .find("\n}\n")
        .map(|i| open + i + 1)
        .unwrap_or_else(|| panic!("{MAIN} の `mod self_test` の閉じ括弧が無い"));
    (open, close)
}

/// cargo を起こす綴り
const CARGO_NEW: &str = "Command::new(\"cargo\")";
/// 起こす呼び出し（ここまでに dir を決めていなければ cwd のまま回る）
const RUNNERS: [&str; 3] = [".status()", ".output()", ".spawn()"];

/// `code` の `range` の中で cargo を起こす箇所のうち、起こす前に `.current_dir(` が無いものを
/// `rel:line` で返す（1 の本体。合成した入力でも検出力を確かめる）
fn cargo_without_dir(code: &str, rel: &str, range: std::ops::Range<usize>) -> Vec<String> {
    let mut out = Vec::new();
    let hay = &code[range.clone()];
    for (i, _) in hay.match_indices(CARGO_NEW) {
        let at = range.start + i;
        let rest = &code[at..range.end];
        let runner = RUNNERS.iter().filter_map(|r| rest.find(r)).min();
        let window = &rest[..runner.unwrap_or(rest.len())];
        if runner.is_none() || !window.contains(".current_dir(") {
            out.push(format!(
                "{rel}:{} セルフテストが cargo をカレントディレクトリのまま起こしている\
                 （リポジトリの外から起動すると `could not find Cargo.toml` で落ちる = #1556 が戻る）。\
                 起こす前に `.current_dir(<根>)` で dir を決める（根は `repo_root_1556::cli_build_dir()`）",
                line_at(code, at)
            ));
        }
    }
    out
}

/// 1: セルフテストの cargo は起こす前に dir を決める
#[test]
fn issue1556_セルフテストのcargoはリポジトリの根で回す() {
    let src = read_raw(MAIN);
    let code = code_of(&src, MAIN, true);
    let (start, end) = self_test_span(&code);
    let mut bad = cargo_without_dir(&code, MAIN, start..end);
    let mut seen_cargo = code[start..end].matches(CARGO_NEW).count();

    let dir = repo_root().join(SELF_TEST_DIR);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{SELF_TEST_DIR} を読めない: {e}"))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();
    for path in files {
        let rel = format!(
            "{SELF_TEST_DIR}/{}",
            path.file_name().unwrap().to_string_lossy()
        );
        let code = code_of(&read_raw(&rel), &rel, false);
        seen_cargo += code.matches(CARGO_NEW).count();
        bad.extend(cargo_without_dir(&code, &rel, 0..code.len()));
    }
    // 項目 14 の 1 本は必ずある = 走査が空振りしていない
    assert!(
        seen_cargo >= 1,
        "{MAIN} の `mod self_test` に `{CARGO_NEW}` が 1 つも無い（項目 14 を消した・綴りを変えたならこの番犬も見直すこと）"
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// 2: 項目 14 は根を決めてから回し、見つからなければ案内で止まる
#[test]
fn issue1556_項目14は根を決めてから回し見つからなければ案内で止まる() {
    let src = read_raw(MAIN);
    let code = code_of(&src, MAIN, true);
    let (start, end) = self_test_span(&code);
    let args = code[start..end]
        .find("\"-p\", \"tako-cli\"")
        .map(|i| start + i)
        .unwrap_or_else(|| {
            panic!("{MAIN} の `mod self_test` に項目 14 の `\"-p\", \"tako-cli\"` が無い（この番犬も見直すこと）")
        });
    let cargo_at = code[start..args]
        .rfind(CARGO_NEW)
        .map(|i| start + i)
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} 項目 14 の cargo の起こし方が変わった",
                line_at(&code, args)
            )
        });
    let run_at = code[args..end]
        .find(".status()")
        .map(|i| args + i)
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} 項目 14 の `.status()` が無い",
                line_at(&code, args)
            )
        });
    let item14 = &code[cargo_at..run_at];
    let line = line_at(&code, cargo_at);
    assert!(
        item14.contains("repo_root_1556::cli_build_dir()"),
        "{MAIN}:{line} 項目 14 が cargo の dir を `repo_root_1556::cli_build_dir()` で決めていない\
         （cwd に依存する形へ戻った = #1556）"
    );
    let missing = item14.find("CliBuildDir::Missing(").unwrap_or_else(|| {
        panic!(
            "{MAIN}:{line} 項目 14 が根の見つからない形（`CliBuildDir::Missing`）を扱っていない\
             （cwd のまま回すと cargo の英語の 1 行で終わり、前提が伝わらない = #1556）"
        )
    });
    let arm_end = item14[missing..]
        .find(",\n")
        .map(|i| missing + i)
        .unwrap_or(item14.len());
    assert!(
        item14[missing..arm_end].contains("fail("),
        "{MAIN}:{} 根が見つからないときに `fail(` で案内を出して止まっていない（#1556）",
        line_at(&code, cargo_at + missing)
    );
}

/// 3: 根の解決は 1 実装（main.rs は目印を自分で探さない・解決はビルド時のパスを使わない）
#[test]
fn issue1556_根の解決は1実装でビルド時のパスを使わない() {
    let src = read_raw(MAIN);
    let code = code_of(&src, MAIN, true);
    let (start, end) = self_test_span(&code);
    if let Some(i) = code[start..end].find("crates/tako-app/Cargo.toml") {
        panic!(
            "{MAIN}:{} `mod self_test` が根の目印を自分で探している（根の解決は `repo_root_1556` の 1 実装へ。\
             2 つあると項目 14 と他の項目で根がずれる = #1556）",
            line_at(&code, start + i)
        );
    }
    let resolver = code_of(&read_raw(RESOLVER), RESOLVER, false);
    assert!(
        resolver.contains("crates/tako-app/Cargo.toml"),
        "{RESOLVER} に根の目印が無い（目印を変えたならこの番犬も見直すこと）"
    );
    if let Some(i) = resolver.find("env!(") {
        panic!(
            "{RESOLVER}:{} 根の解決がビルド時のパス（`env!(`）を使っている。ビルド機の絶対パスが release バイナリへ\
             残る（#1848）。実行時の `std::env::var_os(\"CARGO_MANIFEST_DIR\")` を使う",
            line_at(&resolver, i)
        );
    }
}

/// 検出力: 修正前の形（dir を決めずに起こす）を名指し、修正後の形は通す
#[test]
fn issue1556_番犬は修正前の形を名指して修正後の形を通す() {
    let before = "fn x() {\n    let built = std::process::Command::new(\"cargo\")\n        \
                  .args([\"build\", \"-p\", \"tako-cli\", \"--quiet\"])\n        .status();\n}\n";
    let bad = cargo_without_dir(before, "synthetic.rs", 0..before.len());
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("synthetic.rs:2 "), "{bad:?}");

    let after = "fn x() {\n    let mut build = std::process::Command::new(\"cargo\");\n    \
                 build.args([\"build\"]);\n    build.current_dir(root);\n    build.status();\n}\n";
    assert!(cargo_without_dir(after, "synthetic.rs", 0..after.len()).is_empty());

    // 起こす呼び出しの後ろで dir を決めても遅い（その cargo は cwd で回った後）
    let late = "fn x() {\n    let mut b = std::process::Command::new(\"cargo\");\n    \
                b.output();\n    b.current_dir(root);\n}\n";
    assert_eq!(
        cargo_without_dir(late, "synthetic.rs", 0..late.len()).len(),
        1
    );
}

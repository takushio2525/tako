//! 番犬: `ScreenLine` は「行に全角が在るか」の旗を持たない（#1388）
//!
//! ## なぜ止めるのか
//!
//! `ScreenLine::has_wide` の doc は「描画時にセル幅固定レイアウトへ切り替える判定に使う」
//! だったが、#787 の専用 Element がグリッドを**常に**セル幅固定で組むようになった時点で
//! production の読み手が 1 つも無くなり、**フィールドと doc と毎行の `windows(2)` 走査だけ**が
//! 残っていた。実害は嘘の doc（信じて直しても描画は何も変わらない）と、書き手が 4 か所
//! （`screen.rs` / `links.rs` ×2 / `terminal_grid.rs` のテストヘルパ）に分かれ、
//! `windows(2).any(|w| w[1] - w[0] > 1)` 版が**右端の全角を構造的に見落とす**一方で
//! テストヘルパの `chars().any(is_wide)` だけ答えが違ったこと。
//! テストヘルパが production より正しい状態は、使い直すときに嘘の緑を出す。
//!
//! 全角は `cell_cols` が 2 列飛ぶことで表す（旗は要らない）。行の性質が本当に必要に
//! なったら、**読み手を書いてから**フィールドを足す。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 1. [`has_wideという識別子がcratesに残っていない`] — `crates/` 配下の `.rs`
//!    （製品コードもテストも、**コメントも含めて**）に `has_wide` が 1 つも無いこと。
//!    テストヘルパだけ復活すると「production より正しいテスト」の形へ戻るし、
//!    コメントに残った名前はコピペで復活する。経緯の説明は
//!    `.agent/conventions.md`「行の情報は『読み手が在るもの』だけ持つ」節に置く
//! 2. [`行の全角旗をcell_colsから組む形が復活していない`] — 製品コードが
//!    `cell_cols.windows(2)` を `> 1` で畳んで**行 1 本の真偽**にしていないこと
//!    （右端の全角を落とす物差しそのもの。名前を変えて復活しても落ちる）
//! 3. [`ScreenLineのフィールドは3つ`] — 指紋スナップショット。フィールドを増減させたら
//!    落ちるので、「読み手が在るか」を宣言してから足すことになる
//!
//! 挙動そのものは `tako-core` の単体テストが見る
//! （`screen::tests::全角の右隣スペーサーは近道で飛ばさない` /
//! `scroll_mirror::tests::全角文字のセル列が正しい` /
//! `screen::tests::nfdの結合文字はtextに残りcell_colsへ同じ列を積む` が
//! 「全角のぶん `cell_cols` が 2 列飛ぶ」ことで見るので、全角を半角に差し替えると落ちる）。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// この番犬自身（説明文に `has_wide` を書いてある）
const SELF_REL: &str = "crates/tako-control/tests/issue1388_has_wide_watchdog.rs";

/// テストモジュールより手前だけを見る（fixture や期待値の文字列に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付く）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

fn rel_of(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .expect("リポジトリ内")
        .to_string_lossy()
        .replace('\\', "/")
}

/// `crates/` 配下の `.rs` を全部集める（**テストも含む**。`target` は除く）
fn all_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root().join("crates"), &mut out);
    out.sort();
    assert!(out.len() > 20, "走査対象が少なすぎる: {}", out.len());
    out
}

// ------------------------------------------------------- 1. 識別子の再登場

/// `rel` の中の `has_wide` を file:line で並べる（番犬自身の説明文は対象外）
fn offending_identifier(rel: &str, src: &str) -> Vec<String> {
    if rel == SELF_REL {
        return Vec::new();
    }
    src.lines()
        .enumerate()
        .filter(|(_, line)| line.contains("has_wide"))
        .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        .collect()
}

#[test]
fn has_wideという識別子がcratesに残っていない() {
    let mut found = Vec::new();
    for path in all_sources() {
        let rel = rel_of(&path);
        let src = std::fs::read_to_string(&path).expect("ソースを読む");
        found.extend(offending_identifier(&rel, &src));
    }
    assert!(
        found.is_empty(),
        "「行に全角が在るか」の旗が復活している（#1388 で落とした。全角は `cell_cols` が\n\
         2 列飛ぶことで表す。行の性質が要るなら**読み手を書いてから**足す）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1388_注入_識別子の再登場を名指しで落とせる() {
    // 修正前そのものの形（フィールド定義 + 製品コードの書き手）
    let injected = "pub struct ScreenLine {\n    \
        pub cell_cols: Vec<usize>,\n    \
        pub has_wide: bool,\n}\n";
    let got = offending_identifier("crates/tako-core/src/screen.rs", injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":3:"), "行番号が合っていない: {got:?}");

    // テストヘルパだけの復活も落とす（production より正しいテストを作らせない）
    let helper = "fn f() {}\n#[cfg(test)]\nmod tests {\n    \
        let l = ScreenLine { has_wide: text.chars().any(is_wide) };\n}\n";
    let got = offending_identifier("crates/tako-app/src/terminal_grid.rs", helper);
    assert_eq!(got.len(), 1, "テストヘルパの復活を見逃した: {got:?}");
    assert!(got[0].contains(":4:"), "行番号が合っていない: {got:?}");

    // 番犬自身の説明文は対象外
    assert!(offending_identifier(SELF_REL, injected).is_empty());
}

// ------------------------------------------------- 2. 旗を組む形の復活

/// 「`cell_cols` の列差を畳んで行 1 本の真偽にする」形を file:line で並べる。
/// `windows(2)` × `> 1` の組でだけ当たる（`== 2` を見る個別の assert には当たらない）
fn offending_flag_builder(rel: &str, src: &str) -> Vec<String> {
    production_source(src)
        .lines()
        .enumerate()
        .filter(|(_, line)| !is_comment(line))
        .filter(|(_, line)| line.contains("windows(2)"))
        .filter(|(_, line)| line.contains("> 1"))
        .filter(|(_, line)| line.contains("any(") || line.contains("all("))
        .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        .collect()
}

#[test]
fn 行の全角旗をcell_colsから組む形が復活していない() {
    let mut found = Vec::new();
    for path in all_sources() {
        let rel = rel_of(&path);
        if rel == SELF_REL {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("ソースを読む");
        found.extend(offending_flag_builder(&rel, &src));
    }
    assert!(
        found.is_empty(),
        "列差を畳んで「行に全角が在るか」を作る形が復活している（**右端の全角を構造的に\n\
         見落とす**物差し。#1388。必要なら `cell_cols` と行幅 `cols` から 1 実装で求める）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1388_注入_行の全角旗を組む形を名指しで落とせる() {
    // 修正前そのもの（名前を変えても落ちる）
    let injected = "fn build() {\n    \
        let wide_line = cell_cols.windows(2).any(|w| w[1] - w[0] > 1);\n}\n";
    let got = offending_flag_builder("crates/tako-core/src/screen.rs", injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":2:"), "行番号が合っていない: {got:?}");

    // 「2 列飛ぶ」を見る個別の検査には当たらない（テストの assert がこの形）
    let ok = "fn f() {\n    \
        assert!(cell_cols.windows(2).any(|w| w[1] - w[0] == 2));\n}\n";
    assert!(offending_flag_builder("crates/x/src/a.rs", ok).is_empty());
    // 単調性の検査（#1387）にも当たらない
    let monotonic = "fn f() {\n    \
        assert!(line.cell_cols.windows(2).all(|w| w[0] <= w[1]));\n}\n";
    assert!(offending_flag_builder("crates/x/src/a.rs", monotonic).is_empty());

    // コメントとテストモジュールは対象外
    assert!(offending_flag_builder(
        "crates/x/src/a.rs",
        "// cell_cols.windows(2).any(|w| w[1] - w[0] > 1)\n"
    )
    .is_empty());
    let in_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n    \
        let w = c.windows(2).any(|w| w[1] - w[0] > 1);\n}\n";
    assert!(offending_flag_builder("crates/x/src/a.rs", in_tests).is_empty());
}

// ------------------------------------------------- 3. フィールドの指紋

/// `ScreenLine` が持ってよいフィールド（**この順**）。
/// 増減させるときは「production の読み手が在るか」を確かめてからここを直す
const SCREEN_LINE_FIELDS: &[&str] = &["text", "runs", "cell_cols"];

const SCREEN_LINE_REL: &str = "crates/tako-core/src/screen.rs";

/// `pub struct ScreenLine { .. }` の `pub <名前>:` を宣言順に返す（定義の開始行も返す）
fn screen_line_fields(src: &str) -> (usize, Vec<String>) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("pub struct ScreenLine {"))
        .expect("`pub struct ScreenLine {` が見つからない（改名したら番犬も直す）");
    let mut fields = Vec::new();
    for line in &lines[start + 1..] {
        if line.starts_with('}') {
            break;
        }
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("pub ") {
            if let Some((name, _)) = rest.split_once(':') {
                fields.push(name.trim().to_string());
            }
        }
    }
    (start + 1, fields)
}

#[test]
fn screenlineのフィールドは3つ() {
    let src = std::fs::read_to_string(repo_root().join(SCREEN_LINE_REL)).expect("screen.rs");
    let (line, fields) = screen_line_fields(&src);
    assert_eq!(
        fields, SCREEN_LINE_FIELDS,
        "{SCREEN_LINE_REL}:{line}: `ScreenLine` のフィールドが変わっている。\n\
         増やすなら **production の読み手を書いてから**（#1388: 読み手のいない旗を\n\
         毎行計算し、doc だけが「描画で使う」と言っている状態を作らない）。\n\
         意図した変更なら番犬の `SCREEN_LINE_FIELDS` も直す"
    );
}

#[test]
fn i1388_注入_フィールドの増減を名指しで落とせる() {
    let injected = "\npub struct ScreenLine {\n    \
        pub text: String,\n    \
        pub runs: Vec<StyleRun>,\n    \
        pub cell_cols: Vec<usize>,\n    \
        pub has_wide: bool,\n}\n";
    let (line, fields) = screen_line_fields(injected);
    // 先頭の改行で 1 行目が空 = 定義は 2 行目（返す値は 1-based）
    assert_eq!(line, 2, "定義の開始行を取り違えている");
    assert_ne!(fields, SCREEN_LINE_FIELDS, "増えたフィールドを見逃した");
    assert_eq!(fields, ["text", "runs", "cell_cols", "has_wide"]);

    // doc コメント（`/// ...`）を挟んでも宣言順に拾える
    let with_docs = "pub struct ScreenLine {\n    \
        /// 行のテキスト\n    \
        pub text: String,\n    \
        pub runs: Vec<StyleRun>,\n    \
        pub cell_cols: Vec<usize>,\n}\n";
    assert_eq!(screen_line_fields(with_docs).1, SCREEN_LINE_FIELDS);
}

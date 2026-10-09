//! **#1917 の番犬**: TypeScript の構文の塗りが、正規表現の前置き（当たらない行で VM を
//! 起こさない等価形への書き換え）を通らない形へ戻らないようにする。
//!
//! ## 何が起きていたのか（release 実測・1 MB の TS）
//!
//! two-face の TS の構文定義は先読み・後読みだらけで、fancy-regex はそういう正規表現を
//! 「行の全バイト位置で VM を回す」形で走らせる。syntect は 1 行ごとに文脈の正規表現を
//! 1 回ずつ探すので、release でも 1 MB の塗りが 5.9 秒（同じ大きさの Rust の 4.6 倍・
//! そのほとんどが正規表現の探索）かかっていた。`tako-app/src/syntax_prefilter.rs` が TS / TSX の正規表現を
//! `\G(?=(?s:.)*?(?:必要条件))(?s:.)*?\K(?:元の正規表現)` へ書き換え、構文セットの取得は
//! そこを通す。
//!
//! ## ここで止める 4 つ
//!
//! 1. [`構文セットは書き換えの1実装から取る`] — `SyntectHighlighter::new` が two-face の
//!    構文セットを直に読む形へ戻る（書き換えが丸ごと効かなくなる）
//! 2. [`書き換えの対象はtsとtsx`] — 対象の構文名から TS / TSX が抜ける
//! 3. [`fancy_regexは1版だけ`] — 書き換えを読むパーサ（tako-app の直依存）と、実際に照合する
//!    エンジン（syntect の依存）が別の版になる（前置きの等価性の前提が崩れる）
//! 4. [`書き換えは往復の検査と構造の検証を通す`] — 直列化の往復検査（syntect の形が変わったら
//!    書き換えずに元へ戻る）か、1 本ずつの構造の検証（前置き + 元の正規表現そのもの）が消える
//!
//! 書き換えの効き目そのもの（重い正規表現が当たらない行で本体を試さない = バックトラック回数・
//! 塗りの結果が前後で一致・TS 以外の構文は 1 バイトも変えない）は `tako-app` 側の単体テスト
//! `syntax_prefilter::tests` が見る。`TAKO_1917_LEGACY=1` でそちらが名指しで落ちる。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const PREFILTER: &str = "crates/tako-app/src/syntax_prefilter.rs";
const LOCK: &str = "Cargo.lock";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// コメントを落とした眺め（説明文に書いた同じ綴りで緑にならないように。#1609）
fn read_code(rel: &str) -> String {
    code_view::without_comments_checked(&read(rel), rel)
}

/// 見つけた位置の行番号（`file:line` で名指しする）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `fn <name>(` から次の `fn` の手前までを切り出す（開始の行番号つき）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (usize, &'a str) {
    let head = format!("fn {name}(");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel}: `fn {name}` が見つからない（#1917）"));
    let rest = &src[at + head.len()..];
    let end = rest
        .find("\nfn ")
        .or_else(|| rest.find("\n    fn "))
        .unwrap_or(rest.len());
    (line_at(src, at), &rest[..end])
}

#[test]
fn 構文セットは書き換えの1実装から取る() {
    let src = read_code(PREVIEW);
    if let Some(at) = src.find("two_face::syntax::") {
        panic!(
            "{PREVIEW}:{}: two-face の構文セットを直に読んでいる。\n\
             構文セットは `crate::syntax_prefilter::syntax_set()` から取ること\n\
             （直に読むと TS の正規表現の前置きが効かず、1 MB の TS の塗りが 1.8 → 5.9 秒に戻る。#1917）",
            line_at(&src, at)
        );
    }
    let at = src
        .find("impl SyntectHighlighter {")
        .unwrap_or_else(|| panic!("{PREVIEW}: `impl SyntectHighlighter` が見つからない（#1917）"));
    let (line, body) = fn_body(&src[at..], PREVIEW, "new");
    let line = line_at(&src, at) + line - 1;
    assert!(
        body.contains("crate::syntax_prefilter::syntax_set()"),
        "{PREVIEW}:{line}: `SyntectHighlighter::new` が `syntax_prefilter::syntax_set()` を通らない（#1917）"
    );
}

#[test]
fn 書き換えの対象はtsとtsx() {
    let src = read_code(PREFILTER);
    let at = src
        .find("const TARGET_SYNTAXES")
        .unwrap_or_else(|| panic!("{PREFILTER}: `TARGET_SYNTAXES` が見つからない（#1917）"));
    let decl = &src[at..at + src[at..].find("];").expect("宣言の終わり")];
    for name in ["\"TypeScript\"", "\"TypeScriptReact\""] {
        assert!(
            decl.contains(name),
            "{PREFILTER}:{}: 書き換えの対象から {name} が抜けた（#1917）\n{decl}",
            line_at(&src, at)
        );
    }
}

#[test]
fn fancy_regexは1版だけ() {
    let lock = read(LOCK);
    let entries: Vec<usize> = lock
        .match_indices("\nname = \"fancy-regex\"\n")
        .map(|(at, _)| line_at(&lock, at + 1))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "{LOCK}: fancy-regex が {} 版ある（{}行目）。\n\
         TS の正規表現の書き換え（tako-app の fancy-regex で読む）と、実際に照合する syntect の\n\
         fancy-regex が別の版だと、前置きの等価性の前提が崩れる。版を揃えること（#1917）",
        entries.len(),
        entries
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

#[test]
fn 書き換えは往復の検査と構造の検証を通す() {
    let src = read_code(PREFILTER);
    let (line, build) = fn_body(&src, PREFILTER, "build");
    assert!(
        build.contains("bincode::serialize(&set)") && build.contains("&original[..]"),
        "{PREFILTER}:{line}: `build` が直列化の往復検査をしていない。\n\
         syntect の直列化の形が変わったときに、壊れた構文セットを使わず元へ戻る前提が崩れる（#1917）"
    );
    let (line, rewrite) = fn_body(&src, PREFILTER, "rewrite");
    assert!(
        rewrite.contains("verified("),
        "{PREFILTER}:{line}: `rewrite` が書き換えた正規表現の構造を検証していない（#1917）"
    );
    let (line, verified) = fn_body(&src, PREFILTER, "verified");
    for needle in [
        "ContinueFromPreviousMatchEnd",
        "KeepOut",
        "has_group(guard)",
        "body == original",
    ] {
        assert!(
            verified.contains(needle),
            "{PREFILTER}:{line}: `verified` が `{needle}` を確かめていない（#1917）"
        );
    }
}

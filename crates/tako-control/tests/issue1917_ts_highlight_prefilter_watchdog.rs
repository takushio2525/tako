//! **#1917 / #1935 の番犬**: 構文の塗りが、正規表現の前置き（当たらない行で VM を
//! 起こさない等価形への書き換え）を通らない形へ戻らないようにする。
//!
//! ## 何が起きていたのか（release 実測・1 MB の入力）
//!
//! two-face の TS の構文定義は先読み・後読みだらけで、fancy-regex はそういう正規表現を
//! 「行の全バイト位置で VM を回す」形で走らせる。syntect は 1 行ごとに文脈の正規表現を
//! 1 回ずつ探すので、release でも 1 MB の TS の塗りが 5.9 秒（同じ大きさの Rust の 4.6 倍・
//! そのほとんどが正規表現の探索）かかっていた。ほかの言語も同じ形（JavaScript 3.8 秒・
//! C# 3.2 秒。#1935）。正規表現を `\G(?=(?s:.)*?(?:必要条件))(?s:.)*?\K(?:元の正規表現)` へ
//! 書き換えると、1 MB の TS は 1.8 秒・JavaScript は 1.9 秒・C# は 1.0 秒になる。
//!
//! 書き換えは**ビルド時**に `build.rs` が `tako-app/src/syntax_rewrite.rs` で行い、起動時は
//! `tako-app/src/syntax_prefilter.rs` が差し込むだけ。前置きはコンパイルの固定費を持つので、
//! 小さい入力は元の正規表現の構文セットで、大きい入力だけ前置き付きで塗る（#1935）。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`構文セットは書き換えの1実装から取る`] — `SyntectHighlighter` が two-face の構文セットを
//!    直に読む形へ戻る / 大きい入力が前置き付きの構文セットを通らなくなる
//! 2. [`書き換えの対象は計測で選んだ21構文`] — 対象の構文名から計測で選んだ言語が抜ける
//! 3. [`fancy_regexは1版だけ`] — 書き換えを読むパーサ（tako-app のビルド依存）と、実際に照合する
//!    エンジン（syntect の依存）が別の版になる（前置きの等価性の前提が崩れる）
//! 4. [`書き換えは往復の検査と構造の検証を通す`] — 直列化の往復検査（syntect の形が変わったら
//!    書き換えずに元へ戻る）・差し込む前の指紋の照合・1 本ずつの構造の検証
//!    （前置き + 元の正規表現そのもの）のどれかが消える
//! 5. [`書き換えはビルド時で起動時は差し込むだけ`] — `build.rs` が生成をやめる / アプリが
//!    実行時に正規表現を書き換える形へ戻る（全 214 構文で release 約 340 ms を起動のたびに払う）
//! 6. [`塗りは入力の行数で構文セットを選ぶ`] — 全文の塗り（`run`）かエディタの塗り（`refresh`）が
//!    行数で構文セットを選ばなくなる（UI スレッドで塗る小さい入力の冷えた 1 回目が重くなる）
//!
//! 書き換えの効き目そのもの（重い正規表現が当たらない行で本体を試さない = バックトラック回数・
//! 塗りの結果が前後で一致・対象外の構文は 1 バイトも変えない・生成物が今のコードで作り直したものと
//! 一致）は `tako-app` 側の単体テスト `syntax_prefilter::tests` が見る。`TAKO_1917_LEGACY=1` /
//! `TAKO_1935_LEGACY=1` でそちらが名指しで落ちる。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const PREFILTER: &str = "crates/tako-app/src/syntax_prefilter.rs";
const REWRITE: &str = "crates/tako-app/src/syntax_rewrite.rs";
const MIRROR: &str = "crates/tako-app/src/syntax_mirror.rs";
const BUILD: &str = "crates/tako-app/build.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
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
        .unwrap_or_else(|| panic!("{rel}: `fn {name}` が見つからない（#1917 / #1935）"));
    let rest = &src[at + head.len()..];
    let end = [
        "\nfn ",
        "\n    fn ",
        "\npub(crate) fn ",
        "\n    pub(crate) fn ",
    ]
    .iter()
    .filter_map(|next| rest.find(next))
    .min()
    .unwrap_or(rest.len());
    (line_at(src, at), &rest[..end])
}

/// `impl SyntectHighlighter {` の中の `fn <name>(` の本体（行番号はファイル全体のもの）
fn highlighter_fn<'a>(src: &'a str, name: &str) -> (usize, &'a str) {
    let at = src.find("impl SyntectHighlighter {").unwrap_or_else(|| {
        panic!("{PREVIEW}: `impl SyntectHighlighter` が見つからない（#1917 / #1935）")
    });
    let (line, body) = fn_body(&src[at..], PREVIEW, name);
    (line_at(src, at) + line - 1, body)
}

#[test]
fn 構文セットは書き換えの1実装から取る() {
    let src = read_code(PREVIEW);
    if let Some(at) = src.find("two_face::syntax::") {
        panic!(
            "{PREVIEW}:{}: two-face の構文セットを直に読んでいる。\n\
             構文セットは `crate::syntax_prefilter` の `plain_set()` / `syntax_set()` から取ること\n\
             （直に読むと正規表現の前置きが効かず、1 MB の TS の塗りが 1.8 → 5.9 秒に戻る。#1917 / #1935）",
            line_at(&src, at)
        );
    }
    let (line, new) = highlighter_fn(&src, "new");
    assert!(
        new.contains("crate::syntax_prefilter::plain_set()"),
        "{PREVIEW}:{line}: `SyntectHighlighter::new` が `syntax_prefilter::plain_set()` を通らない（#1935）"
    );
    let (line, set_for) = highlighter_fn(&src, "set_for");
    for needle in [
        "crate::syntax_prefilter::variant_for(syntax, lines)",
        "crate::syntax_prefilter::syntax_set",
        "crate::syntax_prefilter::ts_only_set",
    ] {
        assert!(
            set_for.contains(needle),
            "{PREVIEW}:{line}: `set_for` が `{needle}` を通らない。\n\
             大きい入力は前置き付きの構文セットで塗ること（#1917 / #1935）"
        );
    }
}

/// 段 1 の計測（1 MB の入力で 15% 以上かつ 50 ms 以上短くなる）で選んだ構文
const MEASURED_TARGETS: [&str; 21] = [
    "TypeScript",
    "TypeScriptReact",
    "JavaScript",
    "C#",
    "C++",
    "Ruby",
    "Perl",
    "Python",
    "Go",
    "C",
    "Swift",
    "Rust",
    "Java",
    "Lua",
    "SQL",
    "CSS",
    "Bourne Again Shell (bash)",
    "YAML",
    "Markdown",
    "HTML",
    "Kotlin",
];

#[test]
fn 書き換えの対象は計測で選んだ21構文() {
    let src = read_code(MIRROR);
    let at = src
        .find("const TARGETS")
        .unwrap_or_else(|| panic!("{MIRROR}: `TARGETS` が見つからない（#1917 / #1935）"));
    let decl = &src[at..at + src[at..].find("];").expect("宣言の終わり")];
    for name in MEASURED_TARGETS {
        assert!(
            decl.contains(&format!("target(\"{name}\",")),
            "{MIRROR}:{}: 書き換えの対象から \"{name}\" が抜けた（#1917 / #1935）\n\
             外すなら計測の表（.agent/architecture.md）と理由を添えて、この番犬の表も直すこと",
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
         正規表現の書き換え（tako-app の build.rs が fancy-regex で読む）と、実際に照合する syntect の\n\
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
    // 起動時の差し込み: 往復検査と、生成したときの土台の文脈との指紋の照合
    let src = read_code(PREFILTER);
    let (line, build) = fn_body(&src, PREFILTER, "build");
    let (splice_line, splice) = fn_body(&src, PREFILTER, "splice");
    for (name, line, body, needle) in [
        ("build", line, build, "bincode::serialize(&set)"),
        ("build", line, build, "&original[..]"),
        (
            "splice",
            splice_line,
            splice,
            "Fingerprint::of(&syntax.serialized_lazy_contexts)",
        ),
    ] {
        assert!(
            body.contains(needle),
            "{PREFILTER}:{line}: `{name}` が `{needle}` を確かめていない。\n\
             syntect の直列化の形や two-face の版が変わったときに、壊れた・食い違った文脈を\n\
             差し込まず元へ戻る前提が崩れる（#1917 / #1935）"
        );
    }
    // ビルド時の生成: 往復検査と 1 本ずつの構造の検証
    let src = read_code(REWRITE);
    let (line, generate) = fn_body(&src, REWRITE, "generate");
    assert!(
        generate.contains("bincode::serialize(&set)") && generate.contains("&original[..]"),
        "{REWRITE}:{line}: `generate` が直列化の往復検査をしていない（#1917 / #1935）"
    );
    let (line, rewrite) = fn_body(&src, REWRITE, "rewrite");
    assert!(
        rewrite.contains("verified("),
        "{REWRITE}:{line}: `rewrite` が書き換えた正規表現の構造を検証していない（#1917）"
    );
    let (line, verified) = fn_body(&src, REWRITE, "verified");
    for needle in [
        "ContinueFromPreviousMatchEnd",
        "KeepOut",
        "has_group(guard)",
        "body == original",
    ] {
        assert!(
            verified.contains(needle),
            "{REWRITE}:{line}: `verified` が `{needle}` を確かめていない（#1917）"
        );
    }
}

#[test]
fn 書き換えはビルド時で起動時は差し込むだけ() {
    let build = read_code(BUILD);
    for needle in [
        "#[path = \"src/syntax_rewrite.rs\"]",
        "#[path = \"src/syntax_mirror.rs\"]",
        "generate_prefiltered_syntaxes();",
        "syntax_rewrite::generate(",
        "prefiltered-syntaxes.bin",
        "cargo:rerun-if-changed=src/syntax_rewrite.rs",
    ] {
        assert!(
            build.contains(needle),
            "{BUILD}: `{needle}` が無い。前置き付きの文脈はビルド時に作ること（#1935）"
        );
    }
    let prefilter = read_code(PREFILTER);
    assert!(
        prefilter
            .contains("include_bytes!(concat!(env!(\"OUT_DIR\"), \"/prefiltered-syntaxes.bin\"))"),
        "{PREFILTER}: ビルド時に作った文脈を読んでいない（#1935）"
    );
    // アプリ本体（テスト以外）は書き換えない = 書き換えのモジュールはテストのときだけ入る
    let main = read_code(MAIN);
    let at = main
        .find("mod syntax_rewrite;")
        .unwrap_or_else(|| panic!("{MAIN}: `mod syntax_rewrite;` が見つからない（#1935）"));
    assert!(
        main[..at].trim_end().ends_with("#[cfg(test)]"),
        "{MAIN}:{}: `mod syntax_rewrite` がテスト以外でも入っている。\n\
         実行時に正規表現を書き換えると、全 214 構文で release 約 340 ms を起動のたびに払う（#1935）",
        line_at(&main, at)
    );
    // 差し込む側・塗る側のテスト以外のコードは書き換えを呼ばない
    for rel in [PREFILTER, PREVIEW] {
        let src = read_code(rel);
        let body = src.split("#[cfg(test)]").next().unwrap_or(&src);
        if let Some(at) = body.find("syntax_rewrite::") {
            panic!(
                "{rel}:{}: テスト以外から書き換えを呼んでいる（#1935）",
                line_at(&src, at)
            );
        }
    }
}

#[test]
fn 塗りは入力の行数で構文セットを選ぶ() {
    let src = read_code(PREVIEW);
    for name in ["run", "refresh"] {
        let (line, body) = highlighter_fn(&src, name);
        assert!(
            body.contains("self.set_for(") && body.contains("Self::same_syntax_in(set,"),
            "{PREVIEW}:{line}: `{name}` が入力の行数で構文セットを選んでいない（`set_for`）。\n\
             小さい入力まで前置き付きで塗ると、UI スレッドで塗るチャットのコードブロックや\n\
             エディタの冷えた 1 回目が重くなる（#1935）"
        );
    }
    let (line, refresh) = highlighter_fn(&src, "refresh");
    assert!(
        refresh.contains("cache.variant == variant"),
        "{PREVIEW}:{line}: `refresh` が構文セットの種類の入れ替わりで全文へ戻らない（#1935）"
    );
}

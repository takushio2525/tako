//! 番犬: Code Runner の変数展開（`${file}` 等）が、実行ペインのシェルの方言を見ずに引用する形へ
//! 戻らない（Issue #1668）。
//!
//! ## 何を止めたいのか
//!
//! `runner::expand_with` は `${file}` 等の値を語の**途中**にも埋める（`-o ${fileNoExt}.exe` /
//! `.\${fileNoExt}.exe`）。#1668 以前は方言を見ずに POSIX の `quote_for_shell` で囲んでいたので、
//! Windows の実行ペイン（PowerShell）では `-o 'my file'.exe` が文字列のメンバー参照（= 空の引数）
//! になり、`'it'\''s.c'` は構文エラーになった。正しい形は `RunVars::dialect` の
//! `ShellDialect::quote_fragment`（POSIX = 単引用符 / PowerShell = バッククオート）の 1 本で、
//! 壊れ方は「`quote_for_shell` を直に呼ぶ 1 行が戻る」か「単引用符の `quote_arg` を当てる」形:
//!
//! 1. `runner.rs` の本番コードに、方言を見ない引用（`quote_for_shell` / `quote_paths_for_shell`）も、
//!    語の先頭で壊れる単引用符の形（`quote_arg`）も置かない。**落ちるときは file:line で名指し**
//! 2. `expand_with` が `dialect.quote_fragment(` を通っている
//! 3. 実挙動: Windows の展開が PowerShell の形（引用符で始まらず、POSIX の形と違う）になる
//!    （方言側の PowerShell の腕が POSIX へ委ねる形に戻ったら、`quote_fragment` を名指しで落とす）
//!
//! ## 眺めの選び方
//!
//! - 呼び出しの不在（規則 1）は `code_view`（コメントも文字列も潰す）を `production`
//!   （テスト領域を潰す）に重ねる。止めたいのはコンパイルされる呼び出しで、テストの期待値の
//!   組み立て（`quote_for_shell` で POSIX の形を作る）は正しく存在する
//! - 配線の存在（規則 2）は `without_comments`（#1609 の寄せ先）
//!
//! ## 対象外
//!
//! プロジェクト既定の組み立て（`runner_project.rs`）と `tako:shell` の外側の包み
//! （`platform::shell::declared_shell_command`）は変数展開を通らずに引用する別の経路で、
//! #2286 で扱う
//!
//! ## A/B（検出力の確かめ方）
//!
//! `注入した違反をfile_lineで名指しする` が、`expand_with` の引用を `quote_for_shell` へ戻した
//! 写しで規則 1 と 2 が落ち、`runner.rs:<行>` を名指すことを確かめる

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};
use production_range::production;
use std::path::{Path, PathBuf};
use tako_core::platform::shell_dialect::ShellDialect;
use tako_core::platform::support::Platform;
use tako_core::runner::{expand_variables, RunVars};

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

const RUNNER: &str = "crates/tako-core/src/runner.rs";
const DIALECT: &str = "crates/tako-core/src/platform/shell_dialect.rs";

/// 変数展開の経路に置かない引用（方言を見ない POSIX の形と、語の先頭で壊れる単引用符の形）
const BANNED: &[(&str, &str)] = &[
    ("quote_for_shell(", "方言を見ない POSIX の引用"),
    ("quote_paths_for_shell(", "方言を見ない POSIX の引用"),
    (
        "quote_arg(",
        "単引用符の形（PowerShell では語の先頭で `'my file'.exe` がメンバー参照になる）",
    ),
];

/// `byte` が何行目か（1 始まり）
fn 行番号(src: &str, byte: usize) -> usize {
    src[..byte].matches('\n').count() + 1
}

/// 規則 1: 本番コードに方言を見ない引用の呼び出しが無い
fn 方言を見ない引用を探す(src: &str, rel: &str) -> Vec<String> {
    // 走査が空振りしていないことを先に確かめる（眺めが空だと「無い」が必ず通る）
    without_comments_checked(src, rel);
    let view = code_view(&production(src, rel));
    let mut hits = Vec::new();
    for (banned, why) in BANNED {
        for (pos, _) in view.match_indices(banned) {
            // 語の頭でだけ数える（`ps_quote_arg(` のような別の名前を拾わない）
            let prev = view[..pos].chars().next_back();
            if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            hits.push(format!(
                "{rel}:{} に `{}` がある（{why}）。Code Runner の変数展開は実行ペインの方言の \
                 `ShellDialect::quote_fragment`（`RunVars::dialect`）で引用する（Issue #1668）",
                行番号(src, pos),
                banned.trim_end_matches('(')
            ));
        }
    }
    hits
}

/// 規則 2: `expand_with` が方言の `quote_fragment` を通る
fn 展開の引用が方言を通るか(src: &str, rel: &str) -> Result<(), String> {
    let view = without_comments_checked(src, rel);
    let start = view
        .find("fn expand_with(")
        .ok_or_else(|| format!("{rel}: `fn expand_with(` が無い（改名したらこの番犬も追う）"))?;
    let end = view[start..]
        .find("\n}\n")
        .map(|e| start + e)
        .ok_or_else(|| {
            format!(
                "{rel}:{}: `expand_with` の終わりが見つからない",
                行番号(src, start)
            )
        })?;
    if view[start..end].contains("dialect.quote_fragment(") {
        Ok(())
    } else {
        Err(format!(
            "{rel}:{}: `expand_with` が `dialect.quote_fragment(` を通っていない。値は実行ペインの \
             方言で引用する（POSIX = 単引用符 / PowerShell = バッククオート。Issue #1668）",
            行番号(src, start)
        ))
    }
}

#[test]
fn 変数展開の経路に方言を見ない引用が無い() {
    let src = read(RUNNER);
    let hits = 方言を見ない引用を探す(&src, RUNNER);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 変数展開は方言のquote_fragmentを通る() {
    展開の引用が方言を通るか(&read(RUNNER), RUNNER).unwrap_or_else(|e| panic!("{e}"));
}

/// 規則 3（実挙動）: Windows の展開は PowerShell の形、macOS は POSIX の形
#[test]
fn windowsの展開はpowershellの形でmacosはposixの形() {
    let src = read(DIALECT);
    let at = src
        .find("pub fn quote_fragment(")
        .unwrap_or_else(|| panic!("{DIALECT}: `pub fn quote_fragment(` が無い"));
    let here = format!("{DIALECT}:{}", 行番号(&src, at));
    for name in ["my file", "it's", "say \"hi\"", "$HOME", "a&b"] {
        let path = PathBuf::from(format!("/tmp/x/{name}.c"));
        let tpl = "-o ${fileNoExt}.exe";
        let win = expand_variables(tpl, &RunVars::for_file(&path).on(Platform::Windows));
        let mac = expand_variables(tpl, &RunVars::for_file(&path).on(Platform::MacOs));
        assert_eq!(
            mac,
            format!("-o {}.exe", tako_core::quote_for_shell(name)),
            "macOS の展開が POSIX の形でない（#1668 以前と同じであるべき）"
        );
        assert_eq!(
            win,
            format!("-o {}.exe", ShellDialect::PowerShell.quote_fragment(name)),
            "Windows の展開が実行ペインの方言（PowerShell）を通っていない"
        );
        assert!(
            win != mac && !win.contains(" '"),
            "{here}: PowerShell の `quote_fragment` が POSIX / 単引用符の形を返している \
             （`{win}`。語の先頭の単引用符は `.exe` がメンバー参照になる = Issue #1668）"
        );
    }
}

// --- 番犬そのものが効いていることの自己検査（写しへの注入 A/B）---

#[test]
fn 注入した違反をfile_lineで名指しする() {
    let src = read(RUNNER);
    let anchor = "vars.dialect.quote_fragment(s)";
    let at = src
        .find(anchor)
        .unwrap_or_else(|| panic!("注入の目印 {anchor:?} が無い"));
    let want_line = 行番号(&src, at);
    for replacement in ["quote_for_shell(s)", "vars.dialect.quote_arg(s)"] {
        let injected = src.replacen(anchor, replacement, 1);
        let hits = 方言を見ない引用を探す(&injected, RUNNER);
        assert!(
            hits.iter()
                .any(|h| h.starts_with(&format!("{RUNNER}:{want_line} "))),
            "`{replacement}` を戻した写しで {RUNNER}:{want_line} を名指していない: {hits:?}"
        );
        assert!(
            展開の引用が方言を通るか(&injected, RUNNER).is_err(),
            "`{replacement}` を戻した写しで規則 2 が落ちない"
        );
    }
    // テストの期待値の組み立て（`use crate::shell::quote_for_shell` のテスト）は数えない
    assert!(
        src.contains("quote_for_shell"),
        "前提: runner.rs のテストは POSIX の期待値を quote_for_shell で組む"
    );
}

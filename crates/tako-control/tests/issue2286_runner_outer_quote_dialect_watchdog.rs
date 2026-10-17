//! 番犬: Code Runner の変数展開**以外**の引用 3 経路が、実行ペインのシェルの方言を見ない形へ
//! 戻らない（Issue #2286。変数展開の経路は #1668 の `issue1668_runner_quote_dialect_watchdog`）。
//!
//! ## 何を止めたいのか
//!
//! 3 経路とも、組み立てた文字列を読むのは**実行ペインのシェル**（macOS = `/bin/sh`、Windows =
//! PowerShell。`platform::shell::run_pane_dialect_for`）なのに、#2286 以前は別の方言で引用していた:
//!
//! 1. `tako:shell` の外側の包み（`platform::shell::declared_shell_command`）が**宣言したシェル**の
//!    方言で本体を囲んでいた。macOS の `pwsh -Command 'echo it''s'` は `/bin/sh` が `echo its` にし、
//!    Windows の `bash -c '…'\''…'` は PowerShell の構文エラー。正しい形は
//!    `run_pane_dialect_for(platform)` の `enclose_arg`（フラグだけが宣言したシェルで決まる）
//! 2. プロジェクト既定の組み立て（`runner_project.rs`）が POSIX の `quote_for_shell` を直に呼んでいた。
//!    Windows では `'` を含む名前が構文エラー、`,` は配列、先頭の `@` は splatting。正しい形は
//!    `arg`（= `run_pane_dialect_for(platform).quote_fragment`）の 1 本
//! 3. 宣言の `tako:cwd`（`runner.rs`）を macOS だけ「引用して展開 → 全体が単引用符なら外す」で
//!    作っていたので、`${fileDir}/build` が `/tmp/my dir/'/tmp/my dir'/build` になった。正しい形は
//!    両 OS で `declared_cwd_value`（引用しない展開 `expand_literal`）の 1 本
//!
//! 規則（**落ちるときは file:line で名指し**）:
//!
//! - 規則 1: `runner_project.rs` の本番コードに方言を見ない引用（`quote_for_shell` 等）・方言を
//!   決め打ちした引用（`ShellDialect::Posix.` / `ShellDialect::PowerShell.`）を置かず、`fn arg` が
//!   `run_pane_dialect_for(` と `.quote_fragment(` を通る
//! - 規則 2: `declared_shell_command` が `run_pane_dialect_for(platform)` と `.enclose_arg(` を通り、
//!   方言を決め打ちした引用・手書きの `'\''`・`quote_for_shell` を置かない
//! - 規則 3: `resolve_core` の宣言の cwd は `declared_cwd_value(` を通り（`strip_quotes(` を直に
//!   呼ばない）、`declared_cwd_value` は `expand_literal(` で展開する（引用する展開を使わない）
//! - 規則 4（実挙動）: 3 経路それぞれの両 OS の形
//!
//! ## 眺めの選び方
//!
//! 呼び出しの不在（規則 1）は `code_view` を `production` に重ねる（テストの期待値の組み立てに
//! `quote_for_shell` が正しく存在する）。関数の本体を見る規則 2・3 は `without_comments`
//! （`'\''` のような文字リテラルを残す。#1609 の寄せ先）。
//!
//! ## A/B（検出力の確かめ方）
//!
//! `注入した違反をfile_lineで名指しする` が、3 経路それぞれへ方言を見ない引用を戻した写しで
//! 対応する規則が落ち、`<file>:<行>` を名指すことを確かめる

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};
use production_range::production;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tako_core::platform::shell::declared_shell_command;
use tako_core::platform::support::Platform;
use tako_core::runner::{resolve_file_in, ConfigLayer, RuntimeWords};

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

const SHELL: &str = "crates/tako-core/src/platform/shell.rs";
const PROJECT: &str = "crates/tako-core/src/runner_project.rs";
const RUNNER: &str = "crates/tako-core/src/runner.rs";

/// `byte` が何行目か（1 始まり）
fn 行番号(src: &str, byte: usize) -> usize {
    src[..byte].matches('\n').count() + 1
}

/// `needle` が語の頭で現れる位置（`ps_quote_arg(` のような別の名前を拾わない）
fn 語の頭で探す<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).filter_map(move |(pos, _)| {
        let prev = hay[..pos].chars().next_back();
        (!prev.is_some_and(|c| c.is_alphanumeric() || c == '_')).then_some(pos)
    })
}

/// トップレベルの関数 `sig`（`fn arg(` 等）の本体の範囲（バイト）。眺めの上で探す
fn 関数の範囲(view: &str, src: &str, rel: &str, sig: &str) -> Result<(usize, usize), String> {
    let start = 語の頭で探す(view, sig)
        .next()
        .ok_or_else(|| format!("{rel}: `{sig}` が無い（改名したらこの番犬も追う。Issue #2286）"))?;
    let end = view[start..]
        .find("\n}\n")
        .map(|e| start + e)
        .ok_or_else(|| {
            format!(
                "{rel}:{}: `{sig}` の終わりが見つからない",
                行番号(src, start)
            )
        })?;
    Ok((start, end))
}

const WHY: &str =
    "実行ペインのシェルの方言（`run_pane_dialect_for(platform)`）で引用する（Issue #2286）";

/// 規則 1: プロジェクト既定の組み立て
fn 組み立ての引用を調べる(src: &str, rel: &str) -> Vec<String> {
    const BANNED: &[(&str, &str)] = &[
        ("quote_for_shell(", "方言を見ない POSIX の引用"),
        ("quote_paths_for_shell(", "方言を見ない POSIX の引用"),
        ("ShellDialect::Posix.", "方言を決め打ちした引用"),
        ("ShellDialect::PowerShell.", "方言を決め打ちした引用"),
    ];
    without_comments_checked(src, rel);
    let view = code_view(&production(src, rel));
    let mut hits = Vec::new();
    for (banned, why) in BANNED {
        for pos in 語の頭で探す(&view, banned) {
            hits.push(format!(
                "{rel}:{} に `{}` がある（{why}）。コマンドへ埋める名前は `arg(platform, …)` で{WHY}",
                行番号(src, pos),
                banned.trim_end_matches(['(', '.'])
            ));
        }
    }
    let body_view = without_comments_checked(src, rel);
    match 関数の範囲(&body_view, src, rel, "fn arg(") {
        Ok((s, e)) => {
            for need in ["run_pane_dialect_for(", ".quote_fragment("] {
                if !body_view[s..e].contains(need) {
                    hits.push(format!(
                        "{rel}:{}: `fn arg` が `{need}` を通っていない。{WHY}",
                        行番号(src, s)
                    ));
                }
            }
        }
        Err(e) => hits.push(e),
    }
    hits
}

/// 規則 2: `tako:shell` の外側の包み
fn 外側の包みを調べる(src: &str, rel: &str) -> Vec<String> {
    let view = without_comments_checked(src, rel);
    let (s, e) = match 関数の範囲(&view, src, rel, "fn declared_shell_command(") {
        Ok(r) => r,
        Err(e) => return vec![e],
    };
    let body = &view[s..e];
    let mut hits = Vec::new();
    for need in ["run_pane_dialect_for(platform)", ".enclose_arg("] {
        if !body.contains(need) {
            hits.push(format!(
                "{rel}:{}: `declared_shell_command` が `{need}` を通っていない。本体は{WHY}",
                行番号(src, s)
            ));
        }
    }
    for banned in [
        "ShellDialect::PowerShell.",
        "ShellDialect::Posix.",
        r"replace('\''",
        "quote_for_shell(",
    ] {
        for pos in 語の頭で探す(body, banned) {
            hits.push(format!(
                "{rel}:{}: `declared_shell_command` に `{banned}` がある（宣言したシェルの方言・手書きの\
                 引用で外側を囲む形）。外側を読むのは実行ペインのシェルなので{WHY}",
                行番号(src, s + pos)
            ));
        }
    }
    hits
}

/// 規則 3: 宣言の `tako:cwd`
fn 作業ディレクトリの展開を調べる(src: &str, rel: &str) -> Vec<String> {
    let view = without_comments_checked(src, rel);
    let mut hits = Vec::new();
    match 関数の範囲(&view, src, rel, "fn resolve_core(") {
        Ok((s, e)) => {
            let body = &view[s..e];
            if !body.contains("declared_cwd_value(") {
                hits.push(format!(
                    "{rel}:{}: `resolve_core` の宣言の cwd が `declared_cwd_value(` を通っていない\
                     （cwd はパスとして使うので引用せずに展開する 1 実装。Issue #2286）",
                    行番号(src, s)
                ));
            }
            for pos in 語の頭で探す(body, "strip_quotes(") {
                hits.push(format!(
                    "{rel}:{}: `resolve_core` が `strip_quotes(` を直に呼んでいる（「引用して展開してから\
                     外す」形は `${{fileDir}}/build` が `/tmp/my dir/'/tmp/my dir'/build` になる。\
                     `declared_cwd_value` を通す。Issue #2286）",
                    行番号(src, s + pos)
                ));
            }
        }
        Err(e) => hits.push(e),
    }
    match 関数の範囲(&view, src, rel, "fn declared_cwd_value(") {
        Ok((s, e)) => {
            let body = &view[s..e];
            if !body.contains("expand_literal(") {
                hits.push(format!(
                    "{rel}:{}: `declared_cwd_value` が `expand_literal(` を通っていない（Issue #2286）",
                    行番号(src, s)
                ));
            }
            for banned in ["expand_variables(", "expand("] {
                for pos in 語の頭で探す(body, banned) {
                    hits.push(format!(
                        "{rel}:{}: `declared_cwd_value` が引用する展開 `{banned}` を使っている\
                         （cwd はシェルへ渡さない。Issue #2286）",
                        行番号(src, s + pos)
                    ));
                }
            }
        }
        Err(e) => hits.push(e),
    }
    hits
}

#[test]
fn プロジェクト既定の組み立ては実行ペインの方言で引用する() {
    let hits = 組み立ての引用を調べる(&read(PROJECT), PROJECT);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn tako_shellの外側は実行ペインの方言で囲む() {
    let hits = 外側の包みを調べる(&read(SHELL), SHELL);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 宣言のcwdは引用しない展開の1実装を通る() {
    let hits = 作業ディレクトリの展開を調べる(&read(RUNNER), RUNNER);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

/// 規則 4（実挙動）: 3 経路の両 OS の形
#[test]
fn 三経路の両osの形() {
    // 経路 1: 外側は実行ペインの方言（フラグだけが宣言したシェル）
    assert_eq!(
        declared_shell_command(Platform::MacOs, "pwsh", "echo it's"),
        r"pwsh -Command 'echo it'\''s'",
        "{SHELL}: macOS の外側を読むのは /bin/sh"
    );
    assert_eq!(
        declared_shell_command(Platform::Windows, "bash", "echo it's"),
        "bash -c 'echo it''s'",
        "{SHELL}: Windows の外側を読むのは PowerShell"
    );

    // 経路 2: npm のプロジェクトの中の `it's.js`（script に載っていない = `node <rel>`）
    let scratch = tako_core::test_residue::ScratchDir::new("issue2286-watchdog");
    let root = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
    std::fs::write(root.join("package.json"), "{\"name\":\"x\"}").unwrap();
    let file = root.join("it's.js");
    std::fs::write(&file, "").unwrap();
    let bounds = tako_core::project_root::SearchBounds::with_ceilings([root.clone()]);
    let resolve = |platform: Platform, file: &Path, head: &str, bounds| {
        resolve_file_in(
            platform,
            file,
            head,
            &BTreeMap::new(),
            None,
            None,
            bounds,
            &RuntimeWords::new(),
            &ConfigLayer::default(),
        )
        .unwrap_or_else(|e| panic!("{}: 解決できない: {e}", file.display()))
        .plan
    };
    assert_eq!(
        resolve(Platform::MacOs, &file, "", Some(&bounds)).command,
        r"node 'it'\''s.js'",
        "{PROJECT}: macOS は POSIX の形のまま"
    );
    assert_eq!(
        resolve(Platform::Windows, &file, "", Some(&bounds)).command,
        "node it`'s.js",
        "{PROJECT}: Windows は PowerShell の形（`arg`）"
    );

    // 経路 3: 空白を含むファイルの段 + `${fileDir}/build`
    let py = PathBuf::from("/tmp/my dir/x.py");
    let head = "# tako:run: python ${fileBase}\n# tako:cwd: ${fileDir}/build\n";
    for platform in [Platform::MacOs, Platform::Windows] {
        assert_eq!(
            resolve(platform, &py, head, None).cwd,
            PathBuf::from("/tmp/my dir/build"),
            "{RUNNER}: {platform:?} の宣言の cwd が 1 つのパスになっていない"
        );
    }
}

// --- 番犬そのものが効いていることの自己検査（写しへの注入 A/B）---

/// 規則の検査（ソース, 相対パス）→ 名指し
type 検査 = fn(&str, &str) -> Vec<String>;

/// `anchor` を `replacement` へ差し替えた写しで、`check` が `rel:<anchor の行>` を名指すか
fn 名指すか(rel: &str, anchor: &str, replacement: &str, check: 検査) -> Result<(), String> {
    let src = read(rel);
    let at = src
        .find(anchor)
        .ok_or_else(|| format!("注入の目印 {anchor:?} が {rel} に無い"))?;
    let want = format!("{rel}:{} ", 行番号(&src, at));
    let want_colon = format!("{rel}:{}:", 行番号(&src, at));
    let hits = check(&src.replacen(anchor, replacement, 1), rel);
    if hits
        .iter()
        .any(|h| h.starts_with(&want) || h.starts_with(&want_colon))
    {
        Ok(())
    } else {
        Err(format!(
            "`{anchor}` → `{replacement}` の写しで {want} を名指していない: {hits:?}"
        ))
    }
}

#[test]
fn 注入した違反をfile_lineで名指しする() {
    let cases: &[(&str, &str, &str, 検査)] = &[
        // 経路 1: 宣言したシェルの方言で外側を囲む形（#2286 以前）
        (
            SHELL,
            "outer.enclose_arg(command)",
            "ShellDialect::PowerShell.quote_arg(command)",
            外側の包みを調べる,
        ),
        (
            SHELL,
            "outer.enclose_arg(command)",
            r#"format!("'{}'", command.replace('\'', "'\\''"))"#,
            外側の包みを調べる,
        ),
        // 経路 2: 方言を見ない POSIX の引用を直に戻す
        (
            PROJECT,
            "arg(p.platform, &rel)",
            "quote_for_shell(&rel)",
            組み立ての引用を調べる,
        ),
        (
            PROJECT,
            "arg(p.platform, &script)",
            "crate::platform::shell_dialect::ShellDialect::Posix.quote_fragment(&script)",
            組み立ての引用を調べる,
        ),
        // 経路 3: 「引用して展開してから外す」形を戻す
        (
            RUNNER,
            "declared_cwd_value(cwd_str, &vars)",
            "strip_quotes(vars.dialect, &expand(cwd_str))",
            作業ディレクトリの展開を調べる,
        ),
    ];
    let mut failures = Vec::new();
    for (rel, anchor, replacement, check) in cases {
        if let Err(e) = 名指すか(rel, anchor, replacement, *check) {
            failures.push(e);
        }
    }
    // `fn arg` / `declared_cwd_value` の中身を差し替えた写しは関数の行を名指す
    for (rel, sig, anchor, replacement, check) in [
        (
            PROJECT,
            "fn arg(",
            ".quote_fragment(word)",
            ".quote_arg(word)",
            組み立ての引用を調べる as 検査,
        ),
        (
            RUNNER,
            "fn declared_cwd_value(",
            "expand_literal(&strip_quotes",
            "expand_variables(&strip_quotes",
            作業ディレクトリの展開を調べる,
        ),
    ] {
        let src = read(rel);
        let fn_at = src
            .find(sig)
            .unwrap_or_else(|| panic!("{rel}: {sig} が無い"));
        let want = format!("{rel}:{}:", 行番号(&src, fn_at));
        let at = src[fn_at..]
            .find(anchor)
            .map(|a| fn_at + a)
            .unwrap_or_else(|| panic!("{rel}: {sig} の中に {anchor:?} が無い"));
        let mut injected = src.clone();
        injected.replace_range(at..at + anchor.len(), replacement);
        let hits = check(&injected, rel);
        if !hits.iter().any(|h| h.starts_with(&want)) {
            failures.push(format!(
                "{sig} の `{anchor}` → `{replacement}` の写しで {want} を名指していない: {hits:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

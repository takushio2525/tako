//! `CODEX_HOME` の解決の入口が 1 本であることの番犬（#2798）
//!
//! `CODEX_HOME` が**空文字・空白だけ**で立っているとき（Windows の PowerShell は `$null` を空文字列で
//! 渡すので、env を外したつもりの空の値が立ちやすい）、素の `var_os("CODEX_HOME")` を
//! `PathBuf::from` して join すると相対パスの `config.toml` / `AGENTS.md` / `auth.json` = **作業 dir** を
//! 見る。#2788 で MCP の片付けの経路だけを直したが、同じ読み方が診断・setup・認証の読み取り・
//! tako-cli の setup の 4 か所に残っていた（setup は作業 dir の `AGENTS.md` へ書いていた）。
//!
//! 根本原因は「解決が 1 か所に無く、各所が素の env を読んで空を値として扱う」ことなので、
//! 解決の正本を `tako_control::codex_session::codex_home`（値の読み方は `codex_home_value`）の 1 本にし、
//! **その外で `CODEX_HOME` を素で読む形**をここで `file:line` で名指す。
//!
//! # 何を禁じるか
//!
//! `var_os(` / `var(` の引数が `CODEX_HOME` を指すもの:
//!
//! - 文字列 `"CODEX_HOME"`
//! - 正本の定数 `CODEX_HOME_ENV`（パス付きでも）
//! - ファイルの中で `const <名前>: &str = "CODEX_HOME"`（または `= …::CODEX_HOME_ENV`）と束ねた別名。
//!   同じ名前の定数が複数あれば、呼び出しより前の**いちばん近い定義**で決める（関数ごとの `const KEY` の影）
//!
//! 書き込み（`set_var` / `remove_var` / `Command::env`）は読み取りではないので数えない。
//! コメントと文字列の中（この番犬自身の注入の断片・説明文）も数えない（`code_view` で潰した眺めで位置を決める）。
//! 許すのは正本の `crates/tako-control/src/codex_session.rs` だけ。
//!
//! # `.codex` の決め打ちの join（#2810）
//!
//! env を素で読まなくても、**ホームへ `.codex` を join して codex の置き場を組む**と、空でない
//! `CODEX_HOME`（例: `D:\codex`）を見ないので同じずれが起きる。#2798 の後も共通ルールの同期の書き先
//! （`agents_sync::AgentKind::target_path` の `home.join(".codex/AGENTS.md")`）と導入計画の本体の置き場
//! （`agent_install` の `payload_rel: Some(".codex/packages/standalone")`）がこの形で残っていた。
//!
//! 本番コード（`crates/*/src` の `#[cfg(test)]` の外。テスト専用のファイルも除く）で、**中身が `.codex` で
//! 始まる文字列リテラル**（`".codex"` / `".codex/…"` / `".codex\\…"`）を `file:line` で名指す。
//! 置き場は `codex_home()` から組むこと。`.codex` を名前として書いてよいもの（解決の正本の既定・プロジェクトの
//! `.codex/config.toml`・検証プロセスの置き場など）は [`ALLOWED_DOT_CODEX`] に**理由つき**で載せる。
//! 載せた行が消えたら（使われない例外）それも落とす。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

/// 素で読んでよい唯一のファイル（解決の正本）
const RESOLVER: &str = "crates/tako-control/src/codex_session.rs";

/// 正本の定数の名前
const ENV_CONST: &str = "CODEX_HOME_ENV";

/// 読み取りの呼び出し（直前が識別子の文字なら `remove_var(` / `set_var(` の一部なので数えない）
const READ_CALLS: &[&str] = &["var_os(", "var("];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// `crates/**/*.rs`（`src/` も `tests/` も。target は除く）を (リポジトリ相対パス, 本文) で集める
fn crate_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut paths = Vec::new();
    collect(&root.join("crates"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let body = std::fs::read_to_string(&path).ok()?;
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            Some((rel, body))
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 右辺が `CODEX_HOME` を指すか（文字列そのもの・正本の定数）
fn names_codex_home(expr: &str) -> bool {
    expr.contains("\"CODEX_HOME\"") || expr.split(|c: char| !is_ident(c)).any(|t| t == ENV_CONST)
}

/// `const <名前>: <型> = <右辺>;` の定義（位置・名前・`CODEX_HOME` を指すか）。
/// `text` はコメントだけを潰した眺め（文字列は残す）、`code` はコメントと文字列を潰した眺め
fn const_defs(text: &str, code: &str) -> Vec<(usize, String, bool)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("const ") {
        let start = from + at;
        from = start + "const ".len();
        if code[..start].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let Some(colon) = code[from..].find(':').map(|i| from + i) else {
            continue;
        };
        let name = code[from..colon].trim();
        if name.is_empty() || !name.chars().all(is_ident) {
            continue;
        }
        let Some(eq) = code[colon..].find('=').map(|i| colon + i) else {
            continue;
        };
        let Some(semi) = code[eq..].find(';').map(|i| eq + i) else {
            continue;
        };
        out.push((
            start,
            name.to_string(),
            names_codex_home(&text[eq + 1..semi]),
        ));
    }
    out
}

/// `open`（`(` の位置）に対応する `)` の位置（文字列を潰した眺めで数える）
fn matching_paren(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in code.bytes().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 1 ファイルの中で `CODEX_HOME` を素で読む呼び出しを `file:line 引数` で返す
fn raw_reads(rel: &str, src: &str) -> Vec<String> {
    let text = code_view::without_comments(src);
    let code = code_view::code_view(src);
    let defs = const_defs(&text, &code);
    // 呼び出しより前のいちばん近い定義（無ければ後ろの最初の定義 = モジュールの頭の外に置いた定数）
    let alias_is_codex = |name: &str, at: usize| {
        let same = || defs.iter().filter(|(_, n, _)| n == name);
        same()
            .rfind(|(pos, _, _)| *pos < at)
            .or_else(|| same().next())
            .is_some_and(|(_, _, codex)| *codex)
    };
    let mut out = Vec::new();
    for call in READ_CALLS {
        let mut from = 0;
        while let Some(at) = code[from..].find(call) {
            let start = from + at;
            from = start + call.len();
            if code[..start].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            let open = start + call.len() - 1;
            let Some(close) = matching_paren(&code, open) else {
                continue;
            };
            let arg = text[open + 1..close].trim();
            let hits = names_codex_home(arg)
                || arg
                    .split(|c: char| !is_ident(c))
                    .any(|t| !t.is_empty() && alias_is_codex(t, start));
            if hits {
                let line = code[..start].matches('\n').count() + 1;
                out.push(format!("{rel}:{line} {call}{arg})"));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn codex_homeを素で読むのは解決の正本だけ() {
    let offenders: Vec<String> = crate_sources()
        .iter()
        .filter(|(rel, _)| rel != RESOLVER)
        .flat_map(|(rel, body)| raw_reads(rel, body))
        .collect();
    assert!(
        offenders.is_empty(),
        "`CODEX_HOME` を解決の外で素で読んでいる: {offenders:#?}\n\
         codex の置き場は tako_control::codex_session::codex_home()（env 計画など値だけを持つときは \
         codex_home_value）を通すこと。素の読み出しは空・空白だけの値を置き場として join し、\
         作業 dir の config.toml / AGENTS.md / auth.json を見る（#2788 / #2798）"
    );
}

/// 番犬自身が「何も走査していない」状態で緑にならないこと（走査の場所・needle が実装とずれたら落ちる）
#[test]
fn 走査が空振りしていない() {
    let sources = crate_sources();
    assert!(
        sources.len() > 500,
        "走査したファイルが少なすぎる（走査に失敗している）: {} 本",
        sources.len()
    );
    let resolver = sources
        .iter()
        .find(|(rel, _)| rel == RESOLVER)
        .expect("解決の正本が在る");
    let hits = raw_reads(&resolver.0, &resolver.1);
    assert_eq!(
        hits.len(),
        1,
        "正本の素の読み出し（`var_os(CODEX_HOME_ENV)`）を 1 つだけ見つけるはず: {hits:#?}"
    );
}

/// 名指す形と、名指さない形（番犬自身の検出力）
#[test]
fn 注入した素の読み出しを名指す() {
    let named = |src: &str| raw_reads("x.rs", src);
    // 文字列そのもの（var_os / var の両方・複数行の呼び出しは var_os( の行）
    assert_eq!(
        named("fn a() {\n    let v = std::env::var_os(\"CODEX_HOME\");\n}\n"),
        vec!["x.rs:2 var_os(\"CODEX_HOME\")"]
    );
    assert_eq!(
        named("fn a() { std::env::var(\"CODEX_HOME\").ok(); }\n"),
        vec!["x.rs:1 var(\"CODEX_HOME\")"]
    );
    assert_eq!(
        named("fn a() {\n    std::env::var_os(\n        \"CODEX_HOME\",\n    );\n}\n").len(),
        1
    );
    // 正本の定数（パス付き・use して素の名前）
    assert_eq!(
        named("fn a() { std::env::var_os(crate::codex_session::CODEX_HOME_ENV); }\n").len(),
        1
    );
    assert_eq!(named("fn a() { var_os(CODEX_HOME_ENV); }\n").len(), 1);
    // ファイルの中の別名（文字列 / 正本の定数へ束ねたもの）
    assert_eq!(
        named("fn a() {\n    const KEY: &str = \"CODEX_HOME\";\n    std::env::var_os(KEY);\n}\n"),
        vec!["x.rs:3 var_os(KEY)"]
    );
    assert_eq!(
        named(
            "const K: &str = tako_control::codex_session::CODEX_HOME_ENV;\n\
             fn a() { std::env::var(K).ok(); }\n"
        )
        .len(),
        1
    );
    // 名指さない: 書き込み・コメント・文字列の中・他の env・影になった別名
    let quiet = [
        "fn a() { std::env::remove_var(\"CODEX_HOME\"); std::env::set_var(\"CODEX_HOME\", \"\"); }\n",
        "fn a() {\n    // std::env::var_os(\"CODEX_HOME\")\n}\n",
        "/// `var_os(\"CODEX_HOME\")` を読まない\nfn a() {}\n",
        "fn a() { let s = \"std::env::var_os(\\\"CODEX_HOME\\\")\"; }\n",
        "fn a() { cmd.env(\"CODEX_HOME\", dir).env_remove(\"CODEX_HOME\"); }\n",
        "fn a() { std::env::var_os(\"CLAUDE_CONFIG_DIR\"); }\n",
        "fn a() {\n    const KEY: &str = \"CODEX_HOME\";\n    plan.export_value(KEY);\n}\n\
         fn b() {\n    const KEY: &str = \"TAKO_OTHER\";\n    std::env::var_os(KEY);\n}\n",
    ];
    for src in quiet {
        assert_eq!(named(src), Vec::<String>::new(), "名指さない形: {src}");
    }
}

/// 実ファイルへ修正前の読み方を注入すると、そのファイルの行で名指す（走査が実ファイルの形で生きている証拠）。
/// 置き換える断片が見つからなければ、読み手の書き方が変わったので注入の表を直すこと
#[test]
fn 実ファイルへ素の読み出しを戻すと名指す() {
    // 修正前の読み方（空の値を置き場として PathBuf にする）
    const RAW: &str = "std::env::var_os(\"CODEX_HOME\").map(std::path::PathBuf::from)";
    // (ファイル, 読み手の断片, その中の正本の呼び出し) = 読み手ごとに 1 つ
    const IN_CONTROL: &str = "crate::codex_session::codex_home()";
    const IN_CLI: &str = "tako_control::codex_session::codex_home()";
    let cases: &[(&str, &str, &str)] = &[
        (
            "crates/tako-control/src/agent_mcp.rs",
            "crate::codex_session::codex_home().map(|dir| dir.join(\"config.toml\"))",
            IN_CONTROL,
        ),
        (
            "crates/tako-control/src/diagnostics.rs",
            "crate::codex_session::codex_home()?.join(\"AGENTS.md\")",
            IN_CONTROL,
        ),
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "let home = crate::codex_session::codex_home()?;",
            IN_CONTROL,
        ),
        (
            "crates/tako-cli/src/setup.rs",
            "tako_control::codex_session::codex_home()?.join(\"AGENTS.md\")",
            IN_CLI,
        ),
    ];
    let root = repo_root();
    for (rel, fragment, call) in cases {
        let body = std::fs::read_to_string(root.join(rel)).expect("読み手のファイル");
        assert_eq!(
            raw_reads(rel, &body),
            Vec::<String>::new(),
            "{rel} は今は通る"
        );
        let at = body.find(fragment).unwrap_or_else(|| {
            panic!("{rel} に注入する断片が無い（読み手の書き方が変わった）: {fragment}")
        });
        let fragment_line = body[..at].matches('\n').count() + 1;
        let injected = body.replacen(fragment, &fragment.replacen(call, RAW, 1), 1);
        let hits = raw_reads(rel, &injected);
        assert_eq!(hits.len(), 1, "{rel} の注入を 1 つ名指すはず: {hits:#?}");
        assert!(
            hits[0].starts_with(&format!("{rel}:{fragment_line} ")),
            "{rel} の注入した行（{fragment_line}）を名指すはず: {hits:#?}"
        );
    }
}

// --- #2810: `.codex` の決め打ちの join ---------------------------------------------

/// 本番コードで `.codex` を名前として書いてよいもの（ファイル, その行の断片, 理由）。
/// 断片はコメントを潰した眺めの 1 行に含まれる形で書く（rustfmt の折り返しをまたがない）
const ALLOWED_DOT_CODEX: &[(&str, &str, &str)] = &[
    (
        RESOLVER,
        "h.join(\".codex\")",
        "解決の正本の既定（`CODEX_HOME` が空・空白だけ・未設定のとき `<home>/.codex`）",
    ),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "dir.join(\".codex\").join(\"config.toml\")",
        "プロジェクトの `.codex/config.toml`（cwd から git の根までの各 dir。codex の置き場ではない）",
    ),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "agent_config_home().map(|h| h.join(\".codex\"))",
        "env 計画が `CODEX_HOME` を明示で外したとき / 検証プロセスの置き場（本物の `CODEX_HOME` を読まない意図）",
    ),
    (
        "crates/tako-control/src/usage/index.rs",
        "vec![normalize(&h.join(\".codex\"))]",
        "検証プロセスの置き場（本物の `CODEX_HOME` を読まない意図。`codex_homes` の doc）",
    ),
    (
        "crates/tako-control/src/usage/index.rs",
        "\"\" | \".codex\" =>",
        "置き場を組まない（既定の置き場の名前を表示名 `codex` へ読み替えるだけ）",
    ),
];

/// 本番コードの眺め: `#[cfg(test)]` の領域とコメントを空白へ潰す（文字列は残す・行番号は原文のまま）
fn production_text(src: &str) -> String {
    code_view::without_comments(&production_range::scan(src).text)
}

/// 中身が `.codex` で始まる文字列リテラル（`".codex"` / `".codex/…"` / `".codex\…"`）を
/// (行番号, その行) で返す。`text` は [`production_text`] の眺め
fn dot_codex_literals(text: &str) -> Vec<(usize, String)> {
    const NEEDLE: &str = "\".codex";
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(NEEDLE) {
        let start = from + at;
        from = start + NEEDLE.len();
        // 文字列の中のエスケープした引用符（`"…\".codex…"`）は開きの引用符ではない
        if text[..start].ends_with('\\') {
            continue;
        }
        // `.codexrc` のような別の名前は数えない（終わり・区切りが続くものだけ）
        if !matches!(text[from..].chars().next(), Some('"' | '/' | '\\')) {
            continue;
        }
        let line = text[..start].matches('\n').count() + 1;
        out.push((line, lines.get(line - 1).unwrap_or(&"").to_string()));
    }
    out
}

/// 1 ファイルの名指し（例外に載っていない `.codex` のリテラル）と、使った例外の添字
fn dot_codex_offenders(rel: &str, src: &str) -> (Vec<String>, Vec<usize>) {
    let mut offenders = Vec::new();
    let mut used = Vec::new();
    for (line, text) in dot_codex_literals(&production_text(src)) {
        let allowed = ALLOWED_DOT_CODEX
            .iter()
            .position(|(file, fragment, _)| *file == rel && text.contains(fragment));
        match allowed {
            Some(i) => used.push(i),
            None => offenders.push(format!("{rel}:{line} {}", text.trim())),
        }
    }
    (offenders, used)
}

/// `crates/*/src` の本番のファイル（テスト専用のファイルを除く）を (相対パス, 本文) で
fn product_files() -> Vec<(String, String)> {
    let files = production_range::product_sources();
    let test_only = production_range::test_only_files(&files);
    let root = repo_root();
    files
        .into_iter()
        .filter(|rel| !test_only.contains(rel))
        .map(|rel| {
            let body = std::fs::read_to_string(root.join(&rel)).expect("本番のファイル");
            (rel, body)
        })
        .collect()
}

#[test]
fn issue2810_codexの置き場をdot_codexの決め打ちで組まない() {
    let files = product_files();
    assert!(
        files.len() > 300,
        "走査した本番のファイルが少なすぎる（走査に失敗している）: {} 本",
        files.len()
    );
    let mut offenders = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for (rel, body) in &files {
        let (named, hit) = dot_codex_offenders(rel, body);
        offenders.extend(named);
        used.extend(hit);
    }
    assert!(
        offenders.is_empty(),
        "codex の置き場を `.codex` の決め打ちで組んでいる: {offenders:#?}\n\
         空でない `CODEX_HOME`（例: D:\\codex）を見ないので、診断・setup が見る置き場とずれる。\
         置き場は tako_control::codex_session::codex_home() から組むこと（tako-core の中なら解決済みの値を\
         引数で受ける = agent_install の payload_dir_in）。codex の置き場ではない `.codex`（プロジェクトの\
         `.codex/config.toml` など）なら、理由を添えて ALLOWED_DOT_CODEX に載せる（#2810）"
    );
    let unused: Vec<String> = ALLOWED_DOT_CODEX
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, (file, fragment, _))| format!("{file}: {fragment}"))
        .collect();
    assert!(
        unused.is_empty(),
        "使われていない例外（行が消えた・書き方が変わった）: {unused:#?}\n\
         直したなら ALLOWED_DOT_CODEX から消し、書き方が変わったなら断片を直す"
    );
}

/// 名指す形と、名指さない形（番犬自身の検出力）
#[test]
fn issue2810_注入したdot_codexの決め打ちを名指す() {
    let named = |src: &str| dot_codex_offenders("x.rs", src).0;
    for (src, line) in [
        (
            "fn a(h: &Path) -> PathBuf {\n    h.join(\".codex\")\n}\n",
            2,
        ),
        (
            "fn a(h: &Path) -> PathBuf { h.join(\".codex/AGENTS.md\") }\n",
            1,
        ),
        (
            "const R: Option<&str> = Some(\".codex/packages/standalone\");\n",
            1,
        ),
        ("fn a() { Path::new(r\".codex\\config.toml\"); }\n", 1),
        ("fn a() { Path::new(\".codex\\\\config.toml\"); }\n", 1),
        ("fn a() { let p = r#\".codex\"#; }\n", 1),
    ] {
        let hits = named(src);
        assert_eq!(hits.len(), 1, "名指す形: {src}");
        assert!(hits[0].starts_with(&format!("x.rs:{line} ")), "{hits:?}");
    }
    let quiet = [
        // コメント・doc の中
        "fn a() {\n    // h.join(\".codex\")\n}\n",
        "/// `home.join(\".codex/AGENTS.md\")` を組まない\nfn a() {}\n",
        // 説明文の途中・別の名前・エスケープした引用符の中
        "fn a() { msg(\"プロジェクトの .codex/config.toml と同名\"); }\n",
        "fn a() { h.join(\".codexrc\"); h.join(\"x.codex\"); }\n",
        "fn a() { msg(\"say \\\".codex/\\\"\"); }\n",
        // テストの中（`#[cfg(test)]` の領域は本番コードではない）
        "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b(h: &Path) { h.join(\".codex\"); }\n}\n",
    ];
    for src in quiet {
        assert_eq!(named(src), Vec::<String>::new(), "名指さない形: {src}");
    }
}

/// 実ファイルへ修正前の決め打ちを戻すと、そのファイルの行で名指す（走査が実ファイルの形で生きている証拠）。
/// 置き換える断片が見つからなければ、読み手の書き方が変わったので注入の表を直すこと
#[test]
fn issue2810_実ファイルへ決め打ちを戻すと名指す() {
    // (ファイル, 今の断片, 修正前の形)
    let cases: &[(&str, &str, &str)] = &[
        (
            "crates/tako-control/src/agents_sync.rs",
            "crate::codex_session::codex_home()?.join(\"AGENTS.md\")",
            "home.join(\".codex/AGENTS.md\")",
        ),
        (
            "crates/tako-core/src/platform/agent_install.rs",
            "payload_rel: Some(\"packages/standalone\"),",
            "payload_rel: Some(\".codex/packages/standalone\"),",
        ),
        // 計画の根を正本を通さずに組む形
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "crate::codex_session::codex_home().ok_or(SetupRefusal::BootstrapHomeUnresolved)?",
            "home.join(\".codex\")",
        ),
        // #2816: worker の信頼の書き先（#2810 では「既知の残り」として凍結していた形）
        (
            "crates/tako-control/src/orchestrator/agent.rs",
            "Path::new(cwd).join(codex_home).join(\"config.toml\")",
            "home.join(\".codex/config.toml\")",
        ),
    ];
    let root = repo_root();
    for (rel, fragment, before) in cases {
        let body = std::fs::read_to_string(root.join(rel)).expect("読み手のファイル");
        assert_eq!(
            dot_codex_offenders(rel, &body).0,
            Vec::<String>::new(),
            "{rel} は今は通る"
        );
        let at = body.find(fragment).unwrap_or_else(|| {
            panic!("{rel} に注入する断片が無い（読み手の書き方が変わった）: {fragment}")
        });
        let fragment_line = body[..at].matches('\n').count() + 1;
        let injected = body.replacen(fragment, before, 1);
        let hits = dot_codex_offenders(rel, &injected).0;
        assert_eq!(hits.len(), 1, "{rel} の注入を 1 つ名指すはず: {hits:#?}");
        assert!(
            hits[0].starts_with(&format!("{rel}:{fragment_line} ")),
            "{rel} の注入した行（{fragment_line}）を名指すはず: {hits:#?}"
        );
    }
}

//! **#2659 の番犬**: PowerShell の統合スクリプトを、読み手と書き手が同じファイルで取り合う形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! Windows の CI で `crates/tako-core/tests/shell_integration_powershell.rs` が間欠的に落ちていた
//! （#2432 の着地後 306 回のテスト段で 2 回）。同じテストプロセスの 10 本が同じ
//! `%TEMP%\tako-si-test-<pid>\tako.ps1` を書き直しており、あるテストのペインの PowerShell が読む瞬間に
//! 別のテストが書くと、Windows では片方が共有違反（OS エラー 32）で失敗する。
//!
//! - 11/01: 書き手の `std::fs::write` が `The process cannot access the file …` で panic
//! - 11/02: 読み手（製品の起動形の `ReadAllText`）が失敗し、製品の本文が `catch { }` で**黙って**捨てたので
//!   統合が入らず、`wait_ready` の 30 秒待ちが切れた（画面の `Cannot load PSReadline module.` は
//!   主因ではない = 起動の後の `-EncodedCommand` は PSReadLine の成否に依らず走り、統合にも
//!   PSReadLine 抜きの経路がある）
//!
//! 直したのは 3 点:
//!
//! - テストはテストごとの置き場（`ScratchDir`）へ書く = 自分のファイルを書いてから自分のペインに読ませる
//! - 製品の本文（`shell_integration::powershell_bootstrap_in`）は共有違反（32 / 33）のときだけ
//!   上限つきで読み直し、読めなければ統合が入らなかったことを警告で出す
//! - 製品の書き出し（`shell_integration::write_powershell_script`）は中身が同じなら書き直さない
//!   （別のプロセスが開いたばかりのペインと重ならない）
//!
//! ## ここで止めるもの
//!
//! 1. [`製品の本文は読み直して読めなければ知らせる`] / [`製品の書き出しは中身が同じなら書かない`] —
//!    `crates/tako-core/src/shell_integration.rs` の形を file:line で拘束する（A/B の腕
//!    `if issue2659_legacy() {` の外で見る）
//! 2. [`テストはテストごとの置き場へ書く`] — 統合スクリプトを自分で書き出すテスト（正本を
//!    `include_str!` で埋めるファイル）の書き出しの呼び手が、同じ関数の中で `ScratchDir::new(` を通すこと・
//!    書き出しの本体が `temp_dir()` / `process::id()` で置き場を自分で決めないこと
//! 3. [`ab_のゲートは1か所`] — `TAKO_2659_LEGACY` を読む製品コードは 1 か所
//! 4. [`修正前の形の注入を名指す`] — 1・2 の検査が修正前の形（1 回だけ読んで黙る・毎回書き直す・
//!    プロセスで共有する置き場）の注入を file:line で名指すこと
//!
//! 動作そのもの（ロックを握ってから放す注入で、修正前は統合なし・修正後は読み直して入る・握られたままなら
//! 警告が出る）は Windows の `shell_integration_powershell.rs` の `issue2659_*` が実シェルで見る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const SI: &str = "crates/tako-core/src/shell_integration.rs";

/// 製品コードを持つ 4 クレート（A/B のゲートの数を数える範囲）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 統合スクリプトの正本を埋める綴り（これを持つテストは自分でスクリプトを書き出している）
const EMBED: &str = "include_str!(\"../shell-integration/tako.ps1\")";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ。文字列は残る）
fn product_text(raw: &str) -> String {
    production_range::scan(raw).text
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

/// 関数 `name` の製品の経路（入れ子の関数と A/B の旧経路の腕を除く）の本文を、文字列ごと返す。
/// 範囲は字面の眺め（文字列を潰したもの）で決め、中身は原文から切り出す（PowerShell の本文は文字列の中）
struct Body {
    line: usize,
    /// (開始位置, 本文) の並び
    parts: Vec<(usize, String)>,
    /// A/B の腕も含めた本文
    whole: String,
}

impl Body {
    fn of(text: &str, rel: &str, name: &str) -> Result<Body, String> {
        let graph = call_graph::Graph::build(vec![call_graph::Source::new(rel, text)]);
        let f = graph
            .find(rel, name)
            .into_iter()
            .min_by_key(|&f| graph.fns[f].line)
            .ok_or_else(|| format!("{rel} に `fn {name}` が無い（改名したなら番犬も追うこと）"))?;
        let def = &graph.fns[f];
        Ok(Body {
            line: def.line,
            parts: graph
                .product_ranges(f)
                .into_iter()
                .map(|(s, e)| (s, text[s..e].to_string()))
                .collect(),
            whole: text[def.body.0..=def.body.1].to_string(),
        })
    }

    /// 製品の経路での最初の出現（原文のバイト位置）
    fn find(&self, needle: &str) -> Option<usize> {
        self.parts
            .iter()
            .find_map(|(s, part)| part.find(needle).map(|i| s + i))
    }
}

/// 1 の本体（製品）: shell_integration.rs の本文から、修正前の形へ戻った箇所を file:line で並べる
fn product_offenders(raw: &str) -> Vec<String> {
    let text = product_text(raw);
    let mut out = Vec::new();

    match Body::of(&text, SI, "powershell_bootstrap_in") {
        Err(e) => out.push(e),
        Ok(body) => {
            let at = |needle: &str| body.find(needle);
            let line = body.line;
            if !body.whole.contains("issue2659_legacy()") {
                out.push(format!(
                    "{SI}:{line}（fn powershell_bootstrap_in）に A/B のゲート `issue2659_legacy()` が無い（修正前の本文を同じバイナリで再現できない。#2659）"
                ));
            }
            match (at("while ($true)"), at("ReadAllText($__takoShellIntegrationScript)")) {
                (Some(l), Some(r)) if l < r => {}
                _ => out.push(format!(
                    "{SI}:{line}（fn powershell_bootstrap_in）が統合スクリプトを読み直しの輪の中で読んでいない = 1 回の共有違反で統合が入らない（#2659）"
                )),
            }
            if at("@(32, 33) -notcontains").is_none() {
                out.push(format!(
                    "{SI}:{line}（fn powershell_bootstrap_in）が読み直しを共有違反（ERROR_SHARING_VIOLATION 32 / ERROR_LOCK_VIOLATION 33）に絞っていない（#2659）"
                ));
            }
            if at("POWERSHELL_SCRIPT_READ_RETRIES").is_none() {
                out.push(format!(
                    "{SI}:{line}（fn powershell_bootstrap_in）の読み直しに上限（POWERSHELL_SCRIPT_READ_RETRIES）が無い（#2659）"
                ));
            }
            if at("Write-Warning").is_none() {
                out.push(format!(
                    "{SI}:{line}（fn powershell_bootstrap_in）が読めなかったことを知らせない = 統合が理由の出ないまま入らない（#2659）"
                ));
            }
            if let Some(i) = at("try {{ . ([scriptblock]::Create([System.IO.File]::ReadAllText(") {
                out.push(format!(
                    "{SI}:{}（fn powershell_bootstrap_in）が読み込みと実行を 1 つの `catch {{ }}` で黙って捨てる形（修正前）に戻っている（#2659）",
                    line_of(&text, i)
                ));
            }
        }
    }

    match Body::of(&text, SI, "write_powershell_script") {
        Err(e) => out.push(e),
        Ok(body) => match (body.find("std::fs::read(&path)"), body.find("std::fs::write(&path")) {
            (Some(r), Some(w)) if r < w => {}
            (_, Some(w)) => out.push(format!(
                "{SI}:{}（fn write_powershell_script）が今の中身を確かめずに書き直す = 開いたばかりのペインの読み込みと重なる（#2659）",
                line_of(&text, w)
            )),
            _ => out.push(format!(
                "{SI}:{}（fn write_powershell_script）に書き出し（`std::fs::write(&path`）が見当たらない（番犬も追うこと）",
                body.line
            )),
        },
    }
    out
}

/// 2 の本体（テスト）: 統合スクリプトを書き出すテストファイルから、置き場を他のテストと共有しうる
/// 書き出しを file:line で並べる。返すのは (名指し, 見た呼び手の数)
fn test_offenders(rel: &str, raw: &str) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let graph = call_graph::Graph::build(vec![call_graph::Source::new(rel, raw)]);
    let src = &graph.sources[0];
    let fns = src.fns();
    let body_of = |f: &call_graph::FnDef| &raw[f.body.0..=f.body.1];

    let writers: Vec<&call_graph::FnDef> =
        fns.iter().filter(|f| body_of(f).contains(EMBED)).collect();
    if writers.is_empty() {
        out.push(format!(
            "{rel}: 正本を埋める綴り（{EMBED}）を持つ関数が見つからない（書き出しの形を変えたなら番犬も追うこと）"
        ));
        return (out, 0);
    }
    let mut callers = 0;
    for w in writers {
        // コメントに綴りが出ても拾わないよう、字面の眺め（コメント・文字列を潰したもの）で見る
        let code = &src.code[w.body.0..=w.body.1];
        for leak in ["temp_dir()", "process::id()"] {
            if let Some(i) = code.find(leak) {
                out.push(format!(
                    "{rel}:{}（fn {}）が統合スクリプトの置き場を自分で決めている（`{leak}`）= 同じプロセスのテストが同じファイルを取り合う（#2659）",
                    src.line_of(w.body.0 + i),
                    w.name
                ));
            }
        }
        let call = format!("{}(", w.name);
        for c in fns.iter().filter(|c| !std::ptr::eq(*c, w)) {
            let code = &src.code[c.body.0..=c.body.1];
            let Some(i) = code.find(&call) else { continue };
            // 名前の一部が一致しただけのもの（`write_script_in` に対する `my_write_script_in`）は外す
            let before = code[..i].chars().next_back();
            if before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
                continue;
            }
            callers += 1;
            if !code.contains("ScratchDir::new(") {
                out.push(format!(
                    "{rel}:{}（fn {}）が統合スクリプトを書き出す置き場を `ScratchDir::new(` で作っていない = 他のテストと同じファイルを取り合いうる（#2659）",
                    src.line_of(c.body.0 + i),
                    c.name
                ));
            }
        }
    }
    (out, callers)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("読める").flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// 統合スクリプトの正本を埋めているテストファイル（ワークスペースルートからの相対パス, 本文）
fn embedding_tests() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates"))
        .expect("crates/ が読める")
        .flatten()
    {
        let tests = krate.path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let raw = std::fs::read_to_string(&path).ok()?;
            raw.contains(EMBED).then(|| {
                let rel = path
                    .strip_prefix(&root)
                    .expect("中")
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, raw)
            })
        })
        .collect()
}

/// 1: 今の本文は共有違反だけを上限つきで読み直し、読めなければ警告を出す
#[test]
fn 製品の本文は読み直して読めなければ知らせる() {
    let hits: Vec<String> = product_offenders(&read(SI))
        .into_iter()
        .filter(|h| h.contains("powershell_bootstrap_in"))
        .collect();
    assert!(
        hits.is_empty(),
        "ペインの PowerShell が統合スクリプトを読む形が修正前へ戻っている（#2659）:\n  {}",
        hits.join("\n  ")
    );
}

/// 1: 今の書き出しは中身が同じなら書かない
#[test]
fn 製品の書き出しは中身が同じなら書かない() {
    let hits: Vec<String> = product_offenders(&read(SI))
        .into_iter()
        .filter(|h| h.contains("write_powershell_script"))
        .collect();
    assert!(
        hits.is_empty(),
        "統合スクリプトの書き出しが修正前（毎回書き直す）へ戻っている（#2659）:\n  {}",
        hits.join("\n  ")
    );
}

/// 2: 統合スクリプトを書き出すテストは、テストごとの置き場へ書く（走査が空振りしていないことも見る）
#[test]
fn テストはテストごとの置き場へ書く() {
    let files = embedding_tests();
    assert!(
        files
            .iter()
            .any(|(rel, _)| rel == "crates/tako-core/tests/shell_integration_powershell.rs"),
        "正本を埋めるテストに shell_integration_powershell.rs が見つからない（走査が空振りしている）: {:?}",
        files.iter().map(|(r, _)| r).collect::<Vec<_>>()
    );
    let mut hits = Vec::new();
    let mut callers = 0;
    for (rel, raw) in &files {
        let (h, c) = test_offenders(rel, raw);
        hits.extend(h);
        callers += c;
    }
    assert!(
        hits.is_empty(),
        "統合スクリプトを他のテストと同じ置き場へ書きうる（#2659）:\n  {}\n\n\
         同じプロセスのテストが同じファイルを書き直すと、Windows では読み手か書き手が共有違反で落ちる。\
         置き場は `tako_core::test_residue::ScratchDir::new(\"<タグ>\")` で作ること",
        hits.join("\n  ")
    );
    // 呼び手は少なくとも write_script と #2432 の「空白入りの置き場」の 2 本（空振りしていない = 見ている数の下限）
    assert!(
        callers >= 2,
        "書き出しの呼び手を {callers} 本しか見ていない（走査が空振りしている）"
    );
}

/// 3: A/B のゲートは製品コードに 1 か所（`TAKO_2659_LEGACY` を読むのは `shell_integration::issue2659_legacy` だけ）
#[test]
fn ab_のゲートは1か所() {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ROOTS {
        walk(&root.join(dir), &mut files);
    }
    let mut hits = Vec::new();
    for path in files {
        let raw = std::fs::read_to_string(&path).expect("読める");
        let text = product_text(&raw);
        for (i, _) in text.match_indices("\"TAKO_2659_LEGACY\"") {
            let rel = path
                .strip_prefix(&root)
                .expect("中")
                .to_string_lossy()
                .replace('\\', "/");
            hits.push(format!("{rel}:{}", line_of(&text, i)));
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`TAKO_2659_LEGACY` を読む製品コードは shell_integration::issue2659_legacy の 1 か所だけ: {hits:?}"
    );
    assert!(hits[0].starts_with(SI), "{hits:?}");
}

/// 4: 修正前の形の注入を 1・2 の検査が file:line で名指す（同じテストの中の A/B。ファイルは触らない）
#[test]
fn 修正前の形の注入を名指す() {
    let raw = read(SI);
    let product_cases: [(&str, &str, &str); 4] = [
        (
            "読み直しの輪を外して 1 回だけ読む",
            "while ($true) {{ \\",
            "if ($true) {{ \\",
        ),
        (
            "読めなかったことを知らせない（修正前の黙る形）",
            "else {{ Write-Warning ({warning}) }}",
            "else {{ }}",
        ),
        (
            "共有違反以外も読み直す（上限まで待たされる）",
            "(@(32, 33) -notcontains ($__takoShellIntegrationErr.HResult -band 0xFFFF))",
            "($false)",
        ),
        (
            "中身を確かめずに毎回書き直す",
            "if !issue2659_legacy() && std::fs::read(&path).is_ok_and(|current| current == bytes) {",
            "if false {",
        ),
    ];
    for (what, from, to) in product_cases {
        assert!(
            raw.contains(from),
            "注入の起点が {SI} に無い（{what}。番犬も追うこと）: {from}"
        );
        let hits = product_offenders(&raw.replacen(from, to, 1));
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{SI}:"))),
            "注入（{what}）を file:line で名指せない: {hits:?}"
        );
    }

    const TESTS: &str = "crates/tako-core/tests/shell_integration_powershell.rs";
    let raw = read(TESTS);
    let test_cases: [(&str, &str, &str); 2] = [
        (
            "プロセスで共有する置き場へ書く（修正前の write_script）",
            "    let dir = tako_core::test_residue::ScratchDir::new(\"i2659-si\");\n    let path = write_script_in(dir.path());",
            "    let path = write_script_in(&std::env::temp_dir().join(format!(\"tako-si-test-{}\", std::process::id())));",
        ),
        (
            "書き出しの本体が置き場を自分で決める",
            "    std::fs::create_dir_all(dir).expect(\"一時ディレクトリを作れること\");",
            "    let dir = &std::env::temp_dir().join(\"tako-si-shared\");\n    std::fs::create_dir_all(dir).expect(\"一時ディレクトリを作れること\");",
        ),
    ];
    for (what, from, to) in test_cases {
        assert!(
            raw.contains(from),
            "注入の起点が {TESTS} に無い（{what}。番犬も追うこと）: {from}"
        );
        let (hits, _) = test_offenders(TESTS, &raw.replacen(from, to, 1));
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{TESTS}:"))),
            "注入（{what}）を file:line で名指せない: {hits:?}"
        );
    }
}

//! **#1790 の番犬**: 製品ファイル（`crates/*/src/` の `.rs` すべて）に、理由も丸めも無い
//! 危ない形の範囲添字（文字列のバイト位置での切り出し）が無い。
//!
//! ## なぜ止めるのか
//!
//! `&s[..N]` / `&s[a..]` の添字は**バイト位置**なので、多バイト文字の途中に当たると panic する。
//! 同じ型が #1728（プレビューの描画中）/ #1746（tako-cli の gate）/ #1756（テーマ色の検査）/
//! #2020（番犬の固定幅の窓）と続き、そのたびに番犬を 1 ファイル・1 crate ずつ足してきた。
//! #1757 の時点で、危ない形に絞っても理由も丸めも無いものが本番コードに 50 件あり
//! （2026-09-27）、2026-10-16 には 81 件 + 製品ファイルの `#[cfg(test)]` に 68 件まで増えていた。
//! #1790 で全件を仕分け（直す / 理由コメント）、視界を製品ファイル全体へ広げた。
//!
//! ## 何を固定するか
//!
//! 判定は `common/range_index.rs` の [`sites`] の 1 実装（#2043 の番犬と共有する）。
//!
//! 1. **本番の領域**（[`Rule::Risky`]）: 範囲添字のうち、次の危ない形を落とす
//!    - 固定幅の窓（`at..at + 400` / `at.saturating_sub(N)..at` / `..80`）
//!    - 境界が数値リテラルそのもの（`[1..]` / `[..5]` / `[124..136]`。0 は数えない）
//!    - `.min(` を含む（`[..s.len().min(8)]` / `[..offset.min(len)]`）
//!    - `len()` から引く（`[..s.len() - 1]` / `[s.len().saturating_sub(20)..]`）
//! 2. **テストの領域**（`test` を正の位置に含む cfg の中と、そういう cfg の `mod x;` で読む
//!    ファイル = `production_range` と同じ読み。[`Rule::FixedWindow`]）: `crates/*/tests/` と
//!    同じ固定幅の窓（#2043）。直し方は std の `ceil_char_boundary` / `floor_char_boundary` で
//!    端を外側へ丸める（`byte_window` は `crates/*/tests/` からしか取り込めない）
//!
//! 通すもの: `floor_char_boundary(` / `ceil_char_boundary(` で丸めてある・同じ行か 1 行上に
//! `// 切り出し安全: <理由>`・受け手が文字列ではない（宣言の型が `Vec` / スライス / 配列・
//! `vec![…]` / `collect::<Vec<…>>()` / `….to_vec()` / `….as_bytes()` / `[0u8; N]` で作った・
//! 直後が `.iter()` / `.join(` 等・受け手が `….as_bytes()`）。
//! `[i + 1..]`（`find` した区切りの次）のような可変の位置は数えない（多くは ASCII の区切りで、
//! 全部に理由を書かせると理由コメントが読まれなくなる。既知の穴）。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙・テスト専用ファイルの見分け・形の判定が実ファイルで
//! 生きていることを、[`修正前の形を実際に落とす`] で #1790 で直した形が `file:line` で
//! 名指しされることを、[`直した実ファイルを直書きへ戻すと名指しする`] で**実際のファイルの
//! 1 行を修正前へ戻した本文**が落ちることを確かめる。[`テストの窓の直し方は端の多バイト文字で落ちない`]
//! は修正前の式が panic し直した式が通ることを、[`手の丸めを置き換えた式は同じ位置を返す`] は
//! 手書きの丸めを std へ置き換えた 4 か所が挙動を変えないことを確かめる（注入は `cargo test` の
//! 中で完結し、リポジトリのファイルは触らない）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// 本番コードとテスト領域の境界は 1 実装（#1420 / #1445）。`code_view` もここから使う
#[path = "common/production_range.rs"]
mod production_range;

// 範囲添字の検出と、形・受け手の判定は #1728 / #1746 / #2043 の番犬と共有する 1 実装
#[path = "common/range_index.rs"]
mod range_index;

use production_range::code_view;
use range_index::{sites, Rule, Site, Verdict, REASON_MARK};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/src/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn product_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} が読める: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("ディレクトリ項目").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ が読める");
    for entry in crates {
        let src = entry.expect("crate の項目").path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

// --------------------------------------------------------------- テスト専用のファイル

/// `mod NAME;`（外のファイルを読む宣言）なら NAME
fn out_of_line_mod(line: &str) -> Option<&str> {
    let mut t = line.trim();
    for vis in ["pub(crate) ", "pub(super) ", "pub "] {
        if let Some(rest) = t.strip_prefix(vis) {
            t = rest;
            break;
        }
    }
    let name = t.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

/// 宣言の直前の属性行にある `#[path = "…"]` の値（コメント行と他の属性は飛ばす）
fn path_attr<'a>(raw_lines: &[&'a str], decl: usize) -> Option<&'a str> {
    for l in raw_lines[..decl].iter().rev() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("#[path = \"") {
            return rest.strip_suffix("\"]");
        }
        if !(t.starts_with("#[") || t.starts_with("//")) {
            return None;
        }
    }
    None
}

/// `parent` が `mod name;` で読むファイル（リポジトリルートからの相対パス）
fn resolve_mod(parent: &str, name: &str, path: Option<&str>) -> String {
    let dir = Path::new(parent).parent().expect("親のディレクトリ");
    let file = Path::new(parent)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let is_mod_rs = matches!(file, "main.rs" | "lib.rs" | "mod.rs")
        || dir.file_name().is_some_and(|d| d == "bin");
    let candidates: Vec<PathBuf> = match path {
        Some(p) => vec![dir.join(p)],
        None => {
            let base = if is_mod_rs {
                dir.to_path_buf()
            } else {
                dir.join(file.trim_end_matches(".rs"))
            };
            vec![
                base.join(format!("{name}.rs")),
                base.join(name).join("mod.rs"),
            ]
        }
    };
    let root = repo_root();
    for c in &candidates {
        let normalized = root.join(c);
        if normalized.is_file() {
            let canon = normalized.canonicalize().expect("実在するファイル");
            return canon
                .strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/");
        }
    }
    panic!(
        "{parent} の `mod {name};` が読むファイルが見つからない（候補 {candidates:?}）。\
         インラインの mod の中の宣言なら、この番犬の解決を足す"
    );
}

/// ファイル全体がテスト専用のもの（`#[cfg(test)] mod x;` で読むファイルと、その子孫）
fn test_only_files(files: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut frontier: Vec<String> = Vec::new();
    for rel in files {
        let raw = read(rel);
        let raw_lines: Vec<&str> = raw.lines().collect();
        let tests = code_view::code_view(&production_range::tests_only(&raw));
        for (k, l) in tests.lines().enumerate() {
            if let Some(name) = out_of_line_mod(l) {
                let child = resolve_mod(rel, name, path_attr(&raw_lines, k));
                if out.insert(child.clone()) {
                    frontier.push(child);
                }
            }
        }
    }
    // テスト専用のファイルが読む子は、cfg が付いていなくてもテスト専用
    while let Some(rel) = frontier.pop() {
        let raw = read(&rel);
        let raw_lines: Vec<&str> = raw.lines().collect();
        let code = code_view::code_view(&raw);
        for (k, l) in code.lines().enumerate() {
            if let Some(name) = out_of_line_mod(l) {
                let child = resolve_mod(&rel, name, path_attr(&raw_lines, k));
                if out.insert(child.clone()) {
                    frontier.push(child);
                }
            }
        }
    }
    out
}

// --------------------------------------------------------------- 判定

/// 1 ファイルの判定（本番の領域は [`Rule::Risky`]・テストの領域は [`Rule::FixedWindow`]）。
/// 返すのは `(テストの領域か, 判定)` を行の順に
fn file_sites(raw: &str, test_only: bool) -> Vec<(bool, Site)> {
    let code = code_view::code_view(raw);
    let regions = production_range::scan(raw).regions;
    let in_tests = |line: usize| test_only || regions.iter().any(|&(s, e)| s <= line && line <= e);
    let mut out: Vec<(bool, Site)> = sites(raw, &code, Rule::Risky)
        .into_iter()
        .filter(|s| !in_tests(s.line))
        .map(|s| (false, s))
        .chain(
            sites(raw, &code, Rule::FixedWindow)
                .into_iter()
                .filter(|s| in_tests(s.line))
                .map(|s| (true, s)),
        )
        .collect();
    out.sort_by_key(|(_, s)| s.line);
    out
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。違反は `file:line — 理由`
fn offenders(raw: &str, rel: &str, test_only: bool) -> Vec<String> {
    offenders_of(rel, &file_sites(raw, test_only))
}

/// 判定済みの 1 ファイルから違反の見出しを作る
fn offenders_of(rel: &str, sites: &[(bool, Site)]) -> Vec<String> {
    sites
        .iter()
        .filter(|(_, s)| s.verdict == Verdict::Offender)
        .map(|(in_tests, s)| {
            let how = if *in_tests {
                "テストの窓は端を外側へ丸める（終端は `s.ceil_char_boundary(at + N)`・始端は \
                 `s.floor_char_boundary(at.saturating_sub(N))`。`crates/*/tests/` なら \
                 `tests/common/byte_window.rs`）"
            } else {
                "表示の切り詰めは `tako_core::text::truncate_chars`（文字数）、分けるなら \
                 `floor_char_boundary` で丸めてから、外から来た位置は `s.get(a..b)` で受ける"
            };
            format!(
                "{rel}:{} — 範囲添字に理由も丸めも無い（{}: `{}`）。文字列の添字はバイト位置なので、\
                 多バイト文字の途中に当たると panic する（#1728 / #1746 / #2020 / #1790）。{how}。\
                 `Vec` / スライスなら宣言に型（`Vec<…>` / `&[…]`）を書く。ASCII と分かっている位置\
                 なら同じ行か 1 行上に `// {REASON_MARK} <理由>` を書く",
                s.line, s.shape, s.text
            )
        })
        .collect()
}

/// 製品ファイル全体の走査の結果（2 本のテストで共有する。debug ビルドで 1 回 3 秒ほどかかる）
struct Scanned {
    files: Vec<String>,
    test_only: BTreeSet<String>,
    /// ファイルごとの判定（`(テストの領域か, 判定)`）
    sites: Vec<(String, Vec<(bool, Site)>)>,
}

fn scanned() -> &'static Scanned {
    static SCANNED: OnceLock<Scanned> = OnceLock::new();
    SCANNED.get_or_init(|| {
        let files = product_sources();
        let test_only = test_only_files(&files);
        let sites = files
            .iter()
            .map(|rel| (rel.clone(), file_sites(&read(rel), test_only.contains(rel))))
            .collect();
        Scanned {
            files,
            test_only,
            sites,
        }
    })
}

// --------------------------------------------------------------- テスト

#[test]
fn 製品ファイルに理由も丸めも無いバイト位置の切り出しが無い() {
    let found: Vec<String> = scanned()
        .sites
        .iter()
        .flat_map(|(rel, sites)| offenders_of(rel, sites))
        .collect();
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと（列挙が壊れて 0 件・形の判定が死んで 0 件で緑になるのを防ぐ）
#[test]
fn 走査が空振りしていない() {
    let Scanned {
        files,
        test_only,
        sites,
    } = scanned();
    for must in [
        "crates/tako-app/src/main.rs",
        "crates/tako-control/src/dispatch.rs",
        "crates/tako-control/src/bin/tako-lsp-fake.rs",
        "crates/tako-core/src/text.rs",
        "crates/tako-cli/src/main.rs",
    ] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（crates/*/src/ の列挙が壊れている）"
        );
    }
    assert!(files.len() >= 400, "列挙が少なすぎる: {} 件", files.len());

    // `#[cfg(test)] mod tests;` / `#[path]` 付きの宣言 / その子孫を見分けている
    // （`#[cfg(any(test, feature = "visual-test"))]` もテストの領域 = production_range と同じ読み）
    for must in [
        "crates/tako-control/src/runtime_probe/tests.rs",
        "crates/tako-core/src/runtime_env/tests.rs",
        "crates/tako-app/src/ui_text_lang_race.rs",
    ] {
        assert!(
            test_only.contains(must),
            "{must} をテスト専用と見分けていない: {test_only:?}"
        );
    }
    for prod in [
        "crates/tako-app/src/main.rs",
        "crates/tako-control/src/lib.rs",
    ] {
        assert!(!test_only.contains(prod), "{prod} をテスト専用と読んだ");
    }

    let (mut risky_row, mut risky_reason, mut test_row, mut test_reason) = (0, 0, 0, 0);
    for (_, file) in sites {
        for (in_tests, s) in file {
            match (*in_tests, s.verdict) {
                (false, Verdict::RowWindow) => risky_row += 1,
                (false, Verdict::Reason) => risky_reason += 1,
                (true, Verdict::RowWindow) => test_row += 1,
                (true, Verdict::Reason) => test_reason += 1,
                _ => {}
            }
        }
    }
    // 2026-10-16 の実測（#1790 の仕分けの後）は本番の領域で行の窓 65 / 理由 51、
    // テストの領域で行の窓 30 / 理由 12。形か受け手の判定が死ぬとここが 0 に近づく
    assert!(
        risky_row >= 50,
        "本番の領域の行の窓が {risky_row} 件しか見えない（受け手の判定が壊れている）"
    );
    assert!(
        risky_reason >= 40,
        "本番の領域の理由コメントが {risky_reason} 件しか見えない（形の判定が壊れている）"
    );
    assert!(
        test_row >= 20 && test_reason >= 8,
        "テストの領域の判定が見えない（行の窓 {test_row} / 理由 {test_reason}）"
    );
}

#[test]
fn 修正前の形を実際に落とす() {
    // 本番の領域（`Rule::Risky`）。各 1 件を名指しすること
    let prod: &[(&str, &str, usize)] = &[
        (
            "① token の末尾 12 バイト（`.min(` の上限）",
            "fn a(token: &str) -> &str {\n    let tail = &token[..12.min(token.len())];\n    tail\n}\n",
            2,
        ),
        (
            "② 偽 LSP の %xx（前方の固定幅）",
            "fn b(rest: &str, i: usize) {\n    let _ = u8::from_str_radix(&rest[i + 1..i + 3], 16);\n}\n",
            2,
        ),
        (
            "③ 先頭 1 文字を飛ばす（数値の境界）",
            "fn c(text: &str) -> String {\n    text[1..].to_string()\n}\n",
            2,
        ),
        (
            "④ 末尾の 1 文字を落とす（`len()` から引く）",
            "fn d(tok: &str) -> bool {\n    tok[..tok.len() - 1].is_empty()\n}\n",
            2,
        ),
        (
            "⑤ 末尾から数える（saturating_sub）",
            "fn e(s: &str) -> &str {\n    &s[s.len().saturating_sub(20)..]\n}\n",
            2,
        ),
        (
            "⑥ let を遡る `.min(`",
            "fn f(s: &str) -> &str {\n    let end = s.len().min(120);\n    &s[..end]\n}\n",
            3,
        ),
    ];
    for (label, src, line) in prod {
        let found = offenders(src, "注入.rs", false);
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
    }

    // テストの領域（`Rule::FixedWindow`）: 10/16 の Issue コメントの 4 か所の修正前の形
    let tests: &[(&str, &str, usize)] = &[
        (
            "⑦ profiles.rs の前方 1200",
            "#[cfg(test)]\nmod tests {\n    fn t(src: &str) {\n        let fetch_start = src.find(\"x\").unwrap();\n        let fetch_body = &src[fetch_start..fetch_start + 1200];\n    }\n}\n",
            5,
        ),
        (
            "⑧ main.rs の先頭 400",
            "#[cfg(test)]\nmod tests {\n    fn t(after_set: &str) {\n        assert!(after_set[..400].contains(\"x\"));\n    }\n}\n",
            4,
        ),
        (
            "⑨ right_panel.rs の見つからないときの 8000",
            "#[cfg(test)]\nmod tests {\n    fn t(body: &str) {\n        let end = body.find(\"x\").unwrap_or(body.len().min(8000));\n        let _ = &body[..end];\n    }\n}\n",
            5,
        ),
    ];
    for (label, src, line) in tests {
        let found = offenders(src, "注入.rs", false);
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
        assert!(
            found[0].contains("ceil_char_boundary"),
            "テストの領域の直し方を案内していない: {found:?}"
        );
    }
    // テスト専用のファイルは全体がテストの領域（数値の境界は数えず、固定幅の窓は数える）
    let whole = "fn t(s: &str, hash: &str) {\n    let _ = &s[1..];\n    let _ = &hash[..7];\n}\n";
    let found = offenders(whole, "tests.rs", true);
    assert!(
        found.len() == 1 && found[0].starts_with("tests.rs:3 —"),
        "{found:?}"
    );

    // 安全な形・文字列ではない受け手は通す（常に落ちる無意味な番犬になっていないこと）
    let ok = "
fn ok(s: &str, v: &[u8], argv: Vec<String>, at: usize) {
    let lines: Vec<&str> = s.lines().collect();
    let a = &lines[lines.len() - 5..];
    let b = &v[124..136];
    let c = &argv[1..];
    let d = &s.as_bytes()[..3];
    let parts = vec![1, 2, 3];
    let e = &parts[2..];
    let f = s[..1.min(s.len())].len();
    let g = &s[..s.floor_char_boundary(120)];
    // 切り出し安全: 直前の starts_with('-') で先頭は ASCII
    let h = &s[1..];
    let i = &s[2..]; // 切り出し安全: 先頭 2 バイトは ASCII の `~/`
    let j = &s[at + 1..];
    let k = &s[0..at];
    let l = lines[..2].join(\",\");
    let m = \"&s[..12] は文字列の中なので数えない\";
    // &s[..12] は注釈の中なので数えない
}
";
    let found = offenders(ok, "注入.rs", false);
    assert_eq!(
        found.len(),
        1,
        "通すべき形まで落としたか、`.min(` の上限を見逃した: {found:?}"
    );
    assert!(
        found[0].starts_with("注入.rs:10 —"),
        "`[..1.min(s.len())]` だけを落とす: {found:?}"
    );

    // 理由コメントは 2 行以上離すと効かない（そばに置く規約）
    let far = "fn far(s: &str) {\n    // 切り出し安全: 離れた理由\n    let x = 1;\n    let k = &s[1..];\n}\n";
    let found = offenders(far, "注入.rs", false);
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:4 —")),
        "離れた理由コメントで通してしまった: {found:?}"
    );

    // 起点の添字（enumerate）で行の窓と読むのはテストの窓だけ。本番の文字列は落とす
    let bytes_pos = "fn p(s: &str) {\n    for (i, _) in s.bytes().enumerate() {\n        let _ = &s[i..i + 3];\n    }\n}\n";
    let found = offenders(bytes_pos, "注入.rs", false);
    assert!(
        found.len() == 1 && found[0].starts_with("注入.rs:3 —"),
        "{found:?}"
    );
}

/// 実際のファイルの 1 行を修正前へ戻した本文を、その `file:line` で名指しすること
#[test]
fn 直した実ファイルを直書きへ戻すと名指しする() {
    let test_only = &scanned().test_only;
    // (ファイル, 今の行に含まれる字面, 修正前の行)
    let cases: &[(&str, &str, &str)] = &[
        (
            "crates/tako-app/src/settings_window/profiles.rs",
            "let fetch_body = &src[fetch_start..src.ceil_char_boundary(fetch_start + 1200)];",
            "        let fetch_body = &src[fetch_start..fetch_start + 1200];",
        ),
        (
            "crates/tako-control/src/bin/tako-lsp-fake.rs",
            "u8::from_str_radix(hex, 16)",
            "            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {",
        ),
    ];
    for (rel, now, before) in cases {
        let raw = read(rel);
        assert!(
            offenders(&raw, rel, test_only.contains(*rel)).is_empty(),
            "{rel} は今の本文で緑のはず"
        );
        let lines: Vec<&str> = raw.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.contains(now))
            .unwrap_or_else(|| panic!("{rel} に `{now}` が無い（直した行が動いた?）"));
        let mut injected: Vec<&str> = lines.clone();
        injected[at] = before;
        let injected = injected.join("\n");
        let found = offenders(&injected, rel, test_only.contains(*rel));
        let want = format!("{rel}:{} —", at + 1);
        assert!(
            found.len() == 1 && found[0].starts_with(&want),
            "修正前の行へ戻した {rel} を `{want}` で名指ししていない: {found:?}"
        );
    }
}

/// #1790 で直したテストの窓の式は、端の多バイト文字で落ちない（修正前の式は panic する）
#[test]
fn テストの窓の直し方は端の多バイト文字で落ちない() {
    // 前方（profiles.rs の 1200 / main.rs の 400）: 「目」が 3..6 に跨がり、終端 4 は文字の途中
    let src = "abc目印";
    let at = 0usize;
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| src[at..at + 4].len());
    assert!(
        old.is_err(),
        "修正前の前方の式が panic しない（注入が効いていない）"
    );
    let window = &src[at..src.ceil_char_boundary(at + 4)];
    assert_eq!(window, "abc目", "終端は外側の文字境界へ広がる");
    // 幅が末尾を越えても文字列の端で止まる（修正前は範囲外で panic した）
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| src[at..at + 1200].len());
    assert!(old.is_err());
    assert_eq!(&src[at..src.ceil_char_boundary(at + 1200)], src);

    // 先頭（acceptance_gates.rs の 40 / right_panel.rs の見つからないときの 8000）
    let text = "あいう";
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[..4.min(text.len())].len());
    assert!(old.is_err(), "修正前の先頭の式が panic しない");
    assert_eq!(&text[..text.ceil_char_boundary(4)], "あい");
    assert_eq!(text.ceil_char_boundary(8000), text.len());

    // 固定の位置（diag.rs の混線した行）: 21..23 が多バイト文字の途中でも、バイトで比べれば
    // panic せずに不一致になって assert が名指しする
    let mixed = format!("[{}メッセージ", "x".repeat(20));
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| &mixed[21..23] == "] ");
    assert!(old.is_err(), "修正前の固定位置の式が panic しない");
    assert!(mixed.as_bytes()[21..23] != *b"] ");
    assert!(!mixed.as_bytes()[23..].starts_with(b"thread"));
}

/// 手で書いていた丸め（`while !is_char_boundary(end) { end -= 1 }` / `from += 1`）を
/// `floor_char_boundary` / `ceil_char_boundary` へ置き換えた 4 か所（preview.rs・lsp/server.rs・
/// runtime_env/tier_p.rs・text_edit/enter.rs）は、すべての位置で同じ値を返す（挙動を変えない）
#[test]
fn 手の丸めを置き換えた式は同じ位置を返す() {
    fn floor_by_hand(text: &str, max: usize) -> usize {
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        end
    }
    fn ceil_by_hand(text: &str, from: usize) -> usize {
        let mut from = from;
        while !text.is_char_boundary(from) {
            from += 1;
        }
        from
    }
    for text in ["", "abc", "あいう", "a目b😀c\r\n", "\u{301}e\u{302}"] {
        for i in 0..=text.len() + 4 {
            assert_eq!(
                text.floor_char_boundary(i),
                floor_by_hand(text, i),
                "{text:?} の {i}"
            );
            if i <= text.len() {
                assert_eq!(
                    text.ceil_char_boundary(i),
                    ceil_by_hand(text, i),
                    "{text:?} の {i}"
                );
            }
        }
    }
}

//! **#1666 の番犬**: テストが一時ディレクトリを**固定名**で取らない。
//!
//! ## なぜ止めるのか
//!
//! `std::env::temp_dir()` はマシン全体で 1 つで、別の worktree の worker が走らせる
//! `cargo test` も**同じテストを同じ名前で**使う。固定名の `temp_dir().join("x")` に
//! fixture を書いて末尾で `remove_dir_all` するテストは、並行する相手の
//! `remove_dir_all` で自分の fixture を消される（または `create_dir_all` と `write` が
//! 競合する）ので、負荷ではなく**並行実行そのもの**で落ちる。実物: 2026-09-23 に
//! `preview::tests::読み取りと編集で標準言語セットのシンタックスハイライトを使う` が
//! 別 worktree の `cargo test` と重なった 1 巡目だけ落ちた（#1666）。
//! 消さない側は名前が 1 つなので残骸の作り手も辿れない（#1296 / #1312）。
//!
//! 直し方は `tako_core::test_residue::ScratchDir::new("<目印>")`（tako-core の中では
//! `crate::test_residue::ScratchDir`）。呼ぶたびに別の dir になり、スコープを抜けると
//! （panic の巻き戻しでも）消える。
//!
//! ## 何を落とすか
//!
//! テストの領域（`crates/*/tests/` の全体と、`crates/*/src/` の `test` を正の位置に含む cfg の
//! 中・`#[cfg(test)] mod x;` で読むファイル = `production_range` と同じ読み）で:
//!
//! 1. **固定のリテラル**: `env::temp_dir().join("x")` / `.join(r"x")`（改行をまたいでも同じ）
//! 2. **囲んでいる関数の引数そのもの**: `fn write_test_pdf(dir_name: &str, …)` の中の
//!    `env::temp_dir().join(dir_name)`。一意かどうかが呼び手しだいで、呼び手は
//!    固定名を渡しがち（#1666 で `preview.rs` の 5 本がこの形だった）
//!
//! 通すもの: 同じ行か 1 行上に `// 固定名安全: <理由>`（理由は空にしない）。
//! **作らない・消さない**パス（OSC 7 の本文に載せるだけの文字列など）か、呼び手が
//! pid 入りの名前を渡すと分かっている補助だけに使う。
//!
//! 見ないもの: 本番コード（置き場の決め方は製品の判断で、テストの並行とは別の話）、
//! `format!` で組んだ名前（pid / 時刻を入れる形。残骸の側は #1312 の番犬が受け持つ）、
//! 定数や `let` で束ねた固定名を経由する形（既知の穴。#1666 の時点で実在しない）。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙・テスト専用ファイルの見分け・呼び出しの検出・目印の
//! 通過が実ファイルで生きていることを、[`修正前の形を実際に落とす`] で #1666 で直した形が
//! `file:line` で名指しされ、通すべき形が通ることを、[`直した実ファイルを固定名へ戻すと名指しする`]
//! で**実際のファイルの 1 行を修正前へ戻した本文**が落ちることを確かめる（注入は
//! `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// 本番コードとテスト領域の境界（ファイル単位を含む）は 1 実装（#1420 / #1445 / #1790）
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/tests/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn test_sources() -> Vec<String> {
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
        let tests = entry.expect("crate の項目").path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
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

// --------------------------------------------------------------- 語彙

/// 通す目印（同じ行か 1 行上。`:` の後に理由が要る）
const MARK: &str = "固定名安全:";

/// 一時ディレクトリの入口（`std::env::temp_dir()` / `env::temp_dir()`）
const CALL: &str = "env::temp_dir()";

/// 一意な置き場の補助（寄せた先が消えたら気づくために数える）
const HELPERS: [&str; 2] = ["ScratchDir::new(", "process_scratch("];

// --------------------------------------------------------------- 字句の部品

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 空白（改行を含む）を飛ばした位置
fn skip_ws(s: &str, mut at: usize) -> usize {
    while s.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// `s[at..]` が文字列リテラル（`"…"` / `r"…"` / `r#"…"#`）で始まるなら中身
fn string_literal(s: &str, at: usize) -> Option<String> {
    let rest = &s[at..];
    let (body, close) = if let Some(body) = rest.strip_prefix('"') {
        (body, "\"".to_string())
    } else if let Some(after_r) = rest.strip_prefix('r') {
        let hashes = after_r.len() - after_r.trim_start_matches('#').len();
        let body = after_r[hashes..].strip_prefix('"')?;
        (body, format!("\"{}", "#".repeat(hashes)))
    } else {
        return None;
    };
    let end = body.find(&close).unwrap_or(body.len());
    Some(body[..end].to_string())
}

/// `s[at..]` が識別子（`&` / `&mut ` を前置してよい）だけで `)` に閉じるなら、その識別子
fn bare_ident_arg(s: &str, at: usize) -> Option<&str> {
    let mut rest = &s[at..];
    rest = rest.strip_prefix('&').unwrap_or(rest);
    rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let end = rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len());
    let ident = &rest[..end];
    let first_ok = ident.chars().next().is_some_and(|c| !c.is_ascii_digit());
    let closes = rest[end..].trim_start().starts_with(')');
    (first_ok && closes).then_some(ident)
}

/// 行の頭のバイト位置（1 ファイルで 1 回だけ数える）
fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// バイト位置 → 0 始まりの行
fn line_index(starts: &[usize], at: usize) -> usize {
    starts.partition_point(|&s| s <= at) - 1
}

/// 同じ行か 1 行上に、理由つきの目印がある
fn marked(raw: &[&str], i: usize) -> bool {
    let has = |line: &str| {
        line.split_once(MARK)
            .is_some_and(|(_, reason)| !reason.trim().is_empty())
    };
    raw.get(i).is_some_and(|l| has(l)) || (i > 0 && has(raw[i - 1]))
}

/// `at` を囲んでいる（直前に頭がある）関数の引数名。頭は `code`（コメントと文字列を
/// 潰した眺め）の行で探すので、文字列の中の `fn ` を拾わない
fn enclosing_params(code: &str, starts: &[usize], at: usize) -> BTreeSet<String> {
    let mut i = line_index(starts, at);
    loop {
        let line_end = starts.get(i + 1).map_or(code.len(), |&e| e);
        let line = &code[starts[i]..line_end];
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            let head = starts[i] + line.find(name).unwrap_or(0) + name.len();
            return params_after(code, head);
        }
        if i == 0 {
            return BTreeSet::new();
        }
        i -= 1;
    }
}

/// 括弧の深さの増減（`->` の `>` は閉じではない）
fn nesting(bytes: &[u8], k: usize) -> i32 {
    match bytes[k] {
        b'(' | b'[' | b'{' | b'<' => 1,
        b'>' if k > 0 && bytes[k - 1] == b'-' => 0,
        b')' | b']' | b'}' | b'>' => -1,
        _ => 0,
    }
}

/// 関数名の直後から引数リストを読む（対応する `)` まで。各引数の `名前:` を拾う）
fn params_after(code: &str, from: usize) -> BTreeSet<String> {
    let bytes = code.as_bytes();
    let Some(open) = code[from..].find('(').map(|k| from + k) else {
        return BTreeSet::new();
    };
    let mut depth = 0i32;
    let mut close = open;
    for k in open..bytes.len() {
        depth += nesting(bytes, k);
        if depth == 0 {
            close = k;
            break;
        }
    }
    let mut out = BTreeSet::new();
    let mut depth = 0i32;
    let mut start = open + 1;
    for (k, &b) in bytes.iter().enumerate().take(close + 1).skip(open + 1) {
        if k < close {
            depth += nesting(bytes, k);
        }
        if (b == b',' && depth == 0) || k == close {
            let param = code[start..k].trim();
            let param = param.strip_prefix("mut ").unwrap_or(param);
            if let Some((name, _)) = param.split_once(':') {
                let name = name.trim();
                if !name.is_empty() && name.chars().all(is_ident) {
                    out.insert(name.to_string());
                }
            }
            start = k + 1;
        }
    }
    out
}

// --------------------------------------------------------------- 判定

/// 違反 1 件（行は 1 始まり）
#[derive(Clone, Debug)]
struct Offender {
    line: usize,
    /// 固定のリテラル（`Some("tako_x")`）か、引数の名前（`None` のとき `param`）
    literal: Option<String>,
    param: Option<String>,
}

/// 1 ファイルぶんの走査の結果
#[derive(Default)]
struct FileScan {
    offenders: Vec<Offender>,
    /// テストの領域にある `env::temp_dir()` の呼び出し
    calls: usize,
    /// そのうち `.join(` が続くもの
    joins: usize,
    /// 目印で通したもの
    marked: usize,
    /// 一意な置き場の補助の呼び出し（コメントと文字列は数えない）
    helper_uses: usize,
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。
/// `test_only` = ファイル全体がテスト（`crates/*/tests/` と `#[cfg(test)] mod x;` で読むファイル）
fn scan_source(src: &str, test_only: bool) -> FileScan {
    let region = if test_only {
        src.to_string()
    } else {
        production_range::tests_only(src)
    };
    // 呼び出しは「コードだけの眺め」で探し（コメント・文字列の中の綴りを拾わない）、
    // 引数がリテラルかは「コメントだけ潰した眺め」で読む（#1609 の型。どちらもバイト長が同じ）
    let code = code_view::code_view(&region);
    let kept = code_view::without_comments(&region);
    let raw: Vec<&str> = src.lines().collect();
    let starts = line_starts(src);
    let mut scan = FileScan {
        helper_uses: HELPERS.iter().map(|h| code.matches(h).count()).sum(),
        ..FileScan::default()
    };
    for (at, _) in code.match_indices(CALL) {
        // `xenv::temp_dir()` のような別の識別子の尾は数えない
        if code[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        scan.calls += 1;
        let dot = skip_ws(&kept, at + CALL.len());
        if !kept[dot..].starts_with(".join(") {
            continue;
        }
        scan.joins += 1;
        let arg = skip_ws(&kept, dot + ".join(".len());
        let literal = string_literal(&kept, arg);
        let param = if literal.is_none() {
            bare_ident_arg(&kept, arg)
                .filter(|ident| enclosing_params(&code, &starts, at).contains(*ident))
                .map(str::to_string)
        } else {
            None
        };
        if literal.is_none() && param.is_none() {
            continue;
        }
        let line = line_index(&starts, at);
        let arg_line = line_index(&starts, arg);
        if marked(&raw, line) || marked(&raw, arg_line) {
            scan.marked += 1;
            continue;
        }
        scan.offenders.push(Offender {
            line: line + 1,
            literal,
            param,
        });
    }
    scan
}

fn offenders(src: &str, rel: &str, test_only: bool) -> Vec<String> {
    offenders_of(rel, &scan_source(src, test_only))
}

fn offenders_of(rel: &str, scan: &FileScan) -> Vec<String> {
    scan.offenders
        .iter()
        .map(|o| {
            let what = match (&o.literal, &o.param) {
                (Some(lit), _) => format!("固定（\"{lit}\"）"),
                (None, Some(p)) => {
                    format!("引数 `{p}` のまま（一意かどうかが呼び手しだい）")
                }
                (None, None) => unreachable!("違反は名前の出どころを持つ"),
            };
            format!(
                "{rel}:{} — テストの一時ディレクトリの名前が{what}。別の worktree の cargo test も同じ名前を\
                 使うので、一方の remove_dir_all が他方の fixture を消す（#1666）。\
                 `tako_core::test_residue::ScratchDir::new(\"<目印>\")`（tako-core の中は `crate::`）で\
                 一意な置き場を取る（スコープを抜けると消える）。作らない・消さないパスなら同じ行か \
                 1 行上に `// {MARK} <理由>` を書く",
                o.line
            )
        })
        .collect()
}

/// テストの領域全体の走査の結果（3 本のテストで共有する）
struct Scanned {
    files: Vec<String>,
    test_only: BTreeSet<String>,
    scans: Vec<(String, FileScan)>,
}

fn scanned() -> &'static Scanned {
    static SCANNED: OnceLock<Scanned> = OnceLock::new();
    SCANNED.get_or_init(|| {
        let product = production_range::product_sources();
        let test_only = production_range::test_only_files(&product);
        let tests = test_sources();
        let scans = product
            .iter()
            .map(|rel| (rel.clone(), test_only.contains(rel)))
            .chain(tests.iter().map(|rel| (rel.clone(), true)))
            .map(|(rel, whole)| {
                let scan = scan_source(&read(&rel), whole);
                (rel, scan)
            })
            .collect();
        let mut files = product;
        files.extend(tests);
        Scanned {
            files,
            test_only,
            scans,
        }
    })
}

// --------------------------------------------------------------- テスト

#[test]
fn テストが一時ディレクトリを固定名で取っていない() {
    let found: Vec<String> = scanned()
        .scans
        .iter()
        .flat_map(|(rel, scan)| offenders_of(rel, scan))
        .collect();
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと（列挙が壊れて 0 件・検出が死んで 0 件で緑になるのを防ぐ）
#[test]
fn 走査が空振りしていない() {
    let Scanned {
        files,
        test_only,
        scans,
    } = scanned();
    for must in [
        "crates/tako-app/src/preview.rs",
        "crates/tako-app/src/platform/pdf/windows.rs",
        "crates/tako-control/src/dispatch.rs",
        "crates/tako-core/src/tab.rs",
        "crates/tako-core/tests/shell_integration_powershell.rs",
    ] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（crates/*/src・crates/*/tests の列挙が壊れている）"
        );
    }
    // `#[cfg(test)] mod tests;` で読むファイルを丸ごとテストとして見ている
    assert!(
        test_only.contains("crates/tako-control/src/runtime_probe/tests.rs"),
        "テスト専用ファイルの見分けが壊れている: {test_only:?}"
    );
    let total = |f: fn(&FileScan) -> usize| scans.iter().map(|(_, s)| f(s)).sum::<usize>();
    let (calls, joins, marked, helper_uses) = (
        total(|s| s.calls),
        total(|s| s.joins),
        total(|s| s.marked),
        total(|s| s.helper_uses),
    );
    eprintln!(
        "走査: {} ファイル（丸ごとテスト {} 件）・テスト領域の temp_dir() {calls} 件・うち join \
         {joins} 件・目印で通した {marked} 件・一意な置き場の補助の利用 {helper_uses} 件",
        files.len(),
        test_only.len()
    );
    // テストの領域の呼び出しを拾えている（#1666 の時点の実測は 393 件・うち join 286 件）
    assert!(calls >= 300, "temp_dir() の検出が死んでいる: {calls} 件");
    assert!(joins >= 200, ".join( の検出が死んでいる: {joins} 件");
    // 目印で通した形（PowerShell の OSC 7 の本文・pid 入りの名前を渡される補助）を拾えている
    assert!(marked >= 2, "目印の通過が働いていない: {marked} 件");
    // 寄せた先が実際に使われている（コメントと文字列は数えない）
    assert!(
        helper_uses >= 100,
        "一意な置き場の補助の利用が少なすぎる: {helper_uses} 件"
    );
}

/// #1666 で直した修正前の形が `file:line` で名指しされ、通すべき形は通ること
#[test]
fn 修正前の形を実際に落とす() {
    let tests = |body: &str| format!("#[cfg(test)]\nmod tests {{\n{body}}}\n");
    // (名前, 本文, 名指しされる行)
    let bad: [(&str, String, usize); 7] = [
        (
            "preview.rs の修正前（固定名に fixture を書いて消す）",
            tests("    fn t() {\n        let scratchpad = std::env::temp_dir().join(\"tako_editor_highlight_test\");\n        std::fs::remove_dir_all(&scratchpad).ok();\n    }\n"),
            4,
        ),
        (
            "use してから env::temp_dir()",
            tests("    use std::env;\n    fn t() {\n        let tmp = env::temp_dir().join(\"tako_test_prune_171\");\n    }\n"),
            5,
        ),
        (
            "改行をまたぐ .join(",
            tests("    fn t() {\n        let dir = std::env::temp_dir()\n            .join(\"tako-1069-scan-test\");\n    }\n"),
            4,
        ),
        (
            "生文字列",
            tests("    fn t() {\n        let dir = std::env::temp_dir().join(r\"tako_pdf_win_size_test\");\n    }\n"),
            4,
        ),
        (
            "名前を引数で受ける補助（preview.rs の write_test_pdf の修正前）",
            tests("    fn write_test_pdf(dir_name: &str, file: &str, contents: &[&str]) -> PathBuf {\n        let dir = std::env::temp_dir().join(dir_name);\n        dir.join(file)\n    }\n"),
            4,
        ),
        (
            "理由の無い目印は通さない",
            tests("    fn t() {\n        // 固定名安全:\n        let dir = std::env::temp_dir().join(\"x\");\n    }\n"),
            5,
        ),
        (
            "crates/*/tests の直書き（ファイル全体がテスト）",
            "fn t() {\n    let dir = std::env::temp_dir().join(\"tako-test-profiles\");\n}\n".to_string(),
            2,
        ),
    ];
    for (name, src, line) in &bad {
        let whole = !src.starts_with("#[cfg(test)]");
        let found = offenders(src, "x.rs", whole);
        assert!(
            found.len() == 1 && found[0].starts_with(&format!("x.rs:{line} ")),
            "{name}: x.rs:{line} を名指ししない: {found:#?}"
        );
    }

    // 本番コードの領域は見ない（テストの領域の外）
    assert!(offenders(
        "fn f() {\n    let dir = std::env::temp_dir().join(\"tako-update\");\n}\n",
        "x.rs",
        false
    )
    .is_empty());

    let ok: [(&str, String); 7] = [
        (
            "一意な置き場",
            tests("    fn t() {\n        let dir = tako_core::test_residue::ScratchDir::new(\"editor-highlight\");\n    }\n"),
        ),
        (
            "pid 入りの format!",
            tests("    fn t() {\n        let dir = std::env::temp_dir().join(format!(\"tako-x-{}\", std::process::id()));\n    }\n"),
        ),
        (
            "引数ではない束ね（呼び手しだいではない）",
            tests("    fn t() {\n        let leaf = format!(\"t766{}\", std::process::id() % 1000);\n        let target = std::env::temp_dir().join(&leaf);\n    }\n"),
        ),
        (
            "理由つきの目印（同じ行の上）",
            tests("    fn t() {\n        // 固定名安全: 作らない・消さない（OSC 7 の本文に載せるだけ）\n        let foreign = std::env::temp_dir().join(\"tako-1199-foreign\");\n    }\n"),
        ),
        (
            "引数を受ける補助でも、目印つき",
            tests("    fn cwd_tracking(program: &str, dir_name: &str) {\n        // 固定名安全: 呼び手は pid 入りの名前を渡す\n        let target = std::env::temp_dir().join(dir_name);\n    }\n"),
        ),
        (
            "join しない temp_dir()",
            tests("    fn t() {\n        let c = Cfg { cwd: Some(std::env::temp_dir()) };\n        assert!(dir.starts_with(std::env::temp_dir()));\n    }\n"),
        ),
        (
            "コメントと文字列の中は見ない",
            tests("    fn t() {\n        // let dir = std::env::temp_dir().join(\"x\");\n        let s = \"std::env::temp_dir().join(\\\"x\\\")\";\n    }\n"),
        ),
    ];
    for (name, src) in &ok {
        let found = offenders(src, "x.rs", false);
        assert!(
            found.is_empty(),
            "{name}: 通すべき形を名指しした: {found:#?}"
        );
    }
}

/// 実際のファイルの直した 1 行を修正前へ戻した本文が、その行で名指しされること
#[test]
fn 直した実ファイルを固定名へ戻すと名指しする() {
    // (ファイル, 今の行, 修正前の行)。今の行はファイルの中で最初に現れるものを戻す
    let reverts = [
        (
            "crates/tako-app/src/preview.rs",
            "let scratchpad = tako_core::test_residue::ScratchDir::new(\"editor-highlight-1666\");",
            "let scratchpad = std::env::temp_dir().join(\"tako_editor_highlight_test\");",
        ),
        (
            "crates/tako-app/src/preview.rs",
            "let dir = tako_core::test_residue::ScratchDir::new(&format!(\"pdf-{tag}\"));",
            "let dir = std::env::temp_dir().join(tag);",
        ),
        (
            "crates/tako-app/src/platform/pdf/windows.rs",
            "let dir = tako_core::test_residue::ScratchDir::new(\"pdf-win-size\");",
            "let dir = std::env::temp_dir().join(\"tako_pdf_win_size_test\");",
        ),
        (
            "crates/tako-core/src/tab.rs",
            "let scratch = crate::test_residue::ScratchDir::new(\"prune-tab-171\");",
            "let scratch = std::env::temp_dir().join(\"tako_test_prune_tab_171\");",
        ),
        (
            "crates/tako-control/src/dispatch.rs",
            "let scratch = tako_core::test_residue::ScratchDir::new(\"dispatch-prune-171\");",
            "let scratch = std::env::temp_dir().join(\"tako_test_prune_171\");",
        ),
        (
            // 目印を外すと名指しされる（目印が実際に効いている）
            "crates/tako-core/tests/shell_integration_powershell.rs",
            "// 固定名安全: 作らない・消さない（別のシェルの OSC 7 の本文に載せる文字列だけ）",
            "//",
        ),
    ];
    for (rel, now, before) in reverts {
        let src = read(rel);
        let at = src
            .find(now)
            .unwrap_or_else(|| panic!("{rel} に今の行 `{now}` が無い（直した形が変わった）"));
        let whole = rel.contains("/tests/");
        assert!(
            offenders(&src, rel, whole).is_empty(),
            "{rel} は今の形で緑のはず"
        );
        let reverted = format!("{}{before}{}", &src[..at], &src[at + now.len()..]);
        // 目印を外す注入は、目印の次の行（呼び出しのある行）が名指しされる
        let line = src[..at].matches('\n').count() + 1 + usize::from(before == "//");
        let found = offenders(&reverted, rel, whole);
        assert!(
            found
                .iter()
                .any(|f| f.starts_with(&format!("{rel}:{line} "))),
            "{rel}:{line} を修正前（`{before}`）へ戻しても名指ししない: {found:#?}"
        );
    }
}

//! **#2248 の番犬**: 自分のテストバイナリを子として起こすテストは、子が出した目印を
//! **行頭一致で読まない**。
//!
//! ## なぜ止めるのか
//!
//! 親が `current_exe()` を `--exact <子のテスト> --nocapture` で起こし、子が `println!` した
//! 目印の行を読む形は、子の libtest が直列（`--test-threads=1`、または親から受け継いだ env
//! `RUST_TEST_THREADS=1`）のとき `test <名前> ... ` を**改行せずに**出すので、子の 1 行目が
//! その後ろへつながる。行頭一致（`strip_prefix("目印")` / `starts_with("目印")`）は外れ、
//! CI（env なし = 並列）では緑のまま、`RUST_TEST_THREADS=1` を渡した回だけ落ちる
//! （#2248: issue2035 / issue2048 の 2 本。`ipc.rs` / `issue2151` / `test_write_isolation.rs` は
//! 既に行の途中から拾っていた）。
//!
//! ## 何を固定するか
//!
//! `crates/*/src` と `crates/*/tests` の `.rs` のうち、`current_exe()` と `"--exact"` の両方を
//! 含む（= 自分のテストバイナリを子で起こす）ファイルで、
//!
//! 1. `"--exact"` の次に渡した**子のテストの名前**（文字列リテラルか、同じファイルの
//!    `const 名前: &str = "…";`。`mod::path` は最後の段）から、その関数の本体を引き
//! 2. 本体の `print!` / `println!` の書式文字列の頭（最初の `{` まで）を目印とみなし
//! 3. その頭に当たる文字列リテラルを `.strip_prefix("…")` / `.starts_with("…")` で読んでいる行を
//!    `file:line` で名指す
//!
//! 子の本体に限るのは、同じファイルの本番コードが出す行（`println!("  [{}] …")` 等）と、
//! テストが関数の戻り値を読む行（`starts_with("  [")`）を取り違えないため（#2248 の初版が
//! `tako-cli/src/main.rs` で踏んだ誤検知）。
//!
//! 直し方: 目印は `split_once("目印")` で行の途中から拾い、子は `--test-threads=1` で起こして
//! 出力の形を親の env に依らず 1 つに決める（直した読み方が毎回の CI で試される）。
//! 通すもの: 同じ行か 1 行上に `// 行頭一致安全: <理由>`。
//!
//! 既知の穴: 子の名前を引数や変数で渡す形（`tako-cli` の `issue1958_both_caller_arms(test)`）と、
//! 目印を定数で書く形（`println!("{MARKER} …")` と `strip_prefix(MARKER)`）は見ない。
//! `eprint!` / `eprintln!`（標準エラー）の目印は数えない（libtest の `test <名前> ... ` は
//! 標準出力なので、標準エラーの行頭は崩れない）。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙・子の名前の解決（リテラル / 定数）・目印の読み取りが
//! 実ファイルで生きていることを、[`修正前の形を名指しし直した形は通す`] で #2248 の 2 本の
//! 修正前の形が名指しされ、通すべき形が通ることを、[`直した実ファイルを修正前へ戻すと名指しする`]
//! で**実際のファイルの読み方を修正前へ戻した本文**が落ちることを確かめる（注入は `cargo test` の
//! 中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

/// 通す目印（同じ行か 1 行上）
const MARK: &str = "行頭一致安全:";

/// この番犬自身（修正前の形を例として持つので走査から外す）
const SELF_REL: &str = "crates/tako-control/tests/issue2248_child_marker_watchdog.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/src` と `crates/*/tests` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn sources() -> Vec<String> {
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
        let dir = entry.expect("crate の項目").path();
        for sub in ["src", "tests"] {
            if dir.join(sub).is_dir() {
                walk(&dir.join(sub), &mut files);
            }
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

/// 自分のテストバイナリを子として起こすファイルか
fn reinvokes_self(src: &str) -> bool {
    src.contains("current_exe()") && src.contains("\"--exact\"")
}

/// 開きの `"` の直後（`start`）から閉じの `"` までの中身（`\"` は飛ばす）
fn literal_at(src: &str, start: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(&src[start..i]),
            _ => i += 1,
        }
    }
    None
}

/// `src[at..]` の空白を飛ばした先が文字列リテラルならその中身
fn literal_after(src: &str, at: usize) -> Option<&str> {
    let rest = &src[at..];
    let trimmed = rest.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    literal_at(src, at + (rest.len() - trimmed.len()) + 1)
}

/// `"--exact"` の次に渡した子のテストの名前（リテラルか同じファイルの `&str` 定数。`mod::path` は最後の段）
fn child_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("\"--exact\"") {
        from += at + "\"--exact\"".len();
        let rest = src[from..].trim_start();
        let Some(rest) = rest.strip_prefix(',') else {
            continue;
        };
        let arg_at = src.len() - rest.len();
        let name = match literal_after(src, arg_at) {
            Some(lit) => Some(lit.to_string()),
            None => {
                let ident: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let decl = format!("const {ident}: &str =");
                if ident.is_empty() {
                    None
                } else {
                    src.find(&decl)
                        .and_then(|d| literal_after(src, d + decl.len()))
                        .map(str::to_string)
                }
            }
        };
        if let Some(name) = name {
            let last = name.rsplit("::").next().unwrap_or_default().to_string();
            if !last.is_empty() && !names.contains(&last) {
                names.push(last);
            }
        }
    }
    names
}

/// `fn <name>(` の行から、同じ字下げの閉じ括弧の行までの本体（rustfmt の形を前提にする）
fn fn_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let head = format!("fn {name}(");
    let start = src.find(&head)?;
    let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = src[line_start..]
        .chars()
        .take_while(|c| *c == ' ')
        .collect();
    let close = format!("\n{indent}}}");
    let end = src[start..].find(&close)? + start;
    Some(&src[start..end])
}

/// `print!` / `println!` の書式文字列の頭（最初の `{` まで。空は捨てる）。
/// `eprint!` / `eprintln!` は直前が識別子の文字なので数えない
fn printed_heads(src: &str) -> Vec<String> {
    let mut heads = Vec::new();
    for pat in ["println!(", "print!("] {
        let mut from = 0;
        while let Some(at) = src[from..].find(pat) {
            let pos = from + at;
            from = pos + pat.len();
            if src[..pos]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            if let Some(lit) = literal_after(src, from) {
                let head = lit.split('{').next().unwrap_or_default();
                if !head.is_empty() {
                    heads.push(head.to_string());
                }
            }
        }
    }
    heads
}

/// 子のテスト（`--exact` で名指ししたもの）が標準出力へ出す目印の頭
fn child_heads(src: &str) -> Vec<String> {
    child_names(src)
        .iter()
        .filter_map(|name| fn_body(src, name))
        .flat_map(printed_heads)
        .collect()
}

/// 子の目印を行頭一致で読んでいる行（1 始まりの行番号と、読んでいる文字列）
fn line_start_reads(src: &str, heads: &[String]) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//")
            || line.contains(MARK)
            || (i > 0 && lines[i - 1].contains(MARK))
        {
            continue;
        }
        for pat in [".strip_prefix(\"", ".starts_with(\""] {
            let mut from = 0;
            while let Some(at) = line[from..].find(pat) {
                let open = from + at + pat.len();
                from = open;
                let Some(lit) = literal_at(line, open) else {
                    continue;
                };
                if !lit.is_empty() && heads.iter().any(|h| h.starts_with(lit)) {
                    out.push((i + 1, lit.to_string()));
                }
            }
        }
    }
    out
}

/// 1 ファイルの違反（`file:line — 理由`）
fn violations(rel: &str, src: &str) -> Vec<String> {
    if !reinvokes_self(src) {
        return Vec::new();
    }
    let heads = child_heads(src);
    line_start_reads(src, &heads)
        .into_iter()
        .map(|(line, lit)| {
            format!(
                "{rel}:{line} — 子のテストバイナリが出した目印 `{lit}` を行頭一致で読んでいる。\
                 直列の libtest（`--test-threads=1` / 受け継いだ `RUST_TEST_THREADS=1`）は \
                 `test <名前> ... ` の後ろへ改行なしで続けるので外れる。`split_once(\"{lit}\")` で\
                 行の途中から拾い、子を `--test-threads=1` で起こす（#2248）。\
                 本当に行頭に来るなら同じ行か 1 行上に `// {MARK} <理由>`"
            )
        })
        .collect()
}

#[test]
fn 子の出力の目印を行頭一致で読むテストが無い() {
    let mut found = Vec::new();
    for rel in sources() {
        if rel == SELF_REL {
            continue;
        }
        found.extend(violations(&rel, &read(&rel)));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 列挙・子を起こすファイルの見分け・子の名前の解決・目印の読み取りが実ファイルで生きている
#[test]
fn 走査が空振りしていない() {
    let all = sources();
    assert!(
        all.iter().any(|r| r == SELF_REL),
        "列挙に tests/ が入っていない"
    );
    assert!(
        all.iter()
            .any(|r| r == "crates/tako-control/src/orchestrator/registry.rs"),
        "列挙に src/ が入っていない"
    );
    let reinvoking: Vec<&String> = all
        .iter()
        .filter(|r| *r != SELF_REL && reinvokes_self(&read(r)))
        .collect();
    // #2248 の棚卸しで 18 ファイル（増減はするので下限だけ見る）
    assert!(
        reinvoking.len() >= 10,
        "子を起こすファイルが少なすぎる（見分けが壊れた？）: {reinvoking:?}"
    );
    for (rel, head) in [
        // 子の名前が `"--exact",` の次の行にある形
        (
            "crates/tako-control/tests/issue2035_lsp_init_options.rs",
            "TAKO_2035_PROBE sent=",
        ),
        // 書式文字列が `println!(` の次の行にある形
        (
            "crates/tako-control/tests/issue2048_lsp_isolated_autostart.rs",
            "TAKO_2048_PROBE autostart=",
        ),
    ] {
        assert!(
            reinvoking.iter().any(|r| *r == rel),
            "{rel} を子を起こすファイルと見ていない"
        );
        let heads = child_heads(&read(rel));
        assert!(
            heads.iter().any(|h| h == head),
            "{rel} の子の目印 `{head}` を拾えていない: {heads:?}"
        );
    }
    // 子の名前を定数で渡す形（`=` の次の行へ折り返した定数 / `mod::path` つき）も本体まで引ける
    for (rel, name) in [
        (
            "crates/tako-control/src/test_write_isolation.rs",
            "子プロセス_本番相当の書き込みを一通り行う",
        ),
        (
            "crates/tako-app/src/update_checker.rs",
            "子プロセス_配布系統を判別する",
        ),
    ] {
        let src = read(rel);
        assert!(
            child_names(&src).iter().any(|n| n == name),
            "{rel} の子の名前 `{name}` を定数から引けていない: {:?}",
            child_names(&src)
        );
        assert!(
            fn_body(&src, name).is_some(),
            "{rel} の `{name}` の本体を引けない"
        );
    }
}

#[test]
fn 修正前の形を名指しし直した形は通す() {
    let parent = |read_line: &str| {
        format!(
            "fn probe() {{\n\
             \x20   let mut command = Command::new(std::env::current_exe().unwrap());\n\
             \x20   command.args([\"--exact\", \"子\", \"--nocapture\"]);\n\
             \x20   let stdout = String::new();\n\
             \x20   let line = stdout\n\
             \x20       .lines()\n\
             {read_line}\n\
             }}\n\
             fn 子() {{\n\
             \x20   println!(\"TAKO_2035_PROBE sent={{sent}}\");\n\
             \x20   println!(\n\
             \x20       \"TAKO_2048_PROBE autostart={{}} fetch={{}}\",\n\
             \x20       a, b\n\
             \x20   );\n\
             \x20   eprintln!(\"ERR_MARK {{x}}\");\n\
             }}\n\
             fn 表示(a: u32) {{\n\
             \x20   println!(\"  [{{a}}] 子ではない本番の出力\");\n\
             }}\n"
        )
    };
    let named = |read_line: &str| violations("x.rs", &parent(read_line));

    // #2248 の修正前の 2 本（7 行目）
    let old_2035 = named("        .find_map(|l| l.strip_prefix(\"TAKO_2035_PROBE sent=\"))");
    assert_eq!(old_2035.len(), 1, "{old_2035:?}");
    assert!(old_2035[0].starts_with("x.rs:7 — "), "{old_2035:?}");
    let old_2048 = named("        .find(|l| l.starts_with(\"TAKO_2048_PROBE \"))");
    assert_eq!(old_2048.len(), 1, "{old_2048:?}");
    assert!(old_2048[0].contains("`TAKO_2048_PROBE `"), "{old_2048:?}");

    // 子の名前を定数で渡しても同じ
    let via_const = format!(
        "const CHILD: &str =\n    \"tests::子\";\n{}",
        parent("        .find_map(|l| l.strip_prefix(\"TAKO_2035_PROBE sent=\"))")
            .replace("\"--exact\", \"子\"", "\"--exact\", CHILD")
    );
    assert_eq!(violations("x.rs", &via_const).len(), 1);

    // 直した形（行の途中から拾う）
    assert!(named(
        "        .find_map(|l| l.split_once(\"TAKO_2035_PROBE sent=\").map(|(_, rest)| rest))"
    )
    .is_empty());
    // 目印で通す
    assert!(named(&format!(
        "        // {MARK} 子は print! の前に何も出さない\n\
         \x20       .find_map(|l| l.strip_prefix(\"TAKO_2035_PROBE sent=\"))"
    ))
    .is_empty());
    // 標準エラーの目印・子の出力でない文字列・子でない関数の出力は見ない
    // （最後は初版が `tako-cli/src/main.rs` の `take_while(|l| !l.starts_with("  ["))` で踏んだ誤検知）
    assert!(named("        .find(|l| l.starts_with(\"ERR_MARK\"))").is_empty());
    assert!(named("        .filter(|l| l.starts_with(\"test result:\"))").is_empty());
    assert!(named("        .take_while(|l| !l.starts_with(\"  [\"))").is_empty());

    // 子を起こさないファイル（画面の読み取り等）は同じ読み方でも見ない
    let screen = parent("        .find_map(|l| l.strip_prefix(\"TAKO_2035_PROBE sent=\"))")
        .replace("\"--exact\", ", "");
    assert!(violations("x.rs", &screen).is_empty());
}

/// 実際のファイルの読み方を修正前へ戻した本文を名指しする（リポジトリのファイルは触らない）
#[test]
fn 直した実ファイルを修正前へ戻すと名指しする() {
    for (rel, fixed, legacy) in [
        (
            "crates/tako-control/tests/issue2035_lsp_init_options.rs",
            ".find_map(|l| l.split_once(\"TAKO_2035_PROBE sent=\").map(|(_, rest)| rest))",
            ".find_map(|l| l.strip_prefix(\"TAKO_2035_PROBE sent=\"))",
        ),
        (
            "crates/tako-control/tests/issue2048_lsp_isolated_autostart.rs",
            ".find_map(|l| l.split_once(\"TAKO_2048_PROBE \").map(|(_, rest)| rest))",
            ".find(|l| l.starts_with(\"TAKO_2048_PROBE \"))",
        ),
    ] {
        let src = read(rel);
        assert!(violations(rel, &src).is_empty(), "{rel} の現行は通る");
        assert!(
            src.contains(fixed),
            "{rel} に直した読み方が無い（改名したらここも直す）"
        );
        let found = violations(rel, &src.replacen(fixed, legacy, 1));
        assert_eq!(
            found.len(),
            1,
            "{rel} を修正前へ戻した本文を名指さない: {found:?}"
        );
        eprintln!("{}", found[0]);
    }
}

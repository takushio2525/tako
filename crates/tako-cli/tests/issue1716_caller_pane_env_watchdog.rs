//! 番犬: tako-cli のテストが、手元のシェルの `TAKO_PANE_ID` で通る「偽の緑」を作らない（Issue #1716）
//!
//! `build_request` の `target_pane` は `--pane` が無ければ呼び出し元（`TAKO_PANE_ID`）へ倒れる。
//! テストが `--pane` を省略すると、tako の中（シェルが env を継承している）で走らせたときだけ通り、
//! CI（env 無し）で「対象ペインを特定できない」と落ちる（PR #1706 で踏んだ）。塞ぎ方は 2 段:
//!
//! - **単体テスト（`src/main.rs` ほか `tako` の実行ファイルのモジュール）**: テストのビルドでは
//!   `caller_pane` を env を読まない差し替え（`main.rs` の `test_caller`）へ向ける（入口）。
//!   ここでは差し替えを迂回する経路を `file:line` で落とす = env の直読み・lib の中からの
//!   `caller_pane()`・実行ファイルのモジュールからの `ipc::caller_pane` の名指し
//! - **結合テスト（`tests/*.rs`）**: 製品の `tako` を子プロセスで起こすファイルは、
//!   `TAKO_PANE_ID` / `TAKO_SOCKET` を子へ渡さない形（両方の `env_remove` / `env_clear()` /
//!   `TAKO_` で始まる env の一掃）を持つ
//!
//! 意図して製品の読みを見る行（差し替えの実証の対照など）と、env を外さずに `tako` を起こしてよい
//! 結合テストの行（呼び出し元・接続先を読まない段として登録するだけ等）は、同じ行か 1 行上に
//! `// tako:caller-env-ok <理由>` を書くと通す。
//!
//! 既知の穴: env の名前を定数や変数で渡す読み（`var(KEY)`）・`std::env::vars()` の走査・
//! マクロで組んだ呼び出しは見ない。結合テストはファイル単位で見る（同じファイルの別の起動箇所が
//! env を外していれば通る）。規約は `.agent/conventions.md` の Issue #1716 の節。

use std::path::{Path, PathBuf};

const PANE_ENV: &str = "TAKO_PANE_ID";

/// 意図して通す行の目印（同じ行か 1 行上。製品の読みを見る対照・env を外さなくてよい起動）
const OK_MARKER: &str = "tako:caller-env-ok";

/// env を直に読んでよい唯一の場所（製品の呼び出し元ペインの 1 実装）
const ENV_READ_ALLOWED: (&str, &str) = ("src/ipc.rs", "caller_pane");

/// lib の中で `caller_pane()` を呼んでよい関数と理由。lib はテストのビルドでも製品の
/// `ipc::caller_pane`（env を読む）を引くので、ここに無い関数から呼ぶと差し替えを迂回する
const LIB_CALL_ALLOWED: &[(&str, &str, &str)] = &[(
    "src/mcp_bridge.rs",
    "serve_until_end",
    "MCP ブリッジの常駐ループ（実行時だけ。テストは決め方の純粋関数 \
     `caller_pane_plan` へ値を渡して見る）",
)];

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} を読めない: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `crates/tako-cli` からの相対パス（区切りは `/`。Windows でも同じ表記で比べる）
fn rel(file: &Path) -> String {
    file.strip_prefix(crate_dir())
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file:?}: {e}"))
}

/// コメントだけの行（doc を含む）
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 行 `line`（1 始まり）か 1 行上に目印 [`OK_MARKER`] があるか
fn marked_ok(src: &str, line: usize) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    [line.checked_sub(2), Some(line - 1)]
        .into_iter()
        .flatten()
        .filter_map(|i| lines.get(i))
        .any(|l| l.contains(OK_MARKER))
}

/// `word` が識別子の一部ではなく単独で現れるか（`resolve_caller_pane_via_app` を拾わない）
fn has_word(line: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// 行 `line`（1 始まり）を囲む関数の名前（直前の関数の頭。頭の読み方は共有ヘルパの 1 実装 = #1496）
fn enclosing_fn(src: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = src.lines().take(line).collect();
    lines
        .iter()
        .rev()
        .find_map(|l| tako_core::source_scan::fn_head_name(l))
        .map(str::to_string)
}

/// 呼び出し元ペインの env（`TAKO_PANE_ID`）を直に読む行（1 始まり。`set_var` / `remove_var` は読みではない）
fn pane_env_reads(src: &str) -> Vec<usize> {
    let pats = [
        format!("var(\"{PANE_ENV}\")"),
        format!("var_os(\"{PANE_ENV}\")"),
    ];
    src.lines()
        .enumerate()
        .filter(|(_, l)| {
            !is_comment(l)
                && pats.iter().any(|p| {
                    l.match_indices(p.as_str()).any(|(at, _)| {
                        !l[..at]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    })
                })
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// `caller_pane(` を呼ぶ行（定義の行は除く）
fn caller_pane_calls(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| {
            !is_comment(l)
                && !l.contains("fn caller_pane(")
                && l.match_indices("caller_pane(").any(|(at, _)| {
                    !l[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// `caller_pane` を ipc から名指しする行（`ipc::caller_pane` / `use tako_cli::ipc::{… caller_pane …}`）
fn ipc_caller_pane_mentions(src: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut in_group = false;
    for (i, l) in src.lines().enumerate() {
        if is_comment(l) {
            continue;
        }
        let opens = l.contains("ipc::{");
        if opens {
            in_group = true;
        }
        let path = l.match_indices("ipc::caller_pane").any(|(at, _)| {
            !l[at + "ipc::caller_pane".len()..]
                .starts_with(|c: char| c.is_alphanumeric() || c == '_')
        });
        if path || (in_group && has_word(l, "caller_pane")) {
            found.push(i + 1);
        }
        if in_group && l.contains('}') {
            in_group = false;
        }
    }
    found
}

/// 実行ファイルのモジュールで許す唯一の形: `#[cfg(not(test))]` の直後の `use tako_cli::ipc::caller_pane;`
fn is_cfg_not_test_import(src: &str, line: usize) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    let Some(this) = lines.get(line - 1) else {
        return false;
    };
    let prev = lines[..line - 1]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && !is_comment(l));
    this.trim() == "use tako_cli::ipc::caller_pane;"
        && prev.is_some_and(|p| p.trim() == "#[cfg(not(test))]")
}

/// `mod 名前;` で宣言したモジュールのファイル（`src/名前.rs`）
fn declared_modules(root: &str) -> Vec<String> {
    root.lines()
        .filter_map(|l| {
            let t = l.trim();
            let t = t.strip_prefix("pub ").unwrap_or(t);
            let name = t.strip_prefix("mod ")?.strip_suffix(';')?;
            Some(format!("src/{name}.rs"))
        })
        .collect()
}

/// テストのビルドで env を読む `caller_pane` を引く経路（差し替えの迂回）の `file:line`
fn bypasses() -> Vec<String> {
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    files.sort();
    let main = read(&crate_dir().join("src/main.rs"));
    let lib = read(&crate_dir().join("src/lib.rs"));
    let mut bin_modules = vec!["src/main.rs".to_string()];
    bin_modules.extend(declared_modules(&main));
    let mut lib_modules = vec!["src/lib.rs".to_string()];
    lib_modules.extend(declared_modules(&lib));

    let mut found = Vec::new();
    for file in &files {
        let rel = rel(file);
        let src = read(file);
        let mut push = |line: usize, what: String| {
            if !marked_ok(&src, line) {
                found.push(format!("crates/tako-cli/{rel}:{line}: {what}"));
            }
        };
        for line in pane_env_reads(&src) {
            let at = enclosing_fn(&src, line);
            if (rel.as_str(), at.as_deref()) != (ENV_READ_ALLOWED.0, Some(ENV_READ_ALLOWED.1)) {
                push(
                    line,
                    format!("`{PANE_ENV}` の直読み（`caller_pane` を通す）"),
                );
            }
        }
        // lib のモジュール（実行ファイルと共有する stdio.rs も含む）からの呼び出し
        if lib_modules.contains(&rel) {
            for line in caller_pane_calls(&src) {
                let at = enclosing_fn(&src, line);
                let allowed = LIB_CALL_ALLOWED
                    .iter()
                    .any(|(f, func, _)| *f == rel && at.as_deref() == Some(*func));
                if !allowed {
                    push(
                        line,
                        "lib の `caller_pane()`（テストのビルドでも env を読む。呼び出し元は引数で受ける）"
                            .to_string(),
                    );
                }
            }
        }
        if bin_modules.contains(&rel) {
            for line in ipc_caller_pane_mentions(&src) {
                if rel == "src/main.rs" && is_cfg_not_test_import(&src, line) {
                    continue;
                }
                push(
                    line,
                    "`ipc::caller_pane` の名指し（テストのビルドでも env を読む。`main.rs` の \
                     `caller_pane` を使う）"
                        .to_string(),
                );
            }
        }
    }
    found
}

/// 製品の `tako` を子で起こすのに、`TAKO_PANE_ID` / `TAKO_SOCKET` を外す形を持たない結合テスト
fn leaky_spawns() -> Vec<String> {
    let bin_exe = concat!("CARGO_BIN", "_EXE_");
    let this = Path::new(file!())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let mut files = Vec::new();
    rust_files(&crate_dir().join("tests"), &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        if file.file_name().map(|n| n.to_string_lossy().into_owned()) == this {
            continue;
        }
        let src = read(file);
        if let Some(line) = spawn_without_env_strip(&src, bin_exe).filter(|l| !marked_ok(&src, *l))
        {
            found.push(format!("crates/tako-cli/{}:{line}", rel(file)));
        }
    }
    found
}

/// `bin_exe` を含むのに env を外す形が無ければ、最初の `bin_exe` の行（1 始まり）
fn spawn_without_env_strip(src: &str, bin_exe: &str) -> Option<usize> {
    let line = src
        .lines()
        .enumerate()
        .find(|(_, l)| !is_comment(l) && l.contains(bin_exe))?
        .0
        + 1;
    let strips_both =
        src.contains("env_remove(\"TAKO_PANE_ID\")") && src.contains("env_remove(\"TAKO_SOCKET\")");
    let clears = src.contains(".env_clear()") || src.contains("starts_with(\"TAKO_\")");
    (!strips_both && !clears).then_some(line)
}

#[test]
fn テストのビルドで呼び出し元ペインのenvを読む経路を作らない() {
    let found = bypasses();
    assert!(
        found.is_empty(),
        "テストが `--pane` を省略すると、手元のシェルが継承した `TAKO_PANE_ID` で通り CI（env 無し）で\
         落ちる（#1716）。テストのビルドでは `main.rs` の `caller_pane` が env を読まない差し替えへ\
         向くので、それを迂回しない:\n{}",
        found.join("\n")
    );
}

#[test]
fn 結合テストは製品のtakoへ呼び出し元と接続先のenvを渡さない() {
    let found = leaky_spawns();
    assert!(
        found.is_empty(),
        "製品の `tako` を子で起こす結合テストは `TAKO_PANE_ID` / `TAKO_SOCKET` を外す（両方の \
         `env_remove` / `env_clear()` / `TAKO_` で始まる env の一掃）。外さないと手元でだけ通る・\
         本番の GUI へ届く（#1716）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 検出器は直読みと迂回と外し忘れを拾う() {
    // env の直読み（コメントは数えない・定数経由は既知の穴）
    let src = "fn a() {\n    std::env::var(\"TAKO_PANE_ID\").ok();\n}\n\
               // std::env::var(\"TAKO_PANE_ID\")\n\
               fn b() {\n    std::env::var_os(\"TAKO_PANE_ID\");\n    std::env::var(KEY);\n}\n\
               fn c() {\n    std::env::remove_var(\"TAKO_PANE_ID\");\n    std::env::set_var(\"TAKO_PANE_ID\", \"1\");\n}\n";
    assert_eq!(pane_env_reads(src), vec![2, 6]);
    assert_eq!(enclosing_fn(src, 2).as_deref(), Some("a"));
    assert_eq!(enclosing_fn(src, 6).as_deref(), Some("b"));
    let src = "pub(crate) async fn head() {\n    std::env::var(\"TAKO_PANE_ID\");\n}\n";
    assert_eq!(enclosing_fn(src, 2).as_deref(), Some("head"));

    // lib の呼び出し（定義と似た名前は数えない）
    let src = "pub fn caller_pane() -> Option<u64> {\n    None\n}\n\
               fn serve() {\n    let c = caller_pane();\n    resolve_caller_pane_via_app(None);\n}\n\
               fn plan() {\n    caller_pane_plan(ipc::caller_pane(), true);\n}\n";
    assert_eq!(caller_pane_calls(src), vec![5, 9]);

    // 実行ファイルのモジュールからの名指し（use の束の中・修飾した呼び出し）
    let src = "use tako_cli::ipc::{\n    caller_pane, resolve_caller_pane_via_app, send_request,\n};\n\
               #[cfg(not(test))]\nuse tako_cli::ipc::caller_pane;\n\
               #[cfg(test)]\nuse test_caller::caller_pane;\n\
               fn x() {\n    tako_cli::ipc::caller_pane();\n    tako_cli::ipc::resolve_caller_pane_via_app(None);\n}\n\
               use tako_cli::ipc::caller_pane;\n";
    assert_eq!(ipc_caller_pane_mentions(src), vec![2, 5, 9, 12]);
    assert!(is_cfg_not_test_import(src, 5));
    assert!(
        !is_cfg_not_test_import(src, 12),
        "cfg の無い取り込みは通さない"
    );
    // 目印は同じ行か 1 行上だけ
    let src = "// tako:caller-env-ok 対照\nlet a = ipc::caller_pane();\n\nlet b = ipc::caller_pane(); // tako:caller-env-ok 対照\nlet c = ipc::caller_pane();\n";
    assert_eq!(
        (1..=5).map(|l| marked_ok(src, l)).collect::<Vec<_>>(),
        [true, true, false, true, true]
    );
    assert!(!marked_ok("x\ny\nz\n", 1) && !marked_ok("x\ny\nz\n", 3));

    // 結合テスト: 外す形が無いファイルだけ最初の起動の行を返す
    let bin = "CARGO_BIN_EXE_";
    let leaky = "fn t() {\n    // CARGO_BIN_EXE_tako\n    Command::new(env!(\"CARGO_BIN_EXE_tako\"))\n        .arg(\"x\");\n}\n";
    assert_eq!(spawn_without_env_strip(leaky, bin), Some(3));
    let half = format!(
        "{leaky}// .env_remove(\"TAKO_PANE_ID\")\nfn u() {{ c.env_remove(\"TAKO_PANE_ID\"); }}\n"
    );
    assert_eq!(
        spawn_without_env_strip(&half, bin),
        Some(3),
        "片方だけでは通さない"
    );
    for strip in [
        ".env_remove(\"TAKO_PANE_ID\").env_remove(\"TAKO_SOCKET\")",
        ".env_clear()",
        "if k.starts_with(\"TAKO_\") {}",
    ] {
        assert_eq!(
            spawn_without_env_strip(&format!("{leaky}{strip}\n"), bin),
            None,
            "{strip}"
        );
    }
    assert_eq!(
        spawn_without_env_strip("fn t() {}\n", bin),
        None,
        "起こさないファイルは見ない"
    );

    // `mod` の宣言からモジュールのファイルを出す
    assert_eq!(
        declared_modules("#[macro_use]\nmod stdio;\n\npub mod ipc;\nmod setup;\nmod inline {\n}\n"),
        vec!["src/stdio.rs", "src/ipc.rs", "src/setup.rs"]
    );
}

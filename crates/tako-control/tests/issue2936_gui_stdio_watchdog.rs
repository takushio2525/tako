//! 番犬: GUI のプロセスの標準出力・標準エラーへの書き込みが panic しうる形へ戻らない（Issue #2936）
//!
//! 修正前は GUI（tako-app）の出力マクロが std の版で、書けない標準エラー（Windows の実測は
//! 受け継いだ std ハンドルが書けない別物を指した `ERROR_ACCESS_DENIED`）への 1 行で panic し、
//! GUI ごと落ちた。直し方は `tako_core::stdio` の先頭（GUI の規則）。守りは 3 つの約束で成り立つので、
//! それぞれを `file:line` で名指して固定する:
//!
//! 1. tako-app の `fn main` は **2 文目で** GUI の規則（`tako_core::stdio::adopt_gui_rule`）を採る。
//!    採るのは GUI の入口だけ（CLI が採ると、読み手の閉じたパイプで静かに 0 で終わる CLI の規則が
//!    崩れる）。例外は #2936 のテストの子を起こす補助（`tako_core::test_child`）
//! 2. GUI のプロセスで走るクレート（tako-app / tako-core / tako-control。`src/bin/` の別の実行ファイルを
//!    除く）の本番コードは、出力マクロ以外の口で標準出力・標準エラーへ書かない。直にハンドルを取る
//!    場所は凍結表（理由つき）に載せ、増えた・減ったら `file:line` で名指す
//! 3. 同じ本番コードに `dbg!` を置かない（std の `eprintln!` を通り、書けないと panic する）
//!
//! 出力マクロの宣言の順（tako-app を含む 5 つの根）は `crates/tako-cli/tests/issue1758_broken_pipe.rs`、
//! std の版の名指し（tako-app を含む）は `crates/tako-cli/tests/issue1805_lib_stdio.rs` が見る。
//! 実際に書けない標準エラーで走り切ることは `crates/tako-core/tests/issue2936_gui_stdio.rs` と
//! tako-app の `stdio::tests` が子プロセスで見る。

#[path = "common/emoji_scan.rs"]
mod emoji_scan;

use emoji_scan::production_range::{self, code_view};

/// GUI の入口
const GUI_MAIN: &str = "crates/tako-app/src/main.rs";

/// `fn main` の 1 文目（#1811 の番犬が固定している）
const MARK_STMT: &str = "tako_core::paths::mark_product_process();";

/// `fn main` の 2 文目に置く文
const ADOPT_STMT: &str = "tako_core::stdio::adopt_gui_rule();";

/// GUI の規則を採ってよい場所（それ以外が呼べば名指す）
const ADOPT_CALLERS: &[(&str, &str)] = &[
    (GUI_MAIN, "GUI の入口（fn main の 2 文目）"),
    (
        "crates/tako-core/src/test_child.rs",
        "テスト専用: #2936 の子が書けない標準エラーの下で規則を採る（stdio_child_report）",
    ),
];

/// GUI のプロセスで走るクレートのソース（`src/bin/` は別の実行ファイルなので除く）
const GUI_CRATE_SOURCES: &[&str] = &[
    "crates/tako-app/src",
    "crates/tako-core/src",
    "crates/tako-control/src",
];

/// 標準出力・標準エラーのハンドルを直に取ってよい場所（ファイル, 数, 理由）。
/// `.is_terminal()` だけを問う形は書き込まないので数えない
const DIRECT_HANDLES: &[(&str, usize, &str)] = &[
    (
        "crates/tako-core/src/stdio.rs",
        4,
        "出力マクロの実体（GUI の規則と CLI の規則。GUI の規則では書き込みの失敗を捨てる）",
    ),
    (
        "crates/tako-core/src/test_child.rs",
        1,
        "テスト専用: #2936 の子が規則を採る前に標準エラーへ書けるかを確かめる（結果は Err で受ける）",
    ),
    (
        "crates/tako-control/src/queue_wrapper.rs",
        3,
        "`tako queue` の CLI の待ち表示と進捗の素通し（GUI のプロセスでは走らない・\
         書き込みはどれも `let _ =` で失敗を捨てる）",
    ),
];

/// 桁 0 の `fn main` の本体（`{` の直後から）を、コメントを落とした眺めで返す
fn main_body(rel: &str) -> String {
    let src = std::fs::read_to_string(emoji_scan::repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    main_body_of(&src, rel)
}

fn main_body_of(src: &str, rel: &str) -> String {
    let view = code_view::without_comments_checked(src, rel);
    let mut offset = 0;
    for line in view.split_inclusive('\n') {
        if tako_core::source_scan::is_top_level_fn_head(line)
            && tako_core::source_scan::fn_head_name(line) == Some("main")
        {
            let rest = &view[offset..];
            let open = rest
                .find('{')
                .unwrap_or_else(|| panic!("{rel}: fn main の本体の `{{` が無い"));
            return rest[open + 1..].to_string();
        }
        offset += line.len();
    }
    panic!("{rel}: 桁 0 の fn main が無い（走査が壊れている）");
}

/// `fn main` の本体の 2 文目が GUI の規則の採用か。違えば先頭 3 行を添えた理由
fn adopts_second(body: &str) -> Result<(), String> {
    let head = || {
        body.trim_start()
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(rest) = body.trim_start().strip_prefix(MARK_STMT) else {
        return Err(format!(
            "1 文目が `{MARK_STMT}` ではない（#1811）。先頭:\n{}",
            head()
        ));
    };
    if rest.trim_start().starts_with(ADOPT_STMT) {
        Ok(())
    } else {
        Err(format!(
            "2 文目が `{ADOPT_STMT}` ではない。採らない / 後ろへ動くと、それより前の出力と \
             tako-core / tako-control / tako-app の全 `eprintln!` が書けない標準エラーで panic し \
             GUI ごと落ちる（#2936）。先頭:\n{}",
            head()
        ))
    }
}

/// 1: GUI の入口は 2 文目で GUI の規則を採る
#[test]
fn guiのmainは2文目でguiの規則を採る() {
    if let Err(why) = adopts_second(&main_body(GUI_MAIN)) {
        panic!("{GUI_MAIN}: {why}");
    }
}

/// 1 の検出力: 採らない・3 文目へ動いた形を落とし、今の形を通す
#[test]
fn guiのmainの2文目の判定は外した形を落とす() {
    let ok =
        format!("fn main() {{\n    {MARK_STMT}\n    // 説明\n    {ADOPT_STMT}\n    x();\n}}\n");
    assert!(adopts_second(&main_body_of(&ok, "ok.rs")).is_ok());
    let missing = format!("fn main() {{\n    {MARK_STMT}\n    x();\n}}\n");
    assert!(adopts_second(&main_body_of(&missing, "missing.rs")).is_err());
    let third = format!("fn main() {{\n    {MARK_STMT}\n    x();\n    {ADOPT_STMT}\n}}\n");
    assert!(adopts_second(&main_body_of(&third, "third.rs")).is_err());
    let commented = format!("fn main() {{\n    {MARK_STMT}\n    // {ADOPT_STMT}\n    x();\n}}\n");
    assert!(
        adopts_second(&main_body_of(&commented, "commented.rs")).is_err(),
        "コメントアウトした採用を採用と読んでいる"
    );
}

/// 1: GUI の規則を採るのは GUI の入口（とテストの子の補助）だけ
#[test]
fn guiの規則を採るのはguiの入口だけ() {
    const CALL: &str = "adopt_gui_rule(";
    let mut stray = Vec::new();
    let mut definitions = 0;
    for (rel, src) in emoji_scan::sources_under(&["crates"]) {
        let code = code_view::code_view(&src);
        for (idx, line) in code.lines().enumerate() {
            if !line.contains(CALL) {
                continue;
            }
            if line.contains(&format!("fn {CALL}")) {
                definitions += 1;
                continue;
            }
            if !ADOPT_CALLERS.iter().any(|(allowed, _)| *allowed == rel) {
                stray.push(format!("{rel}:{}: {}", idx + 1, line.trim()));
            }
        }
    }
    assert_eq!(
        definitions, 1,
        "adopt_gui_rule の定義が 1 つではない（走査が空振り / 改名）"
    );
    assert!(
        stray.is_empty(),
        "GUI の規則（書き込みの失敗を種類によらず捨てる）を GUI の入口以外が採っている。\
         CLI が採ると読み手の閉じたパイプで静かに 0 で終わる CLI の規則が崩れる（#2936 / #1758）:\n{}",
        stray.join("\n")
    );
}

/// 識別子の一部・メソッド呼び出しではない `name()` の位置（`io::stderr()` / `stderr()`）
fn call_sites(line: &str, name: &str) -> Vec<usize> {
    let needle = format!("{name}()");
    line.match_indices(&needle)
        .filter(|(at, _)| {
            !line[..*at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
        .map(|(at, _)| at)
        .collect()
}

/// 本番コード（テスト領域・コメント・文字列を潰した眺め）で、標準出力・標準エラーのハンドルを
/// 直に取る位置（`.is_terminal()` だけを問う形を除く）
fn direct_handle_sites(src: &str) -> Vec<usize> {
    let production = production_range::scan(src).text;
    let code = code_view::code_view(&production);
    let mut lines = Vec::new();
    for (idx, line) in code.lines().enumerate() {
        for name in ["stdout", "stderr"] {
            for at in call_sites(line, name) {
                let after = &line[at + name.len() + 2..];
                if after.trim_start().starts_with(".is_terminal()") {
                    continue;
                }
                lines.push(idx + 1);
            }
        }
    }
    lines
}

/// `dbg!(` の位置（本番コード）
fn dbg_sites(src: &str) -> Vec<usize> {
    let production = production_range::scan(src).text;
    let code = code_view::code_view(&production);
    code.lines()
        .enumerate()
        .filter(|(_, line)| {
            line.match_indices("dbg!(").any(|(at, _)| {
                !line[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        })
        .map(|(idx, _)| idx + 1)
        .collect()
}

fn gui_sources() -> Vec<(String, String)> {
    let sources: Vec<(String, String)> = emoji_scan::sources_under(GUI_CRATE_SOURCES)
        .into_iter()
        .filter(|(rel, _)| !rel.contains("/src/bin/"))
        .collect();
    assert!(
        sources.iter().any(|(rel, _)| rel == GUI_MAIN) && sources.len() > 200,
        "GUI のクレートのソースを拾えていない（{} 本）",
        sources.len()
    );
    sources
}

/// 2: 出力マクロ以外の口（ハンドルを直に取る形）は凍結表の場所と数だけ
#[test]
fn guiのプロセスで標準出力と標準エラーを直に書く場所は凍結表だけ() {
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    for (rel, src) in gui_sources() {
        let sites = direct_handle_sites(&src);
        if sites.is_empty() {
            continue;
        }
        seen.push(rel.clone());
        let names = sites
            .iter()
            .map(|l| format!("{rel}:{l}"))
            .collect::<Vec<_>>()
            .join("\n  ");
        match DIRECT_HANDLES.iter().find(|(f, _, _)| *f == rel) {
            None => problems.push(format!(
                "凍結表に無いファイルが標準出力・標準エラーのハンドルを直に取っている（{} か所）:\n  {names}",
                sites.len()
            )),
            Some((_, count, _)) if *count != sites.len() => problems.push(format!(
                "{rel} の直のハンドルが凍結表の {count} か所から {} か所へ変わった:\n  {names}",
                sites.len()
            )),
            Some(_) => {}
        }
    }
    for (file, _, _) in DIRECT_HANDLES {
        if !seen.iter().any(|s| s == file) {
            problems.push(format!(
                "凍結表の {file} に直のハンドルが無い（消したなら表からも外す / 走査が空振りしている）"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "GUI のプロセスで走るコードが出力マクロを通らずに標準出力・標準エラーへ書いている（#2936）。\n\
         `writeln!(io::stderr(), …).unwrap()` のような形は書けない標準エラーで panic し GUI ごと落ちる。\n\
         出力マクロ（`eprintln!` 等 = `tako_core::stdio` の GUI の規則）で書くか、失敗を捨てる形\
         （`let _ = …`）にしたうえで凍結表へ理由つきで足す:\n{}",
        problems.join("\n")
    );
}

/// 2 の検出力: 直のハンドルの形を拾い、`.is_terminal()` と別名のメソッドは拾わない
#[test]
fn 直のハンドルの判定は書き込みの形だけを拾う() {
    let src = "fn f() {\n\
               let _ = writeln!(std::io::stderr(), \"x\").unwrap();\n\
               let mut out = io::stdout();\n\
               let tty = std::io::stderr().is_terminal();\n\
               let e = child.stderr();\n\
               let s = my_stderr();\n\
               // io::stderr() はコメント\n\
               let t = \"io::stdout()\";\n\
               }\n";
    assert_eq!(
        direct_handle_sites(src),
        vec![2, 3],
        "拾うのは 2 行目と 3 行目だけ"
    );
}

/// 3: 本番コードに `dbg!` を置かない
#[test]
fn guiのプロセスの本番コードにdbgを置かない() {
    let mut found = Vec::new();
    for (rel, src) in gui_sources() {
        for line in dbg_sites(&src) {
            found.push(format!("{rel}:{line}"));
        }
    }
    assert!(
        found.is_empty(),
        "`dbg!` は std の `eprintln!` を通り、書けない標準エラーで panic して GUI ごと落ちる（#2936）。\
         消すか、残すなら出力マクロ（`eprintln!`）で書く:\n{}",
        found.join("\n")
    );
    assert_eq!(
        dbg_sites("fn f() {\n    dbg!(x);\n    my_dbg!(y);\n}\n"),
        vec![2]
    );
}

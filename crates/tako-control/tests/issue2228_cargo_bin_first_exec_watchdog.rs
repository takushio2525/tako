//! 番犬: cargo が作る偽の言語サーバ（`tako-lsp-fake`）を、e2e の時計の中で初めて exec しない（Issue #2228）
//!
//! # なぜ要るか
//!
//! macOS は新しいファイルの初回の exec に検査を挟む（#2222）。cargo が作り直した実行ファイルも
//! 新しいファイルで、検査の長さは大きさに比例する（2 MB の偽サーバで 0.16〜0.35 秒・83 MB の debug の
//! `tako` で 1.4〜1.5 秒。検査はシステム全体で 1 本ずつ進み、他のビルドと重なると上限 30 秒を食った
//! 実例がある = #1838）。LSP の e2e は `wait_until(…, 10 秒)` などの時計を偽サーバの起動ごと測るので、
//! 作り直した直後の最初のテストのバイナリが検査を時計の中で払う。直し方は時計を回す前に
//! `tako_core::fake_exec::warm_binary` で 1 回空打ちすること（規約 `.agent/conventions.md` の #2222 の節）。
//!
//! # 縛ること
//!
//! - [`偽サーバは_main_の_1_文目で空打ちと注入を受ける`] — 空打ち（`tako-fake-exec-warm`）で本文を
//!   走らせずに終わり、注入（`TAKO_2222_INJECT_FIRST_EXEC_MS`）で初回の exec を演じる
//! - [`偽サーバを名指す_e2e_は空打ちを通す`] — `tests/` で `CARGO_BIN_EXE_tako-lsp-fake` を名指す
//!   ファイルは `fake_exec::warm_binary(` を呼ぶ（ファイル単位。どの関数で呼ぶかまでは見ない）
//! - [`実物の偽サーバは空打ちで本文を走らせず注入の印を書く`] — 上の 2 つの配線を実プロセスで見る
//!
//! - [`gui_のセルフテストは偽の実行ファイルを空打ちの口で書く`] — GUI のセルフテスト（main.rs の
//!   `mod self_test` と `crates/tako-app/src/self_test/`）は、節が使うスタブに実行権を直に付けない
//!   （`from_mode(0o755)` ほか）。書く口は `write_fake_exec`（中は `fake_exec::write_script` = 空打ちつき）。
//!   #601 の段と `run-dropdown-1733` の偽の python が 20 秒の窓の中で初めて exec されていた（#2228 の 2）
//!
//! テストのスクリプト（`scripts/**/test-*.sh`）の同じ形（素の `chmod +x` のスタブ）は、シェルの
//! 字句解析を持つ `shell_scripts.rs` の `テストのスタブは素のchmodではなく空打ちの補助で実行権を付ける`
//! が縛る（既存分は数を凍結・増やすと file:line で名指す。#2228 の 1）。
//!
//! 既知の穴: 空打ちを呼ぶ位置（時計より前か）と、写し（`fs::copy(FAKE, …)`）を起こす前の掛け直しは
//! 見ない。どちらも規約で縛る。セルフテストでスタブを写す（`fs::copy`）形も見ない（今は無い）。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で対象のファイルが採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use code_view::{code_view, without_comments};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const TESTS: &str = "crates/tako-control/tests";
const FAKE_SRC: &str = "crates/tako-control/src/bin/tako-lsp-fake.rs";
const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// e2e が偽サーバを名指す字面（`env!("CARGO_BIN_EXE_tako-lsp-fake")` の中身）
const FAKE_ENV: &str = "CARGO_BIN_EXE_tako-lsp-fake";
/// 空打ちの呼び出し
const WARM_CALL: &str = "fake_exec::warm_binary(";
/// 偽サーバの `main` の 1 文目
const ENTER_STMT: &str = "tako_core::fake_exec::enter_test_binary();";
/// この番犬自身（偽サーバを名指すが、時計を持たない実プロセスの確かめだけ）
const SELF_FILE: &str = "issue2228_cargo_bin_first_exec_watchdog.rs";

/// GUI のセルフテストの本体が居るファイル（`mod self_test {` から行頭の `}` まで）
const APP_MAIN: &str = "crates/tako-app/src/main.rs";
/// GUI のセルフテストの節のファイルの置き場
const SELF_TEST_DIR: &str = "crates/tako-app/src/self_test";
/// 節のスタブを書く口（`mod self_test` の中の 1 実装）
const SELF_TEST_WRITER: &str = "write_fake_exec(";
/// その口の中身（空打ちつきで書く 1 実装）
const SELF_TEST_WRITER_BODY: &str = "tako_core::fake_exec::write_script(";
/// 実行権を付けるモードの指定（この後ろの 8 進数を読む）
const EXEC_MODE_CALLS: &[&str] = &["from_mode(0o", "set_mode(0o", ".mode(0o"];
/// 監査済みの例外宣言 `// tako:fake-exec-ok <理由>`（その行か直前の行。理由の無い宣言は無効）
const EXEC_MODE_OK: &str = "tako:fake-exec-ok";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `tests/` の下の Rust ファイル（`common/` を含む。この番犬は除く）を `(相対パス, 中身)` で
fn test_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = workspace_root();
    let mut files = Vec::new();
    walk(&root.join(TESTS), &mut files);
    files.sort();
    files
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|n| n != SELF_FILE))
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).unwrap();
            (rel, src)
        })
        .collect()
}

/// 偽サーバを名指すのに空打ちを呼ばないファイルを `rel:行` で返す（名指した最初の行）
fn missing_warm(rel: &str, src: &str) -> Option<String> {
    // 名指しは文字列の中（`env!("…")`）にあるのでコメントだけ潰す。呼び出しはコードだけで見る
    let named = without_comments(src);
    let at = named.find(FAKE_ENV)?;
    if code_view(src).contains(WARM_CALL) {
        return None;
    }
    let line = named[..at].matches('\n').count() + 1;
    Some(format!("{rel}:{line}"))
}

/// 偽サーバの `main` の 1 文目が [`ENTER_STMT`] でなければ `rel:行` を返す
fn fake_main_violation(rel: &str, src: &str) -> Option<String> {
    let code = code_view(src);
    let head = code
        .find("fn main() {")
        .unwrap_or_else(|| panic!("{rel}: fn main が見つからない（走査が空振りしている）"));
    let body_start = head + "fn main() {".len();
    // コメントを潰した眺めで最初の文を探し、元のソースの同じ位置の文字を比べる（長さは同じ）
    let offset = code[body_start..].find(|c: char| !c.is_whitespace())?;
    let first = body_start + offset;
    if src[first..].starts_with(ENTER_STMT) {
        return None;
    }
    Some(format!("{rel}:{}", src[..first].matches('\n').count() + 1))
}

/// GUI のセルフテストの本体（main.rs の `mod self_test {` の次の行から、行頭の `}` の前まで）の
/// 0 始まりの行の範囲
fn self_test_range(src: &str) -> std::ops::Range<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == "mod self_test {")
        .unwrap_or_else(|| {
            panic!("{APP_MAIN} に `mod self_test {{` が無い（走査が空振りしている）")
        })
        + 1;
    let end = start
        + lines[start..]
            .iter()
            .position(|l| *l == "}")
            .unwrap_or_else(|| panic!("{APP_MAIN} の `mod self_test` の閉じ括弧が無い"));
    start..end
}

/// GUI のセルフテストの節のファイル（`self_test/` の下の Rust）を `(相対パス, 中身)` で
fn self_test_sources() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut out = Vec::new();
    let mut dirs = vec![root.join(SELF_TEST_DIR)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// コードの 1 行に、実行権（どれかの x）を付けるモードの指定があるか
fn grants_exec_mode(code_line: &str) -> bool {
    EXEC_MODE_CALLS.iter().any(|call| {
        code_line.match_indices(call).any(|(at, _)| {
            let digits: String = code_line[at + call.len()..]
                .chars()
                .take_while(|c| c.is_digit(8) || *c == '_')
                .filter(|c| *c != '_')
                .collect();
            u32::from_str_radix(&digits, 8).is_ok_and(|mode| mode & 0o111 != 0)
        })
    })
}

/// `range`（0 始まりの行）の中で、スタブに実行権を直に付けている行を `rel:行` で返す。
/// 呼び出しはコードだけで見る（コメントと文字列は潰す）。例外宣言は原文で見る
fn self_test_exec_mode_violations(
    rel: &str,
    src: &str,
    range: std::ops::Range<usize>,
) -> Vec<String> {
    let code = code_view(src);
    let original: Vec<&str> = src.lines().collect();
    let declared = |i: usize| {
        original.get(i).is_some_and(|l| {
            l.split(EXEC_MODE_OK)
                .nth(1)
                .is_some_and(|reason| !reason.trim().is_empty())
        })
    };
    code.lines()
        .enumerate()
        .filter(|(i, line)| {
            range.contains(i)
                && grants_exec_mode(line)
                && !(declared(*i) || i.checked_sub(1).is_some_and(declared))
        })
        .map(|(i, _)| format!("{rel}:{}", i + 1))
        .collect()
}

/// GUI のセルフテスト全体（main.rs の `mod self_test` + `self_test/`）の違反
fn all_self_test_violations() -> Vec<String> {
    let main = read(APP_MAIN);
    let mut out = self_test_exec_mode_violations(APP_MAIN, &main, self_test_range(&main));
    for (rel, src) in self_test_sources() {
        let lines = src.lines().count();
        out.extend(self_test_exec_mode_violations(&rel, &src, 0..lines));
    }
    out
}

#[test]
fn gui_のセルフテストは偽の実行ファイルを空打ちの口で書く() {
    let violations = all_self_test_violations();
    assert!(
        violations.is_empty(),
        "GUI のセルフテストが節のスタブに実行権を直に付けている（#2228）。作りたてのスタブを節の時計\
         （`wait_for_focused_text` の 20 秒・一覧の待ちほか）の中で初めて exec すると、macOS の初回 exec の\
         検査を時計の中で払う。`write_fake_exec(cx, &path, text).await`（中は `fake_exec::write_script` =\
         書いて実行権を付けて空打ち）で書く。スタブでない物（ディレクトリの権限ほか）は、その行か直前の行に\
         `// {EXEC_MODE_OK} <理由>` で外せる:\n{}",
        violations.join("\n")
    );
}

#[test]
fn セルフテストの空打ちの口は_fake_exec_を通す() {
    let main = read(APP_MAIN);
    let range = self_test_range(&main);
    let code = code_view(&main);
    let lines: Vec<&str> = code.lines().collect();
    let head = (range.start..range.end)
        .find(|&i| lines[i].contains("async fn write_fake_exec("))
        .unwrap_or_else(|| panic!("{APP_MAIN} の `mod self_test` に `write_fake_exec` が無い"));
    // 口の本体（次の空行まで）が 1 実装を呼ぶ
    let body: String = lines[head..]
        .iter()
        .take_while(|l| !l.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains(SELF_TEST_WRITER_BODY),
        "`write_fake_exec` が `{SELF_TEST_WRITER_BODY}` を通していない（空打ちが抜ける）:\n{body}"
    );
    // #2228 で寄せた 2 か所が口を使い続けている
    let at_601 = (range.start..range.end)
        .any(|i| lines[i].contains(SELF_TEST_WRITER) && lines[i].contains("&user_cli"));
    assert!(
        at_601,
        "{APP_MAIN} の #601 の段が `{SELF_TEST_WRITER}` でユーザーの tako を書いていない"
    );
    let dropdown = self_test_sources()
        .into_iter()
        .find(|(rel, _)| rel.ends_with("run_dropdown_1733.rs"))
        .expect("run_dropdown_1733.rs が採れる");
    assert!(
        code_view(&dropdown.1).contains(SELF_TEST_WRITER),
        "{} の偽の python が `{SELF_TEST_WRITER}` を通していない",
        dropdown.0
    );
    // 走査が空振りしていない（11/04 時点で本体 6 万行超・節のファイル 30 本超）
    assert!(
        range.len() > 30_000,
        "mod self_test が {} 行しか採れない",
        range.len()
    );
    assert!(
        self_test_sources().len() >= 10,
        "self_test/ の節のファイルが少なすぎる"
    );
}

#[test]
fn 偽サーバは_main_の_1_文目で空打ちと注入を受ける() {
    let violation = fake_main_violation(FAKE_SRC, &read(FAKE_SRC));
    assert!(
        violation.is_none(),
        "{}: 偽サーバの fn main の 1 文目が `{ENTER_STMT}` ではない（#2228）。\
         空打ちで本文が走る / 初回 exec の注入が効かない",
        violation.unwrap()
    );
}

#[test]
fn 偽サーバを名指す_e2e_は空打ちを通す() {
    let missing: Vec<String> = test_sources()
        .iter()
        .filter_map(|(rel, src)| missing_warm(rel, src))
        .collect();
    assert!(
        missing.is_empty(),
        "偽サーバ（`{FAKE_ENV}`）を名指すのに `{WARM_CALL}` を呼ばない e2e がある（#2228）。\n\
         時計（wait_until・要求の上限）を回す前に `tako_core::fake_exec::warm_binary(FAKE)` を呼ぶ:\n{}",
        missing.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let named: Vec<String> = test_sources()
        .into_iter()
        .filter(|(_, src)| without_comments(src).contains(FAKE_ENV))
        .map(|(rel, _)| rel)
        .collect();
    // 10/31 時点で 25 本。大きく減ったら読み先か字面の取り違え
    assert!(
        named.len() >= 20,
        "偽サーバを名指す e2e が {} 本しか採れない（走査が空振りしている）: {named:?}",
        named.len()
    );
    assert!(
        named
            .iter()
            .any(|rel| rel.ends_with("issue1678_lsp_e2e.rs")),
        "{named:?}"
    );
}

/// 実プロセス: 空打ちの引数なら本文を走らせない（起動の記録 `--spawns` を書かない）。
/// 注入の env があれば初回の exec で印を書き、2 回目は書き換えない（写し = 新しいファイルで見る）
#[test]
fn 実物の偽サーバは空打ちで本文を走らせず注入の印を書く() {
    let scratch = tako_core::test_residue::ScratchDir::new("t2228-fake-warm");
    let spawns = scratch.path().join("spawns.txt");
    let out = Command::new(FAKE)
        .args([tako_core::fake_exec::WARM_ARG, "--spawns"])
        .arg(&spawns)
        .stdin(Stdio::null())
        .env_remove(tako_core::fake_exec::INJECT_FIRST_EXEC_ENV)
        .output()
        .expect("偽サーバを起こせる");
    assert!(out.status.success(), "{out:?}");
    assert!(!spawns.exists(), "空打ちで本文が走った（起動を記録した）");

    let copy = scratch
        .path()
        .join(format!("fake-copy{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(FAKE, &copy).expect("偽サーバを写す");
    let marker = scratch.path().join(format!(
        "fake-copy{}.first-exec",
        std::env::consts::EXE_SUFFIX
    ));
    let run = || {
        let out = Command::new(&copy)
            .arg(tako_core::fake_exec::WARM_ARG)
            .stdin(Stdio::null())
            .env(tako_core::fake_exec::INJECT_FIRST_EXEC_ENV, "1")
            .output()
            .expect("写しを起こせる");
        assert!(out.status.success(), "{out:?}");
    };
    run();
    let first = std::fs::read_to_string(&marker).expect("初回の exec で注入の印を書く");
    run();
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        first,
        "2 回目の exec は初回ではない（印を書き換えない）"
    );
}

/// 現行ソースから作り直した逆戻りが file:line で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    // A. e2e から空打ちを外す（修正前の形）
    let rel = "crates/tako-control/tests/issue1678_lsp_e2e.rs";
    let src = read(rel);
    assert_eq!(missing_warm(rel, &src), None, "現行は通る");
    let reverted: String = src
        .lines()
        .filter(|line| !line.contains(WARM_CALL))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(
        reverted, src,
        "注入が空振りしている（空打ちの行が見つからない）"
    );
    let named = missing_warm(rel, &reverted).expect("空打ちを外した e2e を名指す");
    let want_line = src
        .lines()
        .position(|line| line.contains(FAKE_ENV))
        .unwrap()
        + 1;
    assert_eq!(named, format!("{rel}:{want_line}"));

    // B. 偽サーバの main から 1 文目を外す
    let src = read(FAKE_SRC);
    assert_eq!(fake_main_violation(FAKE_SRC, &src), None, "現行は通る");
    let reverted = src.replacen(&format!("    {ENTER_STMT}\n"), "", 1);
    assert_ne!(
        reverted, src,
        "注入が空振りしている（1 文目が見つからない）"
    );
    let named = fake_main_violation(FAKE_SRC, &reverted).expect("1 文目を外した main を名指す");
    assert!(named.starts_with(&format!("{FAKE_SRC}:")), "{named}");
    let line: usize = named.rsplit(':').next().unwrap().parse().unwrap();
    assert!(
        reverted
            .lines()
            .nth(line - 1)
            .is_some_and(|l| l.contains("let args")),
        "名指した行が外した後の 1 文目: {named}"
    );

    // C. 1 文目の前に別の文を置く（順序の逆戻り = 注入より前に本文の読みが走る）
    let moved = src.replacen(
        &format!("    {ENTER_STMT}\n"),
        &format!("    let early = std::env::args().count();\n    {ENTER_STMT}\n"),
        1,
    );
    assert!(fake_main_violation(FAKE_SRC, &moved).is_some());

    // D. #601 の段: 空打ちの口の後ろに、実行権を直に付ける修正前の行を戻す
    let main = read(APP_MAIN);
    let range = self_test_range(&main);
    assert!(self_test_exec_mode_violations(APP_MAIN, &main, range.clone()).is_empty());
    let anchor = main
        .lines()
        .position(|l| l.contains(SELF_TEST_WRITER) && l.contains("&user_cli"))
        .expect("#601 の段の空打ちの行");
    let legacy = "                    std::fs::set_permissions(&user_cli, std::fs::Permissions::from_mode(0o755))";
    let reverted: String = main
        .lines()
        .enumerate()
        .flat_map(|(i, l)| {
            let mut v = vec![format!("{l}\n")];
            if i == anchor {
                v.push(format!("{legacy}\n"));
            }
            v
        })
        .collect();
    let named = self_test_exec_mode_violations(APP_MAIN, &reverted, self_test_range(&reverted));
    assert_eq!(named, vec![format!("{APP_MAIN}:{}", anchor + 2)]);
    // 理由つきの例外宣言なら外せる・理由が無ければ外せない
    let declared = reverted.replacen(
        legacy,
        &format!("{legacy} // {EXEC_MODE_OK} ディレクトリの権限の戻し"),
        1,
    );
    assert!(
        self_test_exec_mode_violations(APP_MAIN, &declared, self_test_range(&declared)).is_empty()
    );
    let bare = reverted.replacen(legacy, &format!("{legacy} // {EXEC_MODE_OK}"), 1);
    assert_eq!(
        self_test_exec_mode_violations(APP_MAIN, &bare, self_test_range(&bare)).len(),
        1
    );
    // `mod self_test` の外（製品のコード）は見ない
    let outside = format!("{legacy}\n{main}");
    assert!(
        self_test_exec_mode_violations(APP_MAIN, &outside, self_test_range(&outside)).is_empty()
    );

    // E. run-dropdown-1733 の偽の python: 修正前の `set_permissions(… from_mode(0o755))` を戻す
    let (rel, src) = self_test_sources()
        .into_iter()
        .find(|(rel, _)| rel.ends_with("run_dropdown_1733.rs"))
        .unwrap();
    let lines = src.lines().count();
    assert!(self_test_exec_mode_violations(&rel, &src, 0..lines).is_empty());
    let reverted = format!(
        "{src}fn legacy(python: &std::path::Path) {{\n    use std::os::unix::fs::PermissionsExt;\n    \
         let _ = std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755));\n}}\n"
    );
    assert_eq!(
        self_test_exec_mode_violations(&rel, &reverted, 0..reverted.lines().count()),
        vec![format!("{rel}:{}", lines + 3)]
    );
    // 実行権の無いモード（0o644 / 0o600）とコメント・文字列の中は数えない
    for body in [
        "let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));",
        "let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));",
        "// std::fs::Permissions::from_mode(0o755)",
        "let s = \"from_mode(0o755)\";",
    ] {
        assert!(
            self_test_exec_mode_violations("x.rs", body, 0..1).is_empty(),
            "{body}"
        );
    }
    assert_eq!(
        self_test_exec_mode_violations("x.rs", "opts.mode(0o7_00);", 0..1),
        vec!["x.rs:1".to_string()]
    );
}

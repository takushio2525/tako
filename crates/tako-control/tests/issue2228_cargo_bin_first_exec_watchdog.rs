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
//! 既知の穴: 空打ちを呼ぶ位置（時計より前か）と、写し（`fs::copy(FAKE, …)`）を起こす前の掛け直しは
//! 見ない。どちらも規約で縛る。
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
}

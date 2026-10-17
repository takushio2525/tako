//! ライブラリ側の出力の扱いは、プロセスの入口が規則を採ったときだけ変わる（Issue #1805）
//!
//! `tako_core::stdio` の規則（標準エラーの読み手が閉じていたら行を捨てて続ける）は `tako` /
//! `tako-mcp` の main が採る。GUI（tako-app）は採らないので、GUI のプロセスで走るライブラリの
//! 出力は std の版と同じでなければならない（普段は同じバイト列・読み手が閉じていれば std と
//! 同じく panic）。GUI を起こす代わりに、**規則を採らないプロセス = このテストバイナリの子**から
//! 同じライブラリ関数（gate check = `[gate check] command:` を stderr へ出す）を呼んで確かめる。
//!
//! 子はこのテストバイナリ自身（正規の置き場のまま起こす。#1811）を `--exact` で 1 本だけ走らせ、
//! [`CHILD_ENV`] を見て子の仕事をする。`--nocapture` を付ける（捕捉されると std の版も fd 2 へ
//! 書かず、読み手の閉じたパイプを踏まない）。

use std::io::{PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};

use tako_core::test_residue::ScratchDir;

/// 子として起こされた印（値 = 規則を採るか: `std` / `cli`）
const CHILD_ENV: &str = "TAKO_1805_STDIO_CHILD";
/// 子へ渡すゲートの置き場
const STORE_ENV: &str = "TAKO_1805_STDIO_STORE";
/// 子が最後まで走ったことを親へ知らせる行（stdout）
const DONE: &str = "issue1805-child-done";
const TEST_NAME: &str = "入口が規則を採らなければ_ライブラリの出力はstdの版と同じ";

/// 子へ渡すパイプが兄弟テストの子へ漏れないよう、起こしてから待ち終えるまでを直列にする
/// （#1748。漏れた先が読み手を握っていると EPIPE を踏まないまま緑になる）
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn closed_pipe() -> PipeWriter {
    let (reader, writer) = std::io::pipe().expect("パイプを作れない");
    drop(reader);
    writer
}

/// 子の仕事: 規則を（採るなら採って）gate check を呼び、最後まで走れたら [`DONE`] を出す
fn child(rule: &str) -> ! {
    if rule == "cli" {
        tako_core::stdio::drop_lines_on_closed_stderr();
    }
    let store = PathBuf::from(std::env::var_os(STORE_ENV).expect("置き場が渡っていない"));
    let result = tako_control::acceptance_gates::execute_gate_check_at(&store, "t1805", false)
        .expect("gate check が失敗した");
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{DONE} {}", result["overall"]);
    let _ = out.flush();
    std::process::exit(0);
}

fn spawn_child(rule: &str, store: &Path, stderr: Stdio) -> Output {
    Command::new(std::env::current_exe().expect("自分の場所が分からない"))
        .args([
            TEST_NAME,
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "-q",
        ])
        .env(CHILD_ENV, rule)
        .env(STORE_ENV, store)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .output()
        .expect("子を起こせない")
}

#[test]
fn 入口が規則を採らなければ_ライブラリの出力はstdの版と同じ() {
    if let Ok(rule) = std::env::var(CHILD_ENV) {
        child(&rule);
    }
    let scratch = ScratchDir::new("issue1805-stdio");
    let store = scratch.path().join("gates.yaml");
    tako_control::acceptance_gates::set_gate_payload_at(
        &store,
        "t1805",
        r#"[{"id":"c","kind":{"type":"command","cmd":"true"}}]"#,
        None,
    )
    .expect("ゲートを置けない");

    // 1) GUI と同じ（規則を採らない）・普段の stderr: std の `eprintln!` と同じバイト列
    let normal = {
        let _serial = serial();
        spawn_child("std", &store, Stdio::piped())
    };
    assert!(
        String::from_utf8_lossy(&normal.stdout).contains(DONE),
        "子が最後まで走っていない: {normal:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&normal.stderr),
        "[gate check] command: true\n",
        "規則を採らないプロセスの stderr は std の `eprintln!` と同じはず"
    );

    // 2) GUI と同じ（規則を採らない）・読み手の閉じた stderr: std の版と同じく panic する
    //    （GUI の扱いは #1805 で変えない。変えるならこの表明と `.agent/conventions.md` を直す）
    let cut_std = {
        let _serial = serial();
        spawn_child("std", &store, Stdio::from(closed_pipe()))
    };
    assert!(
        !String::from_utf8_lossy(&cut_std.stdout).contains(DONE),
        "規則を採らないプロセスまで行を捨てて続けている（GUI の出力の扱いが変わった）"
    );
    assert_eq!(
        cut_std.status.code(),
        Some(101),
        "std の版の panic: {cut_std:?}"
    );

    // 3) CLI と同じ（規則を採る）・読み手の閉じた stderr: 行を捨てて最後まで走る
    let cut_cli = {
        let _serial = serial();
        spawn_child("cli", &store, Stdio::from(closed_pipe()))
    };
    assert!(
        String::from_utf8_lossy(&cut_cli.stdout).contains(&format!("{DONE} \"passed\"")),
        "規則を採ったのに最後まで走らない（修正前はここで panic した）: {cut_cli:?}"
    );
    assert_eq!(cut_cli.status.code(), Some(0));
}

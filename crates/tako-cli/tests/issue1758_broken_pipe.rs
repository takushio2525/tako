//! `tako … | head -1` で読み手が先に閉じても panic しない（Issue #1758）
//!
//! 修正前は std の `println!` が EPIPE（Windows は `ERROR_NO_DATA`）で panic し、
//! `failed printing to stdout: Broken pipe` と `メインスレッドが異常終了した` の
//! 2 つの panic メッセージを出して終了コード 101 で終わっていた。直し方と、
//! 標準出力 / 標準エラーで扱いを分けた理由は `src/stdio.rs` の先頭。
//!
//! 読み手を閉じる瞬間が子の書き込みと競らないよう、基本の形は**子を起こす前に
//! 読み手を閉じたパイプ**を渡す（最初の書き込みで必ず EPIPE になる）。`| head -1`
//! そのものの形（1 行読んでから閉じる）は、パイプの容量より大きい出力で確かめる。

use std::io::{BufRead, BufReader, PipeWriter, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};

use tako_core::test_residue::ScratchDir;

/// 子へ渡すパイプが兄弟テストの子へ漏れないよう、起こしてから待ち終えるまでを直列にする
/// （#1748）。漏れた先が読み手を握っていると EPIPE を踏まないまま緑になる
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// 隔離した tako（本番の GUI にも data dir にも触らない）
fn tako(data_dir: &ScratchDir, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
    cmd.args(args)
        .env_remove("TAKO_SOCKET")
        .env_remove("TAKO_PANE_ID")
        .env_remove("TAKO_TOKEN")
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", data_dir.path())
        .stdin(Stdio::null());
    cmd
}

/// 読み手を閉じ済みのパイプの書き手（ここへの最初の書き込みが必ず EPIPE になる）
fn closed_pipe() -> PipeWriter {
    let (reader, writer) = std::io::pipe().expect("パイプを作れない");
    drop(reader);
    writer
}

fn assert_quiet_success(args: &[&str], status: ExitStatus, stderr: &str) {
    let cmd = args.join(" ");
    assert!(
        !stderr.contains("panicked"),
        "`tako {cmd}` が読み手の切断で panic した:\n{stderr}"
    );
    assert_eq!(stderr, "", "`tako {cmd}` は静かに終わるはず");
    assert_eq!(status.code(), Some(0), "`tako {cmd}` の終了コード");
}

#[test]
fn 読み手が先に閉じても_panicせず0で終わる() {
    // 表の出力（行ごとの println!）と JSON の出力（1 回の println! で 100 KB 超）
    for args in [&["platform"][..], &["agent-support", "--json"][..]] {
        let _serial = serial();
        let data_dir = ScratchDir::new("issue1758");
        let out = tako(&data_dir, args)
            .stdout(closed_pipe())
            .stderr(Stdio::piped())
            .output()
            .expect("tako を起こせない");
        assert_quiet_success(args, out.status, &String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn head_1_と同じく1行だけ読んで閉じても_panicしない() {
    let args = ["agent-support", "--json"];
    let _serial = serial();
    let data_dir = ScratchDir::new("issue1758");

    // 前提: 出力が「パイプの容量（macOS / Linux は最大 64 KiB。Windows はもっと小さい）+
    // 最初の 1 回の読み（BufReader の 8 KiB）」を超える。超えないと読み手が閉じる前に
    // 書き終わり、EPIPE を踏まないまま緑になる
    let full = tako(&data_dir, &args).output().expect("tako を起こせない");
    assert!(full.status.success(), "前提の実行が失敗した: {full:?}");
    assert!(
        full.stdout.len() > 80 * 1024,
        "出力が小さすぎて場面が成立しない（{} バイト）。もっと大きい出力のサブコマンドへ替える",
        full.stdout.len()
    );

    let mut child = tako(&data_dir, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("tako を起こせない");
    let mut first = String::new();
    // この文の終わりで BufReader ごと読み手が閉じる = `head -1` が 1 行出して終わった瞬間
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut first)
        .expect("1 行目を読めない");
    assert_eq!(first.trim_end(), "{", "JSON の 1 行目");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("stderr を読めない");
    let status = child.wait().expect("tako を待てない");
    assert_quiet_success(&args, status, &stderr);
}

/// `tako … 2>&1 | head -1` の形: 標準出力と標準エラーが同じ閉じたパイプ。
/// 標準エラーの切断で打ち切ると、失敗（`error: …` の後の 1）が 0 に化ける
#[test]
fn 標準エラーも閉じていても_終了コードはそのまま() {
    for (args, code) in [
        (&["platform"][..], 0),
        (&["task", "gate", "show", "no-such-task-1758"][..], 1),
    ] {
        let _serial = serial();
        let data_dir = ScratchDir::new("issue1758");
        let writer = closed_pipe();
        let status = tako(&data_dir, args)
            .stdout(writer.try_clone().expect("書き手を複製できない"))
            .stderr(writer)
            .status()
            .expect("tako を起こせない");
        // stderr も閉じているので panic のメッセージは読めない。修正前は 101 で見分けが付く
        assert_eq!(
            status.code(),
            Some(code),
            "`tako {}` の終了コード",
            args.join(" ")
        );
    }
}

/// 差し替えのマクロは宣言より後ろにしか効かない。`mod stdio;` が他の `mod` より
/// 後ろへ動くと、それより前に宣言したモジュール（`setup.rs`）は黙って std の版へ戻る
#[test]
fn 出力マクロの宣言が他のmodより先にある() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs");
    let src = std::fs::read_to_string(path).expect("src/main.rs を読めない");
    let lines: Vec<&str> = src.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.starts_with("mod "))
        .expect("src/main.rs に mod 宣言が無い");
    assert_eq!(
        lines[first],
        "mod stdio;",
        "src/main.rs:{} が最初の mod 宣言。`#[macro_use] mod stdio;` を先頭へ戻す",
        first + 1
    );
    assert_eq!(
        lines.get(first.wrapping_sub(1)).copied(),
        Some("#[macro_use]"),
        "src/main.rs:{} の `mod stdio;` に `#[macro_use]` が無い（マクロが外へ出ない）",
        first + 1
    );
}

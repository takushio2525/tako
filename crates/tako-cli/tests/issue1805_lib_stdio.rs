//! CLI の経路で走るライブラリ側（tako-core / tako-control）の出力も、読み手の閉じたパイプで
//! panic しない / `tako mcp serve` はクライアントの切断で静かに 0 で終わる（Issue #1805）
//!
//! #1758 は tako-cli の `println!` / `eprintln!` だけを直したので、`tako task gate check` の
//! `[gate check] command:`（`tako_control::acceptance_gates`）は読み手の閉じた stderr で
//! panic したままだった（修正前の実測: 終了コード 101・stdout に結果が 1 バイトも出ない）。
//! 同じ panic は `tako mcp serve` の `tools/call tako_task_gate_check` でも起き、ブリッジが
//! 応答を返さずに 101 で落ちていた。直し方は `tako_core::stdio` の先頭。
//!
//! 読み手を閉じる瞬間が子の書き込みと競らないよう、**子を起こす前に読み手を閉じたパイプ**を
//! 渡す（最初の書き込みで必ず EPIPE になる。#1758 の統合テストと同じ形）。

use std::io::{PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};

use tako_core::test_residue::ScratchDir;

/// 子へ渡すパイプが兄弟テストの子へ漏れないよう、起こしてから待ち終えるまでを直列にする
/// （#1748）。漏れた先が読み手を握っていると EPIPE を踏まないまま緑になる
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// 隔離した実行ファイル（本番の GUI にも data dir にも触らない）
fn isolated(exe: &str, data_dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_remove("TAKO_SOCKET")
        .env_remove("TAKO_PANE_ID")
        .env_remove("TAKO_TOKEN")
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", data_dir);
    cmd
}

fn tako(data_dir: &Path, args: &[&str]) -> Command {
    let mut cmd = isolated(env!("CARGO_BIN_EXE_tako"), data_dir, args);
    cmd.stdin(Stdio::null());
    cmd
}

/// 読み手を閉じ済みのパイプの書き手（ここへの最初の書き込みが必ず EPIPE になる）
fn closed_pipe() -> PipeWriter {
    let (reader, writer) = std::io::pipe().expect("パイプを作れない");
    drop(reader);
    writer
}

/// cwd 付きのコマンド型ゲートを置く（check が `[gate check] command:` と `cwd:` の 2 行を
/// stderr へ出す）
fn gate_with_command(data_dir: &ScratchDir) {
    let cwd = data_dir.path().to_string_lossy().into_owned();
    let out = tako(
        data_dir.path(),
        &[
            "task",
            "gate",
            "set",
            "t1805",
            "--command",
            "true",
            "--cwd",
            &cwd,
        ],
    )
    .output()
    .expect("tako を起こせない");
    assert!(out.status.success(), "gate set が失敗した: {out:?}");
}

#[test]
fn ライブラリ側のeprintlnは_標準エラーが閉じていても行を捨てて続ける() {
    let _serial = serial();
    let data_dir = ScratchDir::new("issue1805");
    gate_with_command(&data_dir);
    let args = ["task", "gate", "check", "t1805", "--no-sync"];

    // 前提: 普段はライブラリ側（acceptance_gates::execute_command）が stderr へ 2 行出す
    let normal = tako(data_dir.path(), &args)
        .output()
        .expect("tako を起こせない");
    let normal_err = String::from_utf8_lossy(&normal.stderr);
    assert!(
        normal_err.contains("[gate check] command: true")
            && normal_err.contains("[gate check] cwd:"),
        "前提の stderr にライブラリ側の行が無い（場面が成立しない）:\n{normal_err}"
    );
    assert_eq!(normal.status.code(), Some(0), "前提の実行: {normal:?}");

    let cut = tako(data_dir.path(), &args)
        .stdout(Stdio::piped())
        .stderr(closed_pipe())
        .output()
        .expect("tako を起こせない");
    // 修正前は 101 で、stdout は空（結果を出す前に panic した）
    assert_eq!(
        cut.status.code(),
        Some(0),
        "stderr の切断で終了コードが変わった（101 = ライブラリ側の eprintln! の panic）"
    );
    assert_eq!(
        String::from_utf8_lossy(&cut.stdout),
        String::from_utf8_lossy(&normal.stdout),
        "stderr の行を捨てた後も、結果は普段と同じだけ stdout へ出るはず"
    );
}

/// `tako … 2>&1 | head -1` の形: 標準出力と標準エラーが同じ閉じたパイプ
#[test]
fn ライブラリ側の出力を通るコマンドも_両方閉じていれば静かに0で終わる() {
    let _serial = serial();
    let data_dir = ScratchDir::new("issue1805");
    gate_with_command(&data_dir);
    let writer = closed_pipe();
    let status = tako(
        data_dir.path(),
        &["task", "gate", "check", "t1805", "--no-sync"],
    )
    .stdout(writer.try_clone().expect("書き手を複製できない"))
    .stderr(writer)
    .status()
    .expect("tako を起こせない");
    assert_eq!(status.code(), Some(0), "修正前は 101");
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"issue1805","version":"0"}}}"#;
const GATE_CHECK: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_task_gate_check","arguments":{"task_id":"t1805","sync_checkpoint":false}}}"#;

/// MCP の stdio ブリッジの 2 つの入口（`tako mcp serve` と、.app で引き渡される `tako-mcp`）
fn bridges() -> [(&'static str, PathBuf); 2] {
    [
        ("tako mcp serve", PathBuf::from(env!("CARGO_BIN_EXE_tako"))),
        ("tako-mcp", PathBuf::from(env!("CARGO_BIN_EXE_tako-mcp"))),
    ]
}

/// ブリッジへ `lines` を流して stdin を閉じ、終わるまで待つ
fn run_bridge(exe: &Path, data_dir: &Path, lines: &[&str], stdout: Stdio, stderr: Stdio) -> Output {
    let exe = exe.to_str().expect("実行ファイルのパスが UTF-8 でない");
    let mut child = isolated(exe, data_dir, &["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("ブリッジを起こせない");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for line in lines {
            // ブリッジが先に終わっていたら書けない（それも結果として下で見る）
            let _ = writeln!(stdin, "{line}");
        }
    }
    child.wait_with_output().expect("ブリッジを待てない")
}

/// 終わり方を 1 つに決める（#1805）: クライアントが stdout の読み手を閉じた = 切断で、
/// tako の失敗ではない。stdin の EOF と同じく静かに 0 で終える。
/// 修正前は `error: stdout への書き込みに失敗: Broken pipe (os error 32)` を出して 1 だった
#[test]
fn mcp_serve_はクライアントがstdoutを閉じたら静かに0で終わる() {
    for (name, exe) in bridges() {
        let _serial = serial();
        let data_dir = ScratchDir::new("issue1805");
        let out = run_bridge(
            &exe,
            data_dir.path(),
            &[INIT],
            Stdio::from(closed_pipe()),
            Stdio::piped(),
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr, "", "`{name}` は切断で何も出さないはず");
        assert_eq!(
            out.status.code(),
            Some(0),
            "`{name}` の終了コード（修正前は 1）"
        );
        // 痕跡は persist.log へ 1 行（stderr の読み手も居ないことが多い）
        let log = std::fs::read_to_string(data_dir.path().join("persist.log")).unwrap_or_default();
        assert!(
            log.contains("mcp serve: クライアントが stdout を閉じたので終わる"),
            "`{name}` が persist.log に切断の痕跡を残していない:\n{log}"
        );
    }
}

/// stdin の EOF で終わる普段の形は変えない（0・stderr はライブラリ側の行だけ）
#[test]
fn mcp_serve_はstdinのeofで0のまま() {
    for (name, exe) in bridges() {
        let _serial = serial();
        let data_dir = ScratchDir::new("issue1805");
        let out = run_bridge(
            &exe,
            data_dir.path(),
            &[INIT],
            Stdio::piped(),
            Stdio::piped(),
        );
        assert_eq!(out.status.code(), Some(0), "`{name}`: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "",
            "`{name}` の stderr"
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(r#""id":1"#),
            "`{name}` が initialize に応答していない"
        );
    }
}

/// ブリッジのプロセスで走るライブラリ側の `eprintln!`（gate check）も、MCP クライアントが
/// stderr を閉じていて panic しない。修正前はここで 101 で落ち、id 2 の応答が返らなかった
#[test]
fn mcp_serve_のgate_checkは_stderrが閉じていても応答を返す() {
    for (name, exe) in bridges() {
        let _serial = serial();
        let data_dir = ScratchDir::new("issue1805");
        gate_with_command(&data_dir);
        let out = run_bridge(
            &exe,
            data_dir.path(),
            &[INIT, GATE_CHECK],
            Stdio::piped(),
            Stdio::from(closed_pipe()),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(0),
            "`{name}` の終了コード（修正前は 101）"
        );
        let responses: Vec<serde_json::Value> = stdout
            .lines()
            .map(|l| serde_json::from_str(l).expect("応答が JSON でない"))
            .collect();
        let gate = responses
            .iter()
            .find(|r| r["id"] == 2)
            .unwrap_or_else(|| panic!("`{name}` が gate check に応答していない:\n{stdout}"));
        let text = gate["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains(r#""overall": "passed""#),
            "`{name}` の gate check の結果:\n{gate}"
        );
    }
}

/// 差し替えのマクロが効かない書き方を、ライブラリ側（CLI の経路で走るクレート）へ戻させない。
///
/// - `std::eprintln!` のように std の版を名指すと、`#[macro_use] mod stdio;` を迂回して
///   読み手の閉じたパイプで panic する版に戻る（実体の `tako_core::stdio` だけが例外）
/// - `src/bin/` の実行ファイルの根は lib の `#[macro_use]` が届かない別のクレートなので、
///   素の `eprintln!` 自体が std の版になる（`tako-mcp` は `writeln!` で書き、失敗を捨てる）
///
/// tako-app（GUI）は対象外: 規則を採らないので出力の扱いは std の版と同じ（#1805）。
/// `crates/tako-control/src/bin/tako-lsp-fake.rs` もテスト用の偽の言語サーバで CLI の経路に
/// 無いので対象外（stdout は LSP の通り道）
#[test]
fn ライブラリ側でstdの出力マクロを名指さない() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let implementation = Path::new("tako-core/src/stdio.rs");
    let mut offenders = Vec::new();
    for krate in ["tako-core", "tako-control", "tako-cli"] {
        for file in rust_files(&root.join(krate).join("src")) {
            let rel = file
                .strip_prefix(&root)
                .expect("crates/ の外")
                .to_path_buf();
            if rel == implementation || rel == Path::new("tako-control/src/bin/tako-lsp-fake.rs") {
                continue;
            }
            let in_bin = rel.components().any(|c| c.as_os_str() == "bin");
            let src = std::fs::read_to_string(&file).expect("読めない");
            for (i, line) in src.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                let named_std = ["std::print", "std::eprint"]
                    .iter()
                    .any(|p| code.contains(&format!("{p}!")) || code.contains(&format!("{p}ln!")));
                let bare_in_bin = in_bin && has_bare_print_macro(code);
                if named_std || bare_in_bin {
                    offenders.push(format!("crates/{}:{}: {}", rel.display(), i + 1, code));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "std の出力マクロ（読み手の閉じたパイプで panic する版）が CLI の経路へ戻っている。\
         素の `eprintln!` 等（`#[macro_use] mod stdio;` の版）で書く（#1805）:\n{}",
        offenders.join("\n")
    );
}

/// 識別子の一部ではない `println!` / `print!` / `eprintln!` / `eprint!`
fn has_bare_print_macro(code: &str) -> bool {
    ["println!", "print!", "eprintln!", "eprint!"]
        .iter()
        .any(|m| {
            code.match_indices(m).any(|(at, _)| {
                !code[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
            })
        })
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("dir を読めない").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

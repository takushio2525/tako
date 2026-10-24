//! #2021: MCP の stdio ブリッジを軽くしても、**応答のバイト列と挙動は変わらない**。
//!
//! 軽くした形（clap を通らない早い分岐・カタログを使い捨ての子プロセス = カタログ係に
//! 組ませる・macOS の .app では `tako-mcp` へ exec で引き渡す）と、同じバイナリの旧経路
//! （`TAKO_2021_LEGACY=1` = clap を通り、カタログを常駐プロセスで組む）へ同じ MCP の
//! メッセージ列を流し、stdout をバイト単位で突き合わせる。
//!
//! - tako の中（接続情報あり）: initialize・tools/list（約 200 KB）・未知パラメータの拒否・
//!   GUI が居ないときの tools/call・ping・未対応のメソッド・壊れた JSON
//! - tako の外（接続情報なし）: 0 ツールのまま（FR-2.3.2）
//! - macOS: `tako-mcp` が libSystem だけをリンクする（`build.rs` の `-dead_strip_dylibs`）/
//!   .app の中の `tako mcp serve` が `tako-mcp` へ引き渡される / 隣に無ければ引き渡さない
//! - unix: カタログ係を起こせない（実行ファイルが消えた）ときは同じプロセスで組んで同じ答え
//!
//! 本番の data dir・GUI へ届かないよう、接続情報は存在しないソケットと一時の data dir を渡す

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const TAKO: &str = env!("CARGO_BIN_EXE_tako");
const TAKO_MCP: &str = env!("CARGO_BIN_EXE_tako-mcp");

/// テストごとの一時 dir（終わったら消す）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tk2021-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作れる");
        Self(dir)
    }

    /// ブリッジが前段の断り（未知のパラメータ等）を組む表示言語を settings.json に書く（#2365）。
    /// 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
    fn language(&self, lang: &str) -> &Self {
        let data = self.0.join("data");
        std::fs::create_dir_all(&data).expect("data dir を作れる");
        std::fs::write(
            data.join("settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        self
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 接続情報の形
#[derive(Clone, Copy)]
enum Env {
    /// tako の中（ソケットは存在しない = GUI が落ちている間の呼び出し）
    Inside,
    /// tako の外（`TAKO_SOCKET` / `TAKO_TOKEN` なし）
    Outside,
}

/// MCP のメッセージ列を流して stdout をそのまま返す
fn session(bin: &Path, args: &[&str], env: Env, legacy: bool, scratch: &Scratch) -> Vec<u8> {
    let mut command = Command::new(bin);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TAKO_") {
            command.env_remove(key);
        }
    }
    command
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", scratch.0.join("data"));
    if let Env::Inside = env {
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\tako-2021-none-{}", std::process::id())
        } else {
            scratch.0.join("none.sock").display().to_string()
        };
        command
            .env("TAKO_SOCKET", socket)
            .env("TAKO_TOKEN", "t2021")
            .env("TAKO_PANE_ID", "7");
    }
    if legacy {
        command.env("TAKO_2021_LEGACY", "1");
    }
    let mut child = command.spawn().expect("ブリッジを起動できる");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for line in MESSAGES {
            writeln!(stdin, "{line}").expect("stdin へ書ける");
        }
    }
    let out = child.wait_with_output().expect("ブリッジが終わる");
    assert!(
        out.status.success(),
        "{} {args:?}: 終了コードが 0 でない（{}）",
        bin.display(),
        out.status
    );
    out.stdout
}

/// 流すメッセージ列（Claude Code が実際に送る順 + 境界）
const MESSAGES: &[&str] = &[
    r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2021","version":"0"}}}"#,
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    // id の無い tools/list は応答しない（カタログ係も起こさない）
    r#"{"jsonrpc":"2.0","method":"tools/list"}"#,
    // 未知パラメータ（#227）: 許可パラメータの表で拒否する
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_split_pane","arguments":{"pane":1,"bogus":true}}}"#,
    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tako_list_panes","arguments":{"x":1}}}"#,
    // GUI が居ない（ソケットが無い）ときの中継
    r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tako_list_panes","arguments":{}}}"#,
    r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tako_no_such_tool","arguments":{}}}"#,
    r#"{"jsonrpc":"2.0","id":"s","method":"ping"}"#,
    r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
    r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"cursor":"x"}}"#,
    "not json",
];

fn lines(bytes: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).expect("応答は 1 行 1 JSON"))
        .collect()
}

/// 旧経路と同じバイト列か（違えば最初に食い違う行を出す）
fn assert_same_bytes(label: &str, legacy: &[u8], now: &[u8]) {
    if legacy == now {
        return;
    }
    let old = String::from_utf8_lossy(legacy);
    let new = String::from_utf8_lossy(now);
    let at = old
        .lines()
        .zip(new.lines())
        .position(|(a, b)| a != b)
        .unwrap_or(old.lines().count().min(new.lines().count()));
    let clip = |s: Option<&str>| s.map(|s| s.chars().take(300).collect::<String>());
    panic!(
        "{label}: 旧経路と応答のバイト列が違う（{} 行目。旧 {} バイト / 新 {} バイト）\n旧: {:?}\n新: {:?}",
        at + 1,
        legacy.len(),
        now.len(),
        clip(old.lines().nth(at)),
        clip(new.lines().nth(at)),
    );
}

#[test]
fn tako_の中の応答は旧経路とバイト単位で同じ() {
    let scratch = Scratch::new("inside");
    scratch.language("ja");
    let tako = Path::new(TAKO);
    let legacy = session(tako, &["mcp", "serve"], Env::Inside, true, &scratch);

    // 応答の中身が想定どおりか（比べる元が壊れていないこと）
    let replies = lines(&legacy);
    assert_eq!(replies.len(), 10, "応答の数（notification 2 つは返さない）");
    let tools = replies[1]["result"]["tools"].as_array().expect("tools");
    assert!(tools.len() > 100, "tako の中ではカタログ全体を返す");
    assert!(
        replies[2]["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("未知のパラメータ: bogus")),
        "{}",
        replies[2]
    );
    assert!(
        replies[3]["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("引数なし")),
        "{}",
        replies[3]
    );
    assert_eq!(replies[4]["result"]["isError"], true, "{}", replies[4]);

    // 軽くした形（dev の置き場の `tako mcp serve` = 引き渡さずカタログ係を使う / `tako-mcp`）
    let now = session(tako, &["mcp", "serve"], Env::Inside, false, &scratch);
    assert_same_bytes("tako mcp serve", &legacy, &now);
    let small = session(Path::new(TAKO_MCP), &[], Env::Inside, false, &scratch);
    assert_same_bytes("tako-mcp", &legacy, &small);
    let small_args = session(
        Path::new(TAKO_MCP),
        &["mcp", "serve"],
        Env::Inside,
        false,
        &scratch,
    );
    assert_same_bytes("tako-mcp mcp serve", &legacy, &small_args);
}

/// #2365: 前段の断り（未知のパラメータ・不明なツール）はブリッジ自身が組むので、呼び出し元
/// ペインが決まっていても（`TAKO_PANE_ID`）settings.json の表示言語で返る。#2358 までの
/// ブリッジは呼び出し元が決まると表示言語を決めず、静的既定のまま組んでいた
#[test]
fn 呼び出し元が決まっていても前段の断りは設定の表示言語() {
    for (lang, unknown_param, no_args, unknown_tool) in [
        (
            "en",
            "unknown parameter: bogus",
            "takes no arguments",
            "unknown tool: tako_no_such_tool",
        ),
        (
            "ja",
            "未知のパラメータ: bogus",
            "引数なし",
            "不明なツール: tako_no_such_tool",
        ),
    ] {
        let scratch = Scratch::new(&format!("lang-{lang}"));
        scratch.language(lang);
        let replies = lines(&session(
            Path::new(TAKO_MCP),
            &[],
            Env::Inside,
            false,
            &scratch,
        ));
        let message = |i: usize| {
            replies[i]["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        assert!(message(2).contains(unknown_param), "{lang}: {}", replies[2]);
        assert!(message(3).contains(no_args), "{lang}: {}", replies[3]);
        assert!(message(5).contains(unknown_tool), "{lang}: {}", replies[5]);
    }
}

#[test]
fn tako_の外では_0_ツールのまま() {
    let scratch = Scratch::new("outside");
    let tako = Path::new(TAKO);
    let legacy = session(tako, &["mcp", "serve"], Env::Outside, true, &scratch);
    let replies = lines(&legacy);
    assert_eq!(replies[1]["result"]["tools"], serde_json::json!([]));
    assert!(
        replies[0]["result"]["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("tako アプリの外")),
        "{}",
        replies[0]
    );
    for (label, bin, args) in [
        ("tako mcp serve", TAKO, &["mcp", "serve"][..]),
        ("tako-mcp", TAKO_MCP, &[][..]),
    ] {
        let now = session(Path::new(bin), args, Env::Outside, false, &scratch);
        assert_same_bytes(label, &legacy, &now);
    }
}

#[test]
fn tako_mcp_は余計な引数を断る() {
    let out = Command::new(TAKO_MCP)
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .expect("起動できる");
    assert_eq!(out.status.code(), Some(2));
}

/// カタログ係（= 自分の実行ファイル）を起こせないときは同じプロセスで組み、答えは変わらない
#[cfg(unix)]
#[test]
fn カタログ係を起こせなくても同じ答え() {
    let scratch = Scratch::new("nohelper");
    let legacy = session(
        Path::new(TAKO),
        &["mcp", "serve"],
        Env::Inside,
        true,
        &scratch,
    );
    // 一時の置き場へ写した tako-mcp を起動してから消す（実行中のファイルは unix なら消せる）
    let bin = scratch.0.join("tako-mcp");
    place(Path::new(TAKO_MCP), &bin);
    let mut command = Command::new(&bin);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TAKO_") {
            command.env_remove(key);
        }
    }
    command
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", scratch.0.join("data"))
        .env("TAKO_SOCKET", scratch.0.join("none.sock"))
        .env("TAKO_TOKEN", "t2021")
        .env("TAKO_PANE_ID", "7");
    let mut child = command.spawn().expect("起動できる");
    let mut stdin = child.stdin.take().expect("stdin");
    // 1 行目への応答を待ってから消す（起動途中で消すと exec 自体が失敗し得る）
    writeln!(stdin, "{}", MESSAGES[0]).expect("書ける");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut first = String::new();
    std::io::BufRead::read_line(&mut stdout, &mut first).expect("initialize の応答");
    std::fs::remove_file(&bin).expect("消せる");
    for line in &MESSAGES[1..] {
        writeln!(stdin, "{line}").expect("書ける");
    }
    drop(stdin);
    let mut rest = Vec::new();
    std::io::Read::read_to_end(&mut stdout, &mut rest).expect("読める");
    assert!(child.wait().expect("終わる").success());
    let mut now = first.into_bytes();
    now.extend_from_slice(&rest);
    assert_same_bytes("カタログ係なし", &legacy, &now);
    // 倒れたことは persist.log に残る（診断の手掛かり）
    let log =
        std::fs::read_to_string(scratch.0.join("data").join("persist.log")).unwrap_or_default();
    assert!(log.contains("カタログ係"), "persist.log:\n{log}");
}

/// macOS の配布物の形: `tako-mcp` は libSystem だけをリンクする（像 約 350 個 → 約 50 個）。
///
/// **release（LTO）でだけ成り立つ**: debug は使わない関数が LLVM で消えないまま目的ファイルに
/// 残り、その中のフレームワークの記号の参照でリンカが dylib を「使った」と数える（dead strip は
/// その後）。配布物の .app は release なので、`scripts/build-app.sh` も同じ検査で止まる
/// （`cargo test --release -p tako-cli --test issue2021_mcp_bridge` で走る）
#[cfg(all(target_os = "macos", not(debug_assertions)))]
#[test]
fn tako_mcp_は_libsystem_だけをリンクする() {
    let out = Command::new("otool")
        .args(["-L", TAKO_MCP])
        .output()
        .expect("otool を起動できる");
    let text = String::from_utf8_lossy(&out.stdout);
    let libs: Vec<&str> = text
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        libs,
        vec!["/usr/lib/libSystem.B.dylib"],
        "tako-mcp が libSystem 以外をリンクしている（ブリッジの到達するコードが\
         フレームワークを使い始めたか、build.rs の -dead_strip_dylibs が外れた）:\n{text}"
    );
}

/// macOS の .app の中の `tako mcp serve` は `tako-mcp` へ exec で引き渡され（pid は同じ）、
/// 隣に無ければ引き渡さずに同じ答えを返す
#[cfg(target_os = "macos")]
#[test]
fn app_の中では_tako_mcp_へ引き渡す() {
    let scratch = Scratch::new("bundle");
    let legacy = session(
        Path::new(TAKO),
        &["mcp", "serve"],
        Env::Inside,
        true,
        &scratch,
    );
    let macos = scratch.0.join("tako.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("作れる");
    let tako = macos.join("tako");
    place(Path::new(TAKO), &tako);

    // 隣に無い: 引き渡さない（tako のまま）
    assert_eq!(image_after_initialize(&tako, &scratch), "tako");
    let alone = session(&tako, &["mcp", "serve"], Env::Inside, false, &scratch);
    assert_same_bytes(".app（tako-mcp 無し）", &legacy, &alone);

    // 隣に在る: 引き渡す
    place(Path::new(TAKO_MCP), &macos.join("tako-mcp"));
    assert_eq!(image_after_initialize(&tako, &scratch), "tako-mcp");
    let handed = session(&tako, &["mcp", "serve"], Env::Inside, false, &scratch);
    assert_same_bytes(".app（tako-mcp 在り）", &legacy, &handed);

    // A/B: 旧経路は引き渡さない
    let mut command = bridge_command(&tako, &scratch);
    command.env("TAKO_2021_LEGACY", "1");
    assert_eq!(image_of(command), "tako");
}

#[cfg(target_os = "macos")]
fn bridge_command(bin: &Path, scratch: &Scratch) -> Command {
    let mut command = Command::new(bin);
    command
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TAKO_") {
            command.env_remove(key);
        }
    }
    command
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", scratch.0.join("data"));
    command
}

#[cfg(target_os = "macos")]
fn image_after_initialize(bin: &Path, scratch: &Scratch) -> String {
    image_of(bridge_command(bin, scratch))
}

/// initialize の応答が返った時点で、そのプロセスがどの実行ファイルの像か（`ps -o comm=`）
#[cfg(target_os = "macos")]
fn image_of(mut command: Command) -> String {
    let mut child = command.spawn().expect("起動できる");
    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", MESSAGES[0]).expect("書ける");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut first = String::new();
    std::io::BufRead::read_line(&mut stdout, &mut first).expect("initialize の応答");
    assert!(first.contains("\"protocolVersion\""), "{first}");
    let out = Command::new("ps")
        .args(["-o", "comm=", "-p", &child.id().to_string()])
        .output()
        .expect("ps を起動できる");
    drop(stdin);
    let _ = child.wait();
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&comm)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or(comm)
}

/// 実行ファイルを一時の置き場へ置く（hard link。同じボリュームでなければ写す）。
/// 消しても元のビルド成果物は残る
#[cfg(unix)]
fn place(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).expect("写せる");
    }
}

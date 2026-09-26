//! テスト専用の偽 LSP サーバ（#1678。stdio のモック）
//!
//! 配布物には入らない（`scripts/build-app.sh` / Windows インストーラは `tako-app` と
//! `tako` だけを名指しで同梱する）。テストからは `env!("CARGO_BIN_EXE_tako-lsp-fake")` で
//! 絶対パスを取る（Cargo 標準。Windows でも同じ形）。
//!
//! 受け取ったメッセージを 1 行 1 JSON で `--log` へ追記し、起動のたびに自分の pid を
//! `--spawns` へ追記する。フレーミングは**tako 側の実装を使わず自前で書く**
//! （tako の読み書きに穴があっても偽サーバが同じ穴で辻褄を合わせないように）。
//!
//! シナリオ（`--scenario` か `TAKO_LSP_FAKE_SCENARIO`）:
//!
//! | 名前 | 振る舞い |
//! |---|---|
//! | `normal` | 握手して黙る。didOpen を受けたら診断を 1 件 publish する |
//! | `slow` | initialize に応答しない |
//! | `crash` | initialized を受けたら自分で落ちる |
//! | `split-header` | 応答の `Content-Length` を 2 回に分けて書く |
//! | `garbage` | initialize の応答の前に壊れた JSON を 1 つ混ぜる |
//! | `chatty` | initialized の後に通知を大量に投げる |
//! | `ask` | initialized の後に `workspace/configuration` を問い合わせる |
//! | `die` | 起動直後に即死する（initialize を読まない） |

use std::io::{BufRead, BufReader, Write};

fn arg_or_env(args: &[String], flag: &str, env: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var(env).ok().filter(|v| !v.is_empty()))
}

fn append(path: &Option<String>, line: &str) {
    if let Some(path) = path {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn read_one(input: &mut impl BufRead) -> Option<serde_json::Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

struct Out {
    split: bool,
}

impl Out {
    fn send(&self, message: serde_json::Value) {
        let body = message.to_string();
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdout = std::io::stdout().lock();
        if self.split {
            // ヘッダを 2 回に分けて書く（間を空けて、読み手の 1 回の read に乗らないように）
            let (a, b) = header.split_at(9);
            let _ = stdout.write_all(a.as_bytes());
            let _ = stdout.flush();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = stdout.write_all(b.as_bytes());
        } else {
            let _ = stdout.write_all(header.as_bytes());
        }
        let _ = stdout.write_all(body.as_bytes());
        let _ = stdout.flush();
    }

    fn raw(&self, bytes: &[u8]) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(bytes);
        let _ = stdout.flush();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = arg_or_env(&args, "--scenario", "TAKO_LSP_FAKE_SCENARIO")
        .unwrap_or_else(|| "normal".into());
    let log = arg_or_env(&args, "--log", "TAKO_LSP_FAKE_LOG");
    let spawns = arg_or_env(&args, "--spawns", "TAKO_LSP_FAKE_SPAWNS");
    append(&spawns, &std::process::id().to_string());
    eprintln!("tako-lsp-fake: scenario={scenario}");
    if scenario == "die" {
        // 起動直後に即死する（initialize を 1 通も読まない）
        std::process::exit(2);
    }

    let out = Out {
        split: scenario == "split-header",
    };
    let mut input = BufReader::new(std::io::stdin());
    while let Some(message) = read_one(&mut input) {
        append(&log, &message.to_string());
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = message.get("id").cloned();
        match (method, id) {
            ("initialize", Some(id)) => {
                if scenario == "slow" {
                    continue;
                }
                if scenario == "garbage" {
                    out.raw(b"Content-Length: 5\r\n\r\n{nope");
                }
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": {
                            "positionEncoding": "utf-16",
                            "textDocumentSync": { "openClose": true, "change": 2 },
                        },
                        "serverInfo": { "name": "tako-lsp-fake", "version": "1" },
                    },
                }));
            }
            ("initialized", None) => match scenario.as_str() {
                "crash" => std::process::exit(3),
                "chatty" => {
                    for n in 0..5000 {
                        out.send(serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "window/logMessage",
                            "params": { "type": 4, "message": format!("n={n}") },
                        }));
                    }
                }
                "ask" => out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "cfg-1",
                    "method": "workspace/configuration",
                    "params": { "items": [{ "section": "a" }, { "section": "b" }] },
                })),
                _ => {}
            },
            ("textDocument/didOpen", None) => {
                let uri = message["params"]["textDocument"]["uri"].clone();
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": uri,
                        "diagnostics": [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 0, "character": 1 },
                            },
                            "severity": 2,
                            "message": "fake",
                        }],
                    },
                }));
            }
            ("shutdown", Some(id)) => out.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null,
            })),
            ("exit", None) => std::process::exit(0),
            _ => {}
        }
    }
    // stdin が閉じた（親が落ちた）ら自分で終わる = 孤児にならない
}

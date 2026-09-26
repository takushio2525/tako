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
//! | `no-goto` | 定義ジャンプの能力（`definitionProvider` 等）を申告しない（#1680） |
//! | `loading` | initialized の後 `experimental/serverStatus`（`quiescent: false`）を送り、読み込み（既定 0.8 秒。`--loading-ms` / `TAKO_LSP_FAKE_LOADING_MS`）が済むまで定義ジャンプに空（`[]`）で答え、済んだら `quiescent: true` を送る（rust-analyzer の振る舞い。#1680） |
//! | `full-sync` | `normal` と同じだが全文同期（`change: 1`）を申告する |
//!
//! `--diagnostics <file>` か `TAKO_LSP_FAKE_DIAGNOSTICS`（LSP の `Diagnostic` の JSON 配列）を
//! 渡すと、didOpen と didChange のたびに**その配列をそのまま** publish する（#1679。
//! 固定の配列と tako 側の応答を突き合わせるため。didChange の回は文書の版も付ける）。
//! 渡さなければ didOpen で 1 件だけ publish する（#1678 の既定）。
//!
//! 定義ジャンプの 4 種（`textDocument/definition` / `declaration` / `typeDefinition` /
//! `implementation`。#1680）は `--goto <file>` か `TAKO_LSP_FAKE_GOTO` の規則で答える。
//! 規則は JSON の配列で、**上から順に最初に当たった 1 つ**を使う:
//!
//! ```json
//! [{ "method": "textDocument/definition", "uri_suffix": "main.rs", "line": 3,
//!    "result": { "uri": "file:///…/other.rs", "range": { … } } },
//!  { "line": 9, "silent": true },
//!  { "line": 11, "crash": true }]
//! ```
//!
//! `method` / `uri_suffix` / `line`（要求の 0 起点の行）は省けば何にでも当たる。
//! `silent` は答えない（未応答の検査）、`crash` は自分で落ちる（途中で落ちたときの検査）。
//! 当たる規則が無ければ `null`（= 見つからない）で答える。

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

/// 定義ジャンプの 4 種の method（#1680）
const GOTO_METHODS: [&str; 4] = [
    "textDocument/definition",
    "textDocument/declaration",
    "textDocument/typeDefinition",
    "textDocument/implementation",
];

/// 定義ジャンプの 4 種の能力のキー
const GOTO_PROVIDERS: [&str; 4] = [
    "definitionProvider",
    "declarationProvider",
    "typeDefinitionProvider",
    "implementationProvider",
];

/// 規則のうち、この要求に最初に当たるもの
fn goto_rule<'a>(
    rules: &'a [serde_json::Value],
    method: &str,
    params: &serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64();
    rules.iter().find(|rule| {
        rule["method"].as_str().is_none_or(|m| m == method)
            && rule["uri_suffix"].as_str().is_none_or(|s| uri.ends_with(s))
            && rule["line"].as_u64().is_none_or(|l| Some(l) == line)
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = arg_or_env(&args, "--scenario", "TAKO_LSP_FAKE_SCENARIO")
        .unwrap_or_else(|| "normal".into());
    let log = arg_or_env(&args, "--log", "TAKO_LSP_FAKE_LOG");
    let spawns = arg_or_env(&args, "--spawns", "TAKO_LSP_FAKE_SPAWNS");
    let fixed: Option<serde_json::Value> =
        arg_or_env(&args, "--diagnostics", "TAKO_LSP_FAKE_DIAGNOSTICS")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok());
    let goto_rules: Vec<serde_json::Value> = arg_or_env(&args, "--goto", "TAKO_LSP_FAKE_GOTO")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    append(&spawns, &std::process::id().to_string());
    eprintln!("tako-lsp-fake: scenario={scenario}");
    if scenario == "die" {
        // 起動直後に即死する（initialize を 1 通も読まない）
        std::process::exit(2);
    }

    let out = std::sync::Arc::new(Out {
        split: scenario == "split-header",
    });
    // `loading`: 読み込みが済んだか（済むまでは定義ジャンプに空で答える）
    let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(scenario != "loading"));
    let loading_ms: u64 = arg_or_env(&args, "--loading-ms", "TAKO_LSP_FAKE_LOADING_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);
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
                // `full-sync` は全文同期（change = 1）を申告する（#1660 の計測で
                // 大きいファイルの 1 打鍵が全文を送るときの負荷を測る）
                let change = if scenario == "full-sync" { 1 } else { 2 };
                let mut capabilities = serde_json::json!({
                    "positionEncoding": "utf-16",
                    "textDocumentSync": { "openClose": true, "change": change },
                });
                if scenario != "no-goto" {
                    for key in GOTO_PROVIDERS {
                        capabilities[key] = serde_json::json!(true);
                    }
                }
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": capabilities,
                        "serverInfo": { "name": "tako-lsp-fake", "version": "1" },
                    },
                }));
            }
            ("initialized", None) => match scenario.as_str() {
                "crash" => std::process::exit(3),
                "loading" => {
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStatus",
                        "params": { "health": "ok", "quiescent": false },
                    }));
                    let (out, loaded) = (out.clone(), loaded.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(loading_ms));
                        loaded.store(true, std::sync::atomic::Ordering::SeqCst);
                        out.send(serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "experimental/serverStatus",
                            "params": { "health": "ok", "quiescent": true },
                        }));
                    });
                }
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
            ("textDocument/didOpen" | "textDocument/didChange", None) if fixed.is_some() => {
                let document = &message["params"]["textDocument"];
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": document["uri"].clone(),
                        "version": document["version"].clone(),
                        "diagnostics": fixed.clone(),
                    },
                }));
            }
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
            (method, Some(id)) if GOTO_METHODS.contains(&method) => {
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": [] }));
                    continue;
                }
                let Some(rule) = goto_rule(&goto_rules, method, &message["params"]) else {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                    continue;
                };
                if rule["crash"].as_bool() == Some(true) {
                    std::process::exit(4);
                }
                if rule["silent"].as_bool() == Some(true) {
                    continue;
                }
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": rule.get("result").cloned().unwrap_or(serde_json::Value::Null),
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

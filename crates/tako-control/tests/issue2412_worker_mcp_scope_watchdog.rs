//! 番犬: worker を起動する経路は必ず MCP の絞り込み（#2412）を配線する
//!
//! ## なぜ止めるのか
//!
//! claude は起動のたびに user scope の MCP サーバーを全部起こす（1 本あたり 12〜15 プロセス・
//! 約 400 MB）。2026-10-25 に worker 40 本ぶんでメモリが尽きて PC が落ちた。絞り込みは
//! `worker_mcp::prepare` → `WorkerLaunch::mcp_settings` → `--settings` の 1 本道なので、
//! **新しい起動経路を足して `mcp_settings` を渡し忘れる**と、その経路の worker だけ黙って
//! 全部持ち込む（単体テストは今の出力しか見ないので、同じ形の再発はソースの形で止める）。
//!
//! codex（#2418）も同じ 1 本道で、`WorkerLaunch::mcp_codex_off` → `-c mcp_servers.<名>.enabled=false`。
//!
//! 1. [`worker_を起動する経路は_mcp_settings_を渡している`] — spawn / git resolve / 再開の
//!    `WorkerLaunch` が `mcp_settings:` と `mcp_codex_off:` を持ち、リテラルの `None` / 空ではない
//! 2. [`止める設定の組み立ては1実装しかない`] — `deniedMcpServers` と codex の `enabled=false` を書く
//!    製品コードが `worker_mcp.rs` だけ（経路ごとに写しが増えない）

use std::path::{Path, PathBuf};

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切る**（#986 の番犬と同じ切り方）
fn production_source(src: &str) -> String {
    let body = match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    };
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_production(rel: &str) -> String {
    let path = workspace_file(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読む: {e}"));
    production_source(&src)
}

/// `WorkerLaunch {` から対応する `}` までを 1 件ずつ切り出す（波括弧の釣り合いで数える）
fn worker_launch_blocks(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut from = 0;
    while let Some(rel) = body[from..].find("WorkerLaunch {") {
        let start = from + rel;
        let mut depth = 0usize;
        let mut end = start;
        for (i, b) in bytes[start..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(end > start, "WorkerLaunch の括弧が閉じていない");
        out.push(body[start..end].to_string());
        from = end;
    }
    out
}

/// 1: spawn / git resolve / 再開の起動経路は `mcp_settings` を渡す
#[test]
fn worker_を起動する経路は_mcp_settings_を渡している() {
    // (ファイル, 少なくともこの数の WorkerLaunch がある)
    let files = [
        ("crates/tako-control/src/dispatch.rs", 2),
        ("crates/tako-control/src/resume_launch.rs", 1),
    ];
    for (rel, at_least) in files {
        let body = read_production(rel);
        let blocks = worker_launch_blocks(&body);
        assert!(
            blocks.len() >= at_least,
            "{rel} の WorkerLaunch が {} 件しか見つからない（改名したら番犬も直す）",
            blocks.len()
        );
        for block in &blocks {
            assert!(
                block.contains("mcp_settings:"),
                "{rel}: WorkerLaunch に mcp_settings が無い = この経路の claude worker は user scope の MCP を全部持ち込む（#2412）:\n{block}"
            );
            assert!(
                !block.contains("mcp_settings: None"),
                "{rel}: 起動経路の WorkerLaunch が mcp_settings: None = user scope の MCP を全部持ち込む（#2412）:\n{block}"
            );
            // #2418: codex の worker は `mcp_codex_off` で止める（渡し忘れ = user 設定の MCP を全部持ち込む）
            assert!(
                block.contains("mcp_codex_off: &worker_mcp.codex_off"),
                "{rel}: WorkerLaunch が mcp_codex_off に worker_mcp::prepare の結果を渡していない = この経路の codex worker は user 設定の MCP を全部持ち込む（#2418）:\n{block}"
            );
        }
    }
}

/// 2: 止める設定（`deniedMcpServers` と codex の `enabled=false`）の組み立ては `worker_mcp.rs` の 1 実装だけ
#[test]
fn 止める設定の組み立ては1実装しかない() {
    let dir = workspace_file("crates/tako-control/src");
    let mut writers = Vec::new();
    let mut codex_writers = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                let src = std::fs::read_to_string(&p).unwrap();
                let body = production_source(&src);
                let rel = p.strip_prefix(&dir).unwrap().display().to_string();
                if body.contains("deniedMcpServers") {
                    writers.push(rel.clone());
                }
                if body.contains("enabled=false") {
                    codex_writers.push(rel);
                }
            }
        }
    }
    assert_eq!(
        writers,
        ["worker_mcp.rs"],
        "deniedMcpServers を組むコードが worker_mcp.rs 以外にある（正本は worker_mcp::settings_json の 1 実装）"
    );
    assert_eq!(
        codex_writers,
        ["worker_mcp.rs"],
        "codex の enabled=false を組むコードが worker_mcp.rs 以外にある（正本は worker_mcp::codex_off_args の 1 実装。#2418）"
    );
}

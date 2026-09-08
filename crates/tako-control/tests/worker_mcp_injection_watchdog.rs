//! 番犬: worker を起動する経路は必ず MCP の一時注入を配線する（#986）
//!
//! ## なぜ止めるのか
//!
//! `mcp_servers` を組むコードが **master 経路の 1 箇所しか無かった**のが #986 の症状
//! （棚卸し §5.3 = エピック #975 最大の穴）。単体テストは「今の出力」を固定するが、
//! **同じ形の再発**（新しい spawn 経路を足して `tako_bin` を渡し忘れる / 注入を
//! master 側だけ直す）はソースの形でしか止められない。
//!
//! ## 3 本立て（どれか 1 本だけでは穴が残る）
//!
//! 1. [`worker を起動する経路は_tako_bin_を渡している`] — `WorkerLaunch` を組む
//!    製品コードが `tako_bin:` を持つこと。修正前の `dispatch.rs` を名指しで落とす
//! 2. [`mcp一時注入の組み立ては1実装しかない`] — `mcp_servers.tako.args` を書く
//!    製品コードが `codex_mcp_args` だけであること（master / worker で写しが増えない）
//! 3. [`caller_paneをtako_pane_idだけで決めていない`] — MCP ブリッジが env 単独で
//!    諦めず pid 祖先辿りへ落ちること（agy / 親 env を渡さないクライアント。#987 も同型）

use std::path::{Path, PathBuf};

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付くので、
/// 最初の出現で切ると製品コードのほとんどを見落とす）
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

/// `WorkerLaunch {` から対応する `}` までを 1 件ずつ切り出す（波括弧の釣り合いで数える。
/// 引数の中に `json!{}` 等が入っても式の境界を取り違えない）
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

/// 1: worker / git resolve の spawn 経路は `tako_bin` を渡す
#[test]
fn worker_を起動する経路は_tako_bin_を渡している() {
    let body = read_production("crates/tako-control/src/dispatch.rs");
    let blocks = worker_launch_blocks(&body);
    assert!(
        blocks.len() >= 2,
        "dispatch.rs の WorkerLaunch が {} 件しか見つからない（改名したら番犬も直す）",
        blocks.len()
    );
    for block in &blocks {
        assert!(
            block.contains("tako_bin:"),
            "WorkerLaunch に tako_bin が無い = この経路の codex worker は MCP を呼べない（#986）:\n{block}"
        );
        // `tako_bin: None` は「MCP を配線しない」の明示なので、spawn 経路では誤り。
        // ソース走査で見られるのはリテラルの None までで、実行時に None になる変数は
        // 見えない（そこは単体テスト `i986_codex_workerに_mcp_の一時注入が付く` が受け持つ）
        assert!(
            !block.contains("tako_bin: None"),
            "spawn 経路の WorkerLaunch が tako_bin: None = codex worker から MCP が消える（#986）:\n{block}"
        );
    }
}

/// 2: 注入の組み立ては `codex_mcp_args` の 1 実装だけ
#[test]
fn mcp一時注入の組み立ては1実装しかない() {
    let files = [
        "crates/tako-control/src/orchestrator/agent.rs",
        "crates/tako-control/src/orchestrator/mod.rs",
        "crates/tako-control/src/dispatch.rs",
    ];
    let mut writers = Vec::new();
    for rel in files {
        let body = read_production(rel);
        for (i, line) in body.lines().enumerate() {
            if line.contains("mcp_servers.tako.args") {
                writers.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        writers.len(),
        1,
        "MCP 一時注入の組み立てが {} 箇所ある（正本は agent::codex_mcp_args の 1 実装）: {writers:?}",
        writers.len()
    );
    assert!(
        writers[0].contains("orchestrator/agent.rs"),
        "正本の置き場が変わっている: {writers:?}"
    );
}

/// 3: MCP ブリッジは env 単独で諦めない（pid 祖先辿りへ落ちる）
#[test]
fn caller_paneをtako_pane_idだけで決めていない() {
    let body = read_production("crates/tako-cli/src/main.rs");
    let start = body
        .find("fn mcp_serve()")
        .expect("mcp_serve が見つからない（改名したら番犬も直す）");
    let block = &body[start..];
    let end = block.find("\nfn ").map(|i| i + 1).unwrap_or(block.len());
    let block = &block[..end];
    assert!(
        block.contains("caller_pane_plan("),
        "mcp_serve が caller_pane_plan を通っていない = 解決順が散っている（#986）"
    );
    assert!(
        block.contains("CallerPanePlan::ResolveByPid"),
        "pid 祖先辿りのフォールバックが無い = env を渡さない系統から caller_pane が解けない（#986 / #987）"
    );
}

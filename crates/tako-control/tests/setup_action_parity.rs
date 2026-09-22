//! MCP が広告する action と dispatch が受理する action のドリフト検出（#1057）
//!
//! ## なぜ要るか（実際に踏んだ）
//!
//! #1057 の実装中、A/B のために `dispatch` の `handoff` アームを外したまま
//! コミットしてしまった。**カタログは `handoff` を広告し続ける**ので、
//! AI から見ると「あるはずのツールが不明な action で断られる」状態になる。
//! `cargo test` はこれを 1 件も検出しなかった（dispatch のアームを覆うテストが無い）。
//!
//! ここではソースを走査して「スキーマの enum に載っている action は
//! dispatch のアームにも在る」ことを固定する。dispatch は `ControlHost` を
//! 要求するので実呼び出しでは書きにくいが、**ドリフトの検出には十分**。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// そのツールの `inputSchema.properties.action.enum`
fn advertised_actions(tool: &str) -> Vec<String> {
    let tools = tako_control::mcp::tools();
    let found = tools
        .iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} が MCP カタログに無い"));
    found["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{tool} の action に enum が無い"))
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

/// dispatch.rs の本文（アームの文字列リテラルを探す）。
///
/// **コメントは落とす**（#1609）。アームを消しても「`"install" =>` を受ける」と
/// 書いた説明コメントが残っていれば緑になる形を避ける。文字列リテラルは
/// 囲みごと残るので、`"\"{action}\" =>"` の照合はそのまま効く
fn dispatch_source() -> String {
    let rel = "crates/tako-control/src/dispatch.rs";
    let text = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    code_view::without_comments_checked(&text, rel)
}

/// `Request::<variant>` のブロックを切り出す（他の variant のアームを誤って拾わない）
fn request_block(source: &str, variant: &str) -> String {
    let needle = format!("Request::{variant} {{");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("dispatch に Request::{variant} が無い"));
    let rest = &source[start..];
    // 次の `Request::` まで（= このアームの範囲）
    let end = rest[needle.len()..]
        .find("\n        Request::")
        .map(|i| i + needle.len())
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn setupツールが広告するactionはdispatchが受理する() {
    let source = dispatch_source();
    for (tool, variant) in [
        ("tako_setup_bootstrap", "SetupBootstrap"),
        ("tako_setup_deps", "SetupDeps"),
    ] {
        let block = request_block(&source, variant);
        for action in advertised_actions(tool) {
            let arm = format!("\"{action}\" =>");
            assert!(
                block.contains(&arm),
                "{tool} は action={action:?} を広告しているが dispatch の \
                 Request::{variant} に {arm} が無い\n\
                 → カタログとアームのどちらかが古い（#1057 でアームを消したまま \
                 コミットして踏んだ）"
            );
        }
    }
}

/// 逆向き: dispatch が受理する action はカタログにも載っている
/// （AI から到達できない隠し action を作らない）
#[test]
fn dispatchが受理するactionはカタログに載っている() {
    let source = dispatch_source();
    for (tool, variant) in [
        ("tako_setup_bootstrap", "SetupBootstrap"),
        ("tako_setup_deps", "SetupDeps"),
    ] {
        let block = request_block(&source, variant);
        let advertised = advertised_actions(tool);
        // アームは `"<action>" =>` の形。`other =>` は網羅の受け皿なので対象外
        for line in block.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix('"') else {
                continue;
            };
            let Some((action, tail)) = rest.split_once('"') else {
                continue;
            };
            if !tail.trim_start().starts_with("=>") {
                continue;
            }
            assert!(
                advertised.iter().any(|a| a == action),
                "{tool}: dispatch は action={action:?} を受理するが \
                 カタログの enum に無い（AI から到達できない）"
            );
        }
    }
}

//! MCP 公開カタログの完全スナップショット。
//!
//! ツール名だけでなく説明文・inputSchema・公開順も公開契約として固定し、
//! 構造整理で意図せず変わらないことを `cargo test` だけで検出する。

use std::collections::HashSet;
use std::path::Path;

use tako_control::mcp;

const SNAPSHOT: &str = include_str!("../testdata/mcp_tools_full_snapshot.json");

fn canonicalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

/// 実行プラットフォームで変わる語のプレースホルダ。
///
/// リンクを開く打鍵は macOS = `⌘+クリック` / Windows = `Ctrl+クリック`（#763）。
/// カタログは**実行 OS の打鍵**を申告するのが正しいので、スナップショットは
/// そこだけ伏せて比べる（伏せないと Windows ランナーの `cargo test` が必ず割れる）
const LINK_CLICK_PLACEHOLDER: &str = "<link-click>";

/// 実行 OS に追従する語を伏せる。**1 件も見つからなければ失敗**
/// （追従をやめて直書きへ戻したら、それはそれで検出したい）
fn mask_platform_words(rendered: String) -> String {
    let link_click =
        tako_core::platform::keys::link_click(tako_core::platform::support::Platform::current());
    assert!(
        rendered.contains(&link_click),
        "カタログに「{link_click}」が 1 件も無い。\
         リンクの打鍵の申告が実行プラットフォームに追従していない（#763）"
    );
    rendered.replace(&link_click, LINK_CLICK_PLACEHOLDER)
}

fn rendered_catalog() -> String {
    let catalog = canonicalize(serde_json::Value::Array(mcp::tools()));
    mask_platform_words(
        serde_json::to_string_pretty(&catalog).expect("MCP カタログを JSON 化できる") + "\n",
    )
}

#[test]
fn mcp公開カタログが完全スナップショットと一致する() {
    let actual = rendered_catalog();
    if std::env::var_os("TAKO_UPDATE_MCP_SNAPSHOT").is_some() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/mcp_tools_full_snapshot.json");
        std::fs::write(&path, &actual)
            .unwrap_or_else(|e| panic!("スナップショットを更新できない {}: {e}", path.display()));
        return;
    }

    assert_eq!(
        actual, SNAPSHOT,
        "MCP 公開カタログが変化した。挙動変更でないことを確認し、意図した変更の場合だけ\
         `TAKO_UPDATE_MCP_SNAPSHOT=1 cargo test -p tako-control \
         --test mcp_catalog_snapshot` で更新する"
    );
}

#[test]
fn mcpツール名は空でなく重複しない() {
    let tools = mcp::tools();
    let mut names = HashSet::with_capacity(tools.len());
    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .expect("全 MCP ツールに文字列の name が必要");
        assert!(!name.is_empty(), "MCP ツール名は空にできない");
        assert!(names.insert(name), "MCP ツール名が重複している: {name}");
    }
}

#[test]
fn snapshotのobjectキー順はserde_jsonのfeatureに依存しない() {
    let mut object = serde_json::Map::new();
    object.insert("z".into(), serde_json::json!({ "y": 1, "a": 2 }));
    object.insert("a".into(), serde_json::json!(0));

    let rendered = serde_json::to_string(&canonicalize(object.into())).unwrap();
    assert_eq!(rendered, r#"{"a":0,"z":{"a":2,"y":1}}"#);
}

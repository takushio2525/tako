//! mcp_calls — 会話の記録（transcript）で呼ばれた MCP サーバーを数える純関数（Issue #2412）
//!
//! worker を tako の MCP だけで起動する移行（`tako_control::worker_mcp`）が、直近の実績から
//! プロジェクトごとに持ち込む MCP を決めるのに使う。数 GB を読むので**バイト列のまま**探す
//! （`regex::bytes` はリテラルの接頭辞を SIMD で先に探す。行ごとに UTF-8 の文字列を作って
//! `str` で探す形は release でも 6 GB に 4 秒かかった = 実測。読み込みだけなら 0.8 秒）。

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// tool_use の `"name":"mcp__<server>__<tool>"`。JSON の文字列値の中（`\"name\":\"mcp__…`）は
/// 引用符が `\"` になるので当たらない。サーバー名は claude が `[A-Za-z0-9_-]` へ畳んだ形
fn pattern() -> &'static regex::bytes::Regex {
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::bytes::Regex::new(r#""name":"mcp__([A-Za-z0-9_-]+?)__"#).expect("固定の正規表現")
    })
}

/// `bytes` の中の MCP の呼び出しをサーバー名ごとに数えて `out` へ足す
pub fn count_calls(bytes: &[u8], out: &mut BTreeMap<String, u32>) {
    for caps in pattern().captures_iter(bytes) {
        if let Some(server) = caps.get(1) {
            // 文字の集合が ASCII に限られているので UTF-8 として必ず読める
            let server = String::from_utf8_lossy(server.as_bytes()).into_owned();
            *out.entry(server).or_default() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(text: &str) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        count_calls(text.as_bytes(), &mut out);
        out
    }

    #[test]
    fn tool_useの名前からサーバーを数える() {
        let got = count(concat!(
            r#"{"type":"tool_use","id":"t","name":"mcp__playwright__browser_click","input":{}}"#,
            "\n",
            r#"{"type":"tool_use","name":"mcp__claude_ai_Gmail__search"}"#,
            r#"{"type":"tool_use","name":"mcp__sequential-thinking__think"}"#,
            r#"{"type":"tool_use","name":"mcp__playwright__browser_navigate"}"#,
        ));
        assert_eq!(got.get("playwright"), Some(&2));
        assert_eq!(got.get("claude_ai_Gmail"), Some(&1));
        assert_eq!(got.get("sequential-thinking"), Some(&1));
    }

    #[test]
    fn 文字列値の中と組み込みツールは数えない() {
        let got = count(concat!(
            r#"{"type":"tool_result","content":"\"name\":\"mcp__github__x\""}"#,
            r#"{"type":"tool_use","name":"Bash"}"#,
            r#"{"type":"tool_use","name":"mcp__"}"#,
        ));
        assert!(got.is_empty(), "{got:?}");
    }
}

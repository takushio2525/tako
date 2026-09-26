//! 診断（`textDocument/publishDiagnostics`）の受信と保持だけ（#1678）
//!
//! 描画（波線・右パネル）は #1679 の担当で、ここは**受けて保つだけ**。
//! 保持はファイル URI ごとで、**開いている文書のぶんだけ**持つ（閉じたら捨てる = #830 の
//! 「閉じたら解放」の作法。開いていない URI への publish は保持しない = 数が文書数で頭打ち）。

use std::collections::HashMap;

use lsp_types::{Diagnostic, PublishDiagnosticsParams};
use serde_json::Value;

/// URI ごとの最新の診断
#[derive(Debug, Default)]
pub struct DiagnosticsStore {
    by_uri: HashMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticsStore {
    /// `publishDiagnostics` の params を取り込む。`open_uri` はサーバの綴りの URI から
    /// **こちらが開いている文書の URI** を引く（符号化の流儀の違いを吸収する）。
    /// 引けない URI は捨てる。取り込んだら `true`（形が壊れていれば `false`）
    pub fn publish(&mut self, params: Value, open_uri: impl Fn(&str) -> Option<String>) -> bool {
        let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(params) else {
            return false;
        };
        let Some(uri) = open_uri(params.uri.as_str()) else {
            return false;
        };
        self.by_uri.insert(
            uri,
            Entry {
                version: params.version,
                diagnostics: params.diagnostics,
            },
        );
        true
    }

    /// 文書を閉じたら捨てる
    pub fn forget(&mut self, uri: &str) {
        self.by_uri.remove(uri);
    }

    /// その文書の診断の数
    pub fn count(&self, uri: &str) -> usize {
        self.by_uri.get(uri).map_or(0, |e| e.diagnostics.len())
    }

    /// 保持している診断の総数
    pub fn total(&self) -> usize {
        self.by_uri.values().map(|e| e.diagnostics.len()).sum()
    }

    /// その文書の診断（#1679 が読む）
    pub fn get(&self, uri: &str) -> Option<(&[Diagnostic], Option<i32>)> {
        self.by_uri
            .get(uri)
            .map(|e| (e.diagnostics.as_slice(), e.version))
    }

    /// 保持している URI の数
    pub fn uri_count(&self) -> usize {
        self.by_uri.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(uri: &str, n: usize) -> Value {
        let diagnostics: Vec<Value> = (0..n)
            .map(|i| {
                json!({
                    "range": {"start": {"line": i, "character": 0}, "end": {"line": i, "character": 1}},
                    "message": "m",
                })
            })
            .collect();
        json!({ "uri": uri, "version": 3, "diagnostics": diagnostics })
    }

    #[test]
    fn 開いている文書のぶんだけ保持し閉じたら捨てる() {
        let mut store = DiagnosticsStore::default();
        let open = |uri: &str| (uri == "file:///w/a.x").then(|| uri.to_string());
        assert!(store.publish(params("file:///w/a.x", 2), open));
        assert!(!store.publish(params("file:///w/other.x", 5), open));
        assert_eq!(store.count("file:///w/a.x"), 2);
        assert_eq!(store.total(), 2);
        assert_eq!(store.get("file:///w/a.x").map(|(_, v)| v), Some(Some(3)));
        // 後から来た publish が置き換える（積み増さない）
        assert!(store.publish(params("file:///w/a.x", 1), open));
        assert_eq!(store.total(), 1);
        store.forget("file:///w/a.x");
        assert_eq!(store.total(), 0);
        assert_eq!(store.uri_count(), 0);
    }

    #[test]
    fn 壊れた形は取り込まない() {
        let mut store = DiagnosticsStore::default();
        assert!(!store.publish(json!({"uri": 1}), |u| Some(u.to_string())));
        assert_eq!(store.total(), 0);
    }
}

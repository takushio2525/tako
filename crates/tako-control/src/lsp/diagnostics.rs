//! 診断（`textDocument/publishDiagnostics`）の受信と保持（#1678 / #1679）
//!
//! 保持はファイル URI ごとで、**開いている文書のぶんだけ**持つ（閉じたら捨てる = #830 の
//! 「閉じたら解放」の作法。開いていない URI への publish は保持しない = 数が文書数で頭打ち）。
//!
//! #1679 から、受けた時点で **tako の座標**（`tako_core::lsp::diagnostic`。0 起点の行 +
//! 行内の UTF-8 バイト桁）へ写して持つ。写す本文は**サーバへ送った本文の写し**
//! （= サーバが見ている本文）で、UTF-16 の桁の変換は `position::LineIndex` の 1 実装を通る。
//! 波線・右パネル・CLI / MCP はすべてここを読む（同じ表を読むので食い違わない）。

use std::collections::HashMap;
use std::sync::Arc;

use lsp_types::{NumberOrString, PublishDiagnosticsParams};
use serde_json::Value;
use tako_core::lsp::diagnostic::{self, Diagnostic, Point, Severity};
use tako_core::lsp::position::LineIndex;

/// 1 文書あたりに保つ診断の上限。サーバが桁違いの数を送ってきても UI と
/// メモリが引きずられないように（超えたぶんは**軽いものから**捨て、`dropped` で知らせる）
pub const MAX_PER_DOCUMENT: usize = 5_000;

/// 1 文書の最新の診断（`items` は `Arc` なので UI が写しを持っても本体は 1 つ）
#[derive(Debug, Clone, Default)]
pub struct DocDiagnostics {
    /// サーバが付けた版（付けないサーバもある）
    pub version: Option<i32>,
    /// 位置の順（[`diagnostic::sort`]）
    pub items: Arc<[Diagnostic]>,
    /// [`MAX_PER_DOCUMENT`] を超えて捨てた数
    pub dropped: usize,
}

/// URI ごとの最新の診断
#[derive(Debug, Default)]
pub struct DiagnosticsStore {
    by_uri: HashMap<String, DocDiagnostics>,
}

impl DiagnosticsStore {
    /// `publishDiagnostics` の params を取り込む。`open_doc` はサーバの綴りの URI から
    /// **こちらが開いている文書の URI とサーバへ送った本文**を引く（符号化の流儀の違いを
    /// 吸収する）。引けない URI は捨てる。取り込んだら取り込んだ URI（形が壊れていれば `None`）
    pub fn publish<'t>(
        &mut self,
        params: Value,
        open_doc: impl Fn(&str) -> Option<(String, &'t str)>,
    ) -> Option<String> {
        let params = serde_json::from_value::<PublishDiagnosticsParams>(params).ok()?;
        let (uri, text) = open_doc(params.uri.as_str())?;
        let index = LineIndex::new(text);
        let mut items: Vec<Diagnostic> = params
            .diagnostics
            .iter()
            .map(|d| from_lsp(d, &index))
            .collect();
        let dropped = items.len().saturating_sub(MAX_PER_DOCUMENT);
        if dropped > 0 {
            // 重いものを残す（同じ重さの中は位置の順）
            items.sort_by_key(|d| (d.severity, d.start));
            items.truncate(MAX_PER_DOCUMENT);
        }
        diagnostic::sort(&mut items);
        self.by_uri.insert(
            uri.clone(),
            DocDiagnostics {
                version: params.version,
                items: items.into(),
                dropped,
            },
        );
        Some(uri)
    }

    /// 文書を閉じた / サーバが止まったら捨てる。捨てたら `true`
    pub fn forget(&mut self, uri: &str) -> bool {
        self.by_uri.remove(uri).is_some()
    }

    /// その文書の診断の数
    pub fn count(&self, uri: &str) -> usize {
        self.by_uri.get(uri).map_or(0, |e| e.items.len())
    }

    /// 保持している診断の総数（#1679 の受け入れ条件「閉じたら 0」の観測口）
    pub fn total(&self) -> usize {
        self.by_uri.values().map(|e| e.items.len()).sum()
    }

    /// その文書の診断
    pub fn get(&self, uri: &str) -> Option<DocDiagnostics> {
        self.by_uri.get(uri).cloned()
    }

    /// 保持している URI の数
    pub fn uri_count(&self) -> usize {
        self.by_uri.len()
    }
}

/// LSP の診断 1 件 → tako の座標（本文は**サーバが見ている本文**）
fn from_lsp(d: &lsp_types::Diagnostic, index: &LineIndex<'_>) -> Diagnostic {
    let point = |p: lsp_types::Position| {
        let (line, col) = index.line_byte_col(p.line as usize, p.character as usize);
        Point { line, col }
    };
    let start = point(d.range.start);
    // 終点が始点より手前（壊れた範囲）なら幅 0 に揃える
    let end = point(d.range.end).max(start);
    let severity = Severity::from_lsp(d.severity.map(severity_number));
    Diagnostic {
        severity,
        start,
        end,
        message: d.message.clone(),
        source: d.source.clone(),
        code: d.code.as_ref().map(|c| match c {
            NumberOrString::Number(n) => n.to_string(),
            NumberOrString::String(s) => s.clone(),
        }),
    }
}

/// `DiagnosticSeverity`（数値の newtype）→ 1〜4
fn severity_number(severity: lsp_types::DiagnosticSeverity) -> i64 {
    use lsp_types::DiagnosticSeverity as S;
    match severity {
        S::WARNING => 2,
        S::INFORMATION => 3,
        S::HINT => 4,
        _ => 1,
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

    const TEXT: &str = "a\nb\nc\nd\ne\nf\n";

    #[test]
    fn 開いている文書のぶんだけ保持し閉じたら捨てる() {
        let mut store = DiagnosticsStore::default();
        let open = |uri: &str| (uri == "file:///w/a.x").then(|| (uri.to_string(), TEXT));
        assert_eq!(
            store.publish(params("file:///w/a.x", 2), open).as_deref(),
            Some("file:///w/a.x")
        );
        assert!(store
            .publish(params("file:///w/other.x", 5), open)
            .is_none());
        assert_eq!(store.count("file:///w/a.x"), 2);
        assert_eq!(store.total(), 2);
        assert_eq!(store.get("file:///w/a.x").map(|d| d.version), Some(Some(3)));
        // 後から来た publish が置き換える（積み増さない）
        assert!(store.publish(params("file:///w/a.x", 1), open).is_some());
        assert_eq!(store.total(), 1);
        assert!(store.forget("file:///w/a.x"));
        assert!(!store.forget("file:///w/a.x"));
        assert_eq!(store.total(), 0);
        assert_eq!(store.uri_count(), 0);
    }

    #[test]
    fn 壊れた形は取り込まない() {
        let mut store = DiagnosticsStore::default();
        assert!(store
            .publish(json!({"uri": 1}), |u| Some((u.to_string(), "")))
            .is_none());
        assert_eq!(store.total(), 0);
    }

    /// UTF-16 の桁を**サーバが見ている本文**で UTF-8 バイトへ写す（日本語・絵文字・CRLF）
    #[test]
    fn 受けた時点で_tako_の座標へ写す() {
        let text = "// 日本😀\r\nlet x = 1;\n";
        let mut store = DiagnosticsStore::default();
        let open = |u: &str| Some((u.to_string(), text));
        store.publish(
            json!({
                "uri": "file:///w/a.x",
                "diagnostics": [
                    // 「本😀」= UTF-16 で 4..7（本 1 + 😀 2）
                    {"range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 7}},
                     "severity": 2, "message": "w", "source": "src", "code": "E0308"},
                    // 行末を超える桁は `\r` の手前へ
                    {"range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 99}},
                     "message": "省略はエラー", "code": 42},
                ],
            }),
            open,
        );
        let got = store.get("file:///w/a.x").unwrap();
        assert_eq!(got.items.len(), 2);
        let w = &got.items[0];
        assert_eq!(
            (w.start, w.end),
            (Point { line: 0, col: 6 }, Point { line: 0, col: 13 })
        );
        assert_eq!(w.severity, Severity::Warning);
        assert_eq!(
            (w.source.as_deref(), w.code.as_deref()),
            (Some("src"), Some("E0308"))
        );
        let e = &got.items[1];
        assert_eq!(
            (e.start, e.end),
            (Point { line: 1, col: 4 }, Point { line: 1, col: 10 })
        );
        assert_eq!(e.severity, Severity::Error);
        assert_eq!(e.code.as_deref(), Some("42"));
    }

    #[test]
    fn 逆向きの範囲は幅0に揃える() {
        let mut store = DiagnosticsStore::default();
        store.publish(
            json!({"uri": "file:///w/a", "diagnostics": [
                {"range": {"start": {"line": 1, "character": 1}, "end": {"line": 0, "character": 0}}, "message": "m"}
            ]}),
            |u| Some((u.to_string(), TEXT)),
        );
        let d = &store.get("file:///w/a").unwrap().items[0];
        assert_eq!(d.start, d.end);
    }

    #[test]
    fn 上限を超えたら軽いものから捨てて数を知らせる() {
        let mut store = DiagnosticsStore::default();
        let mut list: Vec<Value> = (0..MAX_PER_DOCUMENT + 3)
            .map(|_| {
                json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                       "severity": 4, "message": "h"})
            })
            .collect();
        list.push(json!({"range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 1}},
                         "severity": 1, "message": "e"}));
        store.publish(json!({"uri": "file:///w/a", "diagnostics": list}), |u| {
            Some((u.to_string(), TEXT))
        });
        let got = store.get("file:///w/a").unwrap();
        assert_eq!(got.items.len(), MAX_PER_DOCUMENT);
        assert_eq!(got.dropped, 4);
        assert!(
            got.items.iter().any(|d| d.severity == Severity::Error),
            "重いものは残る"
        );
    }
}

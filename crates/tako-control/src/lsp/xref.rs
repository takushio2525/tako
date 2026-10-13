//! 参照検索・呼び出し階層・型階層（S8 / #1685）の問い合わせの型と応答の組み立て
//!
//! 問い合わせそのもの（文書を開く・サーバの起動を待つ・要求を投げる）は状態を持つ
//! [`super::manager`] の `Shared::xref` / `Shared::xref_expand` が行い、ここは**その入出力の形**と
//! 応答の JSON だけを持つ。結果の保持・ツリーの展開状態の正本は `tako_core::lsp::xref`、
//! 行を開く着地は `crate::dispatch`（定義ジャンプと同じ `plan_landing`）。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::xref::{self as core, Body, NodeSeed, Place, Tree, XrefKind, XrefResult};

use super::{goto, text, GotoError};

/// 問い合わせの上限の既定。起動と握手を含む。参照の検索は大きなプロジェクトで定義ジャンプより
/// 長くかかる（全ファイルを見る）ので既定を長めに取る。無限には待たない
/// （`.agent/conventions.md`「外部コマンドを待つときは上限を持つ（#1503）」）。
/// `TAKO_LSP_XREF_TIMEOUT_SECS` で変えられる（0 / 不正 / 空は既定へ落とす）
pub const DEFAULT_XREF_TIMEOUT: Duration = Duration::from_secs(60);

/// 応答に載せる行の既定（CLI `--limit` / MCP `limit`。0 = 保持しているもの全部）
pub const DEFAULT_LIMIT: usize = 100;

/// `TAKO_1685_LEGACY=1` で **#1685 前の形**へ戻す（同一バイナリで A/B を取る入口）: 右クリック
/// メニューに参照・階層の項目を出さない・握手の能力から型の知らないキー（`typeHierarchyProvider`）を
/// 落とす。CLI / MCP の `tako lsp references` 等は残る（戻すのは既存経路への影響だけ）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_1685_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 本番の上限（env で値だけを変えられる）
pub fn xref_timeout() -> Duration {
    std::env::var("TAKO_LSP_XREF_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(DEFAULT_XREF_TIMEOUT, Duration::from_secs)
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::xref`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrefRequest {
    pub kind: XrefKind,
    pub path: PathBuf,
    /// tako の座標（0 起点の行・行内の UTF-8 バイト桁）。LSP の座標へは manager が直す（#1769）
    pub line: usize,
    pub column: usize,
    pub timeout: Duration,
    /// 文書を開いていないときにサーバへ渡す本文（編集セッションの全文）。`None` ならディスク
    pub document: Option<String>,
}

/// 答え
#[derive(Debug, Clone, PartialEq)]
pub enum XrefAnswer {
    /// 参照の一覧（`total` は上限で切る前の件数）
    List {
        server: &'static str,
        items: Vec<Place>,
        total: usize,
    },
    /// 階層の根と、それぞれの 1 段目の子（展開に失敗した根は `None` = 閉じたまま）
    Tree {
        server: &'static str,
        roots: Vec<(NodeSeed, Option<Vec<NodeSeed>>)>,
    },
}

impl XrefAnswer {
    pub fn server(&self) -> &'static str {
        match self {
            Self::List { server, .. } | Self::Tree { server, .. } => server,
        }
    }

    /// 0 件か（読み込み中のサーバの空の答えを問い直す判定にも使う）
    pub fn is_empty(&self) -> bool {
        match self {
            Self::List { items, .. } => items.is_empty(),
            Self::Tree { roots, .. } => roots.is_empty(),
        }
    }

    /// 結果の中身へ（ツリーは 1 段目の子を読み込んで根を開いた形）
    pub fn into_body(self) -> (Body, usize) {
        match self {
            Self::List { items, total, .. } => (Body::List(items), total),
            Self::Tree { roots, .. } => {
                let (seeds, children): (Vec<NodeSeed>, Vec<Option<Vec<NodeSeed>>>) =
                    roots.into_iter().unzip();
                let mut tree = Tree::new(seeds);
                let ids: Vec<usize> = tree.roots().to_vec();
                for (id, kids) in ids.into_iter().zip(children) {
                    if let Some(kids) = kids {
                        let _ = tree.set_children(id, kids);
                    }
                }
                let total = tree.len();
                (Body::Tree(tree), total)
            }
        }
    }
}

/// ツリーの 1 段の展開（UI スレッドで作り、背景で [`super::LspManager::xref_expand`] へ渡す）
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandRequest {
    pub kind: XrefKind,
    /// 結果を問い合わせた文書（この文書を受け持つサーバへ送る）
    pub path: PathBuf,
    /// 展開する要素（サーバから受けたまま = `NodeSeed::raw`）
    pub item: Value,
    pub timeout: Duration,
}

/// 展開の答え
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandAnswer {
    pub server: &'static str,
    pub children: Vec<NodeSeed>,
}

/// 種類の呼び名（`text::XREF_KIND_LABELS` を `XrefKind::ALL` の順で引く）
pub fn kind_label(kind: XrefKind) -> &'static str {
    let index = XrefKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default();
    text::XREF_KIND_LABELS[index].text()
}

/// 失敗の応答（`status` / `reason` / `next_step`。言語サーバの状態の文は定義ジャンプと共有）
pub fn error_json(error: &GotoError, kind: XrefKind) -> Value {
    let mut out = json!({
        "status": error.status(),
        "kind": kind.slug(),
        "reason": error.reason_in(text::XREF_UNSUPPORTED_REASON, kind_label(kind)),
        "next_step": error.next_step(),
    });
    error.add_server(&mut out);
    out
}

/// 結果を持たないときの応答
pub fn no_result_json() -> Value {
    json!({
        "status": "no-result",
        "reason": text::XREF_NO_RESULT_REASON.text(),
        "next_step": text::XREF_NO_RESULT_NEXT_STEP.text(),
    })
}

/// 場所 1 つの wire 形。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8
/// バイト）なので、そのまま次の問い合わせ（`tako lsp references --line --column`）へ渡せる
pub fn place_json(place: &Place) -> Value {
    json!({
        "path": place.path.display().to_string(),
        "line": place.line + 1,
        "column": place.column,
        "text": place.excerpt,
    })
}

/// 場所を tako の座標へ直す（定義ジャンプの [`goto::locate`] と同じ 1 実装）
pub fn place_of(
    uri: String,
    path: PathBuf,
    line: usize,
    character: usize,
    source: Option<&str>,
) -> Place {
    let t = goto::locate(uri, path, line, character, source);
    Place {
        uri: t.uri,
        path: t.path,
        line: t.line,
        column: t.column,
        char_column: t.char_column,
        excerpt: t.excerpt,
    }
}

/// ノード 1 つの wire 形（`children` は読み込んだ子の数。`null` = まだ問い合わせていない）
fn node_json(tree: &Tree, id: usize) -> Value {
    let Some(node) = tree.node(id) else {
        return Value::Null;
    };
    let mut out = place_json(&node.place);
    out["id"] = json!(id);
    out["depth"] = json!(node.depth);
    out["name"] = json!(node.name);
    out["symbol_kind"] = json!(core::symbol_kind_name(node.kind));
    if let Some(detail) = &node.detail {
        out["detail"] = json!(detail);
    }
    out["expanded"] = json!(node.expanded);
    out["children"] = json!(node.children.as_ref().map(Vec::len));
    out
}

/// 結果の wire 形（`limit` は載せる行の数。`None` = 既定 [`DEFAULT_LIMIT`]・0 = 全部）。
///
/// 一覧は `items[]`（`id` = 行の番号）、ツリーは**見えている行**の `nodes[]`（深さ優先・`id` =
/// ノードの番号・`expanded`）と開いているノードの `expanded[]`。どちらの `id` も
/// `tako lsp open-result --item` / `expand` / `collapse` へそのまま渡せる
pub fn result_json(result: &XrefResult, limit: Option<usize>) -> Value {
    let cap = match limit.unwrap_or(DEFAULT_LIMIT) {
        0 => usize::MAX,
        n => n,
    };
    let mut out = json!({
        "status": if result.body.is_empty() { "none" } else { "found" },
        "kind": result.kind.slug(),
        "server": result.server,
        "pane": result.pane.as_u64(),
        "from": {
            "path": result.path.display().to_string(),
            "line": result.line,
            "column": result.column,
            "symbol": result.symbol,
        },
    });
    match &result.body {
        Body::List(items) => {
            let shown: Vec<Value> = items
                .iter()
                .take(cap)
                .enumerate()
                .map(|(i, place)| {
                    let mut row = place_json(place);
                    row["id"] = json!(i + 1);
                    row
                })
                .collect();
            out["total"] = json!(result.total);
            out["shown"] = json!(shown.len());
            out["truncated"] = json!(result.total.saturating_sub(shown.len()));
            out["items"] = Value::Array(shown);
        }
        Body::Tree(tree) => {
            let rows = tree.rows();
            let shown: Vec<Value> = rows
                .iter()
                .take(cap)
                .map(|row| node_json(tree, row.id))
                .collect();
            out["node_count"] = json!(tree.len());
            out["visible"] = json!(rows.len());
            out["shown"] = json!(shown.len());
            out["truncated"] = json!(rows.len().saturating_sub(shown.len()) + tree.truncated);
            out["expanded"] = json!(tree.expanded());
            out["nodes"] = Value::Array(shown);
        }
    }
    if result.body.is_empty() {
        let label = kind_label(result.kind);
        out["reason"] = json!(text::fill(text::XREF_NONE_REASON, &[("kind", label)]));
        out["next_step"] = json!(text::XREF_NONE_NEXT_STEP.text());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::i18n::Lang;
    use tako_core::PaneId;

    fn place(path: &str, line: usize, excerpt: &str) -> Place {
        Place {
            uri: format!("file://{path}"),
            path: PathBuf::from(path),
            line,
            column: 4,
            char_column: 4,
            excerpt: excerpt.into(),
        }
    }

    fn seed(name: &str, line: usize) -> NodeSeed {
        NodeSeed {
            name: name.into(),
            kind: 12,
            detail: None,
            place: place(&format!("/w/{name}.rs"), line, name),
            raw: json!({ "name": name }),
        }
    }

    fn list_result(items: Vec<Place>, total: usize) -> XrefResult {
        XrefResult {
            pane: PaneId::from_raw(7),
            path: PathBuf::from("/w/src/main.rs"),
            line: 3,
            column: 4,
            symbol: "helper".into(),
            kind: XrefKind::References,
            server: "srv".into(),
            body: Body::List(items),
            total,
            seq: 1,
        }
    }

    #[test]
    fn 一覧の行はパスと行と抜粋と番号() {
        let result = list_result(
            vec![
                place("/w/a.rs", 0, "helper();"),
                place("/w/b.rs", 9, "x = helper(1);"),
            ],
            2,
        );
        let out = result_json(&result, None);
        assert_eq!(out["status"], json!("found"));
        assert_eq!(out["from"]["symbol"], json!("helper"));
        assert_eq!(
            out["items"],
            json!([
                { "id": 1, "path": "/w/a.rs", "line": 1, "column": 4, "text": "helper();" },
                { "id": 2, "path": "/w/b.rs", "line": 10, "column": 4, "text": "x = helper(1);" },
            ])
        );
        assert_eq!(
            (out["total"].clone(), out["truncated"].clone()),
            (json!(2), json!(0))
        );
    }

    #[test]
    fn 載せる行は上限で切り切った数を言う() {
        let items: Vec<Place> = (0..5).map(|i| place("/w/a.rs", i, "")).collect();
        // 上限で切る前の全件数が 9（保持は 5）
        let result = list_result(items, 9);
        let out = result_json(&result, Some(2));
        assert_eq!(
            (out["shown"].clone(), out["truncated"].clone()),
            (json!(2), json!(7))
        );
        let all = result_json(&result, Some(0));
        assert_eq!(
            (all["shown"].clone(), all["truncated"].clone()),
            (json!(5), json!(4))
        );
    }

    #[test]
    fn 空の結果は理由と次の一手を持つ() {
        let out = result_json(&list_result(Vec::new(), 0), None);
        assert_eq!(out["status"], json!("none"));
        for key in ["reason", "next_step"] {
            let s = out[key].as_str().unwrap_or("");
            assert!(!s.is_empty() && !s.contains('{'), "{key}: {s}");
        }
    }

    #[test]
    fn ツリーは見えている行と開いているノードを返す() {
        let answer = XrefAnswer::Tree {
            server: "srv",
            roots: vec![(seed("root", 0), Some(vec![seed("a", 1), seed("b", 2)]))],
        };
        let (body, total) = answer.into_body();
        assert_eq!(total, 3);
        let mut result = list_result(Vec::new(), 0);
        result.kind = XrefKind::IncomingCalls;
        result.body = body;
        let out = result_json(&result, None);
        assert_eq!(out["expanded"], json!([1]));
        let ids: Vec<u64> = out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(out["nodes"][0]["children"], json!(2));
        assert_eq!(out["nodes"][1]["children"], Value::Null);
        assert_eq!(out["nodes"][1]["depth"], json!(1));
        assert_eq!(out["nodes"][1]["symbol_kind"], json!("function"));
    }

    #[test]
    fn 展開に失敗した根は閉じたまま() {
        let answer = XrefAnswer::Tree {
            server: "srv",
            roots: vec![(seed("r", 0), None)],
        };
        let (Body::Tree(tree), _) = answer.into_body() else {
            panic!("ツリー");
        };
        assert!(tree.expanded().is_empty());
        assert!(tree.node(1).is_some_and(|n| n.children.is_none()));
    }

    #[test]
    fn 失敗はどれも理由と次の一手を持つ() {
        for error in [
            GotoError::Unsupported { server: "s" },
            GotoError::NoServer,
            GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: true,
            },
        ] {
            let out = error_json(&error, XrefKind::Supertypes);
            assert_eq!(out["kind"], json!("supertypes"));
            for key in ["reason", "next_step"] {
                let s = out[key].as_str().unwrap_or("");
                assert!(!s.is_empty() && !s.contains('{'), "{error:?} {key}: {s}");
            }
        }
    }

    #[test]
    fn 種類の呼び名は日英とも_5_つ別() {
        for lang in [Lang::Ja, Lang::En] {
            let mut labels: Vec<&str> = text::XREF_KIND_LABELS
                .iter()
                .map(|n| n.text_in(lang))
                .collect();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(labels.len(), XrefKind::ALL.len());
        }
    }

    #[test]
    fn 上限は_env_で変えられ不正は既定へ落ちる() {
        // env を触るテストはこの 1 本だけ
        std::env::set_var("TAKO_LSP_XREF_TIMEOUT_SECS", "x");
        assert_eq!(xref_timeout(), DEFAULT_XREF_TIMEOUT);
        std::env::set_var("TAKO_LSP_XREF_TIMEOUT_SECS", "5");
        assert_eq!(xref_timeout(), Duration::from_secs(5));
        std::env::remove_var("TAKO_LSP_XREF_TIMEOUT_SECS");
        assert_eq!(xref_timeout(), DEFAULT_XREF_TIMEOUT);
    }
}

//! ワークスペース横断検索（FR-3.43 / #1694）の制御プレーン側: ジョブと応答の形
//!
//! 走査と照合の本体は `tako_core::workspace_search`。ここは dispatch の 3 段
//! （`prepare_offload` → background → `finish_offload`）が受け渡すジョブと、CLI / MCP /
//! GUI が読む応答の JSON を持つ。探す範囲の解決（workspace を読む）と結果の行を開く着地
//! （`open_file` を通す）は dispatch 側にある。
//!
//! **走査は [`WorkspaceSearchJob::run`] の中だけ**（UI スレッドで呼ばない）。番犬
//! `issue1694_workspace_search_watchdog` が、UI スレッドで走る経路から走査の口
//! （`workspace_search::search(`）が呼ばれていないことを縛る。

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};
use tako_core::workspace_search::{self, Matcher, Options, Outcome, Query, QueryError};

/// CLI / MCP が共有する action の綴り（既定は `search`）
pub const ACTIONS: [&str; 4] = ["search", "results", "open", "clear"];
/// 応答に載せる行の既定（`limit` 省略時。0 = 保持しているぶん全部）
pub const DEFAULT_LIMIT: usize = 100;

/// background で走らせる 1 回の検索
pub struct WorkspaceSearchJob {
    pub(crate) query: Query,
    pub(crate) options: Options,
    pub(crate) cancel: Arc<AtomicBool>,
    /// 右パネルへ出す検索の番号（`SearchStore::begin` が振った。出さない検索は `None`）
    pub(crate) show: Option<u64>,
    pub(crate) limit: Option<usize>,
}

impl WorkspaceSearchJob {
    /// 照合器を組んで探す（**UI スレッドで呼ばない**。上限は `Options::time_limit`）
    pub fn run(self) -> WorkspaceSearchLanding {
        let result = Matcher::new(&self.query)
            .map(|matcher| workspace_search::search(&matcher, &self.options, &self.cancel));
        WorkspaceSearchLanding {
            query: self.query,
            roots: self.options.roots,
            show: self.show,
            limit: self.limit,
            result,
        }
    }
}

/// 探した結果（出す検索は UI スレッドの `workspace_search_land` が置き場へ置く）
pub struct WorkspaceSearchLanding {
    pub(crate) query: Query,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) show: Option<u64>,
    pub(crate) limit: Option<usize>,
    pub(crate) result: Result<Outcome, QueryError>,
}

impl WorkspaceSearchLanding {
    /// 出さない検索の応答（置き場に触らない = background からそのまま返せる）
    pub fn reply(&self) -> Result<Value, crate::DispatchError> {
        match &self.result {
            Ok(outcome) => Ok(outcome_json(&self.query, &self.roots, outcome, self.limit)),
            Err(e) => Err(crate::DispatchError::InvalidParams(e.reason())),
        }
    }
}

/// 結果の応答（`search` と `results` が同じ形を返す）
pub fn outcome_json(
    query: &Query,
    roots: &[PathBuf],
    outcome: &Outcome,
    limit: Option<usize>,
) -> Value {
    let total = outcome.hits.len();
    let limit = match limit.unwrap_or(DEFAULT_LIMIT) {
        0 => total,
        n => n.min(total),
    };
    let items: Vec<Value> = outcome
        .hits
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, hit)| {
            json!({
                "id": index + 1,
                "path": hit.path.display().to_string(),
                "line": hit.line,
                "column": hit.column,
                "excerpt": hit.excerpt,
            })
        })
        .collect();
    json!({
        "status": if total == 0 { "none" } else { "found" },
        "query": query.text,
        "regex": query.regex,
        "case_sensitive": query.case_sensitive,
        "roots": roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
        "total": total,
        "occurrences": outcome.occurrences,
        "files_matched": outcome.files_matched,
        "files_searched": outcome.files_searched,
        "skipped": {
            "binary": outcome.skipped.binary,
            "too_large": outcome.skipped.too_large,
            "unreadable": outcome.skipped.unreadable,
        },
        "stopped": outcome.stopped.map(|s| s.slug()),
        "elapsed_ms": outcome.elapsed.as_millis() as u64,
        "items": items,
        // 応答に載せなかった行（全部は limit=0 / 右パネル）
        "truncated": total - limit,
    })
}

/// 右パネルの結果の応答（`results`）。置き場の状態（走っているか・失敗したか・空か）も載せる
pub fn store_json(store: &workspace_search::SearchStore, limit: Option<usize>) -> Value {
    let running = store.running().map(|r| {
        json!({
            "query": r.query.text,
            "regex": r.query.regex,
            "case_sensitive": r.query.case_sensitive,
        })
    });
    let mut out = if let Some(shown) = store.shown() {
        outcome_json(&shown.query, &shown.roots, &shown.outcome, limit)
    } else if let Some((query, error)) = store.error() {
        json!({
            "status": "error",
            "query": query.text,
            "regex": query.regex,
            "case_sensitive": query.case_sensitive,
            "reason": error.reason(),
        })
    } else {
        json!({ "status": if running.is_some() { "searching" } else { "empty" } })
    };
    out["running"] = running.unwrap_or(Value::Null);
    out["retained"] = json!(store.retained());
    out
}

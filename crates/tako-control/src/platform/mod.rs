//! プラットフォーム依存を閉じ込める抽象境界（`tako-control` 分）。
//!
//! 設計の正は `.agent/plans/2026-07-windows-port-architecture.md`。
//! 原則: **`cfg(target_os)` / `cfg(unix)` を書いてよいのはこのモジュール配下だけ**。
//! 呼び出し側（dispatch / remote / CLI）は単一のコードパスを持つ。
//!
//! 新しくプラットフォーム分岐が必要になったら、呼び出し側に `cfg` を足すのではなく
//! ここに境界を追加する。

pub mod local_endpoint;
pub mod os_integration;
pub mod process;

use serde_json::{json, Value};
use tako_core::platform::support::{self, Platform};

/// プラットフォーム対応マトリクスの参照結果を組み立てる。
///
/// CLI（`tako platform`）と MCP（`tako_platform`）の**両方がこの 1 本を通る**ので、
/// 表示が食い違うことがない。`platform` 省略時は実行中のプラットフォーム、
/// `status` 省略時は全件を返す
pub fn report(platform: Option<&str>, status: Option<&str>) -> Result<Value, String> {
    let target = match platform {
        Some(p) => Platform::parse(p)
            .ok_or_else(|| format!("未知のプラットフォーム: {p}（macos / windows）"))?,
        None => Platform::current(),
    };
    if let Some(s) = status {
        const KNOWN: [&str; 4] = ["supported", "degraded", "pending", "unsupported"];
        if !KNOWN.contains(&s) {
            return Err(format!("未知の状態: {s}（{}）", KNOWN.join(" / ")));
        }
    }
    let features: Vec<Value> = support::features(target, status)
        .into_iter()
        .map(|(f, s)| {
            let mut o = json!({ "key": f.key, "status": s.status() });
            // 理由文は表示言語に追従する（マトリクス 1 箇所定義・#435）
            if let Some(note) = s.note() {
                o["note"] = json!(note.text());
            }
            if let Some(issue) = s.issue() {
                o["issue"] = json!(issue);
            }
            o
        })
        .collect();

    let counts = ["supported", "degraded", "pending", "unsupported"]
        .iter()
        .map(|s| {
            (
                s.to_string(),
                json!(support::features(target, Some(s)).len()),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    Ok(json!({
        "platform": target.as_str(),
        "current": Platform::current().as_str(),
        "filter": status,
        "counts": counts,
        "total": features.len(),
        "features": features,
    }))
}

//! agent 能力マトリクス（#982）の参照結果の組み立て。
//!
//! 正本は `tako_core::agent_support`。ここは **CLI（`tako agent-support`）と
//! MCP（`tako_agent_support`）が同じ 1 本を通る**ようにするための薄い層で、
//! `tako_control::platform`（OS 軸）と対になっている。
//!
//! docs（`docs/src/content/docs/agent-support.md`）も
//! `tako agent-support --json` を読んで生成するので、
//! **宣言・診断・docs が常に同じ 1 つの事実を指す**。

use serde_json::{json, Value};

use tako_core::agent_support::{self as support, Agent};

const STATUSES: [&str; 4] = ["supported", "degraded", "pending", "unsupported"];

/// agent 能力マトリクスの参照結果を組み立てる。
///
/// `agent` 省略時は**全系統ぶんの表**（docs 生成と俯瞰用）、
/// 指定時はその系統だけを返す。`status` 省略時は全件。
pub fn report(agent: Option<&str>, status: Option<&str>) -> Result<Value, String> {
    let target = match agent {
        Some(a) => Some(Agent::parse(a).ok_or_else(|| {
            format!(
                "未知の agent: {a}（{}）",
                Agent::ALL
                    .iter()
                    .map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        })?),
        None => None,
    };
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("未知の状態: {s}（{}）", STATUSES.join(" / ")));
        }
    }

    // 表示対象の系統。単独指定でも同じ組み立てを通す（表示の食い違いを構造で防ぐ）
    let agents: Vec<Agent> = match target {
        Some(a) => vec![a],
        None => Agent::ALL.to_vec(),
    };

    let features: Vec<Value> = support::MATRIX
        .iter()
        .filter_map(|f| {
            // status を渡されたら「指定系統のどれかがその状態」の行だけ残す
            let cells: Vec<Value> = agents.iter().map(|a| cell(f, *a)).collect();
            if let Some(want) = status {
                let hit = agents.iter().any(|a| f.on(*a).status() == want);
                if !hit {
                    return None;
                }
            }
            let mut o = json!({
                "key": f.key,
                // 説明は表示言語に追従する分と、両言語ぶんの両方を返す。
                // **docs の生成物が実行環境の言語で変わってはいけない**
                // （#591 で実際に踏んだ: 手元は日本語・CI は英語で `--check` が落ちた）
                "summary": f.summary.text(),
                "summary_ja": f.summary.ja(),
                "summary_en": f.summary.en(),
                "evidence": f.evidence.kind(),
                "agents": serde_json::Map::from_iter(
                    agents.iter().map(|a| a.as_str().to_string()).zip(cells),
                ),
            });
            if let Some(citation) = f.evidence.citation() {
                o["evidence_detail"] = json!(citation);
            }
            Some(o)
        })
        .collect();

    // 系統ごとの内訳。**絞り込み前の全件**に対して数える（俯瞰の数字が
    // フィルタで変わると「何件中いくつ」が読めなくなる）
    let counts = serde_json::Map::from_iter(agents.iter().map(|a| {
        let per = serde_json::Map::from_iter(
            STATUSES
                .iter()
                .map(|s| (s.to_string(), json!(support::features(*a, Some(s)).len()))),
        );
        (a.as_str().to_string(), Value::Object(per))
    }));

    let mut out = json!({
        "agents": agents.iter().map(|a| json!({
            "key": a.as_str(),
            "label": a.label(),
            "baseline": a.is_baseline(),
        })).collect::<Vec<_>>(),
        "filter": status,
        "counts": counts,
        "total": features.len(),
        "matrix_total": support::MATRIX.len(),
        "features": features,
    });
    if let Some(a) = target {
        out["agent"] = json!(a.as_str());
        out["degraded_notes"] = json!(support::degraded_note_items(a)
            .into_iter()
            .map(|n| json!({ "ja": n.ja(), "en": n.en() }))
            .collect::<Vec<_>>());
    }
    Ok(out)
}

/// 1 マスぶんの JSON
fn cell(f: &support::AgentFeature, agent: Agent) -> Value {
    let s = f.on(agent);
    let mut o = json!({ "status": s.status() });
    if let Some(note) = s.note() {
        o["note"] = json!(note.text());
        o["note_ja"] = json!(note.ja());
        o["note_en"] = json!(note.en());
    }
    if let Some(issue) = s.issue() {
        o["issue"] = json!(issue);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs の生成物が**実行環境の言語で変わってはいけない**（#591 の教訓）。
    ///
    /// `summary` / `note` は表示言語に追従するので、生成器がそれを読むと
    /// 「日本語ロケールで生成 → 英語ロケールの CI で `--check` が落ちる」が起きる。
    /// 実際に踏んだので、言語に依存しない `*_ja` / `*_en` が必ず載ることを固定する。
    ///
    /// **言語グローバルを動かさない**のは、`tako-core` の `lang_guard` が
    /// クレート内限定で tako-control から排他できないため（動かすと #608 / #807 と
    /// 同型の並列競合フレークになる）。構造で担保する形にした
    #[test]
    fn 応答には言語に依存しない文言が載る() {
        let has_cjk = |s: &str| {
            s.chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF))
        };
        let v = report(None, None).expect("全系統の表を引けない");
        let mut notes_checked = 0;
        for f in v["features"].as_array().expect("features が配列でない") {
            let key = f["key"].as_str().unwrap_or_default();
            for field in ["summary_ja", "summary_en"] {
                let s = f[field].as_str().unwrap_or_default();
                assert!(!s.is_empty(), "{key} に {field} が無い");
            }
            assert!(
                !has_cjk(f["summary_en"].as_str().unwrap_or_default()),
                "{key} の summary_en に日本語が残っている"
            );
            for agent in ["claude", "codex", "agy", "local"] {
                let cell = &f["agents"][agent];
                if cell["status"] == "supported" {
                    continue;
                }
                let ja = cell["note_ja"].as_str().unwrap_or_default();
                let en = cell["note_en"].as_str().unwrap_or_default();
                assert!(!ja.is_empty(), "{key} / {agent} に note_ja が無い");
                assert!(!en.is_empty(), "{key} / {agent} に note_en が無い");
                assert!(!has_cjk(en), "{key} / {agent} の note_en に日本語: {en}");
                notes_checked += 1;
            }
        }
        assert!(
            notes_checked > 50,
            "縮退の検査数が少なすぎる: {notes_checked}"
        );
    }

    #[test]
    fn 全系統の表と単独指定が同じ判定を返す() {
        let all = report(None, None).unwrap();
        let codex = report(Some("codex"), None).unwrap();
        assert_eq!(all["total"], codex["total"]);
        let find = |v: &Value, key: &str| -> Value {
            v["features"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["key"] == key)
                .unwrap()["agents"]["codex"]
                .clone()
        };
        for f in all["features"].as_array().unwrap() {
            let key = f["key"].as_str().unwrap();
            assert_eq!(f["agents"]["codex"], find(&codex, key), "{key} で食い違い");
        }
        assert_eq!(codex["agent"], "codex");
        assert!(codex["degraded_notes"].as_array().unwrap().len() > 1);
    }

    #[test]
    fn 状態で絞り込める() {
        let pending = report(Some("codex"), Some("pending")).unwrap();
        let total = pending["total"].as_u64().unwrap();
        assert!(total > 0 && total < pending["matrix_total"].as_u64().unwrap());
        for f in pending["features"].as_array().unwrap() {
            assert_eq!(f["agents"]["codex"]["status"], "pending");
            assert!(f["agents"]["codex"]["issue"].as_u64().unwrap() > 0);
        }
        // 内訳は絞り込みに影響されない（俯瞰の数字を保つ）
        let all = report(Some("codex"), None).unwrap();
        assert_eq!(pending["counts"], all["counts"]);
    }

    #[test]
    fn 未知の入力は選択肢つきで拒否する() {
        let e = report(Some("gemini"), None).unwrap_err();
        assert!(e.contains("claude") && e.contains("local"), "{e}");
        let e = report(None, Some("broken")).unwrap_err();
        assert!(e.contains("supported"), "{e}");
    }

    /// 根拠が応答に載ること（#591 の evidence と同じ狙い。AI が「なぜそう言えるか」を読める）
    #[test]
    fn 根拠が応答に載る() {
        let r = report(None, None).unwrap();
        let mut with_detail = 0;
        for f in r["features"].as_array().unwrap() {
            let kind = f["evidence"].as_str().unwrap();
            assert!(
                [
                    "source",
                    "self-test",
                    "unit-test",
                    "measured",
                    "by-design",
                    "unverified"
                ]
                .contains(&kind),
                "未知の根拠種別: {kind}"
            );
            if kind != "unverified" {
                assert!(
                    f["evidence_detail"].is_string(),
                    "{} の根拠本文が無い",
                    f["key"]
                );
                with_detail += 1;
            }
        }
        assert!(with_detail > 30, "根拠つきの行が少なすぎる: {with_detail}");
    }
}

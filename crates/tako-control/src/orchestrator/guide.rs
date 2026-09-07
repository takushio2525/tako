//! master の手順書（Issue #1154）
//!
//! master / solo の system prompt は**長寿命セッションが起動した瞬間に払う固定費**で、
//! #1139 の棚卸しでは tako リポジトリ最大の項目（takodev の生成物 59,501 B ≒ 4 万トークン）
//! だった。そこで手順の詳細 — monitoring のイベント別対処表・acceptance の手順・
//! worker prompt テンプレート・引き継ぎの書き方 — を prompt から出し、
//! **引く条件が満たされたときだけ**取得する形へ移した（#322 の「既定を賢く」と同じ方向）。
//!
//! # 不変条件
//!
//! - **本文は 1 文字も捨てない**。ここに置いてあるのは prompt から移した原文そのままで、
//!   欠落ゼロは `crates/tako-control/tests/prompt_guides.rs` が
//!   変更前テンプレート全文（fixture）と突き合わせて拘束する
//! - **正本は 1 箇所**。prompt 側の案内 1 行と、この本文は同じ 1 実装
//!   （`GUIDES`）から topic 名を引く。テンプレートの topic 表と `GUIDES` の一致も
//!   番犬が見るので、消えた topic を案内したままにはならない
//! - **プレースホルダは prompt と同じ解決を通す**（`{CTX_THRESHOLD}` 等）。
//!   `Profile::render_prompt_placeholders` の 1 実装を prompt / solo / guide が共有する
//!
//! # A/B（`TAKO_1154_LEGACY=1`）
//!
//! 立てると `build_from_template` が **各 guide の本文を prompt へ差し戻す**ので、
//! 同一バイナリで変更前の量・内容へ戻せる（`tako_core::context_budget::legacy_1154`
//! が同時に system prompt の予算も外す）。差し戻しは本文と量を戻すもので、
//! **原文の並び順とは一致しない**（`handoff` は `behavior` の一部を切り出した派生 topic
//! なので、差し戻しでは二重に出さない）。

use serde_json::{json, Value};
use tako_core::context_budget as budget;
use tako_core::platform::support::Note;

/// 一覧応答に添える案内（理由文は表示言語に追従し、日英も併せて返す。#591 と同じ作法）
const HINT: Note = Note::new(
    "topic を指定するとその全文が返る（prompt から移した原文そのまま）",
    "Pass a topic to get its full text — the same text that was moved out of the prompt",
);

/// 手順書 1 本
#[derive(Debug, Clone, Copy)]
pub struct Guide {
    /// 引くときの名前（prompt の topic 表・CLI・MCP がこれで指す）
    pub topic: &'static str,
    /// 表示用の見出し
    pub title: &'static str,
    /// `TAKO_1154_LEGACY=1` のときに本文を差し戻すブロック名（先頭のブロックの位置へ入る）。
    /// **空なら派生 topic** = 別の guide の一部を切り出したものなので差し戻しには参加しない
    pub restores: &'static [&'static str],
    /// 移した本文（原文そのまま）
    pub body: &'static str,
}

/// 手順書の一覧（**正本**。prompt の topic 表と一致していることを番犬が見る）
pub const GUIDES: &[Guide] = &[
    Guide {
        topic: "context-budget",
        title: "Startup Load Budget",
        restores: &["context-budget"],
        body: include_str!("guides/context-budget.md"),
    },
    Guide {
        topic: "task-intake",
        title: "Task Intake",
        restores: &["task-intake"],
        body: include_str!("guides/task-intake.md"),
    },
    Guide {
        topic: "worker-prompt",
        title: "Worker Prompt Template",
        restores: &["worker-prompt-template"],
        body: include_str!("guides/worker-prompt.md"),
    },
    Guide {
        topic: "spawning",
        title: "Running and Spawning Workers",
        restores: &["running-workers", "spawning-workers"],
        body: include_str!("guides/spawning.md"),
    },
    Guide {
        topic: "monitoring",
        title: "Monitoring Workers",
        restores: &["monitoring"],
        body: include_str!("guides/monitoring.md"),
    },
    Guide {
        topic: "acceptance",
        title: "Acceptance Inspection",
        restores: &["acceptance"],
        body: include_str!("guides/acceptance.md"),
    },
    Guide {
        topic: "lifecycle",
        title: "Worker Lifecycle Management",
        restores: &["lifecycle"],
        body: include_str!("guides/lifecycle.md"),
    },
    Guide {
        topic: "handoff",
        // behavior の項目 8（引き継ぎ）だけを切り出した派生 topic。
        // 閾値超過は master が最も頻繁に踏む経路なので、10 KB の behavior 全文を
        // 引かずに済むよう単独で取れるようにしてある
        title: "Handing Off Before Your Context Runs Out",
        restores: &[],
        body: include_str!("guides/handoff.md"),
    },
    Guide {
        topic: "tools",
        title: "Available Tools, Worker Status and Projects",
        restores: &["tools", "worker-status", "projects"],
        body: include_str!("guides/tools.md"),
    },
    Guide {
        topic: "quality-ops",
        title: "Quality Operations",
        restores: &["quality-ops"],
        body: include_str!("guides/quality-ops.md"),
    },
    Guide {
        topic: "behavior",
        title: "Behavioral Principles",
        restores: &["behavior"],
        body: include_str!("guides/behavior.md"),
    },
];

/// topic 名の正規化（大文字小文字と `_` / `-` の違いを吸収する）
fn normalize(topic: &str) -> String {
    topic.trim().to_ascii_lowercase().replace('_', "-")
}

/// topic で引く
pub fn find(topic: &str) -> Option<&'static Guide> {
    let want = normalize(topic);
    GUIDES.iter().find(|g| normalize(g.topic) == want)
}

/// 引ける topic の一覧（案内文に使う）
pub fn topics() -> Vec<&'static str> {
    GUIDES.iter().map(|g| g.topic).collect()
}

/// `TAKO_1154_LEGACY=1` のとき、このブロックの位置へ差し戻す本文
/// （`restores` の**先頭**に一致する guide だけ。同じ本文を 2 度出さない）
pub fn restored_at_block(block: &str) -> impl Iterator<Item = &'static Guide> {
    let block = block.to_string();
    GUIDES
        .iter()
        .filter(move |g| g.restores.first().is_some_and(|b| *b == block))
}

/// master が到達できる本文の全体（system prompt テンプレート + 全 topic の本文）。
///
/// 「この手順が書かれていること」を確かめる番犬はここを見る。#1154 で手順は prompt から
/// 手順書へ移ったので、prompt だけを検査すると**記述の存在を見落とす**
/// （そして「prompt に無いから足す」と量が戻る）。プレースホルダは未解決のまま返す
pub fn master_corpus() -> String {
    let mut s = String::from(super::DEFAULT_SYSTEM_PROMPT);
    for g in GUIDES {
        s.push('\n');
        s.push_str(g.body);
    }
    s
}

/// CLI / MCP が返す JSON。`topic` 省略で一覧、指定で全文。
///
/// プレースホルダはプロファイルで解決するので、prompt に焼かれている値
/// （引き継ぎ閾値・タブ命名規約）と guide の記述がずれない
pub fn json(topic: Option<&str>, profile_name: &str) -> Result<Value, String> {
    let profile = super::load_profile_of(super::ProfileKind::Master, profile_name)
        .unwrap_or_else(|_| super::Profile::default());
    match topic.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(json!({
            "profile": profile_name,
            "count": GUIDES.len(),
            "topics": GUIDES
                .iter()
                .map(|g| {
                    let text = profile.render_prompt_placeholders(g.body);
                    json!({
                        "topic": g.topic,
                        "title": g.title,
                        "bytes": text.len(),
                        "lines": text.lines().count(),
                        "est_tokens": budget::estimate_tokens(&text),
                    })
                })
                .collect::<Vec<_>>(),
            "hint": HINT.text(),
            "hint_ja": HINT.ja(),
            "hint_en": HINT.en(),
        })),
        Some(t) => {
            let g = find(t).ok_or_else(|| {
                format!("未知の topic: {t}（引けるのは {}）", topics().join(" / "))
            })?;
            let text = profile.render_prompt_placeholders(g.body);
            Ok(json!({
                "profile": profile_name,
                "topic": g.topic,
                "title": g.title,
                "bytes": text.len(),
                "lines": text.lines().count(),
                "est_tokens": budget::estimate_tokens(&text),
                "text": text,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topicは重複せず本文が空でない() {
        let mut seen = std::collections::BTreeSet::new();
        for g in GUIDES {
            assert!(seen.insert(normalize(g.topic)), "topic が重複: {}", g.topic);
            assert!(!g.body.trim().is_empty(), "{} の本文が空", g.topic);
            assert!(!g.title.trim().is_empty(), "{} の見出しが空", g.topic);
        }
    }

    #[test]
    fn topicの表記ゆれを吸収する() {
        assert_eq!(find("monitoring").unwrap().topic, "monitoring");
        assert_eq!(find("Monitoring").unwrap().topic, "monitoring");
        assert_eq!(find("quality_ops").unwrap().topic, "quality-ops");
        assert_eq!(find(" worker-prompt ").unwrap().topic, "worker-prompt");
        assert!(find("nope").is_none());
    }

    #[test]
    fn 未知のtopicは引ける一覧を返す() {
        let err = json(Some("nope"), "default").unwrap_err();
        for t in topics() {
            assert!(err.contains(t), "案内に {t} が無い: {err}");
        }
    }

    #[test]
    fn 派生topicは差し戻しに参加しない() {
        // handoff は behavior の一部なので、差し戻しで二重に出してはいけない
        let at_behavior: Vec<&str> = restored_at_block("behavior").map(|g| g.topic).collect();
        assert_eq!(at_behavior, vec!["behavior"]);
        assert!(restored_at_block("worker-status").next().is_none());
        assert_eq!(
            restored_at_block("tools")
                .map(|g| g.topic)
                .collect::<Vec<_>>(),
            vec!["tools"]
        );
    }

    #[test]
    fn 一覧と全文のjson() {
        let list = json(None, "default").unwrap();
        assert_eq!(list["count"], GUIDES.len());
        assert_eq!(list["topics"].as_array().unwrap().len(), GUIDES.len());
        let one = json(Some("acceptance"), "default").unwrap();
        assert_eq!(one["topic"], "acceptance");
        assert!(one["text"]
            .as_str()
            .unwrap()
            .contains("Acceptance Inspection"));
        assert_eq!(
            one["bytes"].as_u64().unwrap() as usize,
            one["text"].as_str().unwrap().len()
        );
    }

    #[test]
    fn 本文のプレースホルダはプロファイルで解決される() {
        // 引き継ぎ手順に `{CTX_THRESHOLD}` が残ったまま渡ると、master は自分の閾値を
        // 「{CTX_THRESHOLD}%」だと読む。prompt と同じ解決を通す
        let raw = find("handoff").unwrap().body;
        assert!(raw.contains("{CTX_THRESHOLD}"), "原文には残っている");
        let out = json(Some("handoff"), "default").unwrap();
        let text = out["text"].as_str().unwrap();
        assert!(
            !text.contains("{CTX_THRESHOLD}"),
            "解決されずに渡っている: {text:.200}"
        );
        let profile = super::super::Profile::default();
        assert!(text.contains(&format!(
            "{}% context usage",
            profile.resolved_ctx_threshold().value
        )));
    }
}

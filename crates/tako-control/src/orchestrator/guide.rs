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
//! 立てると `build_from_template` が **各 topic の本文を prompt へ差し戻す**ので、
//! 同一バイナリで「移送しなかった場合」の prompt を作れる。**予算は外さない**ので、
//! そのとき番犬（`crates/tako-control/tests/context_budget.rs`）が超過で落ちるのが
//! 「移送しなければ予算を超えていた」ことの実測になる。
//!
//! 差し戻しは本文と量を戻すもので、移送前の prompt の byte 再現ではない
//! （案内文は残るので原文より大きくなる。`handoff` は `behavior` の一部を切り出した
//! 派生 topic なので、差し戻しでは二重に出さない）。

use serde_json::{json, Value};
use tako_core::context_budget as budget;
use tako_core::platform::support::Note;
use tako_core::prompt_append;

/// 一覧応答に添える案内（理由文は表示言語に追従し、日英も併せて返す。#591 と同じ作法）
const HINT: Note = Note::new(
    "topic を指定するとその全文が返る（prompt から移した原文そのまま）",
    "Pass a topic to get its full text — the same text that was moved out of the prompt",
);

/// 手順書の本文（Issue #1477 で 2 種になった）
#[derive(Debug, Clone, Copy)]
pub enum GuideBody {
    /// prompt から移した原文そのまま。**移送の番犬（`prompt_guides.rs`）が見るのはこちら**
    Static(&'static str),
    /// プロファイルと利用者のファイルから組み立てる本文（#1477）。
    ///
    /// tako が書いた散文ではない（個人のルール・委任方針・ledger 由来の既定）ので
    /// 移送の番犬の対象外。代わりに**「常時部だけが prompt に載る」**ことを
    /// `prompt_budget_1477.rs` が拘束する
    Dynamic(fn(&super::Profile) -> String),
}

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
    /// 移した本文
    pub body: GuideBody,
}

impl Guide {
    /// 静的な本文（動的 guide なら `None`）。移送の番犬と差し戻しが使う
    pub fn static_body(&self) -> Option<&'static str> {
        match self.body {
            GuideBody::Static(s) => Some(s),
            GuideBody::Dynamic(_) => None,
        }
    }

    /// プレースホルダ解決前の本文（動的 guide はここで組み立てる）
    pub fn body_raw(&self, profile: &super::Profile) -> String {
        match self.body {
            GuideBody::Static(s) => s.to_string(),
            GuideBody::Dynamic(f) => f(profile),
        }
    }
}

// ─── 動的な本文（#1477） ──────────────────────────────────────────────

/// `delegation`: 委任の判断材料。**prompt の固定費から外した分**（#1477）。
///
/// 中身は「プロファイルの `delegate_guidance`」+「`ledger::build_judgment_section()`
/// （組み込み既定 + 利用者の `judgment-local.md` + 調査頻度の制御）」で、
/// どちらも**利用者の設定から組み立てる**。prompt には引く条件だけが残る
fn delegation_body(profile: &super::Profile) -> String {
    let mut out = String::new();
    if let Some(g) = profile.delegate_guidance_text() {
        out.push_str("## Delegation Guidance (this profile)\n\n");
        out.push_str(g.trim());
        out.push('\n');
    }
    out.push_str(super::ledger::build_judgment_section().trim_start_matches('\n'));
    out.push('\n');
    out
}

/// 追記が無い / 全部が常時部のときの案内（本文は空にしない）
const NO_LOCAL_RULES: &str = "This profile has no on-demand local rules: either \
`prompt_blocks.append` is unset, or the whole append is small enough to stay in the \
prompt every turn. Everything your operator wrote is already above.\n";

/// `local-rules`: 追記（`prompt_blocks.append`）の on-demand 部（#1477）。
///
/// 区切り `<!-- tako:on-demand -->` より後ろがここへ来る。prompt に載るのは
/// 常時部と**自動生成の索引 1 行**だけなので、詳細が要るときにここを引く
fn local_rules_body(profile: &super::Profile) -> String {
    let Some(raw) = profile.prompt_append_text() else {
        return NO_LOCAL_RULES.to_string();
    };
    let part = prompt_append::split(&raw);
    if part.on_demand.trim().is_empty() {
        return NO_LOCAL_RULES.to_string();
    }
    part.on_demand.to_string()
}

/// `platform`: この環境で使えない・機能が落ちる操作の理由（#1571）。
///
/// 本文は対応マトリクス（`tako_core::platform::support`）からの生成物なので、
/// **縮退が 1 件増えるたびに伸びる**。prompt へ全文載せていた頃は Windows で
/// 4110 バイトあり、tako が作る側が取り分（18944 バイト）を 1.7〜2.7 KB 超えて
/// 利用者の追記の取り分を削っていた。prompt に残すのは件数と引き方だけ（#1154 の作法）。
///
/// プロファイルには依らない（見るのは実行中のプラットフォーム）が、tako の
/// リポジトリに本文が無い生成物なので移送の番犬の対象外 = `Dynamic` で持つ
fn platform_body(_profile: &super::Profile) -> String {
    crate::platform::facts::PlatformFacts::current().full_section()
}

/// 手順書の一覧（**正本**。prompt の topic 表と一致していることを番犬が見る）
pub const GUIDES: &[Guide] = &[
    Guide {
        topic: "context-budget",
        title: "Startup Load Budget",
        restores: &["context-budget"],
        body: GuideBody::Static(include_str!("guides/context-budget.md")),
    },
    Guide {
        topic: "task-intake",
        title: "Task Intake",
        restores: &["task-intake"],
        body: GuideBody::Static(include_str!("guides/task-intake.md")),
    },
    Guide {
        topic: "worker-prompt",
        title: "Worker Prompt Template",
        restores: &["worker-prompt-template"],
        body: GuideBody::Static(include_str!("guides/worker-prompt.md")),
    },
    Guide {
        topic: "spawning",
        title: "Running and Spawning Workers",
        // #1477 で `no-investigate` の偵察 3 手順もここへ移した（差し戻しの位置は
        // 先頭の `running-workers` のままなので A/B の組み立て順は変わらない）
        restores: &["running-workers", "spawning-workers", "no-investigate"],
        body: GuideBody::Static(include_str!("guides/spawning.md")),
    },
    Guide {
        topic: "monitoring",
        title: "Monitoring Workers",
        restores: &["monitoring"],
        body: GuideBody::Static(include_str!("guides/monitoring.md")),
    },
    Guide {
        topic: "acceptance",
        title: "Acceptance Inspection",
        restores: &["acceptance"],
        body: GuideBody::Static(include_str!("guides/acceptance.md")),
    },
    Guide {
        topic: "lifecycle",
        title: "Worker Lifecycle Management",
        restores: &["lifecycle"],
        body: GuideBody::Static(include_str!("guides/lifecycle.md")),
    },
    Guide {
        topic: "handoff",
        // behavior の項目 8（引き継ぎ）だけを切り出した派生 topic。
        // 閾値超過は master が最も頻繁に踏む経路なので、10 KB の behavior 全文を
        // 引かずに済むよう単独で取れるようにしてある
        title: "Handing Off Before Your Context Runs Out",
        restores: &[],
        body: GuideBody::Static(include_str!("guides/handoff.md")),
    },
    Guide {
        topic: "remote",
        // #1004 で新しく足した topic（移送ではないので差し戻しには参加しない）。
        // リモート / SSH は「AI が自分でやる半分」と「ユーザーに渡す半分」の境目が
        // 手順の要点で、prompt 側には引く条件だけを置く
        title: "Remote Folders and SSH Setup",
        restores: &[],
        body: GuideBody::Static(include_str!("guides/remote.md")),
    },
    Guide {
        topic: "user-tasks",
        // #1450 で新しく足した topic（移送ではないので差し戻しには参加しない）。
        // prompt 側には「ユーザーの手が要るものはここへ起票する」という条件だけを置き、
        // 種類の選び方・返答の扱い・本文の書き方はここへ置く
        title: "Asking the User: Filing Tasks a Human Must Do",
        restores: &[],
        body: GuideBody::Static(include_str!("guides/user-tasks.md")),
    },
    Guide {
        topic: "tools",
        title: "Available Tools, Worker Status and Projects",
        restores: &["tools", "worker-status", "projects"],
        body: GuideBody::Static(include_str!("guides/tools.md")),
    },
    Guide {
        topic: "quality-ops",
        title: "Quality Operations",
        restores: &["quality-ops"],
        body: GuideBody::Static(include_str!("guides/quality-ops.md")),
    },
    Guide {
        topic: "behavior",
        title: "Behavioral Principles",
        restores: &["behavior"],
        body: GuideBody::Static(include_str!("guides/behavior.md")),
    },
    Guide {
        topic: "delegation",
        // #1477 で model-policy から外した分。`restores` を空にしてあるのは、
        // 差し戻し（A/B）を `TAKO_1477_LEGACY` 側で **生成と同じ 1 実装**
        // （generate_model_policy_section）が行うため。ここから戻すと
        // 「移送前の prompt」と組み立て順がずれる
        title: "Choosing a Worker's Model and Effort",
        restores: &[],
        body: GuideBody::Dynamic(delegation_body),
    },
    Guide {
        topic: "local-rules",
        // #1477: 利用者の追記の on-demand 部。本文は利用者のファイルなので
        // tako のリポジトリには無い（= 移送の番犬の対象外）
        title: "This Machine's Local Rules (on-demand part)",
        restores: &[],
        body: GuideBody::Dynamic(local_rules_body),
    },
    Guide {
        topic: "platform",
        // #1571: 縮退の理由の全文。差し戻し（A/B）は `TAKO_1571_LEGACY` 側で
        // **生成と同じ 1 実装**（`PlatformFacts::notes_section_in`）が行うので
        // `restores` は空（ここから戻すと platform 片が二重に出る）
        title: "What Is Degraded on This Platform",
        restores: &[],
        body: GuideBody::Dynamic(platform_body),
    },
];

/// #1154 の A/B: 立てると各 topic の本文を prompt へ差し戻す（= 移送前の中身に戻す）。
/// 予算は外さないので、番犬が超過で落ちることが「移送の効き」の実測になる
pub fn legacy_restore() -> bool {
    std::env::var_os("TAKO_1154_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// #1477 の A/B: 立てると **移送前の姿**へ戻す —— 委任の判断材料
/// （`delegate_guidance` + judgment）を model-policy へ inline し、
/// 追記（`prompt_blocks.append`）を分割せず全文 prompt へ載せる。
/// 予算は外さないので、そのとき番犬が超過で落ちるのが「移送の効き」の実測になる
pub fn legacy_1477() -> bool {
    std::env::var_os("TAKO_1477_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

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
    for body in GUIDES.iter().filter_map(Guide::static_body) {
        s.push('\n');
        s.push_str(body);
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
                    let text = profile.render_prompt_placeholders(&g.body_raw(&profile));
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
            let text = profile.render_prompt_placeholders(&g.body_raw(&profile));
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
        // 動的 guide（#1477）も**設定が空の既定プロファイル**で本文を持つ
        // （空を返すと「引いたのに何も無い」= 引き方を誤ったのか設定が無いのか判らない）
        let profile = super::super::Profile::default();
        let mut seen = std::collections::BTreeSet::new();
        for g in GUIDES {
            assert!(seen.insert(normalize(g.topic)), "topic が重複: {}", g.topic);
            assert!(
                !g.body_raw(&profile).trim().is_empty(),
                "{} の本文が空",
                g.topic
            );
            assert!(!g.title.trim().is_empty(), "{} の見出しが空", g.topic);
        }
    }

    #[test]
    fn 動的guideは設定から組み立てる() {
        use super::super::{PromptBlocks, WorkerModelPolicy};
        let p = super::super::Profile {
            worker_model_policy: WorkerModelPolicy::Delegate,
            delegate_guidance: Some("UI は上位モデルへ".into()),
            prompt_blocks: Some(PromptBlocks {
                append: Some("常時\n<!-- tako:on-demand -->\n## 詳細\n本文\n".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let d = find("delegation").unwrap().body_raw(&p);
        assert!(d.contains("UI は上位モデルへ"), "{d}");
        assert!(d.contains("Delegation Judgment Criteria"), "{d}");
        assert!(d.contains("bugfix-rooted"), "{d}");
        let l = find("local-rules").unwrap().body_raw(&p);
        assert_eq!(l, "## 詳細\n本文\n", "区切りより後ろだけ");
        // 追記が無いプロファイルでも案内を返す（空にしない）
        let none = find("local-rules")
            .unwrap()
            .body_raw(&super::super::Profile::default());
        assert!(none.contains("no on-demand local rules"), "{none}");
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
        let raw = find("handoff").unwrap().static_body().unwrap();
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

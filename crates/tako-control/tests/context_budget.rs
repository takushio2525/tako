//! 起動時ロードの予算の番犬（Issue #1139）
//!
//! **規約を書いただけでは守られない**（`~/.claude/CLAUDE.md` に「30 件超で archive を
//! 提案する」と書いてあったのに 3 か月・332 エントリ積もった）ので、
//! tako リポジトリ自身の作業ログを `tako_core::context_budget` の**判定と同じ 1 本**で
//! 検査して CI で落とす。
//!
//! 併せて、tako が配る規約文（3 か所）が予算表と一致していることを固定する。
//! 数値を手で書くと必ずずれるので、規約文は予算表から**生成**して埋め込む。

use std::path::{Path, PathBuf};
use tako_core::context_budget as budget;
use tako_core::context_budget::{ItemKind, Metric};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// CI が落とす軸。**1 エントリの行数は入れない** — 本文を削るのは要約の捏造になり
/// 自動では直せないので、`tako context-budget` が提案として名指しするにとどめる
fn enforced(metric: Metric) -> bool {
    matches!(metric, Metric::Bytes | Metric::Entries | Metric::WorkDays)
}

#[test]
fn 作業ログが起動時ロードの予算に収まっている() {
    let text = read(".agent/progress.md");
    let m = budget::measure(ItemKind::ProgressLog, &text);
    let over: Vec<String> = budget::violations(ItemKind::ProgressLog, &m)
        .into_iter()
        .filter(|v| enforced(v.metric))
        .map(|v| {
            format!(
                "  {} が {} で上限 {} を超えている（{}）",
                v.metric.as_str(),
                v.actual,
                v.limit,
                v.note.ja()
            )
        })
        .collect();
    assert!(
        over.is_empty(),
        ".agent/progress.md が起動時ロードの予算を超えている\n{}\n\
         直し方: cargo run -p tako-cli -- context-budget fix\n\
         （古いエントリが .agent/progress-archive.md へ 1 行で移る。本文は git 履歴に残る）",
        over.join("\n")
    );
}

#[test]
fn エージェント規約が起動時ロードの予算に収まっている() {
    // `AGENTS.md` は `CLAUDE.md` の 1 行リダイレクト経由で**毎ターン全文が載る**。
    // 長い注記・実測・罠は `.agent/commands.md` のような別ファイルへ出し、
    // 規約からはバックティック参照で案内する（`@import` にはしない）
    let text = read("AGENTS.md");
    let m = budget::measure(ItemKind::AgentsGuide, &text);
    let over: Vec<String> = budget::violations(ItemKind::AgentsGuide, &m)
        .into_iter()
        .filter(|v| enforced(v.metric))
        .map(|v| {
            format!(
                "  {} が {} で上限 {} を超えている（{}）",
                v.metric.as_str(),
                v.actual,
                v.limit,
                v.note.ja()
            )
        })
        .collect();
    assert!(
        over.is_empty(),
        "AGENTS.md が起動時ロードの予算を超えている\n{}\n\
         直し方: 長い節を `.agent/` 配下の別ファイルへ移し、バックティック参照で案内する",
        over.join("\n")
    );
}

/// #1154: tako が配る **既定 blocks だけ**で組んだ system prompt が予算に収まっていること。
///
/// これは tako 自身の生成物なので、超えたらユーザーに我慢させるのではなく tako を直す。
/// ユーザー側の追記（`prompt_blocks.append`）は環境によって量が違うのでここでは見ない
/// （実環境の合計は `tako context-budget` が測り、超過は `proposals` で block 別に出る）
#[test]
fn 既定のsystem_promptが起動時ロードの予算に収まっている() {
    use tako_control::orchestrator::{Profile, PromptMode};
    // 素のプロファイル（追記なし）= tako が配る既定の姿
    let profile = Profile::default();
    for (label, template, mode) in [
        (
            "master",
            tako_control::orchestrator::DEFAULT_SYSTEM_PROMPT,
            PromptMode::Master,
        ),
        (
            "solo",
            tako_control::orchestrator::SOLO_SYSTEM_PROMPT,
            PromptMode::Solo,
        ),
    ] {
        let pieces = profile.build_prompt_pieces(template, "default", mode);
        let text = tako_control::orchestrator::join_prompt_pieces(&pieces);
        let m = budget::measure(ItemKind::SystemPrompt, &text);
        let over: Vec<String> = budget::violations(ItemKind::SystemPrompt, &m)
            .into_iter()
            .filter(|v| enforced(v.metric))
            .map(|v| {
                let mut top: Vec<&tako_control::orchestrator::PromptPiece> =
                    pieces.iter().collect();
                top.sort_by_key(|p| std::cmp::Reverse(p.bytes()));
                let breakdown: Vec<String> = top
                    .iter()
                    .take(5)
                    .map(|p| format!("{}={} B", p.name, p.bytes()))
                    .collect();
                format!(
                    "  {label}: {} が {} で上限 {} を超えている（大きい順: {}）",
                    v.metric.as_str(),
                    v.actual,
                    v.limit,
                    breakdown.join(", ")
                )
            })
            .collect();
        assert!(
            over.is_empty(),
            "既定の system prompt が起動時ロードの予算を超えている\n{}\n\
             直し方: 手順の詳細を crates/tako-control/src/orchestrator/guides/ の topic へ移し、\
             prompt には「いつ引くか」だけを残す（欠落ゼロは tests/prompt_guides.rs が拘束する）",
            over.join("\n")
        );
    }
}

#[test]
fn アーカイブは毎ターン読み込まれる側に置かれていない() {
    // アーカイブを `@import` してしまうと、移送した意味がまるごと消える
    let agents = read("AGENTS.md");
    for line in agents.lines() {
        assert!(
            !(line.contains("@.agent/progress-archive.md")),
            "AGENTS.md がアーカイブを @import している: {line}"
        );
    }
}

/// tako が配る規約文の置き場。**同じ本文が入っていること**をここで固定する
const RULE_FILES: [&str; 3] = [
    "AGENTS.md",
    ".agent/conventions.md",
    "resources/setup/templates/sections/07-context-budget.md",
];

#[test]
fn 規約文は予算表から生成したものと一致する() {
    let want = budget::rule_markdown();
    // 環境変数を立てたときだけ、予算表から生成し直して埋め込む
    if std::env::var("TAKO_UPDATE_CONTEXT_RULE").is_ok() {
        for rel in RULE_FILES {
            let p = repo_root().join(rel);
            let text = std::fs::read_to_string(&p).expect("規約ファイル");
            let (b, e) = (budget::RULE_BEGIN, budget::RULE_END);
            let (Some(start), Some(end)) = (text.find(b), text.find(e)) else {
                panic!("{rel} にマーカーが無い（{b} … {e} を置いてから実行する）");
            };
            let updated = format!(
                "{}{b}\n{want}\n{e}{}",
                &text[..start],
                &text[end + e.len()..]
            );
            std::fs::write(&p, updated).expect("規約の書き戻し");
        }
    }
    for rel in RULE_FILES {
        let text = read(rel);
        let got = budget::extract_rule(&text).unwrap_or_else(|| {
            panic!(
                "{rel} に予算の規約文が無い（{} … {} で囲む）",
                budget::RULE_BEGIN,
                budget::RULE_END
            )
        });
        assert_eq!(
            got,
            want.trim(),
            "{rel} の規約文が予算表とずれている。\
             数値は tako_core::context_budget の定数が正本。\
             更新: TAKO_UPDATE_CONTEXT_RULE=1 cargo test -p tako-control --test context_budget"
        );
    }
}

#[test]
fn 予算表の数値が規約文に出ている() {
    // 生成をやめて手書きへ戻したときに気づけるよう、要の数値だけ直に確かめる
    let rule = budget::rule_markdown();
    for needle in [
        &budget::PROGRESS_MAX_WORK_DAYS.to_string(),
        &budget::PROGRESS_MAX_ENTRIES.to_string(),
        &budget::ARCHIVE_RETAIN_DAYS.to_string(),
        &(budget::AGENTS_GUIDE_MAX_BYTES / 1024).to_string(),
        &(budget::SYSTEM_PROMPT_MAX_BYTES / 1024).to_string(),
    ] {
        assert!(rule.contains(needle.as_str()), "規約文に {needle} が無い");
    }
}

// ─── 検出力（この番犬が本当に落ちることを実測する） ────────────────────

#[test]
fn 超過した作業ログを予算超過として名指しする() {
    // 実物と同じ形（40 行のエントリが 56 作業日ぶん）を合成する
    let mut text = String::from("# Progress Log\n");
    for d in 1..=28 {
        for m in [8, 9] {
            text.push_str(&format!("\n## 2026-{m:02}-{d:02}（合成）\n"));
            for i in 0..40 {
                text.push_str(&format!("- 行 {i}\n"));
            }
        }
    }
    let m = budget::measure(ItemKind::ProgressLog, &text);
    let over: Vec<Metric> = budget::violations(ItemKind::ProgressLog, &m)
        .into_iter()
        .filter(|v| enforced(v.metric))
        .map(|v| v.metric)
        .collect();
    assert!(over.contains(&Metric::Bytes), "バイト超過を名指しする");
    assert!(over.contains(&Metric::Entries), "件数超過を名指しする");
    assert!(over.contains(&Metric::WorkDays), "作業日超過を名指しする");

    // そして fix で予算内へ入ることまで（番犬が示す直し方が本当に効く）
    let plan = budget::plan_prune(&text, "", budget::date_to_days("2026-09-06").unwrap());
    assert!(plan.entries_preserved(), "移送で 1 件も失わない");
    let after = budget::measure(ItemKind::ProgressLog, &plan.progress_text);
    assert!(
        budget::violations(ItemKind::ProgressLog, &after)
            .into_iter()
            .all(|v| !enforced(v.metric)),
        "fix の後は CI が見る軸をすべて満たす"
    );
}

#[test]
fn 規約文が予算表とずれたら落ちる() {
    // 埋め込み側を 1 文字変えたら不一致になること（比較が空振りしていない証拠）
    let want = budget::rule_markdown();
    let tampered = want.replacen("5 作業日", "50 作業日", 1);
    assert_ne!(tampered, want, "置換が効いている");
    let doc = format!(
        "前書き\n{}\n{tampered}\n{}\n後書き",
        budget::RULE_BEGIN,
        budget::RULE_END
    );
    assert_ne!(
        budget::extract_rule(&doc).unwrap(),
        want.trim(),
        "ずれた規約文を一致とみなしてはいけない"
    );
}

/// 番犬: **毎ターン読まれるファイルにマージの残骸を混ぜない**。
///
/// #1241 の merge commit が `.agent/progress.md` へコンフリクトマーカーを 2 箇所
/// 持ち込み、**AI の起動時ロードにそのまま載った**（`@import` されるファイルなので
/// 全ターンに効く）。人が読めば一目だが、マージの解消は機械的に繰り返すので
/// 目視は当てにならない。
///
/// 検査対象は「予算表が `@import` されると宣言しているファイル」に限る
/// （リポジトリ全体を走査すると、コンフリクトの実例を載せた docs に当たる）。
#[test]
fn 毎ターン読まれるファイルにマージの残骸が無い() {
    // 3 文字までにしてこのファイル自身の説明文に当たらないようにする
    let markers = ["<<<<<<<", "=======", ">>>>>>>", "|||||||"];
    let mut bad: Vec<String> = Vec::new();
    for rel in [".agent/progress.md", ".agent/activeContext.md", "AGENTS.md"] {
        let path = repo_root().join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            // **行頭にある**ものだけを見る（本文中の `=======` 区切りや
            // コードブロックの引用に当たらない）。`=======` は Markdown の
            // 見出し下線でもありうるので、他のマーカーと同居する行だけ数える
            if markers[..3].iter().any(|m| line.starts_with(m)) && line.starts_with("<<<<<<<")
                || line.starts_with(">>>>>>>")
                || line.starts_with("|||||||")
            {
                bad.push(format!("  {rel}:{}: {line}", i + 1));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "毎ターン @import されるファイルにマージのコンフリクトマーカーが残っている\
         （{} 行）。AI の起動時ロードにそのまま載るので、解消してからコミットする:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

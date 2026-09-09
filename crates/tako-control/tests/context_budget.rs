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

// ─── コンフリクトマーカーの番犬（Issue #1246） ────────────────────────

/// 走査対象 = **毎ターン読まれる md**（`.agent/` 配下の md 全部 + 規約 2 本）。
///
/// `progress.md` は「全 PR が末尾へ 1〜3 行追記する」規約なので 6〜8 本の worker が
/// 並走すると rebase のたびに衝突し、#1232 / #1241 では**解消し損ねたマーカーが
/// 2 回 commit された**。`AGENTS.md` から `@import` される先なので、残骸は
/// 毎ターンのトークンを浪費し、`context_budget` の件数・バイト判定もズラす。
/// 2 回とも「解消したつもりの取りこぼし」だったので、目視ではなく機械検査で落とす。
///
/// #1241 で入れた狭い版（4 ファイル固定）を**置き換えた**もの。旧版には
/// ①`=======` を一度も拾えない（`&&` が `||` より強いので判定の前半が死んでいた）
/// ②`.agent/plans/` や `conventions.md` を見ない ③8 文字以上の装飾罫線を誤検知する、
/// の 3 点があった。番犬を 2 本並べると片方だけ直って乖離するので 1 本に寄せる。
fn scanned_markdown() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((rel, text));
                }
            }
        }
    }

    let root = repo_root();
    let mut out = Vec::new();
    walk(&root.join(".agent"), &root, &mut out);
    for rel in ["AGENTS.md", "CLAUDE.md"] {
        out.push((rel.to_string(), read(rel)));
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct MarkerHit {
    /// 1 始まりの行番号（エディタ・`file:line` 参照と同じ数え方）
    line: usize,
    marker: &'static str,
}

/// git が書いたコンフリクトマーカーだけを拾う。
///
/// - 記号は **7 文字ちょうど**で、後ろは行末か空白（git が書く `<<<<<<< HEAD` の形）。
///   8 文字以上の飾り罫線（`========`）は対象外
/// - **`=======` は単独では見ない**。Markdown の setext 見出し（見出し文の下の `===`）と
///   同形で、7 文字ちょうどの見出しは正規の Markdown として在りうるため。
///   git は必ず `<<<<<<<` → (`|||||||`) → `=======` → `>>>>>>>` の順に書くので、
///   **`<<<<<<<` が開いた領域の中にあるときだけ**マーカーとみなせば検出力は落ちない
/// - `>>>>>>>` は領域の外でも拾う（`<<<<<<<` 側だけ消した中途半端な解消を落とすため）
fn scan_conflict_markers(text: &str) -> Vec<MarkerHit> {
    let mut hits = Vec::new();
    let mut inside = false;
    for (i, raw) in text.lines().enumerate() {
        let Some(marker) = git_conflict_marker(raw.strip_suffix('\r').unwrap_or(raw)) else {
            continue;
        };
        let hit = MarkerHit {
            line: i + 1,
            marker,
        };
        match marker {
            "<<<<<<<" => {
                inside = true;
                hits.push(hit);
            }
            "|||||||" | "=======" if inside => hits.push(hit),
            ">>>>>>>" => {
                inside = false;
                hits.push(hit);
            }
            _ => {}
        }
    }
    hits
}

/// 行が git のマーカー行そのものなら記号を返す（行頭固定・7 文字ちょうど）
fn git_conflict_marker(line: &str) -> Option<&'static str> {
    ["<<<<<<<", "|||||||", "=======", ">>>>>>>"]
        .into_iter()
        .find(|m| {
            line.strip_prefix(m)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        })
}

#[test]
fn 毎ターン読まれる_md_にコンフリクトマーカーが残っていない() {
    let files = scanned_markdown();
    // 走査が空振りしていない自己検査（置き場を動かしたら緑のまま素通りする事故を防ぐ）
    assert!(
        !files.is_empty(),
        "走査対象が見つからない — {} 配下の md 列挙が壊れている",
        repo_root().join(".agent").display()
    );
    assert!(
        files.iter().any(|(rel, _)| rel == ".agent/progress.md"),
        "この番犬が守るべき .agent/progress.md が走査対象に入っていない（列挙: {:?}）",
        files.iter().map(|(rel, _)| rel).collect::<Vec<_>>()
    );

    let found: Vec<String> = files
        .iter()
        .flat_map(|(rel, text)| {
            scan_conflict_markers(text)
                .into_iter()
                .map(move |h| format!("  {rel}:{} に `{}` が残っている", h.line, h.marker))
        })
        .collect();
    assert!(
        found.is_empty(),
        "コンフリクトマーカーが commit されている（{} 件）\n{}\n\
         直し方: 該当行の前後を読み、両方の版を残す形へ手で直してマーカー行を消す。\n\
         progress.md の衝突は**末尾追記どうし**なので両方残すのが正解\n\
         （.agent/conventions.md「作業ログの衝突は両方残す」節）",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn 残ったコンフリクトマーカーを行番号つきで名指しする() {
    // 実物（.agent/progress.md）の末尾へ、rebase が残すのと同じ 3 行を挿し込む
    let real = read(".agent/progress.md");
    let base_lines = real.lines().count();
    let injected = format!("{real}<<<<<<< HEAD\n- 自版\n=======\n- 他版\n>>>>>>> origin/main\n");

    let hits = scan_conflict_markers(&injected);
    assert_eq!(
        hits,
        vec![
            MarkerHit {
                line: base_lines + 1,
                marker: "<<<<<<<"
            },
            MarkerHit {
                line: base_lines + 3,
                marker: "======="
            },
            MarkerHit {
                line: base_lines + 5,
                marker: ">>>>>>>"
            },
        ],
        "3 行すべてを行番号つきで名指しする"
    );

    // `|||||||`（diff3 スタイル）と、`<<<<<<<` 側だけ消した中途半端な解消も落とす
    assert_eq!(
        scan_conflict_markers("a\n<<<<<<< ours\nb\n||||||| base\nc\n=======\nd\n>>>>>>> theirs\n")
            .iter()
            .map(|h| h.line)
            .collect::<Vec<_>>(),
        vec![2, 4, 6, 8],
        "diff3 スタイルの 4 種すべてを拾う"
    );
    assert_eq!(
        scan_conflict_markers("a\n=======\nb\n>>>>>>> theirs\n")
            .iter()
            .map(|h| h.marker)
            .collect::<Vec<_>>(),
        vec![">>>>>>>"],
        "`<<<<<<<` 側だけ消した中途半端な解消も `>>>>>>>` で落ちる"
    );
}

#[test]
fn setext_見出しをコンフリクトマーカーと誤認しない() {
    // 7 文字ちょうどの setext 見出し（= 誤検知の唯一の危険地帯）と、
    // 罫線・インラインコードでの例示。どれも `<<<<<<<` が開いた領域の外なので拾わない
    let md = "\
Progress
=======

本文。マーカー（`<<<<<<<` / `=======` / `>>>>>>>`）はインラインコードなら行頭に来ない。

小見出し
-------

========
--------
";
    assert_eq!(
        scan_conflict_markers(md),
        vec![],
        "正規の Markdown を 1 行も拾ってはいけない"
    );

    // 検出の根拠（`<<<<<<<` が開いていれば同じ `=======` を拾う）
    assert_eq!(
        scan_conflict_markers("<<<<<<< HEAD\nProgress\n=======\n>>>>>>> x\n").len(),
        3,
        "領域の中の `=======` は拾う（見分けているのであって見逃しているのではない）"
    );

    // 8 文字以上・6 文字以下は git が書く形ではない
    for line in ["========", "======", "<<<<<<<<", "<<<<<<"] {
        assert_eq!(git_conflict_marker(line), None, "{line} はマーカーではない");
    }
}

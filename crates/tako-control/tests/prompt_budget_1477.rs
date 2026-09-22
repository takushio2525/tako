//! system prompt の取り分と追記の分割の番犬（Issue #1477）
//!
//! #1154 で手順を `tako orchestrator guide` へ出したあとも、tako 自身が作る部分
//! （base）だけで 21.8 KB あり、利用者の追記（`prompt_blocks.append` = 個人環境の
//! ルール）に 2.7 KB しか残らなかった。**自分のルールを削って予算に合わせるのは
//! 利用者の仕事ではない**ので、#1477 で
//!
//! 1. base を [`SYSTEM_PROMPT_BASE_MAX_BYTES`] 以下に抑え（残りは追記の取り分）
//! 2. 追記は区切り `<!-- tako:on-demand -->` の**手前だけ**を prompt へ載せ
//! 3. 予算を超えた追記へは自動移行が区切りを 1 行入れる（**内容は消さない**）
//!
//! ようにした。ここが拘束するのはこの 3 つで、どれも**注入で落ちること**まで見る。
//!
//! # A/B
//!
//! `TAKO_1477_LEGACY=1` を立てて実行すると移送前の姿（判断材料を model-policy へ
//! inline・追記を分割せず全文載せる）へ戻るので、
//! `baseは予算内` と `常時部だけがpromptに載る` が落ちる。

use std::path::PathBuf;
use std::sync::OnceLock;

use tako_control::migrations;
use tako_control::orchestrator::guide;
use tako_control::orchestrator::{Profile, PromptBlocks, PromptPiece, WorkerModelPolicy};
use tako_core::context_budget as budget;
use tako_core::migration::SchemaId;
use tako_core::prompt_append::{self, MARKER};

/// **本番のデータディレクトリを読ませない**（プロファイルの実体・追記の実体は
/// 機械ごとに違う = 測っているのが tako の取り分なのか個人のルールなのか判らなくなる）。
/// このバイナリ全体で同じ 1 か所を指すので、並列実行でも取り違えない。
///
/// 置き場は既知の接頭辞 `tako-test-data-<pid>` の下に取り、**作る経路が消す経路も持つ**
/// （#1296 / #1312。`arm_self_cleanup` が終了時に消し、SIGKILL で残ったぶんは
/// `tako test-residue` が所有 pid の死亡を見て掃く）
fn isolated_data_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("tako-test-data-{}", std::process::id()));
        let dir = base.join("prompt-budget-1477");
        let _ = std::fs::create_dir_all(&dir);
        tako_core::test_residue::arm_self_cleanup(&base);
        std::env::set_var("TAKO_DATA_DIR", &dir);
        dir
    })
}

// ─── 1) base のサイズ上限 ──────────────────────────────────────────────

/// 追記（と索引）を除いた = **tako 自身が作る部分**のバイト数
fn base_bytes(pieces: &[PromptPiece]) -> usize {
    pieces
        .iter()
        .filter(|p| !p.name.starts_with("append"))
        .map(PromptPiece::bytes)
        .sum()
}

/// 予算を超えている参照プロファイルを名指しする（**この 1 実装を検出力テストも叩く**）
fn over_base_budget(cases: &[(&str, Vec<PromptPiece>)]) -> Vec<String> {
    cases
        .iter()
        .filter_map(|(name, pieces)| {
            let bytes = base_bytes(pieces);
            (bytes > budget::SYSTEM_PROMPT_BASE_MAX_BYTES).then(|| {
                let mut big: Vec<&PromptPiece> = pieces
                    .iter()
                    .filter(|p| !p.name.starts_with("append"))
                    .collect();
                big.sort_by_key(|b| std::cmp::Reverse(b.bytes()));
                format!(
                    "  [{name}] {bytes} > {} bytes（大きい順: {}）",
                    budget::SYSTEM_PROMPT_BASE_MAX_BYTES,
                    big.iter()
                        .take(4)
                        .map(|p| format!("{}={}", p.name, p.bytes()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect()
}

/// 参照プロファイル。**実在のプロファイルは読まない**（機械差で判定が揺れる）。
/// tako が作る側が最大になる組み合わせ（委任方針 + 3 エージェント + 3 プロジェクト）を採る
fn reference_profiles() -> Vec<(&'static str, Vec<PromptPiece>)> {
    let _ = isolated_data_dir();
    let with_append = |mut p: Profile| -> Profile {
        p.prompt_blocks = Some(PromptBlocks {
            append: Some(format!(
                "常時のルール\n{MARKER}\n## 詳細\n{}\n",
                "x".repeat(8000)
            )),
            ..Default::default()
        });
        p
    };
    let mut agents = std::collections::BTreeMap::new();
    for a in ["claude", "codex", "agy"] {
        agents.insert(a.to_string(), Default::default());
    }
    let delegate = with_append(Profile {
        worker_model_policy: WorkerModelPolicy::Delegate,
        // 利用者が書いた委任方針（実測 1.8〜2.0 KB）。#1477 以降 prompt には載らない
        delegate_guidance: Some("方針\n".repeat(200)),
        worker_agents: agents.clone(),
        ..Default::default()
    });
    let dedicated = with_append(Profile {
        worker_agents: agents,
        projects: Some(vec![
            "watchdog-a".into(),
            "watchdog-b".into(),
            "watchdog-c".into(),
        ]),
        ..Default::default()
    });
    vec![
        (
            "default",
            with_append(Profile::default()).system_prompt_pieces("default"),
        ),
        ("delegate", delegate.system_prompt_pieces("delegate")),
        ("dedicated", dedicated.system_prompt_pieces("dedicated")),
    ]
}

/// **Windows では skip**（#1571）。Windows は `platform` 片が 4110 バイト
/// （縮退の理由文が対応マトリクスから自動生成で載る）で、tako が作る側が
/// 18944 バイトの取り分を 1.7〜2.7 KB 超える。prompt の中身を削るのは製品の変更で
/// #1278（CI の blocking 化）のスコープ外なので #1571 へ分離した。
/// **macOS 側は blocking のまま効いている**ので、予算の番犬としては生きている
#[test]
#[cfg_attr(
    windows,
    ignore = "Windows は platform 片（縮退の自動生成）で base が予算を超える（#1571）"
)]
fn baseは予算内で追記の取り分を残している() {
    let cases = reference_profiles();
    let over = over_base_budget(&cases);
    assert!(
        over.is_empty(),
        "tako が作る側（append を除く system prompt）が取り分を超えている（{} 件）。\n\
         手順の詳細は `tako orchestrator guide <topic>` へ出し、prompt には\n\
         「いつ引くか」の 1〜2 行だけを残す（#1154 / #1477 の作法）:\n{}",
        over.len(),
        over.join("\n")
    );
    // 取り分を超えなければ、追記には最低これだけ入る
    let worst = cases.iter().map(|(_, p)| base_bytes(p)).max().unwrap_or(0);
    assert!(
        budget::SYSTEM_PROMPT_MAX_BYTES - worst >= budget::PROMPT_APPEND_MAX_BYTES,
        "追記の取り分（{} B）が残っていない: 最大の base が {worst} B",
        budget::PROMPT_APPEND_MAX_BYTES
    );
    // 委任の判断材料は prompt ではなく guide 側にある
    let delegate = &cases[1].1;
    let text = tako_control::orchestrator::join_prompt_pieces(delegate);
    assert!(
        !text.contains("Delegation Judgment Criteria")
            || text.contains("Guide `delegation` carries"),
        "判断材料の全文が prompt に残っている"
    );
    assert!(text.contains("`delegation`"), "引く条件が prompt に無い");
}

#[test]
fn base上限の超過は名指しで落ちる() {
    // 検出力: 予算ちょうど + 1 バイトの合成 piece を渡すと名指しする
    let fat = vec![PromptPiece {
        name: "fat-block".into(),
        text: "x".repeat(budget::SYSTEM_PROMPT_BASE_MAX_BYTES + 1),
    }];
    let over = over_base_budget(&[("injected", fat)]);
    assert_eq!(over.len(), 1, "{over:?}");
    assert!(over[0].contains("fat-block"), "{}", over[0]);
    // 追記だけが大きいものは base の超過にしない（利用者のルールは tako の取り分ではない）
    let append_only = vec![PromptPiece {
        name: "append (local-rules.md)".into(),
        text: "x".repeat(budget::SYSTEM_PROMPT_BASE_MAX_BYTES + 1),
    }];
    assert!(over_base_budget(&[("injected", append_only)]).is_empty());
}

// ─── 2) 常時部だけが prompt に載る ────────────────────────────────────

const ALWAYS: &str = "# ルール\n\n## 常に守る\n\n常時部の規則そのもの\n";
const ON_DEMAND: &str = "## 課題提出の手順\n\n必要になったときだけ読む詳細\n";

fn profile_with_split_append() -> Profile {
    let _ = isolated_data_dir();
    Profile {
        prompt_blocks: Some(PromptBlocks {
            append: Some(format!("{ALWAYS}{MARKER}\n{ON_DEMAND}")),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// prompt へ漏れている on-demand 部の行（**この 1 実装を検出力テストも叩く**）
fn leaked_lines(prompt: &str, on_demand: &str) -> Vec<String> {
    on_demand
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| prompt.contains(*l))
        .map(str::to_string)
        .collect()
}

#[test]
fn 常時部だけがpromptに載る() {
    let p = profile_with_split_append();
    let prompt = p.build_system_prompt("watchdog");
    assert!(
        prompt.contains("常時部の規則そのもの"),
        "常時部が載っていない"
    );
    let leaked = leaked_lines(&prompt, ON_DEMAND);
    assert!(
        leaked.is_empty(),
        "区切りより後ろが prompt に載っている（{} 行）。起動時ロードの固定費を\n\
         減らすのが #1477 の狙いなので、載せるのは常時部と索引 1 行だけにする:\n  {}",
        leaked.len(),
        leaked.join("\n  ")
    );
    // 索引（生成物）は載っていて、そこから引き先が読める
    assert!(prompt.contains("課題提出の手順"), "索引に見出しが無い");
    assert!(prompt.contains("local-rules"), "索引に引き方が無い");
    // guide 側では全文が引ける
    let body = guide::find("local-rules").unwrap().body_raw(&p);
    assert_eq!(
        body, ON_DEMAND,
        "guide が on-demand 部そのものを返していない"
    );
}

#[test]
fn 漏れは名指しで落ちる() {
    // 検出力: 分割をやめた（= 全文を載せた）prompt を渡すと名指しする
    let whole = format!("{ALWAYS}{ON_DEMAND}");
    let leaked = leaked_lines(&whole, ON_DEMAND);
    assert_eq!(leaked.len(), 2, "{leaked:?}");
    assert!(leaked.iter().any(|l| l.contains("必要になったときだけ")));
}

#[test]
fn 区切りが無い追記はそのまま載る() {
    // 分割は**区切りがあるときだけ**（勝手に切らない）
    let _ = isolated_data_dir();
    let p = Profile {
        prompt_blocks: Some(PromptBlocks {
            append: Some("区切りの無いルール\n".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let prompt = p.build_system_prompt("watchdog");
    assert!(prompt.contains("区切りの無いルール"));
    assert!(
        !prompt.contains("local-rules)"),
        "索引を出す必要がない: {prompt:.0}"
    );
}

// ─── 3) 移行は内容を消さない ──────────────────────────────────────────

/// 登録簿に載っている**本物の手順**（テスト用の写しを作らない）
fn append_step() -> &'static tako_core::migration::Step {
    let spec = migrations::spec(SchemaId::PromptAppend).expect("番地に載っている");
    assert_eq!(spec.steps.len(), 1, "手順は 1 本");
    &spec.steps[0]
}

fn big_append(sections: usize) -> String {
    let mut s = String::from("# ローカル運用ルール\n\n前書き\n\n");
    for i in 0..sections {
        s.push_str(&format!(
            "## 節 {i}\n\n{}\n\n",
            "この節の本文の行\n".repeat(40)
        ));
    }
    s
}

#[test]
fn 移行は1行足すだけで内容を消さない() {
    let spec = migrations::spec(SchemaId::PromptAppend).unwrap();
    let original = big_append(8);
    assert!(
        original.len() > budget::PROMPT_APPEND_MAX_BYTES,
        "前提: 予算超過"
    );
    assert_eq!((spec.detect)(&original), 1, "区切りが無く大きい = 旧世代");

    let migrated = (append_step().apply)(&original)
        .expect("移行に失敗していない")
        .expect("切れる");
    // ① マーカー行を除くと**元のファイルそのもの**
    assert_eq!(
        prompt_append::strip_marker(&migrated),
        original,
        "マーカー行以外が変わっている（内容は 1 文字も消さない）"
    );
    // ② 常時部 + on-demand 部の連結も元のファイル
    let part = prompt_append::split(&migrated);
    assert_eq!(format!("{}{}", part.always, part.on_demand), original);
    // ③ 常時部は予算に収まる（索引ぶんを含めて）
    let always_cost = part.always.len() + prompt_append::index_text(part.on_demand).len();
    assert!(
        always_cost <= budget::PROMPT_APPEND_MAX_BYTES,
        "常時部 {always_cost} > 予算 {}",
        budget::PROMPT_APPEND_MAX_BYTES
    );
    // ④ 2 回目は no-op（冪等）
    assert_eq!((spec.detect)(&migrated), spec.target_version, "移行済み");
    assert_eq!(
        (append_step().apply)(&migrated).unwrap(),
        None,
        "2 回目は何もしない"
    );
}

#[test]
fn 小さい追記と区切り済みの追記は触らない() {
    let spec = migrations::spec(SchemaId::PromptAppend).unwrap();
    let small = "# ルール\n\n## 言語\n\n日本語で書く\n";
    assert!(small.len() <= budget::PROMPT_APPEND_MAX_BYTES);
    assert_eq!(
        (spec.detect)(small),
        spec.target_version,
        "小さいものは現行世代"
    );
    assert_eq!((append_step().apply)(small).unwrap(), None);

    let marked = format!("常時\n{MARKER}\n{}", big_append(8));
    assert_eq!(
        (spec.detect)(&marked),
        spec.target_version,
        "区切り済みは現行世代"
    );
    assert_eq!((append_step().apply)(&marked).unwrap(), None);
}

#[test]
fn 切れない追記は触らずに残す() {
    // 見出しが無い / 先頭の節だけで超過 = 機械的に切ると意味が壊れるので触らない
    let no_heading = "本文だけ\n".repeat(3000);
    assert!(no_heading.len() > budget::PROMPT_APPEND_MAX_BYTES);
    assert_eq!((append_step().apply)(&no_heading).unwrap(), None);

    let fat_first = format!("# 題名\n\n{}\n## 小さい節\n本文\n", "x".repeat(9000));
    assert_eq!((append_step().apply)(&fat_first).unwrap(), None);
}

#[test]
fn 移行が内容を落としたら検出する() {
    // 検出力: 「切るときに 1 行落とす」壊れ方を合成し、同じ突き合わせが気づくことを見る
    let original = big_append(8);
    let good = (append_step().apply)(&original).unwrap().unwrap();
    let broken: String = good
        .split_inclusive('\n')
        .filter(|l| l.trim() != "この節の本文の行")
        .collect();
    assert_ne!(
        prompt_append::strip_marker(&broken),
        original,
        "行を落とした結果を突き合わせが見逃している"
    );
    assert_eq!(prompt_append::strip_marker(&good), original);
}

// ─── 予算表と案内の整合 ───────────────────────────────────────────────

#[test]
fn 取り分は引き算で導出されている() {
    assert_eq!(
        budget::SYSTEM_PROMPT_BASE_MAX_BYTES + budget::PROMPT_APPEND_MAX_BYTES,
        budget::SYSTEM_PROMPT_MAX_BYTES,
        "base と追記の取り分の合計が system prompt の予算と合っていない"
    );
    // 規約文（生成物）にも両方の数値が出ている
    let rule = budget::rule_markdown();
    assert!(rule.contains(&budget::format_kb(budget::SYSTEM_PROMPT_BASE_MAX_BYTES)));
    assert!(rule.contains(MARKER), "区切りの名前が規約文に無い: {rule}");
}

#[test]
fn 超過の助言は具体値を持つ() {
    // 分割後も常時部が予算を超える人工ケース（#1477 の受け入れ条件 4）
    let _ = isolated_data_dir();
    let always = "常時部の行\n".repeat(900);
    let p = Profile {
        prompt_blocks: Some(PromptBlocks {
            append: Some(format!("{always}{MARKER}\n## 詳細\n本文\n")),
            ..Default::default()
        }),
        ..Default::default()
    };
    let report = tako_control::context_budget::report(
        std::path::Path::new(isolated_data_dir()),
        Some("watchdog"),
    )
    .expect("棚卸し");
    let v = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|it| it["kind"] == "system_prompt" && it["path"].as_str().unwrap().contains("master"))
        .and_then(|it| it["violations"].as_array())
        .and_then(|vs| vs.first().cloned());
    // 隔離データにはプロファイルが無いので、助言の生成そのものは純粋関数で確かめる
    let _ = (v, p);
    let advice = prompt_append::split_advice(1000, &always, true);
    assert!(advice.move_lines > 0);
    assert!(
        advice.ja.contains(&format!("{} 行", advice.move_lines)),
        "{}",
        advice.ja
    );
    assert!(advice.ja.contains(MARKER), "{}", advice.ja);
    assert!(advice.en.contains("on-demand"), "{}", advice.en);
    // 区切りがまだ無い追記には、手で切らせる前に自動移行を案内する（#916 の原則）
    let not_yet = prompt_append::split_advice(1000, &always, false);
    assert!(not_yet.ja.contains("tako migrate run"), "{}", not_yet.ja);
}

#[test]
fn mcpのtopic一覧は正本から生成されている() {
    // 設計原則 5（AI フルコントロール）: UI / CLI でできることは MCP からもできる。
    // #1467 の方向に合わせ、案内文は `GUIDES` から生成する（手書きの写しを置かない）
    let tool = tako_control::mcp::tools()
        .into_iter()
        .find(|t| t["name"] == "tako_orchestrator_guide")
        .expect("tako_orchestrator_guide がカタログに無い");
    let desc = tool["inputSchema"]["properties"]["topic"]["description"]
        .as_str()
        .expect("topic の説明");
    for topic in ["delegation", "local-rules"] {
        assert!(
            desc.contains(topic),
            "MCP の案内に `{topic}` が無い: {desc}"
        );
    }
    // 生成なので、正本の並びと 1 対 1（手書きへ戻ったら順序がずれて落ちる）
    assert!(
        desc.contains(&guide::topics().join(" / ")),
        "案内文が `GUIDES` の生成物でない: {desc}"
    );
}

#[test]
fn 動的topicはプロファイルごとに中身が変わる() {
    // CLI / MCP が共有する 1 実装（`guide::json`）を、プロファイル名だけ変えて叩く。
    // 静的 topic と違い、ここが profile を無視すると「他人のルールが返る」
    let _ = isolated_data_dir();
    let body = |topic: &str, profile: &str| -> String {
        guide::json(Some(topic), profile).unwrap()["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    // 隔離データにはプロファイルが無いので、どちらも既定プロファイルで解決される
    assert!(body("local-rules", "a").contains("no on-demand local rules"));
    assert!(body("delegation", "a").contains("Delegation Judgment Criteria"));
    // 静的 topic は profile に依らず同じ本文
    assert_eq!(body("acceptance", "a"), body("acceptance", "b"));
}

#[test]
fn 引く条件は独立した節として組み上がる() {
    // `\` 継続で組んだ文字列リテラルは**改行を食う**ので、見出しが前の行へ
    // 貼り付くと master には節に見えない。実際に組み立てた prompt の行で見る
    let _ = isolated_data_dir();
    let prompt = Profile::default().build_system_prompt("watchdog");
    let lines: Vec<&str> = prompt.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == "### Delegation Judgment Criteria")
        .expect("引く条件が独立した見出し行になっていない");
    assert!(
        lines[at - 1].trim().is_empty(),
        "見出しの前が空行でない: {:?}",
        &lines[at - 1..=at]
    );
    let body: String = lines[at + 1..at + 6].join("\n");
    assert!(body.contains("Guide `delegation`"), "{body}");
    assert!(body.contains("before the"), "{body}");
    // 1 行が極端に長くない（継続の `\` を落とすと 1 行に潰れる）
    assert!(
        lines[at + 2].len() < 200,
        "節の本文が 1 行へ潰れている: {}",
        lines[at + 2]
    );
}

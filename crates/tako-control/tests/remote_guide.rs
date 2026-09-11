//! リモート / SSH の手順書とそのトリガーの番犬（Issue #1004）
//!
//! #1004 の症状は「master / solo の system prompt に remote / SSH の言及が 0 件で、
//! AI はツール description から自力で辿るしかなく品質がばらつく」だった。直し方は
//! #1154 の形に従う = **手順の全文は topic へ置き、prompt には引く条件だけを足す**。
//!
//! そこで拘束するのは 3 つ。どれが破れても #1004 の症状に戻る:
//!
//! 1. **topic が在る** — `remote` を引けて、Issue の 4 項目
//!    （ホストの正本 / できること・できないこと + 委譲 / 相乗り前提と失敗の分類 /
//!    自動検知のオン・オフ）と #1006 の開き先が本文に在る
//! 2. **トリガーが在る** — master の topic 表と solo の prompt が `remote` を名指しし、
//!    ユーザーの「リモート開きたいから設定して」から topic へ届く
//! 3. **トリガーが手順を抱えていない** — prompt 側の増分は
//!    [`TRIGGER_BUDGET_BYTES`] 以内で、手順の本文は prompt にインラインで載らない。
//!    prompt 全体は起動時ロードの予算（`ItemKind::SystemPrompt`）に収まる
//!
//! 3 を別に縛るのは、「prompt に無いから足す」で本文が prompt 側へ戻る（= #1154 と
//! 逆方向へ育つ）のが一番起きやすい壊れ方だから。本文の欠落・創作は
//! `tests/prompt_guides.rs` が別に見る。

use tako_control::orchestrator::guide;
use tako_control::orchestrator::{
    join_prompt_pieces, Profile, PromptMode, DEFAULT_SYSTEM_PROMPT, SOLO_SYSTEM_PROMPT,
};

/// topic 名の正本（prompt の表・MCP の説明・この番犬が同じ 1 語を指す）
const TOPIC: &str = "remote";

/// prompt へ足してよい増分の上限（#1004 の依頼条件。master + solo の合計）。
/// **手順を prompt 側へ書き戻すと必ずここを割る**
const TRIGGER_BUDGET_BYTES: usize = 500;

/// 素のプロファイル（追記なし = tako が配る既定の姿）で組み立てた prompt
fn built(template: &str, mode: PromptMode) -> String {
    let profile = Profile::default();
    join_prompt_pieces(&profile.build_prompt_pieces(template, "default", mode))
}

/// master の topic 表の行のうち、`remote` を指しているもの
fn master_trigger_lines() -> Vec<String> {
    built(DEFAULT_SYSTEM_PROMPT, PromptMode::Master)
        .lines()
        .filter(|l| l.trim_start().starts_with(&format!("| `{TOPIC}`")))
        .map(str::to_string)
        .collect()
}

/// solo の `### Remote / SSH` 節（見出しから次の見出しまで）
fn solo_trigger_section() -> Vec<String> {
    let prompt = built(SOLO_SYSTEM_PROMPT, PromptMode::Solo);
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for line in prompt.lines() {
        let is_heading = line.trim_start().starts_with('#');
        if inside && is_heading {
            break;
        }
        if is_heading && line.to_ascii_lowercase().contains(TOPIC) {
            inside = true;
        }
        if inside {
            out.push(line.to_string());
        }
    }
    out
}

fn bytes_of(lines: &[String]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

#[test]
fn remoteのtopicが引ける() {
    let g = guide::find(TOPIC).unwrap_or_else(|| {
        panic!(
            "topic `{TOPIC}` が引けない（#1004 の手順書が消えている）。\
             crates/tako-control/src/orchestrator/guide.rs の GUIDES へ \
             guides/remote.md を戻す。引ける topic: {}",
            guide::topics().join(" / ")
        )
    });
    assert!(!g.body.trim().is_empty(), "`{TOPIC}` の本文が空");
    // 表記ゆれ（大文字・アンダースコア）でも同じ topic へ届く
    assert_eq!(guide::find("Remote").map(|g| g.topic), Some(TOPIC));
    // CLI（tako orchestrator guide）と MCP（tako_orchestrator_guide）は
    // dispatch でこの 1 本を共有するので、ここが本文の正
    let out = guide::json(Some(TOPIC), "default").expect("remote の全文");
    assert_eq!(out["topic"], TOPIC);
    assert_eq!(out["text"].as_str().unwrap_or_default(), g.body);
}

#[test]
fn remoteの本文がissue1004の4項目に答えている() {
    let body = guide::find(TOPIC).expect("remote の手順書").body;
    // Issue #1004「明文化する内容」の 4 項目 + #1006 の開き先。
    // 実装が変わって語が変わったら**本文を実装へ合わせる**（ここを緩めない）
    let required: &[(&str, &[&str])] = &[
        ("1. ホストの正本", &["~/.ssh/config", "tako_ssh_hosts"]),
        (
            "2. できること / できないこと + 委譲",
            &["tako_run_interactive", "tako_show_command", "ssh-copy-id"],
        ),
        (
            "3. 相乗り前提と失敗の分類",
            &[
                "ControlMaster",
                "BatchMode=yes",
                "Permission denied (publickey)",
                "known_hosts",
            ],
        ),
        ("4. 自動検知のオン・オフ", &["action: \"auto\"", "skipped"]),
        ("#1006 の開き先", &["`split`", "`tab`", "`pane`"]),
    ];
    let mut missing: Vec<String> = Vec::new();
    for (item, markers) in required {
        for m in *markers {
            if !body.contains(m) {
                missing.push(format!("  [{item}] {m}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "`{TOPIC}` の本文が #1004 の要件を落としている（{} 件）:\n{}\n\
         直し方: crates/tako-control/src/orchestrator/guides/remote.md へ書き、\
         同じ本文を tests/fixtures/guides_added_after_1154.md へ 1 文字も変えずに宣言する",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn masterのtopic表にremoteのトリガーがある() {
    let lines = master_trigger_lines();
    assert_eq!(
        lines.len(),
        1,
        "master の topic 表に `{TOPIC}` の行が {} 本ある（1 本であること）。\
         crates/tako-control/src/orchestrator/default_system_prompt.md の \
         `<!-- block: guides -->` の表を直す",
        lines.len()
    );
    let row = &lines[0];
    // 「いつ引くか」が書かれていること（topic 名だけ並べても引かれない）
    for word in ["SSH", "remote"] {
        assert!(
            row.to_ascii_lowercase()
                .contains(&word.to_ascii_lowercase()),
            "トリガー行に `{word}` が無い（何をきっかけに引くのか読めない）: {row}"
        );
    }
}

#[test]
fn soloのpromptにremoteのトリガーがある() {
    let section = solo_trigger_section();
    assert!(
        !section.is_empty(),
        "solo の prompt に remote / SSH のトリガーが無い（#1004 の症状そのもの）。\
         crates/tako-control/src/orchestrator/solo_system_prompt.md へ \
         `### Remote / SSH` の節を戻す"
    );
    let text = section.join("\n");
    for marker in ["tako_orchestrator_guide", &format!("topic: \"{TOPIC}\"")] {
        assert!(
            text.contains(marker),
            "solo のトリガーに `{marker}` が無い（solo は topic 表を持たないので、\
             ここで引き方まで書いておく必要がある）:\n{text}"
        );
    }
}

#[test]
fn トリガーは手順を抱えず予算に収まっている() {
    use tako_core::context_budget as budget;
    use tako_core::context_budget::{ItemKind, Metric};

    // 1) prompt 側の増分（master の表 1 行 + solo の 1 節）が上限以内
    let master = bytes_of(&master_trigger_lines());
    let solo = bytes_of(&solo_trigger_section());
    assert!(
        master + solo <= TRIGGER_BUDGET_BYTES,
        "prompt 側のトリガーが {} B（master {master} + solo {solo}）で上限 \
         {TRIGGER_BUDGET_BYTES} B を超えている。手順の本文は \
         guides/remote.md 側へ置き、prompt には「いつ引くか」だけを残す",
        master + solo
    );

    // 2) 手順の本文が prompt へインラインで戻っていない
    //    （`restores` が空の topic は tests/prompt_guides.rs の同型検査の対象外なので、
    //     ここで見る。「prompt に無いから足す」で量が戻るのを止める）
    let body = guide::find(TOPIC).expect("remote の手順書").body;
    let prompts = [
        ("master", built(DEFAULT_SYSTEM_PROMPT, PromptMode::Master)),
        ("solo", built(SOLO_SYSTEM_PROMPT, PromptMode::Solo)),
    ];
    let mut inlined: Vec<String> = Vec::new();
    for line in body.lines().map(str::trim).filter(|l| l.len() > 60) {
        for (label, prompt) in &prompts {
            if prompt.contains(line) {
                inlined.push(format!("  [{label}] {}", &line[..line.len().min(80)]));
            }
        }
    }
    assert!(
        inlined.is_empty(),
        "手順の本文が prompt へインラインで載っている（{} 件）。\
         起動時ロードの固定費が増え、同じ手順が 2 か所へ割れる:\n{}",
        inlined.len(),
        inlined.join("\n")
    );

    // 3) prompt 全体が起動時ロードの予算に収まっている（閾値は既存の予算テストと同じ）
    for (label, prompt) in &prompts {
        let m = budget::measure(ItemKind::SystemPrompt, prompt);
        let over: Vec<String> = budget::violations(ItemKind::SystemPrompt, &m)
            .into_iter()
            .filter(|v| v.metric == Metric::Bytes)
            .map(|v| format!("  {label}: {} > {}", v.actual, v.limit))
            .collect();
        assert!(
            over.is_empty(),
            "system prompt が起動時ロードの予算を超えた:\n{}",
            over.join("\n")
        );
    }
}

// ─── 検出力（この番犬が本当に落ちることを実測する） ────────────────────

#[test]
fn 本文から要件が落ちたら名指しする() {
    // 上の `remoteの本文が…` と同じ判定を、要件を欠いた本文へ当てる
    let body = "## Remote\n~/.ssh/config のことだけ書いた本文\n";
    let missing: Vec<&str> = ["tako_run_interactive", "ControlMaster", "action: \"auto\""]
        .into_iter()
        .filter(|m| !body.contains(m))
        .collect();
    assert_eq!(
        missing.len(),
        3,
        "要件の欠落を検出できていない: {missing:?}"
    );
    // 実物は 1 件も落ちない（= 常に落ちる番犬ではない）
    let real = guide::find(TOPIC).expect("remote の手順書").body;
    assert!(
        ["tako_run_interactive", "ControlMaster", "action: \"auto\""]
            .into_iter()
            .all(|m| real.contains(m))
    );
}

#[test]
fn 節の切り出しが見出しで止まる() {
    // solo のトリガー節を数えている抽出が、次の見出し以降を巻き込んでいない
    let section = solo_trigger_section();
    assert!(
        !section.is_empty(),
        "節が 1 行も取れていない（トリガーが消えているなら \
         `soloのpromptにremoteのトリガーがある` が正の名指し）"
    );
    assert!(
        section.len() <= 8,
        "solo のトリガー節が {} 行（次の節まで巻き込んでいないか）:\n{}",
        section.len(),
        section.join("\n")
    );
    assert!(
        section[0].trim_start().starts_with('#'),
        "節の先頭が見出しでない: {}",
        section[0]
    );
    assert!(
        section[1..]
            .iter()
            .all(|l| !l.trim_start().starts_with('#')),
        "節の途中に見出しが混ざっている:\n{}",
        section.join("\n")
    );
}

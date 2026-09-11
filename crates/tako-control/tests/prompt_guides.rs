//! system prompt から手順書へ移した本文の番犬（Issue #1154）
//!
//! #1154 は master / solo の system prompt（起動時ロード最大の固定費 = takodev の生成物
//! 59,501 B ≒ 4 万トークン）から**手順の詳細だけ**を外へ出した。危ないのは
//! 「削ったつもりが本当に消えていた」ことなので、**移送前の全文を fixture に固定**し、
//! `(新しいテンプレート ∪ 全 topic の本文)` と多重集合で突き合わせて欠落ゼロを機械で示す。
//!
//! 拘束しているのは 4 つ:
//!
//! 1. **欠落ゼロ** — 移送前の 1 行 1 行が、テンプレートか手順書のどちらかに残っている
//! 2. **手順書は原文以外を含まない** — 要約・言い換え・創作が混ざっていない
//!    （テンプレート側の案内文は新しく書いてよい。そちらは 3 で量を縛る）。
//!    **新機能の手順を手順書へ足す道**は fixture `guides_added_after_1154.md` への
//!    宣言（Issue 番号 + 本文そのまま）で開いている —— テンプレート側へ書くと
//!    #1154 の方向と逆（起動時ロードの固定費が増え、同じ表が 2 か所へ割れる）ため
//! 3. **行き先が申告どおり** — ブロック B の本文は「新テンプレートの B」か
//!    「`restores` に B を持つ手順書」にしか行っていない（monitoring の本文が
//!    acceptance の手順書へ紛れ込む、が起きない）
//! 4. **案内が生きている** — テンプレートの topic 表と `GUIDES` が一致し、
//!    本文を失ったブロックは自分の topic を名指ししている
//!
//! **意図して本文を消すとき**（古くなった手順の削除など）は fixture も同じ PR で
//! 更新する。そうしないとここが落ちる = 「気づかないうちに消えた」を作らない仕掛け。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tako_control::orchestrator::guide::{self, GUIDES};
use tako_control::orchestrator::DEFAULT_SYSTEM_PROMPT;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 移送前の system prompt テンプレート全文（2026-09-07 / #1154 の直前）
fn before() -> String {
    let p = repo_root().join("crates/tako-control/tests/fixtures/system_prompt_before_1154.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// `<!-- block: name -->` で割る（製品側 `parse_prompt_blocks` と同じ規則）
fn blocks(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(name) = line
            .trim()
            .strip_prefix("<!-- block: ")
            .and_then(|s| s.strip_suffix(" -->"))
        {
            if let Some(b) = cur.take() {
                out.push(b);
            }
            cur = Some((name.to_string(), String::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(b) = cur.take() {
        out.push(b);
    }
    out
}

/// 比較用の行（**空行と前後の空白だけを落とす**。本文は 1 文字も変えない）
fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// 行の多重集合（同じ行が 2 度出てくるものを 1 度に潰さない）
fn bag(text: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for l in lines_of(text) {
        *m.entry(l).or_insert(0) += 1;
    }
    m
}

fn merge(into: &mut BTreeMap<String, usize>, from: &BTreeMap<String, usize>) {
    for (k, v) in from {
        *into.entry(k.clone()).or_insert(0) += v;
    }
}

/// テンプレート + 全 topic の本文（master が到達できる本文の全体）
fn corpus_bag() -> BTreeMap<String, usize> {
    let mut all = bag(DEFAULT_SYSTEM_PROMPT);
    for g in GUIDES {
        merge(&mut all, &bag(g.body));
    }
    all
}

fn show(line: &str) -> String {
    let max = 90;
    if line.chars().count() <= max {
        return line.to_string();
    }
    line.chars().take(max).collect::<String>() + "…"
}

/// 移送前の本文のうち、行き先（`have`）で足りていない行（**この 1 実装を検出力テストも叩く**）
fn missing_lines(before_text: &str, have: &BTreeMap<String, usize>) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for (name, body) in blocks(before_text) {
        for (line, want) in bag(&body) {
            let got = have.get(&line).copied().unwrap_or(0);
            if got < want {
                missing.push(format!("  [{name}] {} ({got}/{want})", show(&line)));
            }
        }
    }
    missing
}

/// 移送後に**新しく足した**手順書の本文（Issue 番号つきで宣言してあるもの）。
///
/// 移送の番犬が止めたいのは「要約・言い換えで意味が変わる」ことなので、
/// **新機能の手順を足す道**は別に開けてある。fixture へ 1 文字も変えずに貼れば通り、
/// 宣言していない行は今までどおり創作として落ちる。
/// 注記（`<!-- … -->` で始まる行）は宣言に数えない
fn added_after() -> String {
    let p = repo_root().join("crates/tako-control/tests/fixtures/guides_added_after_1154.md");
    let text =
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()));
    text.lines()
        .filter(|l| !l.trim_start().starts_with("<!--"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 移送前の本文にも「後から足した」宣言にも無い行（= 手順書側の要約・言い換え・創作）
fn invented_lines(bodies: &[(&str, &str)], before_bag: &BTreeMap<String, usize>) -> Vec<String> {
    let allowed_added = bag(&added_after());
    let mut out: Vec<String> = Vec::new();
    for (topic, body) in bodies {
        for line in lines_of(body) {
            if !before_bag.contains_key(&line) && !allowed_added.contains_key(&line) {
                out.push(format!("  [{topic}] {}", show(&line)));
            }
        }
    }
    out
}

#[test]
fn 移した本文は1文字も失われていない() {
    let before = before();
    let missing = missing_lines(&before, &corpus_bag());
    assert!(
        missing.is_empty(),
        "#1154 で移した本文が {} 行ぶん失われている（テンプレートにも手順書にも無い）:\n{}\n\
         直し方: 消えた行を該当 topic の本文（crates/tako-control/src/orchestrator/guides/）\
         へ戻す。**意図して消したなら** fixture \
         crates/tako-control/tests/fixtures/system_prompt_before_1154.md も同じ PR で更新する",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn 手順書は原文以外の行を含まない() {
    // 手順書は「移した原文そのまま」でなければならない（要約すると意味が変わる）。
    // 新しく書いてよいのはテンプレート側の案内文だけ
    let bodies: Vec<(&str, &str)> = GUIDES.iter().map(|g| (g.topic, g.body)).collect();
    let invented = invented_lines(&bodies, &bag(&before()));
    assert!(
        invented.is_empty(),
        "手順書に移送前の本文に無い行がある（{} 行）。要約・言い換えは手順書に置かない\
         （置くならテンプレート側の案内文にする）。**新機能の手順を足したのなら** \
         その本文を crates/tako-control/tests/fixtures/guides_added_after_1154.md へ \
         Issue 番号つきで宣言する（1 文字も変えずに貼る）:\n{}",
        invented.len(),
        invented.join("\n")
    );
}

#[test]
fn ブロックの本文は申告した行き先にしか無い() {
    let before = before();
    let now: BTreeMap<String, String> = blocks(DEFAULT_SYSTEM_PROMPT).into_iter().collect();
    let mut stray: Vec<String> = Vec::new();
    for (name, body) in blocks(&before) {
        // 行き先 = 同名ブロック（残したぶん）+ `restores` に自分を挙げた手順書
        let mut dest = now.get(&name).map(|b| bag(b)).unwrap_or_default();
        for g in GUIDES
            .iter()
            .filter(|g| g.restores.contains(&name.as_str()))
        {
            merge(&mut dest, &bag(g.body));
        }
        for (line, want) in bag(&body) {
            if dest.get(&line).copied().unwrap_or(0) < want {
                stray.push(format!("  [{name}] {}", show(&line)));
            }
        }
    }
    assert!(
        stray.is_empty(),
        "ブロックの本文が申告した行き先の外にある（{} 行）。\
         guide の `restores` に由来ブロックを挙げるか、本文を正しい topic へ移す:\n{}",
        stray.len(),
        stray.join("\n")
    );
}

#[test]
fn テンプレートのtopic表と手順書の一覧が一致する() {
    let template: BTreeMap<String, String> = blocks(DEFAULT_SYSTEM_PROMPT).into_iter().collect();
    let index = template
        .get("guides")
        .expect("`guides` ブロック（topic 表）がテンプレートに無い");
    // 表に無い topic は誰も引けない
    for g in GUIDES {
        assert!(
            index.contains(&format!("`{}`", g.topic)),
            "topic `{}` がテンプレートの topic 表に無い（案内されていない topic は引かれない）",
            g.topic
        );
    }
    // 表にあるのに実在しない topic は「引いたら怒られる」案内になる
    for line in index.lines().filter(|l| l.trim_start().starts_with("| `")) {
        let name = line
            .trim_start()
            .trim_start_matches("| `")
            .split('`')
            .next()
            .unwrap_or("");
        assert!(
            guide::find(name).is_some(),
            "topic 表の `{name}` に対応する手順書が無い"
        );
    }
}

#[test]
fn mcpツールの説明にも全topicが並んでいる() {
    // topic 名の置き場が 3 つ（`GUIDES` / テンプレートの表 / MCP の inputSchema）
    // あるので、増やしたときに取り残されないよう縛る
    let tool = tako_control::mcp::tools()
        .into_iter()
        .find(|t| t["name"] == "tako_orchestrator_guide")
        .expect("tako_orchestrator_guide がカタログに無い");
    let desc = tool["inputSchema"]["properties"]["topic"]["description"]
        .as_str()
        .expect("topic の説明");
    for g in GUIDES {
        assert!(
            desc.contains(g.topic),
            "MCP ツールの topic 説明に `{}` が無い: {desc}",
            g.topic
        );
    }
}

#[test]
fn 本文を移したブロックは自分のtopicを名指ししている() {
    let template: BTreeMap<String, String> = blocks(DEFAULT_SYSTEM_PROMPT).into_iter().collect();
    for g in GUIDES {
        for b in g.restores {
            let body = template
                .get(*b)
                .unwrap_or_else(|| panic!("ブロック `{b}` がテンプレートから消えている"));
            assert!(
                body.contains(&format!("`{}`", g.topic)),
                "ブロック `{b}` は本文を topic `{}` へ移したのに、その topic を名指ししていない\
                 （master は手順の在り処を知れない）",
                g.topic
            );
        }
    }
}

#[test]
fn 差し戻しは全topicを1度ずつ拾う() {
    // `TAKO_1154_LEGACY=1` の A/B が本文を取りこぼさない（= 旧挙動を本当に再現する）
    let mut restored: Vec<&str> = Vec::new();
    for (name, _) in blocks(DEFAULT_SYSTEM_PROMPT) {
        for g in guide::restored_at_block(&name) {
            restored.push(g.topic);
        }
    }
    let mut sorted = restored.clone();
    sorted.sort_unstable();
    let n = sorted.len();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        n,
        "同じ topic を 2 度差し戻している: {restored:?}"
    );
    for g in GUIDES.iter().filter(|g| !g.restores.is_empty()) {
        assert!(
            restored.contains(&g.topic),
            "topic `{}` が差し戻されない（`restores` の先頭 `{:?}` がテンプレートに無い）",
            g.topic,
            g.restores.first()
        );
    }
}

// ─── 検出力（この番犬が本当に落ちることを実測する） ────────────────────

#[test]
fn 行を1本落としたら欠落として名指しする() {
    // 比較が空振りしていないことの証拠。実物の monitoring から 1 行抜いて、
    // 同じ 1 実装（`missing_lines`）がその行を名指しすることまで見る
    let before = before();
    let victim = guide::find("monitoring")
        .expect("monitoring の手順書")
        .body
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("- `api_error`"))
        .expect("対処表の 1 行")
        .to_string();

    let mut tampered = corpus_bag();
    // その行だけを行き先から取り上げる（= 移送で落とした状態を合成する）
    tampered.remove(&victim);
    let missing = missing_lines(&before, &tampered);
    assert!(
        missing.iter().any(|m| m.contains(&victim)),
        "落とした行を名指しできていない: {missing:?}"
    );
    // 取り上げる前は 1 件も出ない（= 常に落ちる番犬ではない）
    assert!(missing_lines(&before, &corpus_bag()).is_empty());
}

#[test]
fn 手順書に要約を混ぜたら創作として名指しする() {
    let before_bag = bag(&before());
    let invented = invented_lines(
        &[(
            "monitoring",
            "要するに worker が止まったら様子を見る（要約）",
        )],
        &before_bag,
    );
    assert_eq!(invented.len(), 1, "創作した行を名指しする: {invented:?}");
    // 原文そのままの行は創作扱いしない
    let real = guide::find("acceptance").unwrap().body;
    assert!(invented_lines(&[("acceptance", real)], &before_bag).is_empty());
}

#[test]
fn 後から足した本文は宣言したものだけ通る() {
    // 新機能の手順を足す道（`added_after`）が**素通しになっていない**ことの証拠。
    // 宣言と 1 文字でも違えば創作として落ちる
    let before_bag = bag(&before());
    let declared = lines_of(&added_after());
    assert!(
        !declared.is_empty(),
        "宣言 fixture の読み出しが壊れている（注記だけになっている）"
    );
    let first = declared[0].clone();
    assert!(
        invented_lines(&[("monitoring", &first)], &before_bag).is_empty(),
        "宣言した行が創作扱いされている: {first}"
    );
    let tampered = format!("{first}（言い換え）");
    assert_eq!(
        invented_lines(&[("monitoring", &tampered)], &before_bag).len(),
        1,
        "宣言と違う行が素通りしている: {tampered}"
    );
}

#[test]
fn 既定のpromptに手順の全文が載っていない() {
    // #1154 の本体。**移送が本当に効いている**ことを、各 topic の特徴的な 1 行が
    // prompt に載っていないことで確かめる。組み立て済みの prompt を見るので
    // `TAKO_1154_LEGACY=1`（本文を差し戻す A/B）ではここが落ちる
    let profile = tako_control::orchestrator::Profile::default();
    let built = profile.build_from_template(DEFAULT_SYSTEM_PROMPT, "default");
    let prompt = built.as_str();
    let mut inlined: Vec<String> = Vec::new();
    for g in GUIDES.iter().filter(|g| !g.restores.is_empty()) {
        // その topic の本文のうち、案内文に出てこない実体行を代表として選ぶ
        let sample = lines_of(g.body)
            .into_iter()
            .filter(|l| l.len() > 60 && !l.starts_with('#') && !l.starts_with('|'))
            .max_by_key(|l| l.len())
            .unwrap_or_default();
        if !sample.is_empty() && prompt.contains(&sample) {
            inlined.push(format!("  [{}] {}", g.topic, show(&sample)));
        }
    }
    assert!(
        inlined.is_empty(),
        "手順の全文が prompt にインラインで残っている（{} 件）。\
         起動時ロードの固定費を減らすのが #1154 の狙いなので、本文は topic 側に置き、\
         prompt には引く条件だけを残す:\n{}",
        inlined.len(),
        inlined.join("\n")
    );
}

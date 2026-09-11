//! 番犬: 実行拒否の分類を「送達後・作業ゼロ」のゲートから外さない（#1034）
//!
//! ## なぜ止めるのか
//!
//! #983 の `detect_launch_failure` は「まだ送達の証拠が無い worker」に限るゲートを持つ。
//! これは**正しい設計**で、仕事を始めた worker の scrollback に流れる
//! `command not found`（agent 自身が実行したコマンド）を起動失敗と誤検知しないためにある。
//!
//! #1034 は「送達が成立したあとに実行を断られる」事象なのでそのゲートの外に落ちていた。
//! ここで**既存のゲートを緩める**と #983 が避けた誤検知が丸ごと戻るので、
//! #1034 は**別のゲート**（一次シグナルで作業ステップを 1 件も観測していない）で分類する。
//!
//! 単体テストは「いまの判定結果」を固定するが、**ゲートの緩み**は
//! ソースの形でしか止められない。
//!
//! ## `None`（観測ゼロ）の扱い（#1295）
//!
//! 旧版のこの doc は「`Some(true)` / `None` では分類しない」と書いていたのに、
//! assert は `agent_work_started != Some(true)`（= `None` を**通す**形）を固定していた。
//! **宣言と検査が逆を向いていた**ので、番犬は穴を止められなかった。
//!
//! 正しい規則は 3 値で割れる:
//!
//! | `agent_work_started` | 分類してよいか |
//! |---|---|
//! | `Some(true)`（作業を観測した） | しない |
//! | `Some(false)`（作業ゼロを観測した） | する |
//! | `None`（観測ゼロ = 何も言えない） | **能力マトリクスが宣言した系統だけ**（agy） |
//!
//! `None` を作業ゼロと読めるのは「拒否が会話の成立前に起こるので直接の証拠を採る
//! 対象がそもそも無い」系統だけで、宣言は `WORKER_REFUSAL_WORK_PROOF` の 1 マスが持つ
//! （#982 の規約）。**claude の腕は `agent_work_started` を一度も代入しない**ので、
//! `None` を無条件に通すとゲートが claude で常に開く（= #1295 のバグ）。
//!
//! ## doc と assert を機械で結ぶ
//!
//! 上の表を書き換えても assert が追従しなければ同じ事故が起きる。そこで
//! **この doc の 1 行を assert の入力にする**（[`番犬のdocとassertが同じ規則を指す`]）。
//! 空白の違いは無視して突き合わせるので、rustfmt の折り返しには追従する。
//!
//! ゲートの形: `None => matches!( agent_support::refusal_work_proof(agent), RefusalWorkProof::NoConversation ),`
//!
//! ## 7 本立て
//!
//! 1. [`実行拒否は一次シグナルの作業ゼロで門番している`] — 分類が
//!    「一次シグナルで作業を 1 歩も観測していない」で門番されていること
//! 2. [`既存の送達ゲートを緩めていない`] — #983 の `never_delivered` ゲートが残っていること
//! 3. [`画面推定のbusyをゲートの根拠にしていない`] — ゲートの材料が実況ログ由来であること
//! 4. [`未調査の系統に推測の文言を置いていない`] — 実採取していない系統の宣言が空であること
//! 5. [`ゲートがnoneを無条件に通す形へ戻っていない`] — `!= Some(true)` が A/B の
//!    旧アーム以外に現れないこと（#1295）
//! 6. [`noneを通す判定はマトリクスから引く`] — 系統を直書きせずマトリクスを引くこと（#982）
//! 7. [`番犬のdocとassertが同じ規則を指す`] — この doc の「ゲートの形」が実物と一致すること

use std::path::{Path, PathBuf};

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文に書いた実採取の引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!") || t.starts_with("///"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の本文（シグネチャから次の行頭 `}` まで）を切り出す
fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn production(rel: &str) -> String {
    let src = std::fs::read_to_string(crate_file(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    without_comments(production_source(&src))
}

#[test]
fn 実行拒否は一次シグナルの作業ゼロで門番している() {
    let src = production("src/dispatch.rs");
    let call = src
        .find("detect_execution_refused")
        .expect("実行拒否の分類が dispatch から外れている（idle + delivered へ戻る）");
    // 呼び出しの手前 1200 文字にゲートがあること
    let before = &src[call.saturating_sub(1200)..call];
    assert!(
        before.contains("work_unobserved"),
        "実行拒否の分類が「一次シグナルで作業を 1 歩も観測していない」で門番されていない。\
         ここを外すと、仕事を始めた worker の画面に同じ文字列が流れただけで完了を \
         error に落とす（#983 が command not found で避けた形）"
    );
    let gate = production_gate(&src);
    assert!(
        gate.contains("Some(started) => !started"),
        "作業を観測した worker（Some(true)）を除外していない / \
         作業ゼロを観測した worker（Some(false)）を分類していない"
    );
    assert!(
        gate.contains("WORKER_STATUS_STRUCTURED"),
        "一次シグナルを持つ系統に限っていない。持たない系統では「会話が無い」と \
         「見えないだけ」が区別できないので、作業中の worker を拒否と誤読しうる"
    );
    assert!(
        before.contains("\"idle\"") && before.contains("\"unknown\""),
        "停止している worker に限る条件が無い（busy 中に分類してはいけない）"
    );
}

#[test]
fn 既存の送達ゲートを緩めていない() {
    let src = production("src/dispatch.rs");
    // #983 のゲートは**そのまま**残っていること（緩めて 1 本にまとめない）
    assert!(
        src.contains("let never_delivered = !registry_session_detected"),
        "#983 の「まだ送達の証拠が無い worker に限る」ゲートが消えている。\
         #1034 はこれを緩めるのではなく別ゲートで分類する設計"
    );
    let launch = src
        .find("detect_launch_failure")
        .expect("#983 の起動失敗の分類が消えている");
    let before = &src[launch.saturating_sub(700)..launch];
    assert!(
        before.contains("never_delivered"),
        "#983 の分類が never_delivered ゲートの中から出ている"
    );
}

#[test]
fn 画面推定のbusyをゲートの根拠にしていない() {
    let src = production("src/dispatch.rs");
    // ゲートの材料は実況ログ由来（agy = transcript の MODEL ステップ /
    // codex = rollout の task_started）でなければならない。
    // 画面推定の busy は TUI の起動描画を拾うので根拠にならない（#1034 の実測）
    assert!(
        src.contains("agent_work_started = Some(st.agent_work_started())"),
        "agy の作業開始の判定が実況 JSONL 由来でない"
    );
    assert!(
        !src.contains("agent_work_started = Some(screen_looks_busy"),
        "画面推定の busy をゲートの根拠にしている（起動描画だけで busy に見える）"
    );
    let agy = production("src/agy_session.rs");
    let body = fn_body(&agy, "    pub fn agent_work_started(&self) -> bool {");
    assert!(
        body.contains("model_step_seen"),
        "作業開始の証拠が MODEL ステップの観測になっていない"
    );
}

#[test]
fn 未調査の系統に推測の文言を置いていない() {
    let src = production("src/orchestrator/agent_cli.rs");
    let body = fn_body(
        &src,
        "fn execution_refused_patterns(agent: Agent) -> &'static [&'static str] {",
    );
    assert!(
        body.contains("Agent::Claude | Agent::Codex => &[]"),
        "実採取していない系統に文言が入っている（#982 の規約: 推測を書かない）"
    );
    assert!(
        body.contains("\"account eligibility\""),
        "agy の実採取の軸（両版に共通して残る語）が消えている"
    );
    // 一時的な API エラーの常套句を単体で採らない（`api_error` を食う）
    for generic in ["\"please try again later\"", "\"please try again shortly\""] {
        assert!(
            !body.contains(generic),
            "{generic} を単体のパターンにしている。API エラーの常套句と衝突して \
             続行指示で直る停止を実行拒否と誤分類する"
        );
    }
}

/// 実物のゲート本体（#1295 で `dispatch.rs` の 1 関数へ閉じた）
fn production_gate(src: &str) -> String {
    fn_body(src, "fn refusal_gate_open_in(")
}

/// 生のソースで `needle` を含む**コード行**の `file:line`（説明文は数えない）。
/// **落ちたときに直す場所をそのまま出す**ためにある（#1274 / #1300 と同じ作法）
fn code_lines_with(rel: &str, needle: &str) -> Vec<String> {
    let src = std::fs::read_to_string(crate_file(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let name = rel.rsplit('/').next().unwrap_or(rel);
    src.lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!") || t.starts_with("///"))
                && l.contains(needle)
        })
        .map(|(i, l)| format!("{name}:{}: {}", i + 1, l.trim()))
        .collect()
}

/// 空白の違いを無視して突き合わせる（rustfmt の折り返しに追従するため）
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// #1295: `None`（観測ゼロ）を無条件に作業ゼロと数える形へ戻っていないこと。
///
/// claude の腕は `agent_work_started` を一度も代入しないので、この形へ戻すと
/// ゲートが **claude では常に開く**（正常に働いた worker を error へ落とす）
#[test]
fn ゲートがnoneを無条件に通す形へ戻っていない() {
    let src = production("src/dispatch.rs");
    let hits = code_lines_with("src/dispatch.rs", "agent_work_started != Some(true)");
    assert_eq!(
        hits.len(),
        1,
        "`agent_work_started != Some(true)`（= None を無条件に通す形）が \
         A/B の旧アーム以外に現れている。claude の腕はこの値を一度も代入しないので、\
         この形はゲートを claude で常に開く（#1295）:\n  {}",
        hits.join("\n  ")
    );
    let legacy = fn_body(&src, "fn legacy_work_unobserved(");
    assert!(
        legacy.contains("agent_work_started != Some(true)"),
        "唯一の出現が A/B の旧アーム（legacy_work_unobserved）ではない"
    );
    let gate = production_gate(&src);
    assert!(
        gate.contains("legacy_work_unobserved") && gate.contains("if legacy"),
        "旧アームが env（TAKO_1295_LEGACY）の下以外から呼ばれている"
    );
}

/// #1295 / #982: `None` を通す系統の判断を**マトリクスから引く**こと。
///
/// 系統を直書きすると宣言が 2 箇所になり、「どちらが正か」が分からなくなる
#[test]
fn noneを通す判定はマトリクスから引く() {
    let src = production("src/dispatch.rs");
    let gate = production_gate(&src);
    assert!(
        gate.contains("refusal_work_proof"),
        "None を通す判断がマトリクス（WORKER_REFUSAL_WORK_PROOF）から引かれていない"
    );
    for hardcoded in ["Agent::Agy", "Agent::Claude", "\"agy\"", "\"claude\""] {
        if !gate.contains(hardcoded) {
            continue;
        }
        let at = code_lines_with("src/dispatch.rs", hardcoded);
        panic!(
            "ゲートが系統（{hardcoded}）を直書きしている。系統ごとの差は \
             agent_support::MATRIX の 1 マスへ宣言すること（#982）:\n  {}",
            at.join("\n  ")
        );
    }
}

/// #1295: このファイルの doc が言う「ゲートの形」が実物と一致すること。
///
/// 旧版は doc（`Some(false)` でだけ分類）と assert（`!= Some(true)`）が逆を向いていた。
/// **doc の 1 行を assert の入力にする**ことで、片方だけ直して食い違うことができなくなる
#[test]
fn 番犬のdocとassertが同じ規則を指す() {
    let own = include_str!("agy_execution_refused_watchdog.rs");
    let line = own
        .lines()
        .find(|l| l.starts_with("//! ゲートの形: "))
        .expect("doc の「ゲートの形」行が消えている（消すなら実物と結ぶ別の手を用意すること）");
    let form = line
        .trim_start_matches("//! ゲートの形: ")
        .trim()
        .trim_matches('`');
    let src = production("src/dispatch.rs");
    let gate = squeeze(&production_gate(&src));
    assert!(
        gate.contains(&squeeze(form)),
        "doc が言うゲートの形が実物に無い。\n  doc: {form}\n  実物: {gate}"
    );
}

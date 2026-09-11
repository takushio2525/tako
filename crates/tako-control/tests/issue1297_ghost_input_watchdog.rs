//! 番犬: 入力欄の AI ゴースト提案を「人の下書き」と読み違えない（#1297）
//!
//! ## なぜ止めるのか
//!
//! #1273 は「背景作業が残る入力待ち」を idle と読めるようにしたが、その最後の関門
//! （入力欄が空か）を**文字列だけ**で見ていた。claude は空欄へ AI のゴースト提案を
//! 薄字で描き、文面は `CI が緑になったら merge して…` のような任意の自然文なので
//! `INPUT_PLACEHOLDERS` には原理的に載らない。結果、提案が出ているあいだ
//! #1273 の腕がまるごと死んでいた（本番 pane 1636 / 1761 / 1775 / 1784。
//! master は watch のタイムアウトでしか気づけなかった）。
//!
//! 単体テストは「いまの判定結果」を固定するが、**同じ形の再発**
//! （属性を捨てて文字列へ戻す・ghost を下書きに数える・user / mixed を空に数える・
//! そもそも属性を配線しない）はソースの形でしか止まらない。
//!
//! ## 4 本立て
//!
//! 1. [`判定は文字列だけに戻せない`] — `input_waiting_with_background_work_in` が
//!    属性（`InputStyle`）を受け取り、分類器を通していること
//! 2. [`ghostを下書き扱いにしない`] — `Ghost` / `None` は「下書きなし」。
//!    ここが崩れると #1297 がそのまま再発する
//! 3. [`userとmixedを空扱いにしない`] — 逆向きの崩れ（安全側の放棄）。
//!    人が打ちかけている入力欄を踏み潰して worker を完了扱いにする
//! 4. [`属性はdispatchから配線されている`] — 判定を直しても、`worker_status` が
//!    属性を採って渡していなければ本番では 1 ビットも変わらない

use std::path::{Path, PathBuf};

use tako_control::orchestrator::wait::{
    self, classify_input_draft_in, BackgroundIdle, InputDraft, INPUT_DRAFT_UNREADABLE,
};
use tako_core::agent_support::Agent;
use tako_core::InputStyle;

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

/// コメント行を落とす（説明文に書いた引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
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
    without_comments(&rest[..end])
}

/// マッチした行を「ファイル:行番号」つきで名指しする
fn locate(path: &str, src: &str, needle: &str) -> String {
    src.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| format!("{path}:{} {}", i + 1, l.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

const WAIT_PATH: &str = "crates/tako-control/src/orchestrator/wait.rs";
const DISPATCH_PATH: &str = "crates/tako-control/src/dispatch.rs";

fn wait_src() -> String {
    std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs を読める")
}

/// #1297 の本番 pane 1636 と同じ形（#927 に沿ってラベルはプレースホルダ）。
/// ターンは終わり、背景シェルが 1 本、入力欄には AI のゴースト提案
const GHOST_SUGGESTION: &str = "\
⏺ PR を出しました。CI の結果を待ちます。

✻ Worked for 2m 04s · done 12:43 PM · 1 shell still running

─────────────────────────────────────────────
❯ merge the PR once CI is green
─────────────────────────────────────────────
  [model placeholder]  worker: placeholder task
  ⏵⏵ auto mode on · 1 shell · ← for agents";

fn verdict(style: Option<InputStyle>) -> BackgroundIdle {
    wait::input_waiting_with_background_work_in(
        GHOST_SUGGESTION,
        false,
        Some(Agent::Claude),
        style,
        false,
        false,
    )
}

#[test]
fn 判定は文字列だけに戻せない() {
    let src = wait_src();
    let prod = production_source(&src);
    let sig = "pub fn input_waiting_with_background_work_in(";
    let head = &prod[prod.find(sig).expect("判定関数が見つからない")..];
    let head = &head[..head.find(')').unwrap_or(head.len())];
    assert!(
        head.contains("InputStyle"),
        "判定が入力欄の属性を受け取らなくなっている。文字列だけでは claude の \
         AI ゴースト提案と人の下書きを区別できず、提案が出ているあいだ \
         #1273 の腕が丸ごと死ぬ（#1297 の本番 4 ペインがこれ）\n{}",
        locate(WAIT_PATH, &src, sig)
    );

    let body = fn_body(prod, sig);
    assert!(
        body.contains("classify_input_draft"),
        "判定が分類器を通っていない（属性を受け取っても使っていなければ同じこと）\n{}",
        locate(WAIT_PATH, &src, sig)
    );
    assert!(
        !body.contains("input_content_is_empty"),
        "文字列判定を判定関数へ直接書き戻している。空かどうかの決定は \
         `classify_input_draft` の 1 実装に閉じること（2 か所に散ると、\
         片方だけ属性を見る形 = #1297 そのものへ戻る）\n{}",
        locate(WAIT_PATH, &src, "input_content_is_empty")
    );

    // 実挙動: 同じ画面で属性の有無だけが結論を分ける
    assert_eq!(
        verdict(Some(InputStyle::Ghost)).overriding(),
        Some("1 shell"),
        "ゴースト提案のある入力待ちを覆せていない"
    );
    assert_eq!(
        verdict(None),
        BackgroundIdle::DraftIndistinguishable("1 shell".into()),
        "属性が取れないのに理由を残さず捨てている"
    );
    assert_eq!(
        verdict(None).blocked_reason(),
        Some(INPUT_DRAFT_UNREADABLE),
        "覆せなかった理由が応答へ出せない"
    );
}

#[test]
fn ghostを下書き扱いにしない() {
    let src = wait_src();
    let body = fn_body(production_source(&src), "pub fn classify_input_draft_in(");
    assert!(
        body.contains("is_user_draft"),
        "下書きの述語が `InputStyle::is_user_draft`（正本）を通っていない。\
         ここで `matches!` を書き直すと、ghost を下書きに数える形へ戻せてしまう\n{}",
        locate(WAIT_PATH, &src, "pub fn classify_input_draft_in(")
    );

    // dim だけの入力欄（= AI の提案）と空欄はどちらも「下書きなし」
    for style in [InputStyle::Ghost, InputStyle::None] {
        assert_eq!(
            classify_input_draft_in("merge the PR once CI is green", Some(style), false),
            InputDraft::Absent,
            "{style:?} を下書きとして扱っている（#1297 の再発）"
        );
        assert_eq!(
            verdict(Some(style)).overriding(),
            Some("1 shell"),
            "{style:?} の画面で覆せていない"
        );
    }
}

#[test]
fn userとmixedを空扱いにしない() {
    // 逆向きの崩れ。人が打ちかけた入力欄を「空」と読むと、
    // #1273 の安全側（下書きを踏み潰さない）が消える
    for style in [InputStyle::User, InputStyle::Mixed] {
        assert_eq!(
            classify_input_draft_in("draft by human", Some(style), false),
            InputDraft::Present,
            "{style:?} を下書きなしとして扱っている（人の入力を踏み潰す）"
        );
        assert_eq!(
            verdict(Some(style)),
            BackgroundIdle::No,
            "{style:?} の画面を入力待ちと読んでいる"
        );
    }
    // 「読めない」も覆さない側（安全側へ倒す）
    assert_eq!(
        classify_input_draft_in("draft by human", None, false),
        InputDraft::Indistinguishable
    );
    assert!(verdict(None).overriding().is_none());
}

#[test]
fn 属性はdispatchから配線されている() {
    let src = std::fs::read_to_string(crate_file("src/dispatch.rs")).expect("dispatch.rs を読める");
    let prod = production_source(&src);

    // 採る側: `read_pane` の `input_status` と同じ 1 実装から採っていること
    let collect = fn_body(prod, "fn collect_worker_status_ctx(");
    assert!(
        collect.contains("analyze_input"),
        "worker_status が入力欄の属性を採っていない。判定を直しても本番では \
         1 ビットも変わらない\n{}",
        locate(DISPATCH_PATH, &src, "fn collect_worker_status_ctx(")
    );

    // 渡す側: 覆す腕へ属性が届いていること
    let corrections = fn_body(prod, "fn apply_worker_status_corrections(");
    let arm = corrections
        .split("input_waiting_with_background_work")
        .nth(1)
        .expect("覆す腕が見つからない（#1273 の番犬も見ている）");
    let arm = &arm[..arm.find(");").unwrap_or(arm.len())];
    assert!(
        arm.contains("input_style"),
        "覆す腕へ属性を渡していない（素の画面テキストだけで判定している）\n{}",
        locate(DISPATCH_PATH, &src, "input_waiting_with_background_work(")
    );

    // 報告側: 覆せなかった理由が応答に載ること（master が watch のタイムアウトを待たずに済む）
    assert!(
        corrections.contains("idle_override_blocked"),
        "覆せなかった理由を応答へ載せていない\n{}",
        locate(DISPATCH_PATH, &src, "idle_despite_primary_busy\":")
    );
}

//! 同一バイナリの A/B: `TAKO_1297_LEGACY=1` で **#1297 前の永久 busy** が再現する
//!
//! #1297 の症状は「ターンは終わり背景シェルだけが残っているのに、入力欄に
//! claude の AI ゴースト提案（薄字）が出ているせいで `worker_status` が busy のまま」。
//! この env で旧挙動（入力欄を**文字列だけ**で判定する）へ戻る。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、
//! さらに **1 本のテスト関数**の中で見る。
//! 既定側は判断を引数で渡す `…_in(legacy_ghost=false)` を使う
//! （公開関数はこれに `legacy_1297()` を渡すだけなので、env が唯一の差になる）。

use tako_control::orchestrator::wait::{self, BackgroundIdle, InputDraft};
use tako_core::agent_support::Agent;
use tako_core::InputStyle;

/// #1297 の本番 pane 1636（#927 に沿ってラベルはプレースホルダ）。
/// ターンは終わり、背景シェルが 1 本、入力欄には AI のゴースト提案
const GHOST_SUGGESTION: &str = "\
⏺ PR を出しました。CI の結果を待ちます。

✻ Worked for 2m 04s · done 12:43 PM · 1 shell still running

─────────────────────────────────────────────
❯ merge the PR once CI is green
─────────────────────────────────────────────
  [model placeholder]  worker: placeholder task
  ⏵⏵ auto mode on · 1 shell · ← for agents";

#[test]
fn legacyでゴースト提案による永久busyが再現する() {
    // --- 既定（#1297 の修正あり）: dim の提案は「下書きなし」= 入力待ち ---
    let fixed = wait::input_waiting_with_background_work_in(
        GHOST_SUGGESTION,
        false,
        Some(Agent::Claude),
        Some(InputStyle::Ghost),
        false,
        false,
    );
    assert_eq!(
        fixed.overriding(),
        Some("1 shell"),
        "既定でゴースト提案つきの入力待ちを判定できていない"
    );
    assert_eq!(
        wait::classify_input_draft_in(
            "merge the PR once CI is green",
            Some(InputStyle::Ghost),
            false
        ),
        InputDraft::Absent
    );
    eprintln!("[1297-ab] 既定: {fixed:?}");

    // --- legacy: 属性を見ない = 「空でない」で打ち切る ---
    std::env::set_var("TAKO_1297_LEGACY", "1");
    assert!(wait::legacy_1297(), "env が読まれていない");
    let legacy = wait::input_waiting_with_background_work(
        GHOST_SUGGESTION,
        false,
        Some(Agent::Claude),
        Some(InputStyle::Ghost),
    );
    assert_eq!(
        legacy,
        BackgroundIdle::No,
        "legacy なのに属性を見ている（A/B が成立していない）"
    );
    assert_eq!(
        wait::classify_input_draft("merge the PR once CI is green", Some(InputStyle::Ghost)),
        InputDraft::Present,
        "legacy は「空でない = 下書きあり」で打ち切る形だった"
    );
    eprintln!("[1297-ab] legacy: {legacy:?}（= watch は永久に WORKER_IDLE を出せない）");

    // 入力欄が素で空なら legacy でも従来どおり倒せる（#1273 の腕そのものは生きている =
    // 差が出るのは「ゴースト提案が出ているとき」だけ、という切り分け）
    let empty = GHOST_SUGGESTION.replace("❯ merge the PR once CI is green", "❯");
    assert_eq!(
        wait::input_waiting_with_background_work(&empty, false, Some(Agent::Claude), None)
            .overriding(),
        Some("1 shell"),
        "legacy で #1273 まで壊れている（A/B の差が #1297 に閉じていない）"
    );
}

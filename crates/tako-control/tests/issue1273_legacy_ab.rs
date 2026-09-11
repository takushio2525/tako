//! 同一バイナリの A/B: `TAKO_1273_LEGACY=1` で **#1273 前の永久 busy** が再現する
//!
//! #1273 の症状は「一次シグナル（`claude agents --json`）が背景シェル / Monitor の
//! 生存で busy を返し続け、tako 側に busy → idle の経路が無いので watch が
//! `WORKER_IDLE` を出せない」。この env で旧挙動（画面で覆さない）へ戻る。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、
//! さらに **1 本のテスト関数**の中で見る。
//! 既定側は判断を引数で渡す `…_in(legacy=false)` を使う
//! （公開関数はこれに `legacy_1273()` を渡すだけなので、env が唯一の差になる）。

use tako_control::orchestrator::wait;
use tako_core::agent_support::Agent;

/// 実採取の形（#927 に沿ってラベルはプレースホルダ）。ターンは終わり、
/// 背景シェルと Monitor だけが生きている
const IDLE_WITH_BACKGROUND: &str = "\
⏺ リリースビルド中です。完了を待ちます。

✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running

─────────────────────────────────────────────
❯
─────────────────────────────────────────────
  [model placeholder]  worker: placeholder task
  ctx  40% ████░░░░░░
  ⏵⏵ auto mode on · 1 shell, 1 monitor · ← for agents";

#[test]
fn legacyで永久busyが再現する() {
    // --- 既定（#1273 の修正あり）: 入力待ちと判定できる ---
    let fixed = wait::input_waiting_with_background_work_in(
        IDLE_WITH_BACKGROUND,
        false,
        Some(Agent::Claude),
        // 属性が取れない経路でも、入力欄が素で空なら従来どおり倒せる（#1297）
        None,
        false,
        false,
    );
    assert_eq!(
        fixed.overriding(),
        Some("1 shell, 1 monitor"),
        "既定で入力待ちを判定できていない"
    );
    eprintln!("[1273-ab] 既定: {fixed:?}");

    // --- legacy: 画面で覆さない = 一次シグナルの busy がそのまま残る ---
    std::env::set_var("TAKO_1273_LEGACY", "1");
    assert!(wait::legacy_1273(), "env が読まれていない");
    let legacy = wait::input_waiting_with_background_work(
        IDLE_WITH_BACKGROUND,
        false,
        Some(Agent::Claude),
        None,
    );
    assert_eq!(
        legacy,
        wait::BackgroundIdle::No,
        "legacy なのに画面で覆している（A/B が成立していない）"
    );
    eprintln!("[1273-ab] legacy: {legacy:?}（= watch は永久に WORKER_IDLE を出せない）");
}

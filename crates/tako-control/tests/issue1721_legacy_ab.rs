//! 同一バイナリの A/B: `TAKO_1721_LEGACY=1` で **#1721 前の狭いペインの永久 busy** が再現する
//!
//! #1721 の症状は「狭いペインで claude がターンの終わりの行（`· <内訳> still running`）を
//! 内訳の途中で折り返し、1 行ずつ読む判定が申告を拾えず、背景作業だけが残る入力待ちの
//! worker が busy のまま（`idle_override_blocked` も null）」。この env で旧挙動へ戻る。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、`run_alone!` の子
//! （このテスト 1 本だけが走るプロセス）の中で立てる（#2400）。既定側は判断を引数で渡す
//! `background_work_summary_with(legacy_wrap = false)` で見る（`legacy_1721()` を env を立てる前に
//! 読むと `OnceLock` が false で固まり、legacy 側が戻らない）。

use tako_control::orchestrator::wait;
use tako_core::agent_support::Agent;

/// 実採取（2026-11-01 / claude 2.1.301・36 桁・器あり・mod なし）。修正前の実測は
/// `busy / agents-auto / idle_override_blocked: null / background_work: null`
const IDLE_36: &str = "\
❯ Run in background: sleep 1700

  Ran 1 shell command

⏺ RUN-DONE

✻ Crunched for 0s · done 6:12 PM · 1
  shell still running

────────────────────────────────────
❯
────────────────────────────────────
  ⏵⏵ auto mode on · 1 shell · ← f…";

#[test]
fn legacyで狭いペインの永久busyが再現する() {
    if tako_core::run_alone!(legacyで狭いペインの永久busyが再現する) {
        return;
    }
    // --- 既定（#1721 の修正あり）: 折り返しをつないで申告を読める ---
    let fixed = wait::background_work_summary_with(IDLE_36, Some(Agent::Claude), false, false);
    assert_eq!(fixed.as_deref(), Some("1 shell"), "既定で読めていない");
    eprintln!("[1721-ab] 既定の申告: {fixed:?}");

    // --- legacy: env を立てると公開関数（dispatch が呼ぶ口）が修正前の読み方へ戻る ---
    std::env::set_var("TAKO_1721_LEGACY", "1");
    assert!(wait::legacy_1721(), "env が読まれていない");
    let legacy =
        wait::input_waiting_with_background_work(IDLE_36, false, Some(Agent::Claude), None);
    assert_eq!(
        legacy,
        wait::BackgroundIdle::No,
        "legacy で修正前の永久 busy が再現していない"
    );
    assert_eq!(legacy.blocked_reason(), None, "修正前は理由も null");
    assert_eq!(
        wait::background_work_summary_for(IDLE_36, Some(Agent::Claude)),
        None,
        "legacy で `background_work` が修正前の null に戻っていない"
    );
    eprintln!("[1721-ab] legacy: {legacy:?}");
}

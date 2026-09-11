//! 同一バイナリの A/B: `TAKO_1367_LEGACY=1` で **#1367 の「器なしペインの busy が
//! 消費側へ届かない」**が再現する
//!
//! #372 で走査（`RunningChildrenScanState`）は器なしペインも数えるようになったが、
//! それを**引く側**は器のセッション名（`busy_sessions`）を見たままだった。
//! その結果、tmux 未導入 / persist OFF（= Homebrew cask の既定）では
//! 「走査は busy と知っているのに、close 確認（#566）も GUI モードの判定（#694）も
//! チャットの `agent_running` も false」という状態になっていた。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**の **1 本のテスト関数**で見る。
//! 既定側は判断を引数で渡す `is_pane_busy_in(legacy_backend_only=false)` を使う
//! （公開関数 `is_pane_busy` はこれに `legacy_1367()` を渡すだけなので env が唯一の差）。

use std::collections::HashMap;
use std::time::Instant;

use tako_control::agents::{
    self, scan_running_children_in, ProcessSnapshot, RunningChildrenScanTarget,
};

/// #1367 の実測構成（2026-09-12 の隔離 GUI と同じ形）:
/// 器なし = tako-app(10) → ペインのシェル(100) → 背景の `sleep 300`(200)、
/// 器あり = tako-s1 の pane_pid(500) → 子(600)
fn mixed_snapshot() -> ProcessSnapshot {
    ProcessSnapshot::from_parts_for_test(
        vec![("tako-s1:0.0".to_string(), 500u32)],
        [(10, 1), (100, 10), (200, 100), (500, 1), (600, 500)].into(),
        HashMap::new(),
    )
}

fn mixed_targets() -> Vec<RunningChildrenScanTarget> {
    vec![
        RunningChildrenScanTarget {
            pane: 1,
            backend_session: None,
            child_pid: Some(100),
            command_state: tako_core::CommandState::Idle,
            has_agent_role: false,
        },
        RunningChildrenScanTarget {
            pane: 2,
            backend_session: Some("tako-s1".to_string()),
            child_pid: None,
            command_state: tako_core::CommandState::Idle,
            has_agent_role: true,
        },
    ]
}

#[test]
fn legacyで器なしペインのbusyが消費側へ届かない() {
    let now = Instant::now();
    let state =
        scan_running_children_in(mixed_targets(), true, now, Some(&mixed_snapshot()), false);
    // 前提: 走査そのものは両方を掴んでいる（#372 の成果）
    assert_eq!(state.busy_panes, vec![1]);
    assert_eq!(state.busy_sessions, vec!["tako-s1".to_string()]);

    // --- 既定（#1367 の修正あり）: 器なしペインでも真 ---
    assert!(
        state.is_pane_busy_in(1, false),
        "既定で器なしペインの busy が消費側へ届いていない"
    );
    assert!(state.is_pane_busy_in(2, false), "器ありが壊れている");
    eprintln!(
        "[1367-ab] 既定: pane1(器なし)={} pane2(器あり)={}",
        state.is_pane_busy_in(1, false),
        state.is_pane_busy_in(2, false)
    );

    // --- legacy: 器のセッション名しか見ない = 器なしは永久に false ---
    std::env::set_var("TAKO_1367_LEGACY", "1");
    assert!(agents::legacy_1367(), "env が読まれていない");
    assert!(
        !state.is_pane_busy(1),
        "legacy なのに器なしペインが busy になっている（A/B が成立していない）"
    );
    eprintln!(
        "[1367-ab] legacy: pane1(器なし)={}（= close 確認も GUI 判定も agent_running も false）",
        state.is_pane_busy(1)
    );

    // 器ありは legacy でも従来どおり真（= 差が出るのは器なしペインだけ）
    assert!(
        state.is_pane_busy(2),
        "器あり構成は #1367 の修正前から真だった（ここが false なら A/B の腕が広すぎる）"
    );

    // 走査対象に居ないペインはどちらの腕でも false（材料の無いものを busy と言わない）
    assert!(!state.is_pane_busy(9999));
    assert!(!state.is_pane_busy_in(9999, false));

    // 空の走査結果（起動直後）はどちらの腕でも false
    let empty = tako_control::agents::RunningChildrenScanState::default();
    assert!(!empty.is_pane_busy(1));
}

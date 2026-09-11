//! 同一バイナリの A/B: `TAKO_372_LEGACY=1` で **#372 の「器なし構成で busy が常に 0」**
//! が再現する
//!
//! #372 の残っていた症状は「tmux を入れていない / persist OFF（= Homebrew cask の既定）の
//! 構成では、エージェントが動いていてもスリープ防止が発動しない」。
//! 走査対象が器（tmux / psmux）のセッション集合しかなく、器が無い構成では対象が常に空
//! だったため、`busy_agents` が無条件に 0 になっていた。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、
//! さらに **1 本のテスト関数**の中で見る。
//! 既定側は判断を引数で渡す `scan_running_children_in(legacy_backend_only=false)` を使う
//! （公開関数 `scan_running_children` はこれに `legacy_372()` を渡すだけなので、
//! env が唯一の差になる）。

use std::collections::HashMap;
use std::time::Instant;

use tako_control::agents::{
    self, scan_running_children, scan_running_children_in, ProcessSnapshot,
    RunningChildrenScanTarget,
};

/// #372 の実測構成（2026-09-11 の隔離 GUI と同じ形）:
/// tako-app(10) → ペインのシェル `/bin/zsh -l`(100) → 実行中コマンド `sleep 300`(200)。
/// 器（tmux）のセッションは 1 つも無い
fn direct_pane_snapshot() -> ProcessSnapshot {
    let parents: HashMap<u32, u32> = [(10, 1), (100, 10), (200, 100)].into();
    ProcessSnapshot::from_parts_for_test(Vec::new(), parents, HashMap::new())
}

fn direct_pane_targets() -> Vec<RunningChildrenScanTarget> {
    vec![RunningChildrenScanTarget {
        pane: 1,
        backend_session: None,
        child_pid: Some(100),
        command_state: tako_core::CommandState::Running,
        has_agent_role: false,
    }]
}

#[test]
fn legacyで器なし構成のbusyが0になる() {
    let now = Instant::now();
    let snapshot = direct_pane_snapshot();

    // --- 既定（#372 の修正あり）: 器が無くても PTY 直下の子から辿って数える ---
    let fixed = scan_running_children_in(direct_pane_targets(), true, now, Some(&snapshot), false);
    assert_eq!(
        fixed.busy_count(),
        1,
        "既定で器なしペインの実行中コマンドを数えられていない"
    );
    assert_eq!(fixed.busy_panes, vec![1]);
    eprintln!(
        "[372-ab] 既定: busy_count={} busy_panes={:?} busy_sessions={:?}",
        fixed.busy_count(),
        fixed.busy_panes,
        fixed.busy_sessions
    );

    // --- legacy: 器のセッションしか見ない = 対象が空 = busy 0 ---
    std::env::set_var("TAKO_372_LEGACY", "1");
    assert!(agents::legacy_372(), "env が読まれていない");
    let legacy = scan_running_children(direct_pane_targets(), true, now, Some(&snapshot));
    assert_eq!(
        legacy.busy_count(),
        0,
        "legacy なのに器なしペインを数えている（A/B が成立していない）"
    );
    assert!(legacy.busy_panes.is_empty());
    eprintln!(
        "[372-ab] legacy: busy_count={}（= while-agents-running が永久に発動しない）",
        legacy.busy_count()
    );

    // 器がある構成なら legacy でも従来どおり数えられる
    // （= 差が出るのは「器を持たないペイン」だけ、という切り分け）
    let with_backend = ProcessSnapshot::from_parts_for_test(
        vec![("tako-s1:0.0".to_string(), 500u32)],
        [(500, 1), (600, 500)].into(),
        HashMap::new(),
    );
    let backend_targets = vec![RunningChildrenScanTarget {
        pane: 2,
        backend_session: Some("tako-s1".to_string()),
        child_pid: None,
        command_state: tako_core::CommandState::Unknown,
        has_agent_role: true,
    }];
    let legacy_backend = scan_running_children(backend_targets, true, now, Some(&with_backend));
    assert_eq!(
        legacy_backend.busy_count(),
        1,
        "器あり構成は #372 の修正前から数えられていた（ここが 0 なら A/B の腕が広すぎる）"
    );
}

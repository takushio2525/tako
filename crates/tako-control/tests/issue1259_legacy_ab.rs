//! 同一バイナリの A/B: `TAKO_1259_LEGACY=1` で **#1259 前の無音**が再現する
//!
//! #1259 の症状は 2 つで、どちらもこの env で旧挙動へ戻る:
//!
//! 1. **顛末が `persist.log` へ 1 行も出ない**（`delivery::log_status`）
//! 2. **返らない peer の背景試行を上限なしで待つ**（`prompt_delivery::peer_wait`）
//!
//! env はプロセス全体の状態なので、他のテストと混ざらないよう**専用の
//! 統合テストバイナリ**に置き、さらに **1 本のテスト関数**の中で
//! 既定 → legacy の順に見る（並列実行でも取り違えない。`TAKO_DATA_DIR` も同じ理由）。

use std::path::{Path, PathBuf};

use tako_core::prompt_delivery::{peer_wait, PeerPhase, PeerWait, Stall, Status, PEER_BUDGET_SECS};

fn data_dir(tag: &str) -> PathBuf {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-1259-ab-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("一時 data dir を作れる");
    dir
}

fn persist_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("persist.log")).unwrap_or_default()
}

/// A/B 本体（記録と待ちを 1 本で見る。env はプロセス全体なので分割しない）。
///
/// - 記録: 既定は書く / legacy は 1 行も書かない（= #1259 の「痕跡ゼロ」）
/// - 待ち: 既定は上限で諦める / legacy は待ち続ける（= #1259 の無限待ち）
#[test]
fn legacyで無音と無限待ちが再現する() {
    let dir = data_dir("log");
    std::env::set_var("TAKO_DATA_DIR", &dir);
    std::env::remove_var("TAKO_1259_LEGACY");

    let waiting = Status::waiting(Stall::PeerPending, 42);
    let gave_up = Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 121);
    tako_control::delivery::log_status(Some(1627), None, &waiting);
    tako_control::delivery::log_status(Some(1627), None, &gave_up);
    let after_fix = persist_log(&dir);
    assert!(
        after_fix.contains("pane=1627")
            && after_fix.contains("理由=peer_pending")
            && after_fix.contains("顛末=flow_timeout"),
        "既定で顛末が記録されていない:\n{after_fix}"
    );
    let lines_after_fix = after_fix.lines().count();
    assert_eq!(lines_after_fix, 2, "2 回書いたのに {lines_after_fix} 行");
    // 何を比べたのかを出力へ残す（`--nocapture` で実物の行が読める）
    for line in after_fix.lines() {
        eprintln!("[1259-ab] 既定: {line}");
    }

    // --- legacy: 何も書かない（GUI の stderr だけ = 誰も読めない） ---
    std::env::set_var("TAKO_1259_LEGACY", "1");
    tako_control::delivery::log_status(Some(1627), None, &waiting);
    tako_control::delivery::log_status(Some(1627), None, &gave_up);
    let after_legacy = persist_log(&dir);
    assert_eq!(
        after_legacy.lines().count(),
        lines_after_fix,
        "TAKO_1259_LEGACY=1 なのに書いている（A/B が取れない）:\n{after_legacy}"
    );
    eprintln!("[1259-ab] legacy: 追記 0 行（合計 {lines_after_fix} 行のまま）");
    // 応答（`tako_send_input` / `tako_read_pane` の `delivery`）へ載る実物
    eprintln!(
        "[1259-ab] 応答 waiting: {}",
        serde_json::to_string(&waiting.to_json()).expect("JSON 化できる")
    );
    eprintln!(
        "[1259-ab] 応答 gave_up: {}",
        serde_json::to_string(&gave_up.to_json()).expect("JSON 化できる")
    );

    std::env::remove_var("TAKO_1259_LEGACY");
    std::env::remove_var("TAKO_DATA_DIR");
    let _ = std::fs::remove_dir_all(&dir);

    // --- ② 待ちの A/B ---
    // 上限を大きく超えても、既定は段階に応じて必ず決着する
    let long = PEER_BUDGET_SECS * 100;
    assert_eq!(
        peer_wait(PeerPhase::Resolving, long),
        PeerWait::FallbackToKeys
    );
    assert_eq!(
        peer_wait(PeerPhase::Sending, long),
        PeerWait::StopUnconfirmed
    );

    std::env::set_var("TAKO_1259_LEGACY", "1");
    for phase in [
        PeerPhase::Resolving,
        PeerPhase::Sending,
        PeerPhase::Verifying,
    ] {
        assert_eq!(
            peer_wait(phase, long),
            PeerWait::KeepWaiting,
            "TAKO_1259_LEGACY=1 なら上限を持たない（{phase:?}）"
        );
    }
    std::env::remove_var("TAKO_1259_LEGACY");
}

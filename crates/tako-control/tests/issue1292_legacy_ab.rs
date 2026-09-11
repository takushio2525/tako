//! 同一バイナリの A/B: `TAKO_1292_LEGACY=1` で **#1292 の「前任を名乗る」応答**が再現する
//!
//! `tako_send_input(await_prompt=true)` の応答へ載せる「積む前の状態」は、
//! `prompt_delivery_states` がペインを閉じるまで消えないせいで**決着済みの前任**を
//! 含む。素通しすると 1 時間前の `gave_up` が「積んだ瞬間に失敗した」と読め、
//! 10 秒前の `delivered` に至っては master が届いたと判断して監視をやめる。
//!
//! env はプロセス全体の状態なので、他のテストと混ざらないよう**専用の統合テスト
//! バイナリ**に置き、さらに **1 本のテスト関数**の中で既定 → legacy の順に見る
//! （並列実行でも取り違えない。`issue1259_legacy_ab.rs` と同じ理由）。

use tako_core::prompt_delivery::{pending_predecessor, Stall, Status};

#[test]
fn legacyで決着済みの前任が積む応答に出る() {
    let gave_up = Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 120).to_json();
    let delivered = Status::delivered("peer", "delivered", 3).to_json();
    let waiting = Status::waiting(Stall::PeerPending, 42).to_json();

    // --- 既定（#1292 修正後）: 決着済みは通さない ---
    std::env::remove_var("TAKO_1292_LEGACY");
    for settled in [&gave_up, &delivered] {
        assert_eq!(
            pending_predecessor(Some(settled.clone())),
            None,
            "既定で決着済みの前任を通している: {settled}"
        );
        eprintln!(
            "[1292-ab] 既定: {} -> queued（前任を名乗らない）",
            settled["state"]
        );
    }
    assert_eq!(
        pending_predecessor(Some(waiting.clone())),
        Some(waiting.clone()),
        "未決着まで落としている（#1259 の「いま何を待っているか」が消える）"
    );

    // --- legacy（#1292 前）: 決着済みがそのまま出る = Issue の症状 ---
    std::env::set_var("TAKO_1292_LEGACY", "1");
    for settled in [&gave_up, &delivered] {
        let out = pending_predecessor(Some(settled.clone()));
        assert_eq!(
            out.as_ref().map(|v| v["state"].clone()),
            Some(settled["state"].clone()),
            "legacy で前任が素通しされない（A/B の入口が効いていない）"
        );
        eprintln!(
            "[1292-ab] legacy: {} -> {}（前任を名乗る = #1292 の症状）",
            settled["state"],
            out.expect("legacy は素通し")["state"]
        );
    }
    // 未決着は両アームで同じ（#1259 の挙動は据え置き）
    assert_eq!(pending_predecessor(Some(waiting.clone())), Some(waiting));
    std::env::remove_var("TAKO_1292_LEGACY");
}

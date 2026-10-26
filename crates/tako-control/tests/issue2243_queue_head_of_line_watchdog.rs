//! 順番待ちの先頭詰まり（#2243）の番犬
//!
//! # なぜ要るか
//!
//! `tako queue` の割り当て（`tako_core::queue::admit`）は「先頭が入れなければ後ろも待つ」だった。
//! 先頭が「同じ target dir を使う処理が走っている」で待つ間、別の target dir の後ろの処理まで
//! 空き枠（実行 1/3）の前で待たされた（2026-10-17 に master のレビューの `cargo test` が約 5 分、
//! #2230 の worker が約 6 分）。target dir 待ちの件はその dir が空くまでどのみち走れないので、
//! 後ろに追い越させる。ただし
//!
//! - 同じ target dir の処理どうしの直列（cargo のロック待ちを避ける）は崩さない
//! - 枠・メモリでは追い越さない（資源の追い越しは設計書 §4.2 で S4 の後に判断）
//! - 1 件の待ちを追い越せるのは累計 `max_overtaken`（同時数 × 2）まで = 先頭が飢えない
//!
//! # 何を縛るか
//!
//! 1. 純関数 `admit` の振る舞い（上の 3 つと、追い越しそのもの）
//! 2. 調停役（`crates/tako-control/src/queue.rs` の `decide`）が追い越しを有効にして `admit` を呼ぶ
//!    （無効にできるのは A/B の env `TAKO_2243_LEGACY=1` だけ）
//!
//! 失敗の文言が逆戻りの名前（どの約束が崩れたか）を言う。

use std::path::PathBuf;

use tako_core::queue::{
    admit, max_overtaken, AdmitInput, Limits, Priority, RunningJob, WaitReason, WaitingJob,
};

const GIB: u64 = 1024;

fn limits(max_jobs: u32) -> Limits {
    Limits {
        max_jobs,
        reserve_mb: 4 * GIB,
        swap_guard_mb: 2 * GIB,
        max_wait_min: 30,
    }
}

fn running(id: u64, dir: &str) -> RunningJob {
    RunningJob {
        id,
        estimate_mb: 3 * GIB,
        used_mb: Some(3 * GIB),
        target_dir: Some(dir.into()),
        ..RunningJob::default()
    }
}

fn waiting(id: u64, at: u64, dir: &str, overtaken: u32) -> WaitingJob {
    WaitingJob {
        id,
        estimate_mb: 3 * GIB,
        target_dir: Some(dir.into()),
        priority: Priority::Normal,
        enqueued_at_ms: at,
        overtaken,
    }
}

fn input<'a>(l: Limits, run: &'a [RunningJob], wait: &'a [WaitingJob]) -> AdmitInput<'a> {
    AdmitInput {
        // 機の値は採れない扱い（同時数だけで決める = 機の状態で揺れない）
        machine: None,
        limits: l,
        running: run,
        waiting: wait,
        unmanaged: 0,
        regathering: false,
        overtake: true,
        release_after_peak: true,
        min_progress: true,
    }
}

#[test]
fn target_dir待ちの先頭は空き枠を塞がない() {
    let run = [running(1, "/r/target")];
    let wait = [
        waiting(10, 1, "/r/target", 0),
        waiting(11, 2, "/other/target", 0),
    ];
    let a = admit(&input(limits(3), &run, &wait));
    assert_eq!(
        a.grants,
        vec![11],
        "#2243 の先頭詰まりが戻った: 先頭が同じ target dir 待ちのとき、別の target dir の後ろの処理が\
         空き枠（実行 1/3）の前で待たされている。target dir だけで入れない件は後ろに追い越させる\
         （tako_core::queue::admit）: {a:?}"
    );
    assert_eq!(
        a.reason_of(10),
        Some(&WaitReason::TargetDir),
        "追い越させた先頭の理由は target dir のまま（待ちの表示が「何を待っているか」を言えない）"
    );
}

#[test]
fn 同じtarget_dirの直列は崩さない() {
    let run = [running(1, "/r/target")];
    let wait = [
        waiting(10, 1, "/r/target", 0),
        waiting(11, 2, "/other/target", 0),
        waiting(12, 3, "/r/target", 0),
    ];
    let a = admit(&input(limits(4), &run, &wait));
    assert!(
        !a.grants.contains(&10) && !a.grants.contains(&12),
        "同じ target dir の直列が崩れた: 持ち主が走っている dir の処理を入れた（cargo のロック待ちで\
         枠を持ったまま寝る）: {a:?}"
    );
    // 持ち主が終わった後も、同じ dir の中は受付の順に 1 件ずつ
    let wait = [
        waiting(10, 1, "/r/target", 0),
        waiting(12, 3, "/r/target", 0),
    ];
    let a = admit(&input(limits(4), &[], &wait));
    assert_eq!(
        a.grants,
        vec![10],
        "同じ target dir の中の順番が崩れた（後ろの 12 が先頭の 10 を追い越した / 2 件同時に入れた）: {a:?}"
    );
}

#[test]
fn 枠では追い越さない() {
    let run = [running(1, "/r/target")];
    let wait = [
        waiting(10, 1, "/r/target", 0),
        waiting(11, 2, "/a/target", 0),
        waiting(12, 3, "/b/target", 0),
    ];
    // 同時 2 件: 11 が入って枠が埋まる → 12 は待つ（11 が枠で止まったら後ろも待つ）
    let a = admit(&input(limits(2), &run, &wait));
    assert_eq!(
        a.grants,
        vec![11],
        "枠の数を超えて入れた / 追い越しが枠の判定を飛ばした: {a:?}"
    );
    let a = admit(&input(limits(1), &run, &wait));
    assert!(
        a.grants.is_empty() && matches!(a.reason_of(11), Some(WaitReason::MaxJobs { .. })),
        "枠が全部埋まっているのに追い越して入れた: {a:?}"
    );
}

#[test]
fn 追い越しの上限で先頭が飢えない() {
    let l = limits(3);
    let cap = max_overtaken(&l);
    let run = [running(1, "/r/target")];
    let wait = [waiting(10, 1, "/r/target", cap), waiting(11, 2, "/a", 0)];
    let a = admit(&input(l, &run, &wait));
    assert!(
        a.grants.is_empty(),
        "追い越しの上限（{cap} 件）が効いていない: 上限まで追い越された先頭をさらに追い越した\
         （後ろが先頭を追い越し続けると先頭が飢える）: {a:?}"
    );
    assert_eq!(a.stopped, Some(WaitReason::OvertakeLimit { max: cap }));
    // dir が空けば、同時 1 件でも先頭が後ろより先に入る
    let a = admit(&input(limits(1), &[], &wait));
    assert_eq!(
        a.grants,
        vec![10],
        "dir が空いたのに、上限まで追い越された先頭より後ろが先に入った: {a:?}"
    );
}

#[test]
fn 調停役は追い越しを有効にしてadmitを呼ぶ() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/queue.rs");
    let src = std::fs::read_to_string(&path).expect("調停役のソースを読める");
    let calls = src.matches("q::admit(&AdmitInput {").count();
    assert_eq!(
        calls,
        1,
        "{}: admit の呼び出しは decide の 1 か所のはず",
        path.display()
    );
    assert_eq!(
        src.matches("overtake: !legacy_2243(),").count(),
        1,
        "{}: 調停役が追い越しを有効にしていない（#2243 の先頭詰まりが本番で戻る）。\
         無効にできるのは A/B の env TAKO_2243_LEGACY=1 だけ",
        path.display()
    );
    assert!(
        src.contains(r#"pub const ENV_LEGACY_2243: &str = "TAKO_2243_LEGACY";"#)
            && src.contains("std::env::var_os(ENV_LEGACY_2243).is_some_and(|v| v == \"1\")"),
        "{}: 修正前の形へ戻す口は env TAKO_2243_LEGACY=1 だけのはず",
        path.display()
    );
}

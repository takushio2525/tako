//! 順番待ちの実行中の予約（#2254）の番犬
//!
//! # なぜ要るか
//!
//! `tako queue` の割り当て（`tako_core::queue::admit`）は、走っている処理の「まだ使っていない見込み」を
//! 最後まで「見込み − 今の使用量」で押さえていた。2026-10-17 の本番で、ピーク（最大 7.3 GB）を過ぎて
//! 数百 MB しか使っていない `cargo test --workspace` が 6.9 GB を押さえ続け、先頭の小さい処理（見込みは
//! test の既定 7 GB）が「空きメモリ不足（必要 13.9 GB）」で 30 分待って諦め、後ろの 7 件も順に諦めた
//! （全 worker のビルド・テストが止まった）。
//!
//! # 何を縛るか
//!
//! 1. ピークを過ぎた実行中は見込みを押さえない（純関数 `admit` / `RunningJob::pending_mb`）
//! 2. 本当に空きが無いとき（メモリ圧 warn / critical・空きが実際に足りない）は今まで通り待つ
//! 3. 小さい処理の見込みは過去の記録から（同じコマンド → 同じ形 → 区分の既定）
//! 4. 調停役（`crates/tako-control/src/queue.rs`）が 1〜3 を有効にしている（無効にできるのは
//!    A/B の env `TAKO_2254_LEGACY=1` だけ）
//! 5. 過去の記録（`<data_dir>/resources/history.jsonl`）の行に生の argv とパスが載らない
//!
//! 失敗の文言が逆戻りの名前（どの約束が崩れたか）を言う。

use std::path::{Path, PathBuf};

use tako_core::queue::{
    admit, effective_limits, estimate, history_keys, AdmitInput, EstimateFrom, HistoryRow, JobKind,
    LimitSettings, MachineView, Priority, RunningJob, WaitReason, WaitingJob, TEST_ESTIMATE_MB,
};
use tako_core::resources::Pressure;

const GIB: u64 = 1024;

fn machine(available_mb: u64, pressure: Pressure) -> MachineView {
    MachineView {
        total_mb: 36 * GIB,
        available_mb: Some(available_mb),
        pressure: Some(pressure),
        swap_used_mb: Some(0),
    }
}

/// 10/17 の本番の `cargo test --workspace`: 見込み 7 GB・最大 7.3 GB を過ぎて今 0.3 GB
fn past_peak() -> RunningJob {
    RunningJob {
        id: 1,
        estimate_mb: TEST_ESTIMATE_MB,
        used_mb: Some(300),
        peak_mb: 7475,
        ran_s: 600,
        peak_seen_s: Some(240),
        ..RunningJob::default()
    }
}

fn waiting(estimate_mb: u64) -> WaitingJob {
    WaitingJob {
        id: 10,
        estimate_mb,
        target_dir: None,
        priority: Priority::Normal,
        enqueued_at_ms: 1,
        overtaken: 0,
    }
}

fn input<'a>(m: MachineView, run: &'a [RunningJob], wait: &'a [WaitingJob]) -> AdmitInput<'a> {
    AdmitInput {
        machine: Some(m),
        // 36 GB の機の既定（同時 3・予約 3.6 GB）
        limits: effective_limits(&LimitSettings::default(), Some(36 * GIB)),
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
fn ピークを過ぎた実行中は見込みを押さえない() {
    let run = [past_peak()];
    assert_eq!(
        run[0].pending_mb(true),
        0,
        "ピークを過ぎた処理が「見込み − 今の使用量」を押さえ続けている（#2254 の逆戻り）"
    );
    // 本番の数字: 空き 11 GB・後ろは test の既定 7 GB → 7 + 予約 3.6 = 10.6 GB で入る
    // （修正前は 6.7 + 7 + 3.6 = 17.3 GB で待った）
    let wait = [waiting(TEST_ESTIMATE_MB)];
    let a = admit(&input(machine(11 * GIB, Pressure::Normal), &run, &wait));
    assert_eq!(
        a.grants,
        vec![10],
        "ピークを過ぎて 0.3 GB しか使っていない実行中が、後ろを空きメモリ不足で止めた（#2254）: {a:?}"
    );
    // 後ろが過去の記録の 1.1 GB なら、空き 8 GB でも入る
    let wait = [waiting(1152)];
    let a = admit(&input(machine(8 * GIB, Pressure::Normal), &run, &wait));
    assert_eq!(a.grants, vec![10], "{a:?}");
}

#[test]
fn 本当に空きが無ければ待つ() {
    let run = [past_peak()];
    let wait = [waiting(1152)];
    for (m, want) in [
        (machine(30 * GIB, Pressure::Warn), "pressure_warn"),
        (machine(30 * GIB, Pressure::Critical), "pressure_critical"),
        (machine(4 * GIB, Pressure::Normal), "memory"),
    ] {
        let a = admit(&input(m, &run, &wait));
        assert!(
            a.grants.is_empty() && a.blocked.as_ref().map(WaitReason::code) == Some(want),
            "安全側が崩れた: {m:?} で {want} のはずが {a:?}"
        );
    }
    // ピークの前（最大 2 GB・今 1.5 GB・記録なし）は今まで通り「見込み − 今の使用量」を押さえる
    let ramp = RunningJob {
        used_mb: Some(1536),
        peak_mb: 2048,
        peak_seen_s: Some(10),
        ..past_peak()
    };
    assert_eq!(
        ramp.pending_mb(true),
        TEST_ESTIMATE_MB - 1536,
        "走り出したばかりの処理の見込みを押さえていない"
    );
}

#[test]
fn 小さい処理の見込みは過去の記録から() {
    let repo = "/r/tako";
    let argv = |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_string).collect() };
    let row = |cmd: &str, peak_mb: u64| {
        let k = history_keys(&argv(cmd), repo).unwrap();
        HistoryRow {
            key: k.key,
            shape: k.shape,
            name: k.name,
            kind: JobKind::Test,
            peak_mb,
            ..HistoryRow::default()
        }
    };
    let history = vec![
        row("cargo test -p tako-control --test issue1994_a", 1100),
        row("cargo test -p tako-control --test issue2243_b", 1300),
        row("cargo test --workspace", 7300),
    ];
    // 同じコマンド
    let k = history_keys(&argv("cargo test -p tako-control --test issue1994_a"), repo);
    let e = estimate(JobKind::Test, k.as_ref(), &history);
    assert_eq!(
        (e.mb, e.from),
        (1152, EstimateFrom::History),
        "同じコマンドの過去の記録を引いていない（test の既定 7 GB を当てはめた）"
    );
    // 初めての --test の名前でも、同じ形（パッケージと --test）の記録から
    let k = history_keys(
        &argv("cargo test -p tako-control --test issue2254_new"),
        repo,
    );
    let e = estimate(JobKind::Test, k.as_ref(), &history);
    assert_eq!(
        (e.mb, e.from),
        (1408, EstimateFrom::Shape),
        "初めての --test の名前に test の既定 7 GB を当てはめた（同じ形の記録を引いていない）"
    );
}

#[test]
fn 調停役はピーク後の解放と過去の記録の見込みを有効にしている() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/queue.rs");
    let src = std::fs::read_to_string(&path).expect("調停役のソースを読める");
    assert_eq!(
        src.matches("release_after_peak: !legacy_2254(),").count(),
        1,
        "{}: 調停役がピーク後の解放を有効にしていない（#2254 の 30 分詰まりが本番で戻る）。\
         無効にできるのは A/B の env TAKO_2254_LEGACY=1 だけ",
        path.display()
    );
    assert!(
        src.contains("q::estimate(wire.kind, wire.history.as_ref(), history)"),
        "{}: 調停役が過去の記録から見込みを引いていない（小さい処理に区分の既定を当てはめる）",
        path.display()
    );
    assert!(
        src.contains(r#"pub const ENV_LEGACY_2254: &str = "TAKO_2254_LEGACY";"#)
            && src.contains("std::env::var_os(ENV_LEGACY_2254).is_some_and(|v| v == \"1\")"),
        "{}: 修正前の形へ戻す口は env TAKO_2254_LEGACY=1 だけのはず",
        path.display()
    );
}

/// 過去の記録の行（と wrapper が送る鍵）に生の argv とパスが載らない（#927 / 設計書 §6）
#[test]
fn 過去の記録に生のargvとパスが載らない() {
    let cwd = std::env::temp_dir().join(format!("tk2254-watchdog-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    let argv: Vec<String> = [
        "env",
        "SECRET_TOKEN=abc123",
        "/opt/tools/bin/cargo",
        "test",
        "--manifest-path",
        "/home/someone/proj/Cargo.toml",
        "--test",
        "issue_private_name",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let wire = tako_control::queue_wrapper::build_wire(
        &argv,
        &tako_control::queue_wrapper::Options::default(),
        Path::new(&cwd),
    );
    let keys = wire.history.clone().expect("鍵を送る");
    let row = HistoryRow {
        key: keys.key.clone(),
        shape: keys.shape.clone(),
        name: keys.name.clone(),
        kind: wire.kind,
        peak_mb: 1,
        ..HistoryRow::default()
    };
    let line = serde_json::to_string(&row).unwrap();
    let sent = serde_json::to_string(&keys).unwrap();
    for text in [&line, &sent] {
        for leaked in [
            "/",
            "someone",
            "abc123",
            "SECRET",
            "issue_private_name",
            "opt",
            "manifest",
        ] {
            assert!(
                !text.contains(leaked),
                "過去の記録 / 鍵に生の argv・パスが載った（{leaked}）: {text}"
            );
        }
    }
    assert_eq!(
        keys.name, "cargo test",
        "表示名はプログラム + サブコマンドだけ"
    );
    let _ = std::fs::remove_dir(&cwd);
}

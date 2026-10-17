//! 狭いペインの worker の停止判定（Issue #1847）
//!
//! # 1 つ目の症状: ENOTFOUND で止まった worker が error ではなく idle
//!
//! 2026-09-29 夜の回線断で worker 7 体が
//! `API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)` を最後に
//! 止まり、`status` は `idle`・`recommended_action: null` のまま朝まで誰も再開させなかった。
//!
//! 回線復帰での再開そのものは #2112 / #2157 / #2181（ネット復帰の即時再開。入力欄を錨に
//! 上へ辿る `net_stop::detect`）が担うようになったが、`status` の分類（`detect_worker_error`）は
//! 「空行を除いた末尾 15 行に `API Error` があるか」のままだった。32 桁ではエラーが 4 行・
//! フッターが 8 行・長いターンの締めの行が 2 行に割れ、実画面（下の fixture）でエラーの頭の行は
//! **下から 14 行目**。tmux の focus-events の案内・tako mod の帯・締めの後の通知行（#2157）の
//! どれか 2 行で窓の外へ出て idle に戻る。`net_stop.rs` の #2112 のテストは
//! 「既存の窓の判定はこの画面を api_error と読めない」をわざわざ固定していた。
//!
//! 修正は `detect_worker_error` の段 3'（窓で外れた後に `net_stop::detect` を通す）と、
//! worker_status がその判定へ画面全体を渡すこと（`detect_worker_error_on`）。
//!
//! # 2 つ目の症状: status の `choice_dialog` が末尾 30 行で null
//!
//! worker_status はダイアログを `recent_output`（末尾 30 行）で判定していた。32 桁の
//! AskUserQuestion は 36 行を超えるので、選択カーソルも箱の罫線も窓の外へ落ちる
//! （read_pane / respond は画面全体なので見つける = 食い違い）。events も同じ材料に揃える。
//!
//! # A/B
//!
//! `TAKO_1847_LEGACY=1`（env は `OnceLock` で一度だけ読むので、legacy の腕は自分自身を
//! 子プロセスで起動して観る）。

use std::process::Command;

use tako_control::orchestrator::wait::{self, WorkerErrorKind, WorkerEventKind};

/// claude 2.1.301 を到達できない API 先（`.invalid`）へ向け、再試行 7 回ぶん（1 分強）の
/// ターンの後に止まった実画面（32 桁 × 40 行・隔離 tmux）。ヘッダのパスは `/…/work`
const ENOTFOUND_32X40: &str = include_str!("fixtures/issue1847-enotfound-32x40.txt");

/// 32 桁の AskUserQuestion（説明の長い 4 択）の実画面（tako-core の fixture を共有）
const ASK_32X70_LONG: &str =
    include_str!("../../tako-core/tests/fixtures/issue1847-ask-32x70-long.txt");

const MESSAGE: &str =
    "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)";

const LEGACY_ENV: &str = "TAKO_1847_LEGACY";
const LEGACY_TEST: &str = "issue1847_legacyで窓の外の停止とダイアログを見落とす";

/// 実採取の focus-events の案内（同じ claude を 32 桁 × 24 行で採った画面の 1 行）
const HINT: &str = "  tmux focus-events off · add…";
/// tako mod の帯（FR-2.42.13: プロンプトの上に 1 行・区切りは ` | `）
const BAND: &str = "  tako | worker-1847";
/// 締めの後の通知行（#2157 の pane 2670 で実観測した文を 32 桁で折り返したもの）
const NOTICE: &str = "⏺ tako: plugin.json changed —\n  reloaded (modules:\n  hooks/register.ts unchanged;\n  14 hooks: …)";

/// 入力欄の上の区切り線の直前へ行を差し込む
fn above_input(screen: &str, extra: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let input = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with('❯'))
        .expect("入力欄");
    let rule = input - 1;
    format!(
        "{}\n{extra}\n{}",
        lines[..rule].join("\n"),
        lines[rule..].join("\n")
    )
}

/// ターンの締めの行（2 行に割れている）の直後へ行を差し込む
fn after_closing(screen: &str, extra: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let closing = lines
        .iter()
        .position(|l| l.trim_start().starts_with('✻'))
        .expect("締めの行");
    let end = closing + 2;
    format!(
        "{}\n\n{extra}\n{}",
        lines[..end].join("\n"),
        lines[end..].join("\n")
    )
}

/// worker_status の `recent_output` と同じ材料（末尾の空行を刈った最後の 30 行）
fn tail30(screen: &str) -> String {
    let mut lines: Vec<&str> = screen.lines().collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let from = lines.len().saturating_sub(30);
    lines[from..].join("\n")
}

/// 実画面に現実に足される行を足した形（どれもエラーの頭の行を 15 行の窓の外へ押し出す）
fn pushed_out_screens() -> Vec<(&'static str, String)> {
    vec![
        (
            "案内 + 帯",
            above_input(ENOTFOUND_32X40, &format!("{HINT}\n{BAND}")),
        ),
        (
            "通知 + 案内",
            above_input(&after_closing(ENOTFOUND_32X40, NOTICE), HINT),
        ),
    ]
}

fn assert_api_error(name: &str, got: Option<(WorkerErrorKind, String)>) {
    let (kind, detail) = got.unwrap_or_else(|| panic!("{name}: api_error と読めない"));
    assert_eq!(kind, WorkerErrorKind::ApiError, "{name}");
    assert_eq!(kind.recommended_action(), "resume", "{name}");
    assert_eq!(detail, MESSAGE, "{name}: 折り返しを結合した 1 論理行");
}

#[test]
fn issue1847_実画面の窓の境目() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 素の実画面は窓の中（下から 14 行目）= 修正前から api_error
    let (kind, detail) = wait::detect_worker_error(ENOTFOUND_32X40).expect("素の実画面");
    assert_eq!(kind, WorkerErrorKind::ApiError);
    assert_eq!(
        detail, "⏺ API Error: Can't reach",
        "窓の段で取れる（段 3' に来ない）"
    );
}

#[test]
fn issue1847_窓の外へ押し出された接続エラーをapi_errorと読む() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    for (name, screen) in pushed_out_screens() {
        assert_api_error(name, wait::detect_worker_error(&screen));
        // worker_status の材料（末尾 30 行 + 画面全体）。背の高いペインでは会話とフッターの
        // あいだが空行で埋まり、末尾 30 行からはエラーの頭の行が欠ける
        let tail = tail30(&screen);
        assert!(
            !tail.contains("API Error"),
            "{name}: 末尾 30 行にエラーの頭が入っている（fixture の前提が崩れた）"
        );
        assert_eq!(
            wait::detect_worker_error(&tail),
            None,
            "{name}: 末尾 30 行だけ"
        );
        assert_api_error(name, wait::detect_worker_error_on(&tail, Some(&screen)));
    }
}

#[test]
fn issue1847_止まっていない画面と接続系でないエラーはapi_errorにしない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let (_, pushed) = pushed_out_screens().remove(1);
    // もう誰かが「続き」を送った（最後の項目が人の発言）
    let resumed = above_input(&pushed, "\n❯ 続きを");
    assert_eq!(wait::detect_worker_error(&resumed), None, "人の発言の後");
    // 自動の再試行中（スピナーの行に Retrying）
    let retrying = above_input(&pushed, "✻ Can't    · Retrying in 6s ·\n  attempt 3/7");
    assert_eq!(wait::detect_worker_error(&retrying), None, "再試行中");
    // 接続系でない API エラー（5xx）は窓の外なら従来どおり拾わない（回線では直らない）
    let server = pushed.replace(
        "⏺ API Error: Can't reach\n  the API server —\n  check your internet or\n  DNS (ENOTFOUND)",
        "⏺ API Error: 500\n  {\"type\":\"error\",\n  \"error\":{\"type\":\n  \"api_error\"}}",
    );
    assert_ne!(server, pushed, "置き換えが効いている");
    assert_eq!(wait::detect_worker_error(&server), None, "5xx は対象外");
}

#[test]
fn issue1847_末尾30行に収まらないダイアログもeventsに出る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let tail = tail30(ASK_32X70_LONG);
    // 末尾 30 行だけ（修正前の材料）では見つからない
    assert_eq!(wait::choice_dialog_json(&tail), None);
    let dialog = wait::choice_dialog_json(ASK_32X70_LONG).expect("画面全体なら見つかる");
    assert_eq!(dialog["kind"], "select");
    assert_eq!(dialog["options"].as_array().map(Vec::len), Some(6));
    let events = wait::collect_worker_events_on("waiting", Some(&tail), Some(ASK_32X70_LONG), None);
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.kind, WorkerEventKind::ChoiceDialog { dialog_kind, .. } if dialog_kind == "select")),
        "events に choice_dialog が無い: {events:?}"
    );
    // ダイアログ中は質問として通知しない（#748）
    assert!(!events.iter().any(|e| e.kind == WorkerEventKind::Question));
}

/// 同一バイナリの A/B: `TAKO_1847_LEGACY=1` で 2 つの症状が戻る
#[test]
fn issue1847_legacyで窓の外の停止とダイアログを見落とす() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        for (name, screen) in pushed_out_screens() {
            assert_eq!(
                wait::detect_worker_error(&screen),
                None,
                "{name}: legacy で api_error と読めてしまう"
            );
            assert_eq!(
                wait::detect_worker_error_on(&tail30(&screen), Some(&screen)),
                None,
                "{name}"
            );
        }
        eprintln!("[1847-ab] legacy: 窓の外の接続エラー -> 検知なし（idle のまま）");
        return;
    }
    let exe = std::env::current_exe().expect("テストバイナリの場所");
    let out = Command::new(exe)
        .args([LEGACY_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(LEGACY_ENV, "1")
        .output()
        .expect("子プロセスを起動できる");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "legacy の腕が落ちた:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("[1847-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
}

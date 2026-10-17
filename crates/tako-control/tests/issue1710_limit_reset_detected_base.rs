//! Issue #1710 — 利用上限からの自動復帰が、画面に残った前日の告知を「いま」から解き直さない
//!
//! # 何が壊れていたか
//!
//! 2026-09-24 12:31 に週次上限（`You've hit your weekly limit · resets 7am (Asia/Tokyo)`）で
//! 止まった worker が、PC のスリープ / GUI の再起動をまたいだ 9/25 12:43 に
//! `reset_at = 1790373600`（= 9/26 07:00）/ `waiting_for_reset: あと 65844 秒` で待っていた。
//! 上限は 9/25 07:00 に解けていたのに 18 時間待つ判断をしていた。
//!
//! GUI の追跡状態（`LimitResumeTracker`）はメモリにしか無く、作り直すたびに画面の
//! `resets 7am` を**いま**から見た次の 7 時へ解いていた。告知が出た時刻（= 最初に見た時刻）
//! から解けば 9/25 07:00 で、いまはそれを過ぎているので即座に（マージン後に）再開すべき。
//!
//! # 直し方（この番犬が固定するもの）
//!
//! 1. 告知を最初に見た時刻をペインの属性に持つ（`Pane::limit_seen`。layout.json に載る）
//! 2. 解除時刻はその時刻から解く（`limit_stop::observe_limit_stop`）
//! 3. 追跡を（再）開始した時点でもう解けていたら `いま + マージン`（`limit_resume::resume_due`）
//!
//! 時計はすべて注入（実時間を待たない・比べない）。画面は採取形の fixture（本物の claude は
//! 起動しない）。`TAKO_1710_LEGACY=1` で走らせると修正後を固定した検査が落ちる（A/B）。
//! 修正前の計算そのものは `_in(.., legacy = true)` と従来の入口（`detect_limit_stop`）で
//! env に触らずに固定する。

use tako_control::layout::{self, LayoutFile, PaneMeta};
use tako_control::limit_stop::{detect_limit_stop, observe_limit_stop, LimitHint, ObservedLimit};
use tako_core::claude_mod::ModRateLimit;
use tako_core::limit_resume::{
    decide, resume_due, resume_due_in, step_seen, HoldReason, LimitSeen, LimitStop, ResumeAction,
    ResumeDecision, ResumeInput, SAFETY_MARGIN_SECS, STABLE_SECS,
};
use tako_core::{Pane, PaneId, PaneOrigin, SplitDirection, Workspace};

/// Asia/Tokyo（UTC+9）
const JST: i32 = 9 * 3600;
/// 2026-09-25 07:00 JST（実例の解除時刻）
const RESET_0925_0700: i64 = 1_790_287_200;
/// 2026-09-26 07:00 JST（修正前が待っていた時刻 = Issue の `reset_at` そのもの）
const RESET_0926_0700: i64 = 1_790_373_600;
/// 2026-09-24 12:31 JST（上限に当たった = 告知を最初に見た）
const DETECTED_0924_1231: i64 = RESET_0925_0700 - (18 * 3600 + 29 * 60);
/// 2026-09-25 12:43 JST（GUI を再起動して追跡を作り直した）
const RESTART_0925_1243: i64 = RESET_0925_0700 + 5 * 3600 + 43 * 60;

/// claude の週次上限で idle のまま止まった画面（#1093 の採取形と同じ幾何）
fn weekly_idle(reset: &str) -> Vec<String> {
    format!(
        "⏺ 実装を進めます

  ⎿  You've hit your weekly limit · resets {reset}

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  ⏵⏵ accept edits on
  ⏸ 待機中 · ? for shortcuts"
    )
    .lines()
    .map(str::to_string)
    .collect()
}

/// 同じ告知の上に上限の対処ダイアログが出た画面（#748 の採取形）
fn weekly_dialog(reset: &str) -> Vec<String> {
    format!(
        "⏺ 実装を進めます
  ⎿  You've hit your weekly limit · resets {reset}

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   What do you want to do?

   ❯ 1. Stop and wait for limit to reset
     2. Upgrade to Max 20x for higher session limits every month
     3. Continue with usage credits

   Enter to confirm · Esc to cancel"
    )
    .lines()
    .map(str::to_string)
    .collect()
}

/// 1 回目の観測（GUI が上限に当たった瞬間を見た）。返り値の `seen` がペインの記憶になる
fn first_observation(lines: &[String], at: i64, tz: i32) -> ObservedLimit {
    observe_limit_stop(lines, None, at, tz, None).expect("上限として検知される")
}

/// 追跡を作り直した GUI の判断（記憶 → 検知時刻・作り直した時刻 → 追跡の開始）
fn decide_after(
    stop: &LimitStop,
    detected_at: i64,
    tracked_since: i64,
    now: i64,
) -> ResumeDecision {
    decide(&ResumeInput {
        enabled: true,
        stop: Some(stop),
        now,
        first_seen: detected_at,
        tracked_since,
        stable_since: Some(tracked_since),
        attempts: 0,
        last_attempt: None,
        user_draft: false,
    })
}

// ---------------------------------------------------------------------------
// 受け入れ条件 1: 検知 = 前日 12:31・現在 = 翌日 12:43・画面に resets 7am
// ---------------------------------------------------------------------------

#[test]
fn 受け入れ1_前日の告知は検知時刻から解き今からマージン後に再開する() {
    let lines = weekly_idle("7am (Asia/Tokyo)");
    let day1 = first_observation(&lines, DETECTED_0924_1231, JST);
    assert_eq!(day1.seen.detected_at, DETECTED_0924_1231);
    assert_eq!(
        day1.stop.reset_at,
        Some(RESET_0925_0700),
        "検知時は 9/25 07:00"
    );

    // 翌日 12:43 に GUI を再起動。画面には前日の告知が残っていて、記憶は layout.json から戻る
    let after =
        observe_limit_stop(&lines, Some(&day1.seen), RESTART_0925_1243, JST, None).expect("検知");
    assert_eq!(
        after.stop.reset_at,
        Some(RESET_0925_0700),
        "記憶の時刻（9/24 12:31）から解いていない = いまから解き直して翌日の 7 時へ進んだ"
    );
    assert_eq!(after.seen, day1.seen, "同じ告知なので記憶は書き換えない");

    let due = resume_due(&after.stop, after.seen.detected_at, RESTART_0925_1243);
    assert_eq!(
        due,
        RESTART_0925_1243 + SAFETY_MARGIN_SECS,
        "resume_at = 現在 + マージン になっていない"
    );
    // 画面の静止（10 秒）を待ったところではまだマージンの中
    assert_eq!(
        decide_after(
            &after.stop,
            after.seen.detected_at,
            RESTART_0925_1243,
            RESTART_0925_1243 + STABLE_SECS
        ),
        ResumeDecision::Hold {
            reason: HoldReason::WaitingForReset,
            remaining_secs: SAFETY_MARGIN_SECS - STABLE_SECS,
        }
    );
    // マージンを過ぎたら発動する
    assert_eq!(
        decide_after(
            &after.stop,
            after.seen.detected_at,
            RESTART_0925_1243,
            RESTART_0925_1243 + SAFETY_MARGIN_SECS
        )
        .action(),
        Some(ResumeAction::Nudge)
    );
}

#[test]
fn 受け入れ1_修正前は翌々日の7時を待つ() {
    // 修正前の経路 = 追跡を作り直すたびに「いま」を基準に解く（`detect_limit_stop` は
    // 観測時刻 = いまを渡す従来の入口）。Issue の実測値がそのまま出ることを固定する
    let lines = weekly_idle("7am (Asia/Tokyo)");
    let legacy = detect_limit_stop(&lines, RESTART_0925_1243, JST).expect("検知");
    assert_eq!(
        legacy.reset_at,
        Some(RESET_0926_0700),
        "修正前は 9/26 07:00（Issue の reset_at=1790373600）"
    );
    let due = resume_due_in(&legacy, RESTART_0925_1243, RESTART_0925_1243, true);
    assert_eq!(due, RESET_0926_0700 + SAFETY_MARGIN_SECS);
    // 12:44:36 の時点で「あと 65844 秒」（Issue の `last_outcome` と同じ数）
    assert_eq!(due - (RESTART_0925_1243 + 96), 65_844);
}

// ---------------------------------------------------------------------------
// 受け入れ条件 2: 回帰なし + 境界
// ---------------------------------------------------------------------------

#[test]
fn 受け入れ2_当日中の再評価は今まで通り翌日の7時を待つ() {
    let lines = weekly_idle("7am (Asia/Tokyo)");
    let day1 = first_observation(&lines, DETECTED_0924_1231, JST);
    // 同じ日の 13:00 に作り直す（まだ解けていない）
    let at_1300 = DETECTED_0924_1231 + 29 * 60;
    let after = observe_limit_stop(&lines, Some(&day1.seen), at_1300, JST, None).expect("検知");
    assert_eq!(after.stop.reset_at, Some(RESET_0925_0700), "翌日 7:00");
    let due = resume_due(&after.stop, after.seen.detected_at, at_1300);
    assert_eq!(due, RESET_0925_0700 + SAFETY_MARGIN_SECS, "早撃ちしない");
    assert_eq!(
        decide_after(
            &after.stop,
            after.seen.detected_at,
            at_1300,
            at_1300 + STABLE_SECS
        ),
        ResumeDecision::Hold {
            reason: HoldReason::WaitingForReset,
            remaining_secs: RESET_0925_0700 + SAFETY_MARGIN_SECS - (at_1300 + STABLE_SECS),
        }
    );
    // 記憶の無い初回の検知は修正前と同じ値（= 通常運転は 1 ビットも変わらない）
    assert_eq!(
        day1.stop.reset_at,
        detect_limit_stop(&lines, DETECTED_0924_1231, JST)
            .unwrap()
            .reset_at
    );
}

#[test]
fn 境界_日付をまたいだ再起動() {
    // 9/24 23:50 JST に「resets 1am」= 解除は 9/25 01:00
    let detected = RESET_0925_0700 - 7 * 3600 - 10 * 60;
    let reset_0100 = RESET_0925_0700 - 6 * 3600;
    let lines = weekly_idle("1am (Asia/Tokyo)");
    let day1 = first_observation(&lines, detected, JST);
    assert_eq!(day1.stop.reset_at, Some(reset_0100));

    // 日付をまたいだ 00:30 に再起動（まだ解けていない）→ 01:00 を待つ（24 時間先へ飛ばない）
    let at_0030 = reset_0100 - 30 * 60;
    let a = observe_limit_stop(&lines, Some(&day1.seen), at_0030, JST, None).unwrap();
    assert_eq!(a.stop.reset_at, Some(reset_0100));
    assert_eq!(
        resume_due(&a.stop, a.seen.detected_at, at_0030),
        reset_0100 + SAFETY_MARGIN_SECS
    );

    // 01:30 に再起動（もう解けている）→ いま + マージン。修正前は翌日 01:00 まで待った
    let at_0130 = reset_0100 + 30 * 60;
    let b = observe_limit_stop(&lines, Some(&day1.seen), at_0130, JST, None).unwrap();
    assert_eq!(b.stop.reset_at, Some(reset_0100));
    assert_eq!(
        resume_due(&b.stop, b.seen.detected_at, at_0130),
        at_0130 + SAFETY_MARGIN_SECS
    );
    assert_eq!(
        detect_limit_stop(&lines, at_0130, JST).unwrap().reset_at,
        Some(reset_0100 + 86_400),
        "修正前の計算（いまから解く）は翌日の 01:00"
    );
}

#[test]
fn 境界_東京以外のタイムゾーン() {
    // (表記, UTC からのずれ) — 負のずれ（ローカルの日付が UTC より遅れる）と 30 分刻み
    for (zone, tz) in [
        ("America/Los_Angeles", -7 * 3600),
        ("Asia/Kolkata", 5 * 3600 + 30 * 60),
    ] {
        // ローカル 2026-09-24 12:31 / 2026-09-25 07:00 / 2026-09-25 12:43 を各地の時刻で作る
        let local_midnight_0925 = 1_790_294_400 - i64::from(tz); // 2026-09-25 00:00 ローカル
        let reset = local_midnight_0925 + 7 * 3600;
        let detected = local_midnight_0925 - 86_400 + 12 * 3600 + 31 * 60;
        let restart = local_midnight_0925 + 12 * 3600 + 43 * 60;
        let lines = weekly_idle(&format!("7am ({zone})"));

        let day1 = first_observation(&lines, detected, tz);
        assert_eq!(day1.stop.reset_at, Some(reset), "{zone}: 検知時の解除時刻");
        let after = observe_limit_stop(&lines, Some(&day1.seen), restart, tz, None).unwrap();
        assert_eq!(
            after.stop.reset_at,
            Some(reset),
            "{zone}: 記憶の時刻から解く"
        );
        assert_eq!(
            resume_due(&after.stop, after.seen.detected_at, restart),
            restart + SAFETY_MARGIN_SECS,
            "{zone}: いま + マージン"
        );
        assert_eq!(
            detect_limit_stop(&lines, restart, tz).unwrap().reset_at,
            Some(reset + 86_400),
            "{zone}: 修正前の計算は翌日の 7 時"
        );
    }
}

#[test]
fn 境界_違う告知に変わったらいまを基準にする() {
    let day1 = first_observation(&weekly_idle("7am (Asia/Tokyo)"), DETECTED_0924_1231, JST);
    // 翌日 12:43 の画面は別の告知（5h 枠・19:50 解除）= いま初めて見た
    let lines = weekly_idle("7:50pm (Asia/Tokyo)");
    let after = observe_limit_stop(&lines, Some(&day1.seen), RESTART_0925_1243, JST, None).unwrap();
    assert_ne!(after.seen.notice, day1.seen.notice);
    assert_eq!(
        after.seen.detected_at, RESTART_0925_1243,
        "記憶は新しい告知で書き換わる"
    );
    assert_eq!(
        after.stop.reset_at,
        Some(RESET_0925_0700 + 12 * 3600 + 50 * 60),
        "当日 19:50（前日の記憶で解くと前日 19:50 = 早撃ちになる）"
    );
}

#[test]
fn 境界_日付つきの告知は基準に依らない() {
    // 週枠の通常形（24 時間より先は日付つき。#1096）。記憶の時刻が古くても同じ絶対時刻
    let lines = weekly_idle("Sep 26, 7am (Asia/Tokyo)");
    let day1 = first_observation(&lines, DETECTED_0924_1231, JST);
    assert_eq!(day1.stop.reset_at, Some(RESET_0926_0700));
    let after = observe_limit_stop(&lines, Some(&day1.seen), RESTART_0925_1243, JST, None).unwrap();
    assert_eq!(after.stop.reset_at, Some(RESET_0926_0700));
    assert_eq!(
        resume_due(&after.stop, after.seen.detected_at, RESTART_0925_1243),
        RESET_0926_0700 + SAFETY_MARGIN_SECS,
        "まだ先の解除は今までどおり待つ"
    );
}

#[test]
fn 境界_ダイアログ型と_idle型は同じ告知として見分ける() {
    // 同じエピソードの中で上限の対処ダイアログが出ても、見出しが同じなら記憶は同じ鍵
    let idle = first_observation(&weekly_idle("7am (Asia/Tokyo)"), DETECTED_0924_1231, JST);
    let dialog = observe_limit_stop(
        &weekly_dialog("7am (Asia/Tokyo)"),
        Some(&idle.seen),
        RESTART_0925_1243,
        JST,
        None,
    )
    .expect("ダイアログ型として検知");
    assert_eq!(dialog.seen, idle.seen, "型が変わっても記憶を引き継ぐ");
    assert_eq!(dialog.stop.reset_at, Some(RESET_0925_0700));
}

// ---------------------------------------------------------------------------
// 受け入れ条件 3: GUI の再起動（layout.json の復元）・スリープ明けの描き直し
// ---------------------------------------------------------------------------

#[test]
fn 受け入れ3_再起動の復元を通しても記憶が効きリセット済みならすぐ再開する() {
    let lines = weekly_idle("7am (Asia/Tokyo)");
    // 9/24 12:31: 上限で止まったペインの記憶を GUI が書く（drive_limit_autoresume と同じ規則）
    let day1 = first_observation(&lines, DETECTED_0924_1231, JST);
    let step = step_seen(None, Some(&day1.seen), None, DETECTED_0924_1231);
    let root = Pane::new(PaneOrigin::User);
    let root_id = root.id();
    let mut ws = Workspace::new("1", root);
    let worker = Pane::new(PaneOrigin::Cli);
    let worker_id = worker.id();
    ws.active_tab_mut()
        .tree_mut()
        .split(root_id, SplitDirection::Right, worker)
        .unwrap();
    ws.active_tab_mut()
        .tree_mut()
        .get_mut(worker_id)
        .unwrap()
        .set_limit_seen(step.write.expect("初めての告知は書く"));

    // 終了時の save_layout → 再起動時の復元（layout.json の往復）
    let json =
        serde_json::to_string(&layout::capture(&ws, &|_| PaneMeta::default(), None)).unwrap();
    let file: LayoutFile = serde_json::from_str(&json).unwrap();
    let (restored, _) = layout::restore(&file).expect("復元できる");
    let memo: Option<LimitSeen> = restored
        .pane_anywhere(PaneId::from_raw(worker_id.as_u64()))
        .and_then(|p| p.limit_seen())
        .cloned();
    assert_eq!(memo.as_ref(), Some(&day1.seen), "記憶が再起動をまたがない");

    // 9/25 12:43: 再起動直後の最初の tick は器の描き直しで画面が空（告知が見えない）
    let blank: Vec<String> = Vec::new();
    assert!(observe_limit_stop(&blank, memo.as_ref(), RESTART_0925_1243, JST, None).is_none());
    let t1 = step_seen(memo.as_ref(), None, None, RESTART_0925_1243);
    assert_eq!(t1.write, None, "描き直しの 1 tick で記憶を捨てた");
    // 2 秒後の tick で画面が戻る → 追跡を作る（tracked_since = この時刻）
    let t2_at = RESTART_0925_1243 + 2;
    let observed = observe_limit_stop(&lines, memo.as_ref(), t2_at, JST, None).expect("検知");
    let t2 = step_seen(memo.as_ref(), Some(&observed.seen), t1.absent_since, t2_at);
    assert_eq!(t2.write, None, "同じ告知なので書き換えない");
    assert_eq!(t2.absent_since, None);
    assert_eq!(observed.stop.reset_at, Some(RESET_0925_0700));
    assert_eq!(
        resume_due(&observed.stop, observed.seen.detected_at, t2_at),
        t2_at + SAFETY_MARGIN_SECS
    );
    assert_eq!(
        decide_after(
            &observed.stop,
            observed.seen.detected_at,
            t2_at,
            t2_at + SAFETY_MARGIN_SECS
        )
        .action(),
        Some(ResumeAction::Nudge),
        "リセット済みなのに再開しない"
    );
}

// ---------------------------------------------------------------------------
// 受け入れ条件 4: tako mod が絶対時刻を報告しているペイン
// ---------------------------------------------------------------------------

fn mod_limit(percent: f64, resets_at: &str) -> ModRateLimit {
    ModRateLimit {
        kind: "seven_day".into(),
        percent_used: percent,
        resets_at: Some(resets_at.into()),
        observed_at: 0,
    }
}

#[test]
fn 受け入れ4_modの絶対時刻は基準に依らずそのまま採る() {
    // mod の `$.session.usage()` の rateLimits（秒精度・タイムゾーンつき）。
    // 告知が GUI の停止中に出て記憶が無いペインでも、mod が届いていれば正しい
    let hint = LimitHint::from_mod(&[mod_limit(100.0, "2026-09-24T22:00:00.000Z")]);
    assert_eq!(hint.reset_at, Some(RESET_0925_0700));
    let lines = weekly_idle("7am (Asia/Tokyo)");
    let o = observe_limit_stop(&lines, None, RESTART_0925_1243, JST, Some(&hint)).unwrap();
    assert!(o.reset_from_hint);
    assert_eq!(o.stop.reset_at, Some(RESET_0925_0700));
    assert_eq!(
        resume_due(&o.stop, o.seen.detected_at, RESTART_0925_1243),
        RESTART_0925_1243 + SAFETY_MARGIN_SECS,
        "過去の解除時刻はいま + マージン"
    );
    // mod の報告がまだ届いていない tick（GUI の再起動直後）は画面から解く = 記憶が要る
    let no_hint = observe_limit_stop(&lines, None, RESTART_0925_1243, JST, None).unwrap();
    assert!(!no_hint.reset_from_hint);
    assert_eq!(no_hint.stop.reset_at, Some(RESET_0926_0700));
}

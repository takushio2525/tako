//! 番犬: 狭いペインで折り返した申告をつないで読む（#1721）
//!
//! ## なぜ止めるのか
//!
//! #1273 の「背景作業つき入力待ちを idle と読む」腕は、ターンの終わりの行
//! `✻ … · done <時刻> · <内訳> still running` を**1 行ずつ**読んでいた。狭いペインでは claude が
//! この行を 2 桁の字下げで折り返し、内訳が `· 1` と `shell still running` に割れるので申告が拾えず、
//! 報告を終えて入力待ちの worker が背景の sleep / Monitor が残るだけで busy のままだった
//! （2026-11-01 実測・claude 2.1.301・36 / 24 桁: `busy / agents-auto / idle_override_blocked: null`）。
//!
//! 同じ形の再発は単体テストの写しだけでは止まらない（つなぎ直しを外す・前のターンの申告を読む
//! ガードを外す・理由を null に戻す、はどれも写しの一部を書き換えると通ってしまう）ので、
//! ソースの形と実採取の画面での挙動の両方で見て、外れたら `file:line` で名指す。
//!
//! ## 4 本立て
//!
//! 1. [`折り返した申告をつないで読む`] — `claude_background_work_summary_in` が
//!    `claude_wrapped_lines` を通り、実採取の 3 画面（50 / 36 / 24 桁）で内訳が読めること
//! 2. [`前のターンの申告で生成中を覆さない`] — つないだ申告に `nothing_after_turn_end` を
//!    要求すること。狭いペインはスピナーの経過時間も `esc to interrupt` も切れるので、これを外すと
//!    次のターンを生成中の画面を前のターンの申告で idle と誤報する（実測で mod あり / なしとも起きた）
//! 3. [`読めないときは理由を名指す`] — 申告が読めず状態行が幅で切れていれば
//!    `idle_override_blocked = status_line_truncated`（null に戻すと master が理由を読めない）
//! 4. [`修正前へ戻すab口がある`] — `TAKO_1721_LEGACY=1` の口が判定と読み方の両方に効くこと

use std::path::{Path, PathBuf};

use tako_control::orchestrator::wait;
use tako_core::agent_support::Agent;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテストモジュールで走査範囲が消えない（行番号も保たれる）
#[path = "common/production_range.rs"]
mod production_range;

const WAIT_PATH: &str = "crates/tako-control/src/orchestrator/wait.rs";

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn wait_src() -> String {
    std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs を読める")
}

/// 本番コードだけの眺め（テストの写しや番犬自身の文字列に当たらない）
fn production_source(src: &str) -> String {
    production_range::production(src, WAIT_PATH)
}

/// コメント行を落とす（説明文に書いた引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の本文（シグネチャから次の行頭 `}` まで）を切り出す
fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    without_comments(&rest[..end])
}

/// マッチした行を「ファイル:行番号」つきで名指しする
fn locate(src: &str, needle: &str) -> String {
    src.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| format!("{WAIT_PATH}:{} {}", i + 1, l.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 実採取（2026-11-01 / claude 2.1.301・一時の設定 dir + 偽 Messages API・器あり・隔離 GUI）。
/// ターンの終わりの行と入力欄のあいだの空行だけ 1 行に詰めた
const IDLE_50: &str = "\
⏺ RUN-DONE

✻ Brewed for 0s · done 6:10 PM · 1 shell still
  running

tako | pane 2 | tab 2 [ compact ]             [-]

──────────────────────────────────────────────────
❯
──────────────────────────────────────────────────
  ⏵⏵ auto mode on · 1 shell · ← for agents · ↓ …";

const IDLE_36: &str = "\
⏺ RUN-DONE

✻ Crunched for 0s · done 6:12 PM · 1
  shell still running

────────────────────────────────────
❯
────────────────────────────────────
  ⏵⏵ auto mode on · 1 shell · ← f…";

const IDLE_24: &str = "\
⏺ RUN-DONE

✻ Brewed for 0s · done
  6:16 PM · 1 shell
  still running

────────────────────────
❯
────────────────────────
  ⏵⏵ auto mode on · 1";

/// 36 桁で次のターンを生成中（実採取）。前のターンの申告が折り返したまま上に残り、
/// スピナーは `✳ Gitifying…` だけ・状態行の `esc to interrupt` も `…` で切れる
const BUSY_36: &str = "\
⏺ RUN-DONE

✻ Crunched for 0s · done 6:20 PM · 1
  shell still running

❯ Reply slowly: 60

✳ Gitifying…
  ⎿  Tip: Running multiple Claude
     sessions? Press ← on an empty
     prompt to see them all in one
     place

────────────────────────────────────
❯
────────────────────────────────────
  ⏵⏵ auto mode on · 1 shell · esc…";

fn verdict(screen: &str) -> wait::BackgroundIdle {
    wait::input_waiting_with_background_work_in(
        screen,
        false,
        Some(Agent::Claude),
        None,
        false,
        false,
    )
}

#[test]
fn 折り返した申告をつないで読む() {
    let src = wait_src();
    let sig = "pub(crate) fn claude_background_work_summary_in(";
    let body = fn_body(&production_source(&src), sig);
    assert!(
        body.contains("claude_wrapped_lines("),
        "申告を 1 行ずつしか読んでいない。狭いペインでは claude が内訳の途中で折り返すので、\
         背景作業だけが残る入力待ちの worker が busy のままになる（#1721）\n{}",
        locate(&src, sig)
    );
    for (cols, screen) in [(50, IDLE_50), (36, IDLE_36), (24, IDLE_24)] {
        assert_eq!(
            wait::background_work_summary_with(screen, Some(Agent::Claude), false, false)
                .as_deref(),
            Some("1 shell"),
            "{cols} 桁の実採取の申告が読めない\n{}",
            locate(&src, sig)
        );
        assert_eq!(
            verdict(screen).overriding(),
            Some("1 shell"),
            "{cols} 桁の背景作業つき入力待ちを覆せない\n{}",
            locate(&src, "pub fn input_waiting_with_background_work_in(")
        );
    }
}

#[test]
fn 前のターンの申告で生成中を覆さない() {
    let src = wait_src();
    let sig = "pub(crate) fn claude_background_work_summary_in(";
    let body = fn_body(&production_source(&src), sig);
    assert!(
        body.contains("nothing_after_turn_end("),
        "つないだ申告が画面の最後のメッセージか（下に次の依頼もスピナーも無いか）を見ていない。\
         狭いペインはスピナーの経過時間も `esc to interrupt` も切れて `screen_looks_busy` が\
         何も引けないので、前のターンの申告で生成中を idle と誤報する（#1721 の実測で起きた）\n{}",
        locate(&src, sig)
    );
    assert_eq!(
        wait::background_work_summary_with(BUSY_36, Some(Agent::Claude), false, false),
        None,
        "次のターンを生成中の画面で前のターンの申告を読んでいる\n{}",
        locate(&src, "fn nothing_after_turn_end(")
    );
    assert_eq!(
        verdict(BUSY_36).overriding(),
        None,
        "次のターンを生成中の狭い画面を入力待ちと読んでいる\n{}",
        locate(&src, sig)
    );
}

#[test]
fn 読めないときは理由を名指す() {
    let src = wait_src();
    let prod = &production_source(&src);
    let sig = "pub fn input_waiting_with_background_work_in(";
    assert!(
        fn_body(prod, sig).contains("BackgroundIdle::StatusLineTruncated"),
        "申告が読めず状態行が切れた画面で理由を返していない（idle_override_blocked が null に戻る）\n{}",
        locate(&src, sig)
    );
    assert!(
        fn_body(prod, "pub fn blocked_reason(").contains("STATUS_LINE_TRUNCATED"),
        "StatusLineTruncated が理由コードへ写されていない\n{}",
        locate(&src, "pub fn blocked_reason(")
    );
    // 実挙動: 24 桁の写しの申告を `…` で切った変種（折り返さず切る版の claude が来ても安全側）
    let cut = IDLE_24.replace("  6:16 PM · 1 shell\n  still running", "  6:16 PM · 1 sh…");
    let v = verdict(&cut);
    assert_eq!(v.overriding(), None, "読めない申告で覆している");
    assert_eq!(
        v.blocked_reason(),
        Some(wait::STATUS_LINE_TRUNCATED),
        "覆せなかった理由が `status_line_truncated` になっていない\n{}",
        locate(&src, "fn claude_status_line_truncated(")
    );
    // 生成中の狭い画面も状態行が切れているので理由は同じ（status は busy のまま）
    assert_eq!(
        verdict(BUSY_36).blocked_reason(),
        Some(wait::STATUS_LINE_TRUNCATED)
    );
}

#[test]
fn 修正前へ戻すab口がある() {
    let src = wait_src();
    let prod = &production_source(&src);
    assert!(
        fn_body(prod, "pub fn legacy_1721(").contains("TAKO_1721_LEGACY"),
        "A/B の env が読まれていない\n{}",
        locate(&src, "pub fn legacy_1721(")
    );
    let sig = "pub fn input_waiting_with_background_work_in(";
    let body = fn_body(prod, sig);
    assert!(
        body.contains("legacy_1721()") && body.contains("!legacy_wrap"),
        "判定が #1721 の A/B を通っていない（読み方と理由の両方を修正前へ戻せること）\n{}",
        locate(&src, sig)
    );
    assert_eq!(
        wait::background_work_summary_with(IDLE_36, Some(Agent::Claude), false, true),
        None,
        "legacy の読み方で狭いペインの申告が読めている = 修正前を再現できない"
    );
}

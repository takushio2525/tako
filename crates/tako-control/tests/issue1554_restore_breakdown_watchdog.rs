//! 復元の内訳が全ペインを説明することの番犬（Issue #1554）
//!
//! # 何が壊れていたか
//!
//! 「復元成功: N タブ / M ペイン（tmux 再 attach … / Claude resume … / 新規シェル … /
//! プレビュー …）」の内訳合計が M と合わない行が、実測で 25 件中 7 件（差 1〜2）あった。
//! 原因は復元ループの**無言の `continue`**で、Web ビューペイン・`spawn_session` の
//! 失敗・ワークスペース未配置・resume 入力の宛先なし の 4 経路がどのカウンタにも
//! 入っていなかった。失敗そのものも `eprintln!` 止まり（GUI の stderr はどこにも
//! 出ない）で persist.log に痕跡が無く、**無言で壊れる**状態だった。
//!
//! AGENTS.md が「再起動後にエージェントが戻らないとき」の正本として案内している
//! 診断がこの行なので、説明できないペインが 1 件でもあると調査がそこで止まる。
//!
//! # 検査は 4 本立て（1 本ずつでは穴が残る）
//!
//! 1. [`復元ループのすべての出口が結末を記録する`] — 数え忘れた `continue` を file:line で
//!    名指す。カテゴリを増やしても「記録しないで抜ける枝」は作れない
//!    （[`たまり場と退避タブは失敗として数えない`] — 「起こさないのが正しいペイン」を
//!    失敗の側へ寄せるのも禁じる。毎起動 `復元失敗 2` と出ると本当の失敗が埋もれる）
//! 2. [`内訳のラベルは一実装から出る`] — 呼び出し側が 1 行目を自前で組み直す形を禁じる
//!    （2 系統に分かれると「表示は増えたが数えていない」が戻ってくる）
//! 3. [`個別の失敗は数えると同時にpersistlogへ残す`] — 「数えるだけ」「出すだけ」の
//!    片落ちを禁じる
//! 4. [`合計とペイン数の食い違いを黙らせない`] — 将来また記録しない枝が増えても、
//!    実行時に 1 行残る形であることを固定する
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りして緑になるのを防ぐため、検出力（[`記録を落とすと名指しできる`]）と
//! 走査の健全性（区間に `continue` が実際に採れていること）を同じファイルで固定する。

use std::path::{Path, PathBuf};

use tako_control::restore_report::{
    FailureReason, HiddenKind, PaneOutcome, RestoreBreakdown, LABEL_FAILED, LABEL_HIDDEN,
};
use tako_core::agent_support::Agent;

const MAIN: &str = "crates/tako-app/src/main.rs";
const REPORT: &str = "crates/tako-control/src/restore_report.rs";

/// 復元ループの入口（内訳の器を作る行）
const REGION_START: &str =
    "let mut breakdown = tako_control::restore_report::RestoreBreakdown::new();";
/// 復元ループの直後（「1 つも起動できない」の判定）
const REGION_END: &str = "if app.terminals.is_empty()";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 復元ループの区間（1 始まりの行番号つき）。区間が採れなければ**落とす**
/// （走査が空振りしたまま緑になるのを防ぐ）
fn restore_loop(source: &str) -> Vec<(usize, &str)> {
    let numbered: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .collect();
    let start = numbered
        .iter()
        .position(|(_, l)| l.contains(REGION_START))
        .unwrap_or_else(|| {
            panic!("{MAIN} に復元ループの入口（`{REGION_START}`）が無い。#1554 の内訳が別実装へ移った？")
        });
    let end = numbered[start..]
        .iter()
        .position(|(_, l)| l.contains(REGION_END))
        .unwrap_or_else(|| panic!("{MAIN} に復元ループの出口（`{REGION_END}`）が無い"))
        + start;
    numbered[start..end].to_vec()
}

/// 結末を記録せずに抜ける `continue`（file:line 名指し）。
///
/// `continue` の手前 10 行以内に記録の呼び出しがあれば良しとする（記録 → `continue` は
/// 同じ枝の中で隣り合うのが唯一の書き方で、`record_restore_failure` の引数が複数行に
/// 折れても収まる幅）
fn unrecorded_continues(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    let mut bad = Vec::new();
    for (i, (line_no, line)) in region.iter().enumerate() {
        if line.trim() != "continue;" {
            continue;
        }
        let back = i.saturating_sub(10);
        let recorded = region[back..i]
            .iter()
            .any(|(_, l)| l.contains("breakdown.record(") || l.contains("record_restore_failure("));
        if !recorded {
            bad.push(format!("{MAIN}:{line_no}: {}", line.trim()));
        }
    }
    bad
}

/// 1: 数え忘れた `continue` が無い
#[test]
fn 復元ループのすべての出口が結末を記録する() {
    let source = read(MAIN);
    let region = restore_loop(&source);
    let exits = region
        .iter()
        .filter(|(_, l)| l.trim() == "continue;")
        .count();
    assert!(
        exits >= 4,
        "復元ループに `continue` が {exits} 件しか見えない（走査が空振りしている）。\n\
         #1554 の時点で 4 経路（タブ配下に居ない / プレビュー / Web ビュー / spawn 失敗）ある"
    );
    let bad = unrecorded_continues(&source);
    assert!(
        bad.is_empty(),
        "復元ループが結末を記録せずに抜けている（#1554 の再発）。\n\
         内訳の合計がペイン数より小さくなり、persist.log の「復元成功」が全ペインを\n\
         説明しなくなる。`breakdown.record(..)` か `record_restore_failure(..)` を\n\
         通してから `continue` すること:\n  {}",
        bad.join("\n  ")
    );
}

/// 1': 記録を 1 つ落とすと名指しできる（検出力）
#[test]
fn 記録を落とすと名指しできる() {
    let source = read(MAIN);
    assert!(unrecorded_continues(&source).is_empty(), "現行は緑のはず");
    // 行を空行へ置き換える（行番号を動かさずに記録だけ落とす）
    let broken = source.replace(
        "breakdown.record(tako_control::restore_report::PaneOutcome::Webview);",
        "",
    );
    assert_ne!(
        broken, source,
        "Web ビューの記録が見つからない（実装が変わった？）"
    );
    let bad = unrecorded_continues(&broken);
    assert_eq!(
        bad.len(),
        1,
        "Web ビューの記録を落としても名指しされない = 番犬が効いていない: {bad:?}"
    );
    assert!(
        bad[0].starts_with(&format!("{MAIN}:")),
        "file:line で名指すこと: {bad:?}"
    );
}

/// 2: 1 行目のラベルは `restore_report` の 1 実装からだけ出る
#[test]
fn 内訳のラベルは一実装から出る() {
    let report = read(REPORT);
    assert!(
        report.contains("tmux 再 attach") && report.contains("プレビュー"),
        "{REPORT} が内訳のラベルを持っていない（正本が移った？）"
    );
    let main = read(MAIN);
    assert!(
        !main.contains("tmux 再 attach"),
        "{MAIN} が 1 行目を自前で組み直している（#1554 の再発）。\n\
         ラベルと件数の正本は `restore_report::RestoreBreakdown::segments` の 1 本で、\n\
         2 系統に分かれると「表示は増えたが数えていない」が戻ってくる"
    );
    // 合計は区間の総和から作る（カテゴリを落とすと合計も落ちる形を固定する）
    let total = report
        .split("pub fn total(")
        .nth(1)
        .expect("`total` が無い")
        .split("}\n")
        .next()
        .unwrap_or_default();
    assert!(
        total.contains("self.segments()"),
        "`total` が `segments` 以外から合計を作っている（#1554）。\n\
         別々に数えると「1 行目には出るが合計に入らない」カテゴリが作れてしまう"
    );
}

/// 3: 個別の失敗は「数える」と「残す」を同時に行う
#[test]
fn 個別の失敗は数えると同時にpersistlogへ残す() {
    let main = read(MAIN);
    let helper = main
        .split("fn record_restore_failure(")
        .nth(1)
        .expect("`record_restore_failure` が無い（#1554 の失敗記録が消えた）")
        .split("\n}\n")
        .next()
        .unwrap_or_default();
    assert!(
        helper.contains("breakdown.record("),
        "失敗を内訳へ数えていない（1 行目が全ペインを説明しなくなる）"
    );
    assert!(
        helper.contains("persist_diag(") && helper.contains("failure_line("),
        "失敗の 1 行を persist.log へ残していない（GUI の stderr は誰も読めない = #1554 の症状）"
    );
    // 失敗の記録は必ずこの 1 実装を通る（`record(Failed(..))` を直に書かせない）
    let region = restore_loop(&main);
    let direct: Vec<String> = region
        .iter()
        .filter(|(_, l)| l.contains("PaneOutcome::Failed("))
        .map(|(n, l)| format!("{MAIN}:{n}: {}", l.trim()))
        .collect();
    assert!(
        direct.is_empty(),
        "復元ループが失敗を直に数えている（persist.log への 1 行が落ちる）。\n\
         `record_restore_failure` を通すこと:\n  {}",
        direct.join("\n  ")
    );
}

/// 4: 合計とペイン数の食い違いを実行時に名指す
#[test]
fn 合計とペイン数の食い違いを黙らせない() {
    let main = read(MAIN);
    assert!(
        main.contains("breakdown.mismatch(restored.len())"),
        "内訳の合計と復元ペイン数を突き合わせていない（#1554）。\n\
         記録しない枝が将来増えたときに、また無言で壊れる"
    );
    let call = main
        .split("breakdown.mismatch(restored.len())")
        .nth(1)
        .unwrap_or_default();
    assert!(
        call[..call.len().min(200)].contains("persist_diag("),
        "食い違いを persist.log へ残していない（読めない診断は診断ではない）"
    );
}

/// 5: 診断へペイン内容を載せない（conventions の診断ログ規約）
#[test]
fn 診断へペイン内容を載せない() {
    let report = read(REPORT);
    for banned in ["capture_pane", "scrollback", "TAKO_TOKEN", "read_pane"] {
        assert!(
            !report.contains(banned),
            "{REPORT} が診断へ `{banned}` を持ち込んでいる（ペイン内容・トークンは出さない）"
        );
    }
    // 理由は分類だけ（差し込み口を持たない静的なラベル）
    for reason in FailureReason::ALL {
        let label = reason.label();
        assert!(
            !label.is_empty() && !label.contains('{'),
            "理由の分類が壊れている: {label}"
        );
    }
}

/// 6: 不変条件そのもの（内訳の合計 == 記録したペイン数）を境界越しでも確かめる
#[test]
fn 内訳の合計は記録したペイン数と一致する() {
    let mut b = RestoreBreakdown::with_legacy(false);
    let outcomes = [
        PaneOutcome::Reattached,
        PaneOutcome::Resumed {
            agent: Agent::Claude,
            with_role: true,
        },
        PaneOutcome::Resumed {
            agent: Agent::Agy,
            with_role: false,
        },
        PaneOutcome::Preview,
        PaneOutcome::Webview,
        PaneOutcome::Hidden(HiddenKind::Backgrounded),
        PaneOutcome::Hidden(HiddenKind::ShelvedTab),
        PaneOutcome::Failed(FailureReason::SpawnFailed),
        PaneOutcome::Failed(FailureReason::NotPlaced),
        PaneOutcome::Failed(FailureReason::ResumeNotDelivered),
    ];
    for o in outcomes {
        b.record(o);
    }
    assert_eq!(b.total(), outcomes.len());
    assert_eq!(b.mismatch(outcomes.len()), None);
    let line = b.summary(4, outcomes.len());
    assert!(
        line.contains("Web ビュー 1") && line.contains("復元失敗 3"),
        "{line}"
    );
    assert!(line.contains("たまり場・退避 2"), "{line}");
    assert!(line.contains("agy resume 1"), "{line}");
}

/// 1': たまり場（FR-2.15.5）と退避タブ配下（#1487）は**失敗ではない**
#[test]
fn たまり場と退避タブは失敗として数えない() {
    // 器の側: 別カテゴリとして数え、`failed()` を増やさない
    let mut b = RestoreBreakdown::with_legacy(false);
    for kind in HiddenKind::ALL {
        b.record(PaneOutcome::Hidden(kind));
    }
    assert_eq!(b.hidden(), 2);
    assert_eq!(
        b.failed(),
        0,
        "起こさないのが正しいペインを失敗として数えている（#1554）。\n\
         毎起動 `{LABEL_FAILED} N` が出ると、本当の復元失敗が埋もれる"
    );
    assert!(b.summary(1, 2).contains(&format!("{LABEL_HIDDEN} 2")));
    // 呼び出し側: 2 つの集合を引いてから失敗へ落とす形であること
    let main = read(MAIN);
    let region: String = restore_loop(&main)
        .iter()
        .map(|(_, l)| *l)
        .collect::<Vec<_>>()
        .join("\n");
    for needed in [
        "shelved_panes()",
        "shelved_tabs()",
        "HiddenKind::Backgrounded",
        "HiddenKind::ShelvedTab",
    ] {
        assert!(
            region.contains(needed),
            "復元ループが `{needed}` を見ていない（#1554）。\n\
             タブ配下に居ないペインを一律 `FailureReason::NotPlaced` へ落とすと、\n\
             たまり場・退避タブを使っているだけで毎起動「復元失敗」が出る"
        );
    }
}

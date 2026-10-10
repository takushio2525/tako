//! 退避（たまり場・退避タブ）のペインが復元で退避のまま同じ器へ戻ることの番犬（Issue #1576）
//!
//! # 何が壊れていたか
//!
//! 復元ループは「タブ配下に居ないペイン」を `let-else` で抜けていて、器（tmux セッション）を
//! `backend_sessions` へ登録しなかった。起動時の orphan 自動復帰（#191）は
//! `backend_sessions` を protected 集合にするので、退避の器は「復帰」タブへ**別 pane id** で
//! 拾われ（role・タイトル・limit_resume・worker レジストリの紐付けが切れる）、元の退避
//! エントリは端末の無い**幽霊**として残った。人が「復帰」タブを閉じると器ごと kill され、
//! 退避しておいた会話が消える（2026-09-23 に worker 4 本・2026-10-09 に 6 本）。
//!
//! # 検査は 5 本立て
//!
//! 1. [`退避のペインは判断の一実装を通る`] — 復元ループが `shelved_restore::plan` を呼ぶ
//! 2. [`起こすと決めた退避は表と同じ枝へ流れる`] — `Wake` の腕が `continue` で抜けない
//! 3. [`退避を起こさずに数えるのは残す枝だけ`] — `PaneOutcome::Hidden(..)` を直に記録して
//!    抜けるのは `Leave` の腕だけ（#1576 以前の形 = 退避を素通りさせて数えるだけ、を禁じる）
//! 4. [`器の登録は表と退避の共通の枝にある`] — `backend_sessions` への登録が退避の判定より後
//!    （= 退避も通る枝）にあり、orphan 自動復帰より前に終わる
//! 5. [`起こした退避の結末は数え替える`] — 表の結末を `.within(hidden)` を通して記録する
//!
//! # 見逃す側へ倒れないための作り
//!
//! 各検査は**修正を外した形**を注入して file:line で名指しできることを同じファイルで
//! 固定する（[`修正を外すと名指しできる`]）。走査の区間が採れなければ落とす。

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const PLAN: &str = "crates/tako-control/src/shelved_restore.rs";

/// 復元ループの入口（内訳の器を作る行。#1554 の番犬と同じ目印）
const REGION_START: &str =
    "let mut breakdown = tako_control::restore_report::RestoreBreakdown::new();";
/// 復元ループの直後（「1 つも起動できない」の判定）
const REGION_END: &str = "if app.terminals.is_empty()";

#[path = "common/code_view.rs"]
mod code_view;

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

/// コメントを落とした本文（説明文に同じ綴りを書いても真にならない。#1609）。行番号は保たれる
fn read_code(rel: &str) -> String {
    code_view::without_comments_checked(&read(rel), rel)
}

/// 復元ループの区間（1 始まりの行番号つき）。採れなければ落とす
fn restore_loop(source: &str) -> Vec<(usize, &str)> {
    let numbered: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .collect();
    let start = numbered
        .iter()
        .position(|(_, l)| l.contains(REGION_START))
        .unwrap_or_else(|| panic!("{MAIN} に復元ループの入口（`{REGION_START}`）が無い"));
    let end = numbered[start..]
        .iter()
        .position(|(_, l)| l.contains(REGION_END))
        .unwrap_or_else(|| panic!("{MAIN} に復元ループの出口（`{REGION_END}`）が無い"))
        + start;
    numbered[start..end].to_vec()
}

fn at(line_no: usize, line: &str) -> String {
    format!("{MAIN}:{line_no}: {}", line.trim())
}

/// 退避の種別を判定している行（名指しの起点）
fn hidden_detection(region: &[(usize, &str)]) -> Option<(usize, String)> {
    region
        .iter()
        .find(|(_, l)| l.contains("HiddenKind::Backgrounded"))
        .map(|(n, l)| (*n, l.to_string()))
}

/// 1: 復元ループが退避の扱いを `shelved_restore::plan` に聞いていない行
fn missing_plan(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    if region
        .iter()
        .any(|(_, l)| l.contains("shelved_restore::plan("))
    {
        return Vec::new();
    }
    let (n, l) = hidden_detection(&region)
        .unwrap_or_else(|| panic!("{MAIN} の復元ループが退避（たまり場）を判定していない"));
    vec![at(n, &l)]
}

/// `HiddenPlan::<腕>` の腕の本文（次の `HiddenPlan::` か区間の終わりまで）
fn arm<'a>(region: &'a [(usize, &'a str)], name: &str) -> Option<&'a [(usize, &'a str)]> {
    let start = region
        .iter()
        .position(|(_, l)| l.contains(&format!("HiddenPlan::{name}")))?;
    let len = region[start + 1..]
        .iter()
        .position(|(_, l)| l.contains("HiddenPlan::"))
        .unwrap_or(region.len() - start - 1);
    Some(&region[start..start + 1 + len])
}

/// 2: `Wake` の腕が `continue` で抜けている行（起こすと決めた退避を起こさない）
fn wake_escapes(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    let Some(body) = arm(&region, "Wake") else {
        return missing_plan(source)
            .into_iter()
            .chain(std::iter::once(format!(
                "{MAIN}: 復元ループに `HiddenPlan::Wake` の腕が無い"
            )))
            .collect();
    };
    let mut bad: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.trim() == "continue;")
        .map(|(n, l)| at(*n, l))
        .collect();
    if !body.iter().any(|(_, l)| l.contains("Some(kind)")) {
        let (n, l) = body[0];
        bad.push(at(n, l));
    }
    bad
}

/// 3: `PaneOutcome::Hidden(` を直に記録している行のうち、`Leave` の腕の外にあるもの
fn hidden_recorded_outside_leave(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    let mut bad = Vec::new();
    for (i, (n, l)) in region.iter().enumerate() {
        if !l.contains("PaneOutcome::Hidden(") {
            continue;
        }
        let in_leave = region[..i]
            .iter()
            .rev()
            .find(|(_, prev)| prev.contains("HiddenPlan::"))
            .is_some_and(|(_, prev)| prev.contains("HiddenPlan::Leave"));
        if !in_leave {
            bad.push(at(*n, l));
        }
    }
    bad
}

/// 4: 器の登録が退避の判定より前にしか無い / 無い
fn registration_misplaced(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    let resolved = region
        .iter()
        .position(|(_, l)| l.contains("let (pane, hidden) ="));
    let registered = region
        .iter()
        .position(|(_, l)| l.contains("app.backend_sessions.insert(pane,"));
    match (resolved, registered) {
        (Some(r), Some(g)) if g > r => Vec::new(),
        (_, Some(g)) => vec![at(region[g].0, region[g].1)],
        (Some(r), None) => vec![at(region[r].0, region[r].1)],
        (None, None) => {
            let (n, l) = hidden_detection(&region).unwrap_or((region[0].0, String::new()));
            vec![at(n, &l)]
        }
    }
}

/// 5: 表の結末を `.within(hidden)` を通さずに記録している行
fn unwithin_outcomes(source: &str) -> Vec<String> {
    let region = restore_loop(source);
    let mut bad = Vec::new();
    for (i, (n, l)) in region.iter().enumerate() {
        let woken = [
            "PaneOutcome::Reattached",
            "PaneOutcome::Resumed",
            "PaneOutcome::FreshShell(",
            "PaneOutcome::Preview",
        ]
        .iter()
        .any(|k| l.contains(k));
        if !woken {
            continue;
        }
        // 同じ記録の文（`;` か `)` で閉じるまで。複数行に折れても 6 行で収まる）
        let stmt: String = region[i..region.len().min(i + 6)]
            .iter()
            .map(|(_, s)| *s)
            .collect::<Vec<_>>()
            .join("\n");
        let stmt = stmt.split(';').next().unwrap_or_default();
        if !stmt.contains(".within(hidden)") {
            bad.push(at(*n, l));
        }
    }
    bad
}

#[test]
fn 退避のペインは判断の一実装を通る() {
    let source = read_code(MAIN);
    let bad = missing_plan(&source);
    assert!(
        bad.is_empty(),
        "復元ループが退避のペインを `tako_control::shelved_restore::plan` に聞かずに扱っている\n\
         （#1576 の再発。退避の器が `backend_sessions` に入らず、orphan 自動復帰が\n\
         「復帰」タブへ別 pane として拾う）:\n  {}",
        bad.join("\n  ")
    );
    // A/B の口は判断の 1 実装にだけある（呼び出し側で env を読み直さない）
    let plan = read_code(PLAN);
    assert!(
        plan.contains("TAKO_1576_LEGACY"),
        "{PLAN} が A/B（TAKO_1576_LEGACY）を持っていない"
    );
    assert!(
        !source.contains("TAKO_1576_LEGACY"),
        "{MAIN} が A/B の env を直に読んでいる（判断を 2 系統にしない）"
    );
}

#[test]
fn 起こすと決めた退避は表と同じ枝へ流れる() {
    let source = read_code(MAIN);
    let bad = wake_escapes(&source);
    assert!(
        bad.is_empty(),
        "`HiddenPlan::Wake` の腕が表と同じ枝へ流れていない（起こすと決めた退避を起こさない =\n\
         #1576 の幽霊が戻る）。腕は `(pane, Some(kind))` を返して抜けずに続けること:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn 退避を起こさずに数えるのは残す枝だけ() {
    let source = read_code(MAIN);
    let bad = hidden_recorded_outside_leave(&source);
    assert!(
        bad.is_empty(),
        "退避のペインを起こさずに「たまり場・退避」と数えて抜けている（#1576 以前の形）。\n\
         起こさないのは `HiddenPlan::Leave`（A/B と Web ビュー）だけ:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn 器の登録は表と退避の共通の枝にある() {
    let source = read_code(MAIN);
    let bad = registration_misplaced(&source);
    assert!(
        bad.is_empty(),
        "`backend_sessions` への登録が退避の判定（`let (pane, hidden) =`）より後の\n\
         共通の枝に無い（退避の器が protected から漏れて orphan 自動復帰に拾われる）:\n  {}",
        bad.join("\n  ")
    );
    // orphan 自動復帰は復元ループの後で、protected を `backend_sessions` から作る
    let loop_end = source.find(REGION_END).expect("復元ループの出口");
    let recover_call = source
        .find("app.recover_orphan_sessions(cx)")
        .expect("orphan 自動復帰の呼び出しが無い");
    assert!(
        recover_call > loop_end,
        "orphan 自動復帰が復元ループより前に走っている（退避の器がまだ登録されていない）"
    );
    let body = source
        .split("fn recover_orphan_sessions(")
        .nth(1)
        .expect("`recover_orphan_sessions` が無い");
    let head: String = body.chars().take(400).collect();
    assert!(
        head.contains("self.backend_sessions.values()"),
        "orphan 自動復帰の protected が `backend_sessions` から作られていない（#1576 の修正は\n\
         退避の器をそこへ登録することで守っている）"
    );
}

#[test]
fn 起こした退避の結末は数え替える() {
    let source = read_code(MAIN);
    let bad = unwithin_outcomes(&source);
    assert!(
        bad.is_empty(),
        "表の結末を `.within(hidden)` を通さずに記録している（退避のペインが表の件数に混ざり、\n\
         「たまり場・退避」が実際に退避のまま戻した数を言わなくなる）:\n  {}",
        bad.join("\n  ")
    );
}

/// 検出力: 修正を外した形を注入すると、それぞれの検査が file:line で名指す
#[test]
fn 修正を外すと名指しできる() {
    let source = read_code(MAIN);
    for (name, bad) in [
        ("plan", missing_plan(&source)),
        ("wake", wake_escapes(&source)),
        ("leave", hidden_recorded_outside_leave(&source)),
        ("registration", registration_misplaced(&source)),
        ("within", unwithin_outcomes(&source)),
    ] {
        assert!(bad.is_empty(), "現行は緑のはず（{name}）: {bad:?}");
    }
    let named = |bad: &[String]| bad.iter().all(|b| b.starts_with(&format!("{MAIN}:")));

    // ① 判断を呼ばない（#1576 以前は素通り）
    let broken = source.replace("shelved_restore::plan(", "shelved_restore::unused(");
    assert_ne!(broken, source);
    let bad = missing_plan(&broken);
    assert_eq!(bad.len(), 1, "判断を外しても名指されない: {bad:?}");
    assert!(named(&bad), "{bad:?}");

    // ② 起こすと決めた退避を抜けさせる（行数を変えずに置き換える）
    let broken = source.replacen("(PaneId::from_raw(r.pane), Some(kind))", "continue;", 1);
    assert_ne!(broken, source);
    let bad = wake_escapes(&broken);
    assert!(
        !bad.is_empty() && named(&bad),
        "Wake の腕の抜けが名指されない: {bad:?}"
    );

    // ③ 起こさずに数えて抜ける形（#1576 以前の記録）を残す枝の外へ置く
    let broken = source.replacen(
        "tako_control::shelved_restore::HiddenPlan::Leave =>",
        "tako_control::shelved_restore::HiddenPlan::Other =>",
        1,
    );
    assert_ne!(broken, source);
    let bad = hidden_recorded_outside_leave(&broken);
    assert_eq!(bad.len(), 1, "残す枝の外の記録が名指されない: {bad:?}");
    assert!(named(&bad), "{bad:?}");

    // ④ 器の登録を落とす
    let broken = source.replace(
        "app.backend_sessions.insert(pane,",
        "app.backend_names.insert(pane,",
    );
    assert_ne!(broken, source);
    let bad = registration_misplaced(&broken);
    assert_eq!(bad.len(), 1, "器の登録を落としても名指されない: {bad:?}");
    assert!(named(&bad), "{bad:?}");

    // ⑤ 数え替えを落とす
    let broken = source.replacen(
        "PaneOutcome::Reattached.within(hidden)",
        "PaneOutcome::Reattached",
        1,
    );
    assert_ne!(broken, source);
    let bad = unwithin_outcomes(&broken);
    assert_eq!(bad.len(), 1, "数え替えを落としても名指されない: {bad:?}");
    assert!(named(&bad), "{bad:?}");
}

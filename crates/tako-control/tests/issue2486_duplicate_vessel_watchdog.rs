//! 復元で表のペインどうしが同じ器の名前を持つときの番犬（Issue #2486）
//!
//! # 何が壊れていたか
//!
//! `layout.json` で 2 つの表のペインが同じ器の名前を持つと、復元ループは
//! `layout.json` の名前（`r.session`）をそのまま両方のペインの器として登録し、両方を
//! 同じ器へ繋いでいた。器のクライアントは `new-session -A -D` なので 2 本目が 1 本目を
//! 切り離し、1 本目は #1857 の判定（別のクライアントが attach 中）で**ペインごと閉じた**。
//! 3 本なら 2 本が閉じる。
//!
//! # 検査
//!
//! 1. [`復元ループは器の名前をlayoutから直接使わない`] — 区間の中で `r.session` を読む行を
//!    file:line で名指す。器の名前は `restore_vessels::TabVessels::claim` の結果
//!    （`vessel()`）から取る（重複した 2 本目は `None` = spawn が新しい器を払い出す）
//! 2. [`器の登録はclaimした名前だけ`] — `backend_sessions.insert(` が claim の名前の枝の中に在る
//! 3. [`持ち主の表は表のペインから作る`] — 判断の正本（`TabVessels::new`）を通っている
//! 4. [`重複した二本目は器の名前を使わない`] — 判断そのもの（公開 API）を固定する
//!
//! 走査が空振りして緑になるのを防ぐため、検出力（[`layoutの名前へ戻すと名指しできる`]）を
//! 同じファイルで固定する。区間の取り方は #1554 の番犬と同じ（同じ入口と出口）。

use std::path::{Path, PathBuf};

use tako_control::layout::RestoredPane;
use tako_control::restore_vessels::{TabVessel, TabVessels};

const MAIN: &str = "crates/tako-app/src/main.rs";

/// 復元ループの入口（内訳の器を作る行。#1554 の番犬と同じ）
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

#[path = "common/code_view.rs"]
mod code_view;

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 区間（1 始まりの行番号つき）。区間が採れなければ**落とす**（空振りのまま緑にしない）
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

/// 区間の中で layout の器の名前（`r.session`）を直接読む行（file:line 名指し）。
/// `code` はコメントと文字列を潰した眺め（説明文の綴りを拾わない）
fn direct_session_reads(code: &str) -> Vec<String> {
    restore_loop(code)
        .into_iter()
        .filter(|(_, l)| l.contains("r.session"))
        .map(|(n, l)| format!("{MAIN}:{n}: {}", l.trim()))
        .collect()
}

/// 1: 器の名前は claim の結果から取る
#[test]
fn 復元ループは器の名前をlayoutから直接使わない() {
    let code = code_view::code_view(&read(MAIN));
    let region = restore_loop(&code);
    assert!(
        region.iter().any(|(_, l)| l.contains("tab_vessels.claim(")),
        "{MAIN} の復元ループが `tab_vessels.claim(` を呼んでいない（#2486 の判断が外れた / 走査が空振り）"
    );
    let bad = direct_session_reads(&code);
    assert!(
        bad.is_empty(),
        "復元ループが layout の器の名前を直接使っている（#2486 の再発）。\n\
         2 つのペインが同じ名前を持つと両方が同じ器へ繋がり、器のクライアントの `-D` が\n\
         先のペインを切り離してペインごと閉じる。名前は `TabVessels::claim(..).vessel()` から取る:\n{}",
        bad.join("\n")
    );
}

/// 2: `backend_sessions` へ登録するのは claim した名前だけ
#[test]
fn 器の登録はclaimした名前だけ() {
    let code = code_view::code_view(&read(MAIN));
    let region = restore_loop(&code);
    let inserts: Vec<usize> = region
        .iter()
        .enumerate()
        .filter(|(_, (_, l))| l.contains("backend_sessions.insert("))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !inserts.is_empty(),
        "{MAIN} の復元ループに `backend_sessions.insert(` が見えない（走査が空振りしている）"
    );
    let bad: Vec<String> = inserts
        .into_iter()
        .filter(|&i| {
            let back = i.saturating_sub(3);
            !region[back..i]
                .iter()
                .any(|(_, l)| l.contains("= vessel {"))
        })
        .map(|i| format!("{MAIN}:{}: {}", region[i].0, region[i].1.trim()))
        .collect();
    assert!(
        bad.is_empty(),
        "復元ループが claim を通らない名前を器として登録している（#2486）。\n\
         `if let Some(name) = vessel {{ … }}` の枝の中で登録する:\n{}",
        bad.join("\n")
    );
}

/// 3: 持ち主の表は判断の正本から作る（表のペインの順 = 復元の順）
#[test]
fn 持ち主の表は表のペインから作る() {
    let code = code_view::without_comments_checked(&read(MAIN), MAIN);
    let region: String = restore_loop(&code)
        .into_iter()
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        region.contains("restore_vessels::TabVessels::new(&restored"),
        "{MAIN} の復元ループが `restore_vessels::TabVessels::new(&restored, …)` で持ち主を決めていない（#2486）"
    );
    assert!(
        region.contains("restore_vessels::legacy()"),
        "{MAIN} の復元ループが A/B（`TAKO_2486_LEGACY=1`）を見ていない"
    );
    assert!(
        region.contains("restore_vessels::log_line("),
        "{MAIN} の復元ループが重複の理由を persist.log へ残していない（ペインを黙って変えない = #2486）"
    );
}

fn pane(id: u64, session: Option<&str>) -> RestoredPane {
    RestoredPane {
        pane: id,
        session: session.map(str::to_string),
        cwd: None,
        claude_session_id: None,
        agent_resume: None,
        logged_history: None,
        preview: None,
        webview: None,
        ssh: None,
    }
}

/// 4: 判断そのもの。同じ名前の 2 本目・3 本目は器の名前を使わず、先頭のペインを名指す
#[test]
fn 重複した二本目は器の名前を使わない() {
    let restored = vec![
        pane(1, Some("tako-a")),
        pane(2, Some("tako-a")),
        pane(3, Some("tako-b")),
        pane(4, Some("tako-a")),
    ];
    let vessels = TabVessels::new(&restored, |_| true);
    let claims: Vec<TabVessel<'_>> = restored
        .iter()
        .map(|r| vessels.claim(r, true, |_| true))
        .collect();
    let names: Vec<Option<&str>> = claims.iter().map(TabVessel::vessel).collect();
    assert_eq!(
        names,
        vec![Some("tako-a"), None, Some("tako-b"), None],
        "同じ器へ繋ぐペインが 2 本以上ある = 先のペインが閉じる（#2486）"
    );
    let holders: Vec<Option<u64>> = claims
        .iter()
        .map(|c| c.duplicate().map(|d| d.holder))
        .collect();
    assert_eq!(holders, vec![None, Some(1), None, Some(1)]);
}

/// 検出力: 復元ループを layout の名前の直読みへ戻すと file:line で名指す
#[test]
fn layoutの名前へ戻すと名指しできる() {
    let code = code_view::code_view(&read(MAIN));
    assert!(direct_session_reads(&code).is_empty(), "現行は緑のはず");
    // claim の名前を layout の名前へ差し戻す（行番号は動かさない）
    let broken = code.replacen(
        "let vessel = claim.vessel();",
        "let vessel = r.session.as_deref();",
        1,
    );
    assert_ne!(
        broken, code,
        "`let vessel = claim.vessel();` が見つからない（実装が変わった？）"
    );
    let bad = direct_session_reads(&broken);
    assert_eq!(
        bad.len(),
        1,
        "直読みへ戻しても名指しされない = 番犬が効いていない: {bad:?}"
    );
    assert!(
        bad[0].starts_with(&format!("{MAIN}:")),
        "file:line で名指すこと: {bad:?}"
    );
}

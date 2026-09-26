//! 番犬: 実行ペインを積まない / 内部マーカーを画面へ戻さない / バッジを外さない（#1657）
//!
//! ## 何を止めたいのか
//!
//! #1657 以前の実行ペインは、再生ボタンを押すたびに新しいペインを割り（5 回で 5 枚）、
//! 画面の最後に内部の契約 `__TAKO_EXIT=0` をそのまま出し、「Enter で閉じる」の案内も
//! 完了 / 失敗のバッジも無かった。直したあとに戻る形は次の 5 つ:
//!
//! 1. 終了コードを読む側が 2 つに割れる（画面を直接読む口が `run_pane_exit_code` の外に
//!    生える = 側路で確定した値とずれる。#1724 のカード実行記録もここを通す）
//! 2. `Run` の腕が再利用の鍵で探さなくなる / 見つけたペインを `spawn_command_pane` へ渡さない
//! 3. 実行ペインのスクリプトがマーカー行を**退避路の外**で出す（POSIX の `||` の右 /
//!    PowerShell の `catch` の中だけに居るはず）
//! 4. タイトルバーのバッジが描かれなくなる（`render_pane_header` の 3 状態の文言・id）
//! 5. GUI が終わった瞬間の確定をやめる（出力のたびの `settle_run_pane` と 2 秒ごとの
//!    `settle_run_panes`）
//!
//! **落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! - 5 回走らせて 1 枚のまま / `new_pane` / 鍵違い / 閉じたあと:
//!   `dispatch::tests::issue1657_*`
//! - 実 `/bin/sh` で画面にマーカーが出ず側路へ書く:
//!   `platform::shell::tests_1657::posixの実行ペインは画面にマーカーを出さず側路へ書く`
//! - カードの実行記録が実 PTY で exited へ確定する:
//!   `dispatch::tests::issue1657_カードの実行記録は画面にマーカーが無くても確定する`
//! - 隔離 GUI の実経路（ペイン数・画面の文言・`list` の `run`）: `scripts/test-run-pane-reuse-1657.sh`
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `refresh_command_card_runs` の `run_pane_exit_code(host, pane)` を画面の
//!    `find_exit_marker(&rows)` へ戻す /
//! ② `Run` の腕の `spawn_command_pane(… reuse,)` の `reuse` を `None` へ /
//! ③ `posix_run_pane_script` の `report` を `marker_line` 決め打ちへ（常に画面へ出す）/
//! ④ `render_pane_header` の `.when(hv.run_badge, …)` の塊を消す

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const SHELL: &str = "crates/tako-core/src/platform/shell.rs";
const APP: &str = "crates/tako-app/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*')
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで。コメント行は除く）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が無い"));
    let body: Vec<(usize, String)> = (start..=end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

/// `from` を含む行から `to` を含む行の手前まで（コメント行は除く）
fn region(rel: &str, src: &str, from: &str, to: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(from))
        .unwrap_or_else(|| panic!("{rel}: 目印 {from:?} が消えている"));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.contains(to))
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}:{}: {from:?} の終わり {to:?} が無い", start + 1));
    let body = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    (body, start + 1)
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------- 1. 読む側は 1 実装

/// 画面のマーカーを読む `find_exit_marker(` を呼んでよいのは `run_pane_exit_code` の中だけ
/// （側路 → 画面の順を 1 か所に閉じる）。違反は呼び出し行を名指す
fn marker_readers_outside(src: &str) -> Vec<String> {
    let (allowed, _) = fn_body(DISPATCH, src, "pub fn run_pane_exit_code(");
    let allowed: Vec<usize> = allowed.iter().map(|(n, _)| *n).collect();
    src.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .filter(|(_, l)| l.contains("find_exit_marker(") && !l.contains("fn find_exit_marker"))
        .filter(|(i, _)| !allowed.contains(&(i + 1)))
        .map(|(i, l)| format!("{DISPATCH}:{}: {}", i + 1, l.trim()))
        .collect()
}

#[test]
fn 終了コードを読む口はrun_pane_exit_codeの1つだけ() {
    let src = production(DISPATCH);
    let outside = marker_readers_outside(&src);
    assert!(
        outside.is_empty(),
        "画面のマーカーを `run_pane_exit_code` の外で読んでいる（側路で確定した終了コードと\
         ずれる。#1657。カードの実行記録・RunInteractiveStatus・list は run_pane_exit_code を通す）:\n{}",
        outside.join("\n")
    );
    // 読む側の本体が「側路 → 画面」の順を持っている
    let (body, at) = fn_body(DISPATCH, &src, "pub fn run_pane_exit_code(");
    let text = joined(&body);
    let file_at = text.find("run_pane::read").unwrap_or(usize::MAX);
    let screen_at = text.find("find_exit_marker(").unwrap_or(0);
    assert!(
        file_at < screen_at,
        "{DISPATCH}:{at}: run_pane_exit_code が側路ファイルを先に見ていない（#1657）"
    );
    // 使う側がこの 1 実装を呼んでいる
    for (sig, needle) in [
        (
            "pub fn refresh_command_card_runs(",
            "run_pane_exit_code(host, pane)",
        ),
        ("pub fn settle_run_pane(", "run_pane_exit_code(host, pane)"),
    ] {
        let (body, at) = fn_body(DISPATCH, &src, sig);
        assert!(
            joined(&body).contains(needle),
            "{DISPATCH}:{at}: {sig:?} が {needle:?} を通っていない（#1657 / #1724）"
        );
    }
}

#[test]
fn i1657_注入_画面を直接読む口を名指しで落とせる() {
    // 注入 ①: カードの実行記録を画面の直読みへ戻した形
    let injected =
        "pub fn run_pane_exit_code(host: &dyn ControlHost, pane: PaneId) -> Option<i32> {\n\
                    \x20   let x = tako_core::run_pane::read(p);\n\
                    \x20   find_exit_marker(&rows)\n\
                    }\n\
                    pub fn refresh_command_card_runs(host: &mut dyn ControlHost) -> bool {\n\
                    \x20   find_exit_marker(&rows)\n\
                    }\n";
    let got = marker_readers_outside(injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":6:"), "行番号が合っていない: {got:?}");
}

// --------------------------------------------- 2. 再利用

#[test]
fn runの腕は再利用の鍵で探して差し替えへ渡す() {
    let src = production(DISPATCH);
    // Run の腕はこの見出しの直後から RunResolve の腕の手前まで（見出しはコメントなので
    // 範囲の目印にだけ使い、本体からは落ちる）
    let (arm, at) = region(
        DISPATCH,
        &src,
        "// --- Code Runner (FR-3.18, #453) ---",
        "        Request::RunResolve {",
    );
    let text = joined(&arm);
    assert!(
        text.contains("find_run_pane(host.workspace(), tab_id, &key)"),
        "{DISPATCH}:{at}: Run の腕が同じファイル・プロファイルの実行ペインを探していない（#1657）"
    );
    // `spawn_command_pane(` の呼び出しの最後の引数が `reuse`
    let call_at = arm
        .iter()
        .position(|(_, l)| l.contains("spawn_command_pane("))
        .unwrap_or_else(|| panic!("{DISPATCH}:{at}: Run の腕に spawn_command_pane が無い"));
    let close = arm[call_at..]
        .iter()
        .position(|(_, l)| l.trim() == ")?;")
        .map(|i| call_at + i)
        .unwrap_or_else(|| panic!("{DISPATCH}:{}: 呼び出しが閉じていない", arm[call_at].0));
    let (last_no, last) = &arm[close - 1];
    assert_eq!(
        last.trim(),
        "reuse,",
        "{DISPATCH}:{last_no}: Run の spawn_command_pane へ見つけた実行ペインを渡していない\
         （再生ボタンを押すたびにペインが積む = #1657 の症状）"
    );
}

// --------------------------------------------- 3. マーカーは退避路だけ

/// スクリプト本体で `marker_line` を使ってよい形（退避路 / 側路が無いとき）以外の行
fn marker_outside_fallback(body: &[(usize, String)], fallback: &str) -> Vec<String> {
    body.iter()
        .filter(|(_, l)| l.contains("marker_line") && !l.contains("let marker_line"))
        .filter(|(_, l)| !l.contains(fallback) && !l.contains("None => (marker_line"))
        .map(|(n, l)| format!("{SHELL}:{n}: {}", l.trim()))
        .collect()
}

#[test]
fn 実行ペインのマーカーは退避路でだけ出す() {
    let src = production(SHELL);
    for (sig, fallback) in [
        ("fn posix_run_pane_script(", "2>/dev/null || {marker_line}"),
        (
            "fn powershell_run_pane_script(",
            "catch {{ {marker_line} }}",
        ),
    ] {
        let (body, at) = fn_body(SHELL, &src, sig);
        let text = joined(&body);
        assert!(
            text.contains(fallback),
            "{SHELL}:{at}: {sig:?} に側路へ書けなかったときの退避路 {fallback:?} が無い"
        );
        let bad = marker_outside_fallback(&body, fallback);
        assert!(
            bad.is_empty(),
            "{sig:?} が内部マーカーを退避路の外で画面へ出している（#1657）:\n{}",
            bad.join("\n")
        );
        // 案内（人の言葉）を出している
        assert!(
            text.contains("run_exit_hint(lang)"),
            "{SHELL}:{at}: {sig:?} が「終了コード N / Enter で閉じる」の案内を出していない"
        );
    }
    // 公開の入口は #1657 の形を既定にしている（旧形は A/B のときだけ）
    let (body, at) = fn_body(SHELL, &src, "pub fn run_pane_command(");
    let text = joined(&body);
    assert!(
        text.contains("legacy_1657()") && text.contains("imp::run_pane_command(command"),
        "{SHELL}:{at}: run_pane_command の既定が #1657 の形になっていない"
    );
}

#[test]
fn i1657_注入_マーカーの常時出力を名指しで落とせる() {
    // 注入 ③: `report` を `marker_line` 決め打ちにした形
    let body = vec![
        (10, "    let marker_line = format!(\"echo …\");".to_string()),
        (11, "    let report = marker_line.clone();".to_string()),
        (
            12,
            "            None => (marker_line, String::new()),".to_string(),
        ),
    ];
    let got = marker_outside_fallback(&body, "2>/dev/null || {marker_line}");
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains(":11:"), "{got:?}");
}

// --------------------------------------------- 4. バッジ

/// 絵文字（UI に使わない。描画プリミティブと svg だけで組む）
fn has_emoji(line: &str) -> bool {
    line.chars().any(|c| {
        let u = c as u32;
        (0x1F300..=0x1FAFF).contains(&u) || (0x2600..=0x27BF).contains(&u)
    })
}

#[test]
fn タイトルバーは実行ペインのバッジを描く() {
    let src = production(APP);
    let (body, at) = fn_body(APP, &src, "fn render_pane_header(");
    let text = joined(&body);
    for needle in [
        ".when(hv.run_badge,",
        "\"pane-run-badge\"",
        "run_badge_running()",
        "run_badge_succeeded()",
        "run_badge_failed(",
        "interactive_meta()",
    ] {
        assert!(
            text.contains(needle),
            "{APP}:{at}: render_pane_header に実行ペインのバッジの {needle:?} が無い（#1657）"
        );
    }
    // バッジの塊に絵文字が混ざっていない
    let (badge, _) = region(
        APP,
        &src[..],
        ".when(hv.run_badge,",
        "// role ラベル（カンプ",
    );
    let emoji: Vec<String> = badge
        .iter()
        .filter(|(_, l)| has_emoji(l))
        .map(|(n, l)| format!("{APP}:{n}: {}", l.trim()))
        .collect();
    assert!(
        emoji.is_empty(),
        "バッジに絵文字を使っている:\n{}",
        emoji.join("\n")
    );
}

// --------------------------------------------- 5. GUI の確定

#[test]
fn guiは出力のたびと定期更新で終了を確定する() {
    let src = production(APP);
    let (body, at) = fn_body(APP, &src, "    fn on_term_event(");
    assert!(
        joined(&body).contains("tako_control::dispatch::settle_run_pane(self, pane_id)"),
        "{APP}:{at}: on_term_event が実行ペインの終了を確定していない（バッジが 2 秒遅れる。#1657）"
    );
    let periodic = src
        .lines()
        .position(|l| l.contains("tako_control::dispatch::settle_run_panes(app)"));
    assert!(
        periodic.is_some(),
        "{APP}: 定期更新が settle_run_panes を呼んでいない（出力の無い終わり方を取りこぼす）"
    );
}

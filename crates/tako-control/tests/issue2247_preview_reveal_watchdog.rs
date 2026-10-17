//! プレビューのペインの「Finder で表示」が同じ操作の他の入口と 1 実装を通る番犬（#2247）
//!
//! 「Finder で表示」（Windows は「エクスプローラーで表示」= #617）の入口は 4 つある:
//! ファイルツリーの右クリック・ターミナルのパスの cmd+右クリック（#1182）・CLI `tako file reveal` /
//! MCP `tako_file_op`（op=reveal）・プレビューのペインの右クリック（ヘッダ = #185 / 本文 = #1684）。
//! 前の 3 つは dispatch の `FileOpKind::Reveal` を通り、失敗（消えたファイル・OS へ渡せない）を
//! 画面の通知と persist.log へ出していたが、**プレビューのメニューだけが**
//! `os_integration::reveal` を直に呼んで結果を `let _ =` で捨てていた。開いた後に消えた・
//! 動いたファイルのプレビューで押すと、`open -R` が「無い」で終わるのを誰も知らず、
//! 押しても何も起きない（Finder が出ない）になる。
//!
//! この番犬は `main.rs` / `sidebar.rs` のソースを走査して、
//! ① プレビューのメニューの `"reveal"` の腕が共有の入口（`reveal_in_file_manager`）を
//!    プレビューの画面（`NoticeArea::Preview`）で呼ぶこと（直呼びは A/B の腕の中だけ）
//! ② 共有の入口が dispatch（`FileOpKind::Reveal`）と共有の出し口（`notify_ui_dispatch_failed`）を
//!    通ること・ツリーの腕もそれを呼ぶこと
//! ③ tako-app の UI から `os_integration::reveal` を直に呼ぶ箇所が既知のものだけであること
//! を縛る。実経路（偽の `open` で受ける）は `scripts/test-preview-reveal-2247.sh` と
//! visual-test `preview-reveal-2247`。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// コメント行を落とした本文（説明文の中の名前を「通した証拠」と誤認しない）
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `marker` から始まる関数の本体（次の 4 字下げの `fn` まで）
fn fn_body<'a>(src: &'a str, marker: &str) -> &'a str {
    let (_, rest) = src.split_once(marker).unwrap_or_else(|| {
        panic!("`{marker}` が見つからない（#2247。名前を変えたらこの番犬も直す）")
    });
    let end = ["\n    fn ", "\n    pub(crate) fn ", "\n    pub fn "]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

/// `match id { … }` の 1 本の腕（`"<id>" => {` から次の `"…" =>` の手前まで）
fn match_arm<'a>(body: &'a str, id: &str) -> &'a str {
    let head = format!("\"{id}\" => {{");
    let (_, rest) = body
        .split_once(&head)
        .unwrap_or_else(|| panic!("`{head}` の腕が見つからない（#2247）"));
    let end = rest
        .match_indices("\" => ")
        .filter_map(|(at, _)| rest[..at].rfind('"'))
        .next()
        .unwrap_or(rest.len());
    &rest[..end]
}

/// ① プレビューのメニューの「Finder で表示」は共有の入口を通る（直呼びは A/B の腕の中だけ）
#[test]
fn プレビューのメニューのrevealは共有の入口を通る() {
    let main = code_only(&read("crates/tako-app/src/main.rs"));
    let menu = fn_body(&main, "fn render_pane_context_menu(");
    let arm = match_arm(menu, "reveal");
    assert!(
        arm.contains("reveal_in_file_manager(") && arm.contains("NoticeArea::Preview"),
        "プレビューのメニューの \"reveal\" が共有の入口（`reveal_in_file_manager` + \
         `NoticeArea::Preview`）を通っていない（#2247）。直に `os_integration::reveal` を呼ぶと\
         消えたファイルで押しても何も起きない（失敗を誰も知らない）に戻る。腕:\n{arm}"
    );
    // 直呼びは修正前の形を再現する A/B（`TAKO_2247_LEGACY=1`）の腕の中だけに許す
    let direct = arm.matches("os_integration::reveal(").count();
    if direct > 0 {
        let (legacy, modern) = arm
            .split_once("} else {")
            .unwrap_or_else(|| panic!("直呼びが A/B の分岐の外にある（#2247）。腕:\n{arm}"));
        assert!(
            legacy.contains("legacy_2247()") && legacy.matches("os_integration::reveal(").count() == direct,
            "`os_integration::reveal` の直呼びが `legacy_2247()` の腕の外にある（#2247）。腕:\n{arm}"
        );
        assert!(
            !modern.contains("os_integration::reveal("),
            "既定の腕が `os_integration::reveal` を直に呼んでいる（#2247）"
        );
    }
}

/// ② 共有の入口は dispatch と共有の出し口を通り、ツリーの腕もそれを呼ぶ
#[test]
fn 共有の入口はdispatchと通知を通りツリーも同じ入口() {
    let sidebar = code_only(&read("crates/tako-app/src/sidebar.rs"));
    let body = fn_body(&sidebar, "fn reveal_in_file_manager(");
    for mark in [
        "tako_control::dispatch(",
        "FileOpKind::Reveal",
        "notify_ui_dispatch_failed(",
    ] {
        assert!(
            body.contains(mark),
            "`reveal_in_file_manager` が {mark} を通っていない（#2247）。CLI / MCP と同じ dispatch を\
             通し、失敗は共有の出し口（persist.log + 通知欄 + A/B）から出すこと"
        );
    }
    assert!(
        !body.contains("os_integration::reveal("),
        "`reveal_in_file_manager` が dispatch を飛ばして OS を直に呼んでいる（#2247）"
    );
    let tree = fn_body(&sidebar, "fn handle_context_action(");
    let arm = match_arm(tree, "reveal");
    assert!(
        arm.contains("reveal_in_file_manager(") && arm.contains("NoticeArea::Tree"),
        "ツリー（とパスのメニュー #1182）の \"reveal\" が共有の入口を通っていない（#2247）。腕:\n{arm}"
    );
}

/// ③ tako-app の UI から `os_integration::reveal` を直に呼ぶのは既知の箇所だけ
///
/// 新しい画面に「Finder で表示」を足すときは `reveal_in_file_manager` を通す。
/// リストは**減る方向にしか動かさない**（直したのに残っているエントリはここが落とす）
#[test]
fn uiからのrevealの直呼びは既知の箇所だけ() {
    // （ファイル名, 許す数, 理由）
    const KNOWN: &[(&str, usize, &str)] = &[
        // プレビューのメニューの A/B の腕（`TAKO_2247_LEGACY=1` = 修正前の形の再現）
        ("main.rs", 1, "#2247 の A/B"),
        // 設定画面の「設定ファイルを Finder で表示」（#2247 の範囲外。残りは別 Issue）
        ("settings_window.rs", 1, "設定画面"),
    ];
    let dir = workspace_root().join("crates/tako-app/src");
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)
            .expect("tako-app/src を読める")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                // visual-test の節（self_test/）は製品の入口ではない
                if path.file_name().is_some_and(|n| n != "self_test") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = code_only(&std::fs::read_to_string(&path).unwrap_or_default());
            let n = src.matches("os_integration::reveal(").count();
            if n > 0 {
                let rel = path
                    .strip_prefix(&dir)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                found.push((rel, n));
            }
        }
    }
    found.sort();
    for (file, n) in &found {
        let allowed = KNOWN
            .iter()
            .find(|(f, _, _)| f == file)
            .map(|(_, max, _)| *max)
            .unwrap_or(0);
        assert!(
            *n <= allowed,
            "{file} が `os_integration::reveal` を {n} 回直に呼んでいる（許すのは {allowed} 回。#2247）。\
             UI の入口は `reveal_in_file_manager` を通すこと（実測: {found:?}）"
        );
    }
    for (file, max, why) in KNOWN {
        let n = found
            .iter()
            .find(|(f, _)| f == file)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        assert_eq!(
            n, *max,
            "既知の直呼び（{file}・{why}）が {max} 回から {n} 回に変わった（#2247）。\
             直したならリストから外すこと（実測: {found:?}）"
        );
    }
}

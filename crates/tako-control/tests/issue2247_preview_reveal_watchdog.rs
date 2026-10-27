//! 「Finder で表示」がどの入口からも 1 つの実体（tako 自身から NSWorkspace）へ届く番犬（#2247）
//!
//! 「Finder で表示」（Windows は「エクスプローラーで表示」= #617）の入口は、ファイルツリーの右クリック・
//! ターミナルのパスの cmd+右クリック（#1182）・プレビューのペインの右クリック（ヘッダ = #185 /
//! 本文 = #1684）・設定画面の「設定ファイルを Finder で表示」・右パネルの添付・CLI `tako file reveal` /
//! MCP `tako_file_op`（op=reveal）。報告（#2247）は「プレビューから押しても Finder が出てこない」で、
//! 切り分けると 2 つの層があった:
//! - **入口の層**: プレビューのメニューと設定画面だけが dispatch を通らず `os_integration::reveal` を
//!   直に呼んで結果を `let _ =` で捨てていた（開いた後に消えたファイルで押しても無言）
//! - **実体の層**: macOS の実体は子プロセスの `open -R` で、本番でも `open` は成功（終了状態 0）して
//!   いたのに Finder が前に出てこなかった（macOS 14 からの協調的なアクティベーションでは、子の
//!   `open` は tako ではないので前面を譲れない、という見立て）。実体を tako 自身から
//!   `NSWorkspace activateFileViewerSelectingURLs:`（前に Finder へ前面を譲る）へ替えた
//!
//! この番犬はソースを走査して、
//! ① プレビューのメニューの `"reveal"` の腕が共有の入口（`reveal_in_file_manager`）を
//!    プレビューの画面（`NoticeArea::Preview`）で呼ぶこと（直呼びは A/B の腕の中だけ）
//! ② 共有の入口が dispatch（`FileOpKind::Reveal`）と共有の出し口（`notify_ui_dispatch_failed`）を
//!    通ること・ツリーの腕もそれを呼ぶこと
//! ③ tako-app の UI から `os_integration::reveal` を直に呼ぶ箇所が A/B の腕だけであること
//!    （設定画面も dispatch を通る）
//! ④ dispatch の op=reveal の先の macOS の実体が `finder_reveal`（NSWorkspace）で、子の `open -R` は
//!    A/B（`reveal_legacy_2247`）の腕の中だけであること
//! ⑤ `finder_reveal` が Finder へ前面を譲ってから NSWorkspace を呼び、検証の記録の口は AppKit を
//!    呼ぶ前に戻ること（記録の口で本物の Finder を開かない）
//! を縛る。実経路（記録の口と偽の `open` で受ける）は `scripts/test-preview-reveal-2247.sh` と
//! visual-test `preview-reveal-2247`、URL の組み立ては `finder_reveal` の単体テスト。

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

/// ③ tako-app の UI から `os_integration::reveal` を直に呼ぶのは A/B の腕だけ
///
/// 新しい画面に「Finder で表示」を足すときは `reveal_in_file_manager`（または dispatch の
/// `FileOpKind::Reveal`）を通す。リストは**減る方向にしか動かさない**
#[test]
fn uiからのrevealの直呼びは既知の箇所だけ() {
    // （ファイル名, 許す数, 理由）
    const KNOWN: &[(&str, usize, &str)] = &[
        // プレビューのメニューの A/B の腕（`TAKO_2247_LEGACY=1` = 修正前の形の再現）
        ("main.rs", 1, "#2247 の A/B"),
        // 設定画面の「設定ファイルを Finder で表示」の A/B の腕（同上）
        ("settings_window.rs", 1, "#2247 の A/B"),
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

/// ③' 設定画面の「設定ファイルを Finder で表示」も dispatch を通り、直呼びは A/B の腕の中だけ
#[test]
fn 設定画面のrevealもdispatchを通る() {
    let src = code_only(&read("crates/tako-app/src/settings_window.rs"));
    let button = src
        .split_once("\"adv-reveal\"")
        .map(|(_, rest)| &rest[..rest.find(".child(").unwrap_or(rest.len())])
        .expect(
            "設定画面の `adv-reveal` ボタンが見つからない（#2247。名前を変えたらこの番犬も直す）",
        );
    assert!(
        button.contains("reveal_settings_file("),
        "設定画面の「Finder で表示」のボタンが `reveal_settings_file` を呼んでいない（#2247）:\n{button}"
    );
    let body = fn_body(&src, "fn reveal_settings_file(");
    let (before, after) = body
        .split_once("if TakoApp::legacy_2247() {")
        .unwrap_or_else(|| {
            panic!("`reveal_settings_file` の A/B の腕が読めない（#2247）:\n{body}")
        });
    let (legacy, modern) = after.split_once("\n        }\n").unwrap_or_else(|| {
        panic!("`reveal_settings_file` の A/B の腕の終わりが読めない（#2247）:\n{body}")
    });
    assert!(
        !before.contains("os_integration::reveal(")
            && legacy.contains("os_integration::reveal(")
            && legacy.contains("return;"),
        "`reveal_settings_file` の直呼びが A/B の腕の外にある（#2247）:\n{body}"
    );
    assert!(
        modern.contains("self.run(")
            && modern.contains("FileOpKind::Reveal")
            && !modern.contains("os_integration::reveal("),
        "`reveal_settings_file` の既定の腕が dispatch（`self.run` + `FileOpKind::Reveal`）を通っていない（#2247）:\n{body}"
    );
}

/// ④ dispatch の op=reveal の先の macOS の実体は NSWorkspace（`finder_reveal`）で、
/// 子の `open -R` は A/B の腕の中だけ
#[test]
fn macosの実体はnsworkspaceで子のopen_rはabの腕だけ() {
    let dispatch = code_only(&read("crates/tako-control/src/dispatch.rs"));
    let arm = dispatch
        .split_once("FileOpKind::Reveal => {")
        .map(|(_, rest)| &rest[..rest.find("FileOpKind::").unwrap_or(rest.len())])
        .expect("dispatch の `FileOpKind::Reveal` の腕が見つからない（#2247）");
    assert!(
        arm.contains("os_integration::reveal("),
        "dispatch の op=reveal が `os_integration::reveal` を通っていない（#2247）:\n{arm}"
    );
    let os = code_only(&read("crates/tako-control/src/platform/os_integration.rs"));
    let mac = os
        .split_once("#[cfg(target_os = \"macos\")]\nmod imp {")
        .map(|(_, rest)| &rest[..rest.find("\n}\n").unwrap_or(rest.len())])
        .expect("os_integration の macOS の実装が見つからない（#2247）");
    let reveal = fn_body(mac, "pub fn reveal(");
    assert!(
        reveal.contains("finder_reveal::reveal("),
        "macOS の `reveal` が NSWorkspace の実体（`finder_reveal::reveal`）を呼んでいない（#2247）。\
         子の `open -R` へ戻すと、成功しても Finder が前に出てこない（#2247 の報告）:\n{reveal}"
    );
    let (legacy, modern) = reveal
        .split_once("finder_reveal::reveal(")
        .expect("上で確かめた");
    assert!(
        legacy.contains("reveal_legacy_2247()") && legacy.contains("\"-R\""),
        "子の `open -R` が A/B（`reveal_legacy_2247`）の腕の外にある（#2247）:\n{reveal}"
    );
    assert!(
        !modern.contains("spawn_open("),
        "NSWorkspace の後に子の `open` も起こしている（#2247。Finder への頼みが 2 回になる）:\n{reveal}"
    );
    // macOS の実装の中で `-R` を渡すのはこの A/B の 1 か所だけ
    assert_eq!(
        mac.matches("\"-R\"").count(),
        1,
        "macOS の実装に `open -R` が A/B の腕の外にもある（#2247）"
    );
}

/// ⑤ `finder_reveal` は Finder へ前面を譲ってから NSWorkspace を呼び、記録の口は AppKit を呼ぶ前に戻る
#[test]
fn nsworkspaceの前に前面を譲り記録の口はappkitを呼ばない() {
    let src = code_only(&read("crates/tako-control/src/platform/finder_reveal.rs"));
    // 最上位の関数なので本体は次の桁 0 の `}` まで
    let body = src
        .split_once("pub(crate) fn reveal(")
        .map(|(_, rest)| &rest[..rest.find("\n}\n").unwrap_or(rest.len())])
        .expect("`finder_reveal::reveal` が見つからない（#2247。名前を変えたらこの番犬も直す）");
    let at = |needle: &str| {
        body.find(needle).unwrap_or_else(|| {
            panic!("`finder_reveal::reveal` に {needle} が無い（#2247）:\n{body}")
        })
    };
    let record = at("write_record(");
    let yielded = at("c\"yieldActivationToApplicationWithBundleIdentifier:\",\n");
    let activate = at("c\"activateFileViewerSelectingURLs:\"");
    assert!(
        record < yielded && yielded < activate,
        "順番が「記録の口で戻る → Finder へ前面を譲る → NSWorkspace」になっていない（#2247）。\
         記録の口より後で AppKit を呼ぶと検証で本物の Finder が出る・譲る前に頼むと前に出られない:\n{body}"
    );
    assert!(
        body[..record].trim_end().ends_with("return"),
        "記録の口が戻らずに先へ進む（#2247。検証で本物の Finder が出る）:\n{body}"
    );
}

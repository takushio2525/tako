//! **#2760 の番犬**: ファイルツリーの断りの通知を、絶対パスのまま・高さの上限なしで出す形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! ツリーの断りの通知（例: ⌥⌘V で「フォルダを自分自身へは移せない」）は dispatch の文面
//! （CLI / MCP の応答と同じ = 絶対パス入り）を包んで出すので、関係するファイルの絶対パスが
//! 3 回入った。サイドバーは既定で幅 240px なので、一時 dir のような長いパスだと通知が約 10 行に
//! 折り返して約 225px を占め、低い窓（既定 600px）ではツリーの行が窓の外へ押し出された
//! （#2716 の全節実行の tree-multiselect ⑦ で見つかった。利用者から見ても同じことが起きる）。
//!
//! いまは 2 段で止める:
//! - 文面: 画面へ出す直前に `tree_notice_text` がルートからの相対（ルートの外はホームを `~`）へ縮める
//!   （規則の正本は `tako_core::paths::shorten_paths_in_text`。CLI / MCP の応答は変えない）
//! - 高さ: 通知欄は `NOTICE_MAX_H`（本文 `NOTICE_MAX_LINES` 行ぶん）で止まり、超えた分は通知欄の中で
//!   スクロールして読む（`notice_scroll`。新しい通知は頭から見せる）
//!
//! ## ここで止めるもの
//!
//! 1. [`ツリーの通知は出す前にパスを縮める`] — `notify_ui_failure` が `NoticeArea::Tree` の文を
//!    `tree_notice_text(` に通してから `set_remote_notice(` へ渡し、`tree_notice_text` が
//!    `shorten_paths_in_text(` へツリーのルートとホームを渡す。A/B（`legacy_2760()`）を読む
//! 2. [`通知欄は高さに上限を置き中でスクロールする`] — 通知欄（`"remote-notice"`）に `max_h(px(NOTICE_MAX_H))`・
//!    `overflow_y_scroll()`・`track_scroll(&notice_scroll)` があり、`NOTICE_MAX_LINES` は 5 行以下。
//!    `set_remote_notice` が新しい通知でスクロールを頭へ戻す
//! 3. [`visual_testの節が実キーで高さと押し出しを測る`] — 節 `tree-notice-2760` が単独実行の腕と
//!    全節実行の両方に載り、「通知の高さが上限以内」「最後の行が窓の中に残る」を確かめる
//! 4. [`修正前の形を名指しできる`] — 修正前の形を注入すると 1〜3 のどれかが名指しで落ちる
//!
//! ## 相方
//!
//! 実 GUI の A/B は `scripts/test-tree-notice-2760.sh`（`TAKO_2760_LEGACY=1` は「①: 通知の高さが
//! 上限以内」で FAILED）。縮めの規則そのものは `tako_core::paths` の単体テスト（#2760）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const SECTION: &str = "crates/tako-app/src/self_test/tree_notice_2760.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `head` の後ろの最初の `{` から対の `}` までのバイト範囲（コードだけの眺めで括弧を数える）
fn block_range(code: &str, head: &str) -> Option<std::ops::Range<usize>> {
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in code.as_bytes()[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open..open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// 関数の本文（文字列は残しコメントだけ潰した眺め）
fn body_of(src: &str, head: &str) -> Option<String> {
    let range = block_range(&code_view::code_view(src), head)?;
    Some(code_view::without_comments(src)[range].to_string())
}

/// 両方が見つかり、`a` が `b` より前にあるか
fn before(a: Option<usize>, b: Option<usize>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a < b)
}

/// 規則 1: ツリーの通知は出す前にパスを縮めるか
fn shorten_problems(sidebar: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match body_of(sidebar, "pub(crate) fn notify_ui_failure(") {
        Some(body)
            if body.contains("NoticeArea::Tree")
                && before(
                    body.find("self.tree_notice_text(text)"),
                    body.find("self.set_remote_notice(text, true)"),
                ) => {}
        Some(_) => problems.push(
            "`notify_ui_failure` が `NoticeArea::Tree` の文を `tree_notice_text(` に通してから \
             `set_remote_notice(` へ渡していない（dispatch の文面の絶対パスがそのまま通知に載る）"
                .into(),
        ),
        None => problems.push("`fn notify_ui_failure(` が見つからない".into()),
    }
    match body_of(sidebar, "pub(crate) fn tree_notice_text(") {
        Some(body) => {
            for (needle, why) in [
                (
                    "tako_core::paths::shorten_paths_in_text(",
                    "縮めの規則の正本",
                ),
                ("self.filetree.roots()", "ツリーのルートからの相対"),
                ("tako_core::paths::home_dir()", "ルートの外はホームを `~`"),
                ("Self::legacy_2760()", "A/B（旧の形 = 縮めない）"),
            ] {
                if !body.contains(needle) {
                    problems.push(format!("`tree_notice_text` に `{needle}` が無い（{why}）"));
                }
            }
        }
        None => {
            problems.push("`fn tree_notice_text(` が無い（ツリーの通知のパスを縮める口）".into())
        }
    }
    if !sidebar.contains("std::env::var(\"TAKO_2760_LEGACY\")") {
        problems.push("`TAKO_2760_LEGACY` を読む口が無い（A/B）".into());
    }
    problems
}

/// `const NAME: f32 = <数>;` の数
fn const_f32(src: &str, name: &str) -> Option<f32> {
    let head = format!("const {name}: f32 = ");
    let at = src.find(&head)? + head.len();
    let end = at + src[at..].find(';')?;
    src[at..end].trim().replace('_', "").parse().ok()
}

/// 規則 2: 通知欄は高さに上限を置き、中でスクロールするか
fn cap_problems(sidebar: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::without_comments(sidebar);
    match code.find(".id(\"remote-notice\")") {
        Some(at) => {
            // 通知欄の要素の組み立て（`.id` から本文の `.child(SharedString` まで）
            let end = code[at..]
                .find(".child(SharedString::from(text))")
                .map_or(code.len(), |e| at + e);
            let element = &code[at..end];
            for (needle, why) in [
                (".max_h(px(NOTICE_MAX_H))", "高さの上限"),
                (".overflow_y_scroll()", "超えた分は中でスクロールして読む"),
                (".track_scroll(&notice_scroll)", "新しい通知で頭へ戻せる"),
                (
                    ".line_height(px(NOTICE_LINE_PX))",
                    "上限の計算と描画で同じ行の高さ",
                ),
            ] {
                if !element.contains(needle) {
                    problems.push(format!(
                        "通知欄（`remote-notice`）に `{needle}` が無い（{why}）"
                    ));
                }
            }
        }
        None => problems.push("通知欄（`.id(\"remote-notice\")`）が見つからない".into()),
    }
    match const_f32(sidebar, "NOTICE_MAX_LINES") {
        Some(lines) if (1.0..=5.0).contains(&lines) => {}
        Some(lines) => problems.push(format!(
            "`NOTICE_MAX_LINES` が {lines} 行（1〜5 行の外 = 低い窓でツリーの行を押し出す）"
        )),
        None => problems.push("`const NOTICE_MAX_LINES: f32` が無い".into()),
    }
    match body_of(sidebar, "pub(crate) fn set_remote_notice(") {
        Some(body) if body.contains("self.notice_scroll") && body.contains("set_offset(") => {}
        _ => problems.push(
            "`set_remote_notice` が通知欄のスクロールを頭へ戻していない（前の通知を読み進めた位置から\
             新しい通知が出る）"
                .into(),
        ),
    }
    problems
}

/// 規則 3: visual-test の節が単独・全節の両方に載り、高さと押し出しを確かめるか
fn section_problems(main: &str, section: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::without_comments(main);
    if !code.contains("\"tree-notice-2760\" => {") {
        problems.push("単独実行の腕 `\"tree-notice-2760\"` が無い（TAKO_VISUAL_ONLY）".into());
    }
    // 全節実行の並び（`tree_keys_visual` の後ろ = ツリーの節の続き）。単独実行の腕（`"tree-keys"`）の
    // 後ろにも近くに同じ呼び出しがあるので、間に腕の出口（`visual_exit(`）を挟まないものだけを数える
    let full_run = code
        .match_indices("tree_keys_visual(any, window, cx).await;")
        .any(|(at, _)| {
            code[at..]
                .find("tree_notice_2760_visual(any, window, cx).await;")
                .is_some_and(|offset| {
                    offset < 300 && !code[at..at + offset].contains("visual_exit(")
                })
        });
    if !full_run {
        problems.push("全節実行の並びに `tree_notice_2760_visual` が無い（#948）".into());
    }
    let body = code_view::without_comments(section);
    for (needle, why) in [
        ("通知の高さが上限以内", "通知の高さ"),
        ("ツリーの最後の行が窓の中に残る", "行の押し出し"),
        ("文面のパスはルートからの相対", "文面のパス"),
        ("move_key", "実キー（⌥⌘V）の入口"),
    ] {
        if !body.contains(needle) {
            problems.push(format!("節 tree-notice-2760 に `{needle}` が無い（{why}）"));
        }
    }
    problems
}

#[test]
fn ツリーの通知は出す前にパスを縮める() {
    let problems = shorten_problems(&read(SIDEBAR));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 通知欄は高さに上限を置き中でスクロールする() {
    let problems = cap_problems(&read(SIDEBAR));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn visual_testの節が実キーで高さと押し出しを測る() {
    let problems = section_problems(&read(MAIN), &read(SECTION));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 修正前の形を名指しできる() {
    let sidebar = read(SIDEBAR);
    let main = read(MAIN);
    let section = read(SECTION);
    assert!(shorten_problems(&sidebar).is_empty(), "今の本文は通る");
    assert!(cap_problems(&sidebar).is_empty(), "今の本文は通る");
    assert!(
        section_problems(&main, &section).is_empty(),
        "今の本文は通る"
    );
    // 1. 縮めずに出す（修正前）
    let raw = sidebar.replacen("self.tree_notice_text(text)", "text", 1);
    assert_ne!(raw, sidebar, "注入先（縮め）が本文に在る");
    assert!(
        shorten_problems(&raw)
            .iter()
            .any(|p| p.contains("`notify_ui_failure` が")),
        "縮めずに出す通知を名指せる"
    );
    // 2. 高さに上限を置かない（修正前）
    let uncapped = sidebar.replacen(".max_h(px(NOTICE_MAX_H))", "", 1);
    assert_ne!(uncapped, sidebar, "注入先（上限）が本文に在る");
    assert!(
        cap_problems(&uncapped)
            .iter()
            .any(|p| p.contains("NOTICE_MAX_H")),
        "上限の無い通知欄を名指せる"
    );
    // 2'. 上限の行数を広げすぎる
    let tall = sidebar.replacen(
        "const NOTICE_MAX_LINES: f32 = 4.0;",
        "const NOTICE_MAX_LINES: f32 = 12.0;",
        1,
    );
    assert_ne!(tall, sidebar, "注入先（行数の上限）が本文に在る");
    assert!(
        cap_problems(&tall)
            .iter()
            .any(|p| p.contains("NOTICE_MAX_LINES")),
        "広すぎる上限を名指せる"
    );
    // 3. 節が全節実行に載らない
    let unlisted = main.replacen(
        "            tree_keys_visual(any, window, cx).await;\n            \
         // #2760: ツリーの断りの通知の高さの上限と、パスの縮め\n            \
         tree_notice_2760_visual(any, window, cx).await;\n",
        "            tree_keys_visual(any, window, cx).await;\n",
        1,
    );
    assert_ne!(unlisted, main, "注入先（全節実行の並び）が本文に在る");
    assert!(
        section_problems(&unlisted, &section)
            .iter()
            .any(|p| p.contains("全節実行")),
        "全節実行に載らない節を名指せる"
    );
}

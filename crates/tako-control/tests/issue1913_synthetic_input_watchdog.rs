//! **#1913 の番犬**: tako 自身が描く合成入力欄は `tako_core::synthetic_input` の 1 実装で組む。
//!
//! ## なぜ止めるのか
//!
//! セルフテスト / visual-test は claude の入力欄に似た箱（罫線 + `❯` の行）を自分で描いて
//! 入力欄の検査に使い、それを `dialog` がダイアログと誤判定しないことを #754 の単体テストが
//! 固定している。描く側（`tako-app` の `main.rs`）が自前で箱を組み立てると、形を変えても
//! 単体テストは**古い形を検査したまま緑**になる。8 月の版では罫線の桁数（20 → 16）・
//! #737 の本文の状態数・キュー滞留ヒントの持ち主が実際にずれていた。
//!
//! ## 何を固定するか
//!
//! 1. [`描く側は合成入力欄を自前で組み立てない`] — `main.rs` の本番コードに罫線や
//!    `❯` + NBSP を組む形が戻ったら `file:line` で名指しする
//! 2. [`描く側4か所は共通の関数を呼ぶ`] — #719 / #718 / #737 / #1067 の入口を呼んでいる
//! 3. [`dialogの単体テストは共通の関数から形を作る`] — テスト側が手書きの写しへ戻ったら落とす
//! 4. [`検出器は自前の組み立てを名指しする`] — 検出器そのものの検出力（注入で確かめる）

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。コメントを落とした眺めも同じ部品から（#1609）
#[path = "common/production_range.rs"]
mod production_range;
use production_range::code_view;

#[path = "common/test_source.rs"]
mod test_source;

const APP: &str = "crates/tako-app/src/main.rs";
const DIALOG: &str = "crates/tako-core/src/dialog.rs";
const DIALOG_TEST: &str = "takoが自分で描く合成入力欄をダイアログと誤判定しない";

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

/// 本番コード（テスト領域を潰した眺め）からコメントも落とす。箱を組むのはコードなので、
/// 説明文に書いた `─` や `\u{276F}` で誤検出しない。バイト長と行番号は保たれる
fn production_code(rel: &str) -> String {
    let production = production_range::production(&read(rel), rel);
    code_view::without_comments_checked(&production, rel)
}

/// 箱を自前で組み立てている印（小文字へ寄せた行に当てる）。
/// 罫線は箱の形に必ず要るので、罫線の組み立てが戻れば必ずどれかに当たる
const SELF_BUILT: [&str; 10] = [
    "──",                 // 罫線の連なりを文字で直書き
    "\"─\"",              // `"─".repeat(N)`
    "'─'",                // `iter::repeat('─')`
    "\\u{2500}",          // `"\u{2500}".repeat(N)`
    "\\\\u2500",          // printf '%b' へ渡す `\\u2500`
    "❯\u{a0}",            // `❯` + NBSP を文字で直書き
    "❯\\u{a0}",           // `❯\u{a0}`
    "\\u{276f}\\u{a0}",   // `\u{276F}\u{a0}`
    "\\u{276f}\\u{00a0}", // `\u{276F}\u{00A0}`
    "\\\\u276f",          // printf '%b' へ渡す `\\u276F`
];

/// 自前の組み立てに当たった行（1-origin の行番号と行そのもの）
fn self_built_lines(view: &str) -> Vec<(usize, String)> {
    view.lines()
        .enumerate()
        .filter(|(_, line)| {
            let lower = line.to_lowercase();
            SELF_BUILT.iter().any(|p| lower.contains(p))
        })
        .map(|(i, line)| (i + 1, line.trim().to_string()))
        .collect()
}

#[test]
fn 描く側は合成入力欄を自前で組み立てない() {
    let hits = self_built_lines(&production_code(APP));
    assert!(
        hits.is_empty(),
        "合成入力欄（罫線 + `❯` の行）を自前で組み立てている。\n\
         `tako_core::synthetic_input` の関数を呼ぶこと（dialog の単体テストが同じ形を検査する。#1913）:\n{}",
        hits.iter()
            .map(|(line, text)| format!("  {APP}:{line}: {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 描く側4か所は共通の関数を呼ぶ() {
    let view = production_code(APP);
    let missing: Vec<String> = [
        ("#719", "synthetic_input::chat_g3_command("),
        ("#718", "synthetic_input::AUTOGROW_ROWS"),
        ("#718", "synthetic_input::autogrow_command("),
        ("#737", "synthetic_input::gui_input_paint"),
        ("#1067", "synthetic_input::restart_tui_paint"),
    ]
    .into_iter()
    .filter(|(_, call)| !view.contains(call))
    .map(|(issue, call)| format!("{issue} {call}"))
    .collect();
    assert!(
        missing.is_empty(),
        "{APP}: 合成入力欄の入口を呼んでいない: {missing:?}（#1913）"
    );
}

#[test]
fn dialogの単体テストは共通の関数から形を作る() {
    let src = read(DIALOG);
    let body = test_source::body_of(&src, DIALOG_TEST)
        .unwrap_or_else(|| panic!("{DIALOG}: テスト `{DIALOG_TEST}` が見つからない"));
    let start_line = src[..src.find(&body).expect("切り出した本体")]
        .matches('\n')
        .count();
    let code = code_view::without_comments(&body);
    let missing: Vec<&str> = [
        "synthetic_input",
        "chat_g3_lines()",
        "AUTOGROW_ROWS",
        "autogrow_lines(",
        "gui_input_lines(",
        "restart_tui_lines(",
    ]
    .into_iter()
    .filter(|call| !code.contains(call))
    .collect();
    let hits = self_built_lines(&code);
    // 「使っていない」と「手で写している」は同時に起きるので、1 件目で止めずに両方出す
    assert!(
        missing.is_empty() && hits.is_empty(),
        "{DIALOG}: `{DIALOG_TEST}` が描く側と同じ組み立てを使っていない（#1913）。\n\
         呼んでいない入口: {missing:?}\n\
         手で写している行（描く側の変更に追従しない）:\n{}",
        hits.iter()
            .map(|(line, text)| format!("  {DIALOG}:{}: {text}", start_line + line))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 検出器は自前の組み立てを名指しする() {
    // 寄せる前の 4 か所の書き方（#1913 の直前の main）と、その変種
    let injected = [
        r#"text: "clear; printf '\\n────────────────\\n❯ テスト\\n────────────────\\n'".into(),"#,
        r#"let rule = "\\u2500".repeat(16);"#,
        r#""\\u276F row0".to_string()"#,
        r#"let rule = "\u{2500}".repeat(20);"#,
        r#"format!("{rule}\n\u{276F}\u{a0}{body}\n{rule}\n")"#,
        r#"format!("{rule}\n\u{276f}\u{00A0}{body}")"#,
        r#"let rule = "─".repeat(30);"#,
        r#"let rule: String = std::iter::repeat('─').take(16).collect();"#,
        r#"format!("❯\u{a0}{body}")"#,
    ];
    // 4 か所以外で正当に使っている形（ダイアログの fixture・1 文字だけの罫線・入口の呼び出し）
    let legit = [
        r#"type_text(any, cx, "日本語のテスト ⏺ ⎿ │ ─ ✻ mix", false);"#,
        r#""   ❯ 1. Stop and wait for limit to reset\n","#,
        r#"let box_body = tako_core::synthetic_input::gui_input_paint;"#,
    ];
    let mut src = String::new();
    for line in legit.iter().chain(injected.iter()) {
        src.push_str(line);
        src.push('\n');
    }
    let named: Vec<usize> = self_built_lines(&src).into_iter().map(|(l, _)| l).collect();
    let want: Vec<usize> = (legit.len() + 1..=legit.len() + injected.len()).collect();
    assert_eq!(
        named,
        want,
        "注入した自前の組み立て（{} 行目以降）だけを名指しすること",
        legit.len() + 1
    );
}

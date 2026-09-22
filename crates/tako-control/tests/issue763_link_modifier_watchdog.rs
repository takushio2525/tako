//! リンクを開く修飾キーの直読みを止める番犬（Issue #763）。
//!
//! **何を止めるか**: リンク経路（ターミナルの URL / パス・PDF 注釈・Markdown・
//! リリースノート）が GPUI の `Modifiers::platform` を**直接**見ること。
//! `platform` は macOS = command / Windows = **Win キー**に解決されるので、
//! 直読みすると Windows では Win+クリックを要求する形になる。Win+クリックは OS の
//! シェルが先に取るので、ユーザーはリンクへ到達できない（#763 の症状そのもの）。
//!
//! 直し方は 1 つだけ: `keybindings::link_modifier_active`（正本は
//! `tako_core::platform::keys::link_modifier_active` の純粋関数）を通す。
//! `platform || control` を各サイトで素に書くのも**駄目**で、macOS の Ctrl+クリックは
//! 右クリック相当なのでコンテキストメニューとリンク開きが同時に走る。
//!
//! # なぜ tako-control に置くか
//!
//! 見たいのは tako-app（GPUI）のソースだが、突き合わせに GPUI のビルドを
//! 要求したくない。`ui_key_notation.rs` / `docs_keyboard_shortcuts.rs` と同じく
//! **ソースを読む**形にして、ソース走査の番犬の置き場（ここ）へ揃える。
//!
//! 判定そのものの正しさ（macOS = command のみ / Windows = control のみ）は
//! `tako_core::platform::keys` の単体が macOS 上から両 OS ぶん見る。
//! ここが見るのは**配線**（全サイトがその 1 実装を通っているか）だけ。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// リンク経路を実装しているファイル
const LINK_FILES: &[&str] = &[
    "crates/tako-app/src/main.rs",
    "crates/tako-app/src/preview_render.rs",
    "crates/tako-app/src/update_window.rs",
];

/// 判定を通すべき 1 実装（tako-app 側の薄いラッパー）
const HELPER: &str = "link_modifier_active";

/// GPUI の platform 修飾の直読み
const RAW: &str = "modifiers.platform";

/// リンク装飾のホバー更新。**実引数に修飾キーの真偽を渡す**ので、
/// ここへ `modifiers.platform` が来ていたらそのサイトは直読み
const HOVER_FNS: &[&str] = &[
    "update_hovered_link_at",
    "update_pdf_link_hover",
    "update_md_link_hover",
    "update_note_link_hover",
];

/// `main.rs` のマウス系ハンドラ。**本体に platform 直読みがあってはいけない**
/// （このファイルの他の `modifiers.platform` は keystroke 側 = 別の経路）
const MOUSE_HANDLER_FNS: &[&str] = &[
    "on_pane_mouse_down",
    "on_pane_right_mouse_down",
    "on_mouse_move",
    "on_modifiers_changed",
];

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// Rust ソースからコメントを落とす（落とした部分は空白へ置換するので行番号はずれない）。
///
/// 実装の意図を書くコメント（`// cmd+クリック: リンクを開く`）を検出対象にすると、
/// 説明を書き換えただけで落ちる番犬になる
fn strip_comments(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 1usize;
            out.push(' ');
            out.push(' ');
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if b[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        // 文字列リテラル（`"…"` / `r"…"` / `r#"…"#`）はそのまま写す。
        // 中の `//` をコメントと誤認しないため
        if c == '"' {
            out.push(c);
            i += 1;
            while i < b.len() {
                out.push(b[i]);
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.into_iter().collect()
}

/// バイト位置 → 1 始まりの行番号
fn line_of(src: &str, pos: usize) -> usize {
    src[..pos].matches('\n').count() + 1
}

/// `open` の位置（`(` / `{` のどれか）から対応する閉じ括弧までを返す
fn balanced(src: &str, open_at: usize, open: char, close: char) -> &str {
    let bytes: Vec<(usize, char)> = src[open_at..].char_indices().collect();
    let mut depth = 0i32;
    for (off, c) in bytes {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return &src[open_at..open_at + off + c.len_utf8()];
            }
        }
    }
    panic!("括弧が閉じていない（{open} at {open_at}）");
}

/// 規則 1: リンク装飾のホバー呼び出しは、実引数で判定の 1 実装を通す
#[test]
fn リンクホバーの呼び出しは修飾判定の1実装を通る() {
    let mut violations: Vec<String> = Vec::new();
    let mut calls = 0usize;
    for rel in LINK_FILES {
        let src = strip_comments(&read(rel));
        for name in HOVER_FNS {
            // 定義（`fn name(`）ではなく呼び出し（`.name(`）だけを見る
            let needle = format!(".{name}(");
            let mut from = 0usize;
            while let Some(i) = src[from..].find(&needle) {
                let at = from + i;
                let open = at + needle.len() - 1;
                let args = balanced(&src, open, '(', ')');
                from = open + args.len();
                calls += 1;
                let line = line_of(&src, at);
                if args.contains(RAW) {
                    violations.push(format!(
                        "{rel}:{line}: {name}() の実引数が {RAW} を直読みしている\
                         （{HELPER} を通すこと。#763）"
                    ));
                } else if args.contains("modifiers") && !args.contains(HELPER) {
                    // セルフテストの probe（`update_md_link_hover(pos, true, cx)`）は
                    // 修飾を読まずに真偽を直接与えるので対象外。**修飾を読んでいるのに
                    // 1 実装を通っていない**サイトだけを落とす
                    violations.push(format!(
                        "{rel}:{line}: {name}() が修飾キーを読んでいるのに {HELPER} を\
                         通っていない（判定は 1 実装に寄せる。#763）"
                    ));
                }
            }
        }
    }
    assert!(
        calls >= 6,
        "リンクホバーの呼び出しが {calls} 件しか見つからない（構造が変わった？）"
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 2: 「修飾キーを見て click_count も見る」= リンクを開くガード
///
/// 見るのは行ではなく**囲っている `if` の条件**（`&&` で改行されるため）。
/// 修飾キーを読んでいない `click_count` の用途（`match event.click_count` の
/// シングル / ダブル / トリプルクリック判定）は対象外
#[test]
fn リンクを開くクリック判定は修飾判定の1実装を通る() {
    let mut violations: Vec<String> = Vec::new();
    let mut guards = 0usize;
    for rel in LINK_FILES {
        let src = strip_comments(&read(rel));
        let mut from = 0usize;
        while let Some(i) = src[from..].find("click_count") {
            let at = from + i;
            from = at + "click_count".len();
            // 合成イベントの組み立て（`click_count: 1,`）は判定ではない
            if src[from..].starts_with(':') {
                continue;
            }
            let Some(if_at) = src[..at].rfind("if ") else {
                continue;
            };
            let Some(brace) = src[at..].find('{') else {
                continue;
            };
            let cond = &src[if_at..at + brace];
            // 修飾キーを読んでいない click_count は #763 の対象外
            if !cond.contains("modifiers") && !cond.contains(HELPER) {
                continue;
            }
            guards += 1;
            let line = line_of(&src, if_at);
            if cond.contains(RAW) {
                violations.push(format!(
                    "{rel}:{line}: リンクを開くクリック判定が {RAW} を直読みしている\
                     （{HELPER} を通すこと。#763）"
                ));
            } else if !cond.contains(HELPER) {
                violations.push(format!(
                    "{rel}:{line}: リンクを開くクリック判定が {HELPER} を通っていない（#763）"
                ));
            }
        }
    }
    assert!(
        guards >= 4,
        "リンクを開くクリック判定が {guards} 件しか見つからない（構造が変わった？）"
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 3: `main.rs` のマウス系ハンドラ本体に platform 直読みが無い
///
/// 規則 1 / 2 は「呼び出し」と「クリック判定」を見るので、
/// **`cmd+右クリック` のように click_count を見ないガード**（#1182）が漏れる。
/// ハンドラ 1 本ぶんを丸ごと見て、経路の入口を塞ぐ
#[test]
fn マウスハンドラはplatform修飾を直読みしない() {
    let rel = "crates/tako-app/src/main.rs";
    let src = strip_comments(&read(rel));
    let mut violations: Vec<String> = Vec::new();
    for name in MOUSE_HANDLER_FNS {
        let head = format!("\n    fn {name}(");
        let start = src
            .find(&head)
            .unwrap_or_else(|| panic!("{rel} に {name}() が見つからない（構造が変わった？）"));
        assert_eq!(
            src.matches(&head).count(),
            1,
            "{name}() が {rel} に複数ある（番犬がどれを見ているか曖昧になる）"
        );
        // シグネチャの `)` の次にある `{` から本体
        let sig = balanced(&src, start + head.len() - 1, '(', ')');
        let body_open = start + head.len() - 1 + sig.len();
        let body_open = body_open
            + src[body_open..]
                .find('{')
                .unwrap_or_else(|| panic!("{name}() の本体が見つからない"));
        let body = balanced(&src, body_open, '{', '}');
        let mut from = 0usize;
        while let Some(i) = body[from..].find(RAW) {
            let at = body_open + from + i;
            violations.push(format!(
                "{rel}:{}: {name}() が {RAW} を直読みしている（{HELPER} を通すこと。#763）",
                line_of(&src, at)
            ));
            from += i + RAW.len();
        }
    }
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 4: 合成イベントの修飾も 1 実装から組む
///
/// セルフテストが `Modifiers { platform: true, .. }` を直書きしていると、
/// 実装だけ Ctrl へ移っても**入力が Win キーのまま**になり、Windows の
/// セルフテストだけが落ちる（実装の退行と区別が付かない）。
/// 使ってよいのは `keybindings::link_modifiers` /`non_link_modifiers`。
///
/// **キーボード側（`Keystroke { modifiers: Modifiers { platform: true, .. } }`）は
/// 対象外**。あちらは GPUI のキーバインドと同じ platform 修飾で、#763 の範囲ではない
#[test]
fn 合成イベントの修飾も1実装から組む() {
    /// `Keystroke` かどうかを見るために遡る文字数（実測で 30 文字ほど手前にある）
    const LOOKBACK: usize = 200;
    let mut violations: Vec<String> = Vec::new();
    let mut literals = 0usize;
    for rel in LINK_FILES {
        let src = strip_comments(&read(rel));
        let mut from = 0usize;
        while let Some(i) = src[from..].find("Modifiers {") {
            let at = from + i;
            let body = balanced(&src, at + "Modifiers ".len(), '{', '}');
            from = at + "Modifiers {".len();
            literals += 1;
            if !body.contains("platform:") {
                continue;
            }
            let back = &src[at.saturating_sub(LOOKBACK)..at];
            if back.contains("Keystroke {") {
                continue;
            }
            violations.push(format!(
                "{rel}:{}: 合成イベントの修飾に platform を直書きしている\
                 （keybindings::link_modifiers / non_link_modifiers を使う。#763）",
                line_of(&src, at)
            ));
        }
    }
    assert!(
        literals >= 3,
        "Modifiers リテラルが {literals} 件しか見つからない（構造が変わった？）"
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

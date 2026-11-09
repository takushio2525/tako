//! 「修飾 + クリック / Enter」の修飾の直読みを止める番犬（Issue #1601）。
//!
//! **何を止めるか**: キーバインド表に無い「修飾 + クリック / Enter」の操作
//! （ペイン / タブの × の確認スキップ・git のコミット確定・タスクの返答・設定の確定）が
//! GPUI の `Modifiers::platform` を**直接**見ること。`platform` は macOS = command /
//! Windows = **Win キー**に解決されるので、直読みすると Windows では Win+クリック /
//! Win+Enter を要求する形になる。どちらも OS が先に取るので押せず、#1203 では案内ごと
//! 落としていた（#1601 の症状）。
//!
//! 直し方は 1 つだけ: `keybindings::platform_modifier_active`（正本は
//! `tako_core::platform::keys::platform_modifier_active` の純粋関数・リンクの #763 と
//! 同じ表）を通す。`platform || control` を各サイトで素に書くのも**駄目**で、
//! macOS の Ctrl+クリックは右クリック相当なのでコンテキストメニューと同時に走る。
//!
//! 規則は #763 の `issue763_link_modifier_watchdog` の 4 規則を流用した:
//!
//! 1. 確認スキップのクリック（`close_*_with_confirm` の呼び出し）の実引数
//! 2. 「修飾 + Enter」の確定（入力欄のキー処理の `"enter"` の腕）
//! 3. 判定に control を素で混ぜない（呼び手・薄い層・正本の macOS の腕）
//! 4. 案内の表示は判定と同じ表から引き、`Option` で落とす形へ戻さない
//!
//! 判定そのものの正しさ（macOS = command のみ / Windows = control のみ）は
//! `tako_core::platform::keys` の単体が macOS 上から両 OS ぶん見る。
//! ここが見るのは**配線**（全サイトがその 1 実装を通っているか）だけ。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 判定を通すべき 1 実装（tako-app 側の薄いラッパー）
const HELPER: &str = "platform_modifier_active";

/// GPUI の platform 修飾の直読み（`event.modifiers().platform` / `ks.modifiers.platform`）
const RAW: &[&str] = &["modifiers().platform", "modifiers.platform"];

/// GPUI の control 修飾の直読み（規則 3）
const RAW_CONTROL: &[&str] = &["modifiers().control", "modifiers.control"];

/// 確認スキップの × を実装しているファイル
const CLOSE_FILES: &[&str] = &[
    "crates/tako-app/src/main.rs",
    "crates/tako-app/src/chat_view.rs",
    "crates/tako-app/src/starter.rs",
    "crates/tako-app/src/tab_bar.rs",
];

/// 確認スキップの判定を受ける関数（第 2 引数が「修飾が押されているか」）
const CLOSE_FNS: &[&str] = &["close_pane_with_confirm", "close_tab_with_confirm"];

/// 「修飾 + Enter」で確定する入力欄のキー処理（ファイル, 関数名）
const ENTER_HANDLERS: &[(&str, &str)] = &[
    (
        "crates/tako-app/src/right_panel.rs",
        "handle_git_commit_key",
    ),
    (
        "crates/tako-app/src/tasks_panel.rs",
        "handle_task_comment_key",
    ),
    ("crates/tako-app/src/settings_window.rs", "handle_key"),
];

/// 案内の表示サイト（ファイル, 案内を組む関数, 実引数が通るべき表の関数）
const HINT_SITES: &[(&str, &str, &str)] = &[
    (
        "crates/tako-app/src/main.rs",
        "close_skip_hint",
        "modifier_click_prefix",
    ),
    (
        "crates/tako-app/src/right_panel.rs",
        "git_commit_placeholder",
        "modifier_enter",
    ),
    (
        "crates/tako-app/src/settings_window.rs",
        "advanced_edit_help",
        "platform_modifier",
    ),
];

/// 案内を組む関数の定義（規則 4 の `Option` 検査）
const HINT_FNS: &[(&str, &str)] = &[
    ("crates/tako-app/src/ui_text/dialog.rs", "close_skip_hint"),
    (
        "crates/tako-app/src/ui_text/panel.rs",
        "git_commit_placeholder",
    ),
    (
        "crates/tako-app/src/ui_text/settings.rs",
        "advanced_edit_help",
    ),
];

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// 同じソースの 2 つの眺め（バイト長が同じなので位置を共有できる）。
///
/// `code` は文字列も潰した眺めで括弧の対応を数える（`"{branch}"` の `{` を数えない）。
/// `text` はコメントだけを潰した眺めで中身を見る（`"enter"` や直書きの記号を残す）
struct Views {
    code: String,
    text: String,
}

fn views(rel: &str) -> Views {
    let src = read(rel);
    let code = code_view::code_view(&src);
    let text = code_view::without_comments(&src);
    assert_eq!(code.len(), text.len(), "{rel}: 眺めの長さがずれた");
    Views { code, text }
}

/// バイト位置 → 1 始まりの行番号
fn line_of(src: &str, pos: usize) -> usize {
    src[..pos].matches('\n').count() + 1
}

/// `open_at`（`(` / `{` のどれか）から対応する閉じ括弧の次までのバイト位置
fn balanced_end(code: &str, open_at: usize, open: u8, close: u8) -> usize {
    let mut depth = 0i32;
    for (off, b) in code.as_bytes()[open_at..].iter().enumerate() {
        if *b == open {
            depth += 1;
        } else if *b == close {
            depth -= 1;
            if depth == 0 {
                return open_at + off + 1;
            }
        }
    }
    panic!("括弧が閉じていない（{} at {open_at}）", open as char);
}

/// 関数 `name` の本体（`{ … }`）の範囲。**ファイルに 1 つだけ**であること
fn fn_body(v: &Views, rel: &str, name: &str) -> (usize, usize) {
    let head = format!("fn {name}(");
    let hits: Vec<usize> = v.code.match_indices(&head).map(|(i, _)| i).collect();
    assert_eq!(
        hits.len(),
        1,
        "{rel} に {name}() が {} 個ある（構造が変わった？番犬がどれを見ているか曖昧になる）",
        hits.len()
    );
    let sig_open = hits[0] + head.len() - 1;
    let sig_end = balanced_end(&v.code, sig_open, b'(', b')');
    let body_open = sig_end
        + v.code[sig_end..]
            .find('{')
            .unwrap_or_else(|| panic!("{name}() の本体が見つからない"));
    (body_open, balanced_end(&v.code, body_open, b'{', b'}'))
}

/// main.rs の `mod self_test { … }` を空白へ潰す（バイト長・行番号は保つ）。
/// セルフテスト / visual-test は案内を `Platform` を固定して組んで画面と突き合わせるので、
/// 「実行 OS で引く」規則の対象は本番の描画だけにする
fn without_self_test(mut v: Views) -> Views {
    let Some(at) = v.code.find("\nmod self_test {") else {
        return v;
    };
    let open = at + "\nmod self_test ".len();
    let end = balanced_end(&v.code, open, b'{', b'}');
    let blank = |s: &mut String| {
        let mut bytes = std::mem::take(s).into_bytes();
        for b in &mut bytes[at..end] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        // 範囲の両端は ASCII（`\n` と `}`）なので、中の多バイト文字は丸ごと空白になり UTF-8 のまま
        *s = String::from_utf8(bytes).expect("潰した後も UTF-8");
    };
    blank(&mut v.code);
    blank(&mut v.text);
    v
}

fn has_any(s: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| s.contains(n))
}

/// 確認スキップの呼び出しの実引数（`(位置, 実引数の text)`）
fn close_calls() -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for rel in CLOSE_FILES {
        let v = views(rel);
        for name in CLOSE_FNS {
            // 定義（`fn name(`）ではなく呼び出し（`.name(`）だけを見る
            let needle = format!(".{name}(");
            for (at, _) in v.code.match_indices(&needle) {
                let open = at + needle.len() - 1;
                let end = balanced_end(&v.code, open, b'(', b')');
                out.push((
                    rel.to_string(),
                    line_of(&v.code, at),
                    v.text[open..end].to_string(),
                ));
            }
        }
    }
    out
}

/// 「修飾 + Enter」の腕（`"enter" if … => …` / `"enter" => …`）。
/// `(ファイル, 行, 腕の text = ガード + 本体)`
fn enter_arms() -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for (rel, name) in ENTER_HANDLERS {
        let v = views(rel);
        let (start, end) = fn_body(&v, rel, name);
        let body = &v.text[start..end];
        let mut arms = 0usize;
        for (off, _) in body.match_indices("\"enter\"") {
            let at = start + off;
            let after = v.text[at + "\"enter\"".len()..].trim_start();
            // `matches!(…, "enter" | "escape" …)` のような並べ方は腕ではない
            if !(after.starts_with("if ") || after.starts_with("=>")) {
                continue;
            }
            let arrow = at
                + v.code[at..]
                    .find("=>")
                    .unwrap_or_else(|| panic!("{rel}: \"enter\" の腕の => が見つからない"));
            let rest = &v.code[arrow + 2..];
            let lead = rest.len() - rest.trim_start().len();
            let arm_start = arrow + 2 + lead;
            let arm_end = if v.code.as_bytes()[arm_start] == b'{' {
                balanced_end(&v.code, arm_start, b'{', b'}')
            } else {
                arm_start
                    + v.code[arm_start..]
                        .find(",\n")
                        .unwrap_or_else(|| panic!("{rel}: \"enter\" の腕の終わりが見つからない"))
            };
            arms += 1;
            out.push((
                rel.to_string(),
                line_of(&v.code, at),
                v.text[at..arm_end].to_string(),
            ));
        }
        assert!(
            arms >= 1,
            "{rel}: {name}() に \"enter\" の腕が 1 つも見つからない（構造が変わった？）"
        );
    }
    out
}

/// 規則 1: 確認スキップのクリックは、実引数で判定の 1 実装を通す
#[test]
fn 確認スキップのクリックは修飾判定の1実装を通る() {
    let mut violations: Vec<String> = Vec::new();
    let mut reads = 0usize;
    for (rel, line, args) in close_calls() {
        // キーボードの close（`false`）とセルフテストの直接の真偽（`true`）は
        // 修飾を読まないので対象外。**修飾を読んでいる**サイトだけを見る
        if !args.contains("modifiers") {
            continue;
        }
        reads += 1;
        if has_any(&args, RAW) {
            violations.push(format!(
                "{rel}:{line}: 確認スキップの判定が GPUI の platform 修飾を直読みしている\
                 （Windows では Win+クリック = 押せない。keybindings::{HELPER} を通すこと。#1601）"
            ));
        } else if !args.contains(HELPER) {
            violations.push(format!(
                "{rel}:{line}: 確認スキップの判定が修飾キーを読んでいるのに {HELPER} を\
                 通っていない（判定は 1 実装に寄せる。#1601）"
            ));
        }
    }
    // ペイン枠・チャット表示・スターターの × とタブバーの ×
    assert!(
        reads >= 4,
        "修飾キーを読む確認スキップのクリックが {reads} 件しか見つからない（構造が変わった？）"
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 2: 「修飾 + Enter」の確定は、腕のガード / 本体で判定の 1 実装を通す。
///
/// 入力欄ごとに「判定を通る `"enter"` の腕が 1 つはある」も見る（直読みを消しただけで
/// 確定の腕ごと無くなった、を拾う）。どちらも `file:line` で名指す
#[test]
fn 修飾enterの確定は修飾判定の1実装を通る() {
    let mut violations: Vec<String> = Vec::new();
    let arms = enter_arms();
    for (rel, line, arm) in &arms {
        if has_any(arm, RAW) {
            violations.push(format!(
                "{rel}:{line}: \"enter\" の確定が GPUI の platform 修飾を直読みしている\
                 （Windows では Win+Enter = 押せない。keybindings::{HELPER} を通すこと。#1601）"
            ));
        }
    }
    // git のコミット欄・タスクの返答欄・設定の複数行欄のそれぞれ
    for (rel, name) in ENTER_HANDLERS {
        let through = arms
            .iter()
            .any(|(r, _, arm)| r == rel && arm.contains(HELPER));
        if !through {
            let first = arms
                .iter()
                .find(|(r, _, _)| r == rel)
                .map_or(0, |(_, line, _)| *line);
            violations.push(format!(
                "{rel}:{first}: {name}() の \"enter\" の腕が 1 つも {HELPER} を通っていない\
                 （修飾 + Enter の確定が判定の 1 実装を通らない。#1601）"
            ));
        }
    }
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 3: 判定に control を素で混ぜない（macOS の Ctrl+クリックは右クリック相当）。
///
/// 呼び手（規則 1 / 2 のサイト）・tako-app の薄い層・正本の macOS の腕の 3 か所を見る。
/// `platform || control` は Windows の CI では緑のまま macOS だけを壊すので、
/// 単体テスト（`主修飾キーはosごとに1つだけ受理する`）と合わせて配線でも止める
#[test]
fn 修飾判定にcontrolを素で混ぜない() {
    let mut violations: Vec<String> = Vec::new();
    for (rel, line, args) in close_calls() {
        if has_any(&args, RAW_CONTROL) || (args.contains("modifiers") && args.contains("||")) {
            violations.push(format!(
                "{rel}:{line}: 確認スキップの判定に control を混ぜている\
                 （macOS の Ctrl+クリックは右クリック相当。{HELPER} だけを通すこと。#1601）"
            ));
        }
    }
    for (rel, line, arm) in enter_arms() {
        if has_any(&arm, RAW_CONTROL) {
            violations.push(format!(
                "{rel}:{line}: \"enter\" の確定に control を素で混ぜている\
                 （OS ごとの選び分けは {HELPER} の 1 実装に任せる。#1601）"
            ));
        }
    }
    // tako-app の薄い層: GPUI の Modifiers を正本の引数へ落とすだけ
    let rel = "crates/tako-app/src/keybindings.rs";
    let v = views(rel);
    let (start, end) = fn_body(&v, rel, HELPER);
    let body = &v.code[start..end];
    if body.contains("||") || body.contains("match ") || body.contains("if ") {
        violations.push(format!(
            "{rel}:{}: {HELPER}() の本体で分岐している（表は tako_core::platform::keys の \
             純粋関数へ置く。ここで分けると macOS の CI から Windows 側を検証できない。#1601）",
            line_of(&v.code, start)
        ));
    }
    if !body.contains(&format!("tako_core::platform::keys::{HELPER}(")) {
        violations.push(format!(
            "{rel}:{}: {HELPER}() が tako_core の正本を呼んでいない（#1601）",
            line_of(&v.code, start)
        ));
    }
    // 正本の macOS の腕: control を見ない
    let rel = "crates/tako-core/src/platform/keys.rs";
    let v = views(rel);
    let (start, end) = fn_body(&v, rel, HELPER);
    let body = &v.code[start..end];
    let mac = body
        .find("Platform::MacOs =>")
        .unwrap_or_else(|| panic!("{rel}: {HELPER}() に macOS の腕が見つからない"));
    let mac_arm = &body[mac..body[mac..].find(',').map_or(body.len(), |e| mac + e)];
    if mac_arm.contains("control") {
        violations.push(format!(
            "{rel}:{}: {HELPER}() の macOS の腕が control を見ている\
             （macOS の Ctrl+クリックは右クリック相当。#1601）",
            line_of(&v.code, start + mac)
        ));
    }
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 4: 案内の表示は判定と同じ表（`tako_core::platform::keys`）から実行 OS で引き、
/// `Option` で「Windows では出さない」形へ戻さない。
///
/// 判定だけ Ctrl へ移して案内を直書き（`⌘`）のままにすると、Windows で案内が嘘になる。
/// 逆に案内だけ表から引いて判定が platform のままだと、案内どおりに押しても効かない
/// （こちらは規則 1 / 2 が止める）
#[test]
fn 修飾操作の案内は判定と同じ表から引く() {
    let mut violations: Vec<String> = Vec::new();
    for (rel, hint_fn, table_fn) in HINT_SITES {
        // セルフテスト / visual-test は案内を Platform を固定して組んでよい（突き合わせの控え）
        let v = without_self_test(views(rel));
        let needle = format!("{hint_fn}(");
        let mut calls = 0usize;
        for (at, _) in v.code.match_indices(&needle) {
            // 定義は ui_text 側にあるので、ここに来るのは呼び出しだけ
            let open = at + needle.len() - 1;
            let end = balanced_end(&v.code, open, b'(', b')');
            let args = &v.text[open..end];
            let line = line_of(&v.code, at);
            calls += 1;
            if !args.contains(&format!("keys::{table_fn}(")) {
                violations.push(format!(
                    "{rel}:{line}: {hint_fn}() の修飾が tako_core::platform::keys::{table_fn} \
                     から来ていない（判定と同じ表から引くこと。#1601）"
                ));
            }
            if !args.contains("Platform::current()") {
                violations.push(format!(
                    "{rel}:{line}: {hint_fn}() の修飾が実行 OS（Platform::current()）で\
                     引かれていない（#1601）"
                ));
            }
            if has_any(args, &["\\u{2318}", "\u{2318}", "\"Cmd", "\"Ctrl", "cfg!("]) {
                violations.push(format!(
                    "{rel}:{line}: {hint_fn}() の実引数に修飾の直書き / cfg! の分岐がある（#1601）"
                ));
            }
        }
        // 件数の崩れも違反として並べる（ここで止めると他の名指しが隠れる）
        if calls != 1 {
            violations.push(format!(
                "{rel}: 本番コードの {hint_fn}() の呼び出しが {calls} 件（1 件のはず。構造が変わった？）"
            ));
        }
    }
    for (rel, hint_fn) in HINT_FNS {
        let v = views(rel);
        let head = format!("fn {hint_fn}(");
        let at = v
            .code
            .find(&head)
            .unwrap_or_else(|| panic!("{rel} に {hint_fn}() が見つからない（構造が変わった？）"));
        let open = at + head.len() - 1;
        let end = balanced_end(&v.code, open, b'(', b')');
        if v.code[open..end].contains("Option<") {
            violations.push(format!(
                "{rel}:{}: {hint_fn}() が修飾を Option で受けている\
                 （「Windows では案内を出さない」経路を作らない。両 OS とも押せる。#1601）",
                line_of(&v.code, at)
            ));
        }
    }
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

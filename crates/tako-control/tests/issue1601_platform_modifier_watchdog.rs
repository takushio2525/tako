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
//!
//! #2844 で残りの直読み 3 系統（修飾キーを押しながらのファイルのドロップ・入力欄の
//! `"v"` の貼り付け・CLI `confirm-close` のヘルプの `cmd+W`）を表へ寄せ、規則を足した:
//!
//! 5. 本番コードの `Modifiers::platform` の直読みは、理由つきの凍結表（関数単位の件数）の
//!    外で増やさない。ドロップの判定と入力欄の貼り付けの腕は判定の 1 実装を通る
//! 6. `PasteClipboard` のアクションのハンドラは伝播を止めたまま（`propagate()` を呼ばない）。
//!    呼ぶと入力欄の `"v"` の腕へも打鍵が落ち、同じ ⌘V / Ctrl+V で 2 回貼る

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;
#[path = "common/production_range.rs"]
mod production_range;

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

// --------------------------------------------------------------- #2844（規則 5 / 6）

/// 入力欄の貼り付けの腕が通す 1 実装（tako-app 側の薄いラッパー）
const PASTE_HELPER: &str = "paste_key_active";

/// 規則 5: 本番コードで `Modifiers::platform` の直読みを許す場所
/// （ファイル, 関数, その関数の中の件数, 理由）。
///
/// **「修飾 + X」で何かをする判定（主修飾を選ぶもの）はここへ足さない**。それは
/// `keybindings::platform_modifier_active` / `paste_key_active` を通す（Windows では
/// platform = Win キーで、案内も受理もできない）。ここに載るのは主修飾を選ばない読み
/// （薄い層そのもの・バインド表の読み取り・表記の整形・「どれかの修飾が立っていたら文字
/// 入力ではない」の除外）だけ。件数が増えても減っても落ちる（直したら表から消す）
const KNOWN_PLATFORM_READS: &[(&str, &str, usize, &str)] = &[
    (
        "crates/tako-app/src/keybindings.rs",
        "link_modifier_active",
        1,
        "薄い層: GPUI の Modifiers を正本 keys::link_modifier_active の引数へ落とすだけ",
    ),
    (
        "crates/tako-app/src/keybindings.rs",
        "platform_modifier_active",
        1,
        "薄い層: GPUI の Modifiers を正本 keys::platform_modifier_active の引数へ落とすだけ",
    ),
    (
        "crates/tako-app/src/keybindings.rs",
        "paste_key_active",
        1,
        "薄い層: GPUI の Keystroke を正本 keys::paste_key_active の引数へ落とすだけ（#2844）",
    ),
    (
        "crates/tako-app/src/keybindings.rs",
        "shortcut_hint_for",
        1,
        "バインド表（張る側の定義）の読み取り: Windows で Win キーのバインドを案内に出さない",
    ),
    (
        "crates/tako-app/src/menu_bar.rs",
        "format_keystroke",
        1,
        "表記の整形: バインドの修飾をメニューの右端へ書く（判定ではない）",
    ),
    (
        "crates/tako-app/src/menu_bar.rs",
        "menu_bar_key",
        1,
        "修飾なし（plain）の判定: どの修飾も立っていない F10 / 矢印だけをメニューが受ける",
    ),
    (
        "crates/tako-app/src/tasks_panel.rs",
        "handle_task_comment_key",
        1,
        "platform || control の修飾つきを文字入力から外す（主修飾を選ばない）",
    ),
    (
        "crates/tako-app/src/right_panel.rs",
        "handle_git_commit_key",
        1,
        "platform || control の修飾つきを文字入力から外す（主修飾を選ばない）",
    ),
    (
        "crates/tako-app/src/sidebar.rs",
        "handle_inline_edit_key",
        1,
        "control || platform の修飾つきを文字入力から外す（主修飾を選ばない）",
    ),
    (
        "crates/tako-app/src/main.rs",
        "handle_search_bar_key",
        1,
        "platform || control || alt の修飾つきを検索欄の文字入力から外す（主修飾を選ばない）",
    ),
    (
        "crates/tako-app/src/main.rs",
        "handle_key",
        1,
        "表に無い platform 付きのキー（⌘ / Win）を PTY へ流さない関所（端末の修飾に platform は無い）",
    ),
    (
        "crates/tako-app/src/preview_render.rs",
        "render_preview_pane",
        1,
        "platform || control + ホイールで拡大縮小（両方を受ける・主修飾を選ばない）",
    ),
];

/// ドロップで「修飾キーを押しながら」を読む代入（`drop_cmd_held = …` / `let cmd_held = …`）
const DROP_FILE: &str = "crates/tako-app/src/main.rs";
/// （rustfmt が `=` の後で折り返すので、目印は `=` で止める）
const DROP_READS: &[&str] = &["drop_cmd_held =", "let cmd_held ="];

/// 入力欄の `"v"`（貼り付け）を扱うキー処理（ファイル, 関数名, `"v"` の件数）
const PASTE_HANDLERS: &[(&str, &str, usize)] = &[
    (
        "crates/tako-app/src/tasks_panel.rs",
        "handle_task_comment_key",
        1,
    ),
    (
        "crates/tako-app/src/right_panel.rs",
        "handle_git_branch_input_key",
        1,
    ),
    // match の腕（打鍵を握るかの `handled` も同じ関数を呼ぶ。直読みへ戻すと規則 5 が名指す）
    ("crates/tako-app/src/settings_window.rs", "handle_key", 1),
];

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `pos` を本体に含むいちばん内側の `fn 名前(…) { … }` の名前（`code` は code_view の眺め）
fn enclosing_fn(code: &str, pos: usize) -> Option<String> {
    let bytes = code.as_bytes();
    for (at, _) in code[..pos].rmatch_indices("fn ") {
        if at > 0 && is_ident(bytes[at - 1]) {
            continue;
        }
        let name_start = at + "fn ".len();
        let name_len = code[name_start..]
            .bytes()
            .take_while(|b| is_ident(*b))
            .count();
        if name_len == 0 {
            continue;
        }
        let after = name_start + name_len;
        // `fn 名前(` / `fn 名前<T>(` だけ（型の `fn(u8)` は名前が無いので上で落ちる）
        let Some(paren) = code[after..].find('(').map(|o| after + o) else {
            continue;
        };
        let between = code[after..paren].trim();
        if !(between.is_empty() || between.starts_with('<')) {
            continue;
        }
        let sig_end = balanced_end(code, paren, b'(', b')');
        let tail = &code[sig_end..];
        let open = match (tail.find('{'), tail.find(';')) {
            (Some(b), Some(s)) if s < b => continue, // 本体の無い宣言（trait の中など）
            (Some(b), _) => sig_end + b,
            _ => continue,
        };
        if open > pos {
            continue;
        }
        if pos < balanced_end(code, open, b'{', b'}') {
            return Some(code[name_start..after].to_string());
        }
    }
    None
}

/// 本番コードの `Modifiers::platform` の直読み（`(ファイル, 行, 関数, 行の原文)`）
fn platform_reads() -> Vec<(String, usize, String, String)> {
    let product = production_range::product_sources();
    let test_only = production_range::test_only_files(&product);
    let mut out = Vec::new();
    for rel in product.iter().filter(|r| !test_only.contains(*r)) {
        let src = read(rel);
        if !has_any(&src, RAW) {
            continue;
        }
        // テスト領域を潰した原文 → コメントと文字列も潰した眺め（バイト長と行は保つ）
        let text = production_range::scan(&src).text;
        let code = code_view::code_view(&text);
        let mut hits: Vec<usize> = RAW
            .iter()
            .flat_map(|raw| code.match_indices(raw).map(|(i, _)| i))
            .collect();
        hits.sort_unstable();
        hits.dedup();
        for at in hits {
            let line = line_of(&code, at);
            let source_line = src.lines().nth(line - 1).unwrap_or("").trim().to_string();
            let func = enclosing_fn(&code, at).unwrap_or_else(|| "（関数の外）".to_string());
            out.push((rel.clone(), line, func, source_line));
        }
    }
    out
}

/// 規則 5: `Modifiers::platform` の直読みは凍結表の外で増やさない（#2844）。
///
/// 表に無い場所の直読みは `file:line` で名指す。主修飾を選ぶ判定なら
/// `keybindings::platform_modifier_active` / `paste_key_active` を通し、主修飾を選ばない
/// 読み（どれかの修飾が立っていたら文字入力ではない、など）なら理由を添えて表へ足す
#[test]
fn platform修飾の直読みは凍結表の外で増やさない() {
    let reads = platform_reads();
    let mut by_fn: BTreeMap<(String, String), Vec<(usize, String)>> = BTreeMap::new();
    for (rel, line, func, source) in &reads {
        by_fn
            .entry((rel.clone(), func.clone()))
            .or_default()
            .push((*line, source.clone()));
    }
    let mut violations: Vec<String> = Vec::new();
    for ((rel, func), sites) in &by_fn {
        let known = KNOWN_PLATFORM_READS
            .iter()
            .find(|(r, f, _, _)| r == rel && f == func);
        let allowed = known.map_or(0, |(_, _, n, _)| *n);
        if sites.len() > allowed {
            for (line, source) in sites {
                violations.push(format!(
                    "{rel}:{line}: {func}() が GPUI の platform 修飾を直読みしている（凍結表は {allowed} 件・\
                     実際は {} 件）: `{source}`\n  → Windows では platform = Win キー。「修飾 + X」の判定なら \
                     keybindings::{HELPER} / {PASTE_HELPER} を通す。主修飾を選ばない読みなら理由を添えて \
                     KNOWN_PLATFORM_READS へ足す（#2844）",
                    sites.len()
                ));
            }
        }
    }
    for (rel, func, n, _) in KNOWN_PLATFORM_READS {
        let actual = by_fn
            .get(&(rel.to_string(), func.to_string()))
            .map_or(0, Vec::len);
        if actual < *n {
            violations.push(format!(
                "{rel}: {func}() の platform 修飾の直読みが凍結表の {n} 件より少ない（{actual} 件）。\
                 直したなら KNOWN_PLATFORM_READS の件数を下げるか行を消す（表を実態へ合わせる。#2844）"
            ));
        }
    }
    // 走査が空振りしていない（列挙が壊れて 0 件で緑になるのを防ぐ）
    assert!(
        reads.len() >= KNOWN_PLATFORM_READS.len(),
        "直読みが {} 件しか見つからない（走査が壊れた？）",
        reads.len()
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 5 の続き: 修飾キーを押しながらのファイルのドロップ（cd / 下のターミナルへの
/// パス入力。#415）と、入力欄の `"v"` の貼り付けは判定の 1 実装を通る（#2844）。
///
/// 規則 5 の凍結表は「直読みが増えた」を止める。こちらは「直読みを消しただけで
/// 判定ごと無くなった / control を素で読む形へ変わった」を止める
#[test]
fn ドロップと貼り付けの修飾は判定の1実装を通る() {
    let mut violations: Vec<String> = Vec::new();
    // ドロップ: 修飾を読む代入は全部 platform_modifier_active を通る
    let v = without_self_test(views(DROP_FILE));
    let mut drop_reads = 0usize;
    for needle in DROP_READS {
        for (at, _) in v.code.match_indices(needle) {
            let end = at
                + v.code[at..]
                    .find(';')
                    .unwrap_or_else(|| panic!("{DROP_FILE}: `{needle}` の文の終わりが無い"));
            let stmt = &v.text[at..end];
            if !stmt.contains("modifiers") {
                continue; // `= false` などの後片付け
            }
            drop_reads += 1;
            if !stmt.contains(&format!("keybindings::{HELPER}(")) {
                violations.push(format!(
                    "{DROP_FILE}:{}: ドロップの修飾の判定が {HELPER} を通っていない: `{}`\
                     （Windows では Ctrl+ドロップが効かない。#2844）",
                    line_of(&v.code, at),
                    stmt.trim()
                ));
            }
        }
    }
    // ドラッグ中の 2 種（ツリー / 外部）・ドロップの 2 種・描画フレームの同期
    if drop_reads < 5 {
        violations.push(format!(
            "{DROP_FILE}: 修飾を読むドロップの判定が {drop_reads} 件しか見つからない（5 件のはず。構造が変わった？）"
        ));
    }
    // 貼り付け: 入力欄の `"v"` は paste_key_active を通る（control を素で読まない）
    for (rel, name, want) in PASTE_HANDLERS {
        let v = views(rel);
        let (start, end) = fn_body(&v, rel, name);
        let mut arms = 0usize;
        for (off, _) in v.text[start..end].match_indices("\"v\"") {
            let at = start + off;
            arms += 1;
            let line_end = at + v.text[at..].find('\n').unwrap_or(v.text.len() - at);
            let line_start = v.text[..at].rfind('\n').map_or(0, |i| i + 1);
            let line = &v.text[line_start..line_end];
            if !line.contains(PASTE_HELPER) {
                violations.push(format!(
                    "{rel}:{}: {name}() の貼り付け（\"v\"）が keybindings::{PASTE_HELPER} を通っていない: `{}`\
                     （Windows では Win+V を見て Ctrl+V を落とす。#2844）",
                    line_of(&v.code, at),
                    line.trim()
                ));
            }
        }
        if arms != *want {
            violations.push(format!(
                "{rel}: {name}() の \"v\" が {arms} 件（{want} 件のはず。構造が変わった？）"
            ));
        }
    }
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

/// 規則 6: `PasteClipboard` のアクションのハンドラは伝播を止めたまま（#2844）。
///
/// GPUI はキーバインドのアクションを `on_key_down` より先に配り、バブルのハンドラは既定で
/// 伝播を止める。入力欄の `"v"` の腕（`paste_key_active`）が ⌘V / Ctrl+V を受けても
/// 二重に貼らないのはこのため（tako-app の `貼り付けはバインド表の経路と二重に発火しない`
/// の前提）。ハンドラが `propagate()` を呼ぶと、同じ打鍵が腕へも落ちて 2 回貼る
#[test]
fn 貼り付けのアクションは伝播を止めたまま() {
    let product = production_range::product_sources();
    let test_only = production_range::test_only_files(&product);
    let mut violations: Vec<String> = Vec::new();
    let mut handlers = 0usize;
    for rel in product
        .iter()
        .filter(|r| r.starts_with("crates/tako-app/src/") && !test_only.contains(*r))
    {
        let src = read(rel);
        if !src.contains("PasteClipboard") {
            continue;
        }
        let text = production_range::scan(&src).text;
        let code = code_view::code_view(&text);
        let needle = ".on_action(";
        for (at, _) in code.match_indices(needle) {
            let open = at + needle.len() - 1;
            let end = balanced_end(&code, open, b'(', b')');
            let args = &code[open..end];
            if !args.contains("PasteClipboard") {
                continue;
            }
            handlers += 1;
            if args.contains("propagate(") {
                violations.push(format!(
                    "{rel}:{}: PasteClipboard のハンドラが propagate() を呼んでいる\
                     （同じ打鍵が入力欄の \"v\" の腕へも落ちて 2 回貼る。#2844）",
                    line_of(&code, at)
                ));
            }
        }
    }
    // メイン窓（paste() へ振り分け）と設定窓（編集欄へ挿入）
    assert!(
        handlers >= 2,
        "PasteClipboard のハンドラが {handlers} 件しか見つからない（構造が変わった？）"
    );
    assert!(violations.is_empty(), "\n{}", violations.join("\n"));
}

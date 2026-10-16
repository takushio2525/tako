//! ネット復帰の即時再開で、締めの行より上が画面の外の停止を取りこぼさない番犬（#2181 / FR-2.27.17）
//!
//! # なぜ要るか
//!
//! 低いペインでは締めの行と通知行と入力欄だけで画面が埋まり、締めの行の上の項目（接続エラー）が
//! 画面の外へ押し出される（2026-10-16 の DNS 断で pane 2716 を取りこぼした）。#2181 は
//! この形のときだけ画面の外の直近の行を足して同じ判定（`net_stop::detect`）を通す。
//! 足す経路は 4 つの約束で安全に保っている。どれも 1 行で外せるのに単体テストでは落ちないので、
//! ここで構造を見張る（判定そのもの = 形の見分け・印の照合は `tako_control::net_stop` の
//! `issue2181_*` が旧判定との A/B で固定している）:
//!
//! 1. 見えている行の判定が先（`read_net_screen` は `detect` が Clear のときだけ
//!    `offscreen_print` を引く）。見えている行で「進んだ」「ダイアログ」と言えるなら画面の外は読まない
//! 2. 器の履歴を読む口（`capture_offscreen`）は**背景のスレッドの中でだけ**呼ぶ
//!    （UI スレッドで子プロセスを待たない = #1979 / #1984 と同じ約束）
//! 3. 覚えた答えは**読みに行ったときの形の印と今の印が同じとき**だけ使う
//!    （`offscreen_screen` / `observe_net_stop` / `take_offscreen_reads` の 3 か所）
//! 4. 読みに行っているあいだ（答えが無い）は追跡を動かさない（`drive_net_autoresume` の
//!    `NetRead::Offscreen` の腕は `continue` するだけで、追跡も送った鍵も消さない）。
//!    送る直前の読み直し（`observe_net_stop`）は読みに行かない（覚えた答えだけを見る）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で、**現行ソースから作り直した注入**が file:line で名指しされる
//! ことを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DRIVER: &str = "crates/tako-app/src/net_autoresume.rs";
const JUDGE: &str = "crates/tako-control/src/net_stop.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置の開き括弧に対応する閉じ括弧の位置
fn matching(code: &str, open: usize) -> Option<usize> {
    let (o, c) = match code[open..].chars().next()? {
        '{' => ('{', '}'),
        '(' => ('(', ')'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (i, ch) in code[open..].char_indices() {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(open + i);
            }
        }
    }
    None
}

/// コードだけの眺めで `fn <name>(` の頭の位置と、本体（波括弧の中）の範囲を返す
fn fn_body(code: &str, name: &str) -> Option<(usize, usize, usize)> {
    let head = format!("fn {name}(");
    let start = code.find(&head)?;
    let open = start + code[start..].find('{')?;
    Some((start, open, matching(code, open)?))
}

/// `needle` の後ろの最初の `{` から対応する `}` までの塊（無ければ空）
fn block_after(code: &str, at: usize) -> &str {
    code[at..]
        .find('{')
        .map(|i| at + i)
        .and_then(|o| matching(code, o).map(|e| &code[o..e]))
        .unwrap_or("")
}

/// ① 見えている行の判定が先: `read_net_screen` で `offscreen_print` を引くのは
/// `detect` の答えが Clear かを見た後
fn check_visible_first(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some((head, open, close)) = fn_body(&code, "read_net_screen") else {
        return vec![format!("{file} — fn read_net_screen が無い")];
    };
    let body = &code[open..close];
    let detect = body.find("net_stop::detect(");
    let clear = body.find("!= NetScreen::Clear");
    let print = body.find("net_stop::offscreen_print(");
    match (detect, clear, print) {
        (Some(d), Some(c), Some(p)) if d < c && c < p => Vec::new(),
        (_, _, None) => vec![format!(
            "{file}:{} — read_net_screen が画面の外の要否（offscreen_print）を見ていない",
            line_of(src, head)
        )],
        (_, _, Some(p)) => vec![format!(
            "{file}:{} — 見えている行の判定（detect が Clear か）より先に画面の外へ回っている",
            line_of(src, open + p)
        )],
    }
}

/// ② 器の履歴を読む口は背景のスレッドの中でだけ呼ぶ
fn check_capture_off_ui(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut v = Vec::new();
    let mut from = 0;
    let mut seen = 0usize;
    while let Some(rel) = code[from..].find("capture_offscreen(") {
        let at = from + rel;
        from = at + 1;
        seen += 1;
        // 呼び出しより前の最後の `.spawn(move ||` の閉じ括弧の内側か
        let inside = code[..at].rfind(".spawn(move ||").is_some_and(|s| {
            let open = s + ".spawn".len();
            matching(&code, open).is_some_and(|end| at < end)
        });
        if !inside {
            v.push(format!(
                "{file}:{} — 器の履歴を読む口（capture_offscreen）を背景のスレッドの外で呼んでいる\
                 （UI スレッドで子プロセスを待つ = #1984）",
                line_of(src, at)
            ));
        }
    }
    if seen == 0 {
        v.push(format!(
            "{file} — 器の履歴を読む口（capture_offscreen）が無い（器のペインで画面の外を読めない）"
        ));
    }
    v
}

/// ③ 覚えた答えは形の印が同じときだけ使う
fn check_print_matched(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut v = Vec::new();
    for name in [
        "offscreen_screen",
        "observe_net_stop",
        "take_offscreen_reads",
    ] {
        let Some((head, open, close)) = fn_body(&code, name) else {
            v.push(format!("{file} — fn {name} が無い"));
            continue;
        };
        let body = &code[open..close];
        // 覚えた答えを引く形（印を `print: p` で取り出すパターン）は必ず印の比較を伴う。
        // 値を作る側（`Offscreen::Read { print, screen }`）は印を取り出さないので数えない
        let mut arms = 0usize;
        for needle in ["Offscreen::Read {", "Offscreen::Reading {"] {
            let mut from = 0;
            while let Some(rel) = body[from..].find(needle) {
                let at = from + rel;
                from = at + 1;
                let fields_open = at + needle.len() - 1;
                let fields = matching(body, fields_open).map_or("", |e| &body[fields_open..e]);
                if !fields.contains("print:") {
                    continue;
                }
                arms += 1;
                let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
                if !body[at..line_end].contains("*p == print") {
                    v.push(format!(
                        "{file}:{} — {name} が覚えた答えを形の印を比べずに使っている",
                        line_of(src, open + at)
                    ));
                }
            }
        }
        if arms == 0 {
            v.push(format!(
                "{file}:{} — {name} が覚えた答え（Offscreen）を見ていない",
                line_of(src, head)
            ));
        }
    }
    v
}

/// ④ 読みに行っているあいだは追跡を動かさない・送る直前の読み直しは読みに行かない
fn check_reading_holds(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut v = Vec::new();
    let Some((head, open, close)) = fn_body(&code, "drive_net_autoresume") else {
        return vec![format!("{file} — fn drive_net_autoresume が無い")];
    };
    let body = &code[open..close];
    match body.find("NetRead::Offscreen(print) =>") {
        None => v.push(format!(
            "{file}:{} — drive_net_autoresume が画面の外の形（NetRead::Offscreen）を扱っていない",
            line_of(src, head)
        )),
        Some(arm) => {
            let at = open + arm;
            let none = code[at..close]
                .find("None =>")
                .map(|i| at + i)
                .unwrap_or(close);
            let block = block_after(&code, none);
            if !block.contains("continue") {
                v.push(format!(
                    "{file}:{} — 読みに行っているあいだにこのペインの判断へ進んでいる",
                    line_of(src, none)
                ));
            }
            for forbidden in ["trackers.remove", "sent_keys.remove"] {
                if block.contains(forbidden) {
                    v.push(format!(
                        "{file}:{} — 読みに行っているあいだに追跡 / 送った鍵を消している（{forbidden}）",
                        line_of(src, none)
                    ));
                }
            }
        }
    }
    match fn_body(&code, "observe_net_stop") {
        None => v.push(format!("{file} — fn observe_net_stop が無い")),
        Some((h, o, c)) => {
            let body = &code[o..c];
            if body.contains("offscreen_screen(") || body.contains("capture_offscreen(") {
                v.push(format!(
                    "{file}:{} — 送る直前の読み直しが画面の外を読みに行っている（覚えた答えだけを見ること）",
                    line_of(src, h)
                ));
            }
        }
    }
    v
}

/// ⑤ 判定側: 足した材料は形の印を照合してから `detect` を通す
fn check_judge_matches_print(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some((head, open, close)) = fn_body(&code, "detect_offscreen") else {
        return vec![format!("{file} — fn detect_offscreen が無い")];
    };
    let body = &code[open..close];
    let anchor = body.find("closing_anchor(lines)");
    let guard = body.find("if p == print => detect(lines)");
    match (anchor, guard) {
        (Some(a), Some(g)) if a < g => Vec::new(),
        _ => vec![format!(
            "{file}:{} — detect_offscreen が形の印を照合せずに判定している",
            line_of(src, head)
        )],
    }
}

fn all(driver: &str, judge: &str) -> Vec<String> {
    let mut v = check_visible_first(DRIVER, driver);
    v.extend(check_capture_off_ui(DRIVER, driver));
    v.extend(check_print_matched(DRIVER, driver));
    v.extend(check_reading_holds(DRIVER, driver));
    v.extend(check_judge_matches_print(JUDGE, judge));
    v
}

#[test]
fn 画面の外を足す経路が4つの約束を守る() {
    let v = all(&read(DRIVER), &read(JUDGE));
    assert!(v.is_empty(), "{v:#?}");
}

fn inject(src: &str, from: &str, to: &str) -> String {
    assert_eq!(src.matches(from).count(), 1, "注入の目印が 1 つ: {from}");
    src.replacen(from, to, 1)
}

/// 注入した逆戻りが file:line で名指しされる（番犬が見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    let driver = read(DRIVER);
    let judge = read(JUDGE);
    let names = |v: &[String], file: &str, what: &str| {
        assert!(
            v.iter().any(|m| m.starts_with(&format!("{file}:"))),
            "{what} を名指せない: {v:?}"
        );
    };

    // ① 見えている行の判定より先に画面の外へ回る
    let injected = inject(
        &driver,
        "        let screen = net_stop::detect(&lines);\n        if screen != NetScreen::Clear {",
        "        if let Some(print) = net_stop::offscreen_print(&lines) {\n            return NetRead::Offscreen(print);\n        }\n        let screen = net_stop::detect(&lines);\n        if screen != NetScreen::Clear {",
    );
    names(
        &check_visible_first(DRIVER, &injected),
        DRIVER,
        "① 画面の外を先に見る注入",
    );

    // ② 器の履歴を UI スレッドで読む
    let injected = inject(
        &driver,
        "        self.net_resume\n            .offscreen\n            .insert(pane, Offscreen::Reading { print });",
        "        let _ = net_stop::capture_offscreen(&session);\n        self.net_resume\n            .offscreen\n            .insert(pane, Offscreen::Reading { print });",
    );
    names(
        &check_capture_off_ui(DRIVER, &injected),
        DRIVER,
        "② UI スレッドで器を読む注入",
    );

    // ③ 送る直前の読み直しが印を比べずに覚えた答えを使う
    let injected = inject(
        &driver,
        "                Some(Offscreen::Read { print: p, screen }) if *p == print => screen.clone(),\n                _ => return None,",
        "                Some(Offscreen::Read { print: _p, screen }) if true => screen.clone(),\n                _ => return None,",
    );
    names(
        &check_print_matched(DRIVER, &injected),
        DRIVER,
        "③ 印を比べない注入",
    );

    // ④ 読みに行っているあいだに追跡を消す
    let injected = inject(
        &driver,
        "                    None => {\n                        reading += 1;\n                        continue;",
        "                    None => {\n                        reading += 1;\n                        self.net_resume.trackers.remove(&pane);\n                        continue;",
    );
    names(
        &check_reading_holds(DRIVER, &injected),
        DRIVER,
        "④ 読みに行っているあいだに追跡を消す注入",
    );
    // ④' 送る直前の読み直しが読みに行く
    let injected = inject(
        &driver,
        "            NetRead::Offscreen(print) => match self.net_resume.offscreen.get(&pane) {\n                Some(Offscreen::Read { print: p, screen }) if *p == print => screen.clone(),",
        "            NetRead::Offscreen(print) => match self.net_resume.offscreen.get(&pane) {\n                _ if { let _ = capture_offscreen(\"\"); false } => unreachable!(),\n                Some(Offscreen::Read { print: p, screen }) if *p == print => screen.clone(),",
    );
    names(
        &check_reading_holds(DRIVER, &injected),
        DRIVER,
        "④' 送る直前に読みに行く注入",
    );

    // ⑤ 判定側が印を照合しない
    let injected = inject(
        &judge,
        "        Some((_, p)) if p == print => detect(lines),",
        "        Some((_, _p)) => detect(lines),",
    );
    names(
        &check_judge_matches_print(JUDGE, &injected),
        JUDGE,
        "⑤ 印を照合しない注入",
    );
}

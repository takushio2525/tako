//! TUI への左クリックの転送（#1961。FR-2.5.15）の番犬
//!
//! # なぜ要るか
//!
//! クリックの転送は「GUI のマウスイベント → 判断 → 届け先」の 3 段で、どれか 1 か所の逆戻りが
//! **見た目では分からない事故**になる:
//!
//! - 器のペインのクリックを外側 PTY へ書くと、書き込みが途中で途切れたとき tmux が ESC を単独の
//!   キーと確定し、`0;12;5M` のような断片が claude の入力欄へ平文で入る（#167 と同じ機序）
//! - 器への問い合わせ・注入（tmux の子プロセス）を UI スレッドで待つと、器が詰まった瞬間に GUI が
//!   固まる（#1979 の本番ハングと同じ型）
//! - 離すを取りこぼす・レート制限で捨てると、内側アプリは押しっぱなしだと思い込む
//!   （以後のホバーがドラッグになる）
//! - 判断を `tako_core::mouse_forward` の外で組むと、GUI と AI（`tako click`）で挙動が割れる
//!
//! 単体テストと e2e は「今の形で動く」ことしか見ないので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. `click_forward.rs` で器を起こす呼び出し（`mf::send_tmux(` / `mf::tmux_mouse_flags(` /
//!    `source.resolve()`）は `background_executor()` の spawn の中だけ
//! 2. 器のペインを扱う関数（`emit_tmux` / `pump_click` / `replay_forward`）は外側 PTY へ書かない。
//!    `emit` は器の分岐（`if tmux {` … `return;`）を PTY への書き込みより先に置く
//! 3. 離す（`end_click_forward`）はレート制限（`budget`）を通さない
//! 4. 押したとき（`on_pane_mouse_down`）は、フォーカスを移す前に `focus_change` を測り、
//!    tako の選択（`start_selection(`）より先に転送の判断（`begin_click_forward(`）を通す
//! 5. 離したとき（`on_mouse_up`）は、ほかの状態で `return` する前に `end_click_forward(` を通す。
//!    離すが届かないまま戻ってきた（ボタンなしの移動）ときも `end_click_forward(None` を通す
//! 6. 判断は `mf::route(` / `mf::decide(` / `mf::bypass_modifier(` の 1 実装（修飾を直に読まない）。
//!    A/B の env（`TAKO_1961_LEGACY`）を文字列で読むのは `tako_core::mouse_forward` だけ
//! 7. AI のクリックの器の経路は `prepare_offload` が background へ回す（`Request::PaneClick`）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const CLICK: &str = "crates/tako-app/src/click_forward.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const CORE: &str = "crates/tako-core/src/mouse_forward.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
/// A/B の env の名前を直に読んではいけない側（[`CORE`] の外）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const LEGACY_ENV: &str = "\"TAKO_1961_LEGACY\"";

/// 器を起こす呼び出し（右は名指しの語）
const TMUX_CALLS: &[&str] = &[
    "mf::send_tmux(",
    "mf::tmux_mouse_flags(",
    "source.resolve()",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、`close` で始まる最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(close))
        .map(|i| start + 1 + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の行の番号（1 始まり）
fn line_in(w: &(usize, String), needle: &str) -> Option<usize> {
    w.1.lines()
        .position(|l| l.contains(needle))
        .map(|i| w.0 + i)
}

/// `click_forward.rs` の `impl` の中の関数の窓（4 字下げの `fn` から 4 字下げの `}` まで）
fn method(src: &str, name: &str) -> Option<(usize, String)> {
    window(src, &format!("fn {name}("), "    }")
}

struct Sources {
    click: String,
    main: String,
    dispatch: String,
    /// [`SOURCE_DIRS`] の .rs（相対パス, 中身）
    others: Vec<(String, String)>,
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn current() -> Sources {
    let root = workspace_root();
    let mut others = Vec::new();
    for dir in SOURCE_DIRS {
        let mut files = Vec::new();
        rs_files(&root.join(dir), &mut files);
        for path in files {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if let Ok(body) = std::fs::read_to_string(&path) {
                others.push((rel, body));
            }
        }
    }
    Sources {
        click: read(CLICK),
        main: read(MAIN),
        dispatch: read(DISPATCH),
        others,
    }
}

/// 1. 器を起こす呼び出しは background の spawn の中だけ
fn scan_background(click: &str) -> Vec<String> {
    let lines: Vec<&str> = click.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(call) = TMUX_CALLS.iter().find(|c| line.contains(*c)) else {
            continue;
        };
        if line.trim_start().starts_with("//") {
            continue;
        }
        // 直近の「spawn の入口」か「関数の頭」まで遡る。関数の頭が先なら spawn の外
        // 関数の頭は共有ヘルパで見る（`pub(crate) fn` / `async fn` も頭 = #1496）
        let anchor = (0..=i).rev().find(|&j| {
            lines[j].contains(".spawn(async move {")
                || tako_core::source_scan::fn_head_name(lines[j]).is_some()
        });
        let inside = anchor.is_some_and(|j| {
            lines[j].contains(".spawn(async move {")
                && (j.saturating_sub(2)..=j).any(|k| lines[k].contains("background_executor()"))
        });
        if !inside {
            out.push(report(
                CLICK,
                i + 1,
                &format!(
                    "器を UI スレッドで起こしている（background_executor の spawn の外で `{call}`。\
                     器が詰まると GUI が固まる = #1979 の型）"
                ),
            ));
        }
    }
    out
}

/// 2. 器のペインのクリックを外側 PTY へ書かない
fn scan_pty(click: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["emit_tmux", "pump_click", "replay_forward"] {
        let Some(w) = method(click, name) else {
            out.push(report(CLICK, 1, &format!("`fn {name}(` が見つからない")));
            continue;
        };
        for needle in ["write_click_report(", ".write("] {
            if let Some(line) = line_in(&w, needle) {
                out.push(report(
                    CLICK,
                    line,
                    &format!(
                        "器のペインのクリックを外側 PTY へ書いている（`{name}` の `{needle}`。\
                         途切れると断片が入力欄へ平文で入る = #167 の型）"
                    ),
                ));
            }
        }
    }
    match method(click, "emit") {
        Some(w) => {
            let branch = line_in(&w, "if tmux {");
            let ret = line_in(&w, "return;");
            let write = line_in(&w, "write_click_report(");
            let ordered =
                matches!((branch, ret, write), (Some(b), Some(r), Some(wr)) if b < r && r < wr);
            if !ordered {
                out.push(report(
                    CLICK,
                    write.or(branch).unwrap_or(w.0),
                    "`emit` が器の分岐（`if tmux {` … `return;`）より先に PTY へ書いている",
                ));
            }
        }
        None => out.push(report(CLICK, 1, "`fn emit(` が見つからない")),
    }
    out
}

/// 3. 離すはレート制限を通さない
fn scan_release(click: &str) -> Vec<String> {
    let Some(w) = method(click, "end_click_forward") else {
        return vec![report(CLICK, 1, "`fn end_click_forward(` が見つからない")];
    };
    let mut out = Vec::new();
    if let Some(line) = line_in(&w, "budget") {
        out.push(report(
            CLICK,
            line,
            "離すをレート制限（budget）に通している（捨てると内側アプリが押しっぱなしだと思い込む）",
        ));
    }
    if line_in(&w, "ClickPhase::Release").is_none() {
        out.push(report(
            CLICK,
            w.0,
            "`end_click_forward` が離す（ClickPhase::Release）を渡していない",
        ));
    }
    out
}

/// 4 / 5. GUI のマウスイベントの配線順
fn scan_handlers(main: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(main, "    fn on_pane_mouse_down(", "    }") {
        Some(w) => {
            let measure = line_in(&w, "let focus_change =");
            let focus = line_in(&w, ".focus(pane_id)");
            if !matches!((measure, focus), (Some(m), Some(f)) if m < f) {
                out.push(report(
                    MAIN,
                    focus.or(measure).unwrap_or(w.0),
                    "フォーカスを移した後で focus_change を測っている（フォーカスを移すクリックを TUI へ渡してしまう）",
                ));
            }
            let begin = line_in(&w, "begin_click_forward(");
            let select = line_in(&w, "start_selection(");
            if !matches!((begin, select), (Some(b), Some(s)) if b < s) {
                out.push(report(
                    MAIN,
                    select.or(begin).unwrap_or(w.0),
                    "tako の選択（start_selection）を転送の判断（begin_click_forward）より先に始めている",
                ));
            }
        }
        None => out.push(report(MAIN, 1, "`fn on_pane_mouse_down(` が見つからない")),
    }
    match window(
        main,
        "    fn on_mouse_up(&mut self, event: &MouseUpEvent",
        "    }",
    ) {
        Some(w) => {
            let end = line_in(&w, "end_click_forward(");
            let first_return =
                w.1.lines()
                    .enumerate()
                    .find(|(_, l)| l.contains("return") && !l.trim_start().starts_with("//"))
                    .map(|(i, _)| w.0 + i);
            let ok = match (end, first_return) {
                (Some(e), Some(r)) => e < r,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if !ok {
                out.push(report(
                    MAIN,
                    first_return.or(end).unwrap_or(w.0),
                    "離すの転送（end_click_forward）より前に return がある（離すを取りこぼす）",
                ));
            }
        }
        None => out.push(report(MAIN, 1, "`fn on_mouse_up(` が見つからない")),
    }
    match window(
        main,
        "        if event.pressed_button != Some(MouseButton::Left) {",
        "            return;",
    ) {
        Some(w) => {
            if line_in(&w, "end_click_forward(None").is_none() {
                out.push(report(
                    MAIN,
                    w.0,
                    "離すが届かないまま戻ってきたとき（ボタンなしの移動）に TUI へ離すを渡していない",
                ));
            }
        }
        None => out.push(report(
            MAIN,
            1,
            "ボタンなしの移動の後始末の窓が見つからない",
        )),
    }
    out
}

/// 6. 判断は core の 1 実装・A/B の env は core だけ
fn scan_route(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    match method(&s.click, "begin_click_forward") {
        Some(w) => {
            for needle in ["mf::route(", "mf::decide(", "mf::bypass_modifier("] {
                if line_in(&w, needle).is_none() {
                    out.push(report(
                        CLICK,
                        w.0,
                        &format!("判断を tako_core::mouse_forward の外で組んでいる（`{needle}` を通っていない）"),
                    ));
                }
            }
            for (i, line) in w.1.lines().enumerate() {
                let raw_mod = ["modifiers.alt", "modifiers.shift"]
                    .iter()
                    .any(|m| line.contains(m));
                if raw_mod && !line.contains("mf::bypass_modifier(") {
                    out.push(report(
                        CLICK,
                        w.0 + i,
                        "修飾を直に読んでいる（tako 側の選択へ切り替える修飾は mf::bypass_modifier の 1 実装）",
                    ));
                }
            }
        }
        None => out.push(report(CLICK, 1, "`fn begin_click_forward(` が見つからない")),
    }
    for (rel, body) in &s.others {
        for (i, line) in body.lines().enumerate() {
            if line.contains(LEGACY_ENV) {
                out.push(report(
                    rel,
                    i + 1,
                    "A/B の env（TAKO_1961_LEGACY）を tako_core::mouse_forward の外で読んでいる",
                ));
            }
        }
    }
    out
}

/// 7. AI のクリックの器の経路は background へ
fn scan_offload(dispatch: &str) -> Vec<String> {
    match window(dispatch, "pub fn prepare_offload(", "}") {
        Some(w) => {
            if line_in(&w, "Request::PaneClick {").is_none() {
                vec![report(
                    DISPATCH,
                    w.0,
                    "器のペインへの AI のクリックを UI スレッドで送っている（prepare_offload に PaneClick が無い）",
                )]
            } else {
                Vec::new()
            }
        }
        None => vec![report(
            DISPATCH,
            1,
            "`pub fn prepare_offload(` が見つからない",
        )],
    }
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_background(&s.click);
    out.extend(scan_pty(&s.click));
    out.extend(scan_release(&s.click));
    out.extend(scan_handlers(&s.main));
    out.extend(scan_route(s));
    out.extend(scan_offload(&s.dispatch));
    out
}

#[test]
fn クリックの転送の構造が契約どおり() {
    let found = all(&current());
    assert!(
        found.is_empty(),
        "契約から外れた箇所:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    let hits = s
        .click
        .lines()
        .filter(|l| TMUX_CALLS.iter().any(|c| l.contains(c)))
        .count();
    assert!(hits >= 3, "器を起こす呼び出しを {hits} 件しか拾えていない");
    for name in [
        "emit_tmux",
        "pump_click",
        "replay_forward",
        "emit",
        "end_click_forward",
    ] {
        let (_, w) = method(&s.click, name).unwrap_or_else(|| panic!("fn {name} の窓"));
        assert!(w.lines().count() > 5, "{name} の窓が短すぎる: {w}");
    }
    let (_, w) = window(&s.main, "    fn on_pane_mouse_down(", "    }").unwrap();
    assert!(w.contains("start_selection("), "窓が短すぎる: {w}");
    let (_, w) = window(&s.dispatch, "pub fn prepare_offload(", "}").unwrap();
    assert!(w.contains("Request::TmuxSelectWindow"), "窓が短すぎる");
    assert!(
        s.others.len() > 50,
        "走査した .rs が {} 本しかない",
        s.others.len()
    );
    // A/B の env は core に 1 か所だけある（ゲートの在処）
    assert!(read(CORE).contains(LEGACY_ENV));
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

fn replaced(src: &str, from: &str, to: &str) -> String {
    assert!(src.contains(from), "注入の足場が現行ソースに無い: {from}");
    src.replacen(from, to, 1)
}

#[test]
fn 逆戻りを名指しできる() {
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. 器への問い合わせを UI スレッドで待つ
    let found = mutate(&|s| {
        s.click = replaced(
            &s.click,
            "        self.click_forward = Some(ClickForward::Resolving {\n            pane,\n            events: vec![held],",
            "        let _flags = mf::tmux_mouse_flags(&source.clone().resolve());\n        self.click_forward = Some(ClickForward::Resolving {\n            pane,\n            events: vec![held],",
        );
    });
    assert_named(&found, CLICK, "器を UI スレッドで起こしている");
    // 2. 器のペインのクリックを外側 PTY へ書く / emit の分岐を後ろへ
    let found = mutate(&|s| {
        s.click = replaced(
            &s.click,
            "        let pump = self.click_pumps.entry(pane).or_default();",
            "        if let Some(session) = self.terminals.get(&pane) {\n            session.write_click_report(bytes.clone());\n        }\n        let pump = self.click_pumps.entry(pane).or_default();",
        );
    });
    assert_named(&found, CLICK, "外側 PTY へ書いている");
    let found = mutate(&|s| {
        s.click = replaced(
            &s.click,
            "        if tmux {\n            if self.emit_tmux(pane, report, phase, cell, ctrl) {",
            "        if let Some(session) = self.terminals.get(&pane) {\n            session.write_click_report(Vec::new());\n        }\n        if tmux {\n            if self.emit_tmux(pane, report, phase, cell, ctrl) {",
        );
    });
    assert_named(&found, CLICK, "器の分岐");
    // 3. 離すをレート制限に通す
    let found = mutate(&|s| {
        s.click = replaced(
            &s.click,
            "                // 離すは捨てない（捨てると内側アプリが押しっぱなしだと思い込む）\n",
            "                if !MotionBudget::default().take(std::time::Instant::now()) { let _budget = (); }\n",
        );
    });
    assert_named(&found, CLICK, "離すをレート制限");
    // 4. 選択を転送の判断より先に始める / フォーカスを移した後で測る
    let found = mutate(&|s| {
        s.main = replaced(
            &s.main,
            "            if self.begin_click_forward(pane_id, event, (col, row, _right), focus_change, cx) {",
            "            if let Some(s) = self.terminals.get(&pane_id) { s.start_selection(SelectionKind::Simple, col, row, _right); }\n            if self.begin_click_forward(pane_id, event, (col, row, _right), focus_change, cx) {",
        );
    });
    assert_named(&found, MAIN, "start_selection");
    let found = mutate(&|s| {
        s.main = replaced(
            &s.main,
            "        let focus_change = self.workspace.active_tab().tree().focused() != pane_id;\n        let _ = self.workspace.active_tab_mut().tree_mut().focus(pane_id);",
            "        let _ = self.workspace.active_tab_mut().tree_mut().focus(pane_id);\n        let focus_change = self.workspace.active_tab().tree().focused() != pane_id;",
        );
    });
    assert_named(&found, MAIN, "focus_change");
    // 5. 離すの転送より前に return / ボタンなしの移動で離すを渡さない
    let found = mutate(&|s| {
        s.main = replaced(
            &s.main,
            "        // #1961: TUI へ渡していたクリックの離す（ほかの状態より先に見る = 離すを取りこぼさない）\n",
            "        if self.drag_kind.is_some() {\n            return;\n        }\n",
        );
    });
    assert_named(&found, MAIN, "より前に return がある");
    let found = mutate(&|s| {
        s.main = replaced(
            &s.main,
            "            self.end_click_forward(None, cx);\n",
            "",
        );
    });
    assert_named(&found, MAIN, "離すが届かないまま戻ってきたとき");
    // 6. 修飾を直に読む / A/B の env を core の外で読む
    let found = mutate(&|s| {
        s.click = replaced(
            &s.click,
            "mf::bypass_modifier(event.modifiers.alt, event.modifiers.shift)",
            "event.modifiers.alt",
        );
    });
    assert_named(&found, CLICK, "mf::bypass_modifier");
    let found = mutate(&|s| {
        s.others.push((
            CLICK.into(),
            "fn legacy() -> bool { std::env::var(\"TAKO_1961_LEGACY\").is_ok() }".into(),
        ));
    });
    assert_named(&found, CLICK, "TAKO_1961_LEGACY");
    // 7. AI のクリックを UI スレッドで送る
    let found = mutate(&|s| {
        s.dispatch = replaced(
            &s.dispatch,
            "        Request::PaneClick { pane, row, col } => match pane_click_plan(host, *pane) {",
            "        Request::Noop if false => match pane_click_plan(host, None) {",
        );
    });
    assert_named(&found, DISPATCH, "prepare_offload に PaneClick が無い");
}

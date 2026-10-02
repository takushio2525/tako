//! 定義ジャンプの着地の規則の番犬（#1680）
//!
//! # なぜ要るか
//!
//! 着地の規則（同じファイル = 同じペイン / 同じタブで開いている = そのペインを使い回す /
//! それ以外 = 新しいペイン）は **1 行で壊せる**: 腕の `Some(Direction::Right)` を `None` に
//! すれば「別のファイルでも同じペインを差し替える」（新しいペインを開かない）、
//! `Landing::Reuse(pane) => (pane, …)` の `pane` を問い合わせたペインにすれば
//! 「2 回飛ぶとペインが 2 枚になる」（使い回さない）。どちらも CLI の応答の形は変わらず、
//! GUI で並べて初めて分かる。挙動は dispatch の単体（MockHost）とセルフテストが測り、
//! ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 規則の正本は `tako_core::lsp::goto::plan_landing` の 1 つで、同じファイル → 使い回し →
//!    新しいペインの順に決める
//! 2. dispatch の `lsp_goto_land` はその答えを**そのまま**開き方へ写す（新しいペインは
//!    向きつきの分割 / 新しいタブ、使い回しは答えのペイン、同じファイルは問い合わせたペイン）
//! 3. 開くのは `open_file`（行指定の open と同じ入口 = ジャンプ履歴へ積む）で、起点は
//!    問い合わせたペイン（`JumpFrom::Pane`）
//! 4. `open_file` は行を指定して同じファイルを開くときに読み直さない（編集セッションを捨てない）
//! 5. GUI（`lsp_goto_ui.rs`）は着地を自前で持たない（`prepare_offload` → `finish_offload_on_ui`
//!    = CLI / MCP と同じ 1 本。設計原則 5）。⌘クリックの枝は修飾つきのときだけ入る
//!    （修飾なしのクリックの選択を変えない）
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

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const GOTO: &str = "crates/tako-core/src/lsp/goto.rs";
const GUI: &str = "crates/tako-app/src/lsp_goto_ui.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug, PartialEq, Eq)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の行（1-based。無ければ `None`）
fn line_in(window_start: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| window_start + i)
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_only(window: &str) -> String {
    window
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 腕 `pattern =>` の行と、その右辺（行の残り）
fn arm(start: usize, window: &str, pattern: &str) -> Option<(usize, String)> {
    let needle = format!("{pattern} =>");
    window
        .lines()
        .enumerate()
        .find(|(_, l)| l.trim_start().starts_with(&needle))
        .map(|(i, l)| {
            (
                start + i,
                l.split("=>").nth(1).unwrap_or("").trim().to_string(),
            )
        })
}

/// 1〜4（dispatch と tako-core）の検査
fn scan_rules(dispatch: &str, goto: &str) -> Vec<Offender> {
    let mut out = Vec::new();

    // 1. 規則の正本
    match fn_window(goto, "pub fn plan_landing(") {
        None => out.push(Offender {
            file: GOTO,
            line: 0,
            why: "`plan_landing` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            let same = code.find("return Landing::SamePane");
            let reuse = code.find("return Landing::Reuse(");
            match (same, reuse) {
                (None, _) => out.push(Offender {
                    file: GOTO,
                    line: at,
                    why: "同じファイルを同じペインへ返していない（`Landing::SamePane` が無い）"
                        .into(),
                }),
                (_, None) => out.push(Offender {
                    file: GOTO,
                    line: at,
                    why: "開いているペインを使い回していない（`Landing::Reuse` が無い = \
                          同じ定義へ 2 回飛ぶとペインが 2 枚になる）"
                        .into(),
                }),
                (Some(same), Some(reuse)) if reuse < same => out.push(Offender {
                    file: GOTO,
                    line: line_in(at, &window, "return Landing::Reuse(").unwrap_or(at),
                    why:
                        "使い回しを同じファイルより先に見ている（同じファイルでも別のペインへ飛ぶ）"
                            .into(),
                }),
                _ => {}
            }
            if !code.contains("open_in_tab") || !code.contains("same_file(path, target)") {
                out.push(Offender {
                    file: GOTO,
                    line: at,
                    why: "使い回しの判定が同じタブのプレビューと飛び先を突き合わせていない".into(),
                });
            }
        }
    }

    // 2・3. dispatch の写し方と開く入口
    let Some((at, window)) = fn_window(dispatch, "pub fn lsp_goto_land(") else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`lsp_goto_land` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    let code = code_only(&window);
    if !code.contains("plan_landing(") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "着地を規則の正本（`plan_landing`）で決めていない".into(),
        });
    }
    let expect = [
        (
            "Landing::SamePane",
            "(landing.source_pane, None, false)",
            "同じファイルを問い合わせたペインのまま開いていない",
        ),
        (
            "Landing::Reuse(pane)",
            "(pane, None, false)",
            "開いているペインを使い回していない（答えのペインではない所へ開く = 2 回飛ぶと 2 枚になる）",
        ),
        (
            "Landing::New(NewPane::Right)",
            "(landing.source_pane, Some(Direction::Right), false)",
            "別のファイルを新しいペインで開いていない（右への分割が無い = 同じペインを差し替える）",
        ),
        (
            "Landing::New(NewPane::Down)",
            "(landing.source_pane, Some(Direction::Down), false)",
            "別のファイルを新しいペインで開いていない（下への分割が無い）",
        ),
        (
            "Landing::New(NewPane::Tab)",
            "(landing.source_pane, None, true)",
            "新しいタブで開いていない",
        ),
    ];
    for (pattern, rhs, why) in expect {
        match arm(at, &window, pattern) {
            None => out.push(Offender {
                file: DISPATCH,
                line: at,
                why: format!("腕 `{pattern}` が無い（{why}）"),
            }),
            Some((line, got)) if got.trim_end_matches(',') != rhs => out.push(Offender {
                file: DISPATCH,
                line,
                why: format!("{why}: `{got}`（期待 `{rhs}`）"),
            }),
            _ => {}
        }
    }
    match line_in(at, &window, "let opened = open_file(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "開く入口が `open_file` ではない（行指定の open と別の経路 = 履歴へ積まれない）"
                .into(),
        }),
        Some(line) => {
            if !code.contains("JumpRecord::Record") {
                out.push(Offender {
                    file: DISPATCH,
                    line,
                    why: "ジャンプ履歴へ積んでいない（`JumpRecord::Record` が無い）".into(),
                });
            }
            if !code.contains("jump_from: JumpFrom::Pane(landing.source_pane)") {
                out.push(Offender {
                    file: DISPATCH,
                    line,
                    why: "飛ぶ前の場所を問い合わせたペインから採っていない（戻るが着地先の古い位置へ帰る）"
                        .into(),
                });
            }
        }
    }

    // 4. 同じファイルは読み直さない
    match fn_window(dispatch, "fn open_file(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`open_file` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            let guarded = code.contains("if !same_document {")
                && code
                    .split("if !same_document {")
                    .nth(1)
                    .is_some_and(|rest| rest.trim_start().starts_with("host.set_preview("));
            if !guarded {
                out.push(Offender {
                    file: DISPATCH,
                    line: line_in(at, &window, "host.set_preview(").unwrap_or(at),
                    why: "行を指定して同じファイルを開いても読み直している（編集セッションを捨てる = \
                          同じファイル内の定義ジャンプで編集モードが抜ける）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 5（GUI）の検査
fn scan_gui(gui: &str, render: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (index, line) in gui.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        for bad in [
            "Request::OpenFile",
            "open_file(",
            "set_preview(",
            "split_with_ratio(",
        ] {
            if code.contains(bad) {
                out.push(Offender {
                    file: GUI,
                    line: index + 1,
                    why: format!(
                        "GUI が着地を自前で行っている（`{bad}`）。dispatch の 1 本（finish_offload）を通す"
                    ),
                });
            }
        }
    }
    // ⌘クリック（`start_lsp_goto`）と右クリックメニューの定義ジャンプ（#1684）は共有の入口
    // `start_lsp_goto_request` を通り、そこが CLI / MCP と同じ準備を通る
    match fn_window(gui, "pub(crate) fn start_lsp_goto(") {
        None => out.push(Offender {
            file: GUI,
            line: 0,
            why: "`start_lsp_goto` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window)
                .contains("self.start_lsp_goto_request(pane, request, anchor, cx)")
            {
                out.push(Offender {
                    file: GUI,
                    line: at,
                    why: "⌘クリックが共有の入口（start_lsp_goto_request）を通っていない".into(),
                });
            }
        }
    }
    match fn_window(gui, "pub(crate) fn start_lsp_goto_request(") {
        None => out.push(Offender {
            file: GUI,
            line: 0,
            why: "`start_lsp_goto_request` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window).contains("tako_control::prepare_offload(self, &request)") {
                out.push(Offender {
                    file: GUI,
                    line: at,
                    why: "⌘クリックが CLI / MCP と同じ準備（prepare_offload）を通っていない".into(),
                });
            }
        }
    }
    match fn_window(gui, "fn land_lsp_goto(") {
        None => out.push(Offender {
            file: GUI,
            line: 0,
            why: "`land_lsp_goto` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window).contains("self.finish_offload_on_ui(next, PaneOrigin::User, cx)")
            {
                out.push(Offender {
                    file: GUI,
                    line: at,
                    why: "着地が IPC の続きと同じ 1 本（finish_offload_on_ui）を通っていない"
                        .into(),
                });
            }
        }
    }
    // ⌘クリックの枝は修飾つきのときだけ入り、選択より前で抜ける
    let lines: Vec<&str> = render.lines().collect();
    match lines.iter().position(|l| {
        l.contains("this.start_lsp_goto(pane_id, line, range.start, ev.position, cx);")
    }) {
        None => out.push(Offender {
            file: RENDER,
            line: 0,
            why: "プレビューの ⌘クリックから定義ジャンプを始めていない（走査が空振り）".into(),
        }),
        Some(at) => {
            let guard = lines[at.saturating_sub(8)..at]
                .iter()
                .any(|l| l.contains("crate::keybindings::link_modifier_active(&ev.modifiers)"));
            if !guard {
                out.push(Offender {
                    file: RENDER,
                    line: at + 1,
                    why: "定義ジャンプの枝が修飾キーで守られていない（修飾なしのクリックの選択を奪う）".into(),
                });
            }
            let returns = lines.get(at + 1).is_some_and(|l| l.trim() == "return;");
            if !returns {
                out.push(Offender {
                    file: RENDER,
                    line: at + 1,
                    why: "定義ジャンプを始めたあと選択へ落ちている（⌘クリックで選択も動く）".into(),
                });
            }
        }
    }
    out
}

fn all(dispatch: &str, goto: &str, gui: &str, render: &str) -> Vec<String> {
    scan_rules(dispatch, goto)
        .iter()
        .chain(scan_gui(gui, render).iter())
        .map(Offender::report)
        .collect()
}

fn current() -> (String, String, String, String) {
    (read(DISPATCH), read(GOTO), read(GUI), read(RENDER))
}

#[test]
fn 定義ジャンプの着地は規則の正本どおりに開く() {
    let (dispatch, goto, gui, render) = current();
    let offenders = all(&dispatch, &goto, &gui, &render);
    assert!(
        offenders.is_empty(),
        "定義ジャンプの着地の規則が崩れている（#1680）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let (dispatch, goto, gui, _) = current();
    assert!(fn_window(&goto, "pub fn plan_landing(").is_some());
    let (at, window) = fn_window(&dispatch, "pub fn lsp_goto_land(").unwrap();
    for pattern in [
        "Landing::SamePane",
        "Landing::Reuse(pane)",
        "Landing::New(NewPane::Right)",
        "Landing::New(NewPane::Down)",
        "Landing::New(NewPane::Tab)",
    ] {
        assert!(
            arm(at, &window, pattern).is_some(),
            "腕 {pattern} が採れない"
        );
    }
    assert!(fn_window(&dispatch, "fn open_file(").is_some());
    assert!(fn_window(&gui, "pub(crate) fn start_lsp_goto(").is_some());
    assert!(fn_window(&gui, "pub(crate) fn start_lsp_goto_request(").is_some());
    assert!(fn_window(&gui, "fn land_lsp_goto(").is_some());
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること。
/// `needle` は `関数の宣言>>行の字面` と書くと、その関数の中で最初に出る行を指す
/// （同じ字面が別の関数にもあるとき）
fn assert_named(
    label: &str,
    src: &str,
    file: &str,
    (from, to): (&str, &str),
    needle: &str,
    scan: impl Fn(&str) -> Vec<String>,
) {
    assert!(
        src.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    let injected = src.replacen(from, to, 1);
    let (after, needle) = needle.split_once(">>").unwrap_or(("", needle));
    let lines: Vec<&str> = injected.lines().collect();
    let begin = if after.is_empty() {
        0
    } else {
        lines
            .iter()
            .position(|l| l.contains(after))
            .unwrap_or_else(|| panic!("{label}: 起点 {after:?} が無い"))
    };
    let line = lines[begin..]
        .iter()
        .position(|l| l.contains(needle))
        .map(|i| begin + i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&injected);
    let expected = format!("{file}:{line} ");
    assert!(
        reports.iter().any(|r| r.starts_with(&expected)),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let (dispatch, goto, gui, render) = current();
    let rules_d = |d: &str| all(d, &goto, &gui, &render);
    let rules_g = |g: &str| all(&dispatch, g, &gui, &render);
    let gui_s = |g: &str| all(&dispatch, &goto, g, &render);
    let render_s = |r: &str| all(&dispatch, &goto, &gui, r);

    // A. 新しいペインを開かない（別のファイルでも問い合わせたペインを差し替える）
    assert_named(
        "A 新ペインを開かない",
        &dispatch,
        DISPATCH,
        (
            "Landing::New(NewPane::Right) => (landing.source_pane, Some(Direction::Right), false)",
            "Landing::New(NewPane::Right) => (landing.source_pane, None, false)",
        ),
        "Landing::New(NewPane::Right) =>",
        rules_d,
    );
    // B. 使い回さない（開いているペインがあっても新しく分割する）
    assert_named(
        "B 使い回さない",
        &dispatch,
        DISPATCH,
        (
            "Landing::Reuse(pane) => (pane, None, false)",
            "Landing::Reuse(pane) => (landing.source_pane, Some(Direction::Right), false)",
        ),
        "Landing::Reuse(pane) =>",
        rules_d,
    );
    // C. 規則の正本から使い回しの枝が消える
    assert_named(
        "C 正本が使い回さない",
        &goto,
        GOTO,
        (
            "        return Landing::Reuse(*pane);",
            "        let _ = pane;",
        ),
        "pub fn plan_landing(",
        rules_g,
    );
    // D. 同じファイルでも読み直す
    assert_named(
        "D 読み直す",
        &dispatch,
        DISPATCH,
        (
            "    if !same_document {\n        host.set_preview(",
            "    if !same_document || true {\n        host.set_preview(",
        ),
        "fn open_file(>>host.set_preview(view_pane",
        rules_d,
    );
    // E. 起点が着地先になる（戻るが問い合わせたペインへ帰らない）
    assert_named(
        "E 起点が着地先",
        &dispatch,
        DISPATCH,
        (
            "jump_from: JumpFrom::Pane(landing.source_pane)",
            "jump_from: JumpFrom::Auto",
        ),
        "pub fn lsp_goto_land(>>let opened = open_file(",
        rules_d,
    );
    // F. GUI が自前で開く
    assert_named(
        "F GUI が自前で開く",
        &gui,
        GUI,
        (
            "        let job = match tako_control::prepare_offload(self, &request) {",
            "        let _ = Request::OpenFile;\n        let job = match tako_control::prepare_offload(self, &request) {",
        ),
        "Request::OpenFile;",
        gui_s,
    );
    // G. 修飾なしのクリックでも定義ジャンプへ入る
    assert_named(
        "G 修飾なしでも入る",
        &render,
        RENDER,
        (
            "if crate::keybindings::link_modifier_active(&ev.modifiers)\n                                && ev.click_count == 1\n                            {\n                                if let Some((line, range)) =",
            "if ev.click_count == 1\n                            {\n                                if let Some((line, range)) =",
        ),
        "this.start_lsp_goto(pane_id, line, range.start, ev.position, cx);",
        render_s,
    );
}

//! ファイルツリーのインライン入力（新規ファイル / 新規フォルダ / 名前を変更）の
//! フォーカスと IME の扱いを縛る番犬（#1725）
//!
//! # なぜ要るか
//!
//! #1725 の実測: 入力欄の打鍵（ASCII）は全部入力欄に入っていたのに、**IME の変換だけが
//! 隣のターミナルペインに束縛**されていた（`app_text_input` がインライン編集を宛先から
//! 明示的に外していた）。GPUI は日本語など非 ASCII の入力ソースが有効なとき印字キーを
//! `on_key_down` より先に IME へ渡すので、かなモードでは 1 打鍵目から変換が始まり、
//! 下線も候補窓もターミナルのカーソル位置に出て、未確定のまま確定（unmark）した文字列は
//! ターミナルの PTY へ流れていた = 「日本語が入らない / カーソルがターミナルに座る」。
//!
//! 同じ型（#561 のコミット欄）は直っていたのに、入力欄ごとに挿入・宛先・描画を
//! 手書きしていたので**1 本だけ取り残された**。どれも**動いては見える**（ASCII は入る）ので
//! 目視では気づけない。
//!
//! # 何を縛るか
//!
//! 1. **IME の宛先** — `app_text_input` は見えているインライン入力を最優先で
//!    `AppTextInput::TreeName` にする（旧形の「インライン編集なら None」を持たない）
//! 2. **挿入は 1 関数** — 打鍵・⌘V・IME の確定・unmark の 4 経路がすべて
//!    `insert_app_text_input(AppTextInput::TreeName, …)` → `tree_name_insert` を通り、
//!    `TextField` の挿入はそこ 1 か所だけ
//! 3. **手書きのカーソル演算を持たない** — 状態は `TextField`、編集は `handle_edit_key`
//! 4. **開く / 閉じるが 1 本ずつ** — `inline_edit = Some(` は `open_inline_edit` だけ、
//!    `inline_edit = None` / `.take()` は `close_inline_edit` だけ。閉じる出口が
//!    この入力欄宛ての変換を捨て、元のペインへ打鍵を戻す
//! 5. **描画** — 未確定文字列とキャレットを共有部品（`text_input_marked_at` /
//!    `text_input_caret`）で入力欄の中に描き、外側の押下（`on_mouse_down_out`）で閉じる
//! 6. **見えない入力欄は打鍵を奪わない** — `handle_key` と `text_input_swallows_keys` が
//!    `inline_edit_visible()` で判定する
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が
//! 採れていることを固定し、[`逆戻りを名指しできる`] で**注入 13 通り**が `file:line` で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す
//! （`main.rs` は本番コードと隔離セルフテストが交互に並ぶ）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";

/// 手書きのカーソル操作のしるし（`TextField` を通していれば 1 つも要らない）
const HANDROLLED_INPUT_MARKS: &[&str] = &[
    "floor_char_boundary",
    ".drain(",
    "char_indices()",
    "insert_str(",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（#1420 の 1 実装。**切らずにテスト領域だけを潰す**）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
}

#[derive(Debug)]
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

/// 関数の窓（1-based の宣言行・終わりの行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, usize, String)> {
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
    Some((start + 1, end + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` `//!` `///`）の行を空へ落とした眺め（行数は保つ）。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// コードの行だけを走査して `needle` を含む最初の行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

struct Sources {
    main: String,
    sidebar: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: prod(root, MAIN),
            sidebar: prod(root, SIDEBAR),
        }
    }

    fn get(&self, file: &'static str) -> &str {
        match file {
            MAIN => &self.main,
            _ => &self.sidebar,
        }
    }
}

/// `needle` の関数が `must` をすべてコードとして含むか（無ければ宣言行で名指す）
fn require_in_fn(
    sources: &Sources,
    file: &'static str,
    needle: &str,
    must: &[(&str, &str)],
    out: &mut Vec<Offender>,
) {
    match fn_window(sources.get(file), needle) {
        None => out.push(Offender {
            file,
            line: 0,
            why: format!("`{needle}` が見つからない（走査が空振り）"),
        }),
        Some((at, _, window)) => {
            let body = code_only(&window);
            for (mark, why) in must {
                if !body.contains(mark) {
                    out.push(Offender {
                        file,
                        line: at,
                        why: format!("`{needle}` に `{mark}` が無い: {why}"),
                    });
                }
            }
        }
    }
}

/// 1: IME の宛先。見えているインライン入力が最優先で `TreeName` になる
fn scan_ime_target(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(&sources.main, "fn app_text_input(&self)") {
        None => out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`app_text_input` が見つからない（走査が空振り）".into(),
        }),
        Some((at, _, window)) => {
            let body = code_only(&window);
            // 旧形: 「インライン編集なら None（= ターミナルペインへ束縛）」
            if let Some(i) = code_line_with(&body, "self.inline_edit.is_some()") {
                out.push(Offender {
                    file: MAIN,
                    line: at + i,
                    why: "`app_text_input` がインライン編集を `is_some()` で見ている\
                          （修正前の「インライン編集なら None = 変換をターミナルペインへ束縛」\
                          の形。下線・候補窓・unmark がターミナルへ出る = #1725）"
                        .into(),
                });
            }
            let tree = code_line_with(&body, "return Some(AppTextInput::TreeName)");
            let visible = code_line_with(&body, "self.inline_edit_visible()");
            let branch = code_line_with(&body, "AppTextInput::GitBranch");
            match (visible, tree) {
                (Some(v), Some(t)) if v < t => {
                    // 打鍵の振り分け（handle_key）と同じく、他の入力欄より先に見る
                    if branch.is_some_and(|b| b < t) {
                        out.push(Offender {
                            file: MAIN,
                            line: at + t,
                            why: "`TreeName` が他の入力欄より後ろで決まっている（打鍵は\
                                  インライン入力が最優先で握るので、変換の宛先も同じ順でないと\
                                  打鍵と変換の行き先が割れる。#1725）"
                                .into(),
                        });
                    }
                }
                _ => out.push(Offender {
                    file: MAIN,
                    line: at,
                    why: "`app_text_input` が見えているインライン入力を \
                          `AppTextInput::TreeName` にしていない（かなモードの変換が\
                          隣のターミナルペインに束縛される = #1725 の真因）"
                        .into(),
                }),
            }
        }
    }
    require_in_fn(
        sources,
        MAIN,
        "fn insert_app_text_input(",
        &[(
            "AppTextInput::TreeName => self.tree_name_insert(",
            "IME の確定・unmark の文字列がインライン入力へ入らない（#1725）",
        )],
        &mut out,
    );
    out
}

/// 2: 4 経路の挿入が 1 関数を通り、`TextField` の挿入はそこ 1 か所だけ
fn scan_single_insert(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    const ROUTE: &str = "insert_app_text_input(AppTextInput::TreeName";
    for (file, needle, what) in [
        (MAIN, "    fn paste(&mut self", "⌘V"),
        (
            MAIN,
            "    fn replace_text_in_range(",
            "IME の確定（insertText）",
        ),
        (SIDEBAR, "fn handle_inline_edit_key(", "打鍵"),
    ] {
        require_in_fn(
            sources,
            file,
            needle,
            &[(
                ROUTE,
                &format!("{what}の文字列が共有の挿入を通らない（経路ごとに挿入を書くと、片方だけ直した取りこぼしが残る。#1725）"),
            )],
            &mut out,
        );
    }
    // unmark は変換開始時に決めた宛先（`ime.app_input`）へ入れる = 宛先の決め方が 1 つ
    require_in_fn(
        sources,
        MAIN,
        "    fn unmark_text(&mut self",
        &[(
            "self.insert_app_text_input(target",
            "未確定のまま確定した文字列が宛先の入力欄へ入らない（ターミナルへ流れる = #1725）",
        )],
        &mut out,
    );
    // `TextField` への挿入は `tree_name_insert` の中だけ
    let allowed = fn_window(&sources.sidebar, "fn tree_name_insert(");
    if allowed.is_none() {
        out.push(Offender {
            file: SIDEBAR,
            line: 0,
            why: "`tree_name_insert` が見つからない（走査が空振り）".into(),
        });
    }
    for file in [MAIN, SIDEBAR] {
        let code = code_only(sources.get(file));
        for (i, line) in code.lines().enumerate() {
            let n = i + 1;
            let inside = file == SIDEBAR
                && allowed
                    .as_ref()
                    .is_some_and(|(s, e, _)| (*s..=*e).contains(&n));
            if line.contains("edit.field.insert(") && !inside {
                out.push(Offender {
                    file,
                    line: n,
                    why: "インライン入力の `TextField` へ `tree_name_insert` の外から挿入している\
                          （挿入の入口が 2 つに割れる。#1725）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 3: 手書きのカーソル演算を持たない（状態は `TextField`、編集は `handle_edit_key`）
fn scan_no_handrolled(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(&sources.main, "struct InlineEdit {") {
        None => out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`struct InlineEdit` が見つからない（走査が空振り）".into(),
        }),
        Some((at, _, window)) => {
            let body = code_only(&window);
            if !body.contains("field: crate::text_field::TextField") {
                out.push(Offender {
                    file: MAIN,
                    line: at,
                    why: "`InlineEdit` の状態が `TextField` ではない（手書きの本文 + カーソルへ\
                          戻ると、境界処理が入力欄ごとに割れる。#1459 / #1725）"
                        .into(),
                });
            }
            for mark in ["cursor: usize", "text: String"] {
                if let Some(i) = code_line_with(&body, mark) {
                    out.push(Offender {
                        file: MAIN,
                        line: at + i,
                        why: format!("`InlineEdit` に手書きの `{mark}` が居る（#1725）"),
                    });
                }
            }
        }
    }
    for needle in [
        "fn handle_inline_edit_key(",
        "fn tree_name_insert(",
        "fn open_inline_edit(",
        "fn close_inline_edit(",
        "fn commit_inline_edit(",
    ] {
        let Some((at, _, window)) = fn_window(&sources.sidebar, needle) else {
            out.push(Offender {
                file: SIDEBAR,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            });
            continue;
        };
        let body = code_only(&window);
        for mark in HANDROLLED_INPUT_MARKS {
            if let Some(i) = code_line_with(&body, mark) {
                out.push(Offender {
                    file: SIDEBAR,
                    line: at + i,
                    why: format!(
                        "`{needle}` に手書きのカーソル操作（`{mark}`）が居る。編集は \
                         `TextField` を通す（#1459 / #1725）"
                    ),
                });
            }
        }
    }
    require_in_fn(
        sources,
        SIDEBAR,
        "fn handle_inline_edit_key(",
        &[(
            "field.handle_edit_key(",
            "編集キーを `TextField` へ委ねていない（#1459 / #1725）",
        )],
        &mut out,
    );
    out
}

/// 4: 開く / 閉じるが 1 本ずつ。閉じる出口が変換を捨て、元のペインへ戻す
fn scan_single_open_close(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let open = fn_window(&sources.sidebar, "fn open_inline_edit(");
    let close = fn_window(&sources.sidebar, "fn close_inline_edit(");
    for (name, w) in [("open_inline_edit", &open), ("close_inline_edit", &close)] {
        if w.is_none() {
            out.push(Offender {
                file: SIDEBAR,
                line: 0,
                why: format!("`{name}` が見つからない（走査が空振り）"),
            });
        }
    }
    let within = |w: &Option<(usize, usize, String)>, file: &str, n: usize| {
        file == SIDEBAR && w.as_ref().is_some_and(|(s, e, _)| (*s..=*e).contains(&n))
    };
    for file in [MAIN, SIDEBAR] {
        let code = code_only(sources.get(file));
        for (i, line) in code.lines().enumerate() {
            let n = i + 1;
            if line.contains("inline_edit = Some(") && !within(&open, file, n) {
                out.push(Offender {
                    file,
                    line: n,
                    why: "`open_inline_edit` の外で入力欄を開いている（入口ごとに\
                          フォーカスの戻り先・他の入力欄の後始末が割れる。#1725）"
                        .into(),
                });
            }
            if (line.contains("inline_edit = None") || line.contains("inline_edit.take()"))
                && !within(&close, file, n)
            {
                out.push(Offender {
                    file,
                    line: n,
                    why: "`close_inline_edit` の外で入力欄を閉じている（この入力欄宛ての\
                          変換が残る / 元のペインへ戻らない。#1725）"
                        .into(),
                });
            }
        }
    }
    require_in_fn(
        sources,
        SIDEBAR,
        "fn close_inline_edit(",
        &[
            (
                "AppTextInput::TreeName",
                "閉じた入力欄宛ての変換を捨てていない（続く確定の宛先がずれる。#1725）",
            ),
            (
                ".focus(edit.origin_pane)",
                "閉じたとき元のペインへ打鍵を戻していない（#1725）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        SIDEBAR,
        "fn open_inline_edit(",
        &[
            (
                "origin_pane: self.focused_pane()",
                "開いたときのペインを控えていない（Esc で戻る先が無い。#1725）",
            ),
            (
                "self.clear_text_input_focus()",
                "他の入力欄を畳んでいない（閉じた瞬間に古いフラグが打鍵を拾い直す = #503 の型）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        SIDEBAR,
        "pub(crate) fn handle_context_action(",
        &[(
            "self.open_inline_edit(kind, path)",
            "右クリックメニューが開く入口を通っていない（#1725）",
        )],
        &mut out,
    );
    out
}

/// 5: 入力欄の中に未確定文字列とキャレットを描き、外側の押下で閉じる
fn scan_render(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        SIDEBAR,
        "pub(crate) fn render_sidebar(",
        &[
            (
                "text_input_marked_at(AppTextInput::TreeName",
                "未確定文字列を入力欄の中に描いていない（変換中の読みがどこにも見えない。#1725）",
            ),
            (
                "text_input_caret(AppTextInput::TreeName",
                "キャレットを共有部品で描いていない（候補窓の位置出しに実矩形が渡らない。#1725）",
            ),
            (
                ".on_mouse_down_out(",
                "入力欄の外を押しても閉じない（押したターミナルへ打鍵が戻らない。#1725）",
            ),
            (
                "this.close_inline_edit(false)",
                "外側の押下が閉じる出口を通っていない（#1725）",
            ),
        ],
        &mut out,
    );
    out
}

/// 6: 見えない入力欄は打鍵を奪わない
fn scan_visibility(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        MAIN,
        "    fn handle_key(&mut self, keystroke: &Keystroke",
        &[
            (
                "!self.inline_edit_visible()",
                "見えていない入力欄が打鍵を握り続ける（押しても何も起きない。#1725）",
            ),
            (
                "self.close_inline_edit(false)",
                "見えていない入力欄を畳まずに打鍵を流している（#1725）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn text_input_swallows_keys(&self)",
        &[(
            "self.inline_edit_visible()",
            "⌘V の振り分けが入力欄の見え方と割れている（#1725）",
        )],
        &mut out,
    );
    require_in_fn(
        sources,
        SIDEBAR,
        "pub(crate) fn inline_edit_visible(&self)",
        &[(
            "inline_edit_target_visible(",
            "可視判定が純粋関数（単体テストで固定したもの）を通っていない（#1725）",
        )],
        &mut out,
    );
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_ime_target(sources));
    out.extend(scan_single_insert(sources));
    out.extend(scan_no_handrolled(sources));
    out.extend(scan_single_open_close(sources));
    out.extend(scan_render(sources));
    out.extend(scan_visibility(sources));
    out
}

fn report(offenders: &[Offender]) -> String {
    offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn インライン入力のフォーカスとimeの構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1725 の構造が崩れている:\n{}",
        report(&offenders)
    );
}

/// 走査が空振りしていれば上のテストは無意味に緑になるので、窓が採れていることを固定する
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let sources = Sources::load(&root);
    for (file, needle) in [
        (MAIN, "fn app_text_input(&self)"),
        (MAIN, "fn insert_app_text_input("),
        (MAIN, "    fn paste(&mut self"),
        (MAIN, "    fn replace_text_in_range("),
        (MAIN, "    fn unmark_text(&mut self"),
        (MAIN, "    fn handle_key(&mut self, keystroke: &Keystroke"),
        (MAIN, "fn text_input_swallows_keys(&self)"),
        (MAIN, "struct InlineEdit {"),
        (SIDEBAR, "fn handle_inline_edit_key("),
        (SIDEBAR, "fn tree_name_insert("),
        (SIDEBAR, "fn open_inline_edit("),
        (SIDEBAR, "fn close_inline_edit("),
        (SIDEBAR, "fn commit_inline_edit("),
        (SIDEBAR, "pub(crate) fn handle_context_action("),
        (SIDEBAR, "pub(crate) fn render_sidebar("),
        (SIDEBAR, "pub(crate) fn inline_edit_visible(&self)"),
    ] {
        let (at, end, window) = fn_window(sources.get(file), needle)
            .unwrap_or_else(|| panic!("{file} の `{needle}` の窓が採れない = 走査が壊れている"));
        assert!(
            at > 0 && end > at,
            "{file} の `{needle}` の行範囲が壊れている"
        );
        assert!(
            window.lines().count() > 3,
            "{file} の `{needle}` の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    // 範囲取りが本文を残していること（潰れた眺めを渡していない）
    assert!(
        sources.sidebar.lines().count() > 2000,
        "sidebar.rs の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        sources.sidebar.lines().count()
    );
}

/// 注入が `file:line` で名指しされること（行番号 0 = 空振りの名指しは数えない）
fn expect_hit(sources: &Sources, file: &str, needle: &str) {
    let offenders = scan_all(sources);
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.line > 0 && o.why.contains(needle)),
        "注入が {file} の `{needle}` として名指しされない。実際:\n{}",
        report(&offenders)
    );
}

/// 窓の中だけを置き換える（同じ綴りが別の関数にあっても注入が漏れないように）
fn replace_in_fn(src: &str, needle: &str, from: &str, to: &str) -> String {
    let (_, _, window) = fn_window(src, needle).expect("注入先の窓");
    assert!(
        window.contains(from),
        "注入先 `{needle}` に `{from}` が無い"
    );
    src.replace(&window, &window.replacen(from, to, 1))
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 真因の逆戻り: インライン編集を IME の宛先から外す（修正前の形）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "        if self.inline_edit_visible() {",
        "        if self.inline_edit.is_some() || self.webview_dock_url_focused {\n            return None;\n        }\n        if false {",
    );
    expect_hit(&s, MAIN, "`is_some()` で見ている");

    // (2) 宛先の枝ごと消える
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "return Some(AppTextInput::TreeName);",
        "return None;",
    );
    expect_hit(&s, MAIN, "`AppTextInput::TreeName` にしていない");

    // (3) IME の確定がインライン入力へ入らない
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn insert_app_text_input(",
        "AppTextInput::TreeName => self.tree_name_insert(text, cx),",
        "AppTextInput::TreeName => {}",
    );
    expect_hit(&s, MAIN, "tree_name_insert(");

    // (4) ⌘V が自前で挿入する（経路が割れる）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn paste(&mut self",
        "self.insert_app_text_input(AppTextInput::TreeName, &text, cx);",
        "if let Some(edit) = self.inline_edit.as_mut() { let _ = edit.field.insert(&text, 1024, false); }",
    );
    expect_hit(&s, MAIN, "⌘Vの文字列が共有の挿入を通らない");
    expect_hit(&s, MAIN, "`tree_name_insert` の外から挿入している");

    // (5) unmark の文字列が宛先へ入らない（ターミナルへ流れる）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn unmark_text(&mut self",
        "self.insert_app_text_input(target, &ime.text, cx);",
        "let _ = target;",
    );
    expect_hit(&s, MAIN, "未確定のまま確定した文字列");

    // (6) 手書きのカーソル演算へ戻る
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "fn tree_name_insert(",
        "if let Some(edit) = self.inline_edit.as_mut() {",
        "if let Some(edit) = self.inline_edit.as_mut() {\n            let mut raw = String::new();\n            raw.insert_str(0, text);",
    );
    expect_hit(&s, SIDEBAR, "手書きのカーソル操作（`insert_str(`）");

    // (7) 状態が手書きの本文 + カーソルへ戻る
    let mut s = Sources::load(&root);
    s.main = s.main.replace(
        "    field: crate::text_field::TextField,\n",
        "    text: String,\n    cursor: usize,\n",
    );
    expect_hit(&s, MAIN, "手書きの `cursor: usize`");

    // (8) 入口の外で開く（メニューから直に組む）
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "pub(crate) fn handle_context_action(",
        "self.open_inline_edit(kind, path);",
        "self.inline_edit = Some(InlineEdit { parent: path.to_path_buf(), kind, field: Default::default(), origin_pane: self.focused_pane() });",
    );
    expect_hit(&s, SIDEBAR, "`open_inline_edit` の外で入力欄を開いている");
    expect_hit(&s, SIDEBAR, "右クリックメニューが開く入口を通っていない");

    // (9) 出口の外で閉じる（Esc が直に None を書く）
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "fn handle_inline_edit_key(",
        "self.close_inline_edit(true);",
        "self.inline_edit = None;",
    );
    expect_hit(&s, SIDEBAR, "`close_inline_edit` の外で入力欄を閉じている");

    // (10) 閉じても元のペインへ戻らない
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "fn close_inline_edit(",
        ".focus(edit.origin_pane);",
        ".focus(self.focused_pane());",
    );
    expect_hit(&s, SIDEBAR, "元のペインへ打鍵を戻していない");

    // (11) 外側の押下で閉じない
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "pub(crate) fn render_sidebar(",
        ".on_mouse_down_out(",
        ".on_mouse_down_ignored(",
    );
    expect_hit(&s, SIDEBAR, "入力欄の外を押しても閉じない");

    // (12) 未確定文字列を入力欄に描かない
    let mut s = Sources::load(&root);
    s.sidebar = replace_in_fn(
        &s.sidebar,
        "pub(crate) fn render_sidebar(",
        "self.text_input_marked_at(AppTextInput::TreeName, &theme, 12.0)",
        "None::<gpui::Div>",
    );
    expect_hit(&s, SIDEBAR, "未確定文字列を入力欄の中に描いていない");

    // (13) 見えない入力欄が打鍵を握り続ける
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn handle_key(&mut self, keystroke: &Keystroke",
        "if self.inline_edit.is_some() && !self.inline_edit_visible() {",
        "if false {",
    );
    expect_hit(&s, MAIN, "見えていない入力欄が打鍵を握り続ける");
}

//! ⌘K パレット・Web ビューのアドレスバー・Web dock の URL 欄の IME の扱いを縛る番犬（#1750）
//!
//! # なぜ要るか
//!
//! #1750 の実測（修正前の main）: 3 つの入力欄とも ASCII の打鍵と ⌘V は入力欄に入っていたのに、
//! **IME の変換だけがターミナルペインに束縛**されていた（`app_text_input` の宛先に 3 つとも
//! 無く、dock の URL 欄は「有効な間はペイン束縛のまま」と明示的に外されていた）。GPUI は
//! かなモードの印字キーを `on_key_down` より先に IME へ渡すので、1 打鍵目から下線も候補窓も
//! ターミナルのカーソル位置に出て、確定（パレット / アドレスバー）と unmark（3 つとも）の
//! 文字列はターミナルの PTY へ流れた。Web ペインにフォーカスがあるとアドレスバーの確定は
//! どこにも入らず消え、git のコミット欄にフォーカスが残ったままパレットを開くと変換は
//! パレットの裏のコミット欄へ入った。#1725（ファイルツリーのインライン入力）と同じ型で、
//! 入力欄ごとに挿入・宛先・描画を手書きしていたので**3 本取り残されていた**。
//!
//! # 何を縛るか
//!
//! 1. **IME の宛先** — `app_text_input` は開いているパレットを**最優先**で `Palette` にし
//!    （打鍵の振り分け `handle_key` と同じ順）、見えているアドレスバー / dock の URL 欄を
//!    `WebAddress` / `WebDockUrl` にする（旧形の「dock の URL 欄なら None」を持たない）
//! 2. **挿入は 1 関数ずつ** — 打鍵・⌘V・IME の確定・unmark の 4 経路がすべて
//!    `insert_app_text_input(AppTextInput::…)` → `palette_insert` / `web_address_insert` /
//!    `web_dock_url_insert` を通り、状態への書き込みはそこ 1 か所だけ
//! 3. **手書きのカーソル演算を持たない** — Web の 2 欄の状態は `TextField`、編集は `handle_edit_key`
//! 4. **開く / 閉じるが 1 本ずつ** — 閉じる出口がその欄宛ての変換を捨て、開く入口が他の
//!    入力欄を先に畳む（押下が伝播を止めるので、ルートの一括クリアが走らない）
//! 5. **描画** — 未確定文字列とキャレットを共有部品（`text_input_marked_at` /
//!    `text_input_caret`）で入力欄の中に描く。アドレスバーは長い URL でもキャレット側を
//!    残して詰める（`inline_input_window`）
//! 6. **見えない入力欄は打鍵も変換も奪わない** — `handle_key` と `paste` と `app_text_input` が
//!    同じ可視判定（`web_address_bar_editing` / `web_dock_url_active`）を使う
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が
//! 採れていることを固定し、[`逆戻りを名指しできる`] で**注入 15 通り**が `file:line` で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す
//! （`main.rs` は本番コードと隔離セルフテストが交互に並ぶ）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const OVERLAYS: &str = "crates/tako-app/src/overlays.rs";
const STATUS_BAR: &str = "crates/tako-app/src/status_bar.rs";

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

/// 関数の窓（1-based の宣言行・終わりの行・本文）
type Window = (usize, usize, String);

/// 関数の窓（1-based の宣言行・終わりの行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<Window> {
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
    overlays: String,
    status_bar: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: prod(root, MAIN),
            overlays: prod(root, OVERLAYS),
            status_bar: prod(root, STATUS_BAR),
        }
    }

    fn get(&self, file: &'static str) -> &str {
        match file {
            MAIN => &self.main,
            OVERLAYS => &self.overlays,
            _ => &self.status_bar,
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

/// `needle` の窓の中で `mark` を含むコード行を `file:line` で名指す
fn forbid_in_fn(
    sources: &Sources,
    file: &'static str,
    needle: &str,
    mark: &str,
    why: &str,
    out: &mut Vec<Offender>,
) {
    match fn_window(sources.get(file), needle) {
        None => out.push(Offender {
            file,
            line: 0,
            why: format!("`{needle}` が見つからない（走査が空振り）"),
        }),
        Some((at, _, window)) => {
            if let Some(i) = code_line_with(&code_only(&window), mark) {
                out.push(Offender {
                    file,
                    line: at + i,
                    why: format!("`{needle}` に `{mark}` が居る: {why}"),
                });
            }
        }
    }
}

/// `main.rs` の隔離セルフテスト（`mod self_test`）の行範囲。
///
/// セルフテストは `cfg(test)` ではない（本番バイナリに入り `TAKO_SELF_TEST` で走る）ので
/// 本番の眺めに残る。fixture として状態を直接立てる（項目 71 / 78 等）のは検証の作法なので、
/// 「状態への書き込みはこの 1 関数だけ」の走査からは外す
fn self_test_range(sources: &Sources) -> Option<(usize, usize)> {
    fn_window(&sources.main, "mod self_test {").map(|(s, e, _)| (s, e))
}

/// `files` の本番コードで `mark` を含む行のうち、`allowed`（file, 関数の needle）の窓の外に
/// あるものを名指す（「状態への書き込みはこの 1 関数だけ」の検査。セルフテストは除く）
fn only_inside(
    sources: &Sources,
    files: &[&'static str],
    mark: &str,
    allowed: &[(&'static str, &str)],
    why: &str,
    out: &mut Vec<Offender>,
) {
    let windows: Vec<(&'static str, Option<Window>)> = allowed
        .iter()
        .map(|(file, needle)| (*file, fn_window(sources.get(file), needle)))
        .collect();
    for (file, w) in &windows {
        if w.is_none() {
            out.push(Offender {
                file,
                line: 0,
                why: format!("許可された窓が見つからない（走査が空振り）: `{mark}`"),
            });
        }
    }
    let self_test = self_test_range(sources);
    if self_test.is_none() {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`mod self_test` が見つからない（走査が空振り）".into(),
        });
    }
    for file in files {
        let code = code_only(sources.get(file));
        for (i, line) in code.lines().enumerate() {
            let n = i + 1;
            if !line.contains(mark) {
                continue;
            }
            if *file == MAIN && self_test.is_some_and(|(s, e)| (s..=e).contains(&n)) {
                continue;
            }
            let inside = windows.iter().any(|(f, w)| {
                f == file && w.as_ref().is_some_and(|(s, e, _)| (*s..=*e).contains(&n))
            });
            if !inside {
                out.push(Offender {
                    file,
                    line: n,
                    why: format!("`{mark}` が許可された関数の外に居る: {why}"),
                });
            }
        }
    }
}

/// 1: IME の宛先。パレットが最優先、Web の 2 欄は可視判定を通す
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
            let palette = code_line_with(&body, "return Some(AppTextInput::Palette)");
            let palette_cond = code_line_with(&body, "self.command_palette.is_some()");
            match (palette_cond, palette) {
                (Some(c), Some(p)) if c < p => {
                    // 打鍵の振り分け（handle_key）と同じく、パレットは他の入力欄より先に見る
                    for other in [
                        "AppTextInput::TreeName",
                        "AppTextInput::GitBranch",
                        "AppTextInput::GitCommit",
                        "AppTextInput::TaskComment",
                    ] {
                        if code_line_with(&body, other).is_some_and(|o| o < p) {
                            out.push(Offender {
                                file: MAIN,
                                line: at + p,
                                why: format!(
                                    "`Palette` が `{other}` より後ろで決まっている（パレットは\
                                     開いている間は全キーを握るモーダルなので、変換の宛先も\
                                     最優先でないと打鍵と変換の行き先が割れる = パレットの裏の\
                                     欄が変換を握る。#1750）"
                                ),
                            });
                        }
                    }
                }
                _ => out.push(Offender {
                    file: MAIN,
                    line: at,
                    why: "`app_text_input` が開いているパレットを `AppTextInput::Palette` に\
                          していない（かなモードの変換がターミナルペインに束縛される = #1750 の真因）"
                        .into(),
                }),
            }
            for (cond, ret, what) in [
                (
                    "self.web_address_bar_editing().is_some()",
                    "return Some(AppTextInput::WebAddress)",
                    "Web のアドレスバー",
                ),
                (
                    "self.web_dock_url_active()",
                    "return Some(AppTextInput::WebDockUrl)",
                    "Web dock の URL 欄",
                ),
            ] {
                match (code_line_with(&body, cond), code_line_with(&body, ret)) {
                    (Some(c), Some(r)) if c < r => {}
                    _ => out.push(Offender {
                        file: MAIN,
                        line: at,
                        why: format!(
                            "`app_text_input` が{what}を可視判定 `{cond}` で宛先 `{ret}` に\
                             していない（変換がターミナルへ流れる / 見えない欄が変換を奪う。#1750）"
                        ),
                    }),
                }
            }
            // 旧形: 「dock の URL 欄が有効な間はペイン束縛（None）」。A/B の腕だけに残してよい
            for (i, line) in body.lines().enumerate() {
                if line.contains("self.webview_dock_url_focused") && !line.contains("legacy_1750") {
                    out.push(Offender {
                        file: MAIN,
                        line: at + i,
                        why: "`app_text_input` が dock の URL 欄を生のフラグで見ている\
                              （修正前の「URL 欄なら None = 変換をターミナルペインへ束縛」の形。\
                              下線・候補窓・unmark がターミナルへ出る = #1750）"
                            .into(),
                    });
                }
            }
        }
    }
    require_in_fn(
        sources,
        MAIN,
        "fn insert_app_text_input(",
        &[
            (
                "AppTextInput::Palette => self.palette_insert(",
                "IME の確定・unmark の文字列がパレットへ入らない（#1750）",
            ),
            (
                "AppTextInput::WebAddress => self.web_address_insert(",
                "IME の確定・unmark の文字列がアドレスバーへ入らない（#1750）",
            ),
            (
                "AppTextInput::WebDockUrl => self.web_dock_url_insert(",
                "IME の確定・unmark の文字列が dock の URL 欄へ入らない（#1750）",
            ),
        ],
        &mut out,
    );
    out
}

/// 2: 4 経路の挿入が 1 関数ずつを通り、状態への書き込みはそこだけ
fn scan_single_insert(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    // ⌘V
    require_in_fn(
        sources,
        MAIN,
        "    fn paste(&mut self",
        &[
            (
                "insert_app_text_input(AppTextInput::Palette",
                "⌘Vの文字列がパレットの共有の挿入を通らない（#1750）",
            ),
            (
                "insert_app_text_input(AppTextInput::WebAddress",
                "⌘Vの文字列がアドレスバーの共有の挿入を通らない（#1750）",
            ),
            (
                "insert_app_text_input(AppTextInput::WebDockUrl",
                "⌘Vの文字列が dock の URL 欄の共有の挿入を通らない（#1750）",
            ),
        ],
        &mut out,
    );
    // IME の確定（変換を経ない直接の insertText を含む）
    require_in_fn(
        sources,
        MAIN,
        "    fn replace_text_in_range(",
        &[
            (
                "insert_app_text_input(AppTextInput::Palette",
                "パレットが開いている間の確定文字列がパレットへ入らない（ターミナルへ流れる = #1750）",
            ),
            (
                "Some(AppTextInput::WebAddress)",
                "アドレスバー編集中の直接の確定文字列がアドレスバーへ入らない（#1750）",
            ),
            (
                "Some(AppTextInput::WebDockUrl)",
                "dock の URL 欄の直接の確定文字列が欄へ入らない（#1750）",
            ),
        ],
        &mut out,
    );
    // 打鍵（`key_char` の経路。空白キーの補いだけ残って本体が外れる形も名指す）
    for (needle, route, what) in [
        (
            "fn handle_palette_key(",
            "insert_app_text_input(AppTextInput::Palette, ch, cx)",
            "パレット",
        ),
        (
            "fn handle_webview_address_bar_key(",
            "insert_app_text_input(AppTextInput::WebAddress, ch, cx)",
            "アドレスバー",
        ),
        (
            "fn handle_webview_dock_url_key(",
            "insert_app_text_input(AppTextInput::WebDockUrl, ch, cx)",
            "dock の URL 欄",
        ),
    ] {
        require_in_fn(
            sources,
            MAIN,
            needle,
            &[(
                route,
                &format!("{what}の打鍵が共有の挿入を通らない（経路ごとに挿入を書くと、片方だけ直した取りこぼしが残る。#1750）"),
            )],
            &mut out,
        );
    }
    // 状態への書き込みはそれぞれの挿入関数の中だけ
    only_inside(
        sources,
        &[MAIN, OVERLAYS],
        ".query.push_str(",
        &[(MAIN, "fn palette_insert(")],
        "パレットの検索語へ `palette_insert` の外から挿入している（挿入の入口が割れる。#1750）",
        &mut out,
    );
    only_inside(
        sources,
        &[MAIN, OVERLAYS, STATUS_BAR],
        "webview_dock_url.insert(",
        &[(MAIN, "fn web_dock_url_insert(")],
        "dock の URL 欄へ `web_dock_url_insert` の外から挿入している（#1750）",
        &mut out,
    );
    only_inside(
        sources,
        &[MAIN],
        "field.insert(text, usize::MAX",
        &[(MAIN, "fn web_address_insert(")],
        "アドレスバーへ `web_address_insert` の外から挿入している（#1750）",
        &mut out,
    );
    out
}

/// 3: Web の 2 欄は `TextField`。手書きのカーソル演算を持たない
fn scan_no_handrolled(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(&sources.main, "struct TakoApp {") {
        None => out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`struct TakoApp` が見つからない（走査が空振り）".into(),
        }),
        Some((at, _, window)) => {
            let body = code_only(&window);
            for (must, why) in [
                (
                    "webview_dock_url: crate::text_field::TextField,",
                    "dock の URL 欄の状態が `TextField` ではない（#1459 / #1750）",
                ),
                (
                    "webview_address_bar: HashMap<u64, crate::text_field::TextField>,",
                    "アドレスバーの状態が `TextField` ではない（#1459 / #1750）",
                ),
            ] {
                if !body.contains(must) {
                    out.push(Offender {
                        file: MAIN,
                        line: at,
                        why: why.into(),
                    });
                }
            }
            for mark in [
                "webview_dock_url_input:",
                "webview_dock_url_cursor:",
                "HashMap<u64, (String, usize)>",
            ] {
                if let Some(i) = code_line_with(&body, mark) {
                    out.push(Offender {
                        file: MAIN,
                        line: at + i,
                        why: format!(
                            "手書きの本文 + カーソル（`{mark}`）が居る。境界処理が入力欄ごとに\
                             割れる（#1459 / #1750）"
                        ),
                    });
                }
            }
        }
    }
    for needle in [
        "fn handle_webview_address_bar_key(",
        "fn handle_webview_dock_url_key(",
        "fn web_address_insert(",
        "fn web_dock_url_insert(",
        "fn open_web_address_bar(",
        "fn render_webview_address_bar(",
        "fn render_webview_dock_url_input(",
    ] {
        for mark in HANDROLLED_INPUT_MARKS {
            forbid_in_fn(
                sources,
                MAIN,
                needle,
                mark,
                "手書きのカーソル操作。編集は `TextField` を通す（#1459 / #1750）",
                &mut out,
            );
        }
    }
    for needle in [
        "fn handle_webview_address_bar_key(",
        "fn handle_webview_dock_url_key(",
    ] {
        require_in_fn(
            sources,
            MAIN,
            needle,
            &[(
                ".handle_edit_key(",
                "編集キーを `TextField` へ委ねていない（#1459 / #1750）",
            )],
            &mut out,
        );
    }
    out
}

/// 4: 開く / 閉じるが 1 本ずつ。閉じる出口が変換を捨て、開く入口が他の欄を畳む
fn scan_single_open_close(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let all = [MAIN, OVERLAYS, STATUS_BAR];
    only_inside(
        sources,
        &all,
        "command_palette = None",
        &[(MAIN, "fn close_command_palette(")],
        "`close_command_palette` の外でパレットを閉じている（パレット宛ての変換が残る。#1750）",
        &mut out,
    );
    only_inside(
        sources,
        &all,
        "webview_address_bar_active = Some(",
        &[(MAIN, "fn open_web_address_bar(")],
        "`open_web_address_bar` の外でアドレスバーの編集を始めている（他の欄が変換を握ったまま\
         になる = #503 の型。#1750）",
        &mut out,
    );
    for mark in [
        "webview_address_bar_active.take()",
        "webview_address_bar_active = None",
    ] {
        only_inside(
            sources,
            &all,
            mark,
            &[(MAIN, "fn close_web_address_bar(")],
            "`close_web_address_bar` の外でアドレスバーを閉じている（アドレスバー宛ての変換が\
             残る。#1750）",
            &mut out,
        );
    }
    only_inside(
        sources,
        &all,
        "webview_dock_url_focused = true",
        &[(MAIN, "fn focus_web_dock_url(")],
        "`focus_web_dock_url` の外で URL 欄へフォーカスしている（#1750）",
        &mut out,
    );
    for mark in [
        "webview_dock_url_focused = false",
        "webview_dock_url_focused = this",
        "webview_dock_url_focused = self",
    ] {
        only_inside(
            sources,
            &all,
            mark,
            &[(MAIN, "fn blur_web_dock_url(")],
            "`blur_web_dock_url` の外で URL 欄のフォーカスを外している（欄宛ての変換が残る。#1750）",
            &mut out,
        );
    }
    for (needle, target) in [
        ("fn close_command_palette(", "AppTextInput::Palette"),
        ("fn close_web_address_bar(", "AppTextInput::WebAddress"),
        ("fn blur_web_dock_url(", "AppTextInput::WebDockUrl"),
    ] {
        require_in_fn(
            sources,
            MAIN,
            needle,
            &[(
                &format!("self.discard_app_text_ime({target})"),
                "閉じた入力欄宛ての変換を捨てていない（続く確定の宛先がずれる。#1750）",
            )],
            &mut out,
        );
    }
    for needle in ["fn open_web_address_bar(", "fn focus_web_dock_url("] {
        require_in_fn(
            sources,
            MAIN,
            needle,
            &[(
                "self.clear_text_input_focus()",
                "他の入力欄を畳んでいない（押下が伝播を止めるので、git の欄が打鍵と変換を\
                 握ったままになる = #503 の型。#1750）",
            )],
            &mut out,
        );
    }
    require_in_fn(
        sources,
        MAIN,
        "fn clear_text_input_focus(&mut self)",
        &[
            (
                "self.blur_web_dock_url()",
                "一括クリアが dock の URL 欄の出口を通っていない（#1750）",
            ),
            (
                "self.close_web_address_bar()",
                "一括クリアがアドレスバーの出口を通っていない（#1750）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn render_webview_address_bar(",
        &[(
            "this.open_web_address_bar(pane_id)",
            "アドレスバーの押下が開く入口を通っていない（#1750）",
        )],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn render_webview_dock_url_input(",
        &[(
            "this.focus_web_dock_url()",
            "URL 欄の押下が開く入口を通っていない（#1750）",
        )],
        &mut out,
    );
    out
}

/// 5: 入力欄の中に未確定文字列とキャレットを描く
fn scan_render(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        OVERLAYS,
        "pub(crate) fn render_command_palette(",
        &[
            (
                "text_input_marked_at(crate::AppTextInput::Palette",
                "未確定文字列をパレットの検索欄の中に描いていない（変換中の読みがどこにも見えない。#1750）",
            ),
            (
                "text_input_caret(crate::AppTextInput::Palette",
                "キャレットを共有部品で描いていない（候補窓の位置出しに実矩形が渡らない。#1750）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn render_webview_address_bar(",
        &[
            (
                "text_input_marked_at(AppTextInput::WebAddress",
                "未確定文字列をアドレスバーの中に描いていない（#1750）",
            ),
            (
                "text_input_caret(AppTextInput::WebAddress",
                "アドレスバーのキャレットを共有部品で描いていない（#1750）",
            ),
            (
                "inline_input_window(",
                "長い URL の末尾で打つとキャレットと読みが欄の外へ押し出される（見えない・\
                 候補窓が欄の外に出る。#1725 と同じ 1 実装で詰める。#1750）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn render_webview_dock_url_input(",
        &[
            (
                "text_input_marked_at(AppTextInput::WebDockUrl",
                "未確定文字列を dock の URL 欄の中に描いていない（#1750）",
            ),
            (
                "text_input_caret(AppTextInput::WebDockUrl",
                "dock の URL 欄のキャレットを共有部品で描いていない（#1750）",
            ),
        ],
        &mut out,
    );
    out
}

/// 6: 見えない入力欄は打鍵も変換も奪わない（打鍵・⌘V・変換が同じ可視判定を使う）
fn scan_visibility(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        MAIN,
        "    fn handle_key(&mut self, keystroke: &Keystroke",
        &[
            (
                "self.web_address_bar_editing().is_some()",
                "打鍵の振り分けがアドレスバーの可視判定を通っていない（#1750）",
            ),
            (
                "self.web_dock_url_active() && self.handle_webview_dock_url_key(",
                "打鍵の振り分けが dock の URL 欄の可視判定を通っていない（#1750）",
            ),
        ],
        &mut out,
    );
    // 旧形: 生のフラグだけで打鍵を握る（見えない欄が打鍵を奪う）
    match fn_window(
        &sources.main,
        "    fn handle_key(&mut self, keystroke: &Keystroke",
    ) {
        None => out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`handle_key` が見つからない（走査が空振り）".into(),
        }),
        Some((at, _, window)) => {
            let body = code_only(&window);
            let lines: Vec<&str> = body.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let t = line.trim();
                let old_dock = t
                    .contains("self.webview_dock_url_focused && self.handle_webview_dock_url_key(");
                let old_addr = t.starts_with("&& self.handle_webview_address_bar_key(")
                    && i > 0
                    && lines[i - 1].trim() == "if self.webview_address_bar_active.is_some()";
                if old_dock || old_addr {
                    let flag = if old_dock {
                        "self.webview_dock_url_focused"
                    } else {
                        "self.webview_address_bar_active"
                    };
                    out.push(Offender {
                        file: MAIN,
                        line: at + i,
                        why: format!(
                            "`handle_key` が生のフラグ `{flag}` で打鍵を握っている\
                             （見えない欄が打鍵を奪う。可視判定を通す。#1750）"
                        ),
                    });
                }
            }
        }
    }
    require_in_fn(
        sources,
        MAIN,
        "    fn paste(&mut self",
        &[
            (
                "self.web_address_bar_editing().is_some()",
                "⌘V の振り分けがアドレスバーの可視判定と割れている（#1750）",
            ),
            (
                "self.web_dock_url_active()",
                "⌘V の振り分けが dock の URL 欄の可視判定と割れている（#1750）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn web_address_bar_editing(&self)",
        &[(
            ".tree().contains(",
            "編集中のペインが今のタブに居るかを見ていない（閉じた / 別タブのアドレスバーが\
             打鍵と変換を奪う。#1750）",
        )],
        &mut out,
    );
    require_in_fn(
        sources,
        MAIN,
        "fn web_dock_url_active(&self)",
        &[(
            "self.webview_dock_open",
            "dock が開いているかを見ていない（閉じた dock の URL 欄が打鍵と変換を奪う。#1750）",
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
fn パレットとwebの入力欄のimeの構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1750 の構造が崩れている:\n{}",
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
        (MAIN, "    fn handle_key(&mut self, keystroke: &Keystroke"),
        (MAIN, "fn handle_palette_key("),
        (MAIN, "fn handle_webview_address_bar_key("),
        (MAIN, "fn handle_webview_dock_url_key("),
        (MAIN, "fn palette_insert("),
        (MAIN, "fn web_address_insert("),
        (MAIN, "fn web_dock_url_insert("),
        (MAIN, "fn close_command_palette("),
        (MAIN, "fn open_web_address_bar("),
        (MAIN, "fn close_web_address_bar("),
        (MAIN, "fn focus_web_dock_url("),
        (MAIN, "fn blur_web_dock_url("),
        (MAIN, "fn web_address_bar_editing(&self)"),
        (MAIN, "fn web_dock_url_active(&self)"),
        (MAIN, "fn clear_text_input_focus(&mut self)"),
        (MAIN, "fn render_webview_address_bar("),
        (MAIN, "fn render_webview_dock_url_input("),
        (MAIN, "struct TakoApp {"),
        (OVERLAYS, "pub(crate) fn render_command_palette("),
    ] {
        let (at, end, window) = fn_window(sources.get(file), needle)
            .unwrap_or_else(|| panic!("{file} の `{needle}` の窓が採れない = 走査が壊れている"));
        assert!(
            at > 0 && end > at,
            "{file} の `{needle}` の行範囲が壊れている"
        );
        // 本体 1 行の関数（`web_dock_url_active`）もあるので 3 行（宣言・本体・閉じ）を下限にする
        assert!(
            window.lines().count() >= 3,
            "{file} の `{needle}` の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    // 「状態への書き込みはここだけ」の検査が本物の書き込みを見ていること（許可された窓の中に
    // 1 件ずつ在る = 綴りが変わって素通りになっていない）
    for (needle, mark) in [
        ("fn palette_insert(", ".query.push_str("),
        ("fn web_dock_url_insert(", "webview_dock_url.insert("),
        ("fn web_address_insert(", "field.insert(text, usize::MAX"),
        ("fn close_command_palette(", "command_palette = None"),
        (
            "fn open_web_address_bar(",
            "webview_address_bar_active = Some(",
        ),
        (
            "fn close_web_address_bar(",
            "webview_address_bar_active.take()",
        ),
        ("fn focus_web_dock_url(", "webview_dock_url_focused = true"),
        ("fn blur_web_dock_url(", "webview_dock_url_focused = false"),
    ] {
        let (_, _, window) = fn_window(&sources.main, needle).expect("窓");
        assert!(
            code_only(&window).contains(mark),
            "`{needle}` の中に `{mark}` が無い（綴りが変わり、`only_inside` の検査が素通りになる）"
        );
    }
    // 範囲取りが本文を残していること（潰れた眺めを渡していない）
    assert!(
        sources.main.lines().count() > 20000,
        "main.rs の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        sources.main.lines().count()
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

    // (1) 真因の逆戻り: パレットを IME の宛先から外す
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "return Some(AppTextInput::Palette);",
        "return None;",
    );
    expect_hit(&s, MAIN, "開いているパレットを");

    // (2) パレットが他の入力欄より後ろで決まる（裏の git の欄が変換を握る）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "        if self.command_palette.is_some() && !legacy_1750 {",
        "        if self.git_branch_input.is_some() {\n            return Some(AppTextInput::GitBranch);\n        }\n        if self.command_palette.is_some() && !legacy_1750 {",
    );
    expect_hit(&s, MAIN, "`AppTextInput::GitBranch` より後ろで決まっている");

    // (3) dock の URL 欄を修正前の「None = ペイン束縛」へ戻す
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "        if legacy_1750 && self.webview_dock_url_focused {",
        "        if self.webview_dock_url_focused {",
    );
    expect_hit(&s, MAIN, "dock の URL 欄を生のフラグで見ている");

    // (4) アドレスバーの宛先の枝ごと消える
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn app_text_input(&self)",
        "return Some(AppTextInput::WebAddress);",
        "return None;",
    );
    expect_hit(&s, MAIN, "Web のアドレスバーを可視判定");

    // (5) IME の確定・unmark がパレットへ入らない
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn insert_app_text_input(",
        "AppTextInput::Palette => self.palette_insert(text, cx),",
        "AppTextInput::Palette => {}",
    );
    expect_hit(&s, MAIN, "self.palette_insert(");

    // (6) ⌘V がパレットへ自前で挿入する（経路が割れる）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn paste(&mut self",
        "self.insert_app_text_input(AppTextInput::Palette, &text, cx);",
        "if let Some(p) = self.command_palette.as_mut() { p.query.push_str(&text); }",
    );
    expect_hit(&s, MAIN, "⌘Vの文字列がパレットの共有の挿入を通らない");
    expect_hit(&s, MAIN, "`palette_insert` の外から挿入している");

    // (7) パレットが開いている間の確定文字列がターミナルへ流れる
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn replace_text_in_range(",
        "self.insert_app_text_input(AppTextInput::Palette, text, cx);",
        "let _ = text;",
    );
    expect_hit(&s, MAIN, "パレットが開いている間の確定文字列");

    // (8) dock の URL 欄の打鍵が手書きの挿入へ戻る
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn handle_webview_dock_url_key(",
        "self.insert_app_text_input(AppTextInput::WebDockUrl, ch, cx)",
        "{ let mut raw = String::new(); raw.insert_str(0, ch); }",
    );
    expect_hit(&s, MAIN, "dock の URL 欄の打鍵が共有の挿入を通らない");
    expect_hit(&s, MAIN, "`insert_str(` が居る");

    // (9) 状態が手書きの本文 + カーソルへ戻る
    let mut s = Sources::load(&root);
    s.main = s.main.replace(
        "    webview_dock_url: crate::text_field::TextField,\n",
        "    webview_dock_url_input: String,\n    webview_dock_url_cursor: usize,\n",
    );
    expect_hit(&s, MAIN, "dock の URL 欄の状態が `TextField` ではない");
    expect_hit(&s, MAIN, "`webview_dock_url_cursor:`");

    // (10) 出口の外でパレットを閉じる（Esc が直に None を書く）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn handle_palette_key(",
        "self.close_command_palette();",
        "self.command_palette = None;",
    );
    expect_hit(
        &s,
        MAIN,
        "`close_command_palette` の外でパレットを閉じている",
    );

    // (11) アドレスバーを閉じても宛ての変換を捨てない
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn close_web_address_bar(",
        "self.discard_app_text_ime(AppTextInput::WebAddress);",
        "",
    );
    expect_hit(&s, MAIN, "閉じた入力欄宛ての変換を捨てていない");

    // (12) アドレスバーを開く入口が他の欄を畳まない
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn open_web_address_bar(",
        "self.clear_text_input_focus();",
        "",
    );
    expect_hit(&s, MAIN, "他の入力欄を畳んでいない");

    // (13) パレットの未確定文字列を描かない
    let mut s = Sources::load(&root);
    s.overlays = replace_in_fn(
        &s.overlays,
        "pub(crate) fn render_command_palette(",
        "self.text_input_marked_at(crate::AppTextInput::Palette, &theme, 13.0)",
        "None::<gpui::Div>",
    );
    expect_hit(
        &s,
        OVERLAYS,
        "未確定文字列をパレットの検索欄の中に描いていない",
    );

    // (14) 打鍵の振り分けが生のフラグへ戻る（閉じた dock の URL 欄が打鍵を奪う）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "    fn handle_key(&mut self, keystroke: &Keystroke",
        "if self.web_dock_url_active() && self.handle_webview_dock_url_key(",
        "if self.webview_dock_url_focused && self.handle_webview_dock_url_key(",
    );
    expect_hit(&s, MAIN, "生のフラグ `self.webview_dock_url_focused");

    // (15) 長い URL を詰めずに並べる（キャレットと読みが欄の外へ押し出される）
    let mut s = Sources::load(&root);
    s.main = replace_in_fn(
        &s.main,
        "fn render_webview_address_bar(",
        "crate::sidebar::inline_input_window(before_full, &marked_text, after_full, cells);",
        "(before_full.to_string(), after_full.to_string());",
    );
    expect_hit(&s, MAIN, "キャレットと読みが欄の外へ押し出される");
}

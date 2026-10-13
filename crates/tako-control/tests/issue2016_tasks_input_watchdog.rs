//! 右パネル tasks ビューの返答欄（IME・折り返し・送信の間）と返答の履歴を縛る番犬（#2016）
//!
//! # なぜ要るか
//!
//! #2016 の実測（visual-test `tasks-input` の `TAKO_2016_LEGACY=1`）:
//!
//! - **IME**: 返答欄は IME の宛先（`AppTextInput::TaskComment`）に入っていて確定も欄に入るのに、
//!   **描画側が未確定文字列もキャレット矩形も出していなかった**。変換中の読みは欄のどこにも
//!   描かれず（欄の画素差 0）、候補窓は `bounds_for_range` の既定 = ターミナルの原点
//!   （欄から 650px 離れた左上）に出ていた。宛先の登録（#561）と描画が別の場所にあるので、
//!   宛先だけ足して描画を忘れても**どこも落ちない**
//! - **折り返し**: 行を縮まない子の横並びで描いていたので、183 文字でも 1 行のまま右へはみ出した。
//!   改行を積むと欄が際限なく伸びた（31 行で 566px）
//! - **送信の間**: 「返す」のハンドラが UI スレッドで 積む → 運ぶ → 記録 → 一覧 を回し終えてから
//!   欄を空にしていた（debug・約 100 KB の置き場で押下に +67〜91 ms）
//! - **履歴**: やりとりは新しい順・相対時刻で、フォームと配送の行の**さらに下**に出ていた
//!
//! どれも**動いては見える**（確定した文字は入る・送れば届く）ので、目視では気づきにくい。
//!
//! # 何を縛るか
//!
//! 1. **IME の宛先は必ず描く** — `enum AppTextInput` の**どの値も**、その値を名指して未確定文字列を
//!    描く関数（`text_input_marked*` か `ime_highlight_ranges`）と、キャレット矩形を記録する関数
//!    （`text_input_caret(` か `text_input_caret_bounds`）を持つ（宛先を増やして描画を忘れると落ちる）
//! 2. **返答欄は折り返して上限で止まる** — `render_task_comment` は `StyledText` で描き、
//!    `whitespace_nowrap` / `text_ellipsis` を持たず、`max_h` + `overflow_y_scroll` で止め、
//!    キャレットは実描画のレイアウト（`position_for_index`）から採る
//! 3. **送信の往復を UI スレッドで待たない** — `user_task_respond` は（A/B の旧実装の枝を除いて）
//!    `tako_control::dispatch(` を呼ばず、欄を空にしてから（`reset_form()` が `pump_user_task_sends(`
//!    より前）待ち行列へ積む。`pump_user_task_sends` は 1 本ずつ（`send_running` の門）① 積む / ③ 記録を
//!    `background_executor()` で、② 運ぶを UI スレッドで呼ぶ（並行に積むと返答の並びが入れ替わる）
//! 4. **返答は 1 実装** — dispatch の "respond" は 3 段（`respond_store_at` →
//!    `user_task_respond_deliver` → `user_task_respond_finish`）を通り、YAML へ積む
//!    `user_tasks::respond_at(` / 記録の `record_delivery_at(` はそれぞれの段にしか無い
//! 5. **履歴は古い順でフォームの上** — `render_thread` は `.rev()` を持たず時刻を
//!    `response_stamp` で出し、`render_task_detail` はそれを返答の見出しより前に置く
//! 6. **CLI も同じ時刻** — `tako todo show` の行（`todo_response_text`）も `response_stamp` を通る
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が採れている
//! ことを固定し、[`逆戻りを名指しできる`] で**注入 14 通り**が `file:line` で名指しされることを
//! 確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const TASKS_PANEL: &str = "crates/tako-app/src/tasks_panel.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const OVERLAYS: &str = "crates/tako-app/src/overlays.rs";
/// #1694: 横断検索の検索欄（`AppTextInput::WorkspaceSearch`）の描画
const SEARCH_PANEL: &str = "crates/tako-app/src/search_panel.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 入力欄の描画が載りうるファイル（IME の宛先ごとの描画を探す範囲）
const RENDER_FILES: &[&str] = &[
    MAIN,
    TASKS_PANEL,
    RIGHT_PANEL,
    SIDEBAR,
    OVERLAYS,
    SEARCH_PANEL,
];

/// 未確定文字列を描いたしるし
const MARKED_MARKS: &[&str] = &["text_input_marked", "ime_highlight_ranges"];
/// キャレット矩形（候補窓の位置）を記録したしるし
const CARET_MARKS: &[&str] = &["text_input_caret(", "text_input_caret_bounds"];

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

/// 窓（1-based の宣言行・終わりの行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    window_from(&lines, start)
}

fn window_from(lines: &[&str], start: usize) -> Option<(usize, usize, String)> {
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| {
            **l == close
                || l.starts_with(&format!("{close},"))
                || l.starts_with(&format!("{close})"))
        })
        .map(|(i, _)| i)?;
    Some((start + 1, end + 1, lines[start..=end].join("\n")))
}

/// ファイル内の関数の窓をすべて、行範囲（0-based・両端を含む）で返す
/// （`fn ` で始まる宣言行から同じ字下げの `}` まで。本文の文字列は組まない = main.rs の
/// 数千の関数でも軽い）
fn fn_ranges(lines: &[&str]) -> Vec<(usize, usize)> {
    // 字下げごとに「開いている関数」を 1 つ持つ（正しく入れ子になったコードでは、同じ字下げの
    // 関数が閉じる前に次の同じ字下げの関数は始まらない）。1 回の走査で全部の窓が取れる
    let mut open: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let indent = l.len() - t.len();
        if t == "}" {
            if let Some(start) = open.remove(&indent) {
                out.push((start, i));
            }
            continue;
        }
        let is_fn = [
            "fn ",
            "pub fn ",
            "pub(crate) fn ",
            "pub(super) fn ",
            "async fn ",
            "pub(super) async fn ",
        ]
        .iter()
        .any(|p| t.starts_with(p));
        // 本体の無い宣言（trait の `fn f();`）は窓にしない
        if is_fn && !l.trim_end().ends_with(';') {
            open.insert(indent, i);
        }
    }
    out
}

/// 行ごとの印の累積和（範囲に印のある行が 1 つでもあるかを O(1) で引く）
fn prefix(lines: &[&str], hit: impl Fn(&str) -> bool) -> Vec<usize> {
    let mut acc = vec![0usize; lines.len() + 1];
    for (i, l) in lines.iter().enumerate() {
        acc[i + 1] = acc[i] + usize::from(hit(l));
    }
    acc
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

#[derive(Clone)]
struct Sources {
    main: String,
    tasks_panel: String,
    right_panel: String,
    sidebar: String,
    overlays: String,
    search_panel: String,
    dispatch: String,
    cli: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: prod(root, MAIN),
            tasks_panel: prod(root, TASKS_PANEL),
            right_panel: prod(root, RIGHT_PANEL),
            sidebar: prod(root, SIDEBAR),
            overlays: prod(root, OVERLAYS),
            search_panel: prod(root, SEARCH_PANEL),
            dispatch: prod(root, DISPATCH),
            cli: prod(root, CLI),
        }
    }

    fn get(&self, file: &'static str) -> &str {
        match file {
            MAIN => &self.main,
            TASKS_PANEL => &self.tasks_panel,
            RIGHT_PANEL => &self.right_panel,
            SIDEBAR => &self.sidebar,
            OVERLAYS => &self.overlays,
            SEARCH_PANEL => &self.search_panel,
            DISPATCH => &self.dispatch,
            _ => &self.cli,
        }
    }
}

/// `needle` の窓が `must` をすべてコードとして含むか（無ければ宣言行で名指す）
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

/// `needle` の窓が `banned` をコードとして含んだら、その行で名指す
fn forbid_in_fn(
    sources: &Sources,
    file: &'static str,
    needle: &str,
    banned: &[(&str, &str)],
    out: &mut Vec<Offender>,
) {
    if let Some((at, _, window)) = fn_window(sources.get(file), needle) {
        for (mark, why) in banned {
            if let Some(i) = code_line_with(&window, mark) {
                out.push(Offender {
                    file,
                    line: at + i,
                    why: format!("`{needle}` に `{mark}` がある: {why}"),
                });
            }
        }
    }
}

/// `enum AppTextInput` の値（名前と 1-based の行）
fn app_text_inputs(main: &str) -> Vec<(String, usize)> {
    let Some((at, _, window)) = fn_window(main, "enum AppTextInput {") else {
        return Vec::new();
    };
    code_only(&window)
        .lines()
        .enumerate()
        .skip(1)
        .filter_map(|(i, l)| {
            let t = l.trim();
            let name = t.strip_suffix(',')?;
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| (name.to_string(), at + i))
        })
        .collect()
}

/// 描画のファイル 1 本ぶんの索引（注釈を落とした行・関数の行範囲・印の行の累積和）
struct RenderFile {
    code: Vec<String>,
    ranges: Vec<(usize, usize)>,
    marked: Vec<usize>,
    caret: Vec<usize>,
}

/// 1: IME の宛先はどれも、未確定文字列とキャレット矩形を描く関数を持つ
fn scan_every_target_renders(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let variants = app_text_inputs(&sources.main);
    if variants.is_empty() {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "`enum AppTextInput` の値が 1 つも採れない（走査が空振り）".into(),
        });
        return out;
    }
    // 描画のファイルごとに（注釈を落とした行, 関数の行範囲, 印の累積和）
    let files: Vec<RenderFile> = RENDER_FILES
        .iter()
        .map(|f| {
            let code: Vec<String> = code_only(sources.get(f))
                .lines()
                .map(str::to_string)
                .collect();
            let refs: Vec<&str> = code.iter().map(String::as_str).collect();
            RenderFile {
                ranges: fn_ranges(&refs),
                marked: prefix(&refs, |l| MARKED_MARKS.iter().any(|m| l.contains(m))),
                caret: prefix(&refs, |l| CARET_MARKS.iter().any(|m| l.contains(m))),
                code,
            }
        })
        .collect();
    for (name, line) in variants {
        let tag = format!("AppTextInput::{name}");
        // その値を名指す行の累積和はファイルごとに 1 回だけ作る
        let tagged: Vec<Vec<usize>> = files
            .iter()
            .map(|f| {
                let refs: Vec<&str> = f.code.iter().map(String::as_str).collect();
                prefix(&refs, |l| l.contains(&tag))
            })
            .collect();
        let has = |want_marked: bool| {
            files.iter().zip(&tagged).any(|(f, tagged)| {
                let acc = if want_marked { &f.marked } else { &f.caret };
                f.ranges
                    .iter()
                    .any(|(a, b)| tagged[b + 1] > tagged[*a] && acc[b + 1] > acc[*a])
            })
        };
        if !has(true) {
            out.push(Offender {
                file: MAIN,
                line,
                why: format!(
                    "`{tag}` を IME の宛先にしているのに、その欄へ未確定文字列を描く描画が無い\
                     （変換中の読みが欄のどこにも出ない = #2016 の症状 1）"
                ),
            });
        }
        if !has(false) {
            out.push(Offender {
                file: MAIN,
                line,
                why: format!(
                    "`{tag}` を IME の宛先にしているのに、その欄のキャレット矩形を記録する描画が無い\
                     （候補窓がターミナルや前に使った別の欄の位置に出る = #2016 の症状 1）"
                ),
            });
        }
    }
    out
}

/// 2: 返答欄は折り返して上限で止まり、キャレットと候補窓は実描画のレイアウトから採る
fn scan_comment_render(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        TASKS_PANEL,
        "fn render_task_comment(&mut self",
        &[
            (
                "StyledText",
                "本文 + 未確定文字列を 1 本の折り返すテキストで描く",
            ),
            (
                "ime_highlight_ranges",
                "未確定文字列を下線つきで欄の中に描く",
            ),
            (
                "text_input_caret_bounds",
                "候補窓の位置（bounds_for_range）へキャレット矩形を渡す",
            ),
            (
                "position_for_index(caret_ix)",
                "キャレットを折り返し後の実描画の位置に置く",
            ),
            (".max_h(", "欄は上限の行数で止まる（際限なく伸びない）"),
            (
                "overflow_y_scroll()",
                "上限を超えたら欄の中でスクロールする",
            ),
        ],
        &mut out,
    );
    forbid_in_fn(
        sources,
        TASKS_PANEL,
        "fn render_task_comment(&mut self",
        &[
            (
                "whitespace_nowrap",
                "欄の幅で折り返さない（#2016 の症状 2）",
            ),
            (
                "text_ellipsis",
                "はみ出した本文が見えない（#2016 の症状 2）",
            ),
        ],
        &mut out,
    );
    require_in_fn(
        sources,
        TASKS_PANEL,
        "fn render_task_detail(",
        &[(
            "self.render_task_comment(cx)",
            "詳細が新しい返答欄を描いていない",
        )],
        &mut out,
    );
    out
}

/// 窓から `if legacy_2016() { … }` の枝を除いた本文（A/B の旧実装は同期のままでよい）
fn without_legacy_arm(window: &str) -> String {
    let lines: Vec<&str> = window.lines().collect();
    let Some(start) = lines.iter().position(|l| l.contains("if legacy_2016() {")) else {
        return window.to_string();
    };
    let Some((_, end, _)) = window_from(&lines, start) else {
        return window.to_string();
    };
    // 行数を保つ（名指す行がずれないように枝の中を空行へ）
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| if i >= start && i < end { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 3: 送信の往復を UI スレッドで待たない（欄を先に空にし、1 本ずつ順に 3 段へ流す）
fn scan_send_off_ui_thread(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let respond = "pub(crate) fn user_task_respond(";
    let pump = "pub(crate) fn pump_user_task_sends(";
    let (Some((at, _, window)), Some((pump_at, _, pump_window))) = (
        fn_window(&sources.tasks_panel, respond),
        fn_window(&sources.tasks_panel, pump),
    ) else {
        out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: format!("`{respond}` か `{pump}` が見つからない（走査が空振り）"),
        });
        return out;
    };
    let body = code_only(&without_legacy_arm(&window));
    let pump_body = code_only(&pump_window);
    for (base, text) in [(at, &body), (pump_at, &pump_body)] {
        if let Some(i) = code_line_with(text, "tako_control::dispatch(") {
            out.push(Offender {
                file: TASKS_PANEL,
                line: base + i,
                why: "送信が UI スレッドで dispatch を回している（往復が終わるまで欄が空にならない = \
                      #2016 の症状 3）"
                    .into(),
            });
        }
    }
    match (
        code_line_with(&body, "reset_form()"),
        code_line_with(&body, "pump_user_task_sends("),
    ) {
        (Some(clear), Some(send)) if clear < send => {}
        (_, send) => out.push(Offender {
            file: TASKS_PANEL,
            line: at + send.unwrap_or(0),
            why: "欄を空にする（`reset_form()`）のが往復を出す（`pump_user_task_sends(`）より前に無い\
                  （押した次のフレームで欄が空にならない = #2016 の症状 3）"
                .into(),
        }),
    }
    for (mark, why) in [
        ("user_task_respond_store", "① 積む段を通っていない"),
        ("user_task_respond_deliver", "② 運ぶ段を通っていない"),
        ("user_task_respond_finish", "③ 記録する段を通っていない"),
        (
            "background_executor()",
            "積む・記録するを background で回していない",
        ),
        (
            "send_running",
            "1 本ずつの門が無い（並行に積むと返答の並びが送った順とずれる）",
        ),
    ] {
        if !pump_body.contains(mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line: pump_at,
                why: format!("`{pump}` に `{mark}` が無い: {why}"),
            });
        }
    }
    out
}

/// 4: 返答は dispatch の 3 段の 1 実装を通る
fn scan_respond_single_impl(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        DISPATCH,
        "\"respond\" => {",
        &[
            (
                "respond_store_at(",
                "CLI / MCP の返答が ① 積む段を通っていない",
            ),
            (
                "user_task_respond_deliver(",
                "CLI / MCP の返答が ② 運ぶ段を通っていない",
            ),
            (
                "user_task_respond_finish(",
                "CLI / MCP の返答が ③ 記録する段を通っていない",
            ),
        ],
        &mut out,
    );
    for (mark, owner) in [
        ("crate::user_tasks::respond_at(", "fn respond_store_at("),
        (
            "crate::user_tasks::record_delivery_at(",
            "pub fn user_task_respond_finish(",
        ),
    ] {
        let code = code_only(&sources.dispatch);
        let owner_range = fn_window(&sources.dispatch, owner).map(|(a, b, _)| (a, b));
        for (i, l) in code.lines().enumerate() {
            if !l.contains(mark) {
                continue;
            }
            let line = i + 1;
            let inside = owner_range.is_some_and(|(a, b)| line >= a && line <= b);
            if !inside {
                out.push(Offender {
                    file: DISPATCH,
                    line,
                    why: format!(
                        "`{mark}` が `{owner}` の外にある（画面と CLI / MCP で返答の段が 2 本になる）"
                    ),
                });
            }
        }
    }
    out
}

/// 5 / 6: 履歴は古い順でフォームの上、時刻は CLI と同じ 1 実装
fn scan_history(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    require_in_fn(
        sources,
        TASKS_PANEL,
        "fn render_thread(",
        &[(
            "tako_core::user_task::response_stamp(",
            "やりとりの時刻が `tako todo show` と同じ 1 実装を通っていない",
        )],
        &mut out,
    );
    forbid_in_fn(
        sources,
        TASKS_PANEL,
        "fn render_thread(",
        &[(
            ".rev()",
            "やりとりが新しい順に並ぶ（古い順 = 会話の並びにする）",
        )],
        &mut out,
    );
    if let Some((at, _, window)) = fn_window(&sources.tasks_panel, "fn render_task_detail(") {
        let thread = code_line_with(&window, "render_thread(");
        let heading = code_line_with(&window, "tasks_respond_heading()");
        match (thread, heading) {
            (Some(t), Some(h)) if t < h => {}
            (t, _) => out.push(Offender {
                file: TASKS_PANEL,
                line: at + t.unwrap_or(0),
                why:
                    "やりとりが返答フォームの上に無い（開いても目に入らず「履歴が無い」に見える = \
                      #2016 の症状 4）"
                        .into(),
            }),
        }
    }
    require_in_fn(
        sources,
        CLI,
        "fn todo_response_text(",
        &[(
            "tako_core::user_task::response_stamp(",
            "`tako todo show` の時刻が右パネルと同じ 1 実装を通っていない",
        )],
        &mut out,
    );
    require_in_fn(
        sources,
        CLI,
        "fn print_todo_detail(",
        &[(
            "todo_response_text(",
            "やりとりの行が時刻つきの 1 実装を通っていない",
        )],
        &mut out,
    );
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = scan_every_target_renders(sources);
    out.extend(scan_comment_render(sources));
    out.extend(scan_send_off_ui_thread(sources));
    out.extend(scan_respond_single_impl(sources));
    out.extend(scan_history(sources));
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
fn 返答欄と履歴の構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#2016 の構造が崩れている:\n{}",
        report(&offenders)
    );
}

/// 走査が空振りしていれば上のテストは無意味に緑になるので、窓が採れていることを固定する
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let sources = Sources::load(&root);
    for (file, needle) in [
        (MAIN, "enum AppTextInput {"),
        (TASKS_PANEL, "fn render_task_comment(&mut self"),
        (TASKS_PANEL, "fn render_task_detail("),
        (TASKS_PANEL, "pub(crate) fn user_task_respond("),
        (TASKS_PANEL, "pub(crate) fn pump_user_task_sends("),
        (TASKS_PANEL, "fn render_thread("),
        (DISPATCH, "\"respond\" => {"),
        (DISPATCH, "fn respond_store_at("),
        (DISPATCH, "pub fn user_task_respond_finish("),
        (CLI, "fn todo_response_text("),
        (CLI, "fn print_todo_detail("),
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
    // IME の宛先は今 7 つ。どれも描画の関数が採れている（数の固定ではなく空振りの見張り）
    let variants = app_text_inputs(&sources.main);
    assert!(
        variants.len() >= 7 && variants.iter().any(|(n, _)| n == "TaskComment"),
        "`enum AppTextInput` の値が採れていない: {variants:?}"
    );
    let windows: usize = RENDER_FILES
        .iter()
        .map(|f| fn_ranges(&sources.get(f).lines().collect::<Vec<_>>()).len())
        .sum();
    assert!(
        windows > 500,
        "描画のファイルから関数の窓が {windows} 個しか採れない"
    );
    // 旧実装の枝を除いても送信の本体が残っている（除き過ぎていない）
    let (_, _, w) = fn_window(&sources.tasks_panel, "pub(crate) fn user_task_respond(").unwrap();
    assert!(without_legacy_arm(&w).contains("pump_user_task_sends("));
    assert!(
        w.contains("if legacy_2016() {"),
        "A/B の旧実装の枝が見つからない"
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
    // 読み込み（本番範囲の切り出し）は重いので 1 回だけにして、注入ごとに複製する
    let base = Sources::load(&workspace_root());
    assert!(scan_all(&base).is_empty(), "注入前が既に汚れている");
    let comment = "fn render_task_comment(&mut self";

    // (1) 真因の逆戻り: 返答欄が未確定文字列を描かない
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        comment,
        "crate::ime_highlight_ranges(marked_text, selected.as_ref(), &theme)",
        "Vec::<(std::ops::Range<usize>, gpui::HighlightStyle)>::new()",
    );
    expect_hit(
        &s,
        MAIN,
        "`AppTextInput::TaskComment` を IME の宛先にしているのに、その欄へ未確定文字列",
    );
    expect_hit(&s, TASKS_PANEL, "`ime_highlight_ranges` が無い");

    // (2) 真因の逆戻り: 候補窓の位置へキャレット矩形を渡さない
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        comment,
        "let slot = ime_target.then(|| self.text_input_caret_bounds.clone());",
        "let slot: Option<std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>> = None;",
    );
    expect_hit(&s, MAIN, "その欄のキャレット矩形を記録する描画が無い");

    // (3) 宛先を増やして描画を忘れる（次の欄で同じ型を踏む）
    let mut s = base.clone();
    s.main = s
        .main
        .replacen("    TaskComment,\n", "    TaskComment,\n    NewInput,\n", 1);
    expect_hit(
        &s,
        MAIN,
        "`AppTextInput::NewInput` を IME の宛先にしているのに",
    );

    // (4) 欄の幅で折り返さない
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        comment,
        ".line_height(px(COMMENT_LINE_H))\n                        .child(styled)",
        ".line_height(px(COMMENT_LINE_H))\n                        .whitespace_nowrap()\n                        .child(styled)",
    );
    expect_hit(&s, TASKS_PANEL, "`whitespace_nowrap` がある");

    // (5) 上限なしに伸びる
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        comment,
        ".max_h(px(COMMENT_LINE_H * COMMENT_MAX_LINES + 8.0))",
        ".min_h(px(COMMENT_LINE_H))",
    );
    expect_hit(&s, TASKS_PANEL, "`.max_h(` が無い");

    // (6) キャレットを実描画の位置から採らない（行ごとの横並びへ戻る）
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        comment,
        "layout.position_for_index(caret_ix)",
        "Some(gpui::point(gpui::px(0.0), gpui::px(0.0)))",
    );
    expect_hit(&s, TASKS_PANEL, "`position_for_index(caret_ix)` が無い");

    // (7) 送信が UI スレッドで往復を待つ（修正前の形）
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        "pub(crate) fn user_task_respond(",
        "self.user_tasks.send_seq += 1;",
        "let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n        self.user_tasks.send_seq += 1;",
    );
    expect_hit(
        &s,
        TASKS_PANEL,
        "送信が UI スレッドで dispatch を回している",
    );

    // (8) 欄を空にするのが往復の後
    let mut s = base.clone();
    let (_, _, w) = fn_window(&s.tasks_panel, "pub(crate) fn user_task_respond(").unwrap();
    let moved = w
        .replacen(
            "        self.user_tasks.reset_form();\n        self.user_tasks.notice = None;",
            "        self.user_tasks.notice = None;",
            1,
        )
        .replacen(
            "        self.pump_user_task_sends(cx);\n",
            "        self.pump_user_task_sends(cx);\n        self.user_tasks.reset_form();\n",
            1,
        );
    assert_ne!(moved, w, "注入（8）が当たっていない");
    s.tasks_panel = s.tasks_panel.replace(&w, &moved);
    expect_hit(
        &s,
        TASKS_PANEL,
        "欄を空にする（`reset_form()`）のが往復を出す",
    );

    // (9) 積む段を background へ出さない
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        "pub(crate) fn pump_user_task_sends(",
        "tako_control::dispatch::user_task_respond_store(&request, PaneOrigin::User)",
        "Err::<(), _>(request)",
    );
    expect_hit(&s, TASKS_PANEL, "`user_task_respond_store` が無い");

    // (9b) 1 本ずつの門を外す（続けて押すと並行に積まれ、返答の並びが入れ替わる）
    let mut s = base.clone();
    let (_, _, w) = fn_window(&s.tasks_panel, "pub(crate) fn pump_user_task_sends(").unwrap();
    s.tasks_panel = s
        .tasks_panel
        .replace(&w, &w.replace("send_running", "send_unused"));
    expect_hit(&s, TASKS_PANEL, "1 本ずつの門が無い");

    // (10) CLI / MCP の返答が段を通らず直に積む（画面と 2 本になる）
    let mut s = base.clone();
    s.dispatch = replace_in_fn(
        &s.dispatch,
        "\"respond\" => {",
        "let stored = respond_store_at(path, &params, origin)?;",
        "let stored = crate::user_tasks::respond_at(&path, \"u-1\", d, \"\", v);",
    );
    expect_hit(&s, DISPATCH, "`respond_store_at(` が無い");
    expect_hit(
        &s,
        DISPATCH,
        "`crate::user_tasks::respond_at(` が `fn respond_store_at(` の外",
    );

    // (11) やりとりが新しい順へ戻る
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        "fn render_thread(",
        "for r in responses {",
        "for r in responses.iter().rev() {",
    );
    expect_hit(&s, TASKS_PANEL, "`.rev()` がある");

    // (12) やりとりがフォームの下へ戻る
    let mut s = base.clone();
    let (_, _, w) = fn_window(&s.tasks_panel, "fn render_task_detail(").unwrap();
    let call = "detail = detail.child(render_thread(&theme, &task.responses, &sending, now));";
    assert!(w.contains(call), "注入（12）の素材が無い");
    let moved = w.replacen(call, "", 1).replacen(
        "        // --- 完了 / 却下 ---",
        &format!("        {call}\n        // --- 完了 / 却下 ---"),
        1,
    );
    s.tasks_panel = s.tasks_panel.replace(&w, &moved);
    expect_hit(&s, TASKS_PANEL, "やりとりが返答フォームの上に無い");

    // (13) 時刻の表記が画面と CLI で割れる
    let mut s = base.clone();
    s.cli = replace_in_fn(
        &s.cli,
        "fn todo_response_text(",
        "tako_core::user_task::response_stamp(at, now, utc_offset)",
        "at.to_string()",
    );
    expect_hit(
        &s,
        CLI,
        "`tako todo show` の時刻が右パネルと同じ 1 実装を通っていない",
    );
    let mut s = base.clone();
    s.tasks_panel = replace_in_fn(
        &s.tasks_panel,
        "fn render_thread(",
        "tako_core::user_task::response_stamp(r.at, now, offset)",
        "crate::ui_text::panel::tasks_ago(now - r.at)",
    );
    expect_hit(
        &s,
        TASKS_PANEL,
        "やりとりの時刻が `tako todo show` と同じ 1 実装を通っていない",
    );
}

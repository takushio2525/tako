//! 補完（#1682）の構造の番犬
//!
//! # なぜ要るか
//!
//! 受け入れ条件のうち「候補 1000 件で可視ぶんだけ組む」「古い要求を `$/cancelRequest` で捨てる」
//! 「古い答えを版の照合で捨てる」「5 キーの行き先は表で決める」は、どれも **1 行で壊せる**のに、
//! 壊しても答えの形（CLI の JSON）は変わらない: `gpui::list` を全件の `children` に戻せば
//! 1000 件で 1000 行組むようになり、`cancel_request` を消せば打鍵のたびの要求がサーバに
//! 溜まり、`session.accept` を外せば打ち足した後に古い一覧が出る。挙動は e2e
//! （`issue1682_lsp_completion`）・単体（tako-core / dispatch）・visual-test `completion` が測り、
//! ここは**構造**を縛って file:line で名指す（GUI を立てない CI でも落ちる）。
//!
//! # 何を縛るか
//!
//! 1. manager の取り消しの列: 次の要求は前の要求を `cancel_request`（= `$/cancelRequest`）で捨て、
//!    打鍵の補完はその列を通る
//! 2. GUI の打鍵の要求は `superseding: true`（取り消し合う）で、答えは `session.accept`（番号と版の
//!    照合）を通ってから出す
//! 3. 一覧は `gpui::list` で組む（全件ぶん組むのは A/B の注入口 `no_virtual_list()` の腕だけ）
//! 4. 5 キーの行き先は表（`route_key`）で決め、編集のキーの入口は検索欄より先にそれを引く
//! 5. 確定は dispatch の `lsp_completion_apply` の 1 本（GUI が本文を自前で書き換えない）
//! 6. 絞り込みと並べ替えは tako-core の `rank`（GUI と CLI で並びが食い違わない）
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

const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const GUI: &str = "crates/tako-app/src/lsp_completion_ui.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
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

/// 窓の中で `needle` を含む最初の行（1-based）
fn line_in(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| at + i)
}

/// 規則 1 つ: `file` の関数 `decl` のコードが `must` をすべて含み、`must_not` を含まない
struct Rule {
    file: &'static str,
    decl: &'static str,
    must: &'static [&'static str],
    must_not: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        file: MANAGER,
        decl: "fn supersede(&self, lane: Lane) -> u64 {",
        must: &["cancel_request("],
        must_not: &[],
        why: "次の要求が前の要求を `$/cancelRequest` で捨てていない（打鍵のたびの要求がサーバに溜まり、古い答えを待つ）",
    },
    Rule {
        file: MANAGER,
        decl: "fn completion(&self, request: &CompletionRequest)",
        must: &["self.supersede(Lane::Completion)", "request_in_lane("],
        must_not: &[],
        why: "打鍵の補完が取り消しの列を通っていない（前の要求を取り消せない）",
    },
    Rule {
        file: GUI,
        decl: "fn fire_completion(",
        must: &["superseding: true"],
        must_not: &[],
        why: "GUI の打鍵の要求が取り消し合わない（`superseding: true` でない）",
    },
    Rule {
        file: GUI,
        decl: "fn finish_completion(",
        must: &[".session.accept(", "Verdict::Show"],
        must_not: &[],
        why: "答えを番号と版で照合してから出していない（打ち足した後に古い一覧が出る）",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn render_lsp_completion(",
        must: &["gpui::list(", "if no_virtual_list() {"],
        must_not: &[],
        why: "一覧を `gpui::list` で組んでいない（1000 件で 1000 行組む = #821 の 1 フレーム数千要素）",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn route_completion_key(",
        must: &["comp::route_key("],
        must_not: &[],
        why: "5 キーの行き先を表（`route_key`）で決めていない",
    },
    Rule {
        file: MAIN,
        decl: "fn handle_preview_edit_key(",
        must: &["self.route_completion_key("],
        must_not: &[],
        why: "編集のキーの入口が補完の表を引いていない（Enter が改行・Esc が編集モードを抜ける）",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn accept_completion(",
        must: &["dispatch::lsp_completion_apply("],
        must_not: &["replace_position_ranges(", ".buffer.insert(", "replace_range("],
        why: "確定が dispatch の 1 本（`lsp_completion_apply`）を通っていない（CLI の --choice と入り方が割れる）",
    },
    Rule {
        file: GUI,
        decl: "fn show_completion(",
        must: &["comp::rank("],
        must_not: &[],
        why: "一覧の並びを tako-core の `rank` で決めていない（GUI と CLI で並びが食い違う）",
    },
    Rule {
        file: GUI,
        decl: "fn refilter_completion(",
        must: &["comp::rank("],
        must_not: &[],
        why: "打ち足したときの絞り直しが tako-core の `rank` を通っていない",
    },
    Rule {
        file: DISPATCH,
        decl: "pub fn lsp_completion_land(",
        must: &["rank(", "truncate(", "lsp_completion_apply("],
        must_not: &[],
        why: "CLI / MCP の着地が GUI と同じ絞り込み・件数の上限・確定を通っていない",
    },
];

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(&'static str, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for rule in RULES {
        let src = &sources
            .iter()
            .find(|(f, _)| *f == rule.file)
            .expect("走査対象")
            .1;
        let Some((at, window)) = fn_window(src, rule.decl) else {
            out.push(format!(
                "{}:0 — `{}` が見つからない（走査が空振り）",
                rule.file, rule.decl
            ));
            continue;
        };
        let code = code_only(&window);
        for needle in rule.must {
            if !code.contains(needle) {
                out.push(format!(
                    "{}:{at} — {}（`{needle}` が無い）",
                    rule.file, rule.why
                ));
            }
        }
        for needle in rule.must_not {
            if code.contains(needle) {
                let line = line_in(at, &window, needle).unwrap_or(at);
                out.push(format!(
                    "{}:{line} — {}（`{needle}` が在る）",
                    rule.file, rule.why
                ));
            }
        }
    }
    // 4 の順序: 編集のキーの入口は検索欄の経路より先に補完の表を引く
    // （後だと、検索欄と補完が同時に出ているときの Esc が検索欄を先に閉じる）
    let main = &sources.iter().find(|(f, _)| *f == MAIN).expect("main").1;
    if let Some((at, window)) = fn_window(main, "fn handle_preview_edit_key(") {
        let code = code_only(&window);
        if let (Some(route), Some(search)) = (
            code.find("self.route_completion_key("),
            code.find("self.handle_search_bar_key("),
        ) {
            if search < route {
                let line = line_in(at, &window, "self.route_completion_key(").unwrap_or(at);
                out.push(format!(
                    "{MAIN}:{line} — 補完の表を検索欄の経路より後で引いている（同時表示の Esc が検索欄を先に閉じる）"
                ));
            }
        }
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    [MANAGER, DISPATCH, GUI, MAIN]
        .into_iter()
        .map(|f| (f, read(f)))
        .collect()
}

#[test]
fn 補完の構造は縛りを満たす() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    for rule in RULES {
        let src = &sources.iter().find(|(f, _)| *f == rule.file).unwrap().1;
        let (_, window) = fn_window(src, rule.decl)
            .unwrap_or_else(|| panic!("{}: `{}` の窓が採れない", rule.file, rule.decl));
        // 窓が関数の頭だけで切れていない（本体まで採れている）
        assert!(
            window.lines().count() >= 3,
            "{}: `{}` の窓が短すぎる",
            rule.file,
            rule.decl
        );
    }
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること
fn assert_named(label: &str, file: &'static str, (from, to): (&str, &str), needle: &str) {
    let mut sources = current();
    let slot = sources.iter_mut().find(|(f, _)| *f == file).unwrap();
    assert!(
        slot.1.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    slot.1 = slot.1.replacen(from, to, 1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&sources);
    let expected = format!("{file}:{line} ");
    assert!(
        reports.iter().any(|r| r.starts_with(&expected)),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    // A. 前の要求を取り消さない
    assert_named(
        "A 取り消さない",
        MANAGER,
        ("process.0.cancel_request(&process.1);", "let _ = &process;"),
        "fn supersede(&self, lane: Lane) -> u64 {",
    );
    // B. 一覧を全件ぶん組む（仮想化を外す）
    assert_named(
        "B 仮想化を外す",
        GUI,
        ("gpui::list(state, move |ix, _window, cx| {", "gpui::div().children(std::iter::once(move |ix: usize, _window: &mut gpui::Window, cx: &mut gpui::App| {"),
        "pub(crate) fn render_lsp_completion(",
    );
    // C. GUI の打鍵の要求が取り消し合わない
    assert_named(
        "C 取り消し合わない",
        GUI,
        (
            "            superseding: true,",
            "            superseding: false,",
        ),
        "fn fire_completion(",
    );
    // D. 版の照合を外す（古い答えでも出す）
    assert_named(
        "D 版の照合を外す",
        GUI,
        (
            "let verdict = self.lsp_completion.session.accept(seq, version);",
            "let verdict = { let _ = (seq, version); comp::Verdict::Show };",
        ),
        "fn finish_completion(",
    );
    // E. 表を引かない
    assert_named(
        "E 表を引かない",
        GUI,
        (
            "match comp::route_key(search_open, completion_open, key) {",
            "match { let _ = (search_open, completion_open, key); KeyRoute::Editor } {",
        ),
        "pub(crate) fn route_completion_key(",
    );
    // F. 確定で本文を自前で書き換える
    assert_named(
        "F 自前で書き換える",
        GUI,
        (
            "        match tako_control::dispatch::lsp_completion_apply(",
            "        let _ = edit.buffer.replace_range(0..0, \"\");\n        match tako_control::dispatch::lsp_completion_apply(",
        ),
        "edit.buffer.replace_range(0..0",
    );
    // G. 入口が表を検索欄より後で引く
    assert_named(
        "G 表を後で引く",
        MAIN,
        (
            "        if let Some(handled) = self.route_completion_key(pane_id, keystroke, cx) {\n            return handled;\n        }\n",
            "        if false { let _ = self.handle_search_bar_key(pane_id, keystroke, cx); }\n        if let Some(handled) = self.route_completion_key(pane_id, keystroke, cx) {\n            return handled;\n        }\n",
        ),
        "        if let Some(handled) = self.route_completion_key(pane_id, keystroke, cx) {",
    );
}

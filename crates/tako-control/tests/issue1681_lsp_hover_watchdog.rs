//! ホバー（#1681）の構造の番犬
//!
//! # なぜ要るか
//!
//! 受け入れ条件のうち「Markdown は `render_block` を通す」「ウィンドウの端で見切れない」
//! 「100 回出し入れしても保持件数が増えない」「乗せただけでサーバを起こさない」は、どれも
//! **1 行で壊せる**のに、壊しても答えの形（CLI の JSON）は変わらない: カードの本文を
//! `render_block` を通さず生テキストで描けば見出しもコードブロックも素の字になり、
//! `superseding: true` を外せばなぞるたびの要求がサーバに溜まり、`open: false` を外せば
//! 閲覧しているだけのファイルで rust-analyzer が起きる。挙動は e2e（`issue1681_lsp_hover`）・
//! 単体（tako-core / tako-app の `lsp_hover_ui` / dispatch）・visual-test `hover` が測り、ここは
//! **構造**を縛って file:line で名指す（GUI を立てない CI でも落ちる）。
//!
//! # 何を縛るか
//!
//! 1. manager: マウスの要求は取り消しの列（`Lane::Hover`）を通り、`open` が偽なら開いている文書へ
//!    持ち手として加わるだけ（開かない）
//! 2. GUI のマウスの要求は `superseding: true` と `open: false` で、答えは番号と版の照合を通ってから出す
//! 3. カードの本文は `md_view::render_blocks`（= 1 ブロックずつ `render_block`）を通す。平文は
//!    Markdown として読まない
//! 4. カードの置き場は `hover_card_placement`（中で `compute_menu_position` の返しの判定を使う）
//! 5. カードを出す口は `open_lsp_hover_card` の 1 本（CLI / MCP の `show` も GUI のマウスも）。
//!    編集メニューは CLI と同じ dispatch の 3 段を通す（manager を直に叩かない）
//! 6. 入口: マウス移動がホバーを更新し、打鍵の入口は補完の表を先に引いてからホバーの Esc を見る。
//!    重ね順は補完の一覧が手前。右クリックメニュー（#1684）を開いているあいだはカードを出さない
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
const GUI: &str = "crates/tako-app/src/lsp_hover_ui.rs";
const MD_VIEW: &str = "crates/tako-app/src/md_view.rs";
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
        decl: "    fn hover(&self, request: &HoverRequest)",
        must: &[
            "self.supersede(Lane::Hover)",
            "request_in_lane(",
            "if request.open {",
            "self.join(&uri)",
        ],
        must_not: &[],
        why: "マウスの要求が取り消しの列を通っていない / 開いていない文書でも開いてしまう（乗せただけでサーバが起きる）",
    },
    Rule {
        file: GUI,
        decl: "fn fire_lsp_hover(",
        must: &["superseding: true", "open: false"],
        must_not: &[],
        why: "GUI のマウスの要求が取り消し合わない / 開いていない文書を開く",
    },
    Rule {
        file: GUI,
        decl: "fn finish_lsp_hover(",
        must: &[
            "seq != self.lsp_hover.seq",
            "self.hover_version(target.pane) != version",
        ],
        must_not: &[],
        why: "答えを番号と版で照合してから出していない（離れた語・打ち足した後に古いカードが出る）",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn hover_body(",
        must: &["md_view::render_blocks("],
        must_not: &["StyledText", "SharedString::from("],
        why: "カードの本文が `render_block` を通っていない（Markdown を生テキストで描く）",
    },
    Rule {
        file: MD_VIEW,
        decl: "pub(crate) fn render_blocks(",
        must: &["render_block(theme, base, block, code_index, sink)"],
        must_not: &[],
        why: "読むだけの md の並べ方が 1 ブロックずつ `render_block` を呼んでいない",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn hover_blocks(",
        must: &["preview::markdown_blocks(", "text: content.value.clone()"],
        must_not: &[],
        why: "Markdown のパースが md の唯一の正を通っていない / 平文を素の文字列 1 本で持っていない",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn hover_card_placement(",
        must: &["compute_menu_position("],
        must_not: &[],
        why: "カードの返しが見切れ防止の判定（`compute_menu_position`）を通っていない",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn render_lsp_hover(",
        must: &["hover_card_placement(", "hover_body(", ".occlude()"],
        must_not: &[],
        why: "カードが置き場の 1 実装・本文の 1 実装を通っていない / 下の本文へ押下を通す",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn lsp_hover_enabled_for(",
        must: &[
            "legacy()",
            "self.lsp.has_document(",
            "lsp_completion_open_in(",
            "self.pane_context_menu.is_some()",
        ],
        must_not: &[],
        why: "マウスのホバーの入口が A/B・つながっていない文書・補完の一覧・右クリックメニューを見ていない",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn show_hover_at_cursor(",
        must: &["prepare_offload(", "finish_offload_on_ui("],
        must_not: &["manager.hover(", "self.lsp.hover("],
        why: "編集メニュー / パレットの口が CLI / MCP と同じ dispatch の 3 段を通っていない",
    },
    Rule {
        file: DISPATCH,
        decl: "pub fn lsp_hover_land(",
        must: &["show_lsp_hover("],
        must_not: &[],
        why: "CLI / MCP の `show` が GUI のカードの口を通っていない",
    },
    Rule {
        file: MAIN,
        decl: "    fn show_lsp_hover(",
        must: &["self.open_lsp_hover_card("],
        must_not: &[],
        why: "host の `show_lsp_hover` がカードを出す 1 本（`open_lsp_hover_card`）を通っていない",
    },
    Rule {
        file: MAIN,
        decl: "    fn on_mouse_move(",
        must: &["self.update_lsp_hover("],
        must_not: &[],
        why: "マウス移動がホバーを更新していない（乗せてもカードが出ない）",
    },
    Rule {
        file: MAIN,
        decl: "fn handle_preview_edit_key(",
        must: &["self.route_lsp_hover_key("],
        must_not: &[],
        why: "打鍵の入口がホバーの Esc を見ていない（Esc で編集モードを抜ける）",
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
    let main = &sources.iter().find(|(f, _)| *f == MAIN).expect("main").1;
    // 6 の順序（打鍵）: 補完の表を先に引く（両方出ていれば 1 回目の Esc は補完の一覧を閉じる）
    if let Some((at, window)) = fn_window(main, "fn handle_preview_edit_key(") {
        let code = code_only(&window);
        if let (Some(completion), Some(hover)) = (
            code.find("self.route_completion_key("),
            code.find("self.route_lsp_hover_key("),
        ) {
            if hover < completion {
                let line = line_in(at, &window, "self.route_lsp_hover_key(").unwrap_or(at);
                out.push(format!(
                    "{MAIN}:{line} — ホバーの Esc を補完の表より先に見ている（両方出ているとき 1 回目の Esc がカードを閉じる）"
                ));
            }
        }
    }
    // 6 の順序（重ね順）: 補完の一覧がホバーのカードより手前（後に積む）
    let hover = main.find("self.render_lsp_hover(window, cx)");
    let completion = main.find("self.render_lsp_completion(window, cx)");
    match (hover, completion) {
        (Some(h), Some(c)) if h < c => {}
        (Some(h), _) => {
            let line = main[..h].lines().count();
            out.push(format!(
                "{MAIN}:{line} — ホバーのカードを補完の一覧より手前に積んでいる（打っている最中の一覧が隠れる）"
            ));
        }
        (None, _) => out.push(format!(
            "{MAIN}:0 — ルートの重ね物にホバーのカード（`render_lsp_hover`）が無い"
        )),
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    [MANAGER, DISPATCH, GUI, MD_VIEW, MAIN]
        .into_iter()
        .map(|f| (f, read(f)))
        .collect()
}

#[test]
fn ホバーの構造は縛りを満たす() {
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
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

#[test]
fn 逆戻りを名指しできる() {
    // A. マウスの要求が取り消しの列を通らない
    assert_named(
        "A 取り消しの列を外す",
        MANAGER,
        (
            ".then(|| (Lane::Hover, self.supersede(Lane::Hover)));",
            ".then(|| (Lane::Hover, 0));",
        ),
        "    fn hover(&self, request: &HoverRequest)",
    );
    // B. マウスの要求でも文書を開く（乗せただけでサーバが起きる）
    assert_named(
        "B マウスでも開く",
        GUI,
        ("            open: false,", "            open: true,"),
        "fn fire_lsp_hover(",
    );
    // C. 版の照合を外す（打ち足した後に古いカードを出す）
    assert_named(
        "C 版の照合を外す",
        GUI,
        (
            "            || self.hover_version(target.pane) != version\n",
            "            || { let _ = version; false }\n",
        ),
        "fn finish_lsp_hover(",
    );
    // D. 本文を `render_block` を通さずに描く
    assert_named(
        "D 生テキストで描く",
        GUI,
        (
            "    md_view::render_blocks(theme, HOVER_BASE, blocks, sink)",
            "    let _ = (theme, sink);\n    blocks.iter().map(|b| gpui::StyledText::new(format!(\"{b:?}\")).into_any_element()).collect()",
        ),
        "gpui::StyledText::new(format!",
    );
    // E. 見切れ防止の判定を外す（下へはみ出す）
    assert_named(
        "E 返しの判定を外す",
        GUI,
        (
            "    let (_, flipped_y) = crate::compute_menu_position(",
            "    let (_, flipped_y) = crate::no_flip(",
        ),
        "pub(crate) fn hover_card_placement(",
    );
    // F. 編集メニューが manager を直に叩く（CLI と経路が割れる）
    assert_named(
        "F manager を直に叩く",
        GUI,
        (
            "        let job = match tako_control::prepare_offload(self, &request) {",
            "        let _ = self.lsp.hover(&Default::default());\n        let job = match tako_control::prepare_offload(self, &request) {",
        ),
        "self.lsp.hover(&Default::default())",
    );
    // H. 右クリックメニュー（#1684）を開いていてもカードを出す（メニューを隠す）
    assert_named(
        "H メニューを見ない",
        GUI,
        (
            "            || self.pane_context_menu.is_some()\n            || self.context_menu.is_some()\n        {\n            return false;",
            "        {\n            return false;",
        ),
        "pub(crate) fn lsp_hover_enabled_for(",
    );
    // G. ホバーの Esc を補完の表より先に見る
    assert_named(
        "G Esc の順を逆にする",
        MAIN,
        (
            "        if let Some(handled) = self.route_completion_key(pane_id, keystroke, cx) {\n            return handled;\n        }\n",
            "        if self.route_lsp_hover_key(pane_id, keystroke) { return true; }\n        if let Some(handled) = self.route_completion_key(pane_id, keystroke, cx) {\n            return handled;\n        }\n",
        ),
        "        if self.route_lsp_hover_key(pane_id, keystroke) { return true; }",
    );
}

//! ホバーの続き（#1893）の構造の番犬
//!
//! # なぜ要るか
//!
//! 受け入れ条件は**どれも 1 行で壊せる**のに、壊しても答えの形（CLI の JSON）は変わらない:
//!
//! - マウスの要求を「読み込みを待たない」へ戻すと、rust-analyzer の読み込みの前半（即座に null）で
//!   カードが出ず、済んでも問い直さない（#1893 のコメントの症状）。待ちに `&abandoned` を渡さないと、
//!   なぞったあとの古いスレッドが上限（30 秒）まで残る
//! - GUI が「読み込み中」を立てない・下ろさない
//! - カードを `limit` の全文で描くと、CLI の `--full --show` で数万字の Markdown を組む
//! - 右クリックメニュー・キー・編集メニューのどれかが共通の口（`request_lsp_hover` = dispatch の 3 段）
//!   を外れると、GUI と CLI / MCP で別の操作になる
//! - キーを `bindings_for` に載せ忘れる / A/B の旧腕でもキーが残る
//!
//! 挙動は e2e（`issue1893_lsp_hover_followup`）・単体（core の `limited` / menu・control の着地・
//! keybindings の衝突）・visual-test `hover-1893` が測り、ここは**構造**を縛って file:line で名指す
//! （GUI を立てない CI でも落ちる）。
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
const CTRL_MENU: &str = "crates/tako-control/src/lsp/menu.rs";
const CORE_MENU: &str = "crates/tako-core/src/lsp/menu.rs";
const GUI: &str = "crates/tako-app/src/lsp_hover_ui.rs";
const GUI_MENU: &str = "crates/tako-app/src/lsp_menu_ui.rs";
const KEYS: &str = "crates/tako-app/src/keybindings.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

const FILES: &[&str] = &[
    MANAGER, DISPATCH, CTRL_MENU, CORE_MENU, GUI, GUI_MENU, KEYS, CLI,
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`（`];` も）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let pad = " ".repeat(indent);
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == format!("{pad}}}") || **l == format!("{pad}];"))
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
    // --- ④ 読み込み中のマウスの要求（manager の待ちを補完と共用）---
    Rule {
        file: MANAGER,
        decl: "    fn hover(&self, request: &HoverRequest)",
        must: &[
            "let wait = !request.superseding || !super::hover::legacy_1893();",
            // 空の答えの後の待ち（#1930 で定義ジャンプ・補完と共有の `wait_after_empty` へ）
            "self.wait_after_empty(&key, deadline, &abandoned, &mut empty)",
            "Err(RpcError::Timeout(_)) if self.loading_now(&key) =>",
            "waited_for_loading: waited.then(|| started.elapsed())",
        ],
        must_not: &["&|| false"],
        why: "マウスの要求が読み込みを待たない / 待ちが次の要求・閉じる・文書を閉じるで抜けない / 上限を `loading` で答えない（#1893）",
    },
    Rule {
        file: GUI,
        decl: "fn fire_lsp_hover(",
        must: &["self.lsp.server_loading(", "self.lsp_hover.loading = "],
        must_not: &[],
        why: "マウスの問い合わせを出すときにサーバが読み込み中かを見ていない（「読み込み中」が出ない）",
    },
    Rule {
        file: GUI,
        decl: "fn fire_lsp_hover(",
        must: &["ticket: self.lsp.reserve_hover()"],
        must_not: &[],
        why: "マウスの要求の取り消しの番号を UI スレッドで先に取っていない（背景が語から外れた取り消しを追い越し、読み込みが済むまで待ち続ける）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn hover(&self, request: &HoverRequest)",
        must: &["self.enter_lane(Lane::Hover, request.ticket)"],
        must_not: &[],
        why: "UI が先に取った取り消しの番号を使わずに背景で取り直している（取り消しを追い越す）",
    },
    Rule {
        file: GUI,
        decl: "fn finish_lsp_hover(",
        must: &["self.lsp_hover.loading = None"],
        must_not: &[],
        why: "答えが届いても「読み込み中」を下ろしていない",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn close_lsp_hover(",
        must: &["ui.loading = None"],
        must_not: &[],
        why: "カードを閉じても「読み込み中」が残る",
    },
    Rule {
        file: GUI,
        decl: "pub(crate) fn render_lsp_hover(",
        must: &["self.render_lsp_hover_loading("],
        must_not: &[],
        why: "「読み込み中」を描いていない",
    },
    // --- ③ 全文の口: カードはいつも既定の上限・CLI / MCP は limit ---
    Rule {
        file: GUI,
        decl: "pub(crate) fn open_lsp_hover_card(",
        must: &[".limited(Some(MAX_CHARS))"],
        must_not: &[],
        why: "カードの本文を既定の上限で切っていない（CLI の --full --show で数万字の Markdown を組む）",
    },
    Rule {
        file: DISPATCH,
        decl: "pub fn lsp_hover_land(",
        must: &["landing.limit", "add_waited("],
        must_not: &[],
        why: "CLI / MCP の応答が `limit` を通っていない / 読み込みを待った時間を返さない",
    },
    Rule {
        file: CLI,
        decl: "    fn wire_limit(&self)",
        must: &["if self.full {", "Some(0)"],
        must_not: &[],
        why: "CLI の --full が MCP の limit=0（全文）と同じ wire になっていない",
    },
    // --- ① 右クリックメニューの項目 ---
    Rule {
        file: CORE_MENU,
        decl: "    pub const ALL: [MenuItem;",
        must: &["MenuItem::Hover,"],
        must_not: &[],
        why: "右クリックメニューの項目にホバーが無い",
    },
    Rule {
        file: CTRL_MENU,
        decl: "pub fn item_request(",
        must: &["MenuItem::Hover => Request::LspHover {", "show: Some(true),"],
        must_not: &[],
        why: "「ホバー情報を表示」を押してもカードを出す要求（CLI の `--show` と同じ）にならない",
    },
    Rule {
        file: GUI_MENU,
        decl: "pub(crate) fn on_preview_body_right_click(",
        must: &["self.close_lsp_hover();"],
        must_not: &[],
        why: "右クリックでメニューを開くときに待っているマウスのホバーを捨てていない（メニューを閉じた後に古い語のホバーが発火し、明示のカードを置き換えて消す）",
    },
    Rule {
        file: GUI_MENU,
        decl: "pub(crate) fn run_lsp_menu_item(",
        must: &["MenuItem::Hover => self.request_lsp_hover("],
        must_not: &[],
        why: "右クリックメニューの「ホバー情報を表示」が編集メニュー・キーと同じ口を通っていない",
    },
    // --- ② キー ---
    Rule {
        file: KEYS,
        decl: "fn bindings_for(platform: Platform)",
        must: &["bindings.extend(hover_bindings(platform));"],
        must_not: &[],
        why: "ホバーのキーをバインド表に載せていない（押してもカードが出ない）",
    },
    Rule {
        file: KEYS,
        decl: "pub(crate) fn key_bindings()",
        must: &["legacy_1893()", "\"tako::ShowHover\""],
        must_not: &[],
        why: "A/B の旧腕（TAKO_1893_LEGACY=1）でもホバーのキーが残る",
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
    // ③ manager は全文を返す（切るのは出口）。読み取りの後に `.limited(` を挟むと、CLI の --full が
    // 全文を返せなくなる
    let manager = &sources.iter().find(|(f, _)| *f == MANAGER).unwrap().1;
    if let Some((at, window)) = fn_window(manager, "    fn hover(&self, request: &HoverRequest)") {
        let code = code_only(&window);
        if code.contains(".limited(") || code.contains("hv::truncate(") {
            let line = line_in(at, &window, ".limited(")
                .or_else(|| line_in(at, &window, "hv::truncate("))
                .unwrap_or(at);
            out.push(format!(
                "{MANAGER}:{line} — manager が本文を切っている（CLI / MCP の limit=0 で全文を返せない）"
            ));
        }
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    FILES.iter().map(|f| (*f, read(f))).collect()
}

#[test]
fn ホバーの続きの構造は縛りを満たす() {
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
    // A. マウスの要求は待たない（#1893 の前）
    assert_named(
        "A マウスは待たない",
        MANAGER,
        (
            "let wait = !request.superseding || !super::hover::legacy_1893();",
            "let wait = !request.superseding;",
        ),
        "    fn hover(&self, request: &HoverRequest)",
    );
    // B. 待ちが抜けない（古いスレッドが上限まで残る）。補完にも同じ字面があるので、ホバーの
    //    待つ条件の直後の 1 か所だけを置き換える
    assert_named(
        "B 待ちが抜けない",
        MANAGER,
        (
            "            if content.is_none() && wait {\n                match self.wait_after_empty(&key, deadline, &abandoned, &mut empty) {",
            "            if content.is_none() && wait {\n                match self.wait_after_empty(&key, deadline, &|| false /* 1893-B */, &mut empty) {",
        ),
        "/* 1893-B */",
    );
    // C. manager が本文を切る（全文を返せない）
    assert_named(
        "C manager が切る",
        MANAGER,
        (
            "            let content = hv::parse_response(&answer);",
            "            let content = hv::parse_response(&answer).map(|c| c.limited(Some(hv::MAX_CHARS)));",
        ),
        "hv::parse_response(&answer).map(|c| c.limited(",
    );
    // D. GUI が「読み込み中」を立てない
    assert_named(
        "D 読み込み中を立てない",
        GUI,
        (
            "!tako_control::lsp::hover::legacy_1893() && self.lsp.server_loading(&path);",
            "false;",
        ),
        "fn fire_lsp_hover(",
    );
    // E. カードを全文で描く
    assert_named(
        "E カードを切らない",
        GUI,
        (
            "        let content = content.limited(Some(MAX_CHARS));",
            "        let content = content.clone();",
        ),
        "pub(crate) fn open_lsp_hover_card(",
    );
    // F. CLI / MCP の応答が limit を通らない
    assert_named(
        "F limit を通らない",
        DISPATCH,
        (
            "crate::lsp::hover::found_json(answer.server, content, answer.range, landing.limit);",
            "crate::lsp::hover::found_json(answer.server, content, answer.range, None);",
        ),
        "pub fn lsp_hover_land(",
    );
    // G. メニューの項目が別の口を通る（manager を直に叩く）
    assert_named(
        "G メニューが別の口",
        GUI_MENU,
        (
            "MenuItem::Hover => self.request_lsp_hover(pane, request, cx),",
            "MenuItem::Hover => { let _ = (request, anchor); }",
        ),
        "pub(crate) fn run_lsp_menu_item(",
    );
    // H. キーをバインド表に載せない
    assert_named(
        "H キーを張らない",
        KEYS,
        ("    bindings.extend(hover_bindings(platform));\n", ""),
        "fn bindings_for(platform: Platform)",
    );
    // J. 取り消しの番号を UI で先に取らない（背景が取り消しを追い越す）
    assert_named(
        "J 番号を先に取らない",
        GUI,
        (
            "            ticket: self.lsp.reserve_hover(),",
            "            ticket: None,",
        ),
        "fn fire_lsp_hover(",
    );
    // K. manager が UI の番号を捨てて背景で取り直す
    assert_named(
        "K 番号を取り直す",
        MANAGER,
        (
            "self.enter_lane(Lane::Hover, request.ticket)",
            "self.enter_lane(Lane::Hover, Some(self.supersede(Lane::Hover)))",
        ),
        "    fn hover(&self, request: &HoverRequest)",
    );
    // L. 右クリックで待っているマウスのホバーを捨てない
    assert_named(
        "L 右クリックで捨てない",
        GUI_MENU,
        (
            "        self.close_lsp_hover();\n        let lsp = self.lsp_pane_menu_at(pane, event.position, cx);",
            "        let lsp = self.lsp_pane_menu_at(pane, event.position, cx);",
        ),
        "pub(crate) fn on_preview_body_right_click(",
    );
    // I. 項目からホバーを外す（NOT_IN_MENU へ戻す形）
    assert_named(
        "I 項目から外す",
        CORE_MENU,
        (
            "        MenuItem::Hover,\n        MenuItem::Format,",
            "        MenuItem::Format,",
        ),
        "    pub const ALL: [MenuItem;",
    );
}

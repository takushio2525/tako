//! **#1867 の番犬**: ファイルツリーの複数選択・⌥⌘V・コピーの進み具合と取り消し（FR-3.38）の
//! 要所が外れないようにする。
//!
//! ## ここで止める 8 つ
//!
//! 1. [`選択の状態遷移と範囲はcoreの1実装`] — ⌘クリック / ⇧クリックの規則を UI に書くと、
//!    CLI / MCP の `paths` と選び方の規則が割れる（単体の検査も届かない）
//! 2. [`押下の捕捉フェーズで外した選択を退避する`] — #1860 は押下のたびに選択を外す。退避しないと
//!    ⌘クリックで足す基準が消え（1 行しか選べない）、選んだ行を掴んでもまとめて運べない
//! 3. [`まとめた操作は配下を外して1件ずつ単数の口`] — フォルダと配下を重ねて渡すと 2 回写る・
//!    親を移した後に配下が「見つからない」。1 件ずつは単数と同じ口（文面・後始末が割れない）
//! 4. [`移動として貼るは移動の1実装`] — ⌥⌘V が自前で移すと、開いているペインの付け替え・
//!    同名 / 配下の断りが #1834 と割れる
//! 5. [`取り消しは作りかけを戻し残りを始めない`] — 次の項目へ進む前に印を見て、作った置き場だけを消す
//! 6. [`コピーは走っているものの一覧に載る`] — 載らない経路があると、帯にも `copy_progress` にも
//!    出ず取り消せない
//! 7. [`cliとmcpとguiが1対1`] — 設計原則 5（UI だけの複数操作・取り消しを作らない）
//! 8. [`altgrの文字はツリーが奪わない`] — Windows の AltGr は Ctrl+Alt として届く。欧州配列の
//!    AltGr+V（`@` 等）を ⌥⌘V 扱いすると、選んだ直後の文字入力がファイルを動かす
//!
//! 各規則は `Result` を返す検査関数で、本物のソースに当てる本体と、要所を消した写しに当てて
//! **file:line で名指すか**を確かめる注入（`注入_*`）を同じ関数で回す
//! （検出力の無い番犬は緑のまま腐る）。
//!
//! ## 相方
//!
//! 中身は `tako_core::tree_select` / `file_copy`（進み具合・取り消し・一覧）/ `file_move::drop_verdict_many`
//! の単体、dispatch の応答は `dispatch::tests::issue1867_*`、MCP の振り分けは `mcp::tests`、
//! 実マウス・実キーは visual-test `tree-multiselect` + `scripts/test-tree-multiselect-1867.sh`。
//! ここは**配線が外れていないこと**だけを見る

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const SELECT: &str = "crates/tako-core/src/tree_select.rs";
const COPY: &str = "crates/tako-core/src/file_copy.rs";
const KEYS: &str = "crates/tako-core/src/platform/keys.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象 1 ファイルの眺め（コメントだけ潰した `view` と、文字列も潰した `code`。同じバイト長）
struct Source {
    rel: &'static str,
    code: String,
    view: String,
}

fn source(rel: &'static str) -> Source {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    Source::from_text(rel, &src)
}

/// 関数 1 本（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: &'static str,
    name: String,
    line: usize,
    text: String,
}

type Check = Result<(), String>;

impl Source {
    fn from_text(rel: &'static str, src: &str) -> Self {
        let prod = production_range::production(src, rel);
        let source = Self {
            rel,
            code: code_view(&prod),
            view: without_comments_checked(&prod, rel),
        };
        assert_eq!(
            source.code.len(),
            source.view.len(),
            "{rel}:1 2 つの眺めのバイト長が食い違う"
        );
        source
    }

    fn body(&self, name: &str) -> Result<Body, String> {
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
            let Some(open) = self.code[start..].find('{').map(|i| i + start) else {
                break;
            };
            let mut depth = 0usize;
            let mut end = open;
            for (i, byte) in self.code[open..].bytes().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Ok(Body {
                rel: self.rel,
                name: name.to_string(),
                line: self.code[..start].lines().count() + 1,
                text: self.view[open..end].to_string(),
            });
        }
        Err(format!(
            "{}:1 `fn {name}` が見つからない。\n改名したなら番犬の名前も直すこと（#1867 の配線が外れたまま緑になる）",
            self.rel
        ))
    }

    fn line_with(&self, needle: &str) -> Option<usize> {
        self.view
            .lines()
            .position(|line| line.contains(needle))
            .map(|i| i + 1)
    }

    fn must_have(&self, needle: &str, near: &str, why: &str) -> Check {
        if self.line_with(needle).is_some() {
            return Ok(());
        }
        Err(format!(
            "{}:{} `{needle}` が無い。\n{why}",
            self.rel,
            self.line_with(near).unwrap_or(1)
        ))
    }

    /// `arm` の行から `window` 行以内に `needle` が在ること（match の腕の配線を見る）
    fn arm_calls(&self, arm: &str, needle: &str, window: usize, why: &str) -> Check {
        let Some(line) = self.line_with(arm) else {
            return Err(format!("{}:1 `{arm}` の腕が無い。\n{why}", self.rel));
        };
        let found = self
            .view
            .lines()
            .skip(line - 1)
            .take(window)
            .any(|l| l.contains(needle));
        if found {
            Ok(())
        } else {
            Err(format!(
                "{}:{line} `{arm}` の腕が `{needle}` を通っていない。\n{why}",
                self.rel
            ))
        }
    }

    fn must_not_have(&self, needle: &str, why: &str) -> Check {
        match self.line_with(needle) {
            Some(line) => Err(format!("{}:{line} `{needle}` が在る。\n{why}", self.rel)),
            None => Ok(()),
        }
    }
}

impl Body {
    fn at(&self, needle: &str) -> Option<usize> {
        self.text.find(needle)
    }

    fn must_contain(&self, needle: &str, why: &str) -> Check {
        if self.text.contains(needle) {
            return Ok(());
        }
        Err(format!(
            "{}:{} `fn {}` が `{needle}` を通っていない。\n{why}",
            self.rel, self.line, self.name
        ))
    }

    fn must_not_contain(&self, needle: &str, why: &str) -> Check {
        if !self.text.contains(needle) {
            return Ok(());
        }
        Err(format!(
            "{}:{} `fn {}` に `{needle}` が在る。\n{why}",
            self.rel, self.line, self.name
        ))
    }

    /// `first` が `then` より前に在ること（両方在る前提。無ければその旨で名指す）
    fn must_order(&self, first: &str, then: &str, why: &str) -> Check {
        match (self.at(first), self.at(then)) {
            (Some(a), Some(b)) if a < b => Ok(()),
            (a, b) => Err(format!(
                "{}:{} `fn {}` で `{first}`（{a:?}）が `{then}`（{b:?}）より前に無い。\n{why}",
                self.rel, self.line, self.name
            )),
        }
    }
}

// --- 規則（本物にも注入にも同じ関数を当てる） -------------------------------------

fn rule_select_in_core(select: &Source, sidebar: &Source) -> Check {
    sidebar.body("click_tree_row")?.must_contain(
        "tako_core::tree_select::apply(",
        "⌘クリック / ⇧クリックの状態遷移を UI が自前で決めている（正本は `tree_select::apply`）",
    )?;
    sidebar.body("tree_click_kind")?.must_contain(
        "tako_core::tree_select::click_kind(",
        "押し方（主修飾 / ⇧）の判定を UI が自前で決めている（Windows の Ctrl と macOS の ⌘ が割れる）",
    )?;
    let apply = select.body("apply")?;
    apply.must_contain(
        "range(order, &prev.anchor, row)",
        "⇧クリックの範囲が起点（anchor）から引かれていない（押した順で選ぶと Finder と違う）",
    )?;
    apply.must_contain(
        "if items.is_empty() {",
        "⌘クリックで最後の 1 行を外しても選択が残る（空の選択で ⌘C がツリーへ向く）",
    )?;
    select.body("click_kind")?.must_contain(
        "if multi_legacy() {",
        "A/B（`TAKO_1867_LEGACY=1`）が押し方の判定に効いていない（同一バイナリで旧挙動へ戻せない）",
    )
}

fn rule_stash(app: &Source, sidebar: &Source) -> Check {
    app.must_have(
        "this.stash_tree_selection()",
        "capture_any_mouse_down",
        "押下の捕捉フェーズが選択を退避せずに捨てている（⌘クリックで足せない・まとめて運べない）",
    )?;
    sidebar.body("stash_tree_selection")?.must_contain(
        "self.tree_multi.stash = self.tree_selection.take()",
        "退避していない（捕捉フェーズの後で行が前の選択を見られない）",
    )?;
    sidebar.body("click_tree_row")?.must_contain(
        ".tree_selection_base()",
        "⌘クリックが退避した選択を基準にしていない（捕捉フェーズで外れた後の空から足すので 1 行しか選べない）",
    )?;
    let base = sidebar.body("tree_selection_base")?;
    for (needle, why) in [
        (
            "sel.pane == self.focused_pane()",
            "退避した選択を、選んだときのフォーカスペインと比べていない（別のペインの古い選択へ足す）",
        ),
        (
            "sel.tab == self.workspace.active_tab_id()",
            "退避した選択を、選んだときのタブと比べていない（別のタブの古い選択へ足す）",
        ),
    ] {
        base.must_contain(needle, why)?;
    }
    sidebar.must_have(
        "this.keep_tree_selection_for(&ctx_path)",
        "fn keep_tree_selection_for",
        "選んだ行の押下・右クリックで選択を保っていない（掴んでも 1 件しか運ばない・メニューが 1 件にしか効かない）",
    )
}

fn rule_many(dispatch: &Source) -> Check {
    let many = dispatch.body("file_op_many")?;
    many.must_order(
        "distinct_roots(",
        "match op {",
        "まとめた操作が配下を外す前に始まっている（フォルダと配下を重ねると 2 回写る・「見つからない」）",
    )?;
    for (needle, why) in [
        (
            "clipboard_put(host, &targets",
            "まとめたクリップボードが単数と同じ口（`clipboard_put`）を通っていない",
        ),
        (
            "trash_one(path)",
            "まとめたごみ箱が単数と同じ口（`trash_one`）を通っていない",
        ),
        (
            "run_file_move(host, path",
            "まとめた移動が #1834 の移動の 1 実装を通っていない（付け替え・断りの文面が割れる）",
        ),
    ] {
        many.must_contain(needle, why)?;
    }
    for needle in ["std::fs::rename(", "file_move::execute(", "move_to_trash("] {
        many.must_not_contain(
            needle,
            "まとめた操作が自前で移す / 消す（単数の口と後始末・文面が割れる）",
        )?;
    }
    dispatch.arm_calls(
        "Request::FileOpMany { op, paths, dest } =>",
        "file_op_many(",
        2,
        "`FileOpMany` が dispatch の 1 実装へ写っていない",
    )
}

fn rule_paste_move(dispatch: &Source) -> Check {
    let why = "⌥⌘V が #1834 の移動の 1 実装（`run_paste_cut` → `run_file_move`）を通っていない";
    dispatch.arm_calls("FileOpKind::PasteMove =>", "paste_plan(", 3, why)?;
    dispatch.arm_calls("FileOpKind::PasteMove =>", "ClipMode::Cut", 3, why)?;
    dispatch.arm_calls(
        "FileOpKind::PasteMove =>",
        "run_paste_cut(host, &plan)",
        4,
        why,
    )
}

fn rule_cancel(copy: &Source, dispatch: &Source) -> Check {
    let step = copy.body("step")?;
    step.must_contain(
        "self.progress.is_cancelled()",
        "写す途中で取り消しを見ていない（帯の「取り消し」を押しても最後まで写す）",
    )?;
    step.must_contain(
        "CopyRefusal::Cancelled",
        "取り消しを失敗として返していない（作りかけを戻す道へ入らない）",
    )?;
    copy.body("copy_dir")?.must_order(
        "self.step()?",
        "self.copy_file(&from, &to)",
        "フォルダの中の 1 件ずつの前で取り消しを見ていない",
    )?;
    copy.body("copy_root")?.must_order(
        "self.step()?",
        "std::fs::create_dir(&plan.to)",
        "置き場を作る前に取り消しを見ていない（取り消した後に空の置き場が残る）",
    )?;
    copy.body("execute_with")?.must_contain(
        "if job.created_root {",
        "取り消し・失敗で作った置き場を戻していない（作りかけが残る）",
    )?;
    dispatch.body("run_copy_items")?.must_order(
        "progress.is_cancelled()",
        "run_file_copy(src, dir, naming, progress)",
        "取り消した後も残りの項目を写し始める",
    )
}

fn rule_ticket(dispatch: &Source) -> Check {
    let why =
        "コピーが走っているものの一覧に載らない（帯にも `copy_progress` にも出ず、取り消せない）";
    dispatch
        .body("new")?
        .must_contain("copy_ticket(&items)", why)?;
    dispatch.arm_calls("FileOpKind::Copy =>", "copy_ticket(", 8, why)?;
    dispatch.must_have(
        "run_copy_items(&job.items, job.naming, job.ticket.progress())",
        "OffloadJob::FileCopy(job) =>",
        "background のコピーが札の進み具合を進めていない（帯が 0 のまま）",
    )?;
    dispatch.body("copy_ticket")?.must_contain(
        "tako_core::file_copy::jobs().register(",
        "札をプロセスの一覧（GUI・CLI・MCP が読むもの）へ載せていない",
    )
}

fn rule_one_to_one(
    dispatch: &Source,
    mcp: &Source,
    catalog: &Source,
    cli: &Source,
    sidebar: &Source,
) -> Check {
    let why = "複数選択・⌥⌘V・進み具合・取り消しが CLI / MCP / GUI の 1 つの口を通っていない（設計原則 5）";
    dispatch.arm_calls("FileOpKind::CopyProgress =>", "copy_progress_json(", 1, why)?;
    dispatch.arm_calls("FileOpKind::CopyCancel =>", "copy_cancel(", 1, why)?;
    for (wire, kind) in [
        ("paste_move", "PasteMove"),
        ("copy_progress", "CopyProgress"),
        ("copy_cancel", "CopyCancel"),
    ] {
        mcp.must_have(
            &format!(r#""{wire}" => crate::protocol::FileOpKind::{kind}"#),
            "\"tako_file_op\"",
            &format!("MCP `tako_file_op` の op={wire} が dispatch へ写っていない"),
        )?;
    }
    mcp.must_have(
        "return Ok(Request::FileOpMany {",
        "\"tako_file_op\"",
        "MCP `tako_file_op` の `paths` がまとめた要求へ写っていない",
    )?;
    catalog.must_have(
        r#""paste_move","copy_progress","copy_cancel"]"#,
        "\"tako_file_op\"",
        "カタログの op の enum に paste_move / copy_progress / copy_cancel が無い（AI が選べない）",
    )?;
    catalog.must_have(
        r#""paths": { "type": "array""#,
        "\"tako_file_op\"",
        "カタログに `paths` が無い（AI がまとめて扱えない）",
    )?;
    cli.body("file_op_for")?.must_contain(
        "Request::FileOpMany {",
        "CLI の複数指定がまとめた要求になっていない（MCP の `paths` と応答が割れる）",
    )?;
    for needle in [
        "FileOpKind::PasteMove",
        "FileOpKind::CopyProgress",
        "FileOpKind::CopyCancel",
    ] {
        cli.must_have(
            needle,
            "enum FileCommand",
            "CLI `tako file paste --move` / `progress` / `cancel` が dispatch の op を通っていない",
        )?;
    }
    sidebar
        .body("tree_clip_action")?
        .must_contain("Request::FileOpMany {", why)?;
    sidebar
        .body("tree_clip_action")?
        .must_contain("TreeClipKey::PasteMove", why)?;
    sidebar
        .body("dispatch_tree_many")?
        .must_contain("tako_control::dispatch(", why)?;
    sidebar
        .body("cancel_copy")?
        .must_contain("FileOpKind::CopyCancel", why)?;
    sidebar.must_not_have(
        "jobs().cancel(",
        "帯の「取り消し」が dispatch を通らず一覧を直に触っている（CLI / MCP の取り消しと応答が割れる）",
    )
}

fn rule_altgr(keys: &Source, sidebar: &Source) -> Check {
    keys.body("tree_clip_key")?.must_contain(
        "platform == Platform::Windows && text",
        "Windows の AltGr（Ctrl+Alt）が生む文字を ⌥⌘V 扱いしている（欧州配列の @ でファイルが動く）",
    )?;
    sidebar.body("handle_tree_clip_keystroke")?.must_contain(
        "ks.key_char.as_deref()",
        "打鍵が文字を生むかをキーの判定へ渡していない（AltGr の文字を奪う）",
    )
}

// --- 本体 ---------------------------------------------------------------------------

fn ok(check: Check) {
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[test]
fn 選択の状態遷移と範囲はcoreの1実装() {
    ok(rule_select_in_core(&source(SELECT), &source(SIDEBAR)));
}

#[test]
fn 押下の捕捉フェーズで外した選択を退避する() {
    ok(rule_stash(&source(APP), &source(SIDEBAR)));
}

#[test]
fn まとめた操作は配下を外して1件ずつ単数の口() {
    ok(rule_many(&source(DISPATCH)));
}

#[test]
fn 移動として貼るは移動の1実装() {
    ok(rule_paste_move(&source(DISPATCH)));
}

#[test]
fn 取り消しは作りかけを戻し残りを始めない() {
    ok(rule_cancel(&source(COPY), &source(DISPATCH)));
}

#[test]
fn コピーは走っているものの一覧に載る() {
    ok(rule_ticket(&source(DISPATCH)));
}

#[test]
fn cliとmcpとguiが1対1() {
    ok(rule_one_to_one(
        &source(DISPATCH),
        &source(MCP_REQUEST),
        &source(MCP_CATALOG),
        &source(CLI),
        &source(SIDEBAR),
    ));
}

#[test]
fn altgrの文字はツリーが奪わない() {
    ok(rule_altgr(&source(KEYS), &source(SIDEBAR)));
}

// --- 注入: 要所を消した写しで、規則が file:line を名指して落ちるか -------------------

/// `rel` の本文から `from` を `to` へ 1 か所書き換えた写し
fn injected(rel: &'static str, from: &str, to: &str) -> Source {
    let src = std::fs::read_to_string(repo_root().join(rel)).expect("読める");
    assert!(
        src.contains(from),
        "{rel}:1 注入の目印 `{from}` が本文に無い（番犬の注入を直すこと）"
    );
    Source::from_text(rel, &src.replacen(from, to, 1))
}

fn must_name(check: Check, rel: &str, what: &str) {
    let err = check.expect_err("要所を消した写しでも規則が通った = 検出力が無い");
    assert!(
        err.starts_with(&format!("{rel}:")) && err.contains(what),
        "名指しが file:line + 対象になっていない: {err}"
    );
}

#[test]
fn 注入_uiが状態遷移を自前で決めると名指す() {
    let sidebar = injected(
        SIDEBAR,
        "let next = tako_core::tree_select::apply(base.as_ref(), path, kind, &order);",
        "let next = Some(tako_core::tree_select::Selection::single(path));",
    );
    must_name(
        rule_select_in_core(&source(SELECT), &sidebar),
        SIDEBAR,
        "fn click_tree_row",
    );
    let select = injected(
        SELECT,
        "                if items.is_empty() {\n                    return None;\n                }",
        "",
    );
    must_name(
        rule_select_in_core(&select, &source(SIDEBAR)),
        SELECT,
        "fn apply",
    );
}

#[test]
fn 注入_捕捉フェーズで選択を捨てると名指す() {
    let app = injected(
        APP,
        "if this.stash_tree_selection() {",
        "if this.tree_selection.take().is_some() {",
    );
    let err = rule_stash(&app, &source(SIDEBAR)).expect_err("退避しなくても通った");
    assert!(err.starts_with(&format!("{APP}:")), "{err}");
    let sidebar = injected(
        SIDEBAR,
        "            .tree_selection_base()\n            .filter(|sel| !sel.remote)",
        "            .active_tree_selection()\n            .filter(|sel| !sel.remote)",
    );
    must_name(
        rule_stash(&source(APP), &sidebar),
        SIDEBAR,
        "fn click_tree_row",
    );
}

#[test]
fn 注入_配下を外さず自前で移すと名指す() {
    let dispatch = injected(
        DISPATCH,
        "let (targets, skipped) = tako_core::tree_select::distinct_roots(&all);",
        "let (targets, skipped) = (all.clone(), Vec::<PathBuf>::new());",
    );
    must_name(rule_many(&dispatch), DISPATCH, "fn file_op_many");
    let dispatch = injected(
        DISPATCH,
        ".map(|path| (path.clone(), trash_one(path)))",
        ".map(|path| (path.clone(), crate::platform::os_integration::move_to_trash(path).map(|_| Value::Null).map_err(op_err)))",
    );
    must_name(rule_many(&dispatch), DISPATCH, "fn file_op_many");
}

#[test]
fn 注入_移動として貼るが自前で移すと名指す() {
    let dispatch = injected(
        DISPATCH,
        "                    plan.clip.mode = tako_core::file_clipboard::ClipMode::Cut;\n                    run_paste_cut(host, &plan)",
        "                    let _ = &mut plan;\n                    std::fs::rename(&path, &path).map(|_| Value::Null).map_err(op_err)",
    );
    must_name(
        rule_paste_move(&dispatch),
        DISPATCH,
        "FileOpKind::PasteMove =>",
    );
}

#[test]
fn 注入_取り消しを見ないと名指す() {
    let copy = injected(
        COPY,
        "        if self.progress.is_cancelled() {\n            return Err(CopyRefusal::Cancelled);\n        }",
        "",
    );
    must_name(rule_cancel(&copy, &source(DISPATCH)), COPY, "fn step");
    let copy = injected(
        COPY,
        "        for entry in entries {\n            self.step()?;",
        "        for entry in entries {",
    );
    must_name(rule_cancel(&copy, &source(DISPATCH)), COPY, "fn copy_dir");
    let dispatch = injected(
        DISPATCH,
        "            if progress.is_cancelled() {\n                return Err(tako_core::file_copy::CopyRefusal::Cancelled);\n            }",
        "",
    );
    must_name(
        rule_cancel(&source(COPY), &dispatch),
        DISPATCH,
        "fn run_copy_items",
    );
}

#[test]
fn 注入_一覧に載せないと名指す() {
    let dispatch = injected(
        DISPATCH,
        "        let ticket = copy_ticket(&items);",
        "        let ticket = tako_core::file_copy::CopyJobs::new().register(Vec::new(), PathBuf::new());",
    );
    must_name(rule_ticket(&dispatch), DISPATCH, "fn new");
}

#[test]
fn 注入_mcpかuiの口が外れると名指す() {
    let mcp = injected(
        MCP_REQUEST,
        r#""paste_move" => crate::protocol::FileOpKind::PasteMove,"#,
        r#""paste_move" => crate::protocol::FileOpKind::Paste,"#,
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &mcp,
        &source(MCP_CATALOG),
        &source(CLI),
        &source(SIDEBAR),
    )
    .expect_err("MCP の paste_move が別の op へ写っても通った");
    assert!(err.starts_with(&format!("{MCP_REQUEST}:")), "{err}");
    let sidebar = injected(
        SIDEBAR,
        "        let result = tako_control::dispatch(\n            self,\n            tako_control::protocol::Request::FileOp {\n                op: tako_control::protocol::FileOpKind::CopyCancel,",
        "        tako_core::file_copy::jobs().cancel(Some(id));\n        let result = tako_control::dispatch(\n            self,\n            tako_control::protocol::Request::FileOp {\n                op: tako_control::protocol::FileOpKind::CopyProgress,",
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &source(MCP_REQUEST),
        &source(MCP_CATALOG),
        &source(CLI),
        &sidebar,
    )
    .expect_err("帯の取り消しが dispatch を通らなくても通った");
    assert!(err.starts_with(&format!("{SIDEBAR}:")), "{err}");
}

#[test]
fn 注入_altgrの文字を奪うと名指す() {
    let keys = injected(
        KEYS,
        "let altgr_text = platform == Platform::Windows && text;",
        "let altgr_text = false;",
    );
    must_name(
        rule_altgr(&keys, &source(SIDEBAR)),
        KEYS,
        "fn tree_clip_key",
    );
}

#[test]
fn 番犬が見るファイルが在る() {
    for rel in [
        SELECT,
        COPY,
        KEYS,
        DISPATCH,
        MCP_REQUEST,
        MCP_CATALOG,
        CLI,
        APP,
        SIDEBAR,
    ] {
        assert!(
            repo_root().join(rel).is_file(),
            "{rel}:1 が無い（改名したなら番犬も直す）"
        );
    }
}

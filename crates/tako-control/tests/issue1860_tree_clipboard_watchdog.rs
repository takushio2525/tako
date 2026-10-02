//! **#1860 の番犬**: ファイルツリーのコピー / 切り取り / 貼り付け（FR-3.33）の要所が外れないようにする。
//!
//! ## ここで止める 8 つ
//!
//! 1. [`同名は上書きせず排他的に作る`] — 同名があれば別名へ逃がし、置き場は `create_dir` /
//!    `create_new` で押さえる（`std::fs::copy` は既存のファイルを黙って上書きする）
//! 2. [`失敗したら作った置き場だけを戻す`] — 途中で失敗したら写した先だけを消す。
//!    **コピー元を消す道を作らない**（戻す関数がコピー元を触ったら名指す）
//! 3. [`自分の配下へは実体の形で断る`] — 字面で比べると `..`・リンク・大文字小文字で素通りし、
//!    写した先をまた写して終わらない
//! 4. [`切り取りの貼り付けは移動の1実装`] — 切り取り → 貼り付けは #1834 の `run_file_move`。
//!    2 つ目の移動（自前の `rename`）を作ると付け替え・断りの文面が割れる
//! 5. [`コピーと移動の実行はdispatchの経路だけ`] — GUI が `file_copy::execute` を直に呼ばない
//! 6. [`cliとmcpとguiが1対1`] — 設計原則 5（UI だけのコピーを作らない）
//! 7. [`ユーザーのクリップボードを書くのはguiのホストだけ`] — テスト・dispatch・CLI が
//!    OS のクリップボードを直に書くと、`cargo test` がユーザーのクリップボードを上書きする
//! 8. [`キーは選んでいる間だけツリーへ向く`] — 選んだときのタブ・フォーカスペインと比べずに
//!    ⌘C / ⌘V を奪うと、端末のコピー・貼り付け（Windows は Ctrl+C = SIGINT）が死ぬ
//!
//! 各規則は `Result` を返す検査関数で、本物のソースに当てる本体と、要所を消した写しに当てて
//! **file:line で名指すか**を確かめる注入（`注入_*`）を同じ関数で回す
//! （検出力の無い番犬は緑のまま腐る）。
//!
//! ## 相方
//!
//! 中身は `tako_core::file_copy` / `file_clipboard` の単体、dispatch の応答は
//! `dispatch::tests::issue1860_*`、OS のクリップボードは `platform::file_clipboard` の単体、
//! 実マウス・実キー・Finder 相当との往復は visual-test `tree-clipboard` +
//! `scripts/test-tree-clipboard-1860.sh`。ここは**配線が外れていないこと**だけを見る

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const COPY: &str = "crates/tako-core/src/file_copy.rs";
const CLIP: &str = "crates/tako-core/src/file_clipboard.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const HOST: &str = "crates/tako-control/src/host.rs";
const PLATFORM_CLIP: &str = "crates/tako-control/src/platform/file_clipboard.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const FILETREE: &str = "crates/tako-app/src/filetree.rs";

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
    /// 本文から眺めを作る（注入では書き換えた本文を渡す）
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
            "{}:1 `fn {name}` が見つからない。\n改名したなら番犬の名前も直すこと（#1860 の配線が外れたまま緑になる）",
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
                "{}:{line} `{arm}` の腕が `{needle}` を呼んでいない。\n{why}",
                self.rel
            ))
        }
    }

    /// 本番の範囲に `needle` が在れば、その行を名指す
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

fn rule_no_overwrite(core: &Source) -> Check {
    core.body("free_name")?.must_contain(
        "if !exists_no_follow(&dest_dir.join(name)) {",
        "同名が無いときだけ元の名前を使う、になっていない（同名の上へ写すと上書きする。#1860）",
    )?;
    core.body("free_name")?.must_contain(
        "copy_name(",
        "同名のときに別名（Finder の「のコピー」）を作っていない",
    )?;
    core.body("plan")?
        .must_contain("free_name(", "段取りが名前を決めていない")?;
    let root = core.body("copy_root")?;
    root.must_order(
        ".create_new(true)",
        "self.copy_file(&plan.from, &plan.to)",
        "ファイルの置き場を `create_new` で押さえてから写していない（決めてから作るまでに\n\
         できた同名を `std::fs::copy` が黙って上書きする）",
    )?;
    root.must_contain(
        "std::fs::create_dir(&plan.to)",
        "フォルダの置き場を排他的に作っていない（`create_dir_all` は既存のフォルダへ混ぜる）",
    )?;
    core.body("copy_dir")?.must_order(
        "if exists_no_follow(&to) {",
        "self.copy_file(&from, &to)",
        "フォルダの中の同名（大文字小文字だけ違う名前）を見ずに写している",
    )?;
    core.body("exists_no_follow")?.must_contain(
        "symlink_metadata(",
        "同名の検査がリンクを辿っている（壊れたリンクの上へ黙って上書きする）",
    )
}

fn rule_rollback(core: &Source) -> Check {
    core.body("execute")?.must_contain(
        "if job.created_root {",
        "戻すのが「この呼び出しで置き場を作れたとき」に限られていない（呼ぶ前からあったものを消す）",
    )?;
    let rollback = core.body("rollback")?;
    rollback.must_contain("&plan.to", "戻す対象が写した先になっていない")?;
    rollback.must_not_contain(
        "plan.from",
        "戻すときにコピー元を触っている（失敗したコピーで元のファイルが消える）",
    )
}

fn rule_into_self(core: &Source) -> Check {
    let plan = core.body("plan")?;
    plan.must_contain(
        "lexical_verdict(&from_real, &dest_real)",
        "自分自身・配下の判定を実体の形で行っていない（`..`・リンク・大文字小文字で素通りし、\n\
         写した先をまた写して終わらない）",
    )?;
    plan.must_contain("canonicalize(", "実体の形を作っていない")?;
    plan.must_contain(
        "CopyRefusal::IntoDescendant",
        "配下へのコピーを断っていない",
    )
}

fn rule_cut_is_move(dispatch: &Source) -> Check {
    let cut = dispatch.body("run_paste_cut")?;
    cut.must_contain(
        "run_file_move(host, src, dir)",
        "切り取りの貼り付けが #1834 の移動の 1 実装を通っていない（付け替え・同名 / 配下 / 別のボリュームの\n\
         断りが割れる）",
    )?;
    for needle in ["std::fs::rename(", "file_move::execute("] {
        cut.must_not_contain(
            needle,
            "切り取りの貼り付けが自前で移している（2 つ目の移動実装 = 開いているペインが付け替わらない）",
        )?;
    }
    cut.must_contain(
        "after_cut_paste(",
        "移せたものをクリップボードから抜いていない（移した後も古い場所を指したまま貼れてしまう）",
    )
}

fn rule_one_to_one(
    dispatch: &Source,
    mcp: &Source,
    catalog: &Source,
    cli: &Source,
    sidebar: &Source,
) -> Check {
    let why =
        "コピー / 切り取り / 貼り付けが CLI / MCP / GUI の 1 つの口を通っていない（設計原則 5）";
    dispatch.arm_calls("FileOpKind::Copy =>", "run_file_copy(", 10, why)?;
    dispatch.arm_calls("FileOpKind::ClipboardCopy =>", "clipboard_put(", 3, why)?;
    dispatch.arm_calls("FileOpKind::ClipboardCut =>", "clipboard_put(", 3, why)?;
    dispatch.arm_calls("FileOpKind::Paste =>", "paste_plan(", 3, why)?;
    let prepare = dispatch.body("prepare_offload")?;
    prepare.must_contain(
        "op: FileOpKind::Copy",
        "IPC のコピーが background へ出ていない（大きなフォルダの複製で UI が止まる）",
    )?;
    prepare.must_contain(
        "op: FileOpKind::Paste",
        "IPC の貼り付け（コピー）が background へ出ていない",
    )?;
    for (wire, kind) in [
        ("copy", "Copy"),
        ("clipboard_copy", "ClipboardCopy"),
        ("clipboard_cut", "ClipboardCut"),
        ("clipboard", "Clipboard"),
        ("paste", "Paste"),
    ] {
        mcp.must_have(
            &format!(r#""{wire}" => crate::protocol::FileOpKind::{kind}"#),
            "\"tako_file_op\"",
            &format!("MCP `tako_file_op` の op={wire} が dispatch へ写っていない"),
        )?;
    }
    catalog.must_have(
        r#""move","copy","clipboard_copy","clipboard_cut","clipboard","paste"]"#,
        "\"tako_file_op\"",
        "カタログの op の enum に copy / clipboard_* / paste が無い（AI が選べない）",
    )?;
    cli.body("file_copy_requests")?.must_contain(
        "op: tako_control::protocol::FileOpKind::Copy",
        "CLI `tako file copy` が dispatch の Copy を通っていない",
    )?;
    cli.body("file_copy_requests")?.must_contain(
        "resolve_cli_path(",
        "CLI `tako file copy` が CLI の cwd 基準で絶対化していない（GUI の cwd で読まれる）",
    )?;
    for needle in [
        "FileOpKind::ClipboardCopy",
        "FileOpKind::ClipboardCut",
        "FileOpKind::Clipboard,",
        "FileOpKind::Paste",
    ] {
        cli.must_have(
            needle,
            "FileClipboardCommand",
            "CLI `tako file clipboard` / `tako file paste` が dispatch の op を通っていない",
        )?;
    }
    let action = sidebar.body("tree_clip_action")?;
    action.must_contain("tako_control::dispatch(", why)?;
    action.must_contain("FileOpKind::ClipboardCopy", why)?;
    action.must_contain("FileOpKind::ClipboardCut", why)?;
    let paste = sidebar.body("tree_paste")?;
    paste.must_contain("tako_control::prepare_offload(", why)?;
    paste.must_contain("FileOpKind::Paste", why)?;
    for needle in ["file_copy::execute(", "file_copy::plan(", "std::fs::copy("] {
        sidebar.must_not_have(
            needle,
            "ツリーの UI が自分でファイルを写している。コピーは dispatch `FileOp` の 1 つ",
        )?;
    }
    Ok(())
}

fn rule_clipboard_owner(host: &Source) -> Check {
    host.body("os_file_clipboard_write")?.must_not_contain(
        "file_clipboard::write(",
        "ホストの既定が OS のクリップボードへ書いている（テストのホストがユーザーのクリップボードを上書きする）",
    )
}

fn rule_keys_while_selected(app: &Source, sidebar: &Source) -> Check {
    let selection = sidebar.body("active_tree_selection")?;
    for (needle, why) in [
        (
            "sel.pane == self.focused_pane()",
            "選んだときのフォーカスペインと比べていない（ペインへ移った後も ⌘C / ⌘V をツリーが奪う）",
        ),
        (
            "sel.tab == self.workspace.active_tab_id()",
            "選んだときのタブと比べていない（別のタブで ⌘V がツリーへ貼る）",
        ),
        (
            "self.filetree.visible",
            "ツリーを閉じても選択が効いたまま",
        ),
    ] {
        selection.must_contain(needle, why)?;
    }
    app.body("handle_key")?.must_order(
        "self.handle_tree_clip_keystroke(",
        "self.handle_preview_edit_key(",
        "ツリーの打鍵の振り分けが編集・端末より後ろにある（選んだ行へ ⌘X が届かない）",
    )?;
    app.body("handle_key")?.must_contain(
        "self.tree_selection.take()",
        "ほかの打鍵で選択を外していない（打ち始めた後の ⌘V がツリーへ貼る）",
    )?;
    app.body("copy_selection")?.must_contain(
        "self.active_tree_selection()",
        "⌘C（アクション）がツリーの選択を見ていない",
    )?;
    app.body("paste")?.must_order(
        "self.text_input_swallows_keys()",
        "self.active_tree_selection()",
        "⌘V がツリーへ向く前に入力欄（パレット・インライン入力）を見ていない",
    )
}

fn rule_tree_refresh(filetree: &Source, app: &Source) -> Check {
    filetree.body("note_copied")?.must_contain(
        "self.mark_local_read(",
        "写した先の読み直しに世代を付けていない（古いポーリングの一覧で貼ったものが消える）",
    )?;
    filetree.body("apply_refresh_since")?.must_contain(
        "*read <= since",
        "読み直しより前に読み始めたポーリングの結果を捨てていない",
    )?;
    app.must_have(
        "apply_refresh_since(results, since)",
        "filetree::scan_dirs",
        "2 秒ポーリングが世代つきで当てていない（貼ったものが次のポーリングまで消える）",
    )
}

// --- 本体 ---------------------------------------------------------------------------

fn ok(check: Check) {
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[test]
fn 同名は上書きせず排他的に作る() {
    ok(rule_no_overwrite(&source(COPY)));
}

#[test]
fn 失敗したら作った置き場だけを戻す() {
    ok(rule_rollback(&source(COPY)));
}

#[test]
fn 自分の配下へは実体の形で断る() {
    ok(rule_into_self(&source(COPY)));
}

#[test]
fn 切り取りの貼り付けは移動の1実装() {
    ok(rule_cut_is_move(&source(DISPATCH)));
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
fn キーは選んでいる間だけツリーへ向く() {
    ok(rule_keys_while_selected(&source(APP), &source(SIDEBAR)));
}

#[test]
fn 貼ったものを古いポーリングの一覧で消さない() {
    ok(rule_tree_refresh(&source(FILETREE), &source(APP)));
}

#[test]
fn ユーザーのクリップボードを書くのはguiのホストだけ() {
    ok(rule_clipboard_owner(&source(HOST)));
    let root = repo_root();
    let mut offenders = Vec::new();
    for krate in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let dir = root.join("crates").join(krate).join("src");
        for path in walk_rs(&dir) {
            let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy();
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            offenders.extend(offenders_of(&rel, &src, "file_clipboard::write(", &[APP]));
        }
    }
    assert!(
        offenders.is_empty(),
        "GUI のホスト以外が OS のクリップボードへ書いている:\n{}\n\n\
         書いてよいのは tako-app の `os_file_clipboard_write`（dispatch はホスト経由で書く = \
         テストのホストはユーザーのクリップボードを触らない。#1860）",
        offenders.join("\n")
    );
}

/// コピーの実行（`file_copy::execute(`）を呼んでよいのは core 自身と dispatch の `run_file_copy` だけ
#[test]
fn コピーと移動の実行はdispatchの経路だけ() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for krate in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let dir = root.join("crates").join(krate).join("src");
        for path in walk_rs(&dir) {
            let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy();
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            offenders.extend(offenders_of(
                &rel,
                &src,
                "file_copy::execute(",
                &[COPY, DISPATCH],
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "dispatch の `run_file_copy` 以外がコピーを実行している:\n{}\n\n\
         コピーは dispatch `FileOpKind::Copy` / `Paste` の経路（別名・後始末・応答の文面がそこで揃う。#1860）",
        offenders.join("\n")
    );
    ok(rule_cut_is_move(&source(DISPATCH)));
}

/// 走査で得たリポジトリ相対パスを、定数と同じ `/` 区切りへ揃える（Windows の `\\` = #1834 の教訓）
fn slash_rel(rel: &str) -> String {
    rel.replace('\\', "/")
}

/// 規則: `needle` を呼んでよいのは `allowed` のファイルだけ。違反は `/` 区切りの
/// `file:line 行の中身` で返す（走査の本体と注入が同じ関数を通る）
fn offenders_of(rel: &str, src: &str, needle: &str, allowed: &[&str]) -> Vec<String> {
    let rel = slash_rel(rel);
    if allowed.contains(&rel.as_str()) || !src.contains(needle) {
        return Vec::new();
    }
    let prod = production_range::scan(src).text;
    let view = without_comments_checked(&prod, &rel);
    view.lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(i, line)| format!("{rel}:{} {}", i + 1, line.trim()))
        .collect()
}

fn walk_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_rs(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
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

fn must_name(check: Check, rel: &str, fn_name: &str) {
    let err = check.expect_err("要所を消した写しでも規則が通った = 検出力が無い");
    assert!(
        err.starts_with(&format!("{rel}:")) && err.contains(fn_name),
        "名指しが file:line + 関数名になっていない: {err}"
    );
}

#[test]
fn 注入_同名で上書きさせると名指す() {
    let core = injected(
        COPY,
        "    if !exists_no_follow(&dest_dir.join(name)) {",
        "    if true {",
    );
    must_name(rule_no_overwrite(&core), COPY, "fn free_name");
    let core = injected(COPY, ".create_new(true)", ".create(true)");
    must_name(rule_no_overwrite(&core), COPY, "fn copy_root");
    let core = injected(
        COPY,
        "                if exists_no_follow(&to) {",
        "                if false {",
    );
    must_name(rule_no_overwrite(&core), COPY, "fn copy_dir");
}

#[test]
fn 注入_戻すときにコピー元を消すと名指す() {
    let core = injected(
        COPY,
        "EntryKind::Dir => std::fs::remove_dir_all(&plan.to),",
        "EntryKind::Dir => std::fs::remove_dir_all(&plan.from),",
    );
    must_name(rule_rollback(&core), COPY, "fn rollback");
    let core = injected(
        COPY,
        "            if job.created_root {",
        "            if true {",
    );
    must_name(rule_rollback(&core), COPY, "fn execute");
}

#[test]
fn 注入_配下の判定を字面へ戻すと名指す() {
    let core = injected(
        COPY,
        "lexical_verdict(&from_real, &dest_real)",
        "lexical_verdict(src, dest_dir)",
    );
    must_name(rule_into_self(&core), COPY, "fn plan");
}

#[test]
fn 注入_切り取りを別実装のrenameにすると名指す() {
    let dispatch = injected(
        DISPATCH,
        "        match run_file_move(host, src, dir) {",
        "        match std::fs::rename(src, dir.join(src.file_name().unwrap_or_default()))\n            .map(|_| Value::Null)\n            .map_err(op_err)\n        {",
    );
    must_name(rule_cut_is_move(&dispatch), DISPATCH, "fn run_paste_cut");
}

#[test]
fn 注入_uiが自分で写すかmcpの口が外れると名指す() {
    let sidebar = injected(
        SIDEBAR,
        "        match tako_control::prepare_offload(self, &request) {",
        "        let _ = tako_core::file_copy::execute(&todo!());\n        match tako_control::prepare_offload(self, &request) {",
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &source(MCP_REQUEST),
        &source(MCP_CATALOG),
        &source(CLI),
        &sidebar,
    )
    .expect_err("UI が自分で写しても通った");
    assert!(err.starts_with(&format!("{SIDEBAR}:")), "{err}");
    let mcp = injected(
        MCP_REQUEST,
        r#""paste" => crate::protocol::FileOpKind::Paste,"#,
        r#""paste" => crate::protocol::FileOpKind::Move,"#,
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &mcp,
        &source(MCP_CATALOG),
        &source(CLI),
        &source(SIDEBAR),
    )
    .expect_err("MCP の paste が別の op へ写っても通った");
    assert!(err.starts_with(&format!("{MCP_REQUEST}:")), "{err}");
}

#[test]
fn 注入_選んだペインと比べないと名指す() {
    let sidebar = injected(
        SIDEBAR,
        "                && sel.pane == self.focused_pane()",
        "",
    );
    must_name(
        rule_keys_while_selected(&source(APP), &sidebar),
        SIDEBAR,
        "fn active_tree_selection",
    );
    let app = injected(
        APP,
        "        if self.tree_selection.take().is_some() {",
        "        if false {",
    );
    must_name(
        rule_keys_while_selected(&app, &source(SIDEBAR)),
        APP,
        "fn handle_key",
    );
}

#[test]
fn 注入_ホストの既定が本物へ書くと名指す() {
    let host = injected(
        HOST,
        "        Err(\"このホストは OS のクリップボードを使わない\".into())",
        "        crate::platform::file_clipboard::write(_paths, _cut)",
    );
    must_name(
        rule_clipboard_owner(&host),
        HOST,
        "fn os_file_clipboard_write",
    );
    let found = offenders_of(
        "crates\\tako-control\\src\\dispatch.rs",
        "fn f() {\n    let _ = crate::platform::file_clipboard::write(&[], false);\n}\n",
        "file_clipboard::write(",
        &[APP],
    );
    assert_eq!(found.len(), 1, "dispatch が直に書いても拾わない: {found:?}");
    assert!(found[0].starts_with(&format!("{DISPATCH}:2 ")), "{found:?}");
    assert!(
        offenders_of(
            "crates\\tako-app\\src\\main.rs",
            "fn f() {\n    tako_control::platform::file_clipboard::write(&[], false);\n}\n",
            "file_clipboard::write(",
            &[APP],
        )
        .is_empty(),
        "Windows の区切りでも GUI のホストは許す"
    );
}

#[test]
fn 注入_ポーリングの世代を外すと名指す() {
    let filetree = injected(
        FILETREE,
        "            .filter(|(dir, _)| reads.get(dir).is_none_or(|read| *read <= since))",
        "            .filter(|_| true)",
    );
    must_name(
        rule_tree_refresh(&filetree, &source(APP)),
        FILETREE,
        "fn apply_refresh_since",
    );
}

/// 未使用の定数を作らない（`CLIP` / `PLATFORM_CLIP` は名前の一覧として置き、走査の対象に含める）
#[test]
fn 番犬が見るファイルが在る() {
    for rel in [
        COPY,
        CLIP,
        DISPATCH,
        HOST,
        PLATFORM_CLIP,
        MCP_REQUEST,
        MCP_CATALOG,
        CLI,
        APP,
        SIDEBAR,
        FILETREE,
    ] {
        assert!(
            repo_root().join(rel).is_file(),
            "{rel}:1 が無い（改名したなら番犬も直す）"
        );
    }
}

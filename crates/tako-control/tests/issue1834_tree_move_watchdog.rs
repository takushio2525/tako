//! **#1834 の番犬**: ファイルツリーの D&D による移動（FR-3.32）の要所が外れないようにする。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`同名は上書きせずに断る`] — `rename(2)` は既存のファイルや空のフォルダを**黙って
//!    置き換える**ので、段取りと実行の直前の 2 回、リンクを辿らずに見る
//! 2. [`自分自身と配下へは実体の形で断る`] — 字面で比べると `..`・シンボリックリンク・
//!    Windows の大文字小文字で素通りし、フォルダが自分の中へ消える
//! 3. [`付け替え先は移す前に決め後始末は1回`] — 移した後に照合すると実体の形が引けない
//!    （元の場所はもう無い）。後始末が 2 回・0 回になると付け替えが重なる / 漏れる
//! 4. [`開いているペインの付け替えが中身まで届く`] — パスだけ替えて編集バッファ・言語サーバ・
//!    監視を置き去りにすると、#1659 の「外で削除された」の帯が出る（**Issue の本体**）
//! 5. [`移動の口はdispatchの1つでcliとmcpとguiが1対1`] — 設計原則 5（UI だけの移動を作らない）
//! 6. [`ツリーの受け口はドラッグ中だけ付ける`] — gpui の `on_drag_move` は全リスナーへ届くので、
//!    常時付けると行数ぶんのリスナーがマウス移動のたびに走る
//!
//! 各規則は `Result` を返す検査関数で、本物のソースに当てる本体と、要所を消した写しに当てて
//! **file:line で名指すか**を確かめる注入（`注入_*`）を同じ関数で回す
//! （検出力の無い番犬は緑のまま腐る）。
//!
//! ## 相方
//!
//! 判定と実行の中身は `tako_core::file_move` の単体、dispatch の応答と付け替えは
//! `dispatch::tests::issue1834_*`、ツリーの追従は `tako-app` の `filetree::tests`、
//! 実マウスと実 CLI / MCP は visual-test `tree-move` + `scripts/test-tree-move-1834.sh`。
//! ここは**配線が外れていないこと**だけを見る

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const CORE: &str = "crates/tako-core/src/file_move.rs";
const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
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
            "{}:1 `fn {name}` が見つからない。\n改名したなら番犬の名前も直すこと（#1834 の配線が外れたまま緑になる）",
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

    fn count(&self, needle: &str) -> usize {
        self.text.matches(needle).count()
    }
}

// --- 規則（本物にも注入にも同じ関数を当てる） -------------------------------------

fn rule_name_taken(core: &Source) -> Check {
    let plan = core.body("plan")?;
    plan.must_contain(
        "if exists_no_follow(&to) {",
        "段取りが移動先の同名を見ていない。`rename(2)` は既存のファイル・空のフォルダを\n\
         黙って置き換えるので、同名があれば `NameTaken` で断ること（#1834）",
    )?;
    plan.must_contain("MoveRefusal::NameTaken", "同名を理由として返していない")?;
    let execute = core.body("execute")?;
    execute.must_order(
        "if exists_no_follow(&plan.to) {",
        "std::fs::rename(",
        "実行の直前に同名を見直していない（段取りから実行までに作られたものを上書きする）",
    )?;
    core.body("exists_no_follow")?.must_contain(
        "symlink_metadata(",
        "同名の検査がリンクを辿っている（壊れたリンクの上へ黙って上書きする）",
    )
}

fn rule_into_self(core: &Source) -> Check {
    let plan = core.body("plan")?;
    plan.must_contain(
        "lexical_verdict(&from_real, &dest_real)",
        "自分自身・配下の判定を実体の形で行っていない。字面で比べると `..`・リンク・\n\
         Windows の大文字小文字で素通りし、フォルダが自分の中へ消える（#1834）",
    )?;
    plan.must_contain(
        "canonicalize(",
        "実体の形を作っていない（`tako_core::platform::path::canonicalize` を通すこと）",
    )?;
    let lexical = core.body("lexical_verdict")?;
    lexical.must_contain(
        "dest_dir.starts_with(src)",
        "配下の判定が成分単位（`Path::starts_with`）になっていない",
    )?;
    lexical.must_contain("MoveRefusal::IntoDescendant", "配下への移動を断っていない")?;
    lexical.must_contain("MoveRefusal::IntoSelf", "自分自身への移動を断っていない")
}

fn rule_follow_order(dispatch: &Source) -> Check {
    let run = dispatch.body("run_file_move")?;
    let why =
        "付け替え先は**移す前に**決めること（実体の形で照合できるのは元の場所にあるうちだけ）";
    run.must_order(".open_file_paths()", "file_move::execute(", why)?;
    run.must_order("file_move::follows(", "file_move::execute(", why)?;
    run.must_order(
        "file_move::execute(",
        "host.file_moved(",
        "後始末（付け替え・ツリーの読み直し）を移した後に呼んでいない",
    )?;
    if run.count("host.file_moved(") != 1 {
        return Err(format!(
            "{}:{} `fn run_file_move` が後始末を {} 回呼んでいる（1 回だけ）",
            run.rel,
            run.line,
            run.count("host.file_moved(")
        ));
    }
    run.must_contain(
        "file_move::follow_legacy()",
        "A/B の入口（`TAKO_1834_LEGACY=1`）が付け替えだけを外していない",
    )
}

fn rule_follow_contents(app: &Source, sidebar: &Source, text_edit: &Source) -> Check {
    app.body("file_moved")?.must_contain(
        "self.follow_file_move(",
        "GUI の後始末が付け替えの 1 本（`follow_file_move`）を呼んでいない",
    )?;
    let paths = app.body("open_file_paths")?;
    // `self\n    .previews` とメソッドチェーンで改行を挟むので、`.previews` の字面で見る
    paths.must_contain(
        ".previews",
        "表示中のプレビューを付け替えの候補に入れていない",
    )?;
    paths.must_contain(
        "self.preview_edits",
        "編集バッファを付け替えの候補に入れていない",
    )?;
    let follow = sidebar.body("follow_file_move")?;
    for (needle, why) in [
        (
            "state.path = follow.to.clone()",
            "プレビューのパスを付け替えていない（監視が元の場所を見て削除扱いにする）",
        ),
        (
            "edit.buffer.retarget(",
            "編集バッファのパスを付け替えていない（保存が元の場所へ行き、#1659 の帯が出る）",
        ),
        (
            "DocLink::Unlinked",
            "言語サーバの文書を開き直していない（旧 URI のまま didChange を送り続ける）",
        ),
        ("self.sync_preview_lsp(", "新しい URI で didOpen していない"),
        (
            "self.filetree.note_moved(",
            "ツリーが移動に追従していない（2 秒ポーリングまで元の行が残る）",
        ),
        (
            "self.sync_preview_watches()",
            "ディスクの監視を新しい場所へ張り直していない",
        ),
        (
            "self.jump_history.retarget_path(",
            "ジャンプ履歴を付け替えていない（戻るで「消えていた」として捨てられる）",
        ),
    ] {
        follow.must_contain(needle, why)?;
    }
    text_edit.body("retarget")?.must_not_contain(
        "self.baseline",
        "付け替えが基準（開いた時点の中身）を触っている。中身は動いていないので、\n\
         基準を変えると未保存の変更や外部変更の判定が狂う",
    )
}

fn rule_one_to_one(
    dispatch: &Source,
    mcp: &Source,
    catalog: &Source,
    cli: &Source,
    sidebar: &Source,
    filetree: &Source,
) -> Check {
    let why = "移動が CLI / MCP / GUI の 1 つの口を通っていない（設計原則 5）";
    dispatch.arm_calls("FileOpKind::Move =>", "run_file_move(", 6, why)?;
    mcp.must_have(
        r#""move" => crate::protocol::FileOpKind::Move"#,
        "\"tako_file_op\"",
        "MCP `tako_file_op` の op=move が dispatch へ写っていない",
    )?;
    mcp.must_have(
        r#"dest: str_arg(args, "dest")?"#,
        "\"tako_file_op\"",
        "MCP `tako_file_op` の dest が dispatch へ渡っていない",
    )?;
    catalog.must_have(
        // #1860 で後ろに copy 等が続くようになったので閉じ括弧までは見ない
        r#""open_in_tako","move""#,
        "\"tako_file_op\"",
        "カタログの op の enum に move が無い（AI が選べない）",
    )?;
    catalog.must_have(
        r#""dest": {"#,
        "\"tako_file_op\"",
        "カタログに dest の引数が無い",
    )?;
    // #1867 で移す元が複数取れるようになった（1 件 = `FileOp` / 複数 = `FileOpMany` を
    // `file_op_for` が振り分ける）ので、腕の頭は `paths` を見る
    cli.arm_calls(
        "Command::File(FileCommand::Move { paths }) =>",
        "Some(resolve_cli_path(dest))",
        10,
        "CLI `tako file move` の移動先を CLI の cwd 基準で絶対化していない（GUI の cwd で読まれる）",
    )?;
    cli.arm_calls(
        "Command::File(FileCommand::Move { paths }) =>",
        "FileOpKind::Move",
        8,
        why,
    )?;
    cli.body("file_op_for")?.must_contain(
        "resolve_cli_path(",
        "CLI `tako file move` の移す元を CLI の cwd 基準で絶対化していない",
    )?;
    let drop = sidebar.body("drop_on_tree_row")?;
    drop.must_contain("FileOpKind::Move", why)?;
    drop.must_contain("tako_control::dispatch(", why)?;
    drop.must_not_contain(
        "file_move::execute(",
        "GUI が dispatch を通らずに移している（付け替え・応答の文面が CLI / MCP と割れる）",
    )?;
    for src in [sidebar, filetree] {
        src.must_not_have(
            "std::fs::rename(",
            "ツリーの UI が自分でファイルを動かしている。移動は dispatch `FileOpKind::Move` の 1 つ",
        )?;
    }
    Ok(())
}

fn rule_drag_live(sidebar: &Source) -> Check {
    sidebar.body("decorate_tree_row")?.must_order(
        "if !drag_live",
        "on_drag_move::<FileDrag>",
        "行の受け口がドラッグしていない間も付いている（マウス移動のたびに行数ぶん走る）",
    )
}

// --- 本体 ---------------------------------------------------------------------------

fn ok(check: Check) {
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[test]
fn 同名は上書きせずに断る() {
    ok(rule_name_taken(&source(CORE)));
}

#[test]
fn 自分自身と配下へは実体の形で断る() {
    ok(rule_into_self(&source(CORE)));
}

#[test]
fn 付け替え先は移す前に決め後始末は1回() {
    ok(rule_follow_order(&source(DISPATCH)));
}

#[test]
fn 開いているペインの付け替えが中身まで届く() {
    ok(rule_follow_contents(
        &source(APP),
        &source(SIDEBAR),
        &source(TEXT_EDIT),
    ));
}

#[test]
fn 移動の口はdispatchの1つでcliとmcpとguiが1対1() {
    ok(rule_one_to_one(
        &source(DISPATCH),
        &source(MCP_REQUEST),
        &source(MCP_CATALOG),
        &source(CLI),
        &source(SIDEBAR),
        &source(FILETREE),
    ));
}

#[test]
fn ツリーの受け口はドラッグ中だけ付ける() {
    ok(rule_drag_live(&source(SIDEBAR)));
}

/// 走査で得たリポジトリ相対パスを、定数と同じ `/` 区切りへ揃える。
///
/// `display()` の字面のまま比べると Windows では `crates\tako-control\src\dispatch.rs` になり、
/// 許した dispatch 自身まで違反に数える（#1834 の Windows CI で実際に落ちた）。
/// 書き方は既存の番犬（#1653 / #1811）と同じ
fn slash_rel(rel: &str) -> String {
    rel.replace('\\', "/")
}

/// 規則: 移動の実行（`file_move::execute(`）を呼んでよいのは dispatch だけ。
///
/// `rel` はリポジトリ相対パス（区切りはどちらでもよい）。違反は `/` 区切りの
/// `file:line 行の中身` で返す（走査の本体と注入が同じ関数を通る）
fn execute_offenders(rel: &str, src: &str) -> Vec<String> {
    let rel = slash_rel(rel);
    // 字面すら無いファイルは読まない（丸ごとテスト・コメントだけのファイルは
    // 本番の範囲が空で、共通部品が「空」として止める = 見るものが無い）
    if rel == CORE || !src.contains("file_move::execute(") {
        return Vec::new();
    }
    let prod = production_range::scan(src).text;
    let view = without_comments_checked(&prod, &rel);
    view.lines()
        .enumerate()
        .filter(|(_, line)| line.contains("file_move::execute(") && rel != DISPATCH)
        .map(|(i, line)| format!("{rel}:{} {}", i + 1, line.trim()))
        .collect()
}

/// 移動の実行（`file_move::execute`）を呼んでよいのは dispatch の `run_file_move` だけ
#[test]
fn 移動の実行はdispatchの1か所だけ() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for krate in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let dir = root.join("crates").join(krate).join("src");
        for path in walk_rs(&dir) {
            let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy();
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            offenders.extend(execute_offenders(&rel, &src));
        }
    }
    assert!(
        offenders.is_empty(),
        "dispatch の `run_file_move` 以外が移動を実行している:\n{}\n\n\
         移動は dispatch `FileOpKind::Move` の 1 つ（付け替え・後始末・応答の文面がそこで揃う。#1834）",
        offenders.join("\n")
    );
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
fn 注入_同名の検査を外すと名指す() {
    let core = injected(
        CORE,
        "    if exists_no_follow(&to) {",
        "    if false && exists_no_follow_x(&to) {",
    );
    must_name(rule_name_taken(&core), CORE, "fn plan");
    let core = injected(
        CORE,
        "    if exists_no_follow(&plan.to) {",
        "    if false {",
    );
    must_name(rule_name_taken(&core), CORE, "fn execute");
}

#[test]
fn 注入_配下の検査を字面へ戻すと名指す() {
    let core = injected(
        CORE,
        "lexical_verdict(&from_real, &dest_real)",
        "lexical_verdict(src, dest_dir)",
    );
    must_name(rule_into_self(&core), CORE, "fn plan");
    let core = injected(
        CORE,
        "if dest_dir.starts_with(src) {",
        "if dest_dir.to_string_lossy().starts_with(&*src.to_string_lossy()) {",
    );
    must_name(rule_into_self(&core), CORE, "fn lexical_verdict");
}

#[test]
fn 注入_付け替え先を移した後に決めると名指す() {
    let src = std::fs::read_to_string(repo_root().join(DISPATCH)).expect("読める");
    // 実行を先頭へ持ち上げた写し（付け替え先が移した後に決まる）
    let moved = src.replacen(
        "    let open: Vec<(u64, std::path::PathBuf)> = host",
        "    file_move::execute(&plan).map_err(refused)?;\n    let open: Vec<(u64, std::path::PathBuf)> = host",
        1,
    );
    assert_ne!(moved, src, "{DISPATCH}:1 注入の目印が無い");
    let dispatch = Source::from_text(DISPATCH, &moved);
    must_name(rule_follow_order(&dispatch), DISPATCH, "fn run_file_move");
    let dispatch = injected(
        DISPATCH,
        "    host.file_moved(&plan.from, &plan.to, &follows);",
        "",
    );
    must_name(rule_follow_order(&dispatch), DISPATCH, "fn run_file_move");
}

#[test]
fn 注入_編集バッファを付け替えないと名指す() {
    let sidebar = injected(
        SIDEBAR,
        "edit.buffer.retarget(follow.to.clone());",
        "let _ = &follow.to;",
    );
    must_name(
        rule_follow_contents(&source(APP), &sidebar, &source(TEXT_EDIT)),
        SIDEBAR,
        "fn follow_file_move",
    );
    let sidebar = injected(SIDEBAR, "self.filetree.note_moved(from, to);", "");
    must_name(
        rule_follow_contents(&source(APP), &sidebar, &source(TEXT_EDIT)),
        SIDEBAR,
        "fn follow_file_move",
    );
}

#[test]
fn 注入_uiが自分で移すと名指す() {
    let sidebar = injected(
        SIDEBAR,
        "                let result = tako_control::dispatch(\n                    self,\n                    Request::FileOp {\n                        op: FileOpKind::Move,",
        "                let _ = std::fs::rename(&drag.path, &hover.dest);\n                let result = tako_control::dispatch(\n                    self,\n                    Request::FileOp {\n                        op: FileOpKind::Move,",
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &source(MCP_REQUEST),
        &source(MCP_CATALOG),
        &source(CLI),
        &sidebar,
        &source(FILETREE),
    )
    .expect_err("UI の直接 rename を見逃した");
    assert!(
        err.starts_with(&format!("{SIDEBAR}:")) && err.contains("std::fs::rename("),
        "{err}"
    );
    let mcp = injected(
        MCP_REQUEST,
        r#""move" => crate::protocol::FileOpKind::Move,"#,
        "",
    );
    let err = rule_one_to_one(
        &source(DISPATCH),
        &mcp,
        &source(MCP_CATALOG),
        &source(CLI),
        &source(SIDEBAR),
        &source(FILETREE),
    )
    .expect_err("MCP の op=move の欠落を見逃した");
    assert!(err.starts_with(&format!("{MCP_REQUEST}:")), "{err}");
}

/// Windows の区切り（`\`）で走査結果が来ても、許した dispatch 自身は数えず、
/// 他のファイルへ足した移動の実行は `/` 区切りの file:line で名指す
/// （#1834 の Windows CI: `display()` の字面を `/` 区切りの定数と比べ、dispatch 自身を違反に数えた）
#[test]
fn 注入_windowsの区切りでもdispatch以外の実行だけを名指す() {
    let windows = |rel: &str| rel.replace('/', "\\");
    let dispatch = std::fs::read_to_string(repo_root().join(DISPATCH)).expect("読める");
    assert!(
        dispatch.contains("file_move::execute("),
        "{DISPATCH}:1 前提: dispatch が移動を実行している"
    );
    for rel in [DISPATCH.to_string(), windows(DISPATCH)] {
        assert_eq!(
            execute_offenders(&rel, &dispatch),
            Vec::<String>::new(),
            "許した dispatch 自身を違反に数えた（{rel}）"
        );
    }
    // UI が自分で実行する写し（ドロップの中で dispatch を通らずに移す）
    let marker = "        use tako_core::file_move::{DropVerdict, MoveRefusal};\n";
    let sidebar = std::fs::read_to_string(repo_root().join(SIDEBAR)).expect("読める");
    assert!(sidebar.contains(marker), "{SIDEBAR}:1 注入の目印が無い");
    let bad = sidebar.replacen(
        marker,
        &format!("{marker}        let _ = tako_core::file_move::execute(&plan);\n"),
        1,
    );
    let line = bad
        .lines()
        .position(|l| l.trim() == "let _ = tako_core::file_move::execute(&plan);")
        .expect("注入した行")
        + 1;
    for rel in [SIDEBAR.to_string(), windows(SIDEBAR)] {
        let offenders = execute_offenders(&rel, &bad);
        assert!(
            offenders.len() == 1 && offenders[0].starts_with(&format!("{SIDEBAR}:{line} ")),
            "UI の直接実行を file:line で名指していない（{rel}）: {offenders:?}"
        );
    }
}

#[test]
fn 注入_受け口を常時付けると名指す() {
    let sidebar = injected(
        SIDEBAR,
        "        if !drag_live {\n            return el;\n        }\n",
        "",
    );
    must_name(rule_drag_live(&sidebar), SIDEBAR, "fn decorate_tree_row");
}

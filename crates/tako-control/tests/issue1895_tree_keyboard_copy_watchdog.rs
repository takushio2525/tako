//! **#1895 の番犬**: ファイルツリーのキーでの範囲選択・ごみ箱、1 つのファイルの途中での取り消し、
//! 複数のコピーを 1 つのジョブへ、帯の残り時間（FR-3.39）の要所が外れないようにする。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`キーの範囲はcoreの1実装`] — ⇧↑ / ⇧↓ の範囲を UI が自前で決めると ⇧クリックと規則が
//!    割れる。⇧ だけの矢印・macOS の ⌘⌫・Windows の修飾無しの Delete だけを奪う（Shift+Delete =
//!    エクスプローラーの「完全に削除」・⌫ 単独は端末へ流す）
//! 2. [`ごみ箱は単数と同じ口で見出しとリモートを断る`] — ⌘⌫ が自前で消すと #1399 の通知・CLI の
//!    `tako file trash` と割れる。見出しを断らないと打ち間違いでワークスペースごとごみ箱へ入る
//! 3. [`一つのファイルの途中でも取り消しを見て作りかけを消す`] — 写す単位ごとに取り消しを見ないと
//!    巨大な 1 ファイルの途中で止まらない。止めた作りかけを消さないと半端なファイルが残る
//! 4. [`バイトは単調に増える`] — OS の累計が戻っても帯・`copy_progress` のバイトを減らさない
//! 5. [`まとめたコピーは1つのジョブ`] — CLI `tako file copy a b dst` と MCP `paths` が 1 要求・
//!    札 1 枚にならないと、進み具合も取り消しも 1 件ずつに割れる
//! 6. [`残り時間はcoreの1実装`] — 帯と `copy_progress` の `eta_secs` が別々に見積もると食い違う。
//!    始めの数秒・少ししか写していないうちは出さない（ぶれを見せない）
//!
//! 各規則は `Result` を返す検査関数で、本物のソースに当てる本体と、要所を消した写しに当てて
//! **file:line で名指すか**を確かめる注入（`注入_*`）を同じ関数で回す
//! （検出力の無い番犬は緑のまま腐る）。
//!
//! ## 相方
//!
//! 中身は `tako_core::tree_select::extend` / `platform::keys::tree_select_key` /
//! `file_copy`（途中の取り消し・バイト・`eta`）/ `platform::fs_copy` の単体、dispatch の応答は
//! `dispatch::tests::issue1895_*`、MCP の振り分けは `mcp::tests`、実マウス・実キーと CLI / MCP の
//! 字面一致は visual-test `tree-keyboard-copy` + `scripts/test-tree-keyboard-copy-1895.sh`。
//! ここは**配線が外れていないこと**だけを見る

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const SELECT: &str = "crates/tako-core/src/tree_select.rs";
const COPY: &str = "crates/tako-core/src/file_copy.rs";
const FS_COPY: &str = "crates/tako-core/src/platform/fs_copy.rs";
const KEYS: &str = "crates/tako-core/src/platform/keys.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
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

    /// `name` の関数のうち `nth` 番目（0 始まり。cfg で OS ごとに同じ名前が並ぶとき）
    fn body_nth(&self, name: &str, nth: usize) -> Result<Body, String> {
        let mut offset = 0;
        let mut seen = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
            if seen < nth {
                seen += 1;
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
            "{}:1 `fn {name}`（{nth} 番目）が見つからない。\n改名したなら番犬の名前も直すこと（#1895 の配線が外れたまま緑になる）",
            self.rel
        ))
    }

    fn body(&self, name: &str) -> Result<Body, String> {
        self.body_nth(name, 0)
    }

    fn line_with(&self, needle: &str) -> Option<usize> {
        self.view
            .lines()
            .position(|line| line.contains(needle))
            .map(|i| i + 1)
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

fn rule_keys(select: &Source, keys: &Source, sidebar: &Source) -> Check {
    // #1908 から画面のキーは dispatch の `TreeSelection`（CLI / MCP と同じ口）→ core の
    // `tree_select::on_key` → `extend` を通る。見るのは同じ意図（UI が範囲を自前で決めない）
    let action = sidebar.body("tree_select_key_action")?;
    action.must_contain(
        "tako_control::protocol::Request::TreeSelection {",
        "⇧↑ / ⇧↓ の範囲を UI が自前で決めている（正本は `tree_select::extend` = ⇧クリックと同じ規則。#1908 から dispatch の `TreeSelection` を通る）",
    )?;
    action.must_not_contain(
        "tree_select::extend(",
        "⇧↑ / ⇧↓ の範囲を UI が自前で決めている（dispatch を通らない = CLI / MCP と割れる）",
    )?;
    select.body("on_key")?.must_contain(
        "Key::ExtendUp => return KeyOutcome::Select(extend(prev, Step::Up, &order)),",
        "⇧↑ が core の `extend`（⇧クリックと同じ規則）を通っていない",
    )?;
    select.body("extend")?.must_contain(
        "apply(Some(prev), &order[next], ClickKind::Range, order)",
        "⇧↑ / ⇧↓ が ⇧クリック（起点からの範囲）と同じ `apply` を通っていない（伸ばして逆へ打っても縮まない）",
    )?;
    let key = keys.body("tree_select_key")?;
    key.must_contain(
        "let none_but_shift = shift && !platform_key && !control && !alt;",
        "⇧↑ / ⇧↓ の判定が ⇧ 以外の修飾を見ていない（⌘⇧↑ 等の別の割り当てを奪う）",
    )?;
    key.must_contain(
        "platform == Platform::Windows && !platform_key && !control && !alt && !shift",
        "Windows の Delete の判定が修飾を見ていない（Shift+Delete = 完全に削除・Ctrl+Delete を奪う）",
    )?;
    key.must_contain(
        "platform == Platform::MacOs && platform_key && !control && !alt && !shift",
        "macOS のごみ箱が ⌘⌫ だけになっていない（⌫ 単独で選んだものを消す）",
    )?;
    let handle = sidebar.body("handle_tree_clip_keystroke")?;
    handle.must_order(
        "tako_core::platform::keys::tree_select_key(",
        "self.tree_select_key_action(key, &sel, cx)",
        "選んでいる間の打鍵が範囲選択・ごみ箱の判定を通っていない",
    )?;
    handle.must_contain(
        "tako_core::file_copy::legacy_1895()",
        "A/B（`TAKO_1895_LEGACY=1`）がキーに効いていない（同一バイナリで旧挙動へ戻せない）",
    )
}

fn rule_trash(sidebar: &Source) -> Check {
    let guard = sidebar.body("trash_tree_selection")?;
    for (needle, why) in [
        (
            "trash_remote_refused()",
            "リモート（SSH）の行を ⌘⌫ でローカルのごみ箱の口へ渡している（#919）",
        ),
        (
            "trash_root_refused()",
            "見出しのフォルダを ⌘⌫ で断っていない（打ち間違いでワークスペースごとごみ箱へ入る）",
        ),
    ] {
        guard.must_contain(needle, why)?;
    }
    guard.must_order(
        "trash_root_refused()",
        "self.trash_tree_paths(",
        "見出しを断る前にごみ箱へ入れている",
    )?;
    let paths = sidebar.body("trash_tree_paths")?;
    let why = "⌘⌫ / 右クリックの「削除」が dispatch の `trash` の口を通っていない（CLI `tako file trash` と通知・応答が割れる）";
    paths.must_contain("FileOpKind::Trash", why)?;
    paths.must_contain("tako_control::dispatch(", why)?;
    paths.must_contain("self.dispatch_tree_many(FileOpKind::Trash", why)?;
    paths.must_not_contain(
        "move_to_trash(",
        "GUI が dispatch を通らずに直にごみ箱へ入れている",
    )?;
    sidebar.arm_calls(
        "\"trash\" => {",
        "self.trash_tree_paths(",
        12,
        "右クリックの「削除」と ⌘⌫ が同じ口を通っていない",
    )
}

fn rule_midfile_cancel(copy: &Source, fs: &Source) -> Check {
    let file = copy.body("copy_file")?;
    file.must_contain(
        "fs_copy::copy_file_exclusive(",
        "1 つのファイルを写す単位ごとに進み具合を受け取る口を通っていない（途中で止まらない）",
    )?;
    file.must_contain(
        "!progress.is_cancelled()",
        "写す単位ごとに取り消しを見ていない（巨大な 1 ファイルの途中で止まらない）",
    )?;
    file.must_contain(
        "Err(fs_copy::FileCopyError::Cancelled) => return Err(CopyRefusal::Cancelled)",
        "途中で止めたことを取り消しとして返していない（失敗の通知になる）",
    )?;
    // 最初の `fn copy` = macOS（clone → 中身）/ 2 つ目 = Windows（CopyFileExW）
    let mac = fs.body_nth("copy", 0)?;
    mac.must_order(
        "fcopyfile(&reader, &writer, &mut ctx)",
        "let _ = std::fs::remove_file(dst);",
        "中身を写す途中で止めた・失敗した作りかけを消していない（macOS）",
    )?;
    fs.body("status")?.must_contain(
        "libc::COPYFILE_QUIT",
        "`fcopyfile` の状態コールバックが止める値を返していない（取り消しても最後まで写す）",
    )?;
    fs.body("routine")?.must_contain(
        "PROGRESS_CANCEL",
        "`CopyFileExW` の進み具合ルーチンが止める値を返していない（Windows で取り消しても最後まで写す）",
    )?;
    fs.body_nth("copy", 1)?.must_contain(
        "let _ = std::fs::remove_file(dst);",
        "Windows の途中の失敗の作りかけを消していない",
    )
}

fn rule_monotonic(copy: &Source, fs: &Source) -> Check {
    fs.body("report")?.must_contain(
        "self.copied = self.copied.max(copied);",
        "OS が返した累計が戻ったときにバイトを戻している（進み具合が減る）",
    )?;
    copy.body("copy_file")?.must_order(
        "if copied > reported {",
        "progress.add_done(0, copied - reported);",
        "届いた累計の差分だけを足していない（同じ値で 2 回足す・減る）",
    )?;
    copy.must_not_have(
        "fetch_sub(",
        "進み具合を減らす道がある（帯・`copy_progress` のバイトは単調に増える）",
    )
}

fn rule_one_job(dispatch: &Source, cli: &Source, catalog: &Source) -> Check {
    let why = "まとめたコピーが 1 つのジョブ（札 1 枚）になっていない（進み具合も取り消しも 1 件ずつに割れる）";
    dispatch
        .body("prepare_offload")?
        .must_contain("FileCopyJob::many(paths, dest.as_deref())", why)?;
    let many = dispatch.body("many")?;
    many.must_contain(
        "distinct_roots(",
        "まとめたコピーが配下を外していない（2 回写す）",
    )?;
    many.must_contain("Self::new(items, CopyReply::Many", why)?;
    dispatch
        .body("file_op_many")?
        .must_contain("FileCopyJob::many(paths, dest)?", why)?;
    let requests = cli.body("file_copy_requests")?;
    requests.must_contain(
        "if !tako_core::file_copy::legacy_1895() {",
        "CLI のまとめたコピーが A/B の分かれ目を通っていない",
    )?;
    requests.must_order(
        "if !tako_core::file_copy::legacy_1895() {",
        "file_op_for(",
        "CLI `tako file copy a b dst` がまとめた 1 要求（`file_op_for` = MCP の `paths` と同じ）になっていない",
    )?;
    if catalog
        .line_with("trash・move・copy をまとめて行う")
        .is_none()
    {
        return Err(format!(
            "{}:{} カタログの `paths` の説明に copy が無い（AI がまとめたコピーを選べない）",
            catalog.rel,
            catalog.line_with("\"tako_file_op\"").unwrap_or(1)
        ));
    }
    Ok(())
}

fn rule_eta(copy: &Source, dispatch: &Source, sidebar: &Source) -> Check {
    let eta = copy.body("eta")?;
    for (needle, why) in [
        (
            "snap.copying_for < ETA_MIN_ELAPSED",
            "写し始めの数秒も残り時間を出している（速さがぶれる）",
        ),
        (
            "u128::from(ETA_MIN_PERMILLE)",
            "少ししか写していないうちも残り時間を出している",
        ),
        (
            "snap.counting",
            "数えている間（母数が決まっていない）も出している",
        ),
    ] {
        eta.must_contain(needle, why)?;
    }
    let why =
        "帯と `copy_progress` の残り時間が `file_copy::eta` の 1 実装を通っていない（食い違う）";
    dispatch
        .body("copy_progress_json")?
        .must_contain("tako_core::file_copy::eta(&s)", why)?;
    let band = sidebar.body("render_copy_progress")?;
    band.must_contain("tako_core::file_copy::eta(&snap)", why)?;
    band.must_contain("tako_core::file_copy::eta_label(", why)
}

// --- 本体 ---------------------------------------------------------------------------

fn ok(check: Check) {
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[test]
fn キーの範囲はcoreの1実装() {
    ok(rule_keys(&source(SELECT), &source(KEYS), &source(SIDEBAR)));
}

#[test]
fn ごみ箱は単数と同じ口で見出しとリモートを断る() {
    ok(rule_trash(&source(SIDEBAR)));
}

#[test]
fn 一つのファイルの途中でも取り消しを見て作りかけを消す() {
    ok(rule_midfile_cancel(&source(COPY), &source(FS_COPY)));
}

#[test]
fn バイトは単調に増える() {
    ok(rule_monotonic(&source(COPY), &source(FS_COPY)));
}

#[test]
fn まとめたコピーは1つのジョブ() {
    ok(rule_one_job(
        &source(DISPATCH),
        &source(CLI),
        &source(MCP_CATALOG),
    ));
}

#[test]
fn 残り時間はcoreの1実装() {
    ok(rule_eta(&source(COPY), &source(DISPATCH), &source(SIDEBAR)));
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
fn 注入_uiが範囲を自前で決めると名指す() {
    // dispatch を通らず（#1908 の前の形 = UI が core を直に呼んで自前で当てる）
    let sidebar = injected(
        SIDEBAR,
        "tako_control::protocol::Request::TreeSelection {",
        "tako_control::protocol::Request::List {",
    );
    must_name(
        rule_keys(&source(SELECT), &source(KEYS), &sidebar),
        SIDEBAR,
        "fn tree_select_key_action",
    );
    let sidebar = injected(
        SIDEBAR,
        "        self.context_menu = None;\n        let Some(select) = key.select_key() else {",
        "        self.context_menu = None;\n        let _ = tako_core::tree_select::extend(&sel.as_core(), step, &order);\n        let Some(select) = key.select_key() else {",
    );
    must_name(
        rule_keys(&source(SELECT), &source(KEYS), &sidebar),
        SIDEBAR,
        "fn tree_select_key_action",
    );
    let select = injected(
        SELECT,
        "Key::ExtendUp => return KeyOutcome::Select(extend(prev, Step::Up, &order)),",
        "Key::ExtendUp => return KeyOutcome::Select(prev.clone()),",
    );
    must_name(
        rule_keys(&select, &source(KEYS), &source(SIDEBAR)),
        SELECT,
        "fn on_key",
    );
    let keys = injected(
        KEYS,
        "platform == Platform::Windows && !platform_key && !control && !alt && !shift",
        "platform == Platform::Windows",
    );
    must_name(
        rule_keys(&source(SELECT), &keys, &source(SIDEBAR)),
        KEYS,
        "fn tree_select_key",
    );
}

#[test]
fn 注入_見出しを断らずにごみ箱へ入れると名指す() {
    let sidebar = injected(
        SIDEBAR,
        "                crate::ui_text::sidebar::trash_root_refused(),",
        "                \"\",",
    );
    must_name(rule_trash(&sidebar), SIDEBAR, "fn trash_tree_selection");
    let sidebar = injected(
        SIDEBAR,
        "self.dispatch_tree_many(FileOpKind::Trash, paths, None, op, cx);",
        "let _ = (paths, op);",
    );
    must_name(rule_trash(&sidebar), SIDEBAR, "fn trash_tree_paths");
}

#[test]
fn 注入_途中で取り消しを見ないと名指す() {
    let copy = injected(
        COPY,
        "            !progress.is_cancelled()\n",
        "            true\n",
    );
    must_name(
        rule_midfile_cancel(&copy, &source(FS_COPY)),
        COPY,
        "fn copy_file",
    );
    let fs = injected(
        FS_COPY,
        "                // ここまで来たら `dst` はこの呼び出しが `create_new` で作ったもの\n                let _ = std::fs::remove_file(dst);",
        "",
    );
    must_name(rule_midfile_cancel(&source(COPY), &fs), FS_COPY, "fn copy");
    let fs = injected(
        FS_COPY,
        "            libc::COPYFILE_QUIT\n",
        "            libc::COPYFILE_CONTINUE\n",
    );
    must_name(
        rule_midfile_cancel(&source(COPY), &fs),
        FS_COPY,
        "fn status",
    );
}

#[test]
fn 注入_バイトを戻すと名指す() {
    let fs = injected(
        FS_COPY,
        "self.copied = self.copied.max(copied);",
        "self.copied = copied;",
    );
    must_name(rule_monotonic(&source(COPY), &fs), FS_COPY, "fn report");
}

#[test]
fn 注入_まとめたコピーを1件ずつに割ると名指す() {
    let cli = injected(
        CLI,
        "    if !tako_core::file_copy::legacy_1895() {",
        "    if false {",
    );
    must_name(
        rule_one_job(&source(DISPATCH), &cli, &source(MCP_CATALOG)),
        CLI,
        "fn file_copy_requests",
    );
    let dispatch = injected(
        DISPATCH,
        "FileCopyJob::many(paths, dest.as_deref())\n",
        "FileCopyJob::new(Vec::new(), CopyReply::One)\n",
    );
    must_name(
        rule_one_job(&dispatch, &source(CLI), &source(MCP_CATALOG)),
        DISPATCH,
        "fn prepare_offload",
    );
}

#[test]
fn 注入_残り時間のぶれを見せると名指す() {
    let copy = injected(COPY, "        || snap.copying_for < ETA_MIN_ELAPSED\n", "");
    must_name(
        rule_eta(&copy, &source(DISPATCH), &source(SIDEBAR)),
        COPY,
        "fn eta",
    );
}

#[test]
fn 番犬が見るファイルが在る() {
    for rel in [
        SELECT,
        COPY,
        FS_COPY,
        KEYS,
        DISPATCH,
        MCP_CATALOG,
        CLI,
        SIDEBAR,
    ] {
        assert!(
            repo_root().join(rel).is_file(),
            "{rel}:1 が無い（改名したなら番犬も直す）"
        );
    }
}

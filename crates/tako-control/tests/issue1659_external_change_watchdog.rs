//! **#1659 の番犬**: 外部変更を検知した後の逃げ道が塞がらないようにする。
//!
//! ## 何が起きていたのか（修正前）
//!
//! 編集中のファイルがディスク側で変わると `TextBuffer::save` は永久に `ExternalChanged` を
//! 返し、`tako edit save` に `--force` は無く、読み直しも差分も無かった。未保存だと
//! `set_preview` が別のファイルを開くのも断るので、**ペインを閉じる以外に抜けられなかった**。
//! しかも自動保存（既定 ON）が 500ms ごとに同じ競合を出し続けた。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`競合中は自動保存を止める`] — 500ms ごとの競合へ戻る（**Issue の本体の 1 つ**）
//! 2. [`競合は新しく分かったときだけ知らせる`] — 同じ競合を何度も知らせ直す
//! 3. [`競合の記録は1か所`] — 文面の部分一致（`msg.contains("外部")`）や `SaveStatus::Conflict` の
//!    直書きが散らばり、回数・停止の判定を素通りする経路ができる
//! 4. [`監視は基準と突き合わせて未編集なら追従する`] — 自分の保存や touch を競合と取り違える
//! 5. [`上書きと読み直しはcoreの口を通る`] — 逃げ道が GUI だけ / CLI だけになる
//! 6. [`dispatchとCLIとMCPが1対1で配線されている`] — 設計原則 5（AI フルコントロール）
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 往復の中身（上書き・読み直し・undo で戻る・消されたファイル・CRLF・履歴の大きさ）は
//! `tako_core::text_edit` の単体、dispatch の配線と応答の形は
//! `dispatch::tests::issue1659_*`、自動保存の停止と通知の回数は `tako-app` の
//! `preview::tests`、実経路（隔離 GUI + CLI + MCP）は `scripts/test-external-change-1659.sh`。
//! ここは**配線が外れていないこと**だけを見る。肯定の存在確認はコメントを落とした眺めで見る（#1609）

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const CORE: &str = "crates/tako-core/src/text_edit.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象 1 ファイルの 2 つの眺め（**同じバイト長**なので範囲を互いに使い回せる）。
///
/// - `code`: コメントも文字列も潰した眺め。波括弧の対応を数えるのに使う
/// - `view`: コメントだけ潰した眺め。**識別子とリテラルの中身**を見るのに使う（#1609）
struct Source {
    rel: &'static str,
    code: String,
    view: String,
}

fn source(rel: &'static str) -> Source {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    let prod = production_range::production(&src, rel);
    let source = Source {
        rel,
        code: code_view(&prod),
        view: without_comments_checked(&prod, rel),
    };
    assert_eq!(
        source.code.len(),
        source.view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    source
}

/// 関数 1 本（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: &'static str,
    name: String,
    line: usize,
    /// コメントだけ潰した眺めでの本体
    text: String,
}

impl Source {
    /// 関数の本体を名前で引く。修飾子（`pub(crate) fn` / `async fn`）に依らない（#1496）
    fn body(&self, name: &str) -> Option<Body> {
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
            let open = self.code[start..].find('{')? + start;
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
            return Some(Body {
                rel: self.rel,
                name: name.to_string(),
                line: self.code[..start].lines().count() + 1,
                text: self.view[open..end].to_string(),
            });
        }
        None
    }

    /// 本体を引く（無ければ改名として落とす）
    fn require(&self, name: &str) -> Body {
        self.body(name).unwrap_or_else(|| {
            panic!(
                "{}:1 `fn {name}` が見つからない。\n\
                 改名したなら番犬の名前も直すこと（#1659 の配線が外れたまま緑になる）",
                self.rel
            )
        })
    }

    /// `needle` を含む行（1 起点）。無ければ `None`
    fn line_with(&self, needle: &str) -> Option<usize> {
        self.view
            .lines()
            .position(|line| line.contains(needle))
            .map(|i| i + 1)
    }

    /// `needle` が在ること（無ければ最寄りの手掛かりの行で名指しする）
    fn must_have(&self, needle: &str, near: &str, why: &str) {
        assert!(
            self.line_with(needle).is_some(),
            "{}:{} `{needle}` が無い。\n{why}",
            self.rel,
            self.line_with(near).unwrap_or(1)
        );
    }

    /// `arm` の行から `window` 行以内に `needle` が在ること（match の腕の配線を見る）
    fn arm_calls(&self, arm: &str, needle: &str, window: usize, why: &str) {
        let line = self
            .line_with(arm)
            .unwrap_or_else(|| panic!("{}:1 `{arm}` の腕が無い。\n{why}", self.rel));
        let found = self
            .view
            .lines()
            .skip(line - 1)
            .take(window)
            .any(|l| l.contains(needle));
        assert!(
            found,
            "{}:{line} `{arm}` の腕が `{needle}` を呼んでいない。\n{why}",
            self.rel
        );
    }
}

impl Body {
    fn must_contain(&self, needle: &str, why: &str) {
        assert!(
            self.text.contains(needle),
            "{}:{} `fn {}` が `{needle}` を通っていない。\n{why}",
            self.rel,
            self.line,
            self.name
        );
    }

    fn must_not_contain(&self, needle: &str, why: &str) {
        assert!(
            !self.text.contains(needle),
            "{}:{} `fn {}` に `{needle}` が在る。\n{why}",
            self.rel,
            self.line,
            self.name
        );
    }
}

/// 1: 競合のあいだは自動保存を回さない（保存しても断られるだけで、同じ競合を知らせ直す）
#[test]
fn 競合中は自動保存を止める() {
    let preview = source(PREVIEW);
    preview.require("autosave_due").must_contain(
        "self.conflict.is_none()",
        "自動保存の保留へ入れる判定が競合を見ていない。\n\
         競合中も 500ms ごとに保存を試みて、同じ競合を出し続ける（#1659 の症状）",
    );
    let app = source(APP);
    app.require("run_autosave").must_contain(
        "edit.conflict.is_some()",
        "500ms 待った後の保存が競合を見ていない。\n\
         保留へ入れた後に競合が分かった回（監視が先に知らせた等）で保存を試みてしまう",
    );
}

/// 2: 同じ競合を何度検知しても、知らせるのは 1 回だけ
#[test]
fn 競合は新しく分かったときだけ知らせる() {
    let preview = source(PREVIEW);
    let note = preview.require("note_conflict");
    note.must_contain(
        "c.state == state",
        "既に同じ状態の競合を記録しているかを見ていない = 検知のたびに知らせ直す",
    );
    note.must_contain(
        "return false",
        "同じ競合のときに何もせず抜けていない = 回数（notices）が増え続ける",
    );
    note.must_contain(
        "notices",
        "知らせた回数を数えていない（応答の `conflict.notices` で「1 回だけ」を確かめられない）",
    );
}

/// 3: 競合の記録は `EditState::note_conflict` の 1 か所（判定を素通りする経路を作らない）
#[test]
fn 競合の記録は1か所() {
    let mut offenders = Vec::new();
    for rel in [
        PREVIEW,
        APP,
        SIDEBAR,
        "crates/tako-app/src/preview_render.rs",
    ] {
        let src = source(rel);
        for (i, line) in src.view.lines().enumerate() {
            let direct = line.contains("save_status = Some(preview::SaveStatus::Conflict)")
                || line.contains("save_status = Some(SaveStatus::Conflict)");
            if direct {
                offenders.push((rel, i + 1, line.trim().to_string()));
            }
        }
    }
    // 直書きしてよいのは note_conflict の本体の中だけ
    let preview = source(PREVIEW);
    let note = preview.require("note_conflict");
    let inside = note.line..=note.line + note.text.lines().count();
    let outside: Vec<String> = offenders
        .iter()
        .filter(|(rel, line, _)| !(*rel == PREVIEW && inside.contains(line)))
        .map(|(rel, line, text)| format!("{rel}:{line} {text}"))
        .collect();
    assert!(
        outside.is_empty(),
        "外部変更の競合を `note_conflict` を通さずに記録している:\n{}\n\n\
         `EditState::note_conflict` を呼ぶこと（知らせる回数と自動保存の停止がそこで決まる。#1659）",
        outside.join("\n")
    );
    // 保存の失敗を文面の部分一致で競合と読まない（型で読む = `conflict_state_of`）
    let app = source(APP);
    app.require("run_autosave").must_not_contain(
        "msg.contains(",
        "保存の失敗を文面で競合と判定している。\n\
         文面が変わると黙って外れる。`EditState::conflict_state_of` で型から読むこと（#1659）",
    );
    app.require("save_preview_local")
        .must_contain("conflict_state_of(", "保存の失敗を型で競合と読んでいない");
}

/// 4: ファイル監視は「開いた / 保存した / 読み直した時点の中身」と突き合わせ、
///    未編集ならディスクへ追従する
#[test]
fn 監視は基準と突き合わせて未編集なら追従する() {
    let sidebar = source(SIDEBAR);
    let reload = sidebar.require("apply_preview_reload");
    reload.must_contain(
        "observe_disk(",
        "監視が基準（`TextBuffer::observe_disk`）と突き合わせていない。\n\
         本文とだけ比べると、自分の保存・touch・外で元に戻された回を競合と取り違える",
    );
    reload.must_contain(
        "reload_from(",
        "未編集の編集セッションがディスクへ追従していない（不要な競合の帯が出る）",
    );
    reload.must_contain(
        "note_conflict(",
        "監視が見つけた競合を `note_conflict` へ渡していない",
    );
    reload.must_contain(
        "clear_conflict()",
        "外で元に戻された・同じ中身に書かれたときに競合を解いていない",
    );
}

/// 5: 上書き・読み直し・差分は tako-core の口を通る（GUI・CLI・MCP が同じ保存の意味を持つ）
#[test]
fn 上書きと読み直しはcoreの口を通る() {
    let core = source(CORE);
    let save = core.require("save_inner");
    save.must_contain("ExternalChanged", "通常の保存が外部変更を断っていない");
    save.must_contain(
        "ExternalDeleted",
        "外で消されたファイルを区別していない（上書きで作り直す導線が無くなる）",
    );
    let reload = core.require("reload_from");
    reload.must_contain(
        "changed_span(",
        "読み直しが食い違った範囲だけを書き換えていない（履歴が全文 2 本ぶんになる）",
    );
    reload.must_contain(
        "self.apply_edit(",
        "読み直しが本文を書き換える唯一の口（#1651）を通っていない = undo で戻せない",
    );
    reload.must_contain(
        "Truncation::judge(",
        "読み直しが編集の上限（#1660）を見ていない",
    );

    let app = source(APP);
    let save = app.require("save_preview_local");
    save.must_contain(
        "save_overwrite()",
        "GUI / dispatch の上書きが core の口を通っていない",
    );
    save.must_contain("clear_conflict()", "上書きの後に競合を解いていない");
    let revert = app.require("revert_preview_local");
    revert.must_contain("reload_from_disk()", "読み直しが core の口を通っていない");
    revert.must_contain("clear_conflict()", "読み直しの後に競合を解いていない");
    app.require("save_preview").must_contain(
        "push_preview_remote_sync(pane, force)",
        "上書きの指定がリモートの書き戻し（#966）へ渡っていない",
    );
    // 帯のボタンが同じ口を呼ぶ
    let render = source("crates/tako-app/src/preview_render.rs");
    let bar = render.require("render_conflict_bar");
    bar.must_contain(
        "save_preview_local(pane_id, true)",
        "帯の「上書き保存」が上書きの口を呼んでいない",
    );
    bar.must_contain(
        "revert_preview_local(pane_id)",
        "帯の「読み直す」が読み直しの口を呼んでいない",
    );
    bar.must_contain(
        "toggle_conflict_diff(pane_id)",
        "帯の「差分」が差分を開いていない",
    );
}

/// 6: dispatch → CLI → MCP が 1:1（設計原則 5。UI だけの逃げ道を作らない）
#[test]
fn dispatchとcliとmcpが1対1で配線されている() {
    let dispatch = source(DISPATCH);
    let why = "外部変更の後の逃げ道が CLI / MCP から使えない（設計原則 5）";
    dispatch.arm_calls(
        "Request::PreviewSave { pane, force } =>",
        "host.save_preview(target, force)",
        4,
        why,
    );
    dispatch.arm_calls(
        "Request::PreviewRevert { pane } =>",
        "host.revert_preview(target)",
        4,
        why,
    );
    dispatch.arm_calls(
        "Request::PreviewDiff { pane } =>",
        ".preview_disk_diff(target)",
        6,
        why,
    );
    dispatch.require("preview_edit_reply").must_contain(
        "host.preview_conflict(target)",
        "編集系の応答に競合（`conflict`）が載らない = 保存が通らない理由を CLI / MCP が読めない",
    );
    dispatch.require("open_file").must_contain(
        "holds_other_unsaved(",
        "未保存のプレビューへ別のファイルを差し替えようとして断られる（開けない行き止まり）",
    );

    let mcp = source(MCP_REQUEST);
    for (needle, what) in [
        (
            r#"Some("overwrite") => Request::PreviewSave { pane, force: true }"#,
            "overwrite",
        ),
        (
            r#"Some("reload") => Request::PreviewRevert { pane }"#,
            "reload",
        ),
        (r#"Some("diff") => Request::PreviewDiff { pane }"#, "diff"),
    ] {
        mcp.must_have(
            needle,
            "\"tako_preview_save\"",
            &format!("MCP `tako_preview_save` の action={what} が dispatch へ写っていない"),
        );
    }
    source(MCP_CATALOG).must_have(
        "crate::dispatch::PREVIEW_SAVE_ACTIONS",
        "\"tako_preview_save\"",
        "カタログの action の enum が綴り表の 1 つから組まれていない（写像と食い違う）",
    );

    let cli = source(CLI);
    for (needle, what) in [
        ("force: *force,", "save --force"),
        (
            "EditCommand::Reload { pane } => Request::PreviewRevert",
            "reload",
        ),
        ("EditCommand::Diff { pane } => Request::PreviewDiff", "diff"),
    ] {
        cli.must_have(
            needle,
            "EditCommand::Save",
            &format!("CLI `tako edit {what}` が dispatch へ写っていない"),
        );
    }
}

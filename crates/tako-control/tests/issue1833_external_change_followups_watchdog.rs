//! **#1833 の番犬**: 外部変更の続き（#1659 の後）が塞いだ 3 つの穴を開け直さないようにする。
//!
//! ## 何が起きていたのか（修正前。今の main の隔離 GUI + CLI で再現）
//!
//! 1. 読み直しは undo できる 1 回の編集として積むので、読み直した直後の ⌘Z で本文が
//!    **外部変更より前の自分の版**へ戻る。基準（baseline）はディスク（外部変更）と同じなので
//!    自動保存がそのまま通り、**外部変更を黙って上書きした**（監視が黙って追従した後の ⌘Z も同じ）
//! 2. リモート（SSH 先）への書き戻しが `RemoteErrorKind::Conflict` で断られても、ヘッダの表示だけが
//!    「競合」で、帯にも `conflict` の応答にも `notices` にも載らなかった。記録が
//!    `save_status = Some(if … { SaveStatus::Conflict } else { … })` と if 式の中に書かれ、
//!    #1659 の番犬（`save_status = Some(…Conflict)` の字面）を素通りしていた
//! 3. 上書きの綴りが CLI `--force` / MCP `action=overwrite` で違った
//!
//! ## ここで止める 4 つ
//!
//! 1. [`読み直しを戻した間は自動保存を回さない`] — Issue の本体
//! 2. [`リモートの競合はローカルと同じ口で記録する`] — 字面ではなく `SaveStatus::Conflict` を
//!    書く箇所すべてを見る（if 式の中に隠しても名指す）
//! 3. [`帯の上書きはリモートにもforceで押し出す`] — 押し出しが force 無しだと同じ競合で断られる
//! 4. [`cliの上書きはmcpと同じ綴りの別名を持つ`]
//!
//! 落ちるときは **file:line で名指し**する。往復の中身は `tako_core::text_edit` の単体
//! （`読み直しを戻したかは…`）と `tako-app` の `preview::tests`（`issue1833_*`）、
//! 実経路（隔離 GUI + CLI + MCP + 偽の ssh / sftp）は `scripts/test-external-change-1833.sh`。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const CORE: &str = "crates/tako-core/src/text_edit.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象 1 ファイルの 2 つの眺め（同じバイト長。`code` は波括弧を数える用、
/// `view` はコメントだけ潰して識別子とリテラルを見る用 = #1609）
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

/// 関数 1 本（開始行と、本体が占める行の範囲を持つ）
struct Body {
    rel: &'static str,
    name: String,
    line: usize,
    end_line: usize,
    text: String,
}

impl Source {
    /// 関数の本体を名前で引く（修飾子に依らない = #1496）
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
                end_line: self.code[..end].lines().count(),
                text: self.view[open..end].to_string(),
            });
        }
        None
    }

    fn require(&self, name: &str) -> Body {
        self.body(name).unwrap_or_else(|| {
            panic!(
                "{}:1 `fn {name}` が見つからない。\n\
                 改名したなら番犬の名前も直すこと（#1833 の配線が外れたまま緑になる）",
                self.rel
            )
        })
    }

    /// `needle` を含む行（1 起点）
    fn line_with(&self, needle: &str) -> Option<usize> {
        self.view
            .lines()
            .position(|line| line.contains(needle))
            .map(|i| i + 1)
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

    fn contains_line(&self, line: usize) -> bool {
        (self.line..=self.end_line).contains(&line)
    }
}

/// 1: 読み直しを undo で戻した本文（外部変更より前の版）を自動保存で書かない
#[test]
fn 読み直しを戻した間は自動保存を回さない() {
    // core: 読み直しの差分に印を付け、undo / redo で数え、保存・読み直しで外す
    let core = source(CORE);
    core.require("reload_from").must_contain(
        "kind: EditKind::Reload",
        "読み直しの差分に印が無い = undo で読み直しを戻したことが分からない",
    );
    core.require("reload_from").must_contain(
        "self.reloads_undone = 0",
        "読み直した後も前に戻した読み直しの印が残る（自動保存が止まったままになる）",
    );
    for name in ["undo", "redo"] {
        let body = core.require(name);
        body.must_contain(
            "EditKind::Reload",
            "undo / redo が読み直しの差分を見ていない（戻したかを数えられない）",
        );
        body.must_contain("self.reloads_undone", "戻した読み直しの数を動かしていない");
    }
    core.require("save_inner").must_contain(
        "self.reloads_undone = 0",
        "保存した後も印が残る（明示の保存で外部変更を上書きした後も自動保存が止まったまま）",
    );

    // GUI: 自動保存の判定・保存の直前・undo の後・自動保存を入れたとき
    let preview = source(PREVIEW);
    preview.require("autosave_due").must_contain(
        "self.buffer.reload_undone()",
        "自動保存の保留へ入れる判定が「読み直しを戻した」を見ていない。\n\
         戻した直後の自動保存が外部変更を黙って上書きする（#1833 の本体）",
    );
    preview
        .require("hold_autosave_if_reload_undone")
        .must_contain(
            "self.note_conflict(",
            "止めたことを競合として知らせていない（帯・`conflict`・`notices` に載らない）",
        );
    preview.require("clear_conflict").must_contain(
        "self.buffer.reload_undone()",
        "監視が「基準と同じ」を見ると止めていた帯が消える（本文は外部変更より前の版のまま）",
    );
    let app = source(APP);
    app.require("run_autosave").must_contain(
        "reload_undone()",
        "500ms 待った後の保存が「読み直しを戻した」を見ていない（保留の後に ⌘Z した回で上書きする）",
    );
    app.require("preview_undo_local").must_contain(
        "hold_autosave_if_reload_undone()",
        "undo（GUI の ⌘Z・CLI・MCP の 1 実装）の後に自動保存を止めて知らせていない",
    );
    app.require("set_preview_autosave").must_contain(
        "hold_autosave_if_reload_undone()",
        "戻した本文のまま自動保存を入れたときに知らせていない（黙って止まる）",
    );
}

/// 2: リモートの競合を帯・`conflict`・`notices` の口（`note_conflict_on`）で記録する
#[test]
fn リモートの競合はローカルと同じ口で記録する() {
    let app = source(APP);
    let push = app.require("apply_remote_push_result");
    push.must_contain(
        "note_remote_conflict()",
        "リモートへの書き戻しの競合を帯・応答・回数の口で記録していない（#1833 の 2）",
    );
    push.must_contain(
        "clear_remote_conflict()",
        "書き戻しが通ってもリモートの競合を解いていない（帯が残る）",
    );

    // `SaveStatus::Conflict` を**書く**のは書き手の本体だけ。比べる（`==`）・腕の頭（`=>`）は除く。
    // 字面（`save_status = Some(…Conflict)`）ではなく行で見るので、if 式の中に隠しても名指す
    let preview = source(PREVIEW);
    let writer = preview.require("note_conflict_on");
    let mut offenders = Vec::new();
    for rel in [PREVIEW, APP, SIDEBAR, RENDER] {
        let src = source(rel);
        for (i, line) in src.view.lines().enumerate() {
            let line_no = i + 1;
            if !line.contains("SaveStatus::Conflict") || line.contains("==") || line.contains("=>")
            {
                continue;
            }
            if rel == PREVIEW && writer.contains_line(line_no) {
                continue;
            }
            offenders.push(format!("{rel}:{line_no} {}", line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "競合の状態を `note_conflict_on` を通さずに書いている:\n{}\n\n\
         `EditState::note_conflict` / `note_remote_conflict` を呼ぶこと（帯・`conflict` の応答・\n\
         `notices`・自動保存の停止がそこで決まる。#1659 / #1833）",
        offenders.join("\n")
    );

    // ローカルの写しの保存・監視ではリモートの競合を解かない
    preview.require("clear_conflict").must_contain(
        "c.remote",
        "ローカルの写しの保存・監視がリモートの競合を解く（tako 自身の書き込みで帯が消える）",
    );
    // 応答に相手を載せる（ローカルの形は変えない）
    let dispatch = source(DISPATCH);
    dispatch.require("preview_edit_reply").must_contain(
        "conflict.remote",
        "応答の `conflict` にリモートの印が無い（CLI / MCP が抜け方を選べない）",
    );
    dispatch.require("preview_save").must_contain(
        "!conflict.remote",
        "リモートの競合にローカルの抜け方（読み直し・差分 = 写しが相手）を添えている",
    );
}

/// 3: 帯の「上書き保存」はリモートへも上書きで押し出す
#[test]
fn 帯の上書きはリモートにもforceで押し出す() {
    let render = source(RENDER);
    render.require("render_conflict_bar").must_contain(
        "push_preview_remote_async(pane_id, true",
        "帯の「上書き保存」がリモートへ force 無しで押し出す = 同じ競合で断られて抜けられない",
    );
    let app = source(APP);
    app.require("push_preview_remote_async").must_contain(
        "save_file(&host, &path, &body, force)",
        "押し出しが force を書き戻しへ渡していない",
    );
}

/// 4: CLI の上書きに MCP の `action=overwrite` と同じ綴りの別名がある
#[test]
fn cliの上書きはmcpと同じ綴りの別名を持つ() {
    let cli = source(CLI);
    let line = cli
        .line_with("visible_alias = \"overwrite\"")
        .unwrap_or_else(|| {
            panic!(
                "{CLI}:{} `tako edit save` の `--force` に `--overwrite` の別名が無い。\n\
             MCP `tako_preview_save` の `action=overwrite` と綴りが食い違う（#1833 の 3）",
                cli.line_with("/// 外で変わっていても自分の変更で上書きする")
                    .unwrap_or(1)
            )
        });
    let next = cli.view.lines().nth(line).unwrap_or_default();
    assert!(
        next.contains("force: bool"),
        "{CLI}:{line} `--overwrite` の別名が `force` 以外に付いている: {next}"
    );
    let dispatch = source(DISPATCH);
    assert!(
        dispatch
            .line_with("pub const PREVIEW_SAVE_ACTIONS: &[&str] = &[\"save\", \"overwrite\"")
            .is_some(),
        "{DISPATCH}:{} MCP の action の綴り表から `overwrite` が消えた（CLI の別名と食い違う）",
        dispatch.line_with("PREVIEW_SAVE_ACTIONS").unwrap_or(1)
    );
}

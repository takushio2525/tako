//! **#1658 の番犬**: 「GUI でできて AI にできない編集操作」が戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! 編集の口は `PreviewApply { text }`（全文置換）1 つだけだった。5,000 行の
//! ファイルの 1 行を直すのに本文を丸ごと IPC で送り、`set_text` なのでカーソルは
//! 末尾へ飛び、undo も全文 1 個になっていた。外から「いまの文書の版」を読む口も
//! 無いので、GUI の打鍵と AI の編集がぶつかったことを誰も検知できなかった。
//!
//! ## ここで止める 7 つ
//!
//! 1. [`範囲編集とカーソルの配線が5か所すべてに在る`] — protocol / dispatch / CLI /
//!    MCP 写像 / MCP カタログのどれか 1 つだけが消える形（1:1 が片肺になる）
//! 2. [`編集系の応答は1実装から組む`] — アームごとに `json!` を書いて、**新しい
//!    編集口だけ版を載せ忘れる**形（外から版を読めない操作ができる）
//! 3. [`版が進む場所は1つだけ`] — `self.version` の書き換えが散らばり、
//!    「版が進まない編集」ができて楽観ロックが黙って素通りする形
//! 4. [`行桁の解決は丸めない`] — `resolve_position` が `min` / `snap_cursor` で
//!    黙って寄せる形（送り手の思っていない場所が変わる）
//! 5. [`本文を変える公開apiはすべて版を進める`] — 書き換え口を足したのに版を
//!    進め忘れる形。**挙動で見る**ので、内部表現が変わっても効く
//! 6. [`textbufferの書き換え口は棚卸し済み`] — 5 の表に載っていない新しい書き換え口が
//!    足される形。`TextBuffer` の本文は**私有フィールド**なので、外から本文を変える道は
//!    `&mut self` の公開 API だけ = ここを棚卸ししておけば取りこぼしが起きない
//!    （GUI の打鍵も IME も貼り付けもこの口を通るので、**GUI で変えても版が進む**
//!    ことがこの 2 本の合成で決まる）
//! 7. [`一行直すのに全文を送らない`] — Issue の症状そのもの。5,000 行の 1 行を直す
//!    IPC の**実バイト数**を、全文置換と範囲編集で比べて固定する
//!
//! 落ちるときは **file:line で名指し**する。
//!
//! ## 相方
//!
//! 行・桁の解釈と境界（範囲外 / 文字の途中 / CRLF / 空範囲 / 全範囲）の実挙動は
//! `tako_core::text_edit` の単体テスト。dispatch を通した往復は
//! `tako-control` の `preview範囲編集は行桁で1行だけを差し替えて版を返す` ほか。
//! ここは**配線と不変条件**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;
use tako_core::text_edit::{CursorPlacement, RangeEdit, TextBuffer, TextPosition};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const PROTOCOL: &str = "crates/tako-control/src/protocol.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけの眺め（コメントは潰す / 文字列は囲みごと残す。#1609）。
///
/// **肯定の存在確認**（この綴りが在るか）はこれを通す。全文へ `contains` すると
/// この番犬や走査先の説明文で緑になる
fn prod_view(rel: &str) -> String {
    let src = read(rel);
    without_comments_checked(&production_range::production(&src, rel), rel)
}

/// 眺めの中で `needle` が最初に現れる行番号（1 始まり）
fn line_of(view: &str, needle: &str) -> Option<usize> {
    let at = view.find(needle)?;
    Some(view[..at].bytes().filter(|b| *b == b'\n').count() + 1)
}

/// 関数 1 本の本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    line: usize,
    /// コメントだけ潰した眺めでの本体（リテラルの中身が残っている）
    text: String,
}

/// 関数の本体を名前で引く。**見つからなければ `None`**（改名の検出は呼び出し側）
fn body(rel: &str, name: &str) -> Option<Body> {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
    // 波括弧の対応は「コメントも文字列も潰した眺め」で数える（中の括弧を数えない）
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    assert_eq!(
        code.len(),
        view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = code[start..].find('{')? + start;
        let mut depth = 0usize;
        let mut end = open;
        for (i, byte) in code[open..].bytes().enumerate() {
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
            line: code[..start].bytes().filter(|b| *b == b'\n').count() + 1,
            text: view[open..end].to_string(),
        });
    }
    None
}

// --- 1) 1:1 の配線 -----------------------------------------------------------

/// 範囲編集とカーソルは **tako-core 操作 API + dispatch + CLI + MCP** の 5 か所で
/// 配線されている（AGENTS.md の開発不変条件）。1 つでも欠けたら片肺になる
#[test]
fn 範囲編集とカーソルの配線が5か所すべてに在る() {
    // (ファイル, 在るべき綴り, 何の配線か)
    let wiring: [(&str, &str, &str); 12] = [
        (PROTOCOL, "PreviewEditRange {", "protocol の Request"),
        (PROTOCOL, "PreviewCursor {", "protocol の Request"),
        (DISPATCH, "Request::PreviewEditRange {", "dispatch のアーム"),
        (DISPATCH, "Request::PreviewCursor {", "dispatch のアーム"),
        (DISPATCH, "host.edit_preview_range(", "ControlHost 呼び出し"),
        (DISPATCH, "host.set_preview_cursor(", "ControlHost 呼び出し"),
        (
            MCP_REQUEST,
            "\"tako_preview_edit_range\" => Request::PreviewEditRange",
            "MCP 名から Request への写像",
        ),
        (
            MCP_REQUEST,
            "\"tako_preview_cursor\" => Request::PreviewCursor",
            "MCP 名から Request への写像",
        ),
        (
            MCP_CATALOG,
            "\"name\": \"tako_preview_edit_range\"",
            "MCP カタログ",
        ),
        (
            MCP_CATALOG,
            "\"name\": \"tako_preview_cursor\"",
            "MCP カタログ",
        ),
        (CLI, "EditCommand::ReplaceRange {", "CLI サブコマンド"),
        (CLI, "EditCommand::Cursor {", "CLI サブコマンド"),
    ];
    let mut missing = Vec::new();
    for (rel, needle, what) in wiring {
        if !prod_view(rel).contains(needle) {
            missing.push(format!("{rel}:1 {what}（{needle}）が無い"));
        }
    }
    assert!(
        missing.is_empty(),
        "範囲編集 / カーソルの 1:1 配線が欠けている:\n{}",
        missing.join("\n")
    );
}

// --- 2) 応答の組み立ては 1 実装 ----------------------------------------------

/// 編集系のアームは [`preview_edit_reply`] を通る。
///
/// アームの中で `"editing":` を直書きすると、そのアームだけ `document`（版）が
/// 載らない応答になる = **外から版を読めない編集口**ができる
#[test]
fn 編集系の応答は1実装から組む() {
    let view = prod_view(DISPATCH);
    assert!(
        view.contains("fn preview_edit_reply("),
        "{DISPATCH}:1 応答を組む 1 実装 preview_edit_reply が無い（改名したらここも直す）"
    );
    // 1 実装の中身（版を載せる呼び出し）が消えていないこと
    let reply = body(DISPATCH, "preview_edit_reply")
        .unwrap_or_else(|| panic!("{DISPATCH}:1 preview_edit_reply の本体を取れない"));
    for needle in ["host.preview_document(", "\"document\""] {
        assert!(
            reply.text.contains(needle),
            "{DISPATCH}:{} preview_edit_reply が {needle} を通らない（応答から版が消える）",
            reply.line
        );
    }
    // dispatch_inner の中で、編集系アームが `"editing":` を直書きしていないこと
    let inner = body(DISPATCH, "dispatch_inner")
        .unwrap_or_else(|| panic!("{DISPATCH}:1 dispatch_inner の本体を取れない"));
    let base = line_of(&view, "fn dispatch_inner(").unwrap_or(1);
    let mut offenders = Vec::new();
    for (i, line) in inner.text.lines().enumerate() {
        if line.contains("\"editing\":") {
            offenders.push(format!(
                "{DISPATCH}:{} 編集系の応答を直書きしている（preview_edit_reply を使う）",
                base + i
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "版の載らない応答ができる形:\n{}",
        offenders.join("\n")
    );
}

// --- 3) 版が進む場所は 1 つ ---------------------------------------------------

/// `self.version` への**書き込み**は `bump_version` の中だけ。
///
/// 散らばると「版が進まない編集」ができ、`expected_version` の楽観ロックが
/// 黙って素通りする（守っているつもりで守れていない状態がいちばん危ない）
#[test]
fn 版が進む場所は1つだけ() {
    let src = read(TEXT_EDIT);
    let prod = production_range::production(&src, TEXT_EDIT);
    let code = code_view(&prod);
    let bump = body(TEXT_EDIT, "bump_version")
        .unwrap_or_else(|| panic!("{TEXT_EDIT}:1 bump_version が無い"));
    assert!(
        bump.text.contains("self.version"),
        "{TEXT_EDIT}:{} bump_version が版を書き換えていない",
        bump.line
    );
    // `self.version` に続く最初の非空白が代入なら書き込み
    let mut writes = Vec::new();
    let mut at = 0usize;
    while let Some(found) = code[at..].find("self.version") {
        let pos = at + found;
        at = pos + "self.version".len();
        let rest = code[at..].trim_start();
        // `==`（比較）と `=>`（match の腕）は書き込みではない
        let is_write = rest.starts_with("+=")
            || rest.starts_with("-=")
            || (rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>"));
        if !is_write {
            continue;
        }
        let line = code[..pos].bytes().filter(|b| *b == b'\n').count() + 1;
        // bump_version の本体の中なら合法
        let inside = line >= bump.line && line <= bump.line + bump.text.lines().count();
        if !inside {
            writes.push(format!(
                "{TEXT_EDIT}:{line} bump_version の外で版を書き換えている"
            ));
        }
    }
    assert!(
        writes.is_empty(),
        "版の進め方が散らばっている:\n{}",
        writes.join("\n")
    );
}

// --- 4) 解決は丸めない --------------------------------------------------------

/// 行・桁の解決は**丸めずに拒否する**。
///
/// `min` / `clamp` / `snap_cursor` / `unwrap_or` を通すと、外から来た範囲指定が
/// 黙って別の場所を指す（送り手は「3 行目を直した」と思っているのに他所が変わる）
#[test]
fn 行桁の解決は丸めない() {
    let resolve = body(TEXT_EDIT, "resolve_position")
        .unwrap_or_else(|| panic!("{TEXT_EDIT}:1 resolve_position が無い（改名したらここも直す）"));
    for banned in [".min(", ".max(", ".clamp(", "snap_cursor(", "unwrap_or"] {
        assert!(
            !resolve.text.contains(banned),
            "{TEXT_EDIT}:{} resolve_position が {banned} で黙って丸めている",
            resolve.line
        );
    }
    // 丸めない代わりに理由を返していること
    for needed in [
        "ZeroLine",
        "LineOutOfRange",
        "ColumnOutOfRange",
        "NotCharBoundary",
    ] {
        assert!(
            resolve.text.contains(needed),
            "{TEXT_EDIT}:{} resolve_position が {needed} を返さない",
            resolve.line
        );
    }
}

// --- 5) 挙動: 本文を変える公開 API はすべて版を進める -------------------------

/// 本文を変える公開 API（版が進むべきもの）。[`本文を変える公開apiはすべて版を進める`] が
/// 挙動で確かめ、[`textbufferの書き換え口は棚卸し済み`] が取りこぼしを見る
const MUTATES_TEXT: [&str; 14] = [
    "set_text",
    "insert",
    "newline",
    "delete_backward",
    "delete_forward",
    // 単位を指定した削除（#1652。⌥⌫ / ⌘⌫ と CLI / MCP の `delete`）
    "delete",
    // Tab / ⇧Tab / Enter（#1654。CLI / MCP の `indent` / `outdent` / `newline`）
    "indent",
    "outdent",
    "newline_and_indent",
    "undo",
    "redo",
    "replace_range",
    "replace_all",
    "replace_position_range",
];

/// 本文を変えない `&mut self` の公開 API（版は進まないのが正しい）。
///
/// カーソル・選択は「どこを見ているか」で文書の中身ではない。`save` はディスクへ
/// 書くだけで本文を変えない（変えると保存のたびに版が飛び、楽観ロックが使えなくなる）
const KEEPS_TEXT: [&str; 7] = [
    "set_cursor",
    "select_all",
    "move_cursor",
    "set_cursor_placement",
    "save",
    // 選択をまとめて置く（#1652。GUI の画面選択の写し戻し）
    "set_selection",
    // 器に見える行数（#1652。ページ移動の歩幅。本文ではない）
    "set_viewport_lines",
];

/// 検査する操作 1 つ（名前 + バッファへ当てる手）
type Op = (&'static str, Box<dyn Fn(&mut TextBuffer)>);

/// 書き換え口を足したのに版を進め忘れる形を**挙動で**止める。
///
/// ソースの綴りではなく「操作したら版が増えたか」を見るので、内部表現が
/// 変わっても（履歴が差分になっても）効く
#[test]
fn 本文を変える公開apiはすべて版を進める() {
    let path = PathBuf::from("watchdog-1658.txt");
    // (名前, 操作)。**本文が変わるものだけ**を並べる
    let ops: Vec<Op> = vec![
        ("insert", Box::new(|b: &mut TextBuffer| b.insert("x"))),
        ("newline", Box::new(|b: &mut TextBuffer| b.newline())),
        (
            "delete_backward",
            // カーソルが 0 のままだと消すものが無い = 本文が変わらない
            Box::new(|b: &mut TextBuffer| {
                b.move_cursor(tako_core::text_edit::CursorMovement::DocumentEnd, false);
                b.delete_backward()
            }),
        ),
        (
            "delete_forward",
            Box::new(|b: &mut TextBuffer| {
                b.set_cursor(0, false);
                b.delete_forward()
            }),
        ),
        ("indent", Box::new(|b: &mut TextBuffer| b.indent())),
        (
            "outdent",
            // 浅くできる行を先に作る。set_text も版を進めるので、outdent 自身が
            // 進めたかはこの中で確かめる（外の比較だけだと set_text の分で緑になる）
            Box::new(|b: &mut TextBuffer| {
                b.set_text("    one\n".into());
                b.set_cursor(0, false);
                let before = b.version();
                b.outdent();
                assert!(b.version() > before, "outdent が版を進めていない");
            }),
        ),
        (
            "newline_and_indent",
            Box::new(|b: &mut TextBuffer| b.newline_and_indent()),
        ),
        (
            "delete",
            Box::new(|b: &mut TextBuffer| {
                b.move_cursor(tako_core::text_edit::CursorMovement::DocumentEnd, false);
                b.delete(tako_core::text_edit::DeleteMotion::WordBackward)
            }),
        ),
        (
            "set_text",
            Box::new(|b: &mut TextBuffer| b.set_text("new\nbody\n".into())),
        ),
        (
            "replace_range",
            Box::new(|b: &mut TextBuffer| b.replace_range(0..1, "Z")),
        ),
        (
            "replace_all",
            Box::new(|b: &mut TextBuffer| {
                b.replace_all("o", "0");
            }),
        ),
        (
            "replace_position_range",
            Box::new(|b: &mut TextBuffer| {
                b.replace_position_range(&RangeEdit {
                    start: TextPosition::new(1, 0),
                    end: TextPosition::new(1, 1),
                    text: "Q".into(),
                    expected_version: None,
                })
                .expect("1 行目の 1 バイト目は解ける");
            }),
        ),
        (
            "undo",
            Box::new(|b: &mut TextBuffer| {
                b.undo();
            }),
        ),
        (
            "redo",
            Box::new(|b: &mut TextBuffer| {
                b.redo();
            }),
        ),
    ];
    // 表と手が食い違っていたら、足したほうだけが検査されない
    let mut listed: Vec<&str> = ops.iter().map(|(name, _)| *name).collect();
    listed.sort_unstable();
    let mut expected: Vec<&str> = MUTATES_TEXT.to_vec();
    expected.sort_unstable();
    assert_eq!(
        listed, expected,
        "MUTATES_TEXT と実際に当てる手が食い違っている（片方だけ足した）"
    );
    for (name, op) in &ops {
        let mut buffer = TextBuffer::from_text(path.clone(), "one\ntwo\n".into());
        // undo / redo は「戻すものがある」状態を先に作る
        if *name == "undo" || *name == "redo" {
            buffer.insert("seed");
        }
        if *name == "redo" {
            assert!(buffer.undo());
        }
        let before = buffer.version();
        op(&mut buffer);
        assert!(
            buffer.version() > before,
            "{name} が版を進めていない（{before} のまま）。\
             本文を変える口は bump_version を通すこと"
        );
    }
    // 逆に、本文を変えないカーソル移動は版を進めない（進めると毎回の移動で
    // 楽観ロックが外れ、`expected_version` が使い物にならなくなる）
    let mut buffer = TextBuffer::from_text(path, "one\ntwo\n".into());
    let before = buffer.version();
    buffer
        .set_cursor_placement(&CursorPlacement {
            cursor: TextPosition::new(1, 1),
            select_to: Some(TextPosition::new(2, 2)),
            expected_version: None,
        })
        .expect("解ける位置");
    assert_eq!(
        buffer.version(),
        before,
        "カーソル移動で版が進んでいる（本文は変わっていない）"
    );
}

/// `TextBuffer` の `&mut self` 公開 API は**すべて棚卸し済み**。
///
/// 本文（`text`）は私有フィールドなので、外から中身を変える道はここだけ。
/// 新しい書き換え口を足したら、版を進めるのか進めないのかをこの表に書く
/// （書かないと落ちる）。GUI の打鍵・IME・貼り付けもこの口を通るので、
/// **表が埋まっていれば「GUI で変えたのに版が進まない」は起こらない**
#[test]
fn textbufferの書き換え口は棚卸し済み() {
    let src = read(TEXT_EDIT);
    let code = code_view(&production_range::production(&src, TEXT_EDIT));
    let mut unknown = Vec::new();
    let mut seen = Vec::new();
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim_start();
        // 関数の頭かどうかは共有ヘルパが決める（#1496。`pub(crate) fn` / `async fn` を
        // 取りこぼすと、中の違反が**手前の関数名**で報告される）
        let Some(name) = fn_head_name(trimmed) else {
            continue;
        };
        // 外から触れて本文を変えうるのは「公開 + `&mut self`」だけ
        if !trimmed.starts_with("pub") || !trimmed.contains("(&mut self") {
            continue;
        }
        seen.push(name.to_string());
        if !MUTATES_TEXT.contains(&name) && !KEEPS_TEXT.contains(&name) {
            unknown.push(format!(
                "{TEXT_EDIT}:{} 棚卸しされていない書き換え口 {name}（版を進めるなら \
                 MUTATES_TEXT へ、進めないなら KEEPS_TEXT へ理由つきで足す）",
                i + 1
            ));
        }
    }
    assert!(
        unknown.is_empty(),
        "TextBuffer に新しい書き換え口がある:\n{}",
        unknown.join("\n")
    );
    // 表の側が死んでいないこと（改名すると挙動テストが空振りする）
    for name in MUTATES_TEXT.iter().chain(KEEPS_TEXT.iter()) {
        assert!(
            seen.iter().any(|s| s == name),
            "{TEXT_EDIT}:1 表にある {name} が TextBuffer に無い（改名したら表も直す）"
        );
    }
}

/// Issue の症状（1 行直すのに全文を送る）を**バイト数**で固定する。
///
/// 測るのは IPC に載る `Request` の JSON。時間ではなく大きさなので、
/// 負荷で反転しない（`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
#[test]
fn 一行直すのに全文を送らない() {
    let body: String = (1..=5000)
        .map(|i| format!("line {i:04}: the quick brown fox jumps over the lazy dog\n"))
        .collect();
    let line = "line 2500: the quick brown fox jumps over the lazy dog";
    let replacement = "line 2500: REPLACED";

    let apply = tako_control::protocol::Request::PreviewApply {
        pane: Some(7),
        text: body.replace(line, replacement),
    };
    let range = tako_control::protocol::Request::PreviewEditRange {
        pane: Some(7),
        start_line: 2500,
        start_col: 0,
        end_line: 2500,
        end_col: line.len(),
        text: replacement.into(),
        expected_version: None,
    };
    let apply_bytes = serde_json::to_string(&apply).expect("直列化できる").len();
    let range_bytes = serde_json::to_string(&range).expect("直列化できる").len();
    assert!(
        range_bytes * 100 < apply_bytes,
        "1 行の差し替えが全文置換の 1/100 未満になっていない\
         （範囲編集 {range_bytes} バイト / 全文置換 {apply_bytes} バイト）"
    );
}

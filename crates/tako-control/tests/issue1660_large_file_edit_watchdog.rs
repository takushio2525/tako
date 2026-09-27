//! **#1660 の番犬**: 大きいファイルが編集できない / 断るときに無言、へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! プレビューの読み込み上限（= 編集の上限）が 1 MB / 5,000 行で、tako 自身の `main.rs`
//! （8 万行超）が tako で編集できなかった。断るときは「末尾を省略した大きいファイルは
//! 安全のため編集できない」の固定文だけで、**何を超えたのか・上限はいくつか**を言わず、
//! 画面は編集ボタンを黙って消すだけだった。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`上限の値は10万行と10mbで正本は1か所`] — 上限を旧値へ戻す / tako-app に数字を
//!    直書きして正本と食い違わせる形
//! 2. [`上限を超えたら理由と値を返す`] — 境目（ちょうど上限 / 上限 + 1）の判定と、
//!    断る文面に値が載ることを **tako-core の API を叩いて**見る
//! 3. [`断る理由は画面とcliとmcpへ同じ1実装から出る`] — `EditState::open` が固定文へ戻る /
//!    `tako edit` の応答から `limit` が消える / フッタが値を出さない形
//! 4. [`大きい文書の全文の塗りはuiスレッドでやらない`] — 編集開始の全文ハイライトを
//!    同期へ戻す（10 万行で 2.24 秒止まる）形
//! 5. [`行頭索引は本文を書き換える3つの口すべてで追従する`] — 行・桁の換算を文書の
//!    先頭から数え直す形へ戻す / 索引の追従を書き換え口の 1 つで忘れる形
//! 6. [`描画の行頭は表示行の版だけで使い回さない`] — 行頭のキャッシュの鍵から出どころ
//!    （編集セッションのバッファの版）を落とし、閲覧中の ⌘F の後も表示行から数えた行頭を
//!    使い続ける形（CRLF の文書で検索の強調がずれる。#1800 / #1802 との合流で発見）
//!
//! 落ちるときは **file:line で名指し**する。挙動（塗りの background 化・打ち切り・
//! 表示の文字が本文と一致）は `tako-app` の `preview::large_file_tests`、
//! dispatch を通した往復は `tako-control` の `preview上限を超えた編集は理由と値を返す`、
//! 実 GUI は `scripts/test-large-file-edit-1660.sh`（装飾の実ピクセルは visual-test 節
//! `large-file-decor`）。ここは**配線と不変条件**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::preview_limit::{Truncation, MAX_BYTES, MAX_LINES};
use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const LIMIT: &str = "crates/tako-core/src/preview_limit.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";
const UI_TEXT: &str = "crates/tako-app/src/ui_text/preview.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
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

/// 本番コードだけの眺め（コメントは潰す / 文字列は囲みごと残す。#1609）
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
    text: String,
}

/// 関数の本体を名前で引く（最初に見つかったもの）。見つからなければ `rel:1` で落とす
fn body(rel: &str, name: &str) -> Body {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
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
        let open = code[start..].find('{').expect("関数の本体") + start;
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
        return Body {
            line: code[..start].bytes().filter(|b| *b == b'\n').count() + 1,
            text: view[open..end].to_string(),
        };
    }
    panic!("{rel}:1 `fn {name}` が見つからない（改名したならこの番犬も直す）");
}

fn must_contain(rel: &str, body: &Body, name: &str, needle: &str, why: &str) {
    assert!(
        body.text.contains(needle),
        "{rel}:{} `fn {name}` が `{needle}` を通っていない（#1660）。\n{why}",
        body.line
    );
}

// --- 1) 上限の値と正本 --------------------------------------------------------

/// 上限は **10 万行 / 10 MB**。値の正本は `tako_core::preview_limit` の 1 か所で、
/// tako-app はそこから引く（数字を直書きすると正本と食い違って判定が 2 つになる）
#[test]
fn 上限の値は10万行と10mbで正本は1か所() {
    let view = prod_view(LIMIT);
    for (needle, what) in [
        (
            "pub const MAX_BYTES: usize = 10_000_000;",
            "バイト数の上限（10 MB）",
        ),
        (
            "pub const MAX_LINES: usize = 100_000;",
            "行数の上限（10 万行）",
        ),
    ] {
        let name = needle.split(':').next().unwrap();
        let at = line_of(&view, name).unwrap_or(1);
        assert!(
            view.contains(needle),
            "{LIMIT}:{at} {what}が `{needle}` ではない（#1660。旧値 1 MB / 5,000 行では \
             tako 自身の main.rs = 8 万行超が編集できなかった。上げ下げするなら計測を添えて \
             この番犬と architecture.md「大きいファイルの編集」を直す）"
        );
    }
    // 値そのもの（文字列の置き方を変えても効く）
    assert_eq!(MAX_BYTES, 10_000_000, "{LIMIT}: MAX_BYTES");
    assert_eq!(MAX_LINES, 100_000, "{LIMIT}: MAX_LINES");

    // tako-app は正本から引く。数字の直書きは file:line で名指す
    let preview = prod_view(PREVIEW);
    for (needle, what) in [
        (
            "MAX_BYTES: usize = tako_core::preview_limit::MAX_BYTES;",
            "MAX_BYTES",
        ),
        (
            "MAX_LINES: usize = tako_core::preview_limit::MAX_LINES;",
            "MAX_LINES",
        ),
    ] {
        let at = line_of(&preview, &format!("const {what}")).unwrap_or(1);
        assert!(
            preview.contains(needle),
            "{PREVIEW}:{at} `{what}` が `tako_core::preview_limit` から引いていない（#1660。\
             数字を直書きすると画面の判定と CLI / MCP の文面の値が食い違う）"
        );
    }
}

// --- 2) 境目と文面 --------------------------------------------------------------

/// ちょうど上限は省略しない・1 つ超えたら理由（行数 / 大きさ）と値を返す。
/// 断る文面と `limit` の JSON に**数えた値と上限の両方**が載る
#[test]
fn 上限を超えたら理由と値を返す() {
    assert_eq!(
        Truncation::judge(MAX_BYTES, Some(MAX_BYTES as u64), MAX_LINES),
        None,
        "{LIMIT}: ちょうど上限は全文を読む"
    );
    let lines = Truncation::judge(MAX_BYTES, Some(MAX_BYTES as u64), MAX_LINES + 1)
        .expect("上限 + 1 行は省略する");
    let bytes = Truncation::judge(MAX_BYTES + 1, Some(MAX_BYTES as u64 + 1), 1)
        .expect("上限 + 1 バイトは省略する");
    for (limit, value, reason) in [
        (lines, "100,001 行（上限 100,000 行）", "lines"),
        (bytes, "10 MB（上限 10 MB）", "bytes"),
    ] {
        let refusal = limit.edit_refusal();
        assert!(
            refusal.contains(value),
            "{LIMIT}: 断る文面に値と上限が無い（#1660。無言で断ると、なぜ編集できないかが \
             分からない）: {refusal}"
        );
        let json = limit.to_json();
        assert_eq!(json["reason"], reason, "{LIMIT}: limit.reason");
        assert_eq!(json["max_lines"], MAX_LINES, "{LIMIT}: limit.max_lines");
        assert_eq!(json["max_bytes"], MAX_BYTES, "{LIMIT}: limit.max_bytes");
        assert_eq!(
            json["message"], refusal,
            "{LIMIT}: limit.message と断る文面は同じ"
        );
        assert!(
            limit
                .detail_ja()
                .contains(value.split('（').next().unwrap()),
            "{LIMIT}: 画面の文面（detail_ja）にも値が載る"
        );
    }
}

// --- 3) 画面と CLI / MCP へ同じ 1 実装から ------------------------------------------

/// 断る理由は **`Truncation` の 1 実装**から画面（フッタ）・`tako edit start` のエラー・
/// `tako edit` の応答（`limit`）へ出る。どれか 1 つが固定文・無言へ戻る形を落とす
#[test]
fn 断る理由は画面とcliとmcpへ同じ1実装から出る() {
    let open = body(PREVIEW, "open");
    must_contain(
        PREVIEW,
        &open,
        "open",
        "limit.edit_refusal()",
        "EditState::open が理由と値を言わずに断っている（旧: 固定文だけ）",
    );
    let read_source = body(PREVIEW, "read_text_source");
    must_contain(
        PREVIEW,
        &read_source,
        "read_text_source",
        "Truncation::judge(",
        "読み込みの上限判定が正本（`Truncation::judge`）を通っていない = 画面と応答の判定が 2 つになる",
    );
    let reply = body(DISPATCH, "preview_edit_reply");
    for needle in ["host.preview_limit(", "out[\"limit\"]"] {
        must_contain(
            DISPATCH,
            &reply,
            "preview_edit_reply",
            needle,
            "`tako edit` の応答（CLI / MCP 共通の 1 実装）から上限の理由が消えている",
        );
    }
    let host_impl = body(MAIN, "preview_limit");
    must_contain(
        MAIN,
        &host_impl,
        "preview_limit",
        "to_json()",
        "GUI の ControlHost が上限の理由を返していない（応答の `limit` が常に無くなる）",
    );
    let footer = body(UI_TEXT, "tail_omitted");
    for needle in ["detail_ja()", "detail_en()"] {
        must_contain(
            UI_TEXT,
            &footer,
            "tail_omitted",
            needle,
            "フッタが何を超えたか・上限を出していない（旧: 「大きいファイルのため」だけ）",
        );
    }
    let render = prod_view(PREVIEW_RENDER);
    assert!(
        render.contains("tail_omitted(&limit)"),
        "{PREVIEW_RENDER}:{} フッタが上限の理由を受け取っていない（#1660）",
        line_of(&render, "tail_omitted").unwrap_or(1)
    );
}

// --- 4) 大きい文書の塗り ----------------------------------------------------------

/// 旧上限（5,000 行）を超える文書は、編集開始時の全文ハイライトを UI スレッドでやらない
/// （background へ出す）。1 回の差分の塗りも上限で打ち切る
#[test]
fn 大きい文書の全文の塗りはuiスレッドでやらない() {
    let apply = body(PREVIEW, "apply_editor_text");
    for (needle, why) in [
        (
            "!defer",
            "大きい文書でも全文へ落ちてよい（allow_full = true）形に戻っている = 10 万行で 2.24 秒止まる",
        ),
        (
            "start_plain(",
            "全文を塗らない代わりの平文の表示を組んでいない",
        ),
        (
            "refresh_plain_incremental(",
            "塗りが戻るまでの打鍵が平文の差分で済んでいない",
        ),
    ] {
        must_contain(PREVIEW, &apply, "apply_editor_text", needle, why);
    }
    let incremental = body(PREVIEW, "refresh_incremental");
    must_contain(
        PREVIEW,
        &incremental,
        "refresh_incremental",
        "budget_lines",
        "差分の塗りに打ち切りが無い = 先頭で `/*` を打つと 10 万行の末尾まで塗る（約 2 秒）",
    );
    let view = prod_view(PREVIEW);
    let at = line_of(&view, "const SYNC_FULL_HIGHLIGHT_MAX_LINES").unwrap_or(1);
    assert!(
        view.contains("const SYNC_FULL_HIGHLIGHT_MAX_LINES: usize = 5_000;"),
        "{PREVIEW}:{at} 同期で全文を塗る境目が旧上限（5,000 行）ではない（#1660。上げると \
         編集開始の UI 停止が戻る。下げるなら #1648 のテストと合わせて直す）"
    );
    let main = prod_view(MAIN);
    assert!(
        main.contains("self.drain_pending_editor_seeds(cx);"),
        "{MAIN}:{} render の入口が全文の塗りを起こしていない = 大きい文書の色が永久に揃わない",
        line_of(&main, "fn render(").unwrap_or(1)
    );
}

// --- 5) 行頭索引 ------------------------------------------------------------------

/// 行・桁の換算は `TextBuffer` の行頭索引（二分探索）で引き、本文を書き換える 3 つの口
/// すべてが索引を追従させる。1 つでも忘れると、その編集のあと行・桁がずれる
#[test]
fn 行頭索引は本文を書き換える3つの口すべてで追従する() {
    for name in ["apply_edit", "undo", "redo"] {
        let b = body(TEXT_EDIT, name);
        must_contain(
            TEXT_EDIT,
            &b,
            name,
            "self.splice_line_starts(",
            "本文を書き換えたのに行頭索引を直していない = 以後の行・桁の換算がずれる",
        );
    }
    let b = body(TEXT_EDIT, "line_byte_col");
    must_contain(
        TEXT_EDIT,
        &b,
        "line_byte_col",
        "self.line_starts.partition_point(",
        "行・桁の換算が行頭索引を使っていない",
    );
    assert!(
        !b.text.contains("== b'\\n').count()"),
        "{TEXT_EDIT}:{} `fn line_byte_col` が文書の先頭から改行を数え直している（#1660。\
         GUI は表示中の行ごと・選択の往復・追従で 1 フレームに何度も呼ぶ = 末尾で 1 回 1.5ms）",
        b.line
    );
    let count = body(TEXT_EDIT, "line_count");
    must_contain(
        TEXT_EDIT,
        &count,
        "line_count",
        "self.line_starts.len()",
        "行数を文書全体から数え直している",
    );
}

// --- 6) 描画の行頭の出どころ -------------------------------------------------------

/// 描画は行テキストと行頭を表示行の版（`content_rev`）で使い回すが、行頭は**出どころ**
/// （編集セッションがあればそのバッファ、無ければ表示行）も鍵に含める。
///
/// 閲覧中の ⌘F は編集セッションを生やすだけで表示行の版を進めない。版だけを鍵にすると
/// 表示行から数えた行頭（CR を落とした行 + 1）が残り、CRLF の文書では検索の強調が
/// 1 行につき 1 バイトずつ手前へずれて、末尾近くの行には描かれない（実 GUI で再現 =
/// visual-test 節 `large-file-decor` の crlf view-search が FAILED）
#[test]
fn 描画の行頭は表示行の版だけで使い回さない() {
    let render = body(PREVIEW_RENDER, "render_preview_pane");
    for (needle, why) in [
        (
            "preview::code_line_texts(",
            "行テキストの使い回しを通っていない（この規則の前提。改名したなら直す）",
        ),
        (
            "edit.buffer.version()",
            "行頭の出どころ（編集セッションのバッファの版）を鍵に含めていない = 閲覧中の ⌘F の後も \
             表示行から数えた行頭が残る",
        ),
        (
            "self.preview_line_cache_rev.insert(pane_id, (rev, source))",
            "行頭を取り直したときに出どころを控えていない",
        ),
        (
            "!same_source",
            "出どころが変わったのに行頭を取り直さない（表示行の版が同じなら使い回してしまう）",
        ),
    ] {
        must_contain(PREVIEW_RENDER, &render, "render_preview_pane", needle, why);
    }
}

//! **#1692 の番犬**: 行操作（複製・移動・行インデント・コメントの付け外し）の形を止める。
//!
//! 挙動（7 操作 × 選択の有無の全文・端の移動・undo 1 回・CRLF・表に 1 行足すだけで効く）は
//! `tako_core::text_edit::line_ops` の単体、打鍵の両 OS の列は `platform::editor_keys` の単体、
//! キーバインドとの衝突は tako-app の `エディタの打鍵表はキーバインドと衝突しない`、
//! CLI / MCP の配線は `issue1652_editor_keys_watchdog` の `打鍵とcliとmcpは同じ口を通る` が見る。
//! ここは**入れ方の形**だけを見る:
//!
//! 1. [`行操作のロジックはコメント記号を直書きしない`] — 言語ごとの記号は表の行が持つ。
//!    ロジックへ `"#"` や `"//"` を書いた形（表の外で言語を足す = 受け入れ条件「表以外の
//!    コード変更 0 行」が崩れる）
//! 2. [`対象の行の決め方は1実装`] — Tab の行インデント（`shift_lines`）と行操作が別々に
//!    「選択の終わりが行頭なら含めない」を書く形
//! 3. [`当てられない理由をguiの実行口が捨てない`] — `EditorCommand::apply` の `Err`
//!    （コメント記号を知らない形式）を捨てて、打鍵も CLI / MCP も黙って何も起きない形
//! 4. [`行操作はeditorcommandの口だけを通る`] — GUI / dispatch が `apply_line_operation` を
//!    直接呼んで、打鍵と CLI / MCP の口が割れる形
//!
//! 落ちるときは **file:line で名指し**する。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;
use tako_core::text_edit::{CommentSyntax, COMMENT_TABLE};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const LINE_OPS: &str = "crates/tako-core/src/text_edit/line_ops.rs";
const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

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

/// 本番コードだけの眺め（コメントは潰す / 文字列は囲みごと残す）
fn prod_view(rel: &str) -> String {
    let src = read(rel);
    without_comments_checked(&production_range::production(&src, rel), rel)
}

/// 関数 1 本の本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: &'static str,
    name: &'static str,
    /// `fn` の頭の行
    line: usize,
    /// 本体の開き括弧の行（シグネチャが複数行に渡ると `line` より下になる）
    open_line: usize,
    /// コメントだけ潰した眺めでの本体（開き括弧から。リテラルの中身が残っている）
    text: String,
}

impl Body {
    fn line_at(&self, at: usize) -> usize {
        self.open_line + self.text[..at].bytes().filter(|b| *b == b'\n').count()
    }

    fn lines_of(&self, needle: &str) -> Vec<usize> {
        self.text
            .match_indices(needle)
            .map(|(at, _)| self.line_at(at))
            .collect()
    }

    fn at(&self, line: usize) -> String {
        format!("{}:{line}", self.rel)
    }

    fn head(&self) -> String {
        format!("{}:{} fn {}", self.rel, self.line, self.name)
    }
}

/// 関数の本体を名前で引く。**見つからなければ落とす**（改名で番犬が空振りしない）
fn body(rel: &'static str, name: &'static str) -> Body {
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
        let open = code[start..]
            .find('{')
            .map(|i| i + start)
            .unwrap_or_else(|| panic!("{rel}: fn {name} の本体が無い"));
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
        let line_of = |at: usize| code[..at].bytes().filter(|b| *b == b'\n').count() + 1;
        return Body {
            rel,
            name,
            line: line_of(start),
            open_line: line_of(open),
            text: view[open..end].to_string(),
        };
    }
    panic!("{rel}:1 fn {name} が見つからない（改名したらこの番犬も直す）");
}

/// 本体の中の文字列リテラル（開きの `"` の位置と中身）。`\"` のエスケープだけを解く
fn string_literals(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 文字リテラルの `'"'` は文字列の始まりではない
        if bytes[i] == b'\'' && bytes.get(i + 2) == Some(&b'\'') {
            i += 3;
            continue;
        }
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let open = i;
        i += 1;
        let mut content = Vec::new();
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                content.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            content.push(bytes[i]);
            i += 1;
        }
        out.push((open, String::from_utf8_lossy(&content).into_owned()));
        i += 1;
    }
    out
}

/// 表に載っている記号すべて（行コメントの記号・囲みの始まりと終わり）
fn table_tokens() -> Vec<&'static str> {
    let mut tokens: Vec<&str> = COMMENT_TABLE
        .iter()
        .flat_map(|row| match row.syntax {
            CommentSyntax::Line(token) => vec![token],
            CommentSyntax::Block { open, close } => vec![open, close],
        })
        .collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens
}

// --- 1) 記号は表の行が持つ ------------------------------------------------------

/// 行操作のロジック（記号の判定・付け外し・表の引き方）は記号の文字列を 1 つも持たない。
/// 記号を直書きした行を file:line で名指す（`if ext == "py" { "#" }` の形へ戻すと、
/// その言語だけ表に行が無いのに効き、表の行と食い違っても気づけない）
#[test]
fn 行操作のロジックはコメント記号を直書きしない() {
    let tokens = table_tokens();
    assert!(
        tokens.len() >= 5,
        "{LINE_OPS}:1 コメント記号の表を読めていない（{tokens:?}）"
    );
    let mut found = Vec::new();
    for name in [
        "apply_line_operation",
        "comment_syntax",
        "comment_syntax_for",
        "comment_syntax_in",
        "comment_lines",
        "is_commented",
        "uncomment_cuts",
        "duplicate_lines",
        "move_lines",
        "line_block",
        "follow",
        "leading_ws",
    ] {
        let b = body(LINE_OPS, name);
        for (at, literal) in string_literals(&b.text) {
            let trimmed = literal.trim();
            if let Some(token) = tokens
                .iter()
                .find(|t| trimmed == **t || trimmed.starts_with(**t))
            {
                found.push(format!(
                    "{} fn {name} が記号 {token:?} を直書きしている（\"{literal}\"）。\
                     言語ごとの記号はコメント記号の表の行へ書く",
                    b.at(b.line_at(at))
                ));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// --- 2) 対象の行の決め方 ---------------------------------------------------------

/// 「選択が触れる行・終わりが行頭ならその行を含めない」は `line_block` の 1 実装。
/// Tab の行インデント（`shift_lines`）が自前で数え直す形と、行操作が引かない形を名指す
#[test]
fn 対象の行の決め方は1実装() {
    let mut problems = Vec::new();
    let shift = body(TEXT_EDIT, "shift_lines");
    if shift.lines_of("self.line_block(").is_empty() {
        problems.push(format!("{} が line_block を引いていない", shift.head()));
    }
    for line in shift.lines_of("line_break_len_before(") {
        problems.push(format!(
            "{} shift_lines が選択の終わりの扱いを自前で数え直している（line_block へ寄せる）",
            shift.at(line)
        ));
    }
    for name in ["duplicate_lines", "move_lines", "comment_lines"] {
        let b = body(LINE_OPS, name);
        if b.lines_of("self.line_block(").is_empty() {
            problems.push(format!("{} が line_block を引いていない", b.head()));
        }
    }
    let block = body(LINE_OPS, "line_block");
    if block.lines_of("line_break_len_before(").is_empty() {
        problems.push(format!(
            "{} が選択の終わりの行頭を数えていない（行を丸ごと選ぶと次の行まで動く）",
            block.head()
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// --- 3) 当てられない理由 ---------------------------------------------------------

/// GUI の実行口（打鍵と CLI / MCP の両方が通る）は `EditorCommand::apply` の結果を捨てない。
/// 捨てると、コメント記号を知らない形式で ⌘/ を押しても `tako edit line toggle-comment` を
/// 叩いても**黙って何も起きない**（理由がフッタにもエラーにも出ない）
#[test]
fn 当てられない理由をguiの実行口が捨てない() {
    let runner = body(APP, "run_editor_command_local");
    let calls = runner.lines_of(".apply(&mut edit.buffer)");
    assert!(
        !calls.is_empty(),
        "{} が EditorCommand::apply を呼んでいない",
        runner.head()
    );
    let mut dropped = Vec::new();
    for (at, _) in runner.text.match_indices(".apply(&mut edit.buffer)") {
        let rest = &runner.text[at + ".apply(&mut edit.buffer)".len()..];
        let rest = rest.trim_start();
        if !(rest.starts_with('?') || rest.starts_with(".map_err") || rest.starts_with(".is_err")) {
            dropped.push(format!(
                "{} EditorCommand::apply の結果（当てられない理由）を捨てている",
                runner.at(runner.line_at(at))
            ));
        }
    }
    assert!(dropped.is_empty(), "{}", dropped.join("\n"));
}

// --- 4) 口は 1 つ ----------------------------------------------------------------

/// GUI と dispatch は `apply_line_operation` を直接呼ばない（`EditorCommand::apply` が唯一の口。
/// 直接呼ぶと打鍵と CLI / MCP のどちらかだけ別の結果になる = #1652 と同じ形）
#[test]
fn 行操作はeditorcommandの口だけを通る() {
    let mut direct = Vec::new();
    for rel in [APP, DISPATCH] {
        let view = prod_view(rel);
        for (at, _) in view.match_indices(".apply_line_operation(") {
            let line = view[..at].bytes().filter(|b| *b == b'\n').count() + 1;
            direct.push(format!(
                "{rel}:{line} apply_line_operation を直接呼んでいる（EditorCommand::Line を apply する）"
            ));
        }
    }
    assert!(direct.is_empty(), "{}", direct.join("\n"));
    // 口の実体: EditorCommand::apply が Line を apply_line_operation へ渡している
    let apply = body("crates/tako-core/src/platform/editor_keys.rs", "apply");
    assert!(
        !apply.lines_of(".apply_line_operation(").is_empty(),
        "{} が行操作を TextBuffer::apply_line_operation へ渡していない",
        apply.head()
    );
}

/// 走査が空振りしていないこと（文字列リテラルの切り出しが働く）
#[test]
fn 番犬の走査が成立している() {
    let literals = string_literals(r#"let a = "x\"y"; let b = '"'; f("//");"#);
    assert_eq!(
        literals.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(),
        ["x\"y", "//"]
    );
    let b = body(LINE_OPS, "comment_lines");
    assert!(b.text.len() > 200, "{} の本体が短すぎる", b.head());
}

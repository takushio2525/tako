//! **#1759 の番犬**: Enter の続き（字下げを戻すキーワード・リストの印）と括弧の自動閉じの**入れ方の形**を止める。
//!
//! 挙動（境界の全文・undo 1 回・CRLF・文字列 / コメントの中・設定 OFF）は
//! `tako_core::text_edit` の単体、表の内訳は `file_type` の単体、CLI / MCP の写像は
//! tako-cli と `mcp::tests`、実 GUI の打鍵は visual-test `enter-1759`
//! （`scripts/test-editor-enter-1759.sh`）が見る。ここは形だけ:
//!
//! 1. 戻すキーワードの語（`return` …）をロジックへ直書きしない。語は `file_type` の行の
//!    `dedent_after` 列が持つ（言語名・語を入口に書くと、表の行と食い違っても気づけない）
//! 2. GUI の文字入力と CLI / MCP の `type` は同じ打鍵の口（`EditorCommand::Type` →
//!    `TextBuffer::type_char`）を通る。GUI だけ `insert` へ戻ると、打鍵と AI の結果が割れる
//! 3. Enter（`newline_and_indent`）は 2 つの規則（`continue_list` / `dedents_after` +
//!    `has_unclosed_open`）を通る
//! 4. 自動閉じは飛び越しと対（#1654 の判断）: `type_char` が飛び越し（`take_closing_at`）と
//!    文字列 / コメントの判定（`opens_in_code`）を持ち、入れた閉じ括弧の位置は本文を書き換える
//!    唯一の口（`apply_edit`）で追従し、undo / redo で手放す
//!
//! 落ちるときは **file:line で名指し**する。注入（[`注入した形をそれぞれ名指す`]）で
//! 各検査の検出力を固定する。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const APP: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";
const ENTER: &str = "crates/tako-core/src/text_edit/enter.rs";
const EDITOR_KEYS: &str = "crates/tako-core/src/platform/editor_keys.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 検査するソース（注入で書き換えた写しを渡せるよう、読み込みと検査を分ける）
#[derive(Clone)]
struct Sources {
    files: Vec<(&'static str, String)>,
}

impl Sources {
    fn load() -> Self {
        let root = repo_root();
        let files = [APP, DISPATCH, TEXT_EDIT, ENTER, EDITOR_KEYS]
            .into_iter()
            .map(|rel| {
                let text = std::fs::read_to_string(root.join(rel))
                    .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
                (rel, text)
            })
            .collect();
        Self { files }
    }

    fn get(&self, rel: &str) -> &str {
        &self
            .files
            .iter()
            .find(|(r, _)| *r == rel)
            .expect("読み込んだファイル")
            .1
    }

    /// `rel` の中の `from` を `to` へ 1 か所だけ差し替えた写し（注入）
    fn with(&self, rel: &str, from: &str, to: &str) -> Self {
        let mut out = self.clone();
        let text = &mut out.files.iter_mut().find(|(r, _)| *r == rel).unwrap().1;
        assert!(text.contains(from), "{rel}: 注入の元 {from:?} が無い");
        *text = text.replacen(from, to, 1);
        out
    }
}

/// 関数 1 本の本体（コメントだけ潰した眺め。文字列は残す）と、本体の開き括弧の行
struct Body {
    rel: &'static str,
    name: &'static str,
    open_line: usize,
    text: String,
}

impl Body {
    fn line_at(&self, at: usize) -> usize {
        self.open_line + self.text[..at].bytes().filter(|b| *b == b'\n').count()
    }

    fn has(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }

    fn head(&self) -> String {
        format!("{}:{} fn {}", self.rel, self.open_line, self.name)
    }
}

/// 関数の本体を名前で引く（最初に見つかった同名の関数）。見つからなければ `None`
fn body(sources: &Sources, rel: &'static str, name: &'static str) -> Option<Body> {
    let prod = production_range::production(sources.get(rel), rel);
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = start + code[start..].find('{')?;
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
        let open_line = code[..open].bytes().filter(|b| *b == b'\n').count() + 1;
        return Some(Body {
            rel,
            name,
            open_line,
            text: view[open..end].to_string(),
        });
    }
    None
}

/// 本体の中の文字列リテラル（開きの `"` の位置と中身）
fn string_literals(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
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

/// 表（file_type）の `dedent_after` 列に載っている語すべて
fn table_words() -> Vec<&'static str> {
    let mut words: Vec<&str> = tako_core::file_type::TABLE
        .iter()
        .flat_map(|row| row.dedent_after.iter().copied())
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// 関数に要る呼び出しの並び（ファイル, 関数, 要るもの, 外れたときの意味）
const REQUIRED: &[(&str, &str, &str, &str)] = &[
    (
        APP,
        "replace_text_in_range",
        "EditorCommand::Type(",
        "GUI の文字入力が打鍵の口（EditorCommand::Type）を通らない = CLI / MCP の type と結果が割れる",
    ),
    (
        EDITOR_KEYS,
        "apply",
        "buffer.type_char(",
        "EditorCommand::Type が TextBuffer::type_char を通らない",
    ),
    (
        EDITOR_KEYS,
        "typed",
        "Self::Type",
        "CLI / MCP の type が打鍵 1 文字ずつ（EditorCommand::Type）へ並ばない",
    ),
    (
        TEXT_EDIT,
        "newline_and_indent",
        "self.continue_list(",
        "Enter がリストの印の引き継ぎ（continue_list）を通らない",
    ),
    (
        TEXT_EDIT,
        "newline_and_indent",
        "enter::dedents_after(",
        "Enter が字下げを戻すキーワード（dedents_after）を通らない",
    ),
    (
        TEXT_EDIT,
        "newline_and_indent",
        "has_unclosed_open(",
        "字下げを戻す前に「文が続いている」（閉じていない括弧）を見ていない",
    ),
    (
        TEXT_EDIT,
        "type_char",
        "take_closing_at(",
        "自動閉じが飛び越しと対になっていない（手で打った閉じ括弧が二重になる = #1654 の判断）",
    ),
    (
        TEXT_EDIT,
        "type_char",
        "opens_in_code(",
        "自動閉じが文字列・コメントの中を見分けていない",
    ),
    (
        TEXT_EDIT,
        "type_char",
        "self.auto_close_active()",
        "自動閉じが設定と形式（auto_close 列）を見ていない",
    ),
    (
        TEXT_EDIT,
        "apply_edit",
        "self.auto_pairs.follow(",
        "入れた閉じ括弧の位置が本文の書き換えに追従しない（飛び越す位置がずれる）",
    ),
    (
        TEXT_EDIT,
        "undo",
        "self.auto_pairs.clear()",
        "undo の後も入れた閉じ括弧を追い続ける（位置の対応が取れない）",
    ),
    (
        TEXT_EDIT,
        "redo",
        "self.auto_pairs.clear()",
        "redo の後も入れた閉じ括弧を追い続ける（位置の対応が取れない）",
    ),
];

fn problems(sources: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, name, needle, meaning) in REQUIRED {
        let rel: &'static str = rel;
        let name: &'static str = name;
        match body(sources, rel, name) {
            Some(b) if b.has(needle) => {}
            Some(b) => out.push(format!("{} に {needle} が無い: {meaning}", b.head())),
            None => out.push(format!(
                "{rel}:1 fn {name} が見つからない（改名したらこの番犬も直す）"
            )),
        }
    }
    // dispatch は巨大な 1 関数の腕なので、本番コードの中に居るかで見る
    let dispatch = without_comments_checked(
        &production_range::production(sources.get(DISPATCH), DISPATCH),
        DISPATCH,
    );
    if !dispatch.contains("EditorCommand::typed(") {
        out.push(format!(
            "{DISPATCH}:1 PreviewEditCommand の type が EditorCommand::typed を通らない（打鍵の口と割れる）"
        ));
    }
    // 戻すキーワードの語はロジックに書かない（file_type の dedent_after 列が持つ）
    let words = table_words();
    for (rel, name) in [
        (ENTER, "dedents_after"),
        (ENTER, "list_item"),
        (TEXT_EDIT, "newline_and_indent"),
        (TEXT_EDIT, "continue_list"),
    ] {
        let Some(b) = body(sources, rel, name) else {
            out.push(format!(
                "{rel}:1 fn {name} が見つからない（改名したらこの番犬も直す）"
            ));
            continue;
        };
        for (at, literal) in string_literals(&b.text) {
            if words.contains(&literal.trim()) {
                out.push(format!(
                    "{}:{} fn {name} が語 {literal:?} を直書きしている。語は file_type の行の \
                     dedent_after 列へ書く",
                    b.rel,
                    b.line_at(at)
                ));
            }
        }
    }
    out
}

#[test]
fn enterの続きと自動閉じの入れ方の形() {
    assert!(
        table_words().contains(&"return"),
        "crates/tako-core/src/file_type.rs:1 dedent_after 列を読めていない（{:?}）",
        table_words()
    );
    let found = problems(&Sources::load());
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 各検査が実際に効くか（その形へ戻した写しで、その検査が file:line で名指す）
#[test]
fn 注入した形をそれぞれ名指す() {
    let base = Sources::load();
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            APP,
            "tako_core::platform::editor_keys::EditorCommand::Type(c)",
            "tako_core::platform::editor_keys::EditorCommand::Newline",
            "fn replace_text_in_range に EditorCommand::Type(",
        ),
        (
            TEXT_EDIT,
            "&& self.auto_pairs.take_closing_at(cursor)",
            "&& false",
            "fn type_char に take_closing_at(",
        ),
        (
            TEXT_EDIT,
            "self.auto_pairs.follow(splices);",
            "",
            "fn apply_edit に self.auto_pairs.follow(",
        ),
        (
            TEXT_EDIT,
            "if rules.list_continuation && self.continue_list(range.clone(), line_start) {",
            "if rules.list_continuation {",
            "fn newline_and_indent に self.continue_list(",
        ),
        (
            ENTER,
            "let statement = line.trim_start_matches([' ', '\\t']);",
            "let statement = line.trim_start_matches([' ', '\\t']);\n    let _ = \"return\";",
            "fn dedents_after が語 \"return\" を直書き",
        ),
        (
            DISPATCH,
            "EditorCommand::typed(",
            "EditorCommand::from_edit_name(",
            "EditorCommand::typed を通らない",
        ),
    ];
    for (rel, from, to, want) in cases {
        let found = problems(&base.with(rel, from, to));
        assert!(
            found.iter().any(|p| p.contains(want) && p.contains(rel)),
            "{rel} の注入（{from:?} → {to:?}）を名指さない: {found:?}"
        );
    }
}

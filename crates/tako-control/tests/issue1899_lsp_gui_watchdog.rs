//! LSP の続き 3（#1899）の GUI の構造の番犬
//!
//! # なぜ要るか
//!
//! 受け入れ条件は**どれも 1 行で壊せる**のに、壊しても CLI / MCP の答え（前から `status: loading`）は
//! 変わらない:
//!
//! - 補完の答えが待ちの上限に当たったとき、`finish_completion` が失敗をいつもどおり捨てると「読み込み中」が
//!   黙って消える（#1899 の 2 点目の症状）。描く側が上限の文を見ないと「読み込み中」のまま残る
//! - キー / メニューの要求の語を覚えない・描く側が見ないと、読み込み中はヘッダだけに戻る（コメント ①）
//! - 脚注のコマンドを GUI が自前で組むと、CLI の綴りが変わったときに打っても動かない文になる（コメント ②。
//!   正本は `tako_control::lsp::hover::full_command` の 1 本 = tako-cli の単体がそのまま解けることを縛る）
//!
//! 挙動は単体（`CompletionError::limit_note`）と visual-test `completion-loading-limit` /
//! `hover-key-loading` / `hover-footer-1899`（`scripts/test-lsp-loading-limit-1899.sh`）が測り、ここは
//! **構造**を縛って file:line で名指す（GUI を立てない CI でも落ちる）。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const COMPLETION: &str = "crates/tako-control/src/lsp/completion.rs";
const GUI_COMPLETION: &str = "crates/tako-app/src/lsp_completion_ui.rs";
const GUI_HOVER: &str = "crates/tako-app/src/lsp_hover_ui.rs";

const FILES: &[&str] = &[COMPLETION, GUI_COMPLETION, GUI_HOVER];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let pad = " ".repeat(indent);
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == format!("{pad}}}"))
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_only(window: &str) -> String {
    window
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の行（1-based）
fn line_in(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| at + i)
}

/// 規則 1 つ: `file` の関数 `decl` のコードが `must` をすべて含み、`must_not` を含まない
struct Rule {
    file: &'static str,
    decl: &'static str,
    must: &'static [&'static str],
    must_not: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    // --- 2 点目: 補完の「読み込み中」が上限に当たったら理由の 1 行 ---
    Rule {
        file: COMPLETION,
        decl: "pub fn limit_note(&self, shown_loading: bool)",
        must: &[
            "Self::Loading { secs, .. } => (text::COMPLETION_LOADING_LIMIT_NOTE, *secs)",
            "Self::Query(GotoError::Timeout { secs, .. }) if shown_loading =>",
        ],
        must_not: &[],
        why: "上限に当たった答え（読み込み中 / 「読み込み中」を出していた未応答）を上限の 1 行にしていない",
    },
    Rule {
        file: GUI_COMPLETION,
        decl: "fn finish_completion(",
        must: &["error.limit_note(shown_loading)", "limit: Some(limit)"],
        must_not: &[],
        why: "上限に当たった答えで「読み込み中」を黙って消している（理由の 1 行へ差し替えていない）",
    },
    Rule {
        file: GUI_COMPLETION,
        decl: "fn render_loading_note(",
        must: &["note.limit.clone()"],
        must_not: &[],
        why: "上限の 1 行を描いていない（上限に当たっても「読み込み中」の文のまま）",
    },
    // --- コメント ①: キー / メニューの読み込み中を語の真下に ---
    Rule {
        file: GUI_HOVER,
        decl: "pub(crate) fn request_lsp_hover(",
        must: &["self.lsp_hover.explicit_target = self.explicit_hover_target("],
        must_not: &[],
        why: "キー / メニューで頼んだ位置の語を覚えていない（読み込み中を語の真下に出せない）",
    },
    Rule {
        file: GUI_HOVER,
        decl: "fn render_lsp_hover_loading(",
        must: &["self.explicit_loading_target()", "HoverOrigin::Explicit"],
        must_not: &[],
        why: "キー / メニューで頼んだホバーの読み込み中を語の真下に描いていない（ヘッダだけ）",
    },
    // --- コメント ②: 切ったカードの脚注に全文の取り方 ---
    Rule {
        file: GUI_HOVER,
        decl: "pub(crate) fn render_lsp_hover(",
        must: &["tako_control::lsp::hover::full_command("],
        must_not: &["\"tako lsp hover"],
        why: "切ったカードの脚注の全文の取り方が CLI と同じ 1 本（`full_command`）を通っていない",
    },
];

fn scan(sources: &[(&'static str, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for rule in RULES {
        let src = &sources
            .iter()
            .find(|(f, _)| *f == rule.file)
            .expect("走査対象")
            .1;
        let Some((at, window)) = fn_window(src, rule.decl) else {
            out.push(format!(
                "{}:0 — `{}` が見つからない（走査が空振り）",
                rule.file, rule.decl
            ));
            continue;
        };
        let code = code_only(&window);
        for needle in rule.must {
            if !code.contains(needle) {
                out.push(format!(
                    "{}:{at} — {}（`{needle}` が無い）",
                    rule.file, rule.why
                ));
            }
        }
        for needle in rule.must_not {
            if code.contains(needle) {
                let line = line_in(at, &window, needle).unwrap_or(at);
                out.push(format!(
                    "{}:{line} — {}（`{needle}` が在る）",
                    rule.file, rule.why
                ));
            }
        }
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    FILES.iter().map(|f| (*f, read(f))).collect()
}

#[test]
fn 続き_3_の_gui_の構造は縛りを満たす() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    for rule in RULES {
        let src = &sources.iter().find(|(f, _)| *f == rule.file).unwrap().1;
        let (_, window) = fn_window(src, rule.decl)
            .unwrap_or_else(|| panic!("{}: `{}` の窓が採れない", rule.file, rule.decl));
        assert!(
            window.lines().count() >= 3,
            "{}: `{}` の窓が短すぎる",
            rule.file,
            rule.decl
        );
    }
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること
fn assert_named(label: &str, file: &'static str, (from, to): (&str, &str), needle: &str) {
    let mut sources = current();
    let slot = sources.iter_mut().find(|(f, _)| *f == file).unwrap();
    assert!(
        slot.1.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    slot.1 = slot.1.replacen(from, to, 1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&sources);
    let expected = format!("{file}:{line} ");
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

#[test]
fn 逆戻りを名指しできる() {
    // A. 未応答を「読み込み中」の有無に関わらず黙らせる（#1899 前の未応答）
    assert_named(
        "A 未応答の上限を出さない",
        COMPLETION,
        (
            "Self::Query(GotoError::Timeout { secs, .. }) if shown_loading =>",
            "Self::Query(GotoError::Timeout { secs, .. }) if false =>",
        ),
        "pub fn limit_note(&self, shown_loading: bool)",
    );
    // B. 失敗をいつもどおり捨てる（#1899 前 = 上限で黙って消える）
    assert_named(
        "B 上限で黙って消える",
        GUI_COMPLETION,
        (
            ".then(|| error.limit_note(shown_loading))",
            ".then(|| None)",
        ),
        "fn finish_completion(",
    );
    // C. 描く側が上限の文を見ない
    assert_named(
        "C 上限の文を描かない",
        GUI_COMPLETION,
        (
            "let text = note.limit.clone().map_or_else(",
            "let text = None::<String>.map_or_else(",
        ),
        "fn render_loading_note(",
    );
    // D. キー / メニューの語を覚えない
    assert_named(
        "D 語を覚えない",
        GUI_HOVER,
        (
            "self.lsp_hover.explicit_target = self.explicit_hover_target(pane, &request);",
            "self.lsp_hover.explicit_target = None;",
        ),
        "pub(crate) fn request_lsp_hover(",
    );
    // E. 描く側がキー / メニューの読み込み中を見ない
    assert_named(
        "E 語の真下に描かない",
        GUI_HOVER,
        (
            "let Some(target) = self.explicit_loading_target() else {",
            "let Some(target) = None::<HoverTarget> else {",
        ),
        "fn render_lsp_hover_loading(",
    );
    // F. 脚注のコマンドを自前で組む
    assert_named(
        "F 脚注のコマンドを自前で組む",
        GUI_HOVER,
        (
            "tako_control::lsp::hover::full_command(",
            "format!(\"tako lsp hover --pane {} --line {} --column {} --full\", ",
        ),
        "format!(\"tako lsp hover",
    );
}

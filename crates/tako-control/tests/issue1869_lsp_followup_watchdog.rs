//! LSP の続き（#1869）の構造の番犬
//!
//! # なぜ要るか
//!
//! 受け入れ条件は**どれも 1 行で壊せる**のに、壊しても答えの形（CLI の JSON）は変わらない:
//!
//! - 整形の当て方を「最初の始まり〜最後の終わりの 1 か所」（`compose_changes`）へ戻すと、10 MB の
//!   文書で 1 回の整形が undo の予算 8 MiB を超え、その前の履歴が消える（#1869 の 1）
//! - 離れた箇所の当て直しを 1 か所ずつの `replace_range` にすると、10 万箇所の整形・undo が
//!   本文の長さ × 10 万の転送になる。undo を「つないだ差分を while で戻す」形へ戻すと、件数の上限
//!   （1000）でつながりの途中が捨てられ、半端に戻る（#1682 の形の穴）
//! - 打鍵の補完の要求を「読み込みを待たない」へ戻すと、rust-analyzer の読み込みの前半（即座に
//!   null で答える = 実測）で一覧が出ず、済んでも問い直さない（#1869 の 2 の真因）
//! - GUI が「読み込み中」を立てなくなる / 「読み込み中」を一覧が出ている扱いにして 5 キーを奪う
//!
//! 挙動は単体（`tako_core::text_edit` の `_1869`）・e2e（`issue1869_lsp_followup`）・
//! visual-test `completion-loading` が測り、ここは**構造**を縛って file:line で名指す
//! （GUI を立てない CI でも落ちる）。
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

const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";
const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const GUI: &str = "crates/tako-app/src/lsp_completion_ui.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_lines(window: &str) -> Vec<String> {
    window
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => l[..i].to_string(),
            None => l.to_string(),
        })
        .collect()
}

/// `lines` の最後の行の後ろで、まだ閉じていない繰り返しがあればその綴り（波括弧の深さで数える）
fn open_loop_at(lines: &[String]) -> Option<&'static str> {
    let mut depth: i64 = 0;
    let mut open: Vec<(i64, &'static str)> = Vec::new();
    for line in lines {
        for looping in ["for ", "while ", "loop {", ".for_each("] {
            if line.contains(looping) {
                open.push((depth, looping.trim()));
            }
        }
        depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        open.retain(|(at, _)| depth > *at);
    }
    open.last().map(|(_, looping)| *looping)
}

/// `lines` の最後の行の後ろが、`opener` を含む行で開いた塊の中か。深さは**文字ごと**に数える
/// （`} else {` は塊を閉じてから else の塊を開く = else 側は `opener` の塊の外）
fn inside_block(lines: &[String], opener: &str) -> bool {
    let mut depth: i64 = 0;
    let mut open: Vec<i64> = Vec::new();
    for line in lines {
        if line.contains(opener) {
            open.push(depth);
        }
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    open.retain(|at| depth > *at);
                }
                _ => {}
            }
        }
    }
    !open.is_empty()
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line} — {why}")
}

/// 窓を採る（無ければ空振りとして名指す）
fn window_or(
    out: &mut Vec<String>,
    file: &str,
    src: &str,
    needle: &str,
) -> Option<(usize, Vec<String>)> {
    match fn_window(src, needle) {
        Some((at, window)) => Some((at, code_lines(&window))),
        None => {
            out.push(report(
                file,
                0,
                &format!("`{needle}` が見つからない（走査が空振り）"),
            ));
            None
        }
    }
}

/// text_edit の検査（当て方の一本化・組み直し・undo 1 件）
fn scan_text_edit(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 1. 整形と補完の確定は同じ当て方（`splices_of`）。旧の 1 か所まとめは A/B の旧腕の中だけ
    for needle in ["pub fn apply_changes(", "pub fn replace_position_ranges("] {
        let Some((at, code)) = window_or(&mut out, TEXT_EDIT, src, needle) else {
            continue;
        };
        if !code.iter().any(|l| l.contains("splices_of(")) {
            out.push(report(
                TEXT_EDIT,
                at,
                "離れた箇所を離れたまま当てる `splices_of` を通っていない（整形と補完の当て方が 1 本でない）",
            ));
        }
        for (i, line) in code.iter().enumerate() {
            if line.contains("compose_changes(") && !inside_block(&code[..=i], "legacy_1869()") {
                out.push(report(
                    TEXT_EDIT,
                    at + i,
                    "最初の始まり〜最後の終わりの 1 か所へまとめて当てている（あいだの本文が undo へ載り、10 MB の文書で予算を超える）",
                ));
            }
            if line.contains("self.apply_edit(") {
                if let Some(looping) = open_loop_at(&code[..i]) {
                    out.push(report(
                        TEXT_EDIT,
                        at + i,
                        &format!("`apply_edit` を繰り返し（`{looping}`）の中で呼んでいる = 箇所ごとに差分を積む（undo が 1 回で戻らない・上限で途中が捨てられる）"),
                    ));
                }
            }
        }
    }
    // 2. 離れた箇所は 1 回で組み直す（1 か所ずつ詰め直さない）
    if let Some((at, code)) = window_or(&mut out, TEXT_EDIT, src, "fn splice_text(") {
        for (i, line) in code.iter().enumerate() {
            if line.contains("replace_range(") {
                if let Some(looping) = open_loop_at(&code[..i]) {
                    out.push(report(
                        TEXT_EDIT,
                        at + i,
                        &format!("離れた箇所を繰り返し（`{looping}`）の中で 1 か所ずつ置き換えている = 箇所の数 × 本文の長さの転送"),
                    ));
                }
            }
        }
        if !code.iter().any(|l| l.contains("compute_line_starts(")) {
            out.push(report(
                TEXT_EDIT,
                at,
                "組み直した本文から行頭索引を 1 回で作っていない（1 か所ずつ詰め直すと箇所の数 × 行数）",
            ));
        }
    }
    // 3. 差分 1 件 = undo 1 回（つないだ差分を while で戻す形へ戻らない）
    match src.find("struct EditDelta {") {
        None => out.push(report(
            TEXT_EDIT,
            0,
            "`struct EditDelta` が見つからない（走査が空振り）",
        )),
        Some(pos) => {
            let line = src[..pos].lines().count() + 1;
            let body = &src[pos..];
            let body = &body[..body.find("\n}").unwrap_or(body.len())];
            if !body.contains("spans:") {
                out.push(report(
                    TEXT_EDIT,
                    line,
                    "差分が離れた箇所の並び（`spans`）を持っていない（離れた箇所を 1 件に並べられない）",
                ));
            }
            if let Some(i) = body.lines().position(|l| l.contains("chained")) {
                out.push(report(
                    TEXT_EDIT,
                    line + i,
                    "差分を「つないで」持っている（件数の上限でつながりの途中が捨てられ、undo が半端に戻る）",
                ));
            }
        }
    }
    for needle in ["    pub fn undo(&mut self)", "    pub fn redo(&mut self)"] {
        let Some((at, code)) = window_or(&mut out, TEXT_EDIT, src, needle) else {
            continue;
        };
        if let Some(i) = code
            .iter()
            .position(|l| l.contains("while ") || l.contains("loop {"))
        {
            out.push(report(
                TEXT_EDIT,
                at + i,
                "undo / redo が差分を繰り返して戻している（1 回の操作 = 差分 1 件の形が崩れた）",
            ));
        }
    }
    out
}

/// manager の検査（打鍵の要求も読み込みを待つ・上限は `loading`）
fn scan_manager(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some((at, code)) = window_or(
        &mut out,
        MANAGER,
        src,
        "fn completion(&self, request: &CompletionRequest)",
    ) else {
        return out;
    };
    // 空の答えの後の待ちは `wait_after_empty`（#1930。定義ジャンプ・ホバーと共有の 1 実装）
    match code
        .iter()
        .position(|l| l.contains("self.wait_after_empty("))
    {
        None => out.push(report(MANAGER, at, "空の答えの後に読み込みを待っていない")),
        Some(i) => {
            if !code[i].contains("&abandoned") {
                out.push(report(
                    MANAGER,
                    at + i,
                    "読み込みの待ちが次の打鍵・閉じる・文書を閉じるで抜けない（古いスレッドが上限まで残る）",
                ));
            }
            // 待つかどうかの条件（直前の `if`）が打鍵の要求を外していない
            if let Some(j) = code[..i]
                .iter()
                .rposition(|l| l.contains("if parsed.items.is_empty()"))
            {
                if code[j].contains("superseding") {
                    out.push(report(
                        MANAGER,
                        at + j,
                        "打鍵の要求（`superseding`）は読み込みを待たない形に戻っている（読み込みの前半の null で一覧が出ず、済んでも問い直さない = #1869 の真因）",
                    ));
                }
            }
        }
    }
    if !code
        .iter()
        .any(|l| l.contains("Loading::TimedOut") || l.contains("RpcError::Timeout"))
        || code
            .iter()
            .filter(|l| l.contains("CompletionError::Loading"))
            .count()
            < 2
    {
        out.push(report(
            MANAGER,
            at,
            "上限まで読み込み中だったときに `loading` で答えていない（待たせる後半・空で答える前半の両方）",
        ));
    }
    out
}

/// GUI の検査（「読み込み中」を立てる・下ろす・キーを奪わない）
fn scan_gui(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (needle, must, why) in [
        (
            "fn fire_completion(",
            "self.lsp.server_loading(",
            "打鍵の問い合わせを出すときにサーバが読み込み中かを見ていない（「読み込み中」が出ない）",
        ),
        (
            "fn finish_completion(",
            ".loading = None",
            "答えが届いても「読み込み中」を下ろしていない",
        ),
        (
            "pub(crate) fn close_completion(",
            ".loading = None",
            "一覧を閉じても「読み込み中」が残る",
        ),
        (
            "pub(crate) fn render_lsp_completion(",
            "self.render_loading_note(",
            "「読み込み中」を描いていない",
        ),
    ] {
        let Some((at, code)) = window_or(&mut out, GUI, src, needle) else {
            continue;
        };
        if !code.iter().any(|l| l.contains(must)) {
            out.push(report(GUI, at, why));
        }
    }
    // 「読み込み中」は一覧が出ている扱いにしない（5 キーを奪わない）
    if let Some((at, code)) = window_or(&mut out, GUI, src, "pub(crate) fn lsp_completion_open_in(")
    {
        if let Some(i) = code.iter().position(|l| l.contains("loading")) {
            out.push(report(
                GUI,
                at + i,
                "「読み込み中」を一覧が出ている扱いにしている（Enter / Tab / ↑↓ / Esc を奪う）",
            ));
        }
    }
    out
}

fn current() -> (String, String, String) {
    (read(TEXT_EDIT), read(MANAGER), read(GUI))
}

fn all(text_edit: &str, manager: &str, gui: &str) -> Vec<String> {
    let mut out = scan_text_edit(text_edit);
    out.extend(scan_manager(manager));
    out.extend(scan_gui(gui));
    out
}

#[test]
fn 当て方と読み込み中の構造が契約どおり() {
    let (text_edit, manager, gui) = current();
    let reports = all(&text_edit, &manager, &gui);
    assert!(
        reports.is_empty(),
        "#1869 の契約が崩れた:\n{}",
        reports.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let (text_edit, manager, gui) = current();
    for needle in [
        "pub fn apply_changes(",
        "pub fn replace_position_ranges(",
        "fn splice_text(",
        "    pub fn undo(&mut self)",
        "    pub fn redo(&mut self)",
    ] {
        assert!(fn_window(&text_edit, needle).is_some(), "{needle}");
    }
    assert!(text_edit.contains("struct EditDelta {"));
    assert!(fn_window(
        &manager,
        "fn completion(&self, request: &CompletionRequest)"
    )
    .is_some());
    for needle in [
        "fn fire_completion(",
        "fn finish_completion(",
        "pub(crate) fn close_completion(",
        "pub(crate) fn render_lsp_completion(",
        "pub(crate) fn lsp_completion_open_in(",
    ] {
        assert!(fn_window(&gui, needle).is_some(), "{needle}");
    }
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること。
/// `needle` は `関数の宣言>>行の字面` と書くと、その関数の中で最初に出る行を指す
fn assert_named(
    label: &str,
    src: &str,
    file: &str,
    (from, to): (&str, &str),
    needle: &str,
    scan: impl Fn(&str) -> Vec<String>,
) {
    assert!(
        src.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    let injected = src.replacen(from, to, 1);
    let (after, needle) = needle.split_once(">>").unwrap_or(("", needle));
    let lines: Vec<&str> = injected.lines().collect();
    let begin = if after.is_empty() {
        0
    } else {
        lines
            .iter()
            .position(|l| l.contains(after))
            .unwrap_or_else(|| panic!("{label}: 起点 {after:?} が無い"))
    };
    let line = lines[begin..]
        .iter()
        .position(|l| l.contains(needle))
        .map(|i| begin + i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&injected);
    let expected = format!("{file}:{line} ");
    assert!(
        reports.iter().any(|r| r.starts_with(&expected)),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let (text_edit, manager, gui) = current();
    let te = |t: &str| all(t, &manager, &gui);
    let mg = |m: &str| all(&text_edit, m, &gui);
    let gu = |g: &str| all(&text_edit, &manager, g);

    // A. 整形を 1 か所へまとめて当てる（#1869 前の当て方を旧腕の外で使う）
    assert_named(
        "A 1 か所へまとめる",
        &text_edit,
        TEXT_EDIT,
        (
            "            splices_of(&ordered)\n        };",
            "            let c = compose_changes(&self.text, &ordered);\n            vec![Splice::new(c.0.clone(), &c.1)]\n        };",
        ),
        "pub fn apply_changes(>>let c = compose_changes(",
        te,
    );
    // A2. 補完の確定を 1 か所ずつ apply_edit（つなぐ形へ戻す）
    assert_named(
        "A2 箇所ごとに積む",
        &text_edit,
        TEXT_EDIT,
        (
            "        self.apply_edit(Edit {\n            splices: &splices_of(&ordered),",
            "        for _change in &ordered {}\n        for _change in &ordered {\n        self.apply_edit(Edit {\n            splices: &splices_of(&ordered),",
        ),
        "pub fn replace_position_ranges(>>self.apply_edit(Edit {",
        te,
    );
    // B. 離れた箇所を 1 か所ずつ置き換える
    assert_named(
        "B 1 か所ずつ詰め直す",
        &text_edit,
        TEXT_EDIT,
        (
            "                for splice in many {\n                    text.push_str(",
            "                for splice in many {\n                    self.text.replace_range(splice.range.clone(), splice.text);\n                    text.push_str(",
        ),
        "fn splice_text(>>self.text.replace_range(splice.range.clone()",
        te,
    );
    // C. undo がつないだ差分を while で戻す
    assert_named(
        "C undo を繰り返す",
        &text_edit,
        TEXT_EDIT,
        (
            "        let Some(delta) = self.undo_stack.pop_back() else {\n            return false;\n        };",
            "        while let Some(delta) = self.undo_stack.pop_back() {\n            break;\n        }\n        let Some(delta) = self.undo_stack.pop_back() else {\n            return false;\n        };",
        ),
        "    pub fn undo(&mut self)>>while let Some(delta)",
        te,
    );
    // D. 差分が「つないで」持つ形へ戻る
    assert_named(
        "D つないだ差分",
        &text_edit,
        TEXT_EDIT,
        (
            "    at_millis: u64,\n}\n\n/// 差分 1 件の中の 1 か所",
            "    at_millis: u64,\n    chained: bool,\n}\n\n/// 差分 1 件の中の 1 か所",
        ),
        "struct EditDelta {>>chained: bool,",
        te,
    );
    // E. 打鍵の要求は読み込みを待たない（#1869 前）
    assert_named(
        "E 打鍵は待たない",
        &manager,
        MANAGER,
        (
            "            if parsed.items.is_empty() && wait {",
            "            if parsed.items.is_empty() && !request.superseding {",
        ),
        "fn completion(&self, request: &CompletionRequest)>>if parsed.items.is_empty() && !request.superseding {",
        mg,
    );
    // F. 読み込みの待ちを打ち切らない
    assert_named(
        "F 待ちが抜けない",
        &manager,
        MANAGER,
        (
            "self.wait_after_empty(&key, deadline, &abandoned, &mut empty)",
            "self.wait_after_empty(&key, deadline, &|| false, &mut empty)",
        ),
        "fn completion(&self, request: &CompletionRequest)>>self.wait_after_empty(",
        mg,
    );
    // G. GUI が「読み込み中」を立てない
    assert_named(
        "G 読み込み中を立てない",
        &gui,
        GUI,
        (
            "            && self.lsp.server_loading(edit.buffer.path());",
            "            && false;",
        ),
        "fn fire_completion(",
        gu,
    );
    // H. 「読み込み中」を一覧が出ている扱いにする（5 キーを奪う）
    assert_named(
        "H キーを奪う",
        &gui,
        GUI,
        (
            "            .is_some_and(|p| p.pane == pane)\n    }",
            "            .is_some_and(|p| p.pane == pane)\n            || self.lsp_completion.loading.is_some()\n    }",
        ),
        "pub(crate) fn lsp_completion_open_in(>>self.lsp_completion.loading.is_some()",
        gu,
    );
}

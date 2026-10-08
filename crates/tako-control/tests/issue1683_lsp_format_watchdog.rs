//! 整形（FR-3.33 / #1683）の番犬
//!
//! # なぜ要るか
//!
//! 整形の契約は**1 行で壊せる**: `apply_changes` が書き換えを 1 つずつ `apply_edit` へ
//! 流せば undo が N 回になる（#1651 の塊が崩れる）、並べ替えを消すか逆にすれば後ろの書き換えの
//! 位置がずれる、自動保存の経路から整形を呼べば「打鍵の 500ms 後に勝手に整形される」
//! （既定 OFF にしたユーザー確定の理由そのもの）、設定の既定を `default_true` にすれば
//! 全員の保存が整形付きになる。どれも CLI の応答の形は変わらない。挙動は単体（text_edit /
//! lsp::format / dispatch）と e2e（`issue1683_lsp_format`）が測り、ここは**構造**を縛って
//! file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. `TextBuffer::apply_changes` は `apply_edit` を**ちょうど 1 回**、繰り返しの外で呼ぶ
//!    （undo 1 回で戻る）。種類は `EditKind::Replace`（前後の打鍵とまとまらない）
//! 2. `order_changes` は**安定ソート**（`sort_by_key`）で始まり → 終わりに並べ、重なりを
//!    見てから最小化する（並べ直した後に削る = 挿入どうしの順が入れ替わらない）
//! 3. 自動保存（`run_autosave` / `drive_autosave`）は整形を通らない
//! 4. 保存時整形の設定は既定 OFF（`#[serde(default)]` と `Default` の `false`）
//! 5. GUI は答えを自前で当てない（`apply_changes(` を呼ぶのは host の `apply_preview_changes`
//!    だけ）。入口は CLI / MCP と同じ `prepare_offload`
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
const SETTINGS: &str = "crates/tako-control/src/settings.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const GUI: &str = "crates/tako-app/src/lsp_format_ui.rs";

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

/// 窓の中で `needle` を含む最初のコード行（1-based。無ければ `None`）
fn line_in(at: usize, window: &str, needle: &str) -> Option<usize> {
    code_lines(window)
        .iter()
        .position(|l| l.contains(needle))
        .map(|i| at + i)
}

/// `lines` の最後の行の後ろで、まだ閉じていない繰り返しがあればその綴り。
/// 波括弧の深さで数える（閉じた繰り返しの後ろで呼ぶのは 1 回 = 名指さない）
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

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line} — {why}")
}

/// 1・2（text_edit）の検査
fn scan_text_edit(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    match fn_window(src, "pub fn apply_changes(") {
        None => out.push(report(
            TEXT_EDIT,
            0,
            "`apply_changes` が見つからない（走査が空振り）",
        )),
        Some((at, window)) => {
            let code = code_lines(&window);
            let calls: Vec<usize> = code
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains("self.apply_edit("))
                .map(|(i, _)| at + i)
                .collect();
            match calls.as_slice() {
                [] => out.push(report(
                    TEXT_EDIT,
                    at,
                    "書き換えを `apply_edit` へ通していない（履歴に載らない = undo で戻らない）",
                )),
                [one] => {
                    if let Some(looping) = open_loop_at(&code[..one - at]) {
                        out.push(report(
                            TEXT_EDIT,
                            *one,
                            &format!(
                                "`apply_edit` を繰り返し（`{looping}`）の中で呼んでいる = 書き換え 1 つごとに \
                                 undo が 1 件積まれる（undo 1 回で整形の前へ戻らない）"
                            ),
                        ));
                    }
                }
                many => {
                    for line in many {
                        out.push(report(
                            TEXT_EDIT,
                            *line,
                            "`apply_edit` を 2 回以上呼んでいる（undo 1 回で整形の前へ戻らない）",
                        ));
                    }
                }
            }
            if !code.iter().any(|l| l.contains("kind: EditKind::Replace")) {
                out.push(report(
                    TEXT_EDIT,
                    at,
                    "種類が Replace ではない（前後の打鍵の塊とまとまり、undo が打鍵ごと戻す）",
                ));
            }
        }
    }
    match fn_window(src, "pub fn order_changes(") {
        None => out.push(report(
            TEXT_EDIT,
            0,
            "`order_changes` が見つからない（走査が空振り）",
        )),
        Some((at, window)) => {
            let sort = line_in(
                at,
                &window,
                ".sort_by_key(|(_, change)| (change.range.start, change.range.end))",
            );
            let overlap = line_in(at, &window, "ChangesError::Overlap");
            let trim = line_in(at, &window, "changed_span(");
            match (sort, overlap, trim) {
                (None, ..) => out.push(report(
                    TEXT_EDIT,
                    line_in(at, &window, "sort").unwrap_or(at),
                    "始まり → 終わりの安定ソート（`sort_by_key`）で並べていない（逆順・同じ位置の挿入の順が崩れる）",
                )),
                (Some(sort), Some(overlap), Some(trim)) if !(sort < overlap && overlap < trim) => {
                    out.push(report(
                        TEXT_EDIT,
                        trim,
                        "並べる → 重なりを見る → 削る の順ではない（削ってから並べると挿入どうしの順が入れ替わる）",
                    ))
                }
                (Some(_), None, _) => out.push(report(TEXT_EDIT, at, "重なりを拒否していない")),
                _ => {}
            }
        }
    }
    out
}

/// 3・5（GUI）の検査
fn scan_gui(main: &str, gui: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["fn run_autosave(", "fn drive_autosave("] {
        match fn_window(main, name) {
            None => out.push(report(
                MAIN,
                0,
                &format!("`{name}` が見つからない（走査が空振り）"),
            )),
            Some((at, window)) => {
                for bad in [
                    "save_with_format",
                    "format_focused_preview",
                    "LspFormat",
                    "prepare_offload(",
                ] {
                    if let Some(line) = line_in(at, &window, bad) {
                        out.push(report(
                            MAIN,
                            line,
                            &format!(
                                "自動保存が整形を通っている（`{bad}`）= 打鍵の 500ms 後に勝手に整形される"
                            ),
                        ));
                    }
                }
            }
        }
    }
    // 答えを当てる口は host の 1 つだけ
    let allowed = fn_window(main, "fn apply_preview_changes(");
    if allowed.is_none() {
        out.push(report(
            MAIN,
            0,
            "`apply_preview_changes` が見つからない（走査が空振り）",
        ));
    }
    for (index, line) in main.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if !code.contains(".apply_changes(") {
            continue;
        }
        let inside = allowed.as_ref().is_some_and(|(at, window)| {
            let end = at + window.lines().count();
            (*at..end).contains(&(index + 1))
        });
        if !inside {
            out.push(report(
                MAIN,
                index + 1,
                "GUI が答えを自前で当てている（`apply_changes`）。dispatch の 1 本（finish_offload → apply_preview_changes）を通す",
            ));
        }
    }
    for (index, line) in gui.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if code.contains(".apply_changes(") || code.contains("apply_preview_changes(") {
            out.push(report(
                GUI,
                index + 1,
                "GUI が答えを自前で当てている。dispatch の 1 本（finish_offload）を通す",
            ));
        }
    }
    // 編集メニュー・⇧⌘I（`format_focused_preview`）と右クリックメニューの整形（#1684）は
    // 共有の入口 `format_preview_request` を通り、そこが CLI / MCP と同じ準備を通る
    match fn_window(gui, "pub(crate) fn format_focused_preview(") {
        None => out.push(report(
            GUI,
            0,
            "`pub(crate) fn format_focused_preview(` が見つからない（走査が空振り）",
        )),
        Some((at, window)) => {
            if line_in(
                at,
                &window,
                "self.format_preview_request(pane, request, cx)",
            )
            .is_none()
            {
                out.push(report(
                    GUI,
                    at,
                    "編集メニューの整形が共有の入口（format_preview_request）を通っていない",
                ));
            }
        }
    }
    for name in [
        "pub(crate) fn format_preview_request(",
        "pub(crate) fn save_with_format(",
    ] {
        match fn_window(gui, name) {
            None => out.push(report(
                GUI,
                0,
                &format!("`{name}` が見つからない（走査が空振り）"),
            )),
            Some((at, window)) => {
                if line_in(at, &window, "tako_control::prepare_offload(self, &request)").is_none() {
                    out.push(report(
                        GUI,
                        at,
                        "GUI の入口が CLI / MCP と同じ準備（prepare_offload）を通っていない",
                    ));
                }
            }
        }
    }
    out
}

/// 4（設定の既定）の検査
fn scan_settings(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    match lines
        .iter()
        .position(|l| l.trim() == "pub lsp_format_on_save: bool,")
    {
        None => out.push(report(
            SETTINGS,
            0,
            "`lsp_format_on_save` が見つからない（走査が空振り）",
        )),
        Some(i) => {
            let attribute = lines[..i]
                .iter()
                .rev()
                .find(|l| l.trim_start().starts_with("#[serde("))
                .map(|l| l.trim());
            if attribute != Some("#[serde(default)]") {
                out.push(report(
                    SETTINGS,
                    i + 1,
                    "保存時整形の既定が OFF ではない（`#[serde(default)]` = 旧ファイルも false で読む）",
                ));
            }
        }
    }
    match lines
        .iter()
        .position(|l| l.trim().starts_with("lsp_format_on_save:"))
    {
        None => out.push(report(
            SETTINGS,
            0,
            "`Default` の `lsp_format_on_save` が見つからない",
        )),
        Some(i) if lines[i].trim() != "lsp_format_on_save: false," => out.push(report(
            SETTINGS,
            i + 1,
            "`Settings::default()` の保存時整形が false ではない",
        )),
        _ => {}
    }
    out
}

fn current() -> (String, String, String, String) {
    (read(TEXT_EDIT), read(SETTINGS), read(MAIN), read(GUI))
}

fn all(text_edit: &str, settings: &str, main: &str, gui: &str) -> Vec<String> {
    let mut out = scan_text_edit(text_edit);
    out.extend(scan_settings(settings));
    out.extend(scan_gui(main, gui));
    out
}

#[test]
fn 整形の構造が契約どおり() {
    let (text_edit, settings, main, gui) = current();
    let reports = all(&text_edit, &settings, &main, &gui);
    assert!(
        reports.is_empty(),
        "整形の契約が崩れた:\n{}",
        reports.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let (text_edit, settings, main, gui) = current();
    assert!(fn_window(&text_edit, "pub fn apply_changes(").is_some());
    assert!(fn_window(&text_edit, "pub fn order_changes(").is_some());
    assert!(fn_window(&main, "fn run_autosave(").is_some());
    assert!(fn_window(&main, "fn drive_autosave(").is_some());
    assert!(fn_window(&main, "fn apply_preview_changes(").is_some());
    assert!(fn_window(&gui, "pub(crate) fn format_focused_preview(").is_some());
    assert!(fn_window(&gui, "pub(crate) fn format_preview_request(").is_some());
    assert!(fn_window(&gui, "pub(crate) fn save_with_format(").is_some());
    assert!(settings.contains("pub lsp_format_on_save: bool,"));
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
    let (text_edit, settings, main, gui) = current();
    let te = |t: &str| all(t, &settings, &main, &gui);
    let st = |s: &str| all(&text_edit, s, &main, &gui);
    let mn = |m: &str| all(&text_edit, &settings, m, &gui);
    let gu = |g: &str| all(&text_edit, &settings, &main, g);

    // A. 書き換え 1 つごとに apply_edit（undo が N 回になる）
    assert_named(
        "A 1 つずつ当てる",
        &text_edit,
        TEXT_EDIT,
        (
            "follow_changes(&ordered, anchor));\n        self.apply_edit(Edit {",
            "follow_changes(&ordered, anchor));\n        for _change in &ordered {\n        self.apply_edit(Edit {",
        ),
        "pub fn apply_changes(>>self.apply_edit(",
        te,
    );
    // A2. 2 回に分けて当てる
    assert_named(
        "A2 2 回当てる",
        &text_edit,
        TEXT_EDIT,
        (
            "follow_changes(&ordered, anchor));\n        self.apply_edit(Edit {",
            "follow_changes(&ordered, anchor));\n        self.apply_edit(Edit {\n            range: 0..0,\n            replacement: \"\",\n            cursor,\n            anchor,\n            kind: EditKind::Replace,\n            line_ending: None,\n        });\n        self.apply_edit(Edit {",
        ),
        "pub fn apply_changes(>>self.apply_edit(",
        te,
    );
    // B. 並べない（答えの順のまま当てる = 逆順の答えで位置がずれる）
    assert_named(
        "B 並べない",
        &text_edit,
        TEXT_EDIT,
        (
            "    indexed.sort_by_key(|(_, change)| (change.range.start, change.range.end));",
            "    indexed.reverse();",
        ),
        "pub fn order_changes(",
        te,
    );
    // C. 種類が打鍵（Insert）= 前の打鍵の塊にまとまる
    assert_named(
        "C 打鍵の塊にまとまる",
        &text_edit,
        TEXT_EDIT,
        (
            "            anchor,\n            kind: EditKind::Replace,\n            line_ending: None,\n        });\n        Ok(AppliedChanges {",
            "            anchor,\n            kind: EditKind::Insert,\n            line_ending: None,\n        });\n        Ok(AppliedChanges {",
        ),
        "pub fn apply_changes(",
        te,
    );
    // D. 自動保存が整形を通る
    assert_named(
        "D 自動保存で整形",
        &main,
        MAIN,
        (
            "        match self.save_preview_local(pane_id, false) {\n            Ok(()) => {\n                if let Some(edit) = self.preview_edits.get_mut(&pane_id) {\n                    edit.save_status = Some(preview::SaveStatus::Saved);",
            "        let _ = self.save_with_format(pane_id, cx);\n        match self.save_preview_local(pane_id, false) {\n            Ok(()) => {\n                if let Some(edit) = self.preview_edits.get_mut(&pane_id) {\n                    edit.save_status = Some(preview::SaveStatus::Saved);",
        ),
        "fn run_autosave(>>save_with_format",
        mn,
    );
    // E. 既定 ON
    assert_named(
        "E 既定 ON",
        &settings,
        SETTINGS,
        (
            "    #[serde(default)]\n    pub lsp_format_on_save: bool,",
            "    #[serde(default = \"default_true\")]\n    pub lsp_format_on_save: bool,",
        ),
        "pub lsp_format_on_save: bool,",
        st,
    );
    // F. GUI が答えを自前で当てる
    assert_named(
        "F GUI が自前で当てる",
        &gui,
        GUI,
        (
            "        if seq == self.lsp_format.seq {",
            "        let _ = |b: &mut tako_core::text_edit::TextBuffer| b.apply_changes(Vec::new(), None);\n        if seq == self.lsp_format.seq {",
        ),
        "b.apply_changes(",
        gu,
    );
}

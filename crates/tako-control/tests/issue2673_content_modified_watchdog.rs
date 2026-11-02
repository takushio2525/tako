//! `-32801`（ContentModified = 変更で古くなった答え）の扱いの構造の番犬（#2673）
//!
//! # なぜ要るか
//!
//! rust-analyzer は読み込み直しの最中に受けた要求を `-32801` で取り消す（実測。新しいプロジェクトを
//! 開いた直後の右クリック →「定義へ移動」が 2/2 で飛ばなかった）。tako はエラー応答を `rpc_failure` で
//! 一律に最終の失敗へ落としていたので、**どの問い合わせも 1 行で元へ戻せる**（判定を呼ばずに
//! `?` で返す・補完で問い直す）。戻しても偽サーバの既定のシナリオは `-32801` を返さないので、
//! 既存の e2e は緑のまま。
//!
//! 挙動は e2e（`issue2673_lsp_content_modified`。6 種の修正前と修正後）と単体（`lsp::goto` の
//! `ContentModified`）が測り、ここは**構造**を縛って file:line で名指す:
//!
//! - 定義ジャンプ・参照と階層（展開を含む）・ホバー・名前の変更・整形の問い合わせは、どれも
//!   `after_content_modified`（判定 `ContentModified::step` と読み込みの待ちの 1 実装）を通る
//! - 補完は `-32801` を見分けて捨て（`discarded`）、問い直さない（`after_content_modified` を呼ばない）
//! - `after_content_modified` は A/B（`legacy_2673`）・判定（`.step(`）・待ち（`wait_loaded`）を持つ
//! - 判定は回数の上限と「すぐの問い直しは 1 回」の印を見る
//! - 偽サーバの `--content-modified` は 6 種の要求すべてに効く
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const GOTO_RS: &str = "crates/tako-control/src/lsp/goto.rs";
const FAKE: &str = "crates/tako-control/src/bin/tako-lsp-fake.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 問い直す側の問い合わせ（manager の `Shared` の関数の頭。公開の `LspManager` の口は `pub fn` なので当たらない）
const REQUERYING: [(&str, &str); 6] = [
    ("    fn goto(&self, request: &GotoRequest)", "定義ジャンプ"),
    ("    fn xref(&self, request: &XrefRequest)", "参照と階層"),
    (
        "    fn xref_expand(&self, request: &ExpandRequest)",
        "階層の展開",
    ),
    ("    fn hover(&self, request: &HoverRequest)", "ホバー"),
    (
        "    fn rename(&self, request: &RenameRequest)",
        "名前の変更",
    ),
    ("    fn format(&self, request: &FormatRequest)", "整形"),
];

const COMPLETION: &str = "    fn completion(&self, request: &CompletionRequest)";
const AFTER: &str = "    fn after_content_modified(";
const STEP: &str = "    pub fn step(&mut self, loading: bool)";
const FAKE_FEATURES: &str = "fn answers_language_feature(";

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

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line} — {why}")
}

/// 窓を採る（無ければ空振りとして名指す）
fn window_or(
    out: &mut Vec<String>,
    file: &str,
    src: &str,
    needle: &str,
) -> Option<(usize, String)> {
    let found = fn_window(src, needle);
    if found.is_none() {
        out.push(report(
            file,
            0,
            &format!("`{}` が見つからない（走査が空振り）", needle.trim()),
        ));
    }
    found
}

fn scan_manager(src: &str) -> Vec<String> {
    let view = code_view::without_comments_checked(src, MANAGER);
    let mut out = Vec::new();
    for (needle, label) in REQUERYING {
        let Some((at, body)) = window_or(&mut out, MANAGER, &view, needle) else {
            continue;
        };
        if !body.contains("self.after_content_modified(") {
            out.push(report(
                MANAGER,
                at,
                &format!(
                    "{label} が `-32801`（ContentModified）を `after_content_modified` へ通していない = \
                     読み込み直しの最中に取り消された要求を最終の失敗にする（#2673）"
                ),
            ));
        }
    }
    if let Some((at, body)) = window_or(&mut out, MANAGER, &view, COMPLETION) {
        if !(body.contains("is_content_modified(") && body.contains("discarded")) {
            out.push(report(
                MANAGER,
                at,
                "補完が `-32801` を見分けて捨てていない（`server-error` で返す = #2673）",
            ));
        }
        if body.contains("after_content_modified(") {
            out.push(report(
                MANAGER,
                at,
                "補完が `-32801` の後に問い直している（次の打鍵で置き換わるので問い直さない = #2673）",
            ));
        }
    }
    if let Some((at, body)) = window_or(&mut out, MANAGER, &view, AFTER) {
        for (part, why) in [
            (
                "legacy_2673()",
                "A/B の旧腕（`TAKO_2673_LEGACY=1`）を見ていない",
            ),
            (".step(", "判定 `ContentModified::step` を通していない"),
            (
                "self.wait_loaded(",
                "読み込み中の `-32801` の後に読み込みの完了を待っていない",
            ),
        ] {
            if !body.contains(part) {
                out.push(report(
                    MANAGER,
                    at,
                    &format!("`after_content_modified` が {why}（#2673）"),
                ));
            }
        }
    }
    out
}

fn scan_goto(src: &str) -> Vec<String> {
    let view = code_view::without_comments_checked(src, GOTO_RS);
    let mut out = Vec::new();
    if let Some((at, body)) = window_or(&mut out, GOTO_RS, &view, STEP) {
        for (part, why) in [
            (
                "MAX_CONTENT_MODIFIED_RETRIES",
                "問い直しの回数の上限を見ていない（無限に繰り返す）",
            ),
            ("retried_now", "すぐの問い直しを 1 回に限っていない"),
        ] {
            if !body.contains(part) {
                out.push(report(
                    GOTO_RS,
                    at,
                    &format!("`ContentModified::step` が {why}（#2673）"),
                ));
            }
        }
    }
    out
}

fn scan_fake(src: &str) -> Vec<String> {
    let view = code_view::without_comments_checked(src, FAKE);
    let mut out = Vec::new();
    if let Some((at, body)) = window_or(&mut out, FAKE, &view, FAKE_FEATURES) {
        for part in [
            "GOTO_METHODS",
            "XREF_METHODS",
            "RENAME_METHODS",
            "FORMAT_METHODS",
            "textDocument/completion",
            "textDocument/hover",
        ] {
            if !body.contains(part) {
                out.push(report(
                    FAKE,
                    at,
                    &format!("偽サーバの `--content-modified` が `{part}` に効かない（e2e が 6 種を測れない = #2673）"),
                ));
            }
        }
    }
    if !view.contains("answers_language_feature(method)") {
        out.push(report(
            FAKE,
            0,
            "偽サーバが要求ごとに `answers_language_feature` を引いていない（`--content-modified` が効かない = #2673）",
        ));
    }
    out
}

fn scan_all(manager: &str, goto: &str, fake: &str) -> Vec<String> {
    let mut out = scan_manager(manager);
    out.extend(scan_goto(goto));
    out.extend(scan_fake(fake));
    out
}

#[test]
fn content_modified_は_6_種とも判定を通る() {
    let found = scan_all(&read(MANAGER), &read(GOTO_RS), &read(FAKE));
    assert!(
        found.is_empty(),
        "#2673 の構造が崩れている:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let manager = read(MANAGER);
    let view = code_view::without_comments_checked(&manager, MANAGER);
    for needle in REQUERYING
        .iter()
        .map(|(n, _)| *n)
        .chain([COMPLETION, AFTER])
    {
        let (_, body) =
            fn_window(&view, needle).unwrap_or_else(|| panic!("`{needle}` の窓が採れる"));
        // 窓が関数の途中で切れていない（本文が数行以上ある）
        assert!(body.lines().count() > 10, "`{needle}` の窓が短すぎる");
    }
    let goto = read(GOTO_RS);
    assert!(fn_window(&code_view::without_comments_checked(&goto, GOTO_RS), STEP).is_some());
    assert!(fn_window(&read(FAKE), FAKE_FEATURES).is_some());
}

/// `src` の `needle` の関数の窓の中だけで `from` を `to` へ置き換える（現行ソースから注入を作る）
fn inject(src: &str, needle: &str, from: &str, to: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` が現行ソースに在る"));
    let (_, window) = fn_window(src, needle).unwrap();
    assert!(
        window.contains(from),
        "`{needle}` の窓に `{from}` が在る（注入が空振りしない）"
    );
    let replaced = window.replacen(from, to, 1);
    let before = lines[..start].join("\n");
    let after_start = start + window.lines().count();
    let after = lines[after_start..].join("\n");
    format!("{before}\n{replaced}\n{after}\n")
}

#[track_caller]
fn assert_named(label: &str, found: Vec<String>, file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入 {label} が名指されない（{file} / {why}）: {found:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let manager = read(MANAGER);
    let goto = read(GOTO_RS);
    let fake = read(FAKE);
    // A. 6 種のそれぞれが判定を通らない（`?` で返す形へ戻す）
    for (needle, label) in REQUERYING {
        let injected = inject(
            &manager,
            needle,
            "self.after_content_modified(",
            "self.wait_after_empty_disabled(",
        );
        assert_named(
            &format!("A {label}"),
            scan_all(&injected, &goto, &fake),
            MANAGER,
            &format!("{label} が `-32801`"),
        );
    }
    // B. 補完が `-32801` を見分けない（`server-error` へ戻す）
    let injected = inject(&manager, COMPLETION, "discarded = true;", "");
    let injected = inject(&injected, COMPLETION, "let mut discarded = false;", "");
    let injected = inject(&injected, COMPLETION, "&& !discarded", "");
    assert_named(
        "B 補完が見分けない",
        scan_all(&injected, &goto, &fake),
        MANAGER,
        "補完が `-32801` を見分けて捨てていない",
    );
    // C. 補完が問い直す
    let injected = inject(
        &manager,
        COMPLETION,
        "let wait = ",
        "let _ = self.after_content_modified(&key, \"x\", deadline, &abandoned, &mut Default::default());\n            let wait = ",
    );
    assert_named(
        "C 補完が問い直す",
        scan_all(&injected, &goto, &fake),
        MANAGER,
        "補完が `-32801` の後に問い直している",
    );
    // D. 待ちの 1 実装が A/B を見ない・読み込みを待たない
    let injected = inject(&manager, AFTER, "super::goto::legacy_2673()", "false");
    assert_named(
        "D A/B を見ない",
        scan_all(&injected, &goto, &fake),
        MANAGER,
        "A/B の旧腕",
    );
    let injected = inject(
        &manager,
        AFTER,
        "self.wait_loaded(key, deadline, abandoned)",
        "Loading::Retry",
    );
    assert_named(
        "D 読み込みを待たない",
        scan_all(&injected, &goto, &fake),
        MANAGER,
        "読み込みの完了を待っていない",
    );
    // E. 判定が上限を見ない
    let injected = inject(
        &goto,
        STEP,
        "self.retries >= MAX_CONTENT_MODIFIED_RETRIES || ",
        "",
    );
    assert_named(
        "E 上限を見ない",
        scan_all(&manager, &injected, &fake),
        GOTO_RS,
        "回数の上限",
    );
    // F. 偽サーバの規則が整形に効かない
    let injected = inject(
        &fake,
        FAKE_FEATURES,
        "|| FORMAT_METHODS.contains(&method)",
        "",
    );
    assert_named(
        "F 偽サーバが整形に効かない",
        scan_all(&manager, &goto, &injected),
        FAKE,
        "FORMAT_METHODS",
    );
}

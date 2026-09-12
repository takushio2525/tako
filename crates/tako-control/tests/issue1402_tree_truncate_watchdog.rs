//! ファイルツリーが「切り詰めた / 読めなかった」を黙って捨てていないかの番犬（#1402）
//!
//! # なぜ要るか
//!
//! `crates/tako-app/src/filetree.rs` の `read_dir_sorted` は 1 ディレクトリ
//! 500 件（`MAX_ENTRIES`）で `entries.truncate(..)` し、**切り詰めた事実を
//! `Entry` にも `Row` にも残していなかった**。並びはディレクトリ先なので、
//! サブディレクトリが 500 個あるディレクトリでは**ファイルが 1 行も出ない**。
//! ユーザーからは「そのファイルが存在しない」ように見える（実測: 560 件 →
//! 行 501 / note 0）。同じ関数は `read_dir` の `Err` も `Vec::new()` へ落として
//! いたので、権限の無いフォルダと空のフォルダが同じ見え方だった。
//!
//! 一方で機械可読側（`tako tree git-status`）は `limit` と `truncated` を返す。
//! **CLI / MCP は申告するのに画面だけ黙る**という非対称が本体で、片側だけ直すと
//! すぐ戻る（`truncate` の 1 行・`Vec::new()` の 1 行で再発する）。
//!
//! # 何を縛るか
//!
//! 1. `read_dir_sorted` は切り詰めの事実（`Truncation`）と読み取り失敗の理由を
//!    返す（戻り値を `Vec<Entry>` へ戻すと描画側は永久に知れない）
//! 2. `collect_rows` はそれを**行として見せる**（`local_note_of` → `local_note_row`）
//! 3. 画面の判断は CLI / MCP と**同じ 1 実装**（`tako_core::sidebar::Truncation`）から
//!    出る。`tree_git_status_payload` が `total > limit` を手書きへ戻したら落ちる
//! 4. 情報行の描画（`sidebar::render_note_row`）は `RowNote` の**全種別**に対する
//!    アームを持ち、`_ =>` で受けない（新しい種別が無言の空行になるのを防ぐ）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜4 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓と `RowNote` の種別が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**修正前ソースを再現した 6 通りの注入**が
//! file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const FILETREE: &str = "crates/tako-app/src/filetree.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug, PartialEq, Eq)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
///
/// 終わりは**宣言行と同じ字下げの `}`**（自由関数 = 0 桁 / メソッド = 4 桁）。
/// 「宣言行から N 行」で切ると隣の関数の実装を自分のものと数えてしまう
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

/// 窓の中で `needle` を含む最初の行の 1-based 行番号
fn line_of(window_start: usize, window: &str, needle: &str) -> usize {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| window_start + i)
        .unwrap_or(window_start)
}

/// 注釈（`//` で始まる行）を落とした窓。
///
/// 検査は**コードだけ**を見る。この規約は「なぜそう書くか」を注釈で残す前提なので
/// （`conventions.md`）、アンチパターンを説明した注釈で落ちる形にすると
/// **理由を書くほど落ちる**ことになる
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// `pub enum RowNote { .. }` の種別名（描画側の網羅を検査するための一覧）
fn row_note_variants(src: &str) -> Vec<String> {
    let Some((_, window)) = fn_window(src, "pub enum RowNote {") else {
        return Vec::new();
    };
    window
        .lines()
        .skip(1)
        .filter_map(|l| {
            let t = l.trim();
            if !t.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            let name: String = t.chars().take_while(|c| c.is_alphanumeric()).collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// ツリー側（`filetree.rs`）の検査
fn scan_filetree(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: FILETREE,
            line,
            why,
        })
    };

    // 1. read_dir_sorted は「切り詰めた事実」と「読めなかった理由」を返す
    match fn_window(src, "fn read_dir_sorted(") {
        None => push(0, "`read_dir_sorted` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if let Some(i) = code_line_with(&window, "truncate(MAX_ENTRIES)") {
                push(
                    at + i,
                    "上限で切ったあと**総数を捨てている**（`truncate(MAX_ENTRIES)` で切ると \
                     切り詰めた事実が呼び出し側へ届かず、描画は行に出せない。#1402）"
                        .into(),
                );
            }
            if !code.contains("Truncation::new(") {
                push(
                    at,
                    "切り詰めの判断が 1 実装（`tako_core::sidebar::Truncation`）を \
                     通っていない（CLI の `tree git-status` と答えが割れる）"
                        .into(),
                );
            }
            if let Some(i) = code_line_with(&window, "return Vec::new()") {
                push(
                    at + i,
                    "`read_dir` の失敗を空の一覧へ落としている（権限なしと空が \
                     区別できない。#1402 の受け入れ条件 2）"
                        .into(),
                );
            }
            if !code.contains("error: Some(") {
                push(
                    at,
                    "読めなかった理由を返していない（`DirListing::error` が埋まらない）".into(),
                );
            }
        }
    }

    // 2. 切り詰め・読み取り失敗を**行として見せる**
    match fn_window(src, "fn collect_rows(") {
        None => push(0, "`collect_rows` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("local_note_of(") || !code.contains("local_note_row(") {
                push(
                    at,
                    "切り詰め / 読み取り失敗を行にしていない（`local_note_of` → \
                     `local_note_row` へ繋ぐ。`conventions.md`「弾いたら黙って捨てない」）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "fn local_note_of(") {
        None => push(
            0,
            "`local_note_of` が無い（情報行を出す判断の 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("truncation.truncated()") {
                push(
                    at,
                    "切り詰めを申告していない（`Truncation::truncated()` を見ていない）".into(),
                );
            }
            if !code.contains("RowNote::Truncated") {
                push(
                    at,
                    "切り詰めの行（`RowNote::Truncated`）を返していない".into(),
                );
            }
            if !code.contains("RowNote::Error") {
                push(
                    at,
                    "読めなかった理由の行（`RowNote::Error`）を返していない".into(),
                );
            }
        }
    }
    out
}

/// 描画側（`sidebar.rs` の `render_note_row`）の検査。
///
/// #1402 は「行を作った」だけでは終わらない: 描画が種別を取りこぼすと**空行**になり、
/// 黙って捨てるのと同じ見え方に戻る
fn scan_sidebar(src: &str, variants: &[String]) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: SIDEBAR,
            line,
            why,
        })
    };
    // ローカル行が情報行の描画へ分岐している（#1398 で置いた 1 実装を再利用する）
    if !src.contains("if row.note.is_some() {") || !src.contains("render_note_row(index, &row,") {
        push(
            0,
            "ローカル行が情報行の描画（`render_note_row`）へ分岐していない \
             （押せる行として描くと切り詰めの行が名前なしの空行に見える）"
                .into(),
        );
    }
    match fn_window(src, "fn render_note_row(") {
        None => push(0, "`render_note_row` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            for v in variants {
                if !code.contains(&format!("RowNote::{v}")) {
                    push(
                        at,
                        format!(
                            "`RowNote::{v}` のアームが無い（種別を足したのに描画が \
                             追いついていない = その行は無言の空行になる）"
                        ),
                    );
                }
            }
            if let Some(i) = window
                .lines()
                .position(|l| l.trim_start().starts_with("_ =>"))
            {
                push(
                    at + i,
                    "情報行の描画を `_ =>` で受けている（新しい種別が黙って \
                     同じ見え方になり、#1402 の再発を検出できない）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 機械可読側（`dispatch.rs` の `tree_git_status_payload`）の検査。
///
/// #1402 の本体は「画面と CLI の非対称」なので、CLI 側が判断を**手書きへ戻す**と
/// 同じバグが逆向き（画面は申告・CLI は黙る）で再発する
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn tree_git_status_payload(") {
        None => vec![Offender {
            file: DISPATCH,
            line: 0,
            why: "`tree_git_status_payload` が見つからない（走査が空振り）".into(),
        }],
        Some((at, window)) => {
            let code = code_only(&window);
            if let Some(i) = code_line_with(&window, "total > limit") {
                out.push(Offender {
                    file: DISPATCH,
                    line: at + i,
                    why: "切り詰めの判断を手書きしている（画面と同じ 1 実装 \
                          `tako_core::sidebar::Truncation` を通す。#1402）"
                        .into(),
                });
            }
            if !code.contains("Truncation::new(") {
                out.push(Offender {
                    file: DISPATCH,
                    line: at,
                    why: "`Truncation` を通っていない（画面と答えが割れる）".into(),
                });
            }
            if !code.contains("cut.truncated()") {
                out.push(Offender {
                    file: DISPATCH,
                    line: line_of(at, &window, "\"truncated\""),
                    why: "応答の `truncated` が 1 実装の判断から出ていない".into(),
                });
            }
            out
        }
    }
}

fn all(src_filetree: &str, src_sidebar: &str, src_dispatch: &str) -> Vec<String> {
    let variants = row_note_variants(src_filetree);
    scan_filetree(src_filetree)
        .iter()
        .chain(scan_sidebar(src_sidebar, &variants).iter())
        .chain(scan_dispatch(src_dispatch).iter())
        .map(Offender::report)
        .collect()
}

fn sources(root: &Path) -> (String, String, String) {
    (
        read(root, FILETREE),
        read(root, SIDEBAR),
        read(root, DISPATCH),
    )
}

#[test]
fn ツリーは切り詰めと読み取り失敗を黙って捨てない() {
    let root = workspace_root();
    let (filetree, sidebar, dispatch) = sources(&root);
    let offenders = all(&filetree, &sidebar, &dispatch);
    assert!(
        offenders.is_empty(),
        "ファイルツリーが上限を超えたぶん / 読めなかった事実を黙って捨てている（#1402）。\
         ユーザーからは「そのファイルが存在しない」ように見え、機械可読側 \
         （`tree git-status` の `truncated`）だけが申告する非対称に戻る:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let (filetree, sidebar, dispatch) = sources(&root);
    for (src, rel, needle) in [
        (&filetree, FILETREE, "fn read_dir_sorted("),
        (&filetree, FILETREE, "fn collect_rows("),
        (&filetree, FILETREE, "fn local_note_of("),
        (&filetree, FILETREE, "fn local_note_row("),
        (&filetree, FILETREE, "pub enum RowNote {"),
        (&sidebar, SIDEBAR, "fn render_note_row("),
        (&dispatch, DISPATCH, "fn tree_git_status_payload("),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{rel} の {needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{rel} の {needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{rel} の {needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
    // 種別の一覧が採れている（空なら描画の網羅検査が無意味に緑になる）
    let variants = row_note_variants(&filetree);
    assert!(
        variants.len() >= 4 && variants.iter().any(|v| v == "Truncated"),
        "`RowNote` の種別が採れていない: {variants:?}"
    );
    // 上限そのものは変えていない（#1402 の Out。値が動いたら申告の意味も変わる）
    assert!(
        filetree.contains("const MAX_ENTRIES: usize = 500;"),
        "表示上限の宣言が見つからない（走査が空振り）"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let (filetree, sidebar, dispatch) = sources(&root);

    // 注入 1: 上限で切ったあと総数を捨てる = #1402 の修正前そのもの
    let cut_only = filetree.replace(
        "    let truncation = Truncation::new(entries.len(), MAX_ENTRIES);\n\
         \x20   entries.truncate(truncation.shown);",
        "    entries.truncate(MAX_ENTRIES);",
    );
    assert!(cut_only != filetree, "注入 1 の対象が見つからない");
    let found = all(&cut_only, &sidebar, &dispatch);
    let expected = cut_only
        .lines()
        .position(|l| l.contains("entries.truncate(MAX_ENTRIES);"))
        .expect("注入した行がある")
        + 1;
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{expected}"))),
        "総数を捨てる逆戻りを名指しできていない: {found:?}"
    );

    // 注入 2: 読み取り失敗を空の一覧へ落とす（修正前の `let Ok(..) else` 相当）
    let silent_err = filetree.replace(
        "            return DirListing {\n                entries: Vec::new(),",
        "            return Vec::new();\n            #[allow(unreachable_code)]\n            DirListing {\n                entries: Vec::new(),",
    );
    assert!(silent_err != filetree, "注入 2 の対象が見つからない");
    let found = all(&silent_err, &sidebar, &dispatch);
    let expected = silent_err
        .lines()
        .position(|l| l.contains("return Vec::new();"))
        .expect("注入した行がある")
        + 1;
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{expected}"))),
        "読み取り失敗を黙って空にする逆戻りを名指しできていない: {found:?}"
    );

    // 注入 3: 行を出すのをやめる（計算はするが描画へ渡さない）
    let no_row = filetree.replace(
        "        if let Some(note) = local_note_of(listing) {",
        "        if let Some(note) = None::<RowNote> {",
    );
    assert!(no_row != filetree, "注入 3 の対象が見つからない");
    assert!(
        !all(&no_row, &sidebar, &dispatch).is_empty(),
        "情報行を出さなくしても緑のまま"
    );

    // 注入 4: 切り詰めだけ申告しない（読めない方だけ残す）
    let only_err = filetree.replace("    if listing.truncation.truncated() {", "    if false {");
    assert!(only_err != filetree, "注入 4 の対象が見つからない");
    assert!(
        !all(&only_err, &sidebar, &dispatch).is_empty(),
        "切り詰めの申告を落としても緑のまま"
    );

    // 注入 5: 描画が種別を `_ =>` で受ける（行はあるのに見え方が同じ = 実質無言）
    let wildcard = sidebar.replace(
        "            filetree::RowNote::Truncated { shown, total } => (",
        "            _ => (",
    );
    assert!(wildcard != sidebar, "注入 5 の対象が見つからない");
    let found = all(&filetree, &wildcard, &dispatch);
    assert!(
        found.iter().any(|o| o.contains(SIDEBAR)),
        "描画の取りこぼしを名指しできていない: {found:?}"
    );

    // 注入 6: CLI 側が判断を手書きへ戻す（画面と答えが割れる）
    let hand_rolled = dispatch.replace(
        "    let cut = tako_core::sidebar::Truncation::new(all.len(), limit);",
        "    let total = all.len();\n    let hand = total > limit;\n    let cut = Cut { shown: limit.min(total), total, hand };",
    );
    assert!(hand_rolled != dispatch, "注入 6 の対象が見つからない");
    let found = all(&filetree, &sidebar, &hand_rolled);
    assert!(
        found.iter().any(|o| o.contains(DISPATCH)),
        "CLI 側の手書きへの逆戻りを名指しできていない: {found:?}"
    );
}

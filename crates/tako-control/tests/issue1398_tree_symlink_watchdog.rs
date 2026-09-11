//! ファイルツリーの種別判定が「リンクを辿った先」で決まっているかの番犬（#1398）
//!
//! # なぜ要るか
//!
//! `crates/tako-app/src/filetree.rs` の `read_dir_sorted` は `DirEntry::file_type()`
//! （**リンクを辿らない**）で `is_dir` を決めていたので、ディレクトリへの
//! シンボリックリンクがファイル行になっていた。一方で**開く側**は辿る
//! （`dispatch::OpenFile` の `Path::is_file()` / `tako file open-in-tako` /
//! ⌘+クリックの `open_plan::route`）ので、同じパスについて 2 つの層が逆の判断をし、
//! 「chevron が出ないフォルダを押すと『ファイルではない』で弾かれる」= #1399 と
//! 重なって**完全に無反応**になっていた。
//!
//! 逆戻りは 1 行（`entry_is_dir(&path, ..)` → `e.file_type().ok()?.is_dir()`）で
//! 起こり、GUI を立てないと症状が出ないので CI で落とす。
//!
//! # 何を縛るか
//!
//! 1. ツリーの種別判定は `entry_is_dir` の 1 実装を通り、それが
//!    `is_symlink()` → `std::fs::metadata`（**辿る**）で決める
//! 2. 辿るようになった結果の循環は `collect_rows` が canonical パスの照合で
//!    打ち切り、打ち切りを**行として見せる**（`RowNote::Error`。黙って空にすると
//!    #1398 で直した「押しても無言」に戻る）
//! 3. 開く側（`Request::OpenFile`）は**判定を変えない**（辿る側のまま）。
//!    2 つの層が同じ向きを向いていることがこの Issue の本質なので、
//!    片側だけ直して満足しない形にしておく
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜3 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で 4 つの窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**現行ソースから作り直した 5 通りの注入**が
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

    // 1. read_dir_sorted は種別を 1 実装（entry_is_dir）へ委ねる
    match fn_window(src, "fn read_dir_sorted(") {
        None => push(0, "`read_dir_sorted` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if let Some(i) = window
                .lines()
                .position(|l| l.contains("file_type()") && l.contains("is_dir()"))
            {
                push(
                    at + i,
                    "種別を `file_type()` で決めている（**リンクを辿らない** = \
                     ディレクトリへのリンクがファイル行になる。#1398）"
                        .into(),
                );
            }
            if !window.contains("entry_is_dir(") {
                push(
                    at,
                    "リンクを辿る 1 実装（`entry_is_dir`）を通っていない".into(),
                );
            }
        }
    }

    // 2. entry_is_dir はリンクを辿る（symlink_metadata では辿らない）
    match fn_window(src, "fn entry_is_dir(") {
        None => push(
            0,
            "`entry_is_dir` が無い（種別判定の 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            if !window.contains("is_symlink()") {
                push(at, "リンクを見分けていない（`is_symlink()` が無い）".into());
            }
            if window.contains("symlink_metadata(") {
                push(
                    line_of(at, &window, "symlink_metadata("),
                    "`symlink_metadata` は**リンクを辿らない**（`std::fs::metadata` を使う）"
                        .into(),
                );
            } else if !window.contains("std::fs::metadata(") {
                push(
                    at,
                    "リンク先を見ていない（`std::fs::metadata` が無い）".into(),
                );
            }
        }
    }

    // 3. 循環の打ち切りと、その可視化
    match fn_window(src, "fn collect_rows(") {
        None => push(0, "`collect_rows` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window.contains("chain.contains(") {
                push(
                    at,
                    "展開の循環を照合していない（canonical パスの `chain.contains` が無い）".into(),
                );
            }
            if !window.contains("loop_cut_row(") {
                push(
                    at,
                    "循環を打ち切る行（`loop_cut_row`）へ繋いでいない".into(),
                );
            }
        }
    }
    match fn_window(src, "fn loop_cut_row(") {
        None => push(
            0,
            "`loop_cut_row` が無い（打ち切りを行として見せていない）".into(),
        ),
        Some((at, window)) => {
            if !window.contains("RowNote::Error") {
                push(
                    at,
                    "打ち切りを黙って行っている（`RowNote::Error` で理由を出す。\
                     `conventions.md`「弾いたら黙って捨てない」）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 開く側（`dispatch.rs` の `Request::OpenFile`）の検査。
///
/// #1398 は**ツリー側を開く側へ揃えた**修正なので、開く側が「辿らない」形へ
/// 動いたら同じバグが逆向きに再発する
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    // dispatch の match アーム（字下げ 8 桁）だけを見る（テスト内の呼び出しは字下げが深い）
    let Some(start) = lines
        .iter()
        .position(|l| *l == "        Request::OpenFile {")
    else {
        return vec![Offender {
            file: DISPATCH,
            line: 0,
            why: "`Request::OpenFile` のアームが見つからない（走査が空振り）".into(),
        }];
    };
    let end = (start + 80).min(lines.len());
    let window = lines[start..end].join("\n");
    if let Some(i) = window.lines().position(|l| l.contains("symlink_metadata(")) {
        out.push(Offender {
            file: DISPATCH,
            line: start + 1 + i,
            why: "開く側がリンクを辿らない形へ変わっている（`symlink_metadata`）".into(),
        });
    } else if !window.contains(".is_file()") {
        out.push(Offender {
            file: DISPATCH,
            line: start + 1,
            why: "開く側の種別検査（`Path::is_file()` = 辿る）が無い".into(),
        });
    }
    out
}

fn all(src_filetree: &str, src_dispatch: &str) -> Vec<String> {
    scan_filetree(src_filetree)
        .iter()
        .chain(scan_dispatch(src_dispatch).iter())
        .map(Offender::report)
        .collect()
}

#[test]
fn ツリーの種別判定はリンクを辿る() {
    let root = workspace_root();
    let offenders = all(&read(&root, FILETREE), &read(&root, DISPATCH));
    assert!(
        offenders.is_empty(),
        "ファイルツリーの種別判定が「リンクを辿った先」で決まっていない（#1398）。\
         ディレクトリへのシンボリックリンクが chevron の無いファイル行になり、\
         押しても開く側（`Path::is_file()`）に弾かれて無反応に戻る:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let src = read(&root, FILETREE);
    for needle in [
        "fn read_dir_sorted(",
        "fn entry_is_dir(",
        "fn collect_rows(",
        "fn loop_cut_row(",
    ] {
        let (at, window) = fn_window(&src, needle)
            .unwrap_or_else(|| panic!("{needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
    let dispatch = read(&root, DISPATCH);
    assert!(
        dispatch.lines().any(|l| l == "        Request::OpenFile {"),
        "`Request::OpenFile` のアームの形が変わっている（走査が空振り）"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let filetree = read(&root, FILETREE);
    let dispatch = read(&root, DISPATCH);

    // 注入 1: 種別判定を `file_type()`（辿らない）へ戻す = #1398 の修正前そのもの
    let reverted = filetree.replace(
        "let is_dir = entry_is_dir(&path, &e.file_type().ok()?);",
        "let is_dir = e.file_type().ok()?.is_dir();",
    );
    assert!(reverted != filetree, "注入 1 の対象が見つからない");
    let found = all(&reverted, &dispatch);
    let expected = reverted
        .lines()
        .position(|l| l.contains("let is_dir = e.file_type().ok()?.is_dir();"))
        .expect("注入した行がある")
        + 1;
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{expected}"))),
        "`file_type()` への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 2: リンクを辿らない metadata へ差し替える
    let shallow = filetree.replace(
        "return std::fs::metadata(path).is_ok_and(|m| m.is_dir());",
        "return std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());",
    );
    assert!(shallow != filetree, "注入 2 の対象が見つからない");
    let found = all(&shallow, &dispatch);
    let expected = shallow
        .lines()
        .position(|l| l.contains("symlink_metadata(path)"))
        .expect("注入した行がある")
        + 1;
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{expected}"))),
        "`symlink_metadata`（辿らない）への差し替えを名指しできていない: {found:?}"
    );

    // 注入 3: 循環の照合を落とす
    let unguarded = filetree.replace("if chain.contains(&real) {", "if false {");
    assert!(unguarded != filetree, "注入 3 の対象が見つからない");
    assert!(
        !all(&unguarded, &dispatch).is_empty(),
        "循環の照合が消えても緑のまま"
    );

    // 注入 4: 打ち切りを黙って行う形（理由を出さない）
    let silent = filetree.replace("note: Some(RowNote::Error(", "note: Some(RowNote::Empty(");
    assert!(silent != filetree, "注入 4 の対象が見つからない");
    assert!(
        !all(&silent, &dispatch).is_empty(),
        "打ち切りの理由を落としても緑のまま"
    );

    // 注入 5: 開く側を「辿らない」形へ動かす
    let shallow_open = dispatch.replace(
        "            if !resolved.is_file() {",
        "            if !std::fs::symlink_metadata(&resolved).is_ok_and(|m| m.is_file()) {",
        // 1 件目（OpenFile のアーム）だけが窓に入る
    );
    assert!(shallow_open != dispatch, "注入 5 の対象が見つからない");
    let found = all(&filetree, &shallow_open);
    assert!(
        found.iter().any(|o| o.contains(DISPATCH)),
        "開く側の逆戻りを名指しできていない: {found:?}"
    );
}

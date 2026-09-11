//! docs のショートカット表とキーバインド表の一致を見る番犬（Issue #1323）。
//!
//! **何を止めるか**: `docs/src/content/docs/guides/keyboard-shortcuts.md` と
//! `crates/tako-app/src/keybindings.rs` が食い違うこと。#1323 の時点で docs は
//! 全 8 表が macOS のキーだけで書かれており、`non_macos_bindings()` の 45 本は
//! **1 つも載っていなかった**（Windows 利用者がキーを知る手段が docs 上に無い）。
//!
//! 手で書いた表は必ずずれる。ずれ方は 2 方向あり、どちらも実害が違う:
//!
//! - **載っていない**（バインドはあるのに docs に無い）= 機能を発見できない
//! - **載っているのに無い**（docs にあるのにバインドが無い）= 書いてあるとおりに
//!   押しても効かない。とくに Windows 列へ `cmd-` 由来の打鍵を書くと、Win キーは
//!   OS が奪うので**別のことが起きる**（#585 / #1203）
//!
//! そこで**両方向**を検査する。
//!
//! # なぜ tako-control に置くか
//!
//! 正本の `keybindings.rs` は tako-app（GPUI）にあるが、`bindings_for` は非公開で、
//! docs の突き合わせに GPUI のビルドを要求したくない。`ui_key_notation.rs` と同じく
//! **ソースを読む**形にして、docs 系の番犬の置き場（ここ）へ揃える。
//!
//! バインド表そのものの不変条件（張るキーの集合・案内文との一致）は tako-app 側の
//! `keybindings::tests` が持つ。ここが見るのは **docs との一致だけ**。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

const DOC: &str = "docs/src/content/docs/guides/keyboard-shortcuts.md";
const SRC: &str = "crates/tako-app/src/keybindings.rs";

/// 修飾キーとして扱うトークン
const MODIFIERS: &[&str] = &["cmd", "ctrl", "alt", "shift"];

/// **docs にだけ載ってよい打鍵**。キーバインド表に無い理由を必ず添える。
///
/// ここを増やすときは「なぜ `keybindings.rs` に無いのか」を書く。
/// 書けないなら、それは docs 側の間違い（= この番犬が止めたいもの）
const DOC_ONLY: &[(&str, &str)] = &[
    (
        "right",
        "入力予測の確定。GPUI のキーバインドではなく端末入力の処理側が見るので \
         keybindings.rs には現れない",
    ),
    (
        "tab",
        "入力予測の確定（→ と同じ）。同じ理由でキーバインド表には無い",
    ),
];

/// 打鍵の正規形。修飾を並べ替えて `alt+cmd+left` のような 1 本の文字列にする
/// （`cmd-alt-left` と `alt-cmd-left` を同じものとして比べるため）
fn canon(mods: &BTreeSet<String>, key: &str) -> String {
    let mut parts: Vec<String> = mods.iter().cloned().collect();
    parts.push(key.to_string());
    parts.join("+")
}

/// `keybindings.rs` の spec 文字列（`ctrl-shift-d` / `cmd--` 等）を正規形へ。
///
/// 先頭から既知の修飾を剥がし、**残り全部**をキーとして扱う
/// （`cmd--` の残りは `-`、`shift-insert` の残りは `insert`）
fn canon_spec(spec: &str) -> String {
    let mut rest = spec;
    let mut mods = BTreeSet::new();
    while let Some((m, tail)) = MODIFIERS
        .iter()
        .find_map(|m| rest.strip_prefix(&format!("{m}-")).map(|tail| (*m, tail)))
    {
        mods.insert(m.to_string());
        rest = tail;
    }
    assert!(!rest.is_empty(), "キーの無い spec: {spec}");
    canon(&mods, rest)
}

/// 指定した関数の本体から `KeyBinding::new("<spec>", …)` の spec を拾う
fn specs_in_fn(src: &str, name: &str) -> Vec<String> {
    let head = format!("\nfn {name}() -> Vec<KeyBinding> {{\n");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{SRC} に {name}() が見つからない（構造が変わった？）"));
    let body = &src[start + head.len()..];
    let end = body
        .find("\n}\n")
        .unwrap_or_else(|| panic!("{name}() の終わりが見つからない"));
    let specs: Vec<String> = body[..end]
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("KeyBinding::new(\"")?;
            Some(rest.split('"').next()?.to_string())
        })
        .collect();
    assert!(!specs.is_empty(), "{name}() から 1 本も拾えていない");
    specs
}

/// `<kbd>X</kbd>` の中身を出現順に拾う
fn kbd_tokens(cell: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = cell;
    while let Some(i) = rest.find("<kbd>") {
        rest = &rest[i + "<kbd>".len()..];
        let Some(j) = rest.find("</kbd>") else { break };
        out.push(rest[..j].trim().to_string());
        rest = &rest[j + "</kbd>".len()..];
    }
    out
}

/// docs の表記（`Cmd` / `←` / `F11`）をキーバインドの語彙へ寄せる
fn token(t: &str) -> String {
    match t {
        "\u{2190}" => "left",
        "\u{2192}" => "right",
        "\u{2191}" => "up",
        "\u{2193}" => "down",
        _ => return t.to_lowercase(),
    }
    .to_string()
}

/// 1 本ぶんの打鍵（`<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>`）を正規形へ。
/// `<kbd>` が無いセル（`（macOS のみ）`）は `None`
fn parse_one(s: &str) -> Option<(BTreeSet<String>, String)> {
    let tokens: Vec<String> = kbd_tokens(s).iter().map(|t| token(t)).collect();
    let (key, mods) = tokens.split_last()?;
    let mods: BTreeSet<String> = mods.iter().cloned().collect();
    assert!(
        mods.iter().all(|m| MODIFIERS.contains(&m.as_str())),
        "修飾でないものが修飾の位置にある: {s}"
    );
    assert!(
        !MODIFIERS.contains(&key.as_str()),
        "キーの位置に修飾だけが置かれている: {s}"
    );
    Some((mods, key.clone()))
}

/// セル 1 つから打鍵を拾う。`/` 区切りは「どれでも同じ」、`〜` は数字の範囲
fn parse_cell(cell: &str) -> Vec<String> {
    if let Some((lhs, rhs)) = cell.split_once('\u{301c}') {
        let (mods, from) = parse_one(lhs).unwrap_or_else(|| panic!("範囲の左辺が読めない: {cell}"));
        let (extra, to) = parse_one(rhs).unwrap_or_else(|| panic!("範囲の右辺が読めない: {cell}"));
        assert!(extra.is_empty(), "範囲の右辺に修飾がある: {cell}");
        let (from, to) = (
            from.parse::<u32>().expect("範囲の左辺が数字でない"),
            to.parse::<u32>().expect("範囲の右辺が数字でない"),
        );
        return (from..=to).map(|n| canon(&mods, &n.to_string())).collect();
    }
    cell.split(" / ")
        .filter_map(parse_one)
        .map(|(mods, key)| canon(&mods, &key))
        .collect()
}

/// docs の 3 列表（`| 操作 | macOS | Windows |`）から両列の打鍵を拾う
fn doc_specs(md: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut mac, mut win) = (BTreeSet::new(), BTreeSet::new());
    let mut in_table = false;
    let mut tables = 0usize;
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            in_table = false;
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        if cells == ["操作", "macOS", "Windows"] {
            in_table = true;
            tables += 1;
            continue;
        }
        if !in_table {
            continue;
        }
        if cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue; // 区切り行
        }
        assert_eq!(cells.len(), 3, "3 列表の行が 3 セルでない: {t}");
        mac.extend(parse_cell(cells[1]));
        win.extend(parse_cell(cells[2]));
    }
    assert!(tables >= 8, "3 列のショートカット表が {tables} 個しか無い");
    (mac, win)
}

/// docs / keybindings.rs を読んで (macOS の期待, Windows の期待, docs の macOS 列, docs の Windows 列)
fn sets() -> (
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
) {
    let root = repo_root();
    let src = std::fs::read_to_string(root.join(SRC)).expect("keybindings.rs を読めない");
    let md = std::fs::read_to_string(root.join(DOC)).expect("keyboard-shortcuts.md を読めない");

    let base = specs_in_fn(&src, "base_bindings");
    let macos_only = specs_in_fn(&src, "macos_only_bindings");
    let non_macos = specs_in_fn(&src, "non_macos_bindings");

    let want_mac: BTreeSet<String> = base
        .iter()
        .chain(macos_only.iter())
        .map(|s| canon_spec(s))
        .collect();
    // Windows は `non_macos_bindings()` だけ。`base_bindings()` の `cmd-` は
    // Windows では Win キーへ解決されて OS に奪われるため、案内に出してはいけない
    // （`keybindings::shortcut_hint_for` が platform 修飾のバインドを落とすのと同じ規則）
    let want_win: BTreeSet<String> = non_macos.iter().map(|s| canon_spec(s)).collect();

    let (doc_mac, doc_win) = doc_specs(&md);
    (want_mac, want_win, doc_mac, doc_win)
}

/// docs にだけ載ってよいものを除いた差分を人が読める形にする
fn report(label: &str, want: &BTreeSet<String>, got: &BTreeSet<String>) -> Vec<String> {
    let allowed: BTreeSet<&str> = DOC_ONLY.iter().map(|(k, _)| *k).collect();
    let mut problems = Vec::new();
    for m in want.difference(got) {
        problems.push(format!(
            "{label}: {m} が keybindings.rs にあるのに docs に無い"
        ));
    }
    for e in got.difference(want) {
        if allowed.contains(e.as_str()) {
            continue;
        }
        problems.push(format!(
            "{label}: {e} が docs にあるのに keybindings.rs に無い（押しても効かない）"
        ));
    }
    problems
}

#[test]
fn docsのmacos列はバインド表と一致する() {
    let (want, _, got, _) = sets();
    let problems = report("macOS", &want, &got);
    assert!(
        problems.is_empty(),
        "docs のショートカット表が keybindings.rs とずれている:\n{}",
        problems.join("\n")
    );
}

#[test]
fn docsのwindows列はバインド表と一致する() {
    let (_, want, _, got) = sets();
    let problems = report("Windows", &want, &got);
    assert!(
        problems.is_empty(),
        "docs のショートカット表が keybindings.rs とずれている:\n{}",
        problems.join("\n")
    );
}

/// #585 / #1203: Windows 列へ `cmd-` 由来の打鍵を書くと、Win キーは OS が奪うので
/// 「書いてあるとおりに押すと別のことが起きる」。集合の一致でも落ちるが、
/// **何が起きるか**を名指しできるよう独立させる
#[test]
fn docsのwindows列にcmd由来の打鍵が無い() {
    let (_, _, _, got) = sets();
    let offenders: Vec<&String> = got.iter().filter(|s| s.contains("cmd")).collect();
    assert!(
        offenders.is_empty(),
        "Windows 列に Cmd の打鍵が載っている（Windows では Win キーになり OS に奪われる）: {offenders:?}"
    );
}

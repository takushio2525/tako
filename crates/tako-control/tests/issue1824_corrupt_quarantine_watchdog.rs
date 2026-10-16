//! #1824 の番犬: 壊れたデータファイルの保全と記録が 1 つの口を通ることを機械で固定する
//!
//! 1. **読み手の口**: `recent.json` / `remote/shortcuts.json` を読む本番コードは
//!    `tako_control::data_files` を通す。core の読み手（`RecentList::load_from` /
//!    `ShortcutsFile::load_from` / `load_from_checked`）を直に呼ぶと、壊れた中身は退避されても
//!    persist.log に何も残らない（#1824 の症状 1）。直に呼んでよいのは core の正本と `data_files`
//! 2. **`.corrupt` の rename をしない**: layout.json を `layout.json.corrupt` へ rename すると
//!    前回の `.corrupt` を上書きして消す（症状 2）。`json.corrupt` を書いてよいのは A/B の旧挙動
//!    （`layout::stash_unrestorable` の legacy 枝）の 1 か所だけ
//! 3. **GUI の復元失敗は退避の口を呼ぶ**: main が `layout::stash_after_failed_restore` を呼ばなく
//!    なったら、壊れた layout.json は退避されずに次の保存で消える
//!
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域とコメントは見ない）。
//! 落ちるときは **file:line で名指し**する。検出力は [`植えた回帰を名指せる`] で固定する。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, literals_only};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// core の読み手を直に呼んでよいファイル（正本と、記録する口）
const READER_HOMES: &[&str] = &[
    "crates/tako-core/src/recent.rs",
    "crates/tako-core/src/remote_shortcuts.rs",
    "crates/tako-control/src/data_files.rs",
];

/// 直に呼ぶと記録が残らない読み手の呼び出し
const DIRECT_READS: &[&str] = &[
    "RecentList::load(",
    "RecentList::load_from",
    "ShortcutsFile::load(",
    "ShortcutsFile::load_from",
    "load_from_checked(",
];

/// `.corrupt` の旧挙動を残してよい 1 か所
const LEGACY_CORRUPT_HOME: &str = "crates/tako-control/src/layout.rs";
const CORRUPT_LITERAL: &str = "json.corrupt";

const GUI_MAIN: &str = "crates/tako-app/src/main.rs";
const STASH_CALL: &str = "layout::stash_after_failed_restore(";

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

/// 本番コードだけ（テスト領域を空白へ潰す。バイト長と行番号は原文のまま）
fn production(src: &str) -> String {
    production_range::scan(src).text
}

/// コード（コメントと文字列を除く）に `needles` が現れた所を `file:line` で
fn code_hits(rel: &str, src: &str, needles: &[&str]) -> Vec<String> {
    let code = code_view(&production(src));
    let mut out = Vec::new();
    for needle in needles {
        for (at, _) in code.match_indices(needle) {
            out.push(format!("{rel}:{} に {needle}", line_of(&code, at)));
        }
    }
    out.sort();
    out
}

/// 文字列リテラルの中に `needle` が現れた所を `file:line` で
fn literal_hits(rel: &str, src: &str, needle: &str) -> Vec<String> {
    let lits = literals_only(&production(src));
    lits.match_indices(needle)
        .map(|(at, _)| format!("{rel}:{} に \"{needle}\"", line_of(&lits, at)))
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 全クレートの `src` の `(相対パス, 中身)`（統合テストの `tests/` は見ない）
fn sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates を読める")
        .flatten()
    {
        walk(&entry.path().join("src"), &mut files);
    }
    files.sort();
    let out: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(p).unwrap_or_default())
        })
        .collect();
    assert!(
        out.len() > 100,
        "走査が壊れている（*.rs が {} 件しか見つからない）",
        out.len()
    );
    for must in READER_HOMES.iter().chain([&GUI_MAIN, &LEGACY_CORRUPT_HOME]) {
        assert!(
            out.iter().any(|(rel, _)| rel == must),
            "{must} が見つからない（改名したならこの番犬も直す）"
        );
    }
    out
}

#[test]
fn 読み手は記録する口を通す() {
    let mut found = Vec::new();
    let mut home_hits = 0;
    for (rel, src) in sources() {
        let hits = code_hits(&rel, &src, DIRECT_READS);
        if READER_HOMES.contains(&rel.as_str()) {
            home_hits += hits.len();
            continue;
        }
        found.extend(hits);
    }
    assert!(
        home_hits > 0,
        "正本の中の読み手が見えない（走査が空振りしている）"
    );
    assert!(
        found.is_empty(),
        "recent.json / remote/shortcuts.json は tako_control::data_files::{{load_recent, \
         load_remote_shortcuts}} で読む（直に読むと破損が persist.log に残らない = #1824）: {found:?}"
    );
}

#[test]
fn corruptへのrenameは旧挙動の一か所だけ() {
    let mut found = Vec::new();
    for (rel, src) in sources() {
        found.extend(literal_hits(&rel, &src, CORRUPT_LITERAL));
    }
    let legacy: Vec<&String> = found
        .iter()
        .filter(|h| h.starts_with(&format!("{LEGACY_CORRUPT_HOME}:")))
        .collect();
    assert_eq!(
        legacy.len(),
        1,
        "旧挙動（TAKO_1824_LEGACY）の 1 か所が見えない・または増えた: {found:?}"
    );
    let others: Vec<&String> = found.iter().filter(|h| !legacy.contains(h)).collect();
    assert!(
        others.is_empty(),
        "壊れたファイルは `.corrupt` へ rename せず layout::stash_unrestorable \
         （番号付きの退避）へ寄せる（rename は前回分を上書きする = #1824）: {others:?}"
    );
}

#[test]
fn guiの復元失敗は退避の口を呼ぶ() {
    let src = std::fs::read_to_string(repo_root().join(GUI_MAIN)).expect("main を読める");
    assert_eq!(
        code_hits(GUI_MAIN, &src, &[STASH_CALL]).len(),
        1,
        "{GUI_MAIN} の復元失敗が {STASH_CALL} を呼んでいない（壊れた layout.json が退避されない）"
    );
}

/// 検出力: 回帰を植えると file:line で名指す（テストとコメントと文字列の中は見ない）
#[test]
fn 植えた回帰を名指せる() {
    let planted = "fn f(p: &Path) {\n    let r = tako_core::recent::RecentList::load_from(p);\n    std::fs::rename(p, p.with_extension(\"json.corrupt\"));\n    let s = ShortcutsFile::load_from_checked(p);\n}\n";
    assert_eq!(
        code_hits("x.rs", planted, DIRECT_READS),
        vec![
            "x.rs:2 に RecentList::load_from".to_string(),
            "x.rs:4 に ShortcutsFile::load_from".to_string(),
            "x.rs:4 に load_from_checked(".to_string(),
        ]
    );
    assert_eq!(
        literal_hits("x.rs", planted, CORRUPT_LITERAL),
        vec!["x.rs:3 に \"json.corrupt\"".to_string()]
    );
    let ignored = "// RecentList::load_from(p) / json.corrupt\nfn f() { let _ = \"RecentList::load_from\"; }\n#[cfg(test)]\nmod tests {\n    fn g() { RecentList::load_from(p); let _ = \"json.corrupt\"; }\n}\n";
    assert!(code_hits("x.rs", ignored, DIRECT_READS).is_empty());
    assert!(literal_hits("x.rs", ignored, CORRUPT_LITERAL).is_empty());
}

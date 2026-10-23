//! docs が再現するコマンドパレットの表記を、いまの ui_text と突き合わせる番犬（Issue #2322）。
//!
//! **何を止めるか**: `features/gui-mode` の場面（`GuiModeToggle.astro`）はパレットの見た目を
//! 手で再現していて、ui_text の表記を変えても追従しない。#1830 で表示言語の行だけ直し、
//! 表示モードの行（「表示モードを切り替え（かんたん / ターミナル）」）と地の文の
//! 「表示モードを切り替え」（メニュー・パレットは「表示モードを切替」）は旧表記のまま残った。
//!
//! 見るのは 2 つ:
//!
//! - 場面のパレットの行（`tk-gm-palette-item`）の文言が、パレットに実在する項目名か
//! - 地の文でこの操作の名前を引く「表示モードを…」の鉤括弧が、メニューの項目名か
//!   パレットの項目名か
//!
//! # なぜ tako-control に置くか
//!
//! 正本の `ui_text`（palette.rs / menu.rs）は tako-app（GPUI）にあるが、docs だけの PR は
//! macOS / Windows の Rust ジョブを skip し、docs を読む番犬は `docs-watchdogs`（ubuntu）が
//! `crates/*/tests/*.rs` から拾って走らせる（#2084）。そこで `docs_keyboard_shortcuts.rs` と
//! 同じく**ソースを読む**形にして、docs 系の番犬の置き場（ここ）へ揃える。
//!
//! パレットとメニューの表記そのものの一致（日英）は tako-app 側の
//! `app_menu_tests::パレットの項目はメニューと同じ表記` が持つ。ここが見るのは **docs との一致だけ**。

use std::path::{Path, PathBuf};

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

const PALETTE_SRC: &str = "crates/tako-app/src/ui_text/palette.rs";
const MENU_SRC: &str = "crates/tako-app/src/ui_text/menu.rs";
/// パレットの見た目を再現する場面の置き場（`tk-gm-palette-item` を持つ .astro を全部見る）
const SCENES_DIR: &str = "docs/src/components/scenes";
/// 地の文でこの操作の名前（「表示モードを…」）を引くページ
const PROSE_DOCS: &[&str] = &[
    "docs/src/components/scenes/GuiModeToggle.astro",
    "docs/src/content/docs/features/gui-mode.mdx",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない（置き場が変わったらここも直す）: {e}"))
}

/// テストモジュールより前（本番部）だけを返す
fn production(src: &str) -> &str {
    src.split("#[cfg(test)]").next().unwrap_or(src)
}

/// `tr!(` の直後の最初の文字列リテラル（= 日本語）を、出てくる順に集める
fn tr_ja_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("tr!(") {
        rest = &rest[at + "tr!(".len()..];
        let trimmed = rest.trim_start();
        let Some(body) = trimmed.strip_prefix('"') else {
            continue; // `tr!(format!(..), ..)` のような動的な文言は対象外
        };
        let mut lit = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(e) = chars.next() {
                        lit.push(e);
                    }
                }
                '"' => break,
                _ => lit.push(c),
            }
        }
        out.push(lit);
    }
    out
}

/// パレットの固定コマンドの日本語の項目名（`cmd_label` の腕の `tr!` の日本語）
fn palette_labels_ja() -> Vec<String> {
    let src = read(PALETTE_SRC);
    let body = production(&src);
    let at = body
        .find("pub fn cmd_label(")
        .unwrap_or_else(|| panic!("{PALETTE_SRC} に cmd_label が無い（読み方を直す）"));
    let labels = tr_ja_literals(&body[at..]);
    assert!(
        labels.len() >= 20,
        "{PALETTE_SRC} から読めた項目名が少なすぎる（{} 件）。書き方が変わったら読み方を直す",
        labels.len()
    );
    labels
}

/// 表示メニューの「表示モードを切替」の日本語（`menu::toggle_ui_mode` の `tr!` の日本語）
fn menu_toggle_ui_mode_ja() -> String {
    let src = read(MENU_SRC);
    let body = production(&src);
    let at = body
        .find("pub fn toggle_ui_mode()")
        .unwrap_or_else(|| panic!("{MENU_SRC} に toggle_ui_mode が無い（読み方を直す）"));
    tr_ja_literals(&body[at..])
        .into_iter()
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| panic!("{MENU_SRC} の toggle_ui_mode の文言を読めない"))
}

/// **docs の場面のパレットの行は、パレットに実在する項目名だけを並べる**（#2322）
#[test]
fn docsの場面のパレットの行は実在の項目名() {
    let labels = palette_labels_ja();
    let root = repo_root();
    let mut scenes: Vec<PathBuf> = std::fs::read_dir(root.join(SCENES_DIR))
        .unwrap_or_else(|e| panic!("{SCENES_DIR} を読めない: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "astro"))
        .collect();
    scenes.sort();
    let mut problems = Vec::new();
    let mut seen = 0usize;
    for path in &scenes {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let body = std::fs::read_to_string(path).unwrap_or_default();
        for (i, line) in body.lines().enumerate() {
            // <span class="tk-gm-palette-item …">文言</span>
            let Some(at) = line.find("class=\"tk-gm-palette-item") else {
                continue;
            };
            seen += 1;
            let text = line[at..]
                .split_once('>')
                .and_then(|(_, rest)| rest.split_once("</span>"))
                .map_or("", |(text, _)| text);
            if !labels.iter().any(|l| l == text) {
                problems.push(format!(
                    "{rel}:{}: パレットの行 {text:?} はパレットに無い（{PALETTE_SRC} の表記に揃える）",
                    i + 1
                ));
            }
        }
    }
    assert!(
        seen >= 3,
        "{SCENES_DIR} にパレットの行が見つからない（{seen} 行）。場面の作りが変わったら読み方も直す"
    );
    assert!(
        problems.is_empty(),
        "docs のパレットの再現がいまの項目名と違う（#2322）:\n  {}",
        problems.join("\n  ")
    );
}

/// **docs の地の文が引く「表示モードを…」は、メニューかパレットの項目名そのもの**（#2322）
#[test]
fn docsの地の文の表示モードの名前はメニューかパレットの表記() {
    let labels = palette_labels_ja();
    let menu = menu_toggle_ui_mode_ja();
    let mut problems = Vec::new();
    let mut seen = 0usize;
    for rel in PROSE_DOCS {
        let body = read(rel);
        for (i, line) in body.lines().enumerate() {
            for (name, _) in line.split('「').skip(1).filter_map(|s| s.split_once('」')) {
                if !name.starts_with("表示モードを") {
                    continue;
                }
                seen += 1;
                if name != menu && !labels.iter().any(|l| l == name) {
                    problems.push(format!(
                        "{rel}:{}: 「{name}」はメニュー（「{menu}」）ともパレットの項目名とも違う",
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        seen >= 2,
        "docs の地の文に「表示モードを…」が見つからない（{seen} か所）。書き換えたらここの対象も直す"
    );
    assert!(
        problems.is_empty(),
        "docs の地の文の名前がメニュー・パレットの表記と違う（#2322）:\n  {}",
        problems.join("\n  ")
    );
}

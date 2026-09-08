//! setup の置き場が data dir の境界を通っていることの番犬（#1019）
//!
//! `tako setup` の生成物は長らく tako-cli 側で
//! `~/Library/Application Support/tako/setup` と**直書き**されていた。これは
//!
//! 1. Windows で `%USERPROFILE%\Library\Application Support\tako\setup` という
//!    その OS に存在しない形の場所を作る（data dir の外なので `tako recover` /
//!    設定共有（#513）/ バックアップの対象から外れ、アンインストールでも残る）
//! 2. `TAKO_DATA_DIR` / `TAKO_ISOLATED=1` で隔離した `tako setup` が
//!    **本番の setup ディレクトリを書き換える**（#1002 の検証で実際に踏んだ）
//!
//! の 2 つを同時に起こす。同じ形が別の場所で復活しないよう、
//! **ホーム相対で macOS のパスを組み立てる書き方**をここで機械的に禁じる。
//!
//! 正本は [`tako_control::setup::setup_dir`]（= `tako_core::paths::data_dir()` + `setup`）。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 行コメントを落とした本文（規約の説明文・doc コメントに当たって誤検知しないため）
fn without_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `crates/**/src/**/*.rs` を集める
fn crate_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(&repo_root().join("crates"), &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // ビルド成果物は走査しない
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// ホーム相対で macOS のデータディレクトリを組み立ててよい場所。
///
/// - `paths.rs` = 境界そのもの（`data_dir()` の macOS 分岐）
/// - `setup.rs` = 旧い場所を**見つけて移す**ための定数（`LEGACY_SETUP_REL`）
const ALLOWED: &[&str] = &[
    "crates/tako-core/src/paths.rs",
    "crates/tako-control/src/setup.rs",
];

#[test]
fn ホーム相対でmacosのデータディレクトリを組み立てない() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for path in crate_sources() {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let body = read(&rel);
        for (n, line) in body.lines().enumerate() {
            let trimmed = line.trim_start();
            // 散文（規約の説明・doc コメント）は実装ではないので拾わない
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains(".join(\"Library/Application Support")
                || line.contains(".join(r\"Library\\Application Support")
            {
                offenders.push(format!("{rel}:{}", n + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ホーム相対で macOS のデータディレクトリを組み立てている: {offenders:?}\n\
         置き場は tako_core::paths::data_dir()（OS 別解決 + TAKO_DATA_DIR）を通すこと。\n\
         直書きすると Windows で存在しない形の場所へ書き、隔離も効かない（#1019）"
    );
}

#[test]
fn cliのsetupは正本のsetup_dirを呼ぶ() {
    let body = without_comments(&read("crates/tako-cli/src/setup.rs"));
    assert!(
        body.contains("tako_control::setup::setup_dir()"),
        "tako-cli の setup_dir は tako_control::setup::setup_dir を呼ぶこと（#1019）"
    );
    assert!(
        !body.contains("Library/Application Support"),
        "tako-cli 側に macOS のパスを持たない（#1019）"
    );
    // ホーム解決の入口は paths（#870）。ここで env を読み直すと片方だけ直る
    for needle in ["var_os(\"HOME\")", "var_os(\"USERPROFILE\")"] {
        assert!(
            !body.contains(needle),
            "ホーム解決は tako_core::paths::home_dir() を通すこと（{needle} を直接読まない。#870）"
        );
    }
}

#[test]
fn 正本のsetup_dirはdata_dirから組み立てる() {
    let body = read("crates/tako-control/src/setup.rs");
    let start = body
        .find("pub fn setup_dir()")
        .expect("setup_dir が見つからない");
    let end = start + body[start..].find("\n}\n").expect("関数の終わり");
    let func = &body[start..end];
    assert!(
        func.contains("tako_core::paths::data_dir()") && func.contains("join(\"setup\")"),
        "setup_dir は data_dir() + \"setup\" で組み立てること（#1019）:\n{func}"
    );
}

/// 隔離した検証が**本番の setup を吸い上げない**こと。
/// 「隔離したつもりが本番を触る」がこの Issue そのものなので、移設側で同じ穴を開けない
#[test]
fn 隔離中は移設を走らせない() {
    let body = read("crates/tako-control/src/setup.rs");
    let start = body
        .find("fn live_relocation_pair()")
        .expect("live_relocation_pair が見つからない");
    let end = start + body[start..].find("\n}\n").expect("関数の終わり");
    let func = &body[start..end];
    assert!(
        func.contains("TAKO_DATA_DIR"),
        "移設の要否は TAKO_DATA_DIR の有無を見て決めること（隔離中は本番を触らない。#1019）:\n{func}"
    );
}

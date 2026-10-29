//! 番犬: ペインの spawn が backend の入口（reserve → 器の `wrap_spawn`）を通る（Issue #885）
//!
//! スライス 2a（#519）が `SessionBackend` を入れた後も、tako-app のペイン spawn は
//! `tmux_backend::wrap_options` を直接呼んでいた。そのため `PsmuxBackend::wrap_spawn` /
//! `reserve` には本番の呼び出し元が無く、psmux の器のオーナー記録（#177 の強奪ガードの材料）・
//! 名前の検査（#428）・器ごとの組み立ての差が本番で効いていなかった。#881 では psmux 側の
//! 組み立てを直しても実機で何も変わらず、気づくのに実機 1 往復かかった。
//!
//! ここでは 2 点を見張る:
//!
//! 1. tako-core の外の製品コード（tako-app / tako-control / tako-cli の `src`）が
//!    `wrap_options(` を直接呼んでいない（戻ったら file:line を名指しで落とす）
//! 2. tako-app の `spawn_session` が `backend::wrap_spawn_for_pane(` を通っている

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 走査する製品コードの置き場（tako-core の外。器の組み立ての実装は tako-core にだけ置く）
const PRODUCT_SRC: &[&str] = &[
    "crates/tako-app/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} を読めない: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `src` の中で器の組み立てを直接呼んでいる行（1 始まりの行番号と、その行）。
/// 行コメント（`//` で始まる行）は説明文なので数えない
fn direct_wrap_calls(src: &str) -> Vec<(usize, String)> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            !code.starts_with("//") && code.contains("wrap_options(")
        })
        .map(|(i, line)| (i + 1, line.trim().to_string()))
        .collect()
}

#[test]
fn tako_coreの外の製品コードはwrap_optionsを直接呼ばない() {
    let root = repo_root();
    let mut found = Vec::new();
    for dir in PRODUCT_SRC {
        let mut files = Vec::new();
        rs_files(&root.join(dir), &mut files);
        assert!(
            !files.is_empty(),
            "{dir} に .rs が 1 つも無い（置き場が変わった？）"
        );
        for file in files {
            let src = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("{} を読めない: {e}", file.display()));
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .display()
                .to_string();
            for (line, text) in direct_wrap_calls(&src) {
                found.push(format!("{rel}:{line}: {text}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "ペインの spawn が backend の入口を通らず `wrap_options` を直接呼んでいる（#885）。\n\
         `tako_core::backend::wrap_spawn_for_pane` を通すこと（reserve の名前の検査 = #428、\n\
         psmux の器のオーナー記録 = #177 が本番で効かなくなる）:\n{}",
        found.join("\n")
    );
}

#[test]
fn tako_appのspawn_sessionはbackendの入口を通る() {
    const MAIN: &str = "crates/tako-app/src/main.rs";
    let src = std::fs::read_to_string(repo_root().join(MAIN))
        .unwrap_or_else(|e| panic!("{MAIN} を読めない: {e}"));
    let start = src.find("fn spawn_session(").unwrap_or_else(|| {
        panic!("{MAIN} に spawn_session が無い（名前が変わったならこの番犬も直す）")
    });
    let line = src[..start].lines().count() + 1;
    // 次のメソッド（同じ字下げの `fn `）の手前までを本体とみなす
    let body = &src[start..];
    let end = body[1..].find("\n    fn ").map_or(body.len(), |i| i + 1);
    let body = &body[..end];
    assert!(
        body.contains("wrap_spawn_for_pane("),
        "{MAIN}:{line} の spawn_session が `tako_core::backend::wrap_spawn_for_pane` を通っていない（#885）"
    );
}

/// 検出器そのものの確認（戻した形を本当に名指しできるか）
#[test]
fn 直接呼び出しを行番号つきで拾い説明のコメントは拾わない() {
    let src = "\
fn spawn() {
    // 旧実装は tmux_backend::wrap_options( を直接呼んでいた
    options = tako_core::tmux_backend::wrap_options(
        options,
    );
}
";
    assert_eq!(
        direct_wrap_calls(src),
        vec![(
            3,
            "options = tako_core::tmux_backend::wrap_options(".to_string()
        )]
    );
}

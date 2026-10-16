//! 番犬: CLI の IPC は上限を入れ直せなかった読みで、届いている応答を捨てない（Issue #2169）
//!
//! macOS は相手が閉じて送受信の両方が閉じたソケットへの setsockopt に EINVAL を返すが、
//! 未読の応答はまだ読める。締め切りつきの読み（#2123 の `deadline::Stream`）は読むたびに
//! `set_read_timeout` で残りの時間を入れ直すので、その失敗を `?` で返すと、GUI が応答を書いて
//! すぐ閉じた回に届いている応答を読まずに CLI が失敗する。失敗したら `read_unarmed`
//! （待たない読みを 1 回だけ）へ回す形を守らせる。
//!
//! 振る舞いは `ipc::transport::deadline::tests`（応答を書いてすぐ閉じる受け口の注入）が見る。
//! 振る舞いのテストは unix でだけ走る（`deadline` は unix の型）ので、ここは OS に依らず
//! `set_read_timeout(…)?` の直書きを `file:line` で落とす。

use std::path::{Path, PathBuf};

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} を読めない: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `set_read_timeout(` で始まる式が `)?` で閉じる箇所（改行を挟んだ形も拾う）の行番号
fn propagated_read_timeouts(src: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("set_read_timeout(") {
        let start = from + at;
        let open = start + "set_read_timeout".len();
        // 対応する閉じ括弧
        let mut depth = 0usize;
        let mut close = None;
        for (i, c) in src[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        if src[close + 1..].trim_start().starts_with('?') {
            lines.push(src[..start].matches('\n').count() + 1);
        }
        from = close + 1;
    }
    lines
}

#[test]
fn 読みの上限の入れ直しを問答無用で返さない() {
    let mut files = Vec::new();
    rust_files(&crate_src(), &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file:?}: {e}"));
        for line in propagated_read_timeouts(&src) {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(file);
            found.push(format!("crates/tako-cli/{}:{line}", rel.display()));
        }
    }
    assert!(
        found.is_empty(),
        "`set_read_timeout(…)?` は相手が閉じた後の macOS で EINVAL を返し、届いている応答を\
         読まずに失敗する（#2169）。失敗したら待たない読みで届いた分を読む（`read_unarmed`）: {found:?}"
    );
}

#[test]
fn 検出器は直書きと改行を挟んだ形を拾う() {
    let src = "fn a() {\n    s.set_read_timeout(Some(left))?;\n}\n\
               fn b() {\n    s.set_read_timeout(Some(Duration::from_secs(1)))\n        ?;\n}\n\
               fn c() {\n    if let Err(e) = s.set_read_timeout(Some(left)) {}\n}\n\
               fn d() {\n    s.set_read_timeout(Some(t)).ok();\n}\n";
    assert_eq!(propagated_read_timeouts(src), vec![2, 5]);
}

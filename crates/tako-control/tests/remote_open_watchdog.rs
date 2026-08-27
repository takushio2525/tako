//! 「器つきペインの外側 alt screen」を SSH 化の判定へ渡していないかの番犬（#1006）
//!
//! バックエンド（tmux）ペインでは tmux クライアント自身が alt screen へ入るので、
//! `TerminalSession::is_alt_screen()` は**中身が素のシェルでも常に true**。
//! これを `remote_open::can_ssh_pane` の第 2 引数へそのまま渡すと、persist が
//! 有効な環境（= 既定）の**全ペイン**が「全画面 TUI」扱いになり、
//! ペインメニューの「このペインでリモート接続…」が一度も出なくなる。
//!
//! #694 が `pane_inner_alt_screen` で同じ罠を回避しており、#1006 の隔離セルフテストで
//! 実測（素のシェルのバックエンドペインで outer_alt=true / inner_alt=false）した。
//! 見た目には壊れているように見えないので、機械検査で止める。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // このファイルは <root>/crates/tako-control/tests/ にある
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // ビルド生成物と使い捨て検証コードは対象外
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "poc" || name.starts_with('.') {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// `can_ssh_pane(` の呼び出しから、閉じ括弧までの引数テキストを切り出す
fn call_args(src: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("can_ssh_pane(") {
        let after = &rest[at + "can_ssh_pane(".len()..];
        let mut depth = 1usize;
        let mut end = after.len();
        for (i, c) in after.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        found.push(after[..end].to_string());
        rest = &after[end..];
    }
    found
}

#[test]
fn ssh化の判定へ外側のalt_screenを渡していない() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    assert!(!files.is_empty(), "走査対象が空（パス解決を間違えている）");

    let mut offenders = Vec::new();
    let mut checked = 0usize;
    for file in &files {
        // 番犬自身と、判定の定義側（引数名として出てくる）は対象外
        if file.ends_with("remote_open_watchdog.rs") || file.ends_with("remote_open.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(file) else {
            continue;
        };
        for args in call_args(&src) {
            checked += 1;
            // 生の `is_alt_screen()` を渡していたら、器つきペインで必ず true になる。
            // 器を除いた形（`!backend && …` / `pane_inner_alt_screen`）だけを許す
            let raw = args.contains("is_alt_screen()");
            let guarded = args.contains("pane_inner_alt_screen") || args.contains("!backend");
            if raw && !guarded {
                offenders.push(format!("{}", file.display()));
            }
        }
    }
    assert!(
        checked >= 2,
        "呼び出しを 1 つも見つけられていない（切り出しが壊れた）: checked={checked}"
    );
    assert!(
        offenders.is_empty(),
        "器つきペインの外側 alt screen を SSH 化の判定へ渡している（#1006 / #694）: {offenders:?}"
    );
}

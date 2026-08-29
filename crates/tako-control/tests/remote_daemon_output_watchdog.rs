//! 番犬: remote デーモンは**起動情報を出したあと** stdout / stderr へ書かない（#1049）
//!
//! `spawn_daemon` は子の stdout / stderr を pipe にして起動情報 JSON を読み、
//! 読み終えたら `Child` を落として pipe を閉じる。以後に子が `println!` /
//! `eprintln!` を呼ぶと **EPIPE で panic** する。
//!
//! 実測（#1049）: 自己検査スレッドの「張り直しました」の 1 行で**スレッドが黙って死に**、
//! `serve_ok` が `stale` になるまで誰も気づけなかった。終了直前の 1 行でも危険で、
//! panic すると直後の `cleanup_state_files()` が飛んで state ファイルが残る。
//!
//! 記録先は `<state_dir>/audit.log`（`audit_serve`）と `<state_dir>/tako-remote.serve`。

use std::path::{Path, PathBuf};

fn remote_rs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote.rs")
}

/// 関数本文を波括弧の釣り合いで切り出す（`fn <name>` から対応する `}` まで）
fn function_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let open = src[start..]
        .find('{')
        .unwrap_or_else(|| panic!("{signature} の本文が見つからない"))
        + start;
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("{signature} の本文を閉じられない");
}

/// コメント行を落とす（説明文の中の `eprintln!` を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_no_console_output(body: &str, what: &str) {
    let body = without_comments(body);
    for (n, line) in body.lines().enumerate() {
        for bad in ["println!", "eprintln!", "print!", "eprint!"] {
            assert!(
                !line.contains(bad),
                "{what} の {n} 行目に {bad} がある（#1049: 起動後の stdout / stderr は\n\
                 EPIPE で panic する。記録は audit_serve か serve health ファイルへ）:\n  {line}"
            );
        }
    }
}

#[test]
fn 自己検査スレッドはコンソールへ書かない() {
    let src = std::fs::read_to_string(remote_rs()).expect("remote.rs を読む");
    let body = function_body(&src, "fn serve_watch_loop(");
    assert_no_console_output(&body, "serve_watch_loop");
}

#[test]
fn 起動情報を出したあとの_run_daemon_はコンソールへ書かない() {
    let src = std::fs::read_to_string(remote_rs()).expect("remote.rs を読む");
    let body = function_body(&src, "pub fn run_daemon()");
    // 起動情報 JSON の出力が「ここから先は pipe が閉じる」の境界
    let marker = r#"println!("{info}");"#;
    let at = body
        .find(marker)
        .expect("起動情報の出力が見つからない（改名したら番犬も直す）");
    assert_no_console_output(&body[at + marker.len()..], "run_daemon の起動情報より後");
}

#[test]
fn 検査は実際に踏んだ形を検出する() {
    // #1049 で実際に死んだ形（張り直しの通知を eprintln で出す）を注入すると落ちる
    let injected = "{\n    let was = 1;\n    eprintln!(\"張り直しました（{was}）\");\n}";
    let caught = std::panic::catch_unwind(|| assert_no_console_output(injected, "注入"));
    assert!(caught.is_err(), "注入した eprintln! を検出できていない");
}

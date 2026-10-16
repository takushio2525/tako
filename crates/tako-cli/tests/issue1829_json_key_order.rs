//! 番犬: CLI が出す JSON のキーの並びは、ビルドの形に依らず挿入順（Issue #1829）
//!
//! serde_json の `preserve_order`（`Map` を IndexMap にしてキーを挿入順に保つ）は gpui（zed 由来）が
//! 要求するので、tako-app と一緒にビルドしたとき（配布物の `-p tako-app -p tako-cli`・CI の
//! `--workspace`）だけ効き、手元の `cargo build -p tako-cli` では外れて `Map` が BTreeMap（辞書順）に
//! なっていた。同じ要求への出力がビルドの形でバイト単位に変わり、出力をバイト比較するスクリプト・
//! テストが壊れる（PR #1828 の `tako theme` の応答・PR #2108 の docs の写し）。ルートの Cargo.toml の
//! `[workspace.dependencies]` で全メンバーへ有効にして、並びを挿入順に固定した。
//!
//! このテストが修正の有無で白黒を変えるのは **gpui を含まない宇宙**（`cargo test -p tako-cli`）だけで、
//! `--workspace` では gpui が同じ feature を入れるので修正前でも緑になる。CI がその宇宙を見るのは
//! tako-core の lib.rs 末尾のコンパイル時の番犬（gpui 抜きの clippy の宇宙で型検査される）。
//! 2 つのビルド形の出力のバイト一致そのものは `scripts/test-cli-json-key-order-1829.sh` が実バイナリで見る。

use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 隔離した tako（本番の GUI にも data dir にも触らない）
fn tako(data_dir: &ScratchDir, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
    cmd.args(args)
        .env_remove("TAKO_SOCKET")
        .env_remove("TAKO_PANE_ID")
        .env_remove("TAKO_TOKEN")
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", data_dir.path())
        .stdin(Stdio::null());
    cmd
}

/// 整形済み JSON の中で `marker` の直後に開く最初のオブジェクトの、直下のキーを並びのまま返す。
/// `Value` へ読み直すと読む側の `Map` の実装で並びが変わるので、文字列のまま拾う
fn keys_of_first_object_after(text: &str, marker: &str) -> Vec<String> {
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("出力に {marker} が無い:\n{text}"));
    let rest = &text[start + marker.len()..];
    let open = rest.find('{').expect("オブジェクトが無い");
    let body = &rest[open..];
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut string_start = 0usize;
    // 直下（深さ 1）で閉じた直後の文字列。次の空白以外の文字が `:` ならキー
    let mut pending: Option<&str> = None;
    for (i, c) in body.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
                if depth == 1 {
                    pending = Some(&body[string_start..i]);
                }
            }
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        let candidate = pending.take();
        match c {
            '"' => {
                in_string = true;
                string_start = i + 1;
            }
            ':' => keys.extend(candidate.map(str::to_string)),
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return keys;
                }
            }
            _ => {}
        }
    }
    panic!("オブジェクトが閉じていない:\n{text}");
}

#[test]
fn 抽出器は直下のキーを並びのまま拾う() {
    let text = r#"{ "x": 1, "list": [ { "b": "a:b", "a": { "z": 1 }, "c\"q": [1, {"y": 2}] } ] }"#;
    assert_eq!(
        keys_of_first_object_after(text, r#""list""#),
        vec!["b", "a", "c\\\"q"]
    );
}

#[test]
fn ローカルで組んだ_json_は定義の順で出る() {
    // `tako agent-support --json` は `tako_control::agent_support::report` が `json!` で組んだ
    // `Value` をそのまま整形して出す（GUI 不要）。系統の 1 件は key → label → baseline の順に
    // 書いてあり、辞書順（修正前の単独ビルド）なら baseline が先頭に来る
    let data_dir = ScratchDir::new("issue1829");
    let out = tako(&data_dir, &["agent-support", "--json"])
        .output()
        .expect("tako を起こせない");
    assert_eq!(out.status.code(), Some(0), "終了コード");
    let text = String::from_utf8(out.stdout).expect("UTF-8");
    let keys = keys_of_first_object_after(&text, r#""agents""#);
    assert_eq!(
        keys.first().map(String::as_str),
        Some("key"),
        "系統の 1 件のキーが定義の順で出ていない（serde_json の preserve_order が外れた宇宙。#1829）: {keys:?}"
    );
    let mut sorted = keys.clone();
    sorted.sort();
    assert_ne!(keys, sorted, "キーが辞書順に並び替わっている: {keys:?}");
}

#[test]
fn 読み直した_json_も並びを変えない() {
    // CLI は GUI の応答を `Value` へ読み直して出す。読み直しで並びが変わらないこと（この宇宙の `Map`）
    let text =
        r#"{"theme":"dark","mode":"dark","available":["dark","light"],"nested":{"z":1,"a":2}}"#;
    let value: serde_json::Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(value.to_string(), text);
}

/// GUI の応答をそのまま出すコマンドが、応答のキーの並びを変えずに出す（PR #1828 の `tako theme` の形）。
/// 応答を 1 行返すだけの偽の IPC サーバーで GUI の代わりをする
#[cfg(unix)]
#[test]
fn gui_の応答を並びのまま出す() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    // 辞書順なら available → mode → presets → theme になる並び
    let result = r#"{"theme":"dark","mode":"dark","available":["dark","light"],"presets":[]}"#;
    let data_dir = ScratchDir::new("issue1829");
    let socket = data_dir.path().join("s");
    let listener = UnixListener::bind(&socket).expect("偽の IPC サーバーを立てられない");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("接続を受けられない");
        let mut reader = BufReader::new(stream.try_clone().expect("複製できない"));
        let mut request = String::new();
        reader.read_line(&mut request).expect("要求を読めない");
        let mut stream = stream;
        writeln!(stream, r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#)
            .expect("応答を書けない");
        request
    });

    let out = tako(&data_dir, &["theme"])
        .env("TAKO_SOCKET", &socket)
        .env("TAKO_TOKEN", "issue1829")
        .output()
        .expect("tako を起こせない");
    // 繋がらずに失敗したときは偽の IPC サーバーが accept で待ち続けるので、join より先に見る
    assert_eq!(
        out.status.code(),
        Some(0),
        "終了コード（stderr: {}）",
        String::from_utf8_lossy(&out.stderr)
    );
    let request = server.join().expect("偽の IPC サーバーが落ちた");
    assert!(
        request.contains(r#""method":"theme""#),
        "偽の IPC サーバーへ届いた要求が theme ではない: {request}"
    );
    assert_eq!(
        String::from_utf8(out.stdout).expect("UTF-8").trim_end(),
        result,
        "GUI の応答のキーの並びが CLI で変わった（serde_json の preserve_order が外れた宇宙。#1829）"
    );
}

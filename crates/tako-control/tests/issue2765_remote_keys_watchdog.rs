//! 番犬: スマホ（PWA）のクイックキーが remote daemon を経て**キーとして**ペインへ届く形を崩さない（#2765）。
//!
//! ## 何を止めたいのか
//!
//! PWA のクイックキー（esc / tab / ^C / ↑ / ↓）と ctrl の組み合わせは、キー名（`Escape` / `C-c` …）を
//! `POST /api/panes/<id>/input` の `{keys}` で送る。#281 で daemon の送り先を `tmux send-keys`（名前を
//! 解釈する）から dispatch へ寄せたとき、`{keys}` を `Send` の本文へ写していたので、ペインには名前の文字
//! （`E s c a p e`）が打たれていた（#2765 の実測）。壊れ方は 2 つ:
//!
//! 1. daemon の側で `{keys}` を dispatch `SendKeys`（語彙の正本 `tako_core::backend::key_name_bytes`）以外へ
//!    写す（本文の `Send` に戻す・別の口で読む）
//! 2. PWA の側で語彙に無い名前を送る（daemon は 1 バイトも送らずに 400 で断るので、ボタンが黙って効かない）
//!
//! 1 はソース走査、2 は PWA のソースから名前を拾って語彙の正本に通す。**落ちるときは file:line で名指し**。
//!
//! 相方: 写し方の単体は remote.rs の `issue2765_*`（要求の形・断りの状態コードと接続の保持・A/B）、
//! 実経路は `scripts/test-remote-keys-2765.sh`（実 GUI の器のペイン + 実 daemon + PWA の live spec で、
//! ペインに届いたバイトを 16 進で見る）。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① remote.rs の `input_request` の `Some(keys) if !legacy => crate::protocol::Request::SendKeys {` の腕を消す
//!    （修正前の `Send` の腕だけが残る）/
//! ② remote.rs の `dispatch_send` で `input_request(` を通さず `Request::Send { … text: keys_str … }` を組む /
//! ③ terminal.jsx の `QUICK_KEYS` の `seq: 'Escape'` を `seq: 'Esc'`（語彙に無い名前）へ

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const TERMINAL_REL: &str = "web/tako-remote/src/pages/terminal.jsx";
const API_REL: &str = "web/tako-remote/src/api.js";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// 本番の範囲（最初の `#[cfg(test)]` より前）。remote.rs の番犬の慣例（ファイルの途中に `#[cfg(test)]` を置かない）
fn production(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .take_while(|(_, l)| l.trim() != "#[cfg(test)]")
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// `fn <name>(` の行から、行頭の `}` までの本体（行番号つき）
fn fn_body<'a>(lines: &[(usize, &'a str)], name: &str) -> Vec<(usize, &'a str)> {
    let head = format!("fn {name}(");
    let start = lines
        .iter()
        .position(|(_, l)| l.contains(&head))
        .unwrap_or_else(|| {
            panic!("{REMOTE_REL}: `{head}` が見つからない（関数名を変えたならこの番犬も直す）")
        });
    lines[start..]
        .iter()
        .take_while(|(_, l)| *l != "}")
        .copied()
        .collect()
}

/// ① ② daemon の `{keys}` は `input_request` を通り、そこで `SendKeys` へ写る
#[test]
fn daemonのkeysはsend_keysへ写る() {
    let src = read(REMOTE_REL);
    let lines = production(&src);

    // `{keys}` を読むのは `/input` の腕の 1 か所だけで、それは `dispatch_send` へ渡る
    let readers: Vec<usize> = lines
        .iter()
        .filter(|(_, l)| l.contains(r#"["keys"]"#))
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(
        readers.len(),
        1,
        "{REMOTE_REL}: `{{keys}}` を読む行が 1 か所ではない（行 {readers:?}）。新しい口も `input_request` を通す（#2765）"
    );
    let at = lines
        .iter()
        .position(|(n, _)| *n == readers[0])
        .expect("行");
    assert!(
        lines[at..]
            .iter()
            .take(60)
            .any(|(_, l)| l.contains("dispatch_send(")),
        "{REMOTE_REL}:{}: `{{keys}}` を読んだ値が `dispatch_send` へ渡らない（#2765）",
        readers[0]
    );

    // `dispatch_send` は要求を自前で組まず `input_request` に任せる
    let send = fn_body(&lines, "dispatch_send");
    assert!(
        send.iter().any(|(_, l)| l.contains("input_request(")),
        "{REMOTE_REL}:{}: `dispatch_send` が `input_request` を通らない（#2765）",
        send[0].0
    );
    if let Some((n, _)) = send.iter().find(|(_, l)| l.contains("Request::Send {")) {
        panic!(
            "{REMOTE_REL}:{n}: `dispatch_send` が `Request::Send` を自前で組んでいる。`{{keys}}` を本文で打つと \
             キー名が文字のまま届く（#2765）。要求は `input_request` で組む"
        );
    }

    // `input_request` の `{keys}` の腕（A/B でない方）は `SendKeys`
    let build = fn_body(&lines, "input_request");
    let arm = build
        .iter()
        .find(|(_, l)| l.contains("Some(keys) if !legacy =>"))
        .unwrap_or_else(|| {
            panic!(
                "{REMOTE_REL}:{}: `input_request` に `{{keys}}`（A/B でない方）の腕が無い（#2765）",
                build[0].0
            )
        });
    assert!(
        arm.1.contains("Request::SendKeys"),
        "{REMOTE_REL}:{}: `{{keys}}` の腕が `SendKeys` へ写らない（#2765）: {}",
        arm.0,
        arm.1.trim()
    );
}

/// PWA のソースから `sendKeys(…)` に渡る名前を拾う（`QUICK_KEYS` の `seq` と、直書きの `sendKeys(paneId, '…')`）
fn pwa_key_names(src: &str) -> Vec<(usize, String)> {
    let mut names = Vec::new();
    for (i, line) in src.lines().enumerate() {
        for marker in ["seq: '", "sendKeys(paneId, '"] {
            let mut rest = line;
            while let Some(at) = rest.find(marker) {
                let tail = &rest[at + marker.len()..];
                let Some(end) = tail.find('\'') else { break };
                names.push((i + 1, tail[..end].to_string()));
                rest = &tail[end..];
            }
        }
    }
    names
}

/// ③ PWA が `{keys}` で送る名前は、どれも語彙の正本（`key_name_bytes`）が受ける
#[test]
fn pwaが送るキー名はどれも語彙にある() {
    let api = read(API_REL);
    assert!(
        api.contains("{ keys }"),
        "{API_REL}: `sendKeys` が `{{ keys }}` を送らない（daemon の `{{keys}}` の形が変わったならこの番犬も直す）"
    );
    let src = read(TERMINAL_REL);
    let names = pwa_key_names(&src);
    // クイックキーの 5 つ（esc / tab / ^C / ↑ / ↓）と停止ボタンの Escape が拾えている
    for want in ["Escape", "Tab", "C-c", "Up", "Down"] {
        assert!(
            names.iter().any(|(_, n)| n == want),
            "{TERMINAL_REL}: クイックキーの `{want}` が拾えない（拾い方 `pwa_key_names` を直す）: {names:?}"
        );
    }
    for (line, name) in &names {
        assert!(
            tako_core::backend::key_name_bytes(name).is_some(),
            "{TERMINAL_REL}:{line}: PWA が送るキー名 `{name}` が語彙（tako_core::backend::key_name_bytes）に無い。\
             daemon は 1 バイトも送らずに 400 で断る（#2765）"
        );
    }
    // ctrl の組み合わせ（`C-${e.key}`）は英字なら大小どちらも制御文字 1 バイト
    assert!(
        src.contains("`C-${e.key}`"),
        "{TERMINAL_REL}: ctrl の組み合わせ `C-${{e.key}}` が見つからない（形が変わったならこの番犬も直す）"
    );
    for c in ('a'..='z').chain('A'..='Z') {
        let name = format!("C-{c}");
        assert_eq!(
            tako_core::backend::key_name_bytes(&name),
            Some(vec![(c.to_ascii_lowercase() as u8) - b'a' + 1]),
            "PWA の ctrl + {c}（`{name}`）が制御文字 1 バイトにならない（#2765）"
        );
    }
}

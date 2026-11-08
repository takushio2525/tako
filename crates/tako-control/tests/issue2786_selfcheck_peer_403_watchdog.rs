//! remote の起動時の自己疎通が、接続元検証（#841）の 403 を「到達した」と数えないことの番犬（#2786）
//!
//! # なぜ要るか
//!
//! 自己疎通（#1038）は serve 越しに daemon 自身の `/api/health` を 1 回 GET し、401 / 403 / 404 を「serve が
//! backend へ届いている」証拠として正常に数える。#2786 より前は 403 を本文に依らず到達と数えたので、接続元検証が
//! serve の中継（tailscale デーモン）を信じられず**全リクエストを 403 で断る**環境でも、`tako remote start` は
//! 成功を名乗り、スマホからは全部 403 なのに利用者には原因の手がかりが無かった。
//! いまは daemon が接続元検証の断りの本文に `kind` と理由コードを添え（`RemoteRefusal::http_body`）、自己疎通は
//! 403 の本文を読んで `ServeReachability::PeerRejected` と分ける。daemon は**止めず**に、start の応答の
//! `self_check` と status の `peer_verification.self_check_rejected` に理由を残し、CLI は表示言語で理由と次の一手を
//! 言う。分類の振る舞いは `remote::tests`（`自己疎通は接続元検証の403を到達と数えない` ほか）と
//! `remote_self_check::tests` が見る。ここは**崩れ方の形**を縛る:
//!
//! 1. `probe_serve` は 403 の本文つきの分類 `classify_self_check_reply(` を通す。状態コードだけの
//!    `classify_self_check(` を呼んでよいのは A/B（`remote_self_check::legacy_mode()`）の枝だけ
//! 2. `classify_self_check_reply` は本文を `remote_self_check::peer_reject_of_body(` で読み、`PeerRejected` を返す
//! 3. `run_daemon` は `PeerRejected` を `record_self_check_peer_reject(` で残し、**その枝で止めない**
//!    （`release_serve(` / `return Err(` / `cleanup_state_files(` を書かない = 502 の出口と違う）。
//!    起動情報の `self_check` は `remote_self_check::start_field(` で組む
//! 4. `forwarded_identity` は断りを型（`RemoteRefusal::PeerUnverified(reject)`）のまま返し（文にすると本文に
//!    `kind` / `code` が付かない）、検証を通った接続で `record_peer_trusted(` を呼ぶ。本文を組む `refuse` は
//!    `.http_body()` を通す
//! 5. `daemon_status` は自己疎通の断りを `warnings` に載せる（`remote_self_check::warning_in(`）
//! 6. CLI の `remote_start` / `remote_status` は応答の欄から表示言語で組む
//!    （`remote_self_check::detail_of_start_in(` / `detail_of_status_in(`）
//!
//! [`修正前の形と崩れ方を名指しする`] で、本物のソースへ修正前の形と崩れ方を注入し、file:line で名指しされる
//! ことを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";

const PROBE: &str = "fn probe_serve(";
const CLASSIFY_REPLY: &str = "pub fn classify_self_check_reply(";
const RUN_DAEMON: &str = "pub fn run_daemon() -> io::Result<()> {";
const FORWARDED: &str = "fn forwarded_identity(";
const REFUSE: &str = "fn refuse(request: tiny_http::Request";
const STATUS: &str = "pub fn daemon_status() -> Value {";
const PEER_ARM: &str = "if let Some(ServeReachability::PeerRejected(reject)) = self_check {";
const CLI_START: &str = "fn remote_start() -> Result<(), String> {";
const CLI_STATUS: &str = "fn remote_status() -> Result<(), String> {";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let raw = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 行のコード部分（`//` より前）
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// 窓の中で `needle` を含む最初のコードの行（窓の中の添字）
fn find(w: &[String], needle: &str) -> Option<usize> {
    w.iter().position(|l| code(l).contains(needle))
}

/// 行が `needle`（`名前(`）を呼んでいるか（宣言・長い名前の一部は数えない）
fn calls(line: &str, needle: &str) -> bool {
    let c = code(line);
    c.match_indices(needle).any(|(i, _)| {
        !c[..i].trim_end().ends_with("fn")
            && c[..i]
                .chars()
                .last()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
    })
}

/// 窓を要求する（無ければ名指して空を返す）
fn need(src: &str, rel: &str, needle: &str, out: &mut Vec<String>) -> Option<(usize, Vec<String>)> {
    let found = window(src, needle);
    if found.is_none() {
        out.push(format!(
            "{rel} — `{needle}` が見つからない（改名したならこの番犬の表も直す）"
        ));
    }
    found
}

/// 1. `probe_serve` は本文つきの分類を通し、状態コードだけの分類は A/B の枝に限る
fn scan_probe(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = need(src, REMOTE, PROBE, out) else {
        return;
    };
    if find(&w, "classify_self_check_reply(").is_none() {
        out.push(format!(
            "{REMOTE}:{at} — `probe_serve` が 403 の本文で分類していない（`classify_self_check_reply(` を通さない = \
             接続元検証の 403 を到達と数える #2786 の修正前の形）"
        ));
    }
    for (i, line) in w.iter().enumerate() {
        if !calls(line, "classify_self_check(") {
            continue;
        }
        // A/B の枝の中（直前の行が `if …legacy_mode() {`）だけ。枝の外へ落ちる行は数えない
        let guarded = i > 0
            && code(&w[i - 1]).contains("remote_self_check::legacy_mode()")
            && code(&w[i - 1]).trim_end().ends_with('{');
        if !guarded {
            out.push(format!(
                "{REMOTE}:{} — `probe_serve` が状態コードだけで分類している（`classify_self_check(` は A/B \
                 `remote_self_check::legacy_mode()` の枝だけ）",
                at + i
            ));
        }
    }
}

/// 2. 本文の読み方は 1 実装（`peer_reject_of_body`）
fn scan_classify(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = need(src, REMOTE, CLASSIFY_REPLY, out) else {
        return;
    };
    if find(&w, "remote_self_check::peer_reject_of_body(").is_none()
        || find(&w, "ServeReachability::PeerRejected(").is_none()
    {
        out.push(format!(
            "{REMOTE}:{at} — `classify_self_check_reply` が 403 の本文を `remote_self_check::peer_reject_of_body(` で\
             読んで `PeerRejected` を返していない"
        ));
    }
}

/// 3. `run_daemon` は断りを残し、その枝で止めない
fn scan_daemon(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = need(src, REMOTE, RUN_DAEMON, out) else {
        return;
    };
    match find(&w, PEER_ARM) {
        None => out.push(format!(
            "{REMOTE}:{at} — `run_daemon` が自己疎通の接続元検証の断り（`PeerRejected`）を扱っていない"
        )),
        Some(i) => {
            let arm = window(&w[i..].join("\n"), PEER_ARM)
                .map(|(_, l)| l)
                .unwrap_or_default();
            if find(&arm, "record_self_check_peer_reject(").is_none() {
                out.push(format!(
                    "{REMOTE}:{} — `run_daemon` が自己疎通の断りを `record_self_check_peer_reject(` で残していない \
                     （status から理由を読めない）",
                    at + i
                ));
            }
            for (k, line) in arm.iter().enumerate() {
                for stop in ["release_serve(", "return Err(", "cleanup_state_files("] {
                    if code(line).contains(stop) {
                        out.push(format!(
                            "{REMOTE}:{} — `run_daemon` が接続元検証の断りで daemon を止めている（`{stop}`）。\
                             serve は届いているので止めず、start / status で理由を言う（#2786）",
                            at + i + k
                        ));
                    }
                }
            }
        }
    }
    if find(&w, "remote_self_check::start_field(").is_none() {
        out.push(format!(
            "{REMOTE}:{at} — `run_daemon` の起動情報の `self_check` が `remote_self_check::start_field(` で組まれていない"
        ));
    }
}

/// 4. 断りは型のまま運び、本文は `http_body` で組む。検証を通った接続で断りを下ろす
fn scan_body(src: &str, out: &mut Vec<String>) {
    if let Some((at, w)) = need(src, REMOTE, FORWARDED, out) {
        if find(&w, "RemoteRefusal::PeerUnverified(reject))").is_none()
            || find(&w, "PeerUnverified(reject).to_string()").is_some()
        {
            out.push(format!(
                "{REMOTE}:{at} — `forwarded_identity` が接続元検証の断りを型のまま返していない（文にすると本文に \
                 `kind` / `code` が付かず、自己疎通が見分けられない）"
            ));
        }
        if !w.iter().any(|l| calls(l, "ctx.record_peer_trusted(")) {
            out.push(format!(
                "{REMOTE}:{at} — `forwarded_identity` が検証を通った接続で `record_peer_trusted(` を呼ばない \
                 （自己疎通の断りが下りず、status が言い続ける）"
            ));
        }
    }
    if let Some((at, w)) = need(src, REMOTE, REFUSE, out) {
        if find(&w, ".http_body()").is_none() {
            out.push(format!(
                "{REMOTE}:{at} — `refuse` が `RemoteRefusal::http_body` で本文を組んでいない（接続元検証の `kind` / \
                 `code` が落ちる）"
            ));
        }
    }
}

/// 5. status は断りを warnings に載せる
fn scan_status(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = need(src, REMOTE, STATUS, out) else {
        return;
    };
    if find(&w, "remote_self_check::warning_in(").is_none()
        || find(&w, "self_check_rejected").is_none()
    {
        out.push(format!(
            "{REMOTE}:{at} — `daemon_status` が自己疎通の接続元検証の断りを warnings に載せていない（running だけを名乗る）"
        ));
    }
}

/// 6. CLI は応答の欄から表示言語で組む
fn scan_cli(src: &str, out: &mut Vec<String>) {
    for (head, call) in [
        (CLI_START, "remote_self_check::detail_of_start_in("),
        (CLI_STATUS, "remote_self_check::detail_of_status_in("),
    ] {
        let Some((at, w)) = need(src, CLI_MAIN, head, out) else {
            continue;
        };
        let used = w
            .iter()
            .any(|l| code(l).contains(call) && code(l).contains("lang"));
        if !used {
            out.push(format!(
                "{CLI_MAIN}:{at} — `{}` が自己疎通の接続元検証の断りを表示言語で言っていない（`{call}…, lang)`）",
                head.trim_end_matches(" -> Result<(), String> {")
            ));
        }
    }
}

fn scan_remote(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_probe(src, &mut out);
    scan_classify(src, &mut out);
    scan_daemon(src, &mut out);
    scan_body(src, &mut out);
    scan_status(src, &mut out);
    out
}

fn scan_cli_main(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_cli(src, &mut out);
    out
}

#[test]
fn 自己疎通は接続元検証の403を到達と数えず止めずに理由を言う() {
    let mut failures = scan_remote(&read(REMOTE));
    failures.extend(scan_cli_main(&read(CLI_MAIN)));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn replaced(base: &str, from: &str, to: &str) -> String {
    assert!(
        base.contains(from),
        "注入の置き換え元が見つからない（ソースが変わったなら注入も直す）: {from}"
    );
    base.replacen(from, to, 1)
}

fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("`{needle}` が無い"))
}

fn assert_names(found: &[String], wants: &[String]) {
    let joined = found.join("\n");
    for want in wants {
        assert!(
            joined.contains(want.as_str()),
            "名指せていない: {want}\n---\n{joined}"
        );
    }
}

#[test]
fn 修正前の形と崩れ方を名指しする() {
    let remote = read(REMOTE);
    let cli = read(CLI_MAIN);
    // 本物のソースは通る（以下の注入が空振りでないことの前提）
    assert!(
        scan_remote(&remote).is_empty(),
        "{:?}",
        scan_remote(&remote)
    );
    assert!(scan_cli_main(&cli).is_empty(), "{:?}", scan_cli_main(&cli));

    // 修正前: 403 を本文に依らず到達と数える（状態コードだけの分類）
    let before = replaced(
        &remote,
        "    classify_self_check_reply(result)\n}",
        "    classify_self_check(result.map(|(status, _)| status))\n}",
    );
    assert_names(
        &scan_remote(&before),
        &[
            format!(
                "{REMOTE}:{} — `probe_serve` が 403 の本文で分類していない",
                line_of(&before, PROBE)
            ),
            format!(
                "{REMOTE}:{} — `probe_serve` が状態コードだけで分類している",
                line_of(
                    &before,
                    "classify_self_check(result.map(|(status, _)| status))"
                )
            ),
        ],
    );

    // 崩れ方: 断りで daemon を止める（502 と同じ出口へ寄せる）
    let stops = replaced(
        &remote,
        "        ctx.record_self_check_peer_reject(reject);\n",
        "        ctx.record_self_check_peer_reject(reject);\n        owned.release_serve(\"peer_rejected\");\n",
    );
    assert_names(
        &scan_remote(&stops),
        &[format!(
            "{REMOTE}:{} — `run_daemon` が接続元検証の断りで daemon を止めている（`release_serve(`）",
            line_of(&stops, "owned.release_serve(\"peer_rejected\")")
        )],
    );

    // 崩れ方: 断りを文にして返す（本文に kind / code が付かない）
    let text = replaced(
        &remote,
        "return Err((403, crate::RemoteRefusal::PeerUnverified(reject)));",
        "return Err((403, crate::RemoteRefusal::PeerUnverified(reject).to_string()));",
    );
    assert_names(
        &scan_remote(&text),
        &[format!(
            "{REMOTE}:{} — `forwarded_identity` が接続元検証の断りを型のまま返していない",
            line_of(&text, FORWARDED)
        )],
    );

    // 崩れ方: CLI が start の断りを言わない
    let silent = replaced(
        &cli,
        "    if let Some(warning) = tako_control::remote_self_check::detail_of_start_in(&result, lang) {\n        eprintln!(\"{warning}\");\n    }\n",
        "",
    );
    assert_names(
        &scan_cli_main(&silent),
        &[format!(
            "{CLI_MAIN}:{} — `fn remote_start()` が自己疎通の接続元検証の断りを表示言語で言っていない",
            line_of(&silent, CLI_START)
        )],
    );
}

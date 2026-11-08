//! remote daemon の起動門が画面の読み取りを要件にし、待ち受けの前で判定することの番犬（#2771）
//!
//! # なぜ要るか
//!
//! daemon は tako-app と別プロセスなので、ペインの画面を器（tmux / psmux）から読む。操作は app の IPC へ渡すので、
//! 器へ送る手段（`detached_access`）は要らない。#2771 より前の門は送出まで求めていたので、採取だけの psmux
//! （Windows の器）と器なしでは必ず断り、しかも待ち受けを開いて port ファイルを書いた後で断っていたので、
//! 残った port ファイルが次の start を「stale な待ち受け」にしていた。判定は純関数
//! `remote_backend_gate::check` の 1 実装（3 形の単体テストは `remote_backend_gate::tests`）。
//!
//! # 何を縛るか
//!
//! 1. `run_daemon` は待ち受け（`local_endpoint::bind(`）と port ファイルの書き出しより前に門を通る
//! 2. remote.rs / remote_setup.rs は能力（`detached_access` / `detached_capture`）を直に読まない
//!    （判定が 2 つに割れると、setup が「使える」と言った後に daemon が断る）
//! 3. `tako remote setup` の対話・非対話は、start の案内（対話の最後の行・非対話の QR と `success`）の前に
//!    器の段（`container_step()`）を通り、器の段は門を通る
//!
//! [`修正前の形へ戻すと名指しする`] で、修正前の形の注入が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const SETUP: &str = "crates/tako-control/src/remote_setup.rs";
/// 門の呼び出し（この形で書く）
const GATE: &str = "crate::remote_backend_gate::check_current()";
const RUN_DAEMON: &str = "pub fn run_daemon() -> io::Result<()> {";

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

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
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
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初のコードの行の 1-based 行番号
fn code_line(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| at + i)
}

/// 関数 `func` の中で `first` が `later` のどれよりも前にあるか
fn first_in(
    file: &str,
    src: &str,
    func: &str,
    first: &str,
    later: &[&str],
    why: &str,
    out: &mut Vec<String>,
) {
    let Some((at, w)) = fn_window(src, func) else {
        out.push(format!("{file}:0 — `{func}` が無い（走査の空振り）"));
        return;
    };
    let Some(gate) = code_line(at, &w, first) else {
        out.push(format!(
            "{file}:{at} — `{func}` が `{first}` を通らない。{why}（#2771）"
        ));
        return;
    };
    for l in later {
        match code_line(at, &w, l) {
            Some(line) if line < gate => out.push(format!(
                "{file}:{line} — `{func}` が `{first}` より前に `{l}` へ進む。{why}（#2771）"
            )),
            Some(_) => {}
            None => out.push(format!(
                "{file}:{at} — `{func}` に `{l}` が見つからない（走査の空振り）"
            )),
        }
    }
}

/// 1. daemon の門は待ち受けと port ファイルより前
fn scan_daemon(remote: &str, out: &mut Vec<String>) {
    first_in(
        REMOTE,
        remote,
        RUN_DAEMON,
        GATE,
        &["local_endpoint::bind(", "write_secret_file(&port_path()"],
        "断った後に socket / port ファイルが残り、次の start が「stale な待ち受け」になる",
        out,
    );
}

/// 2. 能力を直に読む行（判定は門の 1 実装）
fn scan_direct_caps(file: &str, src: &str, out: &mut Vec<String>) {
    for (i, line) in src.lines().enumerate() {
        // 能力の欄の読み取りだけを見る（器の採取の入口 `backend().detached_capture()` は呼んでよい）
        let code = line
            .split("//")
            .next()
            .unwrap_or("")
            .replace(".detached_capture(", "");
        if code.contains(".detached_access") || code.contains(".detached_capture") {
            out.push(format!(
                "{file}:{} — 器の能力を直に読んでいる（判定は `remote_backend_gate::check` の 1 実装へ寄せる。#2771）: {}",
                i + 1,
                line.trim()
            ));
        }
    }
}

/// 3. setup は start の案内の前に器の段を通り、器の段は門を通る
fn scan_setup(setup: &str, out: &mut Vec<String>) {
    let why = "器が無いまま start を案内すると daemon の門で必ず断られる";
    first_in(
        SETUP,
        setup,
        "fn container_step() -> Option<SetupStepResult> {",
        GATE,
        &["setup_deps::status_of("],
        "器の段の判定が daemon の門と割れる",
        out,
    );
    first_in(
        SETUP,
        setup,
        "pub fn run_noninteractive(",
        "container_step()",
        &["generate_qr_png(", "result.success = true"],
        why,
        out,
    );
    first_in(
        SETUP,
        setup,
        "pub fn run_interactive(",
        "container_step()",
        &["リモート接続を開始するには"],
        why,
        out,
    );
}

fn scan_all(remote: &str, setup: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_daemon(remote, &mut out);
    scan_direct_caps(REMOTE, remote, &mut out);
    scan_direct_caps(SETUP, setup, &mut out);
    scan_setup(setup, &mut out);
    out
}

#[test]
fn daemonの門は待ち受けの前で画面の読み取りを判定しsetupはstartの前に器を確かめる() {
    let failures = scan_all(&read(REMOTE), &read(SETUP));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 修正前の `run_daemon`（port ファイルの後で送出を直に読む）と、器の段を通らない setup を注入すると名指す
#[test]
fn 修正前の形へ戻すと名指しする() {
    let remote = "\
pub fn run_daemon() -> io::Result<()> {
    ensure_state_dir()?;
    let (server, endpoint) = local_endpoint::bind(&spec)?;
    if let Some(port) = endpoint.port() {
        write_secret_file(&port_path(), &port.to_string())?;
    }
    if !tako_core::backend::capabilities().detached_access {
        return Err(io::Error::other(\"到達手段が無い\"));
    }
    Ok(())
}
";
    let setup = "\
fn container_step() -> Option<SetupStepResult> {
    let state = setup_deps::status_of(container);
    None
}
pub fn run_noninteractive(answers: &A) -> Result<Value, R> {
    let _ = generate_qr_png(&url);
    result.success = true;
    Ok(v)
}
pub fn run_interactive(yes: bool) -> Result<Value, String> {
    writeln!(w, \"リモート接続を開始するには `tako remote start` を実行してください。\");
    Ok(v)
}
";
    let failures = scan_all(remote, setup);
    let joined = failures.join("\n");
    for want in [
        format!("{REMOTE}:1 — `{RUN_DAEMON}` が `{GATE}` を通らない"),
        format!("{REMOTE}:7 — 器の能力を直に読んでいる"),
        format!("{SETUP}:1 — `fn container_step() -> Option<SetupStepResult> {{` が `{GATE}` を通らない"),
        format!("{SETUP}:5 — `pub fn run_noninteractive(` が `container_step()` を通らない"),
        format!("{SETUP}:10 — `pub fn run_interactive(` が `container_step()` を通らない"),
    ] {
        assert!(joined.contains(&want), "名指せていない: {want}\n---\n{joined}");
    }

    // 門を置いても port ファイルの後なら名指す（#2771 の順序の本題）
    let late = remote.replace(
        "    if !tako_core::backend::capabilities().detached_access {\n        return Err(io::Error::other(\"到達手段が無い\"));\n    }\n",
        "    crate::remote_backend_gate::check_current().map_err(|r| io::Error::other(r.to_string()))?;\n",
    );
    let mut out = Vec::new();
    scan_daemon(&late, &mut out);
    let joined = out.join("\n");
    for want in [
        format!("{REMOTE}:3 — `{RUN_DAEMON}` が `{GATE}` より前に `local_endpoint::bind(` へ進む"),
        format!(
            "{REMOTE}:5 — `{RUN_DAEMON}` が `{GATE}` より前に `write_secret_file(&port_path()` へ進む"
        ),
    ] {
        assert!(joined.contains(&want), "名指せていない: {want}\n---\n{joined}");
    }
}

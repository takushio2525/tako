//! remote daemon が断った出口でも、自分が書いた起動の痕跡を回収することの番犬（#2781）
//!
//! # なぜ要るか
//!
//! `run_daemon` は待ち受けを開いた**後**にも断る出口を持つ（Tailscale の検証・serve の設定・デバイスレジストリ・
//! PID / トークンの書き出し・スレッドの起動）。#2781 より前はそれらの出口が自分の書いた port ファイルを残して
//! 返っていたので、次の `tako remote start` が毎回「stale な待ち受け」を言い、そのポートを別のプロセスが
//! 取っていれば起動ごと断っていた（#2771 は器の門を待ち受けの前へ移しただけで、門より後の出口は残った）。
//! いまは待ち受けの直後にガード `OwnedDaemonFiles` を置き、Drop で自分が書いたままのファイルだけを消す。
//! 出口ごとの後片付けではないので、出口を増やしても漏れない。**崩れ方は出口の側ではなくガードの側にある**ので、
//! ここはガードの置き方と書き方を縛る。持ち主の判定の振る舞いは `remote::tests::issue2781_*`、実 daemon の
//! 断りの後に何も残らないことは `crates/tako-cli/tests/issue2781_remote_refused_daemon_leaves_no_files.rs` が見る。
//!
//! # 何を縛るか
//!
//! 1. `run_daemon` は待ち受け（`local_endpoint::bind(` → `Arc::new(server)`）の後にガード
//!    `OwnedDaemonFiles::bound()` を置き、待ち受けとガードの間に断りの出口（`?` / `return`）を置かない
//!    （ガードより前の出口で書いたファイルは回収されない。待ち受けより前に宣言すると、drop が待ち受けを
//!    手放した後になり、port の中身の一致で持ち主と言えなくなる）
//! 2. `run_daemon` は起動の痕跡を直に書かない（`write_secret_file(` / `write_state_file(` / `std::fs::write(`）。
//!    書き出しはすべてガードの `write_secret` を通る（通らない書き出しは回収されない）
//! 3. `run_daemon` は Drop を飛ばす終わり方をしない（`process::exit(` / `mem::forget(`）
//! 4. ガードの Drop は持ち主を確かめて消し（中身の読み戻し）、`cleanup_state_files()` を呼ばない
//!    （呼ぶと後から別の daemon が書いたファイルまで消す）。A/B（`TAKO_2781_LEGACY=1`）を見る
//!
//! [`修正前の形と崩れ方を名指しする`] で、修正前の形と崩れ方の注入が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const RUN_DAEMON: &str = "pub fn run_daemon() -> io::Result<()> {";
const BIND: &str = "local_endpoint::bind(";
const SHARED: &str = "Arc::new(server)";
const GUARD: &str = "OwnedDaemonFiles::bound()";
const DROP_IMPL: &str = "impl Drop for OwnedDaemonFiles {";
const BOUND_FN: &str = "fn bound() -> Self {";

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
fn fn_window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
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
fn find(window: &[String], needle: &str) -> Option<usize> {
    window.iter().position(|l| code(l).contains(needle))
}

/// 1〜3. `run_daemon` の中のガードの置き方と書き方
fn scan_daemon(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = fn_window(src, RUN_DAEMON) else {
        out.push(format!(
            "{REMOTE}:0 — `{RUN_DAEMON}` が無い（走査の空振り）"
        ));
        return;
    };
    let line = |i: usize| at + i;
    let (bind, shared) = (find(&w, BIND), find(&w, SHARED));
    match (bind, shared, find(&w, GUARD)) {
        (None, _, _) | (_, None, _) => out.push(format!(
            "{REMOTE}:{at} — `run_daemon` に `{BIND}` / `{SHARED}` が見つからない（走査の空振り）"
        )),
        (_, _, None) => out.push(format!(
            "{REMOTE}:{at} — `run_daemon` が待ち受けの後にガード `{GUARD}` を置いていない。\
             待ち受けの後の断りの出口（Tailscale の不足ほか）が port ファイルを残し、\
             次の start が「stale な待ち受け」を毎回言う（#2781）"
        )),
        (Some(b), Some(s), Some(g)) => {
            if g < b || g < s {
                out.push(format!(
                    "{REMOTE}:{} — ガード `{GUARD}` を待ち受け（`{BIND}` / `{SHARED}`）より前に宣言している。\
                     drop が待ち受けを手放した後になり、port の中身の一致で持ち主と言えなくなる（#2781）",
                    line(g)
                ));
            } else {
                for (i, l) in w.iter().enumerate().take(g).skip(b + 1) {
                    let c = code(l);
                    if c.contains('?') || c.contains("return") {
                        out.push(format!(
                            "{REMOTE}:{} — 待ち受けとガード `{GUARD}` の間に断りの出口がある。\
                             ここで断ると回収されない（#2781）: {}",
                            line(i),
                            l.trim()
                        ));
                    }
                }
            }
        }
    }
    for (i, l) in w.iter().enumerate() {
        let c = code(l);
        for direct in ["write_secret_file(", "write_state_file(", "std::fs::write("] {
            if c.contains(direct) {
                out.push(format!(
                    "{REMOTE}:{} — `run_daemon` が起動の痕跡を直に書いている（`{direct}`）。\
                     ガードの `owned.write_secret(` を通さない書き出しは、断った出口で回収されない（#2781）",
                    line(i)
                ));
            }
        }
        for skip in ["process::exit(", "mem::forget("] {
            if c.contains(skip) {
                out.push(format!(
                    "{REMOTE}:{} — `run_daemon` が Drop を飛ばす終わり方をしている（`{skip}`）。\
                     ガードが走らず起動の痕跡が残る（#2781）",
                    line(i)
                ));
            }
        }
    }
}

/// 4. ガードの Drop と A/B
fn scan_guard(src: &str, out: &mut Vec<String>) {
    match fn_window(src, DROP_IMPL) {
        None => out.push(format!(
            "{REMOTE}:0 — `{DROP_IMPL}` が無い（ガードが何も回収しない。#2781）"
        )),
        Some((at, w)) => {
            if let Some(i) = find(&w, "cleanup_state_files(") {
                out.push(format!(
                    "{REMOTE}:{} — ガードの Drop が `cleanup_state_files()` を呼んでいる。\
                     持ち主を確かめずに消すので、後から別の daemon が書いたファイルまで消す（#2781）",
                    at + i
                ));
            }
            if find(&w, "read_to_string(").is_none() {
                out.push(format!(
                    "{REMOTE}:{at} — ガードの Drop が中身を読み戻していない。\
                     自分が書いたままかを確かめずに消している（#2781）"
                ));
            }
            if find(&w, "self.legacy").is_none() {
                out.push(format!(
                    "{REMOTE}:{at} — ガードの Drop が A/B（`TAKO_2781_LEGACY=1`）を見ていない（#2781）"
                ));
            }
        }
    }
    match fn_window(src, BOUND_FN) {
        Some((at, w)) if find(&w, "OWNED_FILES_LEGACY_ENV").is_none() => out.push(format!(
            "{REMOTE}:{at} — ガードの `bound()` が A/B の env（`OWNED_FILES_LEGACY_ENV`）を読んでいない（#2781）"
        )),
        Some(_) => {}
        None => out.push(format!(
            "{REMOTE}:0 — ガードの `{BOUND_FN}` が無い（走査の空振り）"
        )),
    }
}

fn scan_all(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_daemon(src, &mut out);
    scan_guard(src, &mut out);
    out
}

#[test]
fn daemonは待ち受けの直後にガードを置き起動の痕跡をガード経由で書く() {
    let failures = scan_all(&read(REMOTE));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 正しい形（行番号は 1 始まり）。注入はここからの置き換えで作る
const GOOD: &str = "\
pub fn run_daemon() -> io::Result<()> {
    let (server, endpoint) = local_endpoint::bind(&spec)?;
    let server = Arc::new(server);
    let mut owned = OwnedDaemonFiles::bound();
    if let Some(port) = endpoint.port() {
        owned.write_secret(&port_path(), &port.to_string())?;
    }
    let (serve_handle, base_url) = establish_tailscale_serve(&endpoint)?;
    owned.write_secret(&pid_path(), &pid_info)?;
    Ok(())
}
impl OwnedDaemonFiles {
    fn bound() -> Self {
        Self::new(std::env::var_os(OWNED_FILES_LEGACY_ENV).is_some(), pid_path(), vec![])
    }
}
impl Drop for OwnedDaemonFiles {
    fn drop(&mut self) {
        if self.legacy {
            return;
        }
        let still_ours = |p: &Path, c: &str| std::fs::read_to_string(p).is_ok_and(|n| n == c);
    }
}
";

fn replaced(from: &str, to: &str) -> String {
    assert!(GOOD.contains(from), "注入の元が無い: {from}");
    GOOD.replacen(from, to, 1)
}

fn assert_names(src: &str, wants: &[String]) {
    let joined = scan_all(src).join("\n");
    for want in wants {
        assert!(
            joined.contains(want.as_str()),
            "名指せていない: {want}\n---\n{joined}"
        );
    }
}

#[test]
fn 修正前の形と崩れ方を名指しする() {
    // 正しい形は通る（以下の注入が空振りでないことの前提）
    assert!(scan_all(GOOD).is_empty(), "{:?}", scan_all(GOOD));

    // 修正前: ガードが無く、port ファイルを直に書く
    let before = replaced("    let mut owned = OwnedDaemonFiles::bound();\n", "")
        .replace("owned.write_secret(", "write_secret_file(");
    assert_names(
        &before,
        &[
            format!("{REMOTE}:1 — `run_daemon` が待ち受けの後にガード `{GUARD}` を置いていない"),
            format!(
                "{REMOTE}:5 — `run_daemon` が起動の痕跡を直に書いている（`write_secret_file(`）"
            ),
        ],
    );

    // ガードを断りの出口の後へ置く
    let late = replaced(
        "    let mut owned = OwnedDaemonFiles::bound();\n",
        "    ensure_ready()?;\n    let mut owned = OwnedDaemonFiles::bound();\n",
    );
    assert_names(
        &late,
        &[format!(
            "{REMOTE}:4 — 待ち受けとガード `{GUARD}` の間に断りの出口がある"
        )],
    );

    // ガードを待ち受けより前に宣言する（drop が待ち受けの後になる）
    let early = replaced(
        "    let (server, endpoint) = local_endpoint::bind(&spec)?;\n    let server = Arc::new(server);\n    let mut owned = OwnedDaemonFiles::bound();\n",
        "    let mut owned = OwnedDaemonFiles::bound();\n    let (server, endpoint) = local_endpoint::bind(&spec)?;\n    let server = Arc::new(server);\n",
    );
    assert_names(
        &early,
        &[format!(
            "{REMOTE}:2 — ガード `{GUARD}` を待ち受け（`{BIND}` / `{SHARED}`）より前に宣言している"
        )],
    );

    // Drop を飛ばす終わり方
    let exit = replaced("    Ok(())\n}\nimpl", "    std::process::exit(1);\n}\nimpl");
    assert_names(
        &exit,
        &[format!(
            "{REMOTE}:10 — `run_daemon` が Drop を飛ばす終わり方をしている（`process::exit(`）"
        )],
    );

    // Drop が持ち主を確かめずに消し、A/B を見ない
    let blind = replaced(
        "        if self.legacy {\n            return;\n        }\n        let still_ours = |p: &Path, c: &str| std::fs::read_to_string(p).is_ok_and(|n| n == c);\n",
        "        cleanup_state_files();\n",
    )
    .replace(
        "std::env::var_os(OWNED_FILES_LEGACY_ENV).is_some()",
        "false",
    );
    assert_names(
        &blind,
        &[
            format!("{REMOTE}:19 — ガードの Drop が `cleanup_state_files()` を呼んでいる"),
            format!("{REMOTE}:17 — ガードの Drop が中身を読み戻していない"),
            format!("{REMOTE}:17 — ガードの Drop が A/B（`TAKO_2781_LEGACY=1`）を見ていない"),
            format!("{REMOTE}:13 — ガードの `bound()` が A/B の env（`OWNED_FILES_LEGACY_ENV`）を読んでいない"),
        ],
    );
}

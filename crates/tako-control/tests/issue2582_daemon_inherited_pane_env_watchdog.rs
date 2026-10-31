//! ペインの中から CLI で起こした remote daemon が、そのペインの env を持ったまま常駐しないことの番犬（#2582）
//!
//! # なぜ要るか
//!
//! #2570 で GUI は起動の最初にペインの中でだけ意味を持つ env（正本 `tako_core::host_env::STRIP`）を
//! 外すようになったが、ペインの中から CLI（`tako remote start`）で起こした remote daemon は CLI の env を
//! 丸ごと受け継ぎ、ペインの識別（`TAKO_PANE_ID`）・起こした GUI への接続（`TAKO_SOCKET` / `TAKO_TOKEN`）・
//! claude のセッションの印・`GIT_EDITOR=true` を持ったまま常駐していた（`ps eww` と資源の走査はそのペインの
//! プロセスとして数え、daemon が起こす子へも素通し）。daemon は CLI の `main` の最初で
//! `tako_control::remote::strip_inherited_env_at_startup` を通して外す。
//!
//! # 何を縛るか
//!
//! 1. CLI の `main` が、器の段（`exec_container_child_if_requested`）の**後**・本体のスレッド
//!    （`std::thread::Builder::new()`）の**前**に、daemon の起動（`remote::is_serve_invocation`）のときだけ
//!    `remote::strip_inherited_env_at_startup()` を呼ぶ（普段の CLI は呼んだペインの env を使うので外さない）
//! 2. `strip_inherited_env_at_startup` が GUI と同じ正本で外し（`host_env::apply_at_startup_for` に
//!    `DAEMON_LEGACY_ENV`）、続けて同じ判断で起こし直す（`host_env::reexec_if_stripped(&applied)`）
//! 3. `run_daemon` が state dir を確保した直後・待ち受けの前に、外した名前を audit.log へ残す
//!    （`audit_startup_env()`）
//! 4. daemon を起こす引数は正本 `SERVE_ARGS` だけ（`"remote", "serve"` を別に書くと、1 の門
//!    `is_serve_invocation` と食い違って外さない daemon ができる）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓を固定し、[`修正前の形へ戻すと名指しする`] で
//! **修正前を再現した注入**が `file:line` で名指しされることを確かめる。daemon のプロセスの env が
//! 実際に外れることは `crates/tako-cli/tests/issue2582_daemon_startup_env.rs`（実プロセス）が縛る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";
const REMOTE: &str = "crates/tako-control/src/remote.rs";
/// daemon を起こす引数の直書きを探す置き場（4）
const SPAWN_DIRS: [&str; 3] = [
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_raw(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: String,
    line: usize,
    why: String,
}

impl Offender {
    fn new(file: &str, line: usize, why: impl Into<String>) -> Self {
        Self {
            file: file.to_string(),
            line,
            why: why.into(),
        }
    }

    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
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

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 窓の中で `needle` を含む最初の**コードの**行の位置（0-based）
fn code_pos(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !is_comment(l) && l.contains(needle))
}

/// 窓の `pos` 行目を含む文の頭の字下げ（rustfmt が `let x =` の後で折った続きの行は、折る前の行で見る）
fn indent_at(window: &str, pos: usize) -> usize {
    let lines: Vec<&str> = window.lines().collect();
    let mut head = pos;
    while head > 0
        && lines
            .get(head - 1)
            .is_some_and(|l| l.trim_end().ends_with('='))
    {
        head -= 1;
    }
    lines
        .get(head)
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or_default()
}

fn missing(file: &str, needle: &str) -> Offender {
    Offender::new(
        file,
        0,
        format!("`{needle}` が見つからない（走査の空振り）"),
    )
}

struct Sources {
    cli_main: String,
    remote: String,
    /// 4 の走査対象（相対パスと本番コードだけの眺め）
    spawners: Vec<(String, String)>,
}

fn rust_files(root: &Path, dir: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = format!("{dir}/{name}");
        if entry.path().is_dir() {
            rust_files(root, &rel, out);
        } else if name.ends_with(".rs") {
            out.push(rel);
        }
    }
}

fn sources(root: &Path) -> Sources {
    let mut files = Vec::new();
    for dir in SPAWN_DIRS {
        rust_files(root, dir, &mut files);
    }
    let spawners = files
        .into_iter()
        .map(|rel| {
            // ほぼテストだけのファイル（`limit_stop.rs` ほか）も読むので、下限の検査は付けない
            // （下限で縮みを見張るのは中身を読む 1〜3 の remote.rs と CLI の main）
            let view = production_range::scan(&read_raw(root, &rel)).text;
            (rel, view)
        })
        .collect();
    Sources {
        cli_main: production_range::production(&read_raw(root, CLI_MAIN), CLI_MAIN),
        remote: production_range::production(&read_raw(root, REMOTE), REMOTE),
        spawners,
    }
}

/// 1. CLI の `main` の中の置き場
fn scan_cli_main(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(main, "fn main() -> ExitCode {") else {
        return vec![missing(CLI_MAIN, "fn main() -> ExitCode {")];
    };
    let line_of = |pos: usize| at + pos;
    let Some(exec) = code_pos(&window, "exec_container_child_if_requested()") else {
        return vec![missing(CLI_MAIN, "exec_container_child_if_requested()")];
    };
    let Some(thread) = code_pos(&window, "std::thread::Builder::new()") else {
        return vec![missing(CLI_MAIN, "std::thread::Builder::new()")];
    };
    let Some(strip) = code_pos(&window, "remote::strip_inherited_env_at_startup()") else {
        out.push(Offender::new(
            CLI_MAIN,
            at,
            "CLI の `main` が daemon の起動のときに `tako_control::remote::strip_inherited_env_at_startup()` を\
             呼んでいない。ペインの中から `tako remote start` で起こした daemon が、そのペインの env\
             （TAKO_PANE_ID・TAKO_SOCKET・claude のセッションの印・GIT_EDITOR=true ほか）を持ったまま常駐する（#2582）",
        ));
        return out;
    };
    if strip < exec {
        out.push(Offender::new(
            CLI_MAIN,
            line_of(strip),
            "daemon の env を外すのが器の段（`exec_container_child_if_requested`）より前にある。段は器の\
             ペインの中で起こされ、ペインの env をシェルへ渡す（#2582）",
        ));
    }
    if strip > thread {
        out.push(Offender::new(
            CLI_MAIN,
            line_of(strip),
            "daemon の env を外すのが本体のスレッド（`std::thread::Builder::new()`）より後にある。env の\
             書き換えは別スレッドの読みと競り、exec し直しはほかのスレッドを残さない（#2582）",
        ));
    }
    // 門: 直前のコードの行が `if …remote::is_serve_invocation(std::env::args_os().skip(1)) {`（main の本文 = 字下げ 4）
    let lines: Vec<&str> = window.lines().collect();
    let gate = lines[..strip]
        .iter()
        .rposition(|l| !is_comment(l) && !l.trim().is_empty());
    let gated = gate.is_some_and(|g| {
        let l = lines[g];
        l.contains("remote::is_serve_invocation(std::env::args_os().skip(1))")
            && l.trim_start().starts_with("if ")
            && l.trim_end().ends_with('{')
            && indent_at(&window, g) == 4
    });
    if !gated {
        out.push(Offender::new(
            CLI_MAIN,
            line_of(strip),
            "daemon の env を外すのが `if tako_control::remote::is_serve_invocation(std::env::args_os().skip(1)) {` の\
             直下に無い。門の外で外すと普段の CLI が呼んだペインの TAKO_PANE_ID / TAKO_SOCKET を失い、別の門の中\
             （`if bridge` ほか）では daemon の起動で外さない（#2582）",
        ));
    }
    out
}

/// 2. `strip_inherited_env_at_startup` が GUI と同じ正本で外し、同じ判断で起こし直す
fn scan_strip_fn(remote: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(remote, "pub fn strip_inherited_env_at_startup() {") else {
        return vec![missing(REMOTE, "pub fn strip_inherited_env_at_startup() {")];
    };
    let line_of = |pos: usize| at + pos;
    let Some(apply) = code_pos(&window, "host_env::apply_at_startup_for(") else {
        out.push(Offender::new(
            REMOTE,
            at,
            "daemon が `tako_core::host_env::apply_at_startup_for(` で外していない（外す名前の正本は GUI と同じ \
             `host_env::STRIP`。#2582）",
        ));
        return out;
    };
    let call: String = window
        .lines()
        .skip(apply)
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    if !call.contains("DAEMON_LEGACY_ENV") {
        out.push(Offender::new(
            REMOTE,
            line_of(apply),
            "daemon の外す処理が A/B の入口 `host_env::DAEMON_LEGACY_ENV`（TAKO_2582_LEGACY）を見ていない（#2582）",
        ));
    }
    if indent_at(&window, apply) != 4 {
        out.push(Offender::new(
            REMOTE,
            line_of(apply),
            "daemon の env を外すのが条件の中にある（起動の形によって外さない道ができる。#2582）",
        ));
    }
    match code_pos(&window, "host_env::reexec_if_stripped(&applied)") {
        None => out.push(Offender::new(
            REMOTE,
            line_of(apply),
            "外した後に `host_env::reexec_if_stripped(&applied)` で起こし直していない。exec の時点の env の塊\
             （ps eww・資源の走査）に起こしたペインの TAKO_PANE_ID が残り、daemon がそのペインのプロセスとして\
             数えられる（#2582）",
        )),
        Some(reexec) if reexec < apply || indent_at(&window, reexec) != 4 => out.push(Offender::new(
            REMOTE,
            line_of(reexec),
            "起こし直しは外した直後に条件なしで置く（GUI と同じ判断 = `wants_reexec`。#2582）",
        )),
        Some(_) => {}
    }
    out
}

/// 3. `run_daemon` が外した名前を audit.log へ残す（state dir の確保の後・待ち受けの前）
fn scan_audit(remote: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(remote, "pub fn run_daemon() -> io::Result<()> {") else {
        return vec![missing(REMOTE, "pub fn run_daemon() -> io::Result<()> {")];
    };
    let Some(state) = code_pos(&window, "ensure_state_dir()?") else {
        return vec![missing(REMOTE, "ensure_state_dir()?")];
    };
    let Some(bind) = code_pos(&window, "local_endpoint::bind(") else {
        return vec![missing(REMOTE, "local_endpoint::bind(")];
    };
    match code_pos(&window, "audit_startup_env()") {
        None => vec![Offender::new(
            REMOTE,
            at,
            "`run_daemon` が起動の最初に外した env の名前を audit.log へ残していない（`audit_startup_env()`。#2582）",
        )],
        Some(audit) if audit < state || audit > bind || indent_at(&window, audit) != 4 => {
            vec![Offender::new(
                REMOTE,
                at + audit,
                "外した名前の audit.log の 1 行は state dir の確保（`ensure_state_dir()?`）の後・待ち受けの前に\
                 条件なしで書く（前だと置き場が無く、後だと起動に失敗した daemon が何も残さない。#2582）",
            )]
        }
        Some(_) => Vec::new(),
    }
}

/// 4. daemon を起こす引数は正本 `SERVE_ARGS` だけ
fn scan_spawn_args(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(
        &s.remote,
        "pub fn spawn_daemon() -> Result<Value, String> {",
    ) {
        None => out.push(missing(
            REMOTE,
            "pub fn spawn_daemon() -> Result<Value, String> {",
        )),
        Some((at, window)) => {
            if code_pos(&window, "cmd.args(SERVE_ARGS)").is_none() {
                out.push(Offender::new(
                    REMOTE,
                    at,
                    "`spawn_daemon` が daemon を正本 `SERVE_ARGS` で起こしていない（CLI の `main` の門 \
                     `is_serve_invocation` と食い違うと外さない daemon ができる。#2582）",
                ));
            }
        }
    }
    let literal = ["\"remote\", \"serve\"", "\"remote\".to_string(), \"serve\""];
    for (rel, view) in &s.spawners {
        // remote.rs と CLI の main は 1〜3 と同じ眺めで読む（注入が両方へ届く）
        let view = match rel.as_str() {
            REMOTE => &s.remote,
            CLI_MAIN => &s.cli_main,
            _ => view,
        };
        for (i, l) in view.lines().enumerate() {
            if is_comment(l) || l.contains("pub const SERVE_ARGS") {
                continue;
            }
            if literal.iter().any(|lit| l.contains(lit)) {
                out.push(Offender::new(
                    rel,
                    i + 1,
                    "daemon（`tako remote serve`）を起こす引数を直書きしている。`tako_control::remote::SERVE_ARGS` を\
                     使う（CLI の `main` の門と食い違うと、ペインの env を外さない daemon ができる。#2582）",
                ));
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(scan_cli_main(&s.cli_main));
    out.extend(scan_strip_fn(&s.remote));
    out.extend(scan_audit(&s.remote));
    out.extend(scan_spawn_args(s));
    out.iter().map(Offender::report).collect()
}

#[test]
fn ペインの中から起こしたdaemonがペインのenvを持ち込まない配線が揃っている() {
    let s = sources(&workspace_root());
    let found = all(&s);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for needle in [
        "fn main() -> ExitCode {",
        "exec_container_child_if_requested()",
        "std::thread::Builder::new()",
        "remote::is_serve_invocation(std::env::args_os().skip(1))",
        "remote::strip_inherited_env_at_startup()",
    ] {
        assert!(
            s.cli_main.contains(needle),
            "{CLI_MAIN} に `{needle}` が無い"
        );
    }
    for needle in [
        "pub fn strip_inherited_env_at_startup() {",
        "host_env::apply_at_startup_for(",
        "host_env::reexec_if_stripped(&applied)",
        "pub fn run_daemon() -> io::Result<()> {",
        "audit_startup_env()",
        "pub fn spawn_daemon() -> Result<Value, String> {",
        "cmd.args(SERVE_ARGS)",
        "pub const SERVE_ARGS",
    ] {
        assert!(s.remote.contains(needle), "{REMOTE} に `{needle}` が無い");
    }
    // 4 の走査は 3 つの crate の本番コードを読んでいる（remote.rs と CLI の main を含む）
    assert!(
        s.spawners.len() >= 50,
        "走査したファイルが少なすぎる: {}",
        s.spawners.len()
    );
    for want in [REMOTE, CLI_MAIN, "crates/tako-app/src/main.rs"] {
        assert!(
            s.spawners.iter().any(|(rel, _)| rel == want),
            "{want} を走査していない"
        );
    }
}

/// 注入を当てる
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    let out = base.replacen(from, to, 1);
    assert!(out != base, "注入「{what}」の対象が見つからない: {from:?}");
    out
}

/// `needle` を含む最初の行（1-based）
fn first_line(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("`{needle}` が無い"))
}

fn assert_named(found: &[String], want: &str, what: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(&format!("{want} "))),
        "注入「{what}」が `{want}` で名指しされない: {found:#?}"
    );
}

fn line_with(src: &str, needle: &str) -> String {
    src.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` の行が無い"))
        .to_string()
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let base = sources(&workspace_root());
    assert!(all(&base).is_empty(), "注入の前は緑であること");
    let strip_call = line_with(&base.cli_main, "remote::strip_inherited_env_at_startup()");

    // 注入 1: #2582 前の CLI の main（daemon の起動でも外さない）
    let mut s = sources(&workspace_root());
    s.cli_main = inject(
        &s.cli_main,
        &strip_call,
        "        // 注入: 外さない",
        "外さない main",
    );
    assert_named(
        &all(&s),
        &format!(
            "{CLI_MAIN}:{}",
            first_line(&s.cli_main, "fn main() -> ExitCode {")
        ),
        "外さない main",
    );

    // 注入 2: 門の外で外す（普段の CLI まで呼んだペインの env を失う）
    let mut s = sources(&workspace_root());
    s.cli_main = inject(
        &s.cli_main,
        &format!("{strip_call}\n"),
        "",
        "門の中の行を抜く",
    );
    s.cli_main = inject(
        &s.cli_main,
        "    tako_core::stdio::stop_quietly_on_closed_stdout();",
        &format!(
            "    tako_core::stdio::stop_quietly_on_closed_stdout();\n    {}",
            strip_call.trim_start()
        ),
        "門の外で外す",
    );
    assert_named(
        &all(&s),
        &format!(
            "{CLI_MAIN}:{}",
            first_line(&s.cli_main, "remote::strip_inherited_env_at_startup()")
        ),
        "門の外で外す",
    );

    // 注入 3: 本体のスレッドの中（cli_main）で外す
    let mut s = sources(&workspace_root());
    s.cli_main = inject(
        &s.cli_main,
        &format!("{strip_call}\n"),
        "",
        "門の中の行を抜く",
    );
    s.cli_main = inject(
        &s.cli_main,
        "        .join();",
        &format!("        .join();\n{strip_call}"),
        "スレッドの後で外す",
    );
    assert_named(
        &all(&s),
        &format!(
            "{CLI_MAIN}:{}",
            first_line(&s.cli_main, "remote::strip_inherited_env_at_startup()")
        ),
        "スレッドの後で外す",
    );

    // 注入 4: 外すだけで起こし直さない（ps eww と資源の走査に起こしたペインが残る）
    let mut s = sources(&workspace_root());
    let reexec = line_with(&s.remote, "host_env::reexec_if_stripped(&applied)");
    s.remote = inject(
        &s.remote,
        &reexec,
        "    let exec_error: Option<String> = None; // 注入",
        "起こし直さない",
    );
    assert_named(
        &all(&s),
        &format!(
            "{REMOTE}:{}",
            first_line(&s.remote, "host_env::apply_at_startup_for(")
        ),
        "起こし直さない",
    );

    // 注入 5: A/B の入口を見ない
    let mut s = sources(&workspace_root());
    s.remote = inject(
        &s.remote,
        "host_env::DAEMON_LEGACY_ENV, ",
        "",
        "A/B を見ない",
    );
    assert_named(
        &all(&s),
        &format!(
            "{REMOTE}:{}",
            first_line(&s.remote, "host_env::apply_at_startup_for(")
        ),
        "A/B を見ない",
    );

    // 注入 6: audit.log へ残さない
    let mut s = sources(&workspace_root());
    let audit = line_with(&s.remote, "    audit_startup_env();");
    s.remote = inject(&s.remote, &format!("{audit}\n"), "", "残さない");
    assert_named(
        &all(&s),
        &format!(
            "{REMOTE}:{}",
            first_line(&s.remote, "pub fn run_daemon() -> io::Result<()> {")
        ),
        "残さない",
    );

    // 注入 7: daemon を直書きの引数で起こす（#2582 前の spawn_daemon の形）
    let mut s = sources(&workspace_root());
    s.remote = inject(
        &s.remote,
        "cmd.args(SERVE_ARGS)",
        "cmd.args([\"remote\", \"serve\"])",
        "直書きの引数",
    );
    assert_named(
        &all(&s),
        &format!(
            "{REMOTE}:{}",
            first_line(&s.remote, "cmd.args([\"remote\", \"serve\"])")
        ),
        "直書きの引数",
    );

    // 注入 8: ほかの置き場が daemon を直書きの引数で起こす（GUI の自動復帰の口に足した形）
    let mut s = sources(&workspace_root());
    let autostart = "crates/tako-control/src/remote_autostart.rs";
    let (_, view) = s
        .spawners
        .iter_mut()
        .find(|(rel, _)| rel == autostart)
        .expect("remote_autostart.rs を走査している");
    view.push_str("\nfn injected() { let _ = std::process::Command::new(\"tako\").args([\"remote\", \"serve\"]); }\n");
    let line = view.lines().count();
    assert_named(
        &all(&s),
        &format!("{autostart}:{line}"),
        "ほかの置き場の直書き",
    );
}

//! UI スレッドから届く子プロセスの待ちが**上限なし**に戻らないかの番犬（#1979）
//!
//! # 何が起きたか
//!
//! 2026-10-10 01:20、本番の GUI が 10 分固まった（CPU 0%・IPC の受け口は backlog で
//! 接続拒否・KILL で再起動）。`sample` のメインスレッドをシンボル付きの同じ版と
//! 命令列で突き合わせると、止まっていたのはこの経路だった:
//!
//! ```text
//! IPC 受信ループ（gpui の spawn）→ prepare_offload → collect_worker_status_ctx
//!   → has_running_children → process_parent_map → capture_process_table
//!   → Command::output()（`ps -axo pid=,ppid=,command=`）→ read_output → poll(-1)
//! ```
//!
//! #1968 が UI スレッド部から外した経路そのもの（番犬
//! `issue1968_ui_thread_subprocess_watchdog.rs` が呼び出しを縛る）。ここはその**型**を縛る:
//! `Command::output()` は子が終わらないと永久に返らず、macOS の std はパイプを
//! `pipe()` → `set_cloexec` の 2 手で作るので、別スレッドが同時に起こした長生きの子
//! （器のサーバー・ペインのシェル）へ書き手が漏れると、**子が終わっても** EOF が来ない
//! （#1768 で同じ穴を実測）。どの子プロセスの待ちも「速いはず」では安全にならない。
//!
//! # 何を縛るか
//!
//! 1. 器への問い合わせ・親子表の採取・`claude agents --json` の走査の 1 実装（[`PRIMITIVES`]）に上限なしの待ち
//!    （`.output()` / `.wait()` / `.status()` / `.wait_with_output()`）が無い。
//!    例外は A/B（`TAKO_1979_LEGACY=1`）の再現アームだけで、ゲートの内側にあること
//! 2. 上限つきの待ち（`probe::wait_with_timeout`）が、打ち切った子を同期に `wait` しない
//!    （カーネルの中で止まった子は SIGKILL でも出てくるまで終わらない）
//! 3. 親子表の採取は macOS で libproc（子プロセスなし）を先に使い、落ち先の `ps` は上限つき
//! 4. `tako list` の器の採り直し（`list-windows -a`）を UI スレッドで待たない:
//!    `prepare_offload` が background のジョブにし、UI スレッドの判断・反映・応答の
//!    組み立ては tmux を起こさない
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const TMUX: &str = "crates/tako-core/src/tmux.rs";
const BACKEND_TMUX: &str = "crates/tako-core/src/backend/tmux.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";
/// `claude agents --json` の走査（スマホの一覧・セッション再起動の計画から同期の dispatch で届く）
const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const PROBE: &str = "crates/tako-core/src/probe.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// 器への問い合わせ・親子表の採取・エージェント一覧の走査の 1 実装
/// （UI スレッドの dispatch・2 秒 tick・監視から届く）
const PRIMITIVES: &[&str] = &[TMUX, BACKEND_TMUX, AGENTS, ORCH];

/// 上限なしの待ち（右は名指しの理由）
const UNBOUNDED_WAITS: &[(&str, &str)] = &[
    (
        ".output()",
        "`Command::output()` は子が終わらないと返らない",
    ),
    (
        ".wait_with_output()",
        "`wait_with_output()` は子が終わらないと返らない",
    ),
    (".wait()", "`Child::wait()` は子が終わらないと返らない"),
    (
        ".status()",
        "`Command::status()` は子が終わらないと返らない",
    ),
];

/// A/B の再現アーム（**修正前の上限なしの待ちをわざと残す**関数）。
/// ここだけは待ちを許すが、呼び出しがゲート（`issue1979_legacy()`）の内側にあることを別に見る
const LEGACY_ARMS: &[(&str, &str)] =
    &[(TMUX, "run_bounded"), (AGENTS, "ps_table_unbounded_legacy")];

/// `tako list` の UI スレッド側で tmux・親子表の採取を起こす口（右は理由）
const FORBIDDEN_ON_UI: &[(&str, &str)] = &[
    (
        "list_windows_by_session(",
        "`tmux list-windows -a` を起こす",
    ),
    (
        "refresh_backend_windows",
        "同期の採り直し（`tmux list-windows -a`）",
    ),
    ("run_tmux(", "tmux を起こす"),
    ("tmux::query(", "tmux を起こす"),
    ("Command::new(", "子プロセスを起こす"),
    ("process_parent_map(", "親子表の採取"),
    ("capture_process_table(", "親子表の採取"),
    (".output()", "子プロセスを上限なしで待つ"),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）。
/// tmux.rs はテストが厚いので下限つきの `production` ではなく素の `scan` を使う
fn read(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `fn 名前(` の名前（字下げ・`pub` / `pub(crate)` / `async` を問わない）
fn fn_name(line: &str) -> Option<&str> {
    if is_comment(line) {
        return None;
    }
    let at = line.find("fn ")?;
    // `fn` の前は空白か修飾子だけ（文字列中の "fn " を拾わない）
    let head = line[..at].trim();
    if !head.split_whitespace().all(|w| {
        matches!(
            w,
            "pub" | "pub(crate)" | "pub(super)" | "async" | "const" | "unsafe"
        )
    }) {
        return None;
    }
    let rest = &line[at + 3..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    (end > 0 && rest[end..].starts_with(['(', '<'])).then(|| &rest[..end])
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(needle))?;
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

/// 窓の中で `needle` を含む**コードの**行（0-based の相対位置）
fn code_lines_with(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

/// 1. 1 実装に上限なしの待ちが無い（A/B の再現アームはゲートの内側だけ）
fn scan_primitive(file: &'static str, src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = fn_name(line) {
            current = Some(name.to_string());
        }
        if is_comment(line) {
            continue;
        }
        for (needle, why) in UNBOUNDED_WAITS {
            if !line.contains(needle) {
                continue;
            }
            let in_legacy_arm = current.as_deref().is_some_and(|f| {
                LEGACY_ARMS
                    .iter()
                    .any(|(arm_file, arm)| *arm_file == file && *arm == f)
            });
            if in_legacy_arm {
                continue;
            }
            out.push(Offender {
                file,
                line: i + 1,
                why: format!(
                    "`fn {}` が上限なしの待ち `{needle}` を使っている（{why}。UI スレッドの \
                     dispatch・2 秒 tick・監視から届く 1 実装なので、ここで止まると窓ごと固まる = \
                     #1979。`probe::command_output_with_timeout` / `tmux::run_tmux` を通すこと）",
                    current.as_deref().unwrap_or("(トップレベル)")
                ),
            });
        }
    }
    out
}

/// 1 の続き: A/B の再現アームがゲートの内側からしか呼ばれず、新しい形の上限つきの待ちが在る
fn scan_legacy_gates(tmux: &str, agents: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(tmux, "fn run_bounded(") {
        None => out.push(Offender {
            file: TMUX,
            line: 0,
            why: "`fn run_bounded` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let gate = code_lines_with(&window, "issue1979_legacy()");
            let raw = code_lines_with(&window, ".output()");
            let bounded = code_lines_with(&window, "command_output_with_timeout(");
            if bounded.is_empty() {
                out.push(Offender {
                    file: TMUX,
                    line: at,
                    why:
                        "`run_bounded` に上限つきの待ち（`probe::command_output_with_timeout`）が \
                          無い（器への問い合わせが全部上限なしに戻る = #1979）"
                            .into(),
                });
            }
            // 上限なしの `.output()` はゲートの後ろ・上限つきの前（= ゲートの腕の中）だけ
            for r in raw {
                let gated =
                    gate.first().is_some_and(|g| *g < r) && bounded.first().is_some_and(|b| r < *b);
                if !gated {
                    out.push(Offender {
                        file: TMUX,
                        line: at + r,
                        why: "`run_bounded` の上限なしの `.output()` が A/B のゲート \
                              （`issue1979_legacy()`）の腕の外にある（製品の既定で上限が消える）"
                            .into(),
                    });
                }
            }
        }
    }
    // `ps_table_unbounded_legacy` の呼び出しは、直前 2 行以内にゲートがあること
    let lines: Vec<&str> = agents.lines().collect();
    let mut calls = 0;
    for (i, line) in lines.iter().enumerate() {
        if is_comment(line)
            || !line.contains("ps_table_unbounded_legacy(")
            || line.contains("fn ps_table_unbounded_legacy(")
        {
            continue;
        }
        calls += 1;
        let from = i.saturating_sub(2);
        if !lines[from..i]
            .iter()
            .any(|l| !is_comment(l) && l.contains("issue1979_legacy()"))
        {
            out.push(Offender {
                file: AGENTS,
                line: i + 1,
                why: "上限なしの `ps`（`ps_table_unbounded_legacy`）が A/B のゲートの外から \
                      呼ばれている（製品の既定で `ps` の `output()` に戻る = 本番ハングの形）"
                    .into(),
            });
        }
    }
    if calls == 0 {
        out.push(Offender {
            file: AGENTS,
            line: 0,
            why:
                "`ps_table_unbounded_legacy` の呼び出しが見つからない（走査が空振り。改名したなら \
                  番犬も追うこと）"
                    .into(),
        });
    }
    out
}

/// 2. 打ち切った子を同期に `wait` しない
fn scan_probe(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub fn wait_with_timeout(") {
        None => out.push(Offender {
            file: PROBE,
            line: 0,
            why: "`wait_with_timeout` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for i in code_lines_with(&window, ".wait()") {
                out.push(Offender {
                    file: PROBE,
                    line: at + i,
                    why: "`wait_with_timeout` が打ち切った子を同期に `wait` している（カーネルの中で \
                          止まった子は SIGKILL でも出てくるまで終わらず、上限が意味を失う = #1979。\
                          刈り取りは `reap_in_background` へ渡す）"
                        .into(),
                });
            }
            if code_lines_with(&window, "reap_in_background(").is_empty() {
                out.push(Offender {
                    file: PROBE,
                    line: at,
                    why: "`wait_with_timeout` が打ち切った子を刈り取り係へ渡していない（ゾンビが残る）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 3. 親子表の採取は macOS で libproc を先に、落ち先の `ps` は上限つき
fn scan_process_table(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn capture_process_table_uncached(") {
        None => out.push(Offender {
            file: AGENTS,
            line: 0,
            why: "`capture_process_table_uncached` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let libproc = code_lines_with(&window, "ports::process_table()");
            let bounded = code_lines_with(&window, "output_with_timeout(");
            if libproc.is_empty() {
                out.push(Offender {
                    file: AGENTS,
                    line: at,
                    why:
                        "親子表の採取が libproc（`tako_core::ports::process_table`）を使っていない \
                          （照会のたびに `ps` を起こす形へ戻る = #1979）"
                            .into(),
                });
            }
            if bounded.is_empty() {
                out.push(Offender {
                    file: AGENTS,
                    line: at,
                    why:
                        "libproc が無いときの `ps` が上限つき（`probe::output_with_timeout`）でない"
                            .into(),
                });
            }
        }
    }
    out
}

/// 4. `tako list` の採り直しを UI スレッドで待たない（dispatch 側）
fn scan_dispatch_list(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };
    match fn_window(src, "pub fn prepare_offload(") {
        None => push(0, "`prepare_offload` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let arm = code_lines_with(&window, "Request::List if");
            let job = code_lines_with(&window, "OffloadJob::List {");
            if arm.is_empty() || job.is_empty() {
                push(
                    at,
                    "`prepare_offload` が `Request::List` を background のジョブ（`OffloadJob::List`）に \
                     していない（`tako list` のたびに UI スレッドが `tmux list-windows -a` を待つ = #1979）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "pub fn run_staged(") {
        None => push(0, "`run_staged` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if code_lines_with(&window, "list_windows_by_session(").is_empty() {
                push(
                    at,
                    "`run_staged` が `OffloadJob::List` で器の window 一覧を採っていない \
                     （background で採らないと `backend_windows` が古いまま返る = #1191 の後退）"
                        .into(),
                );
            }
        }
    }
    // UI スレッドへ戻った続き（反映と応答の組み立て）は tmux を起こさない
    for head in ["OffloadContinuation::List(by_session) =>", "fn list_reply("] {
        match fn_window(src, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                for (needle, why) in FORBIDDEN_ON_UI {
                    for i in code_lines_with(&window, needle) {
                        push(
                            at + i,
                            format!(
                                "`tako list` の UI スレッド側（`{head}`）が `{needle}` を呼んでいる \
                                 （{why}。UI スレッドで子プロセスを待つ = #1979）"
                            ),
                        );
                    }
                }
            }
        }
    }
    out
}

/// 4 の続き: tako-app の判断と反映が tmux を起こさない
fn scan_main_list(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for head in [
        "fn backend_windows_plan(",
        "fn backend_windows_fetch_plan(",
        "fn apply_backend_windows_fetch(",
        "fn apply_backend_windows(",
    ] {
        match fn_window(src, head) {
            None => out.push(Offender {
                file: MAIN,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for (needle, why) in FORBIDDEN_ON_UI {
                    for i in code_lines_with(&window, needle) {
                        out.push(Offender {
                            file: MAIN,
                            line: at + i,
                            why: format!(
                                "UI スレッドで走る `{head}` が `{needle}` を呼んでいる（{why}。\
                                 判断と反映はメモリ操作だけにする = #1979）"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

struct Sources {
    tmux: String,
    backend_tmux: String,
    agents: String,
    orch: String,
    probe: String,
    dispatch: String,
    main: String,
}

impl Sources {
    fn load() -> Self {
        let root = workspace_root();
        Self {
            tmux: read(&root, TMUX),
            backend_tmux: read(&root, BACKEND_TMUX),
            agents: read(&root, AGENTS),
            orch: read(&root, ORCH),
            probe: read(&root, PROBE),
            dispatch: read(&root, DISPATCH),
            main: read(&root, MAIN),
        }
    }

    fn offenders(&self) -> Vec<Offender> {
        let mut v = Vec::new();
        for file in PRIMITIVES {
            let src = match *file {
                TMUX => &self.tmux,
                BACKEND_TMUX => &self.backend_tmux,
                ORCH => &self.orch,
                _ => &self.agents,
            };
            v.extend(scan_primitive(file, src));
        }
        v.extend(scan_legacy_gates(&self.tmux, &self.agents));
        v.extend(scan_probe(&self.probe));
        v.extend(scan_process_table(&self.agents));
        v.extend(scan_dispatch_list(&self.dispatch));
        v.extend(scan_main_list(&self.main));
        v
    }
}

fn fail_with(offenders: &[Offender]) -> ! {
    let list: Vec<String> = offenders.iter().map(Offender::report).collect();
    panic!(
        "UI スレッドから届く子プロセスの待ちが上限なしに戻っている（#1979）:\n  {}",
        list.join("\n  ")
    );
}

#[test]
fn ui_スレッドから届く子プロセスの待ちに上限がある() {
    let offenders = Sources::load().offenders();
    if !offenders.is_empty() {
        fail_with(&offenders);
    }
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::load();
    // 1 実装の窓に本体が入っている（関数名の追跡が効いている）
    for (src, head, must) in [
        (&s.tmux, "fn run_bounded(", "command_output_with_timeout("),
        (&s.tmux, "pub(crate) fn run_tmux(", "run_bounded("),
        (&s.tmux, "pub fn session_alive(", "run_tmux("),
        (&s.backend_tmux, "fn pane_pids_all(", "run_tmux("),
        (&s.agents, "pub fn tmux_pane_pids(", "tmux::query("),
        (
            &s.agents,
            "fn capture_process_table_uncached(",
            "ports::process_table()",
        ),
        (&s.probe, "pub fn wait_with_timeout(", "try_wait()"),
        (
            &s.orch,
            "fn run_claude_agents_json_live(",
            "command_output_with_timeout(",
        ),
        (&s.dispatch, "pub fn prepare_offload(", "Request::List if"),
        (&s.main, "fn backend_windows_plan(", "BACKEND_WINDOWS_FRESH"),
    ] {
        let (_, w) = fn_window(src, head).unwrap_or_else(|| panic!("{head} の窓"));
        assert!(
            !code_lines_with(&w, must).is_empty(),
            "{head} の窓に `{must}` が入っていない（窓の切り方が壊れた?）"
        );
    }
    // 上限なしの待ちの検出そのものが効いている: 再現アームの中には実際に在る
    let (_, w) = fn_window(&s.agents, "fn ps_table_unbounded_legacy(").expect("再現アーム");
    assert!(!code_lines_with(&w, ".output()").is_empty());
    assert_eq!(
        fn_name("    pub(crate) fn run_tmux(socket: Option<&str>)"),
        Some("run_tmux")
    );
    assert_eq!(
        fn_name("    fn pane_pids_all(&self) -> Vec<(String, u32)> {"),
        Some("pane_pids_all")
    );
    assert_eq!(fn_name("    // fn not_code("), None);
    assert_eq!(fn_name(r#"    let s = "fn x(";"#), None);
}

/// 修正前を再現した注入が file:line で名指しされる（番犬の検出力）
#[test]
fn 逆戻りを名指しできる() {
    let s = Sources::load();
    assert!(s.offenders().is_empty(), "前提: 現行は緑");

    let expect_named = |label: &str, offenders: Vec<Offender>, file: &str, needle: &str| {
        let hit = offenders
            .iter()
            .find(|o| o.file == file && o.line > 0 && o.why.contains(needle));
        assert!(
            hit.is_some(),
            "注入「{label}」を名指しできない: {:?}",
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
        eprintln!("注入「{label}」→ {}", hit.unwrap().report());
    };

    // ① 器への問い合わせを修正前の `.output()` へ戻す（`run_tmux` の本体）
    let injected = s.tmux.replacen(
        "    run_bounded(tmux_command(socket), args)\n}",
        "    let output = tmux_command(socket).args(args).output().map_err(|e| e.to_string())?;\n    Ok(String::from_utf8_lossy(&output.stdout).into_owned())\n}",
        1,
    );
    assert_ne!(injected, s.tmux, "注入①の置換元がある");
    expect_named(
        "器の問い合わせを output() へ",
        scan_primitive(TMUX, &injected),
        TMUX,
        "fn run_tmux",
    );

    // ② 器の全ペインの pid を修正前の形（`tmux_command(..).output()`）へ戻す（backend）。
    // 走査は字面だけを見るので、注入はコンパイルが通る形でなくてよい
    let injected = s.backend_tmux.replacen(
        "    fn pane_pids_all(&self) -> Vec<(String, u32)> {\n",
        "    fn pane_pids_all(&self) -> Vec<(String, u32)> {\n        let output = crate::tmux::tmux_command(self.sock()).args([\"list-panes\", \"-a\"]).output();\n",
        1,
    );
    assert_ne!(injected, s.backend_tmux, "注入②の置換元がある");
    expect_named(
        "backend の list-panes を output() へ",
        scan_primitive(BACKEND_TMUX, &injected),
        BACKEND_TMUX,
        "fn pane_pids_all",
    );

    // ③ 親子表の採取をゲートの外の上限なしの `ps` へ戻す（本番ハングの形）
    let injected = s.agents.replacen(
        "    if let Some(table) = tako_core::ports::process_table() {\n        return table;\n    }",
        "    if true {\n        return ps_table_unbounded_legacy();\n    }",
        1,
    );
    assert_ne!(injected, s.agents, "注入③の置換元がある");
    let mut hits = scan_legacy_gates(&s.tmux, &injected);
    hits.extend(scan_process_table(&injected));
    expect_named("ゲートの外の ps", hits, AGENTS, "ゲートの外");

    // ④ 打ち切った子を同期に wait する（修正前の probe）
    let injected = s.probe.replacen(
        "                    reap_in_background(child);",
        "                    let _ = child.wait();",
        1,
    );
    assert_ne!(injected, s.probe, "注入④の置換元がある");
    expect_named(
        "打ち切り後の同期 wait",
        scan_probe(&injected),
        PROBE,
        "同期に `wait`",
    );

    // ④' `claude agents --json` の走査を修正前の `output()` へ戻す
    let injected = s.orch.replacen(
        "    let output = match tako_core::probe::command_output_with_timeout(\n        &mut command,",
        "    let output = match command.output() {\n        Ok(o) => o, _ => return None };\n    let _ = (\n        &mut command,",
        1,
    );
    assert_ne!(injected, s.orch, "注入④'の置換元がある");
    expect_named(
        "claude agents の走査を output() へ",
        scan_primitive(ORCH, &injected),
        ORCH,
        "fn run_claude_agents_json_live",
    );

    // ⑤ `tako list` を background へ出さない（修正前の prepare_offload）
    let injected =
        s.dispatch
            .replacen("        Request::List if ", "        Request::ListX if ", 1);
    assert_ne!(injected, s.dispatch, "注入⑤の置換元がある");
    expect_named(
        "tako list を同期のまま",
        scan_dispatch_list(&injected),
        DISPATCH,
        "OffloadJob::List",
    );

    // ⑥ UI スレッドの判断で tmux を起こす（tako-app）
    let injected = s.main.replacen(
        "        Some(tako_core::tmux_backend::socket_name())\n    }",
        "        let _ = tako_core::tmux::list_windows_by_session(None);\n        Some(tako_core::tmux_backend::socket_name())\n    }",
        1,
    );
    assert_ne!(injected, s.main, "注入⑥の置換元がある");
    expect_named(
        "UI スレッドの判断で list-windows",
        scan_main_list(&injected),
        MAIN,
        "backend_windows_plan",
    );

    // ⑦ UI スレッドへ戻った続きで同期の採り直しをする（dispatch）
    let injected = s.dispatch.replacen(
        "            host.apply_backend_windows_fetch(by_session);",
        "            host.apply_backend_windows_fetch(by_session);\n            host.refresh_backend_windows();",
        1,
    );
    assert_ne!(injected, s.dispatch, "注入⑦の置換元がある");
    expect_named(
        "続きで同期の採り直し",
        scan_dispatch_list(&injected),
        DISPATCH,
        "refresh_backend_windows",
    );
}

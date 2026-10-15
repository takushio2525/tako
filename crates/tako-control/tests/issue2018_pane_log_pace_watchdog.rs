//! 器のペインログの取り込みが毎 tick に戻らないか・閉じるときの最後の増分を落とさないかの番犬（#2018）
//!
//! # 何が起きていたか
//!
//! 器（tmux）のペインログは 2 秒ごとに `tmux list-panes -a`（増分の有無）と
//! `tmux capture-pane`（増えた履歴）を起こして取り込んでいた。出力中の器 1 枚につき
//! tako の回収済みの子プロセスの CPU が約 1%（#2011 S0 の実測）で、出力している器の枚数に
//! 比例して増える。#2018 で「出力の合図（ペインの PTY に届いた Wakeup の回数）が動いた器だけ、
//! 出力が続く間は 10 秒ごと・止まったら次の tick」へ間引いた（判断は
//! `tako_core::pane_log::BackendPace` の 1 実装）。
//!
//! 間引いたぶん、閉じるときに「最後の取り込みから閉じるまでに履歴へ落ちた行」が
//! 最大 10 秒ぶん抜けうるので、器を kill する直前に kill のスレッドで取り込んでから
//! 可視画面とクローズマーカーを書く形にした。
//!
//! # 何を縛るか
//!
//! 1. UI スレッドで走るペインログの関数（収集・閉じる素材・書き込み）が tmux を起こさない
//!    （probe / capture / kill は background とスレッドだけ = #1968 / #1979 と同じ線）
//! 2. 器のペインのジョブは `BackendPace::decide` を通った tick だけ作る
//! 3. 器の取り込み（2 秒ごとの本体と、閉じる直前の取り込み）が同じ錠で 1 本ずつに並ぶ
//!    （並ばないと同じ増分を 2 回書く）
//! 4. `drop_backend_session` の kill のスレッドが、最後の増分の取り込み → kill の順に呼ぶ
//! 5. 器が自ら消えた閉じ方（#2085）は、器の `pane-died` フックが書いた全履歴 + 画面を
//!    **別スレッドで**読む（`apply_pane_log_close_from` は UI スレッド）。読み手
//!    `finish_backend_pane_log_exit` も 3. と同じ錠で並ぶ（2 秒ごとの取り込みと同じ増分を読む）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓に本体が入っていることを固定し、[`逆戻りを名指しできる`] で
//! 修正前の形へ戻した注入が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";

/// UI スレッドで走るペインログの関数
const UI_SIDE: &[&str] = &[
    "fn collect_pane_log_work(",
    "fn backend_pane_log_jobs(",
    "fn pane_log_close_data(",
    "fn apply_pane_log_close_from(",
    "fn write_pane_log_close(",
    "fn flush_pane_log_close_now(",
];

/// UI スレッドで呼んではいけない口（右は名指しの理由）
const FORBIDDEN_ON_UI: &[(&str, &str)] = &[
    ("pane_log_probe_batch(", "`tmux list-panes -a` を起こす"),
    (
        "pane_log_probe_and_capture(",
        "`tmux list-panes -a` + `capture-pane` を起こす",
    ),
    ("tmux::pane_log_probe(", "`tmux display-message` を起こす"),
    ("pane_log_probe_screen(", "`tmux display-message` を起こす"),
    ("capture_history_plain(", "`tmux capture-pane` を起こす"),
    ("capture_scrollback_plain(", "`tmux capture-pane` を起こす"),
    (
        "process_pane_log_jobs(",
        "器の取り込みの本体（tmux を起こす）",
    ),
    (
        "finish_backend_pane_log_close(",
        "閉じる直前の取り込み（tmux を起こす）",
    ),
    ("kill_session(", "`tmux kill-session` を起こす"),
    ("Command::new(", "子プロセスを起こす"),
    (
        "take_pane_died_dump(",
        "器が自ら消える前の書き出し（全履歴 + 画面）をファイルから読む = #2085",
    ),
];

/// 器の取り込みを並べる錠を取るべき関数
const SERIALIZED: &[&str] = &[
    "fn process_pane_log_jobs(",
    "fn finish_backend_pane_log_close(",
    "fn finish_backend_pane_log_exit(",
];

const INGEST_LOCK: &str = "PANE_LOG_BACKEND_INGEST";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&src, rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{MAIN}:{} — {}", self.line, self.why)
    }
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

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
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

fn scan(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| out.push(Offender { line, why });

    // 1. UI スレッド側が tmux を起こさない
    for head in UI_SIDE {
        match fn_window(src, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                for (needle, why) in FORBIDDEN_ON_UI {
                    for i in code_lines_with(&window, needle) {
                        push(
                            at + i,
                            format!(
                                "UI スレッドで走る `{}` が `{needle}` を呼んでいる（{why}。\
                                 器への問い合わせは background だけ = #2018 / #1968）",
                                head.trim_start_matches("fn ").trim_end_matches('(')
                            ),
                        );
                    }
                }
            }
        }
    }

    // 2. 器のジョブは間合いの判断を通った tick だけ
    match fn_window(src, "fn backend_pane_log_jobs(") {
        None => {}
        Some((at, window)) => {
            let decided = code_lines_with(&window, ".decide(");
            let pushed = code_lines_with(&window, "jobs.push(");
            match (decided.first(), pushed.first()) {
                (Some(d), Some(p)) if d < p => {}
                _ => push(
                    at + pushed.first().copied().unwrap_or(0),
                    "`backend_pane_log_jobs` が `BackendPace::decide` を通さずにジョブを作っている \
                     （器の取り込みが毎 tick に戻る = 出力中の器 1 枚につき子プロセスの CPU 約 1%。#2018）"
                        .into(),
                ),
            }
        }
    }

    // 3. 器の取り込みは 1 本ずつ
    for head in SERIALIZED {
        match fn_window(src, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                if code_lines_with(&window, INGEST_LOCK).is_empty() {
                    push(
                        at,
                        format!(
                            "`{}` が `{INGEST_LOCK}` を取っていない（2 秒ごとの取り込みと閉じる直前の \
                             取り込みが同じ増分を同時に読み、行が重複する = #2018）",
                            head.trim_start_matches("fn ").trim_end_matches('(')
                        ),
                    );
                }
            }
        }
    }

    // 4. kill のスレッドは最後の増分を取り込んでから kill する
    match fn_window(src, "fn drop_backend_session(") {
        None => push(
            0,
            "`fn drop_backend_session(` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let spawn = code_lines_with(&window, "std::thread::spawn(")
                .first()
                .copied();
            let finish = code_lines_with(&window, "finish_backend_pane_log_close(")
                .first()
                .copied();
            let kill = code_lines_with(&window, "kill_session(").first().copied();
            let ordered = matches!(
                (spawn, finish, kill),
                (Some(s), Some(f), Some(k)) if s < f && f < k
            );
            if !ordered {
                push(
                    at + kill.or(spawn).unwrap_or(0),
                    "`drop_backend_session` の kill のスレッドが、最後の増分の取り込み \
                     （`finish_backend_pane_log_close`）→ kill の順になっていない（器が消えた後は \
                     履歴を読めないので、最後の取り込みから閉じるまでの行が抜ける = #2018）"
                        .into(),
                );
            }
        }
    }

    // 5. 器が自ら消えた閉じ方は、書き出しを別スレッドで読む（#2085）
    match fn_window(src, "fn apply_pane_log_close_from(") {
        None => {}
        Some((at, window)) => {
            let off_ui = window.iter().any(|l| {
                !is_comment(l)
                    && l.contains("std::thread::spawn(")
                    && l.contains("finish_backend_pane_log_exit(")
            });
            if !off_ui {
                let call = code_lines_with(&window, "finish_backend_pane_log_exit(")
                    .first()
                    .copied()
                    .unwrap_or(0);
                push(
                    at + call,
                    "`apply_pane_log_close_from` が器の書き出しの取り込み（`finish_backend_pane_log_exit`）を \
                     `std::thread::spawn(` の中で呼んでいない（UI スレッドでファイルを読み、2 秒ごとの取り込みの錠を \
                     待つ = #2085 / #1979）"
                        .into(),
                );
            }
        }
    }
    out
}

fn fail_with(offenders: &[Offender]) -> ! {
    let list: Vec<String> = offenders.iter().map(Offender::report).collect();
    panic!(
        "器のペインログの取り込みの形が崩れている（#2018）:\n  {}",
        list.join("\n  ")
    );
}

#[test]
fn 器のペインログは間引いて取り込み閉じる直前に最後の増分を拾う() {
    let main = read(&workspace_root(), MAIN);
    let offenders = scan(&main);
    if !offenders.is_empty() {
        fail_with(&offenders);
    }
}

#[test]
fn 走査が空振りしていない() {
    let main = read(&workspace_root(), MAIN);
    for head in UI_SIDE.iter().chain(SERIALIZED) {
        let (_, w) = fn_window(&main, head).unwrap_or_else(|| panic!("{head} の窓"));
        // 宣言・本体・閉じ括弧の 3 行が最小（`flush_pane_log_close_now` は 1 行の本体）
        assert!(w.len() >= 3, "{head} の窓が短すぎる（{} 行）", w.len());
    }
    // 窓に本体が入っている（各関数の中身を表す行が窓に入っている）
    for (head, body) in [
        ("fn collect_pane_log_work(", "history_plain_lines("),
        ("fn backend_pane_log_jobs(", "pane_delivery"),
        ("fn pane_log_close_data(", "visible_lines()"),
        ("fn process_pane_log_jobs(", "pane_log_probe_batch("),
        ("fn finish_backend_pane_log_close(", "flush_close("),
        ("fn finish_backend_pane_log_exit(", "take_pane_died_dump("),
        ("fn apply_pane_log_close_from(", "CloseOrigin::ProcessExit"),
        ("fn drop_backend_session(", "backend_sessions.remove("),
    ] {
        let (_, w) = fn_window(&main, head).unwrap();
        assert!(
            !code_lines_with(&w, body).is_empty(),
            "{head} の窓に本体（{body}）が入っていない"
        );
    }
}

/// 置換 1 つぶんの注入（修正前の形・壊れた形へ戻す）
struct Injection {
    name: &'static str,
    from: &'static str,
    to: &'static str,
    /// 名指しの理由に含まれるべき語
    expect: &'static str,
}

const INJECTIONS: &[Injection] = &[
    Injection {
        name: "器のジョブを毎 tick 作る（修正前）",
        from: "            let Some(due) = pace.decide(activity, now, legacy) else {\n                riders.push((*pane_id, backend));\n                continue;\n            };",
        to: "            let _ = (pace, activity, now, legacy, &mut riders);\n            let due = tako_core::pane_log::IngestDue { budget: 400, output: true };",
        expect: "BackendPace::decide",
    },
    Injection {
        name: "閉じる素材を集めるときに器を問い合わせる",
        from: "        let visible = session.visible_lines();\n        let has_state",
        to: "        let visible = session.visible_lines();\n        let _ = tako_core::tmux::pane_log_probe(None, \"x\");\n        let has_state",
        expect: "tmux::pane_log_probe(",
    },
    Injection {
        name: "閉じる直前の取り込みを並べない",
        from: "    socket: &str,\n    pane: PaneId,\n    close: PendingPaneLogClose,\n) {\n    let _serial = PANE_LOG_BACKEND_INGEST\n        .lock()\n        .unwrap_or_else(|poisoned| poisoned.into_inner());\n    let data = close.data;",
        to: "    socket: &str,\n    pane: PaneId,\n    close: PendingPaneLogClose,\n) {\n    let data = close.data;",
        expect: INGEST_LOCK,
    },
    Injection {
        name: "器が自ら消えたときの書き出しの取り込みを並べない（#2085）",
        from: "PaneLogManager>>,\n    pane: PaneId,\n    close: PendingPaneLogClose,\n) {\n    let _serial = PANE_LOG_BACKEND_INGEST\n        .lock()\n        .unwrap_or_else(|poisoned| poisoned.into_inner());\n    let data = close.data;",
        to: "PaneLogManager>>,\n    pane: PaneId,\n    close: PendingPaneLogClose,\n) {\n    let data = close.data;",
        expect: INGEST_LOCK,
    },
    Injection {
        name: "器が自ら消えたときの書き出しを UI スレッドで読む（#2085）",
        from: "            std::thread::spawn(move || finish_backend_pane_log_exit(&logs, pane, close));",
        to: "            finish_backend_pane_log_exit(&logs, pane, close);",
        expect: "std::thread::spawn(",
    },
    Injection {
        name: "書き出しを UI スレッドのクローズフラッシュで読む（#2085）",
        from: "        use tako_core::pane_log::{ChunkKind, PaneObservation};\n        let mut mgr = self.pane_logs_lock();",
        to: "        use tako_core::pane_log::{ChunkKind, PaneObservation};\n        let _ = tako_core::tmux_backend::take_pane_died_dump(std::path::Path::new(\"x\"), \"x\");\n        let mut mgr = self.pane_logs_lock();",
        expect: "take_pane_died_dump(",
    },
    Injection {
        name: "kill してから最後の増分を取り込む",
        from: "                if let Some(close) = pending_log {\n                    finish_backend_pane_log_close(&logs, &socket, pane_id, close);\n                }\n                tako_core::tmux_backend::kill_session(&socket, &name)",
        to: "                let killed = tako_core::tmux_backend::kill_session(&socket, &name);\n                if let Some(close) = pending_log {\n                    finish_backend_pane_log_close(&logs, &socket, pane_id, close);\n                }\n                killed",
        expect: "finish_backend_pane_log_close",
    },
];

#[test]
fn 逆戻りを名指しできる() {
    let main = read(&workspace_root(), MAIN);
    assert!(scan(&main).is_empty(), "注入の前提: 現在のソースは緑");
    for inj in INJECTIONS {
        assert_eq!(
            main.matches(inj.from).count(),
            1,
            "注入「{}」の置換元がちょうど 1 か所ある",
            inj.name
        );
        let injected = main.replacen(inj.from, inj.to, 1);
        let offenders = scan(&injected);
        let hit = offenders
            .iter()
            .find(|o| o.line > 0 && o.why.contains(inj.expect));
        assert!(
            hit.is_some(),
            "注入「{}」を名指しできない（違反: {:?}）",
            inj.name,
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}

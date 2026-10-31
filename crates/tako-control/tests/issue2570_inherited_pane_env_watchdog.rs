//! ペインの中から起こし直した GUI が、そのペインの env を自分と子へ持ち込まないことの番犬（#2570）
//!
//! # なぜ要るか
//!
//! 10/31、本番の GUI（master のペインから install 後に起こし直したもの）の env に、master のペインの
//! `TAKO_PANE_ID` / `TAKO_ORCHESTRATOR_ROLE`・起こした tako への接続・mod の注入・claude のセッションの
//! 印・claude の Bash の子の印（`GIT_EDITOR=true`）・6 段に重なった `XDG_DATA_DIRS` が載っていた。
//! ペインは「GUI の env + 注入」で作るので、新しいペインと GUI がペインの外で起こす子へ素通しになる
//! （#2559 の自動命名の報告の取り違えもこの根から出た）。GUI は `main` の最初に
//! `tako_core::host_env::apply_at_startup` で外す。外す名前の正本は `tako_core::host_env::STRIP`。
//!
//! # 何を縛るか
//!
//! 1. GUI の `main` が、器の段（`exec_container_child_if_requested`。ペインの中で起こされ、ペインの
//!    env をシェルへ渡す）の**後**・最初のスレッド（`spawn_stall_watchdog`）の**前**に、条件なしで外し、
//!    続けて外した env のまま起こし直す（`reexec_if_stripped`。exec の時点の env の塊 = `ps eww` と
//!    資源の走査が見る値を外した後の形にする）。
//!    persist.log への 1 行は隔離の置き場が決まった**後**に書く（前に書くと隔離 GUI が本番の
//!    persist.log へ書く）
//! 2. GUI がペインへ積む env の名前（`spawn_session` の `options.env.push(("…"`）がすべて `STRIP` にある
//!    （ペインへ配る名前を増やしたら、ペインの中から起こした GUI からも外す）
//! 3. 検証の隔離 GUI が落とす名前（`isolated-gui.sh` の `ISOLATED_GUI_DROP_ENV`。#2570 より前の
//!    バイナリのために残す）がすべて `STRIP` にある（スクリプトと GUI の判断を揃える）
//! 4. 外しすぎ: GUI の `main` が隔離のために立てる名前（`set_var`）は `STRIP` に無い
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓を固定し、[`修正前の形へ戻すと名指しする`] で
//! **修正前を再現した注入**が `file:line` で名指しされることを確かめる。外す判断そのもの
//! （どの値を外す・畳む・残すか）は `tako_core::host_env` の単体テストが縛る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const ISOLATED_GUI: &str = "scripts/lib/isolated-gui.sh";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_raw(root: &Path, rel: &'static str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
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

/// 窓の中で `needle` を含む最初の**コードの**行の位置（0-based）
fn code_pos(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

fn missing(file: &'static str, needle: &str) -> Offender {
    Offender {
        file,
        line: 0,
        why: format!("`{needle}` が見つからない（走査の空振り）"),
    }
}

/// `STRIP` の全名前
fn strip_names() -> Vec<&'static str> {
    tako_core::host_env::STRIP
        .iter()
        .flat_map(|s| s.names.iter().copied())
        .collect()
}

struct Sources {
    main: String,
    script: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        main: production_range::production(&read_raw(root, MAIN), MAIN),
        // シェルは `#[cfg(test)]` を持たないので原文のまま
        script: read_raw(root, ISOLATED_GUI),
    }
}

/// 1. `main` の中の置き場
fn scan_main_order(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(main, "fn main() {") else {
        out.push(missing(MAIN, "fn main() {"));
        return out;
    };
    let line_of = |pos: usize| at + pos;
    let Some(exec) = code_pos(&window, "exec_container_child_if_requested()") else {
        out.push(missing(MAIN, "exec_container_child_if_requested()"));
        return out;
    };
    let Some(thread) = code_pos(&window, "spawn_stall_watchdog()") else {
        out.push(missing(MAIN, "spawn_stall_watchdog()"));
        return out;
    };
    let Some(apply) = code_pos(&window, "host_env::apply_at_startup()") else {
        out.push(Offender {
            file: MAIN,
            line: at,
            why: "GUI の `main` が起動の最初に `tako_core::host_env::apply_at_startup()` を呼んでいない。\
                  ペインの中から起こし直すと、そのペインの env（TAKO_PANE_ID・claude のセッションの印・\
                  GIT_EDITOR=true ほか）が新しいペインと GUI の子へ素通しになる（#2570）"
                .into(),
        });
        return out;
    };
    if apply < exec {
        out.push(Offender {
            file: MAIN,
            line: line_of(apply),
            why: "env を外すのが器の段（`exec_container_child_if_requested`）より前にある。段は器の\
                  ペインの中で起こされ、ペインの env をシェルへ渡すので、外すと器のペインが識別を失う（#2570）"
                .into(),
        });
    }
    if apply > thread {
        out.push(Offender {
            file: MAIN,
            line: line_of(apply),
            why: "env を外すのが最初のスレッド（`spawn_stall_watchdog`）より後にある。env の書き換えは\
                  別スレッドの読みと競る（#2570）"
                .into(),
        });
    }
    // 条件の中に置かない（main の本文の字下げ = 4 文字）
    let top_level = |pos: usize| {
        window
            .lines()
            .nth(pos)
            .is_some_and(|l| l.len() - l.trim_start().len() == 4)
    };
    if !top_level(apply) {
        out.push(Offender {
            file: MAIN,
            line: line_of(apply),
            why: "env を外すのが条件の中にある（起動の形によって外さない道ができる。#2570）".into(),
        });
    }
    // unix は外した env のまま起こし直す（exec の時点の env の塊 = ps eww と資源の走査が見る値）。
    // 外した直後・最初のスレッドの前に、条件なしで
    match code_pos(&window, "host_env::reexec_if_stripped(&inherited_env)") {
        None => out.push(Offender {
            file: MAIN,
            line: line_of(apply),
            why: "外した後に `host_env::reexec_if_stripped(&inherited_env)` で起こし直していない。exec の時点の \
                  env の塊（ps eww・資源の走査）に起こしたペインの TAKO_PANE_ID が残り、GUI 自身と子が\
                  そのペインのプロセスとして数えられる（#2570）"
                .into(),
        }),
        Some(reexec) => {
            if reexec < apply || reexec > thread || !top_level(reexec) {
                out.push(Offender {
                    file: MAIN,
                    line: line_of(reexec),
                    why: "起こし直しは外した直後・最初のスレッドの前に条件なしで置く（exec はほかのスレッドを\
                          残さない = 起こす前に呼ぶ。#2570）"
                        .into(),
                });
            }
        }
    }
    // persist.log の 1 行は隔離の置き場が決まった後
    let Some(isolation_end) = ["\"TAKO_REMOTE_STATE_DIR\"", "self_test_isolation_defaults("]
        .iter()
        .filter_map(|n| code_pos(&window, n))
        .max()
    else {
        out.push(missing(MAIN, "\"TAKO_REMOTE_STATE_DIR\""));
        return out;
    };
    let logs: Vec<usize> = window
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            !l.trim_start().starts_with("//") && l.contains("inherited_env.log_line()")
        })
        .map(|(i, _)| i)
        .collect();
    if logs.is_empty() {
        out.push(Offender {
            file: MAIN,
            line: at,
            why: "外した名前を persist.log へ残していない（`inherited_env.log_line()`。#2570）"
                .into(),
        });
    }
    for log in logs {
        if log < isolation_end {
            out.push(Offender {
                file: MAIN,
                line: line_of(log),
                why: "外した名前の persist.log の 1 行が隔離の置き場を決めるより前にある（隔離 GUI が\
                      本番の persist.log へ書く。#2570）"
                    .into(),
            });
        }
    }
    out
}

/// 2. GUI がペインへ積む env の名前（`spawn_session` の `.push(("…"`）が `STRIP` にある
fn scan_pane_env(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(main, "fn spawn_session(") else {
        out.push(missing(MAIN, "fn spawn_session("));
        return out;
    };
    let strip = strip_names();
    let mut seen = 0;
    for (i, l) in window.lines().enumerate() {
        if l.trim_start().starts_with("//") {
            continue;
        }
        let Some(rest) = l.split(".push((\"").nth(1) else {
            continue;
        };
        let Some(name) = rest.split('"').next() else {
            continue;
        };
        seen += 1;
        if !strip.contains(&name) {
            out.push(Offender {
                file: MAIN,
                line: at + i,
                why: format!(
                    "ペインへ積む `{name}` が `tako_core::host_env::STRIP` に無い。そのペインの中から起こし直した \
                     GUI が受け継ぎ、新しいペインと GUI の子へ素通しになる（#2570）"
                ),
            });
        }
    }
    if seen == 0 {
        out.push(missing(MAIN, ".push((\"TAKO_"));
    }
    out
}

/// `ISOLATED_GUI_DROP_ENV=( … )` の名前と行
fn script_drop_env(script: &str) -> Option<Vec<(usize, String)>> {
    let lines: Vec<&str> = script.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("ISOLATED_GUI_DROP_ENV=("))?;
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        let t = l.trim();
        if t == ")" {
            return Some(out);
        }
        if t.starts_with('#') {
            continue;
        }
        for name in t.split_whitespace() {
            out.push((i + 1, name.to_string()));
        }
    }
    None
}

/// 3. 検証の隔離 GUI が落とす名前が `STRIP` にある
fn scan_script(script: &str) -> Vec<Offender> {
    let Some(names) = script_drop_env(script) else {
        return vec![missing(ISOLATED_GUI, "ISOLATED_GUI_DROP_ENV=(")];
    };
    let strip = strip_names();
    names
        .into_iter()
        .filter(|(_, n)| !strip.contains(&n.as_str()))
        .map(|(line, name)| Offender {
            file: ISOLATED_GUI,
            line,
            why: format!(
                "隔離 GUI が落とす `{name}` を GUI 自身は外さない（`tako_core::host_env::STRIP` に無い）。\
                 ペインの中から直接起こした GUI に残る。スクリプトと GUI の判断を揃える（#2570）"
            ),
        })
        .collect()
}

/// 4. 外しすぎ: `main` が隔離のために立てる名前は `STRIP` に無い
fn scan_isolation(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(main, "fn main() {") else {
        return vec![missing(MAIN, "fn main() {")];
    };
    let strip = strip_names();
    let lines: Vec<&str> = window.lines().collect();
    let mut seen = 0;
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("//") || !l.contains("std::env::set_var(") {
            continue;
        }
        // 名前は同じ行か次の行の最初の引数
        let arg = l
            .split("std::env::set_var(")
            .nth(1)
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .or_else(|| lines.get(i + 1).map(|n| n.trim()))
            .unwrap_or_default();
        let name = if let Some(lit) = arg.strip_prefix('"') {
            lit.split('"').next().unwrap_or_default().to_string()
        } else if arg.starts_with("tako_core::tmux_backend::SOCKET_ENV") {
            tako_core::tmux_backend::SOCKET_ENV.to_string()
        } else {
            continue;
        };
        seen += 1;
        if strip.contains(&name.as_str()) {
            out.push(Offender {
                file: MAIN,
                line: at + i,
                why: format!(
                    "隔離のために立てる `{name}` が `tako_core::host_env::STRIP` にある（ペインの中の CLI が \
                     この GUI を見つける材料 = 外しすぎ。#2570）"
                ),
            });
        }
    }
    if seen == 0 {
        out.push(missing(MAIN, "std::env::set_var("));
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(scan_main_order(&s.main));
    out.extend(scan_pane_env(&s.main));
    out.extend(scan_script(&s.script));
    out.extend(scan_isolation(&s.main));
    out.iter().map(Offender::report).collect()
}

#[test]
fn ペインの中から起こし直したguiがペインのenvを持ち込まない配線が揃っている() {
    let s = sources(&workspace_root());
    let found = all(&s);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for needle in [
        "fn main() {",
        "fn spawn_session(",
        "exec_container_child_if_requested()",
        "spawn_stall_watchdog()",
        "host_env::apply_at_startup()",
        "inherited_env.log_line()",
        "host_env::reexec_if_stripped(&inherited_env)",
        "\"TAKO_REMOTE_STATE_DIR\"",
    ] {
        assert!(s.main.contains(needle), "{MAIN} に `{needle}` が無い");
    }
    let names = script_drop_env(&s.script).expect("ISOLATED_GUI_DROP_ENV が読める");
    assert!(
        names.len() >= 10,
        "ISOLATED_GUI_DROP_ENV の読み取りが少なすぎる: {names:?}"
    );
    let (_, window) = fn_window(&s.main, "fn spawn_session(").expect("spawn_session の窓");
    for name in ["TAKO_PANE_ID", "TAKO_SOCKET", "TAKO_TOKEN"] {
        assert!(
            window.contains(&format!(".push((\"{name}\"")),
            "spawn_session の `.push((\"{name}\"` が読めない（走査の形が変わった）"
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
        found.iter().any(|f| f.starts_with(want)),
        "注入「{what}」が `{want}` で名指しされない: {found:#?}"
    );
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let base = sources(&workspace_root());
    assert!(all(&base).is_empty(), "注入の前は緑であること");
    let apply_line = |main: &str| {
        main.lines()
            .find(|l| l.contains("host_env::apply_at_startup()"))
            .expect("apply_at_startup の行")
            .to_string()
    };

    // 注入 1: #2570 前の main（env を外さない）
    let mut s = sources(&workspace_root());
    let line = apply_line(&s.main);
    s.main = inject(&s.main, &line, "    // 注入: 外さない", "外さない main");
    assert_named(
        &all(&s),
        &format!("{MAIN}:{}", first_line(&s.main, "fn main() {")),
        "外さない main",
    );

    // 注入 2: 外すのが最初のスレッドより後
    let mut s = sources(&workspace_root());
    let line = apply_line(&s.main);
    s.main = inject(&s.main, &format!("{line}\n"), "", "外す行を抜く");
    s.main = inject(
        &s.main,
        "    tako_control::diag::spawn_stall_watchdog();",
        &format!("    tako_control::diag::spawn_stall_watchdog();\n{line}"),
        "スレッドの後で外す",
    );
    assert_named(
        &all(&s),
        &format!(
            "{MAIN}:{}",
            first_line(&s.main, "host_env::apply_at_startup()")
        ),
        "スレッドの後で外す",
    );

    // 注入 3: ペインへ新しい名前を積んだが STRIP に足していない
    let mut s = sources(&workspace_root());
    s.main = inject(
        &s.main,
        "        if let Some(tab_id) = self.workspace.find_tab_of_pane(pane_id) {",
        "        options.env.push((\"TAKO_INJECTED_2570\".into(), String::new()));\n        if let Some(tab_id) = self.workspace.find_tab_of_pane(pane_id) {",
        "ペインへ新しい名前",
    );
    assert_named(
        &all(&s),
        &format!("{MAIN}:{}", first_line(&s.main, "TAKO_INJECTED_2570")),
        "ペインへ新しい名前",
    );

    // 注入 4: 隔離 GUI が落とす名前を GUI 自身は外さない
    let mut s = sources(&workspace_root());
    s.script = inject(
        &s.script,
        "    TAKO_ORIG_ZDOTDIR\n",
        "    TAKO_ORIG_ZDOTDIR\n    TAKO_SCRIPT_ONLY_2570\n",
        "スクリプトだけが落とす名前",
    );
    assert_named(
        &all(&s),
        &format!(
            "{ISOLATED_GUI}:{}",
            first_line(&s.script, "TAKO_SCRIPT_ONLY_2570")
        ),
        "スクリプトだけが落とす名前",
    );

    // 注入 5: 外しすぎ（隔離の置き場を STRIP の名前で立てる = 例として TAKO_PANE_ID）
    let mut s = sources(&workspace_root());
    s.main = inject(
        &s.main,
        "        if std::env::var_os(\"TAKO_PANE_LOG_DIR\").is_none() {",
        "        std::env::set_var(\"TAKO_PANE_ID\", \"0\");\n        if std::env::var_os(\"TAKO_PANE_LOG_DIR\").is_none() {",
        "外しすぎ",
    );
    assert_named(
        &all(&s),
        &format!(
            "{MAIN}:{}",
            first_line(&s.main, "std::env::set_var(\"TAKO_PANE_ID\"")
        ),
        "外しすぎ",
    );

    // 注入 6: persist.log の 1 行を隔離の置き場より前に書く
    let mut s = sources(&workspace_root());
    s.main = inject(
        &s.main,
        "    tako_control::diag::spawn_stall_watchdog();",
        "    tako_control::diag::spawn_stall_watchdog();\n    let _ = inherited_env.log_line();",
        "置き場より前に書く",
    );
    assert_named(
        &all(&s),
        &format!(
            "{MAIN}:{}",
            first_line(&s.main, "let _ = inherited_env.log_line();")
        ),
        "置き場より前に書く",
    );

    // 注入 7: 外すだけで起こし直さない（ps eww と資源の走査に起こしたペインが残る）
    let mut s = sources(&workspace_root());
    let line = s
        .main
        .lines()
        .find(|l| l.contains("host_env::reexec_if_stripped(&inherited_env)"))
        .expect("起こし直しの行")
        .to_string();
    s.main = inject(
        &s.main,
        &line,
        "    let reexec_error: Option<std::io::Error> = None; // 注入",
        "起こし直さない",
    );
    assert_named(
        &all(&s),
        &format!(
            "{MAIN}:{}",
            first_line(&s.main, "host_env::apply_at_startup()")
        ),
        "起こし直さない",
    );
}

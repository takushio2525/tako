//! ワークスペース横断検索が UI スレッドで走査しないかの番犬（#1694）
//!
//! # なぜ要るか
//!
//! 横断検索は 1 万ファイル級のツリーを読み切る。UI スレッドで 1 回走らせるだけで窓が数百 ms〜
//! 数秒止まる（#1968 / #1984 / #1979 が塞いできた型と同じ）。しかも壊すのは 1 行で足りる:
//! GUI の検索欄が `dispatch_off_ui` の代わりに同期の `tako_control::dispatch` を呼ぶ、
//! `prepare_offload` から腕が消える（= IPC の要求が同期の腕へ落ちる）、準備の段で走査を始める。
//! どれも応答の形は変わらず、CLI の e2e も緑のまま通る。挙動は dispatch の単体（MockHost で
//! IPC と同じ 3 段）と実測スクリプトが測り、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 走査の口（`workspace_search::search(`）を呼ぶのは background のジョブ
//!    （`WorkspaceSearchJob::run`）だけ。dispatch / GUI のソースに走査の口が無い
//! 2. `prepare_offload` が `search` を background のジョブ（`OffloadJob::WorkspaceSearch`）にし、
//!    `run_staged` がそのジョブを走らせる
//! 3. 準備（`workspace_search_prepare`）は走査も照合器の組み立ても呼ばない（範囲の解決と登録だけ）
//! 4. GUI の検索（`run_workspace_search`）は `dispatch_off_ui` を通し、同期の dispatch を呼ばない
//! 5. 上限（保持の行数・所要）が既定の条件に入っている（外すと 1 回の検索で保持が無制限になる）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const JOB: &str = "crates/tako-control/src/workspace_search.rs";
const ENGINE: &str = "crates/tako-core/src/workspace_search.rs";
const GUI: &str = "crates/tako-app/src/search_panel.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// 走査の口（これを呼ぶ = ツリーを読み切る）
const SCAN: &str = "workspace_search::search(";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug, PartialEq, Eq)]
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

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_line(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// `#[cfg(test)]` より前（製品のコード）だけを行番号つきで返す
fn production_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .take_while(|(_, l)| l.trim() != "#[cfg(test)]")
        .map(|(i, l)| (i + 1, code_line(l)))
        .collect()
}

/// 窓の中で `needle` を含む最初のコード行（1-based。無ければ `None`）
fn code_line_in(start: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| code_line(l).contains(needle))
        .map(|i| start + i)
}

/// 1. dispatch / GUI の製品コードに走査の口が無い。ジョブのファイルでは `fn run(` の中だけ
fn scan_entry(dispatch: &str, job: &str, gui: &str, main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src) in [(DISPATCH, dispatch), (GUI, gui), (MAIN, main)] {
        for (line, code) in production_lines(src) {
            if code.contains(SCAN) {
                out.push(Offender {
                    file,
                    line,
                    why: format!(
                        "UI スレッドで走りうる経路から走査の口（{SCAN}）を呼んでいる。\
                         走査は background のジョブ（WorkspaceSearchJob::run）だけ"
                    ),
                });
            }
        }
    }
    match fn_window(job, "pub fn run(self)") {
        None => out.push(Offender {
            file: JOB,
            line: 0,
            why: "`WorkspaceSearchJob::run` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let end = at + window.lines().count();
            for (line, code) in production_lines(job) {
                if code.contains(SCAN) && (line < at || line >= end) {
                    out.push(Offender {
                        file: JOB,
                        line,
                        why: "走査の口をジョブの `run` の外で呼んでいる".into(),
                    });
                }
            }
            if code_line_in(at, &window, SCAN).is_none() {
                out.push(Offender {
                    file: JOB,
                    line: at,
                    why: "ジョブの `run` が走査していない（走査の入口が別の場所へ移った）".into(),
                });
            }
        }
    }
    out
}

/// 2〜3. dispatch: 準備は background のジョブを作るだけ・`run_staged` がそれを走らせる
fn scan_dispatch(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`prepare_offload` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_line_in(at, &window, "Request::WorkspaceSearch {").is_none()
                || code_line_in(at, &window, "OffloadJob::WorkspaceSearch(").is_none()
            {
                out.push(Offender {
                    file: DISPATCH,
                    line: code_line_in(at, &window, "Request::WorkspaceSearch {").unwrap_or(at),
                    why: "`prepare_offload` が横断検索を background のジョブにしていない \
                          （IPC の要求が同期の腕 = UI スレッドで走査する）"
                        .into(),
                });
            }
        }
    }
    match fn_window(dispatch, "pub fn run_staged(self)") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`run_staged` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let arm = code_line_in(at, &window, "OffloadJob::WorkspaceSearch(job) =>");
            let runs = code_line_in(at, &window, "job.run()");
            if arm.is_none() || runs.is_none() {
                out.push(Offender {
                    file: DISPATCH,
                    line: arm.unwrap_or(at),
                    why: "`run_staged` が横断検索のジョブを走らせていない".into(),
                });
            }
        }
    }
    match fn_window(dispatch, "fn workspace_search_prepare(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`workspace_search_prepare` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for needle in [SCAN, "Matcher::new(", ".run()"] {
                if let Some(line) = code_line_in(at, &window, needle) {
                    out.push(Offender {
                        file: DISPATCH,
                        line,
                        why: format!(
                            "準備（UI スレッド）で {needle} を呼んでいる。範囲の解決と登録だけにする"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 4. GUI の検索は `dispatch_off_ui`（= prepare_offload → background）を通す
fn scan_gui(gui: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(gui, "pub(crate) fn run_workspace_search(") {
        None => out.push(Offender {
            file: GUI,
            line: 0,
            why: "`run_workspace_search` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if let Some(line) = code_line_in(at, &window, "tako_control::dispatch(") {
                out.push(Offender {
                    file: GUI,
                    line,
                    why: "検索欄の検索を同期の dispatch で呼んでいる（UI スレッドで走査する）"
                        .into(),
                });
            }
            if code_line_in(at, &window, "dispatch_off_ui(").is_none() {
                out.push(Offender {
                    file: GUI,
                    line: at,
                    why: "検索欄の検索が `dispatch_off_ui` を通っていない".into(),
                });
            }
        }
    }
    out
}

/// 5. 既定の条件に上限が入っている
fn scan_limits(engine: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(engine, "pub fn new(roots: Vec<PathBuf>) -> Self") {
        None => out.push(Offender {
            file: ENGINE,
            line: 0,
            why: "`Options::new` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for needle in [
                "max_results: MAX_RESULTS,",
                "time_limit: DEFAULT_TIME_LIMIT,",
            ] {
                if code_line_in(at, &window, needle).is_none() {
                    out.push(Offender {
                        file: ENGINE,
                        line: at,
                        why: format!("既定の条件に上限 `{needle}` が無い"),
                    });
                }
            }
        }
    }
    match fn_window(engine, "pub fn search(") {
        None => out.push(Offender {
            file: ENGINE,
            line: 0,
            why: "`search` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_line_in(at, &window, "hits.truncate(options.max_results)").is_none() {
                out.push(Offender {
                    file: ENGINE,
                    line: at,
                    why: "並列で集めた行を上限で切っていない".into(),
                });
            }
        }
    }
    out
}

struct Sources {
    dispatch: String,
    job: String,
    engine: String,
    gui: String,
    main: String,
}

fn current() -> Sources {
    Sources {
        dispatch: read(DISPATCH),
        job: read(JOB),
        engine: read(ENGINE),
        gui: read(GUI),
        main: read(MAIN),
    }
}

fn all(s: &Sources) -> Vec<String> {
    scan_entry(&s.dispatch, &s.job, &s.gui, &s.main)
        .into_iter()
        .chain(scan_dispatch(&s.dispatch))
        .chain(scan_gui(&s.gui))
        .chain(scan_limits(&s.engine))
        .map(|o| o.report())
        .collect()
}

#[test]
fn 横断検索はuiスレッドで走査しない() {
    let offenders = all(&current());
    assert!(
        offenders.is_empty(),
        "横断検索が UI スレッドで走査しうる（#1694）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    assert!(fn_window(&s.dispatch, "pub fn prepare_offload(").is_some());
    assert!(fn_window(&s.dispatch, "pub fn run_staged(self)").is_some());
    assert!(fn_window(&s.dispatch, "fn workspace_search_prepare(").is_some());
    assert!(fn_window(&s.job, "pub fn run(self)").is_some());
    assert!(fn_window(&s.gui, "pub(crate) fn run_workspace_search(").is_some());
    assert!(fn_window(&s.engine, "pub fn new(roots: Vec<PathBuf>) -> Self").is_some());
    assert!(fn_window(&s.engine, "pub fn search(").is_some());
    // 製品のコードの範囲が採れている（`#[cfg(test)]` で打ち切られすぎていない）
    assert!(production_lines(&s.dispatch).len() > 10_000);
    assert!(production_lines(&s.main).len() > 10_000);
    assert!(production_lines(&s.job)
        .iter()
        .any(|(_, l)| l.contains(SCAN)));
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` を含む行が名指しされること
fn assert_named(
    label: &str,
    file: &str,
    (from, to): (&str, &str),
    needle: &str,
    edit: impl Fn(&mut Sources, String),
    src: &str,
) {
    assert!(
        src.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    let injected = src.replacen(from, to, 1);
    let line = injected
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let mut sources = current();
    edit(&mut sources, injected);
    let reports = all(&sources);
    let expected = format!("{file}:{line} ");
    assert!(
        reports.iter().any(|r| r.starts_with(&expected)),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let s = current();
    // A. GUI の検索欄が同期の dispatch を呼ぶ
    assert_named(
        "A 同期の dispatch",
        GUI,
        (
            "self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {",
            "let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n        self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {",
        ),
        "let _ = tako_control::dispatch(self, request.clone()",
        |s, src| s.gui = src,
        &s.gui,
    );
    // B. prepare_offload から腕が消える（IPC の要求が同期の腕へ落ちる）
    assert_named(
        "B 腕が消える",
        DISPATCH,
        (
            ".map(|job| OffloadJob::WorkspaceSearch(Box::new(job))),",
            ".map(|_job| unreachable!()),",
        ),
        "        Request::WorkspaceSearch {",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
    // C. 準備の段（UI スレッド）で走査する
    assert_named(
        "C 準備で走査",
        DISPATCH,
        (
            "    let options = Options::new(roots);\n",
            "    let options = Options::new(roots);\n    let _ = tako_core::workspace_search::search(&matcher, &options, &Default::default());\n",
        ),
        "let _ = tako_core::workspace_search::search(&matcher",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
    // D. 既定の条件から保持の上限が消える
    assert_named(
        "D 上限が消える",
        ENGINE,
        (
            "            max_results: MAX_RESULTS,",
            "            max_results: usize::MAX,",
        ),
        "    pub fn new(roots: Vec<PathBuf>) -> Self",
        |s, src| s.engine = src,
        &s.engine,
    );
    // E. run_staged が横断検索のジョブを走らせない（他の腕へ流す）
    assert_named(
        "E run_staged の腕が消える",
        DISPATCH,
        (
            "            OffloadJob::WorkspaceSearch(job) => {\n                let landing = job.run();",
            "            OffloadJob::WorkspaceSearchGone(job) => {\n                let landing = job.go();",
        ),
        "    pub fn run_staged(self)",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
}

//! 横断置換が UI スレッドでファイルを読み書きしないかの番犬（#2045）
//!
//! # なぜ要るか
//!
//! 横断置換は検索の結果（最大 2000 行・ファイルは 1 つ 8 MB まで）のファイルを読み直して照合し、
//! 退避を書き、本体を書き、取り消しでは書き戻す。UI スレッドで 1 回走らせるだけで窓が止まる
//! （#1694 の検索の走査と同じ型）。壊すのは 1 行で足りる: GUI のボタンが `dispatch_off_ui` の代わりに
//! 同期の `tako_control::dispatch` を呼ぶ、`prepare_offload` から腕が消える（= IPC の要求が同期の
//! 腕へ落ちる）、準備の段でファイルを読む。どれも応答の形は変わらず、CLI の e2e も緑のまま通る。
//! 挙動は dispatch の単体（MockHost で IPC と同じ 3 段）と実経路テストが測り、ここは**構造**を縛って
//! file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. ファイルの読み書きの口（[`IO`]）を dispatch / GUI の製品コードから呼ばない。制御側のジョブの
//!    ファイルでは background で走る関数（[`JOB_FNS`]）の中だけ
//! 2. `prepare_offload` が replace / undo を background のジョブ（`OffloadJob::WorkspaceReplace` /
//!    `WorkspaceUndo`）にし、`run_staged` がそれを走らせる
//! 3. 準備（`workspace_replace_prepare` / `workspace_undo_prepare`）はジョブを走らせず、パスの実体も
//!    引き直さない（範囲の行・バッファの写し・印だけ）
//! 4. GUI のプレビュー・当てる・戻す（[`GUI_FNS`]）は `dispatch_off_ui` を通し、同期の dispatch を呼ばない
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
const JOB: &str = "crates/tako-control/src/workspace_replace.rs";
const GUI: &str = "crates/tako-app/src/search_panel.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// ファイルの読み書きの口（これを呼ぶ = ディスクへ触る）
const IO: [&str; 7] = [
    "replace::check_disk(",
    "replace::write_disk(",
    "replace::write_backup(",
    "replace::restore_disk(",
    "replace::prune_backups(",
    "remove_backup_dir(",
    "workspace_search::search(",
];

/// 制御側のジョブのファイルで読み書きしてよい関数（どれも background の `run` からだけ呼ばれる）
const JOB_FNS: [&str; 5] = [
    "    pub fn run(self) -> ReplaceLanding",
    "    fn apply(",
    "fn one_file(",
    "pub(crate) fn remove_backup_dir(",
    "    pub fn run(self) -> UndoLanding",
];

/// GUI の置換の頼み（どれも `dispatch_off_ui` を通す）
const GUI_FNS: [&str; 3] = [
    "    fn request_replace_preview(",
    "    fn apply_replace(",
    "    fn undo_replace(",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

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
    let start = lines.iter().position(|l| l.starts_with(needle))?;
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

fn code_line_in(start: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| code_line(l).contains(needle))
        .map(|i| start + i)
}

/// 1. 読み書きの口は dispatch / GUI に無い・ジョブのファイルでは background の関数の中だけ
fn scan_io(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src) in [(DISPATCH, &s.dispatch), (GUI, &s.gui), (MAIN, &s.main)] {
        for (line, code) in production_lines(src) {
            // 横断検索の走査の口は #1694 の番犬が縛る（dispatch / GUI に無いことは同じ）
            if let Some(io) = IO.iter().find(|io| code.contains(*io)) {
                out.push(Offender {
                    file,
                    line,
                    why: format!(
                        "UI スレッドで走りうる経路からファイルの読み書きの口（{io}）を呼んでいる。\
                         background のジョブ（ReplaceJob::run / UndoJob::run）だけ"
                    ),
                });
            }
        }
    }
    let windows: Vec<(usize, usize)> = JOB_FNS
        .iter()
        .filter_map(|f| fn_window(&s.job, f))
        .map(|(at, w)| (at, at + w.lines().count()))
        .collect();
    if windows.len() != JOB_FNS.len() {
        out.push(Offender {
            file: JOB,
            line: 0,
            why: "background の関数の窓が採れない（走査が空振り）".into(),
        });
    }
    for (line, code) in production_lines(&s.job) {
        let inside = windows.iter().any(|(a, b)| line >= *a && line < *b);
        if let Some(io) = IO.iter().find(|io| code.contains(*io)) {
            if !inside {
                out.push(Offender {
                    file: JOB,
                    line,
                    why: format!("読み書きの口（{io}）を background の関数の外で呼んでいる"),
                });
            }
        }
    }
    out
}

/// 2〜3. dispatch: 準備はジョブを作るだけ・`run_staged` がそれを走らせる
fn scan_dispatch(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(dispatch, "pub fn prepare_offload(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`prepare_offload` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for job in [
                "OffloadJob::WorkspaceReplace(",
                "OffloadJob::WorkspaceUndo(",
            ] {
                if code_line_in(at, &window, job).is_none() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: code_line_in(at, &window, "if action == \"replace\"").unwrap_or(at),
                        why: format!(
                            "`prepare_offload` が置換を background のジョブ（{job}）にしていない \
                             （IPC の要求が同期の腕 = UI スレッドで読み書きする）"
                        ),
                    });
                }
            }
        }
    }
    match fn_window(dispatch, "    pub fn run_staged(self)") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`run_staged` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for arm in [
                "OffloadJob::WorkspaceReplace(job) =>",
                "OffloadJob::WorkspaceUndo(job) =>",
            ] {
                if code_line_in(at, &window, arm).is_none() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: format!("`run_staged` が置換のジョブを走らせていない（{arm} が無い）"),
                    });
                }
            }
        }
    }
    for prepare in [
        "fn workspace_replace_prepare(",
        "fn workspace_undo_prepare(",
    ] {
        match fn_window(dispatch, prepare) {
            None => out.push(Offender {
                file: DISPATCH,
                line: 0,
                why: format!("`{prepare}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for needle in [".run()", "canonicalize", "std::fs::"] {
                    if let Some(line) = code_line_in(at, &window, needle) {
                        out.push(Offender {
                            file: DISPATCH,
                            line,
                            why: format!(
                                "準備（UI スレッド）で {needle} を呼んでいる。範囲の行・バッファの写し・\
                                 印だけにする"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

/// 4. GUI の置換の頼みは `dispatch_off_ui` を通す
fn scan_gui(gui: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for name in GUI_FNS {
        match fn_window(gui, name) {
            None => out.push(Offender {
                file: GUI,
                line: 0,
                why: format!("`{}` が見つからない（走査が空振り）", name.trim()),
            }),
            Some((at, window)) => {
                if let Some(line) = code_line_in(at, &window, "tako_control::dispatch(") {
                    out.push(Offender {
                        file: GUI,
                        line,
                        why:
                            "置換の頼みを同期の dispatch で呼んでいる（UI スレッドで読み書きする）"
                                .into(),
                    });
                }
                if code_line_in(at, &window, "dispatch_off_ui(").is_none() {
                    out.push(Offender {
                        file: GUI,
                        line: at,
                        why: format!("`{}` が `dispatch_off_ui` を通っていない", name.trim()),
                    });
                }
            }
        }
    }
    out
}

struct Sources {
    dispatch: String,
    job: String,
    gui: String,
    main: String,
}

fn current() -> Sources {
    Sources {
        dispatch: read(DISPATCH),
        job: read(JOB),
        gui: read(GUI),
        main: read(MAIN),
    }
}

fn all(s: &Sources) -> Vec<String> {
    scan_io(s)
        .into_iter()
        .chain(scan_dispatch(&s.dispatch))
        .chain(scan_gui(&s.gui))
        .map(|o| o.report())
        .collect()
}

#[test]
fn 横断置換はuiスレッドでファイルを読み書きしない() {
    let offenders = all(&current());
    assert!(
        offenders.is_empty(),
        "横断置換が UI スレッドでファイルを読み書きしうる（#2045）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    for f in JOB_FNS {
        assert!(fn_window(&s.job, f).is_some(), "{f}");
    }
    for f in GUI_FNS {
        assert!(fn_window(&s.gui, f).is_some(), "{f}");
    }
    assert!(fn_window(&s.dispatch, "pub fn prepare_offload(").is_some());
    assert!(fn_window(&s.dispatch, "    pub fn run_staged(self)").is_some());
    assert!(fn_window(&s.dispatch, "fn workspace_replace_prepare(").is_some());
    assert!(fn_window(&s.dispatch, "fn workspace_undo_prepare(").is_some());
    // 製品のコードの範囲が採れている・ジョブのファイルは実際に読み書きの口を呼んでいる
    assert!(production_lines(&s.dispatch).len() > 10_000);
    assert!(production_lines(&s.main).len() > 10_000);
    for io in [
        "replace::check_disk(",
        "replace::write_disk(",
        "replace::restore_disk(",
    ] {
        assert!(
            production_lines(&s.job).iter().any(|(_, l)| l.contains(io)),
            "{io}"
        );
    }
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
    // A. GUI の「すべて置換」が同期の dispatch を呼ぶ
    assert_named(
        "A 同期の dispatch",
        GUI,
        (
            "        let request = self.replace_request(item, file, true);\n",
            "        let request = self.replace_request(item, file, true);\n        let _ = tako_control::dispatch(self, request.clone(), PaneOrigin::User);\n",
        ),
        "let _ = tako_control::dispatch(self, request.clone()",
        |s, src| s.gui = src,
        &s.gui,
    );
    // B. prepare_offload から置換の腕が消える（IPC の要求が同期の腕へ落ちる）
    assert_named(
        "B 腕が消える",
        DISPATCH,
        (
            ".map(|job| OffloadJob::WorkspaceReplace(Box::new(job))),",
            ".map(|_job| unreachable!()),",
        ),
        "        } if action == \"replace\" => Some(",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
    // C. 準備の段（UI スレッド）でファイルを読む
    assert_named(
        "C 準備で読む",
        DISPATCH,
        (
            "    let buffers = workspace_replace_buffers(host, names.as_ref());\n",
            "    let buffers = workspace_replace_buffers(host, names.as_ref());\n    let _ = std::fs::read(\"/x\");\n",
        ),
        "let _ = std::fs::read(\"/x\");",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
    // D. 着地（UI スレッド）で退避から書き戻す
    assert_named(
        "D 着地で書き戻す",
        DISPATCH,
        (
            "            UndoTarget::Disk => Restore::BackupMissing,\n",
            "            UndoTarget::Disk => tako_core::workspace_replace::restore_disk(&p, b, a, &k, true),\n",
        ),
        "UndoTarget::Disk => tako_core::workspace_replace::restore_disk(",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
    // E. 取り消しの準備でパスの実体を引き直す（UI スレッドの syscall）
    assert_named(
        "E 準備で実体を引く",
        DISPATCH,
        (
            ".is_some_and(|(path, _)| path == buffer.path);",
            ".is_some_and(|(path, _)| tako_core::platform::path::canonicalize_or_self(Path::new(&path)) == file.real);",
        ),
        "canonicalize_or_self(Path::new(&path)) == file.real",
        |s, src| s.dispatch = src,
        &s.dispatch,
    );
}

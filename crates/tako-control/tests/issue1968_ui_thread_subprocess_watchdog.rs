//! master の監視（worker の状態・報告の読み取り）が UI スレッドで子プロセスを待たないかの番犬（#1968）
//!
//! # なぜ要るか
//!
//! 「tako が重い・不安定」（#1968）の本番実測で、UI スレッドが 2 系統の子プロセス待ちに
//! 塞がれていた。どちらも master が worker を監視するたびに走る:
//!
//! 1. **`OrchestratorReport` が丸ごと同期**: 器の採取（`tmux capture-pane`）・会話ログの
//!    読み取り・pid 祖先辿り（`ps`）を UI スレッドで実行していた。perf.log で 38 分に
//!    458 回・UI 専有の合計 101 秒（p50 175ms・最大 2.25 秒）
//! 2. **`OrchestratorWorkerStatus` の「UI スレッド部」が子プロセスを起こしていた**:
//!    #168 で background へ逃がしたはずの照会だが、文脈の収集
//!    （`collect_worker_status_ctx`）が `agents::has_running_children`
//!    = `tmux list-panes -a` + `ps` を呼んでいた。`prepare_offload` は計測区間の外なので
//!    perf.log にも出ず、UI ストールは「再開経路（タイマー / キュー）側の遅延」と
//!    誤分類されていた（本番の `sample` でメインスレッドの 10 秒中約 2 秒が `poll`）
//!
//! # 何を縛るか
//!
//! 1. UI スレッド部（`collect_worker_status_ctx` / `collect_report_ctx` /
//!    `verify_ctx_pane_identity`）が子プロセス・ファイル走査の口を呼ばない
//! 2. `prepare_offload` が `OrchestratorReport` を background のジョブ（`OffloadJob::Report`）にする
//! 3. 子プロセスの有無は後段（`probe_running_children`）が埋め、background と同期の
//!    **両経路**がそれを通る（落とすと busy / stalled の判定が静かに壊れる）
//! 4. tako-app の IPC ループが `prepare_offload` を計測区間で包む（再発したら perf.log に出る）
//! 5. UI ストールの記録が「直前に終わった区間」（`recent_spans_within`）を分類へ渡す
//!    （監視ループ自身が UI スレッドで走るので、記録の時点で塞いでいた区間は必ず終わっている。
//!    渡さないと tako の専有が「再開経路の遅延」と誤分類される = 本番で 38 分に 331 件）
//! 6. stale binary 検知の保険の探索（`which_claude` = ログインシェルの `command -v claude`）が
//!    結果の覚え（`memoized_find`）を通る（PATH の痩せた `.app` では 2 秒ごとの指紋取りの
//!    たびにシェルが起動していた = 本番で 20 秒に約 6 本）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で
//! 窓が採れていることを固定し、[`逆戻りを名指しできる`] で**修正前を再現した注入**が
//! file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const STALE: &str = "crates/tako-control/src/stale_binary.rs";

/// UI スレッドで走る収集関数（`prepare_offload` から呼ばれる / 同期経路でも同じ）
const UI_COLLECTORS: &[&str] = &[
    "fn collect_worker_status_ctx(",
    "fn collect_report_ctx(",
    "fn verify_ctx_pane_identity(",
];

/// UI スレッドで呼んではいけない口（子プロセス・会話ログの走査）。
/// 右は名指しの理由
const FORBIDDEN_ON_UI: &[(&str, &str)] = &[
    (
        "agents::has_running_children",
        "`tmux list-panes -a` + `ps` を起こす",
    ),
    ("backend_pane_pids(", "`tmux list-panes -a` を起こす"),
    ("process_parent_map(", "`ps` を起こす"),
    ("Command::new(", "子プロセスを起こす"),
    ("capture_scrollback_joined(", "`tmux capture-pane` を起こす"),
    ("detached_capture(", "器の採取の口（子プロセス）"),
    ("session_alive(", "`tmux has-session` を起こす"),
    (
        "resolve_session_id_for_backend",
        "pid 祖先辿り（`tmux` + `ps`）",
    ),
    (
        "resolve_session_id_for_pane_via_host(",
        "pid 祖先辿り（`tmux` + `ps`）",
    ),
    ("transcript::", "会話ログのファイル走査"),
    ("codex_session::", "codex の実況のファイル走査"),
    ("agy_session::", "agy の実況のファイル走査"),
];

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
    // テスト領域を空白へ潰す（行番号は保たれる。#1420）
    production_range::production(&src, rel)
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
    line.trim_start().starts_with("//")
}

/// 窓の中で `needle` を含む**コードの**行（0-based の相対位置）をすべて
fn code_lines_with(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };

    // 1. UI スレッド部が子プロセス・ファイル走査を呼ばない
    for head in UI_COLLECTORS {
        match fn_window(src, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                for (needle, why) in FORBIDDEN_ON_UI {
                    for i in code_lines_with(&window, needle) {
                        push(
                            at + i,
                            format!(
                                "UI スレッド部 `{}` が `{needle}` を呼んでいる（{why}。\
                                 master の監視のたびに窓が止まる = #1968）",
                                head.trim_start_matches("fn ").trim_end_matches('(')
                            ),
                        );
                    }
                }
            }
        }
    }

    // 2. 報告は background のジョブにする
    match fn_window(src, "pub fn prepare_offload(") {
        None => push(0, "`prepare_offload` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            // 部分一致で別名の腕（`OrchestratorReportX`）を拾わないよう `{` まで見る
            let arm = code_lines_with(&window, "Request::OrchestratorReport {");
            let job = code_lines_with(&window, "OffloadJob::Report");
            if arm.is_empty() || job.is_empty() {
                push(
                    at,
                    "`prepare_offload` が `OrchestratorReport` を background のジョブ \
                     （`OffloadJob::Report`）にしていない（器の採取と会話ログの読み取りが \
                     UI スレッドで走る。本番で 38 分に UI 専有 101 秒 = #1968）"
                        .into(),
                );
            }
        }
    }

    // 3. 子プロセスの有無は後段が埋める（埋める口が生きていて、両経路が通る）
    match fn_window(src, "fn probe_running_children(") {
        None => push(
            0,
            "`probe_running_children` が無い（子プロセスの有無を埋める後段が消えた = \
             busy / stalled の判定が常に「子なし」になる）"
                .into(),
        ),
        Some((at, window)) => {
            if code_lines_with(&window, "agents::has_running_children").is_empty() {
                push(
                    at,
                    "`probe_running_children` が `agents::has_running_children` を呼んでいない \
                     （子プロセスの有無が埋まらない）"
                        .into(),
                );
            }
        }
    }
    // 呼び出し箇所 = 定義以外。background（`run_reply`）と同期経路（`dispatch_inner`）の 2 つ
    let calls: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            !is_comment(l)
                && l.contains("probe_running_children(")
                && !l.contains("fn probe_running_children(")
        })
        .map(|(i, _)| i + 1)
        .collect();
    for (head, place) in [
        ("fn run_reply(", "background の後段（`run_reply`）"),
        ("fn dispatch_inner(", "同期経路（`dispatch_inner`）"),
    ] {
        match fn_window(src, head) {
            None => push(0, format!("`{head}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                let end = at + window.len();
                let inside = calls.iter().any(|&l| l >= at && l < end);
                let status_at = code_lines_with(&window, "finish_worker_status(")
                    .first()
                    .map(|i| at + i)
                    .unwrap_or(at);
                if !inside {
                    push(
                        status_at,
                        format!(
                            "{place} が `probe_running_children` を通さずに \
                             `finish_worker_status` を呼んでいる（UI 部で数えるのをやめたので、\
                             ここで埋めないと `has_running_children` が常に false）"
                        ),
                    );
                }
            }
        }
    }
    out
}

fn scan_main(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let calls: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains("prepare_offload(app, &incoming.request)"))
        .map(|(i, _)| i)
        .collect();
    if calls.is_empty() {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "IPC ループの `prepare_offload(app, &incoming.request)` が見つからない\
                  （走査が空振り）"
                .into(),
        });
    }
    for i in calls {
        // 同じ closure の中（直前 10 行）に計測区間があるか
        let from = i.saturating_sub(10);
        let spanned = lines[from..i]
            .iter()
            .any(|l| !is_comment(l) && l.contains("perf_span(") && l.contains("format!("))
            && lines[from..i]
                .iter()
                .any(|l| l.contains("\"offload_prepare:{}\""));
        if !spanned {
            out.push(Offender {
                file: MAIN,
                line: i + 1,
                why: "IPC ループが `prepare_offload` を計測区間（`offload_prepare:<種別>`）で \
                      包んでいない（UI スレッド部が子プロセスを待っても perf.log に出ず、\
                      UI ストールが誤分類される = #1968）"
                    .into(),
            });
        }
    }
    out
}

/// 5. UI ストールの記録が直前の区間の履歴を渡しているか
fn scan_stall_classify(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let Some(call) = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains("diag::classify_stall("))
    else {
        return vec![Offender {
            file: MAIN,
            line: 0,
            why: "UI ストールの記録（`diag::classify_stall(`）が見つからない（走査が空振り）"
                .into(),
        }];
    };
    let from = call.saturating_sub(12);
    let to = (call + 8).min(lines.len());
    let passes = lines[from..to]
        .iter()
        .any(|l| !is_comment(l) && l.contains("recent_spans_within("));
    if passes {
        Vec::new()
    } else {
        vec![Offender {
            file: MAIN,
            line: call + 1,
            why: "UI ストールの分類へ直前に終わった区間（`recent_spans_within`）を渡していない \
                  （記録の時点で塞いでいた区間は終わっているので、tako の専有が \
                  「再開経路の遅延」と誤分類される = #1968）"
                .into(),
        }]
    }
}

/// 6. 保険の探索が覚えを通るか
fn scan_stale(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn which_claude(") {
        None => out.push(Offender {
            file: STALE,
            line: 0,
            why: "`which_claude` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_lines_with(&window, "memoized_find(").is_empty() {
                let i = code_lines_with(&window, "exe::find(")
                    .first()
                    .copied()
                    .unwrap_or(0);
                out.push(Offender {
                    file: STALE,
                    line: at + i,
                    why: "保険の探索（ログインシェルの `command -v claude`）が覚え（`memoized_find`）を \
                          通っていない（PATH の痩せた `.app` では 2 秒ごとの指紋取りのたびに \
                          シェルが起動する = #1968）"
                        .into(),
                });
            }
        }
    }
    out
}

fn all_offenders(dispatch: &str, main: &str) -> Vec<Offender> {
    let mut v = scan_dispatch(dispatch);
    v.extend(scan_main(main));
    v.extend(scan_stall_classify(main));
    v
}

fn fail_with(offenders: &[Offender]) -> ! {
    let list: Vec<String> = offenders.iter().map(Offender::report).collect();
    panic!(
        "UI スレッドで子プロセスを待つ形が戻っている（#1968）:\n  {}",
        list.join("\n  ")
    );
}

#[test]
fn master_の監視が_ui_スレッドで子プロセスを待たない() {
    let root = workspace_root();
    let dispatch = read(&root, DISPATCH);
    let main = read(&root, MAIN);
    let offenders = all_offenders(&dispatch, &main);
    if !offenders.is_empty() {
        fail_with(&offenders);
    }
}

#[test]
fn 保険の探索が覚えを通る() {
    let root = workspace_root();
    let stale = read(&root, STALE);
    let offenders = scan_stale(&stale);
    if !offenders.is_empty() {
        fail_with(&offenders);
    }
    // 注入: 修正前の形（毎回探す）へ戻すと名指しされる
    let legacy = stale.replacen(
        "    memoized_find(\n        &WHICH_MEMO,",
        "    let _ = (\n        &WHICH_MEMO,",
        1,
    );
    assert_ne!(legacy, stale, "注入の置換元がある");
    let hit = scan_stale(&legacy);
    assert!(
        hit.iter()
            .any(|o| o.line > 0 && o.why.contains("memoized_find")),
        "注入を名指しできない: {:?}",
        hit.iter().map(Offender::report).collect::<Vec<_>>()
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let dispatch = read(&root, DISPATCH);
    for head in UI_COLLECTORS {
        let (_, w) = fn_window(&dispatch, head).unwrap_or_else(|| panic!("{head} の窓"));
        assert!(w.len() >= 5, "{head} の窓が短すぎる（{} 行）", w.len());
    }
    // 収集関数の本体を実際に見ている（器の名前を引く行が窓に入っている）
    let (_, w) = fn_window(&dispatch, "fn collect_worker_status_ctx(").unwrap();
    assert!(
        !code_lines_with(&w, "host.backend_session(").is_empty(),
        "collect_worker_status_ctx の窓に本体が入っていない"
    );
    let (_, w) = fn_window(&dispatch, "fn collect_report_ctx(").unwrap();
    assert!(
        !code_lines_with(&w, ".backend_session(").is_empty(),
        "collect_report_ctx の窓に本体が入っていない"
    );
    let (_, w) = fn_window(&dispatch, "pub fn prepare_offload(").unwrap();
    assert!(
        !code_lines_with(&w, "Request::OrchestratorWorkerStatus").is_empty(),
        "prepare_offload の窓に既存の腕が入っていない"
    );
}

/// 置換 1 つぶんの注入（修正前の形へ戻す）
struct Injection {
    name: &'static str,
    file: &'static str,
    from: &'static str,
    to: &'static str,
    /// 名指しの理由に含まれるべき語
    expect: &'static str,
}

const INJECTIONS: &[Injection] = &[
    Injection {
        name: "収集で子プロセスを数える（修正前の collect_worker_status_ctx）",
        file: DISPATCH,
        from: "        has_running_children: false,\n        live_tail: lines.map(tail_join),",
        to: "        has_running_children: crate::agents::has_running_children(\"x\"),\n        live_tail: lines.map(tail_join),",
        expect: "agents::has_running_children",
    },
    Injection {
        name: "報告の収集で器を採取する",
        file: DISPATCH,
        from: "    ReportCtx {\n        pane_id: query.pane_id,",
        to: "    let _ = capture_scrollback_joined(\"x\", 1);\n    ReportCtx {\n        pane_id: query.pane_id,",
        expect: "capture_scrollback_joined(",
    },
    Injection {
        name: "報告の収集で session_id を pid から解決する",
        file: DISPATCH,
        from: "    ReportCtx {\n        pane_id: query.pane_id,",
        to: "    let _ = crate::agents::resolve_session_id_for_backend(\"x\");\n    ReportCtx {\n        pane_id: query.pane_id,",
        expect: "resolve_session_id_for_backend",
    },
    Injection {
        name: "報告を offload しない（prepare_offload の腕を消す）",
        file: DISPATCH,
        from: "        Request::OrchestratorReport {\n            pane_id,\n            lines,\n            messages,\n            worker,\n        } if !crate::diag::issue1968_legacy() => Some(",
        to: "        Request::OrchestratorReportGone {\n            pane_id,\n            lines,\n            messages,\n            worker,\n        } if !crate::diag::issue1968_legacy() => Some(",
        expect: "OffloadJob::Report",
    },
    Injection {
        name: "background の後段が子プロセスを数えない",
        file: DISPATCH,
        from: "                    } else {\n                        probe_running_children(ctx)\n                    },",
        to: "                    } else {\n                        ctx\n                    },",
        expect: "run_reply",
    },
    Injection {
        name: "同期経路が子プロセスを数えない",
        file: DISPATCH,
        from: "                finish_worker_status(\n                    probe_running_children(ctx),",
        to: "                finish_worker_status(\n                    ctx,",
        expect: "dispatch_inner",
    },
    Injection {
        name: "UI ストールの分類へ直前の区間を渡さない",
        file: MAIN,
        from: "tako_control::diag::recent_spans_within(Duration::from_secs(1) + lag)",
        to: "None::<tako_control::diag::RecentSpans>",
        expect: "recent_spans_within",
    },
    Injection {
        name: "IPC ループの準備を測らない",
        file: MAIN,
        from: "\"offload_prepare:{}\"",
        to: "\"offload_prep_gone:{}\"",
        expect: "offload_prepare",
    },
];

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let dispatch = read(&root, DISPATCH);
    let main = read(&root, MAIN);
    assert!(
        all_offenders(&dispatch, &main).is_empty(),
        "注入の前提: 現在のソースは緑"
    );
    for inj in INJECTIONS {
        let (d, m) = match inj.file {
            DISPATCH => {
                assert_eq!(
                    dispatch.matches(inj.from).count(),
                    1,
                    "注入「{}」の置換元がちょうど 1 か所ある",
                    inj.name
                );
                (dispatch.replacen(inj.from, inj.to, 1), main.clone())
            }
            _ => {
                assert!(
                    main.contains(inj.from),
                    "注入「{}」の置換元がある",
                    inj.name
                );
                (dispatch.clone(), main.replacen(inj.from, inj.to, 1))
            }
        };
        let offenders = all_offenders(&d, &m);
        let hit = offenders
            .iter()
            .find(|o| o.file == inj.file && o.line > 0 && o.why.contains(inj.expect));
        assert!(
            hit.is_some(),
            "注入「{}」を名指しできない（違反: {:?}）",
            inj.name,
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}

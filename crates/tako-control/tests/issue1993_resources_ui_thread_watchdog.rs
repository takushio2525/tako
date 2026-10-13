//! PC リソースの採取が UI スレッドを通らないかの番犬（#1993。エピック #1992 の S1）
//!
//! # なぜ要るか
//!
//! 全プロセスの走査は libproc だけで子プロセスを起こさないが、1 回 wall 11 ms（1,320 本・実測）で
//! UI の 1 フレームを超える。器のペインの根を引くには `tmux list-panes -a`（子プロセス）も要る。
//! 2 秒 tick や dispatch の腕から呼ぶと、#1968 / #1979 の「UI スレッドが子プロセス・重い I/O を待つ」
//! 型をそのまま持ち込む。設計（`.agent/plans/2026-10-resource-manager.md` §2.2・§4.1）は
//! 「採るのは背景スレッドと IPC の受け口だけ、UI は写して渡す / 覚えを読むだけ」。
//!
//! # 何を縛るか
//!
//! 1. GUI の受け渡し（`crates/tako-app/src/resource_panes.rs`）は写して渡すだけ
//!    （`publish_panes` を呼び、走査の口を呼ばない）
//! 2. GUI（`main.rs`）は 2 秒 tick で受け渡しを計測区間つきで呼び、起動時に採取スレッドを立てる。
//!    走査の口（`answer` / `machine_only` / B29）は GUI のどこからも呼ばない
//! 3. dispatch の `Request::Queue` の腕（UI スレッドの同期の dispatch）は覚え（`cached_status`）を読むだけ
//! 4. `ipc::submit` は `Request::Queue` を UI の受け口へ渡す**前に** `resources::answer` で答える
//! 5. `resources` の UI 向けの口（`cached_status` / `publish_panes`）は採らない。採取スレッドは別スレッド
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! 逆戻りを再現した注入が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const PUBLISH: &str = "crates/tako-app/src/resource_panes.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const IPC: &str = "crates/tako-control/src/ipc.rs";
const RESOURCES: &str = "crates/tako-control/src/resources.rs";

/// UI スレッドで呼んではいけない口（右は名指しの理由）
const SCANS: &[(&str, &str)] = &[
    ("resources::answer(", "走査を起こす（1 回 wall 11 ms）"),
    (
        "resources::machine_only(",
        "機の値を間を空けて 2 回採る（250 ms 眠る）",
    ),
    ("platform::resources::", "OS への問い合わせ（境界 B29）"),
    ("refresh_processes(", "全プロセスの走査"),
    ("refresh_machine(", "機の値の採取"),
    ("backend_pane_pids(", "`tmux list-panes -a`（子プロセス）"),
    ("process_parent_map(", "親子表の採取"),
    ("Command::new(", "子プロセスを起こす"),
    (".output()", "子プロセスを待つ"),
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

fn code_lines_with(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

fn scans_in(
    file: &'static str,
    at: usize,
    window: &[String],
    place: &str,
    out: &mut Vec<Offender>,
) {
    for (needle, why) in SCANS {
        for i in code_lines_with(window, needle) {
            out.push(Offender {
                file,
                line: at + i,
                why: format!(
                    "UI スレッドで走る {place} が `{needle}` を呼んでいる（{why}。UI は写して渡すか\
                     覚え（`cached_status`）を読むだけ = #1993 / #1968 / #1979）"
                ),
            });
        }
    }
}

struct Sources {
    publish: String,
    main: String,
    dispatch: String,
    ipc: String,
    resources: String,
}

impl Sources {
    fn load() -> Self {
        let root = workspace_root();
        Self {
            publish: read(&root, PUBLISH),
            main: read(&root, MAIN),
            dispatch: read(&root, DISPATCH),
            ipc: read(&root, IPC),
            resources: read(&root, RESOURCES),
        }
    }

    fn offenders(&self) -> Vec<Offender> {
        let mut out = scan_publish(&self.publish);
        out.extend(scan_main(&self.main));
        out.extend(scan_dispatch(&self.dispatch));
        out.extend(scan_ipc(&self.ipc));
        out.extend(scan_resources(&self.resources));
        out
    }
}

/// 1. GUI の受け渡しは写して渡すだけ
fn scan_publish(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub(crate) fn publish_resource_panes(") {
        None => out.push(Offender {
            file: PUBLISH,
            line: 0,
            why: "`publish_resource_panes` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if code_lines_with(&window, "resources::publish_panes(").is_empty() {
                out.push(Offender {
                    file: PUBLISH,
                    line: at,
                    why: "`publish_resource_panes` が `resources::publish_panes` へ渡していない \
                          （採取がペインを知らず、全部が「tako の外」になる）"
                        .into(),
                });
            }
        }
    }
    // ファイル全体（UI スレッドのモジュール）で走査の口を呼ばない
    let all: Vec<String> = src.lines().map(str::to_string).collect();
    scans_in(
        PUBLISH,
        1,
        &all,
        "ペインの受け渡し（`resource_panes.rs`）",
        &mut out,
    );
    out
}

/// 2. GUI は 2 秒 tick で受け渡し・起動時に採取スレッド。走査の口は呼ばない
fn scan_main(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let call = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains("app.publish_resource_panes()"));
    match call {
        None => {
            // 行は 2 秒 tick の計測区間（受け渡しの区間 > tick 全体の区間）を指す
            let line = lines
                .iter()
                .position(|l| l.contains("\"periodic_prep:resource_panes\""))
                .or_else(|| {
                    lines
                        .iter()
                        .position(|l| l.contains("perf_span(\"periodic_prep\")"))
                })
                .map(|i| i + 1)
                .unwrap_or(0);
            out.push(Offender {
                file: MAIN,
                line,
                why: "2 秒 tick が `app.publish_resource_panes()` を呼んでいない（ペインの一覧が \
                      採取へ届かず、帰属が全部「tako の外」になる）"
                    .into(),
            });
        }
        Some(i) => {
            let from = i.saturating_sub(4);
            let spanned = lines[from..i]
                .iter()
                .any(|l| l.contains("perf_span(\"periodic_prep:resource_panes\")"));
            if !spanned {
                out.push(Offender {
                    file: MAIN,
                    line: i + 1,
                    why: "受け渡しが計測区間（`periodic_prep:resource_panes`）で包まれていない \
                          （UI の専有が増えても perf.log に出ない）"
                        .into(),
                });
            }
        }
    }
    if !lines
        .iter()
        .any(|l| !is_comment(l) && l.contains("tako_control::resources::start()"))
    {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "起動時に採取スレッド（`tako_control::resources::start()`）を立てていない \
                  （status のたびに受け口のスレッドで採ることになり、走査の間隔が守られない）"
                .into(),
        });
    }
    let all: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for (needle, why) in &SCANS[..3] {
        for i in code_lines_with(&all, needle) {
            out.push(Offender {
                file: MAIN,
                line: 1 + i,
                why: format!(
                    "GUI が `{needle}` を呼んでいる（{why}。GUI の中は UI スレッドの経路が多く、\
                     覚えは `cached_status` で読む = #1993）"
                ),
            });
        }
    }
    out
}

/// 3. dispatch の腕は覚えを読むだけ
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains("Request::Queue { action } =>"))
    else {
        return vec![Offender {
            file: DISPATCH,
            line: 0,
            why: "dispatch の `Request::Queue` の腕が見つからない（走査が空振り）".into(),
        }];
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.starts_with("        Request::") || l.trim().is_empty())
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    let window: Vec<String> = lines[start..end].iter().map(|l| l.to_string()).collect();
    let mut out = Vec::new();
    if code_lines_with(&window, "resources::cached_status(").is_empty() {
        out.push(Offender {
            file: DISPATCH,
            line: start + 1,
            why: "`Request::Queue` の腕が覚え（`resources::cached_status`）を読んでいない".into(),
        });
    }
    scans_in(
        DISPATCH,
        start + 1,
        &window,
        "dispatch の `Request::Queue` の腕",
        &mut out,
    );
    out
}

/// 4. `ipc::submit` は UI の受け口へ渡す前に答える
fn scan_ipc(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "pub fn submit(") else {
        return vec![Offender {
            file: IPC,
            line: 0,
            why: "`ipc::submit` が見つからない（走査が空振り）".into(),
        }];
    };
    let answer = code_lines_with(&window, "resources::answer(")
        .first()
        .copied();
    let queue = code_lines_with(&window, "Request::Queue").first().copied();
    let send = code_lines_with(&window, "tx.unbounded_send(")
        .first()
        .copied();
    match (queue, answer, send) {
        (Some(q), Some(a), Some(s)) if q < s && a < s => Vec::new(),
        (_, _, None) => vec![Offender {
            file: IPC,
            line: at,
            why: "`submit` に UI の受け口への送り（`tx.unbounded_send(`）が無い（走査が空振り）".into(),
        }],
        (_, _, Some(s)) => vec![Offender {
            file: IPC,
            line: at + s,
            why: "`submit` が `Request::Queue` を `resources::answer` で答える前に UI の受け口へ \
                  渡している（UI が固まると status も返らない。走査が UI スレッドの dispatch に乗る）"
                .into(),
        }],
    }
}

/// 5. `resources` の UI 向けの口は採らない・採取は別スレッド
fn scan_resources(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for head in ["pub fn cached_status(", "pub fn publish_panes("] {
        match fn_window(src, head) {
            None => out.push(Offender {
                file: RESOURCES,
                line: 0,
                why: format!("`{head}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                let place = format!(
                    "`{}`",
                    head.trim_start_matches("pub fn ").trim_end_matches('(')
                );
                scans_in(RESOURCES, at, &window, &place, &mut out);
                // 自前の名前（b29 = 境界 B29 の別名）も見る
                for i in code_lines_with(&window, "b29::") {
                    out.push(Offender {
                        file: RESOURCES,
                        line: at + i,
                        why: format!("UI 向けの口 {place} が境界 B29（`b29::`）へ問い合わせている"),
                    });
                }
            }
        }
    }
    match fn_window(src, "pub fn start(") {
        None => out.push(Offender {
            file: RESOURCES,
            line: 0,
            why: "`resources::start` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let threaded = !code_lines_with(&window, "std::thread::Builder").is_empty()
                && !code_lines_with(&window, ".spawn(").is_empty();
            if !threaded {
                out.push(Offender {
                    file: RESOURCES,
                    line: at,
                    why: "`resources::start` が採取を別スレッドで回していない（呼び手 = GUI の起動経路 \
                          = UI スレッドで採ることになる）"
                        .into(),
                });
            }
        }
    }
    out
}

#[test]
fn ui_スレッドはpcリソースを採らない() {
    let s = Sources::load();
    let offenders = s.offenders();
    assert!(
        offenders.is_empty(),
        "UI スレッドで PC リソースを採る経路がある:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::load();
    let (_, w) = fn_window(&s.publish, "pub(crate) fn publish_resource_panes(").unwrap();
    assert!(w.len() >= 10, "受け渡しの窓が短すぎる（{} 行）", w.len());
    assert!(
        !code_lines_with(&w, "child_pid()").is_empty(),
        "受け渡しの本体が窓に入っている"
    );
    let (_, w) = fn_window(&s.ipc, "pub fn submit(").unwrap();
    assert!(
        !code_lines_with(&w, "start_run(").is_empty(),
        "submit の本体が窓に入っている"
    );
    let (_, w) = fn_window(&s.resources, "pub fn cached_status(").unwrap();
    assert!(
        !code_lines_with(&w, "status_json(").is_empty(),
        "cached_status の本体が窓に入っている"
    );
    let (_, w) = fn_window(&s.resources, "pub fn start(").unwrap();
    assert!(
        !code_lines_with(&w, "sampler_tick(").is_empty(),
        "start の本体が窓に入っている"
    );
}

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
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        assert_eq!(
            src.matches(from).count(),
            1,
            "注入「{label}」の置換元がちょうど 1 か所ある"
        );
        src.replacen(from, to, 1)
    };

    // ① GUI の受け渡しの中で走査する
    let p = inject(
        &s.publish,
        "        tako_control::resources::publish_panes(entries, own_socket);",
        "        let _ = tako_control::resources::answer(None);\n        tako_control::resources::publish_panes(entries, own_socket);",
        "①",
    );
    expect_named(
        "受け渡しで走査",
        scan_publish(&p),
        PUBLISH,
        "resources::answer(",
    );

    // ② dispatch の腕で採り直す
    let d = inject(
        &s.dispatch,
        "            \"status\" => Ok(crate::resources::cached_status()),",
        "            \"status\" => crate::resources::answer(Some(\"status\")),",
        "②",
    );
    expect_named(
        "dispatch の腕で走査",
        scan_dispatch(&d),
        DISPATCH,
        "cached_status",
    );

    // ③ submit の手前の分岐を外す（UI の受け口へ流れる）
    let i = inject(
        &s.ipc,
        "    if let Request::Queue { action } = &request {",
        "    if let Some(Request::Queue { action }) = None::<Request> {",
        "③",
    );
    // 分岐の形が崩れても「答える前に UI へ渡す」順序の違反として名指しされること
    let i = i.replacen(
        "        return crate::resources::answer(action.as_deref()).map_err(SubmitError::Rejected);",
        "",
        1,
    );
    expect_named("submit の分岐を外す", scan_ipc(&i), IPC, "UI の受け口へ");

    // ④ UI 向けの覚えの読み取りで採る
    let r = inject(
        &s.resources,
        "pub fn cached_status() -> Value {\n",
        "pub fn cached_status() -> Value {\n    refresh_processes(&mut lock(scan_state()), Instant::now(), SCAN_INTERVAL);\n",
        "④",
    );
    expect_named(
        "cached_status で走査",
        scan_resources(&r),
        RESOURCES,
        "refresh_processes(",
    );

    // ⑤ 採取スレッドを立てずにその場で回す
    let r = inject(
        &s.resources,
        "    let spawned = std::thread::Builder::new()\n        .name(\"tako-resources\".into())\n        .spawn(|| loop {\n            sampler_tick(Instant::now());\n            std::thread::sleep(MACHINE_INTERVAL);\n        });",
        "    sampler_tick(Instant::now());\n    let spawned: Result<(), ()> = Ok(());",
        "⑤",
    );
    expect_named(
        "採取を別スレッドにしない",
        scan_resources(&r),
        RESOURCES,
        "別スレッド",
    );

    // ⑥ 2 秒 tick の受け渡しを外す
    let m = inject(
        &s.main,
        "                        app.publish_resource_panes();",
        "                        // 外した",
        "⑥",
    );
    expect_named(
        "2 秒 tick の受け渡しを外す",
        scan_main(&m),
        MAIN,
        "publish_resource_panes",
    );

    // ⑦ GUI が直に機の値を採る
    let m = inject(
        &s.main,
        "        tako_control::resources::start();",
        "        tako_control::resources::start();\n        let _ = tako_control::resources::machine_only(Duration::from_millis(250));",
        "⑦",
    );
    expect_named(
        "GUI が機の値を採る",
        scan_main(&m),
        MAIN,
        "resources::machine_only(",
    );
}

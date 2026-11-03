//! 順番待ちの優先度と終わりの見込み（#1996。エピック #1992 の S4）の番犬
//!
//! # 何を縛るか
//!
//! 1. 段は処理 > ペイン > 役割の順で決まり、役割の既定は表のとおり（純関数 `priority::resolve`）
//! 2. エイジングは待ち 15 分ごとに 1 段・`high` まで・同じ段の中の受付の順を崩さない（`priority::aged`）。
//!    調停役は待ちの並びの入力（`WaitingJob.priority`）にエイジングの後の段を渡している
//! 3. cargo の進みの行は素通しから外れ、それ以外の出力は 1 バイトも変わらない（`eta::ProgressFilter`）。
//!    wrapper は端末の stderr・利用者が決めた `CARGO_TERM_PROGRESS_WHEN`・cargo でない子では横取りしない
//! 4. 過去の記録（`<data_dir>/resources/history.jsonl`）に生の argv・パスが載らない: 書く口
//!    （`queue_history::append`）が行を検査（`row_leak`）してから書き、外れた行は欄を名指して断る
//!
//! 2 と 4 の「配線されている」はソースを読んで確かめ、[`逆戻りを名指しできる`] で配線を外した注入が
//! file:line で名指されることを確かめる。

use std::path::{Path, PathBuf};

use tako_control::queue_history::{row_leak, Leak};
use tako_core::queue::eta::{Progress, ProgressFilter};
use tako_core::queue::priority::{aged, resolve, CallerRole, PriorityFrom, PriorityInput};
use tako_core::queue::{order_waiting, HistoryRow, JobKind, Priority, WaitingJob};

const QUEUE: &str = "crates/tako-control/src/queue.rs";
const HISTORY: &str = "crates/tako-control/src/queue_history.rs";
const WRAPPER: &str = "crates/tako-control/src/queue_wrapper.rs";

const MIN: u64 = 60 * 1000;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

#[test]
fn 段は処理とペインと役割の順で決まる() {
    let worker = PriorityInput {
        in_pane: true,
        role: Some(CallerRole::Worker),
        ..PriorityInput::default()
    };
    assert_eq!(resolve(&worker), (Priority::Normal, PriorityFrom::Role));
    let pane = PriorityInput {
        pane_default: Some(Priority::Top),
        ..worker
    };
    assert_eq!(resolve(&pane), (Priority::Top, PriorityFrom::Pane));
    let job = PriorityInput {
        job: Some(Priority::Low),
        ..pane
    };
    assert_eq!(resolve(&job), (Priority::Low, PriorityFrom::Job));
    // 役割の既定の表
    let role = |in_pane, role, tty| {
        resolve(&PriorityInput {
            in_pane,
            role,
            tty,
            ..PriorityInput::default()
        })
        .0
    };
    assert_eq!(role(false, None, true), Priority::Low, "ペインの外");
    assert_eq!(role(true, Some(CallerRole::Master), false), Priority::High);
    assert_eq!(role(true, Some(CallerRole::Solo), false), Priority::High);
    assert_eq!(role(true, None, true), Priority::Top, "人が直接");
    assert_eq!(role(true, None, false), Priority::Normal);
}

fn waiting(id: u64, at: u64, priority: Priority) -> WaitingJob {
    WaitingJob {
        id,
        estimate_mb: 1024,
        target_dir: None,
        priority,
        enqueued_at_ms: at,
        overtaken: 0,
    }
}

#[test]
fn エイジングは15分ごとに1段でhighまで並びに効く() {
    assert_eq!(aged(Priority::Low, 15 * MIN - 1), Priority::Low);
    assert_eq!(aged(Priority::Low, 15 * MIN), Priority::Normal);
    assert_eq!(aged(Priority::Low, 30 * MIN), Priority::High);
    assert_eq!(
        aged(Priority::Low, 24 * 60 * MIN),
        Priority::High,
        "top へは上げない"
    );
    assert_eq!(aged(Priority::Top, 0), Priority::Top);
    // 時計は受付の時刻（データ）で注入する: 今 = 100 分。31 分待った low は 20 分待った normal（→ high）と
    // 同じ high になり、受付の古い方が先
    let now = 100 * MIN;
    let jobs = [
        (1, now - 20 * MIN, Priority::Normal),
        (2, now - 31 * MIN, Priority::Low),
        (3, now, Priority::Top),
        (4, now - MIN, Priority::Normal),
    ];
    let w: Vec<WaitingJob> = jobs
        .iter()
        .map(|(id, at, p)| waiting(*id, *at, aged(*p, now - at)))
        .collect();
    let ids: Vec<u64> = order_waiting(&w).iter().map(|j| j.id).collect();
    assert_eq!(ids, vec![3, 2, 1, 4]);
}

#[test]
fn 進みの行だけが素通しから外れる() {
    let input: &[u8] = b"   Compiling a v0.1.0 (/x/a)\n    Building [=====>   ] 3/9: a(build)\r\
        warning: unused\n    Building [========>] 8/9: b                \r\
        test result: ok\r\n    Finished `dev` profile in 3.17s\n";
    let mut f = ProgressFilter::new();
    let mut out = f.feed(input);
    out.extend(f.finish());
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "   Compiling a v0.1.0 (/x/a)\nwarning: unused\ntest result: ok\r\n    Finished `dev` profile in 3.17s\n"
    );
    assert_eq!(f.last, Some(Progress { done: 8, total: 9 }));
}

#[test]
fn wrapperは端末と利用者の設定とcargoでない子では横取りしない() {
    use tako_control::queue_wrapper::{progress_env, reads_cargo_progress};
    let argv = |s: &str| -> Vec<String> { s.split(' ').map(str::to_string).collect() };
    let unset = |_: &str| false;
    assert!(reads_cargo_progress(
        &argv("cargo test -p tako-core"),
        false,
        &unset
    ));
    assert!(
        !reads_cargo_progress(&argv("cargo test"), true, &unset),
        "端末"
    );
    assert!(
        !reads_cargo_progress(&argv("cargo build"), false, &|k| k
            == "CARGO_TERM_PROGRESS_WHEN"),
        "利用者が決めた"
    );
    assert!(
        !reads_cargo_progress(&argv("cargo run"), false, &unset),
        "run は外す"
    );
    assert!(!reads_cargo_progress(
        &argv("bash scripts/x.sh"),
        false,
        &unset
    ));
    assert_eq!(
        progress_env(&unset),
        vec![
            ("CARGO_TERM_PROGRESS_WHEN", "always"),
            ("CARGO_TERM_PROGRESS_WIDTH", "80")
        ]
    );
    assert_eq!(
        progress_env(&|k| k == "CARGO_TERM_PROGRESS_WIDTH"),
        vec![("CARGO_TERM_PROGRESS_WHEN", "always")],
        "幅は利用者のまま"
    );
}

fn good_row() -> HistoryRow {
    HistoryRow {
        at: 1,
        key: "0123456789abcdef".into(),
        shape: Some("fedcba9876543210".into()),
        name: "cargo test".into(),
        kind: JobKind::Test,
        peak_mb: 1,
        ran_s: 10,
        exit_code: Some(0),
        units: Some(9),
        ..HistoryRow::default()
    }
}

/// 注入: (名指されるはずの欄, 行の崩し方)
type Inject = (&'static str, fn(&mut HistoryRow));

#[test]
fn 生のargvとパスの載った行は欄を名指して書かない() {
    assert_eq!(row_leak(&good_row()), None);
    let inject: &[Inject] = &[
        ("key", |r| r.key = "cargo test --test issue_private".into()),
        ("shape", |r| r.shape = Some("/home/someone/proj".into())),
        ("name", |r| r.name = "/opt/tools/bin/cargo".into()),
        ("name", |r| r.name = "SECRET_TOKEN=abc cargo".into()),
        ("name", |r| r.name = "cargo test --manifest-path".into()),
    ];
    let dir = tako_core::test_residue::ScratchDir::new("watchdog-1996-history");
    for (field, f) in inject {
        let mut row = good_row();
        f(&mut row);
        assert!(
            matches!(row_leak(&row), Some(Leak { field: got, .. }) if got == *field),
            "注入した {field} を名指していない: {row:?}"
        );
        let err = tako_control::queue_history::append(&dir, &row).unwrap_err();
        assert!(err.contains(field), "断りが欄を名指していない: {err}");
    }
    assert!(
        !dir.join(tako_control::queue_history::FILE).exists(),
        "外れた行を 1 行も書いていない"
    );
    // 正しい行は書ける（検査が書く口を塞いでいない）
    tako_control::queue_history::append(&dir, &good_row()).unwrap();
    let text = std::fs::read_to_string(dir.join(tako_control::queue_history::FILE)).unwrap();
    assert!(!text.contains('/'), "{text}");
}

// ===== 配線（ソースを読む）=====

/// 名前が `name` の関数の本体（次のトップレベルの関数の頭まで）の (開始行, 行)。関数の頭は
/// `tako_core::source_scan::fn_head_name` で読む（`pub(crate) fn` / `async fn` も頭と見なす = #1496）
fn fn_window<'a>(src: &'a str, name: &str) -> Option<(usize, Vec<&'a str>)> {
    use tako_core::source_scan::fn_head_name;
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| fn_head_name(l) == Some(name))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| {
            !l.starts_with(' ')
                && (fn_head_name(l).is_some()
                    || l.starts_with("#[cfg(test)]")
                    || l.starts_with("mod "))
        })
        .map_or(lines.len(), |(i, _)| i);
    Some((start + 1, lines[start..end].to_vec()))
}

fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// 配線の検査。名指し（`file:line 理由`）の一覧
fn scan(queue: &str, history: &str, wrapper: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 2: 待ちの並びの入力はエイジングの後の段
    match fn_window(queue, "waiting_jobs") {
        None => out.push(format!(
            "{QUEUE}:0 `waiting_jobs` が見つからない（走査が空振り）"
        )),
        Some((at, w)) => {
            let line = w
                .iter()
                .position(|l| code(l).trim_start().starts_with("priority:"));
            match line {
                Some(i) if code(w[i]).contains("effective_priority(") => {}
                Some(i) => out.push(format!(
                    "{QUEUE}:{} 待ちの並びへ渡す段がエイジングの後ではない（`effective_priority` を通していない）",
                    at + i
                )),
                None => out.push(format!("{QUEUE}:{at} `WaitingJob` の priority を渡していない（走査が空振り）")),
            }
        }
    }
    // 4: 書く口は検査してから書く
    match fn_window(history, "append") {
        None => out.push(format!(
            "{HISTORY}:0 `append` が見つからない（走査が空振り）"
        )),
        Some((at, w)) => {
            let check = w.iter().position(|l| code(l).contains("row_leak("));
            let write = w.iter().position(|l| code(l).contains("write_all("));
            match (check, write) {
                (Some(c), Some(wr)) if c < wr => {}
                (_, None) => out.push(format!(
                    "{HISTORY}:{at} `append` が書いていない（走査が空振り）"
                )),
                (_, Some(wr)) => out.push(format!(
                    "{HISTORY}:{} 過去の記録を検査（`row_leak`）の前に書いている",
                    at + wr
                )),
            }
        }
    }
    // 3: wrapper は自分の stderr が端末かを見てから進みを読む
    let wired = wrapper
        .lines()
        .enumerate()
        .find(|(_, l)| code(l).contains("reads_cargo_progress(argv"));
    match wired {
        Some((_, l)) if code(l).contains("stderr().is_terminal()") => {}
        Some((i, _)) => out.push(format!(
            "{WRAPPER}:{} 進みを読むかの判断に自分の stderr が端末かを渡していない（端末で横取りする）",
            i + 1
        )),
        None => out.push(format!("{WRAPPER}:0 `run_with` が進みを読むかを判断していない（走査が空振り）")),
    }
    out
}

#[test]
fn 調停役と記録とwrapperが配線されている() {
    let found = scan(&read(QUEUE), &read(HISTORY), &read(WRAPPER));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 逆戻りを名指しできる() {
    let queue = read(QUEUE);
    let history = read(HISTORY);
    let wrapper = read(WRAPPER);
    // 2: 受付の段をそのまま渡す（S2 の形）
    let q2 = queue.replacen(
        "priority: j.effective_priority(wall),",
        "priority: j.wire.priority,",
        1,
    );
    assert_ne!(q2, queue, "注入が当たらない（ソースの形が変わった）");
    let found = scan(&q2, &history, &wrapper);
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(&format!("{QUEUE}:")) && f.contains("エイジング")),
        "{found:?}"
    );
    // 4: 検査を外す
    let h2 = history.replacen(
        "    if let Some(leak) = row_leak(row) {",
        "    if let Some(leak) = None::<Leak> {",
        1,
    );
    assert_ne!(h2, history, "注入が当たらない（ソースの形が変わった）");
    let found = scan(&queue, &h2, &wrapper);
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(&format!("{HISTORY}:")) && f.contains("row_leak")),
        "{found:?}"
    );
    // 3: 端末かを見ない
    let w2 = wrapper.replacen(
        "std::io::stderr().is_terminal(), already_set",
        "false, already_set",
        1,
    );
    assert_ne!(w2, wrapper, "注入が当たらない（ソースの形が変わった）");
    let found = scan(&queue, &history, &w2);
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(&format!("{WRAPPER}:")) && f.contains("端末")),
        "{found:?}"
    );
    // 名指しは行番号つき（0 = 空振りではない）
    for f in scan(&q2, &h2, &w2) {
        let line: usize = f
            .split(':')
            .nth(1)
            .and_then(|s| s.split(' ').next()?.parse().ok())
            .unwrap_or(0);
        assert!(line > 0, "行番号が無い: {f}");
    }
}

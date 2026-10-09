//! LSP の e2e の「読み込み中・起動中の待ち」の番犬（#1922）
//!
//! # なぜ要るか
//!
//! 偽の言語サーバ（`tako-lsp-fake`）を起こす e2e が、Windows の CI でだけ間欠的に落ちていた。
//! 真因は 3 つとも「テストが前提にした順序を実時間の速さに任せていた」で、詳細と実測は
//! `tests/common/lsp_fake_e2e.rs` の冒頭にある:
//!
//! 1. manager が読み込み中（`quiescent: false`）と知る前に要求を送る → `waited_for_loading` が `None`
//! 2. 読み込みが要求より先に終わる（読み込みの長さを `--loading-ms` の実時間で決めていた）
//! 3. 上限つきの要求をサーバの起動ごと測る → 起動が上限を越えて `starting: true`
//!
//! どれも**1 行で戻せる**のに、戻しても macOS ではほぼ緑のまま（Windows のランナーでだけ
//! 間欠的に赤）。ここで e2e の**形**を縛り、戻したら file:line で名指す。
//!
//! # 縛ること
//!
//! - [`読み込みの終わりを実時間で決めていない`] — e2e は `--loading-ms` / `TAKO_LSP_FAKE_LOADING_MS` を
//!   書かない（`LoadingGate::args` の 1 実装だけが渡す）。`loading` シナリオを起こすファイルは
//!   `LoadingGate` を通す
//! - [`前提の待ちを通ってから要求を送る`] — 「読み込み中に送った」を前提にする検査
//!   （`waited_for_loading.is_some()` / `::Loading {` / `server_loading(…)` が真）は、その直前の要求より
//!   前に `wait_loading_known(` を、「起動の後の経路」を前提にする検査（`starting: false`）は
//!   `wait_running(` か `wait_loading_known(` を置く（同じ manager を作った後）
//! - [`待ちの印が意味を保っている`] — 偽サーバは `quiescent: false` を読み込みのループの中で
//!   送る（別スレッドから送ると「診断が数に載った = 知らせも処理済み」が崩れる）・
//!   `wait_loading_known` は診断の数を見る・合図は `--loading-until`・組み込みの知らせの遅れは
//!   `READY_POLL` より長い（待ちを抜けば**どの機でも必ず**落ちる）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で目印が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const TESTS: &str = "crates/tako-control/tests";
const FAKE: &str = "crates/tako-control/src/bin/tako-lsp-fake.rs";
const COMMON: &str = "crates/tako-control/tests/common/lsp_fake_e2e.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 偽サーバを起こす LSP の e2e（`tests/issue*_lsp*.rs`。番犬は除く）
fn e2e_files() -> Vec<String> {
    let dir = workspace_root().join(TESTS);
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{TESTS} が読める: {e}"))
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.starts_with("issue")
                && name.contains("_lsp")
                && name.ends_with(".rs")
                && !name.contains("watchdog")
        })
        .map(|name| format!("{TESTS}/{name}"))
        .collect();
    files.sort();
    files
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line} — {why}")
}

/// 要求を送る呼び出し（manager の問い合わせ）
const REQUESTS: [&str; 4] = [".completion(&", ".hover(&", ".goto(&", ".format(&"];

/// 前提の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Needs {
    /// 「読み込み中に送った」= manager が読み込み中と知ってから送る
    LoadingKnown,
    /// 「起動の後の経路」= 起動済みを待ってから送る
    Running,
}

impl Needs {
    fn barriers(self) -> &'static [&'static str] {
        match self {
            Needs::LoadingKnown => &["wait_loading_known("],
            Needs::Running => &["wait_running(", "wait_loading_known("],
        }
    }
}

/// 行が前提を要る検査か（コメントを落とした行で見る）
fn needs_of(line: &str) -> Option<Needs> {
    let loading_assert = line.contains("assert!(")
        && line.contains(".server_loading(")
        && !line.contains("assert!(!");
    if line.contains("waited_for_loading.is_some()")
        || line.contains("::Loading {")
        || loading_assert
    {
        return Some(Needs::LoadingKnown);
    }
    line.contains("starting: false").then_some(Needs::Running)
}

/// 合図の引数を足す字面（`LoadingGate::args` の 1 実装を通す）
const GATE_ARGS: &str = ".gate().args()";

/// 0 桁目の関数の窓（宣言行と閉じ括弧の 0 起点の添字）
fn window_from(lines: &[&str], head: usize) -> (usize, usize) {
    let end = (head + 1..lines.len())
        .find(|&j| lines[j] == "}")
        .unwrap_or(lines.len() - 1);
    (head, end)
}

/// `at` 行を含む 0 桁目の関数の窓（頭の判定は `source_scan` の 1 実装 = #1496）
fn enclosing_fn(lines: &[&str], at: usize) -> Option<(usize, usize)> {
    let head = (0..=at).rev().find(|&j| is_top_level_fn_head(lines[j]))?;
    let window = window_from(lines, head);
    (window.1 >= at).then_some(window)
}

/// `fn <name>(` の窓
fn fn_named(lines: &[&str], name: &str) -> Option<(usize, usize)> {
    let at = lines
        .iter()
        .position(|l| is_top_level_fn_head(l) && fn_head_name(l) == Some(name))?;
    Some(window_from(lines, at))
}

/// `needle` の直前で開いている呼び出しの関数名（`config(&scratch, "loading"` → `config`）
fn callee_before(line: &str, needle: &str) -> Option<String> {
    let before = &line[..line.find(needle)?];
    let open = before.rfind('(')?;
    let name: String = before[..open]
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!name.is_empty()).then_some(name)
}

/// `#[test]` の関数の窓（宣言行の 0 起点の添字と、閉じ括弧までの行の範囲）
fn test_windows(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let Some(head) = (i + 1..lines.len()).find(|&j| is_top_level_fn_head(lines[j])) else {
            continue;
        };
        out.push(window_from(lines, head));
    }
    out
}

/// 1 ファイルの検査。見つけた目印の数も返す（空振りの検査に使う）
fn scan_e2e(rel: &str, src: &str) -> (Vec<String>, usize) {
    let view = code_view::without_comments_checked(src, rel);
    let lines: Vec<&str> = view.lines().collect();
    let mut out = Vec::new();
    // 1. 読み込みの長さを実時間で決めない
    for (i, line) in lines.iter().enumerate() {
        if line.contains("\"--loading-ms\"") || line.contains("TAKO_LSP_FAKE_LOADING_MS") {
            out.push(report(
                rel,
                i + 1,
                "偽サーバの読み込みの長さを実時間で渡している（遅い機では要求より先に済む = #1922 の 2）。\
                 `LoadingGate::args` を渡し、要求が届いてから `finish` する",
            ));
        }
    }
    // `loading` を起こす行は、その行の関数か、その行が呼ぶ config の関数が合図の引数を足す
    for (i, line) in lines.iter().enumerate() {
        let launches = line.contains("\"loading\"")
            && ["config(", "config_with(", ".to_string()"]
                .iter()
                .any(|call| line.contains(call));
        if !launches {
            continue;
        }
        let gated = |window: Option<(usize, usize)>| {
            window.is_some_and(|(from, to)| (from..=to).any(|j| lines[j].contains(GATE_ARGS)))
        };
        let called = callee_before(line, "\"loading\"").and_then(|name| fn_named(&lines, &name));
        if !gated(enclosing_fn(&lines, i)) && !gated(called) {
            out.push(report(
                rel,
                i + 1,
                "`loading` シナリオを合図（`LoadingGate`）を通らずに起こしている（読み込みが既定の \
                 0.8 秒で勝手に終わる = #1922 の 2）。偽サーバの引数へ `gate().args()` を足す",
            ));
        }
    }
    // 2. 前提の待ちを通ってから要求を送る
    let mut anchors = 0;
    for (head, end) in test_windows(&lines) {
        for at in head..=end {
            let Some(needs) = needs_of(lines[at]) else {
                continue;
            };
            anchors += 1;
            let Some(asked) = (head..at)
                .rev()
                .find(|&j| REQUESTS.iter().any(|r| lines[j].contains(r)))
            else {
                out.push(report(
                    rel,
                    at + 1,
                    "前提を要る検査の前に要求が見つからない（走査の取り違え）",
                ));
                continue;
            };
            let made = (head..asked)
                .rev()
                .find(|&j| lines[j].contains("LspManager::new("))
                .unwrap_or(head);
            let waited =
                (made..asked).any(|j| needs.barriers().iter().any(|b| lines[j].contains(b)));
            if !waited {
                let why = match needs {
                    Needs::LoadingKnown => {
                        "読み込み中と知る前に要求を送っている（manager が `quiescent: false` を処理する前に\
                         送ると、待たされても `waited_for_loading` が載らない = #1922 の 1）。\
                         要求の前に `lsp_fake_e2e::wait_loading_known` を置く"
                    }
                    Needs::Running => {
                        "上限つきの要求をサーバの起動ごと測っている（遅い機では起動が上限を越えて \
                         `starting: true` になる = #1922 の 3）。要求の前に `lsp_fake_e2e::wait_running` を置く"
                    }
                };
                out.push(report(
                    rel,
                    asked + 1,
                    &format!("{why}（検査は {}:{}）", rel, at + 1),
                ));
            }
        }
    }
    (out, anchors)
}

/// 関数の窓（宣言行の 1 起点の行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(view: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = view.lines().collect();
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

/// 偽サーバの検査: `loading` の読み込み中の知らせは読み込みのループの中で送る（別スレッドから
/// 送らない）・didOpen へ診断を返す・合図を読む
fn scan_fake(src: &str) -> Vec<String> {
    let view = code_view::without_comments_checked(src, FAKE);
    let mut out = Vec::new();
    match fn_window(&view, "\"loading\" => {") {
        None => out.push(report(
            FAKE,
            0,
            "`\"loading\" =>` の腕が見つからない（走査が空振り）",
        )),
        Some((at, arm)) => {
            let busy = arm.find("\"quiescent\": false");
            let spawned = arm.find("std::thread::spawn");
            match (busy, spawned) {
                (Some(busy), Some(spawned)) if busy < spawned => {}
                _ => out.push(report(
                    FAKE,
                    at,
                    "`quiescent: false` を読み込みのループの中で（別スレッドを立てる前に）送っていない。\
                     後から届く didOpen の診断が「知らせも処理済み」の印にならない（#1922）",
                )),
            }
            if !arm.contains("until") {
                out.push(report(
                    FAKE,
                    at,
                    "読み込みの終わりが合図（`--loading-until`）を見ていない（#1922）",
                ));
            }
        }
    }
    if !view.contains("(\"textDocument/didOpen\", None) =>") {
        out.push(report(
            FAKE,
            0,
            "didOpen へ診断を返す腕が見つからない（`wait_loading_known` の印が来ない）",
        ));
    }
    if !view.contains("\"--loading-until\"") {
        out.push(report(FAKE, 0, "`--loading-until` を読んでいない"));
    }
    out
}

/// 共通部品の検査: 待ちの印・合図・組み込みの遅れ
fn scan_common(src: &str) -> Vec<String> {
    let view = code_view::without_comments_checked(src, COMMON);
    let mut out = Vec::new();
    match fn_window(&view, "pub fn wait_loading_known(") {
        Some((at, body)) if !body.contains("\"diagnostics\"") => out.push(report(
            COMMON,
            at,
            "`wait_loading_known` が診断の数を見ていない（`quiescent: false` の処理済みを言えない）",
        )),
        Some(_) => {}
        None => out.push(report(COMMON, 0, "`wait_loading_known` が見つからない")),
    }
    match fn_window(&view, "pub fn args(&self)") {
        Some((at, body)) if !(body.contains("\"--loading-until\"") && body.contains("\"0\"")) => {
            out.push(report(
                COMMON,
                at,
                "`LoadingGate::args` が「最短 0 + 合図」になっていない（読み込みの終わりが実時間に戻る）",
            ))
        }
        Some(_) => {}
        None => out.push(report(COMMON, 0, "`LoadingGate::args` が見つからない")),
    }
    match fn_window(&view, "pub fn status_delay_args(") {
        Some((at, body)) => {
            let ms: Option<u128> = body
                .split('"')
                .filter_map(|piece| piece.parse().ok())
                .next();
            let poll = tako_control::lsp::goto::READY_POLL.as_millis();
            if ms.is_none_or(|ms| ms <= poll) {
                out.push(report(
                    COMMON,
                    at,
                    &format!(
                        "組み込みの知らせの遅れ（{ms:?} ms）が `READY_POLL`（{poll} ms）以下。待ちを抜いても\
                         速い機では通ってしまう（どの機でも必ず落ちる順序にならない）"
                    ),
                ))
            }
        }
        None => out.push(report(COMMON, 0, "`status_delay_args` が見つからない")),
    }
    out
}

fn scan_all() -> (Vec<String>, Vec<(String, usize)>) {
    let mut out = Vec::new();
    let mut anchors = Vec::new();
    for rel in e2e_files() {
        let (found, count) = scan_e2e(&rel, &read(&rel));
        out.extend(found);
        anchors.push((rel, count));
    }
    out.extend(scan_fake(&read(FAKE)));
    out.extend(scan_common(&read(COMMON)));
    (out, anchors)
}

#[test]
fn 読み込みの終わりを実時間で決めていない() {
    let (out, _) = scan_all();
    let loading: Vec<&String> = out.iter().filter(|o| o.contains("#1922 の 2")).collect();
    assert!(loading.is_empty(), "\n{}", join(&loading));
}

#[test]
fn 前提の待ちを通ってから要求を送る() {
    let (out, _) = scan_all();
    let wait: Vec<&String> = out
        .iter()
        .filter(|o| o.contains("#1922 の 1") || o.contains("#1922 の 3") || o.contains("取り違え"))
        .collect();
    assert!(wait.is_empty(), "\n{}", join(&wait));
}

#[test]
fn 待ちの印が意味を保っている() {
    let mut out = scan_fake(&read(FAKE));
    out.extend(scan_common(&read(COMMON)));
    assert!(out.is_empty(), "\n{}", out.join("\n"));
}

fn join(items: &[&String]) -> String {
    items
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 目印が採れている（読み先の取り違え・目印の綴りの変化で何も見ない番犬にならない）
#[test]
fn 走査が空振りしていない() {
    let (_, anchors) = scan_all();
    let count = |name: &str| {
        anchors
            .iter()
            .find(|(rel, _)| rel.ends_with(name))
            .map(|(_, n)| *n)
            .unwrap_or_else(|| panic!("{name} を走査していない: {anchors:?}"))
    };
    assert!(
        anchors.len() >= 10,
        "LSP の e2e を拾えていない: {anchors:?}"
    );
    // 後半 / 前半の `waited_for_loading`・読み込み中の `server_loading`・上限の `Loading`
    assert!(count("issue1869_lsp_followup.rs") >= 5, "{anchors:?}");
    assert!(count("issue1893_lsp_hover_followup.rs") >= 5, "{anchors:?}");
    // 未応答の `starting: false`
    assert!(count("issue1680_lsp_goto.rs") >= 1, "{anchors:?}");
    assert!(count("issue1683_lsp_format.rs") >= 1, "{anchors:?}");
}

/// 現行ソースへ注入して、名指しされる（file:line）ことを確かめる
fn assert_named(label: &str, rel: &str, (from, to): (&str, &str), expect: &str) {
    let src = read(rel);
    assert!(
        src.contains(from),
        "{label}: 注入の元の字面が {rel} に無い（ソースが変わった。注入を作り直す）: {from:?}"
    );
    let injected = src.replacen(from, to, 1);
    let out = if rel == FAKE {
        scan_fake(&injected)
    } else if rel == COMMON {
        scan_common(&injected)
    } else {
        scan_e2e(rel, &injected).0
    };
    assert!(
        out.iter()
            .any(|o| o.starts_with(&format!("{rel}:")) && o.contains(expect)),
        "{label}: 注入が名指しされない: {out:?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let e2e_1869 = "crates/tako-control/tests/issue1869_lsp_followup.rs";
    let e2e_1893 = "crates/tako-control/tests/issue1893_lsp_hover_followup.rs";
    let e2e_1680 = "crates/tako-control/tests/issue1680_lsp_goto.rs";
    let e2e_1683 = "crates/tako-control/tests/issue1683_lsp_format.rs";
    // A. 後半の補完を読み込み中と知る前に送る（CI で落ちた形）
    assert_named(
        "A 1869 の後半",
        e2e_1869,
        (
            "#1922）\n    lsp_fake_e2e::wait_loading_known(&manager);\n",
            "#1922）\n",
        ),
        "#1922 の 1",
    );
    // B. 後半のホバーを同じく
    assert_named(
        "B 1893 の後半",
        e2e_1893,
        (
            "#1922）\n    lsp_fake_e2e::wait_loading_known(&manager);\n",
            "#1922）\n",
        ),
        "#1922 の 1",
    );
    // C. 上限 2 秒の `loading` をサーバの起動ごと測る
    assert_named(
        "C 1893 の上限",
        e2e_1893,
        (
            "#1922）\n        lsp_fake_e2e::wait_loading_known(&manager);\n",
            "#1922）\n",
        ),
        "#1922 の 1",
    );
    // D. 上限 1 秒の未応答をサーバの起動ごと測る（Issue のコメントで落ちた形）
    assert_named(
        "D 1680 の未応答",
        e2e_1680,
        ("    lsp_fake_e2e::wait_running(&manager);\n", ""),
        "#1922 の 3",
    );
    // E. 整形の未応答も同じ
    assert_named(
        "E 1683 の未応答",
        e2e_1683,
        ("    lsp_fake_e2e::wait_running(&manager);\n", ""),
        "#1922 の 3",
    );
    // F. 読み込みの長さを実時間で渡す
    assert_named(
        "F 1869 の実時間",
        e2e_1869,
        (
            "    args.extend(scratch.gate().args());\n",
            "    args.extend([\"--loading-ms\".to_string(), \"1500\".to_string()]);\n",
        ),
        "#1922 の 2",
    );
    // G. 合図を通らずに `loading` を起こす（config から合図の引数を外す）
    assert_named(
        "G 1893 の合図なし",
        e2e_1893,
        ("    args.extend(scratch.gate().args());\n", ""),
        "合図（`LoadingGate`）を通らずに",
    );
    // G2. 1680 の `loading` 専用の config から合図の引数を外す
    assert_named(
        "G2 1680 の合図なし",
        e2e_1680,
        (
            "    let mut extra = scratch.gate().args();\n",
            "    let mut extra: Vec<String> = Vec::new();\n",
        ),
        "合図（`LoadingGate`）を通らずに",
    );
    // H. 偽サーバが読み込み中の知らせを別スレッドから送る
    assert_named(
        "H 知らせを別スレッドから",
        FAKE,
        (
            "                    std::thread::sleep(status_delay);\n                    out.send(serde_json::json!({\n                        \"jsonrpc\": \"2.0\",\n                        \"method\": \"experimental/serverStatus\",\n                        \"params\": { \"health\": \"ok\", \"quiescent\": false },\n                    }));\n",
            "",
        ),
        "読み込みのループの中で",
    );
    // I. 待ちの印が診断の数を見ない
    assert_named(
        "I 印が診断を見ない",
        COMMON,
        (
            "                && server[\"diagnostics\"].as_u64().unwrap_or(0) >= 1\n",
            "",
        ),
        "診断の数を見ていない",
    );
    // J. 組み込みの知らせの遅れを `READY_POLL` 以下へ縮める
    assert_named(
        "J 遅れが短い",
        COMMON,
        (
            "\"--status-delay-ms\".to_string(), \"50\".to_string()",
            "\"--status-delay-ms\".to_string(), \"10\".to_string()",
        ),
        "READY_POLL",
    );
}

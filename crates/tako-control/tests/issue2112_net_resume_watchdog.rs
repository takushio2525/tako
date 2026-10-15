//! ネット復帰の即時再開の番犬（#2112 / FR-2.27.17）
//!
//! # なぜ要るか
//!
//! この機能の約束は 3 つとも「1 行で崩せるのに、普段の操作では気づけない」形をしている:
//!
//! 1. **止まったセッションが無いあいだは到達確認を走らせない** — 既定 ON の機能なので、
//!    判断の前に到達確認を起こす形へ崩すと、全利用者の PC が数秒おきに外へ接続し続ける。
//!    画面は何も変わらない
//! 2. **到達確認の I/O を UI スレッドで走らせない** — 名前解決は時間切れを持たない OS の
//!    呼び出しで、回線の状態によっては数秒塞がる。回線が生きている手元では一瞬で返るので気づけない
//! 3. **診断ログへ画面の文字・送った文を出さない**（AGENTS.md の絶対ルール） — 停止の 1 行や
//!    「続き」の文を persist.log へ足しても、動作は何も変わらない
//!
//! もう 1 つ、**設定の書き換えは dispatch を通る**（設定画面のトグルが settings.json を直に
//! 書くと、CLI / MCP と食い違う 2 本目の実装ができる = 開発不変条件）。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で、**現行ソースから作り直した注入**が file:line で名指しされる
//! ことを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DRIVER: &str = "crates/tako-app/src/net_autoresume.rs";
const SETTINGS_WINDOW: &str = "crates/tako-app/src/settings_window.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// コードだけの眺めで `fn <name>(` の頭の位置と、本体（波括弧の中）の範囲を返す
fn fn_body(code: &str, name: &str) -> Option<(usize, usize, usize)> {
    let head = format!("fn {name}(");
    let start = code.find(&head)?;
    let open = start + code[start..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, open, open + i));
                }
            }
            _ => {}
        }
    }
    None
}

/// ① 到達確認は「送ってよい停止がある」と決めた後でだけ起こす。
/// `drive_net_autoresume` は設定 OFF で最初に抜け、`maybe_start_net_probe(` を呼ぶのは
/// `ready == 0` で抜けた後の 1 か所だけ
fn check_probe_gate(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut v = Vec::new();
    let Some((head, open, close)) = fn_body(&code, "drive_net_autoresume") else {
        return vec![format!("{file} — fn drive_net_autoresume が無い")];
    };
    let body = &code[open..close];
    let first = body
        .trim_start_matches('{')
        .trim_start()
        .lines()
        .next()
        .unwrap_or("");
    if !first.contains("if !self.net_resume.enabled") {
        v.push(format!(
            "{file}:{} — drive_net_autoresume の最初の判断が設定 OFF の早期 return ではない",
            line_of(src, head)
        ));
    }
    let gate = body.find("if ready == 0");
    let calls: Vec<usize> = code
        .match_indices("maybe_start_net_probe(")
        .map(|(i, _)| i)
        .filter(|i| !code[..*i].ends_with("fn "))
        .collect();
    for at in &calls {
        let inside = *at > open && *at < close;
        let after_gate = gate.is_some_and(|g| at - open > g);
        if !inside || !after_gate {
            v.push(format!(
                "{file}:{} — 到達確認を「送ってよい停止がある」と決める前に起こしている",
                line_of(src, *at)
            ));
        }
    }
    if calls.is_empty() {
        v.push(format!("{file} — maybe_start_net_probe の呼び出しが無い"));
    }
    v
}

/// ② 到達確認の I/O（`net_stop::probe(`）は背景のスレッドの中でだけ呼ぶ
fn check_probe_off_ui(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut v = Vec::new();
    let calls: Vec<usize> = code
        .match_indices("net_stop::probe(")
        .map(|(i, _)| i)
        .collect();
    if calls.is_empty() {
        v.push(format!(
            "{file} — 到達確認（net_stop::probe）の呼び出しが無い"
        ));
    }
    for at in calls {
        // 直前の `.spawn(move ||` から呼び出しまでに、閉じ括弧で抜けていないこと
        let spawn = code[..at].rfind(".spawn(move ||");
        let inside = spawn.is_some_and(|s| {
            let between = &code[s..at];
            between.matches('(').count() > between.matches(')').count()
        });
        if !inside {
            v.push(format!(
                "{file}:{} — 到達確認を背景のスレッドの外（UI スレッド）で呼んでいる",
                line_of(src, at)
            ));
        }
    }
    v
}

/// ③ persist.log へ画面の文字（停止の 1 行）・送った文を出さない
fn check_no_pane_text_in_log(file: &str, src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for (at, _) in src.match_indices("persist_log(") {
        // 呼び出しの括弧が閉じるまで
        let mut depth = 0usize;
        let mut end = at;
        for (i, c) in src[at..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = at + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let call = &src[at..end];
        if ["message", "nudge_prompt", "prompt"]
            .iter()
            .any(|w| call.contains(w))
        {
            v.push(format!(
                "{file}:{} — persist.log へ画面の文字か送った文を出している",
                line_of(src, at)
            ));
        }
    }
    v
}

/// ④ 設定画面は settings.json の `network_resume` を直に書かない（dispatch を通る）
fn check_settings_via_dispatch(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    code.match_indices(".network_resume =")
        .map(|(at, _)| {
            format!(
                "{file}:{} — 設定画面が network_resume を直に書いている（dispatch の LimitResume を通す）",
                line_of(src, at)
            )
        })
        .collect()
}

#[test]
fn 到達確認は送ってよい停止があるときだけ() {
    let v = check_probe_gate(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn 到達確認はuiスレッドで走らない() {
    let v = check_probe_off_ui(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn 診断ログへ画面の文字と送った文を出さない() {
    let v = check_no_pane_text_in_log(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn 設定画面はdispatchを通る() {
    let v = check_settings_via_dispatch(SETTINGS_WINDOW, &read(SETTINGS_WINDOW));
    assert!(v.is_empty(), "{v:#?}");
}

/// 注入した逆戻りが file:line で名指しされる（番犬が見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    let driver = read(DRIVER);
    // ① 設定 OFF の早期 return を外す
    let injected = driver.replacen(
        "if !self.net_resume.enabled {",
        "if false && !self.net_resume.enabled {",
        1,
    );
    assert!(
        !check_probe_gate(DRIVER, &injected).is_empty(),
        "① 早期 return を外した注入を名指せない"
    );
    // ① 判断の前に到達確認を起こす
    let injected = driver.replacen(
        "let mut ready = 0usize;",
        "self.maybe_start_net_probe(0);\n        let mut ready = 0usize;",
        1,
    );
    let v = check_probe_gate(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.contains("決める前に")),
        "① 判断の前の到達確認を名指せない: {v:?}"
    );
    // ② UI スレッドで到達確認を呼ぶ
    let injected = driver.replacen(
        "probe.in_flight = true;",
        "let _ = tako_control::net_stop::probe(&target, Duration::from_millis(1));\n        probe.in_flight = true;",
        1,
    );
    let v = check_probe_off_ui(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.contains("UI スレッド")),
        "② UI スレッドの到達確認を名指せない: {v:?}"
    );
    // ③ 停止の 1 行を persist.log へ出す（種別の slug の代わりに画面の文字）
    assert_eq!(
        driver.matches("stop.kind.as_str()").count(),
        1,
        "注入の目印が 1 つ"
    );
    let injected = driver.replacen("stop.kind.as_str()", "stop.message.as_str()", 1);
    let v = check_no_pane_text_in_log(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{DRIVER}:"))),
        "③ persist.log への画面の文字を名指せない: {v:?}"
    );
    // ④ 設定画面が settings.json を直に書く
    let window = read(SETTINGS_WINDOW);
    let injected = format!(
        "{window}\nfn bypass(s: &mut tako_control::settings::Settings) {{ s.network_resume = false; }}\n"
    );
    assert!(
        !check_settings_via_dispatch(SETTINGS_WINDOW, &injected).is_empty(),
        "④ 設定画面の直書きを名指せない"
    );
}

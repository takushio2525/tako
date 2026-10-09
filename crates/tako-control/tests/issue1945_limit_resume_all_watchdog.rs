//! 自動復帰の一括 ON / OFF の番犬（#1945）
//!
//! # なぜ要るか
//!
//! #1945 の穴は 3 つとも「1 行で戻せるのに、戻しても通常の操作では気づけない」形をしている:
//!
//! 1. **退避中のペインを落とす** — 一覧・一括・集計・自動復帰の駆動のどれかが
//!    表示中のタブ（`tabs()`）だけを見ると、退避中の master が一括から漏れる。
//!    退避中のペインを持たない画面では何も起きないので、手で触っても分からない
//! 2. **ステータスバーの描画で全ペインを数える** — ユーザーは「アプリが重い」と言っている。
//!    描画のたびに数え直す形へ戻しても、見た目は 1 ピクセルも変わらない
//! 3. **ボタンが dispatch を通らずに書き換える** — CLI / MCP と食い違う 2 本目の実装ができる
//!    （開発不変条件: UI でできることは同じ dispatch を通る）
//!
//! # 縛ること
//!
//! - [`ステータスバーの描画は数えない`] — `status_bar.rs` に集計・全ペイン走査を書かない
//! - [`退避中を落とさない`] — 一覧・集計・一括・既定の採用・駆動は `all_panes()` /
//!   `pane_anywhere(` を通る
//! - [`一括の実装は1本`] — `apply_bulk(` を呼ぶのは dispatch の `limit_resume_bulk` だけ。
//!   ボタンは `Request::LimitResume` を dispatch へ渡す
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で、**現行ソースから作り直した注入**が file:line で
//! 名指しされることを確かめる。

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

const STATUS_BAR: &str = "crates/tako-app/src/status_bar.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const DRIVER: &str = "crates/tako-app/src/limit_autoresume.rs";
const CORE: &str = "crates/tako-core/src/limit_resume_all.rs";
const APP_DIR: &str = "crates/tako-app/src";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line} — {why}")
}

/// コードだけの眺めで `fn <name>(` の頭の行（1 始まり）と本体（波括弧の中）を返す
fn fn_body(code: &str, name: &str) -> Option<(usize, String)> {
    let head = format!("fn {name}(");
    let start = code.find(&head)?;
    let line = code[..start].matches('\n').count() + 1;
    let open = start + code[start..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((line, code[open..open + i].to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

/// ① ステータスバーの描画で数えない（読むのは `limit_resume_summary` のキャッシュだけ）
fn check_status_bar(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    const BANNED: &[&str] = &["summarize(", "refresh_limit_resume_summary(", "all_panes("];
    code.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            BANNED.iter().find(|b| line.contains(*b)).map(|b| {
                report(
                    file,
                    i + 1,
                    &format!(
                        "ステータスバーの描画で `{b}` を呼んでいる（描画のたびに全ペインを数える。\
                         集計は 2 秒 tick と切り替えの直後に数えた `limit_resume_summary` を読む）"
                    ),
                )
            })
        })
        .collect()
}

/// ② 退避中を落とさない: 指定の関数の本体に目印が無ければ関数の頭を名指す
fn check_traverses(file: &str, src: &str, rules: &[(&str, &str)]) -> Vec<String> {
    let code = code_view::code_view(src);
    rules
        .iter()
        .filter_map(|(name, needle)| match fn_body(&code, name) {
            None => Some(report(
                file,
                0,
                &format!("fn {name} が見つからない（改名したら番犬も直す）"),
            )),
            Some((line, body)) if !body.contains(needle) => Some(report(
                file,
                line,
                &format!(
                    "fn {name} が `{needle}` を通っていない（表示中のタブだけを見ると退避中の\
                     ペインが一括・一覧・集計・駆動から漏れる = #1945 の穴）"
                ),
            )),
            Some(_) => None,
        })
        .collect()
}

/// ③ 一括の実装は 1 本: GUI が `apply_bulk(` を直に呼ばない・ボタンは dispatch を通る
fn check_single_impl(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut out: Vec<String> = code
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("apply_bulk("))
        .map(|(i, _)| {
            report(
                file,
                i + 1,
                "GUI が `apply_bulk(` を直に呼んでいる（一括は dispatch の limit_resume_bulk の \
                 1 実装。CLI / MCP と食い違う 2 本目になる）",
            )
        })
        .collect();
    if let Some((line, body)) = fn_body(&code, "toggle_limit_resume_all") {
        if !(body.contains("dispatch(") && body.contains("Request::LimitResume")) {
            out.push(report(
                file,
                line,
                "ステータスバーのボタンが `Request::LimitResume` を dispatch へ渡していない",
            ));
        }
    }
    out
}

const DISPATCH_RULES: &[(&str, &str)] = &[
    ("limit_resume_panes", "all_panes()"),
    ("limit_resume_entry", "pane_anywhere("),
    ("limit_resume_bulk", "all_panes()"),
];
const DRIVER_RULES: &[(&str, &str)] = &[
    ("drive_limit_autoresume", "all_panes()"),
    ("limit_resume_detected", "all_panes()"),
];
const CORE_RULES: &[(&str, &str)] = &[
    ("summarize", "all_panes()"),
    ("apply_bulk", "all_panes()"),
    ("adopt_default", "all_panes()"),
    ("mark_restored_agents_decided", "all_panes()"),
];

fn app_sources() -> Vec<(String, String)> {
    let dir = workspace_root().join(APP_DIR);
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)
            .expect("tako-app/src が読める")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(workspace_root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn ステータスバーの描画は数えない() {
    let v = check_status_bar(STATUS_BAR, &read(STATUS_BAR));
    assert!(v.is_empty(), "{}", v.join("\n"));
}

#[test]
fn 退避中を落とさない() {
    let mut v = check_traverses(DISPATCH, &read(DISPATCH), DISPATCH_RULES);
    v.extend(check_traverses(DRIVER, &read(DRIVER), DRIVER_RULES));
    v.extend(check_traverses(CORE, &read(CORE), CORE_RULES));
    assert!(v.is_empty(), "{}", v.join("\n"));
}

#[test]
fn 一括の実装は1本() {
    let v: Vec<String> = app_sources()
        .iter()
        .flat_map(|(file, src)| check_single_impl(file, src))
        .collect();
    assert!(v.is_empty(), "{}", v.join("\n"));
    // ボタンの関数そのものが在ること（改名で ③ の後半が空振りしない）
    let has_toggle = app_sources()
        .iter()
        .any(|(_, src)| fn_body(&code_view::code_view(src), "toggle_limit_resume_all").is_some());
    assert!(
        has_toggle,
        "toggle_limit_resume_all が見つからない（改名したら番犬も直す）"
    );
}

/// 現行ソースから作り直した注入が、それぞれ file:line で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    let named = |v: &[String], file: &str| v.iter().any(|m| m.starts_with(&format!("{file}:")));

    // ① 描画のたびに数える
    let bar = read(STATUS_BAR).replacen(
        "let summary = self.limit_resume_summary;",
        "let summary = tako_core::limit_resume_all::summarize(&self.workspace, false, |_| false);",
        1,
    );
    let v = check_status_bar(STATUS_BAR, &bar);
    assert!(named(&v, STATUS_BAR), "① の注入を名指せない: {v:?}");

    // ② 表示中のタブだけを見る（関数ごとに 1 つずつ戻す）
    for (file, rules) in [
        (DISPATCH, DISPATCH_RULES),
        (DRIVER, DRIVER_RULES),
        (CORE, CORE_RULES),
    ] {
        let src = read(file);
        for (name, needle) in rules {
            let code = code_view::code_view(&src);
            let (line, body) = fn_body(&code, name).expect("現行ソースに在る");
            // 本体の目印を潰した版（バイト長を保つので行番号は動かない）
            let start = code.find(&body).expect("本体の位置");
            let mut injected = src.clone();
            let replaced = body.replace(needle, &" ".repeat(needle.len()));
            injected.replace_range(start..start + body.len(), &replaced);
            let v = check_traverses(file, &injected, &[(name, needle)]);
            assert!(
                v.iter().any(|m| m.starts_with(&format!("{file}:{line} "))),
                "② {file} の fn {name} の注入を名指せない: {v:?}"
            );
        }
    }

    // ③ GUI が一括を直に書き換える
    let main = "crates/tako-app/src/main.rs";
    let injected = format!(
        "{}\nfn bypass(ws: &mut tako_core::Workspace) {{ tako_core::limit_resume_all::apply_bulk(ws, true, |_| false); }}\n",
        read(main)
    );
    let v = check_single_impl(main, &injected);
    assert!(named(&v, main), "③ apply_bulk の直呼びを名指せない: {v:?}");
    // ③ ボタンが dispatch を通らない
    let src = read(DRIVER).replacen(
        "tako_control::protocol::Request::LimitResume {",
        "tako_control::protocol::Request::Persist {",
        1,
    );
    let v = check_single_impl(DRIVER, &src);
    assert!(
        named(&v, DRIVER),
        "③ dispatch を通らないボタンを名指せない: {v:?}"
    );
}

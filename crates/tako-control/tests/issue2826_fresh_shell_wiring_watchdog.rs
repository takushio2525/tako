//! 番犬: 作りたてのペインへの送信を送達フローへ寄せる配線が抜けていない（Issue #2826）
//!
//! 判断（`tako_core::shell_send::shell_wait`）と dispatch の呼び出しは単体テスト
//! （`dispatch::tests::issue2826_*`）が MockHost で見る。MockHost は「素のシェルとして起こした
//! 経過」を手で入れるので、**GUI がそれを控えて答える配線**が消えても単体テストは緑のまま
//! （本番では作りたてのペインが見分けられず、修正前の即時書き込みへ黙って戻る）。ここで見るのは:
//!
//! 1. GUI（`tako-app`）が `SessionHost` の `fresh_shell_age` / `command_flow_pending` に答える
//!    （既定の実装 = None / false は「作りたてではない」なので、答えないと寄せない）
//! 2. GUI のペインの起動（`spawn_session`）が素のシェルを控える（`note_fresh_shell`）
//! 3. dispatch の送信（`send_to_pane`）が代替画面の分岐**より前**に判断する（器のペインは
//!    器のクライアント自身が代替画面に入るので、後ろに置くと器のペインで判断が効かない）
//!
//! コメントと文字列は潰してから見る（この説明文や他の番犬の期待値を拾わない）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view_mod;
use code_view_mod::code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn code_of(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    code_view(&src)
}

/// `fn <name>(` から、同じ字下げの次の `fn ` / 閉じ括弧までの本体（無ければ None）
fn fn_body<'a>(code: &'a str, name: &str) -> Option<&'a str> {
    let head = format!("fn {name}(");
    let at = code.find(&head)?;
    let line_start = code[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = &code[line_start..at];
    let indent = &indent[..indent.len() - indent.trim_start().len()];
    let close = format!("\n{indent}}}");
    let end = code[at..].find(&close).map_or(code.len(), |i| at + i);
    Some(&code[at..end])
}

/// 判定の本体（下の注入テストも同じ関数を通す）
fn violations(app: &str, dispatch: &str) -> Vec<String> {
    let mut out = Vec::new();
    match fn_body(app, "fresh_shell_age") {
        Some(body) if body.contains("fresh_shell_spawns") => {}
        Some(_) => {
            out.push("tako-app の fresh_shell_age が控え（fresh_shell_spawns）を読まない".into())
        }
        None => out.push(
            "tako-app が SessionHost::fresh_shell_age に答えない（既定 = None で寄せない）".into(),
        ),
    }
    match fn_body(app, "command_flow_pending") {
        Some(body) if body.contains("command_flows") => {}
        Some(_) => out
            .push("tako-app の command_flow_pending が送達フロー（command_flows）を見ない".into()),
        None => out.push("tako-app が SessionHost::command_flow_pending に答えない".into()),
    }
    match fn_body(app, "spawn_session") {
        Some(body) if body.contains("note_fresh_shell(") => {}
        Some(_) => {
            out.push("tako-app の spawn_session が素のシェルを控えない（note_fresh_shell）".into())
        }
        None => out.push("tako-app に spawn_session が見つからない（走査の前提が崩れた）".into()),
    }
    match fn_body(dispatch, "send_to_pane") {
        // 代替画面の**分岐**（判断の材料として `is_alt_screen()` を読む行は前にあってよい）
        Some(body) => match (
            body.find("shell_wait("),
            body.find("if session.is_alt_screen()"),
        ) {
            (Some(judge), Some(alt)) if judge < alt => {}
            (Some(_), Some(_)) => out.push(
                "send_to_pane が代替画面の分岐の後ろで判断している（器のペインで効かない）".into(),
            ),
            (None, _) => out.push("send_to_pane が shell_send::shell_wait を呼ばない".into()),
            (Some(_), None) => {
                out.push("send_to_pane に代替画面の分岐が見つからない（走査の前提が崩れた）".into())
            }
        },
        None => out.push("dispatch に send_to_pane が見つからない（走査の前提が崩れた）".into()),
    }
    out
}

#[test]
fn 作りたてのペインへの送信を寄せる配線が揃っている() {
    let app = code_of("crates/tako-app/src/main.rs");
    let dispatch = code_of("crates/tako-control/src/dispatch.rs");
    let found = violations(&app, &dispatch);
    assert!(
        found.is_empty(),
        "#2826 の配線が抜けた（作りたてのペインへの送信が修正前の即時書き込みへ戻る）:\n{}",
        found.join("\n")
    );
}

/// 検出器そのものの確かめ（注入）: 配線を 1 つずつ外すと、それぞれ名指しで拾う
#[test]
fn 検出器は配線の抜けを1つずつ名指しする() {
    let app = "impl X {\n    fn spawn_session(&mut self) {\n        self.note_fresh_shell(p, t);\n    }\n}\n\
        impl SessionHost for App {\n    fn fresh_shell_age(&self) -> u8 {\n        self.fresh_shell_spawns.get(&p)\n    }\n\
            fn command_flow_pending(&self) -> bool {\n        self.command_flows.iter().any(f)\n    }\n}\n";
    let dispatch = "fn send_to_pane() {\n    if let Some(w) = shell_wait(&t) {}\n    if session.is_alt_screen() {}\n}\n";
    assert!(
        violations(app, dispatch).is_empty(),
        "{:?}",
        violations(app, dispatch)
    );

    let no_age = app.replace("fn fresh_shell_age", "fn other_age");
    assert!(violations(&no_age, dispatch)[0].contains("fresh_shell_age に答えない"));
    let no_note = app.replace("note_fresh_shell(", "noop(");
    assert!(violations(&no_note, dispatch)[0].contains("素のシェルを控えない"));
    let no_pending = app.replace("self.command_flows", "self.other");
    assert!(violations(&no_pending, dispatch)[0].contains("command_flows"));
    let late = "fn send_to_pane() {\n    if session.is_alt_screen() {}\n    if let Some(w) = shell_wait(&t) {}\n}\n";
    assert!(violations(app, late)[0].contains("代替画面の分岐の後ろ"));
    let none = "fn send_to_pane() {\n    if session.is_alt_screen() {}\n}\n";
    assert!(violations(app, none)[0].contains("shell_wait を呼ばない"));
}

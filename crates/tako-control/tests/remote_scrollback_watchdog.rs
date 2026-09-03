//! 番犬: remote の scrollback が器の境界を通ること（#972）
//!
//! `tako remote scrollback` は器（#519 の `SessionBackend` / `DetachedCapture`）を通らず
//! `tako_core::tmux::tmux_command()` を直に叩いていた。壊れ方は 2 通りで、
//! **どちらも「機能が無い」ではなく「常に失敗する」形**で現れる:
//!
//! - Windows（器 = psmux）: tmux 決め打ちの呼び方が
//!   `no server running on session '<socket>__<target>'` になる（実測。#972）
//! - macOS（tmux 3.6）: ターゲットが裸の `=<session>` で
//!   `can't find pane: =<session>` になる（実測 3.6b。#32 が発見した罠）
//!
//! ソース走査にしてあるのは、**macOS の実行では Windows 側の壊れ方が見えない**ため。
//!
//! 併せて、remote.rs に残っている tmux 直呼びの**面**も固定する。許可は
//! 関数単位で、それぞれに理由を書く（黙って増えないようにする）。

use std::path::{Path, PathBuf};

fn remote_rs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote.rs")
}

/// `tmux_command(` を直に呼んでいる行を「囲っている関数名」つきで集める。
/// コメント行は落とす（説明文の中の `tmux_command(` を拾わない）
fn direct_tmux_calls(src: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut current = String::from("<トップレベル>");
    for (idx, line) in src.lines().enumerate() {
        let code = line.trim_start();
        if let Some(name) = fn_name(code) {
            current = name;
        }
        if code.starts_with("//") {
            continue;
        }
        if code.contains("tmux_command(") {
            out.push((current.clone(), idx + 1, code.to_string()));
        }
    }
    out
}

/// `fn <名前>` の宣言行から関数名を取り出す（`pub` / `pub(crate)` / `async` を許す）
fn fn_name(code: &str) -> Option<String> {
    let rest = code
        .strip_prefix("pub(crate) ")
        .or_else(|| code.strip_prefix("pub "))
        .unwrap_or(code);
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    let rest = rest.strip_prefix("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// **#972 の本体**: scrollback の経路に tmux 直呼びが戻っていないこと。
///
/// 許可されるのは 2 つだけで、どちらも scrollback の**答えを出す経路ではない**
#[test]
fn remoteのtmux直呼びは既知の関数だけに閉じている() {
    /// (関数名, 許可の理由)
    const ALLOWED: &[(&str, &str)] = &[
        (
            "tmux_output_with_timeout",
            "daemon の画面 API（`/api/panes/:id/screen` 系）が使うタイムアウト付き実行。\
             `-e` の ANSI 採取・`display-message` の geometry は境界に無いので別 Issue",
        ),
        (
            "scrollback_legacy",
            "#972 の A/B（`TAKO_972_LEGACY=1`）用に旧経路をそのまま保存したもの。\
             既定経路ではない",
        ),
    ];

    let src = std::fs::read_to_string(remote_rs()).expect("remote.rs を読めない");
    let calls = direct_tmux_calls(&src);
    let offenders: Vec<String> = calls
        .iter()
        .filter(|(func, _, _)| !ALLOWED.iter().any(|(a, _)| a == func))
        .map(|(func, line, code)| format!("remote.rs:{line} fn {func}: {code}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "tmux の直呼びが境界の外にある:\n  {}\n\
         → `reach::detached_capture` / `DetachedCapture` を通してください（#972 / #519）",
        offenders.join("\n  ")
    );

    // scrollback そのものは名指しで禁じる（許可リストへ足して素通りさせない）
    assert!(
        !calls.iter().any(|(func, _, _)| func == "scrollback"),
        "scrollback が tmux を直に叩いている（#972 の回帰）"
    );
}

/// scrollback の本体が器の境界（`reach::detached_capture` → `capture_scrollback`）を
/// 通っていること。「直呼びが無い」だけでは、境界を通らない別の抜け道を塞げない
#[test]
fn scrollbackは器の境界を通って採取する() {
    let src = std::fs::read_to_string(remote_rs()).expect("remote.rs を読めない");
    let body = function_body(&src, "pub fn scrollback_session(session: &str, lines: u32)");
    let body = without_comments(&body);
    for needed in ["reach::detached_capture", "capture_scrollback"] {
        assert!(
            body.contains(needed),
            "scrollback が {needed} を通っていない（#972）:\n{body}"
        );
    }
}

/// **入れ子 IPC を作らない**: dispatch（tako-app の内側）が通る経路は
/// `scrollback_session` で、そちらは対象指定の解決（= IPC）を含まないこと。
///
/// dispatch から IPC 解決つきの `scrollback` を呼ぶと、要求を捌いている当の
/// tako-app が自分自身へ接続する形になる
#[test]
fn dispatchはipc解決を含まない入口を使う() {
    let dispatch =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dispatch.rs"))
            .expect("dispatch.rs を読めない");
    let arm = function_body(&dispatch, "fn scrollback_target_session(");
    assert!(
        !without_comments(&arm).contains("AppIpcClient"),
        "dispatch の解決が IPC を張っている（#972）"
    );
    assert!(
        dispatch.contains("remote::scrollback_session(&session"),
        "dispatch が IPC 解決なしの入口（scrollback_session）を通っていない（#972）"
    );
    assert!(
        !dispatch.contains("remote::scrollback(&"),
        "dispatch が CLI 用（IPC 解決つき）の scrollback を呼んでいる（#972）"
    );

    // CLI 用の入口だけが IPC 解決を持つ
    let remote = std::fs::read_to_string(remote_rs()).expect("remote.rs を読めない");
    let cli = function_body(&remote, "pub fn scrollback(pane_id: &str, lines: u32)");
    assert!(
        without_comments(&cli).contains("resolve_scrollback_session"),
        "CLI 用の入口が対象指定の解決を通っていない（#972）"
    );
    let dispatch_entry = function_body(&remote, "pub fn scrollback_session(session: &str");
    assert!(
        !without_comments(&dispatch_entry).contains("resolve_scrollback_session"),
        "dispatch 用の入口が IPC 解決を含んでいる（#972）"
    );
}

/// 関数本文を波括弧の釣り合いで切り出す
fn function_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let open = src[start..]
        .find('{')
        .unwrap_or_else(|| panic!("{signature} の本文が見つからない"))
        + start;
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("{signature} の本文を閉じられない");
}

fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 走査そのものの検出力（関数名の切り出しが効いているか）を自分で確かめる
#[test]
fn 走査は関数名つきで直呼びを拾える() {
    let src = "\
pub fn a() {\n    tako_core::tmux::tmux_command(None);\n}\n\
fn b() {\n    // tmux_command( はコメントなので拾わない\n    let x = 1;\n}\n";
    let calls = direct_tmux_calls(src);
    assert_eq!(calls.len(), 1, "拾い方がおかしい: {calls:?}");
    assert_eq!(calls[0].0, "a");
}

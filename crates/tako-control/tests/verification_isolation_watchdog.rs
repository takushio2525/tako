//! 番犬: **検証プロセスは本番の個人ファイルへ書かない**（Issue #1253 / #944 / #1030）
//!
//! #944 は「置き場を決める側を倒す」で `cargo test` を塞いだが、判定の一部が
//! `cfg(test)` のままだった。GUI セルフテストは `cargo test` ではなく**製品バイナリ**が
//! `TAKO_SELF_TEST=1` で立つので、そこには 1 ビットも効かない —— 実測では
//! `~/.claude.json` のテスト残骸 3,050 件のうち約 70% がこの経路だった。
//!
//! したがって検査するのは「書かないこと」だけでなく**判定の形**そのもの:
//! 書き先を決める関数が `cfg(test)` ではなく実行時判定
//! （`tako_core::paths::is_verification_process`）を引いていること。
//! 挙動側の番犬は `tako_control::test_write_isolation`（空 HOME で子を回す）と
//! `tako-app` の `update_checker`（偽 brew を PATH へ置く）にある。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// `fn <名前>(` から対応する `}` までの本体を返す（波括弧の対応で切る）
fn fn_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("fn {name}("))?;
    let open = text[at..].find('{')? + at;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..open + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 外部エージェントの設定の書き先は**実行時判定**で倒す。
///
/// `cfg(test)` だと GUI セルフテスト（製品バイナリ）が素通りする = #1253 の本体
#[test]
fn 設定の書き先は実行時判定で隔離される() {
    for (rel, func) in [
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "agent_config_home",
        ),
        ("crates/tako-control/src/claude_tui.rs", "env_config_dir"),
    ] {
        let text = read(rel);
        let body =
            fn_body(&text, func).unwrap_or_else(|| panic!("{rel}: fn {func} が見つからない"));
        assert!(
            body.contains("is_verification_process"),
            "{rel}: {func} が実行時判定（paths::is_verification_process）を引いていない（#1253）"
        );
        assert!(
            !body.contains("cfg(test)"),
            "{rel}: {func} が cfg(test) で隔離している。\
             GUI セルフテストは製品バイナリなので効かない（#1253）"
        );
    }
}

/// テストプロセスから brew を起こさない（#1253。#944 の `claude agents --json` と同型）。
///
/// `cargo test --workspace` がユーザーの `~/Library/Caches/Homebrew` へ
/// 698 ファイル作っていた（空 HOME で実測）
#[test]
fn 配布系統の判別はテストプロセスで外部コマンドを起こさない() {
    let rel = "crates/tako-app/src/update_checker.rs";
    let text = read(rel);
    let body = fn_body(&text, "detect_install_method_full").expect("判別関数が見つからない");
    let guard = body.find("is_test_process").unwrap_or_else(|| {
        panic!("{rel}: detect_install_method_full にテストプロセスの門番が無い（#1253）")
    });
    let probe = body.find("is_brew_available").unwrap_or_else(|| {
        panic!("{rel}: brew の実行が見当たらない（実装が変わったら番犬も直す）")
    });
    assert!(
        guard < probe,
        "{rel}: brew を起こした後で門番を通している（順番が逆だと外部コマンドが走る。#1253）"
    );
}

/// 対話 zsh の履歴は **rc の後に**当て直す（#1253）。
///
/// macOS の `/etc/zshrc` は `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` を無条件で
/// 代入するので、環境変数を渡すだけでは対話シェルに効かない（実測で確認した）
#[test]
fn 検証の履歴はrcの後に当て直される() {
    let rel = "crates/tako-core/shell-integration/zshenv.zsh";
    let text = read(rel);
    assert!(
        text.contains("TAKO_VERIFY_HISTFILE"),
        "{rel}: 検証用の履歴の書き先を当て直す仕掛けが無い（#1253）"
    );
    assert!(
        text.contains("precmd_functions+=(_tako_verify_histfile)"),
        "{rel}: 当て直しが precmd に載っていない。\
         .zshenv での代入は /etc/zshrc に潰される（#1253）"
    );
    // ペイン ID で絞ると、履歴を汚していた素の PTY（TAKO_PANE_ID を持たない）が漏れる
    let at = text
        .find("TAKO_VERIFY_HISTFILE-")
        .expect("ガード行が見つからない");
    let guard_line = text[..at].rsplit('\n').next().unwrap_or("");
    assert!(
        !guard_line.contains("TAKO_PANE_ID"),
        "{rel}: 当て直しを TAKO_PANE_ID で絞っている。\
         履歴を汚していたのはペイン ID を持たない素の PTY（#1253）"
    );
}

//! **#2361 の番犬**: Windows の名前付きパイプの accept ループが、待ち受けのインスタンスを
//! 作れないときに止まらない（間を置いて作り直す）。
//!
//! ## 何が起きていたか
//!
//! accept スレッドは 1 本受けるたびに次のインスタンスを `create_server_instance(name, false)`
//! で作って待つ。その作成が 1 回でも失敗すると `break` してスレッドごと終わり、待ち受けの
//! インスタンスが 1 つも残らなかった（GUI は生きているのに CLI / MCP の stdio が届かない）。
//! Windows CI のテスト（`ipc.rs` の `windows_tests::issue2361_*`）は注入で再現と回復を見るが、
//! macOS では走らないので、ここでは**本番コードの形**を固定する（どの OS の CI でも落ちる）。
//!
//! ## 何を固定するか
//!
//! 1. [`次のインスタンスの作成は作り直しの1実装か旧挙動の分岐だけ`] — `ipc.rs` の本番コードで
//!    2 本目以降のインスタンス（`create_server_instance(.., false)`）を作るのは
//!    `tako_core::ipc_socket::create_with_retry` へ渡す作成の closure と、A/B の旧挙動の分岐
//!    （`None if config.legacy`）だけ。それ以外で作って失敗を握る形が戻ったら `file:line` で名指す
//! 2. [`作れないとbreakするのは旧挙動の分岐だけ`] — 「作れない」の warn に続く `break` は
//!    旧挙動の分岐の中の 1 か所だけ
//! 3. [`abの逃げ道は1つのenvに閉じている`] — `TAKO_2361_LEGACY` を読むのは tako-core の 1 か所

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const IPC: &str = "crates/tako-control/src/ipc.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &str) -> Vec<String> {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::production(&src, rel)
        .lines()
        .map(str::to_string)
        .collect()
}

/// `i` 行目から上へ `window` 行のどこかに `needle` があるか
fn preceded_by(lines: &[String], i: usize, window: usize, needle: &str) -> bool {
    lines[i.saturating_sub(window)..=i]
        .iter()
        .any(|l| l.contains(needle))
}

#[test]
fn 次のインスタンスの作成は作り直しの1実装か旧挙動の分岐だけ() {
    let lines = production(IPC);
    let calls: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("create_server_instance(") && l.contains(", false)"))
        .map(|(i, _)| i)
        .collect();
    assert!(
        calls.len() >= 2,
        "{IPC} に 2 本目以降のインスタンスの作成が見つからない（番犬の前提が崩れた）: {calls:?}"
    );
    let stray: Vec<String> = calls
        .iter()
        .filter(|&&i| {
            !preceded_by(&lines, i, 3, "create_with_retry(")
                && !preceded_by(&lines, i, 3, "None if config.legacy")
        })
        .map(|i| format!("{IPC}:{}", i + 1))
        .collect();
    assert!(
        stray.is_empty(),
        "待ち受けの次のインスタンスを tako_core::ipc_socket::create_with_retry の外で作っている\
         （作れないと accept ループが止まる形 = #2361 の修正前）: {stray:?}"
    );
    assert!(
        calls
            .iter()
            .any(|&i| preceded_by(&lines, i, 3, "create_with_retry(")),
        "{IPC} の accept ループが create_with_retry を通っていない"
    );
}

#[test]
fn 作れないとbreakするのは旧挙動の分岐だけ() {
    let lines = production(IPC);
    let warns: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("IPC パイプインスタンスを作れない"))
        .map(|(i, _)| i)
        .collect();
    let named: Vec<String> = warns.iter().map(|i| format!("{IPC}:{}", i + 1)).collect();
    assert_eq!(
        warns.len(),
        1,
        "「作れない」で止まる分岐は旧挙動（A/B）の 1 か所だけ: {named:?}"
    );
    let at = warns[0];
    assert!(
        preceded_by(&lines, at, 6, "None if config.legacy"),
        "「作れない」で止まる分岐が旧挙動（`None if config.legacy`）の外にある: {named:?}"
    );
    assert!(
        lines.get(at + 1).is_some_and(|l| l.trim() == "break;"),
        "旧挙動の分岐が修正前と同じ形（warn の後に break）になっていない: {named:?}"
    );
}

#[test]
fn abの逃げ道は1つのenvに閉じている() {
    let readers: Vec<String> = production_range::product_sources()
        .iter()
        .flat_map(|rel| {
            let src = std::fs::read_to_string(workspace_root().join(rel))
                .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
            production_range::scan(&src)
                .text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains("\"TAKO_2361_LEGACY\""))
                .map(|(i, _)| format!("{rel}:{}", i + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        readers.len() == 1 && readers[0].starts_with("crates/tako-core/src/ipc_socket.rs:"),
        "TAKO_2361_LEGACY を読むのは tako_core::ipc_socket::legacy_2361 だけにする: {readers:?}"
    );
}

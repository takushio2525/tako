//! `tako tmux cleanup` が「黙って何もしない」形へ戻らないための番犬（#1187）
//!
//! 落とすのは 3 つの再発形:
//!
//! 1. **引数を捨てる**（`let _ = socket;`）。ヘルプ・MCP 説明は「省略時は tako バックエンド
//!    サーバー」と書いてあるのに、実装は常に自分の backend しか見ていなかった。
//!    「受け付けるように見えて黙って無視」が一番悪い形なので、引数が host まで届くことを縛る
//! 2. **理由を返さない**。見送ったときに `{"killed":[]}` だけを返すと、呼び出し側（AI）は
//!    「掃除するものが無かった」と区別できず「掃除済み」と報告してしまう
//! 3. **プロセス名だけで見送る**（`ports::other_tako_running()` を掃除本体から直接見る）。
//!    隔離インスタンスは別ソケットなので本番の掃除を止める理由が無い。判定材料は
//!    プロセスの有無ではなく**そのソケットの所有者**（`tmux_cleanup::blocker`）である
//!
//! ソース走査なので、実装が別の書き方に変わっても「その材料を見ているか」で落ちる。

use std::path::{Path, PathBuf};

/// 走査するツリー。`TAKO_1187_WATCHDOG_ROOT` で差し替えられる（A/B 専用:
/// 修正前のソースを取り出した一時ツリーを指すと、この番犬が落ちることを実測できる）
fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("TAKO_1187_WATCHDOG_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読めない: {e}"))
}

/// `needle` で始まるブロックを、波括弧の対応で切り出す（見つからなければ panic）
fn block_after(src: &str, needle: &str) -> String {
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("目印 `{needle}` がソースに無い（実装が動いた?）"));
    // 目印自身が `{` を含む（`Request::X { socket } =>`）ので、開き波括弧は
    // **目印の終わりより後ろ**から探す
    let after = start + needle.len();
    let open = after + src[after..].find('{').expect("ブロックの開き波括弧が無い");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[start..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("ブロックが閉じていない: {needle}");
}

/// コメント行を落とす（説明文が検査対象の形と一致して誤検知するのを防ぐ）
fn code_only(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// legacy（A/B 用の再現アーム）の行は検査対象から外す。
/// `TAKO_1187_LEGACY=1` は**修正前の挙動をわざと再現する**ので、
/// そこに旧実装の形が残っているのは正しい
fn without_legacy(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.contains("legacy") && !l.contains("Legacy") && !l.contains("LEGACY"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn cleanupのsocket引数を捨てていない() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    let arm = code_only(&block_after(&src, "Request::TmuxCleanup { socket } =>"));
    assert!(
        !arm.contains("let _ = socket"),
        "TmuxCleanup が socket を捨てている（#1187 の再発）:\n{arm}"
    );
    let checked = without_legacy(&arm);
    assert!(
        checked.contains("cleanup_orphan_tmux(") && checked.contains("socket"),
        "socket を host の cleanup へ渡していない:\n{arm}"
    );
    // host 側も引数を受け取る形であること（trait のシグネチャが戻ると 1 の再発）
    let host = read(&root, "crates/tako-control/src/host.rs");
    let sig = code_only(&block_after(&host, "fn cleanup_orphan_tmux("));
    assert!(
        sig.contains("socket"),
        "TmuxHost::cleanup_orphan_tmux が socket を受け取っていない:\n{sig}"
    );
}

#[test]
fn cleanupは見送りの理由を応答とpersistlogへ出す() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    let arm = without_legacy(&code_only(&block_after(
        &src,
        "Request::TmuxCleanup { socket } =>",
    )));
    assert!(
        arm.contains("\"skipped\""),
        "応答に見送りの理由（skipped）が無い = 空配列と区別できない（#1187）:\n{arm}"
    );
    assert!(
        arm.contains("persist_log"),
        "見送り / 実行を persist.log へ残していない（.app 起動では stderr はどこにも残らない）:\n{arm}"
    );
}

#[test]
fn 掃除本体はプロセス名ではなくソケットの所有者で見送りを決める() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-app/src/main.rs");
    let body = code_only(&block_after(&src, "fn cleanup_orphan_tmux_with("));
    assert!(
        !body.contains("other_tako_running"),
        "掃除本体が「別の tako-app がいるか」だけで見送っている（#1187 の根因）。\
         判定は tmux_cleanup::blocker（対象ソケットの所有者）を通すこと:\n{body}"
    );
    assert!(
        body.contains("tmux_cleanup::blocker") && body.contains("live_peers"),
        "見送りの判定が tmux_cleanup::blocker / live_peers を通っていない:\n{body}"
    );
}

/// 隔離・セルフテストのソケット名は core と 1 実装。ここが直書きへ戻ると
/// 「相手のソケット名を復元する」側とズレて、所有者判定が静かに外れる
#[test]
fn 隔離ソケット名の生成はcoreと1実装() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-app/src/main.rs");
    for bad in ["format!(\"tako-iso-{}\"", "format!(\"tako-st-{}\""] {
        assert!(
            !src.contains(bad),
            "隔離ソケット名を main.rs で直書きしている（{bad}）。\
             tako_core::tmux_cleanup::isolated_socket_name / self_test_socket_name を使うこと"
        );
    }
    assert!(
        src.contains("tmux_cleanup::isolated_socket_name")
            && src.contains("tmux_cleanup::self_test_socket_name"),
        "一括隔離が core のソケット名生成を通っていない"
    );
}

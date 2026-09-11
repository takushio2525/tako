//! 名前で列挙する器の回収（#1282）が危ない形へ戻らないための番犬
//!
//! 落とすのは 3 つの再発形:
//!
//! 1. **名前一致の一括 kill**（`taskkill /IM` / `Stop-Process -Name` /
//!    `pkill` の名前指定）。他インスタンス・他ワーカーの器を巻き込む（#625 の事故クラス）。
//!    落としてよいのは **pid を名指し**したものだけ
//! 2. **psmux の `kill-server` を製品経路から叩く**。`-L` を落とすと全ソケットの器が死に、
//!    しかも返らないことがある（#1271 の実測）。名前で列挙した器は pid で落とす
//! 3. **ソケットファイルが無い環境で器の列挙を諦める**（`socket_dir()` が `None` なら
//!    空を返す）。これが #1282 の症状そのもの（実機に 24 個残っていても 0 件と答えた）
//!
//! ソース走査なので、実装が別の書き方に変わっても「その材料を見ているか」で落ちる。
//! `TAKO_1282_WATCHDOG_ROOT` に修正前のツリーを指すと、この番犬が落ちることを実測できる

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("TAKO_1282_WATCHDOG_ROOT") {
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

/// 走査対象の製品コード（テストの器は `psmux_cleanup_timeout_watchdog` が見ている）
const PRODUCT_SOURCES: [&str; 2] = [
    "crates/tako-core/src/tmux_cleanup.rs",
    "crates/tako-core/src/platform/procinfo.rs",
];

/// 名前で対象を選ぶ強制終了の書き方。**1 つでもあれば落とす**
const NAME_WIDE_KILL: [&str; 5] = ["/IM", "Stop-Process", "-Name ", "pkill", "killall"];

#[test]
fn 名前一致の一括killが製品コードへ入っていない() {
    let root = workspace_root();
    for rel in PRODUCT_SOURCES {
        let text = read(&root, rel);
        for (i, line) in text.lines().enumerate() {
            // コメント行は説明で言及するので除く（禁止の理由を書けなくなる）
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with("///") || code.starts_with("*") {
                continue;
            }
            for pattern in NAME_WIDE_KILL {
                assert!(
                    !code.contains(pattern),
                    "{rel}:{} に名前一致の強制終了（{pattern}）がある\n\
                     → 落としてよいのは pid を名指ししたものだけ（#1282 / #625）:\n\
                     {code}",
                    i + 1
                );
            }
        }
    }
}

#[test]
fn 器の強制終了はpidを名指ししている() {
    let root = workspace_root();
    let text = read(&root, "crates/tako-core/src/tmux_cleanup.rs");
    assert!(
        text.contains("\"/PID\""),
        "Windows の強制終了が pid 指定（taskkill /PID）になっていない（#1282）"
    );
    assert!(
        text.contains("\"/T\""),
        "器の中のシェルごと落とす /T が無い（器だけ落とすと pwsh が残る。#1271 の実測）"
    );
}

#[test]
fn 名前で列挙した器にkill_serverを使っていない() {
    let root = workspace_root();
    let text = read(&root, "crates/tako-core/src/tmux_cleanup.rs");
    let named = text
        .split("fn reclaim_named")
        .nth(1)
        .expect("reclaim_named が無い（#1282 の回収本体）");
    let body = named.split("\n}\n").next().unwrap_or(named);
    assert!(
        !body.contains("kill-server"),
        "名前で列挙した器へ kill-server を打っている。\n\
         psmux の kill-server は -L を落とすと全ソケットを殺し、返らないことがある\n\
         （#1271 の実測）ので、この経路は pid 指定だけで落とすこと（#1282）"
    );
}

#[test]
fn ソケットファイルが無い環境でも器を列挙する() {
    let root = workspace_root();
    let text = read(&root, "crates/tako-core/src/tmux_cleanup.rs");
    assert!(
        text.contains("fn scan_named_servers"),
        "名前で列挙する実装（scan_named_servers）が無い = #1282 の症状そのもの"
    );
    // `scan_servers` の `None` 側（= ソケット置き場が無い）が名前列挙へ行くこと
    let scan = text
        .split("pub fn scan_servers()")
        .nth(1)
        .expect("scan_servers が無い");
    let body = scan.split("\n}\n").next().unwrap_or(scan);
    assert!(
        body.contains("scan_named_servers()"),
        "ソケット置き場が無いとき（Windows / psmux）に器の列挙を諦めている。\n\
         現在の scan_servers:\n{body}"
    );
    // 回収の判定も名前列挙の器を扱えること
    assert!(
        text.contains("ServerKind::Named"),
        "名前で指す器（ServerKind::Named）を判定・回収が扱っていない"
    );
}

/// 所有者を特定できない器・再 attach されうる器は**消さない**（安全側の維持）
#[test]
fn 材料が取れない器は回収対象にしていない() {
    let root = workspace_root();
    let text = read(&root, "crates/tako-core/src/tmux_cleanup.rs");
    for needle in [
        "ServerVerdict::OwnerUnknown",
        "ServerVerdict::PeerMayReattach",
        "OwnerSource::CommandLine",
    ] {
        assert!(
            text.contains(needle),
            "{needle} が無い。所有者の材料が取れない器を見送る道が消えている（#1282）"
        );
    }
    // is_actionable が増えていないこと（触ってよいのは 2 種類だけ）
    let actionable = text
        .split("fn is_actionable")
        .nth(1)
        .expect("is_actionable が無い");
    let body = actionable.split("\n    }\n").next().unwrap_or(actionable);
    assert!(
        body.contains("Self::Reclaimable { .. } | Self::StaleSocket"),
        "`--apply` で触る判定が増えている（見送りの種別を actionable にしていないか）:\n{body}"
    );
}

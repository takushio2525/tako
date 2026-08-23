//! #898 の実機 e2e: コマンド解決（旧 `dispatch::which`）が実機で正しいパスを返し、
//! それを材料にしている機能が動くこと。
//!
//! ```text
//! cargo test -p tako-control --test issue898_command_resolution_e2e -- --ignored --nocapture
//! ```
//!
//! **Windows でこれが通ることが #898 の受け入れそのもの**。修正前は `which` コマンドを
//! 起こしていたが Windows に `which` は存在しない（実測: 「用語 'which' は…認識されません」）
//! ので、解決は例外なく `None` になっていた = tako.exe が PATH 上に居るのに tako 自身には
//! 「無い」ように見える状態。
//!
//! A/B の取り方（このファイルは残したまま製品側だけ戻す）:
//!
//! ```text
//! git checkout origin/main -- crates/tako-control/src/dispatch.rs \
//!                             crates/tako-control/src/stale_binary.rs
//! ```
//!
//! `--ignored` にしてあるのは、解決結果が**そのマシンに何が入っているか**に依存するため
//! （CI のランナーには claude も tmux も無い）。

/// 解決結果が「絶対パスで、実在するファイル」であることを確かめる
fn assert_resolved(name: &str, got: Option<String>) -> String {
    let path = got.unwrap_or_else(|| {
        panic!("{name} を解決できない（#898 の症状。PATH には居るか Get-Command で確認する）")
    });
    println!("  {name} -> {path}");
    let p = std::path::Path::new(&path);
    assert!(p.is_absolute(), "{name}: 絶対パスでない: {path}");
    assert!(p.is_file(), "{name}: 実在しない: {path}");
    path
}

/// 境界 B16 が実機の PATH を解決できること（旧 `which` の置き換え先そのもの）。
///
/// `tako` は #898 が名指しした本命、`claude` は MCP 自動登録と stale 検知（#498）、
/// `tmux` は `tako_check_health` の `tmux_available` が見るもの。
/// `codex` / `agy` は設定画面のエージェント検出（未導入なら `None` が正しい）
#[test]
#[ignore = "解決結果がそのマシンの導入状況に依存する（実機 e2e）"]
fn 境界b16がtako_claude_tmuxを解決する() {
    println!("必須の 3 つ:");
    let tako = assert_resolved("tako", tako_core::platform::exe::find("tako"));
    assert_resolved("claude", tako_core::platform::exe::find("claude"));
    assert_resolved("tmux", tako_core::platform::exe::find("tmux"));

    // Windows なら実行ファイルの拡張子が付いていること（`PATHEXT` を見ている証拠）
    if cfg!(windows) {
        let lower = tako.to_ascii_lowercase();
        assert!(
            lower.ends_with(".exe") || lower.ends_with(".cmd") || lower.ends_with(".bat"),
            "Windows なのに実行ファイルの拡張子が付いていない: {tako}"
        );
    }

    // 未導入でも落ちない（設定画面のエージェント検出が通る形）
    println!("任意の 2 つ（未導入なら None が正しい）:");
    for name in ["codex", "agy"] {
        println!("  {name} -> {:?}", tako_core::platform::exe::find(name));
    }
}

/// #898 の影響 ①: MCP 自動登録・handoff・スターターが使う tako CLI の実体解決。
///
/// 修正前は解決に失敗して**裸の `tako`** へ落ちるので、PATH に依存しないと動かない
/// コマンドを書いてしまっていた（zip 展開だけの導入で動かない）
#[test]
#[ignore = "解決結果がそのマシンの導入状況に依存する（実機 e2e）"]
fn resolve_tako_binaryが実体パスを返す() {
    let got = tako_control::dispatch::resolve_tako_binary();
    println!("resolve_tako_binary -> {got}");
    assert_ne!(
        got, "tako",
        "裸の `tako` に落ちている = #898 の症状（解決も隣の探索も空振りした）"
    );
    let p = std::path::Path::new(&got);
    assert!(p.is_absolute(), "絶対パスでない: {got}");
    assert!(p.is_file(), "実在しない: {got}");
}

/// #898 の影響 ②: stale claude バイナリ検知（#498）の材料。
///
/// 修正前は PATH 走査が空振りしたときの保険が `which` だったので、Windows では
/// 検知が丸ごと無効（版の張り直しが起きない）だった
#[test]
#[ignore = "claude の導入が要る（実機 e2e）"]
fn stale検知がclaudeランチャと指紋を引ける() {
    let launcher = tako_control::stale_binary::launcher_path();
    println!("launcher_path -> {launcher:?}");
    let launcher = launcher.expect("claude ランチャを引けない = #898 の症状（#498 が無効）");
    assert!(launcher.is_absolute(), "絶対パスでない: {launcher:?}");

    let fp = tako_control::stale_binary::current_binary_fingerprint();
    println!("current_binary_fingerprint -> {fp:?}");
    assert!(
        fp.is_some(),
        "指紋が取れない = 版の張り直しを検知できない（#498 が無効）"
    );
}

//! 外部エージェント CLI（claude / codex / agy）を**問い合わせ目的で起こす唯一の入口**
//! （Issue #1261。#944 / #1253 と同型）
//!
//! 空の `HOME` で `cargo test --workspace` を回すと、テストプロセスから実 CLI が起動され、
//! **CLI 自身が自分のホームを作る**（実測: `~/.gemini` に 32 ファイル・
//! `~/.codex/tmp/arg0/…/.lock`・`~/.claude.json` と `~/.claude/backups/`）。
//! tako 側が 1 バイトも書かなくても、起こした時点で汚染は起きる。
//!
//! 起動元は空 HOME + 偽 CLI（呼ばれたら argv を記録する shim）で 2 つに特定した:
//!
//! | 起動元のテスト | 経路 | 実際に起きた CLI |
//! |---|---|---|
//! | `setup_bootstrap::tests::状態は3系統ぶんまとめて返せる` | `status_all_json` → [`crate::setup_bootstrap::is_authenticated_for`] | `claude auth status --json` / `codex login status` / `agy models` |
//! | `stale_binary::tests::test_check_stale_different_binary` | `check_stale` → `extract_version_from_path` → `extract_version_via_cli` | `claude --version` |
//!
//! 塞ぎ方は #1253 の brew（`update_checker::detect_install_method_full`）と同じで、
//! **判定は実行時**（[`tako_core::paths::is_test_process`]）。`cfg(test)` は
//! クレートを跨がないので、統合テスト（`crates/*/tests/`）から呼ばれた lib は素通りする。
//!
//! 門番をここ 1 か所に置くのは、呼ぶ側それぞれに `if` を散らすと
//! **新しい問い合わせを足したときに素通りする**から。番犬
//! `crates/tako-control/tests/verification_isolation_watchdog.rs` が
//! 「問い合わせ経路が [`run`] を通っていること」を走査で拘束する。
//!
//! 対象外（意図して実 CLI を起こす経路）:
//!
//! - ペインの中で対話的に動く agent 本体（`terminal` / backend 経由の spawn）。
//!   検証プロセスの設定の書き先は #1253 の `orchestrator::agent_config_home` が倒す
//! - `claude agents --json`（[`crate::orchestrator`]）。あちらは `cfg(test)` で塞ぎ、
//!   **統合テスト `tests/issue877_agents_scan_e2e.rs` には実 CLI を通す**のが #944 の設計
//! - **隔離 GUI（`TAKO_ISOLATED=1` / `TAKO_SELF_TEST=1`）は塞がない**。判定を
//!   [`tako_core::paths::is_verification_process`] まで広げると、隔離 GUI の
//!   `setup bootstrap` は「未ログイン」、モデルピッカーは静的一覧しか出せなくなり、
//!   セルフテストが見ている画面が本番と変わってしまう。#1253 の brew も同じ理由で
//!   [`tako_core::paths::is_test_process`] を使っている。
//!   代償として**隔離 GUI から設定画面のモデル一覧や `tako_setup_bootstrap` を叩くと
//!   実 CLI は起きる**（実測: `TAKO_ISOLATED=1 TAKO_SELF_TEST=1` の
//!   `tako setup bootstrap status-all` で 3 系統とも起動した）。塞ぐなら別 Issue で、
//!   「隔離中は問い合わせ結果を偽装する」形（`None` ではなく既知の応答）を用意すること

/// 問い合わせのための起動が禁じられているか。
///
/// テストプロセス（`<target>/<profile>/deps/<名前>-<hash>`）では常に真。
/// A/B の逃げ道は #944 / #1253 と共通の `TAKO_944_LEGACY=1`
/// （「テストプロセスの隔離を切って旧挙動を再現する」1 つのスイッチ）
pub fn blocked() -> bool {
    tako_core::paths::is_test_process() && !tako_core::paths::issue944_legacy()
}

/// エージェント CLI を問い合わせ目的で起こす。
///
/// 禁じられているときは**プロセスを 1 つも作らずに** `None` を返す
/// （CLI が入っていない CI と同じ「取得できない」経路へ倒れるので、
/// 結果もローカル / CI で揃う）。#586 のコンソール窓抑止もここで一度だけ当てる
pub fn run(cmd: &mut std::process::Command) -> Option<std::process::Output> {
    if blocked() {
        return None;
    }
    tako_core::platform::process::no_console_window(cmd)
        .output()
        .ok()
}

#[cfg(test)]
mod tests {
    /// テストプロセスからは 1 プロセスも起こさない（`true` を返す実コマンドでも `None`）
    #[test]
    fn テストプロセスでは問い合わせを起こさない() {
        assert!(super::blocked(), "テストバイナリなのに門番が開いている");
        let program = if cfg!(windows) { "cmd" } else { "true" };
        assert!(
            crate::agent_probe::run(&mut std::process::Command::new(program)).is_none(),
            "門番が閉じているのに子プロセスを起こした（#1261）"
        );
    }
}

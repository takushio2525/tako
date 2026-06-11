//! tako-cli — Layer 1 CLI（Phase 2 で実装）
//!
//! `TAKO_SOCKET` + `TAKO_TOKEN` を読んで IPC サーバーに JSON-RPC で接続する。
//! サブコマンド: split / send / focus / list / read / close / title / resize / layout
//! （操作カタログは `.agent/requirements.md` の FR-2.5）

use std::process::ExitCode;

fn main() -> ExitCode {
    // Phase 2 で実装。現時点はワークスペース構成確定のためのスタブ。
    // FR-2.2.8: tako の外で実行された場合のエラーと同系統のメッセージを先取りしておく
    if std::env::var_os("TAKO_SOCKET").is_none() {
        eprintln!("error: tako アプリ内のターミナルで実行してください（TAKO_SOCKET が未設定）");
    } else {
        eprintln!("error: tako CLI は未実装です（Phase 2 で実装予定）");
    }
    ExitCode::FAILURE
}

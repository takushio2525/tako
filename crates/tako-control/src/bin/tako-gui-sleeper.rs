//! テスト専用の GUI サブシステムの実行ファイル（#2454。眠るだけで窓は出さない）
//!
//! ペインを閉じたとき、ペインから起こした GUI アプリ（`code .`・`notepad` など）は残し、
//! コンソールのプロセスは止める（`tako_core::platform::job` の寿命の決まり）。その「GUI アプリ」の
//! 役をする。Windows では PE ヘッダのサブシステムが GUI（`IMAGE_SUBSYSTEM_WINDOWS_GUI`）になり、
//! tako は実行ファイルのヘッダでそれを見分ける。Windows 以外では属性が効かず、ただ眠るだけ。
//!
//! 配布物には入らない（`scripts/build-app.sh` / Windows インストーラは `tako-app` と `tako` だけを
//! 名指しで同梱する）。テストからは `env!("CARGO_BIN_EXE_tako-gui-sleeper")` で絶対パスを取る。
//!
//! 引数: 眠る秒数（省略時 600）。テストは自分が起こしたこのプロセスを自分で止める。

#![windows_subsystem = "windows"]

fn main() {
    let secs = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(600);
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

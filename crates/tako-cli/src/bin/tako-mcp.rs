//! MCP の stdio ブリッジ専用の小さな実行ファイル（#2021）。
//!
//! 登録のコマンドは今までどおり `tako mcp serve` で、macOS の .app に入っている `tako` が
//! 同じ `Contents/MacOS` に置かれたこれへ exec で引き渡す（`tako_cli::mcp_bridge`）。
//! `tako` は CoreFoundation / IOKit / CoreServices / libobjc をリンクしていて、起動するだけで
//! 約 350 個の像と約 0.5 MB の dirty なページを持つ。ブリッジの到達するコードはどれも使わない
//! ので、ここは `-dead_strip_dylibs`（`build.rs`）で libSystem だけをリンクする。
//!
//! 引数は「無し」か `mcp serve`（`tako` からの引き渡し・カタログ係の起こし方）だけを受ける

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    // #1811: 製品の入口を通ったと宣言する（**必ず 1 文目**）。`tako` から引き渡されたときも
    // exec で像が入れ替わるので、ここでもう一度名乗る
    tako_core::paths::mark_product_process();
    // #1805: MCP クライアントが stderr の読み手を閉じていても、ライブラリ側の `eprintln!`
    // （gate check の `[gate check] command:` 等）で panic せず行を捨てて続ける。
    // 標準出力の規則は採らない: ブリッジの stdout はプロトコルの通り道で、書き込みの失敗は
    // ブリッジ自身が `Result` で扱う（ここには巻き戻しを受け止める入口のスレッドも無い）
    tako_core::stdio::drop_lines_on_closed_stderr();
    // `tako` が引き渡しのときに立てた malloc の設定は、自分の malloc がもう読み終えている。
    // ブリッジが起こす子（カタログ係・受け入れゲートのコマンド）へ渡さないよう消す
    tako_cli::mcp_bridge::forget_malloc_tuning();
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some() && !tako_cli::mcp_bridge::is_mcp_serve_invocation(args) {
        // std の `eprintln!` は書けないと panic するので、書けなくても捨てる（#1758）
        let _ = writeln!(
            std::io::stderr(),
            "tako-mcp は MCP の stdio ブリッジ専用（引数なし か `mcp serve`）。\
             MCP クライアントへの登録は `tako setup-mcp` が行う"
        );
        return ExitCode::from(2);
    }
    // スタックは既定のまま（macOS は 8 MiB、Windows は build.rs の /stack で 8 MiB）。
    // カタログ（`tools`）を組む深いフレームはカタログ係と倒れ先だけが通る
    tako_cli::mcp_bridge::run()
}

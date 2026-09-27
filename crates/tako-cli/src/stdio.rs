//! 標準出力・標準エラーへの書き込み口（Issue #1758）
//!
//! **このクレートの `println!` / `print!` / `eprintln!` / `eprint!` は std のものではなく、
//! ここで定義した同名のマクロを指す**。`main.rs` の先頭で `#[macro_use] mod stdio;` と
//! 宣言しているので、その後ろに並ぶ `mod setup;` と `main.rs` の全体へ効く
//! （宣言の順を崩すと std の版へ戻る = 番犬 `tests/issue1758_broken_pipe.rs`）。
//!
//! std の版は書き込みに失敗すると panic する。Rust は SIGPIPE を無視した状態で main に
//! 入るので、`tako … | head -1` のように読み手が先に閉じると EPIPE がそのまま
//! `failed printing to stdout: Broken pipe` の panic になっていた（修正前の実測:
//! panic メッセージ 2 つ + 終了コード 101）。Windows には SIGPIPE が無く、同じ場面は
//! `ERROR_NO_DATA` = `ErrorKind::BrokenPipe` の Err で返る。
//!
//! 読み手が先に閉じたときの扱い:
//!
//! - **標準出力** → そこで打ち切り、**終了コード 0 で静かに終わる**（ripgrep と同じ）。
//!   読み手が要るぶんを読んで閉じたのは tako の失敗ではない。`resume_unwind` で
//!   巻き戻すので panic フックを通らず（メッセージを出さない）、`Drop` は panic の
//!   ときと同じく走る。`main` が印（[`StdoutClosed`]）を見て 0 を返す
//! - **標準エラー** → 書けなかった行を捨てて**処理を続ける**。診断の行が届かないだけで、
//!   ここで打ち切ると「失敗した」の終了コード（`error: …` の後の 1）が 0 に化ける
//!
//! SIGPIPE を既定へ戻す案は採らない: Windows に効かない / 同じバイナリの
//! `tako remote serve`（起動情報を読んだ親が stdio のパイプを捨てる daemon）が
//! 1 行の出力でプロセスごと死ぬ / `tako mcp serve` の終わり方が「エラー終了」から
//! 「シグナル死」へ変わる。

use std::fmt;
use std::io::{self, Write};

/// 標準出力の読み手が先に閉じたことを `main` へ運ぶ印（`resume_unwind` の payload）。
///
/// `tako-main` 以外のスレッドから出力するなら、その join 側でもこの印を見ること
/// （見ずに `expect` すると、そこで panic メッセージが出る）
pub(crate) struct StdoutClosed;

/// `println!` / `print!` の実体
pub(crate) fn out(args: fmt::Arguments<'_>) {
    if cfg!(test) {
        // 単体テストでは std の経路へ流す。テストハーネスの出力の捕捉は std の `print!` に
        // しか効かず、直に書くと合格したテストの出力まで一覧へ混ざる。切断の扱いは
        // 実バイナリを起こす `tests/issue1758_broken_pipe.rs` が見る
        std::print!("{args}");
        return;
    }
    if let Err(e) = io::stdout().lock().write_fmt(args) {
        on_stdout_error(e);
    }
}

/// `eprintln!` / `eprint!` の実体
pub(crate) fn err(args: fmt::Arguments<'_>) {
    if cfg!(test) {
        // `out` と同じ理由（テストハーネスの出力の捕捉）
        std::eprint!("{args}");
        return;
    }
    if let Err(e) = io::stderr().lock().write_fmt(args) {
        // 読み手が居ない = この行は誰にも届かない。打ち切らずに続ける（モジュール先頭）
        if e.kind() != io::ErrorKind::BrokenPipe {
            panic!("failed printing to stderr: {e}");
        }
    }
}

fn on_stdout_error(e: io::Error) {
    if e.kind() != io::ErrorKind::BrokenPipe {
        // それ以外（書き先のディスクが一杯 等）は std と同じ扱いに留める
        panic!("failed printing to stdout: {e}");
    }
    if std::thread::panicking() {
        // 巻き戻し中の `Drop` からの出力。ここで重ねて巻き戻すと abort になる
        return;
    }
    if cfg!(panic = "abort") {
        // 巻き戻せないビルドでは、同じ結果（静かに 0）をその場で出す
        std::process::exit(0);
    }
    std::panic::resume_unwind(Box::new(StdoutClosed));
}

macro_rules! println {
    () => {
        $crate::stdio::out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::stdio::out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

macro_rules! print {
    ($($arg:tt)*) => {
        $crate::stdio::out(format_args!($($arg)*))
    };
}

macro_rules! eprintln {
    () => {
        $crate::stdio::err(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::stdio::err(format_args!("{}\n", format_args!($($arg)*)))
    };
}

macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::stdio::err(format_args!($($arg)*))
    };
}

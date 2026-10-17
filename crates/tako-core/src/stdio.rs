//! 標準出力・標準エラーへの書き込み口の 1 実装（Issue #1758 / #1805）
//!
//! **tako-core / tako-control / tako-cli の `println!` / `print!` / `eprintln!` / `eprint!` は
//! std のものではなく、各クレートの根で `#[macro_use] mod stdio;` と宣言した同名のマクロを
//! 指し、どれもここの [`out`] / [`err`] を呼ぶ**（宣言の順を崩すと std の版へ戻る =
//! 番犬 `crates/tako-cli/tests/issue1758_broken_pipe.rs`）。
//!
//! std の版は書き込みに失敗すると panic する。Rust は SIGPIPE を無視した状態で main に
//! 入るので、`tako … | head -1` のように読み手が先に閉じると EPIPE がそのまま
//! `failed printing to stdout: Broken pipe` の panic になる。Windows には SIGPIPE が無く、
//! 同じ場面は `ERROR_NO_DATA` = `ErrorKind::BrokenPipe` の Err で返る。#1758 は tako-cli の
//! 出力だけを直したので、CLI の経路で走るライブラリ側の出力（`[gate check] command:` 等）は
//! 読み手の閉じた stderr で panic したままだった（#1805）。
//!
//! 読み手が先に閉じたときの扱い（**CLI の規則**。プロセスの入口が採ったときだけ効く）:
//!
//! - **標準出力** → そこで打ち切り、**終了コード 0 で静かに終わる**（ripgrep と同じ）。
//!   読み手が要るぶんを読んで閉じたのは tako の失敗ではない。`resume_unwind` で
//!   巻き戻すので panic フックを通らず（メッセージを出さない）、`Drop` は panic の
//!   ときと同じく走る。入口が印（[`StdoutClosed`]）を見て 0 を返す
//! - **標準エラー** → 書けなかった行を捨てて**処理を続ける**。診断の行が届かないだけで、
//!   ここで打ち切ると「失敗した」の終了コード（`error: …` の後の 1）が 0 に化ける
//!
//! **入口が規則を採るまでは std の版と同じ**（[`stop_quietly_on_closed_stdout`] /
//! [`drop_lines_on_closed_stderr`]）。GUI（tako-app）は採らないので、GUI のプロセスで走る
//! ライブラリの出力は今までと 1 バイトも変わらない。テストのビルドも採らないので、
//! テストハーネスの出力の捕捉（std の `print!` にしか効かない）もそのまま効く。
//!
//! SIGPIPE を既定へ戻す案は採らない: Windows に効かない / 同じバイナリの
//! `tako remote serve`（起動情報を読んだ親が stdio のパイプを捨てる daemon）が
//! 1 行の出力でプロセスごと死ぬ / `tako mcp serve` の終わり方が「シグナル死」になる。

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// 標準出力の読み手が先に閉じたことを入口へ運ぶ印（`resume_unwind` の payload）。
///
/// 入口のスレッド以外から出力するなら、その join 側でもこの印を見ること
/// （見ずに `expect` すると、そこで panic メッセージが出る）
pub struct StdoutClosed;

static STOP_ON_CLOSED_STDOUT: AtomicBool = AtomicBool::new(false);
static DROP_ON_CLOSED_STDERR: AtomicBool = AtomicBool::new(false);

/// 標準出力の読み手が閉じたら [`StdoutClosed`] で巻き戻して打ち切る（`tako` の main が採る）。
///
/// 採るのは**巻き戻しを受け止める入口**だけ（受け止めないと payload のまま 101 で終わる）
pub fn stop_quietly_on_closed_stdout() {
    STOP_ON_CLOSED_STDOUT.store(true, Ordering::Relaxed);
}

/// 標準エラーの読み手が閉じたら、その行を捨てて続ける（`tako` / `tako-mcp` の main が採る）
pub fn drop_lines_on_closed_stderr() {
    DROP_ON_CLOSED_STDERR.store(true, Ordering::Relaxed);
}

/// `println!` / `print!` の実体
pub fn out(args: fmt::Arguments<'_>) {
    if !STOP_ON_CLOSED_STDOUT.load(Ordering::Relaxed) {
        std::print!("{args}");
        return;
    }
    if let Err(e) = io::stdout().lock().write_fmt(args) {
        on_stdout_error(e);
    }
}

/// `eprintln!` / `eprint!` の実体
pub fn err(args: fmt::Arguments<'_>) {
    if !DROP_ON_CLOSED_STDERR.load(Ordering::Relaxed) {
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

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! println {
    () => {
        $crate::stdio::out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::stdio::out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::stdio::out(format_args!($($arg)*))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprintln {
    () => {
        $crate::stdio::err(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::stdio::err(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::stdio::err(format_args!($($arg)*))
    };
}

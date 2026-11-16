//! 標準出力・標準エラーへの書き込み口の 1 実装（Issue #1758 / #1805 / #2936）
//!
//! **tako-core / tako-control / tako-cli / tako-app の `println!` / `print!` / `eprintln!` /
//! `eprint!` は std のものではなく、各クレートの根で `#[macro_use] mod stdio;` と宣言した同名の
//! マクロを指し、どれもここの [`out`] / [`err`] を呼ぶ**（宣言の順を崩すと std の版へ戻る =
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
//! [`drop_lines_on_closed_stderr`]）。テストのビルドは採らないので、テストハーネスの出力の
//! 捕捉（std の `print!` にしか効かない）もそのまま効く。
//!
//! **GUI の規則**（#2936。tako-app の main が 2 文目で [`adopt_gui_rule`] を採る）: 標準出力・
//! 標準エラーとも、書き込みの失敗は**種類によらず**捨てて続ける。GUI の stdout / stderr は
//! 診断を流すだけで読み手が居なくてもよく、書けないことは失敗ではない。std の版が黙って
//! 捨てるのは「ハンドルが無い」（unix の EBADF / Windows の `ERROR_INVALID_HANDLE` = エクスプローラー
//! から起こした GUI）だけで、それ以外は panic してアプリごと落ちていた。実測は Windows で、
//! 予約タスクから `Start-Process` で起こした GUI（親がすぐ終わり、受け継いだ std ハンドルが
//! 書けない別物を指していた）が器の取り合いの警告（`backend::psmux` の `wrap_spawn`）で
//! `failed printing to stderr: アクセスが拒否されました。 (os error 5)` を出して落ちた。
//! macOS でも stderr の読み手が先に閉じたパイプ（親のスクリプトが終わった等）なら EPIPE で
//! 同じことが起きる。書けた分の出力は std の版と 1 バイトも変わらない（同じハンドル・同じ
//! 書式・同じ同期の書き込み）。A/B は [`GUI_RULE_LEGACY_ENV`]（採らない = 修正前の形）。
//!
//! 起動時に stderr を検査して向け直す案は採らない: 起動の後で壊れる場合（親が後から閉じた
//! パイプ）を守れない。全 `eprintln!` を失敗を捨てる書き方へ置き換える案も採らない: 数が多く
//! （tako-app だけで 1,200 か所超）、後から足す行を書き方で縛るしかない。マクロの差し替えなら
//! 後から足す行も宣言の順だけで同じ口を通る（番犬が宣言の順と入口の採用を見る）。stderr を自前の
//! パイプへ差し替えて転送する案も採らない: 終了の直前の行を落とし、GUI の stderr を受け継いだ子
//! （標準エラーを指定せずに起こした子）が GUI より長生きすると、転送の先が GUI と一緒に消える。
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
static DROP_ALL_WRITE_ERRORS: AtomicBool = AtomicBool::new(false);

/// GUI の規則の A/B（#2936）。`1` なら [`adopt_gui_rule`] が規則を採らない = 修正前の形
/// （GUI の出力は std の版と同じく、書けなければ panic する）
pub const GUI_RULE_LEGACY_ENV: &str = "TAKO_2936_LEGACY";

/// GUI の規則を採る（#2936。tako-app の main の 2 文目）。標準出力・標準エラーへの書き込みの
/// 失敗を種類によらず捨てて続ける。採ったら `true`、A/B（[`GUI_RULE_LEGACY_ENV`]）で
/// 採らなかったら `false`。
///
/// **GUI 以外の入口では採らない**: CLI は端末やパイプへ出すのが本分で、読み手が閉じたときの
/// 扱いは CLI の規則（[`stop_quietly_on_closed_stdout`] / [`drop_lines_on_closed_stderr`]）が決める
pub fn adopt_gui_rule() -> bool {
    if std::env::var_os(GUI_RULE_LEGACY_ENV).is_some_and(|v| v == "1") {
        return false;
    }
    DROP_ALL_WRITE_ERRORS.store(true, Ordering::Relaxed);
    true
}

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
    if DROP_ALL_WRITE_ERRORS.load(Ordering::Relaxed) {
        // GUI の規則: 書けなければ捨てて続ける（モジュール先頭）
        let _ = io::stdout().lock().write_fmt(args);
        return;
    }
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
    if DROP_ALL_WRITE_ERRORS.load(Ordering::Relaxed) {
        // GUI の規則: 書けなければ捨てて続ける（モジュール先頭）
        let _ = io::stderr().lock().write_fmt(args);
        return;
    }
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

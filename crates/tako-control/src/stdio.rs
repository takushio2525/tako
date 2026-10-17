//! 出力マクロの差し替え（Issue #1805）
//!
//! **このクレートの `println!` / `print!` / `eprintln!` / `eprint!` は std のものではなく、
//! ここで定義した同名のマクロを指す**（`lib.rs` の最初の `mod` 宣言の `#[macro_use]`）。
//! 実体は `tako_core::stdio` の 1 実装で、読み手の閉じたパイプへの書き込みで panic しない
//! 規則（標準出力 → 静かに 0 / 標準エラー → 行を捨てて続ける）は CLI の入口が採ったときだけ
//! 効く。GUI のプロセスとテストのビルドでは std の版と同じ（理由は `tako_core::stdio` の先頭）

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! println {
    () => {
        ::tako_core::stdio::out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! print {
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!($($arg)*))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprintln {
    () => {
        ::tako_core::stdio::err(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprint {
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!($($arg)*))
    };
}

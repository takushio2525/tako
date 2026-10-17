//! 標準出力・標準エラーへの書き込み口（Issue #1758 / #1805）
//!
//! **このクレートの `println!` / `print!` / `eprintln!` / `eprint!` は std のものではなく、
//! ここで定義した同名のマクロを指す**。`main.rs` と `lib.rs` の先頭で `#[macro_use] mod stdio;`
//! と宣言しているので、その後ろに並ぶ `mod` と各ファイルの全体へ効く
//! （宣言の順を崩すと std の版へ戻る = 番犬 `tests/issue1758_broken_pipe.rs`）。
//!
//! 実体は `tako_core::stdio` の 1 実装で、tako-core / tako-control の出力も同じ口を通る
//! （#1805。CLI の経路で走るライブラリ側の `[gate check] command:` 等も含めて同じ規則）。
//! `main` が入口で規則を採る:
//!
//! - **標準出力**の読み手が先に閉じた → そこで打ち切り、**終了コード 0 で静かに終わる**
//!   （`main` が印 `tako_core::stdio::StdoutClosed` を見て 0 を返す）
//! - **標準エラー**の読み手が先に閉じた → 書けなかった行を捨てて**処理を続ける**
//!
//! 規則を採るのは実行ファイルの入口だけなので、単体テストのビルドでは std の版と同じ
//! （テストハーネスの出力の捕捉は std の `print!` にしか効かない）。切断の扱いは実バイナリを
//! 起こす `tests/issue1758_broken_pipe.rs` / `tests/issue1805_lib_stdio.rs` が見る。
//! 直し方の詳細と、SIGPIPE を既定へ戻す案を採らない理由は `tako_core::stdio` の先頭。

#[allow(unused_macros)] // lib.rs（ブリッジ・IPC クライアント）は今は使わない
macro_rules! println {
    () => {
        ::tako_core::stdio::out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // lib.rs（ブリッジ・IPC クライアント）は今は使わない
macro_rules! print {
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!($($arg)*))
    };
}

#[allow(unused_macros)] // lib.rs（ブリッジ・IPC クライアント）は今は使わない
macro_rules! eprintln {
    () => {
        ::tako_core::stdio::err(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // lib.rs（ブリッジ・IPC クライアント）は今は使わない
macro_rules! eprint {
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!($($arg)*))
    };
}

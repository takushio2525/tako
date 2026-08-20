//! プラットフォーム依存を閉じ込める抽象境界（`tako-core` 分）。
//!
//! 設計の正は `.agent/plans/2026-07-windows-port-architecture.md`。
//! 原則: **`cfg(target_os)` / `cfg(unix)` を書いてよいのはこのモジュール配下だけ**。
//! 呼び出し側は単一のコードパスを持つ。
//!
//! 新しくプラットフォーム分岐が必要になったら、呼び出し側に `cfg` を足すのではなく
//! ここに境界を追加する。

pub mod clock;
pub mod console;
pub mod exe;
pub mod font;
pub mod ime;
pub mod install_info;
pub mod locale;
pub mod process;
pub mod procinfo;
pub mod quit_signal;
pub mod release_assets;
pub mod shell;
pub mod support;

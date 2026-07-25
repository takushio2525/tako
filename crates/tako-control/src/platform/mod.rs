//! プラットフォーム依存を閉じ込める抽象境界（`tako-control` 分）。
//!
//! 設計の正は `.agent/plans/2026-07-windows-port-architecture.md`。
//! 原則: **`cfg(target_os)` / `cfg(unix)` を書いてよいのはこのモジュール配下だけ**。
//! 呼び出し側（dispatch / remote / CLI）は単一のコードパスを持つ。
//!
//! 新しくプラットフォーム分岐が必要になったら、呼び出し側に `cfg` を足すのではなく
//! ここに境界を追加する。

pub mod local_endpoint;
pub mod os_integration;
pub mod process;

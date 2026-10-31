//! プラットフォーム依存を閉じ込める抽象境界（`tako-core` 分）。
//!
//! 設計の正は `.agent/plans/2026-07-windows-port-architecture.md`。
//! 原則: **`cfg(target_os)` / `cfg(unix)` を書いてよいのはこのモジュール配下だけ**。
//! 呼び出し側は単一のコードパスを持つ。
//!
//! 新しくプラットフォーム分岐が必要になったら、呼び出し側に `cfg` を足すのではなく
//! ここに境界を追加する。

pub mod agent_install;
pub mod bundle_install;
pub mod child_cmd;
pub mod clock;
pub mod console;
pub mod display;
pub mod dpi;
pub mod editor_keys;
pub mod exe;
pub mod fd_inherit;
pub mod fd_limit;
pub mod font;
pub mod fs_copy;
/// 上書きしない移動（境界 B28 の core 分。#1842）
pub mod fs_move;
pub mod ime;
pub mod install_info;
/// 子孫をまとめて回収する入れ物（境界 B30。#2454）
pub mod job;
/// cargo の共有 jobserver の fifo（境界 B31。#1998）
pub mod jobserver;
pub mod keys;
pub mod locale;
pub mod path;
/// 本人だけが読み書きできるファイル・置き場（境界 B32。#1632）
pub mod private_file;
pub mod process;
pub mod procinfo;
pub mod program_path;
pub mod quit_signal;
pub mod release_assets;
/// PC 全体と各プロセスの資源の計測（境界 B29。#1993）
pub mod resources;
pub mod runner_defaults;
pub mod shell;
pub mod shell_dialect;
pub mod ssh_client;
pub mod stack;
pub mod support;
pub mod user_path;
/// Windows の Web ビュー（WebView2）の置き場・ランタイムの版・生成の失敗の案内（#2431）
pub mod webview2;
pub mod window_bounds;
pub mod window_lifecycle;

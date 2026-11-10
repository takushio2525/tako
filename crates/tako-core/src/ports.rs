//! ports — listen ポート検知（FR-2.4.2。Layer 3 パッシブ検知の素材）の公開の名前
//!
//! ペイン配下のプロセスが LISTEN している TCP ポートの列挙と、それに使うプロセス検査
//! （プロセス表・初期環境変数・tako-app の判定・生死）の**呼び出し側の綴り**を保つ層。
//! 実装は抽象境界 B5 の `crate::platform::procinfo` にある（macOS = `procinfo/macos.rs` の
//! libproc、Windows = Toolhelp32 / `GetExtendedTcpTable`。#524）。
//!
//! #2856 まで macOS の実装（libproc）はこのファイルにあり、境界の側
//! （`platform::procinfo` / `platform::resources`）がここを呼ぶ逆向きの依存になっていた。
//! いまは**このファイルに OS の `cfg` を置かない**: 呼び出し側（4 クレート）と番犬が
//! `ports::scan` / `ports::process_alive` などの綴りで依っているので、名前だけを
//! re-export する。新しい OS 実装は境界の側へ足すこと（`ports.rs` に `cfg` を戻す形と、
//! 境界から `crate::ports` を呼ぶ形は `crates/tako-control/tests/platform_parity.rs` が落とす）

pub use crate::platform::procinfo::{
    is_live_tako_app, listening_ports_of_pid, other_tako_pids, other_tako_running, pane_key,
    process_alive, process_env_vars, process_table, scan, tty_rdev, ListenPort,
};

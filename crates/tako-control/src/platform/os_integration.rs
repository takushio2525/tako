//! OS シェル連携（抽象境界 B8）
//!
//! 「ファイルマネージャで表示」「既定アプリで開く」「指定アプリで開く」など、
//! OS のシェルに委ねる操作をまとめる。
//!
//! - macOS: `open` / `open -R` / `open -a`
//! - Windows: `ShellExecuteW` 相当（未実装。B8 の Windows 実装タスク）
//!
//! 呼び出し側（dispatch / sidebar）はこのモジュールだけを見る。

use std::path::Path;

/// ファイルマネージャ（Finder / エクスプローラー）で対象を選択表示する
pub fn reveal(path: &Path) -> Result<(), String> {
    imp::reveal(path)
}

/// 既定アプリで開く
pub fn open_default(path: &Path) -> Result<(), String> {
    imp::open_default(path)
}

/// アプリ名を指定して開く
pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
    imp::open_with(app, path)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn reveal(path: &Path) -> Result<(), String> {
        spawn_open(&["-R".as_ref(), path.as_os_str()], "Finder を開けない")
    }

    pub fn open_default(path: &Path) -> Result<(), String> {
        spawn_open(&[path.as_os_str()], "デフォルトアプリで開けない")
    }

    pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
        spawn_open(
            &["-a".as_ref(), app.as_ref(), path.as_os_str()],
            &format!("アプリ '{app}' で開けない"),
        )
    }

    fn spawn_open(args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
        std::process::Command::new("open")
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("{what}: {e}"))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn reveal(_path: &Path) -> Result<(), String> {
        Err(unsupported("ファイルマネージャでの表示"))
    }

    pub fn open_default(_path: &Path) -> Result<(), String> {
        Err(unsupported("既定アプリで開く操作"))
    }

    pub fn open_with(_app: &str, _path: &Path) -> Result<(), String> {
        Err(unsupported("アプリを指定して開く操作"))
    }

    fn unsupported(what: &str) -> String {
        format!("{what}はこのプラットフォームでは未対応です")
    }
}

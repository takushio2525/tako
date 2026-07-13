//! tako のユーザーデータ配置先（シェル統合スクリプト・接続情報ファイル等）

use std::path::PathBuf;

/// tako のデータディレクトリ。
/// macOS: `~/Library/Application Support/tako`、その他 unix: `$XDG_DATA_HOME/tako`
/// （無ければ `~/.local/share/tako`）。Windows は Phase 6 で対応する。
/// `TAKO_DATA_DIR` で上書き可能（隔離検証用。#177 / #112: 本番の layout.json /
/// settings.json / token / persist.log に一切触れない起動を 1 変数で作れる）
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TAKO_DATA_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    default_data_dir()
}

fn default_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join("Library/Application Support/tako"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join(".local/share"))
            })
            .map(|d| d.join("tako"))
    }
    #[cfg(windows)]
    {
        None
    }
}

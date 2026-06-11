//! tako のユーザーデータ配置先（シェル統合スクリプト・接続情報ファイル等）

use std::path::PathBuf;

/// tako のデータディレクトリ。
/// macOS: `~/Library/Application Support/tako`、その他 unix: `$XDG_DATA_HOME/tako`
/// （無ければ `~/.local/share/tako`）。Windows は Phase 6 で対応する
pub fn data_dir() -> Option<PathBuf> {
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

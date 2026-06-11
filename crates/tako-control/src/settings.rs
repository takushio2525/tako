//! settings — ユーザー設定の永続化（`<data_dir>/settings.json`）
//!
//! 現状の項目は自動リネームの ON/OFF（FR-2.12.4）のみ。項目追加時は
//! `#[serde(default)]` で後方互換を保つ（未知キーは serde が無視する）。
//! 接続情報（トークン入り）は `discovery` 側で、こちらは秘密を含めない。

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// タブ・ペイン名の AI 自動リネーム（FR-2.12.4。既定 ON）
    #[serde(default = "default_true")]
    pub auto_rename: bool,
    /// listen ポート検知 + 提案チップ（FR-2.4.4。既定 ON）
    #[serde(default = "default_true")]
    pub port_detect: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_rename: true,
            port_detect: true,
        }
    }
}

/// 設定ファイルのパス（`<data_dir>/settings.json`）
pub fn settings_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("settings.json"))
}

/// 設定を読む。ファイルが無い・壊れている場合は既定値（呼び出し側でエラー扱いしない）
pub fn load() -> Settings {
    settings_path()
        .and_then(|p| load_from(&p))
        .unwrap_or_default()
}

fn load_from(path: &std::path::Path) -> Option<Settings> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// 設定を書き出す。tmp へ書いて rename する（読み手と競合しない。discovery と同方式）
pub fn save(settings: &Settings) -> io::Result<PathBuf> {
    let path = settings_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "データディレクトリを解決できない",
        )
    })?;
    save_to(&path, settings)?;
    Ok(path)
}

fn save_to(path: &std::path::Path, settings: &Settings) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリが無い"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tako-settings-test-{}-{name}/settings.json",
            std::process::id()
        ))
    }

    #[test]
    fn 書き出しと読み戻しが往復する() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let settings = Settings {
            auto_rename: false,
            port_detect: false,
        };
        save_to(&path, &settings).unwrap();
        assert_eq!(load_from(&path), Some(settings));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 不在や破損は既定値になる() {
        assert_eq!(load_from(&temp_path("missing")), None);
        assert!(Settings::default().auto_rename);
        // 空オブジェクトでも既定が立つ（後方互換）
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert!(parsed.auto_rename);
        assert!(parsed.port_detect);
    }
}

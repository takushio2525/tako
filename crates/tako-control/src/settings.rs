//! settings — ユーザー設定の永続化（`<data_dir>/settings.json`）
//!
//! 項目追加時は `#[serde(default)]` で後方互換を保つ（未知キーは serde が無視する）。
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
    /// 表示中プレビューファイルのライブリロード（Issue #233。既定 ON）
    #[serde(default = "default_true")]
    pub preview_live_reload: bool,
    /// tmux バックエンドによるセッション永続化（Phase 5.5 / FR-5。既定 ON。
    /// tmux 不在環境では設定に関わらず直接 spawn へ無害劣化する）
    #[serde(default = "default_true")]
    pub tmux_persist: bool,
    /// スリープ防止モード（Issue #173。既定 while-agents-running）
    #[serde(default)]
    pub sleep_guard_mode: crate::sleep_guard::SleepGuardMode,
    /// スリープ防止の電源条件（Issue #173。既定 ac-only）
    #[serde(default)]
    pub sleep_guard_power: crate::sleep_guard::PowerCondition,
    /// 蓋閉じ防止モード（Issue #218。既定 off）
    #[serde(default)]
    pub lid_sleep_mode: crate::sleep_guard::LidSleepMode,
    /// ペインの平文ログ保存（Issue #112 B。既定 ON）
    #[serde(default = "default_true")]
    pub pane_logs: bool,
    /// ペインあたりのログ上限（MB。超過でローテーション）
    #[serde(default = "default_pane_log_max_mb")]
    pub pane_log_max_mb: u64,
    /// ログディレクトリ全体の上限（MB。超過で古いファイルから削除）
    #[serde(default = "default_pane_log_total_max_mb")]
    pub pane_log_total_max_mb: u64,
    /// UI テーマ（Issue #217。"dark" / "light"。既定 dark）
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "dark".into()
}

fn default_true() -> bool {
    true
}

fn default_pane_log_max_mb() -> u64 {
    5
}

fn default_pane_log_total_max_mb() -> u64 {
    200
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_rename: true,
            port_detect: true,
            preview_live_reload: true,
            tmux_persist: true,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::default(),
            sleep_guard_power: crate::sleep_guard::PowerCondition::default(),
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::default(),
            pane_logs: true,
            pane_log_max_mb: default_pane_log_max_mb(),
            pane_log_total_max_mb: default_pane_log_total_max_mb(),
            theme: default_theme(),
        }
    }
}

impl Settings {
    /// ペインログ設定を tako-core の設定型へ解決する（Issue #112）
    pub fn pane_log_config(&self) -> tako_core::pane_log::PaneLogConfig {
        tako_core::pane_log::PaneLogConfig {
            enabled: self.pane_logs,
            max_bytes_per_pane: self.pane_log_max_mb.max(1) * 1024 * 1024,
            max_total_bytes: self.pane_log_total_max_mb.max(1) * 1024 * 1024,
        }
    }

    /// テーマモードを tako-core の型へ解決する（不明値は既定ダーク。Issue #217）
    pub fn theme_mode(&self) -> tako_core::theme::ThemeMode {
        tako_core::theme::ThemeMode::parse(&self.theme).unwrap_or_default()
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
            preview_live_reload: false,
            tmux_persist: false,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::On,
            sleep_guard_power: crate::sleep_guard::PowerCondition::Always,
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::WhileAgentsRunning,
            pane_logs: false,
            pane_log_max_mb: 10,
            pane_log_total_max_mb: 300,
            theme: "light".into(),
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
        assert!(parsed.preview_live_reload);
        assert!(parsed.tmux_persist);
        assert_eq!(
            parsed.sleep_guard_mode,
            crate::sleep_guard::SleepGuardMode::WhileAgentsRunning
        );
        assert_eq!(
            parsed.sleep_guard_power,
            crate::sleep_guard::PowerCondition::AcOnly
        );
        // ペインログ設定の既定（Issue #112。旧ファイル後方互換）
        assert!(parsed.pane_logs);
        assert_eq!(parsed.pane_log_max_mb, 5);
        assert_eq!(parsed.pane_log_total_max_mb, 200);
        let config = parsed.pane_log_config();
        assert!(config.enabled);
        assert_eq!(config.max_bytes_per_pane, 5 * 1024 * 1024);
        assert_eq!(config.max_total_bytes, 200 * 1024 * 1024);
        // テーマの既定はダーク（Issue #217。旧ファイル後方互換）
        assert_eq!(parsed.theme, "dark");
        assert_eq!(parsed.theme_mode(), tako_core::theme::ThemeMode::Dark);
    }

    #[test]
    fn theme_modeが不明値でダークへフォールバックする() {
        let light = Settings {
            theme: "light".into(),
            ..Settings::default()
        };
        assert_eq!(light.theme_mode(), tako_core::theme::ThemeMode::Light);
        let unknown = Settings {
            theme: "solarized".into(),
            ..Settings::default()
        };
        assert_eq!(unknown.theme_mode(), tako_core::theme::ThemeMode::Dark);
    }
}

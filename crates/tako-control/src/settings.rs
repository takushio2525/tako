//! settings — ユーザー設定の永続化（`<data_dir>/settings.json`）
//!
//! 項目追加時は `#[serde(default)]` で後方互換を保つ（未知キーは serde が無視する）。
//! 接続情報（トークン入り）は `discovery` 側で、こちらは秘密を含めない。

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// PDF・画像・動画サムネのデコード済み画像キャッシュ上限（Issue #258。MiB）
    #[serde(default = "default_preview_cache_max_mb")]
    pub preview_cache_max_mb: u64,
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
    /// 左サイドバー（ファイルツリー）の幅（px 整数。Issue #307。既定 244）
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    /// エラーレポートの自動送信（Issue #333。既定 OFF = opt-in）
    #[serde(default)]
    pub telemetry: bool,
    /// ステータスバーの利用制限表示で選択中のサービス（Issue #321。既定 "claude"）
    #[serde(default = "default_limit_service")]
    pub limit_service: String,
    /// UI 表示言語（Issue #435。"system" / "ja" / "en"。既定 system = OS ロケール追従）
    #[serde(default = "default_language")]
    pub language: String,
    /// 拡張子ごとの実行コマンド既定（FR-3.18, #453。キーは小文字拡張子・ドットなし。
    /// 値は変数展開が効くコマンドテンプレート。組み込み既定を上書き・追加する）
    #[serde(default)]
    pub runner_defaults: std::collections::BTreeMap<String, String>,
    /// ビルトイン dark/light への部分色上書き（Issue #459。キーは色名、値は "#RRGGBB"）
    #[serde(default)]
    pub theme_colors:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// 名前付きカスタムテーマプリセット（Issue #459）
    #[serde(default)]
    pub theme_presets: std::collections::BTreeMap<String, ThemePreset>,
    /// フォントファミリー（省略時はビルトイン既定 Menlo。Issue #459）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// フォントサイズ（省略時はビルトイン既定 13.0。Issue #459）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

/// テーマプリセット（Issue #459）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemePreset {
    pub base: String,
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, String>,
}

fn default_theme() -> String {
    "dark".into()
}

fn default_sidebar_width() -> u32 {
    244
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

fn default_preview_cache_max_mb() -> u64 {
    tako_core::PREVIEW_CACHE_DEFAULT_MB
}

fn default_limit_service() -> String {
    "claude".into()
}

fn default_language() -> String {
    "system".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_rename: true,
            port_detect: true,
            preview_live_reload: true,
            preview_cache_max_mb: default_preview_cache_max_mb(),
            tmux_persist: true,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::default(),
            sleep_guard_power: crate::sleep_guard::PowerCondition::default(),
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::default(),
            pane_logs: true,
            pane_log_max_mb: default_pane_log_max_mb(),
            pane_log_total_max_mb: default_pane_log_total_max_mb(),
            theme: default_theme(),
            sidebar_width: default_sidebar_width(),
            telemetry: false,
            limit_service: default_limit_service(),
            language: default_language(),
            runner_defaults: std::collections::BTreeMap::new(),
            theme_colors: std::collections::BTreeMap::new(),
            theme_presets: std::collections::BTreeMap::new(),
            font_family: None,
            font_size: None,
        }
    }
}

impl Settings {
    /// theme 値からテーマ実体を解決する（Issue #459）。
    /// 優先順: プリセット名 → ビルトイン名 + theme_colors 上書き → ダークフォールバック
    pub fn resolve_theme(&self) -> (tako_core::theme::Theme, Vec<String>) {
        use tako_core::theme::{Theme, ThemeMode};
        let (mut theme, warnings) = if let Some(preset) = self.theme_presets.get(&self.theme) {
            let mode = ThemeMode::parse(&preset.base).unwrap_or_default();
            let mut t = Theme::for_mode(mode);
            let w = t.apply_overrides(&preset.colors);
            (t, w)
        } else {
            let mode = ThemeMode::parse(&self.theme).unwrap_or_default();
            let mut t = Theme::for_mode(mode);
            let w = if let Some(overrides) = self.theme_colors.get(mode.as_str()) {
                t.apply_overrides(overrides)
            } else {
                Vec::new()
            };
            (t, w)
        };
        if let Some(ref family) = self.font_family {
            theme.font_family = family.clone();
        }
        if let Some(size) = self.font_size {
            let size = size.clamp(8.0, 32.0);
            theme.font_size = size;
            theme.line_height = size * (17.0 / 13.0);
        }
        (theme, warnings)
    }
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

    /// 利用制限表示の選択サービスを解決する（不明値は既定 Claude。Issue #321）
    pub fn limit_service(&self) -> tako_core::LimitService {
        tako_core::LimitService::parse(&self.limit_service).unwrap_or_default()
    }

    /// UI 表示言語の設定値を tako-core の型へ解決する（不明値は既定 system。Issue #435）
    pub fn lang_setting(&self) -> tako_core::i18n::LangSetting {
        tako_core::i18n::LangSetting::parse(&self.language).unwrap_or_default()
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
            preview_cache_max_mb: 768,
            tmux_persist: false,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::On,
            sleep_guard_power: crate::sleep_guard::PowerCondition::Always,
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::WhileAgentsRunning,
            pane_logs: false,
            pane_log_max_mb: 10,
            pane_log_total_max_mb: 300,
            theme: "light".into(),
            sidebar_width: 300,
            telemetry: true,
            limit_service: "codex".into(),
            language: "en".into(),
            runner_defaults: std::collections::BTreeMap::new(),
            theme_colors: std::collections::BTreeMap::new(),
            theme_presets: std::collections::BTreeMap::new(),
            font_family: None,
            font_size: None,
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
        assert_eq!(parsed.language, "system");
        assert_eq!(parsed.lang_setting(), tako_core::i18n::LangSetting::System);
        assert!(parsed.auto_rename);
        assert!(parsed.port_detect);
        assert!(parsed.preview_live_reload);
        assert_eq!(parsed.preview_cache_max_mb, 512);
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
        // サイドバー幅の既定（Issue #307。旧ファイル後方互換）
        assert_eq!(parsed.sidebar_width, 244);
        // テレメトリの既定は OFF（Issue #333。opt-in）
        assert!(!parsed.telemetry);
        // 利用制限サービスの既定は claude（Issue #321。旧ファイル後方互換）
        assert_eq!(parsed.limit_service, "claude");
        assert_eq!(parsed.limit_service(), tako_core::LimitService::Claude);
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

    #[test]
    fn 新フィールドの後方互換() {
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert!(parsed.theme_colors.is_empty());
        assert!(parsed.theme_presets.is_empty());
        assert!(parsed.font_family.is_none());
        assert!(parsed.font_size.is_none());
    }

    #[test]
    fn resolve_themeはビルトインを返す() {
        let s = Settings::default();
        let (theme, warnings) = s.resolve_theme();
        assert!(warnings.is_empty());
        assert_eq!(theme.mode, tako_core::theme::ThemeMode::Dark);
        assert_eq!(theme.accent, tako_core::theme::Rgb::from_hex(0x89b4fa));
    }

    #[test]
    fn resolve_themeは色上書きを適用する() {
        let mut s = Settings::default();
        let mut dark = std::collections::BTreeMap::new();
        dark.insert("accent".into(), "#ff0000".into());
        s.theme_colors.insert("dark".into(), dark);
        let (theme, warnings) = s.resolve_theme();
        assert!(warnings.is_empty());
        assert_eq!(theme.accent, tako_core::theme::Rgb::new(255, 0, 0));
    }

    #[test]
    fn resolve_themeはプリセットを解決する() {
        let mut s = Settings::default();
        let mut colors = std::collections::BTreeMap::new();
        colors.insert("accent".into(), "#00ced1".into());
        s.theme_presets.insert(
            "ocean".into(),
            ThemePreset {
                base: "dark".into(),
                colors,
            },
        );
        s.theme = "ocean".into();
        let (theme, warnings) = s.resolve_theme();
        assert!(warnings.is_empty());
        assert_eq!(theme.accent, tako_core::theme::Rgb::new(0x00, 0xce, 0xd1));
        assert_eq!(theme.mode, tako_core::theme::ThemeMode::Dark);
    }

    #[test]
    fn resolve_themeはフォント設定を適用する() {
        let s = Settings {
            font_family: Some("Monaco".into()),
            font_size: Some(16.0),
            ..Settings::default()
        };
        let (theme, _) = s.resolve_theme();
        assert_eq!(theme.font_family, "Monaco");
        assert!((theme.font_size - 16.0).abs() < f32::EPSILON);
        assert!((theme.line_height - 16.0 * 17.0 / 13.0).abs() < 0.01);
    }
}

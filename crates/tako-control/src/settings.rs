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
    /// ペインの `ssh` を検知してリモートフォルダをツリーへ自動追加する
    /// （FR-3.24.3 / Issue #976。既定 ON）。
    /// OFF にしても明示的な「リモートからフォルダを開く」経路はそのまま使える
    #[serde(default = "default_true")]
    pub ssh_auto_folders: bool,
    /// tako 内 zsh の入力予測（FR-2.4.5 / Issue #600。既定 ON）。
    /// zsh 以外のシェル・ユーザーが自前で導入済みの環境では設定に関わらず無害に素通しする
    #[serde(default = "default_true")]
    pub autosuggest: bool,
    /// 入力予測の確定キーのヒント表示（Issue #614。既定 ON）。
    /// 残り回数そのものは `<data_dir>/shell-integration/autosuggest-hint` にあり、
    /// ここが持つのは「恒久 OFF にしたか」だけ（ON へ戻すと残り回数が既定に戻る）
    #[serde(default = "default_true")]
    pub autosuggest_hint: bool,
    /// ゴースト表示中の Tab を確定にするか（Issue #614。既定 ON）。
    /// OFF なら Tab は常に従来の補完へ委譲される
    #[serde(default = "default_true")]
    pub autosuggest_tab: bool,
    /// 表示中プレビューファイルのライブリロード（Issue #233。既定 ON）
    #[serde(default = "default_true")]
    pub preview_live_reload: bool,
    /// 保存時整形（FR-3.33 / Issue #1683。**既定 OFF** = ユーザー確定）。
    ///
    /// ON でも整形するのは**明示的な保存**（⌘S / `tako edit save` / MCP の save）だけで、
    /// 自動保存（500ms のデバウンス）では整形しない（打鍵の 500ms 後に勝手に整形されない）。
    /// **旧ファイルはキーが無くても false で読める**（`#[serde(default)]`）ので移行 Step は
    /// 要らない（`Settings` の指紋は動くが、読み書きは後方互換）
    #[serde(default)]
    pub lsp_format_on_save: bool,
    /// tako mod（FR-2.42 / Issue #1879。既定 ON）: 新しく作るペインへ Claude Code の mod を
    /// 読ませる（env `CLAUDE_CODE_PLUGIN_DIRS`）。OFF にしても動いている claude には効かない。
    /// **旧ファイルはキーが無くても true で読める**（`default_true`）ので移行 Step は要らない
    /// （`Settings` の指紋は動くが、読み書きは後方互換）
    #[serde(default = "default_true")]
    pub claude_mod: bool,
    /// リミット後の自動復帰の全体の既定（FR-2.27.16 / Issue #1945。**既定 OFF** = #813 の
    /// ペイン単位オプトインを崩さない）。ステータスバーのボタン / `tako limit-resume on --all` /
    /// MCP `tako_limit_resume`（`all` + `enabled`）が書き、以後にエージェントになったペインが
    /// 最初に採る値になる。**旧ファイルはキーが無くても false で読める**（`#[serde(default)]`）
    /// ので移行 Step は要らない（`Settings` の指紋は動くが、読み書きは後方互換）
    #[serde(default)]
    pub limit_resume_all: bool,
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
    /// 蓋閉じ継続の電源条件（Issue #1473。既定 ac-only = 従来どおり AC 接続時のみ）。
    ///
    /// アイドルスリープ側の `sleep_guard_power` とは**別の軸**（蓋を閉じて持ち歩く
    /// リスクはアイドルスリープの防止とは釣り合わないので、まとめて切り替えない）。
    /// **旧ファイルはキーが無くても `PowerCondition::default()` = ac-only で読める**
    /// ので移行 Step は要らない（`Settings` の指紋は動くが、読み書きは後方互換）
    #[serde(default)]
    pub lid_sleep_power: crate::sleep_guard::PowerCondition,
    /// 蓋閉じ継続をバッテリーで続けるときの残量下限（%。Issue #1473。既定 20）。
    /// 旧ファイルにキーが無ければ既定 20 が立つ（同上）
    #[serde(default = "default_lid_battery_floor")]
    pub lid_battery_floor: u8,
    /// ペインの平文ログ保存（Issue #112 B。既定 ON）
    #[serde(default = "default_true")]
    pub pane_logs: bool,
    /// ペインあたりのログ上限（MB。超過でローテーション）
    #[serde(default = "default_pane_log_max_mb")]
    pub pane_log_max_mb: u64,
    /// ログディレクトリ全体の上限（MB。超過で古いファイルから削除）
    #[serde(default = "default_pane_log_total_max_mb")]
    pub pane_log_total_max_mb: u64,
    /// 直接ペインのスクロールバック保持上限（行。Issue #818。既定 10,000）。
    /// 飽和すると `行 × 桁 × 24 B` を保持するので、軽量運用では下げられる。
    /// tmux バックエンドペイン（persist ON）は alt screen のため元々ほぼ 0
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
    /// フォーカスの無いペインの出力による再描画の上限（fps。Issue #1979。既定 30）。
    /// フォーカス中のペインは常に 60 fps。**項目が無い旧ファイルは既定で読める**
    /// （serde の default。移行は要らない）
    #[serde(default = "default_unfocused_redraw_fps")]
    pub unfocused_redraw_fps: u32,
    /// UI テーマ（Issue #217。"dark" / "light"。既定 dark）
    #[serde(default = "default_theme")]
    pub theme: String,
    /// UI 表示モード（Issue #691 / #694。"terminal"（既定）/ "gui"）。
    /// 既定が terminal なので、この項目を知らない既存ユーザーの体験は変わらない
    #[serde(default = "default_ui_mode")]
    pub ui_mode: String,
    /// 左サイドバー（ファイルツリー）の幅（px 整数。Issue #307。既定 244）
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    /// ファイルツリーでドット始まりの項目を表示する（Issue #550。既定 false = 非表示）
    #[serde(default)]
    pub show_hidden_files: bool,
    /// エラーレポートの自動送信（Issue #333。既定 OFF = opt-in）
    #[serde(default)]
    pub telemetry: bool,
    /// 初回起動のウェルカムバナーを閉じた（Issue #549。既定 false）。
    /// 「初回かどうか」の主判定は settings.json の実在（`welcome::should_show`）で、
    /// これはユーザーが明示的に閉じた記録。CLI / MCP `tako welcome` から操作できる
    #[serde(default)]
    pub welcome_dismissed: bool,
    /// 更新通知カードを閉じたときの対象バージョン（Issue #616。None = 閉じていない）。
    /// 値は「どのバージョンを案内していたか」を表すキー（例 `"stable:0.6.1 test:0.7.0-test.1"`）。
    /// 新しいバージョンを検知するとキーが変わり、カードは再び出る（= バージョン単位の抑止）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_card_dismissed: Option<String>,
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

fn default_ui_mode() -> String {
    tako_core::ui_mode::UiMode::default().as_str().into()
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

fn default_scrollback_lines() -> usize {
    tako_core::scrollback::DEFAULT_LINES
}

fn default_unfocused_redraw_fps() -> u32 {
    tako_core::redraw_limit::DEFAULT_UNFOCUSED_FPS
}

fn default_lid_battery_floor() -> u8 {
    crate::sleep_guard::DEFAULT_LID_BATTERY_FLOOR
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
            ssh_auto_folders: true,
            autosuggest: true,
            autosuggest_hint: true,
            autosuggest_tab: true,
            preview_live_reload: true,
            lsp_format_on_save: false,
            claude_mod: true,
            limit_resume_all: false,
            preview_cache_max_mb: default_preview_cache_max_mb(),
            tmux_persist: true,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::default(),
            sleep_guard_power: crate::sleep_guard::PowerCondition::default(),
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::default(),
            lid_sleep_power: crate::sleep_guard::PowerCondition::default(),
            lid_battery_floor: default_lid_battery_floor(),
            pane_logs: true,
            pane_log_max_mb: default_pane_log_max_mb(),
            pane_log_total_max_mb: default_pane_log_total_max_mb(),
            scrollback_lines: default_scrollback_lines(),
            unfocused_redraw_fps: default_unfocused_redraw_fps(),
            theme: default_theme(),
            ui_mode: default_ui_mode(),
            sidebar_width: default_sidebar_width(),
            show_hidden_files: false,
            telemetry: false,
            welcome_dismissed: false,
            update_card_dismissed: None,
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
    /// スリープ防止の設定一式（#1473）。
    ///
    /// `update` / `status` の呼び出し側（GUI の 2 秒 tick / dispatch / CLI）が
    /// **ここ 1 か所から作る**ので、設定を足しても渡し忘れる経路が生まれない
    pub fn sleep_guard_config(&self) -> crate::sleep_guard::SleepGuardConfig {
        crate::sleep_guard::SleepGuardConfig {
            mode: self.sleep_guard_mode,
            power_condition: self.sleep_guard_power,
            lid_sleep_mode: self.lid_sleep_mode,
            lid_power_condition: self.lid_sleep_power,
            lid_battery_floor: self.lid_battery_floor,
        }
    }

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

    /// スクロールバック保持上限を解決する（Issue #818）。
    /// **手書きの範囲外値は丸める**（`pane_log_config` と同じ方針。
    /// 0 を書かれて履歴ゼロのペインが立つより、下限で起動するほうが直せる）
    pub fn resolved_scrollback_lines(&self) -> usize {
        tako_core::scrollback::clamp_lines(self.scrollback_lines)
    }

    /// フォーカスの無いペインの再描画の上限を解決する（Issue #1979）。
    /// 手書きの 0 は既定へ、上限超えは上限へ丸める（起動は必ず成立させる）
    pub fn resolved_unfocused_redraw_fps(&self) -> u32 {
        tako_core::redraw_limit::clamp_fps(self.unfocused_redraw_fps)
    }

    /// テーマモードを tako-core の型へ解決する（不明値は既定ダーク。Issue #217）
    pub fn theme_mode(&self) -> tako_core::theme::ThemeMode {
        tako_core::theme::ThemeMode::parse(&self.theme).unwrap_or_default()
    }

    /// UI 表示モードを tako-core の型へ解決する（不明値は既定 terminal。Issue #694）
    pub fn ui_mode(&self) -> tako_core::ui_mode::UiMode {
        tako_core::ui_mode::UiMode::parse(&self.ui_mode).unwrap_or_default()
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

/// 指定パスから設定を読む（不在・破損は None）。
///
/// **破損していたら既定値へ落とす前に退避する**（#916）。settings.json は
/// theme_colors / theme_presets / runner_defaults のようにユーザーが手で書く
/// 情報を持つのに、旧実装は破損を黙って既定値扱いし、直後の [`save`] が
/// 元の内容を上書きして復元不能にしていた
pub fn load_from(path: &std::path::Path) -> Option<Settings> {
    load_and_report(path).0
}

/// [`load_from`] の本体。破損を persist.log へ申告したら、その 1 行も返す
/// （「実行中に何度壊れても毎回記録される」をテストが配線ごと確かめる口。#1819）
fn load_and_report(path: &std::path::Path) -> (Option<Settings>, Option<String>) {
    let Ok(json) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    match serde_json::from_str(&json) {
        Ok(settings) => {
            UNREADABLE_SEEN.forget(path);
            (Some(settings), None)
        }
        Err(e) => {
            let quarantine =
                tako_core::migration::quarantine_unreadable(path, &tako_core::migration::FsIo);
            if !UNREADABLE_SEEN.first_sighting(path, &json) {
                return (None, None);
            }
            let line = unreadable_line(path, &e.to_string(), quarantine.as_ref());
            crate::diag::persist_log(&line);
            (None, Some(line))
        }
    }
}

/// 破損の申告の重複除け（#1819）。**壊れた中身ごとに 1 回**申告する。
///
/// `load` は多くの経路から呼ばれるので、読むたびに申告するとログが溢れる。以前は
/// 「1 プロセス 1 回」で抑えていたので、実行中にもう一度（別の中身で）壊れても
/// 記録が残らなかった（実測）。いまは「そのファイルで直前に申告した中身と違う」ときに
/// 申告し、読めた回に忘れる（壊れる → 直る → 同じ中身でまた壊れる、も 2 回と数える）
pub(crate) struct UnreadableSeen(std::sync::Mutex<Vec<(PathBuf, String)>>);

static UNREADABLE_SEEN: UnreadableSeen = UnreadableSeen::new();

impl UnreadableSeen {
    pub(crate) const fn new() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }

    /// そのファイルがこの中身で壊れているのを見たのが初めてなら true
    /// （以後は同じ中身のあいだ false）
    pub(crate) fn first_sighting(&self, path: &std::path::Path, body: &str) -> bool {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match seen.iter_mut().find(|(p, _)| p == path) {
            Some((_, last)) if last == body => false,
            Some((_, last)) => {
                *last = body.to_string();
                true
            }
            None => {
                seen.push((path.to_path_buf(), body.to_string()));
                true
            }
        }
    }

    /// 読めた = 直った。次に壊れたら中身が前と同じでも申告する
    pub(crate) fn forget(&self, path: &std::path::Path) {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|(p, _)| p != path);
    }
}

/// 破損の申告の 1 行（退避先と、押し出しが起きたならその旨も載せる）
fn unreadable_line(
    path: &std::path::Path,
    reason: &str,
    quarantine: Option<&tako_core::migration::Quarantine>,
) -> String {
    let where_to = match quarantine {
        Some(q) if q.evicted => format!(
            "{}・{}",
            q.path.display(),
            tako_core::migration::eviction_note()
        ),
        Some(q) => q.path.display().to_string(),
        None => "（退避できず）".to_string(),
    };
    format!(
        "settings.json を解釈できないので既定値で動く: {}（{reason}・退避 {where_to}）",
        path.display()
    )
}

/// 読めない色の上書きを無視した行の頭（起動時・読み直しで共通。#1756 / #1763）
pub const THEME_WARNING_IGNORED: &str = "テーマの色上書きを無視: ";

/// 前回無視した上書きが、読み直した設定では警告にならなくなった行の頭（#1763）。
/// 手で直したときのほか、テーマを切り替えてその上書きを読まなくなったときも出る
pub const THEME_WARNING_RESOLVED: &str =
    "テーマの色上書きの警告が解消（読み直した設定では出ない）: ";

/// テーマを settings.json から解決し、読めない色の警告を persist.log へ出す行を決める帳簿（#1763）。
///
/// **起動時と実行中の読み直し（`ControlHost::reload_theme` / タブバーのトグル）が
/// [`ThemeWarningLog::reload`] の 1 本を通る**。以前は起動時だけが警告を残し、読み直しは
/// 捨てていたので、実行中に settings.json を手で直した値がなぜ効かないのかを追えなかった。
///
/// 読み直しは色を 1 つ変えるたび・テーマを切り替えるたびに走るので、警告をそのまま出すと
/// 同じ行が積もる。そこで**前回と同じ警告は出さず**、変わったぶんだけを出す:
/// 消えた警告は [`THEME_WARNING_RESOLVED`]、増えた警告は起動時と同じ
/// [`THEME_WARNING_IGNORED`] の形式で出す（同じ色の値が別の読めない値へ変わったときは両方が出る）
#[derive(Debug, Default)]
pub struct ThemeWarningLog {
    /// 前回までに persist.log へ反映した警告（起動前は空）
    last: Vec<String>,
}

impl ThemeWarningLog {
    /// settings.json を読んでテーマを解決し、persist.log へ出す行と一緒に返す
    pub fn reload(&mut self) -> (tako_core::theme::Theme, Vec<String>) {
        let (theme, warnings) = load().resolve_theme();
        let lines = self.record(warnings);
        (theme, lines)
    }

    /// 今回の警告から persist.log へ出す行を決め、帳簿を今回の状態へ進める
    pub fn record(&mut self, warnings: Vec<String>) -> Vec<String> {
        let resolved = self
            .last
            .iter()
            .filter(|w| !warnings.contains(w))
            .map(|w| format!("{THEME_WARNING_RESOLVED}{w}"));
        let ignored = warnings
            .iter()
            .filter(|w| !self.last.contains(w))
            .map(|w| format!("{THEME_WARNING_IGNORED}{w}"));
        let lines = resolved.chain(ignored).collect();
        self.last = warnings;
        lines
    }

    /// いま適用しているテーマで無視している色の上書き（`<キー>: <理由>`。#1820）。
    ///
    /// 最後の [`Self::reload`] の警告そのもので、persist.log へ
    /// [`THEME_WARNING_IGNORED`] を付けて出した行の本体と同じ文字列。
    /// `tako theme` / MCP `tako_theme` の応答の `warnings` はここから組む
    pub fn current(&self) -> &[String] {
        &self.last
    }
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

/// 指定パスへ設定を書き出す（`save` の本体。隔離した検証で使えるよう公開）
pub fn save_to(path: &std::path::Path, settings: &Settings) -> io::Result<()> {
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

    /// #916: 壊れた settings.json は既定値へ落ちる前に退避され、
    /// 直後の保存で消えない
    #[test]
    fn 壊れた設定は退避されてから既定値になる() {
        let path = temp_path("broken");
        let dir = path.parent().expect("親がある").to_path_buf();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        std::fs::write(&path, "{ \"theme\": ").expect("書ける");
        assert!(load_from(&path).is_none(), "壊れているので None");
        let quarantine = tako_core::migration::quarantine_path(&path);
        assert_eq!(
            std::fs::read_to_string(&quarantine).expect("退避が読める"),
            "{ \"theme\": ",
            "元の内容が残る"
        );
        // 既定値で保存し直しても退避は残る（= 復元できる）
        save_to(&path, &Settings::default()).expect("保存できる");
        assert!(quarantine.is_file());
        assert!(load_from(&path).is_some(), "保存後は読める");
        // #1819: 実行中にもう一度、別の中身で壊れても、その中身も保存の前に残る
        // （以前は 1 本目が在ると何も写さず、直後の保存が 2 回目の中身を消していた）
        std::fs::write(&path, "{ \"theme\": \"light\", ").expect("書ける");
        assert!(load_from(&path).is_none(), "壊れているので None");
        save_to(&path, &Settings::default()).expect("保存できる");
        assert_eq!(
            std::fs::read_to_string(tako_core::migration::quarantine_slot_path(&path, 2))
                .expect("2 本目の退避が読める"),
            "{ \"theme\": \"light\", ",
            "2 回目に壊れた中身も残る"
        );
        assert_eq!(
            std::fs::read_to_string(&quarantine).expect("退避が読める"),
            "{ \"theme\": ",
            "1 本目は塗り潰さない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #1819: 実行中に何度壊れても、**壊れた回ごとに**退避と申告が両方残る
    /// （`load_from` の配線ごと固定する。以前は 1 プロセス 1 回しか申告せず、
    /// 2 回目の中身は退避されずに直後の保存で消えていた）
    #[test]
    fn 実行中に何度壊れても退避と申告が毎回残る() {
        let path = temp_path("second-break");
        let dir = path.parent().expect("親がある").to_path_buf();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        let bodies = [
            "{ \"theme\": \"1\"",
            "{ \"theme\": \"2\"",
            "{ \"theme\": \"3\"",
        ];
        for (i, body) in bodies.iter().enumerate() {
            std::fs::write(&path, body).expect("書ける");
            let (loaded, line) = load_and_report(&path);
            assert!(loaded.is_none(), "{} 回目: 壊れているので None", i + 1);
            let dest = tako_core::migration::quarantine_slot_path(&path, i + 1);
            let line = line.unwrap_or_else(|| panic!("{} 回目も申告する", i + 1));
            assert!(
                line.contains(&dest.display().to_string()),
                "{} 回目の申告は今回の退避先を名指す: {line}",
                i + 1
            );
            // 同じ中身を読み直しただけでは申告しない（ログを溢れさせない）
            assert_eq!(load_and_report(&path).1, None, "{} 回目の読み直し", i + 1);
            // 呼び出し側の保存（load → 既定値 → save）が上書きしても消えない
            save_to(&path, &Settings::default()).expect("保存できる");
            assert_eq!(
                std::fs::read_to_string(&dest).expect("退避が読める"),
                *body,
                "{} 回目に壊れた中身が残る",
                i + 1
            );
        }
        // 読めたら忘れる = 直ったあと同じ中身でまた壊れたら、それも申告する
        assert!(load_from(&path).is_some());
        std::fs::write(&path, bodies[2]).expect("書ける");
        assert!(
            load_and_report(&path).1.is_some(),
            "直ってから壊れた回も申告する"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #1819: 破損の申告は「壊れた中身ごとに 1 回」。以前は 1 プロセス 1 回だったので、
    /// 実行中に 2 回目に壊れた事実が persist.log に残らなかった
    #[test]
    fn 破損の申告は壊れた中身ごとに1回() {
        let seen = UnreadableSeen::new();
        let path = std::path::Path::new("/tmp/tako-1819/settings.json");
        assert!(seen.first_sighting(path, "A"), "1 回目は申告する");
        assert!(
            !seen.first_sighting(path, "A"),
            "同じ中身を読み直しただけなら申告しない（load は多くの経路から呼ばれる）"
        );
        assert!(
            seen.first_sighting(path, "B"),
            "別の中身で壊れたら 2 回目も申告する"
        );
        seen.forget(path);
        assert!(
            seen.first_sighting(path, "B"),
            "直ってから同じ中身でまた壊れたら、それも申告する"
        );
        seen.forget(std::path::Path::new("/tmp/tako-1819/other.json"));
        assert!(
            !seen.first_sighting(path, "B"),
            "別のファイルが読めても忘れない"
        );
    }

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
            ssh_auto_folders: false,
            autosuggest: false,
            autosuggest_hint: false,
            autosuggest_tab: false,
            preview_live_reload: false,
            lsp_format_on_save: true,
            claude_mod: false,
            limit_resume_all: true,
            preview_cache_max_mb: 768,
            tmux_persist: false,
            sleep_guard_mode: crate::sleep_guard::SleepGuardMode::On,
            sleep_guard_power: crate::sleep_guard::PowerCondition::Always,
            lid_sleep_mode: crate::sleep_guard::LidSleepMode::WhileAgentsRunning,
            lid_sleep_power: crate::sleep_guard::PowerCondition::Always,
            lid_battery_floor: 35,
            pane_logs: false,
            pane_log_max_mb: 10,
            pane_log_total_max_mb: 300,
            scrollback_lines: 2_000,
            unfocused_redraw_fps: 12,
            theme: "light".into(),
            ui_mode: "gui".into(),
            sidebar_width: 300,
            show_hidden_files: true,
            telemetry: true,
            welcome_dismissed: true,
            update_card_dismissed: Some("stable:9.9.9".into()),
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
        // #818: 旧ファイル（キー無し）はそのまま読めて既定 10,000 が使われる
        assert_eq!(
            parsed.scrollback_lines,
            tako_core::scrollback::DEFAULT_LINES
        );
        assert_eq!(
            parsed.resolved_scrollback_lines(),
            tako_core::scrollback::DEFAULT_LINES
        );
        // #1979: 旧ファイル（キー無し）はそのまま読めて既定 30 fps（移行 Step 不要の根拠）
        assert_eq!(
            parsed.resolved_unfocused_redraw_fps(),
            tako_core::redraw_limit::DEFAULT_UNFOCUSED_FPS
        );
        // 手書きの 0 / 桁違いでも起動は成立する（0 = 書いていない扱い・上限超えは上限）
        let zero: Settings = serde_json::from_str(r#"{"unfocused_redraw_fps":0}"#).unwrap();
        assert_eq!(
            zero.resolved_unfocused_redraw_fps(),
            tako_core::redraw_limit::DEFAULT_UNFOCUSED_FPS
        );
        let huge: Settings = serde_json::from_str(r#"{"unfocused_redraw_fps":100000}"#).unwrap();
        assert_eq!(
            huge.resolved_unfocused_redraw_fps(),
            tako_core::redraw_limit::MAX_UNFOCUSED_FPS
        );
        assert_eq!(parsed.lang_setting(), tako_core::i18n::LangSetting::System);
        assert!(parsed.auto_rename);
        assert!(parsed.port_detect);
        // #976: SSH の自動フォルダ追加は既定 ON（旧ファイルにキーが無くても立つ =
        // 移行 Step 不要。指紋テストへ書く「serde default で読める」の根拠がここ）
        assert!(parsed.ssh_auto_folders);
        // #600: 入力予測は既定 ON（旧ファイル後方互換）
        assert!(parsed.autosuggest);
        // #614: 確定キーのヒントと Tab 確定も既定 ON（#600 時代のファイルでも立つ）
        assert!(parsed.autosuggest_hint);
        assert!(parsed.autosuggest_tab);
        // #550: 隠しファイルは既定で非表示（未知キーの後方互換も兼ねる）
        assert!(!parsed.show_hidden_files);
        assert!(parsed.preview_live_reload);
        // #1683: 保存時整形は既定 OFF（ユーザー確定。旧ファイル = キー無しでも false =
        // 移行 Step 不要。指紋テストの更新理由がこれ）
        assert!(!parsed.lsp_format_on_save);
        assert!(!Settings::default().lsp_format_on_save);
        // #1945: 自動復帰の全体の既定は OFF（旧ファイル = キー無しでも false =
        // 移行 Step 不要。指紋テストの更新理由がこれ）
        assert!(!parsed.limit_resume_all);
        assert!(!Settings::default().limit_resume_all);
        // #1879: tako mod は既定 ON（旧ファイル = キー無しでも true = 移行 Step 不要。
        // 指紋テストの更新理由がこれ）
        assert!(parsed.claude_mod);
        assert!(Settings::default().claude_mod);
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
        // #1473: 蓋閉じ継続の電源条件と残量下限は、旧ファイル（キー無し）でも
        // 既定（ac-only / 20%）が立つ = 移行 Step 不要。指紋テストの更新理由がこれ
        assert_eq!(
            parsed.lid_sleep_power,
            crate::sleep_guard::PowerCondition::AcOnly
        );
        assert_eq!(
            parsed.lid_battery_floor,
            crate::sleep_guard::DEFAULT_LID_BATTERY_FLOOR
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
        // ウェルカムバナーの dismiss 記録の既定（Issue #549。旧ファイル後方互換）
        assert!(!parsed.welcome_dismissed);
        // 利用制限サービスの既定は claude（Issue #321。旧ファイル後方互換）
        assert_eq!(parsed.limit_service, "claude");
        assert_eq!(parsed.limit_service(), tako_core::LimitService::Claude);
        // UI 表示モードの既定は terminal（Issue #694。旧ファイル後方互換 =
        // この項目を持たない既存ユーザーの表示は従来どおり）
        assert_eq!(parsed.ui_mode, "terminal");
        assert_eq!(parsed.ui_mode(), tako_core::ui_mode::UiMode::Terminal);
    }

    #[test]
    fn ui_modeが不明値でterminalへフォールバックする() {
        let gui = Settings {
            ui_mode: "gui".into(),
            ..Settings::default()
        };
        assert_eq!(gui.ui_mode(), tako_core::ui_mode::UiMode::Gui);
        let unknown = Settings {
            ui_mode: "simple".into(),
            ..Settings::default()
        };
        assert_eq!(unknown.ui_mode(), tako_core::ui_mode::UiMode::Terminal);
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

    /// #1756: 読めない色が保存済みの settings.json も、そのまま読めて起動できる
    /// （その色だけ既定に残して警告で返す。以前は非 ASCII の値で起動のたびに落ちた）。
    /// 読み方は変えていないので、移行は要らない（#916）
    #[test]
    fn issue1756_読めない色が保存済みでもテーマを解決できる() {
        let json = r##"{
            "theme": "dark",
            "theme_colors": { "dark": { "accent": "#赤色", "green": "#00ff00" } },
            "theme_presets": { "ocean": { "base": "dark", "colors": { "accent": "#１２３４５６" } } }
        }"##;
        let mut s: Settings = serde_json::from_str(json).expect("旧ファイルがそのまま読める");
        let base = tako_core::theme::Theme::default_dark();
        let (theme, warnings) = s.resolve_theme();
        assert_eq!(theme.accent, base.accent, "読めない値は既定のまま");
        assert_eq!(theme.green, tako_core::theme::Rgb::new(0, 255, 0));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("accent: "), "{warnings:?}");
        // プリセット側に残った値も同じ
        s.theme = "ocean".into();
        let (theme, warnings) = s.resolve_theme();
        assert_eq!(theme.accent, base.accent);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// dark の色上書きだけを持つ設定（#1763 のテスト用）
    fn dark_overrides(pairs: &[(&str, &str)]) -> Settings {
        let mut s = Settings::default();
        s.theme_colors.insert(
            "dark".into(),
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        s
    }

    /// 帳簿へ 1 回の読み直しぶんの警告を渡し、出す行を返す
    fn reload_lines(log: &mut ThemeWarningLog, s: &Settings) -> Vec<String> {
        log.record(s.resolve_theme().1)
    }

    /// #1763: 行の頭は #1756 の起動時の記録と同じ文面のまま（persist.log を
    /// 「テーマの色上書き」で引く人・スクリプトの当てを変えない）
    #[test]
    fn issue1763_無視の行は起動時の記録と同じ文面() {
        assert_eq!(THEME_WARNING_IGNORED, "テーマの色上書きを無視: ");
        assert!(THEME_WARNING_RESOLVED.starts_with("テーマの色上書き"));
        let s = dark_overrides(&[("accent", "#赤色"), ("green", "#00ff00")]);
        let (_, warnings) = s.resolve_theme();
        let mut log = ThemeWarningLog::default();
        assert_eq!(
            reload_lines(&mut log, &s),
            vec![format!("テーマの色上書きを無視: {}", warnings[0])],
            "起動時（帳簿が空）の行は #1756 と同じ `無視: <キー>: <理由>`"
        );
    }

    /// #1820: `current`（`tako theme` の応答の `warnings`）は、persist.log へ「無視」として
    /// 出して「解消」でまだ打ち消していない行の本体と常に同じ
    #[test]
    fn issue1820_currentはpersist_logの未解消の行と同じ() {
        let replay = |lines: Vec<String>, live: &mut Vec<String>| {
            for line in lines {
                if let Some(w) = line.strip_prefix(THEME_WARNING_IGNORED) {
                    live.push(w.to_string());
                } else if let Some(w) = line.strip_prefix(THEME_WARNING_RESOLVED) {
                    live.retain(|x| x != w);
                }
            }
        };
        let mut log = ThemeWarningLog::default();
        let mut live: Vec<String> = Vec::new();
        assert!(log.current().is_empty(), "読み直す前は空");
        for colors in [
            &[("accent", "#赤色"), ("red", "#12345")][..],
            &[("accent", "#赤色"), ("red", "#12345"), ("blue", "#ｇ00000")],
            &[("accent", "#ff8800"), ("blue", "#ｇ00000")],
            &[("accent", "#ff8800")],
        ] {
            let s = dark_overrides(colors);
            replay(reload_lines(&mut log, &s), &mut live);
            let mut current = log.current().to_vec();
            current.sort();
            live.sort();
            assert_eq!(current, live, "{colors:?}");
            assert_eq!(log.current(), s.resolve_theme().1.as_slice(), "{colors:?}");
        }
        assert!(log.current().is_empty(), "直し終えたら空");
    }

    /// #1763: 同じ内容のまま何度読み直しても行を積まない
    #[test]
    fn issue1763_同じ警告が続く読み直しでは何も出さない() {
        let s = dark_overrides(&[("accent", "#赤色"), ("red", "#12345")]);
        let mut log = ThemeWarningLog::default();
        assert_eq!(reload_lines(&mut log, &s).len(), 2, "起動時は 2 件とも出す");
        for i in 0..100 {
            assert!(
                reload_lines(&mut log, &s).is_empty(),
                "{i} 回目の読み直しで同じ警告を積んだ"
            );
        }
        // 警告の無い設定も、何度読み直しても何も出さない
        let clean = dark_overrides(&[("accent", "#ff0000")]);
        let mut log = ThemeWarningLog::default();
        for _ in 0..3 {
            assert!(reload_lines(&mut log, &clean).is_empty());
        }
    }

    /// #1763: 実行中に足した読めない色は、起動時と同じ形式で増えたぶんだけ出す
    #[test]
    fn issue1763_増えた警告だけを起動時と同じ形式で出す() {
        let mut log = ThemeWarningLog::default();
        reload_lines(&mut log, &dark_overrides(&[("accent", "#赤色")]));
        let s = dark_overrides(&[("accent", "#赤色"), ("red", "#12345")]);
        let lines = reload_lines(&mut log, &s);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let red = s
            .resolve_theme()
            .1
            .into_iter()
            .find(|w| w.starts_with("red: "))
            .expect("red の警告");
        assert_eq!(lines[0], format!("{THEME_WARNING_IGNORED}{red}"));
    }

    /// #1763: 直した・テーマを切り替えた・別の読めない値へ変えた、のどれでも
    /// 前回の警告が消えたことを名指す（最後に出た行が今の状態を表す）
    #[test]
    fn issue1763_消えた警告は解消として名指す() {
        let bad = dark_overrides(&[("accent", "#赤色"), ("red", "#12345")]);
        let bad_warnings = bad.resolve_theme().1;
        // 直した
        let mut log = ThemeWarningLog::default();
        reload_lines(&mut log, &bad);
        let fixed = dark_overrides(&[("accent", "#ff8800"), ("red", "#ff0000")]);
        let lines = reload_lines(&mut log, &fixed);
        let want: Vec<String> = bad_warnings
            .iter()
            .map(|w| format!("{THEME_WARNING_RESOLVED}{w}"))
            .collect();
        assert_eq!(lines, want);
        assert!(reload_lines(&mut log, &fixed).is_empty(), "解消も積まない");

        // ライトへ切り替えた = dark の上書きを読まなくなった
        let mut log = ThemeWarningLog::default();
        reload_lines(&mut log, &bad);
        let mut light = bad.clone();
        light.theme = "light".into();
        assert_eq!(reload_lines(&mut log, &light), want);
        // ダークへ戻すと、また起動時と同じ形式で出る
        let back = reload_lines(&mut log, &bad);
        assert_eq!(back.len(), 2, "{back:?}");
        assert!(back.iter().all(|l| l.starts_with(THEME_WARNING_IGNORED)));

        // 同じ色を別の読めない値へ変えた: 解消が先・無視が後（最後の行が今の値）
        let mut log = ThemeWarningLog::default();
        reload_lines(&mut log, &dark_overrides(&[("accent", "#赤色")]));
        let lines = reload_lines(&mut log, &dark_overrides(&[("accent", "#abc")]));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(THEME_WARNING_RESOLVED) && lines[0].contains("#赤色"));
        assert!(lines[1].starts_with(THEME_WARNING_IGNORED) && lines[1].contains("#abc"));
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

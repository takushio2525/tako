//! 設定画面（Issue #459 / #486 / #488）— 独立 GPUI ウィンドウ
//!
//! 設計は `.agent/plans/2026-07-settings-ui.md`。設定の実体は settings.json /
//! config.yaml で、この画面はその読み書き UI にすぎない。個別設定の変更は
//! `tako_control::dispatch::dispatch()` を直接呼ぶので CLI / MCP と同一経路を通る
//! （開発不変条件の 1:1 が構造的に成立する）。
//!
//! #486 の総点検で見つかった構造的な欠陥と対処:
//! ① 変更後にスナップショットを再読込せず、表示が実値とずれる → dispatch 後に必ず再読込
//! ② テキスト入力の基盤が無く hex / フォント / コマンドを編集できない → IME 対応の入力を新設
//! ③ セットアップ・リモートタブが render 毎に子プロセス・dispatch を叩く → キャッシュ + 更新ボタン

use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::*;
use tako_control::protocol::Request;
use tako_control::settings::{self, Settings};
use tako_core::theme::{Rgb, Theme};

use crate::file_icons::ui_icon;
use crate::ui_text::settings as txt;
use crate::TakoApp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    Runner,
    Setup,
    Sleep,
    Remote,
    Advanced,
}

impl SettingsTab {
    pub const ALL: &[SettingsTab] = &[
        SettingsTab::General,
        SettingsTab::Appearance,
        SettingsTab::Runner,
        SettingsTab::Setup,
        SettingsTab::Sleep,
        SettingsTab::Remote,
        SettingsTab::Advanced,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SettingsTab::General => txt::tab_general(),
            SettingsTab::Appearance => txt::tab_appearance(),
            SettingsTab::Runner => txt::tab_runner(),
            SettingsTab::Setup => txt::tab_setup(),
            SettingsTab::Sleep => txt::tab_sleep(),
            SettingsTab::Remote => txt::tab_remote(),
            SettingsTab::Advanced => txt::tab_advanced(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "general" => Some(SettingsTab::General),
            "appearance" => Some(SettingsTab::Appearance),
            "runner" => Some(SettingsTab::Runner),
            "setup" => Some(SettingsTab::Setup),
            "sleep" => Some(SettingsTab::Sleep),
            "remote" => Some(SettingsTab::Remote),
            "advanced" => Some(SettingsTab::Advanced),
            _ => None,
        }
    }
}

/// 編集中のテキストフィールド。1 度に 1 つだけ編集する
#[derive(Debug, Clone, PartialEq, Eq)]
enum EditField {
    /// 色の hex 値（色キー）
    ColorHex(String),
    FontFamily,
    FontSize,
    /// テーマプリセット名（保存用）
    PresetName,
    RunnerNewExt,
    RunnerNewCmd,
    /// 既存拡張子のコマンド編集（拡張子）
    RunnerCmd(String),
    PreviewCacheMb,
    PaneLogMaxMb,
    PaneLogTotalMaxMb,
    /// settings.json 直接編集（複数行）
    AdvancedJson,
}

impl EditField {
    fn multiline(&self) -> bool {
        matches!(self, EditField::AdvancedJson)
    }

    /// 要素 ID 用の安定した文字列
    fn slug(&self) -> String {
        match self {
            EditField::ColorHex(k) => format!("color-{k}"),
            EditField::FontFamily => "font-family".into(),
            EditField::FontSize => "font-size".into(),
            EditField::PresetName => "preset-name".into(),
            EditField::RunnerNewExt => "runner-new-ext".into(),
            EditField::RunnerNewCmd => "runner-new-cmd".into(),
            EditField::RunnerCmd(ext) => format!("runner-cmd-{ext}"),
            EditField::PreviewCacheMb => "preview-cache".into(),
            EditField::PaneLogMaxMb => "pane-log-max".into(),
            EditField::PaneLogTotalMaxMb => "pane-log-total".into(),
            EditField::AdvancedJson => "advanced-json".into(),
        }
    }
}

/// 編集中バッファ（cursor はバイト位置）
struct TextEdit {
    field: EditField,
    text: String,
    cursor: usize,
}

/// IME の未確定文字列
struct ImeState {
    text: String,
    selected_utf16: Option<Range<usize>>,
}

/// タブ表示に必要な read-only 状態のキャッシュ。
/// render 毎に子プロセス・dispatch を叩かないよう、タブ切替と更新ボタンでのみ取得する
#[derive(Default)]
struct StatusCache {
    persist: Option<serde_json::Value>,
    fda: Option<serde_json::Value>,
    rules: Option<serde_json::Value>,
    changes: Option<serde_json::Value>,
    remote: Option<serde_json::Value>,
    /// エージェント CLI の検出結果（名前, 導入済み）。background で取得する
    agents: Option<Vec<(String, bool)>>,
}

pub struct SettingsWindow {
    tako_app: WeakEntity<TakoApp>,
    tab: SettingsTab,
    settings: Settings,
    focus: FocusHandle,
    expanded_categories: Vec<bool>,
    edit: Option<TextEdit>,
    ime: Option<ImeState>,
    /// 編集中フィールドの矩形（IME 候補ウィンドウの位置出し用）
    edit_bounds: Option<Bounds<Pixels>>,
    /// 操作結果のメッセージ（本文, エラーか）
    message: Option<(String, bool)>,
    status: StatusCache,
    /// Code Runner 新規追加行の確定前バッファ
    runner_new_ext: String,
    runner_new_cmd: String,
}

impl SettingsWindow {
    pub fn new(
        tako_app: WeakEntity<TakoApp>,
        tab: Option<SettingsTab>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = settings::load();
        let category_count = Theme::COLOR_CATEGORIES.len();
        let mut expanded = vec![false; category_count];
        if let Some(idx) = Theme::COLOR_CATEGORIES
            .iter()
            .position(|(id, _, _)| *id == "accent")
        {
            expanded[idx] = true;
        }

        // 外部（CLI / MCP / master 会話）からの設定変更に追随する
        if let Some(app) = tako_app.upgrade() {
            cx.observe(&app, |this: &mut Self, _app, cx| {
                this.settings = settings::load();
                cx.notify();
            })
            .detach();
        }

        let mut this = Self {
            tako_app,
            tab: tab.unwrap_or(SettingsTab::General),
            settings,
            focus: cx.focus_handle(),
            expanded_categories: expanded,
            edit: None,
            ime: None,
            edit_bounds: None,
            message: None,
            status: StatusCache::default(),
            runner_new_ext: String::new(),
            runner_new_cmd: String::new(),
        };
        this.refresh_tab_status(cx);
        this
    }

    pub fn set_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.edit = None;
            self.message = None;
            self.refresh_tab_status(cx);
            cx.notify();
        }
    }

    // --- dispatch（CLI / MCP と同一経路） ---

    /// dispatch を実行し、スナップショットを再読込する。
    /// #486: 再読込しないと自分の操作結果すら画面へ反映されない（表示と実値がずれる）
    fn dispatch(
        &mut self,
        request: Request,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let Some(app) = self.tako_app.upgrade() else {
            return Err(txt::error_app_gone().to_string());
        };
        let result = app.update(cx, |app, cx| {
            let r =
                tako_control::dispatch::dispatch(app, request, tako_core::pane::PaneOrigin::User);
            // メインウィンドウ側も再描画する（テーマ・言語など描画に効く設定がある）
            cx.notify();
            r
        });
        self.settings = settings::load();
        cx.notify();
        result.map_err(|e| e.to_string())
    }

    /// dispatch して失敗時のみメッセージを出す（成功は画面の値の変化で分かる）
    fn run(&mut self, request: Request, cx: &mut Context<Self>) {
        match self.dispatch(request, cx) {
            Ok(_) => self.message = None,
            Err(e) => self.message = Some((e, true)),
        }
    }

    /// dispatch して成功メッセージも出す（結果が画面に出ない操作＝ボタン系で使う）
    fn run_with_message(&mut self, request: Request, success: String, cx: &mut Context<Self>) {
        match self.dispatch(request, cx) {
            Ok(_) => self.message = Some((success, false)),
            Err(e) => self.message = Some((e, true)),
        }
    }

    /// read-only の状態照会（メッセージも再読込もしない）
    fn query(&mut self, request: Request, cx: &mut Context<Self>) -> Option<serde_json::Value> {
        let app = self.tako_app.upgrade()?;
        app.update(cx, |app, _cx| {
            tako_control::dispatch::dispatch(app, request, tako_core::pane::PaneOrigin::User).ok()
        })
    }

    /// タブに必要な状態を取り直す（タブ切替・更新ボタン・関連操作の後に呼ぶ）
    fn refresh_tab_status(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            SettingsTab::General => {
                self.status.persist = self.query(Request::Persist { enabled: None }, cx);
            }
            SettingsTab::Setup => {
                self.status.fda = self.query(
                    Request::Fda {
                        action: Some("status".into()),
                    },
                    cx,
                );
                self.status.rules = self.query(
                    Request::AgentsSyncRules {
                        action: Some("status".into()),
                        source: None,
                        targets: None,
                    },
                    cx,
                );
                self.status.changes = self.query(Request::SetupChanges, cx);
                self.refresh_agent_clis(cx);
            }
            SettingsTab::Remote => {
                self.status.remote = self.query(Request::RemoteStatus, cx);
            }
            _ => {}
        }
    }

    /// エージェント CLI の検出は子プロセス（which）なので background で行う（#168 の教訓）
    fn refresh_agent_clis(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            ["claude", "codex", "agy"]
                .iter()
                .map(|cli| {
                    let found = std::process::Command::new("which")
                        .arg(cli)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    (cli.to_string(), found)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.status.agents = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn theme(&self) -> Theme {
        self.settings.resolve_theme().0
    }

    // --- テキスト入力 ---

    fn start_edit(
        &mut self,
        field: EditField,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 別フィールドを編集中ならそちらを確定してから切り替える
        if self.edit.is_some() {
            self.commit_edit(cx);
        }
        let cursor = initial.len();
        self.edit = Some(TextEdit {
            field,
            text: initial,
            cursor,
        });
        self.ime = None;
        window.focus(&self.focus.clone(), cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.edit = None;
        self.ime = None;
        cx.notify();
    }

    /// 編集内容を確定して対応する dispatch を撃つ
    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        self.ime = None;
        let value = edit.text.trim().to_string();
        match edit.field {
            EditField::ColorHex(key) => {
                if !value.is_empty() {
                    self.run(theme_request("set-color", Some(key), Some(value), None), cx);
                }
            }
            EditField::FontFamily => {
                let family = if value.is_empty() { None } else { Some(value) };
                self.run(font_request(family, None), cx);
            }
            EditField::FontSize => match value.parse::<f32>() {
                Ok(size) => self.run(font_request(None, Some(size)), cx),
                Err(_) => self.message = Some((txt::error_number().to_string(), true)),
            },
            EditField::PresetName => {
                if !value.is_empty() {
                    self.run_with_message(
                        theme_request("save-preset", None, None, Some(value.clone())),
                        format!("{}: {value}", txt::msg_preset_saved()),
                        cx,
                    );
                }
            }
            // 新規追加は 2 フィールドが揃ってから「追加」ボタンで確定する
            EditField::RunnerNewExt => self.runner_new_ext = value,
            EditField::RunnerNewCmd => self.runner_new_cmd = value,
            EditField::RunnerCmd(ext) => {
                if !value.is_empty() {
                    self.run(
                        Request::RunnerDefaults {
                            ext: Some(ext),
                            command: Some(value),
                            remove: false,
                        },
                        cx,
                    );
                }
            }
            EditField::PreviewCacheMb => match value.parse::<u64>() {
                Ok(mb) => self.run(Request::PreviewCache { max_mb: Some(mb) }, cx),
                Err(_) => self.message = Some((txt::error_number().to_string(), true)),
            },
            EditField::PaneLogMaxMb => match value.parse::<u64>() {
                Ok(mb) => self.run(logs_set(Some(mb), None), cx),
                Err(_) => self.message = Some((txt::error_number().to_string(), true)),
            },
            EditField::PaneLogTotalMaxMb => match value.parse::<u64>() {
                Ok(mb) => self.run(logs_set(None, Some(mb)), cx),
                Err(_) => self.message = Some((txt::error_number().to_string(), true)),
            },
            EditField::AdvancedJson => self.save_advanced_json(edit.text, cx),
        }
        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(ref mut edit) = self.edit {
            edit.text.insert_str(edit.cursor, text);
            edit.cursor += text.len();
            cx.notify();
        }
    }

    fn handle_key(&mut self, ks: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_none() {
            return;
        }
        let multiline = self
            .edit
            .as_ref()
            .map(|e| e.field.multiline())
            .unwrap_or(false);
        // 印字文字は EntityInputHandler（OS の入力経路）が挿入する。ここで無条件に
        // stop_propagation すると 1 文字も入らなくなる（#486 実機で観測）
        let handled = matches!(
            ks.key.as_str(),
            "enter" | "escape" | "backspace" | "delete" | "left" | "right" | "home" | "end"
        ) || (ks.key == "v" && ks.modifiers.platform);
        if !handled {
            return;
        }
        match ks.key.as_str() {
            "enter" => {
                // 複数行フィールドは Enter で改行、⌘+Enter で確定
                if multiline && !ks.modifiers.platform {
                    self.insert_text("\n", cx);
                } else {
                    self.commit_edit(cx);
                }
            }
            "escape" => self.cancel_edit(cx),
            "backspace" => {
                if let Some(ref mut edit) = self.edit {
                    if edit.cursor > 0 {
                        let prev = edit.text[..edit.cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        edit.text.drain(prev..edit.cursor);
                        edit.cursor = prev;
                    }
                }
                cx.notify();
            }
            "delete" => {
                if let Some(ref mut edit) = self.edit {
                    if edit.cursor < edit.text.len() {
                        let next = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                        edit.text.drain(edit.cursor..next);
                    }
                }
                cx.notify();
            }
            "left" => {
                if let Some(ref mut edit) = self.edit {
                    edit.cursor = edit.text[..edit.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                cx.notify();
            }
            "right" => {
                if let Some(ref mut edit) = self.edit {
                    if edit.cursor < edit.text.len() {
                        edit.cursor = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                    }
                }
                cx.notify();
            }
            "home" => {
                if let Some(ref mut edit) = self.edit {
                    edit.cursor = 0;
                }
                cx.notify();
            }
            "end" => {
                if let Some(ref mut edit) = self.edit {
                    edit.cursor = edit.text.len();
                }
                cx.notify();
            }
            "v" if ks.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert_text(&text, cx);
                }
            }
            _ => {}
        }
        window.invalidate_character_coordinates();
        cx.stop_propagation();
    }

    /// settings.json 直接編集の保存（パース検証 → 保存 → 全設定の再適用）
    fn save_advanced_json(&mut self, buffer: String, cx: &mut Context<Self>) {
        let parsed: Settings = match serde_json::from_str(&buffer) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some((format!("{}: {e}", txt::advanced_parse_error()), true));
                return;
            }
        };
        if let Err(e) = settings::save(&parsed) {
            self.message = Some((e.to_string(), true));
            return;
        }
        self.apply_all_settings(&parsed, cx);
        self.settings = settings::load();
        self.message = Some((txt::advanced_saved().to_string(), false));
    }

    /// 保存済み settings をランタイムへ反映する（既存 dispatch を順に撃つ = CLI と同一経路）
    fn apply_all_settings(&mut self, s: &Settings, cx: &mut Context<Self>) {
        let requests = vec![
            Request::Lang {
                action: Some("set".into()),
                value: Some(s.language.clone()),
            },
            Request::Theme {
                action: Some("set".into()),
                mode: Some(s.theme.clone()),
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            Request::AutoRename {
                enabled: Some(s.auto_rename),
            },
            Request::PortDetect {
                enabled: Some(s.port_detect),
            },
            Request::Persist {
                enabled: Some(s.tmux_persist),
            },
            Request::Telemetry {
                action: Some(if s.telemetry {
                    "on".into()
                } else {
                    "off".into()
                }),
            },
            Request::PreviewReload {
                enabled: Some(s.preview_live_reload),
            },
            Request::PreviewCache {
                max_mb: Some(s.preview_cache_max_mb),
            },
            Request::LimitService {
                action: Some("set".into()),
                service: Some(s.limit_service.clone()),
            },
            Request::SleepGuard {
                action: Some("set".into()),
                mode: Some(s.sleep_guard_mode.as_str().to_string()),
                power_condition: Some(s.sleep_guard_power.as_str().to_string()),
                lid_sleep_mode: Some(s.lid_sleep_mode.as_str().to_string()),
            },
            Request::Logs {
                action: "set".into(),
                enabled: Some(s.pane_logs),
                max_mb: Some(s.pane_log_max_mb),
                total_max_mb: Some(s.pane_log_total_max_mb),
                pane: None,
                session_id: None,
                lines: None,
            },
        ];
        for req in requests {
            let _ = self.dispatch(req, cx);
        }
    }

    fn editing(&self, field: &EditField) -> Option<&TextEdit> {
        self.edit.as_ref().filter(|e| &e.field == field)
    }
}

// --- IME（EntityInputHandler）---
// 文字入力はすべてこの経路で入る（on_key_down は制御キーのみ）。
// 日本語 IME の未確定文字列も同じバッファに差し込むのでインライン表示できる

impl EntityInputHandler for SettingsWindow {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = match self.ime {
            Some(ref ime) => ime.text.clone(),
            None => self.edit.as_ref()?.text.clone(),
        };
        let start = crate::utf16_to_byte_offset(&text, range_utf16.start);
        let end = crate::utf16_to_byte_offset(&text, range_utf16.end);
        text.get(start..end).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let range = match self.ime {
            Some(ref ime) => ime.selected_utf16.clone().unwrap_or_else(|| {
                let end = crate::utf16_len(&ime.text);
                end..end
            }),
            None => {
                let edit = self.edit.as_ref()?;
                let pos = crate::byte_to_utf16_offset(&edit.text, edit.cursor);
                pos..pos
            }
        };
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime.as_ref().map(|ime| 0..crate::utf16_len(&ime.text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ime) = self.ime.take() {
            if !ime.text.is_empty() {
                self.insert_text(&ime.text, cx);
            }
            window.invalidate_character_coordinates();
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime = None;
        if !text.is_empty() {
            self.insert_text(text, cx);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // IME は毎回未確定文字列の全文を渡すので丸ごと差し替える。空文字は変換キャンセル
        self.ime = if new_text.is_empty() {
            None
        } else {
            Some(ImeState {
                text: new_text.to_string(),
                selected_utf16: new_selected_range,
            })
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 変換候補ウィンドウは編集中フィールドの直下に出す
        let b = self.edit_bounds.unwrap_or(element_bounds);
        Some(Bounds::new(
            point(b.origin.x, b.origin.y),
            size(px(1.0), b.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();

        // IME の受け口を OS へ登録する（paint フェーズ限定 API なので canvas 経由）
        let ime_registration = {
            let entity = cx.entity();
            let focus = self.focus.clone();
            let bounds = self.edit_bounds;
            canvas(
                |_, _, _| (),
                move |element_bounds, _, window, cx| {
                    let target = bounds.unwrap_or(element_bounds);
                    window.handle_input(&focus, ElementInputHandler::new(target, entity), cx);
                },
            )
            .absolute()
            .size_full()
        };

        div()
            .key_context("SettingsWindow")
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .bg(to_hsla(theme.surface_0))
            .text_color(to_hsla(theme.foreground))
            .on_action(cx.listener(|this, _: &crate::ClosePane, window, cx| {
                // 設定ウィンドウ内の ⌘W は「このウィンドウを閉じる」（設計 §3.4）。
                // remove_window は on_window_should_close を通らないのでハンドルを自分で外す
                if let Some(app) = this.tako_app.upgrade() {
                    app.update(cx, |app, _| app.settings_window_handle = None);
                }
                window.remove_window();
            }))
            // ⌘V はキーバインドで PasteClipboard アクションになるため on_key_down へ
            // 落ちてこない。設定ウィンドウでは編集中バッファへの貼り付けとして受ける
            // （ハンドラが無いとメインウィンドウのターミナルへ流れる恐れもある）
            .on_action(cx.listener(|this, _: &crate::PasteClipboard, _, cx| {
                if this.edit.is_some() {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        this.insert_text(&text, cx);
                    }
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_key(&ev.keystroke, window, cx);
            }))
            .child(ime_registration)
            .child(self.render_nav(cx))
            .child(self.render_content(cx))
    }
}

impl SettingsWindow {
    fn render_nav(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let current = self.tab;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(180.))
            .h_full()
            .bg(to_hsla(theme.mantle))
            .border_r_1()
            .border_color(to_hsla(theme.border_subtle))
            .pt_2()
            .pb_2()
            .children(SettingsTab::ALL.iter().map(|&tab| {
                let is_active = tab == current;
                div()
                    .id(SharedString::from(format!("tab-{tab:?}")))
                    .px_3()
                    .py(px(7.))
                    .mx_2()
                    .rounded(px(6.))
                    .bg(if is_active {
                        to_hsla(theme.surface_highlight)
                    } else {
                        transparent_black()
                    })
                    .text_color(if is_active {
                        to_hsla(theme.foreground)
                    } else {
                        to_hsla(theme.text_muted)
                    })
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|s| s.bg(to_hsla(theme.surface_hover)))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_tab(tab, cx);
                    }))
            }))
    }

    fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let content = match self.tab {
            SettingsTab::General => self.render_general_tab(cx),
            SettingsTab::Appearance => self.render_appearance_tab(cx),
            SettingsTab::Runner => self.render_runner_tab(cx),
            SettingsTab::Setup => self.render_setup_tab(cx),
            SettingsTab::Sleep => self.render_sleep_tab(cx),
            SettingsTab::Remote => self.render_remote_tab(cx),
            SettingsTab::Advanced => self.render_advanced_tab(cx),
        };
        div()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    // タブごとにスクロール状態を分ける（同一 id だと前のタブの
                    // スクロール位置が残り、切替後に途中から表示される。#486）
                    .id(SharedString::from(format!("settings-content-{:?}", self.tab)))
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .bg(to_hsla(theme.surface_0))
                    .px_5()
                    .py_4()
                    .child(content),
            )
            .children(self.message.as_ref().map(|(text, is_error)| {
                div()
                    .flex_none()
                    .px_5()
                    .py_2()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .bg(if *is_error {
                        to_hsla(theme.danger_surface)
                    } else {
                        to_hsla(theme.surface_1)
                    })
                    .text_color(if *is_error {
                        to_hsla(theme.red)
                    } else {
                        to_hsla(theme.text_secondary)
                    })
                    .text_size(px(12.))
                    .child(text.clone())
            }))
    }

    // --- 共通ウィジェット ---

    /// ラベル + 説明 + 右コントロールの 1 行
    fn row(&self, label: &str, desc: &str, control: impl IntoElement) -> Div {
        let theme = self.theme();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .py(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(13.))
                            .child(label.to_string()),
                    )
                    .when(!desc.is_empty(), |d| {
                        d.child(
                            div()
                                .text_color(to_hsla(theme.text_muted))
                                .text_size(px(11.))
                                .child(desc.to_string()),
                        )
                    }),
            )
            .child(div().flex_none().child(control))
    }

    fn section(&self, title: &str) -> Div {
        let theme = self.theme();
        div()
            .pt_2()
            .pb_1()
            .text_color(to_hsla(theme.text_secondary))
            .text_size(px(12.))
            .child(title.to_string())
    }

    fn toggle(
        &self,
        id: &str,
        value: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme();
        div()
            .id(SharedString::from(format!("toggle-{id}")))
            .w(px(38.))
            .h(px(22.))
            .rounded(px(11.))
            .bg(if value {
                to_hsla(theme.accent)
            } else {
                to_hsla(theme.border_default)
            })
            .cursor_pointer()
            .child(
                div()
                    .w(px(18.))
                    .h(px(18.))
                    .mt(px(2.))
                    .ml(if value { px(18.) } else { px(2.) })
                    .rounded(px(9.))
                    .bg(gpui::white()),
            )
            .on_click(handler)
    }

    /// 値の選択（セグメント）。値ごとに make_request を呼んで dispatch する
    fn segmented(
        &self,
        id: &str,
        options: &[(&'static str, String)],
        current: &str,
        cx: &mut Context<Self>,
        make_request: impl Fn(&str) -> Request + Clone + 'static,
    ) -> Div {
        let theme = self.theme();
        let mut row = div()
            .flex()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(to_hsla(theme.surface_1));
        for (value, label) in options {
            let active = *value == current;
            let req = make_request.clone();
            let value_owned = value.to_string();
            row = row.child(
                div()
                    .id(SharedString::from(format!("seg-{id}-{value}")))
                    .px_3()
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(if active {
                        to_hsla(theme.accent)
                    } else {
                        transparent_black()
                    })
                    .text_color(if active {
                        gpui::white()
                    } else {
                        to_hsla(theme.text_muted)
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(!active, |d| {
                        d.hover(|s| s.bg(to_hsla(theme.surface_hover_strong)))
                    })
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let request = req(&value_owned);
                        this.run(request, cx);
                    })),
            );
        }
        row
    }

    fn button(
        &self,
        id: &str,
        label: &str,
        kind: BtnKind,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme();
        let (bg, fg) = match kind {
            BtnKind::Primary => (to_hsla(theme.accent), gpui::white()),
            BtnKind::Normal => (to_hsla(theme.chip_surface), to_hsla(theme.foreground)),
            BtnKind::Danger => (to_hsla(theme.danger_surface), to_hsla(theme.red)),
            BtnKind::Disabled => (to_hsla(theme.surface_1), to_hsla(theme.text_faint)),
        };
        let mut b = div()
            .id(SharedString::from(format!("btn-{id}")))
            .flex_none()
            .px_3()
            .py(px(5.))
            .rounded(px(6.))
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .child(label.to_string());
        if !matches!(kind, BtnKind::Disabled) {
            b = b.cursor_pointer().on_click(handler);
        }
        b
    }

    /// テキスト入力欄。クリックで編集開始、Enter で確定、Esc で取消
    /// テキスト入力欄。クリックで編集開始、Enter で確定、Esc で取消。
    /// width = None は親の残り幅いっぱい（flex_1）に伸ばす
    fn text_field(
        &self,
        field: EditField,
        current: &str,
        placeholder: &str,
        width: Option<Pixels>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme();
        let editing = self.editing(&field);
        let id = field.slug();
        let field_for_click = field.clone();
        let initial = current.to_string();

        let mut inner = div()
            .flex()
            .items_center()
            .min_w(px(0.))
            .overflow_hidden()
            .text_size(px(12.));

        match editing {
            Some(edit) => {
                let (before, after) = edit.text.split_at(edit.cursor);
                inner = inner
                    .child(
                        div()
                            .text_color(to_hsla(theme.foreground))
                            .child(SharedString::from(before.to_string())),
                    )
                    // IME の未確定文字列はカーソル位置にインライン表示する
                    .when_some(self.ime.as_ref(), |d, ime| {
                        d.child(
                            div()
                                .text_color(to_hsla(theme.accent))
                                .child(SharedString::from(ime.text.clone())),
                        )
                    })
                    .child(
                        div()
                            .w(px(1.5))
                            .h(px(14.))
                            .flex_none()
                            .bg(to_hsla(theme.accent)),
                    )
                    .child(
                        div()
                            .text_color(to_hsla(theme.foreground))
                            .child(SharedString::from(after.to_string())),
                    );
            }
            None if current.is_empty() => {
                inner = inner.child(
                    div()
                        .text_color(to_hsla(theme.text_faint))
                        .child(placeholder.to_string()),
                );
            }
            None => {
                inner = inner.child(
                    div()
                        .text_color(to_hsla(theme.foreground))
                        .child(current.to_string()),
                );
            }
        }

        let is_editing = editing.is_some();
        div()
            .id(SharedString::from(format!("field-{id}")))
            .map(|d| match width {
                Some(w) => d.w(w).flex_none(),
                None => d.flex_1().min_w(px(0.)),
            })
            .px_2()
            .py(px(3.))
            .rounded(px(5.))
            .bg(to_hsla(theme.crust))
            .border_1()
            .border_color(if is_editing {
                to_hsla(theme.accent)
            } else {
                to_hsla(theme.border_default)
            })
            .cursor(CursorStyle::IBeam)
            .child(inner)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_edit(field_for_click.clone(), initial.clone(), window, cx);
            }))
    }

    // --- 一般タブ ---

    fn render_general_tab(&self, cx: &mut Context<Self>) -> Div {
        let s = self.settings.clone();
        let theme = self.theme();

        // 永続化の注記（tmux 不在 / セカンダリモード）
        let persist_note = self
            .status
            .persist
            .as_ref()
            .map(|v| {
                let available = v.get("available").and_then(|x| x.as_bool()).unwrap_or(true);
                let secondary = v
                    .get("secondary")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                if secondary {
                    txt::desc_persist_secondary().to_string()
                } else if !available {
                    txt::desc_persist_no_tmux().to_string()
                } else {
                    txt::desc_persist().to_string()
                }
            })
            .unwrap_or_else(|| txt::desc_persist().to_string());

        let auto_rename = s.auto_rename;
        let port_detect = s.port_detect;
        let persist = s.tmux_persist;
        let telemetry = s.telemetry;
        let reload = s.preview_live_reload;
        let pane_logs = s.pane_logs;
        let confirm_close = tako_control::setup::confirm_close_enabled();

        div()
            .flex()
            .flex_col()
            .gap_1()
            // 言語（Issue #488）
            .child(self.row(
                txt::label_language(),
                txt::desc_language(),
                self.segmented(
                    "lang",
                    &[
                        ("system", txt::lang_system().to_string()),
                        ("ja", txt::lang_ja().to_string()),
                        ("en", txt::lang_en().to_string()),
                    ],
                    &s.language,
                    cx,
                    |value| Request::Lang {
                        action: Some("set".into()),
                        value: Some(value.to_string()),
                    },
                ),
            ))
            .child(self.row(
                txt::label_auto_rename(),
                txt::desc_auto_rename(),
                self.toggle(
                    "auto-rename",
                    auto_rename,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::AutoRename {
                                enabled: Some(!auto_rename),
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_port_detect(),
                txt::desc_port_detect(),
                self.toggle(
                    "port-detect",
                    port_detect,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::PortDetect {
                                enabled: Some(!port_detect),
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_persist(),
                &persist_note,
                self.toggle(
                    "persist",
                    persist,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::Persist {
                                enabled: Some(!persist),
                            },
                            cx,
                        );
                        this.refresh_tab_status(cx);
                    }),
                ),
            ))
            .child(self.row(
                txt::label_confirm_close(),
                txt::desc_confirm_close(),
                self.toggle(
                    "confirm-close",
                    confirm_close,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::ConfirmClose {
                                enabled: Some(!confirm_close),
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_telemetry(),
                txt::desc_telemetry(),
                self.toggle(
                    "telemetry",
                    telemetry,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::Telemetry {
                                action: Some(if telemetry { "off".into() } else { "on".into() }),
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_limit_service(),
                txt::desc_limit_service(),
                self.segmented(
                    "limit",
                    &[
                        ("claude", "Claude".to_string()),
                        ("codex", "Codex".to_string()),
                        ("agy", "agy".to_string()),
                    ],
                    &s.limit_service,
                    cx,
                    |value| Request::LimitService {
                        action: Some("set".into()),
                        service: Some(value.to_string()),
                    },
                ),
            ))
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .child(self.section(txt::section_preview())),
            )
            .child(self.row(
                txt::label_preview_reload(),
                txt::desc_preview_reload(),
                self.toggle(
                    "preview-reload",
                    reload,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::PreviewReload {
                                enabled: Some(!reload),
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_preview_cache(),
                txt::desc_preview_cache(),
                self.text_field(
                    EditField::PreviewCacheMb,
                    &s.preview_cache_max_mb.to_string(),
                    "512",
                    Some(px(90.)),
                    cx,
                ),
            ))
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .child(self.section(txt::section_logs())),
            )
            .child(self.row(
                txt::label_pane_logs(),
                txt::desc_pane_logs(),
                self.toggle(
                    "pane-logs",
                    pane_logs,
                    cx.listener(move |this, _, _, cx| {
                        this.run(
                            Request::Logs {
                                action: "set".into(),
                                enabled: Some(!pane_logs),
                                max_mb: None,
                                total_max_mb: None,
                                pane: None,
                                session_id: None,
                                lines: None,
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::label_pane_log_max(),
                txt::desc_pane_log_max(),
                self.text_field(
                    EditField::PaneLogMaxMb,
                    &s.pane_log_max_mb.to_string(),
                    "5",
                    Some(px(90.)),
                    cx,
                ),
            ))
            .child(self.row(
                txt::label_pane_log_total(),
                txt::desc_pane_log_total(),
                self.text_field(
                    EditField::PaneLogTotalMaxMb,
                    &s.pane_log_total_max_mb.to_string(),
                    "200",
                    Some(px(90.)),
                    cx,
                ),
            ))
    }

    // --- 外観タブ ---

    fn render_appearance_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let (resolved_theme, _) = self.settings.resolve_theme();
        let current_theme = self.settings.theme.clone();
        let overrides = self
            .settings
            .theme_colors
            .get(theme.mode.as_str())
            .cloned()
            .unwrap_or_default();

        // テーマ選択（ビルトイン + 保存済みプリセット）
        let mut theme_options: Vec<(&'static str, String)> = vec![
            ("dark", txt::theme_dark().to_string()),
            ("light", txt::theme_light().to_string()),
        ];
        let preset_names: Vec<String> = self.settings.theme_presets.keys().cloned().collect();

        let mut content = div().flex().flex_col().gap_1();

        content = content.child(self.row(
            txt::label_theme(),
            txt::desc_theme(),
            self.segmented(
                "theme",
                &std::mem::take(&mut theme_options),
                &current_theme,
                cx,
                |value| Request::Theme {
                    action: Some("set".into()),
                    mode: Some(value.to_string()),
                    target: None,
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
            ),
        ));

        // フォント
        content = content.child(self.row(
            txt::label_font_family(),
            txt::desc_font_family(),
            self.text_field(
                EditField::FontFamily,
                self.settings.font_family.as_deref().unwrap_or(""),
                "Menlo",
                Some(px(180.)),
                cx,
            ),
        ));
        content = content.child(
            self.row(
                txt::label_font_size(),
                txt::desc_font_size(),
                self.text_field(
                    EditField::FontSize,
                    &self
                        .settings
                        .font_size
                        .map(|s| format!("{s}"))
                        .unwrap_or_default(),
                    "13",
                    Some(px(90.)),
                    cx,
                ),
            ),
        );

        // プリセット
        content = content.child(
            div()
                .pt_3()
                .border_t_1()
                .border_color(to_hsla(theme.border_subtle))
                .child(self.section(txt::label_preset())),
        );
        content = content.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py(px(4.))
                .child(self.text_field(
                    EditField::PresetName,
                    "",
                    txt::placeholder_preset_name(),
                    Some(px(160.)),
                    cx,
                ))
                .child(self.button(
                    "save-preset",
                    txt::button_save_preset(),
                    BtnKind::Primary,
                    cx.listener(|this, _, _, cx| {
                        // 入力途中でも保存ボタンで確定できるようにする
                        if this
                            .edit
                            .as_ref()
                            .is_some_and(|e| e.field == EditField::PresetName)
                        {
                            this.commit_edit(cx);
                        } else {
                            this.message =
                                Some((txt::msg_preset_name_required().to_string(), true));
                        }
                    }),
                )),
        );
        if preset_names.is_empty() {
            content = content.child(
                div()
                    .text_color(to_hsla(theme.text_faint))
                    .text_size(px(11.))
                    .child(txt::msg_no_presets()),
            );
        } else {
            for name in preset_names {
                let is_current = name == current_theme;
                let name_apply = name.clone();
                let name_delete = name.clone();
                content = content.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py(px(2.))
                        .child(
                            div()
                                .flex_1()
                                .text_color(to_hsla(if is_current {
                                    theme.accent
                                } else {
                                    theme.foreground
                                }))
                                .text_size(px(12.))
                                .child(name.clone()),
                        )
                        .child(self.button(
                            &format!("apply-preset-{name}"),
                            txt::button_apply(),
                            BtnKind::Normal,
                            cx.listener(move |this, _, _, cx| {
                                this.run(
                                    Request::Theme {
                                        action: Some("set".into()),
                                        mode: Some(name_apply.clone()),
                                        target: None,
                                        key: None,
                                        value: None,
                                        name: None,
                                        font_family: None,
                                        font_size: None,
                                    },
                                    cx,
                                );
                            }),
                        ))
                        .child(self.button(
                            &format!("delete-preset-{name}"),
                            txt::button_delete(),
                            BtnKind::Danger,
                            cx.listener(move |this, _, _, cx| {
                                this.run(
                                    theme_request(
                                        "delete-preset",
                                        None,
                                        None,
                                        Some(name_delete.clone()),
                                    ),
                                    cx,
                                );
                            }),
                        )),
                );
            }
        }

        // 色設定
        content = content.child(
            div()
                .pt_3()
                .border_t_1()
                .border_color(to_hsla(theme.border_subtle))
                .flex()
                .items_center()
                .justify_between()
                .child(self.section(txt::label_color_settings()))
                .child(self.button(
                    "reset-all-colors",
                    txt::button_reset_all(),
                    BtnKind::Danger,
                    cx.listener(|this, _, _, cx| {
                        this.run(theme_request("reset-colors", None, None, None), cx);
                    }),
                )),
        );

        let mut key_offset = 0usize;
        for (cat_idx, &(cat_id, _, count)) in Theme::COLOR_CATEGORIES.iter().enumerate() {
            let expanded = self
                .expanded_categories
                .get(cat_idx)
                .copied()
                .unwrap_or(false);
            let cat_label = match cat_id {
                "terminal" => txt::category_terminal(),
                "background" => txt::category_background(),
                "border" => txt::category_border(),
                "text" => txt::category_text(),
                "accent" => txt::category_accent(),
                "chrome" => txt::category_chrome(),
                _ => cat_id,
            };
            let mut section = div().flex().flex_col().child(
                div()
                    .id(SharedString::from(format!("cat-{cat_idx}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(5.))
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(to_hsla(theme.surface_hover)))
                    .child(
                        svg()
                            .path(if expanded {
                                ui_icon::CHEVRON_DOWN
                            } else {
                                ui_icon::CHEVRON_RIGHT
                            })
                            .w(px(12.))
                            .h(px(12.))
                            .flex_none()
                            .text_color(to_hsla(theme.text_muted)),
                    )
                    .child(
                        div()
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(13.))
                            .child(format!("{cat_label} ({count})")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.expanded_categories.get_mut(cat_idx) {
                            *v = !*v;
                        }
                        cx.notify();
                    })),
            );
            if expanded {
                let keys = &Theme::COLOR_KEYS[key_offset..key_offset + count];
                for &key in keys {
                    let color = resolved_theme.color(key).unwrap_or(Rgb::new(0, 0, 0));
                    let hex = color.to_hex();
                    let overridden = overrides.contains_key(key);
                    let key_owned = key.to_string();
                    section = section.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .py(px(3.))
                            .pl_5()
                            .child(
                                div()
                                    .w(px(16.))
                                    .h(px(16.))
                                    .flex_none()
                                    .rounded(px(3.))
                                    .bg(to_hsla(color))
                                    .border_1()
                                    .border_color(to_hsla(theme.border_default)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_color(to_hsla(if overridden {
                                        theme.accent
                                    } else {
                                        theme.foreground
                                    }))
                                    .text_size(px(12.))
                                    .child(key.to_string()),
                            )
                            .child(self.text_field(
                                EditField::ColorHex(key_owned.clone()),
                                &hex,
                                "#000000",
                                Some(px(90.)),
                                cx,
                            ))
                            .child(if overridden {
                                self.button(
                                    &format!("reset-{key}"),
                                    txt::button_reset(),
                                    BtnKind::Normal,
                                    cx.listener(move |this, _, _, cx| {
                                        this.run(
                                            theme_request(
                                                "reset-color",
                                                Some(key_owned.clone()),
                                                None,
                                                None,
                                            ),
                                            cx,
                                        );
                                    }),
                                )
                            } else {
                                self.button(
                                    &format!("reset-{key}"),
                                    txt::button_reset(),
                                    BtnKind::Disabled,
                                    |_, _, _| {},
                                )
                            }),
                    );
                }
            }
            content = content.child(section);
            key_offset += count;
        }
        content
    }

    // --- Code Runner タブ ---

    fn render_runner_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let merged = tako_core::merged_defaults(&self.settings.runner_defaults);

        let mut table = div().flex().flex_col();
        table = table.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py(px(4.))
                .border_b_1()
                .border_color(to_hsla(theme.border_subtle))
                .child(
                    div()
                        .w(px(70.))
                        .flex_none()
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_ext()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_command()),
                )
                .child(
                    div()
                        .w(px(60.))
                        .flex_none()
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_source()),
                )
                .child(div().w(px(80.)).flex_none()),
        );

        for (ext, cmd) in &merged {
            let is_user = self.settings.runner_defaults.contains_key(ext);
            let ext_owned = ext.clone();
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(3.))
                    .child(
                        div()
                            .w(px(70.))
                            .flex_none()
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .child(ext.clone()),
                    )
                    .child(div().flex_1().min_w(px(0.)).flex().child(self.text_field(
                        EditField::RunnerCmd(ext.clone()),
                        cmd,
                        "",
                        None,
                        cx,
                    )))
                    .child(
                        div()
                            .w(px(60.))
                            .flex_none()
                            .text_color(if is_user {
                                to_hsla(theme.accent)
                            } else {
                                to_hsla(theme.text_muted)
                            })
                            .text_size(px(11.))
                            .child(if is_user {
                                txt::runner_source_user()
                            } else {
                                txt::runner_source_builtin()
                            }),
                    )
                    .child(div().w(px(80.)).flex_none().child(if is_user {
                        self.button(
                            &format!("rd-reset-{ext}"),
                            txt::button_reset(),
                            BtnKind::Normal,
                            cx.listener(move |this, _, _, cx| {
                                this.run(
                                    Request::RunnerDefaults {
                                        ext: Some(ext_owned.clone()),
                                        command: None,
                                        remove: true,
                                    },
                                    cx,
                                );
                            }),
                        )
                    } else {
                        self.button(
                            &format!("rd-reset-{ext}"),
                            txt::button_reset(),
                            BtnKind::Disabled,
                            |_, _, _| {},
                        )
                    })),
            );
        }

        let new_ext = self.runner_new_ext_value();
        let new_cmd = self.runner_new_cmd_value();
        let can_add = !new_ext.trim().is_empty() && !new_cmd.trim().is_empty();

        let add_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.section(txt::runner_add_header()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.text_field(
                        EditField::RunnerNewExt,
                        &new_ext,
                        txt::runner_col_ext(),
                        Some(px(70.)),
                        cx,
                    ))
                    .child(div().flex_1().min_w(px(0.)).flex().child(self.text_field(
                        EditField::RunnerNewCmd,
                        &new_cmd,
                        txt::runner_placeholder_command(),
                        None,
                        cx,
                    )))
                    .child(if can_add {
                        self.button(
                            "runner-add",
                            txt::runner_add_btn(),
                            BtnKind::Primary,
                            cx.listener(|this, _, _, cx| this.commit_runner_add(cx)),
                        )
                    } else {
                        self.button(
                            "runner-add",
                            txt::runner_add_btn(),
                            BtnKind::Disabled,
                            |_, _, _| {},
                        )
                    }),
            );

        let help = div()
            .flex()
            .flex_col()
            .gap_1()
            .pt_4()
            .child(self.section(txt::runner_help_header()))
            .children(
                [
                    ("${file}", txt::runner_var_file()),
                    ("${fileDir}", txt::runner_var_filedir()),
                    ("${fileBase}", txt::runner_var_filebase()),
                    ("${fileNoExt}", txt::runner_var_filenoext()),
                    ("${ext}", txt::runner_var_ext()),
                ]
                .into_iter()
                .map(|(var, desc)| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_2()
                        .child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .text_color(to_hsla(theme.accent))
                                .text_size(px(12.))
                                .child(var),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_color(to_hsla(theme.text_muted))
                                .text_size(px(12.))
                                .child(desc),
                        )
                }),
            )
            .child(
                div()
                    .pt_1()
                    .text_color(to_hsla(theme.text_faint))
                    .text_size(px(11.))
                    .child(txt::runner_resolution_help()),
            );

        // 新規追加は表より上に置く（表は 21 行あり、下に置くと画面外へ押し出されて
        // 追加できなくなる。#486 の実機監査で判明）
        div()
            .flex()
            .flex_col()
            .child(add_section)
            .child(
                div()
                    .pt_4()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .child(self.section(txt::runner_header())),
            )
            .child(
                div()
                    .pb_2()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(txt::runner_edit_help()),
            )
            .child(table)
            .child(help)
    }

    fn commit_runner_add(&mut self, cx: &mut Context<Self>) {
        // 入力途中（編集中）の値も拾ってから追加する
        if self.edit.is_some() {
            self.commit_edit(cx);
        }
        let ext = self.runner_new_ext.trim().to_string();
        let cmd = self.runner_new_cmd.trim().to_string();
        if ext.is_empty() || cmd.is_empty() {
            return;
        }
        self.run(
            Request::RunnerDefaults {
                ext: Some(ext),
                command: Some(cmd),
                remove: false,
            },
            cx,
        );
        self.runner_new_ext.clear();
        self.runner_new_cmd.clear();
    }

    fn runner_new_ext_value(&self) -> String {
        match self.editing(&EditField::RunnerNewExt) {
            Some(edit) => edit.text.clone(),
            None => self.runner_new_ext.clone(),
        }
    }

    fn runner_new_cmd_value(&self) -> String {
        match self.editing(&EditField::RunnerNewCmd) {
            Some(edit) => edit.text.clone(),
            None => self.runner_new_cmd.clone(),
        }
    }

    // --- セットアップタブ ---

    fn render_setup_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();

        let agents = self.status.agents.clone();
        let agents_rows = match agents {
            Some(list) => {
                let mut rows = div().flex().flex_col().gap(px(2.));
                for (cli, found) in list {
                    rows = rows.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(80.))
                                    .flex_none()
                                    .text_color(to_hsla(theme.foreground))
                                    .text_size(px(12.))
                                    .child(cli.clone()),
                            )
                            .child(
                                div()
                                    .text_color(if found {
                                        to_hsla(theme.green)
                                    } else {
                                        to_hsla(theme.text_faint)
                                    })
                                    .text_size(px(12.))
                                    .child(if found {
                                        txt::setup_installed()
                                    } else {
                                        txt::setup_not_installed()
                                    }),
                            ),
                    );
                }
                rows
            }
            None => div().child(
                div()
                    .text_color(to_hsla(theme.text_faint))
                    .text_size(px(12.))
                    .child(txt::msg_loading()),
            ),
        };

        let fda_status = self
            .status
            .fda
            .as_ref()
            .and_then(|v| v.get("full_disk_access"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let fda_granted = fda_status == "granted";

        let rules_summary = self
            .status
            .rules
            .as_ref()
            .map(|v| {
                let source = v
                    .get("source")
                    .and_then(|x| x.as_str())
                    .unwrap_or("-")
                    .to_string();
                let targets = v
                    .get("targets")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                format!("{source} / {targets}")
            })
            .unwrap_or_else(|| "-".to_string());

        let pending_changes = self
            .status
            .changes
            .as_ref()
            .and_then(|v| v.get("pending"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.section(txt::setup_agents_header()))
                    .child(self.button(
                        "setup-refresh",
                        txt::button_refresh(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.refresh_tab_status(cx);
                            this.message = Some((txt::msg_refreshed().to_string(), false));
                        }),
                    )),
            )
            .child(agents_rows)
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .child(
                        self.row(
                            txt::setup_fda_header(),
                            txt::desc_fda(),
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_color(if fda_granted {
                                            to_hsla(theme.green)
                                        } else {
                                            to_hsla(theme.yellow)
                                        })
                                        .text_size(px(12.))
                                        .child(fda_status),
                                )
                                .child(self.button(
                                    "fda-open",
                                    txt::setup_fda_open(),
                                    BtnKind::Normal,
                                    cx.listener(|this, _, _, cx| {
                                        this.run_with_message(
                                            Request::Fda {
                                                action: Some("open".into()),
                                            },
                                            txt::msg_opened_settings().to_string(),
                                            cx,
                                        );
                                    }),
                                )),
                        ),
                    ),
            )
            .child(self.row(
                txt::setup_mcp_header(),
                txt::desc_mcp(),
                self.button(
                    "setup-mcp",
                    txt::setup_mcp_register(),
                    BtnKind::Normal,
                    cx.listener(|this, _, _, cx| {
                        this.run_with_message(
                            Request::SetupMcp {
                                scope: None,
                                pane: None,
                            },
                            txt::msg_mcp_registered().to_string(),
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::setup_rules_header(),
                &format!("{}: {rules_summary}", txt::desc_rules()),
                self.button(
                    "setup-rules",
                    txt::setup_rules_sync(),
                    BtnKind::Normal,
                    cx.listener(|this, _, _, cx| {
                        this.run_with_message(
                            Request::AgentsSyncRules {
                                action: Some("sync".into()),
                                source: None,
                                targets: None,
                            },
                            txt::msg_rules_synced().to_string(),
                            cx,
                        );
                        this.refresh_tab_status(cx);
                    }),
                ),
            ))
            .child(self.row(
                txt::setup_changes_header(),
                &if pending_changes == 0 {
                    txt::desc_changes_none().to_string()
                } else {
                    format!("{}: {pending_changes}", txt::desc_changes_pending())
                },
                self.button(
                    "setup-run",
                    txt::setup_run_btn(),
                    BtnKind::Primary,
                    cx.listener(|this, _, _, cx| {
                        this.run_with_message(
                            Request::RunInteractive {
                                command: "tako setup".into(),
                                pane: None,
                                tab: None,
                                input_hint: None,
                                direction: None,
                                ratio: None,
                                auto_close: None,
                            },
                            txt::msg_setup_started().to_string(),
                            cx,
                        );
                    }),
                ),
            ))
    }

    // --- スリープ防止タブ ---

    fn render_sleep_tab(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.settings;
        let mode = s.sleep_guard_mode.as_str().to_string();
        let power = s.sleep_guard_power.as_str().to_string();
        let lid = s.lid_sleep_mode.as_str().to_string();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.row(
                txt::sleep_mode_header(),
                txt::desc_sleep_mode(),
                self.segmented(
                    "sleep-mode",
                    &[
                        ("off", txt::sleep_mode_off().to_string()),
                        ("on", txt::sleep_mode_on().to_string()),
                        ("while-agents-running", txt::sleep_mode_agents().to_string()),
                    ],
                    &mode,
                    cx,
                    |value| Request::SleepGuard {
                        action: Some("set".into()),
                        mode: Some(value.to_string()),
                        power_condition: None,
                        lid_sleep_mode: None,
                    },
                ),
            ))
            .child(self.row(
                txt::sleep_power_header(),
                txt::desc_sleep_power(),
                self.segmented(
                    "sleep-power",
                    &[
                        ("ac-only", txt::sleep_power_ac().to_string()),
                        ("always", txt::sleep_power_always().to_string()),
                    ],
                    &power,
                    cx,
                    |value| Request::SleepGuard {
                        action: Some("set".into()),
                        mode: None,
                        power_condition: Some(value.to_string()),
                        lid_sleep_mode: None,
                    },
                ),
            ))
            .child(self.row(
                txt::sleep_lid_header(),
                txt::desc_sleep_lid(),
                self.segmented(
                    "sleep-lid",
                    &[
                        ("off", txt::sleep_mode_off().to_string()),
                        ("while-agents-running", txt::sleep_mode_agents().to_string()),
                    ],
                    &lid,
                    cx,
                    |value| Request::SleepGuard {
                        action: Some("set".into()),
                        mode: None,
                        power_condition: None,
                        lid_sleep_mode: Some(value.to_string()),
                    },
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .child(self.button(
                        "lid-install",
                        txt::sleep_lid_install(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.run_with_message(
                                Request::SleepGuard {
                                    action: Some("install-lid-sleep".into()),
                                    mode: None,
                                    power_condition: None,
                                    lid_sleep_mode: None,
                                },
                                txt::msg_lid_installed().to_string(),
                                cx,
                            );
                        }),
                    ))
                    .child(self.button(
                        "lid-remove",
                        txt::sleep_lid_remove(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.run_with_message(
                                Request::SleepGuard {
                                    action: Some("remove-lid-sleep".into()),
                                    mode: None,
                                    power_condition: None,
                                    lid_sleep_mode: None,
                                },
                                txt::msg_lid_removed().to_string(),
                                cx,
                            );
                        }),
                    )),
            )
    }

    // --- リモートタブ ---

    fn render_remote_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let status = self.status.remote.clone();
        let running = status
            .as_ref()
            .and_then(|v| v.get("running"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let url = status
            .as_ref()
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let transport = status
            .as_ref()
            .and_then(|v| v.get("transport"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.section(txt::remote_daemon_header()))
                    .child(self.button(
                        "remote-refresh",
                        txt::button_refresh(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.refresh_tab_status(cx);
                            this.message = Some((txt::msg_refreshed().to_string(), false));
                        }),
                    )),
            )
            .child(
                self.row(
                    txt::remote_status_label(),
                    &transport,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_color(if running {
                                    to_hsla(theme.green)
                                } else {
                                    to_hsla(theme.text_faint)
                                })
                                .text_size(px(12.))
                                .child(if running {
                                    txt::remote_status_running()
                                } else {
                                    txt::remote_status_stopped()
                                }),
                        )
                        .child(if running {
                            self.button(
                                "remote-stop",
                                txt::remote_stop(),
                                BtnKind::Danger,
                                cx.listener(|this, _, _, cx| {
                                    this.run_with_message(
                                        Request::RemoteStop { force: false },
                                        txt::msg_remote_stopped().to_string(),
                                        cx,
                                    );
                                    this.refresh_tab_status(cx);
                                }),
                            )
                        } else {
                            self.button(
                                "remote-start",
                                txt::remote_start(),
                                BtnKind::Primary,
                                cx.listener(|this, _, _, cx| {
                                    this.run_with_message(
                                        Request::RemoteStart {},
                                        txt::msg_remote_started().to_string(),
                                        cx,
                                    );
                                    this.refresh_tab_status(cx);
                                }),
                            )
                        }),
                ),
            )
            .when(!url.is_empty(), |d| {
                let url_copy = url.clone();
                d.child(
                    self.row(
                        txt::remote_url_label(),
                        txt::desc_remote_url(),
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .max_w(px(320.))
                                    .overflow_hidden()
                                    .text_color(to_hsla(theme.text_muted))
                                    .text_size(px(11.))
                                    .child(url.clone()),
                            )
                            .child(self.button(
                                "remote-copy-url",
                                txt::button_copy(),
                                BtnKind::Normal,
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        url_copy.clone(),
                                    ));
                                    this.message = Some((txt::msg_copied().to_string(), false));
                                    cx.notify();
                                }),
                            )),
                    ),
                )
            })
            .child(self.row(
                txt::remote_setup_header(),
                txt::desc_remote_setup(),
                self.button(
                    "remote-setup-check",
                    txt::button_check(),
                    BtnKind::Normal,
                    cx.listener(|this, _, _, cx| {
                        let result = this.dispatch(
                            Request::RemoteSetup {
                                action: "check".into(),
                                answers: None,
                            },
                            cx,
                        );
                        this.message = Some(match result {
                            Ok(v) => (summarize_remote_setup(&v), false),
                            Err(e) => (e, true),
                        });
                    }),
                ),
            ))
            .child(self.row(
                txt::remote_devices_header(),
                txt::desc_remote_devices(),
                self.button(
                    "remote-devices",
                    txt::button_show(),
                    BtnKind::Normal,
                    cx.listener(|this, _, _, cx| {
                        let result = this.dispatch(
                            Request::RemoteDevices {
                                action: "list".into(),
                                device_id: None,
                            },
                            cx,
                        );
                        this.message = Some(match result {
                            Ok(v) => (summarize_devices(&v), false),
                            Err(e) => (e, true),
                        });
                    }),
                ),
            ))
    }

    // --- 高度タブ ---

    fn render_advanced_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let settings_path = settings::settings_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let data_dir = tako_core::paths::data_dir();
        let config_path = data_dir
            .as_ref()
            .map(|d| d.join("config.yaml").display().to_string())
            .unwrap_or_default();
        let profiles_path = data_dir
            .as_ref()
            .map(|d| d.join("profiles").display().to_string())
            .unwrap_or_default();
        let projects_path = data_dir
            .as_ref()
            .map(|d| d.join("projects.yaml").display().to_string())
            .unwrap_or_default();

        let editing = self.editing(&EditField::AdvancedJson);
        let file_text =
            serde_json::to_string_pretty(&self.settings).unwrap_or_else(|_| "{}".to_string());
        let buffer = editing
            .map(|e| e.text.clone())
            .unwrap_or_else(|| file_text.clone());
        let cursor = editing.map(|e| e.cursor).unwrap_or(0);

        let editor_body = if editing.is_some() {
            let (before, after) = buffer.split_at(cursor);
            div()
                .flex()
                .flex_wrap()
                .text_color(to_hsla(theme.foreground))
                .text_size(px(12.))
                .child(SharedString::from(before.to_string()))
                .child(
                    div()
                        .w(px(1.5))
                        .h(px(14.))
                        .flex_none()
                        .bg(to_hsla(theme.accent)),
                )
                .child(SharedString::from(after.to_string()))
        } else {
            div()
                .text_color(to_hsla(theme.foreground))
                .text_size(px(12.))
                .child(buffer.clone())
        };

        let buffer_for_click = file_text.clone();
        let editor = div()
            .id("adv-editor")
            .w_full()
            .min_h(px(220.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .rounded(px(5.))
            .bg(to_hsla(theme.crust))
            .border_1()
            .border_color(if editing.is_some() {
                to_hsla(theme.accent)
            } else {
                to_hsla(theme.border_default)
            })
            .p_2()
            .cursor(CursorStyle::IBeam)
            .child(editor_body)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.editing(&EditField::AdvancedJson).is_none() {
                    this.start_edit(
                        EditField::AdvancedJson,
                        buffer_for_click.clone(),
                        window,
                        cx,
                    );
                }
            }));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.section(txt::advanced_editor_header()))
            .child(
                div()
                    .pb_1()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(settings_path.clone()),
            )
            .child(editor)
            .child(
                div()
                    .pt_1()
                    .text_color(to_hsla(theme.text_faint))
                    .text_size(px(11.))
                    .child(txt::advanced_edit_help()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .child(self.button(
                        "adv-save",
                        txt::advanced_save(),
                        BtnKind::Primary,
                        cx.listener(|this, _, _, cx| {
                            if this.editing(&EditField::AdvancedJson).is_some() {
                                this.commit_edit(cx);
                            } else {
                                this.message =
                                    Some((txt::msg_nothing_to_save().to_string(), false));
                            }
                        }),
                    ))
                    .child(self.button(
                        "adv-reload",
                        txt::advanced_reload(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.edit = None;
                            this.settings = settings::load();
                            this.message = Some((txt::msg_reloaded().to_string(), false));
                            cx.notify();
                        }),
                    ))
                    .child(self.button(
                        "adv-reveal",
                        txt::advanced_open_finder(),
                        BtnKind::Normal,
                        |_, _, _| {
                            if let Some(path) = settings::settings_path() {
                                let _ = std::process::Command::new("open")
                                    .arg("-R")
                                    .arg(&path)
                                    .spawn();
                            }
                        },
                    ))
                    .child(self.button(
                        "adv-open-editor",
                        txt::advanced_open_editor(),
                        BtnKind::Normal,
                        |_, _, _| {
                            if let Some(path) = settings::settings_path() {
                                let _ = std::process::Command::new("open")
                                    .arg("-t")
                                    .arg(&path)
                                    .spawn();
                            }
                        },
                    )),
            )
            .child(
                div()
                    .pt_4()
                    .border_t_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .child(self.section(txt::advanced_related_header())),
            )
            .child(self.file_path_row("config.yaml", &config_path))
            .child(self.file_path_row("profiles/", &profiles_path))
            .child(self.file_path_row("projects.yaml", &projects_path))
    }

    fn file_path_row(&self, label: &str, path: &str) -> Div {
        let theme = self.theme();
        div()
            .flex()
            .items_center()
            .gap_2()
            .py(px(2.))
            .child(
                div()
                    .w(px(110.))
                    .flex_none()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(12.))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(path.to_string()),
            )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Normal,
    Danger,
    Disabled,
}

fn theme_request(
    action: &str,
    key: Option<String>,
    value: Option<String>,
    name: Option<String>,
) -> Request {
    Request::Theme {
        action: Some(action.to_string()),
        mode: None,
        target: None,
        key,
        value,
        name,
        font_family: None,
        font_size: None,
    }
}

fn font_request(family: Option<String>, size: Option<f32>) -> Request {
    Request::Theme {
        action: Some("set-font".into()),
        mode: None,
        target: None,
        key: None,
        value: None,
        name: None,
        font_family: family,
        font_size: size,
    }
}

fn logs_set(max_mb: Option<u64>, total_max_mb: Option<u64>) -> Request {
    Request::Logs {
        action: "set".into(),
        enabled: None,
        max_mb,
        total_max_mb,
        pane: None,
        session_id: None,
        lines: None,
    }
}

/// remote setup check の応答（{ready, ts_net_url, items:[{item,status}]}）を 1 行にまとめる
fn summarize_remote_setup(v: &serde_json::Value) -> String {
    let ready = v.get("ready").and_then(|x| x.as_bool()).unwrap_or(false);
    let pending: Vec<String> = v
        .get("items")
        .and_then(|x| x.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|i| i.get("status").and_then(|s| s.as_str()) != Some("ok"))
                .filter_map(|i| i.get("item").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let head = format!(
        "{}: {}",
        txt::remote_setup_header(),
        if ready {
            txt::remote_setup_ready()
        } else {
            txt::remote_setup_not_ready()
        }
    );
    if pending.is_empty() {
        head
    } else {
        format!("{head}（{}）", pending.join(", "))
    }
}

fn summarize_devices(v: &serde_json::Value) -> String {
    let count = v
        .get("devices")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!("{}: {count}", txt::remote_devices_header())
}

fn to_hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn タブ名の往復変換ができる() {
        for tab in SettingsTab::ALL {
            let s = format!("{tab:?}").to_lowercase();
            assert_eq!(SettingsTab::from_str(&s), Some(*tab), "tab={tab:?}");
        }
        assert_eq!(SettingsTab::from_str("nope"), None);
    }

    #[test]
    fn 編集フィールドのスラグが一意になる() {
        let fields = [
            EditField::ColorHex("accent".into()),
            EditField::ColorHex("red".into()),
            EditField::FontFamily,
            EditField::FontSize,
            EditField::PresetName,
            EditField::RunnerNewExt,
            EditField::RunnerNewCmd,
            EditField::RunnerCmd("py".into()),
            EditField::PreviewCacheMb,
            EditField::PaneLogMaxMb,
            EditField::PaneLogTotalMaxMb,
            EditField::AdvancedJson,
        ];
        let mut slugs: Vec<String> = fields.iter().map(|f| f.slug()).collect();
        slugs.sort();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(before, slugs.len(), "slug が重複している: {slugs:?}");
    }

    #[test]
    fn 複数行フィールドは高度タブのJSONだけ() {
        assert!(EditField::AdvancedJson.multiline());
        assert!(!EditField::FontFamily.multiline());
        assert!(!EditField::ColorHex("accent".into()).multiline());
    }
}

//! 設定画面（Issue #459）— 独立 GPUI ウィンドウ

use gpui::prelude::FluentBuilder;
use gpui::*;
use tako_control::settings;
use tako_core::theme::{Rgb, Theme, ThemeMode};

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

pub struct SettingsWindow {
    tako_app: WeakEntity<TakoApp>,
    tab: SettingsTab,
    settings: settings::Settings,
    expanded_categories: Vec<bool>,
    // M4: Code Runner
    new_ext: String,
    new_cmd: String,
    // M6: Advanced
    advanced_buffer: String,
    advanced_error: Option<String>,
    advanced_saved: bool,
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

        if let Some(app) = tako_app.upgrade() {
            cx.observe(&app, |this: &mut Self, _app, cx| {
                this.settings = settings::load();
                cx.notify();
            })
            .detach();
        }

        Self {
            tako_app,
            tab: tab.unwrap_or(SettingsTab::General),
            settings,
            expanded_categories: expanded,
            new_ext: String::new(),
            new_cmd: String::new(),
            advanced_buffer: String::new(),
            advanced_error: None,
            advanced_saved: false,
        }
    }

    fn dispatch(&self, request: tako_control::protocol::Request, cx: &mut Context<Self>) {
        if let Some(app) = self.tako_app.upgrade() {
            app.update(cx, |app, _cx| {
                let _ = tako_control::dispatch::dispatch(
                    app,
                    request,
                    tako_core::pane::PaneOrigin::User,
                );
            });
        }
    }

    fn dispatch_query(
        &self,
        request: tako_control::protocol::Request,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        self.tako_app.upgrade().and_then(|app| {
            app.update(cx, |app, _cx| {
                tako_control::dispatch::dispatch(app, request, tako_core::pane::PaneOrigin::User)
                    .ok()
            })
        })
    }

    fn theme(&self) -> Theme {
        self.settings.resolve_theme().0
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        div()
            .flex()
            .size_full()
            .bg(to_hsla(theme.surface_0))
            .text_color(to_hsla(theme.foreground))
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
            .w(px(180.))
            .h_full()
            .bg(to_hsla(theme.mantle))
            .border_r_1()
            .border_color(to_hsla(theme.border_subtle))
            .py_2()
            .children(SettingsTab::ALL.iter().map(|&tab| {
                let is_active = tab == current;
                let bg = if is_active {
                    to_hsla(theme.surface_highlight)
                } else {
                    transparent_black()
                };
                let fg = if is_active {
                    to_hsla(theme.foreground)
                } else {
                    to_hsla(theme.text_muted)
                };
                div()
                    .id(SharedString::from(format!("tab-{:?}", tab)))
                    .px_3()
                    .py(px(6.))
                    .mx_2()
                    .rounded(px(6.))
                    .bg(bg)
                    .text_color(fg)
                    .text_size(px(13.))
                    .cursor_pointer()
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _, _, _cx| {
                        this.tab = tab;
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
            .id("settings-content")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .bg(to_hsla(theme.surface_0))
            .p_4()
            .child(content)
    }

    // --- M2: 一般タブ ---

    fn render_general_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();
        let s = &self.settings;

        let lang_val = s.language.clone();
        let auto_rename = s.auto_rename;
        let port_detect = s.port_detect;
        let persist = s.tmux_persist;
        let telemetry = s.telemetry;
        let reload = s.preview_live_reload;
        let pane_logs = s.pane_logs;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(setting_row(
                txt::label_language(),
                &lang_val,
                &theme,
                cx.listener(|this, _, _, cx| {
                    let opts = ["system", "ja", "en"];
                    let cur = &this.settings.language;
                    let idx = opts
                        .iter()
                        .position(|o| *o == cur)
                        .map(|i| (i + 1) % opts.len())
                        .unwrap_or(0);
                    this.dispatch(
                        Request::Lang {
                            action: Some("set".into()),
                            value: Some(opts[idx].to_string()),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_auto_rename(),
                auto_rename,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.dispatch(
                        Request::AutoRename {
                            enabled: Some(!auto_rename),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_port_detect(),
                port_detect,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.dispatch(
                        Request::PortDetect {
                            enabled: Some(!port_detect),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_persist(),
                persist,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.dispatch(
                        Request::Persist {
                            enabled: Some(!persist),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_telemetry(),
                telemetry,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    let action = if telemetry { "off" } else { "on" };
                    this.dispatch(
                        Request::Telemetry {
                            action: Some(action.into()),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_preview_reload(),
                reload,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.dispatch(
                        Request::PreviewReload {
                            enabled: Some(!reload),
                        },
                        cx,
                    );
                }),
            ))
            .child(toggle_row(
                txt::label_pane_logs(),
                pane_logs,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.dispatch(
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
            ))
    }

    // --- M3: 外観タブ ---

    fn render_appearance_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();
        let (resolved_theme, _) = self.settings.resolve_theme();

        let is_dark = theme.mode == ThemeMode::Dark;
        let theme_segment = div()
            .flex()
            .items_center()
            .gap_2()
            .py(px(4.))
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(13.))
                    .child(txt::label_theme()),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(theme_btn(
                        "Dark",
                        is_dark,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.dispatch(theme_set("dark"), cx);
                        }),
                    ))
                    .child(theme_btn(
                        "Light",
                        !is_dark,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.dispatch(theme_set("light"), cx);
                        }),
                    )),
            );

        let mut color_sections = Vec::new();
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
            let arrow = if expanded { "v" } else { ">" };
            let header = div()
                .id(SharedString::from(format!("cat-{cat_idx}")))
                .flex()
                .items_center()
                .gap_2()
                .py(px(4.))
                .px_1()
                .cursor_pointer()
                .child(
                    div()
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(12.))
                        .child(arrow),
                )
                .child(
                    div()
                        .text_color(to_hsla(theme.foreground))
                        .text_size(px(13.))
                        .child(format!("{cat_label} ({count})")),
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if let Some(v) = this.expanded_categories.get_mut(cat_idx) {
                        *v = !*v;
                    }
                }));

            let mut section = div().flex().flex_col().child(header);
            if expanded {
                let keys = &Theme::COLOR_KEYS[key_offset..key_offset + count];
                for &key in keys {
                    let color = resolved_theme.color(key).unwrap_or(Rgb::new(0, 0, 0));
                    let hex = color.to_hex();
                    let key_owned = key.to_string();
                    section = section.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .py(px(2.))
                            .pl_4()
                            .child(
                                div()
                                    .w(px(16.))
                                    .h(px(16.))
                                    .rounded(px(3.))
                                    .bg(to_hsla(color))
                                    .border_1()
                                    .border_color(to_hsla(theme.border_subtle)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(to_hsla(theme.foreground))
                                    .text_size(px(12.))
                                    .child(key.to_string()),
                            )
                            .child(
                                div()
                                    .text_color(to_hsla(theme.text_muted))
                                    .text_size(px(11.))
                                    .child(hex),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("rst-{key}")))
                                    .px_1()
                                    .text_color(to_hsla(theme.text_faint))
                                    .text_size(px(11.))
                                    .cursor_pointer()
                                    .child(txt::button_reset())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dispatch(
                                            Request::Theme {
                                                action: Some("reset-color".into()),
                                                mode: None,
                                                target: None,
                                                key: Some(key_owned.clone()),
                                                value: None,
                                                name: None,
                                                font_family: None,
                                                font_size: None,
                                            },
                                            cx,
                                        );
                                    })),
                            ),
                    );
                }
            }
            color_sections.push(section);
            key_offset += count;
        }

        let reset_all = div().py_2().child(
            div()
                .id("reset-all-colors")
                .px_2()
                .py(px(4.))
                .rounded(px(4.))
                .bg(to_hsla(theme.danger_surface))
                .text_color(to_hsla(theme.red))
                .text_size(px(12.))
                .cursor_pointer()
                .child(txt::button_reset_all())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::Theme {
                            action: Some("reset-colors".into()),
                            mode: None,
                            target: None,
                            key: None,
                            value: None,
                            name: None,
                            font_family: None,
                            font_size: None,
                        },
                        cx,
                    );
                })),
        );

        let preset_label = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(13.))
                    .child(txt::label_preset()),
            )
            .child(
                div()
                    .px_2()
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(to_hsla(theme.chip_surface))
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(12.))
                    .child(self.settings.theme.clone()),
            );

        let font_label = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(13.))
                    .child(txt::label_font()),
            )
            .child(
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(12.))
                    .child(format!(
                        "{} / {}pt",
                        self.settings.font_family.as_deref().unwrap_or("Menlo"),
                        self.settings.font_size.unwrap_or(13.0)
                    )),
            );

        let mut content = div().flex().flex_col().gap_2();
        content = content.child(theme_segment);
        content = content.child(
            div()
                .text_color(to_hsla(theme.foreground))
                .text_size(px(13.))
                .child(txt::label_color_settings()),
        );
        for section in color_sections {
            content = content.child(section);
        }
        content = content
            .child(reset_all)
            .child(preset_label)
            .child(font_label);
        content
    }

    // --- M4: Code Runner タブ ---

    fn render_runner_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();

        let merged = tako_core::merged_defaults(&self.settings.runner_defaults);

        let mut table = div().flex().flex_col().gap_1();

        // ヘッダ行
        table = table.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py(px(4.))
                .child(
                    div()
                        .w(px(60.))
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_ext()),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_command()),
                )
                .child(
                    div()
                        .w(px(60.))
                        .text_color(to_hsla(theme.text_secondary))
                        .text_size(px(11.))
                        .child(txt::runner_col_source()),
                )
                .child(div().w(px(50.))),
        );

        // データ行
        for (ext, cmd) in &merged {
            let is_user = self.settings.runner_defaults.contains_key(ext);
            let source_label = if is_user {
                txt::runner_source_user()
            } else {
                txt::runner_source_builtin()
            };
            let source_color = if is_user {
                to_hsla(theme.accent)
            } else {
                to_hsla(theme.text_muted)
            };
            let ext_owned = ext.clone();
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(2.))
                    .child(
                        div()
                            .w(px(60.))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .child(ext.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(to_hsla(theme.text_muted))
                            .text_size(px(12.))
                            .overflow_x_hidden()
                            .child(cmd.clone()),
                    )
                    .child(
                        div()
                            .w(px(60.))
                            .text_color(source_color)
                            .text_size(px(11.))
                            .child(source_label),
                    )
                    .child({
                        let action_div = div()
                            .id(SharedString::from(format!("act-rd-{ext}")))
                            .w(px(50.));
                        if is_user {
                            action_div
                                .text_color(to_hsla(theme.text_faint))
                                .text_size(px(11.))
                                .cursor_pointer()
                                .child(txt::button_reset())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.dispatch(
                                        Request::RunnerDefaults {
                                            ext: Some(ext_owned.clone()),
                                            command: None,
                                            remove: true,
                                        },
                                        cx,
                                    );
                                }))
                        } else {
                            action_div
                        }
                    }),
            );
        }

        // 新規追加セクション
        let new_ext = self.new_ext.clone();
        let new_cmd = self.new_cmd.clone();
        let can_add = !new_ext.is_empty() && !new_cmd.is_empty();
        let add_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(13.))
                    .child(txt::runner_add_header()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("runner-new-ext")
                            .w(px(60.))
                            .px_1()
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(to_hsla(theme.chip_surface))
                            .border_1()
                            .border_color(to_hsla(theme.border_subtle))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child(if new_ext.is_empty() {
                                txt::runner_col_ext().to_string()
                            } else {
                                new_ext.clone()
                            })
                            .on_click(cx.listener(|this, _, _, _cx| {
                                this.new_ext = String::new();
                            })),
                    )
                    .child(
                        div()
                            .id("runner-new-cmd")
                            .flex_1()
                            .px_1()
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(to_hsla(theme.chip_surface))
                            .border_1()
                            .border_color(to_hsla(theme.border_subtle))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child(if new_cmd.is_empty() {
                                txt::runner_col_command().to_string()
                            } else {
                                new_cmd.clone()
                            })
                            .on_click(cx.listener(|this, _, _, _cx| {
                                this.new_cmd = String::new();
                            })),
                    )
                    .child({
                        let btn = div()
                            .id("runner-add-btn")
                            .px_2()
                            .py(px(4.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .child(txt::runner_add_btn());
                        if can_add {
                            let ext_to_add = new_ext.clone();
                            let cmd_to_add = new_cmd.clone();
                            btn.bg(to_hsla(theme.accent))
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.dispatch(
                                        Request::RunnerDefaults {
                                            ext: Some(ext_to_add.clone()),
                                            command: Some(cmd_to_add.clone()),
                                            remove: false,
                                        },
                                        cx,
                                    );
                                    this.new_ext.clear();
                                    this.new_cmd.clear();
                                }))
                        } else {
                            btn.bg(to_hsla(theme.border_default))
                                .text_color(to_hsla(theme.text_faint))
                        }
                    }),
            );

        // 変数リファレンス
        let help = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(13.))
                    .child(txt::runner_help_header()),
            )
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
                                .w(px(100.))
                                .text_color(to_hsla(theme.accent))
                                .text_size(px(12.))
                                .child(var),
                        )
                        .child(
                            div()
                                .flex_1()
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

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(14.))
                    .child(txt::runner_header()),
            )
            .child(table)
            .child(add_section)
            .child(help)
    }

    // --- M5: セットアップタブ ---

    fn render_setup_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();

        // エージェント CLI 検出
        let agents_section = {
            let cli_names = ["claude", "codex", "agy"];
            let mut rows = div().flex().flex_col().gap_1();
            for cli in cli_names {
                let found = std::process::Command::new("which")
                    .arg(cli)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                let status_text = if found {
                    txt::setup_installed()
                } else {
                    txt::setup_not_installed()
                };
                let status_color = if found {
                    to_hsla(theme.green)
                } else {
                    to_hsla(theme.text_faint)
                };
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py(px(2.))
                        .child(
                            div()
                                .w(px(80.))
                                .text_color(to_hsla(theme.foreground))
                                .text_size(px(12.))
                                .child(cli),
                        )
                        .child(
                            div()
                                .text_color(status_color)
                                .text_size(px(12.))
                                .child(status_text),
                        ),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section_header(txt::setup_agents_header(), &theme))
                .child(rows)
        };

        // FDA 状態
        let fda_section = {
            let fda_result = self.dispatch_query(
                Request::Fda {
                    action: Some("status".into()),
                },
                // dispatch_query の引数型を考慮
                // HACK: cx を一旦 workaround — 後述 immutable borrow 問題
                // TODO: この関数は &self なので dispatch_query も &self で OK
                cx,
            );
            let fda_status = fda_result
                .as_ref()
                .and_then(|v| v.get("full_disk_access"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let fda_color = if fda_status == "granted" {
                to_hsla(theme.green)
            } else {
                to_hsla(theme.yellow)
            };

            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section_header(txt::setup_fda_header(), &theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_color(fda_color)
                                .text_size(px(12.))
                                .child(fda_status.to_string()),
                        )
                        .child(
                            div()
                                .id("fda-open")
                                .px_2()
                                .py(px(4.))
                                .rounded(px(4.))
                                .bg(to_hsla(theme.chip_surface))
                                .text_color(to_hsla(theme.foreground))
                                .text_size(px(12.))
                                .cursor_pointer()
                                .child(txt::setup_fda_open())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dispatch(
                                        Request::Fda {
                                            action: Some("open".into()),
                                        },
                                        cx,
                                    );
                                })),
                        ),
                )
        };

        // MCP 登録
        let mcp_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::setup_mcp_header(), &theme))
            .child(
                div()
                    .id("setup-mcp-register")
                    .px_2()
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(to_hsla(theme.chip_surface))
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(txt::setup_mcp_register())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            Request::SetupMcp {
                                scope: None,
                                pane: None,
                            },
                            cx,
                        );
                    })),
            );

        // ルール同期
        let rules_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::setup_rules_header(), &theme))
            .child(
                div()
                    .id("setup-rules-sync")
                    .px_2()
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(to_hsla(theme.chip_surface))
                    .text_color(to_hsla(theme.foreground))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(txt::setup_rules_sync())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            Request::AgentsSyncRules {
                                action: Some("sync".into()),
                                source: None,
                                targets: None,
                            },
                            cx,
                        );
                    })),
            );

        // tako setup 実行ボタン
        let run_setup = div().py_2().child(
            div()
                .id("setup-run")
                .px_2()
                .py(px(4.))
                .rounded(px(4.))
                .bg(to_hsla(theme.accent))
                .text_color(gpui::rgb(0xffffff))
                .text_size(px(12.))
                .cursor_pointer()
                .child(txt::setup_run_btn())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::RunInteractive {
                            command: "tako setup".into(),
                            pane: None,
                            tab: None,
                            input_hint: None,
                            direction: None,
                            ratio: None,
                            auto_close: None,
                        },
                        cx,
                    );
                })),
        );

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(agents_section)
            .child(fda_section)
            .child(mcp_section)
            .child(rules_section)
            .child(run_setup)
    }

    // --- M6: スリープ防止タブ ---

    fn render_sleep_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();
        let s = &self.settings;

        let mode_str = s.sleep_guard_mode.as_str();
        let power_str = s.sleep_guard_power.as_str();
        let lid_str = s.lid_sleep_mode.as_str();

        let mode_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::sleep_mode_header(), &theme))
            .child(render_radio_option(
                "sg-off",
                txt::sleep_mode_off(),
                mode_str == "off",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: Some("off".into()),
                            power_condition: None,
                            lid_sleep_mode: None,
                        },
                        cx,
                    );
                }),
            ))
            .child(render_radio_option(
                "sg-on",
                txt::sleep_mode_on(),
                mode_str == "on",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: Some("on".into()),
                            power_condition: None,
                            lid_sleep_mode: None,
                        },
                        cx,
                    );
                }),
            ))
            .child(render_radio_option(
                "sg-agents",
                txt::sleep_mode_agents(),
                mode_str == "while-agents-running",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: Some("while-agents-running".into()),
                            power_condition: None,
                            lid_sleep_mode: None,
                        },
                        cx,
                    );
                }),
            ));

        let power_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::sleep_power_header(), &theme))
            .child(render_radio_option(
                "sp-ac",
                txt::sleep_power_ac(),
                power_str == "ac-only",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: None,
                            power_condition: Some("ac-only".into()),
                            lid_sleep_mode: None,
                        },
                        cx,
                    );
                }),
            ))
            .child(render_radio_option(
                "sp-always",
                txt::sleep_power_always(),
                power_str == "always",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: None,
                            power_condition: Some("always".into()),
                            lid_sleep_mode: None,
                        },
                        cx,
                    );
                }),
            ));

        let lid_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::sleep_lid_header(), &theme))
            .child(render_radio_option(
                "lid-off",
                txt::sleep_mode_off(),
                lid_str == "off",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: None,
                            power_condition: None,
                            lid_sleep_mode: Some("off".into()),
                        },
                        cx,
                    );
                }),
            ))
            .child(render_radio_option(
                "lid-agents",
                txt::sleep_mode_agents(),
                lid_str == "while-agents-running",
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.dispatch(
                        Request::SleepGuard {
                            action: Some("set".into()),
                            mode: None,
                            power_condition: None,
                            lid_sleep_mode: Some("while-agents-running".into()),
                        },
                        cx,
                    );
                }),
            ));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(mode_section)
            .child(power_section)
            .child(lid_section)
    }

    // --- M6: リモートタブ ---

    fn render_remote_tab(&self, cx: &mut Context<Self>) -> Div {
        use tako_control::protocol::Request;
        let theme = self.theme();

        let status_result = self.dispatch_query(Request::RemoteStatus, cx);
        let is_running = status_result
            .as_ref()
            .and_then(|v| v.get("running"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let url = status_result
            .as_ref()
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let status_color = if is_running {
            to_hsla(theme.green)
        } else {
            to_hsla(theme.text_faint)
        };
        let status_text = if is_running {
            txt::remote_status_running()
        } else {
            txt::remote_status_stopped()
        };

        let daemon_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::remote_daemon_header(), &theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_color(status_color)
                            .text_size(px(12.))
                            .child(status_text),
                    )
                    .when(!url.is_empty(), |d| {
                        d.child(
                            div()
                                .text_color(to_hsla(theme.text_muted))
                                .text_size(px(11.))
                                .child(url),
                        )
                    }),
            )
            .child(div().flex().gap_2().child(if is_running {
                div()
                    .id("remote-stop")
                    .px_2()
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(to_hsla(theme.danger_surface))
                    .text_color(to_hsla(theme.red))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(txt::remote_stop())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(Request::RemoteStop { force: false }, cx);
                    }))
            } else {
                div()
                    .id("remote-start")
                    .px_2()
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(to_hsla(theme.accent))
                    .text_color(gpui::rgb(0xffffff))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(txt::remote_start())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(Request::RemoteStart {}, cx);
                    }))
            }));

        div().flex().flex_col().gap_3().child(daemon_section)
    }

    // --- M6: 高度タブ ---

    fn render_advanced_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let settings_path = settings::settings_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let config_path = tako_core::paths::data_dir()
            .map(|d| d.join("config.yaml").display().to_string())
            .unwrap_or_default();
        let profiles_path = tako_core::paths::data_dir()
            .map(|d| d.join("profiles").display().to_string())
            .unwrap_or_default();

        // エディタセクション
        let buffer_display = if self.advanced_buffer.is_empty() {
            serde_json::to_string_pretty(&self.settings).unwrap_or_default()
        } else {
            self.advanced_buffer.clone()
        };

        let editor_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::advanced_editor_header(), &theme))
            .child(
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(settings_path.clone()),
            )
            .child(
                div()
                    .id("adv-editor-area")
                    .w_full()
                    .min_h(px(200.))
                    .max_h(px(400.))
                    .overflow_y_scroll()
                    .rounded(px(4.))
                    .bg(to_hsla(theme.crust))
                    .border_1()
                    .border_color(to_hsla(theme.border_subtle))
                    .p_2()
                    .child(
                        div()
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .child(buffer_display),
                    ),
            )
            .when(self.advanced_error.is_some(), |d| {
                d.child(
                    div()
                        .text_color(to_hsla(theme.red))
                        .text_size(px(12.))
                        .child(format!(
                            "{}: {}",
                            txt::advanced_parse_error(),
                            self.advanced_error.as_deref().unwrap_or("")
                        )),
                )
            })
            .when(self.advanced_saved, |d| {
                d.child(
                    div()
                        .text_color(to_hsla(theme.green))
                        .text_size(px(12.))
                        .child(txt::advanced_saved()),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("adv-reload")
                            .px_2()
                            .py(px(4.))
                            .rounded(px(4.))
                            .bg(to_hsla(theme.chip_surface))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child(txt::advanced_reload())
                            .on_click(cx.listener(|this, _, _, _cx| {
                                this.settings = settings::load();
                                this.advanced_buffer.clear();
                                this.advanced_error = None;
                                this.advanced_saved = false;
                            })),
                    )
                    .child(
                        div()
                            .id("adv-reveal")
                            .px_2()
                            .py(px(4.))
                            .rounded(px(4.))
                            .bg(to_hsla(theme.chip_surface))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child(txt::advanced_open_finder())
                            .on_click(cx.listener(move |_this, _, _, _cx| {
                                if let Some(path) = settings::settings_path() {
                                    let _ = std::process::Command::new("open")
                                        .arg("-R")
                                        .arg(&path)
                                        .spawn();
                                }
                            })),
                    ),
            );

        // 関連ファイルセクション
        let related = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_header(txt::advanced_related_header(), &theme))
            .child(file_path_row("config.yaml", &config_path, &theme))
            .child(file_path_row("profiles/", &profiles_path, &theme));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(editor_section)
            .child(related)
    }
}

// --- ヘルパー ---

fn section_header(text: &str, theme: &Theme) -> Div {
    div()
        .text_color(to_hsla(theme.foreground))
        .text_size(px(13.))
        .pb(px(2.))
        .child(text.to_string())
}

fn file_path_row(label: &str, path: &str, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .pl_2()
        .child(
            div()
                .w(px(100.))
                .text_color(to_hsla(theme.foreground))
                .text_size(px(12.))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .text_color(to_hsla(theme.text_muted))
                .text_size(px(11.))
                .child(path.to_string()),
        )
}

fn render_radio_option(
    id: &str,
    label: &str,
    is_active: bool,
    theme: &Theme,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let dot_color = if is_active {
        to_hsla(theme.accent)
    } else {
        to_hsla(theme.border_default)
    };
    let text_color = if is_active {
        to_hsla(theme.foreground)
    } else {
        to_hsla(theme.text_muted)
    };
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap_2()
        .py(px(2.))
        .pl_2()
        .cursor_pointer()
        .child(
            div()
                .w(px(12.))
                .h(px(12.))
                .rounded(px(6.))
                .border_1()
                .border_color(dot_color)
                .when(is_active, |d| {
                    d.child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .mt(px(2.))
                            .ml(px(2.))
                            .rounded(px(3.))
                            .bg(to_hsla(theme.accent)),
                    )
                }),
        )
        .child(
            div()
                .text_color(text_color)
                .text_size(px(12.))
                .child(label.to_string()),
        )
        .on_click(handler)
}

fn to_hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)).into()
}

fn toggle_row(
    label: &str,
    value: bool,
    theme: &Theme,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let indicator_bg = if value {
        to_hsla(theme.accent)
    } else {
        to_hsla(theme.border_default)
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .py(px(4.))
        .child(
            div()
                .text_color(to_hsla(theme.foreground))
                .text_size(px(13.))
                .child(label.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(format!("toggle-{label}")))
                .w(px(36.))
                .h(px(20.))
                .rounded(px(10.))
                .bg(indicator_bg)
                .cursor_pointer()
                .child(
                    div()
                        .w(px(16.))
                        .h(px(16.))
                        .mt(px(2.))
                        .ml(if value { px(18.) } else { px(2.) })
                        .rounded(px(8.))
                        .bg(gpui::rgb(0xffffff)),
                )
                .on_click(handler),
        )
}

fn setting_row(
    label: &str,
    current: &str,
    theme: &Theme,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .py(px(4.))
        .child(
            div()
                .text_color(to_hsla(theme.foreground))
                .text_size(px(13.))
                .child(label.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(format!("setting-{label}")))
                .px_2()
                .py(px(2.))
                .rounded(px(4.))
                .bg(to_hsla(theme.chip_surface))
                .border_1()
                .border_color(to_hsla(theme.border_subtle))
                .text_color(to_hsla(theme.foreground))
                .text_size(px(12.))
                .cursor_pointer()
                .child(current.to_string())
                .on_click(handler),
        )
}

fn theme_btn(
    label: &str,
    active: bool,
    theme: &Theme,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let bg = if active {
        to_hsla(theme.accent)
    } else {
        to_hsla(theme.chip_surface)
    };
    let fg = if active {
        gpui::rgb(0xffffff).into()
    } else {
        to_hsla(theme.text_muted)
    };
    div()
        .id(SharedString::from(format!("theme-{label}")))
        .px_3()
        .py(px(4.))
        .rounded(px(6.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .cursor_pointer()
        .child(label.to_string())
        .on_click(handler)
}

fn theme_set(mode: &str) -> tako_control::protocol::Request {
    tako_control::protocol::Request::Theme {
        action: Some("set".into()),
        mode: Some(mode.to_string()),
        target: None,
        key: None,
        value: None,
        name: None,
        font_family: None,
        font_size: None,
    }
}

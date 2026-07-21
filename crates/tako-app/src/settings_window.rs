//! 設定画面（Issue #459）— 独立 GPUI ウィンドウ

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
            _ => self.render_placeholder_tab(),
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

    fn render_placeholder_tab(&self) -> Div {
        let theme = self.theme();
        div()
            .text_color(to_hsla(theme.text_muted))
            .text_size(px(14.))
            .child(txt::placeholder_coming_soon())
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
}

// --- ヘルパー ---

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

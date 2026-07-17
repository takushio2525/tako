use gpui::{
    div, point, prelude::*, px, relative, svg, BoxShadow, Context, FontWeight, MouseButton,
    SharedString,
};
use tako_core::{CommandState, LimitService};

use super::*;
use crate::file_icons::ui_icon;

impl TakoApp {
    pub(crate) fn render_status_bar(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme.clone();
        let agents_dot =
            match CommandState::aggregate(self.terminals.values().map(|s| s.command_state())) {
                CommandState::Failed(_) => Some(theme.red),
                CommandState::Running => Some(theme.accent),
                _ => None,
            };
        let toggle = |id: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h_full()
                .px(px(11.0))
                .cursor_pointer()
                .text_size(px(11.5))
                .when(active, |d| {
                    d.text_color(hsla(theme.accent))
                        .bg(rgba_alpha(theme.accent, 0.1))
                })
                .when(!active, |d| d.text_color(hsla(theme.text_tertiary)))
                .hover(|d| d.bg(rgba(theme.surface_hover)))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
        };
        // master エントリ（カンプ: master アイコン + N workers + 失敗ドット）
        let fleet_label = {
            let has_master = self
                .workspace
                .tabs()
                .iter()
                .flat_map(|tab| tab.tree().panes())
                .any(|p| {
                    p.role().is_some_and(|r| {
                        r == "orchestrator-master" || r.starts_with("orchestrator-master:")
                    })
                });
            if has_master {
                let workers: Vec<CommandState> = self
                    .workspace
                    .tabs()
                    .iter()
                    .flat_map(|tab| tab.tree().panes())
                    .filter(|p| {
                        p.role()
                            .is_some_and(|r| r.starts_with("orchestrator-worker:"))
                    })
                    .map(|p| {
                        self.terminals
                            .get(&p.id())
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown)
                    })
                    .collect();
                let failed = workers
                    .iter()
                    .filter(|s| matches!(s, CommandState::Failed(_)))
                    .count();
                Some((workers.len(), failed))
            } else {
                None
            }
        };
        let ctx_pct = self.agent_metrics.ctx_percent.unwrap_or(0);
        let ctx_bar_color = if ctx_pct >= 90 {
            theme.red
        } else if ctx_pct >= 70 {
            theme.yellow
        } else {
            theme.accent
        };
        let ctx_fill_frac = ctx_pct as f32 / 100.0;
        let ctx_detail = self.agent_metrics.ctx_detail.clone();
        let usage_text = self.agent_metrics.usage_text.clone();
        let limit_5h = self.agent_metrics.limit_5h;
        let limit_week = self.agent_metrics.limit_week;
        let selected_limit_service = self.limit_service;
        let limit_menu_open = self.limit_service_menu_open;
        // リミット表示があるとき、usage_text が同じ「Nh NN%」なら重複表示を避ける
        let usage_text = usage_text.filter(|t| {
            limit_5h.is_none() || t.contains('$') || t.contains('k') || t.contains('K')
        });
        // リミットの色（カンプ: >=90 red / >=70 yellow / 通常 text_tertiary）
        let limit_color = |v: u32| {
            if v >= 90 {
                theme.red
            } else if v >= 70 {
                theme.yellow
            } else {
                theme.text_tertiary
            }
        };
        // フォーカスペインの cwd（カンプ: breadcrumb。クリックでコピー）
        let cwd_breadcrumb = self.active_tab_cwd().map(|p| {
            let full = p.display().to_string();
            let short = if let Ok(home) = std::env::var("HOME") {
                if let Ok(rel) = p.strip_prefix(&home) {
                    format!("~/{}", rel.display())
                } else {
                    full.clone()
                }
            } else {
                full.clone()
            };
            let (parent, leaf) = match short.rfind('/') {
                Some(i) if i + 1 < short.len() => {
                    (short[..=i].to_string(), short[i + 1..].to_string())
                }
                _ => (String::new(), short.clone()),
            };
            (parent, leaf, full)
        });
        // アクティブタブ名（カンプ: ctx メーターに表示）
        let active_tab_name = self.workspace.active_tab().title().to_string();
        let sparkline: Vec<f32> = {
            let max = self
                .usage_history
                .iter()
                .cloned()
                .fold(f32::MIN, f32::max)
                .max(1.0);
            self.usage_history.iter().map(|v| v / max).collect()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .overflow_hidden()
            .bg(rgba(theme.tab_bar_background))
            .border_t_1()
            .border_color(hsla(theme.border_subtle))
            .text_size(px(11.5))
            .child(
                toggle("statusbar-filetree", self.filetree.visible)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_filetree();
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(ui_icon::FOLDER)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(if self.filetree.visible {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.text_tertiary)
                            }),
                    )
                    .child("Files"),
            )
            .child({
                let bg_count = self.workspace.shelved_panes().len();
                let drawer_open = self.drawer_visible;
                toggle("statusbar-bg", drawer_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.drawer_visible = !this.drawer_visible;
                        cx.notify();
                    }))
                    .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                        this.background_tab(drag.tab, cx);
                    }))
                    .child(
                        svg()
                            .path(ui_icon::BG_DRAWER)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(if drawer_open {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.text_tertiary)
                            }),
                    )
                    .child("BG")
                    .when(bg_count > 0, |d| {
                        d.child(
                            div()
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(bg_count.to_string())),
                        )
                    })
            })
            .child({
                // Web ビュー dock（FR-3.8 / #155）: 開いたページの一覧・呼び出し・破棄
                let web_count = self.webviews.len();
                toggle("statusbar-web", self.webview_dock_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.webview_dock_open = !this.webview_dock_open;
                        // dock を開いたら URL 入力欄にフォーカス（#207）
                        this.webview_dock_url_focused = this.webview_dock_open;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(ui_icon::GLOBE)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(if self.webview_dock_open {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.text_tertiary)
                            }),
                    )
                    .when(web_count > 0, |d| {
                        d.child(
                            div()
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(web_count.to_string())),
                        )
                    })
            })
            // フォーカスペインの cwd breadcrumb（カンプ新設。クリックでコピー）
            .children(cwd_breadcrumb.map(|(parent, leaf, full)| {
                div()
                    .id("statusbar-cwd")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .h_full()
                    .px(px(12.0))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .font_family(theme.font_family.clone())
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
                    }))
                    .child(
                        svg()
                            .path(ui_icon::FOLDER)
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(truncate(&parent, 28))),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.foreground))
                            .child(SharedString::from(truncate(&leaf, 20))),
                    )
                    .child(
                        svg()
                            .path(ui_icon::COPY)
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(hsla(theme.text_faint)),
                    )
            }))
            // master エントリ（カンプ: master アイコン + N workers + 失敗ドット）
            .children(fleet_label.map(|(worker_count, failed)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .h_full()
                    .px(px(12.0))
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        svg()
                            .path(ui_icon::MASTER)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(hsla(theme.accent)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.foreground))
                            .child("master"),
                    )
                    .child(
                        div()
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(format!(
                                "\u{00B7} {worker_count} workers"
                            ))),
                    )
                    .when(failed > 0, |d| {
                        d.child(
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .ml(px(2.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(theme.red)),
                        )
                    })
            }))
            .when(self.sleep_guard_active || self.lid_sleep_disabled, |d| {
                let label = if self.lid_sleep_disabled {
                    if self.thermal_warning {
                        "awake+lid (!)".to_string()
                    } else {
                        "awake+lid".to_string()
                    }
                } else {
                    "awake".to_string()
                };
                let color = if self.thermal_warning {
                    theme.red
                } else {
                    theme.teal
                };
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .h_full()
                        .px(px(11.0))
                        .border_r_1()
                        .border_color(hsla(theme.border_subtle))
                        // coffee アイコン（#217 SVG。#220 の蓋閉じ・高温警告の色分けを維持）
                        .child(
                            svg()
                                .path(ui_icon::COFFEE)
                                .w(px(13.0))
                                .h(px(13.0))
                                .text_color(hsla(color)),
                        )
                        .child(
                            div()
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(label)),
                        ),
                )
            })
            .child(div().flex_grow(1.0))
            .children(self.render_update_banner(&theme, cx))
            // 利用リミットメーター（Issue #321: サービス切替ドロップダウン + 「7d」表記）
            .child({
                let has_data = selected_limit_service == LimitService::Claude
                    && (limit_5h.is_some() || limit_week.is_some());
                let meter = |label: &'static str, v: u32| {
                    let color = limit_color(v);
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(div().text_color(hsla(theme.text_muted)).child(label))
                        .child(
                            div()
                                .w(px(42.0))
                                .h(px(5.0))
                                .rounded(px(3.0))
                                .bg(rgba(theme.surface_highlight))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative((v as f32 / 100.0).min(1.0)))
                                        .bg(hsla(color)),
                                ),
                        )
                        .child(
                            div()
                                .font_family(theme.font_family.clone())
                                .text_size(px(10.5))
                                .text_color(hsla(color))
                                .child(SharedString::from(format!("{v}%"))),
                        )
                };
                let svc_label = selected_limit_service.as_str();
                let svc_color = match selected_limit_service {
                    LimitService::Claude => theme.accent,
                    LimitService::Codex => theme.teal,
                    LimitService::Agy => theme.yellow,
                };
                div()
                    .id("statusbar-limit")
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h_full()
                    .px(px(12.0))
                    .border_l_1()
                    .border_color(hsla(theme.border_subtle))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.limit_service_menu_open = !this.limit_service_menu_open;
                        cx.notify();
                    }))
                    // サービスラベル + ドット（視覚的区別）
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(hsla(svc_color)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(svc_color))
                                    .child(svc_label),
                            )
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::CHEVRON_DOWN)
                                    .w(px(9.0))
                                    .h(px(9.0))
                                    .text_color(hsla(theme.text_tertiary)),
                            ),
                    )
                    // メーター（データがあるときだけ）
                    .when(has_data, |d| {
                        d.children(limit_5h.map(|v| meter("5h", v)))
                            .children(limit_week.map(|v| meter("7d", v)))
                    })
                    // データがないとき（codex/agy、または claude でデータ未取得）
                    .when(!has_data, |d| {
                        d.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from("--")),
                        )
                    })
                    // ドロップダウンポップアップ
                    .when(limit_menu_open, |d| {
                        d.child(self.render_limit_service_menu(limit_5h, limit_week, cx))
                    })
            })
            // usage（カンプ: トレンドアイコン + スパークライン + tok + cost）
            .children(usage_text.map(|text| {
                let (tokens, cost) = if let Some(pos) = text.find('$') {
                    let tok_part = text[..pos].trim().trim_end_matches('·').trim();
                    let cost_part = text[pos..].trim();
                    (tok_part.to_string(), Some(cost_part.to_string()))
                } else {
                    (text.clone(), None)
                };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .h_full()
                    .px(px(12.0))
                    .border_l_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        svg()
                            .path(ui_icon::TREND)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(hsla(theme.teal)),
                    );
                // スパークライン（カンプ: 3px バー×5。履歴が 2 点以上あるときだけ）
                if sparkline.len() >= 2 {
                    row = row.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(1.5))
                            .h(px(10.0))
                            .children(sparkline.iter().enumerate().map(|(i, v)| {
                                let recent = i + 2 >= sparkline.len();
                                div()
                                    .w(px(3.0))
                                    .h(px((10.0 * v).max(2.0)))
                                    .rounded(px(1.0))
                                    .bg(hsla(if recent {
                                        theme.teal
                                    } else {
                                        theme.text_overlay
                                    }))
                            })),
                    );
                }
                if !tokens.is_empty() {
                    row = row.child(
                        div()
                            .font_family(theme.font_family.clone())
                            .text_color(hsla(theme.foreground))
                            .child(SharedString::from(tokens)),
                    );
                }
                if let Some(c) = cost {
                    row = row.child(
                        div()
                            .font_family(theme.font_family.clone())
                            .text_color(hsla(theme.teal))
                            .child(SharedString::from(c)),
                    );
                }
                row
            }))
            // ctx メーター（カンプ: タブ名 + 70/90% 目盛り線つきバー）
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h_full()
                    .px(px(12.0))
                    .border_l_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(div().text_color(hsla(theme.text_muted)).child("ctx"))
                    .child(
                        div()
                            .font_family(theme.font_family.clone())
                            .text_size(px(10.5))
                            .text_color(hsla(theme.accent))
                            .child(SharedString::from(truncate(&active_tab_name, 12))),
                    )
                    .child(
                        div()
                            .w(px(70.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgba(theme.surface_highlight))
                            .overflow_hidden()
                            .relative()
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(3.0))
                                    .w(relative(ctx_fill_frac))
                                    .bg(hsla(ctx_bar_color)),
                            )
                            // 70% / 90% の目盛り線（カンプ）
                            .child(
                                div()
                                    .absolute()
                                    .top(px(0.0))
                                    .bottom(px(0.0))
                                    .left(px(70.0 * 0.7))
                                    .w(px(1.0))
                                    .bg(hsla_alpha(theme.foreground, 0.25)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(0.0))
                                    .bottom(px(0.0))
                                    .left(px(70.0 * 0.9))
                                    .w(px(1.0))
                                    .bg(hsla_alpha(theme.foreground, 0.25)),
                            ),
                    )
                    .child(
                        div()
                            .font_family(theme.font_family.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(ctx_bar_color))
                            .child(SharedString::from(format!("{ctx_pct}%"))),
                    )
                    .children(ctx_detail.map(|detail| {
                        div()
                            .font_family(theme.font_family.clone())
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(detail))
                    })),
            )
            .child(
                toggle(
                    "statusbar-tmux",
                    self.panel_visible && self.panel_view == PanelView::Tmux,
                )
                .border_r_0()
                .border_l_1()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Tmux, cx);
                }))
                .children(agents_dot.map(|color| {
                    div()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(hsla(color))
                        .shadow(vec![BoxShadow {
                            color: hsla_alpha(color, 0.6),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(4.0),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                }))
                .child(
                    svg()
                        .path(ui_icon::FLEET)
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(
                            if self.panel_visible && self.panel_view == PanelView::Tmux {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.text_tertiary)
                            },
                        ),
                )
                .child("tmux"),
            )
            .child(
                toggle(
                    "statusbar-git",
                    self.panel_visible && self.panel_view == PanelView::Git,
                )
                .border_r_0()
                .border_l_1()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Git, cx);
                }))
                .child(
                    svg()
                        .path(ui_icon::GIT_BRANCH)
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(if self.panel_visible && self.panel_view == PanelView::Git {
                            hsla(theme.accent)
                        } else {
                            hsla(theme.text_tertiary)
                        }),
                )
                .child({
                    let branch = self
                        .sidebar_git
                        .as_ref()
                        .map(|g| truncate(&g.branch, 16))
                        .or_else(|| {
                            self.git_data
                                .as_ref()
                                .and_then(|d| d.branches.iter().find(|b| b.is_current))
                                .map(|b| truncate(&b.name, 16))
                        })
                        .unwrap_or_else(|| "git".into());
                    SharedString::from(branch)
                }),
            )
    }

    fn render_update_banner(
        &self,
        theme: &tako_core::Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = theme.clone();
        let pill = || {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h_full()
                .px_2()
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
        };
        match &self.update_state {
            super::update_checker::UpdateState::Available(info) => {
                let ver = info.version.clone();
                let method = super::update_checker::detect_install_method();
                let method_label = match method {
                    super::update_checker::InstallMethod::Homebrew => "brew",
                    super::update_checker::InstallMethod::Zip => "zip",
                    super::update_checker::InstallMethod::BrokenBrew => "zip (brew 破損)",
                };
                Some(
                    pill()
                        .id("update-banner")
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.accent))
                                .child(SharedString::from(format!(
                                    "v{ver} が利用可能（{method_label}）"
                                ))),
                        )
                        .child(
                            div()
                                .id("update-btn")
                                .cursor_pointer()
                                .px_1()
                                .rounded(px(3.0))
                                .bg(hsla(theme.accent))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.background))
                                .hover(|d| d.opacity(0.8))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_update_confirm(cx);
                                }))
                                .child("更新"),
                        )
                        .child(
                            div()
                                .id("update-dismiss")
                                .cursor_pointer()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_tertiary))
                                .hover(|d| d.text_color(hsla(theme.text_secondary)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_state =
                                        super::update_checker::UpdateState::Dismissed;
                                    cx.notify();
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                )
            }
            super::update_checker::UpdateState::ConfirmPending(info) => {
                let ver = info.version.clone();
                let method = super::update_checker::detect_install_method();
                let method_label = match method {
                    super::update_checker::InstallMethod::Homebrew => "Homebrew",
                    super::update_checker::InstallMethod::Zip
                    | super::update_checker::InstallMethod::BrokenBrew => "ZIP 差し替え",
                };
                Some(
                    pill()
                        .id("update-confirm")
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.yellow))
                                .child(SharedString::from(format!(
                                    "v{ver} に更新して再起動しますか？（{method_label}。実行中のプロセスは失われます）"
                                ))),
                        )
                        .child(
                            div()
                                .id("update-confirm-yes")
                                .cursor_pointer()
                                .px_1()
                                .rounded(px(3.0))
                                .bg(hsla(theme.accent))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.background))
                                .hover(|d| d.opacity(0.8))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_update(cx);
                                }))
                                .child("実行"),
                        )
                        .child(
                            div()
                                .id("update-confirm-no")
                                .cursor_pointer()
                                .px_1()
                                .rounded(px(3.0))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_secondary))
                                .hover(|d| d.text_color(hsla(theme.text_secondary)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_state =
                                        super::update_checker::UpdateState::Dismissed;
                                    cx.notify();
                                }))
                                .child("キャンセル"),
                        )
                        .into_any_element(),
                )
            }
            super::update_checker::UpdateState::Updating(msg) => Some(
                pill()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.yellow))
                            .child(SharedString::from(msg.clone())),
                    )
                    .into_any_element(),
            ),
            super::update_checker::UpdateState::Done(msg) => Some(
                pill()
                    .id("update-done")
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.green))
                            .child(SharedString::from(msg.clone())),
                    )
                    .into_any_element(),
            ),
            super::update_checker::UpdateState::Failed(msg) => Some(
                pill()
                    .id("update-failed")
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.red))
                            .child(SharedString::from(msg.clone())),
                    )
                    .child(
                        div()
                            .id("update-failed-dismiss")
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_tertiary))
                            .hover(|d| d.text_color(hsla(theme.text_secondary)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_state = super::update_checker::UpdateState::Dismissed;
                                cx.notify();
                            }))
                            .child("×"),
                    )
                    .into_any_element(),
            ),
            super::update_checker::UpdateState::BrewFailedFallback { brew_error, .. } => {
                let err_short = if brew_error.len() > 80 {
                    format!("{}…", &brew_error[..77])
                } else {
                    brew_error.clone()
                };
                Some(
                    pill()
                        .id("update-brew-fallback")
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.red))
                                .child(SharedString::from(format!("brew 更新失敗: {err_short}"))),
                        )
                        .child(
                            div()
                                .id("update-fallback-zip")
                                .cursor_pointer()
                                .px_1()
                                .rounded(px(3.0))
                                .bg(hsla(theme.accent))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.background))
                                .hover(|d| d.opacity(0.8))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_zip_fallback(cx);
                                }))
                                .child("zip で更新"),
                        )
                        .child(
                            div()
                                .id("update-fallback-dismiss")
                                .cursor_pointer()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_tertiary))
                                .hover(|d| d.text_color(hsla(theme.text_secondary)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_state =
                                        super::update_checker::UpdateState::Dismissed;
                                    cx.notify();
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                )
            }
            super::update_checker::UpdateState::CheckFailed(msg) => {
                let short = if msg.len() > 60 {
                    format!("{}…", &msg[..msg.floor_char_boundary(57)])
                } else {
                    msg.clone()
                };
                Some(
                    pill()
                        .id("update-check-failed")
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_tertiary))
                                .child(SharedString::from(short)),
                        )
                        .child(
                            div()
                                .id("update-check-failed-dismiss")
                                .cursor_pointer()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_tertiary))
                                .hover(|d| d.text_color(hsla(theme.text_secondary)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_state =
                                        super::update_checker::UpdateState::Dismissed;
                                    cx.notify();
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    fn show_update_confirm(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            super::update_checker::UpdateState::Available(info) => info.clone(),
            _ => return,
        };
        self.update_state = super::update_checker::UpdateState::ConfirmPending(info);
        cx.notify();
    }

    fn start_update(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            super::update_checker::UpdateState::ConfirmPending(info) => info.clone(),
            _ => return,
        };
        self.update_state = super::update_checker::UpdateState::Updating("更新中...".into());
        cx.notify();
        let info_for_fallback = info.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { super::update_checker::perform_update(&info) })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(msg) => {
                        app.update_state = super::update_checker::UpdateState::Done(format!(
                            "{msg} — 再起動中..."
                        ));
                        cx.notify();
                        app.save_layout();
                        if let Err(e) = super::update_checker::restart_app() {
                            app.update_state = super::update_checker::UpdateState::Failed(format!(
                                "更新は完了しましたが再起動に失敗: {e}"
                            ));
                            cx.notify();
                            return;
                        }
                        cx.quit();
                    }
                    Err(msg) => {
                        // brew 失敗で zip フォールバック可能な場合は専用状態に遷移（#50）
                        let method = super::update_checker::detect_install_method();
                        if method == super::update_checker::InstallMethod::Homebrew
                            && info_for_fallback.download_url.is_some()
                        {
                            app.update_state =
                                super::update_checker::UpdateState::BrewFailedFallback {
                                    brew_error: msg,
                                    info: info_for_fallback.clone(),
                                };
                        } else {
                            app.update_state = super::update_checker::UpdateState::Failed(msg);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_zip_fallback(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            super::update_checker::UpdateState::BrewFailedFallback { info, .. } => info.clone(),
            _ => return,
        };
        self.update_state =
            super::update_checker::UpdateState::Updating("zip フォールバックで更新中...".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { super::update_checker::perform_update_zip(&info) })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(msg) => {
                        app.update_state = super::update_checker::UpdateState::Done(format!(
                            "{msg} — 再起動中..."
                        ));
                        cx.notify();
                        app.save_layout();
                        if let Err(e) = super::update_checker::restart_app() {
                            app.update_state = super::update_checker::UpdateState::Failed(format!(
                                "更新は完了しましたが再起動に失敗: {e}"
                            ));
                            cx.notify();
                            return;
                        }
                        cx.quit();
                    }
                    Err(msg) => {
                        app.update_state = super::update_checker::UpdateState::Failed(msg);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_filetree(&mut self) {
        self.filetree.visible = !self.filetree.visible;
        self.sync_filetree_roots();
    }

    fn render_limit_service_menu(
        &self,
        claude_5h: Option<u32>,
        claude_week: Option<u32>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let selected = self.limit_service;

        let row = |svc: LimitService, h5: Option<u32>, w7: Option<u32>| {
            let is_selected = svc == selected;
            let svc_color = match svc {
                LimitService::Claude => theme.accent,
                LimitService::Codex => theme.teal,
                LimitService::Agy => theme.yellow,
            };
            let meter_inline = |label: &'static str, v: Option<u32>| match v {
                Some(val) => {
                    let color = if val >= 90 {
                        theme.red
                    } else if val >= 70 {
                        theme.yellow
                    } else {
                        theme.text_tertiary
                    };
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_muted))
                                .child(label),
                        )
                        .child(
                            div()
                                .w(px(32.0))
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(rgba(theme.surface_highlight))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative((val as f32 / 100.0).min(1.0)))
                                        .bg(hsla(color)),
                                ),
                        )
                        .child(
                            div()
                                .font_family(theme.font_family.clone())
                                .text_size(px(10.0))
                                .text_color(hsla(color))
                                .child(SharedString::from(format!("{val}%"))),
                        )
                        .into_any_element()
                }
                None => div()
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_faint))
                    .child("--")
                    .into_any_element(),
            };

            div()
                .id(SharedString::from(format!("limit-svc-{}", svc.as_str())))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(is_selected, |d| d.bg(rgba(theme.surface_hover_strong)))
                .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.limit_service = svc;
                    this.limit_service_menu_open = false;
                    if !cfg!(test) && std::env::var_os("TAKO_SELF_TEST").is_none() {
                        let mut settings = tako_control::settings::load();
                        settings.limit_service = svc.as_str().into();
                        let _ = tako_control::settings::save(&settings);
                    }
                    cx.notify();
                }))
                // 色ドット
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .flex_none()
                        .rounded_full()
                        .bg(hsla(svc_color)),
                )
                // サービス名
                .child(
                    div()
                        .w(px(48.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(11.5))
                        .text_color(if is_selected {
                            hsla(theme.foreground)
                        } else {
                            hsla(theme.text_secondary)
                        })
                        .child(svc.as_str()),
                )
                // 5h メーター
                .child(meter_inline("5h", h5))
                // 7d メーター
                .child(meter_inline("7d", w7))
        };

        div()
            .absolute()
            .bottom(px(STATUS_BAR_HEIGHT + 4.0))
            .right(px(0.0))
            .w(px(340.0))
            .rounded(px(9.0))
            .bg(rgba(theme.surface_1))
            .border_1()
            .border_color(hsla(theme.border_heavy))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.5),
                offset: point(px(0.), px(12.)),
                blur_radius: px(28.),
                spread_radius: px(0.),
                inset: false,
            }])
            .overflow_hidden()
            .occlude()
            .child(
                div()
                    .px(px(11.0))
                    .pt(px(8.0))
                    .pb(px(7.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_subtle))
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(theme.text_muted))
                    .child("USAGE LIMITS"),
            )
            .child(
                div()
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .child(row(LimitService::Claude, claude_5h, claude_week))
                    .child(row(LimitService::Codex, None, None))
                    .child(row(LimitService::Agy, None, None)),
            )
            .into_any_element()
    }

    pub(crate) fn toggle_panel_view(&mut self, view: PanelView, cx: &mut Context<Self>) {
        if self.panel_visible && self.panel_view == view {
            self.panel_visible = false;
        } else {
            self.panel_visible = true;
            self.panel_view = view;
            if view == PanelView::Tmux {
                self.refresh_tmux(cx);
            }
            if view == PanelView::Git {
                self.refresh_git(cx);
            }
        }
        cx.notify();
    }
}

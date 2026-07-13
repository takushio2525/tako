use gpui::{div, point, prelude::*, px, relative, BoxShadow, Context, FontWeight, SharedString};
use tako_core::CommandState;

use super::*;

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
                .gap_1()
                .h_full()
                .px_2()
                .cursor_pointer()
                .text_size(px(10.5))
                .when(active, |d| {
                    d.text_color(hsla(theme.accent))
                        .bg(rgba_alpha(theme.accent, 0.1))
                })
                .when(!active, |d| d.text_color(hsla(theme.text_tertiary)))
                .hover(|d| d.bg(rgba(theme.surface_hover)))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
        };
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
                let worker_count: usize = self
                    .workspace
                    .tabs()
                    .iter()
                    .flat_map(|tab| tab.tree().panes())
                    .filter(|p| {
                        p.role()
                            .is_some_and(|r| r.starts_with("orchestrator-worker:"))
                    })
                    .count();
                Some(worker_count)
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

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .border_t_1()
            .border_color(hsla(theme.border_subtle))
            .child(
                toggle("statusbar-filetree", self.filetree.visible)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_filetree();
                        cx.notify();
                    }))
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
                    .child(if bg_count > 0 {
                        format!("BG {bg_count}")
                    } else {
                        "BG".into()
                    })
            })
            .child({
                // Web ビュー dock（FR-3.8 / #155）: 開いたページの一覧・呼び出し・破棄
                let web_count = self.webviews.len();
                toggle("statusbar-web", self.webview_dock_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.webview_dock_open = !this.webview_dock_open;
                        cx.notify();
                    }))
                    .child(if web_count > 0 {
                        format!("🌐 {web_count}")
                    } else {
                        "🌐".into()
                    })
            })
            .children(fleet_label.map(|worker_count| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.accent))
                            .child("⚙"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.tab_active_foreground))
                            .child("master"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(format!(
                                "\u{00B7} {worker_count} workers"
                            ))),
                    )
            }))
            .child(div().flex_grow(1.0))
            .children(self.render_update_banner(&theme, cx))
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
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.teal))
                            .child("📊"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("usage"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_active_foreground))
                            .child(SharedString::from(tokens)),
                    );
                if let Some(c) = cost {
                    row = row.child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.teal))
                            .child(SharedString::from(c)),
                    );
                }
                row
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("ctx"),
                    )
                    .child(
                        div()
                            .w(px(70.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgba(theme.surface_highlight))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(3.0))
                                    .w(relative(ctx_fill_frac))
                                    .bg(hsla(ctx_bar_color)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_active_foreground))
                            .child(SharedString::from(format!("{ctx_pct}%"))),
                    )
                    .children(ctx_detail.map(|detail| {
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(detail))
                    })),
            )
            .child(
                toggle(
                    "statusbar-tmux",
                    self.panel_visible && self.panel_view == PanelView::Tmux,
                )
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
                .child("tmux"),
            )
            .child(
                toggle(
                    "statusbar-git",
                    self.panel_visible && self.panel_view == PanelView::Git,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Git, cx);
                }))
                .child({
                    let branch = self
                        .git_data
                        .as_ref()
                        .and_then(|d| d.branches.iter().find(|b| b.is_current))
                        .map(|b| truncate(&b.name, 16))
                        .unwrap_or_else(|| "git".into());
                    SharedString::from(format!("⎇ {branch}"))
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

//! アップデート専用画面 + 上部通知カード（Issue #616）
//!
//! 下部ステータスバーは表示物が増えて限界だったので、アップデート関連の UI を
//! すべてここへ移した。
//!
//! - **専用ウィンドウ** `UpdateWindow`: 設定画面（`settings_window`）・About（`about_window`）と
//!   同じ独立 GPUI ウィンドウ方式。現在バージョン / チャンネル / 配布系統 / 実行環境 /
//!   利用できるアップデート / リリースノート / 「今すぐ更新」までを 1 枚で見せる。
//!   **更新フローの全状態（確認・進行・完了・失敗）もここに出る**（表示先が 1 か所なので、
//!   ステータスバーへ戻る経路は残っていない）
//! - **通知カード** `TakoApp::render_update_card`: メインウィンドウ上部の帯。
//!   自動では消えず、× で閉じるまで残る。閉じた記録は「案内していたバージョン」
//!   （`update_checker::card_key`）単位で settings.json へ永続化するので、
//!   同じバージョンでは再起動をまたいでも出ないが、新しい版を検知すればまた出る
//!
//! 状態の実体は `TakoApp::update_state`（`update_checker::UpdateState`）で、
//! この画面はその表示 + 操作にすぎない。操作は CLI / MCP `tako update` と同じ
//! `update_checker` の関数を呼ぶ（開発不変条件の 1:1）。

use gpui::prelude::FluentBuilder;
use gpui::*;
use tako_core::theme::{Rgb, Theme};

use crate::file_icons::ui_icon;
use crate::ui_text::update as txt;
use crate::update_checker::{
    self, Channel, ChannelUpdates, InstallMethod, UpdateInfo, UpdateState, CURRENT_VERSION,
};
use crate::TakoApp;

/// 専用ウィンドウの既定サイズ（設定画面より縦長。リリースノートを読ませる）
pub const WINDOW_SIZE: (f32, f32) = (640.0, 620.0);

pub struct UpdateWindow {
    tako_app: WeakEntity<TakoApp>,
    focus: FocusHandle,
    /// 配布系統・実行環境の診断（`update_status_json`）。brew サブプロセスを呼びうるので
    /// render では絶対に取らず、開いた時と「確認」の後に background で取り直す
    status: Option<serde_json::Value>,
}

impl UpdateWindow {
    pub fn new(tako_app: WeakEntity<TakoApp>, cx: &mut Context<Self>) -> Self {
        // テーマ変更・更新状態の遷移（自動チェック / CLI / MCP）に追従する
        if let Some(app) = tako_app.upgrade() {
            cx.observe(&app, |_this: &mut Self, _app, cx| cx.notify())
                .detach();
        }
        let this = Self {
            tako_app,
            focus: cx.focus_handle(),
            status: None,
        };
        this.reload_status(cx);
        this
    }

    /// 診断情報を background で取り直す（brew 台帳の確認が入るため UI を止めない）
    fn reload_status(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { update_checker::update_status_json() })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    fn theme(&self) -> Theme {
        tako_control::settings::load().resolve_theme().0
    }

    fn update_state(&self, cx: &App) -> UpdateState {
        self.tako_app
            .upgrade()
            .map(|app| app.read(cx).update_state.clone())
            .unwrap_or(UpdateState::Idle)
    }

    /// TakoApp 側の操作を呼ぶ（更新フローの実体は TakoApp が持つ）
    fn with_app(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut TakoApp, &mut Context<TakoApp>),
    ) {
        if let Some(app) = self.tako_app.upgrade() {
            app.update(cx, |app, cx| f(app, cx));
        }
    }

    fn status_str(&self, key: &str) -> Option<String> {
        self.status.as_ref()?[key].as_str().map(str::to_string)
    }

    fn install_method_label(&self) -> String {
        let method = self
            .status_str("install_method")
            // 診断が届く前は高速パス（ファイルパス判定のみ）で埋める
            .unwrap_or_else(|| update_checker::detect_install_method().label().to_string());
        txt::install_method_display(&method)
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let state = self.update_state(cx);

        div()
            .key_context("UpdateWindow")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(to_hsla(theme.surface_0))
            .text_color(to_hsla(theme.foreground))
            // 設定ウィンドウと同じく ⌘W は「このウィンドウを閉じる」
            .on_action(cx.listener(|this, _: &crate::ClosePane, window, cx| {
                if let Some(app) = this.tako_app.upgrade() {
                    app.update(cx, |app, _| app.update_window_handle = None);
                }
                window.remove_window();
            }))
            .child(
                div()
                    .id("update-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px_5()
                    .py_4()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(self.render_current(&theme))
                    .children(self.render_broken_brew(&theme, cx))
                    .child(self.render_available(&theme, &state, cx))
                    .children(self.render_flow(&theme, &state, cx))
                    .children(self.render_notes(&theme, &state)),
            )
            .child(self.render_footer(&theme, &state, cx))
    }
}

impl UpdateWindow {
    // --- 現在のバージョン ---

    fn render_current(&self, theme: &Theme) -> Div {
        let channel = if CURRENT_VERSION.contains("-test.") {
            Channel::Test
        } else {
            Channel::Stable
        };
        let env = match (
            self.status_str("platform"),
            self.status_str("arch"),
            self.status_str("asset_pattern"),
        ) {
            (Some(p), Some(a), Some(pattern)) => Some(format!("{p} / {a} ({pattern})")),
            (Some(p), Some(a), None) => Some(format!("{p} / {a}")),
            _ => None,
        };
        section(theme, txt::section_current())
            .child(kv(
                theme,
                txt::label_version(),
                format!("v{CURRENT_VERSION}"),
            ))
            .child(kv(
                theme,
                txt::label_channel(),
                channel.display_label().to_string(),
            ))
            .child(kv(
                theme,
                txt::label_install_method(),
                self.install_method_label(),
            ))
            .children(env.map(|e| kv(theme, txt::label_platform(), e)))
    }

    /// broken-brew（.app はあるが brew の台帳に無い）の警告 + 修復ボタン
    fn render_broken_brew(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let broken = self.status_str("install_method").as_deref() == Some("broken-brew");
        if !broken {
            return None;
        }
        Some(
            notice(theme, theme.yellow)
                .child(
                    div()
                        .text_size(px(12.))
                        .child(SharedString::from(txt::broken_brew_note().to_string())),
                )
                .child(div().flex().flex_row().gap(px(8.)).child(button(
                    "update-repair",
                    txt::repair_button(),
                    theme,
                    BtnKind::Normal,
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.start_update_repair(cx));
                        this.reload_status(cx);
                    }),
                ))),
        )
    }

    // --- 利用できるアップデート ---

    fn render_available(&self, theme: &Theme, state: &UpdateState, cx: &mut Context<Self>) -> Div {
        let mut body = section(theme, txt::section_available());
        match state {
            UpdateState::Available(updates) => {
                body = body
                    .child(self.render_channel_row(theme, Channel::Stable, updates, cx))
                    .child(self.render_channel_row(theme, Channel::Test, updates, cx));
                if let Some(note) = updates.rate_limit_note.as_ref() {
                    body = body.child(muted(theme, note.clone()));
                }
            }
            // 手動チェックが「最新版です」で終わった直後
            UpdateState::Done(_) => body = body.child(muted(theme, txt::no_updates().to_string())),
            UpdateState::CheckFailed(msg) => {
                body = body.child(muted(theme, msg.clone()));
            }
            _ => body = body.child(muted(theme, txt::not_checked_yet().to_string())),
        }
        body
    }

    /// チャンネル 1 行（バッジ + バージョン + 配布物名 + 更新ボタン + リリースページ）
    fn render_channel_row(
        &self,
        theme: &Theme,
        channel: Channel,
        updates: &ChannelUpdates,
        cx: &mut Context<Self>,
    ) -> Div {
        let info = match channel {
            Channel::Stable => updates.stable.as_ref(),
            Channel::Test => updates.test.as_ref(),
        };
        let color = match channel {
            Channel::Stable => theme.green,
            Channel::Test => theme.yellow,
        };
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .py(px(5.))
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(to_hsla(color))
                    .text_size(px(10.))
                    .text_color(to_hsla(theme.background))
                    .child(channel.label()),
            );
        let Some(info) = info else {
            return row.child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(to_hsla(theme.text_tertiary))
                    .child(match channel {
                        Channel::Stable => txt::latest(),
                        Channel::Test => txt::no_test_build(),
                    }),
            );
        };
        let url = info.html_url.clone();
        let info_for_click = info.clone();
        row.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .child(SharedString::from(format!("v{}", info.version))),
                )
                .children(info.asset_name.as_ref().map(|name| {
                    div()
                        .text_size(px(10.5))
                        .text_color(to_hsla(theme.text_muted))
                        .child(SharedString::from(format!(
                            "{}: {name}",
                            txt::label_asset()
                        )))
                })),
        )
        .when(!url.is_empty(), |d| {
            d.child(link(
                match channel {
                    Channel::Stable => "update-release-stable",
                    Channel::Test => "update-release-test",
                },
                txt::open_release_page(),
                url,
                theme,
            ))
        })
        .child(button(
            match channel {
                Channel::Stable => "update-now-stable",
                Channel::Test => "update-now-test",
            },
            txt::update_now(),
            theme,
            BtnKind::Primary,
            cx.listener(move |this, _, _, cx| {
                let info = info_for_click.clone();
                this.with_app(cx, move |app, cx| {
                    app.show_update_confirm_for_channel(info, cx)
                });
            }),
        ))
    }

    // --- 更新フローの状態（確認 / 進行 / 完了 / 失敗）---

    fn render_flow(
        &self,
        theme: &Theme,
        state: &UpdateState,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        match state {
            UpdateState::TestWarning(info) => Some(
                notice(theme, theme.yellow)
                    .child(text(theme, txt::test_warning(&info.version)))
                    .child(
                        actions()
                            .child(button(
                                "update-test-yes",
                                txt::cont(),
                                theme,
                                BtnKind::Primary,
                                cx.listener(|this, _, _, cx| {
                                    this.with_app(cx, |app, cx| app.confirm_test_update(cx));
                                }),
                            ))
                            .child(cancel_button("update-test-no", theme, cx)),
                    ),
            ),
            UpdateState::ConfirmPending(info) => {
                let msg = txt::confirm(
                    &info.version,
                    info.channel.display_label(),
                    &self.install_method_label(),
                );
                Some(
                    notice(theme, theme.accent).child(text(theme, msg)).child(
                        actions()
                            .child(button(
                                "update-confirm-yes",
                                txt::run(),
                                theme,
                                BtnKind::Primary,
                                cx.listener(|this, _, _, cx| {
                                    this.with_app(cx, |app, cx| app.start_update(cx));
                                }),
                            ))
                            .child(cancel_button("update-confirm-no", theme, cx)),
                    ),
                )
            }
            UpdateState::Updating(msg) => {
                Some(notice(theme, theme.yellow).child(text(theme, msg.clone())))
            }
            UpdateState::Done(msg) => Some(
                notice(theme, theme.green)
                    .child(text(theme, msg.clone()))
                    .child(actions().child(button(
                        "update-done-dismiss",
                        crate::ui_text::common::close(),
                        theme,
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.with_app(cx, |app, cx| {
                                app.set_update_state(UpdateState::Idle, cx)
                            });
                        }),
                    ))),
            ),
            UpdateState::Failed(msg) => Some(
                notice(theme, theme.red)
                    .child(text(theme, msg.clone()))
                    .child(actions().child(button(
                        "update-failed-dismiss",
                        crate::ui_text::common::close(),
                        theme,
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| {
                            this.with_app(cx, |app, cx| {
                                app.set_update_state(UpdateState::Idle, cx)
                            });
                        }),
                    ))),
            ),
            UpdateState::BrewFailedFallback { brew_error, .. } => Some(
                notice(theme, theme.red)
                    .child(text(theme, txt::brew_failed(brew_error)))
                    .child(
                        actions()
                            .child(button(
                                "update-fallback-zip",
                                txt::update_via_zip(),
                                theme,
                                BtnKind::Primary,
                                cx.listener(|this, _, _, cx| {
                                    this.with_app(cx, |app, cx| app.start_zip_fallback(cx));
                                }),
                            ))
                            .child(cancel_button("update-fallback-dismiss", theme, cx)),
                    ),
            ),
            // チェック失敗は「利用できるアップデート」欄に出しているので二重に出さない
            UpdateState::Idle | UpdateState::Available(_) | UpdateState::CheckFailed(_) => None,
        }
    }

    // --- リリースノート ---

    fn render_notes(&self, theme: &Theme, state: &UpdateState) -> Option<Div> {
        let updates = match state {
            UpdateState::Available(u) => u,
            _ => return None,
        };
        // 安定版を主に見せる（無ければテスト版）。両方あるときは章題にバージョンを添える
        let info: &UpdateInfo = updates.stable.as_ref().or(updates.test.as_ref())?;
        let title = format!("{} (v{})", txt::section_notes(), info.version);
        Some(
            section(theme, &title).child(
                div()
                    .id("update-notes")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(to_hsla(theme.surface_1))
                    .font_family(theme.font_family.clone())
                    .text_size(px(11.5))
                    .text_color(to_hsla(theme.text_secondary))
                    .child(SharedString::from(
                        info.notes
                            .clone()
                            .unwrap_or_else(|| txt::no_notes().to_string()),
                    )),
            ),
        )
    }

    // --- フッター（確認ボタン + 常設の注意書き）---

    fn render_footer(&self, theme: &Theme, state: &UpdateState, cx: &mut Context<Self>) -> Div {
        // チェック中・更新中は二重実行させない
        let busy = matches!(
            state,
            UpdateState::Updating(_) | UpdateState::ConfirmPending(_) | UpdateState::TestWarning(_)
        );
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px_5()
            .py_3()
            .border_t_1()
            .border_color(to_hsla(theme.border_subtle))
            .bg(to_hsla(theme.mantle))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(11.))
                    .text_color(to_hsla(theme.text_muted))
                    .child(SharedString::from(txt::restart_warning().to_string())),
            )
            .child(button(
                "update-check",
                txt::check_button(),
                theme,
                if busy {
                    BtnKind::Disabled
                } else {
                    BtnKind::Normal
                },
                cx.listener(|this, _, _, cx| {
                    this.with_app(cx, |app, cx| app.start_update_check(cx));
                    this.reload_status(cx);
                }),
            ))
    }
}

// ---------------------------------------------------------------------------
// メインウィンドウ側: 上部通知カード + 更新フローの実体
// ---------------------------------------------------------------------------

impl TakoApp {
    /// 上部通知カード（#616）。タブバー直下に全幅で積む。
    ///
    /// オーバーレイにしないのは、× で閉じるまで残る仕様だから
    /// （ターミナル出力の上に居座らせない。ウェルカムバナー #549 と同じ判断）。
    /// 表示条件は「更新あり」かつ「そのバージョンを × で閉じていない」
    pub(crate) fn render_update_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let updates = match &self.update_state {
            UpdateState::Available(u) => u.clone(),
            _ => return None,
        };
        if !update_checker::card_should_show(&updates, self.update_card_dismissed.as_deref()) {
            return None;
        }
        let theme = self.theme.clone();
        let summary = if updates.stable.is_some() && updates.test.is_some() {
            txt::banner_both().to_string()
        } else if let Some(ref s) = updates.stable {
            txt::banner_stable(&s.version)
        } else if let Some(ref t) = updates.test {
            txt::banner_test(&t.version)
        } else {
            return None;
        };

        Some(
            div()
                .id("update-card")
                .flex_none()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(14.0))
                .py(px(9.0))
                .bg(crate::rgba_alpha(theme.accent, 0.10))
                .border_b_1()
                .border_color(crate::hsla_alpha(theme.accent, 0.28))
                .child(
                    svg()
                        .path(ui_icon::ARROW_DOWN)
                        .w(px(13.0))
                        .h(px(13.0))
                        .flex_none()
                        .text_color(crate::hsla(theme.accent)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(crate::hsla(theme.foreground))
                        .child(SharedString::from(txt::card_title().to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(crate::hsla(theme.text_secondary))
                        .child(SharedString::from(summary)),
                )
                // 詳細を見る = 専用画面へ（更新の実行はそちらで確認してから）
                .child(
                    div()
                        .id("update-card-details")
                        .flex_none()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .bg(crate::rgba(theme.accent))
                        .text_color(crate::hsla(theme.background))
                        .hover({
                            let accent = theme.accent;
                            move |d| d.bg(crate::rgba_alpha(accent, 0.85))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_update_open = true;
                            cx.notify();
                        }))
                        .child(SharedString::from(txt::card_details().to_string())),
                )
                // × は「このバージョンについてはもう通知しない」。意味を字で添える
                .child(
                    div()
                        .id("update-card-dismiss")
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .text_size(px(11.0))
                        .text_color(crate::hsla(theme.text_muted))
                        .cursor_pointer()
                        .hover({
                            let hl = theme.surface_highlight;
                            move |d| d.bg(crate::rgba(hl))
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_update_card(cx)))
                        .child(SharedString::from(txt::card_dismiss_hint().to_string()))
                        .child(
                            svg()
                                .path(ui_icon::CLOSE)
                                .w(px(9.0))
                                .h(px(9.0))
                                .flex_none()
                                .text_color(crate::hsla(theme.text_muted)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// カードを閉じる（GUI の ×）。永続化まで含めて CLI / MCP と同じ経路を通す
    pub(crate) fn dismiss_update_card(&mut self, cx: &mut Context<Self>) {
        let _ = tako_control::dispatch::dispatch(
            self,
            tako_control::protocol::Request::Update {
                action: Some("card-dismiss".into()),
                channel: None,
            },
            tako_core::pane::PaneOrigin::User,
        );
        cx.notify();
    }

    /// 更新状態の差し替え（専用画面の「閉じる」等）
    pub(crate) fn set_update_state(&mut self, state: UpdateState, cx: &mut Context<Self>) {
        self.update_state = state;
        cx.notify();
    }

    /// 更新確認へ進む（テスト版は不安定警告を先に挟む）。
    ///
    /// 確認フローへ入る唯一の入口なので、ここで直前の一覧を控えておく。
    /// キャンセルは GitHub へ問い合わせ直さずこの控えへ戻る（無駄な API 消費を避ける）
    pub(crate) fn show_update_confirm_for_channel(
        &mut self,
        info: UpdateInfo,
        cx: &mut Context<Self>,
    ) {
        if let UpdateState::Available(updates) = &self.update_state {
            self.update_available = Some(updates.clone());
        }
        if info.channel == Channel::Test {
            self.update_state = UpdateState::TestWarning(info);
        } else {
            self.update_state = UpdateState::ConfirmPending(info);
        }
        cx.notify();
    }

    /// 確認フローの取り消し（専用画面の「キャンセル」）。控えた一覧へ戻す
    pub(crate) fn cancel_update_flow(&mut self, cx: &mut Context<Self>) {
        self.update_state = match self.update_available.take() {
            Some(updates) => UpdateState::Available(updates),
            None => UpdateState::Idle,
        };
        cx.notify();
    }

    pub(crate) fn confirm_test_update(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            UpdateState::TestWarning(info) => info.clone(),
            _ => return,
        };
        self.update_state = UpdateState::ConfirmPending(info);
        cx.notify();
    }

    pub(crate) fn start_update(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            UpdateState::ConfirmPending(info) => info.clone(),
            _ => return,
        };
        self.update_state = UpdateState::Updating(txt::updating().into());
        cx.notify();
        let info_for_fallback = info.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { update_checker::perform_update(&info) })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(msg) => {
                        app.update_state = UpdateState::Done(txt::restarting(&msg));
                        cx.notify();
                        app.save_layout();
                        if let Err(e) = update_checker::restart_app() {
                            app.update_state =
                                UpdateState::Failed(txt::restart_failed(&e.to_string()));
                            cx.notify();
                            return;
                        }
                        cx.quit();
                    }
                    Err(msg) => {
                        // brew 失敗で zip フォールバック可能な場合は専用状態に遷移（#50）
                        let method = update_checker::detect_install_method();
                        if method == InstallMethod::Homebrew
                            && info_for_fallback.download_url.is_some()
                        {
                            app.update_state = UpdateState::BrewFailedFallback {
                                brew_error: msg,
                                info: info_for_fallback.clone(),
                            };
                        } else {
                            app.update_state = UpdateState::Failed(msg);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn start_zip_fallback(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            UpdateState::BrewFailedFallback { info, .. } => info.clone(),
            _ => return,
        };
        self.update_state = UpdateState::Updating(txt::updating_zip_fallback().into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { update_checker::perform_update_zip(&info) })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                match result {
                    Ok(msg) => {
                        app.update_state = UpdateState::Done(txt::restarting(&msg));
                        cx.notify();
                        app.save_layout();
                        if let Err(e) = update_checker::restart_app() {
                            app.update_state =
                                UpdateState::Failed(txt::restart_failed(&e.to_string()));
                            cx.notify();
                            return;
                        }
                        cx.quit();
                    }
                    Err(msg) => app.update_state = UpdateState::Failed(msg),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// broken-brew の修復（#50）。専用画面のボタンから
    pub(crate) fn start_update_repair(&mut self, cx: &mut Context<Self>) {
        self.update_state = UpdateState::Updating(txt::updating().into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { update_checker::repair_brew() })
                .await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                app.update_state = match result {
                    Ok(msg) => UpdateState::Done(msg),
                    Err(msg) => UpdateState::Failed(msg),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------------------
// 共通ウィジェット（設定画面と同じデザイン言語）
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Normal,
    Disabled,
}

/// 見出し + 本体の 1 区画
fn section(theme: &Theme, title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .pt_2()
        .child(
            div()
                .pb_1()
                .text_color(to_hsla(theme.text_secondary))
                .text_size(px(12.))
                .child(title.to_string()),
        )
        .child(div().h(px(1.)).mb(px(4.)).bg(to_hsla(theme.border_subtle)))
}

/// ラベル + 値の 1 行
fn kv(theme: &Theme, label: &str, value: String) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .py(px(3.))
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .text_size(px(11.5))
                .text_color(to_hsla(theme.text_muted))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.5))
                .child(SharedString::from(value)),
        )
}

fn muted(theme: &Theme, s: String) -> Div {
    div()
        .py(px(4.))
        .text_size(px(12.))
        .text_color(to_hsla(theme.text_tertiary))
        .child(SharedString::from(s))
}

fn text(theme: &Theme, s: String) -> Div {
    div()
        .text_size(px(12.5))
        .text_color(to_hsla(theme.foreground))
        .child(SharedString::from(s))
}

/// 状態ブロック（左に色帯を付けた囲み）
fn notice(theme: &Theme, color: Rgb) -> Div {
    div()
        .mt_2()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(10.))
        .rounded(px(6.))
        .border_l_2()
        .border_color(to_hsla(color))
        .bg(to_hsla(theme.surface_1))
}

fn actions() -> Div {
    div().flex().flex_row().gap(px(8.))
}

fn button(
    id: &'static str,
    label: &str,
    theme: &Theme,
    kind: BtnKind,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (bg, fg) = match kind {
        BtnKind::Primary => (to_hsla(theme.accent), to_hsla(theme.background)),
        BtnKind::Normal => (to_hsla(theme.chip_surface), to_hsla(theme.foreground)),
        BtnKind::Disabled => (to_hsla(theme.surface_1), to_hsla(theme.text_faint)),
    };
    let mut b = div()
        .id(id)
        .flex_none()
        .px_3()
        .py(px(5.))
        .rounded(px(6.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .child(label.to_string());
    if kind != BtnKind::Disabled {
        b = b.cursor_pointer().on_click(on_click);
    }
    b
}

/// 「キャンセル」= 更新フローを畳んで一覧へ戻す
fn cancel_button(id: &'static str, theme: &Theme, cx: &mut Context<UpdateWindow>) -> Stateful<Div> {
    button(
        id,
        crate::ui_text::common::cancel(),
        theme,
        BtnKind::Normal,
        cx.listener(|this: &mut UpdateWindow, _, _, cx| {
            // 直前に見ていた一覧へ戻す（控えがあるので GitHub へ問い合わせ直さない）
            this.with_app(cx, |app, cx| app.cancel_update_flow(cx));
        }),
    )
}

fn link(id: &'static str, label: &str, url: String, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .text_size(px(11.5))
        .text_color(to_hsla(theme.accent))
        .cursor_pointer()
        .hover(|d| d.underline())
        .child(label.to_string())
        .on_click(move |_, _, _| crate::open_external_url(&url))
}

fn to_hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)).into()
}

#[cfg(test)]
mod tests {
    /// 番犬テスト（#616 受け入れ条件 1）: ステータスバーがアップデート状態を
    /// **読まない**こと。読まなければ描きようがないので、これが「下部バーに
    /// アップデート表示が出ない」の構造的な保証になる（見た目の確認より強い）。
    ///
    /// 撤去した実装をうっかり戻す・別経路で描き足す、のどちらもここで落ちる
    #[test]
    fn ステータスバーにアップデート表示が残っていない() {
        let src = include_str!("status_bar.rs");
        for needle in [
            "update_state",
            "UpdateState",
            "update_checker",
            "render_update",
            "update_card",
        ] {
            assert!(
                !src.contains(needle),
                "status_bar.rs に {needle} が残っている。\
                 アップデート UI は update_window（上部カード + 専用画面）が受け持つ（#616）"
            );
        }
    }

    /// 逆側の担保: 受け皿（カード + 専用画面）は確かにアップデート状態を読んでいる。
    /// 上のテストだけだと「全部消した」でも通ってしまう
    #[test]
    fn アップデートuiの受け皿はこのモジュールにある() {
        let src = include_str!("update_window.rs");
        assert!(src.contains("fn render_update_card"), "通知カードが無い");
        assert!(
            src.contains("impl Render for UpdateWindow"),
            "専用画面が無い"
        );
        assert!(
            src.contains("card_should_show"),
            "カードの表示判定（バージョン単位の抑止）を通っていない"
        );
    }
}

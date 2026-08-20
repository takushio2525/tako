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
use crate::preview;
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
    /// リリースノートの Markdown パース結果（#690）。
    ///
    /// `pulldown-cmark` によるパースは render 毎フレームには重いので、
    /// 「どのノート文字列を解いた結果か」を鍵にして持ち回す。
    notes: Option<ParsedNotes>,
    /// ⌘ 押下中にホバーしているノート内リンクの索引（`notes.links` の添字。#690）
    hovered_note_link: Option<usize>,
    /// リリースノート枠のスクロール位置（#690）。ノートは枠内で縦スクロールするので、
    /// 位置を持っておくと再描画で先頭へ戻らない。visual-test も同じ経路で送る
    notes_scroll: ScrollHandle,
}

/// パース済みリリースノート（#690）
struct ParsedNotes {
    /// パース元の md 本文。これが変われば解き直す
    source: String,
    blocks: Vec<preview::MdBlock>,
    /// ⌘+クリックの当たり判定（行番号 + バイト範囲 + 遷移先）。
    /// 索引の正はプレビューと同じ `md_document_links` 1 本（#680）
    links: Vec<crate::MdLinkHit>,
    /// 直近の描画で得た行ごとの実 shaping。ヒットテストはこれを使う
    layouts: std::cell::RefCell<Vec<Option<TextLayout>>>,
    /// この世代のノートが実際に描き終わったか。
    ///
    /// **GPUI の `TextLayout::bounds()` は prepaint 前に呼ぶと `unwrap` で panic する**
    /// （= アプリ全体が落ちる）。ヒットテストはこれが立ってからしか行わない。
    /// 立てるのはノートと同じ親に置いた canvas の paint で、GPUI の div は
    /// 「全子の prepaint → 全子の paint」の順に回すため、paint が呼ばれた時点で
    /// 兄弟のテキストは測り終わっている（描いた世代だけ触るのが構造で保証される）
    painted: std::rc::Rc<std::cell::Cell<bool>>,
}

impl ParsedNotes {
    /// md 本文を解く。**パースは `preview::markdown_blocks`（`pulldown-cmark`）が正**で、
    /// リンク索引は `md_document_links`（プレビューと同じ 1 本）が正（#680 / #690）
    fn parse(source: &str) -> Self {
        let blocks = preview::markdown_blocks(source);
        let links = crate::md_document_links(&blocks);
        Self {
            source: source.to_string(),
            blocks,
            links,
            layouts: std::cell::RefCell::new(Vec::new()),
            painted: std::rc::Rc::new(std::cell::Cell::new(false)),
        }
    }
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
            notes: None,
            hovered_note_link: None,
            notes_scroll: ScrollHandle::new(),
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
        // リリースノートの md パースは本文が変わったときだけ（#690）
        self.sync_notes(&state);

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
                    .children(self.render_notes(&theme, &state, cx)),
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

    /// 表示対象のリリースノート（安定版を主に見せる。無ければテスト版）
    fn notes_source(state: &UpdateState) -> Option<(&UpdateInfo, &str)> {
        let updates = match state {
            UpdateState::Available(u) => u,
            _ => return None,
        };
        let info: &UpdateInfo = updates.stable.as_ref().or(updates.test.as_ref())?;
        Some((info, info.notes.as_deref().unwrap_or_default()))
    }

    /// リリースノートの md パース結果を必要なときだけ作り直す（#690）。
    ///
    /// `render` は毎フレーム走るので、同じ本文なら解き直さない。ノートが差し替わったら
    /// ホバー索引は別の URL を指すので一緒に落とす（#680 の `forget_md_links` と同じ理屈）
    fn sync_notes(&mut self, state: &UpdateState) {
        let source = Self::notes_source(state)
            .map(|(_, notes)| notes)
            .unwrap_or("");
        if source.is_empty() {
            if self.notes.is_some() {
                self.notes = None;
                self.hovered_note_link = None;
            }
            return;
        }
        if self.notes.as_ref().is_some_and(|n| n.source == source) {
            return;
        }
        self.notes = Some(ParsedNotes::parse(source));
        self.hovered_note_link = None;
    }

    /// リリースノート欄（#690: Markdown レンダリング）。
    ///
    /// 生成元は #594 の機構が作る md（見出し・ダウンロード表・リスト・リンク・日英併記）
    /// なので、プレビューペインと**同じ** `md_view::render_block` で描く。
    /// リンクは ⌘+クリックで既定ブラウザ（#680 と同じ UX。http / https のみ）
    fn render_notes(
        &self,
        theme: &Theme,
        state: &UpdateState,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (info, _) = Self::notes_source(state)?;
        let title = format!("{} (v{})", txt::section_notes(), info.version);
        let Some(notes) = self.notes.as_ref() else {
            // ノートが無いリリース（body 空）はプレーンな案内文だけ出す
            return Some(
                section(theme, &title)
                    .child(notes_box(theme).child(muted(theme, txt::no_notes().to_string()))),
            );
        };
        let hovered = self
            .hovered_note_link
            .and_then(|index| notes.links.get(index))
            .map(|hit| (hit.line, hit.range.clone()));
        let (elements, layouts) = crate::md_view::render_document(theme, &notes.blocks, hovered);
        *notes.layouts.borrow_mut() = layouts;
        // 新しい世代はまだ測られていない。描き終わりを canvas の paint で受けてから
        // ヒットテストを許す（GPUI の TextLayout は測る前に触ると panic する）
        notes.painted.set(false);
        let painted = notes.painted.clone();
        // 描き終わりの合図だけを受ける 1px の canvas（ノートと同じ親に置く）
        let paint_probe = canvas(|_, _, _| (), move |_, _, _, _| painted.set(true))
            .w_full()
            .h(px(1.));
        let has_openable_link = notes
            .links
            .iter()
            .any(|hit| tako_core::md_links::browser_url(&hit.url).is_some());
        Some(
            section(theme, &title)
                .child(
                    notes_box(theme)
                        .track_scroll(&self.notes_scroll)
                        .when(self.hovered_note_link.is_some(), |d| {
                            d.cursor(CursorStyle::PointingHand)
                        })
                        // ⌘+ホバーで下線を強め、⌘+クリックで既定ブラウザへ（#680 と同じ規則）。
                        // リンクが 1 本も無いノートではイベント経路自体を載せない
                        .when(has_openable_link, |d| {
                            d.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                this.update_note_link_hover(ev.position, ev.modifiers.platform, cx);
                            }))
                            .on_modifiers_changed(cx.listener(
                                |this, ev: &ModifiersChangedEvent, window, cx| {
                                    let position = window.mouse_position();
                                    this.update_note_link_hover(
                                        position,
                                        ev.modifiers.platform,
                                        cx,
                                    );
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    if !ev.modifiers.platform || ev.click_count != 1 {
                                        return;
                                    }
                                    // ⌘ 単独押下の直後（ホバー未更新）でも位置から引き直す
                                    let index = this
                                        .hovered_note_link
                                        .or_else(|| this.note_link_at(ev.position));
                                    if let Some(index) = index {
                                        this.open_note_link(index);
                                        this.hovered_note_link = None;
                                        cx.notify();
                                    }
                                }),
                            )
                        })
                        .children(elements),
                )
                .child(paint_probe),
        )
    }

    /// セルフテスト用の観測点（#690）。リリースノートの解析・描画状態を返す:
    /// (ブロック数, 表を含むか, 開けるリンク数, 直近描画で控えた行数)。
    /// 「生テキストではなく md として描かれた」を GUI 経路で機械検証するために公開する
    pub(crate) fn notes_probe(&self) -> (usize, bool, usize, usize) {
        let Some(notes) = self.notes.as_ref() else {
            return (0, false, 0, 0);
        };
        (
            notes.blocks.len(),
            notes
                .blocks
                .iter()
                .any(|b| matches!(b.kind, preview::MdBlockKind::Table { .. })),
            notes
                .links
                .iter()
                .filter(|l| tako_core::md_links::browser_url(&l.url).is_some())
                .count(),
            notes.layouts.borrow().len(),
        )
    }

    /// visual-test 用（#690）: 描き終わったノート枠の実 bounds（実ピクセルの走査範囲）。
    /// 矩形はスクロール容器そのものから取る（ヒットテストと同じ「描いた世代」条件つき）
    #[cfg(feature = "visual-test")]
    pub(crate) fn notes_bounds(&self) -> Option<Bounds<Pixels>> {
        let notes = self.notes.as_ref()?;
        notes.painted.get().then(|| self.notes_scroll.bounds())
    }

    /// visual-test 用（#690）: この画面が描くのに使っているテーマ（期待色の出所）
    #[cfg(feature = "visual-test")]
    pub(crate) fn visual_theme(&self) -> Theme {
        self.theme()
    }

    /// visual-test 用（#690）: ノート枠を縦にスクロールさせる（GUI のホイールと同じ経路）。
    /// 戻り値は「実際に動いたか」（下端に着いていれば false）
    #[cfg(feature = "visual-test")]
    pub(crate) fn scroll_notes_to(&self, y: f32) -> bool {
        let before = self.notes_scroll.offset();
        self.notes_scroll.set_offset(gpui::point(before.x, px(-y)));
        f32::from(self.notes_scroll.offset().y) != f32::from(before.y)
    }

    /// visual-test 用（#690）: ⌘+ホバー中のリンクを指定して装飾差分を作る。
    /// `None` でホバー解除。戻り値は「そのリンクが存在したか」
    #[cfg(feature = "visual-test")]
    pub(crate) fn set_hovered_note_link(&mut self, index: Option<usize>) -> bool {
        let exists = match (index, self.notes.as_ref()) {
            (Some(i), Some(notes)) => notes.links.get(i).is_some(),
            (None, _) => true,
            _ => false,
        };
        self.hovered_note_link = index.filter(|_| exists);
        exists
    }

    /// visual-test 用（#690）: 最初に開けるリンクの索引（ホバー装飾の対象）
    #[cfg(feature = "visual-test")]
    pub(crate) fn first_openable_note_link(&self) -> Option<usize> {
        let notes = self.notes.as_ref()?;
        notes
            .links
            .iter()
            .position(|hit| tako_core::md_links::browser_url(&hit.url).is_some())
    }

    /// セルフテスト用（#690）: ⌘+クリックの経路をそのまま叩く。
    /// 実ブラウザは `open_external_url` が `TAKO_SELF_TEST` で抑止する
    pub(crate) fn probe_note_link_click(&mut self, position: Point<Pixels>) -> Option<usize> {
        let index = self.note_link_at(position);
        if let Some(index) = index {
            self.open_note_link(index);
        }
        index
    }

    /// ノート内リンクのヒットテスト（#690）。
    /// 未描画の世代には触らない（`ParsedNotes::painted` の注記のとおり panic 防止）
    fn note_link_at(&self, position: Point<Pixels>) -> Option<usize> {
        let notes = self.notes.as_ref()?;
        if !notes.painted.get() {
            return None;
        }
        crate::md_view::md_link_at_layouts(&notes.links, &notes.layouts.borrow(), position)
    }

    fn update_note_link_hover(
        &mut self,
        position: Point<Pixels>,
        cmd_held: bool,
        cx: &mut Context<Self>,
    ) {
        let found = if cmd_held {
            self.note_link_at(position)
        } else {
            None
        };
        if found != self.hovered_note_link {
            self.hovered_note_link = found;
            cx.notify();
        }
    }

    /// ノート内リンクを既定ブラウザで開く。開いてよい URL の判定は
    /// `md_links::browser_url` が正（http / https のみ。#680）
    fn open_note_link(&self, index: usize) {
        let Some(notes) = self.notes.as_ref() else {
            return;
        };
        let Some(hit) = notes.links.get(index) else {
            return;
        };
        match tako_core::md_links::browser_url(&hit.url) {
            Some(url) => crate::open_external_url(url),
            None => eprintln!("warning: 開けないリンク（http / https のみ）: {}", hit.url),
        }
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

/// リリースノートの囲み（#690）。中身は md レンダリング結果なので、文字色・サイズは
/// ブロック側が決める。ここは面と枠と縦スクロールだけを持つ
fn notes_box(theme: &Theme) -> Stateful<Div> {
    div()
        .id("update-notes")
        .max_h(px(260.))
        .overflow_y_scroll()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(to_hsla(theme.surface_1))
        .flex()
        .flex_col()
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

    // --- リリースノートの Markdown レンダリング（#690）---

    // `use super::*` は gpui の `test` 属性マクロまで取り込んで `#[test]` を
    // 無限展開させるので、必要なものだけ名指しする
    use super::{ParsedNotes, UpdateWindow};
    use crate::preview;
    use crate::update_checker::{Channel, ChannelUpdates, UpdateInfo, UpdateState};

    /// #594 の機構が生成する実物に近いリリースノート（見出し・ダウンロード表・
    /// リスト・リンク・コード・日英併記）。表示側の検証はこれを正とする
    const RELEASE_NOTES: &str = "\
## Highlights / ハイライト

- **Markdown** release notes / リリースノートの md 表示
- Fixed [#680](https://github.com/takushio2525/tako/issues/680)

## Download / ダウンロード

| OS | Arch | Asset |
|---|:-:|---:|
| macOS | arm64 | `tako-v0.6.2-macos-arm64.zip` |
| Windows | x64 | `tako-v0.6.2-windows-x64.zip` |

### Install / インストール手順

```sh
brew upgrade --cask tako
```

> Known limitations / 既知の制限
";

    fn info(version: &str, channel: Channel, notes: Option<&str>) -> UpdateInfo {
        UpdateInfo {
            version: version.into(),
            channel,
            html_url: "https://example.invalid/r".into(),
            download_url: Some("https://example.invalid/tako.zip".into()),
            asset_name: Some("tako.zip".into()),
            notes: notes.map(str::to_string),
        }
    }

    fn available(stable: Option<UpdateInfo>, test: Option<UpdateInfo>) -> UpdateState {
        UpdateState::Available(ChannelUpdates {
            stable,
            test,
            rate_limit_note: None,
        })
    }

    /// 番犬テスト（#690 受け入れ条件 1）: リリースノートが md の描画経路を通っている。
    ///
    /// 生テキスト表示（`info.notes` をそのまま child へ渡す旧実装）へ戻ると落ちる。
    /// **検査対象はテストモジュールより手前だけ**にする: このファイル全体を見ると
    /// 下の `assert!` に書いた探し文字列自身に当たってしまい、実装を消しても通る
    /// （実測で確認済み）。実行時側の保証はセルフテスト項目 90(f) が受け持つ
    #[test]
    fn リリースノートはmdレンダリング経路を通る() {
        let src = include_str!("update_window.rs");
        let impl_src = src
            .split("#[cfg(test)]")
            .next()
            .expect("実装部（テストモジュールより手前）");
        assert!(
            impl_src.contains("md_view::render_document"),
            "リリースノートが md_view の描画を通っていない（#690）"
        );
        assert!(
            impl_src.contains("preview::markdown_blocks"),
            "md のパースを preview::markdown_blocks（正）に任せていない"
        );
    }

    /// 表示対象は安定版が主、無ければテスト版（章題のバージョンもそれに従う）
    #[test]
    fn ノートは安定版優先でテスト版へ落ちる() {
        let stable = info("1.0.0", Channel::Stable, Some("stable notes"));
        let test = info("1.1.0-test.1", Channel::Test, Some("test notes"));

        let both = available(Some(stable.clone()), Some(test.clone()));
        let (picked, notes) = UpdateWindow::notes_source(&both).expect("両方あるとき");
        assert_eq!(picked.version, "1.0.0");
        assert_eq!(notes, "stable notes");

        let only_test = available(None, Some(test));
        let (picked, notes) = UpdateWindow::notes_source(&only_test).expect("テスト版のみ");
        assert_eq!(picked.version, "1.1.0-test.1");
        assert_eq!(notes, "test notes");

        // 更新なし・チェック前・更新中はノート欄そのものを出さない
        for state in [
            UpdateState::Idle,
            UpdateState::Updating("x".into()),
            UpdateState::CheckFailed("x".into()),
            available(None, None),
        ] {
            assert!(UpdateWindow::notes_source(&state).is_none());
        }

        // body 空のリリースは「ノートなし」の案内へ落ちる（パースしない）
        let empty = available(Some(info("1.0.0", Channel::Stable, None)), None);
        let (_, notes) = UpdateWindow::notes_source(&empty).expect("ノート空でも欄は出す");
        assert_eq!(notes, "");
    }

    /// 実物に近いノートが見出し・表・リスト・コード・引用・リンクとして解ける
    /// （受け入れ条件 1 の「レンダリングされる」の中身）
    #[test]
    fn 実リリースノートが見出しと表とリストとリンクに解ける() {
        let parsed = ParsedNotes::parse(RELEASE_NOTES);
        let kinds = &parsed.blocks;
        let heading_levels: Vec<u8> = kinds
            .iter()
            .filter_map(|b| match &b.kind {
                preview::MdBlockKind::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(heading_levels, vec![2, 2, 3], "見出しが解けていない");
        let table = kinds
            .iter()
            .find_map(|b| match &b.kind {
                preview::MdBlockKind::Table {
                    align,
                    header,
                    rows,
                } => Some((align.clone(), header.len(), rows.len())),
                _ => None,
            })
            .expect("ダウンロード表が解けていない");
        assert_eq!(table.1, 3, "表のヘッダが 3 列");
        assert_eq!(table.2, 2, "表の本文が 2 行");
        assert_eq!(
            table.0,
            vec![
                preview::MdAlign::None,
                preview::MdAlign::Center,
                preview::MdAlign::Right
            ],
            "列の配置指定が解けていない"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|b| matches!(b.kind, preview::MdBlockKind::ListItem { .. }))
                .count(),
            2,
            "リスト項目が解けていない"
        );
        assert!(
            kinds.iter().any(
                |b| matches!(&b.kind, preview::MdBlockKind::CodeBlock { lang, .. }
                    if lang.as_deref() == Some("sh"))
            ),
            "コードブロックが解けていない"
        );
        assert!(
            kinds.iter().any(|b| b.quote_depth > 0),
            "引用が解けていない"
        );
        // リンクは 1 本（http なので開ける）。索引はプレビューと同じ経路で作る
        assert_eq!(parsed.links.len(), 1);
        assert_eq!(
            parsed.links[0].url,
            "https://github.com/takushio2525/tako/issues/680"
        );
        assert!(tako_core::md_links::browser_url(&parsed.links[0].url).is_some());
    }

    /// エッジ（受け入れ条件 3）: 空・壊れた md・巨大なノートを解いても落ちない。
    /// 描画側の頑健性は `md_view` の単体テストが見る
    #[test]
    fn 空と壊れたmdと巨大なノートを解いても落ちない() {
        assert!(ParsedNotes::parse("").blocks.is_empty());
        let broken = "```\n| a |\n|--\n### \n[x](javascript:alert(1))\n> >\n";
        let parsed = ParsedNotes::parse(broken);
        // 開けない URL は当たり判定に載っていても browser_url で弾かれる
        assert!(parsed
            .links
            .iter()
            .all(|l| tako_core::md_links::browser_url(&l.url).is_none()));
        let big = RELEASE_NOTES.repeat(400);
        let parsed = ParsedNotes::parse(&big);
        assert_eq!(parsed.links.len(), 400);
        assert!(parsed.blocks.len() > 400);
    }
}

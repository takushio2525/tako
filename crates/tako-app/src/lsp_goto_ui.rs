//! 定義ジャンプの GUI（S3 / #1680）: ⌘ホバーの下線・⌘クリック・候補の一覧・ヘッダの一時表示
//!
//! 修飾は macOS = ⌘ / Windows = Ctrl（#763 の 1 実装 = `keybindings::link_modifier_active`）。
//!
//! ## 問い合わせと着地はここに無い
//!
//! ⌘クリックは CLI `tako lsp definition` / MCP `tako_lsp` と**同じ 3 段**を通る:
//! `tako_control::prepare_offload`（UI スレッドで引数の検査）→
//! `OffloadJob::run_staged`（background で位置を UTF-16 へ直し、言語サーバの起動と応答を待つ）→
//! `tako_control::finish_offload`（UI スレッドで着地 = `open_file` がジャンプ履歴へ積む）。
//! ここが持つのは画面だけ: どこに下線を引くか（検出は `tako_core::lsp::goto::symbol_at`）と、
//! 答え（見つからない・未導入・候補が複数…）の見せ方。
//!
//! 下線の装飾は Markdown のリンク（#680）と同じ `md_view::push_hovered_link_highlight`
//! の 1 実装（検出だけが別）。

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, Context, FontWeight, MouseButton, Pixels, Point, SharedString};
use serde_json::Value;
use tako_control::{LspGotoLanding, OffloadContinuation, OffloadOutcome};
use tako_core::{PaneId, PaneOrigin};

use crate::sidebar::{NoticeArea, NoticeArm};
use crate::{hsla, preview, rgba, LspGotoMenu, TakoApp};

/// 結果の一時表示（見つからない等）を出しておく時間
pub(crate) const GOTO_STATUS_FEEDBACK: Duration = Duration::from_millis(4000);

/// 候補の一覧の幅
const MENU_WIDTH: f32 = 440.0;

impl TakoApp {
    /// このペインで定義ジャンプ（⌘ホバー / ⌘クリック）を使うか。
    ///
    /// コード表示で、検出表にこの拡張子を受け持つサーバがあり、LSP が有効なときだけ。
    /// 受け持つサーバが無いファイルに下線を引くと「押しても何も起きない下線」になる
    pub(crate) fn lsp_goto_enabled_for(&self, pane: PaneId) -> bool {
        if tako_control::dispatch::lsp_goto_legacy() || !self.lsp.is_enabled() {
            return false;
        }
        self.previews.get(&pane).is_some_and(|p| {
            p.mode == preview::PreviewMode::Code
                && tako_core::lsp::servers::resolve(&p.path).is_some()
        })
    }

    /// マウス位置にある ⌘ホバーの対象（0 起点の行と、行内の UTF-8 バイト範囲）。
    ///
    /// **文字の上にあるときだけ**（`index_for_position` の `Ok` だけを採る。行末より右・行間は
    /// 対象外 = md リンクの当たり判定 #680 と同じ）。選択の当たり判定（近傍の行へ寄せる）とは別物
    pub(crate) fn code_symbol_at_position(
        &self,
        pane: PaneId,
        position: Point<Pixels>,
    ) -> Option<(usize, Range<usize>)> {
        self.code_symbol_hit(pane, position)
            .map(|(line, _, symbol)| (line, symbol.range))
    }

    /// [`Self::code_symbol_at_position`] の本体: 0 起点の行・押した文字の行内バイト・対象。
    /// 右クリックメニュー（#1684）は押したバイトをそのまま `tako lsp menu` と同じ準備へ渡す
    /// （選択の中かどうかを CLI / MCP と同じ位置で判定する）
    pub(crate) fn code_symbol_hit(
        &self,
        pane: PaneId,
        position: Point<Pixels>,
    ) -> Option<(usize, usize, tako_core::lsp::goto::Symbol)> {
        if !self.lsp_goto_enabled_for(pane) {
            return None;
        }
        let layouts = self.preview_text_layouts.get(&pane)?;
        let texts = self.preview_line_texts.get(&pane)?;
        for (line, layout) in layouts.iter().enumerate() {
            let Some(layout) = layout else {
                continue;
            };
            if !layout.bounds().contains(&position) {
                continue;
            }
            let byte = layout.index_for_position(position).ok()?;
            let symbol = tako_core::lsp::goto::symbol_at(texts.get(line)?, byte)?;
            return Some((line, byte, symbol));
        }
        None
    }

    /// ⌘ホバーの下線を更新する（描画済みのコードプレビューをすべて見る。フォーカスに依存しない）
    pub(crate) fn update_code_symbol_hover(
        &mut self,
        position: Point<Pixels>,
        cmd_held: bool,
        cx: &mut Context<Self>,
    ) {
        let found = if cmd_held {
            let panes: Vec<PaneId> = self.preview_text_layouts.keys().copied().collect();
            panes.into_iter().find_map(|pane| {
                self.code_symbol_at_position(pane, position)
                    .map(|(line, range)| (pane, line, range))
            })
        } else {
            None
        };
        if found != self.lsp_goto.hovered {
            self.lsp_goto.hovered = found;
            cx.notify();
        }
    }

    /// 描画用: この行でホバー中の範囲
    pub(crate) fn code_symbol_hovered_on(&self, pane: PaneId, line: usize) -> Option<Range<usize>> {
        self.lsp_goto
            .hovered
            .as_ref()
            .filter(|(p, l, _)| *p == pane && *l == line)
            .map(|(_, _, range)| range.clone())
    }

    /// 描画用: このペインのどこかをホバーしているか（ポインタの形を指にする）
    pub(crate) fn code_symbol_hovered_in(&self, pane: PaneId) -> bool {
        self.lsp_goto
            .hovered
            .as_ref()
            .is_some_and(|(p, _, _)| *p == pane)
    }

    /// ペインの定義ジャンプ状態を捨てる（ペイン削除・プレビュー差し替え）。
    /// 行番号は中身ごとに意味が変わるので、残すと別のファイルの行に下線が出る
    pub(crate) fn forget_lsp_goto(&mut self, pane: PaneId) {
        let ui = &mut self.lsp_goto;
        if ui.hovered.as_ref().is_some_and(|(p, _, _)| *p == pane) {
            ui.hovered = None;
        }
        if ui.menu.as_ref().is_some_and(|m| m.pane == pane) {
            ui.menu = None;
        }
        if ui.status.as_ref().is_some_and(|(p, ..)| *p == pane) {
            ui.status = None;
        }
    }

    /// ⌘クリックで定義へ飛ぶ。問い合わせは background で待ち、UI は止めない（上限は
    /// `tako_control::lsp::goto::goto_timeout`）。答えが返るまでに次を押したら古い答えは捨てる
    pub(crate) fn start_lsp_goto(
        &mut self,
        pane: PaneId,
        line: usize,
        column: usize,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let request = tako_control::protocol::Request::LspGoto {
            action: tako_core::lsp::goto::GotoKind::Definition.slug().into(),
            pane: Some(pane.as_u64()),
            line: line + 1,
            column,
            open: None,
            choice: None,
            // 利用者自身の操作なので、着地したペインへフォーカスを移す（CLI / MCP の既定は移さない）
            focus: Some(true),
        };
        self.start_lsp_goto_request(pane, request, anchor, cx);
    }

    /// 定義ジャンプの要求（4 種のどれでも）を ⌘クリックと同じ 3 段へ渡す。
    /// 右クリックメニューの「定義へ移動」等（#1684）もここを通る
    pub(crate) fn start_lsp_goto_request(
        &mut self,
        pane: PaneId,
        request: tako_control::protocol::Request,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.lsp_goto.hovered = None;
        self.lsp_goto.menu = None;
        let job = match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => job,
            Some(Err(e)) => {
                self.show_lsp_goto_status(pane, e.to_string(), true, cx);
                return;
            }
            None => return,
        };
        self.lsp_goto.seq = self.lsp_goto.seq.wrapping_add(1);
        let seq = self.lsp_goto.seq;
        self.lsp_goto.pending = Some(pane);
        self.lsp_goto.status = None;
        let staged = cx
            .background_executor()
            .spawn(async move { job.run_staged() });
        cx.spawn(async move |this, cx| {
            let outcome = staged.await;
            let _ = this.update(cx, |app, cx| {
                app.finish_lsp_goto_click(outcome, seq, pane, anchor, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_lsp_goto_click(
        &mut self,
        outcome: OffloadOutcome,
        seq: u64,
        pane: PaneId,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // 答えを待つあいだに次を押した = この答えは誰も待っていない
        if seq != self.lsp_goto.seq {
            return;
        }
        self.lsp_goto.pending = None;
        match outcome {
            OffloadOutcome::OnUi(next) => {
                let menu = match &next {
                    OffloadContinuation::LspGoto(landing) => {
                        Some((pane, anchor, (**landing).clone()))
                    }
                    // ⌘クリックは定義ジャンプの続きしか返さない（整形の続きは #1683 の入口から）
                    OffloadContinuation::LspFormat(_) => None,
                    // 定義ジャンプの問い合わせからは来ない（コピーの続き = #1860）
                    OffloadContinuation::FileCopied(_) => None,
                    // 定義ジャンプの問い合わせからは来ない（#1682 の補完は別の続き）
                    OffloadContinuation::LspCompletion(_) => None,
                    // 定義ジャンプの問い合わせからは来ない（#1681 のホバーは別の続き）
                    OffloadContinuation::LspHover(_) => None,
                };
                self.land_lsp_goto(next, menu, cx);
            }
            OffloadOutcome::Reply(result) => {
                if let Err(e) = result {
                    self.show_lsp_goto_status(pane, e.to_string(), true, cx);
                }
                cx.notify();
            }
        }
    }

    /// 候補の一覧から 1 つ選ぶ（`choice` は 1 始まり。CLI の `--choice` と同じ）。
    /// 問い合わせ直さず、同じ答えで着地し直す
    pub(crate) fn choose_lsp_goto(&mut self, choice: usize, cx: &mut Context<Self>) {
        let Some(menu) = self.lsp_goto.menu.take() else {
            return;
        };
        let mut landing = menu.landing;
        landing.choice = Some(choice);
        self.land_lsp_goto(OffloadContinuation::LspGoto(Box::new(landing)), None, cx);
    }

    /// 着地（dispatch の `finish_offload` の 1 実装を通す）と、答えの見せ方
    fn land_lsp_goto(
        &mut self,
        next: OffloadContinuation,
        menu: Option<(PaneId, Point<Pixels>, LspGotoLanding)>,
        cx: &mut Context<Self>,
    ) {
        let fallback_pane = menu.as_ref().map(|(pane, ..)| *pane);
        // 見えている窓は notify で描かれる（強制描画は隠れた窓の IPC 向け = #1370）
        let (result, _redraw) = self.finish_offload_on_ui(next, PaneOrigin::User, cx);
        match result {
            Ok(value) => self.present_lsp_goto(&value, menu, cx),
            Err(e) => {
                if let Some(pane) = fallback_pane {
                    self.show_lsp_goto_status(pane, e.to_string(), true, cx);
                }
                self.notify_ui_dispatch_failed(
                    NoticeArea::Preview,
                    NoticeArm::Issue1680,
                    crate::ui_text::preview::goto_op(),
                    None,
                    &e,
                );
            }
        }
        self.sync_filetree_roots();
        cx.notify();
    }

    /// 答えの見せ方: 見つかった → 何も出さない（飛んだこと自体が答え）/ 候補が複数 → 一覧 /
    /// それ以外 → その場（ヘッダ）に理由を出す。直す手のあるもの（未導入・落ちた・未応答…）は
    /// 通知欄にも理由と次の一手を出す（「見つからない」は通知欄を開かせるほどではない）
    fn present_lsp_goto(
        &mut self,
        value: &Value,
        menu: Option<(PaneId, Point<Pixels>, LspGotoLanding)>,
        cx: &mut Context<Self>,
    ) {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        let pane = value["from"]["pane"].as_u64().map(PaneId::from_raw);
        match value["status"].as_str() {
            Some("found") | None => {}
            Some("choose") => {
                if let Some((pane, anchor, landing)) = menu {
                    self.lsp_goto.menu = Some(LspGotoMenu {
                        pane,
                        anchor,
                        landing,
                    });
                }
            }
            Some(status) => {
                let reason = text("reason");
                let actionable = status != "not-found";
                if let Some(pane) = pane {
                    self.show_lsp_goto_status(pane, reason.clone(), actionable, cx);
                }
                if actionable {
                    self.notify_ui_failure(
                        NoticeArea::Preview,
                        NoticeArm::Issue1680,
                        crate::ui_text::preview::goto_op(),
                        status,
                        crate::ui_text::preview::goto_notice(&reason, &text("next_step")),
                    );
                }
            }
        }
    }

    /// ヘッダの一時表示を出す（定義ジャンプと整形 = #1683 が共有する 1 本）
    pub(crate) fn show_lsp_status(
        &mut self,
        pane: PaneId,
        message: String,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.show_lsp_goto_status(pane, message, is_error, cx);
    }

    /// ヘッダへ一時表示を出す（[`GOTO_STATUS_FEEDBACK`] で消える）
    fn show_lsp_goto_status(
        &mut self,
        pane: PaneId,
        message: String,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        let at = Instant::now();
        self.lsp_goto.status = Some((pane, message, is_error, at));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GOTO_STATUS_FEEDBACK).await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .lsp_goto
                    .status
                    .as_ref()
                    .is_some_and(|(.., shown)| *shown == at)
                {
                    app.lsp_goto.status = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// ヘッダの一時表示（問い合わせ中 > 結果）。`bool` は注意色で出すか
    pub(crate) fn lsp_goto_header_status(&self, pane: PaneId) -> Option<(String, bool)> {
        if self.lsp_goto.pending == Some(pane) {
            return Some((crate::ui_text::preview::goto_searching().to_string(), false));
        }
        // #1683: 整形の答えを待っているあいだ
        if self.lsp_format.pending == Some(pane) {
            return Some((crate::ui_text::preview::format_running().to_string(), false));
        }
        self.lsp_goto
            .status
            .as_ref()
            .filter(|(p, .., at)| *p == pane && at.elapsed() < GOTO_STATUS_FEEDBACK)
            .map(|(_, message, is_error, _)| (message.clone(), *is_error))
    }

    /// 候補の一覧（押した位置の下に出す。外を押すと閉じる）
    pub(crate) fn render_lsp_goto_menu(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.lsp_goto.menu.as_ref()?;
        let answer = menu.landing.answer.as_ref()?.as_ref().ok()?;
        let theme = self.theme.clone();
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        rows.push(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_size(px(10.5))
                .text_color(hsla(theme.text_faint))
                .child(SharedString::from(crate::ui_text::preview::goto_choose(
                    answer.targets.len(),
                )))
                .into_any_element(),
        );
        for (index, target) in answer.targets.iter().enumerate() {
            let choice = index + 1;
            let name = target
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| target.path.display().to_string());
            rows.push(
                div()
                    .id(("lsp-goto-choice", index))
                    .flex()
                    .flex_col()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.choose_lsp_goto(choice, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(11.5))
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(format!(
                                        "{name}:{}",
                                        target.line + 1
                                    ))),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.text_secondary))
                                    .child(SharedString::from(target.excerpt.clone())),
                            ),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(9.5))
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(target.path.display().to_string())),
                    )
                    .into_any_element(),
            );
        }
        let viewport = window.viewport_size();
        let left = (f32::from(menu.anchor.x) - 12.0)
            .min(f32::from(viewport.width) - MENU_WIDTH - 8.0)
            .max(0.0);
        Some(
            div()
                .id("lsp-goto-menu-dismiss")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.lsp_goto.menu = None;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("lsp-goto-menu")
                        .absolute()
                        .left(px(left))
                        .top(px(f32::from(menu.anchor.y) + 14.0))
                        .w(px(MENU_WIDTH))
                        .max_h(px(360.0))
                        .overflow_y_scroll()
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_default))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .p(px(6.0))
                        // 一覧の中の押下で外側の「閉じる」を走らせない
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}

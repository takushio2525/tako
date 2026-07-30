//! AI コマンド提案カードの描画と操作（FR-2.22 / Issue #666、表示位置は #681）
//!
//! AI が `tako_show_command` で渡したコマンドを、対象ペインのカードとして出す。
//! 表示は折り返してよいが、**コピー・実行に使うのは保管された論理文字列**なので
//! ペイン幅の影響を受けない（画面から拾ったコマンドが壊れる問題の根治）。
//!
//! 位置は**生成時点のターミナル内容にアンカー**する（#681）。下端固定オーバーレイは
//! claude の入力欄・フッターにちょうど被って実用にならなかったため、
//! ① カードの下端をライブ領域（入力欄・プロンプト）の上辺に合わせ
//! （上に空きが無い起動直後のシェルでは内容の直後へ回す）、
//! ② 出力が流れた / スクロールした分だけ内容と一緒に動かす。
//!
//! ボタンの実体は dispatch（`Request::ShowCommand`）を呼ぶだけ。CLI / MCP と同じ経路を
//! 通るので、UI 操作と AI 操作で挙動が食い違わない（開発不変条件）。

use gpui::{div, prelude::*, px, svg, Context, MouseButton, MouseDownEvent, SharedString};
use tako_core::PaneId;

use super::*;
use crate::file_icons::ui_icon;

/// コピー成功・失敗の表示を維持する時間。2 秒ポーリング（periodic）の再描画で自然に消える
const FEEDBACK_DURATION: std::time::Duration = std::time::Duration::from_millis(2200);

/// コマンド 1 件の本文の最大表示高さ。長いコマンドは折り返して全文出すが、
/// 極端に長いものでターミナルを覆い尽くさないよう高さで止める（スクロールで読める）
const COMMAND_MAX_HEIGHT: f32 = 190.0;

/// アンカーが上端へ近づいてもカードを潰さずに保つ高さ（#681）。ここを下回る空きしか
/// 無いときはスタックの高さを縮めず、テキスト領域の上端で**切り取られる**に任せる
/// （縮めると流れ去る途中でカードが畳まれて見え、内容と一緒に動いて見えない）
const CARD_MIN_VIEW_HEIGHT: f32 = 120.0;

/// ポート検知チップ（FR-2.4.3）が出ているときの下端最小余白
const CHIP_BOTTOM_MARGIN: f32 = 30.0;

/// 過去へスクロールし切ってカードが完全に画面外へ出たと判断する余裕（px 相当）。
/// カードの実高さは GPUI のレイアウト後にしか分からないので、
/// 上限（COMMAND_MAX_HEIGHT + 装飾）より広く取って「まだ一部見えている」を消さない
const OFF_SCREEN_SLACK: f32 = 320.0;

/// ペイン寸法が変わったあと、アンカーを取り直し続ける時間（#681）。
/// TUI は resize の**通知を受けてから**画面を作り直すので、寸法が変わったフレームで
/// 1 度採り直すだけでは作り直し前の画面を見てしまう（実機で claude の入力欄を
/// カードが覆う症状として確認）。落ち着くまで数フレーム追随させる
const ANCHOR_SETTLE: std::time::Duration = std::time::Duration::from_millis(1200);

/// カードのアンカー状態（#681）。ペインの前提が変わったら取り直す
#[derive(Debug, Clone, Copy)]
pub(crate) struct CardAnchorState {
    anchor: tako_core::command_card::CardAnchor,
    /// 内容の下へ置くか（生成時に 1 度だけ決める）。毎フレーム決め直すと、
    /// 出力が増えて上の空きが広がった瞬間に上下へ飛ぶ
    below: bool,
    /// 採取時の alt screen 状態。TUI の起動・終了で画面が総取り替えされるため、
    /// 変わったらアンカーを取り直す（行番号の意味が失われる）
    alt: bool,
    /// 採取時のペイン寸法（cols, rows）。幅が変われば行が reflow し、
    /// 高さが変われば履歴行数が動くのでアンカーを取り直す
    size: (usize, usize),
    /// この時刻まではフレームごとに取り直す（寸法変化直後の TUI 再描画待ち）
    settle_until: Option<std::time::Instant>,
}

impl TakoApp {
    /// カードのアンカーを採取する（#681）。ライブ領域（入力欄・プロンプト）の上辺を
    /// スクロール位置 0 の座標へ正規化して持つ。
    ///
    /// カーソル行は `Screen::ime_cursor` から取る（claude 等は DECTCEM でカーソルを
    /// 消していることがあり `cursor` は None になる。#497 と同じ理由）
    fn capture_card_anchor(
        &self,
        pane_id: PaneId,
        layout: &tako_core::command_card::CardLayout,
        settle_until: Option<std::time::Instant>,
    ) -> Option<CardAnchorState> {
        use tako_core::command_card::{content_tail_row, live_region_top, CardAnchor};
        let session = self.terminals.get(&pane_id)?;
        let screen = session.screen_opts(&self.theme, false);
        let texts: Vec<&str> = screen.lines.iter().map(|l| l.text.as_str()).collect();
        let cursor_row = screen.ime_cursor.map(|(_, row)| row);
        let live_top = live_region_top(&texts, cursor_row);
        let tail = content_tail_row(&texts);
        // ime_cursor / 行番号は「描画されている行」なのでスクロール中は
        // display_offset ぶん下にずれている。位置 0 の座標へ戻して持つ
        let offset = screen.display_offset as f32;
        let anchor = CardAnchor {
            base_row: live_top as f32 - offset,
            tail_row: tail as f32 - offset,
            base_history: session.history_size(),
        };
        Some(CardAnchorState {
            below: CardAnchor::prefers_below(anchor.base_row, anchor.tail_row, layout),
            anchor,
            alt: session.is_alt_screen(),
            size: session.size(),
            settle_until,
        })
    }

    /// 現在のスクロール遡り量（行。0.0 = 最下部）。バックエンド / TmuxOpen ビューペインは
    /// tmux 履歴のローカルミラー、直接ペインはセッションの表示位置（#159 と同じ出し分け）
    fn card_scrolled_back(&self, pane_id: PaneId) -> f32 {
        self.scroll_ctls
            .get(&pane_id)
            .and_then(|c| c.mirror.as_ref())
            .map(|m| m.effective_position())
            .or_else(|| self.terminals.get(&pane_id).map(|s| s.scroll_position()))
            .unwrap_or(0.0)
    }

    /// カードスタックの配置（#681）。スタックは**最新カードのアンカー**に付く
    /// （古いカードはその上に積む）。アンカーを持てないペイン（セッションなし）では
    /// None = 呼び出し側が従来の下端配置へ落ちる。
    ///
    /// セルフテストからも呼ぶ（機械検証: 入力欄に被らない / スクロールで内容と一緒に動く）
    pub(crate) fn command_card_placement(
        &mut self,
        pane_id: PaneId,
        layout: &tako_core::command_card::CardLayout,
    ) -> Option<(
        CardAnchorState,
        Option<tako_core::command_card::CardPlacement>,
    )> {
        let newest = self.command_cards.latest_for(pane_id)?.id().as_u64();
        let session = self.terminals.get(&pane_id)?;
        let (alt, size, history) = (
            session.is_alt_screen(),
            session.size(),
            session.history_size(),
        );
        // 取り直す条件: 未採取 / alt screen 切替 / 寸法変化 / 寸法変化直後の追随中
        let now = std::time::Instant::now();
        let settle = match self.card_anchors.get(&newest) {
            None => Some(None),
            Some(s) if s.alt != alt || s.size != size => Some(Some(now + ANCHOR_SETTLE)),
            // TUI の作り直しが終わるまでは同じ猶予を保ったまま追随する
            Some(s) if s.settle_until.is_some_and(|t| t > now) => Some(s.settle_until),
            Some(_) => None,
        };
        if let Some(settle_until) = settle {
            let fresh = self.capture_card_anchor(pane_id, layout, settle_until)?;
            self.card_anchors.insert(newest, fresh);
        }
        let state = *self.card_anchors.get(&newest)?;
        let scrolled_back = self.card_scrolled_back(pane_id);
        let placement = state
            .anchor
            .place(state.below, history, scrolled_back, layout);
        Some((state, placement))
    }

    /// ライブ領域上辺の現在の描画行（アンカーの採取・更新も行う）。
    /// セルフテストが「入力欄より上か」「スクロールで動くか」を見るのに使う
    pub(crate) fn command_card_anchor_row(
        &mut self,
        pane_id: PaneId,
        layout: &tako_core::command_card::CardLayout,
    ) -> Option<(f32, bool)> {
        let (state, _) = self.command_card_placement(pane_id, layout)?;
        let history = self.terminals.get(&pane_id)?.history_size();
        let row = state
            .anchor
            .viewport_row(history, self.card_scrolled_back(pane_id));
        Some((row, state.below))
    }

    /// 描画に使うレイアウト寸法（#681）。セルフテストも同じ値で判定する
    pub(crate) fn card_layout(
        area_height: f32,
        cell_height: f32,
        chip_present: bool,
    ) -> tako_core::command_card::CardLayout {
        tako_core::command_card::CardLayout {
            cell_height: cell_height.max(1.0),
            area_height: area_height.max(0.0),
            padding: PANE_PADDING,
            min_bottom: if chip_present {
                CHIP_BOTTOM_MARGIN
            } else {
                PANE_PADDING
            },
            min_height: CARD_MIN_VIEW_HEIGHT,
            slack: OFF_SCREEN_SLACK,
        }
    }

    /// 指定ペインのコマンドカード（FR-2.22）。**生成時点のターミナル内容にアンカー**し
    /// （#681）、新しいものを下・古いものを上にして積む。
    ///
    /// `area_height` はテキスト領域の高さ、`cell_height` は 1 行の高さ、
    /// `chip_present` はポート検知チップ（FR-2.4.3）の有無。
    /// 返す要素は**テキスト領域の中**へ入れる（領域の上端で切り取られることで
    /// 「上へ流れて消える」見え方になり、ペインヘッダを覆わない）
    pub(crate) fn render_command_cards(
        &mut self,
        pane_id: PaneId,
        area_height: f32,
        cell_height: f32,
        chip_present: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = self.theme.clone();
        // 表示に必要な情報だけ取り出す（描画中に保管庫を借り続けない）
        let cards: Vec<(u64, Option<String>, Vec<String>)> = self
            .command_cards
            .list(Some(pane_id))
            .into_iter()
            .map(|c| {
                (
                    c.id().as_u64(),
                    c.label().map(str::to_string),
                    c.commands().to_vec(),
                )
            })
            .collect();
        if cards.is_empty() {
            return None;
        }
        // アンカー → 配置。アンカーを持てないペイン（セッションなし）は
        // 従来（#666）どおり下端へ置く
        let layout = Self::card_layout(area_height, cell_height, chip_present);
        let placement = match self.command_card_placement(pane_id, &layout) {
            // 流れ去った / 遡り切ったカードは描かない（保管庫には残るので
            // CLI / MCP の copy / run / dismiss は従来どおり効く）
            Some((_, None)) => return None,
            Some((_, Some(p))) => p,
            None => tako_core::command_card::CardPlacement::Above {
                space: (layout.area_height + layout.padding - layout.min_bottom).max(0.0),
            },
        };
        // カードを置ける高さ。潰れて見えるより切り取られる方が「流れ去る」見え方に
        // 近いので、下限を設けてそれ以上は縮めない
        let space = match placement {
            tako_core::command_card::CardPlacement::Above { space } => space,
            tako_core::command_card::CardPlacement::Below { top } => {
                (layout.area_height + layout.padding - top).max(0.0)
            }
        };
        let max_height = space.max(CARD_MIN_VIEW_HEIGHT);
        let copied = self
            .command_card_copied
            .filter(|(_, _, at)| at.elapsed() < FEEDBACK_DURATION);
        let errored = self
            .command_card_error
            .filter(|(_, at)| at.elapsed() < FEEDBACK_DURATION);

        // 本文はペインの半分までに抑える（1 枚でもボタン行が押し出されないように）
        let command_max = COMMAND_MAX_HEIGHT.min((max_height * 0.5).max(48.0));
        let stack = div()
            .id(("command-card-stack", pane_id.as_u64()))
            .absolute()
            // アンカー行に貼り付ける（#681）。上配置は「上端からアンカー行までの高さを
            // 持つ器」の下端へ、下配置は上端からの距離で置く。どちらも
            // 「上端 + 行 × 行高」基準なので、行スタックと必ず同じ行に載る
            .map(|d| match placement {
                tako_core::command_card::CardPlacement::Above { .. } => d.bottom(px(0.0)),
                tako_core::command_card::CardPlacement::Below { top } => d.top(px(top)),
            })
            .left(px(8.0))
            .right(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            // アンカーの側に収める。溢れた古いカードはスクロールで読める
            // （流れ去る途中はテキスト領域の端で切り取られる。#681）
            .max_h(px(max_height))
            .overflow_y_scroll()
            // 下のペインへ選択・スクロールを漏らさない（提案チップと同じ）
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .children(cards.into_iter().rev().map(|(card_id, label, commands)| {
                let total = commands.len();
                let heading =
                    label.unwrap_or_else(|| crate::ui_text::command_card::heading().to_string());
                div()
                    .id(("command-card", card_id))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(9.0))
                    .bg(rgba(theme.surface_1))
                    .border_1()
                    .border_color(hsla(theme.accent_border_muted))
                    .shadow_sm()
                    // 見出し行（説明ラベル + × 閉じる）
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.text_secondary))
                                    .child(SharedString::from(heading)),
                            )
                            .when(errored.is_some_and(|(id, _)| id == card_id), |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.red))
                                        .child(crate::ui_text::command_card::run_failed()),
                                )
                            })
                            .child(
                                div()
                                    .id(("command-card-close", card_id))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.dismiss_command_card(card_id, cx);
                                    }))
                                    .child(
                                        svg()
                                            .path(ui_icon::CLOSE)
                                            .w(px(9.0))
                                            .h(px(9.0))
                                            .text_color(hsla(theme.text_muted)),
                                    ),
                            ),
                    )
                    // コマンド本体（等幅 + 背景パネル。折り返して全文を出す）
                    .children(commands.into_iter().enumerate().map(|(i, command)| {
                        let index = i + 1;
                        let is_copied =
                            copied.is_some_and(|(id, idx, _)| id == card_id && idx == index);
                        let theme = theme.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .p(px(7.0))
                            .rounded(px(6.0))
                            .bg(rgba(theme.crust))
                            .border_1()
                            .border_color(hsla(theme.border_subtle))
                            .child(
                                div()
                                    .id(("command-card-text", card_id * 100 + index as u64))
                                    .max_h(px(command_max))
                                    // 極端に長いコマンドは高さで止めてスクロールで読ませる
                                    .overflow_y_scroll()
                                    // 折り返して全文表示（コピーは論理文字列なので無関係）
                                    .whitespace_normal()
                                    .font_family(theme.font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(command)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    // 複数コマンドのときだけ番号を出す
                                    .when(total > 1, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(hsla(theme.text_muted))
                                                .child(SharedString::from(
                                                    crate::ui_text::command_card::index_label(
                                                        index, total,
                                                    ),
                                                )),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(self.command_card_button(
                                        ("command-card-copy", (card_id * 100 + index as u64)),
                                        if is_copied {
                                            ui_icon::CHECK
                                        } else {
                                            ui_icon::COPY
                                        },
                                        if is_copied {
                                            crate::ui_text::command_card::copied()
                                        } else {
                                            crate::ui_text::command_card::copy()
                                        },
                                        is_copied,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.copy_command_card(card_id, index, cx);
                                        }),
                                    ))
                                    .child(self.command_card_button(
                                        ("command-card-run", (card_id * 100 + index as u64)),
                                        ui_icon::PLAY,
                                        crate::ui_text::command_card::run(),
                                        false,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.run_command_card(card_id, index, cx);
                                        }),
                                    )),
                            )
                    }))
            }));
        // 上配置は「テキスト領域上端からアンカー行の上端まで」の高さを持つ透明な器へ
        // 入れ、その下端にスタックを貼る。器の高さは行スタックと同じ基準で決まるので、
        // レイアウト計算値（area_height）が実コンテナ高さとずれていても行を外さない。
        // 器より高いカードは器の上へ溢れ、テキスト領域の overflow_hidden で切り取られる
        // = 上へ流れて消える見え方になる
        Some(match placement {
            tako_core::command_card::CardPlacement::Above { space } => div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(space.max(0.0)))
                .child(stack)
                .into_any_element(),
            tako_core::command_card::CardPlacement::Below { .. } => stack.into_any_element(),
        })
    }

    /// カードのボタン（アイコン + ラベル）。狭いペインでも押せるよう高さは固定し、
    /// 並びは flex_wrap で折り返す
    fn command_card_button(
        &self,
        id: (&'static str, u64),
        icon: &'static str,
        label: &'static str,
        active: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let (fg, border) = if active {
            (theme.green, theme.green)
        } else {
            (theme.text_secondary, theme.border_default)
        };
        div()
            .id(id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .px(px(7.0))
            .h(px(20.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .bg(rgba(theme.chip_surface))
            .border_1()
            .border_color(hsla(border))
            .text_size(px(10.5))
            .text_color(hsla(fg))
            .hover(|d| {
                d.bg(rgba(theme.surface_hover))
                    .border_color(hsla(theme.accent))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(hsla(fg)),
            )
            .child(label)
    }

    /// 「コピー」= dispatch 経由でクリップボードへ（CLI / MCP の copy と同一経路）
    pub(crate) fn copy_command_card(&mut self, card: u64, index: usize, cx: &mut Context<Self>) {
        match self.command_card_dispatch("copy", card, index, cx) {
            Ok(_) => {
                self.command_card_copied = Some((card, index, std::time::Instant::now()));
                self.command_card_error = None;
            }
            Err(e) => self.report_command_card_error(card, "copy", &e),
        }
        cx.notify();
    }

    /// 「新規ペインで実行」= dispatch 経由で同じタブに分割して実行
    pub(crate) fn run_command_card(&mut self, card: u64, index: usize, cx: &mut Context<Self>) {
        match self.command_card_dispatch("run", card, index, cx) {
            Ok(_) => self.command_card_error = None,
            Err(e) => self.report_command_card_error(card, "run", &e),
        }
        cx.notify();
    }

    /// × 閉じる
    pub(crate) fn dismiss_command_card(&mut self, card: u64, cx: &mut Context<Self>) {
        if let Err(e) = self.command_card_dispatch("dismiss", card, 1, cx) {
            self.report_command_card_error(card, "dismiss", &e);
        }
        if self
            .command_card_copied
            .is_some_and(|(id, _, _)| id == card)
        {
            self.command_card_copied = None;
        }
        self.prune_card_anchors();
        cx.notify();
    }

    /// カード操作の dispatch 呼び出し（copy / run / dismiss 共通）
    fn command_card_dispatch(
        &mut self,
        action: &str,
        card: u64,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::ShowCommand {
                action: Some(action.to_string()),
                commands: Vec::new(),
                label: None,
                pane: None,
                card: Some(card),
                index: Some(index),
                focus: None,
            },
            PaneOrigin::User,
        )
        .map_err(|e| e.to_string());
        // UI から dispatch を直接呼ぶので、IPC / MCP ループがやっている後処理を
        // ここで肩代わりする。**これを欠くと run でツリーにペインだけができて
        // PTY が起動しない**（#153 で同じ穴を踏んでいる）
        for (pane, options) in std::mem::take(&mut self.pending_attach) {
            if let Err(e) = self.spawn_session(pane, options, cx) {
                eprintln!("warning: コマンドカードの実行ペインを起動できない: {e}");
                self.remove_pane(pane, cx);
            }
        }
        for (pane, data) in std::mem::take(&mut self.pending_writes) {
            if let Some(session) = self.terminals.get(&pane) {
                session.write(data);
            }
        }
        // コピーは押した瞬間に効いてほしいので、render を待たずここで流す
        self.flush_pending_clipboard(cx);
        result
    }

    /// 失敗は画面に一言 + 理由は診断ログへ（dispatch のエラー文は日本語固定 =
    /// UI 文言の i18n 対象外なのでそのまま画面には出さない）
    fn report_command_card_error(&mut self, card: u64, action: &str, reason: &str) {
        eprintln!("warning: コマンドカードの {action} に失敗: {reason}");
        self.command_card_error = Some((card, std::time::Instant::now()));
    }

    /// クリップボード書き込みの保留分を流す（render から呼ぶ）。
    /// GPUI の clipboard API は `App` を要するため dispatch では積むだけにしている
    pub(crate) fn flush_pending_clipboard(&mut self, cx: &mut Context<Self>) {
        for text in std::mem::take(&mut self.pending_clipboard) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// 閉じたペインのカードを掃除する（2 秒ポーリングから呼ぶ）。
    /// カードは揮発なので、ペインが消えたら残しておく意味がない
    pub(crate) fn prune_command_cards(&mut self) {
        if self.command_cards.is_empty() {
            self.card_anchors.clear();
            return;
        }
        let mut alive: std::collections::HashSet<PaneId> = std::collections::HashSet::new();
        for tab in self.workspace.tabs() {
            alive.extend(tab.tree().panes().iter().map(|p| p.id()));
        }
        // バックグラウンド退避中（FR-2.15）のペインは生きている = カードも残す
        alive.extend(self.workspace.shelved_panes().iter().map(|s| s.pane().id()));
        self.command_cards.retain_panes(|p| alive.contains(&p));
        self.prune_card_anchors();
    }

    /// 消えたカードのアンカー（#681）を落とす。カードと同じ寿命に保つ
    fn prune_card_anchors(&mut self) {
        if self.card_anchors.is_empty() {
            return;
        }
        let live: std::collections::HashSet<u64> = self
            .command_cards
            .list(None)
            .iter()
            .map(|c| c.id().as_u64())
            .collect();
        self.card_anchors.retain(|id, _| live.contains(id));
    }
}

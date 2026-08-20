//! AI コマンド提案カードの描画と操作（FR-2.22 / Issue #666、表示位置は #703）
//!
//! AI が `tako_show_command` で渡したコマンドを、対象ペインのカードとして出す。
//! 表示は折り返してよいが、**コピー・実行に使うのは保管された論理文字列**なので
//! ペイン幅の影響を受けない（画面から拾ったコマンドが壊れる問題の根治）。
//!
//! 位置は**ターミナル領域の下に作る専用帯**（#703）。オーバーレイ（#666 の下端固定 /
//! #681 の内容アンカー）はどちらも「ターミナルが描いている絵の上に重ねる」方式なので、
//! claude のように画面全体を使う TUI では会話文が隠れる。カードを出しているあいだは
//! **PTY の行数を帯のぶんだけ減らして**ターミナル領域そのものを縮め、帯をその外側に置く。
//! 重なりは座標計算ではなく**レイアウトの不変条件**としてゼロになる。
//!
//! ボタンの実体は dispatch（`Request::ShowCommand`）を呼ぶだけ。CLI / MCP と同じ経路を
//! 通るので、UI 操作と AI 操作で挙動が食い違わない（開発不変条件）。

use gpui::{canvas, div, prelude::*, px, svg, Context, MouseButton, MouseDownEvent, SharedString};
use tako_core::PaneId;

use super::*;
use crate::file_icons::ui_icon;

/// コピー成功・失敗の表示を維持する時間。2 秒ポーリング（periodic）の再描画で自然に消える
pub(crate) const FEEDBACK_DURATION: std::time::Duration = std::time::Duration::from_millis(2200);

/// コマンド 1 件の本文の最大表示高さ。長いコマンドは折り返して全文出すが、
/// 極端に長いものが帯を占有しないよう高さで止める（帯の中でスクロールして読める）
const COMMAND_MAX_HEIGHT: f32 = 190.0;

/// 帯の上辺の区切り線（ターミナル領域との境界）
const BAND_BORDER: f32 = 1.0;

/// 帯の内側の余白（上下）
const BAND_PADDING_V: f32 = 5.0;

/// 帯の内側の余白（左右）
const BAND_PADDING_H: f32 = 8.0;

/// カードとカードの間隔
const CARD_GAP: f32 = 6.0;

/// 高さの初回推定に使う UI テキスト 1 行の高さ（px）。実描画で採取できたら
/// そちらが正になるので、ここは「1 フレームだけ使う当たり」で足りる
const ESTIMATE_LINE: f32 = 15.0;

/// 同じく、等幅 11.5px の 1 文字あたりの幅（px）
const ESTIMATE_MONO_CHAR: f32 = 6.9;

/// カード 1 枚の枠・パディング（見出し行を除く）
const ESTIMATE_CARD_CHROME: f32 = 2.0 + 16.0 + 4.0;

/// コマンド 1 件のブロックの枠・パディング・ボタン行
const ESTIMATE_BLOCK_CHROME: f32 = 2.0 + 14.0 + 4.0 + 20.0 + 4.0;

/// カード帯の高さ採取スロット（#703）。
///
/// **エンティティを触らない**のが要点（#684 の `PaneContentProbe` と同じ理由）。
/// 描画は root view が貸し出されている最中に走ることがあり、そこで `Entity::update` を
/// 呼ぶとプロセスごと abort する。`Cell` への書き込みだけなら借用に触れない
#[derive(Default)]
pub(crate) struct CardBandProbe {
    /// 直近の実描画で採取した (署名, カードスタックの高さ px)
    measured: std::cell::Cell<Option<(u64, f32)>>,
    /// 再描画を要求済みの署名（同じ状態で何度も要求しないための安全弁）
    requested: std::cell::Cell<Option<u64>>,
}

/// このフレームでカード帯に割り当てた寸法（#703）。
/// `render()`（ペイン矩形の計算）と `render_pane`（帯の描画）が同じ値を見るための受け渡し
#[derive(Debug, Clone, Copy)]
pub(crate) struct CardBandFrame {
    /// テキスト領域から差し引いた帯の高さ（px）。0 なら帯なし
    pub(crate) height: f32,
    /// 高さを決めるのに使った署名（実描画の採取値と突き合わせる）
    signature: u64,
    /// 帯を差し引く前のテキスト領域の高さ（px）と 1 行の高さ（px）。
    /// 採取値から「行数が変わるか」を**プローブの中で**判定するために持ち回る
    area_height: f32,
    cell_height: f32,
}

impl CardBandFrame {
    /// このフレームで割り当てた帯の行数
    fn rows(&self) -> usize {
        (self.height / self.cell_height.max(1.0)).round() as usize
    }
}

impl TakoApp {
    /// テキスト領域から差し引くカード帯の高さ（px。#703）。
    ///
    /// `render()` がペイン矩形（`pane_text_areas` = PTY 行数・マウス座標変換・IME 位置の正）を
    /// 作る**前**に呼ぶ。ここで引いた高さがそのまま「ターミナルに見せない領域」になり、
    /// 帯はその外側へ描かれる。
    ///
    /// - `area_height` / `area_width`: 帯を差し引く**前**のテキスト領域の寸法
    /// - 戻り値 0 = 帯なし（ターミナル領域は 1px も削らない = カード無しと同じ見た目）
    pub(crate) fn card_band_height(
        &mut self,
        pane_id: PaneId,
        area_height: f32,
        area_width: f32,
        cell_height: f32,
    ) -> f32 {
        // 速い道: カードが 1 枚も無いフレーム（ほとんどがこれ）は bool 1 個で抜ける。
        // ここはペイン × フレームで必ず通るので、既存経路への影響をゼロに寄せる
        if self.command_cards.is_empty() {
            if !self.card_bands.is_empty() {
                self.card_bands.clear();
                self.card_band_probes.clear();
            }
            return 0.0;
        }
        // このペインがターミナルとして描かれない（プレビュー・Web ビュー・スターター）なら
        // 帯は作らない。領域を削っても描き先が無い
        if !self.pane_shows_terminal(pane_id) {
            self.card_bands.remove(&pane_id);
            return 0.0;
        }
        let cards = self.command_card_rows(pane_id);
        if cards.is_empty() {
            self.card_bands.remove(&pane_id);
            self.card_band_probes.remove(&pane_id);
            return 0.0;
        }
        // 署名 = カードの内容と描画幅。**これが変わったときだけ高さを測り直す**ので、
        // コピー成功表示のようにカード内の見た目だけが変わっても PTY resize は起きない
        let signature = card_band_signature(&cards, area_width);
        let measured = self
            .card_band_probes
            .get(&pane_id)
            .and_then(|p| p.measured.get())
            .filter(|(sig, _)| *sig == signature)
            .map(|(_, h)| h);
        // 実描画からの採取があればそれが正。無い（= 出現した最初のフレーム）あいだは推定値
        let desired = measured.unwrap_or_else(|| estimate_card_stack_height(&cards, area_width))
            + band_chrome();
        let rows = tako_core::command_card::band_rows(desired, area_height, cell_height);
        let height = tako_core::command_card::band_height(rows, cell_height);
        self.card_bands.insert(
            pane_id,
            CardBandFrame {
                height,
                signature,
                area_height,
                cell_height,
            },
        );
        height
    }

    /// このペインがターミナルとして描かれるか（帯を作ってよいか）。
    /// `render_pane` が早期 return する経路（Web ビュー / プレビュー / スターター）を除く
    pub(crate) fn pane_shows_terminal(&self, pane_id: PaneId) -> bool {
        if self.webviews.iter().any(|e| e.pane == Some(pane_id))
            || self.previews.contains_key(&pane_id)
        {
            return false;
        }
        matches!(
            self.pane_display_for(pane_id),
            tako_core::ui_mode::PaneDisplay::Terminal
        )
    }

    /// 描画に必要な情報だけ取り出す（描画中に保管庫を借り続けない）
    pub(crate) fn command_card_rows(
        &self,
        pane_id: PaneId,
    ) -> Vec<(u64, Option<String>, Vec<String>)> {
        self.command_cards
            .list(Some(pane_id))
            .into_iter()
            .map(|c| {
                (
                    c.id().as_u64(),
                    c.label().map(str::to_string),
                    c.commands().to_vec(),
                )
            })
            .collect()
    }

    /// 指定ペインのコマンドカード帯（FR-2.22 / #703）。
    ///
    /// `band_height` は [`Self::card_band_height`] が決めた高さで、**テキスト領域は
    /// すでにこのぶん縮めてある**。返す要素はテキスト領域の**外**（下）に置く兄弟要素
    pub(crate) fn render_command_card_band(
        &mut self,
        pane_id: PaneId,
        band_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if band_height <= 0.0 {
            return None;
        }
        let theme = self.theme.clone();
        let cards = self.command_card_rows(pane_id);
        if cards.is_empty() {
            return None;
        }
        let copied = self
            .command_card_copied
            .filter(|(_, _, at)| at.elapsed() < FEEDBACK_DURATION);
        let errored = self
            .command_card_error
            .filter(|(_, at)| at.elapsed() < FEEDBACK_DURATION);

        // 高さの実測（#703）。**測る器にはパディングも枠も付けない**:
        // 付けると「実測値に何を足せば帯の高さになるか」がボックスモデルの解釈依存になり、
        // 数 px ずれたまま毎フレーム測り直す羽目になる。余白は外側の器に持たせ、
        // 足し戻す量は `band_chrome()` の 1 箇所で定義する
        let probe = self.card_band_probe(pane_id, cx);
        // 新しいカードほど上（帯が溢れてスクロールしても最新のものが見えている）
        let stack = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .children(cards.iter().rev().map(|(card_id, label, commands)| {
                self.render_command_card(
                    *card_id,
                    label.clone(),
                    commands.clone(),
                    copied,
                    errored,
                    cx,
                )
            }))
            .child(probe);

        Some(
            div()
                .flex_none()
                .w_full()
                .h(px(band_height))
                .bg(rgba(theme.background))
                .overflow_hidden()
                .flex()
                .flex_col()
                // 下のペインへ選択・スクロールを漏らさない（提案チップと同じ）
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                // ターミナル領域との境界線。`border_t` ではなく実体の 1px 行にするのは、
                // 枠が高さに含まれるかどうかの解釈差で帯が 1px ずれないようにするため
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(px(BAND_BORDER))
                        .bg(hsla(theme.border_subtle)),
                )
                .child(
                    div()
                        .id(("command-card-band", pane_id.as_u64()))
                        .flex_1()
                        // 上限を超えたカードは帯の中でスクロールして読む
                        .overflow_y_scroll()
                        .child(
                            div()
                                // 高さ固定の帯の中では、縮まない指定をしないと flex が
                                // 中身を潰す（#656 で踏んだのと同じ罠）
                                .flex_shrink_0()
                                .px(px(BAND_PADDING_H))
                                .py(px(BAND_PADDING_V))
                                .child(stack),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 帯に要る高さを実描画から採取するプローブ（#703）。
    /// 採取値が今フレームの割り当てと食い違うときだけ次フレームを起こす（収束したら何もしない）
    fn card_band_probe(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> impl IntoElement {
        // 署名と割り当て高さは `card_band_height` が同じフレームで決めた値をそのまま使う
        // （描画側で作り直すと、幅の取り方が 1 箇所ずれただけで永久に測り直し続ける）
        let frame = self.card_bands.get(&pane_id).copied();
        let slot = self.card_band_probes.entry(pane_id).or_default().clone();
        let weak = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let Some(frame) = frame else { return };
                let height = f32::from(bounds.size.height);
                slot.measured.set(Some((frame.signature, height)));
                // 採取値で行数が変わらないなら再描画は不要（毎フレーム notify しない）。
                // 足りない側だけでなく余っている側も見る = 推定が過大でも次フレームで縮む。
                // 上限で頭打ちのときは行数が変わらないのでここで止まる
                let want = tako_core::command_card::band_rows(
                    height + band_chrome(),
                    frame.area_height,
                    frame.cell_height,
                );
                if want == frame.rows() {
                    slot.requested.set(None);
                    return;
                }
                if slot.requested.get() == Some(frame.signature) {
                    return;
                }
                slot.requested.set(Some(frame.signature));
                // 描画中の notify は GPUI が捨てるので effect の flush まで遅らせる（#684）
                let weak = weak.clone();
                cx.defer(move |cx| {
                    if let Some(entity) = weak.upgrade() {
                        entity.update(cx, |_, cx| cx.notify());
                    }
                });
            },
            |_, _, _, _| (),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// カード 1 枚
    fn render_command_card(
        &self,
        card_id: u64,
        label: Option<String>,
        commands: Vec<String>,
        copied: Option<(u64, usize, std::time::Instant)>,
        errored: Option<(u64, std::time::Instant)>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let total = commands.len();
        let heading = label.unwrap_or_else(|| crate::ui_text::command_card::heading().to_string());
        div()
            .id(("command-card", card_id))
            .flex_shrink_0()
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
                let is_copied = copied.is_some_and(|(id, idx, _)| id == card_id && idx == index);
                let theme = theme.clone();
                div()
                    .flex_shrink_0()
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
                            .max_h(px(COMMAND_MAX_HEIGHT))
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
                                            crate::ui_text::command_card::index_label(index, total),
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
            self.card_bands.clear();
            self.card_band_probes.clear();
            return;
        }
        let mut alive: std::collections::HashSet<PaneId> = std::collections::HashSet::new();
        for tab in self.workspace.tabs() {
            alive.extend(tab.tree().panes().iter().map(|p| p.id()));
        }
        // バックグラウンド退避中（FR-2.15）のペインは生きている = カードも残す
        alive.extend(self.workspace.shelved_panes().iter().map(|s| s.pane().id()));
        self.command_cards.retain_panes(|p| alive.contains(&p));
        self.card_bands.retain(|p, _| alive.contains(p));
        self.card_band_probes.retain(|p, _| alive.contains(p));
    }
}

/// 帯そのものが食う高さ（区切り線 + 上下余白）
fn band_chrome() -> f32 {
    BAND_BORDER + BAND_PADDING_V * 2.0
}

/// カード内容と描画幅から作る署名（#703）。**これが変わったときだけ帯を測り直す**ので、
/// カード内の見た目だけの変化（コピー成功表示）では PTY resize が起きない
fn card_band_signature(cards: &[(u64, Option<String>, Vec<String>)], width: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // 幅は 1px 単位で丸める（サブピクセルの揺れで測り直さない）
    (width.round() as i64).hash(&mut hasher);
    for (id, label, commands) in cards {
        id.hash(&mut hasher);
        label.hash(&mut hasher);
        commands.hash(&mut hasher);
    }
    hasher.finish()
}

/// カードスタックの高さの初回推定（#703）。実描画で採取できるまでの 1 フレームぶんの
/// 当たりで、ここが多少ずれても次フレームで実測に置き換わる。
/// **採取が何らかの理由で走らない経路が生まれても機能が死なない**ための保険でもある
fn estimate_card_stack_height(cards: &[(u64, Option<String>, Vec<String>)], width: f32) -> f32 {
    // カードの本文が使える幅（帯の余白 + カードのパディング + ブロックのパディング）
    let text_width = (width - BAND_PADDING_H * 2.0 - 20.0 - 14.0).max(40.0);
    let mut total = 0.0;
    for (i, (_, label, commands)) in cards.iter().enumerate() {
        if i > 0 {
            total += CARD_GAP;
        }
        let heading_chars = label.as_ref().map(|l| l.chars().count()).unwrap_or(16) as f32;
        let heading_lines = (heading_chars * 7.0 / text_width).ceil().max(1.0);
        total += ESTIMATE_CARD_CHROME + heading_lines * ESTIMATE_LINE;
        for command in commands {
            let wrapped: f32 = command
                .lines()
                .map(|l| {
                    (l.chars().count() as f32 * ESTIMATE_MONO_CHAR / text_width)
                        .ceil()
                        .max(1.0)
                })
                .sum();
            let body = (wrapped * ESTIMATE_LINE).min(COMMAND_MAX_HEIGHT);
            total += ESTIMATE_BLOCK_CHROME + body;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: u64, label: Option<&str>, commands: &[&str]) -> (u64, Option<String>, Vec<String>) {
        (
            id,
            label.map(str::to_string),
            commands.iter().map(|c| c.to_string()).collect(),
        )
    }

    #[test]
    fn 署名はカード内容と幅だけで決まる() {
        let a = vec![card(1, Some("説明"), &["echo hi"])];
        let b = vec![card(1, Some("説明"), &["echo hi"])];
        assert_eq!(
            card_band_signature(&a, 800.0),
            card_band_signature(&b, 800.0)
        );
        // サブピクセルの揺れでは測り直さない
        assert_eq!(
            card_band_signature(&a, 800.0),
            card_band_signature(&a, 800.4)
        );
        // 内容・幅が変われば測り直す
        assert_ne!(
            card_band_signature(&a, 800.0),
            card_band_signature(&a, 640.0)
        );
        let changed = vec![card(1, Some("説明"), &["echo bye"])];
        assert_ne!(
            card_band_signature(&a, 800.0),
            card_band_signature(&changed, 800.0)
        );
        let added = vec![card(1, Some("説明"), &["echo hi"]), card(2, None, &["ls"])];
        assert_ne!(
            card_band_signature(&a, 800.0),
            card_band_signature(&added, 800.0)
        );
    }

    #[test]
    fn 推定高さはカードが増えるほど大きくなる() {
        let one = vec![card(1, None, &["echo hi"])];
        let two = vec![card(1, None, &["echo hi"]), card(2, None, &["ls -la"])];
        let h1 = estimate_card_stack_height(&one, 800.0);
        let h2 = estimate_card_stack_height(&two, 800.0);
        assert!(h1 > 40.0, "カード 1 枚でボタン行が入る高さ: {h1}");
        assert!(h2 > h1, "2 枚の方が高い: {h1} -> {h2}");
        // 折り返しが増える狭幅では高くなる
        let long = vec![card(1, None, &["cargo test --workspace -- --nocapture"])];
        assert!(
            estimate_card_stack_height(&long, 200.0) > estimate_card_stack_height(&long, 800.0)
        );
        // 極端に長いコマンドでも 1 枚ぶんは上限で頭打ち（帯側の上限とは別の保険）
        let huge = vec![card(1, None, &[&"x".repeat(100_000)])];
        assert!(estimate_card_stack_height(&huge, 800.0) < 400.0);
    }

    #[test]
    fn 幅が0でも推定が破綻しない() {
        let one = vec![card(1, None, &["echo hi"])];
        let h = estimate_card_stack_height(&one, 0.0);
        assert!(h.is_finite() && h > 0.0, "{h}");
    }
}

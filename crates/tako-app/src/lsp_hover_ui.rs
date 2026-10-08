//! ホバー（型・doc のカード。S4 / #1681）の GUI: マウスのデバウンス・答えの照合・カードの置き場・
//! 閉じる条件
//!
//! ## 問い合わせと描画の正本はここに無い
//!
//! 問い合わせは manager の 1 本（`LspManager::hover`。マウスの要求は次の要求が `$/cancelRequest`
//! で捨てる・開いていない文書でサーバを起こさない）。応答の読み取り（3 形）と本文の上限は
//! `tako_core::lsp::hover` の純粋関数。Markdown の描画は `md_view::render_block`（プレビュー・
//! アップデート詳細と同じ 1 実装 = [`hover_body`] は `md_view::render_blocks` を呼ぶだけ）。
//! 編集メニュー / パレット / キー（⇧⌘H・Ctrl+Shift+H）/ 右クリックメニューの「ホバー情報を表示」
//! （#1893）は CLI `tako lsp hover --show` / MCP `tako_lsp` と同じ dispatch の 3 段を通り
//! （[`TakoApp::request_lsp_hover`] の 1 本）、カードを出すのは [`TakoApp::open_lsp_hover_card`] の 1 本。
//!
//! ## サーバの読み込み中（#1893）
//!
//! マウスの要求も manager が読み込みを待って問い直す（補完の打鍵 = #1869 と同じ待ち）。待つあいだは
//! 語の真下に「読み込み中」の 1 行（[`LspHoverUi::loading`]）を出し、答えが届いたらカードへ差し替える。
//! メニュー / キーの要求は定義ジャンプ・整形と同じくヘッダの「問い合わせています」（読み込み中なら
//! その旨）を出す。カードの本文はいつも [`MAX_CHARS`] 字まで（CLI / MCP の `limit` はカードに効かない）。
//!
//! ここが持つのは画面だけ: いつ問い合わせるか（識別子に乗る → デバウンス）・カードをどこに
//! 置くか（[`hover_card_placement`]。語の行の真下、入らなければ真上。ウィンドウの端で見切れない）・
//! いつ閉じるか。
//!
//! ## 置き場を 1 フレーム目に測る
//!
//! Markdown の高さは組んでみるまで分からない。1 フレーム目は見えない（`invisible`）まま組んで
//! 実寸を測り（`canvas` の prepaint = 描画はしない）、2 フレーム目から測った高さで上下を決めて出す
//! （上へ返すときに語の行を覆わない・下へはみ出さない）。

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, AnyElement, Bounds, Context, Keystroke, MouseButton, Pixels,
    Point, SharedString, Size,
};
use tako_control::lsp::{HoverAnswer, HoverError, HoverRequest};
use tako_core::lsp::hover::{Hover, Markup, MAX_CHARS};
use tako_core::PaneId;

use crate::md_view::{self, MdTextSink};
use crate::preview::{self, MdBlock, MdBlockKind, MdSpan};
use crate::{hsla, rgba, MdLinkHit, TakoApp};

/// カードの幅（ウィンドウが狭ければ縮める）
pub(crate) const HOVER_CARD_WIDTH: f32 = 520.0;
/// カードの高さの上限（それより長い doc はカードの中でスクロールする）
pub(crate) const HOVER_CARD_MAX_HEIGHT: f32 = 360.0;
/// ウィンドウの端から空ける幅
pub(crate) const HOVER_EDGE: f32 = 4.0;
/// カードの本文の文字サイズ（見出し・コードブロックの大きさはこれに対する比 = `render_block`）
pub(crate) const HOVER_BASE: f32 = 12.5;
/// 「読み込み中」の 1 行の高さ（#1893。補完の「読み込み中」と同じ寸法）
pub(crate) const HOVER_LOADING_HEIGHT: f32 = 28.0;
/// 「読み込み中」の 1 行の幅
pub(crate) const HOVER_LOADING_WIDTH: f32 = 400.0;

/// カードを出したきっかけ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoverOrigin {
    /// マウスを識別子に乗せた（語とカードの外へ出たら閉じる）
    Mouse,
    /// 編集メニュー / パレット / CLI・MCP の `show`（カードの外の押下・そのペインでの打鍵・Esc で
    /// 閉じる。マウスの位置とフォーカスでは閉じない）
    Explicit,
}

/// マウスが乗っている識別子（ペイン・0 起点の行・行内の UTF-8 バイト範囲）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoverTarget {
    pub(crate) pane: PaneId,
    pub(crate) line: usize,
    pub(crate) range: Range<usize>,
}

/// 測ったカードの実寸（カードの世代つき = 前のカードの寸法で次のカードを置かない）
type MeasuredSlot = Rc<Cell<Option<(u64, Size<Pixels>)>>>;

/// ホバーの GUI の状態（#1681）
#[derive(Default)]
pub(crate) struct LspHoverUi {
    /// マウスが乗っている識別子（デバウンス中・問い合わせ中・カードを出している語）
    target: Option<HoverTarget>,
    /// 乗せ直すたびに進む（デバウンス明け・答えが届いたときに最新かを見る）
    seq: u64,
    /// 出しているカード（同時に 1 枚）
    pub(crate) card: Option<HoverCard>,
    /// カードを出すたびに進む（測った寸法とスクロール位置を前のカードから持ち越さない）
    generation: u64,
    /// 直近に描いたカードの矩形（マウスがカードの上にいる間は閉じない・visual-test が差分の範囲を見る）
    pub(crate) bounds: Option<Bounds<Pixels>>,
    /// 1 フレーム目に測ったカードの実寸（世代つき）
    measured: MeasuredSlot,
    /// 直近に描いたカードの本文の `TextLayout`（リンクの当たり判定）
    layouts: Vec<Option<gpui::TextLayout>>,
    /// 背景で答えを待っている問い合わせの数（100 回の出し入れで増え続けないことを visual-test が測る）
    pub(crate) inflight: usize,
    /// これまでに出したカードの数
    pub(crate) shown: u64,
    /// 直近の答えが失敗だったときの `status`（カードを出さない理由。visual-test が名指す）
    pub(crate) last_failure: Option<&'static str>,
    /// 描く直前の照合でカードを閉じた直近の理由（visual-test が名指す）
    pub(crate) last_invalid: Option<&'static str>,
    /// 言語サーバの起動 / 読み込みを待っているマウスの問い合わせ（#1893）。問い合わせを出した時点で
    /// サーバが起動中か読み込み中なら立て（番号と編集バッファの版）、その答え（カード・何も無い・
    /// 失敗）が届くか、語から外れる・閉じるで下ろす。**表示だけ**（押下は下の本文へ通す）
    pub(crate) loading: Option<(u64, Option<u64>)>,
    /// 直近に描いた「読み込み中」の矩形（visual-test が差分の範囲を見る）
    pub(crate) loading_bounds: Option<Bounds<Pixels>>,
    /// メニュー / キーで頼んで答えを待っている問い合わせ（ペインと番号。#1893）。ヘッダに
    /// 「問い合わせています」を出す（定義ジャンプ・整形の `pending` と同じ）
    pub(crate) pending: Option<(PaneId, u64)>,
    /// メニュー / キーの問い合わせの番号（古い答えでヘッダの印を下ろさない）
    explicit_seq: u64,
}

impl LspHoverUi {
    /// 状態の 1 行（visual-test が外れた回を名指すため。本文は出さない）
    #[cfg(feature = "visual-test")]
    pub(crate) fn debug_state(&self) -> String {
        format!(
            "target={:?} seq={} card={} inflight={} shown={} last_failure={:?} last_invalid={:?} loading={:?} pending={:?}",
            self.target.as_ref().map(|t| (t.line, t.range.clone())),
            self.seq,
            self.card.is_some(),
            self.inflight,
            self.shown,
            self.last_failure,
            self.last_invalid,
            self.loading,
            self.pending,
        )
    }
}

/// 出しているカード 1 枚
pub(crate) struct HoverCard {
    pub(crate) pane: PaneId,
    path: PathBuf,
    /// 語の行（0 起点）と、語の範囲（行内の UTF-8 バイト）。カードはこの語の真下（か真上）に置く
    pub(crate) line: usize,
    pub(crate) range: Range<usize>,
    pub(crate) origin: HoverOrigin,
    /// 本文（Markdown はパース済み・平文は段落 1 つ）
    pub(crate) blocks: Arc<Vec<MdBlock>>,
    /// 本文のリンク（押すとブラウザで開く。当たり判定は `md_view::md_link_at_layouts`）
    links: Arc<Vec<MdLinkHit>>,
    /// 上限で切った本文なら切る前の文字数（visual-test が巨大な doc の切り方を見る）
    pub(crate) truncated: Option<usize>,
    /// 出したときの編集バッファの版（編集していなければ `None`）。変われば閉じる（語の位置がずれる）
    version: Option<u64>,
    generation: u64,
}

/// 答えの本文 → 描くブロック。
///
/// Markdown は `preview::markdown_blocks`（md のパースの唯一の正 = `pulldown-cmark`）、平文は
/// **段落 1 つに素の文字列を 1 本**（解釈もエスケープもしない。改行は `StyledText` がそのまま折る）
pub(crate) fn hover_blocks(content: &Hover) -> Vec<MdBlock> {
    match content.markup {
        Markup::Markdown => preview::markdown_blocks(&content.value),
        Markup::PlainText => vec![MdBlock {
            kind: MdBlockKind::Paragraph {
                spans: vec![MdSpan {
                    text: content.value.clone(),
                    ..MdSpan::default()
                }],
            },
            quote_depth: 0,
            list_depth: 0,
        }],
    }
}

/// カードの本文の要素列。**各ブロックを `md_view::render_block` に通す**（プレビュー・
/// アップデート詳細と同じ見た目の 1 実装。ホバー専用の描き方を作らない）
pub(crate) fn hover_body(
    theme: &tako_core::Theme,
    blocks: &[MdBlock],
    sink: &mut impl MdTextSink,
) -> Vec<AnyElement> {
    md_view::render_blocks(theme, HOVER_BASE, blocks, sink)
}

/// カードを出す語の行（ウィンドウ座標）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HoverAnchor {
    /// 語の左端
    pub(crate) left: f32,
    /// 語の行の上端・下端
    pub(crate) top: f32,
    pub(crate) bottom: f32,
}

/// カードの置き場（左上・幅・高さ・高さの上限）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HoverPlacement {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) width: f32,
    /// 置いたカードの高さ（測った高さを入る分まで縮めたもの）
    pub(crate) height: f32,
    /// カードの高さの上限（これより長い本文はカードの中でスクロールする）
    pub(crate) max_height: f32,
}

/// カードの置き場（純粋関数）。**ウィンドウの端で見切れさせない**:
///
/// - 縦: 語の行の真下。入らなければ語の行の真上へ返す（コンテキストメニューの
///   `compute_menu_position` と同じ「はみ出すなら返す・0 未満にしない」を、語の行を覆わないよう
///   行の上端・下端を基点にして使う）。どちらにも入らなければ広い方に置き、入る高さまで縮める
///   （本文はカードの中でスクロールする）
/// - 横: 語の左端。右へはみ出すなら左へ**ずらす**（返さない = 語の真下からカードへマウスを
///   下ろす経路を切らない）。0 未満にしない。ウィンドウより広ければ幅を縮める
///
/// `size` は（幅, 測った高さ）。まだ測っていなければ高さの上限を渡す
pub(crate) fn hover_card_placement(
    anchor: HoverAnchor,
    size: (f32, f32),
    viewport: (f32, f32),
) -> HoverPlacement {
    let (vw, vh) = viewport;
    let width = size.0.min((vw - 2.0 * HOVER_EDGE).max(0.0));
    let below_room = (vh - HOVER_EDGE - anchor.bottom).max(0.0);
    let above_room = (anchor.top - HOVER_EDGE).max(0.0);
    let wanted = size.1.min(HOVER_CARD_MAX_HEIGHT);
    // 縦の返しは `compute_menu_position` の判定（語の行の下端から測って下へはみ出すか）を使う
    let (_, flipped_y) = crate::compute_menu_position(
        anchor.left,
        anchor.bottom,
        width,
        wanted,
        vw,
        vh - HOVER_EDGE,
    );
    let fits_below = flipped_y >= anchor.bottom;
    let below = fits_below || (wanted > above_room && below_room >= above_room);
    let room = if below { below_room } else { above_room };
    let max_height = HOVER_CARD_MAX_HEIGHT.min(room);
    let height = wanted.min(room);
    let top = if below {
        anchor.bottom
    } else {
        anchor.top - height
    };
    let left = anchor
        .left
        .min(vw - HOVER_EDGE - width)
        .max(HOVER_EDGE.min((vw - width).max(0.0)));
    HoverPlacement {
        left,
        top,
        width,
        height,
        max_height,
    }
}

/// 語の行の矩形（行のレイアウトと語の範囲から。語の右端は範囲の終わりの字の位置）
fn symbol_bounds(layout: &gpui::TextLayout, range: &Range<usize>) -> Option<Bounds<Pixels>> {
    let start = layout.position_for_index(range.start)?;
    let end = layout
        .position_for_index(range.end)
        .filter(|end| end.y == start.y)
        .map_or(layout.bounds().right(), |end| end.x);
    let line_height = layout.line_height();
    let right = if end > start.x + px(1.0) {
        end
    } else {
        start.x + px(1.0)
    };
    Some(Bounds::from_corners(
        start,
        gpui::point(right, start.y + line_height),
    ))
}

/// 矩形を `margin` だけ広げて点が入るか
fn contains_with_margin(bounds: &Bounds<Pixels>, point: Point<Pixels>, margin: f32) -> bool {
    let m = px(margin);
    point.x >= bounds.left() - m
        && point.x <= bounds.right() + m
        && point.y >= bounds.top() - m
        && point.y <= bounds.bottom() + m
}

impl TakoApp {
    /// このペインでマウスのホバーを使うか。コード表示で、検出表にこの拡張子を受け持つサーバがあり、
    /// LSP が有効で、A/B の旧挙動（`TAKO_1681_LEGACY=1`）でなく、**文書が言語サーバにつながっている**
    /// （編集モードか、別のペインが編集している = 乗せただけでサーバを起こさない）ときだけ。
    /// 補完の一覧を出しているペイン・右クリックメニューを開いているあいだは出さない（重ねない）
    pub(crate) fn lsp_hover_enabled_for(&self, pane: PaneId) -> bool {
        if tako_control::lsp::hover::legacy() || !self.lsp.is_enabled() {
            return false;
        }
        let Some(state) = self.previews.get(&pane) else {
            return false;
        };
        if state.mode != preview::PreviewMode::Code
            || tako_core::lsp::servers::resolve(&state.path).is_none()
            || self.lsp_completion_open_in(pane)
            // 右クリックメニュー（#1684 の言語サーバの項目を含む）を開いているあいだは出さない
            // （カードはメニューより手前に積まれるので、語の上でマウスが動くとメニューを隠す）
            || self.pane_context_menu.is_some()
            || self.context_menu.is_some()
        {
            return false;
        }
        self.preview_edits.get(&pane).is_some_and(|e| e.editing)
            || self.lsp.has_document(&state.path)
    }

    /// 編集バッファの版（編集していなければ `None`）
    fn hover_version(&self, pane: PaneId) -> Option<u64> {
        self.preview_edits
            .get(&pane)
            .filter(|e| e.editing)
            .map(|e| e.buffer.version())
    }

    /// マウス位置の識別子と、問い合わせる桁（マウスの下の字の行内バイト）
    fn hover_symbol_at(&self, position: Point<Pixels>) -> Option<(HoverTarget, usize)> {
        'panes: for (pane, layouts) in &self.preview_text_layouts {
            let Some(texts) = self.preview_line_texts.get(pane) else {
                continue;
            };
            for (line, layout) in layouts.iter().enumerate() {
                let Some(layout) = layout else {
                    continue;
                };
                if !layout.bounds().contains(&position) {
                    continue;
                }
                // 行の矩形で当たりを付けてから有効かを見る（マウスが動くたびに全ペインで manager の
                // ロックを取らない）。無効なペイン（別のタブで描かれていない古い行も含む）は飛ばす
                if !self.lsp_hover_enabled_for(*pane) {
                    continue 'panes;
                }
                // 文字の上にあるときだけ（行末より右・行間は対象外 = ⌘ホバー #1680 と同じ）
                let Ok(byte) = layout.index_for_position(position) else {
                    continue 'panes;
                };
                let Some(symbol) = texts
                    .get(line)
                    .and_then(|text| tako_core::lsp::goto::symbol_at(text, byte))
                else {
                    continue 'panes;
                };
                return Some((
                    HoverTarget {
                        pane: *pane,
                        line,
                        range: symbol.range,
                    },
                    byte,
                ));
            }
        }
        None
    }

    /// マウスがカードかカードを出した語の上にいるか（いる間は閉じない）
    fn hover_keeps(&self, position: Point<Pixels>) -> bool {
        let Some(card) = &self.lsp_hover.card else {
            return false;
        };
        if self
            .lsp_hover
            .bounds
            .is_some_and(|b| contains_with_margin(&b, position, 2.0))
        {
            return true;
        }
        self.preview_text_layouts
            .get(&card.pane)
            .and_then(|l| l.get(card.line))
            .and_then(Option::as_ref)
            .and_then(|layout| symbol_bounds(layout, &card.range))
            .is_some_and(|b| contains_with_margin(&b, position, 0.0))
    }

    /// マウスが動いた（`on_mouse_move`）。識別子に乗ったらデバウンスの後に問い合わせ、外れたら閉じる。
    /// `dragging` はボタンを押したまま（選択のドラッグ中は出さない）
    pub(crate) fn update_lsp_hover(
        &mut self,
        position: Point<Pixels>,
        dragging: bool,
        cx: &mut Context<Self>,
    ) {
        if dragging {
            if self.lsp_hover.target.is_some() || self.mouse_hover_card_open() {
                self.close_lsp_hover();
                cx.notify();
            }
            return;
        }
        if self.hover_keeps(position) {
            return;
        }
        match self.hover_symbol_at(position) {
            None => {
                if self.lsp_hover.target.is_some() || self.mouse_hover_card_open() {
                    let explicit = self
                        .lsp_hover
                        .card
                        .as_ref()
                        .is_some_and(|c| c.origin == HoverOrigin::Explicit);
                    if explicit {
                        // メニュー / CLI で出したカードはマウスでは閉じない（待ちだけ捨てる）
                        self.lsp_hover.target = None;
                        self.lsp_hover.seq = self.lsp_hover.seq.wrapping_add(1);
                        self.lsp_hover.loading = None;
                        self.lsp_hover.loading_bounds = None;
                        self.lsp.cancel_hover();
                    } else {
                        self.close_lsp_hover();
                    }
                    cx.notify();
                }
            }
            Some((target, column)) => {
                if self.lsp_hover.target.as_ref() == Some(&target) {
                    return;
                }
                // 別の語へ移った: 前の語のカード・「読み込み中」と待ちを捨ててから、この語をデバウンスへ積む
                if self.mouse_hover_card_open() {
                    self.close_lsp_hover();
                }
                self.lsp_hover.loading = None;
                self.lsp_hover.loading_bounds = None;
                self.lsp.cancel_hover();
                self.lsp_hover.target = Some(target.clone());
                self.schedule_lsp_hover(target, column, cx);
                cx.notify();
            }
        }
    }

    fn mouse_hover_card_open(&self) -> bool {
        self.lsp_hover
            .card
            .as_ref()
            .is_some_and(|c| c.origin == HoverOrigin::Mouse)
    }

    /// デバウンスへ積む（`tako_control::lsp::hover::delay` の後、まだ同じ語に乗っていれば問い合わせる）
    fn schedule_lsp_hover(&mut self, target: HoverTarget, column: usize, cx: &mut Context<Self>) {
        self.lsp_hover.seq = self.lsp_hover.seq.wrapping_add(1);
        let seq = self.lsp_hover.seq;
        let delay = tako_control::lsp::hover::delay();
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = this.update(cx, |app, cx| app.fire_lsp_hover(target, column, seq, cx));
        })
        .detach();
    }

    /// デバウンスが明けた: まだ同じ語に乗っていれば問い合わせる（待つのは background）
    fn fire_lsp_hover(
        &mut self,
        target: HoverTarget,
        column: usize,
        seq: u64,
        cx: &mut Context<Self>,
    ) {
        if seq != self.lsp_hover.seq
            || self.lsp_hover.target.as_ref() != Some(&target)
            || !self.lsp_hover_enabled_for(target.pane)
        {
            return;
        }
        let Some(path) = self.previews.get(&target.pane).map(|p| p.path.clone()) else {
            return;
        };
        let version = self.hover_version(target.pane);
        // #1893: サーバが起動中 / 読み込み中なら、答え（manager が読み込みを待って問い直す）が届くまで
        // 語の真下に「読み込み中」を出す。A/B の旧腕（`TAKO_1893_LEGACY=1`）は出さない（待たない）
        let loading = !tako_control::lsp::hover::legacy_1893() && self.lsp.server_loading(&path);
        self.lsp_hover.loading = loading.then_some((seq, version));
        let request = HoverRequest {
            path,
            line: target.line,
            column,
            timeout: tako_control::lsp::hover::hover_timeout(),
            // 文書は開いている（つながっていなければ入口で弾いた）ので本文を渡さない
            document: None,
            // マウスの要求: 次の要求が来たら manager が `$/cancelRequest` で捨てる
            superseding: true,
            // 開いていない文書でサーバを起こさない（すれ違いで閉じたら NotOpen で返る）
            open: false,
            // 取り消しの番号はここ（UI スレッド）で先に取る = この後に語から外れた取り消しを
            // 背景が追い越さない（追い越すと読み込みが済むまで待ち続ける。#1893）
            ticket: self.lsp.reserve_hover(),
        };
        self.lsp_hover.inflight += 1;
        let manager = self.lsp.clone();
        let task = cx
            .background_executor()
            .spawn(async move { manager.hover(&request) });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                app.lsp_hover.inflight = app.lsp_hover.inflight.saturating_sub(1);
                app.finish_lsp_hover(target, seq, version, outcome, cx);
            });
        })
        .detach();
    }

    /// 答えが届いた: まだ同じ語に乗っていて本文も変わっていなければカードを出す
    fn finish_lsp_hover(
        &mut self,
        target: HoverTarget,
        seq: u64,
        version: Option<u64>,
        outcome: Result<HoverAnswer, HoverError>,
        cx: &mut Context<Self>,
    ) {
        // 「読み込み中」はこの問い合わせの答え（カード・何も無い・失敗のどれでも）が届いたら下ろす
        if self.lsp_hover.loading.is_some_and(|(s, _)| s == seq) {
            self.lsp_hover.loading = None;
            self.lsp_hover.loading_bounds = None;
            cx.notify();
        }
        if seq != self.lsp_hover.seq
            || self.lsp_hover.target.as_ref() != Some(&target)
            || self.hover_version(target.pane) != version
            || self.lsp_completion_open_in(target.pane)
        {
            return;
        }
        // 失敗（未応答・落ちた…）と「何も無い」はカードを出さないだけ（乗せるたびに通知しない。
        // 理由は `tako lsp hover` / `tako lsp status` が返す）
        let answer = match outcome {
            Ok(answer) => answer,
            Err(error) => {
                self.lsp_hover.last_failure = Some(error.status());
                return;
            }
        };
        if self.open_lsp_hover_card(
            target.pane,
            target.line,
            target.range,
            &answer,
            HoverOrigin::Mouse,
        ) {
            cx.notify();
        }
    }

    /// カードを出す（マウス・編集メニュー・CLI / MCP の `show` の 1 本）。出せたら `true`
    /// （本文が無い・その行が描かれていない = 画面の外なら出さない）。`range` は語の範囲で、
    /// サーバが同じ行の範囲を返していればそちらを使う
    pub(crate) fn open_lsp_hover_card(
        &mut self,
        pane: PaneId,
        line: usize,
        range: Range<usize>,
        answer: &HoverAnswer,
        origin: HoverOrigin,
    ) -> bool {
        let Some(content) = &answer.content else {
            return false;
        };
        let Some(path) = self.previews.get(&pane).map(|p| p.path.clone()) else {
            return false;
        };
        let drawn = self
            .preview_text_layouts
            .get(&pane)
            .and_then(|l| l.get(line))
            .is_some_and(Option::is_some);
        if !drawn {
            return false;
        }
        // サーバの範囲（ホバーの対象）は同じ行で語と重なるときだけ、語との和にして使う
        // （語から外れた範囲を採ると、乗せている語がカードを保つ範囲の外になり出した直後に閉じる）
        let range = match answer.range {
            Some((start, end))
                if start.line == line
                    && end.line == line
                    && start.col < range.end.max(range.start + 1)
                    && range.start < end.col =>
            {
                start.col.min(range.start)..end.col.max(range.end)
            }
            _ => range,
        };
        // カードはいつも既定の上限まで（CLI / MCP の `limit` = 全文はカードに効かない。#1893）
        let content = content.limited(Some(MAX_CHARS));
        let blocks = hover_blocks(&content);
        let links = crate::md_document_links(&blocks);
        self.lsp_hover.generation = self.lsp_hover.generation.wrapping_add(1);
        self.lsp_hover.bounds = None;
        self.lsp_hover.layouts.clear();
        self.lsp_hover.shown = self.lsp_hover.shown.wrapping_add(1);
        self.lsp_hover.card = Some(HoverCard {
            pane,
            path,
            line,
            range,
            origin,
            blocks: Arc::new(blocks),
            links: Arc::new(links),
            truncated: content.truncated.then_some(content.total_chars),
            version: self.hover_version(pane),
            generation: self.lsp_hover.generation,
        });
        true
    }

    /// カードを閉じ、待っている問い合わせを捨てる（manager が `$/cancelRequest` を送る）
    pub(crate) fn close_lsp_hover(&mut self) {
        let ui = &mut self.lsp_hover;
        ui.card = None;
        ui.bounds = None;
        ui.layouts.clear();
        ui.loading = None;
        ui.loading_bounds = None;
        ui.target = None;
        ui.seq = ui.seq.wrapping_add(1);
        self.lsp.cancel_hover();
    }

    /// ペインを閉じた / プレビューを差し替えた（カードの語の行は前の本文の座標）
    pub(crate) fn forget_lsp_hover(&mut self, pane: PaneId) {
        let ours = self.lsp_hover.card.as_ref().is_some_and(|c| c.pane == pane)
            || self
                .lsp_hover
                .target
                .as_ref()
                .is_some_and(|t| t.pane == pane);
        if ours {
            self.close_lsp_hover();
        }
    }

    /// 打鍵（プレビューの打鍵の入口の先頭）。Esc はカードだけを閉じて打鍵を取る（編集モードは
    /// 抜けない）。他の打鍵はメニュー / CLI で出したカードを閉じ、打鍵はいつもの経路へ流す
    /// （マウスで出したカードは本文が変われば閉じる = 描く直前の照合）。**カードがフォーカス中の
    /// ペインの上にあるときだけ**見る（別のペインのカードのために端末の Esc = vim 等を奪わない）
    pub(crate) fn route_lsp_hover_key(&mut self, pane: PaneId, keystroke: &Keystroke) -> bool {
        // 「読み込み中」の 1 行も Esc で閉じる（待っている問い合わせも捨てる。#1893）
        if self.hover_loading_target().is_some_and(|t| t.pane == pane)
            && keystroke.key == "escape"
            && keystroke.modifiers == gpui::Modifiers::default()
        {
            self.close_lsp_hover();
            return true;
        }
        let Some(card) = self.lsp_hover.card.as_ref().filter(|c| c.pane == pane) else {
            return false;
        };
        if keystroke.key == "escape" && keystroke.modifiers == gpui::Modifiers::default() {
            self.close_lsp_hover();
            return true;
        }
        let modifier_only = matches!(
            keystroke.key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function" | "capslock"
        );
        if card.origin == HoverOrigin::Explicit && !modifier_only {
            self.close_lsp_hover();
        }
        false
    }

    /// 編集メニュー / パレット / キー（⇧⌘H・Ctrl+Shift+H。#1893）の「ホバー情報を表示」: フォーカス中の
    /// コードの編集カーソル（編集していなければ選択の先頭）の位置で問い合わせ、カードを出す。
    /// 問い合わせは [`Self::request_lsp_hover`]（右クリックメニューと同じ 1 本）。補完の一覧を出して
    /// いれば閉じてから（同じ語の真下に重ねない。明示の操作が勝つ）
    pub(crate) fn show_hover_at_cursor(&mut self, cx: &mut Context<Self>) {
        if tako_control::lsp::hover::legacy() {
            return;
        }
        let pane = self.focused_pane();
        let Some(mode) = self.previews.get(&pane).map(|p| p.mode) else {
            return;
        };
        if mode != preview::PreviewMode::Code {
            self.show_lsp_status(
                pane,
                crate::ui_text::preview::hover_not_code().to_string(),
                true,
                cx,
            );
            return;
        }
        let at = match self.preview_edits.get(&pane).filter(|e| e.editing) {
            Some(edit) => Some(edit.buffer.line_byte_col(edit.buffer.cursor())),
            None => self.preview_selections.get(&pane).map(|s| s.head),
        };
        let Some((line, column)) = at else {
            self.show_lsp_status(
                pane,
                crate::ui_text::preview::hover_no_cursor().to_string(),
                true,
                cx,
            );
            return;
        };
        if self.lsp_completion_open_in(pane) {
            self.close_completion();
        }
        let request = tako_control::protocol::Request::LspHover {
            pane: Some(pane.as_u64()),
            line: line + 1,
            column,
            show: Some(true),
            limit: None,
        };
        self.request_lsp_hover(pane, request, cx);
    }

    /// メニュー / キー / 右クリックメニューのホバーの問い合わせ（#1893 で 1 本へ）。**CLI `tako lsp hover
    /// --show` / MCP と同じ dispatch の 3 段を通す**（明示の問い合わせ = 開いていなければ一時的に開き、
    /// 読み込み中なら済むまで待つ）。待つあいだはヘッダに「問い合わせています」、見つかればカードが
    /// 答え、何も無い・未導入… はヘッダに理由を出す
    pub(crate) fn request_lsp_hover(
        &mut self,
        pane: PaneId,
        request: tako_control::protocol::Request,
        cx: &mut Context<Self>,
    ) {
        let job = match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => job,
            Some(Err(e)) => {
                self.show_lsp_status(pane, e.to_string(), true, cx);
                return;
            }
            None => return,
        };
        self.lsp_hover.explicit_seq = self.lsp_hover.explicit_seq.wrapping_add(1);
        let seq = self.lsp_hover.explicit_seq;
        self.lsp_hover.pending = Some((pane, seq));
        let staged = cx
            .background_executor()
            .spawn(async move { job.run_staged() });
        cx.spawn(async move |this, cx| {
            let outcome = staged.await;
            let _ = this.update(cx, |app, cx| {
                if app.lsp_hover.pending.is_some_and(|(_, s)| s == seq) {
                    app.lsp_hover.pending = None;
                }
                let tako_control::OffloadOutcome::OnUi(next) = outcome else {
                    return;
                };
                let (result, _redraw) =
                    app.finish_offload_on_ui(next, tako_core::PaneOrigin::User, cx);
                match result {
                    // 見つかって出せた = カードが答え。何も無い・未導入… はヘッダに理由を出す
                    Ok(value) if value["shown"] == serde_json::json!(true) => {}
                    Ok(value) => {
                        let reason = value["reason"].as_str().unwrap_or_default().to_string();
                        if !reason.is_empty() {
                            app.show_lsp_status(pane, reason, value["status"] != "none", cx);
                        }
                    }
                    Err(e) => app.show_lsp_status(pane, e.to_string(), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// ヘッダの「問い合わせています」（メニュー / キーで頼んだホバーの答えを待っているあいだ。#1893）。
    /// サーバが読み込み中なら済んだら出すと言う（読み込みの待ちは manager が持つ）
    pub(crate) fn lsp_hover_header_status(&self, pane: PaneId) -> Option<String> {
        let (waiting, _) = self.lsp_hover.pending.filter(|(p, _)| *p == pane)?;
        let loading = self
            .previews
            .get(&waiting)
            .is_some_and(|p| self.lsp.server_loading(&p.path));
        Some(if loading {
            tako_control::lsp::text::HOVER_LOADING_NOTE
                .text()
                .to_string()
        } else {
            crate::ui_text::preview::hover_searching().to_string()
        })
    }

    /// 「読み込み中」を出しているマウスの語（今の番号の問い合わせで、版も変わっていないときだけ。#1893）
    fn hover_loading_target(&self) -> Option<&HoverTarget> {
        let (seq, version) = self.lsp_hover.loading?;
        let target = self.lsp_hover.target.as_ref()?;
        (seq == self.lsp_hover.seq
            && self.lsp_hover.card.is_none()
            && self.hover_version(target.pane) == version)
            .then_some(target)
    }

    /// 「読み込み中」の 1 行（#1893。補完の「読み込み中」= #1869 と同じ見た目と置き場 = 語の頭の真下、
    /// 下に入らなければ真上）。外れていれば下ろして何も描かない。**押下は下の本文へ通す**（表示だけ）
    fn render_lsp_hover_loading(&mut self, window: &gpui::Window) -> Option<AnyElement> {
        let Some(target) = self.hover_loading_target().cloned() else {
            if self.lsp_hover.loading.is_some() && self.lsp_hover.card.is_none() {
                // 版が変わった（打った）・別の語の番号: 印だけ下ろす（待ちは語の移動 / 閉じるが捨てる）
                self.lsp_hover.loading = None;
            }
            self.lsp_hover.loading_bounds = None;
            return None;
        };
        if !self.lsp_hover_enabled_for(target.pane) {
            self.lsp_hover.loading_bounds = None;
            return None;
        }
        let layout = self
            .preview_text_layouts
            .get(&target.pane)?
            .get(target.line)?
            .as_ref()?;
        let symbol = symbol_bounds(layout, &target.range)?;
        let anchor = HoverAnchor {
            left: f32::from(symbol.left()),
            top: f32::from(symbol.top()),
            bottom: f32::from(symbol.bottom()),
        };
        let viewport = window.viewport_size();
        let placement = hover_card_placement(
            anchor,
            (HOVER_LOADING_WIDTH, HOVER_LOADING_HEIGHT),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );
        self.lsp_hover.loading_bounds = Some(Bounds {
            origin: gpui::point(px(placement.left), px(placement.top)),
            size: gpui::size(px(placement.width), px(placement.height)),
        });
        let theme = &self.theme;
        Some(
            div()
                .id("lsp-hover-loading")
                .absolute()
                .left(px(placement.left))
                .top(px(placement.top))
                .w(px(placement.width))
                .h(px(placement.height))
                .px(px(10.0))
                .flex()
                .items_center()
                .bg(rgba(theme.surface_1))
                .border_1()
                .border_color(hsla(theme.border_default))
                .rounded(px(6.0))
                .shadow_lg()
                .text_size(px(12.0))
                .text_color(hsla(theme.text_secondary))
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(SharedString::from(
                            tako_control::lsp::text::HOVER_LOADING_NOTE.text(),
                        )),
                )
                .into_any_element(),
        )
    }

    /// カードがまだ今の画面に当たっているか（描く直前に見る。外れていれば閉じる）。
    /// ペインが消えた・別のファイルに差し替わった・本文が変わった・語の行が画面の外へ出た、
    /// マウスで出したカードはマウスが語とカードの外へ出た、のどれでも外れる
    fn hover_still_valid(&self, window: &gpui::Window) -> Result<(), &'static str> {
        let Some(card) = &self.lsp_hover.card else {
            return Err("no-card");
        };
        let same_file = self
            .previews
            .get(&card.pane)
            .is_some_and(|p| p.path == card.path && p.mode == preview::PreviewMode::Code);
        if !same_file {
            return Err("file");
        }
        if self.hover_version(card.pane) != card.version {
            return Err("edited");
        }
        let drawn = self
            .preview_text_layouts
            .get(&card.pane)
            .and_then(|l| l.get(card.line))
            .is_some_and(Option::is_some);
        if !drawn {
            return Err("off-screen");
        }
        // 右クリックメニューを開いたら閉じる（メニューより手前に積まれて隠す）
        if self.pane_context_menu.is_some() || self.context_menu.is_some() {
            return Err("menu");
        }
        // メニュー / CLI で出したカードはマウスの位置では閉じない（CLI の `--show` はフォーカスが
        // 端末のまま出す = フォーカスでも閉じない。閉じるのはカードの外の押下・打鍵・Esc）
        if card.origin == HoverOrigin::Mouse && !self.hover_keeps(window.mouse_position()) {
            return Err("mouse-left");
        }
        Ok(())
    }

    /// カードの本文のリンクを押した（ブラウザで開く。開けない URL は当たり判定の対象外）
    fn click_hover_card(&mut self, position: Point<Pixels>) {
        let Some(card) = &self.lsp_hover.card else {
            return;
        };
        let Some(index) =
            md_view::md_link_at_layouts(&card.links, &self.lsp_hover.layouts, position)
        else {
            return;
        };
        let Some(url) = tako_core::md_links::browser_url(&card.links[index].url) else {
            return;
        };
        // 開けなかったら通知欄へ理由を出す（リンクの文字列は診断へ出さない）
        if let Err(e) = tako_control::platform::os_integration::open_url(url) {
            self.notify_ui_op_failed(
                crate::sidebar::NoticeArea::Preview,
                crate::sidebar::NoticeArm::Issue1681,
                crate::ui_text::preview::hover_link_op(),
                None,
                &e.to_string(),
            );
        }
    }

    /// ホバーカード（語の行の真下、入らなければ真上）。外れていれば閉じて何も描かない
    pub(crate) fn render_lsp_hover(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.lsp_hover.card.is_some() {
            if let Err(reason) = self.hover_still_valid(window) {
                self.lsp_hover.last_invalid = Some(reason);
                self.close_lsp_hover();
            }
        }
        // カードがまだ無ければ「読み込み中」の 1 行（#1893）
        let Some(card) = self.lsp_hover.card.as_ref() else {
            return self.render_lsp_hover_loading(window);
        };
        let (generation, blocks, truncated) =
            (card.generation, Arc::clone(&card.blocks), card.truncated);
        let layout = self
            .preview_text_layouts
            .get(&card.pane)?
            .get(card.line)?
            .as_ref()?;
        let symbol = symbol_bounds(layout, &card.range)?;
        let anchor = HoverAnchor {
            left: f32::from(symbol.left()),
            top: f32::from(symbol.top()),
            bottom: f32::from(symbol.bottom()),
        };
        let measured = self
            .lsp_hover
            .measured
            .get()
            .filter(|(g, _)| *g == generation)
            .map(|(_, size)| size);
        let viewport = window.viewport_size();
        let placement = hover_card_placement(
            anchor,
            (
                HOVER_CARD_WIDTH,
                measured.map_or(HOVER_CARD_MAX_HEIGHT, |s| f32::from(s.height)),
            ),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );
        self.lsp_hover.bounds = Some(Bounds {
            origin: gpui::point(px(placement.left), px(placement.top)),
            size: gpui::size(px(placement.width), px(placement.height)),
        });
        let theme = self.theme.clone();
        let mut sink = md_view::ReadOnlyMdSink::new(&theme, None);
        let body = hover_body(&theme, &blocks, &mut sink);
        self.lsp_hover.layouts = sink.into_layouts();
        // 1 フレーム目は見えないまま実寸を測る（prepaint で Cell へ書くだけ。変わったら次のフレームを起こす）
        let probe = {
            let slot = Rc::clone(&self.lsp_hover.measured);
            let weak = cx.entity().downgrade();
            canvas(
                move |bounds, _, cx| {
                    // 枠線（上下 1px ずつ）は containing block の外なので足す
                    let size = gpui::size(bounds.size.width, bounds.size.height + px(2.0));
                    if slot.get() != Some((generation, size)) {
                        slot.set(Some((generation, size)));
                        let weak = weak.clone();
                        cx.defer(move |cx| {
                            if let Some(entity) = weak.upgrade() {
                                entity.update(cx, |_, cx| cx.notify());
                            }
                        });
                    }
                },
                |_, _, _, _| (),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let footer = truncated.map(|total| {
            div()
                .flex_none()
                .px(px(10.0))
                .py(px(4.0))
                .border_t_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(10.5))
                .text_color(hsla(theme.text_faint))
                .child(SharedString::from(
                    crate::ui_text::preview::hover_truncated(total),
                ))
        });
        Some(
            div()
                .id("lsp-hover-card")
                .absolute()
                .left(px(placement.left))
                .top(px(placement.top))
                .w(px(placement.width))
                .max_h(px(placement.max_height))
                .when(measured.is_none(), |d| d.invisible())
                .flex()
                .flex_col()
                .bg(rgba(theme.surface_1))
                .border_1()
                .border_color(hsla(theme.border_default))
                .rounded(px(6.0))
                .shadow_lg()
                // 下の本文のホイール・押下を通さない（カードの中のスクロールは本文の器が受ける）
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.click_hover_card(ev.position);
                    }),
                )
                // カードの外を押したら閉じる（どこを押しても = 端末・ツリー・別のペインでも）。
                // 押下そのものは止めない（押した先の操作はいつもどおり効く）
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.lsp_hover.card.is_some() {
                        this.close_lsp_hover();
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .id(("lsp-hover-body", generation as usize))
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .px(px(10.0))
                        .py(px(6.0))
                        .flex()
                        .flex_col()
                        .text_size(px(HOVER_BASE))
                        .text_color(hsla(theme.foreground))
                        .children(body),
                )
                .children(footer)
                .child(probe)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontWeight, HighlightStyle};
    use tako_core::theme::Theme;

    /// 描いた 1 行（テキスト・ハイライトの中身・色・太さ）
    type Row = (
        String,
        Vec<(Range<usize>, HighlightStyle)>,
        tako_core::Rgb,
        Option<FontWeight>,
    );

    /// 受け皿の呼び出しを記録するだけの実装
    #[derive(Default)]
    struct RecordingSink {
        rows: Vec<Row>,
        spacers: usize,
    }

    impl MdTextSink for RecordingSink {
        fn text(
            &mut self,
            text: String,
            highlights: Vec<(Range<usize>, HighlightStyle)>,
            color: tako_core::Rgb,
            weight: Option<FontWeight>,
        ) -> AnyElement {
            self.rows.push((text.clone(), highlights, color, weight));
            div().child(text).into_any_element()
        }

        fn spacer(&mut self) {
            self.spacers += 1;
        }
    }

    /// e2e（`issue1681_lsp_hover`）の偽サーバが返すのと同じ 3 種（見出し / コードブロック / リンク）
    const MARKDOWN: &str = "# Vec\n\n```rust\npub fn len(&self) -> usize\n```\n\n[docs](https://doc.rust-lang.org/std/vec/struct.Vec.html)";

    fn content(markup: Markup, value: &str) -> Hover {
        Hover {
            markup,
            value: value.into(),
            truncated: false,
            total_chars: value.chars().count(),
            range: None,
        }
    }

    /// 受け入れ条件: Markdown の答えの描画に渡る要素列が、**`render_block` を通した形と一致**する
    /// （見出し / コードブロック / リンクの 3 種で固定）。カードの経路（`hover_blocks` →
    /// `hover_body`）と、同じ Markdown を `markdown_blocks` → `render_block` へ直に通した列を比べる
    #[test]
    fn markdown_の答えは_render_block_を通る() {
        let theme = Theme::default();
        let hover = content(Markup::Markdown, MARKDOWN);
        let blocks = hover_blocks(&hover);
        // パースは md の唯一の正（`markdown_blocks`）そのもの
        assert_eq!(blocks, preview::markdown_blocks(MARKDOWN));
        // 3 種がそれぞれ見出し / コードブロック（言語つき）/ リンクの段落として読めている
        assert!(matches!(
            &blocks[0].kind,
            MdBlockKind::Heading { level: 1, spans } if spans[0].text == "Vec"
        ));
        assert!(matches!(
            &blocks[1].kind,
            MdBlockKind::CodeBlock { lang: Some(lang), .. } if lang == "rust"
        ));
        assert!(matches!(
            &blocks[2].kind,
            MdBlockKind::Paragraph { spans }
                if spans[0].link_url.as_deref() == Some("https://doc.rust-lang.org/std/vec/struct.Vec.html")
        ));
        let mut card = RecordingSink::default();
        let elements = hover_body(&theme, &blocks, &mut card);
        let mut direct = RecordingSink::default();
        let mut code = 0;
        for block in &blocks {
            let index = matches!(block.kind, MdBlockKind::CodeBlock { .. }).then(|| {
                code += 1;
                code - 1
            });
            let _ = md_view::render_block(&theme, HOVER_BASE, block, index, &mut direct);
        }
        assert_eq!(elements.len(), blocks.len(), "1 ブロック = 1 要素");
        assert_eq!(card.rows, direct.rows, "カードの描画が render_block と違う");
        // 見出しは H1 の太さ・コードブロックは構文色つき・リンクは accent の下線
        let h1 = md_view::heading_look(1, HOVER_BASE, &theme, theme.foreground);
        assert_eq!(card.rows[0].0, "Vec");
        assert_eq!(card.rows[0].3, Some(h1.weight));
        assert_eq!(card.rows[1].0, "pub fn len(&self) -> usize");
        assert!(!card.rows[1].1.is_empty(), "コードブロックに構文色が付く");
        assert_eq!(card.rows[2].0, "docs");
        assert!(
            card.rows[2].1[0].1.underline.is_some(),
            "リンクに下線が付く"
        );
        // リンクは押して開ける形で拾える（当たり判定は md プレビューと同じ規則）
        let links = crate::md_document_links(&blocks);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].url,
            "https://doc.rust-lang.org/std/vec/struct.Vec.html"
        );
    }

    /// 受け入れ条件: プレーンテキストの答えはエスケープされずそのまま出る（Markdown として
    /// 解釈もしない = `*` は斜体にならず `<` はそのまま。改行も残す）
    #[test]
    fn 平文の答えはそのまま出る() {
        let theme = Theme::default();
        let raw = "a < b && *c* `d` <br>\n# not heading";
        let blocks = hover_blocks(&content(Markup::PlainText, raw));
        assert_eq!(blocks.len(), 1);
        let mut sink = RecordingSink::default();
        let _ = hover_body(&theme, &blocks, &mut sink);
        assert_eq!(sink.rows.len(), 1);
        assert_eq!(sink.rows[0].0, raw, "平文が変わった");
        assert!(
            sink.rows[0].1.is_empty(),
            "平文に装飾が付いた: {:?}",
            sink.rows[0].1
        );
        assert_eq!(sink.rows[0].3, None);
        // 同じ文字列を Markdown として読むと装飾も見出しも付く（= 平文の経路が解釈していない証拠）
        let as_markdown = hover_blocks(&content(Markup::Markdown, raw));
        assert_ne!(as_markdown, blocks);
    }

    fn assert_inside(p: HoverPlacement, viewport: (f32, f32), anchor: HoverAnchor, label: &str) {
        let (vw, vh) = viewport;
        assert!(p.left >= 0.0, "{label}: 左へはみ出す {p:?}");
        assert!(p.top >= 0.0, "{label}: 上へはみ出す {p:?}");
        assert!(p.left + p.width <= vw, "{label}: 右へはみ出す {p:?}");
        assert!(p.top + p.height <= vh, "{label}: 下へはみ出す {p:?}");
        assert!(p.height <= p.max_height, "{label}: 上限より高い {p:?}");
        // 語の行を覆わない（真下か真上）
        assert!(
            p.top >= anchor.bottom || p.top + p.height <= anchor.top,
            "{label}: 語の行を覆う {p:?} {anchor:?}"
        );
    }

    /// 受け入れ条件: ウィンドウの四隅それぞれでカードの矩形が可視領域内に収まる（4 ケース）。
    /// 右端では左へずらし、下端では語の行の真上へ返す（語の行は覆わない）
    #[test]
    fn 四隅のどこでもカードは見切れない() {
        let viewport = (1200.0, 800.0);
        let size = (HOVER_CARD_WIDTH, 300.0);
        let line = 18.0;
        let at = |left: f32, top: f32| HoverAnchor {
            left,
            top,
            bottom: top + line,
        };
        // 左上: 語の真下・語の左端
        let a = at(10.0, 40.0);
        let p = hover_card_placement(a, size, viewport);
        assert_inside(p, viewport, a, "左上");
        assert_eq!((p.left, p.top), (10.0, a.bottom));
        // 右上: 左へずらす（語の真下はカードの中 = マウスを下ろせる）
        let a = at(1150.0, 40.0);
        let p = hover_card_placement(a, size, viewport);
        assert_inside(p, viewport, a, "右上");
        assert_eq!(p.top, a.bottom);
        assert!(p.left <= a.left && a.left <= p.left + p.width, "{p:?}");
        // 左下: 語の行の真上へ返す
        let a = at(10.0, 760.0);
        let p = hover_card_placement(a, size, viewport);
        assert_inside(p, viewport, a, "左下");
        assert_eq!(
            p.top + p.height,
            a.top,
            "真上（カードの下端 = 語の行の上端）"
        );
        // 右下: 左へずらして真上へ返す
        let a = at(1150.0, 760.0);
        let p = hover_card_placement(a, size, viewport);
        assert_inside(p, viewport, a, "右下");
        assert_eq!(p.top + p.height, a.top);
        assert!(p.left <= a.left && a.left <= p.left + p.width, "{p:?}");
    }

    /// 上にも下にも入らない（狭い・低いウィンドウ）なら広い方に入る高さまで縮める。
    /// ウィンドウより広いカードは幅を縮める
    #[test]
    fn 入らないときは広い方へ縮めて置く() {
        let viewport = (300.0, 240.0);
        let a = HoverAnchor {
            left: 200.0,
            top: 60.0,
            bottom: 78.0,
        };
        let p = hover_card_placement(a, (HOVER_CARD_WIDTH, 600.0), viewport);
        assert_inside(p, viewport, a, "狭い");
        assert_eq!(p.width, 300.0 - 2.0 * HOVER_EDGE);
        assert_eq!(p.top, a.bottom, "下の方が広い");
        assert_eq!(p.max_height, 240.0 - HOVER_EDGE - a.bottom);
        // 上の方が広ければ上
        let a = HoverAnchor {
            left: 20.0,
            top: 180.0,
            bottom: 198.0,
        };
        let p = hover_card_placement(a, (HOVER_CARD_WIDTH, 600.0), viewport);
        assert_inside(p, viewport, a, "上の方が広い");
        assert_eq!(p.top + p.height, a.top);
        // 測った高さが上限より高くても上限で止まる（本文はカードの中でスクロールする）
        let p = hover_card_placement(
            HoverAnchor {
                left: 0.0,
                top: 10.0,
                bottom: 28.0,
            },
            (HOVER_CARD_WIDTH, 5000.0),
            (1200.0, 2000.0),
        );
        assert_eq!(p.height, HOVER_CARD_MAX_HEIGHT);
    }
}

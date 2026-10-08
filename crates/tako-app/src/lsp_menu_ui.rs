//! コードプレビューの右クリックメニューの LSP 項目（S7 / #1684）の GUI
//!
//! 本文を**識別子の上で**右クリックすると、ペインのメニュー（ヘッダの右クリックと同じ
//! `pane_context_menu`）の先頭へ言語サーバの項目（定義へ移動・宣言へ移動・型定義へ移動・
//! 実装へ移動・ホバー情報を表示（#1893）・コードを整形・選択範囲を整形）を足す。識別子でない位置・受け持つサーバが無い
//! ファイル・Markdown では従来のメニューのまま。
//!
//! ## 出し分けと操作はここに無い
//!
//! - 材料（位置の検査・識別子・位置を含む選択）は `tako_control::dispatch::lsp_menu_prepare`、
//!   出し分けは `tako_core::lsp::menu::items`（サーバの申告に無い項目は出さない）。CLI
//!   `tako lsp menu` / MCP `tako_lsp` の action=menu と同じ 1 本を通る
//! - 項目を押したときの要求は `tako_control::lsp::menu::item_request` が組み（CLI / MCP が組む
//!   要求と同じ）、⌘クリック・編集メニュー・ホバーのキーと同じ 3 段（`prepare_offload` → `run_staged` →
//!   `finish_offload`）へ渡す。**メニューの中に操作の実装を持たない**（UI 限定の操作を作らない）
//!
//! ここが持つのは「どこで開くか・握手を待つあいだの 1 行・答えの差し替え」だけ。
//!
//! ## サーバがまだ握手していないとき
//!
//! サーバは編集モードに入ったときに起きる（FR-3.28）ので、閲覧中のファイルを初めて右クリック
//! したときは能力がまだ分からない。そのときもメニューは**すぐ開き**、LSP の節には押せない 1 行
//! （「言語サーバに問い合わせています…」）を出して、背景でサーバを起こして待つ（定義ジャンプと
//! 同じ起こし方）。答えが届いた時点で同じメニューがまだ開いていれば 1 行を項目へ差し替え、
//! 出す項目が無ければ節ごと消す。閉じた後に届いた答えは捨てる。

use gpui::{Context, MouseDownEvent, Pixels, Point};
use tako_control::dispatch::LspMenuAnswer;
use tako_control::lsp::menu::ItemTarget;
use tako_core::lsp::menu::MenuItem;
use tako_core::PaneId;

use crate::{preview, PaneContextKind, PaneContextMenu, TakoApp};

/// `TAKO_1684_LEGACY=1` で #1684 前の挙動へ戻す（プレビューの本文の右クリックは何もしない）。
/// 同一バイナリで A/B を取る入口（visual-test `lsp-context-menu` の検出力の実証に使う）
pub(crate) fn lsp_menu_legacy() -> bool {
    std::env::var_os("TAKO_1684_LEGACY").is_some()
}

/// 右クリックメニューの LSP の節
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LspMenuSection {
    /// サーバの握手を待っている（押せない 1 行を出す）
    Pending,
    /// 出す項目（空の節は作らない = 空なら `PaneContextMenu::lsp` を `None` にする）
    Items(Vec<MenuItem>),
}

/// 開いているメニューの LSP の節（`PaneContextMenu::lsp`）
#[derive(Debug, Clone)]
pub(crate) struct LspPaneMenu {
    /// 押したときの位置と選択（**開いた瞬間に決める** = 押す前に選択が動いても、出した項目と
    /// 押したときの範囲が食い違わない）
    pub(crate) target: ItemTarget,
    pub(crate) section: LspMenuSection,
    /// 背景の問い合わせの番号（答えが届いたとき、このメニューがまだ開いているかを見る）
    seq: u64,
}

impl LspPaneMenu {
    /// 項目の出た節（visual-test の四隅の検査がメニューを置くのに使う。背景の答えは待たない =
    /// 番号 0 は `lsp_menu_seq` が 1 から数えるので、どの答えとも突き合わない）
    #[cfg(feature = "visual-test")]
    pub(crate) fn with_items(target: ItemTarget, items: Vec<MenuItem>) -> Self {
        Self {
            target,
            section: LspMenuSection::Items(items),
            seq: 0,
        }
    }
}

impl TakoApp {
    /// プレビューの本文の右クリック（#1684）。テキストのプレビュー（コード・Markdown）では
    /// ペインのメニューを右クリックした位置に開き、識別子の上なら LSP の節を足す。
    /// 画像・PDF・動画は従来どおり何もしない
    pub(crate) fn on_preview_body_right_click(
        &mut self,
        pane: PaneId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if lsp_menu_legacy() {
            return;
        }
        let Some(mode) = self.previews.get(&pane).map(|p| p.mode) else {
            return;
        };
        if !matches!(
            mode,
            preview::PreviewMode::Code | preview::PreviewMode::Markdown
        ) {
            return;
        }
        cx.stop_propagation();
        let lsp = self.lsp_pane_menu_at(pane, event.position, cx);
        self.pane_context_menu = Some(PaneContextMenu {
            pane,
            kind: PaneContextKind::Preview,
            position: event.position,
            // プレビューにエージェントは居ない（#1067 の再起動は対象外）
            restart_modes: Vec::new(),
            lsp,
        });
        cx.notify();
    }

    /// 右クリックした位置の LSP の節。識別子の上でない・受け持つサーバが無い・使えないと
    /// 分かっている・出す項目が無いなら `None`（従来のメニューのまま）
    fn lsp_pane_menu_at(
        &mut self,
        pane: PaneId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<LspPaneMenu> {
        let (line, byte, _) = self.code_symbol_hit(pane, position)?;
        // CLI / MCP の `tako lsp menu` と同じ準備（位置の検査・識別子・位置を含む選択）
        let job =
            tako_control::dispatch::lsp_menu_prepare(self, Some(pane.as_u64()), line + 1, byte)
                .ok()?;
        self.lsp_menu_seq = self.lsp_menu_seq.wrapping_add(1);
        let seq = self.lsp_menu_seq;
        let target = job.context().target();
        if let Some(answer) = job.peek() {
            let items = answer.items();
            return (!items.is_empty()).then_some(LspPaneMenu {
                target,
                section: LspMenuSection::Items(items),
                seq,
            });
        }
        // 握手がまだ: 起こして待つのは背景（UI は止めない）。上限は定義ジャンプと同じ
        let staged = cx.background_executor().spawn(async move { job.run() });
        cx.spawn(async move |this, cx| {
            let answer = staged.await;
            let _ = this.update(cx, |app, cx| app.finish_lsp_pane_menu(seq, &answer, cx));
        })
        .detach();
        Some(LspPaneMenu {
            target,
            section: LspMenuSection::Pending,
            seq,
        })
    }

    /// 背景の問い合わせの答えを、まだ開いている同じメニューへ差し替える
    fn finish_lsp_pane_menu(&mut self, seq: u64, answer: &LspMenuAnswer, cx: &mut Context<Self>) {
        let Some(menu) = self.pane_context_menu.as_mut() else {
            return;
        };
        if menu.lsp.as_ref().is_none_or(|lsp| lsp.seq != seq) {
            return;
        }
        let items = answer.items();
        match menu.lsp.as_mut() {
            Some(lsp) if !items.is_empty() => lsp.section = LspMenuSection::Items(items),
            _ => menu.lsp = None,
        }
        cx.notify();
    }

    /// LSP の項目を押した（#1684）。要求は CLI / MCP と同じ `item_request` が組み、
    /// ⌘クリック（定義ジャンプ）・編集メニュー（整形・ホバー = #1893）と同じ入口へ渡す
    pub(crate) fn run_lsp_menu_item(
        &mut self,
        pane: PaneId,
        item: MenuItem,
        target: ItemTarget,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // 利用者自身の操作なので、定義ジャンプは着地したペインへフォーカスを移す（⌘クリックと同じ）
        let Some(request) = tako_control::lsp::menu::item_request(item, &target, Some(true)) else {
            return;
        };
        match item {
            MenuItem::Goto(_) => self.start_lsp_goto_request(pane, request, anchor, cx),
            // #1893: 編集メニュー・キーの「ホバー情報を表示」と同じ入口（カードを出す）
            MenuItem::Hover => self.request_lsp_hover(pane, request, cx),
            MenuItem::Format | MenuItem::FormatSelection => {
                self.format_preview_request(pane, request, cx)
            }
        }
    }
}

/// メニューの描画用: LSP の節を `(id, 表示名)` の並びへ（`sep*` は区切り線・`note*` は押せない 1 行）
pub(crate) fn section_rows(section: &LspMenuSection) -> Vec<(&'static str, &'static str)> {
    let mut rows: Vec<(&'static str, &'static str)> = match section {
        LspMenuSection::Pending => {
            vec![("note-lsp-pending", crate::ui_text::pane_menu::lsp_pending())]
        }
        LspMenuSection::Items(items) => items
            .iter()
            .map(|item| (item.id(), crate::ui_text::pane_menu::lsp_item(*item)))
            .collect(),
    };
    if !rows.is_empty() {
        rows.push(("sep-lsp", ""));
    }
    rows
}

//! 補完（予測変換）の GUI（S5 / #1682）: 打鍵のデバウンス・答えの照合・仮想化した候補の一覧・
//! 5 キーの行き先
//!
//! ## 問い合わせと確定はここに無い
//!
//! 問い合わせは manager の 1 本（`LspManager::completion`。打鍵の要求は次の打鍵の要求が
//! `$/cancelRequest` で捨てる）、確定は dispatch の 1 本（`lsp_completion_apply` = CLI
//! `tako lsp completion --choice` / MCP `tako_lsp` と同じ）。絞り込みと並べ替え（`rank`）・
//! キーの振り分け表（`route_key`）・古い答えを捨てる版の照合（`Session`）は tako-core の
//! 純粋関数。ここが持つのは画面だけ: いつ問い合わせるか（打鍵 → デバウンス）・一覧の描き方・
//! 選んでいる行。
//!
//! ## 仮想化
//!
//! 候補は 1000 件級になる（rust-analyzer は数千件返すことがある）。全件ぶん element を作ると
//! #821 と同じ「1 フレームで数千要素」になるので、一覧は `gpui::list`（可視の行だけを組む）で描く。
//! 組んだ行の数は `rows_built` に数え、visual-test `completion` が 1000 件で可視ぶんだけかを測る
//! （A/B は `TAKO_1682_NO_VIRTUAL_LIST=1` = 全件ぶん組む旧来の形）。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, point, prelude::*, px, size, Bounds, Context, FontWeight, Keystroke, MouseButton, Pixels,
    SharedString,
};
use tako_control::lsp::{CompletionAnswer, CompletionError, CompletionRequest};
use tako_core::lsp::completion::{
    self as comp, At, CompletionItem, CompletionKey, KeyRoute, PopupKey, Session, Trigger, Verdict,
};
use tako_core::PaneId;

use crate::{hsla, preview, rgba, rgba_alpha, TakoApp};

/// 一度に見せる行数（それより多ければ一覧の中でスクロールする）
pub(crate) const COMPLETION_VISIBLE_ROWS: usize = 10;
/// 1 行の高さ
pub(crate) const COMPLETION_ROW_HEIGHT: f32 = 22.0;
/// 一覧の幅
const POPUP_WIDTH: f32 = 400.0;
/// 選んでいる候補の説明の帯に出す行数の上限
const DETAIL_LINES: usize = 4;
/// 説明の帯の 1 行の高さ
const DETAIL_LINE_HEIGHT: f32 = 15.0;

/// `TAKO_1682_NO_VIRTUAL_LIST=1` で**全件ぶん element を組む**旧来の形へ戻す（A/B の注入口。
/// visual-test `completion` の「可視ぶんだけ」がこれで落ちることが検出力の証拠）
fn no_virtual_list() -> bool {
    std::env::var_os("TAKO_1682_NO_VIRTUAL_LIST").is_some()
}

/// 補完の GUI の状態（#1682）
#[derive(Default)]
pub(crate) struct LspCompletionUi {
    /// 打鍵ごとの要求の番号と版（古い答えを捨てる。照合は tako-core の純粋関数）
    session: Session,
    /// 出している一覧（同時に 1 枚）
    pub(crate) popup: Option<CompletionPopup>,
    /// 一覧の仮想リスト（出すたびに件数で作り直す）
    list: Option<gpui::ListState>,
    /// 直近のフレームで組んだ候補の行の数（受け入れ条件: 1000 件でも可視ぶんだけ）
    pub(crate) rows_built: Rc<Cell<usize>>,
    /// 直近に描いた一覧の矩形（visual-test が差分の範囲を見る）
    pub(crate) bounds: Option<Bounds<Pixels>>,
    /// 一覧を出し直すたびに進む（説明の答えが別の一覧へ紛れ込まない）
    generation: u64,
    /// 版の照合を通った答えの数（0 件に絞れて一覧を出さなかった答えも数える。visual-test が
    /// 「答えは来たが出さなかった」と「まだ答えが来ていない」を区別する）
    pub(crate) answers: u64,
}

impl LspCompletionUi {
    /// 一覧の可視の先頭の行の番号（visual-test が「選択に合わせて送られたか」を読む）
    #[cfg(feature = "visual-test")]
    pub(crate) fn list_top(&self) -> Option<usize> {
        self.list.as_ref().map(|l| l.logical_scroll_top().item_ix)
    }
}

/// 出している一覧 1 枚
pub(crate) struct CompletionPopup {
    pub(crate) pane: PaneId,
    path: std::path::PathBuf,
    /// 答えた候補（tako の座標。並びはサーバの順）
    pub(crate) items: Arc<Vec<CompletionItem>>,
    /// 絞り込み後の並び（`items` の添字）
    pub(crate) order: Arc<Vec<usize>>,
    /// 選んでいる位置（`order` の添字）
    pub(crate) selected: usize,
    /// 打っている行（0 起点）と、語の頭（行内の UTF-8 バイト。一覧の左端をここへ合わせる）
    line: usize,
    word_start: usize,
    /// 答えを出したときの本文の版（確定で追加の編集の位置が使えるかの判定）
    answer_version: u64,
    /// 最後に絞り直したときの本文の版（これと違う版 = 打鍵以外で本文が変わった = 閉じる）
    seen_version: u64,
    is_incomplete: bool,
    resolvable: bool,
    generation: u64,
}

impl CompletionPopup {
    /// 選んでいる候補
    pub(crate) fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(*self.order.get(self.selected)?)
    }
}

/// 選んでいる行が見える位置へ一覧を送る。行の高さは一定なので、先頭の行の番号だけで決まる
/// （`ListState::scroll_to_reveal_item` は測った行の高さで数えるので、まだ描いていない
/// 遠くの行 = 末尾から先頭へ回ったとき等で位置がずれる）
fn reveal(list: &gpui::ListState, selected: usize) {
    let top = list.logical_scroll_top().item_ix;
    let first = if selected < top {
        selected
    } else if selected >= top + COMPLETION_VISIBLE_ROWS {
        selected + 1 - COMPLETION_VISIBLE_ROWS
    } else {
        return;
    };
    list.scroll_to(gpui::ListOffset {
        item_ix: first,
        offset_in_item: px(0.0),
    });
}

/// 編集カーソルの位置（tako の座標）とその行の本文
fn caret(edit: &preview::EditState) -> (At, &str) {
    let buffer = &edit.buffer;
    let (line, col) = buffer.line_byte_col(buffer.cursor());
    let text = buffer.text();
    let starts = buffer.line_starts();
    let start = starts.get(line).copied().unwrap_or(text.len());
    let end = starts
        .get(line + 1)
        .map_or(text.len(), |next| next.saturating_sub(1));
    (At::new(line, col), &text[start..end.max(start)])
}

/// 種類の印（字と色）。アイコンは絵文字を使わず、色つきの角丸の中に 1 字を置く
fn kind_badge(kind: Option<u32>, theme: &tako_core::Theme) -> (&'static str, tako_core::Rgb) {
    match kind.map(comp::kind_slug).unwrap_or("text") {
        "method" | "function" | "constructor" => ("f", theme.mauve),
        "variable" | "field" | "property" | "value" | "constant" | "reference" => ("v", theme.teal),
        "class" | "interface" | "struct" | "enum" | "type-parameter" | "unit" => {
            ("T", theme.yellow)
        }
        "module" | "file" | "folder" => ("m", theme.peach),
        "keyword" | "operator" => ("k", theme.accent),
        "enum-member" => ("e", theme.green),
        "snippet" => ("s", theme.red),
        _ => ("t", theme.text_muted),
    }
}

impl TakoApp {
    /// このペインで打鍵の補完を使うか。コードの編集中で、検出表にこの拡張子を受け持つサーバがあり、
    /// LSP が有効で、A/B の旧挙動（`TAKO_1682_LEGACY=1`）でないときだけ
    pub(crate) fn lsp_completion_enabled_for(&self, pane: PaneId) -> bool {
        if tako_control::lsp::completion::legacy() || !self.lsp.is_enabled() {
            return false;
        }
        self.previews.get(&pane).is_some_and(|p| {
            p.mode == preview::PreviewMode::Code
                && tako_core::lsp::servers::resolve(&p.path).is_some()
        }) && self.preview_edits.get(&pane).is_some_and(|e| e.editing)
    }

    /// このペインに一覧が出ているか
    pub(crate) fn lsp_completion_open_in(&self, pane: PaneId) -> bool {
        self.lsp_completion
            .popup
            .as_ref()
            .is_some_and(|p| p.pane == pane)
    }

    /// 本文へ打鍵で文字が入った直後（直接の insertText と IME の確定）。
    ///
    /// 一覧が出ていて語の続きなら**その場で絞り直す**（応答を待たない。一覧が `isIncomplete` なら
    /// デバウンスして問い直す）。出ていなければ、打った 1 文字が補完を起こすか
    /// （`tako_core::lsp::completion::trigger_for`）を見てデバウンスへ積む
    pub(crate) fn completion_after_typing(
        &mut self,
        pane: PaneId,
        inserted: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.lsp_completion_enabled_for(pane)
            || self
                .preview_edits
                .get(&pane)
                .is_some_and(|e| e.search_visible)
        {
            return;
        }
        if self.lsp_completion_open_in(pane) {
            let continues = !inserted.is_empty() && inserted.chars().all(comp::is_word_char);
            if continues && self.refilter_completion(pane, cx) {
                let incomplete = self
                    .lsp_completion
                    .popup
                    .as_ref()
                    .is_some_and(|p| p.is_incomplete);
                if incomplete {
                    self.schedule_completion(pane, Trigger::Incomplete, cx);
                }
                cx.notify();
                return;
            }
            self.close_completion();
        }
        let path = self.previews.get(&pane).map(|p| p.path.clone());
        let triggers = path
            .map(|path| self.lsp.completion_trigger_characters(&path))
            .unwrap_or_default();
        if let Some(trigger) = comp::trigger_for(inserted, &triggers) {
            self.schedule_completion(pane, trigger, cx);
        }
        cx.notify();
    }

    /// 編集コマンド（Backspace・Delete・移動…）の直後。一覧が出ていれば、本文が変わって
    /// まだ語の中なら絞り直し、そうでなければ閉じる
    pub(crate) fn completion_after_command(
        &mut self,
        pane: PaneId,
        changed: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.lsp_completion_open_in(pane) {
            if changed {
                // デバウンス中・問い合わせ中の要求は打った位置の答え = もう当たらない
                // （待っている要求はサーバ側でも捨てる = `$/cancelRequest`）
                self.close_completion();
            }
            return;
        }
        if changed && self.refilter_completion(pane, cx) {
            if self
                .lsp_completion
                .popup
                .as_ref()
                .is_some_and(|p| p.is_incomplete)
            {
                self.schedule_completion(pane, Trigger::Incomplete, cx);
            }
        } else {
            self.close_completion();
        }
        cx.notify();
    }

    /// デバウンスへ積む（`tako_control::lsp::completion::debounce` の後に最新の 1 つだけが問い合わせる）
    fn schedule_completion(&mut self, pane: PaneId, trigger: Trigger, cx: &mut Context<Self>) {
        let seq = self.lsp_completion.session.schedule();
        let delay = tako_control::lsp::completion::debounce();
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = this.update(cx, |app, cx| app.fire_completion(pane, seq, trigger, cx));
        })
        .detach();
    }

    /// デバウンスが明けた: まだ最新の打鍵なら、今のカーソルで問い合わせる（待つのは background）
    fn fire_completion(
        &mut self,
        pane: PaneId,
        seq: u64,
        trigger: Trigger,
        cx: &mut Context<Self>,
    ) {
        if !self.lsp_completion.session.is_latest(seq) || !self.lsp_completion_enabled_for(pane) {
            return;
        }
        let Some(edit) = self.preview_edits.get(&pane) else {
            return;
        };
        if edit.search_visible {
            return;
        }
        let (at, _) = caret(edit);
        let version = edit.buffer.version();
        let request = CompletionRequest {
            path: edit.buffer.path().to_path_buf(),
            line: at.line,
            column: at.col,
            timeout: tako_control::lsp::completion::completion_timeout(),
            // 編集中 = 文書は開いている（サーバは写しを見ている）ので本文を渡さない
            document: None,
            trigger,
            // 打鍵の要求: 次の打鍵の要求が来たら manager が `$/cancelRequest` で捨てる
            superseding: true,
            resolve_top: 0,
        };
        self.lsp_completion.session.sent(seq, version);
        let manager = self.lsp.clone();
        let task = cx
            .background_executor()
            .spawn(async move { manager.completion(&request) });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                app.finish_completion(pane, seq, at, outcome, cx)
            });
        })
        .detach();
    }

    /// 答えが届いた: **番号と版の両方**を照合し（`Session::accept`）、今の本文への答えだけを出す
    fn finish_completion(
        &mut self,
        pane: PaneId,
        seq: u64,
        asked_at: At,
        outcome: Result<CompletionAnswer, CompletionError>,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.preview_edits.get(&pane) else {
            return;
        };
        let version = edit.buffer.version();
        let verdict = self.lsp_completion.session.accept(seq, version);
        // 版が同じでも、問い合わせのあいだにカーソルだけ動いた（マウスで置き直した）なら
        // 別の位置への答え
        if verdict != Verdict::Show
            || caret(edit).0 != asked_at
            || edit.search_visible
            || !edit.editing
            || self.focused_pane() != pane
        {
            return;
        }
        // 失敗（未導入・未応答…）は一覧を出さないだけ（打鍵のたびに通知しない。理由は
        // `tako lsp completion` / `tako lsp status` が返す）
        let Ok(answer) = outcome else {
            return;
        };
        self.lsp_completion.answers = self.lsp_completion.answers.wrapping_add(1);
        self.show_completion(pane, answer, version, cx);
        cx.notify();
    }

    /// 答えを一覧として出す（0 件に絞れたら出さない）
    fn show_completion(
        &mut self,
        pane: PaneId,
        answer: CompletionAnswer,
        version: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.preview_edits.get(&pane) else {
            return;
        };
        let (at, line) = caret(edit);
        let order = comp::rank(&answer.items, line, at);
        if order.is_empty() {
            self.lsp_completion.popup = None;
            return;
        }
        let selected = comp::initial_selection(&order, &answer.items);
        let word_start = comp::word_start(line, at.col);
        let state = gpui::ListState::new(
            order.len(),
            gpui::ListAlignment::Top,
            // 画面外に余分に組むのは 1 行ぶんだけ（可視ぶんだけを組む = 受け入れ条件）
            px(COMPLETION_ROW_HEIGHT),
        );
        reveal(&state, selected);
        self.lsp_completion.generation = self.lsp_completion.generation.wrapping_add(1);
        self.lsp_completion.list = Some(state);
        self.lsp_completion.popup = Some(CompletionPopup {
            pane,
            path: edit.buffer.path().to_path_buf(),
            items: Arc::new(answer.items),
            order: Arc::new(order),
            selected,
            line: at.line,
            word_start,
            answer_version: version,
            seen_version: version,
            is_incomplete: answer.is_incomplete,
            resolvable: answer.resolvable,
            generation: self.lsp_completion.generation,
        });
        self.resolve_selected_completion(cx);
    }

    /// 出ている一覧を今のカーソルの語で絞り直す。語の外へ出た・1 件も残らないなら `false`
    fn refilter_completion(&mut self, pane: PaneId, cx: &mut Context<Self>) -> bool {
        let Some(edit) = self.preview_edits.get(&pane) else {
            return false;
        };
        let (at, line) = caret(edit);
        let version = edit.buffer.version();
        let Some(popup) = self.lsp_completion.popup.as_mut() else {
            return false;
        };
        let in_word = at.line == popup.line
            && at.col >= popup.word_start
            && line
                .get(popup.word_start..at.col)
                .is_some_and(|typed| typed.chars().all(comp::is_word_char));
        if !in_word {
            return false;
        }
        let order = comp::rank(&popup.items, line, at);
        if order.is_empty() {
            return false;
        }
        popup.selected = comp::initial_selection(&order, &popup.items);
        popup.order = Arc::new(order);
        popup.seen_version = version;
        let state = gpui::ListState::new(
            popup.order.len(),
            gpui::ListAlignment::Top,
            px(COMPLETION_ROW_HEIGHT),
        );
        reveal(&state, popup.selected);
        self.lsp_completion.list = Some(state);
        self.resolve_selected_completion(cx);
        true
    }

    /// 一覧を閉じ、待っている要求（補完・説明）を捨てる（manager が `$/cancelRequest` を送る）
    pub(crate) fn close_completion(&mut self) {
        let ui = &mut self.lsp_completion;
        if ui.popup.take().is_some() || ui.list.is_some() {
            ui.list = None;
        }
        ui.bounds = None;
        ui.session.cancel();
        self.lsp.cancel_completion();
    }

    /// ペインを閉じた / プレビューを差し替えた（一覧の行は前の本文の座標）
    pub(crate) fn forget_lsp_completion(&mut self, pane: PaneId) {
        if self.lsp_completion_open_in(pane) {
            self.close_completion();
        }
    }

    /// 一覧がまだ今の本文に当たっているか（描く直前に見る。外れていれば閉じる）。
    /// フォーカスが移った・編集モードを抜けた・カーソルが語の外へ出た・打鍵以外で本文が
    /// 変わった（undo・CLI / MCP の編集）・ペインが消えた、のどれでも外れる
    fn completion_still_valid(&self) -> bool {
        let Some(popup) = &self.lsp_completion.popup else {
            return false;
        };
        if self.focused_pane() != popup.pane || !self.lsp_completion_enabled_for(popup.pane) {
            return false;
        }
        let Some(edit) = self.preview_edits.get(&popup.pane) else {
            return false;
        };
        let (at, _) = caret(edit);
        edit.buffer.version() == popup.seen_version
            && edit.buffer.path() == popup.path
            && at.line == popup.line
            && at.col >= popup.word_start
    }

    /// 補完と検索バーが取り合う 5 キー（Esc / Enter / ↑ / ↓ / Tab）の行き先を表で決める。
    ///
    /// 表の正本は `tako_core::lsp::completion::route_key`（全 20 通りを単体が固定値で縛る）。
    /// `Some(true)` = 補完が取った。`None` = いつもの経路（検索欄 / 本文）へ流す。
    /// ⌘ / Ctrl / ⌥ 付きはキーバインドの側（表に乗らない）、⇧ 付きは一覧を閉じてから流す
    pub(crate) fn route_completion_key(
        &mut self,
        pane: PaneId,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let m = &keystroke.modifiers;
        if m.platform || m.control || m.alt || m.function {
            return None;
        }
        // IME の変換中の Enter / Esc は変換の確定・取り消し（IME が素通しした打鍵でも奪わない）
        if self.ime_composing_in(pane) {
            return None;
        }
        let key = PopupKey::parse(&keystroke.key)?;
        let completion_open = self.lsp_completion_open_in(pane) && self.completion_still_valid();
        let search_open = self
            .preview_edits
            .get(&pane)
            .is_some_and(|e| e.search_visible);
        if m.shift {
            if completion_open {
                self.close_completion();
            }
            return None;
        }
        match comp::route_key(search_open, completion_open, key) {
            KeyRoute::Completion(action) => {
                self.completion_key(pane, action, cx);
                Some(true)
            }
            KeyRoute::SearchBar { close_completion } => {
                if close_completion {
                    self.close_completion();
                }
                None
            }
            KeyRoute::Editor => None,
        }
    }

    fn completion_key(&mut self, pane: PaneId, action: CompletionKey, cx: &mut Context<Self>) {
        match action {
            CompletionKey::Close => self.close_completion(),
            CompletionKey::Accept => self.accept_completion(pane, cx),
            CompletionKey::Previous | CompletionKey::Next => {
                if let Some(popup) = self.lsp_completion.popup.as_mut() {
                    let count = popup.order.len().max(1);
                    popup.selected = match action {
                        CompletionKey::Previous => (popup.selected + count - 1) % count,
                        _ => (popup.selected + 1) % count,
                    };
                    if let Some(list) = &self.lsp_completion.list {
                        reveal(list, popup.selected);
                    }
                }
                self.resolve_selected_completion(cx);
            }
        }
        cx.notify();
    }

    /// 選んでいる候補で確定する（Enter / Tab / 行の押下）。**dispatch の 1 本を通す**
    /// （CLI / MCP の `choice` と同じ = 置き換えの範囲・自動 import・undo 1 回で戻る が揃う）
    pub(crate) fn accept_completion(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(popup) = self.lsp_completion.popup.take() else {
            return;
        };
        self.close_completion();
        let Some(item) = popup.selected_item().cloned() else {
            return;
        };
        let Some(edit) = self.preview_edits.get(&pane) else {
            return;
        };
        let (at, _) = caret(edit);
        let version = edit.buffer.version();
        let same_text = version == popup.answer_version;
        match tako_control::dispatch::lsp_completion_apply(
            self,
            pane,
            &item,
            at,
            Some(version),
            same_text,
        ) {
            Ok(_) => self.drive_autosave(cx),
            Err(error) => {
                if let Some(edit) = self.preview_edits.get_mut(&pane) {
                    edit.message = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    /// 行を押した: その候補を選んで確定する
    fn click_completion_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(popup) = self.lsp_completion.popup.as_mut() else {
            return;
        };
        popup.selected = index.min(popup.order.len().saturating_sub(1));
        let pane = popup.pane;
        self.accept_completion(pane, cx);
    }

    /// 選んでいる候補に説明が無く、サーバが resolve を持つなら補ってもらう（background。
    /// 前の説明の要求は manager が取り消す）。答えは同じ一覧が出ている間だけ反映する
    fn resolve_selected_completion(&mut self, cx: &mut Context<Self>) {
        let Some(popup) = &self.lsp_completion.popup else {
            return;
        };
        if !popup.resolvable {
            return;
        }
        let Some(&index) = popup.order.get(popup.selected) else {
            return;
        };
        let item = &popup.items[index];
        if item.documentation.is_some() {
            return;
        }
        let (raw, path, generation) = (item.raw.clone(), popup.path.clone(), popup.generation);
        let manager = self.lsp.clone();
        let task = cx
            .background_executor()
            .spawn(async move { manager.resolve_completion(&path, &raw) });
        cx.spawn(async move |this, cx| {
            let Ok(resolved) = task.await else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                let Some(popup) = app.lsp_completion.popup.as_mut() else {
                    return;
                };
                if popup.generation != generation {
                    return;
                }
                if let Some(item) = Arc::make_mut(&mut popup.items).get_mut(index) {
                    comp::apply_resolved(item, &resolved);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 候補の一覧（打っている語の頭の真下。下に入らなければ上）。外れていれば閉じて何も描かない
    pub(crate) fn render_lsp_completion(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.lsp_completion.popup.is_some() && !self.completion_still_valid() {
            self.close_completion();
        }
        let popup = self.lsp_completion.popup.as_ref()?;
        // 位置は描いた行のレイアウトから（語の行が画面外なら出さない）
        let layout = self
            .preview_text_layouts
            .get(&popup.pane)?
            .get(popup.line)?
            .as_ref()?;
        let anchor = layout.position_for_index(popup.word_start)?;
        let line_height = layout.line_height();
        let theme = self.theme.clone();
        let count = popup.order.len();
        let rows = count.min(COMPLETION_VISIBLE_ROWS);
        let list_height = rows as f32 * COMPLETION_ROW_HEIGHT;
        let detail_lines: Vec<String> = popup
            .selected_item()
            .map(|item| {
                item.detail
                    .iter()
                    .chain(item.documentation.iter())
                    .flat_map(|text| text.lines())
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with("```"))
                    .take(DETAIL_LINES)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let detail_height = if detail_lines.is_empty() {
            0.0
        } else {
            detail_lines.len() as f32 * DETAIL_LINE_HEIGHT + 9.0
        };
        // 縁（1px × 2）+ 内側の余白（4px × 2）
        let total = list_height + 10.0 + detail_height;
        let viewport = window.viewport_size();
        let below = f32::from(anchor.y + line_height) + 2.0;
        let top = if below + total > f32::from(viewport.height) - 4.0
            && f32::from(anchor.y) - total - 2.0 >= 0.0
        {
            f32::from(anchor.y) - total - 2.0
        } else {
            below
        };
        // 行の印（14px + 余白）のぶん左へ寄せ、語の頭と候補の字の頭を揃える
        let left = (f32::from(anchor.x) - 31.0)
            .min(f32::from(viewport.width) - POPUP_WIDTH - 4.0)
            .max(0.0);
        let bounds = Bounds {
            origin: point(px(left), px(top)),
            size: size(px(POPUP_WIDTH), px(total)),
        };
        self.lsp_completion.bounds = Some(bounds);
        self.lsp_completion.rows_built.set(0);
        let body = if no_virtual_list() {
            // A/B の注入口: 全件ぶん組む（1000 件なら 1000 行）
            div()
                .id("lsp-completion-all")
                .h(px(list_height))
                .overflow_y_scroll()
                .children((0..count).map(|ix| self.render_completion_row(ix, cx)))
                .into_any_element()
        } else {
            let state = self.lsp_completion.list.clone()?;
            let app = cx.entity().downgrade();
            gpui::list(state, move |ix, _window, cx| {
                let Some(app) = app.upgrade() else {
                    return div().into_any_element();
                };
                app.update(cx, |app, cx| app.render_completion_row(ix, cx))
            })
            .h(px(list_height))
            .w_full()
            .into_any_element()
        };
        let detail = (!detail_lines.is_empty()).then(|| {
            div()
                .mt(px(4.0))
                .pt(px(4.0))
                .px(px(8.0))
                .border_t_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(11.0))
                .line_height(px(DETAIL_LINE_HEIGHT))
                .text_color(hsla(theme.text_secondary))
                .children(detail_lines.into_iter().map(|line| {
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(SharedString::from(line))
                }))
        });
        Some(
            div()
                .id("lsp-completion")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(POPUP_WIDTH))
                .h(px(total))
                .p(px(4.0))
                .bg(rgba(theme.surface_1))
                .border_1()
                .border_color(hsla(theme.border_default))
                .rounded(px(6.0))
                .shadow_lg()
                // 下の本文のホイール・押下を通さない（一覧の中のスクロールは list が受ける）
                .occlude()
                .child(body)
                .children(detail)
                .into_any_element(),
        )
    }

    /// 候補の 1 行（`gpui::list` が**可視の行だけ**呼ぶ）。組んだ数を `rows_built` に数える
    fn render_completion_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let counter = &self.lsp_completion.rows_built;
        counter.set(counter.get() + 1);
        let Some(popup) = &self.lsp_completion.popup else {
            return div().into_any_element();
        };
        let Some(item) = popup.order.get(ix).and_then(|&i| popup.items.get(i)) else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        let selected = ix == popup.selected;
        let (letter, color) = kind_badge(item.kind, theme);
        let label_color = if item.deprecated {
            theme.text_muted
        } else {
            theme.foreground
        };
        div()
            .id(("lsp-completion-row", ix))
            .h(px(COMPLETION_ROW_HEIGHT))
            .w_full()
            .px(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .rounded(px(4.0))
            .when(selected, |d| d.bg(rgba_alpha(theme.accent, 0.22)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.click_completion_row(ix, cx);
                }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded(px(3.0))
                    .bg(rgba_alpha(color, 0.22))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(color))
                    .child(SharedString::from(letter)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(theme.font_family.clone())
                    .text_size(px(12.5))
                    .text_color(hsla(label_color))
                    .when(item.deprecated, |d| d.line_through())
                    .child(SharedString::from(item.label.clone())),
            )
            .children(item.detail.as_ref().map(|detail| {
                div()
                    .flex_none()
                    .max_w(px(POPUP_WIDTH * 0.45))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_faint))
                    .child(SharedString::from(
                        detail.lines().next().unwrap_or_default().to_string(),
                    ))
            }))
            .into_any_element()
    }
}

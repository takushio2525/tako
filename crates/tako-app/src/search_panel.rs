//! 右パネルの `search` ビュー —— ワークスペース横断検索（FR-3.43 / #1694）
//!
//! ## 正本はここに無い
//!
//! 結果の正本は `tako_core::workspace_search::SearchStore`（`TakoApp::workspace_search`）で、
//! この画面はそれを読むだけ。検索欄の Enter・トグル・行のクリック（⌘クリックも同じ）・見出しの ×
//! は CLI / MCP と同じ `Request::WorkspaceSearch`（`search --show` / `open` / `clear`）を dispatch で通す
//! （設計原則 5。UI 層に閉じた操作を作らない = 画面で出した結果を `tako search --results` で読め、
//! `--open N` で同じ行を開ける）。
//!
//! 走査と照合は `prepare_offload` → background → `finish_offload` の 3 段（`dispatch_off_ui`）を
//! 通す。**UI スレッドで走査しない**（番犬 `issue1694_workspace_search_watchdog`）。
//!
//! 検索欄（[`SearchUi`]）だけは画面の持ち物（打ちかけの文字は AI へ見せる状態ではない）。
//! CLI の `--show` が出した検索は欄へ写す（画面と結果の検索語を食い違わせない）。
//!
//! ## 置換（#2045）
//!
//! 検索欄の左の > で置換欄を開く。置換欄の文字列で作ったプレビュー（行ごとの「消す字 → 足す字」）は
//! CLI `tako search --replace <置換後>`（検索語なし = 右パネルの結果へのプレビュー）と同じ要求で頼み、
//! 置き場に置かれたものを行に並べる（打っている最中は少し待ってから頼む）。行の「置換」・ファイルの
//! 「ファイルを置換」・見出しの「すべて置換」は同じ要求に `confirm`（CLI の `--yes`）を付けたもの、
//! 「元に戻す」は `tako search --undo --yes`。照合・退避・書き込みは background（同じ 3 段）。

use gpui::{div, prelude::*, px, svg, Context, HighlightStyle, MouseButton, MouseDownEvent};
use gpui::{SharedString, StrikethroughStyle, StyledText};
use tako_control::protocol::Request;
use tako_core::workspace_replace::{Mark, RowPreview};
use tako_core::workspace_search::{QueryError, Stop};
use tako_core::PaneOrigin;

use super::*;

/// 描く行の上限（残りは「ほか N 件」。全件は CLI / MCP が返す）
const ROWS: usize = 500;
/// 検索語の上限（バイト。貼り付けた巨大な文字列で欄と照合器を膨らませない）
const QUERY_MAX_BYTES: usize = 1000;
/// 置換欄を打ってからプレビューを頼むまで（毎打鍵でファイルを読み直さない）
const PREVIEW_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// 検索欄の状態（結果そのものは `TakoApp::workspace_search`）
#[derive(Debug, Default)]
pub(crate) struct SearchUi {
    pub(crate) input: crate::text_field::TextField,
    /// 欄が打鍵を受けている（クリック / ⇧⌘F で立ち、Esc・ほかのクリックで落ちる）
    pub(crate) focused: bool,
    pub(crate) regex: bool,
    pub(crate) case_sensitive: bool,
    /// 欄へ写した最後の検索の番号（CLI の `--show` が出した検索を欄に映すため）
    synced_seq: u64,
    /// dispatch が断った理由（置き場に残らない失敗 = 範囲が無い等。次の検索で消える）
    pub(crate) notice: Option<String>,
    /// 置換欄を開いている（#2045）
    pub(crate) replace_open: bool,
    pub(crate) replace_input: crate::text_field::TextField,
    /// 打鍵を受けているのが置換欄（`focused` と組で使う。Tab で行き来する）
    pub(crate) replace_focused: bool,
    /// プレビューを頼んだ（結果の番号, 置換の世代, 置換欄の文字列）。同じものを 2 度頼まない
    preview_requested: Option<(u64, u64, String)>,
    /// プレビューを頼むのを遅らせている
    preview_scheduled: bool,
    /// 置き場のプレビューを置換欄へ写した回数（CLI が置いたプレビューを欄へ写す目印）
    synced_previews: u64,
}

impl TakoApp {
    /// ⇧⌘F（Windows は Ctrl+Shift+G）・編集メニュー・パレットの「ファイル横断検索」。
    /// 右パネルを search ビューで開いて検索欄へフォーカスする。プレビュー / ターミナルに 1 行の
    /// 選択があれば検索語へ入れる（VS Code と同じ）
    pub(crate) fn open_find_in_files(&mut self, cx: &mut Context<Self>) {
        let selection = self
            .preview_selected_text()
            .or_else(|| self.focused_session().and_then(|s| s.selection_text()))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && !t.contains('\n') && t.len() <= QUERY_MAX_BYTES);
        self.clear_text_input_focus();
        self.panel_visible = true;
        self.panel_view = PanelView::Search;
        if let Some(text) = selection {
            self.search_ui.input.set_text(text);
        }
        self.search_ui.input.move_end();
        self.search_ui.focused = true;
        cx.notify();
    }

    /// 検索欄が打鍵を受けているか（見えているときだけ。#503 と同じ「stale フラグで拾わない」規則）
    pub(crate) fn search_input_active(&self) -> bool {
        self.search_ui.focused && self.panel_visible && self.panel_view == PanelView::Search
    }

    /// 打鍵を受けている欄が置換欄か（#2045）
    fn replace_field_active(&self) -> bool {
        self.search_ui.replace_open && self.search_ui.replace_focused
    }

    /// 検索欄へ文字を入れる（打鍵・⌘V・IME の確定の共通経路。1 行なので制御文字は落とす）。
    /// 置換欄が打鍵を受けていればそちらへ入れる（#2045。IME の宛先は同じ `WorkspaceSearch`）
    pub(crate) fn search_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
        if !filtered.is_empty() {
            if self.replace_field_active() {
                self.search_ui
                    .replace_input
                    .insert(&filtered, QUERY_MAX_BYTES, false);
                self.schedule_replace_preview(cx);
            } else {
                self.search_ui
                    .input
                    .insert(&filtered, QUERY_MAX_BYTES, false);
            }
        }
        cx.notify();
    }

    /// 検索欄のキー処理。扱ったら `true`（呼び出し側が打鍵を消費する）
    pub(crate) fn handle_search_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = keystroke.modifiers;
        match keystroke.key.as_str() {
            // 置換欄の Enter はプレビューをすぐ頼む（書き換えるのはボタンだけ）
            "enter" if self.replace_field_active() => {
                self.request_replace_preview(cx);
                true
            }
            "enter" => {
                self.run_workspace_search(cx);
                true
            }
            // 置換欄を開いていれば Tab で検索欄 ↔ 置換欄
            "tab" if self.search_ui.replace_open && !m.platform && !m.control => {
                self.search_ui.replace_focused = !self.search_ui.replace_focused;
                self.discard_app_text_ime(AppTextInput::WorkspaceSearch);
                cx.notify();
                true
            }
            "escape" => {
                self.search_ui.focused = false;
                self.discard_app_text_ime(AppTextInput::WorkspaceSearch);
                cx.notify();
                true
            }
            "v" if m.platform => {
                // ⌘V はクリップボード読み出しの共通経路（`paste`）へ任せる
                self.paste(cx);
                true
            }
            // ⌥C / ⌥R（Windows は Alt+C / Alt+R）でトグル（VS Code の検索欄と同じ）
            "c" if m.alt && !m.platform && !m.control => {
                self.toggle_workspace_search_option(false, cx);
                true
            }
            "r" if m.alt && !m.platform && !m.control => {
                self.toggle_workspace_search_option(true, cx);
                true
            }
            key => {
                if self.replace_field_active() {
                    if self.search_ui.replace_input.handle_edit_key(key) {
                        self.schedule_replace_preview(cx);
                        cx.notify();
                        return true;
                    }
                } else if self.search_ui.input.handle_edit_key(key) {
                    cx.notify();
                    return true;
                }
                // 修飾キー付きはショートカットなので通す（⌘W 等）
                if m.platform || m.control {
                    return false;
                }
                if let Some(text) = keystroke.key_char.as_ref().filter(|t| !t.is_empty()) {
                    let text = text.clone();
                    self.search_insert(&text, cx);
                }
                // 未知のキーでも欄がフォーカスされている間はターミナルへ漏らさない
                true
            }
        }
    }

    /// 検索欄の語で探す（Enter / トグル）。CLI `tako search <語> --show` と同じ要求
    pub(crate) fn run_workspace_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_ui.input.text().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.search_ui.notice = None;
        let request = Request::WorkspaceSearch {
            action: "search".into(),
            query: Some(query),
            path: None,
            pane: Some(self.focused_pane().as_u64()),
            regex: Some(self.search_ui.regex),
            case_sensitive: Some(self.search_ui.case_sensitive),
            show: Some(true),
            // 画面は置き場を読むので、応答には行を載せない
            limit: Some(1),
            item: None,
            focus: None,
            replacement: None,
            file: None,
            confirm: None,
        };
        self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {
            // 壊れた正規表現は置き場に理由が残る（ビューが出す）。それ以外の断り（範囲が無い等）は
            // 置き場に残らないので欄の下へ出す
            if let Err(e) = result {
                if app.workspace_search.error().is_none() {
                    app.search_ui.notice = Some(e.to_string());
                }
            }
            cx.notify();
        });
        // 「探しています」を出す（置き場への登録は準備の段で済んでいる）
        if let Some(running) = self.workspace_search.running() {
            self.search_ui.synced_seq = running.seq;
        }
        cx.notify();
    }

    /// 大文字小文字（`regex = false`）/ 正規表現（`regex = true`）のトグル。結果を出していれば
    /// 同じ語で探し直す
    fn toggle_workspace_search_option(&mut self, regex: bool, cx: &mut Context<Self>) {
        if regex {
            self.search_ui.regex = !self.search_ui.regex;
        } else {
            self.search_ui.case_sensitive = !self.search_ui.case_sensitive;
        }
        if !self.workspace_search.is_empty() {
            self.run_workspace_search(cx);
        }
        cx.notify();
    }

    /// 行を開く（右パネルの行のクリック = CLI `tako search --open N` と同じ要求）
    fn open_search_item(&mut self, item: usize, cx: &mut Context<Self>) {
        let request = Request::WorkspaceSearch {
            action: "open".into(),
            query: None,
            path: None,
            pane: Some(self.focused_pane().as_u64()),
            regex: None,
            case_sensitive: None,
            show: None,
            limit: None,
            item: Some(item),
            // 手で押した = 開いた行を読みに行く（定義ジャンプの ⌘クリックと同じ）
            focus: Some(true),
            replacement: None,
            file: None,
            confirm: None,
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.search_ui.notice = Some(e.to_string());
        }
        self.sync_filetree_roots();
        cx.notify();
    }

    /// 見出しの ×（CLI `tako search --clear` と同じ要求。走っている検索も取り消す）
    fn clear_workspace_search(&mut self, cx: &mut Context<Self>) {
        let request = Request::WorkspaceSearch {
            action: "clear".into(),
            query: None,
            path: None,
            pane: None,
            regex: None,
            case_sensitive: None,
            show: None,
            limit: None,
            item: None,
            focus: None,
            replacement: None,
            file: None,
            confirm: None,
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.search_ui.notice = Some(e.to_string());
        }
        cx.notify();
    }

    /// 置換欄の開閉（検索欄の左の >。#2045）。開いたら置換欄へフォーカスしてプレビューを頼み、
    /// 閉じたら置き場のプレビューを下ろす（行の表示を検索だけへ戻す）
    fn toggle_replace(&mut self, cx: &mut Context<Self>) {
        self.search_ui.replace_open = !self.search_ui.replace_open;
        self.clear_text_input_focus();
        if self.search_ui.replace_open {
            self.search_ui.focused = true;
            self.search_ui.replace_focused = true;
            self.search_ui.replace_input.move_end();
            self.search_ui.preview_requested = None;
            self.request_replace_preview(cx);
        } else {
            self.search_ui.replace_focused = false;
            self.workspace_search.replace_mut().clear_preview();
        }
        cx.notify();
    }

    /// 置換の要求（CLI `tako search --replace <置換欄> [--item N | --file F] [--yes]` と同じ形）
    fn replace_request(&self, item: Option<usize>, file: Option<String>, confirm: bool) -> Request {
        Request::WorkspaceSearch {
            action: "replace".into(),
            query: None,
            path: None,
            pane: Some(self.focused_pane().as_u64()),
            regex: None,
            case_sensitive: None,
            show: None,
            // 画面は置き場を読むので、応答には行を載せない
            limit: Some(1),
            item,
            focus: None,
            replacement: Some(self.search_ui.replace_input.text().to_string()),
            file,
            confirm: confirm.then_some(true),
        }
    }

    /// 打っている最中は少し待ってからプレビューを頼む（待つ間の打鍵は 1 回にまとめる）
    fn schedule_replace_preview(&mut self, cx: &mut Context<Self>) {
        if self.search_ui.preview_scheduled {
            return;
        }
        self.search_ui.preview_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let _ = this.update(cx, |app, cx| {
                app.search_ui.preview_scheduled = false;
                app.request_replace_preview(cx);
            });
        })
        .detach();
    }

    /// 右パネルの結果へのプレビューを頼む（同じ結果・世代・文字列ではもう頼まない）
    fn request_replace_preview(&mut self, cx: &mut Context<Self>) {
        if !self.search_ui.replace_open {
            return;
        }
        let Some(seq) = self.workspace_search.shown().map(|s| s.seq) else {
            return;
        };
        let key = (
            seq,
            self.workspace_search.replace().generation(),
            self.search_ui.replace_input.text().to_string(),
        );
        if self.search_ui.preview_requested.as_ref() == Some(&key) {
            return;
        }
        self.search_ui.preview_requested = Some(key);
        let request = self.replace_request(None, None, false);
        self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {
            // 置換の参照の誤りは置き場に理由が残る（欄の下に出る）。それ以外の断りは欄の下へ
            if let Err(e) = result {
                let in_store = app
                    .workspace_search
                    .replace()
                    .preview()
                    .is_some_and(|p| p.error.is_some());
                if !in_store {
                    app.search_ui.notice = Some(e.to_string());
                }
            }
            cx.notify();
        });
    }

    /// 当てる（行の「置換」= item・ファイルの「ファイルを置換」= file・見出しの「すべて置換」= どちらも無し）
    fn apply_replace(&mut self, item: Option<usize>, file: Option<String>, cx: &mut Context<Self>) {
        self.search_ui.notice = None;
        let request = self.replace_request(item, file, true);
        self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {
            match result {
                Err(e) => app.search_ui.notice = Some(e.to_string()),
                // 当てなかったファイルがあれば、最初の 1 つの理由を欄の下に出す（行にも理由が並ぶ）
                Ok(out) => {
                    app.search_ui.notice = out["files"].as_array().and_then(|files| {
                        files.iter().find_map(|f| {
                            let reason = f["reason"].as_str()?;
                            (f["status"] != "unchanged" && f["status"] != "already-applied").then(
                                || {
                                    crate::ui_text::panel::search_replace_skipped(
                                        f["path"].as_str().unwrap_or(""),
                                        reason,
                                    )
                                },
                            )
                        })
                    });
                }
            }
            app.search_ui.preview_requested = None;
            app.request_replace_preview(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// 最後の置換を戻す（CLI `tako search --undo --yes` と同じ要求）
    fn undo_replace(&mut self, cx: &mut Context<Self>) {
        self.search_ui.notice = None;
        let request = Request::WorkspaceSearch {
            action: "undo".into(),
            query: None,
            path: None,
            pane: None,
            regex: None,
            case_sensitive: None,
            show: None,
            limit: None,
            item: None,
            focus: None,
            replacement: None,
            file: None,
            confirm: Some(true),
        };
        self.dispatch_off_ui(request, PaneOrigin::User, cx, |app, result, cx| {
            match result {
                Err(e) => app.search_ui.notice = Some(e.to_string()),
                Ok(out) => {
                    app.search_ui.notice = out["files"].as_array().and_then(|files| {
                        files.iter().find_map(|f| {
                            (f["status"] == "changed"
                                || f["status"] == "edited"
                                || f["status"] == "backup-missing"
                                || f["status"] == "write-failed")
                                .then(|| {
                                    crate::ui_text::panel::search_replace_skipped(
                                        f["path"].as_str().unwrap_or(""),
                                        f["reason"].as_str().unwrap_or(""),
                                    )
                                })
                        })
                    });
                }
            }
            app.search_ui.preview_requested = None;
            app.request_replace_preview(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// CLI が置いたプレビュー（`tako search --replace <置換後>`）を置換欄へ写して開く。
    /// 打っている最中（欄にフォーカス）は書き換えない
    fn sync_replace_input(&mut self) {
        let placed = self.workspace_search.replace().previews_placed();
        if placed <= self.search_ui.synced_previews {
            return;
        }
        self.search_ui.synced_previews = placed;
        let Some(preview) = self.workspace_search.replace().preview() else {
            return;
        };
        if self.search_ui.focused && self.search_ui.replace_focused {
            return;
        }
        if preview.replacement != self.search_ui.replace_input.text() {
            self.search_ui
                .replace_input
                .set_text(preview.replacement.clone());
            self.search_ui.replace_input.move_end();
        }
        self.search_ui.replace_open = true;
    }

    /// CLI の `--show` が出した検索を欄へ写す（画面と結果の検索語を食い違わせない）。
    /// 打っている最中（欄にフォーカス）は書き換えない
    fn sync_search_input(&mut self) {
        let latest = self
            .workspace_search
            .running()
            .map(|r| (r.seq, r.query.clone()))
            .or_else(|| {
                self.workspace_search
                    .shown()
                    .map(|s| (s.seq, s.query.clone()))
            });
        let Some((seq, query)) = latest else {
            return;
        };
        if seq <= self.search_ui.synced_seq || self.search_ui.focused {
            return;
        }
        self.search_ui.synced_seq = seq;
        self.search_ui.input.set_text(query.text);
        self.search_ui.input.move_end();
        self.search_ui.regex = query.regex;
        self.search_ui.case_sensitive = query.case_sensitive;
    }

    /// 検索欄 / 置換欄の 1 つ（#2045 で置換欄も同じ形）。押すとその欄が打鍵を受ける。
    /// IME の宛先はどちらも `AppTextInput::WorkspaceSearch`（下線とキャレットは打鍵を受ける欄にだけ出す）
    fn search_field(
        &self,
        replace: bool,
        active: bool,
        theme: &tako_core::Theme,
        probe: &dyn Fn(String) -> gpui::Canvas<()>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let input = if replace {
            &self.search_ui.replace_input
        } else {
            &self.search_ui.input
        };
        let (before, after) = input.split_at_caret();
        let visible = ((self.panel_width - 130.0) / 6.5).max(6.0) as usize;
        let empty = input.text().is_empty();
        let id = if replace {
            "search-replace-input"
        } else {
            "search-input"
        };
        let placeholder = if replace {
            crate::ui_text::panel::search_replace_placeholder()
        } else {
            crate::ui_text::panel::search_placeholder()
        };
        div()
            .id(id)
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .px(px(6.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .cursor(gpui::CursorStyle::IBeam)
            .border_1()
            .border_color(hsla(if active {
                theme.accent_border_muted
            } else {
                theme.border_inner
            }))
            .bg(rgba(theme.surface_0))
            .text_size(px(11.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    // #496: 押した瞬間にルートの dismiss で自分が消えないよう伝播を止める
                    this.clear_text_input_focus();
                    this.search_ui.focused = true;
                    this.search_ui.replace_focused = replace;
                    if replace {
                        this.search_ui.replace_input.move_end();
                    } else {
                        this.search_ui.input.move_end();
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .when(empty && !active, |d| {
                d.child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(hsla(theme.text_faint))
                        .child(placeholder),
                )
            })
            .when(!empty, |d| {
                d.child(div().flex_none().child(SharedString::from(
                    crate::right_panel::tail_chars(before, visible),
                )))
            })
            // #561: 未確定文字列は入力欄の中へ出す（変換の宛先 = AppTextInput::WorkspaceSearch）
            .children(
                active
                    .then(|| self.text_input_marked(AppTextInput::WorkspaceSearch, theme))
                    .flatten(),
            )
            .when(active, |d| {
                d.child(self.text_input_caret(AppTextInput::WorkspaceSearch, theme))
            })
            .when(!empty, |d| {
                d.child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(SharedString::from(crate::right_panel::head_chars(
                            after, visible,
                        ))),
                )
            })
            .child(probe(id.to_string()))
    }

    /// 置換の文字のボタン（すべて置換 / 元に戻す / ファイルを置換 / 行の置換。#2045）。
    /// 押せないときは薄く出し、押しても何もしない（呼び手の on_click は `enabled` のときだけ効く）
    fn search_button(
        &self,
        id: impl Into<SharedString>,
        label: &'static str,
        enabled: bool,
        theme: &tako_core::Theme,
        probe: &dyn Fn(String) -> gpui::Canvas<()>,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id: SharedString = id.into();
        div()
            .id(gpui::ElementId::Name(id.clone()))
            .relative()
            .flex_none()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .text_size(px(10.5))
            .whitespace_nowrap()
            .border_1()
            .border_color(hsla(theme.border_inner))
            .text_color(hsla(if enabled {
                theme.foreground
            } else {
                theme.text_faint
            }))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(hsla(theme.surface_hover)))
            })
            .child(label)
            .child(probe(id.to_string()))
    }

    pub(crate) fn render_search_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.sync_search_input();
        self.sync_replace_input();
        let theme = self.theme.clone();
        let probe_bounds = self.panel_click_probe_bounds.clone();
        let mono = SharedString::from(theme.font_family.clone());
        let focused = self.search_input_active();

        let mut root = div()
            .id("panel-search-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground));
        let note = |text: String| {
            div()
                .px(px(10.0))
                .py(px(8.0))
                .text_size(px(11.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        // visual-test が実フレームの幾何で在処を読むための実矩形（診断ビューと同じ）
        let probe = |key: String| {
            let probe = probe_bounds.clone();
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    probe.borrow_mut().insert(key.clone(), bounds);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };

        // --- 検索欄 + トグル（左の > で置換欄を開く。#2045） ---
        let replace_active = focused && self.replace_field_active();
        let query_active = focused && !self.replace_field_active();
        // 欄は `cx` を借りるので、`cx` を掴むトグルの閉包より先に組む
        let query_field = self.search_field(false, query_active, &theme, &probe, cx);
        let replace_field = self
            .search_ui
            .replace_open
            .then(|| self.search_field(true, replace_active, &theme, &probe, cx));
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: &'static str| {
            let tip_theme = theme.clone();
            div()
                .id(id)
                .relative()
                .flex_none()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .font_family(mono.clone())
                .text_size(px(11.0))
                .border_1()
                .border_color(hsla(if on {
                    theme.accent_border_muted
                } else {
                    theme.border_inner
                }))
                .when(on, |d| d.bg(rgba(theme.accent_muted)))
                .text_color(hsla(if on {
                    theme.foreground
                } else {
                    theme.text_muted
                }))
                .tooltip(move |_, cx| {
                    cx.new(|_| crate::tab_bar::HintTooltip::new(tip.to_string(), tip_theme.clone()))
                        .into()
                })
                // ルートの一括 dismiss（#503）から自分を守る（押した瞬間に欄のフォーカスを落とさない）
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .child(label)
                .child(probe(id.to_string()))
        };
        let expander = {
            let tip_theme = theme.clone();
            div()
                .id("search-replace-toggle")
                .relative()
                .flex_none()
                .px(px(2.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(theme.surface_hover)))
                .tooltip(move |_, cx| {
                    cx.new(|_| {
                        crate::tab_bar::HintTooltip::new(
                            crate::ui_text::panel::search_replace_toggle().to_string(),
                            tip_theme.clone(),
                        )
                    })
                    .into()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_replace(cx)))
                .child(
                    svg()
                        .path(if self.search_ui.replace_open {
                            crate::file_icons::ui_icon::CHEVRON_DOWN
                        } else {
                            crate::file_icons::ui_icon::CHEVRON_RIGHT
                        })
                        .w(px(10.0))
                        .h(px(10.0))
                        .text_color(hsla(theme.text_muted)),
                )
                .child(probe("search-replace-toggle".to_string()))
        };
        root =
            root.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .pl(px(4.0))
                    .pr(px(8.0))
                    .py(px(6.0))
                    .when(!self.search_ui.replace_open, |d| {
                        d.border_b_1().border_color(hsla(theme.border_inner))
                    })
                    .child(expander)
                    .child(query_field)
                    .child(
                        toggle(
                            "search-case",
                            "Aa",
                            self.search_ui.case_sensitive,
                            crate::ui_text::panel::search_case_toggle(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_workspace_search_option(false, cx)
                        })),
                    )
                    .child(
                        toggle(
                            "search-regex",
                            ".*",
                            self.search_ui.regex,
                            crate::ui_text::panel::search_regex_toggle(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_workspace_search_option(true, cx)
                        })),
                    ),
            );
        // --- 置換欄 + すべて置換 / 元に戻す（#2045） ---
        let preview = self
            .workspace_search
            .shown()
            .and_then(|shown| {
                self.workspace_search
                    .replace()
                    .preview()
                    .filter(|p| p.seq == shown.seq)
            })
            .filter(|p| p.replacement == self.search_ui.replace_input.text())
            .cloned();
        if let Some(replace_field) = replace_field {
            let ready = preview
                .as_ref()
                .is_some_and(|p| p.rows.values().any(|r| matches!(r, RowPreview::Ready(_))));
            let busy = self.workspace_search.replace().busy().is_some();
            let can_undo = !self.workspace_search.replace().history().is_empty();
            root = root.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .pl(px(20.0))
                    .pr(px(8.0))
                    .pb(px(6.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_inner))
                    .child(replace_field)
                    .child(
                        self.search_button(
                            "search-replace-all",
                            crate::ui_text::panel::search_replace_all(),
                            ready && !busy,
                            &theme,
                            &probe,
                            cx,
                        )
                        .when(ready && !busy, |d| {
                            d.on_click(
                                cx.listener(|this, _, _, cx| this.apply_replace(None, None, cx)),
                            )
                        }),
                    )
                    .when(can_undo, |d| {
                        d.child(
                            self.search_button(
                                "search-replace-undo",
                                crate::ui_text::panel::search_replace_undo(),
                                !busy,
                                &theme,
                                &probe,
                                cx,
                            )
                            .when(!busy, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.undo_replace(cx)))
                            }),
                        )
                    }),
            );
            if let Some(error) = preview.as_ref().and_then(|p| p.error.as_ref()) {
                root = root.child(note(error.text_in(tako_core::i18n::lang())));
            }
            // 結果・置換欄が変わったら（打鍵・当てた・別の検索）少し待ってからプレビューを頼む
            let stale = self.workspace_search.shown().is_some_and(|shown| {
                self.search_ui.preview_requested.as_ref()
                    != Some(&(
                        shown.seq,
                        self.workspace_search.replace().generation(),
                        self.search_ui.replace_input.text().to_string(),
                    ))
            });
            if stale {
                self.schedule_replace_preview(cx);
            }
        }
        if let Some(notice) = self.search_ui.notice.clone() {
            root = root.child(note(crate::ui_text::panel::search_failed(&notice)));
        }

        // --- 状態（走っている / 失敗 / 空 / 結果） ---
        if let Some(running) = self.workspace_search.running() {
            root = root.child(note(crate::ui_text::panel::search_searching(
                &running.query.text,
            )));
        }
        if let Some((_, error)) = self.workspace_search.error() {
            let text = match error {
                QueryError::Empty => crate::ui_text::panel::search_error_empty().to_string(),
                QueryError::Newline => crate::ui_text::panel::search_error_newline().to_string(),
                QueryError::Regex(detail) => crate::ui_text::panel::search_error_regex(detail),
            };
            return root.child(note(text));
        }
        let Some(shown) = self.workspace_search.shown() else {
            if self.workspace_search.running().is_none() {
                root = root.child(note(crate::ui_text::panel::search_empty().into()));
            }
            return root;
        };
        let outcome = &shown.outcome;
        let roots: Vec<String> = shown
            .roots
            .iter()
            .map(|r| r.display().to_string())
            .collect();
        root = root.child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .px(px(10.0))
                .py(px(6.0))
                .border_b_1()
                .border_color(hsla(theme.border_inner))
                .text_size(px(11.0))
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(SharedString::from(crate::ui_text::panel::search_count(
                            outcome.hits.len(),
                            outcome.files_matched,
                        ))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(crate::ui_text::panel::search_scope(
                            &roots,
                        ))),
                )
                .child(
                    div()
                        .id("panel-search-close")
                        .relative()
                        .flex_none()
                        .px_1()
                        .cursor_pointer()
                        .tooltip({
                            let tip_theme = theme.clone();
                            move |_, cx| {
                                cx.new(|_| {
                                    crate::tab_bar::HintTooltip::new(
                                        crate::ui_text::panel::search_close().to_string(),
                                        tip_theme.clone(),
                                    )
                                })
                                .into()
                            }
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.clear_workspace_search(cx)))
                        .child(
                            svg()
                                .path(crate::file_icons::ui_icon::CLOSE)
                                .w(px(10.0))
                                .h(px(10.0))
                                .text_color(hsla(theme.text_muted)),
                        )
                        .child(probe("panel-search-close".to_string())),
                ),
        );
        let stopped = match outcome.stopped {
            Some(Stop::Limit) => Some(crate::ui_text::panel::search_stopped_limit(
                tako_core::workspace_search::MAX_RESULTS,
            )),
            Some(Stop::Time) => Some(crate::ui_text::panel::search_stopped_time(
                tako_core::workspace_search::DEFAULT_TIME_LIMIT.as_secs(),
            )),
            Some(Stop::Cancelled) => {
                Some(crate::ui_text::panel::search_stopped_cancelled().to_string())
            }
            None => None,
        };
        if let Some(text) = stopped {
            root = root.child(note(text));
        }
        let skipped = outcome.skipped;
        if skipped.binary + skipped.too_large + skipped.unreadable > 0 {
            root = root.child(note(crate::ui_text::panel::search_skipped(
                skipped.binary,
                skipped.too_large,
                skipped.unreadable,
            )));
        }
        if outcome.hits.is_empty() {
            return root.child(note(crate::ui_text::panel::search_none().into()));
        }

        // --- 結果の一覧（ファイルごとに見出し。行を押すと開く） ---
        let muted = |text: String| {
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        let highlight = HighlightStyle {
            color: Some(hsla(theme.foreground)),
            background_color: Some(hsla_alpha(theme.accent, 0.30)),
            ..HighlightStyle::default()
        };
        // #2045: 置換のプレビューは消す字（取り消し線）と足す字を色で分ける
        let deleted = HighlightStyle {
            color: Some(hsla(theme.text_muted)),
            background_color: Some(hsla_alpha(theme.red, 0.22)),
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(hsla(theme.red)),
            }),
            ..HighlightStyle::default()
        };
        let inserted = HighlightStyle {
            color: Some(hsla(theme.foreground)),
            background_color: Some(hsla_alpha(theme.green, 0.25)),
            ..HighlightStyle::default()
        };
        let replace_open = self.search_ui.replace_open;
        let busy = self.workspace_search.replace().busy().is_some();
        let applied = self.workspace_search.replace().applied().clone();
        let row_preview = |index: usize| -> Option<&RowPreview> {
            preview.as_ref().filter(|_| replace_open)?.rows.get(&index)
        };
        let mut body = div()
            .id("panel-search-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(px(11.5));
        let mut last_path: Option<&std::path::Path> = None;
        let mut hidden = 0;
        for (index, hit) in outcome.hits.iter().enumerate() {
            if index >= ROWS {
                hidden = outcome.hits.len() - index;
                break;
            }
            let id = index + 1;
            // 結果は（ルート, パス, 行）順に並んでいる = ファイルが変わるたびに見出し
            if last_path != Some(hit.path.as_path()) {
                last_path = Some(hit.path.as_path());
                let in_file = outcome.hits.iter().filter(|h| h.path == hit.path).count();
                // そのファイルに当てられる行があれば「ファイルを置換」（#2045）
                let file_ready = outcome
                    .hits
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.path == hit.path)
                    .any(|(i, _)| matches!(row_preview(i), Some(RowPreview::Ready(_))));
                let file_path = hit.path.display().to_string();
                body = body.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .pt(px(8.0))
                        .pb(px(3.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(hit.rel.clone())),
                        )
                        .child(muted(in_file.to_string()))
                        .when(file_ready, |d| {
                            d.child(div().flex_1()).child(
                                self.search_button(
                                    format!("search-replace-file-{id}"),
                                    crate::ui_text::panel::search_replace_file(),
                                    !busy,
                                    &theme,
                                    &probe,
                                    cx,
                                )
                                .when(!busy, |d| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_replace(None, Some(file_path.clone()), cx)
                                    }))
                                }),
                            )
                        }),
                );
            }
            // 置換のプレビュー（#2045）: 当てられる行は差分の抜粋、当てない行は理由の札
            let ready = match row_preview(index) {
                Some(RowPreview::Ready(line)) if !applied.contains(&index) => Some(line.clone()),
                _ => None,
            };
            let tag = if !replace_open {
                None
            } else if applied.contains(&index) {
                Some(crate::ui_text::panel::search_replace_done().to_string())
            } else {
                match row_preview(index) {
                    Some(RowPreview::Skipped(skip)) => {
                        Some(crate::ui_text::panel::search_replace_skip(*skip).to_string())
                    }
                    _ => None,
                }
            };
            let (text, highlights): (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) =
                match &ready {
                    Some(line) => {
                        let marks: Vec<(usize, usize)> =
                            line.diff_marks.iter().map(|(s, e, _)| (*s, *e)).collect();
                        let styles = line.diff_marks.iter().map(|(_, _, k)| match k {
                            Mark::Deleted => deleted,
                            Mark::Inserted => inserted,
                        });
                        (
                            line.diff.clone(),
                            char_ranges_to_bytes_kept(&line.diff, &marks)
                                .into_iter()
                                .zip(styles)
                                .filter_map(|(r, style)| r.map(|r| (r, style)))
                                .collect(),
                        )
                    }
                    None => (
                        hit.excerpt.clone(),
                        char_ranges_to_bytes(&hit.excerpt, &hit.ranges)
                            .into_iter()
                            .map(|r| (r, highlight))
                            .collect(),
                    ),
                };
            body = body.child(
                div()
                    .id(("search-row", id as u64))
                    .relative()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(18.0))
                    .pr(px(10.0))
                    .py(px(3.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(theme.surface_hover)))
                    .child(muted(hit.line.to_string()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .child(
                                StyledText::new(SharedString::from(text))
                                    .with_highlights(highlights),
                            ),
                    )
                    .when_some(tag, |d, tag| d.child(muted(tag)))
                    .when(ready.is_some(), |d| {
                        d.child(
                            self.search_button(
                                format!("search-replace-row-{id}"),
                                crate::ui_text::panel::search_replace_row(),
                                !busy,
                                &theme,
                                &probe,
                                cx,
                            )
                            // 行のクリック（開く）へ伝えない
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .when(!busy, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.apply_replace(Some(id), None, cx)
                                }))
                            }),
                        )
                    })
                    // 押す（⌘クリックも同じ）と開く。押した瞬間にルートの dismiss で
                    // 欄のフォーカスが落ちるのは構わない（開いたペインへ移る）
                    .on_click(cx.listener(move |this, _, _, cx| this.open_search_item(id, cx)))
                    .child(probe(format!("search-row-{id}"))),
            );
        }
        if hidden > 0 {
            body = body.child(note(crate::ui_text::panel::search_more(hidden)));
        }
        root.child(body)
    }
}

/// [`char_ranges_to_bytes`] の位置を保つ版（#2045。範囲ごとに印が対になるので、落とした範囲は `None`）
fn char_ranges_to_bytes_kept(
    text: &str,
    ranges: &[(usize, usize)],
) -> Vec<Option<std::ops::Range<usize>>> {
    ranges
        .iter()
        .map(|r| char_ranges_to_bytes(text, std::slice::from_ref(r)).pop())
        .collect()
}

/// 抜粋の中の文字範囲をバイト範囲へ直す（`StyledText` の強調はバイトで受ける）。
/// 範囲外・空の範囲は落とす
pub(crate) fn char_ranges_to_bytes(
    text: &str,
    ranges: &[(usize, usize)],
) -> Vec<std::ops::Range<usize>> {
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    ranges
        .iter()
        .filter_map(|&(s, e)| {
            let start = *offsets.get(s)?;
            let end = *offsets.get(e)?;
            (start < end).then_some(start..end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::char_ranges_to_bytes;

    #[test]
    fn 文字範囲をバイト範囲へ直す() {
        // 「日本語」は 1 字 3 バイト
        assert_eq!(
            char_ranges_to_bytes("日本語 needle", &[(4, 10)]),
            vec![10..16]
        );
        assert_eq!(
            char_ranges_to_bytes("abc", &[(1, 2), (2, 2), (5, 9)]),
            vec![1..2]
        );
        assert_eq!(char_ranges_to_bytes("abc", &[(0, 3)]), vec![0..3]);
    }
}

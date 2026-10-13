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

use gpui::{div, prelude::*, px, svg, Context, HighlightStyle, MouseButton, MouseDownEvent};
use gpui::{SharedString, StyledText};
use tako_control::protocol::Request;
use tako_core::workspace_search::{QueryError, Stop};
use tako_core::PaneOrigin;

use super::*;

/// 描く行の上限（残りは「ほか N 件」。全件は CLI / MCP が返す）
const ROWS: usize = 500;
/// 検索語の上限（バイト。貼り付けた巨大な文字列で欄と照合器を膨らませない）
const QUERY_MAX_BYTES: usize = 1000;

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

    /// 検索欄へ文字を入れる（打鍵・⌘V・IME の確定の共通経路。1 行なので制御文字は落とす）
    pub(crate) fn search_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
        if !filtered.is_empty() {
            self.search_ui
                .input
                .insert(&filtered, QUERY_MAX_BYTES, false);
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
            "enter" => {
                self.run_workspace_search(cx);
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
                if self.search_ui.input.handle_edit_key(key) {
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
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.search_ui.notice = Some(e.to_string());
        }
        cx.notify();
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

    pub(crate) fn render_search_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.sync_search_input();
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

        // --- 検索欄 + トグル ---
        let (before, after) = self.search_ui.input.split_at_caret();
        let visible = ((self.panel_width - 110.0) / 6.5).max(6.0) as usize;
        let empty = self.search_ui.input.text().is_empty();
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
        root = root.child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .px(px(8.0))
                .py(px(6.0))
                .border_b_1()
                .border_color(hsla(theme.border_inner))
                .child(
                    div()
                        .id("search-input")
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
                        .border_color(hsla(if focused {
                            theme.accent_border_muted
                        } else {
                            theme.border_inner
                        }))
                        .bg(rgba(theme.surface_0))
                        .text_size(px(11.5))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                // #496: 押した瞬間にルートの dismiss で自分が消えないよう伝播を止める
                                this.clear_text_input_focus();
                                this.search_ui.focused = true;
                                this.search_ui.input.move_end();
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .when(empty && !focused, |d| {
                            d.child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(hsla(theme.text_faint))
                                    .child(crate::ui_text::panel::search_placeholder()),
                            )
                        })
                        .when(!empty, |d| {
                            d.child(div().flex_none().child(SharedString::from(
                                crate::right_panel::tail_chars(before, visible),
                            )))
                        })
                        // #561: 未確定文字列は入力欄の中へ出す（変換の宛先 = AppTextInput::WorkspaceSearch）
                        .children(self.text_input_marked(AppTextInput::WorkspaceSearch, &theme))
                        .when(focused, |d| {
                            d.child(self.text_input_caret(AppTextInput::WorkspaceSearch, &theme))
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
                        .child(probe("search-input".to_string())),
                )
                .child(
                    toggle(
                        "search-case",
                        "Aa",
                        self.search_ui.case_sensitive,
                        crate::ui_text::panel::search_case_toggle(),
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.toggle_workspace_search_option(false, cx)
                        }),
                    ),
                )
                .child(
                    toggle(
                        "search-regex",
                        ".*",
                        self.search_ui.regex,
                        crate::ui_text::panel::search_regex_toggle(),
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.toggle_workspace_search_option(true, cx)),
                    ),
                ),
        );
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
                        .child(muted(in_file.to_string())),
                );
            }
            let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> =
                char_ranges_to_bytes(&hit.excerpt, &hit.ranges)
                    .into_iter()
                    .map(|r| (r, highlight))
                    .collect();
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
                                StyledText::new(SharedString::from(hit.excerpt.clone()))
                                    .with_highlights(highlights),
                            ),
                    )
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

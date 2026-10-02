//! 整形の GUI（S6 / #1683）: 編集メニュー・⇧⌘I（Windows は Ctrl+Shift+I）・パレット・
//! 保存時整形（⌘S）の入口と、答えの見せ方（ヘッダの一時表示）
//!
//! ## 問い合わせと当て方はここに無い
//!
//! どの入口も CLI `tako lsp format` / `tako edit save` / MCP `tako_lsp` と**同じ 3 段**を通る:
//! `tako_control::prepare_offload`（UI スレッドで編集モードへ入り、本文と版を採る）→
//! `OffloadJob::run_staged`（background で言語サーバの起動と応答を待つ）→
//! `tako_control::finish_offload`（UI スレッドで頼んだ版へ当てる = `TextBuffer::apply_changes`。
//! 保存時整形はそのあと保存する）。ここが持つのは「どのペインの・どの範囲を頼むか」と、
//! 答え（整形した・変える所が無い・未導入…）の見せ方だけ。
//!
//! **自動保存は整形しない**: 自動保存（`run_autosave`）はここを通らない（打鍵の 500ms 後に
//! 勝手に整形されない = 既定 OFF の理由）。番犬 `issue1683_lsp_format_watchdog` が縛る。

use gpui::Context;
use serde_json::Value;
use tako_control::protocol::{LineColRange, Request};
use tako_control::OffloadOutcome;
use tako_core::{PaneId, PaneOrigin};

use crate::{preview, TakoApp};

/// 問い合わせ中の印（ペインごとに 1 つ。答えを待つあいだヘッダに出す）
#[derive(Debug, Default)]
pub(crate) struct LspFormatUi {
    pub(crate) pending: Option<PaneId>,
    seq: u64,
}

impl TakoApp {
    /// 編集メニュー / ⇧⌘I / パレット: フォーカス中のペインを整形する。
    /// `selection` なら選択範囲だけ（選択が無ければ理由を出して何もしない）
    pub(crate) fn format_focused_preview(&mut self, selection: bool, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        // ターミナルのペインでは何もしない（打鍵を奪っていても失うものは無い = keybindings）
        let Some(mode) = self.previews.get(&pane).map(|p| p.mode) else {
            return;
        };
        if mode != preview::PreviewMode::Code {
            self.show_lsp_status(
                pane,
                crate::ui_text::preview::format_not_code().to_string(),
                true,
                cx,
            );
            return;
        }
        let range = if selection {
            match self.preview_selection_range(pane) {
                Some(range) => Some(range),
                None => {
                    self.show_lsp_status(
                        pane,
                        crate::ui_text::preview::format_no_selection().to_string(),
                        true,
                        cx,
                    );
                    return;
                }
            }
        } else {
            None
        };
        let request = Request::LspFormat {
            pane: Some(pane.as_u64()),
            range,
        };
        self.format_preview_request(pane, request, cx);
    }

    /// 整形の要求を CLI / MCP と同じ 3 段へ渡す（編集メニュー・⇧⌘I と、右クリックメニューの
    /// 「コードを整形」「選択範囲を整形」= #1684 が共有する入口）
    pub(crate) fn format_preview_request(
        &mut self,
        pane: PaneId,
        request: Request,
        cx: &mut Context<Self>,
    ) {
        match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => self.run_lsp_format(pane, job, cx),
            Some(Err(e)) => self.show_lsp_status(pane, e.to_string(), true, cx),
            None => {}
        }
    }

    /// ⌘S の保存時整形（#1683）。整形を挟んだら `true`（保存は答えを当てた後に行う）。
    /// 挟まない（設定が OFF・上書き保存・競合中・リモート・受け持つサーバが無い）なら `false` で、
    /// 呼び手は従来どおり保存する = 既定 OFF の利用者の ⌘S は変更前と 1 バイトも変わらない
    pub(crate) fn save_with_format(&mut self, pane: PaneId, cx: &mut Context<Self>) -> bool {
        if !self.lsp_format_on_save {
            return false;
        }
        let request = Request::PreviewSave {
            pane: Some(pane.as_u64()),
            force: false,
        };
        match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => {
                self.run_lsp_format(pane, job, cx);
                true
            }
            // 整形の準備で断られた（編集モードへ入れない等）: 理由を出して、保存は従来どおり
            // （整形が保存を止めない）
            Some(Err(e)) => {
                self.show_lsp_status(pane, e.to_string(), true, cx);
                false
            }
            None => false,
        }
    }

    /// プレビューの選択（行・桁）を整形の範囲へ。選択が無ければ `None`。
    /// ControlHost の `preview_selection`（右クリックメニューの「選択範囲を整形」= #1684）も読む
    pub(crate) fn preview_selection_range(&self, pane: PaneId) -> Option<LineColRange> {
        let selection = self.preview_selections.get(&pane)?;
        let (start, end) = if selection.anchor <= selection.head {
            (selection.anchor, selection.head)
        } else {
            (selection.head, selection.anchor)
        };
        (start != end).then_some(LineColRange {
            start_line: start.0 + 1,
            start_col: start.1,
            end_line: end.0 + 1,
            end_col: end.1,
        })
    }

    /// 3 段の 2 段目と 3 段目（準備は呼び手が CLI / MCP と同じ `prepare_offload` で済ませた）
    fn run_lsp_format(
        &mut self,
        pane: PaneId,
        job: tako_control::OffloadJob,
        cx: &mut Context<Self>,
    ) {
        self.lsp_format.seq = self.lsp_format.seq.wrapping_add(1);
        let seq = self.lsp_format.seq;
        self.lsp_format.pending = Some(pane);
        let staged = cx
            .background_executor()
            .spawn(async move { job.run_staged() });
        cx.spawn(async move |this, cx| {
            let outcome = staged.await;
            let _ = this.update(cx, |app, cx| app.finish_lsp_format(outcome, seq, pane, cx));
        })
        .detach();
        cx.notify();
    }

    /// 答えを当てる（dispatch の `finish_offload` の 1 実装）と、答えの見せ方。
    /// 後から押した整形があっても古い答えは捨てない（版が進んでいれば `stale` で当たらない）
    fn finish_lsp_format(
        &mut self,
        outcome: OffloadOutcome,
        seq: u64,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        if seq == self.lsp_format.seq {
            self.lsp_format.pending = None;
        }
        let result = match outcome {
            OffloadOutcome::OnUi(next) => {
                // 見えている窓は notify で描かれる（強制描画は隠れた窓の IPC 向け = #1370）
                let (result, _redraw) = self.finish_offload_on_ui(next, PaneOrigin::User, cx);
                result
            }
            OffloadOutcome::Reply(result) => result,
        };
        match result {
            Ok(value) => self.present_lsp_format(pane, &value, cx),
            Err(e) => self.show_lsp_status(pane, e.to_string(), true, cx),
        }
        cx.notify();
    }

    /// 答えの見せ方: 整形した・変える所が無い → 注記 / それ以外 → 理由と次の一手（注意色）。
    /// 保存時整形（応答に `saved` がある）は整形の節（`format`）を見る
    fn present_lsp_format(&mut self, pane: PaneId, value: &Value, cx: &mut Context<Self>) {
        let format = if value.get("saved").is_some() {
            &value["format"]
        } else {
            value
        };
        let text = |key: &str| format[key].as_str().unwrap_or_default().to_string();
        match format["status"].as_str() {
            Some("formatted" | "unchanged") => {
                self.show_lsp_status(pane, text("note"), false, cx);
            }
            _ => {
                // 保存時整形は「整形せずに保存した（理由）」の注記を持つ
                let message = match format["note"].as_str() {
                    Some(note) => note.to_string(),
                    None => {
                        crate::ui_text::preview::goto_notice(&text("reason"), &text("next_step"))
                    }
                };
                self.show_lsp_status(pane, message, true, cx);
            }
        }
    }
}

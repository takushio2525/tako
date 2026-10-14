//! 名前の変更の GUI（S9 / #1686）: 右クリックメニューの「名前の変更…」で開く入力の帯と、答えの見せ方
//!
//! ## 問い合わせと当て方はここに無い
//!
//! Enter は CLI `tako lsp rename --yes` / MCP `tako_lsp` の action=rename（`confirm=true`）と**同じ要求**を、
//! 整形と同じ 3 段（`prepare_offload` → `run_staged` → `finish_offload`）へ渡す。`prepareRename` の拒否・
//! 段階 1 の判定（頼んだペインのファイル以外に及ぶなら当てない）・undo 1 回の当て方は dispatch と
//! `TextBuffer::apply_changes` が持つ。ここが持つのは欄の状態（打った名前・キャレット）と、答え
//! （書き換えた・当てなかった・断られた）の見せ方だけ。
//!
//! GUI の Enter は利用者が名前を打って確かめた操作なので当てる（`confirm=true`）。当てるのは編集バッファ
//! だけで保存はしない（undo 1 回で戻る）。CLI / MCP の既定の dry-run は、画面を見ていない AI が件数と
//! 場所を先に読むための口（#496 の git 操作と同じ作法）。
//!
//! 欄の編集操作は `TextField` の 1 実装（#1459）、IME の宛先は `AppTextInput::LspRename`
//! （#561 / #1725 の作法 = 打鍵の振り分けと変換の宛先を同じ判定 `lsp_rename_input_active` で揃える）。

use gpui::prelude::*;
use gpui::{canvas, div, px, Context, Keystroke, MouseButton, MouseDownEvent, SharedString};
use serde_json::Value;
use tako_control::lsp::menu::ItemTarget;
use tako_control::protocol::Request;
use tako_control::OffloadOutcome;
use tako_core::theme::Theme;
use tako_core::{PaneId, PaneOrigin};

use crate::{hsla, hsla_alpha, rgba_alpha, AppTextInput, TakoApp};

/// 新しい名前の上限（バイト。貼り付けた巨大な文字列で欄と要求を膨らませない）
const NAME_MAX_BYTES: usize = 256;

/// 名前の変更の欄の状態（ウィンドウに 1 つ）
#[derive(Debug, Default)]
pub(crate) struct LspRenameUi {
    pub(crate) input: Option<RenameInput>,
    /// 問い合わせの番号（閉じた / 開き直した欄へ古い答えを見せない）
    seq: u64,
}

/// 開いている欄
#[derive(Debug)]
pub(crate) struct RenameInput {
    pub(crate) pane: PaneId,
    /// 頼む位置（行 1 始まり・桁 0 始まりの行内 UTF-8 バイト = 識別子の先頭。⌘クリックと同じ）
    pub(crate) line: usize,
    pub(crate) column: usize,
    /// 開いたときの名前（見出しに出す）
    pub(crate) original: String,
    pub(crate) field: crate::text_field::TextField,
    /// 問い合わせ中（その間は打鍵を受けない = 答えと欄の中身を食い違わせない）
    pub(crate) running: bool,
    /// 断られた理由（名前の誤り・この位置は変えられない）。欄は開いたまま直して打ち直せる
    pub(crate) notice: Option<String>,
    seq: u64,
}

impl TakoApp {
    /// 右クリックメニューの「名前の変更…」（#1686）。欄を今の名前で開く（キャレットは末尾）
    pub(crate) fn open_lsp_rename(
        &mut self,
        pane: PaneId,
        target: ItemTarget,
        cx: &mut Context<Self>,
    ) {
        // 今の名前は ⌘ホバー・メニューと同じ判定（`symbol_at`）で、編集中ならその本文から採る
        let original = tako_control::PreviewHost::preview_goto_source(
            self,
            pane,
            target.line.saturating_sub(1),
        )
        .ok()
        .and_then(|source| {
            let symbol = tako_core::lsp::goto::symbol_at(&source.line_text, target.column)?;
            Some(source.line_text[symbol.range].to_string())
        })
        .unwrap_or_default();
        // 他の入力欄（git の欄・検索欄・Web の欄）は畳む（打鍵と変換の宛先を 1 つに保つ = #503）
        self.clear_text_input_focus();
        self.lsp_rename.seq = self.lsp_rename.seq.wrapping_add(1);
        let mut field = crate::text_field::TextField::default();
        field.set_text(original.clone());
        self.lsp_rename.input = Some(RenameInput {
            pane,
            line: target.line,
            column: target.column,
            original,
            field,
            running: false,
            notice: None,
            seq: self.lsp_rename.seq,
        });
        cx.notify();
    }

    /// 欄が**画面に出ていて**打鍵・変換を受けるか（頼んだペインが今のタブにあって、プレビューのまま）。
    /// 見えない欄に打鍵を吸わせない（#1725 の `inline_edit_visible` と同じ規則）
    pub(crate) fn lsp_rename_input_active(&self) -> bool {
        self.lsp_rename.input.as_ref().is_some_and(|input| {
            self.workspace.active_tab().tree().contains(input.pane)
                && self.previews.contains_key(&input.pane)
        })
    }

    /// 欄を閉じる（**閉じる出口はここ 1 本**。欄宛ての変換も捨てる = #1750 と同じ理由）
    pub(crate) fn close_lsp_rename(&mut self) {
        if self.lsp_rename.input.take().is_some() {
            self.discard_app_text_ime(AppTextInput::LspRename);
        }
    }

    /// 欄へ文字列を入れる（打鍵・⌘V・IME の確定・unmark の 4 経路の共通。**挿入はここ 1 本**）。
    /// 名前は 1 行なので制御文字は `TextField` が落とす
    pub(crate) fn lsp_rename_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(input) = self.lsp_rename.input.as_mut().filter(|i| !i.running) {
            let _fits = input.field.insert(text, NAME_MAX_BYTES, false);
            input.notice = None;
        }
        cx.notify();
    }

    /// 欄のキー処理（`handle_key` が欄の出ている間だけ呼ぶ。打鍵はすべてここで消費する）
    pub(crate) fn handle_lsp_rename_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let m = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" => self.submit_lsp_rename(cx),
            "escape" => {
                self.close_lsp_rename();
                cx.notify();
            }
            // ⌘V はクリップボード読み出しの共通経路（`paste`）へ任せる（横断検索の欄と同じ）
            "v" if m.platform => self.paste(cx),
            key => {
                let running = self.lsp_rename.input.as_ref().is_some_and(|i| i.running);
                if running {
                    return;
                }
                if let Some(input) = self.lsp_rename.input.as_mut() {
                    if input.field.handle_edit_key(key) {
                        input.notice = None;
                        cx.notify();
                        return;
                    }
                }
                // 修飾キー付きは打たない（⌘W 等は欄を出している間は効かない = 打鍵を漏らさない）
                if m.platform || m.control {
                    return;
                }
                if let Some(text) = keystroke.key_char.as_ref().filter(|t| !t.is_empty()) {
                    let text = text.clone();
                    self.lsp_rename_insert(&text, cx);
                }
            }
        }
    }

    /// Enter: CLI `tako lsp rename --yes` と同じ要求を 3 段へ渡す
    fn submit_lsp_rename(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.lsp_rename.input.as_mut().filter(|i| !i.running) else {
            return;
        };
        let request = Request::LspRename {
            pane: Some(input.pane.as_u64()),
            line: input.line,
            column: input.column,
            new_name: input.field.text().trim().to_string(),
            confirm: true,
        };
        let (pane, seq) = (input.pane, input.seq);
        input.running = true;
        input.notice = None;
        match tako_control::prepare_offload(self, &request) {
            Some(Ok(job)) => {
                let staged = cx
                    .background_executor()
                    .spawn(async move { job.run_staged() });
                cx.spawn(async move |this, cx| {
                    let outcome = staged.await;
                    let _ =
                        this.update(cx, |app, cx| app.finish_lsp_rename(outcome, seq, pane, cx));
                })
                .detach();
            }
            // 準備で断られた（名前が空・位置の誤り）: 欄に理由を出して打ち直させる
            Some(Err(e)) => self.lsp_rename_notice(seq, e.to_string()),
            None => self.lsp_rename_notice(seq, String::new()),
        }
        cx.notify();
    }

    /// 欄に理由を出して打ち直せる状態へ戻す（同じ欄がまだ開いているときだけ）
    fn lsp_rename_notice(&mut self, seq: u64, notice: String) {
        if let Some(input) = self.lsp_rename.input.as_mut().filter(|i| i.seq == seq) {
            input.running = false;
            input.notice = (!notice.is_empty()).then_some(notice);
        }
    }

    /// 答えを返す / 当てる（dispatch の `finish_offload` の 1 実装）と、答えの見せ方
    fn finish_lsp_rename(
        &mut self,
        outcome: OffloadOutcome,
        seq: u64,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        let result = match outcome {
            OffloadOutcome::OnUi(next) => {
                let (result, _redraw) = self.finish_offload_on_ui(next, PaneOrigin::User, cx);
                result
            }
            OffloadOutcome::Reply(result) => result,
        };
        match result {
            Ok(value) => self.present_lsp_rename(seq, pane, &value, cx),
            Err(e) => self.lsp_rename_notice(seq, e.to_string()),
        }
        cx.notify();
    }

    /// 答えの見せ方: 書き換えた・書き換える所が無い → 欄を閉じて注記 / 名前の誤り・変えられない位置 →
    /// 欄を開いたまま理由（打ち直せる）/ それ以外（段階 1 で当てなかった・未導入・未応答…）→ 欄を閉じて
    /// ヘッダに理由と次の一手（注意色）
    fn present_lsp_rename(
        &mut self,
        seq: u64,
        pane: PaneId,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        let notice = crate::ui_text::preview::goto_notice(&text("reason"), &text("next_step"));
        match value["status"].as_str() {
            Some("rejected" | "refused") => self.lsp_rename_notice(seq, notice),
            Some(status) => {
                if self
                    .lsp_rename
                    .input
                    .as_ref()
                    .is_some_and(|input| input.seq == seq)
                {
                    self.close_lsp_rename();
                }
                match status {
                    "renamed" | "unchanged" => self.show_lsp_status(pane, text("note"), false, cx),
                    // 段階 1 で当てなかった: 件数（note）と理由を並べる
                    "not-applied" => self.show_lsp_status(
                        pane,
                        crate::ui_text::preview::goto_notice(&text("note"), &notice),
                        true,
                        cx,
                    ),
                    _ => self.show_lsp_status(pane, notice, true, cx),
                }
            }
            None => self.lsp_rename_notice(seq, notice),
        }
    }

    /// 欄の帯（タイトルの下。言語サーバの状態の帯・外部変更の競合の帯と同じ置き方）。
    /// 欄が出ていないペインでは `None`
    pub(crate) fn render_lsp_rename_bar(
        &self,
        pane_id: PaneId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let input = self
            .lsp_rename
            .input
            .as_ref()
            .filter(|input| input.pane == pane_id)?;
        let key = pane_id.as_u64();
        let accent = theme.accent;
        let (before, after) = input.field.split_at_caret();
        let probe = self.panel_click_probe_bounds.clone();
        let probe_key = format!("preview-rename-input-{key}");
        let field = div()
            .id(("preview-rename-input", key))
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
            .border_color(hsla(theme.accent_border_muted))
            .bg(crate::rgba(theme.surface_0))
            .font_family(SharedString::from(theme.font_family.clone()))
            .text_size(px(12.0))
            .text_color(hsla(theme.foreground))
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(before.to_string())),
            )
            // #561: 未確定文字列は欄の中へ出す（変換の宛先 = AppTextInput::LspRename）
            .children(self.text_input_marked_at(AppTextInput::LspRename, theme, 12.0))
            .when(!input.running, |d| {
                d.child(self.text_input_caret(AppTextInput::LspRename, theme))
            })
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(after.to_string())),
            )
            // 実矩形は visual-test が実マウスで押す・写すために記録する（帯のボタンと同じ置き方）
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, _| {
                        probe.borrow_mut().insert(probe_key.clone(), bounds);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        let status = if input.running {
            Some((crate::ui_text::preview::rename_running().to_string(), false))
        } else {
            input.notice.clone().map(|notice| (notice, true))
        };
        Some(
            div()
                .id(("preview-rename-bar", key))
                .flex_none()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(11.0))
                .bg(rgba_alpha(accent, 0.08))
                .border_b_1()
                .border_color(hsla_alpha(accent, 0.45))
                // ルートの一括 dismiss（#503）から欄を守る（押した瞬間に欄を閉じない）
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .text_color(hsla(theme.foreground))
                        .child(SharedString::from(crate::ui_text::preview::rename_title(
                            &input.original,
                        ))),
                )
                .child(field)
                .child(
                    div()
                        .text_color(hsla_alpha(theme.foreground, 0.6))
                        .child(crate::ui_text::preview::rename_hint()),
                )
                .children(status.map(|(line, warn)| {
                    div()
                        .text_color(if warn {
                            hsla(theme.yellow)
                        } else {
                            hsla_alpha(theme.foreground, 0.7)
                        })
                        .child(SharedString::from(line))
                }))
                .into_any_element(),
        )
    }
}

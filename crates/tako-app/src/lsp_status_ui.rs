//! エディタに言語サーバの状態を出す（#1944）: タイトルの 1 行（取得中 / 起動中 / 読み込み中 /
//! 動作中）と、未導入・取得の失敗・止まった・起こす前提が未設定（#1951。Arduino の基板）のときの帯
//! （理由 + 次の一手 + 押せば入る / 確かめ直すボタン）
//!
//! 「インテリジェンス機能が黙って出ない」を無くすのが目的。状態の正本は manager
//! （`LspManager::document_server`。`tako lsp status` / MCP `tako_lsp_server` と同じ判定と同じ文）で、
//! ここは見せ方と、ボタンから manager の同じ口（`install` / `restart`）を背景で呼ぶことだけを持つ。
//! 描き直しのきっかけは manager の状態の知らせ（`LspManager::state_events`。診断の知らせとは別の口）。
//!
//! A/B は `TAKO_1944_LEGACY=1`（取得も状態の表示も出さない = #1678 の「案内だけ」）。
//!
//! **帯はコードを全部隠さない**（#2751）: 帯の文（見出し・理由・次の一手）の器は高さに上限
//! （[`bar_text_max_h`] = ペインの高さの 1/3 まで・本文がペインの高さの半分を下回らない範囲）を持ち、
//! 超えた分は器の中でスクロールして読む（右端のつまみ = はみ出しているときだけ）。ボタンの行は器の外 =
//! 常に見えて押せる。狭いペイン（幅 240px 前後）で長いパスの理由が折り返しても、本文の行は押し出されない。
//! 広いペインの帯は上限に届かないので見た目は変わらない。A/B は `TAKO_2751_LEGACY=1`（上限なし）。

use std::hash::{Hash, Hasher};

use gpui::prelude::*;
use gpui::{canvas, div, point, px, Context, MouseButton, MouseDownEvent, SharedString};
use tako_control::lsp::DocumentServer;
use tako_core::lsp::state::ServerState;
use tako_core::theme::Theme;
use tako_core::PaneId;

use crate::{hsla, hsla_alpha, rgba_alpha, TakoApp};

/// 帯の字の大きさ（#1944）
const BAR_TEXT_PX: f32 = 11.0;
/// 帯の 1 行の高さ（#2751）。GPUI の既定（字の大きさ × 黄金比 = `phi()`）と同じ値。上限の計算にだけ
/// 使い、描画の行の高さは変えない（広いペインの帯の見た目を #2751 の前と同じに保つ）
pub(crate) const BAR_LINE_PX: f32 = BAR_TEXT_PX * 1.618_034;
/// 帯の文の器がペインの高さに占めてよい割合の上限（#2751）。高いペインで帯が本文の半分近くまで
/// 伸びないための上限で、既定の窓（960×600 = ペインの高さ約 415px）では本文の半分の側が先に効く
pub(crate) const BAR_TEXT_MAX_SHARE: f32 = 0.33;
/// 帯の文の器の外側の高さ（#2751）: 文の上の余白 4 + ボタンの行（上下の余白 4 × 2 + ボタンの上下の
/// 余白 1 × 2 と枠 1 × 2 + 1 行）+ 下の枠線 1
pub(crate) const BAR_CHROME_PX: f32 = 4.0 + 4.0 * 2.0 + (1.0 + 1.0) * 2.0 + BAR_LINE_PX + 1.0;
/// ペインの中で帯と本文の行の外に要る高さ（#2751）: タイトルバー + 上下の枠 + 本文の上下の余白
const PANE_RESERVED_PX: f32 = crate::PANE_TITLE_BAR
    + crate::PANE_BORDER * 2.0
    + crate::preview_render::PREVIEW_BODY_PADDING * 2.0;
/// スクロールのつまみの幅と最小の高さ（#2751）
const BAR_THUMB_W: f32 = 3.0;
const BAR_THUMB_MIN_H: f32 = 12.0;

/// 描画に渡るペインの矩形の高さ（`render_pane` の `area` = `pane_text_area_rect` のテキスト領域 =
/// 枠 + 余白とタイトルバーを除いた矩形）から、ペイン全体の高さを戻す（#2751）。プレビューのペインは
/// タイトルバーの下にバナーを積まない（`stacked_top` は `PANE_TITLE_BAR` だけ）
pub(crate) fn pane_height_of_area(area_h: f32) -> f32 {
    area_h + (crate::PANE_BORDER + crate::PANE_PADDING) * 2.0 + crate::PANE_TITLE_BAR
}

/// 帯の文の器の高さの上限（#2751）。ペインの高さ `pane_h` の [`BAR_TEXT_MAX_SHARE`] までで、かつ本文の
/// 器がペインの高さの半分を下回らない範囲。どれだけ低いペインでも見出しの 1 行（[`BAR_LINE_PX`]）は残す
/// （= 本文の半分を守れるのはペインの高さが約 230px 以上のとき）
pub(crate) fn bar_text_max_h(pane_h: f32) -> f32 {
    let share = pane_h * BAR_TEXT_MAX_SHARE;
    let half = pane_h * 0.5 - BAR_CHROME_PX - PANE_RESERVED_PX;
    share.min(half).max(BAR_LINE_PX)
}

/// 帯の文の器のスクロールのつまみ（上端・高さ。#2751）。`viewport_h` = 器の高さ・`max_offset` =
/// はみ出している高さ・`offset` = GPUI のスクロール量（下へ送るほど負）。はみ出していなければ `None`
pub(crate) fn bar_thumb(viewport_h: f32, max_offset: f32, offset: f32) -> Option<(f32, f32)> {
    if viewport_h <= 0.0 || max_offset < 0.5 {
        return None;
    }
    let h = (viewport_h * viewport_h / (viewport_h + max_offset))
        .max(BAR_THUMB_MIN_H)
        .min(viewport_h);
    let ratio = (-offset / max_offset).clamp(0.0, 1.0);
    Some(((viewport_h - h) * ratio, h))
}

/// 帯の文の器のスクロール（#2751。`TakoApp::lsp_bar_scrolls` の 1 ペインぶん）。帯の文の指紋を覚え、
/// 文が変わった（未導入 → 取得の失敗 等）ら頭から見せる
#[derive(Default)]
pub(crate) struct BarScroll {
    digest: u64,
    handle: gpui::ScrollHandle,
}

impl BarScroll {
    /// visual-test が器のはみ出しとスクロール量を読む口
    #[cfg(feature = "visual-test")]
    pub(crate) fn handle_for_test(&self) -> gpui::ScrollHandle {
        self.handle.clone()
    }
}

/// #2751 の A/B（0 = env 未読 / 1 = 新 / 2 = 旧）。`TAKO_2751_LEGACY=1` で**同一バイナリのまま**旧の形
/// （帯の文の器に上限なし = 狭いペインでは帯が本文の行を全部押し出す）へ戻す
static LEGACY_2751: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(crate) fn legacy_2751() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    match LEGACY_2751.load(Relaxed) {
        0 => {
            let on = std::env::var("TAKO_2751_LEGACY").as_deref() == Ok("1");
            LEGACY_2751.store(u8::from(on) + 1, Relaxed);
            on
        }
        v => v == 2,
    }
}

/// #2751 の A/B を実行中に倒す（visual-test 専用 = 同じ場面・同じ文で新旧の画素を比べる）。
/// 倒した直後のフレームから効く
#[cfg(feature = "visual-test")]
pub(crate) fn set_legacy_2751(on: bool) {
    LEGACY_2751.store(u8::from(on) + 1, std::sync::atomic::Ordering::Relaxed);
}

/// タイトルの 1 行の色合い
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadgeTone {
    /// 取得中・起動中・読み込み中（進んでいる）
    Busy,
    /// 動作中（控えめ）
    Quiet,
}

impl TakoApp {
    /// 編集中のペインの言語サーバの状態（無ければ `None` = 受け持つサーバが無い・LSP を止めている）
    pub(crate) fn lsp_document_server(&self, pane_id: PaneId) -> Option<DocumentServer> {
        if tako_control::lsp::fetch::legacy() {
            return None;
        }
        let edit = self.preview_edits.get(&pane_id)?;
        if !edit.editing {
            return None;
        }
        self.lsp.document_server(edit.buffer.path())
    }

    /// タイトルの 1 行（帯を出す状態では出さない）
    pub(crate) fn lsp_status_badge(&self, pane_id: PaneId) -> Option<(String, BadgeTone)> {
        let server = self.lsp_document_server(pane_id)?;
        badge_of(&server)
    }

    /// 未導入・取得の失敗・止まったときの帯（理由 + 次の一手 + ボタン）。`area_h` = 描画に渡るペインの矩形
    /// （テキスト領域）の高さ（#2751。ペイン全体の高さへ戻して帯の文の器の高さの上限を決める）
    pub(crate) fn render_lsp_status_bar(
        &self,
        pane_id: PaneId,
        area_h: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let server = self.lsp_document_server(pane_id)?;
        if badge_of(&server).is_some() {
            return None;
        }
        let (title, action) = match server.state {
            ServerState::NotInstalled if server.fetch_failed => (
                crate::ui_text::preview::lsp_fetch_failed(server.id),
                server.can_install.then_some(BarAction::Install(true)),
            ),
            ServerState::NotInstalled => (
                crate::ui_text::preview::lsp_not_installed(server.id),
                server.can_install.then_some(BarAction::Install(false)),
            ),
            ServerState::GaveUp | ServerState::Stopped => (
                crate::ui_text::preview::lsp_unavailable(server.id),
                Some(BarAction::Restart),
            ),
            // #1951: 理由の行に満たすコマンド（`arduino-cli board attach -b <基板> …`）。ボタンは確かめ直し
            ServerState::Unconfigured => (
                crate::ui_text::preview::lsp_unconfigured(server.id),
                Some(BarAction::Recheck),
            ),
            _ => return None,
        };
        let key = pane_id.as_u64();
        let id = server.id;
        let detail: Vec<String> = [server.reason.clone(), server.next_step.clone()]
            .into_iter()
            .flatten()
            .collect();
        let accent = theme.yellow;
        let button = action.map(|action| {
            let probe = self.panel_click_probe_bounds.clone();
            let probe_key = format!("preview-lsp-action-{key}");
            let label = match action {
                BarAction::Install(false) => crate::ui_text::preview::lsp_install(),
                BarAction::Install(true) => crate::ui_text::preview::lsp_retry(),
                BarAction::Restart => crate::ui_text::preview::lsp_restart(),
                BarAction::Recheck => crate::ui_text::preview::lsp_recheck(),
            };
            div()
                .id(("preview-lsp-action", key))
                .relative()
                .flex_none()
                .px(px(6.0))
                .py(px(1.0))
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(hsla_alpha(accent, 0.5))
                .text_color(hsla(theme.foreground))
                .hover(|d| d.bg(rgba_alpha(accent, 0.2)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.lsp_status_action(id, action, cx);
                }))
                .child(label)
                // 実矩形は visual-test が実マウスで押すために記録する（競合の帯と同じ置き方）
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
                )
        });
        // #2751: 帯の文の器の高さに上限を置き、超えた分は器の中でスクロールする（旧の形は上限なし）
        let scroll = (!legacy_2751()).then(|| self.lsp_bar_scroll(pane_id, &title, &detail));
        let pane_h = pane_height_of_area(area_h);
        let title_el = div()
            .text_color(hsla(theme.foreground))
            .child(SharedString::from(title));
        let last = detail.len();
        let detail_els = detail.into_iter().enumerate().map(|(i, line)| {
            div()
                .text_color(hsla_alpha(theme.foreground, 0.7))
                .child(SharedString::from(line))
                // 最後の行の実矩形（visual-test が「スクロールすれば末尾まで読める」を測る）
                .when(i + 1 == last, |d| {
                    d.relative().child(crate::tab_shape::probe_canvas(
                        self.panel_click_probe_bounds.clone(),
                        format!("preview-lsp-last-{key}"),
                    ))
                })
        });
        let text = match scroll {
            // 旧の形（#2751 の前と同じ要素の並び）
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(title_el)
                .children(detail_els),
            Some(scroll) => {
                let viewport = scroll.bounds();
                let thumb = bar_thumb(
                    f32::from(viewport.size.height),
                    f32::from(scroll.max_offset().y),
                    f32::from(scroll.offset().y),
                );
                // つまみの有無は前のフレームの寸法で決まる。描いた後の寸法と食い違えば（帯が出た直後・
                // ペインの寸法が変わった）もう 1 フレーム描き直す（食い違いが無ければ何もしない）
                let drawn = thumb.is_some();
                let watch = scroll.clone();
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("preview-lsp-text", key))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .max_h(px(bar_text_max_h(pane_h)))
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .child(title_el)
                            .children(detail_els),
                    )
                    // はみ出しているときだけ右の余白につまみ（GPUI の矩形。位置を示すだけで押せない）
                    .children(thumb.map(|(top, h)| {
                        div()
                            .absolute()
                            .top(px(top))
                            .right(px(-(BAR_THUMB_W + 2.0)))
                            .w(px(BAR_THUMB_W))
                            .h(px(h))
                            .rounded(px(BAR_THUMB_W / 2.0))
                            .bg(hsla_alpha(theme.foreground, 0.35))
                            .child(crate::tab_shape::probe_canvas(
                                self.panel_click_probe_bounds.clone(),
                                format!("preview-lsp-thumb-{key}"),
                            ))
                    }))
                    // 描いた後のはみ出しを確かめ、つまみの有無と食い違えば次のフレームを頼む
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                if (f32::from(watch.max_offset().y) >= 0.5) != drawn {
                                    window.request_animation_frame();
                                }
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    // 文の器の実矩形（スクロールの器の外で採る = 器の中身の高さに数えない）
                    .child(crate::tab_shape::probe_canvas(
                        self.panel_click_probe_bounds.clone(),
                        format!("preview-lsp-text-{key}"),
                    ))
            }
        };
        let notice = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(6.0))
            .px(px(8.0))
            .pt(px(4.0))
            // 印: 縦棒（GPUI の矩形。絵文字・記号の字形に頼らない）
            .child(
                div()
                    .flex_none()
                    .mt(px(1.0))
                    .w(px(3.0))
                    .h(px(14.0))
                    .rounded(px(1.5))
                    .bg(hsla(accent)),
            )
            .child(text);
        let row = div()
            .flex()
            .flex_row()
            .justify_end()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .children(button);
        Some(
            div()
                .id(("preview-lsp-status", key))
                .relative()
                .flex_none()
                .w_full()
                .flex()
                .flex_col()
                .text_size(px(BAR_TEXT_PX))
                .bg(rgba_alpha(accent, 0.10))
                .border_b_1()
                .border_color(hsla_alpha(accent, 0.45))
                .child(notice)
                .child(row)
                // 帯の実矩形（visual-test が本文の行の押し出しを測る）
                .child(crate::tab_shape::probe_canvas(
                    self.panel_click_probe_bounds.clone(),
                    format!("preview-lsp-band-{key}"),
                ))
                .into_any_element(),
        )
    }

    /// 帯の文の器のスクロール（#2751）。帯の文が前に描いたときと違えば頭へ戻す
    fn lsp_bar_scroll(
        &self,
        pane_id: PaneId,
        title: &str,
        detail: &[String],
    ) -> gpui::ScrollHandle {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        title.hash(&mut hasher);
        detail.hash(&mut hasher);
        let digest = hasher.finish();
        let mut scrolls = self.lsp_bar_scrolls.borrow_mut();
        let entry = scrolls.entry(pane_id).or_default();
        if entry.digest != digest {
            entry.digest = digest;
            entry.handle.set_offset(point(px(0.0), px(0.0)));
        }
        entry.handle.clone()
    }

    /// 帯のボタン: manager の `install` / `restart` を背景で呼ぶ（CLI `tako lsp install` / MCP
    /// `tako_lsp_server` と同じ口。取っているあいだの様子は manager の知らせで描き直る）
    fn lsp_status_action(&mut self, id: &'static str, action: BarAction, cx: &mut Context<Self>) {
        let manager = self.lsp.clone();
        cx.background_executor()
            .spawn(async move {
                match action {
                    BarAction::Install(_) => {
                        manager.install(Some(id));
                    }
                    // 確かめ直しも restart の口（前提を満たしていれば起き、まだなら同じ帯に戻る）
                    BarAction::Restart | BarAction::Recheck => {
                        manager.restart(Some(id));
                    }
                }
            })
            .detach();
        cx.notify();
    }

    /// manager が状態の変化を知らせた（`LspManager::state_events` + `take_refresh_wanted`）: 編集セッションを
    /// 同期し直して（断っていた文書が使えるようになっていれば開く）描き直す
    pub(crate) fn refresh_lsp_links(&mut self, cx: &mut Context<Self>) {
        let panes: Vec<PaneId> = self
            .preview_edits
            .iter()
            .filter(|(_, edit)| edit.editing)
            .map(|(pane, _)| *pane)
            .collect();
        for pane in panes {
            self.sync_preview_lsp(pane);
        }
        cx.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarAction {
    /// 取って入れる（`true` = 失敗のあとのもう一度）
    Install(bool),
    Restart,
    /// 起こす前提を満たしたかを確かめ直す（#1951）
    Recheck,
}

/// タイトルの 1 行の中身（帯を出す状態は `None`）
fn badge_of(server: &DocumentServer) -> Option<(String, BadgeTone)> {
    // 取っている最中なら状態に依らず進捗（`tako lsp install` が取っているあいだは未導入のまま）。
    // 取得物が決まる前（錠を取った直後）は「取得中」だけ
    if let Some(progress) = &server.fetch {
        let mb = |n: u64| n as f64 / 1_000_000.0;
        let detail = (!progress.item.is_empty() && progress.total > 0).then(|| {
            (
                progress.item.as_str(),
                mb(progress.done),
                mb(progress.total.max(progress.done)),
            )
        });
        return Some((
            crate::ui_text::preview::lsp_fetching(server.id, detail),
            BadgeTone::Busy,
        ));
    }
    Some(match server.state {
        ServerState::Fetching => (
            crate::ui_text::preview::lsp_fetching(server.id, None),
            BadgeTone::Busy,
        ),
        ServerState::Starting | ServerState::Crashed | ServerState::NotStarted => (
            crate::ui_text::preview::lsp_starting(server.id),
            BadgeTone::Busy,
        ),
        ServerState::Running if server.loading => (
            crate::ui_text::preview::lsp_loading(server.id),
            BadgeTone::Busy,
        ),
        ServerState::Running | ServerState::Stopping => (
            crate::ui_text::preview::lsp_running(server.id),
            BadgeTone::Quiet,
        ),
        // #2048: 隔離起動で自動では起こさなかった = 帯（本文の高さが変わる）ではなく控えめな 1 行。
        // 検証の GUI の画面を、言語サーバの有無で変えない
        ServerState::Stopped if server.held => (
            crate::ui_text::preview::lsp_held(server.id),
            BadgeTone::Quiet,
        ),
        ServerState::NotInstalled
        | ServerState::GaveUp
        | ServerState::Stopped
        | ServerState::Unconfigured => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(state: ServerState) -> DocumentServer {
        DocumentServer {
            id: "srv",
            state,
            loading: false,
            fetch: None,
            reason: None,
            next_step: None,
            can_install: true,
            fetch_failed: false,
            held: false,
        }
    }

    #[test]
    fn 帯を出す状態ではタイトルの行を出さない() {
        for state in ServerState::ALL {
            let shown = badge_of(&server(state)).is_some();
            let bar = matches!(
                state,
                ServerState::NotInstalled
                    | ServerState::GaveUp
                    | ServerState::Stopped
                    | ServerState::Unconfigured
            );
            assert_eq!(shown, !bar, "{state:?}");
        }
    }

    /// #2048: 隔離起動で起こさなかった器は帯を出さず（本文の高さを変えない）、控えめな 1 行だけ
    #[test]
    fn 隔離で起こさなかった器は帯でなく控えめな_1_行() {
        let mut s = server(ServerState::Stopped);
        s.held = true;
        let (text, tone) = badge_of(&s).expect("1 行を出す");
        assert_eq!(tone, BadgeTone::Quiet);
        assert!(text.starts_with("srv "), "{text}");
        // 利用者の stop で止めた器は従来どおり帯（1 行は出さない）
        assert!(badge_of(&server(ServerState::Stopped)).is_none());
    }

    #[test]
    fn 取っている最中は未導入でも進捗を出す() {
        let mut s = server(ServerState::NotInstalled);
        s.fetch = Some(tako_control::lsp::fetch::ProgressSnapshot {
            item: "x 1".into(),
            phase: tako_control::lsp::fetch::Phase::Downloading,
            step: 1,
            steps: 1,
            done: 2_000_000,
            total: 4_000_000,
            elapsed: std::time::Duration::ZERO,
        });
        let (text, tone) = badge_of(&s).expect("進捗を出す");
        assert_eq!(tone, BadgeTone::Busy);
        assert!(text.contains("2.0"), "{text}");
        assert!(text.contains("4.0"), "{text}");
        // 取得物が決まる前（錠を取った直後）は数字を出さない（0.0 / 0.0 MB と見せない）
        if let Some(p) = s.fetch.as_mut() {
            p.item.clear();
            p.total = 0;
        }
        let (text, _) = badge_of(&s).expect("取得中だけ");
        assert!(!text.contains("0.0"), "{text}");
    }

    /// #2751: 帯の文の器の上限は、ペインの高さが約 230px 以上なら本文の器をペインの高さの半分以上残す
    #[test]
    fn 帯の文の器は本文をペインの高さの半分以上残す() {
        for h in (230..=2400).step_by(2) {
            let h = h as f32;
            let band = bar_text_max_h(h) + BAR_CHROME_PX;
            let body = h - PANE_RESERVED_PX - band;
            assert!(body >= h * 0.5 - 0.01, "pane_h={h} band={band} body={body}");
            // 高いペインでも帯はペインの高さの割合で止まる
            assert!(
                bar_text_max_h(h) <= h * BAR_TEXT_MAX_SHARE + 0.01,
                "pane_h={h}"
            );
        }
        // 低すぎるペインでも見出しの 1 行は残す
        assert_eq!(bar_text_max_h(100.0), BAR_LINE_PX);
        // 既定の窓（960×600）のペイン（415px）は本文の半分の側が効いて約 110px。広いペインの帯
        // （見出し 1 行 + 理由 1〜2 行 + 次の一手 2 行 ≈ 94px = 実測）は届かない = 見た目は変わらない
        assert!((bar_text_max_h(415.0) - (207.5 - BAR_CHROME_PX - PANE_RESERVED_PX)).abs() < 0.01);
        assert!(bar_text_max_h(415.0) > 94.0);
    }

    /// #2751: 描画に渡る矩形（テキスト領域）の高さからペイン全体の高さへ戻す（実測 361 → 415px）
    #[test]
    fn 描画に渡る矩形からペインの高さを戻す() {
        assert!((pane_height_of_area(361.0) - 415.0).abs() < 0.01);
    }

    /// #2751: スクロールのつまみは、はみ出しているときだけ・器の中の位置を示す
    #[test]
    fn つまみははみ出しているときだけ位置を示す() {
        assert_eq!(bar_thumb(100.0, 0.0, 0.0), None);
        assert_eq!(bar_thumb(100.0, 0.4, 0.0), None);
        assert_eq!(bar_thumb(0.0, 50.0, 0.0), None);
        let (top, h) = bar_thumb(100.0, 100.0, 0.0).expect("はみ出している");
        assert_eq!(top, 0.0);
        assert!((h - 50.0).abs() < 0.01, "{h}");
        // 末尾まで送ると下端に着く（行き過ぎた量は端で止める）
        for offset in [-100.0, -500.0] {
            let (top, h) = bar_thumb(100.0, 100.0, offset).expect("はみ出している");
            assert!((top + h - 100.0).abs() < 0.01, "{offset}: {top} + {h}");
        }
        // とても長い中身でも最小の高さは残す
        let (_, h) = bar_thumb(100.0, 100_000.0, 0.0).expect("はみ出している");
        assert_eq!(h, BAR_THUMB_MIN_H);
    }

    #[test]
    fn 読み込み中と動作中を分ける() {
        let mut s = server(ServerState::Running);
        assert_eq!(badge_of(&s).unwrap().1, BadgeTone::Quiet);
        s.loading = true;
        assert_eq!(badge_of(&s).unwrap().1, BadgeTone::Busy);
    }
}

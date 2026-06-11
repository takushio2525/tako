//! tako-app — GPUI バイナリ（UI 層）
//!
//! Phase 1 後半: 複数ペイン同時描画（PaneTree::layout → GPUI 要素変換）、タブバー、
//! iTerm2 踏襲のキーバインド、色 / カーソル / スクロールバック / 選択コピペ / PTY リサイズ追従。
//!
//! GPUI への依存はこのクレートだけに閉じ込める（`.agent/architecture.md`）。
//! ドメインロジック（Workspace / PaneTree / TerminalSession / Theme / Screen）はすべて
//! tako-core 側にあり、UI 層はドメイン API を呼んで結果を描画するだけにする（設計原則 5:
//! 同じ API を将来 MCP / CLI からも呼ぶため、UI に閉じたロジックを作らない）。
//!
//! `TAKO_SELF_TEST=1` で起動すると、キーディスパッチ経由で入力・分割・タブ・色・
//! スクロールバック・コピペの経路を機械検証して終了する。

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    actions, div, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    FocusHandle, Font, FontStyle, FontWeight, HighlightStyle, Hsla, KeyBinding, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba,
    ScrollDelta, ScrollWheelEvent, SharedString, Size, StrikethroughStyle, StyledText, TextStyle,
    UnderlineStyle, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use tako_core::{
    Pane, PaneId, PaneOrigin, Rect, SelectionKind, SessionNotice, SplitAxis, SplitDirection, TabId,
    TerminalSession, Theme, Workspace,
};

/// 新規セッションの初期グリッド。最初の render で実寸へリサイズされる
const INITIAL_COLS: usize = 80;
const INITIAL_ROWS: usize = 24;

/// タブバーの高さ（px）
const TAB_BAR_HEIGHT: f32 = 32.0;
/// ペイン枠線の太さ（px）
const PANE_BORDER: f32 = 1.0;
/// ペイン内側の余白（px）
const PANE_PADDING: f32 = 4.0;
/// キーボードリサイズ 1 回あたりの比率変化
const RESIZE_STEP: f32 = 0.05;

actions!(
    tako,
    [
        SplitRight,
        SplitDown,
        ClosePane,
        NewTab,
        NextTab,
        PrevTab,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        WidenPane,
        NarrowPane,
        TallenPane,
        ShortenPane,
        CopySelection,
        PasteClipboard,
        Quit,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9
    ]
);

/// iTerm2 の操作感を踏襲したキーバインド
fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-w", ClosePane, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("cmd-alt-left", FocusLeft, None),
        KeyBinding::new("cmd-alt-right", FocusRight, None),
        KeyBinding::new("cmd-alt-up", FocusUp, None),
        KeyBinding::new("cmd-alt-down", FocusDown, None),
        KeyBinding::new("ctrl-cmd-right", WidenPane, None),
        KeyBinding::new("ctrl-cmd-left", NarrowPane, None),
        KeyBinding::new("ctrl-cmd-down", TallenPane, None),
        KeyBinding::new("ctrl-cmd-up", ShortenPane, None),
        KeyBinding::new("cmd-c", CopySelection, None),
        KeyBinding::new("cmd-v", PasteClipboard, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-1", ActivateTab1, None),
        KeyBinding::new("cmd-2", ActivateTab2, None),
        KeyBinding::new("cmd-3", ActivateTab3, None),
        KeyBinding::new("cmd-4", ActivateTab4, None),
        KeyBinding::new("cmd-5", ActivateTab5, None),
        KeyBinding::new("cmd-6", ActivateTab6, None),
        KeyBinding::new("cmd-7", ActivateTab7, None),
        KeyBinding::new("cmd-8", ActivateTab8, None),
        KeyBinding::new("cmd-9", ActivateTab9, None),
    ]
}

fn rgba(c: tako_core::Rgb) -> Rgba {
    Rgba {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
}

fn hsla(c: tako_core::Rgb) -> Hsla {
    rgba(c).into()
}

struct TakoApp {
    workspace: Workspace,
    terminals: HashMap<PaneId, TerminalSession>,
    theme: Theme,
    focus_handle: FocusHandle,
    /// 実測したセル寸法（最初の render で確定）
    cell_size: Option<Size<Pixels>>,
    /// マウス選択中のペイン
    selecting: Option<PaneId>,
    /// 直近 render でのアクティブタブ各ペインのテキスト領域（マウス座標→セル変換用）
    pane_text_areas: Vec<(PaneId, Bounds<Pixels>)>,
}

impl TakoApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            // ルートペインは下の spawn_pane で差し替える（Workspace::new がペインを要求するため仮を渡す）
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
            terminals: HashMap::new(),
            theme: Theme::default(),
            focus_handle: cx.focus_handle(),
            cell_size: None,
            selecting: None,
            pane_text_areas: Vec::new(),
        };
        // 仮ルートペインにセッションを張る
        let root_id = app.workspace.active_tab().tree().focused();
        app.attach_session(root_id, cx);
        app
    }

    /// ペイン ID に対する新しい TerminalSession を起動し、イベント中継タスクを張る
    fn attach_session(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let (session, mut rx) = TerminalSession::spawn(INITIAL_COLS, INITIAL_ROWS)
            .expect("PTY 付きシェルを起動できなかった");
        self.terminals.insert(pane_id, session);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let result = this.update(cx, |app: &mut TakoApp, cx| {
                    app.on_term_event(pane_id, event, cx);
                });
                if result.is_err() {
                    break; // View が破棄された
                }
            }
        })
        .detach();
    }

    fn on_term_event(
        &mut self,
        pane_id: PaneId,
        event: tako_core::TermEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.terminals.get_mut(&pane_id) else {
            return;
        };
        match session.process_event(event) {
            Some(SessionNotice::Exited) => self.remove_pane(pane_id, cx),
            Some(SessionNotice::ClipboardStore(text)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Some(SessionNotice::TitleChanged) | None => {}
        }
        cx.notify();
    }

    fn focused_pane(&self) -> PaneId {
        self.workspace.active_tab().tree().focused()
    }

    fn focused_session(&self) -> Option<&TerminalSession> {
        self.terminals.get(&self.focused_pane())
    }

    // --- ペイン操作（ドメイン API の薄い呼び出し。FR-2.5 と同じセマンティクス） ---

    fn split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        let target = self.focused_pane();
        let pane = Pane::new(PaneOrigin::User);
        let pane_id = pane.id();
        if self
            .workspace
            .active_tab_mut()
            .tree_mut()
            .split(target, direction, pane)
            .is_ok()
        {
            self.attach_session(pane_id, cx);
        }
        cx.notify();
    }

    /// フォーカス中ペインを閉じる。タブ最後の 1 ペインならタブを閉じ、最後のタブならアプリ終了
    fn close_focused_pane(&mut self, cx: &mut Context<Self>) {
        self.remove_pane(self.focused_pane(), cx);
    }

    fn remove_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(tab_id) = self
            .workspace
            .tabs()
            .iter()
            .find(|t| t.tree().contains(pane_id))
            .map(|t| t.id())
        else {
            return;
        };
        let tab = self
            .workspace
            .get_tab_mut(tab_id)
            .expect("直前に存在を確認したタブ");
        match tab.tree_mut().close(pane_id) {
            Ok(_) => {
                self.terminals.remove(&pane_id);
            }
            Err(_) => {
                // LastPane: タブごと閉じる
                self.remove_tab(tab_id, cx);
            }
        }
        cx.notify();
    }

    fn remove_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.get_tab(tab_id) else {
            return;
        };
        let pane_ids: Vec<PaneId> = tab.tree().panes().iter().map(|p| p.id()).collect();
        match self.workspace.close_tab(tab_id) {
            Ok(_) => {
                for id in pane_ids {
                    self.terminals.remove(&id);
                }
            }
            Err(_) => {
                // LastTab: アプリ終了は UI 層の責務
                cx.quit();
            }
        }
        cx.notify();
    }

    fn focus_direction(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        self.workspace
            .active_tab_mut()
            .tree_mut()
            .focus_direction(direction);
        cx.notify();
    }

    fn resize_focused(&mut self, axis: SplitAxis, delta: f32, cx: &mut Context<Self>) {
        let target = self.focused_pane();
        // 合う軸の分割が無いときは何もしない（単一ペイン等）
        let _ = self
            .workspace
            .active_tab_mut()
            .tree_mut()
            .resize_by(target, axis, delta);
        cx.notify();
    }

    fn new_tab(&mut self, cx: &mut Context<Self>) {
        let title = format!("{}", self.workspace.tabs().len() + 1);
        let pane = Pane::new(PaneOrigin::User);
        let pane_id = pane.id();
        self.workspace.create_tab(title, pane);
        self.attach_session(pane_id, cx);
        cx.notify();
    }

    fn activate_tab_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.tabs().get(index).map(|t| t.id()) {
            let _ = self.workspace.activate_tab(id);
        }
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.focused_session().and_then(|s| s.selection_text()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if let Some(session) = self.focused_session() {
            session.paste(&text);
        }
        cx.notify();
    }

    // --- キー入力 ---

    fn handle_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        // cmd を含む未バインドのキーはシェルへ流さない
        if keystroke.modifiers.platform {
            return;
        }
        if let Some(bytes) = keystroke_to_bytes(keystroke) {
            if let Some(session) = self.focused_session() {
                session.clear_selection();
                session.write(bytes);
                cx.notify();
            }
        }
    }

    // --- マウス ---

    /// ウィンドウ座標をペイン内のセル座標へ変換する（col, row, セル右半分か）
    fn cell_at(&self, pane_id: PaneId, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id)?;
        let cell = self.cell_size?;
        let session = self.terminals.get(&pane_id)?;
        let (cols, rows) = session.size();
        let local = position - area.origin;
        let x = (f32::from(local.x) / f32::from(cell.width)).max(0.0);
        let y = (f32::from(local.y) / f32::from(cell.height)).max(0.0);
        let col = (x as usize).min(cols.saturating_sub(1));
        let row = (y as usize).min(rows.saturating_sub(1));
        Some((col, row, x.fract() > 0.5))
    }

    fn on_pane_mouse_down(
        &mut self,
        pane_id: PaneId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let _ = self.workspace.active_tab_mut().tree_mut().focus(pane_id);
        if let Some((col, row, right)) = self.cell_at(pane_id, event.position) {
            if let Some(session) = self.terminals.get(&pane_id) {
                let kind = match event.click_count {
                    1 => SelectionKind::Simple,
                    2 => SelectionKind::Word,
                    _ => SelectionKind::Line,
                };
                session.clear_selection();
                session.start_selection(kind, col, row, right);
                self.selecting = Some(pane_id);
            }
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(pane_id) = self.selecting else {
            return;
        };
        if let Some((col, row, right)) = self.cell_at(pane_id, event.position) {
            if let Some(session) = self.terminals.get(&pane_id) {
                session.extend_selection(col, row, right);
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.selecting.take() {
            // iTerm2 流の copy-on-select
            if let Some(text) = self
                .terminals
                .get(&pane_id)
                .and_then(|s| s.selection_text())
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            cx.notify();
        }
    }

    fn on_pane_scroll(
        &mut self,
        pane_id: PaneId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.cell_size else {
            return;
        };
        let lines = match event.delta {
            ScrollDelta::Lines(l) => (l.y * 3.0) as i32,
            ScrollDelta::Pixels(p) => (f32::from(p.y) / f32::from(cell.height)) as i32,
        };
        if lines != 0 {
            if let Some(session) = self.terminals.get(&pane_id) {
                session.scroll_display(lines);
                cx.notify();
            }
        }
    }

    // --- 描画 ---

    /// セル寸法を実測する（等幅前提で 'M' の advance + テーマ行高）
    fn measure_cell(&mut self, window: &mut Window) -> Size<Pixels> {
        if let Some(cell) = self.cell_size {
            return cell;
        }
        let font = Font {
            family: SharedString::from(self.theme.font_family.clone()),
            ..gpui::font(self.theme.font_family.clone())
        };
        let font_id = window.text_system().resolve_font(&font);
        let width = window
            .text_system()
            .advance(font_id, px(self.theme.font_size), 'M')
            .map(|advance| advance.width)
            // 計測に失敗してもセル幅ゼロで詰まないよう概算へフォールバック
            .unwrap_or(px(self.theme.font_size * 0.6));
        let cell = size(width, px(self.theme.line_height));
        self.cell_size = Some(cell);
        cell
    }

    fn text_style(&self) -> TextStyle {
        TextStyle {
            color: hsla(self.theme.foreground),
            font_family: SharedString::from(self.theme.font_family.clone()),
            font_size: px(self.theme.font_size).into(),
            line_height: px(self.theme.line_height).into(),
            ..TextStyle::default()
        }
    }

    fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.workspace.active_tab_id();
        let tabs: Vec<_> = self
            .workspace
            .tabs()
            .iter()
            .map(|tab| {
                let id = tab.id();
                // タブ表示名: フォーカス中ペインの OSC タイトルがあれば優先
                let label = tab
                    .tree()
                    .panes()
                    .iter()
                    .find(|p| p.id() == tab.tree().focused())
                    .and_then(|p| self.terminals.get(&p.id()))
                    .and_then(|s| s.title())
                    .unwrap_or(tab.title())
                    .to_string();
                (id, label)
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .children(tabs.into_iter().map(|(id, label)| {
                let is_active = id == active;
                div()
                    .id(("tab", id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .px_3()
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgba(theme.tab_active_background))
                            .border_b_2()
                            .border_color(hsla(theme.accent))
                    })
                    .text_color(if is_active {
                        hsla(theme.tab_active_foreground)
                    } else {
                        hsla(theme.tab_inactive_foreground)
                    })
                    .text_size(px(12.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.workspace.activate_tab(id);
                        cx.notify();
                    }))
                    .child(SharedString::from(truncate(&label, 24)))
                    .child(
                        div()
                            .id(("tab-close", id.as_u64()))
                            .px_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_tab(id, cx);
                            }))
                            .child("×"),
                    )
            }))
            .child(
                div()
                    .id("tab-new")
                    .px_3()
                    .cursor_pointer()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                    .child("+"),
            )
    }

    fn render_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: Bounds<Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let default_style = self.text_style();
        let cell = self.cell_size.expect("render 冒頭で実測済み");

        // PTY リサイズ追従: テキスト領域に収まる cols/rows へ
        let cols = (f32::from(area.size.width) / f32::from(cell.width)).floor() as usize;
        let rows = (f32::from(area.size.height) / f32::from(cell.height)).floor() as usize;
        if let Some(session) = self.terminals.get_mut(&pane_id) {
            session.resize(
                cols,
                rows,
                f32::from(cell.width).round() as u16,
                f32::from(cell.height).round() as u16,
            );
        }

        let screen = self.terminals.get(&pane_id).map(|s| s.screen(&theme));

        let lines: Vec<_> = screen
            .map(|screen| {
                screen
                    .lines
                    .into_iter()
                    .map(|line| {
                        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
                            .runs
                            .iter()
                            .map(|run| {
                                (
                                    run.range.clone(),
                                    HighlightStyle {
                                        color: Some(hsla(run.fg)),
                                        background_color: run.bg.map(hsla),
                                        font_weight: run.bold.then_some(FontWeight::BOLD),
                                        font_style: run.italic.then_some(FontStyle::Italic),
                                        underline: run.underline.then_some(UnderlineStyle {
                                            thickness: px(1.0),
                                            color: None,
                                            wavy: false,
                                        }),
                                        strikethrough: run.strikeout.then_some(
                                            StrikethroughStyle {
                                                thickness: px(1.0),
                                                color: None,
                                            },
                                        ),
                                        fade_out: None,
                                    },
                                )
                            })
                            .collect();
                        div().h(px(theme.line_height)).child(
                            StyledText::new(line.text)
                                .with_default_highlights(&default_style, highlights),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.pane_border)
            })
            .p(px(PANE_PADDING))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.on_pane_mouse_down(pane_id, event, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                this.on_pane_scroll(pane_id, event, cx);
            }))
            .children(lines)
    }
}

/// 文字数ベースの単純な切り詰め（タブ表示名用）
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// GPUI の Keystroke を端末入力バイト列へ変換する
fn keystroke_to_bytes(ks: &Keystroke) -> Option<Vec<u8>> {
    // Ctrl+英字 → C0 制御コード
    if ks.modifiers.control {
        let mut chars = ks.key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_lowercase() as u8) & 0x1f]);
            }
        }
    }
    let bytes: &[u8] = match ks.key.as_str() {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        _ => {
            // 印字可能文字は key_char をそのまま送る（IME 確定文字列もここに来る）
            let ch = ks.key_char.as_ref()?;
            if ch.is_empty() {
                return None;
            }
            return Some(ch.as_bytes().to_vec());
        }
    };
    Some(bytes.to_vec())
}

impl Render for TakoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = self.measure_cell(window);
        let theme = self.theme.clone();

        // アクティブタブのレイアウト（単位矩形）と、マウス変換用のピクセル矩形を更新する
        let viewport = window.viewport_size();
        let content_origin = point(px(0.0), px(TAB_BAR_HEIGHT));
        let content_size = size(viewport.width, viewport.height - px(TAB_BAR_HEIGHT));
        let tree = self.workspace.active_tab().tree();
        let focused = tree.focused();
        let layout = tree.layout(Rect::UNIT);
        self.pane_text_areas = layout
            .iter()
            .map(|(id, r)| {
                let inset = PANE_BORDER + PANE_PADDING;
                let origin = point(
                    content_origin.x + content_size.width * r.x + px(inset),
                    content_origin.y + content_size.height * r.y + px(inset),
                );
                let area_size = size(
                    content_size.width * r.width - px(inset * 2.0),
                    content_size.height * r.height - px(inset * 2.0),
                );
                (*id, Bounds::new(origin, area_size))
            })
            .collect();

        let panes: Vec<_> = layout
            .into_iter()
            .map(|(id, rect)| {
                let area = self
                    .pane_text_areas
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, b)| *b)
                    .expect("直前に同じ layout から構築済み");
                self.render_pane(id, rect, area, id == focused, cx)
            })
            .collect();
        let _ = cell;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgba(theme.background))
            .font_family(theme.font_family.clone())
            .text_size(px(theme.font_size))
            .track_focus(&self.focus_handle)
            .key_context("TakoApp")
            .on_action(
                cx.listener(|this, _: &SplitRight, _, cx| this.split(SplitDirection::Right, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitDown, _, cx| this.split(SplitDirection::Down, cx)),
            )
            .on_action(cx.listener(|this, _: &ClosePane, _, cx| this.close_focused_pane(cx)))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_tab(cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| {
                this.workspace.activate_next_tab();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| {
                this.workspace.activate_prev_tab();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, _, cx| {
                this.focus_direction(SplitDirection::Left, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusRight, _, cx| {
                this.focus_direction(SplitDirection::Right, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusUp, _, cx| {
                    this.focus_direction(SplitDirection::Up, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusDown, _, cx| {
                this.focus_direction(SplitDirection::Down, cx)
            }))
            .on_action(cx.listener(|this, _: &WidenPane, _, cx| {
                this.resize_focused(SplitAxis::Horizontal, RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &NarrowPane, _, cx| {
                this.resize_focused(SplitAxis::Horizontal, -RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &TallenPane, _, cx| {
                this.resize_focused(SplitAxis::Vertical, RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &ShortenPane, _, cx| {
                this.resize_focused(SplitAxis::Vertical, -RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &PasteClipboard, _, cx| this.paste(cx)))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &ActivateTab1, _, cx| this.activate_tab_index(0, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, _, cx| this.activate_tab_index(1, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, _, cx| this.activate_tab_index(2, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, _, cx| this.activate_tab_index(3, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, _, cx| this.activate_tab_index(4, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, _, cx| this.activate_tab_index(5, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, _, cx| this.activate_tab_index(6, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, _, cx| this.activate_tab_index(7, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab9, _, cx| this.activate_tab_index(8, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.handle_key(&event.keystroke, cx);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.on_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.on_mouse_up(event, cx);
                }),
            )
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().relative().children(panes))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.bind_keys(key_bindings());
        let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(TakoApp::new);
                    window.focus(&view.read(cx).focus_handle.clone(), cx);
                    view
                },
            )
            .expect("ウィンドウを開けなかった");
        cx.activate(true);

        if std::env::var_os("TAKO_SELF_TEST").is_some() {
            self_test::run(window, cx);
        }
    });
}

/// セルフテスト: キーディスパッチ経由で入力・分割・フォーカス・リサイズ・タブ・色・
/// スクロールバック・コピペの経路を機械検証して終了する。
/// `WindowHandle<V>::update` 内の dispatch_keystroke はルートビューの二重借用で
/// パニックするため AnyWindowHandle::update を使う（poc/README.md）
mod self_test {
    use super::*;
    use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};

    fn fail(step: &str) -> ! {
        println!("TAKO_APP_SELF_TEST_FAILED: {step}");
        std::process::exit(1);
    }

    fn check(cond: bool, step: &str) {
        if !cond {
            fail(step);
        }
    }

    /// 文字列をキーストローク列としてウィンドウへ流し込む
    fn type_text(any: AnyWindowHandle, cx: &mut AsyncApp, text: &str, enter: bool) {
        let text = text.to_string();
        let _ = any.update(cx, |_, window, cx| {
            for ch in text.chars() {
                let keystroke = Keystroke {
                    modifiers: Modifiers::default(),
                    key: ch.to_string(),
                    key_char: Some(ch.to_string()),
                };
                window.dispatch_keystroke(keystroke, cx);
            }
            if enter {
                // 固定文字列のパースは失敗しない（論理的に到達不能）
                window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
            }
        });
    }

    fn press(any: AnyWindowHandle, cx: &mut AsyncApp, combo: &str) {
        let combo = combo.to_string();
        let _ = any.update(cx, |_, window, cx| {
            window.dispatch_keystroke(
                Keystroke::parse(&combo).expect("セルフテストのキー表記は固定"),
                cx,
            );
        });
    }

    /// フォーカス中ペインの表示行に needle が含まれるか
    fn focused_contains(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, needle: &str) -> bool {
        window
            .update(cx, |app, _, _| {
                app.focused_session()
                    .map(|s| s.visible_lines().iter().any(|l| l.contains(needle)))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    pub fn run(window: WindowHandle<TakoApp>, cx: &mut App) {
        cx.spawn(async move |cx| {
            let any: AnyWindowHandle = window.into();
            let wait = |cx: &mut AsyncApp, ms: u64| {
                cx.background_executor().timer(Duration::from_millis(ms))
            };

            // 1. 起動 + 素の入力経路
            wait(cx, 2500).await;
            type_text(any, cx, "echo TAKO-INPUT-OK", true);
            wait(cx, 1000).await;
            check(focused_contains(window, cx, "TAKO-INPUT-OK"), "入力エコー");

            let pane1 = window
                .update(cx, |app, _, _| app.focused_pane())
                .unwrap_or_else(|_| fail("初期ペイン取得"));

            // 2. cmd-d 縦分割（右に生える）
            press(any, cx, "cmd-d");
            wait(cx, 1500).await;
            let (pane_count, terminal_count, pane2) = window
                .update(cx, |app, _, _| {
                    (
                        app.workspace.active_tab().tree().len(),
                        app.terminals.len(),
                        app.focused_pane(),
                    )
                })
                .unwrap_or_else(|_| fail("分割後の状態取得"));
            check(pane_count == 2 && terminal_count == 2, "cmd-d で 2 ペイン");
            check(pane2 != pane1, "分割後フォーカスは新ペイン");

            // 3. 新ペインだけに入力が流れる
            type_text(any, cx, "echo TAKO-PANE2-OK", true);
            wait(cx, 1000).await;
            check(
                focused_contains(window, cx, "TAKO-PANE2-OK"),
                "ペイン 2 へ入力",
            );
            let pane1_clean = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&pane1)
                        .map(|s| {
                            !s.visible_lines()
                                .iter()
                                .any(|l| l.contains("TAKO-PANE2-OK"))
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(pane1_clean, "ペイン 1 に漏れない");

            // 4. 方向フォーカス移動
            press(any, cx, "cmd-alt-left");
            let refocused = window
                .update(cx, |app, _, _| app.focused_pane())
                .unwrap_or_else(|_| fail("フォーカス移動後の取得"));
            check(refocused == pane1, "cmd-alt-left で左ペインへ");

            // 5. キーボードリサイズ（フォーカスペインの横取り分が増える）
            press(any, cx, "ctrl-cmd-right");
            let width = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .active_tab()
                        .tree()
                        .layout(Rect::UNIT)
                        .into_iter()
                        .find(|(id, _)| *id == pane1)
                        .map(|(_, r)| r.width)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            check(width > 0.52, "ctrl-cmd-right でリサイズ");

            // 6. cmd-w でフォーカスペインを閉じる
            press(any, cx, "cmd-w");
            wait(cx, 300).await;
            let (pane_count, terminal_count, survivor) = window
                .update(cx, |app, _, _| {
                    (
                        app.workspace.active_tab().tree().len(),
                        app.terminals.len(),
                        app.focused_pane(),
                    )
                })
                .unwrap_or_else(|_| fail("クローズ後の状態取得"));
            check(
                pane_count == 1 && terminal_count == 1,
                "cmd-w で 1 ペインへ",
            );
            check(survivor == pane2, "残ペインへフォーカス引き継ぎ");

            // 7. cmd-t で新タブ
            press(any, cx, "cmd-t");
            wait(cx, 1500).await;
            let (tab_count, is_second_active, terminal_count) = window
                .update(cx, |app, _, _| {
                    let tabs = app.workspace.tabs();
                    (
                        tabs.len(),
                        tabs.last().map(|t| t.id()) == Some(app.workspace.active_tab_id()),
                        app.terminals.len(),
                    )
                })
                .unwrap_or_else(|_| fail("新タブ後の状態取得"));
            check(
                tab_count == 2 && is_second_active && terminal_count == 2,
                "cmd-t で新タブ",
            );

            // 8. 色つき出力（ANSI 赤）が Theme の赤へ解決される
            type_text(any, cx, r"printf '\e[31mTAKO-RED\e[0m\n'", true);
            wait(cx, 1000).await;
            let red_ok = window
                .update(cx, |app, _, _| {
                    let theme = app.theme.clone();
                    app.focused_session()
                        .map(|s| {
                            let screen = s.screen(&theme);
                            screen.lines.iter().any(|line| {
                                line.text.contains("TAKO-RED")
                                    && line.runs.iter().any(|run| {
                                        run.fg == theme.ansi[1]
                                            && line.text[run.range.clone()].contains("TAKO-RED")
                                    })
                            })
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(red_ok, "ANSI 赤の解決");

            // 9. スクロールバック表示
            type_text(any, cx, "seq 1 200", true);
            wait(cx, 1200).await;
            let scroll_ok = window
                .update(cx, |app, _, _| {
                    let theme = app.theme.clone();
                    app.focused_session()
                        .map(|s| {
                            s.scroll_display(5);
                            let screen = s.screen(&theme);
                            let ok = screen.display_offset == 5 && screen.cursor.is_none();
                            s.scroll_to_bottom();
                            ok
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(scroll_ok, "スクロールバック表示");

            // 10. cmd-v ペースト
            let _ = window.update(cx, |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string("TAKO-PASTE-OK".into()));
            });
            press(any, cx, "cmd-v");
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "TAKO-PASTE-OK"),
                "cmd-v ペースト",
            );

            // 11. 選択 → cmd-c コピー
            let selected = window
                .update(cx, |app, _, _| {
                    let session = app.focused_session()?;
                    let lines = session.visible_lines();
                    let (row, line) = lines
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, l)| l.contains("TAKO-PASTE-OK"))?;
                    let col = line.find("TAKO-PASTE-OK")?;
                    session.start_selection(SelectionKind::Simple, col, row, false);
                    session.extend_selection(col + "TAKO-PASTE-OK".len() - 1, row, true);
                    Some(())
                })
                .ok()
                .flatten();
            check(selected.is_some(), "選択範囲の設定");
            press(any, cx, "cmd-c");
            wait(cx, 200).await;
            let copied = window
                .update(cx, |_, _, cx| {
                    cx.read_from_clipboard().and_then(|item| item.text())
                })
                .ok()
                .flatten();
            check(copied.as_deref() == Some("TAKO-PASTE-OK"), "cmd-c コピー");

            // 12. cmd-1 でタブ切替
            press(any, cx, "cmd-1");
            let back_to_first = window
                .update(cx, |app, _, _| {
                    app.workspace.tabs().first().map(|t| t.id())
                        == Some(app.workspace.active_tab_id())
                })
                .unwrap_or(false);
            check(back_to_first, "cmd-1 でタブ 1 へ");

            // 13. PTY リサイズ追従（初期 80x24 から実寸へ広がっている）
            let resized = window
                .update(cx, |app, _, _| {
                    app.focused_session()
                        .map(|s| s.size().0 > INITIAL_COLS)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(resized, "PTY リサイズ追従");

            println!("TAKO_APP_SELF_TEST_OK");
            std::process::exit(0);
        })
        .detach();
    }
}

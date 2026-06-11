//! Phase 0 PoC (b): alacritty_terminal + PTY + GPUI の最小ターミナル
//!
//! 成功条件: ウィンドウ内でシェルが起動し、出力が描画され、キー入力を送れること。
//! 色・スクロール・リサイズ・カーソル描画は対象外（Phase 1 でやる）。
//!
//! 構成:
//! - alacritty_terminal の tty モジュールで PTY + シェルを spawn（portable-pty は不使用。
//!   alacritty_terminal 自体が macOS openpty / Windows ConPTY を吸収している）
//! - EventLoop（専用 IO スレッド）が PTY 出力をパースして Term グリッドを更新
//! - EventListener から futures channel 経由で GPUI に Wakeup を渡し、cx.notify() で再描画
//! - キー入力は GPUI の KeyDownEvent → バイト列に変換して Notifier で PTY へ書き込み

use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{test::TermSize, Config, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Keystroke, Modifiers,
    SharedString, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use std::time::Duration;

const COLS: usize = 100;
const ROWS: usize = 30;

/// alacritty の EventLoop スレッドから GPUI 側へイベントを中継するプロキシ
#[derive(Clone)]
struct EventProxy(UnboundedSender<TermEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

struct TerminalView {
    term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
    focus_handle: FocusHandle,
}

impl TerminalView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = unbounded::<TermEvent>();
        let proxy = EventProxy(tx);

        let term_size = TermSize::new(COLS, ROWS);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &term_size,
            proxy.clone(),
        )));

        // セル寸法は PTY の TIOCSWINSZ 用。PoC は固定値でよい
        let window_size = WindowSize {
            num_lines: ROWS as u16,
            num_cols: COLS as u16,
            cell_width: 8,
            cell_height: 16,
        };
        let pty = tty::new(&tty::Options::default(), window_size, 0).expect("PTY 生成に失敗");

        let event_loop =
            EventLoop::new(term.clone(), proxy, pty, false, false).expect("EventLoop 生成に失敗");
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();

        // PTY 側イベントを受けて再描画。PtyWrite（端末からの応答要求）だけは PTY に書き戻す
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let result = this.update(cx, |view: &mut TerminalView, cx| {
                    if let TermEvent::PtyWrite(text) = event {
                        view.notifier.notify(text.into_bytes());
                    }
                    cx.notify();
                });
                if result.is_err() {
                    break; // View が破棄された
                }
            }
        })
        .detach();

        Self {
            term,
            notifier,
            focus_handle: cx.focus_handle(),
        }
    }

    /// セルフテスト用: グリッドのどこかに指定文字列だけの行があるか
    fn grid_has_exact_line(&self, needle: &str) -> bool {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut lines = vec![String::new(); ROWS];
        for indexed in content.display_iter {
            let row = indexed.point.line.0;
            if (0..ROWS as i32).contains(&row) {
                lines[row as usize].push(indexed.cell.c);
            }
        }
        lines.iter().any(|l| l.trim() == needle)
    }

    /// 表示中グリッドを行ごとの文字列へ変換（色・装飾は捨てる）
    fn visible_lines(&self) -> Vec<SharedString> {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut lines = vec![String::new(); ROWS];
        for indexed in content.display_iter {
            let row = indexed.point.line.0;
            if (0..ROWS as i32).contains(&row) {
                lines[row as usize].push(indexed.cell.c);
            }
        }
        lines
            .into_iter()
            .map(|l| {
                // 空行は高さを保つため空白 1 文字にする
                let l = l.trim_end().to_string();
                SharedString::from(if l.is_empty() { " ".to_string() } else { l })
            })
            .collect()
    }

    fn handle_key(&mut self, keystroke: &Keystroke) {
        if let Some(bytes) = keystroke_to_bytes(keystroke) {
            self.notifier.notify(bytes);
        }
    }
}

/// GPUI の Keystroke を端末入力バイト列へ変換（PoC 用の最小マッピング）
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

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .font_family("Menlo")
            .text_size(px(13.))
            .p_2()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _window, _cx| {
                this.handle_key(&event.keystroke);
            }))
            .children(self.visible_lines().into_iter().map(|line| {
                div().h(px(16.)).whitespace_nowrap().child(line)
            }))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(840.), px(540.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(TerminalView::new);
                    window.focus(&view.read(cx).focus_handle.clone(), cx);
                    view
                },
            )
            .expect("ウィンドウを開けなかった");
        cx.activate(true);
        println!("TAKO_POC_TERMINAL_OPENED");

        // セルフテスト: GPUI のキーディスパッチ経路（dispatch_keystroke → on_key_down →
        // Notifier → PTY）で echo を流し、シェル出力がグリッドに反映されるかを機械検証する。
        // ウィンドウが occluded で描画停止していても入力・パース経路は検証できる
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(2))
                .await;
            // WindowHandle<V>::update はルートビューを lease するため、その中で
            // dispatch_keystroke すると listener の view.update と二重借用でパニックする。
            // AnyWindowHandle::update（AnyView 渡し・lease なし）を使う
            let any_window: gpui::AnyWindowHandle = window.into();
            let _ = any_window.update(cx, |_view, window, cx| {
                for ch in "echo TAKO-INPUT-OK".chars() {
                    let keystroke = Keystroke {
                        modifiers: Modifiers::default(),
                        key: ch.to_string(),
                        key_char: Some(ch.to_string()),
                    };
                    window.dispatch_keystroke(keystroke, cx);
                }
                window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
            });
            cx.background_executor()
                .timer(Duration::from_secs(2))
                .await;
            let _ = window.update(cx, |view, _window, _cx| {
                if view.grid_has_exact_line("TAKO-INPUT-OK") {
                    println!("TAKO_POC_INPUT_ECHO_VERIFIED");
                } else {
                    println!("TAKO_POC_INPUT_ECHO_MISSING");
                }
            });
        })
        .detach();
    });
}

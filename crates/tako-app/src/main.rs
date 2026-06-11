//! tako-app — GPUI バイナリ（UI 層）
//!
//! Phase 1 前半: PoC（`poc/03-term-poc`）相当の最小ターミナル（1 ペイン）。
//! タブ UI・複数ペイン描画・スクロールバック・色は Phase 1 後半でやる。
//!
//! GPUI への依存はこのクレートだけに閉じ込める（`.agent/architecture.md`）。
//! ターミナルのドメインロジック（PTY・グリッド）は tako-core::TerminalSession 側にある。
//!
//! `TAKO_SELF_TEST=1` で起動すると、GPUI キーディスパッチ経由で echo を注入し
//! 入力 → PTY → シェル実行 → グリッド反映の経路を機械検証して終了する。

use std::time::Duration;

use futures::StreamExt;
use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Keystroke, Modifiers,
    SharedString, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use tako_core::TerminalSession;

const COLS: usize = 100;
const ROWS: usize = 30;

struct TerminalView {
    session: TerminalSession,
    focus_handle: FocusHandle,
}

impl TerminalView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (session, mut rx) =
            TerminalSession::spawn(COLS, ROWS).expect("PTY 付きシェルを起動できなかった");

        // PTY 側イベントを受けてセッションに処理させ、再描画する
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let result = this.update(cx, |view: &mut TerminalView, cx| {
                    view.session.process_event(event);
                    cx.notify();
                });
                if result.is_err() {
                    break; // View が破棄された
                }
            }
        })
        .detach();

        Self {
            session,
            focus_handle: cx.focus_handle(),
        }
    }

    fn handle_key(&mut self, keystroke: &Keystroke) {
        if let Some(bytes) = keystroke_to_bytes(keystroke) {
            self.session.write(bytes);
        }
    }

    /// 表示行を GPUI 描画用に変換（空行は高さを保つため空白 1 文字にする）
    fn render_lines(&self) -> Vec<SharedString> {
        self.session
            .visible_lines()
            .into_iter()
            .map(|l| SharedString::from(if l.is_empty() { " ".to_string() } else { l }))
            .collect()
    }
}

/// GPUI の Keystroke を端末入力バイト列へ変換（Phase 1 前半の最小マッピング）
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
            .on_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _window, _cx| {
                    this.handle_key(&event.keystroke);
                }),
            )
            .children(
                self.render_lines()
                    .into_iter()
                    .map(|line| div().h(px(16.)).whitespace_nowrap().child(line)),
            )
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

        if std::env::var_os("TAKO_SELF_TEST").is_some() {
            run_self_test(window, cx);
        }
    });
}

/// 入力 → PTY → シェル実行 → グリッド反映の経路を機械検証して終了する。
/// PoC と同じ手口: WindowHandle<V>::update 内の dispatch_keystroke はルートビューの
/// 二重借用でパニックするため AnyWindowHandle::update を使う（poc/README.md）
fn run_self_test(window: gpui::WindowHandle<TerminalView>, cx: &mut App) {
    const NEEDLE: &str = "TAKO-INPUT-OK";
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let any_window: gpui::AnyWindowHandle = window.into();
        let _ = any_window.update(cx, |_view, window, cx| {
            for ch in format!("echo {NEEDLE}").chars() {
                let keystroke = Keystroke {
                    modifiers: Modifiers::default(),
                    key: ch.to_string(),
                    key_char: Some(ch.to_string()),
                };
                window.dispatch_keystroke(keystroke, cx);
            }
            // enter は固定文字列のパースであり失敗しない（論理的に到達不能）
            window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
        });
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let verified = window
            .update(cx, |view, _window, _cx| {
                view.session
                    .visible_lines()
                    .iter()
                    .any(|l| l.trim() == NEEDLE)
            })
            .unwrap_or(false);
        if verified {
            println!("TAKO_APP_INPUT_ECHO_VERIFIED");
            std::process::exit(0);
        } else {
            println!("TAKO_APP_INPUT_ECHO_MISSING");
            std::process::exit(1);
        }
    })
    .detach();
}

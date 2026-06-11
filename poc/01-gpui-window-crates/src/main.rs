//! Phase 0 PoC (a): crates.io 版 gpui 0.2.2 で最小ウィンドウを開く
//! 成功条件: ウィンドウが開き、テキストが描画され、クラッシュしないこと

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};

struct HelloTako;

impl Render for HelloTako {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0x1e1e2e))
            .text_xl()
            .text_color(rgb(0xcdd6f4))
            .child("tako Phase 0 — GPUI (crates.io 0.2.2) window OK")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.), px(360.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| HelloTako),
        )
        .expect("ウィンドウを開けなかった");
        cx.activate(true);
        // 起動検証用マーカー（外部からの起動成功判定に使う）
        println!("TAKO_POC_WINDOW_OPENED");
    });
}

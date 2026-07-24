//! About ウィンドウ（Issue #485）— tako メニューの「tako について」
//!
//! macOS 標準の About パネル相当。バージョン（`CARGO_PKG_VERSION` = .app の
//! `CFBundleShortVersionString` と同一。`scripts/build-app.sh` が Cargo.toml から
//! 生成する）・配布系統・ライセンス・関連リンクを 1 枚で見せる。
//! 独立 GPUI ウィンドウとして開く点は設定画面（`settings_window`）と同じ方式。

use gpui::*;
use tako_core::theme::{Rgb, Theme};

use crate::ui_text::about as txt;
use crate::TakoApp;

/// リンク先（言語非依存）
pub const REPOSITORY_URL: &str = "https://github.com/takushio2525/tako";
pub const DOCUMENTATION_URL: &str = "https://tako-docs.pages.dev/";
pub const RELEASES_URL: &str = "https://github.com/takushio2525/tako/releases";
pub const ISSUES_URL: &str = "https://github.com/takushio2525/tako/issues";

pub struct AboutWindow {
    tako_app: WeakEntity<TakoApp>,
    /// 「情報をコピー」を押した直後だけ完了表示に切り替える
    copied: bool,
}

impl AboutWindow {
    pub fn new(tako_app: WeakEntity<TakoApp>, cx: &mut Context<Self>) -> Self {
        // テーマ変更（tako theme / 設定画面）に追従する
        if let Some(app) = tako_app.upgrade() {
            cx.observe(&app, |_this: &mut Self, _app, cx| cx.notify())
                .detach();
        }
        Self {
            tako_app,
            copied: false,
        }
    }

    fn theme(&self) -> Theme {
        tako_control::settings::load().resolve_theme().0
    }

    /// 不具合報告に貼れる 1 行情報（バージョン + 配布系統 + アーキテクチャ）
    fn info_line(&self) -> String {
        format!(
            "tako {} ({}, {})",
            crate::update_checker::CURRENT_VERSION,
            crate::update_checker::detect_install_method().label(),
            std::env::consts::ARCH,
        )
    }
}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let version = crate::update_checker::CURRENT_VERSION;
        let method = crate::update_checker::detect_install_method().label();
        div()
            .flex()
            .flex_col()
            .items_center()
            .size_full()
            .gap(px(6.))
            .pt(px(28.))
            .px(px(24.))
            .pb(px(20.))
            .bg(to_hsla(theme.surface_0))
            .text_color(to_hsla(theme.foreground))
            .child(
                div()
                    .text_size(px(34.))
                    .font_weight(FontWeight::BOLD)
                    .child(txt::PRODUCT),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(to_hsla(theme.text_muted))
                    .child(txt::tagline()),
            )
            .child(
                div()
                    .mt(px(10.))
                    .text_size(px(13.))
                    .child(SharedString::from(txt::version(version))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(to_hsla(theme.text_muted))
                    .child(SharedString::from(txt::install_method(method))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(to_hsla(theme.text_muted))
                    .child(SharedString::from(txt::license_line(txt::LICENSE))),
            )
            .child(
                div()
                    .mt(px(14.))
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(button(
                        "about-check-updates",
                        txt::check_updates(),
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            if let Some(app) = this.tako_app.upgrade() {
                                app.update(cx, |app, cx| app.start_update_check(cx));
                            }
                        }),
                    ))
                    .child(button(
                        "about-copy-info",
                        if self.copied {
                            txt::copied()
                        } else {
                            txt::copy_info()
                        },
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(this.info_line()));
                            this.copied = true;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_row()
                    .gap(px(14.))
                    .child(link(
                        "about-repo",
                        txt::repository(),
                        REPOSITORY_URL,
                        &theme,
                    ))
                    .child(link(
                        "about-docs",
                        txt::documentation(),
                        DOCUMENTATION_URL,
                        &theme,
                    ))
                    .child(link(
                        "about-releases",
                        txt::releases(),
                        RELEASES_URL,
                        &theme,
                    )),
            )
    }
}

fn button(
    id: &'static str,
    label: &str,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(to_hsla(theme.border_subtle))
        .bg(to_hsla(theme.surface_1))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|d| d.bg(to_hsla(theme.surface_highlight)))
        .child(SharedString::from(label.to_string()))
        .on_click(on_click)
}

fn link(id: &'static str, label: &str, url: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(px(11.5))
        .text_color(to_hsla(theme.accent))
        .cursor_pointer()
        .hover(|d| d.underline())
        .child(SharedString::from(label.to_string()))
        .on_click(move |_, _, _| crate::open_external_url(url))
}

fn to_hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)).into()
}

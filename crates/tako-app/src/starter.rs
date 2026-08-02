//! スターター — GUI ライク表示モードの「空ペイン 3 ボタン」（Issue #691 / #694）
//!
//! 仕様の正は `.agent/plans/2026-07-gui-mode.md` §2.2。ターミナルに抵抗感のある
//! ユーザーが最初に見る画面なので、**次の行動が常に 3 つに絞られている**ことを最優先にする。
//!
//! 表示するかどうかは `TakoApp::pane_display_for`（判定表 = `tako_core::ui_mode`）が決める。
//! ここは描画とクリックの配線だけを持ち、押下時の実処理は
//! `TakoApp::starter_action`（= dispatch / シェル書き込み）に委ねる。

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, MouseButton,
    SharedString,
};
use std::time::Duration;
use tako_core::ui_mode::{SettleKind, StarterAction};

use super::*;
use crate::file_icons::ui_icon;

/// プロファイル一覧を読み直す間隔（#739）。**スターターが画面に出ている間だけ**
/// 2 秒 tick が使う TTL で、新しいポーリングは作らない（§3.1 の方針）
pub(crate) const STARTER_PROFILES_TTL: Duration = Duration::from_secs(15);

/// ドロップダウンの幅・高さの上限（狭幅・多件数でも崩れないようここで抑える）
const PROFILE_MENU_WIDTH: f32 = 300.0;
const PROFILE_MENU_MAX_HEIGHT: f32 = 300.0;

/// 起動カードから選べるプロファイル 1 件（#739）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StarterProfile {
    /// プロファイル名（`default` = 既定）
    pub name: String,
    /// 選ぶ手がかり（担当プロジェクト / 起動フォルダ / モデル）。取れなければ空
    pub summary: String,
}

impl StarterProfile {
    /// 表示名。既定は名前ではなく「既定」と出す（#322 の最簡形と同じ考え方で、
    /// 初心者に `default` という内部名を覚えさせない）
    pub(crate) fn label(&self) -> String {
        if self.name == DEFAULT_PROFILE {
            crate::ui_text::ui_mode::starter_profile_default().to_string()
        } else {
            self.name.clone()
        }
    }
}

/// master / solo それぞれの選択肢（#739）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StarterProfiles {
    pub master: Vec<StarterProfile>,
    pub solo: Vec<StarterProfile>,
}

impl StarterProfiles {
    pub(crate) fn for_action(&self, action: StarterAction) -> &[StarterProfile] {
        match action {
            StarterAction::Master => &self.master,
            StarterAction::Solo => &self.solo,
            // 起動しないカード（コマンド入力へ / setup）にプロファイルは無い
            StarterAction::UseTerminal | StarterAction::Setup => &[],
        }
    }

    /// ▾ を出すか。選択肢が既定 1 つだけのときは出さない
    /// （押しても 1 件しか出ないシェブロンは初心者にはノイズ。Code Runner #453 と同方針）
    pub(crate) fn has_choice(&self, action: StarterAction) -> bool {
        self.for_action(action).len() >= 2
    }
}

/// 既定プロファイルの名前（`tako master` が引数なしで使うもの）
const DEFAULT_PROFILE: &str = "default";

/// スターターから起動するコマンドのサブコマンド部（#739）。
///
/// 既定プロファイルは引数を付けない最簡形（#322。実体は CLI の案内文と同じ
/// `orchestrator::launch_command`）。プロファイル名は
/// [`validate_profile_name`](tako_control::orchestrator::validate_profile_name) を
/// 通ったものだけ通す — **これはシェルへ書き込む文字列**なので、プロファイル
/// ディレクトリに置かれた奇妙なファイル名がそのままコマンド行に混ざらないようにする
pub(crate) fn starter_subcommand(action: StarterAction, profile: Option<&str>) -> Option<String> {
    let sub = action.subcommand()?;
    let Some(profile) = profile else {
        return Some(sub.to_string());
    };
    if tako_control::orchestrator::validate_profile_name(profile).is_err() {
        return None;
    }
    Some(tako_control::orchestrator::launch_command(sub, profile))
}

/// プロファイル一覧の読み込み（**background executor から呼ぶ**。ファイル I/O を含む）。
///
/// 既定は `default.yaml` がまだ無くても必ず先頭に並べる（`tako master` は引数なしで
/// 既定プロファイルを作って起動するため、選択肢としては常に存在する）
pub(crate) fn load_starter_profiles() -> StarterProfiles {
    use tako_control::orchestrator::ProfileKind;
    StarterProfiles {
        master: load_profiles_of(ProfileKind::Master),
        solo: load_profiles_of(ProfileKind::Solo),
    }
}

fn load_profiles_of(kind: tako_control::orchestrator::ProfileKind) -> Vec<StarterProfile> {
    use tako_control::orchestrator as orch;
    let summary_of = |name: &str| {
        orch::load_profile_of(kind, name)
            .map(|p| profile_summary(&p))
            .unwrap_or_default()
    };
    let mut out = vec![StarterProfile {
        name: DEFAULT_PROFILE.to_string(),
        summary: summary_of(DEFAULT_PROFILE),
    }];
    for name in orch::list_profiles_of(kind).unwrap_or_default() {
        // 既定は先頭で出し済み。名前が検証を通らないものは起動コマンドに
        // できないので一覧にも出さない（押せないものを見せない）
        if name == DEFAULT_PROFILE || orch::validate_profile_name(&name).is_err() {
            continue;
        }
        let summary = summary_of(&name);
        out.push(StarterProfile { name, summary });
    }
    out
}

/// 一覧に添える 1 行の手がかり（#739 の「選びやすく」）。
/// 担当プロジェクト → 起動フォルダ → モデルの順に、最初に取れたものだけ出す
fn profile_summary(profile: &tako_control::orchestrator::Profile) -> String {
    use crate::ui_text::ui_mode as txt;
    if let Some(projects) = profile.projects.as_ref().filter(|p| !p.is_empty()) {
        let shown: Vec<&str> = projects.iter().take(3).map(String::as_str).collect();
        let mut list = shown.join(" / ");
        if projects.len() > shown.len() {
            list = txt::starter_profile_more(list, projects.len() - shown.len());
        }
        return txt::starter_profile_projects(&list);
    }
    if let Some(cwd) = profile.cwd.as_ref().filter(|c| !c.is_empty()) {
        return shorten_home(cwd);
    }
    profile.master_model_label()
}

/// ホーム配下を `~` 表記へ（cwd チップと同じ規則）
fn shorten_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

impl TakoApp {
    /// ペイン枠 + ヘッダ（スターターと準備中プレースホルダの共通部分）。
    /// ターミナルペインと同じ高さ・同じ × 位置にしてあるので、GUI 表示のまま閉じられる
    fn render_gui_pane_frame(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        focused: bool,
        show_terminal_button: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        // cwd チップ（ヘッダ左。いまどのフォルダで話が始まるかを見せる）
        let cwd_label = self.terminals.get(&pane_id).and_then(|s| s.cwd()).map(|p| {
            let full = p.to_string_lossy().to_string();
            let home = std::env::var("HOME").unwrap_or_default();
            if !home.is_empty() && full.starts_with(&home) {
                format!("~{}", &full[home.len()..])
            } else {
                full
            }
        });

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .rounded(px(7.0))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla_alpha(theme.accent, 0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                }])
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    let _ = this.workspace.active_tab_mut().tree_mut().focus(pane_id);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .bg(rgba(if focused {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .border_b_1()
                    .border_color(hsla(if focused {
                        theme.border_default
                    } else {
                        theme.border_subtle
                    }))
                    .text_size(px(11.0))
                    .child(
                        svg()
                            .path(ui_icon::CHAT_BUBBLE)
                            .w(px(13.0))
                            .h(px(13.0))
                            .flex_none()
                            .text_color(hsla(theme.accent)),
                    )
                    .when_some(cwd_label, |d, label| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(label)),
                        )
                    })
                    .child(div().flex_grow(1.0))
                    // 準備中は「待たずに中身を見る」逃げ道を必ず用意する（#720）。
                    // チャットヘッダの同名ボタンと同じ揮発解除の経路
                    .when(show_terminal_button, |d| {
                        d.child(
                            div()
                                .id(("gui-pane-to-terminal", pane_id.as_u64()))
                                .flex()
                                .flex_none()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .cursor_pointer()
                                .border_1()
                                .border_color(hsla(theme.border_subtle))
                                .text_color(hsla(theme.text_secondary))
                                .hover(|d| {
                                    d.bg(rgba(theme.surface_hover))
                                        .border_color(hsla(theme.border_default))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation()
                                    }),
                                )
                                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.starter_action(pane_id, StarterAction::UseTerminal, cx);
                                }))
                                .child(
                                    svg()
                                        .path(ui_icon::PROMPT)
                                        .w(px(11.0))
                                        .h(px(11.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_secondary)),
                                )
                                .child(SharedString::from(txt::chat_show_terminal().to_string())),
                        )
                    })
                    .child(
                        div()
                            .id(("starter-close", pane_id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.25)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation()
                                }),
                            )
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_pane_with_confirm(
                                    pane_id,
                                    event.modifiers().platform,
                                    CloseOrigin::PaneButton,
                                    cx,
                                );
                            }))
                            .child(
                                svg()
                                    .path(ui_icon::CLOSE)
                                    .w(px(13.0))
                                    .h(px(13.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    ),
            )
    }

    /// 準備中プレースホルダ（Issue #720）。ペインを作った直後・エージェントを起動した
    /// 直後の「表示種別が決まっていない」数秒を覆う。
    ///
    /// **上限つきの過渡期（`tako_core::ui_mode::SettleState`）でしか出ない**ので、
    /// ここが出っぱなしになることはない。加えてヘッダの「ターミナルを表示」で
    /// いつでも中身へ抜けられる（起動が失敗して固まったときの逃げ道）
    pub(crate) fn render_preparing_pane(
        &mut self,
        pane_id: PaneId,
        kind: SettleKind,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        let width = f32::from(area.size.width);
        let height = f32::from(area.size.height);
        let compact = width < 420.0 || height < 260.0;

        // 明滅する点（チャットの busy ドット・タブバーの実行中ドットと同じ表現）。
        // 新しい UI 表現を増やさないのと、**アニメーションが毎フレーム再描画を要求する**
        // ので、猶予が切れた瞬間に何もしなくても通常表示へ入れ替わる
        let pulse = div()
            .w(px(9.0))
            .h(px(9.0))
            .flex_none()
            .rounded_full()
            .bg(hsla(theme.accent))
            .with_animation(
                ("pane-preparing-pulse", pane_id.as_u64()),
                Animation::new(Duration::from_secs(2)).repeat(),
                |el, t| el.opacity(1.0 - 0.7 * (std::f32::consts::PI * t).sin()),
            );

        let detail = match kind {
            SettleKind::Shell => txt::preparing_shell(),
            SettleKind::Agent => txt::preparing_agent(),
        };

        self.render_gui_pane_frame(pane_id, rect, focused, true, cx)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(9.0))
                            .child(pulse)
                            .child(
                                div()
                                    .text_size(px(if compact { 13.0 } else { 15.0 }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(txt::preparing_title().to_string())),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(detail.to_string())),
                    ),
            )
    }

    /// スターター表示のペイン 1 枚（ターミナルペインと同じ枠 + 3 カード + setup リンク）
    pub(crate) fn render_starter_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: gpui::Bounds<gpui::Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        // 狭いペインでは説明文とコマンド併記を落とす（見切れさせない。#185 と同方針）
        let width = f32::from(area.size.width);
        let height = f32::from(area.size.height);
        let compact = width < 420.0 || height < 260.0;
        let very_compact = width < 300.0 || height < 190.0;

        // #739: プロファイル選択 ▾ を出すかの材料（TTL つきキャッシュ。読み直しは 2 秒 tick）
        let profiles = self.starter_profiles.clone();
        // 合成マウス用の座標記録先（`self` を閉じ込めないよう先に取り出す）
        #[cfg(feature = "visual-test")]
        let chevron_slot = self.starter_chevron_bounds.clone();

        let card = |action: StarterRow, cx: &mut Context<Self>| -> gpui::AnyElement {
            let StarterRow {
                action,
                icon,
                title,
                desc,
                command,
                primary,
            } = action;
            let t = theme.clone();
            // #739: カードは「既定で起動する本体」と「▾ でプロファイルを選ぶ」の
            // 2 領域に割る（Code Runner #453 と同じスプリットボタン）。**入れ子にしない**
            // ので、押下がどちらの操作になるかが構造で決まる（伝播の取り合いが起きない）
            let has_choice = profiles.has_choice(action);
            let body_radius = px(if has_choice { 0.0 } else { 10.0 });
            div()
                .flex()
                .flex_row()
                .items_stretch()
                .w_full()
                // 説明が 2 行に折り返してもカードが潰れて隣と重ならないようにする
                // （flex の自動最小サイズに頼れないので明示する。#656 と同じ罠）
                .flex_shrink_0()
                .rounded(px(10.0))
                .border_1()
                // 枠は外側が持つ（本体と ▾ の境目に二重線を作らない）。
                // ホバーで枠が明るくなる従来の見え方は、外側のホバーで保つ
                .when(primary, |d| d.border_color(hsla_alpha(theme.accent, 0.55)))
                .when(!primary, |d| {
                    let t = t.clone();
                    d.border_color(hsla(t.border_subtle))
                        .hover(move |d| d.border_color(hsla(t.border_default)))
                })
                .child(
                    div()
                        .id((
                            "starter-card",
                            (pane_id.as_u64() << 4) | action_index(action),
                        ))
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(14.0))
                        .py(if compact { px(10.0) } else { px(13.0) })
                        .rounded_l(px(10.0))
                        .rounded_r(body_radius)
                        .cursor_pointer()
                        .when(primary, |d| {
                            let t = t.clone();
                            d.bg(rgba_alpha(t.accent, 0.14))
                                .hover(move |d| d.bg(rgba_alpha(t.accent, 0.24)))
                        })
                        .when(!primary, |d| {
                            let t = t.clone();
                            d.bg(rgba(t.surface_1))
                                .hover(move |d| d.bg(rgba(t.surface_hover)))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.starter_action(pane_id, action, cx);
                        }))
                        .child(
                            div()
                                .w(px(30.0))
                                .h(px(30.0))
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.0))
                                .bg(rgba(if primary {
                                    theme.surface_2
                                } else {
                                    theme.chip_surface
                                }))
                                .child(svg().path(icon).w(px(15.0)).h(px(15.0)).text_color(hsla(
                                    if primary {
                                        theme.accent
                                    } else {
                                        theme.text_secondary
                                    },
                                ))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(7.0))
                                        .child(
                                            div()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(hsla(theme.foreground))
                                                .child(SharedString::from(title)),
                                        )
                                        // 実コマンドの併記（初心者の学習経路。#322 の最簡形）
                                        .when_some(
                                            command.filter(|_| !compact),
                                            |d, command: &'static str| {
                                                d.child(
                                                    div()
                                                        .flex_none()
                                                        .font_family(theme.font_family.clone())
                                                        .text_size(px(10.0))
                                                        .text_color(hsla(theme.text_faint))
                                                        .child(command),
                                                )
                                            },
                                        ),
                                )
                                .when(!very_compact, |d| {
                                    d.child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(hsla(theme.text_secondary))
                                            .child(SharedString::from(desc)),
                                    )
                                }),
                        ),
                )
                // #739: プロファイル選択 ▾（選択肢が 2 つ以上のときだけ）
                .when(has_choice, |d| {
                    let t = t.clone();
                    d.child(
                        div()
                            .id((
                                "starter-card-profiles",
                                (pane_id.as_u64() << 4) | action_index(action),
                            ))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(30.0))
                            .rounded_r(px(10.0))
                            .cursor_pointer()
                            .border_l_1()
                            .border_color(hsla(if primary {
                                t.border_default
                            } else {
                                t.border_subtle
                            }))
                            .when(primary, |d| {
                                let t = t.clone();
                                d.bg(rgba_alpha(t.accent, 0.14))
                                    .hover(move |d| d.bg(rgba_alpha(t.accent, 0.30)))
                            })
                            .when(!primary, |d| {
                                let t = t.clone();
                                d.bg(rgba(t.surface_1))
                                    .hover(move |d| d.bg(rgba(t.surface_hover)))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_starter_profile_menu(
                                        pane_id,
                                        action,
                                        event.position,
                                        cx,
                                    );
                                }),
                            )
                            // #739: 合成マウスで実際に ▾ を押すための座標記録
                            // （absolute なのでレイアウトは変わらない。#718 と同方式）
                            .child({
                                #[cfg(feature = "visual-test")]
                                {
                                    let slot = chevron_slot.clone();
                                    let record = matches!(action, StarterAction::Master);
                                    gpui::canvas(
                                        |_, _, _| (),
                                        move |bounds, _, _, _| {
                                            if record {
                                                slot.set(Some(bounds));
                                            }
                                        },
                                    )
                                    .absolute()
                                    .size_full()
                                    .into_any_element()
                                }
                                #[cfg(not(feature = "visual-test"))]
                                {
                                    gpui::Empty.into_any_element()
                                }
                            })
                            .child(
                                svg()
                                    .path(ui_icon::CHEVRON_DOWN)
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .text_color(hsla(theme.text_secondary)),
                            ),
                    )
                })
                .into_any_element()
        };

        self.render_gui_pane_frame(pane_id, rect, focused, false, cx)
            // 本体: 見出し + カード 3 枚（縦積み・中央寄せ）
            .child(
                div()
                    .id(("starter-body", pane_id.as_u64()))
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(if compact { px(8.0) } else { px(10.0) })
                    .px(px(16.0))
                    .py(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(460.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(if compact { 14.0 } else { 16.0 }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(txt::starter_title().to_string())),
                            )
                            .when(!very_compact, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(hsla(theme.text_muted))
                                        .child(SharedString::from(
                                            txt::starter_subtitle().to_string(),
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(460.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .children(starter_rows().into_iter().map(|row| card(row, cx))),
                    )
                    // 下部の控えめなリンク行（#720）。カードと同格の主要導線にはしない
                    // （初回バナー #549 と役割が重なるため）。実行方式はカードと同じ
                    // シェル書き込みで、AI 側の等価操作は既存の `tako setup`
                    .when(!very_compact, |d| {
                        d.child(
                            div()
                                .id(("starter-setup-link", pane_id.as_u64()))
                                .w_full()
                                .max_w(px(460.0))
                                .flex_shrink_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(4.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.text_muted))
                                .hover(|d| {
                                    d.bg(rgba(theme.surface_hover))
                                        .text_color(hsla(theme.text_secondary))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation()
                                    }),
                                )
                                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.starter_action(pane_id, StarterAction::Setup, cx);
                                }))
                                // 「やり直す」= 既存の循環矢印（#438）。UI に絵文字は使わない
                                .child(
                                    svg()
                                        .path(ui_icon::REFRESH)
                                        .w(px(12.0))
                                        .h(px(12.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_muted)),
                                )
                                .child(SharedString::from(txt::starter_setup_link().to_string()))
                                .when(!compact, |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .font_family(theme.font_family.clone())
                                            .text_size(px(10.0))
                                            .text_color(hsla(theme.text_faint))
                                            .child(txt::CARD_SETUP_COMMAND),
                                    )
                                }),
                        )
                    })
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .w_full()
                                .max_w(px(460.0))
                                .flex_shrink_0()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(txt::starter_footnote().to_string())),
                        )
                    }),
            )
    }

    // --- プロファイル選択ドロップダウン（#739 / G4） ---------------------

    /// ▾ の開閉。同じカードの ▾ をもう一度押したら閉じる（Code Runner #453 と同じ）
    pub(crate) fn toggle_starter_profile_menu(
        &mut self,
        pane_id: PaneId,
        action: StarterAction,
        anchor: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .starter_profile_menu
            .as_ref()
            .is_some_and(|(pane, act, _)| *pane == pane_id && *act == action);
        self.starter_profile_menu = if same {
            None
        } else {
            Some((pane_id, action, anchor))
        };
        cx.notify();
    }

    /// スターターが画面に出ていて、プロファイル一覧が未読込 / TTL 切れか（#739）。
    ///
    /// **スターターが 1 枚も出ていないときは false** を返すので、通常のターミナル
    /// 利用中はディスクを触らない（新しい常駐ポーリングを作らない。#340 の方針）
    pub(crate) fn starter_profiles_stale(&self) -> bool {
        if !self.ui_mode.is_gui() {
            return false;
        }
        if self
            .starter_profiles_at
            .is_some_and(|at| at.elapsed() < STARTER_PROFILES_TTL)
        {
            return false;
        }
        self.workspace
            .tabs()
            .iter()
            .flat_map(|tab| tab.tree().panes())
            .any(|pane| {
                self.pane_display_for(pane.id()) == tako_core::ui_mode::PaneDisplay::Starter
            })
    }

    /// 読み込み結果の反映。中身が変わったときだけ再描画を求める
    pub(crate) fn apply_starter_profiles(&mut self, loaded: StarterProfiles) -> bool {
        self.starter_profiles_at = Some(std::time::Instant::now());
        if self.starter_profiles == loaded {
            return false;
        }
        self.starter_profiles = loaded;
        true
    }

    /// ドロップダウン本体（ルート直下のオーバーレイ。#361 / #453 と同じ方式）。
    ///
    /// 画面外へはみ出さないよう、ビューポート実寸で幅・左端・高さを詰め、
    /// 下に入らないときは上へ反転する（#346 のコンテキストメニューと同じ規則）
    pub(crate) fn render_starter_profile_menu_overlay(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (pane_id, action, anchor) = self.starter_profile_menu?;
        let profiles = self.starter_profiles.for_action(action);
        if profiles.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        use crate::ui_text::ui_mode as txt;

        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let menu_w = PROFILE_MENU_WIDTH.min((vw - 16.0).max(160.0));
        let left = (f32::from(anchor.x) - menu_w / 2.0).clamp(8.0, (vw - menu_w - 8.0).max(8.0));
        // 下に入らなければ上へ反転する（狭い / 低いウィンドウでも全項目へ届く）
        let below = vh - f32::from(anchor.y) - 16.0;
        let (top, max_h) = if below >= 160.0 {
            (
                f32::from(anchor.y) + 6.0,
                below.min(PROFILE_MENU_MAX_HEIGHT),
            )
        } else {
            let above = (f32::from(anchor.y) - 16.0).max(120.0);
            let height = above.min(PROFILE_MENU_MAX_HEIGHT);
            ((f32::from(anchor.y) - 6.0 - height).max(8.0), height)
        };

        let rows: Vec<gpui::AnyElement> = profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                let name = profile.name.clone();
                let summary = profile.summary.clone();
                // 併記する実コマンド（学習経路。既定は引数なしの最簡形。#322）
                let command = starter_subcommand(action, Some(&name))
                    .map(|sub| format!("tako {sub}"))
                    .unwrap_or_default();
                div()
                    .id(("starter-profile-row", index as u64))
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(1.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.starter_profile_menu = None;
                        this.starter_action_with_profile(pane_id, action, Some(&name), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(7.0))
                            // 名前が主役。**名前は縮めず**、併記する実コマンドの側を
                            // 縮める（逆にすると `visual-prof…` のように名前だけが
                            // 削れて、どれを選ぶのか分からなくなる。実測して直した）
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(170.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(profile.label())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(theme.font_family.clone())
                                    .text_size(px(9.5))
                                    .text_color(hsla(theme.text_faint))
                                    .child(SharedString::from(command)),
                            ),
                    )
                    .when(!summary.is_empty(), |d| {
                        d.child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(summary)),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .id("starter-profile-dismiss")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                        this.starter_profile_menu = None;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("starter-profile-menu")
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(menu_w))
                        .max_h(px(max_h))
                        .overflow_y_scroll()
                        // 件数が上限高さを超えたら中でスクロールする
                        // （画面外へ伸ばさない = 下の項目に必ず手が届く）
                        .track_scroll(&self.starter_profile_scroll)
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_default))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .p(px(6.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .flex_shrink_0()
                                .px(px(10.0))
                                .py(px(3.0))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(
                                    txt::starter_profile_menu_title().to_string(),
                                )),
                        )
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}

/// 1 枚のカードの材料（描画に必要なものだけ。文言は `ui_text::ui_mode` が正）
struct StarterRow {
    action: StarterAction,
    icon: &'static str,
    title: String,
    desc: String,
    command: Option<&'static str>,
    primary: bool,
}

fn starter_rows() -> Vec<StarterRow> {
    use crate::ui_text::ui_mode as txt;
    vec![
        StarterRow {
            action: StarterAction::Master,
            icon: ui_icon::ORCH,
            title: txt::card_master_title().to_string(),
            desc: txt::card_master_desc().to_string(),
            command: Some(txt::CARD_MASTER_COMMAND),
            primary: true,
        },
        StarterRow {
            action: StarterAction::Solo,
            icon: ui_icon::CHAT_BUBBLE,
            title: txt::card_solo_title().to_string(),
            desc: txt::card_solo_desc().to_string(),
            command: Some(txt::CARD_SOLO_COMMAND),
            primary: false,
        },
        StarterRow {
            action: StarterAction::UseTerminal,
            icon: ui_icon::PROMPT,
            title: txt::card_terminal_title().to_string(),
            desc: txt::card_terminal_desc().to_string(),
            command: None,
            primary: false,
        },
    ]
}

/// 要素 ID の安定した連番（同一ペイン内でカードごとに違う ID にする）
fn action_index(action: StarterAction) -> u64 {
    match action {
        StarterAction::Master => 0,
        StarterAction::Solo => 1,
        StarterAction::UseTerminal => 2,
        // setup はカードではなく下部リンク（専用 ID を持つ）ので、
        // ここに来るのは将来カード化したときだけ
        StarterAction::Setup => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// カードは仕様の 3 枚がこの順で並ぶ（master が主 = 既定の一手）
    #[test]
    fn カードは3枚で順序と起動コマンドが仕様どおり() {
        let rows = starter_rows();
        assert_eq!(rows.len(), 3);
        let actions: Vec<StarterAction> = rows.iter().map(|r| r.action).collect();
        assert_eq!(
            actions,
            vec![
                StarterAction::Master,
                StarterAction::Solo,
                StarterAction::UseTerminal
            ]
        );
        assert!(rows[0].primary, "AI チームに任せるが主ボタン");
        assert_eq!(rows[0].command, Some("tako master"));
        assert_eq!(rows[1].command, Some("tako solo"));
        // 「コマンド入力へ」は何も起動しないのでコマンド併記も無い
        assert_eq!(rows[2].command, None);
        assert_eq!(rows[2].action.subcommand(), None);
        // 要素 ID がカードごとに衝突しない
        let mut ids: Vec<u64> = actions.iter().map(|a| action_index(*a)).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    /// #739: ▾ から選んだプロファイルが起動コマンドになる。
    /// 既定は引数なしの最簡形（#322）で、CLI の案内文と同じ規則を通る
    #[test]
    fn プロファイル指定の起動コマンド() {
        let sub = |action, profile| starter_subcommand(action, profile);
        assert_eq!(sub(StarterAction::Master, None), Some("master".into()));
        assert_eq!(
            sub(StarterAction::Master, Some("default")),
            Some("master".into()),
            "既定は引数を付けない（#322 の最簡形）"
        );
        assert_eq!(
            sub(StarterAction::Master, Some("work")),
            Some("master -work".into())
        );
        assert_eq!(
            sub(StarterAction::Solo, Some("fast")),
            Some("solo -fast".into())
        );
        // 起動しないカードにはコマンドが無い
        assert_eq!(sub(StarterAction::UseTerminal, Some("work")), None);
    }

    /// **シェルへ書き込む文字列**なので、名前の検証を通らないものは起動しない。
    /// プロファイルディレクトリのファイル名は誰でも作れるため、ここが最後の砦
    #[test]
    fn 不正なプロファイル名は起動コマンドにならない() {
        for bad in [
            "$(touch /tmp/pwned)",
            "a b",
            "../evil",
            "-tab",
            ".hidden",
            "",
            "a;rm -rf x",
            "a'b",
        ] {
            assert_eq!(
                starter_subcommand(StarterAction::Master, Some(bad)),
                None,
                "'{bad}' は拒否されるべき"
            );
        }
    }

    /// ▾ は選択肢が 2 つ以上のときだけ出す（既定 1 件だけならノイズ）
    #[test]
    fn シェブロンは選択肢が複数のときだけ出す() {
        let profile = |name: &str| StarterProfile {
            name: name.to_string(),
            summary: String::new(),
        };
        let only_default = StarterProfiles {
            master: vec![profile("default")],
            solo: vec![profile("default")],
        };
        assert!(!only_default.has_choice(StarterAction::Master));
        let with_extra = StarterProfiles {
            master: vec![profile("default"), profile("work")],
            solo: vec![profile("default")],
        };
        assert!(with_extra.has_choice(StarterAction::Master));
        assert!(!with_extra.has_choice(StarterAction::Solo));
        // 起動しないカードは常に選択肢なし
        assert!(!with_extra.has_choice(StarterAction::UseTerminal));
        assert!(with_extra.for_action(StarterAction::Setup).is_empty());
    }

    /// 一覧に添える手がかりは projects → cwd → モデルの順で 1 つだけ
    #[test]
    fn プロファイルの手がかりは1行にまとまる() {
        use tako_control::orchestrator::Profile;
        // ① 担当プロジェクト（4 件目以降は件数へ畳む）
        let mut p = Profile {
            projects: Some(vec![
                "alpha".into(),
                "beta".into(),
                "gamma".into(),
                "delta".into(),
            ]),
            ..Profile::default()
        };
        let projects = profile_summary(&p);
        assert!(projects.contains("alpha / beta / gamma"), "{projects}");
        assert!(!projects.contains("delta"), "4 件目は畳む: {projects}");
        // ② projects が無ければ起動フォルダ
        p.projects = None;
        p.cwd = Some("/tmp/tako-summary".into());
        assert_eq!(profile_summary(&p), "/tmp/tako-summary");
        // ③ どちらも無ければモデル表示（空文字にはしない）
        p.cwd = None;
        assert!(!profile_summary(&p).is_empty());
    }

    /// 既定エントリは `default.yaml` が無くても必ず先頭に並ぶ
    /// （`tako master` は引数なしで既定プロファイルを作って起動するため）
    #[test]
    fn 既定は一覧の先頭に必ず並ぶ() {
        let profiles = load_starter_profiles();
        for list in [&profiles.master, &profiles.solo] {
            assert_eq!(list.first().map(|p| p.name.as_str()), Some("default"));
            // 名前の重複が無い（既定を二重に出さない）
            let mut names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
            let total = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), total, "プロファイル名が重複している");
        }
    }
}

//! タブバー — Claude Design カンプ準拠のピル型タブ（Issue #217）
//!
//! カンプ: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html` の
//! tab-bar セクション。高さ 44px / ピル型タブ（h30・radius 8）/ ペイン状態
//! ミニインジケータ / fail 数表示 / ⌘K 検索エントリ / 通知ベル + バッジ /
//! テーマ切替ボタン。traffic lights はタイトルバー統合（native）で同居する。
//!
//! オーバーフロー対応（Issue #208）: タブ数に応じてラベルを縮小 + GPUI
//! ScrollHandle で横スクロール + アクティブタブの自動スクロールイン。
//!
//! # ヒットテストの不変条件（Issue #576。Windows でボタンが死ぬ）
//!
//! タブバー根 div は `window_control_area(WindowControlArea::Drag)` を張っている（#312）。
//! GPUI の `Window::hit_test` は hitbox を手前から走査して `HitboxBehavior::BlockMouse`
//! （= `occlude()`）で `break` するため、**子が occlude していないと祖先（= 根 div）の
//! hitbox まで hit test に積まれ**、`on_hit_test_window_control` が Drag を返す。
//! Windows の `WM_NCHITTEST` はこれを `HTCAPTION` に変換するので、子ボタンの上でも
//! mouse down が `DefWindowProc` のウィンドウ移動モーダルループに食われ、mouse up が
//! アプリに届かず click が成立しない（macOS は `on_hit_test_window_control` が空実装の
//! ため同じコードでも壊れない = Windows 固有）。
//!
//! したがって **タブバー上の対話要素は必ず `.occlude()` すること**。
//! 逆に `tab-scroll-area` には付けない —— `flex_1` で空き領域の大半を占めるため、
//! 付けるとタブバー空き領域でのウィンドウドラッグ移動（#312）が死ぬ。
//!
//! # スクロール領域の中で occlude するときの追加規約（Issue #961）
//!
//! `occlude()` は hit test を **break** させるので、祖先である `tab-scroll-area` の
//! hitbox が `mouse_hit_test.ids` から落ちる。GPUI の `overflow_x_scroll` は
//! `hitbox.should_handle_scroll()`（= `ids.contains`）で発火を決めるため、
//! **タブピルの上ではホイールがスクロール領域へ一切届かない**。#576 でピルへ
//! `occlude()` を付けた結果、#208 の横スクロールが丸ごと死んでいた（#961）。
//!
//! そこで **スクロール領域の中で occlude する要素は
//! [`TabScrollOcclude::occlude_scrolling`] を使い、ホイールを自分で中継する**こと。
//! 素の `.occlude()` を使うと同じ穴が開く（番犬テストが名指しで落とす）。
//!
//! # ウィンドウコントロール（Issue #584 → #657 でこの行から移設）
//!
//! Windows は `hide_title_bar` でネイティブのキャプションボタンが生成されないため
//! 自前で描くが、**置き場所は一段上の in-window メニューバー行**（`menu_bar.rs`）。
//! VSCode と同じく「メニュー群 … ウィンドウコントロール」を 1 行に収める形にした。
//! この行に残る macOS 専用の要素は左端の `TRAFFIC_LIGHTS_SPACER`（native traffic
//! lights が載る余白）だけ。

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Context, Div,
    DragMoveEvent, FontWeight, Pixels, ScrollDelta, ScrollWheelEvent, SharedString, Stateful,
    WindowControlArea,
};
use tako_core::{CommandState, TitleSource};

use super::*;
use crate::file_icons::ui_icon;

/// traffic lights（12px × 3 + gap 8px × 2 = 52px）+ 右余白 16px。
/// native traffic lights を持つのは macOS だけなので、他プラットフォームでは
/// 意味のない左端余白になる（#584）。0 にして左端からタブを並べる
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_SPACER: f32 = 68.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHTS_SPACER: f32 = 0.0;

/// 1 タブのラベル込みの参考幅（px）。ラベル truncate 上限を決めるために使う概算値。
/// 実測: dot(7) + gap(8) + pl(10) + label + pr(11) + gap(3)。
/// ラベル 1 文字あたり約 7px（12.5px フォントの平均グリフ幅）
const TAB_CHROME_PX: f32 = 42.0;
const CHAR_WIDTH_PX: f32 = 7.0;
/// タブラベルの最大文字数（通常時）
const LABEL_MAX_CHARS: usize = 24;
/// タブラベルの最小文字数（縮小限界）
const LABEL_MIN_CHARS: usize = 6;
/// 実行中ドットの脈動（#217）を「走り始めの合図」に限る回数（Issue #945）。
///
/// 1 回 = [`DOT_PULSE_PERIOD`]（不透明度 1.0 → 0.35 → 1.0 の 1 往復）。
/// 3 回 = 6 秒で、そのあとは色だけの静的表示になる
const DOT_PULSE_COUNT: u32 = 3;
/// 脈動 1 往復ぶんの長さ（#217 から不変。合計は [`DOT_PULSE_COUNT`] 倍）
const DOT_PULSE_PERIOD: Duration = Duration::from_secs(2);

/// 実行中ドットの不透明度（`t` は脈動全体の進捗 0.0〜1.0）。
///
/// [`DOT_PULSE_COUNT`] 回ぶんの正弦の山を並べたもの。**両端がちょうど 1.0** に
/// なるので、脈動が終わった瞬間に不透明度が飛ばない（#945）
fn tab_dot_opacity(t: f32) -> f32 {
    let phase = std::f32::consts::PI * t * DOT_PULSE_COUNT as f32;
    1.0 - 0.65 * phase.sin().abs()
}

/// 脈動が最後に計算した不透明度（#945 の検証用。f32 のビット列。初期値 = 1.0）
static DOT_PULSE_LAST_OPACITY: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3f80_0000);
/// 脈動のフレームを計算した回数（同上。単調増加）
static DOT_PULSE_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 脈動が 1 フレーム計算されたことを記録する（引数はそのまま返す）。
///
/// GPUI の `AnimationElement` は**アニメーションが終わっていないフレームだけ**
/// `request_animation_frame()` を呼ぶので、「時間を空けて描き直しても不透明度が
/// 動かない」= 完了していて**フレーム要求も止まっている**、と言い切れる。
/// 画面（ディスプレイリンク）の有無に依存せず A/B が取れるのでこの形にした
fn record_dot_opacity(opacity: f32) -> f32 {
    use std::sync::atomic::Ordering::Relaxed;
    DOT_PULSE_LAST_OPACITY.store(opacity.to_bits(), Relaxed);
    DOT_PULSE_FRAMES.fetch_add(1, Relaxed);
    opacity
}

/// 脈動の観測値（計算フレーム数, 最後の不透明度）。セルフテスト項目 128（#945）用
pub(crate) fn dot_pulse_probe() -> (u64, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        DOT_PULSE_FRAMES.load(Relaxed),
        f32::from_bits(DOT_PULSE_LAST_OPACITY.load(Relaxed)),
    )
}

/// 脈動 1 巡の長さ（セルフテストが待ち時間をここから作るための公開。#945）
pub(crate) fn dot_pulse_total() -> Duration {
    DOT_PULSE_PERIOD * DOT_PULSE_COUNT
}

/// 脈動を #945 前（2 秒周期の無限 repeat）へ戻す逃げ道（`TAKO_945_LEGACY=1`）。
///
/// 同じバイナリで「フレーム要求が止まるか」を A/B するために使う。
/// 既定は有効（未設定 = 有限回で終わる）
fn dot_pulse_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_945_LEGACY").is_some())
}

/// タブバーの横スクロール量をホイールの delta から決める（Issue #961）。
///
/// GPUI の `overflow_x_scroll` と**同じ意味論**にする: 横 delta があればそれを使い、
/// 無ければ縦 delta を横へ回す（`restrict_scroll_to_axis` 既定 false・縦は非スクロール）。
/// ここがずれると「ピルの上」と「タブの隙間」でスクロール量が食い違う
fn tab_scroll_dx(delta: &ScrollDelta, line_height: Pixels) -> Pixels {
    let d = delta.pixel_delta(line_height);
    if d.x == Pixels::ZERO {
        d.y
    } else {
        d.x
    }
}

/// ホイール中継を #961 前（素の `occlude()`）へ戻す逃げ道（`TAKO_961_LEGACY=1`）。
///
/// 同じバイナリで「タブピルの上でスクロールできるか」を A/B するために使う
fn tab_scroll_relay_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_961_LEGACY").is_some())
}

/// スクロール領域（`tab-scroll-area`）の**中**で `occlude()` する要素に付ける（#576 + #961）。
///
/// `occlude()` だけだと祖先のスクロール領域が hit test から落ちてホイールが死ぬので、
/// 自分で受けて [`TakoApp::scroll_tab_bar_by`] へ中継する。
/// スクロール領域の**外**（⌘K / ベル / 表示モード / テーマ）は素の `occlude()` でよい
pub(crate) trait TabScrollOcclude: Sized {
    fn occlude_scrolling(self, cx: &Context<TakoApp>) -> Self;
}

impl TabScrollOcclude for Stateful<Div> {
    fn occlude_scrolling(self, cx: &Context<TakoApp>) -> Self {
        let el = self.occlude();
        if tab_scroll_relay_disabled() {
            return el;
        }
        el.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
            this.scroll_tab_bar_by(&event.delta, window.line_height(), cx);
        }))
    }
}

/// 右端コントロール群の概算幅
/// （⌘K(210+px) + bell(30) + ui-mode(30) + theme(30) + gap + margin。#694 で +30）
const RIGHT_CONTROLS_PX: f32 = 330.0;

/// 1 行のヒント表示（#694 のツールチップ用の最小ビュー）。
/// GPUI のツールチップは `AnyView` を要求するので、ドラッグゴースト（`DragGhost`）と
/// 同じ「小さな Render 実装」パターンで用意する
pub(crate) struct HintTooltip {
    label: String,
    theme: Theme,
}

impl HintTooltip {
    pub(crate) fn new(label: String, theme: Theme) -> Self {
        Self { label, theme }
    }
}

impl Render for HintTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgba(self.theme.surface_2))
            .border_1()
            .border_color(hsla(self.theme.border_default))
            .text_size(px(11.0))
            .text_color(hsla(self.theme.foreground))
            .child(SharedString::from(self.label.clone()))
    }
}

impl TakoApp {
    /// タブ数と利用可能幅からラベルの truncate 上限文字数を決定する
    fn tab_label_max_chars(&self, tab_count: usize, window: &Window) -> usize {
        if tab_count == 0 {
            return LABEL_MAX_CHARS;
        }
        let vw = f32::from(window.viewport_size().width);
        // ウィンドウコントロールは一段上のメニューバー行へ移った（#657）ので、
        // この行の右端で場所を取るのは ⌘K / ベル / テーマだけ
        let available = vw - TRAFFIC_LIGHTS_SPACER - RIGHT_CONTROLS_PX - 40.0;
        let per_tab = available / tab_count as f32;
        let label_px = (per_tab - TAB_CHROME_PX).max(0.0);
        let chars = (label_px / CHAR_WIDTH_PX) as usize;
        chars.clamp(LABEL_MIN_CHARS, LABEL_MAX_CHARS)
    }

    /// アクティブタブが表示領域に入るよう ScrollHandle を更新する。
    /// タブ切替を行うすべての経路（クリック・⌘数字・CLI/MCP）から呼ぶ。
    /// scroll_to_item は子要素インデックスで動くため、タブバーの表示と同じ
    /// 全タブの並びで位置を計算する（#380: タブバーは全ウィンドウ共有）
    pub(crate) fn scroll_active_tab_into_view(&self) {
        let active = self.workspace.active_tab_id();
        if let Some(idx) = self.workspace.tabs().iter().position(|t| t.id() == active) {
            self.tab_scroll_handle.scroll_to_item(idx);
        }
    }

    /// タブバーを横スクロールする（Issue #961）。
    ///
    /// `overflow_x_scroll` の hitbox が `occlude()` で落ちる位置（= タブピルの上）から
    /// 中継されてくる。GPUI の既定ハンドラと**同じことをする**: offset を足すだけで
    /// クランプはしない（次の prepaint が `-max_offset..=0` へ丸める）ので、
    /// ピルの上と隙間の上で挙動が一致する
    pub(crate) fn scroll_tab_bar_by(
        &mut self,
        delta: &ScrollDelta,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        let dx = tab_scroll_dx(delta, line_height);
        if dx == Pixels::ZERO {
            return;
        }
        let offset = self.tab_scroll_handle.offset();
        self.tab_scroll_handle
            .set_offset(point(offset.x + dx, offset.y));
        cx.notify();
    }

    /// タブバーの + ボタン: クリックされたウィンドウに新規タブを作る（Issue #339。
    /// 非アクティブウィンドウの + でも activation イベントの順序に依存せず正しく動かす）
    pub(crate) fn new_tab_in_viewport(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(lid) = self.viewport_of(window) {
            if self.workspace.get_window(lid).is_some() && self.workspace.active_window_id() != lid
            {
                let _ = self.workspace.activate_window(lid);
            }
        }
        self.new_tab(cx);
    }

    pub(crate) fn render_tab_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        // タブバーは全ウィンドウ共有（#380）: どのウィンドウにも全タブを描き、
        // 「このウィンドウの表示タブ」だけをアクティブ表示にする。
        // 他ウィンドウで表示中のタブは W バッジで区別する
        let viewport = self
            .viewport_of(window)
            .unwrap_or_else(|| self.workspace.active_window_id());
        let active = self
            .workspace
            .get_window(viewport)
            .map(|w| w.active_tab())
            .unwrap_or_else(|| self.workspace.active_tab_id());

        // アクティブタブが変わった（dispatch / new_tab 等）ら自動スクロールイン。
        // スクロール位置（tab_scroll_handle）は共有のためアクティブウィンドウでのみ追従
        if viewport == self.workspace.active_window_id() && self.last_active_tab != Some(active) {
            self.last_active_tab = Some(active);
            self.scroll_active_tab_into_view();
        }

        let tabs: Vec<_> = self
            .workspace
            .tabs()
            .iter()
            .map(|tab| {
                let id = tab.id();
                let label = if tab.title_source() == TitleSource::Default {
                    tab.tree()
                        .panes()
                        .iter()
                        .find(|p| p.id() == tab.tree().focused())
                        .and_then(|p| self.terminals.get(&p.id()))
                        .and_then(|s| s.title())
                        .unwrap_or(tab.title())
                        .to_string()
                } else {
                    tab.title().to_string()
                };
                let pane_states: Vec<CommandState> = tab
                    .tree()
                    .panes()
                    .iter()
                    .filter_map(|p| self.terminals.get(&p.id()))
                    .map(|s| s.command_state())
                    .collect();
                let agg = CommandState::aggregate(pane_states.iter().cloned());
                let fails = pane_states
                    .iter()
                    .filter(|s| matches!(s, CommandState::Failed(_)))
                    .count();
                // 他ウィンドウで表示中のタブに出す区別バッジ（#380。W<番号>）
                let shown_in = self
                    .workspace
                    .windows()
                    .iter()
                    .find(|w| w.id() != viewport && w.active_tab() == id)
                    .map(|w| w.id().as_u64());
                // 自動命名の直後だけ出す「この名前を固定」の印（#552 案 4）
                let pin_hint =
                    tab.title_source() == TitleSource::Auto && self.auto_title_hint_active(id);
                (id, label, agg, pane_states, fails, shown_in, pin_hint)
            })
            .collect();
        let attention: usize = tabs.iter().map(|(_, _, _, _, fails, _, _)| fails).sum();
        let state_color = |state: &CommandState| match state {
            CommandState::Failed(_) => theme.red,
            CommandState::Running => theme.accent,
            CommandState::Idle => theme.green,
            CommandState::Unknown => theme.text_overlay,
        };

        let label_max = self.tab_label_max_chars(tabs.len(), window);
        let tab_drop = self.tab_drop_target;
        let is_pane_dragging = self.drag_kind == Some(DragKind::Pane);
        let tab_reorder = self.tab_reorder_indicator;
        let is_tab_dragging = self.drag_kind == Some(DragKind::Tab);
        let dragging_tab_id = self.dragging_tab;

        div()
            .id("tab-bar")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .pl(px(16.0))
            .pr(px(12.0))
            .bg(rgba(theme.mantle))
            .border_b_1()
            .border_color(hsla(theme.border_subtle))
            .window_control_area(WindowControlArea::Drag)
            // macOS: タブバー空き領域のドラッグでウインドウ移動（#312）。
            // GPUI の WindowControlArea::Drag は hitbox 登録のみ。macOS では
            // on_hit_test_window_control が空実装のため、Zed と同じく
            // mouse_down → mouse_move で start_window_move() を明示呼び出しする
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = true;
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = false;
                    this.tab_mouse_down = false;
                }),
            )
            // 上の on_mouse_up は hitbox の hover ゲート付きなので、occlude した子
            //（タブピル・各ボタン。#576）の上で離すと発火しない。「mouse up は必ず
            // 押下フラグを解除する」不変条件を保つため out 側でも同じ解除を行う。
            // これが無いと tab_mouse_down が立ちっぱなしになり、タブをクリックした後の
            // 空き領域ドラッグでウィンドウ移動（#312 / #308）が効かなくなる
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = false;
                    this.tab_mouse_down = false;
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.titlebar_dragging = false;
                this.tab_mouse_down = false;
            }))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.titlebar_dragging && !this.tab_mouse_down {
                    this.titlebar_dragging = false;
                    window.start_window_move();
                }
            }))
            // ダブルクリックでズーム（macOS 標準操作。#312）
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            // native traffic lights の載る領域（macOS のみ。#584）
            .when(TRAFFIC_LIGHTS_SPACER > 0.0, |d| {
                d.child(div().w(px(TRAFFIC_LIGHTS_SPACER)).h_full().flex_none())
            })
            // タブ領域（横スクロール対応。Issue #208）
            // scroll_to_item が直接子要素のインデックスで動作するため、
            // タブを scrollable コンテナの直接子要素にする
            .child(
                div()
                    .id("tab-scroll-area")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll_handle)
                    .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                        this.drop_tab_reorder(drag.tab, None, cx);
                    }))
                    .on_drag_move::<PaneDrag>(cx.listener(
                        |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                            this.set_tab_drop_target(None, cx);
                        },
                    ))
                    .on_drop::<PaneDrag>(cx.listener(|this, drag: &PaneDrag, _, cx| {
                        this.drop_pane_on_tab(drag.pane, None, cx);
                    }))
                    .children(tabs.into_iter().map(
                        |(id, label, agg, pane_states, fails, shown_in, pin_hint)| {
                            let is_active = id == active;
                            let dot_color = state_color(&agg);
                            let pulsing = matches!(agg, CommandState::Running);

                            let dot = div()
                                .w(px(7.0))
                                .h(px(7.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(dot_color))
                                .when(is_active, |d| {
                                    d.shadow(vec![BoxShadow {
                                        color: hsla_alpha(dot_color, 0.7),
                                        offset: point(px(0.), px(0.)),
                                        blur_radius: px(6.0),
                                        spread_radius: px(0.),
                                        inset: false,
                                    }])
                                });
                            // 脈動は「走り始めた」の合図なので有限回で終わらせる（#945）。
                            //
                            // GPUI の `AnimationElement` は動いているあいだ毎フレーム
                            // `request_animation_frame()` を呼ぶ。`repeat()` だと
                            // エージェント（claude / codex）のようにフォアグラウンドで
                            // 走り続けるペインのタブが**永久にフレームを要求し続ける**ので、
                            // #786 / #801 / #803 で削った毎フレームの固定費が復活する。
                            // oneshot なら完了フレームで要求が止まり、以後は色だけの
                            // 静的表示（= 走っていること自体は分かる）になる。
                            //
                            // 走り終われば `pulsing` が false になって要素ごと消えるため、
                            // 次に何かが走り始めたときは element state が作り直されて
                            // 脈動もやり直される（GPUI は描かれなかった element state を捨てる）
                            let dot = if pulsing && dot_pulse_legacy() {
                                dot.with_animation(
                                    ("tab-dot-pulse", id.as_u64()),
                                    Animation::new(DOT_PULSE_PERIOD).repeat(),
                                    |el, t| {
                                        el.opacity(record_dot_opacity(
                                            1.0 - 0.65 * (std::f32::consts::PI * t).sin(),
                                        ))
                                    },
                                )
                                .into_any_element()
                            } else if pulsing {
                                dot.with_animation(
                                    ("tab-dot-pulse", id.as_u64()),
                                    Animation::new(dot_pulse_total()),
                                    |el, t| el.opacity(record_dot_opacity(tab_dot_opacity(t))),
                                )
                                .into_any_element()
                            } else {
                                dot.into_any_element()
                            };

                            let truncated = truncate(&label, label_max);

                            // タブ D&D 並べ替えの挿入インジケータ（#371）
                            let show_indicator = is_tab_dragging && tab_reorder == Some(Some(id));
                            let is_drag_source = is_tab_dragging && dragging_tab_id == Some(id);

                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .flex_shrink_0()
                                .when(show_indicator, |d| {
                                    d.child(
                                        div()
                                            .w(px(3.0))
                                            .h(px(22.0))
                                            .flex_none()
                                            .rounded(px(1.5))
                                            .bg(hsla(theme.accent))
                                            .shadow(vec![BoxShadow {
                                                color: hsla_alpha(theme.accent, 0.5),
                                                offset: point(px(0.), px(0.)),
                                                blur_radius: px(4.0),
                                                spread_radius: px(0.),
                                                inset: false,
                                            }]),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("tab", id.as_u64()))
                                        .group("tab-pill")
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(8.0))
                                        .h(px(30.0))
                                        .pl(px(10.0))
                                        .pr(px(11.0))
                                        .flex_shrink_0()
                                        .rounded(px(8.0))
                                        .cursor_pointer()
                                        // 根 div の Drag ヒットテストに勝たせる（#576）+
                                        // ホイールをスクロール領域へ中継する（#961）
                                        .occlude_scrolling(cx)
                                        .when(is_drag_source, |d| {
                                            d.opacity(0.4)
                                                .border_1()
                                                .border_color(hsla(theme.border_subtle))
                                                .border_dashed()
                                        })
                                        .when(is_active && !is_drag_source, |d| {
                                            d.bg(rgba(theme.tab_active_background))
                                                .border_1()
                                                .border_color(hsla(theme.border_heavy))
                                                .shadow(vec![BoxShadow {
                                                    color: hsla_alpha(theme.foreground, 0.05),
                                                    offset: point(px(0.), px(1.)),
                                                    blur_radius: px(0.),
                                                    spread_radius: px(0.),
                                                    inset: true,
                                                }])
                                        })
                                        .when(!is_active && !is_drag_source, |d| {
                                            d.hover(|d| d.bg(rgba(theme.surface_hover)))
                                        })
                                        .when(is_pane_dragging && tab_drop == Some(Some(id)), |d| {
                                            d.bg(rgba_alpha(theme.accent, 0.15))
                                                .border_2()
                                                .border_color(hsla(theme.accent))
                                        })
                                        .text_color(if is_active {
                                            hsla(theme.tab_active_foreground)
                                        } else if fails > 0 {
                                            hsla(theme.text_tertiary)
                                        } else {
                                            hsla(theme.tab_inactive_foreground)
                                        })
                                        .text_size(px(12.5))
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(move |this, _, _, _| {
                                                this.tab_mouse_down = true;
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // 共有タブバー（#380）: クリックしたウィンドウへ
                                            // 表示を移す（他ウィンドウ所属なら奪取）
                                            this.select_tab_in_viewport(id, window, cx);
                                        }))
                                        .on_drag(
                                            TabDrag { tab: id },
                                            self.drag_ghost_builder_with_tab(
                                                DragKind::Tab,
                                                truncated.clone(),
                                                Some(id),
                                                cx,
                                            ),
                                        )
                                        .on_drag_move::<TabDrag>(cx.listener(
                                            move |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                                                // GPUI の on_drag_move は capture フェーズで
                                                // 全登録要素に発火するため、自身の bounds 内か
                                                // を明示チェックする（#413）
                                                if !e.bounds.contains(&e.event.position) {
                                                    return;
                                                }
                                                if e.drag(cx).tab == id {
                                                    return;
                                                }
                                                this.set_tab_reorder_indicator(Some(id), cx);
                                            },
                                        ))
                                        .on_drop::<TabDrag>(cx.listener(
                                            move |this, drag: &TabDrag, _, cx| {
                                                this.drop_tab_reorder(drag.tab, Some(id), cx);
                                            },
                                        ))
                                        .on_drag_move::<PaneDrag>(cx.listener(
                                            move |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                                                this.set_tab_drop_target(Some(id), cx);
                                            },
                                        ))
                                        .on_drop::<PaneDrag>(cx.listener(
                                            move |this, drag: &PaneDrag, _, cx| {
                                                this.drop_pane_on_tab(drag.pane, Some(id), cx);
                                            },
                                        ))
                                        .child(dot)
                                        .child(
                                            div()
                                                .font_weight(if is_active {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .child(SharedString::from(truncated)),
                                        )
                                        // 自動命名の直後だけ出る「この名前を固定」の印
                                        // （#552 案 4）。クリックでこの名前が手動名として
                                        // 固定され、以後 自動リネームに書き換えられなくなる。
                                        // 時間（PIN_HINT_TTL）が経てば静かに消える
                                        .when(pin_hint, |d| {
                                            d.child(
                                                div()
                                                    .id(("tab-pin-title", id.as_u64()))
                                                    .w(px(17.0))
                                                    .h(px(17.0))
                                                    .flex()
                                                    .flex_none()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(5.0))
                                                    .cursor_pointer()
                                                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                                    .on_click(cx.listener(
                                                        move |this, _: &gpui::ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.pin_auto_tab_title(id, cx);
                                                        },
                                                    ))
                                                    .child(
                                                        svg()
                                                            .path(ui_icon::PIN)
                                                            .w(px(11.0))
                                                            .h(px(11.0))
                                                            .text_color(hsla(theme.accent)),
                                                    ),
                                            )
                                        })
                                        // 他ウィンドウで表示中の区別バッジ（#380。
                                        // クリックすればこのウィンドウへ表示が移る）
                                        .when_some(shown_in, |d, win| {
                                            d.child(
                                                div()
                                                    .flex_none()
                                                    .px(px(4.0))
                                                    .h(px(15.0))
                                                    .flex()
                                                    .items_center()
                                                    .rounded(px(4.0))
                                                    .border_1()
                                                    .border_color(hsla(theme.border_subtle))
                                                    .text_size(px(9.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(hsla(theme.text_muted))
                                                    .child(SharedString::from(format!("W{win}"))),
                                            )
                                        })
                                        .when(is_active && pane_states.len() > 1, |d| {
                                            d.child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap(px(2.5))
                                                    .children(pane_states.iter().map(|s| {
                                                        div()
                                                            .w(px(5.0))
                                                            .h(px(5.0))
                                                            .flex_none()
                                                            .rounded(px(1.5))
                                                            .bg(hsla(state_color(s)))
                                                    })),
                                            )
                                        })
                                        .when(!is_active && fails > 0, |d| {
                                            d.child(
                                                div()
                                                    .font_family(theme.font_family.clone())
                                                    .text_size(px(10.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(hsla(theme.red))
                                                    .child(SharedString::from(format!(
                                                        "{fails} fail"
                                                    ))),
                                            )
                                        })
                                        .when(is_active, |d| {
                                            d.child(
                                                div()
                                                    .id(("tab-bg", id.as_u64()))
                                                    .w(px(17.0))
                                                    .h(px(17.0))
                                                    .flex()
                                                    .flex_none()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(5.0))
                                                    .cursor_pointer()
                                                    // 根 div の Drag ヒットテストに勝たせる（#576）+
                                                    // ホイールをスクロール領域へ中継する（#961）
                                                    .occlude_scrolling(cx)
                                                    .text_color(hsla(theme.text_muted))
                                                    .hover(|d| {
                                                        d.bg(rgba(theme.surface_highlight))
                                                            .text_color(hsla(theme.foreground))
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.background_tab(id, cx);
                                                    }))
                                                    .child(
                                                        svg()
                                                            .path(ui_icon::MINUS)
                                                            .w(px(12.0))
                                                            .h(px(12.0))
                                                            .text_color(hsla(theme.text_muted)),
                                                    ),
                                            )
                                        })
                                        .when(is_active, |d| {
                                            d.child(
                                            div()
                                                .id(("tab-close", id.as_u64()))
                                                .w(px(17.0))
                                                .h(px(17.0))
                                                .flex()
                                                .flex_none()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(5.0))
                                                .cursor_pointer()
                                                // 根 div の Drag ヒットテストに勝たせる（#576）+
                                                // ホイールをスクロール領域へ中継する（#961）
                                                .occlude_scrolling(cx)
                                                .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                                .on_click(cx.listener(
                                                    move |this, event: &gpui::ClickEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.close_tab_with_confirm(
                                                            id,
                                                            event.modifiers().platform,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    svg()
                                                        .path(ui_icon::CLOSE)
                                                        .w(px(12.0))
                                                        .h(px(12.0))
                                                        .text_color(hsla(theme.text_muted)),
                                                ),
                                        )
                                        }),
                                ) // .child(div() inner tab pill)
                        },
                    ))
                    // 末尾の挿入インジケータ（タブ D&D 並べ替え: 末尾移動。#371）
                    .when(is_tab_dragging && tab_reorder == Some(None), |d| {
                        d.child(
                            div()
                                .w(px(3.0))
                                .h(px(22.0))
                                .flex_none()
                                .rounded(px(1.5))
                                .bg(hsla(theme.accent))
                                .shadow(vec![BoxShadow {
                                    color: hsla_alpha(theme.accent, 0.5),
                                    offset: point(px(0.), px(0.)),
                                    blur_radius: px(4.0),
                                    spread_radius: px(0.),
                                    inset: false,
                                }]),
                        )
                    })
                    // +: 新規タブ（カンプ 30×30 / radius 8）
                    .child(
                        div()
                            .id("tab-new")
                            .w(px(30.0))
                            .h(px(30.0))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            // 根 div の Drag ヒットテストに勝たせる（#576）+
                            // ホイールをスクロール領域へ中継する（#961）
                            .occlude_scrolling(cx)
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_tab_in_viewport(window, cx)
                            }))
                            .on_drag_move::<TabDrag>(cx.listener(
                                |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                                    if !e.bounds.contains(&e.event.position) {
                                        return;
                                    }
                                    this.set_tab_reorder_indicator(None, cx);
                                },
                            ))
                            .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                                this.drop_tab_reorder(drag.tab, None, cx);
                            }))
                            .on_drag_move::<PaneDrag>(cx.listener(
                                |this, _: &DragMoveEvent<PaneDrag>, _, cx| {
                                    this.set_tab_drop_target(None, cx);
                                },
                            ))
                            .on_drop::<PaneDrag>(cx.listener(|this, drag: &PaneDrag, _, cx| {
                                this.drop_pane_on_tab(drag.pane, None, cx);
                            }))
                            .when(self.tab_drop_target == Some(None), |d| {
                                d.bg(rgba_alpha(theme.accent, 0.2))
                                    .border_2()
                                    .border_color(hsla(theme.accent))
                            })
                            .child(
                                svg()
                                    .path(ui_icon::PLUS)
                                    .w(px(15.0))
                                    .h(px(15.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    ),
            )
            // ⌘K コマンドパレット入口（カンプ: h30 / min-w 210 / radius 8）
            .child(
                div()
                    .id("cmdk-entry")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(30.0))
                    .px(px(12.0))
                    .min_w(px(210.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .bg(rgba(theme.surface_1))
                    .text_color(hsla(theme.text_muted))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
                    .hover(|d| {
                        d.border_color(hsla(theme.border_heavy))
                            .text_color(hsla(theme.text_tertiary))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_command_palette(window, cx);
                    }))
                    .child(
                        svg()
                            .path(ui_icon::SEARCH)
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child(crate::ui_text::palette::search_placeholder())
                    .child(div().flex_grow(1.0))
                    .child(
                        div()
                            .font_family(theme.font_family.clone())
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_faint))
                            .border_1()
                            .border_color(hsla(theme.surface_highlight))
                            .rounded(px(4.0))
                            .px(px(5.0))
                            .py(px(1.0))
                            .child("⌘K"),
                    ),
            )
            // 通知ベル + 未読バッジ（カンプ: 30×30 / バッジ 14px red）
            .child(
                div()
                    .id("attention-bell")
                    .relative()
                    .w(px(30.0))
                    .h(px(30.0))
                    .ml(px(4.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel_visible = !this.panel_visible;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(ui_icon::BELL)
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(theme.text_tertiary)),
                    )
                    .when(attention > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(2.0))
                                .right(px(2.0))
                                .min_w(px(14.0))
                                .h(px(14.0))
                                .px(px(3.0))
                                .rounded(px(7.0))
                                .bg(hsla(theme.red))
                                .border_2()
                                .border_color(rgba(theme.mantle))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.crust))
                                .child(SharedString::from(attention.to_string())),
                        )
                    }),
            )
            // 表示モード切替（#694。GUI ライク ⇔ ターミナル。テーマボタンの左隣 =
            // 「アプリ全体の見た目」と同格の概念という位置づけ。現在モードのアイコンを出す）
            .child(
                div()
                    .id("ui-mode-toggle")
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_ui_mode(cx);
                    }))
                    // 新設ボタンなので何が起きるかを言葉で出す（アイコンだけでは
                    // 「表示モードが変わる」と分からない）
                    .tooltip({
                        let theme = theme.clone();
                        let gui = self.ui_mode.is_gui();
                        move |_, cx| {
                            let label = if gui {
                                crate::ui_text::ui_mode::toggle_to_terminal()
                            } else {
                                crate::ui_text::ui_mode::toggle_to_gui()
                            };
                            cx.new(|_| HintTooltip {
                                label: label.to_string(),
                                theme: theme.clone(),
                            })
                            .into()
                        }
                    })
                    .child(
                        svg()
                            .path(if self.ui_mode.is_gui() {
                                ui_icon::CHAT_BUBBLE
                            } else {
                                ui_icon::PROMPT
                            })
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(if self.ui_mode.is_gui() {
                                theme.accent
                            } else {
                                theme.text_muted
                            })),
                    ),
            )
            // テーマ切替（カンプ: 太陽アイコン。ライト時は月。Issue #217）
            .child(
                div()
                    .id("theme-toggle")
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    // 根 div の Drag ヒットテストに勝たせる（#576）
                    .occlude()
                    .hover(|d| d.bg(rgba(theme.surface_highlight)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_theme(cx);
                    }))
                    .child(
                        svg()
                            .path(match theme.mode {
                                tako_core::theme::ThemeMode::Dark => ui_icon::SUN,
                                tako_core::theme::ThemeMode::Light => ui_icon::MOON,
                            })
                            .w(px(15.0))
                            .h(px(15.0))
                            .text_color(hsla(theme.text_muted)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 脈動の**両端**が不透明度 1.0 になる（#945）。
    ///
    /// GPUI の oneshot は完了後 `delta` を 1.0 に貼り付けたまま止まるので、
    /// t=1.0 が 1.0 でないと「脈動が終わった瞬間に色が飛ぶ」ことになる
    #[test]
    fn 脈動の両端は不透明度_1_0() {
        assert!((tab_dot_opacity(0.0) - 1.0).abs() < 1e-4);
        assert!((tab_dot_opacity(1.0) - 1.0).abs() < 1e-4);
    }

    /// 山の数は [`DOT_PULSE_COUNT`] 回で、山の底は #217 と同じ 0.35
    #[test]
    fn 脈動は指定回数ぶん同じ山を並べる() {
        for k in 0..DOT_PULSE_COUNT {
            // 各山の頂点（= 最も薄いところ）
            let t = (k as f32 + 0.5) / DOT_PULSE_COUNT as f32;
            assert!(
                (tab_dot_opacity(t) - 0.35).abs() < 1e-4,
                "山 {k} の底が 0.35 でない: {}",
                tab_dot_opacity(t)
            );
            // 山と山の境目は 1.0 へ戻る
            let edge = k as f32 / DOT_PULSE_COUNT as f32;
            assert!((tab_dot_opacity(edge) - 1.0).abs() < 1e-4);
        }
    }

    /// 不透明度は常に 0.0〜1.0（`.abs()` を外すと 1.0 を超える）
    #[test]
    fn 不透明度は常に範囲内() {
        for i in 0..=1000 {
            let t = i as f32 / 1000.0;
            let o = tab_dot_opacity(t);
            assert!((0.0..=1.0).contains(&o), "t={t} で {o}");
        }
    }

    /// 横 delta があればそれ、無ければ縦 delta を横へ回す（GPUI の
    /// `overflow_x_scroll` と同じ規則。#961）
    #[test]
    fn ホイールの縦回転は横スクロールへ回る() {
        let lh = px(20.0);
        // 縦だけ → 横へ回る
        assert_eq!(
            tab_scroll_dx(&ScrollDelta::Pixels(point(px(0.0), px(-30.0))), lh),
            px(-30.0)
        );
        // 横があれば横が勝つ（縦は捨てる = GPUI と同じ）
        assert_eq!(
            tab_scroll_dx(&ScrollDelta::Pixels(point(px(12.0), px(-30.0))), lh),
            px(12.0)
        );
        // 行単位は line_height 倍
        assert_eq!(
            tab_scroll_dx(&ScrollDelta::Lines(point(0.0, 2.0)), lh),
            px(40.0)
        );
        // 完全にゼロなら何もしない
        assert_eq!(
            tab_scroll_dx(&ScrollDelta::Pixels(point(px(0.0), px(0.0))), lh),
            Pixels::ZERO
        );
    }

    /// スクロール領域の中で `occlude()` する要素は、必ずホイールを中継する（#961 の番犬）。
    ///
    /// 素の `.occlude()` に戻すと GPUI の hit test が break してスクロール領域の
    /// hitbox が落ち、**タブピルの上でホイールが一切効かなくなる**（#576 が
    /// #208 のスクロールを壊した機序そのもの）。ソース走査なので macOS からも
    /// Windows CI からも走る
    #[test]
    fn スクロール領域の中のoccludeはホイールを中継する() {
        let src = include_str!("tab_bar.rs");
        // `tab-scroll-area` の中にあり、かつ occlude する要素の id
        let inside = [
            r#".id(("tab", "#,
            r#".id(("tab-bg", "#,
            r#".id(("tab-close", "#,
            r#".id("tab-new")"#,
        ];
        let lines: Vec<&str> = src.lines().collect();
        for id in inside {
            let at = lines
                .iter()
                .position(|l| l.contains(id))
                .unwrap_or_else(|| panic!("{id} が見つからない（id を変えたら番犬も直すこと）"));
            // その要素のビルダ連鎖（次の `.id(` の手前まで）に occlude 系が 1 つある
            let end = lines[at + 1..]
                .iter()
                .position(|l| l.contains(".id("))
                .map(|i| at + 1 + i)
                .unwrap_or(lines.len());
            let chain = &lines[at..end];
            let relays = chain
                .iter()
                .filter(|l| l.contains(".occlude_scrolling(cx)"))
                .count();
            let bare = chain.iter().filter(|l| l.trim() == ".occlude()").count();
            assert_eq!(
                (relays, bare),
                (1, 0),
                "{id}: スクロール領域の中では occlude_scrolling(cx) を使うこと \
                 (#961。relays={relays} bare={bare})"
            );
        }
    }

    /// 脈動の長さは「1 往復 × 回数」（セルフテストの待ち時間の根拠）
    #[test]
    fn 脈動の全長は往復の整数倍() {
        assert_eq!(dot_pulse_total(), DOT_PULSE_PERIOD * DOT_PULSE_COUNT);
        assert_eq!(dot_pulse_total(), Duration::from_secs(6));
    }
}

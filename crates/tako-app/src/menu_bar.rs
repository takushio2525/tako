//! in-window メニューバー行（Issue #657。**実質 Windows 専用**）
//!
//! macOS はアプリメニュー（#485）が OS のグローバルメニューバーへ載るが、
//! `gpui_windows` の `Platform::set_menus` は渡された `Menu` を内部に保存するだけで
//! ウィンドウメニューを作らない（rev `cafbf4b5` の `platform.rs`）。つまり Windows では
//! メニューへの出口が一つも無いので、tako がタブバーの一段上に自前の 1 行として描く。
//! 参照イメージは VSCode のカスタムタイトルバー（左からメニュー群、右端にウィンドウ
//! コントロール、空き領域はウィンドウドラッグ）。
//!
//! 行を出すかどうかは `MENU_BAR_HEIGHT > 0.0`（cfg で 0 / 30）で決める。**関数本体は
//! 両プラットフォームでコンパイルする**（`#[cfg]` でまるごと消すと macOS 側が
//! コンパイルされず、Windows 実機でしか壊れに気づけない経路が増えるため）。
//!
//! # Windows のヒットテストの罠（実装前に必ず読む）
//!
//! `hide_title_bar` のとき `gpui_windows::handle_hit_test_msg` は
//!
//! 1. `WindowControlArea::Close` / `Max` / `Min` を **即 return** する（上端判定より優先）
//! 2. `WindowControlArea::Drag` は保留し、**上端 `frame_y` px（100% DPI で約 8px、
//!    125% で約 10px）を `HTTOP` = リサイズエッジとして先に返す**（最大化中は除く）
//!
//! ので:
//!
//! - ウィンドウコントロールは最上段でも全面がクリックできる（1 のため）
//! - **自前 `on_click` のメニュートリガーは上端 `frame_y` px が OS に食われる**。
//!   だから `MENU_TRIGGER_TOP`（4px）だけ下げ、さらにトリガー高さを行より低くして
//!   「押せる下側」を確保する。上端に残る数 px は素直にウィンドウリサイズへ譲る
//!   （Zed 本体のタイトルバーも同じ挙動）
//!
//! # `.occlude()` は必須（#576）
//!
//! 行の根 div は `WindowControlArea::Drag` を張るため、子が occlude していないと
//! 祖先の hitbox まで hit test に積まれ、`on_hit_test_window_control` が Drag を返して
//! `HTCAPTION` に化け、ボタンが完全に死ぬ（macOS では再現しない Windows 固有の罠）。

use gpui::{
    div, prelude::*, px, svg, AnyElement, Context, MouseButton, SharedString, TextRun, Window,
    WindowControlArea,
};

use super::*;
use crate::file_icons::ui_icon;

/// メニューバーのラベル文字サイズ（px）
const MENU_LABEL_SIZE: f32 = 12.5;
/// メニュートリガーの左右パディング（px）
const MENU_TRIGGER_PAD: f32 = 9.0;
/// メニュートリガーの高さ（px）
const MENU_TRIGGER_HEIGHT: f32 = 22.0;
/// メニュートリガーの上マージン（px）。上端はリサイズエッジに食われる（モジュール doc）
const MENU_TRIGGER_TOP: f32 = 4.0;
/// 行の左端余白（px）
const MENU_BAR_PAD_LEFT: f32 = 8.0;

/// ドロップダウンの最小幅（px）
const DROPDOWN_MIN_WIDTH: f32 = 210.0;
/// ドロップダウン項目の高さ（px）
const DROPDOWN_ITEM_HEIGHT: f32 = 26.0;
/// ドロップダウンの左右パディング（px）
const DROPDOWN_ITEM_PAD: f32 = 10.0;
/// サブメニュー項目のインデント（px）
const DROPDOWN_SUBMENU_INDENT: f32 = 16.0;

/// 自前ウィンドウコントロール（#584）1 個の一辺。bell / theme ボタンと同寸
const WINDOW_CONTROL_SIZE: f32 = 30.0;
/// 自前ウィンドウコントロール群の概算幅（3 ボタン + gap + 左マージン）。
/// macOS は native traffic lights が同居するので描かない = 0
#[cfg(target_os = "macos")]
pub(crate) const WINDOW_CONTROLS_PX: f32 = 0.0;
#[cfg(not(target_os = "macos"))]
pub(crate) const WINDOW_CONTROLS_PX: f32 = WINDOW_CONTROL_SIZE * 3.0 + 12.0;

/// in-window メニューバーの開閉状態（Issue #657）。
///
/// `TakoApp` は全ウィンドウの root view を兼ねる（#339 のビューポート方式）ので、
/// この状態も全ウィンドウで共有される = ウィンドウ A で開くと B でも開いて見える。
/// タブバーが全ウィンドウ共通（#380）なのと同じ割り切りで、直すならウィンドウ単位の
/// 状態表（`viewports` と同じキー空間）へ移す必要がある
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MenuBarState {
    /// 開いているメニューの添字（None = 閉じている）
    pub open: Option<usize>,
    /// キーボード操作でハイライト中の行の添字（`menu_rows` の並び。マウス時は None）
    pub highlighted: Option<usize>,
    /// その場展開しているサブメニューの添字（`OwnedMenu::items` の並び）
    pub expanded: Option<usize>,
}

/// メニュートリガー 1 個の実測レイアウト（描画とドロップダウンの位置決めで共用する）
#[derive(Debug, Clone)]
pub(crate) struct MenuTrigger {
    pub label: SharedString,
    /// 行内の左端 x（px）
    pub left: f32,
    /// トリガーの幅（px）
    pub width: f32,
}

/// ドロップダウンに実際に並ぶ 1 行。
///
/// **描画・キーボード操作・CLI / MCP のパス解決がすべてこの並びを正とする**
/// （別々に組み立てると「見えている順」と「↑↓ で動く順」がずれる）
pub(crate) enum MenuRow<'a> {
    /// 実行できる項目
    Action {
        label: &'a str,
        action: &'a dyn gpui::Action,
        /// サブメニュー内なら親の添字
        parent: Option<usize>,
    },
    /// 区切り線（選択できない）
    Separator,
    /// その場展開するサブメニューの見出し
    Submenu {
        /// `OwnedMenu::items` 内の添字
        index: usize,
        label: &'a str,
        expanded: bool,
    },
}

impl MenuRow<'_> {
    /// キーボードで選べる行か（区切り線だけ選べない）
    fn selectable(&self) -> bool {
        !matches!(self, MenuRow::Separator)
    }
}

/// メニュー 1 枚をドロップダウンの表示行へ展開する（純粋関数）。
///
/// サブメニュー（表示 ▸ パネル / ウインドウ ▸ ペインを選択）は 2 段目のポップアップに
/// せず**その場展開**する。ポップアップの入れ子は座標計算とフォーカス遷移が増える割に、
/// tako のサブメニューは 3〜4 項目しか無いため。展開状態は `expanded` が持つ
pub(crate) fn menu_rows(menu: &gpui::OwnedMenu, expanded: Option<usize>) -> Vec<MenuRow<'_>> {
    let mut rows = Vec::new();
    for (index, item) in menu.items.iter().enumerate() {
        match item {
            gpui::OwnedMenuItem::Separator => rows.push(MenuRow::Separator),
            gpui::OwnedMenuItem::Action { name, action, .. } => rows.push(MenuRow::Action {
                label: name.as_str(),
                action: action.as_ref(),
                parent: None,
            }),
            gpui::OwnedMenuItem::Submenu(sub) => {
                let is_expanded = expanded == Some(index);
                rows.push(MenuRow::Submenu {
                    index,
                    label: sub.name.as_ref(),
                    expanded: is_expanded,
                });
                if is_expanded {
                    for child in &sub.items {
                        if let gpui::OwnedMenuItem::Action { name, action, .. } = child {
                            rows.push(MenuRow::Action {
                                label: name.as_str(),
                                action: action.as_ref(),
                                parent: Some(index),
                            });
                        }
                    }
                }
            }
            // macOS の Services 等。Windows 版の構成には入らない（`app_menus`）が、
            // 入っていても「押せない行」として黙って捨てる（ダミーを描かない）
            gpui::OwnedMenuItem::SystemMenu(_) => {}
        }
    }
    rows
}

/// キーストロークを Windows 慣習の表記へ整形する（`Ctrl+Shift+T`）。
///
/// GPUI の `Display for Keystroke` は `ctrl-shift-t` 形式（小文字・ハイフン区切り）で
/// メニュー表示には向かないので使わない。純粋関数なので macOS からも検証できる
pub(crate) fn format_keystroke(modifiers: &gpui::Modifiers, key: &str) -> String {
    let mut out = String::new();
    if modifiers.control {
        out.push_str("Ctrl+");
    }
    if modifiers.platform {
        // GPUI の platform 修飾は macOS = ⌘ / Windows = Win キー
        out.push_str(if cfg!(target_os = "macos") {
            "Cmd+"
        } else {
            "Win+"
        });
    }
    if modifiers.alt {
        out.push_str("Alt+");
    }
    if modifiers.shift {
        out.push_str("Shift+");
    }
    out.push_str(&pretty_key(key));
    out
}

/// キー名を表示用に整える（`escape` → `Esc`、`t` → `T`）
fn pretty_key(key: &str) -> String {
    match key {
        "escape" => "Esc".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "plus" => "+".to_string(),
        "minus" => "-".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                // 1 文字キーは大文字（`t` → `T`）、`f11` 等は先頭だけ大文字
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// アクション名 → ショートカット表示のマップ（Issue #657）。
///
/// `Window::bindings_for_action` は dispatch tree（直前フレームのフォーカス状態）に
/// 依存するので使わない。`keybindings::key_bindings()` の静的な表から引く方が
/// 決定的で、CLI / MCP のスナップショット（Window を持たない経路）とも同じ値になる。
///
/// GPUI の `bindings_for_action` と同じく**後に登録されたものを優先**する
/// （`platform_bindings` が macOS 慣習のバインドを上書きする形になっているため）
pub(crate) fn shortcut_map() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for binding in crate::keybindings::key_bindings() {
        let keys: Vec<String> = binding
            .keystrokes()
            .iter()
            .map(|ks| format_keystroke(ks.modifiers(), ks.key()))
            .collect();
        if keys.is_empty() {
            continue;
        }
        out.insert(binding.action().name().to_string(), keys.join(" "));
    }
    out
}

impl TakoApp {
    /// メニュー構成のスナップショット（Issue #657。CLI / MCP `tako_menu` 用）
    pub(crate) fn build_menu_bar_snapshot(&self) -> tako_control::protocol::MenuBarSnapshot {
        use tako_control::protocol::{MenuBarSnapshot, MenuItemSnapshot, MenuSnapshot};
        let shortcuts = shortcut_map();
        let to_item = |item: &gpui::OwnedMenuItem| -> Option<MenuItemSnapshot> {
            match item {
                gpui::OwnedMenuItem::Separator => Some(MenuItemSnapshot::Separator),
                gpui::OwnedMenuItem::Action { name, action, .. } => {
                    let action_name = action.name().to_string();
                    Some(MenuItemSnapshot::Action {
                        label: name.clone(),
                        shortcut: shortcuts.get(&action_name).cloned(),
                        action: action_name,
                    })
                }
                gpui::OwnedMenuItem::Submenu(sub) => Some(MenuItemSnapshot::Submenu {
                    label: sub.name.to_string(),
                    items: sub
                        .items
                        .iter()
                        .filter_map(|child| match child {
                            gpui::OwnedMenuItem::Action { name, action, .. } => {
                                let action_name = action.name().to_string();
                                Some(MenuItemSnapshot::Action {
                                    label: name.clone(),
                                    shortcut: shortcuts.get(&action_name).cloned(),
                                    action: action_name,
                                })
                            }
                            _ => None,
                        })
                        .collect(),
                }),
                // macOS の Services 等。tako から実行できないので一覧にも出さない
                gpui::OwnedMenuItem::SystemMenu(_) => None,
            }
        };
        MenuBarSnapshot {
            in_window: MENU_BAR_HEIGHT > 0.0,
            open: self
                .menu_bar
                .open
                .and_then(|i| self.menu_defs.get(i))
                .map(|m| m.name.to_string()),
            menus: self
                .menu_defs
                .iter()
                .map(|menu| MenuSnapshot {
                    name: menu.name.to_string(),
                    items: menu.items.iter().filter_map(to_item).collect(),
                })
                .collect(),
        }
    }

    /// アクション名（`tako::NewTab`）からメニュー定義中のアクションを引く（Issue #657）。
    ///
    /// `gpui::Action` を名前から作る一般的な手段が無いので、**表示しているメニューに
    /// 実在する項目だけ**発火できる形にする（CLI から任意のアクションを撃てない = 安全側）
    pub(crate) fn find_menu_action(&self, name: &str) -> Option<Box<dyn gpui::Action>> {
        for menu in &self.menu_defs {
            for item in &menu.items {
                match item {
                    gpui::OwnedMenuItem::Action { action, .. } if action.name() == name => {
                        return Some(action.boxed_clone());
                    }
                    gpui::OwnedMenuItem::Submenu(sub) => {
                        for child in &sub.items {
                            if let gpui::OwnedMenuItem::Action { action, .. } = child {
                                if action.name() == name {
                                    return Some(action.boxed_clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// メニュー定義（言語別）を貼り直す（Issue #657）。
    ///
    /// `app_menus()` は呼ぶたびに `Box<dyn Action>` を 40 個以上作るので、毎フレーム
    /// 呼ばずに言語が変わったときだけキャッシュする。`OwnedMenu` は `Clone` なので
    /// 保持できる（`Menu` は保持できない）
    pub(crate) fn refresh_menu_defs(&mut self) {
        self.menu_defs = crate::app_menus()
            .into_iter()
            .map(|menu| menu.owned())
            .collect();
    }

    /// メニュートリガーの実測レイアウト（Issue #657）。
    ///
    /// ラベル幅を実測してトリガー幅を自分で決めることで、**描画とドロップダウンの
    /// 位置決めが同じ数値**になる。GPUI に幅を任せると、あとから bounds を採取する
    /// 仕掛け（canvas + defer）が要り、#315 R2 のようなタイミング窓を持ち込む
    fn menu_bar_triggers(&self, window: &mut Window) -> Vec<MenuTrigger> {
        let mut out = Vec::new();
        let mut x = MENU_BAR_PAD_LEFT;
        for menu in &self.menu_defs {
            let label = SharedString::from(menu.name.to_string());
            let run = TextRun {
                len: label.len(),
                font: gpui::font(self.theme.font_family.clone()),
                color: hsla(self.theme.foreground),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let text_w = f32::from(
                window
                    .text_system()
                    .shape_line(label.clone(), px(MENU_LABEL_SIZE), &[run], None)
                    .width,
            );
            let width = text_w + MENU_TRIGGER_PAD * 2.0;
            out.push(MenuTrigger {
                label,
                left: x,
                width,
            });
            x += width;
        }
        out
    }

    /// メニューバー行（Issue #657）。macOS は行を持たない（`MENU_BAR_HEIGHT == 0`）
    pub(crate) fn render_menu_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if MENU_BAR_HEIGHT <= 0.0 {
            return None;
        }
        let theme = self.theme.clone();
        let triggers = self.menu_bar_triggers(window);
        // ドロップダウンの位置決めが同じ数値を使えるよう保存する（描画順は
        // メニューバー → オーバーレイなので、この代入は必ず先に走る）
        self.menu_trigger_layout = triggers.clone();
        let open = self.menu_bar.open;

        let row = div()
            .id("menu-bar")
            .flex()
            .flex_row()
            .items_start()
            .h(px(MENU_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .bg(rgba(theme.crust))
            .window_control_area(WindowControlArea::Drag)
            // 空き領域のドラッグでウィンドウ移動（タブバー行 #312 と同じ作法）。
            // Windows は WindowControlArea::Drag → HTCAPTION で OS が処理するが、
            // macOS は on_hit_test_window_control が空実装なので明示呼び出しが要る
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.titlebar_dragging = false;
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.titlebar_dragging = false;
            }))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.titlebar_dragging {
                    this.titlebar_dragging = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            });

        // トリガー群は「切り詰められる側」に置く（flex_1 + overflow_hidden）。
        // ウィンドウコントロールを flex_none にして先に幅を確保するので、
        // **ウィンドウを狭めてもメニューが押し出すだけで閉じるボタンは残る**。
        // 左 padding は `menu_bar_triggers` の積算起点（MENU_BAR_PAD_LEFT）と同じ値に
        // する（食い違うとドロップダウンが 1 個分ずれて出る）。
        // `occlude` は付けない —— 空き領域でのウィンドウドラッグ移動が死ぬ
        // （tab_bar の `tab-scroll-area` と同じ理由）
        let mut triggers_row = div()
            .id("menu-bar-triggers")
            .flex()
            .flex_row()
            .items_start()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .pl(px(MENU_BAR_PAD_LEFT));
        for (index, trigger) in triggers.iter().enumerate() {
            let is_open = open == Some(index);
            triggers_row = triggers_row.child(
                div()
                    .id(("menu-trigger", index as u64))
                    // 絶対配置にはしない（positioned 祖先が誰になるかに依存して壊れる）。
                    // 幅は実測値を明示指定するので、flex で並べても `trigger.left` の
                    // 積算と一致する。上マージンは上端リサイズエッジ避け（モジュール doc）
                    .mt(px(MENU_TRIGGER_TOP))
                    .w(px(trigger.width))
                    .h(px(MENU_TRIGGER_HEIGHT))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    // 根 div の Drag ヒットテストに勝たせる（#576）。これが無いと死ぬ
                    .occlude()
                    .text_size(px(MENU_LABEL_SIZE))
                    .when(is_open, |d| {
                        d.bg(rgba(theme.surface_highlight))
                            .text_color(hsla(theme.foreground))
                    })
                    .when(!is_open, |d| {
                        d.text_color(hsla(theme.text_tertiary))
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // 押した瞬間に開閉する（VSCode と同じ）。空き領域ドラッグの
                            // フラグも降ろして、メニュー操作でウィンドウが動かないようにする
                            this.titlebar_dragging = false;
                            this.toggle_menu(index, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        // 開いている間はホバーで隣のメニューへ切り替わる（VSCode / OS 標準）
                        if this.menu_bar.open.is_some() && this.menu_bar.open != Some(index) {
                            this.open_menu(index, cx);
                        }
                    }))
                    .child(trigger.label.clone()),
            );
        }

        // 右端のウィンドウコントロール（#584 からこの行へ移設）
        Some(
            row.child(triggers_row)
                .when(WINDOW_CONTROLS_PX > 0.0, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .items_start()
                            .gap(px(2.0))
                            .pr(px(6.0))
                            .children(self.render_window_controls(window, &theme)),
                    )
                })
                .into_any_element(),
        )
    }

    /// 開いているメニューのドロップダウン（Issue #657）。
    ///
    /// ルート直下のオーバーレイとして描く。行の子として絶対配置すると、後から描かれる
    /// 兄弟（タブバー・ペイン）に隠れる/クリップされる（#361 / #341 と同じ罠）
    pub(crate) fn render_menu_dropdown(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let index = self.menu_bar.open?;
        let menu = self.menu_defs.get(index)?;
        let trigger = self.menu_trigger_layout.get(index)?;
        let theme = self.theme.clone();
        let highlighted = self.menu_bar.highlighted;
        let rows = menu_rows(menu, self.menu_bar.expanded);
        // ショートカット表示は静的なキーバインド表から引く（`shortcut_map` の doc）。
        // 開いている 1 枚分だけなので、開いたフレームで 1 回組み立てれば足りる
        let shortcuts = shortcut_map();

        // 画面右端で見切れないよう左へ寄せる（#314 のコンテキストメニューと同じ考え方）
        let viewport_w = f32::from(window.viewport_size().width);
        let left = trigger.left.min((viewport_w - DROPDOWN_MIN_WIDTH).max(0.0));

        let mut list = div()
            .id("menu-dropdown")
            .absolute()
            .left(px(left))
            .top(px(MENU_BAR_HEIGHT))
            .min_w(px(DROPDOWN_MIN_WIDTH))
            .py(px(4.0))
            .rounded(px(8.0))
            .bg(rgba(theme.surface_1))
            .border_1()
            .border_color(hsla(theme.border_heavy))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                // 背景クリックの dismiss に食われないようにする
                cx.stop_propagation();
            });

        for (row_index, row) in rows.iter().enumerate() {
            let is_highlighted = highlighted == Some(row_index);
            match row {
                MenuRow::Separator => {
                    list = list.child(
                        div()
                            .my(px(3.0))
                            .mx(px(6.0))
                            .h(px(1.0))
                            .bg(hsla(theme.border_subtle)),
                    );
                }
                MenuRow::Action {
                    label,
                    action,
                    parent,
                } => {
                    let shortcut = shortcuts.get(action.name()).cloned();
                    let indent = if parent.is_some() {
                        DROPDOWN_SUBMENU_INDENT
                    } else {
                        0.0
                    };
                    let boxed = action.boxed_clone();
                    list = list.child(
                        div()
                            .id(("menu-item", row_index as u64))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(16.0))
                            .h(px(DROPDOWN_ITEM_HEIGHT))
                            .pl(px(DROPDOWN_ITEM_PAD + indent))
                            .pr(px(DROPDOWN_ITEM_PAD))
                            .mx(px(4.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .text_color(hsla(theme.text_secondary))
                            .when(is_highlighted, |d| {
                                d.bg(rgba(theme.surface_hover_strong))
                                    .text_color(hsla(theme.foreground))
                            })
                            .hover(|d| {
                                d.bg(rgba(theme.surface_hover_strong))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.menu_bar.highlighted != Some(row_index) {
                                    this.menu_bar.highlighted = Some(row_index);
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.invoke_menu_action(boxed.boxed_clone(), window, cx);
                            }))
                            .child(div().flex_1().child(SharedString::from(label.to_string())))
                            .when_some(shortcut, |d, keys| {
                                d.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.text_faint))
                                        .child(SharedString::from(keys)),
                                )
                            }),
                    );
                }
                MenuRow::Submenu {
                    index: sub_index,
                    label,
                    expanded,
                } => {
                    let sub_index = *sub_index;
                    let expanded = *expanded;
                    list = list.child(
                        div()
                            .id(("menu-submenu", row_index as u64))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(DROPDOWN_ITEM_HEIGHT))
                            .px(px(DROPDOWN_ITEM_PAD))
                            .mx(px(4.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .text_color(hsla(theme.text_secondary))
                            .when(is_highlighted, |d| {
                                d.bg(rgba(theme.surface_hover_strong))
                                    .text_color(hsla(theme.foreground))
                            })
                            .hover(|d| {
                                d.bg(rgba(theme.surface_hover_strong))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.menu_bar.highlighted != Some(row_index) {
                                    this.menu_bar.highlighted = Some(row_index);
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_menu_submenu(sub_index, cx);
                            }))
                            .child(div().flex_1().child(SharedString::from(label.to_string())))
                            .child(
                                svg()
                                    .path(if expanded {
                                        ui_icon::CHEVRON_DOWN
                                    } else {
                                        ui_icon::CHEVRON_RIGHT
                                    })
                                    .w(px(11.0))
                                    .h(px(11.0))
                                    .text_color(hsla(theme.text_muted)),
                            ),
                    );
                }
            }
        }

        // 背景クリック / 別の場所への mouse down で閉じる（run メニュー #453 と同じ作法）
        Some(
            div()
                .id("menu-dropdown-dismiss")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_menu(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(list)
                .into_any_element(),
        )
    }

    /// 最小化 / 最大化↔復元 / 閉じるボタン（Issue #584。**Windows 専用**）
    ///
    /// `tako_titlebar_options()` の `appears_transparent` は GPUI Windows では
    /// `hide_title_bar` になり、ウィンドウスタイルから `WS_CAPTION` が落ちる。
    /// ネイティブのキャプションボタンはそもそも生成されないので自前で描く。
    /// macOS は native traffic lights が同居するため描かない（`WINDOW_CONTROLS_PX == 0`）。
    ///
    /// # 実際にウィンドウを動かすのは GPUI のネイティブ経路（`on_click` ではない）
    ///
    /// `window_control_area` は hitbox を登録するだけだが、`gpui_windows` の
    /// `handle_hit_test_msg`（`WM_NCHITTEST`）がその area を
    /// `HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE` に変換し、`handle_nc_mouse_up_msg` が
    /// `ShowWindowAsync(SW_MINIMIZE)` /（`IsZoomed` で出し分けた）`SW_MAXIMIZE`・`SW_NORMAL` /
    /// `PostMessageW(WM_CLOSE)` を実行する。つまり最大化↔復元のトグルは GPUI 側の責務で、
    /// ここが持つのは「今どちらの状態か」を示すアイコンの出し分けだけ。
    /// `HTMAXBUTTON` を返すことで Windows 11 の Snap Layouts（最大化ボタンのホバーで出る
    /// レイアウト選択）も自動で効く。
    ///
    /// # `.occlude()` は必須（#576）
    ///
    /// 付けないと祖先のメニューバー根 div（`WindowControlArea::Drag`）の hitbox も
    /// `mouse_hit_test.ids` に残り、`on_hit_test_window_control` は
    /// **登録順（= 描画順 = 祖先が先）で最初に一致したもの**を返すため `Drag` が勝つ。
    /// すると `HTCAPTION` に化けてボタンが完全に死ぬ。
    fn render_window_controls(&self, window: &Window, theme: &Theme) -> Vec<AnyElement> {
        let maximized = window.is_maximized();
        // 一辺 30px・角丸 8・ホバーで背景 = bell / theme ボタンと同じ作法。
        // `danger` = 閉じるボタン。ホバーを赤にするのは Windows 標準の作法で、
        // その上でアイコンが読めるよう前景も反転させる（通知バッジと同じ red/crust の組）
        let button = |id: &'static str, area: WindowControlArea, icon: &'static str, danger| {
            div()
                .id(id)
                .group(id)
                .w(px(WINDOW_CONTROL_SIZE))
                .h(px(WINDOW_CONTROL_SIZE))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .cursor_pointer()
                .window_control_area(area)
                // 根 div の Drag ヒットテストに勝たせる（#576）。これが無いと死ぬ
                .occlude()
                .hover(|d| {
                    if danger {
                        d.bg(hsla(theme.red))
                    } else {
                        d.bg(rgba(theme.surface_highlight))
                    }
                })
                .child(
                    svg()
                        .path(icon)
                        .w(px(15.0))
                        .h(px(15.0))
                        .text_color(hsla(theme.text_muted))
                        .when(danger, |s| {
                            s.group_hover(id, |s| s.text_color(hsla(theme.crust)))
                        }),
                )
        };

        vec![
            button(
                "window-minimize",
                WindowControlArea::Min,
                ui_icon::MINUS,
                false,
            )
            .into_any_element(),
            button(
                "window-maximize",
                WindowControlArea::Max,
                if maximized {
                    ui_icon::WINDOW_RESTORE
                } else {
                    ui_icon::WINDOW_MAXIMIZE
                },
                false,
            )
            .into_any_element(),
            button(
                "window-close",
                WindowControlArea::Close,
                ui_icon::CLOSE,
                true,
            )
            .into_any_element(),
        ]
    }

    // --- 開閉・実行 ---------------------------------------------------------

    /// メニューを開く（Issue #657）
    pub(crate) fn open_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.menu_defs.len() {
            return;
        }
        self.menu_bar = MenuBarState {
            open: Some(index),
            highlighted: None,
            expanded: None,
        };
        cx.notify();
    }

    /// 同じメニューなら閉じる、違うメニューなら開き直す
    pub(crate) fn toggle_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.menu_bar.open == Some(index) {
            self.close_menu(cx);
        } else {
            self.open_menu(index, cx);
        }
    }

    /// メニューを閉じる
    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu_bar == MenuBarState::default() {
            return;
        }
        self.menu_bar = MenuBarState::default();
        cx.notify();
    }

    /// サブメニューのその場展開をトグルする
    fn toggle_menu_submenu(&mut self, sub_index: usize, cx: &mut Context<Self>) {
        self.menu_bar.expanded = if self.menu_bar.expanded == Some(sub_index) {
            None
        } else {
            Some(sub_index)
        };
        cx.notify();
    }

    /// メニュー項目のアクションを発火する（Issue #657）。
    ///
    /// 先にメニューを閉じてから dispatch する。開いたままだと、アクションの副作用で
    /// 開くダイアログ（設定・About）の下にドロップダウンが残る
    pub(crate) fn invoke_menu_action(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_bar = MenuBarState::default();
        cx.notify();
        // root div の on_action / cx.on_action（#103 のグローバル登録）へ配送する。
        // tako は単一 focus_handle 設計で root にフォーカスがあるため両方に届く
        window.dispatch_action(action, cx);
    }

    /// メニューバーのキー操作（Issue #657）。処理したら true を返し、PTY へは送らない。
    ///
    /// **F10 を入口にする**（Windows 標準のメニューフォーカスキー）。`Alt` は採らない —
    /// tako は Alt+印字文字を PTY へ meta エンコードする（#575）ので、Alt を奪うと
    /// Claude Code の Alt+V 等が壊れる。F1〜F12 は `keystroke_to_bytes` が PTY へ
    /// 送っていないので、F10 を奪っても失うものが無い（#602 の F11 と同じ判断）
    pub(crate) fn menu_bar_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if MENU_BAR_HEIGHT <= 0.0 {
            return false;
        }
        let key = keystroke.key.as_str();
        let plain = !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.shift;

        // 閉じているとき: F10 で先頭メニューを開く（それ以外は素通り）
        if self.menu_bar.open.is_none() {
            if key == "f10" && plain && !self.menu_defs.is_empty() {
                self.open_menu(0, cx);
                self.move_menu_highlight(1, cx);
                return true;
            }
            return false;
        }

        // 開いているとき: メニュー操作のキーを奪う
        match key {
            "escape" | "f10" => {
                self.close_menu(cx);
                true
            }
            "left" => {
                self.step_menu(-1, cx);
                true
            }
            "right" => {
                // サブメニューの見出し上なら展開（OS メニューの → と同じ）
                if let Some(sub) = self.highlighted_submenu() {
                    if self.menu_bar.expanded != Some(sub) {
                        self.toggle_menu_submenu(sub, cx);
                        return true;
                    }
                }
                self.step_menu(1, cx);
                true
            }
            "down" => {
                self.move_menu_highlight(1, cx);
                true
            }
            "up" => {
                self.move_menu_highlight(-1, cx);
                true
            }
            "enter" => {
                self.activate_highlighted_menu_row(window, cx);
                true
            }
            // それ以外の打鍵はメニューを閉じてターミナルへ通す。奪ってしまうと
            // 「メニューを開いたのを忘れて打った文字が消える」= 入力の欠落になる
            _ => {
                self.close_menu(cx);
                false
            }
        }
    }

    /// 隣のメニューへ移動する（開いたまま）
    fn step_menu(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.menu_defs.len();
        if count == 0 {
            return;
        }
        let current = self.menu_bar.open.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(count as isize) as usize;
        self.open_menu(next, cx);
        self.move_menu_highlight(1, cx);
    }

    /// ハイライトを次の選択可能な行へ動かす（区切り線は飛ばす）
    fn move_menu_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(index) = self.menu_bar.open else {
            return;
        };
        let Some(menu) = self.menu_defs.get(index) else {
            return;
        };
        let rows = menu_rows(menu, self.menu_bar.expanded);
        if rows.is_empty() {
            return;
        }
        let len = rows.len() as isize;
        // 未選択なら delta 方向の端から探し始める
        let start = match self.menu_bar.highlighted {
            Some(current) => current as isize,
            None if delta > 0 => -1,
            None => len,
        };
        let mut candidate = start;
        for _ in 0..rows.len() {
            candidate = (candidate + delta).rem_euclid(len);
            if rows[candidate as usize].selectable() {
                self.menu_bar.highlighted = Some(candidate as usize);
                cx.notify();
                return;
            }
        }
    }

    /// ハイライト中の行がサブメニュー見出しならその添字
    fn highlighted_submenu(&self) -> Option<usize> {
        let menu = self.menu_defs.get(self.menu_bar.open?)?;
        let rows = menu_rows(menu, self.menu_bar.expanded);
        match rows.get(self.menu_bar.highlighted?)? {
            MenuRow::Submenu { index, .. } => Some(*index),
            _ => None,
        }
    }

    /// Enter: ハイライト中の行を実行する（サブメニュー見出しなら展開）
    fn activate_highlighted_menu_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.open else {
            return;
        };
        let Some(highlighted) = self.menu_bar.highlighted else {
            return;
        };
        // action は menu_defs からの借用なので、先に取り出してから self を触る
        let picked = {
            let Some(menu) = self.menu_defs.get(open) else {
                return;
            };
            let rows = menu_rows(menu, self.menu_bar.expanded);
            match rows.get(highlighted) {
                Some(MenuRow::Action { action, .. }) => Some(Ok(action.boxed_clone())),
                Some(MenuRow::Submenu { index, .. }) => Some(Err(*index)),
                _ => None,
            }
        };
        match picked {
            Some(Ok(action)) => self.invoke_menu_action(action, window, cx),
            Some(Err(sub)) => {
                self.toggle_menu_submenu(sub, cx);
                self.move_menu_highlight(1, cx);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_menu() -> gpui::OwnedMenu {
        use gpui::{Menu, MenuItem};
        Menu::new("ファイル")
            .items(vec![
                MenuItem::action("新規タブ", crate::NewTab),
                MenuItem::separator(),
                MenuItem::submenu(Menu::new("パネル").items(vec![
                    MenuItem::action("fleet ビュー", crate::ShowFleetPanel),
                    MenuItem::action("git ビュー", crate::ShowGitPanel),
                ])),
                MenuItem::action("終了", crate::Quit),
            ])
            .owned()
    }

    /// 折りたたみ時はサブメニューの中身が並ばない
    #[test]
    fn サブメニューは折りたたみ時に子を出さない() {
        let menu = sample_menu();
        let rows = menu_rows(&menu, None);
        assert_eq!(rows.len(), 4, "見出し 1 行として数える");
        assert!(matches!(
            rows[2],
            MenuRow::Submenu {
                expanded: false,
                ..
            }
        ));
    }

    /// 展開時は子がその場に並び、↑↓ の対象になる
    #[test]
    fn サブメニューを展開すると子が同じ並びに入る() {
        let menu = sample_menu();
        let rows = menu_rows(&menu, Some(2));
        assert_eq!(rows.len(), 6, "見出し + 子 2 件");
        match &rows[3] {
            MenuRow::Action { label, parent, .. } => {
                assert_eq!(*label, "fleet ビュー");
                assert_eq!(*parent, Some(2), "親の添字を持つ = インデントの根拠");
            }
            _ => panic!("展開後は子が Action 行として並ぶ"),
        }
    }

    /// 区切り線は選択できない（↑↓ が止まってはいけない）
    #[test]
    fn 区切り線は選択できない() {
        let menu = sample_menu();
        let rows = menu_rows(&menu, None);
        assert!(!rows[1].selectable());
        assert!(rows[0].selectable());
        assert!(rows[2].selectable(), "サブメニュー見出しは選べる");
    }

    /// ショートカット表記は Windows 慣習（`ctrl-shift-t` ではなく `Ctrl+Shift+T`）
    #[test]
    fn ショートカット表記がwindows慣習になる() {
        let m = gpui::Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(format_keystroke(&m, "t"), "Ctrl+Shift+T");
        assert_eq!(format_keystroke(&gpui::Modifiers::default(), "f10"), "F10");
        assert_eq!(
            format_keystroke(&gpui::Modifiers::default(), "escape"),
            "Esc"
        );
        let alt = gpui::Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(format_keystroke(&alt, "left"), "Ctrl+Alt+←");
    }

    /// メニューバー行の高さは macOS で 0（OS のメニューバーに載るので行を持たない）。
    ///
    /// 値は cfg で決まる定数なので、いったん変数へ移してから比べる
    /// （`assert!(定数 > 0.0)` は clippy の `assertions_on_constants` に当たる）
    #[test]
    fn メニューバー行はwindowsだけ高さを持つ() {
        let bar = MENU_BAR_HEIGHT;
        let controls = WINDOW_CONTROLS_PX;
        if cfg!(target_os = "macos") {
            assert_eq!(bar, 0.0);
            assert_eq!(controls, 0.0, "macOS は native traffic lights");
        } else {
            assert!(bar > 0.0);
            assert!(controls > 0.0);
        }
        assert_eq!(
            crate::top_chrome_height(),
            bar + crate::TAB_BAR_HEIGHT,
            "上部クロームの合計は 1 箇所で定義する"
        );
    }

    /// トリガーは上端のリサイズエッジ（`frame_y` ≒ 8px @100% DPI）を避けて置く。
    /// ここが 0 だと 100% DPI でメニューがクリックできない（モジュール doc の罠）
    #[test]
    fn メニュートリガーは上端リサイズエッジを避ける() {
        let top = MENU_TRIGGER_TOP;
        let height = MENU_TRIGGER_HEIGHT;
        assert!(top > 0.0);
        // 「行から溢れない」は行を持つ環境（Windows）だけの不変条件。
        // macOS は `MENU_BAR_HEIGHT == 0` で行を描かないので対象外
        if MENU_BAR_HEIGHT > 0.0 {
            assert!(top + height <= MENU_BAR_HEIGHT, "トリガーが行から溢れない");
        }
    }
}

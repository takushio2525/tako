//! remote_panel — リモート接続の GUI（#283 弾4）
//!
//! - 承認ダイアログ: 未登録端末のペアリング要求（+ role 昇格要求）を Mac 画面に表示し、
//!   ユーザーが role を選んで許可 / 拒否する。**この経路だけがペアリングを承認できる**
//!   （AI フルコントロール不変条件の例外。`.agent/requirements.md`）
//! - status bar インジケータ: daemon 稼働 + 接続中端末数を常時表示。クリックで
//!   端末一覧ポップオーバー（接続状態・role・revoke・kill switch = 全遮断）
//!
//! daemon の状態取得（admin API `/api/admin/state`）と承認 / 拒否 / revoke は
//! HTTP 越しのため background executor で実行し、UI スレッドには結果 snapshot だけを持つ。
//! 絵文字は使わず、状態表示は GPUI の描画プリミティブ（色ドット・SVG アイコン）で行う。

use gpui::{
    div, point, prelude::*, px, svg, BoxShadow, Context, FontWeight, MouseButton, MouseDownEvent,
    SharedString,
};
use serde_json::Value;

use crate::file_icons::ui_icon;
// main.rs のラッパー hsla / rgba / rgba_alpha（tako_core::Rgb を受ける）と TakoApp を使う
use super::{hsla, rgba, rgba_alpha, TakoApp};

/// リモート GUI の UI スレッド状態（admin state の snapshot + ポップオーバー開閉）
#[derive(Default)]
pub struct RemoteUiState {
    /// daemon 稼働中か（/api/admin/state が成功したか）
    pub running: bool,
    /// 登録済み端末（admin state の devices）
    pub devices: Vec<Value>,
    /// 承認待ちのペアリング / 昇格要求（admin state の pending）
    pub pending: Vec<Value>,
    /// デバイス ID → WS 接続数（admin state の connections）
    pub connections: std::collections::HashMap<String, u64>,
    /// 端末一覧ポップオーバーの開閉
    pub panel_open: bool,
    /// 承認ダイアログで選択中の role（デバイス ID → role 文字列）
    pub selected_role: std::collections::HashMap<String, String>,
}

impl RemoteUiState {
    /// 接続中（WS 1 本以上）の端末数
    pub fn connected_count(&self) -> usize {
        self.connections.values().filter(|&&c| c > 0).count()
    }
}

/// admin state を JSON から取り込む（background 取得結果を UI へ反映する）
pub fn apply_admin_state(state: &mut RemoteUiState, value: &Value) {
    state.running = value["running"].as_bool().unwrap_or(false);
    state.devices = value["devices"].as_array().cloned().unwrap_or_default();
    state.pending = value["pending"].as_array().cloned().unwrap_or_default();
    state.connections.clear();
    if let Some(conns) = value["connections"].as_object() {
        for (id, count) in conns {
            state
                .connections
                .insert(id.clone(), count.as_u64().unwrap_or(0));
        }
    }
    // 消えた pending の role 選択は掃除する
    let pending_ids: std::collections::HashSet<&str> = state
        .pending
        .iter()
        .filter_map(|p| p["device_id"].as_str())
        .collect();
    state
        .selected_role
        .retain(|id, _| pending_ids.contains(id.as_str()));
}

/// リモート GUI が最前面に出すオーバーレイの種別。承認ダイアログを最優先にする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOverlay {
    /// ペアリング / 昇格の承認ダイアログ（承認待ちがある）
    Pairing,
    /// 端末一覧ポップオーバー（panel_open）
    Panel,
    /// 何も出さない
    None,
}

/// UI 状態からどのオーバーレイを出すか決める（描画と分離した純粋判定）。
/// 承認待ちがあれば必ず承認ダイアログを最優先で出す = 未登録端末が接続しても
/// Mac 側の承認操作が確実に前面に現れる（#283）
pub fn overlay_kind(state: &RemoteUiState) -> RemoteOverlay {
    if !state.pending.is_empty() {
        RemoteOverlay::Pairing
    } else if state.panel_open {
        RemoteOverlay::Panel
    } else {
        RemoteOverlay::None
    }
}

/// role の 4 段階（弱い順）。承認ダイアログの選択肢に使う
/// （表示ラベルは `ui_text::remote::role_label` が言語別に解決する）
const ROLES: &[&str] = &["observe", "interact", "manage", "admin"];

impl TakoApp {
    /// リモート admin state を background で取得して UI に反映する。
    /// daemon 停止中は running=false になるだけ（エラーにしない）。
    /// 状態確認（PID ファイル読み取り）+ admin API（HTTP）をまとめて background で行い、
    /// UI スレッドをブロックしない（#168 の方針: 外部 I/O は UI スレッドで同期実行しない）
    pub(crate) fn refresh_remote_state(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let value = cx
                .background_executor()
                .spawn(async move {
                    // daemon 稼働中のときだけ admin state を取得する
                    if tako_control::remote::daemon_status()["running"].as_bool() == Some(true) {
                        tako_control::remote::admin_request("GET", "/api/admin/state", None)
                    } else {
                        Ok(serde_json::json!({ "running": false }))
                    }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                let before_running = app.remote.running;
                let before_pending = app.remote.pending.len();
                let before_connected = app.remote.connected_count();
                match value {
                    Ok(v) if v["running"].as_bool() == Some(true) => {
                        apply_admin_state(&mut app.remote, &v)
                    }
                    _ => {
                        // daemon 停止中 or 未起動 or 取得失敗: running=false へ倒す
                        app.remote.running = false;
                        app.remote.devices.clear();
                        app.remote.pending.clear();
                        app.remote.connections.clear();
                    }
                }
                // 表示に影響する変化があったときだけ再描画する（毎 2 秒の無駄 notify を避ける）
                if before_running != app.remote.running
                    || before_pending != app.remote.pending.len()
                    || before_connected != app.remote.connected_count()
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// ペアリング / 昇格要求を承認する（GUI 限定経路。background で admin API を叩く）
    fn remote_approve(&mut self, device_id: String, cx: &mut Context<Self>) {
        let role = self.remote.selected_role.get(&device_id).cloned();
        cx.spawn(async move |this, cx| {
            let body = match &role {
                Some(r) => serde_json::json!({ "device_id": device_id, "role": r }),
                None => serde_json::json!({ "device_id": device_id }),
            };
            let _ = cx
                .background_executor()
                .spawn(async move {
                    tako_control::remote::admin_request(
                        "POST",
                        "/api/admin/pair/approve",
                        Some(&body),
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| app.refresh_remote_state(cx));
        })
        .detach();
    }

    /// ペアリング / 昇格要求を拒否する（GUI 限定経路）
    fn remote_deny(&mut self, device_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let body = serde_json::json!({ "device_id": device_id });
            let _ = cx
                .background_executor()
                .spawn(async move {
                    tako_control::remote::admin_request("POST", "/api/admin/pair/deny", Some(&body))
                })
                .await;
            let _ = this.update(cx, |app, cx| app.refresh_remote_state(cx));
        })
        .detach();
    }

    /// 端末を失効させる（接続中なら即時切断される。CLI / MCP と同じ admin API）
    fn remote_revoke(&mut self, device_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let body = serde_json::json!({ "device_id": device_id });
            let _ = cx
                .background_executor()
                .spawn(async move {
                    tako_control::remote::admin_request(
                        "POST",
                        "/api/admin/devices/revoke",
                        Some(&body),
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| app.refresh_remote_state(cx));
        })
        .detach();
    }

    /// kill switch: 全遮断（`tako remote stop` 相当）。daemon を止めれば全端末が切断される
    fn remote_kill_switch(&mut self, cx: &mut Context<Self>) {
        self.remote.panel_open = false;
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move { tako_control::remote::daemon_stop() })
                .await;
            let _ = this.update(cx, |app, cx| app.refresh_remote_state(cx));
        })
        .detach();
    }

    /// status bar のリモートインジケータ（daemon 稼働 + 接続端末数）。
    /// daemon 停止中は None（表示しない）
    pub(crate) fn render_remote_indicator(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.remote.running {
            return None;
        }
        let theme = &self.theme;
        let connected = self.remote.connected_count();
        let pending = self.remote.pending.len();
        // 接続中は accent、待機のみは text_tertiary。承認待ちがあれば黄色ドットを添える
        let dot_color = if connected > 0 {
            theme.accent
        } else {
            theme.text_tertiary
        };
        let label = if connected > 0 {
            crate::ui_text::remote::connected_count(connected)
        } else {
            "remote".to_string()
        };
        Some(
            div()
                .id("statusbar-remote")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h_full()
                .px(px(11.0))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(hsla(theme.text_tertiary))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
                .hover(|d| d.bg(rgba(theme.surface_hover)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.remote.panel_open = !this.remote.panel_open;
                    cx.notify();
                }))
                // 接続状態ドット（絵文字ではなく描画プリミティブ）
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded_full()
                        .bg(hsla(dot_color)),
                )
                .child(
                    svg()
                        .path(ui_icon::REMOTE)
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(hsla(theme.text_tertiary)),
                )
                .child(SharedString::from(label))
                // 承認待ちバッジ（黄色の件数チップ）
                .when(pending > 0, |d| {
                    d.child(
                        div()
                            .px(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgba_alpha(theme.yellow, 0.25))
                            .text_color(hsla(theme.yellow))
                            .text_size(px(10.5))
                            .child(SharedString::from(crate::ui_text::remote::pending_count(
                                pending,
                            ))),
                    )
                })
                .into_any_element(),
        )
    }

    /// 承認ダイアログ + 端末一覧ポップオーバーのオーバーレイ（ウィンドウ全面）。
    /// 承認待ちがあれば承認ダイアログを最優先で出す
    pub(crate) fn render_remote_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        match overlay_kind(&self.remote) {
            RemoteOverlay::Pairing => {
                // 承認待ちを 1 件ずつ処理する（overlay_kind が Some を保証）
                let req = self.remote.pending.first().expect("overlay_kind の保証");
                Some(self.render_pairing_dialog(req, cx))
            }
            RemoteOverlay::Panel => Some(self.render_remote_panel(cx)),
            RemoteOverlay::None => None,
        }
    }

    /// ペアリング / 昇格承認ダイアログ（GUI 限定経路。#283）
    fn render_pairing_dialog(&self, req: &Value, cx: &mut Context<Self>) -> gpui::Div {
        let theme = &self.theme;
        let device_id = req["device_id"].as_str().unwrap_or("").to_string();
        let name = req["name"]
            .as_str()
            .unwrap_or(crate::ui_text::remote::unnamed_device())
            .to_string();
        let login = req["login"].as_str().unwrap_or("").to_string();
        let node_name = req["node_name"].as_str().unwrap_or("").to_string();
        let requested_role = req["requested_role"]
            .as_str()
            .unwrap_or("observe")
            .to_string();
        let is_upgrade = req["kind"].as_str() == Some("upgrade");
        // 選択中 role（未選択なら要求 role を既定に）
        let selected = self
            .remote
            .selected_role
            .get(&device_id)
            .cloned()
            .unwrap_or_else(|| requested_role.clone());

        let title = if is_upgrade {
            crate::ui_text::remote::approve_role_change_title()
        } else {
            crate::ui_text::remote::approve_connect_title()
        };

        let mut role_buttons = div().flex().flex_col().gap_1();
        for value in ROLES {
            let label = crate::ui_text::remote::role_label(value);
            let value = value.to_string();
            let is_sel = selected == value;
            let did = device_id.clone();
            role_buttons = role_buttons.child(
                div()
                    .id(SharedString::from(format!("remote-role-{value}")))
                    .px_3()
                    .py_1()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .border_1()
                    .when(is_sel, |d| {
                        d.border_color(hsla(theme.accent))
                            .text_color(hsla(theme.accent))
                            .bg(rgba_alpha(theme.accent, 0.1))
                    })
                    .when(!is_sel, |d| {
                        d.border_color(hsla(theme.border_subtle))
                            .text_color(hsla(theme.tab_inactive_foreground))
                    })
                    .text_size(px(12.5))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote.selected_role.insert(did.clone(), value.clone());
                        cx.notify();
                    }))
                    .child(SharedString::from(label.to_string())),
            );
        }

        let did_approve = device_id.clone();
        let did_deny = device_id.clone();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000088))
            .child(
                div()
                    .w(px(400.0))
                    .p_4()
                    .rounded(px(12.0))
                    .bg(rgba(theme.tab_bar_background))
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .shadow(vec![BoxShadow {
                        color: gpui::rgba(0x00000066).into(),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(24.0),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.foreground))
                            .child(SharedString::from(title.to_string())),
                    )
                    // 端末情報（名前・identity・ノード名）
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(crate::ui_text::remote::device_name(
                                &name,
                            )))
                            .child(SharedString::from(crate::ui_text::remote::device_user(
                                &login,
                            )))
                            .when(!node_name.is_empty(), |d| {
                                d.child(SharedString::from(crate::ui_text::remote::device_node(
                                    &node_name,
                                )))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(hsla(theme.text_tertiary))
                            .child(crate::ui_text::remote::choose_role()),
                    )
                    .child(role_buttons)
                    // アクション: 拒否 / 許可
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(
                                div()
                                    .id("remote-deny")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .bg(rgba_alpha(theme.red, 0.2))
                                    .text_color(hsla(theme.red))
                                    .text_size(px(12.5))
                                    .hover(|d| d.bg(rgba_alpha(theme.red, 0.35)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remote_deny(did_deny.clone(), cx);
                                    }))
                                    .child(crate::ui_text::remote::deny()),
                            )
                            .child(
                                div()
                                    .id("remote-approve")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .bg(rgba_alpha(theme.accent, 0.3))
                                    .text_color(hsla(theme.accent))
                                    .text_size(px(12.5))
                                    .hover(|d| d.bg(rgba_alpha(theme.accent, 0.5)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remote_approve(did_approve.clone(), cx);
                                    }))
                                    .child(crate::ui_text::remote::approve()),
                            ),
                    ),
            )
    }

    /// 端末一覧ポップオーバー（接続状態・role・revoke・kill switch）
    fn render_remote_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = &self.theme;
        let mut list = div().flex().flex_col().gap_1();
        if self.remote.devices.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.0))
                    .text_color(hsla(theme.text_tertiary))
                    .child(crate::ui_text::remote::no_devices()),
            );
        }
        for device in &self.remote.devices {
            let id = device["id"].as_str().unwrap_or("").to_string();
            let name = device["name"]
                .as_str()
                .unwrap_or(crate::ui_text::remote::unnamed_device())
                .to_string();
            let role = device["role"].as_str().unwrap_or("observe").to_string();
            let connected = self.remote.connections.get(&id).copied().unwrap_or(0) > 0;
            let dot = if connected {
                theme.accent
            } else {
                theme.text_tertiary
            };
            let did = id.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .bg(rgba(theme.surface_highlight))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(hsla(dot)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(hsla(theme.foreground))
                                            .child(SharedString::from(name)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(hsla(theme.text_tertiary))
                                            .child(SharedString::from(format!(
                                                "{role}{}",
                                                if connected {
                                                    crate::ui_text::remote::connected_suffix()
                                                } else {
                                                    ""
                                                }
                                            ))),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("remote-revoke-{id}")))
                            .px_2()
                            .py_1()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.red))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote_revoke(did.clone(), cx);
                            }))
                            .child(crate::ui_text::remote::revoke()),
                    ),
            );
        }

        // 全面クリックで閉じる背景 + 右下のポップオーバー
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.remote.panel_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .right(px(12.0))
                    .bottom(px(38.0))
                    .w(px(300.0))
                    .p_3()
                    .rounded(px(10.0))
                    .bg(rgba(theme.tab_bar_background))
                    .border_1()
                    .border_color(hsla(theme.border_subtle))
                    .shadow(vec![BoxShadow {
                        color: gpui::rgba(0x00000066).into(),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(24.0),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(crate::ui_text::remote::panel_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(hsla(theme.text_tertiary))
                                    .child(SharedString::from(
                                        crate::ui_text::remote::connections_now(
                                            self.remote.connected_count(),
                                        ),
                                    )),
                            ),
                    )
                    .child(list)
                    // kill switch: 全遮断
                    .child(
                        div()
                            .id("remote-kill-switch")
                            .mt_1()
                            .px_3()
                            .py_1()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .items_center()
                            .flex()
                            .justify_center()
                            .bg(rgba_alpha(theme.red, 0.2))
                            .text_color(hsla(theme.red))
                            .text_size(px(12.0))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.35)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remote_kill_switch(cx);
                            }))
                            .child(crate::ui_text::remote::stop_all()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_admin_stateは接続数と端末を取り込む() {
        let mut state = RemoteUiState::default();
        let value = json!({
            "running": true,
            "devices": [{ "id": "nDEV1", "name": "iPhone", "role": "observe" }],
            "pending": [{ "device_id": "nDEV2", "name": "iPad", "requested_role": "interact" }],
            "connections": { "nDEV1": 2, "nDEV2": 0 },
        });
        apply_admin_state(&mut state, &value);
        assert!(state.running);
        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.connected_count(), 1, "接続数 > 0 の端末のみ数える");
    }

    #[test]
    fn apply_admin_stateは消えたpendingのrole選択を掃除する() {
        let mut state = RemoteUiState::default();
        state
            .selected_role
            .insert("nGONE".to_string(), "admin".to_string());
        state
            .selected_role
            .insert("nDEV2".to_string(), "manage".to_string());
        let value = json!({
            "running": true,
            "devices": [],
            "pending": [{ "device_id": "nDEV2", "name": "iPad", "requested_role": "interact" }],
            "connections": {},
        });
        apply_admin_state(&mut state, &value);
        assert!(
            !state.selected_role.contains_key("nGONE"),
            "消えた要求の選択は掃除"
        );
        assert!(
            state.selected_role.contains_key("nDEV2"),
            "残る要求の選択は保持"
        );
    }

    #[test]
    fn 停止中はconnected_countが0() {
        let state = RemoteUiState::default();
        assert_eq!(state.connected_count(), 0);
    }

    #[test]
    fn overlay_kindは承認待ちを最優先で出す() {
        let mut state = RemoteUiState::default();
        // 何も無ければ None
        assert_eq!(overlay_kind(&state), RemoteOverlay::None);
        // panel_open だけなら Panel
        state.panel_open = true;
        assert_eq!(overlay_kind(&state), RemoteOverlay::Panel);
        // 承認待ちがあれば panel より承認ダイアログが優先
        state.pending = vec![json!({ "device_id": "nX", "name": "iPhone" })];
        assert_eq!(
            overlay_kind(&state),
            RemoteOverlay::Pairing,
            "承認待ちは panel_open より優先して前面に出る"
        );
        // 承認待ちが消えれば panel に戻る
        state.pending.clear();
        assert_eq!(overlay_kind(&state), RemoteOverlay::Panel);
    }
}

//! remote_panel — リモート接続の GUI（#283 弾4 / #590）
//!
//! - 承認ダイアログ: 未登録端末のペアリング要求（+ role 昇格要求）を Mac 画面に表示し、
//!   ユーザーが role を選んで許可 / 拒否する。**この経路だけがペアリングを承認できる**
//!   （AI フルコントロール不変条件の例外。`.agent/requirements.md`）
//! - status bar インジケータ: **daemon の稼働に関わらず常時表示**（#590）。稼働中は
//!   接続中端末数、停止中はオフ表示。クリックで
//!   - 稼働中: 端末一覧ポップオーバー（接続状態・role・revoke・kill switch = 全遮断）
//!   - 停止中: 起動パネル（Tailscale の不足項目 + 起動ボタン + setup コマンド案内）
//!
//! カードは**インジケータの実描画位置へアンカーする**（#615）。ボタンとカードが
//! 離れた場所に出ると対応関係が読み取れないため、`render_remote_indicator` が
//! paint 時に自分の bounds を `RemoteUiState::anchor` へ書き戻し、`remote_popover`
//! がそれを基準に「ボタン直上・左端そろえ」で置く（位置決めは `popover_position`）。
//! カード内の主アクションは起動 ⇔ 停止のトグル（`primary_action`）で、停止しても
//! カードは開いたままにする = 1 か所で往復できる（#615）。
//!
//! daemon の状態取得（admin API `/api/admin/state`）と承認 / 拒否 / revoke / 起動 / 停止は
//! 子プロセス・HTTP・ファイル I/O を伴うため background executor で実行し、UI スレッドには
//! 結果 snapshot だけを持つ（NFR-8: UI スレッドで重 I/O をしない）。
//! 絵文字は使わず、状態表示は GPUI の描画プリミティブ（色ドット・SVG アイコン）で行う。
//!
//! **CLI / MCP との 1:1**（開発不変条件）。GUI は dispatch と同じ操作関数を background で呼ぶ
//! （dispatch::dispatch 自体は同期 API なので UI スレッドを最長 30 秒塞ぐ = 呼べない。
//! git タブ #496 と同じ方式で「操作ロジックは tako-control 側、GUI は呼ぶだけ」を守る）:
//!
//! | GUI 操作 | 実体 | dispatch | CLI | MCP |
//! |---|---|---|---|---|
//! | 起動ボタン | `remote::spawn_daemon` | `RemoteStart`（`RemoteHost::remote_start`） | `tako remote start` | `tako_remote_start` |
//! | 停止ボタン（= kill switch。確認つき） | `remote::daemon_stop` | `RemoteStop`（`RemoteHost::remote_stop`） | `tako remote stop` | `tako_remote_stop` |
//! | 状態ポーリング | `remote::daemon_status` | `RemoteStatus` | `tako remote status` | `tako_remote_status` |
//! | 不足項目の確認 | `remote_setup::check_status` | `RemoteSetup{action:"check"}` | `tako remote setup --answers`（check） | `tako_remote_setup` |
//! | 端末の失効 | admin API `/devices/revoke` | `RemoteDevices{revoke}` | `tako remote devices revoke` | `tako_remote_devices` |

use gpui::{
    canvas, div, point, prelude::*, px, svg, Bounds, BoxShadow, Context, FontWeight, MouseButton,
    MouseDownEvent, Pixels, SharedString, Window,
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
    /// 接続 URL（固定 ts.net URL。daemon 稼働中のみ。token は含まない = #283）
    pub url: Option<String>,
    /// 登録済み端末（admin state の devices）
    pub devices: Vec<Value>,
    /// 承認待ちのペアリング / 昇格要求（admin state の pending）
    pub pending: Vec<Value>,
    /// デバイス ID → WS 接続数（admin state の connections）
    pub connections: std::collections::HashMap<String, u64>,
    /// 端末一覧 / 起動パネルの開閉
    pub panel_open: bool,
    /// 承認ダイアログで選択中の role（デバイス ID → role 文字列）
    pub selected_role: std::collections::HashMap<String, String>,
    /// 起動処理の実行中（#590。二重起動を防ぎつつ「押した」ことを見せる）
    pub starting: bool,
    /// 直近の起動失敗メッセージ（#590。黙って失敗させない）
    pub start_error: Option<String>,
    /// `remote_setup::check_status` の結果（#590。起動パネルを開いたときに background 取得）
    pub setup: Option<Value>,
    /// setup 状態の取得中
    pub setup_loading: bool,
    /// 停止の確認待ち（#615。接続中端末が切れるので即実行しない）
    pub stop_confirm: bool,
    /// 停止処理の実行中（#615。二重停止を防ぎつつ「押した」ことを見せる）
    pub stopping: bool,
    /// 直近の停止失敗メッセージ（#615。起動と同じく黙って失敗させない）
    pub stop_error: Option<String>,
    /// ステータスバーインジケータの**実描画位置**（#615。カードのアンカー）。
    /// paint フェーズでしか分からないので canvas の paint フックから書き戻す
    /// （`text_input_caret` と同じ idiom）
    pub anchor: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

impl RemoteUiState {
    /// 接続中（WS 1 本以上）の端末数
    pub fn connected_count(&self) -> usize {
        self.connections.values().filter(|&&c| c > 0).count()
    }
}

/// ステータスバーインジケータの表示状態（#590）。
/// 描画と分離した純粋判定にして、見た目の分岐をテストできるようにする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteIndicator {
    /// daemon 停止中（クリックで起動パネル）
    Off,
    /// 起動処理中
    Starting,
    /// 停止処理中（#615）
    Stopping,
    /// 稼働中・接続端末なし
    Idle,
    /// 稼働中・接続端末あり（件数）
    Connected(usize),
}

/// UI 状態からインジケータの表示状態を決める。
/// 起動 / 停止処理中は daemon 状態より優先（押した反応をすぐ返す）
pub fn indicator_state(state: &RemoteUiState) -> RemoteIndicator {
    if state.starting {
        return RemoteIndicator::Starting;
    }
    if state.stopping {
        return RemoteIndicator::Stopping;
    }
    if !state.running {
        return RemoteIndicator::Off;
    }
    match state.connected_count() {
        0 => RemoteIndicator::Idle,
        n => RemoteIndicator::Connected(n),
    }
}

/// カードの主アクション（#615。起動 ⇔ 停止のトグル）。
/// 「いまボタンを押すと何が起きるか」を 1 つの型で表し、描画と e2e が同じ判定を見る
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    /// 停止中: 押すと起動する
    Start,
    /// 起動処理中: 押せない
    Starting,
    /// 稼働中: 押すと停止の確認を出す
    Stop,
    /// 停止の確認待ち: 実行 / キャンセルを選ぶ
    StopConfirm,
    /// 停止処理中: 押せない
    Stopping,
}

/// UI 状態からカードの主アクションを決める。
/// 処理中（起動 / 停止）は daemon 状態より優先し、二重実行を構造的に防ぐ
pub fn primary_action(state: &RemoteUiState) -> RemoteAction {
    if state.starting {
        RemoteAction::Starting
    } else if state.stopping {
        RemoteAction::Stopping
    } else if !state.running {
        RemoteAction::Start
    } else if state.stop_confirm {
        RemoteAction::StopConfirm
    } else {
        RemoteAction::Stop
    }
}

/// カードの幅（px）。位置計算と描画で同じ値を使う
pub const CARD_WIDTH: f32 = 320.0;
/// カードとウィンドウ端の最小余白（px）
const CARD_MARGIN: f32 = 8.0;
/// カードとアンカーボタンの間隔（px）
const CARD_GAP: f32 = 6.0;

/// カードの表示位置を決める（#615。純粋関数なのでウィンドウ無しで検査できる）。
///
/// - `anchor`: インジケータボタンの実描画矩形の (left, top)。まだ描かれていなければ `None`
/// - 戻り値: overlay ルート（ウィンドウ全面）に対する `(left, bottom)` の px
///
/// 左端はボタンの左端にそろえ、右端がウィンドウからはみ出すぶんだけ**左へ引き戻す**。
/// コンテキストメニュー（#314 `compute_menu_position`）のようなフリップにしないのは、
/// フリップするとカードがボタンから離れて #615 の「乖離して嫌だ」に逆戻りするため。
/// 下端はボタンの上端 + 間隔（ステータスバーの高さを直書きしない）
pub fn popover_position(
    anchor: Option<(f32, f32)>,
    card_width: f32,
    viewport: (f32, f32),
) -> (f32, f32) {
    let (vw, vh) = viewport;
    // ウィンドウがカードより狭いときは左余白に寄せる（max で min > max を作らない）
    let max_left = (vw - card_width - CARD_MARGIN).max(CARD_MARGIN);
    match anchor {
        Some((x, y)) if x.is_finite() && y.is_finite() && vh.is_finite() => {
            (x.clamp(CARD_MARGIN, max_left), (vh - y + CARD_GAP).max(0.0))
        }
        // 初回フレーム（インジケータ未 paint）はステータスバー直上へフォールバック
        _ => (CARD_MARGIN, super::STATUS_BAR_HEIGHT + CARD_GAP),
    }
}

/// `remote_setup::check_status()` の JSON から「起動を妨げる不足項目」の item キーを抜く（#590）。
///
/// `serve` だけ扱いが違う: 未設定（`not_configured`）は daemon 起動時に tako が自分で
/// 設定するので不足ではない。`conflict` / `error`（tako 管理外の設定が既にある）は
/// 起動が失敗するので不足として扱う（`remote::establish_tailscale_serve` と同じ判定）
pub fn setup_blockers(setup: &Value) -> Vec<String> {
    let Some(items) = setup["items"].as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|entry| {
            let item = entry["item"].as_str()?;
            let status = entry["status"].as_str().unwrap_or("");
            let blocking = if item == "serve" {
                matches!(status, "conflict" | "error")
            } else {
                status == "missing"
            };
            blocking.then(|| item.to_string())
        })
        .collect()
}

/// admin state を JSON から取り込む（background 取得結果を UI へ反映する）
pub fn apply_admin_state(state: &mut RemoteUiState, value: &Value) {
    state.running = value["running"].as_bool().unwrap_or(false);
    state.url = value["url"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
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
    /// 端末一覧ポップオーバー（panel_open + daemon 稼働中）
    Panel,
    /// 起動パネル（panel_open + daemon 停止中。#590）
    Start,
    /// 何も出さない
    None,
}

/// UI 状態からどのオーバーレイを出すか決める（描画と分離した純粋判定）。
/// 承認待ちがあれば必ず承認ダイアログを最優先で出す = 未登録端末が接続しても
/// Mac 側の承認操作が確実に前面に現れる（#283）。
/// daemon 停止中の panel_open は起動パネルへ振り分ける（#590）
pub fn overlay_kind(state: &RemoteUiState) -> RemoteOverlay {
    if !state.pending.is_empty() {
        RemoteOverlay::Pairing
    } else if !state.panel_open {
        RemoteOverlay::None
    } else if state.running {
        RemoteOverlay::Panel
    } else {
        RemoteOverlay::Start
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
                        // daemon 停止中 or 未起動 or 取得失敗: running=false へ倒す。
                        // 外部から daemon を止められた場合もこの経路で停止表示へ戻る（#590）
                        app.remote.running = false;
                        app.remote.url = None;
                        app.remote.devices.clear();
                        app.remote.pending.clear();
                        app.remote.connections.clear();
                        // 止まった以上、停止確認は宙に浮かせない（#615。外部停止でも消える）
                        app.remote.stop_confirm = false;
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

    /// リモートを起動する（#590。`tako remote start` / MCP `tako_remote_start` と同一実体）。
    ///
    /// `spawn_daemon` は子プロセスの起動情報を最長 30 秒待つので、UI スレッドでは絶対に
    /// 呼ばない（NFR-8）。押した直後に `starting` を立てて二重起動を防ぎ、失敗時は
    /// daemon が返した理由（Tailscale の不足項目列挙など）をそのままパネルへ出す
    pub(crate) fn remote_do_start(&mut self, cx: &mut Context<Self>) {
        if self.remote.starting {
            return;
        }
        self.remote.starting = true;
        self.remote.start_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { tako_control::remote::spawn_daemon() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.remote.starting = false;
                match result {
                    Ok(info) => {
                        // URL は次のポーリングでも入るが、起動直後に見せたいので即反映する
                        app.remote.url = info["url"]
                            .as_str()
                            .map(|s| s.to_string())
                            .filter(|s| !s.is_empty());
                        app.remote.start_error = None;
                    }
                    Err(e) => app.remote.start_error = Some(e),
                }
                cx.notify();
                // 起動できたかは daemon 状態が正（起動情報が来ても後段で落ちる余地がある）
                app.refresh_remote_state(cx);
                // 失敗の理由を項目単位で示せるよう setup 状態も取り直す
                if app.remote.start_error.is_some() {
                    app.refresh_remote_setup(cx);
                }
            });
        })
        .detach();
    }

    /// Tailscale の setup 状態（不足項目）を background で取得する（#590）。
    /// `tako remote setup`（check）/ MCP `tako_remote_setup` と同一実体。
    /// `tailscale status --json` の子プロセス実行を伴うので、起動パネルを開いた
    /// ときと再確認ボタンだけで叩く（毎 2 秒のポーリングには載せない。#340 の教訓）
    pub(crate) fn refresh_remote_setup(&mut self, cx: &mut Context<Self>) {
        if self.remote.setup_loading {
            return;
        }
        self.remote.setup_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let value = cx
                .background_executor()
                .spawn(async move { tako_control::remote_setup::check_status() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.remote.setup_loading = false;
                app.remote.setup = Some(value);
                cx.notify();
            });
        })
        .detach();
    }

    /// インジケータのクリック（#590）。パネルを開閉し、停止中に開いたときは
    /// setup 状態を取り直す（開くたびに最新の不足項目を見せる）。
    /// 開き直しで前回の確認待ちが残っていると誤操作になるので毎回畳む（#615）
    pub(crate) fn toggle_remote_panel(&mut self, cx: &mut Context<Self>) {
        self.remote.panel_open = !self.remote.panel_open;
        self.remote.stop_confirm = false;
        if self.remote.panel_open {
            // 前回の失敗表示を引きずらない（起動側と同じ扱い。#615）
            self.remote.stop_error = None;
            if !self.remote.running {
                self.remote.start_error = None;
                self.refresh_remote_setup(cx);
            }
        }
        cx.notify();
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

    /// kill switch の入口（#615）: 押しても即座には止めず、カード内に確認を出す。
    /// 接続中の端末が切断される破壊的操作なので、件数を見せてから実行させる
    pub(crate) fn remote_request_stop(&mut self, cx: &mut Context<Self>) {
        if self.remote.stopping {
            return;
        }
        self.remote.stop_confirm = true;
        self.remote.stop_error = None;
        cx.notify();
    }

    /// 停止確認のキャンセル（#615）
    pub(crate) fn remote_cancel_stop(&mut self, cx: &mut Context<Self>) {
        self.remote.stop_confirm = false;
        cx.notify();
    }

    /// kill switch: 全遮断（`tako remote stop` / MCP `tako_remote_stop` と同一実体）。
    /// daemon を止めれば全端末が切断される。
    ///
    /// **カードは開いたままにする**（#615）: 停止すると `overlay_kind` が起動パネルへ
    /// 切り替わるので、同じ場所で起動へ戻れる（起動 ⇔ 停止の往復が 1 か所で完結する）。
    /// `daemon_stop` はプロセス終了待ちを含むので UI スレッドでは呼ばない（NFR-8）
    pub(crate) fn remote_do_stop(&mut self, cx: &mut Context<Self>) {
        if self.remote.stopping {
            return;
        }
        self.remote.stopping = true;
        self.remote.stop_confirm = false;
        self.remote.stop_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { tako_control::remote::daemon_stop() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.remote.stopping = false;
                if let Err(e) = result {
                    // 黙って失敗させない（起動失敗と同じ扱い）
                    app.remote.stop_error = Some(e);
                } else {
                    // 起動パネルへ切り替わるので、不足項目を最新にしておく
                    app.remote.start_error = None;
                    app.refresh_remote_setup(cx);
                }
                cx.notify();
                // 止まったかは daemon 状態が正
                app.refresh_remote_state(cx);
            });
        })
        .detach();
    }

    /// status bar のリモートインジケータ（#283 + #590）。
    ///
    /// **daemon が停止していても常時表示する**（#590）: 表示が消えるとリモート機能の
    /// 存在ごと GUI から消え、`tako remote start` を知らないと到達できなくなる。
    /// 停止中は中空のリング + オフ文言で、稼働中（塗りドット）と一目で区別できる。
    /// クリックは稼働中 = 端末一覧、停止中 = 起動パネル（`render_remote_overlay`）。
    /// paint 時に自分の矩形を `remote.anchor` へ書き戻し、カードの位置決めに使う（#615）
    pub(crate) fn render_remote_indicator(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = &self.theme;
        let pending = self.remote.pending.len();
        let state = indicator_state(&self.remote);
        let anchor_slot = self.remote.anchor.clone();
        // 状態ごとの色と文言（絵文字は使わない）
        let (dot_color, label, text_color) = match state {
            RemoteIndicator::Off => (
                theme.text_faint,
                crate::ui_text::remote::indicator_off().to_string(),
                theme.text_faint,
            ),
            RemoteIndicator::Starting => (
                theme.yellow,
                crate::ui_text::remote::indicator_starting().to_string(),
                theme.text_muted,
            ),
            RemoteIndicator::Stopping => (
                theme.yellow,
                crate::ui_text::remote::indicator_stopping().to_string(),
                theme.text_muted,
            ),
            RemoteIndicator::Idle => (
                theme.text_tertiary,
                crate::ui_text::remote::indicator_idle().to_string(),
                theme.text_tertiary,
            ),
            RemoteIndicator::Connected(n) => (
                theme.accent,
                crate::ui_text::remote::connected_count(n),
                theme.text_tertiary,
            ),
        };
        // 停止中は中空リング（枠のみ）にして、塗りドット = 稼働中と形でも区別する
        let off = state == RemoteIndicator::Off;
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
            .text_color(hsla(text_color))
            .border_r_1()
            .border_color(hsla(theme.border_subtle))
            .when(self.remote.panel_open, |d| d.bg(rgba(theme.surface_hover)))
            .hover(|d| d.bg(rgba(theme.surface_hover)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_remote_panel(cx)))
            // 実描画位置をカードのアンカーとして記録する（#615）。absolute なので
            // flex 行のレイアウトには一切参加しない（見た目は不変）
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, _| anchor_slot.set(Some(bounds)),
                )
                .absolute()
                .size_full(),
            )
            // 接続状態ドット（絵文字ではなく描画プリミティブ）
            .child(
                div()
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded_full()
                    .when(off, |d| d.border_1().border_color(hsla(dot_color)))
                    .when(!off, |d| d.bg(hsla(dot_color))),
            )
            .child(
                svg()
                    .path(ui_icon::REMOTE)
                    .w(px(13.0))
                    .h(px(13.0))
                    .text_color(hsla(text_color)),
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
            .into_any_element()
    }

    /// 承認ダイアログ + 端末一覧ポップオーバーのオーバーレイ（ウィンドウ全面）。
    /// 承認待ちがあれば承認ダイアログを最優先で出す。
    /// `window` はカードをインジケータ直上へ収めるためのビューポート実寸に使う（#615）
    pub(crate) fn render_remote_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        match overlay_kind(&self.remote) {
            RemoteOverlay::Pairing => {
                // 承認待ちを 1 件ずつ処理する（overlay_kind が Some を保証）
                let req = self.remote.pending.first().expect("overlay_kind の保証");
                Some(self.render_pairing_dialog(req, cx))
            }
            RemoteOverlay::Panel => Some(self.render_remote_panel(window, cx)),
            RemoteOverlay::Start => Some(self.render_remote_start_panel(window, cx)),
            RemoteOverlay::None => None,
        }
    }

    /// ポップオーバーの外枠（背面クリックで閉じる + インジケータ直上の吹き出し）。
    /// 端末一覧（稼働中）と起動パネル（停止中）で共有する。
    /// 位置は `popover_position`（ボタン左端そろえ + ウィンドウ端クランプ。#615）
    fn remote_popover(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        body: gpui::Div,
    ) -> gpui::Div {
        let theme = &self.theme;
        let viewport = window.viewport_size();
        let anchor = self
            .remote
            .anchor
            .get()
            .map(|b| (f32::from(b.origin.x), f32::from(b.origin.y)));
        let (left, bottom) = popover_position(
            anchor,
            CARD_WIDTH,
            (f32::from(viewport.width), f32::from(viewport.height)),
        );
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.remote.panel_open = false;
                    this.remote.stop_confirm = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .bottom(px(bottom))
                    .w(px(CARD_WIDTH))
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
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(body),
            )
    }

    /// クリックでコピーできるコマンド行（sleep-guard ポップオーバー #440 と同方式）
    fn remote_copy_row(
        &self,
        id: &'static str,
        text: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        let to_copy = text.clone();
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .bg(rgba(theme.surface_highlight))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(to_copy.clone()));
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .font_family(theme.font_family.clone())
                    .text_size(px(10.5))
                    .text_color(hsla(theme.foreground))
                    .child(SharedString::from(text)),
            )
            .child(
                svg()
                    .path(ui_icon::COPY)
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(hsla(theme.text_faint)),
            )
    }

    /// カードの主アクションボタン（#615）。起動と停止で**同じ形・同じ場所**にすることで、
    /// 状態に応じて起動 ⇔ 停止に切り替わる = トグルだと分かるようにする。
    /// 「どの状態でどのボタンになるか」の対応表をここ 1 か所に集約し、処理中は
    /// 淡色 + クリック不可（二重実行の防止）。確認待ちは専用ブロックが受けるので出さない
    fn remote_action_button(
        &self,
        action: RemoteAction,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        use crate::ui_text::remote as text;
        let theme = &self.theme;
        // (要素 ID, アイコン, ラベル, 色, 押せるか)
        let (id, icon, label, color, enabled) = match action {
            RemoteAction::Start => (
                "remote-start",
                ui_icon::PLAY,
                text::start_button(),
                theme.accent,
                true,
            ),
            RemoteAction::Starting => (
                "remote-start",
                ui_icon::PLAY,
                text::start_button_busy(),
                theme.accent,
                false,
            ),
            RemoteAction::Stop => (
                "remote-kill-switch",
                ui_icon::STOP,
                text::stop_button(),
                theme.red,
                true,
            ),
            RemoteAction::Stopping => (
                "remote-kill-switch",
                ui_icon::STOP,
                text::stop_button_busy(),
                theme.red,
                false,
            ),
            // 確認待ちはボタンではなく確認ブロックを出す
            RemoteAction::StopConfirm => return None,
        };
        Some(
            div()
                .id(SharedString::from(id))
                .mt_1()
                .px_3()
                .py_1()
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .when(!enabled, |d| {
                    d.bg(rgba(theme.surface_highlight))
                        .text_color(hsla(theme.text_tertiary))
                })
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .bg(rgba_alpha(color, 0.3))
                        .text_color(hsla(color))
                        .hover(|d| d.bg(rgba_alpha(color, 0.5)))
                        .on_click(cx.listener(move |this, _, _, cx| match action {
                            RemoteAction::Start => this.remote_do_start(cx),
                            // 停止は破壊的なのでまず確認（実行は remote_do_stop）
                            RemoteAction::Stop => this.remote_request_stop(cx),
                            _ => {}
                        }))
                })
                .child(
                    svg()
                        .path(icon)
                        .w(px(11.0))
                        .h(px(11.0))
                        .flex_none()
                        .text_color(hsla(if enabled { color } else { theme.text_tertiary })),
                )
                .child(label),
        )
    }

    /// 起動パネル（#590）。daemon 停止中にインジケータをクリックしたときに出る。
    /// Tailscale の不足項目（CLI と同じ判定 = `remote_setup::check_status`）を先に見せ、
    /// 起動ボタンは常に押せる（daemon が返す理由が最終的な正なので、黙って封じない）
    fn render_remote_start_panel(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        use crate::ui_text::remote as text;
        let theme = &self.theme;
        let action = primary_action(&self.remote);
        let blockers = self
            .remote
            .setup
            .as_ref()
            .map(setup_blockers)
            .unwrap_or_default();
        let checked = self.remote.setup.is_some();

        // セットアップ状態（確認中 / 準備 OK / 不足項目の列挙）
        let mut status_block = div().flex().flex_col().gap_1();
        if self.remote.setup_loading && !checked {
            status_block = status_block.child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_tertiary))
                    .child(text::setup_checking()),
            );
        } else if checked && blockers.is_empty() {
            status_block = status_block.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        svg()
                            .path(ui_icon::CHECK)
                            .w(px(11.0))
                            .h(px(11.0))
                            .flex_none()
                            .text_color(hsla(theme.green)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.text_tertiary))
                            .child(text::setup_ready()),
                    ),
            );
        } else if checked {
            status_block = status_block.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        svg()
                            .path(ui_icon::WARNING)
                            .w(px(11.0))
                            .h(px(11.0))
                            .flex_none()
                            .text_color(hsla(theme.yellow)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.yellow))
                            .child(text::setup_missing_header()),
                    ),
            );
            for item in &blockers {
                // 未知の item キーでも黙って消さず、キーをそのまま出す
                let label = match text::setup_item_label(item) {
                    "" => item.clone(),
                    l => l.to_string(),
                };
                status_block = status_block.child(
                    div()
                        .pl(px(16.0))
                        .text_size(px(10.5))
                        .line_height(px(15.0))
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(format!("\u{00B7} {label}"))),
                );
            }
        }

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            // ヘッダー（タイトル + 再確認）
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                svg()
                                    .path(ui_icon::REMOTE)
                                    .w(px(13.0))
                                    .h(px(13.0))
                                    .flex_none()
                                    .text_color(hsla(theme.text_tertiary)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(text::start_panel_title()),
                            ),
                    )
                    .child(
                        div()
                            .id("remote-recheck")
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_tertiary))
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_remote_setup(cx)))
                            .child(text::recheck_button()),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .line_height(px(15.0))
                    .text_color(hsla(theme.text_muted))
                    .child(text::start_panel_desc()),
            )
            .child(status_block)
            // 起動ボタン（停止ボタンと同じ形。#615 のトグル）
            .children(self.remote_action_button(action, cx))
            // 起動失敗の理由（daemon / CLI が返した文面をそのまま出す = 黙って失敗させない）
            .children(self.remote.start_error.as_ref().map(|err| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(rgba_alpha(theme.red, 0.12))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.red))
                            .child(text::start_failed()),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(10.0))
                            .line_height(px(14.0))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(err.clone())),
                    )
            }))
            // セットアップ手順（不足があるときだけ）
            .when(checked && !blockers.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.text_faint))
                        .child(text::setup_hint()),
                )
                .child(self.remote_copy_row(
                    "remote-copy-setup",
                    text::SETUP_COMMAND.to_string(),
                    cx,
                ))
            });

        self.remote_popover(window, cx, body)
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

    /// 端末一覧ポップオーバー（接続状態・role・revoke・停止トグル = kill switch）
    fn render_remote_panel(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        use crate::ui_text::remote as text;
        let theme = &self.theme;
        let action = primary_action(&self.remote);
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

        let body = div()
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
                            .child(SharedString::from(crate::ui_text::remote::connections_now(
                                self.remote.connected_count(),
                            ))),
                    ),
            )
            // 接続 URL（#590: GUI から起動した人が次にどこへ繋ぐか分かるようにする。
            // token を含まない固定 ts.net URL なので画面に出しても安全 = #283）
            .children(self.remote.url.clone().map(|url| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_faint))
                            .child(crate::ui_text::remote::url_label()),
                    )
                    .child(self.remote_copy_row("remote-copy-url", url, cx))
            }))
            .child(list)
            // 停止トグル = kill switch（全遮断）。確認を挟んでから実行する（#615）
            .children(match action {
                // 確認待ち: 何が起きるかを件数つきで示し、実行 / キャンセルを選ばせる
                RemoteAction::StopConfirm => Some(
                    div()
                        .mt_1()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .py(px(7.0))
                        .rounded(px(6.0))
                        .bg(rgba_alpha(theme.red, 0.12))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    svg()
                                        .path(ui_icon::WARNING)
                                        .w(px(11.0))
                                        .h(px(11.0))
                                        .flex_none()
                                        .text_color(hsla(theme.red)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.red))
                                        .child(text::stop_confirm_title()),
                                ),
                        )
                        .child(
                            div()
                                .min_w(px(0.0))
                                .text_size(px(10.5))
                                .line_height(px(15.0))
                                .text_color(hsla(theme.text_muted))
                                .child(SharedString::from(text::stop_confirm_body(
                                    self.remote.connected_count(),
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .justify_end()
                                .child(
                                    div()
                                        .id("remote-stop-cancel")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .text_size(px(12.0))
                                        .text_color(hsla(theme.text_tertiary))
                                        .hover(|d| d.bg(rgba(theme.surface_hover)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.remote_cancel_stop(cx);
                                        }))
                                        .child(crate::ui_text::common::cancel()),
                                )
                                .child(
                                    div()
                                        .id("remote-stop-confirm")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .bg(rgba_alpha(theme.red, 0.3))
                                        .text_color(hsla(theme.red))
                                        .text_size(px(12.0))
                                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.5)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.remote_do_stop(cx);
                                        }))
                                        .child(text::stop_confirm_yes()),
                                ),
                        ),
                ),
                // 通常 / 停止処理中: 起動ボタンと同じ形のトグル（下の行が出す）
                _ => None,
            })
            .children(self.remote_action_button(action, cx))
            // 停止失敗の理由（起動失敗と同じ扱い = 黙って失敗させない）
            .children(self.remote.stop_error.as_ref().map(|err| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(rgba_alpha(theme.red, 0.12))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.red))
                            .child(text::stop_failed()),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(10.0))
                            .line_height(px(14.0))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(err.clone())),
                    )
            }));

        // 全面クリックで閉じる背景 + インジケータ直上のポップオーバー（起動パネルと共有）
        self.remote_popover(window, cx, body)
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
    fn apply_admin_stateは接続urlを取り込む() {
        let mut state = RemoteUiState::default();
        apply_admin_state(
            &mut state,
            &json!({ "running": true, "url": "https://mac.tail1234.ts.net" }),
        );
        assert_eq!(
            state.url.as_deref(),
            Some("https://mac.tail1234.ts.net"),
            "起動後に接続先を GUI へ出すため url を保持する（#590）"
        );
        // url が空文字・欠落なら None（空行を描画しない）
        apply_admin_state(&mut state, &json!({ "running": true, "url": "" }));
        assert_eq!(state.url, None);
        apply_admin_state(&mut state, &json!({ "running": true }));
        assert_eq!(state.url, None);
    }

    #[test]
    fn indicator_stateは停止中と稼働中を区別する() {
        let mut state = RemoteUiState::default();
        // 既定（daemon 停止中）は Off = ステータスバーから消えない（#590）
        assert_eq!(indicator_state(&state), RemoteIndicator::Off);
        // 起動処理中は daemon 状態より優先（押した反応を即返す）
        state.starting = true;
        assert_eq!(indicator_state(&state), RemoteIndicator::Starting);
        state.starting = false;
        // 稼働中・接続なし
        state.running = true;
        assert_eq!(indicator_state(&state), RemoteIndicator::Idle);
        // 稼働中・接続あり
        state.connections.insert("nDEV1".to_string(), 2);
        state.connections.insert("nDEV2".to_string(), 0);
        assert_eq!(indicator_state(&state), RemoteIndicator::Connected(1));
    }

    #[test]
    fn setup_blockersは不足項目のキーを返す() {
        // check_status の実形（remote_setup::check_status）に合わせた JSON
        let setup = json!({
            "ready": false,
            "items": [
                { "item": "tailscale", "status": "ok", "detail": "/opt/homebrew/bin/tailscale" },
                { "item": "daemon", "status": "ok" },
                { "item": "login", "status": "missing", "detail": "NeedsLogin" },
                { "item": "https", "status": "missing" },
                { "item": "dns_name", "status": "missing", "detail": "unknown" },
            ],
        });
        assert_eq!(
            setup_blockers(&setup),
            vec!["login", "https", "dns_name"],
            "missing の item キーだけを順序どおり返す"
        );
    }

    #[test]
    fn setup_blockersはserve未設定を不足に数えない() {
        // serve は daemon 起動時に tako が自分で設定する（NotConfigured は正常経路）
        let ok = json!({
            "ready": true,
            "items": [
                { "item": "tailscale", "status": "ok" },
                { "item": "daemon", "status": "ok" },
                { "item": "login", "status": "ok" },
                { "item": "https", "status": "ok" },
                { "item": "dns_name", "status": "ok" },
                { "item": "serve", "status": "not_configured" },
            ],
        });
        assert!(
            setup_blockers(&ok).is_empty(),
            "serve 未設定は起動を妨げない"
        );
        // tako 管理外の serve 設定は起動が失敗するので不足として出す
        let conflict = json!({
            "ready": true,
            "items": [{ "item": "serve", "status": "conflict", "detail": "カスタム設定が存在" }],
        });
        assert_eq!(setup_blockers(&conflict), vec!["serve"]);
        // items が無い / 壊れた JSON でも panic しない
        assert!(setup_blockers(&json!({})).is_empty());
    }

    #[test]
    fn overlay_kindは承認待ちを最優先で出す() {
        let mut state = RemoteUiState::default();
        // 何も無ければ None
        assert_eq!(overlay_kind(&state), RemoteOverlay::None);
        // panel_open + 稼働中なら端末一覧
        state.running = true;
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

    #[test]
    fn overlay_kindは停止中のクリックを起動パネルへ振り分ける() {
        let mut state = RemoteUiState {
            panel_open: true,
            ..Default::default()
        };
        assert_eq!(
            overlay_kind(&state),
            RemoteOverlay::Start,
            "daemon 停止中のクリックは起動導線を出す（#590）"
        );
        // 起動が成功して running になったら端末一覧へ切り替わる（開いたままでよい）
        state.running = true;
        assert_eq!(overlay_kind(&state), RemoteOverlay::Panel);
        // 閉じていれば何も出さない
        state.panel_open = false;
        assert_eq!(overlay_kind(&state), RemoteOverlay::None);
        state.running = false;
        assert_eq!(overlay_kind(&state), RemoteOverlay::None);
    }

    // --- #615: 起動 ⇔ 停止トグルとカードのアンカー ---

    #[test]
    fn primary_actionは起動と停止を往復する() {
        let mut state = RemoteUiState {
            panel_open: true,
            ..Default::default()
        };
        // 停止中は起動できる
        assert_eq!(primary_action(&state), RemoteAction::Start);
        // 起動処理中は押せない（二重起動の防止）
        state.starting = true;
        assert_eq!(primary_action(&state), RemoteAction::Starting);
        state.starting = false;
        // 稼働中は停止できる
        state.running = true;
        assert_eq!(primary_action(&state), RemoteAction::Stop);
        // 停止ボタンは確認を挟む（接続端末が切れる破壊的操作）
        state.stop_confirm = true;
        assert_eq!(primary_action(&state), RemoteAction::StopConfirm);
        // 停止処理中は状態より優先（running が落ちる前でも押せない表示）
        state.stopping = true;
        assert_eq!(primary_action(&state), RemoteAction::Stopping);
        state.stopping = false;
        state.stop_confirm = false;
        // 停止しきったら同じカードが起動へ戻る = 往復
        state.running = false;
        assert_eq!(primary_action(&state), RemoteAction::Start);
    }

    #[test]
    fn indicator_stateは停止処理中を区別する() {
        let mut state = RemoteUiState {
            running: true,
            stopping: true,
            ..Default::default()
        };
        assert_eq!(indicator_state(&state), RemoteIndicator::Stopping);
        // 起動処理中の方が優先（同時に立つことはないが順序を固定しておく）
        state.starting = true;
        assert_eq!(indicator_state(&state), RemoteIndicator::Starting);
    }

    #[test]
    fn popover_positionはボタン直上へアンカーする() {
        // ステータスバー（高さ 32）左寄りのボタン。ウィンドウは 1200x800
        let (left, bottom) = popover_position(Some((200.0, 768.0)), 320.0, (1200.0, 800.0));
        assert_eq!(left, 200.0, "カード左端はボタン左端にそろえる");
        assert_eq!(bottom, 800.0 - 768.0 + 6.0, "ボタン上端の 6px 上に置く");
    }

    #[test]
    fn popover_positionは右端でクランプする() {
        // 右寄りのボタン（1150）だとカード右端が 1470 で画面外 → 左へ引き戻す
        let (left, _) = popover_position(Some((1150.0, 768.0)), 320.0, (1200.0, 800.0));
        assert_eq!(left, 1200.0 - 320.0 - 8.0, "右端の余白を残して収める");
        assert!(left + 320.0 <= 1200.0, "はみ出さない");
        // 左端に寄りすぎても余白を割らない
        let (left, _) = popover_position(Some((-40.0, 768.0)), 320.0, (1200.0, 800.0));
        assert_eq!(left, 8.0);
    }

    #[test]
    fn popover_positionは極端な入力でも破綻しない() {
        // ウィンドウがカードより狭い（clamp の min > max を作らない = panic しない）
        let (left, _) = popover_position(Some((100.0, 300.0)), 320.0, (200.0, 320.0));
        assert_eq!(left, 8.0);
        // まだ描かれていない（初回フレーム）はステータスバー直上へフォールバック
        let (left, bottom) = popover_position(None, 320.0, (1200.0, 800.0));
        assert_eq!(left, 8.0);
        assert_eq!(bottom, super::super::STATUS_BAR_HEIGHT + 6.0);
        // NaN 混入でもフォールバックへ倒す
        let (left, bottom) = popover_position(Some((f32::NAN, 768.0)), 320.0, (1200.0, 800.0));
        assert_eq!(left, 8.0);
        assert_eq!(bottom, super::super::STATUS_BAR_HEIGHT + 6.0);
    }
}

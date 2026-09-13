//! remote_role — 端末の role を動かす経路の**宣言表**と、昇格を PC の人へ縛る判断（#1452）
//!
//! # なぜ表にするのか
//!
//! #1452 で足した操作は「権限の更新をリクエストする」「role を変える」の 2 つだけで、
//! **スマホ側は新しい API を 1 本も呼ばない**（リクエストは #283 からある
//! `POST /api/pair` がそのまま `RequestKind::Upgrade` の保留を作る）。
//!
//! だから危ないのは操作が増えることではなく、**role を動かせる経路が
//! 表に載らないまま増えること**。ここに `purpose`（何をする経路か）・
//! `client_method`（PWA の呼び口）・`layer`（誰が到達できるか）・
//! `grants_upgrade`（昇格させうるか）・`gui_only`（PC の人の操作に限るか）を
//! 1 か所で宣言し、
//!
//! - [`device_role_for`] を `remote::required_role` が引く（= 表が実際の認可を決める）
//! - 番犬 `tests/issue1452_role_grant_watchdog.rs` がソース走査で
//!   「昇格経路が CLI / MCP から叩かれていないか」「PWA の呼び口が表に在るか」を照合する
//!
//! の 2 方向から縛る。**昇格できる経路が生えたら、表に載せて `gui_only` にするまで CI が落ちる。**
//!
//! # 昇格を「PC の人の操作」に縛る 3 段（どれか 1 つでは穴が残る）
//!
//! 1. **方向で切る**（[`decide`]）。降格・据え置きは誰でも通し、**昇格は
//!    呼び出し元が GUI でなければ 403**。CLI / MCP は経路そのものは 1:1 で持つので、
//!    「昇格は断られる」を**実測できる**（経路が無いと 403 という観測ができない）
//! 2. **呼び出し元プロセスのゲート**（[`verify_admin_caller`]）。管理トークンは
//!    0600 = 同一ユーザーなので、**トークンだけでは AI と GUI を区別できない**。
//!    接続元ソケットの所有プロセスを引き（#841 の 1 実装）、実行ファイル**名**が
//!    tako-app のときだけ「PC の人が押した」と見なす
//! 3. **番犬**。経路の増殖・UI の二重実装・無言の失敗をソース走査で落とす
//!
//! ## ゲートが「引けない」ときに閉じない理由
//!
//! [`CallerCheck::Unavailable`] は**通す**（ただし監査へ `caller_check=unavailable` と
//! 名乗る）。閉じると、peer を引けない構成 —— `TAKO_REMOTE_ENDPOINT=unix` の UDS
//! （tiny_http が accept 済み fd を渡さないので接続元 pid を引く口が無い）—— で
//! **GUI からの承認そのものが不可能になる**。#841 が Windows の所有者ゲートで採ったのと
//! 同じ物差しで、「positively 別人だと分かったときだけ断る」。
//! 受容した残りは `.agent/threat-model-remote.md` に書く。

use std::net::SocketAddr;

use crate::platform::local_endpoint::{self, Endpoint};
use crate::remote_auth::DeviceRole;

/// tako の GUI バイナリの実行ファイル名（`.app` の中でも dev ビルドでもこれ）。
/// CLI は `tako` で**別名**なので、名前で GUI と CLI を撃ち分けられる
pub const GUI_IMAGE_NAME: &str = "tako-app";

/// 追加で「PC の人の操作」と見なす実行ファイル名（カンマ区切り）。
/// **隔離テストが curl から承認を撃つためだけの口**で、本番で使うものではない
/// （#841 の `TAKO_REMOTE_TRUSTED_PEER_NAMES` と同じ作法）
pub const TRUSTED_ADMIN_NAMES_ENV: &str = "TAKO_REMOTE_TRUSTED_ADMIN_NAMES";

/// 経路が何をするものか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolePurpose {
    /// 端末が「この role がほしい」と申告する（承認待ちを作るだけ。権限は動かない）
    Request,
    /// 自分の登録状態と承認待ちを読む
    SelfStatus,
    /// 承認待ちを承認する（**昇格しうる**）
    Approve,
    /// 承認待ちを拒否する
    Deny,
    /// 保留を介さず role を直接置く（**昇格しうる**）
    SetRole,
    /// 端末と承認待ちの一覧
    List,
    /// 端末の登録を消す
    Revoke,
}

impl RolePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::SelfStatus => "self_status",
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::SetRole => "set_role",
            Self::List => "list",
            Self::Revoke => "revoke",
        }
    }
}

/// 誰がその経路へ到達できるか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteLayer {
    /// tailnet の実在ノードなら到達できる（層①のみ。ペアリング前でも通る）
    Tailnet,
    /// ペアリング済み端末が role 認可を通って到達する（層②）
    Device,
    /// ローカル管理トークン限定（GUI / CLI / MCP。serve 経由は `check_admin` が弾く）
    Admin,
}

/// 1 本の経路の宣言
#[derive(Debug, Clone, Copy)]
pub struct RoleRoute {
    pub purpose: RolePurpose,
    /// PWA（`web/tako-remote/src/api.js`）の呼び口の名前。PWA が呼ばない経路は空文字
    pub client_method: &'static str,
    pub method: &'static str,
    /// 具体形のパス（docs・番犬・テストが使う）
    pub sample_path: &'static str,
    pub layer: RouteLayer,
    /// 層②の経路に要る role（`Device` 以外では意味を持たない）
    pub device_role: Option<DeviceRole>,
    /// この経路を通ると role が**上がりうる**か
    pub grants_upgrade: bool,
    /// 上げるには PC の人の操作（= tako-app からの呼び出し）が要るか
    pub gui_only: bool,
    /// 監査ログ（`<state_dir>/audit.log`）へ残すイベント名
    pub audit_event: Option<&'static str>,
    matches: fn(&str) -> bool,
}

impl RoleRoute {
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method.eq_ignore_ascii_case(method) && (self.matches)(path)
    }
}

fn is_pair(path: &str) -> bool {
    path == "/api/pair"
}
fn is_me(path: &str) -> bool {
    path == "/api/me"
}
fn is_devices(path: &str) -> bool {
    path == "/api/devices"
}
fn is_devices_revoke(path: &str) -> bool {
    path == "/api/devices/revoke"
}
fn is_admin_state(path: &str) -> bool {
    path == "/api/admin/state"
}
fn is_admin_approve(path: &str) -> bool {
    path == "/api/admin/pair/approve"
}
fn is_admin_deny(path: &str) -> bool {
    path == "/api/admin/pair/deny"
}
fn is_admin_role(path: &str) -> bool {
    path == "/api/admin/devices/role"
}
fn is_admin_revoke(path: &str) -> bool {
    path == "/api/admin/devices/revoke"
}

/// 経路表（**正本**）。
///
/// `grants_upgrade` が真なのは 2 本だけ（[`RolePurpose::Approve`] と
/// [`RolePurpose::SetRole`]）で、どちらも `gui_only`。この不変条件は
/// 下の単体テストと番犬の両方が見る
pub const ROLE_ROUTES: &[RoleRoute] = &[
    RoleRoute {
        purpose: RolePurpose::Request,
        client_method: "pair",
        method: "POST",
        sample_path: "/api/pair",
        // ペアリング前の端末も通る必要がある（層①のみ）。
        // **申告するだけで権限は 1 ミリも動かない**ので昇格経路ではない
        layer: RouteLayer::Tailnet,
        device_role: None,
        grants_upgrade: false,
        gui_only: false,
        audit_event: Some("upgrade_requested"),
        matches: is_pair,
    },
    RoleRoute {
        purpose: RolePurpose::SelfStatus,
        client_method: "me",
        method: "GET",
        sample_path: "/api/me",
        layer: RouteLayer::Tailnet,
        device_role: None,
        grants_upgrade: false,
        gui_only: false,
        audit_event: None,
        matches: is_me,
    },
    RoleRoute {
        purpose: RolePurpose::List,
        client_method: "",
        method: "GET",
        sample_path: "/api/devices",
        layer: RouteLayer::Device,
        device_role: Some(DeviceRole::Admin),
        grants_upgrade: false,
        gui_only: false,
        audit_event: None,
        matches: is_devices,
    },
    RoleRoute {
        purpose: RolePurpose::Revoke,
        client_method: "",
        method: "POST",
        sample_path: "/api/devices/revoke",
        layer: RouteLayer::Device,
        device_role: Some(DeviceRole::Admin),
        grants_upgrade: false,
        gui_only: false,
        audit_event: Some("device_revoked"),
        matches: is_devices_revoke,
    },
    RoleRoute {
        purpose: RolePurpose::List,
        client_method: "",
        method: "GET",
        sample_path: "/api/admin/state",
        layer: RouteLayer::Admin,
        device_role: None,
        grants_upgrade: false,
        gui_only: false,
        audit_event: None,
        matches: is_admin_state,
    },
    RoleRoute {
        purpose: RolePurpose::Approve,
        client_method: "",
        method: "POST",
        sample_path: "/api/admin/pair/approve",
        layer: RouteLayer::Admin,
        device_role: None,
        grants_upgrade: true,
        gui_only: true,
        audit_event: Some("upgrade_approved"),
        matches: is_admin_approve,
    },
    RoleRoute {
        purpose: RolePurpose::Deny,
        client_method: "",
        method: "POST",
        sample_path: "/api/admin/pair/deny",
        layer: RouteLayer::Admin,
        device_role: None,
        grants_upgrade: false,
        gui_only: false,
        audit_event: Some("upgrade_denied"),
        matches: is_admin_deny,
    },
    RoleRoute {
        purpose: RolePurpose::SetRole,
        client_method: "",
        method: "POST",
        sample_path: "/api/admin/devices/role",
        layer: RouteLayer::Admin,
        device_role: None,
        grants_upgrade: true,
        gui_only: true,
        audit_event: Some("role_changed"),
        matches: is_admin_role,
    },
    RoleRoute {
        purpose: RolePurpose::Revoke,
        client_method: "",
        method: "POST",
        sample_path: "/api/admin/devices/revoke",
        layer: RouteLayer::Admin,
        device_role: None,
        grants_upgrade: false,
        gui_only: false,
        audit_event: Some("device_revoked"),
        matches: is_admin_revoke,
    },
];

/// 層②の経路なら必要 role を返す（`remote::required_role` が引く）
pub fn device_role_for(method: &str, path: &str) -> Option<DeviceRole> {
    ROLE_ROUTES
        .iter()
        .find(|r| r.layer == RouteLayer::Device && r.matches(method, path))
        .and_then(|r| r.device_role)
}

/// PWA の呼び口の名前から経路を引く（番犬が使う）
pub fn route_for_client_method(name: &str) -> Option<&'static RoleRoute> {
    ROLE_ROUTES
        .iter()
        .find(|r| !r.client_method.is_empty() && r.client_method == name)
}

/// 昇格しうる経路（番犬と単体テストが「全部 `gui_only` か」を見る）
pub fn upgrade_routes() -> impl Iterator<Item = &'static RoleRoute> {
    ROLE_ROUTES.iter().filter(|r| r.grants_upgrade)
}

// --- 方向の判定 --------------------------------------------------------------

/// role をどちらへ動かすのか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleChange {
    /// 強い role へ（未登録から与えるのもこちら）
    Upgrade,
    /// 弱い role へ
    Downgrade,
    /// 変わらない
    Same,
}

impl RoleChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upgrade => "upgrade",
            Self::Downgrade => "downgrade",
            Self::Same => "same",
        }
    }
}

/// 現 role（`None` = 未登録）から `next` への動きを分類する。
///
/// **未登録 → 何か は必ず昇格**。何も持っていない相手に権限を与えるのは、
/// observe であっても「PC の人が許した」ことにしたい（#283 のペアリング承認と同じ物差し）
pub fn classify(current: Option<DeviceRole>, next: DeviceRole) -> RoleChange {
    match current {
        None => RoleChange::Upgrade,
        Some(cur) if next > cur => RoleChange::Upgrade,
        Some(cur) if next < cur => RoleChange::Downgrade,
        Some(_) => RoleChange::Same,
    }
}

// --- 呼び出し元プロセスのゲート ----------------------------------------------

/// 管理 API の呼び出し元を引いた結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerCheck {
    /// tako-app からの呼び出しだと確かめられた
    Gui,
    /// GUI ではないと**確かめられた**（CLI / MCP / curl / 別プロセス）
    NotGui,
    /// 引けなかった（UDS エンドポイント・peer が閉じた・表を引けない）。
    /// **閉じない**（doc のモジュール説明を参照）
    Unavailable,
}

impl CallerCheck {
    /// 監査・status に載せる安定した識別子（本文・パス・トークンは載せない）
    pub fn code(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::NotGui => "not_gui",
            Self::Unavailable => "unavailable",
        }
    }
}

/// 実行ファイル名が tako の GUI のものか（純関数）。
/// 名前の畳み方は #841 と共有する（`Tako-App.exe` のような形で片方だけ通らないように）
pub fn looks_like_tako_gui(file_name: &str) -> bool {
    let stem = local_endpoint::peer_name_stem(file_name);
    if stem.is_empty() {
        return false;
    }
    if stem == GUI_IMAGE_NAME {
        return true;
    }
    extra_trusted_admin_names().contains(&stem)
}

fn extra_trusted_admin_names() -> Vec<String> {
    std::env::var(TRUSTED_ADMIN_NAMES_ENV)
        .ok()
        .map(|v| {
            v.split(',')
                .map(local_endpoint::peer_name_stem)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 管理 API の接続元が tako-app かを引く（#841 の `verify_peer` と同じ材料・同じ順）。
///
/// 所有者は見ない: 管理 API はもともとローカル管理トークン（0600）で同一ユーザーに
/// 限られていて、ここで撃ち分けたいのは**同じユーザーの中の GUI と CLI** だから
pub fn verify_admin_caller(endpoint: &Endpoint, peer: Option<SocketAddr>) -> CallerCheck {
    let Endpoint::Loopback(local_port) = endpoint else {
        // UDS: 接続元 pid を引く口が無い（accept 済み fd が手に入らない）
        return CallerCheck::Unavailable;
    };
    let Some(peer) = peer else {
        return CallerCheck::Unavailable;
    };
    let Some(found) = tako_core::platform::procinfo::loopback_tcp_peer(peer.port(), *local_port)
    else {
        return CallerCheck::Unavailable;
    };
    let Some(path) = tako_core::platform::procinfo::image_path(found.pid) else {
        return CallerCheck::Unavailable;
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if looks_like_tako_gui(&name) {
        CallerCheck::Gui
    } else {
        CallerCheck::NotGui
    }
}

// --- 判断 --------------------------------------------------------------------

/// role を動かしてよいかの答え（**この関数が唯一の判断**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantDecision {
    Allow,
    /// 403 で断る。`code` は安定した理由コード、`message` は人が読む説明
    Deny {
        code: &'static str,
        message: String,
    },
}

impl GrantDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// 断る理由コード（PWA・CLI・テストが文字列で照合する）
pub const DENY_UPGRADE_REQUIRES_GUI: &str = "upgrade_requires_gui";

/// role の変更を許すか。**降格と据え置きは誰でも通し、昇格は GUI だけ**。
///
/// 純関数なので、番犬はここへ注入（例: `Upgrade` でも `Allow` を返す）して
/// 検出力を実測できる
pub fn decide(change: RoleChange, caller: CallerCheck) -> GrantDecision {
    match change {
        RoleChange::Same | RoleChange::Downgrade => GrantDecision::Allow,
        RoleChange::Upgrade => match caller {
            CallerCheck::Gui | CallerCheck::Unavailable => GrantDecision::Allow,
            CallerCheck::NotGui => GrantDecision::Deny {
                code: DENY_UPGRADE_REQUIRES_GUI,
                message: "権限を上げる操作は tako の画面（設定 → リモート、または承認\
                          ダイアログ）からのみ行えます。CLI / MCP / スマホからは\
                          降格と削除だけができます"
                    .to_string(),
            },
        },
    }
}

/// スマホが書いた理由の整形（制御文字を落として 200 字で切る）。
///
/// **監査ログには載せない**（FR-6.8: 監査に内容を残さない）。出すのは承認ダイアログと
/// ユーザータスクの本文だけ
pub const MAX_REASON_CHARS: usize = 200;

pub fn sanitize_reason(raw: &str) -> String {
    raw.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_REASON_CHARS)
        .collect()
}

// --- ユーザータスク（#1450 kind=permission）への起票と解決 --------------------
//
// **授権の正本ではない**。授権は `DeviceRegistry.pending`（メモリのみ = daemon の
// 再起動で消える安全側）で、ここで作るのは「人がまだ答えていないことの記録」だけ。
// タスクを `done` / `dismiss` しても権限は 1 ミリも動かない。逆向きの経路
// （user_tasks → `remote_auth::approve`）は**存在しない**ことを番犬が縛る。
//
// 起票が要る理由は再起動を跨ぐ受け皿: 承認ダイアログは daemon が落ちれば消えるが、
// このタスクは残るので、ユーザーは後から設定 → リモートで直接与えられる。

/// タスクに 1 本だけ入れる端末の目印。再リクエスト・承認・拒否のときに
/// 「同じ端末の古い依頼」を引き当てるために使う（表示にも出るので人が読める形）
pub fn task_marker(device_id: &str) -> String {
    format!("tako:remote-permission/{device_id}")
}

/// 起票するタスクの題と本文（純関数。番犬と単体テストが直接読む）
pub fn task_title(device_name: &str, requested: DeviceRole) -> String {
    let name = if device_name.trim().is_empty() {
        "(名称未設定)"
    } else {
        device_name.trim()
    };
    format!(
        "リモート端末「{name}」が {} 権限を求めています",
        requested.as_str()
    )
}

/// 本文。**理由はここにだけ載る**（監査ログには載せない = FR-6.8）
pub fn task_body(
    device_name: &str,
    current: Option<DeviceRole>,
    requested: DeviceRole,
    reason: &str,
) -> String {
    let now = current.map(|r| r.as_str()).unwrap_or("未登録");
    let mut body = format!(
        "端末: {device_name}\n現在: {now}\n要求: {}\n\n         許可するには tako の画面で操作します（CLI / MCP からは上げられません）:\n         - 承認ダイアログが出ていれば「許可」\n         - 消えていれば 設定 → リモート → 端末の権限を選び直す",
        requested.as_str()
    );
    let reason = sanitize_reason(reason);
    if !reason.is_empty() {
        body.push_str(&format!("\n\n端末が書いた理由: {reason}"));
    }
    body
}

/// 起票 / 解決の結果（無言で落とさないために呼び出し側が診断へ出す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// 新しく起票した
    Filed(String),
    /// 同じ端末・同じ要求の依頼が既に開いていたので起票しなかった
    Deduped(String),
    /// 対象が無かった（解決時）
    None,
    /// 解決した
    Resolved(String),
    /// app へ届かなかった（daemon 単独稼働・IPC 断）
    Unavailable(String),
}

impl TaskOutcome {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Filed(_) => "filed",
            Self::Deduped(_) => "deduped",
            Self::None => "none",
            Self::Resolved(_) => "resolved",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

/// 同じ端末について開いている permission タスクを引く（id, 題名）。
/// 応答の形だけに依存する純関数なので、テストから JSON を直接食わせられる
pub fn open_task_for_device(list: &serde_json::Value, device_id: &str) -> Option<(String, String)> {
    let marker = task_marker(device_id);
    list.get("tasks")?.as_array()?.iter().find_map(|t| {
        if t.get("status").and_then(|v| v.as_str()) != Some("open") {
            return None;
        }
        if t.get("kind").and_then(|v| v.as_str()) != Some("permission") {
            return None;
        }
        let has = t
            .get("links")?
            .as_array()?
            .iter()
            .any(|l| l.as_str() == Some(marker.as_str()));
        if !has {
            return None;
        }
        Some((
            t.get("id")?.as_str()?.to_string(),
            t.get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        ))
    })
}

/// 権限リクエストをユーザータスクとして起票する（app が居なければ届かないと名乗る）。
///
/// **daemon → app の IPC で `Request::UserTask{add}` を撃つだけ**（CLI / MCP と同じ 1 経路）。
/// #1450 の `notify_user_task_added` がそのまま発火するので、通知欄の口をここで新設しない
pub(crate) fn file_upgrade_task(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
    device_id: &str,
    device_name: &str,
    current: Option<DeviceRole>,
    requested: DeviceRole,
    reason: &str,
) -> TaskOutcome {
    // 同じ端末の依頼が既に開いていれば起票しない（二重リクエストで一覧が溢れない）。
    // daemon を再起動しても効くように、メモリではなく**一覧を引いて**判断する
    if let Some(list) = user_task_request(app_conn, list_open_permission_tasks()) {
        if let Some((id, _)) = open_task_for_device(&list, device_id) {
            return TaskOutcome::Deduped(id);
        }
    }
    let req = crate::protocol::Request::UserTask {
        action: "add".into(),
        id: None,
        title: Some(task_title(device_name, requested)),
        body: Some(task_body(device_name, current, requested, reason)),
        kind: Some("permission".into()),
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: Some(vec![task_marker(device_id)]),
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: None,
        caller_role: None,
    };
    match user_task_request(app_conn, req) {
        Some(v) => match v.get("id").and_then(|x| x.as_str()) {
            Some(id) => TaskOutcome::Filed(id.to_string()),
            None => TaskOutcome::Unavailable("応答に id が無い".into()),
        },
        None => TaskOutcome::Unavailable("tako app へ届かなかった".into()),
    }
}

/// 決着した依頼のタスクを閉じる（承認 = `done` / 拒否・取り下げ = `dismiss`）。
///
/// **タスクを閉じても権限は動かない**（逆向きの操作はこの関数にも存在しない）
pub(crate) fn resolve_upgrade_task(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
    device_id: &str,
    approved: bool,
) -> TaskOutcome {
    let Some(list) = user_task_request(app_conn, list_open_permission_tasks()) else {
        return TaskOutcome::Unavailable("tako app へ届かなかった".into());
    };
    let Some((id, _)) = open_task_for_device(&list, device_id) else {
        return TaskOutcome::None;
    };
    let req = crate::protocol::Request::UserTask {
        action: if approved {
            "done".into()
        } else {
            "dismiss".into()
        },
        id: Some(id.clone()),
        title: None,
        body: None,
        kind: None,
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: None,
        caller_role: None,
    };
    match user_task_request(app_conn, req) {
        Some(_) => TaskOutcome::Resolved(id),
        None => TaskOutcome::Unavailable("tako app へ届かなかった".into()),
    }
}

fn list_open_permission_tasks() -> crate::protocol::Request {
    crate::protocol::Request::UserTask {
        action: "list".into(),
        id: None,
        title: None,
        body: None,
        kind: Some("permission".into()),
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: None,
        caller_role: None,
    }
}

/// daemon → app の 1 往復。**失敗は握り潰さず `None`** にして呼び出し側が診断へ出す
fn user_task_request(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
    request: crate::protocol::Request,
) -> Option<serde_json::Value> {
    crate::remote::with_app_ipc(app_conn, |conn| {
        let client = conn.get()?;
        match client.request(request) {
            Ok(v) => Some(v),
            Err(_) => {
                // 接続は破棄して次回張り直す（#1403 と同じ扱い）
                conn.invalidate();
                None
            }
        }
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 昇格しうる経路はすべてgui_onlyで監査イベントを持つ() {
        let mut count = 0;
        for r in upgrade_routes() {
            count += 1;
            assert!(
                r.gui_only,
                "{} {} が昇格しうるのに gui_only でない（#1452 の不変条件）",
                r.method, r.sample_path
            );
            assert_eq!(
                r.layer,
                RouteLayer::Admin,
                "{} {} が昇格しうるのに管理 API でない",
                r.method,
                r.sample_path
            );
            assert!(
                r.audit_event.is_some(),
                "{} {} が監査イベントを持たない",
                r.method,
                r.sample_path
            );
            assert!(
                r.client_method.is_empty(),
                "{} {} は昇格しうるのに PWA の呼び口を持っている",
                r.method,
                r.sample_path
            );
        }
        assert_eq!(count, 2, "昇格しうる経路は approve と set_role の 2 本だけ");
    }

    #[test]
    fn 表の具体形が自分の判定に当たる() {
        for r in ROLE_ROUTES {
            assert!(
                r.matches(r.method, r.sample_path),
                "{} の sample_path が自分の matches に当たらない",
                r.sample_path
            );
        }
    }

    #[test]
    fn 呼び口の名前は重複しない() {
        let mut names: Vec<&str> = ROLE_ROUTES
            .iter()
            .map(|r| r.client_method)
            .filter(|n| !n.is_empty())
            .collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "client_method が重複: {names:?}");
    }

    #[test]
    fn 層2の経路だけがrole判定を返す() {
        assert_eq!(
            device_role_for("GET", "/api/devices"),
            Some(DeviceRole::Admin)
        );
        assert_eq!(
            device_role_for("POST", "/api/devices/revoke"),
            Some(DeviceRole::Admin)
        );
        // 層①・管理 API は層②の判定に出てこない
        assert_eq!(device_role_for("POST", "/api/pair"), None);
        assert_eq!(device_role_for("GET", "/api/me"), None);
        assert_eq!(device_role_for("POST", "/api/admin/devices/role"), None);
        assert_eq!(device_role_for("POST", "/api/unknown"), None);
    }

    #[test]
    fn classifyは未登録からの付与を昇格として扱う() {
        assert_eq!(classify(None, DeviceRole::Observe), RoleChange::Upgrade);
        assert_eq!(classify(None, DeviceRole::Admin), RoleChange::Upgrade);
        assert_eq!(
            classify(Some(DeviceRole::Observe), DeviceRole::Interact),
            RoleChange::Upgrade
        );
        assert_eq!(
            classify(Some(DeviceRole::Admin), DeviceRole::Observe),
            RoleChange::Downgrade
        );
        assert_eq!(
            classify(Some(DeviceRole::Manage), DeviceRole::Manage),
            RoleChange::Same
        );
    }

    #[test]
    fn 降格と据え置きは呼び出し元を問わず通る() {
        for caller in [
            CallerCheck::Gui,
            CallerCheck::NotGui,
            CallerCheck::Unavailable,
        ] {
            assert!(decide(RoleChange::Downgrade, caller).is_allow());
            assert!(decide(RoleChange::Same, caller).is_allow());
        }
    }

    #[test]
    fn 昇格はguiでないと確かめられたときだけ断る() {
        assert!(decide(RoleChange::Upgrade, CallerCheck::Gui).is_allow());
        // 引けないときは閉じない（UDS / peer 消失で GUI の承認まで止まるため）
        assert!(decide(RoleChange::Upgrade, CallerCheck::Unavailable).is_allow());
        match decide(RoleChange::Upgrade, CallerCheck::NotGui) {
            GrantDecision::Deny { code, .. } => assert_eq!(code, DENY_UPGRADE_REQUIRES_GUI),
            GrantDecision::Allow => panic!("CLI / MCP からの昇格が通ってしまった"),
        }
    }

    #[test]
    fn gui判定は拡張子と大文字小文字を畳む() {
        assert!(looks_like_tako_gui("tako-app"));
        assert!(looks_like_tako_gui("tako-app.exe"));
        assert!(looks_like_tako_gui("Tako-App.EXE"));
        assert!(looks_like_tako_gui("/opt/x/tako-app"));
        // CLI は別名（ここが畳まれると撃ち分けが消える）
        assert!(!looks_like_tako_gui("tako"));
        assert!(!looks_like_tako_gui("tako.exe"));
        assert!(!looks_like_tako_gui("curl"));
        assert!(!looks_like_tako_gui(""));
    }

    #[test]
    fn udsエンドポイントは引けないと名乗る() {
        let ep = Endpoint::Unix(std::path::PathBuf::from("/tmp/x.sock"));
        assert_eq!(verify_admin_caller(&ep, None), CallerCheck::Unavailable);
    }

    #[test]
    fn peerが無いループバックは引けないと名乗る() {
        let ep = Endpoint::Loopback(12345);
        assert_eq!(verify_admin_caller(&ep, None), CallerCheck::Unavailable);
    }

    #[test]
    fn 理由は制御文字を落として200字で切る() {
        assert_eq!(
            sanitize_reason("  ファイルを  見たい \n "),
            "ファイルを 見たい"
        );
        assert_eq!(sanitize_reason("a\u{0}\u{7}b"), "ab");
        let long: String = "あ".repeat(500);
        assert_eq!(sanitize_reason(&long).chars().count(), MAX_REASON_CHARS);
        assert_eq!(sanitize_reason(""), "");
    }
}

//! remote_tasks — スマホ（PWA）から「人がやること」を片付ける経路の**宣言表**と受け口（#1450 B3）
//!
//! # 何を足して、何を足していないか
//!
//! 足したのは **HTTP の受け口 4 本だけ**。中身はすべて B1（#1450）の
//! [`crate::protocol::Request::UserTask`] を素通しで呼ぶ（`list` / `respond` / `done` / `dismiss`）。
//! データモデルも永続も配送も B1 の 1 実装のままで、ここには 1 行も写しを置かない。
//!
//! **添付のダウンロードは経路を 1 本も足していない**（§添付の解決）。既存のファイル API
//! （`/api/files/download?root=…&path=…` = #1079 / #1085）へそのまま載る形に解決して返すので、
//! 認可は [`crate::remote_files::resolve_in_root`] の 1 実装が引き続き正。
//!
//! # なぜ表にするのか
//!
//! #1449 / #1451 / #1452 と同じ理由。危ないのは経路が増えることではなく、
//! **増えた経路が role の判断からこぼれること**（#1405: 未知の GET は `required_role` の
//! 最後で `Observe` へ落ちる）。ここに `purpose` / `client_method`（PWA の呼び口）/
//! `role` / `audit_event` を 1 か所で宣言し、
//!
//! - [`role_for`] を `remote::required_role` が引く（= 表が実際の認可を決める）
//! - 番犬 `tests/issue1450b3_tasks_pwa_watchdog.rs` が「PWA が呼ぶ `/api/tasks…` が
//!   すべて表に在るか」をソース走査で照合する
//!
//! の 2 方向から縛る。**表に無い `/api/tasks…` は安全側（Manage）へ落とす床**も置いてあるので、
//! 受け口だけ先に生えても弱い role へこぼれない。
//!
//! # role の判断（Issue #1450 の記載どおり）
//!
//! - 一覧・詳細は **Observe**。タスクは「ユーザーへの依頼」そのもので、画面を見られる端末なら
//!   読めてよい。**ただし添付の中身は別**で、落とすには Interact 以上 + ルートの解決が要る
//!   （タスク本文と添付の絶対パスが Observe から見える点は受容した判断 = Issue のコメント）
//! - 返答 / 完了 / 却下は **Interact**。`POST /api/panes/*/input`（= 人の代わりに打つ）と
//!   同じ強さに置く: どれも「ユーザーとして意思表示をする」操作で、
//!   返答は起票した master の入力欄へ配送される（B1 = FR-2.40.5）

use crate::remote_auth::DeviceRole;
use crate::remote_files::{self, TreeRoot};
use serde_json::{json, Value};

/// このモジュールが受け持つ接頭辞
pub const TASKS_PREFIX: &str = "/api/tasks";

/// Web Share API に**ファイル**を載せてよい上限。
///
/// `navigator.share({ files })` は Blob を丸ごとメモリへ載せるので、数百 MB の動画を
/// 渡すとスマホのタブが落ちる。大きい添付は「ダウンロード → ファイル / 写真アプリ →
/// 投稿アプリで選ぶ」が実際に通る経路で、そちらは既存のストリーミング配信がそのまま効く。
/// **PWA 側の定数（`tasks.jsx` の `SHARE_MAX_BYTES`）と一致していることを番犬が見る**
pub const SHARE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 経路が何をするためのものか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPurpose {
    /// 一覧 + 詳細 + 未完了件数（画面はこの 1 本で全部描く）
    List,
    /// 返答（decision + comment）を積み、起票 master へ配送する
    Respond,
    /// 片付いた
    Done,
    /// やらない
    Dismiss,
}

impl TaskPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Respond => "respond",
            Self::Done => "done",
            Self::Dismiss => "dismiss",
        }
    }
}

/// 1 本の経路の宣言
#[derive(Debug, Clone, Copy)]
pub struct TaskRoute {
    pub purpose: TaskPurpose,
    /// PWA（`web/tako-remote/src/api.js`）の呼び口の名前
    pub client_method: &'static str,
    pub method: &'static str,
    /// 具体形のパス（docs・番犬・テストが使う。可変部は代表値）
    pub sample_path: &'static str,
    pub role: DeviceRole,
    /// 監査ログ（`<state_dir>/audit.log`）へ残すイベント名。読むだけの経路は残さない
    pub audit_event: Option<&'static str>,
    /// B1 の [`crate::protocol::Request::UserTask`] のどの `action` を呼ぶか（**素通し**）
    pub action: &'static str,
    matches: fn(&str) -> bool,
}

impl TaskRoute {
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method.eq_ignore_ascii_case(method) && (self.matches)(path)
    }
}

fn is_list(path: &str) -> bool {
    path == TASKS_PREFIX
}
fn is_respond(path: &str) -> bool {
    is_verb(path, "respond")
}
fn is_done(path: &str) -> bool {
    is_verb(path, "done")
}
fn is_dismiss(path: &str) -> bool {
    is_verb(path, "dismiss")
}

/// `/api/tasks/<id>/<verb>` の形か（`<id>` は 1 区切りだけ = 階層を作らせない）
fn is_verb(path: &str, verb: &str) -> bool {
    task_id_of(path, verb).is_some()
}

/// `/api/tasks/<id>/<verb>` から `<id>` を取り出す。形が違えば `None`
pub fn task_id_of(path: &str, verb: &str) -> Option<String> {
    let rest = path.strip_prefix(TASKS_PREFIX)?.strip_prefix('/')?;
    let id = rest.strip_suffix(verb)?.strip_suffix('/')?;
    // id に `/` が残る = 階層つきの形。受け付けない（B1 の id は `u-N`）
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id.to_string())
}

/// 経路表（**正本**）。
///
/// 詳細（`show`）の経路を足していないのは、B1 の `list` が 1 件ぶんの全項目
/// （本文 / 添付 / copy_texts / responses / delivery）を返すから。画面は一覧の中身を
/// そのまま開くので往復が増えない（B2 の右パネルと同じ判断）
pub const TASK_ROUTES: &[TaskRoute] = &[
    TaskRoute {
        purpose: TaskPurpose::List,
        client_method: "tasks",
        method: "GET",
        sample_path: "/api/tasks",
        // 「人がやること」の一覧は画面を見られる端末なら読めてよい（Issue #1450 の記載）。
        // 添付の**中身**を落とすのは別の判断（`FILE_ROUTES` の Interact + ルート解決）
        role: DeviceRole::Observe,
        audit_event: None,
        action: "list",
        matches: is_list,
    },
    TaskRoute {
        purpose: TaskPurpose::Respond,
        client_method: "respondTask",
        method: "POST",
        sample_path: "/api/tasks/u-1/respond",
        role: DeviceRole::Interact,
        audit_event: Some("task_respond"),
        action: "respond",
        matches: is_respond,
    },
    TaskRoute {
        purpose: TaskPurpose::Done,
        client_method: "doneTask",
        method: "POST",
        sample_path: "/api/tasks/u-1/done",
        role: DeviceRole::Interact,
        audit_event: Some("task_done"),
        action: "done",
        matches: is_done,
    },
    TaskRoute {
        purpose: TaskPurpose::Dismiss,
        client_method: "dismissTask",
        method: "POST",
        sample_path: "/api/tasks/u-1/dismiss",
        role: DeviceRole::Interact,
        audit_event: Some("task_dismiss"),
        action: "dismiss",
        matches: is_dismiss,
    },
];

/// この表に載っている経路なら必要 role を返す（`remote::required_role` が引く）。
///
/// 表に無い `/api/tasks…` は **Manage** へ落とす（#1405 の「未知の経路が弱い role へ
/// こぼれる」を塞ぐ床）。受け口が無いので実際には 404 になるが、認可を先に通しておくと
/// **将来ここへハンドラを足した人が role を宣言し忘れても弱くならない**
pub fn role_for(method: &str, path: &str) -> Option<DeviceRole> {
    if let Some(role) = TASK_ROUTES
        .iter()
        .find(|r| r.matches(method, path))
        .map(|r| r.role)
    {
        return Some(role);
    }
    owns_path(path).then_some(DeviceRole::Manage)
}

/// このモジュールが受け持つパスか（メソッドを問わない）
pub fn owns_path(path: &str) -> bool {
    path == TASKS_PREFIX || path.starts_with(&format!("{TASKS_PREFIX}/"))
}

/// PWA の呼び口の名前から経路を引く（番犬が使う）
pub fn route_for_client_method(name: &str) -> Option<&'static TaskRoute> {
    TASK_ROUTES.iter().find(|r| r.client_method == name)
}

/// 状態を動かす経路だけ（読むだけの経路は含まない）
pub fn mutating_routes() -> impl Iterator<Item = &'static TaskRoute> {
    TASK_ROUTES.iter().filter(|r| r.audit_event.is_some())
}

/// #1450 B3 の A/B。`TAKO_1450B3_LEGACY=1` で**同一バイナリのまま** B3 以前へ戻す
/// （`/api/tasks…` が 404 = スマホからはタスクが見えない・片付けられない）。
/// PWA 側の逃げ道は `?tako_1450b3_legacy=1` の 1 つだけ
pub fn legacy_1450b3() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1450B3_LEGACY").map(|v| v == "1") == Ok(true))
}

// ============================================================================
// 添付の解決（新しい配信経路を作らない）
// ============================================================================
//
// # 設計: 添付を「既存のファイル API で開ける形」へ翻訳するだけ
//
// タスクの添付は**絶対パス**（master が起票時に書く欄）。ここでその絶対パスを
// [`remote_files::shortcut_target`]（#1451 が持つ 1 実装）で
// `{root, path_rel}` へ翻訳して返し、PWA は既存の
// `/api/files/download?root=…&path=…` をそのまま叩く。
//
// - ルートは [`remote_files::roots_for`] = **その端末の role で組んだ一覧**なので、
//   manage 端末は `fs` ルートでどこの添付でも落とせ、interact 端末は
//   PC のツリーに現に出ているフォルダ配下だけになる（#1079 の約束そのまま）
// - 解決できない添付は `available: false`。画面は「この端末では開けない」と理由を出し、
//   #1452 の権限リクエスト導線へ繋ぐ（黙って押せない行にしない）
//
// **タスク専用の root を作る案は採らない**: 添付のパスは AI（master）が書く欄なので、
// それを根拠に配れるようにすると **AI が弱い端末の読める範囲を広げられる**。
// 全体閲覧を Manage に置いた #1451 の判断と正面から食い違う（Issue #1450 で master 了承済み）。

/// 添付 1 件に「どう開くか」を書き足す（**純粋関数**。ルートは引数で受ける）。
///
/// 足すのは `name` / `size` / `root` / `path_rel` / `available` だけで、
/// B1 が返した `path` / `exists` は 1 バイトも書き換えない
fn decorate_attachment(entry: &mut Value, roots: &[TreeRoot]) {
    let Some(path) = entry
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    entry["name"] = json!(base_name(&path));
    let exists = entry
        .get("exists")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if exists {
        if let Ok(meta) = std::fs::metadata(&path) {
            entry["size"] = json!(meta.len());
        }
    }
    match resolve_target(roots, &path, exists) {
        // 実在しないものは「開ける」と言わない（押せるボタンを出して 404 にしない）
        Some((root, rel)) if exists => {
            entry["root"] = json!(root);
            entry["path_rel"] = json!(rel);
            entry["available"] = json!(true);
        }
        _ => entry["available"] = json!(false),
    }
}

/// 添付の絶対パスを「どのルートの・どの相対パスか」へ解決する。
///
/// 判定そのものは [`remote_files::shortcut_target`]（#1451 の 1 実装・**純粋関数**）。
/// ここが足すのは**綴りの違いの吸収**だけ: ルートは app 側で canonicalize 済みなのに、
/// 添付のパスは master が書いたままの綴りで来るので、`/tmp/…`（実体は `/private/tmp/…`）の
/// ように**同じファイルが別の文字列**だと当たらない（実測で発覚。生成物を `/tmp` や
/// `/var` へ書き出す運用は普通にある）。実在するときだけ実体の綴りでもう一度引く
fn resolve_target(roots: &[TreeRoot], path: &str, exists: bool) -> Option<(String, String)> {
    if let Some(found) = remote_files::shortcut_target(roots, path) {
        return Some(found);
    }
    if !exists {
        return None;
    }
    // Windows の verbatim prefix（`\\?\`）を落とす 1 実装を通す（#970 の境界）。
    // 落とさないと、同じフォルダなのに綴りが変わってルート配下と見なせなくなる
    let canon = tako_core::platform::path::canonicalize(std::path::Path::new(path)).ok()?;
    remote_files::shortcut_target(roots, &canon.to_string_lossy())
}

/// 末尾の名前（`/` と `\` の両方で切る。Windows のパスが混ざっても効く）
fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// 応答（`{tasks:[…]}` = 一覧 / 単体のタスク = respond・done・dismiss）の
/// すべての添付に開き方を書き足す
///
/// **`get_mut` で引く**のが要点: `value["tasks"]` は serde_json の `IndexMut` なので、
/// 無いキーを読むだけで `null` を**書き込んでしまう**（単体のタスク応答に
/// `"tasks": null` が生えて、読み手が一覧と単体を見分けられなくなる = 実測で発覚）
pub fn decorate_response(value: &mut Value, roots: &[TreeRoot]) {
    if let Some(tasks) = value.get_mut("tasks") {
        if let Some(list) = tasks.as_array_mut() {
            for task in list {
                decorate_task(task, roots);
            }
        }
        return;
    }
    decorate_task(value, roots);
}

fn decorate_task(task: &mut Value, roots: &[TreeRoot]) {
    let Some(attachments) = task.get_mut("attachments").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in attachments {
        decorate_attachment(entry, roots);
    }
}

// ============================================================================
// HTTP の受け口
// ============================================================================

/// 受け口が外界へ触るための道具（`remote.rs` が組んで渡す）。
/// `remote_files::FilesDeps` と同じ形にしてあるので、読む側が迷わない
pub struct TasksDeps<'a> {
    /// tako app へ IPC で問い合わせる。**失敗は status つき**で受け取る
    /// （app が「頼み方が悪い」と答えたのか届かなかったのかを取り違えないため。
    /// 分類は `remote.rs` の `app_dispatch` の 1 実装が持つ）
    pub send: &'a dyn Fn(crate::protocol::Request) -> Result<Value, (u16, String)>,
    /// 監査ログへ 1 行足す（event, extra）
    pub audit: &'a dyn Fn(&str, Value),
    /// 応答に付ける CORS ヘッダ（`remote.rs` の 1 実装をそのまま使う）
    pub cors: Vec<tiny_http::Header>,
    /// この要求を出した端末の role。**添付の解決範囲はこの値だけで決まる**
    /// （`remote.rs` の認可が確定させた値をそのまま受け取り、ここで再判定しない）
    pub role: DeviceRole,
}

fn header(name: &[u8], value: &[u8]) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name, value).expect("固定ヘッダ")
}

/// JSON 応答。タスクの本文は機密扱いなので `no-store, private` を必ず付ける
fn respond_json(request: tiny_http::Request, deps: &TasksDeps, status: u16, body: &Value) {
    let mut resp = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header(b"Content-Type", b"application/json"));
    for h in deps.cors.clone() {
        resp = resp.with_header(h);
    }
    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
    let _ = request.respond(resp);
}

/// `/api/tasks*` の受け口。role の検査は呼び出し側（`remote.rs` の共通経路）が済ませている
pub fn handle_tasks_request(
    mut request: tiny_http::Request,
    path: &str,
    url_full: &str,
    deps: &TasksDeps,
) {
    // A/B: B3 以前（= スマホからは見えない）を同一バイナリで再現する
    if legacy_1450b3() {
        return respond_json(
            request,
            deps,
            404,
            &json!({
                "error": "この版にはタスクの画面が無い",
                "error_en": "Tasks are not available in this build",
                "kind": "not_found",
            }),
        );
    }
    let method = request.method().as_str().to_string();
    let Some(route) = TASK_ROUTES.iter().find(|r| r.matches(&method, path)) else {
        // 受け持つパスだがメソッドが違う（405）と、そもそも無いパス（404）を分ける
        let known_path = TASK_ROUTES.iter().any(|r| (r.matches)(path));
        let (status, body) = if known_path {
            (
                405,
                json!({
                    "error": "このメソッドには対応していない",
                    "error_en": "Method not allowed",
                    "kind": "method_not_allowed",
                }),
            )
        } else {
            (404, json!({ "error": "API エンドポイントが見つからない" }))
        };
        return respond_json(request, deps, status, &body);
    };

    let body = if method.eq_ignore_ascii_case("POST") {
        match remote_files::read_json_body(&mut request) {
            Ok(v) => v,
            Err(e) => {
                return respond_json(
                    request,
                    deps,
                    400,
                    &json!({ "error": e, "kind": "bad_body" }),
                )
            }
        }
    } else {
        Value::Null
    };

    let (status, out) = dispatch_route(deps, route, path, url_full, &body);
    if status < 300 {
        if let Some(event) = route.audit_event {
            // 監査には**何をしたか**だけ残す（タスクの本文・コメント・添付のパスは書かない）
            (deps.audit)(
                event,
                json!({ "task": task_id_of(path, route.purpose.as_str()) }),
            );
        }
    }
    respond_json(request, deps, status, &out)
}

/// 表の 1 行を B1 の `Request::UserTask` へ素通しする（**ここに操作の実装は無い**）
fn dispatch_route(
    deps: &TasksDeps,
    route: &TaskRoute,
    path: &str,
    url_full: &str,
    body: &Value,
) -> (u16, Value) {
    let mut req = crate::protocol::Request::UserTask {
        action: route.action.to_string(),
        id: None,
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
        // どこから返した返答かがタスクに残る（B1 の `via`）
        via: Some("pwa".to_string()),
        pane: None,
        caller_role: None,
    };
    let crate::protocol::Request::UserTask {
        id,
        kind,
        status,
        all,
        decision,
        comment,
        ..
    } = &mut req
    else {
        unreachable!("直前に組んだ UserTask")
    };
    match route.purpose {
        TaskPurpose::List => {
            // 既定は未完了のみ（B1 の `list` の既定と同じ）。`all=1` で全部
            *status = remote_files::query_value(url_full, "status").filter(|s| !s.is_empty());
            *kind = remote_files::query_value(url_full, "kind").filter(|s| !s.is_empty());
            *all = remote_files::query_value(url_full, "all").map(|v| v == "1" || v == "true");
        }
        TaskPurpose::Respond => {
            let Some(task) = task_id_of(path, "respond") else {
                return (404, json!({ "error": "タスク id が読み取れない" }));
            };
            *id = Some(task);
            *decision = body["decision"].as_str().map(str::to_string);
            *comment = body["comment"].as_str().map(str::to_string);
        }
        TaskPurpose::Done | TaskPurpose::Dismiss => {
            let Some(task) = task_id_of(path, route.purpose.as_str()) else {
                return (404, json!({ "error": "タスク id が読み取れない" }));
            };
            *id = Some(task);
        }
    }

    match (deps.send)(req) {
        Ok(mut value) => {
            // 添付の開き方を書き足す（**ルートの解決に失敗しても一覧は返す**。
            // 黙って落とさず、理由を `attachment_error` に載せて画面へ出す）
            match remote_files::roots_for(&|r| (deps.send)(r).map_err(|(_, m)| m), deps.role) {
                Ok(roots) => decorate_response(&mut value, &roots),
                Err(e) => {
                    decorate_response(&mut value, &[]);
                    value["attachment_error"] = json!(e);
                }
            }
            (200, value)
        }
        // status は `app_dispatch` の分類（400 = 頼み方が悪い / 500 = 内部 / 503 = app 不在）。
        // 「見つからない」だけは 404 へ寄せる（**無いもの**と**壊れた頼み方**を画面が区別できる）
        Err((status, message)) => {
            let status = if message.contains("見つからない") {
                404
            } else {
                status
            };
            let kind = match status {
                404 => "not_found",
                503 => "app_unreachable",
                _ => "rejected",
            };
            (status, json!({ "error": message, "kind": kind }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表の具体形が自分の判定に当たる() {
        for r in TASK_ROUTES {
            assert!(
                r.matches(r.method, r.sample_path),
                "{} の sample_path（{}）が自分の matches に当たらない",
                r.client_method,
                r.sample_path
            );
            assert_eq!(
                role_for(r.method, r.sample_path),
                Some(r.role),
                "{} の role が表から引けない",
                r.client_method
            );
            assert!(
                owns_path(r.sample_path),
                "{} の sample_path をルータが受け持たない",
                r.client_method
            );
        }
    }

    #[test]
    fn 状態を動かす経路はinteract以上で監査イベントを持つ() {
        for r in mutating_routes() {
            assert!(
                r.role >= DeviceRole::Interact,
                "{} ({} {}) が Interact 未満: 返答 / 完了 / 却下は\
                 「ユーザーとして意思表示をする」操作（#1450）",
                r.client_method,
                r.method,
                r.sample_path
            );
            assert!(
                r.audit_event.is_some(),
                "{} が監査イベントを持たない",
                r.client_method
            );
        }
        // 読むだけの経路は Observe（Issue #1450 の記載）
        let list = route_for_client_method("tasks").expect("一覧の経路");
        assert_eq!(list.role, DeviceRole::Observe);
        assert!(list.audit_event.is_none(), "読むだけの経路は監査に残さない");
    }

    #[test]
    fn 表に無い経路は床のmanageへ落ちる() {
        assert_eq!(role_for("POST", "/api/tasks"), Some(DeviceRole::Manage));
        assert_eq!(
            role_for("GET", "/api/tasks/u-1/respond"),
            Some(DeviceRole::Manage)
        );
        assert_eq!(
            role_for("POST", "/api/tasks/u-1/purge"),
            Some(DeviceRole::Manage),
            "将来足された受け口が弱い role へこぼれない"
        );
        assert_eq!(role_for("GET", "/api/panes"), None, "担当外は何も返さない");
        assert!(!owns_path("/api/tasksfoo"), "接頭辞の取り違え");
    }

    #[test]
    fn 呼び口の名前は重複しない() {
        let mut names: Vec<&str> = TASK_ROUTES.iter().map(|r| r.client_method).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "client_method が重複: {names:?}");
    }

    #[test]
    fn タスクidは1区切りだけ受け取る() {
        assert_eq!(
            task_id_of("/api/tasks/u-12/done", "done").as_deref(),
            Some("u-12")
        );
        assert_eq!(task_id_of("/api/tasks/u-12/respond", "done"), None);
        assert_eq!(
            task_id_of("/api/tasks/a/b/done", "done"),
            None,
            "階層つきの id は受け取らない"
        );
        assert_eq!(task_id_of("/api/tasks//done", "done"), None);
        assert_eq!(task_id_of("/api/tasks/done", "done"), None);
    }

    #[test]
    fn 添付は実在してルートに当たるときだけ開ける() {
        let dir = std::env::temp_dir().join(format!("tako-1450b3-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("out")).expect("一時ディレクトリ");
        let video = dir.join("out/v6.mp4");
        std::fs::write(&video, b"0123456789").expect("偽の添付");
        let roots = vec![TreeRoot {
            id: "aaaaaaaaaaaa".into(),
            path: dir.clone(),
            name: "dir".into(),
            tab: 1,
            tab_title: "t".into(),
            kind: remote_files::RootKind::Tree,
        }];

        let mut value = json!({
            "tasks": [{
                "id": "u-1",
                "attachments": [
                    { "path": video.to_string_lossy(), "exists": true },
                    { "path": "/nowhere/gone.mp4", "exists": false },
                ],
            }]
        });
        decorate_response(&mut value, &roots);
        let a = &value["tasks"][0]["attachments"];
        assert_eq!(a[0]["available"], true);
        assert_eq!(a[0]["root"], "aaaaaaaaaaaa");
        assert_eq!(a[0]["path_rel"], "out/v6.mp4");
        assert_eq!(a[0]["name"], "v6.mp4");
        assert_eq!(a[0]["size"], 10);
        assert_eq!(a[1]["available"], false, "消えた添付は開けると言わない");
        assert_eq!(
            a[1]["name"], "gone.mp4",
            "名前だけは出す（何が消えたか分かる）"
        );
        assert!(a[1]["root"].is_null());

        // ルートが 1 つも無い端末（= 解決できない）
        let mut value2 =
            json!({ "attachments": [{ "path": video.to_string_lossy(), "exists": true }] });
        decorate_response(&mut value2, &[]);
        assert_eq!(value2["attachments"][0]["available"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 綴りが違っても同じファイルなら解決できる() {
        // ルートは app が canonicalize した実体・添付は素の綴り、という食い違いを作る
        let base = tako_core::platform::path::canonicalize(&std::env::temp_dir())
            .expect("一時ディレクトリの実体");
        let dir = base.join(format!("tako-1450b3-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).expect("実体");
        std::fs::write(dir.join("real/v6.mp4"), b"x").expect("添付");
        let link = dir.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("real"), &link).expect("symlink");
        #[cfg(not(unix))]
        let link = dir.join("real");

        let roots = vec![TreeRoot {
            id: "aaaaaaaaaaaa".into(),
            path: dir.join("real"),
            name: "real".into(),
            tab: 1,
            tab_title: "t".into(),
            kind: remote_files::RootKind::Tree,
        }];
        // symlink 経由の綴り（`…/link/v6.mp4`）はそのままではルート配下に見えない
        let spelled = link.join("v6.mp4").to_string_lossy().to_string();
        assert!(
            remote_files::shortcut_target(&roots, &spelled).is_none(),
            "前提が崩れている（素の綴りで当たってしまう）"
        );
        let resolved = resolve_target(&roots, &spelled, true).expect("実体の綴りで解決できる");
        assert_eq!(resolved.0, "aaaaaaaaaaaa");
        assert_eq!(resolved.1, "v6.mp4");
        // 実在しないものは実体を引きに行かない（存在を問い合わせる副作用を増やさない）
        assert!(resolve_target(&roots, &spelled, false).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 単体のタスク応答に一覧のキーが生えない() {
        // `value["tasks"]` で読むと serde_json が `null` を**書き込む**（実測で発覚）。
        // 生えると読み手が「一覧が 0 件」と「1 件の詳細」を見分けられなくなる
        let mut single = json!({ "id": "u-1", "attachments": [] });
        decorate_response(&mut single, &[]);
        assert!(
            single.get("tasks").is_none(),
            "単体の応答に tasks が生えた: {single}"
        );
        assert_eq!(single["id"], "u-1");

        // 添付を持たないタスクでも壊れない
        let mut bare = json!({ "id": "u-2" });
        decorate_response(&mut bare, &[]);
        assert!(bare.get("attachments").is_none(), "添付のキーも生やさない");
    }

    #[test]
    fn 共有の上限はpwaと同じ値を宣言している() {
        // 桁を間違えた変更（64 KB / 64 GB）をこの 1 行で止める
        assert_eq!(SHARE_MAX_BYTES, 67_108_864);
    }
}

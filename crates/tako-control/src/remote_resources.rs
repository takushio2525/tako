//! remote_resources — スマホ（PWA）から PC の負荷と順番待ちを見る・優先度を変える・取り消す経路の
//! **宣言表**と受け口（#1999。エピック #1992 の S7。設計は `.agent/plans/2026-10-resource-manager.md` の §7）
//!
//! # 何を足して、何を足していないか
//!
//! 足したのは **HTTP の受け口 4 本だけ**。中身は FR-2.43 の [`crate::protocol::Request::Queue`] の
//! `status` / `priority` / `cancel` を素通しで呼ぶ。PC の resources ビュー（#1995）も CLI `tako queue` も
//! MCP `tako_queue` も同じ `Request::Queue` を通る（受付番号の操作と status は `ipc::submit` の手前の
//! `queue::answer`、ペインの既定は UI スレッドの dispatch）ので、**並び・段・見込みはどこから読んでも
//! 1 実装**になる。`limits`（上限の書き換え）・`start`（agent の起動待ちを起こす）・wrapper 専用の
//! `enqueue` / `poll` / `started` / `done` は表に載せていない（= 床の Manage へ落ちて 404 になる）。
//!
//! # なぜ表にするのか
//!
//! #1449 / #1450 B3 / #1724 と同じ理由。危ないのは経路が増えることではなく、**増えた経路が role の
//! 判断からこぼれること**（#1405）。ここに `purpose` / `client_method`（PWA の呼び口）/ `role` /
//! `audit_event` を 1 か所で宣言し、
//!
//! - [`role_for`] を `remote::required_role` が引く（= 表が実際の認可を決める）
//! - 番犬 `tests/issue1999_remote_resources_watchdog.rs` が「PWA が呼ぶ `/api/resources…` がすべて表に
//!   在るか」「変更が Manage 以上か」「受け口の二重の門が dispatch より前に在るか」を照合する
//!
//! の 2 方向から縛る。表に無い `/api/resources…` は安全側（Manage）へ落とす床も置く。
//!
//! # role の判断（Issue #1999 の記載どおり）
//!
//! - 読むだけ（機の状態・実行中・待ち）は **Observe**（全役割）。PC の画面を見られる端末なら、
//!   その PC の負荷を読めてよい（ペインのタイトル・コマンドの表示名は `/api/v2/panes` と画面に既に出ている）
//! - 優先度の変更・取り消しは **Manage**。`POST /api/panes/*/close`（ペインを閉じる）と同じ強さ:
//!   取り消しは走っているビルドを止める = 他人の作業を終わらせる操作で、打鍵（Interact）より強い
//!
//! 受け口の中でも role を**もう一度**確かめる（[`handle_resources_request`]）。認可の正は
//! `remote::required_role` + `authorize_device` だが、ルータの配線を誤って表を通らない枝へ繋いでも、
//! 変更だけは弱い端末へ開かない（画面で隠すだけにしない）。
//!
//! # GUI が居ないとき
//!
//! 読む経路だけは CLI `tako queue` と同じく、daemon が機の状態を自分で採って返す
//! （[`crate::resources::machine_only`]。`gui: false` と注記つき）。順番待ちの節は返さない
//! （調停役は GUI の中にしか居ない）。変更は 503（app 不在）で断る。

use crate::remote_auth::DeviceRole;
use crate::remote_files;
use serde_json::{json, Value};

/// このモジュールが受け持つ接頭辞
pub const RESOURCES_PREFIX: &str = "/api/resources";

/// GUI が居ないとき、機全体の CPU% を出すために 2 回採る間隔（CLI `tako queue` と同じ長さ）
const OFFLINE_CPU_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// 経路が何をするためのものか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePurpose {
    /// 機の状態・実行中・待ち（`tako queue --json` の機と順番待ちの節）
    Status,
    /// 受付番号の処理の優先度を変える（`tako queue priority <id> <段>`）
    JobPriority,
    /// ペインから出る処理の優先度の既定を変える（`tako queue priority --pane N <段>`）
    PanePriority,
    /// 受付番号の処理を取り消す（`tako queue cancel <id>`）
    Cancel,
}

impl ResourcePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::JobPriority => "job_priority",
            Self::PanePriority => "pane_priority",
            Self::Cancel => "cancel",
        }
    }
}

/// 1 本の経路の宣言
#[derive(Debug, Clone, Copy)]
pub struct ResourceRoute {
    pub purpose: ResourcePurpose,
    /// PWA（`web/tako-remote/src/api.js`）の呼び口の名前
    pub client_method: &'static str,
    pub method: &'static str,
    /// 具体形のパス（docs・番犬・テストが使う。可変部は代表値）
    pub sample_path: &'static str,
    pub role: DeviceRole,
    /// 監査ログ（`<state_dir>/audit.log`）へ残すイベント名。読むだけの経路は残さない
    pub audit_event: Option<&'static str>,
    /// [`crate::protocol::Request::Queue`] のどの `action` を呼ぶか（**素通し**）
    pub action: &'static str,
    matches: fn(&str) -> bool,
}

impl ResourceRoute {
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method.eq_ignore_ascii_case(method) && (self.matches)(path)
    }
}

fn is_status(path: &str) -> bool {
    path == RESOURCES_PREFIX
}

fn is_job_priority(path: &str) -> bool {
    job_id_of(path, "priority").is_some()
}

fn is_cancel(path: &str) -> bool {
    job_id_of(path, "cancel").is_some()
}

fn is_pane_priority(path: &str) -> bool {
    pane_id_of(path).is_some()
}

/// `/api/resources/<kind>/<id>/<verb>` の `<id>`（10 進の正整数。階層・符号・空は受け付けない）
fn id_of(path: &str, kind: &str, verb: &str) -> Option<u64> {
    let rest = path
        .strip_prefix(RESOURCES_PREFIX)?
        .strip_prefix('/')?
        .strip_prefix(kind)?
        .strip_prefix('/')?;
    let id = rest.strip_suffix(verb)?.strip_suffix('/')?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok().filter(|id| *id > 0)
}

/// `/api/resources/jobs/<id>/<verb>` の受付番号（= wrapper の pid）
pub fn job_id_of(path: &str, verb: &str) -> Option<u64> {
    id_of(path, "jobs", verb)
}

/// `/api/resources/panes/<id>/priority` のペイン ID（tako のペイン番号。`/api/v2/panes` の `id` と同じ）
pub fn pane_id_of(path: &str) -> Option<u64> {
    id_of(path, "panes", "priority")
}

/// 経路表（**正本**）
pub const RESOURCE_ROUTES: &[ResourceRoute] = &[
    ResourceRoute {
        purpose: ResourcePurpose::Status,
        client_method: "resources",
        method: "GET",
        sample_path: "/api/resources",
        // PC の画面を見られる端末なら、その PC の負荷を読めてよい（Issue #1999 = 読むだけは全役割）
        role: DeviceRole::Observe,
        audit_event: None,
        action: "status",
        matches: is_status,
    },
    ResourceRoute {
        purpose: ResourcePurpose::JobPriority,
        client_method: "setJobPriority",
        method: "POST",
        sample_path: "/api/resources/jobs/4242/priority",
        // 変更は manage 以上（Issue #1999。ペインを閉じるのと同じ強さ）
        role: DeviceRole::Manage,
        audit_event: Some("queue_priority"),
        action: "priority",
        matches: is_job_priority,
    },
    ResourceRoute {
        purpose: ResourcePurpose::PanePriority,
        client_method: "setPanePriority",
        method: "POST",
        sample_path: "/api/resources/panes/7/priority",
        role: DeviceRole::Manage,
        audit_event: Some("queue_pane_priority"),
        action: "priority",
        matches: is_pane_priority,
    },
    ResourceRoute {
        purpose: ResourcePurpose::Cancel,
        client_method: "cancelJob",
        method: "POST",
        sample_path: "/api/resources/jobs/4242/cancel",
        // 走っているビルドを止める = 他人の作業を終わらせる操作（Issue #1999 = manage 以上）
        role: DeviceRole::Manage,
        audit_event: Some("queue_cancel"),
        action: "cancel",
        matches: is_cancel,
    },
];

/// この表に載っている経路なら必要 role を返す（`remote::required_role` が引く）。
///
/// 表に無い `/api/resources…` は **Manage** へ落とす（#1405 の「未知の経路が弱い role へ
/// こぼれる」を塞ぐ床。`remote_cards::role_for` と同じ作り）
pub fn role_for(method: &str, path: &str) -> Option<DeviceRole> {
    if let Some(role) = RESOURCE_ROUTES
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
    path == RESOURCES_PREFIX || path.starts_with(&format!("{RESOURCES_PREFIX}/"))
}

/// PWA の呼び口の名前から経路を引く（番犬が使う）
pub fn route_for_client_method(name: &str) -> Option<&'static ResourceRoute> {
    RESOURCE_ROUTES.iter().find(|r| r.client_method == name)
}

// ============================================================================
// HTTP の受け口
// ============================================================================

/// 受け口が外界へ触るための道具（`remote.rs` が組んで渡す）。
/// `remote_cards::CardsDeps` と同じ形にしてあるので、読む側が迷わない
pub struct ResourcesDeps<'a> {
    /// tako app へ IPC で問い合わせる。**失敗は status つき**で受け取る
    /// （分類は `remote.rs` の `app_dispatch` の 1 実装が持つ）
    pub send: &'a dyn Fn(crate::protocol::Request) -> Result<Value, (u16, String)>,
    /// 監査ログへ 1 行足す（event, extra）
    pub audit: &'a dyn Fn(&str, Value),
    /// 応答に付ける CORS ヘッダ（`remote.rs` の 1 実装をそのまま使う）
    pub cors: Vec<tiny_http::Header>,
    /// この要求を出した端末の role（`remote.rs` の認可が確定させた値）
    pub role: DeviceRole,
    /// この要求を出した端末の ID（Tailscale の `Node.StableID`）。persist.log の「どの端末から」に使う
    pub device_id: &'a str,
}

fn header(name: &[u8], value: &[u8]) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name, value).expect("固定ヘッダ")
}

/// JSON 応答。コマンドの表示名・ペインのタイトルが載るので `no-store, private` を必ず付ける
fn respond_json(request: tiny_http::Request, deps: &ResourcesDeps, status: u16, body: &Value) {
    let mut resp = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header(b"Content-Type", b"application/json"));
    for h in deps.cors.clone() {
        resp = resp.with_header(h);
    }
    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
    let _ = request.respond(resp);
}

/// `/api/resources*` の受け口。role の検査は呼び出し側（`remote.rs` の共通経路）が済ませているが、
/// **表の role に届かない端末はここでも断る**（配線を誤っても変更を弱い端末へ開かない）
pub fn handle_resources_request(mut request: tiny_http::Request, path: &str, deps: &ResourcesDeps) {
    let method = request.method().as_str().to_string();
    let Some(route) = RESOURCE_ROUTES.iter().find(|r| r.matches(&method, path)) else {
        // 受け持つパスだがメソッドが違う（405）と、そもそも無いパス（404）を分ける
        let known_path = RESOURCE_ROUTES.iter().any(|r| (r.matches)(path));
        let (status, body) = if known_path {
            (
                405,
                crate::RemoteRefusal::MethodNotAllowed.kind_body("method_not_allowed"),
            )
        } else {
            (
                404,
                json!({ "error": crate::RemoteRefusal::ApiEndpointNotFound.to_string() }),
            )
        };
        return respond_json(request, deps, status, &body);
    };

    // 二重の門（#1999）。正は `remote::required_role` だが、ここを通る時点で
    // 表の role に届いていないなら配線の誤り。**調停役へ頼まずに**断る
    if deps.role < route.role {
        crate::diag::persist_log(&format!(
            "リモートの順番待ち: 表の role に届かない要求を受け口で断った \
             （配線の誤り）: 端末={} 経路={} 必要={} 現在={}",
            deps.device_id,
            route.purpose.as_str(),
            route.role.as_str(),
            deps.role.as_str()
        ));
        return respond_json(
            request,
            deps,
            403,
            &json!({
                "error": crate::RemoteRefusal::RouteRoleTooLow {
                    required: route.role,
                    current: deps.role,
                }
                .to_string(),
                "kind": "forbidden",
            }),
        );
    }

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

    let (status, out) = dispatch_route(deps, route, path, &body);
    if let Some(event) = route.audit_event {
        let outcome = out
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or(if status < 300 { "ok" } else { "rejected" });
        // 監査・診断ログには**何をしたか**だけ残す（コマンドの表示名・ペインのタイトルは書かない）
        let target = match route.purpose {
            ResourcePurpose::PanePriority => {
                format!(
                    "ペイン={}",
                    pane_id_of(path).map_or("?".into(), |p| p.to_string())
                )
            }
            _ => format!(
                "受付番号={}",
                job_id_of(path, route.action).map_or("?".into(), |id| id.to_string())
            ),
        };
        let level = body.get("priority").and_then(Value::as_str);
        (deps.audit)(
            event,
            json!({
                "job": job_id_of(path, route.action),
                "pane": pane_id_of(path),
                "priority": level,
                "outcome": outcome,
            }),
        );
        crate::diag::persist_log(&format!(
            "リモートから順番待ちを操作: 端末={} 操作={} {target}{} 結果={outcome}",
            deps.device_id,
            route.purpose.as_str(),
            level.map(|l| format!(" 段={l}")).unwrap_or_default(),
        ));
    }
    respond_json(request, deps, status, &out)
}

/// 表の 1 行を `Request::Queue` へ素通しする（**ここに操作の実装は無い**）
fn dispatch_route(
    deps: &ResourcesDeps,
    route: &ResourceRoute,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let mut id = None;
    let mut pane = None;
    let mut priority = None;
    match route.purpose {
        ResourcePurpose::Status => {}
        ResourcePurpose::JobPriority | ResourcePurpose::Cancel => {
            let Some(job) = job_id_of(path, route.action) else {
                return refusal(
                    404,
                    crate::RemoteRefusal::ResourceJobIdUnreadable,
                    "not_found",
                );
            };
            id = Some(job);
        }
        ResourcePurpose::PanePriority => {
            let Some(p) = pane_id_of(path) else {
                return refusal(
                    404,
                    crate::RemoteRefusal::ResourcePaneIdUnreadable,
                    "not_found",
                );
            };
            pane = Some(p);
        }
    }
    if matches!(
        route.purpose,
        ResourcePurpose::JobPriority | ResourcePurpose::PanePriority
    ) {
        // 段の読み（top / high / normal / low / default）は調停役 / dispatch の 1 実装に任せる。
        // ここは形（文字列か）だけを見る（CLI の `queue_priority_request` と同じ分担）
        match body.get("priority").and_then(Value::as_str) {
            Some(level) if !level.trim().is_empty() => priority = Some(level.trim().to_string()),
            _ => {
                return refusal(
                    400,
                    crate::RemoteRefusal::ResourcePriorityRequired,
                    "bad_request",
                )
            }
        }
    }
    let req = crate::protocol::Request::Queue {
        action: Some(route.action.to_string()),
        // wrapper 専用の `job` と上限の `limits` はスマホから渡さない（表にも載せない）
        job: None,
        id,
        limits: None,
        pane,
        priority,
    };

    match (deps.send)(req) {
        Ok(value) => match route.purpose {
            ResourcePurpose::Status => (200, status_body(value)),
            _ => (200, value),
        },
        Err((status, message)) => {
            if route.purpose == ResourcePurpose::Status && matches!(status, 502 | 503) {
                // GUI が居ない・繋がらない: CLI `tako queue` と同じく機の状態だけをこの daemon で採る
                return (200, offline_status(&message));
            }
            let (status, kind) = classify_error(status, &message);
            (status, json!({ "error": message, "kind": kind }))
        }
    }
}

fn refusal(status: u16, r: crate::RemoteRefusal, kind: &str) -> (u16, Value) {
    (status, json!({ "error": r.to_string(), "kind": kind }))
}

/// status の答え（`tako queue --json` と同じ形）から、PWA が描く節だけを**手を加えずに**抜き出す。
///
/// 機（`machine`）と順番待ち（`queue`。実行中・待ち・上限・止まっている理由）は 1 バイトも変えない
/// （受付番号・並び・段・見込みが CLI と同じ答えになる = Issue #1999 の受け入れ条件）。
/// 走査の内訳（`panes` / `heavy` / `external` / `scan`）・agent の起動待ち（`launches`）・cargo の札
/// （`jobserver`。fifo のパスを含む）は画面に出さないので端末へ配らない
pub fn status_body(status: Value) -> Value {
    let mut out = json!({
        "gui": status.get("gui").cloned().unwrap_or(Value::Bool(true)),
        "machine": status.get("machine").cloned().unwrap_or(Value::Null),
        "queue": status.get("queue").cloned().unwrap_or(Value::Null),
    });
    if let Some(why) = status.get("machine_unavailable") {
        out["machine_unavailable"] = why.clone();
    }
    out
}

/// GUI が居ない・繋がらないときの読む経路の答え（CLI `tako queue` の手元の腕と同じ中身）
fn offline_status(reason: &str) -> Value {
    let mut local = crate::resources::machine_only(OFFLINE_CPU_WINDOW);
    local["gui"] = Value::Bool(false);
    let mut out = status_body(local);
    // 注記は daemon の表示言語（本文の `error` と同じ。PWA は表示に使うだけで分岐は `gui` で行う）
    out["note"] = json!(crate::ReplyNote::QueueOfflineNote.to_string());
    out["reason"] = json!(reason);
    out
}

/// dispatch の失敗を HTTP の status と種別へ分ける（#1999）。
///
/// status は `app_dispatch` の分類（400 = 頼み方が悪い / 500 = 内部 / 502・503 = app に届かない）。
/// そのうえで**画面が出し分ける 1 つ**だけ寄せ直す:
/// - 受付番号が順番待ちに居ない（押す前に終わった・取り消された）→ 404 `gone`
///   （読みは [`crate::dispatch::Reason::says_queue_job_unknown`]。日英とも）
pub fn classify_error(status: u16, message: &str) -> (u16, &'static str) {
    if matches!(status, 502 | 503) {
        return (503, "app_unreachable");
    }
    if crate::dispatch::Reason::says_queue_job_unknown(message)
        || crate::dispatch::DispatchError::says_not_found(message)
    {
        return (404, "gone");
    }
    (status, "rejected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表の具体形が自分の判定に当たる() {
        for r in RESOURCE_ROUTES {
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
            assert!(owns_path(r.sample_path));
            // 同じ具体形に当たる経路は 1 本だけ（priority と cancel を取り違えない）
            let hits = RESOURCE_ROUTES
                .iter()
                .filter(|o| o.matches(r.method, r.sample_path))
                .count();
            assert_eq!(hits, 1, "{} の具体形が {hits} 本に当たる", r.sample_path);
        }
    }

    #[test]
    fn 読むのはobserveで変更はmanage以上() {
        assert_eq!(role_for("GET", "/api/resources"), Some(DeviceRole::Observe));
        for path in [
            "/api/resources/jobs/12/priority",
            "/api/resources/jobs/12/cancel",
            "/api/resources/panes/3/priority",
        ] {
            assert_eq!(role_for("POST", path), Some(DeviceRole::Manage), "{path}");
        }
        // 表に無い形・メソッド違い・上限の書き換え・wrapper 専用の操作は Manage の床
        for (m, p) in [
            ("POST", "/api/resources"),
            ("GET", "/api/resources/jobs/12/cancel"),
            ("POST", "/api/resources/limits"),
            ("POST", "/api/resources/jobs/12/start"),
            ("POST", "/api/resources/jobs/12/enqueue"),
            ("POST", "/api/resources/jobs/0/cancel"),
            ("POST", "/api/resources/jobs/abc/cancel"),
            ("POST", "/api/resources/jobs/1/2/cancel"),
            ("POST", "/api/resources/panes/3/cancel"),
        ] {
            assert_eq!(role_for(m, p), Some(DeviceRole::Manage), "{m} {p}");
        }
        // 受け持たないパスは表の外（`None` = 他の判定へ回る）
        assert_eq!(role_for("GET", "/api/resourcesx"), None);
        assert_eq!(role_for("GET", "/api/panes"), None);
    }

    #[test]
    fn 受付番号とペインidは正の10進だけを受け付ける() {
        assert_eq!(
            job_id_of("/api/resources/jobs/4242/cancel", "cancel"),
            Some(4242)
        );
        assert_eq!(
            job_id_of("/api/resources/jobs/007/priority", "priority"),
            Some(7)
        );
        assert_eq!(pane_id_of("/api/resources/panes/9/priority"), Some(9));
        for bad in [
            "/api/resources/jobs/0/cancel",
            "/api/resources/jobs//cancel",
            "/api/resources/jobs/-1/cancel",
            "/api/resources/jobs/+1/cancel",
            "/api/resources/jobs/1a/cancel",
            "/api/resources/jobs/1/2/cancel",
            "/api/resources/jobs/99999999999999999999999/cancel",
            "/api/resources/jobs/7/cancelx",
            "/api/resources/jobs/7",
            "/api/resources/panes/7/cancel",
        ] {
            assert_eq!(job_id_of(bad, "cancel"), None, "{bad}");
        }
        assert_eq!(
            job_id_of("/api/resources/panes/7/priority", "priority"),
            None
        );
        assert_eq!(pane_id_of("/api/resources/jobs/7/priority"), None);
    }

    #[test]
    fn statusは機と順番待ちの節を手を加えずに抜き出す() {
        let queue = json!({
            "arbiter": true,
            "running": [{ "id": 11, "label": "cargo build", "priority": "high" }],
            "waiting": [
                { "id": 12, "position": 1, "priority": "top" },
                { "id": 13, "position": 2, "priority": "normal" },
            ],
            "limits": { "max_jobs": 1 },
        });
        let machine = json!({ "memory_mb": { "total": 12288, "used": 4096 }, "pressure": "warn" });
        let full = json!({
            "gui": true,
            "machine": machine,
            "queue": queue,
            "panes": [{ "pane": 1, "title": "x" }],
            "heavy": [],
            "jobserver": { "path": "/var/folders/xx/T/tako-fifo" },
            "launches": { "waiting": [] },
            "scan": { "processes": 1 },
        });
        let out = status_body(full);
        assert_eq!(out["queue"], queue, "順番待ちの節はそのまま");
        assert_eq!(out["machine"], machine, "機の節はそのまま");
        assert_eq!(out["gui"], true);
        for dropped in ["panes", "heavy", "jobserver", "launches", "scan"] {
            assert!(out.get(dropped).is_none(), "{dropped} を端末へ配らない");
        }
        // 機の値を採れない OS の理由は落とさない（画面が「未対応」と書ける）
        let out = status_body(json!({ "machine": null, "machine_unavailable": "unsupported" }));
        assert_eq!(out["machine_unavailable"], "unsupported");
        assert_eq!(out["machine"], Value::Null);
    }

    #[test]
    fn 失敗の分類は受付番号の不在だけを寄せ直す() {
        for lang in [tako_core::i18n::Lang::Ja, tako_core::i18n::Lang::En] {
            let gone = crate::dispatch::Reason::QueueJobUnknown(42).text_in(lang);
            assert_eq!(classify_error(400, &gone), (404, "gone"), "{gone}");
        }
        // cancel の断りは日本語の固定文（`queue::cancel`）。同じ読みで寄る
        assert_eq!(
            classify_error(400, "受付番号 42 は順番待ちに居ない（`tako queue` で一覧）"),
            (404, "gone")
        );
        assert_eq!(
            classify_error(503, "tako app が稼働していない"),
            (503, "app_unreachable")
        );
        assert_eq!(
            classify_error(502, "接続が切れた"),
            (503, "app_unreachable")
        );
        assert_eq!(
            classify_error(400, "未知の優先度: urgent"),
            (400, "rejected")
        );
        assert_eq!(classify_error(500, "内部エラー"), (500, "rejected"));
    }
}

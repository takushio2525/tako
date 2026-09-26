//! remote_cards — スマホ（PWA）からコマンド提案カードを見る・実行する経路の**宣言表**と受け口（#1724）
//!
//! # 何を足して、何を足していないか
//!
//! 足したのは **HTTP の受け口 2 本だけ**。中身は FR-2.22（#666）の
//! [`crate::protocol::Request::ShowCommand`] の `list` / `run` を素通しで呼ぶ。
//! PC のカードの「新規ペインで実行」ボタンも CLI `tako show-command --run` も MCP
//! `tako_show_command` も同じ dispatch（`dispatch_show_command`）を通るので、
//! **実行先のペイン・起動の包み方・実行記録（#1724）はどこから押しても 1 実装**になる。
//!
//! **コマンド本文を受け取る口は作らない**。スマホが渡せるのは「どのカードの何件目か」
//! （カード ID と 1 始まりの番号）だけで、走る文字列は AI が `show` で置いた論理文字列に
//! 限られる。`show`（カードを作る）・`copy`（PC のクリップボードを書く）・`dismiss` は
//! 表に載せていない（= 床の Manage へ落ちて 404 になる）。
//!
//! # なぜ表にするのか
//!
//! #1449 / #1450 B3 / #1451 / #1452 と同じ理由。危ないのは経路が増えることではなく、
//! **増えた経路が role の判断からこぼれること**（#1405）。ここに `purpose` /
//! `client_method`（PWA の呼び口）/ `role` / `audit_event` を 1 か所で宣言し、
//!
//! - [`role_for`] を `remote::required_role` が引く（= 表が実際の認可を決める）
//! - 番犬 `tests/issue1724_remote_card_run_watchdog.rs` が「PWA が呼ぶ `/api/cards…` が
//!   すべて表に在るか」「実行が Interact 以上か」「本文を受け取っていないか」を照合する
//!
//! の 2 方向から縛る。表に無い `/api/cards…` は安全側（Manage）へ落とす床も置く。
//!
//! # role の判断（Issue #1724 の記載どおり）
//!
//! - 一覧は **Observe**。カードは PC の画面に出ているもので、画面を見られる端末なら
//!   読めてよい（view の端末ではカードは見えるが実行は出ない = Issue の記載）
//! - 実行は **Interact**。`POST /api/panes/*/input`（ペインへ打鍵する）と同じ強さ。
//!   打鍵は任意の文字列を送れるので、AI が置いた文字列しか走らない実行はそれより弱い操作で、
//!   Interact を超える強さに置く理由が無い
//!
//! 受け口の中でも role を**もう一度**確かめる（[`handle_cards_request`]）。認可の正は
//! `remote::required_role` + `authorize_device` だが、ルータの配線を誤って表を
//! 通らない枝へ繋いでも、実行だけは弱い端末へ開かない（画面で隠すだけにしない）

use crate::remote_auth::DeviceRole;
use crate::remote_files;
use serde_json::{json, Value};

/// このモジュールが受け持つ接頭辞
pub const CARDS_PREFIX: &str = "/api/cards";

/// 経路が何をするためのものか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPurpose {
    /// そのペインに出ているカードの一覧（コマンドの論理文字列 + 実行記録）
    List,
    /// カードの 1 件を PC で実行する（PC の「新規ペインで実行」と同じ経路）
    Run,
}

impl CardPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Run => "run",
        }
    }
}

/// 1 本の経路の宣言
#[derive(Debug, Clone, Copy)]
pub struct CardRoute {
    pub purpose: CardPurpose,
    /// PWA（`web/tako-remote/src/api.js`）の呼び口の名前
    pub client_method: &'static str,
    pub method: &'static str,
    /// 具体形のパス（docs・番犬・テストが使う。可変部は代表値）
    pub sample_path: &'static str,
    pub role: DeviceRole,
    /// 監査ログ（`<state_dir>/audit.log`）へ残すイベント名。読むだけの経路は残さない
    pub audit_event: Option<&'static str>,
    /// [`crate::protocol::Request::ShowCommand`] のどの `action` を呼ぶか（**素通し**）
    pub action: &'static str,
    matches: fn(&str) -> bool,
}

impl CardRoute {
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method.eq_ignore_ascii_case(method) && (self.matches)(path)
    }
}

fn is_list(path: &str) -> bool {
    path == CARDS_PREFIX
}

fn is_run(path: &str) -> bool {
    card_id_of(path).is_some()
}

/// `/api/cards/<id>/run` から `<id>` を取り出す。形が違えば `None`
/// （id は dispatch のカード ID = 10 進の正整数。階層・符号・空は受け付けない）
pub fn card_id_of(path: &str) -> Option<u64> {
    let rest = path.strip_prefix(CARDS_PREFIX)?.strip_prefix('/')?;
    let id = rest.strip_suffix("/run")?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok().filter(|id| *id > 0)
}

/// 経路表（**正本**）
pub const CARD_ROUTES: &[CardRoute] = &[
    CardRoute {
        purpose: CardPurpose::List,
        client_method: "commandCards",
        method: "GET",
        sample_path: "/api/cards",
        // カードは PC の画面に出ているもの。画面を見られる端末なら読めてよい（Issue #1724）
        role: DeviceRole::Observe,
        audit_event: None,
        action: "list",
        matches: is_list,
    },
    CardRoute {
        purpose: CardPurpose::Run,
        client_method: "runCommandCard",
        method: "POST",
        sample_path: "/api/cards/7/run",
        // ペインへ打鍵できる役割と同じ強さ（`/api/panes/*/input` と同じ。Issue #1724）
        role: DeviceRole::Interact,
        audit_event: Some("card_run"),
        action: "run",
        matches: is_run,
    },
];

/// この表に載っている経路なら必要 role を返す（`remote::required_role` が引く）。
///
/// 表に無い `/api/cards…` は **Manage** へ落とす（#1405 の「未知の経路が弱い role へ
/// こぼれる」を塞ぐ床。`remote_tasks::role_for` と同じ作り）
pub fn role_for(method: &str, path: &str) -> Option<DeviceRole> {
    if let Some(role) = CARD_ROUTES
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
    path == CARDS_PREFIX || path.starts_with(&format!("{CARDS_PREFIX}/"))
}

/// PWA の呼び口の名前から経路を引く（番犬が使う）
pub fn route_for_client_method(name: &str) -> Option<&'static CardRoute> {
    CARD_ROUTES.iter().find(|r| r.client_method == name)
}

// ============================================================================
// HTTP の受け口
// ============================================================================

/// 受け口が外界へ触るための道具（`remote.rs` が組んで渡す）。
/// `remote_tasks::TasksDeps` と同じ形にしてあるので、読む側が迷わない
pub struct CardsDeps<'a> {
    /// tako app へ IPC で問い合わせる。**失敗は status つき**で受け取る
    /// （分類は `remote.rs` の `app_dispatch` の 1 実装が持つ）
    pub send: &'a dyn Fn(crate::protocol::Request) -> Result<Value, (u16, String)>,
    /// 監査ログへ 1 行足す（event, extra）
    pub audit: &'a dyn Fn(&str, Value),
    /// 応答に付ける CORS ヘッダ（`remote.rs` の 1 実装をそのまま使う）
    pub cors: Vec<tiny_http::Header>,
    /// この要求を出した端末の role（`remote.rs` の認可が確定させた値）
    pub role: DeviceRole,
    /// この要求を出した端末の ID（Tailscale の `Node.StableID`）。persist.log の
    /// 「どの端末から」に使う
    pub device_id: &'a str,
}

fn header(name: &[u8], value: &[u8]) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name, value).expect("固定ヘッダ")
}

/// JSON 応答。カードのコマンドは機密扱いなので `no-store, private` を必ず付ける
fn respond_json(request: tiny_http::Request, deps: &CardsDeps, status: u16, body: &Value) {
    let mut resp = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header(b"Content-Type", b"application/json"));
    for h in deps.cors.clone() {
        resp = resp.with_header(h);
    }
    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
    let _ = request.respond(resp);
}

/// `/api/cards*` の受け口。role の検査は呼び出し側（`remote.rs` の共通経路）が済ませているが、
/// **表の role に届かない端末はここでも断る**（配線を誤っても実行を弱い端末へ開かない）
pub fn handle_cards_request(
    mut request: tiny_http::Request,
    path: &str,
    url_full: &str,
    deps: &CardsDeps,
) {
    let method = request.method().as_str().to_string();
    let Some(route) = CARD_ROUTES.iter().find(|r| r.matches(&method, path)) else {
        // 受け持つパスだがメソッドが違う（405）と、そもそも無いパス（404）を分ける
        let known_path = CARD_ROUTES.iter().any(|r| (r.matches)(path));
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

    // 二重の門（#1724）。正は `remote::required_role` だが、ここを通る時点で
    // 表の role に届いていないなら配線の誤り。**実行を走らせずに**断る
    if deps.role < route.role {
        crate::diag::persist_log(&format!(
            "リモートのコマンドカード: 表の role に届かない要求を受け口で断った \
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
                "error": format!(
                    "この操作には {} 以上の role が必要（現在: {}）",
                    route.role.as_str(),
                    deps.role.as_str()
                ),
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

    let (status, out) = dispatch_route(deps, route, path, url_full, &body);
    if route.purpose == CardPurpose::Run {
        let card = card_id_of(path);
        // 番号は**要求の本文**から取る（失敗した応答には番号が載らないので、応答から
        // 取ると「どれを押して断られたか」が監査から落ちる）
        let index = requested_index(&body).ok();
        let outcome = out
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or(if status < 300 { "ok" } else { "rejected" });
        // 監査には**何をしたか**だけ残す（コマンド本文・cwd・ペインの内容は書かない）
        if let Some(event) = route.audit_event {
            (deps.audit)(
                event,
                json!({ "card": card, "index": index, "outcome": outcome }),
            );
        }
        // 診断ログ（persist.log）へも「どの端末から・どのカードを」だけ（Issue #1724 /
        // AGENTS.md の絶対ルール = 送信テキスト・ペイン内容は出さない）
        crate::diag::persist_log(&format!(
            "リモートからコマンドカードを実行: 端末={} カード={} 番号={} 結果={outcome}",
            deps.device_id,
            card.map_or_else(|| "?".to_string(), |c| c.to_string()),
            index.map_or_else(|| "?".to_string(), |i| i.to_string()),
        ));
    }
    respond_json(request, deps, status, &out)
}

/// 表の 1 行を `Request::ShowCommand` へ素通しする（**ここに操作の実装は無い**）
fn dispatch_route(
    deps: &CardsDeps,
    route: &CardRoute,
    path: &str,
    url_full: &str,
    body: &Value,
) -> (u16, Value) {
    let mut pane = None;
    let mut card = None;
    let mut index = None;
    match route.purpose {
        CardPurpose::List => {
            // 一覧はペインごと（PWA のペイン画面 = 数値のペイン ID）。
            // 省略を「呼び出し元のペイン」に倒すと daemon の IPC 接続のフォーカスに依存するので、
            // 明示を必須にする
            match remote_files::query_value(url_full, "pane")
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
            {
                Some(p) => pane = Some(p),
                None => {
                    return (
                        400,
                        json!({
                            "error": "pane（数値のペイン ID）が必要",
                            "kind": "bad_request",
                        }),
                    )
                }
            }
        }
        CardPurpose::Run => {
            let Some(id) = card_id_of(path) else {
                return (
                    404,
                    json!({ "error": "カード id が読み取れない", "kind": "not_found" }),
                );
            };
            card = Some(id);
            match requested_index(body) {
                Ok(n) => index = Some(n),
                Err(()) => {
                    return (
                        400,
                        json!({ "error": "index は 1 始まりの整数", "kind": "bad_request" }),
                    )
                }
            }
        }
    }
    let req = crate::protocol::Request::ShowCommand {
        action: Some(route.action.to_string()),
        // **本文は常に空**。スマホからコマンドの文字列を渡す口は作らない（#1724）
        commands: Vec::new(),
        label: None,
        pane,
        card,
        index,
        // PC のボタンと同じ既定 = 手元のペインのフォーカスを動かさない（FR-2.22.4）
        focus: Some(false),
    };

    match (deps.send)(req) {
        Ok(value) => match route.purpose {
            CardPurpose::List => (200, value),
            // 実行の応答は「どのペインで走り始めたか」と記録だけ返す
            // （本文・cwd はスマホが既に知っているか、知る必要が無い）
            CardPurpose::Run => (
                200,
                json!({
                    "pane": value.get("pane"),
                    "from_pane": value.get("from_pane"),
                    "card": value.get("card"),
                    "index": value.get("index"),
                    "run": value.get("run"),
                }),
            ),
        },
        Err((status, message)) => {
            let (status, kind) = classify_error(status, &message);
            (status, json!({ "error": message, "kind": kind }))
        }
    }
}

/// 実行する番号（1 始まり。PC のカードの番号と同じ）。省略は 1 件目（CLI / MCP の既定と同じ）。
/// 範囲の検査は dispatch（`CommandCard::command`）の 1 実装に任せ、ここは形だけ見る
fn requested_index(body: &Value) -> Result<usize, ()> {
    match body.get("index") {
        None | Some(Value::Null) => Ok(1),
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or(()),
    }
}

/// dispatch の失敗を HTTP の status と種別へ分ける（#1724）。
///
/// status は `app_dispatch` の分類（400 = 頼み方が悪い / 500 = 内部 / 503 = app 不在）。
/// そのうえで**画面が出し分ける 2 つ**だけ寄せ直す:
/// - 前回の実行がまだ走っている → 409 `still_running`（目印は [`STILL_RUNNING_MARK`]）
/// - カード / ペインが見つからない（PC 側で閉じられた）→ 404 `not_found`
///
/// [`STILL_RUNNING_MARK`]: tako_core::command_card::STILL_RUNNING_MARK
pub fn classify_error(status: u16, message: &str) -> (u16, &'static str) {
    if status == 503 {
        return (503, "app_unreachable");
    }
    if message.contains(tako_core::command_card::STILL_RUNNING_MARK) {
        return (409, "still_running");
    }
    if message.contains("見つからない") || message.contains("表示中のコマンドカードが無い")
    {
        return (404, "not_found");
    }
    (status, "rejected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表の具体形が自分の判定に当たる() {
        for r in CARD_ROUTES {
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
        }
    }

    #[test]
    fn 実行はinteract以上で一覧はobserve() {
        assert_eq!(role_for("GET", "/api/cards"), Some(DeviceRole::Observe));
        assert_eq!(
            role_for("POST", "/api/cards/12/run"),
            Some(DeviceRole::Interact)
        );
        // 表に無い形・メソッド違いは Manage の床
        for (m, p) in [
            ("POST", "/api/cards"),
            ("GET", "/api/cards/12/run"),
            ("POST", "/api/cards/12/show"),
            ("POST", "/api/cards/12/copy"),
            ("POST", "/api/cards/12/dismiss"),
            ("POST", "/api/cards/abc/run"),
            ("POST", "/api/cards/0/run"),
            ("POST", "/api/cards/1/2/run"),
        ] {
            assert_eq!(role_for(m, p), Some(DeviceRole::Manage), "{m} {p}");
        }
        // 受け持たないパスは表の外（`None` = 他の判定へ回る）
        assert_eq!(role_for("GET", "/api/cardsx"), None);
        assert_eq!(role_for("GET", "/api/panes"), None);
    }

    #[test]
    fn カードidは正の10進だけを受け付ける() {
        assert_eq!(card_id_of("/api/cards/7/run"), Some(7));
        assert_eq!(card_id_of("/api/cards/007/run"), Some(7));
        for bad in [
            "/api/cards/0/run",
            "/api/cards//run",
            "/api/cards/-1/run",
            "/api/cards/+1/run",
            "/api/cards/1a/run",
            "/api/cards/1/2/run",
            "/api/cards/99999999999999999999999/run",
            "/api/cards/7/runx",
            "/api/cards/7",
        ] {
            assert_eq!(card_id_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn 失敗の分類は実行中と見つからないだけを寄せ直す() {
        let running = tako_core::CommandCardError::StillRunning { index: 1, pane: 9 }.to_string();
        assert_eq!(classify_error(400, &running), (409, "still_running"));
        assert_eq!(
            classify_error(400, "カードが見つからない（id=4）"),
            (404, "not_found")
        );
        assert_eq!(
            classify_error(400, "ペイン 12 が見つからない"),
            (404, "not_found")
        );
        assert_eq!(
            classify_error(400, "このペインに表示中のコマンドカードが無い"),
            (404, "not_found")
        );
        assert_eq!(
            classify_error(503, "tako app が稼働していない"),
            (503, "app_unreachable")
        );
        assert_eq!(
            classify_error(400, "コマンド番号が範囲外（指定: 5、このカードは 1〜1）"),
            (400, "rejected")
        );
        assert_eq!(classify_error(500, "内部エラー"), (500, "rejected"));
    }
}

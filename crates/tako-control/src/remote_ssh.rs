//! リモート（スマホ）からの SSH 切り替え / 新規接続（#1080。エピック #1059 柱 2-H）。
//!
//! # なぜ独立モジュールなのか
//!
//! daemon 側の HTTP ルーティング本体（`remote.rs`）は 6000 行を超えており、
//! リモート刷新の 3 本の柱が同時に手を入れる。ここは **#1080 の判断だけ**を持ち、
//! `remote.rs` 側は「ルートを 1 本ずつこのモジュールへ渡す」だけにしてある。
//!
//! # 設計上の不変条件
//!
//! - **開き先の語彙は増やさない**。`tako_core::remote_open::RemoteOpenTarget`
//!   （`split` / `tab` / `pane`）を GUI / CLI / MCP と共有する（#1006 / #553）
//! - **`can_ssh` の判定を作り直さない**。判定材料（セッションの有無・器つきペインの
//!   内側 alt screen・OSC 133・role）を持っているのは GUI 側だけなので、
//!   `list` 応答に載った答え（`dispatch::can_ssh_json`）をそのまま運ぶ。
//!   daemon が材料から再現すると、器つきペインの外側 alt screen を掴む罠
//!   （#694 / #1006）を必ず踏む
//! - **接続の失敗はペインを消さない**（#919 / #1040）。この層は接続を「始める」だけで、
//!   進み方と失敗の理由は `ssh_connect`（#1010）としてペイン一覧に載り続ける

use serde_json::{json, Value};
use tako_core::remote_open::RemoteOpenTarget;

/// リモートからの接続要求（HTTP のボディ + パスから組み立てた、検証済みの値）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshOpenRequest {
    /// `~/.ssh/config` の Host 名
    pub host: String,
    /// 開き先（既定 = `split` = いま開いているタブへ新ペイン。#1006）
    pub target: RemoteOpenTarget,
    /// 対象ペイン（`target=pane` は SSH 化する相手 / `split` は分割元）
    pub pane: Option<u64>,
    /// 接続後に `cd` するリモートのパス（#919 要件 4）
    pub remote_dir: Option<String>,
}

/// 要求を組み立てられなかった理由（HTTP ステータス + 日本語の理由 + 次の一手）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshRequestError {
    pub status: u16,
    pub message: String,
}

impl SshRequestError {
    fn bad(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }
}

/// ホスト名として受け付けてよいか（**許可制**）。
///
/// # なぜ許可制なのか
///
/// この値は最終的に 2 通りの経路へ流れる:
/// 1. `target=split` / `tab` は `ssh_pane_script` の argv（シェルを経由しない）
/// 2. `target=pane` は**素のシェルへ打つ 1 行**（#640 の送達確認つき経路）
///
/// 2 の経路でも `launch_cmd::quote` が引用するのでメタ文字が実行に化けることは無い
/// （実測: `sh_quote` の素通し集合は英数 + `/ . - _` だけ、PowerShell 側は常に引用）。
/// それでも許可制にするのは、**引用の実装が将来変わっても壊れない**ようにするためと、
/// 空白・制御文字が入ると #640 のエコー照合（打った行と画面の行を突き合わせる）が
/// 静かに空振りするため。
///
/// 許可するのは実在しうるホスト表記に足りるだけ:
/// 英数（IDN のため Unicode も許す）+ `. - _ : @ [ ] % +`
/// （`user@host` / IPv6 の `[::1]` / スコープ付き `fe80::1%en0` / 別名の記号）。
///
/// **一覧を許可リストにはしない**: `Host *` にマッチする相手や
/// `~/.ssh/config` を使わない運用（素の FQDN / IP）があるため
fn host_is_acceptable(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host.chars().all(|c| {
            c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '@' | '[' | ']' | '%' | '+')
        })
}

/// HTTP ボディ（+ `POST /api/panes/:id/ssh` のパス由来ペイン）から要求を組み立てる。
///
/// `path_pane` が `Some` = ペイン指定のルート。この形は
/// **そのペインをそのまま SSH にする**（`target=pane`）のが既定で、
/// ボディで `split` / `tab` を明示すればそのペインを分割元として使う
pub fn parse_open_request(
    body: &Value,
    path_pane: Option<u64>,
) -> Result<SshOpenRequest, SshRequestError> {
    let host = body["host"].as_str().unwrap_or("").trim().to_string();
    if host.is_empty() {
        return Err(SshRequestError::bad(
            "host が必要（`GET /api/ssh-hosts` の name を渡す）",
        ));
    }
    if !host_is_acceptable(&host) {
        return Err(SshRequestError::bad(
            "host に使えない文字が入っている（英数と . - _ : @ [ ] % + のみ。\
             ~/.ssh/config の Host 名を渡す）",
        ));
    }

    let target = match body.get("target") {
        None | Some(Value::Null) => {
            // 既定は経路で変わる: ペイン指定のルートは「このペインを SSH にする」、
            // ペイン非指定のルートは #1006 の既定（いまのタブへ新ペイン）
            if path_pane.is_some() {
                RemoteOpenTarget::Pane
            } else {
                RemoteOpenTarget::default()
            }
        }
        Some(Value::String(s)) => RemoteOpenTarget::parse(s).ok_or_else(|| {
            SshRequestError::bad(format!(
                "target が不正（{}）",
                RemoteOpenTarget::values_hint()
            ))
        })?,
        Some(_) => {
            return Err(SshRequestError::bad(format!(
                "target は文字列（{}）",
                RemoteOpenTarget::values_hint()
            )))
        }
    };

    // 対象ペインはパス優先（`POST /api/panes/:id/ssh` の :id が正）。
    // ボディ側の pane はペイン非指定ルート（`POST /api/ssh`）でだけ効く
    let pane =
        match path_pane {
            Some(p) => Some(p),
            None => match body.get("pane") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_u64().ok_or_else(|| {
                    SshRequestError::bad("pane は数値ペイン ID（一覧の id を渡す）")
                })?),
            },
        };

    if target == RemoteOpenTarget::Pane && pane.is_none() {
        return Err(SshRequestError::bad(
            "target=pane は対象ペインが必要（`POST /api/panes/<id>/ssh` を使うか pane を渡す）",
        ));
    }

    let remote_dir = body["remote_dir"]
        .as_str()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    if remote_dir.as_deref().is_some_and(|d| d.contains('"')) {
        // 接続後の `cd "<path>"` は両方言で通る形（#919）なので二重引用符だけ弾く
        return Err(SshRequestError::bad("remote_dir に二重引用符は使えない"));
    }

    Ok(SshOpenRequest {
        host,
        target,
        pane,
        remote_dir,
    })
}

/// 検証済みの要求を dispatch の `OpenRemote` へ変換する。
///
/// `focus` は**常に true**（スマホから開いた相手は Mac の画面でも前に出したい。
/// 見えないところにペインだけ増える状態を作らない）
pub fn to_open_remote(req: &SshOpenRequest) -> crate::protocol::Request {
    crate::protocol::Request::OpenRemote {
        host: req.host.clone(),
        focus: Some(true),
        remote_dir: req.remote_dir.clone(),
        target: Some(req.target),
        pane: req.pane,
        tab: None,
        direction: None,
    }
}

/// `list` 応答（app 経由）から `/api/v2/panes` の各エントリへ SSH 関連の状態を移す。
///
/// 運ぶのは 2 つだけ:
/// - `ssh_connect`: 接続待ち / 失敗 / 再接続中（#1010 / #1040）。**成功したら消える**
/// - `can_ssh`: そのペインをそのまま SSH にできるか（#1006 の判定。#1080 受け入れ条件 ③）
pub fn attach_ssh_state(result: &mut Value, list: &Value) {
    let mut by_id: std::collections::HashMap<u64, (&Value, &Value)> =
        std::collections::HashMap::new();
    if let Some(tabs) = list["tabs"].as_array() {
        for tab in tabs {
            let Some(panes) = tab["panes"].as_array() else {
                continue;
            };
            for pane in panes {
                if let Some(id) = pane["id"].as_u64() {
                    by_id.insert(id, (&pane["ssh_connect"], &pane["can_ssh"]));
                }
            }
        }
    }
    let Some(panes) = result["panes"].as_array_mut() else {
        return;
    };
    for pane in panes {
        let id = pane["id"].as_u64().unwrap_or(0);
        match by_id.get(&id) {
            Some((ssh_connect, can_ssh)) => {
                pane["ssh_connect"] = (*ssh_connect).clone();
                // 古い app（`can_ssh` を載せない世代）と繋がっている可能性があるので、
                // 欠けていたら「分からない」ではなく**理由つきの false** にする。
                // 黙って true にすると「メニューに出たのに断られる」を作る
                pane["can_ssh"] = if can_ssh.is_null() {
                    unknown_can_ssh()
                } else {
                    (*can_ssh).clone()
                };
            }
            None => {
                pane["ssh_connect"] = Value::Null;
                pane["can_ssh"] = unknown_can_ssh();
            }
        }
    }
}

/// app 不在（tmux 直読み）経路の `can_ssh`。
///
/// 接続そのものが app 経由でしかできない（`OpenRemote` は dispatch）ので、
/// **押せない理由を先に出す**。ここで true を返すと、押した後に 503 で断られる
pub fn attach_app_unavailable(result: &mut Value) {
    let Some(panes) = result["panes"].as_array_mut() else {
        return;
    };
    for pane in panes {
        pane["ssh_connect"] = Value::Null;
        pane["can_ssh"] = json!({
            "ok": false,
            "reason": "app_unavailable",
            "note": "tako app が稼働していないので SSH 接続を開けない（Mac で tako を起動する）",
        });
    }
}

fn unknown_can_ssh() -> Value {
    json!({
        "ok": false,
        "reason": "unknown",
        "note": "このペインを SSH 化できるか判定できない（Mac 側の tako が古い可能性がある）",
    })
}

/// IPC 経由で `SshHosts` を引く（#20 の dispatch と 1:1）。
///
/// **app 経由に統一する**（daemon から `~/.ssh/config` を直接読む実装は作らない）:
/// 接続自体が app 経由でしかできないので、一覧だけ取れても押せる先が無い。
/// 実装が 2 つに割れると「一覧には出るのに繋げない」食い違いが生える
pub(crate) fn list_hosts(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
) -> Result<Value, (u16, String)> {
    request_via_app(
        app_conn,
        crate::protocol::Request::SshHosts,
        "tako app が稼働していない（SSH ホスト一覧は app 経由のみ）",
    )
}

/// IPC 経由で `OpenRemote` を呼ぶ（接続を**始める**だけ。成立の可否は `ssh_connect`）
pub(crate) fn open_remote(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
    req: &SshOpenRequest,
) -> Result<Value, (u16, String)> {
    let mut out = request_via_app(
        app_conn,
        to_open_remote(req),
        "tako app が稼働していない（リモートからの SSH 接続は app 経由のみ）",
    )?;
    // 応答に「次に何を見ればいいか」を添える。接続の成否はこの時点では**まだ分からない**
    // （#919 / #1040: 失敗しても ssh_connect に理由が残り、ペインは消えない）
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "poll".into(),
            json!("/api/v2/panes の ssh_connect で接続の進み方と失敗の理由が読める"),
        );
    }
    Ok(out)
}

/// dispatch を IPC で呼ぶ共通経路。app 不在は 503、失敗は 502（接続は破棄して張り直す）
fn request_via_app(
    app_conn: &std::sync::Arc<std::sync::RwLock<crate::remote::AppConnection>>,
    request: crate::protocol::Request,
    unavailable: &str,
) -> Result<Value, (u16, String)> {
    let mut conn = app_conn
        .write()
        .map_err(|_| (500u16, "内部エラー".to_string()))?;
    let client = conn.get().ok_or((503u16, unavailable.to_string()))?;
    match client.request(request) {
        Ok(v) => Ok(v),
        Err(e) => {
            // 「このペインは SSH 化できない」は dispatch の**正常な拒否**（#1006）なので、
            // IPC が壊れたことにして接続を捨ててはいけない（次の操作まで巻き添えになる）
            if is_refusal(&e) {
                return Err((409, e));
            }
            conn.invalidate();
            Err((502, e))
        }
    }
}

/// dispatch の拒否（呼び出し側の指定が通らない）か、経路の故障かを見分ける。
///
/// 判定材料は `PaneSshBlock::message` が必ず添える次の一手（`target=split`）と、
/// ペイン解決の失敗。文言に依存するのでゆるく見て、外れたら 502 側（安全側 =
/// 接続を張り直す）へ倒れる
fn is_refusal(err: &str) -> bool {
    err.contains("target=split") || err.contains("見つからない")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ペイン指定ルートの既定はこのペインを_ssh_にする() {
        let req = parse_open_request(&json!({ "host": "win" }), Some(7)).unwrap();
        assert_eq!(req.target, RemoteOpenTarget::Pane);
        assert_eq!(req.pane, Some(7));
        assert_eq!(req.host, "win");
        assert_eq!(req.remote_dir, None);
    }

    #[test]
    fn ペイン非指定ルートの既定は現在タブへの新ペイン() {
        // #1006 の既定をそのまま共有する（リモート側で語彙も既定も作り直さない）
        let req = parse_open_request(&json!({ "host": "win" }), None).unwrap();
        assert_eq!(req.target, RemoteOpenTarget::default());
        assert_eq!(req.target, RemoteOpenTarget::Split);
        assert_eq!(req.pane, None);
    }

    #[test]
    fn ペイン指定ルートでも開き先を明示できる() {
        let req =
            parse_open_request(&json!({ "host": "win", "target": "split" }), Some(3)).unwrap();
        assert_eq!(req.target, RemoteOpenTarget::Split);
        // 分割元として渡る
        assert_eq!(req.pane, Some(3));
    }

    #[test]
    fn パス由来のペインがボディより強い() {
        // URL に出ている対象と実際に触る対象がずれると監査ログが嘘になる
        let req = parse_open_request(&json!({ "host": "win", "pane": 99 }), Some(3)).unwrap();
        assert_eq!(req.pane, Some(3));
    }

    #[test]
    fn 不正な要求は理由つきで断る() {
        let cases = [
            (json!({}), None, "host"),
            (json!({ "host": "   " }), None, "host"),
            (json!({ "host": "a b" }), None, "使えない文字"),
            (json!({ "host": "a'b" }), None, "使えない文字"),
            (json!({ "host": "win", "target": "window" }), None, "target"),
            (json!({ "host": "win", "target": 3 }), None, "target"),
            (json!({ "host": "win", "pane": "3" }), None, "pane"),
            (
                json!({ "host": "win", "target": "pane" }),
                None,
                "target=pane",
            ),
            (
                json!({ "host": "win", "remote_dir": "a\"b" }),
                None,
                "二重引用符",
            ),
        ];
        for (body, path_pane, needle) in cases {
            let err =
                parse_open_request(&body, path_pane).expect_err(&format!("{body} は断るべき"));
            assert_eq!(err.status, 400, "{body}");
            assert!(
                err.message.contains(needle),
                "理由に {needle} が出ていない: {} ({body})",
                err.message
            );
        }
    }

    #[test]
    fn 行を割る文字やシェルのメタ文字が混じったホストは弾く() {
        // `target=pane` は素のシェルへ 1 行打つ（#640 の送達経路）ので、
        // 行が割れると残りが**別のコマンド**として実行される。
        // 引用（`sh_quote` / `ps_quote`）でも防げるが、そちらの実装が変わっても
        // 壊れないよう入口で落とす
        for host in [
            "win\nrm -rf /",
            "win\rmalicious",
            "win\u{7}",
            "a`b",
            "a$(id)b",
            "a;id",
            "a|id",
            "a&id",
            "a>out",
            "a*b",
            "win host",
            "a\\b",
        ] {
            assert!(
                parse_open_request(&json!({ "host": host }), None).is_err(),
                "{host:?} を受けてはいけない"
            );
        }
    }

    #[test]
    fn 前後の空白は落として受ける() {
        // 手入力・コピペで付く前後の空白まで拒否すると使い勝手だけが悪くなる。
        // 落とした結果が安全な 1 語なら通す（間に挟まった空白は上のテストで弾く）
        for raw in [" win", "win\n", "\twin \r\n"] {
            let req = parse_open_request(&json!({ "host": raw }), None)
                .unwrap_or_else(|e| panic!("{raw:?} は受けるべき: {}", e.message));
            assert_eq!(req.host, "win", "{raw:?}");
        }
    }

    #[test]
    fn 一覧に無いホストも受ける() {
        // `Host *` にマッチする相手や ~/.ssh/config を使わない運用があるので、
        // 一覧を許可リストにしない（#1080 の設計判断）
        for host in [
            "10.0.0.5",
            "user@example.internal",
            "[2001:db8::1]",
            "fe80::1%en0",
            "build-01.example.test",
            "検証機",
        ] {
            assert!(
                parse_open_request(&json!({ "host": host }), None).is_ok(),
                "{host:?} は実在しうる表記なので受ける"
            );
        }
    }

    #[test]
    fn 要求は_dispatch_の_openremote_へそのまま渡る() {
        let req = SshOpenRequest {
            host: "win".into(),
            target: RemoteOpenTarget::Tab,
            pane: Some(5),
            remote_dir: Some("/srv/app".into()),
        };
        match to_open_remote(&req) {
            crate::protocol::Request::OpenRemote {
                host,
                focus,
                remote_dir,
                target,
                pane,
                tab,
                direction,
            } => {
                assert_eq!(host, "win");
                // スマホから開いた相手は Mac の画面でも前に出す
                assert_eq!(focus, Some(true));
                assert_eq!(remote_dir.as_deref(), Some("/srv/app"));
                assert_eq!(target, Some(RemoteOpenTarget::Tab));
                assert_eq!(pane, Some(5));
                assert_eq!(tab, None);
                assert_eq!(direction, None);
            }
            other => panic!("OpenRemote 以外へ化けた: {other:?}"),
        }
    }

    fn v2_panes(ids: &[u64]) -> Value {
        json!({
            "panes": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "api_version": 2,
        })
    }

    #[test]
    fn list_の_ssh_状態がペイン一覧へ移る() {
        let list = json!({
            "tabs": [{
                "id": 1,
                "panes": [
                    {
                        "id": 1,
                        "ssh_connect": { "host": "win", "phase": "connecting" },
                        "can_ssh": { "ok": true },
                    },
                    {
                        "id": 2,
                        "ssh_connect": null,
                        "can_ssh": { "ok": false, "reason": "agent_role", "note": "…" },
                    },
                ],
            }],
        });
        let mut result = v2_panes(&[1, 2]);
        attach_ssh_state(&mut result, &list);
        let panes = result["panes"].as_array().unwrap();
        assert_eq!(panes[0]["ssh_connect"]["phase"], "connecting");
        assert_eq!(panes[0]["can_ssh"]["ok"], true);
        assert!(panes[1]["ssh_connect"].is_null());
        assert_eq!(panes[1]["can_ssh"]["ok"], false);
        assert_eq!(panes[1]["can_ssh"]["reason"], "agent_role");
    }

    #[test]
    fn 判定が取れないペインは理由つきで_false() {
        // 古い app（can_ssh を載せない世代）/ list に居ないペインの両方。
        // ここで true にすると「押せるのに断られる」= #1080 受け入れ条件 ③ が壊れる
        let list = json!({ "tabs": [{ "id": 1, "panes": [{ "id": 1 }] }] });
        let mut result = v2_panes(&[1, 42]);
        attach_ssh_state(&mut result, &list);
        let panes = result["panes"].as_array().unwrap();
        for p in panes {
            assert_eq!(p["can_ssh"]["ok"], false, "{p}");
            assert_eq!(p["can_ssh"]["reason"], "unknown", "{p}");
            assert!(!p["can_ssh"]["note"].as_str().unwrap_or("").is_empty());
        }
    }

    #[test]
    fn app_不在なら押せない理由を先に出す() {
        let mut result = v2_panes(&[1]);
        attach_app_unavailable(&mut result);
        let pane = &result["panes"][0];
        assert_eq!(pane["can_ssh"]["ok"], false);
        assert_eq!(pane["can_ssh"]["reason"], "app_unavailable");
        assert!(pane["can_ssh"]["note"]
            .as_str()
            .unwrap()
            .contains("tako app"));
    }

    #[test]
    fn dispatch_の拒否は接続の故障と区別する() {
        // #1006 の「このペインは SSH 化できない」は正常な拒否。502 にすると
        // IPC 接続が捨てられ、次の操作まで巻き添えで落ちる
        assert!(is_refusal(
            "pane 7 は全画面 TUI を表示中（素のシェルではない）ので SSH 化できない。\
             TUI を終了させるか、target=split で新しいペインを作って接続する"
        ));
        assert!(is_refusal("ペイン 42 が見つからない"));
        assert!(!is_refusal("接続が切断された"));
    }
}

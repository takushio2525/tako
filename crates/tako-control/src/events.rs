//! events — 変化の出来事の購読 `Request::Events`（FR-2.45 / Issue #2731）の応答を組む。
//!
//! バッファと待ちの機構は `tako_core::events`。ここは要求の検証と JSON の形だけを持つ
//! （CLI `tako events` と MCP `tako_events` が同じ応答になる 1 実装）。
//!
//! **IPC / MCP の要求は `ipc::submit` が UI の受け口へ渡す前にここで答える**（長ポーリングの待ちは
//! 接続のスレッド）。UI スレッドの dispatch へ来た要求（GUI の中からの呼び出し）は待たずに返す

use serde_json::{json, Map, Value};
use tako_core::events::{Detail, Event, Kind, Page, Query};

use crate::dispatch::{DispatchError, Reason};
use crate::protocol::Request;

/// `Request::Events` に答える。`can_wait` は呼び手が接続のスレッドか（UI スレッドは偽）
pub fn answer(request: &Request, can_wait: bool) -> Result<Value, DispatchError> {
    let query = query_of(request)?;
    Ok(page_json(
        &tako_core::events::log().read(&query, can_wait),
        tako_core::i18n::lang(),
    ))
}

/// 要求を読みの条件へ。知らない種類は断る（受け手の綴り間違いを黙って空で返さない）
pub fn query_of(request: &Request) -> Result<Query, DispatchError> {
    let Request::Events {
        since,
        kinds,
        panes,
        wait_ms,
    } = request
    else {
        return Err(DispatchError::InvalidParams(
            "出来事の購読の要求ではない".into(),
        ));
    };
    let kinds = kinds
        .iter()
        .map(|name| {
            Kind::parse(name).ok_or_else(|| {
                Reason::UnknownValue {
                    key: "kinds",
                    got: name.clone(),
                    allowed: known_kinds().join(" / "),
                }
                .invalid()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Query {
        since: since.unwrap_or(0),
        kinds,
        panes: panes.clone(),
        wait_ms: wait_ms.unwrap_or(0),
    })
}

fn known_kinds() -> Vec<&'static str> {
    Kind::ALL.iter().map(|k| k.as_str()).collect()
}

/// 応答の JSON。**載せるのは種類・番号・時刻・ペイン / タブの id・role・タブ名・固定の語彙だけ**
/// （ペインの内容・送信テキスト・会話の本文は `Detail` がそもそも持たない）
pub fn page_json(page: &Page, lang: tako_core::i18n::Lang) -> Value {
    let mut out = json!({
        "epoch": page.epoch,
        "cursor": page.cursor,
        "dropped": page.dropped.is_some(),
        "events": page.events.iter().map(event_json).collect::<Vec<_>>(),
        "wait_ms": page.wait_ms,
        "kinds_known": known_kinds(),
    });
    if let Some(dropped) = page.dropped {
        out["dropped_reason"] = json!(dropped.code());
    }
    if page.wait_clamped {
        out["wait_clamped"] = json!(true);
    }
    if let Some(skip) = page.wait_skipped {
        out["wait_skipped"] = json!(skip.code());
        out["wait_skipped_reason"] = json!(skip.text_in(lang));
    }
    out
}

fn event_json(event: &Event) -> Value {
    let mut out = json!({
        "seq": event.seq,
        "at": rfc3339_ms(event.at_ms),
        "kind": event.detail.kind().as_str(),
    });
    if let Some(pane) = event.detail.pane() {
        out["pane"] = json!(pane);
    }
    if let Some(tab) = event.detail.tab() {
        out["tab"] = json!(tab);
    }
    out["data"] = Value::Object(data_of(&event.detail));
    out
}

fn data_of(detail: &Detail) -> Map<String, Value> {
    let mut data = Map::new();
    let mut put = |key: &str, value: Value| {
        if !value.is_null() {
            data.insert(key.to_string(), value);
        }
    };
    match detail {
        Detail::PaneAdded {
            origin,
            restored,
            role,
            ..
        } => {
            put("origin", json!(origin));
            put("restored", json!(restored));
            put("role", json!(role));
        }
        Detail::PaneClosed { role, .. } => put("role", json!(role)),
        Detail::RoleChanged { from, to, .. } => {
            put("from", json!(from));
            put("to", json!(to));
        }
        Detail::TabRenamed {
            from, to, source, ..
        } => {
            put("from", json!(from));
            put("to", json!(to));
            put("source", json!(source));
        }
        Detail::TurnChanged {
            from, to, source, ..
        } => {
            put("from", json!(from));
            put("to", json!(to));
            put("source", json!(source));
        }
    }
    data
}

/// unix ms → `YYYY-MM-DDTHH:MM:SS.mmmZ`
fn rfc3339_ms(at_ms: u64) -> String {
    let secs = crate::diag::format_utc((at_ms / 1000) as i64);
    let ms = at_ms % 1000;
    match secs.strip_suffix('Z') {
        Some(head) => format!("{head}.{ms:03}Z"),
        None => secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::events::{Dropped, EventLog, Outcome, WaitSkip, MAX_WAIT_MS};
    use tako_core::i18n::Lang;

    fn request(kinds: &[&str], wait_ms: Option<u64>) -> Request {
        Request::Events {
            since: Some(0),
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            panes: Vec::new(),
            wait_ms,
        }
    }

    #[test]
    fn 知らない種類は候補を添えて断る() {
        let err = query_of(&request(&["transcript_appended"], None)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("transcript_appended"), "{text}");
        assert!(text.contains("turn_changed"), "{text}");
    }

    #[test]
    fn 応答は種類と番号と時刻とidと語彙だけを載せる() {
        let log = EventLog::with_epoch(10, "e1".into());
        log.push(Detail::RoleChanged {
            pane: 48,
            from: Some("orchestrator-master".into()),
            to: Some("orchestrator-master:dev".into()),
        });
        log.push(Detail::TabRenamed {
            tab: 3,
            from: "1".into(),
            to: "レビュー".into(),
            source: "manual",
        });
        let query = query_of(&request(&[], Some(MAX_WAIT_MS + 5))).unwrap();
        let page = log.read(&query, false);
        let v = page_json(&page, Lang::Ja);
        assert_eq!(v["epoch"], "e1");
        assert_eq!(v["cursor"], 2);
        assert_eq!(v["dropped"], false);
        assert_eq!(v["wait_ms"], MAX_WAIT_MS);
        assert_eq!(v["wait_clamped"], true);
        assert_eq!(v["kinds_known"].as_array().unwrap().len(), Kind::ALL.len());
        let role = &v["events"][0];
        assert_eq!(role["kind"], "role_changed");
        assert_eq!(role["pane"], 48);
        assert_eq!(role["data"]["from"], "orchestrator-master");
        assert_eq!(role["data"]["to"], "orchestrator-master:dev");
        assert!(role["at"].as_str().unwrap().ends_with('Z'));
        let tab = &v["events"][1];
        assert_eq!(tab["tab"], 3);
        assert!(tab.get("pane").is_none());
        assert_eq!(tab["data"]["source"], "manual");
    }

    #[test]
    fn 溢れと待たなかった理由を欄で返す() {
        let log = EventLog::with_epoch(1, "e".into());
        log.push(Detail::PaneClosed {
            pane: 1,
            role: None,
        });
        log.push(Detail::PaneClosed {
            pane: 2,
            role: None,
        });
        let page = log.read(&Query::default(), false);
        assert_eq!(page.dropped, Some(Dropped::Overflow));
        let v = page_json(&page, Lang::En);
        assert_eq!(v["dropped"], true);
        assert_eq!(v["dropped_reason"], "overflow");
        let waiting = log.read(
            &Query {
                since: 2,
                wait_ms: 10,
                ..Query::default()
            },
            false,
        );
        assert_eq!(waiting.outcome, Outcome::Immediate);
        assert_eq!(waiting.wait_skipped, Some(WaitSkip::NotOnConnectionThread));
        let v = page_json(&waiting, Lang::En);
        assert_eq!(v["wait_skipped"], "not_on_connection_thread");
        assert!(v["wait_skipped_reason"]
            .as_str()
            .unwrap()
            .contains("UI thread"));
    }

    #[test]
    fn 時刻はミリ秒つきのrfc3339() {
        assert_eq!(rfc3339_ms(1_791_452_867_009), "2026-10-08T09:47:47.009Z");
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
    }
}

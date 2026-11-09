//! 番犬: 変化の出来事（`tako_events` / `tako events`。#2731）に本文が載らない
//!
//! 出来事は外のプロセス（外部の GUI フロントエンド・スクリプト・別の AI）へ流れる。ペインの内容・
//! 送信テキスト・会話の本文・トークンを載せると、tako の外へ本文を流す口になる（AGENTS.md の
//! 絶対ルール）。会話の正本は transcript で、出来事は「何が変わったか」の種類と id と語彙だけを運ぶ。
//!
//! 守り方は 2 段:
//!
//! 1. **型**: `tako_core::events::Detail` の欄の名前を許可表と突き合わせる。欄を足した PR は
//!    ここで file:line を名指して落ちる（足すなら本文でないことを確かめてから表へ足す）
//! 2. **応答**: 書き手（`Pane` / `Tab` / `ModHub::accept`）を本物で通し、本文に当たる値
//!    （報告の会話 id・ツール名・モデル名）を混ぜた材料から組んだ応答の JSON に、許可表の外の鍵も
//!    その値も現れないことを見る

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;
use tako_core::events::{Detail, EventLog, Query};
use tako_core::{Pane, PaneOrigin, Tab};

/// `Detail` の欄として許す名前（本文を持たない: id・role・タブ名・固定の語彙）
const DETAIL_FIELDS: &[&str] = &[
    "pane", "tab", "origin", "restored", "role", "from", "to", "source",
];

/// 応答の JSON に現れてよい鍵（ページ・出来事・data の全部）
const RESPONSE_KEYS: &[&str] = &[
    // ページ
    "epoch",
    "cursor",
    "dropped",
    "dropped_reason",
    "events",
    "wait_ms",
    "wait_clamped",
    "wait_skipped",
    "wait_skipped_reason",
    "kinds_known",
    // 出来事
    "seq",
    "at",
    "kind",
    "pane",
    "tab",
    "data",
    // data
    "origin",
    "restored",
    "role",
    "from",
    "to",
    "source",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// `pub enum Detail {` から閉じ括弧までの (行番号, 行) を返す
fn detail_block(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut depth = 0i32;
    for (i, line) in src.lines().enumerate() {
        if !inside {
            if line.trim_start().starts_with("pub enum Detail {") {
                inside = true;
                depth = 1;
            }
            continue;
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 {
            break;
        }
        out.push((i + 1, line));
    }
    out
}

/// 1 行に書かれた欄の名前（`name: Type` の `name`）。コメント行は見ない
fn field_names(line: &str) -> Vec<String> {
    let code = line.split("//").next().unwrap_or("");
    let body = match code.find('{') {
        Some(at) => &code[at + 1..],
        None => code,
    };
    body.split(',')
        .filter_map(|part| {
            let (name, ty) = part.split_once(':')?;
            let name = name.trim();
            (!ty.trim().is_empty()
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .then(|| name.to_string())
        })
        .collect()
}

/// 型の段: `Detail` の欄は許可表の中だけ
fn detail_strays(src: &str, file: &str) -> Vec<String> {
    detail_block(src)
        .into_iter()
        .flat_map(|(line_no, line)| {
            field_names(line)
                .into_iter()
                .filter(|name| !DETAIL_FIELDS.contains(&name.as_str()))
                .map(move |name| format!("{file}:{line_no}: 欄 `{name}`"))
        })
        .collect()
}

#[test]
fn 出来事の欄は本文を持たない名前だけ() {
    let path = repo_root().join("crates/tako-core/src/events.rs");
    let src = std::fs::read_to_string(&path).expect("events.rs を読める");
    assert!(
        !detail_block(&src).is_empty(),
        "`pub enum Detail {{` が見つからない（番犬が空振りしている）"
    );
    let strays = detail_strays(&src, "crates/tako-core/src/events.rs");
    assert!(
        strays.is_empty(),
        "出来事（#2731）の欄に許可表の外の名前がある: {strays:?}。ペインの内容・送信テキスト・\
         会話の本文・トークンは外へ流さない。本文でないと確かめたうえで DETAIL_FIELDS へ足すこと"
    );
}

/// 検出力の担保: 本文の欄を足した形を名指す
#[test]
fn 番犬は本文の欄を見逃さない() {
    let bad = "pub enum Detail {\n    RoleChanged {\n        pane: u64,\n        text: String,\n    },\n    PaneClosed { pane: u64, body: String },\n}\n";
    let strays = detail_strays(bad, "x.rs");
    assert_eq!(strays, ["x.rs:4: 欄 `text`", "x.rs:6: 欄 `body`"]);
}

fn keys_outside(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if !RESPONSE_KEYS.contains(&key.as_str()) {
                    out.push(key.clone());
                }
                keys_outside(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| keys_outside(v, out)),
        _ => {}
    }
}

/// 応答の段: 書き手を本物で通し、本文に当たる値を混ぜても応答には現れない
#[test]
fn 書き手を通した応答に本文の値が現れない() {
    let captured = tako_core::events::capture_on_this_thread();
    let mut pane = Pane::new(PaneOrigin::Cli);
    pane.set_role(Some("orchestrator-worker:p".into()));
    pane.set_title(Some("SECRET-PANE-TITLE".into()));
    let mut tab = Tab::new("1", Pane::new(PaneOrigin::User));
    tab.set_title_manual("レビュー");
    let mut hub = tako_core::claude_mod::ModHub::new(true);
    let report = tako_core::claude_mod::parse_report(serde_json::json!({
        "schema": 1, "mod_version": "t", "at": 1, "turn": "permission",
        "session_id": "SECRET-SESSION-ID",
        "model": "SECRET-MODEL",
        "pending_tool": "SECRET-TOOL",
        "last_turn": { "duration_ms": 1, "reason": "SECRET-REASON" },
    }))
    .expect("報告");
    hub.accept(pane.id().as_u64(), report, Instant::now());
    drop(pane);
    let details: Vec<Detail> = captured.take();
    let kinds: Vec<&str> = details.iter().map(|d| d.kind().as_str()).collect();
    for kind in tako_core::events::Kind::ALL {
        assert!(
            kinds.contains(&kind.as_str()),
            "書き手が {} を積んでいない: {kinds:?}",
            kind.as_str()
        );
    }
    let log = EventLog::with_epoch(64, "e".into());
    for detail in details {
        log.push(detail);
    }
    let page = log.read(&Query::default(), false);
    let json = tako_control::events::page_json(&page, tako_core::i18n::Lang::Ja);
    let text = json.to_string();
    for secret in [
        "SECRET-PANE-TITLE",
        "SECRET-SESSION-ID",
        "SECRET-MODEL",
        "SECRET-TOOL",
        "SECRET-REASON",
    ] {
        assert!(!text.contains(secret), "応答に {secret} が載った: {text}");
    }
    let mut strays = Vec::new();
    keys_outside(&json, &mut strays);
    assert!(
        strays.is_empty(),
        "応答に許可表の外の鍵がある: {strays:?}（本文でないと確かめてから RESPONSE_KEYS へ）"
    );
}

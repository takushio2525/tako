//! `tako mod`（`Request::Mod`）の dispatch 側（エピック #1877 / S1 #1879。FR-2.42）
//!
//! 判断と報告の型は `tako_core::claude_mod` が持つ。ここは「GUI のメモリにある
//! [`tako_core::claude_mod::ModHub`] を読み書きし、JSON に組む」だけ。
//!
//! | action | 中身 | CLI | MCP |
//! |---|---|---|---|
//! | `status`（既定） | 展開先・版・注入の有無・ペインごとの最終報告（鮮度つき） | `tako mod` | `tako_mod` |
//! | `on` / `off` | 設定 `claude_mod` の切替（次に作るペインから） | `tako mod on` / `off` | `tako_mod` |
//! | `report` | mod からの状態報告。応答は tako 側のスナップショット | `tako mod report`（stdin） | **出さない** |
//!
//! `report` を MCP に載せないのは、AI が叩くと**自分の状態を偽って注入できるだけ**で、
//! AI にとっての等価物（読む側の `tako_mod` / `tako_orchestrator_self`）は別にあるから
//! （FR-2.42 に例外の理由として書いた）。

use std::time::Instant;

use serde_json::{json, Value};
use tako_core::claude_mod::{self as core, Accepted, ModHub, OffReason, StoredReport};
use tako_core::PaneId;

use crate::dispatch::DispatchError;
use crate::host::ControlHost;

/// CLI / dispatch が受け付ける action
pub const ACTIONS: &[&str] = &["status", "on", "off", "report"];
/// MCP が受け付ける action（`report` は載せない。モジュール冒頭の理由）
pub const MCP_ACTIONS: &[&str] = &["status", "on", "off"];

/// `tako setup` の段（設計書 §3.2。GUI 起動時と並ぶ 2 つ目の発火点）。
///
/// 展開するのは tako の data dir の中だけなので予告も `[y/N]` も要らない（Claude Code の
/// 設定ファイルは 1 バイトも書かない）。**止めない**: 展開に失敗しても tako は画面の読み取りで
/// 動くので、利用者へ残る作業にはしない（1 行の注意だけ）
pub fn run_setup_stage() -> String {
    if core::ab_off() {
        return format!(
            "  [legacy] tako mod の展開は {} で無効（#1879 前の挙動）",
            core::AB_OFF_ENV
        );
    }
    match core::install() {
        Ok((dir, outcome)) => {
            let place = tako_core::paths::shorten_home(&dir.display().to_string());
            match outcome {
                core::InstallOutcome::Written => {
                    format!("  [ok] tako mod（Claude Code の mod）を展開: {place}")
                }
                core::InstallOutcome::Unchanged => {
                    format!("  [ok] tako mod（Claude Code の mod）は最新: {place}")
                }
            }
        }
        Err(e) => {
            format!("  [warn] tako mod を展開できない（claude の状態は画面から読むので続行）: {e}")
        }
    }
}

/// `Request::Mod` の本体
pub fn run(
    host: &mut dyn ControlHost,
    action: Option<&str>,
    report: Option<Value>,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    match action.unwrap_or("status") {
        "status" => status(host),
        "on" | "off" => {
            let enabled = action == Some("on");
            host.set_claude_mod_enabled(enabled)
                .map_err(DispatchError::Operation)?;
            let mut out = status(host)?;
            out["applies_to"] = json!(
                "次に作るペインから（動いている claude には効かない。止めるなら claude を起動し直す）"
            );
            Ok(out)
        }
        "report" => accept_report(host, report, pane),
        other => Err(DispatchError::InvalidParams(format!(
            "未知の action: {other}（{}）",
            ACTIONS.join(" / ")
        ))),
    }
}

fn hub(host: &dyn ControlHost) -> Result<&ModHub, DispatchError> {
    host.claude_mod().ok_or_else(|| {
        DispatchError::Operation("この tako は mod を扱わない（2 つ目のインスタンス等）".into())
    })
}

fn reason_json(reason: &OffReason) -> Value {
    json!({ "code": reason.code(), "message": reason.describe() })
}

/// 報告を受け取り、tako 側のスナップショットを返す（設計書 §2.3。S3 から mod が使う）
fn accept_report(
    host: &mut dyn ControlHost,
    report: Option<Value>,
    pane: Option<u64>,
) -> Result<Value, DispatchError> {
    let value = report.ok_or_else(|| {
        DispatchError::InvalidParams("report には報告の JSON が要る（stdin で渡す）".into())
    })?;
    let parsed = core::parse_report(value).map_err(DispatchError::InvalidParams)?;
    let pane_id = resolve_reporting_pane(host, pane)?;
    let accepted = host
        .claude_mod_mut()
        .ok_or_else(|| DispatchError::Operation("この tako は mod の報告を受けない".into()))?
        .accept(pane_id.as_u64(), parsed, Instant::now());
    let enabled = host.claude_mod().is_some_and(|h| h.enabled);
    let ws = host.workspace();
    let tab = ws.find_tab_of_pane(pane_id).and_then(|t| ws.get_tab(t));
    let pane_title = tab
        .and_then(|t| t.tree().get(pane_id))
        .and_then(|p| p.title())
        .map(str::to_string);
    Ok(json!({
        "accepted": match accepted {
            Accepted::Stored => "stored",
            Accepted::Ended => "ended",
        },
        "pane": pane_id.as_u64(),
        "fresh_for_ms": core::FRESH_FOR.as_millis() as u64,
        "tako": {
            "pane_title": pane_title,
            "tab_title": tab.map(|t| t.title().to_string()),
            "mod_enabled": enabled,
        },
    }))
}

/// 報告の送り主のペイン。現世代にそのまま居ればそれ、居なければ #210 の旧 ID → 新 ID の対応
/// （tako の再起動をまたいで生き残った claude は古い `TAKO_PANE_ID` を持っている）
fn resolve_reporting_pane(
    host: &dyn ControlHost,
    pane: Option<u64>,
) -> Result<PaneId, DispatchError> {
    match crate::dispatch::resolve_pane(host.workspace(), pane) {
        Ok((_, id)) => Ok(id),
        Err(DispatchError::PaneNotFound(raw)) => host
            .resolve_stale_pane(PaneId::from_raw(raw))
            .filter(|id| host.workspace().find_tab_of_pane(*id).is_some())
            .ok_or(DispatchError::PaneNotFound(raw)),
        Err(e) => Err(e),
    }
}

/// ペイン 1 行の状態
fn pane_row(
    hub: &ModHub,
    pane: PaneId,
    title: Option<&str>,
    claude_on_screen: bool,
    now: Instant,
) -> Value {
    let raw = pane.as_u64();
    let injection = hub.injections.get(&raw);
    let stored = hub.reports.get(&raw);
    let (state, reason) = match (stored, injection) {
        (Some(s), _) if core::is_fresh(s.received, now) => ("reporting", Value::Null),
        (Some(s), _) => (
            "stale",
            json!({
                "code": "mod_stale",
                "message": format!(
                    "最後の報告から {} 秒（{} 秒で失効）。claude が終わったか mod が止まった",
                    now.saturating_duration_since(s.received).as_secs(),
                    core::FRESH_FOR.as_secs()
                ),
            }),
        ),
        (None, Some(inj)) => match &inj.off {
            Some(off) => ("not_injected", reason_json(off)),
            None if claude_on_screen => (
                "no_report",
                json!({
                    "code": "mod_absent",
                    "message": "claude は動いているが mod から報告が無い。--safe-mode / --bare / \
                        disableAllHooks / 組織の allowManagedModsOnly で mod が読まれていないか、\
                        注入より前から動いていた claude",
                }),
            ),
            None => ("waiting", Value::Null),
        },
        (None, None) => ("unknown", Value::Null),
    };
    let mut row = json!({
        "pane": raw,
        "title": title,
        "state": state,
        "injected": injection.map(|i| i.off.is_none()),
        "claude_on_screen": claude_on_screen,
    });
    if !reason.is_null() {
        row["reason"] = reason;
    }
    if let Some(s) = stored {
        row["report"] = report_json(s, now);
    }
    row
}

fn report_json(stored: &StoredReport, now: Instant) -> Value {
    let mut v = serde_json::to_value(&stored.report).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.insert(
            "age_ms".into(),
            json!(now.saturating_duration_since(stored.received).as_millis() as u64),
        );
        map.insert("reports".into(), json!(stored.count));
    }
    v
}

/// `tako mod`（status）
fn status(host: &dyn ControlHost) -> Result<Value, DispatchError> {
    let hub = hub(host)?;
    let now = Instant::now();
    let injection = hub.decide(core::ab_off());
    let mut panes = Vec::new();
    for tab in host.workspace().tabs() {
        for pane in tab.tree().panes() {
            let id = pane.id();
            let on_screen = host.session(id).is_some_and(|s| {
                !matches!(
                    crate::claude_tui::detect(&s.visible_lines()),
                    crate::claude_tui::ClaudeScreen::Unknown
                )
            });
            panes.push(pane_row(hub, id, pane.title(), on_screen, now));
        }
    }
    let count = |state: &str| panes.iter().filter(|p| p["state"] == state).count();
    let summary = json!({
        "reporting": count("reporting"),
        "stale": count("stale"),
        "no_report": count("no_report"),
        "not_injected": count("not_injected"),
    });
    Ok(json!({
        "enabled": hub.enabled,
        "injecting": injection.is_on(),
        "reason": injection.off_reason().map(reason_json),
        "plugin_dir": hub.plugin_dir.as_ref().map(|p| p.display().to_string()),
        "install": match &hub.install_result {
            Some(Ok(outcome)) => json!(outcome.as_str()),
            Some(Err(e)) => json!(format!("error: {e}")),
            None => Value::Null,
        },
        "cli": hub.cli.as_ref().map(|p| p.display().to_string()),
        "claude_version": hub.claude_version,
        "min_claude_version": core::MIN_CLAUDE_VERSION,
        "mod_version": env!("CARGO_PKG_VERSION"),
        "fresh_for_ms": core::FRESH_FOR.as_millis() as u64,
        "summary": summary,
        "panes": panes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn report(turn: &str) -> core::ModReport {
        core::parse_report(json!({
            "schema": 1, "mod_version": "t", "at": 1, "turn": turn,
            "context": {"tokens": 10, "window": 100, "percent": 10}, "rate_limits": [],
        }))
        .unwrap()
    }

    fn hub() -> ModHub {
        let mut hub = ModHub::new(true);
        hub.plugin_dir = Some(PathBuf::from("/d/claude-mod/tako"));
        hub.cli = Some(PathBuf::from("/a/tako"));
        hub.claude_version = Some("2.1.294".into());
        hub
    }

    #[test]
    fn ペインの行は報告の鮮度と注入の有無で状態が決まる() {
        let now = Instant::now();
        let mut hub = hub();
        let on = hub.decide(false);
        hub.record_spawn(1, &on);
        hub.record_spawn(2, &on);
        hub.record_spawn(3, &core::Injection::Off(OffReason::Disabled));
        hub.record_spawn(4, &on);
        hub.accept(1, report("busy"), now);
        hub.accept(4, report("idle"), now - Duration::from_secs(60));

        let row = pane_row(&hub, PaneId::from_raw(1), None, true, now);
        assert_eq!(row["state"], "reporting");
        assert_eq!(row["report"]["turn"], "busy");
        assert_eq!(row["report"]["context"]["percent"], 10);

        let row = pane_row(&hub, PaneId::from_raw(2), None, true, now);
        assert_eq!(row["state"], "no_report");
        assert_eq!(row["reason"]["code"], "mod_absent");
        assert!(row["reason"]["message"]
            .as_str()
            .unwrap()
            .contains("--safe-mode"));
        let row = pane_row(&hub, PaneId::from_raw(2), None, false, now);
        assert_eq!(
            row["state"], "waiting",
            "claude が居ないペインは理由を出さない"
        );

        let row = pane_row(&hub, PaneId::from_raw(3), None, true, now);
        assert_eq!(row["state"], "not_injected");
        assert_eq!(row["reason"]["code"], "disabled");

        let row = pane_row(&hub, PaneId::from_raw(4), None, true, now);
        assert_eq!(row["state"], "stale");
        assert_eq!(row["reason"]["code"], "mod_stale");

        let row = pane_row(&hub, PaneId::from_raw(9), None, false, now);
        assert_eq!(row["state"], "unknown");
    }
}

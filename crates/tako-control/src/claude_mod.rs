//! `tako mod`（`Request::Mod`）の dispatch 側（エピック #1877 / S1 #1879。FR-2.42）
//!
//! 判断と報告の型は `tako_core::claude_mod` が持つ。ここは「GUI のメモリにある
//! [`tako_core::claude_mod::ModHub`] を読み書きし、JSON に組む」だけ。
//!
//! | action | 中身 | CLI | MCP |
//! |---|---|---|---|
//! | `status`（既定） | 展開先・版・注入の有無・ペインごとの最終報告（鮮度つき） | `tako mod` | `tako_mod` |
//! | `on` / `off` | 設定 `claude_mod` の切替（次に作るペインから） | `tako mod on` / `off` | `tako_mod` |
//! | `band-on` / `band-off` | Claude Code の画面の帯を出す / 隠す（S3 #1881。報告の応答で全 mod へ中継） | `tako mod band on` / `off` | `tako_mod` |
//! | `report` | mod からの状態報告。応答は tako 側のスナップショット（帯・サイドバーの材料） | `tako mod report`（stdin） | **出さない** |
//!
//! `report` を MCP に載せないのは、AI が叩くと**自分の状態を偽って注入できるだけ**で、
//! AI にとっての等価物（読む側の `tako_mod` / `tako_orchestrator_self`）は別にあるから
//! （FR-2.42 に例外の理由として書いた）。

use std::time::Instant;

use serde_json::{json, Value};
use tako_core::claude_mod::{
    self as core, Accepted, ModHub, ModRateLimit, ModUnavailable, OffReason, StoredReport,
};
use tako_core::PaneId;

use crate::dispatch::DispatchError;
use crate::host::ControlHost;

/// CLI / dispatch が受け付ける action
pub const ACTIONS: &[&str] = &["status", "on", "off", "band-on", "band-off", "report"];
/// MCP が受け付ける action（`report` は載せない。モジュール冒頭の理由）
pub const MCP_ACTIONS: &[&str] = &["status", "on", "off", "band-on", "band-off"];

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

// --- S2（#1880）: 一次ソースとしての読み口 -----------------------------------------
//
// `orchestrator self` / `worker_status` / `read` が同じ形で mod の値を載せるための部品。
// 判断（新鮮か・束ね方・停止の手がかり）は `tako_core::claude_mod` が持ち、ここは
// ホストから引き当てて JSON に組むだけ

/// ペインの報告を一次ソースとして引き当てる（`TAKO_1877_S2_LEGACY` を見る 1 本）
pub fn lookup_pane(
    host: &dyn ControlHost,
    pane: PaneId,
    now: Instant,
) -> Result<&StoredReport, ModUnavailable> {
    core::lookup(host.claude_mod(), pane.as_u64(), now, core::s2_legacy())
}

/// そのペインのアカウントの使用制限（束ね済み）。引き当てに失敗したら空
pub fn account_rate_limits(
    host: &dyn ControlHost,
    pane: PaneId,
    now: Instant,
) -> Vec<ModRateLimit> {
    if core::s2_legacy() {
        return Vec::new();
    }
    host.claude_mod()
        .map(|hub| hub.account_rate_limits(pane.as_u64(), now))
        .unwrap_or_default()
}

/// 画面下のステータスバーの 5h / 7d（#1903）。`panes` はフォーカス順（フォーカス → 他のペイン）。
///
/// 最初に mod の使用制限が引けたペインのアカウントの値（[`account_rate_limits`] の 1 実装 =
/// `orchestrator self` / `worker_status` / 帯と同じ束ね方）を先に見て、どのペインにも無ければ画面の値
/// （`screen` = `refresh_agent_metrics` が画面から読んだ 5h / 7d）。判断は
/// `tako_core::claude_mod::bar_limits`。A/B の `TAKO_1903_LEGACY` では画面の値だけ（#1903 前）
pub fn status_bar_limits(
    host: &dyn ControlHost,
    panes: &[PaneId],
    screen: (Option<u32>, Option<u32>),
    now: Instant,
) -> core::BarLimits {
    let mod_limits = if core::limits_legacy() {
        Vec::new()
    } else {
        panes
            .iter()
            .map(|pane| account_rate_limits(host, *pane, now))
            .find(|limits| !limits.is_empty())
            .unwrap_or_default()
    };
    core::bar_limits(&mod_limits, screen.0, screen.1)
}

/// `worker_status` のために UI スレッドで写し取る報告（`WorkerStatusCtx` は UI 外で仕上げるので、
/// ホストへの借用を持ち出せない）
#[derive(Debug, Clone)]
pub struct ModSnapshot {
    /// `Box` に入れる: 報告は数百バイトあり、`OffloadJob` の変種の大きさを揃える（clippy）
    looked_up: Result<Box<StoredReport>, ModUnavailable>,
    rate_limits: Vec<ModRateLimit>,
    at: Instant,
}

impl Default for ModSnapshot {
    /// 報告なし（ホストを持たない経路・ペインの同一性が崩れたとき）
    fn default() -> Self {
        Self {
            looked_up: Err(ModUnavailable::Absent),
            rate_limits: Vec::new(),
            at: Instant::now(),
        }
    }
}

impl ModSnapshot {
    pub fn capture(host: &dyn ControlHost, pane: PaneId) -> Self {
        let at = Instant::now();
        Self {
            looked_up: lookup_pane(host, pane, at).map(|s| Box::new(s.clone())),
            rate_limits: account_rate_limits(host, pane, at),
            at,
        }
    }

    /// テスト用: 報告をそのまま持たせる
    pub fn with_report(stored: StoredReport, rate_limits: Vec<ModRateLimit>) -> Self {
        Self {
            looked_up: Ok(Box::new(stored)),
            rate_limits,
            at: Instant::now(),
        }
    }

    pub fn view(&self) -> Result<&StoredReport, ModUnavailable> {
        self.looked_up
            .as_ref()
            .map(|s| s.as_ref())
            .map_err(Clone::clone)
    }

    pub fn rate_limits(&self) -> &[ModRateLimit] {
        &self.rate_limits
    }

    pub fn at(&self) -> Instant {
        self.at
    }
}

/// ターン状態（応答の `mod_turn`）。使えなければ null（理由は `mod_reason`）
pub fn turn_json(looked_up: &Result<&StoredReport, ModUnavailable>, now: Instant) -> Value {
    match looked_up {
        Ok(stored) => json!({
            "turn": stored.report.turn.as_str(),
            "pending_tool": stored.report.pending_tool,
            "classic_events": stored.report.classic_events,
            "model": stored.report.model,
            "effort": stored.report.effort,
            "age_ms": now.saturating_duration_since(stored.received).as_millis() as u64,
        }),
        Err(_) => Value::Null,
    }
}

/// mod を使えなかった理由（応答の `mod_reason`。null = 使えた）
pub fn reason_code(looked_up: &Result<&StoredReport, ModUnavailable>) -> Value {
    match looked_up {
        Ok(_) => Value::Null,
        Err(why) => json!(why.code()),
    }
}

/// 使用制限（応答の `rate_limits`。claude は mod から = #1880。空なら null）。
///
/// `limited` / `reset_at` のキー名は codex の `rate_limits`（#985）と揃える
/// （読み手が系統で書き分けなくてよい）。`reset_at` は**上限に当たった窓の解除時刻**
/// （unix 秒・秒精度）で、停止の判定は画面のまま（#813）
pub fn rate_limits_json(limits: &[ModRateLimit]) -> Value {
    if limits.is_empty() {
        return Value::Null;
    }
    let windows: Vec<Value> = limits
        .iter()
        .map(|l| {
            json!({
                "kind": l.kind,
                "used_percent": l.percent_used,
                "resets_at": l.resets_at.as_deref().and_then(core::parse_resets_at),
                "resets_at_iso": l.resets_at,
            })
        })
        .collect();
    json!({
        "source": "mod",
        "windows": windows,
        "limited": limits.iter().any(|l| l.percent_used >= 100.0),
        "reset_at": core::limit_reset_at(limits),
    })
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
        "band-on" | "band-off" => {
            let hidden = action == Some("band-off");
            host.claude_mod_mut()
                .ok_or_else(|| {
                    DispatchError::Operation(
                        "この tako は mod を扱わない（2 つ目のインスタンス等）".into(),
                    )
                })?
                .request_band(hidden, epoch_ms());
            let mut out = status(host)?;
            out["applies_to"] = json!(
                "動いている claude の帯へ次の報告（最大 15 秒）で届く。Claude Code の中で /tako band on|off \
                 を後から打てば、そちらが勝つ"
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
    Ok(json!({
        "accepted": match accepted {
            Accepted::Stored => "stored",
            Accepted::Ended => "ended",
        },
        "pane": pane_id.as_u64(),
        "fresh_for_ms": core::FRESH_FOR.as_millis() as u64,
        "tako": snapshot(host, pane_id, accepted, core::s3_legacy()),
    }))
}

/// 報告の応答に載せる tako 側のスナップショット（設計書 §2.3）。
///
/// S3（#1881）から帯・サイドバーの材料（`view`）も載せる。終わったセッションには描く先が
/// 無いので載せない。A/B（`legacy` = `TAKO_1877_S3_LEGACY`）でも載せない = mod は何も描かない
pub(crate) fn snapshot(
    host: &dyn ControlHost,
    pane_id: PaneId,
    accepted: Accepted,
    legacy: bool,
) -> Value {
    let enabled = host.claude_mod().is_some_and(|h| h.enabled);
    let ws = host.workspace();
    let tab = ws.find_tab_of_pane(pane_id).and_then(|t| ws.get_tab(t));
    let pane_title = tab
        .and_then(|t| t.tree().get(pane_id))
        .and_then(|p| p.title())
        .map(str::to_string);
    let mut out = json!({
        "pane_title": pane_title,
        "tab_title": tab.map(|t| t.title().to_string()),
        "mod_enabled": enabled,
    });
    if accepted == Accepted::Stored && !legacy {
        out["view"] =
            serde_json::to_value(band_view(host, pane_id, Instant::now())).unwrap_or(Value::Null);
    }
    out
}

/// 帯・サイドバーの材料（S3 #1881。判断は `tako_core::claude_mod::band_view`）。
///
/// worker は右パネル orch と同じ規則（`Workspace::workers_of`）で引き、1 本ずつ mod の報告・
/// 画面・コマンド状態の手掛かりを集める（報告は最大 1 秒に 1 回なので、画面の判定もその頻度）
fn band_view(host: &dyn ControlHost, pane: PaneId, now: Instant) -> core::BandView {
    let ws = host.workspace();
    let tab = ws.find_tab_of_pane(pane).and_then(|t| ws.get_tab(t));
    let workers = ws
        .workers_of(pane)
        .into_iter()
        .map(|p| worker_facts(host, p, now))
        .collect();
    core::band_view(core::BandInput {
        pane: pane.as_u64(),
        pane_title: tab
            .and_then(|t| t.tree().get(pane))
            .and_then(|p| p.title())
            .map(str::to_string),
        tab_title: tab.map(|t| t.title().to_string()),
        lang: tako_core::i18n::lang().as_str(),
        workers,
        ctx: lookup_pane(host, pane, now)
            .ok()
            .and_then(|s| s.report.context.clone()),
        rate_limits: account_rate_limits(host, pane, now),
        band_request: host.claude_mod().and_then(|h| h.band_request),
        thresholds: core::band_thresholds(),
    })
}

/// worker 1 本の手掛かり（判断は `tako_core::claude_mod::classify_worker`）
fn worker_facts(host: &dyn ControlHost, pane: &tako_core::Pane, now: Instant) -> core::WorkerFacts {
    use crate::claude_tui::DialogKind;
    let id = pane.id();
    let mod_turn = lookup_pane(host, id, now).ok().map(|s| s.report.turn);
    let limited = mod_turn.is_some()
        && account_rate_limits(host, id, now)
            .iter()
            .any(|l| l.percent_used >= 100.0);
    let session = host.session(id);
    let lines = session.map(|s| s.visible_lines()).unwrap_or_default();
    let dialog = crate::claude_tui::detect_choice_dialog(&lines);
    core::WorkerFacts {
        pane: id.as_u64(),
        name: pane
            .title()
            .or_else(|| pane.role())
            .map(str::to_string)
            .unwrap_or_else(|| format!("pane {}", id.as_u64())),
        mod_turn,
        limited,
        dialog_on_screen: dialog
            .as_ref()
            .is_some_and(|d| !d.kind.auto_accepted() && d.kind != DialogKind::UsageLimit),
        permission_on_screen: dialog
            .as_ref()
            .is_some_and(|d| d.kind == DialogKind::Permission),
        limit_dialog_on_screen: dialog
            .as_ref()
            .is_some_and(|d| d.kind == DialogKind::UsageLimit),
        screen_busy: !lines.is_empty()
            && crate::orchestrator::wait::screen_looks_busy(&lines.join("\n")),
        command: session.map(|s| s.command_state()).unwrap_or_default(),
    }
}

/// いまの時刻（epoch ms。mod の `$.clock.now()` と同じ物差し）
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
        // S3（#1881）: 帯の中継（`tako mod band on|off`）と A/B。ペインごとの帯の状態は行の report.band
        "band": {
            "request": hub.band_request,
            "legacy": core::s3_legacy(),
            "ctx_percent": core::band_thresholds().ctx_percent,
            "limit_percent": core::band_thresholds().limit_percent,
        },
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

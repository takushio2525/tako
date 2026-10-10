//! `tako mod`（`Request::Mod`）の dispatch 側（エピック #1877 / S1 #1879。FR-2.42）
//!
//! 判断と報告の型は `tako_core::claude_mod` が持つ。ここは「GUI のメモリにある
//! [`tako_core::claude_mod::ModHub`] を読み書きし、JSON に組む」だけ。
//!
//! | action | 中身 | CLI | MCP |
//! |---|---|---|---|
//! | `status`（既定） | 展開先・版・注入の有無・ペインごとの最終報告（鮮度つき） | `tako mod` | `tako_mod` |
//! | `on` / `off` | 設定 `claude_mod` の切替（次に作るペインから） | `tako mod on` / `off` | `tako_mod` |
//! | `band-on` / `band-off` | Claude Code の画面の帯を出す / 隠す（S3 #1881。正本は ui.json の `band.hidden` = #1960。報告の応答で全 mod へ中継） | `tako mod band on` / `off` | `tako_mod` |
//! | `ui` | 定型の UI 設定 ui.json の表示と変更（#1960。本体は [`crate::claude_mod_ui`]） | `tako mod ui …`（ローカル処理） | `tako_mod` |
//! | `install` / `uninstall` | 設定 dir ごとの `skills/tako` へ写しを置く / 印つきの写しだけ外す（S7-1 #1959。`dry_run` で判断だけ） | `tako mod install` / `uninstall [--dry-run]`（ローカル処理） | `tako_mod` |
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
pub const ACTIONS: &[&str] = &[
    "status",
    "on",
    "off",
    "band-on",
    "band-off",
    "ui",
    "install",
    "uninstall",
    "report",
];
/// MCP が受け付ける action（`report` は載せない。モジュール冒頭の理由）
pub const MCP_ACTIONS: &[&str] = &[
    "status",
    "on",
    "off",
    "band-on",
    "band-off",
    "ui",
    "install",
    "uninstall",
];

/// `tako setup` の段（設計書 §3.2。GUI 起動時と並ぶ 2 つ目の発火点）。
///
/// 展開するのは tako の data dir の中だけなので予告も `[y/N]` も要らない（Claude Code の
/// 設定 dir へ写しを置くのは別の段 = `claude_mod_install::run_setup_stage`。#1959）。
/// **止めない**: 展開に失敗しても tako は画面の読み取りで動くので、利用者へ残る作業にはしない
/// （1 行の注意だけ）
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
    ui: Option<tako_core::claude_mod_ui::UiRequest>,
    dry_run: bool,
) -> Result<Value, DispatchError> {
    match action.unwrap_or("status") {
        // S7-1（#1959）: 設定 dir ごとの skills/tako。GUI が知っている材料（mod の報告で見えた
        // 設定 dir・控えた claude の版・設定）で、CLI のローカル処理と同じ 1 実装を呼ぶ
        "install" | "uninstall" => {
            let hub = hub(host)?;
            let reported: Vec<String> = hub.reported_config_dirs.iter().cloned().collect();
            let ctx = crate::claude_mod_install::Context {
                enabled: hub.enabled,
                legacy: tako_core::claude_mod_install::legacy(),
                claude_version: hub.claude_version.clone(),
                verification: tako_core::paths::is_verification_process(),
            };
            crate::claude_mod_install::run_action(
                action.unwrap_or_default(),
                dry_run,
                &reported,
                &ctx,
            )
            .map_err(DispatchError::InvalidParams)
        }
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
            hub(host)?;
            // #1960: 正本は ui.json の band.hidden（`tako mod ui set band.hidden` と同じ 1 実装）
            let request = tako_core::claude_mod_ui::UiRequest {
                op: Some("set".into()),
                key: Some("band.hidden".into()),
                value: Some(json!(action == Some("band-off"))),
                ..Default::default()
            };
            crate::claude_mod_ui::run(&ui_path(host)?, &request).map_err(ui_error)?;
            let mut out = status(host)?;
            out["applies_to"] = json!(
                "動いている claude の帯へ次の報告（最大 15 秒）で届く。Claude Code の中で /tako band on|off \
                 を後から打てば、そちらが勝つ"
            );
            Ok(out)
        }
        "ui" => {
            crate::claude_mod_ui::run(&ui_path(host)?, &ui.unwrap_or_default()).map_err(ui_error)
        }
        "report" => accept_report(host, report, pane),
        other => Err(DispatchError::InvalidParams(format!(
            "未知の action: {other}（{}）",
            ACTIONS.join(" / ")
        ))),
    }
}

/// ui.json の置き場（ホストが決める。テストのホストは自分の一時ファイル）
fn ui_path(host: &dyn ControlHost) -> Result<std::path::PathBuf, DispatchError> {
    host.claude_mod_ui_path().ok_or_else(|| {
        DispatchError::Operation("データディレクトリが決まらない（HOME が無い）".into())
    })
}

fn ui_error(e: crate::claude_mod_ui::RunError) -> DispatchError {
    match e {
        crate::claude_mod_ui::RunError::Invalid(m) => DispatchError::InvalidParams(m),
        crate::claude_mod_ui::RunError::Refused(m) => DispatchError::Operation(m),
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
    // #1960: 帯のトグルの正本は ui.json。mod の中で切り替えた（`$.store` の時刻が新しい）なら取り込む
    if let (Some(band), Some(path), false) = (
        parsed.band.as_ref(),
        host.claude_mod_ui_path(),
        core::s3_legacy(),
    ) {
        crate::claude_mod_ui::import_band(&path, band);
    }
    let accepted = host
        .claude_mod_mut()
        .ok_or_else(|| DispatchError::Operation("この tako は mod の報告を受けない".into()))?
        .accept(pane_id.as_u64(), parsed, Instant::now());
    Ok(json!({
        "accepted": accepted.as_str(),
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
    let looked_up = lookup_pane(host, pane, now).ok();
    let session = host.session(pane);
    core::band_view(core::BandInput {
        pane: pane.as_u64(),
        pane_title: tab
            .and_then(|t| t.tree().get(pane))
            .and_then(|p| p.title())
            .map(str::to_string),
        tab_title: tab.map(|t| t.title().to_string()),
        lang: tako_core::i18n::lang().as_str(),
        workers,
        ctx: looked_up.and_then(|s| s.report.context.clone()),
        rate_limits: account_rate_limits(host, pane, now),
        thresholds: core::band_thresholds(),
        // #1960: 検証済みの ui.json（読めない部分は既定。帯のトグルの中継もここから作る）
        ui: host
            .claude_mod_ui_path()
            .map(|p| crate::claude_mod_ui::view(&p))
            .unwrap_or_default(),
        // #1962: バーを描かせるか（利用者の statusLine の有無 × 画面の末尾の数値）とボタンの引数の材料
        status_line: looked_up.and_then(|s| s.report.status_line),
        screen_shows_usage: session
            .is_some_and(|s| core::screen_shows_usage(&s.tail_lines(core::USAGE_SCAN_LINES))),
        cwd: session
            .and_then(|s| s.cwd())
            .map(|p| p.display().to_string()),
        s7_legacy: core::s7_legacy(),
    })
}

/// 画面下のステータスバーの claude の区画（5h / 7d / ctx）を誰が出すか（#1962。判断は
/// `tako_core::claude_mod::claude_bar_owner`）。`focused` はフォーカス中のペイン。その新鮮な報告
/// （45 秒以内。引き当ては一次ソースと同じ [`lookup_pane`] の 1 本）が無ければ（mod なし・古い・
/// codex / agy / シェル・S2 の A/B）今のまま tako が出す
pub fn status_bar_owner(
    host: &dyn ControlHost,
    focused: PaneId,
    now: Instant,
) -> core::ClaudeBarOwner {
    let report = lookup_pane(host, focused, now).ok().map(|s| &s.report);
    core::claude_bar_owner(report, core::s7_legacy())
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
pub(crate) fn epoch_ms() -> u64 {
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
    let dormant = hub.dormant.get(&raw);
    let (state, reason) = match (stored, injection) {
        // #1959: 利用者が Claude Code 側で止めた（mod の最後の報告）。休眠した mod はもう
        // 報告しないので鮮度に依らず出す
        _ if dormant.is_some() => ("user_disabled", reason_json(&OffReason::UserDisabled)),
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
        // #1962: 帯を描けているか（新鮮で renders を送ってくる mod だけ）
        if core::is_fresh(s.received, now) {
            if let Some(band) = band_state_json(&s.report) {
                row["band"] = band;
            }
        }
    }
    row
}

/// 帯の状態（#1962。`drawn` / `not_drawn` / `hidden_by_other_mod`）。renders を送らない mod
/// （S7-3 前・A/B の S3 の描き方）は `None`
fn band_state_json(report: &core::ModReport) -> Option<Value> {
    let renders = report.renders.as_ref()?;
    if core::band_hidden_by_other_mod(report) {
        return Some(json!({
            "state": "hidden_by_other_mod",
            "message": "帯は他の mod に隠された（Claude Code の帯のフックが呼ばれていない = 外側の mod が next を\
                呼ばずに帯を描いている）。入力欄の下の行のバーとサイドバー（/tako）はそのまま使える",
        }));
    }
    Some(json!({
        "state": if renders.band { "drawn" } else { "not_drawn" },
        "buttons": renders.buttons,
        "usage_bar": renders.usage_bar,
    }))
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

/// ステータスバーの claude の区画の持ち主（`tako mod` の `status_bar`。フォーカス中のペインつき）
pub fn status_bar_json(host: &dyn ControlHost, now: Instant) -> Value {
    let focused = host.workspace().active_tab().tree().focused();
    let mut out = status_bar_owner(host, focused, now).to_json();
    out["pane"] = json!(focused.as_u64());
    out
}

/// `tako mod`（status）
fn status(host: &dyn ControlHost) -> Result<Value, DispatchError> {
    let hub = hub(host)?;
    let now = Instant::now();
    let ui_path = host.claude_mod_ui_path();
    let ui = ui_path.as_deref().map(crate::claude_mod_ui::load);
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
        "user_disabled": count("user_disabled"),
    });
    // S7-1（#1959）: 設定 dir ごとの skills/tako（読むだけ。報告で見えた dir も含む）
    let reported: Vec<String> = hub.reported_config_dirs.iter().cloned().collect();
    let skills = crate::claude_mod_install::status_json(
        &reported,
        hub.claude_version.as_deref(),
        hub.enabled,
    );
    Ok(json!({
        "enabled": hub.enabled,
        "injecting": injection.is_on(),
        // S3（#1881）: 帯の中継と A/B。ペインごとの帯の状態は行の report.band。
        // 中継の正本は ui.json の band.hidden（#1960）
        "band": {
            "request": ui.as_ref().and_then(|u| u.config.band_request()),
            "hidden": ui.as_ref().is_some_and(|u| u.config.band.hidden),
            "legacy": core::s3_legacy(),
            // #1962: S3 の描き方へ戻す A/B（TAKO_1877_S7_LEGACY）
            "s7_legacy": core::s7_legacy(),
            "ctx_percent": core::band_thresholds().ctx_percent,
            "limit_percent": core::band_thresholds().limit_percent,
        },
        // #1962: 画面下のステータスバーの claude の区画（5h / 7d / ctx）を誰が出しているか
        // （フォーカス中のペインの mod がバーを描いていれば mod = ステータスバーからは外す）
        "status_bar": status_bar_json(host, now),
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
        // #1960: 定型の UI 設定（詳細と変更は `tako mod ui`）
        "ui": ui_path
            .as_deref()
            .zip(ui.as_ref())
            .map(|(path, loaded)| crate::claude_mod_ui::status_json(path, loaded)),
        "skills": skills,
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

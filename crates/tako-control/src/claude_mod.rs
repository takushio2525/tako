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

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use serde_json::{json, Value};
use tako_core::claude_mod::{
    self as core, Accepted, ModHub, ModRateLimit, ModUnavailable, OffReason, StoredReport,
};
use tako_core::claude_mod_ui::Vocab;
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
    // #1963: GPUI のカード帯の判断（描画のたびに引く）が ui.json を読まずに済むよう、
    // cards.place は報告を受けるたびにここで写す。#1964: chat.place も同じ（GUI モードの表示の判断）
    let (cards_place, chat_place) = host
        .claude_mod_ui_path()
        .map(|p| {
            let ui = crate::claude_mod_ui::view(&p);
            (ui.cards.place, ui.chat.place)
        })
        .unwrap_or_default();
    let hub = host
        .claude_mod_mut()
        .ok_or_else(|| DispatchError::Operation("この tako は mod の報告を受けない".into()))?;
    hub.cards_place = cards_place;
    hub.chat_place = chat_place;
    let accepted = hub.accept(pane_id.as_u64(), parsed, Instant::now());
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
        // #1963: このペインのコマンドカード（mod が会話の行と帯に描く。正本は tako の store）
        cards: host
            .command_cards()
            .map(|c| c.list(Some(pane)).into_iter().cloned().collect())
            .unwrap_or_default(),
        cards_legacy: core::cards_legacy(),
        // #2159: 会話の行のカードにクリックで押す「実行 / コピー」を置いてよいか
        card_click: card_click(host),
        // #1964: チャット風の描画の組は表示モードで変わる（ターミナル表示の既定はコードのコピーだけ）
        ui_mode_gui: host.ui_mode().is_gui(),
        chat_legacy: core::chat_legacy(),
    })
}

/// tako がペインの TUI へ左クリックを渡すか（#2159。応答の `view.cards.click` と `tako mod` の `cards.click`）。
/// `tako mouse` の設定（#1961）が on で、#1961 の A/B（クリックは常に tako の選択）でも #2159 の A/B でも
/// ないときだけ真。偽なら mod は会話の行のカードにボタンを置かず、帯のホットキーで押す案内だけを描く
/// （押せないボタンを見せない = §9.10）。全画面かどうか（main screen はマウス報告を要求しない）は mod が
/// 描くときに `viewport.isFullscreen` で見る
pub fn card_click(host: &dyn ControlHost) -> bool {
    let mouse = host.mouse_forward_status();
    mouse.enabled && !mouse.legacy && !core::card_click_legacy()
}

/// GUI モードでそのペインのチャットを誰が出すか（#1964。判断は `tako_core::claude_mod::chat_owner` の 1 本）。
/// 報告は一次ソースと同じ [`lookup_pane`]（45 秒）で引き、置き場は報告のたびに写した ui.json の
/// `chat.place`（[`core::ModHub::chat_place`]）。A/B は `TAKO_1964_LEGACY` と `TAKO_1877_S7_LEGACY`
/// （S3 の描き方はチャットを描かない）。GPUI はペインの表示を決めるたびに引く（GUI モードのときだけ）
pub fn chat_owner(host: &dyn ControlHost, pane: PaneId, now: Instant) -> core::ChatOwner {
    core::chat_owner(core::ChatOwnerInput {
        place: host.claude_mod().map(|h| h.chat_place).unwrap_or_default(),
        ui_mode_gui: host.ui_mode().is_gui(),
        stored: lookup_pane(host, pane, now).ok(),
        legacy: core::chat_legacy() || core::s7_legacy(),
    })
}

/// チャットの持ち主の JSON（`tako mod` の各ペインの `chat` と `tako ui-mode` の診断。#1964）
pub fn chat_owner_json(host: &dyn ControlHost, pane: PaneId, now: Instant) -> Value {
    let owner = chat_owner(host, pane, now);
    json!({ "drawn_by": owner.as_str(), "reason": owner.reason() })
}

/// GPUI のカード帯にそのカードを出すのは誰か（#1963。判断は `tako_core::claude_mod::card_owner`
/// の 1 本）。報告は一次ソースと同じ [`lookup_pane`]（45 秒）で引き、置き場は報告のたびに写した
/// ui.json の `cards.place`（[`core::ModHub::cards_place`]）。A/B は `TAKO_1963_LEGACY` と
/// `TAKO_1877_S7_LEGACY`（S3 の描き方はカードを描かない）
pub fn card_owner(
    host: &dyn ControlHost,
    card: &tako_core::CommandCard,
    now: Instant,
) -> core::CardOwner {
    core::card_owner(core::CardOwnerInput {
        place: host.claude_mod().map(|h| h.cards_place).unwrap_or_default(),
        stored: lookup_pane(host, card.pane(), now).ok(),
        legacy: core::cards_legacy() || core::s7_legacy(),
        card: card.id().as_u64(),
        shown_at: card.shown_at(),
        now,
    })
}

/// そのペインのカードのうち、GPUI のカード帯から外すもの（#1963。mod が Claude Code の画面に描く）。
/// GPUI はカード帯を組むたびにこれを引く（ペイン × フレーム。カードが無いペインは呼ばれない）
pub fn cards_drawn_by_mod(host: &dyn ControlHost, pane: PaneId, now: Instant) -> BTreeSet<u64> {
    host.command_cards()
        .map(|cards| {
            cards
                .list(Some(pane))
                .into_iter()
                .filter(|card| card_owner(host, card, now).yields())
                .map(|card| card.id().as_u64())
                .collect()
        })
        .unwrap_or_default()
}

/// カードの持ち主の JSON（`tako show-command --list` / `show` の各カードの `drawn_by`。#1963）
pub fn card_owner_json(
    host: &dyn ControlHost,
    card: &tako_core::CommandCard,
    now: Instant,
) -> Value {
    let owner = card_owner(host, card, now);
    json!({ "drawn_by": owner.as_str(), "reason": owner.reason() })
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
            // #1891: 理由はここでは材料なし（候補を並べた文）。背景の段（[`finish_status`]）が
            // ペインの claude を調べられたら 1 つに決めて差し替える
            None if claude_on_screen => {
                (NO_REPORT, no_report_reason(core::classify_no_report(None)))
            }
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

/// ペインの行の状態のうち「claude が画面に居るのに報告が無い」（#1891 で `check-health` も数える）
const NO_REPORT: &str = "no_report";

/// 報告の無いペインの理由（#1891。`code` は S2 の語彙 `mod_absent` のまま、`cause` が判別した 1 つ）
fn no_report_reason(cause: core::NoReportCause) -> Value {
    json!({
        "code": ModUnavailable::Absent.code(),
        "cause": cause.code(),
        "message": cause.describe(),
    })
}

/// `tako mod`（status）の UI スレッドの段で写した材料（#1891）。
///
/// UI スレッドではペインの行（報告の鮮度・注入の記録・画面の claude）までを組み、ファイル
/// （設定 dir ごとの skills/tako）と実プロセス（報告の無い claude のコマンド行と起動時の env）は
/// 背景の段（[`finish_status_in_background`]）で読む。**`tako mod` と `tako check-health` は
/// この同じ 2 段を通る**（check-health は [`health_section`] で項目を選ぶだけで、判定を持たない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusCtx {
    /// skills・ui.json・調べの結果を除いた応答（行まで組み終えたもの）
    out: Value,
    /// ui.json の置き場（読むのは背景の段）
    ui_path: Option<std::path::PathBuf>,
    /// skills の材料
    reported: Vec<String>,
    claude_version: Option<String>,
    enabled: bool,
    /// env の印を比べる展開先
    plugin_dir: Option<std::path::PathBuf>,
    /// 報告の無い claude を調べるペイン（[`core::NO_REPORT_PROBE_LIMIT`] まで）
    probes: Vec<crate::agent_resume::PaneProbe>,
    /// 上限の外で調べなかった数
    over_limit: usize,
}

/// `tako mod`（status）の UI スレッドの段（#1891。**子プロセスもファイルも触らない** = ui.json・skills の
/// 読みと報告の無い claude の調べは背景の段）
pub fn status_ctx(host: &dyn ControlHost) -> Result<StatusCtx, DispatchError> {
    let hub = hub(host)?;
    let now = Instant::now();
    let injection = hub.decide(core::ab_off());
    let legacy = core::no_report_legacy();
    let mut panes = Vec::new();
    let mut probes = Vec::new();
    let mut over_limit = 0;
    for tab in host.workspace().tabs() {
        for pane in tab.tree().panes() {
            let id = pane.id();
            let on_screen = host.session(id).is_some_and(|s| {
                !matches!(
                    crate::claude_tui::detect(&s.visible_lines()),
                    crate::claude_tui::ClaudeScreen::Unknown
                )
            });
            let mut row = pane_row(hub, id, pane.title(), on_screen, now);
            // #1964: GUI モードでこのペインのチャットを誰が出すか（mod が描いている組は report.renders.chat）
            if hub.reports.contains_key(&id.as_u64()) {
                row["chat"] = chat_owner_json(host, id, now);
            }
            // #1891: 報告の無い claude は背景の段で調べる（上限まで。A/B は調べない）
            if row["state"] == NO_REPORT && !legacy {
                if probes.len() < core::NO_REPORT_PROBE_LIMIT {
                    probes.push(crate::agent_resume::PaneProbe {
                        pane: id.as_u64(),
                        backend: host.backend_session(id),
                        pty_pid: host.session(id).and_then(|s| s.child_pid()),
                    });
                } else {
                    over_limit += 1;
                }
            }
            panes.push(row);
        }
    }
    let count = |state: &str| panes.iter().filter(|p| p["state"] == state).count();
    let summary = json!({
        "reporting": count("reporting"),
        "stale": count("stale"),
        "no_report": count(NO_REPORT),
        "not_injected": count("not_injected"),
        "user_disabled": count("user_disabled"),
    });
    let out = json!({
        "enabled": hub.enabled,
        "injecting": injection.is_on(),
        // S3（#1881）: 帯の中継と A/B。ペインごとの帯の状態は行の report.band。
        // 中継の正本は ui.json の band.hidden（#1960。読むのは背景の段 = finish_status）
        "band": {
            "request": Value::Null,
            "hidden": false,
            "legacy": core::s3_legacy(),
            // #1962: S3 の描き方へ戻す A/B（TAKO_1877_S7_LEGACY）
            "s7_legacy": core::s7_legacy(),
            "ctx_percent": core::band_thresholds().ctx_percent,
            "limit_percent": core::band_thresholds().limit_percent,
        },
        // #1962: 画面下のステータスバーの claude の区画（5h / 7d / ctx）を誰が出しているか
        // （フォーカス中のペインの mod がバーを描いていれば mod = ステータスバーからは外す）
        "status_bar": status_bar_json(host, now),
        // #1963: コマンドカードを誰が描くか（カードごとの持ち主は `tako show-command --list` の drawn_by）
        "cards": {
            "place": hub.cards_place.as_str(),
            "legacy": core::cards_legacy(),
            "pickup_grace_ms": core::CARD_PICKUP_GRACE.as_millis() as u64,
            // #2159: 会話の行のカードをクリックで押せるか（偽 = 帯のホットキーで押す）と、その A/B
            "click": card_click(host),
            "click_legacy": core::card_click_legacy(),
        },
        // #1964: チャット風の描画（吹き出し・コードのコピー・ツールの要約）。auto の GUI モードでの持ち主は
        // 判断点が決まるまで tako（GPUI のチャット）。ペインごとの持ち主は panes[].chat
        "chat": {
            "place": hub.chat_place.as_str(),
            "auto_in_gui": core::AUTO_CHAT_OWNER_GUI.as_str(),
            "ui_mode": host.ui_mode().as_str(),
            "legacy": core::chat_legacy(),
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
        // #1960: 定型の UI 設定（詳細と変更は `tako mod ui`。読むのは背景の段 = finish_status）
        "ui": Value::Null,
        // S7-1（#1959）: 設定 dir ごとの skills/tako（読むのは背景の段 = finish_status）
        "skills": Value::Null,
        // #1891: 報告の無い claude を調べたか（finish_status が埋める）
        "no_report_probe": Value::Null,
        "panes": panes,
    });
    Ok(StatusCtx {
        out,
        ui_path: host.claude_mod_ui_path(),
        reported: hub.reported_config_dirs.iter().cloned().collect(),
        claude_version: hub.claude_version.clone(),
        enabled: hub.enabled,
        plugin_dir: hub.plugin_dir.clone(),
        probes,
        over_limit,
    })
}

/// 報告の無い claude を調べる（#1891。**子プロセスを起こすので背景の段だけ**から呼ぶ）。
///
/// 走査（プロセスの表 + 器の `list-panes -a`。どちらも上限つき・libproc）は 1 回で、調べるペインが
/// 無ければ走査もしない。起動時の env を読めるのは macOS だけ（`ports::process_env_vars`）なので、
/// ほかの OS では調べない = 理由は候補を並べた文のまま。調べなかったら `None`
fn probe_no_report(ctx: &StatusCtx) -> Option<HashMap<u64, core::NoReportEvidence>> {
    if ctx.probes.is_empty() || !cfg!(target_os = "macos") {
        return None;
    }
    let snap = crate::agents::ProcessSnapshot::capture();
    Some(
        ctx.probes
            .iter()
            .map(|probe| {
                (
                    probe.pane,
                    no_report_evidence(probe, &snap, ctx.plugin_dir.as_deref()),
                )
            })
            .collect(),
    )
}

/// ペイン 1 つぶんの材料。claude かどうかは PC リソースの分類（`resources::kind_of`。#1993）の 1 実装で見る
fn no_report_evidence(
    probe: &crate::agent_resume::PaneProbe,
    snap: &crate::agents::ProcessSnapshot,
    plugin_dir: Option<&std::path::Path>,
) -> core::NoReportEvidence {
    let claude = crate::agent_resume::pane_pids_newest_first(probe, snap)
        .into_iter()
        .find(|&pid| {
            tako_core::resources::kind_of("", snap.argv(pid), false)
                == tako_core::resources::ProcKind::Agent("claude")
        });
    let Some(pid) = claude else {
        return core::NoReportEvidence::default();
    };
    core::NoReportEvidence {
        argv: snap.argv(pid).map(str::to_string),
        injected_env: tako_core::ports::process_env_vars(pid, &[core::PLUGIN_DIRS_ENV, "PATH"])
            .and_then(|env| core::env_has_injection(&env, plugin_dir)),
    }
}

/// `tako mod`（status）の仕上げ（#1891）。`evidence` は背景の段が調べた材料（`None` = 調べていない
/// = 同期の腕・A/B・調べるペインが無い・macOS 以外）。報告の無いペインの理由は
/// [`core::classify_no_report`] の 1 本で決める
fn finish_status(ctx: StatusCtx, evidence: Option<&HashMap<u64, core::NoReportEvidence>>) -> Value {
    let StatusCtx {
        mut out,
        ui_path,
        reported,
        claude_version,
        enabled,
        probes,
        over_limit,
        ..
    } = ctx;
    // #1960: 定型の UI 設定（帯の中継の正本 = band.hidden もここから）
    if let Some(path) = ui_path.as_deref() {
        let loaded = crate::claude_mod_ui::load(path);
        out["band"]["request"] = json!(loaded.config.band_request());
        out["band"]["hidden"] = json!(loaded.config.band.hidden);
        out["ui"] = crate::claude_mod_ui::status_json(path, &loaded);
    }
    // S7-1（#1959）: 設定 dir ごとの skills/tako（読むだけ。報告で見えた dir も含む）
    out["skills"] =
        crate::claude_mod_install::status_json(&reported, claude_version.as_deref(), enabled);
    if let (Some(evidence), Some(rows)) = (evidence, out["panes"].as_array_mut()) {
        for row in rows.iter_mut().filter(|r| r["state"] == NO_REPORT) {
            let found = row["pane"].as_u64().and_then(|pane| evidence.get(&pane));
            if let Some(found) = found {
                row["reason"] = no_report_reason(core::classify_no_report(Some(found)));
            }
        }
    }
    out["no_report_probe"] = json!({
        "ran": evidence.is_some(),
        "panes": probes.len(),
        "limit": core::NO_REPORT_PROBE_LIMIT,
        "over_limit": over_limit,
    });
    out
}

/// 背景の段（[`StatusCtx`] の続き）。報告の無い claude を調べてから仕上げる。**UI スレッドで呼ばない**
/// （`OffloadJob::ModStatus` と `tako check-health` の背景の段が呼ぶ）
pub fn finish_status_in_background(ctx: StatusCtx) -> Value {
    let evidence = probe_no_report(&ctx);
    finish_status(ctx, evidence.as_ref())
}

/// `tako mod`（status）の同期の腕（テスト・直呼び・`on` / `off` / `band-*` の応答・A/B）。
/// 実プロセスは調べない（UI スレッドで走りうるので）= 報告の無いペインの理由は候補を並べた文
fn status(host: &dyn ControlHost) -> Result<Value, DispatchError> {
    Ok(finish_status(status_ctx(host)?, None))
}

// --- `tako check-health` の mod の項目（#1891） -------------------------------------

/// check-health に載せるキー（`tako mod --json` と同じキー・同じ値）
const HEALTH_KEYS: &[&str] = &[
    "enabled",
    "injecting",
    "reason",
    "claude_version",
    "min_claude_version",
    "summary",
    "no_report_probe",
    "skills",
];
/// check-health に載せるペインの行のキー（行の報告の中身 = ctx・使用制限・帯は載せない）
const HEALTH_ROW_KEYS: &[&str] = &[
    "pane",
    "title",
    "state",
    "injected",
    "claude_on_screen",
    "reason",
];

/// `tako check-health` の `claude_mod` 節（#1891）。**判定を持たない**: `tako mod`（status）の
/// 応答（[`finish_status_in_background`] の結果）から同じキーを写すだけ
pub fn health_section(status: &Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("available".into(), json!(true));
    for key in HEALTH_KEYS {
        out.insert((*key).into(), status[*key].clone());
    }
    let rows: Vec<Value> = status["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            let mut kept = serde_json::Map::new();
            for key in HEALTH_ROW_KEYS {
                if let Some(v) = row.get(*key) {
                    kept.insert((*key).into(), v.clone());
                }
            }
            Value::Object(kept)
        })
        .collect();
    out.insert("panes".into(), Value::Array(rows));
    Value::Object(out)
}

/// mod の項目が分からないとき（GUI に届かない・mod を扱わない tako）の `claude_mod` 節（#1891）
pub fn health_unavailable(message: &str) -> Value {
    json!({
        "available": false,
        "reason": { "code": "unavailable", "message": message },
    })
}

/// 注入しない理由のうち、利用者の選択ではなく直す手がある（= check-health の指摘に出す）もの
const HEALTH_FIXABLE_OFF: &[&str] = &["not_installed", "no_cli", "claude_too_old"];

/// `claude_mod` 節から check-health の `issues` を組む（#1891。節の値を読むだけ）
pub fn health_issues(section: &Value) -> Vec<Value> {
    let mut issues = Vec::new();
    if section["available"] != json!(true) {
        return issues;
    }
    let off_code = section["reason"]["code"].as_str().unwrap_or_default();
    if section["injecting"] == json!(false) && HEALTH_FIXABLE_OFF.contains(&off_code) {
        issues.push(json!({
            "level": "info",
            "check": "claude_mod",
            "message": text::injection_line(section),
        }));
    }
    let lines: Vec<String> = section["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["state"] == NO_REPORT)
        .filter_map(text::reason_line)
        .map(|line| line.trim().to_string())
        .collect();
    if !lines.is_empty() {
        issues.push(json!({
            "level": "info",
            "check": "claude_mod_no_report",
            "message": format!(
                "claude が動いているのに mod の報告が無いペインが {} 個（{}）。詳しくは tako mod",
                lines.len(),
                lines.join(" / ")
            ),
        }));
    }
    issues
}

/// 人が読む行（#1891）。`tako mod` と `tako check-health` の CLI が**同じ関数で**同じ字面を出す
pub mod text {
    use serde_json::Value;

    /// 1 行目（注入するか・しないなら理由。分からなければ不明と理由）
    pub fn injection_line(status: &Value) -> String {
        let reason = status["reason"]["message"].as_str();
        if status["available"] == Value::Bool(false) {
            return format!("tako mod: 不明（{}）", reason.unwrap_or("-"));
        }
        match (status["injecting"].as_bool(), reason) {
            (Some(true), _) => "tako mod: 新しく作るペインへ注入する".to_string(),
            (_, Some(reason)) => format!("tako mod: 注入しない（{reason}）"),
            _ => "tako mod: 注入しない".to_string(),
        }
    }

    /// ペインの行の理由（報告が届いていない行だけ。`  pane N  状態  理由`）
    pub fn reason_line(row: &Value) -> Option<String> {
        let reason = row["reason"]["message"].as_str()?;
        Some(format!(
            "  pane {}  {}  {reason}",
            row["pane"].as_u64().unwrap_or(0),
            row["state"].as_str().unwrap_or("-"),
        ))
    }
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
        // #1891: 行を組む段（UI スレッド）では材料が無い = 候補を並べた文
        assert_eq!(row["reason"]["cause"], "undetermined");
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

    // --- #1891: 報告の無いペインの理由と check-health の項目 ---

    /// 4 ペイン（1 = --safe-mode / 2 = 注入前の claude / 3 = tako mod off / 4 = 報告あり）の材料
    fn status_ctx_1891(hub: &ModHub, now: Instant) -> StatusCtx {
        let rows: Vec<Value> = (1..=4)
            .map(|p| pane_row(hub, PaneId::from_raw(p), None, true, now))
            .collect();
        StatusCtx {
            ui_path: None,
            out: json!({
                "enabled": true,
                "injecting": true,
                "reason": null,
                "band": {"legacy": false},
                "claude_version": "2.1.294",
                "min_claude_version": core::MIN_CLAUDE_VERSION,
                "summary": {"reporting": 1, "no_report": 2, "not_injected": 1},
                "skills": null,
                "no_report_probe": null,
                "panes": rows,
            }),
            reported: Vec::new(),
            claude_version: Some("2.1.294".into()),
            enabled: true,
            plugin_dir: hub.plugin_dir.clone(),
            probes: Vec::new(),
            over_limit: 0,
        }
    }

    fn hub_1891(now: Instant) -> ModHub {
        let mut hub = hub();
        let on = hub.decide(false);
        hub.record_spawn(1, &on);
        hub.record_spawn(2, &on);
        hub.record_spawn(3, &core::Injection::Off(OffReason::Disabled));
        hub.record_spawn(4, &on);
        hub.accept(4, report("idle"), now);
        hub
    }

    #[test]
    fn 報告の無いペインの理由は背景の材料で1つに決まる() {
        let now = Instant::now();
        let hub = hub_1891(now);
        let evidence = HashMap::from([
            (
                1,
                core::NoReportEvidence {
                    argv: Some("claude --safe-mode".into()),
                    injected_env: Some(true),
                },
            ),
            (
                2,
                core::NoReportEvidence {
                    argv: Some("claude".into()),
                    injected_env: Some(false),
                },
            ),
        ]);
        let status = finish_status(status_ctx_1891(&hub, now), Some(&evidence));
        let rows = status["panes"].as_array().unwrap();
        assert_eq!(rows[0]["reason"]["cause"], "safe_mode");
        assert_eq!(rows[0]["reason"]["code"], "mod_absent");
        assert_eq!(
            rows[0]["reason"]["message"],
            core::NoReportCause::SafeMode.describe()
        );
        assert_eq!(rows[1]["reason"]["cause"], "before_injection");
        // 注入しなかった行・報告のある行は調べの材料で変わらない
        assert_eq!(rows[2]["reason"]["code"], "disabled");
        assert_eq!(rows[3]["state"], "reporting");
        assert_eq!(status["no_report_probe"]["ran"], true);
        assert_eq!(
            status["no_report_probe"]["limit"],
            core::NO_REPORT_PROBE_LIMIT
        );

        // 同期の腕（調べない）は候補を並べた文のまま
        let sync = finish_status(status_ctx_1891(&hub, now), None);
        assert_eq!(sync["panes"][0]["reason"]["cause"], "undetermined");
        assert_eq!(sync["no_report_probe"]["ran"], false);
    }

    #[test]
    fn check_healthの節はtako_modの応答から同じキーと値を写すだけ() {
        let now = Instant::now();
        let hub = hub_1891(now);
        let evidence = HashMap::from([(
            1,
            core::NoReportEvidence {
                argv: Some("claude --safe-mode".into()),
                injected_env: Some(true),
            },
        )]);
        let status = finish_status(status_ctx_1891(&hub, now), Some(&evidence));
        let section = health_section(&status);
        assert_eq!(section["available"], true);
        for key in HEALTH_KEYS {
            assert_eq!(section[*key], status[*key], "{key}");
        }
        // 帯・カード等の mod の表示の設定は載せない
        assert!(section.get("band").is_none());
        let rows = section["panes"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for (row, full) in rows.iter().zip(status["panes"].as_array().unwrap()) {
            for key in HEALTH_ROW_KEYS {
                assert_eq!(row.get(*key), full.get(*key), "{key}");
            }
            // 報告の中身（ctx・使用制限・帯）は載せない
            assert!(row.get("report").is_none());
            // 人が読む行は `tako mod` と同じ関数 = 同じ字面
            assert_eq!(text::reason_line(row), text::reason_line(full));
        }
        assert_eq!(
            text::reason_line(&rows[0]).unwrap(),
            format!(
                "  pane 1  no_report  {}",
                core::NoReportCause::SafeMode.describe()
            )
        );
        assert_eq!(
            text::reason_line(&rows[3]),
            None,
            "報告のある行は理由を出さない"
        );
        assert_eq!(
            text::injection_line(&section),
            text::injection_line(&status)
        );
        assert_eq!(
            text::injection_line(&section),
            "tako mod: 新しく作るペインへ注入する"
        );

        // 指摘は報告の無い claude の行だけ（tako mod off の行は利用者の選択なので出さない）
        let issues = health_issues(&section);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0]["check"], "claude_mod_no_report");
        let message = issues[0]["message"].as_str().unwrap();
        assert!(message.contains("2 個"), "{message}");
        assert!(
            message.contains("pane 1  no_report  claude が --safe-mode"),
            "{message}"
        );
    }

    #[test]
    fn check_healthのmodの項目は分からなければ理由つきの不明() {
        let section = health_unavailable("tako アプリへ届かない（x）");
        assert_eq!(section["available"], false);
        assert_eq!(
            text::injection_line(&section),
            "tako mod: 不明（tako アプリへ届かない（x））"
        );
        assert!(health_issues(&section).is_empty());

        // 直す手がある「注入しない」は指摘に出る。利用者の選択（off）は出さない
        let off = |reason: OffReason| {
            json!({
                "available": true,
                "injecting": false,
                "reason": reason_json(&reason),
                "panes": [],
            })
        };
        let too_old = health_issues(&off(OffReason::ClaudeTooOld("2.1.280".into())));
        assert_eq!(too_old.len(), 1);
        assert_eq!(too_old[0]["check"], "claude_mod");
        assert!(too_old[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("tako mod: 注入しない（claude 2.1.280 は下限"));
        assert!(health_issues(&off(OffReason::Disabled)).is_empty());
        assert!(health_issues(&off(OffReason::ClaudeUnknown)).is_empty());
    }
}

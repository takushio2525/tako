//! 2 秒 tick の判定がペインの画面から読むもの（Issue #2647）。
//!
//! # なぜ背景で読むか
//!
//! 2 秒ごとの定期更新（`periodic_prep`）の 5 つの段 — エージェントのメトリクス（#357）・
//! キュー滞留の救出（#572）・master の引き継ぎの知らせ（#749）・利用制限からの自動復帰（#813）・
//! ネット復帰の再開（#2112）— は、どれもペインごとに端末のロックを取って画面を平文へ組み、
//! 行を解析していた。1 tick にエージェントのペイン 1 本あたり約 6 回のロックを UI スレッドで取り、
//! ペインの数に比例して重くなる（段 1 の実測・debug ビルド・24 本: 5 段の合計 p50 15.6ms /
//! p95 23.0ms、うち利用制限の段が p50 11.9ms = 行の解析が約 3/4・装飾つきの画面の組み立てが残り）。
//! PTY のスレッドが出力の解析でロックを握っていれば、その間も UI スレッドが待つ
//! （本番の perf.log 10/28〜11/02: `periodic_prep` の専有 247 件・平均 76ms・最大 492ms）。
//!
//! いまは UI スレッドでペインへの手（[`PaneAccess`]。#1200 / #2646 と同じ、スレッドを越えられる手）を
//! 集めるだけにし（[`plan`]）、ロック・組み立て・解析は背景のスレッドで **1 ペインにつき 1 回の
//! ロック**で済ませる（[`read`]）。master の引き継ぎの閾値の設定（profiles/*.yaml）の読み直しも
//! ここで済ませる。**判定（追跡・猶予・送るかどうか）はこれまでどおり UI スレッドの各段が同じ関数で
//! 行う**ので、判定の結果と発火の時機は変わらない。
//!
//! # 1 実装
//!
//! 各段は画面を [`TickSource`] 越しに読む。修正前の形（`TerminalSession` をその場で読む）と
//! 背景で採った 1 枚（[`PaneTick`]）の両方がこれを満たし、解析（メトリクス・キューのヒント・
//! 利用制限の判定・接続エラーの判定）は trait の既定の 1 実装を両方が通る。背景はその既定の実装を
//! 先に走らせて答えを持ち帰るだけ = 解析の規則が 2 つに割れない。
//!
//! A/B は `TAKO_2647_LEGACY=1`（[`legacy`]。各段がその場で `TerminalSession` を読む = 修正前）。
//! 検証用の注入は `TAKO_2647_INJECT_READ_MS`（画面を読むロック 1 回ごとにその ms だけ待つ =
//! PTY のスレッドがロックを握っている状態の再現。負荷は作らない）

use std::cell::RefCell;
use std::collections::HashMap;

use tako_control::limit_stop;
use tako_control::net_stop::{self, NetScreen};
use tako_core::limit_resume::LimitStopKind;
use tako_core::terminal::{agent_metrics_from_lines, c2_legacy, AGENT_TUI_TAIL_LINES};
use tako_core::{AgentMetrics, InputStatus, PaneAccess, PaneId, TerminalSession};

use crate::net_autoresume::NetRead;
use crate::TakoApp;

/// A/B の入口（`TAKO_2647_LEGACY=1`）。**ゲートはここ 1 か所**: 定期更新は [`plan`] が `None` を
/// 返したら背景で読まず、各段は [`TickScreens::Live`] でその場の画面を読む（修正前の形）
pub(crate) fn legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2647_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 検証用の注入（`TAKO_2647_INJECT_READ_MS`）。画面を読むロック 1 回ごとにこの ms だけ待つ
fn inject_read_delay() {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms = *MS.get_or_init(|| {
        std::env::var("TAKO_2647_INJECT_READ_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    });
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// 利用制限の段（#813）が 1 ペインから読むもの
#[derive(Debug, Clone)]
pub(crate) struct LimitRead {
    /// 可視行のすべて（`TerminalSession::visible_lines` と同じ）
    pub(crate) lines: Vec<String>,
    /// 画面の指紋（`limit_stop::screen_fingerprint`）
    pub(crate) fingerprint: u64,
    /// 上限で止まっているか・型・根拠の 1 行（`limit_stop::classify_limit_stop`）
    pub(crate) classified: Option<(LimitStopKind, String)>,
}

/// 判定が画面から読む口。修正前の `TerminalSession` と、背景で採った 1 枚（[`PaneTick`]）の両方が満たす。
///
/// 解析（下の既定の実装）は **1 実装**で、背景はこれを先に走らせて答えを持ち帰る
pub(crate) trait TickSource {
    fn is_alt_screen(&self) -> bool;
    fn tail_lines(&self, n: usize) -> Vec<String>;
    fn visible_lines(&self) -> Vec<String>;
    fn analyze_input(&self) -> Option<InputStatus>;
    fn lines_with_history(&self, history: usize) -> Vec<String>;

    /// TUI のフッターのメトリクス（`TerminalSession::agent_metrics` と同じ規則。#357 / #1301）
    fn agent_metrics(&self) -> Option<AgentMetrics> {
        let lines = if c2_legacy() {
            self.visible_lines()
        } else {
            self.tail_lines(AGENT_TUI_TAIL_LINES)
        };
        agent_metrics_from_lines(&lines)
    }

    /// キュー滞留のヒント（#572）が入力欄に出ているか。`legacy_c3` は `TAKO_1001_C3_LEGACY`
    fn queued_hint_pending(&self, legacy_c3: bool) -> bool {
        let hint_lines = if legacy_c3 {
            self.visible_lines()
        } else {
            self.tail_lines(AGENT_TUI_TAIL_LINES)
        };
        tako_control::claude_tui::queued_messages_pending(&hint_lines)
    }

    /// 利用制限の段（#813）の読み
    fn limit_read(&self) -> LimitRead {
        let lines = self.visible_lines();
        LimitRead {
            fingerprint: limit_stop::screen_fingerprint(&lines),
            classified: limit_stop::classify_limit_stop(&lines),
            lines,
        }
    }

    /// ネット復帰の段（#2112）の読み（画面だけで決まる部分。画面の外の答えの覚えは UI 側）
    fn net_read(&self) -> NetRead {
        TakoApp::read_net_screen(self)
    }
}

/// 修正前の形: その場で端末を読む（ロック 1 回ごとに注入の待ち）
impl TickSource for TerminalSession {
    fn is_alt_screen(&self) -> bool {
        inject_read_delay();
        TerminalSession::is_alt_screen(self)
    }
    fn tail_lines(&self, n: usize) -> Vec<String> {
        inject_read_delay();
        TerminalSession::tail_lines(self, n)
    }
    fn visible_lines(&self) -> Vec<String> {
        inject_read_delay();
        TerminalSession::visible_lines(self)
    }
    fn analyze_input(&self) -> Option<InputStatus> {
        inject_read_delay();
        TerminalSession::analyze_input(self)
    }
    fn lines_with_history(&self, history: usize) -> Vec<String> {
        inject_read_delay();
        TerminalSession::lines_with_history(self, history)
    }
}

/// 背景で採った 1 ペインぶん（[`read`] が作る）。UI スレッドはここから読むだけで、端末のロックを取らない。
///
/// 解析を済ませていないもの（計画の後でキュー滞留・利用制限・ネット復帰の対象になったペイン）は、
/// 採った行から同じ既定の実装で解く（UI スレッドの解析だけ・ロックは取らない）。採っていないもの
/// （入力欄・画面の外の履歴は、止まっている画面のときだけ採る）を各段が求めたら、持っている手
/// （[`PaneAccess`]）でその場で読む = 修正前と同じ答え（採る条件は各段が求める条件を覆うので、
/// 通常は起きない）
pub(crate) struct PaneTick {
    access: PaneAccess,
    alt: bool,
    /// 可視行のすべて（`alt_and_lines` の 1 回のロック）
    lines: Vec<String>,
    metrics: Option<AgentMetrics>,
    queued_pending: Option<bool>,
    limit: RefCell<Option<LimitRead>>,
    net: Option<NetRead>,
    input: Option<Option<InputStatus>>,
    history: RefCell<Option<Vec<String>>>,
}

impl TickSource for PaneTick {
    fn is_alt_screen(&self) -> bool {
        self.alt
    }
    fn tail_lines(&self, n: usize) -> Vec<String> {
        let from = self.lines.len().saturating_sub(n);
        self.lines[from..].to_vec()
    }
    fn visible_lines(&self) -> Vec<String> {
        self.lines.clone()
    }
    fn analyze_input(&self) -> Option<InputStatus> {
        match &self.input {
            Some(input) => input.clone(),
            None => {
                inject_read_delay();
                self.access.analyze_input()
            }
        }
    }
    fn lines_with_history(&self, history: usize) -> Vec<String> {
        match self.history.borrow_mut().take() {
            Some(lines) => lines,
            None => {
                inject_read_delay();
                self.access.lines_with_history(history)
            }
        }
    }
    fn agent_metrics(&self) -> Option<AgentMetrics> {
        self.metrics.clone()
    }
    fn queued_hint_pending(&self, legacy_c3: bool) -> bool {
        self.queued_pending
            .unwrap_or_else(|| RawLines(self).queued_hint_pending(legacy_c3))
    }
    fn limit_read(&self) -> LimitRead {
        let taken = self.limit.borrow_mut().take();
        taken.unwrap_or_else(|| RawLines(self).limit_read())
    }
    fn net_read(&self) -> NetRead {
        self.net
            .clone()
            .unwrap_or_else(|| RawLines(self).net_read())
    }
}

/// 1 ペインぶんの読み口（[`TickScreens::source`] の答え）
pub(crate) enum TickSrc<'a> {
    Live(&'a TerminalSession),
    Captured(&'a PaneTick),
}

impl TickSource for TickSrc<'_> {
    fn is_alt_screen(&self) -> bool {
        match self {
            Self::Live(s) => TickSource::is_alt_screen(*s),
            Self::Captured(t) => t.is_alt_screen(),
        }
    }
    fn tail_lines(&self, n: usize) -> Vec<String> {
        match self {
            Self::Live(s) => TickSource::tail_lines(*s, n),
            Self::Captured(t) => t.tail_lines(n),
        }
    }
    fn visible_lines(&self) -> Vec<String> {
        match self {
            Self::Live(s) => TickSource::visible_lines(*s),
            Self::Captured(t) => t.visible_lines(),
        }
    }
    fn analyze_input(&self) -> Option<InputStatus> {
        match self {
            Self::Live(s) => TickSource::analyze_input(*s),
            Self::Captured(t) => t.analyze_input(),
        }
    }
    fn lines_with_history(&self, history: usize) -> Vec<String> {
        match self {
            Self::Live(s) => TickSource::lines_with_history(*s, history),
            Self::Captured(t) => t.lines_with_history(history),
        }
    }
    fn agent_metrics(&self) -> Option<AgentMetrics> {
        match self {
            Self::Live(s) => TickSource::agent_metrics(*s),
            Self::Captured(t) => t.agent_metrics(),
        }
    }
    fn queued_hint_pending(&self, legacy_c3: bool) -> bool {
        match self {
            Self::Live(s) => TickSource::queued_hint_pending(*s, legacy_c3),
            Self::Captured(t) => t.queued_hint_pending(legacy_c3),
        }
    }
    fn limit_read(&self) -> LimitRead {
        match self {
            Self::Live(s) => TickSource::limit_read(*s),
            Self::Captured(t) => t.limit_read(),
        }
    }
    fn net_read(&self) -> NetRead {
        match self {
            Self::Live(s) => TickSource::net_read(*s),
            Self::Captured(t) => t.net_read(),
        }
    }
}

/// 1 tick ぶんの画面（各段へ渡す）
pub(crate) enum TickScreens {
    /// その場で `TerminalSession` を読む（A/B の修正前・定期更新の外から各段を呼ぶ経路）
    Live,
    /// 背景で採った 1 枚
    Captured {
        panes: HashMap<PaneId, PaneTick>,
        /// 背景で読み直した引き継ぎの閾値の設定（プロファイル → (閾値, 自動の引き継ぎ)）
        policies: Vec<(String, (u32, bool))>,
    },
}

impl TickScreens {
    /// `pane` の読み口。端末の無いペインは `None`（修正前の `terminals.get(&pane)` と同じ）。
    /// 背景で採っていないペイン（採った後に開いた）はその場で読む
    pub(crate) fn source<'a>(
        &'a self,
        terminals: &'a HashMap<PaneId, TerminalSession>,
        pane: PaneId,
    ) -> Option<TickSrc<'a>> {
        if let Self::Captured { panes, .. } = self {
            if let Some(tick) = panes.get(&pane) {
                // 採った後に閉じたペインは読まない（修正前は閉じたペインの端末が無い）
                if terminals.contains_key(&pane) {
                    return Some(TickSrc::Captured(tick));
                }
                return None;
            }
        }
        terminals.get(&pane).map(TickSrc::Live)
    }

    /// 背景で読み直した引き継ぎの閾値の設定を取り出す（1 tick に 1 回）
    pub(crate) fn take_policies(&mut self) -> Vec<(String, (u32, bool))> {
        match self {
            Self::Live => Vec::new(),
            Self::Captured { policies, .. } => std::mem::take(policies),
        }
    }
}

/// 背景へ渡す 1 ペインぶんの手と、何を読むか
struct PlanItem {
    pane: PaneId,
    access: PaneAccess,
    queued: bool,
    limit: bool,
    net: bool,
    /// 器のペインか（画面の外の履歴は器を持たない直接のペインだけ端末から読む = #2181）
    has_backend: bool,
}

/// master の引き継ぎの閾値の設定を読み直す候補（`handoff_policy_for` の TTL が切れかけのもの）
struct PolicyWant {
    pane: PaneId,
    profile: String,
    /// mod の報告と transcript が言う ctx%（UI のメモリから写す）。画面の ctx% は背景で解く
    mod_pct: Option<u32>,
    transcript_pct: Option<u32>,
}

/// 背景へ渡すもの（[`plan`] が作り、[`read`] が読む）
pub(crate) struct TickPlan {
    items: Vec<PlanItem>,
    policies: Vec<PolicyWant>,
    legacy_c3: bool,
}

/// 設定の読み直しを前倒しする幅。UI スレッドの段が見る時刻（背景で読んだ後）でも TTL 内に収める
const POLICY_PREFETCH_MARGIN: std::time::Duration = std::time::Duration::from_secs(5);

/// UI スレッド: 背景へ渡す手を集める（**メモリの操作だけ**。端末のロックもファイルも触らない）。
/// A/B（[`legacy`]）なら `None` = 背景で読まない
pub(crate) fn plan(app: &TakoApp) -> Option<TickPlan> {
    if legacy() {
        return None;
    }
    let mut want: HashMap<PaneId, (bool, bool, bool)> = HashMap::new();
    // メトリクス（#357）・キュー滞留（#572）・引き継ぎ（#749）が見るのは表のタブのペイン
    for tab in app.workspace.tabs() {
        for pane in tab.tree().panes() {
            want.entry(pane.id()).or_default().0 = true;
        }
    }
    for pane in app.limit_resume_targets() {
        want.entry(pane).or_default().1 = true;
    }
    for (pane, _) in app.net_resume_targets() {
        want.entry(pane).or_default().2 = true;
    }
    let items = want
        .into_iter()
        .filter_map(|(pane, (queued, limit, net))| {
            let session = app.terminals.get(&pane)?;
            Some(PlanItem {
                pane,
                access: session.access(),
                queued,
                limit,
                net,
                has_backend: app.backend_sessions.contains_key(&pane),
            })
        })
        .collect();
    Some(TickPlan {
        items,
        policies: app.handoff_policy_wants(POLICY_PREFETCH_MARGIN),
        legacy_c3: crate::queued_recovery_legacy(),
    })
}

/// 背景: 1 ペインにつき 1 回のロックで画面を採り、各段の解析を済ませる。
/// 引き継ぎの閾値の設定（profiles/*.yaml）もここで読み直す
pub(crate) fn read(plan: TickPlan) -> TickScreens {
    let _span = tako_control::diag::perf_span("periodic_tick_screens");
    let mut panes = HashMap::with_capacity(plan.items.len());
    for item in plan.items {
        inject_read_delay();
        let (alt, lines) = item.access.alt_and_lines();
        let mut tick = PaneTick {
            access: item.access,
            alt,
            lines,
            metrics: None,
            queued_pending: None,
            limit: RefCell::new(None),
            net: None,
            input: None,
            history: RefCell::new(None),
        };
        // 解析は trait の既定の 1 実装（修正前の形と同じ規則）。`tick` 自身はまだ答えを持たないので、
        // 行だけを持つ読み口として既定の実装を通す
        let raw = RawLines(&tick);
        let metrics = raw.agent_metrics();
        let queued_pending = item.queued.then(|| raw.queued_hint_pending(plan.legacy_c3));
        let limit = item.limit.then(|| raw.limit_read());
        let net = item.net.then(|| raw.net_read());
        // 入力欄（人の下書きの判定）は止まっている画面だけが求める: 利用制限の idle 型（#813）・
        // 接続エラーの停止と画面の外の停止（#2112 / #2181）。それ以外では装飾つきの画面を組まない
        let need_input = limit
            .as_ref()
            .is_some_and(|l| matches!(l.classified, Some((LimitStopKind::Idle, _))))
            || net.as_ref().is_some_and(|n| {
                matches!(
                    n,
                    NetRead::Screen(NetScreen::Stopped(_)) | NetRead::Offscreen(_)
                )
            });
        let input = need_input.then(|| {
            inject_read_delay();
            tick.access.analyze_input()
        });
        // 画面の外の履歴（#2181）は器を持たない直接のペインだけ端末から読む（器のペインは器の履歴を
        // net_autoresume が背景のスレッドで読む）
        let history =
            (!item.has_backend && matches!(net, Some(NetRead::Offscreen(_)))).then(|| {
                inject_read_delay();
                tick.access.lines_with_history(net_stop::OFFSCREEN_LINES)
            });
        tick.metrics = metrics;
        tick.queued_pending = queued_pending;
        tick.limit = RefCell::new(limit);
        tick.net = net;
        tick.input = input;
        tick.history = RefCell::new(history);
        panes.insert(item.pane, tick);
    }
    // 引き継ぎの閾値の設定（#749）。修正前は UI スレッドの段が「ctx% が下限以上」のときだけ読んでいた。
    // ここでは mod / 画面 / transcript のどれかが下限以上なら読む（段が解く ctx% はこの 3 つのどれか =
    // 段が求めるものを覆う）。同じプロファイルは 1 回だけ
    let mut policies: Vec<(String, (u32, bool))> = Vec::new();
    for want in plan.policies {
        if policies.iter().any(|(p, _)| *p == want.profile) {
            continue;
        }
        let screen_pct = panes
            .get(&want.pane)
            .and_then(|t| t.metrics.as_ref())
            .and_then(|m| m.ctx_percent);
        let reaches = [want.mod_pct, screen_pct, want.transcript_pct]
            .into_iter()
            .flatten()
            .any(|pct| pct >= tako_core::handoff::CTX_THRESHOLD_MIN);
        if !reaches {
            continue;
        }
        let (resolved, auto) = tako_control::orchestrator::handoff_policy_for(&want.profile);
        policies.push((want.profile, (resolved.value, auto)));
    }
    TickScreens::Captured { panes, policies }
}

/// 採った行だけを読む口（[`PaneTick`] を trait の既定の実装 = 解析の 1 実装へ通す。背景の解析と、
/// UI スレッドで解析を済ませていない対象の解き直しが使う）
struct RawLines<'a>(&'a PaneTick);

impl TickSource for RawLines<'_> {
    fn is_alt_screen(&self) -> bool {
        self.0.alt
    }
    fn tail_lines(&self, n: usize) -> Vec<String> {
        self.0.tail_lines(n)
    }
    fn visible_lines(&self) -> Vec<String> {
        self.0.lines.clone()
    }
    fn analyze_input(&self) -> Option<InputStatus> {
        inject_read_delay();
        self.0.access.analyze_input()
    }
    fn lines_with_history(&self, history: usize) -> Vec<String> {
        inject_read_delay();
        self.0.access.lines_with_history(history)
    }
}

impl TakoApp {
    /// 引き継ぎの閾値の設定を背景で読み直す候補（master ごと。TTL が `margin` 以内に切れるもの）
    fn handoff_policy_wants(&self, margin: std::time::Duration) -> Vec<PolicyWant> {
        let now = std::time::Instant::now();
        let mut out = Vec::new();
        for tab in self.workspace.tabs() {
            for pane in tab.tree().panes() {
                let Some(role) = pane.role() else { continue };
                let Some(profile) = tako_core::handoff::master_profile_of_role(role) else {
                    continue;
                };
                if !self.handoff_policy_expiring(profile, now, margin) {
                    continue;
                }
                let mod_pct =
                    match tako_core::claude_mod::ctx_input(&tako_core::claude_mod::lookup(
                        Some(&self.claude_mod),
                        pane.id().as_u64(),
                        now,
                        tako_core::claude_mod::s2_legacy(),
                    )) {
                        tako_core::ctx_usage::ModCtxInput::Fresh(c) => c.percent,
                        tako_core::ctx_usage::ModCtxInput::Unavailable(_) => None,
                    };
                out.push(PolicyWant {
                    pane: pane.id(),
                    profile: profile.to_string(),
                    mod_pct,
                    transcript_pct: self.transcript_ctx.get(&pane.id()).copied(),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 背景の読み口は、修正前の `TerminalSession` の読みと同じ答えを返す
    /// （行は 1 バイトも変わらない・メトリクスと利用制限の判定は同じ関数）
    #[test]
    fn 背景で採った1枚は修正前の読みと同じ答えを返す() {
        use std::time::{Duration, Instant};
        // claude の画面の形（alternate screen・太字・全角・利用制限の告知・入力欄・フッター）
        let script = concat!(
            "printf '\\033[?1049h\\033[1m❯ 作業を続けて\\033[0m\\n\\n",
            "⏺ You'\\''ve hit your session limit · resets 3am (Asia/Tokyo)\\n\\n",
            "────────\\n❯ \\n────────\\n",
            "  [Opus 5 (1M context) · 1H] ctx 55%% · 5h 40%% · 7d 20%%'; sleep 30"
        );
        let (session, _rx) = TerminalSession::spawn(
            60,
            12,
            tako_core::SpawnOptions {
                command: Some(tako_core::SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script.to_string()],
                }),
                ..tako_core::SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !session
            .visible_lines()
            .iter()
            .any(|l| l.contains("ctx 55%"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            session
                .visible_lines()
                .iter()
                .any(|l| l.contains("ctx 55%")),
            "画面が整わない: {:?}",
            session.visible_lines()
        );
        let live: &TerminalSession = &session;
        // 計画で解析を頼んだ形（queued / limit / net）と、頼んでいない形（計画の後で対象になった
        // ペイン = 採った行から同じ実装で解き直す）の両方が修正前の読みと同じ答えを返す
        for wanted in [true, false] {
            let plan = TickPlan {
                items: vec![PlanItem {
                    pane: PaneId::from_raw(7),
                    access: session.access(),
                    queued: wanted,
                    limit: wanted,
                    net: wanted,
                    has_backend: false,
                }],
                policies: Vec::new(),
                legacy_c3: false,
            };
            let TickScreens::Captured { panes, .. } = read(plan) else {
                panic!("背景の読みは Captured を返す");
            };
            let tick = panes.get(&PaneId::from_raw(7)).expect("採ったペイン");
            assert_eq!(tick.is_alt_screen(), TickSource::is_alt_screen(live));
            assert_eq!(tick.visible_lines(), TickSource::visible_lines(live));
            assert_eq!(
                tick.tail_lines(AGENT_TUI_TAIL_LINES),
                TickSource::tail_lines(live, AGENT_TUI_TAIL_LINES)
            );
            assert_eq!(
                format!("{:?}", tick.agent_metrics()),
                format!("{:?}", TickSource::agent_metrics(live))
            );
            assert_eq!(tick.agent_metrics().and_then(|m| m.ctx_percent), Some(55));
            assert_eq!(
                tick.queued_hint_pending(false),
                TickSource::queued_hint_pending(live, false)
            );
            let (a, b) = (tick.limit_read(), TickSource::limit_read(live));
            assert_eq!(a.lines, b.lines);
            assert_eq!(a.fingerprint, b.fingerprint);
            assert_eq!(a.classified, b.classified);
            assert!(matches!(a.classified, Some((LimitStopKind::Idle, _))));
            assert_eq!(
                format!("{:?}", tick.net_read()),
                format!("{:?}", TickSource::net_read(live))
            );
            // 利用制限の idle 型で止まっている = 頼んだ形では入力欄を採っている（UI スレッドでロックを
            // 取らない）。頼んでいない形は持っている手でその場で読む（同じ答え）
            assert_eq!(tick.input.is_some(), wanted);
            assert_eq!(
                format!("{:?}", tick.analyze_input()),
                format!("{:?}", TickSource::analyze_input(live))
            );
        }
    }
}

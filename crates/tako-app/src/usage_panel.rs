//! 右パネルの `usage` ビュー —— 利用制限の使い道（Issue #2203。エピック #2192 の S4）
//!
//! ## 正本はここに無い
//!
//! 数値の正本は `tako usage` / MCP `tako_sessions` の `action: "usage"` と同じ
//! `tako_control::usage::usage_payload`。この画面は `Request::Sessions { action: "usage" }` を
//! `dispatch_off_ui` で投げ（`prepare_offload` → background のジョブ `OffloadJob::UsageSummary`）、
//! 届いた JSON を描くだけ。**描画・UI スレッドで索引のファイルを読まない**（#1984 / #2042）。
//! 数字を文字にするのも CLI と同じ `tako_core::usage::display` の 1 実装。
//!
//! ## 掘り下げ
//!
//! 段は「プロジェクト → Issue → 会話」（行を押すと `tako_core::usage::report::drill` が次の段と
//! 絞り込みを返す）。どの段も `tako usage --by <段> --project <値> --issue <値> --show` で
//! CLI / MCP から同じ所を開ける（設計原則 5）。いま開いている段は `tako panel` の応答の `usage`。
//!
//! 印（タブ・行の矢印・更新）は既存の SVG のパスで描く。絵文字は使わない。

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, relative, svg, Context, SharedString};
use serde_json::{json, Value};
use tako_core::usage::display;
use tako_core::usage::report::{self, By, GroupKey, IssueFilter, ProjectFilter};
use tako_core::PaneOrigin;

pub(crate) use tako_control::usage::UsageQuery;

use super::*;

/// 1 段に描く行の数（既定。「さらに表示」で [`USAGE_ROWS_STEP`] ずつ増える。全件は `tako usage --limit`）
pub(crate) const USAGE_ROWS: usize = 20;
/// 「さらに表示」1 回で増やす行の数
pub(crate) const USAGE_ROWS_STEP: usize = 30;
/// 見ている間の読み直しの間隔（索引の背景の更新 = `GUI_INTERVAL` と同じ 60 秒）
const USAGE_REFRESH: Duration = Duration::from_secs(60);
/// 索引が読み途中（初回の取り込み・ほかの更新が鍵を持っていた）のときの読み直しの間隔
const USAGE_CATCH_UP: Duration = Duration::from_secs(5);
/// 期間の切り替えの並び（`tako usage --window` の値）
pub(crate) const USAGE_WINDOWS: [&str; 6] = ["7d", "5h", "day", "week", "month", "all"];

/// 掘り下げの 1 段（`tako usage --by --project --issue` と 1:1）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageLevel {
    pub(crate) by: String,
    pub(crate) project: Option<String>,
    pub(crate) issue: Option<String>,
}

impl UsageLevel {
    /// 最上段（プロジェクトの一覧）
    pub(crate) fn top() -> Self {
        Self {
            by: By::Project.as_str().to_string(),
            project: None,
            issue: None,
        }
    }
}

/// 届いた答え
pub(crate) struct UsageResult {
    /// どの問い合わせの答えか（段・期間を切り替えた直後に古い答えを新しい段の見出しで描かない）
    pub(crate) query: UsageQuery,
    pub(crate) value: Result<Value, String>,
    /// 届いた時刻（unix 秒。解除時刻の表記の「今日」の基準）
    pub(crate) at_unix: i64,
}

/// usage ビューの状態（画面の一時状態。永続しない）
#[derive(Default)]
pub(crate) struct UsagePanel {
    /// 掘り下げの段（先頭が最上段・末尾がいま見ている段）。空 = まだ開いていない（最上段として扱う）
    pub(crate) levels: Vec<UsageLevel>,
    /// 期間（`None` = 7d）
    pub(crate) window: Option<String>,
    /// tako のアカウントの絞り込み（`--show --account` で来たときだけ）
    pub(crate) account: Option<String>,
    /// 「さらに表示」で増やした行の数
    pub(crate) extra_rows: usize,
    pub(crate) result: Option<UsageResult>,
    /// 次の機会に読み直す（`tako panel --view usage` / `--show` / 段の切り替え）
    pub(crate) pending: bool,
    pub(crate) requested: u64,
    pub(crate) applied: u64,
    pub(crate) inflight: usize,
    pub(crate) last_fetch: Option<Instant>,
    pub(crate) notes_open: bool,
    /// いま描いている行（visual-test が CLI の数値と照合する。描画のたびに描いたものへ差し替わる）
    pub(crate) rendered: Vec<UsageLoginView>,
}

impl UsagePanel {
    /// 段（空なら最上段）
    pub(crate) fn levels(&self) -> Vec<UsageLevel> {
        if self.levels.is_empty() {
            vec![UsageLevel::top()]
        } else {
            self.levels.clone()
        }
    }

    pub(crate) fn current(&self) -> UsageLevel {
        self.levels().pop().unwrap_or_else(UsageLevel::top)
    }

    pub(crate) fn window(&self) -> &str {
        self.window.as_deref().unwrap_or("7d")
    }

    /// いまの段の問い合わせ（`tako usage` と同じ引数）
    pub(crate) fn query(&self) -> UsageQuery {
        let level = self.current();
        UsageQuery {
            by: Some(level.by),
            window: Some(self.window().to_string()),
            account: self.account.clone(),
            project: level.project,
            issue: level.issue,
            limit: Some(USAGE_ROWS + self.extra_rows),
        }
    }

    /// CLI / MCP の `show` が渡した問い合わせから段を組む（最上段 + その段。最上段そのものなら 1 段）
    pub(crate) fn show(&mut self, query: &UsageQuery) {
        let by = query
            .by
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(By::parse)
            .unwrap_or(By::Project);
        let set = |v: &Option<String>| v.clone().filter(|s| !s.trim().is_empty());
        let level = UsageLevel {
            by: by.as_str().to_string(),
            project: set(&query.project),
            issue: set(&query.issue),
        };
        self.levels = if level == UsageLevel::top() {
            vec![level]
        } else {
            vec![UsageLevel::top(), level]
        };
        self.window = set(&query.window).map(|w| w.trim().to_ascii_lowercase());
        self.account = set(&query.account);
        self.extra_rows = query.limit.map_or(0, |l| l.saturating_sub(USAGE_ROWS));
        self.pending = true;
    }

    /// 行を押して 1 段掘る（掘れない段なら何もしない = `false`）
    pub(crate) fn drill(&mut self, key: &GroupKey) -> bool {
        let Some(by) = By::parse(&self.current().by) else {
            return false;
        };
        let Some(next) = report::drill(by, key) else {
            return false;
        };
        let mut levels = self.levels();
        levels.push(UsageLevel {
            by: next.by.as_str().to_string(),
            project: next.project,
            issue: next.issue,
        });
        self.levels = levels;
        self.extra_rows = 0;
        self.pending = true;
        true
    }

    /// パンくずの i 段目へ戻る
    pub(crate) fn back_to(&mut self, index: usize) {
        let mut levels = self.levels();
        levels.truncate(index + 1);
        self.levels = levels;
        self.extra_rows = 0;
        self.pending = true;
    }

    pub(crate) fn set_window(&mut self, window: &str) {
        self.window = (window != "7d").then(|| window.to_string());
        self.pending = true;
    }

    /// `tako panel` の応答の `usage`（開いている段・期間・絞り込み）
    pub(crate) fn state_json(&self) -> Value {
        let q = self.query();
        json!({
            "by": q.by,
            "window": q.window,
            "account": q.account,
            "project": q.project,
            "issue": q.issue,
            "limit": q.limit,
            "depth": self.levels().len(),
            "loading": self.inflight > 0,
        })
    }

    /// 次に読み直すまでの間（索引が読み途中なら短く）
    fn refresh_interval(&self) -> Duration {
        let catching_up = self.result.as_ref().is_some_and(|r| {
            r.value.as_ref().is_ok_and(|v| {
                let u = &v["index"]["update"];
                u["caught_up"] == Value::Bool(false) || u["stopped"] == "locked"
            })
        });
        if catching_up {
            USAGE_CATCH_UP
        } else {
            USAGE_REFRESH
        }
    }
}

/// 1 ログインぶんの描く中身（文字はすべて CLI と同じ書式で作ってある）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageLoginView {
    pub(crate) title: String,
    /// 観測した最新の %（`("7d", "83%")`。主の枠が先）
    pub(crate) limits: Vec<(String, String)>,
    pub(crate) resets: Option<String>,
    pub(crate) range: Option<String>,
    pub(crate) total: Option<String>,
    pub(crate) rows: Vec<UsageRowView>,
    pub(crate) more: u64,
    pub(crate) outside: Option<String>,
    pub(crate) estimated: Option<String>,
    pub(crate) unavailable: Vec<String>,
}

/// 段の 1 行
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageRowView {
    pub(crate) name: String,
    pub(crate) key: GroupKey,
    /// 主の枠の %（`30.9%`。観測の無い枠なら `None`）
    pub(crate) main: Option<String>,
    pub(crate) main_value: f64,
    pub(crate) estimated: Option<String>,
    /// もう一方の枠（`5h 1.2%`）
    pub(crate) secondary: Option<String>,
    pub(crate) usd: String,
    pub(crate) tokens: String,
    pub(crate) sessions: u64,
    pub(crate) drillable: bool,
}

/// 絞り込みの値（`project:X` など）の表示名
fn project_title(spec: &str) -> String {
    match ProjectFilter::parse(spec) {
        Some(ProjectFilter::Named(p) | ProjectFilter::Project(p)) => p.to_string(),
        Some(ProjectFilter::Profile(p)) => crate::ui_text::panel::usage_profile(p),
        Some(ProjectFilter::Unclassified) => crate::ui_text::panel::usage_unclassified().into(),
        None => spec.to_string(),
    }
}

fn issue_title(spec: &str) -> String {
    match IssueFilter::parse(spec) {
        Some(IssueFilter::Number(n)) => format!("#{n}"),
        Some(IssueFilter::Missing) => crate::ui_text::panel::usage_no_issue().into(),
        None => spec.to_string(),
    }
}

/// パンくずの 1 段の名前
pub(crate) fn level_title(level: &UsageLevel) -> String {
    if let Some(issue) = &level.issue {
        return issue_title(issue);
    }
    if let Some(project) = &level.project {
        return project_title(project);
    }
    if level.by == By::Project.as_str() {
        crate::ui_text::panel::usage_root().to_string()
    } else {
        level.by.clone()
    }
}

/// 段の行の名前（いまの段の絞り込みで分かる部分は省く = パンくずに出ている）
fn row_name(level: &UsageLevel, key: &GroupKey, fallback: &str) -> String {
    let project = || project_title(&ProjectFilter::spec_of(key));
    let issue = || issue_title(&IssueFilter::spec_of(key));
    match By::parse(&level.by) {
        Some(By::Project) => project(),
        Some(By::Issue) if level.project.is_some() => issue(),
        Some(By::Issue) => format!("{} {}", project(), issue()),
        Some(By::Session) => {
            let sid: String = key
                .session
                .as_deref()
                .unwrap_or("-")
                .chars()
                .take(8)
                .collect();
            let role = key
                .role
                .clone()
                .unwrap_or_else(|| crate::ui_text::panel::usage_manual().into());
            let mut parts = vec![sid, role];
            if level.project.is_none() {
                parts.push(project());
            }
            if level.issue.is_none() {
                parts.push(issue());
            }
            if let Some(label) = &key.label {
                parts.push(label.clone());
            }
            parts.join(" · ")
        }
        _ => fallback.to_string(),
    }
}

/// 応答から描く中身を組む（純関数。GPUI に依存しない = 単体テストで CLI と同じ書式を確かめる）
pub(crate) fn login_views(
    payload: &Value,
    level: &UsageLevel,
    now: i64,
    utc_offset: i32,
) -> Vec<UsageLoginView> {
    let window = payload["window"].as_str().unwrap_or("7d");
    let main_kind = if window == "5h" {
        "five_hour"
    } else {
        "seven_day"
    };
    let drillable =
        By::parse(&level.by).is_some_and(|by| report::drill(by, &GroupKey::default()).is_some());
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    let mut out = Vec::new();
    for l in payload["logins"].as_array().into_iter().flatten() {
        let mut title = crate::ui_text::panel::usage_login(l["label"].as_str().unwrap_or("-"));
        if l["agent"] == "codex" {
            title.push_str(" · codex");
        }
        // 観測した最新の %（S1 の要約。主の枠が先）
        let mut windows: Vec<&Value> = l["windows"].as_array().into_iter().flatten().collect();
        windows.sort_by_key(|w| w["kind"].as_str() != Some(main_kind));
        let limits = windows
            .iter()
            .filter(|w| w["expired"] != Value::Bool(true))
            .filter_map(|w| {
                let kind = display::kind_label(w["kind"].as_str()?);
                Some((
                    kind.to_string(),
                    display::observed_percent(w["used_percent"].as_f64()?),
                ))
            })
            .collect();
        let resets = windows
            .iter()
            .find(|w| w["kind"] == main_kind && w["expired"] != Value::Bool(true))
            .and_then(|w| w["resets_at"].as_i64())
            .map(|r| {
                crate::ui_text::panel::usage_resets(&tako_core::user_task::response_stamp(
                    r, now, utc_offset,
                ))
            });
        let usage = &l["usage"];
        let mut view = UsageLoginView {
            title,
            limits,
            resets,
            range: None,
            total: None,
            rows: Vec::new(),
            more: 0,
            outside: None,
            estimated: None,
            unavailable: Vec::new(),
        };
        if usage.is_object() {
            view.range = Some(display::range_label(
                window,
                &usage["range"],
                now,
                utc_offset,
            ));
            let t = &usage["total"];
            view.total = Some(crate::ui_text::panel::usage_amount(
                &display::api_usd(f(&t["api_usd"])),
                &display::tokens(t["total_tokens"].as_u64().unwrap_or(0)),
                t["sessions"].as_u64().unwrap_or(0),
            ));
            let kinds: Vec<&Value> = usage["limit_share"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            let available = |kind: &str| {
                kinds
                    .iter()
                    .any(|k| k["kind"] == kind && k["available"] == Value::Bool(true))
            };
            let other_kind = if main_kind == "five_hour" {
                "seven_day"
            } else {
                "five_hour"
            };
            for g in usage["groups"].as_array().into_iter().flatten() {
                let key: GroupKey = serde_json::from_value(g["key"].clone()).unwrap_or_default();
                let share = |kind: &str| &g["limit_share"][kind];
                let main_value = f(&share(main_kind)["percent"]);
                view.rows.push(UsageRowView {
                    name: row_name(level, &key, g["name"].as_str().unwrap_or("-")),
                    main: available(main_kind).then(|| display::share_percent(main_value)),
                    main_value,
                    estimated: available(main_kind)
                        .then(|| display::estimated(f(&share(main_kind)["estimated"])))
                        .flatten(),
                    secondary: available(other_kind).then(|| {
                        format!(
                            "{} {}",
                            display::kind_label(other_kind),
                            display::share_percent(f(&share(other_kind)["percent"]))
                        )
                    }),
                    usd: display::api_usd(f(&g["api_usd"])),
                    tokens: display::tokens(g["total_tokens"].as_u64().unwrap_or(0)),
                    sessions: g["sessions"].as_u64().unwrap_or(0),
                    key,
                    drillable,
                });
            }
            view.more = usage["more_groups"].as_u64().unwrap_or(0);
            if let Some(k) = kinds.iter().find(|k| k["kind"] == main_kind) {
                if k["available"] == Value::Bool(true) {
                    view.outside = Some(display::share_percent(f(&k["outside"])));
                    view.estimated = display::estimated(f(&k["estimated"]));
                }
            }
            for k in &kinds {
                if k["available"] != Value::Bool(true) {
                    let kind = display::kind_label(k["kind"].as_str().unwrap_or(""));
                    view.unavailable
                        .push(crate::ui_text::panel::usage_unavailable(kind));
                }
            }
        }
        out.push(view);
    }
    out
}

impl TakoApp {
    /// usage ビューのタブを出すか（開いている間か、このセッションで一度でも開いたとき）
    pub(crate) fn usage_tab_visible(&self) -> bool {
        self.panel_view == PanelView::Usage
            || self.usage.result.is_some()
            || !self.usage.levels.is_empty()
    }

    /// usage ビューを開く（ステータスバーの 5h / 7d・パレット・タブ）。`window` を渡すと最上段から
    pub(crate) fn open_usage_view(&mut self, window: Option<&str>, cx: &mut Context<Self>) {
        if let Some(window) = window {
            self.usage.levels = vec![UsageLevel::top()];
            self.usage.extra_rows = 0;
            self.usage.set_window(window);
        }
        if self.usage.result.is_none() {
            self.usage.pending = true;
        }
        self.panel_visible = true;
        self.panel_view = PanelView::Usage;
        self.drain_usage_pending(cx);
        cx.notify();
    }

    /// 頼まれた読み直しを出す（パネルが usage を出しているときだけ。閉じていれば開いたときに出す）
    pub(crate) fn drain_usage_pending(&mut self, cx: &mut Context<Self>) {
        if self.usage.pending && self.panel_visible && self.panel_view == PanelView::Usage {
            self.spawn_usage_fetch(cx);
        }
    }

    /// 2 秒の定期処理から呼ぶ（**新しいタイマーを作らない**。見ていなければ何もしない）
    pub(crate) fn tick_usage_panel(&mut self, cx: &mut Context<Self>) {
        if !self.panel_visible || self.panel_view != PanelView::Usage || self.usage.inflight > 0 {
            return;
        }
        let due = self
            .usage
            .last_fetch
            .is_none_or(|t| t.elapsed() >= self.usage.refresh_interval());
        if self.usage.pending || due {
            self.spawn_usage_fetch(cx);
        }
    }

    /// いまの段を読む。CLI `tako usage` / MCP と同じ要求を `dispatch_off_ui` で投げる
    /// （集計は background のジョブ。UI スレッドは要求を組んで答えを置くだけ）
    pub(crate) fn spawn_usage_fetch(&mut self, cx: &mut Context<Self>) {
        let query = self.usage.query();
        let request = tako_control::protocol::Request::Sessions {
            action: "usage".into(),
            id: None,
            role: None,
            project: query.project.clone(),
            limit: query.limit,
            pane: None,
            tab: None,
            direction: None,
            by: query.by.clone(),
            window: query.window.clone(),
            account: query.account.clone(),
            issue: query.issue.clone(),
            show: None,
        };
        self.usage.pending = false;
        self.usage.requested += 1;
        self.usage.inflight += 1;
        self.usage.last_fetch = Some(Instant::now());
        let seq = self.usage.requested;
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            let usage = &mut app.usage;
            usage.inflight = usage.inflight.saturating_sub(1);
            // 後から頼んだ読み直しが先に着いていたら、古い答えで上書きしない
            if seq <= usage.applied {
                return;
            }
            usage.applied = seq;
            usage.result = Some(UsageResult {
                query,
                value: result.map_err(|e| e.to_string()),
                at_unix: tako_core::user_task::unix_now(),
            });
            cx.notify();
        });
        cx.notify();
    }

    pub(crate) fn render_usage_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let levels = self.usage.levels();
        let level = self.usage.current();
        let window = self.usage.window().to_string();
        let loading = self.usage.inflight > 0;
        let probe_bounds = self.panel_click_probe_bounds.clone();
        let probe = move |key: String| {
            let probe = probe_bounds.clone();
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    probe.borrow_mut().insert(key.clone(), bounds);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };

        // --- 見出し（題・更新中・読み直し）---
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .px(px(10.0))
            .pt(px(8.0))
            .pb(px(6.0))
            .text_size(px(12.0))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(crate::ui_text::panel::usage_title()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .when(loading && self.usage.result.is_some(), |d| {
                        d.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_muted))
                                .child(crate::ui_text::panel::usage_updating()),
                        )
                    })
                    .child(
                        div()
                            .id("usage-refresh")
                            .relative()
                            .p(px(3.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .tooltip({
                                let tip_theme = theme.clone();
                                move |_, cx| {
                                    cx.new(|_| {
                                        crate::tab_bar::HintTooltip::new(
                                            crate::ui_text::panel::usage_refresh().to_string(),
                                            tip_theme.clone(),
                                        )
                                    })
                                    .into()
                                }
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.usage.pending = true;
                                this.drain_usage_pending(cx);
                            }))
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::REFRESH)
                                    .size(px(12.0))
                                    .text_color(hsla(theme.text_muted)),
                            )
                            .child(probe("usage-refresh".to_string())),
                    ),
            );

        // --- 期間の切り替え ---
        let windows = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(4.0))
            .px(px(10.0))
            .pb(px(6.0))
            .children(USAGE_WINDOWS.iter().map(|w| {
                let active = *w == window;
                let w = *w;
                div()
                    .id(SharedString::from(format!("usage-window-{w}")))
                    .relative()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .text_size(px(10.5))
                    .cursor_pointer()
                    .border_1()
                    .border_color(hsla(if active {
                        theme.accent
                    } else {
                        theme.border_subtle
                    }))
                    .when(active, |d| d.bg(rgba(theme.accent_muted)))
                    .text_color(hsla(if active {
                        theme.foreground
                    } else {
                        theme.text_muted
                    }))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage.set_window(w);
                        this.drain_usage_pending(cx);
                    }))
                    .child(crate::ui_text::panel::usage_window(w))
                    .child(probe(format!("usage-window-{w}")))
            }));

        // --- パンくず（押すとその段へ戻る）---
        let depth = levels.len();
        let mut crumbs = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(3.0))
            .px(px(10.0))
            .pb(px(7.0))
            .text_size(px(11.0));
        for (i, l) in levels.iter().enumerate() {
            let last = i + 1 == depth;
            if i > 0 {
                crumbs = crumbs.child(
                    svg()
                        .path(crate::file_icons::ui_icon::CHEVRON_RIGHT)
                        .size(px(9.0))
                        .text_color(hsla(theme.text_faint)),
                );
            }
            crumbs = crumbs.child(
                div()
                    .id(SharedString::from(format!("usage-crumb-{i}")))
                    .relative()
                    .px(px(3.0))
                    .rounded(px(3.0))
                    .text_color(hsla(if last { theme.foreground } else { theme.accent }))
                    .when(last, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .when(!last, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.usage.back_to(i);
                                this.drain_usage_pending(cx);
                            }))
                    })
                    .child(SharedString::from(level_title(l)))
                    .child(probe(format!("usage-crumb-{i}"))),
            );
        }

        let mut root = div()
            .id("panel-usage-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_b_1()
                    .border_color(hsla(theme.border_inner))
                    .child(header)
                    .child(windows)
                    .child(crumbs),
            );

        let note = |text: String| {
            div()
                .px(px(10.0))
                .py(px(10.0))
                .text_size(px(11.5))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(text))
        };
        // 段・期間を切り替えた直後は、古い答えを新しい見出しで描かない（集計中を出す）
        let current_query = self.usage.query();
        let shown = self.usage.result.as_ref().filter(|r| {
            r.query.by == current_query.by
                && r.query.window == current_query.window
                && r.query.project == current_query.project
                && r.query.issue == current_query.issue
                && r.query.account == current_query.account
        });
        let Some(result) = shown else {
            self.usage.rendered.clear();
            return root.child(note(crate::ui_text::panel::usage_loading().into()));
        };
        let payload = match &result.value {
            Ok(v) => v.clone(),
            Err(e) => {
                self.usage.rendered.clear();
                return root.child(note(crate::ui_text::panel::usage_failed(e)));
            }
        };
        let offset = tako_core::platform::clock::local_utc_offset();
        let views = login_views(&payload, &level, result.at_unix, offset);
        self.usage.rendered = views.clone();

        let mut body = div()
            .id("panel-usage-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pb(px(12.0))
            .text_size(px(11.5));
        if views.is_empty() {
            body = body.child(note(crate::ui_text::panel::usage_empty().into()));
        }
        if let Some(n) = payload["unreadable_lines"].as_u64() {
            body = body.child(note(crate::ui_text::panel::usage_unreadable(n)));
        }
        let mut row_index = 0usize;
        for (li, view) in views.iter().enumerate() {
            // ログインの見出し: 名前 + 最新の %（主の枠が先）+ 解除時刻
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(10.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(view.title.clone())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_none()
                                    .gap(px(8.0))
                                    .font_family(theme.font_family.clone())
                                    .text_size(px(11.0))
                                    .children(view.limits.iter().map(|(kind, pct)| {
                                        div()
                                            .flex()
                                            .flex_row()
                                            .gap(px(3.0))
                                            .child(
                                                div()
                                                    .text_color(hsla(theme.text_muted))
                                                    .child(SharedString::from(kind.clone())),
                                            )
                                            .child(SharedString::from(pct.clone()))
                                    })),
                            ),
                    )
                    .children(
                        [view.range.clone(), view.resets.clone(), view.total.clone()]
                            .into_iter()
                            .flatten()
                            .map(|t| {
                                div()
                                    .text_size(px(10.5))
                                    .text_color(hsla(theme.text_muted))
                                    .child(SharedString::from(t))
                            }),
                    ),
            );
            if view.range.is_some() && view.rows.is_empty() {
                body = body.child(note(crate::ui_text::panel::usage_period_empty().into()));
            }
            // 段の行（主の枠の % の大きい順 = 応答の並び）。棒は段の中で最大の行を幅いっぱいにする
            let max = view
                .rows
                .iter()
                .map(|r| r.main_value)
                .fold(0.0_f64, f64::max);
            for (ri, row) in view.rows.iter().enumerate() {
                let index = row_index;
                row_index += 1;
                let key = row.key.clone();
                let frac = if max > 0.0 {
                    (row.main_value / max) as f32
                } else {
                    0.0
                };
                let detail = [
                    row.secondary.clone(),
                    Some(crate::ui_text::panel::usage_amount(
                        &row.usd,
                        &row.tokens,
                        row.sessions,
                    )),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                body = body.child(
                    div()
                        .id(SharedString::from(format!("usage-row-{li}-{ri}")))
                        .relative()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .when(row.drillable, |d| {
                            d.cursor_pointer()
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.usage.drill(&key) {
                                        this.drain_usage_pending(cx);
                                    }
                                }))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(SharedString::from(row.name.clone())),
                                )
                                .children(row.main.clone().map(|m| {
                                    div()
                                        .flex_none()
                                        .font_family(theme.font_family.clone())
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(SharedString::from(m))
                                }))
                                .children(row.estimated.clone().map(|e| {
                                    div()
                                        .flex_none()
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.text_muted))
                                        .child(SharedString::from(e))
                                }))
                                .when(row.drillable, |d| {
                                    d.child(
                                        svg()
                                            .path(crate::file_icons::ui_icon::CHEVRON_RIGHT)
                                            .flex_none()
                                            .size(px(10.0))
                                            .text_color(hsla(theme.text_faint)),
                                    )
                                }),
                        )
                        .when(row.main.is_some(), |d| {
                            d.child(
                                div()
                                    .h(px(3.0))
                                    .w_full()
                                    .rounded(px(2.0))
                                    .bg(rgba(theme.surface_highlight))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(frac.clamp(0.0, 1.0)))
                                            .bg(hsla(theme.accent)),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_muted))
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(detail)),
                        )
                        .child(probe(format!("usage-row-{index}"))),
                );
            }
            if view.more > 0 {
                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(crate::ui_text::panel::usage_more(
                            view.more,
                        )))
                        .child(
                            div()
                                .id(SharedString::from(format!("usage-more-{li}")))
                                .relative()
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .text_color(hsla(theme.accent))
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.usage.extra_rows += USAGE_ROWS_STEP;
                                    this.usage.pending = true;
                                    this.drain_usage_pending(cx);
                                }))
                                .child(crate::ui_text::panel::usage_show_more())
                                .child(probe(format!("usage-more-{li}"))),
                        ),
                );
            }
            let footer = [
                view.outside
                    .clone()
                    .map(|v| (crate::ui_text::panel::usage_outside().to_string(), v)),
                view.estimated
                    .clone()
                    .map(|v| (crate::ui_text::panel::usage_estimated().to_string(), v)),
            ];
            for (label, value) in footer.into_iter().flatten() {
                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(2.0))
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_muted))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(theme.font_family.clone())
                                .child(SharedString::from(value)),
                        ),
                );
            }
            for text in &view.unavailable {
                body = body.child(
                    div()
                        .px(px(10.0))
                        .py(px(2.0))
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(text.clone())),
                );
            }
        }
        // 注記（API 換算の意味・按分の読み方・索引の読み途中など。既定は畳む）
        let notes: Vec<String> = payload["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect();
        if !notes.is_empty() {
            let open = self.usage.notes_open;
            body = body.child(
                div()
                    .id("usage-notes")
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.usage.notes_open = !this.usage.notes_open;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(if open {
                                crate::file_icons::ui_icon::CHEVRON_DOWN
                            } else {
                                crate::file_icons::ui_icon::CHEVRON_RIGHT
                            })
                            .size(px(9.0))
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child(SharedString::from(crate::ui_text::panel::usage_notes(
                        notes.len(),
                    )))
                    .child(probe("usage-notes".to_string())),
            );
            if open {
                body = body.children(notes.into_iter().map(|n| {
                    div()
                        .px(px(10.0))
                        .py(px(2.0))
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(n))
                }));
            }
        }
        root = root.child(body);
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(key: Value, name: &str, pct7: f64, pct5: f64, usd: f64, tokens: u64) -> Value {
        json!({
            "key": key, "name": name, "api_usd": usd, "total_tokens": tokens, "sessions": 2,
            "responses": 3, "tokens": {},
            "limit_share": { "seven_day": { "percent": pct7, "estimated": 0.44 }, "five_hour": { "percent": pct5 } }
        })
    }

    fn payload(window: &str, groups: Vec<Value>) -> Value {
        json!({
            "window": window,
            "logins": [{
                "label": "A", "login": "abcdef012345", "agent": "claude",
                "windows": [
                    { "kind": "five_hour", "used_percent": 12.0, "resets_at": 1_792_003_600, "expired": false },
                    { "kind": "seven_day", "used_percent": 83.5, "resets_at": 1_792_086_400, "expired": false }
                ],
                "usage": {
                    "range": { "basis": "rolling" },
                    "total": { "api_usd": 20.6, "total_tokens": 365_000_000u64, "sessions": 4 },
                    "groups": groups,
                    "more_groups": 3,
                    "limit_share": [
                        { "kind": "five_hour", "available": true, "outside": 0.0, "estimated": 0.0 },
                        { "kind": "seven_day", "available": true, "outside": 0.4, "estimated": 1.25 }
                    ]
                }
            }]
        })
    }

    /// 行の文字は CLI（`tako usage`）と同じ書式 = `tako_core::usage::display`。主の枠は期間で替わる
    #[test]
    fn 行の文字はcliと同じ書式で主の枠が先() {
        crate::ui_text::tests_support::with_lang(tako_core::i18n::Lang::Ja, || {
            let p = payload(
                "7d",
                vec![
                    group(
                        json!({ "project": "tako" }),
                        "tako",
                        30.94,
                        1.2,
                        20.6,
                        12_345,
                    ),
                    group(
                        json!({ "profile": "default" }),
                        "プロファイル: default",
                        2.0,
                        0.0,
                        0.0,
                        9,
                    ),
                    group(json!({}), "未分類", 0.0, 0.0, 0.0, 0),
                ],
            );
            let views = login_views(&p, &UsageLevel::top(), 1_792_000_000, 0);
            assert_eq!(views.len(), 1);
            let v = &views[0];
            assert_eq!(v.title, "ログイン A");
            assert_eq!(
                v.limits,
                [
                    ("7d".to_string(), "83.5%".to_string()),
                    ("5h".into(), "12%".into())
                ]
            );
            assert_eq!(
                v.range.as_deref(),
                Some("直近 7 日（観測した今の窓が無い）")
            );
            assert_eq!(
                v.total.as_deref(),
                Some("API 換算 $20.60 · 3.65 億 トークン · 会話 4")
            );
            let r = &v.rows[0];
            assert_eq!(
                (r.name.as_str(), r.main.as_deref()),
                ("tako", Some("30.9%"))
            );
            assert_eq!(r.estimated.as_deref(), Some("+0.4"));
            assert_eq!(r.secondary.as_deref(), Some("5h 1.2%"));
            assert_eq!((r.usd.as_str(), r.tokens.as_str()), ("$20.60", "1.2 万"));
            assert!(r.drillable);
            assert_eq!(v.rows[1].name, "プロファイル: default");
            assert_eq!(v.rows[2].name, "未分類");
            assert_eq!(v.more, 3);
            assert_eq!(v.outside.as_deref(), Some("0.4%"));
            assert_eq!(v.estimated.as_deref(), Some("+1.2"));
            // 5h を選ぶと主の枠が 5h
            let p = payload(
                "5h",
                vec![group(
                    json!({ "project": "tako" }),
                    "tako",
                    30.94,
                    1.2,
                    1.0,
                    1,
                )],
            );
            let v = &login_views(&p, &UsageLevel::top(), 1_792_000_000, 0)[0];
            assert_eq!(v.limits[0].0, "5h");
            assert_eq!(v.rows[0].main.as_deref(), Some("1.2%"));
            assert_eq!(v.rows[0].secondary.as_deref(), Some("7d 30.9%"));
        });
    }

    /// 掘り下げ: プロジェクト → Issue → 会話。段の名前はパンくずに出ている部分を省く
    #[test]
    fn 掘り下げとパンくずと問い合わせ() {
        crate::ui_text::tests_support::with_lang(tako_core::i18n::Lang::Ja, || {
            let mut s = UsagePanel::default();
            assert_eq!(s.query().by.as_deref(), Some("project"));
            assert_eq!(s.query().limit, Some(USAGE_ROWS));
            assert!(s.drill(&GroupKey {
                profile: Some("tako".into()),
                ..GroupKey::default()
            }));
            let q = s.query();
            assert_eq!(
                (q.by.as_deref(), q.project.as_deref(), q.issue.as_deref()),
                (Some("issue"), Some("profile:tako"), None)
            );
            assert!(s.drill(&GroupKey {
                profile: Some("tako".into()),
                issue: None,
                ..GroupKey::default()
            }));
            let q = s.query();
            assert_eq!(
                (q.by.as_deref(), q.issue.as_deref()),
                (Some("session"), Some("none"))
            );
            let titles: Vec<String> = s.levels().iter().map(level_title).collect();
            assert_eq!(titles, ["プロジェクト", "プロファイル: tako", "Issue なし"]);
            assert!(!s.drill(&GroupKey::default()), "会話の段は行き止まり");
            // 会話の行の名前（絞り込みで分かるプロジェクト・Issue は省く）
            let key = GroupKey {
                session: Some("0123456789abcdef".into()),
                role: Some("worker".into()),
                label: Some("#2203 usage".into()),
                ..GroupKey::default()
            };
            assert_eq!(
                row_name(&s.current(), &key, "-"),
                "01234567 · worker · #2203 usage"
            );
            s.back_to(0);
            assert_eq!(s.levels(), [UsageLevel::top()]);
            // 期間
            s.set_window("5h");
            assert_eq!(s.query().window.as_deref(), Some("5h"));
            s.set_window("7d");
            assert_eq!(s.window, None);
        });
    }

    /// CLI / MCP の `show` は最上段 + その段を組む（最上段そのものなら 1 段）
    #[test]
    fn showは最上段とその段を組む() {
        let mut s = UsagePanel::default();
        s.show(&UsageQuery::default());
        assert_eq!(s.levels, [UsageLevel::top()]);
        assert!(s.pending);
        s.show(&UsageQuery {
            by: Some("session".into()),
            project: Some("project:tako".into()),
            issue: Some("2203".into()),
            window: Some("5H".into()),
            limit: Some(50),
            ..UsageQuery::default()
        });
        assert_eq!(s.levels.len(), 2);
        let q = s.query();
        assert_eq!(q.by.as_deref(), Some("session"));
        assert_eq!(q.window.as_deref(), Some("5h"));
        assert_eq!(q.limit, Some(50));
        assert_eq!(s.state_json()["depth"], 2);
    }
}

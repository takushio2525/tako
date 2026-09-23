//! 環境診断の項目の正本（#1505）
//!
//! ## 何が起きていたのか
//!
//! 同じ「この環境で tako は使えるか」を、**2 つの実装が別々に答えていた**。
//!
//! - `tako setup --check`（`tako-cli/src/setup.rs`）: エージェント CLI / 依存 /
//!   MCP 登録 / 指示ファイル / プロファイル …を自前の `eprintln!` で並べる。
//!   シェル統合・tako CLI の PATH は #1502 / #1504 で足したが、**更新・remote・
//!   IPC の受け口は 1 行も無かった**
//! - `tako check-health`（`dispatch::check_health`）: tako CLI の PATH / tmux /
//!   DPI / 窓の置き先 / IPC を見るが、**シェル統合・MCP 登録・依存・remote は
//!   1 行も見ていない**
//!
//! 「UI でできることは CLI / MCP でもできる」（設計原則 5）の前に、そもそも
//! **答えが 2 種類あった**。片方に項目を足しても、もう片方は黙って古いままになる。
//!
//! ## この形（項目の正本を 1 つにする）
//!
//! [`collect`] が**項目の集合・判定・人へ出す行**をまとめて持ち、
//!
//! - `tako setup --check` は [`Report::lines`] を出して [`Report::remaining`] で締める
//! - `check_health`（CLI / dispatch / MCP `tako_check_health`）は
//!   [`Report::to_json`] を `diagnostics` 節として載せる
//!
//! の 2 通りが**同じ材料から**出る。判定は 1 つも二重に書かない
//! （tmux の有無・tako CLI の PATH・シェル統合はすでに 1 実装があるので、
//! ここはそれを**呼ぶだけ**で、新しい判断を足さない）。
//!
//! 番犬 `crates/tako-control/tests/issue1505_diagnostics_single_source.rs` が
//! 「`run_check` が正本以外から項目を組む」形を file:line で落とす。
//!
//! ## 重さ（呼ぶ側の責任）
//!
//! [`collect`] はエージェント CLI へ問い合わせるので**秒単位で掛かる**
//! （実測: 3 系統そろった Mac で 10.8 秒。内訳は `claude mcp list` 4.4 秒 /
//! `agy models` 3.0 秒。#1505 前の `--check` は認証の問い合わせを 2 度打っていたので
//! 12.4 秒だった）。GUI から呼ぶ経路（dispatch の `CheckHealth`）は
//! `prepare_offload` で background executor へ出すこと。UI スレッドで呼ぶと
//! そのぶん窓が止まる。

use serde_json::{json, Value};

use tako_core::platform::agent_install::AgentKind;

use crate::setup_remaining::{Remaining, RemainingKind};

/// 診断 1 件の重さ。表示のラベル（`[OK]` 等）と機械可読の語彙を 1 対 1 で持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 整っている
    Ok,
    /// 状態の申告だけ（対処は要らない）
    Info,
    /// 無くても tako は動くが、あると良いもの
    Optional,
    /// 検出の申告（何が見つかったか）
    Detected,
    /// 整っていない（人の手か次のコマンドが要る）
    Missing,
    /// 壊れている / 食い違っている
    Warn,
}

impl Status {
    /// 行頭の `[...]` に入れる語
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Info => "情報",
            Self::Optional => "任意",
            Self::Detected => "検出",
            Self::Missing => "不足",
            Self::Warn => "警告",
        }
    }

    /// 機械可読の語彙（`--json` / MCP がこちらを読む）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Optional => "optional",
            Self::Detected => "detected",
            Self::Missing => "missing",
            Self::Warn => "warn",
        }
    }

    /// 整っていると言えるか（`healthy` の材料）
    pub fn is_ok(self) -> bool {
        matches!(
            self,
            Self::Ok | Self::Info | Self::Optional | Self::Detected
        )
    }
}

/// 診断 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 機械キー（**安定**。`--json` と番犬が名指しに使う）
    pub key: String,
    pub status: Status,
    /// 1 行の要約（ラベルを除いた本文）
    pub summary: String,
    /// 人へ出す行（字下げ込み・そのまま `--check` の出力になる）
    pub lines: Vec<String>,
    /// この項目から積まれる残り作業（#1501）
    pub remaining: Vec<Remaining>,
}

impl Item {
    /// `  [ラベル] 要約` の 1 行だけを持つ項目
    pub fn new(key: impl Into<String>, status: Status, summary: impl Into<String>) -> Self {
        let summary = summary.into();
        Self {
            key: key.into(),
            status,
            lines: vec![format!("  [{}] {summary}", status.label())],
            summary,
            remaining: Vec::new(),
        }
    }

    /// すでに整形済みの文字列（改行を含んでよい）から項目を作る。
    /// 文面の正本が他所（`setup_bootstrap` / `shell_integration`）にある項目用
    pub fn from_text(
        key: impl Into<String>,
        status: Status,
        summary: impl Into<String>,
        text: &str,
    ) -> Self {
        Self {
            key: key.into(),
            status,
            summary: summary.into(),
            lines: text.lines().map(str::to_string).collect(),
            remaining: Vec::new(),
        }
    }

    /// 続きの行を足す（字下げは呼び手が決める）
    pub fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }

    /// 残り作業を足す
    pub fn remaining(mut self, remaining: Remaining) -> Self {
        self.remaining.push(remaining);
        self
    }

    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "status": self.status.as_str(),
            "summary": self.summary,
            "lines": self.lines,
            "remaining": self.remaining.iter().map(remaining_json).collect::<Vec<_>>(),
        })
    }
}

/// 残り作業 1 件を JSON へ（`--check` の末尾と同じ材料を機械可読にする）
fn remaining_json(item: &Remaining) -> Value {
    json!({
        "title": item.kind.title(),
        "command": item.kind.command(),
        "detail": item.detail,
    })
}

/// 診断の結果一式
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub items: Vec<Item>,
    /// 上限で打ち切った問い合わせ（#1503）。**確認できなかったことを黙らせない**。
    /// 人へは `agent_probe` が stderr へ出し、ここは AI / 機械が読む写し
    pub probe_timeouts: Vec<tako_core::probe::TimeoutNotice>,
}

impl Report {
    /// `tako setup --check` に出す行（**残り作業は含まない**）
    pub fn lines(&self) -> Vec<String> {
        self.items
            .iter()
            .flat_map(|item| item.lines.iter().cloned())
            .collect()
    }

    /// 積まれた残り作業（呼び手が `setup_remaining::summarize` へ渡す）
    pub fn remaining(&self) -> Vec<Remaining> {
        self.items
            .iter()
            .flat_map(|item| item.remaining.iter().cloned())
            .collect()
    }

    /// 項目のキー一覧（番犬と機械照合が使う）
    pub fn keys(&self) -> Vec<String> {
        self.items.iter().map(|item| item.key.clone()).collect()
    }

    /// 整っていない項目があるか
    pub fn has_gap(&self) -> bool {
        self.items.iter().any(|item| !item.status.is_ok())
    }

    /// `check_health` の `diagnostics` 節
    pub fn to_json(&self) -> Value {
        let remaining = self.remaining();
        json!({
            "items": self.items.iter().map(Item::to_json).collect::<Vec<_>>(),
            "keys": self.keys(),
            "remaining": crate::setup_remaining::summarize(remaining)
                .iter()
                .map(remaining_json)
                .collect::<Vec<_>>(),
            // #1503: 上限で打ち切った問い合わせ（`dispatch::SetupRun` の
            // `probe_timeouts` と同じ形）。空なら全部を確認できたということ
            "probe_timeouts": self.probe_timeouts.iter().map(|n| json!({
                "label": n.label,
                "waited_secs": n.waited_secs,
            })).collect::<Vec<_>>(),
        })
    }
}

/// 環境診断の全項目（**この 1 実装が正本**）。
///
/// 重い（エージェント CLI へ問い合わせる）ので、UI スレッドから呼ばないこと。
pub fn collect() -> Report {
    let mut items = Vec::new();
    let mut probe_timeouts = Vec::new();
    let bootstrap = crate::setup_bootstrap::status_all();
    items.extend(agent_cli_items(&bootstrap));
    items.push(tako_cli_path_item());
    items.push(shell_integration_item());
    items.push(agents_detected_item(&bootstrap));
    items.extend(dep_items());
    items.extend(fda_item());
    let (mcp, mcp_timeouts) = mcp_items(&bootstrap);
    items.extend(mcp);
    probe_timeouts.extend(mcp_timeouts);
    items.extend(agent_role_items(&bootstrap));
    items.extend(setup_items());
    items.extend(instruction_items(&bootstrap));
    items.push(agents_sync_item());
    items.extend(sleep_guard_items());
    items.push(config_share_item());
    items.push(profiles_item());
    items.push(remote_item());
    items.push(ipc_item());
    Report {
        items,
        probe_timeouts,
    }
}

// --- エージェント CLI のゼロスタート導入（#868 / #989） ----------------------

type BootstrapStates = [(
    AgentKind,
    Result<crate::setup_bootstrap::BootstrapState, String>,
)];

/// 導入・PATH・ログインの段（`tako setup bootstrap status-all` と同じ材料）
fn agent_cli_items(states: &BootstrapStates) -> Vec<Item> {
    use crate::setup_bootstrap::Step;

    let ready: Vec<&str> = states
        .iter()
        .filter(|(_, state)| state.as_ref().is_ok_and(|s| s.step == Step::Ready))
        .map(|(agent, _)| agent.as_str())
        .collect();
    // **1 つでも ready なら tako は使える**ので、そこを最初に言う
    let mut head = if ready.is_empty() {
        Item::new(
            "agent_cli",
            Status::Missing,
            "エージェント CLI の導入: 使える系統がありません",
        )
        .line("         tako setup を実行すると、ここから最後まで案内します")
    } else {
        Item::new(
            "agent_cli",
            Status::Ok,
            format!(
                "エージェント CLI の導入: 使える系統 {}（導入 / PATH / ログイン）",
                ready.join(" / ")
            ),
        )
    };
    // 1 つも使える系統が無いときだけ、**一番短い道**を残り作業にする
    if ready.is_empty() {
        if let Some(remaining) = shortest_bootstrap_remaining(states) {
            head = head.remaining(remaining);
        }
    }

    let mut items = vec![head];
    for (agent, state) in states {
        let key = format!("agent_cli.{}", agent.as_str());
        items.push(match state {
            Ok(state) if state.step == Step::Ready => Item::from_text(
                key,
                Status::Ok,
                format!("{}: 完了", agent.as_str()),
                &format!("         [OK] {}: 完了", agent.as_str()),
            ),
            Ok(state) => Item::from_text(
                key,
                Status::Missing,
                format!(
                    "{}: {} ({})",
                    agent.as_str(),
                    state.step_description(),
                    state.step.as_str()
                ),
                &format!(
                    "         [不足] {}: {} ({})",
                    agent.as_str(),
                    state.step_description(),
                    state.step.as_str()
                ),
            ),
            Err(e) => Item::from_text(
                key,
                Status::Warn,
                format!("{}: 確認できません（{e}）", agent.as_str()),
                &format!("         [警告] {}: 確認できません（{e}）", agent.as_str()),
            ),
        });
    }
    items
}

/// 使える系統が 1 つも無いときに人が進む**一番短い道**。
///
/// どの系統を仕上げるかの判断は `setup_bootstrap::choose_target` の 1 実装
/// （`tako setup` の bootstrap 段が実際に面倒を見る系統と同じもの）
fn shortest_bootstrap_remaining(states: &BootstrapStates) -> Option<Remaining> {
    if states.is_empty() {
        return None;
    }
    let target = crate::setup_bootstrap::choose_target(states);
    match states
        .iter()
        .find(|(agent, _)| *agent == target)
        .map(|(_, state)| state)
    {
        Some(Ok(state)) => RemainingKind::for_step(state.step, target).map(Remaining::new),
        Some(Err(e)) => Some(Remaining::with_detail(
            RemainingKind::AgentStatusUnknown,
            vec![e.clone()],
        )),
        None => None,
    }
}

/// 検出できた CLI の一覧（パス / 認証 / プラン）。判定は `status_all` の 1 実装から引く
fn agents_detected_item(states: &BootstrapStates) -> Item {
    let detected: Vec<(AgentKind, &crate::setup_bootstrap::BootstrapState)> = states
        .iter()
        .filter_map(|(agent, state)| state.as_ref().ok().map(|s| (*agent, s)))
        .filter(|(_, state)| state.binary.is_some())
        .collect();
    if detected.is_empty() {
        let mut item = Item::from_text(
            "agents_detected",
            Status::Missing,
            "claude / codex / agy のいずれも見つかりません",
            "  エージェント CLI:\n    [不足] claude / codex / agy のいずれも見つかりません",
        );
        for agent in crate::setup_bootstrap::bootstrap_agents() {
            item = item.line(format!("      {}: {}", agent.as_str(), install_hint(agent)));
        }
        return item.remaining(Remaining::new(RemainingKind::AgentMissing));
    }
    let names: Vec<&str> = detected.iter().map(|(agent, _)| agent.as_str()).collect();
    let mut item = Item::from_text(
        "agents_detected",
        Status::Detected,
        format!("検出: {}", names.join(" / ")),
        "  エージェント CLI:",
    );
    for (agent, state) in detected {
        let auth = if state.authenticated {
            "認証済み"
        } else {
            "未認証"
        };
        let plan = state.account_plan.as_deref().unwrap_or("プラン不明");
        let path = state
            .binary
            .as_deref()
            .map(tako_core::paths::shorten_home)
            .unwrap_or_default();
        item = item.line(format!(
            "    [検出] {}: {path}（{auth} / {plan}）",
            agent.as_str()
        ));
    }
    item
}

/// 導入の案内 1 行（正本は境界 B17 = `platform::agent_install`。#989 / #322）
fn install_hint(agent: AgentKind) -> String {
    let guidance = crate::orchestrator::agent_cli::guidance(agent.into());
    match (guidance.command, guidance.docs_url) {
        (Some(cmd), _) => cmd.to_string(),
        (None, Some(url)) => url.to_string(),
        (None, None) => guidance
            .manual
            .map(|m| m.text().to_string())
            .unwrap_or_default(),
    }
}

/// master を任せられるか / worker 専用かの申告（#989）
fn agent_role_items(states: &BootstrapStates) -> Vec<Item> {
    let installed = |kind: AgentKind| {
        states.iter().any(|(agent, state)| {
            *agent == kind && state.as_ref().is_ok_and(|s| s.binary.is_some())
        })
    };
    let mut items = Vec::new();
    if installed(AgentKind::Codex) {
        items.push(Item::new(
            "agent_role.codex",
            Status::Ok,
            "Codex: master 起動時にも一時注入",
        ));
    }
    if installed(AgentKind::Agy) {
        items.push(Item::new(
            "agent_role.agy",
            Status::Info,
            "agy: worker 専用（master は非対応）",
        ));
    }
    items
}

// --- tako CLI の PATH（#1502）/ シェル統合（#1504） -------------------------

fn tako_cli_path_item() -> Item {
    let text = crate::setup_bootstrap::tako_cli_path_check_line();
    let status = status_from_line(&text);
    Item::from_text("tako_cli_path", status, summary_of(&text), &text)
}

fn shell_integration_item() -> Item {
    let text = crate::shell_integration::check_line();
    let status = status_from_line(&text);
    let mut item = Item::from_text("shell_integration", status, summary_of(&text), &text);
    if let Some(remaining) = crate::shell_integration::check_remaining() {
        item = item.remaining(remaining);
    }
    item
}

/// 他所が組んだ 1 行目の `[ラベル]` から重さを読む（文面の正本を 2 つにしないため）
fn status_from_line(text: &str) -> Status {
    let head = text.lines().next().unwrap_or_default();
    for status in [
        Status::Ok,
        Status::Info,
        Status::Optional,
        Status::Detected,
        Status::Missing,
        Status::Warn,
    ] {
        if head.contains(&format!("[{}]", status.label())) {
            return status;
        }
    }
    // `[設置]` `[legacy]` 等（段が実行された形）は「整っている」側へ倒す
    Status::Info
}

/// 1 行目からラベルを落とした本文（機械可読の要約）
fn summary_of(text: &str) -> String {
    let head = text.lines().next().unwrap_or_default().trim();
    match (head.find('['), head.find(']')) {
        (Some(0), Some(close)) => head[close + 1..].trim().to_string(),
        _ => head.to_string(),
    }
}

// --- 任意依存（#88 / #1499 / #1509 / #1524） --------------------------------

/// 依存ツール 1 件ごとの状態。**案内の文面は `setup_deps` の 1 実装が持つ**ので、
/// 読み取り専用の文脈（`CheckOnly`）でそこを呼んで出力を受け取る
/// （呼び手が案内を組み直すと `tako setup` と `--check` で文面が割れる = #1509）
fn dep_items() -> Vec<Item> {
    crate::setup_deps::status()
        .into_iter()
        .map(|state| {
            let dep = state.dep;
            let key = format!("dep.{}", dep.bin);
            if let Some(path) = &state.found {
                return Item::new(key, Status::Ok, format!("{}: {path}", dep.bin));
            }
            let (mark, kind) = if dep.required {
                (Status::Missing, "必須")
            } else {
                (Status::Optional, "任意")
            };
            let mut item = Item::new(key, mark, format!("{}: 見つかりません（{kind}）", dep.bin))
                .line(format!("      用途: {}", dep.purpose));
            if !dep.required {
                item = item.line("      無くても tako 自体は動きますが、上記の機能が使えません");
            }
            for line in dep_guide_lines(&state) {
                item = item.line(line);
            }
            if dep.required {
                item = item.remaining(Remaining::new(RemainingKind::Dep {
                    bin: dep.bin.to_string(),
                }));
            }
            item
        })
        .collect()
}

/// 読み取りだけの文脈で `setup_deps` が出す案内行（**導入は 1 つも起こさない**）
fn dep_guide_lines(state: &crate::setup_deps::DepStatus) -> Vec<String> {
    let ctx = crate::setup_deps::DepOfferContext {
        // `--check` は読み取り専用（`offer_for` が `CheckOnly` へ倒す唯一の条件）
        stage_installs: false,
        review: false,
        assume_yes: false,
        stdin_is_terminal: false,
        legacy: crate::setup_deps::legacy_mode(),
    };
    let mut buffer: Vec<u8> = Vec::new();
    let mut reader = std::io::empty();
    let outcome = crate::setup_deps::offer_and_install(
        state,
        ctx,
        &mut crate::setup_deps::DepPromptIo {
            writer: &mut buffer,
            reader: &mut reader,
            indent: "      ",
        },
    );
    debug_assert!(
        matches!(outcome, Ok(crate::setup_deps::DepOutcome::NotInstalled)),
        "読み取り専用の文脈で導入が走った（#1505 / #1499）"
    );
    String::from_utf8_lossy(&buffer)
        .lines()
        .map(str::to_string)
        .collect()
}

// --- フルディスクアクセス（macOS） ------------------------------------------

#[cfg(target_os = "macos")]
fn fda_item() -> Vec<Item> {
    if crate::fda::is_granted() {
        return vec![Item::new(
            "fda",
            Status::Ok,
            "フルディスクアクセス: 付与済み（許可ダイアログは表示されません）",
        )];
    }
    vec![Item::new(
        "fda",
        Status::Optional,
        "フルディスクアクセス: 未付与（推奨）",
    )
    .line("      macOS が「tako.app から、ほかのアプリからのデータへのアクセス権を")
    .line("      求められています」と頻繁に表示する原因です。フルディスクアクセスを")
    .line("      付与すると、このダイアログが出なくなります。")
    .line("      設定方法: システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を追加")
    .line("      付与方法: tako fda open でシステム設定を開き、tako を追加してください")]
}

#[cfg(not(target_os = "macos"))]
fn fda_item() -> Vec<Item> {
    Vec::new()
}

// --- MCP 登録（#979） -------------------------------------------------------

fn mcp_items(states: &BootstrapStates) -> (Vec<Item>, Vec<tako_core::probe::TimeoutNotice>) {
    use crate::agent_mcp::McpState;
    use crate::orchestrator::agent::WorkerAgent;

    let installed = |kind: AgentKind| {
        states.iter().any(|(agent, state)| {
            *agent == kind && state.as_ref().is_ok_and(|s| s.binary.is_some())
        })
    };
    let mut items = Vec::new();
    let mut timeouts = Vec::new();
    // claude だけ登録の読み方が違う（`dispatch::setup_mcp` が書き手なので
    // `agent_mcp` は面倒を見ない = `handled_here` が false）
    if installed(AgentKind::Claude) {
        // 打ち切りの知らせも受け取る（#1503。確認できなかったことを黙らせない）
        let (registered, healthy, notice) = crate::agent_mcp::claude_health_now();
        timeouts.extend(notice);
        let key = "mcp.claude";
        let item = if registered && healthy {
            Item::new(key, Status::Ok, "Claude MCP: tako が登録済み")
        } else if registered {
            let mut item = Item::new(
                key,
                Status::Warn,
                "Claude MCP: 登録済みだがパスが消失しています",
            );
            if let Some(cmd) = crate::agent_mcp::claude_registered_command() {
                item = item.line(format!("         登録パス: {cmd}"));
            }
            item.line("         tako setup または tako setup-mcp で修復できます")
        } else {
            Item::new(
                key,
                Status::Missing,
                "Claude MCP: tako が未登録（tako setup-mcp で登録できます）",
            )
        };
        items.push(if registered && healthy {
            item
        } else {
            item.remaining(Remaining::new(RemainingKind::Mcp {
                agent: "claude".to_string(),
            }))
        });
    }
    for (kind, agent) in [
        (AgentKind::Codex, WorkerAgent::Codex),
        (AgentKind::Agy, WorkerAgent::Agy),
    ] {
        if !installed(kind) {
            continue;
        }
        let name = agent.as_str();
        let key = format!("mcp.{name}");
        let state = crate::agent_mcp::state(agent);
        let mut item = match &state {
            McpState::Ready { .. } => {
                Item::new(key, Status::Ok, format!("{name} MCP: tako が登録済み"))
            }
            McpState::Dead { command } => Item::new(
                key,
                Status::Warn,
                format!("{name} MCP: 登録済みだがパスが消失しています"),
            )
            .line(format!("         登録パス: {command}"))
            .line("         tako setup または tako setup-mcp で修復できます"),
            // 登録行はあるのに env の転送が無い = ツールが 0 個になる（#979 の実測）。
            // 「未登録」と混ぜると原因を追えないので別の文言にする
            McpState::EnvMissing { .. } => Item::new(
                key,
                Status::Warn,
                format!("{name} MCP: 登録済みだが tako と通信する env の転送設定がありません"),
            )
            .line("         そのままだとツールが 0 個になります")
            .line(format!("         tako setup-mcp --agent {name} で入れ直せます")),
            McpState::NotRegistered => Item::new(
                key,
                Status::Missing,
                format!("{name} MCP: tako が未登録（tako setup-mcp で登録できます）"),
            ),
            // CLI は検出できているので Unknown は「一覧を読めなかった」だけ
            McpState::Unknown => Item::new(
                key,
                Status::Warn,
                format!("{name} MCP: 登録状態を確認できません（{name} mcp list を手で確認してください）"),
            ),
        };
        if state.describe_gap().is_some() {
            item = item.remaining(Remaining::new(RemainingKind::Mcp {
                agent: name.to_string(),
            }));
        }
        items.push(item);
    }
    (items, timeouts)
}

// --- セットアップ本体と追従（#94） ------------------------------------------

fn setup_items() -> Vec<Item> {
    let Ok(config_path) = crate::setup::config_yaml_path() else {
        return vec![
            Item::new(
                "setup",
                Status::Info,
                "config.yaml: 置き場を特定できません（HOME が未設定）",
            ),
            update_item_unapplied(),
        ];
    };
    if !config_path.is_file() {
        return vec![
            Item::new("setup", Status::Info, "config.yaml: 未作成"),
            update_item_unapplied(),
        ];
    }
    let config = match crate::setup::load_config() {
        Ok(config) => config,
        Err(e) => {
            return vec![
                Item::new(
                    "setup",
                    Status::Warn,
                    format!("config.yaml: 読めません（{e}）"),
                ),
                update_item_unapplied(),
            ]
        }
    };
    if !config.setup.completed {
        return vec![
            Item::new("setup", Status::Info, "セットアップ: 未完了"),
            // **項目を落とさない**（#1505）: 「いま何リビジョンか」は未実施でも答えられる。
            // ここで消すと状態によって項目の集合が変わり、機械照合が成り立たない
            update_item_unapplied(),
        ];
    }
    let mut items = vec![Item::new(
        "setup",
        Status::Ok,
        format!(
            "セットアップ: 完了済み ({})",
            config.setup.completed_at.as_deref().unwrap_or("日時不明")
        ),
    )];
    // アップデート追従状況（Issue #94）
    items.push(
        match crate::setup::pending_changes(config.setup.applied_revision) {
            Ok(pending) if pending.is_empty() => Item::new(
                "update",
                Status::Ok,
                format!(
                    "アップデート追従: 最新（rev {}）",
                    config.setup.applied_revision
                ),
            ),
            Ok(pending) => Item::new(
                "update",
                Status::Info,
                format!(
                    "アップデート追従: 未適用の setup 変更が {} 件（tako setup --changes で詳細）",
                    pending.len()
                ),
            ),
            Err(e) => Item::new(
                "update",
                Status::Warn,
                format!("アップデート追従: 確認できません（{e}）"),
            ),
        },
    );
    if let Some(agent) = config.setup.selected_agent.as_deref() {
        items.push(Item::new(
            "setup.agent",
            Status::Ok,
            format!("既定エージェント: {agent}"),
        ));
    }
    if !config.setup.provider_plans.is_empty() {
        let plans = config
            .setup
            .provider_plans
            .iter()
            .map(|(provider, plan)| format!("{provider}={plan}"))
            .collect::<Vec<_>>()
            .join(", ");
        items.push(Item::new(
            "setup.plans",
            Status::Ok,
            format!("申告・検出プラン: {plans}"),
        ));
    }
    items
}

/// まだ setup を通していないときの追従行（**項目は落とさない**）
fn update_item_unapplied() -> Item {
    match crate::setup::current_revision() {
        Ok(current) => Item::new(
            "update",
            Status::Info,
            format!("アップデート追従: 未適用（現在の setup リビジョンは {current}）"),
        ),
        Err(e) => Item::new(
            "update",
            Status::Warn,
            format!("アップデート追従: 確認できません（{e}）"),
        ),
    }
}

// --- グローバル指示ファイル -------------------------------------------------

fn instruction_items(states: &BootstrapStates) -> Vec<Item> {
    states
        .iter()
        .filter(|(_, state)| state.as_ref().is_ok_and(|s| s.binary.is_some()))
        .filter_map(|(agent, _)| {
            let path = instruction_path(*agent)?;
            let display = tako_core::paths::shorten_home(&path.to_string_lossy());
            let key = format!("instructions.{}", agent.as_str());
            Some(if path.is_file() {
                Item::new(key, Status::Ok, format!("{display}: 存在します"))
            } else {
                Item::new(key, Status::Info, format!("{display}: 未作成"))
            })
        })
        .collect()
}

/// 系統ごとのグローバル指示ファイル（`tako setup` が書く先と同じ置き場）
fn instruction_path(agent: AgentKind) -> Option<std::path::PathBuf> {
    let home = tako_core::paths::home_dir().filter(|p| p.is_absolute())?;
    Some(match agent {
        AgentKind::Claude => home.join(".claude/CLAUDE.md"),
        AgentKind::Codex => codex_home_dir()?.join("AGENTS.md"),
        AgentKind::Agy => home.join(".gemini/GEMINI.md"),
    })
}

fn codex_home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            tako_core::paths::home_dir()
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".codex"))
        })
}

// --- エージェント共通ルール同期（#136） --------------------------------------

fn agents_sync_item() -> Item {
    let key = "agents_sync";
    match crate::agents_sync::status() {
        Ok(status) => {
            let st = status["status"].as_str().unwrap_or("unknown");
            match st {
                "not_configured" => {
                    Item::new(key, Status::Info, "エージェント共通ルール同期: 未設定")
                }
                "up_to_date" => Item::new(key, Status::Ok, "エージェント共通ルール同期: 最新"),
                "outdated" => Item::new(
                    key,
                    Status::Info,
                    "エージェント共通ルール同期: ずれあり（tako agents sync-rules で同期）",
                ),
                "source_missing" => {
                    let path = status["source_path"].as_str().unwrap_or("?");
                    Item::new(
                        key,
                        Status::Missing,
                        format!("エージェント共通ルール同期: 正本が見つからない ({path})"),
                    )
                }
                other => Item::new(
                    key,
                    Status::Warn,
                    format!("エージェント共通ルール同期: {other}"),
                ),
            }
        }
        Err(e) => Item::new(
            key,
            Status::Info,
            format!("エージェント共通ルール同期: 確認失敗 ({e})"),
        ),
    }
}

// --- スリープ防止（#173 / #524） --------------------------------------------

fn sleep_guard_items() -> Vec<Item> {
    use crate::sleep_guard::{LidSleepMode, SleepGuardMode};

    let settings = crate::settings::load();
    let mode = settings.sleep_guard_mode;
    let power = settings.sleep_guard_power;
    let mut items = vec![match mode {
        SleepGuardMode::Off => Item::new(
            "sleep_guard",
            Status::Info,
            "スリープ防止: 無効（tako sleep-guard set --mode while-agents-running で有効化）",
        ),
        _ => Item::new(
            "sleep_guard",
            Status::Ok,
            format!(
                "スリープ防止: mode={}, power={}",
                mode.as_str(),
                power.as_str()
            ),
        ),
    }];
    // 蓋閉じ継続を持たない OS では案内しない（#524）
    if crate::sleep_guard::lid_control_supported() {
        // 未完了かどうかだけを見る。手段（macOS の sudoers）は sleep_guard の内側（#697）
        let setup_pending = crate::sleep_guard::lid_setup_pending();
        items.push(match settings.lid_sleep_mode {
            LidSleepMode::Off => Item::new(
                "lid_sleep",
                Status::Info,
                "蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）",
            ),
            LidSleepMode::WhileAgentsRunning if setup_pending => Item::new(
                "lid_sleep",
                Status::Missing,
                "蓋閉じ防止: while-agents-running だが sudoers 未登録（tako sleep-guard install-lid-sleep で登録）",
            ),
            LidSleepMode::WhileAgentsRunning => {
                Item::new("lid_sleep", Status::Ok, "蓋閉じ防止: while-agents-running")
            }
        });
    }
    items
}

// --- 設定共有（#513 / #793） ------------------------------------------------

fn config_share_item() -> Item {
    let lines = config_share_lines(&crate::config_share::env::detect(), false);
    let text = lines.join("\n");
    Item::from_text(
        "config_share",
        status_from_line(&text),
        summary_of(&text),
        &text,
    )
}

/// 設定共有の状態表示（Issue #793）。setup サマリと `--check` が**同じ判定**
/// （`config_share::env::Guidance`）から文言を作るので、片方だけ古くならない。
/// **質問は含まない**（#262 の質問ゼロ）。`verbose` = setup サマリ向けに説明行を足す
pub fn config_share_lines(
    env: &crate::config_share::env::ShareEnvironment,
    verbose: bool,
) -> Vec<String> {
    use crate::config_share::env::Guidance;

    let repo = env.repo.as_deref().unwrap_or("?");
    let next = env.next_command();
    let mut lines = Vec::new();
    match env.guidance() {
        // 配線済みなら状態を 1 行示すだけ。勧誘はしない（#793 受け入れ条件 4 = 冪等）
        Guidance::Linked => {
            lines.push(format!("  [OK] 設定共有: 配線済み（{repo}）"));
            if verbose {
                lines.push(
                    "       差分は `tako config status`、同期は `tako config push` / `pull`".into(),
                );
            }
        }
        Guidance::Broken => {
            lines.push(format!(
                "  [警告] 設定共有: 配線先が git リポジトリではありません（{repo}）"
            ));
            lines.push(format!("         `{next}` で繋ぎ直せます"));
        }
        // 既に自力で共有している利用者には、まず相乗りを示す（二重管理を作らない）
        Guidance::AdoptExisting => {
            lines.push("  [情報] 設定共有: 未配線".into());
            for found in &env.external {
                lines.push(format!(
                    "         {} は既に {} で管理されています{}",
                    found.path,
                    found.repo,
                    if found.same_place {
                        "（tako の置き場と一致）"
                    } else {
                        "（tako の置き場とは別）"
                    }
                ));
            }
            lines.push(format!("         同じリポジトリへ相乗りするなら `{next}`"));
            lines.push(
                "         別のリポジトリを作ると同じ内容が 2 箇所に並びます（二重管理）".into(),
            );
        }
        Guidance::Fresh => {
            lines.push(format!(
                "  [情報] 設定共有: 未配線（複数デバイスで同じ AI 設定を使うなら `{next}`）"
            ));
            if verbose {
                lines.push(
                    "         claude のグローバル指示と tako の宣言的設定を git 1 本で共有します"
                        .into(),
                );
                lines.push(
                    "         秘匿情報とこのマシン固有の状態は共有対象から構造的に外れます".into(),
                );
            }
        }
    }
    lines
}

// --- プロファイル -----------------------------------------------------------

fn profiles_item() -> Item {
    match crate::orchestrator::list_profiles() {
        Ok(profiles) if !profiles.is_empty() => Item::new(
            "profiles",
            Status::Ok,
            format!(
                "プロファイル: {} 個（{}）",
                profiles.len(),
                profiles.join(", ")
            ),
        ),
        Ok(_) => Item::new(
            "profiles",
            Status::Info,
            "プロファイル: 未作成（tako master で自動生成されます）",
        ),
        Err(e) => Item::new(
            "profiles",
            Status::Info,
            format!("プロファイル: 確認失敗 ({e})"),
        ),
    }
}

// --- リモート公開（#1049 / #1485） ------------------------------------------

/// スマホから使えるか。**状態ファイルを読むだけ**（`daemon_status` は死んだ
/// プロセスの残骸を掃除する = 書き込みがあるので `--check` からは呼ばない）
fn remote_item() -> Item {
    let key = "remote";
    let desired = crate::remote::read_desired().is_some();
    let Some(pid) = crate::remote::daemon_pid_if_alive() else {
        return if desired {
            // #1485 の症状（公開のはずが止まっている）を名指しする
            Item::new(
                key,
                Status::Missing,
                "リモート公開: 前回は公開していましたが、いまは止まっています",
            )
            .line("         tako remote start で公開し直せます")
        } else {
            Item::new(
                key,
                Status::Info,
                "リモート公開: 停止中（スマホから使うなら tako remote start）",
            )
        };
    };
    // 動いていることと tailnet から見えることは別物（#1049）
    let health = crate::remote::read_serve_health();
    let fields = crate::remote_serve::status_fields(
        health.as_ref(),
        crate::remote_autostart::now_epoch_secs(),
        crate::remote_serve::watch_interval().as_secs(),
    );
    match crate::remote_serve::degraded_warning(&fields) {
        Some(warning) => {
            let mut item = Item::new(
                key,
                Status::Warn,
                "リモート公開: 稼働中ですが tailnet からは届いていません",
            );
            for line in warning.lines() {
                item = item.line(format!("         {line}"));
            }
            item.line("         tako remote status で詳細を確認できます")
        }
        None => Item::new(
            key,
            Status::Ok,
            format!("リモート公開: 稼働中（pid {pid}）"),
        ),
    }
}

// --- IPC の受け口（#1441） --------------------------------------------------

/// CLI / MCP が届くか。
///
/// **観測者で文面を変えない**のが要点（#1505）。GUI の中では起動時の記録
/// （`ipc_socket::status`）、外からは繋ぎ先の実測（ソケットへ 1 回繋ぐ）を読むが、
/// 届いているときはどちらも**同じ 1 行**になる。受け口の詳細（`kind` / バイト数 /
/// bind エラー）は `check_health` の `ipc` 節が持つ（#1441）ので、ここでは重ねない
fn ipc_item() -> Item {
    const KEY: &str = "ipc";
    const REACHABLE: &str = "IPC の受け口: 立っています（tako CLI / MCP が届きます）";

    // GUI の中（起動時に記録がある）
    if let Some(status) = tako_core::ipc_socket::status() {
        if status.bound {
            return Item::new(KEY, Status::Ok, REACHABLE);
        }
        return Item::new(
            KEY,
            Status::Warn,
            "IPC の受け口: 立っていません（tako CLI / MCP は届きません）",
        )
        .line(format!(
            "         理由: {}",
            status.error.as_deref().unwrap_or("理由不明")
        ));
    }
    // 外から（繋ぎ先の実測）。置き場の決め方は `ipc_socket` の 1 実装を通す（#1441）
    let Some(data_dir) = tako_core::paths::data_dir() else {
        return Item::new(
            KEY,
            Status::Warn,
            "IPC の受け口: データディレクトリを特定できません",
        );
    };
    let limit = tako_core::ipc_socket::max_path_bytes();
    let socket = tako_core::ipc_socket::resolve_with(&data_dir, &std::env::temp_dir(), limit);
    if socket.exists() && crate::discovery::socket_alive(&socket.to_string_lossy()) {
        return Item::new(KEY, Status::Ok, REACHABLE);
    }
    // #1441 の症状（深い data dir でソケットが上限を超える）は原因が違うので名指しする
    let bytes = tako_core::ipc_socket::path_bytes(&socket);
    if bytes > limit {
        return Item::new(
            KEY,
            Status::Warn,
            "IPC の受け口: データディレクトリが深すぎます（tako CLI / MCP は届きません）",
        )
        .line(format!(
            "         ソケットパスが {bytes} バイトで上限 {limit} バイトを超えています"
        ))
        .line("         もっと浅い TAKO_DATA_DIR で起動し直してください");
    }
    Item::new(
        KEY,
        Status::Info,
        "IPC の受け口: 届きません（tako アプリが起動していません）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ラベルと機械可読の語彙が1対1() {
        for status in [
            Status::Ok,
            Status::Info,
            Status::Optional,
            Status::Detected,
            Status::Missing,
            Status::Warn,
        ] {
            assert!(!status.label().is_empty());
            assert!(!status.as_str().is_empty());
            // 行から重さを読み戻せる（文面の正本が他所にある項目で使う）
            let line = format!("  [{}] なにか: 状態", status.label());
            assert_eq!(status_from_line(&line), status, "{line}");
        }
    }

    #[test]
    fn 要約はラベルを落とした本文になる() {
        assert_eq!(
            summary_of("  [OK] tmux: /usr/bin/tmux"),
            "tmux: /usr/bin/tmux"
        );
        assert_eq!(
            summary_of("  [不足] tako CLI の PATH: 使えません\n         次の 1 手"),
            "tako CLI の PATH: 使えません"
        );
        // ラベルが無い行はそのまま
        assert_eq!(summary_of("  エージェント CLI:"), "エージェント CLI:");
    }

    #[test]
    fn 項目のjsonはキーと重さと行を持つ() {
        let item = Item::new("dep.tmux", Status::Ok, "tmux: /usr/bin/tmux")
            .remaining(Remaining::new(RemainingKind::ShellIntegration));
        let value = item.to_json();
        assert_eq!(value["key"], "dep.tmux");
        assert_eq!(value["status"], "ok");
        assert_eq!(value["summary"], "tmux: /usr/bin/tmux");
        assert_eq!(value["lines"][0], "  [OK] tmux: /usr/bin/tmux");
        assert_eq!(
            value["remaining"][0]["command"],
            "tako shell-integration install"
        );
    }

    #[test]
    fn レポートは行と残りを項目から組む() {
        let report = Report {
            probe_timeouts: Vec::new(),
            items: vec![
                Item::new("a", Status::Ok, "あ"),
                Item::new("b", Status::Missing, "い")
                    .line("      続き")
                    .remaining(Remaining::new(RemainingKind::AgentMissing)),
            ],
        };
        assert_eq!(report.keys(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            report.lines(),
            vec!["  [OK] あ", "  [不足] い", "      続き"]
        );
        assert_eq!(report.remaining().len(), 1);
        assert!(report.has_gap(), "不足が 1 件あるのに gap でない");
        let value = report.to_json();
        assert_eq!(value["items"].as_array().map(Vec::len), Some(2));
        assert_eq!(value["keys"][1], "b");
        assert_eq!(value["remaining"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn 残り作業は面倒を見る系統の段から積まれる() {
        // 読めない系統しか無いときは理由を持ち上げる（黙って消さない）
        let states = vec![(AgentKind::Claude, Err("読めません".to_string()))];
        let remaining = shortest_bootstrap_remaining(&states).expect("残り作業が選ばれる");
        assert_eq!(remaining.kind, RemainingKind::AgentStatusUnknown);
        assert_eq!(remaining.detail, vec!["読めません".to_string()]);
        // 空なら 1 件も積まない
        assert!(shortest_bootstrap_remaining(&[]).is_none());
    }
}

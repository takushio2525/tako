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
//!
//! 外部プロセスを起こす問い合わせは**重ねて走らせる**（#1622。[`collect_with`]）。
//! 導入状況（約 3 秒）を呼び出し元のスレッドで採る間に、`claude mcp list`（約 5.7 秒）ほかを
//! 背景で最大 [`PROBE_CONCURRENCY`] 本ずつ走らせ、項目は今まで通りの並びで組む（実測 2026-10-30:
//! check-health 9.7〜12.0 秒 → 5.8〜6.4 秒・出力はバイト一致）。番犬はこのファイルの単体テスト
//! `外部の問い合わせは導入状況と重ねて走り答えは逐次と同じ`（偽の問い合わせが互いの始まりを
//! 待ち合う = 時間で比べない）、実経路は `scripts/test-check-health-parallel-1622.sh`。
//! A/B は `TAKO_1622_LEGACY=1`（重ねない）。
//!
//! 認証の問い合わせ（`claude auth status --json` / `codex login status` / `agy models`）は
//! **上限つきで系統を並べて**聞く（#2255。既定 10 秒 = `probe::auth_probe_timeout`）。
//! check-health は固まった GUI の切り分けに使う道具なので、返らない CLI 1 つで診断ごと
//! 固まると切り分けができない。打ち切った系統は「認証: 不明（N 秒で応答なし）」と出し、
//! `probe_timeouts` にも載せる。番犬 `crates/tako-control/tests/issue2255_diagnostics_auth_probe_watchdog.rs`
//! が [`collect`] から辿れる関数で上限なしの `agent_probe::run` を呼ぶ形を file:line で落とす

use serde_json::{json, Value};

use tako_core::i18n::Lang;
use tako_core::platform::agent_install::AgentKind;

use crate::setup_remaining::{Remaining, RemainingKind};
use crate::HealthText;

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
    const ALL: [Status; 6] = [
        Self::Ok,
        Self::Info,
        Self::Optional,
        Self::Detected,
        Self::Missing,
        Self::Warn,
    ];

    /// 行頭の `[...]` に入れる語（日本語）
    pub fn label(self) -> &'static str {
        self.label_in(Lang::Ja)
    }

    /// 行頭の `[...]` に入れる語を表示言語で（#2365。英語は `tako setup` の段の `[warning]` と揃える）
    pub fn label_in(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Ok, _) => "OK",
            (Self::Info, Lang::Ja) => "情報",
            (Self::Info, Lang::En) => "info",
            (Self::Optional, Lang::Ja) => "任意",
            (Self::Optional, Lang::En) => "optional",
            (Self::Detected, Lang::Ja) => "検出",
            (Self::Detected, Lang::En) => "detected",
            (Self::Missing, Lang::Ja) => "不足",
            (Self::Missing, Lang::En) => "missing",
            (Self::Warn, Lang::Ja) => "警告",
            (Self::Warn, Lang::En) => "warning",
        }
    }

    /// `[ラベル]`（表示言語）
    fn tag(self, lang: Lang) -> String {
        format!("[{}]", self.label_in(lang))
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
    /// `--json` だけに載せる項目固有の欄（#1623。行へ畳んだものの全件など）。人へ出す行には出ない
    pub fields: serde_json::Map<String, Value>,
}

impl Item {
    /// `  [ラベル] 要約` の 1 行だけを持つ項目（ラベルは表示言語 = #2365）
    pub fn new(
        key: impl Into<String>,
        status: Status,
        summary: impl Into<String>,
        lang: Lang,
    ) -> Self {
        let summary = summary.into();
        Self {
            key: key.into(),
            status,
            lines: vec![format!("  {} {summary}", status.tag(lang))],
            summary,
            remaining: Vec::new(),
            fields: serde_json::Map::new(),
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
            fields: serde_json::Map::new(),
        }
    }

    /// 続きの行を足す（字下げは呼び手が決める）
    pub fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }

    /// 改行で区切った複数の続きの行を足す
    fn lines_of(mut self, text: &str) -> Self {
        self.lines.extend(text.lines().map(str::to_string));
        self
    }

    /// 残り作業を足す
    pub fn remaining(mut self, remaining: Remaining) -> Self {
        self.remaining.push(remaining);
        self
    }

    /// `--json` だけに載せる欄を足す（#1623）。共通の欄（`key` / `status` / `summary` / `lines` /
    /// `remaining`）と同じ名前は上書きしない
    fn field(mut self, name: &str, value: Value) -> Self {
        self.fields.insert(name.to_string(), value);
        self
    }

    pub fn to_json(&self) -> Value {
        self.to_json_in(Lang::Ja)
    }

    /// 項目の JSON（残り作業の見出しは表示言語。#2365）
    pub fn to_json_in(&self, lang: Lang) -> Value {
        let mut value = json!({
            "key": self.key,
            "status": self.status.as_str(),
            "summary": self.summary,
            "lines": self.lines,
            "remaining": self.remaining.iter().map(|r| remaining_json(r, lang)).collect::<Vec<_>>(),
        });
        if let Value::Object(map) = &mut value {
            for (name, field) in &self.fields {
                map.entry(name.clone()).or_insert_with(|| field.clone());
            }
        }
        value
    }
}

/// 残り作業 1 件を JSON へ（`--check` の末尾と同じ材料を機械可読にする）
fn remaining_json(item: &Remaining, lang: Lang) -> Value {
    json!({
        "title": item.kind.title_in(lang),
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
    /// 項目の文を組んだ表示言語（#2365。残り作業の見出しも同じ言語で出す）
    pub lang: Lang,
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
        let lang = self.lang;
        let remaining = self.remaining();
        json!({
            "items": self.items.iter().map(|item| item.to_json_in(lang)).collect::<Vec<_>>(),
            "keys": self.keys(),
            "remaining": crate::setup_remaining::summarize(remaining)
                .iter()
                .map(|r| remaining_json(r, lang))
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
/// 文は呼んだ時点の表示言語（`tako_core::i18n::lang()`）で組む（#2365）。GUI（dispatch）はそのまま、
/// GUI を通さない CLI（届かないときの check-health・`tako setup --check`）は呼ぶ前に表示言語を決める。
///
/// 重い（エージェント CLI へ問い合わせる）ので、UI スレッドから呼ばないこと。
/// 外部プロセスを起こす問い合わせは重ねて走らせる（#1622。[`collect_with`]）。
///
/// 問い合わせは**呼び出しの形**（`|| f()`）で書く。`f` を値で渡すと字面の呼び出しグラフ
/// （#2255 の番犬 `crates/tako-control/tests/issue2255_diagnostics_auth_probe_watchdog.rs`）が
/// ここから先を辿れなくなり、問い合わせの先で上限なしの口を呼んでも見逃す
#[allow(clippy::redundant_closure)]
pub fn collect() -> Report {
    let lang = tako_core::i18n::lang();
    // #2255: 認証の問い合わせは上限つきで、系統を並べて聞く（返らない CLI 1 つで診断ごと
    // 固まらないため）。A/B の旧経路は setup と同じ上限なしの 1 系統ずつ
    let bootstrap = || {
        if tako_core::probe::issue2255_legacy() {
            crate::setup_bootstrap::status_all()
        } else {
            crate::setup_bootstrap::status_all_within(tako_core::probe::auth_probe_timeout())
        }
    };
    let probes = Probes {
        bootstrap: &bootstrap,
        claude_mcp: &|| crate::agent_mcp::claude_health_now(),
        codex_mcp: &|| crate::agent_mcp::state(crate::orchestrator::agent::WorkerAgent::Codex),
        agy_mcp: &|| crate::agent_mcp::state(crate::orchestrator::agent::WorkerAgent::Agy),
        deps: &|| dep_items(lang),
        config_share: &|| config_share_item(lang),
        tako_cli_path: &|| tako_cli_path_item(lang),
    };
    collect_with(&probes, probe_concurrency(issue1622_legacy()), lang)
}

// --- 外部の問い合わせを重ねる（#1622） ----------------------------------------

/// 導入状況（3 系統）。`setup_bootstrap::status_all` / `status_all_within` の答えの形
type BootstrapList = Vec<(
    AgentKind,
    Result<crate::setup_bootstrap::BootstrapState, String>,
)>;

/// `claude mcp list` の答え（登録済みか・登録パスが生きているか・打ち切りの知らせ）
type ClaudeMcp = (bool, bool, Option<tako_core::probe::TimeoutNotice>);

/// 外部プロセスを起こす問い合わせの口（#1622）。
///
/// 項目の組み立て（[`assemble`]）はここで採った答えだけを読み、問い合わせを 1 つも起こさない
/// （判定は今まで通りの関数が持つ = 重ねても 1 マスも変わらない）。本番は [`collect`] が
/// 実物を差し、テストは偽の口を差して重なりと同時数を観測する（本物の CLI を起こさない）
struct Probes<'a> {
    /// 導入状況。呼び出し元のスレッドで採る（系統どうしは `status_all_within` が並べる = #2255）
    bootstrap: &'a (dyn Fn() -> BootstrapList + Sync),
    claude_mcp: &'a (dyn Fn() -> ClaudeMcp + Sync),
    codex_mcp: &'a (dyn Fn() -> crate::agent_mcp::McpState + Sync),
    agy_mcp: &'a (dyn Fn() -> crate::agent_mcp::McpState + Sync),
    /// 導入状況に依らない項目は、問い合わせから項目の組み立てまで背景で済ませる
    /// （`dep_items` / `config_share_item` / `tako_cli_path_item` をそのまま呼ぶ）
    deps: &'a (dyn Fn() -> Vec<Item> + Sync),
    config_share: &'a (dyn Fn() -> Item + Sync),
    tako_cli_path: &'a (dyn Fn() -> Item + Sync),
}

/// 背景で採った答え（導入状況を除く）
struct Answers {
    claude_mcp: ClaudeMcp,
    codex_mcp: crate::agent_mcp::McpState,
    agy_mcp: crate::agent_mcp::McpState,
    deps: Vec<Item>,
    config_share: Item,
    tako_cli_path: Item,
}

/// 背景で外部の問い合わせを同時に走らせる上限（#1622）。
///
/// 呼び出し元のスレッドは導入状況（系統ごとに 1 本ずつ並べる = #2255）を採るので、同時に走る
/// 子プロセスは最大でこの数 + 系統数。ログインシェル（`exe::find`）と CLI を一度に起こしすぎて、
/// 遅い機で個々の上限（ログインシェルは既定 5 秒）に掛かりやすくならないよう抑える。
/// 実測（3 系統の Mac）では一番長い `claude mcp list`（5.7 秒）の間に残り（計 1.6 秒）が
/// もう 1 本で片付くので 2 本で足りる。遅い機（依存の検出が長い等）の余裕を見て 3 本
const PROBE_CONCURRENCY: usize = 3;

/// 背景の同時数。A/B の旧経路は 0 = 重ねない（導入状況の後に 1 本ずつ）
fn probe_concurrency(legacy: bool) -> usize {
    if legacy {
        0
    } else {
        PROBE_CONCURRENCY
    }
}

/// A/B（#1622）: `TAKO_1622_LEGACY=1` で外部の問い合わせを重ねず、導入状況の後に 1 本ずつ採る
/// （問い合わせの集合と項目の組み立ては同じ = 出力は一致する）。**ゲートはこの 1 か所**
fn issue1622_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1622_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 外部の問い合わせを重ねて採り、今まで通りの並びで項目を組む（#1622）。
///
/// 背景の問い合わせは**長い順に始める**（`claude mcp list` は登録済みの MCP サーバへ 1 台ずつ
/// 繋ぐので一番長い = 実測 5.7 秒）。MCP の問い合わせは導入状況を待たずに始めるが、項目は今まで
/// 通り「導入済みの系統だけ」出す（[`mcp_items`] の判定。未導入の系統の答えは捨てる）。
/// 答えは種類ごとの置き場へ入るので、終わった順は項目の並びに響かない
fn collect_with(probes: &Probes<'_>, concurrency: usize, lang: Lang) -> Report {
    let claude_mcp = Slot::new();
    let codex_mcp = Slot::new();
    let agy_mcp = Slot::new();
    let deps = Slot::new();
    let config_share = Slot::new();
    let tako_cli_path = Slot::new();
    let jobs: Vec<Job<'_>> = vec![
        Box::new(|| claude_mcp.put((probes.claude_mcp)())),
        Box::new(|| deps.put((probes.deps)())),
        Box::new(|| config_share.put((probes.config_share)())),
        Box::new(|| codex_mcp.put((probes.codex_mcp)())),
        Box::new(|| tako_cli_path.put((probes.tako_cli_path)())),
        Box::new(|| agy_mcp.put((probes.agy_mcp)())),
    ];
    let bootstrap = overlap(concurrency, jobs, probes.bootstrap);
    let answers = Answers {
        claude_mcp: claude_mcp.take(),
        codex_mcp: codex_mcp.take(),
        agy_mcp: agy_mcp.take(),
        deps: deps.take(),
        config_share: config_share.take(),
        tako_cli_path: tako_cli_path.take(),
    };
    assemble(bootstrap, answers, lang)
}

/// 背景で走らせる問い合わせ 1 本（答えは自分の [`Slot`] へ置く）
type Job<'a> = Box<dyn FnOnce() + Send + 'a>;

/// `here` を呼び出し元のスレッドで走らせながら、`jobs` を並びの順に最大 `limit` 本のスレッドで
/// 走らせる（#1622）。並びの先頭ほど先に始まる。全部が終わってから `here` の答えを返す。
///
/// `limit == 0` は重ねない（`here` の後に 1 本ずつ = A/B の旧経路）。どれかが panic したら
/// 全部を待ってから panic する（逐次のときと同じく呼び手へ伝わる）
fn overlap<R>(limit: usize, jobs: Vec<Job<'_>>, here: impl FnOnce() -> R) -> R {
    if limit == 0 {
        let out = here();
        jobs.into_iter().for_each(|job| job());
        return out;
    }
    let workers = limit.min(jobs.len());
    let queue = std::sync::Mutex::new(jobs.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                // 錠は次の 1 本を取り出す間だけ持つ（走らせている間は持たない）
                let next = queue.lock().ok().and_then(|mut queue| queue.next());
                let Some(job) = next else { break };
                job();
            });
        }
        here()
    })
}

/// 背景の問い合わせが答えを 1 度だけ置く場所
struct Slot<T>(std::sync::Mutex<Option<T>>);

impl<T> Slot<T> {
    fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    fn put(&self, value: T) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(value);
        }
    }

    /// [`overlap`] が返った後に読む。問い合わせが panic したら `overlap` が先に panic するので、
    /// ここへ来たときは必ず置かれている
    fn take(self) -> T {
        self.0
            .into_inner()
            .ok()
            .flatten()
            .expect("背景の問い合わせが答えを置いている（#1622）")
    }
}

/// 採った答えから項目を組む（**問い合わせを 1 つも起こさない**。並びと判定は #1622 の前と同じ）
fn assemble(bootstrap: BootstrapList, answers: Answers, lang: Lang) -> Report {
    let mut items = Vec::new();
    let mut probe_timeouts = Vec::new();
    probe_timeouts.extend(
        bootstrap
            .iter()
            .filter_map(|(_, state)| state.as_ref().ok()?.auth_timeout.clone()),
    );
    // #1532 / #1533: ログインシェル（実行ファイルの検出・PATH の読み取り）の打ち切りも載せる。
    // PATH の読み取りは 3 系統とも同じ問い合わせなので、同じ名札は 1 件にまとめる
    for (_, state) in bootstrap.iter() {
        let Ok(state) = state else { continue };
        for notice in [&state.locate_timeout, &state.path_timeout]
            .into_iter()
            .flatten()
        {
            if !probe_timeouts.contains(notice) {
                probe_timeouts.push(notice.clone());
            }
        }
    }
    items.extend(agent_cli_items(&bootstrap, lang));
    items.push(answers.tako_cli_path);
    items.push(shell_integration_item(lang));
    items.push(agents_detected_item(&bootstrap, lang));
    items.extend(answers.deps);
    items.extend(fda_item(lang));
    items.extend(webview2_item(
        &tako_core::platform::webview2::runtime(),
        lang,
    ));
    let (mcp, mcp_timeouts) = mcp_items(
        &bootstrap,
        answers.claude_mcp,
        &answers.codex_mcp,
        &answers.agy_mcp,
        lang,
    );
    items.extend(mcp);
    probe_timeouts.extend(mcp_timeouts);
    items.extend(agent_role_items(&bootstrap, lang));
    items.extend(setup_items(lang));
    items.extend(instruction_items(&bootstrap, lang));
    items.push(agents_sync_item(lang));
    items.extend(sleep_guard_items(lang));
    items.push(answers.config_share);
    items.push(profiles_item(lang));
    items.push(remote_item(lang));
    items.push(ipc_item(lang));
    Report {
        items,
        probe_timeouts,
        lang,
    }
}

// --- エージェント CLI のゼロスタート導入（#868 / #989） ----------------------

type BootstrapStates = [(
    AgentKind,
    Result<crate::setup_bootstrap::BootstrapState, String>,
)];

/// 認証を確かめられなかった（問い合わせを上限で打ち切った）系統の 1 句（#2255）。
/// 未認証と言うと「ログインし直す」へ人を送ってしまうので、**不明**と言い分ける
fn auth_unknown(state: &crate::setup_bootstrap::BootstrapState, lang: Lang) -> Option<String> {
    state.auth_timeout.as_ref().map(|notice| {
        HealthText::AuthUnknown {
            secs: notice.waited_secs,
        }
        .text_in(lang)
    })
}

/// 認証が不明なせいで段が「ログイン」に見えているか（段の判定は認証済みかどうかしか見ない）
fn auth_step_unknown(state: &crate::setup_bootstrap::BootstrapState, lang: Lang) -> Option<String> {
    (state.step == crate::setup_bootstrap::Step::Auth)
        .then(|| auth_unknown(state, lang))
        .flatten()
}

/// 段が「確かめられなかった」せいで決まっているときの 1 句。認証（#2255）に加えて、
/// ログインシェルの打ち切り（実行ファイルの検出 = #1533・PATH の読み取り = #1532）で
/// 「未導入」「PATH に無い」に見えている段も**不明**と言い分ける（導入や PATH の設定へ人を送らない）
fn step_unknown(state: &crate::setup_bootstrap::BootstrapState, lang: Lang) -> Option<String> {
    use crate::setup_bootstrap::Step;
    if let Some(notice) = state.step_unconfirmed() {
        return Some(
            HealthText::StepUnknown {
                install: state.step == Step::Install,
                secs: notice.waited_secs,
            }
            .text_in(lang),
        );
    }
    auth_step_unknown(state, lang)
}

/// 導入・PATH・ログインの段（`tako setup bootstrap status-all` と同じ材料）
fn agent_cli_items(states: &BootstrapStates, lang: Lang) -> Vec<Item> {
    use crate::setup_bootstrap::Step;

    let ready: Vec<&str> = states
        .iter()
        .filter(|(_, state)| state.as_ref().is_ok_and(|s| s.step == Step::Ready))
        .map(|(agent, _)| agent.as_str())
        .collect();
    // ready が無く、認証（#2255）・導入・PATH（#1532 / #1533）を確かめられなかった系統がある。
    // 使えるかどうか分からないので「使える系統がありません」とも、導入やログインへの道とも言わない
    let unknown: Vec<String> = states
        .iter()
        .filter_map(|(agent, state)| {
            let text = step_unknown(state.as_ref().ok()?, lang)?;
            Some(
                HealthText::AgentUnknown {
                    agent: agent.as_str().to_string(),
                    text,
                }
                .text_in(lang),
            )
        })
        .collect();
    // **1 つでも ready なら tako は使える**ので、そこを最初に言う
    let mut head = if ready.is_empty() && !unknown.is_empty() {
        Item::new(
            "agent_cli",
            Status::Warn,
            HealthText::AgentCliUnconfirmed(unknown.join(" / ")).text_in(lang),
            lang,
        )
    } else if ready.is_empty() {
        Item::new(
            "agent_cli",
            Status::Missing,
            HealthText::AgentCliNone.text_in(lang),
            lang,
        )
        .line(HealthText::AgentCliNoneHint.text_in(lang))
    } else {
        Item::new(
            "agent_cli",
            Status::Ok,
            HealthText::AgentCliReady(ready.join(" / ")).text_in(lang),
            lang,
        )
    };
    // 1 つも使える系統が無いときだけ、**一番短い道**を残り作業にする
    if ready.is_empty() && unknown.is_empty() {
        if let Some(remaining) = shortest_bootstrap_remaining(states, lang) {
            head = head.remaining(remaining);
        }
    }

    // 系統ごとの行は字下げを深くして親の項目の下に並べる
    let sub_item = |key: String, status: Status, summary: String| {
        let line = format!("         {} {summary}", status.tag(lang));
        Item::from_text(key, status, summary, &line)
    };
    let mut items = vec![head];
    for (agent, state) in states {
        let key = format!("agent_cli.{}", agent.as_str());
        let name = agent.as_str().to_string();
        items.push(match state {
            Ok(state) if state.step == Step::Ready => {
                sub_item(key, Status::Ok, HealthText::AgentDone(name).text_in(lang))
            }
            Ok(state) => match step_unknown(state, lang) {
                // 確かめられなかった（#2255 / #1532 / #1533）。「ログイン未完了」「未導入」と言うと
                // 人をログインや導入へ送ってしまう
                Some(text) => sub_item(key, Status::Warn, format!("{name}: {text}")),
                None => sub_item(
                    key,
                    Status::Missing,
                    format!(
                        "{name}: {} ({})",
                        state.step_description_in(lang),
                        state.step.as_str()
                    ),
                ),
            },
            Err(e) => sub_item(
                key,
                Status::Warn,
                HealthText::AgentStatusUnreadable {
                    agent: name,
                    error: e.clone(),
                }
                .text_in(lang),
            ),
        });
    }
    items
}

/// 使える系統が 1 つも無いときに人が進む**一番短い道**。
///
/// どの系統を仕上げるかの判断は `setup_bootstrap::choose_target` の 1 実装
/// （`tako setup` の bootstrap 段が実際に面倒を見る系統と同じもの）
fn shortest_bootstrap_remaining(states: &BootstrapStates, lang: Lang) -> Option<Remaining> {
    if states.is_empty() {
        return None;
    }
    let target = crate::setup_bootstrap::choose_target(states);
    match states
        .iter()
        .find(|(agent, _)| *agent == target)
        .map(|(_, state)| state)
    {
        // 種別の分からないシェルの PATH の段は、`tako setup bootstrap path` へ誘導せず（同じ断りで
        // 止まる）、その断りの全文（足すディレクトリの一覧）を理由の行に載せる（#2564）
        Some(Ok(state)) => RemainingKind::for_step_by_hand(
            state.step,
            target,
            crate::setup_bootstrap::path_by_hand(state),
        )
        .map(|kind| match kind {
            // 診断は check-health と `--check` が同じ文を出す正本（#1505）。どちらも表示言語で組む（#2365）
            RemainingKind::AgentPathByHand(agent) => {
                match crate::setup_bootstrap::path_by_hand_refusal() {
                    Some(refusal) => Remaining::path_by_hand(agent, &refusal, lang),
                    None => Remaining::new_in(kind, lang),
                }
            }
            kind => Remaining::new_in(kind, lang),
        })
        .map(|item| item.noting_state_in(state, lang)),
        Some(Err(e)) => Some(Remaining::with_detail(
            RemainingKind::AgentStatusUnknown,
            vec![e.clone()],
        )),
        None => None,
    }
}

/// 検出できた CLI の一覧（パス / 認証 / プラン）。判定は `status_all` の 1 実装から引く
fn agents_detected_item(states: &BootstrapStates, lang: Lang) -> Item {
    let detected: Vec<(AgentKind, &crate::setup_bootstrap::BootstrapState)> = states
        .iter()
        .filter_map(|(agent, state)| state.as_ref().ok().map(|s| (*agent, s)))
        .filter(|(_, state)| state.binary.is_some())
        .collect();
    // 実行ファイルの検出を上限で打ち切った系統（#1533）。見つからなかったのではなく確かめられなかった
    let unconfirmed: Vec<(AgentKind, u64)> = states
        .iter()
        .filter_map(|(agent, state)| {
            let state = state.as_ref().ok()?;
            let notice = state.locate_timeout.as_ref()?;
            state
                .binary
                .is_none()
                .then_some((*agent, notice.waited_secs))
        })
        .collect();
    let heading = HealthText::AgentsHeading.text_in(lang);
    if detected.is_empty() && !unconfirmed.is_empty() {
        let names: Vec<&str> = unconfirmed.iter().map(|(a, _)| a.as_str()).collect();
        let secs = unconfirmed
            .iter()
            .map(|(_, s)| *s)
            .max()
            .unwrap_or_default();
        let text = HealthText::AgentsUnconfirmed {
            names: names.join(" / "),
            secs,
        }
        .text_in(lang);
        return Item::from_text(
            "agents_detected",
            Status::Warn,
            text.clone(),
            &format!("{heading}\n    {} {text}", Status::Warn.tag(lang)),
        )
        .line(format!(
            "      {}",
            HealthText::LoginShellWaitHint.text_in(lang)
        ));
    }
    if detected.is_empty() {
        let text = HealthText::AgentsNoneFound.text_in(lang);
        let mut item = Item::from_text(
            "agents_detected",
            Status::Missing,
            text.clone(),
            &format!("{heading}\n    {} {text}", Status::Missing.tag(lang)),
        );
        for agent in crate::setup_bootstrap::bootstrap_agents() {
            item = item.line(format!(
                "      {}: {}",
                agent.as_str(),
                install_hint(agent, lang)
            ));
        }
        return item.remaining(Remaining::new_in(RemainingKind::AgentMissing, lang));
    }
    let names: Vec<&str> = detected.iter().map(|(agent, _)| agent.as_str()).collect();
    let mut item = Item::from_text(
        "agents_detected",
        Status::Detected,
        HealthText::AgentsDetected(names.join(" / ")).text_in(lang),
        &heading,
    );
    for (agent, state) in detected {
        let path = state
            .binary
            .as_deref()
            .map(tako_core::paths::shorten_home)
            .unwrap_or_default();
        // 認証を確かめられなかったら、プランも同じ問い合わせから読むので並べない（#2255）
        let auth = auth_unknown(state, lang).unwrap_or_else(|| {
            let auth = if state.authenticated {
                HealthText::SignedIn
            } else {
                HealthText::SignedOut
            }
            .text_in(lang);
            let plan = state
                .account_plan
                .clone()
                .unwrap_or_else(|| HealthText::PlanUnknown.text_in(lang));
            format!("{auth} / {plan}")
        });
        item = item.line(
            HealthText::AgentDetectedLine {
                agent: agent.as_str().to_string(),
                path,
                auth,
            }
            .text_in(lang),
        );
    }
    for (agent, secs) in unconfirmed {
        item = item.line(
            HealthText::AgentUnconfirmedLine {
                agent: agent.as_str().to_string(),
                secs,
            }
            .text_in(lang),
        );
    }
    item
}

/// 導入の案内 1 行（正本は境界 B17 = `platform::agent_install`。#989 / #322）
fn install_hint(agent: AgentKind, lang: Lang) -> String {
    let guidance = crate::orchestrator::agent_cli::guidance(agent.into());
    match (guidance.command, guidance.docs_url) {
        (Some(cmd), _) => cmd.to_string(),
        (None, Some(url)) => url.to_string(),
        (None, None) => guidance
            .manual
            .map(|m| m.text_in(lang).to_string())
            .unwrap_or_default(),
    }
}

/// master を任せられるか / worker 専用かの申告（#989）
fn agent_role_items(states: &BootstrapStates, lang: Lang) -> Vec<Item> {
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
            HealthText::RoleCodex.text_in(lang),
            lang,
        ));
    }
    if installed(AgentKind::Agy) {
        items.push(Item::new(
            "agent_role.agy",
            Status::Info,
            HealthText::RoleAgy.text_in(lang),
            lang,
        ));
    }
    items
}

// --- tako CLI の PATH（#1502）/ シェル統合（#1504） -------------------------

fn tako_cli_path_item(lang: Lang) -> Item {
    let text = crate::setup_bootstrap::tako_cli_path_check_line(lang);
    let status = status_from_line(&text);
    Item::from_text("tako_cli_path", status, summary_of(&text), &text)
}

fn shell_integration_item(lang: Lang) -> Item {
    let text = crate::shell_integration::check_line(lang);
    let status = status_from_line(&text);
    let mut item = Item::from_text("shell_integration", status, summary_of(&text), &text);
    if let Some(remaining) = crate::shell_integration::check_remaining() {
        item = item.remaining(remaining);
    }
    item
}

/// 他所が組んだ 1 行目の `[ラベル]` から重さを読む（文面の正本を 2 つにしないため）。
/// ラベルは日英のどちらでも読む（行は表示言語で組まれる = #2365）
fn status_from_line(text: &str) -> Status {
    let head = text.lines().next().unwrap_or_default();
    for status in Status::ALL {
        for lang in [Lang::Ja, Lang::En] {
            if head.contains(&status.tag(lang)) {
                return status;
            }
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
fn dep_items(lang: Lang) -> Vec<Item> {
    crate::setup_deps::status()
        .into_iter()
        .map(|state| {
            let dep = state.dep;
            let key = format!("dep.{}", dep.bin);
            // 在るが動かない（tailscale の `--version` が固まる / 非 0。#1816）= 壊れている。
            // 任意依存なので残り作業には数えない（`tako setup` の依存段と同じ文面・同じ扱い）
            if let Some(text) = state.unrunnable_text_in(lang) {
                return Item::new(key, Status::Warn, text, lang).line(format!(
                    "      {}",
                    crate::tailscale::unrunnable_hint_in(lang)
                ));
            }
            if let Some(path) = &state.found {
                return Item::new(key, Status::Ok, format!("{}: {path}", dep.bin), lang);
            }
            // 検出を上限で打ち切った（#1533）。「見つかりません」とも導入の案内とも言わない
            if let Some(text) = state.unconfirmed_text_in(lang) {
                return Item::new(key, Status::Warn, text, lang).line(format!(
                    "      {}",
                    HealthText::LoginShellWaitHint.text_in(lang)
                ));
            }
            let mark = if dep.required {
                Status::Missing
            } else {
                Status::Optional
            };
            let summary = HealthText::DepNotFound {
                bin: dep.bin.to_string(),
                required: dep.required,
            }
            .text_in(lang);
            let mut item = Item::new(key, mark, summary, lang)
                .line(HealthText::DepPurpose(dep.purpose.text_in(lang).to_string()).text_in(lang));
            if !dep.required {
                item = item.line(HealthText::DepOptionalNote.text_in(lang));
            }
            for line in dep_guide_lines(&state, lang) {
                item = item.line(line);
            }
            if dep.required {
                item = item.remaining(Remaining::new_in(
                    RemainingKind::Dep {
                        bin: dep.bin.to_string(),
                    },
                    lang,
                ));
            }
            item
        })
        .collect()
}

/// 読み取りだけの文脈で `setup_deps` が出す案内行（**導入は 1 つも起こさない**）
fn dep_guide_lines(state: &crate::setup_deps::DepStatus, lang: Lang) -> Vec<String> {
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
            lang,
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
fn fda_item(lang: Lang) -> Vec<Item> {
    if crate::fda::is_granted() {
        return vec![Item::new(
            "fda",
            Status::Ok,
            HealthText::FdaGranted.text_in(lang),
            lang,
        )];
    }
    vec![Item::new(
        "fda",
        Status::Optional,
        HealthText::FdaNotGranted.text_in(lang),
        lang,
    )
    .lines_of(&HealthText::FdaNotGrantedDetail.text_in(lang))]
}

#[cfg(not(target_os = "macos"))]
fn fda_item(_lang: Lang) -> Vec<Item> {
    Vec::new()
}

// --- WebView2 ランタイム（Windows の Web ビュー。#2431） ------------------------

/// WebView2 ランタイムの版（Windows だけ。macOS は対象外 = 項目を出さない）。
/// 判定と文言は `platform::webview2` の 1 実装（check-health の `webview2` 節と同じ材料）
fn webview2_item(runtime: &tako_core::platform::webview2::Runtime, lang: Lang) -> Option<Item> {
    let summary = runtime.summary_in(lang)?;
    let status = match runtime.state() {
        "missing" => Status::Missing,
        "outdated" => Status::Warn,
        _ => Status::Ok,
    };
    let item = Item::new("webview2", status, summary, lang);
    Some(match runtime.next_step_in(lang) {
        Some(step) => item.line(format!("      {step}")),
        None => item,
    })
}

// --- MCP 登録（#979） -------------------------------------------------------

/// 問い合わせの答え（`claude` = `agent_mcp::claude_health_now`・`codex` / `agy` = `agent_mcp::state`）は
/// 導入状況を待たずに背景で採ったもの（#1622）。**未導入の系統の答えはここで捨てる**（項目の集合は
/// 今まで通り導入状況だけで決まる）
fn mcp_items(
    states: &BootstrapStates,
    claude: ClaudeMcp,
    codex: &crate::agent_mcp::McpState,
    agy: &crate::agent_mcp::McpState,
    lang: Lang,
) -> (Vec<Item>, Vec<tako_core::probe::TimeoutNotice>) {
    use crate::agent_mcp::McpState;
    use crate::orchestrator::agent::WorkerAgent;

    let installed = |kind: AgentKind| {
        states.iter().any(|(agent, state)| {
            *agent == kind && state.as_ref().is_ok_and(|s| s.binary.is_some())
        })
    };
    let text = |t: HealthText| t.text_in(lang);
    let mut items = Vec::new();
    let mut timeouts = Vec::new();
    // claude だけ登録の読み方が違う（`dispatch::setup_mcp` が書き手なので
    // `agent_mcp` は面倒を見ない = `handled_here` が false）
    if installed(AgentKind::Claude) {
        // 打ち切りの知らせも受け取る（#1503。確認できなかったことを黙らせない）
        let (registered, healthy, notice) = claude;
        let key = "mcp.claude";
        let name = || "Claude".to_string();
        // 問い合わせ（実行ファイルの検出 = #1533 / `claude mcp list` = #1503）を打ち切って
        // 登録を読めなかった。未登録ではなく確かめられなかったので、登録し直しへ人を送らない
        let unconfirmed = notice.clone().filter(|_| !registered);
        timeouts.extend(notice);
        let item = if registered && healthy {
            Item::new(
                key,
                Status::Ok,
                text(HealthText::McpRegistered(name())),
                lang,
            )
        } else if registered {
            let mut item = Item::new(
                key,
                Status::Warn,
                text(HealthText::McpPathGone(name())),
                lang,
            );
            if let Some(cmd) = crate::agent_mcp::claude_registered_command() {
                item = item.line(text(HealthText::McpRegisteredPath(cmd)));
            }
            item.line(text(HealthText::McpRepairHint))
        } else if let Some(n) = &unconfirmed {
            Item::new(
                key,
                Status::Warn,
                text(HealthText::McpClaudeUnconfirmed {
                    label: n.label.clone(),
                    secs: n.waited_secs,
                }),
                lang,
            )
        } else {
            Item::new(
                key,
                Status::Missing,
                text(HealthText::McpNotRegistered(name())),
                lang,
            )
        };
        items.push(if (registered && healthy) || unconfirmed.is_some() {
            item
        } else {
            item.remaining(Remaining::new_in(
                RemainingKind::Mcp {
                    agent: "claude".to_string(),
                },
                lang,
            ))
        });
    }
    for (kind, agent, state) in [
        (AgentKind::Codex, WorkerAgent::Codex, codex),
        (AgentKind::Agy, WorkerAgent::Agy, agy),
    ] {
        if !installed(kind) {
            continue;
        }
        let name = agent.as_str();
        let key = format!("mcp.{name}");
        let mut item = match state {
            McpState::Ready { .. } => Item::new(
                key,
                Status::Ok,
                text(HealthText::McpRegistered(name.to_string())),
                lang,
            ),
            McpState::Dead { command } => Item::new(
                key,
                Status::Warn,
                text(HealthText::McpPathGone(name.to_string())),
                lang,
            )
            .line(text(HealthText::McpRegisteredPath(command.clone())))
            .line(text(HealthText::McpRepairHint)),
            // 登録行はあるのに env の転送が無い = ツールが 0 個になる（#979 の実測）。
            // 「未登録」と混ぜると原因を追えないので別の文言にする
            McpState::EnvMissing { .. } => Item::new(
                key,
                Status::Warn,
                text(HealthText::McpEnvMissing(name.to_string())),
                lang,
            )
            .line(text(HealthText::McpNoTools))
            .line(text(HealthText::McpReinstall(name.to_string()))),
            McpState::NotRegistered => Item::new(
                key,
                Status::Missing,
                text(HealthText::McpNotRegistered(name.to_string())),
                lang,
            ),
            // CLI は検出できているので Unknown は「一覧を読めなかった」だけ
            McpState::Unknown => Item::new(
                key,
                Status::Warn,
                text(HealthText::McpUnknown(name.to_string())),
                lang,
            ),
        };
        if state.describe_gap().is_some() {
            item = item.remaining(Remaining::new_in(
                RemainingKind::Mcp {
                    agent: name.to_string(),
                },
                lang,
            ));
        }
        items.push(item);
    }
    (items, timeouts)
}

// --- セットアップ本体と追従（#94） ------------------------------------------

fn setup_items(lang: Lang) -> Vec<Item> {
    let text = |t: HealthText| t.text_in(lang);
    let Ok(config_path) = crate::setup::config_yaml_path() else {
        return vec![
            Item::new("setup", Status::Info, text(HealthText::ConfigNoHome), lang),
            update_item_unapplied(lang),
        ];
    };
    if !config_path.is_file() {
        return vec![
            Item::new(
                "setup",
                Status::Info,
                text(HealthText::ConfigNotCreated),
                lang,
            ),
            update_item_unapplied(lang),
        ];
    }
    // 読めない理由も表示言語で（型の口 = `SetupRefusal`。日本語は従来の `load_config` と同じ文）
    let config = match crate::setup::load_config_typed() {
        Ok(config) => config,
        Err(e) => {
            return vec![
                Item::new(
                    "setup",
                    Status::Warn,
                    text(HealthText::ConfigUnreadable(e.text_in(lang))),
                    lang,
                ),
                update_item_unapplied(lang),
            ]
        }
    };
    if !config.setup.completed {
        return vec![
            Item::new(
                "setup",
                Status::Info,
                text(HealthText::SetupIncomplete),
                lang,
            ),
            // **項目を落とさない**（#1505）: 「いま何リビジョンか」は未実施でも答えられる。
            // ここで消すと状態によって項目の集合が変わり、機械照合が成り立たない
            update_item_unapplied(lang),
        ];
    }
    let mut items = vec![Item::new(
        "setup",
        Status::Ok,
        text(HealthText::SetupDone(config.setup.completed_at.clone())),
        lang,
    )];
    // アップデート追従状況（Issue #94）
    items.push(
        match crate::setup::pending_changes(config.setup.applied_revision) {
            Ok(pending) if pending.is_empty() => Item::new(
                "update",
                Status::Ok,
                text(HealthText::UpdateLatest(config.setup.applied_revision)),
                lang,
            ),
            Ok(pending) => Item::new(
                "update",
                Status::Info,
                text(HealthText::UpdatePending(pending.len())),
                lang,
            ),
            Err(e) => Item::new(
                "update",
                Status::Warn,
                text(HealthText::UpdateUnknown(e)),
                lang,
            ),
        },
    );
    if let Some(agent) = config.setup.selected_agent.as_deref() {
        items.push(Item::new(
            "setup.agent",
            Status::Ok,
            text(HealthText::DefaultAgent(agent.to_string())),
            lang,
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
            text(HealthText::Plans(plans)),
            lang,
        ));
    }
    items
}

/// まだ setup を通していないときの追従行（**項目は落とさない**）
fn update_item_unapplied(lang: Lang) -> Item {
    match crate::setup::current_revision() {
        Ok(current) => Item::new(
            "update",
            Status::Info,
            HealthText::UpdateUnapplied(current).text_in(lang),
            lang,
        ),
        Err(e) => Item::new(
            "update",
            Status::Warn,
            HealthText::UpdateUnknown(e).text_in(lang),
            lang,
        ),
    }
}

// --- グローバル指示ファイル -------------------------------------------------

fn instruction_items(states: &BootstrapStates, lang: Lang) -> Vec<Item> {
    states
        .iter()
        .filter(|(_, state)| state.as_ref().is_ok_and(|s| s.binary.is_some()))
        .filter_map(|(agent, _)| {
            let path = instruction_path(*agent)?;
            let display = tako_core::paths::shorten_home(&path.to_string_lossy());
            let key = format!("instructions.{}", agent.as_str());
            Some(if path.is_file() {
                Item::new(
                    key,
                    Status::Ok,
                    HealthText::InstructionsExist(display).text_in(lang),
                    lang,
                )
            } else {
                Item::new(
                    key,
                    Status::Info,
                    HealthText::InstructionsMissing(display).text_in(lang),
                    lang,
                )
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

fn agents_sync_item(lang: Lang) -> Item {
    let key = "agents_sync";
    let item = |status: Status, t: HealthText| Item::new(key, status, t.text_in(lang), lang);
    match crate::agents_sync::status() {
        Ok(status) => {
            let st = status["status"].as_str().unwrap_or("unknown");
            match st {
                "not_configured" => item(Status::Info, HealthText::SyncNotConfigured),
                "up_to_date" => item(Status::Ok, HealthText::SyncUpToDate),
                "outdated" => item(Status::Info, HealthText::SyncOutdated),
                "source_missing" => {
                    let path = status["source_path"].as_str().unwrap_or("?");
                    item(
                        Status::Missing,
                        HealthText::SyncSourceMissing(path.to_string()),
                    )
                }
                other => item(Status::Warn, HealthText::SyncOther(other.to_string())),
            }
        }
        Err(e) => item(Status::Info, HealthText::SyncFailed(e)),
    }
}

// --- スリープ防止（#173 / #524） --------------------------------------------

fn sleep_guard_items(lang: Lang) -> Vec<Item> {
    use crate::sleep_guard::{LidSleepMode, SleepGuardMode};

    let settings = crate::settings::load();
    let mode = settings.sleep_guard_mode;
    let power = settings.sleep_guard_power;
    let mut items = vec![match mode {
        SleepGuardMode::Off => Item::new(
            "sleep_guard",
            Status::Info,
            HealthText::SleepOff.text_in(lang),
            lang,
        ),
        _ => Item::new(
            "sleep_guard",
            Status::Ok,
            HealthText::SleepOn {
                mode: mode.as_str().to_string(),
                power: power.as_str().to_string(),
            }
            .text_in(lang),
            lang,
        ),
    }];
    // 蓋閉じ継続を持たない OS では案内しない（#524）
    if crate::sleep_guard::lid_control_supported() {
        // 未完了かどうかだけを見る。手段（macOS の sudoers）は sleep_guard の内側（#697）
        let setup_pending = crate::sleep_guard::lid_setup_pending();
        let (status, text) = match settings.lid_sleep_mode {
            LidSleepMode::Off => (Status::Info, HealthText::LidOff),
            LidSleepMode::WhileAgentsRunning if setup_pending => {
                (Status::Missing, HealthText::LidSudoersMissing)
            }
            LidSleepMode::WhileAgentsRunning => (Status::Ok, HealthText::LidOn),
        };
        items.push(Item::new("lid_sleep", status, text.text_in(lang), lang));
    }
    items
}

// --- 設定共有（#513 / #793） ------------------------------------------------

fn config_share_item(lang: Lang) -> Item {
    let lines = config_share_lines_in(&crate::config_share::env::detect(), false, lang);
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
/// **質問は含まない**（#262 の質問ゼロ）。`verbose` = setup サマリ向けに説明行を足す。
/// 日本語（`tako setup` の要約）。`--check` / check-health は [`config_share_lines_in`]
pub fn config_share_lines(
    env: &crate::config_share::env::ShareEnvironment,
    verbose: bool,
) -> Vec<String> {
    config_share_lines_in(env, verbose, Lang::Ja)
}

/// [`config_share_lines`] を表示言語で（#2365）
pub fn config_share_lines_in(
    env: &crate::config_share::env::ShareEnvironment,
    verbose: bool,
    lang: Lang,
) -> Vec<String> {
    use crate::config_share::env::Guidance;

    let text = |t: HealthText| t.text_in(lang);
    let repo = env.repo.as_deref().unwrap_or("?").to_string();
    let next = env.next_command();
    let mut lines = Vec::new();
    match env.guidance() {
        // 配線済みなら状態を 1 行示すだけ。勧誘はしない（#793 受け入れ条件 4 = 冪等）
        Guidance::Linked => {
            lines.push(text(HealthText::ShareLinked(repo)));
            if verbose {
                lines.push(text(HealthText::ShareLinkedUsage));
            }
        }
        Guidance::Broken => {
            lines.push(text(HealthText::ShareBroken(repo)));
            lines.push(text(HealthText::ShareRelink(next.to_string())));
        }
        // 既に自力で共有している利用者には、まず相乗りを示す（二重管理を作らない）
        Guidance::AdoptExisting => {
            lines.push(text(HealthText::ShareUnlinked));
            for found in &env.external {
                lines.push(text(HealthText::ShareManaged {
                    path: found.path.to_string(),
                    repo: found.repo.to_string(),
                    same: found.same_place,
                }));
            }
            lines.push(text(HealthText::ShareAdopt(next.to_string())));
            lines.push(text(HealthText::ShareDuplicate));
        }
        Guidance::Fresh => {
            lines.push(text(HealthText::ShareFresh(next.to_string())));
            if verbose {
                lines.push(text(HealthText::ShareFreshWhat));
                lines.push(text(HealthText::ShareFreshSafe));
            }
        }
    }
    lines
}

// --- プロファイル -----------------------------------------------------------

/// 1 行に名前で並べるプロファイルの数（#1623）。残りは「他 N 個」へ畳み、全件は `--json` の `names` に置く
const PROFILES_SHOWN: usize = 3;

/// A/B（#1623）: `TAKO_1623_LEGACY=1` で登録済みの名前を 1 行に全部並べる（`names` も載せない）
fn issue1623_legacy() -> bool {
    matches!(
        std::env::var("TAKO_1623_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

fn profiles_item(lang: Lang) -> Item {
    // 一覧の失敗の理由は型の口で受け、行と同じ表示言語で組む（#2365。日本語固定の
    // `list_profiles()` だと英語表示でも括弧の中だけ日本語のまま混ざった）
    profiles_item_from(
        crate::orchestrator::list_profiles_of_typed(crate::orchestrator::ProfileKind::Master),
        lang,
        issue1623_legacy(),
    )
}

/// プロファイルの項目を一覧の結果から組む（#1623）。
///
/// #1623 前は登録済みの名前を `, ` で 1 行に全部並べていた（84 個の砂場で 1 行 449 字。
/// `check-health --json` の `lines` にも同じ 1 行が載る）。いまは件数・先頭
/// [`PROFILES_SHOWN`] 個・「他 N 個」の 1 行にし、全件は `names`（名前順）へ置く
fn profiles_item_from(
    listed: Result<Vec<String>, crate::orchestrator::ConfigError>,
    lang: Lang,
    legacy: bool,
) -> Item {
    const KEY: &str = "profiles";
    let names = match listed {
        Ok(names) => names,
        Err(e) => {
            return Item::new(
                KEY,
                Status::Info,
                HealthText::ProfilesFailed(e.text_in(lang)).text_in(lang),
                lang,
            )
        }
    };
    if names.is_empty() {
        let item = Item::new(
            KEY,
            Status::Info,
            HealthText::ProfilesNone.text_in(lang),
            lang,
        );
        return if legacy {
            item
        } else {
            item.field("names", json!(names))
        };
    }
    if legacy {
        let count = HealthText::ProfilesCount {
            count: names.len(),
            shown: names.join(", "),
            rest: 0,
        };
        return Item::new(KEY, Status::Ok, count.text_in(lang), lang);
    }
    let (shown, rest) = profiles_shown(&names);
    let count = HealthText::ProfilesCount {
        count: names.len(),
        shown: shown.join(" / "),
        rest,
    };
    Item::new(KEY, Status::Ok, count.text_in(lang), lang).field("names", json!(names))
}

/// 1 行に並べる先頭と、並べなかった残りの個数（#1623）。
///
/// `default`（引数なしの `tako master` が使う = #322 の最簡形で案内する名前）を先頭に、残りは
/// 名前順（`list_profiles` の並び）。いま動いている master の有無では並びを変えない
/// （観測者で文面を変えない = #1505。GUI の外から叩いた check-health とも同じ 1 行になる）
fn profiles_shown(names: &[String]) -> (Vec<&str>, usize) {
    let default = tako_core::handoff::DEFAULT_PROFILE;
    let names_iter = || names.iter().map(String::as_str);
    let shown: Vec<&str> = names_iter()
        .filter(|name| *name == default)
        .chain(names_iter().filter(|name| *name != default))
        .take(PROFILES_SHOWN)
        .collect();
    let rest = names.len() - shown.len();
    (shown, rest)
}

// --- リモート公開（#1049 / #1485） ------------------------------------------

/// スマホから使えるか。**状態ファイルを読むだけ**（`daemon_status` は死んだ
/// プロセスの残骸を掃除する = 書き込みがあるので `--check` からは呼ばない）
fn remote_item(lang: Lang) -> Item {
    let key = "remote";
    let text = |t: HealthText| t.text_in(lang);
    let desired = crate::remote::read_desired().is_some();
    let Some(pid) = crate::remote::daemon_pid_if_alive() else {
        return if desired {
            // #1485 の症状（公開のはずが止まっている）を名指しする
            Item::new(
                key,
                Status::Missing,
                text(HealthText::RemoteStoppedUnexpectedly),
                lang,
            )
            .line(text(HealthText::RemoteStartHint))
        } else {
            Item::new(key, Status::Info, text(HealthText::RemoteStopped), lang)
        };
    };
    // 動いていることと tailnet から見えることは別物（#1049）
    let health = crate::remote::read_serve_health();
    let fields = crate::remote_serve::status_fields(
        health.as_ref(),
        crate::remote_autostart::now_epoch_secs(),
        crate::remote_serve::watch_interval().as_secs(),
    );
    match crate::remote_serve::degraded_warning_in(&fields, lang) {
        Some(warning) => {
            let mut item = Item::new(key, Status::Warn, text(HealthText::RemoteUnreachable), lang);
            for line in warning.lines() {
                item = item.line(format!("         {line}"));
            }
            item.line(text(HealthText::RemoteStatusHint))
        }
        None => Item::new(key, Status::Ok, text(HealthText::RemoteRunning(pid)), lang),
    }
}

// --- IPC の受け口（#1441） --------------------------------------------------

/// CLI / MCP が届くか。
///
/// **観測者で文面を変えない**のが要点（#1505）。GUI の中では起動時の記録
/// （`ipc_socket::status`）、外からは繋ぎ先の実測（ソケットへ 1 回繋ぐ）を読むが、
/// 届いているときはどちらも**同じ 1 行**になる。受け口の詳細（`kind` / バイト数 /
/// bind エラー）は `check_health` の `ipc` 節が持つ（#1441）ので、ここでは重ねない
fn ipc_item(lang: Lang) -> Item {
    const KEY: &str = "ipc";
    let text = |t: HealthText| t.text_in(lang);

    // GUI の中（起動時に記録がある）
    if let Some(status) = tako_core::ipc_socket::status() {
        if status.bound {
            return Item::new(KEY, Status::Ok, text(HealthText::IpcUp), lang);
        }
        return Item::new(KEY, Status::Warn, text(HealthText::IpcDown), lang)
            .line(text(HealthText::IpcDownReason(status.error.clone())));
    }
    // 外から（繋ぎ先の実測）。置き場の決め方は `ipc_socket` の 1 実装を通す（#1441）
    let Some(data_dir) = tako_core::paths::data_dir() else {
        return Item::new(KEY, Status::Warn, text(HealthText::IpcNoDataDir), lang);
    };
    let limit = tako_core::ipc_socket::max_path_bytes();
    let socket = tako_core::ipc_socket::resolve_with(&data_dir, &std::env::temp_dir(), limit);
    if socket.exists() && crate::discovery::socket_alive(&socket.to_string_lossy()) {
        return Item::new(KEY, Status::Ok, text(HealthText::IpcUp), lang);
    }
    // #1441 の症状（深い data dir でソケットが上限を超える）は原因が違うので名指しする
    let bytes = tako_core::ipc_socket::path_bytes(&socket);
    if bytes > limit {
        return Item::new(KEY, Status::Warn, text(HealthText::IpcTooDeep), lang)
            .line(text(HealthText::IpcPathTooLong { bytes, limit }))
            .line(text(HealthText::IpcShallowerHint));
    }
    Item::new(KEY, Status::Info, text(HealthText::IpcAppNotRunning), lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2431: WebView2 の版の項目。macOS は出さない・入っていなければ不足・古ければ警告で、
    /// どちらも次の一手の行を持つ
    #[test]
    fn webview2の項目は版と次の一手を出す() {
        use tako_core::platform::webview2::{Runtime, INSTALL_COMMAND, REQUIRED_VERSION};
        assert_eq!(webview2_item(&Runtime::NotApplicable, Lang::Ja), None);
        let fresh = webview2_item(
            &Runtime::Installed {
                version: "146.0.3856.84".into(),
            },
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(fresh.key, "webview2");
        assert_eq!(fresh.status, Status::Ok);
        assert!(fresh.summary.contains("146.0.3856.84"));
        assert_eq!(fresh.lines.len(), 1);
        let missing = webview2_item(&Runtime::Missing, Lang::Ja).unwrap();
        assert_eq!(missing.status, Status::Missing);
        assert!(missing.summary.contains("無い"));
        assert!(
            missing.lines[1].contains(INSTALL_COMMAND),
            "{:?}",
            missing.lines
        );
        let old = webview2_item(
            &Runtime::Installed {
                version: "120.0.2210.91".into(),
            },
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(old.status, Status::Warn);
        assert!(old.summary.contains(REQUIRED_VERSION));
        assert_eq!(old.lines.len(), 2);
    }

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
        let item = Item::new("dep.tmux", Status::Ok, "tmux: /usr/bin/tmux", Lang::Ja)
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
            lang: Lang::Ja,
            items: vec![
                Item::new("a", Status::Ok, "あ", Lang::Ja),
                Item::new("b", Status::Missing, "い", Lang::Ja)
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
        let remaining =
            shortest_bootstrap_remaining(&states, Lang::Ja).expect("残り作業が選ばれる");
        assert_eq!(remaining.kind, RemainingKind::AgentStatusUnknown);
        assert_eq!(remaining.detail, vec!["読めません".to_string()]);
        // 空なら 1 件も積まない
        assert!(shortest_bootstrap_remaining(&[], Lang::Ja).is_none());
    }

    /// 段と認証の結果だけを変えた導入状況（パスはホームの外の決め打ち = #927）
    fn bootstrap_state(
        agent: AgentKind,
        step: crate::setup_bootstrap::Step,
        authenticated: bool,
        auth_timeout_secs: Option<u64>,
    ) -> crate::setup_bootstrap::BootstrapState {
        crate::setup_bootstrap::BootstrapState {
            agent,
            binary: Some(format!("/opt/fake/bin/{}", agent.as_str())),
            authenticated,
            account_plan: authenticated.then(|| "max".to_string()),
            launcher_dir: std::path::PathBuf::from("/opt/fake/bin"),
            launcher_dir_on_path: step != crate::setup_bootstrap::Step::Path,
            launcher_dir_on_process_path: step != crate::setup_bootstrap::Step::Path,
            profile: None,
            profile_has_block: false,
            shell: None,
            step,
            plan: crate::setup_bootstrap::install_plan_for(agent).expect("導入の手順"),
            auth_timeout: auth_timeout_secs.map(|waited_secs| tako_core::probe::TimeoutNotice {
                label: tako_core::probe::label(
                    agent.as_str(),
                    crate::setup_bootstrap::auth_probe_argv(agent),
                ),
                waited_secs,
            }),
            locate_timeout: None,
            path_timeout: None,
        }
    }

    /// ログインシェルの打ち切りの知らせ（名札は実行ファイル名と引数だけ = #927）
    fn login_shell_notice(script: &str, waited_secs: u64) -> tako_core::probe::TimeoutNotice {
        tako_core::probe::TimeoutNotice {
            label: tako_core::probe::label("zsh", &["-l", "-c", script]),
            waited_secs,
        }
    }

    fn line_of(item: &Item) -> String {
        item.lines.join("\n")
    }

    /// #2255: 認証の問い合わせを打ち切った系統は「不明」と出し、ログインや導入へ人を送らない
    #[test]
    fn 認証を確かめられなかった系統は不明と出してログインへ送らない() {
        use crate::setup_bootstrap::Step;
        let states = vec![(
            AgentKind::Claude,
            Ok(bootstrap_state(
                AgentKind::Claude,
                Step::Auth,
                false,
                Some(10),
            )),
        )];
        let items = agent_cli_items(&states, Lang::Ja);
        assert_eq!(items[0].status, Status::Warn, "{:?}", items[0]);
        assert!(
            items[0]
                .summary
                .contains("claude の認証: 不明（10 秒で応答なし）"),
            "{}",
            items[0].summary
        );
        assert!(
            items[0].remaining.is_empty(),
            "不明なのに残り作業（ログイン / 導入）を積んだ: {:?}",
            items[0].remaining
        );
        assert_eq!(items[1].key, "agent_cli.claude");
        assert_eq!(items[1].status, Status::Warn);
        assert_eq!(
            line_of(&items[1]),
            "         [警告] claude: 認証: 不明（10 秒で応答なし）"
        );
        let detected = agents_detected_item(&states, Lang::Ja);
        assert!(
            line_of(&detected).contains(
                "    [検出] claude: /opt/fake/bin/claude（認証: 不明（10 秒で応答なし））"
            ),
            "{}",
            line_of(&detected)
        );
    }

    /// #2255 の回帰なし: 打ち切っていなければ今まで通り（認証済み / 未認証・ログインの残り作業）
    #[test]
    fn 打ち切っていなければ認証の行は今まで通り() {
        use crate::setup_bootstrap::Step;
        let ready = vec![(
            AgentKind::Claude,
            Ok(bootstrap_state(AgentKind::Claude, Step::Ready, true, None)),
        )];
        let items = agent_cli_items(&ready, Lang::Ja);
        assert_eq!(items[0].status, Status::Ok);
        assert_eq!(line_of(&items[1]), "         [OK] claude: 完了");
        assert!(line_of(&agents_detected_item(&ready, Lang::Ja))
            .contains("    [検出] claude: /opt/fake/bin/claude（認証済み / max）"));

        let logged_out = vec![(
            AgentKind::Claude,
            Ok(bootstrap_state(AgentKind::Claude, Step::Auth, false, None)),
        )];
        let items = agent_cli_items(&logged_out, Lang::Ja);
        assert_eq!(items[0].status, Status::Missing);
        assert_eq!(
            items[0].remaining.first().map(|r| r.kind.clone()),
            RemainingKind::for_step(Step::Auth, AgentKind::Claude),
            "未認証ならログインを残り作業にする（今まで通り）"
        );
        assert_eq!(items[1].status, Status::Missing);
        assert!(line_of(&items[1]).contains("[不足] claude:"));
        assert!(line_of(&agents_detected_item(&logged_out, Lang::Ja))
            .contains("（未認証 / プラン不明）"));
    }

    /// 使える系統が 1 つでもあれば頭は OK のまま。打ち切った系統だけが警告になる。
    /// PATH の段で止まっている系統は、認証が不明でも PATH の不足を言う（そちらが先に要る手）
    #[test]
    fn 打ち切った系統だけが警告になる() {
        use crate::setup_bootstrap::Step;
        let states = vec![
            (
                AgentKind::Claude,
                Ok(bootstrap_state(AgentKind::Claude, Step::Ready, true, None)),
            ),
            (
                AgentKind::Codex,
                Ok(bootstrap_state(
                    AgentKind::Codex,
                    Step::Auth,
                    false,
                    Some(3),
                )),
            ),
            (
                AgentKind::Agy,
                Ok(bootstrap_state(AgentKind::Agy, Step::Path, false, Some(3))),
            ),
        ];
        let items = agent_cli_items(&states, Lang::Ja);
        assert_eq!(items[0].status, Status::Ok, "{:?}", items[0]);
        assert_eq!(items[2].status, Status::Warn);
        assert_eq!(
            line_of(&items[2]),
            "         [警告] codex: 認証: 不明（3 秒で応答なし）"
        );
        assert_eq!(items[3].status, Status::Missing, "{:?}", items[3]);
        assert!(
            line_of(&items[3]).contains("(path)"),
            "{}",
            line_of(&items[3])
        );
        let detected = line_of(&agents_detected_item(&states, Lang::Ja));
        assert!(detected.contains("codex: /opt/fake/bin/codex（認証: 不明（3 秒で応答なし））"));
        assert!(detected.contains("agy: /opt/fake/bin/agy（認証: 不明（3 秒で応答なし））"));
    }

    /// #1533: 実行ファイルの検出（ログインシェル）を打ち切った系統は「未導入」と言わず、導入の残り作業も積まない
    #[test]
    fn 検出を確かめられなかった系統は未導入と言わない() {
        use crate::setup_bootstrap::Step;
        let mut claude = bootstrap_state(AgentKind::Claude, Step::Install, false, None);
        claude.binary = None;
        claude.locate_timeout = Some(login_shell_notice("command -v claude", 5));
        let states = vec![(AgentKind::Claude, Ok(claude))];
        let items = agent_cli_items(&states, Lang::Ja);
        assert_eq!(items[0].status, Status::Warn, "{:?}", items[0]);
        assert!(
            items[0]
                .summary
                .contains("claude の導入: 不明（ログインシェルが 5 秒で応答なし）"),
            "{}",
            items[0].summary
        );
        assert!(
            items[0].remaining.is_empty(),
            "確かめられないのに導入を残り作業にした: {:?}",
            items[0].remaining
        );
        assert_eq!(
            line_of(&items[1]),
            "         [警告] claude: 導入: 不明（ログインシェルが 5 秒で応答なし）"
        );
        let detected = agents_detected_item(&states, Lang::Ja);
        assert_eq!(detected.status, Status::Warn, "{detected:?}");
        assert!(
            detected.remaining.is_empty(),
            "確かめられないのに「エージェント CLI が無い」を残り作業にした"
        );
        assert!(
            line_of(&detected)
                .contains("[警告] claude を確かめられません（ログインシェルが 5 秒で応答なし）"),
            "{}",
            line_of(&detected)
        );
        assert!(!line_of(&detected).contains("いずれも見つかりません"));
    }

    /// #1532: ログインシェルの PATH の読み取りを打ち切った系統は「PATH に無い」と言わない。
    /// 検出で打ち切って PATH を読みに行かなかったときも同じ（検出の知らせから読む）
    #[test]
    fn pathを確かめられなかった系統はpathに無いと言わない() {
        use crate::setup_bootstrap::Step;
        let mut read_timeout = bootstrap_state(AgentKind::Claude, Step::Path, false, None);
        read_timeout.path_timeout = Some(login_shell_notice("printf %s \"$PATH\"", 2));
        let mut locate_timeout = bootstrap_state(AgentKind::Codex, Step::Path, false, None);
        locate_timeout.locate_timeout = Some(login_shell_notice("command -v codex", 2));
        let states = vec![
            (AgentKind::Claude, Ok(read_timeout)),
            (AgentKind::Codex, Ok(locate_timeout)),
        ];
        let items = agent_cli_items(&states, Lang::Ja);
        assert_eq!(items[0].status, Status::Warn, "{:?}", items[0]);
        for (i, agent) in [(1, "claude"), (2, "codex")] {
            assert_eq!(
                line_of(&items[i]),
                format!("         [警告] {agent}: PATH: 不明（ログインシェルが 2 秒で応答なし）")
            );
        }
        // 打ち切っていない Path の段は今まで通り不足（回帰なし）
        let plain = vec![(
            AgentKind::Claude,
            Ok(bootstrap_state(AgentKind::Claude, Step::Path, false, None)),
        )];
        let items = agent_cli_items(&plain, Lang::Ja);
        assert_eq!(items[1].status, Status::Missing);
        assert!(line_of(&items[1]).contains("(path)"));
    }

    // --- #1622: 外部の問い合わせを重ねる ------------------------------------------

    const DIAGNOSTICS_RS: &str = "crates/tako-control/src/diagnostics.rs";

    /// 本番コードで `needle` が最初に現れる行（名指しに使う。テスト領域はファイルの末尾なので先に当たらない）
    fn source_line(needle: &str) -> String {
        let line = include_str!("diagnostics.rs")
            .lines()
            .position(|l| l.contains(needle))
            .map_or(0, |i| i + 1);
        format!("{DIAGNOSTICS_RS}:{line}")
    }

    /// 偽の問い合わせが互いの始まりを待ち合う観測台（#1622）。
    ///
    /// **実時間で比べない**: 重なっていれば待ち合いはその場で成立し、重なっていなければ（直列）
    /// 相手が始まらないまま締め切りを過ぎる。締め切りはテストが固まらないための安全弁で、
    /// 判定は「待ち合いが成立したか」という状態だけで行う
    struct Rendezvous {
        seen: std::sync::Mutex<Seen>,
        changed: std::sync::Condvar,
    }

    #[derive(Default)]
    struct Seen {
        /// 始まった口（始まった順。背景かどうか）
        started: Vec<(&'static str, bool)>,
        /// いま走っている背景の口の数と、その最大
        running: usize,
        max_running: usize,
        /// 成立しなかった待ち合い
        unmet: Vec<String>,
        /// 締め切りを過ぎた待ちがある（以降の待ちはすぐ抜ける = 失敗が 20 秒 × 件数にならない）
        gave_up: bool,
    }

    impl Seen {
        fn has_started(&self, name: &str) -> bool {
            self.started.iter().any(|(n, _)| *n == name)
        }

        fn background_started(&self) -> usize {
            self.started.iter().filter(|(_, bg)| *bg).count()
        }
    }

    /// 待ち合いの安全弁。成立する側はすぐ抜けるので、通るときの所要時間には出ない
    const RENDEZVOUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

    impl Rendezvous {
        fn new() -> Self {
            Self {
                seen: std::sync::Mutex::new(Seen::default()),
                changed: std::sync::Condvar::new(),
            }
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Seen> {
            self.seen.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// `name` が始まった。背景の口なら、それより前に始まった背景の口の数を返す
        fn enter(&self, name: &'static str, background: bool) -> usize {
            let mut seen = self.lock();
            let before = seen.background_started();
            seen.started.push((name, background));
            if background {
                seen.running += 1;
                seen.max_running = seen.max_running.max(seen.running);
            }
            self.changed.notify_all();
            before
        }

        fn leave(&self) {
            let mut seen = self.lock();
            seen.running -= 1;
            self.changed.notify_all();
        }

        /// `cond` が成り立つまで待つ。締め切りを過ぎたら成立しなかったと記録する
        fn wait_until(&self, who: &str, what: &str, cond: impl Fn(&Seen) -> bool) {
            let deadline = std::time::Instant::now() + RENDEZVOUS_DEADLINE;
            let mut seen = self.lock();
            while !cond(&seen) && !seen.gave_up {
                let now = std::time::Instant::now();
                if now >= deadline {
                    seen.unmet
                        .push(format!("{who} が「{what}」を待ったが成立しなかった"));
                    seen.gave_up = true;
                    self.changed.notify_all();
                    break;
                }
                seen = self
                    .changed
                    .wait_timeout(seen, deadline - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }

        /// 背景の口の本体: 最初の 1 組（上限ぶん）はそろうまで互いに待つ = 上限まで重なることの観測
        fn background<T>(&self, name: &'static str, answer: T) -> T {
            let before = self.enter(name, true);
            if before < PROBE_CONCURRENCY {
                self.wait_until(
                    name,
                    &format!("背景の口が {PROBE_CONCURRENCY} 本同時に走る"),
                    |s| s.background_started() >= PROBE_CONCURRENCY,
                );
            }
            self.leave();
            answer
        }
    }

    /// 偽の答え: claude は導入済みで認証を打ち切り・`claude mcp list` も打ち切り、codex は使える、
    /// agy は**未導入**（背景で採った MCP の答えは捨てられるべき）
    fn fake_bootstrap() -> BootstrapList {
        use crate::setup_bootstrap::Step;
        let mut agy = bootstrap_state(AgentKind::Agy, Step::Install, false, None);
        agy.binary = None;
        vec![
            (
                AgentKind::Claude,
                Ok(bootstrap_state(
                    AgentKind::Claude,
                    Step::Auth,
                    false,
                    Some(10),
                )),
            ),
            (
                AgentKind::Codex,
                Ok(bootstrap_state(AgentKind::Codex, Step::Ready, true, None)),
            ),
            (AgentKind::Agy, Ok(agy)),
        ]
    }

    fn fake_claude_mcp() -> ClaudeMcp {
        let notice = tako_core::probe::TimeoutNotice {
            label: tako_core::probe::label("claude", &["mcp", "list"]),
            waited_secs: 15,
        };
        (false, false, Some(notice))
    }

    fn fake_mcp_ready() -> crate::agent_mcp::McpState {
        crate::agent_mcp::McpState::Ready {
            command: "/opt/fake/bin/tako".to_string(),
        }
    }

    fn fake_deps() -> Vec<Item> {
        crate::setup_deps::current_deps()
            .into_iter()
            .map(|dep| {
                let key = format!("dep.{}", dep.bin);
                Item::new(
                    key,
                    Status::Ok,
                    format!("{0}: /opt/fake/bin/{0}", dep.bin),
                    Lang::Ja,
                )
            })
            .collect()
    }

    fn fake_config_share() -> Item {
        let env = crate::config_share::env::ShareEnvironment {
            linked: false,
            repo: None,
            repo_ok: false,
            external: Vec::new(),
            gh: None,
        };
        let text = config_share_lines(&env, false).join("\n");
        Item::from_text(
            "config_share",
            status_from_line(&text),
            summary_of(&text),
            &text,
        )
    }

    fn fake_tako_cli_path() -> Item {
        let text = "  [OK] tako CLI の PATH: 外部ターミナルからも使えます（/opt/fake/bin）";
        Item::from_text("tako_cli_path", Status::Ok, summary_of(text), text)
    }

    fn fake_answers() -> Answers {
        Answers {
            claude_mcp: fake_claude_mcp(),
            codex_mcp: fake_mcp_ready(),
            agy_mcp: fake_mcp_ready(),
            deps: fake_deps(),
            config_share: fake_config_share(),
            tako_cli_path: fake_tako_cli_path(),
        }
    }

    /// 比べる眺め。`ipc` の項目は GUI 内の記録（プロセス全体で 1 つ）を読み、並走する ipc の
    /// テストが書き換えうるので外す（ここで見たいのは問い合わせの答えから組んだ並びと判定）
    fn comparable(report: &Report) -> (Value, Vec<String>) {
        let kept = Report {
            items: report
                .items
                .iter()
                .filter(|item| item.key != "ipc")
                .cloned()
                .collect(),
            probe_timeouts: report.probe_timeouts.clone(),
            lang: report.lang,
        };
        (kept.to_json(), kept.lines())
    }

    /// #1622 の番犬: 外部の問い合わせは導入状況と重ねて走り、背景は上限まで重なって上限を超えず、
    /// 答えは逐次に組んだものと 1 バイトも変わらない
    #[test]
    fn 外部の問い合わせは導入状況と重ねて走り答えは逐次と同じ() {
        let watch = Rendezvous::new();
        let bootstrap = || {
            watch.enter("bootstrap", false);
            watch.wait_until(
                "導入状況（bootstrap）",
                "claude mcp list が始まる",
                |s| s.has_started("claude_mcp"),
            );
            fake_bootstrap()
        };
        let claude_mcp = || {
            // 背景の先頭（一番長い問い合わせ）は導入状況と重なる
            watch.enter("claude_mcp", true);
            watch.wait_until(
                "claude mcp list",
                "導入状況（bootstrap）が始まる",
                |s| s.has_started("bootstrap"),
            );
            watch.leave();
            fake_claude_mcp()
        };
        let probes = Probes {
            bootstrap: &bootstrap,
            claude_mcp: &claude_mcp,
            codex_mcp: &|| watch.background("codex_mcp", fake_mcp_ready()),
            agy_mcp: &|| watch.background("agy_mcp", fake_mcp_ready()),
            deps: &|| watch.background("deps", fake_deps()),
            config_share: &|| watch.background("config_share", fake_config_share()),
            tako_cli_path: &|| watch.background("tako_cli_path", fake_tako_cli_path()),
        };
        let report = collect_with(&probes, probe_concurrency(false), Lang::Ja);
        let seen = watch.lock();

        // 1. 重なった（直列へ戻すと、導入状況か背景の 1 組が相手の始まりを待ったまま締め切りを過ぎる）
        assert!(
            seen.unmet.is_empty(),
            "{}（fn collect_with）が外部の問い合わせを重ねずに走らせた（#1622）:\n  {}\n\
             始まった順: {:?}\n\n\
             `claude mcp list`（実測 5.7 秒）が導入状況（実測 3 秒）の後に並ぶと check-health が 10 秒へ戻る。\n\
             導入状況は呼び出し元のスレッドで、残りは背景で最大 {PROBE_CONCURRENCY} 本ずつ重ねること（`overlap`）",
            source_line("fn collect_with("),
            seen.unmet.join("\n  "),
            seen.started,
        );
        // 2. 背景の同時数は上限を超えない
        assert!(
            seen.max_running <= PROBE_CONCURRENCY,
            "{}（PROBE_CONCURRENCY = {PROBE_CONCURRENCY}）を超えて背景の問い合わせが {} 本同時に走った（#1622）。\n\
             ログインシェルと CLI を一度に起こしすぎると、遅い機で個々の上限に掛かりやすくなる",
            source_line("const PROBE_CONCURRENCY"),
            seen.max_running,
        );
        // 3. どの口も 1 度だけ呼ばれた（重ねても問い合わせの集合は変わらない）
        let mut names: Vec<&str> = seen.started.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "agy_mcp",
                "bootstrap",
                "claude_mcp",
                "codex_mcp",
                "config_share",
                "deps",
                "tako_cli_path"
            ],
            "{}: 問い合わせの呼ばれ方が変わった（#1622）",
            source_line("fn collect_with(")
        );
        drop(seen);

        // 4. 逐次に組んだもの（問い合わせを起こさない組み立てへ同じ答えを渡す）と 1 バイトも変わらない
        let serial = assemble(fake_bootstrap(), fake_answers(), Lang::Ja);
        assert_eq!(
            comparable(&report),
            comparable(&serial),
            "{}: 重ねた結果が逐次に組んだものと違う（#1622。判定と並びは 1 マスも変えない）",
            source_line("fn collect_with(")
        );

        // 5. 打ち切りの知らせは重ねた後も落ちない・順も今まで通り（認証 → `claude mcp list`）
        let labels: Vec<&str> = report
            .probe_timeouts
            .iter()
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                tako_core::probe::label(
                    "claude",
                    crate::setup_bootstrap::auth_probe_argv(AgentKind::Claude)
                )
                .as_str(),
                tako_core::probe::label("claude", &["mcp", "list"]).as_str(),
            ],
            "打ち切りの知らせが落ちたか並びが変わった（#1622 / #1503 / #2255）"
        );
        let mcp_claude = report
            .items
            .iter()
            .find(|item| item.key == "mcp.claude")
            .expect("mcp.claude の項目");
        assert_eq!(mcp_claude.status, Status::Warn, "{mcp_claude:?}");
        assert!(
            mcp_claude.summary.contains("登録状態を確認できません")
                && mcp_claude.summary.contains("15 秒で応答なし"),
            "`claude mcp list` の打ち切りの知らせが項目から落ちた（#1503 / #1622）: {mcp_claude:?}"
        );

        // 6. 未導入の系統（agy）の答えは捨てる（項目の集合は導入状況だけで決まる = 今まで通り）
        for key in ["mcp.agy", "agent_role.agy", "instructions.agy"] {
            assert!(
                report.items.iter().all(|item| item.key != key),
                "未導入の agy の {key} が出た（背景で採った答えを導入状況で捨てていない。#1622）"
            );
        }
        assert!(report.items.iter().any(|item| item.key == "mcp.codex"));
    }

    /// #1622: 重ねても重ねなくても（A/B の旧経路 = `TAKO_1622_LEGACY=1`）答えは 1 バイトも変わらない
    #[test]
    fn 重ねても重ねなくても答えは同じ() {
        let bootstrap = || fake_bootstrap();
        let probes = Probes {
            bootstrap: &bootstrap,
            claude_mcp: &|| fake_claude_mcp(),
            codex_mcp: &|| fake_mcp_ready(),
            agy_mcp: &|| fake_mcp_ready(),
            deps: &|| fake_deps(),
            config_share: &|| fake_config_share(),
            tako_cli_path: &|| fake_tako_cli_path(),
        };
        let parallel = collect_with(&probes, probe_concurrency(false), Lang::Ja);
        let legacy = collect_with(&probes, probe_concurrency(true), Lang::Ja);
        assert_eq!(comparable(&parallel), comparable(&legacy));
        assert_eq!(probe_concurrency(true), 0, "A/B の旧経路は重ねない");
        assert!(probe_concurrency(false) > 1, "既定は重ねる");
    }

    /// #1622: 本番の入口が A/B のゲートから同時数を決める（`collect_with` へ決め打ちの値を渡すと、
    /// 上の番犬は緑のまま本番だけ直列へ戻る）
    #[test]
    fn 本番の入口はゲートから同時数を決める() {
        let src = include_str!("diagnostics.rs");
        let head = "pub fn collect() -> Report {";
        let start = src.find(head).expect("collect の頭");
        let end = start + src[start..].find("\n}\n").expect("collect の終わり");
        assert!(
            src[start..end].contains("collect_with(&probes, probe_concurrency(issue1622_legacy()), lang)"),
            "{}（fn collect）が `probe_concurrency(issue1622_legacy())` で同時数を決めていない（#1622）",
            source_line(head)
        );
    }

    /// `overlap` は並びの先頭から始め、上限ぶんだけ重ね、答えを取り違えない（#1622）
    #[test]
    fn overlapは全部を1度ずつ走らせてからhereの答えを返す() {
        for limit in [0, 1, 2, PROBE_CONCURRENCY, 8] {
            let done = std::sync::Mutex::new(Vec::new());
            let jobs: Vec<Job<'_>> = (0..5)
                .map(|i| {
                    let done = &done;
                    Box::new(move || done.lock().expect("錠").push(i)) as Job<'_>
                })
                .collect();
            let out = overlap(limit, jobs, || "here");
            assert_eq!(out, "here");
            let mut ran = done.into_inner().expect("錠");
            ran.sort_unstable();
            assert_eq!(ran, vec![0, 1, 2, 3, 4], "limit={limit}");
        }
        // 空でも here は走る
        assert_eq!(overlap(PROBE_CONCURRENCY, Vec::new(), || 7), 7);
    }

    /// `list_profiles` と同じ並び（名前順）の一覧
    fn profile_names(list: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = list.iter().map(|name| name.to_string()).collect();
        names.sort();
        names
    }

    /// 本番と同じ形の 84 個（#1623 の実測の件数。`default` は名前順で 2 番目）
    fn many_profiles() -> Vec<String> {
        let mut names: Vec<String> = ["codex", "default", "takodev"]
            .iter()
            .map(|name| name.to_string())
            .chain((1..=81).map(|i| format!("p{i:02}")))
            .collect();
        names.sort();
        names
    }

    /// #1623 の番犬: プロファイルの項目は「件数・先頭 3 個（`default` が先頭）・他 N 個」の 1 行で、
    /// 全件は `--json` の `names`（名前順）に載る。0 / 1 / 先頭ちょうど / 1 個はみ出す / 84 個を
    /// 日英で固定する。#1623 前（1 行に全部並べる）へ戻ると組み立ての行を名指しして落ちる
    #[test]
    fn プロファイルの項目は先頭3個と他n個の1行で全件はnamesに載る() {
        let at = source_line("fn profiles_item_from(");
        let cases: Vec<(Vec<String>, &str, &str)> = vec![
            (
                Vec::new(),
                "  [情報] プロファイル: 未作成（tako master で自動生成されます）",
                "  [info] profiles: none yet (tako master creates one automatically)",
            ),
            (
                profile_names(&["work"]),
                "  [OK] プロファイル: 1 個（work）",
                "  [OK] profiles: 1 (work)",
            ),
            (
                profile_names(&["work", "codex", "default"]),
                "  [OK] プロファイル: 3 個（default / codex / work）",
                "  [OK] profiles: 3 (default / codex / work)",
            ),
            (
                profile_names(&["d", "c", "b", "a"]),
                "  [OK] プロファイル: 4 個（a / b / c …他 1 個）",
                "  [OK] profiles: 4 (a / b / c … 1 more)",
            ),
            (
                many_profiles(),
                "  [OK] プロファイル: 84 個（default / codex / p01 …他 81 個）",
                "  [OK] profiles: 84 (default / codex / p01 … 81 more)",
            ),
        ];
        for (names, ja, en) in cases {
            for (lang, want) in [(Lang::Ja, ja), (Lang::En, en)] {
                let item = profiles_item_from(Ok(names.clone()), lang, false);
                assert_eq!(
                    item.lines,
                    vec![want.to_string()],
                    "{at}: {} 個のプロファイルの行が #1623 の形（件数・先頭 {PROFILES_SHOWN} 個・他 N 個）でない（{lang:?}）",
                    names.len()
                );
                let value = item.to_json_in(lang);
                assert_eq!(
                    value["names"],
                    json!(names),
                    "{at}: `--json` の `names` が全件（名前順）でない（{} 個・{lang:?}）",
                    names.len()
                );
                assert_eq!(value["lines"], json!([want]));
            }
        }
    }

    /// #1623 の A/B: 旧形は 1 行に全部並べ、`names` を載せない（#1623 前とバイト一致）
    #[test]
    fn プロファイルの項目の旧形は1行に全部並べる() {
        let names = many_profiles();
        let item = profiles_item_from(Ok(names.clone()), Lang::Ja, true);
        assert_eq!(
            item.lines,
            vec![format!(
                "  [OK] プロファイル: 84 個（{}）",
                names.join(", ")
            )]
        );
        assert!(item.to_json_in(Lang::Ja).get("names").is_none());
        let none = profiles_item_from(Ok(Vec::new()), Lang::En, true);
        assert!(none.to_json_in(Lang::En).get("names").is_none());
    }

    /// #1623: 一覧を読めないときは理由を出し、`names` は載せない（空の一覧と言い分ける）
    #[test]
    fn プロファイルを読めないときはnamesを載せない() {
        let denied = || crate::orchestrator::ConfigError::ListProfiles("io".into());
        let item = profiles_item_from(Err(denied()), Lang::En, false);
        assert_eq!(item.status, Status::Info);
        // 括弧の中の理由も行と同じ表示言語（#2365）
        assert_eq!(
            item.lines,
            vec!["  [info] profiles: check failed (failed to read the profiles directory: io)"]
        );
        assert!(item.to_json_in(Lang::En).get("names").is_none());
        let ja = profiles_item_from(Err(denied()), Lang::Ja, false);
        assert_eq!(
            ja.lines,
            vec!["  [情報] プロファイル: 確認失敗 (プロファイルディレクトリの読み取りに失敗: io)"]
        );
    }

    /// #1623: 項目固有の欄は共通の欄を上書きしない
    #[test]
    fn 項目固有の欄は共通の欄を上書きしない() {
        let item = Item::new("k", Status::Ok, "s", Lang::Ja)
            .field("key", json!("other"))
            .field("extra", json!([1, 2]));
        let value = item.to_json();
        assert_eq!(value["key"], "k");
        assert_eq!(value["extra"], json!([1, 2]));
    }

    /// #1623: 本番の入口が A/B のゲートから旧形かどうかを決める（決め打ちの値を渡すと、上の番犬は
    /// 緑のまま本番だけ旧形へ戻る）
    #[test]
    fn プロファイルの項目の本番の入口はゲートから旧形を決める() {
        let src = include_str!("diagnostics.rs");
        let head = "fn profiles_item(lang: Lang) -> Item {";
        let start = src.find(head).expect("profiles_item の頭");
        let end = start + src[start..].find("\n}\n").expect("profiles_item の終わり");
        let body: String = src[start..end].split_whitespace().collect();
        assert!(
            body.contains(
                "profiles_item_from(crate::orchestrator::list_profiles_of_typed(crate::orchestrator::ProfileKind::Master),lang,issue1623_legacy()"
            ),
            "{}（fn profiles_item）が `issue1623_legacy()` で旧形を決めていない（#1623）",
            source_line(head)
        );
    }
}

//! agent_support — agent 系統ごとの能力マトリクス（Issue #982 / エピック #975）
//!
//! ## なぜ要るか
//!
//! tako は **OS 軸で同じ問題を一度解いている**（`platform::support`。#515 / #591）。
//! agent 軸には正本が無く、「claude だけできること」の判断がコードのあちこちへ散っている。
//! `WorkerAgent::has_agents_api()` だけが型で表された能力差で、
//! 監視・送達・resume・setup・MCP は `"claude"` 文字列 か claude 固有モジュール
//! （`claude_tui` / `agents` / `sessions` / `peer_messaging`）へ直接依存している。
//!
//! ここを正本にすると、以降の作業は「マトリクスの 1 マスを動かして根拠を書く」粒度に揃う。
//!
//! ## 設計は `platform::support` の写し
//!
//! | | OS 軸（`platform::support`） | agent 軸（ここ） |
//! |---|---|---|
//! | 軸 | macOS / Windows | claude / codex / agy / ローカル LLM |
//! | 基準系（根拠欄を持たない） | macOS（開発機） | claude（tako の実装基準） |
//! | 状態 | `Support` 4 値 | `AgentSupport` 4 値（同じ意味） |
//! | 根拠 | `Evidence` | `AgentEvidence`（`Source` を足した。後述） |
//! | 理由文 | `Note`（日英対） | **同じ `Note` を再利用**する |
//! | 根拠必須の検査 | T7 | `t7_claude以外の判定には根拠が要る` |
//! | docs | `windows-support.md`（生成物 + CI `--check`） | `agent-support.md`（同） |
//!
//! ## `Source` を足した理由
//!
//! OS 軸の能力は**実機を動かさないと分からない**（だから `SelfTest` / `Measured` が主役）。
//! agent 軸は違って、能力の大半が **tako 自身の配線の有無**で決まる。
//! 「codex worker に MCP を注入していない」は実機を触らずに `grep` で確定できる事実で、
//! 上流 CLI にそもそも手段が無い（`ByDesign`）のとは別種の根拠なので分けた。
//!
//! ## 過大にも過小にも申告しない
//!
//! この宣言は最終的に system prompt へ流れる（OS 軸の #516 と同じ経路を agent 軸へ
//! 広げるのが #992）。甘い宣言は「使える」と信じたエージェントを失敗させ続け、
//! 辛い宣言は使える機能を回避させる。どちらも実害があるので、
//! **`Supported` / `Degraded` / `Unsupported` は根拠を持つことをテストで強制する**。
//!
//! とくに **「上流に手段が無い（`Unsupported`）」と「まだ調べていない（`Pending`）」を
//! 混ぜない**こと。未調査を `Unsupported` へ倒すと、実際には open な道を
//! エージェントが永久に避けるようになる。
//!
//! ## `Local`（ローカル LLM）列の読み方
//!
//! **まだ 1 系統も成立していない枠**で、初期値はほぼ全部 `Pending`。
//! しかも #990（codex CLI を Ollama へ向ける第一歩）ではハーネス自体が codex なので、
//! 成立した時点で codex 列の値をかなり引き継ぐ。
//! 「ローカル LLM を一級の系統として扱う」のは #991。
//! この段階で `Unsupported` へ倒せるマスはほとんど無い、というのが正直な状態。

use crate::platform::support::Note;

/// マトリクスが対象とする agent 系統
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    /// Claude Code。tako の実装基準（= 基準系）
    Claude,
    /// OpenAI Codex CLI
    Codex,
    /// Antigravity CLI
    Agy,
    /// ローカル LLM（Ollama 等）。**まだ成立していない枠**（#990 / #991）
    Local,
}

impl Agent {
    /// 列挙の正本。**既存 5 enum との対応はこの並びを基準に検証する**
    /// （`crates/tako-control/tests/agent_parity.rs`）
    pub const ALL: [Agent; 4] = [Self::Claude, Self::Codex, Self::Agy, Self::Local];

    /// TUI をキー操作で駆動する既存 3 系統（`WorkerAgent` と同じ並び）。
    /// **ローカル LLM は TUI 前提を外す**（#991）ので別扱いになる
    pub const TUI: [Agent; 3] = [Self::Claude, Self::Codex, Self::Agy];

    /// 種別名。設定ファイル・CLI 引数・実際のコマンド名と同一表記
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
            Self::Local => "local",
        }
    }

    /// 種別名からのパース。**大文字小文字を無視する**（`LimitService::parse` と同じ寛容さ）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" => Some(Self::Agy),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// 人間向けの表示名。**日英で同じ**ものだけを使う（製品名 + 一般語）ので
    /// `Note` にしない
    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "OpenAI Codex CLI",
            Self::Agy => "Antigravity CLI",
            Self::Local => "Local LLM",
        }
    }

    /// tako の実装基準か。基準系は根拠欄を持たない（`platform::support` の macOS と同じ）
    pub const fn is_baseline(self) -> bool {
        matches!(self, Self::Claude)
    }
}

/// ある能力がある agent でどこまで使えるか。
/// **意味は `platform::support::Support` と同じ**にそろえてある
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSupport {
    /// claude 同等に動く
    Supported,
    /// 動くが落ちる。`note` は UI・エラーメッセージ・docs にそのまま出る
    Degraded { note: Note },
    /// tako 側が未実装、または**まだ調べていない**。追跡 Issue を必ず持つ
    Pending { note: Note, issue: u32 },
    /// 上流の CLI にその概念・手段がそもそも無い。
    /// **「調べていない」をここへ倒してはいけない**（それは `Pending`）
    Unsupported { note: Note },
}

impl AgentSupport {
    pub fn status(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Degraded { .. } => "degraded",
            Self::Pending { .. } => "pending",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// 縮退の理由（表示言語に追従する）
    pub fn note(self) -> Option<Note> {
        match self {
            Self::Supported => None,
            Self::Degraded { note } | Self::Pending { note, .. } | Self::Unsupported { note } => {
                Some(note)
            }
        }
    }

    pub fn issue(self) -> Option<u32> {
        match self {
            Self::Pending { issue, .. } => Some(issue),
            _ => None,
        }
    }

    /// 呼び出して意味があるか（縮退していても動くなら true）
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Supported | Self::Degraded { .. })
    }
}

/// claude 以外の判定の根拠。**何をもってそう言えるのか**を表そのものに持たせる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEvidence {
    /// **このリポジトリのコード本文**が示す構造的事実（`file:line` か grep の結果）。
    /// 「tako がその配線を持っていない」は実機を触らずに確定できるので、
    /// agent 軸ではこれが主役になる（OS 軸には無い種別）
    Source(&'static str),
    /// 実機の GUI セルフテスト（`TAKO_SELF_TEST=1`）が通した項目
    SelfTest(&'static str),
    /// `cargo test` で緑のテスト名
    UnitTest(&'static str),
    /// 実際に動かした記録（Issue コメント / `.agent/plans/` の記録節）
    Measured(&'static str),
    /// 上流 CLI の仕様・設計判断。**実測する対象がそもそも無い**もの
    ByDesign(&'static str),
    /// 未確認。**`Supported` / `Degraded` / `Unsupported` にはできない**（T7 相当が落とす）
    Unverified,
}

impl AgentEvidence {
    /// 判定の裏づけになる文言（未確認なら `None`）
    pub fn citation(self) -> Option<&'static str> {
        match self {
            Self::Source(s)
            | Self::SelfTest(s)
            | Self::UnitTest(s)
            | Self::Measured(s)
            | Self::ByDesign(s) => Some(s),
            Self::Unverified => None,
        }
    }

    /// 根拠の種別（docs の表と `tako agent-support --json` に出す）
    pub fn kind(self) -> &'static str {
        match self {
            Self::Source(_) => "source",
            Self::SelfTest(_) => "self-test",
            Self::UnitTest(_) => "unit-test",
            Self::Measured(_) => "measured",
            Self::ByDesign(_) => "by-design",
            Self::Unverified => "unverified",
        }
    }
}

/// 1 能力ぶんの対応状況。
///
/// `key` は **MCP ツール名ではなく能力の名前**。OS 軸と違うのは、
/// 1 つの MCP ツールの中に「agent で差が出る部分」と「出ない部分」が混ざるから
/// （例: `tako_orchestrator_report` は第 1 層 = 差なし / 第 2 層 = claude 専用）。
pub struct AgentFeature {
    pub key: &'static str,
    /// 利用者向けの 1 行説明（docs の表と応答に出る）
    pub summary: Note,
    pub claude: AgentSupport,
    pub codex: AgentSupport,
    pub agy: AgentSupport,
    pub local: AgentSupport,
    /// claude 以外の判定根拠。**行に 1 本**。
    /// 同じ配線の有無が 3 系統をまとめて決めることが多いので、1 本の引用で足りる
    /// （足りない行は引用の中で系統ごとに書き分ける）。
    /// claude は基準系なので根拠欄を持たない
    pub evidence: AgentEvidence,
}

impl AgentFeature {
    pub fn on(&self, agent: Agent) -> AgentSupport {
        match agent {
            Agent::Claude => self.claude,
            Agent::Codex => self.codex,
            Agent::Agy => self.agy,
            Agent::Local => self.local,
        }
    }

    /// claude 以外に 1 つでも断定（`Supported` / `Degraded` / `Unsupported`）があるか。
    /// **根拠必須の判定に使う**
    pub fn asserts_non_baseline(&self) -> bool {
        [self.codex, self.agy, self.local]
            .iter()
            .any(|s| !matches!(s, AgentSupport::Pending { .. }))
    }
}

/// 能力キーの定数。**呼び出し側が文字列を直書きしないため**に置く
/// （タイポは `None` として素通りしてしまうので、型で防げるところは防ぐ）
pub mod keys {
    /// アカウント切替（`CLAUDE_CONFIG_DIR` 相当）への追従
    pub const ACCOUNT_SWITCH: &str = "account_switch";
    /// spawn 時に系統を選べるか
    pub const AGENT_SELECT_AT_SPAWN: &str = "agent_select_at_spawn";
    /// thinking / reasoning effort の指定
    pub const EFFORT_CONTROL: &str = "effort_control";
    /// コンフリクト解消エージェントの起動
    pub const GIT_RESOLVE_AGENT: &str = "git_resolve_agent";
    /// ステータスバーの利用制限サービス切替
    pub const LIMIT_SERVICE_SWITCH: &str = "limit_service_switch";
    /// ctx% 超過での自動ハンドオフ
    pub const MASTER_AUTO_HANDOFF: &str = "master_auto_handoff";
    /// master の ctx% 監視
    pub const MASTER_CTX_PERCENT: &str = "master_ctx_percent";
    /// master の引き継ぎ
    pub const MASTER_HANDOFF: &str = "master_handoff";
    /// master の起動
    pub const MASTER_LAUNCH: &str = "master_launch";
    /// master への MCP 接続
    pub const MASTER_MCP: &str = "master_mcp";
    /// master への system prompt 注入
    pub const MASTER_SYSTEM_PROMPT: &str = "master_system_prompt";
    /// ベンダー公式のリモート操作（スマホアプリ / Web）への会話の委譲
    pub const REMOTE_CONTROL: &str = "remote_control";
    /// ハーネスだけ建て直して会話を続ける（#1067。ペインの右クリック / `tako session-restart`）
    pub const SESSION_RESTART_HARNESS: &str = "session_restart_harness";
    /// 引き継ぎを書かせてセッションを交代する（#1067。#749 の手動版）
    pub const SESSION_RESTART_HANDOFF: &str = "session_restart_handoff";
    /// セッションカタログへの登録
    pub const SESSIONS_CATALOG: &str = "sessions_catalog";
    /// セッションの resume
    pub const SESSIONS_RESUME: &str = "sessions_resume";
    /// setup の認証判定
    pub const SETUP_AUTH_CHECK: &str = "setup_auth_check";
    /// setup の認証誘導（ログインの起動代行）
    pub const SETUP_AUTH_LAUNCH: &str = "setup_auth_launch";
    /// setup の CLI 自動導入
    pub const SETUP_CLI_INSTALL: &str = "setup_cli_install";
    /// setup の CLI 検出
    pub const SETUP_DETECT: &str = "setup_detect";
    /// setup の MCP 恒久登録
    pub const SETUP_MCP_REGISTER: &str = "setup_mcp_register";
    /// setup でのモデル選択（一覧の取得手段は系統ごとに違う。#1002）
    pub const SETUP_MODEL_PICKER: &str = "setup_model_picker";
    /// setup のプラン検出
    pub const SETUP_PLAN_DETECT: &str = "setup_plan_detect";
    /// setup の推奨プロファイル生成
    pub const SETUP_PROFILE_RECOMMEND: &str = "setup_profile_recommend";
    /// setup の共通ルール同期
    pub const SETUP_RULES_SYNC: &str = "setup_rules_sync";
    /// solo の起動
    pub const SOLO_LAUNCH: &str = "solo_launch";
    /// Bypass ダイアログの事前承諾
    pub const WORKER_BYPASS_PREACCEPT: &str = "worker_bypass_preaccept";
    /// 選択肢ダイアログの検知と応答
    pub const WORKER_CHOICE_DIALOG: &str = "worker_choice_dialog";
    /// worker ペインからの tako CLI 操作
    pub const WORKER_CLI_CONTROL: &str = "worker_cli_control";
    /// 突然死の検知と復旧コマンド提示
    pub const WORKER_DEATH_RESUME: &str = "worker_death_resume";
    /// 送達の第 1 層（画面を介さない直送）
    pub const WORKER_DELIVERY_PEER: &str = "worker_delivery_peer";
    /// 利用上限からの自動復帰
    pub const WORKER_LIMIT_AUTORESUME: &str = "worker_limit_autoresume";
    /// 利用上限で止まったことの検知
    pub const WORKER_LIMIT_DETECT: &str = "worker_limit_detect";
    /// 利用上限メトリクス（残量%）の取得
    pub const WORKER_LIMIT_METRICS: &str = "worker_limit_metrics";
    /// worker からの MCP 接続
    pub const WORKER_MCP: &str = "worker_mcp";
    /// permission ダイアログの検知と応答
    pub const WORKER_PERMISSION_DIALOG: &str = "worker_permission_dialog";
    /// 初期プロンプトの送達
    pub const WORKER_PROMPT_DELIVERY: &str = "worker_prompt_delivery";
    /// プロンプト未達の検知
    pub const WORKER_PROMPT_UNDELIVERED: &str = "worker_prompt_undelivered";
    /// 報告の第 1 層（scrollback）
    pub const WORKER_REPORT_SCROLLBACK: &str = "worker_report_scrollback";
    /// 報告の第 2 層（transcript）
    pub const WORKER_REPORT_TRANSCRIPT: &str = "worker_report_transcript";
    /// worker の spawn
    pub const WORKER_SPAWN: &str = "worker_spawn";
    /// busy / idle の判定
    pub const WORKER_STATUS_DETECT: &str = "worker_status_detect";
    /// 構造化された状態の一次シグナル（`claude agents --json` 相当）
    pub const WORKER_STATUS_STRUCTURED: &str = "worker_status_structured";
    /// 作業フォルダの事前信頼
    pub const WORKER_TRUST: &str = "worker_trust";
}

/// 縮退の理由。**同じ理由を複数の能力で共有するので定数に集約する**
/// （文言を直したいときに 1 箇所で済む。`platform::support::notes` と同じ方針）
pub mod notes {
    use super::Note;

    // ─── tako 側の配線が無い（= Pending の主因） ─────────────────────

    /// 実装が claude 固有モジュールへ直結していて、他系統への分岐点が無い
    pub const NOT_WIRED: Note = Note::new(
        "tako の実装が claude 専用で、この系統への配線がまだ無い",
        "tako implements this for claude only; there is no plumbing for this agent yet",
    );

    /// 上流に手段があるかどうかを**まだ調べていない**。
    /// **`Unsupported` へ倒すと open な道を永久に避けることになる**ので `Pending` に置く
    /// 手段は在りそうだが実機で確かめていない（引き継ぎ再起動の codex 列）
    pub const NOT_MEASURED_HANDOFF_RESTART: Note = Note::new(
        "手段は揃っているが実機で確かめていない（claude で先行実装した）",
        "The pieces are in place but this has not been measured on a real machine (claude went first)",
    );
    pub const NOT_INVESTIGATED: Note = Note::new(
        "この系統に同等の手段があるかを実物で調べていない（無いと確定したわけではない）",
        "Whether this agent offers an equivalent mechanism has not been investigated yet (it is not established to be impossible)",
    );

    /// ローカル LLM の系統そのものがまだ成立していない
    pub const LOCAL_NOT_ESTABLISHED: Note = Note::new(
        "ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い）",
        "No local-LLM path exists yet (the repository contains no reference to Ollama at all)",
    );

    // ─── agy 固有 ──────────────────────────────────────────

    /// #127。master / solo の組み立てが agy に到達しない
    pub const AGY_NOT_ORCHESTRATOR: Note = Note::new(
        "agy は worker 専用で、master / solo としては起動前にエラーになる（#127）",
        "agy is worker-only; launching it as master or solo fails before start-up (#127)",
    );

    /// #984 の実物調査。agy の会話は SQLite なので読むには新しい依存が要る
    pub const AGY_CONVERSATION_SQLITE: Note = Note::new(
        "agy は会話を SQLite で持つため（~/.gemini/antigravity-cli/conversations/）読むには新しい依存が要る。生存は presence のロックで分かるがターンの開始・完了は取れない",
        "agy stores conversations in SQLite (~/.gemini/antigravity-cli/conversations/), so reading them needs a new dependency; liveness is visible via its presence lock, but turn start/completion is not",
    );

    /// #985 の再調査（agy 1.1.22）。**窓つきの利用上限という概念自体が無い**
    pub const AGY_CREDITS_NOT_WINDOWED: Note = Note::new(
        "agy の残量は前払いの AI クレジット残高で、5h / 週のような枠とリセット時刻が無い。残高も対話の /credits モーダルの中にしか出ないので、worker の画面を乱さずに読む口が無い（#985 で agy 1.1.22 を再調査）",
        "In agy, headroom is a prepaid AI-credit balance: there is no 5h/weekly window and no reset time. The balance appears only inside the interactive /credits modal, so there is no way to read it without disturbing the worker's screen (re-verified on agy 1.1.22 in #985)",
    );

    /// #985: 上限で「止まる」概念が無いので待つべきリセットが存在しない
    pub const AGY_NO_LIMIT_RESET: Note = Note::new(
        "agy はクレジットを使い切っても「解除を待つ」出口が無い（買い足す導線しか無い）ので、待って再開するという動作が成立しない（#985）",
        "When agy runs out of credits there is no \"wait for the reset\" exit at all, only a purchase link, so waiting and resuming cannot work (#985)",
    );

    // ─── 実測で分かっている縮退 ─────────────────────────────────

    /// #983 の変更 2。一次シグナルが無い系統は「未達」と断定できない
    pub const SCREEN_ONLY_DELIVERY: Note = Note::new(
        "送達を裏づける一次シグナルが無いので、猶予を過ぎても「未達」と断定せず「未確認」を返す（自動再送を撃つと二重指示になる）",
        "There is no primary signal that confirms delivery, so after the grace period tako reports \"unverified\" rather than asserting non-delivery (auto-resending here would duplicate the instruction)",
    );

    /// `wait.rs` の `need_streak`。構造化シグナルが無いぶん確定までの往復が増える
    pub const SCREEN_ONLY_STATUS: Note = Note::new(
        "状態が画面推定のみなので完了の確定が遅い（同じ判定を 8 回続けて見る必要がある。claude は 3 回）",
        "State is inferred from the screen only, so completion is confirmed slowly (eight consecutive readings are required, versus three for claude)",
    );

    /// agy のフッター「(Thinking)」で busy を誤爆した履歴（#120）
    pub const SCREEN_ONLY_STATUS_AGY: Note = Note::new(
        "状態が画面推定のみで完了の確定が遅く、かつフッターの文言で busy を誤検知した履歴がある（#120）",
        "State is inferred from the screen only, so completion is confirmed slowly; a footer string has also caused false busy detection before (#120)",
    );

    /// #512 / #558。claude だけが config dir 切替に追従する
    pub const FIXED_CONFIG_PATH: Note = Note::new(
        "設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない",
        "The config file location is fixed, so tako's account switching has no effect for this agent",
    );

    /// #985: codex はクレジット切れの出口が課金しか無い
    pub const CODEX_CREDITS_NEED_PURCHASE: Note = Note::new(
        "5h / 週の枠は解除を待って自分で再開するが、ワークスペースのクレジットが尽きた場合は「待つ」出口が無い（増枠申請・購入・獲得済みリセットの引き換えしか無いので、tako は何も選ばずに止まる）",
        "The 5h and weekly windows resume by themselves once they reset, but when workspace credits run out there is no \"wait\" exit at all (only request-an-increase, purchase, or redeeming an earned reset), so tako stops without choosing anything",
    );

    /// spawn は通るが pending のまま索引が育たない（transcript が無いため）
    pub const CATALOG_PENDING_ONLY: Note = Note::new(
        "spawn の記録は残るが、会話の実体を索引できないので pending のまま期限切れで消える",
        "The spawn is recorded, but the conversation itself cannot be indexed, so the entry stays pending and expires",
    );

    /// 起動はできるが MCP を話せない（#986 が埋める）
    pub const LAUNCH_ONLY_NO_MCP: Note = Note::new(
        "起動はできるが tako の MCP ツールを呼べない（#986）",
        "It launches, but cannot call tako's MCP tools (#986)",
    );

    // ─── 上流に概念が無い（= Unsupported） ───────────────────────

    /// #790 の第 1 層は claude の Cross-Session Messaging に固有
    pub const NO_PEER_INBOX: Note = Note::new(
        "画面を介さない直送は claude の受信箱（Cross-Session Messaging）に固有の仕組みで、他系統には相当物が無い",
        "Screen-free direct delivery relies on claude's cross-session messaging inbox, which has no counterpart in other agents",
    );

    /// ローカル LLM のダイアログ事情は**どのハーネスが先に成立するかで変わる**。
    ///
    /// #990（codex CLI を Ollama へ向ける）なら画面は codex の TUI なのでダイアログは在り、
    /// #991（非 TUI 経路）ならそもそも画面が無い。**まだ決まっていない**ので
    /// `Unsupported`（= 上流に手段が無い）へ倒してはいけない
    pub const LOCAL_HARNESS_UNDECIDED: Note = Note::new(
        "ローカル LLM のハーネスが決まっていないので可否が定まらない（codex TUI を借りる #990 なら在り、非 TUI 経路の #991 なら無い）",
        "The local-LLM harness is not decided yet, so this is undetermined (present if it borrows the codex TUI in #990, absent on the non-TUI path in #991)",
    );

    /// #1068 の実測（codex 0.150.1）。`remote-control` はあるが**別の形**
    pub const CODEX_SELF_HOSTED_REMOTE: Note = Note::new(
        "codex にも remote-control（experimental）はあるが、これは自前で app-server デーモンを立てて websocket + bearer トークンで TUI を繋ぐ形で、ベンダー側のスマホアプリへ会話を出すものではない。tako 側の配線も無い",
        "codex does have an (experimental) remote-control, but it is a self-hosted app-server daemon that a TUI connects to over websocket with a bearer token: it does not surface the conversation in a vendor mobile app. tako has no plumbing for it either",
    );

    /// #1068 の実測（agy 1.1.23）。フラグ・サブコマンドの全件にリモート操作が無い
    pub const AGY_NO_REMOTE_CONTROL: Note = Note::new(
        "agy の `--help` 全件（フラグ 24 / サブコマンド 11）にリモート操作の口が無い（`mic-serve` はマイクを別ホストへ配るだけ）ので、会話を外の端末から操作する手段がそもそも無い",
        "The full agy `--help` surface (24 flags, 11 subcommands) has no remote-control entry point at all (`mic-serve` only shares a microphone with another host), so there is no way to drive the conversation from another device",
    );

    /// ローカルで動かすモデルに利用上限という概念が無い
    pub const NO_LOCAL_USAGE_LIMIT: Note = Note::new(
        "自分のマシンで動かすモデルなので利用上限という概念が無い",
        "The model runs on your own machine, so there is no notion of a usage limit",
    );
}

/// 指定 agent での対応状況。未登録キーは `None`（= マトリクスへの登録漏れ）
pub fn support_for(agent: Agent, key: &str) -> Option<AgentSupport> {
    MATRIX.iter().find(|f| f.key == key).map(|f| f.on(agent))
}

/// 指定 agent でその能力が使えるか。**未登録キーは `true`**（素通し）。
///
/// 登録漏れで機能が止まるより、被覆テストの失敗で気付く方がよい
/// （`platform::support::gate_in` の `None => Ok(())` と同じ判断）
pub fn supports(agent: Agent, key: &str) -> bool {
    support_for(agent, key).is_none_or(|s| s.is_usable())
}

/// 初期プロンプトが届いたかを**何で観測できるか**（#983 の変更 2）。
///
/// 旧実装は `registry::prompt_delivery_assessment` が `agent != "claude"` を
/// `NotApplicable` で即返していた。誤検知を避ける判断としては妥当だったが、
/// **結果として「観測手段が無い = 何も言わない」**になっていた（#983 の無言死）。
///
/// どちらの手が使えるかは**マトリクスの 1 マス**（[`keys::WORKER_STATUS_STRUCTURED`]）が
/// 決める。系統が増えても、この関数ではなくマトリクスを直せば追従する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryObservation {
    /// 画面に依らない一次シグナルがある（claude の transcript / codex の rollout）。
    /// 「猶予を過ぎても一次シグナルが無い」は**未達と断定してよい**
    Structured,
    /// 画面の送達確認（#32 / #640 の PromptFlow の結果）しか手が無い。
    /// 記録が残っていなければ「届いていない」のか「観測できなかった」のか区別できないので、
    /// **未達と断定せず「未確認」を返す**（自動再送を撃たせない = 二重指示事故を作らない）
    ScreenOnly,
}

/// 指定 agent の送達観測手段。判断は [`keys::WORKER_STATUS_STRUCTURED`] のマスから引く
pub fn delivery_observation(agent: Agent) -> DeliveryObservation {
    if supports(agent, keys::WORKER_STATUS_STRUCTURED) {
        DeliveryObservation::Structured
    } else {
        DeliveryObservation::ScreenOnly
    }
}

/// 指定 agent の能力一覧。`status` を渡すとその状態だけに絞る
pub fn features(agent: Agent, status: Option<&str>) -> Vec<(&'static AgentFeature, AgentSupport)> {
    MATRIX
        .iter()
        .map(|f| (f, f.on(agent)))
        .filter(|(_, s)| status.is_none_or(|want| s.status() == want))
        .collect()
}

/// 縮退している能力の理由を `Note` のまま返す（重複は畳む）。
///
/// **文言を早期に `&'static str` へ解決すると、その時点の言語で凍結して
/// 言語切替に追従しなくなる**。prompt へ注入するなど後で描画するものは必ずこちらを使う
pub fn degraded_note_items(agent: Agent) -> Vec<Note> {
    let mut seen: Vec<Note> = Vec::new();
    for f in MATRIX {
        if let Some(note) = f.on(agent).note() {
            if !seen.contains(&note) {
                seen.push(note);
            }
        }
    }
    seen
}

/// 縮退している能力の説明文。system prompt へ注入して
/// 「この系統で何ができないか」をエージェントへ知らせるのに使う（#992）
pub fn degraded_notes(agent: Agent) -> Vec<&'static str> {
    degraded_note_items(agent)
        .into_iter()
        .map(Note::text)
        .collect()
}

/// 実行してよいかの判定。`Err` の中身はそのまま利用者への診断メッセージになる。
/// **メッセージをマトリクス以外の場所に書かない**ための唯一の入口
pub fn gate(agent: Agent, key: &str) -> Result<(), String> {
    gate_in(agent, key, crate::i18n::lang())
}

/// `gate` の言語を明示する版。**言語グローバルに触らず解決できる**ようにするため、
/// 実体はこちらの純粋関数に置く（定型文と理由文で別々に `i18n::lang()` を読むと
/// その間の切替で日英が混ざる。#608）
pub fn gate_in(agent: Agent, key: &str, lang: crate::i18n::Lang) -> Result<(), String> {
    match support_for(agent, key) {
        None => Ok(()),
        Some(s) if s.is_usable() => Ok(()),
        Some(s) => {
            let note = s.note().map(|n| n.text_in(lang)).unwrap_or_default();
            let label = agent.label();
            Err(match (lang, s.issue()) {
                (crate::i18n::Lang::Ja, Some(issue)) => format!(
                    "{key} は {label} では使えません（{note}）。追跡: #{issue}。\
                     対応したら crates/tako-core/src/agent_support.rs の対応状況を更新してください"
                ),
                (crate::i18n::Lang::Ja, None) => {
                    format!("{key} は {label} では使えません（{note}）")
                }
                (crate::i18n::Lang::En, Some(issue)) => format!(
                    "{key} is not available for {label} ({note}). Tracking: #{issue}. \
                     Update crates/tako-core/src/agent_support.rs once supported."
                ),
                (crate::i18n::Lang::En, None) => {
                    format!("{key} is not available for {label} ({note})")
                }
            })
        }
    }
}

// ─── マトリクスを短く書くための const ヘルパー ─────────────────────
// 40 行 × 4 列を素で書くと読めなくなるので、状態だけを短縮する。
// **根拠は行ごとに書く**（共有すると引用の意味が薄れる）

use AgentSupport as S;

const fn degraded(note: Note) -> AgentSupport {
    S::Degraded { note }
}

const fn pending(note: Note, issue: u32) -> AgentSupport {
    S::Pending { note, issue }
}

const fn unsupported(note: Note) -> AgentSupport {
    S::Unsupported { note }
}

/// ローカル LLM の未成立（第一歩 = #990）
const fn local_pending() -> AgentSupport {
    pending(notes::LOCAL_NOT_ESTABLISHED, 990)
}

/// ローカル LLM の未成立（一級対応 = #991 が要る領域）
const fn local_pending_first_class() -> AgentSupport {
    pending(notes::LOCAL_NOT_ESTABLISHED, 991)
}

/// agent 系統ごとの対応状況の**正本**。**キーは昇順**（テストが検証する）。
///
/// 初期値は #975 の棚卸しレポート（2026-08-27・基準コミット `a620e8a`）の
/// §1「分岐の全件一覧」と §9「総括マトリクス」から引いた。
/// **本調査では実機を動かしていない**ので、根拠は原則
/// `Source`（コード本文の引用）と、既存 Issue に残る実測記録（`Measured`）に限る。
pub const MATRIX: &[AgentFeature] = &[
    AgentFeature {
        key: keys::ACCOUNT_SWITCH,
        summary: Note::new(
            "アカウント（資格情報）の切替に追従する",
            "Honours tako's account (credential) switching",
        ),
        claude: S::Supported,
        codex: pending(notes::FIXED_CONFIG_PATH, 975),
        agy: pending(notes::FIXED_CONFIG_PATH, 975),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "orchestrator/agent.rs の事前信頼は claude だけ CLAUDE_CONFIG_DIR（#512 / #558）を \
             見て書き先を決め、codex は ~/.codex/config.toml、agy は \
             ~/.gemini/antigravity-cli/settings.json を固定で開く（同ファイルのコメントが明示）",
        ),
    },
    AgentFeature {
        key: keys::AGENT_SELECT_AT_SPAWN,
        summary: Note::new(
            "worker を立てるときに系統を選べる（設定の書き換えや再起動なしで）",
            "The agent can be chosen when spawning a worker, with no config edit or restart",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "orchestrator/agent.rs の WorkerAgent が spawn 引数・プロファイルの両方から \
             解決され、build_worker_cmd_in が 3 系統ぶんのコマンドを組む。\
             ペイン単位・タスク単位の切替導線は #988",
        ),
    },
    AgentFeature {
        key: keys::EFFORT_CONTROL,
        summary: Note::new(
            "thinking / reasoning effort を tako から指定する",
            "Thinking / reasoning effort can be set from tako",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending(),
        evidence: AgentEvidence::Measured(
            "#1002 の実測（agy 1.1.22）: `--effort（low|medium|high）` が --help に実在し、\
             `agy models` が挙げる 6 モデルすべてで不正値が \
             `invalid --effort \"bogus\" (valid: low, medium, high)` として咎められる \
             = 表示名に \"(High)\" 等を含むモデルでも --effort の検証が走る。正しい組み合わせは \
             検証を通り API 呼び出しへ進む。**未知のモデル名のときだけ** \
             `--effort is not supported for model \"…\"` になる（この文言を「agy は effort 非対応」と \
             読み違えないこと）。orchestrator/agent.rs は claude = --effort / \
             codex = -c model_reasoning_effort= / agy = --effort へ写像する（旧挙動は TAKO_1002_LEGACY=1）",
        ),
    },
    AgentFeature {
        key: keys::GIT_RESOLVE_AGENT,
        summary: Note::new(
            "コンフリクト解消エージェントとして起動する（#496）",
            "Can be launched as the merge-conflict resolver agent (#496)",
        ),
        claude: S::Supported,
        codex: degraded(notes::LAUNCH_ONLY_NO_MCP),
        agy: degraded(notes::LAUNCH_ONLY_NO_MCP),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "dispatch.rs の git resolve は 3 系統とも起動できるが、worker と同じ経路なので \
             MCP の一時注入が無い（mcp_servers を組むのは orchestrator/mod.rs の master 側だけ）",
        ),
    },
    AgentFeature {
        key: keys::LIMIT_SERVICE_SWITCH,
        summary: Note::new(
            "ステータスバーの利用制限表示をこの系統へ切り替えられる（#217 / #357）",
            "The status-bar usage-limit readout can be switched to this agent (#217 / #357)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: unsupported(notes::AGY_CREDITS_NOT_WINDOWED),
        local: local_pending(),
        evidence: AgentEvidence::Measured(
            "#985: ステータスバーの codex 表示は rollout の構造化データ（`rate_limits`）を \
             読む形になり、有料プランの実データが出る。agy は取得不能を再確認して \
             unsupported の明示表示のまま（#357 の判断は理由を差し替えて維持）",
        ),
    },
    AgentFeature {
        key: keys::MASTER_AUTO_HANDOFF,
        summary: Note::new(
            "ctx% が閾値を超えたら自分で引き継ぐ（#749）",
            "Hands off by itself once the context ratio crosses the threshold (#749)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_INVESTIGATED, 984),
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "#749 の発火材料は画面由来の ctx%。codex 側のパターンは terminal.rs にあるが \
             採取 fixture が無く、実際に描画されるかは未確認（棚卸し §10 の 2 番）",
        ),
    },
    AgentFeature {
        key: keys::MASTER_CTX_PERCENT,
        summary: Note::new(
            "コンテキスト残量を画面から読み取れる",
            "The remaining context ratio can be read off the screen",
        ),
        claude: S::Supported,
        codex: degraded(Note::new(
            "worker の状態照会では構造化ソースから ctx% が取れるが、master の自動ハンドオフは画面のパターンを見るので master 経路では未確認",
            "The context ratio is available from the structured source for worker status queries, but the master auto-handoff reads screen patterns, so the master path is unverified",
        )),
        agy: pending(notes::AGY_CONVERSATION_SQLITE, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Measured(
            "#984: rollout の token_count に last_token_usage.total_tokens と \
             model_context_window があり、worker_status の ctx_percent へ載せた（実測で 8%）。\
             master の #749 は terminal.rs の画面パターンを見る別経路なのでそこは未確認",
        ),
    },
    AgentFeature {
        key: keys::MASTER_HANDOFF,
        summary: Note::new(
            "master の引き継ぎ（後任の spawn と管轄の受け渡し）が通る",
            "Master handoff works: the successor is spawned and jurisdiction transfers",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "handoff の後任は build_master_cmd_in（orchestrator/mod.rs）を通るので \
             master が起動できる系統では通る。agy はその関数に到達しない",
        ),
    },
    AgentFeature {
        key: keys::MASTER_LAUNCH,
        summary: Note::new(
            "master オーケストレーターとして起動する（`tako master`）",
            "Can be launched as the master orchestrator (`tako master`)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "orchestrator/mod.rs の build_master_cmd_in は claude / codex で分岐し、\
             agy は unreachable!() で到達しない（#127 の設計判断。前提の再評価は #987）",
        ),
    },
    AgentFeature {
        key: keys::MASTER_MCP,
        summary: Note::new(
            "master が tako の MCP ツール群を呼べる",
            "The master can call tako's MCP tools",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "orchestrator/mod.rs が codex へ -c mcp_servers.tako.* を起動時に一時注入する \
             （恒久登録はしない = tako 外の codex にツールを出さない。FR-2.3.2）",
        ),
    },
    AgentFeature {
        key: keys::MASTER_SYSTEM_PROMPT,
        summary: Note::new(
            "master の system prompt がモデルへ届く",
            "The master system prompt reaches the model",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "claude は --append-system-prompt-file、codex は developer_instructions で注入する \
             （orchestrator/mod.rs）。agy の注入手段（custom agent 定義の起動時選択）は \
             公式ドキュメントに記載が無く #987 で実機確認する",
        ),
    },
    AgentFeature {
        key: keys::REMOTE_CONTROL,
        summary: Note::new(
            "会話をベンダー公式のリモート操作（スマホアプリ / Web）へ委譲できる（#1068）",
            "The conversation can be delegated to the vendor's official remote control (mobile app / web) (#1068)",
        ),
        claude: S::Supported,
        codex: pending(notes::CODEX_SELF_HOSTED_REMOTE, 1059),
        agy: unsupported(notes::AGY_NO_REMOTE_CONTROL),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Measured(
            "#1068 の実測（2026-09-02）: claude 2.1.232 の `--help` に \
             `--remote-control [name]  Start an interactive session with Remote Control enabled` が在り、\
             tako は build_master_cmd_in / build_worker_cmd_in の claude 分岐でこれを渡す。\
             codex 0.150.1 の `--help` には `remote-control  [experimental] Manage the app-server daemon \
             with remote control enabled` と `--remote <ADDR>` があるが自前ホストの app-server 経路で別物。\
             agy 1.1.23 の `--help` 全件にはリモート操作の口が無い",
        ),
    },
    AgentFeature {
        key: keys::SESSION_RESTART_HANDOFF,
        summary: Note::new(
            "引き継ぎを書かせてセッションを交代する（#1067。ペインの右クリック / `tako session-restart --mode handoff`）",
            "The session is replaced after the agent writes a handoff (#1067; pane context menu / `tako session-restart --mode handoff`)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_MEASURED_HANDOFF_RESTART, 1067),
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "引き継ぎ再起動は master ペインへ定型文を送り、エージェント自身が              tako_orchestrator_handoff を呼ぶ形（handoff.rs の restart_prompt）。             codex master は #979 で MCP が届くので成立しうるが未実測。             agy は master になれない（#987）ので対象そのものが無い",
        ),
    },
    AgentFeature {
        key: keys::SESSION_RESTART_HARNESS,
        summary: Note::new(
            "会話を保ったまま CLI プロセスだけ建て直す（#1067。CLI の自動更新に追いつく手段）",
            "The CLI process is rebuilt while the conversation is kept (#1067; how a session catches up with a CLI auto-update)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 984),
        agy: pending(notes::NOT_WIRED, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "session_restart の harness は sessions::resume_command（claude --resume）を              組んで送るので、resume を配線していない系統では成立しない              （手段自体は上流にある: codex resume / agy --conversation）",
        ),
    },
    AgentFeature {
        key: keys::SESSIONS_CATALOG,
        summary: Note::new(
            "会話がセッションカタログに索引される（#112）",
            "Conversations are indexed in the session catalog (#112)",
        ),
        claude: S::Supported,
        codex: degraded(notes::CATALOG_PENDING_ONLY),
        agy: degraded(notes::CATALOG_PENDING_ONLY),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "sessions.rs の昇格は claude のセッション検出（transcript）に依存する。\
             3 系統とも spawn 時に pending 記録は作られるが、claude 以外は昇格しない",
        ),
    },
    AgentFeature {
        key: keys::SESSIONS_RESUME,
        summary: Note::new(
            "過去の会話を復元して続ける（`tako sessions resume`）",
            "A past conversation can be resumed (`tako sessions resume`)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 984),
        agy: pending(notes::NOT_WIRED, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "dispatch.rs の resume は claude --resume <session_id> を組み、\
             ~/.claude/projects の transcript を前提にする（claude 以外は分類済みエラーで \
             手動の代替を案内する）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_AUTH_CHECK,
        summary: Note::new(
            "setup が認証済みかどうかを判定できる",
            "Setup can tell whether the CLI is already authenticated",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: degraded(Note::new(
            "認証の有無は分かるがプランを取れないので、推奨プロファイルの規模を決められない",
            "Authentication is detectable, but the plan is not, so the recommended profile size cannot be derived",
        )),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "tako-cli/src/setup.rs のプラン解決は認証済み・導入済みの provider だけを巡る \
             （#262）。agy は provider としてプランを返さない",
        ),
    },
    AgentFeature {
        key: keys::SETUP_AUTH_LAUNCH,
        summary: Note::new(
            "未認証なら setup がログインまで案内・代行する",
            "When unauthenticated, setup guides and performs the login",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 989),
        agy: pending(notes::NOT_WIRED, 989),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "setup.rs の認証誘導は claude の導線しか持たない（#868 のゼロスタートも claude 限定）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_CLI_INSTALL,
        summary: Note::new(
            "CLI 自体が入っていない環境へ setup が導入する（#868）",
            "Setup installs the CLI itself on a machine that does not have it (#868)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 989),
        agy: pending(notes::NOT_WIRED, 989),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "platform/agent_install.rs の AgentKind が Claude 1 値しか持たず、\
             recipe() も claude ぶんしか無い（#868 の Out of scope。拡張は #989）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_DETECT,
        summary: Note::new(
            "setup がこの CLI の導入を検出する",
            "Setup detects that this CLI is installed",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: pending(notes::LOCAL_NOT_ESTABLISHED, 990),
        evidence: AgentEvidence::Source(
            "setup.rs の SetupAgent が 3 系統を列挙し、platform::exe::find（B16）で解決する",
        ),
    },
    AgentFeature {
        key: keys::SETUP_MCP_REGISTER,
        summary: Note::new(
            "setup が tako の MCP サーバーをこの CLI へ恒久登録する",
            "Setup registers tako's MCP server with this CLI persistently",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending(),
        evidence: AgentEvidence::Measured(
            "#979（main の 63a7c26）で `tako setup-mcp` が 3 系統へ登録するようになった。\
             書き先は claude = ~/.claude.json / codex = ~/.codex/config.toml の \
             [mcp_servers.tako] / agy = ~/.gemini/config/mcp_config.json で、codex は \
             env_vars 許可リストまで足して実セッションから tako_list_panes が通ることを実測。\
             正本は tako-control::agent_mcp",
        ),
    },
    AgentFeature {
        key: keys::SETUP_MODEL_PICKER,
        summary: Note::new(
            "setup でモデルを選んでプロファイルへ反映できる（一覧は CLI から実取得し、\
             一覧コマンドを持たない系統は同梱の既知リスト + 取得不可の明示。#1002）",
            "Setup can pick a model and write it to the profile (the list is fetched from the CLI; \
             agents without a list command fall back to a built-in list and say so explicitly) (#1002)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending(),
        evidence: AgentEvidence::Measured(
            "#1002 の実測（2026-08-27）: codex 0.150.1 は `codex debug models` が \
             `Render the raw model catalog as JSON` で slug / display_name / \
             supported_reasoning_levels / context_window を返す（未認証でも既定カタログ、 \
             認証すると内容が変わる）。agy 1.1.22 は `agy models` が `id<TAB>表示名` の TSV を \
             stdout へ返し未認証は exit 1 + `Please sign in to view available models.`。 \
             claude 2.1.232 は該当サブコマンドが無く `claude models` は**プロンプトとして \
             解釈される**（一覧はセッション内の /model のみ）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_PLAN_DETECT,
        summary: Note::new(
            "setup が契約プランを検出して推奨規模を決める",
            "Setup detects the subscription plan and sizes the recommendation",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: unsupported(Note::new(
            "agy はプラン情報を出さないので検出できない",
            "agy does not expose plan information, so it cannot be detected",
        )),
        local: unsupported(Note::new(
            "ローカルモデルに契約プランという概念が無い",
            "A local model has no notion of a subscription plan",
        )),
        evidence: AgentEvidence::Source(
            "setup.rs の Provider は Claude / Gpt / Google の 3 値だが、プラン取得は \
             claude / gpt の 2 経路しか実装が無い（#226）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_PROFILE_RECOMMEND,
        summary: Note::new(
            "setup が起動プロファイルを組み立てる",
            "Setup assembles a launch profile",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: degraded(Note::new(
            "worker としてのプロファイルは作れるが、master には別系統が自動で選ばれる",
            "A worker profile can be created, but a different agent is chosen automatically for the master",
        )),
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "setup.rs は選択した agent を worker_agent へ書くが、master_agent は \
             claude / codex しか受け付けない（agy は起動前エラーになるため）",
        ),
    },
    AgentFeature {
        key: keys::SETUP_RULES_SYNC,
        summary: Note::new(
            "共通ルールをこの CLI のグローバル指示ファイルへ同期する（#136）",
            "Shared rules are synced into this CLI's global instruction file (#136)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "agents_sync.rs の AgentKind が 3 系統ぶんの書き先を持つ \
             （~/.claude/CLAUDE.md / ~/.codex/AGENTS.md / ~/.gemini/GEMINI.md）",
        ),
    },
    AgentFeature {
        key: keys::SOLO_LAUNCH,
        summary: Note::new(
            "1 対 1 対話の solo として起動する（`tako solo`）",
            "Can be launched as a one-to-one solo agent (`tako solo`)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_NOT_ORCHESTRATOR, 987),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "solo は master と同じ build_master_cmd_in を通るので、agy は同じ理由で \
             起動前にエラーになる（#111 / #127）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_BYPASS_PREACCEPT,
        summary: Note::new(
            "起動直後の Bypass 確認ダイアログを事前に承諾しておく（#407）",
            "The bypass confirmation dialog shown at start-up is pre-accepted (#407)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 983),
        agy: pending(notes::NOT_WIRED, 983),
        local: pending(notes::LOCAL_HARNESS_UNDECIDED, 991),
        evidence: AgentEvidence::Source(
            "dispatch.rs の事前承諾は 2 箇所とも WorkerAgent::Claude を条件にしている。\
             codex / agy は default_skip_permissions() が true なので常に skip 側なのに \
             事前承諾が無い（棚卸し §1.3(c)）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_CHOICE_DIALOG,
        summary: Note::new(
            "選択肢ダイアログを構造として読み、番号やラベルで応答する（#748）",
            "Choice dialogs are read structurally and answered by number or label (#748)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: pending(notes::LOCAL_HARNESS_UNDECIDED, 991),
        evidence: AgentEvidence::Source(
            "claude_tui.rs は claude v2.1.198 / codex 0.144.1 / agy 1.1.0 の実採取画面の \
             和集合として実装され、CODEX_TRUST_DIALOG / AGY_PERMISSION_DIALOG 等の \
             fixture が同ファイルに在る",
        ),
    },
    AgentFeature {
        key: keys::WORKER_CLI_CONTROL,
        summary: Note::new(
            "worker ペインの中から tako CLI で tako を操作できる",
            "tako can be driven with the tako CLI from inside the worker pane",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "TAKO_PANE_ID / TAKO_SOCKET / TAKO_TOKEN の注入と PATH 注入（#601）は \
             ペイン単位で agent に依らない",
        ),
    },
    AgentFeature {
        key: keys::WORKER_DEATH_RESUME,
        summary: Note::new(
            "突然死を検知して復旧コマンドを提示する（#390）",
            "Sudden death is detected and a recovery command is offered (#390)",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 984),
        agy: pending(notes::NOT_WIRED, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "dispatch.rs のレジストリの resume_command はコメントどおり claude のみ \
             （session ID から claude --resume を組む）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_DELIVERY_PEER,
        summary: Note::new(
            "画面を介さずに指示を直送する（生成中でも取りこぼさない。#790）",
            "Instructions are delivered without going through the screen, so nothing is dropped mid-generation (#790)",
        ),
        claude: S::Supported,
        codex: unsupported(notes::NO_PEER_INBOX),
        agy: unsupported(notes::NO_PEER_INBOX),
        local: unsupported(notes::NO_PEER_INBOX),
        evidence: AgentEvidence::ByDesign(
            "第 1 層は claude の Cross-Session Messaging（受信箱の socket へ直送）に固有。\
             AGENTS.md「worker への指示送達（#790）」も codex / agy / Windows は常に \
             第 2 層と明記している",
        ),
    },
    AgentFeature {
        key: keys::WORKER_LIMIT_AUTORESUME,
        summary: Note::new(
            "利用上限の解除後に自分で再開する（#813）",
            "Work resumes by itself once the usage limit resets (#813)",
        ),
        claude: S::Supported,
        codex: degraded(notes::CODEX_CREDITS_NEED_PURCHASE),
        agy: unsupported(notes::AGY_NO_LIMIT_RESET),
        local: unsupported(notes::NO_LOCAL_USAGE_LIMIT),
        evidence: AgentEvidence::Measured(
            "#985 実測（2026-08-27 / codex-cli 0.150.1）: codex の解除時刻は 2 つの経路で \
             取れる。① 画面の `Try again at Aug 28th, 2026 4:24 AM.`（バイナリ内書式 \
             `\" Try again at \"` + `\", %Y %-I:%M %p\"`。日付を挟む形は #985 前は読めず、\
             不明の猶予 900 秒で早撃ちして 3 回で諦めていた）② rollout の \
             `rate_limits.<枠>.resets_at`（epoch 秒。書式にもタイムゾーンにも依存しない）。\
             セルフテスト項目 111 の codex 節が解除前は撃たず解除後に再開するところまで見る \
             （`TAKO_985_LEGACY=1` へ戻すと reset_at=None で FAILED になることを実測）。\
             agy 1.1.22 は `/credits` に「待つ」出口が無く（Get More AI Credits / See Activity）、\
             待って再開する動作そのものが成立しない",
        ),
    },
    AgentFeature {
        key: keys::WORKER_LIMIT_DETECT,
        summary: Note::new(
            "利用上限で止まったことを検知する",
            "Detects that the agent has stopped at a usage limit",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: unsupported(notes::AGY_CREDITS_NOT_WINDOWED),
        local: unsupported(notes::NO_LOCAL_USAGE_LIMIT),
        evidence: AgentEvidence::Measured(
            "#985 実測（2026-08-27）: codex 0.150.1 の停止文言 `You've hit your usage limit.` と \
             接近ダイアログ `Approaching rate limits` をバイナリ内文字列で確認し、\
             `limit_stop.rs` の実採取 fixture が両方を検知することをテストで固定した。\
             agy 1.1.22 は**窓つきの利用上限を持たない**（`agy --help` に usage / quota 系の \
             サブコマンドが無く、バイナリの `RateLimit` は全部 PR レビュー設定と \
             Go / sentry の内部名。残量は `/credits` = 前払いクレジット）ので、\
             検知すべき「上限で止まった状態」自体が存在しない",
        ),
    },
    AgentFeature {
        key: keys::WORKER_LIMIT_METRICS,
        summary: Note::new(
            "利用制限の残量（%）を取り出す（#357）",
            "Usage-limit headroom (%) can be read (#357)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: unsupported(notes::AGY_CREDITS_NOT_WINDOWED),
        local: unsupported(notes::NO_LOCAL_USAGE_LIMIT),
        evidence: AgentEvidence::Measured(
            "#985 実測（2026-08-27 / codex-cli 0.150.1 / plan_type = plus = **有料プラン**）: \
             rollout の `token_count` に `rate_limits.primary`（`window_minutes: 300` = 5h）と \
             `.secondary`（`10080` = 週）が数値で載る。**#357 の画面スクレイピングは \
             0.150.1 では成立しない**（実測: TUI のフッターはモデル名と cwd だけで、\
             `5h limit: [██…] 90% left (resets 23:23)` は `/status` のモーダルの中にしか \
             出ない = 常時見えるところに `primary NN%` は無い）ので、構造化ソースが正になった。\
             両者の解除時刻が一致することも確認（rollout の 1787840583 = 画面の 23:23）。\
             agy 1.1.22 は前払いクレジットで枠が無い（`/credits` を実行して確認）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_MCP,
        summary: Note::new(
            "worker が tako の MCP ツール群を呼べる",
            "The worker can call tako's MCP tools",
        ),
        claude: S::Supported,
        codex: pending(notes::NOT_WIRED, 986),
        agy: pending(notes::NOT_WIRED, 986),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "mcp_servers を組む非テストコードは orchestrator/mod.rs（master 経路）だけで、\
             WorkerLaunch には tako_bin も MCP 引数も無い（棚卸し §5.3 = 最大の穴）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_PERMISSION_DIALOG,
        summary: Note::new(
            "permission ダイアログを検知して応答する（#319 / #577）",
            "Permission dialogs are detected and answered (#319 / #577)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: pending(notes::LOCAL_HARNESS_UNDECIDED, 991),
        evidence: AgentEvidence::Source(
            "claude_tui.rs の detect_permission_dialog は 3 系統のパターンを持ち、\
             agy の「Do you want to proceed?」も対象に入っている",
        ),
    },
    AgentFeature {
        key: keys::WORKER_PROMPT_DELIVERY,
        summary: Note::new(
            "初期プロンプトが送達確認つきで届く（#32 / #530）",
            "The initial prompt is delivered with confirmation (#32 / #530)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "第 2 層のキー操作経路（claude_tui::deliver_via_tmux）は 3 系統の入力欄 \
             （❯ / › / >）を見分けるので agent 非依存に動く",
        ),
    },
    AgentFeature {
        key: keys::WORKER_PROMPT_UNDELIVERED,
        summary: Note::new(
            "プロンプトが届かなかったことを検知して再送手段を出す（#390 / #530）",
            "A prompt that never arrived is detected and a resend path is offered (#390 / #530)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: degraded(notes::SCREEN_ONLY_DELIVERY),
        local: local_pending_first_class(),
        evidence: AgentEvidence::UnitTest(
            "#983 の変更 2 で prompt_delivery_assessment の判断を delivery_observation \
             （このマトリクスの WORKER_STATUS_STRUCTURED）から引く形にした。codex は rollout の \
             task_started を送達の証拠にできるので claude と同じく未達を断定し、agy は画面確認しか \
             無いので未達ではなく unverified（+ verify_then_resend）を返す。緑のテスト: \
             registry の「一次シグナルの無い系統は未達と断定せず未確認を返す」\
             「送達の観測手段はマトリクスから引く」「ターンが走った証拠は画面検証の失敗より強い」/ \
             dispatch の「issue983_観測手段の無い系統でも送達判定が黙らない」",
        ),
    },
    AgentFeature {
        key: keys::WORKER_REPORT_SCROLLBACK,
        summary: Note::new(
            "画面の履歴から報告を取れる（#364 の第 1 層）",
            "Reports can be pulled from the scrollback (first layer of #364)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending_first_class(),
        evidence: AgentEvidence::Source(
            "第 1 層は器の capture（capture-pane -p -J -S）なので agent に依らない \
             （dispatch.rs の report が明記）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_REPORT_TRANSCRIPT,
        summary: Note::new(
            "構造化された会話ログから報告を取れる（#364 の第 2 層 / `--messages`）",
            "Reports can be pulled from the structured transcript (second layer of #364)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_CONVERSATION_SQLITE, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Measured(
            "#984 で codex アダプタを実装。rollout JSONL の response_item（role=assistant）を \
             読むので `report --messages N` が codex でも実データを返す。応答の \
             transcript_agent でどちらを読んだか分かる。agy は会話が SQLite なので未対応",
        ),
    },
    AgentFeature {
        key: keys::WORKER_SPAWN,
        summary: Note::new(
            "worker として起動できる",
            "Can be spawned as a worker",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: S::Supported,
        local: local_pending(),
        evidence: AgentEvidence::Source(
            "orchestrator/agent.rs の build_worker_cmd_in が唯一の組み立て口で、\
             effort / 権限スキップ / role 注入の 3 点だけを系統別に分岐する",
        ),
    },
    AgentFeature {
        key: keys::WORKER_STATUS_DETECT,
        summary: Note::new(
            "作業中か終わったかを判定する",
            "Determines whether the agent is working or done",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: degraded(notes::SCREEN_ONLY_STATUS),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Measured(
            "#984: codex は構造化ソース（codex-session）を得たので need_streak が 8 → 3 に \
             なり claude と同じ確定速度になる。同一タスクの A/B 実測（primes 25 個）で \
             before = source=screen / ctx=None / **開始前の t=3s・6s に idle を出す**、\
             after = t=9s から source=codex-session で busy を 2 標本とも捉え t=15s から \
             idle + ctx=8。agy は画面推定のままだが、弱マーカーを agent 別に分離したので \
             (Thinking) 型の誤爆は構造的に起こらない（残る差は確定までの回数だけ）",
        ),
    },
    AgentFeature {
        key: keys::WORKER_STATUS_STRUCTURED,
        summary: Note::new(
            "画面に依らない一次シグナルで状態を取れる（`claude agents --json` 相当）",
            "State is available from a primary signal that does not depend on the screen (equivalent to `claude agents --json`)",
        ),
        claude: S::Supported,
        codex: S::Supported,
        agy: pending(notes::AGY_CONVERSATION_SQLITE, 984),
        local: local_pending_first_class(),
        evidence: AgentEvidence::Measured(
            "#984 で codex-cli 0.150.1 を実物調査: $CODEX_HOME/sessions/ の rollout JSONL に \
             task_started / task_complete が**逐次**書かれる（250 語生成を 1 秒刻みで観測: \
             t=1s 開始 → t=27s 完了）。tako は status_source=codex-session として読む。\
             agy は会話が SQLite（~/.gemini/antigravity-cli/conversations/<id>.db）で、\
             生存は presence/<id>.lock で分かるがターンの開始・完了は取れない",
        ),
    },
    AgentFeature {
        key: keys::WORKER_TRUST,
        summary: Note::new(
            "作業フォルダを起動前に信頼済みにしておく（信頼ダイアログで止まらない）",
            "The working folder is marked trusted before launch, so no trust dialog blocks start-up",
        ),
        claude: S::Supported,
        codex: degraded(notes::FIXED_CONFIG_PATH),
        agy: degraded(notes::FIXED_CONFIG_PATH),
        local: pending(notes::LOCAL_HARNESS_UNDECIDED, 990),
        evidence: AgentEvidence::Source(
            "orchestrator/agent.rs の ensure_trusted_in が 3 系統ぶん書き分けてある \
             （claude = <config dir>/.claude.json / codex = ~/.codex/config.toml / \
             agy = ~/.gemini/antigravity-cli/settings.json）。claude 以外は固定パス",
        ),
    },
];

// ─── 既存 enum との相互変換（tako-core の中にあるぶん） ─────────────────
//
// **変換をこのファイルへ置く**ことで、既存 enum のファイルを 1 行も触らずに
// 正本へ寄せられる（並行作業との衝突を避ける狙いもある）。
// tako-control / tako-cli 側の enum は各クレートで変換を持つ（tako-core は
// 下位レイヤなので上のクレートの型を見られない）。対応関係の一覧は
// `.agent/agent-enums.md`、機械検証は `crates/tako-control/tests/agent_parity.rs`。

impl From<crate::terminal::LimitService> for Agent {
    fn from(v: crate::terminal::LimitService) -> Self {
        use crate::terminal::LimitService as L;
        match v {
            L::Claude => Self::Claude,
            L::Codex => Self::Codex,
            L::Agy => Self::Agy,
        }
    }
}

impl From<crate::platform::agent_install::AgentKind> for Agent {
    fn from(v: crate::platform::agent_install::AgentKind) -> Self {
        use crate::platform::agent_install::AgentKind as K;
        match v {
            K::Claude => Self::Claude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Lang};

    /// **根拠なき判定を構造的に禁止する**（`platform::support` の T7 と同じ狙い）。
    ///
    /// この宣言は最終的に system prompt へ流れる（#992）。甘い宣言は「使える」と
    /// 信じたエージェントを失敗させ続け、辛い宣言は使える機能を回避させる。
    /// **claude 以外を断定するなら根拠を書く**。書けないなら `Pending` のままにする
    #[test]
    fn t7_claude以外の判定には根拠が要る() {
        for f in MATRIX {
            if f.asserts_non_baseline() {
                assert!(
                    f.evidence != AgentEvidence::Unverified,
                    "{} は claude 以外について「使える / 縮退している / 使えないと分かっている」と\
                     宣言しているのに根拠が無い。コード本文の引用（file / 関数名）・実機で緑の\
                     テスト名・実測の記録のどれかを evidence へ書くこと\
                     （書けないなら Pending のままにする）",
                    f.key
                );
            }
            if let Some(citation) = f.evidence.citation() {
                assert!(!citation.trim().is_empty(), "{} の根拠が空文字", f.key);
            }
        }
    }

    /// T7 の対: 未確認なら追跡先が要る。
    /// 「調べていないし誰も追いかけていない」マスを残さない
    #[test]
    fn t7_未確認の行は追跡issueを持つ() {
        for f in MATRIX {
            if f.evidence != AgentEvidence::Unverified {
                continue;
            }
            for agent in [Agent::Codex, Agent::Agy, Agent::Local] {
                let AgentSupport::Pending { issue, .. } = f.on(agent) else {
                    continue; // 上のテストが落とす
                };
                assert!(
                    issue != 0,
                    "{} / {} は未確認なのに追跡 Issue が無い",
                    f.key,
                    agent.as_str()
                );
            }
        }
    }

    /// **「上流に手段が無い」と「まだ調べていない」を混ぜない**。
    ///
    /// 未調査を `Unsupported` へ倒すと、実際には open な道をエージェントが
    /// 永久に避けるようになる（#985 が agy の limit について警告しているのと同じ罠）。
    /// `Unsupported` は上流の仕様・設計判断・実地調査のどれかで裏を取ること
    #[test]
    fn 未調査をunsupportedへ倒していない() {
        for f in MATRIX {
            for agent in [Agent::Codex, Agent::Agy, Agent::Local] {
                let AgentSupport::Unsupported { note } = f.on(agent) else {
                    continue;
                };
                assert_ne!(
                    note,
                    notes::NOT_INVESTIGATED,
                    "{} / {} が「未調査」の理由で Unsupported になっている。\
                     調べていないものは Pending（追跡 Issue つき）に置くこと",
                    f.key,
                    agent.as_str()
                );
                assert!(
                    matches!(
                        f.evidence,
                        AgentEvidence::ByDesign(_)
                            | AgentEvidence::Measured(_)
                            | AgentEvidence::Source(_)
                    ),
                    "{} / {} を Unsupported と断定するなら、上流の仕様（ByDesign）・\
                     実地調査（Measured）・コード本文（Source）のどれかで裏を取ること",
                    f.key,
                    agent.as_str()
                );
            }
        }
    }

    /// 縮退には必ず理由が要る。`Pending` には追跡先も要る
    #[test]
    fn 縮退には理由と追跡先が必須() {
        for f in MATRIX {
            for agent in Agent::ALL {
                let s = f.on(agent);
                if let AgentSupport::Pending { issue, .. } = s {
                    assert!(
                        issue != 0,
                        "{} / {} が Pending なのに追跡 Issue が無い",
                        f.key,
                        agent.as_str()
                    );
                }
                if !matches!(s, AgentSupport::Supported) {
                    assert!(
                        s.note().is_some(),
                        "{} / {} は Supported ではないのに理由が無い",
                        f.key,
                        agent.as_str()
                    );
                }
            }
        }
    }

    /// キーの重複と並び順。順序を固定しておくと差分レビューが読める
    #[test]
    fn キーは一意で昇順() {
        let keys: Vec<&str> = MATRIX.iter().map(|f| f.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "MATRIX のキーは昇順で並べること");
        let mut dedup = sorted.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), keys.len(), "MATRIX のキーが重複している");
    }

    /// `keys` の定数と MATRIX が**両方向**で一致すること。
    /// 呼び出し側は定数しか使えないので、定数だけあって行が無い（= 素通り）を防ぐ
    #[test]
    fn keysの定数とマトリクスが一対一() {
        // keys モジュールの定数はソースから拾う（マクロを増やさずに両方向を見る）
        let src = include_str!("agent_support.rs");
        let mod_start = src
            .find("pub mod keys {")
            .expect("keys モジュールが見つからない");
        let mod_src = &src[mod_start..];
        let mod_end = mod_src
            .find("\n}\n")
            .expect("keys モジュールの終端が見つからない");
        let mut declared: Vec<&str> = Vec::new();
        for line in mod_src[..mod_end].lines() {
            let Some(rest) = line.trim().strip_prefix("pub const ") else {
                continue;
            };
            let Some(value) = rest.split('=').nth(1) else {
                continue;
            };
            declared.push(value.trim().trim_end_matches(';').trim_matches('"'));
        }
        let registered: Vec<&str> = MATRIX.iter().map(|f| f.key).collect();
        let mut missing: Vec<&str> = declared
            .iter()
            .copied()
            .filter(|k| !registered.contains(k))
            .collect();
        let mut extra: Vec<&str> = registered
            .iter()
            .copied()
            .filter(|k| !declared.contains(k))
            .collect();
        missing.sort_unstable();
        extra.sort_unstable();
        assert!(
            missing.is_empty(),
            "keys に定数はあるのに MATRIX へ登録されていない: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "MATRIX にあるのに keys の定数が無い（呼び出し側が文字列を直書きすることになる）: {extra:?}"
        );
        assert!(
            declared.len() >= 20,
            "keys の抽出が壊れている（{} 件）",
            declared.len()
        );
    }

    /// 理由文・説明文の日英が両方埋まっていて、**英語に日本語が混ざっていない**こと
    /// （#435 の i18n 要件。マトリクスは docs にも出るので混在は目に見える）
    #[test]
    fn 理由文と説明は日英そろっている() {
        let has_cjk = |s: &str| {
            s.chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF))
        };
        for f in MATRIX {
            let mut all = vec![f.summary];
            all.extend(Agent::ALL.iter().filter_map(|a| f.on(*a).note()));
            for note in all {
                assert!(
                    !note.ja().trim().is_empty() && !note.en().trim().is_empty(),
                    "{} の文言に空がある",
                    f.key
                );
                assert!(
                    !has_cjk(note.en()),
                    "{} の英語文言に日本語が混ざっている: {}",
                    f.key,
                    note.en()
                );
                assert_ne!(note.ja(), note.en(), "{} の日英が同一", f.key);
            }
        }
    }

    /// claude は tako の実装基準。**基準系が縮退している行を黙って作らない**
    /// （作るなら claude 側の未実装なので、それ自体が別の Issue になる）
    #[test]
    fn claudeは基準系なので全て対応済み() {
        for f in MATRIX {
            assert_eq!(
                f.claude,
                AgentSupport::Supported,
                "{} は基準系の claude が Supported ではない。\
                 claude 自身の未実装ならその旨の Issue を立てて、ここへ理由と追跡先を書くこと",
                f.key
            );
        }
    }

    /// 未登録キーは素通しする（登録漏れで機能が止まらない）
    #[test]
    fn 未登録キーは素通しする() {
        assert!(supports(Agent::Agy, "存在しない能力"));
        assert!(gate(Agent::Agy, "存在しない能力").is_ok());
        assert_eq!(support_for(Agent::Agy, "存在しない能力"), None);
    }

    /// 構造化された状態の出口を持つ系統（`has_structured_status` の参照先）。
    ///
    /// #982 の時点では claude だけだったが、**#984 で codex も持つことが分かった**
    /// （rollout JSONL の `task_started` / `task_complete`。実測）。
    /// agy は会話が SQLite なので未対応
    #[test]
    fn 構造化シグナルはclaudeとcodex() {
        assert!(supports(Agent::Claude, keys::WORKER_STATUS_STRUCTURED));
        assert!(supports(Agent::Codex, keys::WORKER_STATUS_STRUCTURED));
        assert!(!supports(Agent::Agy, keys::WORKER_STATUS_STRUCTURED));
        assert!(!supports(Agent::Local, keys::WORKER_STATUS_STRUCTURED));
    }

    #[test]
    fn 状態で絞り込める() {
        let all = features(Agent::Codex, None).len();
        let pending = features(Agent::Codex, Some("pending")).len();
        assert_eq!(all, MATRIX.len());
        assert!(pending > 0 && pending < all);
        assert!(features(Agent::Claude, Some("pending")).is_empty());
        // 4 状態の合計が全件になる（status() の網羅性）
        let sum: usize = ["supported", "degraded", "pending", "unsupported"]
            .iter()
            .map(|s| features(Agent::Agy, Some(s)).len())
            .sum();
        assert_eq!(sum, all);
    }

    #[test]
    fn 縮退理由の一覧は重複しない() {
        let notes = degraded_note_items(Agent::Agy);
        let mut dedup = notes.clone();
        dedup.dedup();
        assert_eq!(notes.len(), dedup.len());
        assert!(!notes.is_empty());
        assert!(degraded_note_items(Agent::Claude).is_empty());
    }

    /// 診断メッセージが表示言語に追従すること（グローバル追従の検査なので直列化する）
    #[test]
    fn gateの診断は表示言語に追従する() {
        let _guard = i18n::testing::lang_guard();
        let key = keys::WORKER_MCP;
        i18n::set_lang(Lang::En);
        let en = gate(Agent::Codex, key).unwrap_err();
        i18n::set_lang(Lang::Ja);
        let ja = gate(Agent::Codex, key).unwrap_err();
        assert!(
            !en.chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
            "英語の診断に日本語が残っている: {en}"
        );
        assert!(en.contains("#986") && ja.contains("#986"));
        assert!(en.contains("OpenAI Codex CLI") && ja.contains("OpenAI Codex CLI"));
    }

    #[test]
    fn 種別名は往復する() {
        for a in Agent::ALL {
            assert_eq!(Agent::parse(a.as_str()), Some(a));
            assert_eq!(Agent::parse(&a.as_str().to_uppercase()), Some(a));
        }
        assert_eq!(Agent::parse("gemini"), None);
        assert!(Agent::Claude.is_baseline());
        assert!(!Agent::Codex.is_baseline());
    }

    /// tako-core 内の既存 enum との対応。**値が増減したらここが落ちる**
    /// （網羅 match なので、変換先を足さないとコンパイルが通らない）
    #[test]
    fn tako_coreの既存enumが正本へ写る() {
        use crate::platform::agent_install::AgentKind;
        use crate::terminal::LimitService;

        // LimitService は TUI 3 系統と 1:1
        let from_limit: Vec<Agent> = LimitService::ALL.iter().map(|v| Agent::from(*v)).collect();
        assert_eq!(from_limit, Agent::TUI.to_vec());
        for v in LimitService::ALL {
            assert_eq!(Agent::from(v).as_str(), v.as_str());
        }
        // agent_install は claude 1 値のみ（拡張は #989）
        assert_eq!(Agent::from(AgentKind::Claude), Agent::Claude);
        assert_eq!(
            support_for(Agent::Codex, keys::SETUP_CLI_INSTALL).map(|s| s.status()),
            Some("pending"),
            "agent_install が codex へ拡張されたらマトリクスも更新すること"
        );
    }
}

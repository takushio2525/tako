//! ゼロスタート導入（#868 → #989 で 3 系統へ）
//!
//! 「エージェント CLI を入れたことがない人」が `tako setup` 一発で始められるようにする。
//! 検出型の setup（導入済み CLI を見つけて最適化する）の**手前**に、
//! 導入そのものを引き受ける段を足す。
//!
//! ## 段（順に進む。すべて冪等で、途中から再開できる）
//!
//! 1. [`Step::Install`] — エージェント CLI を公式インストーラで導入する
//! 2. [`Step::Path`] — ランチャーの置き場所をログインシェルの PATH へ通す
//! 3. [`Step::Auth`] — その系統のログインコマンドの実行を**ユーザーへ依頼する**
//!    （tako は代行しない。#1129 の理由は [`auth_instructions_for`]）
//! 4. [`Step::Ready`] — 既存の検出型 setup へ引き継ぐ
//!
//! ## agent を引数で受ける（#989）
//!
//! 4 段の形は 3 系統（claude / codex / agy）で同じで、違うのは
//! **手順・ログインコマンド・認証の確かめ方**だけ。だから段の実装は 1 本にして、
//! 系統は [`AgentKind`] を**引数で受ける**。こうすると
//! 「claude だけ通る経路」が構造的に作れない（#989 の再発防止）。
//!
//! 引数なしの旧 API（[`status`] / [`install`] / [`ensure_path`] …）は
//! **残していない**。claude 既定の入口を残すと、新しい機能がそこへ吸い寄せられて
//! また 1 系統だけ通る形に戻る（`AgentKind` が Claude 1 値だった理由がそれ）
//!
//! ## 設計の要点
//!
//! - **手順の正本はプラットフォーム境界**（[`tako_core::platform::agent_install`]）。
//!   ここは「いまどの段か」を判定して実行するだけで、URL やパスを持たない
//! - **失敗は黙って飲まない**。どの段で何が起きたかを [`BootstrapError`] の
//!   具体的な文面にして返す（受け入れ条件 3）
//! - **入れる前に何をどこに入れるか出す**（[`InstallPlan`]。受け入れ条件 4）
//! - Homebrew 自体が無い場合は**案内だけ**にする。Homebrew のインストーラは
//!   sudo でパスワードを求める（実物で確認: 2026-08-21 時点の install.sh に
//!   sudo 参照 49 箇所・`have_sudo_access`）。setup が黙って権限昇格を走らせない

use crate::orchestrator::agent::WorkerAgent;
use crate::setup_refusal::{FetchDetail, PathByHandNote};
use crate::SetupRefusal;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tako_core::platform::agent_install::{self, AgentKind, InstallRecipe};
use tako_core::platform::support::Platform;
use tako_core::platform::user_path;
use tako_core::shell_profile::{self, PathChange, ShellKind};
use tako_core::tako_cli_path;

/// 導入の進み具合。`status()` が「次に何をすべきか」として返す
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// CLI が無い（か、あるが自動導入できないプラットフォーム）
    Install,
    /// CLI はあるが PATH から引けない
    Path,
    /// CLI はあるが未認証
    Auth,
    /// 検出型 setup へ進める
    Ready,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Path => "path",
            Self::Auth => "auth",
            Self::Ready => "ready",
        }
    }

    /// 利用者向けの 1 行説明（**どの系統の話かを必ず含める**。#989）。
    ///
    /// 引数なしの `describe()` は置いていない。3 系統を並べて出す画面で
    /// 「Claude Code をインストールします」が codex の段に出ると嘘になる
    pub fn describe_for(self, agent: AgentKind) -> String {
        match self {
            Self::Install => format!("{} をインストールします", agent.product()),
            Self::Path => format!(
                "{} コマンドをどのターミナルからも使えるようにします",
                agent.as_str()
            ),
            Self::Auth => format!("{}にログインします", account_label(agent)),
            Self::Ready => "導入は済んでいます".to_string(),
        }
    }
}

/// 認証の相手（誰のアカウントへログインするのか）。
///
/// 実測で確かめた呼び名だけを書く: codex は `codex login status` が
/// `Logged in using ChatGPT` を返す / agy は公式 docs の sign-in が Google アカウント
pub fn account_label(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "Claude アカウント",
        AgentKind::Codex => "ChatGPT アカウント",
        AgentKind::Agy => "Google アカウント",
    }
}

/// ゼロスタート導入が面倒を見る系統。
///
/// `TAKO_989_LEGACY=1` で **claude 1 系統だけ**（#989 前）へ戻る
/// = 同一バイナリで A/B が取れる
pub fn bootstrap_agents() -> Vec<AgentKind> {
    if legacy_single_agent() {
        return vec![AgentKind::Claude];
    }
    AgentKind::ALL.to_vec()
}

/// `TAKO_989_LEGACY=1` で #989 前（claude 専用のゼロスタート導入）へ戻す
pub fn legacy_single_agent() -> bool {
    std::env::var_os("TAKO_989_LEGACY").is_some()
}

/// コマンドへ付ける `--agent` 断片。**claude は既定なので付けない**
/// （#322 の「最も簡単なコマンドを提案する」原則。CLI の案内文と
/// 引き継ぎの指示文が同じ形になるよう正本をここに置く）
pub fn agent_flag(agent: AgentKind) -> String {
    match agent {
        AgentKind::Claude => String::new(),
        other => format!(" --agent {}", other.as_str()),
    }
}

/// 文字列から系統を決める。**未知の名前は黙って claude にしない**
/// （取り違えたまま別の系統を入れてしまうため）。
///
/// 断りの文は日本語のまま（呼び手は CLI の `tako setup bootstrap` = 枠が日本語の手元の処理）。
/// 断りを表示言語で返す dispatch（MCP の `tako_setup_bootstrap`）は [`parse_agent_typed`] を通す（#2365）
pub fn parse_agent(value: &str) -> Result<AgentKind, String> {
    parse_agent_typed(value).map_err(SetupRefusal::into_ja)
}

/// [`parse_agent`] の型の口（#2365）
pub fn parse_agent_typed(value: &str) -> Result<AgentKind, SetupRefusal> {
    AgentKind::parse(value).ok_or_else(|| SetupRefusal::UnknownBootstrapAgent {
        got: value.to_string(),
        allowed: AgentKind::ALL
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
    })
}

/// 「何をどこに入れるか」。**実行前に必ずこれを見せる**（受け入れ条件 4）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub agent: &'static str,
    /// 利用者へ見せる製品名（「何を入れるのか」）。コマンド名だけだと
    /// 3 系統を並べたときにどれの話か分からない（#989）
    pub product: &'static str,
    /// 手順の対象プラットフォーム。**呼び名の出し分けに使う**（#925）。
    ///
    /// `cfg!` で分岐せず**値として持ち回す**のは #905（スリープ防止の呼び名）と同じ判断で、
    /// これにより **macOS 上から Windows 側の文面を検査できる**（#515）。
    /// 出どころは [`InstallRecipe::platform`] なので、手順と文面がずれることが無い
    pub platform: Platform,
    /// 公式ドキュメントに載っているコマンド（利用者が手で打っても同じ結果になる形）
    pub official_command: String,
    /// 取得元 URL
    pub source_url: String,
    /// コマンド本体の置き場所
    pub launcher: PathBuf,
    /// 実体（バージョンごと）の置き場所。**単一バイナリの系統は `None`**
    pub payload: Option<PathBuf>,
    /// バックグラウンド自動更新が効くか（機械が読む側）
    pub auto_updates: bool,
    /// 以後どう更新されるかの 1 行（人が読む側。出どころは [`InstallRecipe::update_note`]）
    pub update_note: &'static str,
    /// tako が実行を代行できるか。false = 手順を案内するだけ
    pub can_run: bool,
}

impl InstallPlan {
    /// 表示用の行（CLI・GUI・MCP のどこから出しても同じ文面になるよう 1 か所で作る）
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("入れるもの: {}（{} コマンド）", self.product, self.agent),
            format!("実行するコマンド: {}", self.official_command),
            format!("取得元: {}", self.source_url),
            format!("コマンドの置き場所: {}", display_path(&self.launcher)),
        ];
        // 単一バイナリの系統（agy）はランチャー自身が本体なので、
        // 同じパスを 2 回言わない
        if let Some(payload) = &self.payload {
            out.push(format!("本体の置き場所: {}", display_path(payload)));
        }
        out.push(format!("以後の更新: {}", self.update_note));
        out.push(privilege_line(self.platform));
        out
    }

    /// この構成で**画面に出る文字列すべて**（導入計画の表示 + 引き継ぎの指示文）。
    ///
    /// 「その OS で通じない語が出ていない」を GUI も実機も無しに `cargo test` で
    /// 固定するための入口（#925。#905 の `visible_texts` と同じ役目）
    pub fn visible_texts(&self) -> Vec<String> {
        let mut out = self.lines();
        out.extend(handoff_prompt_text(self, None).lines().map(str::to_string));
        out
    }

    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "product": self.product,
            "platform": self.platform.as_str(),
            "official_command": self.official_command,
            "source_url": self.source_url,
            "launcher": self.launcher.display().to_string(),
            "payload": self.payload.as_ref().map(|p| p.display().to_string()),
            "auto_updates": self.auto_updates,
            "update_note": self.update_note,
            "can_run": self.can_run,
            "lines": self.lines(),
        })
    }
}

/// 依存ツール 1 件の状態（tmux 等）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepState {
    pub bin: String,
    pub found: Option<String>,
    pub required: bool,
}

/// いまの導入状況
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapState {
    pub agent: AgentKind,
    /// 解決できた実行ファイル（PATH 外のランチャーも含む）
    pub binary: Option<String>,
    pub authenticated: bool,
    /// 認証済みのときの正規化済みプラン名（#1505。未認証 / 取得できない系統は None）
    pub account_plan: Option<String>,
    /// ランチャーの置き場所が PATH に入っているか
    pub launcher_dir: PathBuf,
    /// 「新しく開いたターミナル（tako の新しいタブを含む）が見る PATH」に入っているか
    pub launcher_dir_on_path: bool,
    /// **いまのプロセスの PATH** に入っているか（#2428）。`tako setup` をペインから打ったなら、
    /// そのタブで引けるか。`launcher_dir_on_path` だけが立っているなら、新しいタブを開けば引ける
    pub launcher_dir_on_process_path: bool,
    /// PATH 追記の書き先（判定できないシェルなら None）
    pub profile: Option<PathBuf>,
    pub profile_has_block: bool,
    pub shell: Option<ShellKind>,
    pub step: Step,
    pub plan: InstallPlan,
    /// 認証の問い合わせを上限で打ち切った（= 未認証ではなく**確かめられなかった**。#2255）。
    /// 上限を持つのは環境診断の経路（[`status_all_within`]）だけで、setup の経路は常に `None`
    pub auth_timeout: Option<tako_core::probe::TimeoutNotice>,
    /// 実行ファイルの検出（ログインシェルの `command -v`）を上限で打ち切った（#1533）。
    /// 立っていて `binary` が無ければ、未導入ではなく**導入済みかを確かめられなかった**
    pub locate_timeout: Option<tako_core::probe::TimeoutNotice>,
    /// ログインシェルの PATH の読み取りを上限で打ち切った（#1532）。
    /// 立っていて `launcher_dir_on_path` が false なら、PATH に無いのではなく**確かめられなかった**
    pub path_timeout: Option<tako_core::probe::TimeoutNotice>,
}

impl BootstrapState {
    /// この状態の段の 1 行説明（**自分の系統で**組む。#989）
    pub fn step_description(&self) -> String {
        self.step.describe_for(self.agent)
    }

    /// 段（未導入 / PATH に無い）がログインシェルの打ち切りのせいで決まっているときの知らせ
    /// （#1532 / #1533）。`Some` なら、その段は**確かめられなかった**のであって事実ではない。
    /// 呼び手はそれを「未導入」「PATH に無い」と言わず、導入や PATH の書き込みへ進まない。
    /// 認証の打ち切り（[`Self::auth_timeout`]）は段の判定とは別に読む（#2255）
    pub fn step_unconfirmed(&self) -> Option<&tako_core::probe::TimeoutNotice> {
        match self.step {
            Step::Install => self.locate_timeout.as_ref(),
            Step::Path => self.path_timeout.as_ref().or(self.locate_timeout.as_ref()),
            Step::Auth | Step::Ready => None,
        }
    }

    /// このタブでは引けないが、tako の新しいタブなら引ける（#2428）。
    /// 立つのは新しいペインがレジストリの PATH を読み直す環境（Windows）だけ。unix の判定と文面は変えない
    pub fn needs_new_tab(&self) -> bool {
        self.needs_new_tab_in(user_path::rereads_for_new_panes())
    }

    /// [`Self::needs_new_tab`] の本体（読み直すかを引数で受ける**純粋関数**）
    fn needs_new_tab_in(&self, rereads: bool) -> bool {
        rereads
            && self.binary.is_some()
            && self.launcher_dir_on_path
            && !self.launcher_dir_on_process_path
    }

    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent.as_str(),
            "product": self.agent.product(),
            "installed": self.binary.is_some(),
            "binary": self.binary,
            "authenticated": self.authenticated,
            "account_plan": self.account_plan,
            "launcher_dir": self.launcher_dir.display().to_string(),
            "launcher_dir_on_path": self.launcher_dir_on_path,
            // #2428: いまのプロセスで引けるか / 新しいタブを開けば引けるか
            "launcher_dir_on_process_path": self.launcher_dir_on_process_path,
            "needs_new_tab": self.needs_new_tab(),
            "profile": self.profile.as_ref().map(|p| p.display().to_string()),
            "profile_has_block": self.profile_has_block,
            "shell": self.shell.map(ShellKind::as_str),
            "next_step": self.step.as_str(),
            "next_step_description": self.step_description(),
            // #1532 / #1533: 段がログインシェルの打ち切りで決まっている（null = 確かめた答え）
            "next_step_unconfirmed": self.step_unconfirmed().map(timeout_json),
            "install_plan": self.plan.to_json(),
            "deps": deps_json(),
            "homebrew": homebrew_json(),
        })
    }
}

/// 打ち切りの知らせの JSON（環境診断の `probe_timeouts` と同じ形）
fn timeout_json(notice: &tako_core::probe::TimeoutNotice) -> Value {
    json!({ "label": notice.label, "waited_secs": notice.waited_secs })
}

/// ホームディレクトリ。**取得できない環境では何も書かない**
fn home_dir() -> Result<PathBuf, String> {
    home_dir_typed().map_err(SetupRefusal::into_ja)
}

/// [`home_dir`] の型の口（#2365）
fn home_dir_typed() -> Result<PathBuf, SetupRefusal> {
    tako_core::paths::home_dir().ok_or(SetupRefusal::BootstrapHomeUnresolved)
}

fn display_path(path: &Path) -> String {
    // `~` 短縮の正本は tako_core::paths::shorten_home（#893）
    tako_core::paths::shorten_home(&path.to_string_lossy())
}

/// この環境の手順（系統を引数で受ける。#989）
pub fn recipe_for(agent: AgentKind) -> InstallRecipe {
    agent_install::current_recipe(agent)
}

/// `TAKO_1057_LEGACY=1` で #1057 前の挙動へ戻す（**同一バイナリで A/B が取れる**）。
///
/// 戻るのは 3 点: ①Windows は自動インストールを代行しない ②PATH の判定に
/// `exe::find`（PATH 外まで走査）を使う ③引き継ぎ先を探さない
pub fn legacy_mode() -> bool {
    std::env::var_os("TAKO_1057_LEGACY").is_some()
}

/// 認証の段でユーザーへ出す案内（理由 + 次の一手）。**文面の正本はここ 1 箇所**。
///
/// ## tako が `claude auth login` を代行しない理由（#1129）
///
/// ブラウザ操作待ちのプロセスは**自分では終わらない**。tako が起こすと寿命の
/// 持ち主が居なくなり、Windows は子プロセスの終了要求（#1067 の境界 B5）が
/// 未実装なので、ペイン close も隔離インスタンスの終了も孫を回収しない。
/// 実機ではこれで `claude auth login` が 1 日で 46 本まで積み上がり、
/// `Win32_Processor.LoadPercentage` が 100% に張り付いた（#1129 の採取）。
///
/// 「人が見ているか」を stdin が端末かどうかで判別することはできない
/// （セルフテストがペインへ打ち込む `tako setup` も PTY 上では端末に見える）。
/// だから条件で絞るのではなく**構造的に起こさない**。
/// これは AGENTS.md / docs / MCP の説明（#1057「認証は代行させない」）と同じ契約で、
/// コードだけがそこから外れていた。
pub fn auth_instructions_for(agent: AgentKind) -> Vec<String> {
    let mut out = vec![
        format!("{}へのログインが必要です。", account_label(agent)),
        "ブラウザでの操作が要るため tako は代行しません。".to_string(),
    ];
    // ログインコマンドの正本は `agent_cli::auth_command`（3 系統とも実物で確認済み）。
    // **無い系統を推測で埋めない**ので、取れなければ次の 1 手の行を出さない
    match crate::orchestrator::agent_cli::auth_command(agent.into()) {
        Some(cmd) => {
            out.push(format!("次の 1 手: {cmd}"));
            // **ログインコマンドが CLI 名そのもの = 専用のサブコマンドが無い**
            // （agy がこれ。公式 docs: keyring に有効なセッションがあれば無言で通り、
            // 無ければ既定のブラウザが開く。SSH 先では URL とコードの手入力になる）。
            // 系統名で分岐せずコマンドの形から導くので、同じ形の CLI が増えても効く
            if cmd.trim() == agent.as_str() {
                out.push(format!(
                    "（{} は専用のログインコマンドを持たないので、引数なしの起動が入口です）",
                    agent.as_str()
                ));
            }
        }
        None => out.push(format!(
            "次の 1 手: {} の公式手順にしたがってログインしてください",
            agent.product()
        )),
    }
    out.push("ログインしたら tako setup をやり直してください".to_string());
    out
}

/// [`auth_instructions_for`] に、いまの導入状況から言えることを添えたもの（#2428）。
///
/// このタブからは引けないが新しいタブなら引けるとき、「次の 1 手」の**手前**に
/// 新しいタブを開く 1 行を入れる（打ってから「見つからない」と言われる前に読める位置）
pub fn auth_instructions_in(state: &BootstrapState) -> Vec<String> {
    with_new_tab_note(auth_instructions_for(state.agent), new_tab_note(state))
}

/// 案内の行へ新しいタブの 1 行を差し込む（**純粋関数**）
fn with_new_tab_note(mut lines: Vec<String>, note: Option<String>) -> Vec<String> {
    if let Some(note) = note {
        let at = lines
            .iter()
            .position(|line| line.starts_with("次の 1 手"))
            .unwrap_or(lines.len());
        lines.insert(at, note);
    }
    lines
}

/// [`BootstrapState::needs_new_tab`] のときに添える 1 行（**文面の正本はここ**。#2428）。
///
/// tako の新しいタブはレジストリの PATH を読み直す（`user_path::pane_path`）ので、
/// tako 自体の再起動は要らない。外のターミナルも、新しく開けば同じ値を読む
pub fn new_tab_note(state: &BootstrapState) -> Option<String> {
    state
        .needs_new_tab()
        .then(|| new_tab_note_text(state.agent))
}

fn new_tab_note_text(agent: AgentKind) -> String {
    format!(
        "このタブは {} の置き場所が PATH に入る前に開いたので、ここからはまだ引けません。\
         tako で新しいタブを開いてから打ってください（tako の再起動は要りません）",
        agent.as_str()
    )
}

/// `TAKO_1129_LEGACY=1` で修正前（tako が `claude auth login` を起こす）へ戻す。
/// 同一バイナリで A/B を取るためだけの逃げ道
pub fn legacy_auth_launch() -> bool {
    std::env::var_os("TAKO_1129_LEGACY").is_some()
}

/// 権限の説明（`Platform` を引数に取る純粋関数）。
///
/// `sudo` は **unix の概念で Windows では通じない**（#925）。Windows の昇格は UAC で、
/// 対応する呼び名が無いので手段の名前を出さず「管理者権限」だけで書く。
/// どちらの文面にも**プラットフォームに依らない語**「管理者権限」が入るので、
/// セルフテスト項目 119（#920）はこの出し分けを入れても落ちない
pub fn privilege_line(platform: Platform) -> String {
    if legacy_privilege_line() {
        return "sudo（管理者権限）は使いません。ホームディレクトリの中だけで完結します"
            .to_string();
    }
    match platform {
        Platform::MacOs => "sudo（管理者権限）は使いません。ホームディレクトリの中だけで完結します",
        Platform::Windows => "管理者権限は使いません。ホームディレクトリの中だけで完結します",
    }
    .to_string()
}

/// `TAKO_925_LEGACY=1` で修正前（プラットフォームに依らない `sudo` 固定の文面）へ戻す。
/// 同一バイナリで A/B を取るためだけの逃げ道（`legacy_auth_launch` と同じ形）
pub fn legacy_privilege_line() -> bool {
    std::env::var_os("TAKO_925_LEGACY").is_some()
}

/// 「何をどこに入れるか」
pub fn install_plan_for(agent: AgentKind) -> Result<InstallPlan, String> {
    install_plan_for_typed(agent).map_err(SetupRefusal::into_ja)
}

/// [`install_plan_for`] の型の口（#2365）
fn install_plan_for_typed(agent: AgentKind) -> Result<InstallPlan, SetupRefusal> {
    let home = home_dir_typed()?;
    Ok(plan_from(&recipe_for(agent), &home))
}

/// 手順 + home から計画を組む（**環境変数を読まない純粋関数**なので、
/// 隔離 HOME のテストがそのまま書ける）
pub fn plan_from(r: &InstallRecipe, home: &Path) -> InstallPlan {
    InstallPlan {
        agent: r.agent.as_str(),
        product: r.agent.product(),
        platform: r.platform,
        official_command: r.source.official_command.to_string(),
        source_url: r.source.url.to_string(),
        launcher: r.launcher_path_in(home),
        payload: r.payload_dir_in(home),
        auto_updates: r.auto_updates,
        update_note: r.update_note,
        can_run: r.tako_can_run,
    }
}

/// エージェント CLI を解決する。PATH に無くても**インストーラが置く場所**を見る。
///
/// インストール直後は profile へ書いた PATH が現プロセスにも
/// `$SHELL -l -c` にも反映されない（シェルを開き直すまで）。ここで拾わないと
/// 「入れたのに見つかりません」で setup が止まる
pub fn resolve_binary_for(agent: AgentKind) -> Option<String> {
    let r = recipe_for(agent);
    if let Some(found) = tako_core::platform::exe::find(r.agent.as_str()) {
        return Some(found);
    }
    installed_launcher_for(agent)
}

/// インストーラが置く場所だけを見る（**子プロセスを起こさない**。#1989）。
///
/// [`resolve_binary_for`] の後半。PATH の探索（unix はログインシェル）を上限つきで済ませた
/// 呼び手（`agent_cli::locate`）が、打ち切ったあとにもう一度上限なしのシェルを起こさないための口
pub fn installed_launcher_for(agent: AgentKind) -> Option<String> {
    let r = recipe_for(agent);
    let home = home_dir().ok()?;
    let launcher = r.launcher_path_in(&home);
    is_executable(&launcher).then(|| launcher.display().to_string())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 認証を確かめるために叩く引数（**3 系統の正本はここ 1 箇所**。#989）。
///
/// すべて実出力で確認した形だけを置く:
///
/// - claude: `claude auth status --json` → `{"loggedIn": true}`
/// - codex: `codex login status` → `Logged in using ChatGPT` / exit 0
/// - agy: `agy models` → 一覧 / exit 0（`agy` は認証判定用のサブコマンドを持たず、
///   未認証だと `Please sign in to view available models.` になる。#1002 の実測）
pub fn auth_probe_argv(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::Claude => &["auth", "status", "--json"],
        AgentKind::Codex => &["login", "status"],
        AgentKind::Agy => &["models"],
    }
}

/// 認証の問い合わせ 1 回ぶんの結果（#1505）。
///
/// 認証の有無と**プラン名を同じ 1 回の問い合わせから**読む。以前は
/// `is_authenticated_for`（bool だけ）と `tako setup` 側の検出（プランつき）が
/// 別々に CLI を起こしていたので、`tako setup --check` は 3 系統ぶんを 2 度ずつ叩いていた
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthState {
    pub authenticated: bool,
    /// 正規化済みプラン名。**個人識別子や token は保持しない**
    pub plan: Option<String>,
}

/// 認証済みか。**メールアドレスや組織名は読み捨てる**（診断ログへ個人情報を出さないため）。
///
/// claude だけ exit code では判断できない（未ログインでも 0 を返しうるので
/// `loggedIn` を見る）。他の 2 系統は exit code が判定そのもの
pub fn is_authenticated_for(agent: AgentKind, binary: &str) -> bool {
    auth_state_for(agent, binary).authenticated
}

/// 認証の有無とプラン名（上限なしで待つ = setup の経路。出力の解釈は `auth_state_from` の 1 か所）。
///
/// 環境診断（`tako check-health` / `tako setup --check`）からは呼ばない。返らない CLI が
/// 1 つあると診断ごと固まる（#2255）ので、あちらは [`auth_state_within`] を通す
pub fn auth_state_for(agent: AgentKind, binary: &str) -> AuthState {
    // #1261: 問い合わせの起動は正本の門番を通す。テストプロセスでは 1 つも起こさない
    // （実 CLI は起動しただけで `~/.gemini` / `~/.codex/tmp` / `~/.claude.json` を作る）
    let output = crate::agent_probe::run(
        std::process::Command::new(binary)
            .args(auth_probe_argv(agent))
            .stdin(std::process::Stdio::null()),
    );
    auth_state_from(agent, output)
}

/// [`auth_state_for`] の**待ちに上限を持つ**版（環境診断の経路。#2255）。
///
/// 上限を超えたら子を kill して「確かめられなかった」を返す（戻り値の 2 つ目。未認証とは区別する）。
/// 門番（#1261）と打ち切りの知らせ（#1503）は `agent_probe::output_within` が持つ
pub fn auth_state_within(
    agent: AgentKind,
    binary: &str,
    budget: std::time::Duration,
) -> (AuthState, Option<tako_core::probe::TimeoutNotice>) {
    let outcome = crate::agent_probe::output_within(binary, auth_probe_argv(agent), budget);
    let timeout = outcome.timeout_notice();
    (auth_state_from(agent, outcome.into_output()), timeout)
}

/// 問い合わせの出力の解釈（上限の有無で 2 つに割らない）。取れなかったら未認証
fn auth_state_from(agent: AgentKind, output: Option<std::process::Output>) -> AuthState {
    let Some(output) = output else {
        return AuthState::default();
    };
    if !output.status.success() {
        return AuthState::default();
    }
    match agent {
        AgentKind::Claude => serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .map(|v| parse_claude_auth(&v))
            .unwrap_or_default(),
        // codex のプランは CLI が返さないので `auth.json` の JWT から読む
        AgentKind::Codex => AuthState {
            authenticated: true,
            plan: codex_plan_from_auth_file().as_deref().map(normalize_plan),
        },
        // agy 1.1.1 は models で認証判定できるが、プラン / quota は返さない
        AgentKind::Agy => AuthState {
            authenticated: true,
            plan: None,
        },
    }
}

/// `claude auth status --json` の解釈（**純粋関数**なので実出力を fixture で固定できる）。
///
/// 呼び手は exit code を先に見ているので、ここは本文だけを見る
pub fn parse_claude_auth(value: &Value) -> AuthState {
    if !value
        .get("loggedIn")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return AuthState::default();
    }
    let plan = value
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .map(normalize_plan)
        .or_else(|| {
            value
                .get("authMethod")
                .and_then(|v| v.as_str())
                .filter(|method| method.to_ascii_lowercase().contains("api"))
                .map(|_| "api".to_string())
        });
    AuthState {
        authenticated: true,
        plan,
    }
}

/// プラン名の表記ゆれを 1 つへ寄せる（`Max 5x` / `max_5x` → `max-5x`）
pub fn normalize_plan(plan: &str) -> String {
    plan.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

/// Codex の OAuth JWT payload に含まれる ChatGPT plan claim をローカルで読む。
/// token 自体・account ID・メールアドレスは戻り値にもログにも出さない。
pub fn codex_plan_from_auth_file() -> Option<String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| tako_core::paths::home_dir().map(|h| h.join(".codex")))?;
    codex_plan_from_auth_file_at(&home.join("auth.json"))
}

fn codex_plan_from_auth_file_at(path: &Path) -> Option<String> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    if value
        .get("OPENAI_API_KEY")
        .and_then(|v| v.as_str())
        .is_some_and(|key| !key.is_empty())
    {
        return Some("api".to_string());
    }
    for token_name in ["id_token", "access_token"] {
        let token = value
            .get("tokens")
            .and_then(|v| v.as_object())?
            .get(token_name)
            .and_then(|v| v.as_str());
        let Some(payload) = token.and_then(decode_jwt_payload) else {
            continue;
        };
        if let Some(plan) = payload
            .get("https://api.openai.com/auth")
            .and_then(|v| v.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
        {
            return Some(plan.to_string());
        }
    }
    None
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = decode_base64url(payload)?;
    serde_json::from_slice(&decoded).ok()
}

/// 依存追加を避けるための最小 base64url decoder（JWT payload 読み取り専用）。
fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

/// ログインシェルの種別と profile
fn shell_target() -> (Option<ShellKind>, Option<PathBuf>) {
    let home = match home_dir() {
        Ok(home) => home,
        Err(_) => return (None, None),
    };
    let shell = std::env::var("SHELL").unwrap_or_default();
    let kind = shell_kind_for(&shell, cfg!(windows));
    let profile = kind.map(|k| home.join(k.login_profile_rel()));
    (kind, profile)
}

/// `$SHELL` の値からログインシェルの種別を決める（**純粋関数**。#2550）。
///
/// `$SHELL` が**空・未設定**（GUI から起動した .app 等）のときだけ OS の既定を仮定する
/// （macOS は 10.15 以降 zsh が既定。空を未設定と読むのは `exe::user_shell` と同じで、そこが同じ既定の
/// ログインシェル [`tako_core::platform::exe::fallback_login_shell`] に尋ねる = 書いた `.zprofile` を読む。#2583）。
/// **設定されていて種別の分からないシェル**（nu・tcsh 等）は `None` = 書かずに PATH へ足すディレクトリを返して断る
/// （構文はそのシェルの書き方に任せる = #2557）。
/// #2550 より前はこれも zsh と見なし、そのシェルが読まない `.zprofile` へ書いて成功を返していた
fn shell_kind_for(shell: &str, windows: bool) -> Option<ShellKind> {
    if shell.is_empty() {
        return (!windows).then_some(ShellKind::Zsh);
    }
    ShellKind::from_shell_path(shell)
}

/// PATH を tako が書けず、利用者が自分のシェルの設定ファイルで足すしかないか（**純粋関数**。#2564）。
///
/// 種別の分からないシェル（unix）だけが当たる。Windows はユーザー環境変数へ書くので当たらない
/// （`shell` は Windows では常に `None`）。当たるときは `tako setup bootstrap path` も同じ断り
/// （[`SetupRefusal::ShellProfileUnknown`]）で止まるので、`tako setup` の段・`--check` はそこへ誘導しない
pub fn path_by_hand_for(shell: Option<ShellKind>, user_path_supported: bool) -> bool {
    shell.is_none() && !user_path_supported
}

/// [`path_by_hand_for`] をこの導入状況で引く
pub fn path_by_hand(state: &BootstrapState) -> bool {
    path_by_hand_for(state.shell, user_path::is_supported())
}

/// 種別の分からないシェルなら、`tako setup bootstrap path` が返すのと同じ断り（足すディレクトリの一覧つき）。
/// `tako setup --check` の残り作業がこれを理由の行に載せる（#2564）
pub fn path_by_hand_refusal() -> Option<SetupRefusal> {
    if !path_by_hand_for(shell_target().0, user_path::is_supported()) {
        return None;
    }
    unknown_shell_refusal_here()
}

/// [`path_by_hand`] がこの導入状況で当たるなら、[`path_by_hand_refusal`] と同じ断り。
/// 初回起動の導線の「次にやること」が PATH の段で引く（#2576）
pub fn path_by_hand_refusal_for(state: &BootstrapState) -> Option<SetupRefusal> {
    if !path_by_hand(state) {
        return None;
    }
    unknown_shell_refusal_here()
}

/// いまのホームで `tako setup bootstrap path` が足すディレクトリを並べた、種別の分からないシェルの断り
fn unknown_shell_refusal_here() -> Option<SetupRefusal> {
    let home = home_dir().ok()?;
    let dirs = desired_path_dirs(&home, tako_cli_placement(&home).as_ref());
    Some(unknown_shell_refusal(&home, &dirs))
}

/// 種別の分からないシェルの断り。足すディレクトリは書く行と同じ `$HOME/…` の式で重複なし（#2557。
/// `~` 短縮の表示は sh の `export` に包まれて二重引用符の中に入り、sh 系では展開されなかった）
fn unknown_shell_refusal(home: &Path, dirs: &[PathBuf]) -> SetupRefusal {
    SetupRefusal::ShellProfileUnknown {
        shell: std::env::var("SHELL").unwrap_or_default(),
        dirs: shell_profile::dirs_for_unknown_shell(home, dirs),
    }
}

/// いまの導入状況を調べる（読み取りだけ。副作用なし）。認証の問い合わせは上限なしで待つ
/// （setup の経路。環境診断は [`status_for_within`]）
pub fn status_for(agent: AgentKind) -> Result<BootstrapState, String> {
    status_for_typed(agent).map_err(SetupRefusal::into_ja)
}

/// [`status_for`] の型の口（断りを表示言語で返す dispatch の `tako_setup_bootstrap` が使う。#2365）
pub fn status_for_typed(agent: AgentKind) -> Result<BootstrapState, SetupRefusal> {
    status_with(agent, |binary| (auth_state_for(agent, binary), None))
}

/// [`status_for`] の認証の問い合わせを `budget` で打ち切る版（環境診断の経路。#2255）
pub fn status_for_within(
    agent: AgentKind,
    budget: std::time::Duration,
) -> Result<BootstrapState, String> {
    status_with(agent, |binary| auth_state_within(agent, binary, budget))
        .map_err(SetupRefusal::into_ja)
}

/// 導入状況の組み立て。認証の問い合わせ方（上限の有無）は呼び手が `auth` で渡す
/// （ここに両方の口を並べると、診断の経路から上限なしの口へ字面で届く = #2255 の番犬が名指す）
fn status_with(
    agent: AgentKind,
    auth: impl FnOnce(&str) -> (AuthState, Option<tako_core::probe::TimeoutNotice>),
) -> Result<BootstrapState, SetupRefusal> {
    let home = home_dir_typed()?;
    let r = recipe_for(agent);
    let plan = plan_from(&r, &home);
    // [`resolve_binary_for`] と同じ順（PATH → インストーラの置き場所）。PATH の探索の答えは
    // PATH の判定でもう一度使う（同じ問い合わせのログインシェルを 2 度起こさない。#1533）
    let found = tako_core::platform::exe::find_bounded(agent.as_str());
    let binary = found.path.clone().or_else(|| installed_launcher_for(agent));
    // 認証とプランは 1 回の問い合わせから読む（#1505）
    let (auth, auth_timeout) = binary.as_deref().map(auth).unwrap_or_default();
    let authenticated = auth.authenticated;
    let launcher_dir = r.launcher_dir_in(&home);
    let (reach, path_timeout) = launcher_dir_on_path(&launcher_dir, &found);
    let on_path = reach.new_terminal;
    let (shell, profile) = shell_target();
    let profile_has_block = profile
        .as_deref()
        .is_some_and(shell_profile::profile_has_block);

    let step = if binary.is_none() {
        Step::Install
    } else if !on_path {
        Step::Path
    } else if !authenticated {
        Step::Auth
    } else {
        Step::Ready
    };

    Ok(BootstrapState {
        agent,
        binary,
        authenticated,
        account_plan: auth.plan,
        launcher_dir,
        launcher_dir_on_path: on_path,
        launcher_dir_on_process_path: reach.this_process,
        profile,
        profile_has_block,
        shell,
        step,
        plan,
        auth_timeout,
        locate_timeout: found.timeout,
        path_timeout,
    })
}

/// どの系統を仕上げるか。**途中まで入っているものを優先する**
/// （入れ直しではなく再開になるので、一番短い道になる）。
///
/// `tako setup` の bootstrap 段と `tako setup --check` の「残り 1 件」が
/// **同じ系統**を名指しするよう、判断はここ 1 か所に置く（#1505）
pub fn choose_target(states: &[(AgentKind, Result<BootstrapState, String>)]) -> AgentKind {
    let started = states.iter().find_map(|(agent, state)| {
        state
            .as_ref()
            .ok()
            .filter(|s| matches!(s.step, Step::Path | Step::Auth))
            .map(|_| *agent)
    });
    if let Some(agent) = started {
        return agent;
    }
    // どれも未導入。代行できる系統のうち先頭（= 推奨）を採る。
    // `bootstrap_agents()` の並びが「基準系 → master 可 → worker 専用」なので、
    // ここで別の優先順位を作らない
    states
        .iter()
        .find_map(|(agent, state)| {
            state
                .as_ref()
                .ok()
                .filter(|s| s.plan.can_run)
                .map(|_| *agent)
        })
        .or_else(|| states.first().map(|(agent, _)| *agent))
        .unwrap_or(AgentKind::Claude)
}

/// 3 系統ぶんの導入状況（**面倒を見る系統だけ**。`TAKO_989_LEGACY=1` なら claude だけ）。
///
/// 読めなかった系統は落とさず理由つきで並べる（無言で消えると
/// 「なぜ出てこないのか」が追えない）
pub fn status_all() -> Vec<(AgentKind, Result<BootstrapState, String>)> {
    bootstrap_agents()
        .into_iter()
        .map(|agent| (agent, status_for(agent)))
        .collect()
}

/// [`status_all`] の環境診断版（#2255）: 認証の問い合わせを `budget` で打ち切り、**系統を並べて**聞く。
///
/// 1 系統ずつ待つと、返らない CLI が 3 つあれば上限の 3 倍待つ。並べれば全部が返らなくても
/// 認証の確認の全体は `budget`（+ 起動と後始末）で終わる。並びは [`bootstrap_agents`] のまま返す
pub fn status_all_within(
    budget: std::time::Duration,
) -> Vec<(AgentKind, Result<BootstrapState, String>)> {
    let agents = bootstrap_agents();
    std::thread::scope(|scope| {
        let probes: Vec<_> = agents
            .into_iter()
            .map(|agent| (agent, scope.spawn(move || status_for_within(agent, budget))))
            .collect();
        probes
            .into_iter()
            .map(|(agent, probe)| {
                let state = probe.join().unwrap_or_else(|_| {
                    Err("導入状況の確認の途中で落ちました（#2255）".to_string())
                });
                (agent, state)
            })
            .collect()
    })
}

/// 3 系統ぶんの状況を JSON へ（CLI / MCP が同じ内容を見る）
pub fn status_all_json() -> Value {
    json!({
        "agents": status_all()
            .into_iter()
            .map(|(agent, state)| match state {
                Ok(state) => state.to_json(),
                Err(e) => json!({ "agent": agent.as_str(), "error": e }),
            })
            .collect::<Vec<_>>(),
    })
}

/// ランチャーの置き場所が「**新しく開いたターミナルが見る PATH**」に入っているか。
///
/// ## Windows で `exe::find` を使わない理由（#1057）
///
/// [`tako_core::platform::exe::find`] は PATH の外（`~\.local\bin` 等）まで
/// 走査する（「入れたのに再ログインしていない」を拾う保険。#525）。
/// これを on_path の判定に使うと **「PATH に無いが tako からは見つかる」を
/// 「PATH に在る」と誤って答える**ので、PATH を通す段が丸ごと飛ぶ。
/// Windows は PATH をレジストリで持つので、そちらを直接見る（境界 B23）。
///
/// unix は `exe::find` がログインシェルの `command -v` なので PATH の実態を
/// そのまま反映する（`.app` の痩せた PATH 対策として必要）＝従来のまま。
/// `found` は呼び手が済ませた PATH の探索（[`tako_core::platform::exe::find_bounded`]）の答えで、
/// 同じ問い合わせのログインシェルをもう一度起こさないために受け取る（#1533）。
///
/// 戻り値の 2 つ目は、ログインシェルの PATH の読み取りを上限で打ち切った知らせ（#1532）。
/// PATH の探索がいま打ち切られたなら PATH は読みに行かない（同じログインシェルへ尋ねても上限まで
/// 待つだけ）。そのときの「確かめられなかった」は [`BootstrapState::step_unconfirmed`] が探索の
/// 知らせから読む
///
/// ## 「新しいターミナル」と「いまのプロセス」を分ける（#2428）
///
/// 戻り値の 1 つ目は両方を持つ。Windows ではレジストリに入っただけでは**いまのタブには届かない**
/// （PATH はプロセスの起動時に 1 回だけ渡る）ので、「PATH に在る」だけで「次の 1 手」を出すと、
/// そのタブで打った人には「見つからない」になる。新しいペインの PATH は
/// `user_path::reach` がペインの実物（`user_path::pane_path`）と同じ組み方で見る
fn launcher_dir_on_path(
    dir: &Path,
    found: &tako_core::platform::exe::BoundedFind,
) -> (PathReach, Option<tako_core::probe::TimeoutNotice>) {
    let path_var = std::env::var("PATH").unwrap_or_default();
    if user_path::is_supported() && !legacy_mode() {
        if user_path::rereads_for_new_panes() {
            let reach = user_path::reach(dir);
            return (
                PathReach {
                    new_terminal: reach.new_pane,
                    this_process: reach.this_process,
                },
                None,
            );
        }
        // `TAKO_2428_LEGACY=1`: #2428 前（ユーザー PATH を PowerShell で読み、いまのタブと区別しない）
        let this_process = user_path::contains_entry(&path_var, dir);
        let on_path = this_process
            || user_path::read()
                .map(|value| user_path::contains_entry(&value.raw, dir))
                .unwrap_or(false);
        return (
            PathReach {
                new_terminal: on_path,
                this_process,
            },
            None,
        );
    }
    let this_process = shell_profile::path_contains(&path_var, dir);
    let reach = |new_terminal| PathReach {
        new_terminal,
        this_process,
    };
    // どれか 1 つで「在る」（判定の順は答えを変えない）。ログインシェルを起こす段を最後に置く
    if this_process || found.path.is_some() {
        return (reach(true), None);
    }
    if found.timeout.is_some() {
        return (reach(false), None);
    }
    let (seen, timeout) = login_shell_sees(dir);
    (reach(seen), timeout)
}

/// ランチャーの置き場所がどこから引けるか（#2428）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathReach {
    /// 新しく開いたターミナル（tako の新しいタブを含む）が見る PATH に入っている
    new_terminal: bool,
    /// いまのプロセスの PATH に入っている
    this_process: bool,
}

/// ログインシェルの PATH に `dir` が入っているか（`.app` の痩せた PATH 対策）。
/// 戻り値の 2 つ目は読み取りを上限で打ち切った知らせ（#1532。立っていたら false は「確かめられなかった」）
///
/// # これは意図的に unix 専用（B21 へ機械的に寄せないこと）
///
/// ログインシェルを起こすので、#877 が片付けている「`$SHELL -l -c` の直書きの一族」に見える。
/// **だが意図が違う**ので [`tako_core::platform::child_cmd::user_env_cli`]（境界 B21）へは寄せられない。
/// 読み取りそのものは境界 B16 の [`tako_core::platform::exe::login_shell_path`]（上限つきの 1 実装。
/// #1532）が持つ。
///
/// | | 一族（agents 走査など） | ここ |
/// |---|---|---|
/// | 意図 | ユーザーの環境で **CLI を 1 回走らせる** | **ログインシェルの PATH を読む** |
/// | Windows の対応物 | PATH で解決した実体を直接起動 | **無い** |
///
/// Windows の PATH は**レジストリ由来でシェル profile 由来ではない**ため、
/// 「ログインシェルの PATH」という概念そのものが存在しない（`login_shell_path` は Windows で常に空）。
/// 判定は [`tako_core::platform::exe::find`]（境界 B16。PATH + ユーザーが入れがちな場所を走査）と
/// [`resolve_binary_for`]（インストーラの置き場所を直接見る）に任せる。
/// **壊れている経路ではなく、Windows には要らない経路**（#868 / #877）
fn login_shell_sees(dir: &Path) -> (bool, Option<tako_core::probe::TimeoutNotice>) {
    let read = tako_core::platform::exe::login_shell_path();
    let sees = read
        .value
        .as_deref()
        .is_some_and(|path| shell_profile::path_contains(path, dir));
    (sees, read.timeout)
}

// --- インストール実行 ---

/// インストーラを取得して実行する。
///
/// 公式の 1 行（`curl … | bash`）と結果は同じだが、**取得と実行を分ける**。
/// こうすると「取れなかった」「取れたが中身がスクリプトでない（プロキシの HTML
/// エラーページ）」「実行が失敗した」を切り分けて具体的に報告できる。
/// 公式のトラブルシュートにも `syntax error near unexpected token '<'` として
/// 載っている実在の失敗モードで、パイプのままだと利用者には理由が見えない
pub fn install_for(agent: AgentKind, opts: InstallOptions) -> Result<Value, String> {
    install_for_typed(agent, opts).map_err(SetupRefusal::into_ja)
}

/// [`install_for`] の型の口（#2365。断りを表示言語で返す dispatch の経路が通る）
pub fn install_for_typed(agent: AgentKind, opts: InstallOptions) -> Result<Value, SetupRefusal> {
    let plan = install_plan_for_typed(agent)?;
    if opts.dry_run {
        return Ok(json!({
            "performed": false,
            "reason": "dry_run",
            "install_plan": plan.to_json(),
        }));
    }
    // セルフテストは開発者の実ホームで走る（`TAKO_ISOLATED=1` が隔離するのは tako の
    // データディレクトリだけで HOME ではない）。dry_run の扱いが将来壊れたときに
    // **実インストールが実ホームへ走る**ことがないよう、ここで構造的に止める。
    // 過去にテストが実環境を壊した事故があるので、経路自体を塞いでおく
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return Err(SetupRefusal::InstallDuringSelfTest);
    }
    if !plan.can_run || (legacy_mode() && cfg!(windows)) {
        return Err(SetupRefusal::InstallNotDelegable {
            os: std::env::consts::OS.to_string(),
            command: plan.official_command.clone(),
        });
    }
    let script = fetch_installer(agent, &plan)?;
    let result = run_installer(agent, &script, opts.interactive);
    // 一時ファイルは成否にかかわらず片付ける
    let _ = std::fs::remove_file(&script);
    let log = result?;

    let binary = resolve_binary_for(agent).ok_or_else(|| SetupRefusal::InstalledButMissing {
        launcher: display_path(&plan.launcher),
        command: plan.official_command.clone(),
    })?;
    Ok(json!({
        "performed": true,
        "binary": binary,
        "install_plan": plan.to_json(),
        // 対話実行では利用者が画面で見ているので空。捕捉実行では AI が診断に使える
        "output": log,
    }))
}

/// インストーラの取得手段。
///
/// curl / wget が無い環境でも詰まらないよう、最後の手段として PowerShell の
/// `Invoke-WebRequest` を置く（Windows 10 1803 以降は `curl.exe` が同梱されるので
/// 通常はそちらが選ばれる。実機実測では `C:\Windows\System32\curl.exe`）
enum Downloader {
    Curl(String),
    Wget(String),
    PowerShell(String),
}

impl Downloader {
    /// この環境で使えるものを 1 つ選ぶ
    fn detect() -> Option<Self> {
        if let Some(bin) = tako_core::platform::exe::find("curl") {
            return Some(Self::Curl(bin));
        }
        if let Some(bin) = tako_core::platform::exe::find("wget") {
            return Some(Self::Wget(bin));
        }
        if !cfg!(windows) {
            return None;
        }
        tako_core::platform::exe::find("powershell")
            .or_else(|| tako_core::platform::exe::find("pwsh"))
            .or_else(|| Some("powershell.exe".to_string()))
            .map(Self::PowerShell)
    }

    fn program(&self) -> &str {
        match self {
            Self::Curl(bin) | Self::Wget(bin) | Self::PowerShell(bin) => bin,
        }
    }
}

/// PowerShell 経路の取得スクリプト。**URL と保存先は env で渡す**ので
/// 引用符の入れ子もコードページも関与しない（`platform::user_path` と同じ作法）
const POWERSHELL_FETCH_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'\n\
$ProgressPreference = 'SilentlyContinue'\n\
Invoke-WebRequest -UseBasicParsing -Uri $env:TAKO_FETCH_URL -OutFile $env:TAKO_FETCH_DEST\n";

/// インストーラを一時ファイルへ取得する。取得できた中身が本物かまで見る
fn fetch_installer(agent: AgentKind, plan: &InstallPlan) -> Result<PathBuf, SetupRefusal> {
    let runner = recipe_for(agent).runner;
    let downloader = Downloader::detect().ok_or(SetupRefusal::NoDownloader)?;
    // 拡張子はインタプリタが要求するもの（PowerShell は `.ps1` 以外を実行しない）
    let dest = std::env::temp_dir().join(format!(
        "tako-{}-install-{}.{}",
        agent.as_str(),
        std::process::id(),
        runner.script_ext
    ));
    let bin = downloader.program().to_string();
    let mut command = std::process::Command::new(&bin);
    // #586: GUI プロセス（dispatch）から到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut command);
    match &downloader {
        Downloader::Curl(_) => {
            command.args(["-fsSL", &plan.source_url, "-o"]).arg(&dest);
        }
        Downloader::Wget(_) => {
            command.args(["-q", &plan.source_url, "-O"]).arg(&dest);
        }
        Downloader::PowerShell(_) => {
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &tako_core::platform::shell::encode_powershell_command(POWERSHELL_FETCH_SCRIPT),
                ])
                .env("TAKO_FETCH_URL", &plan.source_url)
                .env("TAKO_FETCH_DEST", &dest);
        }
    }
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| SetupRefusal::InstallerToolSpawn {
            program: bin.clone(),
            error: e.to_string(),
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&dest);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(SetupRefusal::InstallerFetchFailed {
            url: plan.source_url.clone(),
            detail: if detail.is_empty() {
                FetchDetail::Exit {
                    program: bin,
                    code: output.status.code().unwrap_or(-1),
                }
            } else {
                FetchDetail::Stderr(detail.to_string())
            },
        });
    }

    // 先頭だけ見る。`install.ps1` は `param(...)` → `Set-StrictMode` → …の順なので
    // 512 バイトあればどちらの署名も判定できる（実物で確認）
    let head = std::fs::read(&dest)
        .map_err(|e| SetupRefusal::InstallerUnreadable(e.to_string()))?
        .into_iter()
        .take(512)
        .collect::<Vec<u8>>();
    if !agent_install::looks_like_installer(runner.signature, &head) {
        let _ = std::fs::remove_file(&dest);
        return Err(SetupRefusal::NotAnInstaller {
            url: plan.source_url.clone(),
        });
    }
    Ok(dest)
}

/// インストール実行の指定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InstallOptions {
    /// 実行せず計画だけ返す
    pub dry_run: bool,
    /// 端末があるか。true = 出力を利用者へ流す（進捗が見える・インストーラの TUI が動く）、
    /// false = 捕捉して応答へ載せる（GUI 内 dispatch / MCP から呼ばれる経路）
    pub interactive: bool,
}

/// 取得したインストーラを実行する。
///
/// 端末があるときは出力をそのまま流す（インストーラは進捗と TUI を出す）。
/// GUI 内 dispatch から呼ばれたときは端末が無いので捕捉し、失敗時の診断へ回す
fn run_installer(
    agent: AgentKind,
    script: &Path,
    interactive: bool,
) -> Result<String, SetupRefusal> {
    let r = recipe_for(agent);
    let runner = r.runner;
    // インタプリタと引数はプラットフォーム境界（B17）が持つデータから組む。
    // ここに `bash` / `powershell` を書かない = 経路の取り違えが起きない
    let shell = runner
        .candidates
        .iter()
        .find_map(|name| tako_core::platform::exe::find(name))
        .unwrap_or_else(|| runner.fallback.to_string());
    let mut command = std::process::Command::new(&shell);
    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut command);
    command.args(runner.args_for(script));
    // インストーラの対話プロンプトを黙らせる env（codex の `Start Codex now?` 対策）。
    // **手順の側が持つデータ**なので、ここに系統名を書かない
    for (key, value) in r.installer_env {
        command.env(key, value);
    }
    let (status, log) = if interactive {
        let status = command
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| SetupRefusal::InstallerToolSpawn {
                program: shell.clone(),
                error: e.to_string(),
            })?;
        (status, String::new())
    } else {
        let output = command
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| SetupRefusal::InstallerToolSpawn {
                program: shell.clone(),
                error: e.to_string(),
            })?;
        let mut log = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            if !log.is_empty() {
                log.push('\n');
            }
            log.push_str(stderr.trim());
        }
        (output.status, log)
    };
    if status.success() {
        return Ok(log);
    }
    let code = status.code().unwrap_or(-1);
    // 出力の添え方（空なら添えない）は `SetupRefusal` の文の側が持つ
    Err(match code {
        // インストーラ自身が説明を出す終了コード
        137 => SetupRefusal::InstallerOutOfMemory { log },
        c if c >= 128 => SetupRefusal::InstallerKilled { code: c, log },
        // 案内先は**その系統の公式ページ**（claude のトラブルシュートを
        // codex / agy の失敗に出さない。#989 のエッジ検証で踏んだ）
        c => SetupRefusal::InstallerFailed {
            product: agent.product().to_string(),
            code: c,
            troubleshoot_url: r.troubleshoot_url.to_string(),
            log,
        },
    })
}

// --- PATH 通し ---

/// この環境で tako が PATH へ通したいディレクトリの**全部**（正本 1 箇所。#1502）。
///
/// ブロックは 1 組しか無いので、書くときは**毎回この全部**を渡す
/// （部分集合を渡すと前回入れた分が消える）。並びは
/// 「tako 自身の CLI → エージェント CLI のランチャー」で、重複は
/// [`shell_profile::ensure_dirs_on_path_in`] が落とす。
/// **macOS は 3 系統とも `~/.local/bin` で tako の symlink 先も同じ**なので、
/// 実際に並ぶのは 1 個 = ブロックの中身は #1502 前と変わらない
fn desired_path_dirs(home: &Path, tako: Option<&tako_cli_path::Placement>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = tako.map(|p| p.dir.clone()).into_iter().collect();
    for agent in bootstrap_agents() {
        dirs.push(recipe_for(agent).launcher_dir_in(home));
    }
    dirs
}

/// tako CLI の設置計画（解決できなければ `None` = 従来どおりエージェントぶんだけ通す）
fn tako_cli_placement(home: &Path) -> Option<tako_cli_path::Placement> {
    if legacy_no_tako_cli_path() {
        return None;
    }
    tako_cli_path::plan_here(home)
}

/// `TAKO_1502_LEGACY=1` で #1502 前（tako CLI を PATH へ設置しない）へ戻す。
/// **同一バイナリで A/B が取れる**（検証は `scripts/test-tako-cli-path-1502.sh`）
pub fn legacy_no_tako_cli_path() -> bool {
    std::env::var_os("TAKO_1502_LEGACY").is_some()
}

/// A/B のときに出す 1 行（**何が無効なのかを名乗る**。黙って落とすと
/// 「設置されない」が仕様なのか壊れたのか分からない）
const LEGACY_LINE: &str =
    "  [legacy] tako CLI の PATH 設置は TAKO_1502_LEGACY=1 で無効（#1502 前の挙動）";

/// tako CLI の symlink を張る（unix のみ。Windows は `link: None` なので何もしない）
fn ensure_tako_cli_link(
    plan: &tako_cli_path::Placement,
) -> Result<tako_cli_path::LinkChange, SetupRefusal> {
    match plan.link.as_deref() {
        Some(link) => Ok(tako_cli_path::ensure_link_typed(link, &plan.source)?),
        None => Ok(tako_cli_path::LinkChange::Unchanged),
    }
}

/// tako CLI の設置状況を JSON 節にする（`ensure_path_for` / `check-health` / `--check` が同じ材料を読む）
pub fn tako_cli_path_json() -> Value {
    if legacy_no_tako_cli_path() {
        return json!({ "resolved": false, "reason": "TAKO_1502_LEGACY=1（#1502 前の挙動）" });
    }
    let Ok(home) = home_dir() else {
        return json!({ "resolved": false, "reason": "ホームディレクトリを特定できません" });
    };
    let Some(plan) = tako_cli_placement(&home) else {
        return json!({
            "resolved": false,
            "reason": "tako CLI の実体を解決できません（実行中バイナリの隣に tako がありません）",
        });
    };
    let state = plan
        .link
        .as_deref()
        .map(|link| tako_cli_path::link_state(link, &plan.source));
    let (on_path, unconfirmed) = dir_on_path(&plan.dir);
    json!({
        "resolved": true,
        "dir": plan.dir.display().to_string(),
        "dir_display": display_path(&plan.dir),
        "source": plan.source.display().to_string(),
        "source_display": display_path(&plan.source),
        "link": plan.link.as_ref().map(|l| l.display().to_string()),
        "link_display": plan.link.as_deref().map(display_path),
        // symlink を張らない形（Windows / 実体が置き場所そのものに居る）は null
        "link_state": state.as_ref().map(tako_cli_path::LinkState::as_str),
        "dir_on_path": on_path,
        // #1532: ログインシェルの PATH の読み取りを上限で打ち切った（null = 確かめた答え）。
        // 立っているときの `dir_on_path` はこのプロセスの PATH で代わりに答えたもの
        "dir_on_path_unconfirmed": unconfirmed.as_ref().map(timeout_json),
        // 外部ターミナルから `tako` が打てるか（= PATH に在り、その場所の tako が使える）
        "usable": on_path && state.as_ref().map(|s| s.usable()).unwrap_or(true),
    })
}

/// `dir` が「新しく開いたターミナルが見る PATH」に在るか（OS 差は既存の判定を使う）。
/// 戻り値の 2 つ目はログインシェルの PATH の読み取りを上限で打ち切った知らせ（#1532）
fn dir_on_path(dir: &Path) -> (bool, Option<tako_core::probe::TimeoutNotice>) {
    if user_path::is_supported() {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let on_path = user_path::contains_entry(&path_var, dir)
            || user_path::read()
                .map(|value| user_path::contains_entry(&value.raw, dir))
                .unwrap_or(false);
        return (on_path, None);
    }
    // **ログインシェルが見るか**を優先して答える（外部ターミナルの話をしているので、
    // いま走っているプロセスの PATH は答えにならない）。読めないときだけ自分の PATH へ戻す
    let read = tako_core::platform::exe::login_shell_path();
    let on_path = match read.value {
        Some(path) => shell_profile::path_contains(&path, dir),
        None => shell_profile::path_contains(&std::env::var("PATH").unwrap_or_default(), dir),
    };
    (on_path, read.timeout)
}

/// tako CLI を外部ターミナルから打てるようにする（冪等。FR-2.14.5 / #1502）。
///
/// `tako setup` の段としても、`tako setup bootstrap path` の一部としても、
/// ここ 1 本を通る
pub fn ensure_tako_cli_path() -> Result<Value, String> {
    ensure_tako_cli_path_inner().map_err(CliPathError::into_ja)
}

/// [`ensure_tako_cli_path`] の失敗。種別の分からないシェルの断りを段の文で見分けるために型で持つ（#2564）
enum CliPathError {
    /// 従来の日本語の文（#1502 の legacy・ホーム・実体の解決）
    Text(String),
    Refusal(SetupRefusal),
}

impl CliPathError {
    fn into_ja(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Refusal(refusal) => refusal.into_ja(),
        }
    }
}

fn ensure_tako_cli_path_inner() -> Result<Value, CliPathError> {
    if legacy_no_tako_cli_path() {
        return Err(CliPathError::Text(
            "TAKO_1502_LEGACY=1（#1502 前の挙動）".to_string(),
        ));
    }
    let home = home_dir().map_err(CliPathError::Text)?;
    let Some(plan) = tako_cli_placement(&home) else {
        return Err(CliPathError::Text(
            "tako CLI の実体を解決できません（実行中バイナリの隣に tako がありません）".to_string(),
        ));
    };
    let link_change = ensure_tako_cli_link(&plan).map_err(CliPathError::Refusal)?;
    let path = ensure_dirs_on_path(&home, &desired_path_dirs(&home, Some(&plan)))
        .map_err(CliPathError::Refusal)?;
    Ok(json!({
        "link_change": link_change.as_str(),
        "path": path,
        "tako_cli": tako_cli_path_json(),
    }))
}

/// `tako setup` の段として走らせて、人へ出す文面を返す（#1502）。
///
/// **文面の正本はここ 1 箇所**。失敗しても setup 全体は止めない
/// （tako 内側のシェルからは `tako` が打てる = 致命ではない）
pub fn run_tako_cli_path_stage() -> String {
    if legacy_no_tako_cli_path() {
        return LEGACY_LINE.to_string();
    }
    let before = tako_cli_path_json();
    match ensure_tako_cli_path_inner() {
        Ok(value) => {
            let after = &value["tako_cli"];
            let dir = after["dir_display"].as_str().unwrap_or("?");
            let wrote = value["path"]["wrote"].as_bool().unwrap_or(false)
                || value["link_change"]
                    .as_str()
                    .is_some_and(|c| matches!(c, "created" | "updated"));
            if !wrote && before["usable"].as_bool().unwrap_or(false) {
                return format!("  [OK] tako CLI: どのターミナルからも使えます（{dir}）");
            }
            let mut line =
                "  [設置] tako CLI: どのターミナルからも tako を打てるようにしました".to_string();
            if let Some(link) = after["link_display"].as_str() {
                line.push_str(&format!(
                    "\n         {link} → {}",
                    after["source_display"].as_str().unwrap_or("?")
                ));
            }
            // 種別の分からないシェル（#2564）は PATH を書いていない（利用者が足し終えていた）ので、
            // 戻す PATH のブロックが無い = undo-path へ誘導しない（そのシェルでは undo-path も断る）
            if value["path"]["shell"].is_null() {
                line.push_str(&format!("\n         PATH: {dir}"));
            } else {
                line.push_str(&format!(
                    "\n         PATH: {} （元に戻すには tako setup bootstrap undo-path）",
                    dir
                ));
            }
            if !value["path"]["verified"].as_bool().unwrap_or(false) {
                line.push_str(
                    "\n         いま開いているターミナルには反映されません（開き直すと有効）",
                );
            }
            line
        }
        // 種別の分からないシェル（#2564）: `tako setup bootstrap path` も同じ断りで止まるので誘導せず、
        // 足すディレクトリの一覧で閉じる
        Err(CliPathError::Refusal(refusal @ SetupRefusal::ShellProfileUnknown { .. })) => {
            PathByHandNote::CliStage(refusal).text_in(tako_core::i18n::lang())
        }
        // 止めない。理由と次の一手だけ出す
        Err(e) => format!(
            "  [警告] tako CLI の PATH 設置を見送りました: {}\n\
             \x20        tako の中では tako が打てます（#601）。\n\
             \x20        外部ターミナルでも使うなら tako setup bootstrap path を実行してください",
            e.into_ja()
        ),
    }
}

/// `tako setup --check` の 1 行（#1502）。**判定は [`tako_cli_path_json`] の 1 実装**
pub fn tako_cli_path_check_line() -> String {
    if legacy_no_tako_cli_path() {
        return LEGACY_LINE.to_string();
    }
    let state = tako_cli_path_json();
    if !state["resolved"].as_bool().unwrap_or(false) {
        return format!(
            "  [警告] tako CLI の PATH: 確認できません（{}）",
            state["reason"].as_str().unwrap_or("理由不明")
        );
    }
    let dir = state["dir_display"].as_str().unwrap_or("?");
    // ログインシェルの PATH を上限内に読めなかった（#1532）。このプロセスの PATH で代わりに答えた
    // 「使える / 入っていない」を言うと外部ターミナルの話にならないので、確かめられないと言う
    // （リンクが切れているのは PATH と関係なく確かな不足なので、下の理由へ進む）
    if let Some(secs) = state["dir_on_path_unconfirmed"]["waited_secs"].as_u64() {
        if state["link_state"].as_str() != Some("dangling") {
            return format!(
                "  [警告] tako CLI の PATH: 確認できません（ログインシェルが {secs} 秒で応答なし）\n\
                 \x20        .zprofile などが入力を待っていないか確かめてください"
            );
        }
    }
    if state["usable"].as_bool().unwrap_or(false) {
        return format!("  [OK] tako CLI の PATH: 外部ターミナルからも使えます（{dir}）");
    }
    // **状態ごとに違う理由を出す**（「使えません」だけだと、まだ設置していないのか
    // `.app` を動かして切れたのかが分からず、次の一手が選べない）
    let reason = match state["link_state"].as_str() {
        Some("dangling") => "リンクの指す先が消えています（tako.app を移動しましたか？）",
        Some("missing") if state["dir_on_path"].as_bool().unwrap_or(false) => {
            "まだ設置していません"
        }
        _ if !state["dir_on_path"].as_bool().unwrap_or(false) => "設置先が PATH に入っていません",
        _ => "まだ設置していません",
    };
    // 種別の分からないシェルで置き場所が PATH に無い（#2564）: `tako setup bootstrap path` は
    // PATH の段で同じ断りに止まるので誘導せず、足すディレクトリを示す
    // （判定は check-health の `cli_in_path` と同じ 1 実装 = #2576）
    if let Some(hint) = tako_cli_dir_by_hand(&state) {
        // この行は check-health と `--check` が同じ文を出す診断の正本（#1505）。2 つは表示言語の
        // 決め方が違うので、1 行目（日本語）と同じく言語を固定して組む（表示言語で組むと食い違う）
        return format!(
            "  [不足] tako CLI の PATH: 外部ターミナルからは使えません（{reason}）\n{}",
            PathByHandNote::CliCheckHint { dir: hint }.text_in(tako_core::i18n::Lang::Ja)
        );
    }
    format!(
        "  [不足] tako CLI の PATH: 外部ターミナルからは使えません（{reason}）\n\
         \x20        tako setup bootstrap path で設置できます"
    )
}

/// tako CLI の置き場所を、利用者が自分のシェルの設定ファイルで PATH へ足すしかないなら、その置き場所
/// （`$HOME/…` の式）。`state` は [`tako_cli_path_json`]。`tako setup --check` の行と check-health の
/// `cli_in_path` が同じ判定を通る（#2564 / #2576）
pub fn tako_cli_dir_by_hand(state: &Value) -> Option<String> {
    tako_cli_dir_by_hand_for(
        state,
        path_by_hand_for(shell_target().0, user_path::is_supported()),
        home_dir().ok().as_deref(),
    )
}

/// [`tako_cli_dir_by_hand`] の判断（**純粋関数**。`by_hand` = [`path_by_hand_for`] の答え）。
///
/// 置き場所が PATH に在る（リンクだけ無い / 切れている）なら `tako setup bootstrap path` は通るので None
/// （従来の案内のまま）。ホームを解決できなければ表示用のパスで示す
pub fn tako_cli_dir_by_hand_for(
    state: &Value,
    by_hand: bool,
    home: Option<&Path>,
) -> Option<String> {
    if !by_hand
        || !state["resolved"].as_bool().unwrap_or(false)
        || state["dir_on_path"].as_bool().unwrap_or(false)
    {
        return None;
    }
    home.and_then(|home| {
        shell_profile::dirs_for_unknown_shell(
            home,
            &[PathBuf::from(state["dir"].as_str().unwrap_or_default())],
        )
        .into_iter()
        .next()
    })
    .or_else(|| Some(state["dir_display"].as_str().unwrap_or("?").to_string()))
}

/// check-health の `cli_in_path`（tako CLI が外部ターミナルの PATH に見つからない）の (level, 文)（**純粋関数**）。
///
/// `injected` = tako が開くシェルへ自動で足している CLI の置き場所（#601。在れば tako の中では打てる = info）、
/// `by_hand_dir` = [`tako_cli_dir_by_hand`]。足すしかない（種別の分からないシェルで置き場所が PATH に無い）なら
/// `tako setup bootstrap path` は同じ断りで止まるので誘導せず、`--check` の行と同じ案内（足す置き場所）で
/// 閉じる（#2576）。check-health の診断は表示言語を解決しないので日本語で固定する（#1505 / #2564 と同じ）
pub fn tako_cli_in_path_issue(
    injected: Option<&Path>,
    by_hand_dir: Option<String>,
) -> (&'static str, String) {
    let by_hand = by_hand_dir.map(|dir| {
        PathByHandNote::CliCheckHint { dir }
            .text_in(tako_core::i18n::Lang::Ja)
            .trim_start()
            .to_string()
    });
    // 注入が効いていれば tako の中では打てる = 致命ではない。外部ターミナルでも使いたい人向けの
    // 案内に落とす（level を下げても対処法は示し続ける）。#1502 からは手順ではなく 1 コマンド（#322）
    match (injected, by_hand) {
        (Some(dir), None) => (
            "info",
            format!(
                "tako CLI は PATH に無いが、tako が開くシェルには {} を自動で追加するので \
                 tako の中では `tako` が使える（#601）。外部ターミナルでも使いたい場合は \
                 tako setup bootstrap path を実行すること（#1502）",
                dir.display()
            ),
        ),
        (Some(dir), Some(hint)) => (
            "info",
            format!(
                "tako CLI は PATH に無いが、tako が開くシェルには {} を自動で追加するので \
                 tako の中では `tako` が使える（#601）。外部ターミナルでも使いたい場合は、{hint}",
                dir.display()
            ),
        ),
        (None, None) => (
            "error",
            "tako CLI が PATH に見つからない。tako setup bootstrap path で設置するか、\
             scripts/build-app.sh --install でインストールすること"
                .to_string(),
        ),
        (None, Some(hint)) => ("error", format!("tako CLI が PATH に見つからない。{hint}")),
    }
}

/// check-health の `tako_cli_link`（設置した tako CLI のリンクの指す先が消えている）の文（**純粋関数**。#2583）。
///
/// `by_hand` = [`tako_cli_dir_by_hand`] が当たる（種別の分からないシェルで置き場所が PATH に無い）。当たるなら
/// `tako setup bootstrap path` はリンクを張り直した後の PATH の段で同じ断りに止まる（堂々巡り）ので誘導せず、
/// 起動時の張り直し（tako-app の `repair_tako_cli_link`。シェルに依らず、切れているときだけ張り直す）で閉じる。
/// PATH へ足す案内は同じ判定の `cli_in_path` / `--check` の行が受け持つ。check-health の診断は表示言語を
/// 解決しないので日本語で固定する（#1505 / #2576 と同じ）
pub fn tako_cli_link_issue(link_display: &str, by_hand: bool) -> String {
    if by_hand {
        return format!(
            "tako CLI のリンク（{link_display}）の指す先が消えている。tako.app を移動・入れ直した場合は、\
             その tako を起動し直すと張り直される（#1502）"
        );
    }
    format!(
        "tako CLI のリンク（{link_display}）の指す先が消えている。tako.app を移動・削除した場合は \
         tako setup bootstrap path で張り直すこと（#1502）"
    )
}

/// `dirs` をログインシェルの PATH（Windows はユーザー環境変数）へ通す
fn ensure_dirs_on_path(home: &Path, dirs: &[PathBuf]) -> Result<Value, SetupRefusal> {
    if user_path::is_supported() {
        return ensure_user_path_dirs(dirs);
    }
    let (shell, _) = shell_target();
    let Some(shell) = shell else {
        return unknown_shell_path(home, dirs);
    };
    // 「書かなくてよいか」は**新しく開いたターミナルが見る PATH**で判定する（#1502）。
    // プロセスの PATH は親から受け継いだだけで、ログインシェルが同じとは限らない
    // （tako 内のペインは #601 で CLI ディレクトリを注入済み / `.app` は逆に痩せている）。
    // ログインシェルを起こせない環境（`$SHELL` が壊れている等）はプロセスの PATH へ戻す。
    // 上限で打ち切った（#1532）ときは PATH を判定に使わない = 目印のブロックの有無だけで決める
    // （冪等。プロセスの PATH へ戻すと、#601 で注入済みの tako のペインでは「もう通っている」と誤答する）
    let read = tako_core::platform::exe::login_shell_path();
    let path_var = match (&read.value, &read.timeout) {
        (Some(path), _) => Some(path.clone()),
        (None, Some(_)) => None,
        (None, None) => Some(std::env::var("PATH").unwrap_or_default()),
    };
    let outcome =
        shell_profile::ensure_dirs_on_path_in_typed(home, shell, dirs, path_var.as_deref())?;
    // 実際にログインシェルから引けるようになったかを**確かめてから**返す。
    // 「書いたはず」で終わらせない（.zshrc へ書いて届かなかった類の失敗を検出する）。
    // 読み取りが打ち切られたなら尋ね直さない（同じログインシェルを上限まで待つだけ）
    let verified = outcome.change == PathChange::AlreadyOnPath
        || (read.timeout.is_none() && outcome.dirs.iter().all(|d| login_shell_sees(d).0));
    Ok(json!({
        "shell": shell.as_str(),
        "profile": outcome.profile.display().to_string(),
        "profile_display": display_path(&outcome.profile),
        "dirs": outcome.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "dirs_display": outcome.dirs.iter().map(|d| display_path(d)).collect::<Vec<_>>(),
        "change": outcome.change.as_str(),
        "wrote": outcome.change.wrote(),
        "verified": verified,
        // #1532: ログインシェルの PATH の読み取りを上限で打ち切った（`verified` は確かめていない）
        "login_shell_timeout": read.timeout.as_ref().map(timeout_json),
    }))
}

/// 種別の分からないシェルの PATH 通し（#2550 / #2557 / #2564）。設定ファイルは書かない。
///
/// 足すディレクトリが**もう PATH に見えている**なら断らずに `already_on_path` を返す（#2564）。
/// 断りは「このディレクトリをご自身の設定ファイルへ足してください」なので、利用者が足し終えた後も
/// 断り続けると、案内どおりにしたのに `tako setup` が同じ案内を出し続ける（形を変えた堂々巡り）。
/// 見え方の判定は既知のシェルと同じ「新しく開いたターミナルが見る PATH」（読めなければプロセスの
/// PATH。tcsh は `-l -c` を受け付けないのでこちらになる）で、上限で打ち切ったときは確かめられない = 断る
fn unknown_shell_path(home: &Path, dirs: &[PathBuf]) -> Result<Value, SetupRefusal> {
    let read = tako_core::platform::exe::login_shell_path();
    let path_var = match (&read.value, &read.timeout) {
        (Some(path), _) => Some(path.clone()),
        (None, Some(_)) => None,
        (None, None) => Some(std::env::var("PATH").unwrap_or_default()),
    };
    let visible = !dirs.is_empty()
        && path_var
            .as_deref()
            .is_some_and(|path| dirs.iter().all(|d| shell_profile::path_contains(path, d)));
    if !visible {
        return Err(unknown_shell_refusal(home, dirs));
    }
    let mut seen: Vec<&PathBuf> = Vec::new();
    for dir in dirs {
        if !seen.contains(&dir) {
            seen.push(dir);
        }
    }
    Ok(json!({
        // 書き先は無い（種別の分からないシェルの設定ファイルは書かない）
        "shell": Value::Null,
        "profile": Value::Null,
        "profile_display": Value::Null,
        "dirs": seen.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "dirs_display": seen.iter().map(|d| display_path(d)).collect::<Vec<_>>(),
        "change": PathChange::AlreadyOnPath.as_str(),
        "wrote": false,
        "verified": true,
        "login_shell_timeout": Value::Null,
    }))
}

/// ランチャーの置き場所を「新しく開いたターミナルが見る PATH」へ通す（冪等）。
///
/// #1502 から **tako 自身の CLI もここで一緒に通す**（マーカーブロックは 1 組のまま）
pub fn ensure_path_for(agent: AgentKind) -> Result<Value, String> {
    ensure_path_for_typed(agent).map_err(SetupRefusal::into_ja)
}

/// [`ensure_path_for`] の型の口（#2365）
pub fn ensure_path_for_typed(agent: AgentKind) -> Result<Value, SetupRefusal> {
    let home = home_dir_typed()?;
    let r = recipe_for(agent);
    let dir = r.launcher_dir_in(&home);
    let plan = tako_cli_placement(&home);
    // symlink は PATH より先に張る（PATH だけ通って実体が無い状態を作らない）
    let link_change = match plan.as_ref() {
        Some(plan) => ensure_tako_cli_link(plan)?,
        None => tako_cli_path::LinkChange::Unchanged,
    };
    if user_path::is_supported() {
        return ensure_user_path(agent, &dir, plan.as_ref(), link_change);
    }
    let path = ensure_dirs_on_path(&home, &desired_path_dirs(&home, plan.as_ref()))?;
    let verified = path["verified"].as_bool().unwrap_or(false);
    Ok(json!({
        "agent": agent.as_str(),
        "shell": path["shell"],
        "profile": path["profile"],
        "profile_display": path["profile_display"],
        "dir": dir.display().to_string(),
        "dir_display": display_path(&dir),
        // ブロックが通しているディレクトリの全部（#1502）
        "dirs": path["dirs"],
        "dirs_display": path["dirs_display"],
        "change": path["change"],
        "wrote": path["wrote"],
        "verified": verified,
        "tako_cli_link_change": link_change.as_str(),
        "tako_cli": tako_cli_path_json(),
        "note": if verified {
            format!(
                "新しく開くターミナルから {} コマンドと tako コマンドが使えます",
                agent.as_str()
            )
        } else {
            "設定は書きましたが、いまのシェルにはまだ反映されていません。\
             ターミナルを開き直すと有効になります"
                .to_string()
        },
    }))
}

/// Windows のユーザー PATH（`HKCU\Environment\Path`）へ通す（冪等。境界 B23）。
///
/// **末尾へ足す**ので、ユーザーが自分で並べた優先順位は動かない。
/// 書いたあと**読み直して確かめる**（「書いたはず」で終わらせない）
fn ensure_user_path(
    agent: AgentKind,
    dir: &Path,
    tako: Option<&tako_cli_path::Placement>,
    link_change: tako_cli_path::LinkChange,
) -> Result<Value, SetupRefusal> {
    let mut dirs = vec![dir.to_path_buf()];
    // #1502: tako 自身の CLI のディレクトリも同じ経路で足す（**末尾へ追記**なので
    // ユーザーが並べた優先順位は動かない）
    if let Some(plan) = tako {
        if !dirs.contains(&plan.dir) {
            dirs.push(plan.dir.clone());
        }
    }
    let path = ensure_user_path_dirs(&dirs)?;
    let verified = path["verified"].as_bool().unwrap_or(false);
    Ok(json!({
        "agent": agent.as_str(),
        "shell": path["shell"],
        // 書き先は「ファイル」ではないので、人へはレジストリのキーを見せる
        "profile": path["profile"],
        "profile_display": path["profile_display"],
        "dir": dir.display().to_string(),
        "dir_display": display_path(dir),
        "dirs": path["dirs"],
        "dirs_display": path["dirs_display"],
        "change": path["change"],
        "wrote": path["wrote"],
        "verified": verified,
        "tako_cli_link_change": link_change.as_str(),
        "tako_cli": tako_cli_path_json(),
        "note": user_path_note(agent, verified, user_path::rereads_for_new_panes()),
    }))
}

/// Windows の PATH 段の注記（**純粋関数**。読み直すかを引数で受ける）。
///
/// #2428 から、tako の新しいタブはレジストリの PATH を読み直すので「新しいタブ」でよいと案内する
/// （前の「新しいターミナルを開いてください」は、tako の新しいタブでは直らず再起動が要った）
fn user_path_note(agent: AgentKind, verified: bool, rereads: bool) -> String {
    if verified {
        return format!(
            "{} コマンドと tako コマンドがどのターミナルからも使えます",
            agent.as_str()
        );
    }
    if rereads {
        return "ユーザー環境変数 Path には入っていますが、いま開いているタブにはまだ届いていません。\
                tako で新しいタブを開くと使えます（tako の再起動は要りません。\
                tako の外のターミナルも、新しく開けば使えます）"
            .to_string();
    }
    "ユーザー環境変数へ追加しました。いま開いているターミナルには反映されないので、\
     新しいターミナルを開いてください"
        .to_string()
}

/// ユーザー環境変数 `Path` へ `dirs` を**1 エントリずつ末尾追記**する（境界 B23）。
///
/// unix と違いマーカーが無いので「ブロックごと書き直す」ができず、
/// エントリ単位の追記になる。既に在るものは触らない（冪等）
fn ensure_user_path_dirs(dirs: &[PathBuf]) -> Result<Value, SetupRefusal> {
    let process_path = std::env::var("PATH").unwrap_or_default();
    let mut change = PathChange::AlreadyOnPath;
    let mut process_has = true;
    for entry in dirs {
        process_has &= user_path::contains_entry(&process_path, entry);
        let current = user_path::read_typed()?;
        // レジストリに既に在る = 新しいターミナルからは引ける
        if let Some(next) = user_path::append_entry(&current.raw, entry) {
            user_path::write_typed(&user_path::UserPathValue {
                raw: next,
                kind: current.kind.clone(),
            })?;
            let after = user_path::read_typed()?;
            if !user_path::contains_entry(&after.raw, entry) {
                return Err(SetupRefusal::UserPathNotReflected {
                    dir: display_path(entry),
                });
            }
            change = PathChange::Installed;
        }
    }
    // レジストリへ入っていても**いまのプロセスには反映されない**（Windows は
    // 再ログイン / 新しいプロセス起動まで伝播しない。#525 実測）。
    // #2428 前は「レジストリに既に在った」だけで確かめたことにしていたので、このタブでは
    // 引けないのに「どのターミナルからも使えます」と言っていた
    let verified = process_has || (user_path::legacy_2428() && change == PathChange::AlreadyOnPath);
    Ok(json!({
        "shell": "windows-user-path",
        "profile": "HKCU\\Environment\\Path",
        "profile_display": "ユーザー環境変数 Path",
        "dirs": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "dirs_display": dirs.iter().map(|d| display_path(d)).collect::<Vec<_>>(),
        "change": change.as_str(),
        "wrote": change.wrote(),
        "verified": verified,
    }))
}

/// 置いた PATH ブロックを取り除く（元のバイト列へ戻す）
pub fn undo_path_for(agent: AgentKind) -> Result<Value, String> {
    undo_path_for_typed(agent).map_err(SetupRefusal::into_ja)
}

/// [`undo_path_for`] の型の口（#2365）
pub fn undo_path_for_typed(agent: AgentKind) -> Result<Value, SetupRefusal> {
    let home = home_dir_typed()?;
    let plan = tako_cli_placement(&home);
    // #1502: tako が張った symlink も一緒に外す（**symlink 以外は消さない**）
    let link_change = match plan.as_ref().and_then(|p| p.link.as_deref()) {
        Some(link) => tako_cli_path::remove_link_typed(link)?,
        None => tako_cli_path::LinkChange::Absent,
    };
    if user_path::is_supported() {
        return undo_user_path(
            agent,
            &recipe_for(agent).launcher_dir_in(&home),
            plan.as_ref(),
            link_change,
        );
    }
    let (shell, _) = shell_target();
    let shell = shell.ok_or(SetupRefusal::ShellProfileUnknownForUndo)?;
    let outcome = shell_profile::remove_from_profile_in_typed(&home, shell)?;
    Ok(json!({
        "agent": agent.as_str(),
        "shell": shell.as_str(),
        "profile": outcome.profile.display().to_string(),
        "change": outcome.change.as_str(),
        "tako_cli_link_change": link_change.as_str(),
        "tako_cli_link": plan.as_ref().and_then(|p| p.link.as_ref()).map(|l| l.display().to_string()),
    }))
}

/// Windows のユーザー PATH からランチャーの置き場所のエントリを取り除く。
/// **他のエントリは 1 つも触らない**（純粋関数側のテストで固定してある）。
///
/// ## unix との非対称（意図的）
///
/// unix はマーカーブロックを目印にするので「tako が置いた分」だけを外せる。
/// レジストリの PATH は 1 本の文字列で目印を置ける場所が無いため、
/// **誰が入れたかに関係なくランチャーの置き場所のエントリを外す**
/// （公式インストーラが入れた分も対象）。`undo-path` は AI / 利用者が
/// 明示的に叩くコマンドで、外した結果は `tako setup bootstrap path` で戻せるので
/// この非対称は受け入れる。目印のための状態ファイルは作らない
/// （#513 の共有カタログへ分類が要るものを、可逆な 1 操作のために増やさない）
fn undo_user_path(
    agent: AgentKind,
    dir: &Path,
    tako: Option<&tako_cli_path::Placement>,
    link_change: tako_cli_path::LinkChange,
) -> Result<Value, SetupRefusal> {
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(plan) = tako {
        if !dirs.contains(&plan.dir) {
            dirs.push(plan.dir.clone());
        }
    }
    let mut change = PathChange::Absent;
    for entry in &dirs {
        let current = user_path::read_typed()?;
        if let Some(next) = user_path::remove_entry(&current.raw, entry) {
            user_path::write_typed(&user_path::UserPathValue {
                raw: next,
                kind: current.kind.clone(),
            })?;
            change = PathChange::Removed;
        }
    }
    Ok(json!({
        "agent": agent.as_str(),
        "shell": "windows-user-path",
        "profile": "HKCU\\Environment\\Path",
        "dirs": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "change": change.as_str(),
        "tako_cli_link_change": link_change.as_str(),
    }))
}

// --- 自動導入が通らなかったときの引き継ぎ（#1057）---

/// 引き継ぎ先の並び（**入れる対象の系統は呼び出し側が外す**）。
///
/// 順序は claude → codex → agy（master を務められる系統を先に置く）。
///
/// # なぜ能力マトリクス（#982）の 1 マスにしないか
///
/// 「この CLI へ導入の代行を頼めるか」を [`crate::agent_support`] のキーにすると
/// **入れる対象の系統が `Unsupported`** になるが、それは相手の能力ではなく
/// 「いま入れようとしているのがそれだから」でしかない。マトリクスは
/// `claudeは基準系なので全て対応済み` で claude = 全 Supported を強制するので、
/// claude を入れる場合の「claude は候補にならない」は表せない（#1002 が同じ壁に当たった）。
/// なので候補の集合はここで宣言し、テスト（`引き継ぎ先に入れる対象自身を選ばない`）で拘束する
const HANDOFF_AGENTS: &[WorkerAgent] = &[WorkerAgent::Claude, WorkerAgent::Codex, WorkerAgent::Agy];

/// 引き継ぎ先 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffCandidate {
    pub agent: &'static str,
    pub path: String,
}

/// この環境で引き継げる相手（`target` = いま入れようとしている系統は外す）
pub fn handoff_candidates_for(target: AgentKind) -> Vec<HandoffCandidate> {
    if legacy_mode() {
        // #1057 前は「案内だけ」で引き継ぎ先を探さなかった
        return Vec::new();
    }
    HANDOFF_AGENTS
        .iter()
        .filter(|agent| agent.as_str() != target.as_str())
        .filter_map(|agent| {
            crate::orchestrator::agent_cli::locate(*agent)
                .ok()
                .map(|path| HandoffCandidate {
                    agent: agent.as_str(),
                    path,
                })
        })
        .collect()
}

/// 代行を頼む指示文（**純粋関数**なので文面をテストで固定できる）。
///
/// 「何を・どこへ・どうやって入れるか」と「やってはいけないこと」を書く。
/// 認証はブラウザ操作が要るので**代行させない**（ユーザーへ依頼させる）。
/// PATH の段は `tako setup bootstrap path` で通る形（既知のシェル・Windows）。種別の分からないシェルの文は
/// [`handoff_prompt_text_for`]
pub fn handoff_prompt_text(plan: &InstallPlan, reason: Option<&str>) -> String {
    handoff_prompt_text_for(plan, reason, None)
}

/// [`handoff_prompt_text`] の本体（**純粋関数**。#2583）。
///
/// `by_hand` は PATH を利用者が自分のシェルの設定ファイルで足すしかないときの断り
/// （[`path_by_hand_refusal`] = `tako setup bootstrap path` が返すのと同じ断り）。在れば PATH の段は
/// そのコマンドを示さず（同じ断りで止まる = #2564 / #2576 と同じ堂々巡り）、断りの全文を利用者へ伝えて
/// 足してもらう依頼にする（そのシェルの設定ファイルは tako が書かない = 代行させない。#2557）
pub fn handoff_prompt_text_for(
    plan: &InstallPlan,
    reason: Option<&str>,
    by_hand: Option<&SetupRefusal>,
) -> String {
    let reason = reason.unwrap_or("tako の自動インストールが通らなかった");
    // 系統名は `--agent` の値にそのまま使える形で出す（AI がコマンドを組めるように）
    let agent = plan.agent;
    let payload_line = match &plan.payload {
        Some(payload) => format!("         - 本体の置き場所: {}\n", display_path(payload)),
        // 単一バイナリの系統は本体 = コマンド自身（同じパスを 2 回言わない）
        None => String::new(),
    };
    let kind = AgentKind::parse(agent);
    let auth_command = kind
        .and_then(|a| crate::orchestrator::agent_cli::auth_command(a.into()))
        .unwrap_or("公式手順のログイン");
    // claude は既定なので `--agent` を付けない（#322 の最簡形）
    let flag = kind.map(agent_flag).unwrap_or_default();
    let path_step = match by_hand {
        Some(refusal) => format!(
            "3. `next_step` が `path` なら**ユーザーへ次の案内をそのまま伝えて PATH の追加を依頼する**\n\
             \x20  （このシェルの設定ファイルは tako が書けないので代行しない）:\n\
             {}\n\
             \x20  追加してもらったら 2 から確かめ直す\n",
            refusal
                .text_in(tako_core::i18n::Lang::Ja)
                .lines()
                .map(|line| format!("   > {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        None => format!("3. `next_step` が `path` なら `tako setup bootstrap path{flag}` を実行する\n"),
    };
    format!(
        "tako から引き継ぎ: {product}（{agent} CLI）の導入を代行してください。\n\
         \n\
         ## 状況\n\
         \n\
         - この環境で tako 自身の自動インストールが成立しませんでした（理由: {reason}）\n\
         - 公式の導入コマンド: {command}\n\
         - コマンドの置き場所: {launcher}\n\
         {payload_line}\
         - 管理者権限は不要です。ホームディレクトリの中だけで完結します\n\
         \n\
         ## やること（この順に）\n\
         \n\
         1. 上の公式コマンドを実行して {agent} を導入する。失敗したら出力のエラーを読み、\n\
         \x20  ネットワーク・プロキシ・ディスク容量などの原因を切り分けて対処する\n\
         2. `tako setup bootstrap status{flag} --json` で `next_step` を確認する\n\
         \x20  （`install` のままなら導入できていない）\n\
         {path_step}\
         4. `next_step` が `auth` なら**ユーザーへ `{auth_command}` の実行を依頼する**\n\
         \x20  （ブラウザ操作が要るので代行しない）\n\
         5. `next_step` が `ready` になったら `tako setup` を実行して設定を完了させる\n\
         \n\
         ## 守ること\n\
         \n\
         - 上の手順以外でユーザーの設定ファイル・PATH・レジストリを書き換えない\n\
         - 別の入れ方（Homebrew・npm 等）へ勝手に切り替えない。公式の native インストーラを使う\n\
         - うまくいかないときは、何がどこで失敗したかを日本語で報告して止まる\n",
        product = plan.product,
        command = plan.official_command,
        launcher = display_path(&plan.launcher),
    )
}

/// 引き継ぎの計画。CLI・MCP が同じ内容を見る（`available` が false なら
/// 従来どおり公式コマンドの案内へ落とす）
pub fn handoff_plan_for(agent: AgentKind, reason: Option<&str>) -> Result<Value, String> {
    handoff_plan_for_typed(agent, reason).map_err(SetupRefusal::into_ja)
}

/// [`handoff_plan_for`] の型の口（#2365）
pub fn handoff_plan_for_typed(
    agent: AgentKind,
    reason: Option<&str>,
) -> Result<Value, SetupRefusal> {
    let plan = install_plan_for_typed(agent)?;
    let candidates = handoff_candidates_for(agent);
    // 種別の分からないシェルでは `tako setup bootstrap path` も同じ断りで止まるので、PATH の段は
    // 利用者への依頼にする（判定は `tako setup` の残り・welcome と同じ 1 実装。#2583）
    let by_hand = if tako_core::platform::exe::issue2583_legacy() {
        None
    } else {
        path_by_hand_refusal()
    };
    let prompt = handoff_prompt_text_for(&plan, reason, by_hand.as_ref());
    let recommended = candidates.first();
    Ok(json!({
        "agent": agent.as_str(),
        "product": agent.product(),
        "available": !candidates.is_empty(),
        "reason": reason,
        "candidates": candidates
            .iter()
            .map(|c| json!({ "agent": c.agent, "path": c.path }))
            .collect::<Vec<_>>(),
        "recommended": recommended.map(|c| c.agent),
        "prompt": prompt,
        // そのまま打てる形（#322 の最簡形）。引数の prompt はシェルのクォートが
        // 要るので、CLI / GUI は argv で渡す。ここは「誰へ頼むか」を示す
        "launch_command": recommended.map(|c| format!("{} \"<上の prompt>\"", c.agent)),
        "install_plan": plan.to_json(),
        "fallback": format!(
            "引き継げる別のエージェント CLI がありません。\n\
             次のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {}",
            plan.official_command
        ),
    }))
}

// --- 依存ツールと Homebrew ---

/// 依存の検出は [`crate::setup_deps`] が正本（CLI・MCP・`--review` が同じ実装を通る）。
/// ここは `status()` の応答へ載せるだけ
fn deps_json() -> Value {
    crate::setup_deps::status_json()
}

/// Homebrew の状態と、無い場合の案内。
///
/// **自動導入はしない**。Homebrew のインストーラは sudo でパスワードを求める
/// （実物で確認: install.sh に sudo 参照 49 箇所・`have_sudo_access`）。
/// setup が黙って権限昇格を走らせるべきではないうえ、brew で入れる依存は
/// すべて任意なので、無くてもゼロスタートは完走できる
fn homebrew_json() -> Value {
    let brew = tako_core::platform::exe::find("brew");
    json!({
        "found": brew,
        "auto_install": false,
        "reason": "Homebrew のインストーラは管理者パスワードを求めるため、tako は代行しません",
        "guidance": "https://brew.sh の手順を実行すると、tmux などの任意依存を導入できるようになります",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2550: 種別の分からない `$SHELL` を zsh と見なさない（空・未設定のときだけ OS の既定）
    #[test]
    fn 種別の分からないシェルはzshと見なさず空と未設定だけ既定を仮定する() {
        for unknown in [
            "/bin/tcsh",
            "/opt/homebrew/bin/nu",
            "/usr/bin/false",
            "/usr/local/bin/elvish",
            "/nonexistent/tako-2550/xonsh",
            "  ",
        ] {
            for windows in [false, true] {
                assert_eq!(
                    shell_kind_for(unknown, windows),
                    None,
                    "{unknown:?}（windows={windows}）を既知のシェルと見なした = 読まれない profile へ書く"
                );
            }
        }
        // 空 = 未設定（`std::env::var` の失敗も空で来る）だけ OS の既定
        assert_eq!(shell_kind_for("", false), Some(ShellKind::Zsh));
        assert_eq!(shell_kind_for("", true), None);
        // 既存の利用者の答えは変えない（存在しないパスでも名前で決まる）
        for (shell, kind) in [
            ("/bin/zsh", ShellKind::Zsh),
            ("/nonexistent/tako-2550/zsh", ShellKind::Zsh),
            ("/bin/bash", ShellKind::Bash),
            ("/bin/sh", ShellKind::Bash),
            ("/opt/homebrew/bin/fish", ShellKind::Fish),
            (
                r"C:\Program Files\PowerShell\7\pwsh.exe",
                ShellKind::PowerShell,
            ),
        ] {
            assert_eq!(shell_kind_for(shell, false), Some(kind), "{shell}");
        }
    }

    /// #2583: `$SHELL` が空・未設定のとき、setup が書く profile の種別と、ログインシェルの問い合わせ
    /// （`exe::user_shell` の既定）が起こすシェルの種別が同じ（食い違うと書いた profile が読まれず、
    /// 初回起動の導線の段が `path` のまま残る）。macOS だけ（他の unix に zsh が在るとは限らない）
    #[cfg(target_os = "macos")]
    #[test]
    fn shellが空か未設定のときsetupが書くprofileと問い合わせるシェルの種別が同じ() {
        let asked = tako_core::platform::exe::fallback_login_shell();
        assert_eq!(
            ShellKind::from_shell_path(asked),
            shell_kind_for("", false),
            "setup は {:?} の profile へ書くが、ログインシェルの問い合わせは {asked} で起こす",
            shell_kind_for("", false)
        );
        assert_eq!(
            ShellKind::Zsh.login_profile_rel(),
            ".zprofile",
            "zsh のログインシェルが読む profile"
        );
    }

    /// #2564: PATH を利用者が自分で足すしかないのは「unix の種別の分からないシェル」だけ。
    /// 既知のシェルと Windows（ユーザー環境変数へ書く）は従来どおり `tako setup bootstrap path` で通る
    #[test]
    fn pathを自分で足すしかないのはunixの種別の分からないシェルだけ() {
        for (shell, windows) in [("/bin/tcsh", false), ("/usr/local/bin/nu", false)] {
            assert!(
                path_by_hand_for(shell_kind_for(shell, windows), windows),
                "{shell}"
            );
        }
        for (shell, windows) in [
            ("/bin/zsh", false),
            ("/bin/bash", false),
            ("/opt/homebrew/bin/fish", false),
            ("", false),
            // Windows は `$SHELL` が無く種別は None だが、ユーザー環境変数へ書くので当たらない
            ("", true),
            ("/bin/tcsh", true),
        ] {
            assert!(
                !path_by_hand_for(shell_kind_for(shell, windows), windows),
                "{shell} / windows={windows}"
            );
        }
    }

    #[test]
    fn claude_auth_jsonから認証とプランを取得する() {
        let value = serde_json::json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "subscriptionType": "Max"
        });
        assert_eq!(
            parse_claude_auth(&value),
            AuthState {
                authenticated: true,
                plan: Some("max".into())
            }
        );
        // 未ログインならプランも持たない
        assert_eq!(
            parse_claude_auth(&serde_json::json!({ "loggedIn": false })),
            AuthState::default()
        );

        let api = serde_json::json!({"loggedIn": true, "authMethod": "api_key"});
        assert_eq!(
            parse_claude_auth(&api),
            AuthState {
                authenticated: true,
                plan: Some("api".into())
            }
        );
    }

    #[test]
    fn codexのjwtからプランだけを取得する() {
        let dir =
            std::env::temp_dir().join(format!("tako-issue226-codex-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        let payload =
            "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19";
        std::fs::write(
            &path,
            format!(r#"{{"tokens":{{"id_token":"header.{payload}.signature"}}}}"#),
        )
        .unwrap();
        assert_eq!(codex_plan_from_auth_file_at(&path).as_deref(), Some("plus"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn プラン名の表記ゆれは1つへ寄る() {
        assert_eq!(normalize_plan(" Max 5x "), "max-5x");
        assert_eq!(normalize_plan("max_20x"), "max-20x");
        assert_eq!(normalize_plan("pro"), "pro");
    }

    /// そのプラットフォーム・系統の手順から計画を組む
    /// （**手順と文面の出どころを 1 つにする**。組み立ては製品と同じ `plan_from`）。
    /// `home` を引数で受けるので実 HOME を読まない
    fn plan_for(platform: Platform, home: &Path) -> InstallPlan {
        plan_of(platform, AgentKind::Claude, home)
    }

    fn plan_of(platform: Platform, agent: AgentKind, home: &Path) -> InstallPlan {
        plan_from(&agent_install::recipe(platform, agent), home)
    }

    /// 権限の呼び名がプラットフォームで変わること（#925）。
    ///
    /// 期待値は `Platform` を引数に取る純粋関数から作るので、**macOS から
    /// Windows 側の文面をそのまま突き合わせられる**（#515 / #905 と同じ作法）
    #[test]
    fn 権限の説明はプラットフォームで呼び名が変わる() {
        assert_eq!(
            privilege_line(Platform::MacOs),
            "sudo（管理者権限）は使いません。ホームディレクトリの中だけで完結します"
        );
        assert_eq!(
            privilege_line(Platform::Windows),
            "管理者権限は使いません。ホームディレクトリの中だけで完結します"
        );
        // どちらにも**プラットフォームに依らない語**が入る（#920 の項目 119 が
        // `管理者権限` で見ているので、出し分けを入れても落ちない）
        for platform in [Platform::MacOs, Platform::Windows] {
            assert!(
                privilege_line(platform).contains("管理者権限"),
                "{platform:?}: 共通語が消えている"
            );
        }
    }

    /// Windows 構成の画面文字列に unix 固有の語が出ないこと（#925）。
    ///
    /// GUI も Windows 実機も要らずに `cargo test` で固定できるのが要点。
    /// `.local/bin` は**両 OS 共通**の置き場なので禁止語に入れない（実際に Windows でも出る）
    #[test]
    fn windows構成にunix固有の語が出ない() {
        const UNIX_ONLY: &[&str] = &["sudo", "sudoers", "pmset", "chmod", "bash", "install.sh"];
        let home = Path::new("/tmp/h");

        let texts = plan_for(Platform::Windows, home).visible_texts();
        assert!(
            texts.len() >= 6,
            "画面に出る文字列を集められていない: {texts:?}"
        );
        let offenders: Vec<&String> = texts
            .iter()
            .filter(|t| UNIX_ONLY.iter().any(|word| t.contains(word)))
            .collect();
        assert!(
            offenders.is_empty(),
            "Windows で通じない語が出ている: {offenders:?}"
        );

        // macOS 側は unix の呼び名を残す（語をまるごと消しただけの退行を捕まえる）
        let mac = plan_for(Platform::MacOs, home).visible_texts();
        assert!(
            mac.iter().any(|t| t.contains("sudo")),
            "macOS 側の sudo の案内が消えている: {mac:?}"
        );
    }

    /// 取得物の見分け方は境界（B17）が持つ。ここではこの環境の署名で
    /// **HTML エラーページを弾けること**だけを確かめる
    /// （判定そのものの総当たりは `platform::agent_install` 側のテスト）
    #[test]
    fn この環境の署名でhtmlエラーページを弾く() {
        let signature = recipe_for(AgentKind::Claude).runner.signature;
        assert!(!agent_install::looks_like_installer(
            signature,
            b"<!DOCTYPE html>"
        ));
        assert!(!agent_install::looks_like_installer(signature, b""));
    }

    #[test]
    fn 段は導入状況から一意に決まる() {
        // Step の順序（Install → Path → Auth → Ready）を固定する
        assert_eq!(Step::Install.as_str(), "install");
        assert_eq!(Step::Path.as_str(), "path");
        assert_eq!(Step::Auth.as_str(), "auth");
        assert_eq!(Step::Ready.as_str(), "ready");
        // 段の説明は**どの系統の話か**を必ず含む（#989）
        for step in [Step::Install, Step::Path, Step::Auth, Step::Ready] {
            for agent in AgentKind::ALL {
                assert!(!step.describe_for(agent).is_empty());
            }
        }
        // Ready 以外は系統名か製品名が出る（claude 決め打ちへ戻っていない）
        for step in [Step::Install, Step::Path, Step::Auth] {
            let codex = step.describe_for(AgentKind::Codex);
            assert!(
                !codex.contains("Claude"),
                "codex の段に claude の語が出ている: {codex}"
            );
            assert_ne!(codex, step.describe_for(AgentKind::Claude));
        }
    }

    /// 受け入れ条件 4: 実行前に「何をどこに入れるか」が必ず出ること
    #[test]
    fn 導入計画は何をどこに入れるかを必ず含む() {
        // **両プラットフォームぶんを macOS から検証する**（#515 / #920）。
        // リテラルを書かず「その計画の中身が行に出ているか」を見るので、
        // 手順が変わってもテストがずれない
        let home = Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            let r = agent_install::recipe(platform, AgentKind::Claude);
            let plan = plan_for(platform, home);
            // 置き場所の表示は**実行中の OS** の区切りになる（`launcher_path_in` は
            // `PathBuf`）。`launcher_rel` / `payload_rel` は `/` 区切りの静的文字列なので
            // 突き合わせる前に寄せる。これを忘れると Windows でだけ落ちる（#920 の原因）
            let lines = plan.lines().join("\n").replace('\\', "/");
            assert!(
                lines.contains(r.source.official_command),
                "{platform:?}: 公式コマンドが出ていない: {lines}"
            );
            assert!(
                lines.contains(r.launcher_rel),
                "{platform:?}: 置き場所が出ていない: {lines}"
            );
            if let Some(payload) = r.payload_rel {
                assert!(
                    lines.contains(payload),
                    "{platform:?}: 本体の置き場所が出ていない: {lines}"
                );
            }
            assert!(
                lines.contains("管理者権限"),
                "{platform:?}: 権限の説明が無い: {lines}"
            );
            // **他方の手順が混ざっていない**（計画がプラットフォームに依らなくなる
            // 退行の検出。`install.sh` と `install.ps1` は互いに排他）
            let other = agent_install::recipe(
                match platform {
                    Platform::MacOs => Platform::Windows,
                    Platform::Windows => Platform::MacOs,
                },
                AgentKind::Claude,
            );
            assert!(
                !lines.contains(other.source.official_command),
                "{platform:?}: 別プラットフォームの手順が混ざっている: {lines}"
            );
            let json = plan.to_json();
            assert_eq!(json["can_run"], r.tako_can_run);
            assert!(json["lines"].as_array().is_some_and(|a| a.len() >= 5));
        }
    }

    /// Windows も PowerShell 経路で代行する（#1057。実機実測を経て倒した）
    #[test]
    fn windowsもpowershell経路で代行する() {
        let r = agent_install::recipe(
            tako_core::platform::support::Platform::Windows,
            AgentKind::Claude,
        );
        assert!(r.tako_can_run);
        assert!(r.source.official_command.contains("install.ps1"));
    }

    #[test]
    fn 依存はすべて任意でhomebrewは自動導入しない() {
        let brew = homebrew_json();
        assert_eq!(brew["auto_install"], false);
        assert!(brew["reason"]
            .as_str()
            .is_some_and(|s| s.contains("パスワード")));
        for dep in deps_json().as_array().unwrap() {
            assert_eq!(dep["required"], false, "必須の依存を増やさない");
        }
    }

    /// 引き継ぎの指示文には「何を・どこへ・どうやって」と禁止事項が入る（#1057）。
    /// **両プラットフォームぶんを macOS から検証する**
    #[test]
    fn 引き継ぎの指示文は導入計画から作られる() {
        let home = Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            let r = agent_install::recipe(platform, AgentKind::Claude);
            let plan = plan_for(platform, home);
            let prompt = handoff_prompt_text(&plan, Some("取得が 403 で失敗した"));
            assert!(prompt.contains(r.source.official_command), "{prompt}");
            assert!(prompt.contains("取得が 403 で失敗した"), "{prompt}");
            // 認証は代行させない（ブラウザ操作が要る）
            assert!(prompt.contains("claude auth login"), "{prompt}");
            assert!(prompt.contains("代行しない"), "{prompt}");
            // 次の一手が機械的に辿れる形で入っている
            assert!(prompt.contains("tako setup bootstrap status"), "{prompt}");
            assert!(prompt.contains("tako setup bootstrap path"), "{prompt}");
            // 勝手な入れ方への切り替えを禁じる
            assert!(prompt.contains("Homebrew"), "{prompt}");
            // 他方の手順が混ざらない
            let other = agent_install::recipe(
                match platform {
                    Platform::MacOs => Platform::Windows,
                    Platform::Windows => Platform::MacOs,
                },
                AgentKind::Claude,
            );
            assert!(
                !prompt.contains(other.source.official_command),
                "{platform:?}: 別プラットフォームの手順が混ざっている: {prompt}"
            );
        }
    }

    /// #2583: 種別の分からないシェルでは PATH の段が `tako setup bootstrap path`（同じ断りで止まる）を示さず、
    /// 同じ断りの全文を利用者へ伝えて足してもらう依頼になる。それ以外の段は既定の口と同じ行のまま
    #[test]
    fn 未知のシェルの引き継ぎの指示文はpathの段で同じ断りで止まるコマンドを示さない() {
        let plan = plan_for(Platform::MacOs, Path::new("/tmp/h"));
        let refusal = SetupRefusal::ShellProfileUnknown {
            shell: "/bin/tcsh".to_string(),
            dirs: vec!["$HOME/.local/bin".to_string()],
        };
        let known = handoff_prompt_text(&plan, None);
        let by_hand = handoff_prompt_text_for(&plan, None, Some(&refusal));
        assert!(!by_hand.contains("bootstrap path"), "{by_hand}");
        assert!(by_hand.contains("代行しない"), "{by_hand}");
        // 断りの全文（どのシェルか・足すディレクトリ）を引用の形で伝える
        for line in refusal.text_in(tako_core::i18n::Lang::Ja).lines() {
            assert!(
                by_hand.lines().any(|l| l == format!("   > {line}")),
                "断りの行 {line:?} を伝えていない: {by_hand}"
            );
        }
        // PATH の段の他は同じ（差し替えるのは段 3 だけ）
        let without_step3 = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            let mut in_step3 = false;
            for line in text.lines() {
                if line.starts_with("3. ") {
                    in_step3 = true;
                } else if line.starts_with("4. ") {
                    in_step3 = false;
                }
                if !in_step3 {
                    out.push(line.to_string());
                }
            }
            out
        };
        assert_eq!(without_step3(&known), without_step3(&by_hand));
        assert!(known
            .contains("3. `next_step` が `path` なら `tako setup bootstrap path` を実行する\n"));
    }

    /// 引き継ぎ先に**入れる対象自身**を選ばない（入れる対象なので候補になりえない）。
    /// #989 で対象が 3 系統になったので、どの系統を入れるときも自分を外す
    #[test]
    fn 引き継ぎ先に入れる対象自身を選ばない() {
        // master を務められる系統を先に置く（claude → codex → agy）
        assert_eq!(HANDOFF_AGENTS.first(), Some(&WorkerAgent::Claude));
        assert_eq!(HANDOFF_AGENTS.len(), AgentKind::ALL.len());
        for target in AgentKind::ALL {
            for candidate in handoff_candidates_for(target) {
                assert_ne!(
                    candidate.agent,
                    target.as_str(),
                    "{} を入れるのに自分が候補",
                    target.as_str()
                );
            }
        }
    }

    /// 候補が居なければ従来の案内（公式コマンド）へ落ちる
    #[test]
    fn 候補が居なければ公式コマンドの案内へ落ちる() {
        let _guard = crate::orchestrator::agent_cli::test_force_missing(&[
            WorkerAgent::Claude,
            WorkerAgent::Codex,
            WorkerAgent::Agy,
        ]);
        for target in AgentKind::ALL {
            let plan = handoff_plan_for(target, Some("テスト")).expect("計画は作れる");
            assert_eq!(plan["available"], false, "{}", target.as_str());
            assert!(plan["candidates"].as_array().is_some_and(|a| a.is_empty()));
            assert_eq!(plan["recommended"], serde_json::Value::Null);
            let fallback = plan["fallback"].as_str().unwrap_or_default();
            assert!(
                fallback.contains(recipe_for(target).source.official_command),
                "{fallback}"
            );
        }
    }

    /// 候補が居れば「誰へ頼むか」が決まる
    #[test]
    fn 候補が居れば引き継ぎ先が決まる() {
        let _guard = crate::orchestrator::agent_cli::test_force_found(&[(
            WorkerAgent::Codex,
            "/tmp/fake/codex",
        )]);
        let plan = handoff_plan_for(AgentKind::Claude, None).expect("計画は作れる");
        assert_eq!(plan["available"], true);
        assert_eq!(plan["recommended"], "codex");
        assert!(plan["prompt"].as_str().is_some_and(|p| !p.is_empty()));
    }

    /// 3 系統ぶんの状態を一度に返す（`tako setup --check` と同じ材料。#989）
    #[test]
    fn 状態は3系統ぶんまとめて返せる() {
        let value = status_all_json();
        let agents = value["agents"].as_array().expect("配列");
        assert_eq!(agents.len(), AgentKind::ALL.len());
        let names: Vec<&str> = agents.iter().filter_map(|a| a["agent"].as_str()).collect();
        for agent in AgentKind::ALL {
            assert!(names.contains(&agent.as_str()), "{names:?}");
        }
    }

    /// `TAKO_989_LEGACY=1` で claude 1 系統だけへ戻る（同一バイナリでの A/B）。
    ///
    /// env を書き換えるので、この 1 本だけの子で走らせる（他のテストの `bootstrap_agents` が
    /// 旧挙動を読まない。#2400）
    #[test]
    fn legacy_envで対象がclaudeだけへ戻る() {
        // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
        if tako_core::run_alone!(legacy_envで対象がclaudeだけへ戻る) {
            return;
        }
        assert_eq!(bootstrap_agents(), AgentKind::ALL.to_vec());
        // SAFETY: 子のプロセスにはこのテストしか居ない（`run_alone!`）
        unsafe { std::env::set_var("TAKO_989_LEGACY", "1") };
        let legacy = bootstrap_agents();
        unsafe { std::env::remove_var("TAKO_989_LEGACY") };
        assert_eq!(legacy, vec![AgentKind::Claude]);
    }

    /// 3 系統の計画が**それぞれ違う中身**になる（1 系統ぶんを 3 回出していない）
    #[test]
    fn 系統ごとに違う計画が出る() {
        let home = Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            let plans: Vec<InstallPlan> = AgentKind::ALL
                .into_iter()
                .map(|agent| plan_of(platform, agent, home))
                .collect();
            for (i, a) in plans.iter().enumerate() {
                for b in plans.iter().skip(i + 1) {
                    assert_ne!(a.official_command, b.official_command, "{platform:?}");
                    assert_ne!(a.launcher, b.launcher, "{platform:?}");
                    assert_ne!(a.product, b.product, "{platform:?}");
                }
                // 各計画の行に自分の系統名が出る（どれの話か分かる）
                let lines = a.lines().join("\n");
                assert!(lines.contains(a.product), "{lines}");
                assert!(lines.contains(a.update_note), "{lines}");
            }
        }
    }

    /// 引き継ぎの指示文が系統ごとにその系統のログインコマンドを案内する（#989）
    #[test]
    fn 引き継ぎの指示文は系統ごとのログインコマンドを案内する() {
        let home = Path::new("/tmp/h");
        for agent in AgentKind::ALL {
            let plan = plan_of(Platform::MacOs, agent, home);
            let prompt = handoff_prompt_text(&plan, None);
            let cmd = crate::orchestrator::agent_cli::auth_command(agent.into())
                .expect("3 系統ともログインの入口がある");
            assert!(
                prompt.contains(cmd),
                "{} の指示文にログインコマンドが無い: {prompt}",
                agent.as_str()
            );
            // 次の一手は**そのまま打てる最簡形**（claude は `--agent` を付けない）
            assert!(
                prompt.contains(&format!(
                    "tako setup bootstrap status{} --json",
                    agent_flag(agent)
                )),
                "{} の指示文の次の一手が最簡形でない: {prompt}",
                agent.as_str()
            );
            // 単一バイナリの系統は「本体の置き場所」を言わない（同じパスを 2 回言わない）
            assert_eq!(
                prompt.contains("本体の置き場所"),
                plan.payload.is_some(),
                "{}",
                agent.as_str()
            );
        }
    }

    // --- 新しいタブで引ける / このタブで引ける（#2428）---

    /// 認証の段にいる Windows の導入状況（パスはプレースホルダ = #927）
    fn auth_state(on_path: bool, on_process_path: bool) -> BootstrapState {
        let home = Path::new("C:\\Users\\winuser");
        BootstrapState {
            agent: AgentKind::Claude,
            binary: Some("C:\\Users\\winuser\\.local\\bin\\claude.exe".to_string()),
            authenticated: false,
            account_plan: None,
            launcher_dir: PathBuf::from("C:\\Users\\winuser\\.local\\bin"),
            launcher_dir_on_path: on_path,
            launcher_dir_on_process_path: on_process_path,
            profile: None,
            profile_has_block: false,
            shell: None,
            step: Step::Auth,
            plan: plan_of(Platform::Windows, AgentKind::Claude, home),
            auth_timeout: None,
            locate_timeout: None,
            path_timeout: None,
        }
    }

    #[test]
    fn 新しいタブで引けてこのタブで引けないときだけ新しいタブを勧める() {
        // 公式インストーラがユーザー PATH へ足した直後（#2428 の症状の形）
        assert!(auth_state(true, false).needs_new_tab_in(true));
        // このタブでも引ける = 勧めない
        assert!(!auth_state(true, true).needs_new_tab_in(true));
        // 新しいタブでも引けない = PATH の段の話なので、新しいタブを勧めても直らない
        assert!(!auth_state(false, false).needs_new_tab_in(true));
        // 新しいペインが読み直さない環境（unix・A/B の旧アーム）では立てない = 文面を変えない
        assert!(!auth_state(true, false).needs_new_tab_in(false));
        // 未導入なら開くタブの話にならない
        let mut missing = auth_state(true, false);
        missing.binary = None;
        assert!(!missing.needs_new_tab_in(true));
    }

    #[test]
    fn 新しいタブの1行は次の1手の手前に入る() {
        let base = auth_instructions_for(AgentKind::Claude);
        let note = new_tab_note_text(AgentKind::Claude);
        let lines = with_new_tab_note(base.clone(), Some(note.clone()));
        let at_note = lines.iter().position(|l| *l == note).expect("1 行が入る");
        let at_next = lines
            .iter()
            .position(|l| l.starts_with("次の 1 手"))
            .expect("次の 1 手が残る");
        assert_eq!(at_note + 1, at_next, "次の 1 手の直前に置く: {lines:?}");
        assert_eq!(lines.len(), base.len() + 1, "ほかの行は 1 行も変えない");
        // 文面は「新しいタブ」「再起動は要らない」と系統名を名指す
        assert!(note.contains("新しいタブ") && note.contains("再起動は要りません"));
        assert!(note.contains("claude"));
        // 添えるものが無ければ元のまま（unix の案内は 1 文字も変わらない）
        assert_eq!(with_new_tab_note(base.clone(), None), base);
        // 「次の 1 手」の行が無い案内でも落とさず末尾へ置く
        let tail = with_new_tab_note(vec!["a".to_string()], Some(note.clone()));
        assert_eq!(tail, vec!["a".to_string(), note]);
    }

    #[test]
    fn windowsのpath段の注記はこのタブで引けるかで変わる() {
        let agent = AgentKind::Claude;
        // このタブで引ける
        assert!(user_path_note(agent, true, true).contains("どのターミナルからも使えます"));
        // レジストリにだけある: 新しいタブを開けばよい（tako の再起動は要らない）
        let note = user_path_note(agent, false, true);
        assert!(note.contains("tako で新しいタブ"), "{note}");
        assert!(note.contains("再起動は要りません"), "{note}");
        // A/B の旧アーム（新しいペインが読み直さない）は前の文面のまま
        assert_eq!(
            user_path_note(agent, false, false),
            "ユーザー環境変数へ追加しました。いま開いているターミナルには反映されないので、\
             新しいターミナルを開いてください"
        );
    }

    #[test]
    fn 新しいタブで引けるかとこのタブで引けるかをjsonで分けて返す() {
        let state = auth_state(true, false);
        let json = state.to_json();
        assert_eq!(json["launcher_dir_on_path"], true);
        assert_eq!(json["launcher_dir_on_process_path"], false);
        // `needs_new_tab` は実行中の環境で決まる（Windows だけが立てる）
        assert_eq!(json["needs_new_tab"], user_path::rereads_for_new_panes());
    }
}

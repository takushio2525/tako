//! agent CLI の実在検査と、見つからないときの「理由 + 次の一手」（Issue #983）
//!
//! ## なぜ要るか
//!
//! codex / agy の CLI が入っていない環境で spawn すると、tako は**構造化された失敗を
//! 1 つも出さなかった**（実測。#983 の棚卸し）:
//!
//! - spawn 前の実在検査が無いので、組み立てたコマンドはそのままシェルへ流れ、
//!   ペインに `command not found` が出るだけ
//! - 送達（`shell_send`。#640）は「エコーが返って実行された」までしか見ないので**成功扱い**
//! - `registry::prompt_delivery_assessment` は claude 以外を `NotApplicable` で即返すので
//!   `PromptUndelivered` にもならない
//!
//! 結果として「spawn は成功したと言われたのに worker が何もしない」= **無言死**になる。
//! ここは**ペインを作る前**に落として、理由と次の一手を返すための層。
//!
//! ## 設計
//!
//! - 判断は**純粋関数**（`problem_of` / `guidance`）。`locate` だけが実際に探す
//! - 文言は [`Note`]（日英）。**「何が無いか」だけでなく「次に何をするか」を必ず含める**
//! - 分類は enum。#983 の変更 3（未認証 / 事前信頼の失敗 / 起動直後の異常終了 …）で
//!   系統を足す前提。**新しい無言経路を作らない**のがこの enum の目的
//! - 導入手順は agent ごとに**実物で確認した形**だけを書く（推測の URL は載せない）

use super::agent::WorkerAgent;
use tako_core::agent_support::Agent;
use tako_core::i18n::Lang;
use tako_core::platform::support::Note;

/// 起動経路の系統（`WorkerAgent`）を、マトリクスの系統（`Agent`）へ写す。
/// **起動経路にローカル LLM はまだ無い**（#990 / #991）ので、この向きだけが全射
fn agent_of(agent: WorkerAgent) -> Agent {
    Agent::parse(agent.as_str()).unwrap_or(Agent::Claude)
}

/// agent CLI を起動できない / 起動しても仕事が始まらない理由の分類（#983 の変更 3）。
///
/// **「起動に失敗した」を 1 種類に潰さない**のがこの enum の目的。
/// 潰すと利用者には「動かない」としか伝わらず、次の一手が出せない
/// （見本: qwen-web-lab の `harness/ollama.py` は接続不能を
/// 「理由 + `ollama serve` が動いているか確認」まで分解して返す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCliProblem {
    /// PATH（と既知の設置先）に実行ファイルが無い
    NotFound,
    /// 実行ファイルはあるがログインしていない（起動しても認証を求められて作業が始まらない）
    NotAuthenticated,
    /// 作業フォルダの事前信頼（#32 / #558）を書き込めなかった。
    /// **致命ではない**（tako が信頼ダイアログを検出して承諾する）が、黙って遅くなるので言う
    TrustWriteFailed,
    /// 起動直後に終了した（ペインがシェルのプロンプトへ戻っている）
    ExitedImmediately,
    /// ローカル LLM の runtime へ接続できない（Ollama が起動していない等）
    LocalRuntimeDown,
    /// 指定のモデルがローカルに取得されていない（`ollama pull` 前）
    LocalModelMissing,
}

impl AgentCliProblem {
    /// 全種別（テストが 4 系統 × 全種別を総当たりするための正本）
    pub const ALL: [AgentCliProblem; 6] = [
        Self::NotFound,
        Self::NotAuthenticated,
        Self::TrustWriteFailed,
        Self::ExitedImmediately,
        Self::LocalRuntimeDown,
        Self::LocalModelMissing,
    ];

    /// 機械可読な種別（応答 JSON・ログ用）
    pub fn kind(self) -> &'static str {
        match self {
            Self::NotFound => "cli_not_found",
            Self::NotAuthenticated => "not_authenticated",
            Self::TrustWriteFailed => "trust_write_failed",
            Self::ExitedImmediately => "exited_immediately",
            Self::LocalRuntimeDown => "local_runtime_down",
            Self::LocalModelMissing => "local_model_missing",
        }
    }

    /// この失敗がその系統で起こりうるか。
    ///
    /// **起こりえない組み合わせに文言を用意しない**（claude に「Ollama が起動していない」と
    /// 言わない / ローカルモデルに「ログインしてください」と言わない）。
    /// 逆に、起こりうるのに文言が無い組み合わせはテストが落とす
    pub fn applies_to(self, agent: Agent) -> bool {
        match self {
            // 実行ファイルが要るのは 4 系統とも同じ（ローカルは runtime の実行ファイル）
            Self::NotFound | Self::ExitedImmediately => true,
            // ローカルで動かすモデルにログインという概念が無い
            Self::NotAuthenticated | Self::TrustWriteFailed => agent != Agent::Local,
            // runtime とモデルの取得はローカル固有
            Self::LocalRuntimeDown | Self::LocalModelMissing => agent == Agent::Local,
        }
    }

    /// 「何が起きたか」。**どの CLI の話かが必ず分かる形**にする
    fn reason(self, agent: Agent, lang: Lang) -> String {
        let name = cli_name(agent);
        let note = match self {
            Self::NotFound => Note::new(
                "が見つかりません（PATH と既知の設置先を探しました）",
                " was not found (looked in PATH and the known install location)",
            ),
            Self::NotAuthenticated => Note::new(
                "はログインしていません（起動しても認証を求められて作業が始まりません）",
                " is not signed in (it starts up but asks for authentication instead of working)",
            ),
            Self::TrustWriteFailed => Note::new(
                "用の事前信頼（作業フォルダを信頼済みとして登録する設定）を書き込めませんでした",
                "'s pre-trust entry (registering the working folder as trusted) could not be written",
            ),
            Self::ExitedImmediately => Note::new(
                "が起動直後に終了しました（ペインはシェルのプロンプトへ戻っています）",
                " exited immediately after launch (the pane is back at the shell prompt)",
            ),
            Self::LocalRuntimeDown => Note::new(
                "に接続できません（ローカル LLM の runtime が起動していない可能性）",
                " could not be reached (the local-LLM runtime may not be running)",
            ),
            Self::LocalModelMissing => Note::new(
                "に指定のモデルがありません（まだ取得していない可能性）",
                " does not have the requested model (it may not have been pulled yet)",
            ),
        };
        format!("{name} CLI{}", note.text_in(lang))
    }

    /// 「次に何をするか」。**空を返してはいけない**（無言にしないための不変条件。
    /// テストが 4 系統 × 全種別で 1 行以上あることを見る）
    fn next_steps(self, agent: Agent, lang: Lang) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        match self {
            Self::NotFound => {
                let g = guidance(agent);
                match (g.command, g.manual) {
                    (Some(cmd), _) => {
                        out.push(next_step_install().text_in(lang).replace("{cmd}", cmd));
                    }
                    (None, Some(manual)) => out.push(manual.text_in(lang).to_string()),
                    (None, None) => out.push(next_step_generic().text_in(lang).to_string()),
                }
                out.push(next_step_path().text_in(lang).to_string());
                if let Some(url) = g.docs_url {
                    out.push(format!("{}{url}", reference_label().text_in(lang)));
                }
            }
            Self::NotAuthenticated => {
                let cmd = auth_command(agent).unwrap_or_else(|| cli_name(agent));
                out.push(
                    Note::new(
                        "次の一手: `{cmd}` でログインし、そのうえで spawn し直す",
                        "Next: sign in with `{cmd}`, then spawn again",
                    )
                    .text_in(lang)
                    .replace("{cmd}", cmd),
                );
                out.push(
                    Note::new(
                        "アカウントを使い分けているなら、その worker の `--account` が指す資格情報でログインしているか確認する",
                        "If you keep separate accounts, check that the credentials this worker's `--account` points at are the ones you signed in with",
                    )
                    .text_in(lang)
                    .to_string(),
                );
            }
            Self::TrustWriteFailed => {
                out.push(
                    Note::new(
                        "次の一手: 設定ディレクトリの書き込み権限を直してから spawn し直す",
                        "Next: fix write permissions on the config directory, then spawn again",
                    )
                    .text_in(lang)
                    .to_string(),
                );
                out.push(
                    Note::new(
                        "直さなくても tako が信頼ダイアログを検出して承諾しますが、そのぶん最初の指示が届くまで遅くなります",
                        "Even without fixing it tako detects and accepts the trust dialog, but the first instruction takes longer to land",
                    )
                    .text_in(lang)
                    .to_string(),
                );
            }
            Self::ExitedImmediately => {
                out.push(
                    Note::new(
                        "次の一手: 同じコマンドをそのペインで手で実行し、終了理由を見る（ログイン切れ・引数の不一致が多い）",
                        "Next: run the same command by hand in that pane and read why it exits (expired login and unsupported arguments are the usual causes)",
                    )
                    .text_in(lang)
                    .to_string(),
                );
                if let Some(cmd) = auth_command(agent) {
                    out.push(
                        Note::new(
                            "ログイン切れなら `{cmd}` で入り直す",
                            "If the login expired, sign in again with `{cmd}`",
                        )
                        .text_in(lang)
                        .replace("{cmd}", cmd),
                    );
                }
            }
            Self::LocalRuntimeDown => {
                out.push(
                    Note::new(
                        "次の一手: `ollama serve` が動いているか確認する（起動していなければ立ち上げてから spawn し直す）",
                        "Next: check that `ollama serve` is running (start it, then spawn again)",
                    )
                    .text_in(lang)
                    .to_string(),
                );
            }
            Self::LocalModelMissing => {
                out.push(
                    Note::new(
                        "次の一手: `ollama pull <モデル名>` で取得してから spawn し直す",
                        "Next: fetch it with `ollama pull <model>`, then spawn again",
                    )
                    .text_in(lang)
                    .to_string(),
                );
                out.push(
                    Note::new(
                        "手元にあるモデルは `ollama list` で確認できる",
                        "`ollama list` shows which models you already have",
                    )
                    .text_in(lang)
                    .to_string(),
                );
            }
        }
        out
    }
}

/// 利用者がターミナルで打つ名前。**ローカル LLM だけは CLI 名ではなく runtime 名**
/// （`Agent::Local.as_str()` は "local" で、そんなコマンドは存在しない）
fn cli_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Local => "ollama",
        other => other.as_str(),
    }
}

/// ログインのコマンド（**正本**。`agent_models` の未認証案内もここを引く）。
/// ローカル LLM に認証は無いので `None`
pub fn auth_command(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Claude => Some("claude auth login"),
        Agent::Codex => Some("codex login"),
        // agy は専用のログインサブコマンドが無く、引数なし起動でサインインへ入る
        // （実測のエラー文: `Launch the CLI without arguments to sign in.`）
        Agent::Agy => Some("agy"),
        Agent::Local => None,
    }
}

/// 「理由 + 次の一手」の**唯一の組み立て口**（表示言語を明示する純粋関数）。
///
/// 4 系統（claude / codex / agy / ローカル LLM）× 全種別の文言はここだけで決まるので、
/// テストが総当たりで固定できる
pub fn problem_message_in(agent: Agent, problem: AgentCliProblem, lang: Lang) -> String {
    let mut out = problem.reason(agent, lang);
    out.push_str(match lang {
        Lang::Ja => "。",
        Lang::En => ".",
    });
    for step in problem.next_steps(agent, lang) {
        out.push_str("\n  ");
        out.push_str(&step);
    }
    out
}

/// 導入の案内（実物で確認した形だけを持つ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallGuidance {
    /// 1 行で実行できる公式コマンド。**無い agent は None**（推測を書かない）
    pub command: Option<&'static str>,
    /// 参考 URL（公式）。無ければ None
    pub docs_url: Option<&'static str>,
    /// コマンドが無い agent 向けの言い換え（何をすれば入るか）
    pub manual: Option<Note>,
}

/// agent ごとの導入案内。
///
/// - claude: 手順の正本は境界 B17（`platform::agent_install`）が持つので**そこから引く**
///   （#868 と食い違わせない。プラットフォーム差もそちらが面倒を見る）
/// - codex: codex 自身の更新ダイアログが提示する形（実測。0.144.4 の
///   `Update now (runs sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | …')`）
/// - agy: 公開された 1 行コマンドを**確認できていない**（実体は Antigravity 同梱の
///   単一バイナリ）。推測を書かず「導入して PATH へ通す」だけを案内する
pub fn guidance(agent: Agent) -> InstallGuidance {
    use tako_core::platform::agent_install::{self, AgentKind};
    match agent {
        Agent::Claude => InstallGuidance {
            command: Some(
                agent_install::current_recipe(AgentKind::Claude)
                    .source
                    .official_command,
            ),
            docs_url: Some("https://code.claude.com/docs/en/setup"),
            manual: None,
        },
        Agent::Codex => InstallGuidance {
            command: Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
            docs_url: Some("https://developers.openai.com/codex/cli"),
            manual: None,
        },
        // ローカル LLM の runtime。`ollama` は公式が 1 行コマンドを公開している
        Agent::Local => InstallGuidance {
            command: Some("curl -fsSL https://ollama.com/install.sh | sh"),
            docs_url: Some("https://ollama.com/download"),
            manual: None,
        },
        Agent::Agy => InstallGuidance {
            command: None,
            docs_url: None,
            manual: Some(Note::new(
                "Antigravity CLI（agy）を導入して PATH へ通してください",
                "Install the Antigravity CLI (agy) and put it on your PATH",
            )),
        },
    }
}

/// 起動できない理由（表示用）。`message()` が「理由 + 次の一手」を組む
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCliError {
    pub agent: WorkerAgent,
    pub problem: AgentCliProblem,
}

impl AgentCliError {
    /// 利用者・AI へ返す 1 通の説明。**次の一手を必ず含める**
    pub fn message(&self) -> String {
        self.message_in(tako_core::i18n::lang())
    }

    /// 言語を明示しての説明。**表示言語グローバルに触らず検査できる**ようにするため、
    /// 実体はこちらの純粋関数に置く（#608 / #807 で踏んだ並列テストの競合対策と同じ作法）
    pub fn message_in(&self, lang: Lang) -> String {
        problem_message_in(agent_of(self.agent), self.problem, lang)
    }

    /// 応答 JSON へ載せる形（`kind` で機械が分岐できる）
    pub fn to_json(self) -> serde_json::Value {
        let g = guidance(agent_of(self.agent));
        serde_json::json!({
            "kind": self.problem.kind(),
            "agent": self.agent.as_str(),
            "install_command": g.command,
            "docs_url": g.docs_url,
            "message": self.message(),
        })
    }
}

fn next_step_install() -> Note {
    Note::new(
        "次の一手: `{cmd}` で導入し、新しいシェルで `--version` が出ることを確認する",
        "Next: install it with `{cmd}`, then confirm `--version` works in a new shell",
    )
}

fn next_step_generic() -> Note {
    Note::new(
        "次の一手: この CLI を導入して PATH へ通す",
        "Next: install the CLI and put it on your PATH",
    )
}

fn next_step_path() -> Note {
    Note::new(
        "導入済みなら PATH が通っていません（`tako setup` が検出と PATH 通しを案内します）",
        "If it is already installed, it is not on your PATH (`tako setup` walks through detection and PATH setup)",
    )
}

fn reference_label() -> Note {
    Note::new("参考: ", "Reference: ")
}

/// ペインの画面から**起動そのものの失敗**を分類する（#983 の変更 3）。
///
/// 呼び出し側は「まだ送達の証拠が無い worker」に限って使うこと。一度でも仕事が
/// 始まった worker の scrollback には、agent が実行したコマンドの `command not found` が
/// 普通に流れる（そこで起動失敗と言うと誤検知になる）。
///
/// パターンは**実採取の文字列だけ**を使う（推測の文言を足すと誤検知の温床になる）:
///
/// - `command not found` / `is not recognized as`（PowerShell）に**その agent 自身の名前**が
///   同居する行 = シェルが起動コマンドを解決できなかった
/// - agy: `Please sign in to view available models.` /
///   `Launch the CLI without arguments to sign in.`（実測。#1002）
/// - claude: `Not logged in`（実測。#652 / #877）/ `OAuth session expired`（実測）
pub fn detect_launch_failure(agent: WorkerAgent, output: &str) -> Option<AgentCliProblem> {
    let name = agent.as_str();
    // 末尾 20 行だけを見る（起動は最後の出来事なので、古い行まで遡ると誤検知が増える）
    let tail: Vec<&str> = {
        let all: Vec<&str> = output.lines().collect();
        all[all.len().saturating_sub(20)..].to_vec()
    };
    for l in &tail {
        let has_name = l.contains(name);
        if has_name && (l.contains("command not found") || l.contains("is not recognized as")) {
            return Some(AgentCliProblem::NotFound);
        }
    }
    if tail.iter().any(|l| looks_unauthenticated_screen(l)) {
        return Some(AgentCliProblem::NotAuthenticated);
    }
    None
}

/// 画面 1 行が「ログインしていない」と言っているか（**実採取の文言だけ**）。
///
/// `agent_models::looks_unauthenticated` はコマンドの stderr を見る用で
/// `login` / `sign in` のような 1 語も拾う。画面は agent の作業出力が流れるので
/// そのままでは誤検知する（コードに `login` と書いてあるだけで反応してしまう）
fn looks_unauthenticated_screen(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "not logged in",
        "oauth session expired",
        "please sign in",
        "please log in",
        "without arguments to sign in",
        "invalid api key",
        "please run /login",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// #983 の A/B 用の env。`TAKO_983_LEGACY=1` で**同一バイナリのまま** #983 の
/// 3 つの変更をまとめて外し、旧挙動へ戻す:
///
/// 1. 変更 1: 実在検査をしない（組み立てたコマンドをそのままシェルへ流す）
/// 2. 変更 2: 送達判定が `agent != "claude"` を `NotApplicable` で即返す（= 黙る）
/// 3. 変更 3: 画面からの起動失敗の分類をしない（`idle` = 完了に見える）
pub fn legacy_mode() -> bool {
    std::env::var("TAKO_983_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 変更 1 の A/B（呼び出し側で意図が読めるように名前を分けてある）
pub fn legacy_skip_preflight() -> bool {
    legacy_mode()
}

// テスト専用: 「この agent は見つからない」ことにする（スレッドローカル）。
//
// 実在検査は環境そのものを見るので、テストから「無い状態」を作るには env を
// 触るしかない —— が、`cargo test` は並列なので env の書き換えは他のテストへ漏れる
// （#608 / #807 で表示言語グローバルで踏んだ型）。**スレッドローカル**にすると
// 同じスレッドで走るそのテストにだけ効く
#[cfg(test)]
thread_local! {
    static TEST_MISSING: std::cell::RefCell<Vec<WorkerAgent>> =
        const { std::cell::RefCell::new(Vec::new()) };
    // 「見つかったことにする」側。**実探索（ログインシェルの起動）を避ける**ための穴で、
    // dispatch の正常系テストが使う。実探索そのものは agent_cli の unit テストが 1 回だけ
    // 通す（プロセス全体の fd 数を見ている `ipc::連続接続でfdが漏れない` は、テストが
    // 一時的に開く fd で揺れる。重い経路をテストの本筋から外して安定させる）
    static TEST_FOUND: std::cell::RefCell<Vec<(WorkerAgent, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// テスト専用: 抜けると元へ戻す番人
#[cfg(test)]
pub struct TestMissingGuard;

#[cfg(test)]
impl Drop for TestMissingGuard {
    fn drop(&mut self) {
        TEST_MISSING.with(|m| m.borrow_mut().clear());
        TEST_FOUND.with(|m| m.borrow_mut().clear());
    }
}

/// テスト専用: 指定した agent を「見つからない」ことにする
#[cfg(test)]
pub fn test_force_missing(agents: &[WorkerAgent]) -> TestMissingGuard {
    TEST_MISSING.with(|m| *m.borrow_mut() = agents.to_vec());
    TestMissingGuard
}

/// テスト専用: 指定した agent を「このパスで見つかった」ことにする
#[cfg(test)]
pub fn test_force_found(agents: &[(WorkerAgent, &str)]) -> TestMissingGuard {
    TEST_FOUND
        .with(|m| *m.borrow_mut() = agents.iter().map(|(a, p)| (*a, (*p).to_string())).collect());
    TestMissingGuard
}

#[cfg(test)]
fn forced_missing(agent: WorkerAgent) -> bool {
    TEST_MISSING.with(|m| m.borrow().contains(&agent))
}

#[cfg(test)]
fn forced_found(agent: WorkerAgent) -> Option<String> {
    TEST_FOUND.with(|m| {
        m.borrow()
            .iter()
            .find(|(a, _)| *a == agent)
            .map(|(_, p)| p.clone())
    })
}

/// agent CLI の実行ファイルを探す。見つかればそのパス。
///
/// **探し方は境界 B16（`platform::exe::find`）1 本**（#898。unix はログインシェル経由、
/// Windows は PATH + PATHEXT + ユーザー導入先）。claude だけは「入れた直後で PATH に
/// 反映されていない」状態を #868 の `resolve_binary` が拾えるので、その順で見る
/// （`tako setup` の `find_agent_command` と同じ考え方）
pub fn locate(agent: WorkerAgent) -> Result<String, AgentCliError> {
    // テストでは**既定で実探索をしない**。実探索は unix ではログインシェルを起動する
    // （`$SHELL -l -c`）ので、spawn 系テスト 1 本ごとに子プロセスが増え、プロセス全体の
    // fd 数を見ている `ipc::連続接続でfdが漏れない` が 6 秒間の最小値でも tolerance を
    // 超える（実測: fd 10 → 14。main では緑）。探索そのものは別のテストバイナリにある
    // `tako_core::platform::exe::tests::実環境の基本コマンドを解決できる` が担保する
    #[cfg(test)]
    {
        if forced_missing(agent) {
            return Err(AgentCliError {
                agent,
                problem: AgentCliProblem::NotFound,
            });
        }
        Ok(forced_found(agent).unwrap_or_else(|| format!("/test-stub/bin/{}", agent.as_str())))
    }
    #[cfg(not(test))]
    {
        if let Some(found) = tako_core::platform::exe::find(agent.as_str()) {
            return Ok(found);
        }
        if agent == WorkerAgent::Claude {
            if let Some(found) = crate::setup_bootstrap::resolve_binary() {
                return Ok(found);
            }
        }
        Err(AgentCliError {
            agent,
            problem: AgentCliProblem::NotFound,
        })
    }
}

/// ペインを作る**前**に呼ぶ実在検査。`TAKO_983_LEGACY=1` のときは素通しする
pub fn preflight(agent: WorkerAgent) -> Result<Option<String>, AgentCliError> {
    if legacy_skip_preflight() {
        return Ok(None);
    }
    locate(agent).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 見つからない理由には次の一手が必ず付く() {
        for agent in WorkerAgent::ALL {
            let err = AgentCliError {
                agent,
                problem: AgentCliProblem::NotFound,
            };
            for (lang, next) in [(Lang::Ja, "次の一手"), (Lang::En, "Next:")] {
                let msg = err.message_in(lang);
                assert!(
                    msg.contains(agent.as_str()),
                    "どの CLI の話か分かること: {msg}"
                );
                // コマンドが無い agent は「導入して PATH へ通す」の言い換えで代替する
                assert!(
                    msg.contains(next) || msg.contains("Antigravity"),
                    "次に何をするかが書かれていること（{lang:?}）: {msg}"
                );
                assert!(
                    msg.contains("tako setup"),
                    "導入済みで PATH 未通しの場合の道を示すこと: {msg}"
                );
            }
        }
    }

    #[test]
    fn 導入案内は推測のコマンドを持たない() {
        // 実物で確認した agent だけがコマンドを持つ（agy は単一バイナリで
        // 公開された 1 行コマンドを確認できていない）
        assert!(guidance(Agent::Claude).command.is_some());
        assert!(guidance(Agent::Codex).command.is_some());
        assert!(guidance(Agent::Agy).command.is_none());
        assert!(
            guidance(Agent::Agy).manual.is_some(),
            "コマンドが無い agent には言い換えを用意する（無言にしない）"
        );
        // claude の手順は B17 の正本から引く（#868 と二重管理しない）
        assert_eq!(
            guidance(Agent::Claude).command,
            Some(
                tako_core::platform::agent_install::current_recipe(
                    tako_core::platform::agent_install::AgentKind::Claude
                )
                .source
                .official_command
            )
        );
    }

    #[test]
    fn 応答jsonは機械可読な種別を持つ() {
        let v = AgentCliError {
            agent: WorkerAgent::Codex,
            problem: AgentCliProblem::NotFound,
        }
        .to_json();
        assert_eq!(v["kind"].as_str(), Some("cli_not_found"));
        assert_eq!(v["agent"].as_str(), Some("codex"));
        assert!(v["install_command"].as_str().is_some());
        assert!(v["message"].as_str().unwrap().contains("codex"));
    }

    // 探索経路（B16 = `platform::exe::find`）そのものの健全性は
    // `tako_core::platform::exe::tests::実環境の基本コマンドを解決できる` が見ている。
    // **ここで同じことをやり直さない**: unix の実探索はログインシェルを起動するので、
    // このクレートのテストバイナリでやると `ipc::連続接続でfdが漏れない`（プロセス全体の
    // fd 数を見る）を数秒ぶん揺らして落とす（実測: fd 10 → 14。main では緑）

    #[test]
    fn 系統と種別の総当たりで理由と次の一手が必ず出る() {
        // #983 の受け入れ条件 1: 4 系統 × 各失敗で「理由 + 次の一手」を固定する。
        // **起こりえない組み合わせには文言を用意しない**（applies_to で宣言する）
        for agent in Agent::ALL {
            for problem in AgentCliProblem::ALL {
                if !problem.applies_to(agent) {
                    continue;
                }
                for lang in [Lang::Ja, Lang::En] {
                    let msg = problem_message_in(agent, problem, lang);
                    let steps = problem.next_steps(agent, lang);
                    assert!(
                        !steps.is_empty(),
                        "次の一手が空 = 無言と同じ（{agent:?} / {problem:?} / {lang:?}）"
                    );
                    assert!(
                        msg.contains(cli_name(agent)),
                        "どの CLI の話か分かること（{agent:?} / {problem:?}）: {msg}"
                    );
                    // 「次の一手」は 1 行目とは別の行に出る（読み手が探せる形）
                    assert!(
                        msg.lines().count() >= 2,
                        "理由と次の一手が分かれていること: {msg}"
                    );
                }
                // 日本語と英語で別の文言になっている（英語が日本語の写しでない）
                assert_ne!(
                    problem_message_in(agent, problem, Lang::Ja),
                    problem_message_in(agent, problem, Lang::En),
                );
            }
        }
    }

    #[test]
    fn 起こりえない組み合わせは宣言で外れている() {
        // ローカルで動かすモデルにログイン・事前信頼は無い
        assert!(!AgentCliProblem::NotAuthenticated.applies_to(Agent::Local));
        assert!(!AgentCliProblem::TrustWriteFailed.applies_to(Agent::Local));
        // runtime とモデル取得はローカル固有（claude に「ollama serve」と言わない）
        for agent in [Agent::Claude, Agent::Codex, Agent::Agy] {
            assert!(!AgentCliProblem::LocalRuntimeDown.applies_to(agent));
            assert!(!AgentCliProblem::LocalModelMissing.applies_to(agent));
        }
        assert!(AgentCliProblem::LocalRuntimeDown.applies_to(Agent::Local));
        assert!(AgentCliProblem::LocalModelMissing.applies_to(Agent::Local));
    }

    #[test]
    fn 種別のslugは重複しない() {
        let mut kinds: Vec<&str> = AgentCliProblem::ALL.iter().map(|p| p.kind()).collect();
        kinds.sort_unstable();
        let before = kinds.len();
        kinds.dedup();
        assert_eq!(before, kinds.len(), "slug が重複している: {kinds:?}");
    }

    #[test]
    fn 画面から起動失敗を分類する() {
        // シェルが起動コマンドを解決できなかった（unix / PowerShell）
        assert_eq!(
            detect_launch_failure(WorkerAgent::Codex, "zsh: command not found: codex"),
            Some(AgentCliProblem::NotFound)
        );
        assert_eq!(
            detect_launch_failure(
                WorkerAgent::Agy,
                "agy : The term 'agy' is not recognized as a name of a cmdlet"
            ),
            Some(AgentCliProblem::NotFound)
        );
        // 別のコマンドが見つからないのは起動失敗ではない（agent 名が同居しない）
        assert_eq!(
            detect_launch_failure(WorkerAgent::Codex, "zsh: command not found: rg"),
            None
        );
        // 未認証（実採取の文言）
        for line in [
            "Error: Please sign in to view available models.",
            "Not logged in",
            "Failed to authenticate: OAuth session expired",
        ] {
            assert_eq!(
                detect_launch_failure(WorkerAgent::Claude, line),
                Some(AgentCliProblem::NotAuthenticated),
                "未認証として分類されること: {line}"
            );
        }
        // 通常の作業画面は何も言わない（誤検知ゼロが最優先）
        for line in [
            "  Read(src/login.rs)",
            "> ok, running the tests now",
            "fn login() -> Result<(), Error> {",
        ] {
            assert_eq!(
                detect_launch_failure(WorkerAgent::Claude, line),
                None,
                "作業出力を起動失敗と誤認しないこと: {line}"
            );
        }
    }

    #[test]
    fn ログインコマンドの正本はここにある() {
        // #1002 の未認証案内（agent_models）も同じ表を引く = 二重管理しない
        assert_eq!(auth_command(Agent::Claude), Some("claude auth login"));
        assert_eq!(auth_command(Agent::Codex), Some("codex login"));
        assert_eq!(auth_command(Agent::Agy), Some("agy"));
        assert_eq!(
            auth_command(Agent::Local),
            None,
            "ローカルで動かすモデルに認証は無い"
        );
    }

    #[test]
    fn 文言は日英で用意されている() {
        for note in [
            next_step_install(),
            next_step_generic(),
            next_step_path(),
            reference_label(),
        ] {
            assert!(!note.ja().is_empty());
            assert!(!note.en().is_empty());
            assert_ne!(note.ja(), note.en(), "英語が日本語の写しになっていない");
        }
        assert!(next_step_install().ja().contains("{cmd}"));
        assert!(next_step_install().en().contains("{cmd}"));
    }
}

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
use tako_core::i18n::Lang;
use tako_core::platform::support::Note;

/// agent CLI を起動できない理由の分類（#983）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCliProblem {
    /// PATH（と既知の設置先）に実行ファイルが無い
    NotFound,
}

impl AgentCliProblem {
    /// 機械可読な種別（応答 JSON・ログ用）
    pub fn kind(self) -> &'static str {
        match self {
            Self::NotFound => "cli_not_found",
        }
    }

    fn reason(self, agent: WorkerAgent, lang: Lang) -> String {
        let name = agent.as_str();
        match self {
            Self::NotFound => Note::new(
                "が見つかりません（PATH と既知の設置先を探しました）",
                " was not found (looked in PATH and the known install location)",
            )
            .text_in(lang)
            .to_string()
            .pipe_prefix(name),
        }
    }
}

/// `String` の前に CLI 名を差し込むだけの補助（文言を 1 箇所に保つため）
trait PipePrefix {
    fn pipe_prefix(self, name: &str) -> String;
}

impl PipePrefix for String {
    fn pipe_prefix(self, name: &str) -> String {
        format!("{name} CLI{self}")
    }
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
pub fn guidance(agent: WorkerAgent) -> InstallGuidance {
    use tako_core::platform::agent_install::{self, AgentKind};
    match agent {
        WorkerAgent::Claude => InstallGuidance {
            command: Some(
                agent_install::current_recipe(AgentKind::Claude)
                    .source
                    .official_command,
            ),
            docs_url: Some("https://code.claude.com/docs/en/setup"),
            manual: None,
        },
        WorkerAgent::Codex => InstallGuidance {
            command: Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
            docs_url: Some("https://developers.openai.com/codex/cli"),
            manual: None,
        },
        WorkerAgent::Agy => InstallGuidance {
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
        let mut out = self.problem.reason(self.agent, lang);
        out.push_str(match lang {
            Lang::Ja => "。\n  ",
            Lang::En => ".\n  ",
        });
        let g = guidance(self.agent);
        match (g.command, g.manual) {
            (Some(cmd), _) => {
                out.push_str(&next_step_install().text_in(lang).replace("{cmd}", cmd));
            }
            (None, Some(manual)) => out.push_str(manual.text_in(lang)),
            (None, None) => out.push_str(next_step_generic().text_in(lang)),
        }
        out.push_str("\n  ");
        out.push_str(next_step_path().text_in(lang));
        if let Some(url) = g.docs_url {
            out.push_str("\n  ");
            out.push_str(reference_label().text_in(lang));
            out.push_str(url);
        }
        out
    }

    /// 応答 JSON へ載せる形（`kind` で機械が分岐できる）
    pub fn to_json(self) -> serde_json::Value {
        let g = guidance(self.agent);
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

/// #983 の A/B 用の env。`TAKO_983_LEGACY=1` で**同一バイナリのまま**
/// 「実在検査をしない（コマンドをそのままシェルへ流す）」旧挙動へ戻す
pub fn legacy_skip_preflight() -> bool {
    std::env::var("TAKO_983_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
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
        assert!(guidance(WorkerAgent::Claude).command.is_some());
        assert!(guidance(WorkerAgent::Codex).command.is_some());
        assert!(guidance(WorkerAgent::Agy).command.is_none());
        assert!(
            guidance(WorkerAgent::Agy).manual.is_some(),
            "コマンドが無い agent には言い換えを用意する（無言にしない）"
        );
        // claude の手順は B17 の正本から引く（#868 と二重管理しない）
        assert_eq!(
            guidance(WorkerAgent::Claude).command,
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

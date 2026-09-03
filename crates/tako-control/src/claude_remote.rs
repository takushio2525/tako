//! Claude 公式 Remote Control の opt-in と適格性判定（Issue #1068 / エピック #1059）
//!
//! ## 何をする層か
//!
//! tako が spawn する master / worker / solo は、既定では Remote Control に繋がらない
//! （実測: tako 管轄の 5 セッションすべて未接続。`research/2026-09-01-remote-renewal-claude-official.md` §2.4）。
//! プロファイルの `remote_control` を true にしたときだけ、claude の起動コマンドへ
//! `--remote-control` を足す。
//!
//! ## なぜ既定 OFF なのか
//!
//! Remote Control に委譲した会話は **Anthropic のサーバーに transcript が保存され**、
//! 認証も claude.ai アカウントへ移る（role 4 段は効かない）。
//! tako の現行モデルは「会話はローカルに閉じている」なので、
//! **黙って外へ同期させ始めてはいけない**（レポート §6 の設計上の帰結 1）。
//!
//! ## なぜ起動前に適格性を見るのか
//!
//! 適格でないアカウント・環境で `--remote-control` を渡すと claude 自身が起動時に
//! 失敗する（docs: `claude remote-control --help` は不適格だとフラグ一覧ではなく
//! エラーを返す）。tako から見ると**ペインが即死する**ので、
//! 「フラグを付けない + 理由と次の一手を出す」へ倒す。
//!
//! ## 「証明できるときだけ断る」
//!
//! ここで判定できるのは**ローカルの読み取りだけで確定する事実**に限る。
//! プラン・組織のエントイトルメント・Trusted Devices・ZDR は
//! ローカルからは分からないので、**分からないものを不適格と言わない**
//! （分からなければフラグを付けて claude 自身に言わせる）。
//! 過剰に断ると「使えるのに使えない」を作り、#982 の過小申告と同じ害になる。
//!
//! ## 判定の根拠（claude 2.1.232 のバイナリから採取した実装）
//!
//! ```text
//! function S_o(){ if(process.env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC) return "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC";
//!                 if(process.env.DISABLE_TELEMETRY) return "DISABLE_TELEMETRY";
//!                 if(Hn(process.env.DO_NOT_TRACK)) return "DO_NOT_TRACK"; return null }
//! function dkr(){ return !Y.DISABLE_GROWTHBOOK && Dhe() }
//! function GQu(){ let e=Y.ANTHROPIC_BASE_URL; if(!e) return; try{ let t=new URL(e).host;
//!                 if(t==="api.anthropic.com") return; return t }catch{ return } }
//! function Hn(e){ if(!e) return !1; ... return ["1","true","yes","on"].includes(t) }
//! ```
//!
//! 対応するユーザー向け文言も同じバイナリに在る:
//!
//! - `Remote Control requires feature-flag evaluation, which is disabled because ${t} is set.`
//! - `Remote Control is only available when using Claude via api.anthropic.com.`
//! - `Remote Control requires a full-scope login token. Long-lived tokens (from
//!   `claude setup-token` or CLAUDE_CODE_OAUTH_TOKEN) are limited to inference-only ...`
//! - `Remote Control is disabled by your organization's policy (managed setting
//!   `disableRemoteControl`).`
//!
//! **`DISABLE_TELEMETRY` と `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` は「空でなければ」**
//! 効く（値の真偽解釈をしない）が、`DO_NOT_TRACK` / `DISABLE_GROWTHBOOK` は真偽解釈を通る。
//! ここでもその差をそのまま写す（`DO_NOT_TRACK=0` で断ると誤って機能を殺す）。

use tako_core::platform::support::Note;

/// フラグの綴り。**1 箇所に持つ**（CLI / worker / master の 3 経路が同じ文字列を使う）
pub const REMOTE_CONTROL_FLAG: &str = "--remote-control";

/// 「空でなければ効く」env（値の真偽解釈をしない）
const RAW_BLOCKING_ENV: &[&str] = &[
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "DISABLE_TELEMETRY",
];

/// 真偽解釈を通してから効く env
const BOOL_BLOCKING_ENV: &[&str] = &["DO_NOT_TRACK", "DISABLE_GROWTHBOOK"];

/// API エンドポイントの差し替え（真偽値で切り替わるもの）
const REDIRECT_BOOL_ENV: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// claude.ai サブスク認証ではない認証（Remote Control は使えない）
const NON_SUBSCRIPTION_AUTH_ENV: &[&str] = &["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"];

/// claude の真偽 env 解釈（バイナリの `Hn`）。`1` / `true` / `yes` / `on` だけが真
pub fn env_is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// `ANTHROPIC_BASE_URL` が Remote Control を壊すか（バイナリの `GQu`）。
/// **`api.anthropic.com` を指しているなら壊さない**（差し替えていないので）。
/// URL として読めない値は判定材料にしない（claude 側も `catch` で無視する）
pub fn base_url_redirects(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    // `scheme://host[:port]/...` の host 部分だけを見る。`url` クレートは
    // ワークスペースに無いので、判定に必要な範囲だけを自前で切る
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // 認証情報つき（`user:pass@host`）と port を落とす
    let host = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
    !host.eq_ignore_ascii_case("api.anthropic.com")
}

/// 不適格の種別。**理由を型にする**ので、UI / CLI / 応答が同じ分類を共有する
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ineligible {
    /// フィーチャーフラグ評価が env で止められている
    FeatureFlagsDisabled { env_key: String },
    /// API エンドポイントが `api.anthropic.com` 以外へ向いている
    EndpointRedirected { env_key: String },
    /// claude.ai サブスク認証ではない（API キー / 長寿命トークン）
    NonSubscriptionAuth { env_key: String },
    /// 組織のポリシーで無効化されている（managed settings）
    DisabledByPolicy { source: String },
    /// この系統に Remote Control が無い（codex / agy）
    AgentUnsupported { agent: String },
}

impl Ineligible {
    /// 分類の識別子（応答 JSON・診断ログ用）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FeatureFlagsDisabled { .. } => "feature_flags_disabled",
            Self::EndpointRedirected { .. } => "endpoint_redirected",
            Self::NonSubscriptionAuth { .. } => "non_subscription_auth",
            Self::DisabledByPolicy { .. } => "disabled_by_policy",
            Self::AgentUnsupported { .. } => "agent_unsupported",
        }
    }

    /// 何がそう判定させたか（env 名 / 設定ファイル名 / 系統名）。
    /// **値は入れない**（`ANTHROPIC_API_KEY` の中身は秘匿情報）
    pub fn detail(&self) -> &str {
        match self {
            Self::FeatureFlagsDisabled { env_key }
            | Self::EndpointRedirected { env_key }
            | Self::NonSubscriptionAuth { env_key } => env_key,
            Self::DisabledByPolicy { source } => source,
            Self::AgentUnsupported { agent } => agent,
        }
    }

    /// なぜ使えないか（日英）
    pub fn reason(&self) -> Note {
        match self {
            Self::FeatureFlagsDisabled { .. } => Note::new(
                "Remote Control はフィーチャーフラグ評価を必要とするが、環境変数でそれが止められている",
                "Remote Control needs feature-flag evaluation, and an environment variable has disabled it",
            ),
            Self::EndpointRedirected { .. } => Note::new(
                "Remote Control は api.anthropic.com 経由のときだけ使える（Bedrock / Vertex / Foundry・エンドポイント差し替えは対象外）",
                "Remote Control only works when Claude talks to api.anthropic.com (Bedrock / Vertex / Foundry and endpoint overrides are out of scope)",
            ),
            Self::NonSubscriptionAuth { .. } => Note::new(
                "Remote Control は claude.ai サブスクのログインが必要（API キー・`claude setup-token` の長寿命トークンは推論専用スコープなので不可）",
                "Remote Control requires a claude.ai subscription login (API keys and the long-lived `claude setup-token` tokens are inference-only)",
            ),
            Self::DisabledByPolicy { .. } => Note::new(
                "組織のポリシー（managed settings の disableRemoteControl）で Remote Control が無効化されている",
                "Remote Control is disabled by your organization's policy (the managed setting disableRemoteControl)",
            ),
            Self::AgentUnsupported { .. } => Note::new(
                "この系統に Claude 公式の Remote Control に相当する仕組みが無い（claude 専用）",
                "This agent has no counterpart to Claude's official Remote Control (it is claude-only)",
            ),
        }
    }

    /// 次の一手（日英）。**押せば直るものだけを書く**
    pub fn next_step(&self) -> Note {
        match self {
            Self::FeatureFlagsDisabled { .. } => Note::new(
                "その環境変数を外したシェルから起動する（プロファイルの env で設定しているなら `tako orchestrator profiles set <名前> --env-unset <キー>`）",
                "Launch from a shell without that variable (if a profile sets it, run `tako orchestrator profiles set <name> --env-unset <KEY>`)",
            ),
            Self::EndpointRedirected { .. } => Note::new(
                "api.anthropic.com 直の構成で起動する（エンドポイント差し替えを外す）。差し替えが必要な環境では Remote Control は使えないので、tako 自前のリモート（`tako remote start`）を使う",
                "Launch against api.anthropic.com directly (drop the endpoint override). Where the override is required, Remote Control cannot be used: use tako's own remote instead (`tako remote start`)",
            ),
            Self::NonSubscriptionAuth { .. } => Note::new(
                "`claude auth login` で claude.ai アカウントにログインし、API キー・長寿命トークンの環境変数を外す",
                "Sign in with `claude auth login` using your claude.ai account, and unset the API-key / long-lived-token variables",
            ),
            Self::DisabledByPolicy { .. } => Note::new(
                "組織の管理者に Remote Control の許可を依頼する（tako 側では解除できない）",
                "Ask your organization admin to allow Remote Control (tako cannot override it)",
            ),
            Self::AgentUnsupported { .. } => Note::new(
                "会話をスマホから見たい場合は master / worker を claude で起動する（`tako agent-support --agent <系統>` で差を確認できる）",
                "To reach the conversation from a phone, launch the master / worker with claude (`tako agent-support --agent <agent>` shows the differences)",
            ),
        }
    }
}

/// 適格性の判定材料。**env と設定を読んだ結果だけ**を持つ（I/O から切り離すため）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EligibilityFacts {
    /// 起動先プロセスが実際に見る env（プロファイルの env 計画を反映済み）。
    /// **キーと値の対**で持つ。値は判定にしか使わず、外へは出さない
    pub env: Vec<(String, String)>,
    /// managed settings の `disableRemoteControl` が true か
    pub disabled_by_policy: Option<String>,
    /// 起動する agent 系統（`claude` 以外は Remote Control を持たない）
    pub agent: String,
}

impl EligibilityFacts {
    fn env_value(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// 判定結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// ローカルからは不適格の根拠が見つからない。
    /// **「適格と確認できた」ではない**（プラン・組織設定は claude 自身しか知らない）
    NoLocalBlocker,
    /// ローカルの読み取りだけで不適格が確定した
    Blocked(Ineligible),
}

impl Eligibility {
    pub fn blocker(&self) -> Option<&Ineligible> {
        match self {
            Self::NoLocalBlocker => None,
            Self::Blocked(b) => Some(b),
        }
    }
}

/// 適格性の判定（**純粋関数**。env グローバルにも設定ファイルにも触らない）。
///
/// 判定順は「確実に効くもの」から。`agent` が claude 以外なら env を見る前に落とす
/// （そもそもフラグが存在しない）
pub fn eligibility(facts: &EligibilityFacts) -> Eligibility {
    if facts.agent != "claude" {
        return Eligibility::Blocked(Ineligible::AgentUnsupported {
            agent: facts.agent.clone(),
        });
    }
    if let Some(source) = &facts.disabled_by_policy {
        return Eligibility::Blocked(Ineligible::DisabledByPolicy {
            source: source.clone(),
        });
    }
    for key in NON_SUBSCRIPTION_AUTH_ENV {
        if facts.env_value(key).is_some_and(|v| !v.trim().is_empty()) {
            return Eligibility::Blocked(Ineligible::NonSubscriptionAuth {
                env_key: (*key).to_string(),
            });
        }
    }
    if facts
        .env_value("ANTHROPIC_BASE_URL")
        .is_some_and(base_url_redirects)
    {
        return Eligibility::Blocked(Ineligible::EndpointRedirected {
            env_key: "ANTHROPIC_BASE_URL".to_string(),
        });
    }
    for key in REDIRECT_BOOL_ENV {
        if facts.env_value(key).is_some_and(env_is_true) {
            return Eligibility::Blocked(Ineligible::EndpointRedirected {
                env_key: (*key).to_string(),
            });
        }
    }
    // 「空でなければ効く」env が先（claude の S_o と同じ順序・同じ寛容さ）
    for key in RAW_BLOCKING_ENV {
        if facts.env_value(key).is_some_and(|v| !v.is_empty()) {
            return Eligibility::Blocked(Ineligible::FeatureFlagsDisabled {
                env_key: (*key).to_string(),
            });
        }
    }
    for key in BOOL_BLOCKING_ENV {
        if facts.env_value(key).is_some_and(env_is_true) {
            return Eligibility::Blocked(Ineligible::FeatureFlagsDisabled {
                env_key: (*key).to_string(),
            });
        }
    }
    Eligibility::NoLocalBlocker
}

/// 起動コマンドへ `--remote-control` を足すかどうかの決定（**純粋関数**）。
///
/// - `opt_in` が false → 何もしない（既定。**旧挙動と 1 バイトも変わらない**）
/// - `opt_in` かつローカルに阻害要因なし → フラグを足す
/// - `opt_in` だが不適格 → **フラグを足さず**理由を返す（ペインを即死させない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlDecision {
    /// 起動コマンドへ足すフラグ（無ければ `None`）
    pub flag: Option<&'static str>,
    /// opt-in していたのに使えなかった理由
    pub blocked: Option<Ineligible>,
}

impl RemoteControlDecision {
    /// 何もしない（既定 / legacy）
    pub fn off() -> Self {
        Self {
            flag: None,
            blocked: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.flag.is_some()
    }
}

/// opt-in と適格性からフラグの有無を決める（**純粋関数**）
pub fn decide(opt_in: bool, facts: &EligibilityFacts) -> RemoteControlDecision {
    if !opt_in {
        return RemoteControlDecision::off();
    }
    match eligibility(facts) {
        Eligibility::NoLocalBlocker => RemoteControlDecision {
            flag: Some(REMOTE_CONTROL_FLAG),
            blocked: None,
        },
        Eligibility::Blocked(b) => RemoteControlDecision {
            flag: None,
            blocked: Some(b),
        },
    }
}

/// #1068 の A/B 用の env。`TAKO_1068_LEGACY=1` で**同一バイナリのまま**
/// 「`--remote-control` を一度も付けない」旧挙動へ戻す
pub fn legacy_never_remote_control() -> bool {
    std::env::var("TAKO_1068_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

// --- 判定材料の収集（ここだけが I/O に触る） ---------------------------------

/// managed settings（組織ポリシー）の置き場。claude の docs に載っている固定パス。
/// **ユーザー設定は見ない**: project / local scope の `remoteControlAtStartup: true` は
/// claude 自身が無視する（repo-scoped では有効化できない）ので、
/// tako が読んで判断材料にすると実態とずれる
#[cfg(target_os = "macos")]
const MANAGED_SETTINGS: &str = "/Library/Application Support/ClaudeCode/managed-settings.json";
#[cfg(target_os = "windows")]
const MANAGED_SETTINGS: &str = r"C:\ProgramData\ClaudeCode\managed-settings.json";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const MANAGED_SETTINGS: &str = "/etc/claude-code/managed-settings.json";

/// managed settings の `disableRemoteControl` を読む（読めなければ判定しない）
fn policy_blocker_at(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if value.get("disableRemoteControl")?.as_bool()? {
        Some(path.display().to_string())
    } else {
        None
    }
}

/// 判定材料を集める（**読み取りだけ**）。
///
/// `plan` は起動時に注入される env 計画（プロファイル + アカウント解決の結果）。
/// **プロセス env に plan を重ねた結果**が起動先プロセスの見る env なので、
/// `unset` は取り除き `export` は上書きする
pub fn collect_facts(agent: &str, plan: &crate::orchestrator::EnvPlan) -> EligibilityFacts {
    let mut env: Vec<(String, String)> = Vec::new();
    let watched: Vec<&str> = RAW_BLOCKING_ENV
        .iter()
        .chain(BOOL_BLOCKING_ENV)
        .chain(REDIRECT_BOOL_ENV)
        .chain(NON_SUBSCRIPTION_AUTH_ENV)
        .copied()
        .chain(std::iter::once("ANTHROPIC_BASE_URL"))
        .collect();
    for key in watched {
        // 明示 unset が最優先（起動先には届かない）
        if plan.unsets.iter().any(|k| k == key) {
            continue;
        }
        // プロファイルの export はログインシェルの rc / direnv より後勝ち（#500）
        if let Some((_, v)) = plan.exports.iter().find(|(k, _)| k == key) {
            env.push((key.to_string(), v.clone()));
            continue;
        }
        if let Ok(v) = std::env::var(key) {
            env.push((key.to_string(), v));
        }
    }
    EligibilityFacts {
        env,
        disabled_by_policy: policy_blocker_at(std::path::Path::new(MANAGED_SETTINGS)),
        agent: agent.to_string(),
    }
}

/// 起動時に画面へ出す 1 行（#981 の `sandbox_bypass_line` と同じ思想）。
/// **付いているときも付いていないときも出す**ので、
/// 「スマホから見えない」と言われたときに理由が画面に残る
pub fn status_line(decision: &RemoteControlDecision) -> String {
    match (&decision.flag, &decision.blocked) {
        (Some(_), _) => Note::new(
            "Remote Control: 有効（claude.ai / Claude アプリからこの会話を操作できます。会話は Anthropic 側にも保存されます）",
            "Remote Control: on (this conversation can be driven from claude.ai and the Claude app; the transcript is also stored on Anthropic's side)",
        )
        .text()
        .to_string(),
        (None, Some(blocked)) => format!(
            "Remote Control: {} — {} / {}",
            Note::new("有効化できません", "could not be enabled").text(),
            blocked.reason().text(),
            blocked.next_step().text()
        ),
        (None, None) => Note::new(
            "Remote Control: 無効（この会話はローカルに閉じたままです）",
            "Remote Control: off (this conversation stays local)",
        )
        .text()
        .to_string(),
    }
}

/// 不適格の種別（[`Ineligible::kind`] の slug）から理由と次の一手を引き直す。
///
/// **なぜ slug を経由するのか**: リンク解決（`claude_remote_link`）が持つのは
/// `LinkState::Ineligible { reason: <slug> }` だけで、`Ineligible` の値そのものは
/// 握っていない（transcript の読み取り結果と env の判定は別の層で起きる）。
/// 型を持ち回す形にすると解決層が env 判定の詳細を抱えることになるので、
/// **slug を語彙として共有し、文言はここ 1 か所で引く**形にした。
///
/// 網羅は [`tests::不適格の全種別がslugから文言を引ける`] が拘束する
/// （新しい種別を足して分岐を忘れると落ちる）。
pub fn notes_for_kind(kind: &str) -> Option<(Note, Note)> {
    // 種別ごとに 1 個ずつ代表値を作って `reason` / `next_step` を引く。
    // detail（env 名など）は文言に出さないので空で構わない
    let sample = match kind {
        "feature_flags_disabled" => Ineligible::FeatureFlagsDisabled {
            env_key: String::new(),
        },
        "endpoint_redirected" => Ineligible::EndpointRedirected {
            env_key: String::new(),
        },
        "non_subscription_auth" => Ineligible::NonSubscriptionAuth {
            env_key: String::new(),
        },
        "disabled_by_policy" => Ineligible::DisabledByPolicy {
            source: String::new(),
        },
        "agent_unsupported" => Ineligible::AgentUnsupported {
            agent: String::new(),
        },
        _ => return None,
    };
    Some((sample.reason(), sample.next_step()))
}

/// `status_line` の OFF に添える「入れ方」の案内コマンド。
/// 引数は最簡形（既定値で済む引数を付けない。#322）
pub fn enable_hint_command(profile_name: &str) -> String {
    format!("tako orchestrator profiles set {profile_name} --remote-control true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::EnvPlan;

    fn facts(agent: &str, env: &[(&str, &str)]) -> EligibilityFacts {
        EligibilityFacts {
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            disabled_by_policy: None,
            agent: agent.to_string(),
        }
    }

    #[test]
    fn 素の環境ではローカルの阻害要因が無い() {
        assert_eq!(
            eligibility(&facts("claude", &[])),
            Eligibility::NoLocalBlocker
        );
    }

    #[test]
    fn フィーチャーフラグを止める_env_は不適格になる() {
        // 「空でなければ効く」ものは値の中身を問わない（claude の S_o と同じ）
        for key in [
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "DISABLE_TELEMETRY",
        ] {
            for value in ["1", "0", "false", "anything"] {
                let e = eligibility(&facts("claude", &[(key, value)]));
                assert_eq!(
                    e,
                    Eligibility::Blocked(Ineligible::FeatureFlagsDisabled {
                        env_key: key.to_string()
                    }),
                    "{key}={value} は不適格でなければならない"
                );
            }
        }
    }

    #[test]
    fn 真偽解釈を通す_env_は偽の値では断らない() {
        // DO_NOT_TRACK=0 で断ると、使えるのに使えない状態を作る
        for key in ["DO_NOT_TRACK", "DISABLE_GROWTHBOOK"] {
            for value in ["0", "false", "no", "off", ""] {
                assert_eq!(
                    eligibility(&facts("claude", &[(key, value)])),
                    Eligibility::NoLocalBlocker,
                    "{key}={value} で断ってはいけない"
                );
            }
            for value in ["1", "true", "YES", " on "] {
                assert_eq!(
                    eligibility(&facts("claude", &[(key, value)])),
                    Eligibility::Blocked(Ineligible::FeatureFlagsDisabled {
                        env_key: key.to_string()
                    }),
                    "{key}={value} は不適格でなければならない"
                );
            }
        }
    }

    #[test]
    fn base_url_は_api_anthropic_com_を指していれば阻害しない() {
        assert!(!base_url_redirects("https://api.anthropic.com"));
        assert!(!base_url_redirects("https://api.anthropic.com/v1"));
        assert!(!base_url_redirects("https://API.ANTHROPIC.COM:443/v1?x=1"));
        assert!(!base_url_redirects(""));
        assert!(base_url_redirects("https://gateway.example.test"));
        assert!(base_url_redirects("http://localhost:4000"));
        // 認証情報つきでも host を見る（`@` の手前に api.anthropic.com が入る偽装）
        assert!(base_url_redirects("https://api.anthropic.com@evil.test/v1"));
    }

    #[test]
    fn エンドポイント差し替えは不適格になる() {
        assert_eq!(
            eligibility(&facts(
                "claude",
                &[("ANTHROPIC_BASE_URL", "https://gw.example.test")]
            )),
            Eligibility::Blocked(Ineligible::EndpointRedirected {
                env_key: "ANTHROPIC_BASE_URL".to_string()
            })
        );
        for key in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ] {
            assert_eq!(
                eligibility(&facts("claude", &[(key, "1")])),
                Eligibility::Blocked(Ineligible::EndpointRedirected {
                    env_key: key.to_string()
                })
            );
        }
    }

    #[test]
    fn api_キー_長寿命トークン認証は不適格になる() {
        for key in ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            assert_eq!(
                eligibility(&facts("claude", &[(key, "sk-placeholder")])),
                Eligibility::Blocked(Ineligible::NonSubscriptionAuth {
                    env_key: key.to_string()
                })
            );
            // 空文字は「設定していない」と同じ扱い
            assert_eq!(
                eligibility(&facts("claude", &[(key, "  ")])),
                Eligibility::NoLocalBlocker
            );
        }
    }

    #[test]
    fn claude_以外の系統は_env_を見る前に落ちる() {
        for agent in ["codex", "agy"] {
            // 阻害 env が無くても落ちる（そもそもフラグが無い）
            assert_eq!(
                eligibility(&facts(agent, &[])),
                Eligibility::Blocked(Ineligible::AgentUnsupported {
                    agent: agent.to_string()
                })
            );
        }
    }

    #[test]
    fn 組織ポリシーは_env_より先に効く() {
        let mut f = facts("claude", &[("DISABLE_TELEMETRY", "1")]);
        f.disabled_by_policy = Some("managed-settings.json".into());
        assert_eq!(
            eligibility(&f),
            Eligibility::Blocked(Ineligible::DisabledByPolicy {
                source: "managed-settings.json".into()
            })
        );
    }

    #[test]
    fn opt_in_していなければフラグを付けない() {
        let d = decide(false, &facts("claude", &[]));
        assert_eq!(d, RemoteControlDecision::off());
        assert!(!d.enabled());
    }

    #[test]
    fn opt_in_して適格ならフラグを付ける() {
        let d = decide(true, &facts("claude", &[]));
        assert_eq!(d.flag, Some(REMOTE_CONTROL_FLAG));
        assert!(d.blocked.is_none());
    }

    #[test]
    fn opt_in_しても不適格ならフラグを付けず理由を返す() {
        let d = decide(true, &facts("claude", &[("DISABLE_TELEMETRY", "1")]));
        assert_eq!(d.flag, None, "不適格でフラグを付けるとペインが即死する");
        let blocked = d.blocked.expect("理由が要る");
        assert_eq!(blocked.kind(), "feature_flags_disabled");
        assert_eq!(blocked.detail(), "DISABLE_TELEMETRY");
        assert!(!blocked.reason().ja().is_empty());
        assert!(!blocked.reason().en().is_empty());
        assert!(!blocked.next_step().ja().is_empty());
        assert!(!blocked.next_step().en().is_empty());
    }

    #[test]
    fn 全種別が日英の理由と次の一手を持つ() {
        let all = [
            Ineligible::FeatureFlagsDisabled {
                env_key: "DISABLE_TELEMETRY".into(),
            },
            Ineligible::EndpointRedirected {
                env_key: "ANTHROPIC_BASE_URL".into(),
            },
            Ineligible::NonSubscriptionAuth {
                env_key: "ANTHROPIC_API_KEY".into(),
            },
            Ineligible::DisabledByPolicy {
                source: "managed-settings.json".into(),
            },
            Ineligible::AgentUnsupported {
                agent: "codex".into(),
            },
        ];
        let mut kinds = std::collections::BTreeSet::new();
        for b in &all {
            assert!(kinds.insert(b.kind()), "種別が重複している: {}", b.kind());
            for note in [b.reason(), b.next_step()] {
                assert!(!note.ja().is_empty() && !note.en().is_empty());
                assert_ne!(note.ja(), note.en(), "日英が同一文言になっている");
            }
        }
        assert_eq!(kinds.len(), all.len());
    }

    #[test]
    fn 収集は明示_unset_を反映する() {
        // プロファイルが unset していれば、プロセス env に在っても起動先には届かない
        let plan = EnvPlan {
            exports: vec![],
            unsets: vec!["DISABLE_TELEMETRY".into()],
        };
        let f = collect_facts("claude", &plan);
        assert!(
            !f.env.iter().any(|(k, _)| k == "DISABLE_TELEMETRY"),
            "unset したキーが材料に残っている"
        );
    }

    #[test]
    fn 収集はプロファイルの_export_を優先する() {
        let plan = EnvPlan {
            exports: vec![("DISABLE_TELEMETRY".into(), "1".into())],
            unsets: vec![],
        };
        let f = collect_facts("claude", &plan);
        assert_eq!(f.env_value("DISABLE_TELEMETRY"), Some("1"));
        assert!(matches!(
            eligibility(&f),
            Eligibility::Blocked(Ineligible::FeatureFlagsDisabled { .. })
        ));
    }

    #[test]
    fn 状態の_1_行は_3_通りとも中身がある() {
        let on = status_line(&RemoteControlDecision {
            flag: Some(REMOTE_CONTROL_FLAG),
            blocked: None,
        });
        let off = status_line(&RemoteControlDecision::off());
        let blocked = status_line(&RemoteControlDecision {
            flag: None,
            blocked: Some(Ineligible::FeatureFlagsDisabled {
                env_key: "DISABLE_TELEMETRY".into(),
            }),
        });
        for line in [&on, &off, &blocked] {
            assert!(line.starts_with("Remote Control:"), "{line}");
        }
        assert_ne!(on, off);
        assert_ne!(off, blocked);
        // 理由が読めない 1 行は出さない（無言の詰まりを作らない）
        assert!(blocked.contains("DISABLE_TELEMETRY") || blocked.len() > on.len());
    }

    #[test]
    fn 有効化の案内は最簡形() {
        assert_eq!(
            enable_hint_command("default"),
            "tako orchestrator profiles set default --remote-control true"
        );
    }

    /// `--remote-control` の綴りは claude 2.1.232 の `--help` と一致する
    /// （実測: `--remote-control [name]  Start an interactive session with Remote Control enabled`）
    #[test]
    fn フラグの綴りが上流と一致する() {
        assert_eq!(REMOTE_CONTROL_FLAG, "--remote-control");
    }

    #[test]
    fn ポリシー判定は読めないファイルでは何も言わない() {
        let dir = std::env::temp_dir().join("tako-1068-policy-test");
        let _ = std::fs::create_dir_all(&dir);
        let missing = dir.join("missing.json");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(policy_blocker_at(&missing), None);

        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        assert_eq!(policy_blocker_at(&broken), None);

        let off = dir.join("off.json");
        std::fs::write(&off, r#"{"disableRemoteControl": false}"#).unwrap();
        assert_eq!(policy_blocker_at(&off), None);

        let on = dir.join("on.json");
        std::fs::write(&on, r#"{"disableRemoteControl": true}"#).unwrap();
        assert_eq!(
            policy_blocker_at(&on),
            Some(on.display().to_string()),
            "true のときだけ断る"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不適格の種別を足したら文言も足す。**`kind()` の match 腕をソースから採る**ので、
    /// enum に値を増やして [`notes_for_kind`] の分岐を忘れると落ちる
    /// （型で縛れないのは slug を語彙として共有しているため。理由は関数の doc に書いた）
    #[test]
    fn 不適格の全種別がslugから文言を引ける() {
        let src = include_str!("claude_remote.rs");
        // `fn kind(&self) -> &'static str {` から次の `}` までの腕を採る
        let start = src
            .find("fn kind(&self) -> &'static str {")
            .expect("kind() が在る");
        let body = &src[start..];
        let end = body.find("\n    }").expect("kind() の閉じ");
        let arms = &body[..end];
        let mut kinds: Vec<&str> = Vec::new();
        for line in arms.lines() {
            if let Some((_, rest)) = line.split_once("=> \"") {
                if let Some((slug, _)) = rest.split_once('"') {
                    kinds.push(slug);
                }
            }
        }
        assert!(
            kinds.len() >= 5,
            "kind() の腕を採れていない（採取ロジックの破損）: {kinds:?}"
        );
        for kind in &kinds {
            let notes = notes_for_kind(kind);
            assert!(
                notes.is_some(),
                "種別 {kind} の文言が引けない（notes_for_kind に分岐を足すこと）"
            );
            let (reason, next) = notes.unwrap();
            // 日英どちらも空でない（#435: 新機能は日英必須）
            for note in [reason, next] {
                assert!(!note.ja().is_empty(), "{kind}: 日本語が空");
                assert!(!note.en().is_empty(), "{kind}: 英語が空");
            }
        }
        // 知らない slug は None（上流の書式変更で嘘の理由を出さない）
        assert!(notes_for_kind("nope").is_none());
    }
}

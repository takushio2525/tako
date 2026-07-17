//! `tako setup` — 質問ゼロを既定にした自動セットアップコマンド。
//!
//! エージェント CLI（claude / codex / agy）の検出・プラン解決 →
//! 依存ツールチェック → MCP 登録 → 指示・profile・リソース生成 → 最終サマリ、
//! の一連のフローを提供する。個別対話は明示的な `--review` だけで起動する。
//! IPC 不要で、tako アプリ未起動でも動作する。
//!
//! config.yaml のスキーマと setup changelog は `tako_control::setup` にある
//! （MCP `tako_setup_changes` と共有。二重実装を作らない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tako_control::setup::{
    load_config, pending_changes, resolve_setup_value, ChangeKind, ResolvedSetupValue,
    SetupAnswers, SetupChange, SetupPlan, SetupValueSource, CHANGES_YAML,
};

// --- バイナリ埋め込みリソース ---

const SYSTEM_PROMPT: &str = include_str!("../../../resources/setup/system-prompt.md");
const TPL_00_LANGUAGE: &str =
    include_str!("../../../resources/setup/templates/sections/00-language.md");
const TPL_01_INTERACTION: &str =
    include_str!("../../../resources/setup/templates/sections/01-interaction-style.md");
const TPL_02_GIT: &str =
    include_str!("../../../resources/setup/templates/sections/02-git-workflow.md");
const TPL_03_CODE: &str =
    include_str!("../../../resources/setup/templates/sections/03-code-quality.md");
const TPL_04_SAFETY: &str =
    include_str!("../../../resources/setup/templates/sections/04-safety-rules.md");
const TPL_05_PROPOSAL: &str =
    include_str!("../../../resources/setup/templates/sections/05-proposal-quality.md");
const TPL_06_VERIFICATION: &str =
    include_str!("../../../resources/setup/templates/sections/06-completion-verification.md");
const CONFIG_DEFAULT: &str = include_str!("../../../resources/setup/templates/config-default.yaml");
const INSTRUCTIONS_DEFAULT: &str =
    include_str!("../../../resources/setup/templates/instructions-default.md");

pub fn load_answers(input: Option<&str>) -> Result<SetupAnswers, String> {
    let Some(input) = input else {
        return Ok(SetupAnswers::default());
    };
    let json = if input == "-" {
        let mut json = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut json)
            .map_err(|e| format!("setup answers の標準入力読み取りに失敗: {e}"))?;
        json
    } else if let Some(path) = input.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|e| format!("setup answers ファイルの読み取りに失敗 ({path}): {e}"))?
    } else {
        input.to_string()
    };
    SetupAnswers::from_json(&json)
}

// --- パスユーティリティ ---

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn setup_dir() -> Result<PathBuf, String> {
    home_dir()
        .map(|h| h.join("Library/Application Support/tako/setup"))
        .ok_or_else(|| "ホームディレクトリが取得できない（$HOME 未設定）".into())
}

fn codex_home_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")))
}

fn instruction_path(agent: SetupAgent) -> Option<PathBuf> {
    let home = home_dir()?;
    Some(match agent {
        SetupAgent::Claude => home.join(".claude/CLAUDE.md"),
        SetupAgent::Codex => codex_home_dir()?.join("AGENTS.md"),
        SetupAgent::Agy => home.join(".gemini/GEMINI.md"),
    })
}

fn display_home_relative(path: &Path) -> String {
    home_dir()
        .and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|| path.display().to_string())
}

// --- 環境チェック ---

fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// ログインシェル経由でコマンドを探す（GUI 起動や Homebrew の PATH 差異に対応）
fn find_command(name: &str) -> Option<String> {
    let output = std::process::Command::new(login_shell())
        .args(["-l", "-c", &format!("command -v {name}")])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

/// setup を進行できるエージェント CLI。agy はオーケストレーターでは worker 専用だが、
/// `--review` の対話エージェントとしては利用できる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupAgent {
    Claude,
    Codex,
    Agy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Gpt,
    Google,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gpt => "gpt",
            Self::Google => "google",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Gpt => "GPT / ChatGPT",
            Self::Google => "Google",
        }
    }
}

impl SetupAgent {
    const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Agy];

    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" => Some(Self::Agy),
            _ => None,
        }
    }

    fn provider(self) -> Provider {
        match self {
            Self::Claude => Provider::Claude,
            Self::Codex => Provider::Gpt,
            Self::Agy => Provider::Google,
        }
    }

    fn supports_master(self) -> bool {
        !matches!(self, Self::Agy)
    }

    fn install_hint(self) -> &'static str {
        match self {
            Self::Claude => "https://docs.anthropic.com/en/docs/claude-code",
            Self::Codex => "https://developers.openai.com/codex/cli",
            Self::Agy => "agy install",
        }
    }
}

#[derive(Debug, Clone)]
struct DetectedAgent {
    kind: SetupAgent,
    path: String,
    authenticated: bool,
    /// 正規化済みプラン名。個人識別子や token は保持しない。
    plan: Option<String>,
}

fn command_output(path: &str, args: &[&str]) -> Option<std::process::Output> {
    std::process::Command::new(path).args(args).output().ok()
}

fn detect_agents() -> Vec<DetectedAgent> {
    SetupAgent::ALL
        .into_iter()
        .filter_map(|kind| {
            let path = find_command(kind.as_str())?;
            let (authenticated, plan) = match kind {
                SetupAgent::Claude => detect_claude_auth(&path),
                SetupAgent::Codex => detect_codex_auth(&path),
                SetupAgent::Agy => detect_agy_auth(&path),
            };
            Some(DetectedAgent {
                kind,
                path,
                authenticated,
                plan,
            })
        })
        .collect()
}

fn detect_claude_auth(path: &str) -> (bool, Option<String>) {
    let Some(output) = command_output(path, &["auth", "status", "--json"]) else {
        return (false, None);
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return (false, None);
    };
    parse_claude_auth_json(&value, output.status.success())
}

fn parse_claude_auth_json(
    value: &serde_json::Value,
    command_succeeded: bool,
) -> (bool, Option<String>) {
    let authenticated = command_succeeded
        && value
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    if !authenticated {
        return (false, None);
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
    (true, plan)
}

fn detect_codex_auth(path: &str) -> (bool, Option<String>) {
    let authenticated =
        command_output(path, &["login", "status"]).is_some_and(|output| output.status.success());
    let plan = authenticated
        .then(codex_plan_from_auth_file)
        .flatten()
        .map(|p| normalize_plan(&p));
    (authenticated, plan)
}

fn detect_agy_auth(path: &str) -> (bool, Option<String>) {
    let authenticated =
        command_output(path, &["models"]).is_some_and(|output| output.status.success());
    // agy 1.1.1 は models で認証判定できるが、プラン / quota は返さない。
    (authenticated, None)
}

fn normalize_plan(plan: &str) -> String {
    plan.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

/// Codex の OAuth JWT payload に含まれる ChatGPT plan claim をローカルで読む。
/// token 自体・account ID・メールアドレスは戻り値にもログにも出さない。
fn codex_plan_from_auth_file() -> Option<String> {
    let path = codex_home_dir()?.join("auth.json");
    codex_plan_from_auth_file_at(&path)
}

fn codex_plan_from_auth_file_at(path: &Path) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
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

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
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

// --- 依存ツールチェック ---

/// tako が実行時に使う外部コマンドの定義
struct ExternalDep {
    /// コマンド名
    bin: &'static str,
    /// 必須依存か（false = 任意。無くても tako 自体は動く）
    required: bool,
    /// 影響する機能の説明
    purpose: &'static str,
    /// brew でインストールする場合のパッケージ名（None = brew 非対応）
    brew_pkg: Option<&'static str>,
    /// brew 以外の導入案内
    install_hint: &'static str,
}

const EXTERNAL_DEPS: &[ExternalDep] = &[
    ExternalDep {
        bin: "tmux",
        required: false,
        purpose: "リモート接続（tako remote）・再起動時のセッション完全復元・オーケストレーターの worker 管理",
        brew_pkg: Some("tmux"),
        install_hint: "https://github.com/tmux/tmux/wiki/Installing",
    },
    ExternalDep {
        bin: "git",
        required: false,
        purpose: "git パネル（ブランチ・コミットグラフ・diff 表示）",
        brew_pkg: Some("git"),
        install_hint: "xcode-select --install でも導入できます",
    },
    ExternalDep {
        bin: "tailscale",
        required: false,
        purpose: "スマホからのリモート接続（tako remote。WireGuard E2E 暗号化）",
        brew_pkg: Some("tailscale"),
        install_hint: "App Store で「Tailscale」を検索、または brew install tailscale",
    },
];

/// 依存ツールのチェック段階。検出結果を `[OK]` / `[任意]` / `[不足]` で表示し、
/// interactive = true なら未導入の依存をその場で brew インストールできる。
/// 戻り値は検出したエージェントと、チェック後も欠けている必須依存の一覧。
fn run_dependency_check(interactive: bool) -> (Vec<DetectedAgent>, Vec<String>) {
    let agents = detect_agents();
    eprintln!("  エージェント CLI:");
    for agent in &agents {
        let auth = if agent.authenticated {
            "認証済み"
        } else {
            "未認証"
        };
        let plan = agent.plan.as_deref().unwrap_or("プラン不明");
        eprintln!(
            "    [検出] {}: {}（{auth} / {plan}）",
            agent.kind.as_str(),
            display_home_relative(Path::new(&agent.path))
        );
    }
    let brew = find_command("brew");
    let mut missing_required = if agents.is_empty() {
        eprintln!("    [不足] claude / codex / agy のいずれも見つかりません");
        for kind in SetupAgent::ALL {
            eprintln!("      {}: {}", kind.as_str(), kind.install_hint());
        }
        vec!["エージェント CLI（claude / codex / agy のいずれか）".to_string()]
    } else {
        Vec::new()
    };
    for dep in EXTERNAL_DEPS {
        if let Some(path) = find_command(dep.bin) {
            eprintln!("  [OK] {}: {path}", dep.bin);
            continue;
        }
        let (mark, kind) = if dep.required {
            ("[不足]", "必須")
        } else {
            ("[任意]", "任意")
        };
        eprintln!("  {mark} {}: 見つかりません（{kind}）", dep.bin);
        eprintln!("      用途: {}", dep.purpose);
        if !dep.required {
            eprintln!("      無くても tako 自体は動きますが、上記の機能が使えません");
        }
        let mut installed = false;
        match (dep.brew_pkg, brew.as_deref()) {
            (Some(pkg), Some(brew_bin)) => {
                eprintln!("      導入方法: brew install {pkg}");
                if interactive {
                    installed = offer_brew_install(pkg, brew_bin);
                }
            }
            (Some(pkg), None) => {
                eprintln!(
                    "      導入方法: brew install {pkg}（要 Homebrew）/ {}",
                    dep.install_hint
                );
            }
            (None, _) => {
                eprintln!("      導入方法: {}", dep.install_hint);
            }
        }
        if installed {
            match find_command(dep.bin) {
                Some(path) => eprintln!("  [OK] {}: {path}（インストール完了）", dep.bin),
                None => {
                    eprintln!(
                        "  [警告] {}: インストール後も検出できません。シェルを開き直してから再実行してください",
                        dep.bin
                    );
                    if dep.required {
                        missing_required.push(dep.bin.to_string());
                    }
                }
            }
        } else if dep.required {
            missing_required.push(dep.bin.to_string());
        }
    }
    // FDA チェック（macOS のみ。任意だが強く推奨）
    #[cfg(target_os = "macos")]
    {
        run_fda_check(interactive);
    }
    // スリープ防止の設定案内
    run_sleep_guard_check(interactive);
    (agents, missing_required)
}

/// 未導入の依存をその場で brew インストールするか確認して実行する。
/// インストールが成功したら true
fn offer_brew_install(pkg: &str, brew_bin: &str) -> bool {
    eprint!("      今すぐ brew install {pkg} を実行しますか？ [y/N]: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let answer = input.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("      スキップしました（後から brew install {pkg} で導入できます）");
        return false;
    }
    let status = std::process::Command::new(brew_bin)
        .args(["install", pkg])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();
    match status {
        Ok(s) if s.success() => true,
        _ => {
            eprintln!("      [警告] brew install {pkg} が失敗しました。手動で導入してください");
            false
        }
    }
}

/// スリープ防止（Issue #173）の設定案内。
/// L0〜L3 の段階式で、ユーザーの利用スタイルに合わせたスリープ防止を設定する
fn run_sleep_guard_check(interactive: bool) {
    let settings = tako_control::settings::load();
    let mode = settings.sleep_guard_mode;
    let power = settings.sleep_guard_power;
    eprintln!();
    eprintln!(
        "  スリープ防止: mode={}, power={}",
        mode.as_str(),
        power.as_str()
    );
    if !interactive {
        eprintln!("      設定変更: tako sleep-guard set --mode <mode> --power <condition>");
        return;
    }
    eprintln!("      エージェントが長時間動いている間に PC がスリープすると作業が止まります。");
    eprintln!("      スリープ防止の稼働レベルを選んでください:");
    eprintln!();
    eprintln!("      [0] OS 任せ（機能オフ）");
    eprintln!("      [1] AC 接続時のみアイドルスリープ防止（推奨）");
    eprintln!("      [2] バッテリー時もアイドルスリープ防止（電池消耗に注意）");
    eprintln!("      [3] 蓋閉じでも稼働（案内のみ — 手動設定が必要）");
    eprintln!();
    let current_level = match (mode, power) {
        (tako_control::sleep_guard::SleepGuardMode::Off, _) => 0,
        (_, tako_control::sleep_guard::PowerCondition::AcOnly) => 1,
        (_, tako_control::sleep_guard::PowerCondition::Always) => 2,
    };
    eprint!("      レベルを選択 [0-3]（現在: L{current_level}、Enter でスキップ）: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return;
    }
    let choice = input.trim();
    if choice.is_empty() {
        eprintln!("      現在の設定を維持します");
        return;
    }
    let mut new_settings = settings;
    match choice {
        "0" => {
            new_settings.sleep_guard_mode = tako_control::sleep_guard::SleepGuardMode::Off;
            eprintln!("      [OK] L0: スリープ防止を無効にしました（OS 任せ）");
        }
        "1" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::AcOnly;
            eprintln!("      [OK] L1: AC 接続時のみ、エージェント稼働中にスリープを防止します");
        }
        "2" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::Always;
            eprintln!("      [OK] L2: バッテリー時もエージェント稼働中にスリープを防止します");
            eprintln!("      [警告] 電池消耗が速くなります。AC 接続での利用を推奨します");
        }
        "3" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::AcOnly;
            eprintln!("      [OK] L3: L1 の設定を適用しました（AC 接続時のみ防止）");
            eprintln!();
            eprintln!("      蓋閉じでの継続稼働:");
            eprintln!("      ─────────────────────────────────────────────");
            eprintln!("      tako sleep-guard install-lid-sleep");
            eprintln!("        初回のみ管理者パスワードが必要。以後 tako が");
            eprintln!("        エージェント稼働中だけ自動で蓋閉じ継続を有効にします。");
            eprintln!("        解除: tako sleep-guard remove-lid-sleep");
            eprintln!("      ─────────────────────────────────────────────");
        }
        other => {
            eprintln!("      不明な選択: {other}。現在の設定を維持します");
            return;
        }
    }
    if let Err(e) = tako_control::settings::save(&new_settings) {
        eprintln!("      [警告] 設定の保存に失敗: {e}");
    }
}

/// FDA（フルディスクアクセス）の案内ステップ。
/// macOS の TCC（Transparency, Consent, and Control）による「ほかのアプリからの
/// データへのアクセス権を求められています」ダイアログを一括で消す方法を案内する。
#[cfg(target_os = "macos")]
fn run_fda_check(interactive: bool) {
    if tako_control::fda::is_granted() {
        eprintln!("  [OK] フルディスクアクセス: 付与済み（許可ダイアログは表示されません）");
        return;
    }
    eprintln!("  [任意] フルディスクアクセス: 未付与（推奨）");
    eprintln!("      macOS が「tako.app から、ほかのアプリからのデータへのアクセス権を");
    eprintln!("      求められています」と頻繁に表示する原因です。フルディスクアクセスを");
    eprintln!("      付与すると、このダイアログが出なくなります。");
    eprintln!(
        "      設定方法: システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を追加"
    );
    if !interactive {
        eprintln!("      付与方法: tako fda open でシステム設定を開き、tako を追加してください");
        return;
    }
    eprint!("      今すぐシステム設定を開きますか？ [y/N]: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return;
    }
    let answer = input.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("      スキップしました（後から tako fda open で設定画面を開けます）");
        return;
    }
    if let Err(e) = tako_control::fda::open_settings() {
        eprintln!("      [警告] {e}");
        return;
    }
    eprintln!(
        "      システム設定を開きました。tako を「フルディスクアクセス」に追加してください。"
    );
    eprintln!("      [警告] 付与後、tako アプリの再起動が必要です（⌘Q で終了 → 再度起動）。");
    eprintln!("        再起動するまで許可ダイアログが表示され続けることがあります。");

    // 再チェック（FDA は再起動後に有効になるため通常ここでは検出できないが、
    // 過去に付与済みで検出が遅延していた場合は拾える）
    eprintln!();
    eprint!("      設定しましたか？ 確認します... ");
    // 設定画面での操作を待つ猶予
    std::thread::sleep(std::time::Duration::from_secs(2));
    if tako_control::fda::is_granted() {
        eprintln!("[OK] 付与を確認しました。tako を再起動すると反映されます。");
    } else {
        eprintln!("まだ検出できません。");
        eprintln!("        付与後に tako を再起動すれば反映されます。今は先に進みます。");
    }
}

/// MCP 登録の健全性を確認。
/// 返り値: (登録あり, 登録パスが生きている)
fn check_claude_mcp_health(claude_path: &str) -> (bool, bool) {
    let output = std::process::Command::new(claude_path)
        .args(["mcp", "list"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let has_tako = stdout.lines().any(|line| {
                let lower = line.to_lowercase();
                lower.contains("tako") && !lower.contains("no mcp")
            });
            if !has_tako {
                return (false, false);
            }
            // settings.json から登録パスを直接読む（claude mcp list の出力は
            // ✔/✘ の有無やフォーマットがバージョンで変わり得るため）
            let path_alive = read_mcp_command_path()
                .map(|p| std::path::Path::new(&p).is_file())
                .unwrap_or(true); // 読めなければ楽観判定
            (true, path_alive)
        }
        _ => (false, false),
    }
}

/// settings.json から tako MCP 登録の command パスを読み取る
fn read_mcp_command_path() -> Option<String> {
    let home = home_dir()?;
    let path = home.join(".claude").join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&content).ok()?;
    settings
        .get("mcpServers")?
        .get("tako")?
        .get("command")?
        .as_str()
        .map(String::from)
}

fn run_setup_mcp() -> Result<(), String> {
    let tako_bin = tako_control::dispatch::resolve_tako_binary();
    let settings_dir = home_dir()
        .ok_or("ホームディレクトリが取得できない")?
        .join(".claude");
    let settings_path = settings_dir.join("settings.json");
    match tako_control::dispatch::setup_mcp_settings(&tako_bin, &settings_path) {
        Ok(result) => {
            if result.repaired {
                let old = result.old_command.as_deref().unwrap_or("(不明)");
                eprintln!("  [修復] MCP: 登録パスが消失していたため付け替えました");
                eprintln!("         旧: {old}");
                eprintln!("         新: {tako_bin}");
            } else if result.already_existed {
                eprintln!("  MCP: 既に設定されています");
            } else {
                eprintln!("  MCP: 設定を追加しました");
            }
            Ok(())
        }
        Err(e) => Err(format!("MCP 設定の追加に失敗: {e}")),
    }
}

fn configure_agent_mcp(agent: &DetectedAgent) -> Result<(), String> {
    match agent.kind {
        SetupAgent::Claude => {
            let (registered, healthy) = check_claude_mcp_health(&agent.path);
            if registered && healthy {
                eprintln!("  [OK] Claude MCP: tako が登録済み");
                Ok(())
            } else if registered && !healthy {
                eprintln!("  [警告] Claude MCP: 登録パスが消失しています。修復します");
                run_setup_mcp()
            } else {
                eprintln!("  [設定] Claude MCP を自動登録します");
                run_setup_mcp()
            }
        }
        SetupAgent::Codex => {
            eprintln!("  [OK] Codex MCP: tako master 起動時に一時設定を注入します");
            Ok(())
        }
        SetupAgent::Agy => {
            eprintln!("  [情報] agy は worker 専用のため MCP 登録は不要です");
            Ok(())
        }
    }
}

// --- リソース書き出し ---

fn write_resource(dir: &Path, rel_path: &str, content: &str) -> Result<(), String> {
    let path = dir.join(rel_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("ディレクトリの作成に失敗 ({}): {e}", parent.display()))?;
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("ファイルの書き出しに失敗 ({}): {e}", path.display()))
}

fn write_all_resources(setup_dir: &Path) -> Result<(), String> {
    write_resource(
        setup_dir,
        "templates/sections/00-language.md",
        TPL_00_LANGUAGE,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/01-interaction-style.md",
        TPL_01_INTERACTION,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/02-git-workflow.md",
        TPL_02_GIT,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/03-code-quality.md",
        TPL_03_CODE,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/04-safety-rules.md",
        TPL_04_SAFETY,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/05-proposal-quality.md",
        TPL_05_PROPOSAL,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/06-completion-verification.md",
        TPL_06_VERIFICATION,
    )?;
    write_resource(setup_dir, "templates/config-default.yaml", CONFIG_DEFAULT)?;
    write_resource(
        setup_dir,
        "templates/instructions-default.md",
        INSTRUCTIONS_DEFAULT,
    )?;
    // setup changelog の全履歴（setup エージェントが Read できるように毎回最新を展開）
    write_resource(setup_dir, "changes.yaml", CHANGES_YAML)?;
    Ok(())
}

// --- アップデート追従（Issue #94） ---

/// pending-changes.md のパス（setup ディレクトリ直下。setup エージェントが Read する）
fn pending_changes_path(setup_dir: &Path) -> PathBuf {
    setup_dir.join("pending-changes.md")
}

/// 未適用の変更一覧を CLI に表示する
fn print_pending_changes(pending: &[SetupChange], applied_revision: u32) {
    eprintln!(
        "  [情報] 前回のセットアップ（rev {applied_revision}）以降、アップデートで setup に {} 件の変更が入っています:",
        pending.len()
    );
    for change in pending {
        let kind = match change.kind {
            ChangeKind::Auto => "自動適用",
            ChangeKind::Guided => "--review で個別確認",
        };
        eprintln!(
            "      [rev {} / v{} / {kind}] {}",
            change.revision, change.version, change.title
        );
    }
}

/// 未適用の変更に応じて pending-changes.md を書き出す / 追従不要なら消す（stale 防止）
fn sync_pending_changes_file(
    setup_dir: &Path,
    pending: &[SetupChange],
    applied_revision: u32,
) -> Result<(), String> {
    let path = pending_changes_path(setup_dir);
    if pending.is_empty() {
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("pending-changes.md の削除に失敗: {e}"))?;
        }
        return Ok(());
    }
    let md = tako_control::setup::render_pending_markdown(pending, applied_revision);
    std::fs::write(&path, md).map_err(|e| format!("pending-changes.md の書き出しに失敗: {e}"))
}

fn select_setup_agent(
    agents: &[DetectedAgent],
    previous: Option<&str>,
    reuse_previous: bool,
    assume_yes: bool,
) -> Result<(SetupAgent, SetupValueSource), String> {
    match agents {
        [] => Err("エージェント CLI が見つかりません".into()),
        [only] => {
            if let Some(previous) = previous.filter(|value| *value != only.kind.as_str()) {
                eprintln!(
                    "  [detected] setup agent: {}（previous: {previous} は利用不可。検出値を優先）",
                    only.kind.as_str()
                );
                return Ok((only.kind, SetupValueSource::Detected));
            }
            let state = if only.authenticated {
                "認証済み CLI は 1 つ"
            } else {
                "検出された CLI は 1 つ"
            };
            eprintln!(
                "  [detected] setup agent: {}（{state}）",
                only.kind.as_str()
            );
            Ok((only.kind, SetupValueSource::Detected))
        }
        _ => {
            if reuse_previous {
                if let Some(previous_kind) = previous.and_then(SetupAgent::parse) {
                    if agents
                        .iter()
                        .any(|agent| agent.kind == previous_kind && agent.authenticated)
                    {
                        eprintln!("  [previous] setup agent: {}", previous_kind.as_str());
                        return Ok((previous_kind, SetupValueSource::Previous));
                    }
                    eprintln!(
                        "  [情報] previous setup agent `{}` は現在利用できないため、再選択します",
                        previous_kind.as_str()
                    );
                }
            }
            if assume_yes {
                let index = default_agent_index(agents);
                let selected = agents[index - 1].kind;
                eprintln!("  [default] setup agent: {}", selected.as_str());
                return Ok((selected, SetupValueSource::Default));
            }
            eprintln!();
            eprintln!("セットアップを進めるエージェントを選択してください:");
            for (index, agent) in agents.iter().enumerate() {
                let auth = if agent.authenticated {
                    "認証済み"
                } else {
                    "未認証"
                };
                eprintln!("  {}) {}（{auth}）", index + 1, agent.kind.as_str());
            }
            let default_index = default_agent_index(agents);
            eprint!("選択 [{default_index}]: ");
            let mut input = String::new();
            let _ = std::io::stdin().read_line(&mut input);
            let source = if input.trim().is_empty() {
                SetupValueSource::Default
            } else {
                SetupValueSource::Input
            };
            choose_setup_agent(agents, input.trim()).map(|agent| (agent, source))
        }
    }
}

fn default_agent_index(agents: &[DetectedAgent]) -> usize {
    agents
        .iter()
        .position(|agent| agent.authenticated)
        .unwrap_or(0)
        + 1
}

fn choose_setup_agent(agents: &[DetectedAgent], input: &str) -> Result<SetupAgent, String> {
    let selected = if input.is_empty() {
        default_agent_index(agents)
    } else {
        input
            .parse::<usize>()
            .map_err(|_| "選択は番号で入力してください".to_string())?
    };
    agents
        .get(selected.saturating_sub(1))
        .map(|agent| agent.kind)
        .ok_or_else(|| format!("選択範囲は 1〜{} です", agents.len()))
}

/// 認証済み CLI に対応するプロバイダと検出プランだけを返す。
/// 未導入・未認証のプロバイダを質問対象へ混ぜない（Issue #262 方針 A）。
fn detected_provider_plans(agents: &[DetectedAgent]) -> Vec<(Provider, Option<String>)> {
    agents
        .iter()
        .filter(|agent| agent.authenticated)
        .map(|agent| (agent.kind.provider(), agent.plan.clone()))
        .collect()
}

fn collect_provider_plans(
    agents: &[DetectedAgent],
    previous: &BTreeMap<String, String>,
    reuse_previous: bool,
    assume_yes: bool,
) -> BTreeMap<String, ResolvedSetupValue> {
    let mut plans = if reuse_previous {
        previous
            .iter()
            .map(|(provider, plan)| {
                (
                    provider.clone(),
                    resolve_setup_value(None, Some(plan), None)
                        .expect("previous があれば必ず解決できる"),
                )
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    for (provider, detected) in detected_provider_plans(agents) {
        let previous_plan = previous.get(provider.as_str()).map(String::as_str);
        let resolved = match detected.as_deref() {
            // Claude の status は max の倍率を返さない。前回倍率がなければ安全な max
            // （固定モデルを選ばない）へ丸め、--review 時だけ詳細を聞く。
            Some("max") if provider == Provider::Claude => {
                if reuse_previous
                    && previous_plan
                        .is_some_and(|plan| matches!(plan, "max" | "max-5x" | "max-20x"))
                {
                    let plan = previous_plan.unwrap_or("max");
                    eprintln!(
                        "  [previous] {} プラン: {plan}（detected: max）",
                        provider.label()
                    );
                    resolve_setup_value(None, Some(plan), None)
                        .expect("previous があれば必ず解決できる")
                } else {
                    if let Some(previous_plan) = previous_plan.filter(|_| reuse_previous) {
                        eprintln!(
                            "  [detected] {} プラン: max（previous: {previous_plan}。検出値を優先）",
                            provider.label()
                        );
                    }
                    prompt_plan(provider, Some("max"), assume_yes)
                }
            }
            Some(plan) => {
                let resolved =
                    resolve_setup_value(Some(plan), previous_plan.filter(|_| reuse_previous), None)
                        .expect("detected があれば必ず解決できる");
                if let Some(previous) = resolved.previous.as_deref() {
                    eprintln!(
                        "  [{}] {} プラン: {}（previous: {previous}。検出値を優先）",
                        resolved.source.label(),
                        provider.label(),
                        resolved.value
                    );
                } else {
                    eprintln!(
                        "  [{}] {} プラン: {}",
                        resolved.source.label(),
                        provider.label(),
                        resolved.value
                    );
                }
                resolved
            }
            None if reuse_previous && previous_plan.is_some() => {
                let resolved = resolve_setup_value(None, previous_plan, None)
                    .expect("previous があれば必ず解決できる");
                eprintln!(
                    "  [{}] {} プラン: {}",
                    resolved.source.label(),
                    provider.label(),
                    resolved.value
                );
                resolved
            }
            None => prompt_plan(provider, None, assume_yes),
        };
        plans.insert(provider.as_str().to_string(), resolved);
    }
    plans
}

fn prompt_plan(provider: Provider, detected: Option<&str>, assume_yes: bool) -> ResolvedSetupValue {
    if assume_yes {
        let resolved = if provider == Provider::Claude && detected == Some("max") {
            eprintln!(
                "  [detected] {} プラン: max（倍率は未検出のため [default] 未指定）",
                provider.label()
            );
            return ResolvedSetupValue {
                value: "max".to_string(),
                source: SetupValueSource::Detected,
                previous: None,
            };
        } else {
            resolve_setup_value(None, None, Some("unknown"))
                .expect("default があれば必ず解決できる")
        };
        eprintln!(
            "  [{}] {} プラン: {}",
            resolved.source.label(),
            provider.label(),
            resolved.value
        );
        return resolved;
    }

    eprintln!();
    let (value, source) = match provider {
        Provider::Claude if detected == Some("max") => {
            eprintln!("Claude Max を検出しました。契約倍率を選んでください:");
            eprintln!("  1) Max 5x");
            eprintln!("  2) Max 20x");
            eprintln!("  3) 不明");
            eprint!("選択 [3]: ");
            let (choice, source) = read_choice("3");
            let value = match choice.as_str() {
                "1" => "max-5x".into(),
                "2" => "max-20x".into(),
                _ => "max".into(),
            };
            (value, source)
        }
        Provider::Claude => {
            eprintln!("Claude のプランを選んでください:");
            eprintln!("  1) Free / 未契約  2) Pro  3) Max 5x  4) Max 20x");
            eprintln!("  5) Team / Enterprise  6) API  7) 不明");
            eprint!("選択 [7]: ");
            let (choice, source) = read_choice("7");
            let value = match choice.as_str() {
                "1" => "free",
                "2" => "pro",
                "3" => "max-5x",
                "4" => "max-20x",
                "5" => "team-enterprise",
                "6" => "api",
                _ => "unknown",
            }
            .into();
            (value, source)
        }
        Provider::Gpt => {
            eprintln!("GPT / ChatGPT のプランを選んでください:");
            eprintln!("  1) Free / 未契約  2) Plus  3) Pro");
            eprintln!("  4) Business / Enterprise  5) API  6) 不明");
            eprint!("選択 [6]: ");
            let (choice, source) = read_choice("6");
            let value = match choice.as_str() {
                "1" => "free",
                "2" => "plus",
                "3" => "pro",
                "4" => "business-enterprise",
                "5" => "api",
                _ => "unknown",
            }
            .into();
            (value, source)
        }
        Provider::Google => {
            eprintln!("Google のプランを選んでください（agy からは自動取得できません）:");
            eprintln!("  1) Free / 未契約  2) Google AI Pro  3) Google AI Ultra");
            eprintln!("  4) Workspace / Enterprise  5) 不明");
            eprint!("選択 [5]: ");
            let (choice, source) = read_choice("5");
            let value = match choice.as_str() {
                "1" => "free",
                "2" => "google-ai-pro",
                "3" => "google-ai-ultra",
                "4" => "workspace-enterprise",
                _ => "unknown",
            }
            .into();
            (value, source)
        }
    };
    eprintln!(
        "  [{}] {} プラン: {value}",
        source.label(),
        provider.label()
    );
    ResolvedSetupValue {
        value,
        source,
        previous: None,
    }
}

fn read_choice(default: &str) -> (String, SetupValueSource) {
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let trimmed = input.trim();
    if matches!(trimmed, "1" | "2" | "3" | "4" | "5" | "6" | "7") {
        (trimmed.to_string(), SetupValueSource::Input)
    } else {
        (default.to_string(), SetupValueSource::Default)
    }
}

fn plain_provider_plans(plans: &BTreeMap<String, ResolvedSetupValue>) -> BTreeMap<String, String> {
    plans
        .iter()
        .map(|(provider, value)| (provider.clone(), value.value.clone()))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PlanScale {
    Limited,
    Standard,
    High,
}

fn plan_scale(provider: Provider, plan: Option<&str>) -> PlanScale {
    let plan = plan.unwrap_or("unknown");
    match provider {
        Provider::Claude => match plan {
            "max-20x" | "team-enterprise" | "enterprise" | "api" => PlanScale::High,
            "pro" | "max" | "max-5x" | "team" => PlanScale::Standard,
            _ => PlanScale::Limited,
        },
        Provider::Gpt => match plan {
            "pro" | "business-enterprise" | "business" | "enterprise" | "api" => PlanScale::High,
            "plus" | "team" => PlanScale::Standard,
            _ => PlanScale::Limited,
        },
        Provider::Google => match plan {
            "google-ai-ultra" | "workspace-enterprise" | "enterprise" => PlanScale::High,
            "google-ai-pro" | "workspace" => PlanScale::Standard,
            _ => PlanScale::Limited,
        },
    }
}

fn effort_for(agent: SetupAgent, scale: PlanScale) -> Option<&'static str> {
    match agent {
        SetupAgent::Claude => Some(match scale {
            PlanScale::Limited => "medium",
            PlanScale::Standard => "high",
            PlanScale::High => "max",
        }),
        SetupAgent::Codex => Some(match scale {
            PlanScale::Limited => "medium",
            PlanScale::Standard => "high",
            PlanScale::High => "xhigh",
        }),
        SetupAgent::Agy => None,
    }
}

fn recommended_profile(
    selected: SetupAgent,
    agents: &[DetectedAgent],
    plans: &BTreeMap<String, String>,
) -> (tako_control::orchestrator::Profile, String) {
    use tako_control::orchestrator::{AgentWorkerConfig, Profile, WorkerModelPolicy};

    let master = if selected.supports_master() {
        selected
    } else {
        agents
            .iter()
            .find(|a| a.kind == SetupAgent::Claude && a.authenticated)
            .or_else(|| {
                agents
                    .iter()
                    .find(|a| a.kind == SetupAgent::Codex && a.authenticated)
            })
            .or_else(|| agents.iter().find(|a| a.kind.supports_master()))
            .map(|a| a.kind)
            // agy 単独時は master 非対応であることを注記し、後方互換の claude 既定を残す。
            .unwrap_or(SetupAgent::Claude)
    };
    let master_provider = master.provider();
    let master_scale = plan_scale(
        master_provider,
        plans.get(master_provider.as_str()).map(String::as_str),
    );
    let mut profile = Profile {
        master_agent: Some(master.as_str().to_string()),
        model: None,
        effort: effort_for(master, master_scale)
            .unwrap_or("high")
            .to_string(),
        worker_agent: Some(selected.as_str().to_string()),
        ..Profile::default()
    };

    let usable_count = agents.iter().filter(|a| a.authenticated).count();
    if usable_count > 1 && master_scale >= PlanScale::Standard {
        profile.worker_model_policy = WorkerModelPolicy::Delegate;
        let names = agents
            .iter()
            .filter(|a| a.authenticated)
            .map(|a| a.kind.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        profile.delegate_guidance = Some(format!(
            "利用可能な {names} から、重い実装は高プラン側、軽い調査は低負荷側へ振り分ける。モデル未指定時は各 CLI の既定モデルを使う。"
        ));
    }

    for agent in agents.iter().filter(|a| a.authenticated) {
        let scale = plan_scale(
            agent.kind.provider(),
            plans
                .get(agent.kind.provider().as_str())
                .map(String::as_str),
        );
        profile.worker_agents.insert(
            agent.kind.as_str().to_string(),
            AgentWorkerConfig {
                model: None,
                effort: effort_for(agent.kind, scale).map(str::to_string),
                skip_permissions: matches!(agent.kind, SetupAgent::Codex | SetupAgent::Agy),
                args: Vec::new(),
            },
        );
    }

    let master_ready = agents
        .iter()
        .any(|agent| agent.authenticated && agent.kind.supports_master());
    let note = if selected == SetupAgent::Agy && !master_ready {
        "agy は worker 専用です。tako master を使う前に claude または codex を導入してログインしてください。"
            .to_string()
    } else if selected == SetupAgent::Agy {
        format!(
            "agy は worker 専用のため、master={} / worker=agy としました。",
            master.as_str()
        )
    } else {
        format!(
            "master / worker を {}、モデルは各 CLI の既定値としました。",
            selected.as_str()
        )
    };
    (profile, note)
}

fn prepare_profile(
    selected: SetupAgent,
    agents: &[DetectedAgent],
    plans: &BTreeMap<String, String>,
    provided: Option<&tako_control::orchestrator::Profile>,
) -> Result<&'static str, String> {
    use tako_control::orchestrator;

    let profile_path = orchestrator::profiles_dir()
        .ok_or("ホームディレクトリが取得できない")?
        .join("default.yaml");
    let existed = profile_path.is_file();
    orchestrator::ensure_defaults()?;
    if let Some(notice) = orchestrator::migrate_legacy_default_profile() {
        eprintln!("  [移行] {notice}");
    }
    if let Some(profile) = provided {
        profile.save("default")?;
        eprintln!(
            "  [input] profile を保存: {}",
            display_home_relative(&profile_path)
        );
        return Ok("answers で指定された default profile を適用。");
    }
    if existed {
        eprintln!("  [previous] 既存の default プロファイルを維持します");
        return Ok("既存の default profile を前回どおり維持。");
    }
    let (recommended, note) = recommended_profile(selected, agents, plans);
    recommended.save("default")?;
    eprintln!(
        "  [OK] 推奨プロファイルを保存: {}",
        display_home_relative(&profile_path)
    );
    eprintln!("       {note}");
    Ok("モデルは各 CLI の既定値。effort と worker ポリシーはプラン規模から推奨済み。")
}

#[derive(serde::Serialize)]
struct SetupContext<'a> {
    selected_agent: &'a str,
    instruction_file: String,
    installed_agents: Vec<&'a str>,
    authenticated_agents: Vec<&'a str>,
    provider_plans: &'a BTreeMap<String, String>,
    profile_note: &'a str,
}

fn write_setup_context(
    dir: &Path,
    selected: SetupAgent,
    agents: &[DetectedAgent],
    plans: &BTreeMap<String, String>,
    profile_note: &str,
) -> Result<(), String> {
    let instruction_file = instruction_path(selected)
        .map(|path| display_home_relative(&path))
        .unwrap_or_else(|| "(取得不能)".to_string());
    let context = SetupContext {
        selected_agent: selected.as_str(),
        instruction_file,
        installed_agents: agents.iter().map(|agent| agent.kind.as_str()).collect(),
        authenticated_agents: agents
            .iter()
            .filter(|agent| agent.authenticated)
            .map(|agent| agent.kind.as_str())
            .collect(),
        provider_plans: plans,
        profile_note,
    };
    let yaml = serde_yaml::to_string(&context)
        .map_err(|e| format!("setup-context.yaml の生成に失敗: {e}"))?;
    write_resource(dir, "setup-context.yaml", &yaml)
}

fn launch_setup_agent(
    agent: &DetectedAgent,
    dir: &Path,
    greeting: &str,
) -> Result<std::process::ExitStatus, String> {
    let mut command = std::process::Command::new(&agent.path);
    command.current_dir(dir);
    match agent.kind {
        SetupAgent::Claude | SetupAgent::Codex => {
            command.arg(greeting);
        }
        SetupAgent::Agy => {
            command.args(["--prompt-interactive", greeting]);
        }
    }
    command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("{} の起動に失敗: {e}", agent.kind.as_str()))
}

fn should_reuse_previous(config: &tako_control::setup::SetupConfig, review: bool) -> bool {
    config.setup.completed && !review
}

fn default_profile_path() -> Result<PathBuf, String> {
    tako_control::orchestrator::profiles_dir()
        .map(|dir| dir.join("default.yaml"))
        .ok_or_else(|| "ホームディレクトリが取得できない".to_string())
}

fn print_setup_summary(plan: &SetupPlan) {
    eprintln!();
    if plan.is_empty() {
        eprintln!("セットアップ結果: 変更なし（前回の設定は最新）");
    } else {
        eprintln!("セットアップ結果（変更したのはこれだけです）:");
        eprintln!("{}", plan.render_diff());
    }
}

/// 認証済みエージェントを対話で選んで返す。
/// エージェント 1 つなら Y/n、複数なら番号選択。選ばなければ None。
fn prompt_launch_agent(agents: &[DetectedAgent]) -> Option<DetectedAgent> {
    let launchable: Vec<_> = agents.iter().filter(|a| a.authenticated).collect();
    if launchable.is_empty() {
        return None;
    }
    eprintln!();
    if launchable.len() == 1 {
        let agent = launchable[0];
        eprint!(
            "続けて {} で対話を開始しますか？ [Y/n]: ",
            agent.kind.as_str()
        );
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let trimmed = input.trim().to_ascii_lowercase();
        if trimmed.is_empty() || trimmed == "y" || trimmed == "yes" {
            return Some(agent.clone());
        }
        return None;
    }
    eprintln!("続けてエージェントで対話を開始しますか？");
    for (i, agent) in launchable.iter().enumerate() {
        eprintln!("  {}) {}", i + 1, agent.kind.as_str());
    }
    eprintln!("  Enter) 起動しない");
    eprint!("選択 [Enter]: ");
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .parse::<usize>()
        .ok()
        .and_then(|n| launchable.get(n.wrapping_sub(1)))
        .map(|a| (*a).clone())
}

/// エージェント CLI をユーザーのホームディレクトリで対話起動する。
fn launch_agent_interactive(agent: &DetectedAgent) -> Result<std::process::ExitStatus, String> {
    let cwd = home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut command = std::process::Command::new(&agent.path);
    command.current_dir(&cwd);
    command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    command
        .status()
        .map_err(|e| format!("{} の起動に失敗: {e}", agent.kind.as_str()))
}

fn apply_instruction(agent: SetupAgent, provided: Option<&str>) -> Result<(), String> {
    let path = instruction_path(agent).ok_or("グローバル指示ファイルのパスを取得できません")?;
    if let Some(content) = provided {
        tako_control::config_io::atomic_write_with_backup(&path, content)?;
        eprintln!(
            "  [input] グローバル指示ファイルを保存: {}",
            display_home_relative(&path)
        );
        return Ok(());
    }
    if path.is_file() {
        eprintln!(
            "  [previous] グローバル指示ファイルを維持: {}",
            display_home_relative(&path)
        );
        return Ok(());
    }
    tako_control::config_io::atomic_write(&path, INSTRUCTIONS_DEFAULT)?;
    eprintln!(
        "  [default] グローバル指示ファイルを作成: {}",
        display_home_relative(&path)
    );
    Ok(())
}

fn apply_sleep_guard_answers(
    answers: Option<&tako_control::setup::SetupSleepGuardAnswers>,
) -> Result<(), String> {
    let Some(answers) = answers else {
        return Ok(());
    };
    let mut settings = tako_control::settings::load();
    if let Some(mode) = answers.mode.as_deref() {
        settings.sleep_guard_mode = tako_control::sleep_guard::SleepGuardMode::from_str_opt(mode)
            .ok_or_else(|| format!("不正な sleep_guard.mode: {mode}"))?;
    }
    if let Some(power) = answers.power.as_deref() {
        settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::from_str_opt(power)
            .ok_or_else(|| format!("不正な sleep_guard.power: {power}"))?;
    }
    tako_control::settings::save(&settings)
        .map_err(|e| format!("スリープ防止設定の保存に失敗: {e}"))?;
    eprintln!(
        "  [input] スリープ防止: mode={}, power={}",
        settings.sleep_guard_mode.as_str(),
        settings.sleep_guard_power.as_str()
    );
    Ok(())
}

fn apply_projects(
    projects: Option<&BTreeMap<String, tako_control::orchestrator::ProjectEntry>>,
) -> Result<(), String> {
    let Some(projects) = projects else {
        return Ok(());
    };
    let config = tako_control::orchestrator::ProjectsConfig {
        projects: projects.clone(),
    };
    config.save()?;
    eprintln!("  [input] プロジェクト登録: {} 件", projects.len());
    Ok(())
}

fn mark_setup_complete(
    selected: SetupAgent,
    plans: &BTreeMap<String, String>,
    orchestrator: Option<&tako_control::setup::SetupOrchestratorAnswers>,
) -> Result<u32, String> {
    let revision = tako_control::setup::current_revision()?;
    let current = load_config()?;
    if current.setup.completed
        && current.setup.applied_revision == revision
        && current.setup.applied_version.as_deref() == Some(env!("CARGO_PKG_VERSION"))
        && current.setup.selected_agent.as_deref() == Some(selected.as_str())
        && current.setup.provider_plans == *plans
        && orchestrator.is_none()
    {
        return Ok(revision);
    }
    tako_control::setup::mutate_config(|config| {
        config.setup.completed = true;
        config.setup.completed_at = Some(now_iso8601());
        config.setup.applied_revision = revision;
        config.setup.applied_version = Some(env!("CARGO_PKG_VERSION").to_string());
        config.setup.selected_agent = Some(selected.as_str().to_string());
        config.setup.provider_plans = plans.clone();
        if let Some(orchestrator) = orchestrator {
            if let Some(auto_close) = orchestrator.auto_close {
                config.orchestrator.auto_close = auto_close;
            }
            if let Some(auto_push) = orchestrator.auto_push {
                config.orchestrator.auto_push = auto_push;
            }
        }
    })?;
    Ok(revision)
}

// --- メインエントリ ---

/// `tako setup --check` — 環境チェックだけ実行して終了
pub fn run_check() -> Result<(), String> {
    eprintln!("tako セットアップ 環境チェック");
    eprintln!("─────────────────────────────");

    // エージェント CLI + 任意依存。--check では表示のみ。
    let (agents, _) = run_dependency_check(false);

    // MCP 登録（claude のみ永続登録。codex は master 起動時注入、agy は worker 専用）
    if let Some(claude) = agents.iter().find(|a| a.kind == SetupAgent::Claude) {
        let (registered, healthy) = check_claude_mcp_health(&claude.path);
        if registered && healthy {
            eprintln!("  [OK] Claude MCP: tako が登録済み");
        } else if registered && !healthy {
            eprintln!("  [警告] Claude MCP: 登録済みだがパスが消失しています");
            if let Some(cmd) = read_mcp_command_path() {
                eprintln!("         登録パス: {cmd}");
            }
            eprintln!("         tako setup または tako setup-mcp で修復できます");
        } else {
            eprintln!("  [不足] Claude MCP: tako が未登録（tako setup-mcp で登録できます）");
        }
    }
    if agents.iter().any(|a| a.kind == SetupAgent::Codex) {
        eprintln!("  [OK] Codex MCP: tako master 起動時に一時注入");
    }
    if agents.iter().any(|a| a.kind == SetupAgent::Agy) {
        eprintln!("  [情報] agy: worker 専用（master / MCP 接続は非対応）");
    }

    // config.yaml
    let config_path = tako_control::setup::config_yaml_path()?;
    if config_path.is_file() {
        let config = load_config()?;
        if config.setup.completed {
            eprintln!(
                "  [OK] セットアップ: 完了済み ({})",
                config.setup.completed_at.as_deref().unwrap_or("日時不明")
            );
            // アップデート追従状況（Issue #94）
            let pending = pending_changes(config.setup.applied_revision)?;
            if pending.is_empty() {
                eprintln!(
                    "  [OK] アップデート追従: 最新（rev {}）",
                    config.setup.applied_revision
                );
            } else {
                eprintln!(
                    "  [情報] アップデート追従: 未適用の setup 変更が {} 件（tako setup --changes で詳細）",
                    pending.len()
                );
            }
            if let Some(agent) = config.setup.selected_agent.as_deref() {
                eprintln!("  [OK] 既定エージェント: {agent}");
            }
            if !config.setup.provider_plans.is_empty() {
                let plans = config
                    .setup
                    .provider_plans
                    .iter()
                    .map(|(provider, plan)| format!("{provider}={plan}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!("  [OK] 申告・検出プラン: {plans}");
            }
        } else {
            eprintln!("  [情報] セットアップ: 未完了");
        }
    } else {
        eprintln!("  [情報] config.yaml: 未作成");
    }

    // 検出したエージェントのグローバル指示ファイル
    for agent in &agents {
        if let Some(path) = instruction_path(agent.kind) {
            if path.is_file() {
                eprintln!("  [OK] {}: 存在します", display_home_relative(&path));
            } else {
                eprintln!("  [情報] {}: 未作成", display_home_relative(&path));
            }
        }
    }

    // エージェント共通ルール同期（Issue #136）
    match tako_control::agents_sync::status() {
        Ok(status) => {
            let st = status["status"].as_str().unwrap_or("unknown");
            match st {
                "not_configured" => {
                    eprintln!("  [情報] エージェント共通ルール同期: 未設定");
                }
                "up_to_date" => {
                    eprintln!("  [OK] エージェント共通ルール同期: 最新");
                }
                "outdated" => {
                    eprintln!(
                        "  [情報] エージェント共通ルール同期: ずれあり（tako agents sync-rules で同期）"
                    );
                }
                "source_missing" => {
                    let path = status["source_path"].as_str().unwrap_or("?");
                    eprintln!("  [不足] エージェント共通ルール同期: 正本が見つからない ({path})");
                }
                _ => {
                    eprintln!("  ? エージェント共通ルール同期: {st}");
                }
            }
        }
        Err(e) => eprintln!("  [情報] エージェント共通ルール同期: 確認失敗 ({e})"),
    }

    // スリープ防止（Issue #173）
    {
        let settings = tako_control::settings::load();
        let mode = settings.sleep_guard_mode;
        let power = settings.sleep_guard_power;
        match mode {
            tako_control::sleep_guard::SleepGuardMode::Off => {
                eprintln!("  [情報] スリープ防止: 無効（tako sleep-guard set --mode while-agents-running で有効化）");
            }
            _ => {
                eprintln!(
                    "  [OK] スリープ防止: mode={}, power={}",
                    mode.as_str(),
                    power.as_str()
                );
            }
        }
        let lid_mode = settings.lid_sleep_mode;
        let sudoers = tako_control::sleep_guard::is_sudoers_installed();
        match lid_mode {
            tako_control::sleep_guard::LidSleepMode::Off => {
                eprintln!(
                    "  [情報] 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）"
                );
            }
            tako_control::sleep_guard::LidSleepMode::WhileAgentsRunning => {
                if sudoers {
                    eprintln!("  [OK] 蓋閉じ防止: while-agents-running（sudoers 登録済み）");
                } else {
                    eprintln!("  [不足] 蓋閉じ防止: while-agents-running だが sudoers 未登録（tako sleep-guard install-lid-sleep で登録）");
                }
            }
        }
    }

    // プロファイル一覧
    match tako_control::orchestrator::list_profiles() {
        Ok(profiles) if !profiles.is_empty() => {
            eprintln!(
                "  [OK] プロファイル: {} 個（{}）",
                profiles.len(),
                profiles.join(", ")
            );
        }
        Ok(_) => eprintln!("  [情報] プロファイル: 未作成（tako master で自動生成されます）"),
        Err(e) => eprintln!("  [情報] プロファイル: 確認失敗 ({e})"),
    }

    Ok(())
}

/// `tako setup --reset` — config.yaml の setup.completed を false にリセット
pub fn run_reset() -> Result<(), String> {
    // ロック付き read-modify-write（#169: 他フィールドの並行更新を巻き戻さない）
    tako_control::setup::mutate_config(|config| {
        config.setup.completed = false;
        config.setup.completed_at = None;
    })?;
    eprintln!("セットアップ状態をリセットしました。tako setup で再実行できます");
    Ok(())
}

/// `tako setup --changes` — アップデート追従状況の表示（Issue #94）。
/// MCP `tako_setup_changes` と同じ照会（`--json` で同一ペイロードを出力）
pub fn run_changes(json: bool) -> Result<(), String> {
    if json {
        let status = tako_control::setup::changes_status()?;
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(|e| format!("JSON 変換に失敗: {e}"))?
        );
        return Ok(());
    }
    let config = load_config()?;
    let current = tako_control::setup::current_revision()?;
    let applied = config.setup.applied_revision;
    eprintln!("tako setup アップデート追従状況");
    eprintln!("─────────────────────────────");
    eprintln!(
        "  現在の setup リビジョン: {current}（tako v{}）",
        env!("CARGO_PKG_VERSION")
    );
    if !config.setup.completed {
        eprintln!("  セットアップ: 未実施（tako setup を実行すると最新の設定で導入されます）");
        return Ok(());
    }
    match &config.setup.applied_version {
        Some(v) => eprintln!("  適用済みリビジョン: {applied}（tako v{v} で setup 実行）"),
        None => eprintln!("  適用済みリビジョン: {applied}"),
    }
    let pending = pending_changes(applied)?;
    if pending.is_empty() {
        eprintln!("  [OK] 最新です。追従が必要な変更はありません");
        return Ok(());
    }
    eprintln!("  未適用の変更: {} 件", pending.len());
    eprintln!();
    for change in &pending {
        let kind = match change.kind {
            ChangeKind::Auto => "auto（setup 再実行で自動適用）",
            ChangeKind::Guided => "guided（setup --review で個別確認・適用）",
        };
        eprintln!(
            "  [rev {} / v{} / {}] {}",
            change.revision, change.version, change.date, change.title
        );
        eprintln!("      区分: {kind}");
        for line in change.description.lines() {
            eprintln!("      {line}");
        }
        eprintln!();
    }
    eprintln!("  `tako setup` を実行すると追従できます");
    Ok(())
}

/// `tako setup` — メインのセットアップフロー。
/// 通常実行と `--yes` はどちらも積極的自動化で質問ゼロ。`--answers` は検出値より優先する。
pub fn run_setup(assume_yes: bool, review: bool, answers: &SetupAnswers) -> Result<(), String> {
    eprintln!("tako セットアップ");
    eprintln!("═════════════════");
    eprintln!();

    // 前回値をすべての質問より先に読む。破損時は既定値で上書きせず中断する。
    let config = load_config()?;
    let is_first_run = !config.setup.completed;
    let reuse_previous = should_reuse_previous(&config, review);
    let review_mode = review;
    if assume_yes {
        eprintln!("  [default] 非対話モードで既定値を適用します");
    }
    if reuse_previous {
        eprintln!("  [previous] 前回の設定を引き継ぎます");
        eprintln!();
    }

    // setup 中は項目別 y/n を出さない。未導入依存・FDA・スリープ設定は状態と
    // 専用コマンドだけを表示し、ユーザーが必要なときに個別操作できるようにする。
    let (agents, missing) = run_dependency_check(false);
    if !missing.is_empty() {
        return Err(format!(
            "必須の依存ツールが不足しています: {}。\n\
             導入後に tako setup を再実行してください",
            missing.join(", ")
        ));
    }
    let (selected, selected_source) = if let Some(answer) = answers.selected_agent.as_deref() {
        let selected =
            SetupAgent::parse(answer).ok_or_else(|| format!("不正な selected_agent: {answer}"))?;
        eprintln!("  [input] setup agent: {}", selected.as_str());
        (selected, SetupValueSource::Input)
    } else {
        select_setup_agent(
            &agents,
            config.setup.selected_agent.as_deref(),
            reuse_previous,
            !review,
        )?
    };
    let selected_agent = agents
        .iter()
        .find(|agent| agent.kind == selected)
        .ok_or("選択したエージェントの検出情報がありません")?;
    if !selected_agent.authenticated {
        return Err(format!(
            "{} は未認証です。先に {} を単独起動してログインしてから再実行してください",
            selected.as_str(),
            selected.as_str()
        ));
    }

    let mut resolved_plans = collect_provider_plans(
        &agents,
        &config.setup.provider_plans,
        reuse_previous,
        !review,
    );
    for (provider, value) in &answers.provider_plans {
        let detected = resolved_plans
            .get(provider)
            .map(|value| value.value.as_str());
        if detected.is_some_and(|detected| detected != value) {
            eprintln!(
                "  [input] {provider} プラン: {value}（detected/previous: {}。明示回答を優先）",
                detected.unwrap_or("unknown")
            );
        } else {
            eprintln!("  [input] {provider} プラン: {value}");
        }
        resolved_plans.insert(
            provider.clone(),
            ResolvedSetupValue {
                value: value.clone(),
                source: SetupValueSource::Input,
                previous: None,
            },
        );
    }
    let plans = plain_provider_plans(&resolved_plans);
    let current_revision = tako_control::setup::current_revision()?;
    let pending = if is_first_run {
        Vec::new()
    } else {
        pending_changes(config.setup.applied_revision)?
    };
    if !pending.is_empty() {
        eprintln!();
        print_pending_changes(&pending, config.setup.applied_revision);
        if review_mode {
            eprintln!("      個別見直しで setup agent が guided 項目を確認します");
        } else {
            eprintln!(
                "      [previous] guided 項目は既存カスタマイズを維持し、auto 項目と revision を追従します"
            );
        }
    }

    let instruction =
        instruction_path(selected).ok_or("グローバル指示ファイルのパスを取得できません")?;
    let instruction_existed = instruction.is_file();
    let profile_path = default_profile_path()?;
    let profile_existed = profile_path.is_file();
    let claude_mcp_missing = if selected == SetupAgent::Claude {
        let (registered, healthy) = check_claude_mcp_health(&selected_agent.path);
        !registered || !healthy
    } else {
        false
    };

    let mut plan = SetupPlan::default();
    plan.push_if_changed(
        "setup.completed",
        config.setup.completed.then_some("true"),
        "true",
        SetupValueSource::Default,
    );
    plan.push_if_changed(
        "setup.selected_agent",
        config.setup.selected_agent.as_deref(),
        selected.as_str(),
        selected_source,
    );
    for (provider, resolved) in &resolved_plans {
        plan.push_if_changed(
            format!("setup.provider_plans.{provider}"),
            config
                .setup
                .provider_plans
                .get(provider)
                .map(String::as_str),
            resolved.value.clone(),
            resolved.source,
        );
    }
    let applied_revision_before = config.setup.applied_revision.to_string();
    plan.push_if_changed(
        "setup.applied_revision",
        Some(&applied_revision_before),
        current_revision.to_string(),
        SetupValueSource::Default,
    );
    if answers.instruction_content.is_some() {
        plan.push_if_changed(
            display_home_relative(&instruction),
            instruction_existed.then_some("既存内容"),
            "answers の指示内容を適用",
            SetupValueSource::Input,
        );
    } else if !instruction_existed {
        plan.push_if_changed(
            display_home_relative(&instruction),
            None,
            "既定の開発ルールを作成",
            SetupValueSource::Default,
        );
    }
    if answers.profile.is_some() {
        plan.push_if_changed(
            "profiles/default.yaml",
            profile_existed.then_some("既存 profile"),
            "answers の profile を適用",
            SetupValueSource::Input,
        );
    } else if !profile_existed {
        plan.push_if_changed(
            "profiles/default.yaml",
            None,
            "検出プランにもとづく推奨 profile を作成",
            SetupValueSource::Default,
        );
    }
    if claude_mcp_missing {
        plan.push_if_changed(
            "Claude MCP",
            Some("未登録"),
            "tako を登録",
            SetupValueSource::Default,
        );
    }
    if let Some(sleep) = &answers.sleep_guard {
        let settings = tako_control::settings::load();
        if let Some(mode) = sleep.mode.as_deref() {
            plan.push_if_changed(
                "settings.sleep_guard_mode",
                Some(settings.sleep_guard_mode.as_str()),
                mode,
                SetupValueSource::Input,
            );
        }
        if let Some(power) = sleep.power.as_deref() {
            plan.push_if_changed(
                "settings.sleep_guard_power",
                Some(settings.sleep_guard_power.as_str()),
                power,
                SetupValueSource::Input,
            );
        }
    }
    if let Some(orchestrator) = &answers.orchestrator {
        if let Some(auto_close) = orchestrator.auto_close {
            plan.push_if_changed(
                "orchestrator.auto_close",
                Some(if config.orchestrator.auto_close {
                    "true"
                } else {
                    "false"
                }),
                auto_close.to_string(),
                SetupValueSource::Input,
            );
        }
        if let Some(auto_push) = orchestrator.auto_push {
            plan.push_if_changed(
                "orchestrator.auto_push",
                Some(if config.orchestrator.auto_push {
                    "true"
                } else {
                    "false"
                }),
                auto_push.to_string(),
                SetupValueSource::Input,
            );
        }
    }
    if answers.projects.is_some() {
        plan.push_if_changed(
            "projects.yaml",
            tako_control::orchestrator::projects_yaml_path()
                .is_some_and(|path| path.is_file())
                .then_some("既存一覧"),
            format!(
                "answers の {} 件を適用",
                answers.projects.as_ref().map_or(0, BTreeMap::len)
            ),
            SetupValueSource::Input,
        );
    }

    // 検出値・既定値だけの標準ケースは確認を挟まず適用する（Issue #262 要件 D）。
    configure_agent_mcp(selected_agent)?;
    apply_instruction(selected, answers.instruction_content.as_deref())?;
    apply_sleep_guard_answers(answers.sleep_guard.as_ref())?;

    let dir = setup_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    write_all_resources(&dir)?;
    eprintln!("  [OK] テンプレートを展開: {}", display_home_relative(&dir));
    for filename in [
        "setup-instructions.md",
        "CLAUDE.md",
        "AGENTS.md",
        "GEMINI.md",
    ] {
        write_resource(&dir, filename, SYSTEM_PROMPT)?;
    }
    sync_pending_changes_file(&dir, &pending, config.setup.applied_revision)?;

    let profile_note = prepare_profile(selected, &agents, &plans, answers.profile.as_ref())?;
    apply_projects(answers.projects.as_ref())?;
    write_setup_context(&dir, selected, &agents, &plans, profile_note)?;

    if review_mode {
        if instruction_existed {
            let parent = instruction.parent().unwrap_or(Path::new("."));
            let filename = instruction
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let backup = find_backup_path(parent, &filename);
            if let Err(e) = std::fs::copy(&instruction, &backup) {
                eprintln!("  [警告] {filename} のバックアップに失敗: {e}");
            } else {
                eprintln!(
                    "  [OK] {filename} をバックアップ: {}",
                    backup.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
        eprintln!();
        eprintln!("個別見直しを開始します。");
        eprintln!("─────────────────────────────────────────────────────");
        let greeting = if pending.is_empty() {
            "最初に setup-instructions.md を読んでください。前回設定の個別見直しを始めます。変更したい項目だけ確認してください。"
        } else {
            "最初に setup-instructions.md と pending-changes.md を読んでください。アップデート変更と前回設定の個別見直しを始めます。"
        };
        let status = launch_setup_agent(selected_agent, &dir, greeting)?;
        if !status.success() {
            eprintln!(
                "{} が終了しました（exit code: {}）",
                selected.as_str(),
                status.code().unwrap_or(-1)
            );
            return Ok(());
        }
    }

    let revision = mark_setup_complete(selected, &plans, answers.orchestrator.as_ref())?;
    sync_pending_changes_file(&dir, &[], revision)?;
    print_setup_summary(&plan);
    eprintln!("セットアップが完了しました。");
    eprintln!();
    eprintln!("スマホからリモート接続するには: tako remote setup");

    // --- 起動ランチャー（Issue #295）---
    // --answers で launch_agent を明示指定した場合はそれに従う。
    // --yes のみ / --answers で launch_agent 省略 / 非 TTY → スキップ
    let launch_result = if let Some(specified) = answers.launch_agent.as_deref() {
        if specified == "none" {
            None
        } else {
            SetupAgent::parse(specified).and_then(|kind| {
                agents
                    .iter()
                    .find(|a| a.kind == kind && a.authenticated)
                    .cloned()
            })
        }
    } else if assume_yes || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        None
    } else {
        prompt_launch_agent(&agents)
    };
    if let Some(agent) = launch_result {
        eprintln!();
        eprintln!("{} を起動します…", agent.kind.as_str());
        eprintln!("─────────────────────────────────────────────────────");
        let status = launch_agent_interactive(&agent)?;
        if !status.success() {
            eprintln!(
                "{} が終了しました（exit code: {}）",
                agent.kind.as_str(),
                status.code().unwrap_or(-1)
            );
        }
    }

    Ok(())
}

fn find_backup_path(dir: &Path, filename: &str) -> PathBuf {
    let today = {
        let output = std::process::Command::new("date")
            .args(["+%Y-%m-%d"])
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => "unknown".into(),
        }
    };
    let base = dir.join(format!("{filename}.backup-{today}"));
    if !base.exists() {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = dir.join(format!("{filename}.backup-{today}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn now_iso8601() -> String {
    let output = std::process::Command::new("date")
        .args(["+%Y-%m-%dT%H:%M:%S%z"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // +0900 → +09:00
            if s.len() >= 24 && !s.contains('+') {
                s
            } else if s.len() >= 24 {
                let (head, tail) = s.split_at(s.len() - 2);
                format!("{head}:{tail}")
            } else {
                s
            }
        }
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_control::setup::SetupConfig;

    // config.yaml のスキーマ・後方互換のテストは tako_control::setup 側にある（Issue #94）

    #[test]
    fn config_from_default_yaml() {
        let config: SetupConfig = serde_yaml::from_str(CONFIG_DEFAULT).unwrap();
        assert!(config.orchestrator.auto_close);
        assert!(!config.setup.completed);
        // モデル設定キーはテンプレに含まれない（profiles/*.yaml が正。Issue #27）
        assert!(!CONFIG_DEFAULT.contains("master_model"));
        assert!(!CONFIG_DEFAULT.contains("worker_model"));
    }

    #[test]
    fn pending_changes_file_sync() {
        let tmp = std::env::temp_dir().join("tako-test-pending-sync");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let pending = pending_changes(0).unwrap();
        assert!(!pending.is_empty(), "初期エントリが存在する");
        // 未適用あり → pending-changes.md が書き出される
        sync_pending_changes_file(&tmp, &pending, 0).unwrap();
        let path = pending_changes_path(&tmp);
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("rev 1"));
        // 追従完了（未適用ゼロ）→ 消える（stale 防止）
        sync_pending_changes_file(&tmp, &[], 4).unwrap();
        assert!(!path.exists());
        // 無い状態での再同期も no-op で成功する
        sync_pending_changes_file(&tmp, &[], 4).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn external_deps_table_is_consistent() {
        // エージェント CLI は 3 者から別途検出するため、汎用依存表には含めない。
        assert!(EXTERNAL_DEPS.iter().all(|dep| !SetupAgent::ALL
            .iter()
            .any(|agent| agent.as_str() == dep.bin)));
        // tmux は任意依存（remote / 永続化 / オーケストレーターが対象機能）
        let tmux = EXTERNAL_DEPS.iter().find(|d| d.bin == "tmux").unwrap();
        assert!(!tmux.required);
        assert!(tmux.purpose.contains("tako remote"));
        assert_eq!(tmux.brew_pkg, Some("tmux"));
        // 依存は tmux / git / tailscale の 3 つ（#282: 旧トンネル用依存は削除済み。
        // #286: tailscale を弾 6 で追加）
        assert_eq!(EXTERNAL_DEPS.len(), 3);
        // 全依存に用途説明と導入案内がある
        for dep in EXTERNAL_DEPS {
            assert!(!dep.purpose.is_empty(), "{} の purpose が空", dep.bin);
            assert!(
                !dep.install_hint.is_empty(),
                "{} の install_hint が空",
                dep.bin
            );
        }
    }

    fn detected(kind: SetupAgent, authenticated: bool, plan: Option<&str>) -> DetectedAgent {
        DetectedAgent {
            kind,
            path: format!("/fake/{}", kind.as_str()),
            authenticated,
            plan: plan.map(str::to_string),
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
            parse_claude_auth_json(&value, true),
            (true, Some("max".into()))
        );
        assert_eq!(parse_claude_auth_json(&value, false), (false, None));

        let api = serde_json::json!({"loggedIn": true, "authMethod": "api_key"});
        assert_eq!(
            parse_claude_auth_json(&api, true),
            (true, Some("api".into()))
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
    fn 複数cliでは認証済みを既定にして番号選択を反映する() {
        let agents = vec![
            detected(SetupAgent::Claude, false, None),
            detected(SetupAgent::Codex, true, Some("plus")),
            detected(SetupAgent::Agy, true, None),
        ];
        assert_eq!(default_agent_index(&agents), 2);
        assert_eq!(choose_setup_agent(&agents, ""), Ok(SetupAgent::Codex));
        assert_eq!(choose_setup_agent(&agents, "3"), Ok(SetupAgent::Agy));
        assert!(choose_setup_agent(&agents, "4").is_err());
        assert_eq!(
            select_setup_agent(&agents, Some("agy"), true, true),
            Ok((SetupAgent::Agy, SetupValueSource::Previous))
        );
    }

    #[test]
    fn 認証済みかつ導入済みのproviderだけをプラン解決対象にする() {
        let agents = vec![
            detected(SetupAgent::Claude, true, Some("pro")),
            detected(SetupAgent::Codex, false, None),
        ];
        assert_eq!(
            detected_provider_plans(&agents),
            vec![(Provider::Claude, Some("pro".into()))]
        );

        let single = vec![detected(SetupAgent::Claude, true, Some("pro"))];
        assert_eq!(detected_provider_plans(&single).len(), 1);
        assert!(detected_provider_plans(&single)
            .iter()
            .all(|(provider, _)| *provider != Provider::Gpt && *provider != Provider::Google));

        let max = collect_provider_plans(
            &[detected(SetupAgent::Claude, true, Some("max"))],
            &BTreeMap::new(),
            false,
            true,
        );
        assert_eq!(max["claude"].value, "max");
        assert_eq!(max["claude"].source, SetupValueSource::Detected);
    }

    #[test]
    fn プラン規模でeffortとworker方針を推奨する() {
        let single = vec![detected(SetupAgent::Claude, true, Some("pro"))];
        let single_plans = BTreeMap::from([
            ("claude".into(), "pro".into()),
            ("gpt".into(), "unknown".into()),
            ("google".into(), "unknown".into()),
        ]);
        let (profile, _) = recommended_profile(SetupAgent::Claude, &single, &single_plans);
        assert_eq!(profile.master_agent.as_deref(), Some("claude"));
        assert_eq!(profile.worker_agent.as_deref(), Some("claude"));
        assert_eq!(profile.effort, "high");
        assert_eq!(
            profile.worker_model_policy,
            tako_control::orchestrator::WorkerModelPolicy::Inherit
        );
        assert!(profile.model.is_none(), "モデルは陳腐化しない CLI 既定");

        let multiple = vec![
            detected(SetupAgent::Claude, true, Some("pro")),
            detected(SetupAgent::Codex, true, Some("pro")),
            detected(SetupAgent::Agy, true, None),
        ];
        let multiple_plans = BTreeMap::from([
            ("claude".into(), "pro".into()),
            ("gpt".into(), "pro".into()),
            ("google".into(), "free".into()),
        ]);
        let (profile, _) = recommended_profile(SetupAgent::Codex, &multiple, &multiple_plans);
        assert_eq!(profile.master_agent.as_deref(), Some("codex"));
        assert_eq!(profile.worker_agent.as_deref(), Some("codex"));
        assert_eq!(profile.effort, "xhigh");
        assert_eq!(
            profile.worker_model_policy,
            tako_control::orchestrator::WorkerModelPolicy::Delegate
        );
        assert_eq!(
            profile
                .worker_agents
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["agy", "claude", "codex"]
        );
        assert!(profile.worker_agents["codex"].skip_permissions);
        assert!(profile.worker_agents["agy"].effort.is_none());
    }

    #[test]
    fn embedded_resources_not_empty() {
        assert!(!SYSTEM_PROMPT.is_empty());
        assert!(!TPL_00_LANGUAGE.is_empty());
        assert!(!TPL_01_INTERACTION.is_empty());
        assert!(!TPL_02_GIT.is_empty());
        assert!(!TPL_03_CODE.is_empty());
        assert!(!TPL_04_SAFETY.is_empty());
        assert!(!TPL_05_PROPOSAL.is_empty());
        assert!(!TPL_06_VERIFICATION.is_empty());
        assert!(!CONFIG_DEFAULT.is_empty());
        assert!(!INSTRUCTIONS_DEFAULT.is_empty());
        assert!(!CHANGES_YAML.is_empty());
    }

    #[test]
    fn system_prompt_mentions_update_follow_flow() {
        // setup エージェントがアップデート追従を実施できるよう、system prompt に
        // pending-changes.md への言及がある（Issue #94）
        assert!(SYSTEM_PROMPT.contains("pending-changes.md"));
        assert!(SYSTEM_PROMPT.contains("changes.yaml"));
    }
}

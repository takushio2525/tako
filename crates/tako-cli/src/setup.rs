//! `tako setup` — 対話式セットアップコマンド。
//!
//! エージェント CLI（claude / codex / agy）の検出・選択とプラン確認 →
//! 依存ツールチェック（tmux・cloudflared・git 任意。未導入は brew で
//! その場インストール可）→ MCP 登録確認 → リソースファイル書き出し →
//! config.yaml の初回/2回目判定 + アップデート追従（Issue #94）→
//! 選択したエージェントを setup cwd で起動する。IPC 不要（tako アプリ未起動でも動作）。
//!
//! config.yaml のスキーマと setup changelog は `tako_control::setup` にある
//! （MCP `tako_setup_changes` と共有。二重実装を作らない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tako_control::setup::{load_config, pending_changes, ChangeKind, SetupChange, CHANGES_YAML};

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
/// setup の対話エージェントとしては利用できる。
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
        bin: "cloudflared",
        required: false,
        purpose: "リモート接続（tako remote）のトンネル公開。未導入だと同一 LAN 内限定の URL になります",
        brew_pkg: Some("cloudflared"),
        install_hint: "https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/",
    },
    ExternalDep {
        bin: "git",
        required: false,
        purpose: "git パネル（ブランチ・コミットグラフ・diff 表示）",
        brew_pkg: Some("git"),
        install_hint: "xcode-select --install でも導入できます",
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
            agent.path
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

fn check_claude_mcp_registered(path: &str) -> bool {
    let output = std::process::Command::new(path)
        .args(["mcp", "list"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("tako")
        }
        _ => false,
    }
}

fn run_setup_mcp() -> Result<(), String> {
    let tako_bin = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| tako_control::dispatch::resolve_tako_binary());
    let settings_dir = home_dir()
        .ok_or("ホームディレクトリが取得できない")?
        .join(".claude");
    let settings_path = settings_dir.join("settings.json");
    match tako_control::dispatch::setup_mcp_settings(&tako_bin, &settings_path) {
        Ok(result) => {
            if result.already_existed {
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
            if check_claude_mcp_registered(&agent.path) {
                eprintln!("  [OK] Claude MCP: tako が登録済み");
                Ok(())
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
            ChangeKind::Guided => "対話で確認",
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

fn select_setup_agent(agents: &[DetectedAgent]) -> Result<SetupAgent, String> {
    match agents {
        [] => Err("エージェント CLI が見つかりません".into()),
        [only] => {
            eprintln!(
                "  [自動選択] 検出された CLI は {} のみです。既定エージェントに設定します",
                only.kind.as_str()
            );
            Ok(only.kind)
        }
        _ => {
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
            choose_setup_agent(agents, input.trim())
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

fn collect_provider_plans(agents: &[DetectedAgent]) -> BTreeMap<String, String> {
    let mut plans = BTreeMap::new();
    for (provider, detected) in detected_provider_plans(agents) {
        let plan = match detected.as_deref() {
            // Claude の status は max の倍率を返さないため、その部分だけ対話で補う。
            Some("max") if provider == Provider::Claude => prompt_plan(provider, Some("max")),
            Some(plan) => {
                eprintln!("  [detected] {} プラン: {plan}", provider.label());
                plan.to_string()
            }
            None => prompt_plan(provider, None),
        };
        plans.insert(provider.as_str().to_string(), plan);
    }
    plans
}

fn prompt_plan(provider: Provider, detected: Option<&str>) -> String {
    eprintln!();
    match provider {
        Provider::Claude if detected == Some("max") => {
            eprintln!("Claude Max を検出しました。契約倍率を選んでください:");
            eprintln!("  1) Max 5x");
            eprintln!("  2) Max 20x");
            eprintln!("  3) 不明");
            eprint!("選択 [3]: ");
            match read_choice("3") {
                "1" => "max-5x".into(),
                "2" => "max-20x".into(),
                _ => "max".into(),
            }
        }
        Provider::Claude => {
            eprintln!("Claude のプランを選んでください:");
            eprintln!("  1) Free / 未契約  2) Pro  3) Max 5x  4) Max 20x");
            eprintln!("  5) Team / Enterprise  6) API  7) 不明");
            eprint!("選択 [7]: ");
            match read_choice("7") {
                "1" => "free",
                "2" => "pro",
                "3" => "max-5x",
                "4" => "max-20x",
                "5" => "team-enterprise",
                "6" => "api",
                _ => "unknown",
            }
            .into()
        }
        Provider::Gpt => {
            eprintln!("GPT / ChatGPT のプランを選んでください:");
            eprintln!("  1) Free / 未契約  2) Plus  3) Pro");
            eprintln!("  4) Business / Enterprise  5) API  6) 不明");
            eprint!("選択 [6]: ");
            match read_choice("6") {
                "1" => "free",
                "2" => "plus",
                "3" => "pro",
                "4" => "business-enterprise",
                "5" => "api",
                _ => "unknown",
            }
            .into()
        }
        Provider::Google => {
            eprintln!("Google のプランを選んでください（agy からは自動取得できません）:");
            eprintln!("  1) Free / 未契約  2) Google AI Pro  3) Google AI Ultra");
            eprintln!("  4) Workspace / Enterprise  5) 不明");
            eprint!("選択 [5]: ");
            match read_choice("5") {
                "1" => "free",
                "2" => "google-ai-pro",
                "3" => "google-ai-ultra",
                "4" => "workspace-enterprise",
                _ => "unknown",
            }
            .into()
        }
    }
}

fn read_choice(default: &str) -> &str {
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    match input.trim() {
        "1" => "1",
        "2" => "2",
        "3" => "3",
        "4" => "4",
        "5" => "5",
        "6" => "6",
        "7" => "7",
        _ => default,
    }
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
    let (recommended, note) = recommended_profile(selected, agents, plans);
    let should_save = if existed {
        eprintln!();
        eprintln!("既存の default プロファイルがあります。プランにもとづく推奨で更新しますか？");
        eprintln!(
            "  推奨: master={} / worker={} / effort={} / policy={:?}",
            recommended.master_agent.as_deref().unwrap_or("claude"),
            recommended.worker_agent.as_deref().unwrap_or("claude"),
            recommended.effort,
            recommended.worker_model_policy
        );
        eprint!("更新する [y/N]: ");
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    } else {
        true
    };
    if should_save {
        if existed {
            orchestrator::Profile::mutate_named("default", |profile| {
                apply_profile_recommendation(profile, &recommended);
            })?;
        } else {
            recommended.save("default")?;
        }
        eprintln!("  [OK] 推奨プロファイルを保存: {}", profile_path.display());
        eprintln!("       {note}");
        Ok("モデルは各 CLI の既定値。effort と worker ポリシーはプラン規模から推奨済み。")
    } else {
        eprintln!("  [維持] 既存プロファイルを変更しませんでした");
        Ok("既存の default profile をユーザー選択で維持。自動推奨は未反映。")
    }
}

fn apply_profile_recommendation(
    profile: &mut tako_control::orchestrator::Profile,
    recommended: &tako_control::orchestrator::Profile,
) {
    // setup が提案する起動設定だけを更新し、ユーザー所有の system prompt・
    // prompt_blocks・projects は保持する。
    profile.master_agent = recommended.master_agent.clone();
    profile.model = recommended.model.clone();
    profile.effort = recommended.effort.clone();
    profile.worker_model_policy = recommended.worker_model_policy;
    profile.worker_model = recommended.worker_model.clone();
    profile.worker_effort = recommended.worker_effort.clone();
    profile.delegate_guidance = recommended.delegate_guidance.clone();
    profile.worker_agent = recommended.worker_agent.clone();
    profile.worker_agents = recommended.worker_agents.clone();
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

// --- メインエントリ ---

/// `tako setup --check` — 環境チェックだけ実行して終了
pub fn run_check() -> Result<(), String> {
    eprintln!("tako セットアップ 環境チェック");
    eprintln!("─────────────────────────────");

    // エージェント CLI + 任意依存。--check では表示のみ。
    let (agents, _) = run_dependency_check(false);

    // MCP 登録（claude のみ永続登録。codex は master 起動時注入、agy は worker 専用）
    if let Some(claude) = agents.iter().find(|a| a.kind == SetupAgent::Claude) {
        if check_claude_mcp_registered(&claude.path) {
            eprintln!("  [OK] Claude MCP: tako が登録済み");
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
            ChangeKind::Guided => "guided（setup の対話で確認・適用）",
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

/// `tako setup` — メインのセットアップフロー
pub fn run_setup() -> Result<(), String> {
    eprintln!("tako セットアップ");
    eprintln!("═════════════════");
    eprintln!();

    // 1. エージェント CLI と依存ツールのチェック
    let (agents, missing) = run_dependency_check(true);
    if !missing.is_empty() {
        return Err(format!(
            "必須の依存ツールが不足しています: {}。\n\
             導入後に tako setup を再実行してください",
            missing.join(", ")
        ));
    }
    let selected = select_setup_agent(&agents)?;
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

    // 1.5 取得できるプランは自動反映し、不足分だけ対話で補う。
    let plans = collect_provider_plans(&agents);

    // 2. 選択エージェントに応じた MCP 設定
    configure_agent_mcp(selected_agent)?;

    // 3. setup ディレクトリ + リソース書き出し
    let dir = setup_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    write_all_resources(&dir)?;
    eprintln!("  [OK] テンプレートを展開: {}", dir.display());

    // 各 CLI の自動読込名 + 明示参照名へ同じ setup 指示を書き出す。
    for filename in [
        "setup-instructions.md",
        "CLAUDE.md",
        "AGENTS.md",
        "GEMINI.md",
    ] {
        let path = dir.join(filename);
        std::fs::write(&path, SYSTEM_PROMPT)
            .map_err(|e| format!("{filename} の書き出しに失敗: {e}"))?;
    }

    // 4. config.yaml の初回 / 2 回目判定
    let config = load_config()?;
    let is_first_run = !config.setup.completed;

    // 4.5 アップデート追従（Issue #94）: 前回セットアップ以降に setup へ入った変更を検出。
    // 初回はすべて最新の内容で導入されるため対象外（完了時に最新リビジョンを記録するのみ）
    let pending = if is_first_run {
        Vec::new()
    } else {
        pending_changes(config.setup.applied_revision)?
    };
    if !pending.is_empty() {
        eprintln!();
        print_pending_changes(&pending, config.setup.applied_revision);
        eprintln!(
            "      詳細を pending-changes.md に書き出しました。選択したエージェントが対話で追従を案内します"
        );
    }
    sync_pending_changes_file(&dir, &pending, config.setup.applied_revision)?;

    // 5. 検出プランにもとづくプロファイル推奨を生成する。
    let profile_note = prepare_profile(selected, &agents, &plans)?;
    write_setup_context(&dir, selected, &agents, &plans, profile_note)?;

    // 選択エージェントのグローバル指示ファイルが存在すればバックアップする。
    if let Some(instruction) = instruction_path(selected) {
        if instruction.is_file() {
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
    }

    eprintln!();
    if is_first_run {
        eprintln!(
            "初回セットアップを開始します。{} が対話で設定をガイドします。",
            selected.as_str()
        );
    } else {
        eprintln!("セットアップメニューを開きます。");
    }
    eprintln!("─────────────────────────────────────────────────────");
    eprintln!();

    let greeting = if is_first_run {
        "最初に setup-instructions.md を読んでください。tako のセットアップを始めます。CLI 側でエージェント選択・プラン確認・推奨プロファイル生成は完了済みです。残りの質問を1つずつ進めてください。"
    } else if !pending.is_empty() {
        "最初に setup-instructions.md と pending-changes.md を読んでください。前回セットアップ以降のアップデート変更への追従から始めてください。"
    } else {
        "最初に setup-instructions.md を読んでください。tako の設定を変更します。何をしますか？"
    };
    let status = launch_setup_agent(selected_agent, &dir, greeting)?;

    if status.success() {
        // セットアップ完了を記録（適用済み setup リビジョンを含む。Issue #94）。
        // claude 対話中に他プロセスが config.yaml を更新していても巻き戻さないよう、
        // 完了フィールドだけをロック付き read-modify-write で更新する（#169）
        let revision = tako_control::setup::current_revision()?;
        tako_control::setup::mutate_config(|config| {
            config.setup.completed = true;
            config.setup.completed_at = Some(now_iso8601());
            config.setup.applied_revision = revision;
            config.setup.applied_version = Some(env!("CARGO_PKG_VERSION").to_string());
            config.setup.selected_agent = Some(selected.as_str().to_string());
            config.setup.provider_plans = plans.clone();
        })?;
        // 追従が完了したので pending-changes.md を消す（stale 防止）
        sync_pending_changes_file(&dir, &[], revision)?;
        eprintln!();
        eprintln!("セットアップが完了しました。");
    } else {
        eprintln!();
        eprintln!(
            "{} が終了しました（exit code: {}）",
            selected.as_str(),
            status.code().unwrap_or(-1)
        );
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
        // cloudflared は任意依存（トンネル公開。未導入だと LAN 限定 URL = #89）
        let cf = EXTERNAL_DEPS
            .iter()
            .find(|d| d.bin == "cloudflared")
            .unwrap();
        assert!(!cf.required);
        assert_eq!(cf.brew_pkg, Some("cloudflared"));
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
    fn 推奨profile更新はsystem_promptとprojectsを保持する() {
        let mut existing = tako_control::orchestrator::Profile {
            system_prompt: Some("custom.md".into()),
            prompt_blocks: Some(tako_control::orchestrator::PromptBlocks {
                prepend: Some("custom".into()),
                ..Default::default()
            }),
            projects: Some(vec!["demo".into()]),
            ..Default::default()
        };
        let recommended = tako_control::orchestrator::Profile {
            master_agent: Some("codex".into()),
            effort: "high".into(),
            worker_agent: Some("codex".into()),
            ..Default::default()
        };
        apply_profile_recommendation(&mut existing, &recommended);
        assert_eq!(existing.master_agent.as_deref(), Some("codex"));
        assert_eq!(existing.system_prompt.as_deref(), Some("custom.md"));
        assert_eq!(
            existing
                .prompt_blocks
                .as_ref()
                .and_then(|blocks| blocks.prepend.as_deref()),
            Some("custom")
        );
        assert_eq!(existing.projects, Some(vec!["demo".into()]));
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

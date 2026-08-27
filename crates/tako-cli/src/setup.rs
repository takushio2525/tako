//! `tako setup` — 検出 + 対話の自動セットアップコマンド。
//!
//! エージェント CLI（claude / codex / agy）の検出・プラン解決 →
//! 依存ツールチェック → MCP 登録 → 指示・profile・リソース生成 → 対話エージェント起動、
//! の一連のフローを提供する。対話 agent は既定で起動し、--yes / 非 TTY 時のみスキップ。
//! IPC 不要で、tako アプリ未起動でも動作する。
//!
//! config.yaml のスキーマと setup changelog は `tako_control::setup` にある
//! （MCP `tako_setup_changes` と共有。二重実装を作らない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tako_control::config_share::env::ShareEnvironment;
use tako_control::setup::{
    compare_instruction_coverage, load_config, pending_changes, resolve_setup_value, ChangeKind,
    InstructionCoverage, ResolvedSetupValue, SetupAnswers, SetupChange, SetupPlan,
    SetupValueSource, CHANGES_YAML, INSTRUCTIONS_DEFAULT, RECOMMENDED_SECTIONS,
};
use tako_control::setup_bootstrap::{self, InstallOptions, Step};

// --- バイナリ埋め込みリソース ---
// 推奨ルールのセクションと既定指示ファイルは tako_control::setup が正
// （項目レベル比較 Issue #322 と共有。二重埋め込みを作らない）

// 正本は tako-control 側に一元化（#516）
use tako_control::setup::SYSTEM_PROMPT;
const CONFIG_DEFAULT: &str = include_str!("../../../resources/setup/templates/config-default.yaml");

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

/// 実行ファイルの探索は抽象境界 B16（[`tako_core::platform::exe`]）に委ねる。
///
/// ここで `$SHELL -l -c "command -v"` を直書きすると Windows では**必ず失敗**する（#525）。
/// macOS 側の解決手順（ログインシェル経由 = `.app` の痩せた PATH でも Homebrew を引ける）は
/// 境界の内側にそのまま移してあるので、macOS の挙動は変わらない
fn find_command(name: &str) -> Option<String> {
    tako_core::platform::exe::find(name)
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
    // #586: `tako setup` は dispatch（GUI 内）からも呼ばれる。そちらは既に
    // コンソールを持たないので、ここを素で起動すると子ごとにウィンドウが出る
    tako_core::platform::process::no_console_window(&mut std::process::Command::new(path))
        .args(args)
        .output()
        .ok()
}

/// エージェント CLI の実行ファイルを解決する。
///
/// **入れた直後は PATH 経由で引けないことがある**（profile へ書いても、判定できない
/// シェルや書き込み失敗では届かない）。その場合でもインストーラが置く場所を見て拾う。
/// ここで拾えないと「入れたのに見つかりません」で setup が止まる（#868）
fn find_agent_command(kind: SetupAgent) -> Option<String> {
    find_command(kind.as_str()).or_else(|| match kind {
        SetupAgent::Claude => setup_bootstrap::resolve_binary(),
        _ => None,
    })
}

fn detect_agents() -> Vec<DetectedAgent> {
    SetupAgent::ALL
        .into_iter()
        .filter_map(|kind| {
            let path = find_agent_command(kind)?;
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

// --- ゼロスタート導入（Issue #868）---

/// `tako setup bootstrap`。MCP `tako_setup_bootstrap` と同じ dispatch 相当を
/// ローカルで実行する（GUI 不要の処理なので IPC を経由しない）
pub fn run_bootstrap(action: Option<&str>, dry_run: bool, json: bool) -> Result<(), String> {
    let action = action.unwrap_or("status");
    let value = match action {
        "status" => setup_bootstrap::status()?.to_json(),
        "install" => setup_bootstrap::install(InstallOptions {
            dry_run,
            // CLI は端末を持つので進捗をそのまま流す
            interactive: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        })?,
        "path" => setup_bootstrap::ensure_path()?,
        "undo-path" => setup_bootstrap::undo_path()?,
        other => {
            return Err(format!(
                "不明な action: {other:?}（status / install / path / undo-path のいずれか）"
            ))
        }
    };
    if json {
        println!("{}", crate::pretty_json(&value));
        return Ok(());
    }
    print_bootstrap_human(action, &value);
    Ok(())
}

fn print_bootstrap_human(action: &str, value: &serde_json::Value) {
    match action {
        "status" => {
            let step = value["next_step"].as_str().unwrap_or("?");
            eprintln!("エージェント CLI の導入状況");
            eprintln!("─────────────────────────");
            eprintln!(
                "  claude: {}",
                value["binary"]
                    .as_str()
                    .map_or("未導入".to_string(), |p| display_home_relative(
                        Path::new(p)
                    ))
            );
            eprintln!(
                "  PATH:   {}",
                if value["launcher_dir_on_path"].as_bool() == Some(true) {
                    "通っています"
                } else {
                    "通っていません"
                }
            );
            eprintln!(
                "  認証:   {}",
                if value["authenticated"].as_bool() == Some(true) {
                    "済み"
                } else {
                    "未ログイン"
                }
            );
            eprintln!(
                "  次の一歩: {} ({step})",
                value["next_step_description"].as_str().unwrap_or("")
            );
            if step != "ready" {
                eprintln!("  → `tako setup` を実行すると、ここから最後まで案内します");
            }
        }
        "install" => {
            if value["performed"].as_bool() == Some(true) {
                eprintln!("インストールが完了しました");
            } else {
                eprintln!("実行はしていません（--dry-run）。実行される内容:");
                print_install_plan(&value["install_plan"]);
            }
        }
        _ => {
            for key in ["profile_display", "dir_display", "change", "note"] {
                if let Some(text) = value[key].as_str() {
                    eprintln!("  {key}: {text}");
                }
            }
        }
    }
}

/// **何をどこに入れるか**（受け入れ条件 4）。実行前に必ず出す
fn print_install_plan(plan: &serde_json::Value) {
    for line in plan["lines"].as_array().into_iter().flatten() {
        if let Some(text) = line.as_str() {
            eprintln!("    - {text}");
        }
    }
}

/// [y/N] / [Y/n] の確認。
///
/// 非対話（`--yes` / 非 TTY）では質問せず続行する。**ただし黙って進めない**:
/// 何を省略したかを 1 行出す（計画自体は呼び出し側が直前に必ず表示している。
/// 受け入れ条件 4「--yes 時は表示のみ」）
fn confirm(prompt: &str, default_yes: bool, assume_yes: bool) -> bool {
    if assume_yes || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!("  {prompt} → 非対話モードのため確認を省略して続行します");
        return true;
    }
    eprint!("  {prompt} [{}]: ", if default_yes { "Y/n" } else { "y/N" });
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return default_yes;
    }
    match input.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

/// ゼロスタート導入の段（#868）。**導入済みなら何も出さずに素通りする**
/// （検出型 setup の従来体験を変えないため）。
///
/// 未導入なら インストール → PATH 通し → 認証誘導 の順に進め、
/// どの段で失敗したかが分かる形でエラーを返す
fn run_bootstrap_stage(assume_yes: bool) -> Result<(), String> {
    let state = setup_bootstrap::status()?;
    if state.step == Step::Ready {
        return Ok(());
    }
    // ここへ来るのは「claude が無い / PATH に無い / 未ログイン」のいずれか。
    // 何が起きるのかを先に伝えてから進む
    eprintln!("はじめてのセットアップ");
    eprintln!("─────────────────────");
    eprintln!("  Claude Code を使えるところまで、このまま案内します");
    eprintln!();

    if state.step == Step::Install {
        eprintln!("  [1/3] {}", Step::Install.describe());
        print_install_plan(&state.plan.to_json());
        if !state.plan.can_run {
            return Err(format!(
                "この環境では自動インストールに対応していません。\n\
                 上のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {}",
                state.plan.official_command
            ));
        }
        if !confirm("この内容でインストールしますか？", true, assume_yes) {
            return Err(format!(
                "インストールを中止しました。\n\
                 自分で入れる場合は次のコマンドを実行してから `tako setup` をやり直してください:\n  {}",
                state.plan.official_command
            ));
        }
        eprintln!();
        setup_bootstrap::install(InstallOptions {
            dry_run: false,
            interactive: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        })
        .map_err(|e| format!("[1/3] インストールに失敗しました。\n{e}"))?;
        eprintln!("  [OK] Claude Code を導入しました");
    }

    // PATH（インストール直後は必ずここを通る）
    let state = setup_bootstrap::status()?;
    if state.step == Step::Path {
        eprintln!();
        eprintln!("  [2/3] {}", Step::Path.describe());
        let result = setup_bootstrap::ensure_path()
            .map_err(|e| format!("[2/3] PATH の設定に失敗しました。\n{e}"))?;
        let profile = result["profile_display"].as_str().unwrap_or("(不明)");
        match result["change"].as_str() {
            Some("already_on_path") => eprintln!("  [OK] すでに使える状態です"),
            Some("unchanged") => eprintln!("  [OK] {profile} は設定済みです"),
            _ => eprintln!("  [OK] {profile} に設定を追加しました"),
        }
        if result["verified"].as_bool() != Some(true) {
            eprintln!(
                "  [警告] {}",
                result["note"]
                    .as_str()
                    .unwrap_or("反映はターミナルの開き直し後です")
            );
        }
    }

    // 認証（ブラウザが開く。非対話では代行しない）
    let state = setup_bootstrap::status()?;
    if state.step == Step::Auth {
        let binary = state
            .binary
            .clone()
            .ok_or("claude コマンドを解決できません")?;
        eprintln!();
        eprintln!("  [3/3] {}", Step::Auth.describe());
        let interactive = !assume_yes && std::io::IsTerminal::is_terminal(&std::io::stdin());
        if !interactive {
            return Err(
                "Claude アカウントへのログインが必要です。\n\
                 ブラウザでの操作が要るため自動化できません。\n\
                 `claude auth login` を実行してログインしてから `tako setup` をやり直してください"
                    .to_string(),
            );
        }
        eprintln!("  ブラウザが開きます。画面の指示にしたがってログインしてください");
        eprintln!();
        let status = tako_core::platform::process::no_console_window(
            &mut std::process::Command::new(&binary),
        )
        .args(["auth", "login"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("[3/3] claude auth login を起動できません: {e}"))?;
        if !status.success() {
            return Err(format!(
                "[3/3] ログインが完了しませんでした（exit {}）。\n\
                 `claude auth login` を自分で実行してから `tako setup` をやり直してください",
                status.code().unwrap_or(-1)
            ));
        }
        // 「起動した」ではなく「ログインできた」ことを確かめてから次へ進む
        if !setup_bootstrap::is_authenticated(&binary) {
            return Err("[3/3] ログインを確認できませんでした。\n\
                 `claude auth status` で状態を確認し、必要なら `claude auth login` を\
                 やり直してから `tako setup` を再実行してください"
                .to_string());
        }
        eprintln!("  [OK] ログインしました");
    }

    eprintln!();
    eprintln!("  導入が完了しました。続けて設定を行います");
    eprintln!();
    Ok(())
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
    // #586: dispatch（GUI 内）から `tako setup` 経由でも走る。親がコンソールを
    // 持たないと Windows は**子のために新しいコンソールを作る**ので、対話でない
    // 子はすべて塞ぐ
    let output = tako_core::platform::process::no_console_window(&mut std::process::Command::new(
        claude_path,
    ))
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
            // ~/.claude.json から登録パスを直接読む（claude mcp list の出力は
            // ✔/✘ の有無やフォーマットがバージョンで変わり得るため）
            let path_alive = read_mcp_command_path()
                .map(|p| std::path::Path::new(&p).is_file())
                .unwrap_or(true); // 読めなければ楽観判定
            (true, path_alive)
        }
        _ => (false, false),
    }
}

/// ~/.claude.json から tako MCP 登録の command パスを読み取る
fn read_mcp_command_path() -> Option<String> {
    let home = home_dir()?;
    let path = home.join(".claude.json");
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
    let scope = tako_control::dispatch::McpScope::User;
    match tako_control::dispatch::setup_mcp(&tako_bin, &scope) {
        Ok(result) => {
            if result.repaired {
                let old = result.old_command.as_deref().unwrap_or("(不明)");
                eprintln!("  [修復] MCP: 登録パスが消失していたため付け替えました");
                eprintln!("         旧: {old}");
                eprintln!("         新: {tako_bin}");
            } else if result.already_existed {
                eprintln!("  MCP: 既に設定されています");
            } else {
                eprintln!(
                    "  MCP: 設定を追加しました（{}）",
                    result.target_path.display()
                );
            }
            if result.legacy_cleaned {
                eprintln!("  [掃除] 旧 settings.json の無効な MCP 設定を除去しました");
            }
            Ok(())
        }
        Err(e) => Err(format!("MCP 設定の追加に失敗: {e}")),
    }
}

/// 検出済みの全エージェントへ tako MCP を登録する（#979）。
///
/// **選択したエージェントだけに絞らない**: 推奨プロファイルは `worker_agents` に
/// 複数を並べるので（`recommended_profile`）、worker として使う CLI からも
/// tako のツールが見えないと設計原則 5「AI フルコントロール」が破れる。
/// 個別の失敗は setup 全体を止めず理由を出す（未導入の CLI はそもそも列挙されない）
fn configure_agent_mcp(agents: &[DetectedAgent]) -> Result<(), String> {
    // claude は登録の健全性まで見て修復する（従来経路。回帰させない）
    if let Some(claude) = agents.iter().find(|a| a.kind == SetupAgent::Claude) {
        let (registered, healthy) = check_claude_mcp_health(&claude.path);
        if registered && healthy {
            eprintln!("  [OK] Claude MCP: tako が登録済み");
        } else {
            if registered {
                eprintln!("  [警告] Claude MCP: 登録パスが消失しています。修復します");
            } else {
                eprintln!("  [設定] Claude MCP を自動登録します");
            }
            run_setup_mcp()?;
        }
    }

    for agent in agents {
        let kind = match agent.kind {
            SetupAgent::Codex => tako_control::orchestrator::agent::WorkerAgent::Codex,
            SetupAgent::Agy => tako_control::orchestrator::agent::WorkerAgent::Agy,
            SetupAgent::Claude => continue,
        };
        let tako_bin = tako_control::dispatch::resolve_tako_binary();
        match tako_control::agent_mcp::register(kind, &tako_bin) {
            Ok(result) if !result.configured => {
                eprintln!("  [OK] {} MCP: tako が登録済み", kind.as_str());
            }
            Ok(result) if result.repaired => {
                eprintln!(
                    "  [修復] {} MCP: 登録パスが消失していたため付け替えました",
                    kind.as_str()
                );
            }
            Ok(_) => {
                eprintln!("  [設定] {} MCP に tako を登録しました", kind.as_str());
            }
            // setup 全体は止めない（他の設定は進めたい）が、無言にはしない
            Err(e) => eprintln!("  [警告] {} MCP: {e}", kind.as_str()),
        }
    }
    if agents.iter().any(|a| a.kind == SetupAgent::Codex) {
        eprintln!("         codex は master 起動時にも一時設定を注入します");
    }
    Ok(())
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
    for (rel_path, content) in RECOMMENDED_SECTIONS {
        write_resource(setup_dir, rel_path, content)?;
    }
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
            ChangeKind::Guided => "対話で個別確認",
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
    /// 同梱推奨ルールとの項目レベル比較の結果（Issue #322）。
    /// setup agent（--review）が Step 1 の裏取りに使う
    instruction_coverage: InstructionCoverageContext,
    /// 設定共有の現状（Issue #793）。setup agent が案内・代行の判断に使う。
    /// **検出は CLI 側で済ませてある**（agent に探索させない = 毎回同じ判断になる）
    config_share: ConfigShareContext<'a>,
}

/// setup agent へ渡す設定共有の現状（Issue #793）
#[derive(serde::Serialize)]
struct ConfigShareContext<'a> {
    /// 配線済みか。true なら**案内しない**（#793 受け入れ条件 4 = 冪等）
    linked: bool,
    /// 配線先（ホームは `~` 表記）
    repo: Option<&'a str>,
    /// 配線先が生きた git リポジトリか
    repo_ok: bool,
    /// linked / broken / adopt_existing / fresh
    guidance: &'static str,
    /// 提示してよい最簡の次の一手（#322）
    next_command: String,
    /// gh CLI の状態（missing / unauthenticated / authenticated / unknown）。
    /// 配線済みのときは判定しないので null
    gh: Option<&'static str>,
    /// `gh repo create` の代行を提案してよいか（authenticated のときだけ true）
    gh_can_create_repo: bool,
    /// 既に外部 git（dotfiles 等）で管理されている共有対象
    external: &'a [tako_control::config_share::env::ExternalManaged],
}

impl<'a> ConfigShareContext<'a> {
    fn from_env(env: &'a ShareEnvironment) -> Self {
        Self {
            linked: env.linked,
            repo: env.repo.as_deref(),
            repo_ok: env.repo_ok,
            guidance: env.guidance().as_str(),
            next_command: env.next_command(),
            gh: env.gh.map(|gh| gh.as_str()),
            gh_can_create_repo: env.gh_can_create_repo(),
            external: &env.external,
        }
    }
}

#[derive(serde::Serialize)]
struct InstructionCoverageContext {
    /// full = 差分なし / partial = 不足の可能性あり / created_default = 同梱既定で新規作成
    status: &'static str,
    /// 「項目タイトル: 概念1・概念2」形式の不足一覧（full / created_default では空）
    missing: Vec<String>,
}

impl InstructionCoverageContext {
    fn from_coverage(coverage: Option<&InstructionCoverage>) -> Self {
        match coverage {
            None => Self {
                status: "created_default",
                missing: Vec::new(),
            },
            Some(coverage) if coverage.is_full() => Self {
                status: "full",
                missing: Vec::new(),
            },
            Some(coverage) => Self {
                status: "partial",
                missing: coverage.missing_summaries(),
            },
        }
    }
}

fn write_setup_context(
    dir: &Path,
    selected: SetupAgent,
    agents: &[DetectedAgent],
    plans: &BTreeMap<String, String>,
    profile_note: &str,
    instruction_coverage: Option<&InstructionCoverage>,
    share_env: &ShareEnvironment,
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
        instruction_coverage: InstructionCoverageContext::from_coverage(instruction_coverage),
        config_share: ConfigShareContext::from_env(share_env),
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

/// setup 完了後の「次の一歩」案内（Issue #322 受け入れ条件 2）。
/// オーケストレーションの最短導線とプロファイルの説明を default profile の実値つきで示す。
/// コマンドは最も簡単な形で案内する（既定で済むものに引数を付けない。`.agent/conventions.md`）
fn print_next_steps(master_ready: bool) {
    use tako_control::orchestrator::{Profile, WorkerModelPolicy};

    eprintln!();
    eprintln!("次の一歩:");
    if master_ready {
        eprintln!(
            "  tako master   オーケストレーションを開始します。起動したら、やってほしいことを"
        );
        eprintln!("                日本語で話しかけるだけです（worker の起動・監視・完了報告・");
        eprintln!("                プロジェクト登録などの設定変更は、すべて master に頼めます）");
        eprintln!("  tako solo     worker を使わず 1 対 1 で対話します");
    } else {
        eprintln!("  agy は worker 専用です。tako master（オーケストレーション）を使うには");
        eprintln!("  claude または codex を導入してログインし、tako setup を再実行してください");
    }

    let profile = Profile::load("default").unwrap_or_default();
    let master = profile.master_agent.as_deref().unwrap_or("claude");
    let worker = profile.worker_agent.as_deref().unwrap_or(master);
    let policy = match profile.worker_model_policy {
        WorkerModelPolicy::Inherit => "master に合わせる",
        WorkerModelPolicy::Fixed => "固定値",
        WorkerModelPolicy::Delegate => "master がタスクごとに判断",
    };
    eprintln!();
    eprintln!("プロファイル（master / worker がどのエージェント・モデル・思考量で動くかの設定）:");
    eprintln!(
        "  現在の default: master={master} / worker={worker} / effort={} / worker モデルは {policy}",
        profile.effort
    );
    eprintln!("  「品質重視にして」「利用回数を節約して」のような調整は master に日本語で頼めます");
}

/// 設定共有ステップの決定（Issue #513）。
/// **標準 setup で質問が増えないこと**を機械検証できるよう、判定だけを純粋関数にする
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigShareStep {
    /// 何もしない（明示的に無効と回答された）
    Skip,
    /// 案内 1 行だけ。**質問しない**
    Info,
    /// すでに配線済みなので状態だけ知らせる
    AlreadyLinked,
    /// 対話で 1 回だけ聞く（`--review` の TTY 経路のみ）
    Ask,
    /// 指定内容で配線する（非対話）
    Link {
        repo: Option<String>,
        path: Option<String>,
        remote: Option<String>,
    },
}

fn decide_config_share_step(
    review: bool,
    assume_yes: bool,
    is_tty: bool,
    already_linked: bool,
    answers: Option<&tako_control::setup::SetupConfigShareAnswers>,
) -> ConfigShareStep {
    // 明示指定が最優先（MCP `tako_setup` / `--answers` 経路）。配線済みでも指定に従う
    if let Some(answer) = answers {
        if answer.enable != Some(true) {
            return ConfigShareStep::Skip;
        }
        return ConfigShareStep::Link {
            repo: answer.repo.clone(),
            path: answer.path.clone(),
            remote: answer.remote.clone(),
        };
    }
    if already_linked {
        return ConfigShareStep::AlreadyLinked;
    }
    // 聞いてよいのは `--review` の対話だけ。標準 setup（#262 の質問ゼロ）は案内どまり
    if review && !assume_yes && is_tty {
        ConfigShareStep::Ask
    } else {
        ConfigShareStep::Info
    }
}

/// 設定共有の状態表示（Issue #793）。setup サマリと `--check` が**同じ判定**
/// （`config_share::env::Guidance`）から文言を作るので、片方だけ古くならない。
/// **質問は含まない**（#262 の質問ゼロ）。`verbose` = setup サマリ向けに説明行を足す
fn config_share_lines(env: &ShareEnvironment, verbose: bool) -> Vec<String> {
    use tako_control::config_share::env::Guidance;

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

/// 設定共有（Issue #513 / #793）の案内・配線。**標準 setup では質問を増やさない**（#262）。
///
/// - `answers.config_share` があれば非対話で配線する（MCP `tako_setup` / `--answers` 経路）
/// - `--review` の対話では y/N で 1 回だけ聞く
/// - それ以外は検出結果（`env`）にもとづく状態表示だけ（質問しない）。
///   実際の設定は、このあと起動する対話アシスタントが代行する（#793）
///
/// 失敗しても setup 全体は止めない（共有はオプションであって前提ではない）
fn apply_config_share(
    review: bool,
    assume_yes: bool,
    answers: Option<&tako_control::setup::SetupConfigShareAnswers>,
    env: &ShareEnvironment,
    agent_follows: bool,
) -> Result<(), String> {
    let step = decide_config_share_step(
        review,
        assume_yes,
        std::io::IsTerminal::is_terminal(&std::io::stdin()),
        env.linked,
        answers,
    );
    match step {
        ConfigShareStep::Skip => return Ok(()),
        ConfigShareStep::Link { repo, path, remote } => {
            return run_config_share_link(repo.as_deref(), path.as_deref(), remote.as_deref())
        }
        // 配線済み・案内のどちらも「表示だけ」。質問は増やさない（#262）
        ConfigShareStep::AlreadyLinked | ConfigShareStep::Info => {
            eprintln!();
            for line in config_share_lines(env, true) {
                eprintln!("{line}");
            }
            // 代行できるのは対話アシスタントが続けて起動するときだけ。
            // `--yes` / 非 TTY では誰も代行しないので、その案内も出さない（#793 受け入れ条件 5）
            if agent_follows && env.guidance().invites_setup() {
                eprintln!(
                    "         このあとの対話アシスタントに「設定を共有したい」と言えば{}",
                    if env.gh_can_create_repo() {
                        "、リポジトリ作成から配線まで任せられます"
                    } else {
                        "、そのまま設定できます"
                    }
                );
            }
            return Ok(());
        }
        ConfigShareStep::Ask => {}
    }

    eprintln!();
    eprintln!("設定共有（任意。Issue #513）");
    eprintln!("  claude のグローバル指示（CLAUDE.md / snippets / commands / templates）と");
    eprintln!("  tako の宣言的設定（profiles / projects / accounts / local-rules）を");
    eprintln!("  git リポジトリ 1 本で別デバイスと共有できます。");
    eprintln!("  秘匿情報（token / credentials）とマシン固有の状態は構造的に除外されます。");
    for found in &env.external {
        eprintln!(
            "  検出: {} は既に {} で管理されています{}",
            found.path,
            found.repo,
            if found.same_place {
                "（相乗りすれば同じ場所に載るので重複しません）"
            } else {
                "（別リポジトリを作ると同じ内容が 2 箇所に並びます）"
            }
        );
    }
    eprint!("  いま設定しますか？ [y/N]: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return Ok(());
    }
    let answer = input.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("  スキップしました（あとから `tako config init` で設定できます）");
        return Ok(());
    }

    let suggested = env
        .external
        .first()
        .filter(|found| found.same_place)
        .map(|found| found.repo.clone());
    match &suggested {
        Some(repo) => {
            eprintln!("  既存の共有リポジトリがあればパスか URL を。Enter で {repo} へ相乗りします")
        }
        None => {
            eprintln!("  既存の共有リポジトリがあればパスか URL を、無ければ Enter（新規作成）")
        }
    }
    eprint!("  リポジトリ: ");
    let mut repo = String::new();
    if std::io::stdin().read_line(&mut repo).is_err() {
        return Ok(());
    }
    let repo = repo.trim().to_string();
    if !repo.is_empty() {
        return run_config_share_link(Some(&repo), None, None);
    }
    if let Some(repo) = suggested {
        return run_config_share_link(Some(&repo), None, None);
    }
    eprintln!("  新規作成します。GitHub 等に置くならリモート URL を、ローカルだけなら Enter");
    eprint!("  リモート URL: ");
    let mut remote = String::new();
    if std::io::stdin().read_line(&mut remote).is_err() {
        return Ok(());
    }
    let remote = remote.trim().to_string();
    run_config_share_link(None, None, (!remote.is_empty()).then_some(remote.as_str()))
}

/// 実際の配線（新規作成 or 既存への接続）。dispatch を通すので CLI / MCP と経路が同じ
fn run_config_share_link(
    repo: Option<&str>,
    path: Option<&str>,
    remote: Option<&str>,
) -> Result<(), String> {
    let (action, target) = match repo {
        Some(repo) => ("link", Some(repo)),
        None => ("init", None),
    };
    match tako_control::dispatch::dispatch_config_share(action, target, path, remote, None, false) {
        Ok(result) => {
            let repo_path = result["repo"]
                .as_str()
                .or_else(|| result["push"]["repo"].as_str())
                .unwrap_or("?");
            eprintln!("  [OK] 設定共有を配線しました: {repo_path}");
            if action == "link" {
                eprintln!("       `tako config pull` でこのデバイスへ取り込めます");
            }
            Ok(())
        }
        Err(e) => {
            // 共有はオプション。ここで setup 全体を落とさない
            eprintln!("  [警告] 設定共有の配線に失敗しました: {e}");
            eprintln!(
                "       あとから `tako config init` / `tako config link <パス>` で設定できます"
            );
            Ok(())
        }
    }
}

/// グローバル指示ファイルを解決し、既存内容は同梱推奨ルールと項目レベルで比較する（Issue #322）。
/// 戻り値は setup-context.yaml へ書かれ、`--review` の setup agent が裏取りに使う。
/// None = 同梱既定で新規作成（既定は全項目カバー済みのため比較不要。機械検証は tako-control のテスト）
fn apply_instruction(
    agent: SetupAgent,
    provided: Option<&str>,
) -> Result<Option<InstructionCoverage>, String> {
    let path = instruction_path(agent).ok_or("グローバル指示ファイルのパスを取得できません")?;
    if let Some(content) = provided {
        tako_control::config_io::atomic_write_with_backup(&path, content)?;
        eprintln!(
            "  [input] グローバル指示ファイルを保存: {}",
            display_home_relative(&path)
        );
        return Ok(Some(report_instruction_coverage(content)));
    }
    if path.is_file() {
        eprintln!(
            "  [previous] グローバル指示ファイルを維持: {}",
            display_home_relative(&path)
        );
        let existing = std::fs::read_to_string(&path)
            .map_err(|e| format!("グローバル指示ファイルの読み取りに失敗: {e}"))?;
        return Ok(Some(report_instruction_coverage(&existing)));
    }
    tako_control::config_io::atomic_write(&path, INSTRUCTIONS_DEFAULT)?;
    eprintln!(
        "  [default] グローバル指示ファイルを作成: {}",
        display_home_relative(&path)
    );
    Ok(None)
}

/// 項目レベル比較の結果を表示して返す。「なんとなく良さそう」の素通しをさせない（Issue #322）
fn report_instruction_coverage(content: &str) -> InstructionCoverage {
    let coverage = compare_instruction_coverage(content);
    for line in coverage.render_lines() {
        eprintln!("  {line}");
    }
    if !coverage.is_full() {
        eprintln!(
            "      補強するには、tako master を起動して「グローバルルールを tako の推奨で補強して」と頼んでください"
        );
    }
    coverage
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

    // ゼロスタート導入の状況（#868）。未導入・PATH 未通し・未ログインのどれで
    // 止まっているかを、実行はせずに先に出す
    match setup_bootstrap::status() {
        Ok(state) if state.step == Step::Ready => {
            eprintln!("  [OK] エージェント CLI の導入: 完了（claude / PATH / ログイン）");
        }
        Ok(state) => {
            eprintln!(
                "  [不足] エージェント CLI の導入: {} ({})",
                state.step.describe(),
                state.step.as_str()
            );
            eprintln!("         tako setup を実行すると、ここから最後まで案内します");
        }
        Err(e) => eprintln!("  [警告] 導入状況を確認できません: {e}"),
    }

    // エージェント CLI + 任意依存。--check では表示のみ。
    let (agents, _) = run_dependency_check(false);

    // MCP 登録（#979 で claude / codex / agy の 3 系統とも永続登録になった）
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
    for agent in &agents {
        let kind = match agent.kind {
            SetupAgent::Codex => tako_control::orchestrator::agent::WorkerAgent::Codex,
            SetupAgent::Agy => tako_control::orchestrator::agent::WorkerAgent::Agy,
            SetupAgent::Claude => continue,
        };
        let name = kind.as_str();
        match tako_control::agent_mcp::current_registration(kind) {
            Some(cmd) if std::path::Path::new(&cmd).is_file() => {
                eprintln!("  [OK] {name} MCP: tako が登録済み");
            }
            Some(cmd) => {
                eprintln!("  [警告] {name} MCP: 登録済みだがパスが消失しています");
                eprintln!("         登録パス: {cmd}");
                eprintln!("         tako setup または tako setup-mcp で修復できます");
            }
            None => {
                eprintln!("  [不足] {name} MCP: tako が未登録（tako setup-mcp で登録できます）")
            }
        }
    }
    if agents.iter().any(|a| a.kind == SetupAgent::Codex) {
        eprintln!("  [OK] Codex: master 起動時にも一時注入");
    }
    if agents.iter().any(|a| a.kind == SetupAgent::Agy) {
        eprintln!("  [情報] agy: worker 専用（master は非対応）");
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
        // 蓋閉じ継続を持たない OS では案内しない（#524）
        if tako_control::sleep_guard::lid_control_supported() {
            let lid_mode = settings.lid_sleep_mode;
            // 未完了かどうかだけを見る。手段（macOS の sudoers）は sleep_guard の内側（#697）
            let setup_pending = tako_control::sleep_guard::lid_setup_pending();
            match lid_mode {
                tako_control::sleep_guard::LidSleepMode::Off => {
                    eprintln!(
                        "  [情報] 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）"
                    );
                }
                tako_control::sleep_guard::LidSleepMode::WhileAgentsRunning => {
                    if setup_pending {
                        eprintln!("  [不足] 蓋閉じ防止: while-agents-running だが sudoers 未登録（tako sleep-guard install-lid-sleep で登録）");
                    } else {
                        eprintln!("  [OK] 蓋閉じ防止: while-agents-running");
                    }
                }
            }
        }
    }

    // 設定共有（Issue #513 / #793）。表示だけで、配線もリポジトリ作成もしない
    for line in config_share_lines(&tako_control::config_share::env::detect(), false) {
        eprintln!("{line}");
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
            ChangeKind::Guided => "guided（対話で個別確認・適用）",
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
/// 設定・データファイルのスキーマ移行を実行する（#916 の恒久原則: 手動移行はさせない）。
///
/// 冪等で、移したものがあるときだけ 1 行出す（無ければ無言）。失敗しても setup 自体は
/// 止めない（移行できないのは原本が残っているという意味で、作業の続行を妨げない）
fn run_data_migration_stage() {
    // 発火点は**ここ 1 本**（#916）。引き継ぎ（#915）の分割移行も登録簿から
    // 呼ばれるので、種別が増えてもこの関数は変わらない
    let lines = tako_control::migrations::setup_lines();
    if lines.is_empty() {
        return;
    }
    eprintln!("設定ファイルの更新");
    eprintln!("──────────────────");
    for line in lines {
        eprintln!("  [migrated] {line}");
    }
    eprintln!();
}

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

    // 設定ファイルの自動マイグレーション（#915 / #916 の段 1）。
    // 依存チェックより**前**に置くのは、依存不足で早期 return するときも
    // データの形式だけは最新化しておきたいため。冪等なので毎回呼んでよい
    run_data_migration_stage();

    // ゼロスタート導入（#868）。導入済みなら何も出さずに素通りする＝従来の検出型と同じ体験。
    // 未導入なら インストール → PATH 通し → 認証 まで案内してから検出型へ進む
    run_bootstrap_stage(assume_yes)?;

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
    // codex / agy も導入済みなら登録対象（#979）。既に登録済みなら plan へ出さない
    for agent in &agents {
        let kind = match agent.kind {
            SetupAgent::Codex => tako_control::orchestrator::agent::WorkerAgent::Codex,
            SetupAgent::Agy => tako_control::orchestrator::agent::WorkerAgent::Agy,
            SetupAgent::Claude => continue,
        };
        let registered = tako_control::agent_mcp::current_registration(kind);
        if tako_control::agent_mcp::registration_is_alive(registered.as_deref()) {
            continue;
        }
        plan.push_if_changed(
            format!("{} MCP", kind.as_str()),
            Some(if registered.is_some() {
                "登録パス消失"
            } else {
                "未登録"
            }),
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
    configure_agent_mcp(&agents)?;
    let instruction_coverage = apply_instruction(selected, answers.instruction_content.as_deref())?;
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
        // プラットフォーム事実の注入（#516）。正本は 1 本で、差分は書き出し時に入れる
        write_resource(
            &dir,
            filename,
            &tako_control::platform::facts::render_current(SYSTEM_PROMPT),
        )?;
    }
    sync_pending_changes_file(&dir, &pending, config.setup.applied_revision)?;

    let profile_note = prepare_profile(selected, &agents, &plans, answers.profile.as_ref())?;
    apply_projects(answers.projects.as_ref())?;
    // 設定共有の現状（#793）。読み取りだけで副作用は無い。
    // 表示（サマリ / --check）と対話アシスタントへの引き渡しで同じ検出結果を使う
    let share_env = tako_control::config_share::env::detect();
    write_setup_context(
        &dir,
        selected,
        &agents,
        &plans,
        profile_note,
        instruction_coverage.as_ref(),
        &share_env,
    )?;

    let revision = mark_setup_complete(selected, &plans, answers.orchestrator.as_ref())?;
    sync_pending_changes_file(&dir, &[], revision)?;
    print_setup_summary(&plan);
    eprintln!("セットアップが完了しました。");
    eprintln!();
    eprintln!("スマホからリモート接続するには: tako remote setup");

    // --- 対話エージェント起動（Issue #295 / #322 / #391）---
    // 既定: 検出フロー完了後に setup agent を対話起動し、設定変更・解説・次の一歩を対話で行う。
    // スキップ条件: --yes / 非 TTY / --answers launch_agent=none
    let skip_agent = assume_yes
        || !std::io::IsTerminal::is_terminal(&std::io::stdin())
        || answers.launch_agent.as_deref().is_some_and(|v| v == "none");

    // 設定共有（#513 / #793）。標準経路は状態表示だけ = 質問を増やさない（#262）
    apply_config_share(
        review_mode,
        assume_yes,
        answers.config_share.as_ref(),
        &share_env,
        !skip_agent,
    )?;

    if !skip_agent {
        if review_mode && instruction_existed {
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
        eprintln!("対話アシスタントを起動します。設定の変更・解説・次の一歩の相談ができます。");
        eprintln!("─────────────────────────────────────────────────────");
        let greeting = if review_mode {
            if pending.is_empty() {
                "最初に setup-instructions.md を読んでください。前回設定の個別見直しを始めます。変更したい項目だけ確認してください。"
            } else {
                "最初に setup-instructions.md と pending-changes.md を読んでください。アップデート変更と前回設定の個別見直しを始めます。"
            }
        } else if is_first_run {
            "最初に setup-instructions.md を読んでください。セットアップが完了しました。設定の確認・変更、コマンドの使い方、次に何をすればよいかなど、何でも聞いてください。"
        } else if !pending.is_empty() {
            "最初に setup-instructions.md と pending-changes.md を読んでください。アップデート変更があります。確認と設定の調整ができます。何でも聞いてください。"
        } else {
            "最初に setup-instructions.md を読んでください。設定の確認・変更、コマンドの使い方、次に何をすればよいかなど、何でも聞いてください。"
        };
        let status = launch_setup_agent(selected_agent, &dir, greeting)?;
        if !status.success() {
            eprintln!(
                "{} が終了しました（exit code: {}）",
                selected.as_str(),
                status.code().unwrap_or(-1)
            );
        }
    } else {
        let master_ready = agents
            .iter()
            .any(|agent| agent.authenticated && agent.kind.supports_master());
        print_next_steps(master_ready);
    }

    Ok(())
}

fn find_backup_path(dir: &Path, filename: &str) -> PathBuf {
    let today = {
        // #586: 対話でない子なのでコンソールウィンドウを出させない
        let output = tako_core::platform::process::no_console_window(
            &mut std::process::Command::new("date"),
        )
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
    // #586: 対話でない子なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("date"))
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
        assert_eq!(RECOMMENDED_SECTIONS.len(), 7);
        for (rel_path, content) in RECOMMENDED_SECTIONS {
            assert!(!content.is_empty(), "{rel_path} が空");
        }
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

    /// **#513 受け入れ条件 3**: 設定共有はオプションであり、
    /// 標準 setup（`--review` なし）では質問が 1 つも増えないこと
    mod config_share_step {
        use super::super::{decide_config_share_step, ConfigShareStep};
        use tako_control::setup::SetupConfigShareAnswers;

        fn decide(
            review: bool,
            assume_yes: bool,
            is_tty: bool,
            linked: bool,
            answers: Option<&SetupConfigShareAnswers>,
        ) -> ConfigShareStep {
            decide_config_share_step(review, assume_yes, is_tty, linked, answers)
        }

        #[test]
        fn 標準setupは対話端末でも質問しない() {
            // review=false = 標準 setup。TTY があっても Ask にならない（#262 質問ゼロ）
            assert_eq!(
                decide(false, false, true, false, None),
                ConfigShareStep::Info
            );
        }

        #[test]
        fn yesと非対話も質問しない() {
            assert_eq!(decide(true, true, true, false, None), ConfigShareStep::Info);
            assert_eq!(
                decide(true, false, false, false, None),
                ConfigShareStep::Info
            );
        }

        #[test]
        fn reviewの対話でだけ聞く() {
            assert_eq!(decide(true, false, true, false, None), ConfigShareStep::Ask);
        }

        #[test]
        fn 明示回答は非対話で配線する() {
            let answers = SetupConfigShareAnswers {
                enable: Some(true),
                repo: Some("~/tako-config-sync".into()),
                ..Default::default()
            };
            assert_eq!(
                decide(false, true, false, false, Some(&answers)),
                ConfigShareStep::Link {
                    repo: Some("~/tako-config-sync".into()),
                    path: None,
                    remote: None,
                }
            );
        }

        #[test]
        fn 明示的に無効なら何もしない() {
            let answers = SetupConfigShareAnswers {
                enable: Some(false),
                ..Default::default()
            };
            assert_eq!(
                decide(true, false, true, false, Some(&answers)),
                ConfigShareStep::Skip
            );
            // enable 省略も「触らない」（既定で有効化しない）
            let empty = SetupConfigShareAnswers::default();
            assert_eq!(
                decide(true, false, true, false, Some(&empty)),
                ConfigShareStep::Skip
            );
        }

        #[test]
        fn 配線済みなら聞き直さない() {
            assert_eq!(
                decide(true, false, true, true, None),
                ConfigShareStep::AlreadyLinked
            );
        }
    }

    /// 表示（setup サマリ / `--check`）の文言（Issue #793）。
    /// 判定は `config_share::env` の純粋関数、ここで見るのは「何をどう見せるか」
    mod config_share_notice {
        use super::super::config_share_lines;
        use tako_control::config_share::env::{
            ExternalKind, ExternalManaged, GhStatus, ShareEnvironment,
        };

        fn env(linked: bool, external: Vec<ExternalManaged>) -> ShareEnvironment {
            ShareEnvironment {
                linked,
                repo: linked.then(|| "~/tako-config-sync".to_string()),
                repo_ok: linked,
                external,
                gh: (!linked).then_some(GhStatus::Authenticated),
            }
        }

        fn dotfiles(repo_rel: &str) -> ExternalManaged {
            ExternalManaged {
                root: "claude",
                path: "~/.claude".into(),
                kind: ExternalKind::Symlink,
                repo: "~/dotfiles".into(),
                same_place: ExternalManaged::shares_place("claude", repo_rel),
                repo_rel: repo_rel.into(),
            }
        }

        fn joined(env: &ShareEnvironment, verbose: bool) -> String {
            config_share_lines(env, verbose).join("\n")
        }

        #[test]
        fn 表示に質問は含まれない() {
            for verbose in [true, false] {
                for env in [
                    env(false, vec![]),
                    env(false, vec![dotfiles("claude")]),
                    env(true, vec![]),
                ] {
                    let text = joined(&env, verbose);
                    assert!(
                        !text.contains("[y/N]") && !text.contains("しますか"),
                        "標準 setup の表示で質問してはいけない（#262）: {text}"
                    );
                }
            }
        }

        #[test]
        fn 配線済みなら勧誘文言を出さない() {
            let text = joined(&env(true, vec![dotfiles("claude")]), true);
            assert!(text.contains("配線済み"), "{text}");
            assert!(
                !text.contains("tako config init") && !text.contains("tako config link"),
                "配線済みで新規配線を勧めない（#793 受け入れ条件 4）: {text}"
            );
        }

        #[test]
        fn 未配線かつ既存運用なしなら新規作成を案内する() {
            let text = joined(&env(false, vec![]), true);
            assert!(text.contains("未配線"), "{text}");
            assert!(text.contains("tako config init"), "{text}");
        }

        #[test]
        fn 既存の外部管理を検出したら相乗りを先に出す() {
            let text = joined(&env(false, vec![dotfiles("claude")]), true);
            assert!(
                text.contains("~/.claude は既に ~/dotfiles で管理"),
                "{text}"
            );
            assert!(
                text.contains("tako config link ~/dotfiles"),
                "相乗り先を最簡形で示す（#322）: {text}"
            );
            assert!(
                text.contains("二重管理"),
                "別リポジトリを作る危険を明示する（#793 受け入れ条件 3）: {text}"
            );
            assert!(
                !text.contains("tako config init"),
                "既存運用があるのに新規作成を第一案にしない: {text}"
            );
        }

        #[test]
        fn 置き場が違えば重複を明示する() {
            let same = joined(&env(false, vec![dotfiles("claude")]), false);
            assert!(same.contains("tako の置き場と一致"), "{same}");
            let differs = joined(&env(false, vec![dotfiles("home/.claude")]), false);
            assert!(differs.contains("tako の置き場とは別"), "{differs}");
        }

        #[test]
        fn checkは説明行を足さない() {
            let check = config_share_lines(&env(false, vec![]), false);
            let summary = config_share_lines(&env(false, vec![]), true);
            assert_eq!(check.len(), 1, "--check は 1 行: {check:?}");
            assert!(summary.len() > check.len(), "サマリでは説明を足す");
        }
    }
}

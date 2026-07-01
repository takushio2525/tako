//! `tako setup` — 対話式セットアップコマンド。
//!
//! claude コマンドの存在確認 → MCP 登録確認 → リソースファイル書き出し →
//! config.yaml の初回/2回目判定 → claude を setup cwd で起動する。
//! IPC 不要（tako アプリ未起動でも動作）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
const CONFIG_DEFAULT: &str = include_str!("../../../resources/setup/templates/config-default.yaml");

// --- config.yaml のスキーマ ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupConfig {
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub setup: SetupState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    #[serde(default = "default_model")]
    pub master_model: String,
    #[serde(default = "default_model")]
    pub worker_model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default = "default_true")]
    pub auto_close: bool,
    #[serde(default = "default_true")]
    pub auto_push: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupState {
    #[serde(default)]
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

fn default_model() -> String {
    "claude-opus-4-6[1m]".into()
}
fn default_effort() -> String {
    "max".into()
}
fn default_true() -> bool {
    true
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            master_model: default_model(),
            worker_model: default_model(),
            effort: default_effort(),
            auto_close: true,
            auto_push: true,
        }
    }
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

fn config_yaml_path() -> Result<PathBuf, String> {
    home_dir()
        .map(|h| h.join("Library/Application Support/tako/orchestrator/config.yaml"))
        .ok_or_else(|| "ホームディレクトリが取得できない（$HOME 未設定）".into())
}

// --- config.yaml の読み書き ---

fn load_config() -> Result<SetupConfig, String> {
    let path = config_yaml_path()?;
    if !path.is_file() {
        return Ok(SetupConfig::default());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("config.yaml の読み取りに失敗: {e}"))?;
    serde_yaml::from_str(&content).map_err(|e| format!("config.yaml のパースに失敗: {e}"))
}

fn save_config(config: &SetupConfig) -> Result<(), String> {
    let path = config_yaml_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    }
    let content =
        serde_yaml::to_string(config).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("config.yaml の書き込みに失敗: {e}"))
}

// --- 環境チェック ---

fn find_claude() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "which claude"])
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

fn check_mcp_registered() -> bool {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "claude mcp list 2>/dev/null"])
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
    write_resource(setup_dir, "templates/config-default.yaml", CONFIG_DEFAULT)?;
    Ok(())
}

// --- メインエントリ ---

/// `tako setup --check` — 環境チェックだけ実行して終了
pub fn run_check() -> Result<(), String> {
    eprintln!("tako セットアップ 環境チェック");
    eprintln!("─────────────────────────────");

    // claude コマンド
    match find_claude() {
        Some(path) => eprintln!("  ✓ claude: {path}"),
        None => eprintln!(
            "  ✗ claude: 見つかりません（https://docs.anthropic.com/en/docs/claude-code）"
        ),
    }

    // MCP 登録
    if check_mcp_registered() {
        eprintln!("  ✓ MCP: tako が登録済み");
    } else {
        eprintln!("  ✗ MCP: tako が未登録（tako setup-mcp で登録できます）");
    }

    // config.yaml
    let config_path = config_yaml_path()?;
    if config_path.is_file() {
        let config = load_config()?;
        if config.setup.completed {
            eprintln!(
                "  ✓ セットアップ: 完了済み ({})",
                config.setup.completed_at.as_deref().unwrap_or("日時不明")
            );
        } else {
            eprintln!("  △ セットアップ: 未完了");
        }
    } else {
        eprintln!("  △ config.yaml: 未作成");
    }

    // ~/.claude/CLAUDE.md
    if let Some(home) = home_dir() {
        let claude_md = home.join(".claude/CLAUDE.md");
        if claude_md.is_file() {
            eprintln!("  ✓ ~/.claude/CLAUDE.md: 存在します");
        } else {
            eprintln!("  △ ~/.claude/CLAUDE.md: 未作成");
        }
    }

    Ok(())
}

/// `tako setup --reset` — config.yaml の setup.completed を false にリセット
pub fn run_reset() -> Result<(), String> {
    let mut config = load_config()?;
    config.setup.completed = false;
    config.setup.completed_at = None;
    save_config(&config)?;
    eprintln!("セットアップ状態をリセットしました。tako setup で再実行できます");
    Ok(())
}

/// `tako setup` — メインのセットアップフロー
pub fn run_setup() -> Result<(), String> {
    eprintln!("tako セットアップ");
    eprintln!("═════════════════");
    eprintln!();

    // 1. claude コマンドの存在確認
    let claude_path = find_claude().ok_or(
        "claude コマンドが見つかりません。\n\
         Claude Code をインストールしてください:\n  \
         https://docs.anthropic.com/en/docs/claude-code",
    )?;
    eprintln!("  ✓ claude: {claude_path}");

    // 2. MCP 登録確認
    if !check_mcp_registered() {
        eprintln!("  △ MCP 未登録 → 自動登録します...");
        run_setup_mcp()?;
    } else {
        eprintln!("  ✓ MCP: tako が登録済み");
    }

    // 3. setup ディレクトリ + リソース書き出し
    let dir = setup_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    write_all_resources(&dir)?;
    eprintln!("  ✓ テンプレートを展開: {}", dir.display());

    // CLAUDE.md（setup 用 system prompt）を書き出す
    let claude_md_path = dir.join("CLAUDE.md");
    std::fs::write(&claude_md_path, SYSTEM_PROMPT)
        .map_err(|e| format!("CLAUDE.md の書き出しに失敗: {e}"))?;

    // 4. config.yaml の初回 / 2 回目判定
    let mut config = load_config()?;
    let is_first_run = !config.setup.completed;

    // 5. claude を setup cwd で起動
    eprintln!();
    if is_first_run {
        eprintln!("初回セットアップを開始します。claude が対話で設定をガイドします。");
    } else {
        eprintln!("セットアップメニューを開きます。");
    }
    eprintln!("─────────────────────────────────────────────────────");
    eprintln!();

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());

    let greeting = if is_first_run {
        "tako のセットアップを始めます。いくつか質問に答えてください。"
    } else {
        "tako の設定を変更します。何をしますか？"
    };

    let claude_cmd = format!(
        "cd '{}' && claude --model '{}' --effort '{}' '{}'",
        dir.display(),
        config.orchestrator.master_model.replace('\'', "'\\''"),
        config.orchestrator.effort.replace('\'', "'\\''"),
        greeting.replace('\'', "'\\''"),
    );

    let status = std::process::Command::new(&shell)
        .args(["-l", "-c", &claude_cmd])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("claude の起動に失敗: {e}"))?;

    if status.success() {
        // セットアップ完了を記録
        config.setup.completed = true;
        config.setup.completed_at = Some(now_iso8601());
        save_config(&config)?;
        eprintln!();
        eprintln!("セットアップが完了しました。");
    } else {
        eprintln!();
        eprintln!(
            "claude が終了しました（exit code: {}）",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
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

    #[test]
    fn config_roundtrip() {
        let config = SetupConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: SetupConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.orchestrator.master_model, "claude-opus-4-6[1m]");
        assert!(!back.setup.completed);
    }

    #[test]
    fn config_from_default_yaml() {
        let config: SetupConfig = serde_yaml::from_str(CONFIG_DEFAULT).unwrap();
        assert_eq!(config.orchestrator.effort, "max");
        assert!(!config.setup.completed);
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
        assert!(!CONFIG_DEFAULT.is_empty());
    }
}

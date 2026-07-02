//! orchestrator — マスターオーケストレーター機能（projects.yaml 管理 + worker spawn/watch）
//!
//! `tako master` で claude のマスターエージェントを起動し、MCP ツール / CLI から
//! 子 worker の spawn・監視・プロジェクト管理を行う。外部スクリプト依存ゼロで
//! tako をインストールするだけで使える。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// バイナリ埋め込みのデフォルト system prompt
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("default_system_prompt.md");

/// オーケストレーター設定ディレクトリのパス。
/// `~/Library/Application Support/tako/orchestrator/`
pub fn config_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library/Application Support/tako/orchestrator"))
}

/// projects.yaml のパス
pub fn projects_yaml_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("projects.yaml"))
}

/// ユーザーカスタム system prompt のパス
pub fn custom_system_prompt_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("master-system.md"))
}

/// system prompt ファイルのパスを解決する。カスタムファイルがあればそれ、なければ None
pub fn resolve_system_prompt_path() -> Option<PathBuf> {
    let custom = custom_system_prompt_path()?;
    if custom.is_file() {
        Some(custom)
    } else {
        None
    }
}

/// `~` を `$HOME` に展開する
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    path.to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

// --- projects.yaml ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectsConfig {
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// projects.yaml を解決済みの絶対パスで返すエントリ
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedProject {
    pub key: String,
    pub cwd: String,
    pub description: Option<String>,
}

impl ProjectsConfig {
    pub fn load() -> Result<Self, String> {
        let path = projects_yaml_path().ok_or("ホームディレクトリが取得できない")?;
        if !path.is_file() {
            return Ok(ProjectsConfig {
                projects: std::collections::BTreeMap::new(),
            });
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("projects.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("projects.yaml のパースに失敗: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = projects_yaml_path().ok_or("ホームディレクトリが取得できない")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
        }
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        std::fs::write(&path, content).map_err(|e| format!("projects.yaml の書き込みに失敗: {e}"))
    }

    pub fn list_resolved(&self) -> Vec<ResolvedProject> {
        self.projects
            .iter()
            .map(|(key, entry)| ResolvedProject {
                key: key.clone(),
                cwd: expand_tilde(&entry.cwd),
                description: entry.description.clone(),
            })
            .collect()
    }

    pub fn resolve_cwd(&self, project: &str) -> Result<String, String> {
        let entry = self
            .projects
            .get(project)
            .ok_or_else(|| format!("プロジェクト '{project}' が projects.yaml に見つからない"))?;
        let cwd = expand_tilde(&entry.cwd);
        if !Path::new(&cwd).is_dir() {
            return Err(format!("cwd が存在しない: {cwd}"));
        }
        Ok(cwd)
    }

    pub fn add(&mut self, key: String, cwd: String, description: Option<String>) {
        self.projects.insert(key, ProjectEntry { cwd, description });
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.projects.remove(key).is_some()
    }
}

// --- プロファイル ---

/// プロファイルの保存ディレクトリ
/// `~/Library/Application Support/tako/orchestrator/profiles/`
pub fn profiles_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("profiles"))
}

/// プロファイル設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "default_profile_model")]
    pub model: String,
    #[serde(default = "default_profile_effort")]
    pub effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
}

fn default_profile_model() -> String {
    "claude-opus-4-6[1m]".into()
}
fn default_profile_effort() -> String {
    "max".into()
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            model: default_profile_model(),
            effort: default_profile_effort(),
            system_prompt: None,
            projects: None,
        }
    }
}

impl Profile {
    /// プロファイルを YAML ファイルから読み込む
    pub fn load(name: &str) -> Result<Self, String> {
        let path = profile_path(name)?;
        if !path.is_file() {
            return Err(format!(
                "プロファイル '{name}' が見つからない: {}",
                path.display()
            ));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("プロファイルの読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("プロファイルのパースに失敗: {e}"))
    }

    /// プロファイルを YAML ファイルに保存する
    pub fn save(&self, name: &str) -> Result<PathBuf, String> {
        let path = profile_path(name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
        }
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        std::fs::write(&path, &content)
            .map_err(|e| format!("プロファイルの書き込みに失敗: {e}"))?;
        Ok(path)
    }

    /// system prompt のパスを解決する。
    /// profile.system_prompt が指定されていればその絶対パス、
    /// なければカスタム master-system.md → デフォルト埋め込みの順
    pub fn resolve_system_prompt(&self) -> Option<PathBuf> {
        if let Some(ref custom) = self.system_prompt {
            let expanded = expand_tilde(custom);
            let p = PathBuf::from(&expanded);
            if p.is_file() {
                return Some(p);
            }
        }
        resolve_system_prompt_path()
    }
}

/// プロファイルのファイルパスを返す
fn profile_path(name: &str) -> Result<PathBuf, String> {
    profiles_dir()
        .map(|d| d.join(format!("{name}.yaml")))
        .ok_or_else(|| "ホームディレクトリが取得できない".into())
}

/// 利用可能なプロファイル名の一覧を返す
pub fn list_profiles() -> Result<Vec<String>, String> {
    let dir = match profiles_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Ok(vec![]),
    };
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| format!("プロファイルディレクトリの読み取りに失敗: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// 初回実行時にデフォルトのディレクトリとファイルを生成する
pub fn ensure_defaults() -> Result<PathBuf, String> {
    let dir = config_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    let yaml_path = dir.join("projects.yaml");
    if !yaml_path.is_file() {
        let template = ProjectsConfig {
            projects: std::collections::BTreeMap::new(),
        };
        template.save()?;
    }
    // デフォルトプロファイルが無ければ作成
    let profiles = profiles_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&profiles)
        .map_err(|e| format!("profiles ディレクトリの作成に失敗: {e}"))?;
    let default_profile = profiles.join("default.yaml");
    if !default_profile.is_file() {
        Profile::default().save("default")?;
    }
    Ok(dir)
}

/// `claude agents --json` をログインシェル経由で実行する。
/// .app バンドル（Dock 起動）の PATH は最小構成で `claude` が見つからないため、
/// `$SHELL -l -c "claude agents --json"` でユーザーの PATH を使う
fn run_claude_agents_json() -> Option<Vec<u8>> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "claude agents --json"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

/// `claude agents --json` から指定 session_id の status と ctx% を取得する
pub fn query_agent_status(session_id: &str) -> AgentStatus {
    let Some(stdout) = run_claude_agents_json() else {
        return AgentStatus {
            status: "unknown".into(),
            ctx_percent: None,
        };
    };
    let Ok(json_str) = String::from_utf8(stdout) else {
        return AgentStatus {
            status: "unknown".into(),
            ctx_percent: None,
        };
    };
    let Ok(agents) = serde_json::from_str::<serde_json::Value>(&json_str) else {
        return AgentStatus {
            status: "unknown".into(),
            ctx_percent: None,
        };
    };
    let Some(agents) = agents.as_array() else {
        return AgentStatus {
            status: "unknown".into(),
            ctx_percent: None,
        };
    };
    match agents
        .iter()
        .find(|a| a["sessionId"].as_str() == Some(session_id))
    {
        None => AgentStatus {
            status: "gone".into(),
            ctx_percent: None,
        },
        Some(agent) => {
            let status = agent["status"].as_str().unwrap_or("unknown").to_string();
            let ctx_percent = agent["contextPercentUsed"].as_f64().map(|v| v as u32);
            AgentStatus {
                status,
                ctx_percent,
            }
        }
    }
}

#[derive(Debug)]
pub struct AgentStatus {
    pub status: String,
    pub ctx_percent: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion() {
        let expanded = expand_tilde("~/Documents/test");
        assert!(!expanded.starts_with("~/"));
        assert!(expanded.contains("Documents/test"));
    }

    #[test]
    fn projects_config_roundtrip() {
        let mut config = ProjectsConfig {
            projects: std::collections::BTreeMap::new(),
        };
        config.add("demo".into(), "~/my-project".into(), Some("テスト".into()));
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: ProjectsConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.projects.len(), 1);
        assert_eq!(back.projects["demo"].cwd, "~/my-project");
    }

    #[test]
    fn profile_default_values() {
        let p = Profile::default();
        assert_eq!(p.model, "claude-opus-4-6[1m]");
        assert_eq!(p.effort, "max");
        assert!(p.system_prompt.is_none());
        assert!(p.projects.is_none());
    }

    #[test]
    fn profile_roundtrip() {
        let p = Profile {
            model: "claude-sonnet-5".into(),
            effort: "high".into(),
            system_prompt: Some("~/my-prompt.md".into()),
            projects: Some(vec!["tako".into(), "demo".into()]),
        };
        let yaml = serde_yaml::to_string(&p).unwrap();
        let back: Profile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.model, "claude-sonnet-5");
        assert_eq!(back.effort, "high");
        assert_eq!(back.system_prompt.as_deref(), Some("~/my-prompt.md"));
        assert_eq!(back.projects.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn profile_deserialize_minimal() {
        let yaml = "model: claude-opus-4-6[1m]\n";
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.effort, "max");
        assert!(p.projects.is_none());
    }

    #[test]
    fn profile_save_load_roundtrip() {
        let tmp = std::env::temp_dir().join("tako-test-profiles");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let name = "test-roundtrip";
        let path = tmp.join(format!("{name}.yaml"));
        let p = Profile {
            model: "test-model".into(),
            effort: "low".into(),
            system_prompt: None,
            projects: Some(vec!["a".into()]),
        };
        let yaml = serde_yaml::to_string(&p).unwrap();
        std::fs::write(&path, &yaml).unwrap();
        let loaded: Profile =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.projects.as_ref().unwrap(), &["a"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

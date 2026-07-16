//! orchestrator — マスターオーケストレーター機能（projects.yaml 管理 + worker spawn/watch）
//!
//! `tako master` で claude のマスターエージェントを起動し、MCP ツール / CLI から
//! 子 worker の spawn・監視・プロジェクト管理を行う。外部スクリプト依存ゼロで
//! tako をインストールするだけで使える。

pub mod agent;
pub mod ledger;
pub mod wait;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use agent::WorkerAgent;

/// バイナリ埋め込みのデフォルト system prompt（master 用）
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("default_system_prompt.md");

/// バイナリ埋め込みの solo system prompt
pub const SOLO_SYSTEM_PROMPT: &str = include_str!("solo_system_prompt.md");

/// solo のデフォルト effort。master の "max" より低くしてエコ運用を既定にする
pub const SOLO_DEFAULT_EFFORT: &str = "high";

/// テスト専用: config_dir() の返り先を隔離ディレクトリへ差し替える。
/// テストが実運用の projects.yaml / profiles / config.yaml に書き込み、
/// 世代バックアップをテスト由来の内容で汚染するのを防ぐ（#169）
#[cfg(test)]
pub(crate) fn test_config_dir_override() -> &'static std::sync::OnceLock<PathBuf> {
    static OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    &OVERRIDE
}

/// オーケストレーター設定ディレクトリのパス。
/// `~/Library/Application Support/tako/orchestrator/`
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = test_config_dir_override().get() {
        return Some(dir.clone());
    }
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

/// handoff ファイルのパス（`<config_dir>/handoff/<profile>.md`）
pub fn handoff_path(profile: &str) -> Option<PathBuf> {
    config_dir().map(|d| d.join("handoff").join(format!("{profile}.md")))
}

/// handoff ファイルの内容を読む。不在なら None
pub fn read_handoff(profile: &str) -> Option<String> {
    let path = handoff_path(profile)?;
    std::fs::read_to_string(&path)
        .ok()
        .filter(|s| !s.trim().is_empty())
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

pub(crate) fn home_dir() -> Option<PathBuf> {
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
        Self::load_from(&path)
    }

    /// パス指定版 load（テスト用に公開）。
    /// ファイル不在は「初期状態」として空を返す。読み取り / パースの失敗は
    /// **0 件に丸めず Err を返す**（#169: 失敗を空として扱うと後続の save が全件を消す）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(ProjectsConfig {
                projects: std::collections::BTreeMap::new(),
            });
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("projects.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("projects.yaml のパースに失敗: {e}"))
    }

    /// 保存（アトミック書き込み + 世代バックアップ。#169）。
    /// 注意: `load()` → 変更 → `save()` の素朴な組み合わせは、間に割り込んだ
    /// 他プロセスの変更を巻き戻す。add / remove は必ず [`Self::mutate`] を使うこと
    pub fn save(&self) -> Result<(), String> {
        let path = projects_yaml_path().ok_or("ホームディレクトリが取得できない")?;
        self.save_to(&path)
    }

    /// パス指定版 save（テスト用に公開）
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)
    }

    /// ロック付き read-modify-write（#169 の再発防止本体）。
    /// `projects.yaml.lock` の排他ロック下で load → f → save を行い、
    /// 複数プロセス（GUI の MCP dispatch / CLI / 複数 master）の並行 add / remove でも
    /// 更新が失われない。既存ファイルのパースに失敗した場合は **f を呼ばず、
    /// 一切書き込まずに** Err を返す
    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = projects_yaml_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    /// パス指定版 mutate（テスト用に公開）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut config = Self::load_from(path)?;
        let result = f(&mut config);
        config.save_to(path)?;
        Ok(result)
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

/// 子 worker のモデル決定ポリシー
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerModelPolicy {
    /// master 自身の model/effort を子にも使う
    #[default]
    Inherit,
    /// worker_model / worker_effort で指定した値に全子統一
    Fixed,
    /// master がタスク内容を見て子ごとに model/effort を判断
    Delegate,
}

/// system prompt のブロック単位制御
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptBlocks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub override_blocks: std::collections::BTreeMap<String, String>,
}

/// 0.2.3 以前が default.yaml に書き込んでいた旧既定モデル。
/// Pro プランでは 1M コンテキスト版が使えず master が起動不能になるため、
/// この値のままのファイルは起動時に自動マイグレーションする（Issue #27）
pub const LEGACY_DEFAULT_MODEL: &str = "claude-opus-4-6[1m]";

/// model 未指定時の表示ラベル（起動コマンドには --model 自体を付けない）
pub const CLAUDE_DEFAULT_LABEL: &str = "(claude CLI default)";

/// エージェント別の worker 設定（`worker_agents.<agent>` の値。Issue #120）。
/// model / effort はそのエージェント CLI のネイティブ表記
/// （codex: `gpt-5.6-terra` 等、agy: `Gemini 3.5 Flash (High)` 等の表示名）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentWorkerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// claude: `--effort` / codex: `-c model_reasoning_effort=` / agy: 無視される
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// 許可プロンプトのスキップ。codex / agy は `WorkerAgent::default_skip_permissions()`
    /// が true のためプロファイル未設定でも承認なしで起動する。明示的に false を設定すると
    /// 承認ありに戻る。claude は既定 false（Claude Code 側の設定に委ねる）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_permissions: bool,
    /// 追加 CLI 引数（上級者向け。例: codex の `--search`）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// `Profile::resolve_agent_launch` の解決結果（spawn で使う worker 起動パラメータ）
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWorkerLaunch {
    pub agent: WorkerAgent,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub skip_permissions: bool,
    pub extra_args: Vec<String>,
}

/// プロファイル設定。
/// `model` が `None` の場合は claude CLI の既定モデルに委ねる（`--model` を付けない）。
/// 1M コンテキスト版（`[1m]` サフィックス）は Max / API プラン限定のため、
/// ユーザーがプロファイルに明示した場合のみ使われる（既定にはしない）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// master のエージェント種別（claude / codex。省略時 claude = 完全後方互換。Issue #127）。
    /// String で保持し起動・設定時に検証する。agy は MCP のペイン毎接続情報（TAKO_*）を
    /// 渡す手段と system prompt 注入手段が無いため master 非対応（worker のみ）。
    /// model / effort は master_agent のネイティブ表記で指定する
    /// （codex: `gpt-5.6-sol` / `none|minimal|low|medium|high|xhigh|max|ultra` 等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_profile_effort")]
    pub effort: String,

    #[serde(default)]
    pub worker_model_policy: WorkerModelPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate_guidance: Option<String>,

    /// worker の既定エージェント種別（claude / codex / agy。省略時 claude。Issue #120）。
    /// String で保持し spawn 時に検証する（不正値は診断つきエラーになる）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_agent: Option<String>,
    /// エージェント別の worker 設定。agent≠claude のモデル・effort・許可スキップ・
    /// 追加引数はここで指定する（claude はここに無ければ従来の worker_model_policy 解決）
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub worker_agents: std::collections::BTreeMap<String, AgentWorkerConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_blocks: Option<PromptBlocks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
}

/// master / claude worker の既定 effort
pub const DEFAULT_PROFILE_EFFORT: &str = "max";

fn default_profile_effort() -> String {
    DEFAULT_PROFILE_EFFORT.into()
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            master_agent: None,
            model: None,
            effort: default_profile_effort(),
            worker_model_policy: WorkerModelPolicy::default(),
            worker_model: None,
            worker_effort: None,
            delegate_guidance: None,
            worker_agent: None,
            worker_agents: std::collections::BTreeMap::new(),
            system_prompt: None,
            prompt_blocks: None,
            projects: None,
        }
    }
}

impl Profile {
    /// master のエージェント種別が claude か（未指定 = claude）。
    /// false のとき、profile.model / effort はそのエージェントのネイティブ表記なので
    /// claude worker への継承・claude 固有の警告（[1m] 等）から除外する（Issue #127）
    pub fn master_agent_is_claude(&self) -> bool {
        self.master_agent.as_deref().is_none_or(|a| a == "claude")
    }

    /// master のエージェント種別を解決する（プロファイル → claude 既定）。
    /// 不正・未対応の種別は起動前のエラーとして返す（Issue #127）
    pub fn resolve_master_agent(&self) -> Result<WorkerAgent, String> {
        match self.master_agent.as_deref() {
            None => Ok(WorkerAgent::Claude),
            Some(name) => validate_master_agent(name)
                .map_err(|e| format!("プロファイルの master_agent が不正: {e}")),
        }
    }

    /// worker_model_policy に従って子 worker の既定 model を解決する。
    /// `None` は claude CLI の既定モデルに委ねることを意味する。
    /// master_agent が claude 以外のとき、master の model はそのエージェントの
    /// ネイティブ表記なので claude worker へは継承しない（Issue #127）
    pub fn resolve_worker_model(&self) -> Option<&str> {
        let master_model_for_claude = if self.master_agent_is_claude() {
            self.model.as_deref()
        } else {
            None
        };
        match self.worker_model_policy {
            WorkerModelPolicy::Inherit | WorkerModelPolicy::Delegate => master_model_for_claude,
            WorkerModelPolicy::Fixed => self.worker_model.as_deref().or(master_model_for_claude),
        }
    }

    /// master のモデル表示ラベル（プロンプト・ログ用）
    pub fn model_label(&self) -> &str {
        self.model.as_deref().unwrap_or(CLAUDE_DEFAULT_LABEL)
    }

    /// master のモデル表示ラベル（エージェント種別対応版）。
    /// model 未指定時は master_agent の CLI 既定であることを示す
    pub fn master_model_label(&self) -> String {
        match self.model.as_deref() {
            Some(m) => m.to_string(),
            None => match self.master_agent.as_deref() {
                None | Some("claude") => CLAUDE_DEFAULT_LABEL.to_string(),
                Some(agent) => format!("({agent} CLI default)"),
            },
        }
    }

    /// 子 worker のモデル表示ラベル（プロンプト・ログ用）
    pub fn worker_model_label(&self) -> &str {
        self.resolve_worker_model().unwrap_or(CLAUDE_DEFAULT_LABEL)
    }

    /// worker_model_policy に従って子 worker の既定 effort を解決する。
    /// master_agent が claude 以外のときは master の effort を継承せず
    /// claude worker の既定（max）へフォールバックする（Issue #127）
    pub fn resolve_worker_effort(&self) -> &str {
        let master_effort_for_claude = if self.master_agent_is_claude() {
            self.effort.as_str()
        } else {
            DEFAULT_PROFILE_EFFORT
        };
        match self.worker_model_policy {
            WorkerModelPolicy::Inherit | WorkerModelPolicy::Delegate => master_effort_for_claude,
            WorkerModelPolicy::Fixed => self
                .worker_effort
                .as_deref()
                .unwrap_or(master_effort_for_claude),
        }
    }

    /// worker のエージェント種別を解決する（spawn の明示指定 → プロファイル既定 → claude）。
    /// 不正な種別名は spawn 時のエラーとして返す（Issue #120）
    pub fn resolve_worker_agent(&self, explicit: Option<&str>) -> Result<WorkerAgent, String> {
        let name = explicit
            .or(self.worker_agent.as_deref())
            .unwrap_or("claude");
        WorkerAgent::parse(name).map_err(|e| {
            if explicit.is_some() {
                e
            } else {
                format!("プロファイルの worker_agent が不正: {e}")
            }
        })
    }

    /// worker 起動パラメータ（モデル・effort・許可スキップ・追加引数）を解決する。
    /// - claude: 明示指定 → `worker_agents.claude` → 従来の worker_model_policy 解決
    ///   （effort は既定 "max" まで必ず埋まる = 従来挙動の維持）
    /// - codex / agy: 明示指定 → `worker_agents.<agent>` → CLI 既定（None のまま）
    pub fn resolve_agent_launch(
        &self,
        agent: WorkerAgent,
        explicit_model: Option<&str>,
        explicit_effort: Option<&str>,
    ) -> ResolvedWorkerLaunch {
        let cfg = self.worker_agents.get(agent.as_str());
        let cfg_model = cfg.and_then(|c| c.model.as_deref());
        let cfg_effort = cfg.and_then(|c| c.effort.as_deref());
        let (model, effort) = if agent == WorkerAgent::Claude {
            (
                explicit_model
                    .or(cfg_model)
                    .or_else(|| self.resolve_worker_model())
                    .map(str::to_string),
                Some(
                    explicit_effort
                        .or(cfg_effort)
                        .unwrap_or_else(|| self.resolve_worker_effort())
                        .to_string(),
                ),
            )
        } else {
            (
                explicit_model.or(cfg_model).map(str::to_string),
                explicit_effort.or(cfg_effort).map(str::to_string),
            )
        };
        ResolvedWorkerLaunch {
            agent,
            model,
            effort,
            skip_permissions: cfg
                .map(|c| c.skip_permissions)
                .unwrap_or_else(|| agent.default_skip_permissions()),
            extra_args: cfg.map(|c| c.args.clone()).unwrap_or_default(),
        }
    }

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

    /// プロファイルを YAML ファイルに保存する（アトミック書き込み + 世代バックアップ。#169）
    pub fn save(&self, name: &str) -> Result<PathBuf, String> {
        let path = profile_path(name)?;
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(&path, &content)?;
        Ok(path)
    }

    /// ロック付き read-modify-write（#169。profiles set 用）。
    /// ファイル不在は default から開始、**パースに失敗した既存ファイルは
    /// default に丸めず Err で中断する**（丸めて save すると設定が消えるため）
    pub fn mutate_named<R>(
        name: &str,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<(PathBuf, R), String> {
        let path = profile_path(name)?;
        let _lock = crate::config_io::lock_exclusive(&path)?;
        let mut profile = Self::load_from_or_default(&path)?;
        let result = f(&mut profile);
        let content = serde_yaml::to_string(&profile)
            .map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(&path, &content)?;
        Ok((path, result))
    }

    /// パス指定版 load。不在なら default、パース失敗は Err（テスト用に公開）
    pub fn load_from_or_default(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("プロファイルの読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("プロファイルのパースに失敗: {e}"))
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

    /// プロファイル設定に基づいて system prompt テキストを合成する。
    /// system_prompt フィールドが指定されている場合はそのファイルの内容をそのまま返す。
    /// そうでなければ DEFAULT_SYSTEM_PROMPT をブロック分割し、prompt_blocks と
    /// worker_model_policy に基づいて合成する。
    pub fn build_system_prompt(&self, profile_name: &str) -> String {
        // system_prompt フィールドが指定されていればファイルを丸ごと返す（既存互換）
        if let Some(ref custom) = self.system_prompt {
            let expanded = expand_tilde(custom);
            let p = std::path::PathBuf::from(&expanded);
            if p.is_file() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    return content;
                }
            }
        }
        // カスタム master-system.md があればそれを使う（ブロック制御はスキップ）
        if let Some(custom_path) = resolve_system_prompt_path() {
            if let Ok(content) = std::fs::read_to_string(&custom_path) {
                return content;
            }
        }

        self.build_from_template(DEFAULT_SYSTEM_PROMPT, profile_name)
    }

    /// テンプレートテキストからブロック制御・identity / model-policy 注入を行って合成する
    pub fn build_from_template(&self, template: &str, profile_name: &str) -> String {
        let blocks = parse_prompt_blocks(template);
        let pb = self.prompt_blocks.as_ref();

        let mut result = String::new();

        if let Some(text) = pb.and_then(|b| b.prepend.as_ref()) {
            result.push_str(&resolve_text_or_file(text));
            result.push_str("\n\n");
        }

        let mut identity_injected = false;

        for (name, content) in &blocks {
            // identity ブロックは disable 不可: role ブロックの直後に注入する
            if !identity_injected && name != "role" {
                result.push_str(&self.generate_identity_section(profile_name, &blocks, pb));
                result.push_str("\n\n");
                identity_injected = true;
            }

            if let Some(b) = pb {
                if b.disable.iter().any(|d| d == name) {
                    continue;
                }
            }

            if name == "model-policy" {
                result.push_str(&self.generate_model_policy_section());
                result.push('\n');
                continue;
            }

            if let Some(override_text) = pb.and_then(|b| b.override_blocks.get(name.as_str())) {
                result.push_str(&resolve_text_or_file(override_text));
                result.push('\n');
                continue;
            }

            result.push_str(content);
            result.push('\n');
        }

        // ブロックが role のみ or 空の場合のフォールバック
        if !identity_injected {
            result.push_str(&self.generate_identity_section(profile_name, &blocks, pb));
            result.push('\n');
        }

        if let Some(text) = pb.and_then(|b| b.append.as_ref()) {
            result.push_str(&resolve_text_or_file(text));
            result.push('\n');
        }

        result.trim_end().to_string()
    }

    /// worker_agent / worker_agents 設定に基づいて「利用可能な worker エージェント」の
    /// 説明テキストを生成する（model-policy セクションに追記。Issue #120）。
    /// どちらも未設定（= claude のみの従来運用）なら空文字列
    fn generate_worker_agents_section(&self) -> String {
        if self.worker_agent.is_none() && self.worker_agents.is_empty() {
            return String::new();
        }
        let default_agent = self.worker_agent.as_deref().unwrap_or("claude");
        let mut lines = vec![
            "\n### Available Worker Agents\n".to_string(),
            format!(
                "This profile can spawn workers on multiple agent CLIs. \
                 The default agent is **{default_agent}** (used when `agent` is omitted).\n"
            ),
        ];
        for agent in WorkerAgent::ALL {
            let name = agent.as_str();
            let cfg = self.worker_agents.get(name);
            let model = cfg
                .and_then(|c| c.model.as_deref())
                .map(|m| format!("model `{m}`"))
                .unwrap_or_else(|| {
                    if agent == WorkerAgent::Claude {
                        format!("model {}", self.worker_model_label())
                    } else {
                        "model (CLI default)".to_string()
                    }
                });
            let mut extras = Vec::new();
            if let Some(e) = cfg.and_then(|c| c.effort.as_deref()) {
                extras.push(format!("effort {e}"));
            }
            let effective_skip = cfg
                .map(|c| c.skip_permissions)
                .unwrap_or_else(|| agent.default_skip_permissions());
            if effective_skip {
                extras.push("skip_permissions".to_string());
            }
            let extras = if extras.is_empty() {
                String::new()
            } else {
                format!(", {}", extras.join(", "))
            };
            lines.push(format!("- `{name}`: {model}{extras}"));
        }
        lines.push(
            "\nPass `agent: \"codex\"` etc. to `tako_orchestrator_spawn` / `tako_orchestrator_run` \
             to pick the agent per task. `model` / `effort` given at spawn time are interpreted \
             in that agent's native vocabulary. codex / agy workers report status via screen \
             heuristics (no `claude agents` signal), so completion detection can take slightly \
             longer than claude workers."
                .to_string(),
        );
        lines.join("\n")
    }

    /// worker_model_policy に基づいて model-policy セクションのテキストを生成する。
    /// judgment 二層（雛形 + ローカル）を末尾に注入する（Issue #292）
    fn generate_model_policy_section(&self) -> String {
        let base = self.generate_model_policy_base();
        let agents = self.generate_worker_agents_section();
        let judgment = ledger::build_judgment_section();
        format!("{base}{agents}{judgment}")
    }

    fn generate_model_policy_base(&self) -> String {
        match self.worker_model_policy {
            WorkerModelPolicy::Inherit => {
                format!(
                    "## Worker Model Policy\n\n\
                     All workers use the same model and effort as this master session:\n\
                     - **Model**: {}\n\
                     - **Effort**: {}\n\n\
                     When calling `tako_orchestrator_spawn` or `tako_orchestrator_run`, do NOT specify\n\
                     `model` or `effort` — the defaults already match this session's configuration.",
                    self.model_label(),
                    self.effort
                )
            }
            WorkerModelPolicy::Fixed => {
                format!(
                    "## Worker Model Policy\n\n\
                     All workers use a fixed model/effort configuration:\n\
                     - **Model**: {}\n\
                     - **Effort**: {}\n\n\
                     When calling `tako_orchestrator_spawn` or `tako_orchestrator_run`, do NOT specify\n\
                     `model` or `effort` unless the user explicitly requests a different model for a\n\
                     specific task.",
                    self.worker_model_label(),
                    self.resolve_worker_effort()
                )
            }
            WorkerModelPolicy::Delegate => {
                let guidance = self
                    .delegate_guidance
                    .as_ref()
                    .map(|g| resolve_text_or_file(g))
                    .unwrap_or_else(|| "タスクの複雑さに応じて判断してください。".to_string());
                format!(
                    "## Worker Model Policy\n\n\
                     You decide the model and effort for each worker based on the task content.\n\
                     **Always** specify `model` and `effort` explicitly in `tako_orchestrator_spawn`\n\
                     and `tako_orchestrator_run` calls.\n\n\
                     If you cannot determine the appropriate model, use the default:\n\
                     - **Default Model**: {}\n\
                     - **Default Effort**: {}\n\n\
                     ### Delegation Guidance\n\n\
                     {guidance}",
                    self.model_label(),
                    self.effort
                )
            }
        }
    }

    /// master の自己認識ブロックを生成する（disable 不可、role 直後に注入）
    fn generate_identity_section(
        &self,
        profile_name: &str,
        blocks: &[(String, String)],
        pb: Option<&PromptBlocks>,
    ) -> String {
        let policy_str = match self.worker_model_policy {
            WorkerModelPolicy::Inherit if self.master_agent_is_claude() => {
                format!(
                    "inherit（master と同じ {} / {}）",
                    self.model_label(),
                    self.effort
                )
            }
            // master が claude 以外のとき master の model / effort は claude worker へ
            // 継承されない（Issue #127）。実際に解決される値を明示する
            WorkerModelPolicy::Inherit => format!(
                "inherit（master は {} のため claude worker へは非継承: {} / {}）",
                self.master_agent.as_deref().unwrap_or("claude"),
                self.worker_model_label(),
                self.resolve_worker_effort()
            ),
            WorkerModelPolicy::Fixed => format!(
                "fixed（{} / {}）",
                self.worker_model_label(),
                self.resolve_worker_effort()
            ),
            WorkerModelPolicy::Delegate => "delegate（master がタスクごとに判断）".into(),
        };

        let profile_path = profile_path(profile_name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(不明)".into());

        let mut customizations = Vec::new();
        if let Some(b) = pb {
            for d in &b.disable {
                customizations.push(format!("  - disabled: `{d}`"));
            }
            for k in b.override_blocks.keys() {
                customizations.push(format!("  - overridden: `{k}`"));
            }
            if b.prepend.is_some() {
                customizations.push("  - prepend: あり".into());
            }
            if b.append.is_some() {
                customizations.push("  - append: あり".into());
            }
        }
        let customization_summary = if customizations.is_empty() {
            "なし（共通テンプレートをそのまま使用）".into()
        } else {
            format!("\n{}", customizations.join("\n"))
        };

        let all_blocks: Vec<&str> = blocks.iter().map(|(n, _)| n.as_str()).collect();

        // worker のエージェント種別（claude / codex / agy）が設定されていれば明示（#120）
        let agent_line = if self.worker_agent.is_some() || !self.worker_agents.is_empty() {
            let configured: Vec<&str> = self.worker_agents.keys().map(String::as_str).collect();
            format!(
                "\n- **Worker agent**: {}（configured: {}）",
                self.worker_agent.as_deref().unwrap_or("claude"),
                if configured.is_empty() {
                    "-".to_string()
                } else {
                    configured.join(", ")
                }
            )
        } else {
            String::new()
        };

        // master のエージェント種別（設定時のみ明示。Issue #127）
        let master_agent_line = match self.master_agent.as_deref() {
            Some(agent) => format!("\n- **Master agent**: {agent}"),
            None => String::new(),
        };

        format!(
            "## Session Identity\n\n\
             - **Profile**: `{profile_name}`\n\
             - **Launch command**: `tako master -{profile_name}`{master_agent_line}\n\
             - **Master model**: {}\n\
             - **Master effort**: {}\n\
             - **Worker model policy**: {policy_str}{agent_line}\n\
             - **Profile config**: `{profile_path}`\n\
             - **Prompt blocks**: {}\n\
             - **Customizations**: {customization_summary}",
            self.master_model_label(),
            self.effort,
            all_blocks.join(", "),
        )
    }

    /// solo 用の system prompt を合成する
    pub fn build_solo_system_prompt(&self, profile_name: &str) -> String {
        let blocks = parse_prompt_blocks(SOLO_SYSTEM_PROMPT);
        let pb = self.prompt_blocks.as_ref();

        let mut result = String::new();

        if let Some(text) = pb.and_then(|b| b.prepend.as_ref()) {
            result.push_str(&resolve_text_or_file(text));
            result.push_str("\n\n");
        }

        for (name, content) in &blocks {
            if let Some(b) = pb {
                if b.disable.iter().any(|d| d == name) {
                    continue;
                }
            }

            if name == "model-policy" {
                result.push_str(&self.generate_solo_model_section(profile_name));
                result.push('\n');
                continue;
            }

            if let Some(override_text) = pb.and_then(|b| b.override_blocks.get(name.as_str())) {
                result.push_str(&resolve_text_or_file(override_text));
                result.push('\n');
                continue;
            }

            result.push_str(content);
            result.push('\n');
        }

        if let Some(text) = pb.and_then(|b| b.append.as_ref()) {
            result.push_str(&resolve_text_or_file(text));
            result.push('\n');
        }

        result.trim_end().to_string()
    }

    /// solo 用のモデル情報セクションを生成する
    fn generate_solo_model_section(&self, profile_name: &str) -> String {
        let profile_path = solo_profile_path(profile_name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(不明)".into());

        // solo でも master_agent 設定が効く（コマンド組み立てを master と共用。Issue #127）
        let agent_line = match self.master_agent.as_deref() {
            Some(agent) => format!("\n- **Agent**: {agent}"),
            None => String::new(),
        };

        format!(
            "## Session Identity\n\n\
             - **Mode**: solo (direct execution, no orchestration)\n\
             - **Profile**: `{profile_name}`\n\
             - **Launch command**: `tako solo -{profile_name}`{agent_line}\n\
             - **Model**: {}\n\
             - **Effort**: {}\n\
             - **Profile config**: `{profile_path}`",
            self.master_model_label(),
            self.effort,
        )
    }
}

// --- solo プロファイル ---

/// solo 用の新規生成する default プロファイル内容
const SOLO_DEFAULT_PROFILE_YAML: &str = "\
# tako solo のプロファイル設定
# model 未指定 = claude CLI の既定モデルで起動する（プラン非依存・推奨）
effort: high
";

/// solo 用のプロファイルディレクトリ
pub fn solo_profiles_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("solo-profiles"))
}

/// solo 用にデフォルトのディレクトリとプロファイルを生成する
pub fn ensure_solo_defaults() -> Result<PathBuf, String> {
    ensure_defaults()?;
    let dir = solo_profiles_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    let default_profile = dir.join("default.yaml");
    if !default_profile.is_file() {
        std::fs::write(&default_profile, SOLO_DEFAULT_PROFILE_YAML)
            .map_err(|e| format!("default.yaml の書き込みに失敗: {e}"))?;
    }
    Ok(dir)
}

/// solo 用のプロファイルパスを返す
fn solo_profile_path(name: &str) -> Result<PathBuf, String> {
    solo_profiles_dir()
        .map(|d| d.join(format!("{name}.yaml")))
        .ok_or_else(|| "ホームディレクトリが取得できない".into())
}

/// solo 用のプロファイルを読み込む
pub fn load_solo_profile(name: &str) -> Result<Profile, String> {
    let path = solo_profile_path(name)?;
    if !path.is_file() {
        return Err(format!(
            "solo プロファイル '{name}' が見つからない: {}",
            path.display()
        ));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("プロファイルの読み取りに失敗: {e}"))?;
    serde_yaml::from_str(&content).map_err(|e| format!("プロファイルのパースに失敗: {e}"))
}

/// solo 用のデフォルトプロファイルを返す（master より effort が低い）
pub fn solo_default_profile() -> Profile {
    Profile {
        effort: SOLO_DEFAULT_EFFORT.into(),
        ..Default::default()
    }
}

/// master として利用可能なエージェント種別の検証（Issue #127）。
/// agy は MCP のペイン毎接続情報（TAKO_SOCKET / TAKO_PANE_ID 等）を子プロセスへ
/// 引き継ぐ設定手段と system prompt 注入手段が無いため master 非対応（worker のみ）
pub fn validate_master_agent(name: &str) -> Result<WorkerAgent, String> {
    let agent = WorkerAgent::parse(name)?;
    match agent {
        WorkerAgent::Claude | WorkerAgent::Codex => Ok(agent),
        WorkerAgent::Agy => Err(
            "agy は master 非対応（master は claude / codex のみ。worker としては利用可能）".into(),
        ),
    }
}

/// codex の MCP stdio サーバー（`tako mcp serve`）へ親環境から引き継ぐ環境変数。
/// codex は既定で MCP 子プロセスの環境を最小構成（PATH / HOME 等）に絞るため、
/// tako の接続情報は `mcp_servers.<name>.env_vars`（引き継ぎホワイトリスト）で明示する。
/// TAKO_ORCHESTRATOR_ROLE は MCP セッションの caller_role（Issue #109 の複数 master
/// 混線対策）に使われる
const CODEX_MCP_ENV_VARS: &str =
    r#"["TAKO_SOCKET","TAKO_TOKEN","TAKO_PANE_ID","TAKO_TAB_ID","TAKO_ORCHESTRATOR_ROLE"]"#;

/// master 起動用のコマンドを組み立てる（master_agent 対応。Issue #127）。
/// claude の出力は従来の claude 固定実装と同一文字列（完全後方互換）。
/// profile.model が None の場合は `--model` を付けずその CLI の既定に委ねる。
/// `tako_bin` は codex の MCP stdio ブリッジ（`<tako_bin> mcp serve`）の起動パス
pub fn build_master_cmd(
    role_env: &str,
    profile: &Profile,
    prompt_path: &Path,
    tako_bin: &str,
) -> Result<String, String> {
    let agent = profile.resolve_master_agent()?;
    let mut cmd = format!("TAKO_ORCHESTRATOR_ROLE='{role_env}' {}", agent.as_str());
    match agent {
        WorkerAgent::Claude => {
            if let Some(model) = profile.model.as_deref() {
                cmd.push_str(&format!(" --model '{model}'"));
            }
            cmd.push_str(&format!(" --effort {}", profile.effort));
            cmd.push_str(&format!(
                " --append-system-prompt-file '{}'",
                prompt_path.display()
            ));
        }
        WorkerAgent::Codex => {
            if let Some(model) = profile.model.as_deref() {
                cmd.push_str(&format!(" --model {}", agent::sh_quote(model)));
            }
            // codex 0.144 の effort は none/minimal/low/medium/high/xhigh/max/ultra
            // （バイナリの enum で確認）。ネイティブ表記をそのまま渡す（worker と同じ思想）
            cmd.push_str(&format!(
                " -c model_reasoning_effort={}",
                agent::sh_quote(&profile.effort)
            ));
            // MCP ツール呼び出し・コマンド実行の承認をスキップ（#132）。
            // `-a never` はコマンド承認のみでMCPツール承認はバイパスしない（実測）。
            // `--dangerously-bypass-approvals-and-sandbox` は両方バイパスする
            cmd.push_str(" --dangerously-bypass-approvals-and-sandbox");
            // MCP 接続は起動時の -c 一時注入（~/.codex/config.toml を汚さず、
            // tako 外で起動した codex にツールを公開しない = FR-2.3.2 と同方針）
            cmd.push_str(&format!(
                " -c {}",
                agent::sh_quote(&format!(
                    "mcp_servers.tako.command={}",
                    agent::toml_quote(tako_bin)
                ))
            ));
            cmd.push_str(r#" -c 'mcp_servers.tako.args=["mcp","serve"]'"#);
            cmd.push_str(&format!(
                " -c 'mcp_servers.tako.env_vars={CODEX_MCP_ENV_VARS}'"
            ));
            // system prompt は developer_instructions（developer ロールメッセージとして
            // モデル可視プロンプトへ注入されることを codex debug prompt-input で実証済み）。
            // "$(cat …)" はダブルクォート内コマンド置換のため、ファイル内容の $ / " / '
            // はシェルに再解釈されない
            cmd.push_str(&format!(
                " -c developer_instructions=\"$(cat {})\"",
                agent::sh_quote(&prompt_path.display().to_string())
            ));
        }
        // resolve_master_agent が拒否する（master 非対応）
        WorkerAgent::Agy => unreachable!("agy は resolve_master_agent で拒否される"),
    }
    Ok(cmd)
}

/// worker 起動用のコマンドを組み立てる（エージェント種別対応は
/// `agent::build_worker_cmd`。ここは claude 用の互換ラッパー）。
/// model が None の場合は `--model` を付けず claude CLI の既定に委ねる
pub fn build_worker_claude_cmd(role: &str, model: Option<&str>, effort: &str) -> String {
    agent::build_worker_cmd(&agent::WorkerLaunch {
        agent: WorkerAgent::Claude,
        role,
        model,
        effort: Some(effort),
        ..Default::default()
    })
}

/// 1M コンテキスト版モデル（`[1m]` サフィックス）への警告文を生成する。
/// プロファイルへの明示 opt-in は尊重して起動は継続するため、警告のみ（Issue #27）
pub fn one_m_model_warning(model: &str, source: &str) -> Option<String> {
    if !model.contains("[1m]") {
        return None;
    }
    Some(format!(
        "⚠ {source} のモデル '{model}' は 1M コンテキスト版のため、Pro プランでは起動できない可能性があります（Max / API プラン向け）。\n  起動に失敗する場合は `tako orchestrator profiles set <プロファイル名> --clear-model` で解除してください"
    ))
}

/// `<!-- block: name -->` マーカーで区切られたブロックをパースする
fn parse_prompt_blocks(text: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(name) = line
            .trim()
            .strip_prefix("<!-- block: ")
            .and_then(|s| s.strip_suffix(" -->"))
        {
            if let Some(prev_name) = current_name.take() {
                blocks.push((prev_name, current_content.trim_end().to_string()));
            }
            current_name = Some(name.to_string());
            current_content = String::new();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }
    if let Some(name) = current_name {
        blocks.push((name, current_content.trim_end().to_string()));
    }
    blocks
}

/// テキストが `~/` で始まる場合はファイルとして読み込み、それ以外はそのまま返す
fn resolve_text_or_file(text: &str) -> String {
    if text.starts_with("~/") {
        let expanded = expand_tilde(text);
        std::fs::read_to_string(&expanded).unwrap_or_else(|_| text.to_string())
    } else {
        text.to_string()
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

/// 新規生成する default.yaml の内容。
/// model は意図的に未指定 = claude CLI の既定モデルで起動する（プラン非依存。Issue #27）
const DEFAULT_PROFILE_YAML: &str = "\
# tako master のプロファイル設定
# model 未指定 = claude CLI の既定モデルで起動する（プラン非依存・推奨）
#   例: model: claude-opus-4-6        … モデルを固定する場合
#   例: model: claude-opus-4-6[1m]    … 1M コンテキスト版（Max / API プラン限定）
effort: max
worker_model_policy: inherit
";

/// 初回実行時にデフォルトのディレクトリとファイルを生成する
pub fn ensure_defaults() -> Result<PathBuf, String> {
    let dir = config_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    // projects.yaml が無ければ空テンプレートを作る。ロック付き mutate 経由なので
    // 「is_file 確認と書き込みの間に他プロセスが実データを書き、それを空で潰す」
    // TOCTOU が起きない（既存があれば内容不変でスキップ、破損なら Err で中断。#169）
    let yaml_path = dir.join("projects.yaml");
    ProjectsConfig::mutate_at(&yaml_path, |_| ())?;
    // デフォルトプロファイルが無ければ作成
    let profiles = profiles_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&profiles)
        .map_err(|e| format!("profiles ディレクトリの作成に失敗: {e}"))?;
    let default_profile = profiles.join("default.yaml");
    if !default_profile.is_file() {
        std::fs::write(&default_profile, DEFAULT_PROFILE_YAML)
            .map_err(|e| format!("default.yaml の書き込みに失敗: {e}"))?;
    }
    Ok(dir)
}

/// 旧バージョン（0.2.3 以前）が default.yaml に書き込んだ `model: claude-opus-4-6[1m]` を
/// 検出し、バックアップを取って model 行を除去する（Issue #27）。
/// 旧既定値と完全一致する場合のみ対象（ユーザーが別の値を明示した場合は触らない）。
/// 戻り値: マイグレーションを実行した場合は通知メッセージ
pub fn migrate_legacy_default_profile() -> Option<String> {
    let path = profiles_dir()?.join("default.yaml");
    migrate_legacy_model_file(&path)
}

/// マイグレーション本体（パス指定・テスト用に分離）。
/// model 行だけを行単位で除去し、他の設定・コメントは保持する
fn migrate_legacy_model_file(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    // backup が既に存在 = 一度マイグレ済み。ユーザーが profiles set で再設定した可能性があるためスキップ
    let backup = path.with_extension("yaml.backup-1m");
    if backup.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let profile: Profile = serde_yaml::from_str(&content).ok()?;
    if profile.model.as_deref() != Some(LEGACY_DEFAULT_MODEL) {
        return None;
    }

    // トップレベル（行頭）の model 行だけを除去。ネスト行（インデント付き）は対象外
    let is_legacy_model_line = |line: &str| {
        line.strip_prefix("model:").is_some_and(|rest| {
            let value = rest.trim();
            value == LEGACY_DEFAULT_MODEL
                || value == format!("'{LEGACY_DEFAULT_MODEL}'")
                || value == format!("\"{LEGACY_DEFAULT_MODEL}\"")
        })
    };
    // まず model 行だけを行単位で除去した候補を作り、Profile として読めるか検証する。
    // 読めない場合（model 1 行のみ・特殊な書式等）は serde 経由の再構成にフォールバック
    let line_surgery = || -> Option<String> {
        if !content.lines().any(is_legacy_model_line) {
            return None;
        }
        let kept: Vec<&str> = content
            .lines()
            .filter(|l| !is_legacy_model_line(l))
            .collect();
        let mut text = kept.join("\n");
        text.push('\n');
        serde_yaml::from_str::<Profile>(&text).ok()?;
        Some(text)
    };
    let migrated = line_surgery().or_else(|| {
        let mut p = profile;
        p.model = None;
        serde_yaml::to_string(&p).ok()
    })?;

    let _ = std::fs::copy(path, &backup);
    std::fs::write(path, &migrated).ok()?;
    Some(format!(
        "profiles/default.yaml の model: {LEGACY_DEFAULT_MODEL}（旧既定値。Pro プランでは起動不能）を\n  削除しました。今後は claude CLI の既定モデルで起動します。\n  1M コンテキスト版を使う場合（Max / API プラン）は model を明示的に再設定してください。\n  バックアップ: {}",
        backup.display()
    ))
}

/// `claude agents --json` をログインシェル経由で実行する。
/// .app バンドル（Dock 起動）の PATH は最小構成で `claude` が見つからないため、
/// `$SHELL -l -c "claude agents --json"` でユーザーの PATH を使う。
///
/// Issue #168: ログインシェル + Node CLI の起動は 1 回 500ms〜1s かかる。master の
/// watch / worker_status が数秒間隔 × worker 数で呼ぶため、TTL 内は前回結果を返し、
/// 実行自体もロック保持で直列化する（多重呼び出しでプロセスが並んで起動しない。
/// 並行呼び出しは実行完了を待って fresh な結果を受け取る）
pub(crate) fn run_claude_agents_json() -> Option<Vec<u8>> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Option<Vec<u8>>)>> = Mutex::new(None);
    /// watch のポーリング間隔（数秒）より短く、判定の鮮度に影響しない長さ
    const TTL: Duration = Duration::from_secs(2);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = cache.as_ref() {
        if at.elapsed() < TTL {
            return value.clone();
        }
    }
    let result = run_claude_agents_json_uncached();
    *cache = Some((Instant::now(), result.clone()));
    result
}

fn run_claude_agents_json_uncached() -> Option<Vec<u8>> {
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

    /// テスト用の隔離ディレクトリ（実運用の projects.yaml には絶対に触らない）
    fn isolated_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-issue169-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// n 件のエントリを持つ ProjectsConfig を作る
    fn config_with_entries(n: usize) -> ProjectsConfig {
        let mut config = ProjectsConfig {
            projects: std::collections::BTreeMap::new(),
        };
        for i in 0..n {
            config.add(
                format!("proj-{i:02}"),
                format!("~/Documents/proj-{i:02}"),
                Some(format!(
                    "テスト用プロジェクト {i}（日本語 description: URL https://example.com/{i}）"
                )),
            );
        }
        config
    }

    /// #169 根本原因の実証（その 1）: serde_yaml は空文字列・`projects:` だけの
    /// 部分内容を「0 件」として**成功**パースする（エラーにならない）。
    /// 旧実装の `std::fs::write`（truncate → write）の窓で並行プロセスが読んだ
    /// 空 / 部分ファイルが正常な 0 件と区別できず、0 件ベースの add → save が
    /// 全エントリを消した。この性質が serde_yaml 更新で変わったら気付くための固定
    #[test]
    fn issue_169_empty_and_partial_yaml_parse_as_zero_projects() {
        // 空文字列（truncate 直後の 0 バイトファイル相当）→ Ok(0 件)
        let empty: ProjectsConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(empty.projects.len(), 0);
        // `projects:` ヘッダ行までの部分書き込み相当 → Ok(0 件)
        let partial_header: ProjectsConfig = serde_yaml::from_str("projects:\n").unwrap();
        assert_eq!(partial_header.projects.len(), 0);
        // 値の途中で切れた場合だけはパースエラーになる
        assert!(serde_yaml::from_str::<ProjectsConfig>("projects:\n  tako:\n    cw").is_err());
    }

    /// #169 根本原因の実証（その 2）: 旧実装の消失機序をファイル操作で再現する。
    /// 58 件のファイルに対し「別プロセスの save が truncate した瞬間」を再現すると、
    /// load は Err ではなく Ok(0 件) を返し、その 0 件へ add → save した結果が
    /// 「add した 1 件だけのファイル」= 事故当日の projects.yaml と一致する
    #[test]
    fn issue_169_truncate_window_reproduces_total_loss() {
        let dir = isolated_dir("truncate-window");
        let path = dir.join("projects.yaml");
        config_with_entries(58).save_to(&path).unwrap();

        // 旧 save = std::fs::write は File::create（truncate）→ write の 2 段階。
        // truncate と write の間に他プロセスの load が走った状況を再現する
        drop(std::fs::File::create(&path).unwrap());

        let loaded = ProjectsConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.projects.len(),
            0,
            "空ファイルが 0 件として成功パースされる（エラーにならない）"
        );

        // 0 件ベースに add → save = 事故の書き込み。結果は 1 件だけのファイル
        let mut lost = loaded;
        lost.add(
            "tako-wt-release".into(),
            "~/Documents/tako-wt-release".into(),
            None,
        );
        lost.save_to(&path).unwrap();
        let after = ProjectsConfig::load_from(&path).unwrap();
        assert_eq!(after.projects.len(), 1, "58 件 → 1 件の全消失が再現された");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #169 修正の固定: mutate_at はアトミック書き込み（tmp + rename）なので
    /// truncate 状態がそもそも発生せず、並行プロセスの add が互いを消さない。
    /// 16 スレッド × 各 4 件 = 64 件の並行 add 後、初期 58 件 + 64 件が全件残る
    #[test]
    fn issue_169_concurrent_mutate_preserves_all_entries() {
        let dir = isolated_dir("concurrent");
        let path = dir.join("projects.yaml");
        config_with_entries(58).save_to(&path).unwrap();

        let mut handles = Vec::new();
        for t in 0..16 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..4 {
                    ProjectsConfig::mutate_at(&path, |config| {
                        config.add(
                            format!("thread-{t:02}-entry-{i}"),
                            format!("~/tmp/t{t}-{i}"),
                            None,
                        );
                    })
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let after = ProjectsConfig::load_from(&path).unwrap();
        assert_eq!(
            after.projects.len(),
            58 + 16 * 4,
            "並行 add で 1 件も消えない（修正前は read-modify-write 競合で消えた）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #169 修正の固定: 破損 YAML への mutate は Err で中断し、ファイルには
    /// 1 バイトも書かない（バックアップ回転も起こさない）
    #[test]
    fn issue_169_mutate_rejects_corrupted_yaml_without_touching_file() {
        let dir = isolated_dir("corrupted");
        let path = dir.join("projects.yaml");
        // 値の途中で切れた壊れ YAML（truncate 事故や手編集ミス相当）
        let corrupted = "projects:\n  tako:\n    cw";
        std::fs::write(&path, corrupted).unwrap();

        let result = ProjectsConfig::mutate_at(&path, |config| {
            config.add("new".into(), "~/tmp/new".into(), None);
        });
        assert!(result.is_err(), "破損 YAML への add はエラーになる");
        assert!(
            result.unwrap_err().contains("パースに失敗"),
            "パース失敗を明示するエラーメッセージ"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupted,
            "破損ファイルは 1 バイトも書き換えられない"
        );
        assert!(
            !crate::config_io::backup_path(&path, 1).exists(),
            "書き込みが走っていないのでバックアップも生まれない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// mutate_at はファイル不在から空で開始してファイルを作る
    /// （ensure_defaults の TOCTOU 置き換えと CLI 初回 add の経路）
    #[test]
    fn mutate_creates_file_when_missing() {
        let dir = isolated_dir("create");
        let path = dir.join("projects.yaml");
        ProjectsConfig::mutate_at(&path, |config| {
            config.add("first".into(), "~/tmp/first".into(), None);
        })
        .unwrap();
        let after = ProjectsConfig::load_from(&path).unwrap();
        assert_eq!(after.projects.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #169: 書き込み前の自動バックアップ。直前世代が .bak.1 に残り、
    /// 誤消去してもロールバックできる
    #[test]
    fn issue_169_backup_generation_on_each_change() {
        let dir = isolated_dir("backup");
        let path = dir.join("projects.yaml");
        config_with_entries(3).save_to(&path).unwrap();
        ProjectsConfig::mutate_at(&path, |config| {
            config.add("added".into(), "~/tmp/added".into(), None);
        })
        .unwrap();

        let backup = crate::config_io::backup_path(&path, 1);
        assert!(backup.is_file(), "変更前の内容が .bak.1 に残る");
        let backed_up = ProjectsConfig::load_from(&backup).unwrap();
        assert_eq!(backed_up.projects.len(), 3, "バックアップは変更前の 3 件");
        let current = ProjectsConfig::load_from(&path).unwrap();
        assert_eq!(current.projects.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #169 横展開: パースできないプロファイルへの mutate_named 相当も
    /// default に丸めて上書きせず Err で中断する（旧実装は unwrap_or_default で
    /// 破損プロファイルを default に丸めて保存 = 設定消失だった）
    #[test]
    fn issue_169_profile_load_from_or_default_fails_loud_on_corruption() {
        let dir = isolated_dir("profile-corrupt");
        let path = dir.join("broken.yaml");
        std::fs::write(&path, "effort: [unclosed").unwrap();
        let result = Profile::load_from_or_default(&path);
        assert!(result.is_err(), "破損プロファイルは default に丸めない");
        // 不在は default から開始できる（初回 set の正当ケース）
        let missing = Profile::load_from_or_default(&dir.join("missing.yaml")).unwrap();
        assert_eq!(missing.effort, Profile::default().effort);
        let _ = std::fs::remove_dir_all(&dir);
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
        // model 無指定 = claude CLI の既定に委ねる（Issue #27。[1m] を既定にしない）
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "max");
        assert_eq!(p.worker_model_policy, WorkerModelPolicy::Inherit);
        assert!(p.worker_model.is_none());
        assert!(p.worker_effort.is_none());
        assert!(p.delegate_guidance.is_none());
        assert!(p.system_prompt.is_none());
        assert!(p.prompt_blocks.is_none());
        assert!(p.projects.is_none());
    }

    #[test]
    fn profile_default_serializes_without_model() {
        let yaml = serde_yaml::to_string(&Profile::default()).unwrap();
        assert!(!yaml.contains("model:") || yaml.contains("worker_model_policy"));
        assert!(!yaml.contains("claude-opus"));
        assert!(!yaml.contains("[1m]"));
    }

    #[test]
    fn default_profile_yaml_template_parses() {
        let p: Profile = serde_yaml::from_str(DEFAULT_PROFILE_YAML).unwrap();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "max");
        assert_eq!(p.worker_model_policy, WorkerModelPolicy::Inherit);
    }

    #[test]
    fn profile_roundtrip() {
        let p = Profile {
            model: Some("claude-sonnet-5".into()),
            effort: "high".into(),
            system_prompt: Some("~/my-prompt.md".into()),
            projects: Some(vec!["tako".into(), "demo".into()]),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&p).unwrap();
        let back: Profile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(back.effort, "high");
        assert_eq!(back.system_prompt.as_deref(), Some("~/my-prompt.md"));
        assert_eq!(back.projects.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn profile_deserialize_minimal() {
        // 旧形式（model 明示）も読める後方互換
        let yaml = "model: claude-opus-4-6[1m]\n";
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.model.as_deref(), Some("claude-opus-4-6[1m]"));
        assert_eq!(p.effort, "max");
        assert_eq!(p.worker_model_policy, WorkerModelPolicy::Inherit);
        assert!(p.projects.is_none());
    }

    #[test]
    fn profile_deserialize_empty() {
        // model 行が無いファイル = claude 既定
        let yaml = "effort: high\n";
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "high");
    }

    #[test]
    fn profile_save_load_roundtrip() {
        let tmp = std::env::temp_dir().join("tako-test-profiles");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let name = "test-roundtrip";
        let path = tmp.join(format!("{name}.yaml"));
        let p = Profile {
            model: Some("test-model".into()),
            effort: "low".into(),
            projects: Some(vec!["a".into()]),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&p).unwrap();
        std::fs::write(&path, &yaml).unwrap();
        let loaded: Profile =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.model.as_deref(), Some("test-model"));
        assert_eq!(loaded.projects.as_ref().unwrap(), &["a"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn worker_model_policy_inherit() {
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "high".into(),
            ..Default::default()
        };
        assert_eq!(p.resolve_worker_model(), Some("claude-fable-5"));
        assert_eq!(p.resolve_worker_effort(), "high");
    }

    #[test]
    fn worker_model_policy_inherit_unspecified() {
        // model 無指定の master → worker も claude 既定
        let p = Profile::default();
        assert_eq!(p.resolve_worker_model(), None);
        assert_eq!(p.model_label(), CLAUDE_DEFAULT_LABEL);
        assert_eq!(p.worker_model_label(), CLAUDE_DEFAULT_LABEL);
    }

    #[test]
    fn worker_model_policy_fixed() {
        let p = Profile {
            model: Some("claude-opus-4-6[1m]".into()),
            effort: "max".into(),
            worker_model_policy: WorkerModelPolicy::Fixed,
            worker_model: Some("claude-sonnet-5".into()),
            worker_effort: Some("medium".into()),
            ..Default::default()
        };
        assert_eq!(p.resolve_worker_model(), Some("claude-sonnet-5"));
        assert_eq!(p.resolve_worker_effort(), "medium");
    }

    #[test]
    fn worker_model_policy_fixed_fallback() {
        let p = Profile {
            model: Some("claude-opus-4-6[1m]".into()),
            effort: "max".into(),
            worker_model_policy: WorkerModelPolicy::Fixed,
            ..Default::default()
        };
        assert_eq!(p.resolve_worker_model(), Some("claude-opus-4-6[1m]"));
        assert_eq!(p.resolve_worker_effort(), "max");
    }

    #[test]
    fn worker_model_policy_delegate() {
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "high".into(),
            worker_model_policy: WorkerModelPolicy::Delegate,
            delegate_guidance: Some("タスクの複雑さで判断".into()),
            ..Default::default()
        };
        assert_eq!(p.resolve_worker_model(), Some("claude-fable-5"));
        assert_eq!(p.resolve_worker_effort(), "high");
    }

    #[test]
    fn worker_model_policy_deserialize() {
        let yaml = "model: claude-fable-5\neffort: high\nworker_model_policy: fixed\nworker_model: claude-sonnet-5\n";
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.worker_model_policy, WorkerModelPolicy::Fixed);
        assert_eq!(p.worker_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(p.resolve_worker_model(), Some("claude-sonnet-5"));
    }

    #[test]
    fn prompt_blocks_deserialize() {
        let yaml = r##"
model: claude-fable-5
effort: high
prompt_blocks:
  disable:
    - no-investigate
  prepend: "# Custom Header"
  append: "# Custom Footer"
  override_blocks:
    behavior: "Custom behavior text"
"##;
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        let blocks = p.prompt_blocks.unwrap();
        assert_eq!(blocks.disable, vec!["no-investigate"]);
        assert_eq!(blocks.prepend.as_deref(), Some("# Custom Header"));
        assert_eq!(blocks.append.as_deref(), Some("# Custom Footer"));
        assert_eq!(
            blocks.override_blocks.get("behavior").map(|s| s.as_str()),
            Some("Custom behavior text")
        );
    }

    #[test]
    fn parse_prompt_blocks_basic() {
        let text = "<!-- block: a -->\nline1\nline2\n<!-- block: b -->\nline3\n";
        let blocks = parse_prompt_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, "a");
        assert!(blocks[0].1.contains("line1"));
        assert_eq!(blocks[1].0, "b");
        assert!(blocks[1].1.contains("line3"));
    }

    #[test]
    fn build_system_prompt_default() {
        let p = Profile::default();
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("Master Orchestrator Agent"));
        assert!(prompt.contains("Worker Model Policy"));
        // 既定はモデル無指定 = claude CLI の既定（[1m] を含まない）
        assert!(prompt.contains(CLAUDE_DEFAULT_LABEL));
        assert!(!prompt.contains("[1m]"));
        // #292: judgment 二層が model-policy の後に注入されていること
        let policy_pos = prompt.find("Worker Model Policy").unwrap();
        let judgment_pos = prompt
            .find("Delegation Judgment Criteria")
            .expect("judgment セクションが存在する");
        assert!(judgment_pos > policy_pos, "judgment は model-policy の後");
        assert!(prompt.contains("Built-in Defaults"));
        assert!(prompt.contains("Survey Frequency Control"));
        assert!(prompt.contains("bugfix-rooted"));
    }

    #[test]
    fn project_resolution_gate_in_default_prompt() {
        let p = Profile::default();
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        // Step 0 が存在し、Step 1 より前にあること
        let step0_pos = prompt.find("Step 0").expect("Step 0 が存在する");
        let step1_pos = prompt.find("Step 1").expect("Step 1 が存在する");
        assert!(step0_pos < step1_pos, "Step 0 は Step 1 より前");
        // 順序制約の主要キーワードが含まれること
        assert!(prompt.contains("Resolve target projects"));
        assert!(prompt.contains("tako_orchestrator_projects"));
        assert!(prompt.contains("high-confidence match"));
        assert!(prompt.contains("Zero matches"));
        // ステップ数が five に更新されていること
        assert!(prompt.contains("five steps"));
        // Step 4 にプロジェクト key 明記の指示があること
        assert!(prompt.contains("Step 0 resolved"));
    }

    #[test]
    fn project_resolution_gate_survives_append() {
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                append: Some("CUSTOM_FOOTER".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("Step 0"));
        assert!(prompt.contains("Resolve target projects"));
        assert!(prompt.ends_with("CUSTOM_FOOTER"));
    }

    #[test]
    fn build_system_prompt_inherit_model() {
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "high".into(),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("claude-fable-5"));
        assert!(prompt.contains("high"));
    }

    #[test]
    fn build_system_prompt_fixed_policy() {
        let p = Profile {
            model: Some("claude-opus-4-6[1m]".into()),
            effort: "max".into(),
            worker_model_policy: WorkerModelPolicy::Fixed,
            worker_model: Some("claude-sonnet-5".into()),
            worker_effort: Some("medium".into()),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("claude-sonnet-5"));
        assert!(prompt.contains("medium"));
        assert!(prompt.contains("fixed model/effort"));
    }

    #[test]
    fn build_system_prompt_delegate_policy() {
        let p = Profile {
            model: Some("claude-opus-4-6[1m]".into()),
            effort: "max".into(),
            worker_model_policy: WorkerModelPolicy::Delegate,
            delegate_guidance: Some("複雑なタスクは Opus、単純なタスクは Sonnet".into()),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("Delegation Guidance"));
        assert!(prompt.contains("複雑なタスクは Opus"));
    }

    #[test]
    fn build_system_prompt_disable_block() {
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                disable: vec!["no-investigate".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(!prompt.contains("The Master Does Not Investigate"));
        assert!(prompt.contains("Master Orchestrator Agent"));
    }

    #[test]
    fn build_system_prompt_override_block() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("behavior".into(), "Custom behavior rules here".into());
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                override_blocks: overrides,
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("Custom behavior rules here"));
        assert!(!prompt.contains("Act on hypotheses"));
    }

    #[test]
    fn build_system_prompt_prepend_append() {
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                prepend: Some("PREPEND_MARKER".into()),
                append: Some("APPEND_MARKER".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.starts_with("PREPEND_MARKER"));
        assert!(prompt.ends_with("APPEND_MARKER"));
    }

    #[test]
    fn identity_block_injected() {
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "high".into(),
            worker_model_policy: WorkerModelPolicy::Fixed,
            worker_model: Some("claude-sonnet-5".into()),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "fable");
        assert!(prompt.contains("Session Identity"));
        assert!(prompt.contains("Profile**: `fable`"));
        assert!(prompt.contains("tako master -fable"));
        assert!(prompt.contains("claude-fable-5"));
        assert!(prompt.contains("fixed"));
        assert!(prompt.contains("claude-sonnet-5"));
    }

    #[test]
    fn identity_block_shows_customizations() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("behavior".into(), "custom".into());
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                disable: vec!["no-investigate".into()],
                override_blocks: overrides,
                prepend: Some("header".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "custom");
        assert!(prompt.contains("disabled: `no-investigate`"));
        assert!(prompt.contains("overridden: `behavior`"));
        assert!(prompt.contains("prepend: あり"));
    }

    #[test]
    fn build_system_prompt_with_worker_agents() {
        let mut agents = std::collections::BTreeMap::new();
        agents.insert(
            "codex".to_string(),
            AgentWorkerConfig {
                model: Some("gpt-5.6-terra".into()),
                effort: Some("medium".into()),
                ..Default::default()
            },
        );
        agents.insert(
            "agy".to_string(),
            AgentWorkerConfig {
                model: Some("Gemini 3.5 Flash (High)".into()),
                skip_permissions: true,
                ..Default::default()
            },
        );
        let p = Profile {
            worker_agent: Some("codex".into()),
            worker_agents: agents,
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "multi");
        assert!(prompt.contains("### Available Worker Agents"));
        assert!(prompt.contains("default agent is **codex**"));
        assert!(prompt.contains("gpt-5.6-terra"));
        assert!(prompt.contains("Gemini 3.5 Flash (High)"));
        assert!(prompt.contains("skip_permissions"));
        // identity にも worker agent 既定が出る
        assert!(prompt.contains("Worker agent**: codex"));
    }

    #[test]
    fn build_system_prompt_without_worker_agents_unchanged() {
        // worker_agent 系が未設定なら従来のプロンプトに新セクション（見出し）は出ない
        // （回帰なし。テンプレ本文にはセクション名への言及があるため見出しで判定する）
        let p = Profile::default();
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(!prompt.contains("### Available Worker Agents"));
        assert!(!prompt.contains("Worker agent**:"));
    }

    #[test]
    fn identity_block_not_disableable() {
        let p = Profile {
            prompt_blocks: Some(PromptBlocks {
                disable: vec!["identity".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_from_template(DEFAULT_SYSTEM_PROMPT, "test");
        assert!(prompt.contains("Session Identity"));
    }

    #[test]
    fn master_cmd_without_model() {
        // 既定（master_agent 未指定 = claude）の出力は #127 以前と完全一致（後方互換）
        let p = Profile::default();
        let cmd = build_master_cmd("master", &p, Path::new("/tmp/p.md"), "tako").unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='master' claude --effort max --append-system-prompt-file '/tmp/p.md'"
        );
        assert!(!cmd.contains("--model"));
    }

    #[test]
    fn master_cmd_with_model() {
        let p = Profile {
            model: Some("claude-opus-4-6[1m]".into()),
            ..Default::default()
        };
        let cmd = build_master_cmd("master:x", &p, Path::new("/tmp/p.md"), "tako").unwrap();
        assert!(cmd.contains("--model 'claude-opus-4-6[1m]'"));
        assert!(cmd.contains("--effort max"));
    }

    #[test]
    fn master_cmd_codex() {
        // codex master: model/effort のネイティブ表記 + MCP 一時注入 + developer_instructions
        let p = Profile {
            master_agent: Some("codex".into()),
            model: Some("gpt-5.6-sol".into()),
            effort: "xhigh".into(),
            ..Default::default()
        };
        let cmd = build_master_cmd(
            "master:sol",
            &p,
            Path::new("/tmp/_system_prompt_sol.md"),
            "/usr/local/bin/tako",
        )
        .unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='master:sol' codex \
             --model gpt-5.6-sol \
             -c model_reasoning_effort=xhigh \
             --dangerously-bypass-approvals-and-sandbox \
             -c 'mcp_servers.tako.command=\"/usr/local/bin/tako\"' \
             -c 'mcp_servers.tako.args=[\"mcp\",\"serve\"]' \
             -c 'mcp_servers.tako.env_vars=[\"TAKO_SOCKET\",\"TAKO_TOKEN\",\"TAKO_PANE_ID\",\"TAKO_TAB_ID\",\"TAKO_ORCHESTRATOR_ROLE\"]' \
             -c developer_instructions=\"$(cat /tmp/_system_prompt_sol.md)\""
        );
    }

    #[test]
    fn master_cmd_codex_without_model_quotes_special_paths() {
        // model 未指定は --model を付けず codex 既定に委ねる。
        // 空白入りパス（.app 内の tako CLI 等）は TOML/シェルの二重クオートで守る
        let p = Profile {
            master_agent: Some("codex".into()),
            ..Default::default()
        };
        let cmd = build_master_cmd(
            "master",
            &p,
            Path::new("/tmp/pro file.md"),
            "/Applications/tako.app/Contents/Resources/tako bin/tako",
        )
        .unwrap();
        assert!(!cmd.contains("--model"));
        assert!(cmd.contains(" -c model_reasoning_effort=max"), "{cmd}");
        assert!(cmd.contains(
            r#" -c 'mcp_servers.tako.command="/Applications/tako.app/Contents/Resources/tako bin/tako"'"#
        ));
        assert!(cmd.contains(r#" -c developer_instructions="$(cat '/tmp/pro file.md')""#));
    }

    #[test]
    fn master_agent_validation() {
        // 未指定 → claude（後方互換）
        assert_eq!(
            Profile::default().resolve_master_agent(),
            Ok(WorkerAgent::Claude)
        );
        // codex → OK
        let codex = Profile {
            master_agent: Some("codex".into()),
            ..Default::default()
        };
        assert_eq!(codex.resolve_master_agent(), Ok(WorkerAgent::Codex));
        // agy → master 非対応の明示エラー（worker では使える旨を含む）
        let agy = Profile {
            master_agent: Some("agy".into()),
            ..Default::default()
        };
        let err = agy.resolve_master_agent().unwrap_err();
        assert!(err.contains("master 非対応"), "{err}");
        assert!(err.contains("worker"), "{err}");
        assert!(
            build_master_cmd("master", &agy, Path::new("/tmp/p.md"), "tako").is_err(),
            "agy はコマンド組み立ても拒否"
        );
        // 不正値 → 対応一覧つきエラー
        let bad = Profile {
            master_agent: Some("gemini".into()),
            ..Default::default()
        };
        let err = bad.resolve_master_agent().unwrap_err();
        assert!(err.contains("master_agent が不正"), "{err}");
        assert!(err.contains("gemini"), "{err}");
    }

    #[test]
    fn codex_master_does_not_leak_model_to_claude_workers() {
        // master が codex のとき、gpt モデル名・codex 用 effort を claude worker へ
        // 継承しない（inherit / delegate / fixed フォールバックの全経路。#127）
        for policy in [
            WorkerModelPolicy::Inherit,
            WorkerModelPolicy::Delegate,
            WorkerModelPolicy::Fixed,
        ] {
            let p = Profile {
                master_agent: Some("codex".into()),
                model: Some("gpt-5.6-sol".into()),
                effort: "xhigh".into(),
                worker_model_policy: policy,
                ..Default::default()
            };
            assert_eq!(p.resolve_worker_model(), None, "policy={policy:?}");
            assert_eq!(p.resolve_worker_effort(), "max", "policy={policy:?}");
        }
        // fixed で worker_model / worker_effort が明示されていればそれを使う（従来通り）
        let fixed = Profile {
            master_agent: Some("codex".into()),
            model: Some("gpt-5.6-sol".into()),
            effort: "xhigh".into(),
            worker_model_policy: WorkerModelPolicy::Fixed,
            worker_model: Some("claude-sonnet-5".into()),
            worker_effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(fixed.resolve_worker_model(), Some("claude-sonnet-5"));
        assert_eq!(fixed.resolve_worker_effort(), "high");
    }

    #[test]
    fn master_model_label_reflects_agent() {
        // model 未指定時のラベルは master_agent の CLI 既定を指す
        assert_eq!(
            Profile::default().master_model_label(),
            CLAUDE_DEFAULT_LABEL
        );
        let codex = Profile {
            master_agent: Some("codex".into()),
            ..Default::default()
        };
        assert_eq!(codex.master_model_label(), "(codex CLI default)");
        let with_model = Profile {
            master_agent: Some("codex".into()),
            model: Some("gpt-5.6-sol".into()),
            ..Default::default()
        };
        assert_eq!(with_model.master_model_label(), "gpt-5.6-sol");
    }

    /// solo は build_master_cmd を共用する。既定プロファイルは model 無指定・
    /// effort=high で、TAKO_ORCHESTRATOR_ROLE は 'solo'（suffix 付きは 'solo:<suffix>'）になる。
    #[test]
    fn solo_cmd_uses_solo_role_and_high_effort() {
        let p = solo_default_profile();
        let cmd = build_master_cmd("solo", &p, Path::new("/tmp/solo.md"), "tako").unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='solo' claude --effort high --append-system-prompt-file '/tmp/solo.md'"
        );
        assert!(
            !cmd.contains("--model"),
            "model 未指定は claude 既定に委ねる"
        );

        let cmd_suffix =
            build_master_cmd("solo:docs", &p, Path::new("/tmp/solo.md"), "tako").unwrap();
        assert!(cmd_suffix.contains("TAKO_ORCHESTRATOR_ROLE='solo:docs'"));
        assert!(cmd_suffix.contains("--effort high"));
    }

    /// solo プロファイルでも master_agent: codex が効く（コマンド組み立て共用。#127）
    #[test]
    fn solo_cmd_codex_agent() {
        let p = Profile {
            master_agent: Some("codex".into()),
            effort: "high".into(),
            ..Default::default()
        };
        let cmd = build_master_cmd("solo", &p, Path::new("/tmp/solo.md"), "tako").unwrap();
        assert!(cmd.starts_with("TAKO_ORCHESTRATOR_ROLE='solo' codex "));
        assert!(cmd.contains("-c model_reasoning_effort=high"));
        assert!(
            cmd.contains("--dangerously-bypass-approvals-and-sandbox"),
            "codex master/solo は承認スキップ"
        );
        assert!(cmd.contains("mcp_servers.tako.command"));
    }

    #[test]
    fn worker_cmd_model_optional() {
        // #120 のエージェント抽象化でモデル名のクオートは安全文字のみなら省く形へ
        // 変わった（シェル解釈後は従来と等価）
        let with = build_worker_claude_cmd("worker:demo", Some("claude-sonnet-5"), "high");
        assert_eq!(
            with,
            "TAKO_ORCHESTRATOR_ROLE='worker:demo' claude --model claude-sonnet-5 --effort high"
        );
        let without = build_worker_claude_cmd("worker:demo", None, "max");
        assert_eq!(
            without,
            "TAKO_ORCHESTRATOR_ROLE='worker:demo' claude --effort max"
        );
    }

    #[test]
    fn resolve_worker_agent_priority() {
        // 明示指定 > プロファイル既定 > claude
        let p = Profile::default();
        assert_eq!(p.resolve_worker_agent(None), Ok(WorkerAgent::Claude));
        assert_eq!(
            p.resolve_worker_agent(Some("codex")),
            Ok(WorkerAgent::Codex)
        );

        let p2 = Profile {
            worker_agent: Some("agy".into()),
            ..Default::default()
        };
        assert_eq!(p2.resolve_worker_agent(None), Ok(WorkerAgent::Agy));
        assert_eq!(
            p2.resolve_worker_agent(Some("codex")),
            Ok(WorkerAgent::Codex),
            "spawn の明示指定がプロファイル既定より優先"
        );
    }

    #[test]
    fn resolve_worker_agent_rejects_unknown() {
        let p = Profile::default();
        let err = p.resolve_worker_agent(Some("gemini")).unwrap_err();
        assert!(err.contains("claude / codex / agy"));

        // プロファイル既定が不正な場合も spawn 時に診断つきエラー
        let p2 = Profile {
            worker_agent: Some("cursor".into()),
            ..Default::default()
        };
        let err2 = p2.resolve_worker_agent(None).unwrap_err();
        assert!(err2.contains("worker_agent が不正"));
    }

    #[test]
    fn resolve_agent_launch_claude_keeps_legacy_resolution() {
        // claude は worker_agents 未設定なら従来の worker_model_policy 解決を維持
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "high".into(),
            ..Default::default()
        };
        let launch = p.resolve_agent_launch(WorkerAgent::Claude, None, None);
        assert_eq!(launch.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(launch.effort.as_deref(), Some("high"));
        assert!(!launch.skip_permissions);
        assert!(launch.extra_args.is_empty());

        // 明示指定が最優先
        let launch2 =
            p.resolve_agent_launch(WorkerAgent::Claude, Some("claude-sonnet-5"), Some("low"));
        assert_eq!(launch2.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(launch2.effort.as_deref(), Some("low"));
    }

    #[test]
    fn resolve_agent_launch_claude_worker_agents_overrides_policy() {
        // worker_agents.claude はポリシー解決より優先（args / skip_permissions も有効）
        let mut agents = std::collections::BTreeMap::new();
        agents.insert(
            "claude".to_string(),
            AgentWorkerConfig {
                model: Some("claude-haiku-4-5".into()),
                effort: Some("low".into()),
                skip_permissions: true,
                args: vec!["--verbose".into()],
            },
        );
        let p = Profile {
            model: Some("claude-fable-5".into()),
            worker_agents: agents,
            ..Default::default()
        };
        let launch = p.resolve_agent_launch(WorkerAgent::Claude, None, None);
        assert_eq!(launch.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(launch.effort.as_deref(), Some("low"));
        assert!(launch.skip_permissions);
        assert_eq!(launch.extra_args, vec!["--verbose".to_string()]);
    }

    #[test]
    fn resolve_agent_launch_codex_uses_agent_config() {
        let mut agents = std::collections::BTreeMap::new();
        agents.insert(
            "codex".to_string(),
            AgentWorkerConfig {
                model: Some("gpt-5.6-terra".into()),
                effort: Some("medium".into()),
                ..Default::default()
            },
        );
        let p = Profile {
            model: Some("claude-fable-5".into()), // master のモデルは codex に波及しない
            worker_agents: agents,
            ..Default::default()
        };
        let launch = p.resolve_agent_launch(WorkerAgent::Codex, None, None);
        assert_eq!(launch.model.as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(launch.effort.as_deref(), Some("medium"));

        // 明示指定が最優先
        let launch2 = p.resolve_agent_launch(WorkerAgent::Codex, Some("gpt-5.6-luna"), None);
        assert_eq!(launch2.model.as_deref(), Some("gpt-5.6-luna"));
    }

    #[test]
    fn resolve_agent_launch_non_claude_defaults_to_cli() {
        // worker_agents 未設定の codex / agy は CLI 既定（model / effort とも None）。
        // master の model・effort（claude 用）は波及しない
        let p = Profile {
            model: Some("claude-fable-5".into()),
            effort: "max".into(),
            ..Default::default()
        };
        for agent in [WorkerAgent::Codex, WorkerAgent::Agy] {
            let launch = p.resolve_agent_launch(agent, None, None);
            assert_eq!(launch.model, None, "{agent:?} は CLI 既定");
            assert_eq!(
                launch.effort, None,
                "{agent:?} に claude の effort を波及させない"
            );
            assert!(launch.skip_permissions, "{agent:?} は既定で承認スキップ");
        }
        // claude は既定で承認あり
        let launch_claude = p.resolve_agent_launch(WorkerAgent::Claude, None, None);
        assert!(!launch_claude.skip_permissions, "claude は既定で承認あり");
    }

    #[test]
    fn resolve_agent_launch_explicit_false_overrides_default() {
        // codex / agy でも skip_permissions: false を明示すると承認ありになる
        for agent_name in ["codex", "agy"] {
            let mut agents = std::collections::BTreeMap::new();
            agents.insert(
                agent_name.to_string(),
                AgentWorkerConfig {
                    skip_permissions: false,
                    ..Default::default()
                },
            );
            let p = Profile {
                worker_agents: agents,
                ..Default::default()
            };
            let agent = WorkerAgent::parse(agent_name).unwrap();
            let launch = p.resolve_agent_launch(agent, None, None);
            assert!(
                !launch.skip_permissions,
                "{agent_name} は明示 false で承認あり"
            );
        }
    }

    #[test]
    fn profile_with_worker_agents_roundtrip() {
        let yaml = r#"
effort: max
worker_agent: codex
worker_agents:
  codex:
    model: gpt-5.6-terra
    effort: medium
  agy:
    model: "Gemini 3.5 Flash (High)"
    skip_permissions: true
"#;
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.worker_agent.as_deref(), Some("codex"));
        assert_eq!(
            p.worker_agents["codex"].model.as_deref(),
            Some("gpt-5.6-terra")
        );
        assert!(p.worker_agents["agy"].skip_permissions);
        assert_eq!(
            p.worker_agents["agy"].model.as_deref(),
            Some("Gemini 3.5 Flash (High)")
        );

        // 再シリアライズしても保持される
        let back: Profile = serde_yaml::from_str(&serde_yaml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.worker_agent.as_deref(), Some("codex"));
        assert_eq!(back.worker_agents.len(), 2);
    }

    #[test]
    fn profile_without_worker_agents_is_backward_compatible() {
        // 既存プロファイル（worker_agent 系なし）がそのまま読め、
        // シリアライズ時に新フィールドが出力されない
        let yaml = "effort: max\nworker_model_policy: inherit\n";
        let p: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.worker_agent, None);
        assert!(p.worker_agents.is_empty());
        assert_eq!(p.resolve_worker_agent(None), Ok(WorkerAgent::Claude));

        let out = serde_yaml::to_string(&p).unwrap();
        assert!(!out.contains("worker_agent"), "未設定時は出力しない: {out}");
    }

    #[test]
    fn one_m_warning_only_for_1m_models() {
        assert!(one_m_model_warning("claude-opus-4-6[1m]", "master").is_some());
        assert!(one_m_model_warning("claude-opus-4-6", "master").is_none());
        assert!(one_m_model_warning("claude-sonnet-5", "worker").is_none());
    }

    #[test]
    fn migrate_removes_legacy_default_model_line() {
        let tmp = std::env::temp_dir().join("tako-test-migrate-legacy");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("default.yaml");
        // 旧バージョンが生成した形 + ユーザー追記のコメント・設定
        std::fs::write(
            &path,
            "# user comment\nmodel: claude-opus-4-6[1m]\neffort: high\nworker_model_policy: inherit\n",
        )
        .unwrap();

        let msg = migrate_legacy_model_file(&path);
        assert!(msg.is_some(), "旧既定値はマイグレーションされる");

        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(!migrated.contains("model:"), "model 行が除去される");
        assert!(migrated.contains("# user comment"), "コメントは保持");
        assert!(migrated.contains("effort: high"), "他の設定は保持");
        let p: Profile = serde_yaml::from_str(&migrated).unwrap();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "high");
        // バックアップが作成される
        assert!(path.with_extension("yaml.backup-1m").is_file());
        // 2 回目は no-op
        assert!(migrate_legacy_model_file(&path).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_keeps_user_specified_models() {
        let tmp = std::env::temp_dir().join("tako-test-migrate-keep");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("default.yaml");
        // 旧既定値と異なる明示指定（[1m] を含んでいても）は opt-in として尊重
        std::fs::write(&path, "model: claude-fable-5[1m]\neffort: max\n").unwrap();
        assert!(migrate_legacy_model_file(&path).is_none());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("claude-fable-5[1m]"), "ファイルは無変更");

        // model 無しのファイルも no-op
        std::fs::write(&path, "effort: max\n").unwrap();
        assert!(migrate_legacy_model_file(&path).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_skips_when_backup_exists_after_user_re_set() {
        // Issue #67: profiles set で [1m] を再設定 → master 起動相当 → model が保持される
        let tmp = std::env::temp_dir().join("tako-test-migrate-issue67");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("default.yaml");

        // 1. 初回マイグレーション（旧既定値がある状態）
        std::fs::write(&path, "model: claude-opus-4-6[1m]\neffort: high\n").unwrap();
        assert!(migrate_legacy_model_file(&path).is_some());
        let backup = path.with_extension("yaml.backup-1m");
        assert!(backup.is_file(), "backup が作成される");

        // 2. ユーザーが profiles set で [1m] を意図的に再設定
        std::fs::write(&path, "model: claude-opus-4-6[1m]\neffort: high\n").unwrap();

        // 3. 次の master 起動（migrate が再度呼ばれる）→ スキップされる
        assert!(
            migrate_legacy_model_file(&path).is_none(),
            "backup 存在時はマイグレーションをスキップ"
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("model: claude-opus-4-6[1m]"),
            "ユーザーが再設定した model は保持される"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn migrate_model_only_file() {
        let tmp = std::env::temp_dir().join("tako-test-migrate-only");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("default.yaml");
        std::fs::write(&path, "model: claude-opus-4-6[1m]\n").unwrap();
        assert!(migrate_legacy_model_file(&path).is_some());
        let migrated = std::fs::read_to_string(&path).unwrap();
        let p: Profile = serde_yaml::from_str(&migrated).unwrap();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "max", "serde default で補われる");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn solo_default_profile_values() {
        let p = solo_default_profile();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, SOLO_DEFAULT_EFFORT);
        assert_eq!(p.effort, "high");
    }

    #[test]
    fn solo_default_profile_yaml_parses() {
        let p: Profile = serde_yaml::from_str(SOLO_DEFAULT_PROFILE_YAML).unwrap();
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "high");
    }

    #[test]
    fn build_solo_system_prompt_basic() {
        let p = solo_default_profile();
        let prompt = p.build_solo_system_prompt("test");
        assert!(prompt.contains("Solo Agent"));
        assert!(prompt.contains("Eco Mode"));
        assert!(prompt.contains("No orchestration"));
        // projects.yaml を把握して cd 無しで話せる（FR 要件・AC4）
        assert!(prompt.contains("Project Awareness"));
        assert!(prompt.contains("tako_orchestrator_projects"));
        assert!(prompt.contains("Session Identity"));
        assert!(prompt.contains("solo"));
        assert!(prompt.contains(CLAUDE_DEFAULT_LABEL));
        assert!(!prompt.contains("Master Orchestrator"));
    }

    #[test]
    fn build_solo_system_prompt_with_model() {
        let p = Profile {
            model: Some("claude-sonnet-5".into()),
            effort: "medium".into(),
            ..Default::default()
        };
        let prompt = p.build_solo_system_prompt("fast");
        assert!(prompt.contains("claude-sonnet-5"));
        assert!(prompt.contains("medium"));
        assert!(prompt.contains("`fast`"));
    }

    #[test]
    fn solo_prompt_blocks_customization() {
        let p = Profile {
            effort: "high".into(),
            prompt_blocks: Some(PromptBlocks {
                disable: vec!["eco".into()],
                append: Some("SOLO_APPEND_MARKER".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let prompt = p.build_solo_system_prompt("custom");
        assert!(
            !prompt.contains("Eco Mode"),
            "eco ブロックは disable される"
        );
        assert!(prompt.contains("SOLO_APPEND_MARKER"));
        assert!(prompt.contains("Solo Agent"), "role ブロックは維持");
    }
}

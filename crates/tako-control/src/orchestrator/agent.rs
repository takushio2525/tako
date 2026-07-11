//! orchestrator::agent — worker エージェント種別（claude / codex / agy）の抽象（Issue #120）
//!
//! worker として起動できるコーディングエージェント CLI を種別ごとに定義する。
//! コマンド組み立て・事前信頼の書き込み先・effort の写像が種別で異なる。
//! TUI の画面検出（入力欄 / 信頼ダイアログ / busy）は `claude_tui` 側で
//! 3 種のパターンの**和集合**として実装しており、送達フローはエージェント非依存。
//!
//! 実 CLI（codex 0.144.1 / agy 1.1.0）の画面採取に基づく差分は Issue #120 の表を参照。

use std::path::Path;

use serde_json::json;

/// worker として起動できるエージェント CLI の種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerAgent {
    /// Claude Code（既定。`claude agents --json` による一次 status シグナルあり）
    #[default]
    Claude,
    /// OpenAI Codex CLI（status は画面推定。effort は `-c model_reasoning_effort=` へ写像）
    Codex,
    /// Antigravity CLI（status は画面推定。effort 指定なし = モデル名に組込み。
    /// 既定でサブコマンド毎に許可ダイアログが出るため skip_permissions が実用上ほぼ必須）
    Agy,
}

impl WorkerAgent {
    pub const ALL: [WorkerAgent; 3] = [Self::Claude, Self::Codex, Self::Agy];

    /// 種別名（設定ファイル・MCP / CLI 引数・コマンド名と同一表記）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    /// 種別名からパースする。未対応の名前は対応一覧つきのエラー
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "agy" => Ok(Self::Agy),
            other => Err(format!(
                "未対応のエージェント種別 '{other}'（対応: claude / codex / agy）"
            )),
        }
    }

    /// `claude agents --json` による一次 status シグナルを持つか。
    /// 持たない種別（codex / agy）の worker_status は画面推定にフォールバックする
    pub fn has_agents_api(&self) -> bool {
        matches!(self, Self::Claude)
    }

    /// プロファイルで明示設定されていない場合の skip_permissions 既定値。
    /// codex / agy は承認ダイアログで worker が停止するため既定でスキップする。
    /// claude は従来どおり承認あり（auto accept は Claude Code 側の設定に委ねる）
    pub fn default_skip_permissions(&self) -> bool {
        match self {
            Self::Claude => false,
            Self::Codex | Self::Agy => true,
        }
    }
}

/// worker 起動コマンドの組み立てパラメータ
#[derive(Debug, Default)]
pub struct WorkerLaunch<'a> {
    pub agent: WorkerAgent,
    /// TAKO_ORCHESTRATOR_ROLE の値（codex / agy は読まないが識別用に一律注入する）
    pub role: &'a str,
    pub model: Option<&'a str>,
    /// thinking / reasoning effort。claude は `--effort`、codex は
    /// `-c model_reasoning_effort=`、agy は指定手段が無いため無視される
    pub effort: Option<&'a str>,
    /// 許可プロンプトのスキップ（claude / agy: `--dangerously-skip-permissions`、
    /// codex: `--dangerously-bypass-approvals-and-sandbox`）。codex / agy は既定 true
    pub skip_permissions: bool,
    /// プロファイル worker_agents.<agent>.args の追加 CLI 引数（上級者向け）
    pub extra_args: &'a [String],
}

/// worker 起動用のシェルコマンドを組み立てる。
/// claude の従来出力（`build_worker_claude_cmd`）と互換（skip_permissions /
/// extra_args 未使用時は既存文字列と一致する）
pub fn build_worker_cmd(launch: &WorkerLaunch) -> String {
    let mut cmd = format!(
        "TAKO_ORCHESTRATOR_ROLE={} {}",
        sh_quote(launch.role),
        launch.agent.as_str()
    );
    if let Some(model) = launch.model {
        cmd.push_str(&format!(" --model {}", sh_quote(model)));
    }
    if let Some(effort) = launch.effort {
        match launch.agent {
            WorkerAgent::Claude => cmd.push_str(&format!(" --effort {effort}")),
            WorkerAgent::Codex => {
                cmd.push_str(&format!(" -c model_reasoning_effort={}", sh_quote(effort)))
            }
            // agy に effort 指定は無い（モデル名の "(High)" 等に組込み）
            WorkerAgent::Agy => {}
        }
    }
    if launch.skip_permissions {
        match launch.agent {
            WorkerAgent::Claude | WorkerAgent::Agy => {
                cmd.push_str(" --dangerously-skip-permissions")
            }
            WorkerAgent::Codex => cmd.push_str(" --dangerously-bypass-approvals-and-sandbox"),
        }
    }
    for arg in launch.extra_args {
        cmd.push(' ');
        cmd.push_str(&sh_quote(arg));
    }
    cmd
}

/// 単一引数のシェルクオート。英数と安全な記号のみなら素通し、
/// それ以外（role のコロン・agy モデル名の空白等）は single quote で囲む
/// （内部の `'` は `'\''`）
pub(crate) fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

// --- 事前信頼（spawn 前にダイアログ自体を出さない。Issue #32 の各 CLI 対応） ---

/// エージェント種別に応じた事前信頼を書き込む。
/// いずれも best-effort: 失敗しても PromptFlow のダイアログ検出 → 承諾が
/// フォールバックするため、呼び出し側は警告ログのみで継続する
pub fn ensure_trusted(agent: WorkerAgent, cwd: &str) -> Result<bool, String> {
    match agent {
        WorkerAgent::Claude => crate::claude_tui::ensure_trusted(cwd),
        WorkerAgent::Codex => ensure_codex_trusted(cwd),
        WorkerAgent::Agy => ensure_agy_trusted(cwd),
    }
}

/// codex: `~/.codex/config.toml` に `[projects."<cwd>"] trust_level = "trusted"` を追記する
fn ensure_codex_trusted(cwd: &str) -> Result<bool, String> {
    let home = crate::orchestrator::home_dir().ok_or("ホームディレクトリを特定できない")?;
    ensure_codex_trusted_at(&home.join(".codex/config.toml"), cwd)
}

fn ensure_codex_trusted_at(path: &Path, cwd: &str) -> Result<bool, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{} を読めない: {e}", path.display())),
    };
    let header = format!("[projects.{}]", toml_quote(cwd));
    // セクションが既に存在すれば触らない（trusted ならスキップ、ユーザーが明示的に
    // untrusted にした場合も尊重する。同名テーブルの重複追記は TOML エラーになるため）
    if content.lines().any(|l| l.trim() == header) {
        return Ok(true);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    }
    let mut updated = content;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!("\n{header}\ntrust_level = \"trusted\"\n"));
    // codex 本体も読み書きするファイルのため、一時ファイル + rename で原子的に置き換える
    let tmp = path.with_extension("toml.tako-tmp");
    std::fs::write(&tmp, &updated).map_err(|e| format!("{} を書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} を置換できない: {e}", path.display()))?;
    Ok(true)
}

/// TOML の basic string としてクオートする（`"` と `\` をエスケープ）
pub(crate) fn toml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// agy: `~/.gemini/antigravity-cli/settings.json` の `trustedWorkspaces` 配列に cwd を追加する
fn ensure_agy_trusted(cwd: &str) -> Result<bool, String> {
    let home = crate::orchestrator::home_dir().ok_or("ホームディレクトリを特定できない")?;
    ensure_agy_trusted_at(&home.join(".gemini/antigravity-cli/settings.json"), cwd)
}

fn ensure_agy_trusted_at(path: &Path, cwd: &str) -> Result<bool, String> {
    let mut root: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| format!("{} を解釈できない: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{} を読めない: {e}", path.display())),
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{} のトップレベルがオブジェクトでない", path.display()))?;
    let list = obj
        .entry("trustedWorkspaces")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("{} の trustedWorkspaces が配列でない", path.display()))?;
    if list.iter().any(|v| v.as_str() == Some(cwd)) {
        return Ok(true); // 既に信頼済み（書き込み不要）
    }
    list.push(json!(cwd));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    }
    // agy 本体も読み書きするファイルのため、一時ファイル + rename で原子的に置き換える
    let tmp = path.with_extension("json.tako-tmp");
    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("設定を直列化できない: {e}"))?;
    std::fs::write(&tmp, serialized).map_err(|e| format!("{} を書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} を置換できない: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_parse_roundtrip() {
        for agent in WorkerAgent::ALL {
            assert_eq!(WorkerAgent::parse(agent.as_str()), Ok(agent));
        }
        assert_eq!(WorkerAgent::parse(" codex "), Ok(WorkerAgent::Codex));
    }

    #[test]
    fn agent_parse_rejects_unknown() {
        let err = WorkerAgent::parse("gemini").unwrap_err();
        assert!(err.contains("gemini"));
        assert!(
            err.contains("claude / codex / agy"),
            "対応一覧を含む: {err}"
        );
        assert!(WorkerAgent::parse("").is_err());
    }

    #[test]
    fn claude_cmd_matches_legacy_format() {
        // 既存 build_worker_claude_cmd とシェル解釈後に等価なコマンドになる
        // （モデル名のクオートは安全文字のみの場合省く。role は常にクオート）
        let cmd = build_worker_cmd(&WorkerLaunch {
            agent: WorkerAgent::Claude,
            role: "worker:demo",
            model: Some("claude-sonnet-5"),
            effort: Some("high"),
            ..Default::default()
        });
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:demo' claude --model claude-sonnet-5 --effort high"
        );
    }

    #[test]
    fn codex_cmd_maps_effort_to_config_override() {
        let cmd = build_worker_cmd(&WorkerLaunch {
            agent: WorkerAgent::Codex,
            role: "worker:demo",
            model: Some("gpt-5.6-terra"),
            effort: Some("medium"),
            ..Default::default()
        });
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:demo' codex --model gpt-5.6-terra -c model_reasoning_effort=medium"
        );
    }

    #[test]
    fn agy_cmd_ignores_effort_and_quotes_model() {
        // agy のモデル名は空白・括弧入りの表示名（"Gemini 3.5 Flash (High)" 等）
        let cmd = build_worker_cmd(&WorkerLaunch {
            agent: WorkerAgent::Agy,
            role: "worker:demo",
            model: Some("Gemini 3.5 Flash (High)"),
            effort: Some("high"),
            ..Default::default()
        });
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:demo' agy --model 'Gemini 3.5 Flash (High)'"
        );
        assert!(!cmd.contains("effort"), "agy に effort は渡さない");
    }

    #[test]
    fn skip_permissions_flag_per_agent() {
        let base = |agent| WorkerLaunch {
            agent,
            role: "worker:x",
            skip_permissions: true,
            ..Default::default()
        };
        assert!(build_worker_cmd(&base(WorkerAgent::Claude))
            .ends_with("claude --dangerously-skip-permissions"));
        assert!(build_worker_cmd(&base(WorkerAgent::Agy))
            .ends_with("agy --dangerously-skip-permissions"));
        assert!(build_worker_cmd(&base(WorkerAgent::Codex))
            .ends_with("codex --dangerously-bypass-approvals-and-sandbox"));
    }

    #[test]
    fn extra_args_are_quoted_and_appended() {
        let args = vec!["--search".to_string(), "has space".to_string()];
        let cmd = build_worker_cmd(&WorkerLaunch {
            agent: WorkerAgent::Codex,
            role: "worker:x",
            extra_args: &args,
            ..Default::default()
        });
        assert!(cmd.ends_with("codex --search 'has space'"));
    }

    #[test]
    fn model_and_effort_omitted_when_none() {
        for agent in WorkerAgent::ALL {
            let cmd = build_worker_cmd(&WorkerLaunch {
                agent,
                role: "worker:x",
                ..Default::default()
            });
            assert!(!cmd.contains("--model"));
            assert!(!cmd.contains("effort"));
            assert!(cmd.starts_with("TAKO_ORCHESTRATOR_ROLE='worker:x' "));
        }
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("simple-model_1.0"), "simple-model_1.0");
        assert_eq!(sh_quote("with space"), "'with space'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn codex_trust_appends_section_once() {
        let dir = std::env::temp_dir().join(format!("tako-codex-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[tui]\ntheme = \"dark\"\n").unwrap();

        assert_eq!(ensure_codex_trusted_at(&path, "/work/proj"), Ok(true));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[tui]"), "既存セクションを保持");
        assert!(content.contains("[projects.\"/work/proj\"]"));
        assert!(content.contains("trust_level = \"trusted\""));

        // 冪等（重複追記しない = TOML の同名テーブル再定義エラーを起こさない）
        assert_eq!(ensure_codex_trusted_at(&path, "/work/proj"), Ok(true));
        let content2 = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content2.matches("[projects.\"/work/proj\"]").count(),
            1,
            "セクションは 1 回だけ"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_trust_creates_file_when_missing() {
        let dir = std::env::temp_dir().join(format!("tako-codex-trust-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        assert_eq!(ensure_codex_trusted_at(&path, "/fresh"), Ok(true));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[projects.\"/fresh\"]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_trust_appends_to_workspaces() {
        let dir = std::env::temp_dir().join(format!("tako-agy-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"enableTelemetry":false,"trustedWorkspaces":["/existing"]}"#,
        )
        .unwrap();

        assert_eq!(ensure_agy_trusted_at(&path, "/new/proj"), Ok(true));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["enableTelemetry"], false, "無関係キーを保持");
        let list: Vec<&str> = root["trustedWorkspaces"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(list, vec!["/existing", "/new/proj"]);

        // 冪等
        assert_eq!(ensure_agy_trusted_at(&path, "/new/proj"), Ok(true));
        let root2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root2["trustedWorkspaces"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_trust_creates_file_when_missing() {
        let dir = std::env::temp_dir().join(format!("tako-agy-trust-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert_eq!(ensure_agy_trusted_at(&path, "/fresh"), Ok(true));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["trustedWorkspaces"][0], "/fresh");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

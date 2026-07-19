//! sessions — セッションカタログ（Issue #112 A）
//!
//! claude が `~/.claude/projects/<dir>/<uuid>.jsonl` に永続化している会話ログへの
//! **参照**（session_id）とメタデータだけを記録するインデックス。会話本文は保存しない
//! （二重保存の回避。Issue #112 の設計方針）。解決する問題は保存ではなく**発見性**:
//! `claude --resume` の一覧は起動ディレクトリ単位 + 冒頭プロンプト抜粋のみで、
//! tako の worker は冒頭が全部同一テンプレのため見分けられない。
//!
//! 記録の流れ:
//! 1. spawn / master / solo 起動時: session_id はまだ無いため、tmux バックエンド
//!    セッション名をキーに **pending 記録**（project / label / prompt 由来の Issue 番号等）
//!    を残す（spawn は dispatch 側、master / solo はペイン role からの解析で足りるため省略）
//! 2. GUI の定期スキャン（`claude agents --json` × pid 祖先辿り）が session_id を
//!    検出した時点で pending をエントリへ**昇格**し、ペインのメタ情報と統合する
//! 3. `tako sessions list / show / resume` と MCP `tako_sessions` が参照する
//!
//! ファイルは `<data_dir>/sessions.yaml`（`TAKO_SESSIONS_FILE` で上書き可。隔離検証用）。
//! 書き込みは config_io（排他 flock + アトミック書き込み + 世代バックアップ。#169）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// カタログに保持する最大エントリ数（last_seen_at の新しい順に残す）
const MAX_ENTRIES: usize = 500;

/// pending 記録の保持期間（秒）。session_id が検出されないまま古びたものを掃除する
/// （codex / agy worker は claude の session 検出に乗らないため、寿命付きで残す）
const PENDING_TTL_SECS: i64 = 7 * 24 * 3600;

/// カタログファイルのパス（`TAKO_SESSIONS_FILE` 上書き → `<data_dir>/sessions.yaml`）
pub fn catalog_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_SESSIONS_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    tako_core::paths::data_dir().map(|d| d.join("sessions.yaml"))
}

/// カタログ本体（sessions.yaml のスキーマ）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionCatalog {
    /// session_id → エントリ
    #[serde(default)]
    pub entries: BTreeMap<String, SessionEntry>,
    /// session_id 検出前の spawn 記録（tmux バックエンドセッション名がキー）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingSpawn>,
}

/// カタログの 1 エントリ（会話本文は持たない。claude jsonl への参照 + メタデータのみ）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// 種別: master / worker / solo / pane（手動起動などロール無しペイン）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// エージェント種別（claude / codex / agy。session_id 検出経路は claude のみ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// プロンプト・ラベルから抽出した Issue 番号（`#123` 形式）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    /// spawn プロンプトの冒頭抜粋（発見性のため。ローカルファイル限定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    /// 会話の作業ディレクトリ（resume 時の起動 cwd。claude agents の cwd を優先）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    /// tmux バックエンドセッション名（最後に観測したもの）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    /// このセッションのペインログファイル（Issue #112 B との突き合わせ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_seen_at: String,
}

/// spawn 時点の記録（session_id 検出前）。キーは tmux バックエンドセッション名
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingSpawn {
    pub tmux_session: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recorded_at: String,
}

impl SessionCatalog {
    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("sessions.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("sessions.yaml のパースに失敗: {e}"))
    }

    pub fn load() -> Result<Self, String> {
        let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同型）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut catalog = Self::load_from(path)?;
        let result = f(&mut catalog);
        let content = serde_yaml::to_string(&catalog)
            .map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    /// エントリを last_seen_at の新しい順に返す
    pub fn sorted_entries(&self) -> Vec<(&String, &SessionEntry)> {
        let mut items: Vec<_> = self.entries.iter().collect();
        items.sort_by(|a, b| b.1.last_seen_at.cmp(&a.1.last_seen_at));
        items
    }

    /// id の前方一致でエントリを解決する。複数一致は候補一覧つきのエラー
    pub fn resolve_id(&self, id_prefix: &str) -> Result<(&String, &SessionEntry), String> {
        // 完全一致を最優先（他 ID の前方部分と衝突しても曖昧にならない）
        if let Some((id, entry)) = self.entries.get_key_value(id_prefix) {
            return Ok((id, entry));
        }
        let matches: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, _)| id.starts_with(id_prefix))
            .collect();
        match matches.len() {
            0 => Err(format!(
                "セッション '{id_prefix}' がカタログに見つからない（tako sessions list で確認）"
            )),
            1 => Ok(matches[0]),
            n => Err(format!(
                "セッション '{id_prefix}' の候補が {n} 件ある（もう少し長い ID を指定）: {}",
                matches
                    .iter()
                    .map(|(id, _)| short_id(id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// spawn 時の pending 記録（dispatch_orchestrator_spawn から呼ぶ）。
/// 同じ tmux セッション名の既存 pending は置き換える。失敗は呼び出し側で警告のみ
/// （カタログの失敗で spawn を止めない）
pub fn record_spawn(record: PendingSpawn) -> Result<(), String> {
    SessionCatalog::mutate(|catalog| {
        catalog
            .pending
            .retain(|p| p.tmux_session != record.tmux_session);
        catalog.pending.push(record);
    })
}

/// GUI の定期スキャンが検出した claude セッション 1 件分
#[derive(Debug, Clone)]
pub struct DetectedSession {
    pub session_id: String,
    /// tmux バックエンドセッション名（pending / ペインとの対応キー）
    pub tmux_session: String,
    /// claude agents --json の cwd（resume の起動ディレクトリとして最優先）
    pub agent_cwd: Option<String>,
    pub model: Option<String>,
}

/// スキャン時点のペインのメタ情報（GUI が workspace から収集する）
#[derive(Debug, Clone, Default)]
pub struct PaneMetaSnapshot {
    pub pane: u64,
    pub tab: u64,
    pub tmux_session: String,
    pub role: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// このペインの現在のペインログファイル（Issue #112 B）
    pub log_file: Option<String>,
}

/// `agents::list_agents_with_panes` の結果（`{"agents": [...]}`）から検出リストを作る。
/// pane 対応（`session:window.pane`）と有効な session_id を持つエントリだけを拾う
pub fn detect_from_agents_value(value: &Value) -> Vec<DetectedSession> {
    value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            let pane = agent["pane"].as_str()?;
            let backend = pane.split_once(':')?.0;
            let session_id = agent["session_id"].as_str()?;
            if !crate::transcript::is_valid_session_id(session_id) {
                return None;
            }
            Some(DetectedSession {
                session_id: session_id.to_string(),
                tmux_session: backend.to_string(),
                agent_cwd: agent["cwd"].as_str().map(str::to_string),
                model: agent["model"].as_str().map(str::to_string),
            })
        })
        .collect()
}

/// 検出結果をカタログへ同期する（pending の昇格 + エントリ更新 + GC）。
/// GUI の定期スキャン（background）から呼ばれる
pub fn sync_detected(
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
) -> Result<(), String> {
    let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
    sync_detected_at(&path, detected, panes)
}

/// パス指定版 sync（テスト用に公開）
pub fn sync_detected_at(
    path: &Path,
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
) -> Result<(), String> {
    if detected.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    SessionCatalog::mutate_at(path, |catalog| {
        for d in detected {
            if !crate::transcript::is_valid_session_id(&d.session_id) {
                continue;
            }
            let pane_meta = panes.iter().find(|p| p.tmux_session == d.tmux_session);
            let pending_idx = catalog
                .pending
                .iter()
                .position(|p| p.tmux_session == d.tmux_session);
            let pending = pending_idx.map(|i| catalog.pending.remove(i));

            let entry = catalog.entries.entry(d.session_id.clone()).or_default();
            if entry.started_at.is_empty() {
                entry.started_at = pending
                    .as_ref()
                    .map(|p| p.recorded_at.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| now.clone());
            }
            entry.last_seen_at = now.clone();
            entry.tmux_session = Some(d.tmux_session.clone());
            if d.agent_cwd.is_some() {
                entry.cwd = d.agent_cwd.clone();
            }
            if d.model.is_some() {
                entry.model = d.model.clone();
            }
            // spawn 時のメタ（プロンプト由来の情報）は pending が最も正確
            if let Some(p) = pending {
                entry.kind = p.kind;
                entry.label = p.label.or(entry.label.take());
                entry.project = p.project.or(entry.project.take());
                entry.agent = p.agent.or(entry.agent.take());
                if entry.model.is_none() {
                    entry.model = p.model;
                }
                entry.effort = p.effort.or(entry.effort.take());
                if !p.issues.is_empty() {
                    entry.issues = p.issues;
                }
                entry.prompt_head = p.prompt_head.or(entry.prompt_head.take());
                if entry.cwd.is_none() {
                    entry.cwd = p.cwd;
                }
            }
            // ペインの現在情報で補完・更新する
            if let Some(meta) = pane_meta {
                entry.pane = Some(meta.pane);
                entry.tab = Some(meta.tab);
                if let Some(role) = meta.role.as_deref() {
                    let parsed = parse_role(role);
                    if entry.kind.is_empty() {
                        entry.kind = parsed.kind.to_string();
                    }
                    if entry.project.is_none() {
                        entry.project = parsed.project;
                    }
                    if entry.label.is_none() {
                        entry.label = parsed.label;
                    }
                    if entry.profile.is_none() {
                        entry.profile = parsed.profile;
                    }
                }
                if entry.label.is_none() {
                    entry.label = meta.title.clone();
                }
                if entry.cwd.is_none() {
                    entry.cwd = meta.cwd.clone();
                }
                if meta.log_file.is_some() {
                    entry.log_file = meta.log_file.clone();
                }
            }
            if entry.kind.is_empty() {
                entry.kind = "pane".into();
            }
            if entry.agent.is_none() {
                entry.agent = Some("claude".into());
            }
            if entry.issues.is_empty() {
                let text = format!(
                    "{} {}",
                    entry.label.as_deref().unwrap_or(""),
                    entry.prompt_head.as_deref().unwrap_or("")
                );
                entry.issues = extract_issues(&text);
            }
        }
        gc(catalog, &now);
    })
}

/// pending の期限切れ掃除 + エントリ数の上限強制
fn gc(catalog: &mut SessionCatalog, now: &str) {
    let cutoff = parse_iso(now).unwrap_or(0) - PENDING_TTL_SECS;
    catalog
        .pending
        .retain(|p| parse_iso(&p.recorded_at).unwrap_or(i64::MAX) >= cutoff);
    if catalog.entries.len() > MAX_ENTRIES {
        let mut ids: Vec<(String, String)> = catalog
            .entries
            .iter()
            .map(|(id, e)| (e.last_seen_at.clone(), id.clone()))
            .collect();
        ids.sort(); // last_seen_at 昇順 = 古い順
        let drop_count = catalog.entries.len() - MAX_ENTRIES;
        for (_, id) in ids.into_iter().take(drop_count) {
            catalog.entries.remove(&id);
        }
    }
}

/// role 文字列の解析結果
#[derive(Debug, PartialEq)]
pub struct ParsedRole {
    pub kind: &'static str,
    pub project: Option<String>,
    pub label: Option<String>,
    pub profile: Option<String>,
}

/// ペインの role 文字列（spawn / master / solo が設定する）を分類する。
/// - `orchestrator-master[:suffix]` → master（suffix = プロファイル）
/// - `solo[:suffix]` → solo
/// - `orchestrator-worker:<project>[:<label>]` → worker
/// - それ以外 → pane
pub fn parse_role(role: &str) -> ParsedRole {
    if let Some(rest) = role.strip_prefix("orchestrator-worker") {
        let mut it = rest.strip_prefix(':').unwrap_or("").splitn(2, ':');
        let project = it.next().filter(|s| !s.is_empty()).map(str::to_string);
        let label = it.next().filter(|s| !s.is_empty()).map(str::to_string);
        return ParsedRole {
            kind: "worker",
            project,
            label,
            profile: None,
        };
    }
    if let Some(rest) = role.strip_prefix("orchestrator-master") {
        let profile = rest.strip_prefix(':').filter(|s| !s.is_empty());
        return ParsedRole {
            kind: "master",
            project: None,
            label: None,
            profile: Some(profile.unwrap_or("default").to_string()),
        };
    }
    if role == "solo" || role.starts_with("solo:") {
        let profile = role.strip_prefix("solo:").filter(|s| !s.is_empty());
        return ParsedRole {
            kind: "solo",
            project: None,
            label: None,
            profile: Some(profile.unwrap_or("default").to_string()),
        };
    }
    ParsedRole {
        kind: "pane",
        project: None,
        label: None,
        profile: None,
    }
}

/// テキストから Issue 番号（`#123`）を抽出する（昇順・重複なし）
pub fn extract_issues(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start && end - start <= 6 {
                if let Ok(n) = text[start..end].parse::<u32>() {
                    if n > 0 && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out.sort_unstable();
    out
}

/// プロンプト冒頭の抜粋（1 行に正規化して最大 `max` 文字）
pub fn prompt_head(prompt: &str, max: usize) -> String {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let head: String = flat.chars().take(max).collect();
        format!("{head}…")
    }
}

/// 表示用の短縮 ID（先頭 8 文字）
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// `list` の応答ペイロード（CLI / MCP 共通）
pub fn list_payload(
    role: Option<&str>,
    project: Option<&str>,
    limit: usize,
) -> Result<Value, String> {
    let catalog = SessionCatalog::load()?;
    let sessions: Vec<Value> = catalog
        .sorted_entries()
        .into_iter()
        .filter(|(_, e)| role.is_none_or(|r| e.kind == r))
        .filter(|(_, e)| project.is_none_or(|p| e.project.as_deref() == Some(p)))
        .take(limit)
        .map(|(id, e)| {
            json!({
                "session_id": id,
                "short_id": short_id(id),
                "kind": e.kind,
                "label": e.label,
                "project": e.project,
                "profile": e.profile,
                "agent": e.agent,
                "model": e.model,
                "issues": e.issues,
                "cwd": e.cwd,
                "tab": e.tab,
                "pane": e.pane,
                "tmux_session": e.tmux_session,
                "log_file": e.log_file,
                "started_at": e.started_at,
                "last_seen_at": e.last_seen_at,
                "resumable": crate::transcript::find_transcript(id).is_some(),
            })
        })
        .collect();
    // codex / agy など session_id 未検出の spawn 記録（resume 不可の制限つきで可視化）
    let pending: Vec<Value> = catalog
        .pending
        .iter()
        .map(|p| {
            json!({
                "tmux_session": p.tmux_session,
                "kind": p.kind,
                "label": p.label,
                "project": p.project,
                "agent": p.agent,
                "issues": p.issues,
                "recorded_at": p.recorded_at,
            })
        })
        .collect();
    Ok(json!({
        "sessions": sessions,
        "pending": pending,
        "catalog_path": catalog_path(),
    }))
}

/// `show` の応答ペイロード（メタ + 会話冒頭の抜粋 + transcript 情報）
pub fn show_payload(id_prefix: &str) -> Result<Value, String> {
    let catalog = SessionCatalog::load()?;
    let (id, entry) = catalog.resolve_id(id_prefix)?;
    let transcript = crate::transcript::find_transcript(id);
    let transcript_info = transcript.as_ref().map(|path| {
        let meta = std::fs::metadata(path).ok();
        json!({
            "path": path,
            "size": meta.as_ref().map(|m| m.len()),
        })
    });
    let first_user = crate::transcript::first_user_text(id, 300);
    Ok(json!({
        "session_id": id,
        "entry": serde_json::to_value(entry).unwrap_or(Value::Null),
        "resumable": transcript.is_some(),
        "transcript": transcript_info,
        "first_user_message": first_user,
    }))
}

/// resume 用の起動コマンドを組み立てる（claude のみ。Issue #112 の制限:
/// codex / agy は session 参照手段が tako から安定して取れないため対象外）
pub fn resume_command(id: &str, entry: &SessionEntry) -> Result<String, String> {
    let agent = entry.agent.as_deref().unwrap_or("claude");
    if agent != "claude" {
        return Err(format!(
            "agent '{agent}' のセッションは resume 非対応（claude のみ。codex は `codex resume`、agy は `agy --conversation` を手動で実行）"
        ));
    }
    if !crate::transcript::is_valid_session_id(id) {
        return Err("session_id の形式が不正".into());
    }
    let role_env = match entry.kind.as_str() {
        "worker" => {
            let project = entry.project.as_deref().unwrap_or("resumed");
            match entry.label.as_deref() {
                Some(l) => Some(format!("worker:{project}:{l}")),
                None => Some(format!("worker:{project}")),
            }
        }
        "master" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("master:{p}"),
            _ => "master".into(),
        }),
        "solo" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("solo:{p}"),
            _ => "solo".into(),
        }),
        _ => None,
    };
    let mut cmd = String::new();
    if let Some(role) = role_env {
        cmd.push_str(&format!(
            "TAKO_ORCHESTRATOR_ROLE={} ",
            crate::orchestrator::agent::sh_quote(&role)
        ));
    }
    cmd.push_str("claude");
    if let Some(model) = entry.model.as_deref() {
        cmd.push_str(&format!(
            " --model {}",
            crate::orchestrator::agent::sh_quote(model)
        ));
    }
    if let Some(effort) = entry.effort.as_deref() {
        cmd.push_str(&format!(" --effort {effort}"));
    }
    cmd.push_str(&format!(" --resume {id}"));
    Ok(cmd)
}

/// 現在時刻の ISO 表記（カタログの記録時刻用）
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    crate::diag::format_utc(secs)
}

/// `YYYY-MM-DDTHH:MM:SSZ` → unix 秒（GC の期限計算・レジストリの経過時間判定用。失敗は None）
pub(crate) fn parse_iso(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // days_from_civil（civil_utc の逆変換）
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// ペイン ID から session_id を逆引きする（カタログの pane フィールドを使用。#284）。
/// カタログが無い・読めない場合は None
pub fn resolve_session_for_pane(pane_id: &str) -> Option<String> {
    let pane_num: u64 = pane_id.parse().ok()?;
    let catalog = SessionCatalog::load().ok()?;
    for (session_id, entry) in &catalog.entries {
        if entry.pane == Some(pane_num) {
            return Some(session_id.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-sessions-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("sessions.yaml")
    }

    #[test]
    fn agents一覧からの検出はpane対応と有効idだけを拾う() {
        // 旧 agents::claude_session_ids_by_backend のマッピング検証を新 API で維持
        let value = serde_json::json!({"agents": [
            {"pane": "tako-a:0.0", "session_id": "session-a", "cwd": "/w/a", "model": "claude-fable-5"},
            {"pane": "tako-b:1.2", "session_id": "session-b"},
            {"pane": "tako-c:0.0", "session_id": "../../invalid"},
            {"session_id": "pane-missing"},
        ]});
        let detected = detect_from_agents_value(&value);
        assert_eq!(detected.len(), 2);
        assert_eq!(detected[0].session_id, "session-a");
        assert_eq!(detected[0].tmux_session, "tako-a");
        assert_eq!(detected[0].agent_cwd.as_deref(), Some("/w/a"));
        assert_eq!(detected[0].model.as_deref(), Some("claude-fable-5"));
        assert_eq!(detected[1].tmux_session, "tako-b");
        // 空 agents は空
        assert!(detect_from_agents_value(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn issue番号の抽出() {
        assert_eq!(
            extract_issues("Issue #112 を実装。#157 と衝突注意"),
            vec![112, 157]
        );
        assert_eq!(extract_issues("#112 #112 #112"), vec![112]);
        assert_eq!(extract_issues("番号なし # だけ"), Vec::<u32>::new());
        assert_eq!(
            extract_issues("色コード #ffffff は数字なら拾う"),
            Vec::<u32>::new()
        );
        assert_eq!(
            extract_issues("fix/112-log は # なしなので拾わない"),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn roleの分類() {
        let w = parse_role("orchestrator-worker:tako:112-session-log");
        assert_eq!(w.kind, "worker");
        assert_eq!(w.project.as_deref(), Some("tako"));
        assert_eq!(w.label.as_deref(), Some("112-session-log"));

        let w2 = parse_role("orchestrator-worker:tako");
        assert_eq!(w2.kind, "worker");
        assert_eq!(w2.project.as_deref(), Some("tako"));
        assert_eq!(w2.label, None);

        let m = parse_role("orchestrator-master");
        assert_eq!(m.kind, "master");
        assert_eq!(m.profile.as_deref(), Some("default"));
        assert_eq!(
            parse_role("orchestrator-master:sol").profile.as_deref(),
            Some("sol")
        );

        assert_eq!(parse_role("solo").kind, "solo");
        assert_eq!(parse_role("solo:fast").profile.as_deref(), Some("fast"));
        assert_eq!(parse_role("dev-server").kind, "pane");
    }

    #[test]
    fn prompt_headの正規化と切り詰め() {
        assert_eq!(prompt_head("a\nb\n  c", 100), "a b c");
        let long = "あ".repeat(50);
        let head = prompt_head(&long, 10);
        assert_eq!(head.chars().count(), 11);
        assert!(head.ends_with('…'));
    }

    #[test]
    fn pending記録と昇格の統合() {
        let path = temp_path("promote");
        // spawn 時の pending 記録
        SessionCatalog::mutate_at(&path, |c| {
            c.pending.push(PendingSpawn {
                tmux_session: "tako-s42".into(),
                kind: "worker".into(),
                label: Some("112-session-log".into()),
                project: Some("tako".into()),
                agent: Some("claude".into()),
                model: None,
                effort: Some("max".into()),
                issues: vec![112],
                prompt_head: Some("Issue #112 を実装する".into()),
                cwd: Some("/work/tako".into()),
                tab: Some(3),
                pane: Some(7),
                recorded_at: "2026-07-13T00:00:00Z".into(),
            });
        })
        .unwrap();

        // 検出 → 昇格（テストは mutate_at ベースの sync を直接再現する）
        let detected = vec![DetectedSession {
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            tmux_session: "tako-s42".into(),
            agent_cwd: Some("/work/tako".into()),
            model: Some("claude-fable-5".into()),
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 7,
            tab: 3,
            tmux_session: "tako-s42".into(),
            role: Some("orchestrator-worker:tako:112-session-log".into()),
            title: Some("tako: 112-session-log".into()),
            cwd: Some("/work/tako".into()),
            log_file: Some("/logs/x.log".into()),
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();

        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert!(catalog.pending.is_empty(), "pending が昇格で消える");
        let entry = &catalog.entries["11111111-2222-3333-4444-555555555555"];
        assert_eq!(entry.kind, "worker");
        assert_eq!(entry.project.as_deref(), Some("tako"));
        assert_eq!(entry.label.as_deref(), Some("112-session-log"));
        assert_eq!(entry.issues, vec![112]);
        assert_eq!(entry.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(entry.effort.as_deref(), Some("max"));
        assert_eq!(entry.cwd.as_deref(), Some("/work/tako"));
        assert_eq!(entry.pane, Some(7));
        assert_eq!(entry.started_at, "2026-07-13T00:00:00Z");
        assert!(!entry.last_seen_at.is_empty());
        assert_eq!(entry.log_file.as_deref(), Some("/logs/x.log"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pending無しでもroleから分類される() {
        let path = temp_path("role-only");
        let detected = vec![DetectedSession {
            session_id: "aaaaaaaa-1111-2222-3333-444444444444".into(),
            tmux_session: "tako-s9".into(),
            agent_cwd: None,
            model: None,
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 2,
            tab: 1,
            tmux_session: "tako-s9".into(),
            role: Some("orchestrator-master:sol".into()),
            title: None,
            cwd: Some("/home/u".into()),
            log_file: None,
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();
        let catalog = SessionCatalog::load_from(&path).unwrap();
        let entry = &catalog.entries["aaaaaaaa-1111-2222-3333-444444444444"];
        assert_eq!(entry.kind, "master");
        assert_eq!(entry.profile.as_deref(), Some("sol"));
        assert_eq!(entry.agent.as_deref(), Some("claude"));
        assert_eq!(entry.cwd.as_deref(), Some("/home/u"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn resolve_idの前方一致と曖昧エラー() {
        let mut catalog = SessionCatalog::default();
        catalog
            .entries
            .insert("abc123".into(), SessionEntry::default());
        catalog
            .entries
            .insert("abd456".into(), SessionEntry::default());
        assert!(catalog.resolve_id("abc").is_ok());
        assert!(catalog.resolve_id("abc123").is_ok());
        let err = catalog.resolve_id("ab").unwrap_err();
        assert!(err.contains("2 件"), "{err}");
        assert!(catalog.resolve_id("zzz").is_err());
    }

    #[test]
    fn resume_commandの組み立てとcodex拒否() {
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("112-log".into()),
            agent: Some("claude".into()),
            model: Some("claude-fable-5".into()),
            effort: Some("max".into()),
            ..Default::default()
        };
        let cmd = resume_command("11111111-2222-3333-4444-555555555555", &entry).unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:tako:112-log' claude --model claude-fable-5 --effort max --resume 11111111-2222-3333-4444-555555555555"
        );

        let solo = SessionEntry {
            kind: "solo".into(),
            profile: Some("default".into()),
            ..Default::default()
        };
        let cmd = resume_command("abc", &solo).unwrap();
        assert!(
            cmd.starts_with("TAKO_ORCHESTRATOR_ROLE=solo claude"),
            "{cmd}"
        );

        let codex = SessionEntry {
            agent: Some("codex".into()),
            ..Default::default()
        };
        let err = resume_command("abc", &codex).unwrap_err();
        assert!(err.contains("resume 非対応"), "{err}");

        let invalid = SessionEntry {
            agent: Some("claude".into()),
            ..Default::default()
        };
        assert!(resume_command("../etc", &invalid).is_err());
    }

    #[test]
    fn gcはpending期限とエントリ上限を強制する() {
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: "old".into(),
            recorded_at: "2026-07-01T00:00:00Z".into(),
            ..Default::default()
        });
        catalog.pending.push(PendingSpawn {
            tmux_session: "fresh".into(),
            recorded_at: "2026-07-13T00:00:00Z".into(),
            ..Default::default()
        });
        for i in 0..(MAX_ENTRIES + 10) {
            catalog.entries.insert(
                format!("session-{i:04}"),
                SessionEntry {
                    last_seen_at: format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
                    ..Default::default()
                },
            );
        }
        gc(&mut catalog, "2026-07-13T12:00:00Z");
        assert_eq!(catalog.pending.len(), 1);
        assert_eq!(catalog.pending[0].tmux_session, "fresh");
        assert_eq!(catalog.entries.len(), MAX_ENTRIES);
        // 最も古い 10 件が消えている
        assert!(!catalog.entries.contains_key("session-0000"));
        assert!(catalog
            .entries
            .contains_key(&format!("session-{:04}", MAX_ENTRIES + 9)));
    }

    #[test]
    fn iso時刻の往復() {
        // civil_utc（format）↔ parse_iso の整合
        let secs = 1_784_000_000; // 2026-07 前後
        let iso = crate::diag::format_utc(secs);
        assert_eq!(parse_iso(&iso), Some(secs));
        assert_eq!(
            parse_iso("2026-07-13T00:00:00Z").map(|s| s % 86_400),
            Some(0)
        );
        assert_eq!(parse_iso("broken"), None);
    }

    #[test]
    fn 破損カタログはerrで丸めない() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "entries:\n  x:\n    kind: [broken").unwrap();
        assert!(SessionCatalog::load_from(&path).is_err());
        let result = SessionCatalog::mutate_at(&path, |c| c.entries.clear());
        assert!(result.is_err(), "破損時は書き込まない");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

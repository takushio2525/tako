//! sessions — セッションカタログ（Issue #112 A）
//!
//! claude が `~/.claude/projects/<dir>/<uuid>.jsonl` に永続化している会話ログへの
//! **参照**（session_id）とメタデータだけを記録するインデックス。会話本文は保存しない
//! （二重保存の回避。Issue #112 の設計方針）。解決する問題は保存ではなく**発見性**:
//! `claude --resume` の一覧は起動ディレクトリ単位 + 冒頭プロンプト抜粋のみで、
//! tako の worker は冒頭が全部同一テンプレのため見分けられない。
//!
//! 記録の流れ:
//! 1. spawn / master / solo 起動時: session_id はまだ無いため **pending 記録**
//!    （project / label / prompt 由来の Issue 番号等）を残す（spawn は dispatch 側、
//!    master / solo はペイン role からの解析で足りるため省略）
//! 2. GUI の定期スキャン（`claude agents --json` × pid 祖先辿り）が session_id を
//!    検出した時点で pending をエントリへ**昇格**し、ペインのメタ情報と統合する
//! 3. `tako sessions list / show / resume` と MCP `tako_sessions` が参照する
//!
//! ## 突き合わせキー（#728）
//!
//! pending と検出結果を結ぶキーは**器（tmux / psmux）のセッション名**。
//! ただし器の導入は任意なので（Windows の psmux・macOS の tmux はどちらも
//! 「入れれば深く復元できる」もの）、器が無い構成では**ペイン ID** に倒す。
//! 器が無いときペインのシェルは tako-app の直接の子なので、claude の対応付けは
//! `TerminalSession::child_pid` からの pid 祖先辿りになる（`agents` の #592 経路）。
//! カタログ本体・復元手順は器に一切依存しない（復元は tako 自身のペイン生成 +
//! `claude --resume` であって、器への送出は要らない）
//!
//! ファイルは `<data_dir>/sessions.yaml`（`TAKO_SESSIONS_FILE` で上書き可。隔離検証用）。
//! 書き込みは config_io（排他 flock + アトミック書き込み + 世代バックアップ。#169）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use tako_core::agent_support::Agent;
use tako_core::platform::support::Platform;

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
    /// session_id 検出前の spawn 記録（キーは器のセッション名 / 器が無ければペイン ID。#728）
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

/// spawn 時点の記録（session_id 検出前）。
///
/// 突き合わせキーは**器のセッション名、器が無ければペイン ID**（#728）。
/// psmux / tmux が無い構成（Windows の既定・tmux 不在の macOS）では器の名前が
/// 付かないので `tmux_session` は `None` になる。キーの解釈は
/// [`PendingSpawn::matches`] に一本化してあり、直接比較してはいけない。
///
/// **旧形式（`tmux_session` が素の文字列）はそのまま読める**: serde は
/// `Option<String>` へ文字列をそのまま入れ、欠けていれば `default` で `None` になる。
/// 移行 Step は要らない（#916 の判断）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingSpawn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
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

impl PendingSpawn {
    /// 検出結果との突き合わせ（#728）。
    ///
    /// 器があるときは**セッション名だけ**で見る（ペイン ID は tako 再起動をまたいで
    /// 振り直されるので、器の名前の方が強い同一性を持つ）。器が無いときだけ
    /// ペイン ID で見る。**両方 None のエントリは何にも一致しない**
    /// （キーの無い記録が最初の検出を横取りするのを防ぐ）
    pub fn matches(&self, session: Option<&str>, pane: Option<u64>) -> bool {
        match (self.tmux_session.as_deref(), session) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => match (self.pane, pane) {
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => false,
            },
        }
    }
}

impl SessionCatalog {
    /// pending 記録を「同じキーのものだけ置き換えて」追加する（#728）。
    ///
    /// キーの解釈は [`PendingSpawn::matches`]（器のセッション名 / 器が無ければペイン ID）。
    /// 単純な `tmux_session` 比較だと、器なし（どちらも `None`）の記録が互いを消し合って
    /// **最後の 1 件しか残らない**。テストが実装をなぞらず**この経路そのもの**を
    /// 通せるように、production の重複排除をここへ切り出してある
    pub fn upsert_pending(&mut self, record: PendingSpawn) {
        self.pending
            .retain(|p| !p.matches(record.tmux_session.as_deref(), record.pane));
        self.pending.push(record);
    }

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
    SessionCatalog::mutate(|catalog| catalog.upsert_pending(record))
}

/// GUI の定期スキャンが検出した claude セッション 1 件分
#[derive(Debug, Clone)]
pub struct DetectedSession {
    pub session_id: String,
    /// 器のセッション名（pending / ペインとの対応キー）。器が無ければ `None`（#728）
    pub tmux_session: Option<String>,
    /// tako のペイン ID。器が無いペインはこれが唯一の対応キーになる（#728）
    pub pane: Option<u64>,
    /// claude agents --json の cwd（resume の起動ディレクトリとして最優先）
    pub agent_cwd: Option<String>,
    pub model: Option<String>,
}

/// スキャン時点のペインのメタ情報（GUI が workspace から収集する）
#[derive(Debug, Clone, Default)]
pub struct PaneMetaSnapshot {
    pub pane: u64,
    pub tab: u64,
    /// 器のセッション名。器を持たないペインは `None`（#728。この場合の対応付けは
    /// 検出側が PTY 直下の子 pid で解決済みなので、ここには pid を持たない）
    pub tmux_session: Option<String>,
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
            let tmux_session = agent["pane"]
                .as_str()
                .and_then(|pane| pane.split_once(':'))
                .map(|(backend, _)| backend.to_string());
            let pane = agent["tako_pane"].as_u64();
            if tmux_session.is_none() && pane.is_none() {
                return None;
            }
            let session_id = agent["session_id"].as_str()?;
            if !crate::transcript::is_valid_session_id(session_id) {
                return None;
            }
            Some(DetectedSession {
                session_id: session_id.to_string(),
                tmux_session,
                pane,
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
            // 対応付けは器のセッション名優先、器が無ければペイン ID（#728）
            let pane_meta = panes
                .iter()
                .find(|p| match (&p.tmux_session, &d.tmux_session) {
                    (Some(mine), Some(theirs)) => mine == theirs,
                    _ => d.pane.is_some_and(|pane| p.pane == pane),
                });
            let pending_idx = catalog
                .pending
                .iter()
                .position(|p| p.matches(d.tmux_session.as_deref(), d.pane));
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
            // 器が無い構成では None のまま（「観測できていない」ではなく「器が無い」）
            if d.tmux_session.is_some() {
                entry.tmux_session = d.tmux_session.clone();
            }
            if let Some(pane) = d.pane {
                entry.pane = Some(pane);
            }
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
                // 器が無い構成の突き合わせキー（#728）。表示・診断のために出す
                "pane": p.pane,
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

/// resume 用の起動コマンドを組み立てる。
///
/// **系統ごとの書式は `tako_core::agent_resume::resume_spec` が正本**（#1238）。
/// claude だけが `--model` / `--effort` を載せられる（`codex resume` は受け取らず、
/// agy は会話に記録された設定を上書きしないため足さない）。
/// 会話が既定以外の config ディレクトリにあれば `CLAUDE_CONFIG_DIR` を前置する（Issue #652）
pub fn resume_command(id: &str, entry: &SessionEntry) -> Result<String, String> {
    let agent = agent_of(entry);
    resume_command_with_env(id, entry, conversation_env(agent, id).as_deref())
}

/// カタログの記録が指す系統（未記録は claude = 既定）
fn agent_of(entry: &SessionEntry) -> Agent {
    entry
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .unwrap_or(Agent::Claude)
}

/// 会話の所在を確かめ、必要なら起動コマンドへ前置きする env を返す（#1238）。
///
/// `None` = **どこにも会話が無い**（resume しても「見つからない」で終わるので起こさない）。
/// claude だけが所在によって `CLAUDE_CONFIG_DIR` の指定を要する（#652）ので、
/// 他系統は「見つかった」を空文字で表す
fn conversation_env(agent: Agent, id: &str) -> Option<String> {
    if agent == Agent::Claude {
        return crate::transcript::resume_env_prefix(id);
    }
    // claude 以外は config ディレクトリの概念が無いので、
    // 「見つかった」を空文字で表す（所在の判定は agent_resume が系統ごとに持つ）
    crate::agent_resume::conversation_exists(agent, id)
        .unwrap_or(false)
        .then(String::new)
}

/// `resume_command` の本体（env プレフィクスを引数で受け取るテスト可能版）
fn resume_command_with_env(
    id: &str,
    entry: &SessionEntry,
    env_prefix: Option<&str>,
) -> Result<String, String> {
    let agent = agent_of(entry);
    let spec = tako_core::agent_resume::resume_spec(agent).ok_or_else(|| {
        format!(
            "agent '{}' のセッションは resume 非対応（対応: claude / codex / agy）",
            agent.as_str()
        )
    })?;
    // 書式検証は全系統共通（パストラバーサル防止。codex / agy の会話 ID も UUID）
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
    // アカウントの config ディレクトリ指定は先頭（rc / direnv より後に効く。#500 / #512 と同型）
    if let Some(prefix) = env_prefix {
        cmd.push_str(prefix);
    }
    if let Some(role) = role_env {
        cmd.push_str(&format!(
            "TAKO_ORCHESTRATOR_ROLE={} ",
            crate::orchestrator::agent::sh_quote(&role)
        ));
    }
    cmd.push_str(spec.program);
    if spec.accepts_launch_flags {
        if let Some(model) = entry.model.as_deref() {
            cmd.push_str(&format!(
                " --model {}",
                crate::orchestrator::agent::sh_quote(model)
            ));
        }
        if let Some(effort) = entry.effort.as_deref() {
            cmd.push_str(&format!(" --effort {effort}"));
        }
    }
    for head in spec.head {
        cmd.push(' ');
        cmd.push_str(head);
    }
    if let Some(flag) = spec.id_flag {
        cmd.push(' ');
        cmd.push_str(flag);
    }
    cmd.push(' ');
    cmd.push_str(id);
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// 復元（PC 再起動後）の 1 ペインぶんの判断（Issue #1076 / #1238）
// ---------------------------------------------------------------------------

/// 新規シェルで開き直す理由（#1076。復元内訳のログに出す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshShellReason {
    /// 保存時に会話を検出できていなかった（= 復元すべき会話が記録されていない）
    NoSessionId,
    /// 保存値の形式が不正（パストラバーサル対策で弾いた）
    InvalidSessionId,
    /// 会話ファイル（transcript / rollout / conversations の db）が無い
    TranscriptMissing,
    /// **その系統をこの環境では復元できない**（#1238）。
    /// 「保存されていない」（`NoSessionId`）と混ぜると原因が読めない
    ResumeUnsupported,
}

impl FreshShellReason {
    /// 診断ログ用の日本語ラベル
    pub fn label(self) -> &'static str {
        match self {
            Self::NoSessionId => "ID なし",
            Self::InvalidSessionId => "形式不正",
            Self::TranscriptMissing => "会話が見つからない",
            Self::ResumeUnsupported => "resume 非対応",
        }
    }
}

/// 復元時に 1 ペインをどう起こすか（#1076）。**理由まで返す**ので、
/// 復元内訳のログが「なぜエージェントが出なかったか」を言える
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePlan {
    /// 器（tmux セッション）が生きている = 実行中プロセスごと再 attach する
    Reattach,
    /// 保存済みの会話を明示 resume する（#1238 で claude 以外へ広げた）
    Resume {
        /// どの系統として起こすか（診断ログ・件数の内訳に出す）
        agent: Agent,
        /// シェルへ投入するコマンド（改行は投入側が付ける）
        command: String,
        /// カタログ / spawn 記録から役割・モデルまで組み立てられたか（診断ログ用）
        with_role: bool,
    },
    /// 保存 cwd で新しいシェルを開くだけ
    FreshShell(FreshShellReason),
}

/// 復元対象のペインに紐づく「どの系統の・どの会話か」（#1238）。
///
/// `id` が `None` = **系統は分かるが会話 ID を採れていない**
/// （agy は最初のターンまで会話が生まれない / Windows には `lsof` が無い）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneResume<'a> {
    pub agent: Agent,
    pub id: Option<&'a str>,
}

/// 復元時の 1 ペインの判断材料（#1076 / #1238）
#[derive(Debug, Clone, Copy)]
pub struct RestoreInput<'a> {
    /// 器（tmux セッション）が生きているか
    pub backend_alive: bool,
    /// `layout.json` に保存されていた系統と会話 ID
    pub resume: Option<PaneResume<'a>>,
    /// 会話が見つかったか。`Some` の中身は claude の `CLAUDE_CONFIG_DIR` 前置き
    /// （[`crate::transcript::resume_env_prefix`]）で、他系統は空文字。
    /// `None` = 会話ファイルがどこにも無い = resume しても失敗するので起こさない（#652）
    pub conversation: Option<&'a str>,
    /// 起動条件（役割 / モデル / effort）の記録
    pub catalog: Option<&'a SessionCatalog>,
    /// 起動条件を spawn 記録から引くためのキー（#728 と同じ「器の名前 → ペイン ID」）
    pub backend_session: Option<&'a str>,
    pub pane: Option<u64>,
    /// 判定する OS（**引数で受ける**ので macOS 上から Windows 側を検証できる）
    pub platform: Platform,
}

/// 復元時の 1 ペインの判断（材料を引数で受け取る純粋版）。
///
/// コマンドの形は [`resume_command`] が正本（役割 env / `--model` / `--effort` /
/// config dir までカタログの記録から復元する）。カタログにも spawn 記録にも
/// 無いときだけ最小形（config dir + resume の引数）へ落ちる
pub fn restore_plan_in(input: RestoreInput<'_>) -> RestorePlan {
    // 器が生きているなら実行中プロセスごと戻る（エージェントを二重起動しない）
    if input.backend_alive {
        return RestorePlan::Reattach;
    }
    let Some(resume) = input.resume else {
        return RestorePlan::FreshShell(FreshShellReason::NoSessionId);
    };
    // **「保存されていない」と「手段が無い」を分ける**（#1238）。判定の正本は
    // tako_core::agent_resume（Windows に lsof が無い = codex / agy の ID を採れない）
    if !tako_core::agent_resume::restore_support(resume.agent, input.platform).is_wired() {
        return RestorePlan::FreshShell(FreshShellReason::ResumeUnsupported);
    }
    let Some(id) = resume.id else {
        return RestorePlan::FreshShell(FreshShellReason::NoSessionId);
    };
    if !crate::transcript::is_valid_session_id(id) {
        return RestorePlan::FreshShell(FreshShellReason::InvalidSessionId);
    }
    let Some(env_prefix) = input.conversation else {
        return RestorePlan::FreshShell(FreshShellReason::TranscriptMissing);
    };
    // 起動条件が分かれば復元する。**役割 env が復元されないと、戻ってきた
    // エージェントは master / worker として認識されない**（オーケストレーターからも
    // MCP からも見えない）ので、ここは最小形へ落とさずに済ませたい
    if let Some(entry) = launch_record(&input, id, resume.agent) {
        if let Ok(command) = resume_command_with_env(id, &entry, Some(env_prefix)) {
            let with_role = matches!(entry.kind.as_str(), "master" | "worker" | "solo");
            return RestorePlan::Resume {
                agent: resume.agent,
                command,
                with_role,
            };
        }
    }
    let minimal = SessionEntry {
        agent: Some(resume.agent.as_str().to_string()),
        ..SessionEntry::default()
    };
    match resume_command_with_env(id, &minimal, Some(env_prefix)) {
        Ok(command) => RestorePlan::Resume {
            agent: resume.agent,
            command,
            with_role: false,
        },
        // 系統が resume を持たない場合はここに来ない（上で弾いてある）が、
        // ID の書式で弾かれた場合の保険
        Err(_) => RestorePlan::FreshShell(FreshShellReason::InvalidSessionId),
    }
}

/// 起動条件の記録を引く（**カタログ本体が先・spawn 記録が後**）。
///
/// claude はセッション検出でカタログ本体へ昇格するが、codex / agy は
/// 昇格経路が無く `pending`（spawn 時の記録）にしか残らない（FR-5.12 の制限）。
/// 器の名前 / ペイン ID で引けば役割・プロジェクト・ラベルは復元できる
fn launch_record(input: &RestoreInput<'_>, id: &str, agent: Agent) -> Option<SessionEntry> {
    let catalog = input.catalog?;
    if let Some(entry) = catalog.entries.get(id) {
        // **系統は layout 由来が正**（記録が食い違っていても、実際に動いていた
        // 系統のコマンドを組む。claude の ID に codex の記録が付いている等の齟齬で
        // 会話への入口を失わない）
        return Some(SessionEntry {
            agent: Some(agent.as_str().to_string()),
            ..entry.clone()
        });
    }
    let pending = catalog
        .pending
        .iter()
        .find(|p| p.matches(input.backend_session, input.pane))?;
    // 系統が食い違う記録は使わない（別のエージェントを起動していたペイン）
    if pending
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .is_some_and(|a| a != agent)
    {
        return None;
    }
    Some(SessionEntry {
        kind: pending.kind.clone(),
        label: pending.label.clone(),
        project: pending.project.clone(),
        // spawn 記録はプロファイル名を持たない（master:<profile> は既定名へ落ちる）
        profile: None,
        agent: Some(agent.as_str().to_string()),
        model: pending.model.clone(),
        effort: pending.effort.clone(),
        ..SessionEntry::default()
    })
}

/// [`restore_plan_in`] の実環境版（会話の所在の走査とカタログの読み込みを行う）。
/// カタログは復元ループの外で 1 回だけ読んで渡す
pub fn restore_plan(
    backend_alive: bool,
    resume: Option<PaneResume<'_>>,
    catalog: Option<&SessionCatalog>,
    backend_session: Option<&str>,
    pane: Option<u64>,
) -> RestorePlan {
    // 器が生きているなら会話ファイルの走査（read_dir）自体が要らない
    if backend_alive {
        return RestorePlan::Reattach;
    }
    // A/B（#1238）: 修正前は claude 以外の会話参照を保存も復元もしていなかった
    let resume = resume.filter(|r| r.agent == Agent::Claude || !crate::agent_resume::legacy_1238());
    let conversation = resume
        .and_then(|r| Some((r.agent, r.id?)))
        .and_then(|(agent, id)| conversation_env(agent, id));
    // A/B（#1076）: 修正前は起動条件を復元せず最小形 `claude --resume <id>` だった
    let catalog = if tako_core::claude_resume::legacy_1076() {
        None
    } else {
        catalog
    };
    restore_plan_in(RestoreInput {
        backend_alive,
        resume,
        conversation: conversation.as_deref(),
        catalog,
        backend_session,
        pane,
        platform: Platform::current(),
    })
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
    let catalog = SessionCatalog::load().ok()?;
    resolve_session_for_pane_in(&catalog, pane_id)
}

/// カタログ指定版（テスト用に分離）。
/// 同一ペイン番号のエントリは世代を跨いで複数堆積する（spawn / 検出のたびに記録が
/// 増える。実運用で同一 pane に 20 世代超を確認）。BTreeMap の辞書順先勝ちだと
/// session_id（uuid）の並び次第で stale な旧世代を毎回返し続けるため（#466）、
/// 最後に生存確認された世代（last_seen_at、無ければ started_at）を選ぶ
pub fn resolve_session_for_pane_in(catalog: &SessionCatalog, pane_id: &str) -> Option<String> {
    let pane_num: u64 = pane_id.parse().ok()?;
    catalog
        .entries
        .iter()
        .filter(|(_, entry)| entry.pane == Some(pane_num))
        .max_by_key(|(_, entry)| {
            if entry.last_seen_at.is_empty() {
                entry.started_at.as_str()
            } else {
                entry.last_seen_at.as_str()
            }
        })
        .map(|(session_id, _)| session_id.clone())
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
        assert_eq!(detected[0].tmux_session.as_deref(), Some("tako-a"));
        assert_eq!(detected[0].agent_cwd.as_deref(), Some("/w/a"));
        assert_eq!(detected[0].model.as_deref(), Some("claude-fable-5"));
        assert_eq!(detected[1].tmux_session.as_deref(), Some("tako-b"));
        // 空 agents は空
        assert!(detect_from_agents_value(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn 器がないペインはtako_paneで検出される() {
        // #728: 器（psmux / tmux）が無い構成では `pane` が付かない。
        // `tako_pane`（tako のペイン ID）が唯一の対応キーになる
        let value = serde_json::json!({"agents": [
            {"tako_pane": 7, "session_id": "11111111-2222-3333-4444-555555555555",
             "cwd": "C:\\w", "model": "claude-opus-5"},
            {"tako_pane": 9, "session_id": "../../invalid"},
            {"session_id": "22222222-3333-4444-5555-666666666666"},
            // 器あり側が優先（両方付くことは無いが、付いたらセッション名を採る）
            {"pane": "tako-x:0.0", "tako_pane": 3,
             "session_id": "33333333-4444-5555-6666-777777777777"},
        ]});
        let detected = detect_from_agents_value(&value);
        assert_eq!(detected.len(), 2);
        assert_eq!(
            detected[0].session_id,
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(detected[0].tmux_session, None);
        assert_eq!(detected[0].pane, Some(7));
        assert_eq!(detected[0].agent_cwd.as_deref(), Some("C:\\w"));
        assert_eq!(detected[1].tmux_session.as_deref(), Some("tako-x"));
        assert_eq!(detected[1].pane, Some(3));
    }

    #[test]
    fn 器なしのpending記録は互いを消さない() {
        // #728: 旧実装は `p.tmux_session != record.tmux_session` で重複排除していた。
        // 器なし（どちらも None）だと全件が「同じキー」に見え、spawn のたびに
        // 直前の記録が消えて最後の 1 件しか残らなかった
        let mut catalog = SessionCatalog::default();
        let make = |pane: u64| PendingSpawn {
            tmux_session: None,
            pane: Some(pane),
            recorded_at: "2026-08-01T00:00:00Z".into(),
            ..Default::default()
        };
        // **production の重複排除そのもの**を通す（実装をなぞると検出力が出ない）
        for pane in [1u64, 2, 3] {
            catalog.upsert_pending(make(pane));
        }
        assert_eq!(catalog.pending.len(), 3, "別ペインの記録は残る");
        // 同じペインの再 spawn は置き換わる
        catalog.upsert_pending(make(2));
        assert_eq!(catalog.pending.len(), 3);
        assert_eq!(
            catalog.pending.iter().filter(|p| p.pane == Some(2)).count(),
            1,
            "同じキーは 1 件に畳まれる"
        );
    }

    #[test]
    fn 突き合わせキーは器の有無で切り替わる() {
        // #728: 器があるときはセッション名だけで見る（ペイン ID は tako 再起動を
        // またいで振り直されるので、器の名前の方が強い同一性を持つ）
        let with_container = PendingSpawn {
            tmux_session: Some("tako-abc".into()),
            pane: Some(5),
            ..Default::default()
        };
        assert!(with_container.matches(Some("tako-abc"), None));
        assert!(!with_container.matches(Some("tako-xyz"), Some(5)));
        // 器ありの記録は、器なしの検出（ペイン ID だけ）には一致しない
        assert!(!with_container.matches(None, Some(5)));

        let without = PendingSpawn {
            tmux_session: None,
            pane: Some(5),
            ..Default::default()
        };
        assert!(without.matches(None, Some(5)));
        assert!(!without.matches(None, Some(6)));
        assert!(!without.matches(Some("tako-abc"), Some(5)));

        // キーを 1 つも持たない記録は何にも一致しない
        let keyless = PendingSpawn::default();
        assert!(!keyless.matches(None, None));
        assert!(!keyless.matches(Some("tako-abc"), Some(5)));
    }

    #[test]
    fn 旧形式のpending記録がそのまま読める() {
        // #728 でスキーマを String -> Option<String> にしたが、serde は
        // 素の文字列を Some(...) として読む。移行 Step は要らない（#916 の判断）
        let old = "entries: {}\npending:\n  - tmux_session: tako-s42\n    kind: worker\n\
                   \n    recorded_at: '2026-08-01T00:00:00Z'\n";
        let catalog: SessionCatalog = serde_yaml::from_str(old).expect("旧形式が読める");
        assert_eq!(catalog.pending.len(), 1);
        assert_eq!(
            catalog.pending[0].tmux_session.as_deref(),
            Some("tako-s42"),
            "旧形式の素の文字列が Some として読める"
        );
        // 器の名前が無い（= 新形式で器なし）記録も読める
        let containerless =
            "pending:\n  - kind: worker\n    pane: 12\n    recorded_at: '2026-08-01T00:00:00Z'\n";
        let catalog: SessionCatalog = serde_yaml::from_str(containerless).expect("器なしが読める");
        assert_eq!(catalog.pending[0].tmux_session, None);
        assert_eq!(catalog.pending[0].pane, Some(12));
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
                tmux_session: Some("tako-s42".into()),
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
            tmux_session: Some("tako-s42".into()),
            pane: Some(7),
            agent_cwd: Some("/work/tako".into()),
            model: Some("claude-fable-5".into()),
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 7,
            tab: 3,
            tmux_session: Some("tako-s42".into()),
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
    fn 器がない構成でもpending記録がペインidで昇格する() {
        // #728: Windows で psmux 未導入（backend=none）の主経路。
        // 器の名前が付かないので、spawn 記録も検出もペイン ID がキーになる
        let path = temp_path("promote-container-less");
        SessionCatalog::mutate_at(&path, |c| {
            c.pending.push(PendingSpawn {
                tmux_session: None,
                kind: "worker".into(),
                label: Some("728-sessions".into()),
                project: Some("tako".into()),
                agent: Some("claude".into()),
                issues: vec![728],
                prompt_head: Some("Issue #728 を実装する".into()),
                cwd: Some("C:\\work\\tako".into()),
                tab: Some(1),
                pane: Some(12),
                recorded_at: "2026-08-01T00:00:00Z".into(),
                ..Default::default()
            });
        })
        .unwrap();

        let detected = vec![DetectedSession {
            session_id: "22222222-3333-4444-5555-666666666666".into(),
            tmux_session: None,
            pane: Some(12),
            agent_cwd: Some("C:\\work\\tako".into()),
            model: Some("claude-opus-5".into()),
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 12,
            tab: 1,
            tmux_session: None,
            role: Some("orchestrator-worker:tako:728-sessions".into()),
            title: Some("tako: 728-sessions".into()),
            cwd: Some("C:\\work\\tako".into()),
            log_file: None,
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();

        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert!(catalog.pending.is_empty(), "pending が昇格で消える");
        let entry = &catalog.entries["22222222-3333-4444-5555-666666666666"];
        assert_eq!(entry.kind, "worker");
        assert_eq!(entry.label.as_deref(), Some("728-sessions"));
        assert_eq!(entry.issues, vec![728]);
        assert_eq!(entry.pane, Some(12));
        assert_eq!(entry.tab, Some(1));
        assert_eq!(entry.cwd.as_deref(), Some("C:\\work\\tako"));
        // 器が無いので名前は付かない（「観測漏れ」ではなく「器が無い」）
        assert_eq!(entry.tmux_session, None);
        assert_eq!(entry.started_at, "2026-08-01T00:00:00Z");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pending無しでもroleから分類される() {
        let path = temp_path("role-only");
        let detected = vec![DetectedSession {
            session_id: "aaaaaaaa-1111-2222-3333-444444444444".into(),
            tmux_session: Some("tako-s9".into()),
            pane: None,
            agent_cwd: None,
            model: None,
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 2,
            tab: 1,
            tmux_session: Some("tako-s9".into()),
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
    fn resume_commandの組み立てと系統ごとの書式() {
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("112-log".into()),
            agent: Some("claude".into()),
            model: Some("claude-fable-5".into()),
            effort: Some("max".into()),
            ..Default::default()
        };
        let cmd =
            resume_command_with_env("11111111-2222-3333-4444-555555555555", &entry, None).unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:tako:112-log' claude --model claude-fable-5 --effort max --resume 11111111-2222-3333-4444-555555555555"
        );

        let solo = SessionEntry {
            kind: "solo".into(),
            profile: Some("default".into()),
            ..Default::default()
        };
        let cmd = resume_command_with_env("abc", &solo, None).unwrap();
        assert!(
            cmd.starts_with("TAKO_ORCHESTRATOR_ROLE=solo claude"),
            "{cmd}"
        );

        // #1238: codex は `resume` サブコマンド + 位置引数。**`--model` は付けない**
        // （`codex resume --help` に無い = 付けると起動しない）
        let codex = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            agent: Some("codex".into()),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env("01a08347-543b-7a20-8e49-a93380efb375", &codex, None).unwrap(),
            "TAKO_ORCHESTRATOR_ROLE='worker:tako' codex resume 01a08347-543b-7a20-8e49-a93380efb375"
        );

        // #1238: agy はグローバルフラグ。会話に記録された設定を上書きしない
        let agy = SessionEntry {
            agent: Some("agy".into()),
            model: Some("Claude Opus 4.6 (Thinking)".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env("874efd16-af95-4b2e-8036-84f6c3eaded3", &agy, None).unwrap(),
            "agy --conversation 874efd16-af95-4b2e-8036-84f6c3eaded3"
        );

        // ローカル LLM は会話の器が定まっていない（#991）ので断る
        let local = SessionEntry {
            agent: Some("local".into()),
            ..Default::default()
        };
        let err = resume_command_with_env("abc", &local, None).unwrap_err();
        assert!(err.contains("resume 非対応"), "{err}");

        // 書式検証（パストラバーサル防止）は全系統に効く
        for agent in ["claude", "codex", "agy"] {
            let invalid = SessionEntry {
                agent: Some(agent.into()),
                ..Default::default()
            };
            assert!(
                resume_command_with_env("../etc", &invalid, None).is_err(),
                "{agent} が不正な ID を通した"
            );
        }
    }

    /// Issue #652: アカウントの会話は `CLAUDE_CONFIG_DIR` を前置しないと
    /// `No conversation found with session ID` で resume に失敗する。
    /// 前置はコマンド先頭（rc / direnv より後に効く位置。#500 / #512 と同型）
    #[test]
    fn resume_commandはconfigdirを先頭に前置する() {
        let entry = SessionEntry {
            kind: "master".into(),
            profile: Some("takodev".into()),
            agent: Some("claude".into()),
            ..Default::default()
        };
        let cmd = resume_command_with_env(
            "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4",
            &entry,
            Some("export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; "),
        )
        .unwrap();
        assert_eq!(
            cmd,
            "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; \
             TAKO_ORCHESTRATOR_ROLE='master:takodev' claude \
             --resume e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4"
        );
    }

    /// テスト用の最小の判断材料（claude・macOS・器は死んでいる）
    fn input<'a>(id: Option<&'a str>, env: Option<&'a str>) -> RestoreInput<'a> {
        RestoreInput {
            backend_alive: false,
            resume: id.map(|id| PaneResume {
                agent: Agent::Claude,
                id: Some(id),
            }),
            conversation: env,
            catalog: None,
            backend_session: None,
            pane: None,
            platform: Platform::MacOs,
        }
    }

    /// Issue #1076: PC 再起動後の復元は、器が消えたペインを保存済みの会話へ戻す。
    /// **役割 env / `--model` まで復元する**（最小形 `claude --resume <id>` だと
    /// 戻ってきた claude が master / worker として認識されない）
    #[test]
    fn restore_planはカタログの起動条件ごと復元する() {
        let id = "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4";
        let env = "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; ";
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            id.into(),
            SessionEntry {
                kind: "master".into(),
                profile: Some("takodev".into()),
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                ..Default::default()
            },
        );
        let plan = restore_plan_in(RestoreInput {
            catalog: Some(&catalog),
            ..input(Some(id), Some(env))
        });
        assert_eq!(
            plan,
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!(
                    "{env}TAKO_ORCHESTRATOR_ROLE='master:takodev' \
                     claude --model claude-opus-5 --resume {id}"
                ),
                with_role: true,
            }
        );
    }

    /// カタログに記録が無いペインは最小形へ落ちる（従来の挙動）
    #[test]
    fn restore_planはカタログ不在なら最小形へ落ちる() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        let empty = SessionCatalog::default();
        let minimal = RestorePlan::Resume {
            agent: Agent::Claude,
            command: format!("{env}claude --resume {id}"),
            with_role: false,
        };
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&empty),
                ..input(Some(id), Some(env))
            }),
            minimal
        );
        // カタログそのものが読めなかった場合も同じ
        assert_eq!(restore_plan_in(input(Some(id), Some(env))), minimal);
    }

    /// 新規シェルへ落ちる理由が区別できる（復元内訳のログに出す。#1076 / #1238）
    #[test]
    fn restore_planは新規シェルの理由を返す() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        // 器が生きている = 実行中プロセスごと再 attach（claude を二重起動しない）
        assert_eq!(
            restore_plan_in(RestoreInput {
                backend_alive: true,
                ..input(Some(id), Some(env))
            }),
            RestorePlan::Reattach
        );
        assert_eq!(
            restore_plan_in(input(None, Some(env))),
            RestorePlan::FreshShell(FreshShellReason::NoSessionId)
        );
        assert_eq!(
            restore_plan_in(input(Some("../../bad"), Some(env))),
            RestorePlan::FreshShell(FreshShellReason::InvalidSessionId)
        );
        // transcript が見つからない = resume しても `No conversation found` になる
        assert_eq!(
            restore_plan_in(input(Some(id), None)),
            RestorePlan::FreshShell(FreshShellReason::TranscriptMissing)
        );
        // #1238: 系統は分かるが ID が採れていない（agy の最初のターン前）
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Agy,
                    id: None
                }),
                ..input(None, Some(env))
            }),
            RestorePlan::FreshShell(FreshShellReason::NoSessionId)
        );
    }

    /// #1238: 「保存されていない」と「この環境には手段が無い」を別の理由として出す。
    /// Windows には `lsof` が無いので codex / agy の ID をそもそも採れない
    #[test]
    fn restore_planはwindowsのcodexとagyをresume非対応と言う() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        for agent in [Agent::Codex, Agent::Agy] {
            assert_eq!(
                restore_plan_in(RestoreInput {
                    resume: Some(PaneResume {
                        agent,
                        id: Some(id)
                    }),
                    platform: Platform::Windows,
                    ..input(None, Some(""))
                }),
                RestorePlan::FreshShell(FreshShellReason::ResumeUnsupported),
                "{agent:?}"
            );
        }
        // claude の ID は OS に依らず取れる（`claude agents --json`）
        assert!(matches!(
            restore_plan_in(RestoreInput {
                platform: Platform::Windows,
                ..input(Some(id), Some(""))
            }),
            RestorePlan::Resume { .. }
        ));
        assert_eq!(
            FreshShellReason::ResumeUnsupported.label(),
            "resume 非対応",
            "内訳ログのラベルが変わると commands.md の読み方が合わなくなる"
        );
    }

    /// #1238: codex / agy は spawn 記録（pending）から役割ごと復元する。
    /// claude と違ってカタログ本体へ昇格しない（FR-5.12 の制限）ので、
    /// ここを見ないと戻ってきた worker がオーケストレーターから見えない
    #[test]
    fn restore_planはcodexとagyをpending記録の役割ごと復元する() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("tako-w2".into()),
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("1238".into()),
            agent: Some("codex".into()),
            ..Default::default()
        });
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Codex,
                    id: Some(id)
                }),
                catalog: Some(&catalog),
                backend_session: Some("tako-w2"),
                ..input(None, Some(""))
            }),
            RestorePlan::Resume {
                agent: Agent::Codex,
                command: format!("TAKO_ORCHESTRATOR_ROLE='worker:tako:1238' codex resume {id}"),
                with_role: true,
            }
        );
        // 別の系統を起動していたペインの記録は使わない（役割を取り違えない）
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Agy,
                    id: Some(id)
                }),
                catalog: Some(&catalog),
                backend_session: Some("tako-w2"),
                ..input(None, Some(""))
            }),
            RestorePlan::Resume {
                agent: Agent::Agy,
                command: format!("agy --conversation {id}"),
                with_role: false,
            }
        );
    }

    /// カタログの記録の系統が layout と食い違っても、**実際に動いていた系統**の
    /// コマンドを組む（記録の齟齬で会話への入口を失わない）
    #[test]
    fn restore_planは記録の齟齬よりlayoutの系統を優先する() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            id.into(),
            SessionEntry {
                agent: Some("codex".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&catalog),
                ..input(Some(id), Some(env))
            }),
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!("{env}claude --resume {id}"),
                with_role: false,
            }
        );
    }

    #[test]
    fn gcはpending期限とエントリ上限を強制する() {
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("old".into()),
            recorded_at: "2026-07-01T00:00:00Z".into(),
            ..Default::default()
        });
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("fresh".into()),
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
        assert_eq!(catalog.pending[0].tmux_session.as_deref(), Some("fresh"));
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

    #[test]
    fn ペイン解決は同一ペインの複数世代から最新を選ぶ() {
        // #466: 同一 pane 番号のエントリは世代を跨いで堆積する。辞書順の
        // 先勝ちだと stale な旧世代を返すことがあった。last_seen_at 最新を選ぶ
        let mut catalog = SessionCatalog::default();
        for (sid, seen) in [
            ("session-old", "2026-07-20T00:00:00Z"),
            ("session-new", "2026-07-22T00:00:00Z"),
            ("session-mid", "2026-07-21T00:00:00Z"),
        ] {
            catalog.entries.insert(
                sid.to_string(),
                SessionEntry {
                    pane: Some(648),
                    last_seen_at: seen.to_string(),
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            resolve_session_for_pane_in(&catalog, "648").as_deref(),
            Some("session-new")
        );
        // 該当ペインなし・数値でない ID は None
        assert_eq!(resolve_session_for_pane_in(&catalog, "649"), None);
        assert_eq!(resolve_session_for_pane_in(&catalog, "tako-a:0.0"), None);
    }

    #[test]
    fn ペイン解決はlast_seen_at欠落時にstarted_atへフォールバックする() {
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            "session-seen".to_string(),
            SessionEntry {
                pane: Some(7),
                started_at: "2026-07-01T00:00:00Z".to_string(),
                last_seen_at: "2026-07-10T00:00:00Z".to_string(),
                ..Default::default()
            },
        );
        catalog.entries.insert(
            "session-started-only".to_string(),
            SessionEntry {
                pane: Some(7),
                started_at: "2026-07-15T00:00:00Z".to_string(),
                last_seen_at: String::new(),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_session_for_pane_in(&catalog, "7").as_deref(),
            Some("session-started-only")
        );
    }
}

//! worker レジストリ（Issue #390）。
//!
//! spawn した worker をペインとは独立の永続ファイル（workers.yaml）へ登録し、
//! アプリ再起動・ペイン消失後も watch / status / report が tmux session /
//! claude session ID 経由で追跡を継続できるようにする。
//!
//! 設計方針:
//! - sessions.yaml（会話カタログ。resume 用途）とは独立。こちらは worker の
//!   ライフサイクル（active / closed）と追跡キーだけを持つ
//! - あくまで**フォールバック層**: 既存の watch / worker_status の判定ロジックには
//!   手を入れず、pane 消失時の解決材料（tmux_session / session_id）を供給する
//!   （#273 / #289 の教訓: 判定変更は最小限に）
//! - レジストリの読み書き失敗で spawn / watch を止めない（呼び出し側は警告のみ）

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// prompt 未達検知の猶予秒数（Issue #390 要件 4）。
/// spawn からこの時間を超えても claude transcript（session_id）が観測できない
/// active worker は「プロンプト未達の疑い」とする。PromptFlow の総合タイムアウト
/// 120 秒 + claude 起動・セッション検出の遅延に十分な余裕を持たせた保守的な値
/// （誤検知で正常 worker を疑わせない）。判定には画面が busy でないこと等の
/// 複合条件を併用する（dispatch 側 = `prompt_delivery_assessment` の呼び出し元）
pub const PROMPT_DELIVERY_GRACE_SECS: i64 = 240;

/// closed エントリを含めた保持上限。超過分は古い closed から削除する
const MAX_WORKERS: usize = 200;

/// テスト専用: registry_path() をプロセス毎の一時ファイルへ固定する。
/// unit テスト（spawn 経由の record_spawn 等）が実運用の workers.yaml を
/// 読み書きして汚染・誤読するのを防ぐ（orchestrator::test_config_dir_override と同思想）
#[cfg(test)]
fn test_registry_path() -> &'static std::sync::OnceLock<PathBuf> {
    static OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    &OVERRIDE
}

/// workers.yaml のパス。`TAKO_WORKERS_FILE` で差し替え可能（テスト・隔離用）
pub fn registry_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(
            test_registry_path()
                .get_or_init(|| {
                    std::env::temp_dir()
                        .join(format!("tako-test-workers-{}.yaml", std::process::id()))
                })
                .clone(),
        )
    }
    #[cfg(not(test))]
    {
        if let Some(p) = std::env::var_os("TAKO_WORKERS_FILE") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        tako_core::paths::data_dir().map(|d| d.join("workers.yaml"))
    }
}

/// レジストリ本体（workers.yaml のスキーマ）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerRegistry {
    /// worker ID（連番の文字列）→ エントリ
    #[serde(default)]
    pub workers: BTreeMap<String, WorkerEntry>,
    /// 次に発番する ID
    #[serde(default)]
    pub next_id: u64,
}

/// レジストリの 1 エントリ。ペイン消失後の追跡に必要なキーを集約する
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// エージェント種別（claude / codex / agy）。prompt 未達検知は claude のみ対象
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// spawn 時のペイン ID（tako 再起動後も layout 復元で同一 ID が維持される）
    pub pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    /// tmux バックエンドセッション名（ペイン消失時の第一フォールバックキー）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    /// claude の session ID（検出後に埋まる。transcript 直読の第二フォールバックキー）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    /// 委任台帳のエントリ ID（Issue #292 との突き合わせ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spawned_at: String,
    /// active / closed
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// closed の理由（explicit_close 等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
    /// claude transcript（session_id）を最初に観測した時刻 = プロンプト到達の証跡
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_delivered_at: Option<String>,
}

impl WorkerEntry {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

/// prompt 送達状態の判定結果（`prompt_delivery_assessment`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDelivery {
    /// transcript（session_id）を観測済み = プロンプト到達
    Delivered,
    /// 未観測だが猶予時間内（起動・検出待ち）
    Pending,
    /// 猶予時間を超えても未観測 = プロンプト未達の疑い。
    /// 最終判定は画面状態（busy でない等）と併せて呼び出し側が行う
    OverdueSuspect,
    /// 判定対象外（claude 以外の agent、closed、時刻パース不能）
    NotApplicable,
}

impl PromptDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Pending => "pending",
            Self::OverdueSuspect => "undelivered",
            Self::NotApplicable => "n/a",
        }
    }
}

impl WorkerRegistry {
    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("workers.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("workers.yaml のパースに失敗: {e}"))
    }

    pub fn load() -> Result<Self, String> {
        let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同型）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut registry = Self::load_from(path)?;
        let result = f(&mut registry);
        let content = serde_yaml::to_string(&registry)
            .map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    /// worker ID（完全一致 → 前方一致）でエントリを解決する
    pub fn resolve(&self, id_prefix: &str) -> Result<(&String, &WorkerEntry), String> {
        if let Some((id, entry)) = self.workers.get_key_value(id_prefix) {
            return Ok((id, entry));
        }
        let matches: Vec<_> = self
            .workers
            .iter()
            .filter(|(id, _)| id.starts_with(id_prefix))
            .collect();
        match matches.len() {
            0 => Err(format!(
                "worker '{id_prefix}' がレジストリに見つからない（tako orchestrator workers で確認）"
            )),
            1 => Ok(matches[0]),
            n => Err(format!(
                "worker '{id_prefix}' の候補が {n} 件ある（完全な ID を指定）: {}",
                matches
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// pane ID から active な worker を引く（pane 消失時のフォールバック解決）。
    /// 同一 pane に複数 active が居る場合（異常系）は最新の spawned_at を返す
    pub fn find_active_by_pane(&self, pane: u64) -> Option<(&String, &WorkerEntry)> {
        self.workers
            .iter()
            .filter(|(_, e)| e.pane == pane && e.is_active())
            .max_by(|a, b| a.1.spawned_at.cmp(&b.1.spawned_at))
    }

    /// 古い closed エントリから削って上限を強制する
    fn gc(&mut self) {
        if self.workers.len() <= MAX_WORKERS {
            return;
        }
        let mut closed: Vec<(String, String)> = self
            .workers
            .iter()
            .filter(|(_, e)| !e.is_active())
            .map(|(id, e)| (e.spawned_at.clone(), id.clone()))
            .collect();
        closed.sort(); // spawned_at 昇順 = 古い順
        let drop_count = self.workers.len() - MAX_WORKERS;
        for (_, id) in closed.into_iter().take(drop_count) {
            self.workers.remove(&id);
        }
    }
}

/// spawn 時の登録内容（`record_spawn` の入力）
#[derive(Debug, Clone, Default)]
pub struct RegisterSpawn {
    pub label: Option<String>,
    pub project: String,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub pane: u64,
    pub tab: Option<u64>,
    pub tmux_session: Option<String>,
    pub issues: Vec<u32>,
    pub ledger_id: Option<String>,
    pub cwd: Option<String>,
    pub prompt_head: Option<String>,
}

/// spawn した worker をレジストリへ登録し、発番した worker ID を返す。
/// 同一 pane の既存 active エントリは closed へ倒す（pane ID 再利用時の残骸対策。
/// 二重登録で同一 pane に active が 2 件並ぶ状態を作らない）
pub fn record_spawn(record: RegisterSpawn) -> Result<String, String> {
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate(|reg| {
        for entry in reg.workers.values_mut() {
            if entry.pane == record.pane && entry.is_active() {
                entry.status = "closed".into();
                entry.closed_at = Some(now.clone());
                entry.close_reason = Some("superseded".into());
            }
        }
        reg.next_id += 1;
        let id = reg.next_id.to_string();
        reg.workers.insert(
            id.clone(),
            WorkerEntry {
                label: record.label.clone(),
                project: record.project.clone(),
                agent: record.agent.clone(),
                model: record.model.clone(),
                effort: record.effort.clone(),
                pane: record.pane,
                tab: record.tab,
                tmux_session: record.tmux_session.clone(),
                session_id: None,
                issues: record.issues.clone(),
                ledger_id: record.ledger_id.clone(),
                cwd: record.cwd.clone(),
                prompt_head: record.prompt_head.clone(),
                spawned_at: now.clone(),
                status: "active".into(),
                closed_at: None,
                close_reason: None,
                prompt_delivered_at: None,
            },
        );
        reg.gc();
        id
    })
}

/// 明示 close されたペインの active worker を closed にする。
/// レジストリ不在（orchestrator 未使用）は何もしない（通常ペインの close に
/// ファイル IO のコストを掛けない）
pub fn mark_closed_by_pane(pane: u64, reason: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(&path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.pane == pane && entry.is_active() {
                entry.status = "closed".into();
                entry.closed_at = Some(now.clone());
                entry.close_reason = Some(reason.to_string());
            }
        }
    })
}

/// 検出済み claude session をレジストリへ反映する（tmux_session キー）。
/// session_id の初観測 = transcript 生成 = プロンプト到達の証跡として
/// `prompt_delivered_at` も同時に記録する。GUI の定期スキャンおよび
/// worker_status の解決成功時（lazy 昇格）から呼ばれる
pub fn record_session_detected(tmux_session: &str, session_id: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    // 変更が無いなら書き込みをスキップ（定期スキャンからの毎回書き込み防止）
    let current = WorkerRegistry::load_from(&path)?;
    let needs_update = current.workers.values().any(|e| {
        e.is_active()
            && e.tmux_session.as_deref() == Some(tmux_session)
            && (e.session_id.as_deref() != Some(session_id) || e.prompt_delivered_at.is_none())
    });
    if !needs_update {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(&path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.is_active() && entry.tmux_session.as_deref() == Some(tmux_session) {
                entry.session_id = Some(session_id.to_string());
                if entry.prompt_delivered_at.is_none() {
                    entry.prompt_delivered_at = Some(now.clone());
                }
            }
        }
    })
}

/// prompt 送達状態を判定する（Issue #390 要件 4）。
/// OverdueSuspect は「疑い」であり、最終的な未達イベントの発火は呼び出し側が
/// 画面状態（busy でない・実行中子プロセスなし）と組み合わせて決める
pub fn prompt_delivery_assessment(entry: &WorkerEntry, now_epoch: i64) -> PromptDelivery {
    if entry.session_id.is_some() || entry.prompt_delivered_at.is_some() {
        return PromptDelivery::Delivered;
    }
    // transcript（session_id）の観測経路があるのは claude のみ。
    // codex / agy を undelivered と誤検知しないため対象外にする
    if entry.agent != "claude" || !entry.is_active() {
        return PromptDelivery::NotApplicable;
    }
    let Some(spawned) = crate::sessions::parse_iso(&entry.spawned_at) else {
        return PromptDelivery::NotApplicable;
    };
    if now_epoch - spawned > PROMPT_DELIVERY_GRACE_SECS {
        PromptDelivery::OverdueSuspect
    } else {
        PromptDelivery::Pending
    }
}

/// workers 一覧の JSON ペイロードを組み立てる。
/// `live_backends` は現存する tmux セッション名（呼び出し側が 1 コマンドで列挙）、
/// `live_panes` は GUI に現存するペイン ID（tree + shelved）。
/// `include_closed` = false なら active のみ返す
pub fn list_payload(
    registry: &WorkerRegistry,
    live_backends: &[String],
    live_panes: &[u64],
    include_closed: bool,
) -> Value {
    let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap_or(0);
    let mut items: Vec<Value> = Vec::new();
    let mut entries: Vec<(&String, &WorkerEntry)> = registry
        .workers
        .iter()
        .filter(|(_, e)| include_closed || e.is_active())
        .collect();
    // 新しい順（spawned_at 降順）
    entries.sort_by(|a, b| b.1.spawned_at.cmp(&a.1.spawned_at));
    for (id, e) in entries {
        let tmux_alive = e
            .tmux_session
            .as_deref()
            .is_some_and(|ts| live_backends.iter().any(|b| b == ts));
        let pane_alive = live_panes.contains(&e.pane);
        let delivery = prompt_delivery_assessment(e, now_epoch);
        items.push(json!({
            "worker_id": id,
            "label": e.label,
            "project": e.project,
            "agent": e.agent,
            "model": e.model,
            "effort": e.effort,
            "pane": e.pane,
            "tab": e.tab,
            "tmux_session": e.tmux_session,
            "session_id": e.session_id,
            "issues": e.issues,
            "ledger_id": e.ledger_id,
            "cwd": e.cwd,
            "prompt_head": e.prompt_head,
            "spawned_at": e.spawned_at,
            "status": e.status,
            "closed_at": e.closed_at,
            "close_reason": e.close_reason,
            "pane_alive": pane_alive,
            "tmux_alive": tmux_alive,
            "prompt_delivery": delivery.as_str(),
        }));
    }
    json!({ "workers": items, "count": items.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("tako-registry-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}-{}.yaml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn sample_record(pane: u64) -> RegisterSpawn {
        RegisterSpawn {
            label: Some("fix-390".into()),
            project: "tako".into(),
            agent: "claude".into(),
            model: None,
            effort: Some("high".into()),
            pane,
            tab: Some(3),
            tmux_session: Some(format!("tako-pane-{pane}")),
            issues: vec![390],
            ledger_id: Some("L1".into()),
            cwd: Some("/tmp/proj".into()),
            prompt_head: Some("Issue #390: ...".into()),
        }
    }

    /// mutate_at で直接登録するテスト用ヘルパー（env 非依存）
    fn register_at(path: &Path, record: RegisterSpawn) -> String {
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.pane == record.pane && entry.is_active() {
                    entry.status = "closed".into();
                    entry.closed_at = Some(now.clone());
                    entry.close_reason = Some("superseded".into());
                }
            }
            reg.next_id += 1;
            let id = reg.next_id.to_string();
            reg.workers.insert(
                id.clone(),
                WorkerEntry {
                    label: record.label.clone(),
                    project: record.project.clone(),
                    agent: record.agent.clone(),
                    model: record.model.clone(),
                    effort: record.effort.clone(),
                    pane: record.pane,
                    tab: record.tab,
                    tmux_session: record.tmux_session.clone(),
                    session_id: None,
                    issues: record.issues.clone(),
                    ledger_id: record.ledger_id.clone(),
                    cwd: record.cwd.clone(),
                    prompt_head: record.prompt_head.clone(),
                    spawned_at: now.clone(),
                    status: "active".into(),
                    closed_at: None,
                    close_reason: None,
                    prompt_delivered_at: None,
                },
            );
            reg.gc();
            id
        })
        .unwrap()
    }

    #[test]
    fn 登録と解決の往復ができる() {
        let path = temp_registry_file("roundtrip");
        let id = register_at(&path, sample_record(42));
        assert_eq!(id, "1");
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (rid, entry) = reg.resolve("1").unwrap();
        assert_eq!(rid, "1");
        assert_eq!(entry.pane, 42);
        assert_eq!(entry.project, "tako");
        assert_eq!(entry.tmux_session.as_deref(), Some("tako-pane-42"));
        assert!(entry.is_active());
        assert_eq!(entry.issues, vec![390]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn find_active_by_paneが引ける() {
        let path = temp_registry_file("bypane");
        register_at(&path, sample_record(10));
        register_at(&path, sample_record(20));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(20).unwrap();
        assert_eq!(entry.pane, 20);
        assert!(reg.find_active_by_pane(99).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 同一paneの再spawnは旧エントリをsupersededに倒す() {
        let path = temp_registry_file("supersede");
        let first = register_at(&path, sample_record(7));
        let second = register_at(&path, sample_record(7));
        assert_ne!(first, second);
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let old = &reg.workers[&first];
        assert_eq!(old.status, "closed");
        assert_eq!(old.close_reason.as_deref(), Some("superseded"));
        // active は新エントリだけ
        let (aid, _) = reg.find_active_by_pane(7).unwrap();
        assert_eq!(aid, &second);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mark_closedで明示closeが記録される() {
        let path = temp_registry_file("close");
        register_at(&path, sample_record(5));
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(&path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.pane == 5 && entry.is_active() {
                    entry.status = "closed".into();
                    entry.closed_at = Some(now.clone());
                    entry.close_reason = Some("explicit_close".into());
                }
            }
        })
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let entry = reg.workers.values().next().unwrap();
        assert_eq!(entry.status, "closed");
        assert_eq!(entry.close_reason.as_deref(), Some("explicit_close"));
        assert!(entry.closed_at.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 破損ファイルはerrで0件に丸めない() {
        let path = temp_registry_file("corrupt");
        std::fs::write(&path, "workers: [this is: not valid").unwrap();
        assert!(WorkerRegistry::load_from(&path).is_err());
        // mutate も Err（黙って空で上書きしない = #169 と同思想）
        let result = WorkerRegistry::mutate_at(&path, |_| ());
        assert!(result.is_err());
        // 破損ファイルはそのまま残る（bak からの復旧余地を消さない）
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("not valid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn session検出でsession_idとprompt_delivered_atが埋まる() {
        let path = temp_registry_file("detect");
        register_at(&path, sample_record(8));
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(&path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.is_active() && entry.tmux_session.as_deref() == Some("tako-pane-8") {
                    entry.session_id = Some("abc-123".into());
                    if entry.prompt_delivered_at.is_none() {
                        entry.prompt_delivered_at = Some(now.clone());
                    }
                }
            }
        })
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let entry = reg.workers.values().next().unwrap();
        assert_eq!(entry.session_id.as_deref(), Some("abc-123"));
        assert!(entry.prompt_delivered_at.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prompt_delivery_assessmentの分岐() {
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let mut entry = WorkerEntry {
            agent: "claude".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        // 猶予内 = pending
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch),
            PromptDelivery::Pending
        );
        // 猶予超過 = 未達疑い
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + PROMPT_DELIVERY_GRACE_SECS + 10),
            PromptDelivery::OverdueSuspect
        );
        // session_id 検出済み = delivered（時間に関係なく）
        entry.session_id = Some("abc".into());
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + 10_000),
            PromptDelivery::Delivered
        );
        // claude 以外は対象外
        let codex = WorkerEntry {
            agent: "codex".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&codex, now_epoch + 10_000),
            PromptDelivery::NotApplicable
        );
        // closed は対象外
        let closed = WorkerEntry {
            agent: "claude".into(),
            status: "closed".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&closed, now_epoch + 10_000),
            PromptDelivery::NotApplicable
        );
    }

    #[test]
    fn resolveの前方一致と曖昧エラー() {
        let path = temp_registry_file("resolve");
        for pane in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
            register_at(&path, sample_record(pane));
        }
        let reg = WorkerRegistry::load_from(&path).unwrap();
        // 完全一致優先（"1" は "10" "11" と前方一致するが完全一致 "1" が勝つ）
        let (id, _) = reg.resolve("1").unwrap();
        assert_eq!(id, "1");
        // 一意な前方一致は不可（"1" 完全一致があるため）だが、存在しない prefix はエラー
        assert!(reg.resolve("99").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn gcはclosedの古い順に削りactiveを守る() {
        let mut reg = WorkerRegistry::default();
        for i in 0..(MAX_WORKERS as u64 + 10) {
            reg.next_id += 1;
            let active = i >= 5; // 先頭 5 件だけ closed
            reg.workers.insert(
                reg.next_id.to_string(),
                WorkerEntry {
                    pane: i,
                    status: if active { "active" } else { "closed" }.into(),
                    spawned_at: format!("2026-07-19T00:{:02}:{:02}Z", i / 60, i % 60),
                    agent: "claude".into(),
                    ..Default::default()
                },
            );
        }
        reg.gc();
        // closed は 5 件しか無いので 5 件だけ削られ、active は全件残る
        assert_eq!(reg.workers.len(), MAX_WORKERS + 10 - 5);
        assert!(reg.workers.values().all(|e| e.is_active()));
    }

    #[test]
    fn list_payloadがライブ状態とdeliveryを含む() {
        let path = temp_registry_file("payload");
        register_at(&path, sample_record(30));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[999], // pane 30 は GUI に不在
            false,
        );
        assert_eq!(payload["count"], 1);
        let w = &payload["workers"][0];
        assert_eq!(w["worker_id"], "1");
        assert_eq!(w["pane_alive"], false);
        assert_eq!(w["tmux_alive"], true);
        assert_eq!(w["prompt_delivery"], "pending");
        assert_eq!(w["status"], "active");
        let _ = std::fs::remove_file(&path);
    }
}

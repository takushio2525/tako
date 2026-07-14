//! task_checkpoints — TaskCheckpoint の Store と YAML 永続化（Issue #242）
//!
//! sessions.rs / config_io と同パターン: `<data_dir>/task_checkpoints.yaml` に
//! 排他 flock + アトミック書き込み + 世代バックアップで永続化する。
//! データモデル（TaskCheckpoint / TaskPhase）は tako-core::task_checkpoint にある。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tako_core::task_checkpoint::{unix_now, TaskCheckpoint, TaskPhase};

/// チェックポイントファイルのパス
/// `TAKO_TASK_CHECKPOINTS_FILE` で上書き可能（隔離検証用）
pub fn store_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_TASK_CHECKPOINTS_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    tako_core::paths::data_dir().map(|d| d.join("task_checkpoints.yaml"))
}

/// task_checkpoints.yaml のトップレベルスキーマ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpointStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub checkpoints: Vec<TaskCheckpoint>,
}

fn default_version() -> u32 {
    1
}

impl Default for TaskCheckpointStore {
    fn default() -> Self {
        Self {
            version: 1,
            checkpoints: Vec::new(),
        }
    }
}

impl TaskCheckpointStore {
    pub fn find(&self, task_id: &str) -> Option<&TaskCheckpoint> {
        self.checkpoints.iter().find(|c| c.task_id == task_id)
    }

    pub fn find_mut(&mut self, task_id: &str) -> Option<&mut TaskCheckpoint> {
        self.checkpoints.iter_mut().find(|c| c.task_id == task_id)
    }

    pub fn upsert(&mut self, checkpoint: TaskCheckpoint) {
        if let Some(existing) = self.find_mut(&checkpoint.task_id) {
            *existing = checkpoint;
        } else {
            self.checkpoints.push(checkpoint);
        }
    }

    pub fn list_by_phase(&self, phase: Option<TaskPhase>) -> Vec<&TaskCheckpoint> {
        let mut items: Vec<_> = self
            .checkpoints
            .iter()
            .filter(|c| phase.is_none_or(|p| c.phase == p))
            .collect();
        items.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        items
    }

    pub fn find_active_by_pane(&self, pane_id: u64) -> Option<&TaskCheckpoint> {
        self.checkpoints
            .iter()
            .find(|c| c.pane_id == Some(pane_id) && !c.phase.is_terminal())
    }

    pub fn find_active_by_pane_mut(&mut self, pane_id: u64) -> Option<&mut TaskCheckpoint> {
        self.checkpoints
            .iter_mut()
            .find(|c| c.pane_id == Some(pane_id) && !c.phase.is_terminal())
    }

    pub fn next_task_id(&self) -> String {
        let max_n = self
            .checkpoints
            .iter()
            .filter_map(|c| c.task_id.strip_prefix("task-"))
            .filter_map(|s| s.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("task-{}", max_n + 1)
    }

    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("task_checkpoints.yaml の読み取りに失敗: {e}"))?;
        if content.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(&content)
            .map_err(|e| format!("task_checkpoints.yaml のパースに失敗: {e}"))
    }

    pub fn load() -> Result<Self, String> {
        let path = store_path().ok_or("データディレクトリを解決できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同パターン）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut store = Self::load_from(path)?;
        let result = f(&mut store);
        let content =
            serde_yaml::to_string(&store).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = store_path().ok_or("データディレクトリを解決できない")?;
        Self::mutate_at(&path, f)
    }
}

/// checkpoint action のレスポンス JSON を構築する
#[allow(clippy::too_many_arguments)]
pub fn checkpoint_payload(
    task_id: Option<&str>,
    pane: Option<u64>,
    issue: Option<u32>,
    branch: Option<&str>,
    phase: Option<&str>,
    last_commit: Option<&str>,
    agent: Option<&str>,
    model: Option<&str>,
    prompt_head: Option<&str>,
    suspended_reason: Option<&str>,
    project: Option<&str>,
    cwd: Option<&str>,
) -> Result<Value, String> {
    let phase_val = phase
        .map(|p| TaskPhase::parse(p).ok_or_else(|| format!("不明な phase: {p}")))
        .transpose()?
        .unwrap_or(TaskPhase::Running);

    let path = store_path().ok_or("データディレクトリを解決できない")?;
    let result = TaskCheckpointStore::mutate_at(&path, |store| {
        let id = task_id
            .map(String::from)
            .unwrap_or_else(|| store.next_task_id());
        let cp = TaskCheckpoint {
            task_id: id.clone(),
            pane_id: pane,
            issue,
            branch: branch.map(String::from),
            phase: phase_val,
            last_commit: last_commit.map(String::from),
            agent: agent.map(String::from),
            model: model.map(String::from),
            prompt_head: prompt_head.map(String::from),
            suspended_reason: suspended_reason.map(String::from),
            project: project.map(String::from),
            cwd: cwd.map(String::from),
            updated_at: unix_now(),
        };
        store.upsert(cp);
        id
    })?;

    Ok(json!({ "task_id": result, "phase": phase_val.as_str() }))
}

/// list action のレスポンス JSON を構築する
pub fn list_payload(phase_filter: Option<&str>) -> Result<Value, String> {
    let phase = phase_filter
        .map(|p| TaskPhase::parse(p).ok_or_else(|| format!("不明な phase: {p}")))
        .transpose()?;
    let store = TaskCheckpointStore::load()?;
    let items: Vec<Value> = store
        .list_by_phase(phase)
        .iter()
        .map(|c| checkpoint_to_json(c))
        .collect();
    Ok(json!({ "checkpoints": items, "count": items.len() }))
}

/// update action のレスポンス JSON（phase 変更 + 理由記録）
pub fn update_phase_payload(
    task_id: &str,
    phase: &str,
    reason: Option<&str>,
) -> Result<Value, String> {
    let new_phase = TaskPhase::parse(phase).ok_or_else(|| format!("不明な phase: {phase}"))?;
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    TaskCheckpointStore::mutate_at(&path, |store| {
        let cp = store
            .find_mut(task_id)
            .ok_or_else(|| format!("チェックポイントが見つからない: {task_id}"))?;
        cp.phase = new_phase;
        if let Some(r) = reason {
            cp.suspended_reason = Some(r.to_string());
        }
        cp.touch();
        Ok(json!({
            "task_id": cp.task_id,
            "phase": cp.phase.as_str(),
        }))
    })?
}

/// pane_id に紐づく active なチェックポイントの phase を Suspended に遷移させる
pub fn suspend_by_pane(pane_id: u64, reason: &str) -> Result<Option<String>, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    suspend_by_pane_at(&path, pane_id, reason)
}

/// パス指定版の suspend_by_pane（テスト・隔離用）
pub fn suspend_by_pane_at(
    path: &std::path::Path,
    pane_id: u64,
    reason: &str,
) -> Result<Option<String>, String> {
    TaskCheckpointStore::mutate_at(path, |store| {
        if let Some(cp) = store.find_active_by_pane_mut(pane_id) {
            cp.phase = TaskPhase::Suspended;
            cp.suspended_reason = Some(reason.to_string());
            cp.touch();
            Some(cp.task_id.clone())
        } else {
            None
        }
    })
}

fn checkpoint_to_json(c: &TaskCheckpoint) -> Value {
    let mut v = json!({
        "task_id": c.task_id,
        "phase": c.phase.as_str(),
        "updated_at": c.updated_at,
    });
    let obj = v.as_object_mut().unwrap();
    if let Some(p) = c.pane_id {
        obj.insert("pane_id".into(), json!(p));
    }
    if let Some(i) = c.issue {
        obj.insert("issue".into(), json!(i));
    }
    if let Some(ref b) = c.branch {
        obj.insert("branch".into(), json!(b));
    }
    if let Some(ref lc) = c.last_commit {
        obj.insert("last_commit".into(), json!(lc));
    }
    if let Some(ref a) = c.agent {
        obj.insert("agent".into(), json!(a));
    }
    if let Some(ref m) = c.model {
        obj.insert("model".into(), json!(m));
    }
    if let Some(ref ph) = c.prompt_head {
        obj.insert("prompt_head".into(), json!(ph));
    }
    if let Some(ref sr) = c.suspended_reason {
        obj.insert("suspended_reason".into(), json!(sr));
    }
    if let Some(ref p) = c.project {
        obj.insert("project".into(), json!(p));
    }
    if let Some(ref cwd) = c.cwd {
        obj.insert("cwd".into(), json!(cwd));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-task-cp-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_persistence() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("task_checkpoints.yaml");
        TaskCheckpointStore::mutate_at(&path, |store| {
            store.upsert(TaskCheckpoint {
                task_id: "task-1".into(),
                pane_id: Some(42),
                issue: Some(242),
                branch: Some("feat/242".into()),
                phase: TaskPhase::Running,
                last_commit: None,
                agent: Some("claude".into()),
                model: None,
                prompt_head: Some("impl #242".into()),
                suspended_reason: None,
                project: Some("tako".into()),
                cwd: Some("/tmp/tako".into()),
                updated_at: 1000,
            });
        })
        .unwrap();

        let store = TaskCheckpointStore::load_from(&path).unwrap();
        assert_eq!(store.version, 1);
        assert_eq!(store.checkpoints.len(), 1);
        let cp = store.find("task-1").unwrap();
        assert_eq!(cp.issue, Some(242));
        assert_eq!(cp.phase, TaskPhase::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suspend_by_pane_transitions_phase() {
        let dir = temp_dir("suspend");
        let path = dir.join("task_checkpoints.yaml");

        TaskCheckpointStore::mutate_at(&path, |store| {
            store.upsert(TaskCheckpoint {
                task_id: "task-1".into(),
                pane_id: Some(10),
                issue: None,
                branch: None,
                phase: TaskPhase::Running,
                last_commit: None,
                agent: None,
                model: None,
                prompt_head: None,
                suspended_reason: None,
                project: None,
                cwd: None,
                updated_at: 100,
            });
        })
        .unwrap();

        // suspend_by_pane_at で直接パスを指定（環境変数の並列テスト干渉を回避）
        let result = suspend_by_pane_at(&path, 10, "usage_limit").unwrap();
        assert_eq!(result, Some("task-1".into()));

        let store = TaskCheckpointStore::load_from(&path).unwrap();
        let cp = store.find("task-1").unwrap();
        assert_eq!(cp.phase, TaskPhase::Suspended);
        assert_eq!(cp.suspended_reason.as_deref(), Some("usage_limit"));

        let result3 = suspend_by_pane_at(&path, 99, "crash").unwrap();
        assert_eq!(result3, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_upsert_and_find() {
        let mut store = TaskCheckpointStore::default();
        let cp = TaskCheckpoint {
            task_id: "task-1".into(),
            pane_id: Some(42),
            issue: Some(242),
            branch: Some("feat/242".into()),
            phase: TaskPhase::Running,
            last_commit: None,
            agent: Some("claude".into()),
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            updated_at: 1000,
        };
        store.upsert(cp);
        assert_eq!(store.checkpoints.len(), 1);

        // upsert で上書き
        store.upsert(TaskCheckpoint {
            task_id: "task-1".into(),
            pane_id: Some(42),
            issue: Some(242),
            branch: Some("feat/242".into()),
            phase: TaskPhase::Verifying,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            updated_at: 2000,
        });
        assert_eq!(store.checkpoints.len(), 1);
        assert_eq!(store.find("task-1").unwrap().phase, TaskPhase::Verifying);
    }

    #[test]
    fn list_by_phase_filters_and_sorts() {
        let mut store = TaskCheckpointStore::default();
        store.upsert(TaskCheckpoint {
            task_id: "task-1".into(),
            phase: TaskPhase::Running,
            updated_at: 100,
            ..default_cp()
        });
        store.upsert(TaskCheckpoint {
            task_id: "task-2".into(),
            phase: TaskPhase::Suspended,
            updated_at: 200,
            ..default_cp()
        });
        store.upsert(TaskCheckpoint {
            task_id: "task-3".into(),
            phase: TaskPhase::Running,
            updated_at: 300,
            ..default_cp()
        });

        let running = store.list_by_phase(Some(TaskPhase::Running));
        assert_eq!(running.len(), 2);
        assert_eq!(running[0].task_id, "task-3");

        let all = store.list_by_phase(None);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn next_task_id_generation() {
        let mut store = TaskCheckpointStore::default();
        assert_eq!(store.next_task_id(), "task-1");
        store.upsert(TaskCheckpoint {
            task_id: "task-5".into(),
            phase: TaskPhase::Running,
            updated_at: 200,
            ..default_cp()
        });
        assert_eq!(store.next_task_id(), "task-6");
    }

    #[test]
    fn corrupt_yaml_returns_error() {
        let dir = temp_dir("corrupt");
        let path = dir.join("task_checkpoints.yaml");
        std::fs::write(&path, "{{invalid yaml").unwrap();
        let result = TaskCheckpointStore::load_from(&path);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_empty() {
        let dir = temp_dir("missing");
        let path = dir.join("nonexistent.yaml");
        let store = TaskCheckpointStore::load_from(&path).unwrap();
        assert!(store.checkpoints.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn e2e_checkpoint_list_update_restart() {
        let dir = temp_dir("e2e");
        let path = dir.join("task_checkpoints.yaml");
        std::env::set_var("TAKO_TASK_CHECKPOINTS_FILE", &path);

        // 1. ファイル不在から checkpoint 記録
        let result = checkpoint_payload(
            Some("test-42"),
            Some(100),
            Some(242),
            Some("feat/242"),
            Some("running"),
            None,
            Some("claude"),
            None,
            Some("Issue #242 の実装"),
            None,
            Some("tako"),
            Some("/tmp/tako"),
        )
        .unwrap();
        assert_eq!(result["task_id"], "test-42");
        assert_eq!(result["phase"], "running");

        // 2. list で確認
        let list = list_payload(None).unwrap();
        assert_eq!(list["count"], 1);
        assert_eq!(list["checkpoints"][0]["task_id"], "test-42");
        assert_eq!(list["checkpoints"][0]["issue"], 242);

        // 3. 同一 task_id で上書き（phase を verifying に）
        let updated = checkpoint_payload(
            Some("test-42"),
            Some(100),
            Some(242),
            Some("feat/242"),
            Some("verifying"),
            Some("abc1234"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(updated["phase"], "verifying");
        let list2 = list_payload(None).unwrap();
        assert_eq!(list2["count"], 1);

        // 4. 同一 pane で 2 つ目の checkpoint（異なる task_id）
        let auto_id = checkpoint_payload(
            None,
            Some(100),
            Some(243),
            Some("feat/243"),
            Some("running"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(auto_id["task_id"], "task-1");
        let list3 = list_payload(None).unwrap();
        assert_eq!(list3["count"], 2);

        // 5. update で suspended に遷移
        let upd = update_phase_payload("test-42", "suspended", Some("usage_limit")).unwrap();
        assert_eq!(upd["phase"], "suspended");

        // 6. phase フィルタで suspended だけ表示
        let suspended = list_payload(Some("suspended")).unwrap();
        assert_eq!(suspended["count"], 1);
        assert_eq!(suspended["checkpoints"][0]["task_id"], "test-42");
        assert_eq!(
            suspended["checkpoints"][0]["suspended_reason"],
            "usage_limit"
        );

        // 7. プロセス再起動シミュレーション（新しい load で同じファイルを読む）
        let store = TaskCheckpointStore::load().unwrap();
        assert_eq!(store.checkpoints.len(), 2);
        assert_eq!(store.find("test-42").unwrap().phase, TaskPhase::Suspended);
        assert_eq!(store.version, 1);

        // 8. 存在しない task_id の update はエラー
        let err = update_phase_payload("nonexistent", "done", None);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("見つからない"));

        std::env::remove_var("TAKO_TASK_CHECKPOINTS_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn default_cp() -> TaskCheckpoint {
        TaskCheckpoint {
            task_id: String::new(),
            pane_id: None,
            issue: None,
            branch: None,
            phase: TaskPhase::Queued,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            updated_at: 0,
        }
    }
}

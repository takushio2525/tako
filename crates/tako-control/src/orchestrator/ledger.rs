//! orchestrator::ledger — 委任台帳（Issue #292）
//!
//! spawn / run 経路で自動収集した委任記録と、検収記録・事後修正を
//! `<data_dir>/orchestrator/ledger.yaml` に追記型で蓄積する。
//! task_type × model の集計（成功率・差し戻し率・平均所要）を提供し、
//! master の model / effort 割り当て判断を実データで育てる基盤。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// task_type の統制語彙（Issue #292）。勝手に増やさない
const VALID_TASK_TYPES: &[&str] = &[
    "bugfix-rooted",
    "bugfix-unrooted",
    "investigation",
    "feature-verifiable",
    "feature-ui",
    "docs",
    "review",
];

pub fn validate_task_type(t: &str) -> Result<(), String> {
    if VALID_TASK_TYPES.contains(&t) {
        Ok(())
    } else {
        Err(format!(
            "不正な task_type '{t}'。使用可能: {}",
            VALID_TASK_TYPES.join(", ")
        ))
    }
}

/// 台帳ファイルのパス
pub fn ledger_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("ledger.yaml"))
}

/// 台帳エントリ（1 行 = 1 委任記録）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// 一意 ID（spawn_<epoch_ms>）
    pub id: String,
    /// ISO 8601 タイムスタンプ
    pub ts: String,
    /// プロジェクトキー
    pub project: String,
    /// spawn 時のラベル
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Issue 番号（プロンプトから抽出）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// 統制語彙のタスク種別
    pub task_type: String,
    /// 使用モデル
    pub model: String,
    /// 使用 effort
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// エージェント種別
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 所要時間（秒）。run 完了時に記録
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// 終了時 ctx%
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx_percent: Option<u64>,
    /// WORKER_ERROR 有無
    #[serde(default)]
    pub had_error: bool,

    // --- 層2: 検収記録 ---
    /// 検収結果: pass / rework / fail
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// 差し戻し回数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
    /// 検収メモ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    // --- 層3: ユーザーフィードバック ---
    /// 検収 pass だが実使用で問題発覚
    #[serde(default)]
    pub post_issue: bool,
    /// 事後修正メモ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amend_note: Option<String>,
}

/// 台帳全体
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub entries: Vec<LedgerEntry>,
}

impl Ledger {
    pub fn load() -> Result<Self, String> {
        let path = ledger_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("ledger.yaml の読み取りに失敗: {e}"))?;
        if content.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(&content).map_err(|e| format!("ledger.yaml のパースに失敗: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = ledger_path().ok_or("ホームディレクトリが取得できない")?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)
    }

    /// ロック付き read-modify-write
    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = ledger_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut ledger = Self::load_from(path)?;
        let result = f(&mut ledger);
        ledger.save_to(path)?;
        Ok(result)
    }

    /// エントリを追加
    pub fn append(&mut self, entry: LedgerEntry) {
        self.entries.push(entry);
    }

    /// ID でエントリを検索
    pub fn find_mut(&mut self, id: &str) -> Option<&mut LedgerEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// task_type × model の集計
    pub fn stats(&self) -> Vec<StatRow> {
        use std::collections::BTreeMap;
        #[derive(Default)]
        struct Acc {
            total: u32,
            pass: u32,
            rework: u32,
            fail: u32,
            post_issue: u32,
            durations: Vec<u64>,
        }
        let mut map: BTreeMap<(String, String), Acc> = BTreeMap::new();
        for e in &self.entries {
            let key = (e.task_type.clone(), e.model.clone());
            let acc = map.entry(key).or_default();
            acc.total += 1;
            match e.outcome.as_deref() {
                Some("pass") => acc.pass += 1,
                Some("rework") => acc.rework += 1,
                Some("fail") => acc.fail += 1,
                _ => {}
            }
            if e.post_issue {
                acc.post_issue += 1;
            }
            if let Some(d) = e.duration_seconds {
                acc.durations.push(d);
            }
        }
        map.into_iter()
            .map(|((task_type, model), acc)| {
                let avg_duration = if acc.durations.is_empty() {
                    None
                } else {
                    Some(acc.durations.iter().sum::<u64>() / acc.durations.len() as u64)
                };
                let evaluated = acc.pass + acc.rework + acc.fail;
                StatRow {
                    task_type,
                    model,
                    total: acc.total,
                    pass: acc.pass,
                    rework: acc.rework,
                    fail: acc.fail,
                    post_issue: acc.post_issue,
                    pass_rate: if evaluated > 0 {
                        Some((acc.pass as f64 / evaluated as f64 * 100.0).round() as u32)
                    } else {
                        None
                    },
                    rework_rate: if evaluated > 0 {
                        Some((acc.rework as f64 / evaluated as f64 * 100.0).round() as u32)
                    } else {
                        None
                    },
                    avg_duration_seconds: avg_duration,
                    unevaluated: acc.total - evaluated,
                }
            })
            .collect()
    }

    /// 未評価（outcome なし）のエントリ数
    pub fn unevaluated_count(&self) -> usize {
        self.entries.iter().filter(|e| e.outcome.is_none()).count()
    }

    /// project が prefix に前方一致するエントリを除去し、除去件数を返す
    pub fn prune_by_project_prefix(&mut self, prefix: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !e.project.starts_with(prefix));
        before - self.entries.len()
    }
}

/// 集計行
#[derive(Debug, Clone, Serialize)]
pub struct StatRow {
    pub task_type: String,
    pub model: String,
    pub total: u32,
    pub pass: u32,
    pub rework: u32,
    pub fail: u32,
    pub post_issue: u32,
    pub pass_rate: Option<u32>,
    pub rework_rate: Option<u32>,
    pub avg_duration_seconds: Option<u64>,
    pub unevaluated: u32,
}

/// spawn 時の自動記録用 ID 生成
pub fn generate_id() -> String {
    let epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("spawn_{epoch_ms}")
}

/// ISO 8601 タイムスタンプ
pub fn now_iso() -> String {
    // UTC ベースで簡易実装
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    // 2000-01-01 からの日数計算（簡易）
    let mut y = 1970u64;
    let mut remaining_days = days;
    loop {
        let year_days = if is_leap(y) { 366 } else { 365 };
        if remaining_days < year_days {
            break;
        }
        remaining_days -= year_days;
        y += 1;
    }
    let month_days: &[u64] = if is_leap(y) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 0u64;
    for &md in month_days {
        if remaining_days < md {
            break;
        }
        remaining_days -= md;
        mo += 1;
    }
    format!(
        "{y:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}Z",
        mo + 1,
        remaining_days + 1,
    )
}

fn is_leap(y: u64) -> bool {
    y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
}

/// spawn 経路で呼ぶ自動記録。失敗しても spawn は止めない
pub fn record_spawn(
    project: &str,
    label: Option<&str>,
    issue: Option<&str>,
    task_type: Option<&str>,
    model: &str,
    effort: Option<&str>,
    agent: Option<&str>,
) -> Result<String, String> {
    let task_type = task_type.unwrap_or("investigation");
    validate_task_type(task_type)?;
    let id = generate_id();
    let entry = LedgerEntry {
        id: id.clone(),
        ts: now_iso(),
        project: project.to_string(),
        label: label.map(str::to_string),
        issue: issue.map(str::to_string),
        task_type: task_type.to_string(),
        model: model.to_string(),
        effort: effort.map(str::to_string),
        agent: agent.map(str::to_string),
        duration_seconds: None,
        ctx_percent: None,
        had_error: false,
        outcome: None,
        rounds: None,
        note: None,
        post_issue: false,
        amend_note: None,
    };
    Ledger::mutate(|l| l.append(entry))?;
    Ok(id)
}

/// outcome の検証
pub fn validate_outcome(o: &str) -> Result<(), String> {
    match o {
        "pass" | "rework" | "fail" => Ok(()),
        _ => Err(format!(
            "不正な outcome '{o}'。使用可能: pass, rework, fail"
        )),
    }
}

/// 検収記録
pub fn record_outcome(
    id: &str,
    outcome: &str,
    rounds: Option<u32>,
    note: Option<&str>,
) -> Result<(), String> {
    validate_outcome(outcome)?;
    Ledger::mutate(|l| {
        let entry = l
            .find_mut(id)
            .ok_or_else(|| format!("エントリ '{id}' が見つからない"))?;
        entry.outcome = Some(outcome.to_string());
        entry.rounds = rounds;
        entry.note = note.map(str::to_string);
        Ok(())
    })?
}

/// 事後修正
pub fn amend_entry(id: &str, amend_note: &str) -> Result<(), String> {
    Ledger::mutate(|l| {
        let entry = l
            .find_mut(id)
            .ok_or_else(|| format!("エントリ '{id}' が見つからない"))?;
        entry.post_issue = true;
        entry.amend_note = Some(amend_note.to_string());
        Ok(())
    })?
}

/// run 完了時に所要時間と ctx% を記録
pub fn update_completion(
    id: &str,
    duration_seconds: Option<u64>,
    ctx_percent: Option<u64>,
    had_error: bool,
) -> Result<(), String> {
    Ledger::mutate(|l| {
        if let Some(entry) = l.find_mut(id) {
            entry.duration_seconds = duration_seconds;
            entry.ctx_percent = ctx_percent;
            entry.had_error = had_error;
        }
    })
}

/// judgment-defaults.md のバイナリ埋め込み
pub const JUDGMENT_DEFAULTS: &str = include_str!("judgment_defaults.md");

/// judgment-local.md のパス
pub fn judgment_local_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("judgment-local.md"))
}

/// judgment テキストの二層合成（雛形 → ローカル優先）。
/// system prompt の model-policy ブロックの後に注入する
pub fn build_judgment_section() -> String {
    let mut result = String::new();
    result.push_str("\n\n## Delegation Judgment Criteria\n\n");
    result.push_str("### Built-in Defaults\n\n");
    result.push_str(JUDGMENT_DEFAULTS.trim());

    if let Some(local_path) = judgment_local_path() {
        if local_path.is_file() {
            if let Ok(local) = std::fs::read_to_string(&local_path) {
                if !local.trim().is_empty() {
                    result.push_str("\n\n### Local Overrides (take precedence)\n\n");
                    result.push_str(local.trim());
                }
            }
        }
    }

    result.push_str("\n\n### Survey Frequency Control\n\n");
    result.push_str(
        "When there are unevaluated deliveries (tasks with no `outcome` in the ledger), \
         you may ask the user \"How did that <label> turn out?\" — but ONLY:\n\
         - At a natural pause (between tasks, not mid-flow)\n\
         - At most 1-2 items per pause\n\
         - Never when unevaluated count is zero\n\
         Use `tako_orchestrator_ledger(action: \"stats\")` to check before surveying.",
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-ledger-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_task_type_accepts_valid() {
        assert!(validate_task_type("bugfix-rooted").is_ok());
        assert!(validate_task_type("docs").is_ok());
        assert!(validate_task_type("review").is_ok());
    }

    #[test]
    fn validate_task_type_rejects_invalid() {
        let err = validate_task_type("random").unwrap_err();
        assert!(err.contains("不正な task_type"));
        assert!(err.contains("bugfix-rooted"));
    }

    #[test]
    fn validate_outcome_accepts_valid() {
        assert!(validate_outcome("pass").is_ok());
        assert!(validate_outcome("rework").is_ok());
        assert!(validate_outcome("fail").is_ok());
    }

    #[test]
    fn validate_outcome_rejects_invalid() {
        assert!(validate_outcome("success").is_err());
    }

    #[test]
    fn ledger_roundtrip() {
        let dir = isolated_dir("roundtrip");
        let path = dir.join("ledger.yaml");

        let entry = LedgerEntry {
            id: "spawn_1234".into(),
            ts: "2026-07-17T00:00:00Z".into(),
            project: "tako".into(),
            label: Some("test-label".into()),
            issue: Some("#292".into()),
            task_type: "bugfix-rooted".into(),
            model: "claude-opus-4-6".into(),
            effort: Some("max".into()),
            agent: Some("claude".into()),
            duration_seconds: Some(120),
            ctx_percent: Some(42),
            had_error: false,
            outcome: Some("pass".into()),
            rounds: Some(1),
            note: Some("OK".into()),
            post_issue: false,
            amend_note: None,
        };

        let mut ledger = Ledger::default();
        ledger.append(entry);
        ledger.save_to(&path).unwrap();

        let loaded = Ledger::load_from(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].id, "spawn_1234");
        assert_eq!(loaded.entries[0].project, "tako");
        assert_eq!(loaded.entries[0].outcome.as_deref(), Some("pass"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ledger_stats_computes_correctly() {
        let entries = vec![
            LedgerEntry {
                id: "s1".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "bugfix-rooted".into(),
                model: "opus".into(),
                effort: None,
                agent: None,
                duration_seconds: Some(100),
                ctx_percent: None,
                had_error: false,
                outcome: Some("pass".into()),
                rounds: Some(1),
                note: None,
                post_issue: false,
                amend_note: None,
            },
            LedgerEntry {
                id: "s2".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "bugfix-rooted".into(),
                model: "opus".into(),
                effort: None,
                agent: None,
                duration_seconds: Some(200),
                ctx_percent: None,
                had_error: false,
                outcome: Some("rework".into()),
                rounds: Some(2),
                note: None,
                post_issue: false,
                amend_note: None,
            },
            LedgerEntry {
                id: "s3".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "docs".into(),
                model: "sonnet".into(),
                effort: None,
                agent: None,
                duration_seconds: None,
                ctx_percent: None,
                had_error: false,
                outcome: None,
                rounds: None,
                note: None,
                post_issue: false,
                amend_note: None,
            },
        ];
        let ledger = Ledger { entries };
        let stats = ledger.stats();

        assert_eq!(stats.len(), 2);
        let br = stats
            .iter()
            .find(|s| s.task_type == "bugfix-rooted")
            .unwrap();
        assert_eq!(br.total, 2);
        assert_eq!(br.pass, 1);
        assert_eq!(br.rework, 1);
        assert_eq!(br.pass_rate, Some(50));
        assert_eq!(br.rework_rate, Some(50));
        assert_eq!(br.avg_duration_seconds, Some(150));

        let d = stats.iter().find(|s| s.task_type == "docs").unwrap();
        assert_eq!(d.total, 1);
        assert_eq!(d.unevaluated, 1);
        assert_eq!(d.pass_rate, None);
    }

    #[test]
    fn ledger_mutate_at_creates_and_appends() {
        let dir = isolated_dir("mutate");
        let path = dir.join("ledger.yaml");

        Ledger::mutate_at(&path, |l| {
            l.append(LedgerEntry {
                id: "s1".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "docs".into(),
                model: "m".into(),
                effort: None,
                agent: None,
                duration_seconds: None,
                ctx_percent: None,
                had_error: false,
                outcome: None,
                rounds: None,
                note: None,
                post_issue: false,
                amend_note: None,
            });
        })
        .unwrap();

        Ledger::mutate_at(&path, |l| {
            l.append(LedgerEntry {
                id: "s2".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "review".into(),
                model: "m".into(),
                effort: None,
                agent: None,
                duration_seconds: None,
                ctx_percent: None,
                had_error: false,
                outcome: None,
                rounds: None,
                note: None,
                post_issue: false,
                amend_note: None,
            });
        })
        .unwrap();

        let loaded = Ledger::load_from(&path).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ledger_find_and_amend() {
        let dir = isolated_dir("amend");
        let path = dir.join("ledger.yaml");

        Ledger::mutate_at(&path, |l| {
            l.append(LedgerEntry {
                id: "s1".into(),
                ts: "t".into(),
                project: "p".into(),
                label: None,
                issue: None,
                task_type: "docs".into(),
                model: "m".into(),
                effort: None,
                agent: None,
                duration_seconds: None,
                ctx_percent: None,
                had_error: false,
                outcome: Some("pass".into()),
                rounds: Some(1),
                note: None,
                post_issue: false,
                amend_note: None,
            });
        })
        .unwrap();

        Ledger::mutate_at(&path, |l| {
            let entry = l.find_mut("s1").unwrap();
            entry.post_issue = true;
            entry.amend_note = Some("実使用でバグ発覚".into());
        })
        .unwrap();

        let loaded = Ledger::load_from(&path).unwrap();
        assert!(loaded.entries[0].post_issue);
        assert_eq!(
            loaded.entries[0].amend_note.as_deref(),
            Some("実使用でバグ発覚")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unevaluated_count() {
        let ledger = Ledger {
            entries: vec![
                LedgerEntry {
                    id: "s1".into(),
                    ts: "t".into(),
                    project: "p".into(),
                    label: None,
                    issue: None,
                    task_type: "docs".into(),
                    model: "m".into(),
                    effort: None,
                    agent: None,
                    duration_seconds: None,
                    ctx_percent: None,
                    had_error: false,
                    outcome: Some("pass".into()),
                    rounds: None,
                    note: None,
                    post_issue: false,
                    amend_note: None,
                },
                LedgerEntry {
                    id: "s2".into(),
                    ts: "t".into(),
                    project: "p".into(),
                    label: None,
                    issue: None,
                    task_type: "docs".into(),
                    model: "m".into(),
                    effort: None,
                    agent: None,
                    duration_seconds: None,
                    ctx_percent: None,
                    had_error: false,
                    outcome: None,
                    rounds: None,
                    note: None,
                    post_issue: false,
                    amend_note: None,
                },
            ],
        };
        assert_eq!(ledger.unevaluated_count(), 1);
    }

    #[test]
    fn now_iso_format() {
        let ts = now_iso();
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        assert_eq!(ts.len(), 20);
    }

    #[test]
    fn judgment_section_includes_defaults() {
        let section = build_judgment_section();
        assert!(section.contains("Delegation Judgment Criteria"));
        assert!(section.contains("Built-in Defaults"));
        assert!(section.contains("Survey Frequency Control"));
    }

    fn make_entry(id: &str, project: &str) -> LedgerEntry {
        LedgerEntry {
            id: id.into(),
            ts: "t".into(),
            project: project.into(),
            label: None,
            issue: None,
            task_type: "docs".into(),
            model: "m".into(),
            effort: None,
            agent: None,
            duration_seconds: None,
            ctx_percent: None,
            had_error: false,
            outcome: None,
            rounds: None,
            note: None,
            post_issue: false,
            amend_note: None,
        }
    }

    #[test]
    fn prune_removes_matching_entries() {
        let mut ledger = Ledger {
            entries: vec![
                make_entry("s1", "tako-selftest-165"),
                make_entry("s2", "tako"),
                make_entry("s3", "tako-selftest-200"),
                make_entry("s4", "other-project"),
            ],
        };
        let removed = ledger.prune_by_project_prefix("tako-selftest-");
        assert_eq!(removed, 2);
        assert_eq!(ledger.entries.len(), 2);
        assert_eq!(ledger.entries[0].id, "s2");
        assert_eq!(ledger.entries[1].id, "s4");
    }

    #[test]
    fn prune_zero_matches() {
        let mut ledger = Ledger {
            entries: vec![make_entry("s1", "tako"), make_entry("s2", "other")],
        };
        let removed = ledger.prune_by_project_prefix("tako-selftest-");
        assert_eq!(removed, 0);
        assert_eq!(ledger.entries.len(), 2);
    }

    #[test]
    fn prune_with_file_roundtrip() {
        let dir = isolated_dir("prune");
        let path = dir.join("ledger.yaml");

        let mut ledger = Ledger::default();
        ledger.append(make_entry("s1", "tako-selftest-1"));
        ledger.append(make_entry("s2", "real-project"));
        ledger.append(make_entry("s3", "tako-selftest-2"));
        ledger.save_to(&path).unwrap();

        let removed =
            Ledger::mutate_at(&path, |l| l.prune_by_project_prefix("tako-selftest-")).unwrap();
        assert_eq!(removed, 2);

        let loaded = Ledger::load_from(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].project, "real-project");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! orchestrator::ledger — 委任台帳（Issue #292）
//!
//! spawn / run 経路で自動収集した委任記録と、検収記録・事後修正を
//! `<data_dir>/orchestrator/ledger.yaml` に追記型で蓄積する。
//! task_type × model の集計（成功率・差し戻し率・平均所要）を提供し、
//! master の model / effort 割り当て判断を実データで育てる基盤。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use tako_core::load_issue::YamlReason;

use super::{ConfigError, ConfigFile};

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
    /// spawn が宣言したモデル（CLI の既定に委ねたら `(default)`）
    pub model: String,
    /// 実際に使われたモデル（#1638。worker レジストリが transcript で確かめたときに書く）。
    /// 未確定・観測しない系統（codex / agy）は None で、集計は宣言の `model` へ倒す。
    /// #1638 前のファイルには無い = serde の default で None として読める（移行は要らない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_actual: Option<String>,
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
    // 読み書きの口は 2 つ（#2365。`ConfigError` の作法）: `String` を返す口は日本語のまま
    // （spawn の自動記録・実際のモデルの書き込みの失敗の stderr）、断りを表示言語で返す
    // MCP の `tako_orchestrator_ledger`（dispatch）と CLI の `tako orchestrator ledger`（手元の処理）は
    // 型の口（`*_typed`）を通す

    pub fn load() -> Result<Self, String> {
        Self::load_typed().map_err(ConfigError::into_ja)
    }

    /// [`Self::load`] の型の口
    pub fn load_typed() -> Result<Self, ConfigError> {
        let path = ledger_path().ok_or(ConfigError::HomeUnresolved)?;
        Self::load_from_typed(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        Self::load_from_typed(path).map_err(ConfigError::into_ja)
    }

    /// [`Self::load_from`] の型の口
    pub fn load_from_typed(path: &Path) -> Result<Self, ConfigError> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            file: ConfigFile::Ledger,
            error: e.to_string(),
        })?;
        if content.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(&content).map_err(|e| ConfigError::Parse {
            file: ConfigFile::Ledger,
            reason: YamlReason::of(&e, &content),
        })
    }

    pub fn save(&self) -> Result<(), String> {
        let path = ledger_path().ok_or_else(|| ConfigError::HomeUnresolved.into_ja())?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        self.save_to_typed(path).map_err(ConfigError::into_ja)
    }

    /// [`Self::save_to`] の型の口
    fn save_to_typed(&self, path: &Path) -> Result<(), ConfigError> {
        let content =
            serde_yaml::to_string(self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        Ok(crate::config_io::atomic_write_with_backup_typed(
            path, &content,
        )?)
    }

    /// ロック付き read-modify-write
    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        Self::mutate_typed(f).map_err(ConfigError::into_ja)
    }

    /// [`Self::mutate`] の型の口
    pub fn mutate_typed<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, ConfigError> {
        let path = ledger_path().ok_or(ConfigError::HomeUnresolved)?;
        Self::mutate_at_typed(&path, f)
    }

    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        Self::mutate_at_typed(path, f).map_err(ConfigError::into_ja)
    }

    /// [`Self::mutate_at`] の型の口
    fn mutate_at_typed<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, ConfigError> {
        let _lock = crate::config_io::lock_exclusive_typed(path)?;
        let mut ledger = Self::load_from_typed(path)?;
        let result = f(&mut ledger);
        ledger.save_to_typed(path)?;
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

    /// task_type × model の集計。
    ///
    /// #1638: model は**実際に使われたモデル**（`model_actual`）で数える。未確定・観測しない系統の
    /// エントリは宣言の `model` で数え、行を分ける（`model_source` = actual / declared）。同じ名前でも
    /// 「確かめた」行と「宣言のまま」の行を混ぜると、宣言が嘘だった分が実態の行へ紛れ込むため。
    /// `mismatched` は宣言と違うモデルで動いた件数（宣言 5.5 → 実際 5 なら実際 5 の行に数える）
    pub fn stats(&self) -> Vec<StatRow> {
        self.stats_with(super::registry::legacy_1638())
    }

    /// [`Ledger::stats`] の A/B を引数で受ける版（`legacy` = 宣言のモデルで数える修正前）
    pub fn stats_with(&self, legacy: bool) -> Vec<StatRow> {
        use std::collections::BTreeMap;
        #[derive(Default)]
        struct Acc {
            total: u32,
            pass: u32,
            rework: u32,
            fail: u32,
            post_issue: u32,
            mismatched: u32,
            durations: Vec<u64>,
        }
        let mut map: BTreeMap<(String, String, &'static str), Acc> = BTreeMap::new();
        for e in &self.entries {
            let (model, source) = match e.model_actual.as_deref().filter(|_| !legacy) {
                Some(actual) => (actual.to_string(), "actual"),
                None => (e.model.clone(), "declared"),
            };
            let mismatched = source == "actual"
                && tako_core::model_check::check(Some(&e.model), Some(&model))
                    == tako_core::model_check::ModelCheck::Mismatch;
            let key = (e.task_type.clone(), model, source);
            let acc = map.entry(key).or_default();
            acc.total += 1;
            if mismatched {
                acc.mismatched += 1;
            }
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
            .map(|((task_type, model, model_source), acc)| {
                let avg_duration = if acc.durations.is_empty() {
                    None
                } else {
                    Some(acc.durations.iter().sum::<u64>() / acc.durations.len() as u64)
                };
                let evaluated = acc.pass + acc.rework + acc.fail;
                StatRow {
                    task_type,
                    model,
                    model_source,
                    mismatched: acc.mismatched,
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
    /// 実際に使われたモデル（`model_source` = actual）か、確かめられなかった宣言（declared）
    pub model: String,
    /// `model` の出どころ（#1638）。actual = transcript で確かめた / declared = 宣言のまま
    pub model_source: &'static str,
    /// 宣言と違うモデルで動いた件数（#1638。actual の行だけが持ちうる）
    pub mismatched: u32,
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
        model_actual: None,
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

/// 実際に使われたモデルを書く（#1638。worker レジストリが transcript で確かめたとき 1 回だけ呼ぶ）。
/// 既に書いてあれば変えない（spawn 時の解決先を残す）。エントリが無ければ何もしない
pub fn record_model_actual(id: &str, actual: &str) -> Result<(), String> {
    let path = ledger_path().ok_or_else(|| ConfigError::HomeUnresolved.into_ja())?;
    record_model_actual_at(&path, id, actual)
}

/// [`record_model_actual`] のパス指定版
pub fn record_model_actual_at(path: &Path, id: &str, actual: &str) -> Result<(), String> {
    // 書く必要が無ければロックも書き込みもしない（台帳が無い / 該当なし / 確定済み）
    let wants = |l: &Ledger| {
        l.entries
            .iter()
            .any(|e| e.id == id && e.model_actual.is_none())
    };
    if !wants(&Ledger::load_from(path)?) {
        return Ok(());
    }
    Ledger::mutate_at(path, |l| {
        if let Some(entry) = l.find_mut(id).filter(|e| e.model_actual.is_none()) {
            entry.model_actual = Some(actual.to_string());
        }
    })
}

/// outcome の検証
pub fn validate_outcome(o: &str) -> Result<(), String> {
    validate_outcome_typed(o).map_err(ConfigError::into_ja)
}

/// [`validate_outcome`] の型の口
fn validate_outcome_typed(o: &str) -> Result<(), ConfigError> {
    match o {
        "pass" | "rework" | "fail" => Ok(()),
        _ => Err(ConfigError::LedgerOutcomeInvalid(o.to_string())),
    }
}

/// 検収記録
pub fn record_outcome(
    id: &str,
    outcome: &str,
    rounds: Option<u32>,
    note: Option<&str>,
) -> Result<(), String> {
    record_outcome_typed(id, outcome, rounds, note).map_err(ConfigError::into_ja)
}

/// [`record_outcome`] の型の口
pub fn record_outcome_typed(
    id: &str,
    outcome: &str,
    rounds: Option<u32>,
    note: Option<&str>,
) -> Result<(), ConfigError> {
    validate_outcome_typed(outcome)?;
    Ledger::mutate_typed(|l| {
        let entry = l
            .find_mut(id)
            .ok_or_else(|| ConfigError::LedgerEntryNotFound(id.to_string()))?;
        entry.outcome = Some(outcome.to_string());
        entry.rounds = rounds;
        entry.note = note.map(str::to_string);
        Ok(())
    })?
}

/// 事後修正
pub fn amend_entry(id: &str, amend_note: &str) -> Result<(), String> {
    amend_entry_typed(id, amend_note).map_err(ConfigError::into_ja)
}

/// [`amend_entry`] の型の口
pub fn amend_entry_typed(id: &str, amend_note: &str) -> Result<(), ConfigError> {
    Ledger::mutate_typed(|l| {
        let entry = l
            .find_mut(id)
            .ok_or_else(|| ConfigError::LedgerEntryNotFound(id.to_string()))?;
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
            model_actual: None,
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
                model_actual: None,
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
                model_actual: None,
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
                model_actual: None,
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
                model_actual: None,
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
                model_actual: None,
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
                model_actual: None,
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
                    model_actual: None,
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
                    model_actual: None,
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
            model_actual: None,
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

    /// 宣言 `declared`・実際 `actual` の 1 件（#1638）
    fn entry_1638(id: &str, declared: &str, actual: Option<&str>, outcome: &str) -> LedgerEntry {
        LedgerEntry {
            task_type: "bugfix-rooted".into(),
            model: declared.into(),
            model_actual: actual.map(str::to_string),
            outcome: Some(outcome.into()),
            ..make_entry(id, "tako")
        }
    }

    /// 受け入れ条件 2: stats は実際のモデルで数える。実際のモデルが取れないものは宣言で数えて
    /// 行を分ける（model_source = declared）。宣言と違ったものは mismatched に数える
    #[test]
    fn statsは実際のモデルで集計する_1638() {
        let ledger = Ledger {
            entries: vec![
                // 宣言 5.5 → 実際 5（無言のフォールバック）が 2 件
                entry_1638("a", "claude-opus-5-5", Some("claude-opus-5"), "pass"),
                entry_1638("b", "claude-opus-5-5", Some("claude-opus-5"), "rework"),
                // 宣言どおり 5 が 1 件
                entry_1638("c", "claude-opus-5", Some("claude-opus-5"), "pass"),
                // 実際を確かめられなかった 5.5（#1638 前の記録・codex / agy）
                entry_1638("d", "claude-opus-5-5", None, "pass"),
                // CLI の既定に委ねた（宣言なし）→ 実際のモデルの行へ。食い違いには数えない
                entry_1638("e", "(default)", Some("claude-opus-5"), "fail"),
            ],
        };
        let rows = ledger.stats_with(false);
        let row = |model: &str, source: &str| {
            rows.iter()
                .find(|r| r.model == model && r.model_source == source)
                .unwrap_or_else(|| panic!("{model} / {source} の行が無い: {rows:?}"))
        };
        let actual5 = row("claude-opus-5", "actual");
        assert_eq!(actual5.total, 4, "実際に 5 で動いた 4 件");
        assert_eq!(actual5.mismatched, 2, "宣言 5.5 だった 2 件");
        assert_eq!((actual5.pass, actual5.rework, actual5.fail), (2, 1, 1));
        let declared55 = row("claude-opus-5-5", "declared");
        assert_eq!(declared55.total, 1);
        assert_eq!(declared55.mismatched, 0);
        assert!(
            !rows
                .iter()
                .any(|r| r.model == "claude-opus-5-5" && r.model_source == "actual"),
            "5.5 で動いた記録は無い: {rows:?}"
        );
        // A/B（TAKO_1638_LEGACY=1 相当）: 修正前は宣言で数える = 5.5 の行に 3 件が載る嘘
        let legacy = ledger.stats_with(true);
        let l55 = legacy
            .iter()
            .find(|r| r.model == "claude-opus-5-5")
            .unwrap();
        assert_eq!((l55.total, l55.model_source), (3, "declared"));
        // JSON の形（MCP / CLI の stats がそのまま出す）
        let v = serde_json::to_value(actual5).unwrap();
        assert_eq!(v["model_source"], "actual");
        assert_eq!(v["mismatched"], 2);
    }

    /// 実際のモデルは 1 回だけ書く（確定済みは変えない）。旧ファイル（キー無し）はそのまま読める
    #[test]
    fn 実際のモデルを1回だけ書き旧ファイルも読める_1638() {
        let dir = tako_core::test_residue::ScratchDir::new("ledger-1638");
        let path = dir.path().join("ledger.yaml");
        std::fs::write(
            &path,
            "entries:\n- id: spawn_1\n  ts: t\n  project: tako\n  task_type: docs\n  model: claude-opus-5-5\n",
        )
        .unwrap();
        let before = Ledger::load_from(&path).unwrap();
        assert_eq!(
            before.entries[0].model_actual, None,
            "旧ファイルは None で読める"
        );
        record_model_actual_at(&path, "spawn_1", "claude-opus-5").unwrap();
        record_model_actual_at(&path, "spawn_1", "claude-sonnet-5").unwrap();
        // 該当の無い ID・台帳の無いパスは何もしない
        record_model_actual_at(&path, "spawn_x", "claude-opus-5").unwrap();
        record_model_actual_at(&dir.path().join("無い.yaml"), "spawn_1", "m").unwrap();
        assert!(!dir.path().join("無い.yaml").exists());
        let after = Ledger::load_from(&path).unwrap();
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].model, "claude-opus-5-5", "宣言は残す");
        assert_eq!(
            after.entries[0].model_actual.as_deref(),
            Some("claude-opus-5")
        );
    }
}

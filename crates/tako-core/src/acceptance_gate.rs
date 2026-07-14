//! acceptance_gate — 受け入れゲートのデータモデル（Issue #244）
//!
//! worker タスクの受け入れ条件（述語）を構造化し、機械検証可能な判定結果を
//! 永続化する。設計の正は `.agent/orchestrator-design.md`「3. 受け入れゲートの遷移条件化」節。
//!
//! Store と YAML 永続化は tako-control::acceptance_gates にある。

use serde::{Deserialize, Serialize};

/// ゲート全体の判定結果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pending,
    Passed,
    Failed,
}

impl GateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for GateStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 個別の受け入れ条件の判定結果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionStatus {
    Pending,
    Passed,
    Failed,
}

impl CriterionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for CriterionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 受け入れ条件の種別
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CriterionKind {
    /// シェルコマンドの実行結果で判定
    Command {
        cmd: String,
        #[serde(default = "default_true")]
        expect_exit_0: bool,
    },
    /// GitHub PR のマージ状態で判定
    PrMerged {
        pr_number: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    /// 人間判断（手動で passed/failed を設定する）
    Custom { description: String },
}

fn default_true() -> bool {
    true
}

/// 1 つの受け入れ条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub kind: CriterionKind,
    #[serde(default = "default_pending")]
    pub status: CriterionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
}

fn default_pending() -> CriterionStatus {
    CriterionStatus::Pending
}

/// task_id に紐づく受け入れゲート
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceGate {
    pub task_id: String,
    pub criteria: Vec<AcceptanceCriterion>,
    pub overall: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl AcceptanceGate {
    /// criteria の status から overall を再計算する
    pub fn recompute_overall(&mut self) {
        if self.criteria.is_empty() {
            self.overall = GateStatus::Pending;
            return;
        }
        if self
            .criteria
            .iter()
            .all(|c| c.status == CriterionStatus::Passed)
        {
            self.overall = GateStatus::Passed;
        } else if self
            .criteria
            .iter()
            .any(|c| c.status == CriterionStatus::Failed)
        {
            self.overall = GateStatus::Failed;
        } else {
            self.overall = GateStatus::Pending;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompute_overall_all_passed() {
        let mut gate = AcceptanceGate {
            task_id: "t1".into(),
            criteria: vec![
                AcceptanceCriterion {
                    id: "a".into(),
                    kind: CriterionKind::Command {
                        cmd: "true".into(),
                        expect_exit_0: true,
                    },
                    status: CriterionStatus::Passed,
                    evidence: None,
                    checked_at: None,
                },
                AcceptanceCriterion {
                    id: "b".into(),
                    kind: CriterionKind::Custom {
                        description: "OK".into(),
                    },
                    status: CriterionStatus::Passed,
                    evidence: None,
                    checked_at: None,
                },
            ],
            overall: GateStatus::Pending,
            cwd: None,
        };
        gate.recompute_overall();
        assert_eq!(gate.overall, GateStatus::Passed);
    }

    #[test]
    fn recompute_overall_one_failed() {
        let mut gate = AcceptanceGate {
            task_id: "t1".into(),
            criteria: vec![
                AcceptanceCriterion {
                    id: "a".into(),
                    kind: CriterionKind::Command {
                        cmd: "true".into(),
                        expect_exit_0: true,
                    },
                    status: CriterionStatus::Passed,
                    evidence: None,
                    checked_at: None,
                },
                AcceptanceCriterion {
                    id: "b".into(),
                    kind: CriterionKind::Command {
                        cmd: "false".into(),
                        expect_exit_0: true,
                    },
                    status: CriterionStatus::Failed,
                    evidence: Some("exit 1".into()),
                    checked_at: None,
                },
            ],
            overall: GateStatus::Pending,
            cwd: None,
        };
        gate.recompute_overall();
        assert_eq!(gate.overall, GateStatus::Failed);
    }

    #[test]
    fn recompute_overall_mixed_pending() {
        let mut gate = AcceptanceGate {
            task_id: "t1".into(),
            criteria: vec![
                AcceptanceCriterion {
                    id: "a".into(),
                    kind: CriterionKind::Command {
                        cmd: "true".into(),
                        expect_exit_0: true,
                    },
                    status: CriterionStatus::Passed,
                    evidence: None,
                    checked_at: None,
                },
                AcceptanceCriterion {
                    id: "b".into(),
                    kind: CriterionKind::Custom {
                        description: "manual".into(),
                    },
                    status: CriterionStatus::Pending,
                    evidence: None,
                    checked_at: None,
                },
            ],
            overall: GateStatus::Pending,
            cwd: None,
        };
        gate.recompute_overall();
        assert_eq!(gate.overall, GateStatus::Pending);
    }

    #[test]
    fn recompute_overall_empty() {
        let mut gate = AcceptanceGate {
            task_id: "t1".into(),
            criteria: vec![],
            overall: GateStatus::Passed,
            cwd: None,
        };
        gate.recompute_overall();
        assert_eq!(gate.overall, GateStatus::Pending);
    }

    #[test]
    fn criterion_kind_serde_roundtrip() {
        let cmd = CriterionKind::Command {
            cmd: "cargo test".into(),
            expect_exit_0: true,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: CriterionKind = serde_json::from_str(&json).unwrap();
        if let CriterionKind::Command { cmd, expect_exit_0 } = parsed {
            assert_eq!(cmd, "cargo test");
            assert!(expect_exit_0);
        } else {
            panic!("wrong variant");
        }

        let pr = CriterionKind::PrMerged {
            pr_number: 247,
            repo: Some("takushio2525/tako".into()),
        };
        let json = serde_json::to_string(&pr).unwrap();
        let parsed: CriterionKind = serde_json::from_str(&json).unwrap();
        if let CriterionKind::PrMerged { pr_number, repo } = parsed {
            assert_eq!(pr_number, 247);
            assert_eq!(repo.as_deref(), Some("takushio2525/tako"));
        } else {
            panic!("wrong variant");
        }
    }
}

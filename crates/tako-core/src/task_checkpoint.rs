//! task_checkpoint — worker タスクのチェックポイントデータモデル（Issue #242）
//!
//! worker が「どの Issue の何フェーズ（実装中/検証中/PR済み）にいるか」を構造化して
//! 保持し、クラッシュ・利用上限・API 切断からの resume を可能にする。
//!
//! データモデルのみ。Store と YAML 永続化は tako-control::task_checkpoints にある。
//! 設計の正は `.agent/orchestrator-design.md`「1. チェックポイントと再開」節。

use serde::{Deserialize, Serialize};

/// タスクの進行フェーズ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Queued,
    Running,
    Verifying,
    Done,
    Failed,
    Suspended,
}

impl TaskPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "verifying" => Some(Self::Verifying),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

impl std::fmt::Display for TaskPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 1 つの worker タスクのチェックポイント
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub phase: TaskPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub updated_at: i64,
}

impl TaskCheckpoint {
    pub fn touch(&mut self) {
        self.updated_at = unix_now();
    }
}

/// 現在の Unix タイムスタンプ（秒）
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_roundtrip() {
        for phase in [
            TaskPhase::Queued,
            TaskPhase::Running,
            TaskPhase::Verifying,
            TaskPhase::Done,
            TaskPhase::Failed,
            TaskPhase::Suspended,
        ] {
            assert_eq!(TaskPhase::parse(phase.as_str()), Some(phase));
        }
        assert_eq!(TaskPhase::parse("unknown"), None);
    }

    #[test]
    fn is_terminal() {
        assert!(TaskPhase::Done.is_terminal());
        assert!(TaskPhase::Failed.is_terminal());
        assert!(!TaskPhase::Running.is_terminal());
        assert!(!TaskPhase::Suspended.is_terminal());
    }
}

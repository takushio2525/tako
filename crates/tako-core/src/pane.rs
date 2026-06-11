//! Pane — ペインのドメインモデル
//!
//! `PaneId` はプロセス生存期間中ユニークな整数 ID（`.agent/architecture.md`）。
//! Phase 2 以降、環境変数（`TAKO_PANE_ID`）や CLI / MCP の引数として外部に公開される。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// プロセス生存期間中ユニークなペイン ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u64);

impl PaneId {
    /// 新しいユニーク ID を採番する（プロセス全体で単調増加）
    fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        PaneId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ペインの生成主体。UI 表示とポリシー制御（FR-2.3.5）に使う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneOrigin {
    /// ユーザーの手動操作で生成
    User,
    /// Layer 1 CLI（`tako split` 等）で生成
    Cli,
    /// Layer 2 MCP ツールで生成
    Mcp,
    /// Layer 3 提案チップへの同意で生成
    Suggestion,
}

/// ペイン。Phase 1 はターミナルのみ。プレビュー種別は Phase 5 で拡張する
#[derive(Debug)]
pub struct Pane {
    id: PaneId,
    origin: PaneOrigin,
    /// 表示タイトル（FR-2.2.6 `tako title` で外部から設定可能になる）
    title: Option<String>,
    /// 役割ラベル（例: worker-1, dev-server。FR-2.1.3）
    role: Option<String>,
}

impl Pane {
    pub fn new(origin: PaneOrigin) -> Self {
        Self {
            id: PaneId::next(),
            origin,
            title: None,
            role: None,
        }
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    pub fn origin(&self) -> PaneOrigin {
        self.origin
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    pub fn set_role(&mut self, role: Option<String>) {
        self.role = role;
    }
}

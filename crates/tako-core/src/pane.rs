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

/// タイトルの出どころ（FR-2.12.3）。明示リネーム（CLI / MCP / UI）= Manual は
/// 自動リネーム（Auto）に上書きされない。Manual のクリアで Default に戻り自動が再開する
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleSource {
    /// 未設定（タブは初期連番のまま）
    #[default]
    Default,
    /// 自動リネーム（FR-2.12）が設定した
    Auto,
    /// 明示リネーム（`tako title` / `tako tab rename` / MCP / UI）で設定された
    Manual,
}

/// ペイン。Phase 1 はターミナルのみ。プレビュー種別は Phase 5 で拡張する
#[derive(Debug)]
pub struct Pane {
    id: PaneId,
    origin: PaneOrigin,
    /// 表示タイトル（FR-2.2.6 `tako title` で外部から設定可能になる）
    title: Option<String>,
    /// `title` の出どころ（FR-2.12.3 の手動優先判定）
    title_source: TitleSource,
    /// 役割ラベル（例: worker-1, dev-server。FR-2.1.3）
    role: Option<String>,
}

impl Pane {
    pub fn new(origin: PaneOrigin) -> Self {
        Self {
            id: PaneId::next(),
            origin,
            title: None,
            title_source: TitleSource::Default,
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

    pub fn title_source(&self) -> TitleSource {
        self.title_source
    }

    /// 明示リネーム（CLI / MCP / UI）。None（空文字クリア）で Default に戻り、
    /// 以後は自動リネーム（FR-2.12）が再び効くようになる
    pub fn set_title(&mut self, title: Option<String>) {
        self.title_source = if title.is_some() {
            TitleSource::Manual
        } else {
            TitleSource::Default
        };
        self.title = title;
    }

    /// 自動リネーム（FR-2.12）。Manual 設定済みなら上書きせず false を返す
    pub fn set_title_auto(&mut self, title: String) -> bool {
        if self.title_source == TitleSource::Manual {
            return false;
        }
        self.title = Some(title);
        self.title_source = TitleSource::Auto;
        true
    }

    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    pub fn set_role(&mut self, role: Option<String>) {
        self.role = role;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 手動タイトルは自動に上書きされない() {
        let mut pane = Pane::new(PaneOrigin::User);
        assert_eq!(pane.title_source(), TitleSource::Default);
        // 未設定 → 自動が効く
        assert!(pane.set_title_auto("ビルド".into()));
        assert_eq!(pane.title(), Some("ビルド"));
        assert_eq!(pane.title_source(), TitleSource::Auto);
        // 手動設定 → 自動は拒否される
        pane.set_title(Some("REVIEWER".into()));
        assert!(!pane.set_title_auto("別名".into()));
        assert_eq!(pane.title(), Some("REVIEWER"));
        assert_eq!(pane.title_source(), TitleSource::Manual);
        // クリアで Default に戻り自動が再開する
        pane.set_title(None);
        assert!(pane.set_title_auto("再開".into()));
        assert_eq!(pane.title(), Some("再開"));
    }
}

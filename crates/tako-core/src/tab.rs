//! Tab — エージェントグループの単位（1 グループ = 1 タブ、`.agent/concept.md`）

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pane::{Pane, TitleSource};
use crate::pane_tree::PaneTree;

/// プロセス生存期間中ユニークなタブ ID（`TAKO_TAB_ID` として外部公開される）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

static TAB_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl TabId {
    fn next() -> Self {
        TabId(TAB_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// 復元 ID の予約（Phase 5.5）。採番カウンタを ID の先へ進める（`PaneId` と同様）
    fn reserve(id: u64) {
        TAB_ID_COUNTER.fetch_max(id.saturating_add(1), Ordering::Relaxed);
    }

    /// 既知の ID から TabId を構築する（バックグラウンドペインの由来タブ復元用。FR-2.15.6）。
    /// 由来タブは既に閉じられていることがあり、その ID が後続の新規タブに再利用されると
    /// 別タブへ誤って紐付くため、採番カウンタを ID の先へ進めて再利用を防ぐ
    pub fn from_raw(id: u64) -> Self {
        TabId::reserve(id);
        TabId(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// タブ。常に 1 つ以上のペインを持つ PaneTree を内包する
#[derive(Debug)]
pub struct Tab {
    id: TabId,
    title: String,
    /// `title` の出どころ（FR-2.12.3。Default = 初期連番のまま）
    title_source: TitleSource,
    tree: PaneTree,
    /// AI が明示追加したフォルダ（#134。ファイルツリーの root に cwd と並んで表示される）
    pinned_folders: Vec<PathBuf>,
}

impl Tab {
    pub fn new(title: impl Into<String>, root_pane: Pane) -> Self {
        Self {
            id: TabId::next(),
            title: title.into(),
            title_source: TitleSource::Default,
            tree: PaneTree::new(root_pane),
            pinned_folders: Vec::new(),
        }
    }

    /// レイアウト復元用（Phase 5.5）。保存済み ID をそのまま再現する
    /// （`TAKO_TAB_ID` を再起動をまたいで有効に保つ）
    pub fn restore(
        id: u64,
        title: String,
        title_source: TitleSource,
        tree: PaneTree,
        pinned_folders: Vec<PathBuf>,
    ) -> Self {
        TabId::reserve(id);
        Self {
            id: TabId(id),
            title,
            title_source,
            tree,
            pinned_folders,
        }
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn title_source(&self) -> TitleSource {
        self.title_source
    }

    /// 初期タイトルの差し替え（出どころは変えない。UI の連番付け直し等）
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// 明示リネーム（`tako tab rename` / MCP / UI。FR-2.12.3 で自動より優先される）
    pub fn set_title_manual(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.title_source = TitleSource::Manual;
    }

    /// 手動リネームの解除。タイトルは保持しつつ Default に戻し、自動リネームを再開させる
    pub fn clear_manual_title(&mut self) {
        self.title_source = TitleSource::Default;
    }

    /// 自動リネーム（FR-2.12）。Manual 設定済みなら上書きせず false を返す
    pub fn set_title_auto(&mut self, title: impl Into<String>) -> bool {
        if self.title_source == TitleSource::Manual {
            return false;
        }
        self.title = title.into();
        self.title_source = TitleSource::Auto;
        true
    }

    pub fn tree(&self) -> &PaneTree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut PaneTree {
        &mut self.tree
    }

    /// タブを消費してペインツリーを取り出す（ペインの別タブ移送で使う）
    pub fn into_tree(self) -> PaneTree {
        self.tree
    }

    // --- pinned_folders（#134: AI からのフォルダ追加） ---

    pub fn pinned_folders(&self) -> &[PathBuf] {
        &self.pinned_folders
    }

    /// フォルダを追加する。正規パス（symlink 解決済み）でデデュープし、既存なら false
    pub fn add_pinned_folder(&mut self, path: PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if self
            .pinned_folders
            .iter()
            .any(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical)
        {
            return false;
        }
        self.pinned_folders.push(canonical);
        true
    }

    /// フォルダを削除する。正規パスで比較し、含まれていなければ false
    pub fn remove_pinned_folder(&mut self, path: &std::path::Path) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(pos) = self
            .pinned_folders
            .iter()
            .position(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical)
        {
            self.pinned_folders.remove(pos);
            true
        } else {
            false
        }
    }

    /// 実体が消えたフォルダエントリを除去する。変化があれば true
    pub fn prune_dead_folders(&mut self) -> bool {
        let before = self.pinned_folders.len();
        self.pinned_folders.retain(|p| p.is_dir());
        self.pinned_folders.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneOrigin;

    #[test]
    fn 手動タイトルは自動に上書きされずクリアで再開する() {
        let mut tab = Tab::new("1", Pane::new(PaneOrigin::User));
        assert_eq!(tab.title_source(), TitleSource::Default);
        assert!(tab.set_title_auto("ビルド"));
        assert_eq!(tab.title(), "ビルド");
        assert_eq!(tab.title_source(), TitleSource::Auto);
        tab.set_title_manual("agents");
        assert!(!tab.set_title_auto("別名"));
        assert_eq!(tab.title(), "agents");
        tab.clear_manual_title();
        assert!(tab.set_title_auto("再開"));
        assert_eq!(tab.title(), "再開");
    }

    // --- #171: pinned_folders の正規パスデデュープ ---

    #[test]
    fn pinned_folder_symlink経由の重複は畳まれる() {
        let mut tab = Tab::new("t", Pane::new(PaneOrigin::User));
        // macOS: /tmp → /private/tmp
        assert!(tab.add_pinned_folder(PathBuf::from("/tmp")));
        assert!(
            !tab.add_pinned_folder(PathBuf::from("/private/tmp")),
            "同じ正規パスの二重追加は false を返す"
        );
        assert_eq!(tab.pinned_folders().len(), 1);
    }

    #[test]
    fn pinned_folder_symlink経由でも削除できる() {
        let mut tab = Tab::new("t", Pane::new(PaneOrigin::User));
        tab.add_pinned_folder(PathBuf::from("/tmp"));
        assert!(
            tab.remove_pinned_folder(&PathBuf::from("/private/tmp")),
            "正規パスが同じなら別表記でも削除できる"
        );
        assert!(tab.pinned_folders().is_empty());
    }

    #[test]
    fn prune_dead_folders_は実体消失エントリを除去する() {
        let mut tab = Tab::new("t", Pane::new(PaneOrigin::User));
        let tmp = std::env::temp_dir().join("tako_test_prune_tab_171");
        std::fs::create_dir_all(&tmp).unwrap();
        tab.add_pinned_folder(tmp.clone());
        assert_eq!(tab.pinned_folders().len(), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
        assert!(tab.prune_dead_folders());
        assert!(tab.pinned_folders().is_empty());
    }
}

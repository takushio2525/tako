//! Pane — ペインのドメインモデル
//!
//! `PaneId` はプロセス生存期間中ユニークな整数 ID（`.agent/architecture.md`）。
//! Phase 2 以降、環境変数（`TAKO_PANE_ID`）や CLI / MCP の引数として外部に公開される。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// プロセス生存期間中ユニークなペイン ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u64);

static PANE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl PaneId {
    /// 新しいユニーク ID を採番する（プロセス全体で単調増加）
    pub(crate) fn next() -> Self {
        PaneId(PANE_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// 復元 ID の予約（Phase 5.5）。採番カウンタを ID の先へ進め、以後の新規採番と
    /// 衝突させない。再起動をまたいで `TAKO_PANE_ID` を有効に保つための土台
    fn reserve(id: u64) {
        PANE_ID_COUNTER.fetch_max(id.saturating_add(1), Ordering::Relaxed);
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// 既知の ID から PaneId を構築する（dispatch 層でワイヤ値を解決する用途）
    pub fn from_raw(id: u64) -> Self {
        PaneId(id)
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

impl PaneOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Suggestion => "suggestion",
        }
    }
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

impl TitleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
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
    /// spawn 元ペイン（オーケストレーター worker 用。セッション内で使い捨て、永続化不要）
    spawned_by: Option<PaneId>,
    /// 実行ペイン（run-interactive / Code Runner / カード）のメタデータ。
    /// セッション内で使い捨て（#1657 で終了コードの状態と再利用の鍵も持つ）
    interactive_meta: Option<crate::run_pane::InteractiveMeta>,
    /// 終了コードの側路ファイル（`crate::run_pane`）。実行ペイン（#1657）と
    /// `tako split --command` の失敗時の保持（#1778）が持つ。**実行ペインのメタとは別**に
    /// 置くのは、保持のペインを実行ペイン（バッジ・`list` の `run`・auto_close）に
    /// しないため。`None` = 用意できなかった / 持たない = 画面のマーカーだけが頼り。
    /// セッション内で使い捨て（layout.json には保存しない）
    exit_file: Option<std::path::PathBuf>,
    /// 利用上限（5h / 週次）後の自動復帰を有効にするか（#813）。既定 OFF。
    /// ペイン単位のオプトインで、layout.json に保存して再起動・復元をまたいで維持する
    limit_autoresume: bool,
    /// 自動復帰の値が「決まった」ペインか（#1945。セッション内で使い捨て・layout.json には
    /// 保存しない）。人の切り替え・一括・プロファイル・spawn のどれかで値が入ったら立つ。
    /// 立っていないペインだけが、エージェントになった瞬間に全体の既定を採る
    /// （[`Self::adopt_limit_resume_default`]）。**個別に切ったペインを後から全体の既定で
    /// 上書きしない**ための印
    limit_autoresume_decided: bool,
    /// 上限の告知を最初に観測した時刻の記憶（#1710）。自動復帰（#813）の追跡はメモリにしか
    /// 無いので、GUI の再起動・追跡の作り直しをまたいで「告知がいつ出たか」をここへ残し、
    /// リセット時刻をその時刻から解く。layout.json に保存する（告知は本文ではなく要約だけ）。
    /// 箱に入れるのは `PaneNode::Leaf` を太らせないため（clippy の large_enum_variant）
    limit_seen: Option<Box<crate::limit_resume::LimitSeen>>,
    /// このペインから出る `tako queue` の処理の優先度の既定（#1996。`None` = 役割の既定）。
    /// `tako queue priority --pane N <段>` で付け、layout.json に保存して GUI の再起動をまたいで残す
    queue_priority: Option<crate::queue::Priority>,
}

impl Pane {
    pub fn new(origin: PaneOrigin) -> Self {
        let id = PaneId::next();
        // #2731: ペインができたことを外の購読者へ（role は後から `set_role` が積む）
        crate::events::emit(crate::events::Detail::PaneAdded {
            pane: id.as_u64(),
            origin: origin.as_str(),
            restored: false,
            role: None,
        });
        Self {
            id,
            origin,
            title: None,
            title_source: TitleSource::Default,
            role: None,
            spawned_by: None,
            interactive_meta: None,
            exit_file: None,
            limit_autoresume: false,
            limit_autoresume_decided: false,
            limit_seen: None,
            queue_priority: None,
        }
    }

    /// レイアウト復元用（Phase 5.5）。保存済み ID をそのまま再現する。
    /// 環境変数 `TAKO_PANE_ID` を再起動をまたいで有効に保つため、ID は変えない
    pub fn restore(
        id: u64,
        origin: PaneOrigin,
        title: Option<String>,
        title_source: TitleSource,
        role: Option<String>,
        limit_autoresume: bool,
    ) -> Self {
        PaneId::reserve(id);
        // #2731: 復元したペインも「できた」として積む（GUI の起動ごとに epoch が変わるので、
        // 受け手は `since: 0` で今あるペインを知れる）
        crate::events::emit(crate::events::Detail::PaneAdded {
            pane: id,
            origin: origin.as_str(),
            restored: true,
            role: role.clone(),
        });
        Self {
            id: PaneId(id),
            origin,
            title,
            title_source,
            role,
            spawned_by: None,
            interactive_meta: None,
            exit_file: None,
            limit_autoresume,
            // 復元したペインの「決定済み」は呼び出し側（復元の後にエージェントかを
            // 知っている GUI）が [`Self::mark_limit_resume_decided`] で立てる
            limit_autoresume_decided: false,
            // 告知の記憶（#1710）は呼び出し側が [`Self::set_limit_seen`] で戻す
            limit_seen: None,
            // 順番待ちの優先度の既定（#1996）は呼び出し側が [`Self::set_queue_priority`] で戻す
            queue_priority: None,
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

    /// `tako queue` の処理の優先度の既定（#1996。`None` = 役割の既定）
    pub fn queue_priority(&self) -> Option<crate::queue::Priority> {
        self.queue_priority
    }

    pub fn set_queue_priority(&mut self, priority: Option<crate::queue::Priority>) {
        self.queue_priority = priority;
    }

    /// 役割ラベルを書き換える。値が変わったら外の購読者へ `role_changed` を積む（#2731。
    /// 比べるのは前後の値だけ = 読み取りを足さない）
    pub fn set_role(&mut self, role: Option<String>) {
        if self.role != role {
            crate::events::emit(crate::events::Detail::RoleChanged {
                pane: self.id.as_u64(),
                from: self.role.clone(),
                to: role.clone(),
            });
        }
        self.role = role;
    }

    pub fn spawned_by(&self) -> Option<PaneId> {
        self.spawned_by
    }

    pub fn set_spawned_by(&mut self, parent: Option<PaneId>) {
        self.spawned_by = parent;
    }

    /// 実行ペインのメタデータ（実行ペインでなければ `None`）
    pub fn interactive_meta(&self) -> Option<&crate::run_pane::InteractiveMeta> {
        self.interactive_meta.as_ref()
    }

    pub fn interactive_meta_mut(&mut self) -> Option<&mut crate::run_pane::InteractiveMeta> {
        self.interactive_meta.as_mut()
    }

    pub fn set_interactive_meta(&mut self, meta: crate::run_pane::InteractiveMeta) {
        self.interactive_meta = Some(meta);
    }

    /// 終了コードの側路ファイル（持たなければ `None`。#1657 / #1778）
    pub fn exit_file(&self) -> Option<&std::path::Path> {
        self.exit_file.as_deref()
    }

    pub fn set_exit_file(&mut self, file: Option<std::path::PathBuf>) {
        self.exit_file = file;
    }

    /// 利用上限後の自動復帰が有効か（#813。既定 false）
    pub fn limit_autoresume(&self) -> bool {
        self.limit_autoresume
    }

    /// 自動復帰のオプトインを設定する（右クリック / CLI / MCP の 3 経路が同じここを通る）。
    /// 値が決まったので、以後は全体の既定（#1945）に上書きされない
    pub fn set_limit_autoresume(&mut self, enabled: bool) {
        self.limit_autoresume = enabled;
        self.limit_autoresume_decided = true;
    }

    /// 自動復帰の値が決まっているか（#1945）
    pub fn limit_resume_decided(&self) -> bool {
        self.limit_autoresume_decided
    }

    /// 値は変えずに「決まった」とだけ印を付ける（#1945）。
    /// 復元したエージェントのペイン（保存時の値が人の選択）と、プロファイルが
    /// 明示 OFF を言っているペインに使う
    pub fn mark_limit_resume_decided(&mut self) {
        self.limit_autoresume_decided = true;
    }

    /// エージェントになったペインへ全体の既定（#1945）を当てる。値を変えたら true。
    ///
    /// **ON にする方向だけ**: 既定が OFF なら何もしない（印も立てない = 後から全体を
    /// ON にしたときに、まだ決まっていないペインとして ON を採れる）。決まったペインは
    /// 触らない（個別に OFF にしたペインを全体の ON で戻さない）
    pub fn adopt_limit_resume_default(&mut self, default: bool) -> bool {
        if self.limit_autoresume_decided || !default {
            return false;
        }
        self.limit_autoresume_decided = true;
        if self.limit_autoresume {
            return false;
        }
        self.limit_autoresume = true;
        true
    }

    /// 上限の告知を最初に観測した時刻の記憶（#1710。無ければ `None`）
    pub fn limit_seen(&self) -> Option<&crate::limit_resume::LimitSeen> {
        self.limit_seen.as_deref()
    }

    /// 告知の記憶を差し替える（#1710。GUI の自動復帰の tick と layout の復元が通る）
    pub fn set_limit_seen(&mut self, seen: Option<crate::limit_resume::LimitSeen>) {
        self.limit_seen = seen.map(Box::new);
    }
}

/// #2731: ペインが消えたことを外の購読者へ。`Pane` は複製できない（`Clone` を持たない）ので、
/// 破棄 = そのペインがプロセスから消えた時点。タブ間の移動・バックグラウンドへの退避は値の移動で
/// 破棄されないので積まない。閉じる 3 経路（GUI の × / タブ close / CLI・MCP の close）と、
/// 実行ペインの差し替え（#1657）・復旧での作り直しのどれもがここを通る
impl Drop for Pane {
    fn drop(&mut self) {
        crate::events::emit(crate::events::Detail::PaneClosed {
            pane: self.id.as_u64(),
            role: self.role.take(),
        });
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

    /// #2731: 生成・role の変化・破棄が外の購読者へ積まれる。同じ値の書き直しは積まない
    #[test]
    fn 生成とroleの変化と破棄を出来事へ積む() {
        use crate::events::Detail;
        let captured = crate::events::capture_on_this_thread();
        let mut pane = Pane::new(PaneOrigin::Mcp);
        let id = pane.id().as_u64();
        pane.set_role(Some("orchestrator-master".into()));
        pane.set_role(Some("orchestrator-master".into()));
        pane.set_role(Some("orchestrator-master:dev".into()));
        drop(pane);
        assert_eq!(
            captured.take(),
            [
                Detail::PaneAdded {
                    pane: id,
                    origin: "mcp",
                    restored: false,
                    role: None,
                },
                Detail::RoleChanged {
                    pane: id,
                    from: None,
                    to: Some("orchestrator-master".into()),
                },
                Detail::RoleChanged {
                    pane: id,
                    from: Some("orchestrator-master".into()),
                    to: Some("orchestrator-master:dev".into()),
                },
                Detail::PaneClosed {
                    pane: id,
                    role: Some("orchestrator-master:dev".into()),
                },
            ]
        );
    }

    /// #2731: 復元は持ち越した role ごと「できた」として積む
    #[test]
    fn 復元したペインもroleつきで積む() {
        use crate::events::Detail;
        let captured = crate::events::capture_on_this_thread();
        let pane = Pane::restore(
            PaneId::next().as_u64(),
            PaneOrigin::Cli,
            None,
            TitleSource::Default,
            Some("dev-server".into()),
            false,
        );
        let id = pane.id().as_u64();
        drop(pane);
        assert_eq!(
            captured.take(),
            [
                Detail::PaneAdded {
                    pane: id,
                    origin: "cli",
                    restored: true,
                    role: Some("dev-server".into()),
                },
                Detail::PaneClosed {
                    pane: id,
                    role: Some("dev-server".into()),
                },
            ]
        );
    }
}

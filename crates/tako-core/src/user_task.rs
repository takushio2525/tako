//! user_task — **人がやること**のデータモデル（Issue #1450 の分割 B1）
//!
//! master / worker が「ユーザーに見てもらう・聞く・許可をもらう」を溜める先。
//! 引き継ぎファイルや会話に散っていた「ユーザー確認待ち」を 1 か所へ寄せ、
//! PC の画面（B2）・スマホ（B3）・CLI / MCP のどこからでも同じものが読める。
//!
//! ## 既存の `tako task` とは別物
//!
//! `task_checkpoint`（#242）と `acceptance_gate`（#244）は **AI（worker）のタスク**で、
//! 進行フェーズと受け入れ述語を持つ。こちらは**人のタスク**で、決めるのも片付けるのも
//! ユーザー。名前空間を分けるため id の綴りも変えてある（AI は `task-N` / 人は `u-N`）。
//!
//! データモデルと純粋な操作だけを置く。永続（YAML）・配送・JSON 化は
//! `tako-control::user_tasks` にある（`task_checkpoint` と同じ分け方）。

use serde::{Deserialize, Serialize};

pub use crate::task_checkpoint::unix_now;

/// タスクの種類（#1450 の要件追加で review / confirm / permission へ広げた）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// 生成物のレビュー依頼（対象は `attachments` / `links` に置く）
    Review,
    /// 確認（「この方針でいいか」）
    Confirm,
    /// 権限・許可の確認
    Permission,
    /// 投稿（X / YouTube。投稿文は `copy_texts`、素材は `attachments`）
    Post,
    /// 上のどれでもないもの
    Other,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Confirm => "confirm",
            Self::Permission => "permission",
            Self::Post => "post",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|k| k.as_str() == s)
    }

    pub fn all() -> &'static [TaskKind] {
        &[
            Self::Review,
            Self::Confirm,
            Self::Permission,
            Self::Post,
            Self::Other,
        ]
    }
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// タスクの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 未完了（画面のバッジに数えるのはこれ）
    Open,
    /// 片付いた
    Done,
    /// やらないことにした
    Dismissed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|s2| s2.as_str() == s)
    }

    pub fn all() -> &'static [TaskStatus] {
        &[Self::Open, Self::Done, Self::Dismissed]
    }

    pub fn is_open(self) -> bool {
        self == Self::Open
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ユーザーの返答（#1450 の要件追加）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// 承認した
    Approve,
    /// 却下した
    Reject,
    /// 直してほしい（作業は続く）
    NeedsChange,
    /// 質問へ答えた（作業は続く）
    Answered,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::NeedsChange => "needs_change",
            Self::Answered => "answered",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|d| d.as_str() == s)
    }

    pub fn all() -> &'static [Decision] {
        &[
            Self::Approve,
            Self::Reject,
            Self::NeedsChange,
            Self::Answered,
        ]
    }

    /// この返答でタスクが片付くか。
    ///
    /// approve / reject は**ユーザーの側の仕事が終わっている**ので閉じる。
    /// needs_change / answered は AI 側の作業がこれから続くので open のまま置く
    /// （閉じてしまうと、直った物を見てもらう先が消える）
    pub fn settles(self) -> Option<TaskStatus> {
        match self {
            Self::Approve => Some(TaskStatus::Done),
            Self::Reject => Some(TaskStatus::Dismissed),
            Self::NeedsChange | Self::Answered => None,
        }
    }
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 返答がどこから入ったか
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// PC の右パネル（B2）
    Pc,
    /// スマホの PWA（B3）
    Pwa,
    /// `tako todo respond`
    Cli,
    /// MCP（AI が代理で入れた返答。人の返答と区別できるようにする）
    Mcp,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pc => "pc",
            Self::Pwa => "pwa",
            Self::Cli => "cli",
            Self::Mcp => "mcp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|v| v.as_str() == s)
    }

    pub fn all() -> &'static [Via] {
        &[Self::Pc, Self::Pwa, Self::Cli, Self::Mcp]
    }
}

/// 配送の顛末（#1450 の要件追加 ③）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// 生きている master のペインへ積んだ（決着はまだ）
    Sent,
    /// master が居なかったので起動して初回メッセージへ載せた（決着はまだ）
    Launched,
    /// 送達を確認した
    Delivered,
    /// 届かなかった（`reason` に分類が入る）
    Failed,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Launched => "launched",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|d| d.as_str() == s)
    }

    pub fn all() -> &'static [DeliveryState] {
        &[Self::Sent, Self::Launched, Self::Delivered, Self::Failed]
    }

    /// まだ決着していない（`prompt_delivery_state` で畳み込む対象）
    pub fn is_pending(self) -> bool {
        matches!(self, Self::Sent | Self::Launched)
    }
}

impl std::fmt::Display for DeliveryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ラベル付きのコピー用テキスト（投稿文 / タイトル / 説明 / ハッシュタグ）。
/// スマホからワンタップでコピーするので、1 本の本文へ混ぜずに分けて持つ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyText {
    pub label: String,
    pub text: String,
}

/// 起票元（返答をどこへ返すか）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOrigin {
    /// master のプロファイル名（`default` を含む）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// 起票した会話の session_id（同じ会話へ返すための手がかり）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 起票したペイン（生きていればここへ返す）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    /// プロジェクトキー
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl TaskOrigin {
    /// 空（何も分からない）か
    pub fn is_empty(&self) -> bool {
        self.profile.is_none()
            && self.session_id.is_none()
            && self.pane.is_none()
            && self.project.is_none()
    }
}

/// ユーザーの返答 1 件（スレッドなので複数積む）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResponse {
    pub decision: Decision,
    /// 自由記述（空でもよい）
    #[serde(default)]
    pub comment: String,
    pub at: i64,
    pub via: Via,
}

/// 配送 1 回の記録
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub state: DeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    /// 失敗・打ち切りの分類（本文は入れない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: i64,
    /// 何番目の返答を運んだか（`responses` の index）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_index: Option<usize>,
}

/// 1 件のユーザータスク
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTask {
    /// `u-N`（AI 側の `task-N` と目で区別できる綴りにしてある）
    pub id: String,
    pub title: String,
    /// markdown 本文
    #[serde(default)]
    pub body: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// 起票者の名乗り（`master:takodev` / `worker:pane-12` / `cli`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// 添付（絶対パス）。**起票時に実在は問わない**（生成前に起票する運用があるため）
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub copy_texts: Vec<CopyText>,
    #[serde(default)]
    pub links: Vec<String>,
    /// 期限（`YYYY-MM-DD`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<TaskOrigin>,
    #[serde(default)]
    pub responses: Vec<TaskResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

impl UserTask {
    pub fn touch(&mut self) {
        self.updated_at = unix_now();
    }

    /// 最後の返答（画面が先頭に出すもの）
    pub fn last_response(&self) -> Option<&TaskResponse> {
        self.responses.last()
    }
}

/// 新規起票の入力（`add` の引数をまとめたもの。引数が 10 個を超えるので構造体で渡す）
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    pub body: String,
    pub kind: Option<TaskKind>,
    pub created_by: Option<String>,
    pub project: Option<String>,
    pub attachments: Vec<String>,
    pub copy_texts: Vec<CopyText>,
    pub links: Vec<String>,
    pub due: Option<String>,
    pub origin: Option<TaskOrigin>,
}

/// 部分更新の入力（`None` は「触らない」、`Some` は「置き換える」）
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub kind: Option<TaskKind>,
    pub project: Option<String>,
    pub attachments: Option<Vec<String>>,
    pub copy_texts: Option<Vec<CopyText>>,
    pub links: Option<Vec<String>>,
    pub due: Option<String>,
}

impl TaskPatch {
    /// 1 つも指定が無い（`update` を空打ちした）
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.kind.is_none()
            && self.project.is_none()
            && self.attachments.is_none()
            && self.copy_texts.is_none()
            && self.links.is_none()
            && self.due.is_none()
    }
}

/// 一覧の絞り込み
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// `None` は**未完了のみ**（画面の既定）。全件は `Some(vec![])` ではなく [`TaskFilter::all`]
    pub status: Option<TaskStatus>,
    pub kind: Option<TaskKind>,
    pub project: Option<String>,
    /// status での絞り込みを外す（`--all`）
    pub any_status: bool,
}

impl TaskFilter {
    /// 全件（status で絞らない）
    pub fn all() -> Self {
        Self {
            any_status: true,
            ..Self::default()
        }
    }

    fn matches(&self, task: &UserTask) -> bool {
        if let Some(kind) = self.kind {
            if task.kind != kind {
                return false;
            }
        }
        if let Some(project) = &self.project {
            if task.project.as_deref() != Some(project.as_str()) {
                return false;
            }
        }
        match (self.any_status, self.status) {
            // 明示された status が最優先（`--all --status done` は done だけ）
            (_, Some(status)) => task.status == status,
            (true, None) => true,
            (false, None) => task.status.is_open(),
        }
    }
}

/// タスクの入れ物（**純データ操作のみ**。永続は tako-control::user_tasks）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub tasks: Vec<UserTask>,
}

fn default_version() -> u32 {
    1
}

impl Default for TaskStore {
    fn default() -> Self {
        Self {
            version: default_version(),
            tasks: Vec::new(),
        }
    }
}

/// id の前置き。AI 側（`task-N`）と取り違えないための綴り
pub const ID_PREFIX: &str = "u-";

impl TaskStore {
    pub fn find(&self, id: &str) -> Option<&UserTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut UserTask> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// 次の id（`u-N`）。**同時 add でも重複しない**のは、呼び出し側が
    /// ファイルロックの中で採番するため（`task_checkpoints::next_task_id` と同じ作法）
    pub fn next_id(&self) -> String {
        let max_n = self
            .tasks
            .iter()
            .filter_map(|t| t.id.strip_prefix(ID_PREFIX))
            .filter_map(|s| s.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{ID_PREFIX}{}", max_n + 1)
    }

    /// 起票する。**空タイトルは拒否**（一覧で選べないものを作らせない）
    pub fn add(&mut self, new: NewTask) -> Result<&UserTask, String> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err("タイトルが空（一覧で選べないタスクは作れない）".into());
        }
        let now = unix_now();
        let task = UserTask {
            id: self.next_id(),
            title,
            body: new.body,
            kind: new.kind.unwrap_or(TaskKind::Other),
            status: TaskStatus::Open,
            created_by: new.created_by,
            project: new.project,
            attachments: new.attachments,
            copy_texts: new.copy_texts,
            links: new.links,
            due: new.due,
            created_at: now,
            updated_at: now,
            origin: new.origin.filter(|o| !o.is_empty()),
            responses: Vec::new(),
            delivery: None,
        };
        self.tasks.push(task);
        Ok(self.tasks.last().expect("直前に push した"))
    }

    /// 部分更新。触っていないフィールドは保つ
    pub fn update(&mut self, id: &str, patch: TaskPatch) -> Result<&UserTask, String> {
        if patch.is_empty() {
            return Err("更新する項目が 1 つも指定されていない".into());
        }
        if let Some(title) = &patch.title {
            if title.trim().is_empty() {
                return Err("タイトルを空にはできない".into());
            }
        }
        let task = self
            .find_mut(id)
            .ok_or_else(|| format!("タスクが見つからない: {id}"))?;
        if let Some(title) = patch.title {
            task.title = title.trim().to_string();
        }
        if let Some(body) = patch.body {
            task.body = body;
        }
        if let Some(kind) = patch.kind {
            task.kind = kind;
        }
        if let Some(project) = patch.project {
            task.project = Some(project);
        }
        if let Some(attachments) = patch.attachments {
            task.attachments = attachments;
        }
        if let Some(copy_texts) = patch.copy_texts {
            task.copy_texts = copy_texts;
        }
        if let Some(links) = patch.links {
            task.links = links;
        }
        if let Some(due) = patch.due {
            task.due = Some(due);
        }
        task.touch();
        Ok(task)
    }

    /// 状態を変える（`done` / `dismiss`）
    pub fn set_status(&mut self, id: &str, status: TaskStatus) -> Result<&UserTask, String> {
        let task = self
            .find_mut(id)
            .ok_or_else(|| format!("タスクが見つからない: {id}"))?;
        task.status = status;
        task.touch();
        Ok(task)
    }

    /// 返答を積む。返るのは積んだ返答の index（配送の記録が指す番号）
    pub fn respond(
        &mut self,
        id: &str,
        decision: Decision,
        comment: &str,
        via: Via,
    ) -> Result<usize, String> {
        let task = self
            .find_mut(id)
            .ok_or_else(|| format!("タスクが見つからない: {id}"))?;
        task.responses.push(TaskResponse {
            decision,
            comment: comment.trim().to_string(),
            at: unix_now(),
            via,
        });
        if let Some(status) = decision.settles() {
            task.status = status;
        }
        task.touch();
        Ok(task.responses.len() - 1)
    }

    /// 配送の記録を置く（1 タスクにつき最後の 1 回だけ持つ）
    pub fn set_delivery(&mut self, id: &str, delivery: Delivery) -> Result<(), String> {
        let task = self
            .find_mut(id)
            .ok_or_else(|| format!("タスクが見つからない: {id}"))?;
        task.delivery = Some(delivery);
        task.touch();
        Ok(())
    }

    /// 一覧（**新しい順**。画面もこの順で出す）
    pub fn list(&self, filter: &TaskFilter) -> Vec<&UserTask> {
        let mut items: Vec<&UserTask> = self.tasks.iter().filter(|t| filter.matches(t)).collect();
        items.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        items
    }

    /// 未完了の件数（B2 / B3 のバッジ）
    pub fn open_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status.is_open()).count()
    }

    /// 未完了の件数を種類ごとに（画面の内訳）
    pub fn open_counts_by_kind(&self) -> Vec<(TaskKind, usize)> {
        TaskKind::all()
            .iter()
            .map(|kind| {
                let n = self
                    .tasks
                    .iter()
                    .filter(|t| t.status.is_open() && t.kind == *kind)
                    .count();
                (*kind, n)
            })
            .collect()
    }

    /// 決着していない配送を持つタスクの id（畳み込みの対象）
    pub fn pending_deliveries(&self) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|t| t.delivery.as_ref().is_some_and(|d| d.state.is_pending()))
            .map(|t| t.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(n: usize) -> TaskStore {
        let mut store = TaskStore::default();
        for i in 0..n {
            store
                .add(NewTask {
                    title: format!("タスク {i}"),
                    ..NewTask::default()
                })
                .expect("起票できる");
        }
        store
    }

    #[test]
    fn 語彙は往復する() {
        for k in TaskKind::all() {
            assert_eq!(TaskKind::parse(k.as_str()), Some(*k));
        }
        for s in TaskStatus::all() {
            assert_eq!(TaskStatus::parse(s.as_str()), Some(*s));
        }
        for d in Decision::all() {
            assert_eq!(Decision::parse(d.as_str()), Some(*d));
        }
        for v in Via::all() {
            assert_eq!(Via::parse(v.as_str()), Some(*v));
        }
        for d in DeliveryState::all() {
            assert_eq!(DeliveryState::parse(d.as_str()), Some(*d));
        }
        assert_eq!(TaskKind::parse("なにか"), None);
    }

    /// id は `u-N` で採番され、**AI 側の `task-N` と混ざらない**
    #[test]
    fn idはuで採番される() {
        let store = store_with(3);
        assert_eq!(
            store
                .tasks
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["u-1", "u-2", "u-3"]
        );
        assert_eq!(store.next_id(), "u-4");
    }

    /// 欠番があっても最大値の次を採る（削除は無いが、手で編集されたファイルを読んだとき）
    #[test]
    fn 採番は最大値の次() {
        let mut store = store_with(1);
        store.tasks[0].id = "u-9".into();
        assert_eq!(store.next_id(), "u-10");
    }

    #[test]
    fn 空タイトルは拒否する() {
        let mut store = TaskStore::default();
        let err = store
            .add(NewTask {
                title: "   ".into(),
                ..NewTask::default()
            })
            .unwrap_err();
        assert!(err.contains("タイトルが空"), "{err}");
        assert!(store.tasks.is_empty(), "拒否したのに積まれている");
    }

    #[test]
    fn 部分更新は触っていない項目を保つ() {
        let mut store = TaskStore::default();
        store
            .add(NewTask {
                title: "元のタイトル".into(),
                body: "元の本文".into(),
                kind: Some(TaskKind::Post),
                links: vec!["https://example.test/a".into()],
                ..NewTask::default()
            })
            .unwrap();
        store
            .update(
                "u-1",
                TaskPatch {
                    title: Some("新しいタイトル".into()),
                    ..TaskPatch::default()
                },
            )
            .unwrap();
        let task = store.find("u-1").unwrap();
        assert_eq!(task.title, "新しいタイトル");
        assert_eq!(task.body, "元の本文", "触っていない本文が消えた");
        assert_eq!(task.kind, TaskKind::Post);
        assert_eq!(task.links.len(), 1);
        assert!(store.update("u-1", TaskPatch::default()).is_err());
    }

    /// approve / reject は閉じ、needs_change / answered は open のまま
    #[test]
    fn 返答の状態遷移() {
        for (decision, expected) in [
            (Decision::Approve, TaskStatus::Done),
            (Decision::Reject, TaskStatus::Dismissed),
            (Decision::NeedsChange, TaskStatus::Open),
            (Decision::Answered, TaskStatus::Open),
        ] {
            let mut store = store_with(1);
            store
                .respond("u-1", decision, "コメント", Via::Pwa)
                .unwrap();
            assert_eq!(
                store.find("u-1").unwrap().status,
                expected,
                "{decision} の遷移が違う"
            );
        }
    }

    #[test]
    fn 返答はスレッドとして積む() {
        let mut store = store_with(1);
        assert_eq!(
            store
                .respond("u-1", Decision::NeedsChange, " 直して ", Via::Pwa)
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .respond("u-1", Decision::Approve, "", Via::Pc)
                .unwrap(),
            1
        );
        let task = store.find("u-1").unwrap();
        assert_eq!(task.responses.len(), 2);
        assert_eq!(
            task.responses[0].comment, "直して",
            "前後の空白が残っている"
        );
        assert_eq!(task.last_response().unwrap().decision, Decision::Approve);
        assert!(store
            .respond("u-404", Decision::Approve, "", Via::Cli)
            .is_err());
    }

    /// 既定の一覧は未完了のみ。`--all` で全件、`--status` は最優先
    #[test]
    fn 一覧の絞り込み() {
        let mut store = store_with(3);
        store.set_status("u-2", TaskStatus::Done).unwrap();
        assert_eq!(store.list(&TaskFilter::default()).len(), 2);
        assert_eq!(store.list(&TaskFilter::all()).len(), 3);
        let done_only = TaskFilter {
            status: Some(TaskStatus::Done),
            ..TaskFilter::all()
        };
        assert_eq!(store.list(&done_only).len(), 1);
        assert_eq!(store.open_count(), 2);
    }

    #[test]
    fn 一覧は更新の新しい順() {
        let mut store = store_with(3);
        // updated_at が同じ秒に並ぶので、明示的にずらす（時間を測る検査ではない）
        store.tasks[0].updated_at = 100;
        store.tasks[1].updated_at = 300;
        store.tasks[2].updated_at = 200;
        let ids: Vec<&str> = store
            .list(&TaskFilter::all())
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["u-2", "u-3", "u-1"]);
    }

    #[test]
    fn 種類別の未完了件数() {
        let mut store = TaskStore::default();
        for kind in [TaskKind::Post, TaskKind::Post, TaskKind::Review] {
            store
                .add(NewTask {
                    title: "t".into(),
                    kind: Some(kind),
                    ..NewTask::default()
                })
                .unwrap();
        }
        store.set_status("u-1", TaskStatus::Dismissed).unwrap();
        let counts = store.open_counts_by_kind();
        assert_eq!(
            counts
                .iter()
                .find(|(k, _)| *k == TaskKind::Post)
                .map(|(_, n)| *n),
            Some(1)
        );
        assert_eq!(
            counts
                .iter()
                .find(|(k, _)| *k == TaskKind::Review)
                .map(|(_, n)| *n),
            Some(1)
        );
        assert_eq!(store.open_count(), 2);
    }

    #[test]
    fn 決着していない配送だけを畳み込みの対象にする() {
        let mut store = store_with(3);
        for (id, state) in [
            ("u-1", DeliveryState::Sent),
            ("u-2", DeliveryState::Delivered),
            ("u-3", DeliveryState::Launched),
        ] {
            store
                .set_delivery(
                    id,
                    Delivery {
                        state,
                        profile: None,
                        pane: Some(7),
                        tab: None,
                        reason: None,
                        at: unix_now(),
                        response_index: Some(0),
                    },
                )
                .unwrap();
        }
        assert_eq!(store.pending_deliveries(), vec!["u-1", "u-3"]);
    }

    /// 空の `origin` は持たない（YAML に空の節を作らない）
    #[test]
    fn 空のoriginは落ちる() {
        let mut store = TaskStore::default();
        store
            .add(NewTask {
                title: "t".into(),
                origin: Some(TaskOrigin::default()),
                ..NewTask::default()
            })
            .unwrap();
        assert!(store.find("u-1").unwrap().origin.is_none());
    }

    /// 省略された項目が既定で埋まる（`#[serde(default)]` の配線）。
    /// **形式によらない serde の振る舞い**を見るので、ここは依存の少ない JSON で確かめる
    /// （YAML ファイルとして読めることは tako-control::user_tasks 側で見る）
    #[test]
    fn 省略された項目は既定で埋まる() {
        let store: TaskStore = serde_json::from_str(r#"{"version":1,"tasks":[]}"#).unwrap();
        assert!(store.tasks.is_empty());
        let store: TaskStore = serde_json::from_str(
            r#"{"tasks":[{"id":"u-1","title":"t","kind":"post","status":"open",
                 "created_at":1,"updated_at":1}]}"#,
        )
        .unwrap();
        assert_eq!(store.version, 1, "version が無いファイルは v1 として読む");
        assert_eq!(store.tasks[0].kind, TaskKind::Post);
        assert!(store.tasks[0].responses.is_empty());
        assert!(store.tasks[0].body.is_empty());
    }
}

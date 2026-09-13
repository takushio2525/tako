//! user_tasks — [`tako_core::user_task`] の永続（YAML）と JSON 化、返答の配送（Issue #1450）
//!
//! `task_checkpoints` と同じ分け方: **モデルと純粋な操作は tako-core**、ここは
//! `<data_dir>/orchestrator/user-tasks.yaml` への読み書きと、CLI / MCP / 画面が
//! 共有する応答 JSON、そして「返答を起票 master へ運ぶ」段取りの組み立てを持つ。
//!
//! ## 配送は既存の経路を呼ぶだけ
//!
//! 返答の配送（#1450 の要件追加 ③）は **新しい送達経路を作らない**。
//! ここが決めるのは「どのペインへ」「何という本文を」までで、実際に送るのは
//! `Request::Send`（= `tako_send_input` と同じ腕）と
//! `orchestrator::master_launch::plan`（= `tako master` / PWA の「+ master」と同じ組み立て）。
//! ペインへ触る部分は host が要るので [`crate::dispatch`] 側に置き、
//! ここには**ペインを選ぶ判断**と**本文の組み立て**という純粋な部分を置く
//! （番犬が注入で落とせるのはこの形にしたため）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_core::user_task::{
    unix_now, CopyText, Decision, Delivery, DeliveryState, NewTask, TaskFilter, TaskKind,
    TaskOrigin, TaskPatch, TaskStatus, TaskStore, UserTask, Via,
};

/// ユーザータスクの置き場。`TAKO_USER_TASKS_FILE` で上書きできる（隔離検証用。
/// `task_checkpoints` の `TAKO_TASK_CHECKPOINTS_FILE` と同じ作法）
pub fn store_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_USER_TASKS_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    crate::orchestrator::config_dir().map(|d| d.join("user-tasks.yaml"))
}

/// 不在は空、パース失敗は Err（0 件へ丸めない。#169）
pub fn load_from(path: &Path) -> Result<TaskStore, String> {
    if !path.is_file() {
        return Ok(TaskStore::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("user-tasks.yaml の読み取りに失敗: {e}"))?;
    if content.trim().is_empty() {
        return Ok(TaskStore::default());
    }
    serde_yaml::from_str(&content).map_err(|e| format!("user-tasks.yaml のパースに失敗: {e}"))
}

pub fn load() -> Result<TaskStore, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    load_from(&path)
}

/// ロック付き read-modify-write（`config_io`。#169 と同パターン）。
/// **同時に 2 プロセスが add しても取りこぼさない**のはこの flock による
pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut TaskStore) -> R) -> Result<R, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("user-tasks.yaml の置き場を作れない: {e}"))?;
    }
    let _lock = crate::config_io::lock_exclusive(path)?;
    let mut store = load_from(path)?;
    let result = f(&mut store);
    let content =
        serde_yaml::to_string(&store).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    crate::config_io::atomic_write_with_backup(path, &content)?;
    Ok(result)
}

pub fn mutate<R>(f: impl FnOnce(&mut TaskStore) -> R) -> Result<R, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    mutate_at(&path, f)
}

// --- JSON 化（CLI / MCP / 画面が共有する形） ---------------------------------

/// タスク 1 件の JSON。
///
/// `attachments` は**パスと実在**を返す。起票時に実在を問わない（生成前に起票する
/// 運用がある）ぶん、読む側が「消えた添付」を出せるようにここで見る
pub fn task_to_json(task: &UserTask) -> Value {
    let attachments: Vec<Value> = task
        .attachments
        .iter()
        .map(|p| json!({ "path": p, "exists": Path::new(p).exists() }))
        .collect();
    let copy_texts: Vec<Value> = task
        .copy_texts
        .iter()
        .map(|c| json!({ "label": c.label, "text": c.text }))
        .collect();
    let responses: Vec<Value> = task
        .responses
        .iter()
        .map(|r| {
            json!({
                "decision": r.decision.as_str(),
                "comment": r.comment,
                "at": r.at,
                "via": r.via.as_str(),
            })
        })
        .collect();
    json!({
        "id": task.id,
        "title": task.title,
        "body": task.body,
        "kind": task.kind.as_str(),
        "status": task.status.as_str(),
        "created_by": task.created_by,
        "project": task.project,
        "attachments": attachments,
        "copy_texts": copy_texts,
        "links": task.links,
        "due": task.due,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
        "origin": task.origin.as_ref().map(origin_to_json),
        "responses": responses,
        "delivery": task.delivery.as_ref().map(delivery_to_json),
    })
}

fn origin_to_json(origin: &TaskOrigin) -> Value {
    json!({
        "profile": origin.profile,
        "session_id": origin.session_id,
        "pane": origin.pane,
        "project": origin.project,
    })
}

fn delivery_to_json(d: &Delivery) -> Value {
    json!({
        "state": d.state.as_str(),
        "profile": d.profile,
        "pane": d.pane,
        "tab": d.tab,
        "reason": d.reason,
        "at": d.at,
        "response_index": d.response_index,
    })
}

/// 一覧の応答。**`open_count` が B2 / B3 のバッジの正本**
pub fn list_json(store: &TaskStore, filter: &TaskFilter) -> Value {
    let items: Vec<Value> = store.list(filter).into_iter().map(task_to_json).collect();
    let by_kind: Vec<Value> = store
        .open_counts_by_kind()
        .iter()
        .map(|(kind, n)| json!({ "kind": kind.as_str(), "count": n }))
        .collect();
    json!({
        "tasks": items,
        "count": items.len(),
        "open_count": store.open_count(),
        "open_counts_by_kind": by_kind,
    })
}

// --- 語彙のパース（CLI / MCP が同じ綴りを使う） -----------------------------

pub fn parse_kind(s: &str) -> Result<TaskKind, String> {
    TaskKind::parse(s).ok_or_else(|| {
        format!(
            "不明な kind: {s}（{} のいずれか）",
            TaskKind::all()
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })
}

pub fn parse_status(s: &str) -> Result<TaskStatus, String> {
    TaskStatus::parse(s).ok_or_else(|| {
        format!(
            "不明な status: {s}（{} のいずれか）",
            TaskStatus::all()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })
}

pub fn parse_decision(s: &str) -> Result<Decision, String> {
    Decision::parse(s).ok_or_else(|| {
        format!(
            "不明な decision: {s}（{} のいずれか）",
            Decision::all()
                .iter()
                .map(|d| d.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })
}

pub fn parse_via(s: &str) -> Result<Via, String> {
    Via::parse(s).ok_or_else(|| {
        format!(
            "不明な via: {s}（{} のいずれか）",
            Via::all()
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })
}

/// `ラベル=本文` を [`CopyText`] にする。`=` が無ければラベルなし（`本文`）
pub fn parse_copy_text(raw: &str) -> CopyText {
    match raw.split_once('=') {
        Some((label, text)) if !label.trim().is_empty() => CopyText {
            label: label.trim().to_string(),
            text: text.to_string(),
        },
        _ => CopyText {
            label: String::new(),
            text: raw.to_string(),
        },
    }
}

// --- 操作（CLI / MCP / 画面が通る 1 実装） -----------------------------------

/// 起票。`origin` は呼び出し側（dispatch）が呼び出し元ペインから埋める
pub fn add_at(path: &Path, new: NewTask) -> Result<Value, String> {
    mutate_at(path, |store| store.add(new).map(task_to_json))?
}

pub fn list_at(path: &Path, filter: &TaskFilter) -> Result<Value, String> {
    let store = load_from(path)?;
    Ok(list_json(&store, filter))
}

pub fn show_at(path: &Path, id: &str) -> Result<Value, String> {
    let store = load_from(path)?;
    store
        .find(id)
        .map(task_to_json)
        .ok_or_else(|| format!("タスクが見つからない: {id}"))
}

pub fn update_at(path: &Path, id: &str, patch: TaskPatch) -> Result<Value, String> {
    mutate_at(path, |store| store.update(id, patch).map(task_to_json))?
}

pub fn set_status_at(path: &Path, id: &str, status: TaskStatus) -> Result<Value, String> {
    mutate_at(path, |store| store.set_status(id, status).map(task_to_json))?
}

/// 返答を積む。返るのは（積んだあとのタスク, 返答の index）。
/// 配送はこのあと呼び出し側（dispatch）が行い、結果を [`record_delivery_at`] で書き戻す
pub fn respond_at(
    path: &Path,
    id: &str,
    decision: Decision,
    comment: &str,
    via: Via,
) -> Result<(UserTask, usize), String> {
    mutate_at(path, |store| {
        let index = store.respond(id, decision, comment, via)?;
        let task = store
            .find(id)
            .cloned()
            .ok_or("直前に更新したタスクが無い")?;
        Ok((task, index))
    })?
}

pub fn record_delivery_at(path: &Path, id: &str, delivery: Delivery) -> Result<(), String> {
    mutate_at(path, |store| store.set_delivery(id, delivery))?
}

// --- 配送の判断（純粋。ペインへ触るのは dispatch 側） -----------------------

/// 生きているペインの 1 行（id と role ラベル）。dispatch が workspace から作る
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRole {
    pub pane: u64,
    pub role: Option<String>,
}

/// 返答をどこへ運ぶか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryTarget {
    /// 生きている master のペインへ送る
    Pane(u64),
    /// master が居ないので、このプロファイルで起動してから初回メッセージへ載せる
    Launch,
}

/// 起票元プロファイルを**どこから**決めたか（#1466 の診断用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginSource {
    /// 呼び出し元が master を名乗った（`TAKO_ORCHESTRATOR_ROLE=master[:<profile>]`）
    CallerRole,
    /// 起票したペイン自身の role ラベルが master だった
    PaneRole,
    /// 起票したペインから `spawned_by` を辿って master に着いた（= **spawn 時点の事実**）
    SpawnChain,
    /// worker のプロジェクトを管轄する master プロファイルが一意に決まった
    Jurisdiction,
}

impl OriginSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CallerRole => "caller_role",
            Self::PaneRole => "pane_role",
            Self::SpawnChain => "spawn_chain",
            Self::Jurisdiction => "jurisdiction",
        }
    }
}

/// 起票元が解けなかったときに配送へ残す理由（`tako todo show` の `配送:` 行に出る）。
///
/// **`default` へ落とさない**のが要点（#1466）。落とすと「たまたま生きている
/// default プロファイルの master」= 別プロジェクトの会話へ返答が入り、
/// 受け取った側にも起票した側にも何が起きたか分からない
pub const UNRESOLVED_ORIGIN_REASON: &str =
    "宛先不明: 起票元の master プロファイルを解決できない（呼び出し元の role も spawn 元の master も残っていない）";

/// #1466 の A/B。`TAKO_1466_LEGACY=1` で「master を名乗る呼び出しだけ解けて、
/// 残りは `default` へ落ちる」修正前の挙動を同一バイナリのまま再現する
pub fn legacy_origin_resolution() -> bool {
    std::env::var_os("TAKO_1466_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// 起票元のプロファイルを解く（**純粋**。#1466）。
///
/// 見る順は「**呼び出し元が名乗ったもの → tako が知っている spawn 時点の事実 →
/// 管轄の登録**」。前ほど確かで、どれも当たらなければ `None`（= 宛先不明）にする。
///
/// worker から起票されたときに **spawn 元の master** へ返るのがこの関数の目的で、
/// `spawn_chain` がその本命（`spawned_by` は spawn の瞬間に tako 自身が張った枝）。
/// `jurisdiction` は GUI 再起動で枝が失われた・master のペインを閉じた場合の保険で、
/// **一意に決まるときしか渡ってこない**（曖昧なら呼び出し側が `None` を渡す）。
///
/// 引数はすべて呼び出し側（dispatch）が workspace とプロファイル一覧から集めた
/// 材料で、この関数自身は env もファイルも見ない = 注入だけで全経路を検査できる
pub fn resolve_origin_profile(
    caller_role: Option<&str>,
    pane_master_profile: Option<&str>,
    spawn_chain_profile: Option<&str>,
    jurisdiction_profile: Option<&str>,
    legacy: bool,
) -> Option<(String, OriginSource)> {
    fn pick(p: Option<&str>) -> Option<&str> {
        p.map(str::trim).filter(|p| !p.is_empty())
    }
    if let Some(profile) = caller_role.and_then(tako_core::handoff::master_profile_of_any_role) {
        return Some((profile.to_string(), OriginSource::CallerRole));
    }
    if let Some(profile) = pick(pane_master_profile) {
        return Some((profile.to_string(), OriginSource::PaneRole));
    }
    if legacy {
        // #1466-legacy-arm: 修正前はここまでしか解かず、残りは `default` へ落ちていた
        return None;
    }
    if let Some(profile) = pick(spawn_chain_profile) {
        return Some((profile.to_string(), OriginSource::SpawnChain));
    }
    pick(jurisdiction_profile).map(|p| (p.to_string(), OriginSource::Jurisdiction))
}

/// 起票元のプロファイル。**解けていなければ `None`**（#1466）。
///
/// 旧挙動（`default` へ落ちる）は `TAKO_1466_LEGACY=1` のときだけ残す
pub fn origin_profile(task: &UserTask) -> Option<String> {
    task.origin
        .as_ref()
        .and_then(|o| o.profile.clone())
        .filter(|p| !p.is_empty())
        // #1466-legacy-arm
        .or_else(|| {
            legacy_origin_resolution().then(|| tako_core::handoff::DEFAULT_PROFILE.to_string())
        })
}

/// 配送先を決める（**純粋**）。
///
/// 起票したペインが今も同じプロファイルの master ならそこへ返す（同じ会話に返る）。
/// 居なければ同じプロファイルの master ペインを探し、それも無ければ起動する。
/// プロファイルの一致は `handoff::master_profile_of_role` の 1 実装で見るので、
/// `orchestrator-master` と `orchestrator-master:default` の揺れを取り違えない
pub fn choose_target(
    panes: &[PaneRole],
    profile: &str,
    origin_pane: Option<u64>,
) -> DeliveryTarget {
    let is_target = |p: &PaneRole| {
        p.role
            .as_deref()
            .and_then(tako_core::handoff::master_profile_of_role)
            .is_some_and(|found| found == profile)
    };
    if let Some(pane) = origin_pane {
        if panes.iter().any(|p| p.pane == pane && is_target(p)) {
            return DeliveryTarget::Pane(pane);
        }
    }
    match panes.iter().find(|p| is_target(p)) {
        Some(p) => DeliveryTarget::Pane(p.pane),
        None => DeliveryTarget::Launch,
    }
}

/// master の入力欄へ流す本文（**純粋**）。
///
/// ペインの内容も添付の中身も載せない（絶対ルール）。載せるのは
/// 「どのタスクへ / どんな返答が / どこから来たか」と、続きを引く口だけ
pub fn response_message(task: &UserTask, index: usize) -> String {
    let response = task.responses.get(index).or_else(|| task.responses.last());
    let (decision, comment, via) = match response {
        Some(r) => (r.decision.as_str(), r.comment.as_str(), r.via.as_str()),
        None => ("answered", "", "cli"),
    };
    let mut out = format!(
        "【ユーザー返答】todo {}「{}」\ndecision={decision} via={via} kind={} status={}",
        task.id,
        task.title,
        task.kind.as_str(),
        task.status.as_str(),
    );
    if !comment.is_empty() {
        out.push_str(&format!("\nコメント: {comment}"));
    }
    out.push_str(&format!(
        "\n本文と添付は `tako todo show {}` で読める。この返答を起点に続きを進めること。",
        task.id
    ));
    out
}

/// master を起動したときの初回メッセージ（**純粋**）。
///
/// 起動直後の master は会話を持っていないので、返答だけ渡しても文脈が無い。
/// タスクの本文も一緒に載せる（`show` を叩かせる往復を 1 回省く）
pub fn launch_message(task: &UserTask, index: usize) -> String {
    let mut out = response_message(task, index);
    if !task.body.trim().is_empty() {
        out.push_str("\n\n--- タスク本文 ---\n");
        out.push_str(task.body.trim());
    }
    out
}

/// 送達フローの顛末（`host.prompt_delivery_state` の JSON）から配送の状態を決める（**純粋**）。
///
/// `None`（フローが無い / 表示寿命 300 秒を過ぎた）は**決着扱いにしない**。
/// 分からないものを「届いた」とも「失敗した」とも言わない（#1259 と同じ規約）
pub fn settle_state(flow: Option<&Value>) -> Option<(DeliveryState, Option<String>)> {
    let flow = flow?;
    match flow["state"].as_str()? {
        "delivered" => Some((DeliveryState::Delivered, None)),
        "gave_up" => {
            let reason = flow["outcome"]
                .as_str()
                .or_else(|| flow["reason"].as_str())
                .unwrap_or("gave_up")
                .to_string();
            Some((DeliveryState::Failed, Some(reason)))
        }
        _ => None,
    }
}

/// 決着していない配送を `prompt_delivery_state` の結果で畳み込む。
///
/// 呼び出し側が `(タスク id, そのペインのフロー状態)` を作って渡す。
/// **読むたびに 1 回**行うので、画面（B2 / B3）が一覧を引けば `delivered` へ確定する
pub fn reconcile_at(path: &Path, observed: &[(String, Option<Value>)]) -> Result<usize, String> {
    let settled: Vec<(String, DeliveryState, Option<String>)> = observed
        .iter()
        .filter_map(|(id, flow)| {
            settle_state(flow.as_ref()).map(|(state, reason)| (id.clone(), state, reason))
        })
        .collect();
    if settled.is_empty() {
        return Ok(0);
    }
    mutate_at(path, |store| {
        let mut n = 0usize;
        for (id, state, reason) in &settled {
            if let Some(task) = store.find_mut(id) {
                if let Some(delivery) = task.delivery.as_mut() {
                    if delivery.state.is_pending() {
                        delivery.state = *state;
                        if reason.is_some() {
                            delivery.reason = reason.clone();
                        }
                        n += 1;
                    }
                }
            }
        }
        n
    })
}

/// 起票元が解けないときの配送記録（#1466）。**宛先を選ばない**ので pane も profile も持たない。
/// 理由は `tako todo show` の `配送:` 行と右パネル / PWA の配送状態にそのまま出る
pub fn unresolved_delivery(response_index: usize) -> Delivery {
    Delivery {
        state: DeliveryState::Failed,
        profile: None,
        pane: None,
        tab: None,
        reason: Some(UNRESOLVED_ORIGIN_REASON.to_string()),
        at: unix_now(),
        response_index: Some(response_index),
    }
}

/// 配送の記録を作る（`at` を 1 か所で入れる）
pub fn delivery_record(
    state: DeliveryState,
    profile: &str,
    pane: Option<u64>,
    tab: Option<u64>,
    reason: Option<String>,
    response_index: usize,
) -> Delivery {
    Delivery {
        state,
        profile: Some(profile.to_string()),
        pane,
        tab,
        reason,
        at: unix_now(),
        response_index: Some(response_index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(&format!("user-tasks-{tag}"))
    }

    fn sample(title: &str) -> NewTask {
        NewTask {
            title: title.into(),
            ..NewTask::default()
        }
    }

    #[test]
    fn 不在のファイルは空として読む() {
        let dir = temp_dir("absent");
        let path = dir.path().join("user-tasks.yaml");
        let store = load_from(&path).expect("不在は空");
        assert!(store.tasks.is_empty());
        assert_eq!(store.version, 1);
    }

    /// 壊れたファイルは**0 件へ丸めず** Err（#169）
    #[test]
    fn 壊れたファイルはエラーにする() {
        let dir = temp_dir("broken");
        let path = dir.path().join("user-tasks.yaml");
        std::fs::write(&path, "tasks: [ここで壊れている").unwrap();
        assert!(load_from(&path).is_err());
    }

    #[test]
    fn 書いて読み直せる() {
        let dir = temp_dir("roundtrip");
        let path = dir.path().join("user-tasks.yaml");
        let v = add_at(&path, sample("投稿してほしい")).unwrap();
        assert_eq!(v["id"], "u-1");
        let reloaded = load_from(&path).unwrap();
        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(reloaded.tasks[0].title, "投稿してほしい");
        // YAML として読めることの確認（tako-core 側は形式に依らない検査をしている）
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("version: 1"), "{text}");
        assert!(text.contains("u-1"), "{text}");
    }

    #[test]
    fn 一覧の応答はバッジの材料を持つ() {
        let dir = temp_dir("counts");
        let path = dir.path().join("user-tasks.yaml");
        for (title, kind) in [("a", TaskKind::Post), ("b", TaskKind::Review)] {
            add_at(
                &path,
                NewTask {
                    title: title.into(),
                    kind: Some(kind),
                    ..NewTask::default()
                },
            )
            .unwrap();
        }
        set_status_at(&path, "u-1", TaskStatus::Done).unwrap();
        let v = list_at(&path, &TaskFilter::default()).unwrap();
        assert_eq!(v["count"], 1, "既定の一覧は未完了だけ");
        assert_eq!(v["open_count"], 1);
        let by_kind = v["open_counts_by_kind"].as_array().unwrap();
        assert_eq!(by_kind.len(), TaskKind::all().len(), "内訳が全種類ぶん無い");
        let v = list_at(&path, &TaskFilter::all()).unwrap();
        assert_eq!(v["count"], 2);
    }

    #[test]
    fn 添付の実在が応答に出る() {
        let dir = temp_dir("attach");
        let path = dir.path().join("user-tasks.yaml");
        let present = dir.path().join("ある.txt");
        std::fs::write(&present, "x").unwrap();
        let missing = dir.path().join("ない.txt");
        add_at(
            &path,
            NewTask {
                title: "t".into(),
                attachments: vec![present.display().to_string(), missing.display().to_string()],
                ..NewTask::default()
            },
        )
        .unwrap();
        let v = show_at(&path, "u-1").unwrap();
        let a = v["attachments"].as_array().unwrap();
        assert_eq!(a[0]["exists"], true);
        assert_eq!(a[1]["exists"], false, "起票は通るが実在は嘘をつかない");
    }

    /// 同時に 2 つ書いても取りこぼさない（flock の read-modify-write）
    #[test]
    fn 同時のaddが取りこぼされない() {
        let dir = temp_dir("concurrent");
        let path = dir.path().join("user-tasks.yaml");
        std::thread::scope(|s| {
            for i in 0..8 {
                let path = path.clone();
                s.spawn(move || {
                    add_at(&path, sample(&format!("同時 {i}"))).expect("同時 add");
                });
            }
        });
        let store = load_from(&path).unwrap();
        assert_eq!(store.tasks.len(), 8, "同時 add が落ちている: {store:?}");
        let mut ids: Vec<&str> = store.tasks.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8, "id が重複している");
    }

    fn pane(id: u64, role: Option<&str>) -> PaneRole {
        PaneRole {
            pane: id,
            role: role.map(str::to_string),
        }
    }

    #[test]
    fn 配送先は起票ペインを優先する() {
        let panes = vec![
            pane(1, Some("orchestrator-master:takodev")),
            pane(2, Some("orchestrator-master:takodev")),
            pane(3, None),
        ];
        assert_eq!(
            choose_target(&panes, "takodev", Some(2)),
            DeliveryTarget::Pane(2)
        );
        // 起票ペインが死んでいれば同じプロファイルの別 master へ
        assert_eq!(
            choose_target(&panes, "takodev", Some(99)),
            DeliveryTarget::Pane(1)
        );
    }

    #[test]
    fn 別プロファイルのmasterへは返さない() {
        let panes = vec![pane(1, Some("orchestrator-master:other"))];
        assert_eq!(
            choose_target(&panes, "takodev", None),
            DeliveryTarget::Launch
        );
    }

    /// `orchestrator-master` と `orchestrator-master:default` は同じ既定プロファイル
    #[test]
    fn 既定プロファイルの綴れの揺れを吸収する() {
        for role in ["orchestrator-master", "orchestrator-master:default"] {
            assert_eq!(
                choose_target(&[pane(5, Some(role))], "default", None),
                DeliveryTarget::Pane(5),
                "{role}"
            );
        }
        // worker / solo は master ではない
        for role in ["orchestrator-worker", "solo", "master"] {
            assert_eq!(
                choose_target(&[pane(5, Some(role))], "default", None),
                DeliveryTarget::Launch,
                "{role}"
            );
        }
    }

    #[test]
    fn 返答の本文は画面内容を載せない() {
        let dir = temp_dir("message");
        let path = dir.path().join("user-tasks.yaml");
        add_at(
            &path,
            NewTask {
                title: "動画を投稿".into(),
                body: "サムネは docs/thumb.png".into(),
                kind: Some(TaskKind::Post),
                ..NewTask::default()
            },
        )
        .unwrap();
        let (task, index) = respond_at(
            &path,
            "u-1",
            Decision::NeedsChange,
            "文字を大きく",
            Via::Pwa,
        )
        .unwrap();
        let msg = response_message(&task, index);
        assert!(msg.contains("【ユーザー返答】todo u-1"), "{msg}");
        assert!(msg.contains("decision=needs_change"), "{msg}");
        assert!(msg.contains("via=pwa"), "{msg}");
        assert!(msg.contains("文字を大きく"), "{msg}");
        assert!(!msg.contains("docs/thumb.png"), "本文まで載せている: {msg}");
        // 起動時は文脈が無いので本文も載せる
        let launch = launch_message(&task, index);
        assert!(launch.contains("docs/thumb.png"), "{launch}");
    }

    #[test]
    fn 送達の顛末から配送状態を決める() {
        assert_eq!(
            settle_state(Some(&json!({ "state": "delivered" }))),
            Some((DeliveryState::Delivered, None))
        );
        assert_eq!(
            settle_state(Some(
                &json!({ "state": "gave_up", "outcome": "flow_timeout" })
            )),
            Some((DeliveryState::Failed, Some("flow_timeout".into())))
        );
        // 未決着・不明は決着させない（騙らない）
        assert_eq!(settle_state(Some(&json!({ "state": "queued" }))), None);
        assert_eq!(settle_state(Some(&json!({ "state": "waiting" }))), None);
        assert_eq!(settle_state(None), None);
    }

    #[test]
    fn 決着した配送だけを書き戻す() {
        let dir = temp_dir("reconcile");
        let path = dir.path().join("user-tasks.yaml");
        for i in 0..2 {
            add_at(&path, sample(&format!("t{i}"))).unwrap();
        }
        for id in ["u-1", "u-2"] {
            record_delivery_at(
                &path,
                id,
                delivery_record(DeliveryState::Sent, "default", Some(7), None, None, 0),
            )
            .unwrap();
        }
        let n = reconcile_at(
            &path,
            &[
                ("u-1".into(), Some(json!({ "state": "delivered" }))),
                ("u-2".into(), Some(json!({ "state": "waiting" }))),
            ],
        )
        .unwrap();
        assert_eq!(n, 1);
        let store = load_from(&path).unwrap();
        assert_eq!(
            store.find("u-1").unwrap().delivery.as_ref().unwrap().state,
            DeliveryState::Delivered
        );
        assert_eq!(
            store.find("u-2").unwrap().delivery.as_ref().unwrap().state,
            DeliveryState::Sent,
            "未決着を勝手に決着させている"
        );
        // 2 度目は何も動かない（冪等）
        assert_eq!(
            reconcile_at(
                &path,
                &[("u-1".into(), Some(json!({ "state": "gave_up" })))]
            )
            .unwrap(),
            0,
            "決着済みを上書きしている"
        );
    }

    #[test]
    fn コピー用テキストのラベル分解() {
        let c = parse_copy_text("投稿文=tako v0.9 を出しました");
        assert_eq!(c.label, "投稿文");
        assert_eq!(c.text, "tako v0.9 を出しました");
        let c = parse_copy_text("ラベルなしの本文");
        assert!(c.label.is_empty());
        assert_eq!(c.text, "ラベルなしの本文");
        // 本文に `=` があってもラベルは最初の 1 つだけで切る
        let c = parse_copy_text("URL=https://example.test/?a=b");
        assert_eq!(c.label, "URL");
        assert_eq!(c.text, "https://example.test/?a=b");
    }

    #[test]
    fn 語彙のパースは綴りを名指しで拒否する() {
        assert!(parse_kind("review").is_ok());
        let err = parse_kind("レビュー").unwrap_err();
        assert!(err.contains("review"), "{err}");
        assert!(parse_decision("needs_change").is_ok());
        assert!(parse_status("dismissed").is_ok());
        assert!(parse_via("pwa").is_ok());
        assert!(parse_via("sms").is_err());
    }

    /// #1466: 起票元が解けていないタスクは **`default` へ落ちない**
    /// （落ちると無関係なプロファイルの master へ返答が届く）
    #[test]
    fn 起票元が解けていなければ宛先は空のまま() {
        let mut store = TaskStore::default();
        store.add(sample("t")).unwrap();
        assert_eq!(
            origin_profile(store.find("u-1").unwrap()),
            None,
            "解けていない起票元が既定プロファイルへ落ちている（#1466）"
        );
        store.tasks[0].origin = Some(TaskOrigin {
            profile: Some("takodev".into()),
            ..TaskOrigin::default()
        });
        assert_eq!(origin_profile(&store.tasks[0]).as_deref(), Some("takodev"));
        // 空文字も「解けていない」（YAML を手で書き換えた場合の保険）
        store.tasks[0].origin = Some(TaskOrigin {
            profile: Some(String::new()),
            ..TaskOrigin::default()
        });
        assert_eq!(origin_profile(&store.tasks[0]), None);
    }

    /// #1466: 宛先不明の配送は「失敗 + 理由」で残る（無言で誰かへ届けない）
    #[test]
    fn 宛先不明の配送は理由を残して失敗する() {
        let d = unresolved_delivery(2);
        assert_eq!(d.state, DeliveryState::Failed);
        assert_eq!(d.profile, None, "宛先不明なのにプロファイルを名乗っている");
        assert_eq!(d.pane, None, "宛先不明なのにペインを選んでいる");
        assert_eq!(d.response_index, Some(2));
        let reason = d.reason.expect("理由が無い");
        assert!(reason.contains("宛先不明"), "{reason}");
    }

    /// #1466: 解決順は「呼び出し元の名乗り → ペインの role → spawn 元 → 管轄」
    #[test]
    fn 起票元は確かな順に解ける() {
        use OriginSource::*;
        // master を名乗る呼び出しが最優先（従来どおり）
        assert_eq!(
            resolve_origin_profile(Some("master:takodev"), None, None, None, false),
            Some(("takodev".into(), CallerRole))
        );
        // 表示用の語彙でも解ける
        assert_eq!(
            resolve_origin_profile(Some("orchestrator-master"), None, None, None, false),
            Some(("default".into(), CallerRole))
        );
        // worker の名乗りは master にならない → ペインの role ラベルを見る
        assert_eq!(
            resolve_origin_profile(Some("worker:tako:1466"), Some("sol"), None, None, false),
            Some(("sol".into(), PaneRole))
        );
        // ペインが master でなければ spawn 元（**#1466 の本命**）
        assert_eq!(
            resolve_origin_profile(Some("worker:tako:1466"), None, Some("takodev"), None, false),
            Some(("takodev".into(), SpawnChain))
        );
        // spawn 元の枝が失われていたら管轄の登録（一意のときだけ渡ってくる）
        assert_eq!(
            resolve_origin_profile(Some("worker:tako:1466"), None, None, Some("tako"), false),
            Some(("tako".into(), Jurisdiction))
        );
        // どれも当たらなければ宛先不明（**既定へ落とさない**）
        assert_eq!(
            resolve_origin_profile(Some("worker:tako:1466"), None, None, None, false),
            None
        );
        assert_eq!(resolve_origin_profile(None, None, None, None, false), None);
        // 空文字は「解けた」にしない
        assert_eq!(
            resolve_origin_profile(None, Some(""), Some("  "), Some(""), false),
            None
        );
    }

    /// #1466 の A/B: legacy アームは spawn 元も管轄も見ず、`default` へ落ちる旧挙動
    #[test]
    fn legacyアームは修正前の宛先を再現する() {
        assert_eq!(
            resolve_origin_profile(
                Some("worker:tako:1466"),
                None,
                Some("takodev"),
                Some("tako"),
                true
            ),
            None,
            "legacy アームが spawn 元を解いている（A/B にならない）"
        );
        // master を名乗る呼び出しは legacy でも解ける（修正前もここは動いていた）
        assert_eq!(
            resolve_origin_profile(Some("master:takodev"), None, None, None, true),
            Some(("takodev".into(), OriginSource::CallerRole))
        );
    }
}

//! worker レジストリ（Issue #390）。
//!
//! spawn した worker をペインとは独立の永続ファイル（workers.yaml）へ登録し、
//! アプリ再起動・ペイン消失後も watch / status / report が tmux session /
//! claude session ID 経由で追跡を継続できるようにする。
//!
//! 設計方針:
//! - sessions.yaml（会話カタログ。resume 用途）とは独立。こちらは worker の
//!   ライフサイクル（active / closed）と追跡キーだけを持つ
//! - あくまで**フォールバック層**: 既存の watch / worker_status の判定ロジックには
//!   手を入れず、pane 消失時の解決材料（tmux_session / session_id）を供給する
//!   （#273 / #289 の教訓: 判定変更は最小限に）
//! - レジストリの読み書き失敗で spawn / watch を止めない（呼び出し側は警告のみ）

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tako_core::pane_log::CloseOrigin;
use tako_core::prompt_delivery::Confidence;

/// prompt 未達検知の猶予秒数（Issue #390 要件 4）。
/// spawn からこの時間を超えても claude transcript（session_id）が観測できない
/// active worker は「プロンプト未達の疑い」とする。PromptFlow の総合タイムアウト
/// 120 秒 + claude 起動・セッション検出の遅延に十分な余裕を持たせた保守的な値
/// （誤検知で正常 worker を疑わせない）。判定には画面が busy でないこと等の
/// 複合条件を併用する（dispatch 側 = `prompt_delivery_assessment` の呼び出し元）
pub const PROMPT_DELIVERY_GRACE_SECS: i64 = 240;

/// 「ペインも器も消えた」状態がこの秒数続いた active エントリを closed へ倒す（Issue #658）。
/// **1 回の観測では倒さない**（この間隔をあけた 2 回以上の観測で同じ判定が出ることを要求する）。
/// 器（tmux / psmux）の列挙が一時的に失敗する・アプリ再起動直後でペインがまだ復元されて
/// いない、といった過渡状態で生きている worker を closed にしないための確認期間。
/// #390 の「ペイン消失後も追跡する」意図は closed でも壊れない（resume_command /
/// report / `workers --all` は status を問わず引ける）
pub const DEAD_CONFIRM_SECS: i64 = 300;

/// closed エントリを含めた保持上限。超過分は古い closed から削除する
const MAX_WORKERS: usize = 200;

/// テスト専用: registry_path() をプロセス毎の一時ファイルへ固定する。
/// unit テスト（spawn 経由の record_spawn 等）が実運用の workers.yaml を
/// 読み書きして汚染・誤読するのを防ぐ（orchestrator::test_config_dir_override と同思想）
#[cfg(test)]
fn test_registry_path() -> &'static std::sync::OnceLock<PathBuf> {
    static OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    &OVERRIDE
}

/// workers.yaml のパス。`TAKO_WORKERS_FILE` で差し替え可能（テスト・隔離用）
pub fn registry_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(
            test_registry_path()
                .get_or_init(|| {
                    // `.bak.N` / `.lock` も脇に出来るので、TMPDIR 直下ではなく
                    // プロセスごとの使い捨ての親の下へ置く（終了時に親ごと消える = #1312）
                    tako_core::test_residue::process_scratch("workers-registry")
                        .join("workers.yaml")
                })
                .clone(),
        )
    }
    #[cfg(not(test))]
    {
        if let Some(p) = std::env::var_os("TAKO_WORKERS_FILE") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        tako_core::paths::data_dir().map(|d| d.join("workers.yaml"))
    }
}

/// レジストリ本体（workers.yaml のスキーマ）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerRegistry {
    /// worker ID（連番の文字列）→ エントリ
    #[serde(default)]
    pub workers: BTreeMap<String, WorkerEntry>,
    /// 次に発番する ID
    #[serde(default)]
    pub next_id: u64,
}

/// レジストリの 1 エントリ。ペイン消失後の追跡に必要なキーを集約する
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// エージェント種別（claude / codex / agy）。prompt 未達検知は claude のみ対象
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// spawn が**宣言した**モデル（`--model` に渡した値。CLI の既定に委ねたら None）。
    /// 実際に使われたモデルは `model_actual`（#1638）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 実際に使われたモデル（#1638。claude の transcript の最初の assistant 行の `message.model`）。
    /// 確定するまで None。**一度確定したら書き換えない**（spawn 時の解決先を残す）。
    /// #1638 前のファイルには無い = serde の default で None として読める（移行は要らない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_actual: Option<String>,
    /// `model_actual` を確定した時刻
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_actual_at: Option<String>,
    /// `model_actual` を読んだ transcript が属したアカウント名（accounts.yaml の逆引き。
    /// 既定の設定 dir / 登録の無い dir は None）。食い違いの警告文に名前を出すため
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_actual_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// spawn 時のペイン ID（tako 再起動後も layout 復元で同一 ID が維持される）
    pub pane: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    /// tmux バックエンドセッション名（ペイン消失時の第一フォールバックキー）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    /// claude の session ID（検出後に埋まる。transcript 直読の第二フォールバックキー）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    /// 委任台帳のエントリ ID（Issue #292 との突き合わせ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spawned_at: String,
    /// active / closed
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// closed の理由（explicit_close 等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
    /// claude transcript（session_id）を最初に観測した時刻 = プロンプト到達の証跡
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_delivered_at: Option<String>,
    /// 「ペインも器も消えている」と**最初に**観測した時刻（Issue #658 の GC 用）。
    /// 生存が再観測されたら消える。`DEAD_CONFIRM_SECS` を超えて残ったら closed へ倒す
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_since: Option<String>,
    /// 送達フロー（PromptFlow）がプロンプトの到達を確認できずに打ち切った時刻（Issue #530）。
    /// session 検出（= claude が起動しただけ）より優先して未達と判定するための証跡
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_delivery_failed_at: Option<String>,
    /// 未達の理由コード（`choice_dialog` = 選択ダイアログに阻まれた / `paste_not_reflected` /
    /// `residual_after_retries` / `flow_timeout`）。規約により画面内容・送信テキストは含めない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_delivery_failure: Option<String>,
    /// 起動に失敗したと観測した時刻（Issue #1940）。spawn は起動コマンドを積んで即返るので、
    /// 「起動コマンドが実行された直後にエージェントが終わった」等は後からここへ記録する
    /// （旧実装は spawn が成功を返したまま worker が黙って止まっていた）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_failed_at: Option<String>,
    /// 起動失敗の理由コード（[`LaunchFailure`] の綴り）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_failure: Option<String>,
    /// `agent_exited` のとき、シェルが報告した終了コード（OSC 133;D。無ければ None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_exit_code: Option<i32>,
    /// spawn した master のプロファイル名（#2470。`default` を含む）。突然死からの復旧
    /// （`tako orchestrator resume <worker_id>`）が spawn と同じプロファイルで組むための記録。
    /// #2470 前のファイルには無い = serde の default で None として読める（移行は要らない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// 起動失敗の理由（Issue #1940）。綴りは `workers` の `launch_failure` にそのまま出る
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchFailure {
    /// 起動コマンドは実行されたが、エージェントがすぐ終わってシェルへ戻った
    /// （不正な引数・未認証・存在しないモデル等。終了コードは `launch_exit_code`）
    AgentExited,
    /// 起動コマンドを打ち切りまでに送り届けられなかった（シェルが立たない等）
    CommandFlowTimeout,
}

impl LaunchFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentExited => "agent_exited",
            Self::CommandFlowTimeout => "command_flow_timeout",
        }
    }
}

impl WorkerEntry {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

/// prompt 送達状態の判定結果（`prompt_delivery_assessment`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDelivery {
    /// transcript（session_id）を観測済み = プロンプト到達
    Delivered,
    /// 未観測だが猶予時間内（起動・検出待ち）
    Pending,
    /// 猶予時間を超えても未観測 = プロンプト未達の疑い。
    /// 最終判定は画面状態（busy でない等）と併せて呼び出し側が行う
    OverdueSuspect,
    /// 猶予時間を超えたが、**この系統には送達を裏づける一次シグナルが無い**（#983）。
    ///
    /// 「届いていない」のか「届いたが観測できていない」のかを区別できないので、
    /// 未達と断定しない = 自動再送（supervisor の `recover_prompt_undelivered`）を
    /// 撃たせない。**黙る（`NotApplicable`）のではなく「未確認」と言う**のが要点で、
    /// master には「画面を見て確かめてから再送する」次の一手が出る
    Unverified,
    /// 判定対象外（closed、時刻パース不能）
    NotApplicable,
}

/// PromptFlow の用途（Issue #778）。worker レジストリが追跡するのは spawn 時の
/// 初回プロンプトだけで、稼働中 worker への後続 send は同じ送達確認ループを使っても
/// spawn プロンプトの送達状態を変更しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDeliveryFlow {
    SpawnPrompt,
    FollowUpSend,
}

impl PromptDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Pending => "pending",
            Self::OverdueSuspect => "undelivered",
            Self::Unverified => "unverified",
            Self::NotApplicable => "n/a",
        }
    }
}

/// 読み取りの使い回し（#1968）。ファイルの中身が前回と**バイト単位で同じ**なら、前回の解釈結果を返す。
///
/// worker の状態照会・報告は 1 回ごとにレジストリを読み、master が watch で 5 秒ごとに
/// 引くので毎秒数回になる。本番の workers.yaml は 200 KB あり、YAML の解釈が照会のたびに
/// UI スレッド（`resolve_worker_query`）と background の両方で走っていた。
/// 更新時刻や inode ではなく中身で比べるのは、Windows の更新時刻の粒度が粗く、同じ長さの
/// 書き直しを取り違えうるから（読み取りと比較は解釈の 100 分の 1 未満）
static LOAD_CACHE: std::sync::Mutex<Option<(PathBuf, String, WorkerRegistry)>> =
    std::sync::Mutex::new(None);

impl WorkerRegistry {
    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）。
    ///
    /// 中身が前回と同じなら解釈し直さずに写しを返す（#1968）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("workers.yaml の読み取りに失敗: {e}"))?;
        if crate::diag::issue1968_legacy() {
            return Self::parse(&content);
        }
        {
            let cache = LOAD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((p, c, reg)) = cache.as_ref() {
                if p == path && *c == content {
                    return Ok(reg.clone());
                }
            }
        }
        let registry = Self::parse(&content)?;
        *LOAD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((path.to_path_buf(), content, registry.clone()));
        Ok(registry)
    }

    fn parse(content: &str) -> Result<Self, String> {
        serde_yaml::from_str(content).map_err(|e| {
            format!(
                "workers.yaml のパースに失敗: {}",
                tako_core::load_issue::yaml_error_reason(&e, content)
            )
        })
    }

    pub fn load() -> Result<Self, String> {
        let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同型）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        // ロックの下でファイルを読む（中身で比べる使い回しなので、書き手が読んでも古くならない）
        let mut registry = Self::load_from(path)?;
        let result = f(&mut registry);
        let content = serde_yaml::to_string(&registry)
            .map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    /// worker ID（完全一致 → 前方一致）でエントリを解決する
    pub fn resolve(&self, id_prefix: &str) -> Result<(&String, &WorkerEntry), String> {
        if let Some((id, entry)) = self.workers.get_key_value(id_prefix) {
            return Ok((id, entry));
        }
        let matches: Vec<_> = self
            .workers
            .iter()
            .filter(|(id, _)| id.starts_with(id_prefix))
            .collect();
        match matches.len() {
            0 => Err(format!(
                "worker '{id_prefix}' がレジストリに見つからない（tako orchestrator workers で確認）"
            )),
            1 => Ok(matches[0]),
            n => Err(format!(
                "worker '{id_prefix}' の候補が {n} 件ある（完全な ID を指定）: {}",
                matches
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// pane ID から active な worker を引く（pane 消失時のフォールバック解決）。
    /// 同一 pane に複数 active が居る場合（異常系）は最新の spawned_at を返す
    pub fn find_active_by_pane(&self, pane: u64) -> Option<(&String, &WorkerEntry)> {
        self.workers
            .iter()
            .filter(|(_, e)| e.pane == pane && e.is_active())
            .max_by(|a, b| a.1.spawned_at.cmp(&b.1.spawned_at))
    }

    /// 古い closed エントリから削って上限を強制する
    fn gc(&mut self) {
        if self.workers.len() <= MAX_WORKERS {
            return;
        }
        let mut closed: Vec<(String, String)> = self
            .workers
            .iter()
            .filter(|(_, e)| !e.is_active())
            .map(|(id, e)| (e.spawned_at.clone(), id.clone()))
            .collect();
        closed.sort(); // spawned_at 昇順 = 古い順
        let drop_count = self.workers.len() - MAX_WORKERS;
        for (_, id) in closed.into_iter().take(drop_count) {
            self.workers.remove(&id);
        }
    }
}

/// spawn 時の登録内容（`record_spawn` の入力）
#[derive(Debug, Clone, Default)]
pub struct RegisterSpawn {
    pub label: Option<String>,
    pub project: String,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub pane: u64,
    pub tab: Option<u64>,
    pub tmux_session: Option<String>,
    pub issues: Vec<u32>,
    pub ledger_id: Option<String>,
    pub cwd: Option<String>,
    pub prompt_head: Option<String>,
    /// spawn した master のプロファイル名（#2470）
    pub profile: Option<String>,
}

/// spawn した worker をレジストリへ登録し、発番した worker ID を返す。
/// 同一 pane の既存 active エントリは closed へ倒す（pane ID 再利用時の残骸対策。
/// 二重登録で同一 pane に active が 2 件並ぶ状態を作らない）
pub fn record_spawn(record: RegisterSpawn) -> Result<String, String> {
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate(|reg| {
        for entry in reg.workers.values_mut() {
            if entry.pane == record.pane && entry.is_active() {
                entry.status = "closed".into();
                entry.closed_at = Some(now.clone());
                entry.close_reason = Some("superseded".into());
            }
        }
        reg.next_id += 1;
        let id = reg.next_id.to_string();
        reg.workers.insert(
            id.clone(),
            WorkerEntry {
                label: record.label.clone(),
                project: record.project.clone(),
                agent: record.agent.clone(),
                model: record.model.clone(),
                model_actual: None,
                model_actual_at: None,
                model_actual_account: None,
                effort: record.effort.clone(),
                pane: record.pane,
                tab: record.tab,
                tmux_session: record.tmux_session.clone(),
                session_id: None,
                issues: record.issues.clone(),
                ledger_id: record.ledger_id.clone(),
                cwd: record.cwd.clone(),
                prompt_head: record.prompt_head.clone(),
                spawned_at: now.clone(),
                status: "active".into(),
                closed_at: None,
                close_reason: None,
                prompt_delivered_at: None,
                dead_since: None,
                prompt_delivery_failed_at: None,
                prompt_delivery_failure: None,
                launch_failed_at: None,
                launch_failure: None,
                launch_exit_code: None,
                profile: record.profile.clone(),
            },
        );
        reg.gc();
        id
    })
}

/// 明示 close の `close_reason` をひと所で綴る（#775）。
///
/// 綴りは**ペインログのクローズマーカーと同じ 1 実装**（`CloseOrigin::marker_with_caller`）
/// から採る。`close:gui` / `close:gui-tab` / `close:kbd` / `close:dispatch(cli, caller=…)`
/// のように発生源が載るので、`workers.yaml` と persist.log / ペインログの監査行を
/// **同じ語彙**で突き合わせられる（#770 の調査で「誰がどの経路で閉じたか」を
/// 消去法でしか絞れなかったことへの手当て）。
///
/// 旧値 `explicit_close` は「明示 close」以上の情報を持たず、値で分岐する消費側も
/// 無い（書き込みと `workers` の JSON 露出だけ）ので置き換える。過去のファイルに
/// 残る `explicit_close` は `Option<String>` のまま読めるので移行は要らない。
/// A/B は `TAKO_775_LEGACY=1`（旧の固定値へ戻す）
pub fn close_reason_for(origin: CloseOrigin, caller_role: Option<&str>) -> String {
    if std::env::var("TAKO_775_LEGACY").as_deref() == Ok("1") {
        return "explicit_close".to_string();
    }
    origin.marker_with_caller(caller_role)
}

/// 明示 close されたペインの active worker を closed にする（発生源つき。#775）。
///
/// **close 経路（GUI / CLI / MCP）はすべてこの入口を通る**。`close_reason` の綴りを
/// 呼び出し側が各々決めると経路ごとに語彙がずれるので、[`close_reason_for`] へ寄せた。
///
/// プロセス終了（`CloseOrigin::ProcessExit`）は**記録しない**。#390 の
/// 「ペインが消えても worker は生きている」追跡を壊さないための不変条件で、
/// 呼び出し側の取り違えをここでも止める（消えたままなら GC が 5 分後に `gone` で倒す）
pub fn mark_closed_by_origin(
    pane: u64,
    origin: CloseOrigin,
    caller_role: Option<&str>,
) -> Result<(), String> {
    if origin == CloseOrigin::ProcessExit {
        return Ok(());
    }
    mark_closed_by_pane(pane, &close_reason_for(origin, caller_role))
}

/// 明示 close されたペインの active worker を closed にする。
/// レジストリ不在（orchestrator 未使用）は何もしない（通常ペインの close に
/// ファイル IO のコストを掛けない）。**worker でないペインでも書き込まない**
/// （全ペインの close 経路から呼ばれるため。#658 で GUI 経路にも配線した）。
/// 発生源を持っている呼び出し側は [`mark_closed_by_origin`] を使うこと（#775）
pub fn mark_closed_by_pane(pane: u64, reason: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    mark_closed_by_pane_at(&path, pane, reason)
}

/// `mark_closed_by_pane` のパス指定版（テスト用。実体はこちら）
pub fn mark_closed_by_pane_at(path: &Path, pane: u64, reason: &str) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let hit = WorkerRegistry::load_from(path)?
        .workers
        .values()
        .any(|e| e.pane == pane && e.is_active());
    if !hit {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.pane == pane && entry.is_active() {
                entry.status = "closed".into();
                entry.closed_at = Some(now.clone());
                entry.close_reason = Some(reason.to_string());
            }
        }
    })
}

/// 検出済み claude session をレジストリへ反映する（ペイン ID キー。#728）。
///
/// 器を持たない構成（Windows で psmux 未導入 / tmux 不在の macOS）では
/// `tmux_session` が無く [`record_session_detected`] が 1 件も引けない。
/// ペイン ID で active な worker を引いて同じ更新をかける。
///
/// ペイン ID 再利用の誤マッチが気になるところだが、ここで渡ってくるのは
/// **今まさにそのペインで claude が動いている**という実プロセスの観測結果なので、
/// active な worker が居るなら同一世代とみなしてよい。
/// 冪等（変化が無ければ書き込まない）なのは tmux_session 版と同じ
pub fn record_session_detected_by_pane(pane: u64, session_id: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    let legacy = tako_core::shell_send::legacy_1940();
    let current = WorkerRegistry::load_from(&path)?;
    let needs_update = current.workers.values().any(|e| {
        e.is_active()
            && e.pane == pane
            && (e.session_id.as_deref() != Some(session_id)
                || (legacy && e.prompt_delivered_at.is_none()))
    });
    if !needs_update {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(&path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.is_active() && entry.pane == pane {
                entry.session_id = Some(session_id.to_string());
                // #1940: 会話の検出は「起動した」証拠であって「依頼文が届いた」証拠ではない。
                // 到達の時刻は送達フローの確認（`record_prompt_delivery`）だけが書く
                if legacy && entry.prompt_delivered_at.is_none() {
                    entry.prompt_delivered_at = Some(now.clone());
                }
            }
        }
    })
}

/// 検出済み claude session をレジストリへ反映する（tmux_session キー）。
/// GUI の定期スキャンおよび worker_status の解決成功時（lazy 昇格）から呼ばれる。
///
/// #1940: 以前は session_id の初観測を「プロンプト到達の証跡」として
/// `prompt_delivered_at` も書いていた。しかし session_id は `claude agents --json`
/// （起動中の会話一覧）から取るので**依頼文が届く前の起動直後に付く**。そのため
/// `prompt_delivery` が届く前に `delivered` を返し、送達フローが諦めた後に
/// `undelivered` へ反転していた（本番 10/9 の pane 2471）。到達の時刻は送達フローの
/// 確認だけが書く（旧挙動は `TAKO_1940_LEGACY=1`）
pub fn record_session_detected(tmux_session: &str, session_id: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    record_session_detected_at(&path, tmux_session, session_id)
}

fn record_session_detected_at(
    path: &Path,
    tmux_session: &str,
    session_id: &str,
) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    // 変更が無いなら書き込みをスキップ（定期スキャンからの毎回書き込み防止）
    let legacy = tako_core::shell_send::legacy_1940();
    let current = WorkerRegistry::load_from(path)?;
    let needs_update = current.workers.values().any(|e| {
        e.is_active()
            && e.tmux_session.as_deref() == Some(tmux_session)
            && (e.session_id.as_deref() != Some(session_id)
                || (legacy && e.prompt_delivered_at.is_none()))
    });
    if !needs_update {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.is_active() && entry.tmux_session.as_deref() == Some(tmux_session) {
                entry.session_id = Some(session_id.to_string());
                if legacy && entry.prompt_delivered_at.is_none() {
                    entry.prompt_delivered_at = Some(now.clone());
                }
            }
        }
    })
}

/// 起動の失敗をレジストリへ記録する（Issue #1940）。
///
/// pane 番号で引く（起動コマンドの送達フローが持つキー）。同番号ペインの再利用による
/// 誤更新を防ぐため、active かつまだ起動失敗を記録していないエントリだけを対象にする。
/// worker でないペイン（master の起動・手動のペイン）は該当が無く no-op
pub fn record_launch_failure(
    pane: u64,
    failure: LaunchFailure,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    record_launch_failure_at(&path, pane, failure, exit_code)
}

fn record_launch_failure_at(
    path: &Path,
    pane: u64,
    failure: LaunchFailure,
    exit_code: Option<i32>,
) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let current = WorkerRegistry::load_from(path)?;
    let target = |e: &WorkerEntry| e.is_active() && e.pane == pane && e.launch_failed_at.is_none();
    if !current.workers.values().any(target) {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        for entry in reg.workers.values_mut() {
            if target(entry) {
                entry.launch_failed_at = Some(now.clone());
                entry.launch_failure = Some(failure.as_str().to_string());
                entry.launch_exit_code = exit_code;
            }
        }
    })
}

/// 起動待ちの後に agent を起こした（#2413）: active で、まだ起動の証拠が無いエントリの `spawned_at` を
/// 今へ直す。送達の猶予（[`PROMPT_DELIVERY_GRACE_SECS`]）は起動からの秒数なので、待った分で
/// 「依頼文が届いていない」と言わない（supervisor の自動再送がシェルへ依頼文を撃たない）
pub fn record_launch_started(pane: u64) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    record_launch_started_at(&path, pane)
}

fn record_launch_started_at(path: &Path, pane: u64) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let current = WorkerRegistry::load_from(path)?;
    let target = |e: &WorkerEntry| {
        e.is_active()
            && e.pane == pane
            && e.session_id.is_none()
            && e.prompt_delivered_at.is_none()
            && e.launch_failed_at.is_none()
    };
    if !current.workers.values().any(target) {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        for entry in reg.workers.values_mut() {
            if target(entry) {
                entry.spawned_at = now.clone();
            }
        }
    })
}

/// 起動の状態（Issue #1940）。`workers` の `launch` に出る。
///
/// - `failed`: 起動失敗を記録した（理由は `launch_failure`）
/// - `started`: エージェントの会話を検出した / 依頼文の到達を確認した
/// - `pending`: どちらもまだ（起動途中。会話を検出できない系統もここに留まる）
/// - `waiting`: メモリの逼迫で起動を待たせている（#2413。`workers` が起動待ちを見て上書きする。
///   理由と順番は `launch_gate`）
pub fn launch_state(entry: &WorkerEntry) -> &'static str {
    if entry.launch_failed_at.is_some() {
        "failed"
    } else if entry.session_id.is_some() || entry.prompt_delivered_at.is_some() {
        "started"
    } else {
        "pending"
    }
}

// ─────────────── 宣言したモデルと実際のモデル（Issue #1638） ───────────────
//
// spawn は `--model` を宣言するだけで、claude CLI はアカウントの entitlement やモデルの廃止で
// **無言で**別のモデルへ解決することがある（2026-09-23 実測: 別アカウントの worker で 5.5 → 5）。宣言（`model`）と
// 実際（`model_actual`）を分けて持ち、食い違えば `workers` / `worker_status` の warnings と
// persist.log へ出し、ledger は実際のモデルで集計する。
//
// 実際のモデルの出どころは **transcript の最初の assistant 行の `message.model`**（API が返した名前）。
// tako mod の `$.session.model()` は「宣言を返すのか解決後を返すのか」が確かめられていないので
// 使わない（宣言を返すなら食い違いを一致と言ってしまう）。mod の報告からは transcript の置き場の
// 手がかり（設定 dir）だけを借りる。どの系統が観測できるかの正本は
// `agent_support` の `worker_model_actual`（codex / agy は degraded = 宣言だけ）

/// #1638 の A/B。`TAKO_1638_LEGACY=1` で**同一バイナリのまま**修正前（実際のモデルを採らず、
/// ledger を宣言のモデルで集計する）へ戻す
pub fn legacy_1638() -> bool {
    std::env::var_os("TAKO_1638_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// この worker の実際のモデルを観測する系統か（マトリクスの `worker_model_actual` が Supported）。
/// 知らない agent 名は観測しない側へ倒す（観測していないのに一致と言わない）
pub fn observes_model_actual(entry: &WorkerEntry) -> bool {
    use tako_core::agent_support::{keys, support_for, Agent, AgentSupport};
    Agent::parse(&entry.agent).is_some_and(|agent| {
        support_for(agent, keys::WORKER_MODEL_ACTUAL) == Some(AgentSupport::Supported)
    })
}

/// 宣言と実際の突き合わせ（`workers` / `worker_status` の `model_check`）
pub fn model_check(entry: &WorkerEntry) -> tako_core::model_check::ModelCheck {
    if !observes_model_actual(entry) {
        return tako_core::model_check::ModelCheck::NotObserved;
    }
    tako_core::model_check::check(entry.model.as_deref(), entry.model_actual.as_deref())
}

/// 食い違いの 1 行（一致・未確定・比べられないなら None）
pub fn model_warning(entry: &WorkerEntry) -> Option<String> {
    if model_check(entry) != tako_core::model_check::ModelCheck::Mismatch {
        return None;
    }
    Some(tako_core::model_check::mismatch_warning(
        entry.model.as_deref()?,
        entry.model_actual.as_deref()?,
        entry.model_actual_account.as_deref(),
    ))
}

/// 実際のモデルを 1 件確定した結果（#1638）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelObservation {
    pub worker_id: String,
    pub pane: u64,
    pub declared: Option<String>,
    pub actual: String,
    pub account: Option<String>,
    pub ledger_id: Option<String>,
}

impl ModelObservation {
    pub fn check(&self) -> tako_core::model_check::ModelCheck {
        tako_core::model_check::check(self.declared.as_deref(), Some(&self.actual))
    }

    /// 食い違いなら warnings / persist.log に出す 1 行
    pub fn warning(&self) -> Option<String> {
        (self.check() == tako_core::model_check::ModelCheck::Mismatch).then(|| {
            tako_core::model_check::mismatch_warning(
                self.declared.as_deref().unwrap_or_default(),
                &self.actual,
                self.account.as_deref(),
            )
        })
    }
}

/// 会話 ID から「最初の応答のモデル, 所属アカウント名」を引く手段（テストは偽の transcript を渡す）
pub type ModelProbe<'a> = dyn Fn(&str) -> Option<(String, Option<String>)> + 'a;

/// 実際のモデルがまだ確定していない、観測できる系統の active worker か
fn awaits_model_actual(entry: &WorkerEntry) -> bool {
    entry.is_active() && entry.model_actual.is_none() && observes_model_actual(entry)
}

/// 本番の探し方: `hint`（tako mod の報告の設定 dir）があれば先に見て、無ければ全設定 dir
/// （既定 + accounts.yaml。`transcript::claude_config_dirs`）を走査する。
/// まだ transcript が無い・応答がまだなら None（= 未確定のまま。次の観測でまた探す）
pub fn probe_transcript_model(
    session_id: &str,
    hint: Option<&Path>,
) -> Option<(String, Option<String>)> {
    let default_dir = super::claude_default_config_dir();
    let location = hint
        .and_then(|dir| {
            crate::transcript::locate_transcript_in(
                &[dir.to_path_buf()],
                default_dir.as_deref(),
                session_id,
            )
        })
        .or_else(|| crate::transcript::locate_transcript(session_id))?;
    let model = crate::transcript::first_assistant_model_at(&location.path)?;
    // 既定の設定 dir は `CLAUDE_CONFIG_DIR` 未設定 = inherit のアカウント（#512）
    let key = (!location.is_default).then(|| location.config_dir.display().to_string());
    let account = super::AccountsConfig::load()
        .ok()
        .and_then(|cfg| super::account_name_for_config_dir(&cfg, key.as_deref()));
    Some((model, account))
}

/// 実際のモデルを記録する。**未確定のエントリだけ**を書き、別の会話へ移ったものは触らない。
/// 書いたら `Some`（呼び出し側が [`publish_observation`] へ流す）
fn record_model_actual_at(
    path: &Path,
    worker_id: &str,
    session_id: &str,
    actual: &str,
    account: Option<String>,
) -> Result<Option<ModelObservation>, String> {
    let fits = |e: &WorkerEntry| {
        awaits_model_actual(e) && e.session_id.as_deref().is_none_or(|s| s == session_id)
    };
    // 変化が無いなら書かない（読み取りは中身で比べる使い回し = #1968）
    if !WorkerRegistry::load_from(path)?
        .workers
        .get(worker_id)
        .is_some_and(fits)
    {
        return Ok(None);
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        let entry = reg.workers.get_mut(worker_id).filter(|e| fits(e))?;
        entry.model_actual = Some(actual.to_string());
        entry.model_actual_at = Some(now);
        entry.model_actual_account = account.clone();
        Some(ModelObservation {
            worker_id: worker_id.to_string(),
            pane: entry.pane,
            declared: entry.model.clone(),
            actual: actual.to_string(),
            account,
            ledger_id: entry.ledger_id.clone(),
        })
    })
}

/// 確定した観測を外へ流す: 食い違いなら persist.log へ 1 行、委任台帳へ実際のモデルを書く。
/// 1 件につき 1 回だけ通る（記録は未確定のエントリにしか書かない）
fn publish_observation(obs: &ModelObservation) {
    if let Some(line) = obs.warning() {
        crate::diag::persist_log(&format!(
            "[orchestrator] worker={} pane={} {line}",
            obs.worker_id, obs.pane
        ));
    }
    if let Some(id) = obs.ledger_id.as_deref() {
        if let Err(e) = super::ledger::record_model_actual(id, &obs.actual) {
            eprintln!("warning: 委任台帳へ実際のモデルを書けない: {e}");
        }
    }
}

/// 会話 ID の分かっている active worker のうち、実際のモデルが未確定のものを transcript で
/// 確かめて記録する（#1638）。GUI の定期スキャン（spawn 直後は即時）と `workers` 一覧が呼ぶ。
/// 未確定の worker が居なければレジストリを読むだけで transcript には触らない
pub fn observe_pending_models() -> Vec<ModelObservation> {
    let Some(path) = registry_path() else {
        return Vec::new();
    };
    observe_pending_models_at(&path, &|sid| probe_transcript_model(sid, None))
}

/// [`observe_pending_models`] のパス・探し方を指定する版（テスト用。実体はこちら）
pub fn observe_pending_models_at(path: &Path, probe: &ModelProbe) -> Vec<ModelObservation> {
    if legacy_1638() || !path.is_file() {
        return Vec::new();
    }
    let Ok(registry) = WorkerRegistry::load_from(path) else {
        return Vec::new();
    };
    let targets: Vec<(String, String)> = registry
        .workers
        .iter()
        .filter(|(_, e)| awaits_model_actual(e))
        .filter_map(|(id, e)| Some((id.clone(), e.session_id.clone()?)))
        .collect();
    let mut found = Vec::new();
    for (id, sid) in targets {
        let Some((actual, account)) = probe(&sid) else {
            continue;
        };
        match record_model_actual_at(path, &id, &sid, &actual, account) {
            Ok(Some(obs)) => {
                publish_observation(&obs);
                found.push(obs);
            }
            Ok(None) => {}
            Err(e) => eprintln!("warning: worker レジストリへ実際のモデルを書けない: {e}"),
        }
    }
    found
}

/// `worker_status` の 1 件版。会話 ID はレジストリに未記録でも照会で解決できていれば渡せる。
/// `hint` は tako mod の報告の設定 dir（accounts.yaml に無い設定 dir の claude でも探せる）。
/// 確定済み・観測しない系統なら何もしない
pub fn observe_worker_model(
    worker_id: &str,
    entry: &WorkerEntry,
    session_id: Option<&str>,
    hint: Option<&Path>,
) -> Option<ModelObservation> {
    let path = registry_path()?;
    observe_worker_model_at(&path, worker_id, entry, session_id, &|sid| {
        probe_transcript_model(sid, hint)
    })
}

/// [`observe_worker_model`] のパス・探し方を指定する版
pub fn observe_worker_model_at(
    path: &Path,
    worker_id: &str,
    entry: &WorkerEntry,
    session_id: Option<&str>,
    probe: &ModelProbe,
) -> Option<ModelObservation> {
    if legacy_1638() || !awaits_model_actual(entry) {
        return None;
    }
    let sid = session_id.or(entry.session_id.as_deref())?;
    let (actual, account) = probe(sid)?;
    match record_model_actual_at(path, worker_id, sid, &actual, account) {
        Ok(Some(obs)) => {
            publish_observation(&obs);
            Some(obs)
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("warning: worker レジストリへ実際のモデルを書けない: {e}");
            None
        }
    }
}

/// 送達フロー（PromptFlow）の結果をレジストリへ記録する（Issue #530 / #1294）。
///
/// `confidence` は 3 値（[`tako_core::prompt_delivery::Confidence`]）:
/// 届いた / 未達確定 / **送ったかもしれない**。旧実装の bool では 3 つ目が
/// 未達へ潰れ、supervisor の自動再送が「届いたかもしれない依頼」を撃っていた（#1294）。
///
/// **`Undelivered` と `Unverified` の記録の形は同じ**（未確認時刻 + 顛末コード）で、
/// どちらだったかは**顛末コードが持つ**（正本は
/// [`tako_core::prompt_delivery::outcome_confidence`]）。読み出し側の
/// [`prompt_delivery_assessment_with`] が同じ表を引き直すので、記録と判定が食い違わない。
///
/// pane 番号で引く（PromptFlow が持つキー）。同番号ペインの再利用による誤更新を防ぐため、
/// active かつ既に決着（delivered / failed）していないエントリだけを対象にする。
/// 記録失敗で送達フローを止めないよう、呼び出し側は警告のみで継続する
pub fn record_prompt_delivery(
    pane: u64,
    flow: PromptDeliveryFlow,
    confidence: Confidence,
    reason: &str,
) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    record_prompt_delivery_at(&path, pane, flow, confidence, reason)
}

fn record_prompt_delivery_at(
    path: &Path,
    pane: u64,
    flow: PromptDeliveryFlow,
    confidence: Confidence,
    reason: &str,
) -> Result<(), String> {
    if flow != PromptDeliveryFlow::SpawnPrompt {
        return Ok(());
    }
    let verified = confidence == Confidence::Delivered;
    if !path.is_file() {
        return Ok(());
    }
    // 変更が無いなら書き込みをスキップ（冪等。record_session_detected と同型）
    let current = WorkerRegistry::load_from(path)?;
    let needs_update = current.workers.values().any(|e| {
        e.is_active()
            && e.pane == pane
            && e.prompt_delivery_failed_at.is_none()
            && (!verified || e.prompt_delivered_at.is_none())
    });
    if !needs_update {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(path, |reg| {
        for entry in reg.workers.values_mut() {
            if !entry.is_active() || entry.pane != pane || entry.prompt_delivery_failed_at.is_some()
            {
                continue;
            }
            if verified {
                if entry.prompt_delivered_at.is_none() {
                    entry.prompt_delivered_at = Some(now.clone());
                }
            } else {
                entry.prompt_delivery_failed_at = Some(now.clone());
                entry.prompt_delivery_failure = Some(reason.to_string());
            }
        }
    })
}

/// 未達 worker へプロンプトを送り直すコマンド（Issue #530 / #390 の再送導線）。
/// prompt 本文は tako 側に残っていないため（規約により保存しない）、master が
/// 同じ依頼文を渡し直す前提のテンプレートを返す
pub fn resend_command(entry: &WorkerEntry) -> Option<String> {
    if !entry.is_active() {
        return None;
    }
    Some(format!("tako send --pane {} '<同じ依頼文>'", entry.pane))
}

/// 突然死からの復旧コマンド（#390: SIGSEGV 等の突然死からの復旧提示）。claude の worker で
/// session ID を検出済みのときだけ出す（`--resume` の互換が確認できているのは claude）。
/// master / supervisor はこの 1 行を**死んだペインのシェルへ送る**か、新しいペインで実行する。
///
/// #2470: 案内は `tako orchestrator resume <worker_id>` の 1 行で、**起動コマンドは打った時点で**
/// spawn と同じ組み立て（`resume_launch::registry_resume_launch` = `build_worker_cmd_in` に会話 ID を
/// 渡す 1 実装）で組む。照会（worker_status / watch / workers）のたびに組むと、spawn と同じ判断のために
/// user scope の `.claude.json`（1 本 260〜350 KB × アカウント数）を読むことになるので、ここは
/// **ファイルを 1 つも読まない純関数**に保つ（番犬 `issue2470_resume_command_watchdog`）。
/// #2470 前は `cd <cwd> && claude --model … --effort … --resume <id>` を組んでいて、#2412 の
/// `--settings`（user scope の MCP を止める）・プロファイルの args（`--permission-mode auto`）と env・
/// `TAKO_ORCHESTRATOR_ROLE` が落ちていた。A/B は `TAKO_2470_LEGACY=1`
pub fn resume_command(worker_id: &str, entry: &WorkerEntry) -> Option<String> {
    if legacy_2470() {
        return legacy_resume_command(entry);
    }
    if entry.agent != "claude" || entry.session_id.is_none() || worker_id.is_empty() {
        return None;
    }
    Some(format!("tako orchestrator resume {worker_id}"))
}

/// #2470 の A/B。`TAKO_2470_LEGACY=1` で**同一バイナリのまま**復旧の案内を修正前の
/// `cd <cwd> && claude --model … --effort … --resume <id>` へ戻す
/// （`tako orchestrator resume` もこの 1 行をそのままシェルで走らせる）
pub fn legacy_2470() -> bool {
    std::env::var("TAKO_2470_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 修正前（#2470 前）の復旧コマンド。照会のたびに会話の記録の所在を探し、会話が既定以外の
/// config ディレクトリにあれば `CLAUDE_CONFIG_DIR` を前置する（#652）
pub fn legacy_resume_command(entry: &WorkerEntry) -> Option<String> {
    let env_prefix = entry
        .session_id
        .as_deref()
        .filter(|_| entry.agent == "claude")
        .and_then(crate::transcript::resume_env_prefix);
    resume_command_with_env(entry, env_prefix.as_deref())
}

/// `legacy_resume_command` の本体（env プレフィクスを引数で受け取るテスト可能版）
fn resume_command_with_env(entry: &WorkerEntry, env_prefix: Option<&str>) -> Option<String> {
    if entry.agent != "claude" {
        return None;
    }
    let sid = entry.session_id.as_deref()?;
    let mut cmd = String::new();
    if let Some(prefix) = env_prefix {
        cmd.push_str(prefix);
    }
    if let Some(cwd) = entry.cwd.as_deref().filter(|c| !c.is_empty()) {
        cmd.push_str(&format!("cd '{}' && ", cwd.replace('\'', "'\\''")));
    }
    cmd.push_str("claude");
    if let Some(model) = entry.model.as_deref().filter(|m| !m.is_empty()) {
        cmd.push_str(&format!(" --model {model}"));
    }
    if let Some(effort) = entry.effort.as_deref().filter(|e| !e.is_empty()) {
        cmd.push_str(&format!(" --effort {effort}"));
    }
    cmd.push_str(&format!(" --resume {sid}"));
    Some(cmd)
}

/// エントリの生存観測（Issue #658）。一覧表示と GC が**同じ規則**を使うための単一定義。
/// `live_backends` は現存する器（tmux / psmux）のセッション名、`live_panes` は GUI に
/// 現存する（ペイン ID, backend セッション名）の組。返り値は (pane_alive, tmux_alive)
pub fn liveness(
    entry: &WorkerEntry,
    live_backends: &[String],
    live_panes: &[(u64, Option<String>)],
) -> (bool, bool) {
    let tmux_alive = entry
        .tmux_session
        .as_deref()
        .is_some_and(|ts| live_backends.iter().any(|b| b == ts));
    let pane_alive = live_panes.iter().any(|(pid, backend)| {
        *pid == entry.pane
            && match entry.tmux_session.as_deref() {
                // 追跡キーがあれば backend の一致で同一性を確認
                Some(expect) => backend.as_deref() == Some(expect),
                // キーが無い（器なし spawn）場合は番号のみで判定
                None => true,
            }
    });
    (pane_alive, tmux_alive)
}

/// GC（`sweep_dead`）の計画。ファイルに触らない純粋な判定結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepPlan {
    /// 初めて「死んで見えた」ので `dead_since` を刻むだけのエントリ
    pub mark: Vec<String>,
    /// 死んだ状態が `DEAD_CONFIRM_SECS` 続いたので closed へ倒すエントリ
    pub close: Vec<String>,
    /// 生存が再観測されたので `dead_since` を消すエントリ
    pub revive: Vec<String>,
}

impl SweepPlan {
    pub fn is_empty(&self) -> bool {
        self.mark.is_empty() && self.close.is_empty() && self.revive.is_empty()
    }
}

/// 死んだ active エントリの掃除計画を立てる（Issue #658。純粋関数 = テスト可能）。
///
/// ペインも器も観測できない active エントリを「死んで見えた」とし、その状態が
/// `DEAD_CONFIRM_SECS` を超えて続いたものだけを closed へ倒す。**1 回の観測では倒さない**
/// ので、器の列挙が一時的に失敗した・アプリ再起動直後でペインがまだ揃っていない、
/// といった過渡状態で生きている worker を落とすことがない
pub fn plan_sweep(
    registry: &WorkerRegistry,
    live_backends: &[String],
    live_panes: &[(u64, Option<String>)],
    now_epoch: i64,
) -> SweepPlan {
    let mut plan = SweepPlan::default();
    for (id, entry) in &registry.workers {
        if !entry.is_active() {
            continue;
        }
        let (pane_alive, tmux_alive) = liveness(entry, live_backends, live_panes);
        if pane_alive || tmux_alive {
            // 生きて見えた。過渡的に死んで見えていた記録は取り消す
            if entry.dead_since.is_some() {
                plan.revive.push(id.clone());
            }
            continue;
        }
        let Some(first_dead) = entry
            .dead_since
            .as_deref()
            .and_then(crate::sessions::parse_iso)
        else {
            // 初観測（または時刻が読めない）→ 刻むだけ。倒すのは次以降の観測
            plan.mark.push(id.clone());
            continue;
        };
        if now_epoch - first_dead >= DEAD_CONFIRM_SECS {
            plan.close.push(id.clone());
        }
    }
    plan
}

/// 死んだ active エントリを closed へ倒し、掃除後のレジストリを返す（Issue #658）。
/// 変更が無ければ**書き込まない**（一覧のたびにファイルを書き換えない）
pub fn sweep_dead_at(
    path: &Path,
    live_backends: &[String],
    live_panes: &[(u64, Option<String>)],
) -> Result<WorkerRegistry, String> {
    let registry = WorkerRegistry::load_from(path)?;
    let now = crate::sessions::now_iso();
    let now_epoch = crate::sessions::parse_iso(&now).unwrap_or(0);
    if plan_sweep(&registry, live_backends, live_panes, now_epoch).is_empty() {
        return Ok(registry);
    }
    // 計画はロックの内側で立て直す（読み取り後に他プロセスが更新していても勝たない）
    WorkerRegistry::mutate_at(path, |reg| {
        let plan = plan_sweep(reg, live_backends, live_panes, now_epoch);
        for id in &plan.mark {
            if let Some(e) = reg.workers.get_mut(id) {
                e.dead_since = Some(now.clone());
            }
        }
        for id in &plan.revive {
            if let Some(e) = reg.workers.get_mut(id) {
                e.dead_since = None;
            }
        }
        for id in &plan.close {
            if let Some(e) = reg.workers.get_mut(id) {
                e.status = "closed".into();
                e.closed_at = Some(now.clone());
                e.close_reason = Some("gone".into());
                e.dead_since = None;
            }
        }
        reg.clone()
    })
}

/// `sweep_dead_at` の既定パス版
pub fn sweep_dead(
    live_backends: &[String],
    live_panes: &[(u64, Option<String>)],
) -> Result<WorkerRegistry, String> {
    let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
    if !path.is_file() {
        return Ok(WorkerRegistry::default());
    }
    sweep_dead_at(&path, live_backends, live_panes)
}

/// レジストリだけでは分からない「送達の一次シグナル」の観測結果（#983 の変更 2）。
///
/// レジストリはファイルなので I/O を伴う観測（codex の rollout を読む等）はできない。
/// **観測は呼び出し側（dispatch の `worker_status`）が行い、結果をここへ渡す**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliveryEvidence {
    /// この系統の一次シグナルが「プロンプトが届いてターンが走った」と示している。
    ///
    /// **claude の session_id はここに入れてはいけない**: セッションの存在は
    /// 「claude が起動した」証拠であって「プロンプトが届いた」証拠ではない（#530）。
    /// 入るのは codex の `task_started` のように**投入されてはじめて起きる**事象だけ
    pub turn_observed: bool,
    /// この系統の一次シグナルを**読もうとして読めなかった**（Issue #1015）。
    ///
    /// 「読めた上でターンが 1 件も無い」と「読めていない」は**別物**なのに、
    /// 旧実装はどちらも猶予超過で `OverdueSuspect`（= `prompt_undelivered` +
    /// 自動再送）へ倒していた。codex の裏取りは rollout（`$CODEX_HOME/sessions/`）
    /// でしかできないので、thread が解決できない / rollout が読めない状況では
    /// **働いている worker へ再送を撃つ**（#1015 の二重指示事故）。
    ///
    /// **既定 `false` = 従来どおり**。claude の一次シグナルは `claude agents --json`
    /// 側なのでここは触らない（#390 の判定は不変）
    pub primary_signal_unreadable: bool,
}

/// 送達フローが残した顛末コードから 3 値目を引き直す（Issue #1294）。
///
/// `legacy = true` は **#1294 前**の挙動（何であれ未達と断定する）。A/B の入口で、
/// 製品経路では `tako_core::prompt_delivery::legacy_undelivered()` が渡る。
/// 顛末コードが無い古い記録は**未達側**へ倒す（従来どおり = 救済を減らさない）
fn failure_assessment(reason: Option<&str>, legacy: bool) -> PromptDelivery {
    use tako_core::prompt_delivery::{outcome_confidence, Confidence};
    if legacy {
        return PromptDelivery::OverdueSuspect;
    }
    match reason.map(outcome_confidence) {
        Some(Confidence::Unverified) => PromptDelivery::Unverified,
        _ => PromptDelivery::OverdueSuspect,
    }
}

/// prompt 送達状態を判定する（Issue #390 要件 4 / #983 の変更 2）。
/// OverdueSuspect は「疑い」であり、最終的な未達イベントの発火は呼び出し側が
/// 画面状態（busy でない・実行中子プロセスなし）と組み合わせて決める
pub fn prompt_delivery_assessment(entry: &WorkerEntry, now_epoch: i64) -> PromptDelivery {
    prompt_delivery_assessment_with(entry, now_epoch, DeliveryEvidence::default())
}

/// 一次シグナルの観測結果つきの判定（#983 の変更 2）。
///
/// 旧実装は `entry.agent != "claude"` を `NotApplicable` で即返していた。
/// 誤検知を避ける判断としては妥当だったが、**結果として「観測手段が無い =
/// 何も言わない」**になっていた（#983 の無言死）。ここでは
/// [`tako_core::agent_support::delivery_observation`] を引いて、
/// 一次シグナルのある系統だけが未達を断定し、無い系統は
/// [`PromptDelivery::Unverified`]（= 未確認）を返す
pub fn prompt_delivery_assessment_with(
    entry: &WorkerEntry,
    now_epoch: i64,
    evidence: DeliveryEvidence,
) -> PromptDelivery {
    use tako_core::agent_support::{self, Agent, DeliveryObservation};

    // ターンが走った = プロンプトが届いた。**画面検証の失敗より強い証拠**なので
    // 最優先する（#1015: 画面確認が失敗と記録した codex worker が、実際には
    // 依頼を読んで作業していたのに永久に undelivered のままだった）
    if evidence.turn_observed && !super::agent_cli::legacy_mode() {
        return PromptDelivery::Delivered;
    }
    // 送達フローが確認できずに決着していれば、それを次に優先する（Issue #530）。
    // session 検出は「claude が起動した」証拠であって「プロンプトが届いた」証拠ではない
    // （初回のテーマ選択・ログイン方法選択ダイアログにプロンプトが食われても
    // claude 自体は起動するため session_id は付く = 旧実装の delivered 偽陽性）。
    //
    // #1294: **未達確定か「送ったかもしれない」か**は顛末コードが持つ
    // （正本は `tako_core::prompt_delivery::outcome_confidence`）。後者を
    // `OverdueSuspect` にすると `prompt_undelivered` → supervisor の自動再送が撃たれ、
    // peer の受信箱へ既に入っている依頼をもう一度渡す（#790 / #1015 の二重投函）
    if entry.prompt_delivery_failed_at.is_some() {
        return failure_assessment(
            entry.prompt_delivery_failure.as_deref(),
            tako_core::prompt_delivery::legacy_undelivered(),
        );
    }
    // #1940: `delivered` は**依頼文の到達を確かめた証拠**があるときだけ返す。
    // `prompt_delivered_at` を書くのは送達フローの確認（入力欄への反映 + 送信後の空、
    // または peer の受信確認）だけ。会話の検出（session_id）は「起動した」証拠に留める
    // （旧実装はこれを到達とみなし、届く前に delivered → 諦めた後に undelivered と反転した）
    let legacy_1940 = tako_core::shell_send::legacy_1940();
    if entry.prompt_delivered_at.is_some() || (legacy_1940 && entry.session_id.is_some()) {
        return PromptDelivery::Delivered;
    }
    if !entry.is_active() {
        return PromptDelivery::NotApplicable;
    }
    // #2413: メモリの逼迫で起動を待たせている = agent がまだ居ない。待った分を猶予に数えない
    // （起こしたら `record_launch_started` が `spawned_at` を起こした時刻へ直す）
    if crate::launch_gate::is_waiting(entry.pane) {
        return PromptDelivery::Pending;
    }
    let Some(spawned) = crate::sessions::parse_iso(&entry.spawned_at) else {
        return PromptDelivery::NotApplicable;
    };
    // #983 の A/B: 旧挙動は「claude 以外は何も言わない」
    if super::agent_cli::legacy_mode() && entry.agent != "claude" {
        return PromptDelivery::NotApplicable;
    }
    if now_epoch - spawned <= PROMPT_DELIVERY_GRACE_SECS {
        return PromptDelivery::Pending;
    }
    // #1940: 起動は確認できた（会話を検出した）が、到達の証拠が無いまま猶予を過ぎた。
    // 送達フローが失われた（GUI の再起動等）可能性があるので未達とは断定しない
    // （断定すると supervisor の自動再送が「届いたかもしれない依頼」を撃つ = #1294）
    if entry.session_id.is_some() {
        return PromptDelivery::Unverified;
    }
    // 猶予超過。**何をもって未達と言えるか**は系統ごとに違う（マトリクスが正本）。
    // 知らない agent 名は「観測手段が無い」側へ倒す（黙らず、断定もしない）。
    //
    // #1015: 一次シグナルを持つ系統でも、**そのシグナルを実際に読めていなければ
    // 未達と断定しない**。「読めて 0 ターン」だけが未達の証拠で、「読めなかった」は
    // 未確認（`verify_then_resend`）に留める。断定すると supervisor の自動再送が走り、
    // 実際には働いている worker へ同じ依頼が二度渡る
    match Agent::parse(&entry.agent).map(agent_support::delivery_observation) {
        Some(DeliveryObservation::Structured) if !evidence.primary_signal_unreadable => {
            PromptDelivery::OverdueSuspect
        }
        _ => PromptDelivery::Unverified,
    }
}

/// workers 一覧の JSON ペイロードを組み立てる。
/// `live_backends` は現存する tmux セッション名（呼び出し側が 1 コマンドで列挙）、
/// `live_panes` は GUI に現存する（ペイン ID, backend セッション名）の組（tree + shelved）。
/// backend は pane ID 再利用の同一性検証に使う: エントリが tmux_session を持つ場合、
/// 同番号ペインの backend が一致しなければ「別物」= pane_alive にしない（#390。
/// 復元なし再起動では新プロセスが同じ番号を別ペインへ振るため）。
/// `include_closed` = false なら active のみ返す
pub fn list_payload(
    registry: &WorkerRegistry,
    live_backends: &[String],
    live_panes: &[(u64, Option<String>)],
    limit_resume_panes: &[u64],
    include_closed: bool,
) -> Value {
    let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap_or(0);
    let mut items: Vec<Value> = Vec::new();
    // #1638: 宣言と実際のモデルの食い違い（worker ごとに 1 行。空 = 何も無い）
    let mut warnings: Vec<String> = Vec::new();
    let mut entries: Vec<(&String, &WorkerEntry)> = registry
        .workers
        .iter()
        .filter(|(_, e)| include_closed || e.is_active())
        .collect();
    // 新しい順（spawned_at 降順）
    entries.sort_by(|a, b| b.1.spawned_at.cmp(&a.1.spawned_at));
    for (id, e) in entries {
        let (pane_alive, tmux_alive) = liveness(e, live_backends, live_panes);
        let delivery = prompt_delivery_assessment(e, now_epoch);
        if let Some(line) = model_warning(e) {
            warnings.push(format!("worker {id}（pane {}）: {line}", e.pane));
        }
        items.push(json!({
            "worker_id": id,
            "label": e.label,
            "project": e.project,
            "agent": e.agent,
            // #1638: `model` は従来どおり宣言（読み手の互換）。宣言 / 実際を分けたのが
            // `model_declared` / `model_actual`（未確定は null）、突き合わせが `model_check`
            "model": e.model,
            "model_declared": e.model,
            "model_actual": e.model_actual,
            "model_check": model_check(e).as_str(),
            "effort": e.effort,
            "pane": e.pane,
            "tab": e.tab,
            "tmux_session": e.tmux_session,
            "session_id": e.session_id,
            "issues": e.issues,
            "ledger_id": e.ledger_id,
            "cwd": e.cwd,
            "prompt_head": e.prompt_head,
            "spawned_at": e.spawned_at,
            "status": e.status,
            "closed_at": e.closed_at,
            "close_reason": e.close_reason,
            // #658: 「ペインも器も見えない」と最初に観測した時刻（GC の確認期間の起点）
            "dead_since": e.dead_since,
            "pane_alive": pane_alive,
            "tmux_alive": tmux_alive,
            "prompt_delivery": delivery.as_str(),
            "prompt_delivery_failure": e.prompt_delivery_failure,
            // #1940: 起動の状態（failed / started / pending）と失敗の理由コード。
            // spawn は即返るので、起動の失敗はここで初めて見える
            "launch": if e.is_active() && crate::launch_gate::is_waiting(e.pane) {
                "waiting"
            } else {
                launch_state(e)
            },
            // #2413: メモリの逼迫で起動を待たせている（理由・順番・上書きの案内）。待っていなければ null
            "launch_gate": e.is_active().then(|| crate::launch_gate::view(e.pane)).flatten(),
            "launch_failure": e.launch_failure,
            "launch_exit_code": e.launch_exit_code,
            "launch_failed_at": e.launch_failed_at,
            "resume_command": resume_command(id, e),
            // #822: 利用上限後の自動復帰（FR-2.27）が有効か。ペインが生きていないときは
            // ペイン属性を読めない（番号が別ペインへ再利用されている可能性もある）ので null
            "limit_resume": pane_alive.then(|| limit_resume_panes.contains(&e.pane)),
            // 未達 worker にだけ再送コマンドを出す（#530 の受け入れ条件 3）。
            // #1940: 起動に失敗した worker へ依頼文を再送しても届かない（エージェントが
            // 居ない）ので出さない。次の一手は起動のやり直し
            "resend_command": match delivery {
                PromptDelivery::OverdueSuspect if e.launch_failed_at.is_none() => {
                    resend_command(e)
                }
                _ => None,
            },
        }));
    }
    json!({ "workers": items, "count": items.len(), "warnings": warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト 1 本ぶんの workers.yaml。置き場はプロセスごとの使い捨ての親の下で、
    /// 終了時に親ごと消える（#1312。以前は pid の付かない固定名
    /// `tako-registry-tests` を並行する別プロセスと共有していた）
    fn temp_registry_file(name: &str) -> PathBuf {
        let dir = tako_core::test_residue::process_scratch("registry-tests");
        let path = dir.join(format!("{name}-{}.yaml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// #1968: 読み取りの使い回しは中身で比べる。同じ長さの書き直し（更新時刻や inode では
    /// 取り違えうる形）でも、書き込みの後でも、古い解釈結果を返さない
    #[test]
    fn 読み取りの使い回しは中身が変われば読み直す_1968() {
        let path = temp_registry_file("load-cache-1968");
        let yaml = |pane: u64| {
            format!("workers:\n  '1':\n    pane: {pane}\n    status: active\nnext_id: 2\n")
        };
        std::fs::write(&path, yaml(11)).unwrap();
        let a = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(a.workers["1"].pane, 11);
        // 同じ中身 = 使い回し（結果は同じ）
        let again = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(again.workers["1"].pane, 11);
        // 同じ長さで中身だけ変える（11 → 22。rename ではない上書き）
        std::fs::write(&path, yaml(22)).unwrap();
        assert_eq!(yaml(11).len(), yaml(22).len());
        let b = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(b.workers["1"].pane, 22, "同じ長さの書き直しを取り違えた");
        // 書き込み経路（mutate_at）の後も新しい中身
        WorkerRegistry::mutate_at(&path, |r| {
            r.workers.get_mut("1").unwrap().pane = 33;
        })
        .unwrap();
        let c = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(
            c.workers["1"].pane, 33,
            "書き込みの後に古い解釈結果を返した"
        );
        // 壊れた中身は使い回しで隠さない（#169: パース失敗は Err）
        std::fs::write(&path, "workers: [\n").unwrap();
        assert!(WorkerRegistry::load_from(&path).is_err());
    }

    fn sample_record(pane: u64) -> RegisterSpawn {
        RegisterSpawn {
            label: Some("fix-390".into()),
            project: "tako".into(),
            agent: "claude".into(),
            model: None,
            effort: Some("high".into()),
            pane,
            tab: Some(3),
            tmux_session: Some(format!("tako-pane-{pane}")),
            issues: vec![390],
            ledger_id: Some("L1".into()),
            cwd: Some("/tmp/proj".into()),
            prompt_head: Some("Issue #390: ...".into()),
            profile: None,
        }
    }

    /// mutate_at で直接登録するテスト用ヘルパー（env 非依存）
    fn register_at(path: &Path, record: RegisterSpawn) -> String {
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.pane == record.pane && entry.is_active() {
                    entry.status = "closed".into();
                    entry.closed_at = Some(now.clone());
                    entry.close_reason = Some("superseded".into());
                }
            }
            reg.next_id += 1;
            let id = reg.next_id.to_string();
            reg.workers.insert(
                id.clone(),
                WorkerEntry {
                    label: record.label.clone(),
                    project: record.project.clone(),
                    agent: record.agent.clone(),
                    model: record.model.clone(),
                    model_actual: None,
                    model_actual_at: None,
                    model_actual_account: None,
                    effort: record.effort.clone(),
                    pane: record.pane,
                    tab: record.tab,
                    tmux_session: record.tmux_session.clone(),
                    session_id: None,
                    issues: record.issues.clone(),
                    ledger_id: record.ledger_id.clone(),
                    cwd: record.cwd.clone(),
                    prompt_head: record.prompt_head.clone(),
                    spawned_at: now.clone(),
                    status: "active".into(),
                    closed_at: None,
                    close_reason: None,
                    prompt_delivered_at: None,
                    dead_since: None,
                    prompt_delivery_failed_at: None,
                    prompt_delivery_failure: None,
                    launch_failed_at: None,
                    launch_failure: None,
                    launch_exit_code: None,
                    profile: record.profile.clone(),
                },
            );
            reg.gc();
            id
        })
        .unwrap()
    }

    #[test]
    fn 登録と解決の往復ができる() {
        let path = temp_registry_file("roundtrip");
        let id = register_at(&path, sample_record(42));
        assert_eq!(id, "1");
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (rid, entry) = reg.resolve("1").unwrap();
        assert_eq!(rid, "1");
        assert_eq!(entry.pane, 42);
        assert_eq!(entry.project, "tako");
        assert_eq!(entry.tmux_session.as_deref(), Some("tako-pane-42"));
        assert!(entry.is_active());
        assert_eq!(entry.issues, vec![390]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn find_active_by_paneが引ける() {
        let path = temp_registry_file("bypane");
        register_at(&path, sample_record(10));
        register_at(&path, sample_record(20));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(20).unwrap();
        assert_eq!(entry.pane, 20);
        assert!(reg.find_active_by_pane(99).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 同一paneの再spawnは旧エントリをsupersededに倒す() {
        let path = temp_registry_file("supersede");
        let first = register_at(&path, sample_record(7));
        let second = register_at(&path, sample_record(7));
        assert_ne!(first, second);
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let old = &reg.workers[&first];
        assert_eq!(old.status, "closed");
        assert_eq!(old.close_reason.as_deref(), Some("superseded"));
        // active は新エントリだけ
        let (aid, _) = reg.find_active_by_pane(7).unwrap();
        assert_eq!(aid, &second);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mark_closedで明示closeが記録される() {
        let path = temp_registry_file("close");
        register_at(&path, sample_record(5));
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(&path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.pane == 5 && entry.is_active() {
                    entry.status = "closed".into();
                    entry.closed_at = Some(now.clone());
                    entry.close_reason = Some("explicit_close".into());
                }
            }
        })
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let entry = reg.workers.values().next().unwrap();
        assert_eq!(entry.status, "closed");
        assert_eq!(entry.close_reason.as_deref(), Some("explicit_close"));
        assert!(entry.closed_at.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 破損ファイルはerrで0件に丸めない() {
        let path = temp_registry_file("corrupt");
        std::fs::write(&path, "workers: [this is: not valid").unwrap();
        assert!(WorkerRegistry::load_from(&path).is_err());
        // mutate も Err（黙って空で上書きしない = #169 と同思想）
        let result = WorkerRegistry::mutate_at(&path, |_| ());
        assert!(result.is_err());
        // 破損ファイルはそのまま残る（bak からの復旧余地を消さない）
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("not valid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn session検出でsession_idとprompt_delivered_atが埋まる() {
        let path = temp_registry_file("detect");
        register_at(&path, sample_record(8));
        let now = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(&path, |reg| {
            for entry in reg.workers.values_mut() {
                if entry.is_active() && entry.tmux_session.as_deref() == Some("tako-pane-8") {
                    entry.session_id = Some("abc-123".into());
                    if entry.prompt_delivered_at.is_none() {
                        entry.prompt_delivered_at = Some(now.clone());
                    }
                }
            }
        })
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let entry = reg.workers.values().next().unwrap();
        assert_eq!(entry.session_id.as_deref(), Some("abc-123"));
        assert!(entry.prompt_delivered_at.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prompt_delivery_assessmentの分岐() {
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let mut entry = WorkerEntry {
            agent: "claude".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        // 猶予内 = pending
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch),
            PromptDelivery::Pending
        );
        // 猶予超過 = 未達疑い
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + PROMPT_DELIVERY_GRACE_SECS + 10),
            PromptDelivery::OverdueSuspect
        );
        // #1940: session_id 検出済み（= 起動した）だけでは delivered にしない。
        // 猶予内は pending、猶予を過ぎて到達の記録が無ければ未確認
        entry.session_id = Some("abc".into());
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch),
            PromptDelivery::Pending
        );
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + 10_000),
            PromptDelivery::Unverified
        );
        // 送達フローが到達を確認した = delivered（時間に関係なく）
        entry.prompt_delivered_at = Some(crate::sessions::now_iso());
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + 10_000),
            PromptDelivery::Delivered
        );
        // #983: claude 以外も黙らない。codex は一次シグナル（rollout）を持つので
        // claude と同じく未達を断定する
        let codex = WorkerEntry {
            agent: "codex".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&codex, now_epoch + 10_000),
            PromptDelivery::OverdueSuspect
        );
        // closed は対象外
        let closed = WorkerEntry {
            agent: "claude".into(),
            status: "closed".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&closed, now_epoch + 10_000),
            PromptDelivery::NotApplicable
        );
    }

    #[test]
    fn 一次シグナルの無い系統は未達と断定せず未確認を返す() {
        // #983 の受け入れ条件 5。旧実装は agent != "claude" を NotApplicable で
        // 即返していた = **何も言わない**。一次シグナルを持たない系統は
        // 「未達」と断定はしないが「未確認」とは言う。
        // #1033 で agy が実況 JSONL を得たので、この役はローカル LLM が担う
        // （ハーネス未定 = 観測手段が決まっていない唯一の系統）
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let agy = WorkerEntry {
            agent: "local".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&agy, now_epoch),
            PromptDelivery::Pending,
            "猶予内は claude と同じ"
        );
        let after = prompt_delivery_assessment(&agy, now_epoch + PROMPT_DELIVERY_GRACE_SECS + 10);
        assert_eq!(after, PromptDelivery::Unverified);
        assert_ne!(after, PromptDelivery::NotApplicable, "黙らないこと");
        assert_ne!(
            after,
            PromptDelivery::OverdueSuspect,
            "断定もしないこと（自動再送を撃たせない）"
        );
        assert_eq!(after.as_str(), "unverified");

        // 知らない agent 名も「観測手段が無い」側へ倒す（黙らず、断定もしない）
        let unknown = WorkerEntry {
            agent: "someagent".into(),
            ..agy.clone()
        };
        assert_eq!(
            prompt_delivery_assessment(&unknown, now_epoch + PROMPT_DELIVERY_GRACE_SECS + 10),
            PromptDelivery::Unverified
        );
    }

    #[test]
    fn ターンが走った証拠は画面検証の失敗より強い() {
        // #1015: 画面の送達確認が失敗と記録した codex worker が、実際には依頼を
        // 読んで作業していたのに永久に undelivered のままだった。
        // rollout の task_started は「投入されたプロンプトでしか起きない」ので、
        // これが観測できたら delivered と言い切ってよい
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let entry = WorkerEntry {
            agent: "codex".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            prompt_delivery_failed_at: Some(crate::sessions::now_iso()),
            prompt_delivery_failure: Some("flow_timeout".into()),
            ..Default::default()
        };
        // 一次シグナルが無ければ従来どおり未達（#530 の判断は変えない）
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + 10_000),
            PromptDelivery::OverdueSuspect
        );
        // ターンを観測できたら delivered へ覆る
        assert_eq!(
            prompt_delivery_assessment_with(
                &entry,
                now_epoch + 10_000,
                DeliveryEvidence {
                    turn_observed: true,
                    ..Default::default()
                }
            ),
            PromptDelivery::Delivered
        );
    }

    #[test]
    fn i1015_一次シグナルを読めていないなら未達と断定しない() {
        // #1015 の受け入れ条件 6: `prompt_undelivered`（= 自動再送のトリガ）は
        // 「rollout を読めて、それでもターンが 1 件も無い」ときだけ出す。
        // rollout が読めない状況（thread が解決できない / ファイルが無い）で断定すると、
        // 実際には働いている worker へ同じ依頼が二度渡る
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let codex = WorkerEntry {
            agent: "codex".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        let overdue = now_epoch + PROMPT_DELIVERY_GRACE_SECS + 10;
        // ① 読めて 0 ターン = 本当に未達（従来どおり断定する）
        assert_eq!(
            prompt_delivery_assessment_with(&codex, overdue, DeliveryEvidence::default()),
            PromptDelivery::OverdueSuspect,
            "読めて 0 ターンなら未達と言ってよい"
        );
        // ② 読めていない = 未確認へ降格（自動再送は撃たれない）
        assert_eq!(
            prompt_delivery_assessment_with(
                &codex,
                overdue,
                DeliveryEvidence {
                    primary_signal_unreadable: true,
                    ..Default::default()
                }
            ),
            PromptDelivery::Unverified,
            "rollout を読めていないのに未達と断定している（#1015）"
        );
        // ③ ターンを観測できたら読めなさに関係なく delivered
        assert_eq!(
            prompt_delivery_assessment_with(
                &codex,
                overdue,
                DeliveryEvidence {
                    turn_observed: true,
                    primary_signal_unreadable: true,
                }
            ),
            PromptDelivery::Delivered
        );
        // ④ 猶予内は読めなさに関係なく pending（降格が猶予判定を食わない）
        assert_eq!(
            prompt_delivery_assessment_with(
                &codex,
                now_epoch,
                DeliveryEvidence {
                    primary_signal_unreadable: true,
                    ..Default::default()
                }
            ),
            PromptDelivery::Pending
        );
        // ⑤ claude は不変（一次シグナルは agents API 側なので #390 の判定を変えない）
        let claude = WorkerEntry {
            agent: "claude".into(),
            ..codex.clone()
        };
        assert_eq!(
            prompt_delivery_assessment(&claude, overdue),
            PromptDelivery::OverdueSuspect
        );
    }

    #[test]
    fn 送達の観測手段はマトリクスから引く() {
        // 判断をこのファイルへ焼き込まない（#982 のマトリクスが正本）。
        // 系統が増えたらマトリクスを直せば追従する、という構造の検査
        use tako_core::agent_support::{delivery_observation, Agent, DeliveryObservation};
        assert_eq!(
            delivery_observation(Agent::Claude),
            DeliveryObservation::Structured
        );
        assert_eq!(
            delivery_observation(Agent::Codex),
            DeliveryObservation::Structured,
            "#984 で rollout を読めるようになった"
        );
        assert_eq!(
            delivery_observation(Agent::Agy),
            DeliveryObservation::Structured,
            "#1033 で実況 JSONL（USER_INPUT）を読めるようになった"
        );
        assert_eq!(
            delivery_observation(Agent::Local),
            DeliveryObservation::ScreenOnly,
            "ハーネス未定なので観測手段が決まっていない"
        );
    }

    #[test]
    fn 送達フローの未達記録はsession検出より優先される() {
        // #530: claude が起動すれば session_id は付く（= 起動の証拠であって
        // プロンプト到達の証拠ではない）。選択ダイアログに食われて未達だった場合、
        // 送達フローが記録した失敗が delivered を上書きする
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let mut entry = WorkerEntry {
            agent: "claude".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            session_id: Some("abc".into()),
            prompt_delivered_at: Some(crate::sessions::now_iso()),
            ..Default::default()
        };
        // 旧実装の判定（session 検出だけで delivered）
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch),
            PromptDelivery::Delivered
        );
        // 送達フローが未達を記録すると undelivered になる
        entry.prompt_delivery_failed_at = Some(crate::sessions::now_iso());
        entry.prompt_delivery_failure = Some("choice_dialog".into());
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch),
            PromptDelivery::OverdueSuspect
        );
        assert_eq!(PromptDelivery::OverdueSuspect.as_str(), "undelivered");
    }

    /// #1940: 本番 10/9 の pane 2471 の順序を再現する。claude が起動して会話が
    /// 検出された時点（依頼文はまだ届いていない）で `delivered` を返し、送達フローが
    /// 諦めた後に `undelivered` へ反転していた。**届く前に delivered を返さない**
    #[test]
    fn issue1940_会話の検出だけでは依頼文の到達とみなさない() {
        let path = temp_registry_file("issue1940-session");
        register_at(&path, sample_record(2471));
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();

        // claude が起動して会話を検出した（`claude agents --json` は起動直後に返す）
        record_session_detected_at(&path, "tako-pane-2471", "sid-2471").unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let e = reg.workers.values().find(|e| e.pane == 2471).unwrap();
        assert_eq!(e.session_id.as_deref(), Some("sid-2471"));
        assert!(
            e.prompt_delivered_at.is_none(),
            "会話の検出は到達の時刻を書かない"
        );
        assert_ne!(
            prompt_delivery_assessment(e, now_epoch),
            PromptDelivery::Delivered,
            "送達フローが確かめる前に delivered を返さない"
        );
        assert_eq!(launch_state(e), "started", "起動したことは言う");

        // 送達フローが諦めた → undelivered（反転ではなく、最初から delivered でない）
        record_prompt_delivery_at(
            &path,
            2471,
            PromptDeliveryFlow::SpawnPrompt,
            Confidence::Undelivered,
            "flow_timeout",
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let e = reg.workers.values().find(|e| e.pane == 2471).unwrap();
        assert_eq!(
            prompt_delivery_assessment(e, now_epoch),
            PromptDelivery::OverdueSuspect
        );

        // 対照: 送達フローが到達を確かめたものだけが delivered
        let ok = temp_registry_file("issue1940-session-ok");
        register_at(&ok, sample_record(2472));
        record_session_detected_at(&ok, "tako-pane-2472", "sid-2472").unwrap();
        record_prompt_delivery_at(
            &ok,
            2472,
            PromptDeliveryFlow::SpawnPrompt,
            Confidence::Delivered,
            tako_core::prompt_delivery::VERIFIED,
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&ok).unwrap();
        let e = reg.workers.values().find(|e| e.pane == 2472).unwrap();
        assert_eq!(
            prompt_delivery_assessment(e, now_epoch),
            PromptDelivery::Delivered
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&ok);
    }

    /// #1940: 起動失敗の 3 欄は serde の default で足したので、欄の無い旧い
    /// `workers.yaml` がそのまま読める（移行は要らない = #916 の指紋の更新理由）
    #[test]
    fn issue1940_起動失敗の欄が無い旧いworkers_yamlをそのまま読める() {
        let old = "next_id: 1\nworkers:\n  '1':\n    project: tako\n    agent: claude\n    \
                   pane: 7\n    spawned_at: '2026-10-01T00:00:00Z'\n    status: active\n";
        let reg: WorkerRegistry = serde_yaml::from_str(old).expect("旧い形が読める");
        let e = &reg.workers["1"];
        assert!(e.launch_failed_at.is_none() && e.launch_failure.is_none());
        assert_eq!(e.launch_exit_code, None);
        assert_eq!(launch_state(e), "pending");
        // 書き戻しても空の欄は出さない（旧い tako が読んでも困らない）
        let back = serde_yaml::to_string(&reg).unwrap();
        assert!(!back.contains("launch_"), "{back}");
    }

    /// #2413: 起動を待たせている worker は、猶予（240 秒）を過ぎても送達を Pending のまま・一覧の
    /// `launch` は `waiting` で理由（`launch_gate`）つき。起こしたら起点（spawned_at）を今へ直すので、
    /// 待った分で「届いていない」（supervisor の自動再送の引き金）を言わない
    #[test]
    fn issue2413_起動待ちの間は送達を保留し_起こしたら起点を直す() {
        use tako_core::queue::MachineView;
        use tako_core::resources::Pressure;
        let path = temp_registry_file("issue2413-gate");
        register_at(&path, sample_record(61));
        register_at(&path, sample_record(62));
        // 古い spawn（猶予を過ぎた）にする
        WorkerRegistry::mutate_at(&path, |reg| {
            for e in reg.workers.values_mut() {
                e.spawned_at = "2026-10-01T00:00:00Z".into();
            }
        })
        .unwrap();
        crate::launch_gate::enable_for_test(
            Some(MachineView {
                total_mb: 36 * 1024,
                available_mb: Some(20 * 1024),
                pressure: Some(Pressure::Critical),
                swap_used_mb: Some(0),
            }),
            std::time::Duration::from_secs(60),
        );
        let held = crate::launch_gate::request(crate::launch_gate::LaunchRequest {
            pane: 61,
            kind: crate::launch_gate::LaunchKind::Spawn,
            agent: "claude".into(),
            label: "x".into(),
            role: None,
            action: crate::launch_gate::LaunchAction::Command {
                command: "claude".into(),
                prompt: None,
            },
        });
        assert!(matches!(held, crate::launch_gate::Decision::Wait(_)));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let now = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        let e61 = reg.workers.values().find(|e| e.pane == 61).unwrap();
        let e62 = reg.workers.values().find(|e| e.pane == 62).unwrap();
        assert_eq!(
            prompt_delivery_assessment(e61, now),
            PromptDelivery::Pending,
            "起動待ちは猶予に数えない"
        );
        assert_ne!(
            prompt_delivery_assessment(e62, now),
            PromptDelivery::Pending,
            "待っていない worker は従来どおり"
        );
        let payload = list_payload(&reg, &[], &[], &[], false);
        let w61 = payload["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["pane"] == 61)
            .unwrap()
            .clone();
        assert_eq!(w61["launch"], "waiting");
        assert_eq!(w61["launch_gate"]["code"], "pressure_critical");

        record_launch_started_at(&path, 61).unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let e61 = reg.workers.values().find(|e| e.pane == 61).unwrap();
        let e62 = reg.workers.values().find(|e| e.pane == 62).unwrap();
        assert_ne!(
            e61.spawned_at, "2026-10-01T00:00:00Z",
            "起点を起こした時刻へ直す"
        );
        assert_eq!(e62.spawned_at, "2026-10-01T00:00:00Z", "別ペインは触らない");
    }

    /// #1940: 起動に失敗した spawn は `workers` に理由コードつきで出る
    /// （spawn は即返るので、ここが無いと worker は成功のまま黙って止まる）
    #[test]
    fn issue1940_起動失敗は一覧に理由つきで出て再送コマンドを出さない() {
        let path = temp_registry_file("issue1940-launch");
        register_at(&path, sample_record(51));
        register_at(&path, sample_record(52));
        record_launch_failure_at(&path, 51, LaunchFailure::AgentExited, Some(1)).unwrap();
        // 2 回目は上書きしない（最初の失敗が正本）
        record_launch_failure_at(&path, 51, LaunchFailure::CommandFlowTimeout, None).unwrap();
        // 送達フローも諦めた（相手が居ない）
        record_prompt_delivery_at(
            &path,
            51,
            PromptDeliveryFlow::SpawnPrompt,
            Confidence::Undelivered,
            tako_core::prompt_delivery::Stall::AgentExited.code(),
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let payload = list_payload(&reg, &[], &[], &[], false);
        let items = payload["workers"].as_array().unwrap();
        let failed = items.iter().find(|w| w["pane"] == 51).unwrap();
        assert_eq!(failed["launch"], "failed");
        assert_eq!(failed["launch_failure"], "agent_exited");
        assert_eq!(failed["launch_exit_code"], 1);
        assert!(failed["launch_failed_at"].is_string());
        assert_eq!(failed["prompt_delivery"], "undelivered");
        assert_eq!(failed["prompt_delivery_failure"], "agent_exited");
        assert!(
            failed["resend_command"].is_null(),
            "エージェントが居ないので依頼文の再送は案内しない"
        );
        let other = items.iter().find(|w| w["pane"] == 52).unwrap();
        assert_eq!(other["launch"], "pending", "別ペインは無傷");
        assert!(other["launch_failure"].is_null());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_prompt_deliveryは同ペインのactiveエントリだけを更新する() {
        let path = temp_registry_file("delivery");
        register_at(&path, sample_record(41));
        // 別ペインの worker（更新対象外）
        register_at(&path, sample_record(42));

        // 未達記録
        WorkerRegistry::mutate_at(&path, |reg| {
            for e in reg.workers.values_mut() {
                if e.pane == 41 {
                    e.prompt_delivery_failed_at = Some(crate::sessions::now_iso());
                    e.prompt_delivery_failure = Some("choice_dialog".into());
                }
            }
        })
        .unwrap();

        let reg = WorkerRegistry::load_from(&path).unwrap();
        let failed = reg.workers.values().find(|e| e.pane == 41).unwrap();
        let other = reg.workers.values().find(|e| e.pane == 42).unwrap();
        assert_eq!(
            failed.prompt_delivery_failure.as_deref(),
            Some("choice_dialog")
        );
        assert!(other.prompt_delivery_failed_at.is_none(), "別ペインは無傷");

        // 未達 worker にだけ再送コマンドが出る
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        assert_eq!(
            prompt_delivery_assessment(failed, now_epoch),
            PromptDelivery::OverdueSuspect
        );
        assert_eq!(
            resend_command(failed).as_deref(),
            Some("tako send --pane 41 '<同じ依頼文>'")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 後続sendの失敗はdelivered済みworkerをundeliveredへ戻さない() {
        let path = temp_registry_file("followup-delivery");
        register_at(&path, sample_record(778));
        let delivered_at = crate::sessions::now_iso();
        WorkerRegistry::mutate_at(&path, |reg| {
            let entry = reg
                .workers
                .values_mut()
                .find(|entry| entry.pane == 778 && entry.is_active())
                .unwrap();
            entry.session_id = Some("session-778".into());
            entry.prompt_delivered_at = Some(delivered_at);
        })
        .unwrap();

        record_prompt_delivery_at(
            &path,
            778,
            PromptDeliveryFlow::FollowUpSend,
            Confidence::Undelivered,
            "flow_timeout",
        )
        .unwrap();

        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(778).unwrap();
        assert!(entry.prompt_delivery_failed_at.is_none());
        assert!(entry.prompt_delivery_failure.is_none());
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        assert_eq!(
            prompt_delivery_assessment(entry, now_epoch),
            PromptDelivery::Delivered
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn spawn初回ダイアログの失敗はsession検出済みでもundeliveredになる() {
        let path = temp_registry_file("spawn-dialog-failure");
        register_at(&path, sample_record(530));
        WorkerRegistry::mutate_at(&path, |reg| {
            let entry = reg
                .workers
                .values_mut()
                .find(|entry| entry.pane == 530 && entry.is_active())
                .unwrap();
            entry.session_id = Some("session-530".into());
            entry.prompt_delivered_at = Some(crate::sessions::now_iso());
        })
        .unwrap();

        record_prompt_delivery_at(
            &path,
            530,
            PromptDeliveryFlow::SpawnPrompt,
            Confidence::Undelivered,
            "choice_dialog",
        )
        .unwrap();

        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(530).unwrap();
        assert_eq!(
            entry.prompt_delivery_failure.as_deref(),
            Some("choice_dialog")
        );
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        assert_eq!(
            prompt_delivery_assessment(entry, now_epoch),
            PromptDelivery::OverdueSuspect
        );
        let _ = std::fs::remove_file(&path);
    }

    /// #1294: peer の書き込みが始まった後に確認が取れなかった顛末は、
    /// レジストリでも**未達と断定しない**（断定すると supervisor が自動再送 = 二重投函）
    #[test]
    fn 送ったかもしれない顛末はundeliveredにならない() {
        for reason in [
            tako_core::prompt_delivery::Stall::PeerSendStalled.code(),
            tako_core::prompt_delivery::PEER_UNCONFIRMED,
        ] {
            let path = temp_registry_file(&format!("unverified-{reason}"));
            register_at(&path, sample_record(1294));
            record_prompt_delivery_at(
                &path,
                1294,
                PromptDeliveryFlow::SpawnPrompt,
                Confidence::Unverified,
                reason,
            )
            .unwrap();

            let reg = WorkerRegistry::load_from(&path).unwrap();
            let (_, entry) = reg.find_active_by_pane(1294).unwrap();
            // 顛末は記録される（無音にはしない）
            assert_eq!(entry.prompt_delivery_failure.as_deref(), Some(reason));
            assert!(entry.prompt_delivery_failed_at.is_some());
            let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
            assert_eq!(
                prompt_delivery_assessment(entry, now_epoch),
                PromptDelivery::Unverified,
                "{reason} は未達と断定できない"
            );
            assert_eq!(
                prompt_delivery_assessment(entry, now_epoch).as_str(),
                "unverified"
            );
            let _ = std::fs::remove_file(&path);
        }
    }

    /// #1294 の A/B（純粋関数側）。旧挙動は顛末に関係なく `undelivered` へ倒していた
    #[test]
    fn 顛末から3値を引く判定はabで切り替わる() {
        let stalled = tako_core::prompt_delivery::Stall::PeerSendStalled.code();
        assert_eq!(
            failure_assessment(Some(stalled), false),
            PromptDelivery::Unverified
        );
        assert_eq!(
            failure_assessment(Some(stalled), true),
            PromptDelivery::OverdueSuspect,
            "TAKO_1294_LEGACY=1 は修正前（未達と断定）へ戻る"
        );
        // 本当に未達の顛末・顛末コードを持たない古い記録は従来どおり未達
        assert_eq!(
            failure_assessment(Some("paste_not_reflected"), false),
            PromptDelivery::OverdueSuspect
        );
        assert_eq!(
            failure_assessment(None, false),
            PromptDelivery::OverdueSuspect
        );
    }

    /// 一次シグナルが「ターンが走った」と言えば、未確認の記録があっても delivered へ昇格する
    /// （#1015 と同じ順序。#1294 でも壊さない）
    #[test]
    fn 未確認でもターン観測があれば届いた側へ昇格する() {
        let path = temp_registry_file("unverified-turn");
        register_at(&path, sample_record(1295));
        record_prompt_delivery_at(
            &path,
            1295,
            PromptDeliveryFlow::SpawnPrompt,
            Confidence::Unverified,
            tako_core::prompt_delivery::PEER_UNCONFIRMED,
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(1295).unwrap();
        let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
        assert_eq!(
            prompt_delivery_assessment_with(
                entry,
                now_epoch,
                DeliveryEvidence {
                    turn_observed: true,
                    ..Default::default()
                },
            ),
            PromptDelivery::Delivered
        );
        let _ = std::fs::remove_file(&path);
    }

    /// spawn プロンプト以外（後続 send）はレジストリを触らない（#778。#1294 でも不変）
    #[test]
    fn 後続sendの未確認はレジストリを触らない() {
        let path = temp_registry_file("unverified-followup");
        register_at(&path, sample_record(1296));
        record_prompt_delivery_at(
            &path,
            1296,
            PromptDeliveryFlow::FollowUpSend,
            Confidence::Unverified,
            tako_core::prompt_delivery::Stall::PeerSendStalled.code(),
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let (_, entry) = reg.find_active_by_pane(1296).unwrap();
        assert!(entry.prompt_delivery_failed_at.is_none());
        assert!(entry.prompt_delivery_failure.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn list_payloadは未達workerにだけ再送コマンドを出す() {
        let mut reg = WorkerRegistry::default();
        reg.workers.insert(
            "1".into(),
            WorkerEntry {
                agent: "claude".into(),
                status: "active".into(),
                pane: 7,
                spawned_at: crate::sessions::now_iso(),
                session_id: Some("sid".into()),
                prompt_delivery_failed_at: Some(crate::sessions::now_iso()),
                prompt_delivery_failure: Some("choice_dialog".into()),
                ..Default::default()
            },
        );
        reg.workers.insert(
            "2".into(),
            WorkerEntry {
                agent: "claude".into(),
                status: "active".into(),
                pane: 8,
                spawned_at: crate::sessions::now_iso(),
                prompt_delivered_at: Some(crate::sessions::now_iso()),
                ..Default::default()
            },
        );
        let payload = list_payload(&reg, &[], &[], &[], false);
        let items = payload["workers"].as_array().unwrap();
        let undelivered = items.iter().find(|w| w["pane"] == 7).unwrap();
        let delivered = items.iter().find(|w| w["pane"] == 8).unwrap();
        assert_eq!(undelivered["prompt_delivery"], "undelivered");
        assert_eq!(undelivered["prompt_delivery_failure"], "choice_dialog");
        assert!(
            undelivered["resend_command"].is_string(),
            "再送コマンドを提示"
        );
        assert_eq!(delivered["prompt_delivery"], "delivered");
        assert!(
            delivered["resend_command"].is_null(),
            "到達済みには出さない"
        );
    }

    /// #2470: 案内は打った時点で spawn と同じ組み立てを通す 1 行。照会のたびに組まない
    /// （ファイルを読まない = 同じ入力なら常に同じ文字列・設定や会話の記録の有無に依らない）
    #[test]
    fn issue2470_resume_commandは打った時点で組む1行だけを返す() {
        let entry = WorkerEntry {
            agent: "claude".into(),
            session_id: Some("abc-123".into()),
            cwd: Some("/tmp/proj".into()),
            model: Some("opus".into()),
            effort: Some("high".into()),
            profile: Some("p2470".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command("7", &entry).as_deref(),
            Some("tako orchestrator resume 7")
        );
        // 会話 ID 未検出・claude 以外・ID が空は案内しない（従来と同じ条件）
        let no_sid = WorkerEntry {
            session_id: None,
            ..entry.clone()
        };
        assert!(resume_command("7", &no_sid).is_none());
        let codex = WorkerEntry {
            agent: "codex".into(),
            ..entry.clone()
        };
        assert!(resume_command("7", &codex).is_none());
        assert!(resume_command("", &entry).is_none());
    }

    /// 以下 2 本は修正前の形（`TAKO_2470_LEGACY=1` の腕 = `legacy_resume_command`）
    #[test]
    fn resume_commandはsession検出済みclaudeのみ組み立てる() {
        let entry = WorkerEntry {
            agent: "claude".into(),
            session_id: Some("abc-123".into()),
            cwd: Some("/tmp/proj".into()),
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env(&entry, None).as_deref(),
            Some("cd '/tmp/proj' && claude --model opus --effort high --resume abc-123")
        );
        // model / effort / cwd 省略時は最簡形
        let minimal = WorkerEntry {
            agent: "claude".into(),
            session_id: Some("abc-123".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env(&minimal, None).as_deref(),
            Some("claude --resume abc-123")
        );
        // session 未検出 / claude 以外は None
        let no_sid = WorkerEntry {
            agent: "claude".into(),
            ..Default::default()
        };
        assert!(resume_command_with_env(&no_sid, None).is_none());
        let codex = WorkerEntry {
            agent: "codex".into(),
            session_id: Some("abc".into()),
            ..Default::default()
        };
        assert!(resume_command_with_env(&codex, None).is_none());
    }

    /// Issue #652: `--account` で spawn した worker の会話は別 config ディレクトリに
    /// あるため、突然死からの復旧コマンドにも `CLAUDE_CONFIG_DIR` の前置が要る
    #[test]
    fn resume_commandはconfigdirを先頭に前置する() {
        let entry = WorkerEntry {
            agent: "claude".into(),
            session_id: Some("abc-123".into()),
            cwd: Some("/tmp/proj".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env(
                &entry,
                Some("export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; ")
            )
            .as_deref(),
            Some(
                "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; \
                 cd '/tmp/proj' && claude --resume abc-123"
            )
        );
    }

    #[test]
    fn resolveの前方一致と曖昧エラー() {
        let path = temp_registry_file("resolve");
        for pane in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
            register_at(&path, sample_record(pane));
        }
        let reg = WorkerRegistry::load_from(&path).unwrap();
        // 完全一致優先（"1" は "10" "11" と前方一致するが完全一致 "1" が勝つ）
        let (id, _) = reg.resolve("1").unwrap();
        assert_eq!(id, "1");
        // 一意な前方一致は不可（"1" 完全一致があるため）だが、存在しない prefix はエラー
        assert!(reg.resolve("99").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn gcはclosedの古い順に削りactiveを守る() {
        let mut reg = WorkerRegistry::default();
        for i in 0..(MAX_WORKERS as u64 + 10) {
            reg.next_id += 1;
            let active = i >= 5; // 先頭 5 件だけ closed
            reg.workers.insert(
                reg.next_id.to_string(),
                WorkerEntry {
                    pane: i,
                    status: if active { "active" } else { "closed" }.into(),
                    spawned_at: format!("2026-07-19T00:{:02}:{:02}Z", i / 60, i % 60),
                    agent: "claude".into(),
                    ..Default::default()
                },
            );
        }
        reg.gc();
        // closed は 5 件しか無いので 5 件だけ削られ、active は全件残る
        assert_eq!(reg.workers.len(), MAX_WORKERS + 10 - 5);
        assert!(reg.workers.values().all(|e| e.is_active()));
    }

    #[test]
    fn list_payloadがライブ状態とdeliveryを含む() {
        let path = temp_registry_file("payload");
        register_at(&path, sample_record(30));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[(999, None)], // pane 30 は GUI に不在
            &[],
            false,
        );
        assert_eq!(payload["count"], 1);
        let w = &payload["workers"][0];
        assert_eq!(w["worker_id"], "1");
        assert_eq!(w["pane_alive"], false);
        assert_eq!(w["tmux_alive"], true);
        assert_eq!(w["prompt_delivery"], "pending");
        assert_eq!(w["status"], "active");
        let _ = std::fs::remove_file(&path);
    }

    /// #822: worker が自動復帰の対象かを一覧に載せる。
    /// ペインが生きていないときは null（番号が別ペインへ再利用されている可能性がある）
    #[test]
    fn issue822_list_payloadにlimit_resumeが載る() {
        let path = temp_registry_file("limit-resume");
        register_at(&path, sample_record(30));
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let live = [(30u64, Some("tako-pane-30".to_string()))];

        // 有効なペイン
        let payload = list_payload(&reg, &["tako-pane-30".to_string()], &live, &[30], false);
        assert_eq!(payload["workers"][0]["pane_alive"], true);
        assert_eq!(payload["workers"][0]["limit_resume"], true);

        // 同じペインが生きているが自動復帰は無効
        let payload = list_payload(&reg, &["tako-pane-30".to_string()], &live, &[], false);
        assert_eq!(payload["workers"][0]["limit_resume"], false);

        // ペインが居ない = ペイン属性を読めないので null
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[(999, None)],
            &[30],
            false,
        );
        assert_eq!(payload["workers"][0]["pane_alive"], false);
        assert!(payload["workers"][0]["limit_resume"].is_null());
    }

    #[test]
    fn list_payloadはpane番号再利用の別ペインをaliveにしない() {
        let path = temp_registry_file("payload-reuse");
        register_at(&path, sample_record(30)); // tmux_session = tako-pane-30
        let reg = WorkerRegistry::load_from(&path).unwrap();
        // 同じ pane 番号 30 が GUI に居るが backend が別（= 再起動後の別ペイン）
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[(30, Some("tako-other".to_string()))],
            &[],
            false,
        );
        assert_eq!(payload["workers"][0]["pane_alive"], false);
        // backend が一致すれば alive（persist 復元で同一ペイン）
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[(30, Some("tako-pane-30".to_string()))],
            &[],
            false,
        );
        assert_eq!(payload["workers"][0]["pane_alive"], true);
        let _ = std::fs::remove_file(&path);
    }

    // --- #658: 死んだ active エントリの GC ---

    /// 生存観測を持たないレジストリを組み立てる（GC 判定のテスト材料）
    fn registry_with(entries: &[(&str, WorkerEntry)]) -> WorkerRegistry {
        let mut reg = WorkerRegistry::default();
        for (id, e) in entries {
            reg.workers.insert((*id).to_string(), e.clone());
        }
        reg
    }

    fn dead_candidate(pane: u64, tmux: Option<&str>, dead_since: Option<&str>) -> WorkerEntry {
        WorkerEntry {
            project: "tako".into(),
            agent: "claude".into(),
            pane,
            tmux_session: tmux.map(str::to_string),
            spawned_at: "2026-07-21T11:35:45Z".into(),
            status: "active".into(),
            dead_since: dead_since.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn 死んだエントリは初回観測では倒れず時刻だけ刻まれる() {
        let reg = registry_with(&[("1", dead_candidate(9, Some("tako-gone"), None))]);
        let plan = plan_sweep(&reg, &[], &[], 1_000_000);
        assert_eq!(plan.mark, vec!["1".to_string()]);
        assert!(
            plan.close.is_empty(),
            "1 回の観測では倒さない（過渡状態対策）"
        );
    }

    #[test]
    fn 確認期間を超えた死亡エントリはcloseされる() {
        let now = 1_000_000;
        let first = crate::diag::format_utc(now - DEAD_CONFIRM_SECS);
        let reg = registry_with(&[("1", dead_candidate(9, Some("tako-gone"), Some(&first)))]);
        let plan = plan_sweep(&reg, &[], &[], now);
        assert_eq!(plan.close, vec!["1".to_string()]);
        assert!(plan.mark.is_empty());

        // 確認期間の手前では倒れない
        let plan = plan_sweep(&reg, &[], &[], now - 1);
        assert!(plan.close.is_empty());
    }

    #[test]
    fn 生きているworkerはgcされない() {
        let now = 1_000_000;
        let old = crate::diag::format_utc(now - DEAD_CONFIRM_SECS * 10);
        // ペインが生きている / 器が生きている / 両方生きている の 3 通り
        let reg = registry_with(&[
            ("1", dead_candidate(9, Some("tako-a"), Some(&old))),
            ("2", dead_candidate(10, Some("tako-b"), Some(&old))),
            ("3", dead_candidate(11, None, Some(&old))),
        ]);
        let live_backends = vec!["tako-b".to_string()];
        let live_panes = vec![
            (9u64, Some("tako-a".to_string())),
            (11u64, None), // 器なし spawn は番号一致で生存
        ];
        let plan = plan_sweep(&reg, &live_backends, &live_panes, now);
        assert!(plan.close.is_empty(), "生きている worker は倒さない");
        // 生き返ったので死亡マークは取り消される
        let mut revived = plan.revive.clone();
        revived.sort();
        assert_eq!(revived, vec!["1", "2", "3"]);
    }

    #[test]
    fn pane番号が再利用されていても器が違えばgc対象になる() {
        let now = 1_000_000;
        let old = crate::diag::format_utc(now - DEAD_CONFIRM_SECS - 1);
        let reg = registry_with(&[("1", dead_candidate(9, Some("tako-old"), Some(&old)))]);
        // 同じ pane 番号だが別の器 = 別物（#390 の同一性検証と同じ規則）
        let plan = plan_sweep(&reg, &[], &[(9u64, Some("tako-new".to_string()))], now);
        assert_eq!(plan.close, vec!["1".to_string()]);
    }

    #[test]
    fn closedエントリはgcの対象外() {
        let now = 1_000_000;
        let old = crate::diag::format_utc(now - DEAD_CONFIRM_SECS * 5);
        let mut e = dead_candidate(9, Some("tako-gone"), Some(&old));
        e.status = "closed".into();
        let reg = registry_with(&[("1", e)]);
        assert!(plan_sweep(&reg, &[], &[], now).is_empty());
    }

    #[test]
    fn gcはファイルへ適用され追跡キーとresumeは残る() {
        let path = temp_registry_file("sweep");
        let id = register_at(&path, sample_record(42));
        WorkerRegistry::mutate_at(&path, |reg| {
            let e = reg.workers.get_mut(&id).unwrap();
            e.session_id = Some("sid-42".into());
            // 確認期間を過ぎた死亡マークを仕込む
            let now = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap();
            e.dead_since = Some(crate::diag::format_utc(now - DEAD_CONFIRM_SECS - 1));
        })
        .unwrap();

        let reg = sweep_dead_at(&path, &[], &[]).unwrap();
        let entry = &reg.workers[&id];
        assert_eq!(entry.status, "closed");
        assert_eq!(entry.close_reason.as_deref(), Some("gone"));
        assert!(entry.closed_at.is_some());
        assert!(entry.dead_since.is_none(), "倒したら死亡マークは畳む");
        // #390 の追跡・復旧材料は closed でも残っている
        assert_eq!(entry.session_id.as_deref(), Some("sid-42"));
        assert!(resume_command(&id, entry).is_some());
        // ディスクにも反映されている
        let on_disk = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(on_disk.workers[&id].status, "closed");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 掃除するものが無ければファイルを書き換えない() {
        let path = temp_registry_file("sweep-noop");
        let id = register_at(&path, sample_record(7));
        let before = std::fs::read_to_string(&path).unwrap();
        // 生きている（器が見えている）→ 何もしない
        let live = vec!["tako-pane-7".to_string()];
        sweep_dead_at(&path, &live, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // 死んで見えた初回は dead_since を刻むので書き換わる
        sweep_dead_at(&path, &[], &[]).unwrap();
        let after = WorkerRegistry::load_from(&path).unwrap();
        assert!(after.workers[&id].dead_since.is_some());
        assert!(after.workers[&id].is_active(), "刻んだだけでまだ active");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 一覧のpane_aliveとgcの判定は同じ規則を使う() {
        let now = 1_000_000;
        let old = crate::diag::format_utc(now - DEAD_CONFIRM_SECS - 1);
        let reg = registry_with(&[
            ("1", dead_candidate(9, Some("tako-a"), Some(&old))), // 生存
            ("2", dead_candidate(10, Some("tako-b"), Some(&old))), // 死亡
        ]);
        let live_panes = vec![(9u64, Some("tako-a".to_string()))];
        let payload = list_payload(&reg, &[], &live_panes, &[], false);
        let items = payload["workers"].as_array().unwrap();
        let alive: Vec<bool> = items
            .iter()
            .map(|w| w["pane_alive"].as_bool().unwrap() || w["tmux_alive"].as_bool().unwrap())
            .collect();
        let closed = plan_sweep(&reg, &[], &live_panes, now).close;
        // 一覧で死んで見えるものだけが GC 対象（表示と GC の判定が食い違わない）
        for (item, is_alive) in items.iter().zip(alive) {
            let id = item["worker_id"].as_str().unwrap().to_string();
            assert_eq!(
                closed.contains(&id),
                !is_alive,
                "worker {id} の一覧表示と GC 判定が食い違っている"
            );
        }
    }

    #[test]
    fn mark_closed_by_paneは該当が無ければ書き込まない() {
        // 専用ファイルで検証する（registry_path() の共有ファイルを使うと
        // 同じプロセスの他テストの登録と混ざる）
        let path = temp_registry_file("mark-closed-noop");
        let id = register_at(&path, sample_record(1234));
        let before = std::fs::read_to_string(&path).unwrap();
        // worker でないペインの close（全ペインの close 経路から呼ばれる）
        mark_closed_by_pane_at(&path, 999_999, "explicit_close").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "該当なしでファイルを書き換えない"
        );
        // 該当があれば倒す
        mark_closed_by_pane_at(&path, 1234, "explicit_close").unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(reg.workers[&id].status, "closed");
        let _ = std::fs::remove_file(&path);
    }

    /// #775: `close_reason` の綴りはペインログのクローズマーカーと同じ 1 実装から採る。
    /// 経路ごとに別の語彙を作らないことがこの関数の存在理由
    #[test]
    fn close_reasonは発生源の語彙をペインログと共有する() {
        for (origin, expected) in [
            (CloseOrigin::PaneButton, "close:gui"),
            (CloseOrigin::TabButton, "close:gui-tab"),
            (CloseOrigin::Keyboard, "close:kbd"),
            (CloseOrigin::Cli, "close:dispatch(cli)"),
            (CloseOrigin::Mcp, "close:dispatch(mcp)"),
        ] {
            assert_eq!(close_reason_for(origin, None), expected);
            // ペインログのマーカーと**同じ文字列**（突き合わせの前提）
            assert_eq!(close_reason_for(origin, None), origin.marker());
        }
        // dispatch 経路は呼び出し元 role も載る（GUI 経路には載らない）
        assert_eq!(
            close_reason_for(CloseOrigin::Cli, Some("orchestrator-master:default")),
            "close:dispatch(cli, caller=orchestrator-master:default)"
        );
        assert_eq!(
            close_reason_for(CloseOrigin::PaneButton, Some("orchestrator-master:default")),
            "close:gui"
        );
    }

    /// #775: 発生源つきの記録が実ファイルへ入ること。#390 の不変条件
    /// （PTY 死亡では倒さない）をこの入口でも守る
    #[test]
    fn mark_closed_by_originは発生源をclose_reasonへ残す() {
        let path = temp_registry_file("mark-closed-origin");
        let id = register_at(&path, sample_record(4321));
        // 器は `registry_path()`（#[cfg(test)] ではプロセス固有の一時ファイル）を
        // 使うので、ここでは綴りの決定とファイル反映を別々に突き合わせる
        let reason = close_reason_for(CloseOrigin::TabButton, None);
        mark_closed_by_pane_at(&path, 4321, &reason).unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(reg.workers[&id].status, "closed");
        assert_eq!(
            reg.workers[&id].close_reason.as_deref(),
            Some("close:gui-tab"),
            "タブ × で閉じた発生源が残る"
        );
        assert!(reg.workers[&id].closed_at.is_some());

        // 既に closed のエントリは再度の close で書き換えない（冪等）
        let after = std::fs::read_to_string(&path).unwrap();
        mark_closed_by_pane_at(&path, 4321, &close_reason_for(CloseOrigin::Keyboard, None))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            after,
            "closed 済みのエントリを二度目の close が上書きしない"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// #775: PTY 死亡（`ProcessExit`）はこの入口でも倒さない。
    ///
    /// #390 の不変条件（ペインが消えても worker は生きている）を、呼び出し側の
    /// 取り違えから守る二重の防御。`mark_closed_by_origin` は `registry_path()` を
    /// 見るので、共有の一時レジストリ（`#[cfg(test)]` ではプロセス固有）へ登録して測る。
    ///
    /// 共有ファイルは並列の他のテストも書くので、比べるのは**このテストのエントリだけ**（#2248）。
    /// 書き手は 2 種類: 別のペインの登録（spawn のテストの `record_spawn`）と、一覧の GC
    /// （`issue390_workersリストがdispatchで返る` の `sweep_dead` = 見えないペインの active へ
    /// `dead_since` を刻む）。`dead_since` は GC の欄で close の経路は触らないので、比べる前に外す
    #[test]
    fn pty死亡はmark_closed_by_originで倒さない() {
        let shared = registry_path().unwrap();
        // 共有レジストリの鍵は採番器から取る（spawn のテストの採番と重ねない = #2239）
        let pane = tako_core::unused_id::pane().as_u64();
        let id = record_spawn(sample_record(pane)).expect("共有レジストリへ登録できる");
        let own = || {
            let mut entry = WorkerRegistry::load_from(&shared).unwrap().workers[&id].clone();
            entry.dead_since = None;
            entry
        };
        let before = own();
        // 並列の書き手の 2 種類をここで必ず挟む（ファイル全体を比べる形へ戻すと毎回落ちる = #2248）
        let other = record_spawn(sample_record(tako_core::unused_id::pane().as_u64()))
            .expect("並列のテストの登録");
        WorkerRegistry::mutate_at(&shared, |reg| {
            // GC の初観測と同じ書き込み（`sweep_dead_at` の mark）
            reg.workers.get_mut(&id).unwrap().dead_since = Some(crate::sessions::now_iso());
        })
        .unwrap();

        mark_closed_by_origin(pane, CloseOrigin::ProcessExit, None).unwrap();
        assert_eq!(own(), before, "PTY 死亡でエントリを書き換えてはいけない");
        assert!(
            own().is_active(),
            "PTY 死亡で active を倒してはいけない（#390）"
        );

        // 明示 close なら同じ入口で倒れる（ガードが全部を止めていない検出力）
        mark_closed_by_origin(pane, CloseOrigin::TabButton, None).unwrap();
        let reg = WorkerRegistry::load_from(&shared).unwrap();
        assert_eq!(reg.workers[&id].status, "closed");
        assert_eq!(
            reg.workers[&id].close_reason.as_deref(),
            Some("close:gui-tab")
        );
        // 後始末（共有ファイルなので自分のエントリだけ消す）
        let _ = WorkerRegistry::mutate_at(&shared, |reg| {
            reg.workers.remove(&id);
            reg.workers.remove(&other);
        });
    }

    // ─── #1638: 宣言したモデルと実際のモデル ───

    /// テスト 1 本ぶんのレジストリに worker を 1 件置く（宣言 `model`・会話 ID・系統を指定）
    fn put_worker(
        path: &Path,
        id: &str,
        agent: &str,
        model: Option<&str>,
        session_id: Option<&str>,
        status: &str,
    ) {
        WorkerRegistry::mutate_at(path, |reg| {
            reg.workers.insert(
                id.to_string(),
                WorkerEntry {
                    pane: 7000 + id.len() as u64,
                    agent: agent.into(),
                    model: model.map(str::to_string),
                    session_id: session_id.map(str::to_string),
                    status: status.into(),
                    spawned_at: crate::sessions::now_iso(),
                    ..Default::default()
                },
            );
        })
        .unwrap();
    }

    /// 偽の transcript（会話 ID → 最初の応答のモデル）。呼ばれた会話 ID を控える
    fn fake_probe<'a>(
        table: &'a [(&'a str, &'a str)],
        asked: &'a std::cell::RefCell<Vec<String>>,
    ) -> impl Fn(&str) -> Option<(String, Option<String>)> + 'a {
        move |sid: &str| {
            asked.borrow_mut().push(sid.to_string());
            table
                .iter()
                .find(|(s, _)| *s == sid)
                .map(|(_, m)| (m.to_string(), Some("univ".to_string())))
        }
    }

    /// 受け入れ条件 1: 宣言 claude-opus-5-5・実際 claude-opus-5 なら両方を返し、warnings に 1 行。
    /// 一致なら warnings は出ない。まだ応答の無い worker は null / pending
    #[test]
    fn 宣言と実際のモデルを分けて持ち食い違いをwarningsに出す_1638() {
        let path = temp_registry_file("model-actual-1638");
        put_worker(
            &path,
            "1",
            "claude",
            Some("claude-opus-5-5"),
            Some("sid-a"),
            "active",
        );
        put_worker(
            &path,
            "22",
            "claude",
            Some("claude-opus-5"),
            Some("sid-b"),
            "active",
        );
        put_worker(
            &path,
            "333",
            "claude",
            Some("claude-opus-5-5"),
            Some("sid-c"),
            "active",
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let table = [("sid-a", "claude-opus-5"), ("sid-b", "claude-opus-5")];
        let found = observe_pending_models_at(&path, &fake_probe(&table, &asked));
        assert_eq!(found.len(), 2, "応答のある 2 件だけ確定する: {found:?}");
        let a = found.iter().find(|o| o.worker_id == "1").unwrap();
        assert_eq!(a.check(), tako_core::model_check::ModelCheck::Mismatch);
        assert!(a.warning().unwrap().contains("アカウント univ の"));

        let reg = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(
            reg.workers["1"].model.as_deref(),
            Some("claude-opus-5-5"),
            "宣言は残す"
        );
        assert_eq!(
            reg.workers["1"].model_actual.as_deref(),
            Some("claude-opus-5")
        );
        assert!(reg.workers["1"].model_actual_at.is_some());
        assert_eq!(
            reg.workers["1"].model_actual_account.as_deref(),
            Some("univ")
        );

        let v = list_payload(&reg, &[], &[], &[], false);
        let item = |id: &str| {
            v["workers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["worker_id"] == id)
                .unwrap()
                .clone()
        };
        let a = item("1");
        assert_eq!(
            a["model"], "claude-opus-5-5",
            "従来の model は宣言のまま（互換）"
        );
        assert_eq!(a["model_declared"], "claude-opus-5-5");
        assert_eq!(a["model_actual"], "claude-opus-5");
        assert_eq!(a["model_check"], "mismatch");
        assert_eq!(item("22")["model_check"], "match");
        // spawn 直後（まだ応答が無い）= 未確定を null と pending で表す
        let c = item("333");
        assert_eq!(c["model_actual"], Value::Null);
        assert_eq!(c["model_check"], "pending");
        let warnings = v["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "食い違いの 1 件だけ: {warnings:?}");
        let line = warnings[0].as_str().unwrap();
        assert!(line.starts_with("worker 1（pane "), "{line}");
        assert!(
            line.contains(
                "宣言 claude-opus-5-5 → 実際 claude-opus-5（アカウント univ の entitlement"
            ),
            "{line}"
        );

        // 確定したものは二度と探さない（transcript に触らない）。未確定の 1 件だけをまた探す
        asked.borrow_mut().clear();
        let again = observe_pending_models_at(&path, &fake_probe(&table, &asked));
        assert!(again.is_empty());
        assert_eq!(*asked.borrow(), vec!["sid-c".to_string()]);
        // 後から応答が来たら確定する。確定済みの値はその後の観測で書き換えない
        let later = [("sid-c", "claude-opus-5-5"), ("sid-a", "claude-sonnet-5")];
        let found = observe_pending_models_at(&path, &fake_probe(&later, &asked));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].check(), tako_core::model_check::ModelCheck::Match);
        let reg = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(
            reg.workers["1"].model_actual.as_deref(),
            Some("claude-opus-5")
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 観測しない系統（codex / agy = マトリクスの degraded）・closed・会話 ID 未検出は探さない
    #[test]
    fn 観測しない系統と閉じたworkerは探さない_1638() {
        let path = temp_registry_file("model-actual-skip-1638");
        put_worker(
            &path,
            "1",
            "codex",
            Some("gpt-5.5"),
            Some("sid-a"),
            "active",
        );
        put_worker(&path, "22", "agy", None, Some("sid-b"), "active");
        put_worker(
            &path,
            "333",
            "claude",
            Some("claude-opus-5-5"),
            Some("sid-c"),
            "closed",
        );
        put_worker(
            &path,
            "4444",
            "claude",
            Some("claude-opus-5-5"),
            None,
            "active",
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let table = [
            ("sid-a", "claude-opus-5"),
            ("sid-b", "claude-opus-5"),
            ("sid-c", "claude-opus-5"),
        ];
        assert!(observe_pending_models_at(&path, &fake_probe(&table, &asked)).is_empty());
        assert!(
            asked.borrow().is_empty(),
            "transcript を探しに行かない: {asked:?}"
        );
        let reg = WorkerRegistry::load_from(&path).unwrap();
        let v = list_payload(&reg, &[], &[], &[], true);
        let check = |id: &str| {
            v["workers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["worker_id"] == id)
                .unwrap()["model_check"]
                .clone()
        };
        assert_eq!(check("1"), "not_observed");
        assert_eq!(check("22"), "not_observed");
        assert_eq!(check("333"), "pending");
        assert_eq!(check("4444"), "pending");
        assert_eq!(v["warnings"], json!([]));
        // 1 件版（worker_status）も同じ: 照会で会話 ID が分かっても観測しない系統は探さない
        let codex = reg.workers["1"].clone();
        assert!(observe_worker_model_at(
            &path,
            "1",
            &codex,
            Some("sid-a"),
            &fake_probe(&table, &asked)
        )
        .is_none());
        // claude でレジストリに会話 ID が無くても、照会で解決した会話 ID で確定できる
        let claude = reg.workers["4444"].clone();
        let late = [("sid-d", "claude-opus-5")];
        let obs = observe_worker_model_at(
            &path,
            "4444",
            &claude,
            Some("sid-d"),
            &fake_probe(&late, &asked),
        )
        .expect("照会の会話 ID で確定する");
        assert_eq!(obs.actual, "claude-opus-5");
        assert_eq!(asked.borrow().as_slice(), ["sid-d".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    /// #1638 前の workers.yaml（model_actual 系のキーが無い）はそのまま読める = 移行は要らない
    #[test]
    fn 実際のモデルのキーが無い旧ファイルを読める_1638() {
        let path = temp_registry_file("model-actual-old-1638");
        std::fs::write(
            &path,
            "workers:\n  '1':\n    agent: claude\n    model: claude-opus-5-5\n    pane: 3\n    \
             session_id: sid-a\n    status: active\nnext_id: 1\n",
        )
        .unwrap();
        let reg = WorkerRegistry::load_from(&path).unwrap();
        assert_eq!(reg.workers["1"].model_actual, None);
        assert_eq!(
            model_check(&reg.workers["1"]),
            tako_core::model_check::ModelCheck::Pending
        );
        // 書き戻しても空のキーは出さない（旧版の tako が読んでも同じ形）
        WorkerRegistry::mutate_at(&path, |_| {}).unwrap();
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("model_actual"));
        let _ = std::fs::remove_file(&path);
    }
}

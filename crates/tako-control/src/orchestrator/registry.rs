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
                    std::env::temp_dir()
                        .join(format!("tako-test-workers-{}.yaml", std::process::id()))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// 判定対象外（claude 以外の agent、closed、時刻パース不能）
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
            Self::NotApplicable => "n/a",
        }
    }
}

impl WorkerRegistry {
    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("workers.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("workers.yaml のパースに失敗: {e}"))
    }

    pub fn load() -> Result<Self, String> {
        let path = registry_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同型）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
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
            },
        );
        reg.gc();
        id
    })
}

/// 明示 close されたペインの active worker を closed にする。
/// レジストリ不在（orchestrator 未使用）は何もしない（通常ペインの close に
/// ファイル IO のコストを掛けない）。**worker でないペインでも書き込まない**
/// （全ペインの close 経路から呼ばれるため。#658 で GUI 経路にも配線した）
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

/// 検出済み claude session をレジストリへ反映する（tmux_session キー）。
/// session_id の初観測 = transcript 生成 = プロンプト到達の証跡として
/// `prompt_delivered_at` も同時に記録する。GUI の定期スキャンおよび
/// worker_status の解決成功時（lazy 昇格）から呼ばれる
pub fn record_session_detected(tmux_session: &str, session_id: &str) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    // 変更が無いなら書き込みをスキップ（定期スキャンからの毎回書き込み防止）
    let current = WorkerRegistry::load_from(&path)?;
    let needs_update = current.workers.values().any(|e| {
        e.is_active()
            && e.tmux_session.as_deref() == Some(tmux_session)
            && (e.session_id.as_deref() != Some(session_id) || e.prompt_delivered_at.is_none())
    });
    if !needs_update {
        return Ok(());
    }
    let now = crate::sessions::now_iso();
    WorkerRegistry::mutate_at(&path, |reg| {
        for entry in reg.workers.values_mut() {
            if entry.is_active() && entry.tmux_session.as_deref() == Some(tmux_session) {
                entry.session_id = Some(session_id.to_string());
                if entry.prompt_delivered_at.is_none() {
                    entry.prompt_delivered_at = Some(now.clone());
                }
            }
        }
    })
}

/// 送達フロー（PromptFlow）の結果をレジストリへ記録する（Issue #530）。
/// `verified = true` は「貼り付けが入力欄へ反映され、送信後に残留が消えた」という
/// 積極的な証拠。`false` は未達の疑い（理由コードつき）。
///
/// pane 番号で引く（PromptFlow が持つキー）。同番号ペインの再利用による誤更新を防ぐため、
/// active かつ既に決着（delivered / failed）していないエントリだけを対象にする。
/// 記録失敗で送達フローを止めないよう、呼び出し側は警告のみで継続する
pub fn record_prompt_delivery(
    pane: u64,
    flow: PromptDeliveryFlow,
    verified: bool,
    reason: &str,
) -> Result<(), String> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    record_prompt_delivery_at(&path, pane, flow, verified, reason)
}

fn record_prompt_delivery_at(
    path: &Path,
    pane: u64,
    flow: PromptDeliveryFlow,
    verified: bool,
    reason: &str,
) -> Result<(), String> {
    if flow != PromptDeliveryFlow::SpawnPrompt {
        return Ok(());
    }
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

/// レジストリの session ID から復旧コマンドを組み立てる（#390: SIGSEGV 等の
/// 突然死からの復旧提示）。claude のみ（--resume の互換が確認できているのは claude）。
/// master はこのコマンドを死んだペインのシェルへ send_input するか、新ペインで実行する。
///
/// 会話が既定以外の config ディレクトリにあれば `CLAUDE_CONFIG_DIR` を前置する
/// （`--account` で spawn した worker の会話は `~/.claude` に無い。Issue #652）
pub fn resume_command(entry: &WorkerEntry) -> Option<String> {
    let env_prefix = entry
        .session_id
        .as_deref()
        .filter(|_| entry.agent == "claude")
        .and_then(crate::transcript::resume_env_prefix);
    resume_command_with_env(entry, env_prefix.as_deref())
}

/// `resume_command` の本体（env プレフィクスを引数で受け取るテスト可能版）
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

/// prompt 送達状態を判定する（Issue #390 要件 4）。
/// OverdueSuspect は「疑い」であり、最終的な未達イベントの発火は呼び出し側が
/// 画面状態（busy でない・実行中子プロセスなし）と組み合わせて決める
pub fn prompt_delivery_assessment(entry: &WorkerEntry, now_epoch: i64) -> PromptDelivery {
    // 送達フローが未達を確定させていれば、それを最優先する（Issue #530）。
    // session 検出は「claude が起動した」証拠であって「プロンプトが届いた」証拠ではない
    // （初回のテーマ選択・ログイン方法選択ダイアログにプロンプトが食われても
    // claude 自体は起動するため session_id は付く = 旧実装の delivered 偽陽性）
    if entry.prompt_delivery_failed_at.is_some() {
        return PromptDelivery::OverdueSuspect;
    }
    if entry.session_id.is_some() || entry.prompt_delivered_at.is_some() {
        return PromptDelivery::Delivered;
    }
    // transcript（session_id）の観測経路があるのは claude のみ。
    // codex / agy を undelivered と誤検知しないため対象外にする
    if entry.agent != "claude" || !entry.is_active() {
        return PromptDelivery::NotApplicable;
    }
    let Some(spawned) = crate::sessions::parse_iso(&entry.spawned_at) else {
        return PromptDelivery::NotApplicable;
    };
    if now_epoch - spawned > PROMPT_DELIVERY_GRACE_SECS {
        PromptDelivery::OverdueSuspect
    } else {
        PromptDelivery::Pending
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
    include_closed: bool,
) -> Value {
    let now_epoch = crate::sessions::parse_iso(&crate::sessions::now_iso()).unwrap_or(0);
    let mut items: Vec<Value> = Vec::new();
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
        items.push(json!({
            "worker_id": id,
            "label": e.label,
            "project": e.project,
            "agent": e.agent,
            "model": e.model,
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
            "resume_command": resume_command(e),
            // 未達 worker にだけ再送コマンドを出す（#530 の受け入れ条件 3）
            "resend_command": match delivery {
                PromptDelivery::OverdueSuspect => resend_command(e),
                _ => None,
            },
        }));
    }
    json!({ "workers": items, "count": items.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("tako-registry-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}-{}.yaml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
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
        // session_id 検出済み = delivered（時間に関係なく）
        entry.session_id = Some("abc".into());
        assert_eq!(
            prompt_delivery_assessment(&entry, now_epoch + 10_000),
            PromptDelivery::Delivered
        );
        // claude 以外は対象外
        let codex = WorkerEntry {
            agent: "codex".into(),
            status: "active".into(),
            spawned_at: crate::sessions::now_iso(),
            ..Default::default()
        };
        assert_eq!(
            prompt_delivery_assessment(&codex, now_epoch + 10_000),
            PromptDelivery::NotApplicable
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
            false,
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
            false,
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
        let payload = list_payload(&reg, &[], &[], false);
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
            false,
        );
        assert_eq!(payload["workers"][0]["pane_alive"], false);
        // backend が一致すれば alive（persist 復元で同一ペイン）
        let payload = list_payload(
            &reg,
            &["tako-pane-30".to_string()],
            &[(30, Some("tako-pane-30".to_string()))],
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
        assert!(resume_command(entry).is_some());
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
        let payload = list_payload(&reg, &[], &live_panes, false);
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
}

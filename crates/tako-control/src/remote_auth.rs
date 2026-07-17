//! remote_auth — tako remote の二層認証（機器ペアリング）と監査 metadata（#283）
//!
//! 認証設計（計画 `.agent/plans/tako-remote-plan.md` §4 が正）:
//! - 層① Tailscale device identity: `tailscale serve` が付与する `X-Forwarded-For` の
//!   接続元 IP を `tailscale whois` で照合し、tailnet 上の実在ノードであることを検証する。
//!   ヘッダ無し（ローカル直結）や `peer not found`（偽装 IP）は拒否する
//!   （弾 0 実測: serve はクライアントの偽ヘッダを完全上書きする。
//!   `.agent/investigations/tailscale-serve-poc.md`）
//! - 層② tako 機器ペアリング: ノードの恒久 ID（`Node.StableID`）をデバイス識別子として
//!   レジストリ（`<state_dir>/devices.json`）と照合する。未登録デバイスは
//!   ペアリング要求（`POST /api/pair`）だけができ、Mac 画面の承認ダイアログで
//!   許可されるまで画面データを一切受け取れない
//!
//! role は 4 段階（強い順に包含）: Observe（画面閲覧のみ・既定）⊂ Interact（+ 入力）⊂
//! Manage（+ close / resize）⊂ Admin（+ 端末管理）。
//! **ペアリング承認と role 昇格は Mac 側 GUI 限定**（AI フルコントロール不変条件の例外。
//! 理由は `.agent/requirements.md` FR-6 節に明記）。承認 API はローカル管理トークン
//! （`<state_dir>/tako-remote.token`、0600）で保護され、tako-app の承認ダイアログだけが呼ぶ。
//!
//! 監査 metadata: 接続開始終了・ペアリング・入力 byte 数・revoke を
//! `<state_dir>/audit.log`（JSONL）へ記録する。**ペイン内容・入力テキストは記録しない**
//! （ログ規約 = AGENTS.md 絶対ルールの維持）。

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tailscale::{WhoisError, WhoisInfo};

/// interact session の idle timeout 既定値（最終入力からこの時間で session 終了）
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// whois 照合結果のキャッシュ TTL（LocalAPI 呼び出しの頻度を抑える）
const WHOIS_CACHE_TTL: Duration = Duration::from_secs(60);
/// 監査ログのローテート閾値（persist.log と同じ方式: 超えたら .1 へ 1 世代退避）
const AUDIT_LOG_MAX_BYTES: u64 = 256 * 1024;
/// 拒否記録の保持期間（PWA が「拒否されました」を表示するためのメモリ内記録）
const DENIED_TTL: Duration = Duration::from_secs(10 * 60);

/// デバイスの role（権限は強い順に包含: Observe ⊂ Interact ⊂ Manage ⊂ Admin）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    /// 画面閲覧のみ（既定）
    Observe,
    /// + テキスト入力・承認応答
    Interact,
    /// + close / resize
    Manage,
    /// + 端末管理（devices list / revoke）
    Admin,
}

impl DeviceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Interact => "interact",
            Self::Manage => "manage",
            Self::Admin => "admin",
        }
    }

    /// 文字列から解析する（不正値は None）
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "observe" => Some(Self::Observe),
            "interact" => Some(Self::Interact),
            "manage" => Some(Self::Manage),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// 登録済みデバイス（devices.json に永続化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// Tailscale ノードの恒久 ID（`Node.StableID`）
    pub id: String,
    /// 表示名（ペアリング要求時に端末が申告。既定はノードのマシン名）
    pub name: String,
    /// ノード所有ユーザーのログイン名
    pub login: String,
    /// ノードの MagicDNS 名
    pub node_name: String,
    pub role: DeviceRole,
    /// 登録時刻（unix epoch 秒）
    pub created_at: u64,
    /// 最終アクセス時刻（unix epoch 秒。永続化はベストエフォート）
    #[serde(default)]
    pub last_seen: u64,
}

/// ペアリング / role 昇格の保留リクエスト（メモリ内のみ。daemon 再起動で消える =
/// 端末が再要求する。安全側に倒す）
#[derive(Debug, Clone, Serialize)]
pub struct PendingRequest {
    /// リクエスト ID = デバイス ID（1 ノードにつき保留は常に 1 件。再要求は上書き）
    pub device_id: String,
    pub name: String,
    pub login: String,
    pub node_name: String,
    pub requested_role: DeviceRole,
    /// 新規ペアリングか、登録済みデバイスの role 変更要求か
    pub kind: RequestKind,
    pub requested_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Pair,
    Upgrade,
}

/// devices.json のトップレベル形式
#[derive(Debug, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<Device>,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// デバイスレジストリ + 保留リクエスト + interact session + whois キャッシュ。
/// daemon が 1 個持ち、全リクエストハンドラから共有する
pub struct DeviceRegistry {
    /// devices.json のパス
    path: PathBuf,
    /// 監査ログのパス
    audit_path: PathBuf,
    devices: HashMap<String, Device>,
    pending: HashMap<String, PendingRequest>,
    /// 拒否したデバイス → 拒否時刻（PWA の「拒否されました」表示用。メモリ内のみ）
    denied: HashMap<String, Instant>,
    /// whois 照合キャッシュ（IP → 結果）
    whois_cache: HashMap<String, (Instant, WhoisInfo)>,
    /// interact session: デバイス ID → 最終入力時刻
    interact_sessions: HashMap<String, Instant>,
    /// idle timeout（TAKO_REMOTE_IDLE_TIMEOUT_SECS で差し替え可能・テスト用）
    idle_timeout: Duration,
}

impl DeviceRegistry {
    /// state ディレクトリ配下の devices.json を読み込んで開く。
    /// ファイル不在は空レジストリ。破損 JSON は読み捨てず起動エラーにする
    /// （黙って全デバイス失効させない）
    pub fn open(state_dir: &std::path::Path) -> Result<Self, String> {
        let path = state_dir.join("devices.json");
        let audit_path = state_dir.join("audit.log");
        let devices = match std::fs::read_to_string(&path) {
            Ok(content) => {
                let file: DevicesFile = serde_json::from_str(&content)
                    .map_err(|e| format!("devices.json の解釈に失敗（{}）: {e}", path.display()))?;
                file.devices
                    .into_iter()
                    .map(|d| (d.id.clone(), d))
                    .collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(format!("devices.json の読み取りに失敗: {e}")),
        };
        let idle_timeout = std::env::var("TAKO_REMOTE_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_IDLE_TIMEOUT);
        Ok(Self {
            path,
            audit_path,
            devices,
            pending: HashMap::new(),
            denied: HashMap::new(),
            whois_cache: HashMap::new(),
            interact_sessions: HashMap::new(),
            idle_timeout,
        })
    }

    /// idle timeout を明示設定する（テスト用）
    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    /// devices.json へ永続化する（0600・atomic rename。secrets ではないが
    /// identity 情報を含むため secret ファイルと同じ扱い）
    fn save(&self) -> Result<(), String> {
        let file = DevicesFile {
            devices: {
                let mut v: Vec<Device> = self.devices.values().cloned().collect();
                v.sort_by_key(|d| d.created_at);
                v
            },
        };
        let content = serde_json::to_string_pretty(&file)
            .map_err(|e| format!("devices.json の構築に失敗: {e}"))?;
        crate::remote::write_state_file(&self.path, &content)
            .map_err(|e| format!("devices.json の書き込みに失敗: {e}"))
    }

    /// 登録済みデバイスを返す（未登録は None）
    pub fn device(&self, device_id: &str) -> Option<&Device> {
        self.devices.get(device_id)
    }

    /// 登録済みデバイス一覧（作成順）
    pub fn devices(&self) -> Vec<Device> {
        let mut v: Vec<Device> = self.devices.values().cloned().collect();
        v.sort_by_key(|d| d.created_at);
        v
    }

    /// 保留リクエスト一覧（要求順）
    pub fn pending(&self) -> Vec<PendingRequest> {
        let mut v: Vec<PendingRequest> = self.pending.values().cloned().collect();
        v.sort_by_key(|p| p.requested_at);
        v
    }

    /// このデバイスが拒否直後か（PWA の状態表示用）
    pub fn recently_denied(&mut self, device_id: &str) -> bool {
        if let Some(at) = self.denied.get(device_id) {
            if at.elapsed() < DENIED_TTL {
                return true;
            }
            self.denied.remove(device_id);
        }
        false
    }

    /// ペアリング / 昇格リクエストを受け付ける。
    /// - 未登録デバイス: kind=Pair の保留を作る（既存の保留は上書き）
    /// - 登録済みデバイス: 現 role と異なる要求なら kind=Upgrade の保留を作る
    /// - 登録済みかつ同 role: 何もしない（already_registered を返す）
    pub fn request_pairing(
        &mut self,
        who: &WhoisInfo,
        name: &str,
        requested_role: DeviceRole,
    ) -> Value {
        let name = if name.trim().is_empty() {
            who.hostname.clone()
        } else {
            name.trim().chars().take(64).collect()
        };
        let kind = match self.devices.get(&who.stable_id) {
            Some(existing) if existing.role == requested_role => {
                return json!({
                    "status": "already_registered",
                    "role": existing.role.as_str(),
                });
            }
            Some(_) => RequestKind::Upgrade,
            None => RequestKind::Pair,
        };
        self.denied.remove(&who.stable_id);
        let req = PendingRequest {
            device_id: who.stable_id.clone(),
            name: name.clone(),
            login: who.login.clone(),
            node_name: who.node_name.clone(),
            requested_role,
            kind,
            requested_at: now_epoch(),
        };
        self.pending.insert(who.stable_id.clone(), req);
        self.audit(
            match kind {
                RequestKind::Pair => "pair_requested",
                RequestKind::Upgrade => "upgrade_requested",
            },
            &who.stable_id,
            &name,
            json!({ "login": who.login, "requested_role": requested_role.as_str() }),
        );
        json!({ "status": "pending" })
    }

    /// 保留リクエストを承認する（Mac GUI 専用経路から呼ばれる）。
    /// role は承認時にダイアログで選び直せる（省略時は要求 role）
    pub fn approve(&mut self, device_id: &str, role: Option<DeviceRole>) -> Result<Device, String> {
        let req = self
            .pending
            .remove(device_id)
            .ok_or_else(|| format!("保留中のペアリング要求が無い: {device_id}"))?;
        let role = role.unwrap_or(req.requested_role);
        let now = now_epoch();
        let device = match self.devices.get(device_id) {
            Some(existing) => Device {
                role,
                last_seen: now,
                ..existing.clone()
            },
            None => Device {
                id: req.device_id.clone(),
                name: req.name.clone(),
                login: req.login.clone(),
                node_name: req.node_name.clone(),
                role,
                created_at: now,
                last_seen: now,
            },
        };
        self.devices.insert(device_id.to_string(), device.clone());
        self.save()?;
        self.audit(
            match req.kind {
                RequestKind::Pair => "pair_approved",
                RequestKind::Upgrade => "upgrade_approved",
            },
            device_id,
            &device.name,
            json!({ "login": device.login, "role": role.as_str() }),
        );
        Ok(device)
    }

    /// 保留リクエストを拒否する（Mac GUI 専用経路から呼ばれる）
    pub fn deny(&mut self, device_id: &str) -> Result<(), String> {
        let req = self
            .pending
            .remove(device_id)
            .ok_or_else(|| format!("保留中のペアリング要求が無い: {device_id}"))?;
        self.denied.insert(device_id.to_string(), Instant::now());
        self.audit(
            match req.kind {
                RequestKind::Pair => "pair_denied",
                RequestKind::Upgrade => "upgrade_denied",
            },
            device_id,
            &req.name,
            json!({ "login": req.login, "requested_role": req.requested_role.as_str() }),
        );
        Ok(())
    }

    /// デバイスを失効させる。登録を削除し、interact session も破棄する。
    /// WS の即時切断は呼び出し側（daemon）が broadcaster 経由で行う
    pub fn revoke(&mut self, device_id: &str) -> Result<Device, String> {
        let device = self
            .devices
            .remove(device_id)
            .ok_or_else(|| format!("登録されていないデバイス: {device_id}"))?;
        self.pending.remove(device_id);
        self.interact_sessions.remove(device_id);
        self.save()?;
        self.audit(
            "device_revoked",
            device_id,
            &device.name,
            json!({ "login": device.login }),
        );
        Ok(device)
    }

    /// アクセスを記録する（last_seen 更新。永続化は revoke / approve 時に乗る）
    pub fn touch(&mut self, device_id: &str) {
        if let Some(d) = self.devices.get_mut(device_id) {
            d.last_seen = now_epoch();
        }
    }

    /// whois キャッシュを引く（TTL 内のみ）
    pub fn cached_whois(&mut self, ip: &str) -> Option<WhoisInfo> {
        if let Some((at, info)) = self.whois_cache.get(ip) {
            if at.elapsed() < WHOIS_CACHE_TTL {
                return Some(info.clone());
            }
            self.whois_cache.remove(ip);
        }
        None
    }

    /// whois 照合結果をキャッシュへ入れる
    pub fn cache_whois(&mut self, ip: &str, info: WhoisInfo) {
        self.whois_cache
            .insert(ip.to_string(), (Instant::now(), info));
    }

    /// input 実行を記録する。idle timeout を超えた（または初回の）input なら
    /// 新しい interact session の開始として true を返す（呼び出し側が通知を出す）
    pub fn record_input(&mut self, device_id: &str) -> bool {
        let now = Instant::now();
        let is_new = match self.interact_sessions.get(device_id) {
            Some(last) => last.elapsed() >= self.idle_timeout,
            None => true,
        };
        self.interact_sessions.insert(device_id.to_string(), now);
        if is_new {
            let name = self
                .devices
                .get(device_id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            self.audit("session_start", device_id, &name, json!({}));
        }
        is_new
    }

    /// idle timeout を超えた interact session を回収する。
    /// 終了した session のデバイス ID を返す（定期スイープから呼ぶ）
    pub fn sweep_idle_sessions(&mut self) -> Vec<String> {
        let timeout = self.idle_timeout;
        let expired: Vec<String> = self
            .interact_sessions
            .iter()
            .filter(|(_, last)| last.elapsed() >= timeout)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.interact_sessions.remove(id);
            let name = self
                .devices
                .get(id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            self.audit("session_end", id, &name, json!({}));
        }
        expired
    }

    /// 監査 metadata を JSONL で追記する。**内容（テキスト・画面）は記録しない**。
    /// AUDIT_LOG_MAX_BYTES を超えたら .1 へローテート（1 世代）
    pub fn audit(&self, event: &str, device_id: &str, device_name: &str, extra: Value) {
        let mut entry = json!({
            "ts": now_epoch(),
            "event": event,
            "device_id": device_id,
            "device_name": device_name,
        });
        if let (Some(obj), Some(ex)) = (entry.as_object_mut(), extra.as_object()) {
            for (k, v) in ex {
                obj.insert(k.clone(), v.clone());
            }
        }
        rotate_if_needed(&self.audit_path);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = writeln!(f, "{entry}");
        }
    }
}

/// audit.log が閾値を超えていたら .1 へ退避する
fn rotate_if_needed(path: &std::path::Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= AUDIT_LOG_MAX_BYTES {
            let rotated = path.with_extension("log.1");
            let _ = std::fs::rename(path, rotated);
        }
    }
}

/// リクエストの識別結果（層①）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// serve 経由の tailnet ノード（whois 照合済み）
    Tailnet(WhoisInfo),
    /// ローカル直結（`X-Forwarded-For` 無し）。管理 API 以外はアクセス不可
    Local,
}

/// 層①の識別: `X-Forwarded-For` ヘッダと whois 照合から接続元を特定する。
/// - ヘッダ無し → Local（serve を経由していない）
/// - ヘッダあり + whois 成功 → Tailnet
/// - ヘッダあり + peer not found → Err（偽装 or 不正経路。拒否）
/// - whois 実行失敗 → Err（安全側に拒否）
pub fn identify(
    registry: &Mutex<DeviceRegistry>,
    ts_cli: &str,
    forwarded_for: Option<&str>,
) -> Result<Identity, String> {
    let Some(xff) = forwarded_for else {
        return Ok(Identity::Local);
    };
    // X-Forwarded-For は「client, proxy1, proxy2」形式がありうる。先頭が接続元
    let ip = xff.split(',').next().unwrap_or("").trim().to_string();
    if ip.is_empty() {
        return Err("X-Forwarded-For が空".into());
    }
    if let Ok(mut reg) = registry.lock() {
        if let Some(info) = reg.cached_whois(&ip) {
            return Ok(Identity::Tailnet(info));
        }
    }
    match crate::tailscale::whois(ts_cli, &ip) {
        Ok(info) => {
            if let Ok(mut reg) = registry.lock() {
                reg.cache_whois(&ip, info.clone());
            }
            Ok(Identity::Tailnet(info))
        }
        Err(WhoisError::PeerNotFound) => Err(format!(
            "接続元 {ip} は tailnet 上のノードではない（whois: peer not found）"
        )),
        Err(WhoisError::Failed(e)) => Err(format!("接続元の identity 検証に失敗: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(id: &str) -> WhoisInfo {
        WhoisInfo {
            stable_id: id.to_string(),
            node_name: format!("{id}.tail1234.ts.net"),
            hostname: format!("host-{id}"),
            login: "user@example.com".to_string(),
        }
    }

    /// テスト用の一時ディレクトリ（drop で削除。既存 config_io テストと同パターン）
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tako-remote-auth-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("テスト dir 作成");
            Self(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_registry(tag: &str) -> (TempDir, DeviceRegistry) {
        let dir = TempDir::new(tag);
        let reg = DeviceRegistry::open(dir.path()).expect("open");
        (dir, reg)
    }

    #[test]
    fn ペアリングの要求から承認までの流れ() {
        let (_dir, mut reg) = temp_registry("pair-flow");
        let w = who("nDEV1");
        let resp = reg.request_pairing(&w, "My iPhone", DeviceRole::Observe);
        assert_eq!(resp["status"], "pending");
        assert_eq!(reg.pending().len(), 1);
        assert!(reg.device("nDEV1").is_none(), "承認前は未登録のまま");

        let device = reg.approve("nDEV1", None).expect("承認");
        assert_eq!(device.role, DeviceRole::Observe);
        assert_eq!(device.name, "My iPhone");
        assert!(reg.pending().is_empty());
        assert!(reg.device("nDEV1").is_some());
    }

    #[test]
    fn 承認時にroleを差し替えられる() {
        let (_dir, mut reg) = temp_registry("approve-role");
        reg.request_pairing(&who("nDEV1"), "pad", DeviceRole::Admin);
        let device = reg
            .approve("nDEV1", Some(DeviceRole::Observe))
            .expect("承認");
        assert_eq!(
            device.role,
            DeviceRole::Observe,
            "要求 Admin でも Observe で登録"
        );
    }

    #[test]
    fn 拒否は登録されずdenied記録が残る() {
        let (_dir, mut reg) = temp_registry("deny");
        reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Observe);
        reg.deny("nDEV1").expect("拒否");
        assert!(reg.device("nDEV1").is_none());
        assert!(reg.pending().is_empty());
        assert!(reg.recently_denied("nDEV1"));
    }

    #[test]
    fn 昇格要求はupgrade種別になり承認でroleが変わる() {
        let (_dir, mut reg) = temp_registry("upgrade");
        reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Observe);
        reg.approve("nDEV1", None).expect("承認");

        let resp = reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Interact);
        assert_eq!(resp["status"], "pending");
        assert_eq!(reg.pending()[0].kind, RequestKind::Upgrade);
        // 昇格の保留中も既存 role のまま
        assert_eq!(reg.device("nDEV1").unwrap().role, DeviceRole::Observe);

        reg.approve("nDEV1", None).expect("昇格承認");
        assert_eq!(reg.device("nDEV1").unwrap().role, DeviceRole::Interact);
    }

    #[test]
    fn 同一roleの再要求はalready_registered() {
        let (_dir, mut reg) = temp_registry("same-role");
        reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Observe);
        reg.approve("nDEV1", None).expect("承認");
        let resp = reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Observe);
        assert_eq!(resp["status"], "already_registered");
        assert!(reg.pending().is_empty());
    }

    #[test]
    fn revokeで登録が消えて永続化される() {
        let (dir, mut reg) = temp_registry("revoke");
        reg.request_pairing(&who("nDEV1"), "x", DeviceRole::Manage);
        reg.approve("nDEV1", None).expect("承認");
        reg.revoke("nDEV1").expect("revoke");
        assert!(reg.device("nDEV1").is_none());

        // 再オープンしても消えたまま（devices.json 反映確認）
        let reg2 = DeviceRegistry::open(dir.path()).expect("再オープン");
        assert!(reg2.device("nDEV1").is_none());
    }

    #[test]
    fn 永続化と再読み込みでデバイスが保持される() {
        let (dir, mut reg) = temp_registry("persist");
        reg.request_pairing(&who("nDEV1"), "iPhone", DeviceRole::Interact);
        reg.approve("nDEV1", None).expect("承認");

        let reg2 = DeviceRegistry::open(dir.path()).expect("再オープン");
        let d = reg2.device("nDEV1").expect("永続化されている");
        assert_eq!(d.name, "iPhone");
        assert_eq!(d.role, DeviceRole::Interact);
    }

    #[test]
    fn 破損したdevicesjsonはエラーになる() {
        let dir = TempDir::new("corrupt");
        std::fs::write(dir.path().join("devices.json"), "{ broken").expect("write");
        assert!(DeviceRegistry::open(dir.path()).is_err());
    }

    #[test]
    fn interact_sessionはidle_timeoutで期限切れになる() {
        let dir = TempDir::new("session-idle");
        let mut reg = DeviceRegistry::open(dir.path()).expect("open");
        reg.set_idle_timeout(Duration::from_secs(0));

        assert!(reg.record_input("nDEV1"), "初回は session 開始");
        // timeout=0 なので即期限切れ → 次も新 session
        assert!(reg.record_input("nDEV1"), "idle 経過後は再び session 開始");
        let expired = reg.sweep_idle_sessions();
        assert_eq!(expired, vec!["nDEV1".to_string()]);
        assert!(reg.sweep_idle_sessions().is_empty(), "二重回収しない");
    }

    #[test]
    fn interact_sessionは連続入力では開始扱いにならない() {
        let (_dir, mut reg) = temp_registry("session-cont");
        assert!(reg.record_input("nDEV1"));
        assert!(
            !reg.record_input("nDEV1"),
            "timeout 内の連続入力は同一 session"
        );
    }

    #[test]
    fn roleのparseとas_strが往復する() {
        for role in [
            DeviceRole::Observe,
            DeviceRole::Interact,
            DeviceRole::Manage,
            DeviceRole::Admin,
        ] {
            assert_eq!(DeviceRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(DeviceRole::parse("root"), None);
        assert!(DeviceRole::Observe < DeviceRole::Interact);
        assert!(DeviceRole::Interact < DeviceRole::Manage);
        assert!(DeviceRole::Manage < DeviceRole::Admin);
    }

    #[test]
    fn 監査ログはjsonlで追記されローテートする() {
        let (dir, reg) = temp_registry("audit");
        reg.audit("conn_open", "nDEV1", "iPhone", json!({ "route": "/ws" }));
        reg.audit("input", "nDEV1", "iPhone", json!({ "bytes": 12 }));
        let content = std::fs::read_to_string(dir.path().join("audit.log")).expect("read");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).expect("JSONL");
        assert_eq!(first["event"], "conn_open");
        assert_eq!(first["device_id"], "nDEV1");
        assert_eq!(first["route"], "/ws");
        let second: Value = serde_json::from_str(lines[1]).expect("JSONL");
        assert_eq!(second["bytes"], 12);

        // ローテート: 閾値超えの状態で書き込むと .1 へ退避される
        let big = "x".repeat(AUDIT_LOG_MAX_BYTES as usize + 1);
        std::fs::write(dir.path().join("audit.log"), &big).expect("write");
        reg.audit("conn_close", "nDEV1", "iPhone", json!({}));
        let rotated = dir.path().join("audit.log.1");
        assert!(rotated.exists(), "ローテート先が存在する");
        let fresh = std::fs::read_to_string(dir.path().join("audit.log")).expect("read");
        assert_eq!(fresh.lines().count(), 1, "新ログは 1 行から始まる");
    }
}

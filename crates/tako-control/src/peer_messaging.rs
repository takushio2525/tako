//! claude の Cross-Session Messaging による指示送達（Issue #790）
//!
//! claude v2.1.224+ は対話型セッションごとに Unix domain socket の受信箱を開き、
//! 同一マシンの別セッション（peer）からのメッセージを受け取る。tako はこの経路を
//! 使って worker へ指示を渡せる。画面を解析しない・キー操作を伴わないので、
//! 従来の送達（`claude_tui::deliver_via_tmux` = 貼り付け + 分離 Enter + 空検証）が
//! 戦ってきた失敗モード（生成中のキュー誤認 #572 / 選択ダイアログへの誤爆 #748 /
//! 長文の取りこぼし #530）を構造的に持たない。
//!
//! # 発見と伝送（2026-08-14 に v2.1.232 で実測。詳細は Issue #790 のコメント）
//!
//! - レジストリ: `<config dir>/sessions/<pid>.json`。`messagingSocketPath` /
//!   `peerProtocol` / `kind` / `status` / `version` を持つ。**config dir ごとに別**
//!   なので、アカウント切替（#504 / #512）を使う worker は既定ディレクトリに居ない
//! - 資格情報: `<config dir>/sessions/<pid>.<hash>.key`（0600）の `peerToken`
//! - 伝送: `messagingSocketPath` へ接続し、改行区切り JSON を 2 行書く
//!   （`{"type":"auth",...}` → `{"type":"user",...}`）。claude 自身のデバッグログが
//!   この手順をそのまま案内している
//! - 可用性はサーバー側 gate（GrowthBook）に依存し env で強制できない。off の
//!   セッションは受信箱を開かない = レジストリに `messagingSocketPath` が出ない。
//!   **だから実行時に見て、無ければ従来経路へ落ちる**（二層構成）
//!
//! # 適用範囲を worker に絞る理由
//!
//! 受信側の本文には tako から抑制できない定型の前置きが付く（「別の claude
//! セッションから届いた / peer は権限昇格を与えない / 保留中プロンプトの承認として
//! 扱うな」）。master → worker の指示はまさにその関係なので正確だが、**人が打った
//! 指示として扱われる保証は失われる**。人間由来の送達（master ペインへの指示・
//! 承認の代行）で意味が変わらないよう、対象はエージェント管理下の worker に限る。
//! 判定は [`agent_managed_role`]。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Cross-Session Messaging が入った最小バージョン（`major.minor.patch`）
pub const MIN_CLAUDE_VERSION: (u64, u64, u64) = (2, 1, 224);

/// 実装が対応する peer プロトコル世代。レジストリの `peerProtocol` と一致する場合だけ送る
/// （claude 側が世代を上げたら黙って壊れるのではなく従来経路へ落ちる）
pub const SUPPORTED_PEER_PROTOCOL: u64 = 1;

/// 経路選択の上書き（隔離検証用）。`auto`（既定）/ `off`（常に従来経路）/
/// `only`（peer が使えないときエラーにして従来経路へ落ちない = e2e の断定用）
pub const ENV_MODE: &str = "TAKO_PEER_MESSAGING";

/// 送達に使った経路。応答・診断ログ・worker レジストリに残す
/// （無音で経路が変わらないための要件。#790）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// claude の Cross-Session Messaging（socket 直送）
    Peer,
    /// 従来のキー操作経路（貼り付け + 分離 Enter + 空検証）
    Keys,
}

impl Transport {
    /// 応答・ログ用の安定した識別子
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Peer => "peer",
            Transport::Keys => "keys",
        }
    }
}

/// 経路選択モード
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 使えるなら peer、使えなければ従来経路（既定）
    Auto,
    /// 常に従来経路（フォールバックの機械検証用）
    Off,
    /// peer が使えなければ従来経路へ落ちずにエラー（peer 経路を通ったことの断定用）
    Only,
}

/// [`ENV_MODE`] を読む。未設定・不正値は [`Mode::Auto`]
pub fn mode() -> Mode {
    mode_of(std::env::var(ENV_MODE).ok().as_deref())
}

/// [`mode`] の判定部分（env 読み出しを外した純粋版。テストが env を汚さない）
pub fn mode_of(raw: Option<&str>) -> Mode {
    match raw.map(str::trim) {
        Some("off") | Some("0") | Some("false") => Mode::Off,
        Some("only") => Mode::Only,
        _ => Mode::Auto,
    }
}

/// peer 経路が使えない理由。呼び出し側はこれを見て従来経路へ落ち、`code` をログに残す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// `TAKO_PEER_MESSAGING=off`
    Disabled,
    /// この OS では未実装（Windows。#467 / #515）
    Unsupported,
    /// 送達先の claude プロセスが特定できない
    NoClaudePid { session: String },
    /// レジストリに該当 pid のエントリが無い（受信箱を開いていない = gate off 等）
    NoRegistryEntry { pid: u32 },
    /// 受信箱を開いていない（`messagingSocketPath` が無い）
    NoSocketPath { pid: u32 },
    /// claude が古い
    OldVersion { version: String },
    /// 未対応のプロトコル世代
    UnsupportedProtocol { got: u64 },
    /// 対話型セッションでない（`claude -p` 等）
    NotInteractive { kind: String },
    /// socket が実在しない（セッション終了直後等）
    SocketMissing { path: PathBuf },
    /// 資格情報を読めない
    TokenUnavailable { note: String },
    /// エージェント管理下の worker ではない（人間由来の送達）
    NotAgentManaged,
}

impl Unavailable {
    /// 安定した理由コード（診断ログ・テストの判定に使う。本文は含めない）
    pub fn code(&self) -> &'static str {
        match self {
            Unavailable::Disabled => "disabled",
            Unavailable::Unsupported => "unsupported_platform",
            Unavailable::NoClaudePid { .. } => "no_claude_pid",
            Unavailable::NoRegistryEntry { .. } => "no_registry_entry",
            Unavailable::NoSocketPath { .. } => "no_socket_path",
            Unavailable::OldVersion { .. } => "old_version",
            Unavailable::UnsupportedProtocol { .. } => "unsupported_protocol",
            Unavailable::NotInteractive { .. } => "not_interactive",
            Unavailable::SocketMissing { .. } => "socket_missing",
            Unavailable::TokenUnavailable { .. } => "token_unavailable",
            Unavailable::NotAgentManaged => "not_agent_managed",
        }
    }

    /// 人向けの説明（診断ログ用。ペイン内容・送信テキストは含めない）
    pub fn note(&self) -> String {
        match self {
            Unavailable::Disabled => format!("{ENV_MODE}=off"),
            Unavailable::Unsupported => "この OS では未対応".into(),
            Unavailable::NoClaudePid { session } => {
                format!("セッション {session} 配下に claude プロセスが見つからない")
            }
            Unavailable::NoRegistryEntry { pid } => {
                format!("pid={pid} のセッションレジストリが無い（受信箱を開いていない）")
            }
            Unavailable::NoSocketPath { pid } => {
                format!("pid={pid} は受信箱を開いていない（messagingSocketPath 無し）")
            }
            Unavailable::OldVersion { version } => {
                let (ma, mi, pa) = MIN_CLAUDE_VERSION;
                format!("claude {version} は Cross-Session Messaging 未対応（{ma}.{mi}.{pa} 以降が必要）")
            }
            Unavailable::UnsupportedProtocol { got } => {
                format!("peerProtocol={got} は未対応（対応 {SUPPORTED_PEER_PROTOCOL}）")
            }
            Unavailable::NotInteractive { kind } => {
                format!("対話型セッションでない（kind={kind}）")
            }
            Unavailable::SocketMissing { .. } => "受信箱の socket が実在しない".into(),
            Unavailable::TokenUnavailable { note } => format!("資格情報を読めない: {note}"),
            Unavailable::NotAgentManaged => {
                "エージェント管理下の worker ではない（人間由来の送達は従来経路）".into()
            }
        }
    }
}

/// セッションレジストリ 1 件（`<config dir>/sessions/<pid>.json`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSession {
    /// claude プロセスの pid
    pub pid: u32,
    /// claude の session_id（transcript 参照キー）
    pub session_id: String,
    /// claude のバージョン文字列
    pub version: String,
    /// peer プロトコル世代
    pub peer_protocol: u64,
    /// `interactive` / `print` 等
    pub kind: String,
    /// claude 自身の申告状態（`idle` / `busy` / `waiting`（ダイアログ待ち）/ `shell`）
    pub status: Option<String>,
    /// プロセス起動時刻の文字列（pid 再利用の同一性検証に使う）
    pub proc_start: Option<String>,
    /// 受信箱の socket
    pub socket_path: Option<PathBuf>,
    /// このエントリが置かれていた config dir
    pub config_dir: PathBuf,
}

/// 送達可能と判定した宛先（資格情報つき）
#[derive(Debug, Clone)]
pub struct PeerTarget {
    /// 宛先セッション
    pub session: PeerSession,
    /// 受信箱の socket（gate 済みなので確定している）
    pub socket_path: PathBuf,
    /// 認証トークン。**ログ・エラー文へ出さない**
    token: String,
}

impl PeerTarget {
    /// 認証行 + 本文行の 2 行（改行区切り JSON）。テストから中身を検証できるよう分離してある
    pub fn payload(&self, text: &str) -> String {
        let auth = serde_json::json!({ "type": "auth", "token": self.token });
        let message = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": text },
        });
        format!("{auth}\n{message}\n")
    }
}

/// `major.minor.patch` を数値へ。先頭 3 要素だけ見る（`2.1.232 (Claude Code)` 等も通す）
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let head = version.split_whitespace().next()?;
    let mut parts = head.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}

/// バージョンが Cross-Session Messaging に対応しているか
pub fn version_supported(version: &str) -> bool {
    parse_version(version).is_some_and(|v| v >= MIN_CLAUDE_VERSION)
}

/// レジストリ JSON 1 件を [`PeerSession`] へ。必須項目が欠けていれば None
pub fn parse_entry(value: &Value, config_dir: &Path) -> Option<PeerSession> {
    let pid = value["pid"].as_u64()?;
    Some(PeerSession {
        pid: u32::try_from(pid).ok()?,
        session_id: value["sessionId"].as_str().unwrap_or_default().to_string(),
        version: value["version"].as_str().unwrap_or_default().to_string(),
        peer_protocol: value["peerProtocol"].as_u64().unwrap_or(0),
        kind: value["kind"].as_str().unwrap_or_default().to_string(),
        status: value["status"].as_str().map(|s| s.to_string()),
        proc_start: value["procStart"].as_str().map(|s| s.to_string()),
        socket_path: value["messagingSocketPath"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from),
        config_dir: config_dir.to_path_buf(),
    })
}

/// 与えられた config ディレクトリ群から pid のレジストリを探す
/// （走査対象を引数で受け取る純粋版。テストから HOME を触らずに検証できる）
pub fn find_entry_in(dirs: &[PathBuf], pid: u32) -> Option<PeerSession> {
    for dir in dirs {
        let path = dir.join("sessions").join(format!("{pid}.json"));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(entry) = parse_entry(&value, dir) {
            // pid だけで引いているので、レジストリ側の pid が食い違うファイルは信じない
            if entry.pid == pid {
                return Some(entry);
            }
        }
    }
    None
}

/// 送達可能かの判定（ファイル I/O を伴わない部分）。
/// socket の実在確認は [`gate`] 側で行う
pub fn gate_metadata(session: &PeerSession) -> Result<&Path, Unavailable> {
    if !version_supported(&session.version) {
        return Err(Unavailable::OldVersion {
            version: session.version.clone(),
        });
    }
    if session.peer_protocol != SUPPORTED_PEER_PROTOCOL {
        return Err(Unavailable::UnsupportedProtocol {
            got: session.peer_protocol,
        });
    }
    if session.kind != "interactive" {
        return Err(Unavailable::NotInteractive {
            kind: session.kind.clone(),
        });
    }
    session
        .socket_path
        .as_deref()
        .ok_or(Unavailable::NoSocketPath { pid: session.pid })
}

/// 送達可能かの判定（socket の実在まで見る）
pub fn gate(session: &PeerSession) -> Result<PathBuf, Unavailable> {
    let socket = gate_metadata(session)?;
    if !socket.exists() {
        return Err(Unavailable::SocketMissing {
            path: socket.to_path_buf(),
        });
    }
    Ok(socket.to_path_buf())
}

/// 資格情報（`peerToken`）を読む。`<config dir>/sessions/<pid>.<hash>.key`。
/// `procStart` が食い違う鍵は pid 再利用の残骸なので使わない
pub fn read_peer_token(session: &PeerSession) -> Result<String, Unavailable> {
    let dir = session.config_dir.join("sessions");
    let entries = std::fs::read_dir(&dir).map_err(|e| Unavailable::TokenUnavailable {
        note: format!("sessions ディレクトリを読めない: {e}"),
    })?;
    let prefix = format!("{}.", session.pid);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) || !name.ends_with(".key") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        // pid 再利用の残骸を弾く（レジストリ側と起動時刻が一致するものだけ使う）
        if let (Some(key_start), Some(reg_start)) =
            (value["procStart"].as_str(), session.proc_start.as_deref())
        {
            if key_start != reg_start {
                continue;
            }
        }
        if let Some(token) = value["peerToken"].as_str().filter(|t| !t.is_empty()) {
            return Ok(token.to_string());
        }
    }
    Err(Unavailable::TokenUnavailable {
        note: format!("pid={} の鍵ファイルが無い", session.pid),
    })
}

/// pid から宛先を解決する（走査対象を引数で受け取る純粋版）
pub fn resolve_in(dirs: &[PathBuf], pid: u32) -> Result<PeerTarget, Unavailable> {
    if !cfg!(unix) {
        return Err(Unavailable::Unsupported);
    }
    let session = find_entry_in(dirs, pid).ok_or(Unavailable::NoRegistryEntry { pid })?;
    let socket_path = gate(&session)?;
    let token = read_peer_token(&session)?;
    Ok(PeerTarget {
        session,
        socket_path,
        token,
    })
}

/// バックエンド tmux セッション名から宛先を解決する。
/// claude の pid は `stale_binary::find_claude_pid_for_backend`（tmux pane_pid →
/// 子孫辿り）で求めるので `claude agents --json` に依存しない
pub fn resolve_for_backend(backend_session: &str) -> Result<PeerTarget, Unavailable> {
    let pid =
        crate::stale_binary::find_claude_pid_for_backend(backend_session).ok_or_else(|| {
            Unavailable::NoClaudePid {
                session: backend_session.to_string(),
            }
        })?;
    resolve_in(&crate::transcript::claude_config_dirs(), pid)
}

/// 受信箱へ 1 通送る。**成功したらフォールバックしてはならない**
/// （受信側が読んだ後に従来経路でも送ると二重投函になる）
#[cfg(unix)]
pub fn send(target: &PeerTarget, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(&target.socket_path)
        .map_err(|e| format!("受信箱へ接続できない: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("送信タイムアウトを設定できない: {e}"))?;
    let payload = target.payload(text);
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("受信箱へ書き込めない: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("受信箱への書き込みを flush できない: {e}"))?;
    // 書き終わりを相手へ伝える（受信側は行単位で読み、EOF で残りを処理する）
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| format!("送信方向を閉じられない: {e}"))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn send(_target: &PeerTarget, _text: &str) -> Result<(), String> {
    Err("この OS では Cross-Session Messaging に未対応".into())
}

/// 送達の確認結果（transcript 由来）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// ターンとして取り込まれた（`origin.kind == "peer"`）
    Delivered,
    /// 生成中・ダイアログ中でキューに入った（ターン終了後に読まれる）
    Queued,
    /// 期限内に痕跡を確認できなかった（送信自体は成功している）
    Unconfirmed,
}

impl Verification {
    /// 応答・ログ用の安定した識別子
    pub fn as_str(self) -> &'static str {
        match self {
            Verification::Delivered => "delivered",
            Verification::Queued => "queued",
            Verification::Unconfirmed => "unconfirmed",
        }
    }

    /// 受信側に届いたと言えるか（キュー投函も届いている）
    pub fn is_received(self) -> bool {
        matches!(self, Verification::Delivered | Verification::Queued)
    }
}

/// transcript のレコード 1 行が peer 送達の痕跡かを判定する（純粋関数）。
///
/// 実測した 3 形態（Issue #790）:
/// - `type:"user"`, `isMeta:true`, `origin.kind == "peer"` … ターンとして処理された
/// - `type:"attachment"`, `attachment.origin.kind == "peer"` … 進行中ターンへ折り込まれた
/// - `type:"queue-operation"`, `operation == "enqueue"` … 生成中・ダイアログ中でキュー投函
///
/// `origin` を見ない 3 番目は「peer とは限らない」ので [`Verification::Queued`] に留める
pub fn classify_record(record: &Value) -> Option<Verification> {
    match record["type"].as_str()? {
        "user" if record["origin"]["kind"].as_str() == Some("peer") => {
            Some(Verification::Delivered)
        }
        "attachment" if record["attachment"]["origin"]["kind"].as_str() == Some("peer") => {
            Some(Verification::Delivered)
        }
        "queue-operation" if record["operation"].as_str() == Some("enqueue") => {
            Some(Verification::Queued)
        }
        _ => None,
    }
}

/// transcript の行群から peer 送達の痕跡を探す（純粋関数）。
/// **渡すのは送信後に追記された行だけ**（[`TranscriptCursor`] が切り出す）
pub fn verify_in_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Verification {
    let mut best = Verification::Unconfirmed;
    for line in lines {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match classify_record(&record) {
            Some(Verification::Delivered) => return Verification::Delivered,
            // キュー投函は見つけても走査を続ける（同じターン内に本命が出ることがある）
            Some(Verification::Queued) => best = Verification::Queued,
            _ => {}
        }
    }
    best
}

/// 送信前に控える transcript の読み取り位置。
///
/// 時刻文字列で「今回の痕跡か」を判定しない: `sessions::now_iso()` は秒精度
/// （`…:21Z`）で transcript はミリ秒（`…:21.218Z`）なので、辞書順では同じ秒の
/// レコードが送信時刻より前に並んでしまう。**ファイル長を控えて追記分だけ読む**方が
/// 正確で、時計にもフォーマットにも依存しない
#[derive(Debug, Clone)]
pub struct TranscriptCursor {
    /// transcript のパス（送信前に見つからなければ None = 初回メッセージで作られる）
    path: Option<PathBuf>,
    /// 送信前のファイル長
    len: u64,
    /// 宛先の session_id（送信後に transcript が作られた場合の再探索用）
    session_id: String,
}

impl TranscriptCursor {
    /// 送信の直前に呼ぶ
    pub fn capture(session_id: &str) -> Self {
        let path = if session_id.is_empty() {
            None
        } else {
            crate::transcript::find_transcript(session_id)
        };
        let len = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        Self {
            path,
            len,
            session_id: session_id.to_string(),
        }
    }

    /// 送信後に追記された分を読んで痕跡を探す
    fn poll(&mut self) -> Verification {
        if self.path.is_none() && !self.session_id.is_empty() {
            // 初回メッセージで transcript が作られた場合はここで拾う（追記分 = 全文）
            self.path = crate::transcript::find_transcript(&self.session_id);
        }
        let Some(path) = self.path.as_ref() else {
            return Verification::Unconfirmed;
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Verification::Unconfirmed;
        };
        // compact 等でファイルが縮んだ場合は全文を対象にする（控えた位置は無効）
        let appended = if (raw.len() as u64) >= self.len {
            &raw[self.len as usize..]
        } else {
            raw.as_str()
        };
        verify_in_lines(appended.lines())
    }
}

/// 送達後に transcript を見て受信を確認する。`deadline` まで短間隔でポーリングする
pub fn verify_delivered(
    cursor: &mut TranscriptCursor,
    deadline: std::time::Instant,
) -> Verification {
    let mut best = Verification::Unconfirmed;
    loop {
        match cursor.poll() {
            Verification::Delivered => return Verification::Delivered,
            Verification::Queued => best = Verification::Queued,
            Verification::Unconfirmed => {}
        }
        if std::time::Instant::now() >= deadline {
            return best;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// このペインへの送達を peer 経路の対象とみなすか（#790）。
///
/// 対象はエージェント管理下の worker のみ。master / solo / 素のペインへの送達は
/// 人間由来（指示・承認の代行）であり、「別セッションからのメッセージ」という
/// 前置きが付くと意味が変わるため従来経路に残す
pub fn agent_managed_role(role: Option<&str>) -> bool {
    role.is_some_and(|r| r.starts_with("orchestrator-worker") || r.starts_with("worker:"))
}

/// バックエンドセッションが worker レジストリ（#390）に active で載っているか。
/// ペインが解決できない経路（`spawn_tmux_delivery`）で「worker かどうか」を知る材料
pub fn backend_is_registered_worker(backend_session: &str) -> bool {
    crate::orchestrator::registry::WorkerRegistry::load()
        .map(|reg| {
            reg.workers
                .values()
                .any(|e| e.is_active() && e.tmux_session.as_deref() == Some(backend_session))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(overrides: Value) -> PeerSession {
        let mut base = json!({
            "pid": 4242,
            "sessionId": "11111111-2222-3333-4444-555555555555",
            "version": "2.1.232",
            "peerProtocol": 1,
            "kind": "interactive",
            "status": "idle",
            "procStart": "Fri Aug 14 12:59:27 2026",
            "messagingSocketPath": "/tmp/cc-socks/4242.sock",
        });
        if let (Some(base), Some(over)) = (base.as_object_mut(), overrides.as_object()) {
            for (k, v) in over {
                if v.is_null() {
                    base.remove(k);
                } else {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
        parse_entry(&base, Path::new("/cfg")).expect("必須項目つき")
    }

    #[test]
    fn バージョン判定は下限で切る() {
        assert!(version_supported("2.1.224"));
        assert!(version_supported("2.1.232 (Claude Code)"));
        assert!(version_supported("2.2.0"));
        assert!(version_supported("3.0.1"));
        assert!(!version_supported("2.1.223"));
        assert!(!version_supported("2.0.999"));
        assert!(!version_supported(""));
        assert!(!version_supported("not-a-version"));
    }

    #[test]
    fn レジストリの必須項目を読む() {
        let s = entry(json!({}));
        assert_eq!(s.pid, 4242);
        assert_eq!(s.peer_protocol, 1);
        assert_eq!(s.kind, "interactive");
        assert_eq!(s.status.as_deref(), Some("idle"));
        assert_eq!(
            s.socket_path.as_deref(),
            Some(Path::new("/tmp/cc-socks/4242.sock"))
        );
        assert_eq!(s.config_dir, Path::new("/cfg"));
    }

    #[test]
    fn 受信箱を開いていないセッションは対象外() {
        // messagingSocketPath 無し = サーバー側 gate が off（bind していない）
        let s = entry(json!({ "messagingSocketPath": null }));
        assert_eq!(
            gate_metadata(&s).unwrap_err(),
            Unavailable::NoSocketPath { pid: 4242 }
        );
    }

    #[test]
    fn 古い版と未対応世代と非対話は対象外() {
        assert_eq!(
            gate_metadata(&entry(json!({ "version": "2.1.220" }))).unwrap_err(),
            Unavailable::OldVersion {
                version: "2.1.220".into()
            }
        );
        assert_eq!(
            gate_metadata(&entry(json!({ "peerProtocol": 2 }))).unwrap_err(),
            Unavailable::UnsupportedProtocol { got: 2 }
        );
        assert_eq!(
            gate_metadata(&entry(json!({ "kind": "print" }))).unwrap_err(),
            Unavailable::NotInteractive {
                kind: "print".into()
            }
        );
    }

    #[test]
    fn 理由コードは安定していて本文を含まない() {
        let cases = [
            (Unavailable::Disabled, "disabled"),
            (Unavailable::NotAgentManaged, "not_agent_managed"),
            (Unavailable::NoRegistryEntry { pid: 1 }, "no_registry_entry"),
            (
                Unavailable::SocketMissing { path: "/x".into() },
                "socket_missing",
            ),
        ];
        for (u, code) in cases {
            assert_eq!(u.code(), code);
            assert!(!u.note().is_empty());
        }
    }

    #[test]
    fn 送信内容は認証行と本文行の二行() {
        let target = PeerTarget {
            session: entry(json!({})),
            socket_path: PathBuf::from("/tmp/cc-socks/4242.sock"),
            token: "tok-abc".into(),
        };
        let payload = target.payload("こんにちは\n2 行目");
        let mut lines = payload.lines();
        let auth: Value = serde_json::from_str(lines.next().expect("認証行")).expect("JSON");
        assert_eq!(auth["type"], "auth");
        assert_eq!(auth["token"], "tok-abc");
        let body: Value = serde_json::from_str(lines.next().expect("本文行")).expect("JSON");
        assert_eq!(body["type"], "user");
        assert_eq!(body["message"]["role"], "user");
        // 改行を含む本文が 1 行の JSON に収まる（改行区切りプロトコルを壊さない）
        assert_eq!(body["message"]["content"], "こんにちは\n2 行目");
        assert!(lines.next().is_none(), "余分な行を送らない");
        assert!(payload.ends_with('\n'), "最終行も改行で閉じる");
    }

    #[test]
    fn peer_の対象はエージェント管理下の_worker_だけ() {
        assert!(agent_managed_role(Some("orchestrator-worker:tako")));
        assert!(agent_managed_role(Some("orchestrator-worker:tako:790")));
        assert!(agent_managed_role(Some("worker:tako")));
        // 人間が話す相手は従来経路のまま
        assert!(!agent_managed_role(Some("master:default")));
        assert!(!agent_managed_role(Some("orchestrator-master:st761")));
        assert!(!agent_managed_role(Some("solo")));
        assert!(!agent_managed_role(None));
    }

    #[test]
    fn transcript_の三形態を分類する() {
        // ターンとして処理された
        assert_eq!(
            classify_record(&json!({
                "type": "user", "isMeta": true,
                "origin": {"kind": "peer", "verifiedPeerPid": 9},
            })),
            Some(Verification::Delivered)
        );
        // 進行中ターンへ折り込まれた（ダイアログ中・生成中）
        assert_eq!(
            classify_record(&json!({
                "type": "attachment",
                "attachment": {"type": "queued_command", "origin": {"kind": "peer"}},
            })),
            Some(Verification::Delivered)
        );
        // キュー投函
        assert_eq!(
            classify_record(&json!({"type": "queue-operation", "operation": "enqueue"})),
            Some(Verification::Queued)
        );
        // 人が打った指示は痕跡ではない
        assert_eq!(
            classify_record(&json!({
                "type": "user", "origin": {"kind": "human"}, "promptSource": "typed",
            })),
            None
        );
        assert_eq!(
            classify_record(&json!({"type": "queue-operation", "operation": "remove"})),
            None
        );
        assert_eq!(classify_record(&json!({"type": "assistant"})), None);
    }

    #[test]
    fn 追記分に痕跡が無ければ未確認() {
        let human = json!({"type": "user", "origin": {"kind": "human"}}).to_string();
        assert_eq!(
            verify_in_lines([human.as_str()].into_iter()),
            Verification::Unconfirmed
        );
        assert_eq!(
            verify_in_lines(std::iter::empty()),
            Verification::Unconfirmed
        );
        // 壊れた行が混ざっても走査を止めない
        let peer = json!({"type": "user", "origin": {"kind": "peer"}}).to_string();
        assert_eq!(
            verify_in_lines(["{壊れた", peer.as_str()].into_iter()),
            Verification::Delivered
        );
    }

    #[test]
    fn キュー投函だけならキューと報告する() {
        let queued = json!({"type": "queue-operation", "operation": "enqueue"}).to_string();
        assert_eq!(
            verify_in_lines([queued.as_str()].into_iter()),
            Verification::Queued
        );
        assert!(Verification::Queued.is_received());
        assert!(!Verification::Unconfirmed.is_received());
    }

    #[test]
    fn 検証は送信前の行を今回の証拠にしない() {
        // 前回の送達（peer）が既に書かれている transcript を用意する
        let dir = temp_dir("cursor");
        let path = dir.join("session.jsonl");
        let peer = json!({"type": "user", "origin": {"kind": "peer"}}).to_string();
        let queued = json!({"type": "queue-operation", "operation": "enqueue"}).to_string();
        let before = format!("{peer}\n{queued}\n{peer}\n");
        std::fs::write(&path, &before).expect("書ける");

        // 送信直前のカーソル（session_id は使わずパスを直に控える経路をテストする）
        let mut cursor = TranscriptCursor {
            path: Some(path.clone()),
            len: std::fs::metadata(&path).expect("stat").len(),
            session_id: String::new(),
        };
        assert_eq!(
            cursor.poll(),
            Verification::Unconfirmed,
            "送信前の痕跡（前回の送達）は今回の証拠にしない"
        );

        // 送信後に追記されたぶんだけが証拠になる
        std::fs::write(&path, format!("{before}{queued}\n")).expect("書ける");
        assert_eq!(cursor.poll(), Verification::Queued);

        // compact でファイルが縮んだら控えた位置は無効なので全文を対象にする。
        // 追記分だけを見ていたら（縮んだ後の追記は無いので）Unconfirmed になる
        std::fs::write(&path, format!("{peer}\n")).expect("書ける");
        assert!(
            std::fs::metadata(&path).expect("stat").len() < cursor.len,
            "縮んだ状態を作れている"
        );
        assert_eq!(cursor.poll(), Verification::Delivered);
        remove_temp_dir(&dir);
    }

    /// テスト用の一時ディレクトリ。`remove_temp_dir` で必ず捨てる
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-790-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        dir
    }

    /// 一時ディレクトリ以外を消さない（worker テストの実環境破壊事故の再発防止）
    fn remove_temp_dir(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 資格情報は起動時刻が食い違う残骸を使わない() {
        let dir = temp_dir("token");
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).expect("作成");
        // pid 再利用の残骸（procStart が違う）
        std::fs::write(
            sessions.join("4242.aaaa.key"),
            json!({"peerToken": "stale", "procStart": "Thu Aug 13 00:00:00 2026"}).to_string(),
        )
        .expect("書ける");
        let mut session = entry(json!({}));
        session.config_dir = dir.clone();
        assert!(matches!(
            read_peer_token(&session),
            Err(Unavailable::TokenUnavailable { .. })
        ));
        // 起動時刻が一致する鍵は使う
        std::fs::write(
            sessions.join("4242.bbbb.key"),
            json!({"peerToken": "fresh", "procStart": "Fri Aug 14 12:59:27 2026"}).to_string(),
        )
        .expect("書ける");
        assert_eq!(read_peer_token(&session).expect("読める"), "fresh");
        remove_temp_dir(&dir);
    }

    #[test]
    fn レジストリ探索は_config_dir_を順に見る() {
        let dir = temp_dir("registry");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(b.join("sessions")).expect("作成");
        std::fs::write(
            b.join("sessions").join("777.json"),
            json!({
                "pid": 777, "sessionId": "s", "version": "2.1.232", "peerProtocol": 1,
                "kind": "interactive", "messagingSocketPath": "/tmp/cc-socks/777.sock",
            })
            .to_string(),
        )
        .expect("書ける");
        // 既定（a）に無く 2 番目（b = アカウント用 config dir）にある場合も見つける
        let found = find_entry_in(&[a, b.clone()], 777).expect("見つかる");
        assert_eq!(found.pid, 777);
        assert_eq!(found.config_dir, b);
        assert!(find_entry_in(&[dir.join("c")], 777).is_none());
        remove_temp_dir(&dir);
    }

    #[test]
    fn モードは環境変数の値で切り替わる() {
        // env を触らずに判定部分を直接検証する（テスト間の env 競合を避ける）
        assert_eq!(mode_of(None), Mode::Auto);
        assert_eq!(mode_of(Some("off")), Mode::Off);
        assert_eq!(mode_of(Some("only")), Mode::Only);
        assert_eq!(mode_of(Some("auto")), Mode::Auto);
        assert_eq!(mode_of(Some("なにか")), Mode::Auto);
        assert_eq!(mode_of(Some(" off ")), Mode::Off);
    }
}

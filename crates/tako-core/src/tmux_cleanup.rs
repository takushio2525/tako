//! tmux_cleanup — orphan 掃除を「誰がそのソケットを所有しているか」で決める（Issue #1187）
//!
//! `cleanup_orphans`（[`crate::tmux_backend`]）は **1 つの tmux ソケットの中**で
//! attached / grouped / protected を見て安全性を担保する。ここが担保するのはその外側
//! ＝ **そのソケットを他の生きた tako-app が使っていないか**。
//!
//! ## なぜプロセス名だけで判定してはいけないか
//!
//! 旧実装は `ports::other_tako_running()`（プロセス名が `tako-app` か）だけを見て
//! 掃除ごと止めていた。隔離インスタンス（`TAKO_ISOLATED=1` = `tako-iso-<pid>` ソケット）は
//! **定義上そのインスタンス専用のソケット**を使うので本番の掃除を止める理由が無いのに、
//! 検証用インスタンスを 1 つ立てただけで本番側の `tako tmux cleanup` が無言で無効化されていた
//! （#1187）。逆に、**同じソケットを共有する**別インスタンスが生きている場合は、その
//! detached セッション（退避ペイン等）を巻き込む危険が本当にあるので見送らねばならない
//! （#113 の事故クラス）。判定材料はプロセス名ではなく**ソケットの所有者**である。
//!
//! ## 相手のソケット名をどうやって知るか
//!
//! 相手プロセスの**初期環境変数**（macOS は `KERN_PROCARGS2` = [`crate::ports::process_env_vars`]）
//! から `TAKO_TMUX_SOCKET` / `TAKO_ISOLATED` / `TAKO_SELF_TEST` を読み、tako-app 自身の
//! 一括隔離と**同じ順序**（[`resolve_socket_name`]）でソケット名を復元する。読めなければ
//! `None` = 不明として**見送る**（安全側に倒す）。

/// 掃除の呼び出し元。ガードの強さが変わる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupScope {
    /// 起動時の自動実行。#113 の猶予（`min_idle_secs`）に加えて
    /// 「他の tako-app が 1 つでも生きていれば見送る」を維持する
    /// （起動直後は layout.json 由来の protected が信用しきれないため）
    Startup,
    /// 明示操作（`tako tmux cleanup` / MCP `tako_tmux_cleanup`）。
    /// 見送るのは「対象ソケットを共有する相手が生きている」ときだけ
    Explicit,
}

/// 生きている別 tako-app インスタンス（自分は含まない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPeer {
    pub pid: u32,
    /// その相手が使う backend tmux ソケット名。`None` = 特定できなかった（安全側 = 見送る）
    pub socket: Option<String>,
}

/// 掃除を見送った理由。空配列と「掃除するものが無かった」を区別するために応答へ載せる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupSkip {
    /// セカンダリモード（protected が空でプライマリのセッションを全部 orphan と誤認する）
    Secondary,
    /// tmux 永続化 OFF
    PersistDisabled,
    /// バックエンドがアプリ終了をまたがない（tmux 不在等）
    BackendNotPersistent,
    /// 対象ソケットを共有する別 tako-app が生きている
    PeerSharesSocket { pids: Vec<u32>, socket: String },
    /// 別 tako-app のソケットを特定できない（環境を読めない）
    PeerSocketUnknown { pids: Vec<u32> },
    /// 起動時の自動実行で、別 tako-app が生きている（#113）
    OtherTakoRunning { pids: Vec<u32> },
}

impl CleanupSkip {
    /// 機械可読の理由コード（応答の `skipped`）
    pub fn code(&self) -> &'static str {
        match self {
            Self::Secondary => "secondary",
            Self::PersistDisabled => "persist_disabled",
            Self::BackendNotPersistent => "backend_not_persistent",
            Self::PeerSharesSocket { .. } => "peer_shares_socket",
            Self::PeerSocketUnknown { .. } => "peer_socket_unknown",
            Self::OtherTakoRunning { .. } => "other_tako_running",
        }
    }

    /// 人が読む理由（どの pid / ソケットが理由かを必ず含める）
    pub fn detail(&self) -> String {
        match self {
            Self::Secondary => {
                "セカンダリモード（多重起動の後発）では orphan 判定を行わない".to_string()
            }
            Self::PersistDisabled => {
                "tmux 永続化が無効（TAKO_PERSIST=0 等）なので掃除対象が無い".to_string()
            }
            Self::BackendNotPersistent => {
                "バックエンドがアプリ終了をまたがない（tmux 不在）ので掃除対象が無い".to_string()
            }
            Self::PeerSharesSocket { pids, socket } => format!(
                "別の tako-app（pid {}）が同じ tmux ソケット {socket} を使用中のため見送った",
                join_pids(pids)
            ),
            Self::PeerSocketUnknown { pids } => format!(
                "別の tako-app（pid {}）が使う tmux ソケットを特定できないため見送った",
                join_pids(pids)
            ),
            Self::OtherTakoRunning { pids } => format!(
                "起動時の自動クリーンアップは別の tako-app（pid {}）が生きている間は行わない（#113）",
                join_pids(pids)
            ),
        }
    }
}

fn join_pids(pids: &[u32]) -> String {
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// 掃除の結果。`killed` が空でも `skipped` を見れば「対象が無かった」と
/// 「見送った」を区別できる（#1187 の本質）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    /// 実際に対象にした tmux ソケット名（`--socket` の尊重を呼び出し側が確認できる）
    pub socket: String,
    pub killed: Vec<String>,
    pub skipped: Option<CleanupSkip>,
}

impl CleanupReport {
    pub fn killed(socket: impl Into<String>, killed: Vec<String>) -> Self {
        Self {
            socket: socket.into(),
            killed,
            skipped: None,
        }
    }

    pub fn skipped(socket: impl Into<String>, skip: CleanupSkip) -> Self {
        Self {
            socket: socket.into(),
            killed: Vec::new(),
            skipped: Some(skip),
        }
    }

    /// persist.log / 診断に残す 1 行。**見送りと実行のどちらも記録する**
    /// （#1187: 理由が誰にも見えないのが本質的な問題だった）
    pub fn log_line(&self, scope: CleanupScope) -> Option<String> {
        let origin = match scope {
            CleanupScope::Startup => "起動時",
            CleanupScope::Explicit => "明示操作",
        };
        match &self.skipped {
            Some(skip) => Some(format!(
                "tmux cleanup: {origin}・socket={} 見送り（{}）: {}",
                self.socket,
                skip.code(),
                skip.detail()
            )),
            None if !self.killed.is_empty() => Some(format!(
                "tmux cleanup: {origin}・socket={} で {} 件 kill: {}",
                self.socket,
                self.killed.len(),
                self.killed.join(", ")
            )),
            // 対象ゼロで見送りも無いときは書かない（起動毎にログを埋めない）
            None => None,
        }
    }
}

/// 掃除を見送るべきか（純粋関数）。`peers` は**自分以外**の生きた tako-app。
///
/// - [`CleanupScope::Startup`][]: 相手が 1 つでもいれば見送る（#113 の従来挙動を維持）
/// - [`CleanupScope::Explicit`][]: **対象ソケットを共有する**相手がいるとき、または
///   相手のソケットを特定できないときだけ見送る
pub fn blocker(
    scope: CleanupScope,
    target_socket: &str,
    peers: &[CleanupPeer],
) -> Option<CleanupSkip> {
    if peers.is_empty() {
        return None;
    }
    if scope == CleanupScope::Startup {
        return Some(CleanupSkip::OtherTakoRunning {
            pids: peers.iter().map(|p| p.pid).collect(),
        });
    }
    let shared: Vec<u32> = peers
        .iter()
        .filter(|p| p.socket.as_deref() == Some(target_socket))
        .map(|p| p.pid)
        .collect();
    if !shared.is_empty() {
        return Some(CleanupSkip::PeerSharesSocket {
            pids: shared,
            socket: target_socket.to_string(),
        });
    }
    let unknown: Vec<u32> = peers
        .iter()
        .filter(|p| p.socket.is_none())
        .map(|p| p.pid)
        .collect();
    if !unknown.is_empty() {
        return Some(CleanupSkip::PeerSocketUnknown { pids: unknown });
    }
    None
}

/// #1187 の A/B。`TAKO_1187_LEGACY=1` で**修正前の挙動**を再現する:
/// `--socket` を捨てて自分の backend 固定・他 tako-app が 1 つでもいれば見送り・
/// 見送りの理由を応答に載せない。3 層（app / dispatch / CLI）が同じ 1 実装を見る
pub fn legacy_1187() -> bool {
    std::env::var_os("TAKO_1187_LEGACY").is_some()
}

/// 隔離起動（`TAKO_ISOLATED=1`）のソケット名。**tako-app の一括隔離と 1 実装**
/// （main.rs 側もこの関数を呼ぶ。名前がズレると所有者判定が外れる）
pub fn isolated_socket_name(pid: u32) -> String {
    format!("tako-iso-{pid}")
}

/// セルフテスト（`TAKO_SELF_TEST`）のソケット名。同上
pub fn self_test_socket_name(pid: u32) -> String {
    format!("tako-st-{pid}")
}

/// `TAKO_ISOLATED` が有効を表す値か（main.rs の一括隔離と同じ判定）
pub fn is_isolated_value(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "on"))
}

/// あるプロセスの backend tmux ソケット名を、その**初期環境**から復元する（純粋関数）。
///
/// 順序は tako-app の一括隔離（`main`）と同じ:
/// `TAKO_TMUX_SOCKET` が設定済みならそれ（空文字は `socket_name()` と同じく既定へ）→
/// `TAKO_ISOLATED` → `TAKO_SELF_TEST` → 既定サーバー。
///
/// `env` は「その環境変数が設定されているか」を返す（`var_os` 相当。未設定 = `None`）
pub fn resolve_socket_name(pid: u32, env: impl Fn(&str) -> Option<String>) -> String {
    let explicit = env(crate::tmux_backend::SOCKET_ENV);
    let resolved = match explicit {
        // 設定済み（空文字も「設定済み」= 一括隔離は上書きしない）
        Some(_) => explicit,
        None if is_isolated_value(env("TAKO_ISOLATED").as_deref()) => {
            Some(isolated_socket_name(pid))
        }
        None if env("TAKO_SELF_TEST").is_some() => Some(self_test_socket_name(pid)),
        None => None,
    };
    // socket_name() と同じく、空文字は既定サーバーへ落ちる
    resolved
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::tmux_backend::DEFAULT_SOCKET.to_string())
}

/// 生きている別 tako-app インスタンスとその backend ソケットを集める。
/// ソケットは相手の初期環境から復元し、読めなければ `None`（= 見送りの根拠になる）
pub fn live_peers() -> Vec<CleanupPeer> {
    const KEYS: [&str; 3] = [
        crate::tmux_backend::SOCKET_ENV,
        "TAKO_ISOLATED",
        "TAKO_SELF_TEST",
    ];
    crate::ports::other_tako_pids()
        .into_iter()
        .map(|pid| {
            let socket = crate::ports::process_env_vars(pid, &KEYS)
                .map(|env| resolve_socket_name(pid, |k| env.get(k).cloned()));
            CleanupPeer { pid, socket }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// orphan 判定（#1188）— cleanup と find が同じ材料・同じ規則を見る
// ---------------------------------------------------------------------------

/// `list-sessions -F` に渡す書式。**順序を変えたら [`parse_session_row`] も直すこと**
pub const LIST_FORMAT: &str = "#{session_name}\t#{session_attached}\t#{session_grouped}\t#{session_group_size}\t#{session_activity}";

/// `list-sessions` の 1 行。数値が読めない古い tmux でも壊れないよう、
/// 解釈は [`SessionRow`] を作るときに 1 回だけ行う
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub name: String,
    pub attached: bool,
    /// `#{session_grouped}` = **グループに属しているか**。
    /// tmux はメンバーが 1 つになってもグループを消さないので、
    /// 「表示中ビューがあるか」の代わりには使えない（#1188 の根因）
    pub grouped_flag: bool,
    /// `#{session_group_size}` = グループのメンバー数。グループ無しは `None`
    pub group_size: Option<u32>,
    /// グループのメンバー数を読めなかった（空でないのに数値でない = 未知の tmux）
    pub group_size_unreadable: bool,
    pub activity: u64,
}

/// [`LIST_FORMAT`] の 1 行を解く（純粋関数）
pub fn parse_session_row(line: &str) -> Option<SessionRow> {
    let mut f = line.split('\t');
    let name = f.next()?.to_string();
    if name.is_empty() {
        return None;
    }
    let attached = f.next()? != "0";
    let grouped_flag = f.next()? != "0";
    let size_field = f.next().unwrap_or("");
    let group_size = size_field.parse::<u32>().ok();
    let group_size_unreadable = !size_field.is_empty() && group_size.is_none();
    let activity = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(SessionRow {
        name,
        attached,
        grouped_flag,
        group_size,
        group_size_unreadable,
        activity,
    })
}

/// そのセッションのグループに**生きた仲間**がいるか（= 表示中ビューが実在するか）。
///
/// #1188: 旧実装は `session_grouped != 0` を「表示中ビューがある」の意味で使っていたが、
/// tmux のセッショングループは**メンバーが 1 つになっても残る**ので、
/// `tako tmux open` で 1 度取り込んだセッションは以後永久に掃除対象から外れていた
/// （実測 tmux 3.6b: ビュー kill 後も `grouped=1` のまま `group_size=1`）。
/// メンバー数を見れば「いま仲間がいるか」を正しく答えられる。
///
/// `legacy` = #1188 の A/B（`TAKO_1188_LEGACY=1`）で旧判定を再現する
pub fn group_has_live_peer(row: &SessionRow, legacy: bool) -> bool {
    if legacy {
        return row.grouped_flag;
    }
    if row.group_size_unreadable {
        return true; // 読めない tmux では安全側（従来どおり触らない）
    }
    row.group_size.is_some_and(|n| n > 1)
}

/// orphan 判定の用途。守る材料は同じで、除外条件だけが違う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanPurpose {
    /// 掃除（`cleanup_orphans`）: attached は触らない・`min_idle_secs` の猶予がある
    Cleanup,
    /// 発見（`find_orphans` = 起動時の自動復帰）: ラッパーは復帰対象にしない
    Recover,
}

/// この行が orphan（= 取り残されたバックエンドセッション）か（純粋関数）。
///
/// 守りは四重: `tako-` 接頭辞 / attached / **グループに生きた仲間がいる** / `protected`。
/// `min_idle_secs` を渡すと最終アクティビティの猶予が五重目になる（#113）
pub fn is_orphan(
    row: &SessionRow,
    protected: &std::collections::HashSet<String>,
    purpose: OrphanPurpose,
    min_idle_secs: Option<u64>,
    now: u64,
    legacy: bool,
) -> bool {
    if !row.name.starts_with(crate::tmux_backend::SESSION_PREFIX) {
        return false; // tako 由来でないものは対象外
    }
    if purpose == OrphanPurpose::Recover && row.name.starts_with(VIEW_PREFIX) {
        return false; // 表示用ラッパーは復帰対象にしない
    }
    if purpose == OrphanPurpose::Cleanup && row.attached {
        return false; // 使用中
    }
    if group_has_live_peer(row, legacy) {
        return false; // 表示中ビューの元 or そのラッパー
    }
    if protected.contains(&row.name) {
        return false; // 現存 / バックグラウンドペイン・表示中ビューが使用中
    }
    if let Some(min_idle) = min_idle_secs {
        // activity が取れない（古い tmux・パース不能 = 0）場合は「idle 十分」に倒し
        // 従来挙動（掃除する）へ劣化する
        if now.saturating_sub(row.activity) < min_idle {
            return false; // 直近までアクティブ = 実行中プロセスの可能性が高い
        }
    }
    true
}

/// 表示用ラッパーの名前の接頭辞（`tako tmux open` が作る grouped session）
pub const VIEW_PREFIX: &str = "tako-view-";

/// #1188 の A/B。`TAKO_1188_LEGACY=1` で修正前の判定（`session_grouped`）に戻す
pub fn legacy_1188() -> bool {
    std::env::var_os("TAKO_1188_LEGACY").is_some()
}

// ---------------------------------------------------------------------------
// サーバー（ソケット）単位の回収（#1192）
// ---------------------------------------------------------------------------
//
// セッション単位の掃除（`cleanup_orphans`）は**自分のサーバーの中**にしか届かない。
// 隔離起動やテストが立てた tmux サーバーは 1 本ごとに別ソケットなので、放置すると
// サーバーもソケットファイルも増え続ける（実測 2026-09-09: 生存 93 本 / 388 MB /
// 最古 11 日・残骸ソケット 1440 個）。
//
// **回収の判定は名前ではなく所有プロセスの生死で行う**（#625 の事故クラス:
// 接頭辞一致の一括 kill が、別 worker の生きている隔離バックエンドを落とした）。
// 「自分のもの → 既定サーバー → 生きた tako-app が使っている → tako 由来でない →
// サーバー不在 → attach 中 → 所有者不明 → 所有者が生きている → 出来たて」を順に除いた
// 残りだけが [`ServerVerdict::Reclaimable`] になる。既定は dry-run。

/// 出来たてのソケットは触らない猶予（秒）。起動途中のサーバーと競合しないため
pub const SERVER_FRESH_SECS: u64 = 60;

/// 1 つの tmux ソケットについて読み取りだけで集めた材料
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub socket: String,
    /// ソケットファイルの実パス（`-S` で直接指す。`-L` は `TMUX_TMPDIR` に縛られる）
    pub path: std::path::PathBuf,
    /// サーバーが応答したか（false = 残骸ソケット）
    pub running: bool,
    /// attach 中のクライアント数（`running` のときだけ意味がある）
    pub clients: usize,
    pub sessions: usize,
    /// ソケットファイルのサイズ（バイト）
    pub bytes: u64,
    /// 最終更新からの経過秒（読めなければ `None` = 出来たて扱いで触らない）
    pub modified_secs_ago: Option<u64>,
    /// 名前に埋まっている所有 pid 候補（`tako-iso-<pid>` 等）。**空 = 所有者不明**
    pub owner_pids: Vec<u32>,
    /// そのうち生きているもの
    pub live_owner_pids: Vec<u32>,
    /// 器の指し方（#1282）。ソケットファイルが無いプラットフォーム（Windows / psmux）は
    /// 名前と器の pid で指す
    pub kind: ServerKind,
    /// 器そのもののプロセス pid（[`ServerKind::Named`] のときだけ埋まる）。
    /// **回収はこの pid を名指しして行う**（名前一致の一括 kill は #625 の事故クラス）
    pub server_pids: Vec<u32>,
    /// [`Self::owner_pids`] の出どころ。回収の強さが変わる（[`OwnerSource`]）
    pub owner_source: OwnerSource,
    /// この機で生きている tako-app の pid（[`ServerKind::Named`] のときだけ埋まる）。
    /// ソケット名が再利用されうる器の**回収の最終ゲート**（[`ServerVerdict::PeerMayReattach`]）
    pub live_app_pids: Vec<u32>,
}

/// 器の指し方。unix はソケットファイル、Windows / psmux は名前付きパイプなので
/// 走査できるファイルが無く、**プロセスのコマンドライン**（`-L <名前>`）で見つける
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerKind {
    /// ソケットファイル（`tmux -S <path>`）
    #[default]
    SocketFile,
    /// 名前付き（`tmux -L <名前>`）。器の pid が分かっている
    Named,
}

/// 所有 pid をどこから読んだか（#1282）。**回収してよい強さが違う**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OwnerSource {
    /// 材料が無い（触らない）
    #[default]
    Unknown,
    /// ソケット名に埋まっている（`tako-iso-<pid>`）。この名前は pid ごとに変わるので
    /// **所有者が死んだら誰も再利用しない** = 回収してよい
    SocketName,
    /// 器のコマンドラインに載っていた隔離マーカー（`tako-iso-data-<pid>`）由来。
    /// ソケット名自体（`TAKO_TMUX_SOCKET` の手動指定）は**再起動した tako-app が
    /// 同じ名前で再 attach しうる**ので、生きた tako-app が居る間は回収しない
    CommandLine,
}

/// そのソケットをどう扱うか（`--apply` で実際に触るのは `Reclaimable` / `StaleSocket` だけ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerVerdict {
    /// 自分（この tako-app）の backend
    SelfSocket,
    /// 既定サーバー（ユーザーの実セッションが載る）
    DefaultSocket,
    /// 生きている別の tako-app が使っている（#1187 の所有者判定）
    PeerOwned { pid: u32 },
    /// tako 由来でない名前
    Foreign,
    /// attach 中のクライアントがいる（誰かが使っている）
    ClientsAttached { clients: usize },
    /// 名前から所有 pid を特定できない（**触らない**）
    OwnerUnknown,
    /// 名前の所有 pid が生きている
    OwnerAlive { pids: Vec<u32> },
    /// 出来たて（起動途中かもしれない）
    TooFresh,
    /// サーバー不在の残骸ソケット（ファイルだけ消す）
    StaleSocket,
    /// 所有 pid はコマンドライン由来で、ソケット名は再利用されうる。
    /// 生きた tako-app が居る間は回収しない（#1282）
    PeerMayReattach { pids: Vec<u32> },
    /// 所有者が死んだサーバー（kill + ソケット削除）
    Reclaimable { pids: Vec<u32> },
}

impl ServerVerdict {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SelfSocket => "self",
            Self::DefaultSocket => "default_server",
            Self::PeerOwned { .. } => "peer_owned",
            Self::Foreign => "foreign",
            Self::ClientsAttached { .. } => "clients_attached",
            Self::OwnerUnknown => "owner_unknown",
            Self::OwnerAlive { .. } => "owner_alive",
            Self::TooFresh => "too_fresh",
            Self::PeerMayReattach { .. } => "peer_may_reattach",
            Self::StaleSocket => "stale_socket",
            Self::Reclaimable { .. } => "reclaimable",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::SelfSocket => "自分の backend サーバー".to_string(),
            Self::DefaultSocket => "既定の backend サーバー（ユーザーの実セッション）".to_string(),
            Self::PeerOwned { pid } => format!("生きている別の tako-app（pid {pid}）が使っている"),
            Self::Foreign => "tako 由来でない名前なので触らない".to_string(),
            Self::ClientsAttached { clients } => {
                format!("attach 中のクライアントが {clients} 個いる")
            }
            Self::OwnerUnknown => "名前から所有 pid を特定できないので触らない".to_string(),
            Self::OwnerAlive { pids } => format!("所有 pid（{}）が生きている", join_pids(pids)),
            Self::TooFresh => format!("できてから {SERVER_FRESH_SECS} 秒以内なので触らない"),
            Self::PeerMayReattach { pids } => format!(
                "所有者はコマンドライン由来でソケット名は再利用されうる。\
                 生きている tako-app（pid {}）が同じ名前へ再 attach しうるので見送った\
                 （すべての tako-app を終了してから叩き直す）",
                join_pids(pids)
            ),
            Self::StaleSocket => "サーバー不在の残骸ソケット（ファイルだけ消す）".to_string(),
            Self::Reclaimable { pids } => format!(
                "所有 pid（{}）が死んでいて attach も無い（サーバーを kill してソケットを消す）",
                join_pids(pids)
            ),
        }
    }

    /// `--apply` で実際に手を入れる対象か
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::Reclaimable { .. } | Self::StaleSocket)
    }

    /// サーバーを kill する対象か（残骸ソケットはファイル削除だけ）
    pub fn kills_server(&self) -> bool {
        matches!(self, Self::Reclaimable { .. })
    }
}

/// kill してよい名前の系統（`tako-` で始まるもの限定。既定サーバー `tako` は含まない）
pub fn is_killable_socket_name(name: &str) -> bool {
    name.starts_with("tako-")
}

/// 残骸ソケット（サーバー不在）としてファイルを消してよい名前の系統。
/// サーバーが居ないので誰も壊さない = kill より広く取れる（`tk*` の使い捨ても拾う）
pub fn is_tako_socket_name(name: &str) -> bool {
    name.starts_with("tako") || name.starts_with("tk")
}

/// 名前に埋まっている所有 pid 候補（`-` 区切りで**まるごと数値**の区画だけ）。
///
/// `tako-iso-4242` → `[4242]` / `tako-972test-4242-legacy` → `[4242]`（pid が中央にある
/// 命名が実在する）/ `tako-iso-1090mac` → `[]`（数字が語の一部 = pid と断定できない）。
/// **候補が 1 つでも生きていれば保護**するので、余分に拾う方向は安全側に働く
pub fn owner_pid_candidates(name: &str) -> Vec<u32> {
    name.trim_end_matches('=')
        .split('-')
        .filter(|seg| !seg.is_empty() && seg.len() <= 7 && seg.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|seg| seg.parse::<u32>().ok())
        .filter(|&pid| pid != 0)
        .collect()
}

/// そのソケットをどう扱うか（純粋関数）。`peers` は #1187 の
/// [`live_peers`]（生きている別 tako-app とその backend ソケット）
pub fn judge_server(
    entry: &ServerEntry,
    self_socket: &str,
    peers: &[CleanupPeer],
) -> ServerVerdict {
    judge_server_with(entry, self_socket, peers, SERVER_FRESH_SECS)
}

/// [`judge_server`] の「出来たて」の閾値を差し替えられる版（テストは 0 を渡して
/// 作りたてのダミーを判定させる）
pub fn judge_server_with(
    entry: &ServerEntry,
    self_socket: &str,
    peers: &[CleanupPeer],
    fresh_secs: u64,
) -> ServerVerdict {
    if entry.socket == self_socket {
        return ServerVerdict::SelfSocket;
    }
    if entry.socket == crate::tmux_backend::DEFAULT_SOCKET {
        return ServerVerdict::DefaultSocket;
    }
    // 生きた tako-app が使っているソケットには触らない。**名前が pid と一致しなくても**
    // 相手の環境から復元した名前で当たる（`TAKO_TMUX_SOCKET=tako-iso-<Issue 番号>` のような
    // 手動指定は名前だけでは見抜けない = #625 の事故に直結する）
    if let Some(peer) = peers
        .iter()
        .find(|p| p.socket.as_deref() == Some(entry.socket.as_str()))
    {
        return ServerVerdict::PeerOwned { pid: peer.pid };
    }
    if !is_tako_socket_name(&entry.socket) {
        return ServerVerdict::Foreign;
    }
    let fresh = entry.modified_secs_ago.is_none_or(|secs| secs < fresh_secs);
    if !entry.running {
        return if fresh {
            ServerVerdict::TooFresh
        } else {
            ServerVerdict::StaleSocket
        };
    }
    if entry.clients > 0 {
        return ServerVerdict::ClientsAttached {
            clients: entry.clients,
        };
    }
    if !is_killable_socket_name(&entry.socket) || entry.owner_pids.is_empty() {
        return ServerVerdict::OwnerUnknown;
    }
    if !entry.live_owner_pids.is_empty() {
        return ServerVerdict::OwnerAlive {
            pids: entry.live_owner_pids.clone(),
        };
    }
    if fresh {
        return ServerVerdict::TooFresh;
    }
    // #1282: 所有者をコマンドラインの隔離マーカーから読んだ器は、**ソケット名自体が
    // 再利用されうる**（`TAKO_TMUX_SOCKET=tako-w1133` のような手動指定は、tako-app が
    // 再起動しても同じ名前を使う）。所有者だった pid が死んでいても、生きている
    // tako-app が同じ名前へ再 attach していれば、それは現役の器である
    if entry.owner_source == OwnerSource::CommandLine && !entry.live_app_pids.is_empty() {
        return ServerVerdict::PeerMayReattach {
            pids: entry.live_app_pids.clone(),
        };
    }
    ServerVerdict::Reclaimable {
        pids: entry.owner_pids.clone(),
    }
}

/// この起動が使っている backend ソケットが**自分の pid から作られた使い捨て**か（#1192）。
///
/// `tako-iso-<自分の pid>` / `tako-st-<自分の pid>` は定義上このプロセス専用で、
/// プロセスが終われば**誰も再利用できない名前**になる（次の隔離起動は別 pid を使う）。
/// だから終了時に自分でサーバーごと落としてよい。逆に `TAKO_TMUX_SOCKET` を明示して
/// 起動した場合は、再起動をまたいでセッションを残す検証（#770 等）が実在するので触らない
pub fn owns_disposable_socket(pid: u32, socket: &str) -> bool {
    socket == isolated_socket_name(pid) || socket == self_test_socket_name(pid)
}

/// 回収の結果（dry-run と `--apply` で同じ形。何を・なぜ触った / 触らなかったかが全部載る）
#[derive(Debug, Clone)]
pub struct ServerCleanupOutcome {
    pub applied: bool,
    pub entries: Vec<(ServerEntry, ServerVerdict)>,
    pub killed: Vec<String>,
    pub removed_sockets: Vec<String>,
}

impl ServerCleanupOutcome {
    pub fn count(&self, code: &str) -> usize {
        self.entries
            .iter()
            .filter(|(_, v)| v.code() == code)
            .count()
    }

    pub fn running(&self) -> usize {
        self.entries.iter().filter(|(e, _)| e.running).count()
    }

    /// persist.log へ残す 1 行（dry-run でも「見ただけ」と分かる形で残す）
    pub fn log_line(&self) -> String {
        format!(
            "tmux cleanup(servers): {} 総数={} 生存={} 回収可={} 残骸={} kill={} ソケット削除={}",
            if self.applied { "実行" } else { "dry-run" },
            self.entries.len(),
            self.running(),
            self.count("reclaimable"),
            self.count("stale_socket"),
            self.killed.len(),
            self.removed_sockets.len()
        )
    }
}

/// サーバー一覧を作る（読み取りのみ）。**器の見つけ方はプラットフォームで違う**（#1282）:
/// ソケットディレクトリがある unix はファイルを走査し、名前付きパイプの
/// Windows / psmux はプロセスのコマンドライン（`-L <名前>`）から見つける
pub fn scan_servers() -> Vec<ServerEntry> {
    match crate::tmux_backend::socket_dir() {
        Some(dir) => scan_servers_in(&dir, crate::ports::process_alive),
        // A/B: 修正前はここが「置き場が無い = 器も無い」で空を返していた
        None if legacy_1282() => Vec::new(),
        None => scan_named_servers(),
    }
}

/// #1282 の A/B。`TAKO_1282_LEGACY=1` で**修正前の挙動**へ戻す:
/// ソケットファイルの走査しか持たないので、名前付きパイプの Windows / psmux では
/// 器が 1 つも見つからない（実機に 24 個残っていても 0 件と答える）。
/// **macOS は 1 ビットも変わらない**（置き場があるので常に走査の側へ行く）
pub fn legacy_1282() -> bool {
    std::env::var_os("TAKO_1282_LEGACY").is_some()
}

/// [`scan_servers`] のディレクトリと pid 生存判定を差し替えられる版。
/// **テストと検証はこれを使う**（本番のソケットディレクトリを対象にしない）
pub fn scan_servers_in(dir: &std::path::Path, alive: impl Fn(u32) -> bool) -> Vec<ServerEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now();
    let mut by_name: std::collections::BTreeMap<String, ServerEntry> =
        std::collections::BTreeMap::new();
    for file in read.flatten() {
        let raw = file.file_name();
        let Some(raw) = raw.to_str() else { continue };
        // macOS は /tmp → /private/tmp の解決で末尾に `=` が付いた別名を作る（同じ実体）
        let name = raw.trim_end_matches('=').to_string();
        if name.is_empty() || by_name.contains_key(&name) {
            continue;
        }
        let path = dir.join(raw);
        let meta = std::fs::metadata(&path).ok();
        let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_secs_ago = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs());
        let running = socket_answers(&path);
        let (sessions, clients) = if running {
            (session_count(&path), client_count(&path))
        } else {
            (0, 0)
        };
        let owner_pids = owner_pid_candidates(&name);
        let live_owner_pids = owner_pids.iter().copied().filter(|&p| alive(p)).collect();
        let owner_source = if owner_pids.is_empty() {
            OwnerSource::Unknown
        } else {
            OwnerSource::SocketName
        };
        by_name.insert(
            name.clone(),
            ServerEntry {
                socket: name,
                path,
                running,
                clients,
                sessions,
                bytes,
                modified_secs_ago,
                owner_pids,
                live_owner_pids,
                kind: ServerKind::SocketFile,
                server_pids: Vec::new(),
                owner_source,
                live_app_pids: Vec::new(),
            },
        );
    }
    by_name.into_values().collect()
}

/// ソケットに繋がるか（= サーバーが生きているか）。**tmux を 1 本ごとに起こさない**
/// （1700 個のソケットに対してプロセスを起こすと分単位になる）。繋いですぐ閉じるのは
/// tmux から見れば「何も言わずに切れたクライアント」なので無害
#[cfg(unix)]
fn socket_answers(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_answers(_path: &std::path::Path) -> bool {
    false
}

fn session_count(path: &std::path::Path) -> usize {
    count_lines(path, &["list-sessions", "-F", "#{session_name}"])
}

/// attach 中のクライアント数。**kill の可否を分ける材料**
fn client_count(path: &std::path::Path) -> usize {
    count_lines(path, &["list-clients", "-F", "#{client_pid}"])
}

/// `tmux -S <path> <args>` の出力行数（失敗は 0）
fn count_lines(path: &std::path::Path, args: &[&str]) -> usize {
    crate::tmux::run_tmux_at(path, args)
        .map(|out| out.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// サーバー単位の回収（#1192）。既定は dry-run で、`apply` のときだけ実際に触る。
pub fn cleanup_servers(
    self_socket: &str,
    peers: &[CleanupPeer],
    apply: bool,
) -> ServerCleanupOutcome {
    cleanup_servers_from(scan_servers(), self_socket, peers, apply, SERVER_FRESH_SECS)
}

/// 走査済みの一覧に対して判定と（`apply` なら）実行を行う。
///
/// **生きている所有者のサーバーには絶対に触らない**。kill の直前にもう 1 度
/// クライアントの有無を測り直す（走査してから apply するまでの隙に attach された
/// サーバーを落とさない）
pub fn cleanup_servers_from(
    entries: Vec<ServerEntry>,
    self_socket: &str,
    peers: &[CleanupPeer],
    apply: bool,
    fresh_secs: u64,
) -> ServerCleanupOutcome {
    let mut judged = Vec::new();
    let mut killed = Vec::new();
    let mut removed_sockets = Vec::new();
    for entry in entries {
        let mut verdict = judge_server_with(&entry, self_socket, peers, fresh_secs);
        if apply && verdict.is_actionable() {
            if entry.kind == ServerKind::Named {
                // #1282: 名前付きパイプの器（psmux）は落とすソケットファイルが無い。
                // **器の pid を名指し**して落とす（残骸ソケットの概念も無いので、
                // ここへ来るのは Reclaimable だけ）
                match reclaim_named(&entry) {
                    Ok(pids) if !pids.is_empty() => killed.push(entry.socket.clone()),
                    Ok(_) => {}
                    Err(clients) => verdict = ServerVerdict::ClientsAttached { clients },
                }
            } else if verdict.kills_server() {
                // 直前の再確認（走査からの時間差で attach された可能性を潰す）
                let clients = client_count(&entry.path);
                if clients > 0 {
                    verdict = ServerVerdict::ClientsAttached { clients };
                    judged.push((entry, verdict));
                    continue;
                }
                let _ = crate::tmux::run_tmux_at(&entry.path, &["kill-server"]);
                remove_socket_files(&entry.path);
                killed.push(entry.socket.clone());
            } else {
                remove_socket_files(&entry.path);
                removed_sockets.push(entry.socket.clone());
            }
        }
        judged.push((entry, verdict));
    }
    ServerCleanupOutcome {
        applied: apply,
        entries: judged,
        killed,
        removed_sockets,
    }
}

/// ソケットファイルを消す（macOS の `=` 付き別名も一緒に）
fn remove_socket_files(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(dir) = path.parent() {
            let base = name.trim_end_matches('=');
            let _ = std::fs::remove_file(dir.join(base));
            let _ = std::fs::remove_file(dir.join(format!("{base}=")));
        }
    }
}

// ---------------------------------------------------------------------------
// 名前で列挙する器（Windows / psmux。#1282）
// ---------------------------------------------------------------------------
//
// unix の走査（[`scan_servers_in`]）は**ソケットファイル**が置き場にあることに依っている。
// psmux は名前付きパイプ（`-L <名前>`）なので、走査できるファイルが 1 つも無く、
// `socket_dir()` は Windows で `None`・`socket_answers` は常に false = **器が 1 つも
// 見つからない**（実機には隔離 GUI 由来の器が 24 個残っていたのに 0 件と答えていた）。
//
// そこで Windows は**プロセスのコマンドライン**から器を見つける。psmux のパッケージは
// `psmux.exe` / `tmux.exe` / `pmux.exe` を同じ実体で配り、**サーバーは起動した
// クライアントの実行ファイル名を名乗る**（#1271 の実測）ので 3 名すべてを数える。
//
// 回収は**器の pid を名指し**して行う（`Stop-Process -Name` / `taskkill /IM` は
// 別インスタンス・別ワーカーの器を巻き込む = #625 の事故クラス）。psmux の
// `kill-server` は `-L` を落とすと全ソケットを殺し、しかも返らないことがある（#1271）
// ので、この経路では一切使わない。

/// 器の実行ファイル名（拡張子・大文字小文字は無視して照合する）
pub const MUX_PROCESS_NAMES: [&str; 3] = ["tmux", "psmux", "pmux"];

/// psmux が 1 ソケットにつき 1 本持つ先読み用サーバーのセッション名。
/// **利用者のセッションではない**ので数に入れない（#1271 の実測）
pub const WARM_SESSION: &str = "__warm__";

/// 値を取るオプション。サブコマンド（`server` / `list-sessions` …）を見つけるとき、
/// **オプションの値をサブコマンドと読み違えない**ために使う
const VALUE_FLAGS: [&str; 11] = [
    "-c", "-f", "-L", "-S", "-T", "-s", "-e", "-x", "-y", "-t", "-n",
];

/// コマンドライン 1 本から読んだ器のプロセス（純粋関数の出力）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxProcess {
    pub pid: u32,
    /// `-L <名前>`（`-S <パス>` ならその名前部分）
    pub socket: String,
    /// サブコマンドが `server` か（false = クライアント = 誰かが使っている印）
    pub is_server: bool,
    /// `-s <名前>`（サーバーが持つセッション名）
    pub session: Option<String>,
    /// コマンドラインに載っていた隔離マーカー由来の所有 pid
    pub marker_pids: Vec<u32>,
    /// プロセスの起動時刻（UNIX 秒）
    pub started_unix: Option<u64>,
}

/// コマンドラインを語へ割る（純粋関数）。
///
/// `"` で囲まれた区間の空白は割らない。**バックスラッシュのエスケープは解釈しない**:
/// Windows のパスは `C:\\Users\\...` のように `\\` を素で含み、`\\"` の形は器の
/// コマンドラインに現れないため、解釈するほうが誤りを増やす
pub fn split_command_line(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for ch in cmd.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// 器のコマンドライン 1 本を読む（純粋関数）。`-L` / `-S` が無ければ `None`
/// （既定のソケットを使う素の tmux / psmux は tako の管理外なので**触らない**）
pub fn parse_mux_process(pid: u32, cmd: &str, started_unix: Option<u64>) -> Option<MuxProcess> {
    let words = split_command_line(cmd);
    let mut socket: Option<String> = None;
    let mut session: Option<String> = None;
    let mut subcommand: Option<String> = None;
    let mut i = 1; // argv[0] は実行ファイル
    while i < words.len() {
        let word = words[i].as_str();
        let value = words.get(i + 1).cloned();
        match word {
            "-L" => socket = value.filter(|v| !v.is_empty()),
            "-S" => {
                socket = value.and_then(|v| {
                    std::path::Path::new(&v)
                        .file_name()
                        .map(|n| n.to_string_lossy().trim_end_matches('=').to_string())
                        .filter(|n| !n.is_empty())
                })
            }
            "-s" => session = value,
            _ => {}
        }
        if VALUE_FLAGS.contains(&word) {
            i += 2;
            continue;
        }
        if !word.starts_with('-') && subcommand.is_none() {
            subcommand = Some(word.to_string());
        }
        i += 1;
    }
    Some(MuxProcess {
        pid,
        socket: socket?,
        is_server: subcommand.as_deref() == Some("server"),
        session,
        marker_pids: marker_owner_pids(cmd),
        started_unix,
    })
}

/// コマンドラインに載っている**隔離マーカー**から所有 tako-app の pid を読む（純粋関数）。
///
/// 隔離起動（`TAKO_ISOLATED=1`）は data / discovery / sessions / pane-logs / remote の
/// 置き場をすべて `tako-iso-<用途>-<自分の pid>` にする（`tako-app` の一括隔離）。
/// 器のプロセスにはそれが `-e TAKO_OSC_SINK=<temp>\tako-iso-data-<pid>\osc\7.osc` の形で
/// 載るので、**ソケット名が手動指定（`TAKO_TMUX_SOCKET=tako-w1133`）でも所有者が分かる**。
///
/// pid の切り出しは [`owner_pid_candidates`] と 1 実装（規則がズレると所有者判定が外れる）
pub fn marker_owner_pids(cmd: &str) -> Vec<u32> {
    const MARKER: &str = "tako-iso-";
    let bytes = cmd.as_bytes();
    let mut out: Vec<u32> = Vec::new();
    let mut from = 0;
    while let Some(rel) = cmd[from..].find(MARKER) {
        let start = from + rel;
        let mut end = start;
        // マーカーの語は ASCII の英数字と `-` / `_` だけ（`.yaml` や `\` で切れる）
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
        {
            end += 1;
        }
        for pid in owner_pid_candidates(&cmd[start..end]) {
            if !out.contains(&pid) {
                out.push(pid);
            }
        }
        from = end.max(start + MARKER.len());
    }
    out
}

/// 読み取った器のプロセスを**ソケット単位**へ畳む（純粋関数）。
///
/// - `alive`: pid の生死（テストは固定の集合を渡す）
/// - `now_unix`: いまの UNIX 秒（出来たての猶予の基準）
/// - `live_app_pids`: この機で生きている `tako-app`（再 attach の見張り）。
///   **この操作を提供している GUI 自身は含めない**（含めると常に見送りになる）
///
/// 器のプロセスが 1 つも無いソケット（クライアントだけが残っている）は返さない
/// ＝ 落とすものが無い
pub fn named_server_entries(
    procs: &[MuxProcess],
    alive: impl Fn(u32) -> bool,
    now_unix: u64,
    live_app_pids: &[u32],
) -> Vec<ServerEntry> {
    let mut by_socket: std::collections::BTreeMap<&str, Vec<&MuxProcess>> =
        std::collections::BTreeMap::new();
    for proc in procs {
        by_socket
            .entry(proc.socket.as_str())
            .or_default()
            .push(proc);
    }
    let mut out = Vec::new();
    for (socket, group) in by_socket {
        let servers: Vec<&&MuxProcess> = group.iter().filter(|p| p.is_server).collect();
        if servers.is_empty() {
            continue; // 器が居ない = 落とすものが無い（クライアントの残骸だけ）
        }
        let clients = group.len() - servers.len();
        let mut sessions: Vec<&str> = servers
            .iter()
            .filter_map(|p| p.session.as_deref())
            .filter(|name| *name != WARM_SESSION)
            .collect();
        sessions.sort_unstable();
        sessions.dedup();
        // 出来たての判定は**いちばん新しい器**で見る（起動途中のソケットを守る）。
        // 1 つでも起動時刻が読めなければ `None` = 出来たて扱い（触らない）
        let modified_secs_ago = servers
            .iter()
            .map(|p| p.started_unix)
            .try_fold(0u64, |newest, at| at.map(|at| newest.max(at)))
            .map(|newest| now_unix.saturating_sub(newest));

        let name_pids = owner_pid_candidates(socket);
        let mut marker_pids: Vec<u32> = Vec::new();
        for proc in &servers {
            for pid in &proc.marker_pids {
                if !marker_pids.contains(pid) {
                    marker_pids.push(*pid);
                }
            }
        }
        // ソケット名から読めたなら**そちらが強い**（`tako-iso-<pid>` は pid ごとに
        // 名前が変わるので誰も再利用しない）。守りを厚くするため候補は合流させる
        let (mut owner_pids, owner_source) = if !name_pids.is_empty() {
            (name_pids, OwnerSource::SocketName)
        } else if !marker_pids.is_empty() {
            (Vec::new(), OwnerSource::CommandLine)
        } else {
            (Vec::new(), OwnerSource::Unknown)
        };
        for pid in marker_pids {
            if !owner_pids.contains(&pid) {
                owner_pids.push(pid);
            }
        }
        let live_owner_pids: Vec<u32> = owner_pids.iter().copied().filter(|&p| alive(p)).collect();
        out.push(ServerEntry {
            socket: socket.to_string(),
            path: std::path::PathBuf::new(), // ソケットファイルが無い
            running: true,                   // プロセスとして見つかった = 生きている
            clients,
            sessions: sessions.len(),
            bytes: 0,
            modified_secs_ago,
            owner_pids,
            live_owner_pids,
            kind: ServerKind::Named,
            server_pids: servers.iter().map(|p| p.pid).collect(),
            owner_source,
            live_app_pids: live_app_pids.to_vec(),
        });
    }
    out
}

/// いまの UNIX 秒（読めなければ 0 = すべて「出来たて」に倒れて何も触らない）
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 名前で列挙した器の一覧（読み取りのみ）。コマンドラインを読めない
/// プラットフォームでは空 = 何もしない
pub fn scan_named_servers() -> Vec<ServerEntry> {
    let procs = read_mux_processes();
    let live_apps = crate::platform::procinfo::live_tako_app_pids(
        &crate::platform::procinfo::snapshot(),
        std::process::id(),
    );
    named_server_entries(
        &procs,
        crate::platform::process::pid_alive,
        now_unix(),
        &live_apps,
    )
}

/// いま動いている器のプロセスを読む（コマンドラインが取れないものは捨てる）
fn read_mux_processes() -> Vec<MuxProcess> {
    crate::platform::procinfo::details_by_name(&MUX_PROCESS_NAMES)
        .into_iter()
        .filter_map(|detail| {
            let cmd = detail.command_line?;
            parse_mux_process(detail.pid, &cmd, detail.started_unix)
        })
        .collect()
}

/// 回収の直前にもう一度測り直す（走査から apply までの隙に attach された器を落とさない）。
/// 戻り値は `(クライアント数, まだ生きている器の pid)`
fn recheck_named(socket: &str) -> (usize, Vec<u32>) {
    let procs = read_mux_processes();
    let clients = procs
        .iter()
        .filter(|p| p.socket == socket && !p.is_server)
        .count();
    let servers = procs
        .iter()
        .filter(|p| p.socket == socket && p.is_server)
        .map(|p| p.pid)
        .collect();
    (clients, servers)
}

/// **pid を名指し**して器を落とす。名前一致（`taskkill /IM` / `Stop-Process -Name`）は
/// 他インスタンスの器を巻き込むので絶対に使わない。
/// Windows は `/T` で器の中の pwsh ごと落とす
fn force_kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        crate::platform::process::no_console_window(&mut cmd);
        let _ = cmd.args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    }
    #[cfg(not(windows))]
    {
        // unix でこの経路へ来るのは名前で列挙した器だけ（ソケットファイルがある
        // 環境では生じない）。実装を空にすると「落としたつもり」になるので、
        // 同じ意味の操作を置く。**子プロセスは起こさない**（`kill(1)` を spawn すると
        // Windows のコンソール窓抑止の境界検査に引っかかるうえ、ここでは不要）
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        if pid > 0 {
            // SAFETY: 正の pid（プロセスグループ / 全プロセスの特別値ではない）へ
            // SIGKILL を送るだけ。引数は値渡しでポインタを触らない
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// 名前で列挙した器を回収する。**生きている器の pid だけ**を名指しで落とす
fn reclaim_named(entry: &ServerEntry) -> Result<Vec<u32>, usize> {
    let (clients, live_pids) = recheck_named(&entry.socket);
    if clients > 0 {
        return Err(clients); // 走査後に attach された
    }
    let mut killed = Vec::new();
    for pid in entry.server_pids.iter().copied() {
        if !live_pids.contains(&pid) {
            continue; // もう居ない
        }
        force_kill_pid(pid);
        killed.push(pid);
    }
    Ok(killed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(pid: u32, socket: Option<&str>) -> CleanupPeer {
        CleanupPeer {
            pid,
            socket: socket.map(str::to_string),
        }
    }

    fn row(line: &str) -> SessionRow {
        parse_session_row(line).expect("行を解ける")
    }

    /// `名前\t attached \t grouped \t group_size \t activity`
    fn line(name: &str, attached: u8, grouped: u8, size: &str, activity: u64) -> String {
        format!("{name}\t{attached}\t{grouped}\t{size}\t{activity}")
    }

    #[test]
    fn list_formatの列を順番どおり解く() {
        let r = row(&line("tako-a", 1, 1, "2", 1757000000));
        assert_eq!(r.name, "tako-a");
        assert!(r.attached && r.grouped_flag);
        assert_eq!(r.group_size, Some(2));
        assert!(!r.group_size_unreadable);
        assert_eq!(r.activity, 1757000000);
        // グループ無し（size 列が空）
        let r = row(&line("tako-b", 0, 0, "", 0));
        assert_eq!(r.group_size, None);
        assert!(!r.group_size_unreadable);
        // 未知の tmux が書式をそのまま返す = 読めない
        let r = row(&line("tako-c", 0, 1, "#{session_group_size}", 0));
        assert!(r.group_size_unreadable);
    }

    /// #1188 の本体。`session_grouped` は 1 度でも表示すると 1 のままなので、
    /// **メンバー数**で「いま仲間がいるか」を判定する
    #[test]
    fn issue1188_ビューを閉じた元セッションは掃除対象へ戻る() {
        let protected = std::collections::HashSet::new();
        // ビューを閉じた後の元セッション（grouped=1 のまま・メンバーは 1）
        let closed = row(&line("tako-blind-4", 0, 1, "1", 0));
        assert!(
            is_orphan(&closed, &protected, OrphanPurpose::Cleanup, None, 0, false),
            "ビューを閉じたら掃除対象へ戻る"
        );
        assert!(
            !is_orphan(&closed, &protected, OrphanPurpose::Cleanup, None, 0, true),
            "修正前（legacy）は永久に見送る = Issue の症状"
        );
        // 表示中ビューがある元セッション（メンバー 2）は守られる
        let live = row(&line("tako-blind-4", 0, 1, "2", 0));
        assert!(!is_orphan(
            &live,
            &protected,
            OrphanPurpose::Cleanup,
            None,
            0,
            false
        ));
        // 読めない tmux は安全側（触らない）
        let unknown = row(&line("tako-blind-4", 0, 1, "#{session_group_size}", 0));
        assert!(!is_orphan(
            &unknown,
            &protected,
            OrphanPurpose::Cleanup,
            None,
            0,
            false
        ));
    }

    #[test]
    fn 掃除の四重ガードは効いたまま() {
        let empty = std::collections::HashSet::new();
        let protected: std::collections::HashSet<String> =
            ["tako-keep".to_string()].into_iter().collect();
        // tako- 接頭辞でない
        assert!(!is_orphan(
            &row(&line("user-session", 0, 0, "", 0)),
            &empty,
            OrphanPurpose::Cleanup,
            None,
            0,
            false
        ));
        // attached
        assert!(!is_orphan(
            &row(&line("tako-live", 1, 0, "", 0)),
            &empty,
            OrphanPurpose::Cleanup,
            None,
            0,
            false
        ));
        // protected
        assert!(!is_orphan(
            &row(&line("tako-keep", 0, 0, "", 0)),
            &protected,
            OrphanPurpose::Cleanup,
            None,
            0,
            false
        ));
        // 猶予内（#113）
        assert!(!is_orphan(
            &row(&line("tako-busy", 0, 0, "", 1000)),
            &empty,
            OrphanPurpose::Cleanup,
            Some(3600),
            1500,
            false
        ));
        // 猶予を過ぎていれば掃除する
        assert!(is_orphan(
            &row(&line("tako-busy", 0, 0, "", 1000)),
            &empty,
            OrphanPurpose::Cleanup,
            Some(3600),
            9000,
            false
        ));
    }

    /// 発見（起動時の自動復帰）はラッパーを対象にせず、attached も見ない
    /// （前のインスタンスが死んでいるので attach は残っていない）
    #[test]
    fn 発見はラッパーを対象にしない() {
        let empty = std::collections::HashSet::new();
        assert!(!is_orphan(
            &row(&line("tako-view-tako-a-7", 0, 0, "", 0)),
            &empty,
            OrphanPurpose::Recover,
            None,
            0,
            false
        ));
        assert!(is_orphan(
            &row(&line("tako-a", 1, 0, "", 0)),
            &empty,
            OrphanPurpose::Recover,
            None,
            0,
            false
        ));
    }

    #[test]
    fn 相手がいなければ見送らない() {
        assert_eq!(blocker(CleanupScope::Explicit, "tako", &[]), None);
        assert_eq!(blocker(CleanupScope::Startup, "tako", &[]), None);
    }

    /// #1187 の本体。隔離インスタンス（別ソケット）は本番の明示掃除を止めない
    #[test]
    fn 明示操作は別ソケットの隔離インスタンスに止められない() {
        let peers = [
            peer(4242, Some("tako-iso-4242")),
            peer(4243, Some("tako-st-4243")),
        ];
        assert_eq!(blocker(CleanupScope::Explicit, "tako", &peers), None);
    }

    #[test]
    fn 同じソケットを使う相手がいれば見送る() {
        let peers = [peer(4242, Some("tako-iso-4242")), peer(71082, Some("tako"))];
        let skip = blocker(CleanupScope::Explicit, "tako", &peers).expect("見送るはず");
        assert_eq!(skip.code(), "peer_shares_socket");
        assert!(skip.detail().contains("71082"), "理由に pid が入る");
        assert!(skip.detail().contains("tako"), "理由にソケット名が入る");
        assert!(
            !skip.detail().contains("4242"),
            "無関係な相手は理由に入れない"
        );
    }

    /// 他人の隔離ソケットを `--socket` で名指ししても、その所有者が生きていれば見送る
    #[test]
    fn 他インスタンスのソケットを名指ししても見送る() {
        let peers = [peer(4242, Some("tako-iso-4242"))];
        let skip = blocker(CleanupScope::Explicit, "tako-iso-4242", &peers).expect("見送るはず");
        assert_eq!(skip.code(), "peer_shares_socket");
    }

    #[test]
    fn ソケットを特定できない相手がいれば安全側に見送る() {
        let peers = [peer(4242, None)];
        let skip = blocker(CleanupScope::Explicit, "tako", &peers).expect("見送るはず");
        assert_eq!(skip.code(), "peer_socket_unknown");
        assert!(skip.detail().contains("4242"));
    }

    /// 起動時は #113 のまま「1 つでもいれば見送る」。ただし理由は返す
    #[test]
    fn 起動時は別ソケットの相手でも見送るが理由を返す() {
        let peers = [peer(4242, Some("tako-iso-4242"))];
        let skip = blocker(CleanupScope::Startup, "tako", &peers).expect("見送るはず");
        assert_eq!(skip.code(), "other_tako_running");
        assert!(skip.detail().contains("4242"));
    }

    #[test]
    fn ソケット名は一括隔離と同じ順序で復元する() {
        let env = |pairs: Vec<(&'static str, &'static str)>| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        // 明示指定が最優先
        assert_eq!(
            resolve_socket_name(
                7,
                env(vec![("TAKO_TMUX_SOCKET", "mine"), ("TAKO_ISOLATED", "1")])
            ),
            "mine"
        );
        // 隔離 > セルフテスト
        assert_eq!(
            resolve_socket_name(
                7,
                env(vec![("TAKO_ISOLATED", "1"), ("TAKO_SELF_TEST", "1")])
            ),
            "tako-iso-7"
        );
        assert_eq!(
            resolve_socket_name(7, env(vec![("TAKO_SELF_TEST", "1")])),
            "tako-st-7"
        );
        // 何も無ければ既定サーバー
        assert_eq!(resolve_socket_name(7, env(vec![])), "tako");
        // 空文字の TAKO_TMUX_SOCKET は「設定済み」なので一括隔離は上書きせず、
        // socket_name() と同じく既定へ落ちる
        assert_eq!(
            resolve_socket_name(
                7,
                env(vec![("TAKO_TMUX_SOCKET", ""), ("TAKO_ISOLATED", "1")])
            ),
            "tako"
        );
        // 隔離の値は 1 / true / on だけ
        assert_eq!(
            resolve_socket_name(7, env(vec![("TAKO_ISOLATED", "0")])),
            "tako"
        );
    }

    /// 実プロセス（自分）の環境から自分のソケット名を復元できる = `socket_name()` と一致する。
    /// 復元の順序が実装とズレたらここで落ちる
    #[test]
    fn 自分の環境からの復元はsocket_nameと一致する() {
        let mine = resolve_socket_name(std::process::id(), |k| std::env::var(k).ok());
        assert_eq!(mine, crate::tmux_backend::socket_name());
    }

    fn server(socket: &str, running: bool, clients: usize, live: &[u32]) -> ServerEntry {
        ServerEntry {
            socket: socket.to_string(),
            path: std::path::PathBuf::from("/tmp/tmux-0").join(socket),
            running,
            clients,
            sessions: if running { 1 } else { 0 },
            bytes: 0,
            modified_secs_ago: Some(3600),
            owner_pids: owner_pid_candidates(socket),
            live_owner_pids: live.to_vec(),
            kind: ServerKind::SocketFile,
            server_pids: Vec::new(),
            owner_source: if owner_pid_candidates(socket).is_empty() {
                OwnerSource::Unknown
            } else {
                OwnerSource::SocketName
            },
            live_app_pids: Vec::new(),
        }
    }

    #[test]
    fn 所有pid候補は区画まるごと数値のものだけ() {
        assert_eq!(owner_pid_candidates("tako-iso-4242"), vec![4242]);
        // pid が中央にある命名（tako-972test-<pid>-<用途>）が実在する
        assert_eq!(
            owner_pid_candidates("tako-972test-10013-legacy"),
            vec![10013]
        );
        assert_eq!(
            owner_pid_candidates("tako-e2e-1185-14479"),
            vec![1185, 14479]
        );
        // 数字が語の一部 = pid と断定できない → 候補なし（= 触らない）
        assert!(owner_pid_candidates("tako-iso-1090mac").is_empty());
        assert!(owner_pid_candidates("tako-w1102").is_empty());
        assert!(owner_pid_candidates("tako").is_empty());
        // macOS の `=` 付き別名でも同じ結果
        assert_eq!(owner_pid_candidates("tako-iso-4242="), vec![4242]);
    }

    /// #1192 の中核。**所有者が生きているものは 1 つも回収候補にしない**
    #[test]
    fn 生きている所有者のサーバーは回収候補にしない() {
        let peers = [CleanupPeer {
            pid: 71082,
            socket: Some("tako-iso-1013".into()), // 名前の数字は pid ではない（手動指定）
        }];
        let cases = [
            // 自分のもの
            (server("tako-st-77", true, 0, &[]), "tako-st-77", "self"),
            // 既定サーバー
            (server("tako", true, 0, &[]), "other", "default_server"),
            // 生きた tako-app が使っている（名前の数字は死んでいても保護される）
            (server("tako-iso-1013", true, 0, &[]), "other", "peer_owned"),
            // attach 中
            (
                server("tako-iso-4242", true, 2, &[]),
                "other",
                "clients_attached",
            ),
            // 所有 pid が生きている
            (
                server("tako-iso-4242", true, 0, &[4242]),
                "other",
                "owner_alive",
            ),
            // 所有者を名前から特定できない
            (
                server("tako-iso-1090mac", true, 0, &[]),
                "other",
                "owner_unknown",
            ),
            // tako 由来でない
            (server("mysocket-4242", true, 0, &[]), "other", "foreign"),
            // 回収可
            (
                server("tako-iso-4242", true, 0, &[]),
                "other",
                "reclaimable",
            ),
            // 残骸ソケット（サーバー不在）
            (
                server("tako-selftest-4242", false, 0, &[]),
                "other",
                "stale_socket",
            ),
        ];
        for (entry, self_socket, want) in cases {
            let verdict = judge_server(&entry, self_socket, &peers);
            assert_eq!(
                verdict.code(),
                want,
                "socket={} → {:?}",
                entry.socket,
                verdict
            );
        }
    }

    #[test]
    fn 出来たてのソケットには触らない() {
        let mut entry = server("tako-iso-4242", true, 0, &[]);
        entry.modified_secs_ago = Some(5);
        assert_eq!(judge_server(&entry, "other", &[]).code(), "too_fresh");
        // 読めない（= 消えた直後かもしれない）ときも触らない
        entry.modified_secs_ago = None;
        assert_eq!(judge_server(&entry, "other", &[]).code(), "too_fresh");
    }

    #[test]
    fn 使い捨てソケットは自分のpidから作った名前だけ() {
        assert!(owns_disposable_socket(42, "tako-iso-42"));
        assert!(owns_disposable_socket(42, "tako-st-42"));
        // 別 pid の名前・明示指定の名前・既定サーバーは対象外
        assert!(!owns_disposable_socket(42, "tako-iso-43"));
        assert!(!owns_disposable_socket(42, "tako-ab1192-fixed"));
        assert!(!owns_disposable_socket(42, "tako"));
    }

    #[test]
    fn 見送りも実行もログ1行になる() {
        let report = CleanupReport::skipped(
            "tako",
            CleanupSkip::PeerSharesSocket {
                pids: vec![9],
                socket: "tako".into(),
            },
        );
        let line = report.log_line(CleanupScope::Explicit).expect("1 行出る");
        assert!(
            line.contains("peer_shares_socket") && line.contains("pid 9"),
            "{line}"
        );
        let report = CleanupReport::killed("tako", vec!["tako-a".into()]);
        let line = report.log_line(CleanupScope::Startup).expect("1 行出る");
        assert!(line.contains("tako-a") && line.contains("起動時"), "{line}");
        // 何も起きなかったときは書かない
        assert_eq!(
            CleanupReport::killed("tako", Vec::new()).log_line(CleanupScope::Explicit),
            None
        );
    }

    // -----------------------------------------------------------------------
    // 名前で列挙する器（Windows / psmux。#1282）
    // -----------------------------------------------------------------------

    /// 実機（Windows 11 / psmux 3.3.7）の器のコマンドラインの形。
    /// **実ユーザー名は置かない**（#927。実機の採取物は `winuser` へ置換してある）
    const WARM_LINE: &str = r"tmux.exe server -s __warm__ -L tako-w1133 -x 120 -y 30";
    const SESSION_LINE: &str = r#"tmux.exe server -s tako-123456789012 -L tako-w1133 -e TAKO_PANE_ID=71 -e "TAKO_OSC_SINK=C:\Users\winuser\AppData\Local\Temp\tako-iso-data-4242\osc\71.osc" -x 120 -y 30"#;

    fn mux(pid: u32, cmd: &str, started: Option<u64>) -> MuxProcess {
        parse_mux_process(pid, cmd, started).expect("器のコマンドラインを解ける")
    }

    #[test]
    fn コマンドラインを引用符ごと語へ割る() {
        assert_eq!(
            split_command_line(WARM_LINE),
            vec![
                "tmux.exe",
                "server",
                "-s",
                "__warm__",
                "-L",
                "tako-w1133",
                "-x",
                "120",
                "-y",
                "30"
            ]
        );
        // `"` の中の空白では割らない。`\` はエスケープとして解釈しない（Windows のパス）
        let words = split_command_line(r#"tmux.exe -L s -e "A=C:\Program Files\x" run"#);
        assert_eq!(words[4], r"A=C:\Program Files\x");
        assert_eq!(words[5], "run");
    }

    #[test]
    fn issue1282_サーバーとクライアントを見分ける() {
        let warm = mux(11, WARM_LINE, Some(1_000));
        assert_eq!(warm.socket, "tako-w1133");
        assert!(warm.is_server);
        assert_eq!(warm.session.as_deref(), Some(WARM_SESSION));
        assert!(warm.marker_pids.is_empty(), "先読み器には所有者の印が無い");

        let session = mux(12, SESSION_LINE, Some(1_000));
        assert!(session.is_server);
        assert_eq!(session.session.as_deref(), Some("tako-123456789012"));
        assert_eq!(
            session.marker_pids,
            vec![4242],
            "隔離データ置き場から所有者が読める"
        );

        // クライアント（`-L` の値をサブコマンドと読み違えないこと）
        let client = mux(
            13,
            "tmux.exe -L tako-w1133 list-sessions -F #{session_name}",
            None,
        );
        assert_eq!(client.socket, "tako-w1133");
        assert!(!client.is_server);

        // `-S <パス>` はファイル名がソケット名
        let by_path = mux(14, "tmux -S /tmp/tmux-501/tako-iso-99= list-clients", None);
        assert_eq!(by_path.socket, "tako-iso-99");

        // `-L` も `-S` も無い = tako の管理外（触らない）
        assert!(parse_mux_process(15, "tmux.exe server", None).is_none());
    }

    #[test]
    fn issue1282_隔離マーカーから所有pidを読む() {
        assert_eq!(marker_owner_pids(SESSION_LINE), vec![4242]);
        // 5 種類の置き場すべてが同じ規則（`.yaml` や `\` で語が切れる）
        assert_eq!(
            marker_owner_pids(
                r"-e A=C:\t\tako-iso-sessions-777.yaml -e B=C:\t\tako-iso-pane-logs-777"
            ),
            vec![777]
        );
        // 数字が語の一部なら pid と断定しない（`owner_pid_candidates` と同じ規則）
        assert!(marker_owner_pids("tako-iso-data-1090mac").is_empty());
        assert!(marker_owner_pids("印は無い").is_empty());
    }

    /// 実機の形そのまま: 1 つのソケットに先読み器 + セッション器 + クライアント
    #[test]
    fn issue1282_ソケット単位へ畳む() {
        let procs = vec![
            mux(11, WARM_LINE, Some(1_000)),
            mux(12, SESSION_LINE, Some(1_200)),
            mux(13, "tmux.exe -L tako-w1133 list-sessions", Some(1_500)),
        ];
        let entries = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.socket, "tako-w1133");
        assert_eq!(e.kind, ServerKind::Named);
        assert_eq!(e.server_pids, vec![11, 12], "クライアントは器に数えない");
        assert_eq!(e.clients, 1);
        assert_eq!(e.sessions, 1, "先読み器（__warm__）はセッションに数えない");
        assert!(e.running);
        assert_eq!(e.owner_pids, vec![4242]);
        assert_eq!(e.owner_source, OwnerSource::CommandLine);
        // 出来たての判定は**いちばん新しい器**（クライアントは見ない）
        assert_eq!(e.modified_secs_ago, Some(9_000 - 1_200));
    }

    #[test]
    fn issue1282_器が居ないソケットは返さない() {
        let procs = vec![mux(13, "tmux.exe -L tako-iso-9 kill-server", Some(1))];
        assert!(named_server_entries(&procs, |_| false, 9_000, &[]).is_empty());
    }

    #[test]
    fn issue1282_起動時刻が読めない器は出来たて扱い() {
        // 所有者が読める器で見る（所有者不明はそれより前の関門で見送られる）
        let procs = vec![mux(
            11,
            "tmux.exe server -s tako-a -L tako-iso-4242 -x 80 -y 24",
            None,
        )];
        let entries = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(entries[0].modified_secs_ago, None);
        assert_eq!(
            judge_server(&entries[0], "tako", &[]),
            ServerVerdict::TooFresh,
            "材料が読めないときは触らない"
        );
    }

    /// #1282 の本体: ソケット名に pid が埋まっていれば unix と同じ規則で回収できる
    #[test]
    fn issue1282_名前由来の所有者は生死で回収可否が決まる() {
        let line = |sock: &str| format!("psmux.exe server -s tako-abc -L {sock} -x 80 -y 24");
        let procs = vec![mux(21, &line("tako-iso-4242"), Some(1_000))];

        let dead = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(dead[0].owner_source, OwnerSource::SocketName);
        assert_eq!(
            judge_server(&dead[0], "tako", &[]),
            ServerVerdict::Reclaimable { pids: vec![4242] }
        );

        let alive = named_server_entries(&procs, |pid| pid == 4242, 9_000, &[]);
        assert_eq!(
            judge_server(&alive[0], "tako", &[]),
            ServerVerdict::OwnerAlive { pids: vec![4242] }
        );

        // 名前由来は tako-app が生きていても回収してよい（その名前は再利用されない）
        let with_app = named_server_entries(&procs, |_| false, 9_000, &[777]);
        assert_eq!(
            judge_server(&with_app[0], "tako", &[]),
            ServerVerdict::Reclaimable { pids: vec![4242] }
        );
    }

    /// コマンドライン由来の所有者は**ソケット名が再利用されうる**ので、
    /// 生きた tako-app が居る間は回収しない（再 attach された器を落とさない）
    #[test]
    fn issue1282_コマンドライン由来は生きたtako_appが居れば見送る() {
        let procs = vec![mux(12, SESSION_LINE, Some(1_000))];

        let alone = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(
            judge_server(&alone[0], "tako", &[]),
            ServerVerdict::Reclaimable { pids: vec![4242] }
        );

        let with_app = named_server_entries(&procs, |_| false, 9_000, &[5150]);
        assert_eq!(
            judge_server(&with_app[0], "tako", &[]),
            ServerVerdict::PeerMayReattach { pids: vec![5150] }
        );
        assert_eq!(
            judge_server(&with_app[0], "tako", &[]).code(),
            "peer_may_reattach"
        );
        assert!(!judge_server(&with_app[0], "tako", &[]).is_actionable());
    }

    /// 材料が 1 つも取れない器は**見送る**（名前にも印にも pid が無い）
    #[test]
    fn issue1282_所有者不明の器は消さない() {
        let procs = vec![mux(
            31,
            "tmux.exe server -s tako-x -L tako-manual -x 80 -y 24",
            Some(1),
        )];
        let entries = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(entries[0].owner_source, OwnerSource::Unknown);
        assert!(entries[0].owner_pids.is_empty());
        assert_eq!(
            judge_server(&entries[0], "tako", &[]),
            ServerVerdict::OwnerUnknown
        );
    }

    /// 使用中（クライアントが attach 中）と、自分・既定サーバーは Named でも守られる
    #[test]
    fn issue1282_使用中と自分の器は守られる() {
        let procs = vec![
            mux(
                41,
                "tmux.exe server -s tako-a -L tako-iso-4242 -x 80 -y 24",
                Some(1_000),
            ),
            mux(
                42,
                "tmux.exe -L tako-iso-4242 attach -t tako-a",
                Some(1_100),
            ),
        ];
        let entries = named_server_entries(&procs, |_| false, 9_000, &[]);
        assert_eq!(
            judge_server(&entries[0], "tako", &[]),
            ServerVerdict::ClientsAttached { clients: 1 }
        );
        // 自分の backend は名前で守られる
        assert_eq!(
            judge_server(&entries[0], "tako-iso-4242", &[]),
            ServerVerdict::SelfSocket
        );
        // 既定サーバーも守られる
        let default_procs = vec![mux(
            43,
            &format!(
                "tmux.exe server -s tako-a -L {} -x 80 -y 24",
                crate::tmux_backend::DEFAULT_SOCKET
            ),
            Some(1_000),
        )];
        let default_entries = named_server_entries(&default_procs, |_| false, 9_000, &[]);
        assert_eq!(
            judge_server(&default_entries[0], "tako-iso-1", &[]),
            ServerVerdict::DefaultSocket
        );
    }

    /// pid の再利用は**安全側**へ倒す: 所有 pid が生きて見えるなら（別プロセスが
    /// その pid を取っていても）触らない。取り違えて落とすより残すほうがよい
    #[test]
    fn issue1282_所有pidが生きて見えるなら触らない() {
        let procs = vec![mux(
            51,
            "tmux.exe server -s tako-a -L tako-iso-4242 -x 80 -y 24",
            Some(1_000),
        )];
        let entries = named_server_entries(&procs, |pid| pid == 4242, 9_000, &[]);
        assert!(!judge_server(&entries[0], "tako", &[]).is_actionable());
    }
}

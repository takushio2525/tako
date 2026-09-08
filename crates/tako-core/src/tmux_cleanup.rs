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
}

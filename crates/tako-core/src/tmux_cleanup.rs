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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(pid: u32, socket: Option<&str>) -> CleanupPeer {
        CleanupPeer {
            pid,
            socket: socket.map(str::to_string),
        }
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

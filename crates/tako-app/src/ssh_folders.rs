//! ペインの `ssh` 検知 → リモートフォルダの自動追加（Issue #976 / #65 要件 1）の
//! アプリ側。
//!
//! 判断は `tako_core::ssh_detect`（コマンド行 → 宛先）と
//! `tako_control::ssh_detect`（どのペインの配下か・再走査の間引き）が持つ。
//! ここが持つのは**追加してよいかの判断**と**切断の見せ方**:
//!
//! - 同じホストのルートがそのタブに既にあれば何もしない（#919 の明示経路と共存し、
//!   「リモート接続…」で開いたフォルダの隣に home が二重に並ばない）
//! - 接続の確認（`connect` + `list_dir`）は**必ず background**。UI スレッドで待つと
//!   自動処理が数百 ms〜数秒のストールとして現れ、原因の見当がつかない（#212 / #772）
//! - 1 エピソード（ssh が生きている間）につき試行は 1 回。失敗しても繰り返さない。
//!   抜けて入り直したら再試行する
//! - **切断してもルートは消さない**（#976 受け入れ条件 3）。行にバッジで状態を出し、
//!   右クリックの「再読み込み」で復帰できる
//!
//! # なぜ勝手にパスワードを聞きに行かないか
//!
//! 接続は `remote_fs::ensure_master`（`BatchMode=yes`）なので、鍵・agent で入れない
//! 相手では**プロンプトを出さずに失敗**する。ユーザーが `ssh` で入れている相手でも、
//! tako が張る接続は別セッションなので再認証が要る場合がある（その場合は理由を
//! 通知に出して見送る = ユーザーが「リモート接続…」で一度入れば ControlMaster を
//! 共有して次から開く）。

use tako_control::ssh_detect::{SshScanState, SshScanTarget};
use tako_core::remote_fs::RemoteRef;

use crate::TakoApp;

/// 同一バイナリで #976 の前の挙動へ戻す A/B（`TAKO_976_LEGACY=1`）。
///
/// 戻るのは 3 つ: **検知しない** / リモートルートを**先頭へ hoist** /
/// ルート名を `host: 末尾要素` にする。検証で「直したものが本当に効いているか」を
/// 同じバイナリの隣同士で比べるための口（#920 / #932 と同じ作法）
pub(crate) fn legacy_mode() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_976_LEGACY").is_some())
}

/// 検知した ssh セッション 1 件に対する自動追加の状態（キーは宛先文字列）
#[derive(Debug, Clone)]
pub(crate) struct SshAutoLink {
    /// 最後に検知したペイン / タブ
    pub pane: u64,
    pub tab: u64,
    /// 最後の走査時点で ssh が生きていたか。false = 切断（行は残す）
    pub live: bool,
    /// このエピソードで自動追加を試したか（成功・失敗とも二度は試さない）
    pub attempted: bool,
    /// 自動で開いたルート（明示的に開かれていたものはここには入らない）
    pub root: Option<RemoteRef>,
    /// 見送り・失敗の理由（`auto` の応答と通知に出す）
    pub note: Option<String>,
}

/// 自動追加の 1 件ぶんの仕事（background で接続を確かめてから器づけする）
#[derive(Debug, Clone)]
pub(crate) struct SshAutoOpenJob {
    pub destination: String,
    pub tab: u64,
}

/// background 専用。接続してリモートのホームを確かめる（`open` と同じ手順）。
///
/// `list_dir` まで通すのは、**開けないルートをツリーへ並べない**ため（#919）
pub(crate) fn probe_remote_home(destination: &str) -> Result<(String, usize), String> {
    /// 「何が起きたか」と「次に何をすべきか」の両方を残す（#919 の原則）。
    /// 生の詳細（3 行目）は通知には長すぎるので `persist.log` 側の話にしない
    fn why(e: tako_core::remote_fs::RemoteError) -> String {
        format!("{} / {}", e.summary(), e.next_step())
    }
    let home = tako_core::remote_fs::connect(destination).map_err(why)?;
    let entries = tako_core::remote_fs::list_dir(destination, &home).map_err(why)?;
    Ok((home, entries.len()))
}

impl TakoApp {
    /// 走査対象のペインを集める（**メモリ上の材料だけ**。OS へ問い合わせない）。
    ///
    /// 自動追加が無効なら空を返す = `should_rescan` が false になり、
    /// プロセス表の採取そのものが起きない
    pub(crate) fn collect_ssh_scan_targets(&self) -> Vec<SshScanTarget> {
        if !self.ssh_auto_folders || legacy_mode() {
            return Vec::new();
        }
        let mut targets: Vec<SshScanTarget> = Vec::new();
        for tab in self.workspace.tabs() {
            for pane in tab.tree().panes() {
                let Some(session) = self.terminals.get(&pane.id()) else {
                    continue;
                };
                targets.push(SshScanTarget {
                    pane: pane.id().as_u64(),
                    tab: tab.id().as_u64(),
                    backend_session: self.backend_sessions.get(&pane.id()).cloned(),
                    child_pid: session.child_pid(),
                    state: session.command_state(),
                });
            }
        }
        // 指紋の比較（`should_rescan`）が並び順に依らないように固定する
        targets.sort_by_key(|t| t.pane);
        targets
    }

    /// **生きている**と思っている ssh を抱えているか。
    ///
    /// 抱えている間だけ、切断の検出のために低頻度の再走査（保険）を許す。
    /// 「一度検知したホストがある」ではなく「いま生きている」で見るのが要点:
    /// 全部切断済みなら保険は不要（入り直せばコマンド状態が動いて指紋が変わる）で、
    /// **ssh を使い終わったあとの tako は再び 1 回も `ps` を起動しない**
    pub(crate) fn ssh_auto_tracked(&self) -> bool {
        self.ssh_links.values().any(|link| link.live)
    }

    /// 走査結果を反映し、**これから接続を確かめるべき仕事**を返す。
    ///
    /// ここではネットワークへ触らない（UI スレッドで呼ばれる）
    pub(crate) fn apply_ssh_scan(&mut self, state: SshScanState) -> Vec<SshAutoOpenJob> {
        let mut jobs: Vec<SshAutoOpenJob> = Vec::new();
        for session in &state.sessions {
            let entry = self
                .ssh_links
                .entry(session.destination.clone())
                .or_insert(SshAutoLink {
                    pane: session.pane,
                    tab: session.tab,
                    live: false,
                    attempted: false,
                    root: None,
                    note: None,
                });
            entry.pane = session.pane;
            entry.tab = session.tab;
            entry.live = true;
            if entry.attempted {
                continue;
            }
            entry.attempted = true;
            // 明示的に開いてあるならそれを尊重する（二重に並べない）
            if self.tab_has_remote_host(session.tab, &session.destination) {
                continue;
            }
            jobs.push(SshAutoOpenJob {
                destination: session.destination.clone(),
                tab: session.tab,
            });
        }
        // 見送った形は理由を残す（なぜ出てこないのかを `auto` で説明できるように）
        for skipped in &state.skipped {
            tako_control::diag::persist_log(&format!(
                "ssh 自動追加を見送り: pane={} 理由={}",
                skipped.pane,
                skipped.reason.note().ja()
            ));
        }
        // 消えた宛先は「切断」へ。**ルートは消さない**
        let mut lost: Vec<String> = Vec::new();
        for (destination, link) in self.ssh_links.iter_mut() {
            if state.is_live(destination) {
                continue;
            }
            if link.live {
                link.live = false;
                // 次に入り直したら自動追加をやり直す
                link.attempted = false;
                lost.push(destination.clone());
            }
        }
        for destination in lost {
            tako_control::diag::persist_log(&format!("ssh 切断を検知: {destination}"));
            // 通知を出すのは**ツリーにフォルダが残っているとき**だけ。
            // 開けなかった（見送った・失敗した）ホストにまで「フォルダは残します」と
            // 言うと、無い物の話をすることになる
            if self.has_remote_host_anywhere(&destination) {
                self.set_remote_notice(
                    crate::ui_text::remote_folder::auto_disconnected(&destination),
                    false,
                );
            }
        }
        self.ssh_scan = state;
        jobs
    }

    /// どのタブでもよいので、このホストのリモートルートが開かれているか
    pub(crate) fn has_remote_host_anywhere(&self, host: &str) -> bool {
        self.workspace
            .tabs()
            .iter()
            .any(|t| t.remote_folders().iter().any(|r| r.host == host))
    }

    /// そのタブにこのホストのリモートルートが既にあるか
    pub(crate) fn tab_has_remote_host(&self, tab: u64, host: &str) -> bool {
        self.workspace
            .tabs()
            .iter()
            .find(|t| t.id().as_u64() == tab)
            .is_some_and(|t| t.remote_folders().iter().any(|r| r.host == host))
    }

    /// background の接続確認の結果を反映する。
    ///
    /// 成功なら `open` とまったく同じ器づけ（`attach_remote_root`）を通す。
    /// 失敗は**理由を残す**が、ユーザーが頼んだ操作ではないので画面を奪わない
    /// （エラー通知ではなく期限つきの通知 + `persist.log`）
    pub(crate) fn apply_ssh_auto_open(
        &mut self,
        job: &SshAutoOpenJob,
        outcome: Result<(String, usize), String>,
        cx: &mut gpui::Context<Self>,
    ) {
        match outcome {
            Ok((home, entries)) => {
                let Some(tab_id) = self
                    .workspace
                    .tabs()
                    .iter()
                    .find(|t| t.id().as_u64() == job.tab)
                    .map(|t| t.id())
                else {
                    return; // 走査から反映までの間にタブが閉じられた
                };
                let remote = RemoteRef::new(job.destination.clone(), home.clone());
                let added =
                    tako_control::dispatch::attach_remote_root(self, remote.clone(), tab_id);
                if let Some(link) = self.ssh_links.get_mut(&job.destination) {
                    link.root = Some(remote.clone());
                    link.note = None;
                }
                tako_control::diag::persist_log(&format!(
                    "ssh 自動追加: {} （{entries} 件・追加={added}）",
                    remote.label()
                ));
                self.set_remote_notice(
                    crate::ui_text::remote_folder::auto_added(&remote.label()),
                    false,
                );
            }
            Err(reason) => {
                if let Some(link) = self.ssh_links.get_mut(&job.destination) {
                    link.note = Some(reason.clone());
                }
                tako_control::diag::persist_log(&format!(
                    "ssh 自動追加に失敗: {} （{reason}）",
                    job.destination
                ));
                self.set_remote_notice(
                    crate::ui_text::remote_folder::auto_skipped(&job.destination, &reason),
                    false,
                );
            }
        }
        cx.notify();
    }

    /// リモートルート行に出す SSH の状態（None = 検知したことのないホスト）
    pub(crate) fn ssh_link_of_host(&self, host: &str) -> Option<&SshAutoLink> {
        self.ssh_links.get(host)
    }

    /// `remote-folder auto` の応答本体（#976。**検知していない / 検知できない**を区別する）
    pub(crate) fn ssh_auto_status_json(&self) -> serde_json::Value {
        let sessions: Vec<serde_json::Value> = self
            .ssh_links
            .iter()
            .map(|(destination, link)| {
                serde_json::json!({
                    "destination": destination,
                    "pane": link.pane,
                    "tab": link.tab,
                    // live = ssh が生きている / false = 切断（ルートは残っている）
                    "state": if link.live { "live" } else { "disconnected" },
                    "auto_added": link.root.as_ref().map(|r| r.label()),
                    "note": link.note,
                })
            })
            .collect();
        let skipped: Vec<serde_json::Value> = self
            .ssh_scan
            .skipped
            .iter()
            .map(|s| {
                serde_json::json!({
                    "pane": s.pane,
                    "reason": s.reason.label(),
                })
            })
            .collect();
        // 「見ていない」「見られない」「見た」を混同しない（#919 の状態表示と同じ思想）
        let detection = match (self.ssh_scan.scanned_at, self.ssh_scan.argv_unavailable) {
            // pending = まだ一度も走査していない（無効・全ペイン idle で走らせる必要が無い）
            (None, _) => "pending",
            // unavailable = プロセスのコマンド行を採れない環境（Windows。#976 は Pending）
            (Some(_), true) => "unavailable",
            (Some(_), false) => "active",
        };
        serde_json::json!({
            "detection": detection,
            "panes_watched": self.ssh_scan.targets.len(),
            "sessions": sessions,
            "skipped": skipped,
            "note": "ペインで ssh に入ると、そのホストのホームがツリーへ自動で並ぶ。\
                切断してもフォルダは残り、行に状態が出る（#976）",
        })
    }
}

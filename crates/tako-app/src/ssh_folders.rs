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
use tako_core::PaneId;

/// 同一バイナリで #976 の前の挙動へ戻す A/B（`TAKO_976_LEGACY=1`）。
///
/// 戻るのは 3 つ: **検知しない** / リモートルートを**先頭へ hoist** /
/// ルート名を `host: 末尾要素` にする。検証で「直したものが本当に効いているか」を
/// 同じバイナリの隣同士で比べるための口（#920 / #932 と同じ作法）
pub(crate) fn legacy_mode() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_976_LEGACY").is_some())
}

/// 同一バイナリで #1041 の前の挙動へ戻す A/B（`TAKO_1041_LEGACY=1`）。
///
/// 戻るのは 2 つ: リモートルートを**経路を問わず全部ローカルの後ろ**へ並べる /
/// フォルダを開いてもターミナルを自動で繋がない
pub(crate) fn legacy_1041() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1041_LEGACY").is_some())
}

/// いま有効なリモートルートの並び規則（#1041）。
///
/// 3 世代の A/B をここ 1 か所で解く（規則そのものは
/// `tako_core::sidebar::remote_root_order` が持つ）。**`remote-folder list` も
/// `ControlHost::remote_root_placement` からこれを引く**ので、画面と応答が食い違わない。
/// #976 の legacy が勝つのは、そちらが「自動検知そのものを止める」= より古い世代だから
pub(crate) fn remote_root_placement() -> tako_core::sidebar::RemoteRootPlacement {
    use tako_core::sidebar::RemoteRootPlacement;
    if legacy_mode() {
        RemoteRootPlacement::AllLeading
    } else if legacy_1041() {
        RemoteRootPlacement::AllTrailing
    } else {
        RemoteRootPlacement::ExplicitFirst
    }
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
    pub(crate) fn apply_ssh_scan(&mut self, mut state: SshScanState) -> Vec<SshAutoOpenJob> {
        // #1041: **tako が開いた SSH ペイン**は、tako が知っているホスト名
        // （`~/.ssh/config` の Host）へ読み替える。
        //
        // `remote_ssh_argv` は config の `User` を宛先へ反映する（`ssh -o … user@host`）
        // ので、検知側が argv から採る宛先は `user@host`。明示 open で開いたルートは
        // 別名（`host`）なので、そのままだと**同じホストが 2 行並ぶ**
        // （実測: `<host>` を開くと `<remoteuser>@<host>` が自動追加された）。
        // #1041 で「フォルダを開いたら必ず SSH ペインが立つ」になったので、
        // これは日常的に起きる。
        //
        // 読み替えは**キーの正規化**であって検知の抑止ではない: ルートを持たない
        // ペイン（`tako open-in remote <host>` で繋いだだけ）はこれまでどおり
        // 自動追加され、名前が別名になるぶん明示経路と突き合わせられるようになる
        for session in &mut state.sessions {
            if let Some(known) = self
                .ssh_connect
                .get(&PaneId::from_raw(session.pane))
                .map(|st| st.host.clone())
            {
                session.destination = known;
            }
        }
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
            .any(|t| t.remote_folders().iter().any(|f| f.remote.host == host))
    }

    /// そのタブにこのホストのリモートルートが既にあるか
    pub(crate) fn tab_has_remote_host(&self, tab: u64, host: &str) -> bool {
        self.workspace
            .tabs()
            .iter()
            .find(|t| t.id().as_u64() == tab)
            .is_some_and(|t| t.remote_folders().iter().any(|f| f.remote.host == host))
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
                // #1041: 自動検知ぶんは `Auto` = ローカルルートの後ろのまま（#976 に回帰ゼロ）
                let added = tako_control::dispatch::attach_remote_root(
                    self,
                    tako_core::remote_fs::RemoteFolder::auto(remote.clone()),
                    tab_id,
                );
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

// --- #1040: 切断したリモートフォルダの自動復帰 -------------------------------

/// 1 ホストぶんの復帰待ち。**切断を観測している間だけ**存在する
/// （= 平常時はネットワークへ 1 回も触らない）
#[derive(Debug, Clone)]
pub(crate) struct RemoteRecovery {
    /// 切断を観測した時刻（表示と診断用）
    pub since: std::time::Instant,
    /// 最後に繋ぎ直しを試した時刻
    pub last_probe: Option<std::time::Instant>,
    /// 試した回数
    pub probes: u32,
    /// background の試行が走っている最中か（二重起動を防ぐ）
    pub in_flight: bool,
}

impl RemoteRecovery {
    fn new() -> Self {
        Self {
            since: std::time::Instant::now(),
            last_probe: None,
            probes: 0,
            in_flight: false,
        }
    }

    fn waited_secs(&self) -> u64 {
        self.last_probe.unwrap_or(self.since).elapsed().as_secs()
    }
}

/// background の試行の結果
#[derive(Debug, Clone)]
pub(crate) struct RemoteRecoveryOutcome {
    pub host: String,
    /// 接続が戻ったか
    pub connected: bool,
    /// 戻らなかった理由（`connected == false` のとき）
    pub reason: Option<String>,
    /// 押し出せた保留中の保存の件数（#966 の pending）
    pub pushed: usize,
}

/// background 専用。接続を試し、戻っていたら保留中の書き戻しも押し出す（#1040 要件 3）。
///
/// **ネットワーク I/O をここに閉じ込める**（UI スレッドで待たせない = #212 / #772 の教訓）。
/// `push` は #966 の 1 実装をそのまま呼ぶので、競合の扱い（`conflict`）も同じ
pub(crate) fn probe_and_push(host: &str) -> RemoteRecoveryOutcome {
    if let Err(e) = tako_core::remote_fs::connect(host) {
        return RemoteRecoveryOutcome {
            host: host.to_string(),
            connected: false,
            reason: Some(format!("{} / {}", e.summary(), e.next_step())),
            pushed: 0,
        };
    }
    // 切断中の保存は消えていない（#966）。戻った合図で自分から押し出す。
    // **force はしない**: 相手が変わっていたら `conflict` として残す（既存の分類に従う）
    let mut pushed = 0usize;
    for entry in tako_core::remote_fs::list_pending()
        .into_iter()
        .filter(|e| e.host == host)
    {
        match tako_core::remote_fs::push_pending(&entry.host, &entry.path, false) {
            Ok(_) => pushed += 1,
            Err(e) => tako_control::diag::persist_log(&format!(
                "保留中の書き戻しを復帰時に押し出せない: {}:{} （{}）",
                entry.host,
                entry.path,
                e.summary()
            )),
        }
    }
    RemoteRecoveryOutcome {
        host: host.to_string(),
        connected: true,
        reason: None,
        pushed,
    }
}

impl TakoApp {
    /// ツリーに出ているリモートホストの接続を見て、復帰待ちを進める（#1040）。
    ///
    /// 戻り値は**これから background で繋ぎ直しに行くホスト**。
    /// 判定材料は `remote_fs::liveness`（= ソケットの stat 1 回）だけなので、
    /// 平常時はホスト数ぶんの stat しか起きない
    pub(crate) fn drive_remote_recovery(&mut self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for tab in self.workspace.tabs() {
            for f in tab.remote_folders() {
                if !hosts.contains(&f.remote.host) {
                    hosts.push(f.remote.host.clone());
                }
            }
        }
        // ツリーから消えたホストの待ちは畳む
        self.remote_recovery.retain(|h, _| hosts.contains(h));

        let mut jobs: Vec<String> = Vec::new();
        let mut restored: Vec<String> = Vec::new();
        for host in hosts {
            match tako_core::remote_fs::liveness(&host) {
                tako_core::remote_fs::Liveness::Live => {
                    // 待っていたものが戻った（ペインの再接続で戻る場合もここを通る）
                    if self.remote_recovery.remove(&host).is_some() {
                        restored.push(host);
                    }
                    continue;
                }
                // #1090: 多重化が無いプラットフォームには「生きている接続」が無いので
                // 生死を判定できない。**切断と決めつけて繋ぎ直しに行かない**
                // （行くと平常時に延々と probe を撃ち続ける）
                tako_core::remote_fs::Liveness::Unknown => {
                    self.remote_recovery.remove(&host);
                    continue;
                }
                tako_core::remote_fs::Liveness::Dead => {}
            }
            let entry = self
                .remote_recovery
                .entry(host.clone())
                .or_insert_with(RemoteRecovery::new);
            if entry.in_flight {
                continue;
            }
            if !tako_core::ssh_reconnect::folder_should_probe(entry.probes, entry.waited_secs()) {
                continue;
            }
            entry.in_flight = true;
            entry.probes += 1;
            entry.last_probe = Some(std::time::Instant::now());
            jobs.push(host);
        }
        for host in restored {
            self.finish_remote_recovery(&host, 0);
        }
        jobs
    }

    /// ペインの再接続が通ったので、同じホストのツリーも起こす（#1040）。
    ///
    /// まだ切断を観測していなくても、**次の tick で必ず読み直す**ように待ちを作る
    pub(crate) fn wake_remote_recovery(&mut self, host: &str) {
        if !self.has_remote_host_anywhere(host) {
            return;
        }
        self.remote_recovery
            .entry(host.to_string())
            .or_insert_with(RemoteRecovery::new)
            .last_probe = None;
    }

    /// background の結果を反映する（#1040）
    pub(crate) fn apply_remote_recovery(
        &mut self,
        outcome: RemoteRecoveryOutcome,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(entry) = self.remote_recovery.get_mut(&outcome.host) {
            entry.in_flight = false;
        }
        if !outcome.connected {
            if let Some(reason) = &outcome.reason {
                tako_control::diag::persist_log(&format!(
                    "リモートフォルダの自動復帰に失敗: {} （{reason}）",
                    outcome.host
                ));
            }
            // 上限まで試したら**静かに止まる**（行の「切断」バッジと
            // 右クリックの「再読み込み」が残る = #976 の従来動作）
            return;
        }
        self.remote_recovery.remove(&outcome.host);
        self.finish_remote_recovery(&outcome.host, outcome.pushed);
        cx.notify();
    }

    /// 接続が戻ったホストのツリーを読み直し、結果を通知する（#1040）
    fn finish_remote_recovery(&mut self, host: &str, pushed: usize) {
        let roots: Vec<tako_core::remote_fs::RemoteRef> = self
            .workspace
            .tabs()
            .iter()
            .flat_map(|t| t.remote_refs())
            .filter(|r| r.host == host)
            .collect();
        if roots.is_empty() {
            return;
        }
        for root in &roots {
            self.filetree.invalidate_remote(root);
        }
        tako_control::diag::persist_log(&format!(
            "リモートフォルダを自動復帰: {host}（{} 件・保留 push {pushed} 件）",
            roots.len()
        ));
        let lang = tako_core::i18n::lang();
        let mut note = tako_core::ssh_reconnect::folder_restored(lang, host);
        if pushed > 0 {
            note.push_str(" / ");
            note.push_str(&tako_core::ssh_reconnect::pending_pushed(
                lang, host, pushed,
            ));
        }
        self.set_remote_notice(note, false);
    }
}

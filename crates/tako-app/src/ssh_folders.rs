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
//! - **見送りの診断ログも 1 エピソードにつき 1 行**（#1258）。抑止の鍵は
//!   `tako_control::ssh_detect::SshSkipLog` が持つ
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

use std::collections::{HashMap, HashSet};

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

/// #2969 の A/B の env（`1` = 修正前と同じく、検知の着地（UI スレッド）が引き取りの 1 行を組むときに
/// ログインシェルの `command -v ssh` を待つ）
pub(crate) const ISSUE2969_LEGACY_ENV: &str = "TAKO_2969_LEGACY";

/// #2969 の A/B が立っているか（**門はここ 1 か所**。呼ばれるたびに読む）。呼び手は
/// `if crate::ssh_folders::issue2969_legacy() {` の形で置く（番犬 `issue2956_setup_dispatch_offload_watchdog` の
/// `TAKO_APP_UI_LANDINGS` が A/B の旧経路の腕として読む形）
pub(crate) fn issue2969_legacy() -> bool {
    matches!(
        std::env::var(ISSUE2969_LEGACY_ENV).ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 検知の着地が引き取りの 1 行（`remote_ssh_argv_with`）を組む ssh の実体を、**走査（background）の段で**引く（#2969）。
///
/// 引くのは走査の結果に引き取るペイン（`settled` = 着地の [`TakoApp::adopt_ssh_connect`] が触らないペインの外）が
/// あるときだけ（`remote_fs::ssh_bin` = unix はログインシェルの `command -v ssh`。上限 `login_shell_timeout`・覚えなし）。
/// 引き取り済みのペインばかりの tick（ssh が生きている間のほぼ全部）はログインシェルを起こさない。
/// `settled` が `None`（引き取りが止めてある = #1446 / #2969 の A/B）なら引かない。
/// 引かなかった・見つからなかったときの `None` は、着地で素の `ssh` になる（待たない側）
pub(crate) fn adopt_ssh_bin(state: &SshScanState, settled: Option<&[u64]>) -> Option<String> {
    let settled = settled?;
    state
        .sessions
        .iter()
        .any(|s| !settled.contains(&s.pane))
        .then(tako_core::remote_fs::ssh_bin)
        .flatten()
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

    /// 走査結果を反映し、**これから接続を確かめるべき仕事**を返す（引き取りの 1 行は素の `ssh` で組む。
    /// 定期の tick は走査が引いた ssh の実体を渡す [`Self::apply_ssh_scan_with`] を通る）
    pub(crate) fn apply_ssh_scan(&mut self, state: SshScanState) -> Vec<SshAutoOpenJob> {
        self.apply_ssh_scan_with(state, None)
    }

    /// 走査結果を反映し、**これから接続を確かめるべき仕事**を返す。
    ///
    /// ここではネットワークへ触らず、外部プロセスも待たない（UI スレッドで呼ばれる）。引き取り
    /// （[`Self::adopt_ssh_connect`]）が打ち直す 1 行の ssh の実体は、走査（background）の段が引いた
    /// `adopt_ssh_bin`（[`adopt_ssh_bin`]）で組む（#2969。修正前はここでログインシェルの `command -v ssh` を待った）
    pub(crate) fn apply_ssh_scan_with(
        &mut self,
        mut state: SshScanState,
        adopt_ssh_bin: Option<String>,
    ) -> Vec<SshAutoOpenJob> {
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
        // #1446: 追跡していない SSH ペインをここで引き取る。**検知の材料は既に在る**
        // （#976 がこの走査で `remote-folder auto` を動かしている）ので、新しい走査は
        // 増えない。これが無いと、ユーザーが手で `ssh <host>` と打ったペインは
        // dispatch を通らないので切断しても誰も見ておらず、無言でローカルへ落ちる
        for session in &state.sessions {
            self.adopt_ssh_connect(
                PaneId::from_raw(session.pane),
                &session.destination,
                adopt_ssh_bin.as_deref(),
            );
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
        // 見送った形は理由を残す（なぜ出てこないのかを `auto` で説明できるように）。
        //
        // **1 エピソードにつき 1 行**（#1258）。この関数は 2 秒 tick のたびに呼ばれ、
        // 走査を間引いた tick でも `state.skipped` は前回ぶんが載っている（`scan` が
        // 「見ていない」を「消えた」と読み替えない仕様）ので、判定のたびに書くと
        // 同じ行が ssh の生存中ずっと積もる（実測 3 時間で 866 行）。
        // いま見送られている姿は `remote-folder auto` の `skipped` がいつでも返すので、
        // ログに要るのは「いつ起きたか」の 1 行だけ
        let fresh = self
            .ssh_skip_log
            .take_new(&state.skipped, tako_control::ssh_detect::legacy_1258());
        for skipped in &fresh {
            tako_control::diag::persist_log(&format!(
                "ssh 自動追加を見送り: pane={} pid={} 理由={}",
                skipped.pane,
                skipped.pid,
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
                    // #1258: ログは 1 エピソード 1 行になったので、どの ssh を
                    // 指しているかは pid で突き合わせる
                    "pid": s.pid,
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

/// #2976 の A/B の env（`1` = 修正前と同じく、定期の tick（UI スレッド）が生死の判定
/// `remote_fs::liveness` を待つ）
pub(crate) const ISSUE2976_LEGACY_ENV: &str = "TAKO_2976_LEGACY";

/// #2976 の A/B が立っているか（**門はここ 1 か所**。呼ばれるたびに読む）。呼び手は
/// `if crate::ssh_folders::issue2976_legacy() {` の形で置く（番犬 `issue2956_setup_dispatch_offload_watchdog` の
/// `TAKO_APP_UI_LANDINGS` が A/B の旧経路の腕として読む形）
pub(crate) fn issue2976_legacy() -> bool {
    matches!(
        std::env::var(ISSUE2976_LEGACY_ENV).ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 生死の判定の結果で 1 ホストの復帰待ちをどう進めたか（#1040 の判断。#2976 で判定から切り離した）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryStep {
    /// 何もしない（平常・試行の最中・間隔がまだ・上限まで試した・判定できない）
    Idle,
    /// 繋ぎ直しを background で試す（試行の印・回数・時刻は立ててある）
    Probe,
    /// 待っていたものが戻った（ツリーを読み直して知らせる）
    Restored,
}

/// 生死の判定の結果で 1 ホストの復帰待ちを進める（#1040）。
///
/// **修正前（UI スレッドで判定する A/B）と修正後（背景で判定して着地で進める）の両方が通る 1 実装**（#2976）。
/// 判断は修正前の `drive_remote_recovery` のループの本体そのまま（メモリ操作だけ）
pub(crate) fn advance_recovery(
    waits: &mut HashMap<String, RemoteRecovery>,
    host: &str,
    liveness: tako_core::remote_fs::Liveness,
) -> RecoveryStep {
    match liveness {
        tako_core::remote_fs::Liveness::Live => {
            // 待っていたものが戻った（ペインの再接続で戻る場合もここを通る）
            return if waits.remove(host).is_some() {
                RecoveryStep::Restored
            } else {
                RecoveryStep::Idle
            };
        }
        // #1090: 多重化が無いプラットフォームには「生きている接続」が無いので
        // 生死を判定できない。**切断と決めつけて繋ぎ直しに行かない**
        // （行くと平常時に延々と probe を撃ち続ける）
        tako_core::remote_fs::Liveness::Unknown => {
            waits.remove(host);
            return RecoveryStep::Idle;
        }
        tako_core::remote_fs::Liveness::Dead => {}
    }
    let entry = waits
        .entry(host.to_string())
        .or_insert_with(RemoteRecovery::new);
    if entry.in_flight {
        return RecoveryStep::Idle;
    }
    if !tako_core::ssh_reconnect::folder_should_probe(entry.probes, entry.waited_secs()) {
        return RecoveryStep::Idle;
    }
    entry.in_flight = true;
    entry.probes += 1;
    entry.last_probe = Some(std::time::Instant::now());
    RecoveryStep::Probe
}

/// 生死の判定に出すホストを選び、判定の最中の印を立てる（#2976）。
///
/// **印の立っているホスト（前の tick の判定がまだ着地していない）は出さない** = 同じホストへ 2 本目の
/// `ssh -O check` を起こさない（応答しない master は上限 `mux_control_timeout` まで返らないので、2 秒 tick の
/// たびに 1 本ずつ積み上がる形を作らない）。印は着地（[`TakoApp::land_remote_liveness`]）が外す
pub(crate) fn begin_liveness_judging(
    judging: &mut HashSet<String>,
    hosts: Vec<String>,
) -> Vec<String> {
    hosts
        .into_iter()
        .filter(|host| judging.insert(host.clone()))
        .collect()
}

/// 背景の判定の着地の判断（#2976。[`TakoApp::land_remote_liveness`] の本体・メモリ操作だけ）。
///
/// 判定の最中の印を外し、ホストがまだツリーに居れば [`advance_recovery`] で進める。判定の間にツリーから
/// 外れたホスト（`in_tree == false`）は進めない = 待ちを作らない・繋ぎ直しに行かない（修正前は判定と反映が
/// UI スレッドの 1 回の中だったので、外れたホストを判定することが無かった）
pub(crate) fn land_liveness(
    waits: &mut HashMap<String, RemoteRecovery>,
    judging: &mut HashSet<String>,
    host: &str,
    liveness: tako_core::remote_fs::Liveness,
    in_tree: bool,
) -> RecoveryStep {
    judging.remove(host);
    if !in_tree {
        return RecoveryStep::Idle;
    }
    advance_recovery(waits, host, liveness)
}

/// **background 専用**: ホストの接続の生死を判定する（#2976）。
///
/// `remote_fs::liveness` は unix で ssh の実体をログインシェルの `command -v ssh` で引き
/// （上限 `login_shell_timeout`）、ControlPath のソケットが在れば `ssh -O check` を起こして
/// 上限 `mux_control_timeout`（既定 5 秒）まで待つ。**UI スレッドから呼ばない**（修正前は定期の tick が
/// UI スレッドでホストごとに待っていた = 応答しない master があると tick ごとに GUI が止まった）
pub(crate) fn judge_remote_host(host: &str) -> tako_core::remote_fs::Liveness {
    tako_core::remote_fs::liveness(host)
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
    /// ツリーに出ているリモートホスト（重複なし・タブとフォルダの順）
    fn remote_tree_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for tab in self.workspace.tabs() {
            for f in tab.remote_folders() {
                if !hosts.contains(&f.remote.host) {
                    hosts.push(f.remote.host.clone());
                }
            }
        }
        hosts
    }

    /// ツリーに出ているリモートホストの接続を見て、復帰待ちを進める（#1040。定期の tick = UI スレッド）。
    ///
    /// 生死の判定（`remote_fs::liveness` = unix は ssh の実体をログインシェルで引き、ControlPath の
    /// ソケットが在れば `ssh -O check` を起こして上限まで待つ。**ソケットの stat 1 回ではない**）は
    /// **背景の段**（[`judge_remote_host`]）で行い、着地（[`Self::land_remote_liveness`]）が結果
    /// （Live / Dead / Unknown）だけを受けて復帰待ちを進める（#2976。修正前はここでホストごとに待った =
    /// 応答しない master があると tick ごとに最大 5 秒 × ホスト数 GUI が止まった）。判定の最中のホストは
    /// 次の tick で出さない（[`begin_liveness_judging`]）。繋ぎ直しと保留の押し出しは従来どおり background
    /// （[`probe_and_push`]）。ここはメモリ操作と仕事の積み込みだけ
    pub(crate) fn drive_remote_recovery(&mut self, cx: &mut gpui::Context<Self>) {
        let hosts = self.remote_tree_hosts();
        // ツリーから消えたホストの待ちは畳む
        self.remote_recovery.retain(|h, _| hosts.contains(h));

        if crate::ssh_folders::issue2976_legacy() {
            // 修正前（A/B）: UI スレッドでホストごとに判定を待ってから進める
            let mut jobs: Vec<String> = Vec::new();
            let mut restored: Vec<String> = Vec::new();
            for host in hosts {
                let liveness = tako_core::remote_fs::liveness(&host);
                match advance_recovery(&mut self.remote_recovery, &host, liveness) {
                    RecoveryStep::Probe => jobs.push(host),
                    RecoveryStep::Restored => restored.push(host),
                    RecoveryStep::Idle => {}
                }
            }
            for host in restored {
                self.finish_remote_recovery(&host, 0);
            }
            self.start_remote_recovery_probes(jobs, cx);
            return;
        }

        // **この tick では待たない**（切り離して投げる）: 応答しない master は上限まで返らないので、
        // 待つと定期更新（sleep guard / layout 保存 / ペインログ）がまとめて遅れる。ホストごとに
        // 別の仕事にする（1 台が黙っても他のホストの判定は遅れない）
        for host in begin_liveness_judging(&mut self.remote_liveness_judging, hosts) {
            cx.spawn(async move |this, cx| {
                let judged = cx
                    .background_executor()
                    .spawn({
                        let host = host.clone();
                        async move { judge_remote_host(&host) }
                    })
                    .await;
                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                    app.land_remote_liveness(&host, judged, cx);
                });
            })
            .detach();
        }
    }

    /// 背景の生死の判定の着地（#2976。UI スレッド・メモリ操作と仕事の積み込みだけ）。
    ///
    /// 判断は [`land_liveness`]（印を外し、修正前と同じ判断の 1 実装 [`advance_recovery`] で進める。
    /// 判定の間にツリーから外れたホストは進めない）。着地の区間は perf.log の
    /// `periodic_land:remote_liveness` で測る（UI スレッドで外部プロセスを待てば名指しで出る）
    pub(crate) fn land_remote_liveness(
        &mut self,
        host: &str,
        liveness: tako_core::remote_fs::Liveness,
        cx: &mut gpui::Context<Self>,
    ) {
        let _s = tako_control::diag::perf_span("periodic_land:remote_liveness");
        let in_tree = self.has_remote_host_anywhere(host);
        match land_liveness(
            &mut self.remote_recovery,
            &mut self.remote_liveness_judging,
            host,
            liveness,
            in_tree,
        ) {
            RecoveryStep::Probe => self.start_remote_recovery_probes(vec![host.to_string()], cx),
            RecoveryStep::Restored => {
                self.finish_remote_recovery(host, 0);
                cx.notify();
            }
            RecoveryStep::Idle => {}
        }
    }

    /// 繋ぎ直しと保留の押し出しを background で試し、結果を [`Self::apply_remote_recovery`] へ戻す（#1040）
    fn start_remote_recovery_probes(&mut self, hosts: Vec<String>, cx: &mut gpui::Context<Self>) {
        for host in hosts {
            cx.spawn(async move |this, cx| {
                let outcome = cx
                    .background_executor()
                    .spawn(async move { probe_and_push(&host) })
                    .await;
                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                    app.apply_remote_recovery(outcome, cx);
                });
            })
            .detach();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tako_core::remote_fs::Liveness;
    use tako_core::ssh_reconnect::{folder_probe_secs, FOLDER_MAX_PROBES};

    /// 前の試行（無ければ切断の観測）から `secs` 秒たった待ち
    fn aged(probes: u32, secs: u64, in_flight: bool) -> RemoteRecovery {
        let at = Instant::now()
            .checked_sub(Duration::from_secs(secs))
            .expect("起動からの時間より短い");
        RemoteRecovery {
            since: at,
            last_probe: (probes > 0).then_some(at),
            probes,
            in_flight,
        }
    }

    fn hosts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn issue2976_生きている接続は待ちを畳んで戻ったと言う() {
        let mut waits = HashMap::new();
        // 待ちが無ければ何もしない（平常時）
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Live),
            RecoveryStep::Idle
        );
        assert!(waits.is_empty(), "平常時に待ちを作らない");
        // 待っていたものが戻った
        waits.insert("h".to_string(), aged(2, 3, false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Live),
            RecoveryStep::Restored
        );
        assert!(waits.is_empty());
        // 試行の最中に戻っても戻ったと言う（ペインの再接続で戻る形 = 修正前と同じ）
        waits.insert("h".to_string(), aged(1, 0, true));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Live),
            RecoveryStep::Restored
        );
    }

    #[test]
    fn issue2976_判定できないホストは繋ぎ直しに行かない() {
        let mut waits = HashMap::new();
        for _ in 0..3 {
            assert_eq!(
                advance_recovery(&mut waits, "h", Liveness::Unknown),
                RecoveryStep::Idle
            );
            assert!(waits.is_empty(), "Unknown で待ちを作らない（#1090）");
        }
        // 待ちがあっても畳む（間隔が過ぎていても試さない）
        waits.insert("h".to_string(), aged(1, 600, false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Unknown),
            RecoveryStep::Idle
        );
        assert!(waits.is_empty());
    }

    #[test]
    fn issue2976_切断は間隔の後に繋ぎ直しを試し上限で止まる() {
        let mut waits = HashMap::new();
        // 初めて切断を見た tick は待ちを作るだけ（最初の間隔 5 秒がまだ）
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Idle
        );
        assert_eq!(waits["h"].probes, 0);
        assert!(!waits["h"].in_flight);
        // 間隔が過ぎたら試す（印・回数・時刻を立てる）
        waits.insert("h".to_string(), aged(0, folder_probe_secs(0), false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Probe
        );
        assert!(waits["h"].in_flight);
        assert_eq!(waits["h"].probes, 1);
        // 試行の最中は重ねない
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Idle
        );
        assert_eq!(waits["h"].probes, 1);
        // 次の間隔の手前は待つ・過ぎたら試す
        waits.insert("h".to_string(), aged(1, folder_probe_secs(1) - 1, false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Idle
        );
        waits.insert("h".to_string(), aged(1, folder_probe_secs(1), false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Probe
        );
        // 上限まで試したら静かに止まる（待ちは残る = 行の「切断」バッジ）
        waits.insert("h".to_string(), aged(FOLDER_MAX_PROBES, 100_000, false));
        assert_eq!(
            advance_recovery(&mut waits, "h", Liveness::Dead),
            RecoveryStep::Idle
        );
        assert!(waits.contains_key("h"));
    }

    #[test]
    fn issue2976_判定の最中のホストは次のtickで出さない() {
        let mut judging = HashSet::new();
        assert_eq!(
            begin_liveness_judging(&mut judging, hosts(&["a", "b"])),
            hosts(&["a", "b"])
        );
        // 前の判定が着地していない間の tick: a / b は出さない（2 本目の `ssh -O check` を起こさない）。
        // 新しく出てきた c だけ出す
        for _ in 0..3 {
            let next = begin_liveness_judging(&mut judging, hosts(&["a", "b", "c"]));
            assert!(
                !next.contains(&"a".to_string()) && !next.contains(&"b".to_string()),
                "判定の最中のホストを重ねた: {next:?}"
            );
            assert!(next.is_empty() || next == hosts(&["c"]));
        }
        // a が着地したら次の tick で a だけ出す
        let mut waits = HashMap::new();
        assert_eq!(
            land_liveness(&mut waits, &mut judging, "a", Liveness::Live, true),
            RecoveryStep::Idle
        );
        assert_eq!(
            begin_liveness_judging(&mut judging, hosts(&["a", "b", "c"])),
            hosts(&["a"])
        );
        // 同じ tick にホストが重複して来ても 1 本
        let mut judging = HashSet::new();
        assert_eq!(
            begin_liveness_judging(&mut judging, hosts(&["d", "d"])),
            hosts(&["d"])
        );
    }

    #[test]
    fn issue2976_判定の間にツリーから外れたホストは進めない() {
        let mut waits = HashMap::new();
        let mut judging = HashSet::new();
        begin_liveness_judging(&mut judging, hosts(&["h"]));
        assert_eq!(
            land_liveness(&mut waits, &mut judging, "h", Liveness::Dead, false),
            RecoveryStep::Idle
        );
        assert!(waits.is_empty(), "外れたホストの待ちを作らない");
        assert!(
            judging.is_empty(),
            "印は外す（戻ってきたら次の tick で判定する）"
        );
        // 待ちがあっても繋ぎ直しに行かない（畳むのは次の tick の retain = 修正前と同じ）
        waits.insert("h".to_string(), aged(0, 600, false));
        begin_liveness_judging(&mut judging, hosts(&["h"]));
        assert_eq!(
            land_liveness(&mut waits, &mut judging, "h", Liveness::Dead, false),
            RecoveryStep::Idle
        );
        assert!(!waits["h"].in_flight);
    }

    /// 修正前（UI スレッドで判定して進める）と修正後（背景で判定 → 着地で進める）を、同じ生死の並びで
    /// tick ごとに突き合わせる。判定がその tick のうちに着地する（= 返る master）なら、どの tick の
    /// 進め方も待ちの状態も同じ
    #[test]
    fn issue2976_返る判定では修正前と同じ進み方になる() {
        use Liveness::{Dead, Live, Unknown};
        let script: &[&[(&str, Liveness)]] = &[
            &[("a", Live), ("b", Dead), ("c", Unknown)],
            &[("a", Dead), ("b", Dead), ("c", Unknown)],
            &[("a", Dead), ("b", Live), ("c", Dead)],
            &[("a", Live), ("b", Live), ("c", Unknown)],
        ];
        let mut legacy_waits: HashMap<String, RemoteRecovery> = HashMap::new();
        let mut waits: HashMap<String, RemoteRecovery> = HashMap::new();
        let mut judging = HashSet::new();
        for (tick, results) in script.iter().enumerate() {
            // 間隔の判断を満たすため、毎 tick 前に待ちを十分古くする（両方に同じ操作）
            for w in legacy_waits.values_mut().chain(waits.values_mut()) {
                let old = aged(w.probes, 600, false);
                w.since = old.since;
                w.last_probe = w.last_probe.map(|_| old.since);
                w.in_flight = false;
            }
            let legacy: Vec<RecoveryStep> = results
                .iter()
                .map(|(h, l)| advance_recovery(&mut legacy_waits, h, *l))
                .collect();
            let names: Vec<String> = results.iter().map(|(h, _)| h.to_string()).collect();
            let out = begin_liveness_judging(&mut judging, names.clone());
            assert_eq!(out, names, "tick {tick}: 着地済みなら全ホストを判定に出す");
            let fixed: Vec<RecoveryStep> = results
                .iter()
                .map(|(h, l)| land_liveness(&mut waits, &mut judging, h, *l, true))
                .collect();
            assert_eq!(legacy, fixed, "tick {tick}: 進め方が修正前と違う");
            let view = |m: &HashMap<String, RemoteRecovery>| {
                let mut v: Vec<(String, u32, bool)> = m
                    .iter()
                    .map(|(h, w)| (h.clone(), w.probes, w.in_flight))
                    .collect();
                v.sort();
                v
            };
            assert_eq!(
                view(&legacy_waits),
                view(&waits),
                "tick {tick}: 待ちの状態が違う"
            );
            assert!(judging.is_empty());
        }
    }
}

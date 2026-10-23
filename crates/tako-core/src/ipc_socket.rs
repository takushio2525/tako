//! ipc_socket — Layer 1 IPC の Unix ドメインソケットの置き場（#1441）
//!
//! ## 直した症状
//!
//! ソケットの実体を `<data_dir>/tako.sock` に置いていたので、**パス長が data dir に
//! 比例していた**。`TAKO_ISOLATED=1 TAKO_DATA_DIR=<深いパス>` の隔離起動（worker の
//! scratchpad はセッション ID 込みで深くなる）では `sun_path` の上限を超えて bind が
//! 失敗し、`warning: IPC サーバーを起動できない` の 1 行だけで GUI は普通に立つ。
//! CLI / MCP からは一切操作できないのに見た目は正常なので、原因に気づくまで時間を失う
//! （#782 の計測中に実測: data dir 150 バイト = ソケット 160 バイト > 上限 103）。
//!
//! ## 決め方（この 1 実装が正）
//!
//! 1. `<data_dir>/tako.sock` が上限に収まるならそこ。**浅いパスの既存挙動は変えない**
//!    （再起動をまたいで同じパス = 既存クライアントがそのまま繋ぎ直せる、が固定パスの
//!    存在理由）
//! 2. 収まらないなら `<temp_dir>/tako-<data dir の 16 桁ハッシュ>.sock` へ逃がす。
//!    **pid ではなくハッシュ**なのは 1 の性質（再起動をまたいだ安定）を保つため
//! 3. `$TMPDIR` 自体が深い機のために [`FALLBACK_TEMP`] も候補に持つ
//! 4. どれも収まらなければ [`SocketPathKind::Overflow`]。bind は失敗するが、
//!    **黙って縮退させない**ために理由（バイト長と上限）を申告する口を持つ
//!
//! data dir 側には参照ファイル [`POINTER_NAME`]（実体パスを 1 行）だけを残す。
//! **実体を symlink にしても解決にならない**: `sun_path` の上限は繋ぐ側が
//! `connect()` へ渡すパスに掛かるので、長いパスの symlink を置いても
//! クライアントは同じ上限で弾かれる。だから参照は「読んで短いパスへ繋ぎ直す」
//! ためのテキストにしてある。
//!
//! ## 繋ぐ側との関係
//!
//! 通常の CLI / MCP は discovery（`control.json`）に書かれた**実体パス**を読むので、
//! ここの決め方を知らなくても繋がる。知る必要があるのは discovery ごと失われたとき
//! （= まさに bind が失敗した隔離起動）の診断で、そこは [`resolve_with`] が
//! bind 側と同じ 1 実装を通る。

use std::path::{Path, PathBuf};

/// 固定ソケットのファイル名（`<data_dir>/tako.sock`）
pub const WELL_KNOWN_NAME: &str = "tako.sock";

/// data dir 側に残す参照ファイル名（中身は実体パス 1 行）
pub const POINTER_NAME: &str = "tako.sock.path";

/// `$TMPDIR` 自体が上限に収まらない機のための最後の候補
pub const FALLBACK_TEMP: &str = "/tmp";

/// `sockaddr_un.sun_path` の要素数（**終端 NUL を含む**）。macOS / BSD = 104
pub const SUN_PATH_BSD: usize = 104;
/// 同上。Linux = 108
pub const SUN_PATH_LINUX: usize = 108;

/// この OS でパスに使える最大バイト数（終端 NUL を除く）。
///
/// 実測（macOS 25.4 / 2026-09-13）: 103 バイトまで bind 成功・104 バイトで
/// `AF_UNIX path too long`。std も `bytes.len() >= sun_path.len()` で弾く。
///
/// 純関数側（[`plan_with`] / [`resolve_with`]）が上限を**引数で受ける**ので、
/// macOS から Linux の上限も検査できる（#515 / #905 と同じ作法）
pub const fn max_path_bytes() -> usize {
    if cfg!(target_os = "linux") {
        SUN_PATH_LINUX - 1
    } else {
        SUN_PATH_BSD - 1
    }
}

/// ソケットパスの決まり方（診断・`check_health` に出す識別子）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketPathKind {
    /// `<data_dir>/tako.sock`（従来どおり。浅い data dir はここ）
    WellKnown,
    /// data dir が深すぎるので短いパスへ逃がした
    Shortened,
    /// 逃がし先も上限に収まらない（bind は失敗する）
    Overflow,
    /// pid 入りの一時パス（単体テスト / セルフテスト / セカンダリモード）
    Temp,
    /// Windows の名前付きパイプ（`sun_path` の上限は無い）
    NamedPipe,
}

impl SocketPathKind {
    /// JSON・診断へ出す ASCII 名（`NoticeArea::tag` と同じ作法）
    pub fn as_str(self) -> &'static str {
        match self {
            SocketPathKind::WellKnown => "well_known",
            SocketPathKind::Shortened => "shortened",
            SocketPathKind::Overflow => "overflow",
            SocketPathKind::Temp => "temp",
            SocketPathKind::NamedPipe => "named_pipe",
        }
    }
}

/// 「どこへ bind するか」の決定（[`plan_with`] の結果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPlan {
    /// 実際に bind / connect するパス
    pub path: PathBuf,
    pub kind: SocketPathKind,
    /// data dir 側に残す参照ファイル（[`SocketPathKind::WellKnown`] 以外で `Some`）
    pub pointer: Option<PathBuf>,
    /// この判定に使った上限（バイト）
    pub limit: usize,
    /// `<data_dir>/tako.sock` のバイト長（短縮した理由がこれ）
    pub well_known_bytes: usize,
}

impl SocketPlan {
    /// 実体パスのバイト長
    pub fn path_bytes(&self) -> usize {
        path_bytes(&self.path)
    }

    /// 上限を超えている理由を 1 行で。**パスそのものは出さない**
    /// （診断ログへ載せるのは長さと上限だけ = `log_ui_failure` と同じ作法）
    pub fn length_note(&self) -> String {
        format!(
            "kind={} path_bytes={} well_known_bytes={} limit={}",
            self.kind.as_str(),
            self.path_bytes(),
            self.well_known_bytes,
            self.limit,
        )
    }
}

/// パスのバイト長（`sun_path` へ入るのはこのバイト列）
pub fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// data dir ごとに安定した短いファイル名（`tako-<16 桁>.sock`）。
///
/// `/a/b` と `/a/b/` が別のソケットにならないよう、区切りの揺れは
/// `Path::components` で正規化してからハッシュする
pub fn short_name(data_dir: &Path) -> String {
    let normalized: PathBuf = data_dir.components().collect();
    let key = normalized.as_os_str().as_encoded_bytes();
    format!("tako-{:016x}.sock", crate::fnv::fnv1a64(key))
}

/// 置き場を決める（**純関数**。env も実ファイルも見ないので、
/// macOS から Linux の上限・深い `$TMPDIR` の機も検査できる）
pub fn plan_with(data_dir: &Path, temp_dir: &Path, limit: usize) -> SocketPlan {
    let well_known = data_dir.join(WELL_KNOWN_NAME);
    let well_known_bytes = path_bytes(&well_known);
    if well_known_bytes <= limit {
        return SocketPlan {
            path: well_known,
            kind: SocketPathKind::WellKnown,
            pointer: None,
            limit,
            well_known_bytes,
        };
    }
    let name = short_name(data_dir);
    let pointer = Some(data_dir.join(POINTER_NAME));
    let mut shortest: Option<PathBuf> = None;
    for base in [temp_dir, Path::new(FALLBACK_TEMP)] {
        let candidate = base.join(&name);
        if path_bytes(&candidate) <= limit {
            return SocketPlan {
                path: candidate,
                kind: SocketPathKind::Shortened,
                pointer,
                limit,
                well_known_bytes,
            };
        }
        if shortest
            .as_ref()
            .is_none_or(|s| path_bytes(&candidate) < path_bytes(s))
        {
            shortest = Some(candidate);
        }
    }
    SocketPlan {
        path: shortest.unwrap_or(well_known),
        kind: SocketPathKind::Overflow,
        pointer,
        limit,
        well_known_bytes,
    }
}

/// #1441 の A/B。`TAKO_1441_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
/// （置き場を `<data_dir>/tako.sock` 直置きに固定する = 深い data dir では bind が
/// `sun_path` で落ち、「警告 1 行で黙って縮退する」が再現する）。
///
/// 画面へ出す側の宣言は `tako-app` の `TakoApp::legacy_1441`（番犬がアームごとの
/// 宣言をそちらに求めるため）。**同じ env を読んでいること**は #1441 の番犬が照合する
pub fn legacy_1441() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1441_LEGACY").map(|v| v == "1") == Ok(true))
}

/// env を読む入口（`TAKO_DATA_DIR` → [`crate::paths::data_dir`]、`std::env::temp_dir()`）
pub fn plan() -> Option<SocketPlan> {
    let data_dir = crate::paths::data_dir()?;
    if legacy_1441() {
        // 旧アーム: 深さに関わらず data dir 直下へ置く（bind は上限で落ちる）
        let path = data_dir.join(WELL_KNOWN_NAME);
        let well_known_bytes = path_bytes(&path);
        return Some(SocketPlan {
            path,
            kind: SocketPathKind::WellKnown,
            pointer: None,
            limit: max_path_bytes(),
            well_known_bytes,
        });
    }
    Some(plan_with(
        &data_dir,
        &std::env::temp_dir(),
        max_path_bytes(),
    ))
}

/// data dir 側の参照ファイルを書く（[`SocketPathKind::WellKnown`] では呼ばない）。
/// 中身は実体パス 1 行。読めなくても本体の動作は変わらない（診断のための保険）
pub fn write_pointer(plan: &SocketPlan) -> std::io::Result<()> {
    let Some(pointer) = plan.pointer.as_ref() else {
        return Ok(());
    };
    if let Some(dir) = pointer.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(pointer, format!("{}\n", plan.path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(pointer, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// data dir 側の参照ファイルを読む（無ければ `None`）
pub fn read_pointer(data_dir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(data_dir.join(POINTER_NAME)).ok()?;
    let line = raw.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// data dir から「いま繋ぐべきソケットパス」を引く。
///
/// **参照ファイルがあればそれが正**（実際に bind した側が書いた値）。無ければ
/// [`plan_with`] と同じ決め方で導く = 繋ぐ側と bind 側が 1 実装を通る
pub fn resolve_with(data_dir: &Path, temp_dir: &Path, limit: usize) -> PathBuf {
    read_pointer(data_dir).unwrap_or_else(|| plan_with(data_dir, temp_dir, limit).path)
}

/// 起動時に 1 回だけ記録する「IPC の受け口が立ったか」の実測（#1441）。
///
/// GUI を立てずに検証できるよう、`check_health` の文と重さは
/// [`IpcStatus::health_issue`] が持つ（#1160 の `Placement` と同じ作法）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcStatus {
    /// 立った受け口（unix = ソケットパス / windows = パイプ名）。失敗なら `None`
    pub endpoint: Option<String>,
    pub kind: SocketPathKind,
    pub bound: bool,
    /// 試したパスのバイト長（named pipe は 0）
    pub path_bytes: usize,
    /// この OS の上限（named pipe は 0）
    pub limit: usize,
    /// `<data_dir>/tako.sock` のバイト長（短縮した理由。named pipe は 0）
    pub well_known_bytes: usize,
    /// 失敗の理由（`io::Error` の表示）
    pub error: Option<String>,
}

impl IpcStatus {
    /// 上限超過で立たなかったか（案内の出し分けに使う）
    pub fn too_long(&self) -> bool {
        !self.bound && self.limit > 0 && self.path_bytes > self.limit
    }

    /// 診断ログ 1 行ぶん。**パスは載せない**（長さと上限だけ）
    pub fn length_note(&self) -> String {
        format!(
            "kind={} bound={} path_bytes={} well_known_bytes={} limit={}",
            self.kind.as_str(),
            self.bound,
            self.path_bytes,
            self.well_known_bytes,
            self.limit,
        )
    }
}

/// プロセス内の記録。`check_health` が読む
static STATUS: std::sync::Mutex<Option<IpcStatus>> = std::sync::Mutex::new(None);

/// IPC の受け口の実測を記録する（サーバー起動の成否どちらでも呼ぶ）。
///
/// **最初に記録できた成功を保つ**。複数ウィンドウの後発は一時パスで立つので、
/// 上書きさせるとプライマリの受け口が見えなくなる。まだ成功が無ければ上書きする
/// （= 失敗は次の成功まで見え続ける）
pub fn record(status: IpcStatus) {
    let Ok(mut slot) = STATUS.lock() else {
        return;
    };
    if slot.as_ref().is_some_and(|s| s.bound) {
        return;
    }
    *slot = Some(status);
}

/// 記録された IPC の受け口（記録が無ければ `None` = GUI 以外のホスト）
pub fn status() -> Option<IpcStatus> {
    STATUS.lock().ok().and_then(|s| s.clone())
}

/// テスト用に記録を消す（本番経路からは呼ばない）
#[doc(hidden)]
pub fn reset_status_for_test() {
    if let Ok(mut slot) = STATUS.lock() {
        *slot = None;
    }
}

// ---------------------------------------------------------------------------
// 受け口の見張り（#2002）
//
// 直した症状: GUI は生きていて accept スレッドも accept で待っているのに、CLI / MCP が
// `Connection refused` のまま戻らなかった（2026-10-12 の本番）。段 1 の切り分けで
// 再現したのは「同じパスへ別の受け口が作り直し、その受け口が後から消えた」形 = GUI の
// 受け口の fd はパスから切り離され、パスには誰も受けない残骸だけが残る。GUI 自身は
// 何も気づかないので、再起動するまで戻らない。
//
// いまは accept スレッドが間隔ごとにパスの実体（dev / ino）を bind した時の値と比べ、
// 消えていた・誰も受けない別の実体に替わっていたら同じパスへ張り直す。
// 判定は [`classify`] の 1 実装（IO は tako-control::ipc が持つ）。
// ---------------------------------------------------------------------------

/// 確かめる間隔の既定（#2002）。正常な間は時間切れのたびに `lstat` を 1 回打つだけ
/// （接続は作らない）ので、CPU / 電力の上で測れる差にならない
pub const WATCH_INTERVAL_MS_DEFAULT: u64 = 10_000;

/// 間隔の下限（`TAKO_IPC_WATCH_MS` で縮めても、これより短い周期にはしない）
pub const WATCH_INTERVAL_MS_MIN: u64 = 50;

/// #2002 の A/B。`TAKO_2002_LEGACY=1` で受け口を見張らない（修正前 = パスから切り離されても
/// accept で待ち続け、終了時はパスの実体を確かめずに消す）。CLI の届かないときの診断も
/// 修正前の表示へ戻す（両側が同じ env を読む）
pub fn legacy_2002() -> bool {
    std::env::var("TAKO_2002_LEGACY").is_ok_and(|v| !v.is_empty())
}

/// 見張る間隔。`None` = 見張らない（A/B の旧挙動）。
/// `TAKO_IPC_WATCH_MS` は検証で縮めるための口（既定 [`WATCH_INTERVAL_MS_DEFAULT`]）
pub fn watch_interval_ms() -> Option<u64> {
    if legacy_2002() {
        return None;
    }
    let ms = std::env::var("TAKO_IPC_WATCH_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(WATCH_INTERVAL_MS_DEFAULT);
    Some(ms.max(WATCH_INTERVAL_MS_MIN))
}

/// ソケットファイルの実体（`lstat` の dev / ino）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketFileId {
    pub dev: u64,
    pub ino: u64,
}

/// パスの 1 回ぶんの見え方（IO は呼び出し側）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathObservation {
    /// ファイルが無い
    Missing,
    /// ソケットファイルがある
    Socket(SocketFileId),
    /// ソケットではないファイルがある（または読めない）
    Other,
}

/// 見張りの 1 回の判定（#2002）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerCheck {
    /// パスが自分の受け口を指している（何もしない）
    Ours,
    /// ファイルが消えていた → 張り直す
    Missing,
    /// 別の実体に替わり、誰も受けていない（落ちたプロセスの残骸）→ 消して張り直す
    ReplacedDead,
    /// 別の実体に替わり、生きた受け口が居る → 奪わない（消えたら次の確認で張り直す）
    ForeignLive,
    /// ソケットではないファイルが置かれた → 触らない
    NotSocket,
}

impl ListenerCheck {
    /// 機械キー（`check_health` の `ipc.watch`）
    pub fn as_str(self) -> &'static str {
        match self {
            ListenerCheck::Ours => "ok",
            ListenerCheck::Missing => "missing",
            ListenerCheck::ReplacedDead => "replaced_dead",
            ListenerCheck::ForeignLive => "foreign_live",
            ListenerCheck::NotSocket => "not_socket",
        }
    }

    /// 張り直すか
    pub fn needs_rebind(self) -> bool {
        matches!(self, ListenerCheck::Missing | ListenerCheck::ReplacedDead)
    }

    /// 人が読む理由（persist.log・check-health）
    pub fn reason(self) -> &'static str {
        match self {
            ListenerCheck::Ours => "受け口はパスに繋がっている",
            ListenerCheck::Missing => "ソケットファイルが消えていた",
            ListenerCheck::ReplacedDead => {
                "ソケットファイルが別の実体に置き換わり、誰も受けていなかった"
            }
            ListenerCheck::ForeignLive => {
                "ソケットファイルが別の受け口に置き換わっている（生きているので奪わずに待つ）"
            }
            ListenerCheck::NotSocket => {
                "ソケットのパスにソケットではないファイルがある（触らずに待つ）"
            }
        }
    }
}

/// パスの見え方から判定する（**判定の 1 実装**）。`foreign_alive` は別の実体に
/// 替わっていたときだけ呼ぶ（そのパスへ 1 本繋いで生きた受け口かを確かめる係）
pub fn classify(
    ours: SocketFileId,
    seen: PathObservation,
    foreign_alive: impl FnOnce() -> bool,
) -> ListenerCheck {
    match seen {
        PathObservation::Socket(id) if id == ours => ListenerCheck::Ours,
        PathObservation::Missing => ListenerCheck::Missing,
        PathObservation::Other => ListenerCheck::NotSocket,
        PathObservation::Socket(_) => {
            if foreign_alive() {
                ListenerCheck::ForeignLive
            } else {
                ListenerCheck::ReplacedDead
            }
        }
    }
}

/// 最後に張り直した記録
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebind {
    /// 張り直した時刻（UNIX ミリ秒）
    pub at_unix_ms: u64,
    pub reason: ListenerCheck,
}

/// 受け口の見張りの記録（`check_health` の `ipc.watch`）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IpcWatch {
    /// 確かめる間隔（ミリ秒）
    pub interval_ms: u64,
    /// 確かめた回数
    pub checks: u64,
    /// 張り直した回数
    pub rebinds: u64,
    /// 最後の判定（まだ確かめていなければ `None`）
    pub last: Option<ListenerCheck>,
    pub last_rebind: Option<Rebind>,
    /// 張り直しに失敗した理由（次の成功で消える）
    pub error: Option<String>,
}

impl IpcWatch {
    pub fn new(interval_ms: u64) -> Self {
        IpcWatch {
            interval_ms,
            ..IpcWatch::default()
        }
    }

    /// 正常か（張り直した後に戻っていれば正常）
    pub fn healthy(&self) -> bool {
        self.error.is_none()
            && !matches!(
                self.last,
                Some(ListenerCheck::ForeignLive | ListenerCheck::NotSocket)
            )
    }

    /// 張り直した 1 行（persist.log。**パスは載せない** = 種別と回数だけ）
    pub fn rebind_log_line(&self, reason: ListenerCheck, kind: SocketPathKind) -> String {
        format!(
            "IPC の受け口を張り直した（#2002）: {}・kind={}・張り直し {} 回目",
            reason.reason(),
            kind.as_str(),
            self.rebinds,
        )
    }
}

/// 見張りの記録（[`STATUS`] に載った受け口の分だけ）
static WATCH: std::sync::Mutex<Option<IpcWatch>> = std::sync::Mutex::new(None);

/// 見張りの記録を置く。**[`status`] の受け口と同じ endpoint のときだけ**置く
/// （後発ウィンドウの一時パス・順番待ちの受け口の見張りで本体の記録を上書きしない）
pub fn record_watch(endpoint: &str, watch: &IpcWatch) {
    let recorded = status()
        .and_then(|s| s.endpoint)
        .is_some_and(|e| e == endpoint);
    if !recorded {
        return;
    }
    if let Ok(mut slot) = WATCH.lock() {
        *slot = Some(watch.clone());
    }
}

/// 見張りの記録（見張っていなければ `None`）
pub fn watch() -> Option<IpcWatch> {
    WATCH.lock().ok().and_then(|s| s.clone())
}

// ---------------------------------------------------------------------------
// ソケットのパスの先客の判定（#2360）
//
// 直した症状: 起動時の生存確認が `connect` 1 回だけで、繋がらなければ `bind` がパスを
// unlink して張り直していた。生きた GUI の受け口が一時的に満杯（backlog あふれ = macOS の
// unix ソケットは ECONNREFUSED を返す。#2002 の段 1 の注入 a1 で 300 本中 27 本）の瞬間に、
// 同じ data dir の 2 つ目の tako-app が多重起動のガード（#113 の control.json の pid）を
// すり抜けて起動すると（control.json が無い・壊れている）、生きた受け口を「死んだ残骸」と
// 読んで奪った。奪われた側は生きたままパスから切り離され、そのペインのシェル
// （TAKO_SOCKET = 固定パス）の CLI / MCP は 2 つ目へ届く（#2002 の「同じパスへ別の受け口が
// 作り直した」の候補の 1 つ）。
//
// いまは「繋がらない」を残骸と読む前に、接続情報（control.json と instances/）に
// そのパスの持ち主として載った生きた tako-app が居ないかを見て、居なければ間を置いて
// 繋ぎ直す。判定は [`judge_occupant`] の 1 実装（IO は tako-control::ipc が持つ）で、
// 起動時の置き場の決定・明示パスの受け口（#1994）・#2002 の見張りが同じものを呼ぶ。
// ---------------------------------------------------------------------------

/// 繋がらなかったときに繋ぎ直す間（ミリ秒・合計 350 ms）。満杯の受け口は accept スレッドが
/// 追いつけば数 ms で空くので、一斉接続の山（#2002 の a1）はこの間に引く。
/// 残骸のときだけ起動がこの分遅れる（落ちた後の初回起動だけ）
pub const LIVENESS_RETRY_DELAYS_MS: [u64; 3] = [50, 100, 200];

/// #2360 の A/B。`TAKO_2360_LEGACY=1` で修正前の生存確認（connect 1 回だけ・持ち主を見ない・
/// 繋ぎ直さない・生きた先客が居てもセカンダリにしない）へ戻す
pub fn legacy_2360() -> bool {
    std::env::var("TAKO_2360_LEGACY").is_ok_and(|v| !v.is_empty())
}

/// ソケットのパスの先客（#2360）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupant {
    /// ファイルが無い（そのまま張る）
    Vacant,
    /// 繋がった = 生きた受け口（奪わない）
    Live,
    /// 繋がらないが、接続情報にこのパスの持ち主として載った生きた tako-app が居る
    /// （受け口が一時的に満杯・詰まっている）= 奪わない
    Busy { pid: u32 },
    /// 繋ぎ直しても繋がらず、生きた持ち主の記録も無い = 残骸（消して張る）
    Dead,
}

impl Occupant {
    /// 生きた受け口が握っているか（奪わない側）
    pub fn holds(self) -> bool {
        matches!(self, Occupant::Live | Occupant::Busy { .. })
    }

    /// 機械キー（`check_health` の `ipc.fixed_path.state`）
    pub fn as_str(self) -> &'static str {
        match self {
            Occupant::Vacant => "vacant",
            Occupant::Live => "live",
            Occupant::Busy { .. } => "busy",
            Occupant::Dead => "dead",
        }
    }

    /// 持ち主の pid（`Busy` だけ）
    pub fn pid(self) -> Option<u32> {
        match self {
            Occupant::Busy { pid } => Some(pid),
            _ => None,
        }
    }

    /// 人が読む理由（persist.log・check-health）
    pub fn reason(self) -> String {
        match self {
            Occupant::Vacant => "ソケットファイルが無い".into(),
            Occupant::Live => "繋がった（生きた受け口が居る）".into(),
            Occupant::Busy { pid } => format!(
                "繋がらないが、接続情報の持ち主（pid {pid}）が生きている（受け口が一時的に満杯か詰まっている）"
            ),
            Occupant::Dead => format!(
                "{} 回繋いでも繋がらず、生きた持ち主の記録も無い（残骸）",
                LIVENESS_RETRY_DELAYS_MS.len() + 1
            ),
        }
    }

    /// 起動時の判定の 1 行（persist.log。**パスは載せない** = 種別だけ）。`Vacant` は書かない
    pub fn startup_log_line(self, kind: SocketPathKind) -> Option<String> {
        match self {
            Occupant::Vacant => None,
            Occupant::Live | Occupant::Busy { .. } => Some(format!(
                "IPC の固定パスに生きた受け口が居るので奪わずに一時パスで立てる（#2360）: {}・kind={}",
                self.reason(),
                kind.as_str(),
            )),
            Occupant::Dead => Some(format!(
                "IPC の固定パスの残骸を消して張り直す（#2360）: {}・kind={}",
                self.reason(),
                kind.as_str(),
            )),
        }
    }
}

/// パスの先客を判定する（**判定の 1 実装**。IO は呼び出し側）。
///
/// - `exists`: パスにファイルがあるか
/// - `connect`: 1 本繋いで受け口が受けたか（呼ぶたびに 1 回繋ぐ）
/// - `live_owner`: 接続情報にこのパスの持ち主として載った、自分以外の生きた tako-app の pid
/// - `wait`: 繋ぎ直す前の間（テストは時計を進めない）
/// - `legacy`: A/B（[`legacy_2360`]）。修正前の connect 1 回だけの判定
pub fn judge_occupant(
    exists: bool,
    mut connect: impl FnMut() -> bool,
    live_owner: impl FnOnce() -> Option<u32>,
    mut wait: impl FnMut(std::time::Duration),
    legacy: bool,
) -> Occupant {
    if !exists {
        return Occupant::Vacant;
    }
    if connect() {
        return Occupant::Live;
    }
    if legacy {
        return Occupant::Dead;
    }
    // 生きた持ち主が載っていれば、受け口は満杯か詰まっているだけ（繋ぎ直しで待たない）
    if let Some(pid) = live_owner() {
        return Occupant::Busy { pid };
    }
    // 記録の無い受け口（接続情報が消えた・別の置き場）でも、一時的な満杯ならこの間に空く
    for ms in LIVENESS_RETRY_DELAYS_MS {
        wait(std::time::Duration::from_millis(ms));
        if connect() {
            return Occupant::Live;
        }
    }
    Occupant::Dead
}

/// 起動時に固定パス（置き場の決め方が選んだパス）で見た先客（#2360。`Vacant` は置かない）
static FIXED_PATH: std::sync::Mutex<Option<Occupant>> = std::sync::Mutex::new(None);

/// 起動時の判定を置く。**[`status`] の受け口と同じ endpoint のときだけ**置く
/// （後発ウィンドウ・順番待ちの受け口で本体の記録を上書きしない = [`record_watch`] と同じ）
pub fn record_fixed_path(endpoint: &str, occupant: Occupant) {
    let recorded = status()
        .and_then(|s| s.endpoint)
        .is_some_and(|e| e == endpoint);
    if !recorded || occupant == Occupant::Vacant {
        return;
    }
    if let Ok(mut slot) = FIXED_PATH.lock() {
        *slot = Some(occupant);
    }
}

/// 起動時に固定パスで見た先客（確かめていない・ファイルが無かったなら `None`）
pub fn fixed_path() -> Option<Occupant> {
    FIXED_PATH.lock().ok().and_then(|s| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: usize = SUN_PATH_BSD - 1;

    fn deep(bytes: usize) -> PathBuf {
        // `/tmp/` + `d` の並びでちょうど `bytes` バイトの data dir を作る
        let head = "/tmp/";
        PathBuf::from(format!("{head}{}", "d".repeat(bytes - head.len())))
    }

    #[test]
    fn 浅いdatadirは固定パスのまま() {
        let plan = plan_with(Path::new("/tmp/tako"), Path::new("/var/tmp"), MAC);
        assert_eq!(plan.kind, SocketPathKind::WellKnown);
        assert_eq!(plan.path, PathBuf::from("/tmp/tako/tako.sock"));
        assert_eq!(plan.pointer, None);
    }

    #[test]
    fn 上限ちょうどは固定パスのまま() {
        // `<data_dir>/tako.sock` がちょうど上限 = 収まる（境界の内側）
        let data = deep(MAC - "/tako.sock".len());
        let plan = plan_with(&data, Path::new("/var/tmp"), MAC);
        assert_eq!(plan.path_bytes(), MAC);
        assert_eq!(plan.kind, SocketPathKind::WellKnown);
    }

    #[test]
    fn 上限を1バイト超えたら短縮する() {
        let data = deep(MAC - "/tako.sock".len() + 1);
        let plan = plan_with(&data, Path::new("/var/tmp"), MAC);
        assert_eq!(plan.kind, SocketPathKind::Shortened);
        assert!(plan.path.starts_with("/var/tmp"), "{:?}", plan.path);
        assert!(plan.path_bytes() <= MAC);
        assert_eq!(plan.pointer, Some(data.join(POINTER_NAME)));
        assert_eq!(plan.well_known_bytes, MAC + 1);
    }

    #[test]
    fn 深いtmpdirはフォールバックへ逃げる() {
        let data = deep(MAC);
        let temp = deep(MAC - 4); // `<temp>/tako-<16 桁>.sock` は必ず上限を超える
        let plan = plan_with(&data, &temp, MAC);
        assert_eq!(plan.kind, SocketPathKind::Shortened);
        assert!(plan.path.starts_with(FALLBACK_TEMP), "{:?}", plan.path);
        assert!(plan.path_bytes() <= MAC);
    }

    #[test]
    fn どこにも収まらなければoverflowを申告する() {
        // 上限を極端に小さくすると `/tmp/tako-<16 桁>.sock` も収まらない
        let plan = plan_with(Path::new("/tmp/x"), Path::new("/var/tmp"), 10);
        assert_eq!(plan.kind, SocketPathKind::Overflow);
        assert!(plan.path_bytes() > plan.limit);
        // 一番短い候補を返す（`/tmp/...` のほう）
        assert!(plan.path.starts_with(FALLBACK_TEMP), "{:?}", plan.path);
    }

    #[test]
    fn 同じdatadirは同じソケット名になる() {
        let a = short_name(Path::new("/tmp/tako-iso-data-42"));
        let b = short_name(Path::new("/tmp/tako-iso-data-42/"));
        assert_eq!(a, b, "末尾の区切りで別のソケットになってはいけない");
        assert_ne!(a, short_name(Path::new("/tmp/tako-iso-data-43")));
        assert_eq!(a.len(), "tako-0123456789abcdef.sock".len());
    }

    #[test]
    fn linuxの上限も検査できる() {
        // macOS で 1 バイト超過する長さでも Linux（107）なら収まる
        let data = deep(SUN_PATH_BSD - "/tako.sock".len());
        assert_eq!(
            plan_with(&data, Path::new("/var/tmp"), MAC).kind,
            SocketPathKind::Shortened
        );
        assert_eq!(
            plan_with(&data, Path::new("/var/tmp"), SUN_PATH_LINUX - 1).kind,
            SocketPathKind::WellKnown
        );
    }

    #[test]
    fn 参照ファイルは実体パスを指す() {
        let dir = std::env::temp_dir().join(format!("tako1441-ptr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = SocketPlan {
            path: PathBuf::from("/tmp/tako-abc.sock"),
            kind: SocketPathKind::Shortened,
            pointer: Some(dir.join(POINTER_NAME)),
            limit: MAC,
            well_known_bytes: 200,
        };
        write_pointer(&plan).unwrap();
        assert_eq!(
            read_pointer(&dir),
            Some(PathBuf::from("/tmp/tako-abc.sock"))
        );
        // 参照が正（bind した側が書いた値を、決め方より優先する）
        assert_eq!(
            resolve_with(&dir, Path::new("/var/tmp"), MAC),
            PathBuf::from("/tmp/tako-abc.sock")
        );
        std::fs::remove_file(dir.join(POINTER_NAME)).unwrap();
        // 参照が無ければ決め方で導く（浅いので固定パス）
        assert_eq!(
            resolve_with(&dir, Path::new("/var/tmp"), MAC),
            dir.join(WELL_KNOWN_NAME)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 記録（[`STATUS`] / [`FIXED_PATH`]）はプロセスで 1 つなので、触るテストを直列にする
    static RECORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn 記録は最初の成功を保つ() {
        let _guard = RECORD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_status_for_test();
        let failed = IpcStatus {
            endpoint: None,
            kind: SocketPathKind::Overflow,
            bound: false,
            path_bytes: 160,
            limit: MAC,
            well_known_bytes: 160,
            error: Some("path must be shorter than SUN_LEN".into()),
        };
        record(failed.clone());
        assert_eq!(status(), Some(failed.clone()));
        assert!(failed.too_long());
        let ok = IpcStatus {
            endpoint: Some("/tmp/tako-abc.sock".into()),
            kind: SocketPathKind::Shortened,
            bound: true,
            path_bytes: 18,
            limit: MAC,
            well_known_bytes: 160,
            error: None,
        };
        record(ok.clone());
        assert_eq!(status(), Some(ok.clone()), "失敗は成功で置き換わる");
        record(failed);
        assert_eq!(
            status(),
            Some(ok),
            "後発ウィンドウの一時パスで成功を上書きしない"
        );
        reset_status_for_test();
    }

    const OURS: SocketFileId = SocketFileId { dev: 1, ino: 100 };

    #[test]
    fn issue2002_同じ実体なら何もしない() {
        let check = classify(OURS, PathObservation::Socket(OURS), || {
            panic!("自分の実体なら繋いで確かめない")
        });
        assert_eq!(check, ListenerCheck::Ours);
        assert!(!check.needs_rebind());
    }

    #[test]
    fn issue2002_消えていたら張り直す() {
        let check = classify(OURS, PathObservation::Missing, || {
            panic!("無いパスへは繋がない")
        });
        assert_eq!(check, ListenerCheck::Missing);
        assert!(check.needs_rebind());
    }

    #[test]
    fn issue2002_誰も受けない別の実体なら張り直し生きていれば奪わない() {
        let other = PathObservation::Socket(SocketFileId { dev: 1, ino: 101 });
        let dead = classify(OURS, other, || false);
        assert_eq!(dead, ListenerCheck::ReplacedDead);
        assert!(dead.needs_rebind());
        let live = classify(OURS, other, || true);
        assert_eq!(live, ListenerCheck::ForeignLive);
        assert!(!live.needs_rebind(), "生きた別の受け口から奪わない");
        // dev が違えば ino が同じでも別の実体
        let other_dev = PathObservation::Socket(SocketFileId { dev: 2, ino: 100 });
        assert_eq!(
            classify(OURS, other_dev, || false),
            ListenerCheck::ReplacedDead
        );
    }

    #[test]
    fn issue2002_ソケットではないファイルには触らない() {
        let check = classify(OURS, PathObservation::Other, || panic!("繋がない"));
        assert_eq!(check, ListenerCheck::NotSocket);
        assert!(!check.needs_rebind());
    }

    #[test]
    fn issue2002_張り直しの1行はパスを載せない() {
        let mut watch = IpcWatch::new(10_000);
        watch.rebinds = 1;
        let line = watch.rebind_log_line(ListenerCheck::ReplacedDead, SocketPathKind::WellKnown);
        assert!(line.contains("張り直し 1 回目"), "{line}");
        assert!(line.contains("kind=well_known"), "{line}");
        assert!(!line.contains('/'), "パスを載せている: {line}");
        assert!(watch.healthy());
        watch.last = Some(ListenerCheck::ForeignLive);
        assert!(!watch.healthy(), "奪われたまま = 正常ではない");
    }

    /// [`judge_occupant`] を台本どおりに動かす（繋いだ結果を順に返し、回数と待った合計を数える）
    struct Script {
        answers: Vec<bool>,
        connects: usize,
        owner_asked: bool,
        waited_ms: u64,
    }

    fn judge(answers: &[bool], owner: Option<u32>, legacy: bool) -> (Occupant, Script) {
        let script = std::cell::RefCell::new(Script {
            answers: answers.to_vec(),
            connects: 0,
            owner_asked: false,
            waited_ms: 0,
        });
        let occupant = judge_occupant(
            true,
            || {
                let mut s = script.borrow_mut();
                let i = s.connects;
                s.connects += 1;
                s.answers.get(i).copied().unwrap_or(false)
            },
            || {
                script.borrow_mut().owner_asked = true;
                owner
            },
            |d| script.borrow_mut().waited_ms += d.as_millis() as u64,
            legacy,
        );
        (occupant, script.into_inner())
    }

    #[test]
    fn issue2360_ファイルが無ければ繋がない() {
        let occupant = judge_occupant(
            false,
            || panic!("無いパスへは繋がない"),
            || panic!("持ち主も見ない"),
            |_| panic!("待たない"),
            false,
        );
        assert_eq!(occupant, Occupant::Vacant);
        assert!(!occupant.holds());
        assert_eq!(occupant.startup_log_line(SocketPathKind::WellKnown), None);
    }

    #[test]
    fn issue2360_繋がれば生きた受け口() {
        let (occupant, s) = judge(&[true], None, false);
        assert_eq!(occupant, Occupant::Live);
        assert!(occupant.holds());
        assert_eq!((s.connects, s.owner_asked, s.waited_ms), (1, false, 0));
    }

    #[test]
    fn issue2360_一時的に繋がらなくても生きた持ち主が居れば奪わない() {
        // 修正前はここで残骸と読んで奪っていた（#2360 の症状）
        let (occupant, s) = judge(&[false], Some(4242), false);
        assert_eq!(occupant, Occupant::Busy { pid: 4242 });
        assert!(occupant.holds(), "満杯の生きた受け口を奪わない");
        assert_eq!(occupant.pid(), Some(4242));
        assert_eq!(
            (s.connects, s.waited_ms),
            (1, 0),
            "持ち主が生きていれば繋ぎ直しで待たない"
        );
        let line = occupant
            .startup_log_line(SocketPathKind::WellKnown)
            .unwrap();
        assert!(
            line.contains("奪わず") && line.contains("pid 4242"),
            "{line}"
        );
        assert!(!line.contains('/'), "パスを載せている: {line}");
    }

    #[test]
    fn issue2360_記録の無い受け口も繋ぎ直しの間に空けば生きている() {
        // 1 回目は満杯で refused、2 回目（50 ms 後）に繋がる = 一斉接続の山が引いた
        let (occupant, s) = judge(&[false, true], None, false);
        assert_eq!(occupant, Occupant::Live);
        assert!(s.owner_asked, "繋ぎ直す前に持ち主を見る");
        assert_eq!((s.connects, s.waited_ms), (2, LIVENESS_RETRY_DELAYS_MS[0]));
        // 最後の 1 回で繋がっても生きている
        let n = LIVENESS_RETRY_DELAYS_MS.len();
        let mut answers = vec![false; n];
        answers.push(true);
        let (occupant, s) = judge(&answers, None, false);
        assert_eq!(occupant, Occupant::Live);
        assert_eq!(s.connects, n + 1);
    }

    #[test]
    fn issue2360_繋ぎ直しても繋がらず持ち主も居なければ残骸() {
        let (occupant, s) = judge(&[], None, false);
        assert_eq!(occupant, Occupant::Dead);
        assert!(!occupant.holds());
        assert_eq!(s.connects, LIVENESS_RETRY_DELAYS_MS.len() + 1);
        assert_eq!(s.waited_ms, LIVENESS_RETRY_DELAYS_MS.iter().sum::<u64>());
        assert_eq!(s.waited_ms, 350, "残骸のときに起動が遅れる上限");
        let line = occupant
            .startup_log_line(SocketPathKind::WellKnown)
            .unwrap();
        assert!(
            line.contains("残骸") && line.contains("4 回繋いでも"),
            "{line}"
        );
    }

    #[test]
    fn issue2360_旧挙動はconnect1回で残骸と読む() {
        // A/B（TAKO_2360_LEGACY=1）: 持ち主を見ず、繋ぎ直さない = 修正前の判定
        let (occupant, s) = judge(&[false, true], Some(4242), true);
        assert_eq!(occupant, Occupant::Dead);
        assert_eq!((s.connects, s.owner_asked, s.waited_ms), (1, false, 0));
        let (occupant, _) = judge(&[true], None, true);
        assert_eq!(occupant, Occupant::Live, "繋がれば旧挙動でも奪わない");
    }

    #[test]
    fn issue2360_起動時の判定は本体の受け口の分だけ置く() {
        let _guard = RECORD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_status_for_test();
        if let Ok(mut slot) = FIXED_PATH.lock() {
            *slot = None;
        }
        record(IpcStatus {
            endpoint: Some("/tmp/tako-2360.sock".into()),
            kind: SocketPathKind::Temp,
            bound: true,
            path_bytes: 19,
            limit: MAC,
            well_known_bytes: 0,
            error: None,
        });
        record_fixed_path("/tmp/other.sock", Occupant::Live);
        assert_eq!(fixed_path(), None, "別の受け口の判定で上書きしない");
        record_fixed_path("/tmp/tako-2360.sock", Occupant::Vacant);
        assert_eq!(fixed_path(), None, "ファイルが無かったことは置かない");
        record_fixed_path("/tmp/tako-2360.sock", Occupant::Busy { pid: 7 });
        assert_eq!(fixed_path(), Some(Occupant::Busy { pid: 7 }));
        reset_status_for_test();
        if let Ok(mut slot) = FIXED_PATH.lock() {
            *slot = None;
        }
    }
}

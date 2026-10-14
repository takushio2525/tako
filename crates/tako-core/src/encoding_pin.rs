//! ペインの文字コード固定の決着と、決着までの入力の保留（Issue #2092）
//!
//! ## 何が起きていたか
//!
//! Windows のペインは疑似コンソールのコードページを UTF-8（65001）へ固定してから使う
//! （境界 B19 = [`crate::platform::console`]。#655 / #659）。固定は子が疑似コンソールへ
//! 接続し終えるのを別スレッドで待ってから当てるので、**シェルと同期していない**。
//! Windows CI の実測（#1696 / PR #2079。20 回）では、固定はプロンプトが画面に出る
//! 7.1〜33.6 ms 前に済んでいた。負荷でこの余裕が逆転すると、固定の前に子が
//! バイト列で書いた出力（`type`・CRT の printf 等）は**後から直せない形で化ける**。
//! 人の打鍵より速い tako 自身の最初の入力（起動コマンド・Code Runner の実行行）が踏みうる。
//! `WriteConsoleW` で書く子（node / Rust / Python）は影響を受けない。
//!
//! ## 何をするか
//!
//! 1. **入力を固定の決着まで保留する**（[`PinGate`]）。入力が届かなければ子はそれに応じた
//!    出力を書かない。決着 = 固定できた / 諦めた / tako 自身がコンソールを持っているので
//!    触らない、のどれか。上限（[`DIRECT_HOLD_LIMIT`] / [`CONTAINER_HOLD_LIMIT`]）を過ぎたら
//!    決着を待たずに流す（固定が詰まっても入力が止まり続けない）
//! 2. **固定の結果を読めるようにする**（[`PinGate::report`] → `tako check-health` の
//!    `encoding_pin` 節）
//! 3. **諦めたこと・上限で流したことを記録する**（[`PinLogEvent`] をペインのイベントで
//!    UI 層へ渡し、UI 層が persist.log へ書く。tako-core はペイン ID を知らないため）
//!
//! ## 保留しないもの
//!
//! - 端末からの応答（DSR などへの返事 = `PtyWrite`）。疑似コンソールや子がその返事を
//!   待っていると子の起動が進まず、固定も決着しない（保留が上限まで伸びるだけで得が無い）
//! - 入力に依らない出力（シェルの起動バナー・最初のプロンプト）は保留では守れない。
//!   cmd.exe / PowerShell は自分の文字を `WriteConsoleW` で書くので実害は無い
//!
//! ## 器（psmux）のペイン
//!
//! 器のペインでは tako の ConPTY 直下の子は psmux クライアントで、シェルは psmux
//! サーバーが作った別の疑似コンソールの中にいる。固定は 2 つ要る（クライアント = 直接の
//! 固定 / 器の中のシェル = [`ContainerPin`]）ので、門は**両方が決着するまで**閉じている。
//! 器の中の pid は器へ問い合わせないと分からない（120 ms 間隔のサブプロセス）ので、
//! 器のペインの上限は直接ペインより長い。
//!
//! unix は固定自体が要らない（`LC_CTYPE` の注入で担保）ので、門も作らない
//! （[`crate::platform::console::pin_needed`]）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::backend::{SessionBackend, SessionRef};
use crate::platform::console::{self, PinOutcome};

/// 直接ペインの保留の上限（**ペインの起動から**数える）。
///
/// 固定は普段すぐ決着する（Windows CI の実測 25 回 = 起動後 5〜16 ms・cmd.exe のプロンプトは
/// 26〜46 ms。実機では子の接続の成立が cmd.exe で 43〜61 ms・PowerShell 5.1 で 97 ms =
/// `platform::console`）。遅い実機の値の 5 倍を取って負荷のある機でも決着の前に解けない
/// ようにし、かつ人が新しいペインへ打ち始める（ペインが出てから反応して打つまで）より短くする。
/// 起動から数えるので、人の打鍵が実際に待たされるのは固定が詰まった回だけで、
/// 最大でも「上限 − 打ち始めまでの時間」
pub const DIRECT_HOLD_LIMIT: Duration = Duration::from_millis(500);

/// 器（psmux）のペインの保留の上限（ペインの起動から数える）。
///
/// 器のセッションができて中のシェルの pid が見えるまで待ってから固定する（問い合わせは
/// [`CONTAINER_PID_INTERVAL`] 間隔・1 回 30〜50 ms のサブプロセス）ぶん直接ペインより長い。
/// Windows CI の実測（psmux 3.4.0・25 回）は器の内側の固定が起動後 154〜180 ms、器の中の
/// プロンプトが tako の画面に出るのが 226〜310 ms。実機は器のセッションができるまで
/// 1 秒未満（psmux 3.3.7）なので、それに pid 探しの 1 間隔を足しても収まる値にした。
/// 器の中のシェルは器ができるまで入力を読めない（早すぎる打鍵は器に食われる = 実測）ので、
/// この間の保留で人が待たされる時間は「器の中のシェルの起動」とほぼ重なる
pub const CONTAINER_HOLD_LIMIT: Duration = Duration::from_millis(1500);

/// 器の中のペインの pid が見えるようになるまでの上限。
/// `new-session` の完了待ちで、実測（Windows 実機・psmux 3.3.7）は 1 秒未満
pub const CONTAINER_PID_TIMEOUT: Duration = Duration::from_secs(15);

/// 器への問い合わせ間隔。1 回がサブプロセス起動（実測 30〜50ms）なので細かくしない
pub const CONTAINER_PID_INTERVAL: Duration = Duration::from_millis(120);

/// 修正前（#2092 前）の形へ戻す A/B（`TAKO_2092_LEGACY=1`）。
///
/// 戻すのは「門を作らない」だけで、固定そのものは #2092 前と同じく非同期に走る
/// （保留しない・結果を記録しない・諦めを persist.log に出さない）。
/// **ペインの起動ごとに読む**（同じテストプロセスの中で新旧を並べて測るため。
/// 起動 1 回に env を 1 回読むだけなので費用は無い）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_2092_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 固定の対象
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinTarget {
    /// tako の ConPTY 直下の子（直接ペインのシェル / 器のペインの psmux クライアント）
    Direct,
    /// 器（psmux）の中のシェル
    Container,
}

impl PinTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Container => "container",
        }
    }
}

/// 1 対象の固定の状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinState {
    /// 試行中
    Pending,
    /// UTF-8 に固定した
    Pinned,
    /// tako 自身がコンソールを持っているので触らなかった（debug ビルド = コンソール
    /// サブシステムで起きる。出荷構成の GUI サブシステムでは起きない）
    Skipped,
    /// 諦めた（理由つき。OS のエラーコード等で、ペインの内容は含めない）
    GaveUp(String),
}

impl PinState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Pinned => "pinned",
            Self::Skipped => "skipped",
            Self::GaveUp(_) => "gave_up",
        }
    }

    /// 決着したか（試行中でない）
    pub fn settled(&self) -> bool {
        !matches!(self, Self::Pending)
    }

    /// 諦めた理由（諦めていなければ None）
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::GaveUp(reason) => Some(reason),
            _ => None,
        }
    }

    fn from_outcome(outcome: PinOutcome) -> Self {
        match outcome {
            PinOutcome::Pinned => Self::Pinned,
            PinOutcome::Failed(reason) => Self::GaveUp(reason),
            PinOutcome::SkippedHasOwnConsole | PinOutcome::NotNeeded => Self::Skipped,
        }
    }
}

/// 1 対象の記録
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetReport {
    pub target: PinTarget,
    /// 固定を当てた（当てようとした）プロセス。器の中の pid は見つかるまで None
    pub pid: Option<u32>,
    pub state: PinState,
    /// ペインの起動から決着までの ms（試行中は None）
    pub settled_ms: Option<u64>,
}

/// 保留の解け方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// すべての対象が決着した
    Settled,
    /// 上限を過ぎた（決着を待たずに流した）
    Limit,
    /// 保留中にペインが閉じられた（保留していた入力は捨てた）
    Closed,
}

impl Release {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Limit => "limit",
            Self::Closed => "closed",
        }
    }
}

/// 保留の記録
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldReport {
    /// 保留した書き込みの数（0 = 決着の前に入力が来なかった）
    pub held_writes: usize,
    /// 保留したバイト数（**中身は持たない**）
    pub held_bytes: usize,
    /// 解け方（まだ保留中なら None）
    pub release: Option<Release>,
    /// 最初の保留から解けるまでの ms（保留していなければ None・保留中なら今までの値）
    pub held_ms: Option<u64>,
    /// 上限（ペインの起動から数える ms）
    pub limit_ms: u64,
}

/// ペイン 1 枚の固定の記録（診断用。`tako check-health` の `encoding_pin` 節）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinReport {
    pub targets: Vec<TargetReport>,
    pub hold: HoldReport,
}

impl PinReport {
    /// ペイン全体の状態。諦めた対象が 1 つでもあれば `gave_up`、試行中があれば `pending`、
    /// すべて触らなかったなら `skipped`、それ以外は `pinned`
    pub fn state(&self) -> &'static str {
        let states = || self.targets.iter().map(|t| &t.state);
        if states().any(|s| matches!(s, PinState::GaveUp(_))) {
            "gave_up"
        } else if states().any(|s| matches!(s, PinState::Pending)) {
            "pending"
        } else if !self.targets.is_empty() && states().all(|s| matches!(s, PinState::Skipped)) {
            "skipped"
        } else {
            "pinned"
        }
    }

    /// 諦めた対象
    pub fn gave_up(&self) -> impl Iterator<Item = &TargetReport> {
        self.targets
            .iter()
            .filter(|t| matches!(t.state, PinState::GaveUp(_)))
    }
}

/// persist.log へ残す出来事（どちらも異常系でしか起きない = 高頻度にならない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinLogEvent {
    /// 固定を諦めた
    GaveUp {
        target: PinTarget,
        pid: Option<u32>,
        elapsed_ms: u64,
        reason: String,
    },
    /// 固定の決着より先に保留の上限が来たので流した
    HoldLimit {
        limit_ms: u64,
        held_writes: usize,
        pending: Vec<PinTarget>,
    },
}

impl PinLogEvent {
    /// persist.log の 1 行。**ペインの内容・送った文字は含めない**（AGENTS.md の絶対ルール）
    pub fn log_line(&self, pane: u64) -> String {
        match self {
            Self::GaveUp {
                target,
                pid,
                elapsed_ms,
                reason,
            } => format!(
                "ペインの文字コードを UTF-8 に固定できず諦めた: pane={pane} 対象={} pid={} \
                 経過={elapsed_ms}ms 理由={reason}（このペインでは UTF-8 のバイト列で書く子の\
                 出力が化ける。直すにはそのペインで chcp 65001。#2092）",
                target.as_str(),
                pid.map_or_else(|| "?".to_string(), |p| p.to_string()),
            ),
            Self::HoldLimit {
                limit_ms,
                held_writes,
                pending,
            } => format!(
                "ペインへの入力の保留が上限 {limit_ms}ms で解けた: pane={pane} 未決着={} \
                 流した書き込み={held_writes} 件（固定より先に届いた入力の出力は化けうる。#2092）",
                pending
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        }
    }
}

type Flush = Box<dyn Fn(Vec<u8>) + Send + Sync>;
type Notify = Box<dyn Fn(PinLogEvent) + Send + Sync>;

/// 固定が決着するまでペインへの入力を保留する門（#2092）。
///
/// 入力は [`Self::submit`] を通す。開いていればそのまま返る（呼び出し側が PTY へ送る）、
/// 閉じていれば門が預かり、開いたときに**預かった順に** flush 先へ流す。
/// 開くのは 1 回だけで、開いた後に閉じ直すことはない（入力の順序を壊さないため）
#[derive(Clone)]
pub struct PinGate(Arc<Shared>);

struct Shared {
    born: Instant,
    limit: Duration,
    /// 開いた後の速い道（毎打鍵でロックを取らない）。立てるのは預かりを流し終えた後
    open: AtomicBool,
    state: Mutex<State>,
    wake: Condvar,
    notify: Notify,
}

#[derive(Default)]
struct State {
    targets: Vec<TargetReport>,
    held: Vec<Vec<u8>>,
    held_writes: usize,
    held_bytes: usize,
    first_held: Option<Instant>,
    release: Option<Release>,
    released_at: Option<Instant>,
    flush: Option<Flush>,
    watching: bool,
    /// 見張りが上限まで眠った回数（テストが実時間ではなく機構で「上限をどこから数えたか」を
    /// 観るための口。`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
    watch_waits: u32,
}

impl PinGate {
    /// 閉じた門を作る。`notify` は persist.log へ残す出来事の受け口（別スレッドから呼ばれる）。
    /// **入力が来る前に** [`Self::register`] で対象を登録し、[`Self::attach_flush`] で
    /// 流し先を渡すこと
    pub fn new(limit: Duration, notify: impl Fn(PinLogEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(Shared {
            born: Instant::now(),
            limit,
            open: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            notify: Box::new(notify),
        }))
    }

    /// 決着を待つ対象を足す。返る札で決着を報告する（落とせば「諦めた」扱い）
    pub fn register(&self, target: PinTarget, pid: Option<u32>) -> PinTicket {
        let mut st = self.lock();
        st.targets.push(TargetReport {
            target,
            pid,
            state: PinState::Pending,
            settled_ms: None,
        });
        PinTicket {
            gate: self.clone(),
            index: st.targets.len() - 1,
            settled: false,
        }
    }

    /// 開いたときに預かった入力を流す先（PTY の書き込みキュー）
    pub fn attach_flush(&self, flush: impl Fn(Vec<u8>) + Send + Sync + 'static) {
        let mut st = self.lock();
        if st.release.is_none() {
            st.flush = Some(Box::new(flush));
        }
    }

    /// 入力を門へ通す。開いていれば `Some`（呼び出し側がそのまま送る）、
    /// 預かったら `None`。閉じた（ペインが閉じられた）後の入力は捨てて `None`
    pub fn submit(&self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if self.0.open.load(Ordering::Acquire) {
            return Some(bytes);
        }
        let mut st = self.lock();
        match st.release {
            Some(Release::Closed) => return None,
            Some(_) => return Some(bytes),
            None => {}
        }
        st.held_writes += 1;
        st.held_bytes += bytes.len();
        st.first_held.get_or_insert_with(Instant::now);
        st.held.push(bytes);
        if !st.watching {
            st.watching = true;
            drop(st);
            self.spawn_watcher();
        }
        None
    }

    /// ペインが閉じられた。預かっていた入力は捨てる（流す先がもう無い）
    pub fn close(&self) {
        let mut st = self.lock();
        if st.release.is_none() {
            st.held.clear();
            st.release = Some(Release::Closed);
            st.released_at = Some(Instant::now());
        }
        st.flush = None;
        self.0.wake.notify_all();
    }

    /// 診断用の記録
    pub fn report(&self) -> PinReport {
        let st = self.lock();
        let held_ms = st.first_held.map(|first| {
            let end = st.released_at.unwrap_or_else(Instant::now);
            millis(end.saturating_duration_since(first))
        });
        PinReport {
            targets: st.targets.clone(),
            hold: HoldReport {
                held_writes: st.held_writes,
                held_bytes: st.held_bytes,
                release: st.release,
                held_ms,
                limit_ms: millis(self.0.limit),
            },
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 預かりを流して開く（ロックの内側で呼ぶ）。**流し終えてから**速い道を立てるので、
    /// 速い道を通った入力が預かりを追い越すことはない
    fn open_locked(&self, st: &mut State, release: Release) {
        let held = std::mem::take(&mut st.held);
        match &st.flush {
            Some(flush) => held.into_iter().for_each(flush),
            None if !held.is_empty() => {
                tracing::debug!("流し先が無いまま保留を解いた（{} 件を捨てた）", held.len());
            }
            None => {}
        }
        st.release = Some(release);
        st.released_at = Some(Instant::now());
        st.flush = None;
        self.0.open.store(true, Ordering::Release);
        self.0.wake.notify_all();
    }

    fn settle(&self, index: usize, state: PinState) {
        let mut st = self.lock();
        let elapsed = millis(self.0.born.elapsed());
        let Some(record) = st.targets.get_mut(index) else {
            return;
        };
        if record.state.settled() {
            return;
        }
        record.state = state.clone();
        record.settled_ms = Some(elapsed);
        let event = match &state {
            PinState::GaveUp(reason) => Some(PinLogEvent::GaveUp {
                target: record.target,
                pid: record.pid,
                elapsed_ms: elapsed,
                reason: reason.clone(),
            }),
            _ => None,
        };
        let closed = st.release == Some(Release::Closed);
        if st.release.is_none() && st.targets.iter().all(|t| t.state.settled()) {
            self.open_locked(&mut st, Release::Settled);
        }
        drop(st);
        // 閉じたペインの諦めは記録しない（もう誰も困らない）
        if let Some(event) = event.filter(|_| !closed) {
            (self.0.notify)(event);
        }
    }

    fn set_pid(&self, index: usize, pid: u32) {
        if let Some(record) = self.lock().targets.get_mut(index) {
            record.pid = Some(pid);
        }
    }

    /// 上限で開ける見張り。**入力を預かったときだけ**立てる（決着が先なら立たない）
    fn spawn_watcher(&self) {
        let gate = self.clone();
        let spawned = std::thread::Builder::new()
            .name("tako-pin-hold".into())
            .spawn(move || gate.watch());
        if spawned.is_err() {
            // 見張りが立たないなら保留しない（入力を止め続けるよりは化けうる方がよい）
            let mut st = self.lock();
            if st.release.is_none() {
                self.open_locked(&mut st, Release::Limit);
            }
        }
    }

    fn watch(&self) {
        let deadline = self.0.born + self.0.limit;
        let mut st = self.lock();
        loop {
            if st.release.is_some() {
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                let pending = st
                    .targets
                    .iter()
                    .filter(|t| !t.state.settled())
                    .map(|t| t.target)
                    .collect();
                let held_writes = st.held_writes;
                self.open_locked(&mut st, Release::Limit);
                drop(st);
                (self.0.notify)(PinLogEvent::HoldLimit {
                    limit_ms: millis(self.0.limit),
                    held_writes,
                    pending,
                });
                return;
            }
            st.watch_waits += 1;
            st = self
                .0
                .wake
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// 見張りが上限まで眠った回数（テスト用の観測口）
    #[cfg(test)]
    fn watch_waits(&self) -> u32 {
        self.lock().watch_waits
    }
}

/// 決着の報告に使う札（1 対象に 1 枚）。報告せずに落とすと「諦めた」扱いになる
/// （固定のスレッドが panic しても門が上限まで閉じたままにならないように）
pub struct PinTicket {
    gate: PinGate,
    index: usize,
    settled: bool,
}

impl PinTicket {
    /// 固定を当てるプロセスが分かった（器の中の pid は後から見つかる）
    pub fn set_pid(&self, pid: u32) {
        self.gate.set_pid(self.index, pid);
    }

    /// 決着を報告する
    pub fn settle(mut self, state: PinState) {
        self.settled = true;
        self.gate.settle(self.index, state);
    }
}

impl Drop for PinTicket {
    fn drop(&mut self) {
        if !self.settled {
            self.gate.settle(
                self.index,
                PinState::GaveUp("固定の試行が決着を報告せずに終わった".into()),
            );
        }
    }
}

/// 器の内側の固定に要る材料（#659 / #2092）。器を配ったペインだけが持つ
#[derive(Clone)]
pub struct ContainerPin {
    backend: &'static dyn SessionBackend,
    session: SessionRef,
}

impl std::fmt::Debug for ContainerPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerPin")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl ContainerPin {
    pub fn new(backend: &'static dyn SessionBackend, session: SessionRef) -> Self {
        Self { backend, session }
    }

    /// 本番の器（[`crate::backend::backend`]）のセッション名から作る。
    /// 固定が要らない（unix）・名前が器の名前として使えないときは None
    pub fn for_session(name: &str) -> Option<Self> {
        if !console::pin_needed() {
            return None;
        }
        let session = SessionRef::new(name).ok()?;
        Some(Self::new(crate::backend::backend(), session))
    }
}

/// ペインの固定を始める（`TerminalSession::spawn_in` の 1 箇所から呼ぶ）。
///
/// 返る門は入力の保留に使う。固定が要らない（unix）・固定する相手がいないときは None
/// （= 門なし = 素通し）。A/B の legacy では門を作らず、固定だけを #2092 前と同じく
/// 非同期に走らせる。**対象をすべて登録してからスレッドを起こす**（先に起こした固定が
/// 後の登録より先に決着すると、門が片方だけを見て開いてしまう）
pub(crate) fn begin(
    child_pid: Option<u32>,
    container: Option<ContainerPin>,
    notify: impl Fn(PinLogEvent) + Send + Sync + 'static,
) -> Option<PinGate> {
    if !console::pin_needed() || (child_pid.is_none() && container.is_none()) {
        return None;
    }
    let limit = if container.is_some() {
        CONTAINER_HOLD_LIMIT
    } else {
        DIRECT_HOLD_LIMIT
    };
    let gate = (!legacy()).then(|| PinGate::new(limit, notify));
    let direct = child_pid.map(|pid| {
        let ticket = gate
            .as_ref()
            .map(|g| g.register(PinTarget::Direct, Some(pid)));
        (pid, ticket)
    });
    let inner = container.map(|pin| {
        let ticket = gate
            .as_ref()
            .map(|g| g.register(PinTarget::Container, None));
        (pin, ticket)
    });
    if let Some((pid, ticket)) = direct {
        start_direct(pid, ticket);
    }
    if let Some((pin, ticket)) = inner {
        start_container(pin, ticket);
    }
    gate
}

/// 器の内側の固定だけを門なしで走らせる（`backend::wrap_spawn_for_pane` 用。
/// あちらは端末を起こす前に呼ばれるので保留の門を持てない）
pub(crate) fn start_container_ungated(pin: ContainerPin) {
    if console::pin_needed() {
        start_container(pin, None);
    }
}

/// 固定を遅らせる・諦めさせる注入（**検証用**。#2092）。
///
/// - `TAKO_2092_INJECT_PIN_DELAY_MS=<n>`: 固定の試行を n ms 遅らせる（直接は起動直後、
///   器は中の pid が見つかった後）
/// - `TAKO_2092_INJECT_PIN_GIVE_UP=1`: 試さずに諦める（遅らせた後）
/// - `TAKO_2092_INJECT_PIN_TARGET=direct|container`: 注入する対象（未指定は両方）
///
/// 新旧（`TAKO_2092_LEGACY=1`）のどちらにも同じように効く = 同じ遅れで化けるかを並べられる
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Injection {
    delay: Option<Duration>,
    give_up: bool,
}

/// 注入で諦めさせたときの理由
pub const INJECTED_GIVE_UP_REASON: &str = "注入（TAKO_2092_INJECT_PIN_GIVE_UP）";

fn injection(target: PinTarget) -> Injection {
    injection_from(target, |key| std::env::var(key).ok())
}

fn injection_from(target: PinTarget, var: impl Fn(&str) -> Option<String>) -> Injection {
    let applies = match var("TAKO_2092_INJECT_PIN_TARGET").as_deref().map(str::trim) {
        None | Some("") => true,
        Some(only) => only.eq_ignore_ascii_case(target.as_str()),
    };
    if !applies {
        return Injection::default();
    }
    Injection {
        delay: var("TAKO_2092_INJECT_PIN_DELAY_MS")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis),
        give_up: matches!(
            var("TAKO_2092_INJECT_PIN_GIVE_UP").as_deref(),
            Some("1" | "true" | "on")
        ),
    }
}

/// 注入を当ててから固定する（呼び出し元のスレッドで待つ）
fn pin_with_injection(pid: u32, injection: &Injection) -> PinState {
    if let Some(delay) = injection.delay {
        std::thread::sleep(delay);
    }
    if injection.give_up {
        return PinState::GaveUp(INJECTED_GIVE_UP_REASON.into());
    }
    PinState::from_outcome(console::pin_pane_to_utf8_blocking(pid))
}

/// 決着を門へ報告する（門なし = legacy は #2092 前と同じく debug ログだけ）
fn report(ticket: Option<PinTicket>, state: PinState) {
    match ticket {
        Some(ticket) => ticket.settle(state),
        None => {
            if let Some(reason) = state.reason() {
                tracing::debug!("ペインのコードページを UTF-8 に固定できず: {reason}");
            }
        }
    }
}

fn start_direct(pid: u32, ticket: Option<PinTicket>) {
    let injection = injection(PinTarget::Direct);
    // UI スレッドを止めないために別スレッドへ逃がす（スレッドが作れなければ札が落ちて
    // 「諦めた」で決着する = 門は閉じたままにならない）
    let _ = std::thread::Builder::new()
        .name("tako-pin-console-cp".into())
        .spawn(move || report(ticket, pin_with_injection(pid, &injection)));
}

/// 器の中のシェルの疑似コンソールを UTF-8 へ固定する（#659）。
///
/// `backend=psmux` のとき、tako の ConPTY 直下の子は **psmux クライアント**であって
/// シェルではない。シェルは psmux サーバーが自前に作った別の疑似コンソールの中で動くので、
/// 直接の固定は器の内側には届かない（#655 の対処が psmux ペインに効かなかった理由そのもの）。
/// 器へ pid を尋ねるのはサブプロセス起動なので、待ちも問い合わせも別スレッドで行う
fn start_container(pin: ContainerPin, ticket: Option<PinTicket>) {
    let injection = injection(PinTarget::Container);
    let _ = std::thread::Builder::new()
        .name("tako-pin-container-cp".into())
        .spawn(move || {
            let deadline = Instant::now() + CONTAINER_PID_TIMEOUT;
            loop {
                // 新規作成なら器のセッションができるまで空。再 attach（復元）なら即座に返る
                // （既に走っているシェルでも固定は効く = 実測。#659）
                let pids = pin.backend.pane_pids(&pin.session);
                if let Some(first) = pids.first() {
                    if let Some(ticket) = &ticket {
                        ticket.set_pid(*first);
                    }
                    // 器のセッションは tako では 1 ペインだが、複数あれば全部へ当てる。
                    // 1 つでも諦めたらその理由で決着する
                    let mut state = PinState::Pinned;
                    for (i, pid) in pids.iter().enumerate() {
                        let injected = if i == 0 {
                            injection.clone()
                        } else {
                            Injection::default()
                        };
                        let got = pin_with_injection(*pid, &injected);
                        if !matches!(state, PinState::GaveUp(_)) && got != PinState::Pinned {
                            state = got;
                        }
                    }
                    report(ticket, state);
                    return;
                }
                if Instant::now() >= deadline {
                    report(
                        ticket,
                        PinState::GaveUp(format!(
                            "器の中のペインの pid を {} 秒以内に取得できず（session={}）",
                            CONTAINER_PID_TIMEOUT.as_secs(),
                            pin.session
                        )),
                    );
                    return;
                }
                std::thread::sleep(CONTAINER_PID_INTERVAL);
            }
        });
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 流し先と通知を記録する偽物
    #[derive(Clone, Default)]
    struct Sink {
        flushed: Arc<Mutex<Vec<Vec<u8>>>>,
        events: Arc<Mutex<Vec<PinLogEvent>>>,
    }

    impl Sink {
        fn gate(&self, limit: Duration) -> PinGate {
            let events = self.events.clone();
            let gate = PinGate::new(limit, move |e| events.lock().unwrap().push(e));
            let flushed = self.flushed.clone();
            gate.attach_flush(move |b| flushed.lock().unwrap().push(b));
            gate
        }

        fn flushed(&self) -> Vec<Vec<u8>> {
            self.flushed.lock().unwrap().clone()
        }

        fn events(&self) -> Vec<PinLogEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    const LONG: Duration = Duration::from_secs(60);

    fn wait_until(mut ok: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ok()
    }

    #[test]
    fn すべての対象が決着するまで預かり決着で預かった順に流す() {
        let sink = Sink::default();
        let gate = sink.gate(LONG);
        let direct = gate.register(PinTarget::Direct, Some(10));
        let inner = gate.register(PinTarget::Container, None);

        assert_eq!(gate.submit(b"a".to_vec()), None);
        assert_eq!(gate.submit(b"b".to_vec()), None);
        direct.settle(PinState::Pinned);
        // 器の中がまだ = 開かない
        assert_eq!(gate.submit(b"c".to_vec()), None);
        assert!(sink.flushed().is_empty());
        inner.set_pid(20);
        inner.settle(PinState::Pinned);

        assert_eq!(
            sink.flushed(),
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
        );
        // 開いた後は素通し（呼び出し側が送る）
        assert_eq!(gate.submit(b"d".to_vec()), Some(b"d".to_vec()));
        let report = gate.report();
        assert_eq!(report.state(), "pinned");
        assert_eq!(report.hold.release, Some(Release::Settled));
        assert_eq!(report.hold.held_writes, 3);
        assert_eq!(report.hold.held_bytes, 3);
        assert_eq!(report.targets[1].pid, Some(20));
        assert!(report.targets.iter().all(|t| t.settled_ms.is_some()));
        assert!(sink.events().is_empty(), "正常系は記録しない");
    }

    #[test]
    fn 決着が入力より先なら何も預からない() {
        let sink = Sink::default();
        let gate = sink.gate(LONG);
        gate.register(PinTarget::Direct, Some(1))
            .settle(PinState::Pinned);
        assert_eq!(gate.submit(b"x".to_vec()), Some(b"x".to_vec()));
        let report = gate.report();
        assert_eq!(report.hold.held_writes, 0);
        assert_eq!(report.hold.held_ms, None);
        assert_eq!(report.hold.release, Some(Release::Settled));
    }

    #[test]
    fn 決着しないまま上限が来たら流して記録する() {
        // 上限の前は勝手に解けない（見張りは上限まで眠っている）。上限が十分遠い門で見る
        let far = Sink::default();
        let gate = far.gate(LONG);
        let _ticket = gate.register(PinTarget::Direct, Some(1));
        assert_eq!(gate.submit(b"early".to_vec()), None);
        assert!(wait_until(|| gate.watch_waits() >= 1), "見張りが眠らない");
        assert!(far.flushed().is_empty(), "上限の前に流れた");
        assert_eq!(gate.report().hold.release, None);
        gate.close();

        // 上限が来たら見張りが流し、出来事を通知する
        let sink = Sink::default();
        let gate = sink.gate(Duration::from_millis(80));
        let ticket = gate.register(PinTarget::Direct, Some(1));
        assert_eq!(gate.submit(b"late".to_vec()), None);
        assert!(wait_until(|| !sink.flushed().is_empty()), "上限で流れない");
        assert_eq!(sink.flushed(), vec![b"late".to_vec()]);
        let report = gate.report();
        assert_eq!(report.hold.release, Some(Release::Limit));
        assert_eq!(report.state(), "pending");
        assert_eq!(
            sink.events(),
            vec![PinLogEvent::HoldLimit {
                limit_ms: 80,
                held_writes: 1,
                pending: vec![PinTarget::Direct],
            }]
        );

        // 上限の後で決着しても二度は流さない・記録は更新される
        ticket.settle(PinState::Pinned);
        assert_eq!(sink.flushed().len(), 1);
        assert_eq!(gate.report().state(), "pinned");
        assert_eq!(gate.report().hold.release, Some(Release::Limit));
    }

    #[test]
    fn 上限は起動から数える() {
        let sink = Sink::default();
        let gate = sink.gate(Duration::from_millis(30));
        let _ticket = gate.register(PinTarget::Direct, Some(1));
        // 起動から上限を過ぎてから最初の入力を預ける
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(gate.submit(b"x".to_vec()), None);
        assert!(wait_until(|| !sink.flushed().is_empty()), "上限で流れない");
        // 見張りは一度も眠らずに流した = 上限を「預けた時刻」ではなく起動から数えている
        // （預けた時刻から数えるなら 30 ms 眠ってから流す）
        assert_eq!(gate.watch_waits(), 0);
        assert_eq!(gate.report().hold.release, Some(Release::Limit));
    }

    #[test]
    fn 諦めも決着として開き記録を出す() {
        let sink = Sink::default();
        let gate = sink.gate(LONG);
        let ticket = gate.register(PinTarget::Container, None);
        ticket.set_pid(4242);
        assert_eq!(gate.submit(b"x".to_vec()), None);
        ticket.settle(PinState::GaveUp("AttachConsole err=31".into()));

        assert_eq!(sink.flushed(), vec![b"x".to_vec()]);
        let report = gate.report();
        assert_eq!(report.state(), "gave_up");
        assert_eq!(report.gave_up().count(), 1);
        let events = sink.events();
        assert_eq!(events.len(), 1);
        let PinLogEvent::GaveUp {
            target,
            pid,
            reason,
            ..
        } = &events[0]
        else {
            panic!("諦めの記録ではない: {events:?}");
        };
        assert_eq!(*target, PinTarget::Container);
        assert_eq!(*pid, Some(4242));
        assert_eq!(reason, "AttachConsole err=31");
    }

    #[test]
    fn 札を報告せずに落としたら諦めとして開く() {
        let sink = Sink::default();
        let gate = sink.gate(LONG);
        let ticket = gate.register(PinTarget::Direct, Some(1));
        assert_eq!(gate.submit(b"x".to_vec()), None);
        drop(ticket);
        assert_eq!(sink.flushed(), vec![b"x".to_vec()]);
        assert_eq!(gate.report().state(), "gave_up");
    }

    #[test]
    fn 保留中に閉じたら預かりを捨て以後も流さず諦めも記録しない() {
        let sink = Sink::default();
        let gate = sink.gate(Duration::from_millis(50));
        let ticket = gate.register(PinTarget::Direct, Some(1));
        assert_eq!(gate.submit(b"secret".to_vec()), None);
        gate.close();
        assert_eq!(gate.submit(b"after".to_vec()), None);
        ticket.settle(PinState::GaveUp("遅れて諦めた".into()));
        // 上限を過ぎても見張りが流さない
        std::thread::sleep(Duration::from_millis(120));

        assert!(sink.flushed().is_empty(), "閉じたペインへ流した");
        assert!(sink.events().is_empty(), "閉じたペインの出来事を記録した");
        let report = gate.report();
        assert_eq!(report.hold.release, Some(Release::Closed));
        assert_eq!(report.hold.held_writes, 1);
    }

    #[test]
    fn 開く瞬間に並行して来た入力も順序を保ち1度ずつ届く() {
        // 送り手（UI スレッド相当）が 0..N を順に送るあいだに別スレッドが決着させる。
        // 門が返した Some を送り手が同じ流し先へ積めば、全体が昇順で欠けも重複も無い
        for _ in 0..20 {
            let sink = Sink::default();
            let gate = sink.gate(LONG);
            let ticket = gate.register(PinTarget::Direct, Some(1));
            let settler = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_micros(300));
                ticket.settle(PinState::Pinned);
            });
            for i in 0u32..2000 {
                if let Some(b) = gate.submit(i.to_le_bytes().to_vec()) {
                    sink.flushed.lock().unwrap().push(b);
                }
            }
            settler.join().unwrap();
            let got: Vec<u32> = sink
                .flushed()
                .iter()
                .map(|b| u32::from_le_bytes(b.as_slice().try_into().unwrap()))
                .collect();
            assert_eq!(got, (0..2000).collect::<Vec<_>>());
        }
    }

    #[test]
    fn 記録の1行に送った文字を含めない() {
        let gave_up = PinLogEvent::GaveUp {
            target: PinTarget::Direct,
            pid: Some(77),
            elapsed_ms: 3001,
            reason: "AttachConsole err=31".into(),
        };
        let line = gave_up.log_line(12);
        assert!(line.contains("pane=12"), "{line}");
        assert!(line.contains("対象=direct"), "{line}");
        assert!(line.contains("pid=77"), "{line}");
        assert!(line.contains("理由=AttachConsole err=31"), "{line}");

        let limit = PinLogEvent::HoldLimit {
            limit_ms: 500,
            held_writes: 2,
            pending: vec![PinTarget::Direct, PinTarget::Container],
        };
        let line = limit.log_line(3);
        assert!(line.contains("上限 500ms"), "{line}");
        assert!(line.contains("未決着=direct,container"), "{line}");
        assert!(line.contains("流した書き込み=2 件"), "{line}");
    }

    #[test]
    fn 注入はtargetの指定どおりに当たる() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        let none = injection_from(PinTarget::Direct, env(&[]));
        assert_eq!(none, Injection::default());

        let both = &[
            ("TAKO_2092_INJECT_PIN_DELAY_MS", "800"),
            ("TAKO_2092_INJECT_PIN_GIVE_UP", "1"),
        ];
        for target in [PinTarget::Direct, PinTarget::Container] {
            let got = injection_from(target, env(both));
            assert_eq!(got.delay, Some(Duration::from_millis(800)));
            assert!(got.give_up);
        }

        let only_container = &[
            ("TAKO_2092_INJECT_PIN_DELAY_MS", "800"),
            ("TAKO_2092_INJECT_PIN_TARGET", "container"),
        ];
        assert_eq!(
            injection_from(PinTarget::Direct, env(only_container)),
            Injection::default()
        );
        assert_eq!(
            injection_from(PinTarget::Container, env(only_container)).delay,
            Some(Duration::from_millis(800))
        );
        // 壊れた値は注入しない（製品の既定へ倒す）
        let broken = &[("TAKO_2092_INJECT_PIN_DELAY_MS", "abc")];
        assert_eq!(injection_from(PinTarget::Direct, env(broken)).delay, None);
    }

    #[test]
    fn ペインの状態は諦め優先で集約する() {
        let target = |state| TargetReport {
            target: PinTarget::Direct,
            pid: None,
            state,
            settled_ms: None,
        };
        let report = |states: Vec<PinState>| PinReport {
            targets: states.into_iter().map(target).collect(),
            hold: HoldReport {
                held_writes: 0,
                held_bytes: 0,
                release: None,
                held_ms: None,
                limit_ms: 500,
            },
        };
        use PinState::*;
        assert_eq!(report(vec![Pinned, GaveUp("x".into())]).state(), "gave_up");
        assert_eq!(report(vec![Pinned, Pending]).state(), "pending");
        assert_eq!(report(vec![Skipped, Skipped]).state(), "skipped");
        assert_eq!(report(vec![Pinned, Skipped]).state(), "pinned");
    }

    #[cfg(not(windows))]
    #[test]
    fn unixでは門を作らずスレッドも起こさない() {
        let gate = begin(Some(1), None, |_| {});
        assert!(gate.is_none());
        assert!(ContainerPin::for_session("tako-0123456789ab").is_none());
    }
}

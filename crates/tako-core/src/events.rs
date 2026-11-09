//! events — tako の外の購読者へ流す変化の出来事（FR-2.45 / Issue #2731）
//!
//! 外のプロセス（外部の GUI フロントエンド・スクリプト・別の AI）が、ペインの増減・role・タブ名・
//! ターン状態の変化を**ポーリングせずに**受け取るための、メモリだけのリングバッファ。
//!
//! - **書き手は「すでに決まった値の変化を積むだけ」**: ペインの生成と破棄（[`crate::pane::Pane`] の
//!   生成と `Drop`）・role の書き換え（`Pane::set_role`）・タブ名の書き換え（`Tab::set_title*`）・
//!   tako mod の報告の受け口（`ModHub::accept`）。値の比べは書き手が持っている前後の値だけで行い、
//!   画面・ファイル・ペインの一覧を読みに行かない（UI スレッドに読み取りを足さない）
//! - **読み手は `Request::Events`**（CLI `tako events` / MCP `tako_events`）。`since` より後の出来事を
//!   返し、無ければ `wait_ms`（上限 [`MAX_WAIT_MS`]）まで待つ長ポーリング。待つのは受け口の接続の
//!   スレッド（`tako_control::ipc::submit` の手前）で、UI スレッドでは待たない
//! - **本文を載せない**: ペインの内容・送信テキスト・会話の本文・トークンを持つ欄を [`Detail`] に
//!   足さない（番犬 `issue2731_events_no_body`）。会話の正本は transcript
//! - **永続化しない**（[`crate::claude_mod::ModHub`] と同じ判断 = #916 の移行の対象を増やさない）。
//!   GUI の起動ごとに [`EventLog::epoch`] が変わり、`seq` は 1 から振り直す。受け手は epoch が
//!   変わったか `dropped` が真なら一覧を読み直す

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::i18n::Lang;

/// バッファに残す出来事の上限（古いものから捨てる）
pub const CAPACITY: usize = 1000;
/// 長ポーリングの待ちの上限（ms）。超えた値はここへ丸め、丸めた値を応答に載せる。
/// IPC の 1 往復の上限（`protocol::DEFAULT_REPLY_TIMEOUT` = 300 秒）の内側
pub const MAX_WAIT_MS: u64 = 30_000;
/// 同時に待てる長ポーリングの本数。超えた分は断らず、待たずにその時点の結果を返す
pub const MAX_WAITERS: usize = 8;

/// 出来事の種類。**足すときはここと [`Kind::ALL`] と [`Detail`] へ 1 つずつ**（応答の
/// `kinds_known` はここから組む = 受け手は知らない種類を読み飛ばせばよい）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    PaneAdded,
    PaneClosed,
    RoleChanged,
    TabRenamed,
    TurnChanged,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::PaneAdded,
        Kind::PaneClosed,
        Kind::RoleChanged,
        Kind::TabRenamed,
        Kind::TurnChanged,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::PaneAdded => "pane_added",
            Kind::PaneClosed => "pane_closed",
            Kind::RoleChanged => "role_changed",
            Kind::TabRenamed => "tab_renamed",
            Kind::TurnChanged => "turn_changed",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == text.trim())
    }
}

/// 出来事の中身。**本文を持つ欄を足さない**（モジュール先頭）。文字列の欄は role・タブ名と、
/// 固定の語彙（`&'static str`）だけ
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    /// ペインができた（新規・復元）。`role` は復元で持ち越した値（新規は後から `RoleChanged`）
    PaneAdded {
        pane: u64,
        /// 生成主体（`user` / `cli` / `mcp` / `suggestion`）
        origin: &'static str,
        restored: bool,
        role: Option<String>,
    },
    /// ペインが消えた（閉じた・作り直しで捨てた）
    PaneClosed { pane: u64, role: Option<String> },
    RoleChanged {
        pane: u64,
        from: Option<String>,
        to: Option<String>,
    },
    TabRenamed {
        tab: u64,
        from: String,
        to: String,
        /// 書き換えた後のタイトルの出どころ（`default` / `auto` / `manual`）
        source: &'static str,
    },
    /// ターンの状態（`idle` / `busy` / `permission` / `question`）が変わった
    TurnChanged {
        pane: u64,
        /// 前の状態（そのペインで最初の観測なら `None`）
        from: Option<&'static str>,
        to: &'static str,
        /// 判定の出どころ（いまは `mod` = tako mod の報告だけ）
        source: &'static str,
    },
}

impl Detail {
    pub fn kind(&self) -> Kind {
        match self {
            Detail::PaneAdded { .. } => Kind::PaneAdded,
            Detail::PaneClosed { .. } => Kind::PaneClosed,
            Detail::RoleChanged { .. } => Kind::RoleChanged,
            Detail::TabRenamed { .. } => Kind::TabRenamed,
            Detail::TurnChanged { .. } => Kind::TurnChanged,
        }
    }

    /// 出来事のペイン（タブの出来事は `None`）
    pub fn pane(&self) -> Option<u64> {
        match self {
            Detail::PaneAdded { pane, .. }
            | Detail::PaneClosed { pane, .. }
            | Detail::RoleChanged { pane, .. }
            | Detail::TurnChanged { pane, .. } => Some(*pane),
            Detail::TabRenamed { .. } => None,
        }
    }

    /// 出来事のタブ（ペインの出来事は `None`）
    pub fn tab(&self) -> Option<u64> {
        match self {
            Detail::TabRenamed { tab, .. } => Some(*tab),
            _ => None,
        }
    }
}

/// バッファの 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// 起動ごとに 1 から単調に増える通し番号
    pub seq: u64,
    /// 積んだ時刻（unix ms）
    pub at_ms: u64,
    pub detail: Detail,
}

/// 読みの条件
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// これより後（`seq > since`）の出来事を返す
    pub since: u64,
    /// 種類で絞る（空 = すべて）
    pub kinds: Vec<Kind>,
    /// ペインで絞る（空 = すべて。指定するとタブの出来事は入らない）
    pub panes: Vec<u64>,
    /// 該当が無いときに待つ長さ（ms。0 = 待たない。[`MAX_WAIT_MS`] へ丸める）
    pub wait_ms: u64,
}

impl Query {
    fn matches(&self, event: &Event) -> bool {
        (self.kinds.is_empty() || self.kinds.contains(&event.detail.kind()))
            && (self.panes.is_empty()
                || event
                    .detail
                    .pane()
                    .is_some_and(|pane| self.panes.contains(&pane)))
    }
}

/// `since` の出来事の一部がもう読めない理由（[`Page::dropped`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// バッファの上限を超えて古いものを捨てた
    Overflow,
    /// `since` がいまの最新より先 = 別の起動（epoch）の値か、書き間違い
    SinceAhead,
}

impl Dropped {
    pub fn code(self) -> &'static str {
        match self {
            Dropped::Overflow => "overflow",
            Dropped::SinceAhead => "since_ahead",
        }
    }
}

/// 待たずに返した理由（[`Page::wait_skipped`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitSkip {
    /// 同時に待てる本数（[`MAX_WAITERS`]）がふさがっていた
    TooManyWaiters,
    /// 待てない場所から呼ばれた（UI スレッドの dispatch）
    NotOnConnectionThread,
}

impl WaitSkip {
    pub fn code(self) -> &'static str {
        match self {
            WaitSkip::TooManyWaiters => "too_many_waiters",
            WaitSkip::NotOnConnectionThread => "not_on_connection_thread",
        }
    }

    pub fn text_in(self, lang: Lang) -> String {
        match (self, lang) {
            (WaitSkip::TooManyWaiters, Lang::Ja) => format!(
                "同時に待てる長ポーリングは {MAX_WAITERS} 本までで、ふさがっていたので待たずに返した。少し置いてから同じ since で呼び直す"
            ),
            (WaitSkip::TooManyWaiters, Lang::En) => format!(
                "Up to {MAX_WAITERS} long polls can wait at once and all were in use, so this returned without waiting. Call again with the same since after a moment"
            ),
            (WaitSkip::NotOnConnectionThread, Lang::Ja) => {
                "UI スレッドから呼ばれたので待たずに返した（待つのは IPC / MCP の接続のスレッドだけ）".to_string()
            }
            (WaitSkip::NotOnConnectionThread, Lang::En) => {
                "Called from the UI thread, so this returned without waiting (only IPC / MCP connection threads wait)".to_string()
            }
        }
    }
}

/// 待ちの結末（応答には載せない。テストが機構の観測値として見る）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 待たずに返した（該当があった・待たない指定・溢れ・待てない）
    Immediate,
    /// 待っている間に該当が積まれて起きた
    Woke,
    /// 該当が無いまま待ちの長さが尽きた
    TimedOut,
}

/// 読みの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub epoch: String,
    /// 次の `since` に渡す値（返したときのバッファの最新の `seq`。絞り込みで外れた出来事も越える）
    pub cursor: u64,
    pub dropped: Option<Dropped>,
    pub events: Vec<Event>,
    /// 丸めた後の待ちの長さ（ms）
    pub wait_ms: u64,
    /// 求められた待ちが上限を超えていて丸めたか
    pub wait_clamped: bool,
    pub wait_skipped: Option<WaitSkip>,
    pub outcome: Outcome,
}

struct Ring {
    events: VecDeque<Event>,
    /// 最後に積んだ `seq`（0 = まだ無い）
    last_seq: u64,
    /// 上限で捨てた中で最も新しい `seq`（0 = まだ捨てていない）
    evicted_upto: u64,
    /// いま待っている長ポーリングの本数
    waiters: usize,
}

/// 出来事のリングバッファ（プロセスに 1 つは [`log`]。テストは自前のものを作る）
pub struct EventLog {
    ring: Mutex<Ring>,
    /// 積んだら待っている読み手を全員起こす
    cv: Condvar,
    capacity: usize,
    epoch: String,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self::with_epoch(capacity, new_epoch())
    }

    pub fn with_epoch(capacity: usize, epoch: String) -> Self {
        Self {
            ring: Mutex::new(Ring {
                events: VecDeque::new(),
                last_seq: 0,
                evicted_upto: 0,
                waiters: 0,
            }),
            cv: Condvar::new(),
            capacity: capacity.max(1),
            epoch,
        }
    }

    /// このプロセス（GUI の起動）の id
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// 最後に積んだ `seq`
    pub fn cursor(&self) -> u64 {
        self.lock().last_seq
    }

    fn lock(&self) -> MutexGuard<'_, Ring> {
        // 積むのも読むのも panic しない短い区間なので、毒は中身ごと使い続ける
        self.ring.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 1 件積んで、待っている読み手を起こす
    pub fn push(&self, detail: Detail) -> u64 {
        let at_ms = now_ms();
        let mut ring = self.lock();
        ring.last_seq += 1;
        let seq = ring.last_seq;
        ring.events.push_back(Event { seq, at_ms, detail });
        while ring.events.len() > self.capacity {
            if let Some(old) = ring.events.pop_front() {
                ring.evicted_upto = old.seq;
            }
        }
        drop(ring);
        self.cv.notify_all();
        seq
    }

    /// `since` より後の出来事を返す。`can_wait` が偽（UI スレッド）なら決して待たない
    pub fn read(&self, query: &Query, can_wait: bool) -> Page {
        let wait_ms = query.wait_ms.min(MAX_WAIT_MS);
        let wait_clamped = query.wait_ms > MAX_WAIT_MS;
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut ring = self.lock();
        let mut registered = false;
        let mut wait_skipped = None;
        let mut outcome = Outcome::Immediate;
        let (events, dropped) = loop {
            let (events, dropped) = collect(&ring, query);
            if !events.is_empty() || dropped.is_some() || wait_ms == 0 {
                break (events, dropped);
            }
            if !registered {
                if !can_wait {
                    wait_skipped = Some(WaitSkip::NotOnConnectionThread);
                    break (events, dropped);
                }
                if ring.waiters >= MAX_WAITERS {
                    wait_skipped = Some(WaitSkip::TooManyWaiters);
                    break (events, dropped);
                }
                ring.waiters += 1;
                registered = true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                outcome = Outcome::TimedOut;
                break (events, dropped);
            }
            ring = self
                .cv
                .wait_timeout(ring, left)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|p| p.into_inner().0);
            outcome = Outcome::Woke;
        };
        if registered {
            ring.waiters -= 1;
            if events.is_empty() {
                outcome = Outcome::TimedOut;
            }
        }
        Page {
            epoch: self.epoch.clone(),
            cursor: ring.last_seq,
            dropped,
            events,
            wait_ms,
            wait_clamped,
            wait_skipped,
            outcome,
        }
    }

    /// いま待っている長ポーリングの本数
    pub fn waiters(&self) -> usize {
        self.lock().waiters
    }
}

/// `since` より後で条件に合うものと、読めない部分があるか
fn collect(ring: &Ring, query: &Query) -> (Vec<Event>, Option<Dropped>) {
    let dropped = if query.since > ring.last_seq {
        Some(Dropped::SinceAhead)
    } else if query.since < ring.evicted_upto {
        Some(Dropped::Overflow)
    } else {
        None
    };
    let events = ring
        .events
        .iter()
        .filter(|e| e.seq > query.since || dropped == Some(Dropped::SinceAhead))
        .filter(|e| query.matches(e))
        .cloned()
        .collect();
    (events, dropped)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 起動ごとの id（起動の時刻 ms と pid。再起動で必ず変わる）
fn new_epoch() -> String {
    format!("{:x}-{:x}", now_ms(), std::process::id())
}

/// このプロセスの出来事のバッファ
pub fn log() -> &'static EventLog {
    static LOG: OnceLock<EventLog> = OnceLock::new();
    LOG.get_or_init(|| EventLog::new(CAPACITY))
}

thread_local! {
    /// [`capture_on_this_thread`] が開いている間だけ `Some`
    static CAPTURE: std::cell::RefCell<Option<Vec<Detail>>> =
        const { std::cell::RefCell::new(None) };
}

/// 書き手の口（このプロセスのバッファへ積む）
pub fn emit(detail: Detail) {
    CAPTURE.with(|c| {
        if let Some(captured) = c.borrow_mut().as_mut() {
            captured.push(detail.clone());
        }
    });
    log().push(detail);
}

/// このスレッドで積まれた出来事の写しを集める（テスト用）。プロセスで共有のバッファ（[`log`]）は
/// 並走するテストの分も混ざり、上限で古いものから捨てられるので、書き手の性質はこちらで確かめる。
/// 落とすと集めるのをやめる
pub fn capture_on_this_thread() -> Capture {
    CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
    Capture {
        _not_send: std::marker::PhantomData,
    }
}

/// [`capture_on_this_thread`] の集め口（作ったスレッドでだけ使える）
pub struct Capture {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Capture {
    /// ここまでに集めた分を取り出す
    pub fn take(&self) -> Vec<Detail> {
        CAPTURE.with(|c| {
            c.borrow_mut()
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default()
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        CAPTURE.with(|c| *c.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn role(pane: u64, to: &str) -> Detail {
        Detail::RoleChanged {
            pane,
            from: None,
            to: Some(to.to_string()),
        }
    }

    fn tab(tab: u64, to: &str) -> Detail {
        Detail::TabRenamed {
            tab,
            from: "1".into(),
            to: to.into(),
            source: "manual",
        }
    }

    #[test]
    fn seqは単調に増えcursorは最新を指す() {
        let log = EventLog::new(10);
        assert_eq!(log.read(&Query::default(), false).cursor, 0);
        assert_eq!(log.push(role(1, "a")), 1);
        assert_eq!(log.push(role(1, "b")), 2);
        let page = log.read(&Query::default(), false);
        assert_eq!(page.cursor, 2);
        assert_eq!(
            page.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 2]
        );
        let after = log.read(
            &Query {
                since: 1,
                ..Query::default()
            },
            false,
        );
        assert_eq!(after.events.iter().map(|e| e.seq).collect::<Vec<_>>(), [2]);
        assert_eq!(after.dropped, None);
    }

    #[test]
    fn 上限を超えて古いsinceで読むとdroppedが立つ() {
        let log = EventLog::new(3);
        for i in 0..5 {
            log.push(role(1, &i.to_string()));
        }
        let old = log.read(
            &Query {
                since: 1,
                ..Query::default()
            },
            false,
        );
        assert_eq!(old.dropped, Some(Dropped::Overflow));
        assert_eq!(
            old.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [3, 4, 5]
        );
        // 捨てた最後（2）まで読んだ受け手は何も失っていない
        let caught_up = log.read(
            &Query {
                since: 2,
                ..Query::default()
            },
            false,
        );
        assert_eq!(caught_up.dropped, None);
    }

    #[test]
    fn 未来のsinceは別の起動の値として待たずにdropped() {
        let log = EventLog::new(10);
        log.push(role(1, "a"));
        let page = log.read(
            &Query {
                since: 99,
                wait_ms: 5_000,
                ..Query::default()
            },
            true,
        );
        assert_eq!(page.dropped, Some(Dropped::SinceAhead));
        assert_eq!(page.outcome, Outcome::Immediate);
        assert_eq!(page.cursor, 1);
        assert_eq!(page.events.len(), 1, "受け手が読み直せるよう残りを全部返す");
    }

    #[test]
    fn 種類とペインで絞りcursorは外れた出来事も越える() {
        let log = EventLog::new(10);
        log.push(role(1, "a"));
        log.push(tab(7, "x"));
        log.push(role(2, "b"));
        let by_kind = log.read(
            &Query {
                kinds: vec![Kind::TabRenamed],
                ..Query::default()
            },
            false,
        );
        assert_eq!(by_kind.events.len(), 1);
        assert_eq!(by_kind.events[0].detail.tab(), Some(7));
        assert_eq!(by_kind.cursor, 3);
        let by_pane = log.read(
            &Query {
                panes: vec![2],
                ..Query::default()
            },
            false,
        );
        assert_eq!(by_pane.events.len(), 1);
        assert_eq!(by_pane.events[0].detail.pane(), Some(2));
    }

    #[test]
    fn 上限を超えた待ちは丸めて応答に載せる() {
        let log = EventLog::new(10);
        log.push(role(1, "a"));
        let page = log.read(
            &Query {
                wait_ms: MAX_WAIT_MS + 1,
                ..Query::default()
            },
            true,
        );
        assert_eq!(page.wait_ms, MAX_WAIT_MS);
        assert!(page.wait_clamped);
        assert_eq!(page.outcome, Outcome::Immediate, "該当があれば待たない");
    }

    #[test]
    fn 該当が無ければ待ちが尽きて空で返りcursorは変わらない() {
        let log = EventLog::new(10);
        log.push(role(1, "a"));
        let page = log.read(
            &Query {
                since: 1,
                wait_ms: 20,
                ..Query::default()
            },
            true,
        );
        assert!(page.events.is_empty());
        assert_eq!(page.cursor, 1);
        assert_eq!(page.outcome, Outcome::TimedOut);
        assert_eq!(log.waiters(), 0, "待ち終えたら枠を返す");
    }

    #[test]
    fn 待っている間に積まれると起きて返す() {
        let log = Arc::new(EventLog::new(10));
        let reader = {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                log.read(
                    &Query {
                        wait_ms: MAX_WAIT_MS,
                        ..Query::default()
                    },
                    true,
                )
            })
        };
        // 読み手が待ちに入ったのを枠の数で確かめてから積む（時間で待たない）
        while log.waiters() == 0 {
            std::thread::yield_now();
        }
        log.push(role(4, "worker"));
        let page = reader.join().expect("読み手");
        assert_eq!(page.outcome, Outcome::Woke);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.cursor, 1);
        assert_eq!(log.waiters(), 0);
    }

    #[test]
    fn 絞り込みに外れた出来事では起きたまま返らない() {
        let log = Arc::new(EventLog::new(10));
        let reader = {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                log.read(
                    &Query {
                        kinds: vec![Kind::TurnChanged],
                        wait_ms: MAX_WAIT_MS,
                        ..Query::default()
                    },
                    true,
                )
            })
        };
        while log.waiters() == 0 {
            std::thread::yield_now();
        }
        log.push(role(4, "worker"));
        log.push(Detail::TurnChanged {
            pane: 4,
            from: None,
            to: "busy",
            source: "mod",
        });
        let page = reader.join().expect("読み手");
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].detail.kind(), Kind::TurnChanged);
        assert_eq!(page.cursor, 2);
    }

    #[test]
    fn 同時の待ちが上限なら待たずに理由を返す() {
        let log = Arc::new(EventLog::new(10));
        let readers: Vec<_> = (0..MAX_WAITERS)
            .map(|_| {
                let log = Arc::clone(&log);
                std::thread::spawn(move || {
                    log.read(
                        &Query {
                            wait_ms: MAX_WAIT_MS,
                            ..Query::default()
                        },
                        true,
                    )
                })
            })
            .collect();
        while log.waiters() < MAX_WAITERS {
            std::thread::yield_now();
        }
        let extra = log.read(
            &Query {
                wait_ms: MAX_WAIT_MS,
                ..Query::default()
            },
            true,
        );
        assert_eq!(extra.wait_skipped, Some(WaitSkip::TooManyWaiters));
        assert_eq!(extra.outcome, Outcome::Immediate);
        assert!(extra.events.is_empty());
        log.push(role(1, "a"));
        for reader in readers {
            assert_eq!(reader.join().expect("読み手").outcome, Outcome::Woke);
        }
        assert_eq!(log.waiters(), 0);
    }

    #[test]
    fn uiスレッドからは決して待たない() {
        let log = EventLog::new(10);
        let page = log.read(
            &Query {
                wait_ms: MAX_WAIT_MS,
                ..Query::default()
            },
            false,
        );
        assert_eq!(page.wait_skipped, Some(WaitSkip::NotOnConnectionThread));
        assert_eq!(page.outcome, Outcome::Immediate);
        assert_eq!(log.waiters(), 0);
    }

    #[test]
    fn 待ちの0は待たずに返す() {
        let log = EventLog::new(10);
        let page = log.read(&Query::default(), true);
        assert_eq!(page.outcome, Outcome::Immediate);
        assert_eq!(page.wait_skipped, None);
        assert_eq!(page.wait_ms, 0);
        assert!(!page.wait_clamped);
    }

    #[test]
    fn 種類の名前は往復する() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("transcript_appended"), None);
    }

    #[test]
    fn epochは起動ごとに変わる値を持つ() {
        let a = EventLog::new(1);
        let b = EventLog::with_epoch(1, "x".into());
        assert!(!a.epoch().is_empty());
        assert_eq!(b.epoch(), "x");
    }
}

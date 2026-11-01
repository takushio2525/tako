//! PTY IO ループ（`alacritty_terminal::event_loop` の移植。#817）
//!
//! # なぜ自前に持つか
//!
//! upstream の `EventLoop::spawn()` は reader スレッドの**スタック**に 1 MiB の配列
//! （`READ_BUFFER_SIZE`）を置く。`[0u8; 1 MiB]` はゼロ初期化なのでスレッド開始時点で
//! 全ページが dirty になり、**ペイン 1 枚につき約 1.03 MB が常駐**していた
//! （16 ペインで 17 MB。#814 の実測）。
//!
//! この定数は `pub(crate)` なので外から下げられない。スレッドのスタックサイズ指定でも
//! 解決しない（memset された分は reserve ではなく resident なので、上限を絞っても
//! 減るのは仮想サイズだけ）。よってループ自体を持ち込み、**読み取りバッファだけ
//! ヒープへ移す**のが唯一の手になる。
//!
//! # upstream との同一性
//!
//! 挙動は変えない。特に:
//!
//! - 1 回のロック中に処理するのは `MAX_LOCKED_READ` = 64 KiB まで
//! - ロックが取れない間は読み進め、`READ_BUFFER_SIZE` = 1 MiB に達したら
//!   ブロッキングでロックを取る（PTY のバックプレッシャ特性を変えない）
//! - シャットダウンは `Msg::Shutdown` 受信でループを抜け、最後に `deregister` する
//!
//! バッファは `INITIAL_READ_BUFFER_SIZE` = 64 KiB から始め、足りなくなったときだけ
//! 上限まで倍々で伸ばす。ロックが取れている通常経路では `MAX_LOCKED_READ` で
//! 打ち切られるので、**read / parse の回数は upstream と変わらない**。
//! 伸びた分は `pty_read` の最後に初期サイズへ戻すので、一度混雑したペインが
//! 1 MiB を抱えたままにはならない。
//!
//! 意図して変えたのは「子の終了を必ず `ChildExit` で知らせる」ための 1 点（#2114）:
//! unix で書き込みが EIO（子の側 = スレーブが全部閉じた）になっても**ループを畳まない**。
//! upstream はここで畳むので、子の終了の合図（SIGCHLD）より先に書き込みが失敗した回は
//! `Pty` の drop が子を回収し、子の終了が誰にも届かない。tako は書けない入力を捨て、
//! PTY の fd を監視から外して合図を待つ。
//!
//! # 由来とライセンス
//!
//! alacritty_terminal 0.26.0 `src/event_loop.rs`（Apache-2.0）の移植。
//! tako が使っていなかった ref_test（記録ファイル書き出し）と drain_on_exit は落とした。
//! 表記は `THIRD-PARTY-NOTICES.md` を参照。

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{self, Event, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
// `EventedPty` の supertrait として `EventedReadWrite`（register / reader / writer）も入る
use alacritty_terminal::tty::{ChildEvent, EventedPty};
use alacritty_terminal::vte::ansi;
use polling::{Event as PollingEvent, Events, PollMode, Poller};

/// 読み取りバッファの上限（upstream の `READ_BUFFER_SIZE`）。
/// ここに達したらロックをブロッキングで取りに行く
const READ_BUFFER_SIZE: usize = 0x10_0000;

/// 1 回のロック中に処理する最大バイト数（upstream の `MAX_LOCKED_READ`）
const MAX_LOCKED_READ: usize = u16::MAX as usize;

/// 子の終了を観測してから PTY を読み切るときの「静かになった」判定（#1628）。
/// この窓のあいだ 1 バイトも来なければ、もう来ないとみなす
const EXIT_DRAIN_QUIET: Duration = Duration::from_millis(50);

/// 同上の打ち切り上限。ペインが死ぬときに 1 回だけ通る段なので短くてよい（#1628）
const EXIT_DRAIN_MAX: Duration = Duration::from_millis(500);

/// A/B の旧経路アーム（`TAKO_1628_LEGACY=1`）。子の終了時に**読み切らずに畳む** =
/// #817 の移植で落ちていた形へ戻す。検証専用で、既定では通らない
fn legacy_1628_skip_drain() -> bool {
    std::env::var("TAKO_1628_LEGACY").is_ok_and(|v| v == "1")
}

/// A/B の旧経路アーム（`TAKO_2114_LEGACY=1`）。書き込みが「相手側が閉じた」で失敗したら
/// **その場でループを畳む** = #2114 の前の形（upstream と同じ）へ戻す。検証専用で、既定では通らない
fn legacy_2114_break_on_closed_write() -> bool {
    std::env::var("TAKO_2114_LEGACY").is_ok_and(|v| v == "1")
}

/// 注入（`TAKO_2114_INJECT_CHILD_EVENT_DELAY_MS=<ms>`）。子の終了の合図（SIGCHLD）を受けてから
/// 処理するまでを指定の時間だけ遅らせる = 負荷の下で合図の処理が遅れた回を再現する。
/// その間も PTY の読み書きは回る。検証専用で、既定では通らない（#2114）
fn inject_2114_child_event_delay() -> Option<Duration> {
    std::env::var("TAKO_2114_INJECT_CHILD_EVENT_DELAY_MS")
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_millis)
}

/// 書き込みの失敗が「子の側（スレーブ）が全部閉じた」ことを表すか（#2114）。
///
/// unix の master は、スレーブの fd が全部閉じて出力を読み切った後の write が EIO になる
/// （macOS で実測。子が tty を閉じて生きている間も同じ）。子の終了の合図はこれとは別の
/// 経路（SIGCHLD）で後から届くので、この失敗は「子が終わった」の代わりにはならない。
/// Windows（ConPTY）は対象外 = 従来どおり畳む
fn is_peer_closed(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::EIO)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

/// [`Msg::HangUpAndDrain`] で子の側が全部閉じるのを待つ上限（#2625）。
///
/// 器（tmux）の attach クライアントなら、SIGHUP から子の側が全部閉じるまでは実測 10〜25ms。
/// 上限に達したら従来どおり閉じる（= tmux サーバーが最後の close で止まりうる形へ戻るだけ）
pub const HANG_UP_DRAIN_MAX: Duration = Duration::from_secs(2);

/// 同上の待ちで、読むものが無いときに次に読むまで寝る間隔
#[cfg(unix)]
const HANG_UP_DRAIN_POLL: Duration = Duration::from_millis(2);

/// 読み取りバッファの初期サイズ。`MAX_LOCKED_READ` 以上の最小の 2 冪（64 KiB）。
/// ロックが取れている通常経路はこのサイズで完結する
const INITIAL_READ_BUFFER_SIZE: usize = 0x1_0000;

// 初期サイズが 1 ロックあたりの処理量（`MAX_LOCKED_READ`）を下回ると、通常経路でも
// read / parse の回数が upstream より増える（#817 のリスク欄）。上限を超えていると
// 伸長条件に到達できない。どちらもコンパイル時に潰す
const _: () = assert!(INITIAL_READ_BUFFER_SIZE >= MAX_LOCKED_READ);
const _: () = assert!(INITIAL_READ_BUFFER_SIZE < READ_BUFFER_SIZE);

// PTY の read / write と子プロセス終了を区別するトークン。
// Windows 側は upstream が `pub` で出しているのでそのまま使う。
// Unix 側は `pub(crate)` で参照できないため同じ値を置き、
// 値が変わったら気づけるよう `tests::unixのptyトークン値がalacrittyと一致する` で実 PTY を張って検証する。
#[cfg(windows)]
use alacritty_terminal::tty::{PTY_CHILD_EVENT_TOKEN, PTY_READ_WRITE_TOKEN};
#[cfg(not(windows))]
const PTY_READ_WRITE_TOKEN: usize = 0;
#[cfg(not(windows))]
const PTY_CHILD_EVENT_TOKEN: usize = 1;

/// IO ループへ送るメッセージ（upstream の `Msg`）
#[derive(Debug)]
pub enum Msg {
    /// PTY へ書き込むデータ
    Input(Cow<'static, [u8]>),
    /// ループを終了する
    Shutdown,
    /// 子（`child` = PTY 直下の子の pid）へ SIGHUP を送り、子の側（スレーブ）が全部閉じるまで
    /// 親側を読み捨ててからループを終了する（#2625。unix だけ。他は [`Msg::Shutdown`] と同じ）。
    ///
    /// 器（tmux）の attach クライアントのペイン用。`Shutdown` の後の `Pty` の drop は
    /// 「SIGHUP → 子を wait → 親側を閉じる」順で、そのとき親側はもう誰も読んでいない。
    /// クライアントの tty は tmux サーバーも開いていて、その**最後の close はサーバーの中**
    /// （`server_client_lost`）で起き、読まれない出力が残った tty の close が約 600ms 戻らない
    /// = 器のサーバー全体（他の器のペインの画面・tako が器へ打つ命令）がその間止まる。
    /// 親側を読み続けたまま閉じさせれば止まらない。`limit` を過ぎたら待たずに閉じる
    HangUpAndDrain { child: u32, limit: Duration },
    /// PTY をリサイズする
    Resize(WindowSize),
}

/// `LoopSender::send` の失敗理由
#[derive(Debug)]
pub enum SendError {
    /// poller への通知に失敗した
    Io(io::Error),
    /// IO スレッドが既に居ない
    Send(mpsc::SendError<Msg>),
}

impl Display for SendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Io(err) => err.fmt(f),
            SendError::Send(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for SendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SendError::Io(err) => err.source(),
            SendError::Send(err) => err.source(),
        }
    }
}

/// IO ループへの送信口。送信後に poller を叩いて待機中のループを起こす
#[derive(Clone)]
pub struct LoopSender {
    sender: Sender<Msg>,
    poller: Arc<Poller>,
}

impl LoopSender {
    pub fn send(&self, msg: Msg) -> Result<(), SendError> {
        self.sender.send(msg).map_err(SendError::Send)?;
        self.poller.notify().map_err(SendError::Io)
    }
}

/// `event::Notify` / `event::OnResize` を満たす送信ラッパ（upstream の `Notifier`）
#[derive(Clone)]
pub struct Notifier(pub LoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B)
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        // 0 バイトを流すとターミナルが固まる（upstream のガード）
        if bytes.is_empty() {
            return;
        }

        let _ = self.0.send(Msg::Input(bytes));
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size));
    }
}

/// 書き込み途中のバッファ（upstream の `Writing`）
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

impl Writing {
    #[inline]
    fn new(c: Cow<'static, [u8]>) -> Writing {
        Writing {
            source: c,
            written: 0,
        }
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    #[inline]
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    #[inline]
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

/// ループが持つ可変状態（upstream の `State`）
#[derive(Default)]
struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
}

impl State {
    #[inline]
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    #[inline]
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    #[inline]
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    #[inline]
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    #[inline]
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }

    /// 書き込み待ちを全部捨てる（相手側が閉じた PTY へは二度と届かない。#2114）
    #[inline]
    fn clear_writes(&mut self) {
        self.write_list.clear();
        self.writing = None;
    }
}

/// 覗き見できる受信口（upstream の `PeekableReceiver`）
struct PeekableReceiver<T> {
    rx: Receiver<T>,
    peeked: Option<T>,
    /// 送信側が全部落ちた（= セッションが破棄された）。
    /// upstream はここで panic するが、tako は静かにシャットダウンへ倒す
    disconnected: bool,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self {
            rx,
            peeked: None,
            disconnected: false,
        }
    }

    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }

        self.peeked.as_ref()
    }

    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            match self.rx.try_recv() {
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    None
                }
                res => res.ok(),
            }
        }
    }
}

/// PTY の IO と VT パースを回す専用スレッド（upstream の `EventLoop`）
pub struct PtyLoop<T: EventedPty, U: EventListener> {
    poll: Arc<Poller>,
    pty: T,
    rx: PeekableReceiver<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    /// 「まだ処理されていない `Wakeup` が 1 件ある」フラグ（#816）。
    ///
    /// `Wakeup` は「画面が変わったので描き直して」以上の意味を持たない**冪等**な合図
    /// なので、受け手が処理する前に 2 件目を送っても仕事は増えない。それどころか
    /// 1 件ごとに受け手のタスクを起こすコストが実測で **PTY read 1 回あたり
    /// 約 25 万命令**（取り込み経路の 78%）になっていた。立っている間は送らず、
    /// 受け手が処理に取りかかるときに自分で倒す（= 受け手主導のバックプレッシャ）。
    /// **倒すのは「グリッドを読む直前」**でなければならない。倒した後に届いた出力は
    /// 次の `Wakeup` を立て直すので、取りこぼしにならない
    wakeup_pending: Arc<AtomicBool>,
    /// [`Msg::HangUpAndDrain`] を受けて抜けたときの子の pid と待つ上限（#2625）。
    /// **立つのは閉じる知らせで抜けたときだけ**（子の終了の腕・読み書きの失敗で抜けたら立たない）
    hang_up: Option<(u32, Duration)>,
}

impl<T, U> PtyLoop<T, U>
where
    T: EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Send + 'static,
{
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: T,
        wakeup_pending: Arc<AtomicBool>,
    ) -> io::Result<PtyLoop<T, U>> {
        let (tx, rx) = mpsc::channel();
        let poll = Poller::new()?.into();
        Ok(PtyLoop {
            poll,
            pty,
            tx,
            rx: PeekableReceiver::new(rx),
            terminal,
            event_proxy,
            wakeup_pending,
            hang_up: None,
        })
    }

    /// 未処理の `Wakeup` が無いときだけ送る（#816）
    #[inline]
    fn notify_wakeup(&self) {
        if !self.wakeup_pending.swap(true, Ordering::AcqRel) {
            self.event_proxy.send_event(Event::Wakeup);
        }
    }

    pub fn channel(&self) -> LoopSender {
        LoopSender {
            sender: self.tx.clone(),
            poller: self.poll.clone(),
        }
    }

    /// チャネルを空にする。`false` を返したらシャットダウン要求
    fn drain_recv_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                Msg::Input(input) => state.write_list.push_back(input),
                Msg::Resize(window_size) => self.pty.on_resize(window_size),
                Msg::Shutdown => return false,
                Msg::HangUpAndDrain { child, limit } => {
                    self.hang_up = Some((child, limit));
                    return false;
                }
            }
        }

        // 送信側が全部落ちていたらシャットダウン扱い。`TerminalSession::drop` は必ず
        // `Msg::Shutdown` を送るので通常は来ないが、来たときにスレッドを取り残さない
        if self.rx.disconnected {
            return false;
        }

        true
    }

    #[inline]
    fn pty_read(&mut self, state: &mut State, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut unprocessed = 0;
        let mut processed = 0;

        // 次のロックを PTY 読み取り用に予約する
        let _terminal_lease = Some(self.terminal.lease());
        let mut terminal = None;

        loop {
            // 空きが尽きたら上限まで倍々で伸ばす。upstream が最初から 1 MiB を
            // スタックへ確保していた箇所（#817）。ロックが取れていれば
            // `MAX_LOCKED_READ` で抜けるので、ここへ来るのはロック競合時だけ
            if unprocessed == buf.len() && buf.len() < READ_BUFFER_SIZE {
                let grown = buf.len().saturating_mul(2).min(READ_BUFFER_SIZE);
                buf.resize(grown, 0);
            }

            // PTY から読む
            // #816: 取り込み経路の計測を入れるならこの read と下の advance が対象になる
            match self.pty.reader().read(&mut buf[unprocessed..]) {
                // Windows / macOS では読むものが無くなるとここに来る
                Ok(0) if unprocessed == 0 => break,
                Ok(got) => unprocessed += got,
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                        // パースが追いついていて PTY がブロックするなら poll へ戻る
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            // ターミナルのロックを試みる
            let terminal = match &mut terminal {
                Some(terminal) => terminal,
                None => terminal.insert(match self.terminal.try_lock_unfair() {
                    // バッファ上限に達したら待ってでもロックを取る
                    None if unprocessed >= READ_BUFFER_SIZE => self.terminal.lock_unfair(),
                    None => continue,
                    Some(terminal) => terminal,
                }),
            };

            // 受け取ったバイト列をパースする
            state.parser.advance(&mut **terminal, &buf[..unprocessed]);

            processed += unprocessed;
            unprocessed = 0;

            // ターミナルを不必要に長く止めない
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }

        // 伸ばした分は戻す（混雑したペインが 1 MiB を抱え続けないように）
        if buf.len() > INITIAL_READ_BUFFER_SIZE {
            buf.truncate(INITIAL_READ_BUFFER_SIZE);
            buf.shrink_to_fit();
        }

        // 同期更新中に飲み込まれた分しか無ければ再描画は要らない
        if state.parser.sync_bytes_count() < processed && processed > 0 {
            self.notify_wakeup();
        }

        // 呼び手が「まだ残っているか」を判断できるよう、処理したバイト数を返す
        // （子の終了後の読み切り = `drain_after_child_exit` が使う。#1628）
        Ok(processed)
    }

    /// 子の終了を観測したあと、PTY に残っているバイトを**読み切る**（#1628）。
    ///
    /// この段の直後で `break 'event_loop` するため、**ここで読み残した末尾は永久に
    /// 失われる**（以後 PTY を読む者はいない）。子が短命なほど当たりやすく、実測では
    /// Windows CI で `Write-Output` 3 行のうち 1 行目しか画面に入らなかった。
    /// upstream alacritty は同じ位置に `drain_on_exit` の段を持つが、#817 の移植で
    /// 「tako は使っていない」として落ちていた（実際は `..Options::default()` の
    /// 既定 `false` を受け取っていただけで、tako が選んだ挙動ではなかった）。
    ///
    /// 読み手は両 OS とも**非ブロッキング**（Windows は `UnblockedReader::try_read`、
    /// unix は `O_NONBLOCK` の master fd）なので、空なら即 0 が返りここで固まらない。
    /// ただし Windows は別スレッドが中継するので「いま空 = もう来ない」とは限らず、
    /// [`EXIT_DRAIN_QUIET`] のあいだ 1 バイトも来ないことをもって打ち切る
    /// （上限は [`EXIT_DRAIN_MAX`]）。1 回の `pty_read` は `MAX_LOCKED_READ` で
    /// 切り上げるので、**繰り返さないと 64 KiB より後ろが落ちる**
    fn drain_after_child_exit(&mut self, state: &mut State, buf: &mut Vec<u8>) {
        // A/B: #817 の形（読み切らずに畳む）へ戻す旧経路アーム
        if legacy_1628_skip_drain() {
            return;
        }
        let started = Instant::now();
        let mut last_progress = Instant::now();
        while started.elapsed() < EXIT_DRAIN_MAX {
            match self.pty_read(state, buf) {
                Ok(0) => {
                    if last_progress.elapsed() >= EXIT_DRAIN_QUIET {
                        return;
                    }
                    // 中継スレッドが次の塊を渡すまでの間、CPU を回し続けない
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(_) => last_progress = Instant::now(),
                // 読めなくなったらそこまで（PTY は既に畳まれている）
                Err(_) => return,
            }
        }
    }

    /// 子へ SIGHUP を送り、子の側（スレーブ）が全部閉じるまで親側を読み捨てる（#2625）。
    ///
    /// 器（tmux）の attach クライアントは SIGHUP で抜け、tmux サーバーがその tty を閉じる。
    /// その間も親側を読んでいるので、サーバーの close は読まれない出力を抱えて待たない。
    /// 読んだバイトは捨てる（閉じたペインの画面はもう描かない・UI へイベントも送らない）。
    /// 子の側が全部閉じたら macOS は EOF（0）、Linux は EIO が返る。`limit` を過ぎたら待たずに戻る
    #[cfg(unix)]
    fn hang_up_and_drain(&mut self, child: u32, limit: Duration) {
        // SAFETY: kill(2) へ値を渡すだけ。子を回収する（= pid を手放す）のはこのループの
        // 子の終了の腕と `Pty` の drop だけで、ここへ来るのは閉じる知らせで抜けたとき
        // （子の終了の腕を通っていない）かつ `Pty` の drop より前 = pid はまだこの子のもの
        unsafe {
            libc::kill(child as libc::pid_t, libc::SIGHUP);
        }
        let started = Instant::now();
        let mut buf = [0u8; 4096];
        loop {
            match self.pty.reader().read(&mut buf) {
                // 子の側が全部閉じた（macOS）
                Ok(0) => return,
                Ok(_) => {}
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => {}
                    ErrorKind::WouldBlock => {
                        if started.elapsed() >= limit {
                            tracing::warn!(
                                "PTY の子の側が {}ms で閉じなかったので待たずに閉じる（#2625）",
                                limit.as_millis()
                            );
                            return;
                        }
                        std::thread::sleep(HANG_UP_DRAIN_POLL);
                    }
                    // 子の側が全部閉じた（Linux の EIO）か、もう読めない
                    _ => return,
                },
            }
        }
    }

    /// Windows（ConPTY）は従来どおり閉じる（#2625 は unix の tty の close の話）
    #[cfg(not(unix))]
    fn hang_up_and_drain(&mut self, _child: u32, _limit: Duration) {}

    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    }
                    Ok(n) => {
                        current.advance(n);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn spawn(mut self) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("PTY reader".to_owned())
            .spawn(move || {
                let mut state = State::default();
                // upstream はここが `[0u8; 1 MiB]`（スタック）だった（#817）
                let mut buf = vec![0u8; INITIAL_READ_BUFFER_SIZE];

                let poll_opts = PollMode::Level;
                let mut interest = PollingEvent::readable(0);

                // PTY を EventedRW インターフェース経由で登録する
                // Safety: 登録する fd は self.pty が保持し、deregister までは生存する
                if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                    tracing::error!("PTY イベントループの登録に失敗した: {err}");
                    return;
                }

                let mut events =
                    Events::with_capacity(NonZeroUsize::new(1024).expect("1024 は非ゼロ"));
                let mut warned_unknown_token = false;
                // 子の側（スレーブ）が全部閉じた後か（#2114）。立ったら PTY の fd を監視から
                // 外し、届く入力は捨てて、子の終了の合図だけを待つ
                let mut peer_closed = false;
                // 注入（#2114）: 子の終了の合図を遅らせる幅と、遅らせ始めた時刻
                let child_event_delay = inject_2114_child_event_delay();
                let mut child_event_seen: Option<Instant> = None;

                'event_loop: loop {
                    // 同期更新のタイムアウトが来たら poll を起こす
                    let handler = state.parser.sync_timeout();
                    let timeout = handler
                        .sync_timeout()
                        .map(|st| st.saturating_duration_since(Instant::now()));

                    events.clear();
                    if let Err(err) = self.poll.wait(&mut events, timeout) {
                        match err.kind() {
                            ErrorKind::Interrupted => continue,
                            _ => {
                                tracing::error!("PTY イベントループの poll に失敗した: {err}");
                                break 'event_loop;
                            }
                        }
                    }

                    // 同期更新のタイムアウト処理
                    if events.is_empty() && self.rx.peek().is_none() {
                        state.parser.stop_sync(&mut *self.terminal.lock());
                        self.notify_wakeup();
                        continue;
                    }

                    // チャネルに溜まったメッセージを先に処理する
                    if !self.drain_recv_channel(&mut state) {
                        break;
                    }
                    // 相手側が閉じた PTY へは二度と届かない。溜めると捨てられずに積もる（#2114）
                    if peer_closed {
                        state.clear_writes();
                    }

                    for event in events.iter() {
                        match event.key {
                            PTY_CHILD_EVENT_TOKEN => {
                                // 注入（#2114）: 合図を読まずに残して遅らせる。level の poll が
                                // 起こし続けるので 1 ms ずつ寝る（その間も PTY の読み書きは回る）
                                if let Some(delay) = child_event_delay {
                                    let seen = *child_event_seen.get_or_insert_with(Instant::now);
                                    if seen.elapsed() < delay {
                                        std::thread::sleep(Duration::from_millis(1));
                                        continue;
                                    }
                                    child_event_seen = None;
                                }
                                if let Some(ChildEvent::Exited(status)) =
                                    self.pty.next_child_event()
                                {
                                    // **知らせる前に読み切る**（#1628）。この先で PTY を
                                    // 読む者はいないので、順序を逆にすると受け手は
                                    // 「終了した」と分かった時点で末尾を読めない
                                    self.drain_after_child_exit(&mut state, &mut buf);
                                    if let Some(status) = status {
                                        self.event_proxy.send_event(Event::ChildExit(status));
                                    }
                                    self.terminal.lock().exit();
                                    self.event_proxy.send_event(Event::Wakeup);
                                    break 'event_loop;
                                }
                            }

                            PTY_READ_WRITE_TOKEN => {
                                if event.is_interrupt() {
                                    // 死んだ PTY へ IO しない
                                    continue;
                                }

                                if event.readable {
                                    if let Err(err) = self.pty_read(&mut state, &mut buf) {
                                        // Linux ではクライアント側が切れると master の read が
                                        // EIO になる。その場合は Exited を待つためループへ戻る
                                        #[cfg(target_os = "linux")]
                                        if err.raw_os_error() == Some(libc::EIO) {
                                            continue;
                                        }

                                        tracing::error!("PTY の読み取りに失敗した: {err}");
                                        break 'event_loop;
                                    }
                                }

                                if event.writable {
                                    if let Err(err) = self.pty_write(&mut state) {
                                        if !is_peer_closed(&err)
                                            || legacy_2114_break_on_closed_write()
                                        {
                                            tracing::error!("PTY の書き込みに失敗した: {err}");
                                            break 'event_loop;
                                        }
                                        // #2114: 子の側が全部閉じた（子が終わりかけ / tty を
                                        // 閉じた）。ここで畳むと子の終了（ChildExit）を誰も
                                        // 知らせないまま `Pty` の drop が子を回収し、ペインは
                                        // 子の死を知らない。書けない入力は捨て、fd の監視を
                                        // 外して（R / W とも EOF が立ち続けるので、外さないと
                                        // 空回りする）子の終了の合図を待つ。その腕が読み切り
                                        // （#1628）と ChildExit を通常どおり行う
                                        tracing::debug!(
                                            "PTY の相手側が閉じた。子の終了を待つ: {err}"
                                        );
                                        peer_closed = true;
                                        state.clear_writes();
                                        interest.readable = false;
                                        interest.writable = false;
                                        if let Err(err) =
                                            self.pty.reregister(&self.poll, interest, poll_opts)
                                        {
                                            tracing::error!(
                                                "PTY イベントの再登録に失敗した: {err}"
                                            );
                                            break 'event_loop;
                                        }
                                    }
                                }
                            }

                            // 未知のトークン。alacritty 側で値が変わった可能性があるので
                            // 黙って握り潰さず 1 回だけ記録する（`tests` の実 PTY 検証と対）
                            other => {
                                if !warned_unknown_token {
                                    warned_unknown_token = true;
                                    tracing::warn!("PTY イベントの未知のトークン: {other}");
                                }
                            }
                        }
                    }

                    // 書き込み待ちがあれば write interest を登録する
                    // （相手側が閉じた後は fd を監視に戻さない。#2114）
                    let needs_write = state.needs_write();
                    if !peer_closed && needs_write != interest.writable {
                        interest.writable = needs_write;

                        if let Err(err) = self.pty.reregister(&self.poll, interest, poll_opts) {
                            tracing::error!("PTY イベントの再登録に失敗した: {err}");
                            break 'event_loop;
                        }
                    }
                }

                // #2625: 閉じる知らせが器のクライアントを先に帰す形なら、子の側が全部閉じるまで
                // 親側を読み捨ててから閉じる（`Pty` の drop より前 = 親側がまだ開いている）
                if let Some((child, limit)) = self.hang_up.take() {
                    self.hang_up_and_drain(child, limit);
                }

                // 監視対象はここでは drop されないので明示的に外す
                let _ = self.pty.deregister(&self.poll);
            })
            .expect("PTY reader スレッドの起動")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ロック粒度と上限は upstream（alacritty_terminal 0.26.0）と同じ値でなければ
    /// ならない。変えると PTY のバックプレッシャ特性が変わる
    /// （初期サイズとの大小関係はモジュール冒頭の `const _` で compile 時に潰している）
    #[test]
    fn ロック粒度とバッファ上限がupstreamと同じ() {
        assert_eq!(READ_BUFFER_SIZE, 0x10_0000);
        assert_eq!(MAX_LOCKED_READ, u16::MAX as usize);
    }

    /// 伸長は上限で頭打ちになり、`unprocessed >= READ_BUFFER_SIZE` の
    /// ブロッキングロック条件へ必ず到達する（無限ループしない）
    #[test]
    fn 読み取りバッファの伸長は上限で止まる() {
        let mut len = INITIAL_READ_BUFFER_SIZE;
        let mut steps = 0;
        while len < READ_BUFFER_SIZE {
            len = len.saturating_mul(2).min(READ_BUFFER_SIZE);
            steps += 1;
            assert!(steps < 64, "伸長が収束しない");
        }
        assert_eq!(len, READ_BUFFER_SIZE);
    }

    /// Unix の `tty::PTY_*_TOKEN` は `pub(crate)` なので値を写している。
    /// 実 PTY を張って「読み取り可能イベント」と「子プロセス終了イベント」が
    /// 写した値で返ることを確認し、alacritty 更新で値が変わったら落ちるようにする
    #[cfg(unix)]
    #[test]
    fn unixのptyトークン値がalacrittyと一致する() {
        use alacritty_terminal::tty::{self, EventedReadWrite, Options, Shell};
        use std::time::Duration;

        let options = Options {
            shell: Some(Shell::new(
                "/bin/sh".to_string(),
                vec!["-c".to_string(), "printf hi; exit 0".to_string()],
            )),
            ..Options::default()
        };
        let window_size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let mut pty = tty::new(&options, window_size, 0).expect("PTY を張れる");

        let poller: Arc<Poller> = Poller::new().expect("poller を作れる").into();
        // Safety: pty はこの関数のスコープ内で生存し、末尾で deregister する
        unsafe {
            pty.register(&poller, PollingEvent::readable(0), PollMode::Level)
                .expect("PTY を登録できる");
        }

        let mut events = Events::with_capacity(NonZeroUsize::new(16).unwrap());
        let mut saw_readable = false;
        let mut saw_child_exit = false;
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut scratch = [0u8; 256];

        // 本体（`spawn`）と同じトークン分岐で判定する
        while Instant::now() < deadline && !(saw_readable && saw_child_exit) {
            events.clear();
            if poller
                .wait(&mut events, Some(Duration::from_millis(200)))
                .is_err()
            {
                continue;
            }
            for event in events.iter() {
                match event.key {
                    PTY_READ_WRITE_TOKEN if event.readable => {
                        if let Ok(n) = pty.reader().read(&mut scratch) {
                            if n > 0 {
                                saw_readable = true;
                            }
                        }
                    }
                    PTY_CHILD_EVENT_TOKEN => {
                        if matches!(pty.next_child_event(), Some(ChildEvent::Exited(_))) {
                            saw_child_exit = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        // 判定は済んだので後始末。トークンが写し違いだと上の分岐で読み残し・
        // 未回収の子プロセスが残り、`Pty::drop` の `child.wait()` が返らなくなる
        // （= 失敗ではなくハングになる）。ここでトークンに依らず読み切って回収し、
        // アサート前に drop まで済ませる
        let cleanup_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if matches!(pty.reader().read(&mut scratch), Ok(n) if n > 0) {
                continue;
            }
            if saw_child_exit
                || matches!(pty.next_child_event(), Some(ChildEvent::Exited(_)))
                || Instant::now() >= cleanup_deadline
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = pty.deregister(&poller);
        drop(pty);

        assert!(
            saw_readable,
            "PTY_READ_WRITE_TOKEN={PTY_READ_WRITE_TOKEN} で読み取りイベントが来なかった。\
             alacritty_terminal の tty::PTY_READ_WRITE_TOKEN が変わった可能性がある"
        );
        assert!(
            saw_child_exit,
            "PTY_CHILD_EVENT_TOKEN={PTY_CHILD_EVENT_TOKEN} で子プロセス終了イベントが来なかった。\
             alacritty_terminal の tty::PTY_CHILD_EVENT_TOKEN が変わった可能性がある"
        );
    }

    /// #2114 の単体テスト用の部品。実 PTY を包み、子の終了の合図を握れるようにし、
    /// 読み書きの回数を数える
    #[cfg(unix)]
    mod held {
        use super::*;
        use alacritty_terminal::tty::{EventedReadWrite, Pty};
        use std::fs::File;
        use std::sync::atomic::AtomicUsize;

        /// ループが PTY をどう触ったかの数え
        #[derive(Default)]
        pub(super) struct Counts {
            pub reads: AtomicUsize,
            pub read_bytes: AtomicUsize,
            pub writes: AtomicUsize,
            pub eio: AtomicUsize,
            /// 合図を握っている間に `next_child_event` が呼ばれた回数 = ループが回った回数
            pub held: AtomicUsize,
        }

        pub(super) struct CountingReader {
            file: File,
            counts: Arc<Counts>,
        }

        impl Read for CountingReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.counts.reads.fetch_add(1, Ordering::SeqCst);
                let result = self.file.read(buf);
                if let Ok(n) = result {
                    self.counts.read_bytes.fetch_add(n, Ordering::SeqCst);
                }
                result
            }
        }

        pub(super) struct CountingWriter {
            file: File,
            counts: Arc<Counts>,
        }

        impl Write for CountingWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.counts.writes.fetch_add(1, Ordering::SeqCst);
                let result = self.file.write(buf);
                if matches!(&result, Err(e) if e.raw_os_error() == Some(libc::EIO)) {
                    self.counts.eio.fetch_add(1, Ordering::SeqCst);
                }
                result
            }

            fn flush(&mut self) -> io::Result<()> {
                self.file.flush()
            }
        }

        /// `hold` が立っている間、子の終了の合図を**読まずに**握る PTY。
        /// 読み書きは同じ master の複製 fd（状態フラグ = O_NONBLOCK は共有）で行う
        pub(super) struct HeldPty {
            inner: Pty,
            reader: CountingReader,
            writer: CountingWriter,
            hold: Arc<AtomicBool>,
            counts: Arc<Counts>,
        }

        impl HeldPty {
            pub fn new(inner: Pty, hold: Arc<AtomicBool>, counts: Arc<Counts>) -> io::Result<Self> {
                let reader = CountingReader {
                    file: inner.file().try_clone()?,
                    counts: counts.clone(),
                };
                let writer = CountingWriter {
                    file: inner.file().try_clone()?,
                    counts: counts.clone(),
                };
                Ok(Self {
                    inner,
                    reader,
                    writer,
                    hold,
                    counts,
                })
            }

            pub fn child_pid(&self) -> u32 {
                self.inner.child().id()
            }
        }

        impl EventedReadWrite for HeldPty {
            type Reader = CountingReader;
            type Writer = CountingWriter;

            unsafe fn register(
                &mut self,
                poll: &Arc<Poller>,
                interest: PollingEvent,
                mode: PollMode,
            ) -> io::Result<()> {
                // Safety: 呼び手の約束（登録元は deregister まで生きる）をそのまま引き継ぐ
                unsafe { self.inner.register(poll, interest, mode) }
            }

            fn reregister(
                &mut self,
                poll: &Arc<Poller>,
                interest: PollingEvent,
                mode: PollMode,
            ) -> io::Result<()> {
                self.inner.reregister(poll, interest, mode)
            }

            fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
                self.inner.deregister(poll)
            }

            fn reader(&mut self) -> &mut CountingReader {
                &mut self.reader
            }

            fn writer(&mut self) -> &mut CountingWriter {
                &mut self.writer
            }
        }

        impl EventedPty for HeldPty {
            fn next_child_event(&mut self) -> Option<ChildEvent> {
                if self.hold.load(Ordering::SeqCst) {
                    // 合図は level で起こし続けるので、握っている間は 1 ms ずつ寝る
                    self.counts.held.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(1));
                    return None;
                }
                self.inner.next_child_event()
            }
        }

        impl event::OnResize for HeldPty {
            fn on_resize(&mut self, window_size: WindowSize) {
                self.inner.on_resize(window_size);
            }
        }

        /// 端末のイベントをそのまま流す受け手
        #[derive(Clone)]
        pub(super) struct ChanListener(pub mpsc::Sender<Event>);

        impl EventListener for ChanListener {
            fn send_event(&self, event: Event) {
                let _ = self.0.send(event);
            }
        }

        /// 途中で落ちても合図を放してループを畳む（握ったままのループを残さない）
        pub(super) struct Release(pub Arc<AtomicBool>, pub LoopSender);

        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
                let _ = self.1.send(Msg::Shutdown);
            }
        }

        /// `ps` の状態（`Z` = ゾンビ = 子は終わってスレーブは全部閉じた）。居なければ None
        pub(super) fn ps_stat(pid: u32) -> Option<String> {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        }

        /// 条件が立つまで待つ（上限 30 秒で偽）
        pub(super) fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
            for _ in 0..3000 {
                if cond() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        }
    }

    /// #2114: 子の側（スレーブ）が全部閉じた後に書き込みが届いても、ループは畳まずに
    /// 子の終了の合図を待ち、`ChildExit` を送る。
    ///
    /// 負荷の下では「子が終わる → 届いた書き込みが EIO → まだ処理していない子の終了の合図」の
    /// 順になりうる（issue1857 のテストが 60 秒 `ChildExit` を待って落ちた回）。ここでは合図を
    /// **ラッパーで握って**その順を必ず作り、段は時間ではなく状態（ゾンビ・EIO の回数・
    /// ループの周回数）で進める。修正前（`TAKO_2114_LEGACY=1`）は EIO でループが畳まれ、
    /// `Pty` の drop が子を回収するので `ChildExit` が来ないまま終わる
    #[cfg(unix)]
    #[test]
    fn issue2114_子の側が閉じた後の書き込みでもchildexitが届く() {
        use alacritty_terminal::term::{test::TermSize, Config, Term};
        use alacritty_terminal::tty::{self, Options, Shell};
        use held::{ps_stat, wait_until, ChanListener, Counts, HeldPty, Release};
        use std::sync::atomic::AtomicUsize;

        const READY: &str = "TAKO2114READY";
        let options = Options {
            shell: Some(Shell::new(
                "/bin/sh".to_string(),
                vec!["-c".to_string(), format!("printf {READY}; exit 7")],
            )),
            ..Options::default()
        };
        let window_size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let counts = Arc::new(Counts::default());
        let hold = Arc::new(AtomicBool::new(true));
        let pty = HeldPty::new(
            tty::new(&options, window_size, 0).expect("PTY を張れる"),
            hold.clone(),
            counts.clone(),
        )
        .expect("master を複製できる");
        let pid = pty.child_pid();
        let (tx, rx) = mpsc::channel();
        let listener = ChanListener(tx);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &TermSize::new(80, 24),
            listener.clone(),
        )));
        let event_loop = PtyLoop::new(term, listener, pty, Arc::new(AtomicBool::new(false)))
            .expect("ループを作れる");
        let sender = event_loop.channel();
        let _release = Release(hold.clone(), sender.clone());
        let handle = event_loop.spawn();
        let seen = |c: &AtomicUsize| c.load(Ordering::SeqCst);

        // ① 子が出力して終わり（ゾンビ = スレーブは全部閉じた）、ループがその出力を読み切った
        assert!(
            wait_until(|| seen(&counts.read_bytes) >= READY.len()
                && ps_stat(pid).is_some_and(|s| s.starts_with('Z'))),
            "前提: 子が終わり出力を読み切る。読んだ={} 子={:?}",
            seen(&counts.read_bytes),
            ps_stat(pid)
        );

        // ② 合図を握ったまま入力を届ける = 書き込みが EIO になる
        sender
            .send(Msg::Input(Cow::Borrowed(&b"x"[..])))
            .expect("ループへ送れる");
        assert!(
            wait_until(|| seen(&counts.eio) >= 1 || handle.is_finished()),
            "前提: 閉じた PTY への書き込みが EIO になる。書いた={} ループ終了={}",
            seen(&counts.writes),
            handle.is_finished()
        );

        // ③ 畳まずに合図を待っている。その後に届いた入力は書きに行かず、fd を監視から
        // 外したので読みにも行かない（R / W とも EOF が立ち続ける = 外さないと空回りする）
        let reads_at_eio = seen(&counts.reads);
        let writes_at_eio = seen(&counts.writes);
        let held_at_eio = seen(&counts.held);
        let _ = sender.send(Msg::Input(Cow::Borrowed(&b"y"[..])));
        wait_until(|| handle.is_finished() || seen(&counts.held) >= held_at_eio + 50);
        assert!(
            !handle.is_finished(),
            "書き込みの EIO でループが畳まれた = 子の終了（ChildExit）が誰にも届かない。子: {:?}",
            ps_stat(pid)
        );
        assert_eq!(
            seen(&counts.reads) - reads_at_eio,
            0,
            "相手側が閉じた後も PTY を読みに行っている（空回り）"
        );
        assert_eq!(
            seen(&counts.writes) - writes_at_eio,
            0,
            "相手側が閉じた後も PTY へ書きに行っている"
        );

        // ④ 合図を放すと、読み切り → ChildExit（終了コード 7）が届いてループが畳まれる
        hold.store(false, Ordering::SeqCst);
        let mut exit = None;
        wait_until(|| {
            // 畳まれたのを先に見てから受け取る（畳まれた後に送られるイベントは無いので、
            // ここで空なら ChildExit は来ていない）
            let finished = handle.is_finished();
            while let Ok(event) = rx.try_recv() {
                if let Event::ChildExit(status) = event {
                    exit = Some(status);
                }
            }
            exit.is_some() || finished
        });
        let status = exit.unwrap_or_else(|| {
            panic!(
                "子の終了（ChildExit）が届かない。ループ終了={} 子: {:?}",
                handle.is_finished(),
                ps_stat(pid)
            )
        });
        assert_eq!(status.code(), Some(7), "子の終了コードがそのまま届く");
        assert!(
            wait_until(|| handle.is_finished()),
            "ChildExit の後はループが畳まれる"
        );
        assert_eq!(ps_stat(pid), None, "子は回収済み");
    }
}

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
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

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
        })
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
    fn pty_read(&mut self, state: &mut State, buf: &mut Vec<u8>) -> io::Result<()> {
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
            self.event_proxy.send_event(Event::Wakeup);
        }

        Ok(())
    }

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
                        self.event_proxy.send_event(Event::Wakeup);
                        continue;
                    }

                    // チャネルに溜まったメッセージを先に処理する
                    if !self.drain_recv_channel(&mut state) {
                        break;
                    }

                    for event in events.iter() {
                        match event.key {
                            PTY_CHILD_EVENT_TOKEN => {
                                if let Some(ChildEvent::Exited(status)) =
                                    self.pty.next_child_event()
                                {
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
                                        tracing::error!("PTY の書き込みに失敗した: {err}");
                                        break 'event_loop;
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
                    let needs_write = state.needs_write();
                    if needs_write != interest.writable {
                        interest.writable = needs_write;

                        if let Err(err) = self.pty.reregister(&self.poll, interest, poll_opts) {
                            tracing::error!("PTY イベントの再登録に失敗した: {err}");
                            break 'event_loop;
                        }
                    }
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
}

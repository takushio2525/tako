//! TerminalSession — alacritty_terminal + PTY のラッパ（GPUI 非依存）
//!
//! Phase 0 PoC（`poc/03-term-poc`）の検証結果に基づく構成:
//! - alacritty_terminal の tty モジュールで PTY + シェルを spawn
//!   （macOS openpty / Windows ConPTY を同クレートが吸収。portable-pty 不要）
//! - EventLoop（専用 IO スレッド）が PTY 出力をパースして Term グリッドを更新
//! - IO スレッドからのイベントは futures channel で UI 層へ中継し、
//!   UI 層は受け取ったイベントを `process_event` に渡してから再描画する
//!
//! Phase 1 後半でリサイズ・スクロールバック・セル単位（色つき）の読み取りに拡張する。

use std::sync::Arc;

use alacritty_terminal::event::{EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{test::TermSize, Config, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

/// PTY / IO スレッドからのイベント。UI 層はこれを `process_event` へ渡す
pub use alacritty_terminal::event::Event as TermEvent;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("PTY の生成に失敗した")]
    Pty(#[source] std::io::Error),
    #[error("PTY IO スレッドの起動に失敗した")]
    EventLoop(#[source] std::io::Error),
}

/// alacritty の IO スレッドから UI 層へイベントを中継するプロキシ
#[derive(Clone)]
pub struct EventProxy(UnboundedSender<TermEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        // 受信側（UI）が先に破棄されていても IO スレッドは落とさない
        let _ = self.0.unbounded_send(event);
    }
}

/// 1 ペイン分のターミナルセッション（シェルプロセス + VT グリッド）
pub struct TerminalSession {
    term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
    rows: usize,
}

impl TerminalSession {
    /// デフォルトシェルを PTY 上で起動する。
    /// 戻り値のレシーバが流すイベントは UI 層で `process_event` に渡すこと。
    /// セル寸法は PTY の TIOCSWINSZ 用の概算値で、Phase 1 前半は固定でよい
    pub fn spawn(
        cols: usize,
        rows: usize,
    ) -> Result<(Self, UnboundedReceiver<TermEvent>), SessionError> {
        let (tx, rx) = unbounded::<TermEvent>();
        let proxy = EventProxy(tx);

        let term_size = TermSize::new(cols, rows);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &term_size,
            proxy.clone(),
        )));

        let window_size = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: 16,
        };
        let pty = tty::new(&tty::Options::default(), window_size, 0).map_err(SessionError::Pty)?;

        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)
            .map_err(SessionError::EventLoop)?;
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();

        Ok((
            Self {
                term,
                notifier,
                rows,
            },
            rx,
        ))
    }

    /// PTY（シェルの stdin）へバイト列を書き込む
    pub fn write(&self, bytes: Vec<u8>) {
        self.notifier.notify(bytes);
    }

    /// IO スレッドから中継されたイベントを処理する。
    /// PtyWrite（端末からの応答要求）は PTY へ書き戻す。UI 層は処理後に再描画すればよい
    pub fn process_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(text) = event {
            self.notifier.notify(text.into_bytes());
        }
    }

    /// 表示中グリッドを行ごとの文字列へ変換する（色・装飾なし）。
    /// Phase 1 後半でセル単位（色つき）の読み取り API に置き換える
    pub fn visible_lines(&self) -> Vec<String> {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut lines = vec![String::new(); self.rows];
        for indexed in content.display_iter {
            let row = indexed.point.line.0;
            if (0..self.rows as i32).contains(&row) {
                lines[row as usize].push(indexed.cell.c);
            }
        }
        lines
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    }
}

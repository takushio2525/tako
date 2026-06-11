//! TerminalSession — alacritty_terminal + PTY のラッパ（GPUI 非依存）
//!
//! Phase 0 PoC（`poc/03-term-poc`）の検証結果に基づく構成:
//! - alacritty_terminal の tty モジュールで PTY + シェルを spawn
//!   （macOS openpty / Windows ConPTY を同クレートが吸収。portable-pty 不要）
//! - EventLoop（専用 IO スレッド）が PTY 出力をパースして Term グリッドを更新
//! - IO スレッドからのイベントは futures channel で UI 層へ中継し、
//!   UI 層は受け取ったイベントを `process_event` に渡してから再描画する
//!
//! 表示内容の読み取りは色解決済みスナップショット（`screen::snapshot`）で行う。

use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{test::TermSize, viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::tty;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use crate::screen::{self, Screen};
use crate::theme::Theme;

/// PTY / IO スレッドからのイベント。UI 層はこれを `process_event` へ渡す
pub use alacritty_terminal::event::Event as TermEvent;

/// スクロールバックの保持行数
const SCROLLBACK_LINES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("PTY の生成に失敗した")]
    Pty(#[source] std::io::Error),
    #[error("PTY IO スレッドの起動に失敗した")]
    EventLoop(#[source] std::io::Error),
}

/// `process_event` が UI 層へ返す通知（再描画以外の対応が必要なもの）
#[derive(Debug, PartialEq, Eq)]
pub enum SessionNotice {
    /// シェルプロセスが終了した（UI 層はペインを閉じる）
    Exited,
    /// タイトルが変わった（OSC 0/2）
    TitleChanged,
    /// OSC 52 によるクリップボード書き込み要求
    ClipboardStore(String),
}

/// シェルの代わりに起動するコマンド（`tako split -- <command>` 等で使う）
#[derive(Debug, Clone)]
pub struct SpawnCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// セッション起動オプション（FR-2.1.1 / FR-2.2.1）。
/// `env` には UI 層が `TAKO_PANE_ID` 等を詰める。値はログに出さない（`conventions.md`）
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// None ならデフォルトシェルを起動する
    pub command: Option<SpawnCommand>,
    /// 起動時の作業ディレクトリ。None なら継承
    pub cwd: Option<PathBuf>,
    /// 追加で注入する環境変数
    pub env: Vec<(String, String)>,
}

/// マウス選択の種類（クリック回数に対応: 1=文字、2=単語、3=行）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Simple,
    Word,
    Line,
}

impl SelectionKind {
    fn to_alacritty(self) -> SelectionType {
        match self {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
        }
    }
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
    cols: usize,
    rows: usize,
    title: Option<String>,
}

impl TerminalSession {
    /// シェル（または `options.command`）を PTY 上で起動する。
    /// 戻り値のレシーバが流すイベントは UI 層で `process_event` に渡すこと。
    /// セル寸法（px）は PTY の TIOCSWINSZ 用。UI 層が実測値で `resize` し直す前提の初期値
    pub fn spawn(
        cols: usize,
        rows: usize,
        options: SpawnOptions,
    ) -> Result<(Self, UnboundedReceiver<TermEvent>), SessionError> {
        let (tx, rx) = unbounded::<TermEvent>();
        let proxy = EventProxy(tx);

        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term_size = TermSize::new(cols, rows);
        let term = Arc::new(FairMutex::new(Term::new(config, &term_size, proxy.clone())));

        let window_size = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: 16,
        };
        let tty_options = tty::Options {
            shell: options.command.map(|c| tty::Shell::new(c.program, c.args)),
            working_directory: options.cwd,
            env: options.env.into_iter().collect(),
            ..tty::Options::default()
        };
        let pty = tty::new(&tty_options, window_size, 0).map_err(SessionError::Pty)?;

        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)
            .map_err(SessionError::EventLoop)?;
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();

        Ok((
            Self {
                term,
                notifier,
                cols,
                rows,
                title: None,
            },
            rx,
        ))
    }

    /// 現在のグリッドサイズ（cols, rows）
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// OSC 0/2 で設定されたタイトル
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// グリッドと PTY（TIOCSWINSZ）の両方をリサイズする。セル寸法は px
    pub fn resize(&mut self, cols: usize, rows: usize, cell_width: u16, cell_height: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.term.lock().resize(TermSize::new(cols, rows));
        self.notifier.on_resize(WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width,
            cell_height,
        });
        self.cols = cols;
        self.rows = rows;
    }

    /// PTY（シェルの stdin）へバイト列を書き込む。
    /// キー入力時はスクロールバック表示を最下部へ戻す（一般的なターミナルの挙動）
    pub fn write(&self, bytes: Vec<u8>) {
        self.scroll_to_bottom();
        self.notifier.notify(bytes);
    }

    /// クリップボード文字列の貼り付け。アプリが要求していればブラケットペーストで包む
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        self.write(paste_payload(text, bracketed));
    }

    /// スクロールバック表示を行数ぶん動かす（正で過去方向）
    pub fn scroll_display(&self, delta_lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(delta_lines));
    }

    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
    }

    /// 表示座標（col, row）から選択を開始する。`side_right` はセル内の右半分か
    pub fn start_selection(&self, kind: SelectionKind, col: usize, row: usize, side_right: bool) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, col, row);
        term.selection = Some(Selection::new(kind.to_alacritty(), point, side(side_right)));
    }

    /// 選択範囲を表示座標（col, row）まで広げる。選択開始前なら何もしない
    pub fn extend_selection(&self, col: usize, row: usize, side_right: bool) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, col, row);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side(side_right));
        }
    }

    /// 選択中テキストを返す（未選択・空選択なら None）
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|s| !s.is_empty())
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// IO スレッドから中継されたイベントを処理する。
    /// PtyWrite（端末からの応答要求）は PTY へ書き戻す。UI 層は処理後に再描画し、
    /// 戻り値の通知（終了・タイトル変更・クリップボード要求）に対応する
    pub fn process_event(&mut self, event: TermEvent) -> Option<SessionNotice> {
        match event {
            TermEvent::PtyWrite(text) => {
                self.notifier.notify(text.into_bytes());
                None
            }
            TermEvent::Title(title) => {
                self.title = Some(title);
                Some(SessionNotice::TitleChanged)
            }
            TermEvent::ResetTitle => {
                self.title = None;
                Some(SessionNotice::TitleChanged)
            }
            TermEvent::ClipboardStore(_, text) => Some(SessionNotice::ClipboardStore(text)),
            TermEvent::Exit | TermEvent::ChildExit(_) => Some(SessionNotice::Exited),
            _ => None,
        }
    }

    /// 表示中グリッドの色解決済みスナップショット（描画・読み取りの基盤）
    pub fn screen(&self, theme: &Theme) -> Screen {
        screen::snapshot(&self.term.lock(), theme)
    }

    /// 表示行を文字列で返す（装飾なし。セルフテスト・将来の `tako read` 用）
    pub fn visible_lines(&self) -> Vec<String> {
        self.screen(&Theme::default())
            .lines
            .into_iter()
            .map(|l| l.text.trim_end().to_string())
            .collect()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // IO スレッドへ終了を通知する（PTY が drop されシェルにも HUP が届く）
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

fn side(right: bool) -> Side {
    if right {
        Side::Right
    } else {
        Side::Left
    }
}

/// 表示座標（スクロール位置考慮なし）をグリッド座標へ変換する
fn viewport_point(term: &Term<EventProxy>, col: usize, row: usize) -> Point {
    let display_offset = term.grid().display_offset();
    let cols = term.grid().columns();
    let rows = term.grid().screen_lines();
    let point = Point::new(
        row.min(rows.saturating_sub(1)),
        Column(col.min(cols.saturating_sub(1))),
    );
    let mut point = viewport_to_point(display_offset, point);
    // スクロールバック先頭より上は最古行へクランプ
    let topmost = Line(-(term.grid().history_size() as i32));
    if point.line < topmost {
        point.line = topmost;
    }
    point
}

/// ブラケットペーストの payload 生成（改行はキャリッジリターンに正規化する）
fn paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ペースト改行は正規化されブラケットモードで包まれる() {
        assert_eq!(paste_payload("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_payload("a\r\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_payload("x", true), b"\x1b[200~x\x1b[201~".to_vec());
    }
}
